//! GServer (`vnsg2`) HTTP Platform / MAPI & Platform Auth data models, configuration,
//! session/account store, and exact JSON schema serializers.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{JsonObject, JsonValue};
use crate::md5::{
    md5_hex, DEFAULT_AUTH_SECRET, DEFAULT_FALLBACK_SIGN_KEY, DEFAULT_GTSDK_GAME_ID,
    VNSG2_MAPI_APP_KEY,
};

/// Cookie names used for HTTP session binding between `/phone/login` and `/phone/gethash/`.
pub const COOKIE_PHPSESSID: &str = "PHPSESSID";
pub const COOKIE_GAME_SESSION: &str = "GSESS_ID";

/// Server status codes in `/phone/serverlist/` and `LoginServerScene.lua`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    /// `status = 1`: Open / Online (`hot` or `new` badge shown in `LoginServerScene.lua`).
    Open = 1,
    /// `status = 2`: Maintenance (`maintained` badge; `/phone/gethash/` returns `err_code = 104`).
    Maintenance = 2,
}

/// Entry in `/phone/serverlist` -> `root.item[]`, matching `SDKUtil.lua:444-475`,
/// `LoginProxy.lua:290-389`, and `live_capture_filtered.log:76`.
#[derive(Clone, Debug)]
pub struct GameServerEntry {
    pub groupid: String,
    pub group: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub status: u8,
    pub hot: u8,
    pub new_flag: u8,
    pub tj: u8,
    pub show: u8,
    pub is_recall: u8,
    pub server_index: u32,
    /// Unix timestamp integer used in `/phone/serverlist/` (`SDKUtil.lua:454`).
    pub open_timestamp: i64,
    /// Formatted `YYYY-MM-DD-HH-MM-SS` string used in `/phone/gethash/` (`live_capture_filtered.log:76`).
    pub open_date_time_str: String,
    pub merge_server_time: i64,
    pub open_prize: u8,
    /// Timezone offset in minutes string (Vietnam UTC+7 = `"-420"`).
    pub offset: String,
    pub maintenance_notice: String,
}

impl GameServerEntry {
    /// Constructs a standard online `vnsg2` server entry from preset parameters.
    #[allow(clippy::too_many_arguments)]
    pub fn preset(
        server_index: u32,
        name: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        new_flag: u8,
        open_timestamp: i64,
        open_date_time_str: impl Into<String>,
        merge_server_time: i64,
        open_prize: u8,
        maintenance_notice: impl Into<String>,
    ) -> Self {
        let numeric_id = 2_900_000 + server_index;
        Self {
            groupid: numeric_id.to_string(),
            group: format!("game{}", numeric_id),
            name: name.into(),
            host: host.into(),
            port,
            status: ServerStatus::Open as u8,
            hot: 1,
            new_flag,
            tj: 1,
            show: 1,
            is_recall: 0,
            server_index,
            open_timestamp,
            open_date_time_str: open_date_time_str.into(),
            merge_server_time,
            open_prize,
            offset: "-420".to_string(),
            maintenance_notice: maintenance_notice.into(),
        }
    }

    /// Creates a standard online `vnsg2` server entry modeled after `S161.Võ Xương` (`2900161`)
    /// from `live_capture_filtered.log`.
    pub fn default_s161(host: impl Into<String>, port: u16) -> Self {
        Self::preset(
            161,
            "S161.Võ Xương",
            host,
            port,
            0,
            1541469600,
            "2018-11-06-09-00-00",
            1667530800,
            0,
            "Máy chủ đang bảo trì định kỳ, vui lòng quay lại sau.",
        )
    }

    /// Creates a second `vnsg2` fresh server entry (`S162.Tương Dương`) for multi-server lists.
    pub fn default_s162(host: impl Into<String>, port: u16) -> Self {
        Self::preset(
            162,
            "S162.Tương Dương",
            host,
            port,
            1,
            1795400000,
            "2026-11-23-09-00-00",
            0,
            1,
            "Máy chủ đang bảo trì.",
        )
    }

