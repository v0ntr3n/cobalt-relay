//! Client-to-Server (C2S) and Server-to-Client (S2C) TCP packet framing, streaming RX buffers,
//! Zlib payload compression, and BabelTime `LCPacket` integrity verification.
//!
//! ## Verified GServer (`vnsg2`) Wire Framing
//!
//! ### 1. Client-to-Server (C2S)
//! - **Production `LCPacket` 24-byte frame** (`Network.lua` `self.mConn:send(data, true)` ->
//!   `CConnection::send(..., wrapLCPacket=true)` -> `LCPacket::makeLCPacket`):
//!   - `[0..4]` `u32` big-endian `body_len` (byte length of `body` after the 24-byte header)
//!   - `[4..8]` `[u8; 4]` protocol flags: `[1, 1, 1, 1]` when `body` is RFC 1950 Zlib-compressed,
//!     or `[0, 1, 1, 1]` when `body` is raw AMF3
//!   - `[8..24]` `[u8; 16]` raw BabelTime MD5 digest (`compute_babeltime_md5(body)`)
//!   - `[24..24+body_len]` compressed (or raw) AMF3 request payload
//! - **Raw 4-byte C2S frame** (`CConnection::send(data, false)` / raw test harnesses):
//!   - `[0..4]` `u32` big-endian `body_len`
//!   - `[4..4+body_len]` raw AMF3 request payload
//!
//! [`ClientRxDecoder`] supports [`C2sFrameMode::LcPacket24`], [`C2sFrameMode::Raw4Byte`], and
//! [`C2sFrameMode::Auto`] (seamless per-frame detection).
//!
//! ### 2. Server-to-Client (S2C)
//! - **8-byte S2C header** (`CConnection::headerSize_ == 8`, `Network:onBodyData(body, head)`):
//!   - `[0..4]` `u32` big-endian `body_len` (`1..=max_body_len`, length of `body` after header)
//!   - `[4]` `flag0` (reserved, `0x00`)
//!   - `[5]` `zipped` (`0` = raw body, non-zero / `1` = RFC 1950 Zlib stream; Lua `string.byte(head, 6)`)
//!   - `[6]` `multi` (`0` = single envelope, non-zero / `1` = multi-envelope; Lua `string.byte(head, 7)`)
//!   - `[7]` `flag3` (reserved, `0x00`)
//!   - `[8..8+body_len]` S2C payload

use std::fmt;

use crate::error::{FrameError, ProtocolError};
use crate::md5::compute_babeltime_md5;
use crate::zlib::{
    zlib_compress_with_mode, zlib_decompress_with_limit, CompressionMode,
    DEFAULT_MAX_DECOMPRESSED_SIZE,
};

/// Fixed header size of a BabelTime C2S `LCPacket` (`4 + 4 + 16 = 24` bytes).
pub const C2S_LCPACKET_HEADER_SIZE: usize = 24;
/// Fixed header size of a raw length-prefixed C2S frame (`4` bytes).
pub const C2S_RAW_HEADER_SIZE: usize = 4;
/// Fixed header size of an S2C frame (`4 + 4 = 8` bytes, matching `CConnection::headerSize_`).
pub const S2C_HEADER_SIZE: usize = 8;
/// Default maximum wire body length (`16 MiB` safe server default; client `CConnection` caps at `64 MiB`).
pub const DEFAULT_MAX_FRAME_BODY_LEN: u32 = 16 * 1024 * 1024;

/// Framing mode expected by [`ClientRxDecoder`] on incoming C2S TCP streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum C2sFrameMode {
    /// Automatically detects 24-byte `LCPacket` (`flags = [0|1, 1, 1, 1]`) vs raw 4-byte `[len][AMF3]`.
    #[default]
    Auto,
    /// Strictly requires 24-byte BabelTime `LCPacket` header + valid 16-byte BabelTime MD5.
    LcPacket24,
    /// Strictly parses `[4-byte BE bodyLen][raw AMF3 payload]`.
    Raw4Byte,
}

