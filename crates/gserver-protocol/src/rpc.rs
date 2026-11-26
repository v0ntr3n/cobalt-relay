//! High-level GServer (`vnsg2`) RPC Request, Response, Push Notification (`re.*`),
//! Heartbeat (`err = "ping"`), and Multi-Batch envelope codecs.
//!
//! Verified directly against `Network.lua`, `SuperMapNetwork.lua`, `LoginProxy.lua`,
//! `NetConst.lua`, `CityData.lua`, and `live_capture_filtered.log`.

use crate::amf3::{self, Amf3Object, Value};
use crate::error::{EnvelopeError, ProtocolError};
use crate::frame::{
    encode_c2s_lcpacket, split_length_prefixed_subpackets, S2cFrame, ServerTxEncoder,
};

/// Maximum sparse 1-based Lua numeric index reconstructed into [`RpcRequest::args`].
const MAX_RECONSTRUCTED_SPARSE_INDEX: usize = 1024;

/// Parses a canonical non-negative integer key (`"0"`, `"1"`, `"2"`, ..., rejecting `"-1"` or `"01"`).
fn parse_canonical_usize_key(key: &str) -> Option<usize> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<usize>().ok()
}

/// Client-to-Server RPC call decoded from `Network:sendMsg` / `sendStatMsg` / `sendNewStatMsg`.
///
/// Exact Lua structure in `Network.lua:249-254`:
/// ```lua
/// local request = {
///     method   = method,                              -- e.g. "user.login"
///     args     = args or {},                          -- 1-based dense Lua array, 0-indexed table, map, or ""
///     token    = self.mToken,                         -- starts at "0"
///     callback = { callbackName = method .. "_" .. mId } -- or "user.stat"
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequest {
    /// Target RPC method name (e.g. `"user.login"`, `"user.getUserBeforLogin"`, `"user.userLogin"`).
    pub method: String,
    /// Positional RPC arguments array (`{}` empty when omitted or `""`, 1-based dense + sparse reconstructed).
    pub args: Vec<Value>,
    /// Session token string (initialized to `"0"` in `Network:ctor`, updated via S2C `token`).
    pub token: String,
    /// Client callback correlation key (`request.callback.callbackName`, e.g. `"user.login_2"`).
    pub callback_name: String,
    /// Optional Lua 0-indexed argument (`args[0]`, encoded by `Amf3Module.cpp` under string key `"0"`).
    pub arg_zero: Option<Value>,
    /// Named / associative entries carried on `args` (`{ boxId = 1 }`, `{ brid = ..., token = ... }`, etc.).
    pub named_args: Vec<(String, Value)>,
    /// Original decoded AMF3 `args` value preserved verbatim for lossless round-tripping and lookup.
    pub raw_args: Option<Value>,
}

impl RpcRequest {
    /// Constructs a new C2S [`RpcRequest`].
    pub fn new(
        method: impl Into<String>,
        args: Vec<Value>,
        token: impl Into<String>,
        callback_name: impl Into<String>,
    ) -> Self {
        let raw_args = Some(Value::dense_array(args.clone()));
        Self {
            method: method.into(),
            args,
            token: token.into(),
            callback_name: callback_name.into(),
            arg_zero: None,
            named_args: Vec::new(),
            raw_args,
        }
    }

    /// Constructs a standard numbered RPC request matching `Network:sendMsg` (`<method>_<seq_id>`).
    pub fn with_seq_id(
        method: impl Into<String>,
        seq_id: u64,
        args: Vec<Value>,
        token: impl Into<String>,
    ) -> Self {
        let method_str = method.into();
        let callback_name = format!("{method_str}_{seq_id}");
        let raw_args = Some(Value::dense_array(args.clone()));
        Self {
            method: method_str,
            args,
            token: token.into(),
            callback_name,
            arg_zero: None,
            named_args: Vec::new(),
            raw_args,
        }
    }

