# `gserver-rpc-catalog`

Zero-dependency pure-Rust (`#![forbid(unsafe_code)]`) RPC catalog, `LCPacket` + AMF3 wire codec, typed bootstrap response builders, and Lua `parseData` schema validators for **GServer (`vnsg2`)**.

## Contents

- `src/net_const.rs`: All **2,107** constants extracted verbatim from decrypted `scripts/game/consts/NetConst.lua` (plus **2,021** Time-Machine mirror constants in `net_const::tm` and **16** inline RPC/push literals such as `barrage.get`, `barrage.enterRoom`, `barrage.getBarrageRecord`, `halidom.getInfo`, `user.stat`, and `user.guideEvent`).
- `src/packet.rs`: Pure-Rust RFC 1321 MD5 + BabelTime `"BabelTime"` rolling-XOR MD5 24-byte `LCPacket` client frame codec (`LCPacket.cpp`) and 8-byte Server->Client frame header codec (`Network.lua:onBodyData`).
- `src/amf3.rs`: Pure-Rust AMF3 encoder/decoder matching `Amf3Module.cpp` (`0x09` 1-based dense Lua arrays vs. `0x0A` anonymous dynamic string-keyed maps, signed 29-bit `U29` integers with automatic IEEE-754 `f64` promotion, and UTF-8 string reference tables).
- `src/envelope.rs`: `Network.lua` request/response RPC envelopes (`err = "ok"`, `<method>_<mId>` callback correlation, server Unix timestamp sync, `token` propagation, and `"dummy"` push frames).
- `src/bootstrap.rs`: Complete ordered `TCP Connect` -> `MainTown` (100% `MainLoading`) RPC catalog (`BOOTSTRAP_SEQUENCE`), typed builders (`UserProfile`, `VaConfig`, `FormationSquad`, `BagSnapshot`, `HeroFormationSnapshot`, `HeroDto`, `CopyListSnapshot`, `CopyChapterSnapshot`, `EnterViewSnapshot`, `BarrageState`), and stateful `BootstrapSession` server simulator.
- `src/validator.rs`: Strict Lua runtime schema validator catching every `nil`-indexing, string-vs-number, 1-based-array-vs-map, timestamp format (`"YYYYMMDDHHMMSS"`), and tutorial/first-fight lock trap.

## Running Offline Verification

```bash
cargo test --offline
```
