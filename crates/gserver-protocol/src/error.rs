//! Structured error types for AMF3 serialization, RFC 1950/1951 Zlib compression,
//! TCP packet framing, RPC envelopes, and asset decryption.

use std::fmt;

/// Unified protocol error covering all layers of `gserver-protocol`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// AMF3 encoding or decoding failure.
    Amf3(Amf3Error),
    /// RFC 1950 Zlib or RFC 1951 Deflate failure.
    Zlib(ZlibError),
    /// C2S / S2C TCP packet framing or integrity failure.
    Frame(FrameError),
    /// High-level RPC or server push envelope schema mismatch.
    Envelope(EnvelopeError),
    /// `0xFEFEFEFE` `bt_decrypt` asset decryption error.
    BtDecrypt(BtDecryptError),
}

/// AMF3 serialization and deserialization errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Amf3Error {
    /// Input buffer ended before the current AMF3 value finished decoding.
    UnexpectedEof { offset: usize, needed: usize, remaining: usize },
    /// Encountered an unknown or unsupported AMF3 type marker byte.
    InvalidMarker { marker: u8, offset: usize },
    /// String reference index out of bounds in `stringRefs`.
    InvalidStringRef { index: usize, table_len: usize, offset: usize },
    /// Object reference index out of bounds in `objectRefs`.
    InvalidObjectRef { index: usize, table_len: usize, offset: usize },
    /// Trait reference index out of bounds in `traitRefs`.
    InvalidTraitRef { index: usize, table_len: usize, offset: usize },
    /// Encountered a cyclic self-reference to an in-progress container during owned `Value` decoding.
    CyclicObjectRef { index: usize, offset: usize },
    /// Nesting depth exceeded configured maximum (default: 64, matching `Amf3Module.cpp`).
    RecursionLimitExceeded { max_depth: usize, offset: usize },
    /// Declared string, ByteArray, array, or object member count exceeded safety allocation bounds.
    AllocationLimitExceeded { kind: &'static str, requested: usize, limit: usize, offset: usize },
    /// String, ByteArray, or collection length exceeds the 28-bit unsigned U29 inline max (`0x0FFF_FFFF`).
    LengthOverflow { kind: &'static str, len: usize },
    /// UTF-8 validation failed when strict UTF-8 mode was requested.
    InvalidUtf8 { offset: usize },
}

/// RFC 1950 Zlib and RFC 1951 Deflate decompression/compression errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZlibError {
    /// Input ended prematurely while reading header, Deflate bits, or Adler-32 trailer.
    UnexpectedEof,
    /// Invalid 2-byte RFC 1950 Zlib header (`CM != 8`, `CINFO > 7`, or `CMF*256 + FLG` not divisible by 31).
    InvalidZlibHeader { cmf: u8, flg: u8 },
    /// Zlib stream requests a preset dictionary (`FDICT == 1`), which GServer does not use.
    PresetDictionaryUnsupported,
    /// Reserved Deflate block type `BTYPE == 0b11`.
    InvalidBlockType,
    /// Stored block (`BTYPE == 0b00`) `LEN` and `NLEN` are not one's complements (`LEN != !NLEN`).
    StoredLengthMismatch { len: u16, nlen: u16 },
    /// Dynamic Huffman header parameters out of RFC 1951 bounds (`HLIT > 286` or `HDIST > 30`).
    InvalidDynamicHeader { hlit: usize, hdist: usize, hclen: usize },
    /// Invalid or oversubscribed/incomplete Huffman code-length table.
    InvalidHuffmanTable(&'static str),
    /// Encountered an unmapped or illegal Huffman code (e.g. lit/len 286..287 or dist 30..31).
    InvalidHuffmanCode,
    /// Invalid run-length instruction in dynamic code-length table (e.g. symbol 16 at index 0 or overflow).
    InvalidCodeLengthRepeat,
    /// LZ77 back-reference distance exceeds already-produced decompressed output bytes.
    InvalidBackReference { distance: usize, output_len: usize },
    /// Decompressed output would exceed configured `max_output_size` bound.
    OutputLimitExceeded { max_output_size: usize },
    /// Computed Adler-32 checksum of decompressed data did not match the 4-byte RFC 1950 trailer.
    Adler32Mismatch { expected: u32, actual: u32 },
}

/// TCP packet framing and BabelTime `LCPacket` integrity errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Frame `bodyLen` is zero (rejected by `CConnection::getBodyLength`).
    ZeroBodyLength,
    /// Frame `bodyLen` exceeds configured `max_body_length` bound.
    BodyTooLarge { body_len: u32, max_body_len: u32 },
    /// Payload size exceeds `u32::MAX` or configured `max_body_length`.
    PayloadOverflow { len: usize },
    /// 24-byte `LCPacket` protocol flags did not match `{0|1, 1, 1, 1}`.
    InvalidLcFlags([u8; 4]),
    /// 16-byte BabelTime MD5 digest in `LCPacket` header did not match payload.
    Md5Mismatch { expected: [u8; 16], actual: [u8; 16] },
    /// Could not auto-detect C2S framing because initial bytes matched neither `LCPacket` nor AMF3.
    UnrecognizedC2sHeader([u8; 8]),
    ///Truncated or malformed sub-packet inside length-prefixed multi-packet body.
    InvalidMultiSubPacket { offset: usize, declared_len: usize, remaining: usize },
}