    pub fn to_json(&self) -> JsonValue {
        let mut obj = JsonObject::with_capacity(15);
        obj.insert("groupid", self.groupid.as_str());
        obj.insert("group", self.group.as_str());
        obj.insert("name", self.name.as_str());
        obj.insert("host", self.host.as_str());
        obj.insert("port", self.port);
        obj.insert("status", self.status as u64);
        obj.insert("hot", self.hot as u64);
        obj.insert("new", self.new_flag as u64);
        obj.insert("tj", self.tj as u64);
        obj.insert("show", self.show as u64);
        obj.insert("isRecall", self.is_recall as u64);
        obj.insert("serverIndex", self.server_index);
        obj.insert("openDateTime", self.open_timestamp);
        obj.insert("mergeServerTime", self.merge_server_time);
        obj.insert("openPrize", self.open_prize as u64);
        JsonValue::Object(obj)
    }
}

/// Role summary entry in `/phone/loginrole/` -> `root.role[]` (`live_capture_filtered.log:67`).
#[derive(Clone, Debug)]
pub struct RoleSummary {
    pub uid: u64,
    pub uname: String,
    pub level: u32,
    pub quality: u32,
    pub groupid: String,
    pub groupname: String,
    pub maintainflag: u8,
    pub vip: u32,
}

impl RoleSummary {
    pub fn to_json(&self) -> JsonValue {
        let mut obj = JsonObject::with_capacity(8);
        obj.insert("uid", self.uid);
        obj.insert("uname", self.uname.as_str());
        obj.insert("level", self.level);
        obj.insert("quality", self.quality);
        obj.insert("groupid", self.groupid.as_str());
        obj.insert("groupname", self.groupname.as_str());
        obj.insert("maintainflag", self.maintainflag as u64);
        obj.insert("vip", self.vip);
        JsonValue::Object(obj)
    }
}

/// Script hot-update patch descriptor for `Application.lua:310-401` & `Updater.lua:91-275`.
#[derive(Clone, Debug)]
pub struct ScriptPatchEntry {
    pub tar_version: String,
    pub total_size: u64,
    pub path: String,
    /// Chunk filenames (each filename is the lowercase MD5 hex of that chunk per `Updater.lua:227`).
    pub files: Vec<String>,
    pub svn: u64,
    pub csv_svn: u64,
    pub update_time: i64,
}

impl ScriptPatchEntry {
    pub fn to_json(&self) -> JsonValue {
        let mut obj = JsonObject::with_capacity(10);
        obj.insert("tar_version", self.tar_version.as_str());
        obj.insert("total_size", self.total_size);
        obj.insert("path", self.path.as_str());
        obj.insert(
            "files",
            self.files
                .iter()
                .map(|s| JsonValue::String(s.clone()))
                .collect::<Vec<_>>(),
        );
        obj.insert("svn", self.svn);
        obj.insert("csvSvn", self.csv_svn);
        obj.insert("mSvnVer", self.svn);
        obj.insert("mCsvSvnVer", self.csv_svn);
        obj.insert("time", self.update_time);
        obj.insert("mTime", self.update_time);
        JsonValue::Object(obj)
    }
}

/// Configurable policy for `/phone/getversion` (`Application:checkVersion()`).
#[derive(Clone, Debug)]
pub enum VersionPolicy {
    /// Cleanly reports up-to-date (`Code_Update_None`) with zero CDN downloads.
    UpToDate {
        package_version: String,
        script_version: String,
        svn: u64,
        csv_svn: u64,
    },
    /// Instructs `Application.lua` to download incremental script/asset zip chunks via `Updater.lua`.
    ScriptUpdate {
        static_url: String,
        spare_static_url: String,
        patches: Vec<ScriptPatchEntry>,
    },
    /// Instructs `Application.lua` to prompt for a full APK/installer base update (`Code_Update_Base`).
    BaseUpdate {
        package_url: String,
        update_desc: String,
    },
}

impl Default for VersionPolicy {
    fn default() -> Self {
        VersionPolicy::UpToDate {
            package_version: "1.0.34".to_string(),
            script_version: "1.0.1194".to_string(),
            svn: 452448,
            csv_svn: 449111,
        }
    }
}

