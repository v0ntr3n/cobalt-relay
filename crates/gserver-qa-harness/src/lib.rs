#![forbid(unsafe_code)]
#![allow(
    clippy::should_implement_trait,
    clippy::chunks_exact_to_as_chunks,
    clippy::manual_is_multiple_of
)]

//! `gserver-qa-harness` — Pure-Rust adversarial QA, deterministic PRNG fuzzing,
//! 32-thread concurrency stress simulator, and local private-server gateway for the GServer (`vnsg2`)
//! Win32 C++ client (`Amf3Module.cpp`, `LCPacket.cpp`, `CConnection.cpp`, `BtPlatformWin32.cpp`,
//! `AccountAuthProvider.cpp`, `AppDelegate.cpp`, and `Network.lua`).
//!
//! Core codecs and cryptographic primitives delegate directly to `gserver-protocol` and `gserver-http`,
//! keeping only adversarial QA fault-injection helpers and behavioral simulation glue here.

pub mod framing;
pub mod fuzzer;
pub mod http_gateway;
pub mod server;

/// AMF3 compatibility shim backed by [`gserver_protocol::amf3`] plus QA-only fault/hazard helpers.
pub mod amf3 {
    use std::fmt;

    use gserver_protocol::amf3::{
        self as proto_amf3, AMF3_MAX_INLINE_LEN, Amf3Decoder as ProtoDecoder,
        Amf3Encoder as ProtoEncoder, Amf3Object, DecodeConfig, Value as ProtoValue,
    };
    use gserver_protocol::Amf3Error as ProtoAmf3Error;

    pub use gserver_protocol::amf3::{
        AMF3_INT_MAX, AMF3_INT_MIN, MARKER_ARRAY, MARKER_BYTE_ARRAY, MARKER_DATE, MARKER_DOUBLE,
        MARKER_FALSE, MARKER_INTEGER, MARKER_NULL, MARKER_OBJECT, MARKER_STRING, MARKER_TRUE,
        MARKER_UNDEFINED, MARKER_XML, MARKER_XML_DOC,
    };

    /// Maximum exact integer representable in IEEE-754 `f64` (`2^53`).
    pub const MAX_SAFE_F64_INT: u64 = 9_007_199_254_740_992;
    /// Maximum nesting depth matching `Amf3Module.cpp:37` (`kMaxRecursionDepth = 64`).
    pub const MAX_RECURSION_DEPTH: usize = proto_amf3::DEFAULT_MAX_DEPTH;

    /// Decoder config letting U29 inline bounds pass to stream-EOF checks (while `gserver-protocol`
    /// still caps `Vec::with_capacity` to remaining input bytes so zero OOM occurs).
    pub(crate) const QA_DECODE_CONFIG: DecodeConfig = DecodeConfig {
        max_depth: MAX_RECURSION_DEPTH,
        max_bytes_len: AMF3_MAX_INLINE_LEN,
        max_collection_len: AMF3_MAX_INLINE_LEN,
        strict_utf8: false,
    };

    /// Binary-safe AMF3 value tree (Lua 5.1 strings are arbitrary 8-bit byte slices).
    #[derive(Debug, Clone, PartialEq)]
    pub enum Amf3Value {
        Undefined,
        Null,
        Bool(bool),
        Integer(i32),
        Double(f64),
        String(Vec<u8>),
        XmlDoc(Vec<u8>),
        Date(f64),
        Array {
            assoc: Vec<(Vec<u8>, Amf3Value)>,
            dense: Vec<Amf3Value>,
        },
        Object {
            class_name: Vec<u8>,
            dynamic: bool,
            sealed: Vec<(Vec<u8>, Amf3Value)>,
            dynamic_members: Vec<(Vec<u8>, Amf3Value)>,
            externalizable: Option<Box<Amf3Value>>,
        },
        Xml(Vec<u8>),
        ByteArray(Vec<u8>),
    }

    impl Amf3Value {
        pub fn from_i64_auto(v: i64) -> Self {
            Self::from_protocol_value(ProtoValue::from(v))
        }

        pub fn from_lua_number(d: f64) -> Self {
            Self::from_protocol_value(ProtoValue::from_lua_number(d))
        }

