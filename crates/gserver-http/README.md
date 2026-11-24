# `gserver-http` — GServer (`vnsg2`) Zero-Dependency Pure-Rust HTTP MAPI & Platform Auth Server

`gserver-http` is a 100% Safe Rust (`#![forbid(unsafe_code)]`) HTTP/1.1 Platform / MAPI & Platform Authentication SDK server crate for **GServer (`vnsg2`)**, built using **only the Rust standard library (`std`)** with zero external `crates.io` dependencies.

## Features

- **Pure-Rust RFC 1321 MD5 & RFC 2104 HMAC-MD5 (`src/md5.rs`)**:
  - Exact port of `bt::Platform::sortUrlParams` + `appKey` URL signing (`BtPlatformWin32.cpp:1391-1432`), verified against all 6 captured live requests in `live_capture_filtered.log` using the production `vnsg2` key `2A0ce68806G8aJ58Mo6_vnsg2@OmZHv` (plus fallback SDK key `9c98948d313ccdAE962e899a60692c_russia`).
  - Exact port of Platform GTSDK Account (`md5(ghk + gameID + u + passMd5 + ts)`) and Guest (`md5(ghk + gameID + deviceID + ts)`) signing (`AccountAuthProvider.cpp:371, 424`).
- **Pure-Rust URL Codec & Cookie Parser (`src/url.rs`)**:
  - RFC 3986 percent-encoding/decoding, `application/x-www-form-urlencoded` decoding, path normalization (`/phone/serverlist` == `/phone/serverlist/`), and `Cookie` header parsing.
- **Complete Pure-Rust RFC 8259 JSON Parser & Serializer (`src/json.rs`)**:
  - Preserves exact integer (`i64`/`u64`) vs floating-point (`f64`) types (`"AMF": 3`, `"port": 8160`, `"timestamp": 1795500450`) and full `\uXXXX` + UTF-16 surrogate pair decoding.
- **Multi-Threaded `std::net::TcpListener` HTTP/1.1 Server (`src/http.rs`)**:
  - Persistent `Keep-Alive` connections, `GET`/`POST`/`HEAD`/`OPTIONS`, `Content-Length` & `Transfer-Encoding: chunked`, `Cookie`/`Set-Cookie` (`PHPSESSID` & `GSESS_ID`), and graceful shutdown.
- **Complete GServer (`vnsg2`) MAPI & Auth Endpoint Coverage (`src/router.rs`, `src/protocol.rs`)**:
  - `/phone/applereview/`, `/phone/getversion/`, `/version.lua`, `/phone/serverlist/`, `/phone/notice/`, `/phone/supernotice/`, `/phone/login`, `/phone/loginrole/`, `/phone/gethash/`, `/api/login/auth/account`, `/api/login/createUser/guest`, `/api/login/register`, `/api/login/login`, auxiliary SDK routes, `/admin/status`, and `/admin/verify_hash`.

## Quick Start

```bash
cargo test --offline
cargo run --offline -- --bind 127.0.0.1:8080 --game-host 127.0.0.1 --game-port 8160
```

## Reverse-Engineered Protocol Summary

### 1. MAPI URL Signing (`BtPlatformWin32.cpp`)
1. Append `&time=<unix_seconds>` to the request query string.
2. Split raw query string on `&`, discard empty segments (and `sign=...` when verifying), sort `key=rawEncodedValue` tokens in **ASCII lexicographical order**, and concatenate **without `&`**.
3. Append `appKey` (`2A0ce68806G8aJ58Mo6_vnsg2@OmZHv` for `vnsg2`).
4. Compute lowercase 32-hex RFC 1321 MD5 digest and append `&sign=<md5>`.

### 2. Cookie Session Binding (`PHPSESSID` / `GSESS_ID`)
- `GET /phone/login` exchanges `(userid, sessionid)` for a 16-hex-char `uid`/`pid` and emits `Set-Cookie: PHPSESSID=<token>; Path=/; HttpOnly` and `Set-Cookie: GSESS_ID=<token>; Path=/; HttpOnly`.
- `GET /phone/gethash/` validates that `PHPSESSID` or `GSESS_ID` is unexpired and bound to the requested `pid`. When `strict_session = true` and the cookie is missing, expired, or bound to a different `pid`, the server returns HTTP 200 with:
  ```json
  {"errornu":"102","err_code":102,"errordesc":"session time out","err_desc":"session time out"}
  ```
  Setting both `"errornu": "102"` (for `SDKUtil.lua` `ERROR_PASS["102"] = true`) and `"err_code": 102` (for `LoginProxy.lua` `if jsonInfo.err_code == 102`) cleanly resets `g.pid = nil` without triggering a fatal SDK popup.
