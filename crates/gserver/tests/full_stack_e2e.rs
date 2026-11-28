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

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;

use common::{
    assert_catalog_valid, complete_mapi_login_and_gethash, decode_zlib_amf3, raw_http_get,
    raw_http_get_text, run_bootstrap_from_step7, tcp_relogin_existing_role, E2eWireClient,
    TempDirGuard,
};
use gserver::{DaemonConfig, RunningServer};
use gserver_http::JsonValue;
use gserver_protocol::Value;
use gserver_rpc_catalog::BOOTSTRAP_SEQUENCE;

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
    let user_id = guest_json
        .get("userID")
        .and_then(JsonValue::as_str)
        .expect("guest userID");
    let session_id = guest_json
        .get("sessionID")
        .and_then(JsonValue::as_str)
        .expect("guest sessionID");

    complete_mapi_login_and_gethash(
        http_addr,
        user_id,
        session_id,
        device_id,
        1_795_500_500,
        verify_cookie_102,
    )
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

    let mut client_a = E2eWireClient::connect(tcp_addr);

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
    run_bootstrap_from_step7(&mut client_a);

    // 4. Connect Client B, create character, and verify real-time multiplayer town + chat + GM pushes!
    let (pid_b, hash_b, _, _) = http_authenticate_guest(http_addr, "win32_dev_bravo", false);
    let mut client_b = E2eWireClient::connect(tcp_addr);
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

    let pushes_a = client_a.wait_for_push("town.addPlayer");
    assert!(
        pushes_a
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("town.addPlayer")),
        "Client A must receive live `town.addPlayer` push when Client B enters town"
    );

    // Client B moves in town -> Client A receives `town.move`!
    let mv_b = client_b.call_rpc("town.move", vec![Value::Int(680), Value::Int(180)], false);
    assert_eq!(mv_b.err, "ok");
    let pushes_a_mv = client_a.wait_for_push("town.move");
    assert!(
        pushes_a_mv
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("town.move")),
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
    let pushes_b_town = client_b.wait_for_push("town.update");
    assert!(
        pushes_b_town
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("town.update")),
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
    let pushes_b_chat = client_b.wait_for_push("re.chat.getMsg");
    assert!(
        pushes_b_chat
            .iter()
            .any(|p| p.callback_name.as_deref() == Some("re.chat.getMsg")),
        "Client B must receive live `re.chat.getMsg` broadcast"
    );

    let last_msgs = client_b.call_rpc("chat.getWorldLastMsgs", vec![], false);
    let Value::Array { dense: blobs, .. } = &last_msgs.ret else {
        panic!("expected array of Zlib-compressed chat blobs");
    };
    assert!(!blobs.is_empty());
    let decoded_msg = decode_zlib_amf3(&blobs[0]);
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
    let rec_amf = decode_zlib_amf3(&rec.ret);
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
    let def_pushes = client_b.wait_for_push("re.arena.dataRefresh");
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

    let mut client_a2 =
        tcp_relogin_existing_role(server2.tcp_addr, &pid_a, "alpha", &hash_a2, uid_a);

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

    drop(client_a2);
    server2.shutdown();
}

#[test]
fn test_cobalt_env_map_precedence_and_coordinated_healthz() {
    let mut cfg = DaemonConfig::default();
    let mut env_map = BTreeMap::new();
    env_map.insert("COBALT_HTTP_ADDR".to_string(), "127.0.0.1:0".to_string());
    env_map.insert("COBALT_TCP_ADDR".to_string(), "127.0.0.1:0".to_string());
    env_map.insert("COBALT_PUBLIC_HOST".to_string(), "127.0.0.1".to_string());
    env_map.insert("COBALT_STRICT_ALL".to_string(), "true".to_string());
    env_map.insert("COBALT_STRICT_SIGN".to_string(), "false".to_string());
    cfg.apply_env_map(&env_map);

    assert_eq!(cfg.http_bind, "127.0.0.1:0");
    assert_eq!(cfg.tcp_bind, "127.0.0.1:0");
    assert!(cfg.strict_rpc);
    assert!(!cfg.strict_sign);

    let tmp = TempDirGuard::new("cobalt_hz");
    cfg.data_dir = tmp.path.clone();

    let srv = RunningServer::start(cfg).expect("start unified RunningServer");
    let expected_tcp = srv.tcp_addr.to_string();

    let (status, _, body) = raw_http_get_text(srv.http_addr, "/healthz", &[]);
    assert_eq!(status, 200, "status={status}");
    assert!(body.contains(r#""ready":true"#), "body={body}");
    assert!(body.contains(r#""mode":"unified_http_tcp""#), "body={body}");
    assert!(body.contains(&expected_tcp), "body={body}");

    srv.shutdown();
}
