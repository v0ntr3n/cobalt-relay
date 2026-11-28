#![forbid(unsafe_code)]

//! Asymmetric wire framing adapters backed by [`gserver_protocol`] (`ClientRxDecoder`,
//! `ServerTxEncoder`, `ServerRxDecoder`, `S2cEnvelope`) plus the stateful `GCLIENT.exe`
//! (`CConnection.cpp` + `Network.lua`) behavioral simulator (`Win32ClientSimulator`).

use std::collections::HashMap;
use std::fmt;

use gserver_protocol::error::FrameError as ProtoFrameError;
use gserver_protocol::frame::{
    encode_c2s_lcpacket, C2sFrameMode, C2sWireKind, ClientRxDecoder, S2cFlags, S2cFrame,
    ServerRxDecoder, ServerTxEncoder, C2S_LCPACKET_HEADER_SIZE, DEFAULT_MAX_FRAME_BODY_LEN,
    S2C_HEADER_SIZE,
};
use gserver_protocol::rpc::{encode_s2c_multi_frame, MultiItemEncoding, S2cEnvelope};
use gserver_protocol::zlib::DEFAULT_MAX_DECOMPRESSED_SIZE;
use gserver_protocol::ProtocolError;

use crate::amf3::{Amf3Decoder, Amf3Encoder, Amf3Error, Amf3Value};
use crate::crypto::compute_babeltime_md5;
use crate::zlib::{map_zlib_error, ZlibError};

pub const CLIENT_LC_HEADER_LEN: usize = C2S_LCPACKET_HEADER_SIZE;
pub const SERVER_RX_HEADER_LEN: usize = S2C_HEADER_SIZE;

/// Strict private-server DoS ceiling (`16 MiB`) for incoming frame bodies and decompressed streams.
pub const MAX_SERVER_FRAME_BODY_LEN: usize = DEFAULT_MAX_FRAME_BODY_LEN as usize;

/// Client-side `CConnection.cpp:30` `kMaxFrameBodyLength` (`64 MiB`).
pub const CLIENT_MAX_FRAME_BODY_LEN: usize = DEFAULT_MAX_DECOMPRESSED_SIZE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    ZeroLengthBody,
    OversizedBody { declared: u32, max: usize },
    InvalidClientFlags([u8; 4]),
    Md5Mismatch { expected: [u8; 16], actual: [u8; 16] },
    Zlib(ZlibError),
    Amf3(Amf3Error),
    ClientSocketClosed(&'static str),
    LuaRuntimeCrash(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLengthBody => write!(f, "zero-length frame body (body_len == 0)"),
            Self::OversizedBody { declared, max } => {
                write!(f, "oversized frame body {declared} bytes exceeds max {max} bytes")
            }
            Self::InvalidClientFlags(fl) => write!(f, "invalid LCPacket flags {fl:?}"),
            Self::Md5Mismatch { .. } => write!(f, "BabelTime 16-byte MD5 integrity check failed"),
            Self::Zlib(e) => write!(f, "zlib decompression error: {e}"),
            Self::Amf3(e) => write!(f, "AMF3 codec error: {e}"),
            Self::ClientSocketClosed(reason) => write!(f, "Win32 CConnection closed: {reason}"),
            Self::LuaRuntimeCrash(msg) => {
                write!(f, "Win32 Lua runtime panic in Network.lua: {msg}")
            }
        }
    }
}

impl From<ZlibError> for FrameError {
    fn from(e: ZlibError) -> Self {
        Self::Zlib(e)
    }
}

impl From<Amf3Error> for FrameError {
    fn from(e: Amf3Error) -> Self {
        Self::Amf3(e)
    }
}

