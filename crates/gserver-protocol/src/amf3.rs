//! Complete pure-Rust AMF3 encoder & decoder matching GServer `Amf3Module.cpp`
//! (`luaopen_amf3` / `libcocos2dlua.so` `0x2c6138`).

use std::collections::HashMap;

use crate::error::Amf3Error;

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

pub const AMF3_INT_MIN: i32 = -268_435_456;
pub const AMF3_INT_MAX: i32 = 268_435_455;
pub const AMF3_MAX_INLINE_LEN: usize = 0x0FFF_FFFF;

pub const DEFAULT_MAX_DEPTH: usize = 64;
pub const DEFAULT_MAX_STRING_OR_BYTES_LEN: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_COLLECTION_LEN: usize = 262_144;

/// Decoder safety and resource bounds configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeConfig {
    pub max_depth: usize,
    pub max_bytes_len: usize,
    pub max_collection_len: usize,
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
    pub class_name: String,
    pub dynamic: bool,
    pub externalizable: bool,
    pub sealed_members: Vec<String>,
}

/// AMF3 `Object` (`0x0A`) value supporting anonymous dynamic Lua maps, typed sealed traits,
/// and externalizable payloads.
#[derive(Debug, Clone, PartialEq)]
pub struct Amf3Object {
    pub class_name: String,
    pub dynamic: bool,
    pub sealed_members: Vec<(String, Value)>,
    pub dynamic_members: Vec<(String, Value)>,
    pub externalized: Option<Box<Value>>,
}

impl Default for Amf3Object {
    fn default() -> Self {
        Self::anonymous_dynamic()
    }
}

impl Amf3Object {
    pub fn anonymous_dynamic() -> Self {
        Self {
            class_name: String::new(),
            dynamic: true,
            sealed_members: Vec::new(),
            dynamic_members: Vec::new(),
            externalized: None,
        }
    }

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

    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        if let Some((_, existing)) = self.dynamic_members.iter_mut().find(|(k, _)| *k == key) {
            *existing = value;
        } else {
            self.dynamic_members.push((key, value));
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.sealed_members
            .iter()
            .chain(self.dynamic_members.iter())
            .map(|(k, v)| (k.as_str(), v))
    }

    pub fn trait_info(&self) -> TraitInfo {
        TraitInfo {
            class_name: self.class_name.clone(),
            dynamic: self.dynamic,
            externalizable: self.externalized.is_some(),
            sealed_members: self.sealed_members.iter().map(|(k, _)| k.clone()).collect(),
        }
    }
}

/// Complete AMF3 dynamic value representation for GServer client-server communication.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Int(i32),
    Double(f64),
    String(String),
    ByteString(Vec<u8>),
    XmlDoc(Vec<u8>),
    Date(f64),
    Array {
        dense: Vec<Value>,
        assoc: Vec<(String, Value)>,
    },
    Object(Amf3Object),
    Xml(Vec<u8>),
    ByteArray(Vec<u8>),
}

impl Value {
    pub fn empty_array() -> Self {
        Self::dense_array(Vec::new())
    }

    pub fn dense_array(dense: Vec<Value>) -> Self {
        Self::Array {
            dense,
            assoc: Vec::new(),
        }
    }

