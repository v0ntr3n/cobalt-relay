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
    assert_eq!(
        expand_resp.ret.get_field("grid_max").and_then(Value::as_i64),
        Some(205)
    );

    // Inspect bagInfo to locate gids for `22102` (stamina pill) and `16012` (normal item)
    let bag_info = client_a.call_rpc("bag.bagInfo", vec![], false).ret;
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
        use_pill.ret.get_field("remaining").and_then(Value::as_i64),
        Some(18)
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
    let crafted_arm_gid = compose_arm
        .ret
        .get_field("gid")
        .and_then(Value::as_i64)
        .unwrap() as i32;
    assert!((20_001..=30_000).contains(&crafted_arm_gid));

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
    assert_eq!(
        auto_rf
            .ret
            .get_field("reinforce_level")
            .and_then(Value::as_i64),
        Some(6)
    );
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

    assert_eq!(client_a.call_rpc("copy.attack", vec![Value::Int(1001), Value::Int(100_105)], false).err, "ok");
    assert_eq!(
        client_a.call_rpc("copy.pass", vec![Value::Int(1001), Value::Int(100_105), Value::empty_array()], false).err,
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
    assert_eq!(parse_copper(&mid_user) as i64, pre_copper + 1750 + 2750);
    assert_eq!(mid_user.get_field("hero_soul").and_then(Value::as_i64), Some(pre_soul + 60 + 100));
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
        cold_copy_chap.get_field("va_copy").and_then(|vc| vc.get_field("100105")).and_then(|s| s.get_field("normal_star")).and_then(Value::as_i64),
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
