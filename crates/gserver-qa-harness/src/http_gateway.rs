#![forbid(unsafe_code)]

//! Multi-threaded HTTP/1.1 SDK Gateway (`HttpSdkGateway`) compatible with `BtPlatformWin32.cpp`
//! and `AccountAuthProvider.cpp` so `GCLIENT.exe` can complete its full HTTP bootstrap against `127.0.0.1`
//! without recompiling the C++ client.
//!
//! Endpoints implemented (matching `live_capture_filtered.log` verbatim):
//! - `/phone/applereview` & `/phone/applereview/`
//! - `/phone/serverlist`  & `/phone/serverlist/`
//! - `/phone/supernotice` & `/phone/supernotice/`
//! - `/phone/login`       & `/phone/login/`  (issues `Set-Cookie: PHPSESSID=...`)
//! - `/phone/loginrole`   & `/phone/loginrole/`
//! - `/phone/gethash`     & `/phone/gethash/` (handles `?&pid=...`, `&&`, `pid=`, cookie session fallback,
//!   and returns `"host": "<game_host>", "port": <game_port>`)

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::crypto::{
    md5_hex, parse_tolerant_query_params, verify_platform_url_signature, LOCAL_DEV_SIGN_KEY,
};

const MAX_HTTP_HEADER_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct HttpGatewayConfig {
    pub game_tcp_host: String,
    pub game_tcp_port: u16,
    pub sign_key: String,
    pub enforce_signature: bool,
    pub fixed_timestamp: i64,
}

impl Default for HttpGatewayConfig {
    fn default() -> Self {
        Self {
            game_tcp_host: "127.0.0.1".to_string(),
            game_tcp_port: 18160,
            sign_key: LOCAL_DEV_SIGN_KEY.to_string(),
            enforce_signature: false,
            fixed_timestamp: 1_795_500_450,
        }
    }
}

#[derive(Default)]
pub struct HttpGatewayMetrics {
    pub total_requests: AtomicU64,
    pub gethash_requests: AtomicU64,
    pub login_requests: AtomicU64,
    pub signature_failures: AtomicU64,
    pub cookie_hits: AtomicU64,
}

pub struct HttpSdkGateway {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    metrics: Arc<HttpGatewayMetrics>,
    accept_thread: Option<JoinHandle<()>>,
}

