//! Pure-Rust AMF3 encoder and decoder matching `Amf3Module.cpp` in the `vnsg2` client.
//!
//! ### Critical Lua <-> AMF3 Mapping Rules (`Amf3Module.cpp`)
//! 1. **1-Based Dense Lua Sequences (`{ v1, v2, ... }` or empty `{}`)**:
//!    Encoded as AMF3 `Array` (`0x09`) with U29 `(dense_len << 1) | 1`, `0x01` empty associative
//!    key terminator, followed by `dense_len` values. In Lua, indices map to `1..=dense_len`.
//! 2. **String-Keyed Lua Dictionaries (`{ ["uid"] = 10001, ... }`)**:
//!    Encoded as AMF3 anonymous dynamic `Object` (`0x0A`) with inline dynamic traits `0x0B`
//!    (`0` sealed properties), empty class name `0x01`, UTF-8 key + AMF3 value pairs, terminated
//!    by empty string `0x01`.
//! 3. **Lua Numbers (`Int` vs `Double`)**:
//!    AMF3 `Integer` (`0x04`) is a signed 29-bit integer (`[-268_435_456, 268_435_455]`). Any
//!    integer outside that range (such as Unix timestamps ~`1.79e9` or 64-bit `uid`/`pid` values
//!    when numeric) MUST be encoded as AMF3 `Double` (`0x05`, IEEE-754 big-endian `f64`).

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// Minimum signed 29-bit integer representable by AMF3 `0x04` (`-2^28`).
pub const AMF3_INT_MIN: i64 = -268_435_456;
/// Maximum signed 29-bit integer representable by AMF3 `0x04` (`2^28 - 1`).
pub const AMF3_INT_MAX: i64 = 268_435_455;

const MARKER_UNDEFINED: u8 = 0x00;
const MARKER_NULL: u8 = 0x01;
const MARKER_FALSE: u8 = 0x02;
const MARKER_TRUE: u8 = 0x03;
const MARKER_INTEGER: u8 = 0x04;
const MARKER_DOUBLE: u8 = 0x05;
const MARKER_STRING: u8 = 0x06;
const MARKER_DATE: u8 = 0x08;
const MARKER_ARRAY: u8 = 0x09;
const MARKER_OBJECT: u8 = 0x0A;
const MARKER_BYTE_ARRAY: u8 = 0x0C;

/// AMF3 value representation aligned with Cocos2d-x Lua table semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum Amf3Value {
    Undefined,
    Null,
    Bool(bool),
    /// Signed integer; encoded as AMF3 `Integer` (`0x04`) when within 29-bit signed bounds,
    /// or automatically promoted to AMF3 `Double` (`0x05`) otherwise.
    Int(i64),
    /// IEEE-754 64-bit floating point (`0x05`).
    Double(f64),
    /// UTF-8 string (`0x06`).
    String(String),
    /// AMF3 Date (`0x08`, milliseconds since Unix epoch).
    Date(f64),
    /// Dense 1-based Lua sequence (`0x09` with empty associative portion).
    Array(Vec<Amf3Value>),
    /// String-keyed Lua table (`0x0A` dynamic anonymous object, or `0x09` associative table).
    Map(BTreeMap<String, Amf3Value>),
    /// Raw byte buffer (`0x0C` ByteArray, used e.g. for compressed sub-packets or chat payloads).
    ByteArray(Vec<u8>),
}

impl Amf3Value {
    pub fn string(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }

    pub fn int(v: i64) -> Self {
        Self::Int(v)
    }

    pub fn empty_array() -> Self {
        Self::Array(Vec::new())
    }

    pub fn empty_map() -> Self {
        Self::Map(BTreeMap::new())
    }

    pub fn map_from<I, K>(iter: I) -> Self
    where
        I: IntoIterator<Item = (K, Amf3Value)>,
        K: Into<String>,
    {
        let mut m = BTreeMap::new();
        for (k, v) in iter {
            m.insert(k.into(), v);
        }
        Self::Map(m)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns a numeric value as `i64` if `Int` or an integral finite `Double`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Double(f) if f.is_finite() && f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }

