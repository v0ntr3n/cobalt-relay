use std::collections::HashSet;

use gserver_rpc_catalog::{
    amf3::{decode_amf3, encode_amf3, Amf3Value, AMF3_INT_MAX, AMF3_INT_MIN},
    bootstrap::{
        BagSnapshot, BootstrapPhase, BootstrapSession, CopyChapterSnapshot, CopyListSnapshot,
        EnterViewSnapshot, FormationSquad, HeroFormationSnapshot, UserProfile, VaConfig,
        BAG_GRID_OPEN_TIMES_DEFAULTS, BAG_GRID_START_DEFAULTS, BAG_SUBBAG_KEYS, BOOTSTRAP_SEQUENCE,
    },
    envelope::{RpcRequestEnvelope, RpcResponseEnvelope},
    net_const::{self, RpcDirection, ALL_NET_CONSTS, INLINE_RPC_LITERALS, NET_CONST_COUNT},
    packet::{babeltime_body_checksum, md5, ClientFrame, PacketError, ServerFrame},
    validator::validate_response_for_method,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[test]
fn net_const_catalog_contains_all_2107_entries_and_tm_mirror() {
    assert_eq!(ALL_NET_CONSTS.len(), 2107);
    assert_eq!(NET_CONST_COUNT, 2107);
    assert_eq!(net_const::tm::ALL_TM_NET_CONSTS.len(), 2021);

    let mut unique_names = HashSet::new();
    let mut empty_methods = Vec::new();
    for entry in ALL_NET_CONSTS {
        assert!(
            unique_names.insert(entry.const_name),
            "duplicate constant name {}",
            entry.const_name
        );
        if entry.method.is_empty() {
            empty_methods.push((entry.const_name, entry.line));
        }
    }
    // Forensic verification: line 5130 `CS_CHESS_TAKEOFF_EQUIP = ""` is the sole empty RPC literal in NetConst.lua
    assert_eq!(empty_methods, vec![("CS_CHESS_TAKEOFF_EQUIP", 5130)]);

    // Verify line 86 semicolon edge case (`CS_GET_BAG_DATA = "bag.bagInfo";`)
    let bag_entry = net_const::by_const_name("CS_GET_BAG_DATA").expect("CS_GET_BAG_DATA present");
    assert_eq!(bag_entry.method, "bag.bagInfo");
    assert_eq!(bag_entry.line, 86);
    assert_eq!(bag_entry.direction, RpcDirection::ClientToServer);

    // Verify actual vnsg2 RPCs vs commonly misguessed names
    assert_eq!(net_const::CS_USER_LOGIN, "user.login");
    assert_eq!(net_const::CS_GET_USER_BEFRO_LOGIN, "user.getUserBeforLogin");
    assert!(net_const::first_by_method("user.getUsers").is_none());
    assert_eq!(net_const::CS_OPENSWITCH_GETSWITCH, "switchfunc.getSwitch");
    assert_eq!(net_const::CS_USER_GET_VA_CONFIG, "user.getVaConfig");
    assert!(net_const::first_by_method("user.getUserConfig").is_none());
    assert_eq!(net_const::CS_FORMATION_GET, "formation.get");
    assert_eq!(
        net_const::CS_FORMATION_GET_POSITION_INFO,
        "formation.getPositionInfo"
    );
    assert!(net_const::first_by_method("formation.getSquad").is_none());

    // Verify all bootstrap RPCs are known in NetConst or inline RPC literals
    assert_eq!(INLINE_RPC_LITERALS.len(), 16);
    for spec in BOOTSTRAP_SEQUENCE {
        assert!(
            net_const::is_known_rpc(spec.method),
            "bootstrap method {} missing from catalog",
            spec.method
        );
        if spec.net_const_name != "<inline>" {
            let found =
                net_const::by_const_name(spec.net_const_name).expect("named NetConst exists");
            assert_eq!(found.method, spec.method);
        }
    }
}

#[test]
fn login_init_data_proxy_and_copy_proxy_fields_match_lua_parsedata_contracts() {
    let ts = 1_795_496_852_i64;

    // 1. UserData (`user.getUser`)
    let user_amf = UserProfile::default().to_get_user_amf3();
    let user_map = user_amf.as_map().expect("user map");
    assert_eq!(
        user_map.get("server_open_time").and_then(Amf3Value::as_str),
        Some("20260101000000")
    );
    assert_eq!(
        user_map
            .get("server_open_time")
            .and_then(Amf3Value::as_str)
            .map(str::len),
        Some(14)
    );
    assert_eq!(
        user_map.get("destiny").and_then(Amf3Value::as_i64),
        Some(-1)
    );
    assert!(user_map
        .get("execution_time")
        .and_then(Amf3Value::as_i64)
        .is_some());
    assert!(user_map
        .get("energy_time")
        .and_then(Amf3Value::as_i64)
        .is_some());
    assert!(user_map
        .get("gold_recharge_accum_all")
        .and_then(Amf3Value::as_i64)
        .is_some());
    validate_response_for_method(
        net_const::CS_GET_USER,
        &[],
        &RpcResponseEnvelope::ok("user.getUser_5", user_amf, ts, "tok"),
        true,
    )
    .expect("UserData valid");

    // 2. OpenSwitchData (`switchfunc.getSwitch`)
    let switch_ret = Amf3Value::Array(vec![
        Amf3Value::Int(1),
        Amf3Value::Int(5),
        Amf3Value::Int(17),
    ]);
    validate_response_for_method(
        net_const::CS_OPENSWITCH_GETSWITCH,
        &[],
        &RpcResponseEnvelope::ok("switchfunc.getSwitch_6", switch_ret, ts, "tok"),
        true,
    )
    .expect("OpenSwitchData valid");

    // 3. ConfigData (`user.getVaConfig`)
    let va_ret = VaConfig::default().to_amf3();
    assert_eq!(
        va_ret.get_field("guide_id").and_then(Amf3Value::as_i64),
        Some(240)
    );
    validate_response_for_method(
        net_const::CS_USER_GET_VA_CONFIG,
        &[],
        &RpcResponseEnvelope::ok("user.getVaConfig_7", va_ret, ts, "tok"),
        true,
    )
    .expect("ConfigData valid");

    // 4. FormationData (`formation.get`)
    let form_ret = FormationSquad::with_major_hero(10_000_001).to_amf3();
    assert_eq!(
        form_ret
            .get_field("formation")
            .and_then(Amf3Value::as_array)
            .map(<[Amf3Value]>::len),
        Some(6)
    );
    validate_response_for_method(
        net_const::CS_FORMATION_GET,
        &[],
        &RpcResponseEnvelope::ok("formation.get_8", form_ret, ts, "tok"),
        true,
    )
    .expect("FormationData valid");

    // 5. BagData (`bag.bagInfo`)
    let bag_ret = BagSnapshot::default().to_amf3();
    let bag_map = bag_ret.as_map().expect("bag map");
    assert_eq!(BAG_SUBBAG_KEYS.len(), 41);
    assert_eq!(BAG_GRID_START_DEFAULTS.len(), 29);
    assert_eq!(BAG_GRID_OPEN_TIMES_DEFAULTS.len(), 12);
    for &sub in BAG_SUBBAG_KEYS {
        assert!(bag_map.contains_key(sub), "missing bag sub-table {}", sub);
    }
    validate_response_for_method(
        net_const::CS_GET_BAG_DATA,
        &[Amf3Value::empty_map()],
        &RpcResponseEnvelope::ok("bag.bagInfo_10", bag_ret, ts, "tok"),
        true,
    )
    .expect("BagData valid");

    // 6. HeroFormationData (`formation.getPositionInfo`)
    let pos_ret = HeroFormationSnapshot::with_protagonist(10_000_001, 1, 10).to_amf3();
    validate_response_for_method(
        net_const::CS_FORMATION_GET_POSITION_INFO,
        &[],
        &RpcResponseEnvelope::ok("formation.getPositionInfo_11", pos_ret, ts, "tok"),
        true,
    )
    .expect("HeroFormationData valid");

    // 7. CopyData (`copy.getList`) & CopyInfoDataController (`copy.getCopy`) & EnterViewData (`smallgame.getInfo`)
    validate_response_for_method(
        net_const::CS_COPY_LIST,
        &[Amf3Value::Int(1)],
        &RpcResponseEnvelope::ok(
            "copy.getList_12",
            CopyListSnapshot::default().to_amf3(),
            ts,
            "tok",
        ),
        true,
    )
    .expect("CopyData valid");

    validate_response_for_method(
        net_const::CS_COPY_CHAPTERINFO,
        &[Amf3Value::Int(1001)],
        &RpcResponseEnvelope::ok(
            "copy.getCopy_13",
            CopyChapterSnapshot::default().to_amf3(),
            ts,
            "tok",
        ),
        true,
    )
    .expect("CopyChapterSnapshot valid");

    validate_response_for_method(
        net_const::CS_SMALLGAME_GETINFO,
        &[],
        &RpcResponseEnvelope::ok(
            "smallgame.getInfo_14",
            EnterViewSnapshot::default().to_amf3(),
            ts,
            "tok",
        ),
        true,
    )
    .expect("EnterViewData valid");
}

#[test]
fn rfc1321_md5_and_babeltime_lcpacket_framing_roundtrip() {
    assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        hex(&md5(b"message digest")),
        "f96b697d7cb7938d525a2f31aaf161d0"
    );

    // Empty body -> MD5("BabelTime")
    assert_eq!(babeltime_body_checksum(b""), md5(b"BabelTime"));

    // Verify rolling XOR formula for 3-byte payload [0x10, 0x35, 0x7A]:
    // "BabelTime" + [0x35 ^ 0x10, 0x7A ^ 0x35, 0x7A ^ 0x03]
    let sample = [0x10u8, 0x35, 0x7A];
    let mut manual = b"BabelTime".to_vec();
    manual.push(0x35 ^ 0x10);
    manual.push(0x7A ^ 0x35);
    manual.push(0x7A ^ 0x03);
    assert_eq!(babeltime_body_checksum(&sample), md5(&manual));

    let req = RpcRequestEnvelope::new_sequenced("user.getUser", vec![], "0", 2);
    let client_wire = req.to_client_frame().encode();
    assert_eq!(client_wire.len(), 24 + req.to_client_frame().body.len());

    let (decoded_req, used) =
        RpcRequestEnvelope::from_client_frame_bytes(&client_wire).expect("valid client frame");
    assert_eq!(used, client_wire.len());
    assert_eq!(decoded_req, req);

    // Tampering a single byte must trigger ChecksumMismatch
    let mut corrupted = client_wire.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0x01;
    assert!(matches!(
        ClientFrame::decode(&corrupted),
        Err(PacketError::ChecksumMismatch { .. })
    ));

    // Server 8-byte header roundtrip
    let dummy = RpcResponseEnvelope::dummy_push(1_795_496_852, "tok_sync");
    let srv_wire = dummy.to_server_frame().encode();
    let (srv_frame, srv_used) = ServerFrame::decode(&srv_wire).expect("valid server frame");
    assert_eq!(srv_used, srv_wire.len());
    assert!(!srv_frame.zipped);
    assert!(!srv_frame.multi);
    let (decoded_dummy, _) =
        RpcResponseEnvelope::from_server_frame_bytes(&srv_wire).expect("valid dummy envelope");
    assert_eq!(decoded_dummy.callback_name, "dummy");
    assert_eq!(decoded_dummy.err, "ok");
}

