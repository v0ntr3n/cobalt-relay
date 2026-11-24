#![forbid(unsafe_code)]

//! Pure-Rust AMF3 encoder, decoder, and hostile compatibility auditor engineered for 100%
//! behavioral parity with `Amf3Module.cpp` in the GServer (`vnsg2`) Win32 C++ client.
//!
//! Key `Amf3Module.cpp` protocol contracts enforced here:
//! 1. **29-bit Signed Integer (`0x04`) vs IEEE-754 Double (`0x05`)**:
//!    - U29 signed bounds are `-268_435_456..=268_435_455` (`-2^28 ..= 2^28 - 1`).
//!    - Negative integers (`-268_435_456..=-1`) mask with `0x1FFF_FFFF` (`bit 28 == 1`), which ALWAYS
//!      produces a 4-byte U29 sequence (`[0xC0..=0xFF, 0x80..=0xFF, 0x80..=0xFF, 0x00..=0xFF]`)
//!      whose 4th byte carries all 8 low bits (`(acc << 8) | b3`), sign-extended via `raw | 0xE000_0000`.
//!    - Any integer `< -268_435_456` or `> 268_435_455` (such as Unix timestamps like `1_795_500_450`)
//!      is **automatically promoted to `0x05 Double` (8-byte BE IEEE-754)**, preventing silent 3-bit
//!      truncation or negative sign-extension in `Network.lua:128` (`mOffset = data.time - os.time()`).
//!    - `-0.0` is preserved as `0x05 Double` (`0x8000_0000_0000_0000`), never `0x04` integer `0`.
//! 2. **Empty Lua Tables `{}` vs Dynamic Maps (`0x09` vs `0x0A`)**:
//!    - Empty Lua table `{}` encodes as `0x09 Array` (`[0x09, 0x01, 0x01]`), NOT `0x0A Object`.
//!    - Pure string-keyed maps encode as anonymous dynamic `0x0A Object` (`[0x0A, 0x0B, 0x01, ..., 0x01]`).
//!    - Mixed Lua tables (`1..N` dense + extra string keys) encode as `0x09 Array` with associative pairs
//!      before `0x01` and dense elements after `0x01`.
//! 3. **Reference Table Isolation**:
//!    - Empty string `""` (`0x01`) is **NEVER** inserted into `string_refs`.
//!    - `string_refs` is shared across `0x06` strings, `0x09` associative keys, and `0x0A` class names,
//!      sealed member names, and dynamic keys.
//!    - `object_refs` is shared across `0x07 XmlDoc`, `0x08 Date`, `0x09 Array`, `0x0A Object`,
//!      `0x0B Xml`, and `0x0C ByteArray` (even a 0-byte `0x0C 0x01` ByteArray registers an object ref!).
//! 4. **Recursion & Allocation Bomb Guards**:
//!    - `MAX_RECURSION_DEPTH = 64` (`Amf3Module.cpp:37`: `depth > 64` fails cleanly).
//!    - Pre-allocations are capped by remaining input bytes so crafted U29 length headers
//!      (`[0x09, 0xBF, 0xFF, 0xFF, 0xFF]`) never cause multi-gigabyte OOM allocations.

use std::collections::HashMap;
use std::fmt;

pub const MARKER_UNDEFINED: u8 = 0x00;
pub const MARKER_NULL: u8 = 0x01;
pub const MARKER_FALSE: u8 = 0x02;
pub const MARKER_TRUE: u8 = 0x03;
pub const MARKER_INTEGER: u8 = 0x04;
pub const MARKER_DOUBLE: u8 = 0x05;
pub const MARKER_STRING: u8 = 0x06;
pub const MARKER_XML_DOC: u8 = 0x07;
pub const MARKER_DATE: u8 = 0x08;
pub const MARKER_ARRAY: u8 = 0x09;
pub const MARKER_OBJECT: u8 = 0x0A;
pub const MARKER_XML: u8 = 0x0B;
pub const MARKER_BYTE_ARRAY: u8 = 0x0C;