    /// Returns any Lua number (`Int` or `Double`) as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(n) => Some(*n as f64),
            Self::Double(f) if f.is_finite() => Some(*f),
            _ => None,
        }
    }

    /// Returns `true` if Lua `tonumber(v)` would succeed (either a number or numeric string).
    pub fn is_lua_tonumber_compatible(&self) -> bool {
        if self.as_f64().is_some() {
            return true;
        }
        if let Some(s) = self.as_str() {
            let trimmed = s.trim();
            !trimmed.is_empty() && trimmed.parse::<f64>().is_ok()
        } else {
            false
        }
    }

    pub fn as_array(&self) -> Option<&[Amf3Value]> {
        match self {
            Self::Array(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, Amf3Value>> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// In Cocos2d-x Lua (`Amf3Module.cpp`), both an empty AMF3 `Array` (`0x09 0x01 0x01`) and an
    /// empty AMF3 `Object` (`0x0A 0x0B 0x01 0x01`) decode to an empty Lua table `{}` where
    /// `pairs(t)` iterates 0 times without error.
    pub fn is_lua_table(&self) -> bool {
        matches!(self, Self::Map(_) | Self::Array(_))
    }

    /// Look up a string key in a `Map`; if this is an empty `Array` (`{}` in Lua), returns `None`
    /// while still confirming table-ness via [`Self::is_lua_table`].
    pub fn get_field(&self, key: &str) -> Option<&Amf3Value> {
        match self {
            Self::Map(m) => m.get(key),
            _ => None,
        }
    }
}

/// Errors encountered while encoding or decoding AMF3 payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Amf3Error {
    UnexpectedEof,
    UnsupportedMarker(u8),
    InvalidUtf8,
    InvalidStringRef(usize),
    InvalidObjectRef(usize),
    UnsupportedSealedObjectTraits,
    TrailingBytes(usize),
}

impl fmt::Display for Amf3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of AMF3 buffer"),
            Self::UnsupportedMarker(m) => write!(f, "unsupported AMF3 type marker 0x{:02X}", m),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 string in AMF3 stream"),
            Self::InvalidStringRef(idx) => write!(f, "invalid AMF3 string reference index {}", idx),
            Self::InvalidObjectRef(idx) => write!(f, "invalid AMF3 object reference index {}", idx),
            Self::UnsupportedSealedObjectTraits => {
                write!(f, "unsupported non-dynamic sealed AMF3 object traits")
            }
            Self::TrailingBytes(n) => write!(f, "{} unexpected trailing bytes after AMF3 root", n),
        }
    }
}

impl std::error::Error for Amf3Error {}

/// Encode an [`Amf3Value`] into raw AMF3 bytes.
pub fn encode_amf3(value: &Amf3Value) -> Vec<u8> {
    let mut enc = Encoder::default();
    enc.write_value(value);
    enc.buf
}

/// Decode a complete AMF3 value from `bytes`, rejecting trailing garbage.
pub fn decode_amf3(bytes: &[u8]) -> Result<Amf3Value, Amf3Error> {
    let mut dec = Decoder::new(bytes);
    let val = dec.read_value()?;
    if dec.pos != bytes.len() {
        return Err(Amf3Error::TrailingBytes(bytes.len() - dec.pos));
    }
    Ok(val)
}

#[derive(Default)]
struct Encoder {
    buf: Vec<u8>,
    string_refs: HashMap<String, u32>,
}

impl Encoder {
    fn write_u29(&mut self, v: u32) {
        let v = v & 0x1FFF_FFFF;
        if v < 0x80 {
            self.buf.push(v as u8);
        } else if v < 0x4000 {
            self.buf.push((((v >> 7) & 0x7F) | 0x80) as u8);
            self.buf.push((v & 0x7F) as u8);
        } else if v < 0x20_0000 {
            self.buf.push((((v >> 14) & 0x7F) | 0x80) as u8);
            self.buf.push((((v >> 7) & 0x7F) | 0x80) as u8);
            self.buf.push((v & 0x7F) as u8);
        } else {
            self.buf.push((((v >> 22) & 0x7F) | 0x80) as u8);
            self.buf.push((((v >> 15) & 0x7F) | 0x80) as u8);
            self.buf.push((((v >> 8) & 0x7F) | 0x80) as u8);
            self.buf.push((v & 0xFF) as u8);
        }
    }

    fn write_amf_string(&mut self, s: &str) {
        if s.is_empty() {
            self.write_u29(1); // inline 0-length string: (0 << 1) | 1
            return;
        }
        if let Some(&idx) = self.string_refs.get(s) {
            if idx < (1 << 28) {
                self.write_u29(idx << 1);
                return;
            }
        }
        let next_idx = self.string_refs.len() as u32;
        self.string_refs.insert(s.to_owned(), next_idx);
        let bytes = s.as_bytes();
        self.write_u29(((bytes.len() as u32) << 1) | 1);
        self.buf.extend_from_slice(bytes);
    }

