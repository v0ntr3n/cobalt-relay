use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::error::{PlayerError, Result};

const MAX_JSON_DEPTH: usize = 128;

/// Deterministic, zero-dependency RFC 8259 JSON value AST.
///
/// Uses `BTreeMap` for objects so serialized snapshots and WAL entries have
/// canonical key ordering across runs and platforms.
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
    pub fn empty_object() -> Self {
        JsonValue::Object(BTreeMap::new())
    }

    pub fn empty_array() -> Self {
        JsonValue::Array(Vec::new())
    }

    pub fn parse(input: &str) -> Result<Self> {
        let mut parser = Parser::new(input);
        let value = parser.parse_value(0)?;
        parser.skip_ws();
        if !parser.is_eof() {
            return Err(parser.err("unexpected trailing characters after JSON value"));
        }
        Ok(value)
    }

    pub fn to_json_string(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write_compact(&mut out);
        out
    }

    pub fn to_json_pretty(&self) -> String {
        let mut out = String::with_capacity(512);
        self.write_pretty(&mut out, 0);
        out.push('\n');
        out
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
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
        match self {
            JsonValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?.get(key)
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(true) => out.push_str("true"),
            JsonValue::Bool(false) => out.push_str("false"),
            JsonValue::Int(n) => {
                let _ = write!(out, "{n}");
            }
            JsonValue::UInt(n) => {
                let _ = write!(out, "{n}");
            }
            JsonValue::Float(f) => {
                if f.is_finite() {
                    let s = f.to_string();
                    out.push_str(&s);
                    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                        out.push_str(".0");
                    }
                } else {
                    out.push_str("null");
                }
            }
            JsonValue::Str(s) => write_escaped_str(out, s),
            JsonValue::Array(items) => {
                out.push('[');
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            JsonValue::Object(map) => {
                out.push('{');
                for (idx, (k, v)) in map.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    write_escaped_str(out, k);
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
                for (idx, item) in items.iter().enumerate() {
                    push_indent(out, indent + 2);
                    item.write_pretty(out, indent + 2);
                    if idx + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push(']');
            }
            JsonValue::Object(map) if !map.is_empty() => {
                out.push_str("{\n");
                for (idx, (k, v)) in map.iter().enumerate() {
                    push_indent(out, indent + 2);
                    write_escaped_str(out, k);
                    out.push_str(": ");
                    v.write_pretty(out, indent + 2);
                    if idx + 1 < map.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push('}');
            }
            _ => self.write_compact(out),
        }
    }
}

fn push_indent(out: &mut String, spaces: usize) {
    for _ in 0..spaces {
        out.push(' ');
    }
}

fn write_escaped_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0C' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<bool> for JsonValue {
    fn from(v: bool) -> Self {
        JsonValue::Bool(v)
    }
}

impl From<u8> for JsonValue {
    fn from(v: u8) -> Self {
        JsonValue::UInt(u64::from(v))
    }
}

impl From<u16> for JsonValue {
    fn from(v: u16) -> Self {
        JsonValue::UInt(u64::from(v))
    }
}

impl From<u32> for JsonValue {
    fn from(v: u32) -> Self {
        JsonValue::UInt(u64::from(v))
    }
}

impl From<u64> for JsonValue {
    fn from(v: u64) -> Self {
        JsonValue::UInt(v)
    }
}

impl From<i32> for JsonValue {
    fn from(v: i32) -> Self {
        JsonValue::Int(i64::from(v))
    }
}

impl From<i64> for JsonValue {
    fn from(v: i64) -> Self {
        JsonValue::Int(v)
    }
}

impl From<f64> for JsonValue {
    fn from(v: f64) -> Self {
        JsonValue::Float(v)
    }
}

impl From<&str> for JsonValue {
    fn from(v: &str) -> Self {
        JsonValue::Str(v.to_string())
    }
}

impl From<String> for JsonValue {
    fn from(v: String) -> Self {
        JsonValue::Str(v)
    }
}

