//! # `gserver-rpc-catalog`
//!
//! `#![forbid(unsafe_code)]` pure-Rust protocol catalog, `LCPacket` + AMF3 wire adapter backed by
//! [`gserver_protocol`], typed bootstrap response builders, and Lua `parseData` schema validators
//! for the **GServer (`vnsg2`)** Cocos2d-x / BabelTime client.
//!
//! ## Modules
//! - [`net_const`]: All **2,107** RPC constants extracted verbatim from `scripts/game/consts/NetConst.lua`
//!   (plus 2,021 Time-Machine `tm*` constants in [`net_const::tm`] and 16 inline RPC/push literals).
//! - [`amf3`]: Lua-table-oriented [`Amf3Value`] AST adapter backed by [`gserver_protocol::amf3`]
//!   (`0x09` 1-based dense Lua sequences vs. `0x0A` anonymous dynamic string-keyed maps, signed
//!   29-bit `U29` integers with automatic IEEE-754 `f64` promotion, and UTF-8 string reference tables).
//! - [`packet`]: 24-byte `LCPacket` client frame and 8-byte `CConnection` server frame adapters
//!   delegating RFC 1321 MD5, `"BabelTime"` XOR-MD5, framing, and Zlib compression to [`gserver_protocol`].
//! - [`envelope`]: `Network.lua` request/response RPC envelopes (`err == "ok"`, `<method>_<mId>`
//!   callback correlation, server Unix timestamp sync, session `token`, and `"dummy"` push frames)
//!   delegating to [`gserver_protocol::rpc`].
//! - [`bootstrap`]: Ordered `TCP Connect` -> `MainTown` (100% `MainLoading`) RPC catalog, typed
//!   response builders, and stateful [`bootstrap::BootstrapSession`] simulator.
//! - [`validator`]: Strict Lua runtime invariant checker catching `nil`-indexing, string-vs-number,
//!   1-based-array-vs-string-map, server timestamp, and tutorial/first-fight lock traps.

#![forbid(unsafe_code)]

/// Lua-aligned AMF3 AST adapter backed by [`gserver_protocol::amf3`].
pub mod amf3 {
    use std::collections::BTreeMap;
    use std::fmt;

    use gserver_protocol::amf3::{Amf3Object, DecodeConfig, Value};

    /// Minimum signed 29-bit integer representable by AMF3 `0x04` (`-2^28`).
    pub const AMF3_INT_MIN: i64 = gserver_protocol::amf3::AMF3_INT_MIN as i64;
    /// Maximum signed 29-bit integer representable by AMF3 `0x04` (`2^28 - 1`).
    pub const AMF3_INT_MAX: i64 = gserver_protocol::amf3::AMF3_INT_MAX as i64;

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
        /// Raw byte buffer (`0x0C` ByteArray, or binary sub-packet payload).
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
            Self::Map(iter.into_iter().map(|(k, v)| (k.into(), v)).collect())
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
            self.as_f64().is_some()
                || self.as_str().is_some_and(|s| {
                    let t = s.trim();
                    !t.is_empty() && t.parse::<f64>().is_ok()
                })
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

        /// Both an empty AMF3 `Array` (`0x09 0x01 0x01`) and an empty AMF3 `Object`
        /// (`0x0A 0x0B 0x01 0x01`) decode to `{}` in Cocos2d-x Lua (`Amf3Module.cpp`).
        pub fn is_lua_table(&self) -> bool {
            matches!(self, Self::Map(_) | Self::Array(_))
        }

        pub fn get_field(&self, key: &str) -> Option<&Amf3Value> {
            match self {
                Self::Map(m) => m.get(key),
                _ => None,
            }
        }

        /// Converts this catalog [`Amf3Value`] into [`gserver_protocol::Value`] preserving exact
        /// wire markers (`0x04` U29 vs `0x05` `Double` promotion and `0x09` vs `0x0A` containers).
        pub fn to_protocol_value(&self) -> Value {
            match self {
                Self::Undefined => Value::Undefined,
                Self::Null => Value::Null,
                Self::Bool(b) => Value::Bool(*b),
                Self::Int(n) => Value::from(*n),
                Self::Double(f) => Value::Double(*f),
                Self::String(s) => Value::String(s.clone()),
                Self::Date(ms) => Value::Date(*ms),
                Self::Array(items) => {
                    Value::dense_array(items.iter().map(Self::to_protocol_value).collect())
                }
                Self::Map(entries) => Value::Object(Amf3Object {
                    class_name: String::new(),
                    dynamic: true,
                    sealed_members: Vec::new(),
                    dynamic_members: entries
                        .iter()
                        .filter(|(k, _)| !k.is_empty())
                        .map(|(k, v)| (k.clone(), v.to_protocol_value()))
                        .collect(),
                    externalized: None,
                }),
                Self::ByteArray(raw) => Value::ByteArray(raw.clone()),
            }
        }

