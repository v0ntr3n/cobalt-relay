# Pointing `GCLIENT.exe` (Win32) at `127.0.0.1` Without Recompiling C++

This guide documents the exact file-resolution, decryption, cookie-jar, and authentication mechanics verified directly from `AppDelegate.cpp`, `BtPlatformWin32.cpp`, and `AccountAuthProvider.cpp`.

## 1. Why Putting `game_config.json` in `writable/` or `hotfix/` Alone Fails!

In `AppDelegate::configureFileUtils()` (`AppDelegate.cpp:354-388`):
1. `CCFileUtils::setSearchPaths` registers layers in the order:
   `writableDir/Resources` -> `hotfixDir` -> `resourcesDir` -> `baseAssetsDir`.
2. **However**, `bt::Platform::getInstance()->loadConfig(...)` does **NOT** read through `CCFileUtils`! Instead, `AppDelegate.cpp:384-388` configures:
   ```cpp
   if (!m_config.baseAssetsDir.empty()) {
       platform->setResourceRoot(normPath(m_config.baseAssetsDir));
   } else if (!m_config.resourcesDir.empty()) {
       platform->setResourceRoot(normPath(m_config.resourcesDir));
   }
   ```
   And `Platform::buildConfigSearchRoots()` (`BtPlatformWin32.cpp:726-733`) searches:
   1. `resourceRoot_` (`--base-assets-dir`, or `--resources-dir` if `--base-assets-dir` is empty)
   2. `GCLIENT_BASE_ASSETS_DIR`
   3. `GCLIENT_ASSETS_DIR`
   4. `GCLIENT_RESOURCES_DIR`
   5. `"base_assets/"`, `"assets/"`, `"Resources/"`, `""`

If you only place an override `platform/game_config.json` inside `writable/Resources/` or `hotfix/`, `bt::Platform::loadConfig` still hits `assets/platform/game_config.json` first via `resourceRoot_` and connects to live Platform (`http://127.0.0.1:8080`)!

### Zero-Touch Overlay Solution (`connect_local_win32.bat`)
Pass **both**:
- `--resources-dir "<path-to-real-assets>"` (all Lua scripts, SQLite DBs, and textures resolve from here)
- `--base-assets-dir "<path-to-platform_override>"` (sets `bt::Platform::resourceRoot_` to `platform_override/`, so `bt::Platform` loads `platform_override/platform/game_config.json` and `config.json` **first** without modifying a single original game file!)
- `--hotfix-dir "<path-to-platform_override>"` (ensures any Lua script reading `platform/*.json` via `CCFileUtils` also sees the override first)

*(If running a binary wrapper without CLI directory flags, run `connect_local_win32.bat --swap-in-place` to back up `assets\platform\*.json` to `*.orig.bak` and install the plaintext overrides in place, and `connect_local_win32.bat --restore` to revert.)*

## 2. Plaintext JSON Bypass (`looksPlainJson` — Zero `BTPRIKEY` DES Required)

`Platform::loadConfig` (`BtPlatformWin32.cpp:792-803`) inspects the first non-whitespace byte of `platform/config.json` and `platform/game_config.json` (after optional `0xFEFEFEFE` `BtDecryptHook` stripping in `readBinaryFile`):
- If the first non-whitespace byte is `{` or `[`, `looksPlainJson` is `true` and `MiniJsonParser` parses the plaintext UTF-8 JSON directly — skipping `"BTPRIKEY"` `F0F0F0F0` DES decryption completely.
- Always set **both** `"host"` and `"host_debug"` to `"http://127.0.0.1:18080"` in `platform/game_config.json`, because `Platform::getHost()` (`BtPlatformWin32.cpp:1343-1357`) switches between `"host"` and `"host_debug"` based on `isDebugEnv_`.

## 3. Bypassing the Win32 Account Popup & Isolating `CURL` Cookies

1. **Pre-seeded Auth Environment Variables (`AccountAuthProvider.cpp:947-963`)**:
   - Setting `GCLIENT_ACCOUNT_PID=636df6782106606d` and `GCLIENT_ACCOUNT_TOKEN=SID_local_dev_session_0001` causes `AccountAuthProvider::login()` to short-circuit before opening the Win32 `GServer - Platform Sign In` dialog and immediately run `bt::Platform`'s `/phone/login` exchange against `http://127.0.0.1:18080/phone/login`.
   - Alternatively, setting `GCLIENT_USE_LOCAL_STUB_AUTH=1` (`BtPlatformWin32.cpp:540`) or `GCLIENT_ACCOUNT_TOKEN=win32_guest_token` (`AccountAuthProvider.cpp:712`, `BtPlatformWin32.cpp:1160`) triggers offline-guest mode, skipping `/phone/login` HTTP exchange entirely while still populating `pid_` so `/phone/gethash/` runs!
2. **`CURL` Cookie Jar Isolation (`BtPlatformWin32.cpp:1491-1566`)**:
   - `performSharedHttpGet` shares cookies in-memory via `CURLSH*` (`CURL_LOCK_DATA_COOKIE`) and persists them to `%GCLIENT_WRITABLE_DIR%\sdk_cookies.txt`.
   - `connect_local_win32.bat` isolates `GCLIENT_WRITABLE_DIR` and `GCLIENT_SESSION_FILE` under `%LOCALAPPDATA%\GCLIENT\profiles\local_rust_qa\` and clears stale `sdk_cookies.txt` on launch so live Platform `PHPSESSID` cookies never leak into local sessions (or vice versa).

## 4. End-to-End Quickstart

1. Start the pure-Rust local HTTP SDK + TCP Game server:
   ```bat
   cargo run --offline --release -- --serve --http-addr 127.0.0.1:18080 --tcp-addr 127.0.0.1:18160
   ```
2. Double-click or run `client_launcher\connect_local_win32.bat` (set `GCLIENT_EXE_PATH` if `GCLIENT.exe` is in another folder).
3. `GCLIENT.exe` boots -> calls `http://127.0.0.1:18080/phone/serverlist/`, `/phone/login`, `/phone/loginrole/`, and `/phone/gethash/` -> receives `"host": "127.0.0.1", "port": 18160` -> connects `CConnection` over TCP directly to `GameTcpServer`!
