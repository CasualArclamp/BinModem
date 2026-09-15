//! The end that dialled.
//!
//! A listener on the local machine, and for everything that connects to it one
//! connection across the link to the far end's proxy. Nothing here understands
//! SOCKS: the browser's side of that conversation is forwarded untouched and
//! answered by the machine that can actually open the connection. This end is
//! a pipe with a modem in the middle of it.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};

use tcp::connection::Report;
use tcp::stack::{Handle, Outgoing, Stack};
use tcp::Endpoint;

use crate::{CHUNK, SOCKS_PORT};

/// One browser connection and the link connection carrying it.
#[derive(Debug)]
struct Relayed {
    socket: TcpStream,
    /// What has come off the link and not gone into the socket yet.
    to_socket: Vec<u8>,
    /// And what has come off the socket and not gone onto the link yet: a
    /// browser can produce a request faster than a modem can carry it.
    to_link: Vec<u8>,
    /// Whether the browser has said it has no more to give.
    socket_finished: bool,
    /// And whether the browser has been told the far end has.
    ///
    /// A FIN over the link means the far end will send no more, and the only
    /// way to say that on a socket is to shut down the writing half of it.
    /// Without this a browser reading to the end of a page waits for ever for
    /// an end that has already happened somewhere else.
    told_socket: bool,
    /// Whether the connection across the link has been answered. Until it is,
    /// the browser is waiting on something that may not be there.
    established: bool,
}

/// The proxy on the machine that dialled.
#[derive(Debug)]
pub struct Client {
    listener: TcpListener,
    stack: Stack,
    /// Where the far end's proxy is.
    server: Endpoint,
    relays: HashMap<Handle, Relayed>,
    log: Vec<String>,
    /// Where the listener actually ended up, which is not what was asked for
    /// when port zero was.
    bound: std::net::SocketAddr,
    /// Whether anything has ever been answered at the far end.
    ///
    /// The far end only answers if it is running the other half of this, which
    /// it does when the machine that answered the call has been asked to carry
    /// web traffic too. Without it there is nothing listening, and nothing
    /// says so: the connections are not refused, they go unanswered, and the
    /// browser is left with a socket that opens and closes having carried
    /// nothing. Worth telling somebody about rather than counting as traffic.
    answered: bool,
}

