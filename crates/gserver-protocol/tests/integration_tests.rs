//! Exhaustive offline test suite for `gserver-protocol`:
//! - AMF3 primitives, U29 boundaries, IEEE-754 doubles, UTF-8 & binary-safe strings
//! - AMF3 string/object/trait reference tables, externalizable objects, depth & allocation bounds
//! - RFC 1950 + RFC 1951 Zlib inflate (stored, fixed Huffman, real C libz dynamic Huffman) & deflate
//! - Adler-32, RFC 1321 MD5, BabelTime `LCPacket` salted XOR-chain MD5, `0xFEFEFEFE` `bt_decrypt`
//! - Streaming C2S `ClientRxDecoder` (24B `LCPacket`, 4B raw, `Auto`, 1-byte chunks, pipelining)
//! - S2C 8-byte `ServerTxEncoder` & `ServerRxDecoder` (single, `zipped=1`, `multi=1`, `zipped+multi`)
//! - Full `vnsg2` `user.login` / `user.userLogin` / `re.*` push / `err="ping"` envelope round-trips

use gserver_protocol::amf3::{
    self, Amf3Decoder, Amf3Encoder, Amf3Object, DecodeConfig, Value, AMF3_INT_MAX, AMF3_INT_MIN,
    MARKER_ARRAY, MARKER_BYTE_ARRAY, MARKER_DATE, MARKER_DOUBLE, MARKER_INTEGER, MARKER_OBJECT,
};
use gserver_protocol::bt_decrypt::{bt_decrypt, bt_decrypt_or_passthrough, bt_encrypt, has_bt_magic};
use gserver_protocol::error::{
    Amf3Error, BtDecryptError, EnvelopeError, FrameError, ProtocolError, ZlibError,
};
use gserver_protocol::frame::{
    encode_c2s_lcpacket, encode_c2s_lcpacket_with_mode, encode_c2s_raw_frame,
    split_length_prefixed_subpackets, C2sFrameMode, C2sWireKind, ClientRxDecoder, S2cFlags,
    ServerRxDecoder, ServerTxEncoder, C2S_LCPACKET_HEADER_SIZE, S2C_HEADER_SIZE,
};
use gserver_protocol::md5::{compute_babeltime_md5, md5_digest, md5_hex};
use gserver_protocol::rpc::{
    decode_s2c_frame_batch, encode_s2c_multi_frame, encode_s2c_single_frame, MultiItemEncoding,
    RpcRequest, S2cEnvelope,
};
use gserver_protocol::zlib::{
    adler32, zlib_compress, zlib_compress_with_mode, zlib_decompress, zlib_decompress_with_limit,
    CompressionMode,
};

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let val = |b: u8| -> u8 {
        match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => 10 + (b - b'a'),
            b'A'..=b'F' => 10 + (b - b'A'),
            _ => panic!("invalid hex char: {b}"),
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        out.push((val(bytes[i]) << 4) | val(bytes[i + 1]));
        i += 2;
    }
    out
}

// ============================================================================
// 1. AMF3 Primitives, U29 Boundaries & String Reference Table Tests
// ============================================================================

#[test]
fn test_01_amf3_primitives_undefined_null_bool() {
    for (val, expected_bytes) in [
        (Value::Undefined, &[0x00u8][..]),
        (Value::Null, &[0x01u8][..]),
        (Value::Bool(false), &[0x02u8][..]),
        (Value::Bool(true), &[0x03u8][..]),
    ] {
        let enc = amf3::encode(&val).unwrap();
        assert_eq!(enc, expected_bytes);
        let (dec, used) = amf3::decode(&enc).unwrap();
        assert_eq!(used, 1);
        assert_eq!(dec, val);
    }
}

#[test]
fn test_02_amf3_u29_integer_widths_and_negative_sign_extension() {
    let cases = [
        (0, vec![0x04, 0x00]),
        (1, vec![0x04, 0x01]),
        (0x7F, vec![0x04, 0x7F]),
        (0x80, vec![0x04, 0x81, 0x00]),
        (0x3FFF, vec![0x04, 0xFF, 0x7F]),
        (0x4000, vec![0x04, 0x81, 0x80, 0x00]),
        (0x1F_FFFF, vec![0x04, 0xFF, 0xFF, 0x7F]),
        (0x20_0000, vec![0x04, 0x80, 0xC0, 0x80, 0x00]),
        (AMF3_INT_MAX, vec![0x04, 0xBF, 0xFF, 0xFF, 0xFF]),
        (-1, vec![0x04, 0xFF, 0xFF, 0xFF, 0xFF]),
        (-128, vec![0x04, 0xFF, 0xFF, 0xFF, 0x80]),
        (AMF3_INT_MIN, vec![0x04, 0xC0, 0x80, 0x80, 0x00]),
    ];

    for (num, expected) in cases {
        let enc = amf3::encode(&Value::Int(num)).unwrap();
        assert_eq!(enc, expected, "mismatch for i32 {num}");
        let (dec, consumed) = amf3::decode(&enc).unwrap();
        assert_eq!(consumed, expected.len());
        assert_eq!(dec, Value::Int(num));
    }
}

#[test]
fn test_03_amf3_integer_out_of_u29_range_promotes_to_double() {
    // Matches Amf3Module.cpp: numbers outside [-2^28, 2^28 - 1] and -0.0 encode as 0x05 Double.
    let over = Value::Int(AMF3_INT_MAX + 1);
    let enc_over = amf3::encode(&over).unwrap();
    assert_eq!(enc_over[0], MARKER_DOUBLE);
    let (dec_over, _) = amf3::decode(&enc_over).unwrap();
    assert_eq!(dec_over, Value::Double(f64::from(AMF3_INT_MAX + 1)));

    let under = Value::Int(AMF3_INT_MIN - 1);
    let enc_under = amf3::encode(&under).unwrap();
    assert_eq!(enc_under[0], MARKER_DOUBLE);
    let (dec_under, _) = amf3::decode(&enc_under).unwrap();
    assert_eq!(dec_under, Value::Double(f64::from(AMF3_INT_MIN - 1)));

    let neg_zero = Value::from_lua_number(-0.0);
    let enc_nz = amf3::encode(&neg_zero).unwrap();
    assert_eq!(enc_nz[0], MARKER_DOUBLE);
    let (dec_nz, _) = amf3::decode(&enc_nz).unwrap();
    match dec_nz {
        Value::Double(d) => assert!(d == 0.0 && d.is_sign_negative()),
        other => panic!("expected Double(-0.0), got {other:?}"),
    }

    let pos_int = Value::from_lua_number(42.0);
    assert_eq!(amf3::encode(&pos_int).unwrap()[0], MARKER_INTEGER);
}

#[test]
fn test_04_amf3_doubles_nan_infinity_and_unix_timestamps() {
    for d in [0.0, std::f64::consts::PI, -1e100, f64::INFINITY, f64::NEG_INFINITY, 1_795_500_450.0] {
        let enc = amf3::encode(&Value::Double(d)).unwrap();
        assert_eq!(enc.len(), 9);
        assert_eq!(enc[0], MARKER_DOUBLE);
        let (dec, used) = amf3::decode(&enc).unwrap();
        assert_eq!(used, 9);
        assert_eq!(dec, Value::Double(d));
    }

    let enc_nan = amf3::encode(&Value::Double(f64::NAN)).unwrap();
    let (dec_nan, _) = amf3::decode(&enc_nan).unwrap();
    match dec_nan {
        Value::Double(x) => assert!(x.is_nan()),
        other => panic!("expected NaN Double, got {other:?}"),
    }
}

