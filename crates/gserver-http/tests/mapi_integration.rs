#![allow(clippy::field_reassign_with_default)]
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gserver_http::{
    compute_mapi_signature, compute_guest_auth_sig, compute_account_auth_sig, hmac_md5_hex, md5_hex,
    parse_json, sign_mapi_url, sort_url_params, start_mapi_server, verify_mapi_query_signature,
    GameServerEntry, JsonNumber, JsonObject, JsonValue, RoleSummary, ScriptPatchEntry,
    ServerConfig, ServerStatus, VersionPolicy, COOKIE_GAME_SESSION, COOKIE_PHPSESSID,
    DEFAULT_FALLBACK_SIGN_KEY, DEFAULT_GTSDK_GAME_ID, DEFAULT_AUTH_SECRET, VNSG2_MAPI_APP_KEY,
};
use gserver_http::url::{
    form_decode, normalize_path, parse_cookie_header, parse_query_params, percent_decode,
    percent_encode, split_request_target,
};

// ===========================================================================
// Raw Loopback HTTP/1.1 Test Client Helpers
// ===========================================================================

#[derive(Debug)]
struct RawHttpResp {
    status: u16,
    headers: Vec<(String, String)>,
    cookies: BTreeMap<String, String>,
    body: String,
}

impl RawHttpResp {
    fn json(&self) -> JsonValue {
        parse_json(&self.body).unwrap_or_else(|e| {
            panic!(
                "Failed to parse HTTP response body as JSON ({}): {:?}",
                e, self.body
            )
        })
    }
}

