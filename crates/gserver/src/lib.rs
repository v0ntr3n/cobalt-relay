#![forbid(unsafe_code)]
//! # `gserver`
//!
//! Unified dual-listener private server daemon for **GServer (`vnsg2`)**, combining:
//! - **HTTP Platform / MAPI & Platform GTSDK Auth Server** (`gserver-http`)
//! - **24-Byte `LCPacket` / 8-Byte S2C AMF3 + Zlib TCP Game Server** (`gserver-protocol`)
//! - **28-Step `MainTown` 100% Bootstrap & 2,107 `NetConst` Catalog** (`gserver-rpc-catalog`)
//! - **Authentic `0xFEFEFEFE` `bt_decrypt` & Three Kingdoms Game Templates** (`gserver-data`)
//! - **Crash-Safe CRC32 WAL + Atomic JSON Account & Character Persistence** (`gserver-player`)
//! - **Multiplayer Town, Chat + GM Commands, Tavern Gacha, Shop, Mail & Battle** (`gserver-gameplay`)

pub mod bridge;
pub mod bxh_snapshot;
pub mod config;
pub mod event_pay_rpcs;
pub mod player_rpcs;
pub mod tcp_server;
pub mod world;

use std::net::SocketAddr;
use std::sync::Arc;

use gserver_http::{
    spawn_http_server, GameServerEntry, HttpServerConfig, HttpServerHandle, MapiService,
    ServerConfig as HttpMapiConfig,
};

pub use config::DaemonConfig;
pub use tcp_server::{
    spawn_tcp_game_server, TcpConnectionRecord, TcpGameServerHandle, TcpServerTelemetry,
    TcpShutdownReason,
};
pub use world::{ConnectionSession, HandledRpc, ServerWorld};

/// Handle owning both the running HTTP MAPI server and TCP AMF3 game server.
pub struct RunningServer {
    pub http_addr: SocketAddr,
    pub tcp_addr: SocketAddr,
    pub world: Arc<ServerWorld>,
    http_handle: HttpServerHandle,
    tcp_handle: TcpGameServerHandle,
}

impl RunningServer {
    /// Starts both the TCP AMF3 game server and HTTP MAPI server, automatically wiring the
    /// resolved TCP port into `/phone/serverlist/` and `/phone/gethash/` responses.
    pub fn start(config: DaemonConfig) -> Result<Self, String> {
        let mapi_cfg = HttpMapiConfig {
            strict_session: config.strict_session,
            strict_sign: config.strict_sign,
            strict_auth_sig: config.strict_auth_sig,
            strict_auth_token: config.strict_auth_token,
            ..HttpMapiConfig::default()
        };

        let mapi = Arc::new(MapiService::new(mapi_cfg));
        let world = ServerWorld::open(config.clone(), Arc::clone(&mapi))?;

        // Bind TCP Game Server first so ephemeral `:0` ports resolve before configuring `/phone/serverlist`
        let tcp_handle =
            spawn_tcp_game_server(&config.tcp_bind, Arc::clone(&world)).map_err(|e| {
                format!(
                    "failed to bind TCP game server on '{}': {e}",
                    config.tcp_bind
                )
            })?;
        let tcp_addr = tcp_handle.local_addr();

        let s161 = GameServerEntry::default_s161(config.public_host.clone(), tcp_addr.port());
        let s162 = GameServerEntry::default_s162(config.public_host.clone(), tcp_addr.port());
        mapi.upsert_server(s161);
        mapi.upsert_server(s162);

        let http_handle = spawn_http_server(
            &config.http_bind,
            Arc::clone(&mapi),
            HttpServerConfig::default(),
        )
        .map_err(|e| {
            format!(
                "failed to bind HTTP MAPI server on '{}': {e}",
                config.http_bind
            )
        })?;
        let http_addr = http_handle.local_addr();

        Ok(Self {
            http_addr,
            tcp_addr,
            world,
            http_handle,
            tcp_handle,
        })
    }

    pub fn tcp_telemetry(&self) -> &Arc<TcpServerTelemetry> {
        self.tcp_handle.telemetry()
    }

    pub fn shutdown(mut self) {
        self.tcp_handle.stop();
        self.http_handle.shutdown();
    }
}
