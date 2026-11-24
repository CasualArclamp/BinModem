//! The end with the internet, when the far end of the call is a BinModem.
//!
//! Connections arrive over the link, each one a browser's, speaking HTTP to a
//! proxy. When one says where it wants to go, a real socket is opened there
//! and from then on the two streams are each other's.
//!
//! The first octet says whether it is HTTP at all: every method name is
//! uppercase letters (RFC 9110 9). A browser set up for SOCKS opens with 5
//! instead (RFC 1928 3), and is told nothing it could use -- this end no
//! longer speaks SOCKS -- so it is said on the panel, where somebody can put
//! the setting right.
//!
//! The only blocking thing a proxy does is open the outbound connection: a
//! name has to be resolved and a handshake has to complete, either of which
//! can take seconds. That happens on a thread of its own so the link keeps
//! moving, and the answer comes back down a channel.
//!
//! Where it opens one is limited: the internet, and not the machine this runs
//! on or the network around it (see [`crate::policy`]).

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use tcp::connection::Report;
use tcp::stack::{Handle, Outgoing, Stack};

use crate::{CHUNK, FAR_PORT, policy, too_much};

/// How long to wait for the far side of the internet before giving up on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// What a browser turned out to be speaking.
#[derive(Debug)]
enum Conversation {
    /// Nothing has arrived to decide by.
    Silent,
    Http(http::Session),
    /// Not HTTP, and nothing more will be made of it.
    Other(&'static str),
}

impl Conversation {
    /// Octets from the browser; returns what should go to the origin server.
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if matches!(self, Conversation::Silent)
            && let Some(&first) = bytes.first()
        {
            *self = if http::speaks_http(first) {
                Conversation::Http(http::Session::new())
            } else if first == 5 {
                Conversation::Other("a browser set up for SOCKS, which this end does not speak")
            } else {
                Conversation::Other("something that is not HTTP")
            };
        }
        match self {
            Conversation::Http(h) => h.feed(bytes),
            Conversation::Silent | Conversation::Other(_) => Vec::new(),
        }
    }

    /// Somewhere that needs a socket, if anywhere does.
    fn wants(&self) -> Option<http::Target> {
        match self {
            Conversation::Http(h) => h.request().cloned(),
            Conversation::Silent | Conversation::Other(_) => None,
        }
    }

    /// It opened. Returns anything that was waiting to go out of it.
    fn opened(&mut self) -> Vec<u8> {
        match self {
            Conversation::Http(h) => h.answer(http::Answer::Opened),
            Conversation::Silent | Conversation::Other(_) => Vec::new(),
        }
    }

    /// It is somewhere this proxy does not go, and the browser is told so.
    fn forbidden(&mut self) {
        if let Conversation::Http(h) = self {
            let _ = h.answer(http::Answer::Forbidden);
        }
    }

    /// It did not. The browser is told in HTTP, which is what makes it show a
    /// page saying so rather than an empty one.
    fn would_not_open(&mut self, why: &str) {
        if let Conversation::Http(h) = self {
            let answer = if why.contains("refused") {
                http::Answer::Refused
            } else if why.contains("resolve") {
                http::Answer::Unreachable
            } else if why.contains("timed out") {
                http::Answer::TimedOut
            } else {
                http::Answer::Failed
            };
            let _ = h.answer(answer);
        }
    }

    /// Octets for the browser.
    fn take_out(&mut self) -> Vec<u8> {
        match self {
            Conversation::Http(h) => h.take_out(),
            Conversation::Silent | Conversation::Other(_) => Vec::new(),
        }
    }

    fn trouble(&self) -> Option<&'static str> {
        match self {
            Conversation::Http(h) => h.trouble(),
            Conversation::Other(why) => Some(why),
            Conversation::Silent => None,
        }
    }

    fn open(&self) -> bool {
        matches!(self, Conversation::Http(h) if h.open())
    }
}

/// One connection over the link, and the socket it turned into.
#[derive(Debug)]
struct Relayed {
    talk: Conversation,
    /// The real connection, once there is one.
    socket: Option<TcpStream>,
    /// The thread opening it, while it is being opened.
    opening: Option<Receiver<Result<TcpStream, Unopened>>>,
    /// What has come off the link and not gone into the socket yet.
    to_socket: Vec<u8>,
    /// And what has come off the socket and not gone onto the link yet.
    ///
    /// The two directions are thousands of times apart in speed, so something
    /// has to hold what the fast one produced until the slow one can take it.
    /// This is that, and [`too_much`] is why it does not grow for ever.
    to_link: Vec<u8>,
    /// Where it was going, for the log.
    going_to: String,
    /// Whether the conversation has already been complained about.
    complained: bool,
    /// Whether the socket has said it has no more to give.
    socket_finished: bool,
    /// And whether the far side of the internet has been told that the
    /// browser has. A shutdown of the writing half is how a socket says it.
    told_socket: bool,
}

