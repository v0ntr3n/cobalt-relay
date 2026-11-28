//! Client-to-Server (C2S) and Server-to-Client (S2C) TCP packet framing, generic
//! [`FramedStreamDecoder`], Zlib payload compression, and BabelTime `LCPacket` verification.

use std::borrow::Cow;
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

#[inline]
fn checked_body_len(len: usize) -> Result<u32, FrameError> {
    if len == 0 {
        return Err(FrameError::ZeroBodyLength);
    }
    u32::try_from(len).map_err(|_| FrameError::PayloadOverflow { len })
}

#[inline]
fn maybe_compress(payload: &[u8], compress: bool, mode: CompressionMode) -> Cow<'_, [u8]> {
    if compress {
        Cow::Owned(zlib_compress_with_mode(payload, mode))
    } else {
        Cow::Borrowed(payload)
    }
}

#[inline]
fn maybe_decompress(
    body: &[u8],
    compressed: bool,
    max_decompressed_len: usize,
) -> Result<Vec<u8>, ProtocolError> {
    if compressed {
        Ok(zlib_decompress_with_limit(body, max_decompressed_len)?)
    } else {
        Ok(body.to_vec())
    }
}

// ============================================================================
// Generic Streaming Frame Decoder (`WireFrame` + `FramedStreamDecoder<F>`)
// ============================================================================

/// Trait implemented by wire frame types ([`C2sFrame`], [`S2cFrame`]) decoded by [`FramedStreamDecoder`].
pub trait WireFrame: Sized {
    /// Per-stream framing configuration (`C2sFrameMode` for C2S, `()` for S2C).
    type Config: Clone + fmt::Debug + Default;

    /// Minimum buffered bytes required before reading and validating the 4-byte BE `body_len`.
    const MIN_HEADER_LEN: usize;

    /// Attempts to decode one complete frame from `avail`, where `body_len` (`avail[0..4]`) has
    /// already been verified to satisfy `1 <= body_len <= max_body_len`.
    fn decode_wire_frame(
        avail: &[u8],
        body_len: u32,
        config: &Self::Config,
        max_decompressed_len: usize,
    ) -> Result<Option<(Self, usize)>, ProtocolError>;
}

/// Generic streaming TCP frame decoder eliminating duplicate buffer management, `body_len`
/// bounds checks, pipelined draining, and compaction across C2S and S2C streams.
#[derive(Debug, Clone)]
pub struct FramedStreamDecoder<F: WireFrame> {
    buf: Vec<u8>,
    read_offset: usize,
    config: F::Config,
    max_body_len: u32,
    max_decompressed_len: usize,
}

impl<F: WireFrame> Default for FramedStreamDecoder<F> {
    fn default() -> Self {
        Self::with_config(F::Config::default())
    }
}