#[test]
fn test_05_amf3_empty_string_never_enters_string_ref_table() {
    // Empty string is 0x06 0x01 and MUST NOT occupy slot 0 of stringRefs!
    let arr = Value::dense_array(vec![
        Value::from(""),
        Value::from("vnsg2"),
        Value::from(""),
        Value::from("vnsg2"),
    ]);
    let enc = amf3::encode(&arr).unwrap();
    // Expected:
    // 09 (Array) 09 (dense=4, inline) 01 (assoc end "")
    // 06 01 ("")
    // 06 0B "vnsg2" (slot 0 in stringRefs)
    // 06 01 ("")
    // 06 00 (stringRef index 0 => 0 << 1 == 0x00!)
    assert_eq!(
        enc,
        vec![
            0x09, 0x09, 0x01, 0x06, 0x01, 0x06, 0x0B, b'v', b'n', b's', b'g', b'2', 0x06, 0x01,
            0x06, 0x00
        ]
    );
    let (dec, used) = amf3::decode(&enc).unwrap();
    assert_eq!(used, enc.len());
    assert_eq!(dec, arr);
}

#[test]
fn test_06_amf3_unicode_vietnamese_and_chinese_strings_dedup() {
    let s1 = "S161.Võ Xương";
    let s2 = "ActiveTaskShopTaskData初始化";
    let val = Value::dense_array(vec![
        Value::from(s1),
        Value::from(s2),
        Value::from(s1),
        Value::from(s2),
    ]);
    let enc = amf3::encode(&val).unwrap();
    // Second occurrences are 2 bytes each ([0x06, ref<<1]), so total size is much smaller than 2x.
    assert!(enc.len() < 3 + 2 * (s1.len() + s2.len()));
    let (dec, _) = amf3::decode(&enc).unwrap();
    assert_eq!(dec, val);
}

#[test]
fn test_07_amf3_binary_safe_byte_string_and_strict_utf8_option() {
    let non_utf8 = vec![0xFF, 0xFE, 0x00, 0x80, 0xAB];
    let val = Value::dense_array(vec![
        Value::ByteString(non_utf8.clone()),
        Value::ByteString(non_utf8.clone()),
    ]);
    let enc = amf3::encode(&val).unwrap();

    // Default config (strict_utf8 = false) preserves binary Lua 0x06 strings cleanly.
    let (dec, _) = amf3::decode(&enc).unwrap();
    assert_eq!(dec, val);

    // Strict UTF-8 mode rejects non-UTF-8 0x06 strings.
    let strict_cfg = DecodeConfig {
        strict_utf8: true,
        ..DecodeConfig::default()
    };
    let err = amf3::decode_from(&enc, 0, strict_cfg).unwrap_err();
    assert!(matches!(err, Amf3Error::InvalidUtf8 { .. }));
}

// ============================================================================
// 2. AMF3 Arrays, Objects, Traits, ObjectRefs & Security Bounds
// ============================================================================

#[test]
fn test_08_amf3_empty_lua_table_and_mixed_assoc_dense_array() {
    let empty = Value::empty_array();
    let enc_empty = amf3::encode(&empty).unwrap();
    assert_eq!(enc_empty, vec![0x09, 0x01, 0x01]);
    assert_eq!(amf3::decode(&enc_empty).unwrap().0, empty);

    let mixed = Value::Array {
        dense: vec![Value::Int(10), Value::Int(20), Value::from("thirty")],
        assoc: vec![
            (String::from("extra"), Value::Bool(true)),
            (String::from("tag"), Value::from("thirty")), // shares stringRef with dense[2]!
        ],
    };
    let enc = amf3::encode(&mixed).unwrap();
    let (dec, used) = amf3::decode(&enc).unwrap();
    assert_eq!(used, enc.len());
    assert_eq!(dec, mixed);
}

#[test]
fn test_09_amf3_anonymous_dynamic_object_matches_amf3module_cpp_wire_layout() {
    // Amf3Module.cpp encodes pure string-keyed Lua tables with:
    // 0x0A (Object), 0x0B (inline obj + inline traits + dynamic, 0 sealed), 0x01 ("" class), ...k/v..., 0x01
    let obj = Value::map([("err", Value::from("ok"))]);
    let enc = amf3::encode(&obj).unwrap();
    assert_eq!(
        enc,
        vec![
            0x0A, 0x0B, 0x01, // Object, traits=0x0B, className=""
            0x07, b'e', b'r', b'r', // key "err" (len 3 -> (3<<1)|1 = 0x07)
            0x06, 0x05, b'o', b'k', // String "ok" (len 2 -> (2<<1)|1 = 0x05)
            0x01  // end of dynamic members
        ]
    );
    let (dec, _) = amf3::decode(&enc).unwrap();
    assert_eq!(dec, obj);
}

#[test]
fn test_10_amf3_sealed_traits_and_trait_reference_deduplication() {
    let make_hero = |id: i32, name: &str| {
        Value::Object(Amf3Object {
            class_name: String::from("vo.HeroVO"),
            dynamic: false,
            sealed_members: vec![
                (String::from("hid"), Value::Int(id)),
                (String::from("name"), Value::from(name)),
            ],
            dynamic_members: Vec::new(),
            externalized: None,
        })
    };

    let list = Value::dense_array(vec![
        make_hero(1001, "Guan Yu"),
        make_hero(1002, "Zhang Fei"),
        make_hero(1003, "Zhao Yun"),
    ]);

    let enc = amf3::encode(&list).unwrap();
    // "vo.HeroVO" should appear as literal ASCII only ONCE in `enc` thanks to traitRefs!
    let needle = b"vo.HeroVO";
    let occurrences = enc.windows(needle.len()).filter(|w| *w == needle).count();
    assert_eq!(occurrences, 1, "sealed trait definition must be deduplicated via traitRefs");

    let (dec, used) = amf3::decode(&enc).unwrap();
    assert_eq!(used, enc.len());
    assert_eq!(dec, list);
}

#[test]
fn test_11_amf3_externalizable_object_and_bytearray_date_xml() {
    let ext_obj = Value::Object(Amf3Object {
        class_name: String::from("flex.messaging.io.ArrayCollection"),
        dynamic: false,
        sealed_members: Vec::new(),
        dynamic_members: Vec::new(),
        externalized: Some(Box::new(Value::dense_array(vec![
            Value::ByteArray(vec![0xDE, 0xAD, 0xBE, 0xEF]),
            Value::Date(1_795_500_450_000.0),
            Value::XmlDoc(b"<doc/>".to_vec()),
            Value::Xml(b"<node attr='1'/>".to_vec()),
        ]))),
    });

    let enc = amf3::encode(&ext_obj).unwrap();
    let (dec, used) = amf3::decode(&enc).unwrap();
    assert_eq!(used, enc.len());
    assert_eq!(dec, ext_obj);
}

