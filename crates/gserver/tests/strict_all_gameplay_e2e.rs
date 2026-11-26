#![forbid(unsafe_code)]
//! Strict All-Gameplay End-to-End Integration Suite (`--strict-all`) for `cobalt-relay` (`gserver`).
//!
//! Exercises EVERY gameplay domain when playing GServer (`vnsg2`) under full strict enforcement:
//!   - `strict_sign = true` (every `/phone/*` MAPI call MUST carry valid `VNSG2_MAPI_APP_KEY` MD5 `sign`)
//!   - `strict_auth_sig = true` (every `/api/login/*` call MUST carry valid GTSDK `ghk` MD5 `sig`)
//!   - `strict_auth_token = true` (`/phone/login` `sessionid` MUST match issued Platform `sessionID`)
//!   - `strict_session = true` (`/phone/gethash` without `PHPSESSID`/`GSESS_ID` MUST return `errornu="102"`)
//!   - `verify_tickets = true` (TCP `user.login` MUST present valid `/phone/gethash` ticket)
//!   - `strict_rpc = true` (zero silent `unsupported_method` pass-throughs; unknown RPCs fail hard)

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gserver::bridge::protocol_to_catalog_amf3;
use gserver::{DaemonConfig, RunningServer};
use gserver_http::{
    compute_account_auth_sig, compute_guest_auth_sig, md5_hex, parse_json, sign_mapi_url,
    JsonValue, DEFAULT_AUTH_SECRET, DEFAULT_GTSDK_GAME_ID, VNSG2_MAPI_APP_KEY,
};
use gserver_protocol::{
    amf3, decode_s2c_frame_batch, zlib_decompress, RpcRequest, S2cEnvelope, ServerRxDecoder, Value,
};
use gserver_rpc_catalog::{
    validate_response_for_method, Amf3Value as CatalogAmf3, RpcResponseEnvelope, BOOTSTRAP_SEQUENCE,
};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "cobalt_relay_strict_e2e_{label}_{}_{}",
            std::process::id(),
            id
        ));
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

fn raw_http_get(
    addr: SocketAddr,
    target_path_and_query: &str,
    cookies: &[String],
) -> (u16, Vec<String>, JsonValue) {
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

    let json = parse_json(body.trim()).unwrap_or_else(|e| {
        panic!("expected JSON body from {target_path_and_query}, got error {e}: {body}")
    });
    (status_code, set_cookies, json)
}

fn signed_mapi_target(addr: SocketAddr, path_and_query: &str, ts: i64) -> String {
    let full = sign_mapi_url(
        &format!("http://{addr}{path_and_query}"),
        ts,
        VNSG2_MAPI_APP_KEY,
    );
    full.strip_prefix(&format!("http://{addr}"))
        .unwrap()
        .to_string()
}

struct StrictWireClient {
    stream: TcpStream,
    rx: ServerRxDecoder,
    seq: u64,
    token: String,
    queued_pushes: Vec<S2cEnvelope>,
}

impl StrictWireClient {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("connect TCP game server");
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        Self {
            stream,
            rx: ServerRxDecoder::new(),
            seq: 1,
            token: "0".to_string(),
            queued_pushes: Vec::new(),
        }
    }

    fn call_rpc(&mut self, method: &str, args: Vec<Value>, compress_c2s: bool) -> S2cEnvelope {
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
                let mut matched: Option<S2cEnvelope> = None;
                for env in batch.items {
                    if let Some(ref tok) = env.token {
                        if !tok.is_empty() && tok != "0" {
                            self.token = tok.clone();
                        }
                    }
                    if matched.is_none() && env.callback_name.as_deref() == Some(cb_name.as_str()) {
                        matched = Some(env);
                    } else {
                        self.queued_pushes.push(env);
                    }
                }
                if let Some(found) = matched {
                    return found;
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

    fn call_ping(&mut self, method: &str) -> S2cEnvelope {
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

    fn drain_pushes(&mut self) -> Vec<S2cEnvelope> {
        while let Some(frame) = self
            .rx
            .next_frame()
            .expect("decode buffered S2C push frame")
        {
            let batch = decode_s2c_frame_batch(&frame).expect("decode buffered push batch");
            self.queued_pushes.extend(batch.items);
        }
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
        let _ = self
            .stream
            .set_read_timeout(Some(Duration::from_secs(2)));
        std::mem::take(&mut self.queued_pushes)
    }
}

fn strict_auth_guest(http_addr: SocketAddr, device_id: &str) -> (String, String, u16) {
    let ts = "1795501000";
    let sig = compute_guest_auth_sig(
        DEFAULT_AUTH_SECRET,
        DEFAULT_GTSDK_GAME_ID,
        device_id,
        ts,
    );
    let (_, _, guest_json) = raw_http_get(
        http_addr,
        &format!(
            "/api/login/createUser/guest?deviceID={device_id}&gameID={DEFAULT_GTSDK_GAME_ID}&timestamp={ts}&sig={sig}"
        ),
        &[],
    );
    assert_eq!(
        guest_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    let user_id = guest_json
        .get("userID")
        .and_then(JsonValue::as_str)
        .unwrap();
    let session_id = guest_json
        .get("sessionID")
        .and_then(JsonValue::as_str)
        .unwrap();

    let login_target = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/login/?pl=android_vnsg2&gn=vnsg2&os=win32&userid={user_id}&sessionid={session_id}"
        ),
        1_795_501_000,
    );
    let (_, cookies, login_json) = raw_http_get(http_addr, &login_target, &[]);
    assert_eq!(
        login_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    let pid = login_json
        .get("pid")
        .and_then(JsonValue::as_scalar_string)
        .unwrap();

    let gethash_target = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/gethash/?pl=android_vnsg2&gn=vnsg2&pid={pid}&group_id=2900161&uuid=uuid_{device_id}"
        ),
        1_795_501_001,
    );
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
    let port = hash_json
        .get("port")
        .and_then(JsonValue::as_u64)
        .unwrap() as u16;
    (pid, hash, port)
}

fn strict_auth_account(
    http_addr: SocketAddr,
    username: &str,
    plaintext_pw: &str,
) -> (String, String, u16) {
    let ts = "1795501010";
    let pw_md5 = md5_hex(plaintext_pw);
    let sig = compute_account_auth_sig(
        DEFAULT_AUTH_SECRET,
        DEFAULT_GTSDK_GAME_ID,
        username,
        &pw_md5,
        ts,
    );
    let (_, _, acct_json) = raw_http_get(
        http_addr,
        &format!(
            "/api/login/auth/account?u={username}&p={pw_md5}&gameID={DEFAULT_GTSDK_GAME_ID}&timestamp={ts}&sig={sig}"
        ),
        &[],
    );
    assert_eq!(
        acct_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    let user_id = acct_json.get("userID").and_then(JsonValue::as_str).unwrap();
    let session_id = acct_json
        .get("sessionID")
        .and_then(JsonValue::as_str)
        .unwrap();

    let login_target = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/login/?pl=android_vnsg2&gn=vnsg2&os=win32&userid={user_id}&sessionid={session_id}"
        ),
        1_795_501_010,
    );
    let (_, cookies, login_json) = raw_http_get(http_addr, &login_target, &[]);
    let pid = login_json
        .get("pid")
        .and_then(JsonValue::as_scalar_string)
        .unwrap();

    let gethash_target = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/gethash/?pl=android_vnsg2&gn=vnsg2&pid={pid}&group_id=2900161&uuid=uuid_{username}"
        ),
        1_795_501_011,
    );
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
    let port = hash_json
        .get("port")
        .and_then(JsonValue::as_u64)
        .unwrap() as u16;
    (pid, hash, port)
}

fn assert_catalog_valid(method: &str, args: &[Value], env: &S2cEnvelope) {
    let cat_args: Vec<CatalogAmf3> = args.iter().map(protocol_to_catalog_amf3).collect();
    let cat_resp = RpcResponseEnvelope {
        err: env.err.clone(),
        callback_name: env.callback_name.clone().unwrap_or_default(),
        ret: protocol_to_catalog_amf3(&env.ret),
        time: env.time.unwrap_or(1_795_501_000),
        token: env.token.clone().unwrap_or_default(),
    };
    if let Err(violations) = validate_response_for_method(method, &cat_args, &cat_resp, true) {
        panic!("Lua schema violation(s) on {method}: {violations:#?}");
    }
}

fn parse_copper(user_ret: &Value) -> u64 {
    user_ret
        .get_field("copper")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| v.as_i64().map(|n| n as u64))
        })
        .expect("numeric copper in user.getUser")
}

fn sum_bag_template_count(bag_ret: &Value, sub_bag: &str, target_tid: i64) -> i64 {
    let Some(Value::Object(map)) = bag_ret.get_field(sub_bag) else {
        return 0;
    };
    let mut total = 0i64;
    for (_, slot) in map.iter() {
        let tid = slot.get_field("item_template_id").and_then(Value::as_i64);
        let num = slot.get_field("item_num").and_then(Value::as_i64);
        if tid == Some(target_tid) {
            total += num.unwrap_or(0);
        }
    }
    total
}

