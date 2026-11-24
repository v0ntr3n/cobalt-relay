@echo off
setlocal EnableExtensions EnableDelayedExpansion

REM ============================================================================
REM GServer (vnsg2) Win32 Local Private-Server Launcher (Zero C++ Recompile)
REM Audited directly against:
REM   - BtPlatformWin32.cpp (buildConfigSearchRoots, looksPlainJson, getHost,
REM                          getSdkCookieJarPath, GCLIENT_USE_LOCAL_STUB_AUTH)
REM   - AccountAuthProvider.cpp (GCLIENT_ACCOUNT_PID, GCLIENT_ACCOUNT_TOKEN, GCLIENT_SESSION_FILE)
REM   - AppDelegate.cpp     (--resources-dir, --base-assets-dir, --hotfix-dir,
REM                          --writable-dir, GCLIENT_WRITABLE_DIR)
REM ============================================================================

set "SCRIPT_DIR=%~dp0"
if "%SCRIPT_DIR:~-1%"=="\" set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"

if "%GCLIENT_HTTP_HOST%"=="" set "GCLIENT_HTTP_HOST=http://127.0.0.1:18080"
if "%GCLIENT_EXE_PATH%"==""  set "GCLIENT_EXE_PATH=%SCRIPT_DIR%\..\GCLIENT.exe"
if not exist "%GCLIENT_EXE_PATH%" (
    if exist "%CD%\GCLIENT.exe" set "GCLIENT_EXE_PATH=%CD%\GCLIENT.exe"
)

for %%F in ("%GCLIENT_EXE_PATH%") do set "CLIENT_ROOT=%%~dpF"
if "%CLIENT_ROOT:~-1%"=="\" set "CLIENT_ROOT=%CLIENT_ROOT:~0,-1%"

if "%GCLIENT_GAME_ASSETS%"=="" (
    if exist "%CLIENT_ROOT%\assets\scripts\main.lua" (
        set "GCLIENT_GAME_ASSETS=%CLIENT_ROOT%\assets"
    ) else if exist "%CLIENT_ROOT%\Resources\scripts\main.lua" (
        set "GCLIENT_GAME_ASSETS=%CLIENT_ROOT%\Resources"
    ) else (
        set "GCLIENT_GAME_ASSETS=%CLIENT_ROOT%\assets"
    )
)

set "OVERRIDE_ROOT=%SCRIPT_DIR%\platform_override"
set "LOCAL_PROFILE_ROOT=%LOCALAPPDATA%\GCLIENT\profiles\local_rust_qa"
set "LOCAL_WRITABLE_DIR=%LOCAL_PROFILE_ROOT%\writable"

if not exist "%OVERRIDE_ROOT%\platform" mkdir "%OVERRIDE_ROOT%\platform"
if not exist "%LOCAL_WRITABLE_DIR%" mkdir "%LOCAL_WRITABLE_DIR%"

REM 1. Write plaintext JSON overrides (BtPlatformWin32.cpp:798 looksPlainJson == true,
REM    so neither BTPRIKEY DES nor 0xFEFEFEFE XOR encryption is required!)
(
echo {
echo   "host": "%GCLIENT_HTTP_HOST%",
echo   "host_debug": "%GCLIENT_HTTP_HOST%",
echo   "log_host": "%GCLIENT_HTTP_HOST%",
echo   "log_host_debug": "%GCLIENT_HTTP_HOST%",
echo   "pushurl": "%GCLIENT_HTTP_HOST%/push",
echo   "gn": "vnsg2",
echo   "language": "vi_VN"
echo }
) > "%OVERRIDE_ROOT%\platform\game_config.json"

(
echo {
echo   "pl": "android",
echo   "os": "android",
echo   "gn": "vnsg2",
echo   "key": "2A0ce68806G8aJ58Mo6_vnsg2@OmZHv",
echo   "userCenterName": "Win32Local",
echo   "debug": "0",
echo   "isAd": "0",
echo   "isStandAlone": "0",
echo   "isSimulation": "0",
echo   "simulateHashHost": "%GCLIENT_HTTP_HOST%"
echo }
) > "%OVERRIDE_ROOT%\platform\config.json"

REM Optional --restore or --swap-in-place mode for builds without CLI path flags
if /I "%~1"=="--restore" (
    if exist "%GCLIENT_GAME_ASSETS%\platform\game_config.json.orig.bak" (
        move /Y "%GCLIENT_GAME_ASSETS%\platform\game_config.json.orig.bak" "%GCLIENT_GAME_ASSETS%\platform\game_config.json" >nul
    )
    if exist "%GCLIENT_GAME_ASSETS%\platform\config.json.orig.bak" (
        move /Y "%GCLIENT_GAME_ASSETS%\platform\config.json.orig.bak" "%GCLIENT_GAME_ASSETS%\platform\config.json" >nul
    )
    echo [connect_local_win32] Restored original Platform platform config files.
    exit /b 0
)

