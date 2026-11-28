//! Wire framing, AMF3-compatible DTO values, declarative DTO macros, RPC envelopes,
//! and `NetConst.lua` method constants delegated to `gserver-protocol` & `gserver-rpc-catalog`.

use std::collections::BTreeMap;
use std::fmt;

use gserver_protocol::amf3::{self, Value, AMF3_INT_MAX, AMF3_INT_MIN};
use gserver_protocol::error::Amf3Error;
use gserver_protocol::frame::{
    encode_c2s_lcpacket, C2sFrameMode, ClientRxDecoder, C2S_LCPACKET_HEADER_SIZE,
    DEFAULT_MAX_FRAME_BODY_LEN,
};
use gserver_protocol::md5::compute_babeltime_md5;
use gserver_protocol::zlib::{zlib_compress, zlib_decompress};

/// Declaratively constructs a `DtoValue::Map` (`{}` when empty, or `"key" => value` entries).
#[macro_export]
macro_rules! dto_map {
    () => { $crate::protocol::DtoValue::Map(::std::collections::BTreeMap::new()) };
    ( $( $k:expr => $v:expr ),+ $(,)? ) => {{
        let mut map = ::std::collections::BTreeMap::new();
        $( map.insert(::std::convert::Into::<::std::string::String>::into($k), $crate::protocol::DtoValue::from($v)); )+
        $crate::protocol::DtoValue::Map(map)
    }};
}

/// Declaratively constructs a `DtoValue::Array` (`[]`, `[elem; count]`, or `[v1, v2, ...]`).
#[macro_export]
macro_rules! dto_arr {
    () => { $crate::protocol::DtoValue::Array(::std::vec::Vec::new()) };
    ( $elem:expr ; $n:expr ) => {
        $crate::protocol::DtoValue::Array((0..$n).map(|_| $crate::protocol::DtoValue::from($elem)).collect())
    };
    ( $( $v:expr ),+ $(,)? ) => {
        $crate::protocol::DtoValue::Array(::std::vec![ $( $crate::protocol::DtoValue::from($v) ),+ ])
    };
}

/// Declaratively constructs an `"ok"` response map (`{ "ret": "ok", ... }`).
#[macro_export]
macro_rules! dto_ok {
    () => { $crate::dto_map! { "ret" => "ok" } };
    ( $( $k:expr => $v:expr ),+ $(,)? ) => { $crate::dto_map! { "ret" => "ok", $( $k => $v, )+ } };
}

pub use crate::{dto_arr, dto_map, dto_ok};

