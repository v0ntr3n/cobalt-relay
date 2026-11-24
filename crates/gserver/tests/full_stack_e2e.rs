#![forbid(unsafe_code)]
//! End-to-End full-stack integration test suite for `gserver` exercising:
//! 1. Live HTTP MAPI / Platform Guest Auth + Cookie Session (`errornu="102"` enforcement + signed URLs).
//! 2. Complete 28-step `MainTown` 100% bootstrap over real TCP `LCPacket` (24B C2S -> 8B S2C),
//!    validating every response against `gserver_rpc_catalog::validate_response_for_method(..., true)`.
//! 3. Live 2-client concurrent multiplayer (`city.enter`, `town.addPlayer`, `city.move`, `town.move`,
//!    GM slash commands `/level`, `/vip`, `/gold`, `/hero`, `/item`, `/mail`, `town.update`,
//!    `chat.sendWorld`, `re.chat.getMsg`, and Zlib-compressed `chat.getWorldLastMsgs` history).
//! 4. Full gameplay loop (`reward.*`, `raffle.multiRaffle`, `shop.*`, `copy.*` + `battle.getRecord`,
//!    `arena.enterArena` + `arena.challenge` with live defender `re.arena.dataRefresh` push).
//! 5. Full server shutdown & restart recovery from CRC32 WAL + atomic JSON (`PlayerStore`).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gserver_http::{parse_json, sign_mapi_url, JsonValue, VNSG2_MAPI_APP_KEY};
use gserver_protocol::{
    amf3, decode_s2c_frame_batch, zlib_decompress, RpcRequest, S2cEnvelope, ServerRxDecoder, Value,
};
use gserver_rpc_catalog::{
    validate_response_for_method, Amf3Value as CatalogAmf3, RpcResponseEnvelope, BOOTSTRAP_SEQUENCE,
};
use gserver::bridge::protocol_to_catalog_amf3;
use gserver::{DaemonConfig, RunningServer};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        let id = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gserver_full_stack_{label}_{}_{}",
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

/// Minimal raw HTTP/1.1 client for testing `gserver-http` over loopback TCP.
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

/// Simulated GServer (`vnsg2`) C++ / Lua TCP client speaking 24-byte `LCPacket` C2S and 8-byte S2C.
struct SimulatedGameClient {
    stream: TcpStream,
    rx: ServerRxDecoder,
    seq: u64,
    token: String,
    queued_pushes: Vec<S2cEnvelope>,
}

impl SimulatedGameClient {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("connect TCP game server");
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(800)))
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

    fn drain_pushes(&mut self) -> Vec<S2cEnvelope> {
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
            .set_read_timeout(Some(Duration::from_millis(800)));
        std::mem::take(&mut self.queued_pushes)
    }
}

/// Performs the full HTTP SDK guest auth + `/phone/login` + `/phone/gethash` handshake.
fn http_authenticate_guest(
    http_addr: SocketAddr,
    device_id: &str,
    verify_cookie_102: bool,
) -> (String, String, String, u16) {
    let (_, _, guest_json) = raw_http_get(
        http_addr,
        &format!("/api/login/createUser/guest?deviceID={device_id}&gameID=GCLIENT"),
        &[],
    );
    let user_id = guest_json.get("userID").and_then(JsonValue::as_str).expect("guest userID");
    let session_id = guest_json
        .get("sessionID")
        .and_then(JsonValue::as_str)
        .expect("guest sessionID");

    let login_url = sign_mapi_url(
        &format!(
            "http://{http_addr}/phone/login/?pl=android_vnsg2&gn=vnsg2&os=win32&userid={user_id}&sessionid={session_id}"
        ),
        1_795_500_500,
        VNSG2_MAPI_APP_KEY,
    );
    let login_target = login_url
        .strip_prefix(&format!("http://{http_addr}"))
        .unwrap();
    let (status, cookies, login_json) = raw_http_get(http_addr, login_target, &[]);
    assert_eq!(status, 200);
    assert!(!cookies.is_empty(), "/phone/login must issue session cookies");
    let pid = login_json
        .get("pid")
        .and_then(JsonValue::as_scalar_string)
        .expect("pid in /phone/login");

    let gethash_url = sign_mapi_url(
        &format!(
            "http://{http_addr}/phone/gethash/?pl=android_vnsg2&gn=vnsg2&pid={pid}&group_id=2900161&uuid=uuid_{device_id}"
        ),
        1_795_500_501,
        VNSG2_MAPI_APP_KEY,
    );
    let gethash_target = gethash_url
        .strip_prefix(&format!("http://{http_addr}"))
        .unwrap();

    if verify_cookie_102 {
        // Without cookies, strict_session MUST return `errornu = "102"`!
        let (_, _, err_json) = raw_http_get(http_addr, gethash_target, &[]);
        assert_eq!(
            err_json.get("errornu").and_then(JsonValue::as_str),
            Some("102"),
            "missing cookie on /phone/gethash must return errornu='102'"
        );
    }

    let (_, _, hash_json) = raw_http_get(http_addr, gethash_target, &cookies);
    assert_eq!(hash_json.get("errornu").and_then(JsonValue::as_str), Some("0"));
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
    let port = hash_json
        .get("port")
        .and_then(JsonValue::as_u64)
        .unwrap() as u16;

    (pid, hash, host, port)
}

