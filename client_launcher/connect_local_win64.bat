@echo off
setlocal EnableExtensions
REM Thin cross-repo delegator to canonical kestrel-canvas\client_launcher\connect_local_win64.bat
for %%D in ("%~dp0..") do set "RELAY_ROOT=%%~fD"
if "%COBALT_RELAY_DIR%"=="" set "COBALT_RELAY_DIR=%RELAY_ROOT%"
set "TARGET_BAT="
if not "%KESTREL_CANVAS_DIR%"=="" (
    if exist "%KESTREL_CANVAS_DIR%\client_launcher\connect_local_win64.bat" (
        set "TARGET_BAT=%KESTREL_CANVAS_DIR%\client_launcher\connect_local_win64.bat"
    )
)
if "%TARGET_BAT%"=="" (
    if exist "%RELAY_ROOT%\..\kestrel-canvas\client_launcher\connect_local_win64.bat" (
        set "TARGET_BAT=%RELAY_ROOT%\..\kestrel-canvas\client_launcher\connect_local_win64.bat"
    )
)
if "%TARGET_BAT%"=="" (
    echo [ERROR] Canonical connect_local_win64.bat not found. Set KESTREL_CANVAS_DIR and re-run.
    exit /b 1
)
call "%TARGET_BAT%" %*
exit /b %ERRORLEVEL%
