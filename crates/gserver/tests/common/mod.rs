#![forbid(unsafe_code)]
#![allow(dead_code)]
//! Shared E2E integration test harness for `gserver` HTTP MAPI + 24B/8B `LCPacket` TCP suites.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gserver::bridge::protocol_to_catalog_amf3;
use gserver_http::{parse_json, sign_mapi_url, JsonValue, VNSG2_MAPI_APP_KEY};
use gserver_protocol::{
    amf3, decode_s2c_frame_batch, zlib_decompress, RpcRequest, S2cEnvelope, ServerRxDecoder, Value,
};
use gserver_rpc_catalog::{
    validate_response_for_method, Amf3Value as CatalogAmf3, RpcResponseEnvelope, BOOTSTRAP_SEQUENCE,
};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

pub struct TempDirGuard {
    pub path: PathBuf,
}

impl TempDirGuard {
    pub fn new(label: &str) -> Self {
        let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("gserver_e2e_{label}_{}_{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp test dir");
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Minimal raw HTTP/1.1 GET helper returning `(status_code, set_cookies, raw_body)`.
pub fn raw_http_get_text(
    addr: SocketAddr,
    target_path_and_query: &str,
    cookies: &[String],
) -> (u16, Vec<String>, String) {
    let mut stream = TcpStream::connect(addr).expect("connect HTTP MAPI");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut req = format!(
        "GET {target_path_and_query} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n"
    );
    if !cookies.is_empty() {
        req.push_str(&format!("Cookie: {}\r\n", cookies.join("; ")));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).expect("write HTTP GET");

    let mut resp_bytes = Vec::new();
    stream.read_to_end(&mut resp_bytes).expect("read HTTP resp");
    let resp_str = String::from_utf8_lossy(&resp_bytes);
    let (head, body) = resp_str
        .split_once("\r\n\r\n")
        .expect("valid HTTP CRLFCRLF");
    let mut lines = head.lines();
    let status_line = lines.next().expect("HTTP status line");
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();

    let mut set_cookies = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("Set-Cookie") {
                if let Some(pair) = v.trim().split(';').next() {
                    set_cookies.push(pair.trim().to_string());
                }
            }
        }
    }

    (status_code, set_cookies, body.to_string())
}

/// Minimal raw HTTP/1.1 GET helper parsing JSON response body.
pub fn raw_http_get(
    addr: SocketAddr,
    target_path_and_query: &str,
    cookies: &[String],
) -> (u16, Vec<String>, JsonValue) {
    let (status_code, set_cookies, body) = raw_http_get_text(addr, target_path_and_query, cookies);
    let json = parse_json(body.trim()).unwrap_or_else(|e| {
        panic!("expected JSON body from {target_path_and_query}, got error {e}: {body}")
    });
    (status_code, set_cookies, json)
}

/// Signs a `/phone/*` MAPI request path + query using `VNSG2_MAPI_APP_KEY` and returns the relative target.
pub fn signed_mapi_target(addr: SocketAddr, path_and_query: &str, ts: i64) -> String {
    let full = sign_mapi_url(
        &format!("http://{addr}{path_and_query}"),
        ts,
        VNSG2_MAPI_APP_KEY,
    );
    full.strip_prefix(&format!("http://{addr}"))
        .unwrap()
        .to_string()
}

/// Simulated GServer (`vnsg2`) C++ / Lua TCP client speaking 24-byte `LCPacket` C2S and 8-byte S2C.
pub struct E2eWireClient {
    stream: TcpStream,
    rx: ServerRxDecoder,
    seq: u64,
    token: String,
    queued_pushes: Vec<S2cEnvelope>,
    read_timeout: Duration,
}

