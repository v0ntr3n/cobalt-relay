#![forbid(unsafe_code)]

//! Asymmetric wire framing codecs & stateful `GCLIENT.exe` (`CConnection.cpp` + `Network.lua`)
//! behavioral simulator.
//!
//! ## Critical Asymmetric Wire Framing Verified from Source
//!
//! ### 1. Client -> Server TX (`LCPacket.cpp:271-308`, 24-Byte Header)
//! - `bytes[0..4]`: Big-Endian `u32` `body_len` (`> 0`, server rejects `> 16 MiB` BEFORE allocating!)
//! - `bytes[4..8]`: Protocol flags `{ compress ? 1 : 0, 1, 1, 1 }`
//! - `bytes[8..24]`: 16-byte raw `BabelTime` adjacent-XOR MD5 digest (`compute_babeltime_md5`)
//! - `bytes[24 .. 24 + body_len]`: Zlib-compressed (when `flags[0] != 0`) or raw AMF3 payload
//!
//! ### 2. Server -> Client RX (`CConnection.h:216`, `CConnection.cpp:489-599`, `Network.lua:102-186`, 8-Byte Header)
//! - `head[0..4]` (Lua `1..4`): Big-Endian `u32` `body_len` (**MUST be `> 0`**; `0` triggers `HEAD_SCRIPT_ERROR` -> `close()`!)
//! - `head[4]`    (Lua `5`):    `flag0` (`0x00`)
//! - `head[5]`    (Lua `6`):    `zipped` (`string.byte(head, 6, 7)` 1st return! `!= 0` -> `GameUtil:unzipData`)
//! - `head[6]`    (Lua `7`):    `multi`  (`string.byte(head, 6, 7)` 2nd return! `!= 0` -> `for i = 1, #data.ret`)
//! - `head[7]`    (Lua `8`):    `flag3` (`0x00`)

use std::collections::HashMap;
use std::fmt;

use crate::amf3::{Amf3Decoder, Amf3Encoder, Amf3Error, Amf3Value};
use crate::crypto::compute_babeltime_md5;
use crate::zlib::{zlib_compress, zlib_decompress_bounded, ZlibError};

pub const CLIENT_LC_HEADER_LEN: usize = 24;
pub const SERVER_RX_HEADER_LEN: usize = 8;

/// Strict private-server DoS ceiling (`16 MiB` = `16,777,216` bytes) for both raw incoming
/// frame bodies and decompressed zlib streams.
pub const MAX_SERVER_FRAME_BODY_LEN: usize = 16 * 1024 * 1024;

/// Client-side `CConnection.cpp:30` `kMaxFrameBodyLength` (`64 MiB`).
pub const CLIENT_MAX_FRAME_BODY_LEN: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    ZeroLengthBody,
    OversizedBody {
        declared: u32,
        max: usize,
    },
    InvalidClientFlags([u8; 4]),
    Md5Mismatch {
        expected: [u8; 16],
        actual: [u8; 16],
    },
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
                write!(
                    f,
                    "oversized frame body {} bytes exceeds max {} bytes",
                    declared, max
                )
            }
            Self::InvalidClientFlags(fl) => write!(f, "invalid LCPacket flags {:?}", fl),
            Self::Md5Mismatch { .. } => write!(f, "BabelTime 16-byte MD5 integrity check failed"),
            Self::Zlib(e) => write!(f, "zlib decompression error: {}", e),
            Self::Amf3(e) => write!(f, "AMF3 codec error: {}", e),
            Self::ClientSocketClosed(reason) => write!(f, "Win32 CConnection closed: {}", reason),
            Self::LuaRuntimeCrash(msg) => {
                write!(f, "Win32 Lua runtime panic in Network.lua: {}", msg)
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

// ============================================================================
// Client -> Server 24-Byte `LCPacket` Encoder & Streaming Server Decoder
// ============================================================================

/// Encodes a client->server 24-byte `LCPacket` matching `LCPacket::makeLCPacket` (`LCPacket.cpp:271-308`).
pub fn encode_client_lc_packet(raw_payload: &[u8], compress: bool) -> Vec<u8> {
    let body = if compress {
        zlib_compress(raw_payload)
    } else {
        raw_payload.to_vec()
    };
    let body_len = body.len() as u32;
    let md5 = compute_babeltime_md5(&body);

    let mut pkt = Vec::with_capacity(CLIENT_LC_HEADER_LEN + body.len());
    pkt.extend_from_slice(&body_len.to_be_bytes());
    pkt.push(if compress { 1 } else { 0 });
    pkt.push(1);
    pkt.push(1);
    pkt.push(1);
    pkt.extend_from_slice(&md5);
    pkt.extend_from_slice(&body);
    pkt
}

/// Checks if `data` is already a valid framed 24-byte `LCPacket` (`CConnection.cpp:347` idempotency guard).
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
    let expected = compute_babeltime_md5(&data[CLIENT_LC_HEADER_LEN..]);
    data[8..24] == expected
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
    /// Buffer is empty (`0` unconsumed bytes); awaiting next 24-byte `LCPacket` frame.
    Idle,
    /// Partial 4-byte big-endian `body_len` prefix buffered (`1..=3` bytes).
    AwaitingLengthPrefix { buffered_bytes: usize },
    /// Validated `body_len` prefix present, awaiting remainder of 24-byte `LCPacket` header (`4..=23` bytes).
    AwaitingLcHeader {
        declared_body_len: u32,
        buffered_bytes: usize,
    },
    /// Complete 24-byte `LCPacket` header buffered, awaiting remaining payload body bytes.
    AwaitingFrameBody {
        declared_body_len: u32,
        buffered_body_bytes: usize,
    },
}