impl Client {
    /// Listen on `at` -- "127.0.0.1:1080" for a browser on this machine -- and
    /// carry everything that arrives to the proxy at `server_address`.
    pub fn new(at: &str, address: [u8; 4], server_address: [u8; 4], seed: u32) -> Result<Self, String> {
        let listener = TcpListener::bind(at).map_err(|e| format!("{at}: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("{at}: {e}"))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?;
        Ok(Self {
            listener,
            stack: Stack::new(address, seed),
            server: Endpoint::new(server_address, SOCKS_PORT),
            relays: HashMap::new(),
            log: Vec::new(),
            bound,
            answered: false,
        })
    }

    /// Where a browser should be pointed.
    pub fn bound(&self) -> std::net::SocketAddr {
        self.bound
    }

    pub fn address(&self) -> [u8; 4] {
        self.stack.address()
    }

    /// IPCP hands the address over after the link is up.
    pub fn set_address(&mut self, address: [u8; 4]) -> bool {
        self.stack.set_address(address)
    }

    /// And says what the far end is called.
    pub fn set_server(&mut self, address: [u8; 4]) {
        self.server = Endpoint::new(address, SOCKS_PORT);
    }

    /// Connections the far end has answered and is carrying.
    pub fn open(&self) -> usize {
        self.relays.values().filter(|r| r.established).count()
    }

    /// Connections a browser is waiting on that the far end has not answered.
    pub fn waiting(&self) -> usize {
        self.relays.values().filter(|r| !r.established).count()
    }

    /// Whether the far end has ever answered one. False with connections
    /// waiting is the other half of the proxy not being there at all.
    pub fn answered(&self) -> bool {
        self.answered
    }

    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    pub fn deliver(&mut self, from: [u8; 4], to: [u8; 4], payload: &[u8]) {
        self.stack.deliver(from, to, payload);
    }

    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.stack.take_outgoing()
    }

    /// One round: take what has connected, move what has arrived.
    pub fn tick(&mut self, ms: u32) {
        self.stack.tick(ms);
        self.accept();

        for event in self.stack.take_events() {
            match event.report {
                Report::Reset | Report::Refused | Report::Closed => {
                    if self.relays.remove(&event.handle).is_some() {
                        self.log.push("proxy: a connection ended".to_owned());
                    }
                }
                Report::Established => {
                    self.answered = true;
                    if let Some(relay) = self.relays.get_mut(&event.handle) {
                        relay.established = true;
                    }
                }
                Report::Data | Report::Closing => {}
            }
        }

        let handles: Vec<Handle> = self.relays.keys().copied().collect();
        for handle in handles {
            self.carry(handle);
        }
    }

    /// Whatever has connected to the listener since last time.
    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((socket, from)) => {
                    if socket.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let _ = socket.set_nodelay(true);
                    let Some(handle) = self.stack.connect(self.server) else {
                        // Nothing left to open one with. Dropping the socket
                        // closes it, which tells the browser at once rather
                        // than leaving it waiting.
                        self.log
                            .push("proxy: too many connections, one was dropped".to_owned());
                        continue;
                    };
                    self.log.push(format!("proxy: {from} wants the far end"));
                    self.relays.insert(
                        handle,
                        Relayed {
                            socket,
                            to_socket: Vec::new(),
                            to_link: Vec::new(),
                            socket_finished: false,
                            told_socket: false,
                            established: false,
                        },
                    );
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    fn carry(&mut self, handle: Handle) {
        // A connection the stack has forgotten is one that is over, not one
        // that never happened: what it handed over before it went is still
        // owed to the browser. Dropping the relay here would close the socket
        // with a page still in hand, and a browser whose connection closes
        // having carried nothing reports an empty page -- which is not what
        // happened, and sends whoever is looking at it after the wrong thing.
        let (from_link, far_finished, forgotten) = match self.stack.get_mut(handle) {
            Some(connection) => {
                let data = connection.take_received();
                (data, connection.finished() && connection.available() == 0, false)
            }
            None => (Vec::new(), true, true),
        };
        let Some(relay) = self.relays.get_mut(&handle) else {
            return;
        };
        relay.to_socket.extend(from_link);

        let mut gone = false;
        while !relay.to_socket.is_empty() {
            match relay.socket.write(&relay.to_socket) {
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

        // Everything the far end sent has been handed over and it has said
        // there will be no more, so the socket is told the same way.
        if far_finished && relay.to_socket.is_empty() && !relay.told_socket {
            relay.told_socket = true;
            let _ = relay.socket.shutdown(std::net::Shutdown::Write);
        }

        if !gone
            && !forgotten
            && !relay.socket_finished
            && !crate::server::too_much(relay.to_link.len())
        {
            let mut buffer = [0u8; CHUNK];
            match relay.socket.read(&mut buffer) {
                Ok(0) => relay.socket_finished = true,
                Ok(n) => relay.to_link.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => gone = true,
            }
        }
        let finished = relay.socket_finished;
        // What came off the link and has not reached the browser yet. The
        // relay has to outlive the connection that brought it.
        let owed_to_socket = !relay.to_socket.is_empty();
        let mut to_link = std::mem::take(&mut relay.to_link);

        let mut still_waiting = false;
        if let Some(connection) = self.stack.get_mut(handle) {
            // Whatever the connection will take now; the rest waits rather
            // than being dropped, because the rest is the middle of a request.
            let took = connection.send(&to_link);
            to_link.drain(..took);
            still_waiting = !to_link.is_empty();
        }
        if let Some(relay) = self.relays.get_mut(&handle) {
            relay.to_link = to_link;
        }
        if !still_waiting
            && (gone || finished)
            && let Some(connection) = self.stack.get_mut(handle)
        {
            connection.close();
        }
        // Both halves are over and nothing is owed either way: nothing more
        // will come off the socket, nothing more will come off the link, and
        // everything that did has gone where it was going. Dropping the relay
        // closes what is left of the socket, so it happens last of all.
        let over = forgotten || (finished && far_finished && !still_waiting);
        if gone || (over && !owed_to_socket) {
            self.relays.remove(&handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

    /// The far end only answers when the machine that answered the call is
    /// carrying web traffic too. Without it there is nothing listening, and
    /// nothing to refuse the connection either: it goes unanswered. That is
    /// not traffic, and the panel has to be able to tell the difference.
    #[test]
    fn a_far_end_that_is_not_there_leaves_the_connections_waiting() {
        let mut client = Client::new("127.0.0.1:0", [10, 0, 0, 2], [10, 0, 0, 1], 7)
            .expect("could not listen");
        assert_eq!((client.open(), client.waiting(), client.answered()), (0, 0, false));

        // A browser connects and asks for something. Nothing at the far end
        // will ever answer, because nothing is there.
        let mut browser = TcpStream::connect(client.bound()).expect("connect");
        let _ = browser.write_all(&[5, 1, 0]);
        for _ in 0..2_000 {
            client.tick(1);
            let _ = client.take_outgoing();
            if client.waiting() > 0 {
                break;
            }
        }
        assert_eq!(client.waiting(), 1, "the browser's connection was not waiting on anything");
        assert_eq!(client.open(), 0, "an unanswered connection was counted as carried");
        assert!(!client.answered(), "it claimed the far end had answered");
    }
}
