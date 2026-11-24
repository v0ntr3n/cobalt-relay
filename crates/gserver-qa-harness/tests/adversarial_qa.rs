#![forbid(unsafe_code)]

//! Comprehensive adversarial QA, edge-case regression, deterministic PRNG fuzzing,
//! hostile TCP framing, and 32-thread concurrent client simulation suite for `gserver-qa-harness`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use gserver_qa_harness::amf3::{
    audit_lua_client_hazards, encode_buggy_truncated_u29_integer, write_u29, Amf3Decoder,
    Amf3Encoder, Amf3Error, Amf3Value, LuaClientHazard, AMF3_INT_MAX, AMF3_INT_MIN, MARKER_ARRAY,
    MARKER_BYTE_ARRAY, MARKER_DOUBLE, MARKER_INTEGER, MARKER_OBJECT, MARKER_STRING,
    MAX_RECURSION_DEPTH,
};
use gserver_qa_harness::crypto::{
    bt_decrypt_asset, bt_encrypt_asset, compute_babeltime_md5, md5_hex,
    parse_tolerant_query_params, sign_platform_url, sort_url_params, verify_platform_url_signature,
    VNSG2_LIVE_SIGN_KEY,
};
use gserver_qa_harness::framing::{
    build_ping_response, build_rpc_response, encode_client_lc_packet, encode_server_multi_frame,
    encode_server_raw_frame, encode_server_single_frame, is_framed_lc_packet, FrameError,
    ServerRxStreamDecoder, Win32ClientSimulator, CLIENT_LC_HEADER_LEN, MAX_SERVER_FRAME_BODY_LEN,
    SERVER_RX_HEADER_LEN,
};
use gserver_qa_harness::fuzzer::{run_deterministic_fuzz_campaign, DEFAULT_FUZZ_SEED};
use gserver_qa_harness::http_gateway::{
    route_sdk_http_request, HttpGatewayConfig, HttpGatewayMetrics, HttpSdkGateway,
};
use gserver_qa_harness::server::{GameServerConfig, GameTcpServer};
use gserver_qa_harness::zlib::{
    adler32, craft_zlib_zero_bomb, zlib_compress, zlib_decompress_bounded, ZlibError,
};

// ============================================================================
// 1. AMF3 U29 Signed Boundaries, Negative 4-Byte Encoding & Timestamp Promotion
// ============================================================================

#[test]
fn test_u29_exact_boundaries_and_auto_double_promotion() {
    // Exact U29 wire byte expectations verified against `Amf3Module.cpp:92-109, 156-169, 704-715`:
    let u29_cases: &[(i32, &[u8])] = &[
        // Minimum 29-bit signed int: -268,435,456 -> 0x1000_0000 -> 4-byte U29
        (-268_435_456, &[MARKER_INTEGER, 0xC0, 0x80, 0x80, 0x00]),
        // -1 -> 0x1FFF_FFFF -> 4-byte U29 (ALL negative U29s require 4 wire bytes!)
        (-1, &[MARKER_INTEGER, 0xFF, 0xFF, 0xFF, 0xFF]),
        // 1-byte U29 bounds (0..=127)
        (0, &[MARKER_INTEGER, 0x00]),
        (127, &[MARKER_INTEGER, 0x7F]),
        // 2-byte U29 bounds (128..=16,383)
        (128, &[MARKER_INTEGER, 0x81, 0x00]),
        (16_383, &[MARKER_INTEGER, 0xFF, 0x7F]),
        // 3-byte U29 bounds (16,384..=2,097,151)
        (16_384, &[MARKER_INTEGER, 0x81, 0x80, 0x00]),
        (2_097_151, &[MARKER_INTEGER, 0xFF, 0xFF, 0x7F]),
        // 4-byte positive U29 bounds (2,097,152..=268,435_455)
        (2_097_152, &[MARKER_INTEGER, 0x80, 0xC0, 0x80, 0x00]),
        (268_435_455, &[MARKER_INTEGER, 0xBF, 0xFF, 0xFF, 0xFF]),
    ];

    for &(val, expected_wire) in u29_cases {
        let amf = Amf3Value::from_i64_auto(val as i64);
        assert_eq!(amf, Amf3Value::Integer(val));
        let wire = Amf3Encoder::encode(&amf).expect("must encode boundary U29");
        assert_eq!(
            wire.as_slice(),
            expected_wire,
            "U29 wire mismatch for {}",
            val
        );
        let decoded = Amf3Decoder::decode(&wire).expect("must decode boundary U29");
        assert_eq!(decoded, Amf3Value::Integer(val));
    }

    // Automatic promotion to `0x05 Double` outside `-268_435_456..=268_435_455`:
    let double_promoted_cases: &[i64] = &[
        (AMF3_INT_MIN as i64) - 1, // -268,435,457
        (AMF3_INT_MAX as i64) + 1, // +268,435,456
        1_667_530_800,             // mergeServerTime from live_capture_filtered.log:76
        1_795_500_450,             // live vnsg2 server timestamp from live_capture_filtered.log:76
        1_879_048_192,             // 0x7000_0000 (bit 28 == 1 -> would sign-extend to -268,435,456 if U29!)
        -1_795_500_450,
    ];

    for &large in double_promoted_cases {
        let amf = Amf3Value::from_i64_auto(large);
        assert_eq!(amf, Amf3Value::Double(large as f64));
        let wire = Amf3Encoder::encode(&amf).expect("must encode promoted Double");
        assert_eq!(wire[0], MARKER_DOUBLE);
        assert_eq!(wire.len(), 9);
        assert_eq!(&wire[1..], &(large as f64).to_be_bytes());
        let decoded = Amf3Decoder::decode(&wire).expect("must decode Double");
        assert_eq!(decoded.as_i64(), Some(large));
    }

    // Even if someone passes `Amf3Value::Integer(1_795_500_450)` directly, `Amf3Encoder` auto-promotes!
    let forced_wire =
        Amf3Encoder::encode(&Amf3Value::Integer(1_795_500_450)).expect("must auto-promote");
    assert_eq!(forced_wire[0], MARKER_DOUBLE);
    assert_eq!(
        Amf3Decoder::decode(&forced_wire).unwrap().as_i64(),
        Some(1_795_500_450)
    );

    // `-0.0` preservation (`Amf3Module.cpp:160-161`)
    let neg_zero = Amf3Value::from_lua_number(-0.0);
    let nz_wire = Amf3Encoder::encode(&neg_zero).unwrap();
    assert_eq!(nz_wire[0], MARKER_DOUBLE);
    if let Amf3Value::Double(d) = Amf3Decoder::decode(&nz_wire).unwrap() {
        assert!(d == 0.0 && d.is_sign_negative());
    } else {
        panic!("expected Double(-0.0)");
    }

    // Demonstrate the exact timestamp corruption bug when a buggy server masks `1_795_500_450` (`0x6B05_29A2`)
    // or `0x7000_0000` into U29:
    // `0x6B05_29A2 & 0x1FFF_FFFF == 0x0B05_29A2 == 184,887,714` (loses 1,610,612,736 seconds!)
    let buggy_ts = encode_buggy_truncated_u29_integer(1_795_500_450);
    assert_eq!(
        Amf3Decoder::decode(&buggy_ts).unwrap(),
        Amf3Value::Integer(184_887_714)
    );
    let buggy_neg_ts = encode_buggy_truncated_u29_integer(0x7000_0000);
    assert_eq!(
        Amf3Decoder::decode(&buggy_neg_ts).unwrap(),
        Amf3Value::Integer(-268_435_456) // Flips positive 2029 timestamp into NEGATIVE!
    );

    // 64-bit Platform `qid` (`1243831974477471744 > 2^53`) precision guard -> `0x06 String`
    let qid = 1_243_831_974_477_471_744_i64;
    let safe_qid = Amf3Value::from_i64_precision_safe(qid);
    assert_eq!(safe_qid.as_str(), Some("1243831974477471744"));
    let unsafe_qid = Amf3Value::Double(qid as f64);
    assert_eq!(
        audit_lua_client_hazards(&unsafe_qid),
        vec![LuaClientHazard::UnsafeLargeFloatInteger]
    );
}