/// Why a socket was not opened.
#[derive(Debug)]
enum Unopened {
    /// It would have gone somewhere [`policy::refused`] keeps callers out of.
    Forbidden(&'static str),
    /// It was tried, and did not open.
    Failed(String),
}

/// The proxy on the machine that answered the call.
#[derive(Debug)]
pub struct Server {
    stack: Stack,
    relays: HashMap<Handle, Relayed>,
    log: Vec<String>,
    port: u16,
    /// Whether [`policy::refused`] is set aside, which only a test wants: its
    /// web server is on this machine.
    anywhere: bool,
}

impl Server {
    /// Listen for browsers at `address` over the link.
    pub fn new(address: [u8; 4], seed: u32) -> Self {
        let mut stack = Stack::new(address, seed);
        stack.listen(FAR_PORT);
        Self {
            stack,
            relays: HashMap::new(),
            log: Vec::new(),
            port: FAR_PORT,
            anywhere: false,
        }
    }

    /// Go anywhere this machine can reach, its own addresses and every port
    /// included.
    ///
    /// For tests, whose web servers listen on the loopback. A caller given
    /// this reaches everything a firewall assumes nobody outside can.
    pub fn reach_anywhere(&mut self) {
        self.anywhere = true;
    }

    /// Size connections for what the link carries (see
    /// [`tcp::Stack::size_for_link`]).
    pub fn size_for_link(&mut self, largest_in: u16, largest_out: u16) {
        self.stack.size_for_link(largest_in, largest_out);
    }

    /// IPCP settles the address after the link is up, which is later than a
    /// stack would like but before anything is open.
    pub fn set_address(&mut self, address: [u8; 4]) -> bool {
        self.stack.set_address(address)
    }

    pub fn address(&self) -> [u8; 4] {
        self.stack.address()
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// How many connections are being carried.
    pub fn open(&self) -> usize {
        self.relays.len()
    }

    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// A TCP payload arrived in a datagram.
    pub fn deliver(&mut self, from: [u8; 4], to: [u8; 4], payload: &[u8]) {
        self.stack.deliver(from, to, payload);
    }

    /// Segments to put on the link.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.stack.take_outgoing()
    }

    /// One round: time passes, connections arrive, sockets are read and
    /// written.
    pub fn tick(&mut self, ms: u32) {
        self.stack.tick(ms);

        for handle in self.stack.take_arrived() {
            // Said out loud because everything between a browser asking and a
            // socket opening happens over the link, where nobody can see it.
            self.log.push("proxy: a connection arrived over the link".to_owned());
            self.relays.insert(
                handle,
                Relayed {
                    talk: Conversation::Silent,
                    socket: None,
                    opening: None,
                    to_socket: Vec::new(),
                    to_link: Vec::new(),
                    going_to: String::new(),
                    complained: false,
                    socket_finished: false,
                    told_socket: false,
                },
            );
        }
        for event in self.stack.take_events() {
            match event.report {
                Report::Reset | Report::Refused | Report::Closed => {
                    if let Some(relay) = self.relays.remove(&event.handle)
                        && !relay.going_to.is_empty()
                    {
                        self.log.push(format!("proxy: {} closed", relay.going_to));
                    }
                }
                Report::Established | Report::Data | Report::Closing => {}
            }
        }

        let handles: Vec<Handle> = self.relays.keys().copied().collect();
        for handle in handles {
            self.carry(handle);
        }
    }