/// Minimum 29-bit signed integer (`-2^28`).
pub const AMF3_INT_MIN: i32 = -268_435_456;
/// Maximum 29-bit signed integer (`+2^28 - 1`).
pub const AMF3_INT_MAX: i32 = 268_435_455;
/// Maximum exact integer representable in IEEE-754 `f64` (`2^53`).
/// Values above this (e.g., Platform `qid = 1243831974477471744`) MUST be encoded as `0x06 String`!
pub const MAX_SAFE_F64_INT: u64 = 9_007_199_254_740_992;
/// Maximum nesting depth matching `Amf3Module.cpp:37` (`kMaxRecursionDepth = 64`).
pub const MAX_RECURSION_DEPTH: usize = 64;

/// Binary-safe AMF3 value tree (Lua 5.1 strings are arbitrary 8-bit byte slices).
#[derive(Debug, Clone, PartialEq)]
pub enum Amf3Value {
    Undefined,
    Null,
    Bool(bool),
    /// 29-bit signed integer (`-268_435_456..=268_435_455`). If constructed with an `i32` outside
    /// U29 range, our encoder automatically promotes it on the wire to `0x05 Double`!
    Integer(i32),
    /// 64-bit IEEE-754 floating-point (`0x05`).
    Double(f64),
    /// 8-bit clean string (`0x06`, stored in `string_refs` when non-empty).
    String(Vec<u8>),
    /// Legacy XMLDocument (`0x07`, stored in `object_refs`, pushed as Lua string in `Amf3Module.cpp`).
    XmlDoc(Vec<u8>),
    /// Epoch milliseconds (`0x08`, stored in `object_refs`, pushed as Lua number in `Amf3Module.cpp`).
    Date(f64),
    /// Dense 1-based Lua sequence + optional string-keyed associative map (`0x09`, stored in `object_refs`).
    Array {
        assoc: Vec<(Vec<u8>, Amf3Value)>,
        dense: Vec<Amf3Value>,
    },
    /// Sealed/dynamic/externalizable object (`0x0A`, stored in `object_refs` + `trait_refs`).
    Object {
        class_name: Vec<u8>,
        dynamic: bool,
        sealed: Vec<(Vec<u8>, Amf3Value)>,
        dynamic_members: Vec<(Vec<u8>, Amf3Value)>,
        externalizable: Option<Box<Amf3Value>>,
    },
    /// E4X XML (`0x0B`, stored in `object_refs`, pushed as Lua string in `Amf3Module.cpp`).
    Xml(Vec<u8>),
    /// Raw ByteArray (`0x0C`, stored in `object_refs` — NEVER `string_refs`!).
    ByteArray(Vec<u8>),
}

impl Amf3Value {
    /// Encodes an `i64` safely for `Amf3Module.cpp`: uses `0x04 Integer` within
    /// `-268_435_456..=268_435_455` and automatically promotes larger/smaller values
    /// (such as Unix timestamps like `1_795_500_450`) to `0x05 Double`.
    pub fn from_i64_auto(v: i64) -> Self {
        if (AMF3_INT_MIN as i64..=AMF3_INT_MAX as i64).contains(&v) {
            Self::Integer(v as i32)
        } else {
            Self::Double(v as f64)
        }
    }

    /// Encodes a Lua `lua_Number` (`f64`) using the exact logic of `Amf3Encoder::encodeNumber`
    /// (`Amf3Module.cpp:156-169`), including preserving `-0.0` as `0x05 Double`.
    pub fn from_lua_number(d: f64) -> Self {
        if d.is_finite() && d >= (AMF3_INT_MIN as f64) && d <= (AMF3_INT_MAX as f64) {
            let iv = d as i32;
            if (iv as f64) == d && !(d == 0.0 && d.is_sign_negative()) {
                return Self::Integer(iv);
            }
        }
        Self::Double(d)
    }