/// Wire format of a decoded C2S frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum C2sWireKind {
    /// 24-byte BabelTime `LCPacket` frame (`compressed` indicates `flags[0] == 1`).
    LcPacket24 { compressed: bool, md5: [u8; 16] },
    /// Raw 4-byte big-endian length-prefixed AMF3 frame.
    Raw4Byte,
}

/// Decoded Client-to-Server frame with decompressed AMF3 payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct C2sFrame {
    /// Declared wire body length (compressed size if `LcPacket24 { compressed: true }`).
    pub wire_body_len: u32,
    /// Detected or configured wire framing variant.
    pub kind: C2sWireKind,
    /// Decompressed raw AMF3 payload ready for [`crate::amf3::decode`] or [`crate::rpc::RpcRequest::from_amf3_bytes`].
    pub amf3_payload: Vec<u8>,
}

/// Observable parser stage of [`ClientRxDecoder`] for connection lifecycle diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum C2sParserState {
    /// No unconsumed bytes buffered.
    Idle,
    /// Partial 4-byte `body_len` prefix buffered (`1..=3` bytes).
    AwaitingLengthPrefix { buffered_bytes: usize },
    /// Partial `LCPacket` header buffered (`4..=23` bytes).
    AwaitingHeader {
        declared_body_len: u32,
        buffered_bytes: usize,
    },
    /// Header buffered; awaiting remaining frame payload bytes.
    AwaitingBody {
        declared_body_len: u32,
        buffered_bytes: usize,
    },
}

impl fmt::Display for C2sParserState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Idle => write!(f, "Idle"),
            Self::AwaitingLengthPrefix { buffered_bytes } => {
                write!(f, "AwaitingLengthPrefix({}/4B)", buffered_bytes)
            }
            Self::AwaitingHeader {
                declared_body_len,
                buffered_bytes,
            } => write!(
                f,
                "AwaitingHeader(buffered={}B, declared_body={}B)",
                buffered_bytes, declared_body_len
            ),
            Self::AwaitingBody {
                declared_body_len,
                buffered_bytes,
            } => write!(
                f,
                "AwaitingBody(buffered={}B, declared_body={}B)",
                buffered_bytes, declared_body_len
            ),
        }
    }
}

/// Encodes an AMF3 C2S payload into a 24-byte BabelTime `LCPacket` matching `LCPacket::makeLCPacket`.
pub fn encode_c2s_lcpacket(amf3_payload: &[u8], compress: bool) -> Result<Vec<u8>, ProtocolError> {
    encode_c2s_lcpacket_with_mode(amf3_payload, compress, CompressionMode::FixedHuffmanLz77)
}

/// Encodes an AMF3 C2S payload into a 24-byte BabelTime `LCPacket` with explicit [`CompressionMode`].
pub fn encode_c2s_lcpacket_with_mode(
    amf3_payload: &[u8],
    compress: bool,
    compression_mode: CompressionMode,
) -> Result<Vec<u8>, ProtocolError> {
    let body_cow: Vec<u8>;
    let body: &[u8] = if compress {
        body_cow = zlib_compress_with_mode(amf3_payload, compression_mode);
        &body_cow
    } else {
        amf3_payload
    };

    if body.is_empty() {
        return Err(FrameError::ZeroBodyLength.into());
    }
    let body_len =
        u32::try_from(body.len()).map_err(|_| FrameError::PayloadOverflow { len: body.len() })?;
    let md5 = compute_babeltime_md5(body);

    let mut frame = Vec::with_capacity(C2S_LCPACKET_HEADER_SIZE + body.len());
    frame.extend_from_slice(&body_len.to_be_bytes());
    frame.extend_from_slice(&[u8::from(compress), 1, 1, 1]);
    frame.extend_from_slice(&md5);
    frame.extend_from_slice(body);
    Ok(frame)
}

