//! Complete HTTP router and handler implementation for GServer (`vnsg2`) MAPI
//! (`/phone/*`), Platform Account / Win32 Guest / Custom Registration (`/api/login/*`),
//! and Server Health/Admin (`/admin/*`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Instant;

use crate::http::{HttpHandler, HttpMethod, HttpRequest, HttpResponse};
use crate::json::{JsonObject, JsonValue};
use crate::md5::{
    compute_guest_auth_sig, compute_account_auth_sig, constant_time_eq_hex, md5_hex,
    verify_mapi_query_signature,
};
use crate::protocol::{
    build_version_lua, build_version_response, derive_feign_role_id, derive_numeric_userid,
    derive_pid_from_userid, AccountRecord, GameServerEntry, HttpCookieSession, IssuedGameHash,
    MapiStore, RoleSummary, ServerConfig, ServerStatus, VersionPolicy, COOKIE_GAME_SESSION,
    COOKIE_PHPSESSID,
};

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

    fn check_mapi_signature(&self, req: &HttpRequest) -> Result<(), HttpResponse> {
        let has_sign = req.query.contains_key("sign");
        if !self.config.strict_sign && !has_sign {
            return Ok(());
        }
        if verify_mapi_query_signature(&req.raw_query, &self.config.accepted_app_keys) {
            Ok(())
        } else {
            let mut obj = JsonObject::new();
            obj.insert("errornu", "1");
            obj.insert("err_code", 403);
            obj.insert("errordesc", "invalid signature");
            Err(HttpResponse::json(200, &JsonValue::Object(obj)))
        }
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
        let max_age = if self.config.session_ttl_secs > 0 {
            Some(self.config.session_ttl_secs as u64)
        } else {
            None
        };
        resp.add_cookie(COOKIE_PHPSESSID, session_id, max_age);
        resp.add_cookie(COOKIE_GAME_SESSION, session_id, max_age);
    }

    // -----------------------------------------------------------------------
    // 1. /admin/status & /admin/verify_hash
    // -----------------------------------------------------------------------

    fn handle_admin_status(&self) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let mut obj = JsonObject::new();
        obj.insert("status", "ok");
        obj.insert("service", "gserver-http");
        obj.insert("game", "vnsg2");
        obj.insert("sdk_version", "0.10.41");
        obj.insert("uptime_secs", self.started_at.elapsed().as_secs());
        obj.insert(
            "total_requests",
            self.total_requests.load(Ordering::Relaxed),
        );
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
        let mut obj = JsonObject::new();
        if let Some(ticket) = self.verify_issued_game_hash(hash, pid) {
            obj.insert("valid", true);
            obj.insert("errornu", "0");
            obj.insert("pid", ticket.pid);
            obj.insert("qid", ticket.qid);
            obj.insert("serverID", ticket.server_id);
            obj.insert("group_id", ticket.group_id);
            obj.insert("timestamp", ticket.timestamp);
        } else {
            obj.insert("valid", false);
            obj.insert("errornu", "1");
        }
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    // -----------------------------------------------------------------------
    // 2. /phone/applereview
    // -----------------------------------------------------------------------

    fn handle_applereview(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
        let client_ip = req
            .remote_addr
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let mut obj = JsonObject::new();
        obj.insert("ret", 404);
        obj.insert("msg", "no data");
        obj.insert("applereview", 0);
        obj.insert("ip", client_ip);
        obj.insert("errornu", "0");
        obj.insert("errordesc", "OK");
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    // -----------------------------------------------------------------------
    // 3. /phone/getversion & /version.lua
    // -----------------------------------------------------------------------

    fn handle_getversion(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let pkg_ver = req.query.get("packageVersion").map(String::as_str);
        let scr_ver = req.query.get("scriptVersion").map(String::as_str);
        let body = build_version_response(&guard.version_policy, pkg_ver, scr_ver);
        HttpResponse::json(200, &body)
    }

    fn handle_version_lua(&self) -> HttpResponse {
        let Ok(guard) = self.store.read() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        HttpResponse::text(200, build_version_lua(&guard.version_policy))
    }

    // -----------------------------------------------------------------------
    // 4. /phone/serverlist
    // -----------------------------------------------------------------------

    fn handle_serverlist(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
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

        let mut resp_obj = JsonObject::new();
        resp_obj.insert("errornu", "0");
        resp_obj.insert("errordesc", "OK");
        resp_obj.insert("root", JsonValue::Object(root));

        HttpResponse::json(200, &JsonValue::Object(resp_obj))
    }

    // -----------------------------------------------------------------------
    // 5. /phone/notice & /phone/supernotice
    // -----------------------------------------------------------------------

    fn handle_notice(&self, req: &HttpRequest, is_super: bool) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
        let return_type = req
            .query
            .get("returntype")
            .map(String::as_str)
            .unwrap_or(if is_super { "tpl" } else { "sgstr" });

        // Support direct WebView HTML rendering when explicitly requested via `format=html`.
        if req.query.get("format").map(String::as_str) == Some("html") {
            return HttpResponse::html(200, self.config.supernotice_html.clone());
        }

        let mut obj = JsonObject::new();
        obj.insert("errornu", "0");
        obj.insert("errordesc", "OK");
        if is_super || return_type == "tpl" {
            obj.insert("data", self.config.supernotice_html.as_str());
        } else {
            obj.insert("data", self.config.notice_text.as_str());
            obj.insert("notice", self.config.notice_text.as_str());
        }
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    // -----------------------------------------------------------------------
    // 6. Platform Account, Win32 Guest, and Custom Username+Password Registration/Login
    // -----------------------------------------------------------------------

    fn handle_account_auth(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();
        let username = params
            .get("u")
            .or_else(|| params.get("username"))
            .map(|s| s.trim())
            .unwrap_or("");
        let raw_pass_or_md5 = params
            .get("p")
            .cloned()
            .or_else(|| params.get("password").map(md5_hex))
            .unwrap_or_default();
        let timestamp = params.get("timestamp").map(String::as_str).unwrap_or("0");
        let game_id = params
            .get("gameID")
            .map(String::as_str)
            .unwrap_or(self.config.gtsdk_game_id.as_str());

        if username.is_empty() || raw_pass_or_md5.is_empty() {
            let mut err = JsonObject::new();
            err.insert("returnCode", "-1");
            err.insert("errornu", "1");
            err.insert("message", "Missing username or password");
            err.insert("errordesc", "Missing username or password");
            return HttpResponse::json(200, &JsonValue::Object(err));
        }

        // Normalize `p`: if client passed 32-hex MD5 (like AccountAuthProvider.cpp), use it lowercase;
        // otherwise MD5-hash plaintext password.
        let pass_md5 = if raw_pass_or_md5.len() == 32
            && raw_pass_or_md5.bytes().all(|b| b.is_ascii_hexdigit())
        {
            raw_pass_or_md5.to_ascii_lowercase()
        } else {
            md5_hex(&raw_pass_or_md5)
        };

        let provided_sig = params.get("sig").map(String::as_str);
        if self.config.strict_auth_sig || provided_sig.is_some() {
            let Some(sig) = provided_sig else {
                let mut err = JsonObject::new();
                err.insert("returnCode", "-3");
                err.insert("message", "Missing Platform signature");
                return HttpResponse::json(200, &JsonValue::Object(err));
            };
            let expected = compute_account_auth_sig(
                &self.config.auth_secret,
                game_id,
                username,
                &pass_md5,
                timestamp,
            );
            if !constant_time_eq_hex(sig, &expected) {
                let mut err = JsonObject::new();
                err.insert("returnCode", "-3");
                err.insert("message", "Invalid Platform signature");
                return HttpResponse::json(200, &JsonValue::Object(err));
            }
        }

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };

        let now = store.now_secs();
        let nonce = store.next_nonce();
        let uname_key = username.to_ascii_lowercase();

        if let Some(existing) = store.accounts_by_username.get(&uname_key).cloned() {
            if !existing.is_guest && !constant_time_eq_hex(&existing.password_md5, &pass_md5) {
                let mut err = JsonObject::new();
                err.insert("returnCode", "-1");
                err.insert("errornu", "1");
                err.insert("message", "Invalid username or password");
                err.insert("errordesc", "Invalid username or password");
                return HttpResponse::json(200, &JsonValue::Object(err));
            }
            let new_sid = format!(
                "SID_{}{}",
                md5_hex(format!("acct:{}:{}:{}", existing.user_id, now, nonce)),
                now
            );
            store
                .auth_sessions
                .insert(new_sid.clone(), existing.user_id.clone());
            if let Some(acct_mut) = store.accounts_by_username.get_mut(&uname_key) {
                acct_mut.auth_session_id = new_sid.clone();
                acct_mut.last_login_time = now;
                acct_mut.login_count += 1;
            }
            let cookie_sess =
                self.issue_cookie_session(&mut store, &existing.pid, &existing.user_id, "", "android");

            let mut out = JsonObject::new();
            out.insert("returnCode", "1");
            out.insert("errornu", "0");
            out.insert("errordesc", "OK");
            out.insert("message", "OK");
            out.insert("userID", existing.user_id);
            out.insert("pid", existing.pid.as_str());
            out.insert("uid", existing.pid);
            out.insert("sessionID", new_sid.as_str());
            out.insert("token", new_sid);
            out.insert("accountName", existing.username);
            out.insert("newuser", 0);
            let mut resp = HttpResponse::json(200, &JsonValue::Object(out));
            self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
            return resp;
        }

        if !self.config.auto_register_on_account_login {
            let mut err = JsonObject::new();
            err.insert("returnCode", "-1");
            err.insert("errornu", "1");
            err.insert("message", "Account does not exist");
            err.insert("errordesc", "Account does not exist");
            return HttpResponse::json(200, &JsonValue::Object(err));
        }

        let user_id = derive_numeric_userid(&uname_key);
        let pid = derive_pid_from_userid(&user_id);
        let sid = format!("SID_{}{}", md5_hex(format!("acct_new:{}:{}:{}", user_id, now, nonce)), now);
        let acct = AccountRecord {
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
        };
        store.insert_account(acct);
        let cookie_sess = self.issue_cookie_session(&mut store, &pid, &user_id, "", "android");

        let mut out = JsonObject::new();
        out.insert("returnCode", "2");
        out.insert("errornu", "0");
        out.insert("errordesc", "OK");
        out.insert("message", "Account registered and signed in");
        out.insert("userID", user_id);
        out.insert("pid", pid.as_str());
        out.insert("uid", pid);
        out.insert("sessionID", sid.as_str());
        out.insert("token", sid);
        out.insert("accountName", username);
        out.insert("newuser", 1);
        let mut resp = HttpResponse::json(200, &JsonValue::Object(out));
        self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
        resp
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
            .cloned()
            .or_else(|| params.get("p").cloned())
            .unwrap_or_default();

        if username.len() < 3 || username.len() > 64 || raw_pw.is_empty() {
            let mut err = JsonObject::new();
            err.insert("errornu", "1");
            err.insert("returnCode", "-1");
            err.insert("errordesc", "Username (3..64 chars) and password are required");
            err.insert("message", "Username (3..64 chars) and password are required");
            return HttpResponse::json(200, &JsonValue::Object(err));
        }

        let pass_md5 = if raw_pw.len() == 32 && raw_pw.bytes().all(|b| b.is_ascii_hexdigit()) {
            raw_pw.to_ascii_lowercase()
        } else {
            md5_hex(&raw_pw)
        };

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let uname_key = username.to_ascii_lowercase();
        if store.accounts_by_username.contains_key(&uname_key) {
            let mut err = JsonObject::new();
            err.insert("errornu", "1");
            err.insert("returnCode", "-2");
            err.insert("errordesc", "Username already exists");
            err.insert("message", "Username already exists");
            return HttpResponse::json(200, &JsonValue::Object(err));
        }

        let now = store.now_secs();
        let nonce = store.next_nonce();
        let user_id = derive_numeric_userid(&uname_key);
        let pid = derive_pid_from_userid(&user_id);
        let sid = format!("SID_{}{}", md5_hex(format!("reg:{}:{}:{}", user_id, now, nonce)), now);
        let uuid = params.get("uuid").cloned().unwrap_or_default();

        let acct = AccountRecord {
            user_id: user_id.clone(),
            pid: pid.clone(),
            username: username.to_string(),
            password_md5: pass_md5,
            is_guest: false,
            device_id: if uuid.is_empty() { None } else { Some(uuid.clone()) },
            auth_session_id: sid.clone(),
            created_at: now,
            last_login_time: now,
            login_count: 1,
            reg_plosgn: self.config.default_plosgn,
        };
        store.insert_account(acct);
        let cookie_sess = self.issue_cookie_session(&mut store, &pid, &user_id, &uuid, "android");

        let mut out = JsonObject::new();
        out.insert("errornu", "0");
        out.insert("errordesc", "OK");
        out.insert("returnCode", "2");
        out.insert("message", "Registered");
        out.insert("userID", user_id);
        out.insert("pid", pid.as_str());
        out.insert("uid", pid);
        out.insert("sessionID", sid.as_str());
        out.insert("token", sid);
        out.insert("accountName", username);
        out.insert("newuser", 1);

        let mut resp = HttpResponse::json(200, &JsonValue::Object(out));
        self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
        resp
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
            let mut err = JsonObject::new();
            err.insert("returnCode", "-1");
            err.insert("errornu", "1");
            err.insert("message", "Missing deviceID");
            err.insert("errordesc", "Missing deviceID");
            return HttpResponse::json(200, &JsonValue::Object(err));
        }

        let provided_sig = params.get("sig").map(String::as_str);
        if self.config.strict_auth_sig || provided_sig.is_some() {
            let Some(sig) = provided_sig else {
                let mut err = JsonObject::new();
                err.insert("returnCode", "-3");
                err.insert("message", "Missing Platform guest signature");
                return HttpResponse::json(200, &JsonValue::Object(err));
            };
            let expected = compute_guest_auth_sig(&self.config.auth_secret, game_id, device_id, timestamp);
            if !constant_time_eq_hex(sig, &expected) {
                let mut err = JsonObject::new();
                err.insert("returnCode", "-3");
                err.insert("message", "Invalid Platform guest signature");
                return HttpResponse::json(200, &JsonValue::Object(err));
            }
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
                    store
                        .auth_sessions
                        .insert(sid.clone(), existing.user_id.clone());
                    if let Some(acct_mut) = store.accounts_by_username.get_mut(&existing_uname) {
                        acct_mut.auth_session_id = sid.clone();
                        acct_mut.last_login_time = now;
                        acct_mut.login_count += 1;
                    }
                    let cookie_sess = self.issue_cookie_session(
                        &mut store,
                        &existing.pid,
                        &existing.user_id,
                        device_id,
                        "android",
                    );

                    let mut out = JsonObject::new();
                    out.insert("returnCode", "1");
                    out.insert("errornu", "0");
                    out.insert("errordesc", "OK");
                    out.insert("message", "OK");
                    out.insert("userID", existing.user_id);
                    out.insert("pid", existing.pid.as_str());
                    out.insert("uid", existing.pid);
                    out.insert("sessionID", sid.as_str());
                    out.insert("token", sid);
                    out.insert("accountName", "Guest");
                    out.insert("newuser", 0);
                    let mut resp = HttpResponse::json(200, &JsonValue::Object(out));
                    self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
                    return resp;
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
        let acct = AccountRecord {
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
        };
        store.insert_account(acct);
        let cookie_sess =
            self.issue_cookie_session(&mut store, &pid, &user_id, device_id, "android");

        let mut out = JsonObject::new();
        out.insert("returnCode", "2");
        out.insert("errornu", "0");
        out.insert("errordesc", "OK");
        out.insert("message", "Guest created");
        out.insert("userID", user_id);
        out.insert("pid", pid.as_str());
        out.insert("uid", pid);
        out.insert("sessionID", sid.as_str());
        out.insert("token", sid);
        out.insert("accountName", "Guest");
        out.insert("newuser", 1);
        let mut resp = HttpResponse::json(200, &JsonValue::Object(out));
        self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
        resp
    }

    // -----------------------------------------------------------------------
    // 7. /phone/login (MAPI session exchange -> Set-Cookie: PHPSESSID & GSESS_ID)
    // -----------------------------------------------------------------------

    fn handle_phone_login(&self, req: &HttpRequest) -> HttpResponse {
        let params = req.merged_params();

        // Also support direct Win32 Guest or custom username+password login on `/phone/login`
        // if caller supplies `username`+`password` or `guest=1` without `userid`.
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

        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }

        let user_id = params.get("userid").map(|s| s.trim()).unwrap_or("");
        let session_id = params.get("sessionid").map(|s| s.trim()).unwrap_or("");
        let uuid = params
            .get("uuid")
            .or_else(|| params.get("bind"))
            .map(|s| s.trim())
            .unwrap_or("");
        let plname = params.get("pl").map(String::as_str).unwrap_or("android");

        if user_id.is_empty() || session_id.is_empty() {
            let mut err = JsonObject::new();
            err.insert("errornu", "1");
            err.insert("errordesc", "missing userid or sessionid");
            return HttpResponse::json(200, &JsonValue::Object(err));
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
                let mut err = JsonObject::new();
                err.insert("errornu", "1");
                err.insert("errordesc", "invalid or expired Platform sessionid");
                return HttpResponse::json(401, &JsonValue::Object(err));
            }
        }

        let now = store.now_secs();
        let (pid, new_user_flag, prev_login_time, reg_plosgn) =
            if let Some(acct) = store.find_account_by_userid_mut(user_id) {
                let prev = acct.last_login_time;
                acct.last_login_time = now;
                acct.login_count += 1;
                acct.auth_session_id = session_id.to_string();
                let pid_cl = acct.pid.clone();
                let plosgn = acct.reg_plosgn;
                (pid_cl, 0u64, prev, plosgn)
            } else {
                // Dynamically bind unseen Platform / Win32 Guest `userid` (including `win32_guest_0001`).
                let pid_cl = derive_pid_from_userid(user_id);
                let acct = AccountRecord {
                    user_id: user_id.to_string(),
                    pid: pid_cl.clone(),
                    username: format!("acct_{}", user_id),
                    password_md5: String::new(),
                    is_guest: user_id.starts_with("win32_guest"),
                    device_id: if uuid.is_empty() {
                        None
                    } else {
                        Some(uuid.to_string())
                    },
                    auth_session_id: session_id.to_string(),
                    created_at: now,
                    last_login_time: now,
                    login_count: 1,
                    reg_plosgn: self.config.default_plosgn,
                };
                store.insert_account(acct);
                (pid_cl, 1u64, now, self.config.default_plosgn)
            };

        store
            .auth_sessions
            .insert(session_id.to_string(), user_id.to_string());
        let cookie_sess = self.issue_cookie_session(&mut store, &pid, user_id, uuid, plname);

        // Exact schema matching `live_capture_filtered.log:57` + `BtPlatformWin32.cpp:1242-1288`:
        // `{"errornu":"0","uid":"636df6782106606d","errordesc":"OK(1-0)","newuser":0,"lastLoginTime":1795455112,"regPlosgn":2,"env":"online"}`
        let mut obj = JsonObject::new();
        obj.insert("errornu", "0");
        obj.insert("uid", pid.as_str());
        obj.insert("pid", pid.as_str());
        obj.insert("errordesc", "OK(1-0)");
        obj.insert("newuser", new_user_flag);
        obj.insert("lastLoginTime", prev_login_time);
        obj.insert("regPlosgn", reg_plosgn);
        obj.insert("env", "online");

        let mut resp = HttpResponse::json(200, &JsonValue::Object(obj));
        self.attach_session_cookies(&mut resp, &cookie_sess.session_id);
        resp
    }

    // -----------------------------------------------------------------------
    // 8. /phone/loginrole
    // -----------------------------------------------------------------------

    fn handle_loginrole(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
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

        let mut obj = JsonObject::new();
        obj.insert("root", JsonValue::Object(root));
        obj.insert("errornu", "0");
        obj.insert("errordesc", "OK(1-0)");
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    // -----------------------------------------------------------------------
    // 9. /phone/gethash (Strict Cookie Session Binding + Game Ticket Generation)
    // -----------------------------------------------------------------------

    fn handle_gethash(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }

        let pid = req.query.get("pid").map(|s| s.trim()).unwrap_or("");
        let uuid = req.query.get("uuid").map(|s| s.trim()).unwrap_or("");
        let plname = req
            .query
            .get("pl")
            .map(|s| s.trim())
            .unwrap_or("android");
        let group_id = req
            .query
            .get("group_id")
            .or_else(|| req.query.get("serverID"))
            .map(|s| s.trim())
            .unwrap_or("");

        if pid.is_empty() {
            let mut err = JsonObject::new();
            err.insert("errornu", "103");
            err.insert("err_code", 103);
            err.insert("errordesc", "no user");
            err.insert("err_desc", "no user");
            return HttpResponse::json(200, &JsonValue::Object(err));
        }

        let Ok(mut store) = self.store.write() else {
            return HttpResponse::text(500, "store lock poisoned");
        };
        let now = store.now_secs();

        // Check `PHPSESSID` or `GSESS_ID` cookie binding (`SDKUtil.lua:34,504` + `LoginProxy.lua:645`).
        let cookie_token = req
            .cookie(COOKIE_PHPSESSID)
            .or_else(|| req.cookie(COOKIE_GAME_SESSION));

        let mut bound_qid: Option<String> = None;
        if self.config.strict_session {
            let session_ok = match cookie_token {
                Some(tok) => match store.cookie_sessions.get(tok) {
                    Some(sess) if !sess.is_expired(now) && sess.pid == pid => {
                        bound_qid = Some(sess.user_id.clone());
                        true
                    }
                    _ => false,
                },
                None => false,
            };
            if !session_ok {
                // Must set BOTH `errornu = "102"` (for `SDKUtil.lua` `ERROR_PASS["102"]`)
                // AND `err_code = 102` (for `LoginProxy.lua` `if jsonInfo.err_code == 102`).
                let mut err = JsonObject::new();
                err.insert("errornu", "102");
                err.insert("err_code", 102);
                err.insert("errordesc", "session time out");
                err.insert("err_desc", "session time out");
                return HttpResponse::json(200, &JsonValue::Object(err));
            }
        } else if let Some(tok) = cookie_token {
            if let Some(sess) = store.cookie_sessions.get(tok) {
                if sess.pid == pid {
                    bound_qid = Some(sess.user_id.clone());
                }
            }
        }

        // Look up target server by `group_id` (`SDKUtil.lua:503`: 101 = `no group msg`).
        let Some(server) = store
            .servers
            .iter()
            .find(|s| s.groupid == group_id || s.group == group_id)
            .cloned()
        else {
            let mut err = JsonObject::new();
            err.insert("errornu", "101");
            err.insert("err_code", 101);
            err.insert("errordesc", "no group msg");
            err.insert("err_desc", "no group msg");
            return HttpResponse::json(200, &JsonValue::Object(err));
        };

        // Check server maintenance status (`SDKUtil.lua:506` & `LoginProxy.lua:648`: 104 = `maintain is open`).
        if server.status == ServerStatus::Maintenance as u8 {
            let mut err = JsonObject::new();
            err.insert("errornu", "104");
            err.insert("err_code", 104);
            err.insert("errordesc", "maintain is open");
            err.insert("err_desc", "maintain is open");
            err.insert("notice", server.maintenance_notice.as_str());
            return HttpResponse::json(200, &JsonValue::Object(err));
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

        let ticket = IssuedGameHash {
            hash: hash.clone(),
            pid: pid.to_string(),
            qid: qid.clone(),
            uuid: uuid.to_string(),
            group_id: server.groupid.clone(),
            server_id: server.group.clone(),
            timestamp: now,
            expires_at: now + 600,
        };
        store.issued_hashes.insert(hash.clone(), ticket);

        let feign_role_id = derive_feign_role_id(pid, &server.groupid);

        // Exact 21-field JSON schema matching `live_capture_filtered.log:76` verbatim:
        let mut obj = JsonObject::with_capacity(21);
        obj.insert("AMF", 3);
        obj.insert("b_fightLog", 1);
        obj.insert("canCreateUser", 1);
        obj.insert("canPay", 1);
        obj.insert("errordesc", "OK");
        obj.insert("errornu", "0");
        obj.insert("feignRoleId", feign_role_id);
        obj.insert("host", server.host);
        obj.insert("mergeServerTime", server.merge_server_time);
        obj.insert("offset", server.offset);
        obj.insert("openDateTime", server.open_date_time_str);
        obj.insert("pid", pid);
        obj.insert("plname", plname);
        obj.insert("plosgn", plosgn);
        obj.insert("port", server.port);
        obj.insert("ptype", 0);
        obj.insert("qid", qid);
        obj.insert("serverID", server.group);
        obj.insert("timestamp", now);
        obj.insert("uuid", uuid);
        obj.insert("hash", hash);

        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    // -----------------------------------------------------------------------
    // 10. Auxiliary Universal Web Endpoints in `BtPlatformWin32.cpp` & `SDKUtil.lua`
    // -----------------------------------------------------------------------

    fn handle_platform_user_info(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
        let mut data = JsonObject::new();
        data.insert("idcardStatus", 1);
        data.insert("isForceRealName", 0);
        data.insert("is_fastreg", 0);
        data.insert("isAdult", 1);
        data.insert("isBindPhone", 1);
        data.insert("isConcern", 0);
        data.insert("isFastUser", 0);

        let mut obj = JsonObject::new();
        obj.insert("errornu", "0");
        obj.insert("errordesc", "success");
        obj.insert("data", JsonValue::Object(data));
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    fn handle_agree_version(&self, req: &HttpRequest) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
        let mut data = JsonObject::new();
        data.insert("status", 0);
        data.insert("version", "1.0");
        data.insert("newUserAgree", "");
        data.insert("newUserPrivacy", "");
        data.insert("userChildPrivacy", "");

        let mut obj = JsonObject::new();
        obj.insert("errornu", "0");
        obj.insert("errordesc", "OK");
        obj.insert("data", JsonValue::Object(data));
        HttpResponse::json(200, &JsonValue::Object(obj))
    }

    fn handle_generic_ok(&self, req: &HttpRequest, extra: BTreeMap<&str, JsonValue>) -> HttpResponse {
        if let Err(resp) = self.check_mapi_signature(req) {
            return resp;
        }
        let mut obj = JsonObject::new();
        obj.insert("errornu", "0");
        obj.insert("errordesc", "OK");
        for (k, v) in extra {
            obj.insert(k, v);
        }
        HttpResponse::json(200, &JsonValue::Object(obj))
    }
}

impl HttpHandler for MapiService {
    fn handle(&self, req: &HttpRequest) -> HttpResponse {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        if !matches!(req.method, HttpMethod::Get | HttpMethod::Post | HttpMethod::Head) {
            let mut err = JsonObject::new();
            err.insert("errornu", "1");
            err.insert("errordesc", "method not allowed");
            return HttpResponse::json(405, &JsonValue::Object(err));
        }

        match req.path.as_str() {
            "/" | "/admin/status" | "/health" => self.handle_admin_status(),
            "/admin/verify_hash" => self.handle_admin_verify_hash(req),

            // Core GServer MAPI endpoints (`/phone/*`)
            "/phone/applereview" => self.handle_applereview(req),
            "/phone/getversion" => self.handle_getversion(req),
            "/version.lua" | "/scripts/version.lua" => self.handle_version_lua(),
            "/phone/serverlist" => self.handle_serverlist(req),
            "/phone/notice" => self.handle_notice(req, false),
            "/phone/supernotice" => self.handle_notice(req, true),
            "/phone/login" => self.handle_phone_login(req),
            "/phone/loginrole" => self.handle_loginrole(req),
            "/phone/gethash" => self.handle_gethash(req),

            // Platform Account / Win32 Guest / Custom Registration & Login endpoints
            "/api/login/auth/account" | "/api/login/login" => self.handle_account_auth(req),
            "/api/login/createUser/guest" | "/phone/guest" => self.handle_guest_auth(req),
            "/api/login/register" | "/phone/register" => self.handle_register(req),

            // Auxiliary SDK endpoints in `BtPlatformWin32.cpp` & `SDKUtil.lua`
            "/phone/userinfo" => {
                let mut extra = BTreeMap::new();
                extra.insert("data", JsonValue::object());
                self.handle_generic_ok(req, extra)
            }
            "/phone/platform/getPlatformUserInfo" => self.handle_platform_user_info(req),
            "/phone/helper/getagreeversion" => self.handle_agree_version(req),
            "/phone/addiction/getaddiction" | "/phone/addiction/clear" => {
                let mut extra = BTreeMap::new();
                extra.insert("limit_time", JsonValue::from(0));
                self.handle_generic_ok(req, extra)
            }
            "/phone/innerbrowse" | "/phone/question" | "/phone/getforcefix" => {
                let mut extra = BTreeMap::new();
                extra.insert("data", JsonValue::object());
                self.handle_generic_ok(req, extra)
            }

            _ => {
                let mut err = JsonObject::new();
                err.insert("errornu", "1");
                err.insert("ret", 404);
                err.insert("errordesc", format!("unknown endpoint: {}", req.path));
                HttpResponse::json(404, &JsonValue::Object(err))
            }
        }
    }
}