fn map_proto_error(err: ProtocolError, max_body_len: usize, wire_body_len: usize) -> FrameError {
    match err {
        ProtocolError::Frame(ProtoFrameError::ZeroBodyLength) => FrameError::ZeroLengthBody,
        ProtocolError::Frame(ProtoFrameError::BodyTooLarge {
            body_len,
            max_body_len,
        }) => FrameError::OversizedBody {
            declared: body_len,
            max: max_body_len as usize,
        },
        ProtocolError::Frame(ProtoFrameError::PayloadOverflow { len }) => {
            FrameError::OversizedBody {
                declared: u32::try_from(len).unwrap_or(u32::MAX),
                max: max_body_len,
            }
        }
        ProtocolError::Frame(ProtoFrameError::InvalidLcFlags(fl)) => {
            FrameError::InvalidClientFlags(fl)
        }
        ProtocolError::Frame(ProtoFrameError::Md5Mismatch { expected, actual }) => {
            FrameError::Md5Mismatch { expected, actual }
        }
        ProtocolError::Frame(ProtoFrameError::UnrecognizedC2sHeader(hdr)) => {
            FrameError::InvalidClientFlags([hdr[4], hdr[5], hdr[6], hdr[7]])
        }
        ProtocolError::Frame(ProtoFrameError::InvalidMultiSubPacket { .. }) => {
            FrameError::LuaRuntimeCrash("invalid multi sub-packet".to_string())
        }
        ProtocolError::Zlib(ze) => FrameError::Zlib(map_zlib_error(ze, wire_body_len)),
        ProtocolError::Amf3(ae) => FrameError::Amf3(Amf3Error::from(ae)),
        ProtocolError::Envelope(ee) => FrameError::LuaRuntimeCrash(ee.to_string()),
        ProtocolError::BtDecrypt(be) => FrameError::LuaRuntimeCrash(be.to_string()),
    }
}

// ============================================================================
// Client -> Server 24-Byte `LCPacket` Encoder & Streaming Server Decoder
// ============================================================================

/// Encodes a client->server 24-byte `LCPacket` via [`gserver_protocol::frame::encode_c2s_lcpacket`].
pub fn encode_client_lc_packet(raw_payload: &[u8], compress: bool) -> Vec<u8> {
    if let Ok(pkt) = encode_c2s_lcpacket(raw_payload, compress) {
        return pkt;
    }
    let md5 = compute_babeltime_md5(&[]);
    let mut pkt = Vec::with_capacity(CLIENT_LC_HEADER_LEN);
    pkt.extend_from_slice(&0u32.to_be_bytes());
    pkt.extend_from_slice(&[u8::from(compress), 1, 1, 1]);
    pkt.extend_from_slice(&md5);
    pkt
}

/// Checks if `data` is already a valid framed 24-byte `LCPacket` (`CConnection.cpp:347`).
pub fn is_framed_lc_packet(data: &[u8]) -> bool {
    if data.len() < CLIENT_LC_HEADER_LEN {
        return false;
    }
    let body_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if body_len != data.len() - CLIENT_LC_HEADER_LEN {
        return false;
    }
    if (data[4] != 0 && data[4] != 1) || data[5] != 1 || data[6] != 1 || data[7] != 1 {
        return false;
    }
    data[8..24] == compute_babeltime_md5(&data[CLIENT_LC_HEADER_LEN..])
}

#[derive(Debug, Clone)]
pub struct DecodedClientPacket {
    pub body_len: u32,
    pub flags: [u8; 4],
    pub md5: [u8; 16],
    pub raw_amf3: Vec<u8>,
    pub value: Amf3Value,
}

/// Observable parser stage of [`ServerRxStreamDecoder`] for deterministic connection lifecycle diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderParserState {
    Idle,
    AwaitingLengthPrefix { buffered_bytes: usize },
    AwaitingLcHeader { declared_body_len: u32, buffered_bytes: usize },
    AwaitingFrameBody { declared_body_len: u32, buffered_body_bytes: usize },
}

impl fmt::Display for DecoderParserState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Idle => write!(f, "Idle"),
            Self::AwaitingLengthPrefix { buffered_bytes } => {
                write!(f, "AwaitingLengthPrefix({buffered_bytes}/4B)")
            }
            Self::AwaitingLcHeader {
                declared_body_len,
                buffered_bytes,
            } => write!(
                f,
                "AwaitingLcHeader(declared={declared_body_len}, {buffered_bytes}/24B)"
            ),
            Self::AwaitingFrameBody {
                declared_body_len,
                buffered_body_bytes,
            } => write!(
                f,
                "AwaitingFrameBody({buffered_body_bytes}/{declared_body_len}B)"
            ),
        }
    }
}

