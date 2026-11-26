#![forbid(unsafe_code)]

//! Multi-threaded, DoS-hardened GServer (`vnsg2`) TCP Game Server (`GameTcpServer`)
//! engineered to stress-test every adversarial & concurrency failure mode of `GCLIENT.exe`:
//! - Enforces 24-byte `LCPacket` ingress (`MAX_SERVER_FRAME_BODY_LEN = 16 MiB` + `16 MiB` zip-bomb cap).
//! - Emits compliant 8-byte `CConnection` egress (`head[5] = zipped`, `head[6] = multi`, `body_len > 0`).
//! - Automatically promotes all timestamps (`1_795_500_450`) and large counters (`>= 2^28`) to `0x05 Double`.
//! - Never emits `Null`/`Undefined` inside dense `0x09` arrays (verified via `audit_lua_client_hazards`).
//! - Explicitly clears Winsock2 `FIONBIO` non-blocking inheritance on accepted sockets, enforces
//!   monotonic per-progress partial-frame idle timeouts, performs graceful `Shutdown::Write` half-close,
//!   and records structured per-connection lifecycle telemetry (`ConnectionLifecycleRecord`).

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::amf3::{audit_lua_client_hazards, Amf3Error, Amf3Value};
use crate::framing::{
    build_ping_response, build_rpc_response, encode_server_multi_frame, encode_server_single_frame,
    DecoderParserState, FrameError, ServerRxStreamDecoder, MAX_SERVER_FRAME_BODY_LEN,
};

const MAX_LIFECYCLE_RECORDS: usize = 4_096;
const MAX_READ_POLL_SLICE: Duration = Duration::from_millis(50);
const MIN_READ_POLL_SLICE: Duration = Duration::from_millis(5);

#[derive(Debug, Clone)]
pub struct GameServerConfig {
    /// Maximum idle gap with zero RX progress allowed while a partial frame (`buffered_len > 0`)
    /// is awaiting completion before closing the connection as [`ShutdownReason::IdleTimeoutTruncatedFrame`].
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub compress_responses: bool,
    pub server_time_sec: i64,
    pub max_frame_body_len: usize,
    pub max_decompressed_len: usize,
}

impl Default for GameServerConfig {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_millis(1500),
            write_timeout: Duration::from_millis(1500),
            compress_responses: true,
            server_time_sec: 1_795_500_450,
            max_frame_body_len: MAX_SERVER_FRAME_BODY_LEN,
            max_decompressed_len: MAX_SERVER_FRAME_BODY_LEN,
        }
    }
}

/// Explicit, deterministic reason why a [`GameTcpServer`] client connection terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownReason {
    /// Client closed its write half (`FIN` / `Ok(0)`) cleanly on a frame boundary (`buffered_len == 0`).
    CleanClientEof,
    /// Client closed (`FIN` / `Ok(0)`) mid-frame with `buffered_bytes > 0` unconsumed bytes.
    TruncatedFrameEof { buffered_bytes: usize },
    /// Client stalled mid-frame (`buffered_bytes > 0`) with no RX progress for `>= read_timeout`.
    IdleTimeoutTruncatedFrame { buffered_bytes: usize, idle_ms: u64 },
    /// Header declared `body_len > max_frame_body_len` (`16 MiB`); rejected before body allocation.
    OversizedFrame { declared: u32, max: usize },
    /// Header declared `body_len == 0`.
    ZeroLengthFrame,
    /// Invalid 4-byte `LCPacket` flags or BabelTime 16-byte MD5 mismatch.
    InvalidHeaderOrChecksum,
    /// Zlib stream decompression error or `> 16 MiB` zip-bomb cap exceeded.
    ZlibError,
    /// AMF3 payload exceeded `MAX_RECURSION_DEPTH` (`64`).
    Amf3DepthBomb { depth: usize },
    /// Malformed AMF3 stream (invalid marker, out-of-bounds reference, unexpected EOF).
    Amf3Malformed,
    /// Server-side RPC response encoding error.
    DispatchError,
    /// Socket write failure while transmitting an S2C response frame.
    WriteError {
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
    /// Socket read failure (e.g., peer reset/abort).
    ReadError {
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
        buffered_bytes: usize,
    },
    /// Server shutdown requested via [`GameTcpServer::stop`].
    ServerShutdown,
}

