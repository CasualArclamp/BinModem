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

    pub fn open(&self) -> usize {
        self.relays.len()
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
                Report::Established | Report::Data | Report::Closing => {}
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
                        },
                    );
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    fn carry(&mut self, handle: Handle) {
        let (from_link, far_finished) = match self.stack.get_mut(handle) {
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
        let mut to_link = std::mem::take(&mut relay.to_link);

        let Some(connection) = self.stack.get_mut(handle) else {
            return;
        };
        // Whatever the connection will take now; the rest waits rather than
        // being dropped, because the rest is the middle of a request.
        let took = connection.send(&to_link);
        to_link.drain(..took);
        let still_waiting = !to_link.is_empty();
        if let Some(relay) = self.relays.get_mut(&handle) {
            relay.to_link = to_link;
        }
        if !still_waiting
            && (gone || finished)
            && let Some(connection) = self.stack.get_mut(handle)
        {
            connection.close();
        }
        // Both halves are over: nothing more will come off the socket and
        // nothing more will come off the link. Dropping the relay closes what
        // is left of the socket.
        if gone || (finished && far_finished && !still_waiting) {
            self.relays.remove(&handle);
        }
    }
}