impl E2eWireClient {
    pub fn connect(addr: SocketAddr) -> Self {
        let read_timeout = Duration::from_secs(5);
        let stream = TcpStream::connect(addr).expect("connect TCP game server");
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(read_timeout)).unwrap();
        Self {
            stream,
            rx: ServerRxDecoder::new(),
            seq: 1,
            token: "0".to_string(),
            queued_pushes: Vec::new(),
            read_timeout,
        }
    }

    pub fn call_rpc(&mut self, method: &str, args: Vec<Value>, compress_c2s: bool) -> S2cEnvelope {
        self.seq += 1;
        let telemetry = method == "user.stat" || method == "user.guideEvent";
        let cb_name = if telemetry {
            method.to_string()
        } else {
            format!("{method}_{}", self.seq)
        };
        let req = RpcRequest::new(method, args, &self.token, cb_name.clone());
        let wire = req
            .to_lcpacket_frame(compress_c2s)
            .expect("encode 24B LCPacket");
        self.stream.write_all(&wire).expect("send C2S frame");
        self.stream.flush().unwrap();

        let mut buf = [0u8; 8192];
        loop {
            while let Some(frame) = self.rx.next_frame().expect("decode S2C 8B frame") {
                let batch = decode_s2c_frame_batch(&frame).expect("decode S2C envelope batch");
                for env in batch.items {
                    if let Some(ref tok) = env.token {
                        if !tok.is_empty() && tok != "0" {
                            self.token = tok.clone();
                        }
                    }
                    if env.callback_name.as_deref() == Some(cb_name.as_str()) {
                        return env;
                    }
                    self.queued_pushes.push(env);
                }
            }
            let n = self
                .stream
                .read(&mut buf)
                .expect("read S2C response bytes before timeout");
            assert!(n > 0, "unexpected EOF waiting for {cb_name}");
            self.rx.feed(&buf[..n]);
        }
    }

    pub fn call_ping(&mut self, method: &str) -> S2cEnvelope {
        let req = RpcRequest::new(method, Vec::new(), &self.token, method);
        let wire = req.to_lcpacket_frame(false).unwrap();
        self.stream.write_all(&wire).unwrap();
        self.stream.flush().unwrap();
        let mut buf = [0u8; 4096];
        loop {
            while let Some(frame) = self.rx.next_frame().unwrap() {
                let batch = decode_s2c_frame_batch(&frame).unwrap();
                for env in batch.items {
                    if env.err == "ping" {
                        return env;
                    }
                    self.queued_pushes.push(env);
                }
            }
            let n = self.stream.read(&mut buf).unwrap();
            assert!(n > 0);
            self.rx.feed(&buf[..n]);
        }
    }

    pub fn drain_pushes(&mut self) -> Vec<S2cEnvelope> {
        let _ = self
            .stream
            .set_read_timeout(Some(Duration::from_millis(80)));
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.rx.feed(&buf[..n]);
                    while let Some(frame) = self.rx.next_frame().expect("decode S2C push frame") {
                        let batch = decode_s2c_frame_batch(&frame).expect("decode push batch");
                        self.queued_pushes.extend(batch.items);
                    }
                }
                Err(_) => break,
            }
        }
        let _ = self.stream.set_read_timeout(Some(self.read_timeout));
        std::mem::take(&mut self.queued_pushes)
    }

    pub fn wait_for_push(&mut self, expected_cb: &str) -> Vec<S2cEnvelope> {
        if self
            .queued_pushes
            .iter()
            .any(|p| p.callback_name.as_deref() == Some(expected_cb))
        {
            return std::mem::take(&mut self.queued_pushes);
        }
        let _ = self
            .stream
            .set_read_timeout(Some(Duration::from_millis(50)));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf = [0u8; 8192];
        while Instant::now() < deadline {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.rx.feed(&buf[..n]);
                    while let Some(frame) = self.rx.next_frame().expect("decode S2C push frame") {
                        let batch = decode_s2c_frame_batch(&frame).expect("decode push batch");
                        self.queued_pushes.extend(batch.items);
                    }
                    if self
                        .queued_pushes
                        .iter()
                        .any(|p| p.callback_name.as_deref() == Some(expected_cb))
                    {
                        break;
                    }
                }
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        let _ = self.stream.set_read_timeout(Some(self.read_timeout));
        std::mem::take(&mut self.queued_pushes)
    }
}

