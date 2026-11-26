//! Multi-threaded TCP AMF3 / `LCPacket` game server listener for GServer (`vnsg2`).

use std::io::{self, Read};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gserver_protocol::{
    C2sFrameMode, C2sParserState, ClientRxDecoder, FrameError, ProtocolError, RpcRequest,
};

use crate::world::{ConnectionSession, ServerWorld};

const MAX_TCP_LIFECYCLE_RECORDS: usize = 2_048;
const READ_POLL_SLICE: Duration = Duration::from_millis(50);
const PARTIAL_FRAME_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

/// Deterministic shutdown classification for [`TcpGameServerHandle`] client connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcpShutdownReason {
    CleanClientEof,
    TruncatedFrameEof {
        buffered_bytes: usize,
    },
    IdleTimeoutTruncatedFrame {
        buffered_bytes: usize,
        idle_ms: u64,
    },
    OversizedFrame {
        declared: u32,
        max: u32,
    },
    ProtocolRejected(String),
    InvalidRpcPayload,
    WriteError(io::ErrorKind),
    ReadError {
        kind: io::ErrorKind,
        buffered_bytes: usize,
    },
    ServerShutdown,
}

/// Per-connection lifecycle record captured by [`TcpGameServerHandle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpConnectionRecord {
    pub conn_id: u64,
    pub peer_addr: Option<SocketAddr>,
    pub connected_at_ms: u64,
    pub last_event_at_ms: u64,
    pub closed_at_ms: u64,
    pub rx_bytes_total: u64,
    pub frames_decoded: u64,
    pub responses_sent: u64,
    pub final_parser_state: C2sParserState,
    pub shutdown_reason: TcpShutdownReason,
}

/// Structured server-wide TCP connection telemetry for `gserver`.
pub struct TcpServerTelemetry {
    pub accepted_connections: AtomicU64,
    pub active_connections: AtomicU64,
    pub clean_truncated_disconnects: AtomicU64,
    pub rejected_oversized_frames: AtomicU64,
    pub rejected_protocol_frames: AtomicU64,
    started_at: Instant,
    records: Mutex<Vec<TcpConnectionRecord>>,
}

impl Default for TcpServerTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpServerTelemetry {
    pub fn new() -> Self {
        Self {
            accepted_connections: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            clean_truncated_disconnects: AtomicU64::new(0),
            rejected_oversized_frames: AtomicU64::new(0),
            rejected_protocol_frames: AtomicU64::new(0),
            started_at: Instant::now(),
            records: Mutex::new(Vec::new()),
        }
    }

    pub fn elapsed_ms(&self, instant: Instant) -> u64 {
        instant
            .saturating_duration_since(self.started_at)
            .as_millis() as u64
    }

    pub fn connection_records(&self) -> Vec<TcpConnectionRecord> {
        let mut out = self.records.lock().map(|g| g.clone()).unwrap_or_default();
        out.sort_by_key(|r| r.conn_id);
        out
    }

    fn push_record(&self, rec: TcpConnectionRecord) {
        if let Ok(mut guard) = self.records.lock() {
            if guard.len() >= MAX_TCP_LIFECYCLE_RECORDS {
                let drop_n = MAX_TCP_LIFECYCLE_RECORDS / 4;
                guard.drain(..drop_n);
            }
            guard.push(rec);
        }
    }
}

/// Handle to a running TCP AMF3 game server listener.
pub struct TcpGameServerHandle {
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    telemetry: Arc<TcpServerTelemetry>,
    accept_thread: Option<JoinHandle<()>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl TcpGameServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn telemetry(&self) -> &Arc<TcpServerTelemetry> {
        &self.telemetry
    }