impl fmt::Display for DecoderParserState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Idle => write!(f, "Idle"),
            Self::AwaitingLengthPrefix { buffered_bytes } => {
                write!(f, "AwaitingLengthPrefix({}/4B)", buffered_bytes)
            }
            Self::AwaitingLcHeader {
                declared_body_len,
                buffered_bytes,
            } => write!(
                f,
                "AwaitingLcHeader(hdr={}/24B, declared_body={}B)",
                buffered_bytes, declared_body_len
            ),
            Self::AwaitingFrameBody {
                declared_body_len,
                buffered_body_bytes,
            } => write!(
                f,
                "AwaitingFrameBody(body={}/{}B)",
                buffered_body_bytes, declared_body_len
            ),
        }
    }
}

/// Stateful, DoS-hardened streaming decoder for client->server 24-byte `LCPacket` streams.
/// Handles 1-byte TCP segmentation, pipelined multi-packet bursts, immediate `> 16 MiB`
/// rejection BEFORE body allocation, MD5 integrity verification, and bounded zlib inflation.
pub struct ServerRxStreamDecoder {
    buf: Vec<u8>,
    read_offset: usize,
    max_body_len: usize,
    max_decompressed_len: usize,
    require_strict_flags: bool,
}

impl Default for ServerRxStreamDecoder {
    fn default() -> Self {
        Self::new(MAX_SERVER_FRAME_BODY_LEN, MAX_SERVER_FRAME_BODY_LEN)
    }
}

impl ServerRxStreamDecoder {
    pub fn new(max_body_len: usize, max_decompressed_len: usize) -> Self {
        Self {
            buf: Vec::new(),
            read_offset: 0,
            max_body_len,
            max_decompressed_len,
            require_strict_flags: true,
        }
    }

    pub fn buffered_len(&self) -> usize {
        self.buf.len().saturating_sub(self.read_offset)
    }

    /// Returns the exact current framing parser stage for structured connection diagnostics.
    pub fn parser_state(&self) -> DecoderParserState {
        let avail = self.buffered_len();
        if avail == 0 {
            return DecoderParserState::Idle;
        }
        if avail < 4 {
            return DecoderParserState::AwaitingLengthPrefix {
                buffered_bytes: avail,
            };
        }
        let hdr_slice = &self.buf[self.read_offset..];
        let declared_body_len =
            u32::from_be_bytes([hdr_slice[0], hdr_slice[1], hdr_slice[2], hdr_slice[3]]);
        if avail < CLIENT_LC_HEADER_LEN {
            DecoderParserState::AwaitingLcHeader {
                declared_body_len,
                buffered_bytes: avail,
            }
        } else {
            DecoderParserState::AwaitingFrameBody {
                declared_body_len,
                buffered_body_bytes: avail - CLIENT_LC_HEADER_LEN,
            }
        }
    }

