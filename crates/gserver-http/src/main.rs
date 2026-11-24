#![forbid(unsafe_code)]

use std::env;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use gserver_http::{start_mapi_server, GameServerEntry, ServerConfig};

fn print_usage() {
    eprintln!(
        "gserver-http 0.1.0 - Zero-dependency GServer (vnsg2) MAPI & Platform Auth HTTP Server\n\
         \n\
         USAGE:\n\
           gserver-http [OPTIONS]\n\
         \n\
         OPTIONS:\n\
           --bind <HOST:PORT>       Bind address (default: 127.0.0.1:8080)\n\
           --game-host <IP>         Game TCP host advertised in serverlist/gethash (default: 127.0.0.1)\n\
           --game-port <PORT>       Primary S161 game TCP port (default: 8160)\n\
           --app-key <KEY>          Additional MAPI MD5 signing key\n\
           --strict-sign            Require valid MD5 sign=... on /phone/* endpoints\n\
           --permissive-session     Disable strict PHPSESSID/GSESS_ID check on /phone/gethash/\n\
           --help, -h               Print this help message"
    );
}

fn main() {
    let mut bind_addr = "127.0.0.1:8080".to_string();
    let mut game_host = "127.0.0.1".to_string();
    let mut game_port: u16 = 8160;
    let mut config = ServerConfig::default();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                bind_addr = args.next().unwrap_or_else(|| {
                    eprintln!("Error: --bind requires <HOST:PORT>");
                    process::exit(2);
                });
            }
            "--game-host" => {
                game_host = args.next().unwrap_or_else(|| {
                    eprintln!("Error: --game-host requires <IP>");
                    process::exit(2);
                });
            }
            "--game-port" => {
                let raw = args.next().unwrap_or_else(|| {
                    eprintln!("Error: --game-port requires <PORT>");
                    process::exit(2);
                });
                game_port = raw.parse::<u16>().unwrap_or_else(|_| {
                    eprintln!("Error: invalid port '{}'", raw);
                    process::exit(2);
                });
            }
            "--app-key" => {
                let key = args.next().unwrap_or_else(|| {
                    eprintln!("Error: --app-key requires <KEY>");
                    process::exit(2);
                });
                config.accepted_app_keys.push(key);
            }
            "--strict-sign" => {
                config.strict_sign = true;
            }
            "--permissive-session" => {
                config.strict_session = false;
            }
            "--help" | "-h" => {
                print_usage();
                return;
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                print_usage();
                process::exit(2);
            }
        }
    }

    config.servers = vec![
        GameServerEntry::default_s161(&game_host, game_port),
        GameServerEntry::default_s162(&game_host, game_port.saturating_add(1)),
    ];

    let (_service, handle) = match start_mapi_server(&bind_addr, config) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Failed to bind gserver-http on {}: {}", bind_addr, e);
            process::exit(1);
        }
    };

    println!(
        "[gserver-http] Listening on {} (game TCP target {}:{})",
        handle.base_url(),
        game_host,
        game_port
    );

    let keep_running = Arc::new(AtomicBool::new(true));
    while keep_running.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_secs(3600));
    }
}
