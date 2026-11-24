#![forbid(unsafe_code)]
//! # `gserver-http`
//!
//! Zero-dependency, 100% Safe Rust (`#![forbid(unsafe_code)]`, `std` only) HTTP Platform / MAPI
//! and Platform/Win32 Authentication SDK server for **GServer (`vnsg2`)**.
//!
//! ## Modules
//! - [`md5`]: Pure-Rust RFC 1321 MD5, RFC 2104 HMAC-MD5, and exact `BtPlatformWin32::sortUrlParams`
//!   + `appKey` (`2A0ce68806G8aJ58Mo6_vnsg2@OmZHv`) & Platform GTSDK URL signature helpers.
//! - [`url`]: RFC 3986 percent-encoder/decoder, `application/x-www-form-urlencoded` parser,
//!   request-target normalizer, and `Cookie` header parser.
//! - [`json`]: Complete RFC 8259 JSON parser and serializer preserving integer vs floating-point
//!   types and object key order.
//! - [`http`]: Multi-threaded `std::net::TcpListener` HTTP/1.1 server supporting `GET`, `POST`,
//!   `HEAD`, `OPTIONS`, `Cookie` / `Set-Cookie`, `Keep-Alive`, `Transfer-Encoding: chunked`, and
//!   clean shutdown.
//! - [`protocol`]: Data structures and exact JSON response builders for GServer (`vnsg2`) MAPI.
//! - [`router`]: Thread-safe [`router::MapiService`] implementing `/phone/serverlist`, `/phone/notice`,
//!   `/phone/supernotice`, `/phone/applereview`, `/phone/getversion`, `/phone/login`,
//!   `/phone/loginrole`, `/phone/gethash`, Platform Account / Win32 Guest / custom registration,
//!   and `/admin/status`.

pub mod http;
pub mod json;
pub mod md5;
pub mod protocol;
pub mod router;
pub mod url;

use std::io;
use std::net::ToSocketAddrs;
use std::sync::Arc;

pub use http::{spawn_http_server, HttpHandler, HttpMethod, HttpRequest, HttpResponse, HttpServerConfig, HttpServerHandle};
pub use json::{parse_json, JsonNumber, JsonObject, JsonParseError, JsonValue};
pub use md5::{
    compute_mapi_signature, compute_guest_auth_sig, compute_account_auth_sig, hmac_md5, hmac_md5_hex,
    md5, md5_hex, sign_mapi_url, sort_url_params, verify_mapi_query_signature,
    DEFAULT_FALLBACK_SIGN_KEY, DEFAULT_GTSDK_GAME_ID, DEFAULT_AUTH_SECRET, VNSG2_MAPI_APP_KEY,
};
pub use protocol::{
    AccountRecord, GameServerEntry, HttpCookieSession, IssuedGameHash, RoleSummary,
    ScriptPatchEntry, ServerConfig, ServerStatus, VersionPolicy, COOKIE_GAME_SESSION,
    COOKIE_PHPSESSID,
};
pub use router::MapiService;

/// Convenience launcher that constructs an `Arc<MapiService>` and binds an HTTP/1.1 listener.
pub fn start_mapi_server<A: ToSocketAddrs>(
    addr: A,
    config: ServerConfig,
) -> io::Result<(Arc<MapiService>, HttpServerHandle)> {
    let service = Arc::new(MapiService::new(config));
    let handle = spawn_http_server(addr, Arc::clone(&service), HttpServerConfig::default())?;
    Ok((service, handle))
}
