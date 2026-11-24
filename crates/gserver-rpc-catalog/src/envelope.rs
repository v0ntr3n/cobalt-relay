//! `Network.lua` RPC request and response envelope structures.
//!
//! ### Request Envelope (`Network.lua:sendMsg`)
//! ```lua
//! {
//!   method   = "user.getUser",
//!   args     = {}, -- 1-based dense Lua array (AMF3 0x09)
//!   token    = "0", -- updated from server responses
//!   callback = { callbackName = "user.getUser_5" } -- mId starts at 1, pre-incremented -> first RPC is _2
//! }
//! ```
//! Fire-and-forget telemetry RPCs (`user.stat` and `user.guideEvent`) omit the `_<mId>` suffix and
//! send `callbackName = "user.stat"` / `"user.guideEvent"`.
//!
//! ### Response Envelope (`Network.lua:onBodyData` / `TimeManager.lua`)
//! ```lua
//! {
//!   err      = "ok",
//!   callback = { callbackName = "user.getUser_5" },
//!   ret      = <RPC-specific payload>,
//!   time     = 1795496852, -- Unix seconds; syncs TimeManager server offset
//!   token    = "sess_0001" -- stored in Network._token for subsequent requests
//! }
//! ```

use crate::amf3::{decode_amf3, encode_amf3, Amf3Error, Amf3Value};
use crate::packet::{ClientFrame, PacketError, ServerFrame};

/// Decoded Client -> Server RPC request envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequestEnvelope {
    pub method: String,
    pub args: Vec<Amf3Value>,
    pub token: String,
    pub callback_name: String,
}

impl RpcRequestEnvelope {
    /// Construct a sequenced RPC request matching `Network.lua` (`<method>_<msg_id>`).
    /// Note: `Network.lua` initializes `mId = 1` and increments before sending, so the first
    /// sequenced RPC on a fresh connection uses `msg_id = 2` (`"user.login_2"`).
    pub fn new_sequenced(
        method: impl Into<String>,
        args: Vec<Amf3Value>,
        token: impl Into<String>,
        msg_id: u64,
    ) -> Self {
        let method = method.into();
        let callback_name = format!("{}_{}", method, msg_id);
        Self {
            method,
            args,
            token: token.into(),
            callback_name,
        }
    }

    /// Construct a fire-and-forget telemetry RPC (`user.stat` or `user.guideEvent`), which sets
    /// `callback.callbackName` equal to `method` without a numeric suffix.
    pub fn new_telemetry(
        method: impl Into<String>,
        args: Vec<Amf3Value>,
        token: impl Into<String>,
    ) -> Self {
        let method = method.into();
        let callback_name = method.clone();
        Self {
            method,
            args,
            token: token.into(),
            callback_name,
        }
    }

    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([
            ("method", Amf3Value::String(self.method.clone())),
            ("args", Amf3Value::Array(self.args.clone())),
            ("token", Amf3Value::String(self.token.clone())),
            (
                "callback",
                Amf3Value::map_from([(
                    "callbackName",
                    Amf3Value::String(self.callback_name.clone()),
                )]),
            ),
        ])
    }

    pub fn from_amf3(val: &Amf3Value) -> Result<Self, String> {
        let map = val
            .as_map()
            .ok_or_else(|| "request envelope must be an AMF3 Object map".to_owned())?;
        let method = map
            .get("method")
            .and_then(Amf3Value::as_str)
            .ok_or_else(|| "request missing string 'method'".to_owned())?
            .to_owned();
        let args = match map.get("args") {
            Some(Amf3Value::Array(items)) => items.clone(),
            Some(Amf3Value::Map(m)) if m.is_empty() => Vec::new(),
            _ => return Err("request 'args' must be a 1-based AMF3 Array".to_owned()),
        };
        let token = map
            .get("token")
            .and_then(Amf3Value::as_str)
            .unwrap_or("0")
            .to_owned();
        let callback_name = map
            .get("callback")
            .and_then(Amf3Value::as_map)
            .and_then(|cb| cb.get("callbackName"))
            .and_then(Amf3Value::as_str)
            .ok_or_else(|| "request missing 'callback.callbackName'".to_owned())?
            .to_owned();
        Ok(Self {
            method,
            args,
            token,
            callback_name,
        })
    }

    pub fn to_client_frame(&self) -> ClientFrame {
        ClientFrame::uncompressed(encode_amf3(&self.to_amf3()))
    }

    pub fn from_client_frame_bytes(stream: &[u8]) -> Result<(Self, usize), String> {
        let (frame, used) = ClientFrame::decode(stream).map_err(|e: PacketError| e.to_string())?;
        let amf = decode_amf3(&frame.body).map_err(|e: Amf3Error| e.to_string())?;
        let req = Self::from_amf3(&amf)?;
        Ok((req, used))
    }
}

