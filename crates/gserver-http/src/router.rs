//! Complete HTTP router and handler implementation for GServer (`vnsg2`) MAPI
//! (`/phone/*`), Platform Account / Win32 Guest / Custom Registration (`/api/login/*`),
//! and Server Health/Admin (`/admin/*`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Instant;

use crate::http::{HttpHandler, HttpMethod, HttpRequest, HttpResponse};
use crate::json::{JsonObject, JsonValue};
use crate::md5::{
    compute_account_auth_sig, compute_guest_auth_sig, constant_time_eq_hex, md5_hex,
    verify_mapi_query_signature,
};
use crate::protocol::{
    build_version_lua, build_version_response, derive_feign_role_id, derive_numeric_userid,
    derive_pid_from_userid, AccountRecord, GameServerEntry, HttpCookieSession, IssuedGameHash,
    MapiStore, RoleSummary, ServerConfig, ServerStatus, VersionPolicy, COOKIE_GAME_SESSION,
    COOKIE_PHPSESSID,
};

/// MAPI endpoints guarded by MD5 query signature verification (`BtPlatformWin32::sortUrlParams`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedMapiEndpoint {
    AppleReview,
    GetVersion,
    ServerList,
    Notice { is_super: bool },
    PhoneLogin,
    LoginRole,
    GetHash,
    PlatformUserInfo,
    AgreeVersion,
    EmptyDataOk,
    AddictionOk,
}

fn mapi_ok<'a>(
    desc: &str,
    extra: impl IntoIterator<Item = (&'a str, JsonValue)>,
) -> HttpResponse {
    let mut obj = JsonObject::new();
    obj.insert("errornu", "0");
    obj.insert("errordesc", desc);
    for (k, v) in extra {
        obj.insert(k, v);
    }
    HttpResponse::json(200, &JsonValue::Object(obj))
}

fn mapi_err(status: u16, err_code: Option<i64>, desc: impl Into<String>) -> HttpResponse {
    let mut obj = JsonObject::new();
    obj.insert("errornu", "1");
    if let Some(code) = err_code {
        obj.insert("err_code", code);
    }
    if status == 404 {
        obj.insert("ret", 404);
    }
    obj.insert("errordesc", desc.into());
    HttpResponse::json(status, &JsonValue::Object(obj))
}

fn gethash_err(code: u32, desc: &str, notice: Option<&str>) -> HttpResponse {
    let mut err = JsonObject::new();
    err.insert("errornu", code.to_string());
    err.insert("err_code", code);
    err.insert("errordesc", desc);
    err.insert("err_desc", desc);
    if let Some(msg) = notice {
        err.insert("notice", msg);
    }
    HttpResponse::json(200, &JsonValue::Object(err))
}

fn auth_err(return_code: &str, message: &str) -> HttpResponse {
    let mut err = JsonObject::new();
    err.insert("returnCode", return_code);
    err.insert("errornu", "1");
    err.insert("message", message);
    err.insert("errordesc", message);
    HttpResponse::json(200, &JsonValue::Object(err))
}

fn normalize_password_md5(raw: &str) -> String {
    if raw.len() == 32 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        raw.to_ascii_lowercase()
    } else {
        md5_hex(raw)
    }
}

/// Thread-safe GServer (`vnsg2`) MAPI & Auth HTTP service.
pub struct MapiService {
    config: ServerConfig,
    store: RwLock<MapiStore>,
    started_at: Instant,
    total_requests: AtomicU64,
}