/// Stateful, DoS-hardened streaming decoder backed by [`gserver_protocol::ClientRxDecoder`].
pub struct ServerRxStreamDecoder {
    inner: ClientRxDecoder,
    max_body_len: usize,
    max_decompressed_len: usize,
}

impl Default for ServerRxStreamDecoder {
    fn default() -> Self {
        Self::new(MAX_SERVER_FRAME_BODY_LEN, MAX_SERVER_FRAME_BODY_LEN)
    }
}

impl ServerRxStreamDecoder {
    pub fn new(max_body_len: usize, max_decompressed_len: usize) -> Self {
        let max_u32 = u32::try_from(max_body_len).unwrap_or(u32::MAX);
        Self {
            inner: ClientRxDecoder::new(C2sFrameMode::LcPacket24)
                .with_limits(max_u32, max_decompressed_len),
            max_body_len,
            max_decompressed_len,
        }
    }

    pub fn buffered_len(&self) -> usize {
        self.inner.buffered_len()
    }

    pub fn parser_state(&self) -> DecoderParserState {
        match self.inner.parser_state() {
            gserver_protocol::C2sParserState::Idle => DecoderParserState::Idle,
            gserver_protocol::C2sParserState::AwaitingLengthPrefix { buffered_bytes } => {
                DecoderParserState::AwaitingLengthPrefix { buffered_bytes }
            }
            gserver_protocol::C2sParserState::AwaitingLcHeader {
                declared_body_len,
                buffered_bytes,
            } => DecoderParserState::AwaitingLcHeader {
                declared_body_len,
                buffered_bytes,
            },
            gserver_protocol::C2sParserState::AwaitingFrameBody {
                declared_body_len,
                buffered_body_bytes,
            } => DecoderParserState::AwaitingFrameBody {
                declared_body_len,
                buffered_body_bytes,
            },
        }
    }

    pub fn push_bytes(&mut self, chunk: &[u8]) -> Result<Vec<DecodedClientPacket>, FrameError> {
        self.inner.feed(chunk);
        let mut packets = Vec::new();
        loop {
            match self.inner.next_frame() {
                Ok(None) => break,
                Ok(Some(frame)) => {
                    let (compressed, md5) = match frame.kind {
                        C2sWireKind::LcPacket24 { compressed, md5 } => (compressed, md5),
                        C2sWireKind::Raw4Byte => (false, [0u8; 16]),
                    };
                    let value = match Amf3Decoder::decode(&frame.amf3_payload) {
                        Ok(v) => v,
                        Err(e) => {
                            self.clear();
                            return Err(FrameError::Amf3(e));
                        }
                    };
                    packets.push(DecodedClientPacket {
                        body_len: frame.wire_body_len,
                        flags: [u8::from(compressed), 1, 1, 1],
                        md5,
                        raw_amf3: frame.amf3_payload,
                        value,
                    });
                }
                Err(err) => {
                    self.clear();
                    return Err(map_proto_error(err, self.max_body_len, 6));
                }
            }
        }
        Ok(packets)
    }

    pub fn clear(&mut self) {
        let max_u32 = u32::try_from(self.max_body_len).unwrap_or(u32::MAX);
        self.inner = ClientRxDecoder::new(C2sFrameMode::LcPacket24)
            .with_limits(max_u32, self.max_decompressed_len);
    }
}

// ============================================================================
// Server -> Client 8-Byte Frame & Envelope Builders (`ServerTxEncoder` + `S2cEnvelope`)
// ============================================================================

fn server_tx_encoder() -> ServerTxEncoder {
    ServerTxEncoder::new().with_max_body_len(CLIENT_MAX_FRAME_BODY_LEN as u32)
}

/// Encodes a single server->client 8-byte framed packet (`multi = 0`) via [`ServerTxEncoder`].
pub fn encode_server_single_frame(
    payload: &Amf3Value,
    compress: bool,
) -> Result<Vec<u8>, FrameError> {
    let raw_amf3 = Amf3Encoder::encode(payload)?;
    if raw_amf3.is_empty() {
        return Err(FrameError::ZeroLengthBody);
    }
    server_tx_encoder()
        .encode_single(&raw_amf3, compress)
        .map_err(|e| map_proto_error(e, CLIENT_MAX_FRAME_BODY_LEN, raw_amf3.len()))
}

