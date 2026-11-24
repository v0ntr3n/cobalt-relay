#![forbid(unsafe_code)]
//! CLI entry point for the unified GServer (`vnsg2`) private server daemon (`gserver`).

use std::env;
use std::process;
use std::thread;
use std::time::Duration;

use gserver::{DaemonConfig, RunningServer};

fn print_help() {
    println!(
        "gserver — Pure-Rust Private Server for GServer (vnsg2)\n\n\
         USAGE:\n\
           gserver [OPTIONS]\n\n\
         OPTIONS:\n\
           --config <FILE>          Load TOML configuration file (default: ./server.toml if present)\n\
           --http-addr <HOST:PORT>  Bind HTTP Platform/MAPI & Platform Auth listener (default: 127.0.0.1:8080)\n\
           --tcp-addr <HOST:PORT>   Bind TCP 24B-RX / 8B-TX AMF3 Game Server (default: 127.0.0.1:8160)\n\
           --public-host <HOST>     Host advertised in /phone/serverlist & /phone/gethash (default: 127.0.0.1)\n\
           --data-dir <DIR>         Persistent CRC32 WAL + player JSON directory (default: ./data)\n\
           --db-export-dir <DIR>    Optional decrypted client DB JSON export directory\n\
           --no-strict-session      Disable PHPSESSID/GSESS_ID cookie enforcement on /phone/gethash\n\
           --no-verify-tickets      Allow TCP user.login without checking /phone/gethash ticket\n\
           -h, --help               Print this help message"
    );
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cfg = match DaemonConfig::from_cli_args(&args) {
        Ok(Some(c)) => c,
        Ok(None) => {
            print_help();
            return;
        }
        Err(err) => {
            eprintln!("error: {err}\n");
            print_help();
            process::exit(2);
        }
    };

    let server = match RunningServer::start(cfg.clone()) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("fatal: {err}");
            process::exit(1);
        }
    };

    println!("==================================================================");
    println!("  GServer (vnsg2) Pure-Rust Private Server — ONLINE");
    println!("==================================================================");
    println!("  HTTP MAPI / Platform Auth : http://{}", server.http_addr);
    println!("  TCP AMF3 Game Server : {} (24B LCPacket RX / 8B S2C TX)", server.tcp_addr);
    println!("  Advertised Host      : {}", cfg.public_host);
    println!("  Persistent Data Dir  : {}", cfg.data_dir.display());
    println!("  Win32 Client Env     : set GCLIENT_AUTH_URL=http://{}", server.http_addr);
    println!("  In-Game GM Commands  : /help /gold /silver /vip /level /item /hero /mail /heal");
    println!("==================================================================");

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
