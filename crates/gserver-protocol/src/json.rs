//! Complete Pure-Rust RFC 8259 JSON parser and serializer (`#![forbid(unsafe_code)]`, `std` only).
//!
//! Preserves exact integer representations (`i64` / `u64`) separately from `f64` floats
//! so protocol fields like `"AMF": 3`, `"port": 8160`, and `"timestamp": 1795500450`
//! serialize cleanly without `.0`, while preserving object key insertion order.

use std::fmt;

const MAX_NESTING_DEPTH: usize = 128;

/// Numeric value in JSON preserving exact integer vs floating-point fidelity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JsonNumber {
    Int(i64),
    Uint(u64),
    Float(f64),
}

impl JsonNumber {
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            JsonNumber::Int(v) => Some(v),
            JsonNumber::Uint(v) => i64::try_from(v).ok(),
            JsonNumber::Float(v) => {
                if v.is_finite() && v.fract() == 0.0 && v >= (i64::MIN as f64) && v < (i64::MAX as f64) {
                    Some(v as i64)
                } else {
                    None
                }
            }
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            JsonNumber::Int(v) => u64::try_from(v).ok(),
            JsonNumber::Uint(v) => Some(v),
            JsonNumber::Float(v) => {
                if v.is_finite() && v.fract() == 0.0 && v >= 0.0 && v < (u64::MAX as f64) {
                    Some(v as u64)
                } else {
                    None
                }
            }
        }
    }

    pub fn as_f64(&self) -> f64 {
        match *self {
            JsonNumber::Int(v) => v as f64,
            JsonNumber::Uint(v) => v as f64,
            JsonNumber::Float(v) => v,
        }
    }
}

/// Ordered JSON object preserving key insertion order with deterministic key lookup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonObject {
    entries: Vec<(String, JsonValue)>,
}

impl JsonObject {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
        }
    }

    /// Inserts or updates `key` with `value`. If `key` already exists, its value
    /// is updated in-place preserving its position.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<JsonValue>) {
        let key = key.into();
        let value = value.into();
        if let Some((_, existing)) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            *existing = value;
        } else {
            self.entries.push((key, value));
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(JsonValue::as_str)
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(JsonValue::as_i64)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(JsonValue::as_bool)
    }

    pub fn get_array(&self, key: &str) -> Option<&[JsonValue]> {
        self.get(key).and_then(JsonValue::as_array)
    }

    pub fn get_object(&self, key: &str) -> Option<&JsonObject> {
        self.get(key).and_then(JsonValue::as_object)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, (String, JsonValue)> {
        self.entries.iter()
    }
}

/// Complete RFC 8259 JSON value AST.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(JsonObject),
}

impl JsonValue {
    pub fn object() -> Self {
        JsonValue::Object(JsonObject::new())
    }

