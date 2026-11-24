#![forbid(unsafe_code)]

//! Multi-threaded, DoS-hardened GServer (`vnsg2`) TCP Game Server (`GameTcpServer`)
//! engineered to stress-test every adversarial & concurrency failure mode of `GCLIENT.exe`:
//! - Enforces 24-byte `LCPacket` ingress (`MAX_SERVER_FRAME_BODY_LEN = 16 MiB` + `16 MiB` zip-bomb cap).
//! - Emits compliant 8-byte `CConnection` egress (`head[5] = zipped`, `head[6] = multi`, `body_len > 0`).
//! - Automatically promotes all timestamps (`1_795_500_450`) and large counters (`>= 2^28`) to `0x05 Double`.
//! - Never emits `Null`/`Undefined` inside dense `0x09` arrays (verified via `audit_lua_client_hazards`).
//! - Tracks atomic QA telemetry across all concurrent client & hostile fuzz threads.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::amf3::{audit_lua_client_hazards, Amf3Error, Amf3Value};
use crate::framing::{
    build_ping_response, build_rpc_response, encode_server_multi_frame, encode_server_single_frame,
    FrameError, ServerRxStreamDecoder, MAX_SERVER_FRAME_BODY_LEN,
};

#[derive(Debug, Clone)]
pub struct GameServerConfig {
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

#[derive(Default)]
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
}

pub struct GameTcpServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    telemetry: Arc<ServerTelemetry>,
    accept_thread: Option<JoinHandle<()>>,
}

impl GameTcpServer {
    pub fn start(bind_addr: &str, config: GameServerConfig) -> std::io::Result<Self> {
        let listener = TcpListener::bind(bind_addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let telemetry = Arc::new(ServerTelemetry::default());

        let shutdown_cl = Arc::clone(&shutdown);
        let telemetry_cl = Arc::clone(&telemetry);
        let handle = thread::spawn(move || {
            let mut conn_seq = 0u64;
            while !shutdown_cl.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        conn_seq += 1;
                        telemetry_cl
                            .accepted_connections
                            .fetch_add(1, Ordering::Relaxed);
                        let cur = telemetry_cl.active_connections.fetch_add(1, Ordering::AcqRel) + 1;
                        update_max(&telemetry_cl.peak_connections, cur);

                        let cfg = config.clone();
                        let tel = Arc::clone(&telemetry_cl);
                        let sd = Arc::clone(&shutdown_cl);
                        thread::spawn(move || {
                            handle_game_client(stream, conn_seq, &cfg, &tel, &sd);
                            tel.active_connections.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
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
        });

        Ok(Self {
            addr,
            shutdown,
            telemetry,
            accept_thread: Some(handle),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn telemetry(&self) -> &Arc<ServerTelemetry> {
        &self.telemetry
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.accept_thread.take() {
            let _ = h.join();
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

fn handle_game_client(
    mut stream: TcpStream,
    conn_seq: u64,
    cfg: &GameServerConfig,
    telemetry: &ServerTelemetry,
    shutdown: &AtomicBool,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(cfg.read_timeout));
    let _ = stream.set_write_timeout(Some(cfg.write_timeout));

    let mut decoder = ServerRxStreamDecoder::new(cfg.max_frame_body_len, cfg.max_decompressed_len);
    let mut session_token = format!("tok_vnsg2_{:06}", conn_seq);
    let mut read_buf = [0u8; 8192];

    while !shutdown.load(Ordering::Acquire) {
        match stream.read(&mut read_buf) {
            Ok(0) => {
                if decoder.buffered_len() > 0 {
                    telemetry
                        .clean_truncated_disconnects
                        .fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
            Ok(n) => match decoder.push_bytes(&read_buf[..n]) {
                Ok(packets) => {
                    for pkt in packets {
                        match dispatch_client_rpc(
                            &pkt.value,
                            &mut session_token,
                            conn_seq,
                            cfg,
                            telemetry,
                        ) {
                            Ok(Some(wire_out)) => {
                                if stream.write_all(&wire_out).is_err() {
                                    return;
                                }
                            }
                            Ok(None) => {}
                            Err(_) => return,
                        }
                    }
                    let _ = stream.flush();
                }
                Err(err) => {
                    record_rejection(&err, telemetry);
                    return;
                }
            },
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if decoder.buffered_len() > 0 {
                    // Peer stalled mid-frame -> drop idle slowloris/truncated stream cleanly!
                    telemetry
                        .clean_truncated_disconnects
                        .fetch_add(1, Ordering::Relaxed);
                    break;
                }
                continue;
            }
            Err(_) => break,
        }
    }
}

fn record_rejection(err: &FrameError, telemetry: &ServerTelemetry) {
    match err {
        FrameError::OversizedBody { .. } => {
            telemetry
                .rejected_oversized_frames
                .fetch_add(1, Ordering::Relaxed);
        }
        FrameError::ZeroLengthBody => {
            telemetry
                .rejected_zero_frames
                .fetch_add(1, Ordering::Relaxed);
        }
        FrameError::Md5Mismatch { .. } | FrameError::InvalidClientFlags(_) => {
            telemetry.rejected_md5_frames.fetch_add(1, Ordering::Relaxed);
        }
        FrameError::Zlib(_) => {
            telemetry
                .rejected_zlib_frames
                .fetch_add(1, Ordering::Relaxed);
        }
        FrameError::Amf3(Amf3Error::MaxDepthExceeded { .. }) => {
            telemetry
                .rejected_amf3_depth_bombs
                .fetch_add(1, Ordering::Relaxed);
        }
        FrameError::Amf3(_) => {
            telemetry
                .rejected_amf3_malformed
                .fetch_add(1, Ordering::Relaxed);
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
                ("qid", Amf3Value::from_i64_precision_safe(1_243_831_974_477_471_744)),
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
