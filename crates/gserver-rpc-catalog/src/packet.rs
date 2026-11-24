//! Pure-Rust `LCPacket` wire framing and RFC 1321 MD5 / `"BabelTime"` XOR-MD5 checksums.
//!
//! ### Client -> Server (`LCPacket.cpp`, 24-byte header + AMF3 body)
//! - `[0..4]`: Big-endian `u32` `body.len()`
//! - `[4..8]`: Flags `[zip_flag, 1, 1, 1]` (`zip_flag = 1` if zlib-compressed, `0` uncompressed)
//! - `[8..24]`: Raw 16-byte MD5 digest over:
//!   - `"BabelTime"` (9 ASCII bytes)
//!   - `body[i + 1] ^ body[i]` for `0 <= i < body.len() - 1`
//!   - `body[body.len() - 1] ^ ((body.len() & 0xFF) as u8)` (when `!body.is_empty()`)
//!
//! ### Server -> Client (`CConnection.cpp` + `Network.lua:onBodyData(body, head)`, 8-byte header)
//! - `[0..4]`: Big-endian `u32` `body.len()`
//! - `[4]`: Reserved (`0`)
//! - `[5]`: `zipped` flag (`string.byte(head, 6)` in 1-based Lua; `0` = raw AMF3, non-zero = zlib)
//! - `[6]`: `multi` flag (`string.byte(head, 7)` in 1-based Lua; `0` = single envelope, non-zero = `ret` array of sub-packets)
//! - `[7]`: Reserved (`0`)

use std::fmt;

/// Client -> Server `LCPacket` header length in bytes.
pub const CLIENT_HEADER_LEN: usize = 24;
/// Default Server -> Client `CConnection` header length in bytes.
pub const SERVER_HEADER_LEN: usize = 8;
/// Hardcoded salt prefix from `LCPacket.cpp`.
pub const BABELTIME_MAGIC_SALT: &[u8; 9] = b"BabelTime";

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// Pure-Rust RFC 1321 MD5 digest.
pub fn md5(input: &[u8]) -> [u8; 16] {
    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(input.len() + 72);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let off = i * 4;
            *word =
                u32::from_le_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

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
            let sum = a.wrapping_add(f).wrapping_add(MD5_K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(MD5_S[i]));
            a = temp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    digest[0..4].copy_from_slice(&a0.to_le_bytes());
    digest[4..8].copy_from_slice(&b0.to_le_bytes());
    digest[8..12].copy_from_slice(&c0.to_le_bytes());
    digest[12..16].copy_from_slice(&d0.to_le_bytes());
    digest
}

/// Compute the 16-byte `LCPacket.cpp` checksum over `body` (`"BabelTime"` + adjacent-XOR buffer).
pub fn babeltime_body_checksum(body: &[u8]) -> [u8; 16] {
    let mut scratch = Vec::with_capacity(BABELTIME_MAGIC_SALT.len() + body.len());
    scratch.extend_from_slice(BABELTIME_MAGIC_SALT);
    if !body.is_empty() {
        for i in 0..(body.len() - 1) {
            scratch.push(body[i + 1] ^ body[i]);
        }
        scratch.push(body[body.len() - 1] ^ ((body.len() & 0xFF) as u8));
    }
    md5(&scratch)
}

/// Wire framing error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    IncompleteHeader {
        needed: usize,
        available: usize,
    },
    IncompleteBody {
        needed: usize,
        available: usize,
    },
    InvalidFlags([u8; 4]),
    ChecksumMismatch {
        expected: [u8; 16],
        actual: [u8; 16],
    },
}

impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteHeader { needed, available } => {
                write!(
                    f,
                    "incomplete packet header: need {} bytes, have {}",
                    needed, available
                )
            }
            Self::IncompleteBody { needed, available } => {
                write!(
                    f,
                    "incomplete packet body: need {} bytes, have {}",
                    needed, available
                )
            }
            Self::InvalidFlags(fl) => write!(f, "invalid LCPacket flags {:?}", fl),
            Self::ChecksumMismatch { .. } => write!(f, "LCPacket BabelTime MD5 checksum mismatch"),
        }
    }
}

