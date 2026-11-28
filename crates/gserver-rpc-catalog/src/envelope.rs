//! `Network.lua` RPC request and response envelope adapters backed by
//! [`gserver_protocol::rpc::{RpcRequest, S2cEnvelope}`] and [`gserver_protocol::zlib`].

use gserver_protocol::rpc::{RpcRequest, S2cEnvelope};
use gserver_protocol::zlib::{zlib_compress, zlib_decompress};
use gserver_protocol::Value;

use crate::amf3::{decode_amf3, encode_amf3, Amf3Error, Amf3Value};
use crate::packet::{ClientFrame, PacketError, ServerFrame};

/// Decoded Client -> Server RPC request envelope (`Network.lua:sendMsg`).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequestEnvelope {
    pub method: String,
    pub args: Vec<Amf3Value>,
    pub token: String,
    pub callback_name: String,
}

impl RpcRequestEnvelope {
    /// Construct a sequenced RPC request matching `Network.lua` (`<method>_<msg_id>`).
    pub fn new_sequenced(
        method: impl Into<String>,
        args: Vec<Amf3Value>,
        token: impl Into<String>,
        msg_id: u64,
    ) -> Self {
        let proto = RpcRequest::with_seq_id(
            method,
            msg_id,
            args.iter().map(Amf3Value::to_protocol_value).collect(),
            token,
        );
        Self::from_protocol_request(proto)
    }

    /// Construct a fire-and-forget telemetry RPC (`user.stat` or `user.guideEvent`).
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

    /// Convert to [`gserver_protocol::rpc::RpcRequest`].
    pub fn to_protocol_request(&self) -> RpcRequest {
        RpcRequest::new(
            self.method.clone(),
            self.args.iter().map(Amf3Value::to_protocol_value).collect(),
            self.token.clone(),
            self.callback_name.clone(),
        )
    }

    /// Convert from [`gserver_protocol::rpc::RpcRequest`].
    pub fn from_protocol_request(req: RpcRequest) -> Self {
        Self {
            method: req.method,
            args: req.args.iter().map(Amf3Value::from_protocol_value).collect(),
            token: req.token,
            callback_name: req.callback_name,
        }
    }

    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::from_protocol_value(&self.to_protocol_request().to_amf3_value())
    }

    pub fn from_amf3(val: &Amf3Value) -> Result<Self, String> {
        let map = val
            .as_map()
            .ok_or_else(|| "request envelope must be an AMF3 Object map".to_owned())?;
        if !map.contains_key("args") {
            return Err("request 'args' must be a 1-based AMF3 Array".to_owned());
        }
        let mut proto_val = val.to_protocol_value();
        // Accept empty Object `{}` for `args` as an empty Lua sequence before delegating to RpcRequest.
        if let Value::Object(ref mut obj) = proto_val {
            if let Some((_, args_slot)) = obj.dynamic_members.iter_mut().find(|(k, _)| k == "args")
            {
                if matches!(args_slot, Value::Object(o) if o.sealed_members.is_empty() && o.dynamic_members.is_empty())
                {
                    *args_slot = Value::empty_array();
                }
            }
        }
        RpcRequest::from_amf3_value(&proto_val)
            .map(Self::from_protocol_request)
            .map_err(|e| e.to_string())
    }

    pub fn to_client_frame(&self) -> ClientFrame {
        ClientFrame::uncompressed(encode_amf3(&self.to_amf3()))
    }

    pub fn from_client_frame_bytes(stream: &[u8]) -> Result<(Self, usize), String> {
        let (frame, used) = ClientFrame::decode(stream).map_err(|e: PacketError| e.to_string())?;
        let amf = decode_amf3(&frame.body).map_err(|e: Amf3Error| e.to_string())?;
        Ok((Self::from_amf3(&amf)?, used))
    }
}

/// Decoded Server -> Client RPC response / push envelope (`Network.lua:onBodyData`).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponseEnvelope {
    pub err: String,
    pub callback_name: String,
    pub ret: Amf3Value,
    pub time: i64,
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

    /// Build the server `"dummy"` sync push observed in `live_capture_filtered.log`.
    pub fn dummy_push(time: i64, token: impl Into<String>) -> Self {
        Self::ok("dummy", Amf3Value::Null, time, token)
    }

    /// Convert to [`gserver_protocol::rpc::S2cEnvelope`].
    pub fn to_protocol_envelope(&self) -> S2cEnvelope {
        S2cEnvelope {
            err: self.err.clone(),
            callback_name: Some(self.callback_name.clone()),
            ret: self.ret.to_protocol_value(),
            token: Some(self.token.clone()),
            time: Some(self.time),
        }
    }

    /// Convert from [`gserver_protocol::rpc::S2cEnvelope`].
    pub fn from_protocol_envelope(env: S2cEnvelope) -> Result<Self, String> {
        Ok(Self {
            err: env.err,
            callback_name: env
                .callback_name
                .ok_or_else(|| "response missing 'callback.callbackName'".to_owned())?,
            ret: Amf3Value::from_protocol_value(&env.ret),
            time: env
                .time
                .ok_or_else(|| "response missing integer/numeric 'time'".to_owned())?,
            token: env
                .token
                .ok_or_else(|| "response missing string 'token'".to_owned())?,
        })
    }

    pub fn to_amf3(&self) -> Amf3Value {
        let mut amf = Amf3Value::from_protocol_value(&self.to_protocol_envelope().to_amf3_value());
        if let Amf3Value::Map(ref mut m) = amf {
            m.insert("ret".to_owned(), self.ret.clone());
            m.insert("time".to_owned(), Amf3Value::Int(self.time));
        }
        amf
    }

    pub fn from_amf3(val: &Amf3Value) -> Result<Self, String> {
        if val.as_map().is_none() {
            return Err("response envelope must be an AMF3 Object map".to_owned());
        }
        S2cEnvelope::from_amf3_value(&val.to_protocol_value())
            .map_err(|e| e.to_string())
            .and_then(Self::from_protocol_envelope)
    }

    pub fn to_server_frame(&self) -> ServerFrame {
        ServerFrame::single_uncompressed(encode_amf3(&self.to_amf3()))
    }

    /// Compress the AMF3 body directly with [`gserver_protocol::zlib::zlib_compress`].
    pub fn to_zlib_amf3_bytes(&self) -> Vec<u8> {
        zlib_compress(&encode_amf3(&self.to_amf3()))
    }

    /// Decompress a Zlib-wrapped AMF3 response body via [`gserver_protocol::zlib::zlib_decompress`].
    pub fn from_zlib_amf3_bytes(compressed: &[u8]) -> Result<Self, String> {
        let raw = zlib_decompress(compressed).map_err(|e| e.to_string())?;
        let amf = decode_amf3(&raw).map_err(|e| e.to_string())?;
        Self::from_amf3(&amf)
    }

    pub fn from_server_frame_bytes(stream: &[u8]) -> Result<(Self, usize), String> {
        let (frame, used) = ServerFrame::decode(stream).map_err(|e: PacketError| e.to_string())?;
        let amf = decode_amf3(&frame.body).map_err(|e: Amf3Error| e.to_string())?;
        Ok((Self::from_amf3(&amf)?, used))
    }
}
