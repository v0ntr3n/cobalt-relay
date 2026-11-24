# GServer (`vnsg2`) — Pure-Rust Private Server (`gserver`)

A zero-external-dependency, `#![forbid(unsafe_code)]` multi-crate Rust private server engineered for the **GServer (`vnsg2` / BabelTime Sanguo 2)** Cocos2d-x C++ / Lua client ([`v0ntr3n/gclient`](https://github.com/v0ntr3n/gclient)).

Every wire frame, cryptographic hash, AMF3 reference table, HTTP MAPI signature, `0xFEFEFEFE` SQLite cell decryptor, and Lua `parseData` schema invariant was reverse-engineered and verified directly against the real C++ client (`LCPacket.cpp`, `CConnection.cpp`, `Amf3Module.cpp`, `BtPlatformWin32.cpp`, `AccountAuthProvider.cpp`, `CConfigManager.cpp`) and decrypted Lua scripts (`Network.lua`, `NetConst.lua`, `LoginProxy.lua`, `UserData.lua`, `BagData.lua`, `HeroFormationData.lua`, `MainTown.lua`, `FightProxy.lua`).

## Workspace Architecture (8 Crates)

| Crate | Responsibility | Safety & Dependencies |
| :--- | :--- | :--- |
| `crates/gserver-protocol` | Adobe AMF3 encoder/decoder (`Amf3Module.cpp`), RFC 1950/1951 Zlib/Deflate codec, RFC 1321 + `"BabelTime"` rolling-XOR MD5, 24-byte `LCPacket` C2S & 8-byte S2C streaming framing, `bt_decrypt` | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver-http` | Multi-threaded HTTP/1.1 Platform/MAPI & Platform GTSDK Auth server (`/phone/getversion`, `/phone/serverlist`, `/phone/notice`, `/phone/supernotice`, `/phone/login`, `/phone/loginrole`, `/phone/gethash` with `PHPSESSID` + `GSESS_ID` `errornu="102"` enforcement, `/api/login/*`, `/admin/status`) | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver-rpc-catalog` | All **2,107** `NetConst.lua` + **2,021** `tm*` constants, ordered **28-step `MainTown` 100% bootstrap** builders, and strict Lua `parseData` schema validator | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver-data` | Exact `bt_decrypt` (`0xFEFEFEFE` 16-key-slot container & `babeltime` SQLite hex-cell unpacker), 59 Three Kingdoms heroes, items, gear suits, towns, bag limits, and 231-level XP curves + runtime JSON DB loader | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver-player` | Persistent accounts, characters, 6-slot squad + 9-cell battle formation, 42 `vnsg2` sub-bags (`10001..740000`), IEEE 802.3 CRC32 Write-Ahead Log (`operations.wal`), and atomic JSON snapshots | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver-gameplay` | Sharded multiplayer town rooms (`city.*`, `imperialcity.*`, `town.addPlayer`/`move`/`delPlayer`/`update`), world & whisper chat + GM slash commands, mail & reward center, tavern gacha (`raffle.*`), shop (`shop.*`), turn-based battle (`appraisal == 50`), PvE copy (`copy.*`), arena (`arena.*`), daily sign-in & online rewards | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver-qa-harness` | Adversarial fuzzing harness (`25,000+` deterministic PRNG malformed AMF3/Zlib/LCPacket/HTTP inputs), 32-thread concurrency stress runner, and standalone QA gateway | `#![forbid(unsafe_code)]`, `std` only |
| `crates/gserver` | Unified production daemon & library bridging `gserver-http`, `gserver-protocol`, `gserver-rpc-catalog`, `gserver-data`, `gserver-player`, and `gserver-gameplay` over live HTTP + TCP listeners | `#![forbid(unsafe_code)]`, `std` only |

## Verified Wire Protocol Summary

1. **HTTP Platform / MAPI (`127.0.0.1:8080`)**:
   - URL signature: `MD5(BtPlatformWin32::sortUrlParams(url_without_sign) + "2A0ce68806G8aJ58Mo6_vnsg2@OmZHv")`.
   - Cookie session binding: `/phone/login/` issues `Set-Cookie: PHPSESSID=...` and `Set-Cookie: GSESS_ID=...`; `/phone/gethash/` verifies session ownership (`errornu = "102"` when missing or mismatched) and issues a signed game login `hash` plus live TCP `host` and `port`.
2. **Client-to-Server TCP (`LCPacket`, 24-Byte Header on `127.0.0.1:8160`)**:
   - `[0..4]`: Big-endian `u32` body length (excluding the 24-byte header).
   - `[4..8]`: Flags `[compressed_flag, 1, 1, 1]`.
   - `[8..24]`: 16-byte salted MD5 over `"BabelTime"` (`9` bytes) + rolling-XOR transformed body (`out[0] = in[0] ^ 0x64`, `out[i] = in[i] ^ out[i - 1]`).
   - `[24..]`: AMF3 request object `{ method, args, token, callback = { callbackName } }` (or RFC 1950 Zlib stream when `flags[0] != 0`).
3. **Server-to-Client TCP (8-Byte Header)**:
   - `[0..4]`: Big-endian `u32` body length (excluding the 8-byte header).
   - `[4..8]`: Flags `[0, zipped, multi, 0]` (`Network.lua` `string.byte(head, 6, 7)` is 1-based).
   - Nested binary blobs in `chat.get*LastMsgs`, `arena.challenge.battleRecord`, and `battle.getRecord` are RFC 1950 Zlib-compressed AMF3 byte strings (`GameUtil:unzipData`).

## Quick Start

### 1. Build & Start the Server
```bash
cargo build --workspace --release
./target/release/gserver --config server.toml
```
Or use `./start_server.sh` (Linux/macOS) or `start_server.bat` (Windows).

### 2. Connect the Win32 Client
Copy `client_launcher/connect_local_win32.bat` next to `GCLIENT.exe` (built from [`v0ntr3n/gclient`](https://github.com/v0ntr3n/gclient)) and launch it, or set:
```bat
set GCLIENT_AUTH_URL=http://127.0.0.1:8080
set GCLIENT_AUTH_URL=http://127.0.0.1:8080
GCLIENT.exe
```

### 3. In-Game GM Slash Commands (Send in World Chat)
- `/help` — List all GM commands in chat
- `/gold <amount>` — Grant Gold (`user.updateUser` live push)
- `/silver <amount>` — Grant Silver/Copper up to `2,000,000,000` cap
- `/vip <0..15>` — Set VIP level and broadcast `town.update` to peers
- `/level <1..230>` — Set character level, unlock formation slots, and broadcast `town.update`
- `/item <template_id> <qty>` — Add items/equipment/fragments with live `bag.update` push
- `/hero <htid>` — Recruit a Three Kingdoms hero (`re.hero.new` live push)
- `/mail <template_id> <qty>` — Deliver system mail + claimable Reward Center bundle (`re.mail.newMail` + `re.reward.new`)
- `/heal` — Restore full stamina (`execution`), `energy`, and hero HP

### 4. Optional: Export Full Client SQLite Databases
```bash
python3 tools/export_db_to_json.py --db-dir /path/to/Resources/res/database --out-dir ./db_export
./target/release/gserver --config server.toml --db-export-dir ./db_export
```

## Full Verification & Fuzzing Suite
Because every crate uses pure Rust `std` with zero external dependencies, the entire workspace builds, lints, fuzzes, and passes integration tests completely offline:
```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -p gserver-qa-harness -- --self-test
```