impl MapiService {
    pub fn new(config: ServerConfig) -> Self {
        let store = MapiStore::new(&config);
        Self {
            config,
            store: RwLock::new(store),
            started_at: Instant::now(),
            total_requests: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Sets or clears a deterministic mock clock (in Unix seconds) for session expiry tests.
    pub fn set_mock_time(&self, unix_secs: Option<i64>) {
        if let Ok(mut guard) = self.store.write() {
            guard.mock_now_secs = unix_secs;
        }
    }

    /// Updates the active `/phone/getversion` manifest policy at runtime.
    pub fn set_version_policy(&self, policy: VersionPolicy) {
        if let Ok(mut guard) = self.store.write() {
            guard.version_policy = policy;
        }
    }

    /// Updates coordinated TCP game-server readiness reported by `/healthz` and `/admin/status`.
    pub fn set_tcp_ready_endpoint(&self, ready: Option<bool>, bound_addr: Option<String>) {
        if let Ok(mut guard) = self.store.write() {
            guard.tcp_ready = ready;
            guard.tcp_bound_addr = bound_addr;
        }
    }

    /// Adds or updates a server entry in `/phone/serverlist/`.
    pub fn upsert_server(&self, entry: GameServerEntry) {
        if let Ok(mut guard) = self.store.write() {
            if let Some(existing) = guard.servers.iter_mut().find(|s| s.groupid == entry.groupid) {
                *existing = entry;
            } else {
                guard.servers.push(entry);
            }
        }
    }

    /// Assigns roles for `pid` returned by `/phone/loginrole/`.
    pub fn set_roles_for_pid(&self, pid: impl Into<String>, roles: Vec<RoleSummary>) {
        if let Ok(mut guard) = self.store.write() {
            guard.roles_by_pid.insert(pid.into(), roles);
        }
    }

    /// Verifies a game ticket `hash` issued by `/phone/gethash/`.
    pub fn verify_issued_game_hash(&self, hash: &str, pid: &str) -> Option<IssuedGameHash> {
        let guard = self.store.read().ok()?;
        let now = guard.now_secs();
        let ticket = guard.issued_hashes.get(hash)?;
        if ticket.pid == pid && now <= ticket.expires_at {
            Some(ticket.clone())
        } else {
            None
        }
    }

    /// Unified signature-verification dispatcher for all `/phone/*` MAPI endpoints.
    pub fn dispatch_signed(&self, req: &HttpRequest, endpoint: SignedMapiEndpoint) -> HttpResponse {
        // Support direct Win32 Guest or custom username+password login on `/phone/login`
        // when caller supplies `username`+`password` or `guest=1` without `userid`.
        if endpoint == SignedMapiEndpoint::PhoneLogin {
            let params = req.merged_params();
            if !params.contains_key("userid") {
                if params.contains_key("username") || params.contains_key("u") {
                    return self.handle_account_auth(req);
                }
                if params.get("guest").map(String::as_str) == Some("1")
                    || params.contains_key("deviceID")
                {
                    return self.handle_guest_auth(req);
                }
            }
        }

        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }

        match endpoint {
            SignedMapiEndpoint::AppleReview => self.handle_applereview(req),
            SignedMapiEndpoint::GetVersion => self.handle_getversion(req),
            SignedMapiEndpoint::ServerList => self.handle_serverlist(),
            SignedMapiEndpoint::Notice { is_super } => self.handle_notice(req, is_super),
            SignedMapiEndpoint::PhoneLogin => self.handle_phone_login(req),
            SignedMapiEndpoint::LoginRole => self.handle_loginrole(req),
            SignedMapiEndpoint::GetHash => self.handle_gethash(req),
            SignedMapiEndpoint::PlatformUserInfo => self.handle_platform_user_info(),
            SignedMapiEndpoint::AgreeVersion => self.handle_agree_version(),
            SignedMapiEndpoint::EmptyDataOk => mapi_ok("OK", [("data", JsonValue::object())]),
            SignedMapiEndpoint::AddictionOk => mapi_ok("OK", [("limit_time", JsonValue::from(0))]),
        }
    }

    fn check_mapi_signature(&self, req: &HttpRequest) -> Result<(), HttpResponse> {
        if !self.config.strict_sign && !req.query.contains_key("sign") {
            return Ok(());
        }
        if verify_mapi_query_signature(&req.raw_query, &self.config.accepted_app_keys) {
            Ok(())
        } else {
            Err(mapi_err(200, Some(403), "invalid signature"))
        }
    }

    fn check_platform_sig<F>(
        &self,
        provided_sig: Option<&str>,
        label: &str,
        compute_expected: F,
    ) -> Result<(), HttpResponse>
    where
        F: FnOnce() -> String,
    {
        if self.config.strict_auth_sig || provided_sig.is_some() {
            let Some(sig) = provided_sig else {
                return Err(auth_err("-3", &format!("Missing {}", label)));
            };
            if !constant_time_eq_hex(sig, &compute_expected()) {
                return Err(auth_err("-3", &format!("Invalid {}", label)));
            }
        }
        Ok(())
    }

    fn issue_cookie_session(
        &self,
        store: &mut MapiStore,
        pid: &str,
        user_id: &str,
        uuid: &str,
        plname: &str,
    ) -> HttpCookieSession {
        let now = store.now_secs();
        let nonce = store.next_nonce();
        let session_id = md5_hex(format!(
            "gserver_sess:{}:{}:{}:{}:{}",
            pid, user_id, uuid, now, nonce
        ));
        let sess = HttpCookieSession {
            session_id: session_id.clone(),
            pid: pid.to_string(),
            user_id: user_id.to_string(),
            uuid: uuid.to_string(),
            plname: plname.to_string(),
            created_at: now,
            expires_at: now + self.config.session_ttl_secs,
        };
        store.cookie_sessions.insert(session_id, sess.clone());
        sess
    }

    fn attach_session_cookies(&self, resp: &mut HttpResponse, session_id: &str) {
        let max_age = (self.config.session_ttl_secs > 0).then_some(self.config.session_ttl_secs as u64);
        resp.add_cookie(COOKIE_PHPSESSID, session_id, max_age);
        resp.add_cookie(COOKIE_GAME_SESSION, session_id, max_age);
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_auth_login(
        &self,
        store: &mut MapiStore,
        return_code: &str,
        message: &str,
        user_id: &str,
        pid: &str,
        sid: &str,
        account_name: &str,
        uuid: &str,
        new_user: u64,
    ) -> HttpResponse {
        let cookie_sess = self.issue_cookie_session(store, pid, user_id, uuid, "android");
        let mut resp = mapi_ok(
            "OK",
            [
                ("returnCode", JsonValue::from(return_code)),
                ("message", JsonValue::from(message)),
                ("userID", JsonValue::from(user_id)),
                ("pid", JsonValue::from(pid)),
                ("uid", JsonValue::from(pid)),
                ("sessionID", JsonValue::from(sid)),
                ("token", JsonValue::from(sid)),
                ("accountName", JsonValue::from(account_name)),
                ("newuser", JsonValue::from(new_user)),
            ],
        );
        self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
        resp
    }

    // -----------------------------------------------------------------------
    // 1. /healthz, /admin/status & /admin/verify_hash
    // -----------------------------------------------------------------------

    fn version_policy_tag(policy: &VersionPolicy) -> &'static str {
        match policy {
            VersionPolicy::UpToDate { .. } => "Code_Update_None",
            VersionPolicy::ScriptUpdate { .. } => "Code_Update_Script",
            VersionPolicy::BaseUpdate { .. } => "Code_Update_Base",
        }
    }

    fn handle_healthz(&self, require_tcp_when_attached: bool) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(503, "store lock poisoned");
        };
        let tcp_ready = guard.tcp_ready.unwrap_or(true);
        let overall_ready = !require_tcp_when_attached || tcp_ready;
        let status_code = if overall_ready { 200 } else { 503 };

