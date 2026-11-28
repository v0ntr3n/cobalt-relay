//! `LCPacket` 24-byte C2S and `CConnection` 8-byte S2C framing adapters backed by
//! [`gserver_protocol::{md5, frame, zlib}`].

use std::fmt;

use gserver_protocol::frame::{
    encode_c2s_lcpacket, C2sFrameMode, C2sWireKind, ClientRxDecoder, S2cFlags, ServerRxDecoder,
    ServerTxEncoder, C2S_LCPACKET_HEADER_SIZE, S2C_HEADER_SIZE,
};
use gserver_protocol::md5::compute_babeltime_md5;
use gserver_protocol::{FrameError, ProtocolError};

/// Client -> Server `LCPacket` header length in bytes (`24`).
pub const CLIENT_HEADER_LEN: usize = C2S_LCPACKET_HEADER_SIZE;
/// Default Server -> Client `CConnection` header length in bytes (`8`).
pub const SERVER_HEADER_LEN: usize = S2C_HEADER_SIZE;
/// Hardcoded salt prefix from `LCPacket.cpp`.
pub const BABELTIME_MAGIC_SALT: &[u8; 9] = b"BabelTime";

/// RFC 1321 MD5 digest delegated to [`gserver_protocol::md5::md5`].
#[inline]
pub fn md5(input: &[u8]) -> [u8; 16] {
    gserver_protocol::md5::md5(input)
}

/// 16-byte `LCPacket.cpp` checksum (`"BabelTime"` + rolling XOR) delegated to
/// [`gserver_protocol::md5::compute_babeltime_md5`].
#[inline]
pub fn babeltime_body_checksum(body: &[u8]) -> [u8; 16] {
    compute_babeltime_md5(body)
}

/// Wire framing error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    IncompleteHeader {
        needed: usize,
        available: usize,
    },
    IncompleteBody {
        needed: usize,
        available: usize,
    },
    InvalidFlags([u8; 4]),
    ChecksumMismatch {
        expected: [u8; 16],
        actual: [u8; 16],
    },
    Protocol(ProtocolError),
}

impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteHeader { needed, available } => {
                write!(f, "incomplete packet header: need {needed} bytes, have {available}")
            }
            Self::IncompleteBody { needed, available } => {
                write!(f, "incomplete packet body: need {needed} bytes, have {available}")
            }
            Self::InvalidFlags(fl) => write!(f, "invalid LCPacket flags {fl:?}"),
            Self::ChecksumMismatch { .. } => write!(f, "LCPacket BabelTime MD5 checksum mismatch"),
            Self::Protocol(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for PacketError {}

/// Client -> Server 24-byte `LCPacket` frame (`body` holds the uncompressed AMF3 payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFrame {
    pub zipped: bool,
    pub body: Vec<u8>,
}

impl ClientFrame {
    pub fn uncompressed(body: Vec<u8>) -> Self {
        Self {
            zipped: false,
            body,
        }
    }

    pub fn compressed(body: Vec<u8>) -> Self {
        Self { zipped: true, body }
    }

