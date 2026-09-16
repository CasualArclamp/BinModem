//! The far half of the proxy, run on the machine that dialled.
//!
//! When the far end of the call is a provider's router, nothing over there
//! reads a proxy request. So this end reads it -- the same [`http::Session`]
//! the far BinModem uses -- and opens the connection itself: to the web
//! server's own address, over this end's TCP, through the router.
//!
//! One of these is one browser connection. Its other end is at most one TCP
//! connection over the link at a time, and which one can change: a browser
//! talking to a proxy is entitled to ask the same connection for a different
//! host next (RFC 9112 3.2.2), and the connection underneath follows it.
//!
//! What it does per round is what [`crate::server`] does, with the two sides
//! swapped: there the browser is across the link and the web server is a
//! socket, here the browser is a socket and the web server is across the
//! link.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};

use tcp::Endpoint;
use tcp::connection::Report;
use tcp::stack::{Handle, Stack};

use crate::resolve::{Answer, Resolver};
use crate::{CHUNK, too_much};

/// How long a connection may take to open before the browser is told it
/// would not. A SYN on a line with a second's round trip, resent at one, two
/// and four seconds, has had its chances by then.
const OPEN_WITHIN_MS: u32 = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Nothing asked for yet, or the last thing asked for is over.
    Idle,
    /// Waiting for the name.
    Resolving,
    /// A SYN is out, and this long has passed.
    Opening(u32),
    Open,
}

/// One browser connection and whatever it is connected to.
#[derive(Debug)]
pub struct Gateway {
    socket: TcpStream,
    from: SocketAddr,
    talk: http::Session,
    stage: Stage,
    /// The connection over the link, while there is one.
    upstream: Option<Handle>,
    /// Where it goes, as the browser named it, and where that turned out to
    /// be.
    going_to: String,
    address: Option<Endpoint>,
    to_upstream: Vec<u8>,
    to_browser: Vec<u8>,
    /// The browser has said it will send no more.
    browser_finished: bool,
    /// And has been told the web server will not either.
    told_browser: bool,
    /// The web server has been told the browser is finished.
    told_upstream: bool,
    /// The web server has sent all it will.
    upstream_finished: bool,
    /// The socket to the browser has failed.
    gone: bool,
    heard: bool,
    complained: bool,
    /// Octets each way, for the panel.
    pub sent: u64,
    pub received: u64,
}

impl Gateway {
    pub fn new(socket: TcpStream, from: SocketAddr) -> Self {
        Self {
            socket,
            from,
            talk: http::Session::new(),
            stage: Stage::Idle,
            upstream: None,
            going_to: String::new(),
            address: None,
            to_upstream: Vec::new(),
            to_browser: Vec::new(),
            browser_finished: false,
            told_browser: false,
            told_upstream: false,
            upstream_finished: false,
            gone: false,
            heard: false,
            complained: false,
            sent: 0,
            received: 0,
        }
    }

    /// The connection over the link this is using, if any.
    pub fn upstream(&self) -> Option<Handle> {
        self.upstream
    }

    /// What it is connected to, for the panel.
    pub fn going_to(&self) -> &str {
        &self.going_to
    }

    /// Whether the browser is waiting on a name or a connection.
    pub fn waiting(&self) -> bool {
        matches!(self.stage, Stage::Resolving | Stage::Opening(_))
    }

    /// Whether there is nothing left for this to do.
    pub fn over(&self) -> bool {
        if self.gone {
            return true;
        }
        let owed = !self.to_browser.is_empty();
        // The request could not be served, and the browser has been told why.
        let refused = self.talk.state() == http::State::Failed && self.stage == Stage::Idle;
        // Both ways are finished and nothing is still on its way.
        let finished = self.browser_finished
            && (self.upstream.is_none() || self.upstream_finished)
            && self.stage != Stage::Resolving
            && !matches!(self.stage, Stage::Opening(_));
        !owed && (refused || finished)
    }

