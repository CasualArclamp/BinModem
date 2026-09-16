//! A web proxy over a modem call: HTTP, and HTTPS through CONNECT.
//!
//! A browser on the machine that dialled is pointed at [`client::Client`] as
//! its HTTP proxy, for both http and https. What happens to a request after
//! that depends on what answered the call:
//!
//! - **Another BinModem**, carrying web traffic. That machine has the
//!   internet and this one only has the call, so each browser connection is
//!   passed across the link untouched and [`server::Server`] at the far end
//!   reads it and opens the real connection.
//! - **Anything else** -- a provider's modem pool, whose far end is a router.
//!   Nothing over there will read a proxy request, so this end reads it
//!   itself ([`gateway`]) and opens the connection straight to the web
//!   server, through the router, with this end's own TCP over this end's own
//!   IP over the call. Only the name lookup goes elsewhere: see [`resolve`].
//!
//! Which of the two it is is found out rather than configured
//! ([`Route::Auto`]): a connection is offered to the far end on
//! [`FAR_PORT`], and a far end that answers is a BinModem.
//!
//! HTTP rather than SOCKS because of what a slow call costs. RFC 9112 3.2.2
//! has the browser put the whole target in its request, so the first thing
//! it says is already the request, and the connection behind it can be opened
//! while it is still arriving.
//!
//! None of this is the operating system's network, except the sockets the
//! browser talks to, the far BinModem's sockets to the internet, and the name
//! lookups. Nothing is installed, routed or configured on the dialling
//! machine beyond a browser's proxy setting.

pub mod client;
pub mod gateway;
pub mod resolve;
pub mod server;

pub use client::Client;
pub use server::Server;

/// Where a BinModem carrying web traffic listens over the link.
///
/// 1080 because that is where it has always been, and a far end running an
/// older build still answers there. What it answers in is HTTP.
pub const FAR_PORT: u16 = 1080;

/// Where the browser is pointed on the dialling machine, unless told
/// otherwise: the port HTTP proxies are conventionally found on.
pub const DEFAULT_PORT: u16 = 8080;

/// How much to move between a socket and a connection in one go.
///
/// A modem carries at most a few kilobytes a second, so the size is about how
/// much work one round of the loop does rather than about throughput.
pub(crate) const CHUNK: usize = 4096;

/// How much may be waiting to go one way before this end stops taking more in.
///
/// The link is slower than the machine by a factor of thousands, so without a
/// limit the buffer between them is however large the page is.
const MOST_BUFFERED: usize = 64 * 1024;

/// Whether a relay is holding more than it should be.
pub(crate) fn too_much(buffered: usize) -> bool {
    buffered >= MOST_BUFFERED
}

/// How the dialling end gets a browser's request to the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Route {
    /// Find out: through a far BinModem if one answers, straight out if not.
    #[default]
    Auto,
    /// Always through the far end's proxy.
    FarEnd,
    /// Always straight to the web server, over this end's own stack.
    Direct,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Auto => "find out",
            Route::FarEnd => "through the far BinModem",
            Route::Direct => "straight to the internet",
        }
    }
}

/// One connection over the link, as the panel shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carried {
    /// What it is for: a host and port, or the far end's proxy.
    pub name: String,
    /// The address and port it went to.
    pub address: String,
    pub state: &'static str,
    /// RFC 6298's smoothed round trip and timeout, in milliseconds.
    pub srtt_ms: u32,
    pub rto_ms: u32,
    /// Segments sent again, since it opened.
    pub resent: u32,
    /// Segment size out, and the congestion window, in octets.
    pub send_mss: u16,
    pub cwnd: u32,
    /// Octets given to it and not yet acknowledged.
    pub unacknowledged: usize,
    /// Octets towards the web and back from it.
    pub sent: u64,
    pub received: u64,
}

/// What the dialling end's proxy is doing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    /// Where a browser should be pointed.
    pub at: String,
    /// The route as asked for, and as it turned out.
    pub asked: Route,
    pub route: &'static str,
    /// Browser connections open, and how many of them are waiting on a name
    /// or a connection.
    pub browsers: usize,
    pub waiting: usize,
    /// Every connection over the link.
    pub carried: Vec<Carried>,
    /// Name lookups made, and how many found nothing.
    pub lookups: u64,
    pub lookup_failures: u64,
    /// Octets to and from browsers.
    pub to_browsers: u64,
    pub from_browsers: u64,
    /// The segment size new connections ask for, and the most they send in.
    pub mss: (u16, u16),
    /// Whether anything at all has been answered over the link.
    pub answered: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_has_a_limit_and_it_is_not_reached_by_a_page() {
        assert!(!too_much(0));
        assert!(!too_much(60_000));
        assert!(too_much(MOST_BUFFERED));
    }
}
