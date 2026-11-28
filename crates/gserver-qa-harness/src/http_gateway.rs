#![forbid(unsafe_code)]

//! Multi-threaded HTTP/1.1 SDK Gateway (`HttpSdkGateway`) backed by [`gserver_http`] and
//! [`gserver_protocol`] (`url` + `json`), compatible with `BtPlatformWin32.cpp` and
//! `AccountAuthProvider.cpp` for local `GCLIENT.exe` bootstrap and deterministic QA fuzzing.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gserver_http::http::{
    spawn_http_server, status_reason, HttpHandler, HttpRequest, HttpResponse, HttpServerConfig,
    HttpServerHandle,
};
use gserver_http::protocol::{build_version_response, VersionPolicy, COOKIE_PHPSESSID};
use gserver_protocol::json::{JsonObject, JsonValue};
use gserver_protocol::url::{parse_cookie_header, split_request_target};

use crate::crypto::{
    md5_hex, parse_tolerant_query_params, verify_platform_url_signature, LOCAL_DEV_SIGN_KEY,
};

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

struct QaHttpHandler {
    config: HttpGatewayConfig,
    metrics: Arc<HttpGatewayMetrics>,
    sessions: Mutex<HashMap<String, String>>,
}

impl HttpHandler for QaHttpHandler {
    fn handle(&self, req: &HttpRequest) -> HttpResponse {
        let (status, set_cookie, body) = route_sdk_http_request(
            &req.raw_target,
            req.header("cookie"),
            &self.config,
            &self.metrics,
            &self.sessions,
        );
        let mut resp = HttpResponse::new(status, status_reason(status));
        resp.set_header("Content-Type", "application/json; charset=utf-8");
        if let Some(sc) = set_cookie {
            resp.set_cookies.push(sc);
        }
        resp.body = body.into_bytes();
        resp
    }
}

pub struct HttpSdkGateway {
    addr: SocketAddr,
    metrics: Arc<HttpGatewayMetrics>,
    handle: HttpServerHandle,
}

