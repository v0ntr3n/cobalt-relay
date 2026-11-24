//! Minimal, zero-dependency, 100% safe RFC 8259 JSON parser & serializer for `gserver-data`.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_i64().and_then(|v| u64::try_from(v).ok())
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|v| u32::try_from(v).ok())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(n) => Some(*n as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?.get(key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON parse error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for JsonError {}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, JsonError> {
        Err(JsonError {
            offset: self.pos,
            message: msg.into(),
        })
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<JsonValue, JsonError> {
        self.skip_ws();
        let Some(b) = self.peek() else {
            return self.err("unexpected end of JSON input");
        };
        match b {
            b'n' => self.parse_literal(b"null", JsonValue::Null),
            b't' => self.parse_literal(b"true", JsonValue::Bool(true)),
            b'f' => self.parse_literal(b"false", JsonValue::Bool(false)),
            b'"' => self.parse_string().map(JsonValue::Str),
            b'[' => self.parse_array(),
            b'{' => self.parse_object(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => self.err(format!("unexpected byte 0x{b:02X}")),
        }
    }

    fn parse_literal(&mut self, lit: &[u8], val: JsonValue) -> Result<JsonValue, JsonError> {
        if self.src.get(self.pos..self.pos + lit.len()) == Some(lit) {
            self.pos += lit.len();
            Ok(val)
        } else {
            self.err("invalid JSON literal")
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, JsonError> {
        if self.pos + 4 > self.src.len() {
            return self.err("truncated \\uXXXX escape");
        }
        let mut acc = 0u16;
        for _ in 0..4 {
            let b = self.src[self.pos];
            self.pos += 1;
            let nib = match b {
                b'0'..=b'9' => u16::from(b - b'0'),
                b'a'..=b'f' => u16::from(b - b'a' + 10),
                b'A'..=b'F' => u16::from(b - b'A' + 10),
                _ => return self.err("invalid hex digit in \\uXXXX escape"),
            };
            acc = (acc << 4) | nib;
        }
        Ok(acc)
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        if self.peek() != Some(b'"') {
            return self.err("expected '\"'");
        }
        self.pos += 1;
        let mut out = String::new();

        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                if self.pos >= self.src.len() {
                    return self.err("unterminated escape sequence");
                }
                let esc = self.src[self.pos];
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code) {
                            if self.src.get(self.pos..self.pos + 2) != Some(b"\\u") {
                                return self.err("unpaired UTF-16 high surrogate");
                            }
                            self.pos += 2;
                            let low = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return self.err("invalid UTF-16 low surrogate");
                            }
                            let combined =
                                0x10000 + (((u32::from(code) - 0xD800) << 10) | (u32::from(low) - 0xDC00));
                            let Some(ch) = char::from_u32(combined) else {
                                return self.err("invalid surrogate scalar");
                            };
                            out.push(ch);
                        } else if let Some(ch) = char::from_u32(u32::from(code)) {
                            out.push(ch);
                        } else {
                            return self.err("invalid Unicode code point");
                        }
                    }
                    _ => return self.err("unsupported string escape"),
                }
            } else {
                // Fast-path contiguous unescaped UTF-8 run
                let start = self.pos;
                while self.pos < self.src.len() {
                    let c = self.src[self.pos];
                    if c == b'"' || c == b'\\' || c < 0x20 {
                        break;
                    }
                    self.pos += 1;
                }
                if self.pos == start {
                    return self.err("unescaped control character in JSON string");
                }
                let slice = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|_| JsonError {
                        offset: start,
                        message: "invalid UTF-8 sequence in JSON string".to_string(),
                    })?;
                out.push_str(slice);
            }
        }
        self.err("unterminated JSON string")
    }

    fn parse_number(&mut self) -> Result<JsonValue, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return self.err("invalid number"),
        }

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.err("expected digit after decimal point");
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
                return self.err("expected digit in exponent");
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let token = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| JsonError {
            offset: start,
            message: "invalid UTF-8 in number".to_string(),
        })?;
        if !is_float {
            if let Ok(i) = token.parse::<i64>() {
                return Ok(JsonValue::Int(i));
            }
        }
        let f = token.parse::<f64>().map_err(|_| JsonError {
            offset: start,
            message: format!("invalid numeric literal '{token}'"),
        })?;
        Ok(JsonValue::Float(f))
    }

    fn parse_array(&mut self) -> Result<JsonValue, JsonError> {
        self.pos += 1; // '['
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => return self.err("expected ',' or ']' in array"),
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, JsonError> {
        self.pos += 1; // '{'
        self.skip_ws();
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return self.err("expected ':' after object key");
            }
            self.pos += 1;
            let val = self.parse_value()?;
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
                _ => return self.err("expected ',' or '}' in object"),
            }
        }
    }
}

/// Parses a UTF-8 JSON document into a [`JsonValue`].
pub fn parse_json(input: &str) -> Result<JsonValue, JsonError> {
    let trimmed = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let mut parser = Parser::new(trimmed);
    let val = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos != parser.src.len() {
        return parser.err("trailing characters after root JSON value");
    }
    Ok(val)
}