/// Decoded Server -> Client RPC response / push envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponseEnvelope {
    /// Must equal `"ok"` for normal callback dispatch (`Network.lua` treats any other value as a protocol error).
    pub err: String,
    /// Echoes the client's `callback.callbackName` (`"<method>_<mId>"`), push method name, or `"dummy"`.
    pub callback_name: String,
    /// RPC-specific return payload passed as `ret` to the Lua callback / `BindableData:parseData(ret)`.
    pub ret: Amf3Value,
    /// Unix timestamp in seconds (`TimeManager.lua` computes server clock offset from `data.time`).
    pub time: i64,
    /// Session token persisted into `Network._token`.
    pub token: String,
}

impl RpcResponseEnvelope {
    pub fn ok(
        callback_name: impl Into<String>,
        ret: Amf3Value,
        time: i64,
        token: impl Into<String>,
    ) -> Self {
        Self {
            err: "ok".to_owned(),
            callback_name: callback_name.into(),
            ret,
            time,
            token: token.into(),
        }
    }

    /// Build the server `"dummy"` sync push observed in `live_capture_filtered.log` (`callbackName:dummy, err:ok`).
    pub fn dummy_push(time: i64, token: impl Into<String>) -> Self {
        Self::ok("dummy", Amf3Value::Null, time, token)
    }

    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([
            ("err", Amf3Value::String(self.err.clone())),
            (
                "callback",
                Amf3Value::map_from([(
                    "callbackName",
                    Amf3Value::String(self.callback_name.clone()),
                )]),
            ),
            ("ret", self.ret.clone()),
            ("time", Amf3Value::Int(self.time)),
            ("token", Amf3Value::String(self.token.clone())),
        ])
    }

    pub fn from_amf3(val: &Amf3Value) -> Result<Self, String> {
        let map = val
            .as_map()
            .ok_or_else(|| "response envelope must be an AMF3 Object map".to_owned())?;
        let err = map
            .get("err")
            .and_then(Amf3Value::as_str)
            .ok_or_else(|| "response missing string 'err'".to_owned())?
            .to_owned();
        let callback_name = map
            .get("callback")
            .and_then(Amf3Value::as_map)
            .and_then(|cb| cb.get("callbackName"))
            .and_then(Amf3Value::as_str)
            .ok_or_else(|| "response missing 'callback.callbackName'".to_owned())?
            .to_owned();
        let ret = map.get("ret").cloned().unwrap_or(Amf3Value::Null);
        let time = map
            .get("time")
            .and_then(Amf3Value::as_i64)
            .ok_or_else(|| "response missing integer/numeric 'time'".to_owned())?;
        let token = map
            .get("token")
            .and_then(Amf3Value::as_str)
            .ok_or_else(|| "response missing string 'token'".to_owned())?
            .to_owned();
        Ok(Self {
            err,
            callback_name,
            ret,
            time,
            token,
        })
    }

    pub fn to_server_frame(&self) -> ServerFrame {
        ServerFrame::single_uncompressed(encode_amf3(&self.to_amf3()))
    }

    pub fn from_server_frame_bytes(stream: &[u8]) -> Result<(Self, usize), String> {
        let (frame, used) = ServerFrame::decode(stream).map_err(|e: PacketError| e.to_string())?;
        let amf = decode_amf3(&frame.body).map_err(|e: Amf3Error| e.to_string())?;
        let resp = Self::from_amf3(&amf)?;
        Ok((resp, used))
    }
}
