# `gserver-qa-harness` — Adversarial QA, Fuzzing & Protocol Audit Report (`vnsg2`)

Pure-Rust, zero-dependency (`#![forbid(unsafe_code)]`) adversarial QA, deterministic PRNG fuzzing, 32-thread concurrency stress harness, and local SDK HTTP + TCP Game gateway for the GServer (`vnsg2`) Win32 C++ client.

## Critical Protocol Compatibility Traps Audited from Win32 C++ & `Network.lua`

### 1. `Amf3Module.cpp` Codec & Lua 5.1 Traps

| Trap | Exact Client Source Behavior | Failure Mode on Private Server / Client | `gserver-qa-harness` Enforcement |
| :--- | :--- | :--- | :--- |
| **29-bit Signed `0x04` Bounds & Negative 4-Byte U29** | `kAmf3IntMin = -268,435,456` (`-2^28`), `kAmf3IntMax = 268,435,455` (`2^28 - 1`). Negative `iv` masks `(uint32_t)iv & 0x1FFFFFFF` (`bit 28 = 1`), so **every negative U29 (`-268,435,456..-1`) uses the 4-byte U29 branch** where byte 4 carries 8 bits (`(acc << 8) \| b3`) and decodes via `raw \| 0xE0000000` (`Amf3Module.cpp:92-109, 710-712`). | Encoders that use 7 bits on byte 4 or standard LEB128/ZigZag corrupt every negative number (`-1` becomes `127` or `2097151`). | Tested across `-268_435_456`, `-1`, `0`, `127`, `128`, `16_383`, `16_384`, `2_097_151`, `2_097_152`, `268_435_455`. |
| **Unix Timestamp (`1795500450`) Overflow vs `0x05 Double`** | Values `< -268,435,456` or `> 268,435,455` MUST be encoded as `0x05 Double` (8-byte BE IEEE-754 `f64`, `Amf3Module.cpp:157-169`). | Masking `1795500450` (`0x6B0529A2`) into `0x04` U29 truncates the top 3 bits to `184,887,714` (or sign-extends `0x70000000` to `-268,435,456`), poisoning `Network.lua:128` `self.mOffset = data.time - os.time()` by ~1.6B seconds. | `Amf3Value::from_i64_auto` & `Amf3Encoder` auto-promote any integer outside U29 range to `0x05 Double`; `> 2^53` IDs (`qid = "1243831974477471744"`) promote to `0x06 String`. |
| **Empty Lua Table `{}` vs Dynamic Object** | `Amf3Encoder::encodeTable` (`Amf3Module.cpp:228-281`) encodes `{}` (`denseCount == 0 && !hasNonDenseKeys`) as **`0x09 Array` (`[0x09, 0x01, 0x01]`)**, while pure string-keyed maps encode as anonymous dynamic `0x0A Object` (`[0x0A, 0x0B, 0x01, ..., 0x01]`). | Server decoders expecting `{}` (`args = {}`) to carry marker `0x0A` reject every no-arg client RPC (`Network.lua:274`). | Full support for `[0x09, 0x01, 0x01]`, mixed `0x09` associative+dense tables, and `0x0A` sealed/dynamic/trait-ref objects. |
| **Empty String `""` & Shared `stringRefs`** | `len == 0` (`0x01`) is **never** inserted into `stringRefs` (`Amf3Module.cpp:128-132, 478-482`). All `0x06` strings, `0x09` keys, and `0x0A` class/member/dynamic keys share one `stringRefs` vector. | Indexing `""` shifts every subsequent string reference by +1, corrupting `"err"` / `"callbackName"` or triggering `luaL_error("invalid string reference index")`. | Verified with interleaved `["" , "alpha", "", "alpha"]` wire tests and out-of-bounds ref rejection. |
| **`0x06 String` vs `0x0C ByteArray` Ref Tables** | Lua 5.1 strings are raw byte slices. `Amf3Decoder` pushes both `0x06` and `0x0C` as Lua strings, **but** `0x06` uses `stringRefs` while `0x0C` (even 0-byte `0x0C 0x01`!) uses `objRefTableIdx` (`Amf3Module.cpp:508-524`). Client->Server always emits `0x06` even for binary bytes. | Strict `std::str::from_utf8` on `0x06` rejects binary Lua strings; mixing `0x0C` into `stringRefs` shifts object/string reference tables. | `Amf3Value::String(Vec<u8>)` is 8-bit clean; `ByteArray` is tracked exclusively in `object_refs`. |
| **`null`/`undefined` Nil-Holes in Dense `0x09` Arrays** | `Amf3Decoder::decodeArray` (`Amf3Module.cpp:578-587`) pushes `nil` for `0x00`/`0x01`, creating a nil hole via `lua_rawseti`. | In Lua 5.1, `#data.ret == 0` if `ret[1] == nil` (`Logger.showFatal("onBodyData rets 不合法")` at `Network.lua:133`), or `#data.ret == 3` on `{pkt1, nil, pkt3}` causing `amf3.decode(nil)` **uncaught C/Lua error** at `Network.lua:116`! | `audit_lua_client_hazards` flags `DenseArrayNilHole` at compile/test time; `Win32ClientSimulator` reproduces exact Lua 5.1 `#t` failure modes. |
| **Nesting Depth & U29 Allocation Bombs** | `kMaxRecursionDepth = 64` (`Amf3Module.cpp:37, 683`). | `depth >= 65` triggers `luaL_error` on client or stack-overflows recursive servers; `[0x09, 0xFF, 0xFF, 0xFF, 0xFF]` causes multi-GiB `Vec::with_capacity` OOM. | Enforces `MAX_RECURSION_DEPTH = 64` and caps container pre-allocation by remaining wire bytes. |

