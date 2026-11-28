//! Pure-Rust RFC 1321 MD5 message-digest, RFC 2104 HMAC-MD5, and GServer (`vnsg2`)
//! MAPI (`BtPlatformWin32::sortUrlParams` + `appKey`) & Platform Account/Guest URL signing.

/// Production `vnsg2` MAPI signing key verified against `live_capture_filtered.log`.
pub const VNSG2_MAPI_APP_KEY: &str = "2A0ce68806G8aJ58Mo6_vnsg2@OmZHv";

/// Fallback default SDK signing key from `BtPlatformWin32.h` (`kDefaultSignKey`).
pub const DEFAULT_FALLBACK_SIGN_KEY: &str = "9c98948d313ccdAE962e899a60692c_russia";

/// Default Platform GTSDK `ghk` secret from `AccountAuthProvider.cpp` (`kDefaultAuthSecret`).
pub const DEFAULT_AUTH_SECRET: &str = "d345c6c5c29669eae007b682625fbf42";

/// Default Platform GTSDK `gameID` from `AccountAuthProvider.cpp` (`kDefaultGtsdkGameId`).
pub const DEFAULT_GTSDK_GAME_ID: &str = "GCLIENT";

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
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

/// Streaming RFC 1321 MD5 hasher.
#[derive(Clone, Debug)]
pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    total_bytes: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    /// Creates a new RFC 1321 MD5 state initialized with standard IV constants.
    pub const fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buffer: [0u8; 64],
            total_bytes: 0,
        }
    }

    /// Feeds arbitrary bytes into the MD5 state.
    pub fn update(&mut self, input: &[u8]) {
        let mut offset = (self.total_bytes as usize) & 0x3F;
        self.total_bytes = self.total_bytes.wrapping_add(input.len() as u64);

        let mut pos = 0usize;
        if offset > 0 && offset + input.len() >= 64 {
            let needed = 64 - offset;
            self.buffer[offset..64].copy_from_slice(&input[..needed]);
            let block = self.buffer;
            self.compress(&block);
            pos = needed;
            offset = 0;
        }

        while pos + 64 <= input.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&input[pos..pos + 64]);
            self.compress(&block);
            pos += 64;
        }

        let rem = input.len() - pos;
        if rem > 0 {
            self.buffer[offset..offset + rem].copy_from_slice(&input[pos..]);
        }
    }

    /// Finalizes the MD5 computation and returns the 16-byte raw digest.
    pub fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.total_bytes.wrapping_mul(8);
        let index = (self.total_bytes as usize) & 0x3F;
        let pad_len = if index < 56 { 56 - index } else { 120 - index };

        let mut padding = [0u8; 64];
        padding[0] = 0x80;
        self.update(&padding[..pad_len]);
        self.update(&bit_len.to_le_bytes());

        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.state[0].to_le_bytes());
        out[4..8].copy_from_slice(&self.state[1].to_le_bytes());
        out[8..12].copy_from_slice(&self.state[2].to_le_bytes());
        out[12..16].copy_from_slice(&self.state[3].to_le_bytes());
        out
    }

    /// Finalizes the MD5 computation and returns the lowercase 32-character hex digest.
    pub fn finalize_hex(self) -> String {
        bytes_to_hex_lower(&self.finalize())
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let j = i * 4;
            *word = u32::from_le_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | (!d)), (7 * i) % 16)
            };

            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

/// Encodes a byte slice as lowercase hexadecimal.
pub fn bytes_to_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

/// Computes the 16-byte RFC 1321 MD5 digest of `data`.
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finalize()
}

/// Computes the 32-character lowercase hexadecimal MD5 digest of `data`.
pub fn md5_hex(data: impl AsRef<[u8]>) -> String {
    bytes_to_hex_lower(&md5(data.as_ref()))
}

/// Computes the 16-byte RFC 2104 HMAC-MD5 authentication tag for `(key, message)`.
pub fn hmac_md5(key: &[u8], message: &[u8]) -> [u8; 16] {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = md5(key);
        key_block[..16].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Md5::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Md5::new();
    outer.update(&opad);
    outer.update(&inner_digest);
    outer.finalize()
}

/// Computes the 32-character lowercase hexadecimal RFC 2104 HMAC-MD5 digest.
pub fn hmac_md5_hex(key: &[u8], message: &[u8]) -> String {
    bytes_to_hex_lower(&hmac_md5(key, message))
}

/// Constant-time ASCII case-insensitive comparison for 32-byte hex digests.
pub fn constant_time_eq_hex(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x.to_ascii_lowercase() ^ y.to_ascii_lowercase();
    }
    diff == 0
}