/// Encodes an AMF3 C2S payload into a raw 4-byte `[4B BE bodyLen][AMF3 payload]` frame.
pub fn encode_c2s_raw_frame(amf3_payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if amf3_payload.is_empty() {
        return Err(FrameError::ZeroBodyLength.into());
    }
    let body_len = u32::try_from(amf3_payload.len()).map_err(|_| FrameError::PayloadOverflow {
        len: amf3_payload.len(),
    })?;
    let mut frame = Vec::with_capacity(C2S_RAW_HEADER_SIZE + amf3_payload.len());
    frame.extend_from_slice(&body_len.to_be_bytes());
    frame.extend_from_slice(amf3_payload);
    Ok(frame)
}

/// Streaming C2S TCP frame decoder handling arbitrary partial reads and pipelined frames.
#[derive(Debug, Clone)]
pub struct ClientRxDecoder {
    buf: Vec<u8>,
    read_offset: usize,
    mode: C2sFrameMode,
    max_body_len: u32,
    max_decompressed_len: usize,
}

impl Default for ClientRxDecoder {
    fn default() -> Self {
        Self::new(C2sFrameMode::Auto)
    }
}

impl ClientRxDecoder {
    /// Creates a new streaming C2S RX decoder with default safety bounds (`16 MiB` wire, `64 MiB` inflated).
    pub fn new(mode: C2sFrameMode) -> Self {
        Self {
            buf: Vec::new(),
            read_offset: 0,
            mode,
            max_body_len: DEFAULT_MAX_FRAME_BODY_LEN,
            max_decompressed_len: DEFAULT_MAX_DECOMPRESSED_SIZE,
        }
    }

    /// Configures maximum wire `bodyLen` and maximum decompressed Zlib output bytes.
    pub fn with_limits(mut self, max_body_len: u32, max_decompressed_len: usize) -> Self {
        self.max_body_len = max_body_len;
        self.max_decompressed_len = max_decompressed_len;
        self
    }

    /// Appends newly read socket bytes into the internal streaming buffer.
    pub fn feed(&mut self, chunk: &[u8]) {
        if !chunk.is_empty() {
            self.buf.extend_from_slice(chunk);
        }
    }

    /// Number of unconsumed bytes currently buffered.
    pub fn buffered_len(&self) -> usize {
        self.buf.len().saturating_sub(self.read_offset)
    }

    /// Returns the current parser state for structured connection lifecycle diagnostics.
    pub fn parser_state(&self) -> C2sParserState {
        let avail = &self.buf[self.read_offset..];
        if avail.is_empty() {
            return C2sParserState::Idle;
        }
        if avail.len() < 4 {
            return C2sParserState::AwaitingLengthPrefix {
                buffered_bytes: avail.len(),
            };
        }
        let declared_body_len = u32::from_be_bytes([avail[0], avail[1], avail[2], avail[3]]);
        match self.mode {
            C2sFrameMode::Raw4Byte => C2sParserState::AwaitingBody {
                declared_body_len,
                buffered_bytes: avail.len(),
            },
            C2sFrameMode::LcPacket24 => {
                if avail.len() < C2S_LCPACKET_HEADER_SIZE {
                    C2sParserState::AwaitingHeader {
                        declared_body_len,
                        buffered_bytes: avail.len(),
                    }
                } else {
                    C2sParserState::AwaitingBody {
                        declared_body_len,
                        buffered_bytes: avail.len(),
                    }
                }
            }
            C2sFrameMode::Auto => {
                if avail.len() < C2S_LCPACKET_HEADER_SIZE && (avail.len() < 5 || avail[4] <= 0x01) {
                    C2sParserState::AwaitingHeader {
                        declared_body_len,
                        buffered_bytes: avail.len(),
                    }
                } else {
                    C2sParserState::AwaitingBody {
                        declared_body_len,
                        buffered_bytes: avail.len(),
                    }
                }
            }
        }
    }

