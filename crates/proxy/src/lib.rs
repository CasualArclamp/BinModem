//! The two ends of a proxy over a modem call.
//!
//! On the machine that dialled, [`client::Client`] listens on a local port and
//! hands whatever connects to it across the link. On the machine that
//! answered, [`server::Server`] takes those, reads the SOCKS request out of
//! each, opens the real connection it asks for, and passes the two streams
//! through each other.
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

/// The port SOCKS is "conventionally located on" (RFC 1928 3).
pub const SOCKS_PORT: u16 = 1080;

/// How much to move between a socket and a connection in one go.
///
/// A modem carries at most a couple of kilobytes a second, so the size is
/// about how much work one round of the loop does rather than about
/// throughput.
pub(crate) const CHUNK: usize = 4096;