        pub fn from_i64_precision_safe(v: i64) -> Self {
            if v.unsigned_abs() > MAX_SAFE_F64_INT {
                Self::from_str(&v.to_string())
            } else {
                Self::from_i64_auto(v)
            }
        }

        pub fn from_str(s: &str) -> Self {
            Self::String(s.as_bytes().to_vec())
        }

        pub fn empty_lua_table() -> Self {
            Self::Array {
                assoc: Vec::new(),
                dense: Vec::new(),
            }
        }

        pub fn dense_array(items: Vec<Amf3Value>) -> Self {
            Self::Array {
                assoc: Vec::new(),
                dense: items,
            }
        }

        pub fn anonymous_object(entries: Vec<(&str, Amf3Value)>) -> Self {
            Self::Object {
                class_name: Vec::new(),
                dynamic: true,
                sealed: Vec::new(),
                dynamic_members: entries
                    .into_iter()
                    .map(|(k, v)| (k.as_bytes().to_vec(), v))
                    .collect(),
                externalizable: None,
            }
        }

        pub fn as_str(&self) -> Option<&str> {
            self.as_lua_bytes().and_then(|b| std::str::from_utf8(b).ok())
        }

        pub fn as_lua_bytes(&self) -> Option<&[u8]> {
            match self {
                Self::String(b) | Self::ByteArray(b) | Self::XmlDoc(b) | Self::Xml(b) => Some(b),
                _ => None,
            }
        }

        pub fn as_f64(&self) -> Option<f64> {
            match self {
                Self::Integer(i) => Some(f64::from(*i)),
                Self::Double(d) | Self::Date(d) => Some(*d),
                _ => None,
            }
        }

        pub fn as_i64(&self) -> Option<i64> {
            match self {
                Self::Integer(i) => Some(i64::from(*i)),
                Self::Double(d) if d.is_finite() && d.fract() == 0.0 => Some(*d as i64),
                _ => None,
            }
        }

        pub fn get_field(&self, key: &str) -> Option<&Amf3Value> {
            let target = key.as_bytes();
            match self {
                Self::Object {
                    sealed,
                    dynamic_members,
                    ..
                } => dynamic_members
                    .iter()
                    .rev()
                    .chain(sealed.iter().rev())
                    .find(|(k, _)| k.as_slice() == target)
                    .map(|(_, v)| v),
                Self::Array { assoc, .. } => assoc
                    .iter()
                    .rev()
                    .find(|(k, _)| k.as_slice() == target)
                    .map(|(_, v)| v),
                _ => None,
            }
        }

        pub(crate) fn to_protocol_value(&self) -> Result<ProtoValue, Amf3Error> {
            self.to_protocol_value_at_depth(0)
        }

