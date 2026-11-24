#![forbid(unsafe_code)]

use std::env;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use gserver_qa_harness::fuzzer::{run_deterministic_fuzz_campaign, DEFAULT_FUZZ_SEED};
use gserver_qa_harness::http_gateway::{HttpGatewayConfig, HttpSdkGateway};
use gserver_qa_harness::server::{GameServerConfig, GameTcpServer};

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut serve_mode = false;
    let mut http_addr = "127.0.0.1:18080".to_string();
    let mut tcp_addr = "127.0.0.1:18160".to_string();
    let mut fuzz_iters: usize = 2_500;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--serve" => serve_mode = true,
            "--http-addr" if i + 1 < args.len() => {
                http_addr = args[i + 1].clone();
                i += 1;
            }
            "--tcp-addr" if i + 1 < args.len() => {
                tcp_addr = args[i + 1].clone();
                i += 1;
            }
            "--fuzz-iters" if i + 1 < args.len() => {
                if let Ok(n) = args[i + 1].parse::<usize>() {
                    fuzz_iters = n;
                }
                i += 1;
            }
            "--help" | "-h" => {
                println!(
                    "gserver-qa-harness — GServer (vnsg2) Pure-Rust Adversarial QA & Local Server\n\
                     Usage:\n\
                       gserver-qa-harness [--fuzz-iters N]        Run deterministic PRNG fuzz self-test\n\
                       gserver-qa-harness --serve [--http-addr 127.0.0.1:18080] [--tcp-addr 127.0.0.1:18160]\n\
                                                                Run local SDK HTTP + Game TCP server for GCLIENT.exe"
                );
                return;
            }
            _ => {}
        }
        i += 1;
    }

    if serve_mode {
        let game_server = GameTcpServer::start(&tcp_addr, GameServerConfig::default())
            .expect("failed to bind GameTcpServer");
        let bound_tcp = game_server.local_addr();

        let http_cfg = HttpGatewayConfig {
            game_tcp_host: bound_tcp.ip().to_string(),
            game_tcp_port: bound_tcp.port(),
            ..HttpGatewayConfig::default()
        };
        let http_gateway = HttpSdkGateway::start(&http_addr, http_cfg)
            .expect("failed to bind HttpSdkGateway");

        println!(
            "[gserver-qa-harness] Local GServer (vnsg2) Private Server Gateway ONLINE\n\
             - SDK HTTP Gateway : http://{}\n\
             - Game TCP Server  : {}\n\
             Launch GCLIENT.exe via client_launcher\\connect_local_win32.bat (Ctrl+C to stop).",
            http_gateway.local_addr(),
            bound_tcp
        );

        loop {
            thread::sleep(Duration::from_secs(5));
            let tel = game_server.telemetry();
            println!(
                "[telemetry] http_reqs={} tcp_accepted={} active={} rpcs={} rejected_dos={}",
                http_gateway.metrics().total_requests.load(Ordering::Relaxed),
                tel.accepted_connections.load(Ordering::Relaxed),
                tel.active_connections.load(Ordering::Relaxed),
                tel.valid_rpcs_handled.load(Ordering::Relaxed),
                tel.rejected_oversized_frames.load(Ordering::Relaxed),
            );
        }
    } else {
        println!(
            "[gserver-qa-harness] Running deterministic PRNG fuzz campaign (seed=0x{:016X}, iterations={})...",
            DEFAULT_FUZZ_SEED, fuzz_iters
        );
        let report = run_deterministic_fuzz_campaign(DEFAULT_FUZZ_SEED, fuzz_iters);
        println!(
            "[gserver-qa-harness] PASS: roundtrips={} amf3_mut={} zlib_mut={} frame_mut={} http_mut={}",
            report.structured_roundtrips,
            report.mutated_amf3_cases,
            report.mutated_zlib_cases,
            report.mutated_frame_cases,
            report.mutated_http_cases
        );
    }
}
