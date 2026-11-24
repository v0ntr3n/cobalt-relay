//! Complete pure-Rust AMF3 encoder & decoder matching GServer `Amf3Module.cpp`
//! (`luaopen_amf3` / `libcocos2dlua.so` `0x2c6138`).
//!
//! Features:
//! - Full AMF3 `Value` representation covering all markers `0x00..=0x0C` (`Undefined`, `Null`,
//!   `Bool`, 29-bit signed `Int`, IEEE-754 BE `Double`, UTF-8 `String` + binary-safe `ByteString`
//!   for non-UTF-8 Lua `0x06` strings, `Date`, `XmlDoc`, `Xml`, `Array { dense, assoc }`,
//!   `Object`, and `ByteArray`).
//! - Complete AMF3 String, Object, and Trait reference tables (`stringRefs`, `objectRefs`,
//!   `traitRefs`), preserving empty-string (`0x01`) exclusion from `stringRefs`.
//! - Configurable recursion depth limit (default `64`, matching `Amf3Module.cpp`) and per-value
//!   allocation bounds against malformed/hostile inputs.

use std::collections::HashMap;

use crate::error::Amf3Error;

// AMF3 type markers (Adobe Action Message Format 3 specification)
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

/// Minimum 29-bit signed AMF3 integer (`-2^28 = -268,435,456`).
pub const AMF3_INT_MIN: i32 = -268_435_456;
/// Maximum 29-bit signed AMF3 integer (`2^28 - 1 = 268,435,455`).
pub const AMF3_INT_MAX: i32 = 268_435_455;
/// Maximum 28-bit inline length/count representable in `U29` (`(len << 1) | 1` fits in 29 bits).
pub const AMF3_MAX_INLINE_LEN: usize = 0x0FFF_FFFF;

/// Default maximum container nesting depth (matches `kMaxRecursionDepth = 64` in `Amf3Module.cpp`).
pub const DEFAULT_MAX_DEPTH: usize = 64;
/// Default maximum single string or ByteArray byte length during decoding (16 MiB).
pub const DEFAULT_MAX_STRING_OR_BYTES_LEN: usize = 16 * 1024 * 1024;
/// Default maximum collection element/member count during decoding (262,144).
pub const DEFAULT_MAX_COLLECTION_LEN: usize = 262_144;

/// Decoder safety and resource bounds configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeConfig {
    /// Maximum allowed recursion depth for nested Arrays/Objects (default: `64`).
    pub max_depth: usize,
    /// Maximum byte length for a single inline String, Xml, or ByteArray (default: `16 MiB`).
    pub max_bytes_len: usize,
    /// Maximum element or member count in a single Array or Object (default: `262,144`).
    pub max_collection_len: usize,
    /// If `true`, non-UTF-8 `0x06` strings fail with [`Amf3Error::InvalidUtf8`] instead of
    /// decoding into [`Value::ByteString`]. Defaults to `false` because GServer Lua 5.1 `multi=1`
    /// batches frequently embed nested binary AMF3 payloads inside `0x06` Lua strings.
    pub strict_utf8: bool,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_bytes_len: DEFAULT_MAX_STRING_OR_BYTES_LEN,
            max_collection_len: DEFAULT_MAX_COLLECTION_LEN,
            strict_utf8: false,
        }
    }
}

/// Trait metadata for an AMF3 `Object` (`0x0A`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitInfo {
    /// Registered ActionScript class name (`""` for anonymous Lua map objects).
    pub class_name: String,
    /// Whether dynamic key-value pairs follow the sealed members (`true` for Lua tables).
    pub dynamic: bool,
    /// Whether the object uses custom `IExternalizable` payload encoding.
    pub externalizable: bool,
    /// Ordered list of sealed property names declared in the trait.
    pub sealed_members: Vec<String>,
}

/// AMF3 `Object` (`0x0A`) value supporting anonymous dynamic Lua maps, typed sealed traits,
/// and externalizable payloads.
#[derive(Debug, Clone, PartialEq)]
pub struct Amf3Object {
    /// Class name (`""` for anonymous objects emitted by `Amf3Module.cpp`).
    pub class_name: String,
    /// Whether dynamic properties are enabled.
    pub dynamic: bool,
    /// Sealed properties in trait declaration order.
    pub sealed_members: Vec<(String, Value)>,
    /// Dynamic key-value properties (preserved in insertion order for deterministic encoding).
    pub dynamic_members: Vec<(String, Value)>,
    /// Optional `IExternalizable` inner value (`_external` in `Amf3Module.cpp`).
    pub externalized: Option<Box<Value>>,
}