    /// Returns the Lua 0-indexed element `args[0]` if present (e.g. `args[0] = gid; args[1] = item_id; args[2] = num`).
    pub fn arg_zero(&self) -> Option<&Value> {
        self.arg_zero.as_ref().or_else(|| {
            self.raw_args
                .as_ref()
                .and_then(|raw| raw.get_field("0"))
        })
    }

    /// Looks up a Lua argument using exact 0-based / 1-based Lua table indexing:
    /// - `lua_idx == 0`: returns Lua `args[0]` (`assoc["0"]` / `object["0"]`).
    /// - `lua_idx >= 1`: returns Lua `args[lua_idx]` (`dense[lua_idx - 1]` or sparse `assoc/object[tostring(lua_idx)]`).
    pub fn arg_at_lua0(&self, lua_idx: usize) -> Option<&Value> {
        if lua_idx == 0 {
            return self.arg_zero();
        }
        match &self.raw_args {
            Some(Value::Array { dense, assoc }) => {
                if lua_idx <= dense.len() {
                    Some(&dense[lua_idx - 1])
                } else {
                    let key = lua_idx.to_string();
                    assoc
                        .iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, v)| v)
                        .or_else(|| self.args.get(lua_idx - 1))
                }
            }
            Some(Value::Object(obj)) => {
                let key = lua_idx.to_string();
                obj.get(&key)
            }
            _ => self.args.get(lua_idx - 1),
        }
    }

    /// Looks up a named/associative argument on `args` (e.g. `boxId`, `brid`, `token`).
    pub fn named_arg(&self, key: &str) -> Option<&Value> {
        self.named_args
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .or_else(|| self.raw_args.as_ref().and_then(|raw| raw.get_field(key)))
    }

    /// Converts this [`RpcRequest`] into an AMF3 [`Value::Object`] matching `Amf3Module.cpp`.
    pub fn to_amf3_value(&self) -> Value {
        let callback_obj = Value::map([("callbackName", Value::String(self.callback_name.clone()))]);
        let args_val = match &self.raw_args {
            Some(raw) => raw.clone(),
            None if self.arg_zero.is_none() && self.named_args.is_empty() => {
                Value::dense_array(self.args.clone())
            }
            None => {
                let mut assoc = Vec::new();
                if let Some(z) = &self.arg_zero {
                    assoc.push((String::from("0"), z.clone()));
                }
                for (k, v) in &self.named_args {
                    if k != "0" {
                        assoc.push((k.clone(), v.clone()));
                    }
                }
                Value::Array {
                    dense: self.args.clone(),
                    assoc,
                }
            }
        };
        Value::map([
            ("method", Value::String(self.method.clone())),
            ("args", args_val),
            ("token", Value::String(self.token.clone())),
            ("callback", callback_obj),
        ])
    }

    /// Serializes this [`RpcRequest`] into raw AMF3 bytes.
    pub fn to_amf3_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(amf3::encode(&self.to_amf3_value())?)
    }

    /// Serializes and wraps this [`RpcRequest`] in a 24-byte BabelTime `LCPacket` (`mConn:send(data, true)`).
    pub fn to_lcpacket_frame(&self, compress: bool) -> Result<Vec<u8>, ProtocolError> {
        let amf3_bytes = self.to_amf3_bytes()?;
        encode_c2s_lcpacket(&amf3_bytes, compress)
    }

    fn normalize_args_field(
        raw_field: Option<&Value>,
    ) -> (Vec<Value>, Option<Value>, Vec<(String, Value)>, Option<Value>) {
        match raw_field {
            None | Some(Value::Null | Value::Undefined) => {
                (Vec::new(), None, Vec::new(), raw_field.cloned())
            }
            Some(raw @ Value::Array { dense, assoc }) => {
                let mut args = dense.clone();
                let mut arg_zero = None;
                let mut named_args = Vec::new();
                let mut max_pos_idx = args.len();

                for (k, v) in assoc {
                    if let Some(idx) = parse_canonical_usize_key(k) {
                        if idx == 0 {
                            arg_zero = Some(v.clone());
                        } else if idx <= MAX_RECONSTRUCTED_SPARSE_INDEX {
                            if idx > max_pos_idx {
                                max_pos_idx = idx;
                            }
                            if args.len() < idx {
                                args.resize(idx, Value::Null);
                            }
                            if matches!(args[idx - 1], Value::Null | Value::Undefined)
                                || idx > dense.len()
                            {
                                args[idx - 1] = v.clone();
                            }
                        } else {
                            named_args.push((k.clone(), v.clone()));
                        }
                    } else {
                        named_args.push((k.clone(), v.clone()));
                    }
                }

                // When Lua passes `{ [0] = gid }` encoded as an associative array with no 1-based keys,
                // also expose `gid` at `args[0]` so positional callers do not drop the sole argument.
                if dense.is_empty() && max_pos_idx == 0 {
                    if let Some(z) = &arg_zero {
                        args.push(z.clone());
                    }
                }

                (args, arg_zero, named_args, Some(raw.clone()))
            }
            Some(raw @ Value::Object(obj)) => {
                let mut arg_zero = None;
                let mut pos_entries: Vec<(usize, Value)> = Vec::new();
                let mut named_args = Vec::new();
                let mut max_pos_idx = 0usize;

                for (k, v) in obj.iter() {
                    if let Some(idx) = parse_canonical_usize_key(k) {
                        if idx == 0 {
                            arg_zero = Some(v.clone());
                        } else if idx <= MAX_RECONSTRUCTED_SPARSE_INDEX {
                            if idx > max_pos_idx {
                                max_pos_idx = idx;
                            }
                            pos_entries.push((idx, v.clone()));
                        } else {
                            named_args.push((k.to_owned(), v.clone()));
                        }
                    } else {
                        named_args.push((k.to_owned(), v.clone()));
                    }
                }

                let mut args = if max_pos_idx > 0 {
                    let mut vec = vec![Value::Null; max_pos_idx];
                    for (idx, v) in pos_entries {
                        vec[idx - 1] = v;
                    }
                    vec
                } else if let Some(z) = &arg_zero {
                    // `{ [0] = gid }` (`#t == 0` in Lua -> encoded as `0x0A` Object with key `"0"`)
                    vec![z.clone()]
                } else {
                    Vec::new()
                };

                // If `args` is purely a named map like `{ boxId = 1 }` or `{ brid = 10, token = "x" }`,
                // keep `args` containing `[Value::Object(obj)]` ONLY if callers inspect `args[0]`?
                // Wait: in Lua `sendReceiveNewHero` passes `{ boxId = 1 }` directly as `args` (`Network:sendMsg(m, cb, { boxId = 1 })`),
                // whereas other RPCs pass `{{ boxId = 1 }}`! Exposing `named_args` AND if `args.is_empty() && arg_zero.is_none() && !named_args.is_empty()`
                // keeping `args` empty while `named_arg("boxId")` / `raw_args` has the map — OR wait:
                // Let's check! What if a handler checks `req.args.first().and_then(as_map)` vs `req.named_arg("boxId")`?
                // Wait: if `{ brid = 123, token = "abc" }` is passed directly as `args`, `req.named_arg("brid")` returns `123`!
                // What if `args` stays empty when there are only named keys? Let's check Requirement 1:
                // "Handle args = Value::Object(map) (when Lua passes { [0] = gid } or { brid = ..., token = ... } or { nil, x }!). Reconstruct positional args if numeric string keys "0", "1", "2" exist, AND expose arg_zero / named_args (or preserve a raw_args: Option<Value> field on RpcRequest with helper methods arg_at_lua0(&self, idx) / named_arg(&self, key))"
                let _ = &mut args;

                (args, arg_zero, named_args, Some(raw.clone()))
            }
            Some(raw) => {
                // Gracefully handle `args = ""` (`CityData.lua:74` `"city.enterShadowInfo"`) -> empty `args`,
                // and wrap other scalar non-array values without failing envelope decoding.
                if raw.as_str() == Some("") {
                    (Vec::new(), None, Vec::new(), Some(raw.clone()))
                } else {
                    (vec![raw.clone()], None, Vec::new(), Some(raw.clone()))
                }
            }
        }
    }

    /// Parses an [`RpcRequest`] from a decoded AMF3 [`Value`].
    pub fn from_amf3_value(val: &Value) -> Result<Self, ProtocolError> {
        if !matches!(val, Value::Object(_) | Value::Array { .. }) {
            return Err(EnvelopeError::ExpectedMap.into());
        }

        let method = val
            .get_field("method")
            .ok_or(EnvelopeError::MissingField("method"))?
            .as_str()
            .ok_or(EnvelopeError::InvalidFieldType {
                field: "method",
                expected: "String",
            })?
            .to_owned();

        let (args, arg_zero, named_args, raw_args) =
            Self::normalize_args_field(val.get_field("args"));

        let token = match val.get_field("token") {
            None | Some(Value::Null | Value::Undefined) => String::from("0"),
            Some(v) => v
                .as_str()
                .ok_or(EnvelopeError::InvalidFieldType {
                    field: "token",
                    expected: "String",
                })?
                .to_owned(),
        };

        let callback_val = val
            .get_field("callback")
            .ok_or(EnvelopeError::MissingField("callback"))?;
        let callback_name = match callback_val {
            Value::String(s) => s.clone(),
            other => other
                .get_field("callbackName")
                .ok_or(EnvelopeError::MissingField("callback.callbackName"))?
                .as_str()
                .ok_or(EnvelopeError::InvalidFieldType {
                    field: "callback.callbackName",
                    expected: "String",
                })?
                .to_owned(),
        };

        Ok(Self {
            method,
            args,
            token,
            callback_name,
            arg_zero,
            named_args,
            raw_args,
        })
    }

    /// Decodes an [`RpcRequest`] directly from raw AMF3 bytes.
    pub fn from_amf3_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (val, _) = amf3::decode(bytes)?;
        Self::from_amf3_value(&val)
    }
}