impl std::error::Error for PacketError {}

/// Client -> Server 24-byte `LCPacket` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFrame {
    pub zipped: bool,
    pub body: Vec<u8>,
}

impl ClientFrame {
    pub fn uncompressed(body: Vec<u8>) -> Self {
        Self {
            zipped: false,
            body,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let len = self.body.len() as u32;
        let checksum = babeltime_body_checksum(&self.body);
        let mut out = Vec::with_capacity(CLIENT_HEADER_LEN + self.body.len());
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&[u8::from(self.zipped), 1, 1, 1]);
        out.extend_from_slice(&checksum);
        out.extend_from_slice(&self.body);
        out
    }

    /// Decode one `ClientFrame` from the front of `stream`, returning `(frame, consumed_bytes)`.
    pub fn decode(stream: &[u8]) -> Result<(Self, usize), PacketError> {
        if stream.len() < CLIENT_HEADER_LEN {
            return Err(PacketError::IncompleteHeader {
                needed: CLIENT_HEADER_LEN,
                available: stream.len(),
            });
        }
        let body_len = u32::from_be_bytes([stream[0], stream[1], stream[2], stream[3]]) as usize;
        let flags = [stream[4], stream[5], stream[6], stream[7]];
        if (flags[0] > 1) || flags[1] != 1 || flags[2] != 1 || flags[3] != 1 {
            return Err(PacketError::InvalidFlags(flags));
        }
        let total = CLIENT_HEADER_LEN + body_len;
        if stream.len() < total {
            return Err(PacketError::IncompleteBody {
                needed: total,
                available: stream.len(),
            });
        }
        let mut actual = [0u8; 16];
        actual.copy_from_slice(&stream[8..24]);
        let body = stream[24..total].to_vec();
        let expected = babeltime_body_checksum(&body);
        if actual != expected {
            return Err(PacketError::ChecksumMismatch { expected, actual });
        }
        Ok((
            Self {
                zipped: flags[0] == 1,
                body,
            },
            total,
        ))
    }
}

/// Server -> Client 8-byte `CConnection` frame (`Network.lua:onBodyData(body, head)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFrame {
    /// Byte 5 (Lua 1-indexed byte 6): `true` if `body` is zlib-compressed.
    pub zipped: bool,
    /// Byte 6 (Lua 1-indexed byte 7): `true` if `data.ret` is a 1-based multi-packet batch array.
    pub multi: bool,
    pub body: Vec<u8>,
}

impl ServerFrame {
    pub fn single_uncompressed(body: Vec<u8>) -> Self {
        Self {
            zipped: false,
            multi: false,
            body,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let len = self.body.len() as u32;
        let mut out = Vec::with_capacity(SERVER_HEADER_LEN + self.body.len());
        out.extend_from_slice(&len.to_be_bytes());
        out.push(0);
        out.push(u8::from(self.zipped));
        out.push(u8::from(self.multi));
        out.push(0);
        out.extend_from_slice(&self.body);
        out
    }

    /// Decode one `ServerFrame` from the front of `stream`, returning `(frame, consumed_bytes)`.
    pub fn decode(stream: &[u8]) -> Result<(Self, usize), PacketError> {
        if stream.len() < SERVER_HEADER_LEN {
            return Err(PacketError::IncompleteHeader {
                needed: SERVER_HEADER_LEN,
                available: stream.len(),
            });
        }
        let body_len = u32::from_be_bytes([stream[0], stream[1], stream[2], stream[3]]) as usize;
        let total = SERVER_HEADER_LEN + body_len;
        if stream.len() < total {
            return Err(PacketError::IncompleteBody {
                needed: total,
                available: stream.len(),
            });
        }
        let zipped = stream[5] != 0;
        let multi = stream[6] != 0;
        let body = stream[SERVER_HEADER_LEN..total].to_vec();
        Ok((
            Self {
                zipped,
                multi,
                body,
            },
            total,
        ))
    }
}
