#![forbid(unsafe_code)]

//! Cryptographic, integrity, URL-signing, and asset-decryption primitives matching the
//! GServer (`vnsg2`) Win32 C++ client:
//! - RFC 1321 MD5 (`md5_raw`, `md5_hex`)
//! - `LCPacket::computeBabelTimeMd5` (`"BabelTime"` + adjacent-byte XOR stream)
//! - `bt::Platform::sortUrlParams` / `signUrl` (tolerant of `?&` / `&&` empty query segments)
//! - `gserver::assets::DecryptBasic` (`0xFEFEFEFE` 32-byte header + 16 key tables from `07_bt_decrypt_keys.inc`)

use std::collections::BTreeMap;
use std::fmt;

/// Default fallback SDK signing key from `BtPlatformWin32.h:158`.
pub const DEFAULT_RUSSIA_SIGN_KEY: &str = "9c98948d313ccdAE962e899a60692c_russia";

/// Live `vnsg2` (`platform/config.json`) SDK signing key verified in `live_capture_filtered.log`.
pub const VNSG2_LIVE_SIGN_KEY: &str = "2A0ce68806G8aJ58Mo6_vnsg2@OmZHv";

/// Local private-server SDK signing key used in `client_launcher/platform_override/platform/config.json`.
pub const LOCAL_DEV_SIGN_KEY: &str = "2A0ce68806G8aJ58Mo6_vnsg2@OmZHv";

const MD5_SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_TABLE: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
    0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
    0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
    0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
    0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
    0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
    0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
    0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// Streaming RFC 1321 MD5 hasher (`#![forbid(unsafe_code)]`, zero allocations).
#[derive(Clone)]
pub struct Md5Hasher {
    state: [u32; 4],
    bit_count: u64,
    buffer: [u8; 64],
}

impl Default for Md5Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5Hasher {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            bit_count: 0,
            buffer: [0u8; 64],
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let idx = ((self.bit_count >> 3) & 63) as usize;
        self.bit_count = self.bit_count.wrapping_add((data.len() as u64).wrapping_mul(8));

        let mut i = 0usize;
        if idx > 0 {
            let part_len = 64 - idx;
            if data.len() >= part_len {
                self.buffer[idx..idx + part_len].copy_from_slice(&data[..part_len]);
                let block = self.buffer;
                self.transform(&block);
                i = part_len;
            } else {
                self.buffer[idx..idx + data.len()].copy_from_slice(data);
                return;
            }
        }

        while i + 64 <= data.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[i..i + 64]);
            self.transform(&block);
            i += 64;
        }

        if i < data.len() {
            let rem = data.len() - i;
            self.buffer[..rem].copy_from_slice(&data[i..]);
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let bits = self.bit_count.to_le_bytes();
        let idx = ((self.bit_count >> 3) & 63) as usize;
        let pad_len = if idx < 56 { 56 - idx } else { 120 - idx };

        let mut padding = [0u8; 64];
        padding[0] = 0x80;
        self.update(&padding[..pad_len]);
        self.update(&bits);

        let mut out = [0u8; 16];
        for (chunk, &word) in out.chunks_exact_mut(4).zip(self.state.iter()) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn transform(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            m[i] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) & 15)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) & 15)
            } else {
                (c ^ (b | (!d)), (7 * i) & 15)
            };

            let tmp = d;
            d = c;
            c = b;
            let sum = a
                .wrapping_add(f)
                .wrapping_add(MD5_TABLE[i])
                .wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(MD5_SHIFT[i]));
            a = tmp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

/// Computes raw 16-byte RFC 1321 MD5 digest.
pub fn md5_raw(data: &[u8]) -> [u8; 16] {
    let mut h = Md5Hasher::new();
    h.update(data);
    h.finalize()
}

/// Computes lowercase 32-character hexadecimal MD5 digest.
pub fn md5_hex(data: &[u8]) -> String {
    bytes_to_hex_lower(&md5_raw(data))
}

