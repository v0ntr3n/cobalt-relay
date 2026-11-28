use std::collections::BTreeMap;

use gserver_protocol::json::{parse_json, JsonNumber, JsonObject, JsonValue as ProtoJson};

use crate::error::{PlayerError, Result};

/// Deterministic RFC 8259 JSON value AST backed by `gserver_protocol::json`.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn empty_object() -> Self { JsonValue::Object(BTreeMap::new()) }
    pub fn empty_array() -> Self { JsonValue::Array(Vec::new()) }

    pub fn parse(input: &str) -> Result<Self> {
        parse_json(input).map(Self::from_proto).map_err(|e| PlayerError::Json(e.to_string()))
    }

    pub fn to_json_string(&self) -> String { self.to_proto().to_json_string() }

    pub fn to_json_pretty(&self) -> String {
        let mut out = self.to_proto().to_json_pretty();
        out.push('\n');
        out
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let JsonValue::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            JsonValue::UInt(n) => Some(*n),
            JsonValue::Int(n) if *n >= 0 => Some(*n as u64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Int(n) => Some(*n),
            JsonValue::UInt(n) => i64::try_from(*n).ok(),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Float(f) => Some(*f),
            JsonValue::Int(n) => Some(*n as f64),
            JsonValue::UInt(n) => Some(*n as f64),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let JsonValue::Str(s) = self { Some(s.as_str()) } else { None }
    }
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        if let JsonValue::Array(a) = self { Some(a.as_slice()) } else { None }
    }
    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        if let JsonValue::Object(m) = self { Some(m) } else { None }
    }
    pub fn as_object_mut(&mut self) -> Option<&mut BTreeMap<String, JsonValue>> {
        if let JsonValue::Object(m) = self { Some(m) } else { None }
    }
    pub fn get(&self, key: &str) -> Option<&JsonValue> { self.as_object()?.get(key) }

    fn from_proto(val: ProtoJson) -> Self {
        match val {
            ProtoJson::Null => JsonValue::Null,
            ProtoJson::Bool(b) => JsonValue::Bool(b),
            ProtoJson::Number(n) => match n {
                JsonNumber::Int(i) => JsonValue::Int(i),
                JsonNumber::Uint(u) => i64::try_from(u).map_or(JsonValue::UInt(u), JsonValue::Int),
                JsonNumber::Float(f) => n
                    .as_i64()
                    .map(JsonValue::Int)
                    .or_else(|| n.as_u64().map(JsonValue::UInt))
                    .unwrap_or(JsonValue::Float(f)),
            },
            ProtoJson::String(s) => JsonValue::Str(s),
            ProtoJson::Array(items) => JsonValue::Array(items.into_iter().map(Self::from_proto).collect()),
            ProtoJson::Object(obj) => JsonValue::Object(
                obj.iter().map(|(k, v)| (k.clone(), Self::from_proto(v.clone()))).collect(),
            ),
        }
    }

    fn to_proto(&self) -> ProtoJson {
        match self {
            JsonValue::Null => ProtoJson::Null,
            JsonValue::Bool(b) => ProtoJson::Bool(*b),
            JsonValue::Int(n) => ProtoJson::Number(JsonNumber::Int(*n)),
            JsonValue::UInt(n) => ProtoJson::Number(JsonNumber::Uint(*n)),
            JsonValue::Float(f) if f.is_finite() => ProtoJson::Number(JsonNumber::Float(*f)),
            JsonValue::Float(_) => ProtoJson::Null,
            JsonValue::Str(s) => ProtoJson::String(s.clone()),
            JsonValue::Array(items) => ProtoJson::Array(items.iter().map(Self::to_proto).collect()),
            JsonValue::Object(map) => {
                let mut obj = JsonObject::with_capacity(map.len());
                for (k, v) in map {
                    obj.insert(k.clone(), v.to_proto());
                }
                ProtoJson::Object(obj)
            }
        }
    }
}

macro_rules! impl_json_from {
    ($($ty:ty => |$v:ident| $expr:expr),+ $(,)?) => {
        $(impl From<$ty> for JsonValue { fn from($v: $ty) -> Self { $expr } })+
    };
}