/// High-level GServer RPC request / response envelope errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    /// Expected top-level AMF3 Object or associative Array map.
    ExpectedMap,
    /// Missing required field on RPC or S2C envelope.
    MissingField(&'static str),
    /// Envelope field had an unexpected AMF3 value type.
    InvalidFieldType { field: &'static str, expected: &'static str },
    /// `multi == 1` S2C envelope contained zero sub-responses (`#rets == 0` is fatal in `Network.lua`).
    EmptyMultiBatch,
}

/// `0xFEFEFEFE` `bt_decrypt` asset errors (ported from `BtDecryptHook.h`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtDecryptError {
    /// Buffer does not start with `0xFEFEFEFE` or is shorter than 32 bytes when encrypted header expected.
    TruncatedHeader { len: usize },
    /// Header `[4..8]` LE32 `payloadSize` does not equal `totalSize - 32`.
    PayloadLengthMismatch { header_payload_size: u32, actual_payload_size: usize },
    /// Unsupported encryption algorithm index in `header[8]` (expected `0` or `1`).
    UnsupportedAlgoIndex(u8),
    /// Key table slot in `header[9]` out of range (`0..=15`).
    InvalidKeyIndex(u8),
}

impl fmt::Display for Amf3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { offset, needed, remaining } => write!(
                f,
                "AMF3 unexpected EOF at offset {offset}: needed {needed} bytes, {remaining} remaining"
            ),
            Self::InvalidMarker { marker, offset } => {
                write!(f, "AMF3 unsupported type marker 0x{marker:02X} at offset {offset}")
            }
            Self::InvalidStringRef { index, table_len, offset } => write!(
                f,
                "AMF3 invalid string ref {index} (table len {table_len}) at offset {offset}"
            ),
            Self::InvalidObjectRef { index, table_len, offset } => write!(
                f,
                "AMF3 invalid object ref {index} (table len {table_len}) at offset {offset}"
            ),
            Self::InvalidTraitRef { index, table_len, offset } => write!(
                f,
                "AMF3 invalid trait ref {index} (table len {table_len}) at offset {offset}"
            ),
            Self::CyclicObjectRef { index, offset } => write!(
                f,
                "AMF3 cyclic self-reference to in-progress object ref {index} at offset {offset}"
            ),
            Self::RecursionLimitExceeded { max_depth, offset } => write!(
                f,
                "AMF3 nesting depth exceeded limit ({max_depth}) at offset {offset}"
            ),
            Self::AllocationLimitExceeded { kind, requested, limit, offset } => write!(
                f,
                "AMF3 {kind} size {requested} exceeds limit {limit} at offset {offset}"
            ),
            Self::LengthOverflow { kind, len } => {
                write!(f, "AMF3 {kind} length {len} exceeds 28-bit U29 maximum")
            }
            Self::InvalidUtf8 { offset } => {
                write!(f, "AMF3 string is not valid UTF-8 at offset {offset}")
            }
        }
    }
}

