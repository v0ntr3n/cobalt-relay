# `gserver-protocol`

100% pure Safe Rust (`#![forbid(unsafe_code)]`, `std`-only, **zero external dependencies**) AMF3, RFC 1950/1951 Zlib, BabelTime `LCPacket` MD5, and TCP packet framing crate reverse-engineered from the GServer (`vnsg2`) C++ (`Amf3Module.cpp`, `LCPacket.cpp`, `CConnection.cpp`, `GameUtil.cpp`, `BtDecryptHook.cpp`) and Lua (`Network.lua`, `SuperMapNetwork.lua`, `LoginProxy.lua`, `NetConst.lua`) client.

## Verified GServer (`vnsg2`) Wire Specification

### 1. Client-to-Server (C2S) Wire Frame (`Network.lua` -> `CConnection::send(data, true)` -> `LCPacket::makeLCPacket`)

In `Network.lua` (`sendMsg`, `sendUnusedMsg`, `sendStatMsg`, `sendNewStatMsg`) and `SuperMapNetwork.lua`, every outgoing AMF3 request is sent via `self.mConn:send(data, true)`, which invokes `LCPacket::makeLCPacket` (`0x2b4b04` in `libcocos2dlua.so`) to emit a **24-byte `LCPacket` header**:

| Byte Offset | Size | Endian | Field | Description |
| :--- | :--- | :--- | :--- | :--- |
| `0..4` | 4 B | Big-Endian | `bodyLen` | `u32` byte length of `body` following the 24-byte header (`> 0`). |
| `4..8` | 4 B | Raw | `flags` | `[1, 1, 1, 1]` when `body` is RFC 1950 Zlib-compressed; `[0, 1, 1, 1]` when uncompressed. |
| `8..24` | 16 B | Raw | `md5` | Raw 16-byte MD5 over `"BabelTime"` (9 B) + XOR chain (`body[i] ^ body[i+1]` for `i < N-1`, `body[N-1] ^ (N & 0xFF)`). |
| `24..24+bodyLen` | `bodyLen` | — | `body` | RFC 1950 Zlib stream (when `flags[0] == 1`) or raw AMF3 payload (`flags[0] == 0`). |

*Note*: When `CConnection::send(data, false)` is called directly in test harnesses, the frame is `[4B BE bodyLen][raw AMF3 payload]`. `ClientRxDecoder::new(C2sFrameMode::Auto)` seamlessly handles both `LcPacket24` and `Raw4Byte` frames on a streaming TCP buffer.

### 2. Client-to-Server AMF3 Request Envelope (`Network:sendMsg`)

`Amf3Module.cpp` encodes pure string-keyed Lua tables (`denseCount == 0 && hasNonDenseKeys`) as anonymous dynamic AMF3 Objects (`0x0A`, traits `0x0B`, className `""` = `0x01`, terminated by `0x01`):

```lua
{
    method   = "user.login",                  -- AMF3 String (0x06)
    args     = { loginHash, uuid, "1.0.1194" }, -- AMF3 dense Array (0x09); empty {} => [0x09, 0x01, 0x01]
    token    = "0",                           -- AMF3 String (starts at "0" in Network:ctor)
    callback = {                              -- AMF3 anonymous dynamic Object (0x0A)
        callbackName = "user.login_2"         -- "<method>_<mId>" (mId starts at 1, pre-incremented)
    }
}
```

Fire-and-forget stat methods (`sendStatMsg`, `sendNewStatMsg`) set `method = callback.callbackName = "user.stat"` or `"user.guideEvent"`.

### 3. Server-to-Client (S2C) 8-Byte Wire Frame (`CConnection` + `Network:onBodyData`)

`CConnection` defaults to `headerSize_ = 8` (`[0..4]` big-endian `u32` `bodyLen`). `Network:onBodyData(body, head)` reads 1-based Lua indices `local zipped, multi = string.byte(head, 6, 7)`:

| Byte Offset (0-based) | Lua `string.byte` (1-based) | Field | Description |
| :--- | :--- | :--- | :--- |
| `0..4` | `1..4` | `bodyLen` | Big-endian `u32` byte length of `payload` after the 8-byte header (`1..=64 MiB`). |
| `4` | `5` | `flag0` | Reserved (`0x00`). |
| `5` | `6` | `zipped` | `0` = raw AMF3 payload; non-zero (`1`) = `GameUtil:unzipData(body)` (RFC 1950 Zlib). |
| `6` | `7` | `multi` | `0` = single envelope; non-zero (`1`) = multi-response batch. |
| `7` | `8` | `flag3` | Reserved (`0x00`). |
| `8..8+bodyLen` | — | `payload` | S2C body bytes. |

### 4. Server-to-Client Envelopes (`multi == 0`, `multi == 1`, Push `re.*`, and Ping)

- **Single Envelope (`multi == 0`)**: `data = amf3.decode(payload)` produces an AMF3 Object:
  - `err`: `"ok"` on success, `"ping"` for server heartbeat (ignored before `callback` lookup), or an error string (triggers error alert and clears one-shot callback).
  - `callback`: `{ callbackName = "<method>_<mId>" }` for RPC replies, `"dummy"` for no-op token/time carrier frames, or `"re.<module>.<action>"` / `"user.updateUser"` for persistent push notifications registered via `Network:regPushCallback`.
  - `ret`: RPC return value or push payload (`Value`).
  - `token` *(optional)*: Updates `Network.mToken`.
  - `time` *(optional)*: Server Unix timestamp in seconds; updates `g.timeManager:setOffset(data.time - os.time())`.
- **Multi-Response Batch (`multi != 0`)**:
  - **Important verified detail (`Network.lua:113-122`)**: Even when `multi != 0`, `Network.lua` calls `data = amf3.decode(data)` on the outer body first, reads `data.token` / `data.time` from the outer object, and loops `for i = 1, #data.ret`: if `type(data.ret[i]) == "table"` it uses `data.ret[i]` directly (`MultiItemEncoding::InlineObject`), otherwise it runs `amf3.decode(data.ret[i])` on the nested serialized AMF3 bytes (`MultiItemEncoding::NestedAmf3ByteString` / `NestedAmf3ByteArray`). Sending raw `[4B BE subLen]` at offset 0 of `body` to the unmodified Lua client would cause `amf3.decode` to read leading `0x00` as AMF3 `Undefined` (`nil`) and error on `#data.ret`. Both the true `Network.lua` outer-AMF3 batch format and `encode_multi_length_prefixed` (`[4B BE subLen][AMF3]`) are supported by `gserver-protocol`.

## Offline Build & Test

```bash
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```