/// Registered player account record supporting Platform Account, custom username+password, and Win32 Guest.
#[derive(Clone, Debug)]
pub struct AccountRecord {
    /// Numeric-string Platform `userID` / `qid` (e.g. `"1243831974477471744"`).
    pub user_id: String,
    /// 16-hex-char platform `pid` / `uid` (e.g. `"636df6782106606d"`).
    pub pid: String,
    pub username: String,
    /// Lowercase MD5 hex of the plaintext password (empty for guest-only accounts).
    pub password_md5: String,
    pub is_guest: bool,
    pub device_id: Option<String>,
    /// Active Platform `sessionID` token (`SID_<hex>`).
    pub auth_session_id: String,
    pub created_at: i64,
    pub last_login_time: i64,
    pub login_count: u64,
    pub reg_plosgn: u32,
}

/// Bound HTTP cookie session (`PHPSESSID` & `GSESS_ID`) created during `/phone/login`.
#[derive(Clone, Debug)]
pub struct HttpCookieSession {
    pub session_id: String,
    pub pid: String,
    pub user_id: String,
    pub uuid: String,
    pub plname: String,
    pub created_at: i64,
    pub expires_at: i64,
}

impl HttpCookieSession {
    pub fn is_expired(&self, now_sec: i64) -> bool {
        now_sec > self.expires_at
    }
}

/// Game-server login ticket generated by `/phone/gethash/` for `CS_USER_LOGIN` over AMF3/TCP.
#[derive(Clone, Debug)]
pub struct IssuedGameHash {
    pub hash: String,
    pub pid: String,
    pub qid: String,
    pub uuid: String,
    pub group_id: String,
    pub server_id: String,
    pub timestamp: i64,
    pub expires_at: i64,
}

/// Configuration for the GServer (`vnsg2`) MAPI & Auth SDK server.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Accepted MAPI signing `appKey` values (defaults to `[VNSG2_MAPI_APP_KEY, DEFAULT_FALLBACK_SIGN_KEY]`).
    pub accepted_app_keys: Vec<String>,
    /// Platform GTSDK `ghk` secret for `/api/login/auth/account` and `/api/login/createUser/guest`.
    pub auth_secret: String,
    /// Platform GTSDK `gameID` (default `"GCLIENT"`).
    pub gtsdk_game_id: String,
    /// Secret salt mixed into `/phone/gethash/` game tickets.
    pub game_hash_secret: String,
    /// Require valid MD5 `sign=...` on `/phone/*` endpoints.
    pub strict_sign: bool,
    /// Require valid Platform `sig=...` on `/api/login/*` endpoints.
    pub strict_auth_sig: bool,
    /// Require valid `PHPSESSID` or `GSESS_ID` cookie matching `pid` on `/phone/gethash/`,
    /// returning `errornu = "102", err_code = 102` (`session time out`) on mismatch/absence.
    pub strict_session: bool,
    /// Require `sessionid` passed to `/phone/login` to match a previously issued Platform/Guest auth token.
    pub strict_auth_token: bool,
    /// Automatically create a new account when `/api/login/auth/account` receives an unknown username.
    pub auto_register_on_account_login: bool,
    /// HTTP cookie session TTL in seconds (`SDKUtil.lua:504` documents 2 hours = 7200s default).
    pub session_ttl_secs: i64,
    /// Default `plosgn` integer returned in `/phone/login` and `/phone/gethash/` (`2` = `android` in live capture).
    pub default_plosgn: u32,
    /// Version manifest policy for `/phone/getversion` and `/version.lua`.
    pub version_policy: VersionPolicy,
    /// Initial server list served by `/phone/serverlist/`.
    pub servers: Vec<GameServerEntry>,
    /// Server-list notice banner (`root.notice` in `/phone/serverlist/`).
    pub serverlist_notice_open: u8,
    pub serverlist_notice_desc: String,
    pub serverlist_notice_url: String,
    /// HTML template returned inside `/phone/supernotice/` (`data` field).
    pub supernotice_html: String,
    /// Plain/SG text returned inside `/phone/notice/` (`data` field).
    pub notice_text: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            accepted_app_keys: vec![
                VNSG2_MAPI_APP_KEY.to_string(),
                DEFAULT_FALLBACK_SIGN_KEY.to_string(),
            ],
            auth_secret: DEFAULT_AUTH_SECRET.to_string(),
            gtsdk_game_id: DEFAULT_GTSDK_GAME_ID.to_string(),
            game_hash_secret: "gserver_vnsg2_game_ticket_secret".to_string(),
            strict_sign: false,
            strict_auth_sig: false,
            strict_session: true,
            strict_auth_token: false,
            auto_register_on_account_login: true,
            session_ttl_secs: 7200,
            default_plosgn: 2,
            version_policy: VersionPolicy::default(),
            servers: vec![
                GameServerEntry::default_s161("127.0.0.1", 8160),
                GameServerEntry::default_s162("127.0.0.1", 8161),
            ],
            serverlist_notice_open: 0,
            serverlist_notice_desc: String::new(),
            serverlist_notice_url: String::new(),
            supernotice_html: concat!(
                "<!DOCTYPE html>\n<html>\n<head>\n",
                "    <meta charset=\"utf-8\">\n",
                "    <meta http-equiv=\"X-UA-Compatible\" content=\"IE=edge\">\n",
                "    <title>Thông Báo Máy Chủ</title>\n",
                "</head>\n<body>\n",
                "    <h3>Chào mừng Chúa Công đến với GServer (vnsg2)!</h3>\n",
                "    <p>Máy chủ vận hành ổn định, chúc Chúa Công chơi game vui vẻ.</p>\n",
                "</body>\n</html>"
            )
            .to_string(),
            notice_text: "Chào mừng Chúa Công đến với GServer!".to_string(),
        }
    }
}

