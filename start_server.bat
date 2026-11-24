@echo off
setlocal
cd /d "%~dp0"
cargo run --release -p gserver -- --config server.toml %*
endlocal