#[test]
fn test_12_amf3_object_reference_table_shared_containers() {
    // Build an AMF3 Array [ obj0, bytearray1, date2, ref(obj0), ref(bytearray1), ref(date2) ]
    let mut enc = Amf3Encoder::new();
    // Outer Array (takes objectRef 0): dense count = 6 => (6 << 1) | 1 = 13 (0x0D), empty assoc 0x01
    let mut raw = vec![MARKER_ARRAY, 0x0D, 0x01];

    // Re-encode using Amf3Encoder with objectRef slot 0 reserved by encoding a wrapper:
    let shared_obj = Value::map([("uid", Value::Int(52592)), ("vip", Value::Int(3))]);
    let shared_ba = Value::ByteArray(vec![1, 2, 3, 4, 5]);
    let shared_date = Value::Date(1_700_000_000_000.0);

    // Encode outer array manually through encoder:
    // Instead, let's encode a 2-element array first? Even better:
    // Let's encode an array of 3 inline items, then append 3 object references!
    enc.encode_value(&Value::dense_array(vec![
        shared_obj.clone(),
        shared_ba.clone(),
        shared_date.clone(),
    ]))
    .unwrap();
    // In `enc`, objectRef 0 = outer Array, objectRef 1 = shared_obj, objectRef 2 = shared_ba, objectRef 3 = shared_date.
    let mut bytes = enc.into_bytes();
    // Change outer array dense_count header at byte 1 from (3<<1)|1 = 0x07 to (6<<1)|1 = 0x0D,
    // and append refs to slot 1 (Object), slot 2 (ByteArray), slot 3 (Date):
    assert_eq!(bytes[1], 0x07);
    bytes[1] = 0x0D;
    bytes.extend_from_slice(&[
        MARKER_OBJECT,
        1 << 1, // ref 1
        MARKER_BYTE_ARRAY,
        2 << 1, // ref 2
        MARKER_DATE,
        3 << 1, // ref 3
    ]);
    let _ = &mut raw;

    let (dec, used) = amf3::decode(&bytes).unwrap();
    assert_eq!(used, bytes.len());
    assert_eq!(
        dec,
        Value::dense_array(vec![
            shared_obj.clone(),
            shared_ba.clone(),
            shared_date.clone(),
            shared_obj,
            shared_ba,
            shared_date,
        ])
    );
}

#[test]
fn test_13_amf3_cyclic_self_reference_and_invalid_refs_rejected_safely() {
    // Array at objectRef 0 whose first element references objectRef 0 (`[0x09, 0x03, 0x01, 0x09, 0x00]`)
    let cyclic = [0x09, 0x03, 0x01, 0x09, 0x00];
    let err = amf3::decode(&cyclic).unwrap_err();
    assert!(matches!(err, Amf3Error::CyclicObjectRef { index: 0, .. }));

    // Out-of-bounds stringRef 5 when table is empty
    let bad_str_ref = [0x06, 5 << 1];
    assert!(matches!(
        amf3::decode(&bad_str_ref).unwrap_err(),
        Amf3Error::InvalidStringRef { index: 5, .. }
    ));

    // Out-of-bounds objectRef 2
    let bad_obj_ref = [0x0A, 2 << 1];
    assert!(matches!(
        amf3::decode(&bad_obj_ref).unwrap_err(),
        Amf3Error::InvalidObjectRef { index: 2, .. }
    ));

    // Out-of-bounds traitRef 1 (`header = (1 << 2) | 1 = 0x05`)
    let bad_trait_ref = [0x0A, 0x05];
    assert!(matches!(
        amf3::decode(&bad_trait_ref).unwrap_err(),
        Amf3Error::InvalidTraitRef { index: 1, .. }
    ));

    // Unknown marker 0x7F
    assert!(matches!(
        amf3::decode(&[0x7F]).unwrap_err(),
        Amf3Error::InvalidMarker { marker: 0x7F, offset: 0 }
    ));
}

#[test]
fn test_14_amf3_recursion_depth_and_allocation_bounds() {
    // Construct 70 nested arrays `[0x09, 0x03, 0x01, ...]` exceeding default max_depth = 64
    let mut deep = Vec::new();
    for _ in 0..70 {
        deep.extend_from_slice(&[0x09, 0x03, 0x01]);
    }
    deep.push(0x01); // Null leaf
    let err = amf3::decode(&deep).unwrap_err();
    assert!(matches!(
        err,
        Amf3Error::RecursionLimitExceeded { max_depth: 64, .. }
    ));

    // Encoder also enforces max_depth
    let mut nested_val = Value::Null;
    for _ in 0..10 {
        nested_val = Value::dense_array(vec![nested_val]);
    }
    let mut enc_limited = Amf3Encoder::new().with_max_depth(5);
    assert!(matches!(
        enc_limited.encode_value(&nested_val).unwrap_err(),
        Amf3Error::RecursionLimitExceeded { max_depth: 5, .. }
    ));

    // Malicious 128 MiB ByteArray header (`len = 0x0800_0000` => header = `0x1000_0001`) with 0 payload bytes
    let mut spoofed = Amf3Encoder::new();
    let mut bad_ba = vec![MARKER_BYTE_ARRAY];
    spoofed.write_u29(((128 * 1024 * 1024) << 1) | 1);
    bad_ba.extend_from_slice(spoofed.as_bytes());
    let alloc_err = amf3::decode(&bad_ba).unwrap_err();
    assert!(matches!(
        alloc_err,
        Amf3Error::AllocationLimitExceeded { kind: "ByteArray", .. }
    ));

    // Sequential multi-value decoding via offset (`amf3.decode(buf, pos)`)
    let mut two_vals = amf3::encode(&Value::from("first")).unwrap();
    let second_offset = two_vals.len();
    two_vals.extend_from_slice(&amf3::encode(&Value::Int(999)).unwrap());
    let mut dec = Amf3Decoder::new(&two_vals);
    assert_eq!(dec.decode_value().unwrap(), Value::from("first"));
    assert_eq!(dec.position(), second_offset);
    assert_eq!(dec.decode_value().unwrap(), Value::Int(999));
    assert_eq!(dec.position(), two_vals.len());
}

// ============================================================================
// 3. RFC 1950 + RFC 1951 Zlib & Adler-32 Tests
// ============================================================================

#[test]
fn test_15_adler32_rfc_vectors_and_large_nmax_rollover() {
    assert_eq!(adler32(b""), 0x0000_0001);
    assert_eq!(adler32(b"a"), 0x0062_0062);
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);

    // 20,000 bytes (> 3 * ADLER_NMAX = 5552) deterministic rollover check
    let big = vec![0xFFu8; 20_000];
    let checksum = adler32(&big);
    let comp = zlib_compress_with_mode(&big, CompressionMode::Stored);
    assert_eq!(zlib_decompress(&comp).unwrap(), big);
    assert_eq!(
        u32::from_be_bytes(comp[comp.len() - 4..].try_into().unwrap()),
        checksum
    );
}

#[test]
fn test_16_zlib_roundtrip_stored_fixed_literals_and_fixed_lz77() {
    let payloads: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"X".to_vec(),
        b"Hello GServer vnsg2!".to_vec(),
        b"callbackName:activity4YearBonFire.refreshTotalNum, err:ok\n".repeat(40),
        // > 65535 bytes to exercise multi-block Stored splitting AND > 32768 LZ77 sliding window!
        (0..140_000usize)
            .map(|i| ((i * 7 + (i / 251)) & 0xFF) as u8)
            .collect(),
    ];

    for mode in [
        CompressionMode::Stored,
        CompressionMode::FixedLiteralsOnly,
        CompressionMode::FixedHuffmanLz77,
    ] {
        for (idx, payload) in payloads.iter().enumerate() {
            let compressed = zlib_compress_with_mode(payload, mode);
            let decompressed = zlib_decompress(&compressed).unwrap();
            assert_eq!(
                &decompressed, payload,
                "round-trip failed for mode {mode:?} payload #{idx} (len={})",
                payload.len()
            );
        }
    }

    // Verify LZ77 achieves strong compression on repetitive RPC data
    let rep = b"{\"method\":\"user.login\",\"serverID\":\"game2900161\",\"AMF\":3}".repeat(50);
    let comp = zlib_compress(&rep);
    assert!(comp.len() * 5 < rep.len(), "expected >5x LZ77 compression, got {} vs {}", comp.len(), rep.len());
}