fn send_raw_http_on_stream(
    stream: &mut TcpStream,
    method: &str,
    target: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> RawHttpResp {
    let mut req = format!("{} {} HTTP/1.1\r\nHost: 127.0.0.1\r\n", method, target);
    if !body.is_empty() {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for &(k, v) in extra_headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");

    stream.write_all(req.as_bytes()).expect("write request head");
    if !body.is_empty() {
        stream.write_all(body).expect("write request body");
    }
    stream.flush().expect("flush request");

    read_http_response(stream, method.eq_ignore_ascii_case("HEAD"))
}

fn read_http_response(stream: &mut TcpStream, head_only: bool) -> RawHttpResp {
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .expect("read HTTP status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("HTTP status code token")
        .parse()
        .expect("parse HTTP status code");

    let mut headers = Vec::new();
    let mut cookies = BTreeMap::new();
    let mut content_length = 0usize;

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read header line");
        let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
        if trimmed.is_empty() {
            break;
        }
        let colon = trimmed.find(':').expect("header colon");
        let name = trimmed[..colon].trim().to_string();
        let val = trimmed[colon + 1..].trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = val.parse().expect("parse Content-Length");
        }
        if name.eq_ignore_ascii_case("set-cookie") {
            if let Some(first_seg) = val.split(';').next() {
                if let Some(eq) = first_seg.find('=') {
                    cookies.insert(
                        first_seg[..eq].trim().to_string(),
                        first_seg[eq + 1..].trim().to_string(),
                    );
                }
            }
        }
        headers.push((name, val));
    }

    let mut body_bytes = vec![0u8; if head_only { 0 } else { content_length }];
    if !body_bytes.is_empty() {
        reader
            .read_exact(&mut body_bytes)
            .expect("read HTTP response body");
    }

    RawHttpResp {
        status,
        headers,
        cookies,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
    }
}

fn http_get(addr: SocketAddr, target: &str, cookie: Option<&str>) -> RawHttpResp {
    let mut stream = TcpStream::connect(addr).expect("connect to loopback server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut hdrs = vec![("Connection", "close")];
    if let Some(c) = cookie {
        hdrs.push(("Cookie", c));
    }
    send_raw_http_on_stream(&mut stream, "GET", target, &hdrs, b"")
}

fn http_post(
    addr: SocketAddr,
    target: &str,
    content_type: &str,
    body: &[u8],
    cookie: Option<&str>,
) -> RawHttpResp {
    let mut stream = TcpStream::connect(addr).expect("connect to loopback server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut hdrs = vec![("Connection", "close"), ("Content-Type", content_type)];
    if let Some(c) = cookie {
        hdrs.push(("Cookie", c));
    }
    send_raw_http_on_stream(&mut stream, "POST", target, &hdrs, body)
}

// ===========================================================================
// 1. Pure-Rust RFC 1321 MD5, HMAC-MD5 & Real Capture Signature Tests (1..8)
// ===========================================================================

#[test]
fn test_01_rfc1321_md5_official_vectors() {
    assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex("a"), "0cc175b9c0f1b6a831c399e269772661");
    assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        md5_hex("message digest"),
        "f96b697d7cb7938d525a2f31aaf161d0"
    );
    assert_eq!(
        md5_hex("abcdefghijklmnopqrstuvwxyz"),
        "c3fcd3d76192e4007dfb496cca67e13b"
    );
    assert_eq!(
        md5_hex("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
        "d174ab98d277d9f5a5611c2c9f419d9f"
    );
    assert_eq!(
        md5_hex("12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
        "57edf4a22be3c955ac49da2e2107b67a"
    );
}

#[test]
fn test_02_md5_multi_chunk_streaming_boundary() {
    let payload: Vec<u8> = (0..512).map(|i| (i * 37 % 251) as u8).collect();
    let oneshot = md5_hex(&payload);

    let mut streaming = gserver_http::md5::Md5::new();
    for chunk in payload.chunks(7) {
        streaming.update(chunk);
    }
    assert_eq!(streaming.finalize_hex(), oneshot);
}

#[test]
fn test_03_rfc2104_hmac_md5_vectors() {
    // RFC 2104 Test Case 1: key = 16 bytes of 0x0b, data = "Hi There"
    let key1 = [0x0bu8; 16];
    assert_eq!(
        hmac_md5_hex(&key1, b"Hi There"),
        "9294727a3638bb1c13f48ef8158bfc9d"
    );

    // RFC 2104 Test Case 2: key = "Jefe", data = "what do ya want for nothing?"
    assert_eq!(
        hmac_md5_hex(b"Jefe", b"what do ya want for nothing?"),
        "750c783e6ab0b503eaa86e310a5db738"
    );

    // RFC 2104 Test Case 3: key = 16 bytes of 0xaa, data = 50 bytes of 0xdd
    let key3 = [0xaau8; 16];
    let data3 = [0xddu8; 50];
    assert_eq!(
        hmac_md5_hex(&key3, &data3),
        "56be34521d144c88dbb8c733f0e8b3f6"
    );

    // Key > 64 bytes (RFC 2202 Test Case 6: 80 bytes of 0xaa)
    let key6 = [0xaau8; 80];
    assert_eq!(
        hmac_md5_hex(&key6, b"Test Using Larger Than Block-Size Key - Hash Key First"),
        "6b1ab7fe4bd7bf8f0b62e6ce61b9d0cd"
    );
}

#[test]
fn test_04_live_capture_all_six_mapi_signatures_exact_match() {
    // Exact captured URLs from `live_capture_filtered.log` lines 27, 36, 45, 56, 64, 75:
    let cases = [
        (
            "http://127.0.0.1:8080/phone/applereview/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&version=1.0.34&time=1795500444&sign=ae1afadf7779b11b79ca86e9d7fc72c7",
            "ae1afadf7779b11b79ca86e9d7fc72c7",
        ),
        (
            "http://127.0.0.1:8080/phone/serverlist/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&packageVersion=1.0.34&scriptVersion=1.0.1194&time=1795500444&sign=364dfa193b076f7ab50b8c4ce17bec20",
            "364dfa193b076f7ab50b8c4ce17bec20",
        ),
        (
            "http://127.0.0.1:8080/phone/supernotice/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&action=get&browser_width=221.5&reserve01=1&reserve02=1&returntype=tpl&serverKey=2901000&time=1795500446&sign=c9ee3c6c503be557a3b1c67757f447b1",
            "c9ee3c6c503be557a3b1c67757f447b1",
        ),
        (
            "http://127.0.0.1:8080/phone/login?userid=1243831974477471744&sessionid=SID_56921a50f34cf49824123bfd56a62d70bb1e51795440318446&pl=android&os=android&gn=vnsg2&bind=21a23244ed44810da2d40c6d4455e686&uuid=21a23244ed44810da2d40c6d4455e686&country=VN&time=1795500447&sign=ff287c962a529dc40aa76175968273fd",
            "ff287c962a529dc40aa76175968273fd",
        ),
        (
            "http://127.0.0.1:8080/phone/loginrole/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&pid=636df6782106606d&time=1795500448&sign=b2b3ac6177ec9380f0713f304bb49429",
            "b2b3ac6177ec9380f0713f304bb49429",
        ),
        (
            "http://127.0.0.1:8080/phone/gethash/?pid=636df6782106606d&uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&group_id=2900161&time=1795500451&sign=57d777266bf1346b90118e5127550da4",
            "57d777266bf1346b90118e5127550da4",
        ),
    ];

    let keys = vec![
        VNSG2_MAPI_APP_KEY.to_string(),
        DEFAULT_FALLBACK_SIGN_KEY.to_string(),
    ];

    for (url, expected_sign) in cases {
        let computed = compute_mapi_signature(url, VNSG2_MAPI_APP_KEY);
        assert_eq!(computed, expected_sign, "Mismatch on URL: {}", url);

        let (_, raw_query) = split_request_target(url);
        assert!(verify_mapi_query_signature(&raw_query, &keys));
    }
}

#[test]
fn test_05_sign_mapi_url_and_sort_url_params() {
    let raw = "http://127.0.0.1:8080/phone/loginrole/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&pid=636df6782106606d";
    let signed = sign_mapi_url(raw, 1795500448, VNSG2_MAPI_APP_KEY);
    assert!(signed.ends_with("&time=1795500448&sign=b2b3ac6177ec9380f0713f304bb49429"));
    assert_eq!(
        sort_url_params(&signed, true),
        "gn=vnsg2os=androidpid=636df6782106606dpl=androidtime=1795500448uuid=21a23244ed44810da2d40c6d4455e686"
    );
}

#[test]
fn test_06_account_and_guest_signatures() {
    let pass_md5 = md5_hex("secret123");
    let sig = compute_account_auth_sig(
        DEFAULT_AUTH_SECRET,
        DEFAULT_GTSDK_GAME_ID,
        "player01",
        &pass_md5,
        "1795500440",
    );
    let expected = md5_hex(format!(
        "{}{}player01{}1795500440",
        DEFAULT_AUTH_SECRET, DEFAULT_GTSDK_GAME_ID, pass_md5
    ));
    assert_eq!(sig, expected);

    let gsig = compute_guest_auth_sig(
        DEFAULT_AUTH_SECRET,
        DEFAULT_GTSDK_GAME_ID,
        "21a23244ed44810da2d40c6d4455e686",
        "1795500440",
    );
    assert_eq!(gsig.len(), 32);
}

// ===========================================================================
// 2. URL Percent Codec, Query Parser & Cookie Header Tests (7..10)
// ===========================================================================

#[test]
fn test_07_url_percent_encode_decode_vietnamese_utf8() {
    let original = "S161.Võ Xương & Tương Dương!";
    let encoded = percent_encode(original);
    assert!(!encoded.contains(' '));
    assert!(!encoded.contains('&'));
    assert_eq!(percent_decode(&encoded), original);
    assert_eq!(form_decode("S161.V%C3%B5+X%C6%B0%C6%A1ng"), "S161.Võ Xương");
    // Malformed % sequences stay intact safely
    assert_eq!(percent_decode("100%25_raw%ZZ%2"), "100%_raw%ZZ%2");
}

#[test]
fn test_08_request_target_and_path_normalization() {
    let (p1, q1) = split_request_target("/phone/serverlist/?uuid=abc&pl=android#frag");
    assert_eq!(p1, "/phone/serverlist");
    assert_eq!(q1, "uuid=abc&pl=android");

    let (p2, q2) = split_request_target("http://127.0.0.1:8080/phone/gethash///?pid=1");
    assert_eq!(p2, "/phone/gethash");
    assert_eq!(q2, "pid=1");

    assert_eq!(normalize_path("/"), "/");
    assert_eq!(normalize_path("//phone//login///"), "/phone/login");

    let parsed = parse_query_params(&q1);
    assert_eq!(parsed.get("uuid").map(String::as_str), Some("abc"));
    assert_eq!(parsed.get("pl").map(String::as_str), Some("android"));
}

#[test]
fn test_09_cookie_header_parser() {
    let cookies = parse_cookie_header(
        "PHPSESSID=a8l2b6rupljq89ebag0lndj1s3; GSESS_ID=\"99887766aabb\"; theme=dark",
    );
    assert_eq!(
        cookies.get("PHPSESSID").map(String::as_str),
        Some("a8l2b6rupljq89ebag0lndj1s3")
    );
    assert_eq!(
        cookies.get("GSESS_ID").map(String::as_str),
        Some("99887766aabb")
    );
    assert_eq!(cookies.get("theme").map(String::as_str), Some("dark"));
}

// ===========================================================================
// 3. Pure-Rust RFC 8259 JSON Parser & Serializer Tests (10..14)
// ===========================================================================

#[test]
fn test_10_json_parse_unicode_escapes_and_surrogate_pairs() {
    // Live capture escaped Vietnamese string `S161.V\u00f5 X\u01b0\u01a1ng` + UTF-16 surrogate pair `\uD83D\uDE80` (rocket)
    let raw = r#"{"groupname":"S161.V\u00f5 X\u01b0\u01a1ng","emoji":"\uD83D\uDE80","escaped":"line1\n\"quoted\"\\slash\/"}"#;
    let parsed = parse_json(raw).expect("valid JSON");
    assert_eq!(
        parsed.get("groupname").and_then(JsonValue::as_str),
        Some("S161.Võ Xương")
    );
    assert_eq!(parsed.get("emoji").and_then(JsonValue::as_str), Some("🚀"));
    assert_eq!(
        parsed.get("escaped").and_then(JsonValue::as_str),
        Some("line1\n\"quoted\"\\slash/")
    );

    // Round-trip serialize and re-parse
    let serialized = parsed.to_json_string();
    let reparsed = parse_json(&serialized).expect("reparse serialized JSON");
    assert_eq!(parsed, reparsed);
}

#[test]
fn test_11_json_numbers_arrays_and_nested_objects() {
    let raw = r#"
    {
        "AMF": 3,
        "offset": -420,
        "big": 1243831974477471744,
        "ratio": 221.5,
        "sci": 1.25e-3,
        "flags": [true, false, null, {"nested": [1, 2, 3]}]
    }
    "#;
    let val = parse_json(raw).expect("parse complex JSON");
    assert_eq!(val.get("AMF").and_then(JsonValue::as_i64), Some(3));
    assert_eq!(val.get("offset").and_then(JsonValue::as_i64), Some(-420));
    assert_eq!(
        val.get("big").and_then(JsonValue::as_i64),
        Some(1243831974477471744)
    );
    assert_eq!(val.get("ratio").and_then(JsonValue::as_f64), Some(221.5));
    let flags = val.get("flags").and_then(JsonValue::as_array).unwrap();
    assert_eq!(flags[0].as_bool(), Some(true));
    assert_eq!(flags[1].as_bool(), Some(false));
    assert!(flags[2].is_null());
    assert_eq!(
        flags[3]
            .get("nested")
            .and_then(JsonValue::as_array)
            .unwrap()
            .len(),
        3
    );

    // Verify exact integer formatting (no `.0` on AMF: 3)
    let mut check_obj = JsonObject::new();
    check_obj.insert("AMF", 3);
    check_obj.insert("ratio", JsonValue::Number(JsonNumber::Float(221.5)));
    assert_eq!(
        JsonValue::Object(check_obj).to_json_string(),
        r#"{"AMF":3,"ratio":221.5}"#
    );
}

#[test]
fn test_12_json_rejects_malformed_inputs() {
    let invalid_cases = [
        "",
        "{",
        r#"{"a": 01}"#,          // RFC 8259 forbids leading zeros
        r#"{"a": 1,}"#,          // trailing comma
        r#"[1, 2,]"#,            // trailing comma in array
        r#"{"a": "unterminated"#,
        r#"{"a": "\uD83D"}"#,    // unpaired high surrogate
        r#"{"a": "\uDC00"}"#,    // unpaired low surrogate
        r#"{"a": 1} trailing"#,  // trailing garbage
    ];
    for bad in invalid_cases {
        assert!(
            parse_json(bad).is_err(),
            "Expected JSON parse failure for input: {:?}",
            bad
        );
    }
}

// ===========================================================================
// 4. Live Loopback Socket (`127.0.0.1:0`) MAPI & Auth Tests (13..32)
// ===========================================================================

#[test]
fn test_13_live_admin_status_health_endpoint() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");
    let resp = http_get(handle.local_addr(), "/admin/status", None);
    assert_eq!(resp.status, 200);
    let json = resp.json();
    assert_eq!(json.get("status").and_then(JsonValue::as_str), Some("ok"));
    assert_eq!(json.get("game").and_then(JsonValue::as_str), Some("vnsg2"));
    assert_eq!(
        json.get("sdk_version").and_then(JsonValue::as_str),
        Some("0.10.41")
    );
    assert!(json.get("servers_count").and_then(JsonValue::as_i64).unwrap() >= 2);
}

#[test]
fn test_14_live_applereview_endpoint() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");
    let path = sign_mapi_url(
        "/phone/applereview/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&version=1.0.34",
        1795500444,
        VNSG2_MAPI_APP_KEY,
    );
    let resp = http_get(handle.local_addr(), &path, None);
    assert_eq!(resp.status, 200);
    let json = resp.json();
    assert_eq!(json.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert_eq!(json.get("applereview").and_then(JsonValue::as_i64), Some(0));
    assert_eq!(json.get("ret").and_then(JsonValue::as_i64), Some(404));
}

#[test]
fn test_15_live_getversion_uptodate_and_script_and_base_policies() {
    let (svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");

    // 1. Default UpToDate policy: errornu="0", error_id=500, empty script=[]
    let resp = http_get(
        handle.local_addr(),
        "/phone/getversion/?packageVersion=1.0.34&scriptVersion=1.0.1194",
        None,
    );
    assert_eq!(resp.status, 200);
    let j = resp.json();
    assert_eq!(j.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert_eq!(j.get("error_id").and_then(JsonValue::as_i64), Some(500));
    assert_eq!(
        j.get("script").and_then(JsonValue::as_array).unwrap().len(),
        0
    );

    // Also verify `/version.lua`
    let vlua = http_get(handle.local_addr(), "/version.lua", None);
    assert_eq!(vlua.status, 200);
    assert!(vlua.body.contains("package = \"1.0.34\""));

    // 2. ScriptUpdate policy
    svc.set_version_policy(VersionPolicy::ScriptUpdate {
        static_url: "http://127.0.0.1:9000/cdn".to_string(),
        spare_static_url: "http://127.0.0.1:9001/cdn".to_string(),
        patches: vec![ScriptPatchEntry {
            tar_version: "1.0.1195".to_string(),
            total_size: 4096,
            path: "/patches/1195".to_string(),
            files: vec!["d41d8cd98f00b204e9800998ecf8427e".to_string()],
            svn: 452500,
            csv_svn: 449200,
            update_time: 1795500500,
        }],
    });
    let resp2 = http_get(handle.local_addr(), "/phone/getversion", None);
    let j2 = resp2.json();
    assert_eq!(j2.get("error_id").and_then(JsonValue::as_i64), Some(200));
    assert_eq!(
        j2.get("static_url").and_then(JsonValue::as_str),
        Some("http://127.0.0.1:9000/cdn")
    );
    let patches = j2.get("script").and_then(JsonValue::as_array).unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].get("tar_version").and_then(JsonValue::as_str),
        Some("1.0.1195")
    );

    // 3. BaseUpdate policy
    svc.set_version_policy(VersionPolicy::BaseUpdate {
        package_url: "https://example.com/gclient_1.0.38.apk".to_string(),
        update_desc: "Update base package".to_string(),
    });
    let resp3 = http_get(handle.local_addr(), "/phone/getversion/", None);
    let j3 = resp3.json();
    let base_pkg = j3
        .get("base")
        .and_then(JsonValue::as_object)
        .unwrap()
        .get_object("package")
        .unwrap();
    assert_eq!(
        base_pkg.get_str("packageUrl"),
        Some("https://example.com/gclient_1.0.38.apk")
    );
}

#[test]
fn test_16_live_serverlist_schema_and_trailing_slash_equivalence() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");

    let r1 = http_get(
        handle.local_addr(),
        "/phone/serverlist?packageVersion=1.0.34&scriptVersion=1.0.1194",
        None,
    );
    let r2 = http_get(
        handle.local_addr(),
        "/phone/serverlist/?packageVersion=1.0.34&scriptVersion=1.0.1194",
        None,
    );
    assert_eq!(r1.status, 200);
    assert_eq!(r1.body, r2.body);

    let j = r1.json();
    assert_eq!(j.get("errornu").and_then(JsonValue::as_str), Some("0"));
    let root = j.get("root").and_then(JsonValue::as_object).unwrap();
    let items = root.get_array("item").unwrap();
    assert!(items.len() >= 2);
    let s161 = items[0].as_object().unwrap();
    assert_eq!(s161.get_str("groupid"), Some("2900161"));
    assert_eq!(s161.get_str("group"), Some("game2900161"));
    assert_eq!(s161.get_str("name"), Some("S161.Võ Xương"));
    assert_eq!(s161.get_str("host"), Some("127.0.0.1"));
    assert_eq!(s161.get_i64("port"), Some(8160));
    assert_eq!(s161.get_i64("status"), Some(1));
    assert_eq!(s161.get_i64("hot"), Some(1));
    assert_eq!(s161.get_i64("new"), Some(0));

    let notice = root.get_object("notice").unwrap();
    assert_eq!(notice.get_i64("open"), Some(0));
}

#[test]
fn test_17_live_notice_and_supernotice_endpoints() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");

    let sn = http_get(
        handle.local_addr(),
        "/phone/supernotice/?action=get&returntype=tpl&browser_width=221.5&reserve01=1&reserve02=1&serverKey=2900161",
        None,
    );
    assert_eq!(sn.status, 200);
    let sn_json = sn.json();
    assert_eq!(sn_json.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert!(sn_json
        .get("data")
        .and_then(JsonValue::as_str)
        .unwrap()
        .starts_with("<!DOCTYPE html>"));

    let n = http_get(
        handle.local_addr(),
        "/phone/notice/?action=get&returntype=sgstr&serverKey=2900161",
        None,
    );
    assert_eq!(n.status, 200);
    let n_json = n.json();
    assert_eq!(n_json.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert!(!n_json
        .get("data")
        .and_then(JsonValue::as_str)
        .unwrap()
        .is_empty());

    // Direct HTML format mode for WebView
    let html_resp = http_get(handle.local_addr(), "/phone/supernotice?format=html", None);
    assert_eq!(html_resp.status, 200);
    assert!(html_resp.body.starts_with("<!DOCTYPE html>"));
}

#[test]
fn test_18_live_full_captured_login_loginrole_gethash_flow() {
    let (svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");
    svc.set_mock_time(Some(1795500450));

    // Step 1: Exchange Platform userid + sessionid on `/phone/login` (exact params from `live_capture_filtered.log:56`)
    let login_url = sign_mapi_url(
        "/phone/login?userid=1243831974477471744&sessionid=SID_56921a50f34cf49824123bfd56a62d70bb1e51795440318446&pl=android&os=android&gn=vnsg2&bind=21a23244ed44810da2d40c6d4455e686&uuid=21a23244ed44810da2d40c6d4455e686&country=VN",
        1795500447,
        VNSG2_MAPI_APP_KEY,
    );
    let login_resp = http_get(handle.local_addr(), &login_url, None);
    assert_eq!(login_resp.status, 200);

    // Verify Set-Cookie headers (`PHPSESSID` & `GSESS_ID`)
    let php_sess = login_resp
        .cookies
        .get(COOKIE_PHPSESSID)
        .expect("Set-Cookie PHPSESSID must be returned")
        .clone();
    let game_sess = login_resp
        .cookies
        .get(COOKIE_GAME_SESSION)
        .expect("Set-Cookie GSESS_ID must be returned")
        .clone();
    assert_eq!(php_sess, game_sess);

    let login_json = login_resp.json();
    assert_eq!(
        login_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    assert_eq!(
        login_json.get("uid").and_then(JsonValue::as_str),
        Some("636df6782106606d")
    );
    assert_eq!(
        login_json.get("errordesc").and_then(JsonValue::as_str),
        Some("OK(1-0)")
    );
    assert_eq!(login_json.get("newuser").and_then(JsonValue::as_i64), Some(0));
    assert_eq!(
        login_json.get("env").and_then(JsonValue::as_str),
        Some("online")
    );

    // Step 2: `/phone/loginrole/`
    let role_url = sign_mapi_url(
        "/phone/loginrole/?uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&pid=636df6782106606d",
        1795500448,
        VNSG2_MAPI_APP_KEY,
    );
    let cookie_hdr = format!("PHPSESSID={}", php_sess);
    let role_resp = http_get(handle.local_addr(), &role_url, Some(&cookie_hdr));
    let role_json = role_resp.json();
    assert_eq!(
        role_json.get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
    let roles = role_json
        .get("root")
        .and_then(JsonValue::as_object)
        .unwrap()
        .get_array("role")
        .unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(
        roles[0].get("uname").and_then(JsonValue::as_str),
        Some("Kaiaoaoa.s161.s161")
    );
    assert_eq!(roles[0].get("level").and_then(JsonValue::as_i64), Some(121));
    assert_eq!(roles[0].get("vip").and_then(JsonValue::as_i64), Some(3));

    // Step 3: `/phone/gethash/` with `Cookie: PHPSESSID=...`
    let hash_url = sign_mapi_url(
        "/phone/gethash/?pid=636df6782106606d&uuid=21a23244ed44810da2d40c6d4455e686&pl=android&os=android&gn=vnsg2&group_id=2900161",
        1795500451,
        VNSG2_MAPI_APP_KEY,
    );
    let hash_resp = http_get(handle.local_addr(), &hash_url, Some(&cookie_hdr));
    assert_eq!(hash_resp.status, 200);
    let hj = hash_resp.json();
    assert_eq!(hj.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert_eq!(hj.get("AMF").and_then(JsonValue::as_i64), Some(3));
    assert_eq!(hj.get("b_fightLog").and_then(JsonValue::as_i64), Some(1));
    assert_eq!(hj.get("canCreateUser").and_then(JsonValue::as_i64), Some(1));
    assert_eq!(hj.get("canPay").and_then(JsonValue::as_i64), Some(1));
    assert_eq!(
        hj.get("feignRoleId").and_then(JsonValue::as_str),
        Some("487932b")
    );
    assert_eq!(
        hj.get("serverID").and_then(JsonValue::as_str),
        Some("game2900161")
    );
    assert_eq!(hj.get("host").and_then(JsonValue::as_str), Some("127.0.0.1"));
    assert_eq!(hj.get("port").and_then(JsonValue::as_i64), Some(8160));
    assert_eq!(hj.get("offset").and_then(JsonValue::as_str), Some("-420"));
    assert_eq!(
        hj.get("openDateTime").and_then(JsonValue::as_str),
        Some("2018-11-06-09-00-00")
    );
    assert_eq!(
        hj.get("mergeServerTime").and_then(JsonValue::as_i64),
        Some(1667530800)
    );
    assert_eq!(hj.get("plosgn").and_then(JsonValue::as_i64), Some(2));
    assert_eq!(
        hj.get("qid").and_then(JsonValue::as_str),
        Some("1243831974477471744")
    );
    assert_eq!(hj.get("timestamp").and_then(JsonValue::as_i64), Some(1795500450));
    let game_hash = hj.get("hash").and_then(JsonValue::as_str).unwrap();
    assert_eq!(game_hash.len(), 32);

    // Step 4: Verify issued game hash via `/admin/verify_hash`
    let verify = http_get(
        handle.local_addr(),
        &format!("/admin/verify_hash?pid=636df6782106606d&hash={}", game_hash),
        None,
    );
    assert_eq!(
        verify.json().get("valid").and_then(JsonValue::as_bool),
        Some(true)
    );
}

#[test]
fn test_19_strict_session_missing_cookie_returns_err_code_102() {
    let mut cfg = ServerConfig::default();
    cfg.strict_session = true;
    let (_svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    let resp = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=636df6782106606d&uuid=21a23244ed44810da2d40c6d4455e686&group_id=2900161",
        None,
    );
    assert_eq!(resp.status, 200);
    let j = resp.json();
    // Both `errornu = "102"` (SDKUtil.lua ERROR_PASS["102"]) and `err_code = 102` (LoginProxy.lua:645)
    assert_eq!(j.get("errornu").and_then(JsonValue::as_str), Some("102"));
    assert_eq!(j.get("err_code").and_then(JsonValue::as_i64), Some(102));
    assert_eq!(
        j.get("errordesc").and_then(JsonValue::as_str),
        Some("session time out")
    );
}

#[test]
fn test_20_strict_session_pid_mismatch_and_expiry_return_err_code_102() {
    let mut cfg = ServerConfig::default();
    cfg.strict_session = true;
    cfg.session_ttl_secs = 60;
    let (svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");
    svc.set_mock_time(Some(1_800_000_000));

    let login_resp = http_get(
        handle.local_addr(),
        "/phone/login?userid=1243831974477471744&sessionid=SID_tok1&uuid=dev1",
        None,
    );
    let sess = login_resp.cookies.get(COOKIE_GAME_SESSION).unwrap().clone();
    let cookie_hdr = format!("GSESS_ID={}", sess);

    // 1. Mismatched `pid` using someone else's valid cookie -> err_code 102!
    let mismatch = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=deadbeef00001111&uuid=dev1&group_id=2900161",
        Some(&cookie_hdr),
    );
    assert_eq!(mismatch.json().get("err_code").and_then(JsonValue::as_i64), Some(102));

    // 2. Matching `GSESS_ID` cookie before TTL -> succeeds (`errornu = "0"`)
    let ok_before_ttl = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=636df6782106606d&uuid=dev1&group_id=2900161",
        Some(&cookie_hdr),
    );
    assert_eq!(
        ok_before_ttl.json().get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );

    // 3. Advance clock past TTL (60s) -> err_code 102 (`session time out`)!
    svc.set_mock_time(Some(1_800_000_120));
    let expired = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=636df6782106606d&uuid=dev1&group_id=2900161",
        Some(&cookie_hdr),
    );
    assert_eq!(expired.json().get("err_code").and_then(JsonValue::as_i64), Some(102));
}

#[test]
fn test_21_permissive_session_allows_cookie_free_gethash() {
    let mut cfg = ServerConfig::default();
    cfg.strict_session = false;
    let (_svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    let resp = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=636df6782106606d&uuid=dev1&group_id=2900161",
        None,
    );
    let j = resp.json();
    assert_eq!(j.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert_eq!(j.get("AMF").and_then(JsonValue::as_i64), Some(3));
}

#[test]
fn test_22_gethash_maintenance_104_and_unknown_group_101_and_missing_pid_103() {
    let mut cfg = ServerConfig::default();
    cfg.strict_session = false;
    let (svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    // Missing pid -> 103
    let r103 = http_get(handle.local_addr(), "/phone/gethash/?group_id=2900161", None);
    assert_eq!(r103.json().get("err_code").and_then(JsonValue::as_i64), Some(103));

    // Unknown group_id -> 101
    let r101 = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=636df6782106606d&group_id=9999999",
        None,
    );
    assert_eq!(r101.json().get("err_code").and_then(JsonValue::as_i64), Some(101));

    // Maintenance server (`status = 2`) -> `errornu = "104"` AND `err_code = 104`
    let mut maint = GameServerEntry::default_s161("127.0.0.1", 8160);
    maint.groupid = "2900999".to_string();
    maint.group = "game2900999".to_string();
    maint.status = ServerStatus::Maintenance as u8;
    maint.maintenance_notice = "Bảo trì nâng cấp S999".to_string();
    svc.upsert_server(maint);

    let r104 = http_get(
        handle.local_addr(),
        "/phone/gethash/?pid=636df6782106606d&group_id=2900999",
        None,
    );
    let j104 = r104.json();
    assert_eq!(j104.get("errornu").and_then(JsonValue::as_str), Some("104"));
    assert_eq!(j104.get("err_code").and_then(JsonValue::as_i64), Some(104));
    assert_eq!(
        j104.get("notice").and_then(JsonValue::as_str),
        Some("Bảo trì nâng cấp S999")
    );
}

#[test]
fn test_23_strict_sign_mode_enforces_valid_md5_signature() {
    let mut cfg = ServerConfig::default();
    cfg.strict_sign = true;
    let (_svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    // 1. Unsigned request rejected
    let unsigned = http_get(
        handle.local_addr(),
        "/phone/serverlist/?packageVersion=1.0.34&scriptVersion=1.0.1194",
        None,
    );
    assert_eq!(
        unsigned.json().get("errornu").and_then(JsonValue::as_str),
        Some("1")
    );

    // 2. Tampered parameter after signing rejected
    let signed = sign_mapi_url(
        "/phone/serverlist/?uuid=dev1&pl=android&os=android&gn=vnsg2&packageVersion=1.0.34&scriptVersion=1.0.1194",
        1795500444,
        VNSG2_MAPI_APP_KEY,
    );
    let tampered = signed.replacen("packageVersion=1.0.34", "packageVersion=9.9.99", 1);
    let bad_resp = http_get(handle.local_addr(), &tampered, None);
    assert_eq!(
        bad_resp.json().get("errornu").and_then(JsonValue::as_str),
        Some("1")
    );

    // 3. Valid signature accepted
    let good_resp = http_get(handle.local_addr(), &signed, None);
    assert_eq!(
        good_resp.json().get("errornu").and_then(JsonValue::as_str),
        Some("0")
    );
}

#[test]
fn test_24_account_auth_and_password_verification() {
    let mut cfg = ServerConfig::default();
    cfg.strict_auth_sig = true;
    let (_svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    let u = "acct_warrior";
    let pw_md5 = md5_hex("MyStrongPass!");
    let ts = "1795500600";
    let sig = compute_account_auth_sig(DEFAULT_AUTH_SECRET, DEFAULT_GTSDK_GAME_ID, u, &pw_md5, ts);

    let url = format!(
        "/api/login/auth/account?u={}&p={}&timestamp={}&sig={}&gameID=GCLIENT&game_version=1.0.38&lang=vi&country=VN&ver=v2.0",
        u, pw_md5, ts, sig
    );
    // First login auto-registers (`returnCode = "2"`)
    let r1 = http_post(handle.local_addr(), &url, "application/x-www-form-urlencoded", b"", None);
    let j1 = r1.json();
    assert_eq!(j1.get("returnCode").and_then(JsonValue::as_str), Some("2"));
    let uid1 = j1.get("userID").and_then(JsonValue::as_str).unwrap().to_string();
    let sid1 = j1.get("sessionID").and_then(JsonValue::as_str).unwrap().to_string();
    assert!(sid1.starts_with("SID_"));

    // Second login with same password returns existing account (`returnCode = "1"`) and same `userID`
    let r2 = http_post(handle.local_addr(), &url, "application/x-www-form-urlencoded", b"", None);
    let j2 = r2.json();
    assert_eq!(j2.get("returnCode").and_then(JsonValue::as_str), Some("1"));
    assert_eq!(j2.get("userID").and_then(JsonValue::as_str), Some(uid1.as_str()));

    // Wrong password rejected (`returnCode = "-1"`)
    let bad_pw = md5_hex("WrongPass");
    let bad_sig = compute_account_auth_sig(DEFAULT_AUTH_SECRET, DEFAULT_GTSDK_GAME_ID, u, &bad_pw, ts);
    let bad_url = format!(
        "/api/login/auth/account?u={}&p={}&timestamp={}&sig={}&gameID=GCLIENT",
        u, bad_pw, ts, bad_sig
    );
    let r3 = http_post(handle.local_addr(), &bad_url, "application/x-www-form-urlencoded", b"", None);
    assert_eq!(
        r3.json().get("returnCode").and_then(JsonValue::as_str),
        Some("-1")
    );
}

#[test]
fn test_25_guest_auth_persistent_device_recovery() {
    let mut cfg = ServerConfig::default();
    cfg.strict_auth_sig = true;
    let (_svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    let dev = "win32_device_99887766554433221100";
    let ts = "1795500700";
    let sig = compute_guest_auth_sig(DEFAULT_AUTH_SECRET, DEFAULT_GTSDK_GAME_ID, dev, ts);
    let url = format!(
        "/api/login/createUser/guest?deviceID={}&timestamp={}&sig={}&gameID=GCLIENT&ver=v2.0",
        dev, ts, sig
    );

    let r1 = http_post(handle.local_addr(), &url, "application/x-www-form-urlencoded", b"", None);
    let j1 = r1.json();
    assert_eq!(j1.get("returnCode").and_then(JsonValue::as_str), Some("2"));
    let uid1 = j1.get("userID").and_then(JsonValue::as_str).unwrap().to_string();
    let pid1 = j1.get("pid").and_then(JsonValue::as_str).unwrap().to_string();

    // Repeat guest login from same `deviceID` recovers the exact same `userID` and `pid`!
    let r2 = http_post(handle.local_addr(), &url, "application/x-www-form-urlencoded", b"", None);
    let j2 = r2.json();
    assert_eq!(j2.get("returnCode").and_then(JsonValue::as_str), Some("1"));
    assert_eq!(j2.get("userID").and_then(JsonValue::as_str), Some(uid1.as_str()));
    assert_eq!(j2.get("pid").and_then(JsonValue::as_str), Some(pid1.as_str()));
}

#[test]
fn test_26_custom_json_registration_and_duplicate_rejection() {
    let mut cfg = ServerConfig::default();
    cfg.auto_register_on_account_login = false;
    let (_svc, handle) = start_mapi_server("127.0.0.1:0", cfg).expect("bind server");

    // Before registration, login fails when `auto_register_on_account_login = false`
    let login_before = http_post(
        handle.local_addr(),
        "/api/login/login",
        "application/json",
        br#"{"username":"custom_hero","password":"pw_alpha"}"#,
        None,
    );
    assert_eq!(
        login_before.json().get("errornu").and_then(JsonValue::as_str),
        Some("1")
    );

    // Register via JSON POST body
    let reg = http_post(
        handle.local_addr(),
        "/api/login/register",
        "application/json",
        br#"{"username":"custom_hero","password":"pw_alpha","uuid":"win32_uuid_abc"}"#,
        None,
    );
    let rj = reg.json();
    assert_eq!(rj.get("errornu").and_then(JsonValue::as_str), Some("0"));
    assert_eq!(rj.get("returnCode").and_then(JsonValue::as_str), Some("2"));
    let pid = rj.get("pid").and_then(JsonValue::as_str).unwrap().to_string();
    let cookie = reg.cookies.get(COOKIE_PHPSESSID).unwrap().clone();

    // Duplicate registration rejected (`returnCode = "-2"`)
    let dup = http_post(
        handle.local_addr(),
        "/api/login/register",
        "application/json",
        br#"{"username":"custom_hero","password":"pw_beta"}"#,
        None,
    );
    assert_eq!(
        dup.json().get("returnCode").and_then(JsonValue::as_str),
        Some("-2")
    );

    // Direct `/phone/gethash/` works immediately using the registration session cookie!
    let gh = http_get(
        handle.local_addr(),
        &format!("/phone/gethash/?pid={}&uuid=win32_uuid_abc&group_id=2900161", pid),
        Some(&format!("PHPSESSID={}", cookie)),
    );
    assert_eq!(gh.json().get("errornu").and_then(JsonValue::as_str), Some("0"));
}

#[test]
fn test_27_http11_keep_alive_persistent_connection_multiple_requests() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");
    let mut stream = TcpStream::connect(handle.local_addr()).expect("connect keep-alive socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");

    // 1st request on TCP socket (Keep-Alive) -> login and obtain PHPSESSID
    let r1 = send_raw_http_on_stream(
        &mut stream,
        "GET",
        "/phone/login?userid=1243831974477471744&sessionid=SID_ka&uuid=ka_uuid",
        &[("Connection", "keep-alive")],
        b"",
    );
    assert_eq!(r1.status, 200);
    let sess = r1.cookies.get(COOKIE_PHPSESSID).unwrap().clone();
    let cookie_val = format!("PHPSESSID={}", sess);

    // 2nd request on the EXACT SAME TCP socket -> serverlist
    let r2 = send_raw_http_on_stream(
        &mut stream,
        "GET",
        "/phone/serverlist/?packageVersion=1.0.34&scriptVersion=1.0.1194",
        &[("Connection", "keep-alive"), ("Cookie", &cookie_val)],
        b"",
    );
    assert_eq!(r2.status, 200);
    assert_eq!(r2.json().get("errornu").and_then(JsonValue::as_str), Some("0"));

    // 3rd request on the EXACT SAME TCP socket -> gethash with `Connection: close`
    let r3 = send_raw_http_on_stream(
        &mut stream,
        "GET",
        "/phone/gethash/?pid=636df6782106606d&uuid=ka_uuid&group_id=2900161",
        &[("Connection", "close"), ("Cookie", &cookie_val)],
        b"",
    );
    assert_eq!(r3.status, 200);
    assert_eq!(r3.json().get("AMF").and_then(JsonValue::as_i64), Some(3));
}

#[test]
fn test_28_chunked_transfer_encoding_post_body() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");
    let mut stream = TcpStream::connect(handle.local_addr()).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");

    let chunk1 = br#"{"username":"chunked_"#;
    let chunk2 = br#"user","password":"secret"}"#;
    let raw_req = format!(
        "POST /api/login/register HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n",
        chunk1.len(),
        std::str::from_utf8(chunk1).unwrap(),
        chunk2.len(),
        std::str::from_utf8(chunk2).unwrap(),
    );
    stream.write_all(raw_req.as_bytes()).unwrap();
    stream.flush().unwrap();

    let resp = read_http_response(&mut stream, false);
    assert_eq!(resp.status, 200);
    let j = resp.json();
    assert_eq!(
        j.get("accountName").and_then(JsonValue::as_str),
        Some("chunked_user")
    );
}

#[test]
fn test_29_auxiliary_sdk_endpoints_and_404_handling() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");

    for ep in [
        "/phone/userinfo/",
        "/phone/platform/getPlatformUserInfo/?xpid=636df6782106606d",
        "/phone/helper/getagreeversion/?appid=1003",
        "/phone/addiction/getaddiction/?pid=636df6782106606d&limit_time=0",
        "/phone/addiction/clear/?pid=636df6782106606d",
        "/phone/innerbrowse/?serverKey=2900161",
        "/phone/question/?action=answer",
        "/phone/getforcefix/",
    ] {
        let r = http_get(handle.local_addr(), ep, None);
        assert_eq!(r.status, 200, "Failed on endpoint {}", ep);
        assert_eq!(
            r.json().get("errornu").and_then(JsonValue::as_str),
            Some("0"),
            "Non-zero errornu on {}",
            ep
        );
    }

    // Unknown endpoint returns HTTP 404 with JSON `errornu = "1"`
    let r404 = http_get(handle.local_addr(), "/phone/nonexistent_route", None);
    assert_eq!(r404.status, 404);
    assert_eq!(r404.json().get("errornu").and_then(JsonValue::as_str), Some("1"));
}

#[test]
fn test_30_concurrent_multi_client_socket_stress() {
    let (svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");
    let addr = handle.local_addr();

    // Register custom role for a test player
    svc.set_roles_for_pid(
        "636df6782106606d",
        vec![RoleSummary {
            uid: 88888,
            uname: "TriệuVân.s161".to_string(),
            level: 150,
            quality: 7,
            groupid: "2900161".to_string(),
            groupname: "S161.Võ Xương".to_string(),
            maintainflag: 0,
            vip: 10,
        }],
    );

    let barrier = Arc::new(std::sync::Barrier::new(8));
    let mut threads = Vec::new();
    for i in 0..8 {
        let b = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            b.wait();
            let dev = format!("concurrent_device_{:04}", i);
            let guest_resp = http_post(
                addr,
                &format!("/api/login/createUser/guest?deviceID={}&timestamp=1795500800", dev),
                "application/x-www-form-urlencoded",
                b"",
                None,
            );
            let gj = guest_resp.json();
            let uid = gj.get("userID").and_then(JsonValue::as_str).unwrap().to_string();
            let sid = gj.get("sessionID").and_then(JsonValue::as_str).unwrap().to_string();

            let login_resp = http_get(
                addr,
                &format!("/phone/login?userid={}&sessionid={}&uuid={}", uid, sid, dev),
                None,
            );
            let php_cookie = login_resp.cookies.get(COOKIE_PHPSESSID).unwrap().clone();
            let pid = login_resp
                .json()
                .get("uid")
                .and_then(JsonValue::as_str)
                .unwrap()
                .to_string();

            let hash_resp = http_get(
                addr,
                &format!("/phone/gethash/?pid={}&uuid={}&group_id=2900161", pid, dev),
                Some(&format!("PHPSESSID={}", php_cookie)),
            );
            let hj = hash_resp.json();
            assert_eq!(hj.get("errornu").and_then(JsonValue::as_str), Some("0"));
            assert_eq!(hj.get("pid").and_then(JsonValue::as_str), Some(pid.as_str()));
        }));
    }

    for t in threads {
        t.join().expect("client thread panicked");
    }
}

#[test]
fn test_31_http_head_and_options_headers_and_method_not_allowed() {
    let (_svc, handle) =
        start_mapi_server("127.0.0.1:0", ServerConfig::default()).expect("bind server");

    // OPTIONS preflight
    let mut s1 = TcpStream::connect(handle.local_addr()).unwrap();
    s1.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let opt = send_raw_http_on_stream(
        &mut s1,
        "OPTIONS",
        "/phone/serverlist/",
        &[("Connection", "close")],
        b"",
    );
    assert_eq!(opt.status, 204);
    assert!(opt
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("allow") && v.contains("GET")));

    // HEAD request returns Content-Length > 0 with empty body
    let mut s2 = TcpStream::connect(handle.local_addr()).unwrap();
    s2.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let head = send_raw_http_on_stream(
        &mut s2,
        "HEAD",
        "/admin/status",
        &[("Connection", "close")],
        b"",
    );
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    let cl: usize = head
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .parse()
        .unwrap();
    assert!(cl > 20);

    // Unsupported DELETE returns 405 Method Not Allowed
    let mut s3 = TcpStream::connect(handle.local_addr()).unwrap();
    s3.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let del = send_raw_http_on_stream(
        &mut s3,
        "DELETE",
        "/phone/serverlist/",
        &[("Connection", "close")],
        b"",
    );
    assert_eq!(del.status, 405);
}