impl_json_from! {
    bool => |v| JsonValue::Bool(v), u8 => |v| JsonValue::UInt(u64::from(v)),
    u16 => |v| JsonValue::UInt(u64::from(v)), u32 => |v| JsonValue::UInt(u64::from(v)),
    u64 => |v| JsonValue::UInt(v), i32 => |v| JsonValue::Int(i64::from(v)),
    i64 => |v| JsonValue::Int(v), f64 => |v| JsonValue::Float(v),
    &str => |v| JsonValue::Str(v.to_string()), String => |v| JsonValue::Str(v),
}

/// Typed extraction helpers for JSON objects.
pub struct JsonObjReader<'a> {
    map: &'a BTreeMap<String, JsonValue>,
}

impl<'a> JsonObjReader<'a> {
    pub fn new(val: &'a JsonValue, ctx: &str) -> Result<Self> {
        val.as_object().map(|map| Self { map }).ok_or_else(|| PlayerError::Json(format!("expected JSON object for {ctx}")))
    }
    pub fn raw(&self) -> &'a BTreeMap<String, JsonValue> { self.map }

    fn opt_map<T>(&self, key: &str, ty: &str, f: impl FnOnce(&'a JsonValue) -> Option<T>) -> Result<Option<T>> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(v) => f(v).map(Some).ok_or_else(|| PlayerError::Json(format!("field '{key}' must be {ty}"))),
        }
    }
    fn narrow<T: TryFrom<U>, U>(v: U, key: &str, ty: &str) -> Result<T> {
        T::try_from(v).map_err(|_| PlayerError::Json(format!("field '{key}' overflows {ty}")))
    }

    pub fn req_str(&self, key: &str) -> Result<String> {
        self.map.get(key).and_then(JsonValue::as_str).map(ToOwned::to_owned).ok_or_else(|| PlayerError::Json(format!("missing or non-string field '{key}'")))
    }
    pub fn opt_str(&self, key: &str) -> Result<Option<String>> { self.opt_map(key, "string or null", |v| v.as_str().map(ToOwned::to_owned)) }
    pub fn req_u64(&self, key: &str) -> Result<u64> {
        self.map.get(key).and_then(JsonValue::as_u64).ok_or_else(|| PlayerError::Json(format!("missing or non-u64 field '{key}'")))
    }
    pub fn opt_u64(&self, key: &str, d: u64) -> Result<u64> { Ok(self.opt_map(key, "u64", JsonValue::as_u64)?.unwrap_or(d)) }
    pub fn opt_u64_nullable(&self, key: &str) -> Result<Option<u64>> { self.opt_map(key, "u64 or null", JsonValue::as_u64) }
    pub fn req_u32(&self, key: &str) -> Result<u32> { Self::narrow(self.req_u64(key)?, key, "u32") }
    pub fn opt_u32(&self, key: &str, d: u32) -> Result<u32> { Self::narrow(self.opt_u64(key, u64::from(d))?, key, "u32") }
    pub fn opt_u8(&self, key: &str, d: u8) -> Result<u8> { Self::narrow(self.opt_u64(key, u64::from(d))?, key, "u8") }
    pub fn opt_i64(&self, key: &str, d: i64) -> Result<i64> { Ok(self.opt_map(key, "i64", JsonValue::as_i64)?.unwrap_or(d)) }
    pub fn opt_i32(&self, key: &str, d: i32) -> Result<i32> { Self::narrow(self.opt_i64(key, i64::from(d))?, key, "i32") }
    pub fn opt_bool(&self, key: &str, d: bool) -> Result<bool> { Ok(self.opt_map(key, "bool", JsonValue::as_bool)?.unwrap_or(d)) }
    pub fn req_val(&self, key: &str) -> Result<&'a JsonValue> { self.map.get(key).ok_or_else(|| PlayerError::Json(format!("missing required field '{key}'"))) }
    pub fn opt_obj(&self, key: &str) -> Result<Option<&'a BTreeMap<String, JsonValue>>> { self.opt_map(key, "object", JsonValue::as_object) }
    pub fn opt_arr(&self, key: &str) -> Result<Option<&'a [JsonValue]>> { self.opt_map(key, "array", JsonValue::as_array) }
}