        fn to_protocol_value_at_depth(&self, depth: usize) -> Result<ProtoValue, Amf3Error> {
            if depth > MAX_RECURSION_DEPTH {
                return Err(Amf3Error::MaxDepthExceeded { depth });
            }
            let key_str = |b: &[u8]| -> String { String::from_utf8_lossy(b).into_owned() };
            match self {
                Self::Undefined => Ok(ProtoValue::Undefined),
                Self::Null => Ok(ProtoValue::Null),
                Self::Bool(b) => Ok(ProtoValue::Bool(*b)),
                Self::Integer(i) => Ok(ProtoValue::Int(*i)),
                Self::Double(d) => Ok(ProtoValue::Double(*d)),
                Self::String(b) => Ok(ProtoValue::ByteString(b.clone())),
                Self::XmlDoc(b) => Ok(ProtoValue::XmlDoc(b.clone())),
                Self::Date(ms) => Ok(ProtoValue::Date(*ms)),
                Self::Xml(b) => Ok(ProtoValue::Xml(b.clone())),
                Self::ByteArray(b) => Ok(ProtoValue::ByteArray(b.clone())),
                Self::Array { assoc, dense } => {
                    let mut proto_assoc = Vec::with_capacity(assoc.len());
                    for (k, v) in assoc {
                        if k.is_empty() {
                            return Err(Amf3Error::EmptyObjectKey);
                        }
                        proto_assoc.push((key_str(k), v.to_protocol_value_at_depth(depth + 1)?));
                    }
                    let mut proto_dense = Vec::with_capacity(dense.len());
                    for item in dense {
                        proto_dense.push(item.to_protocol_value_at_depth(depth + 1)?);
                    }
                    Ok(ProtoValue::Array {
                        dense: proto_dense,
                        assoc: proto_assoc,
                    })
                }
                Self::Object {
                    class_name,
                    dynamic,
                    sealed,
                    dynamic_members,
                    externalizable,
                } => {
                    if let Some(ext) = externalizable {
                        return Ok(ProtoValue::Object(Amf3Object {
                            class_name: key_str(class_name),
                            dynamic: *dynamic,
                            sealed_members: Vec::new(),
                            dynamic_members: Vec::new(),
                            externalized: Some(Box::new(ext.to_protocol_value_at_depth(depth + 1)?)),
                        }));
                    }
                    let mut proto_sealed = Vec::with_capacity(sealed.len());
                    for (k, v) in sealed {
                        proto_sealed.push((key_str(k), v.to_protocol_value_at_depth(depth + 1)?));
                    }
                    let mut proto_dyn = Vec::with_capacity(dynamic_members.len());
                    if *dynamic {
                        for (k, v) in dynamic_members {
                            if k.is_empty() {
                                return Err(Amf3Error::EmptyObjectKey);
                            }
                            proto_dyn.push((key_str(k), v.to_protocol_value_at_depth(depth + 1)?));
                        }
                    }
                    Ok(ProtoValue::Object(Amf3Object {
                        class_name: key_str(class_name),
                        dynamic: *dynamic,
                        sealed_members: proto_sealed,
                        dynamic_members: proto_dyn,
                        externalized: None,
                    }))
                }
            }
        }

        pub(crate) fn from_protocol_value(val: ProtoValue) -> Self {
            match val {
                ProtoValue::Undefined => Self::Undefined,
                ProtoValue::Null => Self::Null,
                ProtoValue::Bool(b) => Self::Bool(b),
                ProtoValue::Int(i) => Self::Integer(i),
                ProtoValue::Double(d) => Self::Double(d),
                ProtoValue::String(s) => Self::String(s.into_bytes()),
                ProtoValue::ByteString(b) => Self::String(b),
                ProtoValue::XmlDoc(b) => Self::XmlDoc(b),
                ProtoValue::Date(ms) => Self::Date(ms),
                ProtoValue::Xml(b) => Self::Xml(b),
                ProtoValue::ByteArray(b) => Self::ByteArray(b),
                ProtoValue::Array { dense, assoc } => Self::Array {
                    assoc: assoc
                        .into_iter()
                        .map(|(k, v)| (k.into_bytes(), Self::from_protocol_value(v)))
                        .collect(),
                    dense: dense.into_iter().map(Self::from_protocol_value).collect(),
                },
                ProtoValue::Object(obj) => Self::Object {
                    class_name: obj.class_name.into_bytes(),
                    dynamic: obj.dynamic,
                    sealed: obj
                        .sealed_members
                        .into_iter()
                        .map(|(k, v)| (k.into_bytes(), Self::from_protocol_value(v)))
                        .collect(),
                    dynamic_members: obj
                        .dynamic_members
                        .into_iter()
                        .map(|(k, v)| (k.into_bytes(), Self::from_protocol_value(v)))
                        .collect(),
                    externalizable: obj
                        .externalized
                        .map(|b| Box::new(Self::from_protocol_value(*b))),
                },
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Amf3Error {
        UnexpectedEof { offset: usize },
        UnsupportedMarker { marker: u8, offset: usize },
        MaxDepthExceeded { depth: usize },
        InvalidStringRef { index: u32, table_len: usize },
        InvalidObjectRef { index: u32, table_len: usize },
        InvalidTraitRef { index: u32, table_len: usize },
        StringTooLong { len: usize },
        EmptyObjectKey,
    }

    impl fmt::Display for Amf3Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::UnexpectedEof { offset } => {
                    write!(f, "amf3.decode: unexpected end of input at offset {offset}")
                }
                Self::UnsupportedMarker { marker, offset } => write!(
                    f,
                    "amf3.decode: unsupported AMF3 type marker 0x{marker:02X} at offset {offset}"
                ),
                Self::MaxDepthExceeded { depth } => {
                    write!(f, "amf3: maximum nesting depth exceeded ({depth})")
                }
                Self::InvalidStringRef { index, table_len } => write!(
                    f,
                    "amf3.decode: invalid string reference index {index} (len={table_len})"
                ),
                Self::InvalidObjectRef { index, table_len } => write!(
                    f,
                    "amf3.decode: invalid object reference index {index} (len={table_len})"
                ),
                Self::InvalidTraitRef { index, table_len } => write!(
                    f,
                    "amf3.decode: invalid trait reference index {index} (len={table_len})"
                ),
                Self::StringTooLong { len } => {
                    write!(f, "amf3.encode: string/array length {len} exceeds U29 limit")
                }
                Self::EmptyObjectKey => {
                    write!(f, "amf3.encode: dynamic/associative key cannot be empty")
                }
            }
        }
    }

