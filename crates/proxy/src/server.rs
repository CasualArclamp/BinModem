//! The end with the internet.
//!
//! Connections arrive over the link, each one a SOCKS conversation. When one
//! says where it wants to go, a real socket is opened to it and from then on
//! the two streams are each other's.
//!
//! The only blocking thing a proxy does is open the outbound connection: a
//! name has to be resolved and a handshake has to complete, either of which
//! can take seconds. That happens on a thread of its own so the link keeps
//! moving, and the answer comes back down a channel.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use socks::{Reply, Session};
use tcp::connection::Report;
use tcp::stack::{Handle, Outgoing, Stack};

use crate::{CHUNK, SOCKS_PORT};

/// How long to wait for the far side of the internet before giving up on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How much may be waiting to go out of a socket before this end stops taking
/// more in.
///
/// The link is slower than the internet by a factor of thousands, so without
/// a limit the buffer between them is however large the page is.
const MOST_BUFFERED: usize = 64 * 1024;

/// One connection over the link, and the socket it turned into.
#[derive(Debug)]
struct Relayed {
    socks: Session,
    /// The real connection, once there is one.
    socket: Option<TcpStream>,
    /// The thread opening it, while it is being opened.
    opening: Option<Receiver<Result<TcpStream, String>>>,
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
    /// Whether the socket has said it has no more to give.
    socket_finished: bool,
    /// And whether the far side of the internet has been told that the
    /// browser has. A shutdown of the writing half is how a socket says it.
    told_socket: bool,
}

/// The proxy on the machine that answered the call.
#[derive(Debug)]
pub struct Server {
    stack: Stack,
    relays: HashMap<Handle, Relayed>,
    log: Vec<String>,
    port: u16,
}

impl Server {
    /// Listen for SOCKS connections at `address` over the link.
    pub fn new(address: [u8; 4], seed: u32) -> Self {
        let mut stack = Stack::new(address, seed);
        stack.listen(SOCKS_PORT);
        Self {
            stack,
            relays: HashMap::new(),
            log: Vec::new(),
            port: SOCKS_PORT,
        }
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
            self.relays.insert(
                handle,
                Relayed {
                    socks: Session::new(),
                    socket: None,
                    opening: None,
                    to_socket: Vec::new(),
                    to_link: Vec::new(),
                    going_to: String::new(),
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
        // Off the link, through SOCKS, and towards the socket.
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
            let forward = relay.socks.feed(&from_link);
            relay.to_socket.extend(forward);
        }

        // A request nobody has answered yet: open it.
        if relay.socket.is_none() && relay.opening.is_none()
            && let Some(request) = relay.socks.request()
        {
            let where_to = format!("{}:{}", request.destination, request.port);
            relay.going_to = where_to.clone();
            self.log.push(format!("proxy: opening {where_to}"));
            let (sender, receiver) = channel();
            std::thread::spawn(move || {
                let _ = sender.send(open(&where_to));
            });
            relay.opening = Some(receiver);
        }

        // Has it opened?
        if let Some(receiver) = relay.opening.as_ref() {
            match receiver.try_recv() {
                Ok(Ok(socket)) => {
                    relay.opening = None;
                    let bound = socket
                        .local_addr()
                        .ok()
                        .and_then(|a| match a {
                            std::net::SocketAddr::V4(v4) => Some((v4.ip().octets(), v4.port())),
                            std::net::SocketAddr::V6(_) => None,
                        })
                        .unwrap_or(([0, 0, 0, 0], 0));
                    let early = relay.socks.answer(Reply::Succeeded, bound);
                    relay.to_socket.extend(early);
                    relay.socket = Some(socket);
                    self.log.push(format!("proxy: {} open", relay.going_to));
                }
                Ok(Err(why)) => {
                    relay.opening = None;
                    // The client is told in its own terms, which is what makes
                    // a browser show the right page rather than "proxy error".
                    let reply = if why.contains("refused") {
                        Reply::ConnectionRefused
                    } else if why.contains("resolve") {
                        Reply::HostUnreachable
                    } else {
                        Reply::GeneralFailure
                    };
                    let _ = relay.socks.answer(reply, ([0, 0, 0, 0], 0));
                    self.log
                        .push(format!("proxy: {} would not open: {why}", relay.going_to));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    relay.opening = None;
                    let _ = relay.socks.answer(Reply::GeneralFailure, ([0, 0, 0, 0], 0));
                }
            }
        }

        // What SOCKS has to say goes back over the link, along with anything
        // the far end sent.
        let for_link = relay.socks.take_out();
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
        let trouble = relay.socks.trouble().is_some() && !relay.socks.open();
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
fn open(where_to: &str) -> Result<TcpStream, String> {
    let addresses: Vec<_> = where_to
        .to_socket_addrs()
        .map_err(|e| format!("could not resolve: {e}"))?
        .collect();
    let mut last = "no address".to_owned();
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(socket) => {
                // Every keystroke and every small write goes at once. A modem
                // is slow enough without waiting to fill a segment as well.
                let _ = socket.set_nodelay(true);
                socket
                    .set_nonblocking(true)
                    .map_err(|e| format!("{address}: {e}"))?;
                return Ok(socket);
            }
            Err(e) => last = format!("{address}: {e}"),
        }
    }
    Err(last)
}

/// Whether a relay is holding more than it should be.
///
/// Kept as a function rather than folded into the loop so the limit has a name
/// and one place to be changed.
pub(crate) fn too_much(buffered: usize) -> bool {
    buffered >= MOST_BUFFERED
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

    /// The server listens where RFC 1928 3 says a SOCKS server lives.
    #[test]
    fn it_listens_where_socks_belongs() {
        let server = Server::new([10, 0, 0, 1], 1);
        assert_eq!(server.port(), 1080);
        assert_eq!(server.address(), [10, 0, 0, 1]);
        assert_eq!(server.open(), 0);
    }
}