    /// Attempts to decode the next complete [`C2sFrame`] from the internal buffer.
    /// Returns `Ok(None)` when more socket bytes are required.
    pub fn next_frame(&mut self) -> Result<Option<C2sFrame>, ProtocolError> {
        let avail = &self.buf[self.read_offset..];
        if avail.len() < 4 {
            return Ok(None);
        }

        let body_len = u32::from_be_bytes([avail[0], avail[1], avail[2], avail[3]]);
        if body_len == 0 {
            return Err(FrameError::ZeroBodyLength.into());
        }
        if body_len > self.max_body_len {
            return Err(FrameError::BodyTooLarge {
                body_len,
                max_body_len: self.max_body_len,
            }
            .into());
        }
        let body_len_usize = body_len as usize;

        let resolved_lc = match self.mode {
            C2sFrameMode::LcPacket24 => true,
            C2sFrameMode::Raw4Byte => false,
            C2sFrameMode::Auto => {
                if avail.len() < 5 {
                    return Ok(None);
                }
                let b4 = avail[4];
                if (0x02..=0x0C).contains(&b4) {
                    false
                } else if b4 <= 0x01 {
                    // Single-byte AMF3 Undefined (0x00) or Null (0x01) has body_len == 1.
                    if body_len_usize == 1 && (avail.len() < 8 || avail[5..8] != [1, 1, 1]) {
                        false
                    } else {
                        if avail.len() < 8 {
                            return Ok(None);
                        }
                        if avail[5..8] == [1, 1, 1] {
                            true
                        } else {
                            let mut hdr = [0u8; 8];
                            hdr.copy_from_slice(&avail[..8]);
                            return Err(FrameError::UnrecognizedC2sHeader(hdr).into());
                        }
                    }
                } else {
                    let mut hdr = [0u8; 8];
                    let copy_n = avail.len().min(8);
                    hdr[..copy_n].copy_from_slice(&avail[..copy_n]);
                    return Err(FrameError::UnrecognizedC2sHeader(hdr).into());
                }
            }
        };

        if resolved_lc {
            let total_needed = C2S_LCPACKET_HEADER_SIZE + body_len_usize;
            if avail.len() < total_needed {
                return Ok(None);
            }

            let flags = [avail[4], avail[5], avail[6], avail[7]];
            if (flags[0] != 0 && flags[0] != 1) || flags[1] != 1 || flags[2] != 1 || flags[3] != 1 {
                return Err(FrameError::InvalidLcFlags(flags).into());
            }

            let mut expected_md5 = [0u8; 16];
            expected_md5.copy_from_slice(&avail[8..24]);
            let body = &avail[24..total_needed];
            let actual_md5 = compute_babeltime_md5(body);
            if expected_md5 != actual_md5 {
                return Err(FrameError::Md5Mismatch {
                    expected: expected_md5,
                    actual: actual_md5,
                }
                .into());
            }

            let compressed = flags[0] != 0;
            let amf3_payload = if compressed {
                zlib_decompress_with_limit(body, self.max_decompressed_len)?
            } else {
                body.to_vec()
            };

            self.consume(total_needed);
            Ok(Some(C2sFrame {
                wire_body_len: body_len,
                kind: C2sWireKind::LcPacket24 {
                    compressed,
                    md5: expected_md5,
                },
                amf3_payload,
            }))
        } else {
            let total_needed = C2S_RAW_HEADER_SIZE + body_len_usize;
            if avail.len() < total_needed {
                return Ok(None);
            }
            let amf3_payload = avail[4..total_needed].to_vec();
            self.consume(total_needed);
            Ok(Some(C2sFrame {
                wire_body_len: body_len,
                kind: C2sWireKind::Raw4Byte,
                amf3_payload,
            }))
        }
    }