/// Encodes a multi-RPC batch server->client 8-byte framed packet (`multi = 1` at `head[6]`)
/// backed by [`S2cEnvelope`], [`encode_s2c_multi_frame`], and [`ServerTxEncoder`].
pub fn encode_server_multi_frame(
    rets: &[Amf3Value],
    token: Option<&str>,
    server_time_sec: Option<i64>,
    compress: bool,
    encode_inner_as_bytes: bool,
) -> Result<Vec<u8>, FrameError> {
    let tx = server_tx_encoder();
    if !rets.is_empty() {
        let maybe_envelopes: Result<Vec<S2cEnvelope>, _> = rets
            .iter()
            .map(|v| {
                v.to_protocol_value()
                    .map_err(ProtocolError::from_qa_amf3)
                    .and_then(|pv| S2cEnvelope::from_amf3_value(&pv))
            })
            .collect();
        if let Ok(envelopes) = maybe_envelopes {
            let encoding = if encode_inner_as_bytes {
                MultiItemEncoding::NestedAmf3ByteString
            } else {
                MultiItemEncoding::InlineObject
            };
            return encode_s2c_multi_frame(
                &tx,
                &envelopes,
                token,
                server_time_sec,
                compress,
                encoding,
            )
            .map_err(|e| map_proto_error(e, CLIENT_MAX_FRAME_BODY_LEN, 6));
        }
    }

    let mut dense_items = Vec::with_capacity(rets.len());
    for item in rets {
        if encode_inner_as_bytes {
            dense_items.push(Amf3Value::String(Amf3Encoder::encode(item)?));
        } else {
            dense_items.push(item.clone());
        }
    }
    let mut fields = vec![("ret", Amf3Value::dense_array(dense_items))];
    if let Some(tok) = token {
        fields.push(("token", Amf3Value::from_str(tok)));
    }
    if let Some(ts) = server_time_sec {
        fields.push(("time", Amf3Value::from_i64_auto(ts)));
    }
    let raw_amf3 = Amf3Encoder::encode(&Amf3Value::anonymous_object(fields))?;
    tx.encode_multi_amf3_payload(&raw_amf3, compress)
        .map_err(|e| map_proto_error(e, CLIENT_MAX_FRAME_BODY_LEN, raw_amf3.len()))
}

trait FromQaAmf3 {
    fn from_qa_amf3(err: Amf3Error) -> Self;
}

impl FromQaAmf3 for ProtocolError {
    fn from_qa_amf3(_: Amf3Error) -> Self {
        ProtocolError::Amf3(gserver_protocol::Amf3Error::LengthOverflow {
            kind: "QA value",
            len: 0,
        })
    }
}

/// Low-level 8-byte server->client header builder backed by [`ServerTxEncoder`].
pub fn encode_server_raw_frame(
    raw_amf3: &[u8],
    compress: bool,
    multi: bool,
) -> Result<Vec<u8>, FrameError> {
    if raw_amf3.is_empty() {
        return Err(FrameError::ZeroLengthBody);
    }
    server_tx_encoder()
        .encode_frame(raw_amf3, S2cFlags::new(compress, multi))
        .map_err(|e| map_proto_error(e, CLIENT_MAX_FRAME_BODY_LEN, raw_amf3.len()))
}

/// Builds a standard GServer RPC response envelope via [`S2cEnvelope`].
pub fn build_rpc_response(
    err: &str,
    callback_name: &str,
    ret: Amf3Value,
    token: Option<&str>,
    server_time_sec: Option<i64>,
) -> Amf3Value {
    if !matches!(ret, Amf3Value::Undefined) && err != "ping" {
        if let Ok(proto_ret) = ret.to_protocol_value() {
            let env = S2cEnvelope {
                err: err.to_string(),
                callback_name: Some(callback_name.to_string()),
                ret: proto_ret,
                token: token.map(str::to_owned),
                time: server_time_sec,
            };
            return Amf3Value::from_protocol_value(env.to_amf3_value());
        }
    }
    let mut entries = vec![
        ("err", Amf3Value::from_str(err)),
        (
            "callback",
            Amf3Value::anonymous_object(vec![("callbackName", Amf3Value::from_str(callback_name))]),
        ),
        ("ret", ret),
    ];
    if let Some(t) = token {
        entries.push(("token", Amf3Value::from_str(t)));
    }
    if let Some(ts) = server_time_sec {
        entries.push(("time", Amf3Value::from_i64_auto(ts)));
    }
    Amf3Value::anonymous_object(entries)
}

