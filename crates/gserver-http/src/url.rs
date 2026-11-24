//! Pure-Rust RFC 3986 URL percent-encoder/decoder, `application/x-www-form-urlencoded`
//! query parser, HTTP request-target splitter, path normalizer, and Cookie header parser.

use std::collections::BTreeMap;

/// Percent-encodes raw UTF-8/bytes according to RFC 3986 unreserved characters
/// (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`), using uppercase hex digits (`%XX`)
/// identical to `bt::Platform::urlEncode` in `BtPlatformWin32.cpp:1459`.
pub fn percent_encode(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decodes `%XX` sequences into raw bytes. If `plus_as_space` is `true`,
/// `'+'` is decoded as `' '` (`0x20`) for `application/x-www-form-urlencoded`.
/// Invalid `%` sequences are preserved verbatim rather than panicking or dropping data.
pub fn percent_decode_bytes(input: &str, plus_as_space: bool) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Percent-decodes a URL component into UTF-8 (lossy replacement on non-UTF-8 bytes).
pub fn percent_decode(input: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(input, false)).into_owned()
}

/// Form-decodes an `application/x-www-form-urlencoded` token (`+` -> space and `%XX`).
pub fn form_decode(input: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(input, true)).into_owned()
}

/// Splits an HTTP request target (e.g. `/phone/gethash/?pid=123&sign=abc` or full URL)
/// into `(normalized_path, raw_query)`.
pub fn split_request_target(target: &str) -> (String, String) {
    let without_scheme = if let Some(rest) = target.strip_prefix("http://") {
        match rest.find('/') {
            Some(pos) => &rest[pos..],
            None => "/",
        }
    } else if let Some(rest) = target.strip_prefix("https://") {
        match rest.find('/') {
            Some(pos) => &rest[pos..],
            None => "/",
        }
    } else {
        target
    };

    let without_fragment = match without_scheme.find('#') {
        Some(pos) => &without_scheme[..pos],
        None => without_scheme,
    };

    let (raw_path, raw_query) = match without_fragment.find('?') {
        Some(pos) => (&without_fragment[..pos], &without_fragment[pos + 1..]),
        None => (without_fragment, ""),
    };

    (normalize_path(raw_path), raw_query.to_string())
}

/// Normalizes an HTTP URL path by collapsing duplicate slashes and stripping trailing
/// slashes (except for root `/`), so `/phone/serverlist/` and `/phone/serverlist`
/// route identically.
pub fn normalize_path(raw_path: &str) -> String {
    let decoded = percent_decode(raw_path);
    let mut out = String::with_capacity(decoded.len().max(1));
    out.push('/');
    for seg in decoded.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if out.len() > 1 {
            out.push('/');
        }
        out.push_str(seg);
    }
    out
}

/// Parses a query string into a `BTreeMap<String, String>` using standard URL
/// percent-decoding (`plus_as_space = true` supports both `%20` and `+`).
pub fn parse_query_params(raw_query: &str) -> BTreeMap<String, String> {
    let q = raw_query.strip_prefix('?').unwrap_or(raw_query);
    let mut map = BTreeMap::new();
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        match part.find('=') {
            Some(eq) => {
                let k = form_decode(&part[..eq]);
                let v = form_decode(&part[eq + 1..]);
                map.insert(k, v);
            }
            None => {
                map.insert(form_decode(part), String::new());
            }
        }
    }
    map
}

/// Parses an HTTP `Cookie:` header (e.g. `PHPSESSID=abc123; GSESS_ID=xyz789`)
/// into key-value pairs.
pub fn parse_cookie_header(header_val: &str) -> BTreeMap<String, String> {
    let mut cookies = BTreeMap::new();
    for pair in header_val.split(';') {
        let trimmed = pair.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let name = trimmed[..eq].trim();
            let mut val = trimmed[eq + 1..].trim();
            if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
                val = &val[1..val.len() - 1];
            }
            if !name.is_empty() {
                cookies.insert(name.to_string(), val.to_string());
            }
        }
    }
    cookies
}