#[test]
fn test_17_zlib_decompress_real_c_libz_vectors_stored_fixed_and_dynamic_huffman() {
    // Vector 1: C zlib level 9 empty
    let c_empty = decode_hex("78da030000000001");
    assert_eq!(zlib_decompress(&c_empty).unwrap(), b"");

    // Vector 2: C zlib level 0 stored block
    let c_stored = decode_hex("7801012100deff48656c6c6f204753657276657220766e7367322073746f72656420626c6f636b21cb020be0");
    assert_eq!(
        zlib_decompress(&c_stored).unwrap(),
        b"Hello GServer vnsg2 stored block!"
    );

    // Vector 3: C zlib single-symbol run ('A' * 512) with distance=1 overlapping RLE copy
    let c_single_run = decode_hex("78da73741c05231900004a9e8201");
    assert_eq!(zlib_decompress(&c_single_run).unwrap(), vec![b'A'; 512]);

    // Vector 4: C zlib two-symbol pattern ("AB" * 512) with distance=2 overlapping copy
    let c_two_sym = decode_hex("78da7374721c85a370148e500800a4b40610");
    assert_eq!(zlib_decompress(&c_two_sym).unwrap(), b"AB".repeat(512));

    // Vector 5: C zlib level 6 repeated GServer JSON
    let c_rpc_lvl6 = decode_hex(
        "789cab56ca4d2dc9c84f51b2522a2d4e2dd2cbc94fcfcc53d2514a4eccc9494a4cce46118f3702ca007965a9459e2e4099f4c4dc54234b0303433343a0447a517e69411e500828130c14d20bcb578828cdcf4b57aaad1eb565d49661610b00718c0678",
    );
    let expected_rpc =
        b"{\"method\":\"user.login\",\"callback\":\"user.login_2\",\"serverID\":\"game2900161\",\"groupname\":\"S161.Vo Xuong\"}".repeat(8);
    assert_eq!(zlib_decompress(&c_rpc_lvl6).unwrap(), expected_rpc);

    // Vector 6: C zlib level 9 BTYPE=2 Dynamic Huffman block over first 1536 bytes of real Network.lua!
    let c_dyn_net_lua = decode_hex(
        "78da8d54cb6adb40145d475f71b11791c016e9d660087553c8a20fdaec4a3163ebda169666d499711313b2e98386a62d813e4c6971bb4936a525909045d2929f899464d55fe88c5e9193e6a18db8cf73eeb957aa56ab46b50ab3721820dc45b9c878dff09933f0d004dbb6012a201089e79925cf6dd90d8f0851b22a0169f74917ed240696f19074500e1f1087f0fb9c2d0da10e1c9f0c5c8e66a94b7cb493a6c216a789f6d9a2925573a9302d4529a615bedc3e19fd3c3e1847ef360ef7d60ef7778dfedc42c363029db9a748a542994e59db8c268169c3f017581f29a828753dc3bfd7e908949935ef807e122b456a332a7149de218132c297cf8f76be1fedbf8fc62f0096db6ac0961ab702cb2b15907c802b86df480ad296caa279cba61a21ebde94ae8f42123f68fa2448bc50ad46a3dd93d14eb4baad9cc51c25ae6c0aa44e56efb16e33e02e9579e9f1afadf0cfc768b4111e8c32f630db19d0b67419854701e14a977a3955e5312836718a0a105fedd2eb1879b6165b7b2c0320a51dbf188f136b14174d1de328079cc63143b1bb166e5b327e21b00ea6c853fa65a71bab4369a694b9f2b5cd649e792dcc0d2333d325407d7925759dd35b4714d7245a5453058a9384fb7b473fd6924bbb7232b56d6ccbb3c39d9a6521154a177a4c14b3ca74e0b79043c078e24e652db718f3905008379f45e3af098968753d7cfd2d5cfd7c7cf021fc32d667f7f7f79b707d3377758827b02069c22a56b5a2912b1ac72a68a5e56da453d4444f8de5a49669e95537920eaa99395156e3d87585447e9339c3db0acfcc40cd96f254a087c451155370e9135f14a3bac92d22c964ed65856a4f96f15f42f1073fc9e8da3c929f85790576069eaeaac021d33b961a12adb38b8293576fa34f5bd73b278e171c545e93679c5f2e80db318b1b8eff1316c81ed274b202f549d2bac53f18ad0f7a",
    );
    assert_eq!((c_dyn_net_lua[2] >> 1) & 3, 2, "must be BTYPE=2 dynamic Huffman");
    let inflated = zlib_decompress(&c_dyn_net_lua).unwrap();
    assert_eq!(inflated.len(), 1536);
    assert_eq!(adler32(&inflated), 0x18AD_0F7A);
    assert!(inflated.starts_with(b"---\n-- @type Network\nmodule( ..."));
}

#[test]
fn test_18_zlib_error_cases_corrupt_header_adler_backref_and_bomb_limit() {
    // Too short
    assert_eq!(zlib_decompress(b"789c"), Err(ZlibError::UnexpectedEof));

    // Bad FCHECK
    assert!(matches!(
        zlib_decompress(&[0x78, 0x9D, 0x03, 0x00, 0, 0, 0, 1]),
        Err(ZlibError::InvalidZlibHeader { cmf: 0x78, flg: 0x9D })
    ));

    // FDICT = 1 (0x78BB: 0x78*256 + 0xBB = 30907 = 31 * 997, bit 5 set)
    assert_eq!(
        zlib_decompress(&[0x78, 0xBB, 0x03, 0x00, 0, 0, 0, 1]),
        Err(ZlibError::PresetDictionaryUnsupported)
    );

    // Reserved BTYPE = 3 (`0b111` = 0x07)
    assert_eq!(
        zlib_decompress(&[0x78, 0x9C, 0x07, 0x00, 0, 0, 0, 1]),
        Err(ZlibError::InvalidBlockType)
    );

    // Stored block LEN/NLEN mismatch
    assert!(matches!(
        zlib_decompress(&[0x78, 0x01, 0x01, 0x04, 0x00, 0x00, 0x00, 1, 2, 3, 4]),
        Err(ZlibError::StoredLengthMismatch { .. })
    ));

    // Corrupted Adler-32 trailer
    let mut valid = zlib_compress(b"integrity test");
    let last = valid.len() - 1;
    valid[last] ^= 0xFF;
    assert!(matches!(
        zlib_decompress(&valid),
        Err(ZlibError::Adler32Mismatch { .. })
    ));

    // Decompression bomb cap enforcement (`max_output_size = 256` on a 512-byte stream)
    let run512 = decode_hex("78da73741c05231900004a9e8201");
    assert_eq!(
        zlib_decompress_with_limit(&run512, 256),
        Err(ZlibError::OutputLimitExceeded {
            max_output_size: 256
        })
    );
}

// ============================================================================
// 4. RFC 1321 MD5, BabelTime Salted MD5 & 0xFEFEFEFE bt_decrypt Tests
// ============================================================================