if /I "%~1"=="--swap-in-place" (
    if exist "%GCLIENT_GAME_ASSETS%\platform\game_config.json" (
        if not exist "%GCLIENT_GAME_ASSETS%\platform\game_config.json.orig.bak" (
            copy /Y "%GCLIENT_GAME_ASSETS%\platform\game_config.json" "%GCLIENT_GAME_ASSETS%\platform\game_config.json.orig.bak" >nul
        )
    )
    if exist "%GCLIENT_GAME_ASSETS%\platform\config.json" (
        if not exist "%GCLIENT_GAME_ASSETS%\platform\config.json.orig.bak" (
            copy /Y "%GCLIENT_GAME_ASSETS%\platform\config.json" "%GCLIENT_GAME_ASSETS%\platform\config.json.orig.bak" >nul
        )
    )
    copy /Y "%OVERRIDE_ROOT%\platform\game_config.json" "%GCLIENT_GAME_ASSETS%\platform\game_config.json" >nul
    copy /Y "%OVERRIDE_ROOT%\platform\config.json" "%GCLIENT_GAME_ASSETS%\platform\config.json" >nul
    echo [connect_local_win32] Installed in-place plaintext overrides under "%GCLIENT_GAME_ASSETS%\platform\".
)

REM 2. Isolate CURL cookie jar (sdk_cookies.txt) & session cache from live Platform
if exist "%LOCAL_WRITABLE_DIR%\sdk_cookies.txt" del /F /Q "%LOCAL_WRITABLE_DIR%\sdk_cookies.txt" >nul 2>&1
set "GCLIENT_PROFILE_DIR=%LOCAL_PROFILE_ROOT%"
set "GCLIENT_WRITABLE_DIR=%LOCAL_WRITABLE_DIR%"
set "GCLIENT_SESSION_FILE=%LOCAL_PROFILE_ROOT%\account_session.json"
set "GCLIENT_UUID_FILE=%LOCAL_PROFILE_ROOT%\device_uuid.txt"

REM 3. Inject pre-authenticated Platform credentials via AccountAuthProvider.cpp:947-963
REM    (bypasses interactive Win32 Sign-In dialog and exchanges cleanly with /phone/login)
if "%GCLIENT_ACCOUNT_PID%"==""   set "GCLIENT_ACCOUNT_PID=636df6782106606d"
if "%GCLIENT_ACCOUNT_TOKEN%"=="" set "GCLIENT_ACCOUNT_TOKEN=SID_local_dev_session_0001"
set "GCLIENT_INSECURE_TLS=1"

REM 4. Pin search-root env vars (BtPlatformWin32.cpp:727-729)
set "GCLIENT_BASE_ASSETS_DIR=%OVERRIDE_ROOT%"
set "GCLIENT_ASSETS_DIR=%OVERRIDE_ROOT%"
set "GCLIENT_RESOURCES_DIR=%GCLIENT_GAME_ASSETS%"

echo [connect_local_win32] Redirecting GCLIENT.exe -> %GCLIENT_HTTP_HOST%
echo   EXE            : %GCLIENT_EXE_PATH%
echo   Game Assets    : %GCLIENT_GAME_ASSETS%
echo   Platform Ovrd  : %OVERRIDE_ROOT%
echo   Isolated State : %LOCAL_PROFILE_ROOT%

if not exist "%GCLIENT_EXE_PATH%" (
    echo [WARN] GCLIENT.exe not found at "%GCLIENT_EXE_PATH%".
    echo        Set GCLIENT_EXE_PATH=C:\path\to\GCLIENT.exe and re-run.
    exit /b 1
)

REM 5. Launch GCLIENT.exe using the dual --resources-dir / --base-assets-dir / --hotfix-dir trick:
REM    - AppDelegate.cpp:385 sets bt::Platform::resourceRoot_ = --base-assets-dir (%OVERRIDE_ROOT%),
REM      so bt::Platform::loadConfig reads %OVERRIDE_ROOT%\platform\game_config.json FIRST!
REM    - AppDelegate.cpp:364-379 sets CCFileUtils searchPaths = [writable, hotfix (%OVERRIDE_ROOT%),
REM      resources (%GCLIENT_GAME_ASSETS%), base_assets (%OVERRIDE_ROOT%)], keeping all game assets intact!
start "" "%GCLIENT_EXE_PATH%" ^
  --writable-dir "%LOCAL_WRITABLE_DIR%" ^
  --hotfix-dir "%OVERRIDE_ROOT%" ^
  --resources-dir "%GCLIENT_GAME_ASSETS%" ^
  --base-assets-dir "%OVERRIDE_ROOT%"