impl Default for Amf3Object {
    fn default() -> Self {
        Self::anonymous_dynamic()
    }
}

impl Amf3Object {
    /// Creates an empty anonymous dynamic AMF3 Object (`class_name = ""`, `dynamic = true`),
    /// matching how `Amf3Module.cpp` encodes string-keyed Lua tables (`0x0A 0x0B 0x01 ... 0x01`).
    pub fn anonymous_dynamic() -> Self {
        Self {
            class_name: String::new(),
            dynamic: true,
            sealed_members: Vec::new(),
            dynamic_members: Vec::new(),
            externalized: None,
        }
    }

    /// Constructs an anonymous dynamic AMF3 Object from key-value pairs.
    pub fn from_dynamic_entries<I, K>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        let mut obj = Self::anonymous_dynamic();
        for (k, v) in entries {
            obj.insert(k.into(), v);
        }
        obj
    }

    /// Inserts or updates a dynamic property (preserving order on overwrite).
    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        if let Some((_, existing)) = self.dynamic_members.iter_mut().find(|(k, _)| *k == key) {
            *existing = value;
        } else {
            self.dynamic_members.push((key, value));
        }
    }

    /// Looks up a property by name across sealed members first, then dynamic members.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.sealed_members
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .or_else(|| {
                self.dynamic_members
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v)
            })
    }

    /// Returns an iterator over all `(key, value)` pairs (sealed followed by dynamic).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.sealed_members
            .iter()
            .chain(self.dynamic_members.iter())
            .map(|(k, v)| (k.as_str(), v))
    }

    /// Builds the [`TraitInfo`] descriptor for this object.
    pub fn trait_info(&self) -> TraitInfo {
        TraitInfo {
            class_name: self.class_name.clone(),
            dynamic: self.dynamic,
            externalizable: self.externalized.is_some(),
            sealed_members: self
                .sealed_members
                .iter()
                .map(|(k, _)| k.clone())
                .collect(),
        }
    }
}

/// Complete AMF3 dynamic value representation for GServer client-server communication.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `0x00` Undefined (decodes to `nil` in Lua client).
    Undefined,
    /// `0x01` Null (decodes to `nil` in Lua client).
    Null,
    /// `0x02` False / `0x03` True.
    Bool(bool),
    /// `0x04` 29-bit signed integer (`[-268_435_456, 268_435_455]`). Out-of-range `i32` values
    /// automatically promote to `0x05` `Double` during encoding, matching `Amf3Module.cpp`.
    Int(i32),
    /// `0x05` IEEE-754 64-bit big-endian floating-point number.
    Double(f64),
    /// `0x06` UTF-8 string (`UTF-8-vr`).
    String(String),
    /// `0x06` Binary-safe string (`UTF-8-vr` containing arbitrary raw bytes, used by Lua 5.1 /
    /// PHP `amfext` when embedding nested AMF3 payloads inside `multi=1` `ret` arrays).
    ByteString(Vec<u8>),
    /// `0x07` Legacy XMLDocument bytes (stored in `objectRefs`).
    XmlDoc(Vec<u8>),
    /// `0x08` Date in milliseconds since Unix epoch (stored in `objectRefs`).
    Date(f64),
    /// `0x09` Dense + associative Array (`{}` empty Lua table encodes as `dense: [], assoc: []`).
    Array {
        dense: Vec<Value>,
        assoc: Vec<(String, Value)>,
    },
    /// `0x0A` Object (anonymous dynamic Lua maps, typed sealed traits, or externalizable objects).
    Object(Amf3Object),
    /// `0x0B` E4X XML bytes (stored in `objectRefs`).
    Xml(Vec<u8>),
    /// `0x0C` Raw binary `ByteArray` (stored in `objectRefs`).
    ByteArray(Vec<u8>),
}

impl Value {
    /// Creates an empty dense AMF3 Array (`0x09 0x01 0x01`), matching `amf3.encode({})` in Lua.
    pub fn empty_array() -> Self {
        Self::Array {
            dense: Vec::new(),
            assoc: Vec::new(),
        }
    }

    /// Creates a pure dense AMF3 Array (`0x09`) from a `Vec<Value>`.
    pub fn dense_array(dense: Vec<Value>) -> Self {
        Self::Array {
            dense,
            assoc: Vec::new(),
        }
    }