/// Formats bytes as lowercase hexadecimal string.
pub fn bytes_to_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Computes the BabelTime integrity MD5 digest for a client->server `LCPacket` body
/// (`LCPacket.cpp:242-265` / `GameUtil::makeLCPacket` @ `0x2b4b04`).
pub fn compute_babeltime_md5(body: &[u8]) -> [u8; 16] {
    let mut hasher = Md5Hasher::new();
    hasher.update(b"BabelTime");
    if !body.is_empty() {
        let mut chunk = [0u8; 256];
        let mut fill = 0usize;
        for i in 0..(body.len() - 1) {
            chunk[fill] = body[i + 1] ^ body[i];
            fill += 1;
            if fill == chunk.len() {
                hasher.update(&chunk);
                fill = 0;
            }
        }
        if fill > 0 {
            hasher.update(&chunk[..fill]);
        }
        let last = body[body.len() - 1] ^ ((body.len() & 0xFF) as u8);
        hasher.update(&[last]);
    }
    hasher.finalize()
}

// ============================================================================
// BtPlatformWin32.cpp URL Query Sorting, Signing & Tolerant Query Parsing
// ============================================================================

/// Error returned when validating a signed SDK HTTP request (`BtPlatformWin32.cpp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    MissingSignParam,
    InvalidSignature { expected: String, actual: String },
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSignParam => write!(f, "missing 'sign' query parameter"),
            Self::InvalidSignature { expected, actual } => {
                write!(f, "signature mismatch: expected {}, got {}", expected, actual)
            }
        }
    }
}

/// Exact port of `bt::Platform::sortUrlParams` (`BtPlatformWin32.cpp:1391-1414`):
/// - Extracts substring after first `?` (or uses full string if no `?` is present).
/// - Splits on `&`, **skipping empty segments** (`amp == start`, such as `?&pid=...` or `a=1&&b=2`),
///   while **preserving keys with empty values** (`pid=` has length 4, so `amp > start`!).
/// - Sorts raw `"key=value"` segments ASCIIbetically BEFORE URL-decoding.
/// - Concatenates sorted segments with NO `&` separator.
pub fn sort_url_params(url_or_query: &str) -> String {
    let query = match url_or_query.find('?') {
        Some(idx) => &url_or_query[idx + 1..],
        None => url_or_query,
    };
    let mut parts: Vec<&str> = query.split('&').filter(|seg| !seg.is_empty()).collect();
    parts.sort_unstable();
    let total_len: usize = parts.iter().map(|s| s.len()).sum();
    let mut out = String::with_capacity(total_len);
    for p in parts {
        out.push_str(p);
    }
    out
}

/// Exact port of `bt::Platform::signUrl(url, customKey)` (`BtPlatformWin32.cpp:1426-1432`)
/// with deterministic `time_sec` injection.
pub fn sign_platform_url(url: &str, time_sec: i64, sign_key: &str) -> String {
    let url_with_time = format!("{}&time={}", url, time_sec);
    let mut to_sign = sort_url_params(&url_with_time);
    to_sign.push_str(sign_key);
    let sig = md5_hex(to_sign.as_bytes());
    format!("{}&sign={}", url_with_time, sig)
}

/// Verifies a signed SDK URL (such as `/phone/gethash/?&pid=...&time=...&sign=...`) against
/// `sign_key`, properly ignoring empty query segments (`?&`, `&&`) while retaining `key=` pairs
/// and computing the signature over raw pre-decoded segments excluding `sign=...`.
pub fn verify_platform_url_signature(
    url_or_query: &str,
    sign_key: &str,
) -> Result<(), SignatureError> {
    let query = match url_or_query.find('?') {
        Some(idx) => &url_or_query[idx + 1..],
        None => url_or_query,
    };

    let mut actual_sign: Option<&str> = None;
    let mut parts: Vec<&str> = Vec::new();
    for seg in query.split('&') {
        if seg.is_empty() {
            continue;
        }
        if let Some(val) = seg.strip_prefix("sign=") {
            actual_sign = Some(val);
        } else {
            parts.push(seg);
        }
    }

    let actual = actual_sign.ok_or(SignatureError::MissingSignParam)?;
    parts.sort_unstable();
    let mut to_sign = String::new();
    for p in parts {
        to_sign.push_str(p);
    }
    to_sign.push_str(sign_key);
    let expected = md5_hex(to_sign.as_bytes());

    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(SignatureError::InvalidSignature {
            expected,
            actual: actual.to_string(),
        })
    }
}