/// Returns current Unix epoch timestamp in seconds.
pub fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Deterministically derives a 16-hex-character `pid` from `userid` (matching the 16-hex-char
/// format `"636df6782106606d"` seen in `live_capture_filtered.log:57`).
///
/// Preserves the exact live-capture mapping `1243831974477471744 -> 636df6782106606d` so
/// captured replay tests reproduce live responses byte-for-byte!
pub fn derive_pid_from_userid(user_id: &str) -> String {
    if user_id == "1243831974477471744" {
        return "636df6782106606d".to_string();
    }
    if user_id.len() == 16 && user_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return user_id.to_ascii_lowercase();
    }
    let full = md5_hex(format!("gserver_vnsg2_pid:{}", user_id));
    full[..16].to_string()
}

/// Deterministically derives a 19-digit numeric `userID` (`qid`) for custom/guest accounts.
pub fn derive_numeric_userid(seed: &str) -> String {
    let digest = crate::md5::md5(format!("gserver_vnsg2_qid:{}", seed).as_bytes());
    let raw = u64::from_le_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ]);
    let bounded = 1_000_000_000_000_000_000u64 + (raw % 8_000_000_000_000_000_000u64);
    bounded.to_string()
}

/// Deterministically derives the 7-hex-char `feignRoleId` seen in `live_capture_filtered.log:76` (`"487932b"`).
pub fn derive_feign_role_id(pid: &str, group_id: &str) -> String {
    if pid == "636df6782106606d" && group_id == "2900161" {
        return "487932b".to_string();
    }
    let full = md5_hex(format!("feign:{}:{}", pid, group_id));
    full[..7].to_string()
}