/// Builds a server heartbeat `{ err = "ping", time = ... }` via [`S2cEnvelope::ping`].
pub fn build_ping_response(server_time_sec: i64) -> Amf3Value {
    Amf3Value::from_protocol_value(S2cEnvelope::ping().with_time(server_time_sec).to_amf3_value())
}

// ============================================================================
// Exact `GCLIENT.exe` (`CConnection.cpp` + `Network.lua`) Behavioral Simulator
// ============================================================================

#[derive(Debug, Clone)]
pub struct DispatchedRpcEvent {
    pub callback_name: String,
    pub err: String,
    pub ret: Amf3Value,
    pub was_push: bool,
}

#[derive(Debug, Clone)]
struct PendingRpcContext {
    is_push: bool,
    request_obj: Amf3Value,
}

/// Cycle-accurate simulation of `CConnection.cpp` (`headerSize_ = 8`, `doRead`) coupled to
/// `Network.lua` (`ctor`, `sendMsg`, `sendStatMsg`, `sendUnusedMsg`, `onBodyData`),
/// using [`ServerRxDecoder`] for streaming 8-byte S2C frame reassembly and bounded Zlib inflation.
pub struct Win32ClientSimulator {
    pub token: String,
    pub time_offset_sec: i64,
    pub m_id: u64,
    pub socket_closed: bool,
    pub close_reason: Option<&'static str>,
    pub message_boxes: Vec<String>,

    rx_decoder: ServerRxDecoder,
    contexts: HashMap<String, PendingRpcContext>,
}

impl Default for Win32ClientSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Win32ClientSimulator {
    pub fn new() -> Self {
        Self {
            token: "0".to_string(),
            time_offset_sec: 0,
            m_id: 1,
            socket_closed: false,
            close_reason: None,
            message_boxes: Vec::new(),
            rx_decoder: ServerRxDecoder::new()
                .with_limits(CLIENT_MAX_FRAME_BODY_LEN as u32, CLIENT_MAX_FRAME_BODY_LEN),
            contexts: HashMap::new(),
        }
    }

    pub fn reg_push_callback(&mut self, method: &str) {
        self.contexts.insert(
            method.to_string(),
            PendingRpcContext {
                is_push: true,
                request_obj: Amf3Value::Null,
            },
        );
    }

    pub fn build_send_msg(
        &mut self,
        method: &str,
        args: Amf3Value,
        compress: bool,
    ) -> (String, Vec<u8>) {
        self.m_id += 1;
        let callback_name = format!("{method}_{}", self.m_id);
        let request = Amf3Value::anonymous_object(vec![
            ("method", Amf3Value::from_str(method)),
            ("args", args),
            ("token", Amf3Value::from_str(&self.token)),
            (
                "callback",
                Amf3Value::anonymous_object(vec![(
                    "callbackName",
                    Amf3Value::from_str(&callback_name),
                )]),
            ),
        ]);
        self.contexts.insert(
            callback_name.clone(),
            PendingRpcContext {
                is_push: false,
                request_obj: request.clone(),
            },
        );
        let raw_amf3 = Amf3Encoder::encode(&request).expect("valid client request AMF3");
        (callback_name, encode_client_lc_packet(&raw_amf3, compress))
    }

    pub fn build_stat_msg(&self, args: Amf3Value) -> Vec<u8> {
        let request = Amf3Value::anonymous_object(vec![
            ("method", Amf3Value::from_str("user.stat")),
            ("args", args),
            ("token", Amf3Value::from_str(&self.token)),
            (
                "callback",
                Amf3Value::anonymous_object(vec![(
                    "callbackName",
                    Amf3Value::from_str("user.stat"),
                )]),
            ),
        ]);
        let raw_amf3 = Amf3Encoder::encode(&request).expect("valid stat AMF3");
        encode_client_lc_packet(&raw_amf3, true)
    }

