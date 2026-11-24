//! The end that dialled: where the browser is pointed.
//!
//! Every connection a browser makes goes one of two ways, and which one is
//! settled once per link ([`crate::Route`]):
//!
//! - through the far end's proxy, when the far end is a BinModem carrying web
//!   traffic. This end is then a pipe with a modem in the middle of it:
//!   whatever the browser says is forwarded untouched, and the machine that
//!   can open the connection reads it.
//! - straight to the web server, when it is not. This end reads the request
//!   itself ([`crate::gateway`]) and opens the connection over the link to
//!   the server's own address.
//!
//! Finding out is one connection offered to the far end's proxy port. A far
//! end with a proxy answers it; a provider's router refuses it, or says
//! nothing at all. Browser connections that arrive while that is being found
//! out wait, unread, and are sent the right way once it is.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

use tcp::Endpoint;
use tcp::connection::Report;
use tcp::stack::{Handle, Outgoing, Stack};

use crate::gateway::Gateway;
use crate::resolve::Resolver;
use crate::{CHUNK, Carried, FAR_PORT, Route, View, too_much};

/// How long a far end has to answer the offer before it is taken not to have
/// a proxy. A SYN on a second's round trip, resent once or twice.
const PROBE_MS: u32 = 8_000;

/// Where the route stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// A connection has been offered to the far end and this long has passed.
    Asking(Option<Handle>, u32),
    FarEnd,
    Direct,
}

impl Decided {
    fn name(self) -> &'static str {
        match self {
            Decided::Asking(..) => "finding out whether the far end has a proxy",
            Decided::FarEnd => "through the far BinModem's proxy",
            Decided::Direct => "straight to the internet",
        }
    }
}

/// One browser connection passed through to the far end's proxy.
#[derive(Debug)]
struct Relayed {
    socket: TcpStream,
    from: SocketAddr,
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
    /// Whether the connection across the link has been answered.
    established: bool,
    said: bool,
    sent: u64,
    received: u64,
}

/// The proxy on the machine that dialled.
#[derive(Debug)]
pub struct Client {
    listener: TcpListener,
    stack: Stack,
    /// Where the far end's proxy would be.
    server: Endpoint,
    asked: Route,
    decided: Decided,
    /// Browser connections that arrived before the route was known.
    held: Vec<(TcpStream, SocketAddr)>,
    relays: HashMap<Handle, Relayed>,
    gateways: Vec<Gateway>,
    resolver: Resolver,
    log: Vec<String>,
    /// Where the listener actually ended up, which is not what was asked for
    /// when port zero was.
    bound: SocketAddr,
    answered: bool,
    to_browsers: u64,
    from_browsers: u64,
}

impl Client {
    /// Listen on `at` -- "127.0.0.1:8080" for a browser on this machine --
    /// and find out which way to send what arrives.
    pub fn new(at: &str, address: [u8; 4], server_address: [u8; 4], seed: u32) -> Result<Self, String> {
        Self::routed(at, address, server_address, seed, Route::Auto)
    }