/// How individual sub-envelopes are represented inside `data.ret[1..N]` when `multi == 1`.
///
/// In `Network.lua:114-121`:
/// ```lua
/// if multi ~= 0 then
///     for i = 1, #data.ret do
///         if type(data.ret[i]) == "table" then
///             rets[i] = data.ret[i]
///         else
///             rets[i] = amf3.decode(data.ret[i])
///         end
///     end
/// end
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MultiItemEncoding {
    /// Encodes each sub-envelope as nested AMF3 bytes wrapped in `0x06` (`Value::ByteString`),
    /// matching BabelTime `lcserver` + PHP `amfext`.
    #[default]
    NestedAmf3ByteString,
    /// Encodes each sub-envelope as nested AMF3 bytes wrapped in `0x0C` (`Value::ByteArray`),
    /// which `Amf3Module.cpp` also pushes as a Lua string for `amf3.decode(data.ret[i])`.
    NestedAmf3ByteArray,
    /// Embeds each sub-envelope directly as an inline AMF3 `0x0A` `Object` (`type == "table"` branch).
    InlineObject,
}

/// Server-to-Client message envelope (`Network:onBodyData`).
///
/// Covers all three S2C message roles used by GServer:
/// 1. **RPC Response**: `err = "ok"` (or error code string), `callback.callbackName = "<method>_<id>"`,
///    `ret = <payload>`, plus optional `token` and `time` (Unix seconds).
/// 2. **Server Push Notification** (`regPushCallback`): `err = "ok"`,
///    `callback.callbackName = "re.<module>.<event>"` (e.g. `"re.mail.newMail"`,
///    `"re.chat.getMsg"`, `"re.userLogin.otherDeviceLogin"`, `"user.updateUser"`, or `"dummy"`),
///    `ret = <push payload>`.
/// 3. **Server Ping / Keepalive**: `err = "ping"` (`Network.lua:142` skips callback dispatch when
///    `ret.err == "ping"`).
#[derive(Debug, Clone, PartialEq)]
pub struct S2cEnvelope {
    /// Status string: `"ok"`, `"ping"`, or a server error identifier.
    pub err: String,
    /// Callback identifier (`callback.callbackName`), optional only when `err == "ping"`.
    pub callback_name: Option<String>,
    /// Return payload or push data (`ret` field).
    pub ret: Value,
    /// Optional updated session token (`data.token` updates `Network.mToken`).
    pub token: Option<String>,
    /// Optional server Unix timestamp in seconds (`data.time` updates `g.timeManager` offset).
    pub time: Option<i64>,
}