    pub fn stop(&mut self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            let _ = TcpStream::connect_timeout(&self.local_addr, Duration::from_millis(200));
        }
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
        if let Ok(mut guard) = self.workers.lock() {
            for worker in guard.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for TcpGameServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawns the multi-threaded TCP game server accepting `LCPacket` (24B) & raw (4B) AMF3 streams.
pub fn spawn_tcp_game_server<A: ToSocketAddrs>(
    bind_addr: A,
    world: Arc<ServerWorld>,
) -> io::Result<TcpGameServerHandle> {
    let listener = TcpListener::bind(bind_addr)?;
    let local_addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = Arc::clone(&shutdown);
    let telemetry = Arc::new(TcpServerTelemetry::new());
    let telemetry_accept = Arc::clone(&telemetry);
    let workers: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let workers_accept = Arc::clone(&workers);

    let accept_thread = thread::Builder::new()
        .name("gserver-tcp-accept".into())
        .spawn(move || {
            let mut conn_seq = 0u64;
            while !shutdown_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if shutdown_flag.load(Ordering::SeqCst) {
                            let _ = stream.shutdown(Shutdown::Both);
                            break;
                        }
                        conn_seq += 1;
                        telemetry_accept
                            .accepted_connections
                            .fetch_add(1, Ordering::AcqRel);
                        telemetry_accept
                            .active_connections
                            .fetch_add(1, Ordering::AcqRel);

                        // Explicitly clear Winsock2 `FIONBIO` inherited from `listener.set_nonblocking(true)`.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_read_timeout(Some(READ_POLL_SLICE));
                        let _ = stream.set_write_timeout(Some(WRITE_SOCKET_TIMEOUT));

                        let world_clone = Arc::clone(&world);
                        let conn_shutdown = Arc::clone(&shutdown_flag);
                        let tel_clone = Arc::clone(&telemetry_accept);
                        if let Ok(worker) = thread::Builder::new()
                            .name(format!("gserver-tcp-conn-{conn_seq}"))
                            .spawn(move || {
                                handle_client_connection(
                                    stream,
                                    Some(peer),
                                    conn_seq,
                                    world_clone,
                                    conn_shutdown,
                                    &tel_clone,
                                );
                                tel_clone.active_connections.fetch_sub(1, Ordering::AcqRel);
                            })
                        {
                            if let Ok(mut list) = workers_accept.lock() {
                                list.retain(|h| !h.is_finished());
                                list.push(worker);
                            }
                        } else {
                            telemetry_accept
                                .active_connections
                                .fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => {
                        if shutdown_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                }
            }
        })?;

    Ok(TcpGameServerHandle {
        local_addr,
        shutdown,
        telemetry,
        accept_thread: Some(accept_thread),
        workers,
    })
}

fn handle_client_connection(
    mut reader: TcpStream,
    peer_addr: Option<SocketAddr>,
    conn_id: u64,
    world: Arc<ServerWorld>,
    shutdown: Arc<AtomicBool>,
    telemetry: &TcpServerTelemetry,
) {
    let connected_at = Instant::now();
    let mut last_rx_at = connected_at;
    let mut last_event_at = connected_at;

    let Ok(writer_stream) = reader.try_clone() else {
        return;
    };
    let writer = Arc::new(Mutex::new(writer_stream));
    let mut rx = ClientRxDecoder::new(C2sFrameMode::Auto);
    let mut session = ConnectionSession::default();
    let mut buf = [0u8; 8192];

    let mut rx_bytes_total = 0u64;
    let mut frames_decoded = 0u64;
    let mut responses_sent = 0u64;
    let mut truncated_disconnect = false;
    let mut oversized_disconnect = false;
    let mut protocol_rejected = false;

    let shutdown_reason = 'conn_loop: loop {
        if shutdown.load(Ordering::SeqCst) {
            break TcpShutdownReason::ServerShutdown;
        }
        match reader.read(&mut buf) {
            Ok(0) => {
                let buffered_bytes = rx.buffered_len();
                if buffered_bytes > 0 {
                    truncated_disconnect = true;
                    break TcpShutdownReason::TruncatedFrameEof { buffered_bytes };
                }
                break TcpShutdownReason::CleanClientEof;
            }
            Ok(n) => {
                let now = Instant::now();
                last_rx_at = now;
                last_event_at = now;
                rx_bytes_total += n as u64;
                rx.feed(&buf[..n]);

                loop {
                    match rx.next_frame() {
                        Ok(Some(frame)) => {
                            frames_decoded += 1;
                            let req = match RpcRequest::from_amf3_bytes(&frame.amf3_payload) {
                                Ok(r) => r,
                                Err(decode_err) => {
                                    telemetry
                                        .rejected_protocol_frames
                                        .fetch_add(1, Ordering::Release);
                                    eprintln!(
                                        "[gserver-tcp] conn={conn_id} peer={peer_addr:?}: skipping malformed AMF3 RPC payload ({}B): {decode_err}",
                                        frame.amf3_payload.len()
                                    );
                                    if let Ok((raw_val, _)) =
                                        gserver_protocol::amf3::decode(&frame.amf3_payload)
                                    {
                                        if let Some(cb) = raw_val
                                            .get_field("callback")
                                            .and_then(|c| c.get_field("callbackName"))
                                            .and_then(gserver_protocol::Value::as_str)
                                        {
                                            let err_env = gserver_protocol::S2cEnvelope::error_response(
                                                cb,
                                                "invalid_rpc_payload",
                                            );
                                            let _ = world.write_envelope(&writer, &err_env);
                                            responses_sent += 1;
                                        }
                                    }
                                    continue;
                                }
                            };
                            let was_logged_in = session.logged_in_uid;
                            let handled = world.handle_rpc(&mut session, &req);

                            if was_logged_in.is_none() {
                                if let Some(uid) = session.logged_in_uid {
                                    world.bind_peer_stream(uid, Arc::clone(&writer));
                                }
                            }

                            for env in &handled.caller_envelopes {
                                if world.write_envelope(&writer, env).is_err() {
                                    world.broadcast_peer_pushes(handled.peer_envelopes);
                                    break 'conn_loop TcpShutdownReason::WriteError(
                                        io::ErrorKind::BrokenPipe,
                                    );
                                }
                                responses_sent += 1;
                                last_event_at = Instant::now();
                            }
                            world.broadcast_peer_pushes(handled.peer_envelopes);
                        }
                        Ok(None) => break,
                        Err(ProtocolError::Frame(FrameError::BodyTooLarge {
                            body_len,
                            max_body_len,
                        })) => {
                            oversized_disconnect = true;
                            break 'conn_loop TcpShutdownReason::OversizedFrame {
                                declared: body_len,
                                max: max_body_len,
                            };
                        }
                        Err(err) => {
                            protocol_rejected = true;
                            break 'conn_loop TcpShutdownReason::ProtocolRejected(err.to_string());
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                if shutdown.load(Ordering::SeqCst) {
                    break TcpShutdownReason::ServerShutdown;
                }
                let buffered_bytes = rx.buffered_len();
                if buffered_bytes > 0 {
                    let idle = last_rx_at.elapsed();
                    if idle >= PARTIAL_FRAME_IDLE_TIMEOUT {
                        truncated_disconnect = true;
                        break TcpShutdownReason::IdleTimeoutTruncatedFrame {
                            buffered_bytes,
                            idle_ms: idle.as_millis() as u64,
                        };
                    }
                }
                continue;
            }
            Err(e) => {
                let buffered_bytes = rx.buffered_len();
                if buffered_bytes > 0 {
                    truncated_disconnect = true;
                }
                break TcpShutdownReason::ReadError {
                    kind: e.kind(),
                    buffered_bytes,
                };
            }
        }
    };

    world.on_disconnect(&session);
    let _ = reader.shutdown(Shutdown::Write);

    let closed_at = Instant::now();
    telemetry.push_record(TcpConnectionRecord {
        conn_id,
        peer_addr,
        connected_at_ms: telemetry.elapsed_ms(connected_at),
        last_event_at_ms: telemetry.elapsed_ms(last_event_at),
        closed_at_ms: telemetry.elapsed_ms(closed_at),
        rx_bytes_total,
        frames_decoded,
        responses_sent,
        final_parser_state: rx.parser_state(),
        shutdown_reason,
    });

    if truncated_disconnect {
        telemetry
            .clean_truncated_disconnects
            .fetch_add(1, Ordering::Release);
    }
    if oversized_disconnect {
        telemetry
            .rejected_oversized_frames
            .fetch_add(1, Ordering::Release);
    }
    if protocol_rejected {
        telemetry
            .rejected_protocol_frames
            .fetch_add(1, Ordering::Release);
    }
}