/// Percent-decodes an HTTP query component (`%XX` and `+` -> `' '`).
pub fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Tolerantly parses HTTP query strings generated by `BtPlatformWin32.cpp` / `libnew_sdk.so`,
/// including leading `?&pid=...`, duplicate `&&`, trailing `&`, and empty `pid=` values.
pub fn parse_tolerant_query_params(url_or_query: &str) -> BTreeMap<String, String> {
    let query = match url_or_query.find('?') {
        Some(idx) => &url_or_query[idx + 1..],
        None => url_or_query,
    };
    let mut map = BTreeMap::new();
    for seg in query.split('&') {
        if seg.is_empty() {
            continue;
        }
        match seg.split_once('=') {
            Some((k, v)) if !k.is_empty() => {
                map.insert(percent_decode(k), percent_decode(v));
            }
            None => {
                map.insert(percent_decode(seg), String::new());
            }
            _ => {}
        }
    }
    map
}

// ============================================================================
// BtDecryptHook.cpp Asset Encryption / Decryption (0xFEFEFEFE + 16 Key Tables)
// ============================================================================

pub const BT_DECRYPT_MAGIC: u32 = 0xFEFE_FEFE;
pub const BT_DECRYPT_HEADER_SIZE: usize = 32;

const KEY_00: [u8; 32] = [0x70, 0x96, 0xB2, 0x9D, 0xD0, 0x52, 0x19, 0x9A, 0x79, 0xC6, 0xD9, 0x6C, 0x6B, 0xBD, 0xDC, 0x4C, 0x51, 0x52, 0xAC, 0xFA, 0x99, 0x5C, 0x65, 0x71, 0x9B, 0x06, 0x80, 0x4A, 0x02, 0x6E, 0x20, 0x02];
const KEY_01: [u8; 32] = [0x57, 0xAE, 0x6D, 0x16, 0x45, 0x2E, 0x43, 0x74, 0x61, 0x76, 0xF0, 0xA4, 0x7F, 0x8C, 0x91, 0x79, 0xBF, 0x6E, 0x3D, 0x76, 0xA6, 0x88, 0x8E, 0x07, 0x2C, 0x12, 0xED, 0x22, 0x86, 0xCE, 0x98, 0xF7];
const KEY_02: [u8; 32] = [0x96, 0x9D, 0x52, 0x9A, 0xC6, 0x6C, 0xBD, 0x4C, 0x52, 0xFA, 0x5C, 0x71, 0x06, 0x4A, 0x6E, 0x02, 0x10, 0x80, 0x08, 0x40, 0x72, 0x4C, 0x56, 0x29, 0x97, 0x4A, 0x5B, 0xE0, 0x58, 0x53, 0x4D, 0x38];
const KEY_03: [u8; 32] = [0x70, 0xB2, 0xD0, 0x19, 0x79, 0xD9, 0x6B, 0xDC, 0x51, 0xAC, 0x99, 0x65, 0x9B, 0x80, 0x02, 0x20, 0x40, 0x04, 0x04, 0x01, 0x99, 0x05, 0x10, 0x9E, 0xF0, 0x70, 0x04, 0x87, 0x27, 0xB2, 0x53, 0xAF];
const KEY_04: [u8; 10] = [0x96, 0xB2, 0x9D, 0x52, 0x79, 0xBD, 0x5C, 0x04, 0xE0, 0xCA];
const KEY_05: [u8; 32] = [0xB2, 0x9D, 0x52, 0x9A, 0x6C, 0xBD, 0x52, 0xFA, 0x71, 0x6E, 0x02, 0x08, 0x72, 0x4C, 0x29, 0x5B, 0x53, 0x4D, 0x3F, 0x2F, 0x33, 0xF9, 0xB6, 0xCA, 0x63, 0x6D, 0x43, 0x61, 0x7F, 0x91, 0x3D, 0x2C];
const KEY_06: [u8; 32] = [0x9D, 0x19, 0x79, 0xD9, 0x6B, 0xBD, 0x51, 0x99, 0x65, 0x71, 0x9B, 0x01, 0x99, 0x05, 0x29, 0x97, 0x5B, 0x87, 0x27, 0x53, 0x53, 0x4D, 0xAF, 0xCB, 0x3F, 0xAD, 0x2F, 0xB3, 0x33, 0x43, 0x89, 0x1B];
const KEY_07: [u8; 32] = [0x70, 0x96, 0xB2, 0xD0, 0x52, 0x9A, 0xC6, 0x6C, 0xDC, 0x4C, 0x52, 0xAC, 0xFA, 0x5C, 0x06, 0x80, 0x4A, 0x02, 0x6E, 0x20, 0x02, 0x40, 0x10, 0x04, 0x80, 0x04, 0x08, 0x40, 0x72, 0x4C, 0x10, 0x56];
const KEY_08: [u8; 32] = [0x74, 0x98, 0x86, 0xED, 0x2C, 0x8E, 0xA6, 0x3D, 0xBF, 0x91, 0x7F, 0xF0, 0x61, 0x43, 0x45, 0x6D, 0x57, 0x63, 0x76, 0xCA, 0x5F, 0x4B, 0xB6, 0x5D, 0xF9, 0x1B, 0x43, 0x33, 0x2F, 0x40, 0x3F, 0xCB];
const KEY_09: [u8; 32] = [0xF7, 0xCE, 0x22, 0x12, 0x07, 0x88, 0x76, 0x6E, 0x79, 0x8C, 0xA4, 0x76, 0x74, 0x2E, 0x16, 0xAE, 0xAF, 0x18, 0xB3, 0xA3, 0x98, 0x38, 0x44, 0x33, 0xEC, 0x89, 0x7C, 0xB3, 0xAD, 0x16, 0x3E, 0xBE];
const KEY_0A: [u8; 32] = [0xF7, 0xED, 0x07, 0x3D, 0xBF, 0x79, 0x91, 0x7F, 0x61, 0x43, 0x45, 0x6D, 0x57, 0xAF, 0x63, 0xB3, 0xA3, 0x5F, 0x4B, 0x5D, 0x33, 0xF9, 0x1B, 0x89, 0x43, 0x33, 0xB3, 0x2F, 0xAD, 0x3F, 0xCB, 0xAF];
const KEY_0B: [u8; 32] = [0x74, 0x98, 0xCE, 0x86, 0x22, 0x12, 0x2C, 0x8E, 0x88, 0xA6, 0x76, 0x6E, 0x8C, 0xA4, 0xF0, 0x76, 0x74, 0x2E, 0x16, 0xAE, 0x18, 0x76, 0xCA, 0x98, 0x38, 0xB6, 0x44, 0xEC, 0x7C, 0x40, 0x16, 0x3E];
const KEY_0C: [u8; 32] = [0xDA, 0x15, 0x7E, 0x7F, 0xB3, 0x6D, 0xAE, 0x90, 0xFF, 0xB2, 0x3D, 0xF4, 0x95, 0x36, 0x94, 0x1E, 0x30, 0x84, 0xFC, 0xC1, 0x27, 0xB9, 0xBA, 0x75, 0x59, 0x26, 0xA9, 0xA7, 0xCF, 0x5F, 0x06, 0x77];
const KEY_0D: [u8; 32] = [0xE4, 0xCD, 0x39, 0x21, 0xED, 0x01, 0xE1, 0xFB, 0xA6, 0x61, 0xF7, 0xD6, 0x86, 0x48, 0x4E, 0xC2, 0x0C, 0x7C, 0x07, 0xA7, 0x6D, 0x3C, 0xB8, 0x39, 0x27, 0x46, 0xD4, 0xB9, 0xA6, 0x00, 0x9E, 0x7A];
const KEY_0E: [u8; 32] = [0x06, 0x4F, 0x22, 0xB3, 0x3F, 0x45, 0x28, 0x28, 0xA3, 0xA6, 0xF5, 0xD6, 0xA1, 0xCA, 0x70, 0x22, 0x50, 0x84, 0x0C, 0x41, 0x0B, 0x51, 0x66, 0xC7, 0x87, 0xBA, 0x5F, 0x67, 0x7F, 0x05, 0xA0, 0xE7];
const KEY_0F: [u8; 32] = [0x48, 0x6D, 0x6B, 0x13, 0x9F, 0xD7, 0x99, 0x9D, 0xFE, 0x93, 0xC1, 0x0C, 0x86, 0x4C, 0x8E, 0x42, 0x14, 0x84, 0x09, 0xD9, 0x77, 0x5C, 0xF4, 0x19, 0x07, 0x4E, 0xE2, 0x07, 0x0A, 0xA6, 0xFC, 0xF6];