### 2. `LCPacket.cpp` + `CConnection.cpp` + `Network.lua` Framing Asymmetry

- **Client -> Server TX (24-Byte `LCPacket`, `LCPacket.cpp:271-308`)**:
  - `[0..4]`: Big-Endian `u32` `bodyLen`
  - `[4..8]`: `{ compress ? 1 : 0, 1, 1, 1 }` (**compressed flag is at 0-based `out[4]`!**)
  - `[8..24]`: 16-byte raw `BabelTime` MD5 over `"BabelTime"` + `(body[i+1] ^ body[i])` + `(body[N-1] ^ (N & 0xFF))`
  - `[24..]`: Zlib stream (`compress2`) or raw AMF3
- **Server -> Client RX (8-Byte `CConnection` Header, `CConnection.h:216` + `Network.lua:104`)**:
  - `CConnection::headerSize_` defaults to **`8` bytes** (NO 16-byte MD5!).
  - `Network.lua:104` calls **`local zipped, multi = string.byte(head, 6, 7)`** (Lua **1-based** indices `6` and `7`!):
    - `head[0..4]` (Lua `1..4`): Big-Endian `u32` `bodyLen` (**MUST be `> 0`**; `bodyLen == 0` triggers `HEAD_SCRIPT_ERROR` -> immediate `close()` in `CConnection.cpp:528-533`!)
    - `head[4]` (Lua `5`): `flag0` (`0x00`)
    - **`head[5]` (Lua `6`)**: **`zipped`** (`!= 0` invokes `GameUtil:unzipData(body)`)
    - **`head[6]` (Lua `7`)**: **`multi`** (`!= 0` loops `for i = 1, #data.ret`, supporting both table items and nested AMF3 binary strings!)
    - `head[7]` (Lua `8`): `flag3` (`0x00`)
  - **Off-By-One Catastrophe**: Putting `zipped = 1` at `head[4]` (copying `LCPacket`) leaves `head[5] == 0`, feeding raw `0x78` zlib header bytes straight into `amf3.decode` (`unsupported AMF3 type marker 0x78`). Copying `{0, 1, 1, 1}` into an 8-byte header sets both `zipped = 1` and `multi = 1` erroneously!
  - **Callback Envelope**: `Network:sendMsg` starts `mId = 1` and increments first (`user.login_2`), while `sendStatMsg` sends `user.stat` without `mContext`. Every non-`"ping"` server frame **must** populate `ret.callback.callbackName` and a non-nil string `ret.err` (`Network.lua:146, 173`).

### 3. `BtPlatformWin32.cpp` + `AppDelegate.cpp` HTTP & Localhost Redirect Quirks

- **Empty Query Segments (`?&pid=...`, `&&`, `pid=`) & Raw Sorting**: `Platform::sortUrlParams` (`BtPlatformWin32.cpp:1391-1414`) ignores empty `&` segments (`amp == start`), retains `key=` empty-valued keys (`getHashWithSimulation` when `pid_` is empty), sorts **raw pre-decoded** `key=value` tokens ASCIIbetically with **no `&` separator**, appends `key` (`2A0ce68806G8aJ58Mo6_vnsg2@OmZHv`), and hashes with lowercase MD5.
- **Trailing Slash Variance**: `/phone/login?userid=...` has **no** trailing slash (`BtPlatformWin32.cpp:1185`), whereas `/phone/gethash/?pid=...`, `/phone/serverlist/?uuid=...`, and `/phone/loginrole/?uuid=...` **include** a trailing slash (`BtPlatformWin32.cpp:1650, 1740`). `HttpSdkGateway` normalizes both.
- **CURL Cookie Persistence**: `BtPlatformWin32.cpp:1491-1566` shares `PHPSESSID` via `CURLSH*` and persists `%GCLIENT_WRITABLE_DIR%\sdk_cookies.txt`.
- **Search-Root Override Trap**: `bt::Platform::loadConfig` reads `resourceRoot_` (`--base-assets-dir`) **before** environment variables and ignores `writable/` and `hotfix/` (`AppDelegate.cpp:384-388`, `BtPlatformWin32.cpp:726-733`). `client_launcher/connect_local_win32.bat` pairs `--resources-dir` (real assets) with `--base-assets-dir` + `--hotfix-dir` (plaintext JSON overrides) and `GCLIENT_ACCOUNT_PID`/`GCLIENT_ACCOUNT_TOKEN` to point `GCLIENT.exe` at `127.0.0.1` with zero C++ recompilation.

## Running the Offline Test & Fuzz Suite

```bash
cargo test --offline
cargo run --offline -- --fuzz-iters 5000
```
