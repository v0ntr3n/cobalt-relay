#![forbid(unsafe_code)]
//! # `gserver-protocol`
//!
//! Pure-Rust (`#![forbid(unsafe_code)]`, zero external crates) wire protocol implementation for the
//! **GServer (`vnsg2`)** client-server TCP protocol:
//!
//! - [`amf3`]: Complete Adobe AMF3 encoder & decoder matching `Amf3Module.cpp` (`luaopen_amf3`),
//!   including string/object/trait reference tables, binary-safe Lua byte strings, recursion depth
//!   guards, and allocation bounds.
//! - [`zlib`]: Pure-Rust safe RFC 1950 (Zlib) + RFC 1951 (Deflate) decompressor (Stored, Fixed
//!   Huffman, and Dynamic Huffman blocks + Adler-32 verification) and compressor (Stored + Fixed
//!   Huffman with 32 KiB LZ77 matching + Adler-32).
//! - [`md5`]: Self-contained RFC 1321 MD5 + BabelTime `LCPacket` salted XOR-chain MD5.
//! - [`frame`]: Streaming C2S frame decoder (`ClientRxDecoder` supporting 24-byte `LCPacket`,
//!   raw 4-byte framing, and auto-detection) and S2C 8-byte frame encoder/decoder
//!   (`ServerTxEncoder` & `ServerRxDecoder` supporting single, `zipped=1`, `multi=1`, and
//!   `zipped=1 + multi=1`).
//! - [`rpc`]: Exact GServer RPC request (`RpcRequest`), response/push/ping envelope (`S2cEnvelope`),
//!   and multi-response batch codecs (`DecodedS2cBatch`, `encode_s2c_multi_frame`).
//! - [`bt_decrypt`]: `0xFEFEFEFE` 32-byte header asset decryptor & encryptor with all 16 key slots.

pub mod amf3;
pub mod bt_decrypt;
pub mod error;
pub mod frame;
pub mod md5;
pub mod rpc;
pub mod zlib;

pub use amf3::{Amf3Decoder, Amf3Encoder, Amf3Object, DecodeConfig, TraitInfo, Value};
pub use error::{
    Amf3Error, BtDecryptError, EnvelopeError, FrameError, ProtocolError, ZlibError,
};
pub use frame::{
    C2sFrame, C2sFrameMode, C2sWireKind, ClientRxDecoder, S2cFlags, S2cFrame, ServerRxDecoder,
    ServerTxEncoder,
};
pub use rpc::{
    decode_s2c_frame_batch, encode_s2c_multi_batch_amf3, encode_s2c_multi_frame,
    encode_s2c_single_frame, DecodedS2cBatch, MultiItemEncoding, RpcRequest, S2cEnvelope,
};
pub use zlib::{
    adler32, zlib_compress, zlib_compress_with_mode, zlib_decompress, zlib_decompress_with_limit,
    CompressionMode,
};