impl fmt::Display for ZlibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "zlib unexpected end of stream"),
            Self::InvalidZlibHeader { cmf, flg } => {
                write!(f, "invalid RFC 1950 zlib header: CMF=0x{cmf:02X}, FLG=0x{flg:02X}")
            }
            Self::PresetDictionaryUnsupported => {
                write!(f, "RFC 1950 FDICT preset dictionary is not supported")
            }
            Self::InvalidBlockType => write!(f, "invalid RFC 1951 deflate block type 0b11"),
            Self::StoredLengthMismatch { len, nlen } => write!(
                f,
                "deflate stored block LEN/NLEN mismatch: LEN=0x{len:04X}, NLEN=0x{nlen:04X}"
            ),
            Self::InvalidDynamicHeader { hlit, hdist, hclen } => write!(
                f,
                "invalid deflate dynamic Huffman counts: HLIT={hlit}, HDIST={hdist}, HCLEN={hclen}"
            ),
            Self::InvalidHuffmanTable(ctx) => write!(f, "invalid Huffman table ({ctx})"),
            Self::InvalidHuffmanCode => write!(f, "invalid or unmapped Huffman code in bitstream"),
            Self::InvalidCodeLengthRepeat => {
                write!(f, "invalid repeat sequence in dynamic Huffman code-length table")
            }
            Self::InvalidBackReference { distance, output_len } => write!(
                f,
                "LZ77 distance {distance} exceeds current output length {output_len}"
            ),
            Self::OutputLimitExceeded { max_output_size } => write!(
                f,
                "decompressed output exceeded maximum limit of {max_output_size} bytes"
            ),
            Self::Adler32Mismatch { expected, actual } => write!(
                f,
                "zlib Adler-32 checksum mismatch: expected 0x{expected:08X}, computed 0x{actual:08X}"
            ),
        }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBodyLength => write!(f, "TCP frame declared invalid bodyLen == 0"),
            Self::BodyTooLarge { body_len, max_body_len } => {
                write!(f, "TCP frame bodyLen {body_len} exceeds limit {max_body_len}")
            }
            Self::PayloadOverflow { len } => {
                write!(f, "TCP frame payload length {len} exceeds maximum allowed frame size")
            }
            Self::InvalidLcFlags(flags) => {
                write!(f, "invalid LCPacket flags {:?} (expected [0|1, 1, 1, 1])", flags)
            }
            Self::Md5Mismatch { .. } => write!(f, "BabelTime LCPacket 16-byte MD5 integrity mismatch"),
            Self::UnrecognizedC2sHeader(hdr) => {
                write!(f, "unrecognized C2S frame header prefix {:02X?}", hdr)
            }
            Self::InvalidMultiSubPacket { offset, declared_len, remaining } => write!(
                f,
                "invalid multi sub-packet at offset {offset}: len {declared_len} > remaining {remaining}"
            ),
        }
    }
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedMap => write!(f, "expected AMF3 Object or associative Array envelope"),
            Self::MissingField(field) => write!(f, "missing required envelope field '{field}'"),
            Self::InvalidFieldType { field, expected } => {
                write!(f, "envelope field '{field}' expected {expected}")
            }
            Self::EmptyMultiBatch => write!(f, "multi S2C envelope must contain at least 1 item"),
        }
    }
}

impl fmt::Display for BtDecryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader { len } => {
                write!(f, "truncated 0xFEFEFEFE bt_decrypt header ({len} < 32 bytes)")
            }
            Self::PayloadLengthMismatch { header_payload_size, actual_payload_size } => write!(
                f,
                "bt_decrypt payload mismatch: header={header_payload_size}, actual={actual_payload_size}"
            ),
            Self::UnsupportedAlgoIndex(algo) => {
                write!(f, "unsupported bt_decrypt algoIndex {algo} (expected 0 or 1)")
            }
            Self::InvalidKeyIndex(idx) => {
                write!(f, "bt_decrypt keyIndex {idx} out of range (expected 0..=15)")
            }
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Amf3(e) => write!(f, "{e}"),
            Self::Zlib(e) => write!(f, "{e}"),
            Self::Frame(e) => write!(f, "{e}"),
            Self::Envelope(e) => write!(f, "{e}"),
            Self::BtDecrypt(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Amf3Error {}
impl std::error::Error for ZlibError {}
impl std::error::Error for FrameError {}
impl std::error::Error for EnvelopeError {}
impl std::error::Error for BtDecryptError {}
impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Amf3(e) => Some(e),
            Self::Zlib(e) => Some(e),
            Self::Frame(e) => Some(e),
            Self::Envelope(e) => Some(e),
            Self::BtDecrypt(e) => Some(e),
        }
    }
}

impl From<Amf3Error> for ProtocolError {
    fn from(v: Amf3Error) -> Self {
        Self::Amf3(v)
    }
}

impl From<ZlibError> for ProtocolError {
    fn from(v: ZlibError) -> Self {
        Self::Zlib(v)
    }
}

impl From<FrameError> for ProtocolError {
    fn from(v: FrameError) -> Self {
        Self::Frame(v)
    }
}

impl From<EnvelopeError> for ProtocolError {
    fn from(v: EnvelopeError) -> Self {
        Self::Envelope(v)
    }
}

impl From<BtDecryptError> for ProtocolError {
    fn from(v: BtDecryptError) -> Self {
        Self::BtDecrypt(v)
    }
}