impl HttpSdkGateway {
    pub fn start(bind_addr: &str, config: HttpGatewayConfig) -> io::Result<Self> {
        let metrics = Arc::new(HttpGatewayMetrics::default());
        let handler = Arc::new(QaHttpHandler {
            config,
            metrics: Arc::clone(&metrics),
            sessions: Mutex::new(HashMap::new()),
        });
        let srv_cfg = HttpServerConfig {
            keep_alive_timeout: Duration::from_secs(3),
            max_requests_per_connection: 16,
        };
        let handle = spawn_http_server(bind_addr, handler, srv_cfg)?;
        let addr = handle.local_addr();
        Ok(Self {
            addr,
            metrics,
            handle,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn metrics(&self) -> &Arc<HttpGatewayMetrics> {
        &self.metrics
    }

    pub fn stop(&mut self) {
        self.handle.shutdown();
    }
}

/// Pure function routing an HTTP request target + `Cookie` header to `(status, set_cookie, json_body)`.
pub fn route_sdk_http_request(
    target: &str,
    cookie_header: Option<&str>,
    cfg: &HttpGatewayConfig,
    metrics: &HttpGatewayMetrics,
    cookie_sessions: &Mutex<HashMap<String, String>>,
) -> (u16, Option<String>, String) {
    metrics.total_requests.fetch_add(1, Ordering::Relaxed);

    let (norm_path, _) = split_request_target(target);
    let is_health_probe = matches!(
        norm_path.as_str(),
        "/" | "/healthz" | "/readyz" | "/livez" | "/health" | "/admin/status"
    );

    if cfg.enforce_signature && !is_health_probe && target.contains('?') {
        if let Err(err) = verify_platform_url_signature(target, &cfg.sign_key) {
            metrics.signature_failures.fetch_add(1, Ordering::Relaxed);
            let mut obj = JsonObject::new();
            obj.insert("errornu", "1");
            obj.insert("errordesc", err.to_string());
            return (403, None, JsonValue::Object(obj).to_json_string());
        }
    }

    let params = parse_tolerant_query_params(target);
    let incoming_phpsessid = cookie_header
        .and_then(|hdr| parse_cookie_header(hdr).remove(COOKIE_PHPSESSID))
        .filter(|v| !v.is_empty());
    if incoming_phpsessid.is_some() {
        metrics.cookie_hits.fetch_add(1, Ordering::Relaxed);
    }

    match norm_path.as_str() {
        "/" | "/healthz" | "/readyz" | "/livez" | "/health" | "/admin/status" => {
            let mut obj = JsonObject::new();
            obj.insert("status", "ok");
            obj.insert("ready", true);
            obj.insert("http_ready", true);
            obj.insert("tcp_ready", true);
            obj.insert("advertised_host", cfg.game_tcp_host.as_str());
            obj.insert("advertised_tcp_port", cfg.game_tcp_port);
            obj.insert("version_policy", "Code_Update_None");
            obj.insert("service", "gserver-qa-harness");
            obj.insert("game", "vnsg2");
            (200, None, JsonValue::Object(obj).to_json_string())
        }
        "/phone/applereview" | "/applereview" => {
            let mut obj = JsonObject::new();
            obj.insert("ret", 404);
            obj.insert("msg", "no data");
            obj.insert("applereview", 0);
            obj.insert("ip", "127.0.0.1");
            obj.insert("errornu", "0");
            obj.insert("errordesc", "OK");
            (200, None, JsonValue::Object(obj).to_json_string())
        }
        "/phone/serverlist" | "/serverlist" => {
            let mut srv = JsonObject::new();
            srv.insert("groupid", "2900161");
            srv.insert("server_id", "game2900161");
            srv.insert("name", "S1.LocalRustQA");
            srv.insert("host", cfg.game_tcp_host.as_str());
            srv.insert("port", cfg.game_tcp_port);
            srv.insert("status", 1);
            srv.insert("hot", 1);
            srv.insert("new", 1);
            srv.insert("openDateTime", "2018-11-06-09-00-00");

            let mut root = JsonObject::new();
            root.insert("item", vec![JsonValue::Object(srv)]);

            let mut obj = JsonObject::new();
            obj.insert("errornu", "0");
            obj.insert("errordesc", "OK");
            obj.insert("root", JsonValue::Object(root));
            (200, None, JsonValue::Object(obj).to_json_string())
        }
        "/phone/supernotice" | "/supernotice" | "/phone/notice" | "/notice" => {
            let mut obj = JsonObject::new();
            obj.insert("errornu", "0");
            obj.insert("errordesc", "OK");
            obj.insert("data", "GServer Local Rust Server Ready");
            (200, None, JsonValue::Object(obj).to_json_string())
        }
        "/phone/getversion" | "/getversion" => {
            let pkg_ver = params
                .get("packageVersion")
                .map(String::as_str)
                .filter(|s| !s.is_empty());
            let scr_ver = params
                .get("scriptVersion")
                .map(String::as_str)
                .filter(|s| !s.is_empty());
            let mut ver_val = build_version_response(&VersionPolicy::default(), pkg_ver, scr_ver);
            if let gserver_http::JsonValue::Object(ref mut obj) = ver_val {
                obj.insert("needUpdate", 0);
            }
            (200, None, ver_val.to_json_string())
        }
        "/phone/login" | "/login" => {
            metrics.login_requests.fetch_add(1, Ordering::Relaxed);
            let userid = params
                .get("userid")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "win32_local_0001".to_string());
            let pid = md5_hex(format!("gserver_pid_{userid}"))[..16].to_string();
            let sess_id = md5_hex(format!("phpsess_{userid}_{}", cfg.fixed_timestamp));
            if let Ok(mut map) = cookie_sessions.lock() {
                map.insert(sess_id.clone(), pid.clone());
            }
            let set_cookie = format!("{COOKIE_PHPSESSID}={sess_id}; Path=/; HttpOnly");
            let mut obj = JsonObject::new();
            obj.insert("errornu", "0");
            obj.insert("uid", pid.as_str());
            obj.insert("pid", pid.as_str());
            obj.insert("errordesc", "OK(1-0)");
            obj.insert("newuser", 0);
            obj.insert("lastLoginTime", cfg.fixed_timestamp);
            obj.insert("regPlosgn", 2);
            obj.insert("env", "local");
            (200, Some(set_cookie), JsonValue::Object(obj).to_json_string())
        }
        "/phone/loginrole" | "/loginrole" => {
            let pid = resolve_pid(&params, incoming_phpsessid.as_deref(), cookie_sessions);
            let mut role = JsonObject::new();
            role.insert("uid", 52592);
            role.insert("pid", pid.as_str());
            role.insert("uname", "LocalGeneral");
            role.insert("level", 120);
            role.insert("quality", 6);
            role.insert("groupid", "2900161");
            role.insert("groupname", "S1.LocalRustQA");
            role.insert("maintainflag", 0);
            role.insert("vip", 10);

            let mut root = JsonObject::new();
            root.insert("role", vec![JsonValue::Object(role)]);
            root.insert("recallOk", 0);

            let mut obj = JsonObject::new();
            obj.insert("root", JsonValue::Object(root));
            obj.insert("errornu", "0");
            obj.insert("errordesc", "OK(1-0)");
            (200, None, JsonValue::Object(obj).to_json_string())
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
            let hash = md5_hex(format!("{pid}:{group_id}:{}", cfg.fixed_timestamp));

            let mut obj = JsonObject::with_capacity(21);
            obj.insert("AMF", 3);
            obj.insert("b_fightLog", 1);
            obj.insert("canCreateUser", 1);
            obj.insert("canPay", 0);
            obj.insert("errordesc", "OK");
            obj.insert("errornu", "0");
            obj.insert("feignRoleId", "487932b");
            obj.insert("host", cfg.game_tcp_host.as_str());
            obj.insert("mergeServerTime", 1_667_530_800_i64);
            obj.insert("offset", "-420");
            obj.insert("openDateTime", "2018-11-06-09-00-00");
            obj.insert("pid", pid);
            obj.insert("plname", "android");
            obj.insert("plosgn", 2);
            obj.insert("port", cfg.game_tcp_port);
            obj.insert("ptype", 0);
            obj.insert("qid", "1243831974477471744");
            obj.insert("serverID", format!("game{group_id}"));
            obj.insert("timestamp", cfg.fixed_timestamp);
            obj.insert("uuid", uuid);
            obj.insert("hash", hash);
            (200, None, JsonValue::Object(obj).to_json_string())
        }
        _ => {
            let mut obj = JsonObject::new();
            obj.insert("errornu", "0");
            obj.insert("errordesc", "OK");
            obj.insert("data", JsonValue::object());
            (200, None, JsonValue::Object(obj).to_json_string())
        }
    }
}

fn resolve_pid(
    params: &BTreeMap<String, String>,
    phpsessid: Option<&str>,
    cookie_sessions: &Mutex<HashMap<String, String>>,
) -> String {
    if let Some(p) = params.get("pid").filter(|s| !s.is_empty()) {
        return p.clone();
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