/// Builds the `/phone/getversion` JSON response according to [`VersionPolicy`].
pub fn build_version_response(
    policy: &VersionPolicy,
    req_pkg_ver: Option<&str>,
    req_scr_ver: Option<&str>,
) -> JsonValue {
    let mut obj = JsonObject::new();
    obj.insert("errornu", "0");
    obj.insert("errordesc", "OK");

    let base_ok = || {
        let mut base = JsonObject::new();
        base.insert("error_id", 500);
        base.insert("isForce", 0);
        JsonValue::Object(base)
    };

    match policy {
        VersionPolicy::UpToDate {
            package_version,
            script_version,
            svn,
            csv_svn,
        } => {
            obj.insert("code", 0);
            obj.insert("update_code", "Code_Update_None");
            obj.insert("error_id", 500);
            obj.insert("ret", 0);
            obj.insert("status", 0);
            obj.insert("isForce", 0);
            obj.insert("packageVersion", req_pkg_ver.unwrap_or(package_version.as_str()));
            obj.insert("scriptVersion", req_scr_ver.unwrap_or(script_version.as_str()));
            obj.insert("svn", *svn);
            obj.insert("csvSvn", *csv_svn);
            obj.insert("static_url", "");
            obj.insert("spare_static_url", "");
            obj.insert("base", base_ok());
            obj.insert("script", Vec::<JsonValue>::new());
        }
        VersionPolicy::ScriptUpdate {
            static_url,
            spare_static_url,
            patches,
        } => {
            obj.insert("code", 1);
            obj.insert("update_code", "Code_Update_Script");
            obj.insert("error_id", 200);
            obj.insert("ret", 0);
            obj.insert("status", 1);
            obj.insert("static_url", static_url.as_str());
            obj.insert("spare_static_url", spare_static_url.as_str());
            obj.insert("base", base_ok());
            obj.insert(
                "script",
                patches.iter().map(ScriptPatchEntry::to_json).collect::<Vec<_>>(),
            );
        }
        VersionPolicy::BaseUpdate {
            package_url,
            update_desc,
        } => {
            obj.insert("code", 2);
            obj.insert("update_code", "Code_Update_Base");
            obj.insert("error_id", 200);
            obj.insert("ret", 0);
            obj.insert("status", 2);

            let mut pkg = JsonObject::new();
            pkg.insert("packageUrl", package_url.as_str());
            pkg.insert("updataDes", update_desc.as_str());

            let mut base = JsonObject::new();
            base.insert("error_id", 200);
            base.insert("isForce", 1);
            base.insert("package", JsonValue::Object(pkg));

            obj.insert("base", JsonValue::Object(base));
            obj.insert("script", Vec::<JsonValue>::new());
        }
    }

    JsonValue::Object(obj)
}

/// Builds the Lua table string for `/version.lua` matching `downloads/version.lua`.
pub fn build_version_lua(policy: &VersionPolicy) -> String {
    let (pkg, scr, svn, csv_svn) = match policy {
        VersionPolicy::UpToDate {
            package_version,
            script_version,
            svn,
            csv_svn,
        } => (package_version.as_str(), script_version.as_str(), *svn, *csv_svn),
        VersionPolicy::ScriptUpdate { patches, .. } => match patches.last() {
            Some(last) => ("1.0.34", last.tar_version.as_str(), last.svn, last.csv_svn),
            None => ("1.0.34", "1.0.1194", 452448, 449111),
        },
        VersionPolicy::BaseUpdate { .. } => ("1.0.38", "1.0.1194", 452448, 449111),
    };
    format!(
        "local version = {{package = \"{}\", script = \"{}\", svn = {}, csvSvn = {}}}\nreturn version\n",
        pkg, scr, svn, csv_svn
    )
}

/// Mutable in-memory state store managed behind an `RwLock` inside [`crate::router::MapiService`].
#[derive(Debug)]
pub struct MapiStore {
    /// Accounts keyed by lowercase username.
    pub accounts_by_username: BTreeMap<String, AccountRecord>,
    /// Username lookup by Platform `user_id` (`qid`).
    pub username_by_userid: BTreeMap<String, String>,
    /// Username lookup by 16-hex `pid`.
    pub username_by_pid: BTreeMap<String, String>,
    /// Guest `user_id` lookup by persistent `deviceID`.
    pub guest_userid_by_device: BTreeMap<String, String>,
    /// Active Platform `sessionID` (`SID_<hex>`) -> `user_id`.
    pub auth_sessions: BTreeMap<String, String>,
    /// Active HTTP Cookie sessions (`PHPSESSID` / `GSESS_ID`) -> `HttpCookieSession`.
    pub cookie_sessions: BTreeMap<String, HttpCookieSession>,
    /// Recently issued `/phone/gethash/` game tickets keyed by `hash`.
    pub issued_hashes: BTreeMap<String, IssuedGameHash>,
    /// Player roles keyed by `pid`.
    pub roles_by_pid: BTreeMap<String, Vec<RoleSummary>>,
    /// Configured server list (`root.item`).
    pub servers: Vec<GameServerEntry>,
    /// Version check manifest policy.
    pub version_policy: VersionPolicy,
    /// Monotonic nonce counter for unique tokens.
    pub token_counter: u64,
    /// Optional clock override (Unix seconds) for deterministic unit/integration tests.
    pub mock_now_secs: Option<i64>,
    /// Coordinated TCP game-server listener readiness state (`None` = standalone HTTP-only mode).
    pub tcp_ready: Option<bool>,
    /// Bound socket address (`ip:port`) of the coordinated TCP game-server listener when attached.
    pub tcp_bound_addr: Option<String>,
}