    /// Drains all complete frames currently available in the buffer.
    pub fn decode_all(&mut self) -> Result<Vec<C2sFrame>, ProtocolError> {
        let mut frames = Vec::new();
        while let Some(frame) = self.next_frame()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    fn consume(&mut self, count: usize) {
        self.read_offset += count;
        if self.read_offset >= self.buf.len() {
            self.buf.clear();
            self.read_offset = 0;
        } else if self.read_offset >= 4096 || self.read_offset * 2 >= self.buf.len() {
            self.buf.drain(..self.read_offset);
            self.read_offset = 0;
        }
    }
}

// ============================================================================
// Server-to-Client (S2C) 8-Byte Frame Flags, TX Encoder, and RX Decoder
// ============================================================================

/// 4-byte flag tuple at offsets `[4..8]` of the 8-byte S2C frame header.
///
/// In `Network.lua:108`: `local zipped, multi = string.byte(head, 6, 7)` (1-based Lua indexing),
/// which maps to 0-based `head[5] == zipped` and `head[6] == multi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct S2cFlags {
    /// `head[4]` (1-based byte 5): reserved (`0`).
    pub flag0: u8,
    /// `head[5]` (1-based byte 6): non-zero (`1`) when payload is RFC 1950 Zlib-compressed.
    pub zipped: u8,
    /// `head[6]` (1-based byte 7): non-zero (`1`) when payload carries multiple responses.
    pub multi: u8,
    /// `head[7]` (1-based byte 8): reserved (`0`).
    pub flag3: u8,
}

impl S2cFlags {
    /// Creates standard S2C flags from `(zipped, multi)` booleans (`flag0 = 0, flag3 = 0`).
    pub const fn new(zipped: bool, multi: bool) -> Self {
        Self {
            flag0: 0,
            zipped: if zipped { 1 } else { 0 },
            multi: if multi { 1 } else { 0 },
            flag3: 0,
        }
    }

    /// Encodes the 4 flag bytes.
    pub const fn to_bytes(self) -> [u8; 4] {
        [self.flag0, self.zipped, self.multi, self.flag3]
    }

    /// Parses the 4 flag bytes from `head[4..8]`.
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self {
            flag0: bytes[0],
            zipped: bytes[1],
            multi: bytes[2],
            flag3: bytes[3],
        }
    }

    /// Returns `true` if `zipped != 0`.
    pub const fn is_zipped(self) -> bool {
        self.zipped != 0
    }

    /// Returns `true` if `multi != 0`.
    pub const fn is_multi(self) -> bool {
        self.multi != 0
    }
}

/// Decoded Server-to-Client frame with decompressed payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S2cFrame {
    /// Declared wire body length (excluding the 8-byte S2C header).
    pub wire_body_len: u32,
    /// Parsed 4-byte S2C header flags (`[flag0, zipped, multi, flag3]`).
    pub flags: S2cFlags,
    /// Decompressed S2C body bytes (if `flags.is_zipped()`, already inflated via RFC 1950 Zlib).
    pub payload: Vec<u8>,
}

/// Server TX 8-byte frame encoder supporting single, `zipped=1`, `multi=1`, and `zipped=1 + multi=1`.
#[derive(Debug, Clone)]
pub struct ServerTxEncoder {
    compression_mode: CompressionMode,
    max_body_len: u32,
}

impl Default for ServerTxEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerTxEncoder {
    /// Creates a new S2C frame encoder using [`CompressionMode::FixedHuffmanLz77`].
    pub fn new() -> Self {
        Self {
            compression_mode: CompressionMode::FixedHuffmanLz77,
            max_body_len: DEFAULT_MAX_FRAME_BODY_LEN,
        }
    }

    /// Sets the RFC 1950/1951 compression mode used whenever `zipped == true`.
    pub fn with_compression_mode(mut self, mode: CompressionMode) -> Self {
        self.compression_mode = mode;
        self
    }

    /// Sets the maximum allowed wire `bodyLen` (client `CConnection` closes on `0` or `> 64 MiB`).
    pub fn with_max_body_len(mut self, max_body_len: u32) -> Self {
        self.max_body_len = max_body_len;
        self
    }