    fn write_value(&mut self, val: &Amf3Value) {
        match val {
            Amf3Value::Undefined => self.buf.push(MARKER_UNDEFINED),
            Amf3Value::Null => self.buf.push(MARKER_NULL),
            Amf3Value::Bool(false) => self.buf.push(MARKER_FALSE),
            Amf3Value::Bool(true) => self.buf.push(MARKER_TRUE),
            Amf3Value::Int(n) => {
                if (*n >= AMF3_INT_MIN) && (*n <= AMF3_INT_MAX) {
                    self.buf.push(MARKER_INTEGER);
                    let u29 = (*n as i32 as u32) & 0x1FFF_FFFF;
                    self.write_u29(u29);
                } else {
                    self.buf.push(MARKER_DOUBLE);
                    self.buf.extend_from_slice(&(*n as f64).to_be_bytes());
                }
            }
            Amf3Value::Double(f) => {
                self.buf.push(MARKER_DOUBLE);
                self.buf.extend_from_slice(&f.to_be_bytes());
            }
            Amf3Value::String(s) => {
                self.buf.push(MARKER_STRING);
                self.write_amf_string(s);
            }
            Amf3Value::Date(ms) => {
                self.buf.push(MARKER_DATE);
                self.write_u29(1); // inline instance
                self.buf.extend_from_slice(&ms.to_be_bytes());
            }
            Amf3Value::Array(items) => {
                self.buf.push(MARKER_ARRAY);
                self.write_u29(((items.len() as u32) << 1) | 1);
                self.write_amf_string(""); // end of empty associative portion
                for item in items {
                    self.write_value(item);
                }
            }
            Amf3Value::Map(entries) => {
                self.buf.push(MARKER_OBJECT);
                // 0x0B = inline instance (bit0=1), inline traits (bit1=1), not externalizable (bit2=0),
                // dynamic=true (bit3=1), sealed_count=0 (bits 4..28 = 0).
                self.write_u29(0x0B);
                self.write_amf_string(""); // anonymous class name
                for (k, v) in entries {
                    if !k.is_empty() {
                        self.write_amf_string(k);
                        self.write_value(v);
                    }
                }
                self.write_amf_string(""); // dynamic members terminator
            }
            Amf3Value::ByteArray(raw) => {
                self.buf.push(MARKER_BYTE_ARRAY);
                self.write_u29(((raw.len() as u32) << 1) | 1);
                self.buf.extend_from_slice(raw);
            }
        }
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    pos: usize,
    string_refs: Vec<String>,
    object_refs: Vec<Amf3Value>,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            string_refs: Vec::new(),
            object_refs: Vec::new(),
        }
    }

    fn read_u8(&mut self) -> Result<u8, Amf3Error> {
        if self.pos >= self.bytes.len() {
            return Err(Amf3Error::UnexpectedEof);
        }
        let b = self.bytes[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], Amf3Error> {
        if self.pos + len > self.bytes.len() {
            return Err(Amf3Error::UnexpectedEof);
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_u29(&mut self) -> Result<u32, Amf3Error> {
        let mut acc: u32 = 0;
        for i in 0..3 {
            let b = self.read_u8()?;
            acc = (acc << 7) | u32::from(b & 0x7F);
            if (b & 0x80) == 0 {
                return Ok(acc);
            }
            let _ = i;
        }
        let b4 = self.read_u8()?;
        Ok((acc << 8) | u32::from(b4))
    }

    fn read_amf_string(&mut self) -> Result<String, Amf3Error> {
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            let idx = (header >> 1) as usize;
            return self
                .string_refs
                .get(idx)
                .cloned()
                .ok_or(Amf3Error::InvalidStringRef(idx));
        }
        let len = (header >> 1) as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let raw = self.read_exact(len)?;
        let s = std::str::from_utf8(raw)
            .map_err(|_| Amf3Error::InvalidUtf8)?
            .to_owned();
        self.string_refs.push(s.clone());
        Ok(s)
    }

    fn read_value(&mut self) -> Result<Amf3Value, Amf3Error> {
        let marker = self.read_u8()?;
        match marker {
            MARKER_UNDEFINED => Ok(Amf3Value::Undefined),
            MARKER_NULL => Ok(Amf3Value::Null),
            MARKER_FALSE => Ok(Amf3Value::Bool(false)),
            MARKER_TRUE => Ok(Amf3Value::Bool(true)),
            MARKER_INTEGER => {
                let u = self.read_u29()?;
                // Sign-extend 29-bit integer if bit 28 is set
                let signed = if (u & 0x1000_0000) != 0 {
                    (u | 0xE000_0000) as i32
                } else {
                    u as i32
                };
                Ok(Amf3Value::Int(i64::from(signed)))
            }
            MARKER_DOUBLE => {
                let raw = self.read_exact(8)?;
                let mut arr = [0u8; 8];
                arr.copy_from_slice(raw);
                Ok(Amf3Value::Double(f64::from_be_bytes(arr)))
            }
            MARKER_STRING => Ok(Amf3Value::String(self.read_amf_string()?)),
            MARKER_DATE => {
                let header = self.read_u29()?;
                if (header & 1) == 0 {
                    let idx = (header >> 1) as usize;
                    return self
                        .object_refs
                        .get(idx)
                        .cloned()
                        .ok_or(Amf3Error::InvalidObjectRef(idx));
                }
                let raw = self.read_exact(8)?;
                let mut arr = [0u8; 8];
                arr.copy_from_slice(raw);
                let val = Amf3Value::Date(f64::from_be_bytes(arr));
                self.object_refs.push(val.clone());
                Ok(val)
            }
            MARKER_ARRAY => {
                let header = self.read_u29()?;
                if (header & 1) == 0 {
                    let idx = (header >> 1) as usize;
                    return self
                        .object_refs
                        .get(idx)
                        .cloned()
                        .ok_or(Amf3Error::InvalidObjectRef(idx));
                }
                let dense_len = (header >> 1) as usize;
                let placeholder_idx = self.object_refs.len();
                self.object_refs.push(Amf3Value::Null);

                let mut assoc = BTreeMap::new();
                loop {
                    let key = self.read_amf_string()?;
                    if key.is_empty() {
                        break;
                    }
                    let val = self.read_value()?;
                    assoc.insert(key, val);
                }

                let mut dense = Vec::with_capacity(dense_len);
                for _ in 0..dense_len {
                    dense.push(self.read_value()?);
                }

                let result = if assoc.is_empty() {
                    Amf3Value::Array(dense)
                } else {
                    // Mixed associative + dense AMF3 array -> Lua 1-based keys merged into map
                    for (i, item) in dense.into_iter().enumerate() {
                        assoc.insert((i + 1).to_string(), item);
                    }
                    Amf3Value::Map(assoc)
                };
                self.object_refs[placeholder_idx] = result.clone();
                Ok(result)
            }
            MARKER_OBJECT => {
                let header = self.read_u29()?;
                if (header & 1) == 0 {
                    let idx = (header >> 1) as usize;
                    return self
                        .object_refs
                        .get(idx)
                        .cloned()
                        .ok_or(Amf3Error::InvalidObjectRef(idx));
                }
                if (header & 0b0110) != 0b0010 || (header & 0b1000) == 0 {
                    return Err(Amf3Error::UnsupportedSealedObjectTraits);
                }
                let sealed_count = (header >> 4) as usize;
                let _class_name = self.read_amf_string()?;

                let placeholder_idx = self.object_refs.len();
                self.object_refs.push(Amf3Value::Null);

                let mut sealed_names = Vec::with_capacity(sealed_count);
                for _ in 0..sealed_count {
                    sealed_names.push(self.read_amf_string()?);
                }
                let mut map = BTreeMap::new();
                for name in sealed_names {
                    let v = self.read_value()?;
                    map.insert(name, v);
                }
                loop {
                    let key = self.read_amf_string()?;
                    if key.is_empty() {
                        break;
                    }
                    let val = self.read_value()?;
                    map.insert(key, val);
                }
                let result = Amf3Value::Map(map);
                self.object_refs[placeholder_idx] = result.clone();
                Ok(result)
            }
            MARKER_BYTE_ARRAY => {
                let header = self.read_u29()?;
                if (header & 1) == 0 {
                    let idx = (header >> 1) as usize;
                    return self
                        .object_refs
                        .get(idx)
                        .cloned()
                        .ok_or(Amf3Error::InvalidObjectRef(idx));
                }
                let len = (header >> 1) as usize;
                let raw = self.read_exact(len)?.to_vec();
                let val = Amf3Value::ByteArray(raw);
                self.object_refs.push(val.clone());
                Ok(val)
            }
            other => Err(Amf3Error::UnsupportedMarker(other)),
        }
    }
}
