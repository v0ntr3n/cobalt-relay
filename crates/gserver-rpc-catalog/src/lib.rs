//! # `gserver-rpc-catalog`
//!
//! Zero-dependency, `#![forbid(unsafe_code)]` pure-Rust protocol catalog, `LCPacket` + AMF3 wire
//! codec, typed bootstrap response builders, and Lua `parseData` schema validators for the
//! **GServer (`vnsg2`)** Cocos2d-x / BabelTime client.
//!
//! ## Modules
//! - [`net_const`]: All **2,107** RPC constants extracted verbatim from `scripts/game/consts/NetConst.lua`
//!   (plus 2,021 Time-Machine `tm*` constants in [`net_const::tm`] and 16 inline RPC/push literals).
//! - [`amf3`]: Pure-Rust AMF3 encoder/decoder matching `Amf3Module.cpp` (`0x09` 1-based dense Lua
//!   sequences vs. `0x0A` anonymous dynamic string-keyed maps, signed 29-bit `U29` integers with
//!   automatic `f64` promotion, and UTF-8 string reference tables).
//! - [`packet`]: Pure-Rust RFC 1321 MD5 and BabelTime `"BabelTime"` rolling-XOR MD5 24-byte
//!   `LCPacket` client frame codec plus 8-byte Server->Client frame header codec (`Network.lua:onBodyData`).
//! - [`envelope`]: `Network.lua` request/response RPC envelopes (`err == "ok"`, `<method>_<mId>`
//!   callback correlation, server Unix timestamp sync, session `token`, and `"dummy"` push frames).
//! - [`bootstrap`]: Exact ordered `TCP Connect` -> `MainTown` (100% `MainLoading`) RPC catalog,
//!   typed response builders (`UserProfile`, `VaConfig`, `BagSnapshot`, `HeroFormationSnapshot`,
//!   `CopyListSnapshot`, `CopyChapterSnapshot`, `EnterViewSnapshot`, `BarrageState`, etc.), and
//!   stateful [`bootstrap::BootstrapSession`] simulator.
//! - [`validator`]: Strict Lua runtime invariant checker catching every `nil`-indexing, string-vs-number,
//!   1-based-array-vs-string-map, server timestamp, and tutorial/first-fight lock trap across the
//!   decrypted Lua client scripts.

#![forbid(unsafe_code)]

pub mod amf3;
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