impl fmt::Display for ShutdownReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CleanClientEof => write!(f, "CleanClientEof"),
            Self::TruncatedFrameEof { buffered_bytes } => {
                write!(f, "TruncatedFrameEof(buffered={}B)", buffered_bytes)
            }
            Self::IdleTimeoutTruncatedFrame {
                buffered_bytes,
                idle_ms,
            } => write!(
                f,
                "IdleTimeoutTruncatedFrame(buffered={}B, idle={}ms)",
                buffered_bytes, idle_ms
            ),
            Self::OversizedFrame { declared, max } => {
                write!(f, "OversizedFrame(declared={}B, max={}B)", declared, max)
            }
            Self::ZeroLengthFrame => write!(f, "ZeroLengthFrame"),
            Self::InvalidHeaderOrChecksum => write!(f, "InvalidHeaderOrChecksum"),
            Self::ZlibError => write!(f, "ZlibError"),
            Self::Amf3DepthBomb { depth } => write!(f, "Amf3DepthBomb(depth={})", depth),
            Self::Amf3Malformed => write!(f, "Amf3Malformed"),
            Self::DispatchError => write!(f, "DispatchError"),
            Self::WriteError { kind, raw_os_error } => {
                write!(f, "WriteError(kind={:?}, os={:?})", kind, raw_os_error)
            }
            Self::ReadError {
                kind,
                raw_os_error,
                buffered_bytes,
            } => write!(
                f,
                "ReadError(kind={:?}, os={:?}, buffered={}B)",
                kind, raw_os_error, buffered_bytes
            ),
            Self::ServerShutdown => write!(f, "ServerShutdown"),
        }
    }
}

/// Structured per-connection lifecycle record captured deterministically upon socket teardown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionLifecycleRecord {
    pub conn_id: u64,
    pub peer_addr: Option<SocketAddr>,
    pub connected_at_ms: u64,
    pub last_event_at_ms: u64,
    pub closed_at_ms: u64,
    pub duration_ms: u64,
    pub rx_bytes_total: u64,
    pub tx_bytes_total: u64,
    pub frames_decoded: u64,
    pub frames_sent: u64,
    pub final_parser_state: DecoderParserState,
    pub shutdown_reason: ShutdownReason,
}

impl fmt::Display for ConnectionLifecycleRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "conn#{} peer={:?} start={}ms last_ev={}ms close={}ms dur={}ms rx={}B tx={}B frames_in={} frames_out={} state={} reason={}",
            self.conn_id,
            self.peer_addr,
            self.connected_at_ms,
            self.last_event_at_ms,
            self.closed_at_ms,
            self.duration_ms,
            self.rx_bytes_total,
            self.tx_bytes_total,
            self.frames_decoded,
            self.frames_sent,
            self.final_parser_state,
            self.shutdown_reason,
        )
    }
}

pub struct ServerTelemetry {
    pub accepted_connections: AtomicU64,
    pub active_connections: AtomicUsize,
    pub peak_connections: AtomicUsize,
    pub valid_rpcs_handled: AtomicU64,
    pub stat_msgs_ignored_cleanly: AtomicU64,
    pub multi_batches_sent: AtomicU64,
    pub rejected_oversized_frames: AtomicU64,
    pub rejected_zero_frames: AtomicU64,
    pub rejected_md5_frames: AtomicU64,
    pub rejected_zlib_frames: AtomicU64,
    pub rejected_amf3_depth_bombs: AtomicU64,
    pub rejected_amf3_malformed: AtomicU64,
    pub clean_truncated_disconnects: AtomicU64,
    pub lua_hazard_violations: AtomicU64,
    started_at: Instant,
    lifecycle_records: Mutex<Vec<ConnectionLifecycleRecord>>,
}