// ============================================================================
// 2. AMF3 Empty Lua Table `{}`, String/Object/Trait Refs & Nil-Hole Traps
// ============================================================================

#[test]
fn test_amf3_empty_table_string_refs_bytearray_and_lua_nil_holes() {
    // 1. Empty Lua table `{}` MUST encode as `[0x09, 0x01, 0x01]` (`Amf3Module.cpp:228,279`)
    let empty_tbl = Amf3Value::empty_lua_table();
    let wire_empty = Amf3Encoder::encode(&empty_tbl).unwrap();
    assert_eq!(wire_empty, vec![MARKER_ARRAY, 0x01, 0x01]);

    // 2. Non-empty string-keyed map encodes as `[0x0A, 0x0B, 0x01, ..., 0x01]`
    let map = Amf3Value::anonymous_object(vec![("err", Amf3Value::from_str("ok"))]);
    let wire_map = Amf3Encoder::encode(&map).unwrap();
    assert_eq!(&wire_map[0..3], &[MARKER_OBJECT, 0x0B, 0x01]);
    assert_eq!(*wire_map.last().unwrap(), 0x01);

    // 3. Empty string `""` (`0x01`) MUST NEVER shift `stringRefs` indices!
    let arr_with_empty_strs = Amf3Value::dense_array(vec![
        Amf3Value::from_str(""),
        Amf3Value::from_str("alpha"),
        Amf3Value::from_str(""),
        Amf3Value::from_str("alpha"),
    ]);
    let wire_strs = Amf3Encoder::encode(&arr_with_empty_strs).unwrap();
    assert_eq!(
        wire_strs,
        vec![
            MARKER_ARRAY,
            (4 << 1) | 1,
            0x01,
            MARKER_STRING,
            0x01, // "" (NOT added to stringRefs)
            MARKER_STRING,
            (5 << 1) | 1,
            b'a',
            b'l',
            b'p',
            b'h',
            b'a', // stringRefs[0] = "alpha"
            MARKER_STRING,
            0x01, // "" (still NOT added!)
            MARKER_STRING,
            0x00, // ref index 0 << 1 == 0x00!
        ]
    );
    assert_eq!(Amf3Decoder::decode(&wire_strs).unwrap(), arr_with_empty_strs);

    // 4. `0x06 String` (`stringRefs`) vs `0x0C ByteArray` (`objectRefs`) separate reference tables!
    let raw_mixed_refs = vec![
        MARKER_ARRAY,
        (4 << 1) | 1,
        0x01,
        MARKER_BYTE_ARRAY,
        (5 << 1) | 1,
        b'b',
        b'i',
        b'n',
        0x00,
        0xFF,
        MARKER_STRING,
        (5 << 1) | 1,
        b'h',
        b'e',
        b'l',
        b'l',
        b'o',
        MARKER_BYTE_ARRAY,
        1 << 1, // objectRefs[1]
        MARKER_STRING,
        0 << 1, // stringRefs[0]
    ];
    let dec_mixed = Amf3Decoder::decode(&raw_mixed_refs).unwrap();
    assert_eq!(
        dec_mixed,
        Amf3Value::dense_array(vec![
            Amf3Value::ByteArray(vec![b'b', b'i', b'n', 0x00, 0xFF]),
            Amf3Value::from_str("hello"),
            Amf3Value::ByteArray(vec![b'b', b'i', b'n', 0x00, 0xFF]),
            Amf3Value::from_str("hello"),
        ])
    );

    assert!(matches!(
        Amf3Decoder::decode(&[MARKER_STRING, 0x00]),
        Err(Amf3Error::InvalidStringRef { index: 0, .. })
    ));
    assert!(matches!(
        Amf3Decoder::decode(&[MARKER_ARRAY, 0x00]),
        Err(Amf3Error::InvalidObjectRef { index: 0, .. })
    ));
    assert!(matches!(
        Amf3Decoder::decode(&[MARKER_OBJECT, 0x01]),
        Err(Amf3Error::InvalidTraitRef { index: 0, .. })
    ));

    // 5. Malicious U29 allocation bomb (`denseCount = 134,217,727` in 5 bytes) fails instantly without OOM!
    let alloc_bomb = [MARKER_ARRAY, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
    assert!(matches!(
        Amf3Decoder::decode(&alloc_bomb),
        Err(Amf3Error::UnexpectedEof { .. })
    ));

    // 6. Lua 5.1 Nil-Hole Trap in `multi=1` `data.ret`:
    let rpc = build_rpc_response("ok", "user.getUser_2", Amf3Value::empty_lua_table(), None, None);
    let hole_at_1 = Amf3Value::anonymous_object(vec![(
        "ret",
        Amf3Value::dense_array(vec![Amf3Value::Null, rpc.clone()]),
    )]);
    assert_eq!(
        audit_lua_client_hazards(&hole_at_1),
        vec![LuaClientHazard::DenseArrayNilHole { lua_index: 1 }]
    );
    let raw_hole_1 =
        encode_server_raw_frame(&Amf3Encoder::encode(&hole_at_1).unwrap(), true, true).unwrap();
    let mut sim1 = Win32ClientSimulator::new();
    assert!(matches!(
        sim1.receive_server_bytes(&raw_hole_1, 1_795_500_450),
        Err(FrameError::LuaRuntimeCrash(msg)) if msg.contains("#rets == 0")
    ));

    let hole_at_2 = Amf3Value::anonymous_object(vec![(
        "ret",
        Amf3Value::dense_array(vec![rpc.clone(), Amf3Value::Undefined, rpc]),
    )]);
    let raw_hole_2 =
        encode_server_raw_frame(&Amf3Encoder::encode(&hole_at_2).unwrap(), false, true).unwrap();
    let mut sim2 = Win32ClientSimulator::new();
    assert!(matches!(
        sim2.receive_server_bytes(&raw_hole_2, 1_795_500_450),
        Err(FrameError::LuaRuntimeCrash(msg)) if msg.contains("got nil")
    ));
}

// ============================================================================
// 3. Asymmetric 24B/8B Framing, Reconnect Replay & Off-By-One Flag Traps
// ============================================================================

#[test]
fn test_asymmetric_framing_and_off_by_one_flag_traps() {
    let mut client = Win32ClientSimulator::new();
    client.reg_push_callback("push.mail.new");

    let (cb_name, tx_pkt) = client.build_send_msg("user.login", Amf3Value::empty_lua_table(), true);
    assert_eq!(cb_name, "user.login_2"); // mId starts at 1, increments BEFORE formatting!
    assert!(is_framed_lc_packet(&tx_pkt));
    assert_eq!(tx_pkt[4..8], [1, 1, 1, 1]);

    // Verify `sendUnusedMsg` reconnect replay updates `token` on in-flight RPCs (`Network.lua:207`)
    client.token = "reconnected_tok_999".to_string();
    let replayed = client.build_reconnect_replay_packets();
    assert_eq!(replayed.len(), 1);
    let mut srv_dec = ServerRxStreamDecoder::default();
    let replayed_decoded = srv_dec.push_bytes(&replayed[0]).unwrap();
    assert_eq!(
        replayed_decoded[0]
            .value
            .get_field("token")
            .and_then(|v| v.as_str()),
        Some("reconnected_tok_999")
    );

    // Verify Server->Client 8-byte header exact indices across all 4 `(zipped, multi)` modes
    for &(compress, multi, nested_bytes) in &[
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (true, true, true),
    ] {
        let mut sim = Win32ClientSimulator::new();
        sim.reg_push_callback("push.mail.new");
        let (cb, _) = sim.build_send_msg("user.getUser", Amf3Value::empty_lua_table(), true);

        let reply = build_rpc_response(
            "ok",
            &cb,
            Amf3Value::anonymous_object(vec![("silver", Amf3Value::from_i64_auto(900_000_000))]),
            Some("tok_abc"),
            Some(1_795_500_455),
        );

        let wire = if multi {
            let push = build_rpc_response(
                "ok",
                "push.mail.new",
                Amf3Value::anonymous_object(vec![("count", Amf3Value::Integer(3))]),
                None,
                None,
            );
            encode_server_multi_frame(
                &[reply, push],
                Some("tok_abc"),
                Some(1_795_500_455),
                compress,
                nested_bytes,
            )
            .unwrap()
        } else {
            encode_server_single_frame(&reply, compress).unwrap()
        };

        assert_eq!(wire[4], 0);
        assert_eq!(wire[5], u8::from(compress)); // Lua string.byte(head, 6) == zipped!
        assert_eq!(wire[6], u8::from(multi));    // Lua string.byte(head, 7) == multi!
        assert_eq!(wire[7], 0);

        let events = sim.receive_server_bytes(&wire, 1_795_500_450).unwrap();
        assert_eq!(sim.token, "tok_abc");
        assert_eq!(sim.time_offset_sec, 5);
        assert_eq!(sim.pending_one_shot_count(), 0);
        assert_eq!(events.len(), if multi { 2 } else { 1 });
    }

    // TRAP A: Off-by-one bug putting `zipped = 1` in `head[4]` and `0` in `head[5]`!
    let valid_resp = build_ping_response(1_795_500_450);
    let mut off_by_one_frame = encode_server_single_frame(&valid_resp, true).unwrap();
    off_by_one_frame[4] = 1;
    off_by_one_frame[5] = 0;
    let mut bad_sim1 = Win32ClientSimulator::new();
    assert!(matches!(
        bad_sim1.receive_server_bytes(&off_by_one_frame, 1_795_500_450),
        Err(FrameError::Amf3(Amf3Error::UnsupportedMarker { marker: 0x78, offset: 0 }))
    ));

    // TRAP B: Server mistakenly sends a 24-byte `LCPacket` to 8-byte RX client!
    let wrong_24b = encode_client_lc_packet(&Amf3Encoder::encode(&valid_resp).unwrap(), true);
    let mut bad_sim2 = Win32ClientSimulator::new();
    assert!(bad_sim2.receive_server_bytes(&wrong_24b, 1_795_500_450).is_err());

    // TRAP C: Server sends 0-byte body (`[0,0,0,0, 0,0,0,0]`) -> `CConnection` closes socket (`HEAD_SCRIPT_ERROR`)!
    let zero_hdr = [0u8; SERVER_RX_HEADER_LEN];
    let mut bad_sim3 = Win32ClientSimulator::new();
    assert_eq!(
        bad_sim3.receive_server_bytes(&zero_hdr, 1_795_500_450).unwrap_err(),
        FrameError::ClientSocketClosed("HEAD_SCRIPT_ERROR")
    );
    assert!(bad_sim3.socket_closed);

    // TRAP D: Missing `ret.callback` on non-ping response -> Lua nil-index crash (`Network.lua:146`)!
    let missing_cb = Amf3Value::anonymous_object(vec![("err", Amf3Value::from_str("ok"))]);
    let missing_cb_wire = encode_server_single_frame(&missing_cb, false).unwrap();
    let mut bad_sim4 = Win32ClientSimulator::new();
    assert!(matches!(
        bad_sim4.receive_server_bytes(&missing_cb_wire, 1_795_500_450),
        Err(FrameError::LuaRuntimeCrash(msg)) if msg.contains("field 'callback'")
    ));
}

// ============================================================================
// 4. Deeply Nested AMF3 Bombs, Corrupted Zlib Streams & Oversized >16 MiB DoS
// ============================================================================

#[test]
fn test_nested_amf3_bombs_corrupted_zlib_and_16mib_dos_guards() {
    fn make_nested_amf3_bytes(levels: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(levels * 3 + 2);
        for _ in 0..levels {
            buf.extend_from_slice(&[MARKER_ARRAY, 0x03, 0x01]);
        }
        buf.extend_from_slice(&[MARKER_INTEGER, 0x2A]);
        buf
    }

    let ok_64 = make_nested_amf3_bytes(MAX_RECURSION_DEPTH);
    assert!(Amf3Decoder::decode(&ok_64).is_ok());

    let bomb_65 = make_nested_amf3_bytes(MAX_RECURSION_DEPTH + 1);
    assert_eq!(
        Amf3Decoder::decode(&bomb_65),
        Err(Amf3Error::MaxDepthExceeded {
            depth: MAX_RECURSION_DEPTH + 1
        })
    );

    let bomb_10k = make_nested_amf3_bytes(10_000);
    assert!(matches!(
        Amf3Decoder::decode(&bomb_10k),
        Err(Amf3Error::MaxDepthExceeded { .. })
    ));

    let mut dec = ServerRxStreamDecoder::default();
    let over_16mib = ((MAX_SERVER_FRAME_BODY_LEN as u32) + 1).to_be_bytes();
    assert_eq!(
        dec.push_bytes(&over_16mib).unwrap_err(),
        FrameError::OversizedBody {
            declared: (MAX_SERVER_FRAME_BODY_LEN as u32) + 1,
            max: MAX_SERVER_FRAME_BODY_LEN,
        }
    );

    let max_u32_hdr = u32::MAX.to_be_bytes();
    assert_eq!(
        dec.push_bytes(&max_u32_hdr).unwrap_err(),
        FrameError::OversizedBody {
            declared: u32::MAX,
            max: MAX_SERVER_FRAME_BODY_LEN,
        }
    );

    assert_eq!(
        zlib_decompress_bounded(&[0x78, 0x9C, 0x03], 1024),
        Err(ZlibError::TruncatedHeader)
    );
    assert_eq!(
        zlib_decompress_bounded(&[0x79, 0x9C, 0, 0, 0, 1], 1024),
        Err(ZlibError::InvalidCompressionMethod(9))
    );
    assert_eq!(
        zlib_decompress_bounded(&[0x78, 0x00, 0, 0, 0, 1], 1024),
        Err(ZlibError::HeaderChecksumFailed)
    );
    assert_eq!(
        zlib_decompress_bounded(&[0x78, 0x9C, 0x07, 0x00, 0x00, 0x00, 0x01], 1024),
        Err(ZlibError::ReservedBlockType)
    );

    let mut bad_adler = zlib_compress(b"BabelTime_GCLIENT_vnsg2_payload");
    let last_idx = bad_adler.len() - 1;
    bad_adler[last_idx] ^= 0xFF;
    assert!(matches!(
        zlib_decompress_bounded(&bad_adler, 1024),
        Err(ZlibError::Adler32Mismatch { .. })
    ));

    let target_bomb_size = MAX_SERVER_FRAME_BODY_LEN + 4096;
    let z_bomb = craft_zlib_zero_bomb(target_bomb_size);
    assert!(z_bomb.len() < 150_000);
    assert_eq!(
        zlib_decompress_bounded(&z_bomb, MAX_SERVER_FRAME_BODY_LEN),
        Err(ZlibError::OutputLimitExceeded {
            limit: MAX_SERVER_FRAME_BODY_LEN
        })
    );

    let bomb_md5 = compute_babeltime_md5(&z_bomb);
    let mut framed_bomb = Vec::with_capacity(CLIENT_LC_HEADER_LEN + z_bomb.len());
    framed_bomb.extend_from_slice(&(z_bomb.len() as u32).to_be_bytes());
    framed_bomb.extend_from_slice(&[1, 1, 1, 1]);
    framed_bomb.extend_from_slice(&bomb_md5);
    framed_bomb.extend_from_slice(&z_bomb);

    let mut bomb_dec = ServerRxStreamDecoder::default();
    assert_eq!(
        bomb_dec.push_bytes(&framed_bomb).unwrap_err(),
        FrameError::Zlib(ZlibError::OutputLimitExceeded {
            limit: MAX_SERVER_FRAME_BODY_LEN
        })
    );

    let payload = b"StoredBlockTest_GCLIENT";
    let plen = payload.len() as u16;
    let nlen = !plen;
    let mut stored_zlib = vec![0x78, 0x01, 0x01];
    stored_zlib.extend_from_slice(&plen.to_le_bytes());
    stored_zlib.extend_from_slice(&nlen.to_le_bytes());
    stored_zlib.extend_from_slice(payload);
    stored_zlib.extend_from_slice(&adler32(payload).to_be_bytes());
    assert_eq!(zlib_decompress_bounded(&stored_zlib, 1024).unwrap(), payload);

    let mut tmp = Vec::new();
    for &u in &[0u32, 0x7F, 0x80, 0x3FFF, 0x4000, 0x1F_FFFF, 0x20_0000, 0x1FFF_FFFF] {
        tmp.clear();
        write_u29(&mut tmp, u);
        let mut p = 0;
        assert_eq!(gserver_qa_harness::amf3::read_u29(&tmp, &mut p).unwrap(), u);
        assert_eq!(p, tmp.len());
    }
}

// ============================================================================
// 5. `BtPlatformWin32.cpp` Live Platform Signatures, `?&pid=` Quirks & `BtDecryptHook`
// ============================================================================

#[test]
fn test_bt_platform_win32_live_signatures_empty_query_segments_and_bt_decrypt() {
    let live_signed_urls = [
        "http://127.0.0.1:8080/phone/applereview/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&version=1.0.34&time=1795500444&sign=ae1afadf7779b11b79ca86e9d7fc72c7",
        "http://127.0.0.1:8080/phone/serverlist/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&packageVersion=1.0.34&scriptVersion=1.0.1194&time=1795500444&sign=364dfa193b076f7ab50b8c4ce17bec20",
        "http://127.0.0.1:8080/phone/supernotice/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&action=get&browser_width=221.5&reserve01=1&reserve02=1&returntype=tpl&serverKey=2901000&time=1795500446&sign=c9ee3c6c503be557a3b1c67757f447b1",
        "http://127.0.0.1:8080/phone/login?userid=1243831974477471744&sessionid=SID_56921a50f34cf49824123bfd56a62d70bb1e51795440318446&pl=android&os=android&gn=vnsg2&bind=21a23244ed44810da2d40c6d4455e686&uuid=21a23244ed44810da2d40c6d4455e686&country=VN&time=1795500447&sign=ff287c962a529dc40aa76175968273fd",
        "http://127.0.0.1:8080/phone/loginrole/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&pid=636df6782106606d&time=1795500448&sign=b2b3ac6177ec9380f0713f304bb49429",
        "http://127.0.0.1:8080/phone/gethash/?pid=636df6782106606d&uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&group_id=2900161&time=1795500451&sign=57d777266bf1346b90118e5127550da4",
    ];

    for url in live_signed_urls {
        verify_platform_url_signature(url, VNSG2_LIVE_SIGN_KEY)
            .expect("live Platform URL signature must validate");
    }

    let with_empty_seg =
        "http://127.0.0.1:18080/phone/gethash/?&pid=636df6782106606d&&uuid=abc&gn=vnsg2";
    let clean_seg = "http://127.0.0.1:18080/phone/gethash/?pid=636df6782106606d&uuid=abc&gn=vnsg2";
    assert_eq!(sort_url_params(with_empty_seg), sort_url_params(clean_seg));
    let signed_weird = sign_platform_url(with_empty_seg, 1_795_500_451, VNSG2_LIVE_SIGN_KEY);
    verify_platform_url_signature(&signed_weird, VNSG2_LIVE_SIGN_KEY).unwrap();

    let sim_empty_pid = "http://127.0.0.1:18080/phone/gethash/?pid=&uuid=abc&gn=vnsg2";
    assert!(sort_url_params(sim_empty_pid).contains("pid="));
    let signed_sim = sign_platform_url(sim_empty_pid, 1_795_500_451, VNSG2_LIVE_SIGN_KEY);
    verify_platform_url_signature(&signed_sim, VNSG2_LIVE_SIGN_KEY).unwrap();

    let parsed = parse_tolerant_query_params(&signed_weird);
    assert_eq!(parsed.get("pid").map(String::as_str), Some("636df6782106606d"));

    let cfg = HttpGatewayConfig {
        enforce_signature: true,
        ..HttpGatewayConfig::default()
    };
    let metrics = HttpGatewayMetrics::default();
    let sessions = Mutex::new(HashMap::new());

    let login_url = sign_platform_url(
        "/phone/login?userid=1243831974477471744&sessionid=SID_tok&pl=android&os=android&gn=vnsg2",
        1_795_500_447,
        &cfg.sign_key,
    );
    let (st, set_cookie, login_body) =
        route_sdk_http_request(&login_url, None, &cfg, &metrics, &sessions);
    assert_eq!(st, 200);
    let cookie_pair = set_cookie
        .expect("login must issue PHPSESSID cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let expected_pid = md5_hex(b"gserver_pid_1243831974477471744")[..16].to_string();
    assert!(login_body.contains(&expected_pid));

    let gethash_url = sign_platform_url(
        "/phone/gethash/?&pid=&uuid=21a23244ed44810da2d40c6d4455e686&group_id=2900161",
        1_795_500_451,
        &cfg.sign_key,
    );
    let (st2, _, hash_body) =
        route_sdk_http_request(&gethash_url, Some(&cookie_pair), &cfg, &metrics, &sessions);
    assert_eq!(st2, 200);
    assert!(hash_body.contains(&format!("\"pid\":\"{}\"", expected_pid)));
    assert!(hash_body.contains("\"host\":\"127.0.0.1\""));
    assert!(hash_body.contains("\"port\":18160"));

    for key_idx in 0u8..16u8 {
        let sample = b"local zipped, multi = string.byte(head, 6, 7); -- Network.lua";
        let enc = bt_encrypt_asset(sample, key_idx).unwrap();
        let dec = bt_decrypt_asset(&enc).unwrap();
        assert_eq!(dec, sample);
    }
}

// ============================================================================
// 6. Live TCP Streaming Torture: 1-Byte Chunks, 64-RPC Pipeline & Truncated EOF
// ============================================================================

#[test]
fn test_1byte_chunked_tcp_pipelined_bursts_and_truncated_frames() {
    let cfg = GameServerConfig {
        read_timeout: Duration::from_millis(300),
        ..GameServerConfig::default()
    };
    let mut server = GameTcpServer::start("127.0.0.1:0", cfg).unwrap();
    let addr = server.local_addr();

    {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

        let mut sim = Win32ClientSimulator::new();
        sim.reg_push_callback("push.reward.update");
        let (cb1, pkt1) = sim.build_send_msg("user.login", Amf3Value::empty_lua_table(), true);
        let (cb2, pkt2) = sim.build_send_msg("qa.multiBurst", Amf3Value::empty_lua_table(), false);

        for &b in pkt1.iter().chain(pkt2.iter()) {
            stream.write_all(&[b]).unwrap();
        }
        stream.flush().unwrap();

        let mut received_events = Vec::new();
        let mut one_byte = [0u8; 1];
        while received_events.len() < 3 {
            let n = stream.read(&mut one_byte).unwrap();
            assert_eq!(n, 1);
            let mut evs = sim.receive_server_bytes(&one_byte, 1_795_500_450).unwrap();
            received_events.append(&mut evs);
        }
        assert_eq!(received_events[0].callback_name, cb1);
        assert_eq!(received_events[1].callback_name, cb2);
        assert_eq!(received_events[2].callback_name, "push.reward.update");
        assert!(received_events[2].was_push);
    }

    {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

        let mut sim = Win32ClientSimulator::new();
        let mut burst = Vec::new();
        let mut expected_cbs = Vec::with_capacity(64);
        for idx in 0..64 {
            let (cb, pkt) = sim.build_send_msg(
                "bag.bagInfo",
                Amf3Value::anonymous_object(vec![("slot", Amf3Value::Integer(idx))]),
                idx % 2 == 0,
            );
            expected_cbs.push(cb);
            burst.extend_from_slice(&pkt);
        }

        stream.write_all(&burst).unwrap();
        stream.flush().unwrap();

        let mut all_events = Vec::with_capacity(64);
        let mut rbuf = [0u8; 4096];
        while all_events.len() < 64 {
            let n = stream.read(&mut rbuf).unwrap();
            assert!(n > 0);
            all_events.extend(sim.receive_server_bytes(&rbuf[..n], 1_795_500_450).unwrap());
        }
        let got_cbs: Vec<String> = all_events.into_iter().map(|e| e.callback_name).collect();
        assert_eq!(got_cbs, expected_cbs);
        assert_eq!(sim.pending_one_shot_count(), 0);
    }

    let sample_pkt = encode_client_lc_packet(
        &Amf3Encoder::encode(&Amf3Value::anonymous_object(vec![(
            "method",
            Amf3Value::from_str("ping"),
        )]))
        .unwrap(),
        true,
    );
    for &cut in &[1usize, 10, 24, sample_pkt.len() - 2] {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(&sample_pkt[..cut]).unwrap();
        drop(s);
    }
    {
        let mut dos_sock = TcpStream::connect(addr).unwrap();
        let mut fake_hdr = [0u8; CLIENT_LC_HEADER_LEN];
        fake_hdr[0..4].copy_from_slice(&((MAX_SERVER_FRAME_BODY_LEN as u32) + 42).to_be_bytes());
        fake_hdr[4..8].copy_from_slice(&[1, 1, 1, 1]);
        let _ = dos_sock.write_all(&fake_hdr);
    }

    thread::sleep(Duration::from_millis(80));
    let tel = server.telemetry();
    assert!(tel.clean_truncated_disconnects.load(Ordering::Relaxed) >= 4);
    assert!(tel.rejected_oversized_frames.load(Ordering::Relaxed) >= 1);
    assert_eq!(tel.lua_hazard_violations.load(Ordering::Relaxed), 0);

    server.stop();
}

// ============================================================================
// 7. 32-Thread Concurrent Win32 Client + Hostile Neighbor Stress Simulation
// ============================================================================

#[test]
fn test_32_thread_concurrent_client_and_hostile_neighbor_stress() {
    let game_server = GameTcpServer::start("127.0.0.1:0", GameServerConfig::default()).unwrap();
    let game_addr = game_server.local_addr();

    let http_cfg = HttpGatewayConfig {
        game_tcp_host: "127.0.0.1".to_string(),
        game_tcp_port: game_addr.port(),
        enforce_signature: true,
        ..HttpGatewayConfig::default()
    };
    let http_gateway = HttpSdkGateway::start("127.0.0.1:0", http_cfg.clone()).unwrap();
    let http_addr = http_gateway.local_addr();

    const NUM_THREADS: usize = 32;
    let barrier = Arc::new(Barrier::new(NUM_THREADS));
    let mut handles = Vec::with_capacity(NUM_THREADS);

    for tid in 0..NUM_THREADS {
        let bar = Arc::clone(&barrier);
        let sign_key = http_cfg.sign_key.clone();
        handles.push(thread::spawn(move || {
            bar.wait();

            let pid = format!("636df6782106{:04x}", tid);
            let raw_url = format!(
                "/phone/gethash/?&pid={}&&uuid=21a23244ed44810da2d40c6d4455{:04x}&pl=android&os=android&gn=vnsg2&group_id=2900161",
                pid, tid
            );
            let signed_path = sign_platform_url(&raw_url, 1_795_500_451, &sign_key);
            let http_req = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                signed_path, http_addr
            );
            let mut hstream = TcpStream::connect(http_addr).unwrap();
            hstream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            hstream.write_all(http_req.as_bytes()).unwrap();
            let mut hresp = String::new();
            hstream.read_to_string(&mut hresp).unwrap();
            assert!(hresp.starts_with("HTTP/1.1 200 OK"), "HTTP failed on tid {}", tid);
            assert!(hresp.contains(&format!("\"port\":{}", game_addr.port())));

            let mut tstream = TcpStream::connect(game_addr).unwrap();
            tstream.set_nodelay(true).unwrap();
            tstream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

            let mut sim = Win32ClientSimulator::new();
            sim.reg_push_callback("push.reward.update");

            let (cb_login, pkt_login) = sim.build_send_msg(
                "user.login",
                Amf3Value::anonymous_object(vec![
                    ("pid", Amf3Value::from_str(&pid)),
                    ("timestamp", Amf3Value::from_i64_auto(1_795_500_450)),
                ]),
                true,
            );
            let pkt_stat = sim.build_stat_msg(Amf3Value::anonymous_object(vec![(
                "step",
                Amf3Value::Integer(tid as i32),
            )]));
            let (cb_user, pkt_user) =
                sim.build_send_msg("user.getUser", Amf3Value::empty_lua_table(), true);
            let (cb_multi, pkt_multi) =
                sim.build_send_msg("qa.multiBurst", Amf3Value::empty_lua_table(), false);

            let mut wire_batch = Vec::new();
            wire_batch.extend_from_slice(&pkt_login);
            wire_batch.extend_from_slice(&pkt_stat);
            wire_batch.extend_from_slice(&pkt_user);
            wire_batch.extend_from_slice(&pkt_multi);

            if tid % 2 == 1 {
                for slice in wire_batch.chunks(17) {
                    tstream.write_all(slice).unwrap();
                }
            } else {
                tstream.write_all(&wire_batch).unwrap();
            }
            tstream.flush().unwrap();

            let mut events = Vec::new();
            let mut rbuf = [0u8; 2048];
            while events.len() < 4 {
                let n = tstream.read(&mut rbuf).unwrap();
                assert!(n > 0, "unexpected EOF on tid {}", tid);
                events.extend(sim.receive_server_bytes(&rbuf[..n], 1_795_500_450).unwrap());
            }

            assert_eq!(events[0].callback_name, cb_login);
            assert_eq!(events[1].callback_name, cb_user);
            assert_eq!(events[2].callback_name, cb_multi);
            assert_eq!(events[3].callback_name, "push.reward.update");
            assert_eq!(sim.pending_one_shot_count(), 0);
            assert_eq!(sim.time_offset_sec, 0);

            if tid % 4 == 0 {
                if let Ok(mut bad) = TcpStream::connect(game_addr) {
                    let mut corrupt = pkt_login.clone();
                    corrupt[12] ^= 0xAA;
                    let _ = bad.write_all(&corrupt);
                }
                if let Ok(mut bad2) = TcpStream::connect(game_addr) {
                    let mut depth_bomb = Vec::with_capacity(80 * 3 + 2);
                    for _ in 0..80 {
                        depth_bomb.extend_from_slice(&[MARKER_ARRAY, 0x03, 0x01]);
                    }
                    depth_bomb.extend_from_slice(&[MARKER_INTEGER, 0x01]);
                    let framed_depth = encode_client_lc_packet(&depth_bomb, true);
                    let _ = bad2.write_all(&framed_depth);
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("concurrent client worker must not panic");
    }

    thread::sleep(Duration::from_millis(50));
    let tel = game_server.telemetry();
    assert!(tel.accepted_connections.load(Ordering::Relaxed) >= 32 + 16);
    assert_eq!(tel.valid_rpcs_handled.load(Ordering::Relaxed), (32 * 3) as u64);
    assert_eq!(tel.stat_msgs_ignored_cleanly.load(Ordering::Relaxed), 32);
    assert_eq!(tel.multi_batches_sent.load(Ordering::Relaxed), 32);
    assert_eq!(tel.rejected_md5_frames.load(Ordering::Relaxed), 8);
    assert_eq!(tel.rejected_amf3_depth_bombs.load(Ordering::Relaxed), 8);
    assert_eq!(tel.lua_hazard_violations.load(Ordering::Relaxed), 0);
    assert_eq!(
        http_gateway.metrics().gethash_requests.load(Ordering::Relaxed),
        32
    );
}

// ============================================================================
// 8. Deterministic PRNG Structured & Mutational Fuzz Suite (5,000 Iterations)
// ============================================================================

#[test]
fn test_deterministic_prng_fuzz_suite_5000_iterations() {
    let summary = run_deterministic_fuzz_campaign(DEFAULT_FUZZ_SEED, 5_000);
    assert_eq!(summary.structured_roundtrips, 5_000);
    assert_eq!(summary.mutated_amf3_cases, 5_000);
    assert_eq!(summary.mutated_zlib_cases, 5_000);
    assert_eq!(summary.mutated_frame_cases, 5_000);
    assert_eq!(summary.mutated_http_cases, 5_000);
}

#[test]
fn test_c_zlib_dynamic_huffman_interop() {
    // Exact RFC 1950 + BTYPE=10 Dynamic-Huffman vector compressed by C libz (level 9):
    let c_zlib_dynamic: &[u8] = &[0x78, 0xDA, 0x73, 0x77, 0xF6, 0xF1, 0x74, 0xF5, 0x0B, 0x89, 0x2F, 0xCB, 0x2B, 0x4E, 0x37, 0x8A, 0x4F, 0xA9, 0xCC, 0x4B, 0xCC, 0xCD, 0x4C, 0x8E, 0xCF, 0x28, 0x4D, 0x4B, 0xCB, 0x4D, 0xCC, 0x8B, 0x4F, 0xCA, 0xC9, 0x4F, 0xCE, 0x8E, 0x37, 0x30, 0x34, 0x32, 0x36, 0x31, 0x35, 0x33, 0xB7, 0xB0, 0x74, 0x74, 0x72, 0x76, 0x71, 0x75, 0x8B, 0x77, 0x1F, 0xD5, 0x34, 0xAA, 0x89, 0x72, 0x4D, 0x00, 0xF4, 0xF2, 0xD3, 0xD5];
    let expected = b"GCLIENT_vnsg2_dynamic_huffman_block_0123456789ABCDEF_".repeat(12);
    let inflated = zlib_decompress_bounded(c_zlib_dynamic, 4096).expect("must inflate C libz BTYPE=10");
    assert_eq!(inflated, expected);

    // Re-compress with our pure-Rust compressor and verify round-trip + Adler-32 trailer match:
    let rust_comp = zlib_compress(&expected);
    assert!(rust_comp.len() < expected.len() / 3);
    assert_eq!(zlib_decompress_bounded(&rust_comp, 4096).unwrap(), expected);
}