#[test]
fn amf3_lua_array_vs_map_and_u29_boundaries_roundtrip() {
    let empty_arr = encode_amf3(&Amf3Value::empty_array());
    assert_eq!(empty_arr, vec![0x09, 0x01, 0x01]);
    assert_eq!(
        decode_amf3(&empty_arr).expect("decode empty array"),
        Amf3Value::empty_array()
    );

    let empty_obj = encode_amf3(&Amf3Value::empty_map());
    assert_eq!(empty_obj, vec![0x0A, 0x0B, 0x01, 0x01]);
    assert_eq!(
        decode_amf3(&empty_obj).expect("decode empty map"),
        Amf3Value::empty_map()
    );

    let boundary = Amf3Value::Array(vec![
        Amf3Value::Int(AMF3_INT_MIN),
        Amf3Value::Int(-1),
        Amf3Value::Int(0),
        Amf3Value::Int(AMF3_INT_MAX),
        Amf3Value::Int(1_795_496_852),
        Amf3Value::String("Chúa Công GServer".to_owned()),
        Amf3Value::String("Chúa Công GServer".to_owned()),
    ]);
    let raw = encode_amf3(&boundary);
    let decoded = decode_amf3(&raw).expect("decode boundary array");
    let arr = decoded.as_array().expect("array");
    assert_eq!(arr[0].as_i64(), Some(AMF3_INT_MIN));
    assert_eq!(arr[1].as_i64(), Some(-1));
    assert_eq!(arr[2].as_i64(), Some(0));
    assert_eq!(arr[3].as_i64(), Some(AMF3_INT_MAX));
    assert_eq!(arr[4].as_i64(), Some(1_795_496_852));
    assert_eq!(arr[5].as_str(), Some("Chúa Công GServer"));
    assert_eq!(arr[6].as_str(), Some("Chúa Công GServer"));
}