    /// Feeds incoming TCP bytes and extracts all complete, validated client packets.
    /// Immediately returns `Err(FrameError::OversizedBody)` as soon as a 4-byte or 24-byte header
    /// declares `body_len > max_body_len`, WITHOUT waiting for or allocating the oversized body!
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Result<Vec<DecodedClientPacket>, FrameError> {
        if !chunk.is_empty() {
            self.buf.extend_from_slice(chunk);
        }

        let mut packets = Vec::new();
        loop {
            let avail = self.buf.len() - self.read_offset;
            if avail < 4 {
                break;
            }
            let hdr_slice = &self.buf[self.read_offset..];
            let body_len =
                u32::from_be_bytes([hdr_slice[0], hdr_slice[1], hdr_slice[2], hdr_slice[3]]);
            if body_len == 0 {
                self.clear();
                return Err(FrameError::ZeroLengthBody);
            }
            if (body_len as usize) > self.max_body_len {
                self.clear();
                return Err(FrameError::OversizedBody {
                    declared: body_len,
                    max: self.max_body_len,
                });
            }

            if avail < CLIENT_LC_HEADER_LEN {
                break;
            }

            let flags = [hdr_slice[4], hdr_slice[5], hdr_slice[6], hdr_slice[7]];
            if self.require_strict_flags
                && ((flags[0] != 0 && flags[0] != 1)
                    || flags[1] != 1
                    || flags[2] != 1
                    || flags[3] != 1)
            {
                self.clear();
                return Err(FrameError::InvalidClientFlags(flags));
            }

            let total_frame_len = CLIENT_LC_HEADER_LEN + (body_len as usize);
            if avail < total_frame_len {
                break;
            }

            let mut md5 = [0u8; 16];
            md5.copy_from_slice(&hdr_slice[8..24]);
            let body = &hdr_slice[CLIENT_LC_HEADER_LEN..total_frame_len];

            let expected_md5 = compute_babeltime_md5(body);
            if md5 != expected_md5 {
                self.clear();
                return Err(FrameError::Md5Mismatch {
                    expected: expected_md5,
                    actual: md5,
                });
            }

            let raw_amf3 = if flags[0] != 0 {
                match zlib_decompress_bounded(body, self.max_decompressed_len) {
                    Ok(decompressed) => decompressed,
                    Err(e) => {
                        self.clear();
                        return Err(FrameError::Zlib(e));
                    }
                }
            } else {
                body.to_vec()
            };

            let value = match Amf3Decoder::decode(&raw_amf3) {
                Ok(v) => v,
                Err(e) => {
                    self.clear();
                    return Err(FrameError::Amf3(e));
                }
            };

            self.read_offset += total_frame_len;
            packets.push(DecodedClientPacket {
                body_len,
                flags,
                md5,
                raw_amf3,
                value,
            });
        }

        self.compact();
        Ok(packets)
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.read_offset = 0;
    }

    fn compact(&mut self) {
        if self.read_offset == 0 {
            return;
        }
        if self.read_offset >= self.buf.len() {
            self.buf.clear();
            self.read_offset = 0;
        } else if self.read_offset >= 4096 || self.read_offset * 2 >= self.buf.len() {
            self.buf.drain(..self.read_offset);
            self.read_offset = 0;
        }
    }
}

// ============================================================================
// Server -> Client 8-Byte Frame Builder (`CConnection.cpp` + `Network.lua`)
// ============================================================================

/// Encodes a single server->client 8-byte framed packet (`multi = 0`):
/// `head = [body_len BE (4B), flag0=0, zipped=head[5], multi=0 (head[6]), flag3=0]`.
pub fn encode_server_single_frame(
    payload: &Amf3Value,
    compress: bool,
) -> Result<Vec<u8>, FrameError> {
    let raw_amf3 = Amf3Encoder::encode(payload)?;
    encode_server_raw_frame(&raw_amf3, compress, false)
}