    /// Creates an anonymous dynamic AMF3 Object (`0x0A`) from key-value pairs.
    pub fn map<I, K>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        Self::Object(Amf3Object::from_dynamic_entries(entries))
    }

    /// Encodes a Lua number using `Amf3Module.cpp`'s exact integer-vs-double promotion rule:
    /// finite integral values in `[AMF3_INT_MIN, AMF3_INT_MAX]` (excluding `-0.0`) become
    /// `Value::Int`, while larger integers (like Unix timestamps > `2^28 - 1`), fractions, `-0.0`,
    /// `NaN`, and `±Inf` become `Value::Double`.
    pub fn from_lua_number(n: f64) -> Self {
        if n.is_finite() && n >= f64::from(AMF3_INT_MIN) && n <= f64::from(AMF3_INT_MAX) {
            let iv = n as i32;
            if f64::from(iv) == n && !(n == 0.0 && n.is_sign_negative()) {
                return Self::Int(iv);
            }
        }
        Self::Double(n)
    }

    /// Convenience helper returning `&str` if `self` is `Value::String` (or UTF-8 `ByteString`/`ByteArray`).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            Self::ByteString(b) | Self::ByteArray(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    /// Convenience helper returning raw bytes for `String`, `ByteString`, or `ByteArray`
    /// (used when `Network.lua` treats Lua strings and ByteArrays interchangeably in `multi=1`).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::String(s) => Some(s.as_bytes()),
            Self::ByteString(b) | Self::ByteArray(b) | Self::Xml(b) | Self::XmlDoc(b) => Some(b),
            _ => None,
        }
    }

    /// Convenience helper extracting an integer (`i64`) from either `Value::Int` or integral `Value::Double`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(i64::from(*v)),
            Self::Double(d) if d.is_finite() && (*d as i64 as f64) == *d => Some(*d as i64),
            _ => None,
        }
    }

    /// Looks up a string key on either an `Object` or the associative part of an `Array`.
    pub fn get_field(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(obj) => obj.get(key),
            Self::Array { assoc, .. } => assoc.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        if (AMF3_INT_MIN..=AMF3_INT_MAX).contains(&v) {
            Self::Int(v)
        } else {
            Self::Double(f64::from(v))
        }
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        if let Ok(iv) = i32::try_from(v) {
            if (AMF3_INT_MIN..=AMF3_INT_MAX).contains(&iv) {
                return Self::Int(iv);
            }
        }
        Self::Double(v as f64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::Double(v)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

// ============================================================================
// Top-level Convenience Functions
// ============================================================================

/// Encodes an AMF3 [`Value`] into a newly allocated byte vector.
pub fn encode(value: &Value) -> Result<Vec<u8>, Amf3Error> {
    let mut encoder = Amf3Encoder::new();
    encoder.encode_value(value)?;
    Ok(encoder.into_bytes())
}

/// Decodes a single AMF3 [`Value`] from the beginning of `bytes` using [`DecodeConfig::default`],
/// returning `(value, bytes_consumed)`.
pub fn decode(bytes: &[u8]) -> Result<(Value, usize), Amf3Error> {
    decode_from(bytes, 0, DecodeConfig::default())
}

/// Decodes a single AMF3 [`Value`] starting at `offset` (matching `amf3.decode(str, pos)`),
/// returning `(value, new_offset)`.
pub fn decode_from(
    bytes: &[u8],
    offset: usize,
    config: DecodeConfig,
) -> Result<(Value, usize), Amf3Error> {
    let mut decoder = Amf3Decoder::with_config(bytes, offset, config);
    let val = decoder.decode_value()?;
    Ok((val, decoder.position()))
}

// ============================================================================
// Stateful AMF3 Encoder (with String, Trait, and Object Reference Tables)
// ============================================================================

/// Stateful AMF3 encoder maintaining `stringRefs`, `traitRefs`, and `objectRefs` tables.
pub struct Amf3Encoder {
    out: Vec<u8>,
    string_refs: HashMap<Vec<u8>, u32>,
    trait_refs: HashMap<TraitInfo, u32>,
    object_ref_count: u32,
    max_depth: usize,
}

impl Default for Amf3Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Amf3Encoder {
    /// Creates a new AMF3 encoder with default depth limit (`64`).
    pub fn new() -> Self {
        Self {
            out: Vec::with_capacity(128),
            string_refs: HashMap::new(),
            trait_refs: HashMap::new(),
            object_ref_count: 0,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    /// Overrides the maximum container nesting depth.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Consumes the encoder and returns the encoded AMF3 byte buffer.
    pub fn into_bytes(self) -> Vec<u8> {
        self.out
    }

    /// Borrows the encoded AMF3 bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.out
    }

    /// Current number of registered entries in `objectRefs`.
    pub fn object_ref_count(&self) -> u32 {
        self.object_ref_count
    }

    /// Encodes a top-level or nested AMF3 [`Value`] into the internal buffer.
    pub fn encode_value(&mut self, value: &Value) -> Result<(), Amf3Error> {
        self.encode_value_at_depth(value, 0)
    }

    /// Explicitly emits an AMF3 object reference (`[marker][U29(ref_index << 1)]`) to a
    /// previously encoded `Array`, `Object`, `ByteArray`, `Date`, `Xml`, or `XmlDoc`.
    pub fn encode_object_ref(&mut self, marker: u8, ref_index: u32) -> Result<(), Amf3Error> {
        if ref_index >= self.object_ref_count {
            return Err(Amf3Error::InvalidObjectRef {
                index: ref_index as usize,
                table_len: self.object_ref_count as usize,
                offset: self.out.len(),
            });
        }
        self.out.push(marker);
        self.write_u29(ref_index << 1);
        Ok(())
    }

    /// Writes a raw unsigned 29-bit variable-length integer (`U29`).
    pub fn write_u29(&mut self, mut v: u32) {
        v &= 0x1FFF_FFFF;
        if v < 0x80 {
            self.out.push(v as u8);
        } else if v < 0x4000 {
            self.out.push((((v >> 7) & 0x7F) | 0x80) as u8);
            self.out.push((v & 0x7F) as u8);
        } else if v < 0x20_0000 {
            self.out.push((((v >> 14) & 0x7F) | 0x80) as u8);
            self.out.push((((v >> 7) & 0x7F) | 0x80) as u8);
            self.out.push((v & 0x7F) as u8);
        } else {
            self.out.push((((v >> 22) & 0x7F) | 0x80) as u8);
            self.out.push((((v >> 15) & 0x7F) | 0x80) as u8);
            self.out.push((((v >> 8) & 0x7F) | 0x80) as u8);
            self.out.push((v & 0xFF) as u8);
        }
    }

    fn write_double_be(&mut self, d: f64) {
        self.out.extend_from_slice(&d.to_bits().to_be_bytes());
    }

    fn write_utf8_vr(&mut self, bytes: &[u8]) -> Result<(), Amf3Error> {
        // Empty string is ALWAYS 0x01 and NEVER inserted into stringRefs.
        if bytes.is_empty() {
            self.out.push(0x01);
            return Ok(());
        }

        if let Some(&idx) = self.string_refs.get(bytes) {
            self.write_u29(idx << 1);
            return Ok(());
        }

        if bytes.len() > AMF3_MAX_INLINE_LEN {
            return Err(Amf3Error::LengthOverflow {
                kind: "string",
                len: bytes.len(),
            });
        }

        if let Ok(next_idx) = u32::try_from(self.string_refs.len()) {
            if (next_idx as usize) <= AMF3_MAX_INLINE_LEN {
                self.string_refs.insert(bytes.to_vec(), next_idx);
            }
        }

        self.write_u29(((bytes.len() as u32) << 1) | 1);
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn register_inline_object(&mut self) {
        self.object_ref_count = self.object_ref_count.saturating_add(1);
    }

    fn encode_bytes_with_marker(
        &mut self,
        marker: u8,
        bytes: &[u8],
        kind: &'static str,
    ) -> Result<(), Amf3Error> {
        if bytes.len() > AMF3_MAX_INLINE_LEN {
            return Err(Amf3Error::LengthOverflow {
                kind,
                len: bytes.len(),
            });
        }
        self.out.push(marker);
        self.register_inline_object();
        self.write_u29(((bytes.len() as u32) << 1) | 1);
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn encode_value_at_depth(&mut self, value: &Value, depth: usize) -> Result<(), Amf3Error> {
        match value {
            Value::Undefined => {
                self.out.push(MARKER_UNDEFINED);
                Ok(())
            }
            Value::Null => {
                self.out.push(MARKER_NULL);
                Ok(())
            }
            Value::Bool(false) => {
                self.out.push(MARKER_FALSE);
                Ok(())
            }
            Value::Bool(true) => {
                self.out.push(MARKER_TRUE);
                Ok(())
            }
            Value::Int(iv) => {
                if (*iv >= AMF3_INT_MIN) && (*iv <= AMF3_INT_MAX) {
                    self.out.push(MARKER_INTEGER);
                    self.write_u29((*iv as u32) & 0x1FFF_FFFF);
                } else {
                    self.out.push(MARKER_DOUBLE);
                    self.write_double_be(f64::from(*iv));
                }
                Ok(())
            }
            Value::Double(d) => {
                self.out.push(MARKER_DOUBLE);
                self.write_double_be(*d);
                Ok(())
            }
            Value::String(s) => {
                self.out.push(MARKER_STRING);
                self.write_utf8_vr(s.as_bytes())
            }
            Value::ByteString(bytes) => {
                self.out.push(MARKER_STRING);
                self.write_utf8_vr(bytes)
            }
            Value::XmlDoc(bytes) => self.encode_bytes_with_marker(MARKER_XML_DOC, bytes, "XmlDoc"),
            Value::Xml(bytes) => self.encode_bytes_with_marker(MARKER_XML, bytes, "Xml"),
            Value::ByteArray(bytes) => {
                self.encode_bytes_with_marker(MARKER_BYTE_ARRAY, bytes, "ByteArray")
            }
            Value::Date(ms) => {
                self.out.push(MARKER_DATE);
                self.register_inline_object();
                self.write_u29(1); // inline Date flag
                self.write_double_be(*ms);
                Ok(())
            }
            Value::Array { dense, assoc } => {
                if depth > self.max_depth {
                    return Err(Amf3Error::RecursionLimitExceeded {
                        max_depth: self.max_depth,
                        offset: self.out.len(),
                    });
                }
                if dense.len() > AMF3_MAX_INLINE_LEN {
                    return Err(Amf3Error::LengthOverflow {
                        kind: "Array",
                        len: dense.len(),
                    });
                }
                self.out.push(MARKER_ARRAY);
                self.register_inline_object();
                self.write_u29(((dense.len() as u32) << 1) | 1);

                for (k, v) in assoc {
                    if !k.is_empty() {
                        self.write_utf8_vr(k.as_bytes())?;
                        self.encode_value_at_depth(v, depth + 1)?;
                    }
                }
                self.out.push(0x01); // empty string terminates associative portion

                for item in dense {
                    self.encode_value_at_depth(item, depth + 1)?;
                }
                Ok(())
            }
            Value::Object(obj) => {
                if depth > self.max_depth {
                    return Err(Amf3Error::RecursionLimitExceeded {
                        max_depth: self.max_depth,
                        offset: self.out.len(),
                    });
                }
                if obj.sealed_members.len() > 0x01FF_FFFF {
                    return Err(Amf3Error::LengthOverflow {
                        kind: "Object sealed members",
                        len: obj.sealed_members.len(),
                    });
                }

                self.out.push(MARKER_OBJECT);
                self.register_inline_object();

                let traits = obj.trait_info();
                // Note: Amf3Module.cpp always writes 0x0B + 0x01 inline for anonymous dynamic
                // objects; for named or sealed traits we deduplicate via `trait_refs`.
                let is_anon_dynamic = traits.class_name.is_empty()
                    && traits.dynamic
                    && !traits.externalizable
                    && traits.sealed_members.is_empty();

                if is_anon_dynamic {
                    self.write_u29(0x0B);
                    self.out.push(0x01);
                    if self.trait_refs.len() <= (AMF3_MAX_INLINE_LEN >> 1) {
                        let next = self.trait_refs.len() as u32;
                        self.trait_refs.entry(traits).or_insert(next);
                    }
                } else if let Some(&trait_idx) = self.trait_refs.get(&traits) {
                    // bit0 = 1 (inline object), bit1 = 0 (trait reference)
                    self.write_u29((trait_idx << 2) | 0x01);
                } else {
                    let mut header = 0x03u32; // inline object + inline traits
                    if traits.externalizable {
                        header |= 0x04;
                    }
                    if traits.dynamic {
                        header |= 0x08;
                    }
                    header |= (traits.sealed_members.len() as u32) << 4;
                    self.write_u29(header);
                    self.write_utf8_vr(traits.class_name.as_bytes())?;
                    for member in &traits.sealed_members {
                        self.write_utf8_vr(member.as_bytes())?;
                    }
                    if let Ok(next_idx) = u32::try_from(self.trait_refs.len()) {
                        self.trait_refs.insert(traits, next_idx);
                    }
                }

                if let Some(ext) = &obj.externalized {
                    return self.encode_value_at_depth(ext, depth + 1);
                }

                for (_, val) in &obj.sealed_members {
                    self.encode_value_at_depth(val, depth + 1)?;
                }

                if obj.dynamic {
                    for (k, val) in &obj.dynamic_members {
                        if !k.is_empty() {
                            self.write_utf8_vr(k.as_bytes())?;
                            self.encode_value_at_depth(val, depth + 1)?;
                        }
                    }
                    self.out.push(0x01); // terminate dynamic members
                }
                Ok(())
            }
        }
    }
}

// ============================================================================
// Stateful AMF3 Decoder (with String, Object, and Trait Reference Tables)
// ============================================================================

/// Stateful AMF3 decoder maintaining `stringRefs`, `objectRefs`, and `traitRefs` tables.
pub struct Amf3Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    config: DecodeConfig,
    string_refs: Vec<Vec<u8>>,
    object_refs: Vec<Option<Value>>,
    trait_refs: Vec<TraitInfo>,
}

impl<'a> Amf3Decoder<'a> {
    /// Creates a new AMF3 decoder starting at byte offset `0` with default safety bounds.
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_config(data, 0, DecodeConfig::default())
    }

    /// Creates a new AMF3 decoder starting at `offset` with explicit [`DecodeConfig`].
    pub fn with_config(data: &'a [u8], offset: usize, config: DecodeConfig) -> Self {
        Self {
            data,
            pos: offset,
            config,
            string_refs: Vec::new(),
            object_refs: Vec::new(),
            trait_refs: Vec::new(),
        }
    }

    /// Current byte cursor offset within the input slice.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Decodes a single AMF3 [`Value`] from the current cursor position.
    pub fn decode_value(&mut self) -> Result<Value, Amf3Error> {
        self.decode_at_depth(0)
    }

    /// Reads an unsigned 29-bit variable-length AMF3 integer (`U29`).
    pub fn read_u29(&mut self) -> Result<u32, Amf3Error> {
        let mut result = 0u32;
        for _ in 0..3 {
            let b = self.read_byte()?;
            result = (result << 7) | u32::from(b & 0x7F);
            if (b & 0x80) == 0 {
                return Ok(result);
            }
        }
        let last = self.read_byte()?;
        Ok((result << 8) | u32::from(last))
    }

    #[inline]
    fn ensure_available(&self, needed: usize) -> Result<(), Amf3Error> {
        let remaining = self.data.len().saturating_sub(self.pos);
        if needed > self.data.len() || self.pos > self.data.len() - needed {
            return Err(Amf3Error::UnexpectedEof {
                offset: self.pos,
                needed,
                remaining,
            });
        }
        Ok(())
    }

    #[inline]
    fn read_byte(&mut self) -> Result<u8, Amf3Error> {
        self.ensure_available(1)?;
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_double_be(&mut self) -> Result<f64, Amf3Error> {
        self.ensure_available(8)?;
        let s = &self.data[self.pos..self.pos + 8];
        self.pos += 8;
        let bits = u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]);
        Ok(f64::from_bits(bits))
    }

    fn read_utf8_vr_bytes(&mut self) -> Result<Vec<u8>, Amf3Error> {
        let offset_before = self.pos;
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            let idx = (header >> 1) as usize;
            return self.string_refs.get(idx).cloned().ok_or(Amf3Error::InvalidStringRef {
                index: idx,
                table_len: self.string_refs.len(),
                offset: offset_before,
            });
        }

        let len = (header >> 1) as usize;
        if len == 0 {
            return Ok(Vec::new());
        }
        if len > self.config.max_bytes_len {
            return Err(Amf3Error::AllocationLimitExceeded {
                kind: "String",
                requested: len,
                limit: self.config.max_bytes_len,
                offset: offset_before,
            });
        }

        self.ensure_available(len)?;
        let bytes = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        self.string_refs.push(bytes.clone());
        Ok(bytes)
    }

    fn read_key_string(&mut self) -> Result<String, Amf3Error> {
        let offset_before = self.pos;
        let raw = self.read_utf8_vr_bytes()?;
        match String::from_utf8(raw) {
            Ok(s) => Ok(s),
            Err(e) => {
                if self.config.strict_utf8 {
                    Err(Amf3Error::InvalidUtf8 {
                        offset: offset_before,
                    })
                } else {
                    Ok(String::from_utf8_lossy(e.as_bytes()).into_owned())
                }
            }
        }
    }

    fn resolve_object_ref(&self, idx: usize, offset: usize) -> Result<Value, Amf3Error> {
        match self.object_refs.get(idx) {
            None => Err(Amf3Error::InvalidObjectRef {
                index: idx,
                table_len: self.object_refs.len(),
                offset,
            }),
            Some(None) => Err(Amf3Error::CyclicObjectRef { index: idx, offset }),
            Some(Some(val)) => Ok(val.clone()),
        }
    }

    fn reserve_object_slot(&mut self) -> usize {
        let slot = self.object_refs.len();
        self.object_refs.push(None);
        slot
    }

    fn finish_object_slot(&mut self, slot: usize, val: &Value) {
        self.object_refs[slot] = Some(val.clone());
    }

    fn decode_bytes_container(
        &mut self,
        kind: &'static str,
        wrap: fn(Vec<u8>) -> Value,
    ) -> Result<Value, Amf3Error> {
        let offset_before = self.pos;
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            return self.resolve_object_ref((header >> 1) as usize, offset_before);
        }
        let len = (header >> 1) as usize;
        if len > self.config.max_bytes_len {
            return Err(Amf3Error::AllocationLimitExceeded {
                kind,
                requested: len,
                limit: self.config.max_bytes_len,
                offset: offset_before,
            });
        }
        self.ensure_available(len)?;
        let slot = self.reserve_object_slot();
        let bytes = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        let val = wrap(bytes);
        self.finish_object_slot(slot, &val);
        Ok(val)
    }

    fn decode_date(&mut self) -> Result<Value, Amf3Error> {
        let offset_before = self.pos;
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            return self.resolve_object_ref((header >> 1) as usize, offset_before);
        }
        let slot = self.reserve_object_slot();
        let ms = self.read_double_be()?;
        let val = Value::Date(ms);
        self.finish_object_slot(slot, &val);
        Ok(val)
    }

    fn decode_array(&mut self, depth: usize) -> Result<Value, Amf3Error> {
        let offset_before = self.pos;
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            return self.resolve_object_ref((header >> 1) as usize, offset_before);
        }

        let dense_count = (header >> 1) as usize;
        if dense_count > self.config.max_collection_len {
            return Err(Amf3Error::AllocationLimitExceeded {
                kind: "Array dense",
                requested: dense_count,
                limit: self.config.max_collection_len,
                offset: offset_before,
            });
        }

        let slot = self.reserve_object_slot();
        let mut assoc = Vec::new();
        loop {
            let key = self.read_key_string()?;
            if key.is_empty() {
                break;
            }
            if assoc.len() >= self.config.max_collection_len {
                return Err(Amf3Error::AllocationLimitExceeded {
                    kind: "Array associative",
                    requested: assoc.len() + 1,
                    limit: self.config.max_collection_len,
                    offset: self.pos,
                });
            }
            let val = self.decode_at_depth(depth + 1)?;
            assoc.push((key, val));
        }

        // Pre-allocate conservatively up to remaining input bytes to prevent malicious header OOM
        let mut dense = Vec::with_capacity(dense_count.min(self.data.len().saturating_sub(self.pos)));
        for _ in 0..dense_count {
            dense.push(self.decode_at_depth(depth + 1)?);
        }

        let val = Value::Array { dense, assoc };
        self.finish_object_slot(slot, &val);
        Ok(val)
    }

    fn decode_object(&mut self, depth: usize) -> Result<Value, Amf3Error> {
        let offset_before = self.pos;
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            return self.resolve_object_ref((header >> 1) as usize, offset_before);
        }

        let traits = if (header & 0x02) == 0 {
            let trait_idx = (header >> 2) as usize;
            self.trait_refs
                .get(trait_idx)
                .cloned()
                .ok_or(Amf3Error::InvalidTraitRef {
                    index: trait_idx,
                    table_len: self.trait_refs.len(),
                    offset: offset_before,
                })?
        } else {
            let externalizable = (header & 0x04) != 0;
            let dynamic = (header & 0x08) != 0;
            let sealed_count = (header >> 4) as usize;
            if sealed_count > self.config.max_collection_len {
                return Err(Amf3Error::AllocationLimitExceeded {
                    kind: "Object trait members",
                    requested: sealed_count,
                    limit: self.config.max_collection_len,
                    offset: offset_before,
                });
            }

            let class_name = self.read_key_string()?;
            let mut sealed_members =
                Vec::with_capacity(sealed_count.min(self.data.len().saturating_sub(self.pos)));
            for _ in 0..sealed_count {
                sealed_members.push(self.read_key_string()?);
            }

            let info = TraitInfo {
                class_name,
                dynamic,
                externalizable,
                sealed_members,
            };
            self.trait_refs.push(info.clone());
            info
        };

        let slot = self.reserve_object_slot();

        if traits.externalizable {
            // Matches `Amf3Module.cpp`: decodes a single inner value into `_external`.
            let ext_val = self.decode_at_depth(depth + 1)?;
            let val = Value::Object(Amf3Object {
                class_name: traits.class_name,
                dynamic: traits.dynamic,
                sealed_members: Vec::new(),
                dynamic_members: Vec::new(),
                externalized: Some(Box::new(ext_val)),
            });
            self.finish_object_slot(slot, &val);
            return Ok(val);
        }

        let mut sealed_members = Vec::with_capacity(traits.sealed_members.len());
        for member_name in traits.sealed_members {
            let member_val = self.decode_at_depth(depth + 1)?;
            sealed_members.push((member_name, member_val));
        }

        let mut dynamic_members = Vec::new();
        if traits.dynamic {
            loop {
                let key = self.read_key_string()?;
                if key.is_empty() {
                    break;
                }
                if dynamic_members.len() >= self.config.max_collection_len {
                    return Err(Amf3Error::AllocationLimitExceeded {
                        kind: "Object dynamic members",
                        requested: dynamic_members.len() + 1,
                        limit: self.config.max_collection_len,
                        offset: self.pos,
                    });
                }
                let val = self.decode_at_depth(depth + 1)?;
                dynamic_members.push((key, val));
            }
        }

        let val = Value::Object(Amf3Object {
            class_name: traits.class_name,
            dynamic: traits.dynamic,
            sealed_members,
            dynamic_members,
            externalized: None,
        });
        self.finish_object_slot(slot, &val);
        Ok(val)
    }

    fn decode_at_depth(&mut self, depth: usize) -> Result<Value, Amf3Error> {
        if depth > self.config.max_depth {
            return Err(Amf3Error::RecursionLimitExceeded {
                max_depth: self.config.max_depth,
                offset: self.pos,
            });
        }

        let marker_offset = self.pos;
        let marker = self.read_byte()?;

        match marker {
            MARKER_UNDEFINED => Ok(Value::Undefined),
            MARKER_NULL => Ok(Value::Null),
            MARKER_FALSE => Ok(Value::Bool(false)),
            MARKER_TRUE => Ok(Value::Bool(true)),
            MARKER_INTEGER => {
                let raw = self.read_u29()?;
                let signed = if (raw & 0x1000_0000) != 0 {
                    (raw | 0xE000_0000) as i32
                } else {
                    raw as i32
                };
                Ok(Value::Int(signed))
            }
            MARKER_DOUBLE => Ok(Value::Double(self.read_double_be()?)),
            MARKER_STRING => {
                let raw = self.read_utf8_vr_bytes()?;
                match String::from_utf8(raw) {
                    Ok(s) => Ok(Value::String(s)),
                    Err(e) => {
                        if self.config.strict_utf8 {
                            Err(Amf3Error::InvalidUtf8 {
                                offset: marker_offset,
                            })
                        } else {
                            Ok(Value::ByteString(e.into_bytes()))
                        }
                    }
                }
            }
            MARKER_XML_DOC => self.decode_bytes_container("XmlDoc", Value::XmlDoc),
            MARKER_DATE => self.decode_date(),
            MARKER_ARRAY => self.decode_array(depth),
            MARKER_OBJECT => self.decode_object(depth),
            MARKER_XML => self.decode_bytes_container("Xml", Value::Xml),
            MARKER_BYTE_ARRAY => self.decode_bytes_container("ByteArray", Value::ByteArray),
            other => Err(Amf3Error::InvalidMarker {
                marker: other,
                offset: marker_offset,
            }),
        }
    }
}