#[test]
fn test_19_md5_rfc1321_and_babeltime_salted_xor_vectors() {
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(md5_hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
    assert_eq!(
        md5_hex(b"abcdefghijklmnopqrstuvwxyz"),
        "c3fcd3d76192e4007dfb496cca67e13b"
    );
    assert_eq!(
        md5_hex(b"1234567890".repeat(8)),
        "57edf4a22be3c955ac49da2e2107b67a"
    );

    // Empty body in LCPacket::computeBabelTimeMd5 hashes just "BabelTime"
    assert_eq!(compute_babeltime_md5(b""), md5_digest(b"BabelTime"));

    // Cross-verified against Python implementation of LCPacket.cpp formula
    let sample_body = decode_hex("789c0b0e010000");
    assert_eq!(
        compute_babeltime_md5(&sample_body),
        decode_hex("8dfca961ddeaf530144430a3d0ba7e5f").as_slice()
    );
}

#[test]
fn test_20_bt_decrypt_all_16_keys_and_header_validation() {
    let sample_lua = b"module(..., seeall(\"lib.Class\"), package.seeall)\nlocal x = 42\n";
    for key_idx in 0..=15u8 {
        let encrypted = bt_encrypt(sample_lua, key_idx).unwrap();
        assert!(has_bt_magic(&encrypted));
        assert_eq!(encrypted.len(), 32 + sample_lua.len());
        let decrypted = bt_decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, sample_lua, "failed for key slot {key_idx}");
    }

    // Passthrough on plaintext Lua
    assert_eq!(bt_decrypt_or_passthrough(sample_lua).unwrap(), sample_lua);

    // Truncated 0xFEFEFEFE buffer rejected even in passthrough mode
    assert!(matches!(
        bt_decrypt_or_passthrough(&[0xFE, 0xFE, 0xFE, 0xFE, 1, 2]),
        Err(BtDecryptError::TruncatedHeader { len: 6 })
    ));

    // Invalid key slot 16
    assert_eq!(
        bt_encrypt(sample_lua, 16),
        Err(BtDecryptError::InvalidKeyIndex(16))
    );

    // Bad payload length in header
    let mut tampered = bt_encrypt(b"hello", 15).unwrap();
    tampered[4] = 99;
    assert!(matches!(
        bt_decrypt(&tampered),
        Err(BtDecryptError::PayloadLengthMismatch { .. })
    ));
}

// ============================================================================
// 5. C2S Framing (`ClientRxDecoder`) & S2C Framing (`ServerTxEncoder`/`ServerRxDecoder`)
// ============================================================================

#[test]
fn test_21_c2s_lcpacket24_compressed_and_uncompressed_roundtrip() {
    let req = RpcRequest::with_seq_id(
        "user.getUserBeforLogin",
        3,
        Vec::new(),
        "SID_session_token_abc",
    );
    let raw_amf3 = req.to_amf3_bytes().unwrap();

    for compress in [true, false] {
        let wire = encode_c2s_lcpacket(&raw_amf3, compress).unwrap();
        assert!(wire.len() > C2S_LCPACKET_HEADER_SIZE);
        assert_eq!(wire[4..8], [u8::from(compress), 1, 1, 1]);

        let mut rx = ClientRxDecoder::new(C2sFrameMode::LcPacket24);
        rx.feed(&wire);
        let frame = rx.next_frame().unwrap().unwrap();
        assert_eq!(rx.buffered_len(), 0);
        assert!(matches!(
            frame.kind,
            C2sWireKind::LcPacket24 { compressed, .. } if compressed == compress
        ));
        assert_eq!(frame.amf3_payload, raw_amf3);

        let parsed = RpcRequest::from_amf3_bytes(&frame.amf3_payload).unwrap();
        assert_eq!(parsed, req);
    }
}

#[test]
fn test_22_c2s_raw_4byte_and_auto_mode_pipelined_1byte_streaming() {
    let req1 = RpcRequest::with_seq_id("user.login", 2, vec![Value::from("uuid-1")], "0");
    let req2 = RpcRequest::new(
        "user.stat",
        vec![Value::Int(101)],
        "tok_2",
        "user.stat",
    );
    let req3 = RpcRequest::with_seq_id("user.userLogin", 4, vec![Value::Int(52592)], "tok_2");

    // Mix 24-byte compressed LCPacket, raw 4-byte frame, and 24-byte uncompressed LCPacket
    // on the same stream in `C2sFrameMode::Auto`, feeding 1 byte at a time!
    let f1 = req1.to_lcpacket_frame(true).unwrap();
    let f2 = encode_c2s_raw_frame(&req2.to_amf3_bytes().unwrap()).unwrap();
    let f3 = encode_c2s_lcpacket_with_mode(
        &req3.to_amf3_bytes().unwrap(),
        false,
        CompressionMode::Stored,
    )
    .unwrap();

    let mut stream = Vec::new();
    stream.extend_from_slice(&f1);
    stream.extend_from_slice(&f2);
    stream.extend_from_slice(&f3);

    let mut rx = ClientRxDecoder::new(C2sFrameMode::Auto);
    let mut decoded_reqs = Vec::new();
    for &byte in &stream {
        rx.feed(&[byte]);
        while let Some(frame) = rx.next_frame().unwrap() {
            decoded_reqs.push(RpcRequest::from_amf3_bytes(&frame.amf3_payload).unwrap());
        }
    }

    assert_eq!(decoded_reqs, vec![req1, req2, req3]);
    assert_eq!(rx.buffered_len(), 0);
}

#[test]
fn test_23_c2s_tampered_md5_bad_flags_and_size_caps_rejected() {
    let req = RpcRequest::with_seq_id("arena.getInfo", 5, vec![], "tok");
    let mut wire = req.to_lcpacket_frame(true).unwrap();

    // Flip 1 bit in body -> BabelTime MD5 mismatch!
    let last = wire.len() - 1;
    wire[last] ^= 0x01;
    let mut rx = ClientRxDecoder::new(C2sFrameMode::LcPacket24);
    rx.feed(&wire);
    assert!(matches!(
        rx.next_frame(),
        Err(ProtocolError::Frame(FrameError::Md5Mismatch { .. }))
    ));

    // Invalid LCPacket flags [2, 1, 1, 1]
    let mut bad_flags = req.to_lcpacket_frame(true).unwrap();
    bad_flags[4] = 2;
    let mut rx2 = ClientRxDecoder::new(C2sFrameMode::LcPacket24);
    rx2.feed(&bad_flags);
    assert_eq!(
        rx2.next_frame(),
        Err(ProtocolError::Frame(FrameError::InvalidLcFlags([2, 1, 1, 1])))
    );

    // Zero body length rejected (`CConnection::getBodyLength` contract)
    let mut rx3 = ClientRxDecoder::default();
    rx3.feed(&[0, 0, 0, 0, 1, 1, 1, 1]);
    assert_eq!(
        rx3.next_frame(),
        Err(ProtocolError::Frame(FrameError::ZeroBodyLength))
    );

    // Oversized bodyLen rejected immediately upon reading 4-byte length prefix
    let mut rx4 = ClientRxDecoder::default().with_limits(1024, 4096);
    rx4.feed(&2048u32.to_be_bytes());
    assert_eq!(
        rx4.next_frame(),
        Err(ProtocolError::Frame(FrameError::BodyTooLarge {
            body_len: 2048,
            max_body_len: 1024,
        }))
    );
}

#[test]
fn test_24_s2c_8byte_header_layout_matches_lua_string_byte_6_7() {
    // Verify exact 1-indexed `string.byte(head, 6, 7)` => 0-indexed `head[5] = zipped, head[6] = multi`!
    let tx = ServerTxEncoder::new();
    let env = S2cEnvelope::ok_response("user.userLogin_4", Value::from("ok"))
        .with_token("tok_99")
        .with_time(1_795_500_451);

    let frame_plain = encode_s2c_single_frame(&tx, &env, false).unwrap();
    assert_eq!(&frame_plain[4..8], &[0, 0, 0, 0]);
    let declared_len = u32::from_be_bytes(frame_plain[0..4].try_into().unwrap()) as usize;
    assert_eq!(declared_len + S2C_HEADER_SIZE, frame_plain.len());

    let frame_zipped = encode_s2c_single_frame(&tx, &env, true).unwrap();
    assert_eq!(&frame_zipped[4..8], &[0, 1, 0, 0]); // head[5] (Lua 6) == 1, head[6] (Lua 7) == 0

    let frame_multi = encode_s2c_multi_frame(
        &tx,
        std::slice::from_ref(&env),
        Some("tok_99"),
        Some(1_795_500_451),
        false,
        MultiItemEncoding::NestedAmf3ByteString,
    )
    .unwrap();
    assert_eq!(&frame_multi[4..8], &[0, 0, 1, 0]); // head[5] == 0, head[6] == 1

    let frame_zip_multi = encode_s2c_multi_frame(
        &tx,
        std::slice::from_ref(&env),
        Some("tok_99"),
        Some(1_795_500_451),
        true,
        MultiItemEncoding::NestedAmf3ByteString,
    )
    .unwrap();
    assert_eq!(&frame_zip_multi[4..8], &[0, 1, 1, 0]); // head[5] == 1, head[6] == 1
}