    impl From<ProtoAmf3Error> for Amf3Error {
        fn from(e: ProtoAmf3Error) -> Self {
            match e {
                ProtoAmf3Error::UnexpectedEof { offset, .. } => Self::UnexpectedEof { offset },
                ProtoAmf3Error::InvalidMarker { marker, offset } => {
                    Self::UnsupportedMarker { marker, offset }
                }
                ProtoAmf3Error::RecursionLimitExceeded { max_depth, .. } => {
                    Self::MaxDepthExceeded {
                        depth: max_depth + 1,
                    }
                }
                ProtoAmf3Error::InvalidStringRef {
                    index, table_len, ..
                } => Self::InvalidStringRef {
                    index: index as u32,
                    table_len,
                },
                ProtoAmf3Error::InvalidObjectRef {
                    index, table_len, ..
                } => Self::InvalidObjectRef {
                    index: index as u32,
                    table_len,
                },
                ProtoAmf3Error::CyclicObjectRef { index, .. } => Self::InvalidObjectRef {
                    index: index as u32,
                    table_len: index,
                },
                ProtoAmf3Error::InvalidTraitRef {
                    index, table_len, ..
                } => Self::InvalidTraitRef {
                    index: index as u32,
                    table_len,
                },
                ProtoAmf3Error::AllocationLimitExceeded { offset, .. } => {
                    Self::UnexpectedEof { offset }
                }
                ProtoAmf3Error::LengthOverflow { len, .. } => Self::StringTooLong { len },
                ProtoAmf3Error::InvalidUtf8 { offset } => Self::UnsupportedMarker {
                    marker: MARKER_STRING,
                    offset,
                },
            }
        }
    }

    pub fn write_u29(out: &mut Vec<u8>, v: u32) {
        let mut enc = ProtoEncoder::new();
        enc.write_u29(v);
        out.extend_from_slice(enc.as_bytes());
    }

    pub fn read_u29(data: &[u8], pos: &mut usize) -> Result<u32, Amf3Error> {
        let mut dec = ProtoDecoder::with_config(data, *pos, QA_DECODE_CONFIG);
        let v = dec.read_u29().map_err(Amf3Error::from)?;
        *pos = dec.position();
        Ok(v)
    }

    pub fn sign_extend_u29(raw: u32) -> i32 {
        if (raw & 0x1000_0000) != 0 {
            (raw | 0xE000_0000) as i32
        } else {
            (raw & 0x0FFF_FFFF) as i32
        }
    }

    /// QA fault-injection helper demonstrating buggy U29 29-bit truncation of large timestamps.
    pub fn encode_buggy_truncated_u29_integer(v: i64) -> Vec<u8> {
        let mut out = Vec::with_capacity(5);
        out.push(MARKER_INTEGER);
        write_u29(&mut out, (v as u32) & 0x1FFF_FFFF);
        out
    }

    #[derive(Default)]
    pub struct Amf3Encoder;

    impl Amf3Encoder {
        pub fn new() -> Self {
            Self
        }

