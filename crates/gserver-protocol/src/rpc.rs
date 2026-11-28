//! High-level GServer (`vnsg2`) RPC Request, Response, Push Notification (`re.*`),
//! Heartbeat (`err = "ping"`), and Multi-Batch envelope codecs.

use crate::amf3::{self, Amf3Object, Value};
use crate::error::{EnvelopeError, ProtocolError};
use crate::frame::{
    encode_c2s_lcpacket, split_length_prefixed_subpackets, S2cFrame, ServerTxEncoder,
};

#[inline]
fn ensure_envelope_map(val: &Value) -> Result<(), EnvelopeError> {
    if matches!(val, Value::Object(_) | Value::Array { .. }) {
        Ok(())
    } else {
        Err(EnvelopeError::ExpectedMap)
    }
}

fn require_string_field(
    val: &Value,
    key: &str,
    field_label: &'static str,
) -> Result<String, EnvelopeError> {
    Ok(val
        .get_field(key)
        .ok_or(EnvelopeError::MissingField(field_label))?
        .as_str()
        .ok_or(EnvelopeError::InvalidFieldType {
            field: field_label,
            expected: "String",
        })?
        .to_owned())
}

fn opt_string_field(val: &Value, field: &'static str) -> Result<Option<String>, EnvelopeError> {
    match val.get_field(field) {
        None | Some(Value::Null | Value::Undefined) => Ok(None),
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .map(Some)
            .ok_or(EnvelopeError::InvalidFieldType {
                field,
                expected: "String",
            }),
    }
}

fn opt_i64_field(val: &Value, field: &'static str) -> Result<Option<i64>, EnvelopeError> {
    match val.get_field(field) {
        None | Some(Value::Null | Value::Undefined) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or(EnvelopeError::InvalidFieldType {
            field,
            expected: "Int or integral Double",
        }),
    }
}

fn opt_callback_name(val: &Value) -> Result<Option<String>, EnvelopeError> {
    match val.get_field("callback") {
        None | Some(Value::Null | Value::Undefined) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(cb_obj) => {
            require_string_field(cb_obj, "callbackName", "callback.callbackName").map(Some)
        }
    }
}

fn insert_opt_token_and_time(obj: &mut Amf3Object, token: Option<&str>, time: Option<i64>) {
    if let Some(tok) = token {
        obj.insert("token", Value::String(tok.to_owned()));
    }
    if let Some(ts) = time {
        obj.insert("time", Value::from(ts));
    }
}

/// Client-to-Server RPC call decoded from `Network:sendMsg` / `sendStatMsg` / `sendNewStatMsg`.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequest {
    pub method: String,
    pub args: Vec<Value>,
    pub token: String,
    pub callback_name: String,
}

impl RpcRequest {
    pub fn new(
        method: impl Into<String>,
        args: Vec<Value>,
        token: impl Into<String>,
        callback_name: impl Into<String>,
    ) -> Self {
        Self {
            method: method.into(),
            args,
            token: token.into(),
            callback_name: callback_name.into(),
        }
    }

    pub fn with_seq_id(
        method: impl Into<String>,
        seq_id: u64,
        args: Vec<Value>,
        token: impl Into<String>,
    ) -> Self {
        let method_str = method.into();
        let callback_name = format!("{method_str}_{seq_id}");
        Self::new(method_str, args, token, callback_name)
    }

    pub fn to_amf3_value(&self) -> Value {
        let callback_obj =
            Value::map([("callbackName", Value::String(self.callback_name.clone()))]);
        Value::map([
            ("method", Value::String(self.method.clone())),
            ("args", Value::dense_array(self.args.clone())),
            ("token", Value::String(self.token.clone())),
            ("callback", callback_obj),
        ])
    }

    pub fn to_amf3_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(amf3::encode(&self.to_amf3_value())?)
    }

    pub fn to_lcpacket_frame(&self, compress: bool) -> Result<Vec<u8>, ProtocolError> {
        encode_c2s_lcpacket(&self.to_amf3_bytes()?, compress)
    }

    pub fn from_amf3_value(val: &Value) -> Result<Self, ProtocolError> {
        ensure_envelope_map(val)?;
        let method = require_string_field(val, "method", "method")?;
        let args = match val.get_field("args") {
            None | Some(Value::Null | Value::Undefined) => Vec::new(),
            Some(Value::Array { dense, .. }) => dense.clone(),
            Some(_) => {
                return Err(EnvelopeError::InvalidFieldType {
                    field: "args",
                    expected: "Array",
                }
                .into())
            }
        };
        let token = opt_string_field(val, "token")?.unwrap_or_else(|| String::from("0"));
        let callback_name =
            opt_callback_name(val)?.ok_or(EnvelopeError::MissingField("callback"))?;

        Ok(Self {
            method,
            args,
            token,
            callback_name,
        })
    }

    pub fn from_amf3_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (val, _) = amf3::decode(bytes)?;
        Self::from_amf3_value(&val)
    }
}

/// How individual sub-envelopes are represented inside `data.ret[1..N]` when `multi == 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MultiItemEncoding {
    #[default]
    NestedAmf3ByteString,
    NestedAmf3ByteArray,
    InlineObject,
}

/// Server-to-Client message envelope (`Network:onBodyData`).
#[derive(Debug, Clone, PartialEq)]
pub struct S2cEnvelope {
    pub err: String,
    pub callback_name: Option<String>,
    pub ret: Value,
    pub token: Option<String>,
    pub time: Option<i64>,
}