impl S2cEnvelope {
    /// Builds a successful (`err = "ok"`) RPC response matching a client's `callback_name`.
    pub fn ok_response(callback_name: impl Into<String>, ret: Value) -> Self {
        Self {
            err: String::from("ok"),
            callback_name: Some(callback_name.into()),
            ret,
            token: None,
            time: None,
        }
    }

    /// Builds an error RPC response (`err != "ok"`).
    pub fn error_response(callback_name: impl Into<String>, err: impl Into<String>) -> Self {
        Self {
            err: err.into(),
            callback_name: Some(callback_name.into()),
            ret: Value::Null,
            token: None,
            time: None,
        }
    }

    /// Builds a server push notification (`re.*` or `user.updateUser`, where `err == "ok"` and
    /// `callback.callbackName` equals the push topic registered via `Network:regPushCallback`).
    pub fn push(push_method: impl Into<String>, payload: Value) -> Self {
        Self {
            err: String::from("ok"),
            callback_name: Some(push_method.into()),
            ret: payload,
            token: None,
            time: None,
        }
    }

    /// Builds a server keepalive ping envelope (`{ err = "ping" }`).
    pub fn ping() -> Self {
        Self {
            err: String::from("ping"),
            callback_name: None,
            ret: Value::Null,
            token: None,
            time: None,
        }
    }