        pub fn encode(val: &Amf3Value) -> Result<Vec<u8>, Amf3Error> {
            let proto_val = val.to_protocol_value()?;
            proto_amf3::encode(&proto_val).map_err(Amf3Error::from)
        }
    }

    pub struct Amf3Decoder;

    impl Amf3Decoder {
        pub fn decode_at(data: &[u8], start_pos: usize) -> Result<(Amf3Value, usize), Amf3Error> {
            let (proto_val, end_pos) =
                proto_amf3::decode_from(data, start_pos, QA_DECODE_CONFIG)
                    .map_err(Amf3Error::from)?;
            Ok((Amf3Value::from_protocol_value(proto_val), end_pos))
        }

        pub fn decode(data: &[u8]) -> Result<Amf3Value, Amf3Error> {
            let (val, _) = Self::decode_at(data, 0)?;
            Ok(val)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum LuaClientHazard {
        DenseArrayNilHole { lua_index: usize },
        UnsafeLargeFloatInteger,
        ExternalizableObjectUsed,
    }

    /// Audits an outgoing server-to-client `Amf3Value` tree for Lua 5.1 / `Amf3Module.cpp` traps.
    pub fn audit_lua_client_hazards(val: &Amf3Value) -> Vec<LuaClientHazard> {
        let mut out = Vec::new();
        audit_inner(val, &mut out);
        out
    }

    fn audit_inner(val: &Amf3Value, hazards: &mut Vec<LuaClientHazard>) {
        match val {
            Amf3Value::Double(d) => {
                if d.is_finite() && d.fract() == 0.0 && d.abs() > (MAX_SAFE_F64_INT as f64) {
                    hazards.push(LuaClientHazard::UnsafeLargeFloatInteger);
                }
            }
            Amf3Value::Array { assoc, dense } => {
                for (_, v) in assoc {
                    audit_inner(v, hazards);
                }
                for (idx0, item) in dense.iter().enumerate() {
                    if matches!(item, Amf3Value::Null | Amf3Value::Undefined) {
                        hazards.push(LuaClientHazard::DenseArrayNilHole {
                            lua_index: idx0 + 1,
                        });
                    }
                    audit_inner(item, hazards);
                }
            }
            Amf3Value::Object {
                sealed,
                dynamic_members,
                externalizable,
                ..
            } => {
                if let Some(ext) = externalizable {
                    hazards.push(LuaClientHazard::ExternalizableObjectUsed);
                    audit_inner(ext, hazards);
                }
                for (_, v) in sealed {
                    audit_inner(v, hazards);
                }
                for (_, v) in dynamic_members {
                    audit_inner(v, hazards);
                }
            }
            _ => {}
        }
    }
}

/// Zlib compatibility shim backed by [`gserver_protocol::zlib`] plus QA zip-bomb generator.
pub mod zlib {
    use std::fmt;

    use gserver_protocol::ZlibError as ProtoZlibError;

    pub use gserver_protocol::zlib::{adler32, zlib_compress};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ZlibError {
        TruncatedHeader,
        InvalidCompressionMethod(u8),
        InvalidWindowSize(u8),
        HeaderChecksumFailed,
        PresetDictUnsupported,
        UnexpectedEof,
        ReservedBlockType,
        StoredLengthComplementMismatch,
        InvalidHuffmanTree,
        InvalidSymbol,
        InvalidDistanceCode(u16),
        DistanceBeforeStart { dist: usize, available: usize },
        OutputLimitExceeded { limit: usize },
        Adler32Mismatch { expected: u32, actual: u32 },
    }

