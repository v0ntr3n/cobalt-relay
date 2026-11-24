//! Self-contained RFC 1321 MD5 and BabelTime `LCPacket` salted XOR-chain MD5.
//!
//! Ported from `LCPacket::computeBabelTimeMd5` / `GameUtil::makeLCPacket` (`0x2b4b04` in
//! `libcocos2dlua.so`).

const MD5_SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20,
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

/// Streaming RFC 1321 MD5 hasher.
#[derive(Clone, Debug)]
pub struct Md5 {
    state: [u32; 4],
    bit_count: u64,
    buffer: [u8; 64],
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    /// Creates a new RFC 1321 MD5 context.
    pub const fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            bit_count: 0,
            buffer: [0u8; 64],
        }
    }

    /// Feeds bytes into the MD5 state without heap allocations.
    pub fn update(&mut self, data: &[u8]) {
        let mut idx = ((self.bit_count >> 3) & 63) as usize;
        self.bit_count = self.bit_count.wrapping_add((data.len() as u64).wrapping_mul(8));

        let mut i = 0usize;
        if idx > 0 {
            let part_len = 64 - idx;
            if data.len() >= part_len {
                self.buffer[idx..64].copy_from_slice(&data[..part_len]);
                let block = self.buffer;
                self.transform(&block);
                i = part_len;
                idx = 0;
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
            self.buffer[idx..idx + rem].copy_from_slice(&data[i..]);
        }
    }

    /// Consumes the MD5 state and returns the raw 16-byte digest.
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
        for (dst, chunk) in m.iter_mut().zip(block.chunks_exact(4)) {
            *dst = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
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

/// Computes the raw 16-byte RFC 1321 MD5 digest of `data`.
pub fn md5_digest(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finalize()
}

/// Formats a 16-byte MD5 digest as a 32-character lowercase hex string.
pub fn md5_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = md5_digest(data);
    let mut out = String::with_capacity(32);
    for &b in &digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
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