/// Completes signed `/phone/login/` + `/phone/gethash/` exchange after platform SDK auth.
pub fn complete_mapi_login_and_gethash(
    http_addr: SocketAddr,
    user_id: &str,
    session_id: &str,
    uuid_suffix: &str,
    ts: i64,
    verify_cookie_102: bool,
) -> (String, String, String, u16) {
    let login_target = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/login/?pl=android_vnsg2&gn=vnsg2&os=win32&userid={user_id}&sessionid={session_id}"
        ),
        ts,
    );
    let (status, cookies, login_json) = raw_http_get(http_addr, &login_target, &[]);
    assert_eq!(status, 200);
    assert!(
        !cookies.is_empty(),
        "/phone/login must issue session cookies"
    );
    assert_eq!(
        login_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    let pid = login_json
        .get("pid")
        .and_then(JsonValue::as_scalar_string)
        .expect("pid in /phone/login");

    let gethash_target = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/gethash/?pl=android_vnsg2&gn=vnsg2&pid={pid}&group_id=2900161&uuid=uuid_{uuid_suffix}"
        ),
        ts + 1,
    );

    if verify_cookie_102 {
        let (_, _, err_json) = raw_http_get(http_addr, &gethash_target, &[]);
        assert_eq!(
            err_json.get("errornu").and_then(JsonValue::as_str),
            Some("102"),
            "missing cookie on /phone/gethash must return errornu='102'"
        );
    }

    let (_, _, hash_json) = raw_http_get(http_addr, &gethash_target, &cookies);
    assert_eq!(
        hash_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    let hash = hash_json
        .get("hash")
        .and_then(JsonValue::as_str)
        .unwrap()
        .to_string();
    let host = hash_json
        .get("host")
        .and_then(JsonValue::as_str)
        .unwrap()
        .to_string();
    let port = hash_json.get("port").and_then(JsonValue::as_u64).unwrap() as u16;

    (pid, hash, host, port)
}

/// Validates a live S2C RPC response envelope against `gserver-rpc-catalog` Lua schema invariants.
pub fn assert_catalog_valid(method: &str, args: &[Value], env: &S2cEnvelope) {
    let cat_args: Vec<CatalogAmf3> = args.iter().map(protocol_to_catalog_amf3).collect();
    let cat_resp = RpcResponseEnvelope {
        err: env.err.clone(),
        callback_name: env.callback_name.clone().unwrap_or_default(),
        ret: protocol_to_catalog_amf3(&env.ret),
        time: env.time.unwrap_or(1_795_500_500),
        token: env.token.clone().unwrap_or_default(),
    };
    if let Err(violations) = validate_response_for_method(method, &cat_args, &cat_resp, true) {
        panic!("Lua schema violation(s) on {method}: {violations:#?}");
    }
}

/// Runs steps 7..=28 of `BOOTSTRAP_SEQUENCE` and validates every response against `gserver-rpc-catalog`.
pub fn run_bootstrap_from_step7(client: &mut E2eWireClient) {
    for spec in BOOTSTRAP_SEQUENCE.iter().filter(|s| s.step_order >= 7) {
        let args: Vec<Value> = match spec.method {
            "bag.bagInfo" => vec![Value::empty_array()],
            "copy.getList" => vec![Value::Int(1)],
            "copy.getCopy" => vec![Value::Int(1001)],
            "reward.getRewardList" => vec![Value::Int(0), Value::Int(0)],
            "user.guideEvent" => vec![Value::Int(240)],
            "barrage.enterRoom" => vec![Value::Int(1)],
            "barrage.getBarrageRecord" => vec![Value::Int(1)],
            _ => vec![],
        };
        let resp = client.call_rpc(spec.method, args.clone(), spec.step_order % 2 == 0);
        assert_catalog_valid(spec.method, &args, &resp);
    }
}

/// Decompresses an RFC 1950 Zlib `ByteString` `Value` and decodes its inner AMF3 payload.
pub fn decode_zlib_amf3(val: &Value) -> Value {
    let compressed = val.as_bytes().expect("Zlib-compressed AMF3 ByteString");
    let decompressed = zlib_decompress(compressed).expect("RFC 1950 Zlib decompress");
    let (decoded, _) = amf3::decode(&decompressed).expect("decode inner AMF3 payload");
    decoded
}

/// Connects a fresh TCP client and logs into an existing persisted character (`user.login` + `user.userLogin`).
pub fn tcp_relogin_existing_role(
    tcp_addr: SocketAddr,
    pid: &str,
    qid: &str,
    hash: &str,
    uid: u64,
) -> E2eWireClient {
    let mut client = E2eWireClient::connect(tcp_addr);
    let login_env = client.call_rpc(
        "user.login",
        vec![Value::map([
            ("pid", Value::String(pid.to_string())),
            ("qid", Value::String(qid.to_string())),
            ("hash", Value::String(hash.to_string())),
            ("serverID", Value::String("game2900161".into())),
        ])],
        false,
    );
    assert_eq!(login_env.err, "ok");
    let ul_env = client.call_rpc("user.userLogin", vec![Value::Int(uid as i32)], false);
    assert_eq!(ul_env.ret.as_str(), Some("ok"));
    client
}