    /// The same, with the route decided in advance.
    pub fn routed(at: &str, address: [u8; 4], server_address: [u8; 4], seed: u32, route: Route) -> Result<Self, String> {
        let listener = TcpListener::bind(at).map_err(|e| format!("{at}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| format!("{at}: {e}"))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?;
        let decided = match route {
            Route::Auto => Decided::Asking(None, 0),
            Route::FarEnd => Decided::FarEnd,
            Route::Direct => Decided::Direct,
        };
        Ok(Self {
            listener,
            stack: Stack::new(address, seed),
            server: Endpoint::new(server_address, FAR_PORT),
            asked: route,
            decided,
            held: Vec::new(),
            relays: HashMap::new(),
            gateways: Vec::new(),
            resolver: Resolver::new(),
            log: Vec::new(),
            bound,
            answered: false,
            to_browsers: 0,
            from_browsers: 0,
        })
    }

    /// Where a browser should be pointed.
    pub fn bound(&self) -> SocketAddr {
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
        self.server = Endpoint::new(address, FAR_PORT);
    }

    /// Size connections for what the link carries, as PPP's MRU counts it:
    /// the largest this end asked for, and the largest the far end will take.
    pub fn size_for_link(&mut self, largest_in: u16, largest_out: u16) {
        self.stack.size_for_link(largest_in, largest_out);
    }

    /// Whether the route has been settled as straight to the internet.
    pub fn direct(&self) -> bool {
        self.decided == Decided::Direct
    }

    /// Connections the far end has answered and is carrying.
    pub fn open(&self) -> usize {
        self.relays.values().filter(|r| r.established).count() + self.gateways.iter().filter(|g| !g.waiting()).count()
    }

    /// Connections a browser is waiting on.
    pub fn waiting(&self) -> usize {
        self.held.len()
            + self.relays.values().filter(|r| !r.established).count()
            + self.gateways.iter().filter(|g| g.waiting()).count()
    }

    /// Whether anything over the link has ever been answered.
    pub fn answered(&self) -> bool {
        self.answered
    }

    pub fn take_log(&mut self) -> Vec<String> {
        let mut log = std::mem::take(&mut self.log);
        log.extend(self.resolver.take_log());
        log
    }

    pub fn deliver(&mut self, from: [u8; 4], to: [u8; 4], payload: &[u8]) {
        self.stack.deliver(from, to, payload);
    }

    /// A router said a datagram of this end's went nowhere.
    pub fn unreachable(&mut self, about: [u8; 4], why: &str) {
        for gateway in &mut self.gateways {
            if gateway.opening_to() == Some(about) {
                gateway.unreachable(&mut self.stack, why, &mut self.log);
            }
        }
    }

    /// Segments to put on the link, each with the address it goes to.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.stack.take_outgoing()
    }

    /// Everything the panel shows.
    pub fn view(&self) -> View {
        let mut named: HashMap<Handle, (String, u64, u64)> = HashMap::new();
        for (handle, relay) in &self.relays {
            named.insert(*handle, (format!("far proxy, for {}", relay.from), relay.sent, relay.received));
        }
        for gateway in &self.gateways {
            if let Some(handle) = gateway.upstream() {
                named.insert(handle, (gateway.going_to().to_owned(), gateway.sent, gateway.received));
            }
        }
        let mut carried: Vec<(Handle, Carried)> = self
            .stack
            .connections()
            .map(|(handle, c)| {
                let status = c.status();
                let (name, sent, received) = named.get(&handle).cloned().unwrap_or_else(|| {
                    let probe = matches!(self.decided, Decided::Asking(Some(h), _) if h == handle);
                    (if probe { "asking the far end".to_owned() } else { "closing".to_owned() }, 0, 0)
                });
                let [a, b, cc, d] = c.remote.address;
                (
                    handle,
                    Carried {
                        name,
                        address: format!("{a}.{b}.{cc}.{d}:{}", c.remote.port),
                        state: status.state.name(),
                        srtt_ms: status.srtt_ms,
                        rto_ms: status.rto_ms,
                        resent: status.resent,
                        send_mss: status.send_mss,
                        cwnd: status.cwnd,
                        unacknowledged: status.unacknowledged,
                        sent,
                        received,
                    },
                )
            })
            .collect();
        carried.sort_by_key(|(handle, _)| *handle);
        View {
            at: self.bound.to_string(),
            asked: self.asked,
            route: self.decided.name(),
            browsers: self.held.len() + self.relays.len() + self.gateways.len(),
            waiting: self.waiting(),
            carried: carried.into_iter().map(|(_, c)| c).collect(),
            lookups: self.resolver.lookups,
            lookup_failures: self.resolver.failures,
            to_browsers: self.to_browsers,
            from_browsers: self.from_browsers,
            mss: self.stack.sizes(),
            answered: self.answered,
        }
    }

    /// One round: settle the route, take what has connected, move what has
    /// arrived.
    pub fn tick(&mut self, ms: u32) {
        self.stack.tick(ms);
        self.accept();

        for event in self.stack.take_events() {
            if let Decided::Asking(Some(probe), _) = self.decided
                && probe == event.handle
            {
                self.probe_answered(probe, event.report);
                continue;
            }
            if let Some(gateway) = self.gateways.iter_mut().find(|g| g.upstream() == Some(event.handle)) {
                if event.report == Report::Established {
                    self.answered = true;
                }
                gateway.event(event.report, &mut self.log);
                continue;
            }
            self.relay_event(event.handle, event.report);
        }
        self.decide(ms);

        let handles: Vec<Handle> = self.relays.keys().copied().collect();
        for handle in handles {
            self.carry(handle);
        }
        for gateway in &mut self.gateways {
            let (sent, received) = (gateway.sent, gateway.received);
            gateway.step(&mut self.stack, &mut self.resolver, ms, &mut self.log);
            self.from_browsers += gateway.sent - sent;
            self.to_browsers += gateway.received - received;
        }
        self.gateways.retain(|g| !g.over());
    }

    /// Offer the far end a connection, and wait to see what it does with it.
    fn decide(&mut self, ms: u32) {
        let Decided::Asking(probe, waited) = self.decided else {
            self.dispatch();
            return;
        };
        match probe {
            None => {
                let probe = self.stack.connect(self.server);
                self.decided = Decided::Asking(probe, 0);
                let [a, b, c, d] = self.server.address;
                self.log.push(format!("proxy: asking {a}.{b}.{c}.{d} whether it has a proxy"));
            }
            Some(handle) => {
                let waited = waited.saturating_add(ms);
                self.decided = Decided::Asking(Some(handle), waited);
                if waited > PROBE_MS {
                    if let Some(c) = self.stack.get_mut(handle) {
                        c.abort();
                    }
                    self.settle(Decided::Direct, "the far end did not answer");
                }
            }
        }
    }

    fn probe_answered(&mut self, probe: Handle, report: Report) {
        match report {
            Report::Established => {
                self.answered = true;
                // It was only a question. Closing it says so to the far end,
                // which sees a connection that asked for nothing.
                if let Some(c) = self.stack.get_mut(probe) {
                    c.close();
                }
                self.settle(Decided::FarEnd, "the far end is a BinModem carrying web traffic");
            }
            Report::Refused | Report::Reset | Report::Closed => {
                self.settle(Decided::Direct, "the far end has no proxy");
            }
            Report::Data | Report::Closing => {}
        }
    }

    fn settle(&mut self, decided: Decided, why: &str) {
        self.decided = decided;
        self.log.push(format!("proxy: {why}, so pages go {}", decided.name()));
        self.dispatch();
    }

    /// Send whatever has been waiting for the route the way it now goes.
    fn dispatch(&mut self) {
        for (socket, from) in std::mem::take(&mut self.held) {
            self.take(socket, from);
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
                    if matches!(self.decided, Decided::Asking(..)) {
                        self.held.push((socket, from));
                    } else {
                        self.take(socket, from);
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    /// One browser connection, sent the way the route goes.
    fn take(&mut self, socket: TcpStream, from: SocketAddr) {
        if self.decided == Decided::Direct {
            self.gateways.push(Gateway::new(socket, from));
            return;
        }
        let Some(handle) = self.stack.connect(self.server) else {
            // Nothing left to open one with. Dropping the socket closes it,
            // which tells the browser at once rather than leaving it waiting.
            self.log.push("proxy: too many connections, one was dropped".to_owned());
            return;
        };
        self.log.push(format!("proxy: {from} wants the far end"));
        self.relays.insert(
            handle,
            Relayed {
                socket,
                from,
                to_socket: Vec::new(),
                to_link: Vec::new(),
                socket_finished: false,
                told_socket: false,
                established: false,
                said: false,
                sent: 0,
                received: 0,
            },
        );
    }

    fn relay_event(&mut self, handle: Handle, report: Report) {
        match report {
            Report::Reset | Report::Refused | Report::Closed => {
                // Which of the three it was matters: refused is a far end
                // that is there and said no, reset is one that lost the
                // connection, and closed is the ordinary end of one.
                let why = match report {
                    Report::Refused => "refused",
                    Report::Reset => "reset",
                    _ => "closed",
                };
                // A close with data still to hand over is not the end of the
                // relay: `carry` finishes the job.
                if report == Report::Closed && self.relays.get(&handle).is_some_and(|r| !r.to_socket.is_empty()) {
                    return;
                }
                if let Some(relay) = self.relays.remove(&handle) {
                    self.log.push(if relay.established {
                        format!("proxy: a connection was {why}")
                    } else {
                        format!("proxy: a connection was {why} before the far end answered")
                    });
                }
            }
            Report::Established => {
                self.answered = true;
                if let Some(relay) = self.relays.get_mut(&handle) {
                    relay.established = true;
                }
            }
            Report::Data | Report::Closing => {}
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
        relay.received += from_link.len() as u64;
        self.to_browsers += from_link.len() as u64;
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

        if !gone && !forgotten && !relay.socket_finished && !too_much(relay.to_link.len()) {
            let mut buffer = [0u8; CHUNK];
            match relay.socket.read(&mut buffer) {
                Ok(0) => relay.socket_finished = true,
                Ok(n) => {
                    if !relay.said {
                        relay.said = true;
                        if !http::speaks_http(buffer[0]) {
                            let what = if buffer[0] == 5 { "is set up for SOCKS" } else { "is not speaking HTTP" };
                            self.log.push(format!(
                                "proxy: {} {what}; set it to use an HTTP proxy, for https as well",
                                relay.from
                            ));
                        }
                    }
                    relay.to_link.extend_from_slice(&buffer[..n]);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => gone = true,
            }
        }
        let finished = relay.socket_finished;
        let owed_to_socket = !relay.to_socket.is_empty();
        let mut to_link = std::mem::take(&mut relay.to_link);

        let mut still_waiting = false;
        let mut took = 0;
        if let Some(connection) = self.stack.get_mut(handle) {
            // Whatever the connection will take now; the rest waits rather
            // than being dropped, because the rest is the middle of a request.
            took = connection.send(&to_link);
            to_link.drain(..took);
            still_waiting = !to_link.is_empty();
        }
        self.from_browsers += took as u64;
        if let Some(relay) = self.relays.get_mut(&handle) {
            relay.sent += took as u64;
            relay.to_link = to_link;
        }
        if !still_waiting
            && (gone || finished)
            && let Some(connection) = self.stack.get_mut(handle)
        {
            connection.close();
        }
        // Both halves are over and nothing is owed either way. Dropping the
        // relay closes what is left of the socket, so it happens last of all.
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

    fn ticks(client: &mut Client, n: usize) {
        for _ in 0..n {
            client.tick(10);
            let _ = client.take_outgoing();
        }
    }

    /// Nothing is sent a way until the way is known: a browser that connects
    /// while the far end is being asked waits, and is counted as waiting.
    #[test]
    fn a_browser_waits_while_the_route_is_found_out() {
        let mut client = Client::new("127.0.0.1:0", [10, 0, 0, 2], [10, 0, 0, 1], 7).expect("could not listen");
        let mut browser = TcpStream::connect(client.bound()).expect("connect");
        let _ = browser.write_all(b"GET http://example.invalid/ HTTP/1.1\r\n\r\n");
        ticks(&mut client, 20);
        let view = client.view();
        assert_eq!(view.route, "finding out whether the far end has a proxy");
        assert_eq!(view.waiting, 1, "{view:?}");
        assert!(!client.direct());
        assert_eq!(view.carried.len(), 1, "no question was put to the far end: {view:?}");
        assert_eq!(view.carried[0].address, "10.0.0.1:1080");
    }

    /// A far end that never answers has no proxy, and the browser that waited
    /// is sent straight out.
    #[test]
    fn a_far_end_that_says_nothing_is_taken_to_have_no_proxy() {
        let mut client = Client::new("127.0.0.1:0", [10, 0, 0, 2], [10, 0, 0, 1], 7).expect("could not listen");
        let mut browser = TcpStream::connect(client.bound()).expect("connect");
        let _ = browser.write_all(b"GET http://192.0.2.9/ HTTP/1.1\r\n\r\n");
        ticks(&mut client, (PROBE_MS / 10 + 20) as usize);
        assert!(client.direct(), "{:?}", client.view().route);
        // And the page it asked for is being opened, to the address it named.
        let view = client.view();
        assert!(
            view.carried.iter().any(|c| c.address == "192.0.2.9:80" && c.name == "192.0.2.9:80"),
            "{view:?}"
        );
    }

    /// Told in advance, it asks nobody anything.
    #[test]
    fn a_route_given_in_advance_is_not_asked_about() {
        let client = Client::routed("127.0.0.1:0", [10, 0, 0, 2], [10, 0, 0, 1], 7, Route::Direct).expect("listen");
        assert!(client.direct());
        assert_eq!(client.view().asked, Route::Direct);
        assert!(client.view().carried.is_empty());
    }
}