    /// Attaches a session `token` update (`self.mToken = data.token` in `Network.lua`).
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Attaches a server Unix `time` (seconds) update (`self.mOffset = data.time - os.time()`).
    pub fn with_time(mut self, unix_seconds: i64) -> Self {
        self.time = Some(unix_seconds);
        self
    }

    /// Returns `true` if this envelope is a server keepalive ping (`err == "ping"`).
    pub fn is_ping(&self) -> bool {
        self.err == "ping"
    }

    /// Returns `true` if this envelope targets a `re.*` or `user.updateUser` push handler.
    pub fn is_push_topic(&self) -> bool {
        matches!(
            self.callback_name.as_deref(),
            Some(name) if name.starts_with("re.") || name == "user.updateUser"
        )
    }

    /// Converts this envelope into an AMF3 [`Value::Object`].
    pub fn to_amf3_value(&self) -> Value {
        let mut obj = Amf3Object::anonymous_dynamic();
        obj.insert("err", Value::String(self.err.clone()));
        if let Some(cb) = &self.callback_name {
            obj.insert(
                "callback",
                Value::map([("callbackName", Value::String(cb.clone()))]),
            );
        }
        if !matches!(self.ret, Value::Undefined) && !(self.is_ping() && matches!(self.ret, Value::Null))
        {
            obj.insert("ret", self.ret.clone());
        }
        if let Some(tok) = &self.token {
            obj.insert("token", Value::String(tok.clone()));
        }
        if let Some(ts) = self.time {
            obj.insert("time", Value::from(ts));
        }
        Value::Object(obj)
    }