impl HttpSdkGateway {
    pub fn start(bind_addr: &str, config: HttpGatewayConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(bind_addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let metrics = Arc::new(HttpGatewayMetrics::default());
        let sessions: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

        let shutdown_cl = Arc::clone(&shutdown);
        let metrics_cl = Arc::clone(&metrics);
        let handle = thread::spawn(move || {
            while !shutdown_cl.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        let cfg = config.clone();
                        let m = Arc::clone(&metrics_cl);
                        let sess = Arc::clone(&sessions);
                        thread::spawn(move || {
                            let _ = handle_http_client(stream, &cfg, &m, &sess);
                        });
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
        });

        Ok(Self {
            addr,
            shutdown,
            metrics,
            accept_thread: Some(handle),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn metrics(&self) -> &Arc<HttpGatewayMetrics> {
        &self.metrics
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.accept_thread.take() {
            let _ = h.join();
        }
    }
}

impl Drop for HttpSdkGateway {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Pure function routing an HTTP request target + `Cookie` header to `(status, set_cookie, json_body)`.
/// Exposed directly for fast deterministic PRNG fuzzing as well as live socket serving.
pub fn route_sdk_http_request(
    target: &str,
    cookie_header: Option<&str>,
    cfg: &HttpGatewayConfig,
    metrics: &HttpGatewayMetrics,
    cookie_sessions: &Mutex<HashMap<String, String>>,
) -> (u16, Option<String>, String) {
    metrics.total_requests.fetch_add(1, Ordering::Relaxed);

    if cfg.enforce_signature && target.contains('?') {
        if let Err(err) = verify_platform_url_signature(target, &cfg.sign_key) {
            metrics.signature_failures.fetch_add(1, Ordering::Relaxed);
            let body = format!(
                "{{\"errornu\":\"1\",\"errordesc\":\"{}\"}}",
                json_escape(&err.to_string())
            );
            return (403, None, body);
        }
    }

    let path = target.split('?').next().unwrap_or(target);
    let norm_path = path.trim_end_matches('/');
    let params = parse_tolerant_query_params(target);

    let incoming_phpsessid = cookie_header.and_then(extract_phpsessid);
    if incoming_phpsessid.is_some() {
        metrics.cookie_hits.fetch_add(1, Ordering::Relaxed);
    }

    match norm_path {
        "/phone/applereview" | "/applereview" => (
            200,
            None,
            "{\"ret\":404,\"msg\":\"no data\",\"applereview\":0,\"ip\":\"127.0.0.1\",\"errornu\":\"0\",\"errordesc\":\"OK\"}".to_string(),
        ),
        "/phone/serverlist" | "/serverlist" => {
            let body = format!(
                "{{\"errornu\":\"0\",\"errordesc\":\"OK\",\"root\":{{\"item\":[{{\"groupid\":\"2900161\",\"server_id\":\"game2900161\",\"name\":\"S1.LocalRustQA\",\"host\":\"{}\",\"port\":{},\"status\":1,\"hot\":1,\"new\":1,\"openDateTime\":\"2018-11-06-09-00-00\"}}]}}}}",
                json_escape(&cfg.game_tcp_host),
                cfg.game_tcp_port
            );
            (200, None, body)
        }
        "/phone/supernotice" | "/supernotice" | "/phone/notice" | "/notice" => (
            200,
            None,
            "{\"errornu\":\"0\",\"errordesc\":\"OK\",\"data\":\"GServer Local Rust Server Ready\"}".to_string(),
        ),
        "/phone/getversion" | "/getversion" | "/phone/getforcefix" | "/getforcefix" => (
            200,
            None,
            "{\"errornu\":\"0\",\"errordesc\":\"OK\",\"needUpdate\":0}".to_string(),
        ),
        "/phone/login" | "/login" => {
            metrics.login_requests.fetch_add(1, Ordering::Relaxed);
            let userid = params
                .get("userid")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "win32_local_0001".to_string());
            let pid = md5_hex(format!("gserver_pid_{}", userid).as_bytes())[..16].to_string();
            let sess_id = md5_hex(format!("phpsess_{}_{}", userid, cfg.fixed_timestamp).as_bytes());
            if let Ok(mut map) = cookie_sessions.lock() {
                map.insert(sess_id.clone(), pid.clone());
            }
            let set_cookie = format!("PHPSESSID={}; Path=/; HttpOnly", sess_id);
            let body = format!(
                "{{\"errornu\":\"0\",\"uid\":\"{}\",\"pid\":\"{}\",\"errordesc\":\"OK(1-0)\",\"newuser\":0,\"lastLoginTime\":{},\"regPlosgn\":2,\"env\":\"local\"}}",
                json_escape(&pid),
                json_escape(&pid),
                cfg.fixed_timestamp
            );
            (200, Some(set_cookie), body)
        }
        "/phone/loginrole" | "/loginrole" => {
            let pid = resolve_pid(&params, incoming_phpsessid.as_deref(), cookie_sessions);
            let body = format!(
                "{{\"root\":{{\"role\":[{{\"uid\":52592,\"pid\":\"{}\",\"uname\":\"LocalGeneral\",\"level\":120,\"quality\":6,\"groupid\":\"2900161\",\"groupname\":\"S1.LocalRustQA\",\"maintainflag\":0,\"vip\":10}}],\"recallOk\":0}},\"errornu\":\"0\",\"errordesc\":\"OK(1-0)\"}}",
                json_escape(&pid)
            );
            (200, None, body)
        }
        "/phone/gethash" | "/gethash" => {
            metrics.gethash_requests.fetch_add(1, Ordering::Relaxed);
            let pid = resolve_pid(&params, incoming_phpsessid.as_deref(), cookie_sessions);
            let uuid = params
                .get("uuid")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "00000000000000000000000000000000".to_string());
            let group_id = params
                .get("group_id")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "2900161".to_string());
            let hash = md5_hex(format!("{}:{}:{}", pid, group_id, cfg.fixed_timestamp).as_bytes());

            // Exact field schema from `live_capture_filtered.log:76`!
            let body = format!(
                "{{\"AMF\":3,\"b_fightLog\":1,\"canCreateUser\":1,\"canPay\":0,\"errordesc\":\"OK\",\"errornu\":\"0\",\"feignRoleId\":\"487932b\",\"host\":\"{}\",\"mergeServerTime\":1667530800,\"offset\":\"-420\",\"openDateTime\":\"2018-11-06-09-00-00\",\"pid\":\"{}\",\"plname\":\"android\",\"plosgn\":2,\"port\":{},\"ptype\":0,\"qid\":\"1243831974477471744\",\"serverID\":\"game{}\",\"timestamp\":{},\"uuid\":\"{}\",\"hash\":\"{}\"}}",
                json_escape(&cfg.game_tcp_host),
                json_escape(&pid),
                cfg.game_tcp_port,
                json_escape(&group_id),
                cfg.fixed_timestamp,
                json_escape(&uuid),
                hash
            );
            (200, None, body)
        }
        _ => (
            200,
            None,
            "{\"errornu\":\"0\",\"errordesc\":\"OK\",\"data\":{}}".to_string(),
        ),
    }
}

fn resolve_pid(
    params: &std::collections::BTreeMap<String, String>,
    phpsessid: Option<&str>,
    cookie_sessions: &Mutex<HashMap<String, String>>,
) -> String {
    if let Some(p) = params.get("pid") {
        if !p.is_empty() {
            return p.clone();
        }
    }
    if let Some(sid) = phpsessid {
        if let Ok(map) = cookie_sessions.lock() {
            if let Some(saved_pid) = map.get(sid) {
                return saved_pid.clone();
            }
        }
    }
    "636df6782106606d".to_string()
}

fn extract_phpsessid(cookie_hdr: &str) -> Option<String> {
    for part in cookie_hdr.split(';') {
        let trimmed = part.trim();
        if let Some(val) = trimmed.strip_prefix("PHPSESSID=") {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn handle_http_client(
    mut stream: TcpStream,
    cfg: &HttpGatewayConfig,
    metrics: &HttpGatewayMetrics,
    cookie_sessions: &Mutex<HashMap<String, String>>,
) -> io::Result<()> {
    // Clear Winsock2 `FIONBIO` non-blocking mode inherited from `listener.set_nonblocking(true)`
    // before installing blocking read/write timeouts.
    stream.set_nonblocking(false)?;
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let mut req_buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                req_buf.extend_from_slice(&chunk[..n]);
                if req_buf.windows(4).any(|w| w == b"\r\n\r\n")
                    || req_buf.len() >= MAX_HTTP_HEADER_BYTES
                {
                    break;
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }

    let req_str = String::from_utf8_lossy(&req_buf);
    let mut lines = req_str.lines();
    let req_line = match lines.next() {
        Some(l) => l,
        None => return Ok(()),
    };
    let mut parts = req_line.split_whitespace();
    let _method = parts.next().unwrap_or("GET");
    let target = parts.next().unwrap_or("/");

    let mut cookie_hdr: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, val)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("cookie") {
                cookie_hdr = Some(val.trim().to_string());
            }
        }
    }

    let (status, set_cookie, body) =
        route_sdk_http_request(target, cookie_hdr.as_deref(), cfg, metrics, cookie_sessions);
    let status_text = if status == 200 { "OK" } else { "Forbidden" };

    let mut resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n",
        status,
        status_text,
        body.len()
    );
    if let Some(sc) = set_cookie {
        resp.push_str(&format!("Set-Cookie: {}\r\n", sc));
    }
    resp.push_str("\r\n");
    resp.push_str(&body);

    stream.write_all(resp.as_bytes())?;
    stream.flush()?;
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}