/// Exact Client-to-Server RPC method names aliased from `gserver_rpc_catalog::net_const`.
pub mod rpc {
    pub use gserver_rpc_catalog::net_const::{
        // User, Bag, Hero & Formation
        CS_BAG_OPEN_GRID_BY_GOLD as BAG_OPEN_GRID_BY_GOLD, CS_BAG_SELL_ITEM as BAG_SELL_ITEMS,
        CS_FORMATION_GET_POSITION_INFO as FORMATION_GET_POSITION_INFO, CS_GET_ALL_HERO as HERO_GET_ALL,
        CS_GET_BAG_DATA as BAG_INFO, CS_GET_USER as USER_GET_USER,
        CS_USER_BEFORE_LOGIN_FIGHT as USER_BEFORE_LOGIN_BATTLE, CS_USER_CREATEUSER as USER_CREATE_USER,
        CS_USER_LOGIN as USER_LOGIN, CS_USER_USERLOGIN as USER_USER_LOGIN,
        // Town, RoyalCity & Castle
        CS_ROYALCITY_ENTER as ROYALCITY_ENTER, CS_ROYALCITY_LEAVE as ROYALCITY_LEAVE,
        CS_ROYALCITY_MOVE as ROYALCITY_MOVE, CS_TOWN_BUILDING_MAINBASE_INFO as HALL_GET_INFO,
        CS_TOWN_BUILDING_MARKET_GET_INFO as MARKET_GET_INFO, CS_TOWN_BUILDING_SMITHY_INFO as SMITHY_GET_INFO,
        CS_TOWN_BUILDING_WORKSHOP_INFO as WORKSHOP_GET_INFO, CS_TOWN_LIST as CASTLE_GET_INFO,
        SC_ROYALCITY_MOVE as TOWN_MOVE,
        // Chat
        CS_CHAT_CHAT_CROSS_SERVER_GET as CHAT_GET_CROSS_TEAM_LAST,
        CS_CHAT_CHAT_CROSS_SERVER_SEND as CHAT_SEND_CROSS_TEAM,
        CS_CHAT_COUNTRY_GET as CHAT_GET_COUNTRY_LAST, CS_CHAT_COUNTRY_SEND as CHAT_SEND_COUNTRY,
        CS_CHAT_GUILD_CROSS_SERVER_GET as CHAT_GET_CROSS_GUILD_LAST,
        CS_CHAT_GUILD_CROSS_SERVER_SEND as CHAT_SEND_CROSS_GUILD,
        CS_CHAT_GUILD_GET as CHAT_GET_GUILD_LAST, CS_CHAT_GUILD_SEND as CHAT_SEND_GUILD,
        CS_CHAT_PERSONAL_GET as CHAT_GET_PERSONAL_LAST, CS_CHAT_PERSONAL_SEND as CHAT_SEND_PERSONAL,
        CS_CHAT_TIMEMACHINE_GET as TM_CHAT_GET_CROSS_GUILD_LAST,
        CS_CHAT_TIMEMACHINE_SEND as TM_CHAT_SEND_CROSS_GUILD,
        CS_CHAT_WORLD_GET as CHAT_GET_WORLD_LAST, CS_CHAT_WORLD_SEND as CHAT_SEND_WORLD,
        // Mail & Reward Center
        CS_ACTIVITY_CODE_GIFTBYCODE as REWARD_GIFT_BY_CODE, CS_GET_MAIL_DATA as MAIL_GET_LIST,
        CS_MAIL_READALLMAIL as MAIL_READ_ALL, CS_REWARD_GETREWARDLIST as REWARD_GET_LIST,
        CS_REWARD_RECEIVEBYRIDARR as REWARD_RECEIVE_BY_RID_ARR, CS_SEND_MAIL as MAIL_SEND,
        // Shop & Tavern Raffle
        CS_RAFFLE_BUY_TOKEN as RAFFLE_BUY_TOKEN, CS_RAFFLE_FREE_AUTO as RAFFLE_AUTO,
        CS_RAFFLE_GET_INFO as RAFFLE_GET_INFO, CS_RAFFLE_LIMIT as RAFFLE_HOT,
        CS_RAFFLE_LIMIT_REFRESH as RAFFLE_REFRESH_HOT, CS_RAFFLE_LIMIT_TEN as RAFFLE_MULTI_HOT,
        CS_RAFFLE_MULTI as RAFFLE_MULTI, CS_RAFFLE_RECEIVE_LUCK_HERO as RAFFLE_RECEIVE_LUCK,
        CS_RAFFLE_RECEIVE_LUCK_HERO_HOT as RAFFLE_RECEIVE_LUCK_HOT,
        CS_RAFFLE_RECEIVE_LUCK_HERO_RED as RAFFLE_RECEIVE_LUCK_RED, CS_RAFFLE_RED as RAFFLE_RED,
        CS_RAFFLE_SIGNAL as RAFFLE_SINGLE, CS_SHOP_BUY as SHOP_BUY, CS_SHOP_GETINFO as SHOP_GET_INFO,
        CS_SHOP_REFRESH as SHOP_REFRESH,
        // PvE Campaign Copy & Battle Record
        CS_BATTLE_GETRECORD as BATTLE_GET_RECORD, CS_COPY_ATTACK as COPY_ATTACK,
        CS_COPY_CHAPTERINFO as COPY_GET_COPY, CS_COPY_FAIL as COPY_FAIL,
        CS_COPY_FETCH_FRESHER_REWARD as COPY_FETCH_FRESHER_REWARD,
        CS_COPY_GET_FIGHT_RECORD as COPY_GET_STRONGH_RECORD,
        CS_COPY_GET_POINT_BOX_REWARD as COPY_FETCH_BOX_REWARD,
        CS_COPY_GET_REWARD as COPY_FETCH_PRIZE, CS_COPY_LIST as COPY_GET_LIST,
        CS_COPY_ONE_KEY_RESET_ATK as COPY_ONE_KEY_RESET_ATK, CS_COPY_PASS as COPY_PASS,
        CS_COPY_RESET_ATK_NUM as COPY_RESET_ATK, CS_COPY_SWEEP as COPY_SWEEP,
        // Arena, Sign, Online & Activity
        CS_ACTIVITY_GETACTIVITYCONF as ACTIVITY_GET_CONF, CS_ACTIVITY_ONLINE_FETCH as ONLINE_FETCH,
        CS_ACTIVITY_ONLINE_GETINFO as ONLINE_GET_INFO,
        CS_ACTIVITY_SIGN_GAIN_VIP_SIGN_REWARD as SIGN_GAIN_VIP, CS_ACTIVITY_SIGN_GETINFO as SIGN_GET_INFO,
        CS_ACTIVITY_SIGN_SIGNTODAY as SIGN_TODAY, CS_ARENA_BUY_CHALLENGE_TIMES as ARENA_BUY_TIMES,
        CS_ARENA_CHALLENGE as ARENA_CHALLENGE, CS_ARENA_ENTARN_ARENA as ARENA_ENTER,
        CS_ARENA_FETCH_NUM_REWARD as ARENA_FETCH_NUM_REWARD, CS_ARENA_GET_BASE_DATA as ARENA_GET_BASE_DATA,
        CS_ARENA_LEAVE_ARENA as ARENA_LEAVE, CS_ARENA_QUICK_CHALLENGE as ARENA_QUICK_CHALLENGE,
        CS_ONLINE_OPEN_GETINFO as OPEN_ONLINE_GET_INFO, CS_ONLINE_OPEN_REVEIVE as OPEN_ONLINE_RECEIVE,
    };