#[test]
fn strict_all_12_domains_gameplay_security_and_wal_recovery() {
    let tmp = TempDirGuard::new("all12");
    let cfg = DaemonConfig::strict_all(tmp.path.clone());
    let server = RunningServer::start(cfg.clone()).expect("start strict RunningServer");
    let http_addr = server.http_addr;
    let tcp_addr = server.tcp_addr;

    // =========================================================================
    // DOMAIN 1: Strict HTTP MAPI Sign, GTSDK Sig, Token, Cookie `102` & Ticket
    // =========================================================================
    // 1a. Unsigned `/phone/getversion` MUST fail with `err_code = 403`
    let (_, _, unsigned_ver) = raw_http_get(
        http_addr,
        "/phone/getversion?pl=android_vnsg2&gn=vnsg2",
        &[],
    );
    assert_eq!(
        unsigned_ver.get("err_code").and_then(JsonValue::as_i64),
        Some(403),
        "strict_sign must reject unsigned /phone/* queries"
    );

    // 1b. Signed `/phone/getversion`, `/phone/applereview`, `/phone/serverlist`, `/phone/notice`, `/phone/supernotice`
    for path in [
        "/phone/getversion?pl=android_vnsg2&gn=vnsg2&os=win32",
        "/phone/applereview/?pl=android_vnsg2&gn=vnsg2",
        "/phone/serverlist/?pl=android_vnsg2&gn=vnsg2",
        "/phone/notice/?pl=android_vnsg2&gn=vnsg2",
        "/phone/supernotice/?pl=android_vnsg2&gn=vnsg2",
    ] {
        let target = signed_mapi_target(http_addr, path, 1_795_501_000);
        let (st, _, body) = raw_http_get(http_addr, &target, &[]);
        assert_eq!(st, 200, "signed {path} must return HTTP 200");
        assert_ne!(
            body.get("err_code").and_then(JsonValue::as_i64),
            Some(403),
            "signed {path} must pass signature check"
        );
    }

    // 1c. Unsigned `/api/login/createUser/guest` MUST fail under `strict_auth_sig`
    let (_, _, unsigned_guest) = raw_http_get(
        http_addr,
        "/api/login/createUser/guest?deviceID=strict_dev_a&gameID=GCLIENT",
        &[],
    );
    assert_eq!(
        unsigned_guest.get("returnCode").and_then(JsonValue::as_str),
        Some("-3"),
        "strict_auth_sig must reject unsigned guest auth"
    );

    // 1d. `/phone/login` with forged `sessionid` MUST fail under `strict_auth_token`
    let forged_login_target = signed_mapi_target(
        http_addr,
        "/phone/login/?pl=android_vnsg2&gn=vnsg2&os=win32&userid=999999&sessionid=SID_forged",
        1_795_501_002,
    );
    let (_, _, forged_login_json) = raw_http_get(http_addr, &forged_login_target, &[]);
    assert_ne!(
        forged_login_json.get("errornu").and_then(JsonValue::as_str),
        Some("0"),
        "strict_auth_token must reject unissued sessionID"
    );

    // 1e. Authenticate Client A (Guest) & Client B (Account) strictly + verify cookie `102`
    let (pid_a, hash_a, port_a) = strict_auth_guest(http_addr, "strict_dev_alpha");
    assert_eq!(port_a, tcp_addr.port());
    let gethash_no_cookie = signed_mapi_target(
        http_addr,
        &format!(
            "/phone/gethash/?pl=android_vnsg2&gn=vnsg2&pid={pid_a}&group_id=2900161&uuid=uuid_x"
        ),
        1_795_501_003,
    );
    let (_, _, err102) = raw_http_get(http_addr, &gethash_no_cookie, &[]);
    assert_eq!(
        err102.get("errornu").and_then(JsonValue::as_str),
        Some("102")
    );

    let (pid_b, hash_b, _) = strict_auth_account(http_addr, "quanvu_strict", "TamQuoc@2026");

    // 1f. Pre-login TCP negative guards
    let mut client_a = StrictWireClient::connect(tcp_addr);
    let prelogin_get = client_a.call_rpc("user.getUser", vec![], false);
    assert_eq!(prelogin_get.err, "user_not_logged_in");

    let bad_hash_login = client_a.call_rpc(
        "user.login",
        vec![Value::map([
            ("pid", Value::String(pid_a.clone())),
            ("qid", Value::String(pid_a.clone())),
            ("hash", Value::String("bad_ticket_000".into())),
            ("serverID", Value::String("game2900161".into())),
        ])],
        false,
    );
    assert_eq!(bad_hash_login.err, "invalid_login_hash");

    // Heartbeat `ping` & `user.ping`
    assert_eq!(client_a.call_ping("ping").err, "ping");
    assert_eq!(client_a.call_ping("user.ping").err, "ping");

    // =========================================================================
    // DOMAIN 2: 28-Step MainTown Bootstrap + Strict Unknown RPC Rejection
    // =========================================================================
    let login_args = vec![
        Value::map([
            ("AMF", Value::Int(3)),
            ("pid", Value::String(pid_a.clone())),
            ("qid", Value::String(pid_a.clone())),
            ("hash", Value::String(hash_a)),
            ("serverID", Value::String("game2900161".into())),
        ]),
        Value::String("uuid_strict_alpha".into()),
        Value::String("1.0.1194".into()),
    ];
    let env_login = client_a.call_rpc("user.login", login_args.clone(), true);
    assert_catalog_valid("user.login", &login_args, &env_login);

    // Invalid protagonist utid=999 must fail strictly
    let bad_utid = client_a.call_rpc(
        "user.createUser",
        vec![Value::Int(999), Value::String("BadUtid".into())],
        false,
    );
    assert_eq!(bad_utid.err, "invalid_utid");

    let create_a_args = vec![Value::Int(1), Value::String("TriệuTửLong".into())];
    let env_create_a = client_a.call_rpc("user.createUser", create_a_args.clone(), true);
    assert_catalog_valid("user.createUser", &create_a_args, &env_create_a);

    let env_b4_a = client_a.call_rpc("user.getUserBeforLogin", vec![], false);
    let uid_a = env_b4_a
        .ret
        .get_field("uid")
        .and_then(Value::as_i64)
        .unwrap() as u64;
    let env_ul_a = client_a.call_rpc("user.userLogin", vec![Value::Int(uid_a as i32)], false);
    assert_eq!(env_ul_a.ret.as_str(), Some("ok"));

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
        let resp = client_a.call_rpc(spec.method, args.clone(), spec.step_order % 2 == 0);
        assert_catalog_valid(spec.method, &args, &resp);
    }

    // Strict RPC check: unknown method MUST return `err = "unsupported_method"`!
    let unknown_rpc = client_a.call_rpc("bogus.nonExistentRpc", vec![], false);
    assert_eq!(
        unknown_rpc.err, "unsupported_method",
        "strict_rpc must never swallow unimplemented methods"
    );

    // Bring Client B online on the same server (`QuanVũVIP`)
    let mut client_b = StrictWireClient::connect(tcp_addr);
    client_b.call_rpc(
        "user.login",
        vec![Value::map([
            ("pid", Value::String(pid_b)),
            ("qid", Value::String("quanvu".into())),
            ("hash", Value::String(hash_b)),
            ("serverID", Value::String("game2900161".into())),
        ])],
        false,
    );
    // Negative: duplicate character name on creation
    let dup_create = client_b.call_rpc(
        "user.createUser",
        vec![Value::Int(2), Value::String("TriệuTửLong".into())],
        false,
    );
    assert_eq!(dup_create.err, "uname_used");

    assert_eq!(
        client_b
            .call_rpc(
                "user.createUser",
                vec![Value::Int(2), Value::String("QuanVũVIP".into())],
                false
            )
            .err,
        "ok"
    );
    let uid_b = client_b
        .call_rpc("user.getUserBeforLogin", vec![], false)
        .ret
        .get_field("uid")
        .and_then(Value::as_i64)
        .unwrap() as u64;
    assert_eq!(
        client_b
            .call_rpc("user.userLogin", vec![Value::Int(uid_b as i32)], false)
            .ret
            .as_str(),
        Some("ok")
    );

    // =========================================================================
    // DOMAIN 3: Progression, Rename, Stamina/Energy, Copper, HeroBag & VIP Gift
    // =========================================================================
    let _ = client_a.drain_pushes();
    let user0 = client_a.call_rpc("user.getUser", vec![], false).ret;
    let copper0 = parse_copper(&user0);
    let exec0 = user0.get_field("execution").and_then(Value::as_i64).unwrap();
    let energy0 = user0.get_field("energy").and_then(Value::as_i64).unwrap();

    // Negative rename: empty & colliding with Client B
    assert_eq!(
        client_a
            .call_rpc("user.changeUname", vec![Value::String("   ".into())], false)
            .err,
        "invalid_uname"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "user.changeUname",
                vec![Value::String("QuanVũVIP".into())],
                false
            )
            .err,
        "uname_used"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "user.changeUname",
                vec![Value::String("LongĐởmThần".into())],
                false
            )
            .err,
        "ok"
    );

    assert_eq!(
        client_a
            .call_rpc("user.buyCopper", vec![Value::Int(2)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("user.buyExecution", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("user.addEnergy", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("user.openHeroBag", vec![Value::Int(2)], false)
            .err,
        "ok"
    );

    // VIP gifts: starter VIP is 8 -> buying VIP 1 succeeds, re-buying VIP 1 fails, buying VIP 12 fails!
    assert_eq!(
        client_a
            .call_rpc("user.buyVipGift", vec![Value::Int(12)], false)
            .err,
        "vip_too_low"
    );
    assert_eq!(
        client_a
            .call_rpc("user.buyVipGift", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("user.buyVipGift", vec![Value::Int(1)], false)
            .err,
        "vip_gift_already_bought"
    );
    let vip_gifts = client_a.call_rpc("user.getVipGiftInfo", vec![], false);
    assert_eq!(vip_gifts.err, "ok");

    // Client preference `vaConfig` roundtrip
    assert_eq!(
        client_a
            .call_rpc(
                "user.setVaConfig",
                vec![
                    Value::String("music_mute".into()),
                    Value::String("1".into())
                ],
                false
            )
            .err,
        "ok"
    );
    let va_cfg = client_a.call_rpc("user.getVaConfig", vec![], false);
    assert_eq!(
        va_cfg.ret.get_field("music_mute").and_then(Value::as_str),
        Some("1")
    );

    let pushes_d3 = client_a.drain_pushes();
    assert!(
        pushes_d3
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("user.updateUser")),
        "Domain 3 mutations must emit live user.updateUser pushes"
    );
    let user1 = client_a.call_rpc("user.getUser", vec![], false).ret;
    assert_eq!(user1.get_field("uname").and_then(Value::as_str), Some("LongĐởmThần"));
    assert_eq!(parse_copper(&user1), copper0 + 200_000);
    assert_eq!(
        user1.get_field("execution").and_then(Value::as_i64),
        Some(exec0 + 25)
    );
    assert_eq!(
        user1.get_field("energy").and_then(Value::as_i64),
        Some(energy0 + 10)
    );

    // =========================================================================
    // DOMAIN 4: 42 Sub-Bags, Item Use, Sell, Expand & Fragment Synthesis
    // =========================================================================
    let expand_resp = client_a.call_rpc(
        "bag.openGridByGold",
        vec![Value::Int(5), Value::String("arm".into())],
        false,
    );
    assert_eq!(expand_resp.err, "ok");
    assert_eq!(expand_resp.ret.as_str(), Some("ok"));

    // Inspect bagInfo to verify expanded `gridMaxNum.arm` (200 + 10 = 210) and locate gids for `22102` & `16012`
    let bag_info = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
    assert_eq!(
        bag_info
            .get_field("gridMaxNum")
            .and_then(|gm| gm.get_field("arm"))
            .and_then(Value::as_i64),
        Some(250)
    );
    let item_bag = bag_info.get_field("item").expect("item sub-bag");
    let mut gid_22102 = 0i32;
    let mut gid_16012 = 0i32;
    if let Value::Object(map) = item_bag {
        for (k, v) in map.iter() {
            let tid = v.get_field("item_template_id").and_then(Value::as_i64);
            if tid == Some(22102) {
                gid_22102 = k.parse().unwrap();
            } else if tid == Some(16012) {
                gid_16012 = k.parse().unwrap();
            }
        }
    }
    assert!(gid_22102 > 0 && gid_16012 > 0, "starter items must exist in item bag");

    // Negative item use: bad gid + excessive count
    assert_eq!(
        client_a
            .call_rpc(
                "exchange.useItemInBag",
                vec![Value::Int(39999), Value::Int(1)],
                false
            )
            .err,
        "item_not_found"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "exchange.useItemInBag",
                vec![Value::Int(gid_22102), Value::Int(9999)],
                false
            )
            .err,
        "insufficient_item_count"
    );

    let _ = client_a.drain_pushes();
    let use_pill = client_a.call_rpc(
        "exchange.useItemInBag",
        vec![Value::Int(gid_22102), Value::Int(2)],
        false,
    );
    assert_eq!(use_pill.err, "ok");
    assert_eq!(
        use_pill.ret.get_field("execution").and_then(Value::as_i64),
        Some(50)
    );

    let use_by_tpl = client_a.call_rpc(
        "exchange.useItemInBagByTpl",
        vec![Value::Int(22202), Value::Int(3)],
        false,
    );
    assert_eq!(use_by_tpl.err, "ok");

    let sell_resp = client_a.call_rpc(
        "bag.sellItems",
        vec![Value::dense_array(vec![Value::dense_array(vec![
            Value::Int(gid_16012),
            Value::Int(0),
            Value::Int(5),
        ])])],
        false,
    );
    assert_eq!(sell_resp.err, "ok");

    // Compose hero from `50_101` x 30 soul fragments -> summons `htid = 401`
    let compose_hero = client_a.call_rpc(
        "exchange.composeHero",
        vec![Value::Int(50_101), Value::Int(1)],
        false,
    );
    assert_eq!(compose_hero.err, "ok");
    let composed_hid = compose_hero
        .ret
        .get_field("hid")
        .and_then(Value::as_i64)
        .unwrap() as u64;
    assert!(composed_hid > 10_000_001);

    // Negative: composing again with 0 remaining fragments must fail!
    assert_eq!(
        client_a
            .call_rpc(
                "exchange.composeHero",
                vec![Value::Int(50_101), Value::Int(1)],
                false
            )
            .err,
        "insufficient_item_count"
    );

    // Compose equipment from `3_113_001` x 20 arm fragments
    let compose_arm = client_a.call_rpc(
        "exchange.composeArm",
        vec![Value::Int(3_113_001), Value::Int(1)],
        false,
    );
    assert_eq!(compose_arm.err, "ok");
    assert_eq!(compose_arm.ret.as_str(), Some("ok"));
    let bag_after_arm = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
    let (crafted_arm_gid, _) =
        find_bag_entry_by_tid(&bag_after_arm, "arm", 113_001).expect("crafted 113_001 in arm bag");
    assert!((10_001..=30_000).contains(&crafted_arm_gid));

    let pushes_d4 = client_a.drain_pushes();
    assert!(
        pushes_d4
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("bag.update")),
        "Domain 4 mutations must emit bag.update pushes"
    );
    assert!(
        pushes_d4
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("re.hero.new")),
        "exchange.composeHero must emit re.hero.new push"
    );

    // =========================================================================
    // DOMAIN 5: Hero Level-Up, Skill, Breakthrough, Lock Protection & Evolve
    // =========================================================================
    // Negative: leveling protagonist directly must be rejected!
    assert_eq!(
        client_a
            .call_rpc(
                "hero.heroLevelUp",
                vec![Value::Int(10_000_001), Value::Int(1)],
                false
            )
            .err,
        "protagonist_cannot_level_directly"
    );

    let lv_up = client_a.call_rpc(
        "hero.heroLevelUp",
        vec![Value::Int(composed_hid as i32), Value::Int(5)],
        false,
    );
    assert_eq!(lv_up.err, "ok");
    assert_eq!(lv_up.ret.get_field("level").and_then(Value::as_i64), Some(6));

    let sk_up = client_a.call_rpc(
        "hero.skillLevelUp",
        vec![Value::Int(composed_hid as i32), Value::Int(2001)],
        false,
    );
    assert_eq!(sk_up.err, "ok");

    let surmount = client_a.call_rpc(
        "evolve.surmount",
        vec![Value::Int(composed_hid as i32)],
        false,
    );
    assert_eq!(surmount.err, "ok");
    assert_eq!(
        surmount.ret.get_field("upgrade_level").and_then(Value::as_i64),
        Some(1)
    );

    // Summon an extra fodder hero via GM `/hero 305`, lock it, verify sacrifice fails, unlock & evolve!
    assert_eq!(
        client_a
            .call_rpc(
                "chat.sendWorld",
                vec![Value::Int(1), Value::String("/hero 305".into())],
                false
            )
            .err,
        "ok"
    );
    let all_heroes = client_a.call_rpc("hero.getAllHeroes", vec![], false).ret;
    let mut fodder_hid = 0i32;
    let mut partner_hid = 0i32;
    if let Value::Object(hm) = &all_heroes {
        for (k, hv) in hm.iter() {
            if hv.get_field("htid").and_then(Value::as_i64) == Some(305) {
                fodder_hid = k.parse().unwrap();
            }
        }
    }
    assert!(fodder_hid > 0);

    assert_eq!(
        client_a
            .call_rpc("hero.lock", vec![Value::Int(fodder_hid)], false)
            .err,
        "ok"
    );
    let locked_ev = client_a.call_rpc(
        "evolve.doEvolve",
        vec![
            Value::Int(composed_hid as i32),
            Value::dense_array(vec![Value::Int(fodder_hid)]),
        ],
        false,
    );
    assert_eq!(locked_ev.err, "locked_hero_cannot_sacrifice");

    assert_eq!(
        client_a
            .call_rpc("hero.unLock", vec![Value::Int(fodder_hid)], false)
            .err,
        "ok"
    );
    // Negative: GM /hero auto-deployed fodder_hid into squad slot 4, so sacrificing while deployed MUST fail!
    let dep_ev = client_a.call_rpc(
        "evolve.doEvolve",
        vec![
            Value::Int(composed_hid as i32),
            Value::dense_array(vec![Value::Int(fodder_hid)]),
        ],
        false,
    );
    assert_eq!(dep_ev.err, "deployed_hero_cannot_sacrifice");

    // Unset fodder_hid from squad slot 4, then sacrifice succeeds!
    assert_eq!(
        client_a
            .call_rpc("formation.unsetHeroFromPosition", vec![Value::Int(4)], false)
            .err,
        "ok"
    );
    let ok_ev = client_a.call_rpc(
        "evolve.doEvolve",
        vec![
            Value::Int(composed_hid as i32),
            Value::dense_array(vec![Value::Int(fodder_hid)]),
        ],
        false,
    );
    assert_eq!(ok_ev.err, "ok");
    assert_eq!(
        ok_ev.ret.get_field("evolve_level").and_then(Value::as_i64),
        Some(1)
    );

    // =========================================================================
    // DOMAIN 6: 6-Slot Squad, 3x3 Battle Grid, Partner, 9-Slot Arming & Forge
    // =========================================================================
    // Negative: removing protagonist from slot 1 MUST fail!
    assert_eq!(
        client_a
            .call_rpc("formation.unsetHeroFromPosition", vec![Value::Int(1)], false)
            .err,
        "protagonist_cannot_be_removed"
    );

    // Deploy `composed_hid` into squad slot 4 (unlocked because starter level is 10; let's level to 20 via GM first)
    assert_eq!(
        client_a
            .call_rpc(
                "chat.sendWorld",
                vec![Value::Int(1), Value::String("/level 35".into())],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "formation.setHeroToPosition",
                vec![Value::Int(composed_hid as i32), Value::Int(4)],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("formation.transHero", vec![Value::Int(2), Value::Int(4)], false)
            .err,
        "ok"
    );

    // Rearrange 3x3 battle grid (`1..=9`) with the exact 4 deployed heroes (`slot 1..4`)
    let squad_arr = client_a.call_rpc("formation.getHeroes", vec![], false).ret;
    let Value::Array { dense: sq, .. } = squad_arr else {
        panic!("expected 6-slot squad array");
    };
    let active_hids: Vec<i64> = sq.iter().filter_map(Value::as_i64).filter(|&h| h > 0).collect();
    assert_eq!(active_hids.len(), 4);

    // Negative: omitting a deployed squad hero in `formation.set` MUST fail!
    let bad_grid = client_a.call_rpc(
        "formation.set",
        vec![Value::map([("1", Value::Int(active_hids[0] as i32))])],
        false,
    );
    assert_eq!(bad_grid.err, "invalid_formation_slot");

    let good_grid = client_a.call_rpc(
        "formation.set",
        vec![Value::map([
            ("1", Value::Int(active_hids[0] as i32)),
            ("4", Value::Int(active_hids[1] as i32)),
            ("7", Value::Int(active_hids[2] as i32)),
            ("9", Value::Int(active_hids[3] as i32)),
        ])],
        false,
    );
    assert_eq!(good_grid.err, "ok");

    // LittleFriend `partner`: summon hero `htid = 309` via GM and assign to partner slot 2
    assert_eq!(
        client_a
            .call_rpc(
                "chat.sendWorld",
                vec![Value::Int(1), Value::String("/hero 309".into())],
                false
            )
            .err,
        "ok"
    );
    if let Value::Object(hm) = client_a.call_rpc("hero.getAllHeroes", vec![], false).ret {
        for (k, hv) in hm.iter() {
            if hv.get_field("htid").and_then(Value::as_i64) == Some(309) {
                partner_hid = k.parse().unwrap();
            }
        }
    }
    assert!(partner_hid > 0);
    assert_eq!(
        client_a
            .call_rpc("formation.openPartner", vec![Value::Int(2)], false)
            .err,
        "ok"
    );
    // Negative: deployed squad hero cannot be placed into partner slot!
    assert_eq!(
        client_a
            .call_rpc(
                "formation.setPartner",
                vec![Value::Int(composed_hid as i32), Value::Int(2)],
                false
            )
            .err,
        "invalid_formation_slot"
    );
    // If GM /hero 309 auto-deployed partner_hid into squad slot 5, unset slot 5 first
    let sq_check = client_a.call_rpc("formation.getHeroes", vec![], false).ret;
    if let Value::Array { dense: sq_slots, .. } = sq_check {
        for (idx, v) in sq_slots.iter().enumerate() {
            if v.as_i64() == Some(i64::from(partner_hid)) {
                assert_eq!(
                    client_a
                        .call_rpc(
                            "formation.unsetHeroFromPosition",
                            vec![Value::Int((idx + 1) as i32)],
                            false
                        )
                        .err,
                    "ok"
                );
            }
        }
    }
    assert_eq!(
        client_a
            .call_rpc(
                "formation.setPartner",
                vec![Value::Int(partner_hid), Value::Int(2)],
                false
            )
            .err,
        "ok"
    );

    // Equipment (`arming`) + Forge: locate starter weapon `112_001` (`20001`) and treasure `182_001` (`10001`)
    let bag_now = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
    let mut starter_wpn_gid = 0i32;
    let mut starter_trs_gid = 0i32;
    if let Some(Value::Object(am)) = bag_now.get_field("arm") {
        for (k, v) in am.iter() {
            if v.get_field("item_template_id").and_then(Value::as_i64) == Some(112_001) {
                starter_wpn_gid = k.parse().unwrap();
            }
        }
    }
    if let Some(Value::Object(tm)) = bag_now.get_field("treasure") {
        for (k, v) in tm.iter() {
            if v.get_field("item_template_id").and_then(Value::as_i64) == Some(182_001) {
                starter_trs_gid = k.parse().unwrap();
            }
        }
    }
    assert!(starter_wpn_gid > 0 && starter_trs_gid > 0);

    // Negative: equipping a Treasure (`182_001`) into Weapon slot `1` MUST fail with `equip_type_mismatch`!
    assert_eq!(
        client_a
            .call_rpc(
                "formation.addArming",
                vec![
                    Value::Int(1),
                    Value::Int(1),
                    Value::Int(starter_trs_gid),
                ],
                false
            )
            .err,
        "equip_type_mismatch"
    );

    // Equip Weapon `112_001` onto slot 1 equip_slot 1, and Treasure `182_001` onto slot 1 equip_slot 5
    assert_eq!(
        client_a
            .call_rpc(
                "formation.addArming",
                vec![
                    Value::Int(1),
                    Value::Int(1),
                    Value::Int(starter_wpn_gid),
                ],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "formation.addArming",
                vec![
                    Value::Int(1),
                    Value::Int(5),
                    Value::Int(starter_trs_gid),
                ],
                false
            )
            .err,
        "ok"
    );

    // Reinforce slot 1 arming via `forge.autoReinforceArm` + reinforce `crafted_arm_gid` in bag via `forge.upgradeArm`
    let auto_rf = client_a.call_rpc("forge.autoReinforceArm", vec![Value::Int(1)], false);
    assert_eq!(auto_rf.err, "ok");
    if let Value::Object(ref step_map) = auto_rf.ret {
        let (_, first_step) = step_map.iter().next().expect("reinforced weapon entry");
        assert!(first_step.get_field("level").and_then(Value::as_i64).unwrap_or(0) > 1);
        assert!(first_step.get_field("upLevel").and_then(Value::as_i64).unwrap_or(0) >= 1);
    } else {
        panic!("expected ArmProxy autoReinforceArm map keyed by item_id, got {:?}", auto_rf.ret);
    }
    let bag_rf = client_a.call_rpc(
        "forge.upgradeArm",
        vec![Value::Int(crafted_arm_gid), Value::Int(3)],
        false,
    );
    assert_eq!(bag_rf.err, "ok");
    assert_eq!(
        bag_rf
            .ret
            .get_field("reinforce_level")
            .and_then(Value::as_i64),
        Some(4)
    );

    // Transfer weapon from squad slot 1 -> squad slot 2, then unequip Treasure from slot 1 back to bag!
    assert_eq!(
        client_a
            .call_rpc(
                "formation.transferArming",
                vec![Value::Int(1), Value::Int(2), Value::Int(1)],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "formation.removeArming",
                vec![Value::Int(1), Value::Int(5)],
                false
            )
            .err,
        "ok"
    );

    // Re-equip crafted weapon `101_102` onto slot 1 so both slot 1 and slot 2 carry reinforced weapons!
    assert_eq!(
        client_a
            .call_rpc(
                "formation.addArming",
                vec![Value::Int(1), Value::Int(1), Value::Int(crafted_arm_gid)],
                false
            )
            .err,
        "ok"
    );

    // Cross-player inspection: Client B inspects Client A (`formation.getOtherPositionInfo`)
    let inspect_a = client_b.call_rpc(
        "formation.getOtherPositionInfo",
        vec![Value::Int(uid_a as i32)],
        false,
    );
    assert_eq!(inspect_a.err, "ok");
    assert_eq!(
        inspect_a.ret.get_field("uname").and_then(Value::as_str),
        Some("LongĐởmThần")
    );

    // =========================================================================
    // DOMAIN 7: Multiplayer Town, RoyalCity & Castle Buildings
    // =========================================================================
    let _ = client_a.drain_pushes();
    let _ = client_b.drain_pushes();
    assert_eq!(
        client_a.call_rpc("city.enter", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client_b.call_rpc("city.enter", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert!(client_a
        .drain_pushes()
        .iter()
        .any(|p| p.callback_name.as_deref() == Some("town.addPlayer")));
    assert_eq!(
        client_b
            .call_rpc("town.move", vec![Value::Int(520), Value::Int(240)], false)
            .err,
        "ok"
    );
    assert!(client_a
        .drain_pushes()
        .iter()
        .any(|p| p.callback_name.as_deref() == Some("town.move")));

    for method in [
        "city.getEnterList",
        "castle.getInfo",
        "hall.getInfo",
        "market.getInfo",
        "workshop.getInfo",
        "smithy.getInfo",
    ] {
        assert_eq!(client_a.call_rpc(method, vec![], false).err, "ok");
    }
    assert_eq!(
        client_a.call_rpc("castle.levelUp", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client_b
            .call_rpc("royalcity.enter", vec![Value::Int(300), Value::Int(200)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_b
            .call_rpc("royalcity.move", vec![Value::Int(350), Value::Int(210)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_b.call_rpc("royalcity.leave", vec![], false).err,
        "ok"
    );

    // =========================================================================
    // DOMAIN 8: Multi-Channel Chat, Private Whisper & GM Slash Commands
    // =========================================================================
    let _ = client_b.drain_pushes();
    assert_eq!(
        client_a
            .call_rpc(
                "chat.sendWorld",
                vec![Value::Int(1), Value::String("Chiến trường Xích Bích mở cửa!".into())],
                true
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "chat.sendPersonal",
                vec![
                    Value::String("QuanVũVIP".into()),
                    Value::Int(1),
                    Value::String("Nhị đệ giữ vững Kinh Châu!".into()),
                ],
                false
            )
            .err,
        "ok"
    );
    let b_chat_pushes = client_b.drain_pushes();
    assert!(
        b_chat_pushes
            .iter()
            .filter(|p| p.callback_name.as_deref() == Some("re.chat.getMsg"))
            .count()
            >= 2,
        "Client B must receive both world broadcast and personal whisper pushes"
    );
    let personal_hist = client_b.call_rpc("chat.getPersonalLastMsgs", vec![], false);
    assert_eq!(personal_hist.err, "ok");

    // =========================================================================
    // DOMAIN 9: Mail Box, Reward Center & Gift Code Redemption
    // =========================================================================
    assert_eq!(
        client_a
            .call_rpc(
                "mail.sendMail",
                vec![
                    Value::Int(uid_b as i32),
                    Value::String("Quân lệnh".into()),
                    Value::String("Xuất binh chinh phạt Bắc Ngụy".into()),
                ],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(client_b.call_rpc("mail.getMailList", vec![], false).err, "ok");
    assert_eq!(client_b.call_rpc("mail.readAllMail", vec![], false).err, "ok");

    let rlist = client_a.call_rpc(
        "reward.getRewardList",
        vec![Value::Int(0), Value::Int(50)],
        false,
    );
    let Value::Array { dense: rewards, .. } = &rlist.ret else {
        panic!("expected reward list array");
    };
    let rid = rewards[0].get_field("rid").and_then(Value::as_i64).unwrap() as i32;
    assert_eq!(
        client_a
            .call_rpc(
                "reward.receiveByRidArr",
                vec![Value::dense_array(vec![Value::Int(rid)])],
                false
            )
            .err,
        "ok"
    );
    // Negative: claiming the same `rid` twice must fail!
    assert_ne!(
        client_a
            .call_rpc(
                "reward.receiveByRidArr",
                vec![Value::dense_array(vec![Value::Int(rid)])],
                false
            )
            .err,
        "ok"
    );

    assert_eq!(
        client_a
            .call_rpc(
                "reward.giftByCode",
                vec![Value::String("GSERVERVIP888".into())],
                false
            )
            .err,
        "ok"
    );
    // Negative: duplicate giftcode redemption must fail!
    assert_ne!(
        client_a
            .call_rpc(
                "reward.giftByCode",
                vec![Value::String("GSERVERVIP888".into())],
                false
            )
            .err,
        "ok"
    );

    // =========================================================================
    // DOMAIN 10: Tavern Gacha (`raffle.*`) & Shops (`shop.*`)
    // =========================================================================
    assert_eq!(client_a.call_rpc("raffle.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client_a
            .call_rpc("raffle.buyRaffleToken", vec![Value::Int(5)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("raffle.raffle", vec![Value::Int(2)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("raffle.multiRaffle", vec![Value::Int(2)], false)
            .err,
        "ok"
    );
    assert_eq!(client_a.call_rpc("raffle.raffleHot", vec![], false).err, "ok");
    assert_eq!(
        client_a
            .call_rpc("raffle.refreshHotByHand", vec![], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("raffle.raffleRed", vec![Value::Int(1)], false)
            .err,
        "ok"
    );

    let shop_info = client_a.call_rpc("shop.getInfo", vec![Value::Int(1)], false);
    let gk = shop_info
        .ret
        .get_field("goods_key")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    assert_eq!(
        client_a
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(1),
                    Value::String(gk),
                    Value::Int(1),
                    Value::Int(101),
                    Value::Int(1),
                ],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("shop.refresh", vec![Value::Int(1)], false)
            .err,
        "ok"
    );

    // =========================================================================
    // DOMAIN 11: PvE Campaign (`copy.*`) + Zlib Battle Replay (`battle.getRecord`)
    // =========================================================================
    let atk = client_a.call_rpc(
        "copy.attack",
        vec![Value::Int(1001), Value::Int(100101)],
        false,
    );
    assert_eq!(atk.err, "ok");
    let brid = atk.ret.get_field("brid").and_then(Value::as_i64).unwrap() as i32;
    let rec = client_a.call_rpc("battle.getRecord", vec![Value::Int(brid)], false);
    let raw_rec = zlib_decompress(rec.ret.as_bytes().unwrap()).unwrap();
    let (rec_amf, _) = amf3::decode(&raw_rec).unwrap();
    assert_eq!(rec_amf.get_field("appraisal").and_then(Value::as_i64), Some(50));

    // Negative: nonexistent battle record ID must fail strictly!
    assert_eq!(
        client_a
            .call_rpc("battle.getRecord", vec![Value::Int(999_999)], false)
            .err,
        "battle_record_not_found"
    );

    assert_eq!(
        client_a
            .call_rpc(
                "copy.pass",
                vec![Value::Int(1001), Value::Int(100101), Value::empty_array()],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "copy.sweep",
                vec![Value::Int(1001), Value::Int(100101), Value::Int(2)],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc(
                "copy.oneKeyResetAtk",
                vec![Value::Int(1001), Value::Int(100101)],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("copy.fetchBoxReward", vec![Value::Int(1001), Value::Int(100101)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a.call_rpc("copy.fetchFresherReward", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client_a.call_rpc("copy.fetchFresherReward", vec![], false).err,
        "already_claimed"
    );

    // =========================================================================
    // DOMAIN 12: PvP Arena, Daily Sign-In, Online Rewards & Cold WAL Restart
    // =========================================================================
    let _ = client_b.call_rpc("arena.enterArena", vec![], false);
    let _ = client_a.call_rpc("arena.enterArena", vec![], false);
    let _ = client_b.drain_pushes();

    let ch = client_a.call_rpc(
        "arena.challenge",
        vec![
            Value::Int(17),
            Value::Int(16),
            Value::Int(0),
            Value::String("QuanVũVIP".into()),
        ],
        false,
    );
    assert_eq!(ch.err, "ok");
    assert_eq!(ch.ret.get_field("position").and_then(Value::as_i64), Some(16));
    assert!(client_b
        .drain_pushes()
        .iter()
        .any(|p| p.callback_name.as_deref() == Some("re.arena.dataRefresh")));

    assert_eq!(
        client_a
            .call_rpc("arena.quickChallenge", vec![Value::Int(16), Value::Int(15)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("arena.buyChallengeTimes", vec![Value::Int(2)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("arena.fetchChallengeNumReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("arena.fetchChallengeNumReward", vec![Value::Int(1)], false)
            .err,
        "already_claimed"
    );

    assert_eq!(client_a.call_rpc("sign.signToday", vec![], false).err, "ok");
    assert_ne!(
        client_a.call_rpc("sign.signToday", vec![], false).err,
        "ok",
        "double daily sign-in must be rejected"
    );
    assert_eq!(
        client_a
            .call_rpc("sign.gainVipSignReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("sign.gainRechargeSignReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client_a
            .call_rpc("online.fetchOnlineReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );

    // Snapshot final Client A state before cold server shutdown
    let snap_user = client_a.call_rpc("user.getUser", vec![], false).ret;
    let snap_gold = snap_user.get_field("gold").and_then(Value::as_i64).unwrap();
    let snap_fc = snap_user
        .get_field("fight_force")
        .or_else(|| snap_user.get_field("fc"))
        .and_then(Value::as_i64)
        .unwrap_or(0);

    drop(client_a);
    drop(client_b);
    server.shutdown();

    // Cold-restart `RunningServer` over the exact same `data_dir` under `--strict-all`
    let server2 = RunningServer::start(cfg).expect("cold restart strict RunningServer");
    let (_, hash_a2, _) = strict_auth_guest(server2.http_addr, "strict_dev_alpha");
    let mut client_a2 = StrictWireClient::connect(server2.tcp_addr);
    assert_eq!(
        client_a2
            .call_rpc(
                "user.login",
                vec![Value::map([
                    ("pid", Value::String(pid_a)),
                    ("qid", Value::String("alpha".into())),
                    ("hash", Value::String(hash_a2)),
                    ("serverID", Value::String("game2900161".into())),
                ])],
                false
            )
            .err,
        "ok"
    );
    assert_eq!(
        client_a2
            .call_rpc("user.userLogin", vec![Value::Int(uid_a as i32)], false)
            .ret
            .as_str(),
        Some("ok")
    );

    let after_user = client_a2.call_rpc("user.getUser", vec![], false).ret;
    assert_eq!(
        after_user.get_field("uname").and_then(Value::as_str),
        Some("LongĐởmThần")
    );
    assert_eq!(after_user.get_field("level").and_then(Value::as_i64), Some(35));
    assert_eq!(
        after_user.get_field("gold").and_then(Value::as_i64),
        Some(snap_gold)
    );
    let after_fc = after_user
        .get_field("fight_force")
        .or_else(|| after_user.get_field("fc"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    assert_eq!(after_fc, snap_fc);

    // Verify composed, leveled, evolved &突破 hero + partner + reinforced arming + 3x3 grid all survived!
    let after_pos = client_a2
        .call_rpc("formation.getPositionInfo", vec![], false)
        .ret;
    let after_hero = after_pos
        .get_field("allHeros")
        .and_then(|m| m.get_field(&composed_hid.to_string()))
        .expect("composed hero persisted");
    assert_eq!(after_hero.get_field("level").and_then(Value::as_i64), Some(6));
    assert_eq!(
        after_hero.get_field("evolve_level").and_then(Value::as_i64),
        Some(1)
    );
    assert_eq!(
        after_hero.get_field("upgrade_level").and_then(Value::as_i64),
        Some(1)
    );

    let after_grid = client_a2.call_rpc("formation.get", vec![], false).ret;
    let mut grid_contains_composed = false;
    if let Some(Value::Object(gm)) = after_grid.get_field("grid_formation") {
        for (_, v) in gm.iter() {
            if v.as_i64() == Some(composed_hid as i64) {
                grid_contains_composed = true;
            }
        }
    }
    assert!(
        grid_contains_composed,
        "composed & deployed hero must remain in 3x3 battle grid after cold restart"
    );

    // VIP 1 gift must still be marked bought after cold restart!
    assert_eq!(
        client_a2
            .call_rpc("user.buyVipGift", vec![Value::Int(1)], false)
            .err,
        "vip_gift_already_bought"
    );

    drop(client_a2);
    server2.shutdown();
}

#[test]
fn issue_3_flow_a_copy_battle_and_flow_b_act_star_gift_claim_live_e2e() {
    let tmp = TempDirGuard::new("issue3_flows");
    let cfg = DaemonConfig::strict_all(tmp.path.clone());
    let server = RunningServer::start(cfg.clone()).expect("start strict RunningServer");
    let (http_addr, tcp_addr) = (server.http_addr, server.tcp_addr);

    let login_and_enter = |http: SocketAddr,
                           tcp: SocketAddr,
                           dev: &str,
                           utid: i32,
                           uname: &str,
                           existing_uid: Option<u64>|
     -> (StrictWireClient, String, u64) {
        let (pid, hash, _) = strict_auth_guest(http, dev);
        let mut cli = StrictWireClient::connect(tcp);
        let login_env = cli.call_rpc(
            "user.login",
            vec![Value::map([
                ("AMF", Value::Int(3)),
                ("pid", Value::String(pid.clone())),
                ("qid", Value::String(pid.clone())),
                ("hash", Value::String(hash)),
                ("serverID", Value::String("game2900161".into())),
            ])],
            true,
        );
        assert_eq!(login_env.err, "ok");
        let uid = if let Some(u) = existing_uid {
            u
        } else {
            let cu = cli.call_rpc(
                "user.createUser",
                vec![Value::Int(utid), Value::String(uname.into())],
                false,
            );
            assert_eq!(cu.err, "ok");
            cli.call_rpc("user.getUserBeforLogin", vec![], false)
                .ret
                .get_field("uid")
                .and_then(Value::as_i64)
                .unwrap() as u64
        };
        assert_eq!(
            cli.call_rpc("user.userLogin", vec![Value::Int(uid as i32)], false)
                .ret
                .as_str(),
            Some("ok")
        );
        (cli, pid, uid)
    };

    // Client A (promoted to Lv.35 + deployed companion) & Client B (stays at starter Lv.10)
    let (mut client_a, _pid_a, uid_a) =
        login_and_enter(http_addr, tcp_addr, "issue3_player_alpha", 1, "ChiếnThầnA6", None);
    let (mut client_b, _pid_b, _uid_b) =
        login_and_enter(http_addr, tcp_addr, "issue3_player_beta", 2, "TânThủB1", None);

    assert_eq!(
        client_a
            .call_rpc("chat.sendWorld", vec![Value::Int(1), Value::String("/level 35".into())], false)
            .err,
        "ok"
    );
    let comp_hero = client_a.call_rpc("exchange.composeHero", vec![Value::Int(50_101), Value::Int(1)], false);
    assert_eq!(comp_hero.err, "ok");
    let comp_hid = comp_hero.ret.get_field("hid").and_then(Value::as_i64).unwrap() as i32;
    assert_eq!(
        client_a
            .call_rpc("formation.setHeroToPosition", vec![Value::Int(comp_hid), Value::Int(2)], false)
            .err,
        "ok"
    );
    let _ = client_a.drain_pushes();

    // === FLOW A: Copy Battle (`copy.attack` + `copy.pass`) Live E2E ===
    for (ch_id, cp_id) in [(0, 0), (1001, 200_101)] {
        assert_eq!(
            client_a.call_rpc("copy.attack", vec![Value::Int(ch_id), Value::Int(cp_id)], false).err,
            "invalid_copy_stage"
        );
    }

    let pre_user = client_a.call_rpc("user.getUser", vec![], false).ret;
    let pre_exec = pre_user.get_field("execution").and_then(Value::as_i64).unwrap();
    let pre_exp = pre_user.get_field("exp").and_then(Value::as_i64).unwrap();
    let pre_copper = parse_copper(&pre_user) as i64;
    let pre_soul = pre_user.get_field("hero_soul").and_then(Value::as_i64).unwrap();
    let pre_gold = pre_user.get_field("gold").and_then(Value::as_i64).unwrap();
    let pre_bag = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
    let pre_17002 = sum_bag_template_count(&pre_bag, "item", 17_002);
    let pre_16012 = sum_bag_template_count(&pre_bag, "item", 16_012);

    let atk_args = vec![Value::Int(1001), Value::Int(100_101)];
    let atk = client_a.call_rpc("copy.attack", atk_args.clone(), false);
    assert_eq!(atk.err, "ok");
    assert_catalog_valid("copy.attack", &atk_args, &atk);
    assert_eq!(atk.ret.get_field("ret").and_then(Value::as_str), Some("ok"));
    let brid = atk.ret.get_field("brid").and_then(Value::as_i64).unwrap();
    assert!(brid > 0);
    assert_eq!(atk.ret.get_field("appraisal").and_then(Value::as_i64), Some(50));

    let atk_drop = atk.ret.get_field("drop").expect("drop dictionary");
    assert_eq!(atk_drop.get_field("copper").and_then(Value::as_i64), Some(1750));
    assert_eq!(atk_drop.get_field("soul").and_then(Value::as_i64), Some(60));
    assert_eq!(
        atk_drop.get_field("item").and_then(|m| m.get_field("17002")).and_then(Value::as_i64),
        Some(1)
    );

    let battle_info = atk.ret.get_field("battleInfo").expect("battleInfo");
    assert_eq!(battle_info.get_field("isPVP").and_then(Value::as_i64), Some(0));
    for team_key in ["arrTeam1", "arrTeam2"] {
        let Value::Object(heroes) = battle_info
            .get_field(team_key)
            .and_then(|t| t.get_field("1"))
            .and_then(|t| t.get_field("arrHero"))
            .expect("arrHero")
        else {
            panic!("expected {team_key}.1.arrHero object");
        };
        if team_key == "arrTeam1" {
            assert_eq!(heroes.iter().count(), 3);
        }
        for (_, unit) in heroes.iter() {
            let Value::Array { dense: skills, .. } = unit.get_field("arrSkill").expect("arrSkill")
            else {
                panic!("expected arrSkill array");
            };
            assert_eq!(skills.len(), 2);
            let Value::Array { dense: s0, .. } = &skills[0] else { panic!("skill 0") };
            let Value::Array { dense: s1, .. } = &skills[1] else { panic!("skill 1") };
            let htid = unit.get_field("htid").and_then(Value::as_i64).unwrap_or(0);
            if team_key == "arrTeam1" {
                let resolved = gserver_data::resolve_hero_battle_skills(htid as u32);
                let expected = match htid {
                    1 => (102, 101),
                    2 => (104, 103),
                    101 => (10102, 10101),
                    401 => (40102, 40101),
                    406 => (40602, 40601),
                    426 => (42602, 42601),
                    _ => (resolved.normal_skill_id as i64, resolved.rage_skill_id as i64),
                };
                assert!(!resolved.used_normal_fallback && !resolved.used_rage_fallback);
                assert_eq!((s0[0].as_i64(), s1[0].as_i64()), (Some(expected.0), Some(expected.1)));
                assert_ne!(s0[0].as_i64(), Some(1001));
                assert_ne!(s1[0].as_i64(), Some(2001));
                assert_ne!(s0[0].as_i64(), Some(1201));
            } else {
                assert_eq!((s0[0].as_i64(), s1[0].as_i64()), (Some(1001), Some(2001)));
            }
        }
    }

    let raw_fight_ret = zlib_decompress(atk.ret.get_field("fightRet").and_then(Value::as_bytes).unwrap()).unwrap();
    let (fight_report, _) = amf3::decode(&raw_fight_ret).unwrap();
    assert_eq!(fight_report.get_field("brid").and_then(Value::as_i64), Some(brid));
    assert_eq!(fight_report.get_field("appraisal").and_then(Value::as_i64), Some(50));
    if let Some(Value::Array { dense: rq, .. }) = fight_report.get_field("rageQueue") {
        for step in rq {
            let rq_sid = step.get_field("skillId").and_then(Value::as_i64).unwrap_or(0);
            assert!(
                rq_sid > 0 && rq_sid != 1201 && gserver_data::is_validated_skill_id(rq_sid as u32),
                "unexpected rageQueue skillId {rq_sid}"
            );
        }
    }
    let rec_env = client_a.call_rpc("battle.getRecord", vec![Value::Int(brid as i32)], false);
    assert_eq!(rec_env.err, "ok");
    let (rec_amf, _) = amf3::decode(&zlib_decompress(rec_env.ret.as_bytes().unwrap()).unwrap()).unwrap();
    assert_eq!(rec_amf.get_field("appraisal").and_then(Value::as_i64), Some(50));

    let _ = client_a.drain_pushes();
    let pass_args = vec![Value::Int(1001), Value::Int(100_101), Value::empty_array()];
    let pass_env = client_a.call_rpc("copy.pass", pass_args.clone(), false);
    assert_eq!(pass_env.err, "ok");
    assert_catalog_valid("copy.pass", &pass_args, &pass_env);
    assert_eq!(pass_env.ret.get_field("battle_result").and_then(Value::as_i64), Some(1));
    assert_eq!(pass_env.ret.get_field("star").and_then(Value::as_i64), Some(3));
    assert_eq!(pass_env.ret.get_field("exp").and_then(Value::as_i64), Some(2100));
    let pass_pushes = client_a.drain_pushes();
    assert!(pass_pushes.iter().any(|p| p.callback_name.as_deref() == Some("user.updateUser")));
    assert!(pass_pushes.iter().any(|p| p.callback_name.as_deref() == Some("bag.update")));

    assert_eq!(client_a.call_rpc("copy.attack", vec![Value::Int(1001), Value::Int(100_106)], false).err, "ok");
    assert_eq!(
        client_a.call_rpc("copy.pass", vec![Value::Int(1001), Value::Int(100_106), Value::empty_array()], false).err,
        "ok"
    );
    assert_eq!(client_a.call_rpc("copy.fetchPrize", vec![Value::Int(1001), Value::Int(0)], false).err, "ok");
    assert_eq!(
        client_a.call_rpc("copy.fetchPrize", vec![Value::Int(1001), Value::Int(0)], false).err,
        "prize_already_claimed"
    );

    let mid_user = client_a.call_rpc("user.getUser", vec![], false).ret;
    assert_eq!(mid_user.get_field("execution").and_then(Value::as_i64), Some(pre_exec - 12));
    assert_eq!(mid_user.get_field("exp").and_then(Value::as_i64), Some(pre_exp + 4200));
    assert_eq!(parse_copper(&mid_user) as i64, pre_copper + 1750 + 3000);
    assert_eq!(mid_user.get_field("hero_soul").and_then(Value::as_i64), Some(pre_soul + 60 + 110));
    assert_eq!(mid_user.get_field("gold").and_then(Value::as_i64), Some(pre_gold + 100));
    let mid_bag = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
    assert_eq!(sum_bag_template_count(&mid_bag, "item", 17_002), pre_17002 + 2);
    assert_eq!(sum_bag_template_count(&mid_bag, "item", 16_012), pre_16012 + 1);

    let copy_list = client_a.call_rpc("copy.getList", vec![Value::Int(1)], false);
    assert_eq!(copy_list.err, "ok");
    assert_catalog_valid("copy.getList", &[Value::Int(1)], &copy_list);
    let c1001 = copy_list.ret.get_field("copyData").and_then(|cd| cd.get_field("1001")).unwrap();
    assert_eq!(c1001.get_field("star").and_then(Value::as_i64), Some(6));
    assert_eq!(c1001.get_field("pass_strongh").and_then(Value::as_i64), Some(2));
    assert_eq!(c1001.get_field("prize").and_then(Value::as_i64), Some(1));
    assert!(copy_list.ret.get_field("copyData").and_then(|cd| cd.get_field("1002")).is_some());

    // === FLOW B: ActStar Gift Claim (`activityStarPlayOpen.receiveGift`) Live E2E ===
    let star_init = client_a.call_rpc("activityStarPlayOpen.getInfo", vec![], false);
    assert_eq!(star_init.err, "ok");
    assert_catalog_valid("activityStarPlayOpen.getInfo", &[], &star_init);
    assert!(matches!(star_init.ret, Value::Array { ref dense, ref assoc } if dense.is_empty() && assoc.is_empty()));

    for bad_id in [0, -1, 99] {
        assert_eq!(
            client_a.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(bad_id)], false).err,
            "invalid_gift_id"
        );
    }
    assert_eq!(
        client_b.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(3)], false).err,
        "level_not_enough"
    );
    for locked_id in [5, 6] {
        assert_eq!(
            client_a.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(locked_id)], false).err,
            "level_not_enough"
        );
    }

    let pre_star_gold = mid_user.get_field("gold").and_then(Value::as_i64).unwrap();
    let pre_star_copper = parse_copper(&mid_user) as i64;
    let pre_10010 = sum_bag_template_count(&mid_bag, "item", 10_010);
    let pre_22202 = sum_bag_template_count(&mid_bag, "item", 22_202);
    let pre_50406 = sum_bag_template_count(&mid_bag, "soul", 50_406);

    let _ = client_a.drain_pushes();
    let g1 = client_a.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(1)], false);
    assert_eq!(g1.err, "ok");
    assert_catalog_valid("activityStarPlayOpen.receiveGift", &[Value::Int(1)], &g1);
    assert_eq!(g1.ret.get_field("id").and_then(Value::as_i64), Some(1));
    let Value::Array { dense: g1_rw, .. } = g1.ret.get_field("reward").unwrap() else { panic!("reward") };
    assert_eq!(g1_rw.len(), 4);
    let star_pushes = client_a.drain_pushes();
    assert!(star_pushes.iter().any(|p| p.callback_name.as_deref() == Some("user.updateUser")));
    assert!(star_pushes.iter().any(|p| p.callback_name.as_deref() == Some("bag.update")));

    let g4 = client_a.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(4)], false);
    assert_eq!(g4.err, "ok");
    assert_catalog_valid("activityStarPlayOpen.receiveGift", &[Value::Int(4)], &g4);

    for dup_id in [1, 4] {
        assert_eq!(
            client_a.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(dup_id)], false).err,
            "already_claimed"
        );
    }

    let post_star_user = client_a.call_rpc("user.getUser", vec![], false).ret;
    assert_eq!(post_star_user.get_field("gold").and_then(Value::as_i64), Some(pre_star_gold + 188 + 588));
    assert_eq!(parse_copper(&post_star_user) as i64, pre_star_copper + 50_000 + 200_000);
    let post_star_bag = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
    assert_eq!(sum_bag_template_count(&post_star_bag, "item", 10_010), pre_10010 + 20);
    assert_eq!(sum_bag_template_count(&post_star_bag, "item", 22_202), pre_22202 + 3);
    assert_eq!(sum_bag_template_count(&post_star_bag, "soul", 50_406), pre_50406 + 35);

    // === Fresh TCP Reconnect + Cold WAL Server Restart Verification ===
    drop(client_a);
    drop(client_b);
    let (mut reconn_client, _, _) =
        login_and_enter(http_addr, tcp_addr, "issue3_player_alpha", 1, "ChiếnThầnA6", Some(uid_a));
    let reconn_star = reconn_client.call_rpc("activityStarPlayOpen.getInfo", vec![], false);
    assert_eq!(reconn_star.err, "ok");
    assert_catalog_valid("activityStarPlayOpen.getInfo", &[], &reconn_star);
    assert_eq!(reconn_star.ret.get_field("1"), Some(&Value::Bool(true)));
    assert_eq!(reconn_star.ret.get_field("4"), Some(&Value::Bool(true)));
    assert!(reconn_star.ret.get_field("2").is_none());
    drop(reconn_client);

    server.shutdown();
    let server2 = RunningServer::start(cfg).expect("cold restart RunningServer");
    let (mut cold_client, _, _) = login_and_enter(
        server2.http_addr,
        server2.tcp_addr,
        "issue3_player_alpha",
        1,
        "ChiếnThầnA6",
        Some(uid_a),
    );
    let cold_user = cold_client.call_rpc("user.getUser", vec![], false).ret;
    assert_eq!(cold_user.get_field("execution").and_then(Value::as_i64), Some(pre_exec - 12));
    assert_eq!(cold_user.get_field("exp").and_then(Value::as_i64), Some(pre_exp + 4200));
    assert_eq!(cold_user.get_field("gold").and_then(Value::as_i64), Some(pre_star_gold + 188 + 588));

    let cold_copy_list = cold_client.call_rpc("copy.getList", vec![Value::Int(1)], false).ret;
    let cold_c1001 = cold_copy_list.get_field("copyData").and_then(|cd| cd.get_field("1001")).unwrap();
    assert_eq!(cold_c1001.get_field("star").and_then(Value::as_i64), Some(6));
    assert_eq!(cold_c1001.get_field("pass_strongh").and_then(Value::as_i64), Some(2));
    assert_eq!(cold_c1001.get_field("prize").and_then(Value::as_i64), Some(1));
    assert!(cold_copy_list.get_field("copyData").and_then(|cd| cd.get_field("1002")).is_some());
    let cold_copy_chap = cold_client.call_rpc("copy.getCopy", vec![Value::Int(1001)], false).ret;
    assert_eq!(
        cold_copy_chap.get_field("va_copy").and_then(|vc| vc.get_field("100106")).and_then(|s| s.get_field("normal_star")).and_then(Value::as_i64),
        Some(3)
    );

    let cold_star = cold_client.call_rpc("activityStarPlayOpen.getInfo", vec![], false);
    assert_eq!(cold_star.err, "ok");
    assert_eq!(cold_star.ret.get_field("1"), Some(&Value::Bool(true)));
    assert_eq!(cold_star.ret.get_field("4"), Some(&Value::Bool(true)));
    assert_eq!(
        cold_client.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(1)], false).err,
        "already_claimed"
    );
    assert_eq!(
        cold_client.call_rpc("activityStarPlayOpen.receiveGift", vec![Value::Int(5)], false).err,
        "level_not_enough"
    );
    let cold_bag = cold_client.call_rpc("bag.bagInfo", vec![], false).ret;
    assert_eq!(sum_bag_template_count(&cold_bag, "item", 17_002), pre_17002 + 2);
    assert_eq!(sum_bag_template_count(&cold_bag, "item", 10_010), pre_10010 + 20);
    assert_eq!(sum_bag_template_count(&cold_bag, "soul", 50_406), pre_50406 + 35);

    drop(cold_client);
    server2.shutdown();
}

#[test]
fn cross_repo_kestrel_canvas_cpp_client_against_strict_gserver() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relay_root = manifest_dir.parent().unwrap().parent().unwrap();
    let runner = relay_root.join("tools").join("run_kestrel_wire_test.py");
    if !runner.is_file() {
        return;
    }

    // Locate sibling kestrel-canvas checkout if present in the workspace
    let mut kestrel_dir: Option<PathBuf> = None;
    if let Ok(env_dir) = std::env::var("KESTREL_CANVAS_DIR") {
        let p = PathBuf::from(env_dir);
        if p.join("tests/test_cobalt_relay_e2e.cpp").is_file() {
            kestrel_dir = Some(p);
        }
    }
    if kestrel_dir.is_none() {
        if let Some(ws_parent) = relay_root.parent() {
            for name in ["kestrel-canvas", "repo"] {
                let cand = ws_parent.join(name);
                if cand.join("tests/test_cobalt_relay_e2e.cpp").is_file() {
                    kestrel_dir = Some(cand);
                    break;
                }
            }
        }
    }
    let Some(kdir) = kestrel_dir else {
        // Standalone cobalt-relay CI without sibling C++ repo still runs the full Rust strict E2E above
        return;
    };

    // Require python3 and g++ on POSIX when sibling C++ checkout is present
    let has_gpp = std::process::Command::new("g++")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let has_py3 = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_gpp || !has_py3 {
        return;
    }

    let gserver_exe = env!("CARGO_BIN_EXE_gserver");
    let status = std::process::Command::new("python3")
        .arg(&runner)
        .arg("--kestrel-dir")
        .arg(&kdir)
        .arg("--gserver-bin")
        .arg(gserver_exe)
        .status()
        .expect("spawn run_kestrel_wire_test.py");
    assert!(
        status.success(),
        "live kestrel-canvas C++17 client <-> cobalt-relay strict E2E test failed"
    );
}

// =============================================================================
// WAVE-2 EXHAUSTIVE MULTI-MODE, `game_TM` (`tm*`), S2C PUSH ORDERING & RESTART E2E
// =============================================================================

impl StrictWireClient {
    /// Sends an RPC with optional custom `raw_args` (e.g. Lua 0-indexed `args[0]` or `{ boxId = 1 }` map)
    /// and collects the ordered sequence of S2C envelopes (including any trailing `[tm]User.updateUser` push),
    /// returning `(ordered_envelopes, main_env_index, main_env)`.
    fn call_rpc_with_raw_ordered(
        &mut self,
        method: &str,
        args: Vec<Value>,
        raw_args: Option<Value>,
        expect_trailing_user_push: Option<&str>,
    ) -> (Vec<S2cEnvelope>, usize, S2cEnvelope) {
        let _ = self.drain_pushes();
        self.seq += 1;
        let cb_name = format!("{method}_{}", self.seq);
        let mut req = RpcRequest::new(method, args, &self.token, cb_name.clone());
        if let Some(raw) = raw_args {
            req.raw_args = Some(raw);
        }
        let wire = req.to_lcpacket_frame(false).expect("encode 24B LCPacket");
        self.stream.write_all(&wire).expect("send C2S frame");
        self.stream.flush().unwrap();

        let mut ordered: Vec<S2cEnvelope> = Vec::new();
        let mut main_idx: Option<usize> = None;
        let mut saw_trailing_user = expect_trailing_user_push.is_none();

        let mut buf = [0u8; 8192];
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while main_idx.is_none() || !saw_trailing_user {
            while let Some(frame) = self.rx.next_frame().expect("decode S2C 8B frame") {
                let batch = decode_s2c_frame_batch(&frame).expect("decode S2C batch");
                for env in batch.items {
                    if let Some(ref tok) = env.token {
                        if !tok.is_empty() && tok != "0" {
                            self.token = tok.clone();
                        }
                    }
                    let idx = ordered.len();
                    if main_idx.is_none() && env.callback_name.as_deref() == Some(cb_name.as_str()) {
                        main_idx = Some(idx);
                    } else if let Some(exp_u) = expect_trailing_user_push {
                        if main_idx.is_some() && env.callback_name.as_deref() == Some(exp_u) {
                            saw_trailing_user = true;
                        }
                    }
                    ordered.push(env);
                }
            }
            if main_idx.is_some() && saw_trailing_user {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for ordered envelopes on {cb_name}: got {:?}",
                ordered
                    .iter()
                    .map(|e| e.callback_name.clone())
                    .collect::<Vec<_>>()
            );
            let _ = self
                .stream
                .set_read_timeout(Some(Duration::from_millis(250)));
            match self.stream.read(&mut buf) {
                Ok(n) if n > 0 => self.rx.feed(&buf[..n]),
                _ => {
                    if main_idx.is_some() {
                        break;
                    }
                }
            }
        }
        let _ = self.stream.set_read_timeout(Some(Duration::from_secs(2)));
        let m_idx = main_idx.expect("main RPC response present");
        let main_env = ordered[m_idx].clone();
        (ordered, m_idx, main_env)
    }

    fn gm(&mut self, cmd: &str) {
        let env = self.call_rpc(
            "chat.sendWorld",
            vec![Value::Int(1), Value::String(cmd.to_string())],
            false,
        );
        assert_eq!(env.err, "ok", "GM command `{cmd}` failed: {env:?}");
    }
}

fn find_bag_entry_by_tid(bag_ret: &Value, sub_bag: &str, target_tid: i64) -> Option<(i32, i64)> {
    let Value::Object(map) = bag_ret.get_field(sub_bag)? else {
        return None;
    };
    for (gid_str, slot) in map.iter() {
        let tid = slot.get_field("item_template_id").and_then(Value::as_i64);
        if tid == Some(target_tid) {
            let gid: i32 = gid_str.parse().ok()?;
            let item_id = slot.get_field("item_id").and_then(Value::as_i64).unwrap_or(0);
            return Some((gid, item_id));
        }
    }
    None
}

fn pick_first_shop_offer(shop_ret: &Value) -> (String, i32, i32) {
    let gk = match shop_ret.get_field("goods_key") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Int(n)) => n.to_string(),
        Some(Value::Double(f)) => (*f as i64).to_string(),
        _ => "0".to_string(),
    };
    let Value::Object(gl) = shop_ret.get_field("goods_list").expect("goods_list map") else {
        panic!("expected goods_list object");
    };
    let (_, first) = gl.iter().next().expect("non-empty goods_list");
    let idx = first.get_field("idx").and_then(Value::as_i64).unwrap_or(1) as i32;
    let id = first.get_field("id").and_then(Value::as_i64).unwrap_or(idx as i64) as i32;
    (gk, idx, id)
}


fn wave2_login_and_enter(
    http_addr: SocketAddr,
    tcp_addr: SocketAddr,
    device_id: &str,
    utid: i32,
    uname: &str,
    existing_uid: Option<u64>,
) -> (StrictWireClient, u64, String) {
    let (pid, hash, _) = strict_auth_guest(http_addr, device_id);
    let mut client = StrictWireClient::connect(tcp_addr);
    let env_login = client.call_rpc(
        "user.login",
        vec![
            Value::map([
                ("AMF", Value::Int(3)),
                ("pid", Value::String(pid.clone())),
                ("qid", Value::String(pid.clone())),
                ("hash", Value::String(hash)),
                ("serverID", Value::String("game2900161".into())),
            ]),
            Value::String(format!("uuid_{device_id}")),
            Value::String("1.0.1194".into()),
        ],
        false,
    );
    assert_eq!(env_login.err, "ok");

    let uid = if let Some(u) = existing_uid {
        u
    } else {
        let cr = client.call_rpc(
            "user.createUser",
            vec![Value::Int(utid), Value::String(uname.to_string())],
            false,
        );
        assert_eq!(cr.err, "ok");
        client
            .call_rpc("user.getUserBeforLogin", vec![], false)
            .ret
            .get_field("uid")
            .and_then(Value::as_i64)
            .unwrap() as u64
    };
    let ul = client.call_rpc("user.userLogin", vec![Value::Int(uid as i32)], false);
    assert_eq!(ul.err, "ok");
    (client, uid, pid)
}

#[test]
fn wave2_raffle_shops_vip_and_s2c_ordering_e2e() {
    let tmp = TempDirGuard::new("w2_raffle_shops");
    let cfg = DaemonConfig::strict_all(tmp.path.clone());
    let server = RunningServer::start(cfg).expect("start strict server");

    let (mut client, uid, _) = wave2_login_and_enter(
        server.http_addr,
        server.tcp_addr,
        "w2_raffle_alpha",
        1,
        "TriệuTửLongW2",
        None,
    );
    assert!(uid > 0);

    // Fund player generously via GM commands for exhaustive raffle & shop testing
    for cmd in [
        "/level 85",
        "/vip 12",
        "/gold 500000",
        "/silver 20000000",
        "/item 16011 15",
        "/item 16012 40",
        "/item 16013 40",
        "/item 19009 20",
        "/item 19056 20",
        "/item 19144 60",
        "/item 19074 110",
        "/item 19207 20",
        "/item 16001 20",
    ] {
        client.gm(cmd);
    }
    let _ = client.drain_pushes();

    // =========================================================================
    // 1. B-09 & B-12 S2C Envelope Ordering + Tavern Raffle (`raffle.*` & `tmRaffle.*`)
    // =========================================================================
    let r_info0 = client.call_rpc("raffle.getInfo", vec![], false);
    assert_eq!(r_info0.err, "ok");

    // 10-pull on Divine pool (`2`) -> `re.hero.new` / `bag.update` MUST arrive BEFORE `main_env`,
    // and `user.updateUser` MUST arrive AFTER `main_env` with numeric `copper`!
    let (envs_10, m_idx_10, main_10) = client.call_rpc_with_raw_ordered(
        "raffle.multiRaffle",
        vec![Value::Int(2)],
        None,
        Some("user.updateUser"),
    );
    assert_eq!(main_10.err, "ok");
    let Value::Array { dense: rw10, .. } = main_10.ret.get_field("reward").expect("reward array")
    else {
        panic!("expected reward array on raffle.multiRaffle");
    };
    assert_eq!(rw10.len(), 10);

    let mut saw_pre_state_push = false;
    let mut saw_post_user_push = false;
    for (idx, env) in envs_10.iter().enumerate() {
        match env.callback_name.as_deref() {
            Some("re.hero.new") | Some("bag.update") => {
                assert!(
                    idx < m_idx_10,
                    "B-09: state push {:?} at {idx} must precede main_env at {m_idx_10}",
                    env.callback_name
                );
                saw_pre_state_push = true;
            }
            Some("user.updateUser") => {
                assert!(
                    idx > m_idx_10,
                    "B-12: user.updateUser at {idx} must follow main_env at {m_idx_10}"
                );
                assert!(
                    matches!(
                        env.ret.get_field("copper"),
                        Some(Value::Int(_) | Value::Double(_))
                    ),
                    "user.updateUser copper must be numeric, got {:?}",
                    env.ret.get_field("copper")
                );
                saw_post_user_push = true;
            }
            _ => {}
        }
    }
    assert!(saw_pre_state_push, "multiRaffle must emit pre-callback state push");
    assert!(saw_post_user_push, "multiRaffle must emit post-callback user.updateUser");

    // `tmRaffle.multiRaffle` -> emits `re.tmHero.new` / `tmBag.update` BEFORE and `tmUser.updateUser` AFTER!
    let (tm_envs, tm_midx, tm_main) = client.call_rpc_with_raw_ordered(
        "tmRaffle.multiRaffle",
        vec![Value::Int(2)],
        None,
        Some("tmUser.updateUser"),
    );
    assert_eq!(tm_main.err, "ok");
    for (idx, env) in tm_envs.iter().enumerate() {
        match env.callback_name.as_deref() {
            Some("re.tmHero.new") | Some("tmBag.update") => assert!(idx < tm_midx),
            Some("tmUser.updateUser") => assert!(idx > tm_midx),
            _ => {}
        }
    }

    // `raffle.singleRaffle` (`[2]`), `raffle.autoRaffle` (multi-pull up to 10 warrior tokens `16011`),
    // `raffle.buyToken` (`5` Gold -> `5` `16013`), `raffle.raffleHot` (`pos = 0`), `raffle.refreshHotByHand`,
    // `raffle.raffleRed` (`pos = 0`), `recruitment.*`, `chat.getLastMainPop`
    assert_eq!(
        client.call_rpc("raffle.singleRaffle", vec![Value::Int(2)], false).err,
        "ok"
    );
    let auto_r = client.call_rpc("raffle.autoRaffle", vec![], false);
    assert_eq!(auto_r.err, "ok");
    if let Some(Value::Array { dense: arw, .. }) = auto_r.ret.get_field("reward") {
        assert_eq!(arw.len(), 5, "autoRaffle must pull all 5 daily free warrior draws at once");
    }

    let pre_buy_tok_gold = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("gold")
        .and_then(Value::as_i64)
        .unwrap();
    let buy_tok = client.call_rpc("raffle.buyToken", vec![Value::Int(5)], false);
    assert_eq!(buy_tok.err, "ok");
    let post_buy_tok_gold = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("gold")
        .and_then(Value::as_i64)
        .unwrap();
    assert_eq!(
        pre_buy_tok_gold - post_buy_tok_gold,
        5,
        "raffle.buyToken(5) must cost 5 Gold (1 Gold = 1 Recruit Seal 16013)"
    );

    // Hot & Red limit pools with 0-based `pos = 0` (`RaffleProxy.lua`)
    assert_eq!(
        client
            .call_rpc("raffle.raffleHot", vec![Value::Int(0), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("raffle.refreshHotByHand", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("raffle.raffleRed", vec![Value::Int(0), Value::Int(1)], false)
            .err,
        "ok"
    );

    // Accumulate enough Divine luck to claim `raffle.receiveLuckHeroGift` (`500` plain Int)
    for _ in 0..3 {
        assert_eq!(
            client.call_rpc("raffle.multiRaffle", vec![Value::Int(2)], false).err,
            "ok"
        );
    }
    let luck_claim = client.call_rpc(
        "raffle.receiveLuckHeroGift",
        vec![Value::Int(500), Value::Int(0)],
        false,
    );
    assert_eq!(luck_claim.err, "ok");

    assert_eq!(client.call_rpc("recruitment.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("recruitment.receiveActiveReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    let pop = client.call_rpc("chat.getLastMainPop", vec![], false);
    assert_eq!(pop.err, "ok");

    // =========================================================================
    // 2. All Secondary Raffles (incl. `{ boxId = 1 }` named map arg!)
    // =========================================================================
    assert_eq!(client.call_rpc("godarmRaffle.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("godarmRaffle.singleRaffle", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("tmGodarmRaffle.multiRaffle", vec![], false).err,
        "ok"
    );

    assert_eq!(client.call_rpc("heroLegendRaffle.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("heroLegendRaffle.buyToken", vec![Value::Int(2000)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmHeroLegendRaffle.raffle", vec![Value::Int(5)], false)
            .err,
        "ok"
    );

    assert_eq!(
        client.call_rpc("activitySoulStoneRecruit.getInfo", vec![], false).err,
        "ok"
    );
    for _ in 0..5 {
        assert_eq!(
            client
                .call_rpc(
                    "activitySoulStoneRecruit.raffle",
                    vec![Value::Int(1), Value::Bool(true)],
                    false,
                )
                .err,
            "ok"
        );
    }
    // Named map argument `{ boxId = 1 }` on `activitySoulStoneRecruit.openBox` (`score >= 50`)!
    let (_, _, ss_box) = client.call_rpc_with_raw_ordered(
        "activitySoulStoneRecruit.openBox",
        vec![],
        Some(Value::map([("boxId", Value::Int(1))])),
        None,
    );
    assert_eq!(ss_box.err, "ok", "openBox with {{boxId=1}} failed: {ss_box:?}");

    assert_eq!(
        client.call_rpc("activityFamousRaffle.getInfo", vec![], false).err,
        "ok"
    );
    for _ in 0..10 {
        assert_eq!(
            client
                .call_rpc(
                    "activityFamousRaffle.raffle",
                    vec![Value::Int(1), Value::Int(1), Value::Bool(true)],
                    false,
                )
                .err,
            "ok"
        );
    }
    let (_, _, fam_box) = client.call_rpc_with_raw_ordered(
        "activityFamousRaffle.openBox",
        vec![],
        Some(Value::map([("boxId", Value::Int(1))])),
        None,
    );
    assert_eq!(fam_box.err, "ok");

    assert_eq!(
        client.call_rpc("activityTopAnimalRecruit.getInfo", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmActivityTopAnimalRecruit.raffle",
                vec![Value::Int(1), Value::Bool(false)],
                false,
            )
            .err,
        "ok"
    );

    assert_eq!(client.call_rpc("raffleArm.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("tmRaffleArm.raffle", vec![Value::Int(1)], false).err,
        "ok"
    );

    // =========================================================================
    // 3. `tmUser.buyCopper` 1-based Dense Array + VIP-0 Gift + Stamina/Execution
    // =========================================================================
    let (bc_envs, bc_midx, bc_main) = client.call_rpc_with_raw_ordered(
        "tmUser.buyCopper",
        vec![Value::Int(3)],
        None,
        Some("tmUser.updateUser"),
    );
    assert_eq!(bc_main.err, "ok");
    let Value::Array {
        dense: bc_rows,
        assoc: bc_assoc,
    } = &bc_main.ret
    else {
        panic!("BuyCoinPanel.lua:187 requires 1-based dense array, got {:?}", bc_main.ret);
    };
    assert!(bc_assoc.is_empty(), "buyCopper array must be purely dense");
    assert_eq!(bc_rows.len(), 3);
    for row in bc_rows {
        let Value::Array { dense: pair, .. } = row else {
            panic!("each buyCopper entry must be [mult, silver]");
        };
        assert_eq!(pair.len(), 2);
        assert!(pair[1].as_i64().unwrap_or(0) >= 100_000);
    }
    assert!(
        bc_envs
            .iter()
            .position(|e| e.callback_name.as_deref() == Some("tmUser.updateUser"))
            .unwrap()
            > bc_midx
    );

    // VIP-0 gift (`args = [0]`) returning `"ok"` (`VipProxy.lua`)!
    let vip0 = client.call_rpc("user.buyVipGift", vec![Value::Int(0)], false);
    assert_eq!(vip0.err, "ok");
    assert_eq!(vip0.ret.as_str(), Some("ok"));
    assert_eq!(
        client.call_rpc("user.buyVipGift", vec![Value::Int(0)], false).err,
        "vip_gift_already_bought"
    );
    let vip_info = client.call_rpc("tmUser.getVipGiftInfo", vec![], false);
    assert_eq!(vip_info.err, "ok");

    assert_eq!(
        client.call_rpc("tmUser.buyExecution", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("tmUser.buyStamina", vec![Value::Int(1)], false).err,
        "ok"
    );

    // =========================================================================
    // 4. All Shops & Extended Currencies (`GoldShop 8`, `MysteryShop 12`, `ArenaShop 2`,
    //    `SoulShop 3`, `TowerShop 7`, `AwakeShop 10`, `Trader`, `BabelShop`, `ActivityShop`)
    // =========================================================================
    // 4a. GoldShop (`8`) stepped pricing
    let gs_info = client.call_rpc("shop.getInfo", vec![Value::Int(8)], false);
    assert_eq!(gs_info.err, "ok");
    let (gs_gk, gs_idx, gs_id) = pick_first_shop_offer(&gs_info.ret);
    let g_before_1 = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("gold")
        .and_then(Value::as_i64)
        .unwrap();
    assert_eq!(
        client
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(8),
                    Value::String(gs_gk.clone()),
                    Value::Int(gs_idx),
                    Value::Int(gs_id),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );
    let g_after_1 = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("gold")
        .and_then(Value::as_i64)
        .unwrap();
    assert_eq!(
        client
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(8),
                    Value::String(gs_gk),
                    Value::Int(gs_idx),
                    Value::Int(gs_id),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );
    let g_after_2 = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("gold")
        .and_then(Value::as_i64)
        .unwrap();
    let cost_1 = g_before_1 - g_after_1;
    let cost_2 = g_after_1 - g_after_2;
    assert!(
        cost_1 > 0 && cost_2 >= cost_1,
        "GoldShop stepped price must be positive and non-decreasing ({cost_1} -> {cost_2})"
    );

    // 4b. MysteryShop (`12`) + `shop.refresh` + `shop.multiRefresh` + `SoulShop` (`3`)
    let mys_info = client.call_rpc("shop.getInfo", vec![Value::Int(12)], false);
    assert_eq!(mys_info.err, "ok");
    assert_eq!(
        client.call_rpc("shop.refresh", vec![Value::Int(12)], false).err,
        "ok"
    );
    let mys_multi = client.call_rpc("shop.multiRefresh", vec![Value::Int(12)], false);
    assert_eq!(mys_multi.err, "ok");
    let (mys_gk, mys_idx, mys_id) = pick_first_shop_offer(&mys_multi.ret);
    // Ensure enough `hero_soul` via SoulShop/raffle or stage before buying if currency is soul
    let soul_info = client.call_rpc("shop.getInfo", vec![Value::Int(3)], false);
    let (s_gk, s_idx, s_id) = pick_first_shop_offer(&soul_info.ret);
    assert_eq!(
        client
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(3),
                    Value::String(s_gk),
                    Value::Int(s_idx),
                    Value::Int(s_id),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );
    let _ = (mys_gk, mys_idx, mys_id);

    // 4c. Earn `arena_coin` via Arena sweep, then spend in `ArenaShop` (`2`) and verify exact sync!
    assert_eq!(client.call_rpc("arena.enterArena", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("arena.sweep", vec![Value::Int(1), Value::Int(5)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("arena.fetchReward", vec![], false).err, "ok");
    let arena_coin_before = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("arena_coin")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    assert!(arena_coin_before >= 200, "expected arena_coin >= 200, got {arena_coin_before}");
    let ar_shop = client.call_rpc("shop.getInfo", vec![Value::Int(2)], false);
    let ar_gk = ar_shop.ret.get_field("goods_key").and_then(Value::as_i64).unwrap_or(0).to_string();
    // Slot idx=7 (`id=7`) costs `200` `arena_coin`
    assert_eq!(
        client
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(2),
                    Value::String(ar_gk),
                    Value::Int(7),
                    Value::Int(7),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );
    let arena_coin_after = client
        .call_rpc("user.getUser", vec![], false)
        .ret
        .get_field("arena_coin")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    assert_eq!(
        arena_coin_before - arena_coin_after,
        200,
        "spent arena_coin must sync back to PlayerStore without .max() resurrection"
    );

    // 4d. Earn `tower_num` via `dota.pass`, then spend in `tmShop` (`TowerShop = 7`)!
    assert_eq!(client.call_rpc("dota.attack", vec![Value::Int(1)], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("dota.pass", vec![Value::Int(1), Value::Bool(true)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("dota.openBox", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("dota.reset", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("dota.sweep", vec![], false).err, "ok");
    let tw_shop = client.call_rpc("shop.getInfo", vec![Value::Int(7)], false);
    assert_eq!(tw_shop.err, "ok");
    let (tw_gk, tw_idx, tw_id) = pick_first_shop_offer(&tw_shop.ret);
    assert_eq!(
        client
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(7),
                    Value::String(tw_gk),
                    Value::Int(tw_idx),
                    Value::Int(tw_id),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );
    let tm_gs = client.call_rpc("tmShop.getInfo", vec![Value::Int(8)], false);
    assert_eq!(tm_gs.err, "ok");
    let (tm_gk, tm_idx, tm_id) = pick_first_shop_offer(&tm_gs.ret);
    assert_eq!(
        client
            .call_rpc(
                "tmShop.buy",
                vec![
                    Value::Int(8),
                    Value::String(tm_gk),
                    Value::Int(tm_idx),
                    Value::Int(tm_id),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );

    // 4e. AwakeShop (`10` — real IDs `>= 365`, offer 1 `id=365` costs `162` Gold!)
    let aw_shop = client.call_rpc("shop.getInfo", vec![Value::Int(10)], false);
    assert_eq!(aw_shop.err, "ok");
    let (aw_gk, aw_idx, aw_id) = pick_first_shop_offer(&aw_shop.ret);
    assert!(aw_id >= 365, "AwakeShop offer IDs must start at 365, got {aw_id}");
    assert_eq!(
        client
            .call_rpc(
                "shop.buy",
                vec![
                    Value::Int(10),
                    Value::String(aw_gk),
                    Value::Int(aw_idx),
                    Value::Int(aw_id),
                    Value::Int(1),
                ],
                false,
            )
            .err,
        "ok"
    );

    // 4f. TraderShop (`timeMachineTrader.*`), BabelTowerShop, LimitShop, WeekendShop, ScoreShop
    assert_eq!(client.call_rpc("timeMachineTrader.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("traderShop.buyFreeGift", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("traderShop.recvFundReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("babeltowerShop.getShopInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("limitShop.getInfo", vec![], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("limitShop.buy", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("weekendShop.buy", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("scoreShop.buy", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );

    drop(client);
    server.shutdown();
}

#[test]
fn wave2_bag_sell_use_openbox_furnace_compose_squad_hero_forge_tm_e2e() {
    let tmp = TempDirGuard::new("w2_player_tm");
    let cfg = DaemonConfig::strict_all(tmp.path.clone());
    let server = RunningServer::start(cfg).expect("start strict server");

    let (mut client, uid, _) = wave2_login_and_enter(
        server.http_addr,
        server.tcp_addr,
        "w2_player_beta",
        1,
        "QuanVũW2TM",
        None,
    );

    for cmd in [
        "/level 90",
        "/vip 12",
        "/gold 500000",
        "/silver 50000000",
        "/item 10028 10",
        "/item 33001 5",
        "/item 30011 5",
        "/item 33003 5",
        "/item 3113002 40",
        "/item 3220021 2",
        "/item 3220022 2",
        "/item 3220023 2",
        "/item 3200105 20",
        "/item 3300101 40",
        "/item 3500401 80",
        "/item 3400101 80",
        "/item 3600101 50",
        "/item 5500101 50",
        "/item 1000001 6",
        "/item 10001 5",
        "/item 701001 2",
        "/item 50406 80",
        "/item 200101 1",
        "/item 200102 1",
        "/item 1400101 1",
        "/item 182003 1",
        "/item 10010 500",
        "/item 10014 50",
        "/item 10015 50",
        "/item 10016 50",
        "/item 10017 20",
        "/item 10018 20",
        "/hero 406",
        "/hero 406",
        "/hero 426",
    ] {
        client.gm(cmd);
    }
    let _ = client.drain_pushes();

    // =========================================================================
    // 1. 0-Indexed Lua `args[0]` Selling + Refinery Selling (`tmBag.sellItems`, `tmRefinery.*`)
    // =========================================================================
    let bag0 = client.call_rpc("tmBag.bagInfo", vec![], false).ret;
    let (gid_16012, iid_16012) =
        find_bag_entry_by_tid(&bag0, "item", 16_012).expect("starter 16012");
    let (gid_22102, iid_22102) =
        find_bag_entry_by_tid(&bag0, "item", 22_102).expect("starter 22102");

    // `tmBag.sellItems` with authentic Lua 0-indexed nested table:
    // `args = { [0] = { [0] = gid, [1] = item_id, [2] = 3 } }`
    let sell_inner = Value::Array {
        dense: vec![Value::Int(iid_16012 as i32), Value::Int(3)],
        assoc: vec![("0".to_string(), Value::Int(gid_16012))],
    };
    let sell_outer = Value::Array {
        dense: vec![],
        assoc: vec![("0".to_string(), sell_inner)],
    };
    let (sell_envs, sell_midx, sell_main) = client.call_rpc_with_raw_ordered(
        "tmBag.sellItems",
        vec![],
        Some(sell_outer),
        Some("tmUser.updateUser"),
    );
    assert_eq!(sell_main.err, "ok");
    assert_eq!(sell_main.ret.as_str(), Some("ok"));
    assert!(
        sell_envs
            .iter()
            .position(|e| e.callback_name.as_deref() == Some("tmBag.update"))
            .unwrap()
            < sell_midx
    );

    // `tmRefinery.refineryFragment`, `tmRefinery.refinerySoul`, `tmRefinery.refineryHeroLegend`
    let (frag_gid, _) = find_bag_entry_by_tid(&bag0, "fragment", 3_113_001).expect("starter armFrag");
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryFragment",
                vec![Value::dense_array(vec![Value::Int(frag_gid)])],
                false,
            )
            .err,
        "ok"
    );
    let (soul_gid, _) = find_bag_entry_by_tid(&bag0, "soul", 50_406).expect("50406 soul");
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refinerySoul",
                vec![Value::dense_array(vec![Value::Int(soul_gid)])],
                false,
            )
            .err,
        "ok"
    );
    let (hl_gid, _) = find_bag_entry_by_tid(&bag0, "hero_legend", 701_001).expect("701001 heroLegend");
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryHeroLegend",
                vec![Value::dense_array(vec![Value::Int(hl_gid)])],
                false,
            )
            .err,
        "ok"
    );

    // =========================================================================
    // 2. 0-Indexed `tmExchange.useItemInBag`, `useItemInBagByTpl`, `openRandgiftBox*` & Bag Expansion
    // =========================================================================
    // Lua 0-indexed `args[0] = gid, args[1] = item_id, args[2] = 2`!
    let use_raw = Value::Array {
        dense: vec![Value::Int(iid_22102 as i32), Value::Int(2)],
        assoc: vec![("0".to_string(), Value::Int(gid_22102))],
    };
    let (_, _, use_env) = client.call_rpc_with_raw_ordered(
        "tmExchange.useItemInBag",
        vec![],
        Some(use_raw),
        Some("tmUser.updateUser"),
    );
    assert_eq!(use_env.err, "ok");
    assert_eq!(
        use_env.ret.get_field("execution").and_then(Value::as_i64),
        Some(50)
    );

    assert_eq!(
        client
            .call_rpc(
                "tmExchange.useItemInBagByTpl",
                vec![Value::Int(22_202), Value::Int(2)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("tmExchange.getOpenBoxInfo", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.openRandgiftBoxByKey",
                vec![Value::Int(33_001), Value::Int(2)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.openRandgiftBoxByGold",
                vec![Value::Int(33_003), Value::Int(2)],
                false,
            )
            .err,
        "ok"
    );

    // Bag expansion (`tmBag.openGridByGold`, `tmBag.openGridByItem`, `tmUser.openHeroGrid`)
    assert_eq!(
        client
            .call_rpc(
                "tmBag.openGridByGold",
                vec![Value::Int(2), Value::String("arm".into())],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmBag.openGridByItem",
                vec![Value::Int(2), Value::String("treasure".into())],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmUser.openHeroGrid", vec![Value::Int(1)], false)
            .err,
        "ok"
    );

    // =========================================================================
    // 3. Synthesis (`tmExchange.compose*`, `quickCompose*` & `emExchange.composeTreasure`)
    // =========================================================================
    let (ch_envs, ch_midx, ch_main) = client.call_rpc_with_raw_ordered(
        "tmExchange.composeHero",
        vec![Value::Int(50_101), Value::Int(1)],
        None,
        Some("tmUser.updateUser"),
    );
    assert_eq!(ch_main.err, "ok");
    let composed_hid = ch_main.ret.get_field("hid").and_then(Value::as_i64).unwrap();
    assert!(
        ch_envs
            .iter()
            .position(|e| e.callback_name.as_deref() == Some("re.tmHero.new"))
            .unwrap()
            < ch_midx
    );

    // Equipment compose (`3_113_002` x20 -> `113_002`) + `quickComposeArm` (remaining x20 -> `113_002`)
    let c_arm1 = client.call_rpc(
        "tmExchange.composeArm",
        vec![Value::Int(3_113_002), Value::Int(1)],
        false,
    );
    assert_eq!(c_arm1.err, "ok");
    assert_eq!(
        client.call_rpc("tmExchange.quickComposeArm", vec![], false).err,
        "ok"
    );

    // Client typo `emExchange.composeTreasure` (`3_220_021..=3_220_023` -> `182_002`) + `oneKeycomposeTreasure` + `composeTreasureByFragment`
    let em_tr = client.call_rpc(
        "emExchange.composeTreasure",
        vec![Value::Int(182_002), Value::Int(1)],
        false,
    );
    assert_eq!(em_tr.err, "ok", "emExchange.composeTreasure failed: {em_tr:?}");
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.oneKeycomposeTreasure",
                vec![Value::Int(182_002)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeTreasureByFragment",
                vec![Value::Int(3_200_105), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );

    // Awake (`x2`), GodArm (`x2`), Pet (`x2`), GodAnimal, TopAnimal, Gem, Normal
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeAwake",
                vec![Value::Int(3_300_101), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("tmExchange.quickComposeAwake", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeGodArm",
                vec![Value::Int(3_500_401), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("tmExchange.quickComposeGodArm", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composePet",
                vec![Value::Int(3_400_101), Value::Int(2)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeGodAnimal",
                vec![Value::Int(3_600_101), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeTopAnimal",
                vec![Value::Int(5_500_101), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeGem",
                vec![Value::Int(1_000_001), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("tmExchange.quickComposeGem", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmExchange.composeNormal",
                vec![Value::Int(10_001), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );

    // =========================================================================
    // 4. Squad, Battle Grid (`{"2","3","5","6","8","9"}`), Slot Gear & Spirit (`tmFormation.*`)
    // =========================================================================
    let f_get = client.call_rpc("tmFormation.get", vec![], false);
    assert_eq!(f_get.err, "ok");
    let grid_val = f_get.ret.get_field("formation").unwrap_or(&f_get.ret);
    let Value::Object(f_cells) = grid_val else {
        panic!("expected 6-cell battle grid map from tmFormation.get, got {:?}", f_get.ret);
    };
    let valid_cells = ["2", "3", "5", "6", "8", "9"];
    assert!((1..=6).contains(&f_cells.iter().count()));
    for (k, _) in f_cells.iter() {
        assert!(
            valid_cells.contains(&k),
            "tmFormation.get emitted illegal grid cell key `{k}`"
        );
    }

    // Locate bench heroes `406` (two copies) and `426`
    let all_h = client.call_rpc("tmHero.getAllHeroes", vec![], false).ret;
    let mut hids_406 = Vec::new();
    let mut hid_426 = 0i64;
    if let Value::Object(hm) = &all_h {
        for (_, hv) in hm.iter() {
            let htid = hv.get_field("htid").and_then(Value::as_i64).unwrap_or(0);
            let hid = hv.get_field("hid").and_then(Value::as_i64).unwrap_or(0);
            if htid == 406 {
                hids_406.push(hid);
            } else if htid == 426 {
                hid_426 = hid;
            }
        }
    }
    assert!(hids_406.len() >= 2 && hid_426 > 0);
    let main_406 = hids_406[0] as i32;
    let sac_406 = hids_406[1] as i32;

    assert_eq!(
        client
            .call_rpc(
                "tmFormation.setHeroToPosition",
                vec![Value::Int(composed_hid as i32), Value::Int(2)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.setHeroToPosition",
                vec![Value::Int(main_406), Value::Int(3)],
                false,
            )
            .err,
        "ok"
    );
    // If GM `/hero 426` auto-deployed `hid_426` into squad `position`, unset it first before placing in Partner!
    if let Value::Array { dense: sq_slots, .. } =
        client.call_rpc("tmFormation.getHeroes", vec![], false).ret
    {
        for (idx, v) in sq_slots.iter().enumerate() {
            if v.as_i64() == Some(hid_426) {
                let unset_res = client.call_rpc(
                    "tmFormation.unsetHeroFromPosition",
                    vec![Value::Int((idx + 1) as i32)],
                    false,
                );
                assert_eq!(unset_res.err, "ok");
                assert_eq!(unset_res.ret, Value::Bool(true));
            }
        }
    }
    assert_eq!(
        client
            .call_rpc("tmFormation.openPartner", vec![Value::Int(7)], false)
            .err,
        "ok"
    );
    let set_partner_res = client.call_rpc(
        "tmFormation.setPartner",
        vec![Value::Int(7), Value::Int(hid_426 as i32)],
        false,
    );
    assert_eq!(set_partner_res.err, "ok");
    assert_eq!(set_partner_res.ret, Value::Bool(true));
    assert_eq!(
        client.call_rpc("tmFormation.unsetMultiPartner", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.openBuddy", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.setBuddy",
                vec![Value::Int(1), Value::Int(hid_426 as i32)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.buddyLvUp", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.rebornBuddy", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.activateMorePower", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.morePowerAutoLvUp", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.getOtherPositionInfo",
                vec![Value::Int(uid as i32)],
                false,
            )
            .err,
        "ok"
    );

    // Equip Arm, Treasure, GodAnimal, TopAnimal, and Spirit onto Squad Slot 1/2
    let bag1 = client.call_rpc("tmBag.bagInfo", vec![], false).ret;
    let (gid_weap, iid_weap) = find_bag_entry_by_tid(&bag1, "arm", 112_001).expect("starter weapon");
    let (_, iid_treas) = find_bag_entry_by_tid(&bag1, "treasure", 182_001).expect("starter treasure");
    let (_, iid_mat_treas) =
        find_bag_entry_by_tid(&bag1, "treasure", 182_003).expect("material treasure");
    let (_, iid_ga) = find_bag_entry_by_tid(&bag1, "god_animal", 600_101).expect("600101 godAnimal");
    let (_, iid_ta) = find_bag_entry_by_tid(&bag1, "top_animal", 5_400_101).expect("5400101 topAnimal");
    let (_, iid_sp1) = find_bag_entry_by_tid(&bag1, "spirit", 200_101).expect("200101 spirit");

    assert_eq!(
        client
            .call_rpc(
                "tmFormation.addArming",
                vec![Value::Int(1), Value::Int(1), Value::Int(gid_weap)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmHero.addMultiArming",
                vec![
                    Value::Int(1),
                    Value::map([("5", Value::Int(iid_treas as i32))]),
                ],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.transferArming",
                vec![Value::Int(1), Value::Int(2), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.addGodAnimal",
                vec![Value::Int(1), Value::Int(iid_ga as i32)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.removeGodAnimal", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.addTopAnimal",
                vec![Value::Int(1), Value::Int(iid_ta as i32)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.removeTopAnimal", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "spirit.addSpirit",
                vec![Value::Int(1), Value::Int(1), Value::Int(iid_sp1 as i32)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "spirit.transferSpirit",
                vec![Value::Int(1), Value::Int(2), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "spirit.removeSpirit",
                vec![Value::Int(2), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );

    // =========================================================================
    // 5. Hero Progression, Evolve, Star, Wash, Pill, Dress, Gold Tier & Forge
    // =========================================================================
    let lv_up = client.call_rpc(
        "tmHero.heroLevelUp",
        vec![Value::Int(main_406), Value::Int(10)],
        false,
    );
    assert_eq!(lv_up.err, "ok");
    assert!(lv_up.ret.get_field("oldLv").is_some());
    assert!(lv_up.ret.get_field("newlv").and_then(Value::as_i64).unwrap_or(0) > 1);
    assert!(lv_up.ret.get_field("itemCost").is_some());

    assert_eq!(
        client
            .call_rpc("tmHero.upgrade", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmHero.awake", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmHero.lock", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmHero.unlock", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );

    // Ensure `sac_406` is undeployed (GM `/hero 406` auto-fills open squad slots) before evolving!
    let pos_info_ev = client.call_rpc("tmFormation.getPositionInfo", vec![], false).ret;
    if let Some(pos_map) = pos_info_ev.get_field("position") {
        for slot in 2..=6 {
            let k = slot.to_string();
            if pos_map.get_field(&k).and_then(Value::as_i64) == Some(i64::from(sac_406)) {
                assert_eq!(
                    client
                        .call_rpc("tmFormation.unsetHeroFromPosition", vec![Value::Int(slot)], false)
                        .err,
                    "ok"
                );
            }
        }
    }
    // Evolve `main_406` using real 1-arg Lua contract (`{ hid }` auto-picks unlocked undeployed `sac_406`!)
    let ev = client.call_rpc(
        "tmEvolve.doEvolve",
        vec![Value::Int(main_406)],
        false,
    );
    assert_eq!(ev.err, "ok");
    assert!(ev.ret.get_field("newEvolvelv").and_then(Value::as_i64).unwrap_or(0) >= 1);

    assert_eq!(
        client
            .call_rpc("tmHerostar.levelUp", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmWash.doWash",
                vec![Value::Int(main_406), Value::Int(1), Value::Int(0)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmWash.save", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("heroPill.addPill", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmHeroPill.reborn", vec![Value::Int(main_406)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmDress.wear", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmDress.rebornDress", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    for gold_rpc in [
        "tmHeroGold.upTier",
        "tmDarkGold.upTier",
        "tmPurpleGold.upTier",
        "tmColorGold.upTier",
        "tmNirvanaGold.upTier",
        "tmGildGold.upTier",
    ] {
        assert_eq!(
            client
                .call_rpc(gold_rpc, vec![Value::Int(main_406)], false)
                .err,
            "ok"
        );
    }

    // Forge: `reinforceArm`, `autoReinforceArm` (`{ [item_id]: { level, upLevel } }`),
    // `upgradeArm`, `armWashRefresh` + `armWashApply`, `reinforceTreasure`, `upgradeTreasure`,
    // `reinforceSpirit`, `exploreSoul.explore`
    assert_eq!(
        client
            .call_rpc(
                "tmForge.reinforceArm",
                vec![Value::Int(iid_weap as i32), Value::Int(5)],
                false,
            )
            .err,
        "ok"
    );
    let auto_reinf = client.call_rpc(
        "tmForge.autoReinforceArm",
        vec![Value::Int(2)],
        false,
    );
    assert_eq!(auto_reinf.err, "ok");
    let weap_key = iid_weap.to_string();
    assert!(
        auto_reinf
            .ret
            .get_field(&weap_key)
            .and_then(|s| s.get_field("level"))
            .is_some(),
        "autoReinforceArm must return {{ [item_id]: {{ level, upLevel }} }}"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmForge.upgradeArm",
                vec![
                    Value::Int(iid_weap as i32),
                    Value::map([("10016", Value::Int(5))]),
                ],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmForge.armWashRefresh",
                vec![Value::Int(iid_weap as i32), Value::Int(0), Value::Int(0)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmForge.armWashApply", vec![Value::Int(iid_weap as i32)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmForge.reinforceTreasure",
                vec![
                    Value::Int(iid_treas as i32),
                    Value::dense_array(vec![Value::Int(iid_mat_treas as i32)]),
                ],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmForge.upgradeTreasure",
                vec![Value::Int(iid_treas as i32)],
                false,
            )
            .err,
        "ok"
    );
    // Hunt souls first (`tmExploreSoul.explore`) or decompose a fodder spirit so `spirit_coin` / fodder is earned, then reinforce `iid_sp1`!
    assert_eq!(
        client
            .call_rpc("tmExploreSoul.explore", vec![Value::Int(5)], false)
            .err,
        "ok"
    );
    let bag_sp = client.call_rpc("tmBag.bagInfo", vec![], false).ret;
    let mut fodder_sp = Vec::new();
    if let Some(Value::Object(m)) = bag_sp.get_field("spirit") {
        for (_, v) in m.iter() {
            if let Some(id) = v.get_field("item_id").and_then(Value::as_i64) {
                if id != iid_sp1 {
                    fodder_sp.push(id as i32);
                }
            }
        }
    }
    assert!(!fodder_sp.is_empty(), "tmExploreSoul.explore must grant spirit items");
    assert_eq!(
        client
            .call_rpc(
                "tmForge.reinforceSpirit",
                vec![
                    Value::Int(iid_sp1 as i32),
                    Value::dense_array(vec![Value::Int(fodder_sp[0])]),
                ],
                false,
            )
            .err,
        "ok"
    );

    // =========================================================================
    // 6. Furnace Decompose & Reborn (`tmRefinery.refinery*`, `reborn*`, `resolveSpirit`, `soulSkill.refinery`)
    //    including currency loop (`refineryAwake -> AwakeArmCoin -> upgradeAwake`,
    //    `refineryGodArm -> GodarmCoin -> godArmUpgrade`)!
    // =========================================================================
    // Unequip weapon & treasure first so we can test reborn/decompose cleanly
    assert_eq!(
        client
            .call_rpc("tmFormation.removeArming", vec![Value::Int(2), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmFormation.removeArming", vec![Value::Int(1), Value::Int(5)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmFormation.unsetHeroFromPosition",
                vec![Value::Int(3)],
                false,
            )
            .err,
        "ok"
    );

    let bag2 = client.call_rpc("tmBag.bagInfo", vec![], false).ret;
    // Collect two Awake (`300_101`), two GodArm (`500_401`), two Pet (`400_101`), crafted Arm (`113_002`),
    // crafted Treasure (`182_002`), Spirit (`200_102`), SoulSkill (`1_400_101`)
    let collect_iids = |sub: &str, tid: i64| -> Vec<i32> {
        let mut out = Vec::new();
        if let Some(Value::Object(m)) = bag2.get_field(sub) {
            for (_, v) in m.iter() {
                if v.get_field("item_template_id").and_then(Value::as_i64) == Some(tid) {
                    if let Some(id) = v.get_field("item_id").and_then(Value::as_i64) {
                        out.push(id as i32);
                    }
                }
            }
        }
        out
    };
    let awake_ids = collect_iids("awake", 300_101);
    let godarm_ids = collect_iids("god_arm", 500_401);
    let pet_ids = collect_iids("pet", 400_101);
    let arm_113002 = collect_iids("arm", 113_002);
    let tr_182002 = collect_iids("treasure", 182_002);
    let sp_200102 = collect_iids("spirit", 200_102);
    let ss_1400101 = collect_iids("soul_skill", 1_400_101);

    // Decompose first Awake (`300_101`) & first GodArm (`500_401`) -> grants `AwakeArmCoin` & `GodarmCoin`,
    // then upgrade the second Awake & second GodArm, then Reborn them!
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryAwake",
                vec![Value::dense_array(vec![Value::Int(awake_ids[0])])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmForge.upgradeAwake", vec![Value::Int(awake_ids[1])], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.rebornAwake",
                vec![Value::dense_array(vec![Value::Int(awake_ids[1])])],
                false,
            )
            .err,
        "ok"
    );

    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryGodArm",
                vec![Value::dense_array(vec![Value::Int(godarm_ids[0])])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmForge.godArmUpgrade",
                vec![Value::Int(godarm_ids[1]), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.rebornGodArm",
                vec![Value::dense_array(vec![Value::Int(godarm_ids[1])])],
                false,
            )
            .err,
        "ok"
    );

    // Reborn upgraded Hero (`main_406`), Arm (`iid_weap`), Treasure (`iid_treas`), Spirit (`iid_sp1`)
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.rebornHero",
                vec![Value::dense_array(vec![Value::Int(main_406)])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.rebornArm",
                vec![Value::dense_array(vec![Value::Int(iid_weap as i32)])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.rebornTreasure",
                vec![Value::dense_array(vec![Value::Int(iid_treas as i32)])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.rebornSpirit",
                vec![Value::dense_array(vec![Value::Int(iid_sp1 as i32)])],
                false,
            )
            .err,
        "ok"
    );

    // Remove `hid_426` from Buddy slot 1 (`hid = 0`) first so it returns to bench before decomposing!
    let clear_buddy = client.call_rpc(
        "tmFormation.setBuddy",
        vec![Value::Int(1), Value::Int(0)],
        false,
    );
    assert_eq!(clear_buddy.err, "ok");
    assert_eq!(clear_buddy.ret, Value::Bool(true));

    // Decompose bench Hero (`tmRefinery.refineryHeros` exact `NetConst.lua:179` spelling!),
    // Arm, Treasure, Pet, Spirit (`resolveSpirit`), TopAnimal (`tmRefinery.refineryTopAnimal`), SoulSkill (`soulSkill.refinery`)
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryHeros",
                vec![Value::dense_array(vec![Value::Int(hid_426 as i32)])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryArm",
                vec![Value::dense_array(vec![Value::Int(arm_113002[0])])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryTreasure",
                vec![Value::dense_array(vec![Value::Int(tr_182002[0])])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryPet",
                vec![Value::dense_array(vec![Value::Int(pet_ids[0])])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "refinery.resolveSpirit",
                vec![Value::dense_array(vec![Value::Int(sp_200102[0])])],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmRefinery.refineryTopAnimal",
                vec![Value::dense_array(vec![Value::Int(iid_ta as i32)])],
                false,
            )
            .err,
        "ok"
    );
    let ss_dec = client.call_rpc(
        "soulSkill.refinery",
        vec![Value::dense_array(vec![Value::Int(ss_1400101[0])])],
        false,
    );
    assert_eq!(ss_dec.err, "ok");

    drop(client);
    server.shutdown();
}

#[test]
fn wave2_all_playable_modes_pipelined_resilience_and_b08_b19_restart_e2e() {
    let tmp = TempDirGuard::new("w2_modes_restart");
    let cfg = DaemonConfig::strict_all(tmp.path.clone());
    let server = RunningServer::start(cfg.clone()).expect("start strict server");

    let (mut client, uid, _) = wave2_login_and_enter(
        server.http_addr,
        server.tcp_addr,
        "w2_modes_gamma",
        1,
        "LữBốW2Play",
        None,
    );

    for cmd in [
        "/level 95",
        "/vip 12",
        "/gold 500000",
        "/silver 20000000",
        "/item 22102 30",
        "/mail 17002 5",
    ] {
        client.gm(cmd);
    }
    // Top up execution via stamina pills for full campaign + dungeon runs
    let bag_init = client.call_rpc("bag.bagInfo", vec![], false).ret;
    let (gid_pill, _) = find_bag_entry_by_tid(&bag_init, "item", 22_102).unwrap();
    assert_eq!(
        client
            .call_rpc(
                "exchange.useItemInBag",
                vec![Value::Int(gid_pill), Value::Int(15)],
                false,
            )
            .err,
        "ok"
    );
    let _ = client.drain_pushes();

    // =========================================================================
    // 1. Campaign (`copy.*`), Chapter Unlock `1001 -> 1002`, `sweep` Top-Level Array,
    //    `timeMachineCopy.*`, `ecopy.*` & `copy_daily.*`
    // =========================================================================
    for stage in [100_101, 100_102, 100_103, 100_105, 100_106] {
        assert_eq!(
            client
                .call_rpc("copy.attack", vec![Value::Int(1001), Value::Int(stage)], false)
                .err,
            "ok"
        );
        assert_eq!(
            client
                .call_rpc(
                    "copy.pass",
                    vec![Value::Int(1001), Value::Int(stage), Value::empty_array()],
                    false,
                )
                .err,
            "ok"
        );
    }
    // Chapter `1002` AND Elite Chapter `2001` (`level >= 26`) must now be unlocked!
    let clist = client.call_rpc("copy.getList", vec![Value::Int(1)], false).ret;
    assert!(
        clist
            .get_field("copyData")
            .and_then(|cd| cd.get_field("1002"))
            .is_some(),
        "clearing 100106 must unlock chapter 1002"
    );
    // `copy.sweep` (`CopySweepPanel.lua:79` requires top-level 1-indexed `Value::Array`!)
    let sweep = client.call_rpc(
        "copy.sweep",
        vec![Value::Int(1001), Value::Int(100_101), Value::Int(2)],
        false,
    );
    assert_eq!(sweep.err, "ok");
    let Value::Array { dense: sweep_rounds, .. } = &sweep.ret else {
        panic!("copy.sweep must return top-level Array, got {:?}", sweep.ret);
    };
    assert_eq!(sweep_rounds.len(), 2);
    assert_eq!(
        client
            .call_rpc(
                "copy.fetchBoxReward",
                vec![Value::Int(1001), Value::Int(100_101)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("copy.fetchPrize", vec![Value::Int(1001), Value::Int(2)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "copy.resetAtk",
                vec![Value::Int(1001), Value::Int(100_101)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("copy.fetchFresherReward", vec![], false).err,
        "ok"
    );

    // `timeMachineCopy.*` (`100_104` is exclusive to TM 10-stage chapter `1001`!)
    assert_eq!(client.call_rpc("timeMachineCopy.getList", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("timeMachineCopy.getCopy", vec![Value::Int(1001)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "timeMachineCopy.attack",
                vec![Value::Int(1001), Value::Int(100_104)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "timeMachineCopy.pass",
                vec![Value::Int(1001), Value::Int(100_104)],
                false,
            )
            .err,
        "ok"
    );

    // Elite Copy (`ecopy.*`)
    assert_eq!(client.call_rpc("ecopy.getEliteCopyInfo", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("ecopy.getCopy", vec![Value::Int(2001)], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("ecopy.attack", vec![Value::Int(2001), Value::Int(200_101)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("ecopy.pass", vec![Value::Int(2001), Value::Int(200_101)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "ecopy.sweep",
                vec![Value::Int(2001), Value::Int(200_101), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("ecopy.reset", vec![Value::Int(2001), Value::Int(200_101)], false)
            .err,
        "ok"
    );

    // Daily Copy (`copy_daily.*` & `timeMachineCopy_daily.*`)
    let dc_info = client.call_rpc("copy_daily.getInfo", vec![Value::Int(1)], false);
    assert_eq!(dc_info.err, "ok");
    assert!(matches!(dc_info.ret.get_field("pass"), Some(Value::Array { .. })));
    assert_eq!(
        client
            .call_rpc("copy_daily.attack", vec![Value::Int(1), Value::Int(101)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("copy_daily.pass", vec![Value::Int(1), Value::Int(101)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("copy_daily.sweep", vec![Value::Int(1), Value::Int(101)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("copy_daily.buyAtkNum", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("timeMachineCopy_daily.getInfo", vec![], false)
            .err,
        "ok"
    );

    // =========================================================================
    // 2. All 12 `babeltower.*` RPCs + `dota.*` / `purgatory.*` + `awakeCopy.*`
    //    + `rebel.getRebelInfo` / `boss.*` + `discover.*` + `devilCopy.*` + `soaringCopy.*`
    // =========================================================================
    assert_eq!(client.call_rpc("babeltower.getBabeltowerInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("babeltower.getBagInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("babeltower.setIdx", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("babeltower.attack", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "babeltower.answer",
                vec![Value::Int(1), Value::Int(2), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "babeltower.specialEvent",
                vec![Value::Int(1), Value::Int(4)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("babeltower.passFloor", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "babeltower.receiveReward",
                vec![Value::Int(1), Value::Int(1)],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("babeltower.autoReceiveReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("babeltower.buyAtkNum", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("babeltower.reset", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("babeltower.sweep", vec![], false).err, "ok");

    // Dota Tower & Purgatory
    assert_eq!(client.call_rpc("dota.getInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("dota.attack", vec![Value::Int(1)], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("dota.pass", vec![Value::Int(1), Value::Bool(true)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("dota.openBox", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("dota.buyAtkNum", vec![Value::Int(1)], false).err, "ok");
    assert_eq!(client.call_rpc("dota.reset", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("dota.sweep", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("purgatory.getInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("purgatory.attack", vec![Value::Int(1)], false).err, "ok");
    assert_eq!(client.call_rpc("purgatory.pass", vec![Value::Int(1)], false).err, "ok");

    // AwakeCopy
    assert_eq!(client.call_rpc("awakeCopy.getAwakeCopyInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("awakeCopy.getRoomList", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("awakeCopy.quickBattle", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("awakeCopy.buyNum", vec![Value::Int(1)], false).err,
        "ok"
    );

    // Rebel (`rebel.getRebelInfo`) & World Boss
    let reb_info = client.call_rpc("rebel.getRebelInfo", vec![], false);
    assert_eq!(reb_info.err, "ok");
    assert!(reb_info.ret.get_field("rebel_list").is_some() || reb_info.ret.get_field("list").is_some() || matches!(reb_info.ret, Value::Object(_)));
    assert_eq!(client.call_rpc("rebel.enterRebel", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("rebel.attackRebel", vec![Value::Int(1), Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("rebel.buyRebelAtkNum", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("boss.getBossData", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("boss.attack", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("boss.getFiendInfo", vec![], false).err, "ok");

    // Discover, DevilCopy, SoaringCopy
    assert_eq!(client.call_rpc("discover.getRewardCity", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("discover.getDiscover", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("discover.attack", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("discover.pass", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client.call_rpc("discover.patrol", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("discover.receivePatrolReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );

    for ns in ["devilCopy", "soaringCopy"] {
        assert_eq!(client.call_rpc(&format!("{ns}.getLoginInfo"), vec![], false).err, "ok");
        assert_eq!(client.call_rpc(&format!("{ns}.getInfo"), vec![], false).err, "ok");
        assert_eq!(
            client
                .call_rpc(&format!("{ns}.getCopy"), vec![Value::Int(1)], false)
                .err,
            "ok"
        );
        assert_eq!(
            client
                .call_rpc(&format!("{ns}.attack"), vec![Value::Int(1)], false)
                .err,
            "ok"
        );
        assert_eq!(
            client
                .call_rpc(&format!("{ns}.pass"), vec![Value::Int(1)], false)
                .err,
            "ok"
        );
        assert_eq!(
            client
                .call_rpc(&format!("{ns}.buyAttackTimes"), vec![Value::Int(1)], false)
                .err,
            "ok"
        );
    }

    // =========================================================================
    // 3. Arena, TimeMachineArena, Mail/TM Mail, Sign/SuperCheckin/Online/DailyTask/Achievements/EatChicken/ImperialCity
    // =========================================================================
    assert_eq!(client.call_rpc("arena.enterArena", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("arena.getRankList", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("arena.challenge", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("arena.buyChallengeTimes", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("arena.fetchChallengeNumReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("timeMachineArena.getArenaInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("timeMachineArena.challenge", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("tmArena.enterArena", vec![], false).err, "ok");

    // Mail & TM Mail / TM Reward
    let mlist = client.call_rpc(
        "mail.getMailBoxList",
        vec![Value::Int(0), Value::Int(50), Value::String("true".into())],
        false,
    );
    assert_eq!(mlist.err, "ok");
    assert_eq!(client.call_rpc("mail.drawAllMailItems", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc(
                "tmMail.getMailBoxList",
                vec![Value::Int(0), Value::Int(50), Value::String("true".into())],
                false,
            )
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("tmReward.getRewardList", vec![Value::Int(0), Value::Int(50)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc(
                "tmReward.giftByCode",
                vec![Value::String("GSERVERVIP888".into())],
                false,
            )
            .err,
        "ok"
    );

    // Daily Progression & City / ImperialCity
    assert_eq!(client.call_rpc("sign.getNormalInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("sign.gainNormalSignReward", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("superCheckin.getInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("superCheckin.sign", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("superCheckin.fetchReward", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("openOnline.getInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("achievements.getShowAchievementsPointReward", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("online.getOnlineInfo", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("online.recv", vec![Value::Int(1)], false).err,
        "ok"
    );
    assert_eq!(client.call_rpc("dailytask.getInfo", vec![], false).err, "ok");
    assert_eq!(
        client
            .call_rpc("dailytask.receiveTask", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("dailytask.receiveActivePoint", vec![Value::Int(20)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("dailytask.receiveAllTask", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("achievements.getShowAchievements", vec![], false).err,
        "ok"
    );
    assert_eq!(
        client
            .call_rpc("achievements.fetchPrize", vec![Value::Int(1)], false)
            .err,
        "ok"
    );
    assert_eq!(client.call_rpc("eatChicken.getInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("eatChicken.eat", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("city.enterShadowInfo", vec![], false).err, "ok");
    assert_eq!(client.call_rpc("imperialcity.getEnterInfo", vec![], false).err, "ok");
    assert_eq!(
        client.call_rpc("imperialcity.enter", vec![Value::Int(1)], false).err,
        "ok"
    );

    let init_raffle_sub = client
        .call_rpc("raffle.getInfo", vec![], false)
        .ret
        .get_field("raffle")
        .cloned()
        .expect("raffle sub-table");
    assert_eq!(
        client.call_rpc("raffle.multiRaffle", vec![Value::Int(2)], false).err,
        "ok"
    );
    let pre_raffle_sub = client
        .call_rpc("raffle.getInfo", vec![], false)
        .ret
        .get_field("raffle")
        .cloned()
        .expect("raffle sub-table after 10-pull");
    assert_ne!(
        pre_raffle_sub, init_raffle_sub,
        "expected tavern raffle sub-table to advance after multiRaffle"
    );

    // =========================================================================
    // 4. Pipelined Malformed AMF3 RPC Resilience (`tcp_server.rs`)
    // =========================================================================
    let _ = client.drain_pushes();
    let bad_lcpacket = gserver_protocol::frame::encode_c2s_lcpacket(&[0xFF, 0x00, 0x7F], false).unwrap();
    client.seq += 1;
    let pipe_cb = format!("user.getUser_{}", client.seq);
    let good_req = RpcRequest::new("user.getUser", vec![], &client.token, pipe_cb.clone());
    let good_lcpacket = good_req.to_lcpacket_frame(false).unwrap();
    let mut pipelined = Vec::with_capacity(bad_lcpacket.len() + good_lcpacket.len());
    pipelined.extend_from_slice(&bad_lcpacket);
    pipelined.extend_from_slice(&good_lcpacket);
    client
        .stream
        .write_all(&pipelined)
        .expect("write pipelined bad + good frames");
    client.stream.flush().unwrap();

    // Read pipelined `user.getUser` response — connection MUST NOT stall!
    let mut pre_restart_user: Option<Value> = None;
    let mut buf = [0u8; 4096];
    while pre_restart_user.is_none() {
        while let Some(frame) = client.rx.next_frame().unwrap() {
            let batch = decode_s2c_frame_batch(&frame).unwrap();
            for env in batch.items {
                if env.callback_name.as_deref() == Some(pipe_cb.as_str()) {
                    assert_eq!(env.err, "ok");
                    pre_restart_user = Some(env.ret);
                }
            }
        }
        if pre_restart_user.is_none() {
            let n = client.stream.read(&mut buf).expect("pipelined frame must not stall");
            assert!(n > 0);
            client.rx.feed(&buf[..n]);
        }
    }
    let pre_u = pre_restart_user.unwrap();
    let pre_tower = pre_u.get_field("tower_num").and_then(Value::as_i64).unwrap_or(0);
    let pre_soul = pre_u.get_field("hero_soul").and_then(Value::as_i64).unwrap_or(0);
    let pre_bag = client.call_rpc("bag.bagInfo", vec![], false).ret;
    let (pre_weap_gid, pre_weap_iid) =
        find_bag_entry_by_tid(&pre_bag, "arm", 112_001).expect("starter weapon in arm bag");
    assert!(
        (10_001..=740_000).contains(&pre_weap_gid),
        "B-08: weapon GID {pre_weap_gid} must lie inside BagData:getBagTypeByGid range"
    );

    // =========================================================================
    // 5. Cold WAL Restart (`B-08` Authentic GIDs + `B-19` `TavernRaffleEngine` State + Currencies)
    // =========================================================================
    drop(client);
    server.shutdown();

    let server2 = RunningServer::start(cfg).expect("cold restart server");
    let (mut cold_client, _, _) = wave2_login_and_enter(
        server2.http_addr,
        server2.tcp_addr,
        "w2_modes_gamma",
        1,
        "LữBốW2Play",
        Some(uid),
    );

    let post_raffle_sub = cold_client
        .call_rpc("raffle.getInfo", vec![], false)
        .ret
        .get_field("raffle")
        .cloned()
        .expect("raffle sub-table after cold restart");
    assert_eq!(
        post_raffle_sub, pre_raffle_sub,
        "B-19: TavernRaffleEngine raffle state must survive cold WAL restart"
    );

    let post_bag = cold_client.call_rpc("bag.bagInfo", vec![], false).ret;
    let (post_weap_gid, post_weap_iid) =
        find_bag_entry_by_tid(&post_bag, "arm", 112_001).expect("weapon after cold restart");
    assert_eq!(
        (post_weap_gid, post_weap_iid),
        (pre_weap_gid, pre_weap_iid),
        "B-08: authentic sub-bag GID & item_id must be 100% preserved across cold restart"
    );

    let post_u = cold_client.call_rpc("user.getUser", vec![], false).ret;
    assert_eq!(post_u.get_field("tower_num").and_then(Value::as_i64), Some(pre_tower));
    assert_eq!(post_u.get_field("hero_soul").and_then(Value::as_i64), Some(pre_soul));

    drop(cold_client);
    server2.shutdown();
}