    pub fn map<I, K>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        Self::Object(Amf3Object::from_dynamic_entries(entries))
    }

    pub fn from_lua_number(n: f64) -> Self {
        if n.is_finite() && (f64::from(AMF3_INT_MIN)..=f64::from(AMF3_INT_MAX)).contains(&n) {
            let iv = n as i32;
            if f64::from(iv) == n && !(n == 0.0 && n.is_sign_negative()) {
                return Self::Int(iv);
            }
        }
        Self::Double(n)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            Self::ByteString(b) | Self::ByteArray(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::String(s) => Some(s.as_bytes()),
            Self::ByteString(b) | Self::ByteArray(b) | Self::Xml(b) | Self::XmlDoc(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(i64::from(*v)),
            Self::Double(d) if d.is_finite() && (*d as i64 as f64) == *d => Some(*d as i64),
            _ => None,
        }
    }

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
        Self::from(i64::from(v))
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

pub fn encode(value: &Value) -> Result<Vec<u8>, Amf3Error> {
    let mut encoder = Amf3Encoder::new();
    encoder.encode_value(value)?;
    Ok(encoder.into_bytes())
}

pub fn decode(bytes: &[u8]) -> Result<(Value, usize), Amf3Error> {
    decode_from(bytes, 0, DecodeConfig::default())
}

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
    pub fn new() -> Self {
        Self {
            out: Vec::with_capacity(128),
            string_refs: HashMap::new(),
            trait_refs: HashMap::new(),
            object_ref_count: 0,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.out
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.out
    }

    pub fn object_ref_count(&self) -> u32 {
        self.object_ref_count
    }

    pub fn encode_value(&mut self, value: &Value) -> Result<(), Amf3Error> {
        self.encode_value_at_depth(value, 0)
    }

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

    pub fn write_u29(&mut self, mut v: u32) {
        v &= 0x1FFF_FFFF;
        if v < 0x80 {
            self.out.push(v as u8);
        } else if v < 0x4000 {
            self.out
                .extend_from_slice(&[(((v >> 7) & 0x7F) | 0x80) as u8, (v & 0x7F) as u8]);
        } else if v < 0x20_0000 {
            self.out.extend_from_slice(&[
                (((v >> 14) & 0x7F) | 0x80) as u8,
                (((v >> 7) & 0x7F) | 0x80) as u8,
                (v & 0x7F) as u8,
            ]);
        } else {
            self.out.extend_from_slice(&[
                (((v >> 22) & 0x7F) | 0x80) as u8,
                (((v >> 15) & 0x7F) | 0x80) as u8,
                (((v >> 8) & 0x7F) | 0x80) as u8,
                (v & 0xFF) as u8,
            ]);
        }
    }

    #[inline]
    fn write_double_be(&mut self, d: f64) {
        self.out.extend_from_slice(&d.to_bits().to_be_bytes());
    }

    #[inline]
    fn check_depth(&self, depth: usize) -> Result<(), Amf3Error> {
        if depth > self.max_depth {
            return Err(Amf3Error::RecursionLimitExceeded {
                max_depth: self.max_depth,
                offset: self.out.len(),
            });
        }
        Ok(())
    }

    #[inline]
    fn check_inline_len(len: usize, limit: usize, kind: &'static str) -> Result<(), Amf3Error> {
        if len > limit {
            return Err(Amf3Error::LengthOverflow { kind, len });
        }
        Ok(())
    }

    fn write_utf8_vr(&mut self, bytes: &[u8]) -> Result<(), Amf3Error> {
        if bytes.is_empty() {
            self.out.push(0x01);
            return Ok(());
        }
        if let Some(&idx) = self.string_refs.get(bytes) {
            self.write_u29(idx << 1);
            return Ok(());
        }
        Self::check_inline_len(bytes.len(), AMF3_MAX_INLINE_LEN, "string")?;

        if let Ok(next_idx) = u32::try_from(self.string_refs.len()) {
            if (next_idx as usize) <= AMF3_MAX_INLINE_LEN {
                self.string_refs.insert(bytes.to_vec(), next_idx);
            }
        }
        self.write_u29(((bytes.len() as u32) << 1) | 1);
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    #[inline]
    fn register_inline_object(&mut self) {
        self.object_ref_count = self.object_ref_count.saturating_add(1);
    }

    fn encode_bytes_with_marker(
        &mut self,
        marker: u8,
        bytes: &[u8],
        kind: &'static str,
    ) -> Result<(), Amf3Error> {
        Self::check_inline_len(bytes.len(), AMF3_MAX_INLINE_LEN, kind)?;
        self.out.push(marker);
        self.register_inline_object();
        self.write_u29(((bytes.len() as u32) << 1) | 1);
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn encode_kv_pairs(
        &mut self,
        pairs: &[(String, Value)],
        next_depth: usize,
    ) -> Result<(), Amf3Error> {
        for (k, v) in pairs {
            if !k.is_empty() {
                self.write_utf8_vr(k.as_bytes())?;
                self.encode_value_at_depth(v, next_depth)?;
            }
        }
        self.out.push(0x01);
        Ok(())
    }

    fn encode_value_at_depth(&mut self, value: &Value, depth: usize) -> Result<(), Amf3Error> {
        match value {
            Value::Undefined => self.out.push(MARKER_UNDEFINED),
            Value::Null => self.out.push(MARKER_NULL),
            Value::Bool(false) => self.out.push(MARKER_FALSE),
            Value::Bool(true) => self.out.push(MARKER_TRUE),
            Value::Int(iv) if (AMF3_INT_MIN..=AMF3_INT_MAX).contains(iv) => {
                self.out.push(MARKER_INTEGER);
                self.write_u29((*iv as u32) & 0x1FFF_FFFF);
            }
            Value::Int(iv) => {
                self.out.push(MARKER_DOUBLE);
                self.write_double_be(f64::from(*iv));
            }
            Value::Double(d) => {
                self.out.push(MARKER_DOUBLE);
                self.write_double_be(*d);
            }
            Value::String(s) => {
                self.out.push(MARKER_STRING);
                return self.write_utf8_vr(s.as_bytes());
            }
            Value::ByteString(bytes) => {
                self.out.push(MARKER_STRING);
                return self.write_utf8_vr(bytes);
            }
            Value::XmlDoc(bytes) => {
                return self.encode_bytes_with_marker(MARKER_XML_DOC, bytes, "XmlDoc")
            }
            Value::Xml(bytes) => return self.encode_bytes_with_marker(MARKER_XML, bytes, "Xml"),
            Value::ByteArray(bytes) => {
                return self.encode_bytes_with_marker(MARKER_BYTE_ARRAY, bytes, "ByteArray")
            }
            Value::Date(ms) => {
                self.out.push(MARKER_DATE);
                self.register_inline_object();
                self.write_u29(1);
                self.write_double_be(*ms);
            }
            Value::Array { dense, assoc } => {
                self.check_depth(depth)?;
                Self::check_inline_len(dense.len(), AMF3_MAX_INLINE_LEN, "Array")?;
                self.out.push(MARKER_ARRAY);
                self.register_inline_object();
                self.write_u29(((dense.len() as u32) << 1) | 1);
                self.encode_kv_pairs(assoc, depth + 1)?;
                for item in dense {
                    self.encode_value_at_depth(item, depth + 1)?;
                }
            }
            Value::Object(obj) => {
                self.check_depth(depth)?;
                Self::check_inline_len(
                    obj.sealed_members.len(),
                    0x01FF_FFFF,
                    "Object sealed members",
                )?;
                self.out.push(MARKER_OBJECT);
                self.register_inline_object();

                let traits = obj.trait_info();
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
                    self.write_u29((trait_idx << 2) | 0x01);
                } else {
                    let header = 0x03u32
                        | (u32::from(traits.externalizable) << 2)
                        | (u32::from(traits.dynamic) << 3)
                        | ((traits.sealed_members.len() as u32) << 4);
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
                    self.encode_kv_pairs(&obj.dynamic_members, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

// ============================================================================
// Stateful AMF3 Decoder (with String, Object, and Trait Reference Tables)
// ============================================================================

enum ObjectHeader {
    Inline { header: u32, offset: usize },
    Ref(Value),
}

pub struct Amf3Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    config: DecodeConfig,
    string_refs: Vec<Vec<u8>>,
    object_refs: Vec<Option<Value>>,
    trait_refs: Vec<TraitInfo>,
}

impl<'a> Amf3Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_config(data, 0, DecodeConfig::default())
    }

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

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn decode_value(&mut self) -> Result<Value, Amf3Error> {
        self.decode_at_depth(0)
    }

    pub fn read_u29(&mut self) -> Result<u32, Amf3Error> {
        let mut result = 0u32;
        for _ in 0..3 {
            let b = self.read_byte()?;
            result = (result << 7) | u32::from(b & 0x7F);
            if (b & 0x80) == 0 {
                return Ok(result);
            }
        }
        Ok((result << 8) | u32::from(self.read_byte()?))
    }

    #[inline]
    fn check_limit(
        kind: &'static str,
        requested: usize,
        limit: usize,
        offset: usize,
    ) -> Result<(), Amf3Error> {
        if requested > limit {
            return Err(Amf3Error::AllocationLimitExceeded {
                kind,
                requested,
                limit,
                offset,
            });
        }
        Ok(())
    }

    #[inline]
    fn read_slice(&mut self, needed: usize) -> Result<&'a [u8], Amf3Error> {
        let remaining = self.data.len().saturating_sub(self.pos);
        if needed > self.data.len() || self.pos > self.data.len() - needed {
            return Err(Amf3Error::UnexpectedEof {
                offset: self.pos,
                needed,
                remaining,
            });
        }
        let s = &self.data[self.pos..self.pos + needed];
        self.pos += needed;
        Ok(s)
    }

    #[inline]
    fn read_byte(&mut self) -> Result<u8, Amf3Error> {
        Ok(self.read_slice(1)?[0])
    }

    fn read_double_be(&mut self) -> Result<f64, Amf3Error> {
        let s = self.read_slice(8)?;
        Ok(f64::from_bits(u64::from_be_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ])))
    }

    fn read_utf8_vr_bytes(&mut self) -> Result<Vec<u8>, Amf3Error> {
        let offset_before = self.pos;
        let header = self.read_u29()?;
        if (header & 1) == 0 {
            let idx = (header >> 1) as usize;
            return self
                .string_refs
                .get(idx)
                .cloned()
                .ok_or(Amf3Error::InvalidStringRef {
                    index: idx,
                    table_len: self.string_refs.len(),
                    offset: offset_before,
                });
        }

        let len = (header >> 1) as usize;
        if len == 0 {
            return Ok(Vec::new());
        }
        Self::check_limit("String", len, self.config.max_bytes_len, offset_before)?;
        let bytes = self.read_slice(len)?.to_vec();
        self.string_refs.push(bytes.clone());
        Ok(bytes)
    }

    fn read_key_string(&mut self) -> Result<String, Amf3Error> {
        let offset_before = self.pos;
        let raw = self.read_utf8_vr_bytes()?;
        match String::from_utf8(raw) {
            Ok(s) => Ok(s),
            Err(_) if self.config.strict_utf8 => Err(Amf3Error::InvalidUtf8 {
                offset: offset_before,
            }),
            Err(e) => Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()),
        }
    }

    fn read_object_header(&mut self) -> Result<ObjectHeader, Amf3Error> {
        let offset = self.pos;
        let header = self.read_u29()?;
        if (header & 1) != 0 {
            return Ok(ObjectHeader::Inline { header, offset });
        }
        let idx = (header >> 1) as usize;
        match self.object_refs.get(idx) {
            None => Err(Amf3Error::InvalidObjectRef {
                index: idx,
                table_len: self.object_refs.len(),
                offset,
            }),
            Some(None) => Err(Amf3Error::CyclicObjectRef { index: idx, offset }),
            Some(Some(val)) => Ok(ObjectHeader::Ref(val.clone())),
        }
    }

    fn reserve_object_slot(&mut self) -> usize {
        let slot = self.object_refs.len();
        self.object_refs.push(None);
        slot
    }

    fn finish_object_slot(&mut self, slot: usize, val: Value) -> Value {
        self.object_refs[slot] = Some(val.clone());
        val
    }

    fn decode_kv_pairs(
        &mut self,
        depth: usize,
        kind: &'static str,
    ) -> Result<Vec<(String, Value)>, Amf3Error> {
        let mut pairs = Vec::new();
        loop {
            let key = self.read_key_string()?;
            if key.is_empty() {
                return Ok(pairs);
            }
            Self::check_limit(kind, pairs.len() + 1, self.config.max_collection_len, self.pos)?;
            pairs.push((key, self.decode_at_depth(depth + 1)?));
        }
    }

    fn decode_bytes_container(
        &mut self,
        kind: &'static str,
        wrap: fn(Vec<u8>) -> Value,
    ) -> Result<Value, Amf3Error> {
        let (header, offset) = match self.read_object_header()? {
            ObjectHeader::Ref(v) => return Ok(v),
            ObjectHeader::Inline { header, offset } => (header, offset),
        };
        let len = (header >> 1) as usize;
        Self::check_limit(kind, len, self.config.max_bytes_len, offset)?;
        let bytes = self.read_slice(len)?.to_vec();
        let slot = self.reserve_object_slot();
        Ok(self.finish_object_slot(slot, wrap(bytes)))
    }

    fn decode_date(&mut self) -> Result<Value, Amf3Error> {
        if let ObjectHeader::Ref(v) = self.read_object_header()? {
            return Ok(v);
        }
        let slot = self.reserve_object_slot();
        let ms = self.read_double_be()?;
        Ok(self.finish_object_slot(slot, Value::Date(ms)))
    }

    fn decode_array(&mut self, depth: usize) -> Result<Value, Amf3Error> {
        let (header, offset) = match self.read_object_header()? {
            ObjectHeader::Ref(v) => return Ok(v),
            ObjectHeader::Inline { header, offset } => (header, offset),
        };
        let dense_count = (header >> 1) as usize;
        Self::check_limit("Array dense", dense_count, self.config.max_collection_len, offset)?;

        let slot = self.reserve_object_slot();
        let assoc = self.decode_kv_pairs(depth, "Array associative")?;
        let mut dense = Vec::with_capacity(dense_count.min(self.data.len().saturating_sub(self.pos)));
        for _ in 0..dense_count {
            dense.push(self.decode_at_depth(depth + 1)?);
        }
        Ok(self.finish_object_slot(slot, Value::Array { dense, assoc }))
    }

    fn decode_object(&mut self, depth: usize) -> Result<Value, Amf3Error> {
        let (header, offset) = match self.read_object_header()? {
            ObjectHeader::Ref(v) => return Ok(v),
            ObjectHeader::Inline { header, offset } => (header, offset),
        };

        let traits = if (header & 0x02) == 0 {
            let trait_idx = (header >> 2) as usize;
            self.trait_refs
                .get(trait_idx)
                .cloned()
                .ok_or(Amf3Error::InvalidTraitRef {
                    index: trait_idx,
                    table_len: self.trait_refs.len(),
                    offset,
                })?
        } else {
            let sealed_count = (header >> 4) as usize;
            Self::check_limit(
                "Object trait members",
                sealed_count,
                self.config.max_collection_len,
                offset,
            )?;
            let class_name = self.read_key_string()?;
            let mut sealed_members =
                Vec::with_capacity(sealed_count.min(self.data.len().saturating_sub(self.pos)));
            for _ in 0..sealed_count {
                sealed_members.push(self.read_key_string()?);
            }
            let info = TraitInfo {
                class_name,
                dynamic: (header & 0x08) != 0,
                externalizable: (header & 0x04) != 0,
                sealed_members,
            };
            self.trait_refs.push(info.clone());
            info
        };

        let slot = self.reserve_object_slot();
        if traits.externalizable {
            let ext_val = self.decode_at_depth(depth + 1)?;
            return Ok(self.finish_object_slot(
                slot,
                Value::Object(Amf3Object {
                    class_name: traits.class_name,
                    dynamic: traits.dynamic,
                    sealed_members: Vec::new(),
                    dynamic_members: Vec::new(),
                    externalized: Some(Box::new(ext_val)),
                }),
            ));
        }

        let mut sealed_members = Vec::with_capacity(traits.sealed_members.len());
        for member_name in traits.sealed_members {
            sealed_members.push((member_name, self.decode_at_depth(depth + 1)?));
        }
        let dynamic_members = if traits.dynamic {
            self.decode_kv_pairs(depth, "Object dynamic members")?
        } else {
            Vec::new()
        };

        Ok(self.finish_object_slot(
            slot,
            Value::Object(Amf3Object {
                class_name: traits.class_name,
                dynamic: traits.dynamic,
                sealed_members,
                dynamic_members,
                externalized: None,
            }),
        ))
    }

    fn decode_at_depth(&mut self, depth: usize) -> Result<Value, Amf3Error> {
        if depth > self.config.max_depth {
            return Err(Amf3Error::RecursionLimitExceeded {
                max_depth: self.config.max_depth,
                offset: self.pos,
            });
        }

        let marker_offset = self.pos;
        match self.read_byte()? {
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
            MARKER_STRING => match String::from_utf8(self.read_utf8_vr_bytes()?) {
                Ok(s) => Ok(Value::String(s)),
                Err(_) if self.config.strict_utf8 => Err(Amf3Error::InvalidUtf8 {
                    offset: marker_offset,
                }),
                Err(e) => Ok(Value::ByteString(e.into_bytes())),
            },
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