    impl fmt::Display for ZlibError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::TruncatedHeader => write!(f, "zlib stream shorter than 6-byte header+trailer"),
                Self::InvalidCompressionMethod(cm) => write!(f, "invalid zlib CM={cm} (expected 8)"),
                Self::InvalidWindowSize(ci) => write!(f, "invalid zlib CINFO={ci} (expected <= 7)"),
                Self::HeaderChecksumFailed => write!(f, "zlib CMF/FLG FCHECK modulo-31 failed"),
                Self::PresetDictUnsupported => write!(f, "zlib FDICT preset dictionary unsupported"),
                Self::UnexpectedEof => write!(f, "unexpected EOF in DEFLATE bitstream"),
                Self::ReservedBlockType => write!(f, "reserved DEFLATE block type BTYPE=11"),
                Self::StoredLengthComplementMismatch => write!(f, "stored block LEN != !NLEN"),
                Self::InvalidHuffmanTree => write!(f, "invalid or oversubscribed Huffman code lengths"),
                Self::InvalidSymbol => write!(f, "unassigned Huffman code encountered"),
                Self::InvalidDistanceCode(code) => write!(f, "reserved DEFLATE distance code {code}"),
                Self::DistanceBeforeStart { dist, available } => write!(
                    f,
                    "LZ77 back-reference distance {dist} exceeds output length {available}"
                ),
                Self::OutputLimitExceeded { limit } => {
                    write!(f, "decompressed output exceeds limit {limit} bytes")
                }
                Self::Adler32Mismatch { expected, actual } => write!(
                    f,
                    "zlib Adler-32 mismatch: expected 0x{expected:08x}, computed 0x{actual:08x}"
                ),
            }
        }
    }

    pub(crate) fn map_zlib_error(err: ProtoZlibError, input_len: usize) -> ZlibError {
        match err {
            ProtoZlibError::UnexpectedEof if input_len < 6 => ZlibError::TruncatedHeader,
            ProtoZlibError::UnexpectedEof => ZlibError::UnexpectedEof,
            ProtoZlibError::InvalidZlibHeader { cmf, .. } => {
                let cm = cmf & 0x0F;
                let cinfo = (cmf >> 4) & 0x0F;
                if cm != 8 {
                    ZlibError::InvalidCompressionMethod(cm)
                } else if cinfo > 7 {
                    ZlibError::InvalidWindowSize(cinfo)
                } else {
                    ZlibError::HeaderChecksumFailed
                }
            }
            ProtoZlibError::PresetDictionaryUnsupported => ZlibError::PresetDictUnsupported,
            ProtoZlibError::InvalidBlockType => ZlibError::ReservedBlockType,
            ProtoZlibError::StoredLengthMismatch { .. } => ZlibError::StoredLengthComplementMismatch,
            ProtoZlibError::InvalidDynamicHeader { .. }
            | ProtoZlibError::InvalidHuffmanTable(_)
            | ProtoZlibError::InvalidCodeLengthRepeat => ZlibError::InvalidHuffmanTree,
            ProtoZlibError::InvalidHuffmanCode => ZlibError::InvalidSymbol,
            ProtoZlibError::InvalidBackReference {
                distance,
                output_len,
            } => ZlibError::DistanceBeforeStart {
                dist: distance,
                available: output_len,
            },
            ProtoZlibError::OutputLimitExceeded { max_output_size } => {
                ZlibError::OutputLimitExceeded {
                    limit: max_output_size,
                }
            }
            ProtoZlibError::Adler32Mismatch { expected, actual } => {
                ZlibError::Adler32Mismatch { expected, actual }
            }
        }
    }

    /// Decompresses an RFC 1950 zlib stream with strict `max_output` zip-bomb enforcement.
    pub fn zlib_decompress_bounded(input: &[u8], max_output: usize) -> Result<Vec<u8>, ZlibError> {
        gserver_protocol::zlib::zlib_decompress_with_limit(input, max_output)
            .map_err(|e| map_zlib_error(e, input.len()))
    }

    /// Builds a compact RFC 1950 Fixed-Huffman RLE zero-bomb expanding to `target_zeros` zero bytes
    /// for adversarial bounded-decompression QA verification.
    pub fn craft_zlib_zero_bomb(target_zeros: usize) -> Vec<u8> {
        if target_zeros == 0 {
            return zlib_compress(&[]);
        }

        let mut out = Vec::with_capacity(target_zeros / 128 + 32);
        out.push(0x78);
        out.push(0x9C);

        let mut bit_buf: u64 = 0;
        let mut bit_count: u8 = 0;
        let mut push_bits = |val: u32, count: u8, dst: &mut Vec<u8>| {
            if count == 0 {
                return;
            }
            bit_buf |= u64::from(val & ((1u32 << count) - 1)) << bit_count;
            bit_count += count;
            while bit_count >= 8 {
                dst.push((bit_buf & 0xFF) as u8);
                bit_buf >>= 8;
                bit_count -= 8;
            }
        };
        let push_msb =
            |code: u16, bits: u8, push: &mut dyn FnMut(u32, u8, &mut Vec<u8>), dst: &mut Vec<u8>| {
                let rev = code.reverse_bits() >> (16 - bits);
                push(u32::from(rev), bits, dst);
            };

        push_bits(1, 1, &mut out); // BFINAL = 1
        push_bits(1, 2, &mut out); // BTYPE = 01 (Fixed Huffman)
        push_msb(0x30, 8, &mut push_bits, &mut out); // literal 0x00

        let mut rem = target_zeros - 1;
        while rem >= 258 {
            push_msb(0xC5, 8, &mut push_bits, &mut out); // sym 285 -> length 258
            push_msb(0x00, 5, &mut push_bits, &mut out); // dist code 0 -> distance 1
            rem -= 258;
        }
        for _ in 0..rem {
            push_msb(0x30, 8, &mut push_bits, &mut out); // trailing literal 0x00
        }
        push_msb(0x00, 7, &mut push_bits, &mut out); // EOB sym 256
        if bit_count > 0 {
            out.push((bit_buf & 0xFF) as u8);
        }

        let s1: u32 = 1;
        let s2: u32 = ((target_zeros as u64) % 65521) as u32;
        out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
        out
    }
}