#[test]
fn test_25_s2c_all_four_flag_modes_and_multi_encodings_streaming_roundtrip() {
    let tx = ServerTxEncoder::new();

    let single_ok = S2cEnvelope::ok_response(
        "activity4YearBonFire.refreshTotalNum_12",
        Value::map([("total", Value::Int(8888))]),
    );
    let push_mail = S2cEnvelope::push(
        "re.mail.newMail",
        Value::map([
            ("mid", Value::Int(701)),
            ("sender", Value::from("Hệ Thống")),
        ]),
    );
    let push_kick = S2cEnvelope::push("re.userLogin.otherDeviceLogin", Value::empty_array());
    let dummy_env = S2cEnvelope::ok_response("dummy", Value::Null);
    let ping_env = S2cEnvelope::ping();

    assert!(push_mail.is_push_topic());
    assert!(push_kick.is_push_topic());
    assert!(ping_env.is_ping());
    assert!(!single_ok.is_push_topic());

    // Build pipelined stream containing:
    // 1. single plain
    // 2. single zipped (ping)
    // 3. multi plain (NestedAmf3ByteString)
    // 4. multi zipped (NestedAmf3ByteArray)
    // 5. multi zipped (InlineObject)
    // 6. multi zipped (LengthPrefixed [4B BE subLen][AMF3])
    let batch_items = vec![
        single_ok.clone(),
        push_mail.clone(),
        push_kick.clone(),
        dummy_env.clone(),
    ];

    let mut wire = Vec::new();
    wire.extend_from_slice(&encode_s2c_single_frame(&tx, &single_ok, false).unwrap());
    wire.extend_from_slice(&encode_s2c_single_frame(&tx, &ping_env, true).unwrap());
    for (sess, ts, zipped, enc_mode) in [
        ("sess_A", 1_795_500_500, false, MultiItemEncoding::NestedAmf3ByteString),
        ("sess_B", 1_795_500_501, true, MultiItemEncoding::NestedAmf3ByteArray),
        ("sess_C", 1_795_500_502, true, MultiItemEncoding::InlineObject),
    ] {
        wire.extend_from_slice(
            &encode_s2c_multi_frame(&tx, &batch_items, Some(sess), Some(ts), zipped, enc_mode)
                .unwrap(),
        );
    }

    let sub1 = single_ok.to_amf3_bytes().unwrap();
    let sub2 = push_mail.to_amf3_bytes().unwrap();
    wire.extend_from_slice(&tx.encode_multi_length_prefixed(&[&sub1, &sub2], true).unwrap());

    // Stream 3 bytes at a time into ServerRxDecoder
    let mut rx = ServerRxDecoder::new();
    let mut frames = Vec::new();
    for chunk in wire.chunks(3) {
        rx.feed(chunk);
        frames.extend(rx.decode_all().unwrap());
    }
    assert_eq!(frames.len(), 6);
    assert_eq!(rx.buffered_len(), 0);

    // Frame 0: single plain
    assert_eq!(frames[0].flags, S2cFlags::new(false, false));
    let b0 = decode_s2c_frame_batch(&frames[0]).unwrap();
    assert_eq!(b0.items, vec![single_ok.clone()]);

    // Frame 1: single zipped ping
    assert_eq!(frames[1].flags, S2cFlags::new(true, false));
    let b1 = decode_s2c_frame_batch(&frames[1]).unwrap();
    assert_eq!(b1.items.len(), 1);
    assert!(b1.items[0].is_ping());

    // Frames 2, 3, 4: multi batches across all 3 Network.lua item encodings
    for (idx, expected_tok, expected_time) in [
        (2usize, "sess_A", 1_795_500_500i64),
        (3usize, "sess_B", 1_795_500_501i64),
        (4usize, "sess_C", 1_795_500_502i64),
    ] {
        let batch = decode_s2c_frame_batch(&frames[idx]).unwrap();
        assert_eq!(batch.token.as_deref(), Some(expected_tok));
        assert_eq!(batch.time, Some(expected_time));
        assert_eq!(batch.items, batch_items);
    }

    // Frame 5: length-prefixed raw sub-packets fallback
    assert_eq!(frames[5].flags, S2cFlags::new(true, true));
    let b5 = decode_s2c_frame_batch(&frames[5]).unwrap();
    assert_eq!(b5.items, vec![single_ok, push_mail]);
}

// ============================================================================
// 6. Full Live-Capture `vnsg2` Login Handshake & Error Envelope Tests
// ============================================================================

#[test]
fn test_26_full_vnsg2_live_capture_login_sequence_roundtrip() {
    // Reconstruct the exact CS_USER_LOGIN ("user.login") packet captured in live_capture_filtered.log:
    let record_item = Value::map([
        ("device_model", Value::from("iphone")),
        ("platform", Value::from("android")),
        ("timestamp", Value::from(1_795_586_850i64)),
        ("userID", Value::from("1243831974477471744")),
    ]);
    let login_hash = Value::map([
        ("AMF", Value::Int(3)),
        ("b_fightLog", Value::Int(1)),
        ("canCreateUser", Value::Int(1)),
        ("canPay", Value::Int(1)),
        ("errordesc", Value::from("OK")),
        ("errornu", Value::from("0")),
        ("feignRoleId", Value::from("487932b")),
        ("hash", Value::from("5461251032ff30a14951332a67ff9ed7")),
        ("host", Value::from("61.28.240.52")),
        ("mergeServerTime", Value::from(1_667_530_800i64)),
        ("offset", Value::from("-420")),
        ("openDateTime", Value::from("2018-11-06-09-00-00")),
        ("pid", Value::from("636df6782106606d")),
        ("plname", Value::from("android")),
        ("plosgn", Value::Int(2)),
        ("port", Value::Int(8160)),
        ("ptype", Value::Int(0)),
        ("qid", Value::from("1243831974477471744")),
        ("record", Value::dense_array(vec![record_item])),
        ("serverID", Value::from("game2900161")),
        ("timestamp", Value::from(1_795_500_450i64)),
        ("uuid", Value::from("21a23244ed44810da2d40c6d4455e686")),
    ]);

    // Step 1: C2S `user.login` (mId increments from 1 -> 2 in Network:sendMsg)
    let c2s_login = RpcRequest::with_seq_id(
        "user.login",
        2,
        vec![
            login_hash.clone(),
            Value::from("21a23244ed44810da2d40c6d4455e686"),
            Value::from("1.0.1194"),
        ],
        "0",
    );
    let wire_c2s = c2s_login.to_lcpacket_frame(true).unwrap();

    let mut server_rx = ClientRxDecoder::new(C2sFrameMode::Auto);
    server_rx.feed(&wire_c2s);
    let c2s_frame = server_rx.next_frame().unwrap().unwrap();
    let decoded_login = RpcRequest::from_amf3_bytes(&c2s_frame.amf3_payload).unwrap();
    assert_eq!(decoded_login.method, "user.login");
    assert_eq!(decoded_login.callback_name, "user.login_2");
    assert_eq!(decoded_login.token, "0");
    assert_eq!(decoded_login.args.len(), 3);
    assert_eq!(
        decoded_login.args[0].get_field("serverID"),
        Some(&Value::from("game2900161"))
    );
    assert_eq!(
        decoded_login.args[0].get_field("timestamp").and_then(Value::as_i64),
        Some(1_795_500_450)
    );

    // Step 2: S2C response batch (`dummy` + `user.login_2` with new session token + server time)
    let server_tx = ServerTxEncoder::new();
    let s2c_wire = encode_s2c_multi_frame(
        &server_tx,
        &[
            S2cEnvelope::ok_response("dummy", Value::Null),
            S2cEnvelope::ok_response("user.login_2", Value::from("ok")),
        ],
        Some("vnsg2_session_9f8e7d"),
        Some(1_795_500_451),
        true,
        MultiItemEncoding::NestedAmf3ByteString,
    )
    .unwrap();

    let mut client_rx = ServerRxDecoder::new();
    client_rx.feed(&s2c_wire);
    let s2c_frame = client_rx.next_frame().unwrap().unwrap();
    let batch = decode_s2c_frame_batch(&s2c_frame).unwrap();
    assert_eq!(batch.token.as_deref(), Some("vnsg2_session_9f8e7d"));
    assert_eq!(batch.time, Some(1_795_500_451));
    assert_eq!(batch.items[0].callback_name.as_deref(), Some("dummy"));
    assert_eq!(batch.items[1].callback_name.as_deref(), Some("user.login_2"));
}