    /// Encodes a 64-bit integer ID safely without IEEE-754 mantissa loss: if `|v| > 2^53`
    /// (such as Platform `qid = 1243831974477471744`), promotes to `0x06 String` just like Platform's backend.
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

    /// Empty Lua table `{}`: `Amf3Module.cpp:228,279` encodes `{}` as `[0x09, 0x01, 0x01]`.
    pub fn empty_lua_table() -> Self {
        Self::Array {
            assoc: Vec::new(),
            dense: Vec::new(),
        }
    }

    /// Dense 1-based Lua sequence `{ v1, v2, ... }` (`0x09`).
    pub fn dense_array(items: Vec<Amf3Value>) -> Self {
        Self::Array {
            assoc: Vec::new(),
            dense: items,
        }
    }

    /// Anonymous dynamic AMF3 Object (`[0x0A, 0x0B, 0x01, ...k/v..., 0x01]`), matching
    /// `Amf3Module.cpp:228-275` for non-empty pure string-keyed Lua tables.
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

    /// Returns UTF-8 string view for `String`, `ByteArray`, `XmlDoc`, or `Xml`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(b) | Self::ByteArray(b) | Self::XmlDoc(b) | Self::Xml(b) => {
                std::str::from_utf8(b).ok()
            }
            _ => None,
        }
    }

    /// Returns raw bytes for `String`, `ByteArray`, `XmlDoc`, or `Xml` ( matching Lua `lua_tolstring`).
    pub fn as_lua_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::String(b) | Self::ByteArray(b) | Self::XmlDoc(b) | Self::Xml(b) => Some(b),
            _ => None,
        }
    }

    /// Returns numeric value as `f64` (matching Lua `lua_tonumber` for `Integer`, `Double`, `Date`).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(i) => Some(*i as f64),
            Self::Double(d) | Self::Date(d) => Some(*d),
            _ => None,
        }
    }

    /// Returns integer value as `i64` if exact.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(i) => Some(*i as i64),
            Self::Double(d) if d.is_finite() && d.fract() == 0.0 => Some(*d as i64),
            _ => None,
        }
    }

    /// Looks up a string key on either an `Object` (sealed or dynamic) or `Array` associative map,
    /// matching how Lua indexes decoded tables (`tbl[key]`).
    pub fn get_field(&self, key: &str) -> Option<&Amf3Value> {
        let target = key.as_bytes();
        match self {
            Self::Object {
                sealed,
                dynamic_members,
                ..
            } => {
                for (k, v) in dynamic_members.iter().rev() {
                    if k.as_slice() == target {
                        return Some(v);
                    }
                }
                for (k, v) in sealed.iter().rev() {
                    if k.as_slice() == target {
                        return Some(v);
                    }
                }
                None
            }
            Self::Array { assoc, .. } => {
                for (k, v) in assoc.iter().rev() {
                    if k.as_slice() == target {
                        return Some(v);
                    }
                }
                None
            }
            _ => None,
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
                write!(f, "amf3.decode: unexpected end of input at offset {}", offset)
            }
            Self::UnsupportedMarker { marker, offset } => write!(
                f,
                "amf3.decode: unsupported AMF3 type marker 0x{:02X} at offset {}",
                marker, offset
            ),
            Self::MaxDepthExceeded { depth } => {
                write!(f, "amf3: maximum nesting depth exceeded ({})", depth)
            }
            Self::InvalidStringRef { index, table_len } => write!(
                f,
                "amf3.decode: invalid string reference index {} (len={})",
                index, table_len
            ),
            Self::InvalidObjectRef { index, table_len } => write!(
                f,
                "amf3.decode: invalid object reference index {} (len={})",
                index, table_len
            ),
            Self::InvalidTraitRef { index, table_len } => write!(
                f,
                "amf3.decode: invalid trait reference index {} (len={})",
                index, table_len
            ),
            Self::StringTooLong { len } => {
                write!(f, "amf3.encode: string/array length {} exceeds U29 limit", len)
            }
            Self::EmptyObjectKey => write!(f, "amf3.encode: dynamic/associative key cannot be empty"),
        }
    }
}