pub const BT_KEY_TABLES: [&[u8]; 16] = [
    &KEY_00, &KEY_01, &KEY_02, &KEY_03, &KEY_04, &KEY_05, &KEY_06, &KEY_07,
    &KEY_08, &KEY_09, &KEY_0A, &KEY_0B, &KEY_0C, &KEY_0D, &KEY_0E, &KEY_0F,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtDecryptError {
    TruncatedMagicHeader,
    NotEncrypted,
    PayloadLengthMismatch { declared: u32, actual: usize },
    UnsupportedAlgoIndex(u8),
    InvalidKeyIndex(u8),
}

impl fmt::Display for BtDecryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedMagicHeader => write!(f, "truncated 0xFEFEFEFE header (< 32 bytes)"),
            Self::NotEncrypted => write!(f, "buffer does not start with 0xFEFEFEFE magic"),
            Self::PayloadLengthMismatch { declared, actual } => write!(
                f,
                "header payload_len {} != actual {}",
                declared, actual
            ),
            Self::UnsupportedAlgoIndex(a) => write!(f, "unsupported algo index {}", a),
            Self::InvalidKeyIndex(k) => write!(f, "invalid key index {} (expected 0..=15)", k),
        }
    }
}

/// Decrypts an GServer `0xFEFEFEFE`-framed asset (`BtDecryptHook.cpp:196-331`).
pub fn bt_decrypt_asset(raw: &[u8]) -> Result<Vec<u8>, BtDecryptError> {
    if raw.len() >= 4 && raw.len() < BT_DECRYPT_HEADER_SIZE {
        let magic = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        if magic == BT_DECRYPT_MAGIC {
            return Err(BtDecryptError::TruncatedMagicHeader);
        }
        return Err(BtDecryptError::NotEncrypted);
    }
    if raw.len() < BT_DECRYPT_HEADER_SIZE {
        return Err(BtDecryptError::NotEncrypted);
    }
    let magic = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if magic != BT_DECRYPT_MAGIC {
        return Err(BtDecryptError::NotEncrypted);
    }
    let declared_len = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    let algo = raw[8];
    let key_idx = raw[9];
    let actual_len = raw.len() - BT_DECRYPT_HEADER_SIZE;
    if declared_len as usize != actual_len {
        return Err(BtDecryptError::PayloadLengthMismatch {
            declared: declared_len,
            actual: actual_len,
        });
    }
    if algo != 0 && algo != 1 {
        return Err(BtDecryptError::UnsupportedAlgoIndex(algo));
    }
    if key_idx as usize >= BT_KEY_TABLES.len() {
        return Err(BtDecryptError::InvalidKeyIndex(key_idx));
    }

    let key = BT_KEY_TABLES[key_idx as usize];
    let mut payload = raw[BT_DECRYPT_HEADER_SIZE..].to_vec();
    let offset = actual_len % key.len();
    let mut i = 0usize;
    while i < actual_len {
        payload[i] ^= key[(i + offset) % key.len()];
        i += 2;
    }
    Ok(payload)
}

/// Encrypts plaintext into an GServer `0xFEFEFEFE`-framed asset (`algo = 1`, `key_idx` in `0..=15`).
pub fn bt_encrypt_asset(plaintext: &[u8], key_idx: u8) -> Result<Vec<u8>, BtDecryptError> {
    if key_idx as usize >= BT_KEY_TABLES.len() {
        return Err(BtDecryptError::InvalidKeyIndex(key_idx));
    }
    let key = BT_KEY_TABLES[key_idx as usize];
    let plen = plaintext.len();
    let mut out = vec![0u8; BT_DECRYPT_HEADER_SIZE + plen];
    out[0..4].copy_from_slice(&BT_DECRYPT_MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&(plen as u32).to_le_bytes());
    out[8] = 1;
    out[9] = key_idx;
    out[BT_DECRYPT_HEADER_SIZE..].copy_from_slice(plaintext);

    let payload = &mut out[BT_DECRYPT_HEADER_SIZE..];
    let offset = plen % key.len();
    let mut i = 0usize;
    while i < plen {
        payload[i] ^= key[(i + offset) % key.len()];
        i += 2;
    }
    Ok(out)
}