/// Typed extraction helpers for JSON objects.
pub struct JsonObjReader<'a> {
    map: &'a BTreeMap<String, JsonValue>,
}

impl<'a> JsonObjReader<'a> {
    pub fn new(val: &'a JsonValue, ctx: &str) -> Result<Self> {
        let map = val
            .as_object()
            .ok_or_else(|| PlayerError::Json(format!("expected JSON object for {ctx}")))?;
        Ok(Self { map })
    }

    pub fn raw(&self) -> &'a BTreeMap<String, JsonValue> {
        self.map
    }

    pub fn req_str(&self, key: &str) -> Result<String> {
        self.map
            .get(key)
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| PlayerError::Json(format!("missing or non-string field '{key}'")))
    }

    pub fn opt_str(&self, key: &str) -> Result<Option<String>> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(v) => v
                .as_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be string or null"))),
        }
    }

    pub fn req_u64(&self, key: &str) -> Result<u64> {
        self.map
            .get(key)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| PlayerError::Json(format!("missing or non-u64 field '{key}'")))
    }

    pub fn opt_u64(&self, key: &str, default: u64) -> Result<u64> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(default),
            Some(v) => v
                .as_u64()
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be u64"))),
        }
    }

    pub fn opt_u64_nullable(&self, key: &str) -> Result<Option<u64>> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be u64 or null"))),
        }
    }

    pub fn req_u32(&self, key: &str) -> Result<u32> {
        let val = self.req_u64(key)?;
        u32::try_from(val).map_err(|_| PlayerError::Json(format!("field '{key}' overflows u32")))
    }

    pub fn opt_u32(&self, key: &str, default: u32) -> Result<u32> {
        let val = self.opt_u64(key, u64::from(default))?;
        u32::try_from(val).map_err(|_| PlayerError::Json(format!("field '{key}' overflows u32")))
    }

    pub fn opt_u8(&self, key: &str, default: u8) -> Result<u8> {
        let val = self.opt_u64(key, u64::from(default))?;
        u8::try_from(val).map_err(|_| PlayerError::Json(format!("field '{key}' overflows u8")))
    }

    pub fn opt_i64(&self, key: &str, default: i64) -> Result<i64> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(default),
            Some(v) => v
                .as_i64()
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be i64"))),
        }
    }

    pub fn opt_i32(&self, key: &str, default: i32) -> Result<i32> {
        let val = self.opt_i64(key, i64::from(default))?;
        i32::try_from(val).map_err(|_| PlayerError::Json(format!("field '{key}' overflows i32")))
    }

    pub fn opt_bool(&self, key: &str, default: bool) -> Result<bool> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(default),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be bool"))),
        }
    }

    pub fn req_val(&self, key: &str) -> Result<&'a JsonValue> {
        self.map
            .get(key)
            .ok_or_else(|| PlayerError::Json(format!("missing required field '{key}'")))
    }

    pub fn opt_obj(&self, key: &str) -> Result<Option<&'a BTreeMap<String, JsonValue>>> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(v) => v
                .as_object()
                .map(Some)
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be object"))),
        }
    }

    pub fn opt_arr(&self, key: &str) -> Result<Option<&'a [JsonValue]>> {
        match self.map.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(v) => v
                .as_array()
                .map(Some)
                .ok_or_else(|| PlayerError::Json(format!("field '{key}' must be array"))),
        }
    }
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn err(&self, msg: &str) -> PlayerError {
        PlayerError::Json(format!("{msg} at byte offset {}", self.pos))
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.err("maximum JSON nesting depth exceeded"));
        }
        self.skip_ws();
        let b = self
            .peek()
            .ok_or_else(|| self.err("unexpected end of JSON input"))?;
        match b {
            b'n' => self.parse_literal(b"null", JsonValue::Null),
            b't' => self.parse_literal(b"true", JsonValue::Bool(true)),
            b'f' => self.parse_literal(b"false", JsonValue::Bool(false)),
            b'"' => self.parse_string().map(JsonValue::Str),
            b'[' => self.parse_array(depth + 1),
            b'{' => self.parse_object(depth + 1),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(self.err("unexpected token in JSON value")),
        }
    }

    fn parse_literal(&mut self, expected: &[u8], val: JsonValue) -> Result<JsonValue> {
        if self.bytes.get(self.pos..self.pos + expected.len()) == Some(expected) {
            self.pos += expected.len();
            Ok(val)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn parse_string(&mut self) -> Result<String> {
        if self.peek() != Some(b'"') {
            return Err(self.err("expected '\"'"));
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
                let esc = self
                    .peek()
                    .ok_or_else(|| self.err("unterminated escape sequence"))?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0C'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let u1 = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&u1) {
                            if self.bytes.get(self.pos..self.pos + 2) != Some(b"\\u") {
                                return Err(self.err("unpaired UTF-16 high surrogate"));
                            }
                            self.pos += 2;
                            let u2 = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&u2) {
                                return Err(self.err("invalid UTF-16 low surrogate"));
                            }
                            let combined = 0x10000
                                + (((u32::from(u1) - 0xD800) << 10) | (u32::from(u2) - 0xDC00));
                            let ch = char::from_u32(combined)
                                .ok_or_else(|| self.err("invalid surrogate codepoint"))?;
                            out.push(ch);
                        } else if (0xDC00..=0xDFFF).contains(&u1) {
                            return Err(self.err("unpaired UTF-16 low surrogate"));
                        } else {
                            let ch = char::from_u32(u32::from(u1))
                                .ok_or_else(|| self.err("invalid unicode escape"))?;
                            out.push(ch);
                        }
                    }
                    _ => return Err(self.err("invalid escape character")),
                }
            } else if b < 0x20 {
                return Err(self.err("unescaped control character in string"));
            } else {
                let rest = &self.src[self.pos..];
                let ch = rest
                    .chars()
                    .next()
                    .ok_or_else(|| self.err("invalid UTF-8 character"))?;
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        Err(self.err("unterminated string literal"))
    }

    fn parse_hex4(&mut self) -> Result<u16> {
        let slice = self
            .src
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.err("truncated \\uXXXX escape"))?;
        if !slice.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(self.err("non-hex digit in \\uXXXX escape"));
        }
        let val =
            u16::from_str_radix(slice, 16).map_err(|_| self.err("failed parsing \\uXXXX hex"))?;
        self.pos += 4;
        Ok(val)
    }

    fn parse_number(&mut self) -> Result<JsonValue> {
        let start = self.pos;
        let mut is_neg = false;
        let mut is_float = false;

        if self.peek() == Some(b'-') {
            is_neg = true;
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
                return Err(self.err("expected digit in exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let slice = &self.src[start..self.pos];
        if is_float {
            let v: f64 = slice
                .parse()
                .map_err(|_| self.err("invalid floating-point number"))?;
            if !v.is_finite() {
                return Err(self.err("non-finite JSON number"));
            }
            Ok(JsonValue::Float(v))
        } else if is_neg {
            let v: i64 = slice
                .parse()
                .map_err(|_| self.err("signed integer out of range"))?;
            Ok(JsonValue::Int(v))
        } else if let Ok(v) = slice.parse::<u64>() {
            if let Ok(iv) = i64::try_from(v) {
                Ok(JsonValue::Int(iv))
            } else {
                Ok(JsonValue::UInt(v))
            }
        } else {
            Err(self.err("unsigned integer out of range"))
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue> {
        self.pos += 1; // '['
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            let item = self.parse_value(depth)?;
            items.push(item);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue> {
        self.pos += 1; // '{'
        self.skip_ws();
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected string key in object"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':' after object key"));
            }
            self.pos += 1;
            let val = self.parse_value(depth)?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(map));
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
    }
}