/// Cryptographic, URL-signing, and `bt_decrypt` shim backed by [`gserver_protocol`].
pub mod crypto {
    use std::collections::BTreeMap;
    use std::fmt;

    pub use gserver_protocol::bt_decrypt::{
        bt_decrypt as bt_decrypt_asset, bt_encrypt as bt_encrypt_asset, BT_DECRYPT_HEADER_SIZE,
        BT_DECRYPT_MAGIC, BT_KEY_TABLES,
    };
    pub use gserver_protocol::md5::{
        bytes_to_hex_lower, compute_babeltime_md5, md5 as md5_raw, md5_hex,
        sign_mapi_url as sign_platform_url, Md5 as Md5Hasher,
        DEFAULT_FALLBACK_SIGN_KEY as DEFAULT_RUSSIA_SIGN_KEY,
        VNSG2_MAPI_APP_KEY as LOCAL_DEV_SIGN_KEY, VNSG2_MAPI_APP_KEY as VNSG2_LIVE_SIGN_KEY,
    };
    pub use gserver_protocol::url::form_decode as percent_decode;
    pub use gserver_protocol::BtDecryptError;

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
                    write!(f, "signature mismatch: expected {expected}, got {actual}")
                }
            }
        }
    }

    pub fn sort_url_params(url_or_query: &str) -> String {
        gserver_protocol::md5::sort_url_params(url_or_query, false)
    }

    pub fn verify_platform_url_signature(
        url_or_query: &str,
        sign_key: &str,
    ) -> Result<(), SignatureError> {
        let query = match url_or_query.find('?') {
            Some(idx) => &url_or_query[idx + 1..],
            None => url_or_query,
        };
        let actual = query
            .split('&')
            .find_map(|seg| seg.strip_prefix("sign="))
            .ok_or(SignatureError::MissingSignParam)?;
        let expected = gserver_protocol::md5::compute_mapi_signature(url_or_query, sign_key);
        if gserver_protocol::md5::constant_time_eq_hex(&expected, actual) {
            Ok(())
        } else {
            Err(SignatureError::InvalidSignature {
                expected,
                actual: actual.to_string(),
            })
        }
    }

    pub fn parse_tolerant_query_params(url_or_query: &str) -> BTreeMap<String, String> {
        let query = match url_or_query.find('?') {
            Some(idx) => &url_or_query[idx + 1..],
            None => url_or_query,
        };
        let mut map = gserver_protocol::url::parse_query_params(query);
        map.remove("");
        map
    }
}