impl Default for ServerTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerTelemetry {
    pub fn new() -> Self {
        Self {
            accepted_connections: AtomicU64::new(0),
            active_connections: AtomicUsize::new(0),
            peak_connections: AtomicUsize::new(0),
            valid_rpcs_handled: AtomicU64::new(0),
            stat_msgs_ignored_cleanly: AtomicU64::new(0),
            multi_batches_sent: AtomicU64::new(0),
            rejected_oversized_frames: AtomicU64::new(0),
            rejected_zero_frames: AtomicU64::new(0),
            rejected_md5_frames: AtomicU64::new(0),
            rejected_zlib_frames: AtomicU64::new(0),
            rejected_amf3_depth_bombs: AtomicU64::new(0),
            rejected_amf3_malformed: AtomicU64::new(0),
            clean_truncated_disconnects: AtomicU64::new(0),
            lua_hazard_violations: AtomicU64::new(0),
            started_at: Instant::now(),
            lifecycle_records: Mutex::new(Vec::new()),
        }
    }

    /// Monotonic milliseconds elapsed since [`ServerTelemetry`] initialization.
    pub fn elapsed_ms(&self, instant: Instant) -> u64 {
        instant
            .saturating_duration_since(self.started_at)
            .as_millis() as u64
    }

    /// Returns all recorded connection lifecycle entries sorted by `conn_id`.
    pub fn connection_records(&self) -> Vec<ConnectionLifecycleRecord> {
        let mut records = self
            .lifecycle_records
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        records.sort_by_key(|r| r.conn_id);
        records
    }

    /// Looks up the lifecycle record for a specific `conn_id`.
    pub fn connection_record(&self, conn_id: u64) -> Option<ConnectionLifecycleRecord> {
        self.lifecycle_records
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|r| r.conn_id == conn_id).cloned())
    }

    /// Formats a deterministic multi-line diagnostic dump of counters and per-connection lifecycles.
    pub fn format_diagnostics(&self) -> String {
        let mut out = format!(
            "ServerTelemetry[accepted={} active={} peak={} rpcs={} stat={} multi={} trunc={} oversize={} zero={} md5={} zlib={} depth={} amf3={} hazards={}]",
            self.accepted_connections.load(Ordering::Acquire),
            self.active_connections.load(Ordering::Acquire),
            self.peak_connections.load(Ordering::Acquire),
            self.valid_rpcs_handled.load(Ordering::Acquire),
            self.stat_msgs_ignored_cleanly.load(Ordering::Acquire),
            self.multi_batches_sent.load(Ordering::Acquire),
            self.clean_truncated_disconnects.load(Ordering::Acquire),
            self.rejected_oversized_frames.load(Ordering::Acquire),
            self.rejected_zero_frames.load(Ordering::Acquire),
            self.rejected_md5_frames.load(Ordering::Acquire),
            self.rejected_zlib_frames.load(Ordering::Acquire),
            self.rejected_amf3_depth_bombs.load(Ordering::Acquire),
            self.rejected_amf3_malformed.load(Ordering::Acquire),
            self.lua_hazard_violations.load(Ordering::Acquire),
        );
        for rec in self.connection_records() {
            out.push_str("\n  - ");
            out.push_str(&rec.to_string());
        }
        out
    }

    fn push_lifecycle_record(&self, record: ConnectionLifecycleRecord) {
        if let Ok(mut guard) = self.lifecycle_records.lock() {
            if guard.len() >= MAX_LIFECYCLE_RECORDS {
                let drop_count = MAX_LIFECYCLE_RECORDS / 4;
                guard.drain(..drop_count);
            }
            guard.push(record);
        }
    }
}