impl S2cEnvelope {
    fn new_base(err: impl Into<String>, callback_name: Option<String>, ret: Value) -> Self {
        Self {
            err: err.into(),
            callback_name,
            ret,
            token: None,
            time: None,
        }
    }

    pub fn ok_response(callback_name: impl Into<String>, ret: Value) -> Self {
        Self::new_base("ok", Some(callback_name.into()), ret)
    }

    pub fn error_response(callback_name: impl Into<String>, err: impl Into<String>) -> Self {
        Self::new_base(err, Some(callback_name.into()), Value::Null)
    }

    pub fn push(push_method: impl Into<String>, payload: Value) -> Self {
        Self::new_base("ok", Some(push_method.into()), payload)
    }

    pub fn ping() -> Self {
        Self::new_base("ping", None, Value::Null)
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn with_time(mut self, unix_seconds: i64) -> Self {
        self.time = Some(unix_seconds);
        self
    }

    pub fn is_ping(&self) -> bool {
        self.err == "ping"
    }

    pub fn is_push_topic(&self) -> bool {
        matches!(
            self.callback_name.as_deref(),
            Some(name) if name.starts_with("re.") || name == "user.updateUser"
        )
    }

    pub fn to_amf3_value(&self) -> Value {
        let mut obj = Amf3Object::anonymous_dynamic();
        obj.insert("err", Value::String(self.err.clone()));
        if let Some(cb) = &self.callback_name {
            obj.insert(
                "callback",
                Value::map([("callbackName", Value::String(cb.clone()))]),
            );
        }
        if !matches!(self.ret, Value::Undefined)
            && !(self.is_ping() && matches!(self.ret, Value::Null))
        {
            obj.insert("ret", self.ret.clone());
        }
        insert_opt_token_and_time(&mut obj, self.token.as_deref(), self.time);
        Value::Object(obj)
    }

    pub fn to_amf3_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(amf3::encode(&self.to_amf3_value())?)
    }

    pub fn from_amf3_value(val: &Value) -> Result<Self, ProtocolError> {
        ensure_envelope_map(val)?;
        let err = require_string_field(val, "err", "err")?;
        let callback_name = match opt_callback_name(val)? {
            Some(cb) => Some(cb),
            None if err == "ping" => None,
            None => return Err(EnvelopeError::MissingField("callback").into()),
        };

        Ok(Self {
            err,
            callback_name,
            ret: val.get_field("ret").cloned().unwrap_or(Value::Null),
            token: opt_string_field(val, "token")?,
            time: opt_i64_field(val, "time")?,
        })
    }

    pub fn from_amf3_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (val, _) = amf3::decode(bytes)?;
        Self::from_amf3_value(&val)
    }
}

/// Decoded S2C delivery matching `Network:onBodyData` (supporting both `multi == 0` and `multi != 0`).
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedS2cBatch {
    pub token: Option<String>,
    pub time: Option<i64>,
    pub items: Vec<S2cEnvelope>,
}

/// Encodes a batch of [`S2cEnvelope`] items into the exact `multi = 1` outer AMF3 payload.
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
        let encoded = match encoding {
            MultiItemEncoding::NestedAmf3ByteString => Value::ByteString(item.to_amf3_bytes()?),
            MultiItemEncoding::NestedAmf3ByteArray => Value::ByteArray(item.to_amf3_bytes()?),
            MultiItemEncoding::InlineObject => item.to_amf3_value(),
        };
        dense.push(encoded);
    }

    let mut outer = Amf3Object::anonymous_dynamic();
    outer.insert("err", Value::String(String::from("ok")));
    outer.insert("ret", Value::dense_array(dense));
    insert_opt_token_and_time(&mut outer, token, time);

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
    tx.encode_single(&envelope.to_amf3_bytes()?, zipped)
}

/// Unpacks a decoded [`S2cFrame`] into a [`DecodedS2cBatch`] following `Network:onBodyData`.
pub fn decode_s2c_frame_batch(frame: &S2cFrame) -> Result<DecodedS2cBatch, ProtocolError> {
    if !frame.flags.is_multi() {
        let env = S2cEnvelope::from_amf3_bytes(&frame.payload)?;
        return Ok(DecodedS2cBatch {
            token: env.token.clone(),
            time: env.time,
            items: vec![env],
        });
    }

    let first_byte = frame.payload.first().copied().unwrap_or(0);
    if first_byte == amf3::MARKER_OBJECT || first_byte == amf3::MARKER_ARRAY {
        let (outer, _) = amf3::decode(&frame.payload)?;
        let token = opt_string_field(&outer, "token")?;
        let time = opt_i64_field(&outer, "time")?;
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
            let env = match elem {
                Value::Object(_) | Value::Array { .. } => S2cEnvelope::from_amf3_value(elem)?,
                other => {
                    let raw = other.as_bytes().ok_or(EnvelopeError::InvalidFieldType {
                        field: "ret[i]",
                        expected: "table or AMF3 byte string",
                    })?;
                    S2cEnvelope::from_amf3_bytes(raw)?
                }
            };
            items.push(env);
        }

        Ok(DecodedS2cBatch { token, time, items })
    } else {
        let slices = split_length_prefixed_subpackets(&frame.payload)?;
        if slices.is_empty() {
            return Err(EnvelopeError::EmptyMultiBatch.into());
        }
        let items = slices
            .into_iter()
            .map(S2cEnvelope::from_amf3_bytes)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DecodedS2cBatch {
            token: items.last().and_then(|e| e.token.clone()),
            time: items.last().and_then(|e| e.time),
            items,
        })
    }
}