    /// Something happened to the connection over the link.
    pub fn event(&mut self, report: Report, log: &mut Vec<String>) {
        match (self.stage, report) {
            (Stage::Opening(_), Report::Established) => {
                self.stage = Stage::Open;
                let early = self.talk.answer(http::Answer::Opened);
                self.to_upstream.extend(early);
                log.push(format!("proxy: {} open", self.going_to));
            }
            (Stage::Opening(_), Report::Refused | Report::Reset | Report::Closed) => {
                let (answer, word) = if report == Report::Refused {
                    (http::Answer::Refused, "refused")
                } else {
                    (http::Answer::Failed, "reset")
                };
                self.upstream = None;
                self.stage = Stage::Idle;
                let _ = self.talk.answer(answer);
                log.push(format!("proxy: {} {word} the connection", self.going_to));
            }
            (Stage::Open, Report::Reset) => {
                self.upstream = None;
                self.upstream_finished = true;
                log.push(format!("proxy: {} reset the connection", self.going_to));
            }
            // A close is not the end of what it carried: the stack keeps a
            // closed connection until what arrived on it has been read, and
            // `step` notices when it has let it go.
            _ => {}
        }
    }

    /// A router said the address could not be reached. Only worth acting on
    /// while still opening: once open, TCP has its own view.
    pub fn unreachable(&mut self, stack: &mut Stack, why: &str, log: &mut Vec<String>) {
        if let (Stage::Opening(_), Some(handle)) = (self.stage, self.upstream.take()) {
            if let Some(c) = stack.get_mut(handle) {
                c.abort();
            }
            self.stage = Stage::Idle;
            let _ = self.talk.answer(http::Answer::Unreachable);
            log.push(format!("proxy: {} could not be reached: {why}", self.going_to));
        }
    }

    /// Where this is trying to get to, if a router's complaint could be
    /// about it.
    pub fn opening_to(&self) -> Option<[u8; 4]> {
        match self.stage {
            Stage::Opening(_) => self.address.map(|a| a.address),
            _ => None,
        }
    }

    /// One round.
    pub fn step(&mut self, stack: &mut Stack, resolver: &mut Resolver, ms: u32, log: &mut Vec<String>) {
        self.read_browser(log);
        self.follow_request(stack, resolver, log);
        self.follow_lookup(stack, resolver, log);

        if let Stage::Opening(waited) = self.stage {
            let waited = waited.saturating_add(ms);
            self.stage = Stage::Opening(waited);
            if waited > OPEN_WITHIN_MS {
                if let Some(c) = self.upstream.take().and_then(|h| stack.get_mut(h)) {
                    c.abort();
                }
                self.stage = Stage::Idle;
                let _ = self.talk.answer(http::Answer::TimedOut);
                log.push(format!("proxy: {} did not answer in time", self.going_to));
            }
        }

        // What the proxy conversation has to say comes first -- a tunnel's
        // 200 goes ahead of anything through it -- then the web server.
        let said = self.talk.take_out();
        self.to_browser.extend(said);
        if let Some(handle) = self.upstream {
            match stack.get_mut(handle) {
                Some(c) => {
                    let data = c.take_received();
                    self.received += data.len() as u64;
                    self.to_browser.extend(data);
                    self.upstream_finished = c.finished() && c.available() == 0;
                }
                // Over, read to the end, and forgotten.
                None if self.stage == Stage::Open => {
                    self.upstream = None;
                    self.upstream_finished = true;
                }
                None => {}
            }
        }
        self.write_browser();

        if self.stage == Stage::Open
            && let Some(c) = self.upstream.and_then(|h| stack.get_mut(h))
        {
            let took = c.send(&self.to_upstream);
            self.sent += took as u64;
            self.to_upstream.drain(..took);
            // The browser has finished asking and all of it has gone: the web
            // server is told, or one waiting for the end of a request waits
            // for ever.
            if self.browser_finished && self.to_upstream.is_empty() && !self.told_upstream {
                self.told_upstream = true;
                c.close();
            }
        }

        // The web server has said everything and all of it has reached the
        // browser, which is told the same way a socket says it.
        if self.upstream_finished && self.to_browser.is_empty() && !self.told_browser {
            self.told_browser = true;
            let _ = self.socket.shutdown(std::net::Shutdown::Write);
        }
        // A browser that has gone takes its connection with it.
        if self.gone
            && let Some(c) = self.upstream.take().and_then(|h| stack.get_mut(h))
        {
            c.close();
        }
    }