    pub fn build_reconnect_replay_packets(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for ctx in self.contexts.values() {
            if !ctx.is_push {
                if let Amf3Value::Object {
                    class_name,
                    dynamic,
                    sealed,
                    mut dynamic_members,
                    externalizable,
                } = ctx.request_obj.clone()
                {
                    for (k, v) in &mut dynamic_members {
                        if k.as_slice() == b"token" {
                            *v = Amf3Value::from_str(&self.token);
                        }
                    }
                    let updated = Amf3Value::Object {
                        class_name,
                        dynamic,
                        sealed,
                        dynamic_members,
                        externalizable,
                    };
                    let raw = Amf3Encoder::encode(&updated).expect("valid replay AMF3");
                    out.push(encode_client_lc_packet(&raw, true));
                }
            }
        }
        out
    }

    pub fn pending_one_shot_count(&self) -> usize {
        self.contexts.values().filter(|c| !c.is_push).count()
    }

    pub fn receive_server_bytes(
        &mut self,
        chunk: &[u8],
        client_now_sec: i64,
    ) -> Result<Vec<DispatchedRpcEvent>, FrameError> {
        if self.socket_closed {
            return Err(FrameError::ClientSocketClosed(
                self.close_reason.unwrap_or("ALREADY_CLOSED"),
            ));
        }
        self.rx_decoder.feed(chunk);

        let mut events = Vec::new();
        loop {
            match self.rx_decoder.next_frame() {
                Ok(None) => break,
                Ok(Some(frame)) => {
                    self.on_s2c_frame(&frame, client_now_sec, &mut events)?;
                }
                Err(ProtocolError::Frame(
                    ProtoFrameError::ZeroBodyLength | ProtoFrameError::BodyTooLarge { .. },
                )) => {
                    self.socket_closed = true;
                    self.close_reason = Some("HEAD_SCRIPT_ERROR");
                    self.rx_decoder = ServerRxDecoder::new()
                        .with_limits(CLIENT_MAX_FRAME_BODY_LEN as u32, CLIENT_MAX_FRAME_BODY_LEN);
                    return Err(FrameError::ClientSocketClosed("HEAD_SCRIPT_ERROR"));
                }
                Err(other) => {
                    self.rx_decoder = ServerRxDecoder::new()
                        .with_limits(CLIENT_MAX_FRAME_BODY_LEN as u32, CLIENT_MAX_FRAME_BODY_LEN);
                    return Err(map_proto_error(other, CLIENT_MAX_FRAME_BODY_LEN, 6));
                }
            }
        }
        Ok(events)
    }