        let primary_srv = guard.servers.first();
        let adv_host = primary_srv.map(|s| s.host.as_str()).unwrap_or("127.0.0.1");
        let adv_port = primary_srv.map(|s| s.port as u64).unwrap_or(8160);

        let mut obj = JsonObject::new();
        obj.insert("status", if overall_ready { "ok" } else { "degraded" });
        obj.insert("ready", overall_ready);
        obj.insert("http_ready", true);
        obj.insert("tcp_ready", tcp_ready);
        obj.insert(
            "mode",
            if guard.tcp_ready.is_some() {
                "unified_http_tcp"
            } else {
                "standalone_http"
            },
        );
        if let Some(ref addr) = guard.tcp_bound_addr {
            obj.insert("tcp_bound_addr", addr.as_str());
        }
        obj.insert("advertised_host", adv_host);
        obj.insert("advertised_tcp_port", adv_port);
        obj.insert("version_policy", Self::version_policy_tag(&guard.version_policy));
        obj.insert("service", "cobalt-relay");
        obj.insert("game", "vnsg2");
        obj.insert("sdk_version", "0.10.41");
        obj.insert("uptime_secs", self.started_at.elapsed().as_secs());
        HttpResponse::json(status_code, &JsonValue::Object(obj))
    }

    fn handle_admin_status(&self) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let tcp_ready = guard.tcp_ready.unwrap_or(true);
        let mut obj = JsonObject::new();
        obj.insert("status", "ok");
        obj.insert("ready", tcp_ready);
        obj.insert("http_ready", true);
        obj.insert("tcp_ready", tcp_ready);
        if let Some(ref addr) = guard.tcp_bound_addr {
            obj.insert("tcp_bound_addr", addr.as_str());
        }
        if let Some(primary) = guard.servers.first() {
            obj.insert("advertised_host", primary.host.as_str());
            obj.insert("advertised_tcp_port", primary.port as u64);
        }
        obj.insert("version_policy", Self::version_policy_tag(&guard.version_policy));
        obj.insert("service", "gserver-http");
        obj.insert("game", "vnsg2");
        obj.insert("sdk_version", "0.10.41");
        obj.insert("uptime_secs", self.started_at.elapsed().as_secs());
        obj.insert("total_requests", self.total_requests.load(Ordering::Relaxed));
        obj.insert("accounts_count", guard.accounts_by_username.len());
        obj.insert("active_cookie_sessions", guard.cookie_sessions.len());
        obj.insert("active_auth_sessions", guard.auth_sessions.len());
        obj.insert("issued_hashes_count", guard.issued_hashes.len());
        obj.insert("servers_count", guard.servers.len());
        obj.insert("strict_session", self.config.strict_session);
        obj.insert("strict_sign", self.config.strict_sign);
        obj.insert("strict_auth_sig", self.config.strict_auth_sig);
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    fn handle_admin_verify_hash(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();
        let hash = params.get("hash").map(String::as_str).unwrap_or("");
        let pid = params.get("pid").map(String::as_str).unwrap_or("");
        if let Some(ticket) = self.verify_issued_game_hash(hash, pid) {
            mapi_ok(
                "OK",
                [
                    ("valid", JsonValue::from(true)),
                    ("pid", JsonValue::from(ticket.pid)),
                    ("qid", JsonValue::from(ticket.qid)),
                    ("serverID", JsonValue::from(ticket.server_id)),
                    ("group_id", JsonValue::from(ticket.group_id)),
                    ("timestamp", JsonValue::from(ticket.timestamp)),
                ],
            )
        } else {
            let mut obj = JsonObject::new();
            obj.insert("valid", false);
            obj.insert("errornu", "1");
            HttpResponse::json(200, &JsonValue::Object(obj))
        }
    }

    // -----------------------------------------------------------------------
    // 2. /phone/applereview, /phone/getversion, /phone/serverlist & /phone/notice
    // -----------------------------------------------------------------------

    fn handle_applereview(&self, req: &HttpRequest) -> HttpResponse {
        let client_ip = req
            .remote_addr
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        mapi_ok(
            "OK",
            [
                ("ret", JsonValue::from(404)),
                ("msg", JsonValue::from("no data")),
                ("applereview", JsonValue::from(0)),
                ("ip", JsonValue::from(client_ip)),
            ],
        )
    }

    fn handle_getversion(&self, req: &HttpRequest) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let params = req.merged_params();
        let pkg_ver = params
            .get("packageVersion")
            .or_else(|| params.get("package_version"))
            .or_else(|| params.get("pkgVer"))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let scr_ver = params
            .get("scriptVersion")
            .or_else(|| params.get("script_version"))
            .or_else(|| params.get("scrVer"))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        HttpResponse::json(200, &build_version_response(&guard.version_policy, pkg_ver, scr_ver))
    }

    fn handle_version_lua(&self) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        HttpResponse::text(200, build_version_lua(&guard.version_policy))
    }

    fn handle_serverlist(&self) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };

        let items: Vec<JsonValue> = guard.servers.iter().map(GameServerEntry::to_json).collect();
        let mut notice = JsonObject::new();
        notice.insert("open", self.config.serverlist_notice_open as u64);
        notice.insert("desc", self.config.serverlist_notice_desc.as_str());
        notice.insert("url", self.config.serverlist_notice_url.as_str());

        let mut root = JsonObject::new();
        root.insert("item", items);
        root.insert("notice", JsonValue::Object(notice));

        mapi_ok("OK", [("root", JsonValue::Object(root))])
    }

    fn handle_notice(&self, req: &HttpRequest, is_super: bool) -> HttpResponse {
        if req.query.get("format").map(String::as_str) == Some("html") {
            return HttpResponse::html(200, self.config.supernotice_html.clone());
        }
        let return_type = req
            .query
            .get("returntype")
            .map(String::as_str)
            .unwrap_or(if is_super { "tpl" } else { "sgstr" });

        if is_super || return_type == "tpl" {
            mapi_ok("OK", [("data", JsonValue::from(self.config.supernotice_html.as_str()))])
        } else {
            mapi_ok(
                "OK",
                [
                    ("data", JsonValue::from(self.config.notice_text.as_str())),
                    ("notice", JsonValue::from(self.config.notice_text.as_str())),
                ],
            )
        }
    }

    // -----------------------------------------------------------------------
    // 3. Platform Account, Win32 Guest, and Custom Username+Password Auth
    // -----------------------------------------------------------------------

    fn handle_account_auth(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();
        let username = params
            .get("u")
            .or_else(|| params.get("username"))
            .map(|s| s.trim())
            .unwrap_or("");
        let raw_pw = params
            .get("p")
            .cloned()
            .or_else(|| params.get("password").map(md5_hex))
            .unwrap_or_default();
        let timestamp = params.get("timestamp").map(String::as_str).unwrap_or("0");
        let game_id = params
            .get("gameID")
            .map(String::as_str)
            .unwrap_or(self.config.gtsdk_game_id.as_str());

        if username.is_empty() || raw_pw.is_empty() {
            return auth_err("-1", "Missing username or password");
        }

        let pass_md5 = normalize_password_md5(&raw_pw);
        if let Err(resp) = self.check_platform_sig(
            params.get("sig").map(String::as_str),
            "Platform signature",
            || {
                compute_account_auth_sig(
                    &self.config.auth_secret,
                    game_id,
                    username,
                    &pass_md5,
                    timestamp,
                )
            },
        ) {
            return resp;
        }

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };

        let now = store.now_secs();
        let nonce = store.next_nonce();
        let uname_key = username.to_ascii_lowercase();

        if let Some(existing) = store.accounts_by_username.get(&uname_key).cloned() {
            if !existing.is_guest && !constant_time_eq_hex(&existing.password_md5, &pass_md5) {
                return auth_err("-1", "Invalid username or password");
            }
            let sid = format!(
                "SID_{}{}",
                md5_hex(format!("acct:{}:{}:{}", existing.user_id, now, nonce)),
                now
            );
            store.auth_sessions.insert(sid.clone(), existing.user_id.clone());
            if let Some(acct_mut) = store.accounts_by_username.get_mut(&uname_key) {
                acct_mut.auth_session_id = sid.clone();
                acct_mut.last_login_time = now;
                acct_mut.login_count += 1;
            }
            return self.finish_auth_login(
                &mut store,
                "1",
                "OK",
                &existing.user_id,
                &existing.pid,
                &sid,
                &existing.username,
                "",
                0,
            );
        }

        if !self.config.auto_register_on_account_login {
            return auth_err("-1", "Account does not exist");
        }

        let user_id = derive_numeric_userid(&uname_key);
        let pid = derive_pid_from_userid(&user_id);
        let sid = format!("SID_{}{}", md5_hex(format!("acct_new:{}:{}:{}", user_id, now, nonce)), now);
        store.insert_account(AccountRecord {
            user_id: user_id.clone(),
            pid: pid.clone(),
            username: username.to_string(),
            password_md5: pass_md5,
            is_guest: false,
            device_id: None,
            auth_session_id: sid.clone(),
            created_at: now,
            last_login_time: now,
            login_count: 1,
            reg_plosgn: self.config.default_plosgn,
        });
        self.finish_auth_login(
            &mut store,
            "2",
            "Account registered and signed in",
            &user_id,
            &pid,
            &sid,
            username,
            "",
            1,
        )
    }

    fn handle_register(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();
        let username = params
            .get("username")
            .or_else(|| params.get("u"))
            .map(|s| s.trim())
            .unwrap_or("");
        let raw_pw = params
            .get("password")
            .or_else(|| params.get("p"))
            .cloned()
            .unwrap_or_default();

        if username.len() < 3 || username.len() > 64 || raw_pw.is_empty() {
            return auth_err("-1", "Username (3..64 chars) and password are required");
        }

        let pass_md5 = normalize_password_md5(&raw_pw);
        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let uname_key = username.to_ascii_lowercase();
        if store.accounts_by_username.contains_key(&uname_key) {
            return auth_err("-2", "Username already exists");
        }

        let now = store.now_secs();
        let nonce = store.next_nonce();
        let user_id = derive_numeric_userid(&uname_key);
        let pid = derive_pid_from_userid(&user_id);
        let sid = format!("SID_{}{}", md5_hex(format!("reg:{}:{}:{}", user_id, now, nonce)), now);
        let uuid = params.get("uuid").cloned().unwrap_or_default();

        store.insert_account(AccountRecord {
            user_id: user_id.clone(),
            pid: pid.clone(),
            username: username.to_string(),
            password_md5: pass_md5,
            is_guest: false,
            device_id: (!uuid.is_empty()).then(|| uuid.clone()),
            auth_session_id: sid.clone(),
            created_at: now,
            last_login_time: now,
            login_count: 1,
            reg_plosgn: self.config.default_plosgn,
        });
        self.finish_auth_login(&mut store, "2", "Registered", &user_id, &pid, &sid, username, &uuid, 1)
    }

    fn handle_guest_auth(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();
        let device_id = params
            .get("deviceID")
            .or_else(|| params.get("uuid"))
            .map(|s| s.trim())
            .unwrap_or("");
        let timestamp = params.get("timestamp").map(String::as_str).unwrap_or("0");
        let game_id = params
            .get("gameID")
            .map(String::as_str)
            .unwrap_or(self.config.gtsdk_game_id.as_str());

        if device_id.is_empty() {
            return auth_err("-1", "Missing deviceID");
        }

        if let Err(resp) = self.check_platform_sig(
            params.get("sig").map(String::as_str),
            "Platform guest signature",
            || compute_guest_auth_sig(&self.config.auth_secret, game_id, device_id, timestamp),
        ) {
            return resp;
        }

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let now = store.now_secs();
        let nonce = store.next_nonce();

        if let Some(existing_uid) = store.guest_userid_by_device.get(device_id).cloned() {
            if let Some(existing_uname) = store.username_by_userid.get(&existing_uid).cloned() {
                if let Some(existing) = store.accounts_by_username.get(&existing_uname).cloned() {
                    let sid = format!(
                        "SID_{}{}",
                        md5_hex(format!("guest:{}:{}:{}", existing.user_id, now, nonce)),
                        now
                    );
                    store.auth_sessions.insert(sid.clone(), existing.user_id.clone());
                    if let Some(acct_mut) = store.accounts_by_username.get_mut(&existing_uname) {
                        acct_mut.auth_session_id = sid.clone();
                        acct_mut.last_login_time = now;
                        acct_mut.login_count += 1;
                    }
                    return self.finish_auth_login(
                        &mut store,
                        "1",
                        "OK",
                        &existing.user_id,
                        &existing.pid,
                        &sid,
                        "Guest",
                        device_id,
                        0,
                    );
                }
            }
        }

        let guest_uname = format!("guest_{}", &md5_hex(device_id)[..12]);
        let user_id = derive_numeric_userid(&format!("guest:{}", device_id));
        let pid = derive_pid_from_userid(&user_id);
        let sid = format!(
            "SID_{}{}",
            md5_hex(format!("guest_new:{}:{}:{}", user_id, now, nonce)),
            now
        );
        store.insert_account(AccountRecord {
            user_id: user_id.clone(),
            pid: pid.clone(),
            username: guest_uname,
            password_md5: String::new(),
            is_guest: true,
            device_id: Some(device_id.to_string()),
            auth_session_id: sid.clone(),
            created_at: now,
            last_login_time: now,
            login_count: 1,
            reg_plosgn: self.config.default_plosgn,
        });
        self.finish_auth_login(
            &mut store,
            "2",
            "Guest created",
            &user_id,
            &pid,
            &sid,
            "Guest",
            device_id,
            1,
        )
    }

    // -----------------------------------------------------------------------
    // 4. /phone/login, /phone/loginrole & /phone/gethash
    // -----------------------------------------------------------------------

    fn handle_phone_login(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();
        let user_id = params.get("userid").map(|s| s.trim()).unwrap_or("");
        let session_id = params.get("sessionid").map(|s| s.trim()).unwrap_or("");
        let uuid = params
            .get("uuid")
            .or_else(|| params.get("bind"))
            .map(|s| s.trim())
            .unwrap_or("");
        let plname = params.get("pl").map(String::as_str).unwrap_or("android");

        if user_id.is_empty() || session_id.is_empty() {
            return mapi_err(200, None, "missing userid or sessionid");
        }

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };

        if self.config.strict_auth_token {
            let valid = store
                .auth_sessions
                .get(session_id)
                .map(|bound_uid| bound_uid == user_id)
                .unwrap_or(false);
            if !valid {
                return mapi_err(401, None, "invalid or expired Platform sessionid");
            }
        }

        let now = store.now_secs();
        let (pid, new_user_flag, prev_login_time, reg_plosgn) =
            if let Some(acct) = store.find_account_by_userid_mut(user_id) {
                let prev = acct.last_login_time;
                acct.last_login_time = now;
                acct.login_count += 1;
                acct.auth_session_id = session_id.to_string();
                (acct.pid.clone(), 0u64, prev, acct.reg_plosgn)
            } else {
                let pid_cl = derive_pid_from_userid(user_id);
                store.insert_account(AccountRecord {
                    user_id: user_id.to_string(),
                    pid: pid_cl.clone(),
                    username: format!("acct_{}", user_id),
                    password_md5: String::new(),
                    is_guest: user_id.starts_with("win32_guest"),
                    device_id: (!uuid.is_empty()).then(|| uuid.to_string()),
                    auth_session_id: session_id.to_string(),
                    created_at: now,
                    last_login_time: now,
                    login_count: 1,
                    reg_plosgn: self.config.default_plosgn,
                });
                (pid_cl, 1u64, now, self.config.default_plosgn)
            };

        store.auth_sessions.insert(session_id.to_string(), user_id.to_string());
        let cookie_sess = self.issue_cookie_session(&mut store, &pid, user_id, uuid, plname);

        let mut resp = mapi_ok(
            "OK(1-0)",
            [
                ("uid", JsonValue::from(pid.as_str())),
                ("pid", JsonValue::from(pid.as_str())),
                ("newuser", JsonValue::from(new_user_flag)),
                ("lastLoginTime", JsonValue::from(prev_login_time)),
                ("regPlosgn", JsonValue::from(reg_plosgn)),
                ("env", JsonValue::from("online")),
            ],
        );
        self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
        resp
    }

    fn handle_loginrole(&self, req: &HttpRequest) -> HttpResponse {
        let pid = req.query.get("pid").map(String::as_str).unwrap_or("");
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };

        let roles: Vec<JsonValue> = guard
            .roles_by_pid
            .get(pid)
            .map(|list| list.iter().map(RoleSummary::to_json).collect())
            .unwrap_or_default();

        let mut root = JsonObject::new();
        root.insert("role", roles);
        root.insert("recallOk", 0);

        mapi_ok("OK(1-0)", [("root", JsonValue::Object(root))])
    }

    fn handle_gethash(&self, req: &HttpRequest) -> HttpResponse {
        let pid = req.query.get("pid").map(|s| s.trim()).unwrap_or("");
        let uuid = req.query.get("uuid").map(|s| s.trim()).unwrap_or("");
        let plname = req.query.get("pl").map(|s| s.trim()).unwrap_or("android");
        let group_id = req
            .query
            .get("group_id")
            .or_else(|| req.query.get("serverID"))
            .map(|s| s.trim())
            .unwrap_or("");

        if pid.is_empty() {
            return gethash_err(103, "no user", None);
        }

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let now = store.now_secs();

        let cookie_token = req
            .cookie(COOKIE_PHPSESSID)
            .or_else(|| req.cookie(COOKIE_GAME_SESSION));

        let mut bound_qid: Option<String> = None;
        if self.config.strict_session {
            let session_ok = match cookie_token.and_then(|tok| store.cookie_sessions.get(tok)) {
                Some(sess) if !sess.is_expired(now) && sess.pid == pid => {
                    bound_qid = Some(sess.user_id.clone());
                    true
                }
                _ => false,
            };
            if !session_ok {
                return gethash_err(102, "session time out", None);
            }
        } else if let Some(sess) = cookie_token.and_then(|tok| store.cookie_sessions.get(tok)) {
            if sess.pid == pid {
                bound_qid = Some(sess.user_id.clone());
            }
        }

        let Some(server) = store
            .servers
            .iter()
            .find(|s| s.groupid == group_id || s.group == group_id)
            .cloned()
        else {
            return gethash_err(101, "no group msg", None);
        };

        if server.status == ServerStatus::Maintenance as u8 {
            return gethash_err(104, "maintain is open", Some(server.maintenance_notice.as_str()));
        }

        let qid = bound_qid
            .or_else(|| store.find_account_by_pid(pid).map(|a| a.user_id.clone()))
            .unwrap_or_else(|| derive_numeric_userid(pid));

        let plosgn = store
            .find_account_by_pid(pid)
            .map(|a| a.reg_plosgn)
            .unwrap_or(self.config.default_plosgn);

        let nonce = store.next_nonce();
        let hash = md5_hex(format!(
            "gethash:{}:{}:{}:{}:{}:{}",
            pid, server.group, now, uuid, nonce, self.config.game_hash_secret
        ));

        store.issued_hashes.insert(
            hash.clone(),
            IssuedGameHash {
                hash: hash.clone(),
                pid: pid.to_string(),
                qid: qid.clone(),
                uuid: uuid.to_string(),
                group_id: server.groupid.clone(),
                server_id: server.group.clone(),
                timestamp: now,
                expires_at: now + 600,
            },
        );

        let feign_role_id = derive_feign_role_id(pid, &server.groupid);
        mapi_ok(
            "OK",
            [
                ("AMF", JsonValue::from(3)),
                ("b_fightLog", JsonValue::from(1)),
                ("canCreateUser", JsonValue::from(1)),
                ("canPay", JsonValue::from(1)),
                ("feignRoleId", JsonValue::from(feign_role_id)),
                ("host", JsonValue::from(server.host)),
                ("mergeServerTime", JsonValue::from(server.merge_server_time)),
                ("offset", JsonValue::from(server.offset)),
                ("openDateTime", JsonValue::from(server.open_date_time_str)),
                ("pid", JsonValue::from(pid)),
                ("plname", JsonValue::from(plname)),
                ("plosgn", JsonValue::from(plosgn)),
                ("port", JsonValue::from(server.port)),
                ("ptype", JsonValue::from(0)),
                ("qid", JsonValue::from(qid)),
                ("serverID", JsonValue::from(server.group)),
                ("timestamp", JsonValue::from(now)),
                ("uuid", JsonValue::from(uuid)),
                ("hash", JsonValue::from(hash)),
            ],
        )
    }

    // -----------------------------------------------------------------------
    // 5. Auxiliary Universal Web Endpoints in `BtPlatformWin32.cpp` & `SDKUtil.lua`
    // -----------------------------------------------------------------------

    fn handle_platform_user_info(&self) -> HttpResponse {
        let mut data = JsonObject::new();
        for (k, v) in [
            ("idcardStatus", 1),
            ("isForceRealName", 0),
            ("is_fastreg", 0),
            ("isAdult", 1),
            ("isBindPhone", 1),
            ("isConcern", 0),
            ("isFastUser", 0),
        ] {
            data.insert(k, v);
        }
        mapi_ok("success", [("data", JsonValue::Object(data))])
    }

    fn handle_agree_version(&self) -> HttpResponse {
        let mut data = JsonObject::new();
        data.insert("status", 0);
        data.insert("version", "1.0");
        data.insert("newUserAgree", "");
        data.insert("newUserPrivacy", "");
        data.insert("userChildPrivacy", "");
        mapi_ok("OK", [("data", JsonValue::Object(data))])
    }
}