/// Encodes a multi-RPC batch server->client 8-byte framed packet (`multi = 1` at `head[6]`,
/// Lua `string.byte(head, 7)`!).
///
/// Supports both `Network.lua:112-118` inner representations:
/// - `encode_inner_as_bytes = false`: `data.ret[i]` is a decoded AMF3 table (`type == "table"`).
/// - `encode_inner_as_bytes = true`: `data.ret[i]` is a nested AMF3 binary `0x06 String`
///   decoded via `amf3.decode(data.ret[i])` on line 116!
pub fn encode_server_multi_frame(
    rets: &[Amf3Value],
    token: Option<&str>,
    server_time_sec: Option<i64>,
    compress: bool,
    encode_inner_as_bytes: bool,
) -> Result<Vec<u8>, FrameError> {
    let mut dense_items = Vec::with_capacity(rets.len());
    for item in rets {
        if encode_inner_as_bytes {
            let nested = Amf3Encoder::encode(item)?;
            dense_items.push(Amf3Value::String(nested));
        } else {
            dense_items.push(item.clone());
        }
    }

    let mut fields = vec![("ret", Amf3Value::dense_array(dense_items))];
    if let Some(tok) = token {
        fields.push(("token", Amf3Value::from_str(tok)));
    }
    if let Some(ts) = server_time_sec {
        // CRITICAL: Must use `from_i64_auto` -> `0x05 Double` for timestamps > 268,435,455!
        fields.push(("time", Amf3Value::from_i64_auto(ts)));
    }

    let outer = Amf3Value::anonymous_object(fields);
    let raw_amf3 = Amf3Encoder::encode(&outer)?;
    encode_server_raw_frame(&raw_amf3, compress, true)
}

/// Low-level 8-byte server->client header builder verifying 0-based `head[5] = zipped`
/// and `head[6] = multi` (matching 1-based Lua `string.byte(head, 6, 7)`).
pub fn encode_server_raw_frame(
    raw_amf3: &[u8],
    compress: bool,
    multi: bool,
) -> Result<Vec<u8>, FrameError> {
    if raw_amf3.is_empty() {
        return Err(FrameError::ZeroLengthBody);
    }
    let body = if compress {
        zlib_compress(raw_amf3)
    } else {
        raw_amf3.to_vec()
    };
    if body.is_empty() {
        return Err(FrameError::ZeroLengthBody);
    }
    let body_len = body.len() as u32;
    let mut out = Vec::with_capacity(SERVER_RX_HEADER_LEN + body.len());
    out.extend_from_slice(&body_len.to_be_bytes()); // head[0..4] -> Lua 1..4
    out.push(0x00); // head[4]    -> Lua 5 (flag0)
    out.push(if compress { 1 } else { 0 }); // head[5]    -> Lua 6 (zipped!)
    out.push(if multi { 1 } else { 0 }); // head[6]    -> Lua 7 (multi!)
    out.push(0x00); // head[7]    -> Lua 8 (flag3)
    out.extend_from_slice(&body);
    Ok(out)
}