    /// Encodes a raw S2C payload with arbitrary [`S2cFlags`], compressing `uncompressed_payload`
    /// with RFC 1950 Zlib when `flags.is_zipped()` is `true`.
    pub fn encode_frame(
        &self,
        uncompressed_payload: &[u8],
        flags: S2cFlags,
    ) -> Result<Vec<u8>, ProtocolError> {
        let compressed_buf: Vec<u8>;
        let wire_body: &[u8] = if flags.is_zipped() {
            compressed_buf = zlib_compress_with_mode(uncompressed_payload, self.compression_mode);
            &compressed_buf
        } else {
            uncompressed_payload
        };

        if wire_body.is_empty() {
            return Err(FrameError::ZeroBodyLength.into());
        }
        let body_len = u32::try_from(wire_body.len()).map_err(|_| FrameError::PayloadOverflow {
            len: wire_body.len(),
        })?;
        if body_len > self.max_body_len {
            return Err(FrameError::BodyTooLarge {
                body_len,
                max_body_len: self.max_body_len,
            }
            .into());
        }

        let mut out = Vec::with_capacity(S2C_HEADER_SIZE + wire_body.len());
        out.extend_from_slice(&body_len.to_be_bytes());
        out.extend_from_slice(&flags.to_bytes());
        out.extend_from_slice(wire_body);
        Ok(out)
    }

    /// Encodes a single (`multi = 0`) AMF3 payload into an 8-byte-headed S2C frame.
    pub fn encode_single(
        &self,
        amf3_payload: &[u8],
        zipped: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.encode_frame(amf3_payload, S2cFlags::new(zipped, false))
    }

    /// Encodes an already-constructed `multi = 1` outer AMF3 batch payload (matching
    /// `Network.lua:onBodyData` `data = amf3.decode(data); for i = 1, #data.ret`) into an
    /// 8-byte-headed S2C frame with `multi = 1` and optional `zipped = 1`.
    pub fn encode_multi_amf3_payload(
        &self,
        outer_amf3_payload: &[u8],
        zipped: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.encode_frame(outer_amf3_payload, S2cFlags::new(zipped, true))
    }

    /// Encodes concatenated `[4B BE subLen][AMF3 subPayload]` sub-packets into an 8-byte-headed
    /// S2C frame with `multi = 1` and optional `zipped = 1` (provided for raw sub-packet framing
    /// pipelines; note that the stock `vnsg2` `Network.lua` client uses [`crate::rpc::encode_s2c_multi_frame`]).
    pub fn encode_multi_length_prefixed(
        &self,
        sub_payloads: &[&[u8]],
        zipped: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        if sub_payloads.is_empty() {
            return Err(FrameError::ZeroBodyLength.into());
        }
        let total_inner: usize = sub_payloads.iter().map(|s| 4 + s.len()).sum();
        let mut concat = Vec::with_capacity(total_inner);
        for sub in sub_payloads {
            if sub.is_empty() {
                return Err(FrameError::ZeroBodyLength.into());
            }
            let sub_len = u32::try_from(sub.len())
                .map_err(|_| FrameError::PayloadOverflow { len: sub.len() })?;
            concat.extend_from_slice(&sub_len.to_be_bytes());
            concat.extend_from_slice(sub);
        }
        self.encode_frame(&concat, S2cFlags::new(zipped, true))
    }
}

/// Splits a concatenated `[4B BE subLen][subPayload]...` buffer into individual sub-packet slices.
pub fn split_length_prefixed_subpackets(body: &[u8]) -> Result<Vec<&[u8]>, ProtocolError> {
    if body.is_empty() {
        return Err(FrameError::ZeroBodyLength.into());
    }
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let rem = body.len() - pos;
        if rem < 4 {
            return Err(FrameError::InvalidMultiSubPacket {
                offset: pos,
                declared_len: 4,
                remaining: rem,
            }
            .into());
        }
        let sub_len =
            u32::from_be_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as usize;
        pos += 4;
        if sub_len == 0 {
            return Err(FrameError::ZeroBodyLength.into());
        }
        let rem_after = body.len() - pos;
        if sub_len > rem_after {
            return Err(FrameError::InvalidMultiSubPacket {
                offset: pos - 4,
                declared_len: sub_len,
                remaining: rem_after,
            }
            .into());
        }
        out.push(&body[pos..pos + sub_len]);
        pos += sub_len;
    }
    Ok(out)
}