#[test]
fn full_new_account_bootstrap_reaches_main_town_100_percent_over_wire() {
    let server_ts = 1_795_496_852_i64;
    let mut session = BootstrapSession::new_account(server_ts);
    let mut token = "0".to_owned();
    let mut m_id = 1_u64;

    let mut send_rpc = |session: &mut BootstrapSession,
                        method: &str,
                        args: Vec<Amf3Value>,
                        is_telemetry: bool|
     -> RpcResponseEnvelope {
        let req = if is_telemetry {
            RpcRequestEnvelope::new_telemetry(method, args, token.clone())
        } else {
            m_id += 1;
            RpcRequestEnvelope::new_sequenced(method, args, token.clone(), m_id)
        };

        let c2s_bytes = req.to_client_frame().encode();
        let (wire_req, _) =
            RpcRequestEnvelope::from_client_frame_bytes(&c2s_bytes).expect("decode C->S frame");

        let resp = session.handle_request(&wire_req);
        let s2c_bytes = resp.to_server_frame().encode();
        let (wire_resp, _) =
            RpcResponseEnvelope::from_server_frame_bytes(&s2c_bytes).expect("decode S->C frame");

        validate_response_for_method(method, &wire_req.args, &wire_resp, true)
            .unwrap_or_else(|v| panic!("schema violation on {}: {:?}", method, v));

        token = wire_resp.token.clone();
        wire_resp
    };

    // Stage A: TCP connect -> user.login -> user.getUserBeforLogin (empty -> CreateUserScene)
    let login_hash = Amf3Value::map_from([
        ("AMF", Amf3Value::Int(3)),
        ("pid", Amf3Value::string("636df6782106606d")),
        ("serverID", Amf3Value::string("game1")),
        ("canCreateUser", Amf3Value::Int(1)),
        ("canPay", Amf3Value::Int(1)),
        ("record", Amf3Value::empty_map()),
    ]);
    let r_login = send_rpc(
        &mut session,
        net_const::CS_USER_LOGIN,
        vec![
            login_hash,
            Amf3Value::string("uuid-vn-001"),
            Amf3Value::string("1.9.80"),
        ],
        false,
    );
    assert_eq!(r_login.callback_name, "user.login_2");

    let r_before_1 = send_rpc(
        &mut session,
        net_const::CS_GET_USER_BEFRO_LOGIN,
        vec![],
        false,
    );
    assert!(
        r_before_1.ret.get_field("uid").is_none(),
        "new account returns empty table without uid"
    );

    // CreateUserScene: random name -> stat -> createUser -> getUserBeforLogin -> userLogin
    let r_names = send_rpc(
        &mut session,
        net_const::CS_USER_GET_RANDOM_NAME,
        vec![Amf3Value::Int(20), Amf3Value::Int(1)],
        false,
    );
    let chosen_name = r_names
        .ret
        .as_array()
        .and_then(|a| a.last())
        .and_then(|e| e.get_field("name"))
        .and_then(Amf3Value::as_str)
        .expect("random name")
        .to_owned();

    let r_stat = send_rpc(&mut session, "user.stat", vec![Amf3Value::Int(1)], true);
    assert_eq!(r_stat.callback_name, "user.stat");

    send_rpc(
        &mut session,
        net_const::CS_USER_CREATEUSER,
        vec![Amf3Value::Int(2), Amf3Value::String(chosen_name.clone())],
        false,
    );

    let r_before_2 = send_rpc(
        &mut session,
        net_const::CS_GET_USER_BEFRO_LOGIN,
        vec![],
        false,
    );
    let uid = r_before_2
        .ret
        .get_field("uid")
        .and_then(Amf3Value::as_i64)
        .expect("created uid present");
    assert_eq!(
        r_before_2
            .ret
            .get_field("uname")
            .and_then(Amf3Value::as_str),
        Some(chosen_name.as_str())
    );

    let r_ulogin = send_rpc(
        &mut session,
        net_const::CS_USER_USERLOGIN,
        vec![Amf3Value::Int(uid)],
        false,
    );
    assert_eq!(r_ulogin.ret.as_str(), Some("ok"));

    // Stage B: LoginInitDataProxy + FestivalManager
    send_rpc(&mut session, net_const::CS_GET_USER, vec![], false);
    send_rpc(
        &mut session,
        net_const::CS_OPENSWITCH_GETSWITCH,
        vec![],
        false,
    );
    send_rpc(
        &mut session,
        net_const::CS_USER_GET_VA_CONFIG,
        vec![],
        false,
    );
    send_rpc(&mut session, net_const::CS_FORMATION_GET, vec![], false);
    send_rpc(
        &mut session,
        net_const::CS_ALL_ACTIVITY_GETACTIVITYINFO,
        vec![],
        false,
    );

    // Stage C: GuideManager:run() -> CopyProxy:essentialData() -> GuideManager:_dataAviable()
    send_rpc(
        &mut session,
        net_const::CS_GET_BAG_DATA,
        vec![Amf3Value::empty_map()],
        false,
    );
    send_rpc(
        &mut session,
        net_const::CS_FORMATION_GET_POSITION_INFO,
        vec![],
        false,
    );
    send_rpc(&mut session, "halidom.getInfo", vec![], false);
    send_rpc(
        &mut session,
        net_const::CS_COPY_LIST,
        vec![Amf3Value::Int(1)],
        false,
    );
    send_rpc(
        &mut session,
        net_const::CS_COPY_CHAPTERINFO,
        vec![Amf3Value::Int(1001)],
        false,
    );

    // Stage D: MainScene:run() -> MainProxy -> EnChatProxy -> ActSevenDayProxy -> MainTownBarrageProxy
    assert!(!session.main_town_ready_100_percent);
    send_rpc(&mut session, net_const::CS_SMALLGAME_GETINFO, vec![], false);
    send_rpc(
        &mut session,
        net_const::CS_REWARD_GETREWARDLIST,
        vec![Amf3Value::Int(0), Amf3Value::Int(100)],
        false,
    );
    send_rpc(&mut session, net_const::CS_CHAT_WORLD_GET, vec![], false);
    send_rpc(&mut session, net_const::CS_CHAT_GUILD_GET, vec![], false);
    send_rpc(
        &mut session,
        net_const::CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE,
        vec![],
        false,
    );
    send_rpc(&mut session, "barrage.get", vec![], false);
    send_rpc(
        &mut session,
        "barrage.enterRoom",
        vec![Amf3Value::Int(1)],
        false,
    );
    assert!(
        session.main_town_ready_100_percent,
        "MainTown must be initialized and MainLoading hidden at 100% on barrage.enterRoom callback"
    );
    send_rpc(
        &mut session,
        "barrage.getBarrageRecord",
        vec![Amf3Value::Int(1)],
        false,
    );

    // Stage E: Post-MainScene:init() background RPCs
    send_rpc(&mut session, net_const::CS_LORDWAR_GET_TOP, vec![], false);
    send_rpc(
        &mut session,
        net_const::CS_CROSSCLANBATTLE_GET_TOP,
        vec![],
        false,
    );
    send_rpc(
        &mut session,
        net_const::CS_CROSSCLANBATTLE_ROUNDINFO,
        vec![],
        false,
    );
    send_rpc(
        &mut session,
        net_const::CS_MILLIONQUIZ_GETINFO,
        vec![],
        false,
    );

    let phases: HashSet<BootstrapPhase> = BOOTSTRAP_SEQUENCE.iter().map(|s| s.phase).collect();
    assert_eq!(phases.len(), 5);
}