    pub const CITY_ENTER: &str = "city.enter";
    pub const CITY_LEAVE: &str = "city.leave";
    pub const CITY_GET_ENTER_LIST: &str = "city.getEnterList";
    pub const SHOP_MULTI_REFRESH: &str = "shop.multiRefresh";
    pub const SHOP_QUICK_BUY: &str = "shop.quickBuy";
    pub const SHOP_AUTO_BUY: &str = "shop.normalShopAutoBuy";
}

/// Exact Server-to-Client push callback names aliased from `gserver_rpc_catalog::net_const`.
pub mod push {
    pub use gserver_rpc_catalog::net_const::{
        CS_BAG_PUSH_UPDATE as BAG_UPDATE, CS_SHOP_PUSH_UPDATE as SHOP_OPEN,
        RE_CHAT_MSG_PUSH as CHAT_GET_MSG, RE_TM_CHAT_MSG_PUSH as TM_CHAT_GET_MSG,
        SC_ACTIVITY_NEWCONF as ACTIVITY_NEW_CONF, SC_ARENA_DATA_REFRESH as ARENA_DATA_REFRESH,
        SC_HERO_NEW as HERO_NEW, SC_NEWMAIL as MAIL_NEW, SC_REWARD_NEW as REWARD_NEW,
        SC_ROYALCITY_ADDPLAYER as TOWN_ADD_PLAYER, SC_ROYALCITY_DELPLAYER as TOWN_DEL_PLAYER,
        SC_ROYALCITY_MOVE as TOWN_MOVE, SC_USER_UPDATEUSER as USER_UPDATE,
    };

    pub const USER_COPPER_OVER_MAX: &str = "re.user.copperOverMax";
    pub const SERVER_NOTIFY: &str = "SERVER_NOTIFY";
    pub const TOWN_UPDATE: &str = "town.update";
}

/// AMF3-compatible value representation used across GServer RPC tables and binary blobs.
#[derive(Clone, PartialEq)]
pub enum DtoValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<DtoValue>),
    Map(BTreeMap<String, DtoValue>),
}