impl<F: WireFrame> FramedStreamDecoder<F> {
    /// Creates a new streaming frame decoder with explicit framing config and default bounds.
    pub fn with_config(config: F::Config) -> Self {
        Self {
            buf: Vec::new(),
            read_offset: 0,
            config,
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

    /// Attempts to decode the next complete frame from the internal buffer.
    pub fn next_frame(&mut self) -> Result<Option<F>, ProtocolError> {
        let avail = &self.buf[self.read_offset..];
        if avail.len() < F::MIN_HEADER_LEN {
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

        match F::decode_wire_frame(avail, body_len, &self.config, self.max_decompressed_len)? {
            Some((frame, consumed)) => {
                self.consume(consumed);
                Ok(Some(frame))
            }
            None => Ok(None),
        }
    }

    /// Drains all complete frames currently available in the buffer.
    pub fn decode_all(&mut self) -> Result<Vec<F>, ProtocolError> {
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
// Client-to-Server (C2S) Framing & `ClientRxDecoder`
// ============================================================================

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
    pub wire_body_len: u32,
    pub kind: C2sWireKind,
    pub amf3_payload: Vec<u8>,
}

impl WireFrame for C2sFrame {
    type Config = C2sFrameMode;
    const MIN_HEADER_LEN: usize = C2S_RAW_HEADER_SIZE;

    fn decode_wire_frame(
        avail: &[u8],
        body_len: u32,
        mode: &C2sFrameMode,
        max_decompressed_len: usize,
    ) -> Result<Option<(Self, usize)>, ProtocolError> {
        let body_len_usize = body_len as usize;
        let resolved_lc = match mode {
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
                    if body_len_usize == 1 && (avail.len() < 8 || avail[5..8] != [1, 1, 1]) {
                        false
                    } else if avail.len() < 8 {
                        return Ok(None);
                    } else if avail[5..8] == [1, 1, 1] {
                        true
                    } else {
                        let mut hdr = [0u8; 8];
                        hdr.copy_from_slice(&avail[..8]);
                        return Err(FrameError::UnrecognizedC2sHeader(hdr).into());
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
            if flags[0] > 1 || flags[1..4] != [1, 1, 1] {
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
            let amf3_payload = maybe_decompress(body, compressed, max_decompressed_len)?;
            Ok(Some((
                Self {
                    wire_body_len: body_len,
                    kind: C2sWireKind::LcPacket24 {
                        compressed,
                        md5: expected_md5,
                    },
                    amf3_payload,
                },
                total_needed,
            )))
        } else {
            let total_needed = C2S_RAW_HEADER_SIZE + body_len_usize;
            if avail.len() < total_needed {
                return Ok(None);
            }
            Ok(Some((
                Self {
                    wire_body_len: body_len,
                    kind: C2sWireKind::Raw4Byte,
                    amf3_payload: avail[4..total_needed].to_vec(),
                },
                total_needed,
            )))
        }
    }
}

/// Streaming C2S TCP frame decoder handling arbitrary partial reads and pipelined frames.
pub type ClientRxDecoder = FramedStreamDecoder<C2sFrame>;

impl FramedStreamDecoder<C2sFrame> {
    /// Creates a new streaming C2S RX decoder with default safety bounds (`16 MiB` wire, `64 MiB` inflated).
    pub fn new(mode: C2sFrameMode) -> Self {
        Self::with_config(mode)
    }
}

/// Encodes an AMF3 C2S payload into a 24-byte BabelTime `LCPacket` matching `LCPacket::makeLCPacket`.
pub fn encode_c2s_lcpacket(
    amf3_payload: &[u8],
    compress: bool,
) -> Result<Vec<u8>, ProtocolError> {
    encode_c2s_lcpacket_with_mode(amf3_payload, compress, CompressionMode::FixedHuffmanLz77)
}

/// Encodes an AMF3 C2S payload into a 24-byte BabelTime `LCPacket` with explicit [`CompressionMode`].
pub fn encode_c2s_lcpacket_with_mode(
    amf3_payload: &[u8],
    compress: bool,
    compression_mode: CompressionMode,
) -> Result<Vec<u8>, ProtocolError> {
    let body = maybe_compress(amf3_payload, compress, compression_mode);
    let body_len = checked_body_len(body.len())?;
    let md5 = compute_babeltime_md5(&body);

    let mut frame = Vec::with_capacity(C2S_LCPACKET_HEADER_SIZE + body.len());
    frame.extend_from_slice(&body_len.to_be_bytes());
    frame.extend_from_slice(&[u8::from(compress), 1, 1, 1]);
    frame.extend_from_slice(&md5);
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Encodes an AMF3 C2S payload into a raw 4-byte `[4B BE bodyLen][AMF3 payload]` frame.
pub fn encode_c2s_raw_frame(amf3_payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let body_len = checked_body_len(amf3_payload.len())?;
    let mut frame = Vec::with_capacity(C2S_RAW_HEADER_SIZE + amf3_payload.len());
    frame.extend_from_slice(&body_len.to_be_bytes());
    frame.extend_from_slice(amf3_payload);
    Ok(frame)
}

// ============================================================================
// Server-to-Client (S2C) 8-Byte Frame Flags, TX Encoder, and `ServerRxDecoder`
// ============================================================================

/// 4-byte flag tuple at offsets `[4..8]` of the 8-byte S2C frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct S2cFlags {
    pub flag0: u8,
    pub zipped: u8,
    pub multi: u8,
    pub flag3: u8,
}

impl S2cFlags {
    pub const fn new(zipped: bool, multi: bool) -> Self {
        Self {
            flag0: 0,
            zipped: if zipped { 1 } else { 0 },
            multi: if multi { 1 } else { 0 },
            flag3: 0,
        }
    }

    pub const fn to_bytes(self) -> [u8; 4] {
        [self.flag0, self.zipped, self.multi, self.flag3]
    }

    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self {
            flag0: bytes[0],
            zipped: bytes[1],
            multi: bytes[2],
            flag3: bytes[3],
        }
    }

    pub const fn is_zipped(self) -> bool {
        self.zipped != 0
    }

    pub const fn is_multi(self) -> bool {
        self.multi != 0
    }
}

/// Decoded Server-to-Client frame with decompressed payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S2cFrame {
    pub wire_body_len: u32,
    pub flags: S2cFlags,
    pub payload: Vec<u8>,
}

impl WireFrame for S2cFrame {
    type Config = ();
    const MIN_HEADER_LEN: usize = S2C_HEADER_SIZE;

    fn decode_wire_frame(
        avail: &[u8],
        body_len: u32,
        _config: &(),
        max_decompressed_len: usize,
    ) -> Result<Option<(Self, usize)>, ProtocolError> {
        let total_needed = S2C_HEADER_SIZE + (body_len as usize);
        if avail.len() < total_needed {
            return Ok(None);
        }

        let flags = S2cFlags::from_bytes([avail[4], avail[5], avail[6], avail[7]]);
        let payload = maybe_decompress(
            &avail[S2C_HEADER_SIZE..total_needed],
            flags.is_zipped(),
            max_decompressed_len,
        )?;

        Ok(Some((
            Self {
                wire_body_len: body_len,
                flags,
                payload,
            },
            total_needed,
        )))
    }
}

/// Streaming S2C frame decoder matching `CConnection::doRead` (`headerSize_ == 8`).
pub type ServerRxDecoder = FramedStreamDecoder<S2cFrame>;

impl FramedStreamDecoder<S2cFrame> {
    /// Creates a new streaming S2C frame decoder.
    pub fn new() -> Self {
        Self::with_config(())
    }
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
    pub fn new() -> Self {
        Self {
            compression_mode: CompressionMode::FixedHuffmanLz77,
            max_body_len: DEFAULT_MAX_FRAME_BODY_LEN,
        }
    }

    pub fn with_compression_mode(mut self, mode: CompressionMode) -> Self {
        self.compression_mode = mode;
        self
    }

    pub fn with_max_body_len(mut self, max_body_len: u32) -> Self {
        self.max_body_len = max_body_len;
        self
    }

    pub fn encode_frame(
        &self,
        uncompressed_payload: &[u8],
        flags: S2cFlags,
    ) -> Result<Vec<u8>, ProtocolError> {
        let wire_body =
            maybe_compress(uncompressed_payload, flags.is_zipped(), self.compression_mode);
        let body_len = checked_body_len(wire_body.len())?;
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
        out.extend_from_slice(&wire_body);
        Ok(out)
    }

    pub fn encode_single(
        &self,
        amf3_payload: &[u8],
        zipped: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.encode_frame(amf3_payload, S2cFlags::new(zipped, false))
    }

    pub fn encode_multi_amf3_payload(
        &self,
        outer_amf3_payload: &[u8],
        zipped: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        self.encode_frame(outer_amf3_payload, S2cFlags::new(zipped, true))
    }

    pub fn encode_multi_length_prefixed(
        &self,
        sub_payloads: &[&[u8]],
        zipped: bool,
    ) -> Result<Vec<u8>, ProtocolError> {
        if sub_payloads.is_empty() {
            return Err(FrameError::ZeroBodyLength.into());
        }
        let mut concat = Vec::with_capacity(sub_payloads.iter().map(|s| 4 + s.len()).sum());
        for sub in sub_payloads {
            let sub_len = checked_body_len(sub.len())?;
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