    /// Encodes this envelope into raw AMF3 bytes.
    pub fn to_amf3_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(amf3::encode(&self.to_amf3_value())?)
    }

    /// Parses a single [`S2cEnvelope`] from a decoded AMF3 [`Value`].
    pub fn from_amf3_value(val: &Value) -> Result<Self, ProtocolError> {
        if !matches!(val, Value::Object(_) | Value::Array { .. }) {
            return Err(EnvelopeError::ExpectedMap.into());
        }

        let err = val
            .get_field("err")
            .ok_or(EnvelopeError::MissingField("err"))?
            .as_str()
            .ok_or(EnvelopeError::InvalidFieldType {
                field: "err",
                expected: "String",
            })?
            .to_owned();

        let callback_name = match val.get_field("callback") {
            None | Some(Value::Null | Value::Undefined) => {
                if err == "ping" {
                    None
                } else {
                    return Err(EnvelopeError::MissingField("callback").into());
                }
            }
            Some(Value::String(s)) => Some(s.clone()),
            Some(cb_obj) => Some(
                cb_obj
                    .get_field("callbackName")
                    .ok_or(EnvelopeError::MissingField("callback.callbackName"))?
                    .as_str()
                    .ok_or(EnvelopeError::InvalidFieldType {
                        field: "callback.callbackName",
                        expected: "String",
                    })?
                    .to_owned(),
            ),
        };

        let ret = val.get_field("ret").cloned().unwrap_or(Value::Null);

        let token = match val.get_field("token") {
            None | Some(Value::Null | Value::Undefined) => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or(EnvelopeError::InvalidFieldType {
                        field: "token",
                        expected: "String",
                    })?
                    .to_owned(),
            ),
        };

        let time = match val.get_field("time") {
            None | Some(Value::Null | Value::Undefined) => None,
            Some(v) => Some(v.as_i64().ok_or(EnvelopeError::InvalidFieldType {
                field: "time",
                expected: "Int or integral Double",
            })?),
        };

        Ok(Self {
            err,
            callback_name,
            ret,
            token,
            time,
        })
    }

    /// Decodes a single [`S2cEnvelope`] from raw AMF3 bytes.
    pub fn from_amf3_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (val, _) = amf3::decode(bytes)?;
        Self::from_amf3_value(&val)
    }
}

/// Decoded S2C delivery matching `Network:onBodyData` (supporting both `multi == 0` and `multi != 0`).
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedS2cBatch {
    /// Outer session token (`data.token`), if updated by the server.
    pub token: Option<String>,
    /// Outer server Unix timestamp (`data.time`), if included by the server.
    pub time: Option<i64>,
    /// Ordered list of unpacked S2C envelopes (`rets[1..#rets]` in `Network.lua`).
    pub items: Vec<S2cEnvelope>,
}

/// Encodes a batch of [`S2cEnvelope`] items into the exact `multi = 1` outer AMF3 payload expected
/// by `Network.lua:113-130`:
/// `{ err = "ok", ret = [ item_1, item_2, ... ], token? = ..., time? = ... }`.
pub fn encode_s2c_multi_batch_amf3(
    items: &[S2cEnvelope],
    token: Option<&str>,
    time: Option<i64>,
    encoding: MultiItemEncoding,
) -> Result<Vec<u8>, ProtocolError> {
    if items.is_empty() {
        return Err(EnvelopeError::EmptyMultiBatch.into());
    }

    let mut dense = Vec::with_capacity(items.len());
    for item in items {
        match encoding {
            MultiItemEncoding::NestedAmf3ByteString => {
                dense.push(Value::ByteString(item.to_amf3_bytes()?));
            }
            MultiItemEncoding::NestedAmf3ByteArray => {
                dense.push(Value::ByteArray(item.to_amf3_bytes()?));
            }
            MultiItemEncoding::InlineObject => {
                dense.push(item.to_amf3_value());
            }
        }
    }

    let mut outer = Amf3Object::anonymous_dynamic();
    outer.insert("err", Value::String(String::from("ok")));
    outer.insert("ret", Value::dense_array(dense));
    if let Some(tok) = token {
        outer.insert("token", Value::String(tok.to_owned()));
    }
    if let Some(ts) = time {
        outer.insert("time", Value::from(ts));
    }

    Ok(amf3::encode(&Value::Object(outer))?)
}