/// Low-level U29 unsigned integer encoder (`0..=0x1FFF_FFFF`, `Amf3Module.cpp:92-109`).
pub fn write_u29(out: &mut Vec<u8>, mut v: u32) {
    v &= 0x1FFF_FFFF;
    if v < 0x80 {
        out.push(v as u8);
    } else if v < 0x4000 {
        out.push((((v >> 7) & 0x7F) | 0x80) as u8);
        out.push((v & 0x7F) as u8);
    } else if v < 0x20_0000 {
        out.push((((v >> 14) & 0x7F) | 0x80) as u8);
        out.push((((v >> 7) & 0x7F) | 0x80) as u8);
        out.push((v & 0x7F) as u8);
    } else {
        out.push((((v >> 22) & 0x7F) | 0x80) as u8);
        out.push((((v >> 15) & 0x7F) | 0x80) as u8);
        out.push((((v >> 8) & 0x7F) | 0x80) as u8);
        out.push((v & 0xFF) as u8);
    }
}

/// Low-level U29 reader (`Amf3Module.cpp:428-447`).
pub fn read_u29(data: &[u8], pos: &mut usize) -> Result<u32, Amf3Error> {
    let mut result = 0u32;
    for _ in 0..3 {
        if *pos >= data.len() {
            return Err(Amf3Error::UnexpectedEof { offset: *pos });
        }
        let b = data[*pos];
        *pos += 1;
        result = (result << 7) | ((b & 0x7F) as u32);
        if (b & 0x80) == 0 {
            return Ok(result);
        }
    }
    if *pos >= data.len() {
        return Err(Amf3Error::UnexpectedEof { offset: *pos });
    }
    let last = data[*pos];
    *pos += 1;
    Ok((result << 8) | (last as u32))
}

/// Sign-extends a 29-bit raw U29 integer (`Amf3Module.cpp:710-712`).
pub fn sign_extend_u29(raw: u32) -> i32 {
    if (raw & 0x1000_0000) != 0 {
        (raw | 0xE000_0000) as i32
    } else {
        (raw & 0x0FFF_FFFF) as i32
    }
}

/// Demonstrates the classic private-server bug: masking an out-of-range integer/timestamp
/// (`v > 268_435_455`, e.g., `1_795_500_450`) into a `0x04` U29 integer instead of promoting to `0x05 Double`.
pub fn encode_buggy_truncated_u29_integer(v: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(MARKER_INTEGER);
    write_u29(&mut out, (v as u32) & 0x1FFF_FFFF);
    out
}

#[derive(Default)]
pub struct Amf3Encoder {
    out: Vec<u8>,
    string_refs: HashMap<Vec<u8>, u32>,
}