/// Builds standard GServer RPC response envelope `{ err = "ok", callback = { callbackName = ... }, ret = ... }`.
pub fn build_rpc_response(
    err: &str,
    callback_name: &str,
    ret: Amf3Value,
    token: Option<&str>,
    server_time_sec: Option<i64>,
) -> Amf3Value {
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

/// Builds a server heartbeat `{ err = "ping", time = ... }` (`Network.lua:144` skips `ret.callback`).
pub fn build_ping_response(server_time_sec: i64) -> Amf3Value {
    Amf3Value::anonymous_object(vec![
        ("err", Amf3Value::from_str("ping")),
        ("time", Amf3Value::from_i64_auto(server_time_sec)),
    ])
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
/// `Network.lua` (`ctor`, `sendMsg`, `sendStatMsg`, `sendUnusedMsg`, `onBodyData`).
///
/// Reproduces EVERY real Win32 client failure mode when fed malformed private-server frames:
/// - `body_len == 0` or `> 64 MiB` -> `HEAD_SCRIPT_ERROR` socket close (`CConnection.cpp:528-533`)
/// - Off-by-one `zipped`/`multi` flags (`head[4]` vs `head[5]`/`head[6]`)
/// - 24-byte `LCPacket` accidentally sent to 8-byte RX client
/// - `null`/`undefined` nil-holes inside `multi=1` `data.ret` array (`#data.ret` Lua 5.1 semantics!)
/// - Missing `ret.callback.callbackName` when `ret.err != "ping"`
/// - Timestamp truncation corrupting `self.mOffset = data.time - client_now_sec`
pub struct Win32ClientSimulator {
    pub token: String,
    pub time_offset_sec: i64,
    pub m_id: u64,
    pub socket_closed: bool,
    pub close_reason: Option<&'static str>,
    pub message_boxes: Vec<String>,

    recv_buf: Vec<u8>,
    recv_cursor: usize,
    expected_body_len: u32,
    current_header: [u8; SERVER_RX_HEADER_LEN],
    contexts: HashMap<String, PendingRpcContext>,
}

impl Default for Win32ClientSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Win32ClientSimulator {
    /// Matches `Network:ctor()` (`Network.lua:36-45`): `mToken = "0"`, `mOffset = 0`, `mId = 1`.
    pub fn new() -> Self {
        Self {
            token: "0".to_string(),
            time_offset_sec: 0,
            m_id: 1,
            socket_closed: false,
            close_reason: None,
            message_boxes: Vec::new(),
            recv_buf: Vec::new(),
            recv_cursor: 0,
            expected_body_len: 0,
            current_header: [0u8; SERVER_RX_HEADER_LEN],
            contexts: HashMap::new(),
        }
    }

    /// Registers a persistent server-push handler (`Network:regPushCallback`, `context[3] = true`).
    pub fn reg_push_callback(&mut self, method: &str) {
        self.contexts.insert(
            method.to_string(),
            PendingRpcContext {
                is_push: true,
                request_obj: Amf3Value::Null,
            },
        );
    }

    /// Encodes a standard client->server RPC via `Network:sendMsg` (`Network.lua:265-288`):
    /// increments `self.mId` first (`_2`, `_3`, ...), registers `mContext[callbackName]`,
    /// AMF3-encodes `{ method, args, token, callback = { callbackName } }`, and wraps in a
    /// 24-byte `LCPacket` (`compress = true`).
    pub fn build_send_msg(
        &mut self,
        method: &str,
        args: Amf3Value,
        compress: bool,
    ) -> (String, Vec<u8>) {
        self.m_id += 1;
        let callback_name = format!("{}_{}", method, self.m_id);
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

    /// Encodes a fire-and-forget `user.stat` telemetry packet (`Network:sendStatMsg`, `Network.lua:304-319`),
    /// which intentionally does NOT insert into `mContext`.
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

    /// Replays all unacknowledged one-shot RPCs with updated `self.mToken` (`Network:sendUnusedMsg`,
    /// `Network.lua:205-216`).
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

    /// Feeds incoming server->client TCP bytes into `CConnection::doRead()` -> `Network:onBodyData`.
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
        if !chunk.is_empty() {
            self.recv_buf.extend_from_slice(chunk);
        }

        let mut events = Vec::new();
        loop {
            let avail = self.recv_buf.len() - self.recv_cursor;
            if self.expected_body_len == 0 {
                if avail < SERVER_RX_HEADER_LEN {
                    break;
                }
                let hdr_slice =
                    &self.recv_buf[self.recv_cursor..self.recv_cursor + SERVER_RX_HEADER_LEN];
                let body_len =
                    u32::from_be_bytes([hdr_slice[0], hdr_slice[1], hdr_slice[2], hdr_slice[3]]);
                // Exact `CConnection.cpp:493, 528-533` check!
                if body_len == 0 || (body_len as usize) > CLIENT_MAX_FRAME_BODY_LEN {
                    self.socket_closed = true;
                    self.close_reason = Some("HEAD_SCRIPT_ERROR");
                    self.recv_buf.clear();
                    self.recv_cursor = 0;
                    return Err(FrameError::ClientSocketClosed("HEAD_SCRIPT_ERROR"));
                }
                self.current_header.copy_from_slice(hdr_slice);
                self.recv_cursor += SERVER_RX_HEADER_LEN;
                self.expected_body_len = body_len;
            }

            let body_avail = self.recv_buf.len() - self.recv_cursor;
            let needed = self.expected_body_len as usize;
            if body_avail < needed {
                break;
            }

            let body = self.recv_buf[self.recv_cursor..self.recv_cursor + needed].to_vec();
            let head = self.current_header;
            self.recv_cursor += needed;
            self.expected_body_len = 0;

            self.on_body_data(&body, &head, client_now_sec, &mut events)?;
        }

        if self.recv_cursor > 0 {
            if self.recv_cursor >= self.recv_buf.len() {
                self.recv_buf.clear();
                self.recv_cursor = 0;
            } else if self.recv_cursor >= 4096 || self.recv_cursor * 2 >= self.recv_buf.len() {
                self.recv_buf.drain(..self.recv_cursor);
                self.recv_cursor = 0;
            }
        }

        Ok(events)
    }

    fn on_body_data(
        &mut self,
        body: &[u8],
        head: &[u8; SERVER_RX_HEADER_LEN],
        client_now_sec: i64,
        events: &mut Vec<DispatchedRpcEvent>,
    ) -> Result<(), FrameError> {
        // Exact `Network.lua:104`: `local zipped, multi = string.byte(head, 6, 7);`
        // Lua 1-based index 6 == Rust 0-based `head[5]`!
        // Lua 1-based index 7 == Rust 0-based `head[6]`!
        let zipped = head[5];
        let multi = head[6];

        let raw_amf3 = if zipped != 0 {
            zlib_decompress_bounded(body, CLIENT_MAX_FRAME_BODY_LEN)?
        } else {
            body.to_vec()
        };

        let data = Amf3Decoder::decode(&raw_amf3)?;
        let mut rets = Vec::new();

        if multi != 0 {
            let ret_field = data.get_field("ret").ok_or_else(|| {
                FrameError::LuaRuntimeCrash(
                    "Network.lua:112: attempt to get length of field 'ret' (a nil value)"
                        .to_string(),
                )
            })?;
            match ret_field {
                Amf3Value::Array { dense, .. } => {
                    // Simulate Lua 5.1 `#data.ret`: stops at nil holes (or panics if nil element is passed to `amf3.decode`)!
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
                Amf3Value::Object { .. } => {
                    // In Lua 5.1, `#` on a pure string-keyed table is `0`!
                }
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

        // `Network.lua:123-130`: outer `data.token` and `data.time`
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

        // `Network.lua:132-134`: `if(#rets == 0) then Logger.showFatal("onBodyData rets 不合法") end`
        if rets.is_empty() {
            return Err(FrameError::LuaRuntimeCrash(
                "Network.lua:133: Logger.showFatal('onBodyData rets 不合法') (#rets == 0)"
                    .to_string(),
            ));
        }

        for ret in rets {
            let err_val = ret.get_field("err");
            let err_str = match err_val {
                Some(v) => v.as_str(),
                None => None,
            };

            if err_str == Some("ping") {
                events.push(DispatchedRpcEvent {
                    callback_name: "ping".to_string(),
                    err: "ping".to_string(),
                    ret: Amf3Value::Null,
                    was_push: true,
                });
                continue;
            }

            // `Network.lua:146`: `local callbackName = ret.callback.callbackName`
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
                    // `Network.lua:173`: `string.format(..., ret.err)` throws in Lua 5.1 when `ret.err == nil`!
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
                // `Network.lua:170-180`: CCMessageBox + clear one-shot context
                if let Some(ctx) = self.contexts.get(&cb_name) {
                    if !ctx.is_push {
                        self.contexts.remove(&cb_name);
                    }
                }
                self.message_boxes
                    .push(format!("callbackName: {} err: {}", cb_name, err_owned));
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

/// Models Lua 5.1 `luaH_getn` (`#t`): if `t[1] == nil`, length is `0`; if `t[N] == nil`,
/// scans backward/binary-searches boundary. For dense prefix before first nil hole when trailing
/// element is non-nil, Lua 5.1 binary search can either stop before the hole OR land past the hole
/// (exposing `t[i] == nil` inside `for i = 1, #t`)!
fn lua51_seq_len(dense: &[Amf3Value]) -> usize {
    if dense.is_empty() || matches!(dense[0], Amf3Value::Null | Amf3Value::Undefined) {
        return 0;
    }
    // Exact port of Lua 5.1 `unbound_search` when array part covers `1..=dense.len()`:
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