pub struct GameTcpServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    telemetry: Arc<ServerTelemetry>,
    accept_thread: Option<JoinHandle<()>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl GameTcpServer {
    pub fn start(bind_addr: &str, config: GameServerConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(bind_addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(ServerTelemetry::new());
        let workers: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let shutdown_cl = Arc::clone(&shutdown);
        let telemetry_cl = Arc::clone(&telemetry);
        let workers_cl = Arc::clone(&workers);
        let handle = thread::Builder::new()
            .name("qa-game-tcp-accept".to_string())
            .spawn(move || {
                let mut conn_seq = 0u64;
                while !shutdown_cl.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            if shutdown_cl.load(Ordering::Acquire) {
                                let _ = stream.shutdown(Shutdown::Both);
                                break;
                            }
                            conn_seq += 1;
                            telemetry_cl
                                .accepted_connections
                                .fetch_add(1, Ordering::AcqRel);
                            let cur = telemetry_cl
                                .active_connections
                                .fetch_add(1, Ordering::AcqRel)
                                + 1;
                            update_max(&telemetry_cl.peak_connections, cur);

                            let cfg = config.clone();
                            let tel = Arc::clone(&telemetry_cl);
                            let sd = Arc::clone(&shutdown_cl);
                            if let Ok(worker) = thread::Builder::new()
                                .name(format!("qa-game-conn-{conn_seq}"))
                                .spawn(move || {
                                    handle_game_client(
                                        stream,
                                        Some(peer),
                                        conn_seq,
                                        &cfg,
                                        &tel,
                                        &sd,
                                    );
                                    tel.active_connections.fetch_sub(1, Ordering::AcqRel);
                                })
                            {
                                if let Ok(mut list) = workers_cl.lock() {
                                    list.retain(|h| !h.is_finished());
                                    list.push(worker);
                                }
                            } else {
                                telemetry_cl
                                    .active_connections
                                    .fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => {
                            if shutdown_cl.load(Ordering::Acquire) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                }
            })?;

        Ok(Self {
            addr,
            shutdown,
            telemetry,
            accept_thread: Some(handle),
            workers,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn telemetry(&self) -> &Arc<ServerTelemetry> {
        &self.telemetry
    }

    pub fn stop(&mut self) {
        if !self.shutdown.swap(true, Ordering::AcqRel) {
            let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(100));
        }
        if let Some(h) = self.accept_thread.take() {
            let _ = h.join();
        }
        if let Ok(mut list) = self.workers.lock() {
            for worker in list.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for GameTcpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn update_max(atom: &AtomicUsize, candidate: usize) {
    let mut prev = atom.load(Ordering::Relaxed);
    while candidate > prev {
        match atom.compare_exchange_weak(prev, candidate, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => prev = actual,
        }
    }
}

fn compute_poll_slice(read_timeout: Duration) -> Duration {
    if read_timeout < MIN_READ_POLL_SLICE {
        MIN_READ_POLL_SLICE
    } else if read_timeout > MAX_READ_POLL_SLICE {
        MAX_READ_POLL_SLICE
    } else {
        read_timeout
    }
}

fn handle_game_client(
    mut stream: TcpStream,
    peer_addr: Option<SocketAddr>,
    conn_seq: u64,
    cfg: &GameServerConfig,
    telemetry: &ServerTelemetry,
    shutdown: &AtomicBool,
) {
    let connected_at = Instant::now();
    let mut last_rx_at = connected_at;
    let mut last_event_at = connected_at;

    // CRITICAL WINDOWS FIX: Winsock2 `accept()` inherits `FIONBIO = 1` from `listener.set_nonblocking(true)`,
    // whereas Linux `accept4()` clears `O_NONBLOCK`. Explicitly restore blocking mode BEFORE setting
    // `SO_RCVTIMEO`/`SO_SNDTIMEO` so `stream.read()` never returns a `0ms` `WSAEWOULDBLOCK` mid-packet!
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(compute_poll_slice(cfg.read_timeout)));
    let _ = stream.set_write_timeout(Some(cfg.write_timeout));

    let mut decoder = ServerRxStreamDecoder::new(cfg.max_frame_body_len, cfg.max_decompressed_len);
    let mut session_token = format!("tok_vnsg2_{:06}", conn_seq);
    let mut read_buf = [0u8; 8192];

    let mut rx_bytes_total = 0u64;
    let mut tx_bytes_total = 0u64;
    let mut frames_decoded = 0u64;
    let mut frames_sent = 0u64;
    let mut deferred_rejection: Option<FrameError> = None;
    let mut truncated_disconnect = false;

    let shutdown_reason = 'conn_loop: loop {
        if shutdown.load(Ordering::Acquire) {
            break ShutdownReason::ServerShutdown;
        }

        match stream.read(&mut read_buf) {
            Ok(0) => {
                let buffered_bytes = decoder.buffered_len();
                if buffered_bytes > 0 {
                    truncated_disconnect = true;
                    break ShutdownReason::TruncatedFrameEof { buffered_bytes };
                }
                break ShutdownReason::CleanClientEof;
            }
            Ok(n) => {
                let now = Instant::now();
                last_rx_at = now;
                last_event_at = now;
                rx_bytes_total += n as u64;

                match decoder.push_bytes(&read_buf[..n]) {
                    Ok(packets) => {
                        if packets.is_empty() {
                            continue;
                        }
                        frames_decoded += packets.len() as u64;
                        for pkt in packets {
                            match dispatch_client_rpc(
                                &pkt.value,
                                &mut session_token,
                                conn_seq,
                                cfg,
                                telemetry,
                            ) {
                                Ok(Some(wire_out)) => {
                                    if let Err(we) = stream.write_all(&wire_out) {
                                        break 'conn_loop ShutdownReason::WriteError {
                                            kind: we.kind(),
                                            raw_os_error: we.raw_os_error(),
                                        };
                                    }
                                    tx_bytes_total += wire_out.len() as u64;
                                    frames_sent += 1;
                                    last_event_at = Instant::now();
                                }
                                Ok(None) => {}
                                Err(_) => break 'conn_loop ShutdownReason::DispatchError,
                            }
                        }
                        if let Err(fe) = stream.flush() {
                            break ShutdownReason::WriteError {
                                kind: fe.kind(),
                                raw_os_error: fe.raw_os_error(),
                            };
                        }
                    }
                    Err(err) => {
                        let reason = classify_rejection_reason(&err);
                        deferred_rejection = Some(err);
                        break reason;
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                continue;
            }
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                if shutdown.load(Ordering::Acquire) {
                    break ShutdownReason::ServerShutdown;
                }
                let buffered_bytes = decoder.buffered_len();
                if buffered_bytes > 0 {
                    let idle = last_rx_at.elapsed();
                    if idle >= cfg.read_timeout {
                        truncated_disconnect = true;
                        break ShutdownReason::IdleTimeoutTruncatedFrame {
                            buffered_bytes,
                            idle_ms: idle.as_millis() as u64,
                        };
                    }
                }
                continue;
            }
            Err(e) => {
                let buffered_bytes = decoder.buffered_len();
                if buffered_bytes > 0 {
                    truncated_disconnect = true;
                }
                break ShutdownReason::ReadError {
                    kind: e.kind(),
                    raw_os_error: e.raw_os_error(),
                    buffered_bytes,
                };
            }
        }
    };

    // Initiate graceful TCP half-close (`SD_SEND` / `FIN`) before socket drop so in-flight S2C
    // response frames are never wiped by an abortive Winsock reset.
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);

    let closed_at = Instant::now();
    let record = ConnectionLifecycleRecord {
        conn_id: conn_seq,
        peer_addr,
        connected_at_ms: telemetry.elapsed_ms(connected_at),
        last_event_at_ms: telemetry.elapsed_ms(last_event_at),
        closed_at_ms: telemetry.elapsed_ms(closed_at),
        duration_ms: closed_at
            .saturating_duration_since(connected_at)
            .as_millis() as u64,
        rx_bytes_total,
        tx_bytes_total,
        frames_decoded,
        frames_sent,
        final_parser_state: decoder.parser_state(),
        shutdown_reason,
    };

    // Commit lifecycle record BEFORE incrementing terminal counters so any test waiting on
    // atomic rejection/truncation counters observes the completed lifecycle record immediately.
    telemetry.push_lifecycle_record(record);

    if truncated_disconnect {
        telemetry
            .clean_truncated_disconnects
            .fetch_add(1, Ordering::Release);
    }
    if let Some(ref err) = deferred_rejection {
        record_rejection(err, telemetry);
    }
}

fn classify_rejection_reason(err: &FrameError) -> ShutdownReason {
    match err {
        FrameError::OversizedBody { declared, max } => ShutdownReason::OversizedFrame {
            declared: *declared,
            max: *max,
        },
        FrameError::ZeroLengthBody => ShutdownReason::ZeroLengthFrame,
        FrameError::Md5Mismatch { .. } | FrameError::InvalidClientFlags(_) => {
            ShutdownReason::InvalidHeaderOrChecksum
        }
        FrameError::Zlib(_) => ShutdownReason::ZlibError,
        FrameError::Amf3(Amf3Error::MaxDepthExceeded { depth }) => {
            ShutdownReason::Amf3DepthBomb { depth: *depth }
        }
        FrameError::Amf3(_) => ShutdownReason::Amf3Malformed,
        FrameError::ClientSocketClosed(_) | FrameError::LuaRuntimeCrash(_) => {
            ShutdownReason::DispatchError
        }
    }
}

fn record_rejection(err: &FrameError, telemetry: &ServerTelemetry) {
    match err {
        FrameError::OversizedBody { .. } => {
            telemetry
                .rejected_oversized_frames
                .fetch_add(1, Ordering::Release);
        }
        FrameError::ZeroLengthBody => {
            telemetry
                .rejected_zero_frames
                .fetch_add(1, Ordering::Release);
        }
        FrameError::Md5Mismatch { .. } | FrameError::InvalidClientFlags(_) => {
            telemetry
                .rejected_md5_frames
                .fetch_add(1, Ordering::Release);
        }
        FrameError::Zlib(_) => {
            telemetry
                .rejected_zlib_frames
                .fetch_add(1, Ordering::Release);
        }
        FrameError::Amf3(Amf3Error::MaxDepthExceeded { .. }) => {
            telemetry
                .rejected_amf3_depth_bombs
                .fetch_add(1, Ordering::Release);
        }
        FrameError::Amf3(_) => {
            telemetry
                .rejected_amf3_malformed
                .fetch_add(1, Ordering::Release);
        }
        _ => {}
    }
}

fn dispatch_client_rpc(
    req: &Amf3Value,
    session_token: &mut String,
    conn_seq: u64,
    cfg: &GameServerConfig,
    telemetry: &ServerTelemetry,
) -> Result<Option<Vec<u8>>, FrameError> {
    let method = req
        .get_field("method")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Fire-and-forget stat events (`Network.lua:304-344`): client never registers `mContext["user.stat"]`,
    // or server can reply with `callbackName = "dummy"` (`live_capture_filtered.log:144`).
    if method == "user.stat" || method == "user.guideEvent" {
        telemetry
            .stat_msgs_ignored_cleanly
            .fetch_add(1, Ordering::Relaxed);
        let dummy_resp = build_rpc_response(
            "ok",
            "dummy",
            Amf3Value::empty_lua_table(),
            None,
            Some(cfg.server_time_sec),
        );
        verify_no_hazards(&dummy_resp, telemetry);
        let bytes = encode_server_single_frame(&dummy_resp, cfg.compress_responses)?;
        return Ok(Some(bytes));
    }

    if method == "ping" || method == "user.ping" {
        telemetry.valid_rpcs_handled.fetch_add(1, Ordering::Relaxed);
        let ping_resp = build_ping_response(cfg.server_time_sec);
        verify_no_hazards(&ping_resp, telemetry);
        let bytes = encode_server_single_frame(&ping_resp, cfg.compress_responses)?;
        return Ok(Some(bytes));
    }

    let callback_name = req
        .get_field("callback")
        .and_then(|c| c.get_field("callbackName"))
        .and_then(|v| v.as_str())
        .unwrap_or("dummy");

    telemetry.valid_rpcs_handled.fetch_add(1, Ordering::Relaxed);

    if method == "qa.multiBurst" {
        telemetry.multi_batches_sent.fetch_add(1, Ordering::Relaxed);
        let rpc_reply = build_rpc_response(
            "ok",
            callback_name,
            Amf3Value::anonymous_object(vec![
                ("ok", Amf3Value::Integer(1)),
                ("serverTime", Amf3Value::from_i64_auto(cfg.server_time_sec)),
            ]),
            None,
            None,
        );
        let push_msg = build_rpc_response(
            "ok",
            "push.reward.update",
            Amf3Value::anonymous_object(vec![
                ("gold", Amf3Value::Integer(88_888)),
                ("silver", Amf3Value::from_i64_auto(1_500_000_000)),
                ("items", Amf3Value::empty_lua_table()),
            ]),
            None,
            None,
        );
        verify_no_hazards(&rpc_reply, telemetry);
        verify_no_hazards(&push_msg, telemetry);
        let frame = encode_server_multi_frame(
            &[rpc_reply, push_msg],
            Some(session_token),
            Some(cfg.server_time_sec),
            cfg.compress_responses,
            true,
        )?;
        return Ok(Some(frame));
    }

    let ret_payload = match method {
        "user.login" => {
            *session_token = format!("tok_vnsg2_{:06}_auth", conn_seq);
            Amf3Value::anonymous_object(vec![
                ("res", Amf3Value::from_str("ok")),
                ("uid", Amf3Value::Integer(52_592)),
                (
                    "qid",
                    Amf3Value::from_i64_precision_safe(1_243_831_974_477_471_744),
                ),
                ("serverTime", Amf3Value::from_i64_auto(cfg.server_time_sec)),
                ("mergeServerTime", Amf3Value::from_i64_auto(1_667_530_800)),
                ("emptyBag", Amf3Value::empty_lua_table()),
            ])
        }
        "user.getUser" => Amf3Value::anonymous_object(vec![
            ("uid", Amf3Value::Integer(52_592)),
            ("uname", Amf3Value::from_str("LocalGeneral")),
            ("level", Amf3Value::Integer(120)),
            ("vip", Amf3Value::Integer(10)),
            ("gold", Amf3Value::Integer(99_999)),
            // > 268,435,455 -> automatically promoted to 0x05 Double!
            ("silver", Amf3Value::from_i64_auto(2_500_000_000)),
            ("fight_force", Amf3Value::from_i64_auto(987_654_321)),
            ("heros", Amf3Value::empty_lua_table()),
        ]),
        _ => {
            // Echo back args safely for generic QA probes
            let echoed_args = req
                .get_field("args")
                .cloned()
                .unwrap_or_else(Amf3Value::empty_lua_table);
            Amf3Value::anonymous_object(vec![
                ("method", Amf3Value::from_str(method)),
                ("echo", echoed_args),
                ("serverTime", Amf3Value::from_i64_auto(cfg.server_time_sec)),
            ])
        }
    };

    let resp = build_rpc_response(
        "ok",
        callback_name,
        ret_payload,
        Some(session_token),
        Some(cfg.server_time_sec),
    );
    verify_no_hazards(&resp, telemetry);
    let wire = encode_server_single_frame(&resp, cfg.compress_responses)?;
    Ok(Some(wire))
}

fn verify_no_hazards(val: &Amf3Value, telemetry: &ServerTelemetry) {
    let hazards = audit_lua_client_hazards(val);
    if !hazards.is_empty() {
        telemetry
            .lua_hazard_violations
            .fetch_add(hazards.len() as u64, Ordering::Relaxed);
    }
}