/// Exact port of `bt::Platform::sortUrlParams` (`BtPlatformWin32.cpp:1391`),
/// with optional exclusion of an existing `sign=...` parameter when verifying
/// already-signed URLs on the server.
///
/// Extracts the substring after `?` (or uses `url_or_query` directly if no `?`
/// is present), splits on `&` skipping empty segments and any `sign=...` token
/// when `strip_sign_param` is `true`, sorts the raw `key=value` segments in
/// lexicographical byte order, and concatenates them WITHOUT `&` separators.
pub fn sort_url_params(url_or_query: &str, strip_sign_param: bool) -> String {
    let query = match url_or_query.find('?') {
        Some(idx) => &url_or_query[idx + 1..],
        None => url_or_query,
    };

    let mut parts: Vec<&str> = query
        .split('&')
        .filter(|seg| !seg.is_empty())
        .filter(|seg| {
            if !strip_sign_param {
                return true;
            }
            !(seg.starts_with("sign=") || *seg == "sign")
        })
        .collect();

    parts.sort_unstable();

    let total_len: usize = parts.iter().map(|s| s.len()).sum();
    let mut out = String::with_capacity(total_len);
    for p in parts {
        out.push_str(p);
    }
    out
}

/// Computes the GServer MAPI `sign` value (`md5_hex(sortUrlParams(query) + appKey)`).
pub fn compute_mapi_signature(url_or_query: &str, app_key: &str) -> String {
    let sorted = sort_url_params(url_or_query, true);
    let mut hasher = Md5::new();
    hasher.update(sorted.as_bytes());
    hasher.update(app_key.as_bytes());
    hasher.finalize_hex()
}

/// Signs a raw MAPI URL just like `bt::Platform::signUrl(url, customKey)` by appending
/// `&time=<timestamp>&sign=<md5>` (or `?time=...` if no query exists yet).
pub fn sign_mapi_url(url: &str, timestamp_sec: i64, app_key: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    let url_with_time = format!("{}{}time={}", url, sep, timestamp_sec);
    let sig = compute_mapi_signature(&url_with_time, app_key);
    format!("{}&sign={}", url_with_time, sig)
}

/// Verifies the `sign` parameter in `raw_query` against one or more accepted `app_keys`.
/// Returns `true` if `sign` is present and matches at least one configured `app_key`.
pub fn verify_mapi_query_signature(raw_query: &str, accepted_keys: &[String]) -> bool {
    let provided_sign = raw_query
        .split('&')
        .find_map(|seg| seg.strip_prefix("sign="));
    let Some(sig) = provided_sign else {
        return false;
    };
    if sig.len() != 32 {
        return false;
    }
    let sorted = sort_url_params(raw_query, true);
    for key in accepted_keys {
        let mut hasher = Md5::new();
        hasher.update(sorted.as_bytes());
        hasher.update(key.as_bytes());
        let expected = hasher.finalize_hex();
        if constant_time_eq_hex(sig, &expected) {
            return true;
        }
    }
    false
}

/// Computes Platform Account `sig = md5(ghk + gameID + username + passMd5 + timestamp)`
/// matching `callAccountAuth` in `AccountAuthProvider.cpp:371`.
pub fn compute_account_auth_sig(
    ghk: &str,
    game_id: &str,
    username: &str,
    pass_md5: &str,
    timestamp: &str,
) -> String {
    let mut hasher = Md5::new();
    hasher.update(ghk.as_bytes());
    hasher.update(game_id.as_bytes());
    hasher.update(username.as_bytes());
    hasher.update(pass_md5.as_bytes());
    hasher.update(timestamp.as_bytes());
    hasher.finalize_hex()
}

/// Computes Platform Guest `sig = md5(ghk + gameID + deviceID + timestamp)`
/// matching `callGuestAuth` in `AccountAuthProvider.cpp:424`.
pub fn compute_guest_auth_sig(
    ghk: &str,
    game_id: &str,
    device_id: &str,
    timestamp: &str,
) -> String {
    let mut hasher = Md5::new();
    hasher.update(ghk.as_bytes());
    hasher.update(game_id.as_bytes());
    hasher.update(device_id.as_bytes());
    hasher.update(timestamp.as_bytes());
    hasher.finalize_hex()
}


/// Computes raw 16-byte MD5 digest (alias matching `gserver-protocol` API).
#[inline]
pub fn md5_digest(data: &[u8]) -> [u8; 16] {
    md5(data)
}

/// Computes the 16-byte BabelTime `LCPacket` integrity digest over a packet `body`:
///
/// - `hash_input[0..9]   = b"BabelTime"`
/// - `hash_input[9 + i]  = body[i + 1] ^ body[i]` for `0 <= i < N - 1`
/// - `hash_input[9 + N-1]= body[N - 1] ^ ((N & 0xFF) as u8)` for `N > 0`
///
/// Streaming implementation avoids allocating a temporary copy of `body`.
pub fn compute_babeltime_md5(body: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(b"BabelTime");

    if !body.is_empty() {
        let mut scratch = [0u8; 256];
        let mut idx = 0usize;
        let n = body.len();

        while idx + 1 < n {
            let chunk_end = (idx + scratch.len()).min(n - 1);
            let count = chunk_end - idx;
            for k in 0..count {
                scratch[k] = body[idx + k + 1] ^ body[idx + k];
            }
            hasher.update(&scratch[..count]);
            idx = chunk_end;
        }

        let tail = [body[n - 1] ^ ((n & 0xFF) as u8)];
        hasher.update(&tail);
    }

    hasher.finalize()
}