/// Encodes a `multi = 1` S2C 8-byte TCP frame ready to transmit over the socket.
pub fn encode_s2c_multi_frame(
    tx: &ServerTxEncoder,
    items: &[S2cEnvelope],
    token: Option<&str>,
    time: Option<i64>,
    zipped: bool,
    encoding: MultiItemEncoding,
) -> Result<Vec<u8>, ProtocolError> {
    let outer = encode_s2c_multi_batch_amf3(items, token, time, encoding)?;
    tx.encode_multi_amf3_payload(&outer, zipped)
}

/// Encodes a single (`multi = 0`) S2C 8-byte TCP frame ready to transmit over the socket.
pub fn encode_s2c_single_frame(
    tx: &ServerTxEncoder,
    envelope: &S2cEnvelope,
    zipped: bool,
) -> Result<Vec<u8>, ProtocolError> {
    let amf3_bytes = envelope.to_amf3_bytes()?;
    tx.encode_single(&amf3_bytes, zipped)
}

/// Unpacks a decoded [`S2cFrame`] into a [`DecodedS2cBatch`] following `Network:onBodyData`
/// (with automatic fallback for raw `[4B BE subLen][AMF3]` sub-packets when `multi != 0` and
/// byte 0 is `0x00` length prefix rather than an AMF3 Object/Array marker).
pub fn decode_s2c_frame_batch(frame: &S2cFrame) -> Result<DecodedS2cBatch, ProtocolError> {
    if !frame.flags.is_multi() {
        let env = S2cEnvelope::from_amf3_bytes(&frame.payload)?;
        return Ok(DecodedS2cBatch {
            token: env.token.clone(),
            time: env.time,
            items: vec![env],
        });
    }

    // Check if payload is an outer AMF3 Object/Array (`0x0A` or `0x09`, matching `Network.lua`)
    // vs raw concatenated `[4B BE subLen][AMF3 subPayload]` sub-packets.
    let first_byte = frame.payload.first().copied().unwrap_or(0);
    if first_byte == amf3::MARKER_OBJECT || first_byte == amf3::MARKER_ARRAY {
        let (outer, _) = amf3::decode(&frame.payload)?;
        let token = match outer.get_field("token") {
            None | Some(Value::Null | Value::Undefined) => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or(EnvelopeError::InvalidFieldType {
                        field: "token",
                        expected: "String",
                    })?
                    .to_owned(),
            ),
        };
        let time = match outer.get_field("time") {
            None | Some(Value::Null | Value::Undefined) => None,
            Some(v) => Some(v.as_i64().ok_or(EnvelopeError::InvalidFieldType {
                field: "time",
                expected: "Int or integral Double",
            })?),
        };

        let ret_arr = match outer.get_field("ret") {
            Some(Value::Array { dense, .. }) => dense,
            Some(_) => {
                return Err(EnvelopeError::InvalidFieldType {
                    field: "ret",
                    expected: "Array",
                }
                .into())
            }
            None => return Err(EnvelopeError::MissingField("ret").into()),
        };

        if ret_arr.is_empty() {
            return Err(EnvelopeError::EmptyMultiBatch.into());
        }

        let mut items = Vec::with_capacity(ret_arr.len());
        for elem in ret_arr {
            match elem {
                Value::Object(_) | Value::Array { .. } => {
                    items.push(S2cEnvelope::from_amf3_value(elem)?);
                }
                other => {
                    let raw = other.as_bytes().ok_or(EnvelopeError::InvalidFieldType {
                        field: "ret[i]",
                        expected: "table or AMF3 byte string",
                    })?;
                    items.push(S2cEnvelope::from_amf3_bytes(raw)?);
                }
            }
        }

        Ok(DecodedS2cBatch { token, time, items })
    } else {
        let slices = split_length_prefixed_subpackets(&frame.payload)?;
        if slices.is_empty() {
            return Err(EnvelopeError::EmptyMultiBatch.into());
        }
        let mut items = Vec::with_capacity(slices.len());
        for sub in slices {
            items.push(S2cEnvelope::from_amf3_bytes(sub)?);
        }
        Ok(DecodedS2cBatch {
            token: items.last().and_then(|e| e.token.clone()),
            time: items.last().and_then(|e| e.time),
            items,
        })
    }
}