impl fmt::Debug for DtoValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "Null"),
            Self::Bool(b) => write!(f, "Bool({b})"),
            Self::Int(i) => write!(f, "Int({i})"),
            Self::Float(v) => write!(f, "Float({v})"),
            Self::Str(s) => write!(f, "Str({s:?})"),
            Self::Bytes(b) => write!(f, "Bytes(len={})", b.len()),
            Self::Array(a) => f.debug_list().entries(a.iter()).finish(),
            Self::Map(m) => f.debug_map().entries(m.iter()).finish(),
        }
    }
}

impl DtoValue {
    pub fn map(entries: impl IntoIterator<Item = (impl Into<String>, DtoValue)>) -> Self {
        Self::Map(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn array(items: impl IntoIterator<Item = DtoValue>) -> Self {
        Self::Array(items.into_iter().collect())
    }

    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Float(v) => Some(*v as i64),
            Self::Str(s) => s.parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            Self::Str(s) => s.parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> { self.as_i64().and_then(|v| u32::try_from(v).ok()) }
    pub fn as_u64(&self) -> Option<u64> { self.as_i64().and_then(|v| u64::try_from(v).ok()) }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Self::Str(s) = self { Some(s.as_str()) } else { None }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Self::Bytes(b) = self { Some(b.as_slice()) } else { None }
    }

    pub fn as_array(&self) -> Option<&[DtoValue]> {
        if let Self::Array(a) = self { Some(a.as_slice()) } else { None }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, DtoValue>> {
        if let Self::Map(m) = self { Some(m) } else { None }
    }

    pub fn get(&self, key: &str) -> Option<&DtoValue> {
        self.as_map()?.get(key)
    }

    /// Losslessly converts [`DtoValue`] into [`gserver_protocol::Value`] (`Bytes` -> `0x0C` `ByteArray`, `Map` -> `0x0A` `Object`).
    pub fn to_protocol_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(*b),
            Self::Int(i) if (i64::from(AMF3_INT_MIN)..=i64::from(AMF3_INT_MAX)).contains(i) => {
                Value::Int(*i as i32)
            }
            Self::Int(i) => Value::Double(*i as f64),
            Self::Float(f) => Value::Double(*f),
            Self::Str(s) => Value::String(s.clone()),
            Self::Bytes(b) => Value::ByteArray(b.clone()),
            Self::Array(items) => {
                Value::dense_array(items.iter().map(Self::to_protocol_value).collect())
            }
            Self::Map(map) => {
                Value::map(map.iter().map(|(k, v)| (k.clone(), v.to_protocol_value())))
            }
        }
    }

    /// Converts a decoded [`gserver_protocol::Value`] into a [`DtoValue`].
    pub fn from_protocol_value(val: &Value) -> Self {
        match val {
            Value::Undefined | Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Int(i) => Self::Int(i64::from(*i)),
            Value::Double(d)
                if d.is_finite()
                    && d.fract() == 0.0
                    && !(f64::from(AMF3_INT_MIN)..=f64::from(AMF3_INT_MAX)).contains(d)
                    && *d >= (i64::MIN as f64)
                    && *d < (i64::MAX as f64) =>
            {
                Self::Int(*d as i64)
            }
            Value::Double(d) | Value::Date(d) => Self::Float(*d),
            Value::String(s) => Self::Str(s.clone()),
            Value::ByteString(b) | Value::ByteArray(b) | Value::Xml(b) | Value::XmlDoc(b) => {
                Self::Bytes(b.clone())
            }
            Value::Array { dense, assoc } if assoc.is_empty() => {
                Self::Array(dense.iter().map(Self::from_protocol_value).collect())
            }
            Value::Array { dense, assoc } => {
                let mut m = BTreeMap::new();
                for (idx, item) in dense.iter().enumerate() {
                    m.insert((idx + 1).to_string(), Self::from_protocol_value(item));
                }
                for (k, item) in assoc {
                    m.insert(k.clone(), Self::from_protocol_value(item));
                }
                Self::Map(m)
            }
            Value::Object(obj) => Self::Map(
                obj.iter()
                    .map(|(k, v)| (k.to_owned(), Self::from_protocol_value(v)))
                    .collect(),
            ),
        }
    }
}

