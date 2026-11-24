#![forbid(unsafe_code)]
#![allow(clippy::should_implement_trait, clippy::chunks_exact_to_as_chunks, clippy::manual_is_multiple_of)]

//! `gserver-qa-harness` — Pure-Rust, zero-dependency adversarial QA, deterministic PRNG fuzzing,
//! 32-thread concurrency stress simulator, and local private-server gateway for the GServer (`vnsg2`)
//! Win32 C++ client (`Amf3Module.cpp`, `LCPacket.cpp`, `CConnection.cpp`, `BtPlatformWin32.cpp`,
//! `AccountAuthProvider.cpp`, `AppDelegate.cpp`, and `Network.lua`).

pub mod amf3;
pub mod crypto;
pub mod framing;
pub mod fuzzer;
pub mod http_gateway;
pub mod server;
pub mod zlib;