    /// Everything one connection has to do this round.
    fn carry(&mut self, handle: Handle) {
        // Off the link, through the HTTP session, and towards the socket.
        let (from_link, browser_finished) = match self.stack.get_mut(handle) {
            Some(connection) => {
                let data = connection.take_received();
                (data, connection.finished() && connection.available() == 0)
            }
            None => {
                self.relays.remove(&handle);
                return;
            }
        };
        let Some(relay) = self.relays.get_mut(&handle) else {
            return;
        };
        if !from_link.is_empty() {
            let forward = relay.talk.feed(&from_link);
            relay.to_socket.extend(forward);
        }
        // Whatever it made of it, said once. Without this a browser speaking
        // neither stalls with nothing on the panel at all.
        if !relay.complained
            && let Some(why) = relay.talk.trouble()
        {
            relay.complained = true;
            self.log.push(format!("proxy: the connection made no sense: {why}"));
        }

        // Somewhere to open, and possibly not the place already open. A
        // browser talking to an HTTP proxy keeps one connection and asks it
        // for whatever host it needs next, so the socket underneath has to be
        // allowed to change -- which is also most of the saving, since the
        // ones that do not change cost nothing at all.
        if relay.opening.is_none()
            && let Some(want) = relay.talk.wants()
            // Nothing already taken off the old socket is abandoned. What has
            // not reached the link yet is the tail of the last page, and the
            // swap can wait the round or two it takes to go.
            && (relay.socket.is_none()
                || (relay.going_to != want.authority && relay.to_link.is_empty()))
        {
            if let Some(socket) = relay.socket.as_mut() {
                // One last look before it goes. A browser only asks for
                // somewhere else once it has read the last response to its
                // declared end (RFC 9112 6.3), so there should be nothing --
                // but "should be nothing" is not a reason to drop it unread.
                let mut buffer = [0u8; CHUNK];
                while let Ok(n) = socket.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    relay.to_link.extend_from_slice(&buffer[..n]);
                }
                self.log
                    .push(format!("proxy: {} is finished with", relay.going_to));
                relay.socket = None;
            }
            // Both of these belong to the socket that has just gone.
            relay.socket_finished = false;
            relay.told_socket = false;
            let where_to = want.authority.clone();
            relay.going_to = where_to.clone();
            let how = if want.tunnel { " to tunnel through" } else { "" };
            self.log.push(format!("proxy: opening {where_to}{how}"));
            let (sender, receiver) = channel();
            let (tunnel, anywhere) = (want.tunnel, self.anywhere);
            std::thread::spawn(move || {
                let _ = sender.send(open(&where_to, tunnel, anywhere));
            });
            relay.opening = Some(receiver);
        }

        // Has it opened?
        if let Some(receiver) = relay.opening.as_ref() {
            match receiver.try_recv() {
                Ok(Ok(socket)) => {
                    relay.opening = None;
                    let early = relay.talk.opened();
                    relay.to_socket.extend(early);
                    relay.socket = Some(socket);
                    self.log.push(format!("proxy: {} open", relay.going_to));
                }
                Ok(Err(Unopened::Forbidden(why))) => {
                    relay.opening = None;
                    relay.talk.forbidden();
                    self.log
                        .push(format!("proxy: {} is off limits: {why}", relay.going_to));
                }
                Ok(Err(Unopened::Failed(why))) => {
                    relay.opening = None;
                    relay.talk.would_not_open(&why);
                    self.log
                        .push(format!("proxy: {} would not open: {why}", relay.going_to));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    relay.opening = None;
                    relay.talk.would_not_open("the thread opening it went away");
                }
            }
        }

        // What the proxy conversation has to say goes back over the link,
        // along with anything the far end sent.
        let for_link = relay.talk.take_out();
        relay.to_link.extend(for_link);
        let mut gone = false;
        if let Some(socket) = relay.socket.as_mut() {
            // Towards the internet.
            while !relay.to_socket.is_empty() {
                match socket.write(&relay.to_socket) {
                    Ok(0) => {
                        gone = true;
                        break;
                    }
                    Ok(n) => {
                        relay.to_socket.drain(..n);
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        gone = true;
                        break;
                    }
                }
            }
        }

        // The browser has finished asking and everything it asked has gone
        // out, so the far side is told -- an HTTP server that waits for the
        // end of a request would otherwise wait for ever.
        if browser_finished && relay.to_socket.is_empty() && !relay.told_socket {
            relay.told_socket = true;
            if let Some(socket) = relay.socket.as_ref() {
                let _ = socket.shutdown(std::net::Shutdown::Write);
            }
        }

        // And back from it -- but only while there is somewhere to put it.
        // The internet is thousands of times faster than the link, so a socket
        // read without a limit on it is a page held entirely in memory.
        if let Some(socket) = relay.socket.as_mut()
            && !gone
            && !relay.socket_finished
            && !too_much(relay.to_link.len())
        {
            let mut buffer = [0u8; CHUNK];
            match socket.read(&mut buffer) {
                Ok(0) => relay.socket_finished = true,
                Ok(n) => relay.to_link.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => gone = true,
            }
        }
        let finished = relay.socket_finished;
        let trouble = relay.talk.trouble().is_some() && !relay.talk.open();
        let going_to = relay.going_to.clone();
        if gone {
            relay.socket = None;
        }
        let mut to_link = std::mem::take(&mut relay.to_link);

