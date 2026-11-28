#![forbid(unsafe_code)]
//! # `gserver-http`
//!
//! Pure-Rust, 100% Safe Rust (`#![forbid(unsafe_code)]`, `std` + `gserver-protocol`) HTTP Platform
//! / MAPI and Platform/Win32 Authentication SDK server for **GServer (`vnsg2`)**.
//!
//! ## Modules
//! - [`md5`], [`url`], [`json`]: Re-exported from [`gserver_protocol`] (RFC 1321 MD5, HMAC-MD5,
//!   `BtPlatformWin32::sortUrlParams` + MAPI/GTSDK signatures, RFC 3986 URL/Cookie codecs, and
//!   RFC 8259 JSON parser/serializer).
//! - [`http`]: Multi-threaded `std::net::TcpListener` HTTP/1.1 server supporting `GET`, `POST`,
//!   `HEAD`, `OPTIONS`, `Cookie` / `Set-Cookie`, `Keep-Alive`, `Transfer-Encoding: chunked`, and
//!   clean shutdown.
//! - [`protocol`]: Data structures and exact JSON response builders for GServer (`vnsg2`) MAPI.
//! - [`router`]: Thread-safe [`router::MapiService`] and [`router::SignedMapiEndpoint`] dispatcher.

pub mod http;
pub mod protocol;
pub mod router;

pub use gserver_protocol::{json, md5, url};

use std::io;
use std::net::ToSocketAddrs;
use std::sync::Arc;

pub use http::{
    spawn_http_server, HttpHandler, HttpMethod, HttpRequest, HttpResponse, HttpServerConfig,
    HttpServerHandle,
};
pub use json::{parse_json, JsonNumber, JsonObject, JsonParseError, JsonValue};
pub use md5::{
    compute_account_auth_sig, compute_guest_auth_sig, compute_mapi_signature, hmac_md5,
    hmac_md5_hex, md5, md5_hex, sign_mapi_url, sort_url_params, verify_mapi_query_signature,
    DEFAULT_AUTH_SECRET, DEFAULT_FALLBACK_SIGN_KEY, DEFAULT_GTSDK_GAME_ID, VNSG2_MAPI_APP_KEY,
};
pub use protocol::{
    AccountRecord, GameServerEntry, HttpCookieSession, IssuedGameHash, RoleSummary,
    ScriptPatchEntry, ServerConfig, ServerStatus, VersionPolicy, COOKIE_GAME_SESSION,
    COOKIE_PHPSESSID,
};
pub use router::{MapiService, SignedMapiEndpoint};

/// Convenience launcher that constructs an `Arc<MapiService>` and binds an HTTP/1.1 listener.
pub fn start_mapi_server<A: ToSocketAddrs>(
    addr: A,
    config: ServerConfig,
) -> io::Result<(Arc<MapiService>, HttpServerHandle)> {
    let service = Arc::new(MapiService::new(config));
    let handle = spawn_http_server(addr, Arc::clone(&service), HttpServerConfig::default())?;
    Ok((service, handle))
}