    /// Encode via [`gserver_protocol::frame::encode_c2s_lcpacket`] (applying RFC 1950 Zlib when `zipped`).
    pub fn encode(&self) -> Vec<u8> {
        if self.body.is_empty() && !self.zipped {
            let mut out = Vec::with_capacity(CLIENT_HEADER_LEN);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&[0, 1, 1, 1]);
            out.extend_from_slice(&babeltime_body_checksum(&[]));
            return out;
        }
        encode_c2s_lcpacket(&self.body, self.zipped)
            .expect("client frame body within u32 frame limit")
    }

    /// Decode one `ClientFrame` from the front of `stream` via [`ClientRxDecoder`], returning `(frame, consumed_bytes)`.
    pub fn decode(stream: &[u8]) -> Result<(Self, usize), PacketError> {
        if stream.len() < CLIENT_HEADER_LEN {
            return Err(PacketError::IncompleteHeader {
                needed: CLIENT_HEADER_LEN,
                available: stream.len(),
            });
        }
        let mut rx = ClientRxDecoder::new(C2sFrameMode::LcPacket24);
        rx.feed(stream);
        match rx.next_frame() {
            Ok(Some(c2s)) => {
                let zipped = matches!(c2s.kind, C2sWireKind::LcPacket24 { compressed: true, .. });
                let total = CLIENT_HEADER_LEN + (c2s.wire_body_len as usize);
                Ok((
                    Self {
                        zipped,
                        body: c2s.amf3_payload,
                    },
                    total,
                ))
            }
            Ok(None) => {
                let body_len =
                    u32::from_be_bytes([stream[0], stream[1], stream[2], stream[3]]) as usize;
                Err(PacketError::IncompleteBody {
                    needed: CLIENT_HEADER_LEN + body_len,
                    available: stream.len(),
                })
            }
            Err(ProtocolError::Frame(FrameError::InvalidLcFlags(flags))) => {
                Err(PacketError::InvalidFlags(flags))
            }
            Err(ProtocolError::Frame(FrameError::Md5Mismatch { expected, actual })) => {
                Err(PacketError::ChecksumMismatch {
                    expected: actual,
                    actual: expected,
                })
            }
            Err(other) => Err(PacketError::Protocol(other)),
        }
    }
}

/// Server -> Client 8-byte `CConnection` frame (`Network.lua:onBodyData(body, head)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFrame {
    /// Byte 5 (Lua 1-indexed byte 6): `true` if wire payload is Zlib-compressed.
    pub zipped: bool,
    /// Byte 6 (Lua 1-indexed byte 7): `true` if `data.ret` is a 1-based multi-packet batch array.
    pub multi: bool,
    /// Decompressed S2C payload bytes.
    pub body: Vec<u8>,
}

impl ServerFrame {
    pub fn single_uncompressed(body: Vec<u8>) -> Self {
        Self {
            zipped: false,
            multi: false,
            body,
        }
    }

    pub fn single_compressed(body: Vec<u8>) -> Self {
        Self {
            zipped: true,
            multi: false,
            body,
        }
    }

    /// Encode via [`ServerTxEncoder`] (applying [`gserver_protocol::zlib`] when `zipped`).
    pub fn encode(&self) -> Vec<u8> {
        let flags = S2cFlags::new(self.zipped, self.multi);
        if self.body.is_empty() && !self.zipped {
            let mut out = Vec::with_capacity(SERVER_HEADER_LEN);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&flags.to_bytes());
            return out;
        }
        ServerTxEncoder::new()
            .encode_frame(&self.body, flags)
            .expect("server frame body within configured frame limit")
    }

    /// Decode one `ServerFrame` from the front of `stream` via [`ServerRxDecoder`], returning `(frame, consumed_bytes)`.
    pub fn decode(stream: &[u8]) -> Result<(Self, usize), PacketError> {
        if stream.len() < SERVER_HEADER_LEN {
            return Err(PacketError::IncompleteHeader {
                needed: SERVER_HEADER_LEN,
                available: stream.len(),
            });
        }
        let mut rx = ServerRxDecoder::new();
        rx.feed(stream);
        match rx.next_frame() {
            Ok(Some(s2c)) => {
                let total = SERVER_HEADER_LEN + (s2c.wire_body_len as usize);
                Ok((
                    Self {
                        zipped: s2c.flags.is_zipped(),
                        multi: s2c.flags.is_multi(),
                        body: s2c.payload,
                    },
                    total,
                ))
            }
            Ok(None) => {
                let body_len =
                    u32::from_be_bytes([stream[0], stream[1], stream[2], stream[3]]) as usize;
                Err(PacketError::IncompleteBody {
                    needed: SERVER_HEADER_LEN + body_len,
                    available: stream.len(),
                })
            }
            Err(other) => Err(PacketError::Protocol(other)),
        }
    }
}