impl From<&DtoValue> for Value { fn from(v: &DtoValue) -> Self { v.to_protocol_value() } }
impl From<DtoValue> for Value { fn from(v: DtoValue) -> Self { v.to_protocol_value() } }
impl From<&Value> for DtoValue { fn from(v: &Value) -> Self { Self::from_protocol_value(v) } }
impl From<Value> for DtoValue { fn from(v: Value) -> Self { Self::from_protocol_value(&v) } }

macro_rules! impl_dto_from {
    ($($ty:ty => |$v:ident| $expr:expr),+ $(,)?) => {
        $( impl From<$ty> for DtoValue { fn from($v: $ty) -> Self { $expr } } )+
    };
}

impl_dto_from! {
    bool => |v| Self::Bool(v),
    i32 => |v| Self::Int(i64::from(v)),
    i64 => |v| Self::Int(v),
    u32 => |v| Self::Int(i64::from(v)),
    u64 => |v| Self::Int(v as i64),
    usize => |v| Self::Int(v as i64),
    f64 => |v| Self::Float(v),
    &str => |v| Self::Str(v.to_owned()),
    String => |v| Self::Str(v),
    &String => |v| Self::Str(v.clone()),
}

/// Decoded client RPC request (`{ method, args, token, callback = { callbackName } }`).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequest {
    pub method: String,
    pub args: Vec<DtoValue>,
    pub callback_name: String,
    pub token: String,
}

impl RpcRequest {
    pub fn new(method: impl Into<String>, args: Vec<DtoValue>, req_id: u32) -> Self {
        let method_str = method.into();
        let callback_name = format!("{method_str}_{req_id}");
        Self { method: method_str, args, callback_name, token: req_id.to_string() }
    }

    pub fn to_dto(&self) -> DtoValue {
        dto_map! {
            "method" => &self.method,
            "args" => DtoValue::Array(self.args.clone()),
            "token" => &self.token,
            "callback" => dto_map! { "callbackName" => &self.callback_name },
        }
    }

    pub fn arg(&self, idx: usize) -> Option<&DtoValue> { self.args.get(idx) }
    pub fn arg_i64(&self, idx: usize) -> Option<i64> { self.arg(idx).and_then(DtoValue::as_i64) }
    pub fn arg_i64_or(&self, idx: usize, default: i64) -> i64 { self.arg_i64(idx).unwrap_or(default) }
    pub fn arg_i32(&self, idx: usize) -> Option<i32> { self.arg_i64(idx).map(|v| v as i32) }
    pub fn arg_i32_or(&self, idx: usize, default: i32) -> i32 { self.arg_i32(idx).unwrap_or(default) }
    pub fn arg_u32(&self, idx: usize) -> Option<u32> { self.arg(idx).and_then(DtoValue::as_u32) }
    pub fn arg_u32_or(&self, idx: usize, default: u32) -> u32 { self.arg_u32(idx).unwrap_or(default) }
    pub fn arg_u64(&self, idx: usize) -> Option<u64> { self.arg(idx).and_then(DtoValue::as_u64) }
    pub fn arg_u64_or(&self, idx: usize, default: u64) -> u64 { self.arg_u64(idx).unwrap_or(default) }
    pub fn arg_usize_or(&self, idx: usize, default: usize) -> usize {
        self.arg_u64(idx).map_or(default, |v| v as usize)
    }
    pub fn arg_str(&self, idx: usize) -> Option<&str> { self.arg(idx).and_then(DtoValue::as_str) }
    pub fn arg_str_or<'a>(&'a self, idx: usize, default: &'a str) -> &'a str {
        self.arg_str(idx).unwrap_or(default)
    }
    pub fn arg_bool(&self, idx: usize) -> Option<bool> { self.arg(idx).and_then(DtoValue::as_bool) }
    pub fn arg_bool_or(&self, idx: usize, default: bool) -> bool { self.arg_bool(idx).unwrap_or(default) }
    pub fn arg_u64_vec(&self, idx: usize) -> Vec<u64> {
        self.arg(idx)
            .and_then(DtoValue::as_array)
            .map(|arr| arr.iter().filter_map(DtoValue::as_u64).collect())
            .unwrap_or_default()
    }
}