    pub fn array() -> Self {
        JsonValue::Array(Vec::new())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, JsonValue::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            JsonValue::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Number(n) => Some(n.as_f64()),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Extracts a scalar string or converts numbers/booleans to string (matching
    /// loose Lua/PHP scalar extraction in GServer).
    pub fn as_scalar_string(&self) -> Option<String> {
        match self {
            JsonValue::String(s) => Some(s.clone()),
            JsonValue::Number(JsonNumber::Int(n)) => Some(n.to_string()),
            JsonValue::Number(JsonNumber::Uint(n)) => Some(n.to_string()),
            JsonValue::Number(JsonNumber::Float(n)) => Some(n.to_string()),
            JsonValue::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&JsonObject> {
        match self {
            JsonValue::Object(obj) => Some(obj),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?.get(key)
    }

    /// Serializes `self` into compact RFC 8259 UTF-8 JSON text.
    pub fn to_json_string(&self) -> String {
        let mut out = String::with_capacity(128);
        self.write_compact(&mut out);
        out
    }

    /// Serializes `self` into human-readable 2-space indented RFC 8259 JSON text.
    pub fn to_json_pretty(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write_pretty(&mut out, 0);
        out
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(true) => out.push_str("true"),
            JsonValue::Bool(false) => out.push_str("false"),
            JsonValue::Number(JsonNumber::Int(n)) => out.push_str(&n.to_string()),
            JsonValue::Number(JsonNumber::Uint(n)) => out.push_str(&n.to_string()),
            JsonValue::Number(JsonNumber::Float(f)) => {
                if f.is_finite() {
                    out.push_str(&f.to_string());
                } else {
                    out.push_str("null");
                }
            }
            JsonValue::String(s) => write_escaped_json_string(out, s),
            JsonValue::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            JsonValue::Object(obj) => {
                out.push('{');
                for (i, (k, v)) in obj.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_escaped_json_string(out, k);
                    out.push(':');
                    v.write_compact(out);
                }
                out.push('}');
            }
        }
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        match self {
            JsonValue::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                let child_indent = indent + 2;
                for (i, item) in items.iter().enumerate() {
                    for _ in 0..child_indent {
                        out.push(' ');
                    }
                    item.write_pretty(out, child_indent);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                for _ in 0..indent {
                    out.push(' ');
                }
                out.push(']');
            }
            JsonValue::Object(obj) if !obj.is_empty() => {
                out.push_str("{\n");
                let child_indent = indent + 2;
                for (i, (k, v)) in obj.iter().enumerate() {
                    for _ in 0..child_indent {
                        out.push(' ');
                    }
                    write_escaped_json_string(out, k);
                    out.push_str(": ");
                    v.write_pretty(out, child_indent);
                    if i + 1 < obj.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                for _ in 0..indent {
                    out.push(' ');
                }
                out.push('}');
            }
            _ => self.write_compact(out),
        }
    }
}

impl From<bool> for JsonValue {
    fn from(v: bool) -> Self {
        JsonValue::Bool(v)
    }
}

impl From<i32> for JsonValue {
    fn from(v: i32) -> Self {
        JsonValue::Number(JsonNumber::Int(v as i64))
    }
}

impl From<i64> for JsonValue {
    fn from(v: i64) -> Self {
        JsonValue::Number(JsonNumber::Int(v))
    }
}

impl From<u16> for JsonValue {
    fn from(v: u16) -> Self {
        JsonValue::Number(JsonNumber::Uint(v as u64))
    }
}

impl From<u32> for JsonValue {
    fn from(v: u32) -> Self {
        JsonValue::Number(JsonNumber::Uint(v as u64))
    }
}

impl From<u64> for JsonValue {
    fn from(v: u64) -> Self {
        JsonValue::Number(JsonNumber::Uint(v))
    }
}

impl From<usize> for JsonValue {
    fn from(v: usize) -> Self {
        JsonValue::Number(JsonNumber::Uint(v as u64))
    }
}

impl From<f64> for JsonValue {
    fn from(v: f64) -> Self {
        JsonValue::Number(JsonNumber::Float(v))
    }
}

impl From<&str> for JsonValue {
    fn from(v: &str) -> Self {
        JsonValue::String(v.to_string())
    }
}

impl From<String> for JsonValue {
    fn from(v: String) -> Self {
        JsonValue::String(v)
    }
}

impl From<Vec<JsonValue>> for JsonValue {
    fn from(v: Vec<JsonValue>) -> Self {
        JsonValue::Array(v)
    }
}

impl From<JsonObject> for JsonValue {
    fn from(v: JsonObject) -> Self {
        JsonValue::Object(v)
    }
}

impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json_string())
    }
}

/// Writes an RFC 8259 escaped JSON string literal (including surrounding quotes).
pub fn write_escaped_json_string(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let b = c as u8;
                out.push_str("\\u00");
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0F) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Error returned when parsing malformed RFC 8259 JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonParseError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for JsonParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON parse error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for JsonParseError {}

/// Parses RFC 8259 UTF-8 JSON text into a [`JsonValue`].
pub fn parse_json(input: &str) -> Result<JsonValue, JsonParseError> {
    let mut parser = Parser {
        src: input,
        bytes: input.as_bytes(),
        pos: 0,
    };
    let val = parser.parse_value(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.err("unexpected trailing characters after JSON value"));
    }
    Ok(val)
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: impl Into<String>) -> JsonParseError {
        JsonParseError {
            offset: self.pos,
            message: msg.into(),
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, JsonParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self.err("maximum JSON nesting depth exceeded"));
        }
        self.skip_ws();
        let Some(b) = self.peek() else {
            return Err(self.err("unexpected end of JSON input"));
        };
        match b {
            b'n' => self.parse_null(),
            b't' => self.parse_true(),
            b'f' => self.parse_false(),
            b'"' => self.parse_string().map(JsonValue::String),
            b'[' => self.parse_array(depth + 1),
            b'{' => self.parse_object(depth + 1),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(self.err(format!("unexpected byte '{}'", b as char))),
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, JsonParseError> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(JsonValue::Null)
        } else {
            Err(self.err("invalid literal (expected 'null')"))
        }
    }

    fn parse_true(&mut self) -> Result<JsonValue, JsonParseError> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(JsonValue::Bool(true))
        } else {
            Err(self.err("invalid literal (expected 'true')"))
        }
    }

    fn parse_false(&mut self) -> Result<JsonValue, JsonParseError> {
        if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(JsonValue::Bool(false))
        } else {
            Err(self.err("invalid literal (expected 'false')"))
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, JsonParseError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err("truncated \\uXXXX escape"));
        }
        let mut acc = 0u16;
        for _ in 0..4 {
            let b = self.bytes[self.pos];
            let digit = match b {
                b'0'..=b'9' => (b - b'0') as u16,
                b'a'..=b'f' => (b - b'a' + 10) as u16,
                b'A'..=b'F' => (b - b'A' + 10) as u16,
                _ => return Err(self.err("invalid hex digit in \\uXXXX escape")),
            };
            self.pos += 1;
            acc = (acc << 4) | digit;
        }
        Ok(acc)
    }

    fn parse_string(&mut self) -> Result<String, JsonParseError> {
        if self.peek() != Some(b'"') {
            return Err(self.err("expected '\"' to start JSON string"));
        }
        self.pos += 1;
        let mut out = String::new();

        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    return Err(self.err("unterminated escape sequence in string"));
                }
                let esc = self.bytes[self.pos];
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0c'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let unit = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&unit) {
                            if self.pos + 2 <= self.bytes.len()
                                && self.bytes[self.pos] == b'\\'
                                && self.bytes[self.pos + 1] == b'u'
                            {
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(self.err("invalid UTF-16 low surrogate in \\u escape"));
                                }
                                let cp = 0x10000u32
                                    + (((unit as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                                let Some(ch) = char::from_u32(cp) else {
                                    return Err(self.err("invalid Unicode scalar value"));
                                };
                                out.push(ch);
                            } else {
                                return Err(self.err("unpaired UTF-16 high surrogate in \\u escape"));
                            }
                        } else if (0xDC00..=0xDFFF).contains(&unit) {
                            return Err(self.err("unpaired UTF-16 low surrogate in \\u escape"));
                        } else if let Some(ch) = char::from_u32(unit as u32) {
                            out.push(ch);
                        } else {
                            return Err(self.err("invalid Unicode code point"));
                        }
                    }
                    _ => return Err(self.err(format!("invalid string escape '\\{}'", esc as char))),
                }
                continue;
            }
            if b < 0x20 {
                return Err(self.err("unescaped control character in JSON string"));
            }

            // Advance one full valid UTF-8 char from `self.src[self.pos..]`.
            let Some(ch) = self.src[self.pos..].chars().next() else {
                return Err(self.err("invalid UTF-8 sequence"));
            };
            out.push(ch);
            self.pos += ch.len_utf8();
        }

        Err(self.err("unterminated JSON string"))
    }

    fn parse_number(&mut self) -> Result<JsonValue, JsonParseError> {
        let start = self.pos;
        let mut is_float = false;

        if self.peek() == Some(b'-') {
            self.pos += 1;
        }

        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.err("leading zeros are not allowed in JSON numbers"));
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err("expected digit in JSON number")),
        }

        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected digit after decimal point"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected digit in number exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let slice = &self.src[start..self.pos];
        if !is_float {
            if let Ok(i) = slice.parse::<i64>() {
                return Ok(JsonValue::Number(JsonNumber::Int(i)));
            }
            if !slice.starts_with('-') {
                if let Ok(u) = slice.parse::<u64>() {
                    return Ok(JsonValue::Number(JsonNumber::Uint(u)));
                }
            }
        }

        let f = slice
            .parse::<f64>()
            .map_err(|_| self.err("invalid floating-point number"))?;
        if !f.is_finite() {
            return Err(self.err("JSON number overflowed to infinity"));
        }
        Ok(JsonValue::Number(JsonNumber::Float(f)))
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, JsonParseError> {
        self.pos += 1; // '['
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            let val = self.parse_value(depth)?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']' in JSON array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, JsonParseError> {
        self.pos += 1; // '{'
        self.skip_ws();
        let mut obj = JsonObject::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(obj));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected string key in JSON object"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':' after key in JSON object"));
            }
            self.pos += 1;
            let val = self.parse_value(depth)?;
            obj.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(obj));
                }
                _ => return Err(self.err("expected ',' or '}' in JSON object")),
            }
        }
    }
}