#[test]
fn test_27_envelope_validation_and_error_responses() {
    // Error response (e.g., banned / maintenance / invalid session)
    let err_env = S2cEnvelope::error_response("user.userLogin_4", "ban:1795600000:cheat");
    let bytes = err_env.to_amf3_bytes().unwrap();
    let parsed = S2cEnvelope::from_amf3_bytes(&bytes).unwrap();
    assert_eq!(parsed.err, "ban:1795600000:cheat");
    assert_eq!(parsed.callback_name.as_deref(), Some("user.userLogin_4"));

    // Empty multi batch rejected (Network.lua `if(#rets == 0) then Logger.showFatal(...)`)
    let tx = ServerTxEncoder::new();
    assert_eq!(
        encode_s2c_multi_frame(&tx, &[], None, None, false, MultiItemEncoding::InlineObject),
        Err(ProtocolError::Envelope(EnvelopeError::EmptyMultiBatch))
    );

    // Missing `method` in RpcRequest rejected
    let bad_req = Value::map([("token", Value::from("0"))]);
    assert_eq!(
        RpcRequest::from_amf3_value(&bad_req),
        Err(ProtocolError::Envelope(EnvelopeError::MissingField("method")))
    );

    // Non-ping S2C envelope missing `callback` rejected
    let bad_s2c = Value::map([("err", Value::from("ok"))]);
    assert_eq!(
        S2cEnvelope::from_amf3_value(&bad_s2c),
        Err(ProtocolError::Envelope(EnvelopeError::MissingField("callback")))
    );
}

#[test]
fn test_28_split_length_prefixed_subpackets_edge_cases() {
    assert_eq!(
        split_length_prefixed_subpackets(&[]),
        Err(ProtocolError::Frame(FrameError::ZeroBodyLength))
    );
    // Truncated 2-byte length prefix
    assert!(matches!(
        split_length_prefixed_subpackets(&[0x00, 0x05]),
        Err(ProtocolError::Frame(FrameError::InvalidMultiSubPacket { .. }))
    ));
    // Declared subLen > remaining
    assert!(matches!(
        split_length_prefixed_subpackets(&[0x00, 0x00, 0x00, 0x10, 1, 2, 3]),
        Err(ProtocolError::Frame(FrameError::InvalidMultiSubPacket {
            declared_len: 16,
            remaining: 3,
            ..
        }))
    ));
}

#[test]
fn test_29_associative_string_keyed_bingo_map_from_live_capture() {
    // In live_capture_filtered.log lines 700..739, `task` map has string keys "1".."19" -> Int(0..110)
    let mut task_obj = Amf3Object::anonymous_dynamic();
    for k in 1..=19 {
        let v = if k >= 8 { 110 } else { k % 2 };
        task_obj.insert(k.to_string(), Value::Int(v));
    }
    let bingo = Value::map([
        ("grid_light", Value::empty_array()),
        ("grid_reward", Value::empty_array()),
        ("star1_num", Value::Int(0)),
        ("star2_num", Value::Int(0)),
        ("task", Value::Object(task_obj)),
    ]);

    let tx = ServerTxEncoder::new();
    let env = S2cEnvelope::ok_response("bingo.getInfo_15", bingo.clone());
    let wire = encode_s2c_single_frame(&tx, &env, true).unwrap();

    let mut rx = ServerRxDecoder::new();
    rx.feed(&wire);
    let frame = rx.next_frame().unwrap().unwrap();
    let batch = decode_s2c_frame_batch(&frame).unwrap();
    assert_eq!(batch.items[0].ret, bingo);
}

#[test]
fn test_30_c2s_auto_mode_single_byte_primitives_and_unrecognized_header() {
    // Raw 4-byte frame containing 1-byte AMF3 Null (`[0,0,0,1, 0x01]`)
    let raw_null = encode_c2s_raw_frame(&[0x01]).unwrap();
    let mut rx = ClientRxDecoder::new(C2sFrameMode::Auto);
    rx.feed(&raw_null);
    let f = rx.next_frame().unwrap().unwrap();
    assert_eq!(f.kind, C2sWireKind::Raw4Byte);
    assert_eq!(amf3::decode(&f.amf3_payload).unwrap().0, Value::Null);

    // Invalid C2S header byte 0xFF after length prefix in Auto mode
    let mut rx_bad = ClientRxDecoder::new(C2sFrameMode::Auto);
    rx_bad.feed(&[0, 0, 0, 10, 0xFF, 0, 0, 0]);
    assert!(matches!(
        rx_bad.next_frame(),
        Err(ProtocolError::Frame(FrameError::UnrecognizedC2sHeader(_)))
    ));
}

#[test]
fn test_31_server_rx_limits_and_buffer_compaction_across_many_frames() {
    let tx = ServerTxEncoder::new();
    let mut rx = ServerRxDecoder::new().with_limits(8192, 16384);

    // Push 200 frames sequentially (> 4096 bytes total) to exercise buffer compaction
    for i in 0..200 {
        let env = S2cEnvelope::ok_response(
            format!("bag.bagInfo_{i}"),
            Value::map([("slot", Value::Int(i)), ("item_tid", Value::Int(60000 + i))]),
        );
        let wire = encode_s2c_single_frame(&tx, &env, i % 2 == 0).unwrap();
        rx.feed(&wire);
        let f = rx.next_frame().unwrap().unwrap();
        let b = decode_s2c_frame_batch(&f).unwrap();
        assert_eq!(b.items[0], env);
    }
    assert_eq!(rx.buffered_len(), 0);
}

#[test]
fn test_32_server_rx_decompression_bomb_rejected() {
    let tx = ServerTxEncoder::new();
    let huge = S2cEnvelope::ok_response(
        "huge_1",
        Value::ByteArray(vec![0xAA; 8192]),
    );
    // Compresses down to < 100 bytes on the wire, but expands > 1024 decompressed limit!
    let wire = encode_s2c_single_frame(&tx, &huge, true).unwrap();
    assert!(wire.len() < 256);

    let mut rx = ServerRxDecoder::new().with_limits(4096, 1024);
    rx.feed(&wire);
    assert_eq!(
        rx.next_frame(),
        Err(ProtocolError::Zlib(ZlibError::OutputLimitExceeded {
            max_output_size: 1024
        }))
    );
}