    fn read_browser(&mut self, log: &mut Vec<String>) {
        if self.gone || self.browser_finished || too_much(self.to_upstream.len()) {
            return;
        }
        let mut buffer = [0u8; CHUNK];
        match self.socket.read(&mut buffer) {
            Ok(0) => self.browser_finished = true,
            Ok(n) => {
                if !self.heard {
                    self.heard = true;
                    if !http::speaks_http(buffer[0]) {
                        // RFC 1928 3 opens a SOCKS greeting with its version,
                        // 5, and a browser set up for SOCKS will get nowhere
                        // here -- so it is told once, plainly.
                        let what = if buffer[0] == 5 { "is set up for SOCKS" } else { "is not speaking HTTP" };
                        log.push(format!(
                            "proxy: {} {what}; set it to use an HTTP proxy, for https as well",
                            self.from
                        ));
                        self.gone = true;
                        return;
                    }
                }
                let forward = self.talk.feed(&buffer[..n]);
                self.to_upstream.extend(forward);
                if !self.complained
                    && let Some(why) = self.talk.trouble()
                {
                    self.complained = true;
                    log.push(format!("proxy: a request made no sense: {why}"));
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => self.gone = true,
        }
    }

    fn write_browser(&mut self) {
        while !self.to_browser.is_empty() && !self.gone {
            match self.socket.write(&self.to_browser) {
                Ok(0) => self.gone = true,
                Ok(n) => {
                    self.to_browser.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => self.gone = true,
            }
        }
    }

    /// Somewhere to go, and possibly somewhere other than where this is.
    fn follow_request(&mut self, stack: &mut Stack, resolver: &mut Resolver, log: &mut Vec<String>) {
        let Some(want) = self.talk.request() else { return };
        let moving = match self.stage {
            Stage::Idle => true,
            // What the old connection sent has to reach the browser first; the
            // move can wait a round or two for it.
            Stage::Open => want.authority != self.going_to && self.to_browser.is_empty(),
            Stage::Resolving | Stage::Opening(_) => false,
        };
        if !moving {
            return;
        }
        let authority = want.authority.clone();
        let tunnel = want.tunnel;
        if let Some(handle) = self.upstream.take()
            && let Some(c) = stack.get_mut(handle)
        {
            let tail = c.take_received();
            self.received += tail.len() as u64;
            self.to_browser.extend(tail);
            c.close();
            log.push(format!("proxy: {} is finished with", self.going_to));
        }
        self.upstream_finished = false;
        self.told_upstream = false;
        self.told_browser = false;
        self.going_to = authority;
        self.address = None;
        self.stage = Stage::Resolving;
        let how = if tunnel { " to tunnel through" } else { "" };
        log.push(format!("proxy: opening {}{how}", self.going_to));
        resolver.ask(&self.going_to);
    }

    fn follow_lookup(&mut self, stack: &mut Stack, resolver: &mut Resolver, log: &mut Vec<String>) {
        if self.stage != Stage::Resolving {
            return;
        }
        match resolver.answer(&self.going_to) {
            Answer::Pending => {}
            Answer::Found(endpoint) => match stack.connect(endpoint) {
                Some(handle) => {
                    self.upstream = Some(handle);
                    self.address = Some(endpoint);
                    self.stage = Stage::Opening(0);
                }
                None => {
                    self.stage = Stage::Idle;
                    let _ = self.talk.answer(http::Answer::Failed);
                    log.push(format!("proxy: {} not opened: too many connections", self.going_to));
                }
            },
            Answer::Failed(why) => {
                self.stage = Stage::Idle;
                let _ = self.talk.answer(http::Answer::Unreachable);
                log.push(format!("proxy: {} not opened: {why}", self.going_to));
            }
        }
    }
}