    fn on_s2c_frame(
        &mut self,
        frame: &S2cFrame,
        client_now_sec: i64,
        events: &mut Vec<DispatchedRpcEvent>,
    ) -> Result<(), FrameError> {
        let data = Amf3Decoder::decode(&frame.payload)?;
        let mut rets = Vec::new();

        if frame.flags.is_multi() {
            let ret_field = data.get_field("ret").ok_or_else(|| {
                FrameError::LuaRuntimeCrash(
                    "Network.lua:112: attempt to get length of field 'ret' (a nil value)"
                        .to_string(),
                )
            })?;
            match ret_field {
                Amf3Value::Array { dense, .. } => {
                    let lua_len = lua51_seq_len(dense);
                    for item in dense.iter().take(lua_len) {
                        match item {
                            Amf3Value::Null | Amf3Value::Undefined => {
                                return Err(FrameError::LuaRuntimeCrash(
                                    "Network.lua:116: bad argument #1 to 'decode' (string expected, got nil)"
                                        .to_string(),
                                ));
                            }
                            tbl @ (Amf3Value::Array { .. } | Amf3Value::Object { .. }) => {
                                rets.push(tbl.clone());
                            }
                            other => {
                                let bytes = other.as_lua_bytes().ok_or_else(|| {
                                    FrameError::LuaRuntimeCrash(
                                        "Network.lua:116: bad argument #1 to 'decode' (string expected)"
                                            .to_string(),
                                    )
                                })?;
                                rets.push(Amf3Decoder::decode(bytes)?);
                            }
                        }
                    }
                }
                Amf3Value::Object { .. } => {}
                _ => {
                    return Err(FrameError::LuaRuntimeCrash(
                        "Network.lua:112: attempt to get length of field 'ret' (not a table)"
                            .to_string(),
                    ));
                }
            }
        } else {
            rets.push(data.clone());
        }

        if let Some(tok_val) = data.get_field("token") {
            if !matches!(tok_val, Amf3Value::Null | Amf3Value::Undefined) {
                if let Some(s) = tok_val.as_str() {
                    self.token = s.to_string();
                }
            }
        }
        if let Some(time_val) = data.get_field("time") {
            if let Some(ts) = time_val.as_f64() {
                self.time_offset_sec = (ts as i64) - client_now_sec;
            }
        }

        if rets.is_empty() {
            return Err(FrameError::LuaRuntimeCrash(
                "Network.lua:133: Logger.showFatal('onBodyData rets 不合法') (#rets == 0)"
                    .to_string(),
            ));
        }

        for ret in rets {
            let err_str = ret.get_field("err").and_then(|v| v.as_str());
            if err_str == Some("ping") {
                events.push(DispatchedRpcEvent {
                    callback_name: "ping".to_string(),
                    err: "ping".to_string(),
                    ret: Amf3Value::Null,
                    was_push: true,
                });
                continue;
            }

            let cb_obj = ret.get_field("callback").ok_or_else(|| {
                FrameError::LuaRuntimeCrash(
                    "Network.lua:146: attempt to index field 'callback' (a nil value)".to_string(),
                )
            })?;
            let cb_name = cb_obj
                .get_field("callbackName")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    FrameError::LuaRuntimeCrash(
                        "Network.lua:146: callbackName missing or non-string".to_string(),
                    )
                })?
                .to_string();

            let err_owned = err_str
                .ok_or_else(|| {
                    FrameError::LuaRuntimeCrash(
                        "Network.lua:173: bad argument #5 to 'format' (string expected, got nil for ret.err)"
                            .to_string(),
                    )
                })?
                .to_string();

            if err_owned == "ok" {
                let mut was_push = false;
                let had_ctx = if let Some(ctx) = self.contexts.get(&cb_name) {
                    was_push = ctx.is_push;
                    true
                } else {
                    false
                };
                if had_ctx && !was_push {
                    self.contexts.remove(&cb_name);
                }
                if had_ctx {
                    let inner_ret = ret.get_field("ret").cloned().unwrap_or(Amf3Value::Null);
                    events.push(DispatchedRpcEvent {
                        callback_name: cb_name,
                        err: err_owned,
                        ret: inner_ret,
                        was_push,
                    });
                }
            } else {
                if let Some(ctx) = self.contexts.get(&cb_name) {
                    if !ctx.is_push {
                        self.contexts.remove(&cb_name);
                    }
                }
                self.message_boxes
                    .push(format!("callbackName: {cb_name} err: {err_owned}"));
                events.push(DispatchedRpcEvent {
                    callback_name: cb_name,
                    err: err_owned,
                    ret: ret.get_field("ret").cloned().unwrap_or(Amf3Value::Null),
                    was_push: false,
                });
            }
        }

        Ok(())
    }
}

fn lua51_seq_len(dense: &[Amf3Value]) -> usize {
    if dense.is_empty() || matches!(dense[0], Amf3Value::Null | Amf3Value::Undefined) {
        return 0;
    }
    let is_nil = |idx1: usize| -> bool {
        if idx1 == 0 || idx1 > dense.len() {
            true
        } else {
            matches!(dense[idx1 - 1], Amf3Value::Null | Amf3Value::Undefined)
        }
    };
    let mut j = dense.len();
    if !is_nil(j) {
        return j;
    }
    let mut i = 0usize;
    while j - i > 1 {
        let m = (i + j) / 2;
        if is_nil(m) {
            j = m;
        } else {
            i = m;
        }
    }
    i
}