impl Amf3Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(val: &Amf3Value) -> Result<Vec<u8>, Amf3Error> {
        let mut enc = Self {
            out: Vec::with_capacity(128),
            string_refs: HashMap::new(),
        };
        enc.encode_value(val, 0)?;
        Ok(enc.out)
    }

    fn write_utf8_vr(&mut self, bytes: &[u8]) -> Result<(), Amf3Error> {
        // CRITICAL `Amf3Module.cpp:128-132`: Empty string is ALWAYS `0x01` and NEVER stored in `stringRefs`!
        if bytes.is_empty() {
            self.out.push(0x01);
            return Ok(());
        }
        if let Some(&ref_idx) = self.string_refs.get(bytes) {
            write_u29(&mut self.out, ref_idx << 1);
            return Ok(());
        }
        if bytes.len() > (AMF3_INT_MAX as usize) {
            return Err(Amf3Error::StringTooLong { len: bytes.len() });
        }
        let next_idx = self.string_refs.len() as u32;
        if next_idx <= (AMF3_INT_MAX as u32) {
            self.string_refs.insert(bytes.to_vec(), next_idx);
        }
        write_u29(&mut self.out, ((bytes.len() as u32) << 1) | 1);
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn encode_value(&mut self, val: &Amf3Value, depth: usize) -> Result<(), Amf3Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Amf3Error::MaxDepthExceeded { depth });
        }
        match val {
            Amf3Value::Undefined => self.out.push(MARKER_UNDEFINED),
            Amf3Value::Null => self.out.push(MARKER_NULL),
            Amf3Value::Bool(false) => self.out.push(MARKER_FALSE),
            Amf3Value::Bool(true) => self.out.push(MARKER_TRUE),
            Amf3Value::Integer(iv) => {
                // Enforce automatic promotion to 0x05 Double if caller passed an i32 outside 29-bit signed range!
                if (*iv >= AMF3_INT_MIN) && (*iv <= AMF3_INT_MAX) {
                    self.out.push(MARKER_INTEGER);
                    write_u29(&mut self.out, (*iv as u32) & 0x1FFF_FFFF);
                } else {
                    self.out.push(MARKER_DOUBLE);
                    self.out.extend_from_slice(&(*iv as f64).to_be_bytes());
                }
            }
            Amf3Value::Double(d) => {
                self.out.push(MARKER_DOUBLE);
                self.out.extend_from_slice(&d.to_be_bytes());
            }
            Amf3Value::String(s) => {
                self.out.push(MARKER_STRING);
                self.write_utf8_vr(s)?;
            }
            Amf3Value::XmlDoc(b) => {
                if b.len() > (AMF3_INT_MAX as usize) {
                    return Err(Amf3Error::StringTooLong { len: b.len() });
                }
                self.out.push(MARKER_XML_DOC);
                write_u29(&mut self.out, ((b.len() as u32) << 1) | 1);
                self.out.extend_from_slice(b);
            }
            Amf3Value::Date(ms) => {
                self.out.push(MARKER_DATE);
                write_u29(&mut self.out, 0x01); // inline date (U29D-value = 1)
                self.out.extend_from_slice(&ms.to_be_bytes());
            }
            Amf3Value::Array { assoc, dense } => {
                if dense.len() > (AMF3_INT_MAX as usize) {
                    return Err(Amf3Error::StringTooLong { len: dense.len() });
                }
                self.out.push(MARKER_ARRAY);
                write_u29(&mut self.out, ((dense.len() as u32) << 1) | 1);
                for (k, v) in assoc {
                    if k.is_empty() {
                        return Err(Amf3Error::EmptyObjectKey);
                    }
                    self.write_utf8_vr(k)?;
                    self.encode_value(v, depth + 1)?;
                }
                self.out.push(0x01); // empty string terminates associative portion
                for item in dense {
                    self.encode_value(item, depth + 1)?;
                }
            }
            Amf3Value::Object {
                class_name,
                dynamic,
                sealed,
                dynamic_members,
                externalizable,
            } => {
                self.out.push(MARKER_OBJECT);
                if let Some(ext_val) = externalizable {
                    // bit0=1 (inline obj), bit1=1 (inline traits), bit2=1 (externalizable) => 0x07
                    write_u29(&mut self.out, 0x07);
                    self.write_utf8_vr(class_name)?;
                    self.encode_value(ext_val, depth + 1)?;
                } else {
                    if sealed.len() > ((AMF3_INT_MAX as usize) >> 3) {
                        return Err(Amf3Error::StringTooLong { len: sealed.len() });
                    }
                    let mut traits_hdr = ((sealed.len() as u32) << 4) | 0x03;
                    if *dynamic {
                        traits_hdr |= 0x08;
                    }
                    write_u29(&mut self.out, traits_hdr);
                    self.write_utf8_vr(class_name)?;
                    for (member_name, _) in sealed {
                        self.write_utf8_vr(member_name)?;
                    }
                    for (_, member_val) in sealed {
                        self.encode_value(member_val, depth + 1)?;
                    }
                    if *dynamic {
                        for (k, v) in dynamic_members {
                            if k.is_empty() {
                                return Err(Amf3Error::EmptyObjectKey);
                            }
                            self.write_utf8_vr(k)?;
                            self.encode_value(v, depth + 1)?;
                        }
                        self.out.push(0x01); // terminate dynamic members
                    }
                }
            }
            Amf3Value::Xml(b) => {
                if b.len() > (AMF3_INT_MAX as usize) {
                    return Err(Amf3Error::StringTooLong { len: b.len() });
                }
                self.out.push(MARKER_XML);
                write_u29(&mut self.out, ((b.len() as u32) << 1) | 1);
                self.out.extend_from_slice(b);
            }
            Amf3Value::ByteArray(b) => {
                if b.len() > (AMF3_INT_MAX as usize) {
                    return Err(Amf3Error::StringTooLong { len: b.len() });
                }
                self.out.push(MARKER_BYTE_ARRAY);
                write_u29(&mut self.out, ((b.len() as u32) << 1) | 1);
                self.out.extend_from_slice(b);
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct TraitInfo {
    class_name: Vec<u8>,
    externalizable: bool,
    dynamic: bool,
    sealed_members: Vec<Vec<u8>>,
}

pub struct Amf3Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    string_refs: Vec<Vec<u8>>,
    object_refs: Vec<Amf3Value>,
    trait_refs: Vec<TraitInfo>,
}

impl<'a> Amf3Decoder<'a> {
    pub fn new(data: &'a [u8], start_pos: usize) -> Self {
        Self {
            data,
            pos: start_pos,
            string_refs: Vec::new(),
            object_refs: Vec::new(),
            trait_refs: Vec::new(),
        }
    }

    /// Decodes a single AMF3 value starting at `start_pos` (0-based), returning `(value, new_pos)`
    /// matching `amf3.decode(str, [pos=0]) -> val, new_pos` (`Amf3Module.cpp:780-817`).
    pub fn decode_at(data: &'a [u8], start_pos: usize) -> Result<(Amf3Value, usize), Amf3Error> {
        let mut dec = Self::new(data, start_pos);
        let val = dec.decode_value(0)?;
        Ok((val, dec.pos))
    }

    /// Convenience wrapper decoding from offset 0.
    pub fn decode(data: &'a [u8]) -> Result<Amf3Value, Amf3Error> {
        let (val, _) = Self::decode_at(data, 0)?;
        Ok(val)
    }

    fn ensure_available(&self, n: usize) -> Result<(), Amf3Error> {
        if n > self.data.len() || self.pos > self.data.len() - n {
            return Err(Amf3Error::UnexpectedEof { offset: self.pos });
        }
        Ok(())
    }

    fn read_byte(&mut self) -> Result<u8, Amf3Error> {
        self.ensure_available(1)?;
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_double_be(&mut self) -> Result<f64, Amf3Error> {
        self.ensure_available(8)?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&self.data[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(f64::from_be_bytes(buf))
    }

    fn read_utf8_vr(&mut self) -> Result<Vec<u8>, Amf3Error> {
        let header = read_u29(self.data, &mut self.pos)?;
        if (header & 1) == 0 {
            let idx = header >> 1;
            let entry = self
                .string_refs
                .get(idx as usize)
                .ok_or(Amf3Error::InvalidStringRef {
                    index: idx,
                    table_len: self.string_refs.len(),
                })?;
            return Ok(entry.clone());
        }
        let len = (header >> 1) as usize;
        if len == 0 {
            // Empty string is NEVER pushed to `string_refs` (`Amf3Module.cpp:478-482`).
            return Ok(Vec::new());
        }
        self.ensure_available(len)?;
        let s = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        self.string_refs.push(s.clone());
        Ok(s)
    }

    fn get_object_ref(&self, idx: u32) -> Result<Amf3Value, Amf3Error> {
        self.object_refs
            .get(idx as usize)
            .cloned()
            .ok_or(Amf3Error::InvalidObjectRef {
                index: idx,
                table_len: self.object_refs.len(),
            })
    }

    fn decode_xml_or_byte_array(&mut self, marker: u8) -> Result<Amf3Value, Amf3Error> {
        let header = read_u29(self.data, &mut self.pos)?;
        if (header & 1) == 0 {
            return self.get_object_ref(header >> 1);
        }
        let len = (header >> 1) as usize;
        self.ensure_available(len)?;
        let bytes = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        let val = match marker {
            MARKER_XML_DOC => Amf3Value::XmlDoc(bytes),
            MARKER_XML => Amf3Value::Xml(bytes),
            _ => Amf3Value::ByteArray(bytes),
        };
        // CRITICAL `Amf3Module.cpp:522`: Even 0-byte `ByteArray`/`Xml` registers an object ref!
        self.object_refs.push(val.clone());
        Ok(val)
    }

    fn decode_date(&mut self) -> Result<Amf3Value, Amf3Error> {
        let header = read_u29(self.data, &mut self.pos)?;
        if (header & 1) == 0 {
            return self.get_object_ref(header >> 1);
        }
        let ms = self.read_double_be()?;
        let val = Amf3Value::Date(ms);
        self.object_refs.push(val.clone());
        Ok(val)
    }

    fn decode_array(&mut self, depth: usize) -> Result<Amf3Value, Amf3Error> {
        let header = read_u29(self.data, &mut self.pos)?;
        if (header & 1) == 0 {
            return self.get_object_ref(header >> 1);
        }
        let dense_count = (header >> 1) as usize;
        // Reserve object reference slot BEFORE decoding children (matching `Amf3Module.cpp:555`).
        let obj_slot = self.object_refs.len();
        self.object_refs.push(Amf3Value::Null);

        let mut assoc = Vec::new();
        loop {
            let key = self.read_utf8_vr()?;
            if key.is_empty() {
                break;
            }
            let val = self.decode_value(depth + 1)?;
            assoc.push((key, val));
        }

        // Guard against allocation bombs: each AMF3 element needs >= 1 byte on the wire!
        let remaining = self.data.len().saturating_sub(self.pos);
        let safe_cap = dense_count.min(remaining).min(4096);
        let mut dense = Vec::with_capacity(safe_cap);
        for _ in 0..dense_count {
            dense.push(self.decode_value(depth + 1)?);
        }

        let arr = Amf3Value::Array { assoc, dense };
        self.object_refs[obj_slot] = arr.clone();
        Ok(arr)
    }

    fn decode_object(&mut self, depth: usize) -> Result<Amf3Value, Amf3Error> {
        let header = read_u29(self.data, &mut self.pos)?;
        if (header & 1) == 0 {
            return self.get_object_ref(header >> 1);
        }

        let traits = if (header & 0x02) == 0 {
            let trait_idx = header >> 2;
            self.trait_refs
                .get(trait_idx as usize)
                .cloned()
                .ok_or(Amf3Error::InvalidTraitRef {
                    index: trait_idx,
                    table_len: self.trait_refs.len(),
                })?
        } else {
            let externalizable = (header & 0x04) != 0;
            let dynamic = (header & 0x08) != 0;
            let sealed_count = (header >> 4) as usize;
            let class_name = self.read_utf8_vr()?;

            let remaining = self.data.len().saturating_sub(self.pos);
            let mut sealed_members = Vec::with_capacity(sealed_count.min(remaining).min(1024));
            for _ in 0..sealed_count {
                sealed_members.push(self.read_utf8_vr()?);
            }
            let info = TraitInfo {
                class_name,
                externalizable,
                dynamic,
                sealed_members,
            };
            self.trait_refs.push(info.clone());
            info
        };

        let obj_slot = self.object_refs.len();
        self.object_refs.push(Amf3Value::Null);

        if traits.externalizable {
            // `Amf3Module.cpp:633-639` decodes one nested AMF3 value into `tbl["_external"]`.
            let inner = self.decode_value(depth + 1)?;
            let obj = Amf3Value::Object {
                class_name: traits.class_name,
                dynamic: traits.dynamic,
                sealed: Vec::new(),
                dynamic_members: Vec::new(),
                externalizable: Some(Box::new(inner)),
            };
            self.object_refs[obj_slot] = obj.clone();
            return Ok(obj);
        }

        let remaining = self.data.len().saturating_sub(self.pos);
        let mut sealed = Vec::with_capacity(traits.sealed_members.len().min(remaining).min(1024));
        for name in traits.sealed_members {
            let val = self.decode_value(depth + 1)?;
            sealed.push((name, val));
        }

        let mut dynamic_members = Vec::new();
        if traits.dynamic {
            loop {
                let key = self.read_utf8_vr()?;
                if key.is_empty() {
                    break;
                }
                let val = self.decode_value(depth + 1)?;
                dynamic_members.push((key, val));
            }
        }

        let obj = Amf3Value::Object {
            class_name: traits.class_name,
            dynamic: traits.dynamic,
            sealed,
            dynamic_members,
            externalizable: None,
        };
        self.object_refs[obj_slot] = obj.clone();
        Ok(obj)
    }

    fn decode_value(&mut self, depth: usize) -> Result<Amf3Value, Amf3Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Amf3Error::MaxDepthExceeded { depth });
        }
        let marker_offset = self.pos;
        let marker = self.read_byte()?;
        match marker {
            MARKER_UNDEFINED => Ok(Amf3Value::Undefined),
            MARKER_NULL => Ok(Amf3Value::Null),
            MARKER_FALSE => Ok(Amf3Value::Bool(false)),
            MARKER_TRUE => Ok(Amf3Value::Bool(true)),
            MARKER_INTEGER => {
                let raw = read_u29(self.data, &mut self.pos)?;
                Ok(Amf3Value::Integer(sign_extend_u29(raw)))
            }
            MARKER_DOUBLE => Ok(Amf3Value::Double(self.read_double_be()?)),
            MARKER_STRING => Ok(Amf3Value::String(self.read_utf8_vr()?)),
            MARKER_XML_DOC | MARKER_XML | MARKER_BYTE_ARRAY => {
                self.decode_xml_or_byte_array(marker)
            }
            MARKER_DATE => self.decode_date(),
            MARKER_ARRAY => self.decode_array(depth),
            MARKER_OBJECT => self.decode_object(depth),
            other => Err(Amf3Error::UnsupportedMarker {
                marker: other,
                offset: marker_offset,
            }),
        }
    }
}

// ============================================================================
// Hostile Win32 Client Compatibility Hazard Auditor
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaClientHazard {
    /// `0x01 Null` or `0x00 Undefined` at 1-based index `lua_index` inside a dense AMF3 Array (`0x09`)
    /// creates a `nil` hole in Lua 5.1 (`lua_rawseti`), truncating `#t` / `ipairs(t)` or crashing
    /// `Network.lua:116` (`amf3.decode(nil)`).
    DenseArrayNilHole { lua_index: usize },
    /// `Amf3Value::Double` holds an integer whose magnitude exceeds `2^53`, losing low-bit precision
    /// in Lua `lua_Number` (must be sent as `0x06 String`, e.g., Platform `qid`).
    UnsafeLargeFloatInteger,
    /// Externalizable `0x0A` object maps into `{ _external = ... }` in `Amf3Module.cpp:637`, which
    /// standard GServer Lua scripts never read.
    ExternalizableObjectUsed,
}

/// Audits an outgoing server-to-client `Amf3Value` tree for subtle Lua 5.1 / `Amf3Module.cpp` traps.
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
