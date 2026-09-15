//! The two ends of a proxy over a modem call.
//!
//! On the machine that dialled, [`client::Client`] listens on a local port and
//! hands whatever connects to it across the link. On the machine that
//! answered, [`server::Server`] takes those, reads out of each what it is
//! asking for, opens the real connection, and passes the two streams through
//! each other.
//!
//! Either protocol, on the one port. A browser may be pointed at it as a SOCKS
//! host or as an HTTP proxy and the far end tells which from the first octet
//! it sends. On a call this slow the HTTP side is the faster of the two by
//! about a second a connection, because SOCKS spends two round trips agreeing
//! what to open before the request crosses and HTTP spends none -- but the
//! choice belongs to whoever is setting up the browser, and getting it wrong
//! is no longer a way to see an empty page.
//!
//! Between them is our own TCP over our own IP over PPP over a modem. Nothing
//! in the path is the operating system's, except the sockets at the far end
//! that actually reach the internet -- which is the point: the dialling
//! machine needs no driver, no adapter, no route and no administrator, only a
//! browser with a proxy setting.
//!
//! Neither of these knows about PPP. Datagram payloads go in and come out, and
//! whoever owns the link carries them.

pub mod client;
pub mod server;

pub use client::Client;
pub use server::Server;

/// The port both proxies live on: where SOCKS is "conventionally located on"
/// (RFC 1928 3), and where an HTTP proxy is equally happy to be.
pub const PROXY_PORT: u16 = 1080;

/// How much to move between a socket and a connection in one go.
///
/// A modem carries at most a couple of kilobytes a second, so the size is
/// about how much work one round of the loop does rather than about
/// throughput.
pub(crate) const CHUNK: usize = 4096;