#[test]
fn validator_catches_every_critical_lua_nil_and_type_trap() {
    let ts = 1_795_496_852_i64;
    let mut session = BootstrapSession::existing_role(UserProfile::default(), ts);

    // 1. Trap: getUserBeforLogin returning 1-based array `[ { uid = 100001 } ]` instead of top-level map
    let bad_before = RpcResponseEnvelope::ok(
        "user.getUserBeforLogin_3",
        Amf3Value::Array(vec![UserProfile::default().to_before_login_amf3()]),
        ts,
        "tok",
    );
    assert!(validate_response_for_method(
        net_const::CS_GET_USER_BEFRO_LOGIN,
        &[],
        &bad_before,
        true
    )
    .is_err());

    // 2. Trap: user.getUser with Unix integer or short string in `server_open_time` instead of 14-char "YYYYMMDDHHMMSS"
    let mut bad_user = session.handle_request(&RpcRequestEnvelope::new_sequenced(
        net_const::CS_GET_USER,
        vec![],
        "tok",
        4,
    ));
    if let Amf3Value::Map(ref mut m) = bad_user.ret {
        m.insert(
            "server_open_time".to_owned(),
            Amf3Value::String("2026-01-01".to_owned()),
        );
    }
    let errs =
        validate_response_for_method(net_const::CS_GET_USER, &[], &bad_user, true).unwrap_err();
    assert!(errs.iter().any(|e| e.field_path == "ret.server_open_time"));

    // 3. Trap: bag.bagInfo missing `daily_use` or wrong request args `{}`
    let good_bag = session.handle_request(&RpcRequestEnvelope::new_sequenced(
        net_const::CS_GET_BAG_DATA,
        vec![Amf3Value::empty_map()],
        "tok",
        5,
    ));
    assert!(validate_response_for_method(
        net_const::CS_GET_BAG_DATA,
        &[], // wrong args: missing inner `{}`!
        &good_bag,
        true
    )
    .is_err());

    let mut bad_bag = good_bag.clone();
    if let Amf3Value::Map(ref mut m) = bad_bag.ret {
        m.remove("daily_use");
    }
    assert!(validate_response_for_method(
        net_const::CS_GET_BAG_DATA,
        &[Amf3Value::empty_map()],
        &bad_bag,
        true
    )
    .is_err());

    // 4. Trap: formation.getPositionInfo missing `va_fail_reward` or `bingfu`
    let mut bad_pos = session.handle_request(&RpcRequestEnvelope::new_sequenced(
        net_const::CS_FORMATION_GET_POSITION_INFO,
        vec![],
        "tok",
        6,
    ));
    if let Amf3Value::Map(ref mut m) = bad_pos.ret {
        m.remove("bingfu");
        if let Some(Amf3Value::Map(ref mut heros)) = m.get_mut("allHeros") {
            if let Some(Amf3Value::Map(ref mut h)) = heros.get_mut("10000001") {
                h.remove("va_fail_reward");
            }
        }
    }
    let pos_errs = validate_response_for_method(
        net_const::CS_FORMATION_GET_POSITION_INFO,
        &[],
        &bad_pos,
        true,
    )
    .unwrap_err();
    assert!(pos_errs
        .iter()
        .any(|e| e.field_path.ends_with("va_fail_reward")));
    assert!(pos_errs.iter().any(|e| e.field_path == "ret.bingfu"));

    // 5. Trap: copy.getCopy chapter 1001 with `100101.normal_star == 0` forces first-fight instead of MainTown 100%
    let mut bad_chap = session.handle_request(&RpcRequestEnvelope::new_sequenced(
        net_const::CS_COPY_CHAPTERINFO,
        vec![Amf3Value::Int(1001)],
        "tok",
        7,
    ));
    if let Amf3Value::Map(ref mut m) = bad_chap.ret {
        if let Some(Amf3Value::Map(ref mut vc)) = m.get_mut("va_copy") {
            if let Some(Amf3Value::Map(ref mut st)) = vc.get_mut("100101") {
                st.insert("normal_star".to_owned(), Amf3Value::Int(0));
            }
        }
    }
    assert!(validate_response_for_method(
        net_const::CS_COPY_CHAPTERINFO,
        &[Amf3Value::Int(1001)],
        &bad_chap,
        true
    )
    .is_err());

    // 6. Trap: user.getVaConfig with unfinished tutorial `guide_id = 10` when MainTown direct entry is expected
    let mut bad_va = session.handle_request(&RpcRequestEnvelope::new_sequenced(
        net_const::CS_USER_GET_VA_CONFIG,
        vec![],
        "tok",
        8,
    ));
    if let Amf3Value::Map(ref mut m) = bad_va.ret {
        m.insert("guide_id".to_owned(), Amf3Value::Int(10));
    }
    assert!(
        validate_response_for_method(net_const::CS_USER_GET_VA_CONFIG, &[], &bad_va, true).is_err()
    );
    // guide_id=360 (whose nextLeadId == 240, matching live_capture_filtered.log) IS valid!
    if let Amf3Value::Map(ref mut m) = bad_va.ret {
        m.insert("guide_id".to_owned(), Amf3Value::Int(360));
    }
    assert!(
        validate_response_for_method(net_const::CS_USER_GET_VA_CONFIG, &[], &bad_va, true).is_ok()
    );
}