/// Compatibility type alias for callers naming the parse error `JsonError`.
pub type JsonError = JsonParseError;

impl JsonObject {
    /// Converts this insertion-ordered `JsonObject` into a `std::collections::BTreeMap`.
    pub fn to_btree_map(&self) -> std::collections::BTreeMap<String, JsonValue> {
        let mut map = std::collections::BTreeMap::new();
        for (k, v) in self.iter() {
            map.insert(k.clone(), v.clone());
        }
        map
    }

    /// Builds a `JsonObject` from a `std::collections::BTreeMap`.
    pub fn from_btree_map(map: std::collections::BTreeMap<String, JsonValue>) -> Self {
        let mut obj = Self::with_capacity(map.len());
        for (k, v) in map {
            obj.insert(k, v);
        }
        obj
    }

    /// Mutable lookup of a key in `JsonObject`.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut JsonValue> {
        for (k, v) in &mut self.entries {
            if k == key {
                return Some(v);
            }
        }
        None
    }
}

impl JsonValue {
    /// Alias for `JsonValue::object()` (`{}`).
    #[inline]
    pub fn empty_object() -> Self {
        Self::object()
    }

    /// Alias for `JsonValue::array()` (`[]`).
    #[inline]
    pub fn empty_array() -> Self {
        Self::array()
    }

    /// Convenience `u32` extractor used by template loaders and player persistence.
    pub fn as_u32(&self) -> Option<u32> {
        self.as_u64().and_then(|n| u32::try_from(n).ok())
    }

    /// Mutable object accessor.
    pub fn as_object_mut(&mut self) -> Option<&mut JsonObject> {
        match self {
            JsonValue::Object(obj) => Some(obj),
            _ => None,
        }
    }
}