/// Server RPC response envelope (`{ err = "ok", callback = { callbackName }, ret, time, token }`).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponse {
    pub err: String,
    pub callback_name: String,
    pub ret: DtoValue,
    pub time: u64,
    pub token: String,
}

impl RpcResponse {
    pub fn ok(req: &RpcRequest, ret: DtoValue, time: u64) -> Self {
        Self { err: "ok".to_owned(), callback_name: req.callback_name.clone(), ret, time, token: req.token.clone() }
    }

    pub fn error(req: &RpcRequest, err_code: impl Into<String>, time: u64) -> Self {
        Self { err: err_code.into(), callback_name: req.callback_name.clone(), ret: DtoValue::Null, time, token: req.token.clone() }
    }

    pub fn to_dto(&self) -> DtoValue {
        dto_map! {
            "err" => &self.err,
            "callback" => dto_map! { "callbackName" => &self.callback_name },
            "ret" => self.ret.clone(),
            "time" => self.time,
            "token" => &self.token,
        }
    }
}

/// Server-to-client asynchronous push envelope (`callbackName` = push channel, `err = "ok"`).
#[derive(Debug, Clone, PartialEq)]
pub struct PushPacket {
    pub callback_name: String,
    pub ret: DtoValue,
    pub time: u64,
}

impl PushPacket {
    pub fn new(callback_name: impl Into<String>, ret: DtoValue, time: u64) -> Self {
        Self { callback_name: callback_name.into(), ret, time }
    }

    pub fn to_dto(&self) -> DtoValue {
        dto_map! {
            "err" => "ok",
            "callback" => dto_map! { "callbackName" => &self.callback_name },
            "ret" => self.ret.clone(),
            "time" => self.time,
        }
    }
}

/// Bundles an RPC response + 0..N live pushes into a `multi = 1` batch envelope (`flag[3] != 0`).
pub fn bundle_multi_envelope(response: &RpcResponse, pushes: &[PushPacket], time: u64) -> DtoValue {
    let mut items = Vec::with_capacity(1 + pushes.len());
    items.push(response.to_dto());
    items.extend(pushes.iter().map(PushPacket::to_dto));
    dto_map! { "err" => "ok", "ret" => DtoValue::Array(items), "time" => time }
}