        let Some(connection) = self.stack.get_mut(handle) else {
            return;
        };
        // As much as the connection will take, and the rest waits. What it
        // will not take now must not be dropped: it is the middle of somebody's
        // page.
        let took = connection.send(&to_link);
        to_link.drain(..took);
        let still_waiting = !to_link.is_empty();
        if let Some(relay) = self.relays.get_mut(&handle) {
            relay.to_link = to_link;
        }
        // Closing while anything is still waiting would throw it away.
        if still_waiting {
            return;
        }
        let Some(connection) = self.stack.get_mut(handle) else {
            return;
        };
        if finished || gone || trouble {
            connection.close();
            if gone && !going_to.is_empty() {
                self.log.push(format!("proxy: {going_to} went away"));
            }
        }
    }
}

/// Resolve and connect, on a thread of its own.
///
/// Each address is judged as it was resolved, and the socket goes to that
/// same address, so a name that resolves to somewhere harmless when asked and
/// somewhere else a moment later gets nowhere: nothing is looked up twice.
fn open(where_to: &str, tunnel: bool, anywhere: bool) -> Result<TcpStream, Unopened> {
    let addresses: Vec<_> = where_to
        .to_socket_addrs()
        .map_err(|e| Unopened::Failed(format!("could not resolve: {e}")))?
        .collect();
    let mut forbidden = None;
    let mut last = None;
    for address in addresses {
        if !anywhere
            && let Some(why) = policy::refused(address.ip(), address.port(), tunnel)
        {
            forbidden = Some(why);
            continue;
        }
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(socket) => {
                // Every keystroke and every small write goes at once. A modem
                // is slow enough without waiting to fill a segment as well.
                let _ = socket.set_nodelay(true);
                socket
                    .set_nonblocking(true)
                    .map_err(|e| Unopened::Failed(format!("{address}: {e}")))?;
                return Ok(socket);
            }
            Err(e) => last = Some(format!("{address}: {e}")),
        }
    }
    // Refused only where nothing was tried: an address that was allowed and
    // failed says more about why there is no page than one that was not.
    match (last, forbidden) {
        (Some(why), _) => Err(Unopened::Failed(why)),
        (None, Some(why)) => Err(Unopened::Forbidden(why)),
        (None, None) => Err(Unopened::Failed("no address".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// It listens where a far end has always found it.
    #[test]
    fn it_listens_where_a_browser_on_the_far_end_is_sent() {
        let server = Server::new([10, 0, 0, 1], 1);
        assert_eq!(server.port(), 1080);
        assert_eq!(server.address(), [10, 0, 0, 1]);
        assert_eq!(server.open(), 0);
    }
}

#[cfg(test)]
mod opening {
    use super::Conversation;

    #[test]
    fn a_request_says_where_it_wants_to_go() {
        let mut http = Conversation::Silent;
        http.feed(b"GET http://example.invalid/ HTTP/1.1\r\n\r\n");
        assert_eq!(http.wants().map(|w| w.authority), Some("example.invalid:80".to_owned()));
        assert!(http.trouble().is_none());
        // And until something arrives there is nothing to decide.
        assert!(Conversation::Silent.wants().is_none());
    }

    /// A browser set up for SOCKS is told nothing, and the panel is told why.
    #[test]
    fn a_socks_greeting_is_recognised_and_refused() {
        let mut socks = Conversation::Silent;
        assert!(socks.feed(&[5, 1, 0]).is_empty());
        assert!(socks.wants().is_none());
        assert!(socks.trouble().is_some_and(|why| why.contains("SOCKS")));
        assert!(socks.take_out().is_empty());
    }

    /// A CONNECT names a tunnel, which is how https goes over the link.
    #[test]
    fn a_connect_is_recognised_as_a_tunnel() {
        let mut talk = Conversation::Silent;
        talk.feed(b"CONNECT example.invalid:443 HTTP/1.1\r\n\r\n");
        let want = talk.wants().expect("it asked for nowhere");
        assert_eq!(want.authority, "example.invalid:443");
        assert!(want.tunnel);
    }
}