impl HttpHandler for MapiService {
    fn handle(&self, req: &HttpRequest) -> HttpResponse {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        if !matches!(req.method, HttpMethod::Get | HttpMethod::Post | HttpMethod::Head) {
            return mapi_err(405, None, "method not allowed");
        }

        match req.path.as_str() {
            "/healthz" | "/readyz" => self.handle_healthz(true),
            "/livez" => self.handle_healthz(false),
            "/" | "/admin/status" | "/health" => self.handle_admin_status(),
            "/admin/verify_hash" => self.handle_admin_verify_hash(req),
            "/version.lua" | "/scripts/version.lua" => self.handle_version_lua(),

            // Core GServer signed MAPI endpoints (`/phone/*` + standalone `/getversion` fallback)
            "/phone/applereview" => self.dispatch_signed(req, SignedMapiEndpoint::AppleReview),
            "/phone/getversion" | "/getversion" => {
                self.dispatch_signed(req, SignedMapiEndpoint::GetVersion)
            }
            "/phone/serverlist" => self.dispatch_signed(req, SignedMapiEndpoint::ServerList),
            "/phone/notice" => {
                self.dispatch_signed(req, SignedMapiEndpoint::Notice { is_super: false })
            }
            "/phone/supernotice" => {
                self.dispatch_signed(req, SignedMapiEndpoint::Notice { is_super: true })
            }
            "/phone/login" => self.dispatch_signed(req, SignedMapiEndpoint::PhoneLogin),
            "/phone/loginrole" => self.dispatch_signed(req, SignedMapiEndpoint::LoginRole),
            "/phone/gethash" => self.dispatch_signed(req, SignedMapiEndpoint::GetHash),

            // Platform Account / Win32 Guest / Custom Registration & Login endpoints
            "/api/login/auth/account" | "/api/login/login" => self.handle_account_auth(req),
            "/api/login/createUser/guest" | "/phone/guest" => self.handle_guest_auth(req),
            "/api/login/register" | "/phone/register" => self.handle_register(req),

            // Auxiliary SDK endpoints in `BtPlatformWin32.cpp` & `SDKUtil.lua`
            "/phone/platform/getPlatformUserInfo" => {
                self.dispatch_signed(req, SignedMapiEndpoint::PlatformUserInfo)
            }
            "/phone/helper/getagreeversion" => {
                self.dispatch_signed(req, SignedMapiEndpoint::AgreeVersion)
            }
            "/phone/addiction/getaddiction" | "/phone/addiction/clear" => {
                self.dispatch_signed(req, SignedMapiEndpoint::AddictionOk)
            }
            "/phone/userinfo"
            | "/phone/innerbrowse"
            | "/phone/question"
            | "/phone/getforcefix" => self.dispatch_signed(req, SignedMapiEndpoint::EmptyDataOk),

            _ => mapi_err(404, None, format!("unknown endpoint: {}", req.path)),
        }
    }
}