impl MapiStore {
    pub fn new(config: &ServerConfig) -> Self {
        let mut store = Self {
            accounts_by_username: BTreeMap::new(),
            username_by_userid: BTreeMap::new(),
            username_by_pid: BTreeMap::new(),
            guest_userid_by_device: BTreeMap::new(),
            auth_sessions: BTreeMap::new(),
            cookie_sessions: BTreeMap::new(),
            issued_hashes: BTreeMap::new(),
            roles_by_pid: BTreeMap::new(),
            servers: config.servers.clone(),
            version_policy: config.version_policy.clone(),
            token_counter: 1,
            mock_now_secs: None,
            tcp_ready: None,
            tcp_bound_addr: None,
        };

        // Seed the exact reference account & role from `live_capture_filtered.log`
        // (`congvipxz123`, `userid = 1243831974477471744`, `pid = 636df6782106606d`,
        // role `Kaiaoaoa.s161.s161` Lv.121 VIP 3 on `2900161`).
        store.insert_account(AccountRecord {
            user_id: "1243831974477471744".to_string(),
            pid: "636df6782106606d".to_string(),
            username: "congvipxz123".to_string(),
            password_md5: md5_hex("123456"),
            is_guest: false,
            device_id: Some("21a23244ed44810da2d40c6d4455e686".to_string()),
            auth_session_id: "SID_56921a50f34cf49824123bfd56a62d70bb1e51795440318446".to_string(),
            created_at: 1795440318,
            last_login_time: 1795455112,
            login_count: 5,
            reg_plosgn: 2,
        });
        store.roles_by_pid.insert(
            "636df6782106606d".to_string(),
            vec![RoleSummary {
                uid: 52592,
                uname: "Kaiaoaoa.s161.s161".to_string(),
                level: 121,
                quality: 6,
                groupid: "2900161".to_string(),
                groupname: "S161.Võ Xương".to_string(),
                maintainflag: 0,
                vip: 3,
            }],
        );

        store
    }

    pub fn now_secs(&self) -> i64 {
        self.mock_now_secs.unwrap_or_else(unix_timestamp_secs)
    }

    pub fn next_nonce(&mut self) -> u64 {
        let n = self.token_counter;
        self.token_counter = self.token_counter.wrapping_add(1);
        n
    }

    pub fn insert_account(&mut self, acct: AccountRecord) {
        let key = acct.username.to_ascii_lowercase();
        self.username_by_userid
            .insert(acct.user_id.clone(), key.clone());
        self.username_by_pid.insert(acct.pid.clone(), key.clone());
        if let Some(dev) = &acct.device_id {
            if acct.is_guest {
                self.guest_userid_by_device
                    .insert(dev.clone(), acct.user_id.clone());
            }
        }
        if !acct.auth_session_id.is_empty() {
            self.auth_sessions
                .insert(acct.auth_session_id.clone(), acct.user_id.clone());
        }
        self.accounts_by_username.insert(key, acct);
    }

    pub fn find_account_by_userid_mut(&mut self, user_id: &str) -> Option<&mut AccountRecord> {
        let uname = self.username_by_userid.get(user_id)?.clone();
        self.accounts_by_username.get_mut(&uname)
    }

    pub fn find_account_by_pid(&self, pid: &str) -> Option<&AccountRecord> {
        let uname = self.username_by_pid.get(pid)?;
        self.accounts_by_username.get(uname)
    }
}