        /// Converts a decoded [`gserver_protocol::Value`] into [`Amf3Value`].
        pub fn from_protocol_value(val: &Value) -> Self {
            match val {
                Value::Undefined => Self::Undefined,
                Value::Null => Self::Null,
                Value::Bool(b) => Self::Bool(*b),
                Value::Int(n) => Self::Int(i64::from(*n)),
                Value::Double(f) => Self::Double(*f),
                Value::String(s) => Self::String(s.clone()),
                Value::Date(ms) => Self::Date(*ms),
                Value::ByteString(b) | Value::ByteArray(b) | Value::Xml(b) | Value::XmlDoc(b) => {
                    Self::ByteArray(b.clone())
                }
                Value::Array { dense, assoc } => {
                    if assoc.is_empty() {
                        Self::Array(dense.iter().map(Self::from_protocol_value).collect())
                    } else {
                        let mut m: BTreeMap<String, Self> = assoc
                            .iter()
                            .map(|(k, v)| (k.clone(), Self::from_protocol_value(v)))
                            .collect();
                        for (i, item) in dense.iter().enumerate() {
                            m.insert((i + 1).to_string(), Self::from_protocol_value(item));
                        }
                        Self::Map(m)
                    }
                }
                Value::Object(obj) => Self::Map(
                    obj.iter()
                        .map(|(k, v)| (k.to_owned(), Self::from_protocol_value(v)))
                        .collect(),
                ),
            }
        }
    }

    impl From<bool> for Amf3Value {
        fn from(v: bool) -> Self {
            Self::Bool(v)
        }
    }

    impl From<i64> for Amf3Value {
        fn from(v: i64) -> Self {
            Self::Int(v)
        }
    }

    impl From<f64> for Amf3Value {
        fn from(v: f64) -> Self {
            Self::Double(v)
        }
    }

    impl From<&str> for Amf3Value {
        fn from(s: &str) -> Self {
            Self::String(s.to_owned())
        }
    }

    impl From<String> for Amf3Value {
        fn from(s: String) -> Self {
            Self::String(s)
        }
    }

    impl From<&String> for Amf3Value {
        fn from(s: &String) -> Self {
            Self::String(s.clone())
        }
    }

    impl From<Vec<Amf3Value>> for Amf3Value {
        fn from(v: Vec<Amf3Value>) -> Self {
            Self::Array(v)
        }
    }

    impl From<BTreeMap<String, Amf3Value>> for Amf3Value {
        fn from(m: BTreeMap<String, Amf3Value>) -> Self {
            Self::Map(m)
        }
    }

    /// Errors encountered while encoding or decoding AMF3 payloads.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Amf3Error {
        Protocol(gserver_protocol::Amf3Error),
        TrailingBytes(usize),
    }

    impl fmt::Display for Amf3Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Protocol(e) => write!(f, "{e}"),
                Self::TrailingBytes(n) => write!(f, "{n} unexpected trailing bytes after AMF3 root"),
            }
        }
    }

    impl std::error::Error for Amf3Error {}

    impl From<gserver_protocol::Amf3Error> for Amf3Error {
        fn from(e: gserver_protocol::Amf3Error) -> Self {
            Self::Protocol(e)
        }
    }

    /// Encode an [`Amf3Value`] into raw AMF3 bytes via [`gserver_protocol::amf3`].
    pub fn encode_amf3(value: &Amf3Value) -> Vec<u8> {
        gserver_protocol::amf3::encode(&value.to_protocol_value())
            .expect("in-memory Amf3Value within AMF3 depth and U29 length limits")
    }

    /// Decode a complete AMF3 value from `bytes` via [`gserver_protocol::amf3`], rejecting trailing bytes.
    pub fn decode_amf3(bytes: &[u8]) -> Result<Amf3Value, Amf3Error> {
        let cfg = DecodeConfig {
            strict_utf8: true,
            ..DecodeConfig::default()
        };
        let (val, used) = gserver_protocol::amf3::decode_from(bytes, 0, cfg)?;
        if used != bytes.len() {
            return Err(Amf3Error::TrailingBytes(bytes.len() - used));
        }
        Ok(Amf3Value::from_protocol_value(&val))
    }
}

pub mod bootstrap;
pub mod envelope;
pub mod net_const;
pub mod packet;
pub mod validator;

pub use amf3::{decode_amf3, encode_amf3, Amf3Error, Amf3Value};
pub use bootstrap::{
    BootstrapPhase, BootstrapRpcSpec, BootstrapSession, CopyChapterSnapshot, CopyListSnapshot,
    EnterViewSnapshot, FormationSquad, HeroDto, HeroFormationSnapshot, UserProfile, VaConfig,
    BOOTSTRAP_SEQUENCE,
};
pub use envelope::{RpcRequestEnvelope, RpcResponseEnvelope};
pub use net_const::{NetConstEntry, RpcDirection, ALL_NET_CONSTS, NET_CONST_COUNT};
pub use packet::{babeltime_body_checksum, md5, ClientFrame, PacketError, ServerFrame};
pub use validator::{validate_response_for_method, LuaSchemaViolation};