fn assert_catalog_valid(method: &str, args: &[Value], env: &S2cEnvelope) {
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

#[test]
fn full_stack_bootstrap_multiplayer_gm_gameplay_and_restart_recovery() {
    let tmp = TempDirGuard::new("e2e");
    let cfg = DaemonConfig {
        http_bind: "127.0.0.1:0".to_string(),
        tcp_bind: "127.0.0.1:0".to_string(),
        public_host: "127.0.0.1".to_string(),
        data_dir: tmp.path.clone(),
        compress_s2c_threshold: 256,
        ..DaemonConfig::default()
    };

    let server = RunningServer::start(cfg.clone()).expect("start RunningServer");
    let http_addr = server.http_addr;
    let tcp_addr = server.tcp_addr;

    // 1. Check /phone/getversion & /phone/serverlist
    let (st_ver, _, ver_json) = raw_http_get(
        http_addr,
        "/phone/getversion?pl=android_vnsg2&gn=vnsg2&os=win32&ResVer=1.8.60&ScriptVer=1.8.68",
        &[],
    );
    assert_eq!(st_ver, 200);
    assert_eq!(ver_json.get("error_id").and_then(JsonValue::as_i64), Some(500));

    let (_, _, sl_json) = raw_http_get(
        http_addr,
        "/phone/serverlist/?pl=android_vnsg2&gn=vnsg2",
        &[],
    );
    let hot_port = sl_json
        .get("root")
        .and_then(|r| r.get("item"))
        .and_then(JsonValue::as_array)
        .unwrap()[0]
        .get("port")
        .and_then(JsonValue::as_u64)
        .unwrap() as u16;
    assert_eq!(hot_port, tcp_addr.port(), "serverlist port must match live TCP port");

    // 2. Authenticate Client A via HTTP SDK + verify cookie `102` + invalid hash rejection
    let (pid_a, hash_a, _host_a, port_a) =
        http_authenticate_guest(http_addr, "win32_dev_alpha", true);
    assert_eq!(port_a, tcp_addr.port());

    let mut client_a = SimulatedGameClient::connect(tcp_addr);

    // Negative check: forged TCP login hash is rejected
    let bad_login = client_a.call_rpc(
        "user.login",
        vec![Value::map([
            ("pid", Value::String(pid_a.clone())),
            ("qid", Value::String(pid_a.clone())),
            ("hash", Value::String("forged_bad_hash".into())),
            ("serverID", Value::String("game2900161".into())),
        ])],
        false,
    );
    assert_eq!(bad_login.err, "invalid_login_hash");

    // 3. Execute ALL 28 steps of `BOOTSTRAP_SEQUENCE` for Client A & validate Lua invariants!
    assert_eq!(BOOTSTRAP_SEQUENCE.len(), 28);

    let login_args = vec![
        Value::map([
            ("AMF", Value::Int(3)),
            ("pid", Value::String(pid_a.clone())),
            ("qid", Value::String(pid_a.clone())),
            ("hash", Value::String(hash_a.clone())),
            ("serverID", Value::String("game2900161".into())),
        ]),
        Value::String("uuid_alpha".into()),
        Value::String("1.8.68".into()),
    ];
    let env = client_a.call_rpc("user.login", login_args.clone(), true);
    assert_catalog_valid("user.login", &login_args, &env);

    // First `getUserBeforLogin` on fresh account -> empty table `{}`
    let env_b4_empty = client_a.call_rpc("user.getUserBeforLogin", vec![], false);
    assert_catalog_valid("user.getUserBeforLogin", &[], &env_b4_empty);

    // Random names + telemetry + character creation (`utid = 1`, `"TriệuVânVIP"`)
    let rn_args = vec![Value::Int(10), Value::Int(1)];
    let env_rn = client_a.call_rpc("user.getRandomName", rn_args.clone(), false);
    assert_catalog_valid("user.getRandomName", &rn_args, &env_rn);

    let env_stat = client_a.call_rpc("user.stat", vec![Value::Int(1)], false);
    assert_eq!(env_stat.err, "ok");

    let create_args = vec![Value::Int(1), Value::String("TriệuVânVIP".into())];
    let env_create = client_a.call_rpc("user.createUser", create_args.clone(), true);
    assert_catalog_valid("user.createUser", &create_args, &env_create);

    // Second `getUserBeforLogin` -> returns `{ uid, uname, utid, level }`!
    let env_b4 = client_a.call_rpc("user.getUserBeforLogin", vec![], false);
    assert_catalog_valid("user.getUserBeforLogin", &[], &env_b4);
    let uid_a = env_b4
        .ret
        .get_field("uid")
        .and_then(Value::as_i64)
        .expect("numeric uid in getUserBeforLogin") as u64;

    let ul_args = vec![Value::Int(uid_a as i32)];
    let env_ul = client_a.call_rpc("user.userLogin", ul_args.clone(), false);
    assert_catalog_valid("user.userLogin", &ul_args, &env_ul);

    // Run every remaining Stage B/C/D/E bootstrap RPC and validate every single one!
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
        let compress_c2s = spec.step_order % 2 == 0;
        let resp = client_a.call_rpc(spec.method, args.clone(), compress_c2s);
        assert_catalog_valid(spec.method, &args, &resp);
    }

    // 4. Connect Client B, create character, and verify real-time multiplayer town + chat + GM pushes!
    let (pid_b, hash_b, _, _) = http_authenticate_guest(http_addr, "win32_dev_bravo", false);
    let mut client_b = SimulatedGameClient::connect(tcp_addr);
    client_b.call_rpc(
        "user.login",
        vec![Value::map([
            ("pid", Value::String(pid_b)),
            ("qid", Value::String("bravo".into())),
            ("hash", Value::String(hash_b)),
            ("serverID", Value::String("game2900161".into())),
        ])],
        false,
    );
    client_b.call_rpc(
        "user.createUser",
        vec![Value::Int(2), Value::String("ĐiêuThuyền".into())],
        false,
    );
    let b4_b = client_b.call_rpc("user.getUserBeforLogin", vec![], false);
    let uid_b = b4_b.ret.get_field("uid").and_then(Value::as_i64).unwrap() as u64;
    client_b.call_rpc("user.userLogin", vec![Value::Int(uid_b as i32)], false);

    // Client A enters town 1, then Client B enters town 1 -> Client A receives `town.addPlayer`!
    let enter_a = client_a.call_rpc("city.enter", vec![Value::Int(1)], false);
    assert_eq!(enter_a.err, "ok");
    let enter_b = client_b.call_rpc("city.enter", vec![Value::Int(1)], false);
    assert_eq!(enter_b.err, "ok");

    let pushes_a = client_a.drain_pushes();
    assert!(
        pushes_a.iter().any(|p| p.callback_name.as_deref() == Some("town.addPlayer")),
        "Client A must receive live `town.addPlayer` push when Client B enters town"
    );

    // Client B moves in town -> Client A receives `town.move`!
    let mv_b = client_b.call_rpc("town.move", vec![Value::Int(680), Value::Int(180)], false);
    assert_eq!(mv_b.err, "ok");
    let pushes_a_mv = client_a.drain_pushes();
    assert!(
        pushes_a_mv.iter().any(|p| p.callback_name.as_deref() == Some("town.move")),
        "Client A must receive live `town.move` push"
    );

    // Client A runs GM commands `/level 55`, `/vip 12`, `/gold 50000`, `/hero 20008`, `/item 16012 10`
    for cmd in [
        "/level 55",
        "/vip 12",
        "/gold 50000",
        "/hero 20008",
        "/item 16012 10",
    ] {
        let gm_resp = client_a.call_rpc(
            "chat.sendWorld",
            vec![Value::Int(1), Value::String(cmd.into())],
            false,
        );
        assert_eq!(gm_resp.err, "ok");
    }
    let pushes_b_town = client_b.drain_pushes();
    assert!(
        pushes_b_town.iter().any(|p| p.callback_name.as_deref() == Some("town.update")),
        "Client B must receive `town.update` when Client A upgrades level/VIP"
    );

    // Client A sends world chat -> Client B receives `re.chat.getMsg`, and `chat.getWorldLastMsgs`
    // returns Zlib-compressed AMF3 byte blobs (`GameUtil:unzipData`)!
    let chat_send = client_a.call_rpc(
        "chat.sendWorld",
        vec![Value::Int(1), Value::String("Anh em tập hợp công thành!".into())],
        true,
    );
    assert_eq!(chat_send.err, "ok");
    let pushes_b_chat = client_b.drain_pushes();
    assert!(
        pushes_b_chat.iter().any(|p| p.callback_name.as_deref() == Some("re.chat.getMsg")),
        "Client B must receive live `re.chat.getMsg` broadcast"
    );

    let last_msgs = client_b.call_rpc("chat.getWorldLastMsgs", vec![], false);
    let Value::Array { dense: blobs, .. } = &last_msgs.ret else {
        panic!("expected array of Zlib-compressed chat blobs");
    };
    assert!(!blobs.is_empty());
    let compressed_blob = blobs[0].as_bytes().expect("ByteString blob");
    let decompressed_blob = zlib_decompress(compressed_blob).expect("RFC 1950 Zlib chat blob");
    let (decoded_msg, _) = amf3::decode(&decompressed_blob).expect("AMF3 chat record");
    assert_eq!(
        decoded_msg
            .get_field("message")
            .and_then(|m| m.get_field("message"))
            .and_then(Value::as_str),
        Some("Anh em tập hợp công thành!")
    );

    // 5. Full Gameplay Loop: Gift Code + Reward Center + Tavern 10-Pull + Shop + PvE Copy + Arena!
    let gift = client_a.call_rpc(
        "reward.giftByCode",
        vec![Value::String("GSERVERVIP888".into())],
        false,
    );
    assert_eq!(gift.err, "ok");

    let rlist = client_a.call_rpc(
        "reward.getRewardList",
        vec![Value::Int(0), Value::Int(50)],
        false,
    );
    if let Value::Array { dense, .. } = &rlist.ret {
        assert!(!dense.is_empty(), "welcome reward bundle must be present");
        let rid = dense[0].get_field("rid").and_then(Value::as_i64).unwrap();
        let claim = client_a.call_rpc(
            "reward.receiveByRidArr",
            vec![Value::dense_array(vec![Value::Int(rid as i32)])],
            false,
        );
        assert_eq!(claim.err, "ok");
    }

    let raffle10 = client_a.call_rpc("raffle.multiRaffle", vec![Value::Int(2)], false);
    assert_eq!(raffle10.err, "ok");

    let shop_info = client_a.call_rpc("shop.getInfo", vec![Value::Int(1)], false);
    let gk = shop_info
        .ret
        .get_field("goods_key")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let shop_buy = client_a.call_rpc(
        "shop.buy",
        vec![
            Value::Int(1),
            Value::String(gk),
            Value::Int(2),
            Value::Int(102),
            Value::Int(1),
        ],
        false,
    );
    assert_eq!(shop_buy.err, "ok");

    let atk = client_a.call_rpc(
        "copy.attack",
        vec![Value::Int(1001), Value::Int(100101)],
        false,
    );
    assert_eq!(atk.err, "ok");
    let brid = atk.ret.get_field("brid").and_then(Value::as_i64).unwrap();
    let pass = client_a.call_rpc(
        "copy.pass",
        vec![Value::Int(1001), Value::Int(100101), Value::empty_array()],
        false,
    );
    assert_eq!(pass.err, "ok");

    let rec = client_a.call_rpc("battle.getRecord", vec![Value::Int(brid as i32)], false);
    let rec_zipped = rec.ret.as_bytes().expect("Zlib-compressed battle record");
    let rec_raw = zlib_decompress(rec_zipped).expect("unzip battle record");
    let (rec_amf, _) = amf3::decode(&rec_raw).expect("decode battle record AMF3");
    assert_eq!(rec_amf.get_field("appraisal").and_then(Value::as_i64), Some(50));

    // Arena: Client B registers first (pos 16), Client A registers second (pos 17), Client A challenges Client B!
    let _ = client_b.call_rpc("arena.enterArena", vec![], false);
    let _ = client_a.call_rpc("arena.enterArena", vec![], false);
    let _ = client_b.drain_pushes();
    let ch = client_a.call_rpc(
        "arena.challenge",
        vec![
            Value::Int(17),
            Value::Int(16),
            Value::Int(0),
            Value::String("ĐiêuThuyền".into()),
        ],
        false,
    );
    assert_eq!(ch.err, "ok");
    assert_eq!(ch.ret.get_field("position").and_then(Value::as_i64), Some(16));
    let def_pushes = client_b.drain_pushes();
    assert!(
        def_pushes
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("re.arena.dataRefresh")),
        "Defender Client B must receive `re.arena.dataRefresh` push"
    );

    // Snapshot Client A's state before shutting down the server
    let pre_shutdown_user = client_a.call_rpc("user.getUser", vec![], false);
    let pre_gold = pre_shutdown_user
        .ret
        .get_field("gold")
        .and_then(Value::as_i64)
        .unwrap();
    assert_eq!(
        pre_shutdown_user.ret.get_field("level").and_then(Value::as_i64),
        Some(55)
    );
    assert_eq!(
        pre_shutdown_user.ret.get_field("vip").and_then(Value::as_i64),
        Some(12)
    );

    drop(client_a);
    drop(client_b);
    server.shutdown();

    // 6. Restart a brand-new `RunningServer` over the same `data_dir` and verify 100% state recovery!
    let server2 = RunningServer::start(cfg).expect("restart RunningServer from WAL + snapshots");
    let (_, hash_a2, _, _) = http_authenticate_guest(server2.http_addr, "win32_dev_alpha", false);

    // Check `/phone/loginrole/` with `pid` includes persisted Lv.55 VIP 12 role summary!
    let (_, _, lr2) = raw_http_get(
        server2.http_addr,
        &format!("/phone/loginrole/?pl=android_vnsg2&gn=vnsg2&pid={pid_a}"),
        &[],
    );
    let roles = lr2
        .get("root")
        .and_then(|r| r.get("role"))
        .and_then(JsonValue::as_array)
        .unwrap();
    assert!(
        roles
            .iter()
            .any(|r| r.get("level").and_then(JsonValue::as_i64) == Some(55)
                && r.get("vip").and_then(JsonValue::as_i64) == Some(12)),
        "restarted MAPI serverlist must expose persisted Lv.55 VIP 12 character"
    );

    let mut client_a2 = SimulatedGameClient::connect(server2.tcp_addr);
    client_a2.call_rpc(
        "user.login",
        vec![Value::map([
            ("pid", Value::String(pid_a)),
            ("qid", Value::String("alpha".into())),
            ("hash", Value::String(hash_a2)),
            ("serverID", Value::String("game2900161".into())),
        ])],
        false,
    );
    let ul2 = client_a2.call_rpc("user.userLogin", vec![Value::Int(uid_a as i32)], false);
    assert_eq!(ul2.ret.as_str(), Some("ok"));

    let post_restart_user = client_a2.call_rpc("user.getUser", vec![], false);
    assert_eq!(
        post_restart_user.ret.get_field("level").and_then(Value::as_i64),
        Some(55)
    );
    assert_eq!(
        post_restart_user.ret.get_field("vip").and_then(Value::as_i64),
        Some(12)
    );
    assert_eq!(
        post_restart_user.ret.get_field("gold").and_then(Value::as_i64),
        Some(pre_gold)
    );

    let pos_after = client_a2.call_rpc("formation.getPositionInfo", vec![], false);
    let Value::Object(pos_obj) = &pos_after.ret else {
        panic!("expected PositionInfo object");
    };
    let all_heros = pos_obj.get("allHeros").expect("allHeros map");
    let mut found_gm_hero_20008 = false;
    if let Value::Object(hm) = all_heros {
        for (_, hval) in hm.iter() {
            if hval.get_field("htid").and_then(Value::as_i64) == Some(20008) {
                found_gm_hero_20008 = true;
            }
        }
    }
    assert!(
        found_gm_hero_20008,
        "GM-summoned Red hero 20008 + raffle heroes must survive full server restart"
    );

    // Keep unused BTreeMap check clean
    let _: BTreeMap<u8, u8> = BTreeMap::new();

    drop(client_a2);
    server2.shutdown();
}