// ============================================================================
// Delegated AMF3 & LCPacket Codecs (`gserver-protocol`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    UnexpectedEof,
    UnsupportedMarker(u8),
    InvalidUtf8,
    InvalidHeader,
    ChecksumMismatch,
    DecompressionFailed,
    Protocol(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected EOF while decoding AMF3/frame"),
            Self::UnsupportedMarker(m) => write!(f, "unsupported AMF3 type marker 0x{m:02X}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 string in AMF3 stream"),
            Self::InvalidHeader => write!(f, "invalid LCPacket 24-byte header"),
            Self::ChecksumMismatch => write!(f, "LCPacket 16-byte BabelTime MD5 mismatch"),
            Self::DecompressionFailed => write!(f, "LCPacket RFC 1950 zlib decompression failed"),
            Self::Protocol(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CodecError {}

impl From<Amf3Error> for CodecError {
    fn from(err: Amf3Error) -> Self {
        match err {
            Amf3Error::UnexpectedEof { .. } => Self::UnexpectedEof,
            Amf3Error::InvalidMarker { marker, .. } => Self::UnsupportedMarker(marker),
            Amf3Error::InvalidUtf8 { .. } => Self::InvalidUtf8,
            other => Self::Protocol(other.to_string()),
        }
    }
}

/// Encodes a [`DtoValue`] into AMF3 bytes by delegating to [`gserver_protocol::amf3::encode`].
pub fn encode_amf3(val: &DtoValue) -> Vec<u8> {
    amf3::encode(&val.to_protocol_value()).expect("DtoValue AMF3 encoding within protocol limits")
}

/// Decodes an AMF3 byte buffer into a [`DtoValue`] by delegating to [`gserver_protocol::amf3::decode`].
pub fn decode_amf3(buf: &[u8]) -> Result<DtoValue, CodecError> {
    let (proto_val, _) = amf3::decode(buf)?;
    Ok(DtoValue::from_protocol_value(&proto_val))
}

/// 24-byte `LCPacket` wire header (`LCPacket.h` / `Network.lua`), delegating BabelTime MD5
/// checksums, RFC 1950 Zlib compression, and C2S framing to `gserver_protocol::{frame, md5, zlib}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LcPacketFrame {
    pub flags: [u8; 4],
    pub checksum: [u8; 16],
    pub payload: Vec<u8>,
}

impl LcPacketFrame {
    pub fn uncompressed(payload: Vec<u8>, multi: bool) -> Self {
        let checksum = compute_babeltime_md5(&payload);
        Self { flags: [0, 1, 0, u8::from(multi)], checksum, payload }
    }

    pub fn compressed(raw_payload: &[u8], multi: bool) -> Self {
        let payload = zlib_compress(raw_payload);
        let checksum = compute_babeltime_md5(&payload);
        Self { flags: [0, 1, 1, u8::from(multi)], checksum, payload }
    }

    pub const fn is_compressed(&self) -> bool { self.flags[2] != 0 }

    pub fn decompress_payload(&self) -> Result<Vec<u8>, CodecError> {
        if self.is_compressed() {
            zlib_decompress(&self.payload).map_err(|_| CodecError::DecompressionFailed)
        } else {
            Ok(self.payload.clone())
        }
    }

    pub fn encode_c2s(amf3_payload: &[u8], compress: bool) -> Result<Vec<u8>, CodecError> {
        encode_c2s_lcpacket(amf3_payload, compress).map_err(|e| CodecError::Protocol(e.to_string()))
    }

    pub fn decode_c2s(wire_bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
        let mut rx = ClientRxDecoder::new(C2sFrameMode::LcPacket24);
        rx.feed(wire_bytes);
        match rx.next_frame() {
            Ok(Some(frame)) => Ok(frame.amf3_payload),
            Ok(None) => Err(CodecError::UnexpectedEof),
            Err(e) => Err(CodecError::Protocol(e.to_string())),
        }
    }

    pub fn to_wire_bytes(&self) -> Vec<u8> {
        let checksum = if self.checksum == [0u8; 16] {
            compute_babeltime_md5(&self.payload)
        } else {
            self.checksum
        };
        let mut out = Vec::with_capacity(C2S_LCPACKET_HEADER_SIZE + self.payload.len());
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.flags);
        out.extend_from_slice(&checksum);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() < C2S_LCPACKET_HEADER_SIZE {
            return Err(CodecError::InvalidHeader);
        }
        let body_len_u32 = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if body_len_u32 > DEFAULT_MAX_FRAME_BODY_LEN {
            return Err(CodecError::InvalidHeader);
        }
        let body_len = body_len_u32 as usize;
        if bytes.len() < C2S_LCPACKET_HEADER_SIZE + body_len {
            return Err(CodecError::UnexpectedEof);
        }
        let flags = [bytes[4], bytes[5], bytes[6], bytes[7]];
        let mut checksum = [0u8; 16];
        checksum.copy_from_slice(&bytes[8..C2S_LCPACKET_HEADER_SIZE]);
        let body = &bytes[C2S_LCPACKET_HEADER_SIZE..C2S_LCPACKET_HEADER_SIZE + body_len];
        if checksum != [0u8; 16] && checksum != compute_babeltime_md5(body) {
            return Err(CodecError::ChecksumMismatch);
        }
        Ok(Self { flags, checksum, payload: body.to_vec() })
    }
}