#[test]
fn test_33_c_zlib_interop_binary_all_byte_values() {
    // C zlib level 9 over 2048 bytes containing 0x00..=0xFF repeated 8 times
    let c_bin = decode_hex(
        "78da6360646266616563e7e0e4e2e6e1e5e3171014121611151397909492969195935750545256515553d7d0d4d2d6d1d5d33730343236313533b7b0b4b2b6b1b5b37770747276717573f7f0f4f2f6f1f5f30f080c0a0e090d0b8f888c8a8e898d8b4f484c4a4e494d4bcfc8cccacec9cdcb2f282c2a2e292d2bafa8acaaaea9adab6f686c6a6e696d6befe8eceaeee9edeb9f3071d2e42953a74d9f3173d6ec3973e7cd5fb070d1e2254b972d5fb172d5ea356bd7addfb071d3e62d5bb76ddfb173d7ee3d7bf7ed3f70f0d0e123478f1d3f71f2d4e93367cf9dbf70f1d2e52b57af5dbf71f3d6ed3b77efdd7ff0f0d1e3274f9f3d7ff1f2d5eb376fdfbdfff0f1d3e72f5fbf7dfff1f3d7ef3f7ffffd6718f5ffa8ff47fd3feaff51ff8ffa7f04fa1f004138fc2e",
    );
    let expected: Vec<u8> = (0..2048).map(|i| (i & 0xFF) as u8).collect();
    assert_eq!(zlib_decompress(&c_bin).unwrap(), expected);

    // And compress the exact same 2048 bytes with our Rust FixedHuffmanLz77 and verify roundtrip
    let rust_comp = zlib_compress(&expected);
    assert!(rust_comp.len() < 400);
    assert_eq!(zlib_decompress(&rust_comp).unwrap(), expected);
}

#[test]
fn test_34_amf3_associative_array_envelope_compatibility() {
    // Verify RpcRequest and S2cEnvelope also accept associative AMF3 Array maps (0x09)
    let assoc_req = Value::Array {
        dense: Vec::new(),
        assoc: vec![
            (String::from("method"), Value::from("user.getUser")),
            (String::from("args"), Value::empty_array()),
            (String::from("token"), Value::from("tok_assoc")),
            (
                String::from("callback"),
                Value::Array {
                    dense: Vec::new(),
                    assoc: vec![(String::from("callbackName"), Value::from("user.getUser_7"))],
                },
            ),
        ],
    };
    let parsed_req = RpcRequest::from_amf3_value(&assoc_req).unwrap();
    assert_eq!(parsed_req.method, "user.getUser");
    assert_eq!(parsed_req.callback_name, "user.getUser_7");
    assert_eq!(parsed_req.token, "tok_assoc");

    let assoc_resp = Value::Array {
        dense: Vec::new(),
        assoc: vec![
            (String::from("err"), Value::from("ok")),
            (String::from("callback"), Value::from("re.chat.getMsg")),
            (String::from("ret"), Value::from("hello world")),
        ],
    };
    let parsed_resp = S2cEnvelope::from_amf3_value(&assoc_resp).unwrap();
    assert!(parsed_resp.is_push_topic());
    assert_eq!(parsed_resp.ret, Value::from("hello world"));
}

#[test]
fn test_35_error_display_and_std_error_source_implementations() {
    let errors: Vec<ProtocolError> = vec![
        Amf3Error::UnexpectedEof {
            offset: 3,
            needed: 8,
            remaining: 2,
        }
        .into(),
        ZlibError::InvalidBlockType.into(),
        FrameError::ZeroBodyLength.into(),
        EnvelopeError::EmptyMultiBatch.into(),
        BtDecryptError::InvalidKeyIndex(99).into(),
    ];
    for e in errors {
        let msg = e.to_string();
        assert!(!msg.is_empty());
        assert!(std::error::Error::source(&e).is_some());
    }
}

#[test]
fn test_36_generic_framed_stream_decoder_and_define_error_macro() {
    use gserver_protocol::{define_error, C2sFrame, FramedStreamDecoder, S2cFrame, WireFrame};

    define_error! {
        enum CustomLeafError {
            Empty => "leaf empty",
            Code(code: u16) => "leaf code 0x{code:04X}",
            Span { pos: usize, tag: &'static str } => "leaf {tag}@{pos}",
        }
        enum CustomOuterError {
            Leaf(#[source] e: CustomLeafError) => "wrapped: {e}",
        }
    }

    let leaf = CustomLeafError::Code(0x12AB);
    assert_eq!(leaf.to_string(), "leaf code 0x12AB");
    assert!(std::error::Error::source(&leaf).is_none());
    assert_eq!(CustomLeafError::Empty.to_string(), "leaf empty");
    assert_eq!(
        CustomLeafError::Span { pos: 7, tag: "hdr" }.to_string(),
        "leaf hdr@7"
    );

    let outer: CustomOuterError = CustomLeafError::Empty.into();
    assert_eq!(outer.to_string(), "wrapped: leaf empty");
    assert!(std::error::Error::source(&outer).is_some());

    // Verify leaf Display strings and Error::source() == None on all 5 leaf protocol error enums
    let amf_e = Amf3Error::InvalidMarker { marker: 0xAB, offset: 9 };
    assert_eq!(amf_e.to_string(), "AMF3 unsupported type marker 0xAB at offset 9");
    assert!(std::error::Error::source(&amf_e).is_none());

    let frame_md5 = FrameError::Md5Mismatch {
        expected: [0; 16],
        actual: [1; 16],
    };
    assert_eq!(
        frame_md5.to_string(),
        "BabelTime LCPacket 16-byte MD5 integrity mismatch"
    );
    assert_eq!(
        FrameError::InvalidLcFlags([2, 1, 1, 1]).to_string(),
        "invalid LCPacket flags [2, 1, 1, 1] (expected [0|1, 1, 1, 1])"
    );
    assert_eq!(
        FrameError::UnrecognizedC2sHeader([0xDE, 0xAD, 0, 1, 2, 3, 4, 5]).to_string(),
        "unrecognized C2S frame header prefix [DE, AD, 00, 01, 02, 03, 04, 05]"
    );

    // Verify ClientRxDecoder & ServerRxDecoder are parameterized over FramedStreamDecoder<F>
    let mut c2s_dec: FramedStreamDecoder<C2sFrame> = ClientRxDecoder::new(C2sFrameMode::Raw4Byte);
    c2s_dec.feed(&encode_c2s_raw_frame(&[0x01]).unwrap());
    assert_eq!(c2s_dec.decode_all().unwrap().len(), 1);

    let tx = ServerTxEncoder::new();
    let mut s2c_dec: FramedStreamDecoder<S2cFrame> = ServerRxDecoder::new();
    s2c_dec.feed(&encode_s2c_single_frame(&tx, &S2cEnvelope::ping(), false).unwrap());
    assert_eq!(s2c_dec.decode_all().unwrap().len(), 1);

    // Custom WireFrame implementation over FramedStreamDecoder<F>
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RawTestFrame(Vec<u8>);
    impl WireFrame for RawTestFrame {
        type Config = ();
        const MIN_HEADER_LEN: usize = 4;
        fn decode_wire_frame(
            avail: &[u8],
            body_len: u32,
            _cfg: &(),
            _max_decomp: usize,
        ) -> Result<Option<(Self, usize)>, ProtocolError> {
            let needed = 4 + body_len as usize;
            if avail.len() < needed {
                return Ok(None);
            }
            Ok(Some((Self(avail[4..needed].to_vec()), needed)))
        }
    }
    let mut custom = FramedStreamDecoder::<RawTestFrame>::default().with_limits(64, 64);
    custom.feed(&[0, 0, 0, 3, b'x', b'y']);
    assert_eq!(custom.next_frame().unwrap(), None);
    custom.feed(b"z");
    assert_eq!(custom.next_frame().unwrap(), Some(RawTestFrame(b"xyz".to_vec())));
    assert_eq!(custom.buffered_len(), 0);
}