/// Streaming S2C frame decoder matching `CConnection::doRead` (`headerSize_ == 8`) +
/// `GameUtil::unzipData` when `head[5] (zipped) != 0`.
#[derive(Debug, Clone)]
pub struct ServerRxDecoder {
    buf: Vec<u8>,
    read_offset: usize,
    max_body_len: u32,
    max_decompressed_len: usize,
}

impl Default for ServerRxDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerRxDecoder {
    /// Creates a new streaming S2C frame decoder.
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            read_offset: 0,
            max_body_len: DEFAULT_MAX_FRAME_BODY_LEN,
            max_decompressed_len: DEFAULT_MAX_DECOMPRESSED_SIZE,
        }
    }

    /// Configures maximum wire `bodyLen` and maximum inflated Zlib output size.
    pub fn with_limits(mut self, max_body_len: u32, max_decompressed_len: usize) -> Self {
        self.max_body_len = max_body_len;
        self.max_decompressed_len = max_decompressed_len;
        self
    }

    /// Appends incoming socket bytes into the internal receive buffer.
    pub fn feed(&mut self, chunk: &[u8]) {
        if !chunk.is_empty() {
            self.buf.extend_from_slice(chunk);
        }
    }

    /// Number of unconsumed bytes currently buffered.
    pub fn buffered_len(&self) -> usize {
        self.buf.len().saturating_sub(self.read_offset)
    }

    /// Attempts to decode the next complete [`S2cFrame`], returning `Ok(None)` if partial.
    pub fn next_frame(&mut self) -> Result<Option<S2cFrame>, ProtocolError> {
        let avail = &self.buf[self.read_offset..];
        if avail.len() < S2C_HEADER_SIZE {
            return Ok(None);
        }

        let body_len = u32::from_be_bytes([avail[0], avail[1], avail[2], avail[3]]);
        if body_len == 0 {
            return Err(FrameError::ZeroBodyLength.into());
        }
        if body_len > self.max_body_len {
            return Err(FrameError::BodyTooLarge {
                body_len,
                max_body_len: self.max_body_len,
            }
            .into());
        }

        let total_needed = S2C_HEADER_SIZE + (body_len as usize);
        if avail.len() < total_needed {
            return Ok(None);
        }

        let flags = S2cFlags::from_bytes([avail[4], avail[5], avail[6], avail[7]]);
        let raw_body = &avail[S2C_HEADER_SIZE..total_needed];
        let payload = if flags.is_zipped() {
            zlib_decompress_with_limit(raw_body, self.max_decompressed_len)?
        } else {
            raw_body.to_vec()
        };

        self.consume(total_needed);
        Ok(Some(S2cFrame {
            wire_body_len: body_len,
            flags,
            payload,
        }))
    }

    /// Drains all complete S2C frames currently buffered.
    pub fn decode_all(&mut self) -> Result<Vec<S2cFrame>, ProtocolError> {
        let mut frames = Vec::new();
        while let Some(frame) = self.next_frame()? {
            frames.push(frame);
        }
        Ok(frames)
    }

    fn consume(&mut self, count: usize) {
        self.read_offset += count;
        if self.read_offset >= self.buf.len() {
            self.buf.clear();
            self.read_offset = 0;
        } else if self.read_offset >= 4096 || self.read_offset * 2 >= self.buf.len() {
            self.buf.drain(..self.read_offset);
            self.read_offset = 0;
        }
    }
}
