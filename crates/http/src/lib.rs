//! The proxy half of HTTP/1.1: RFC 9112 for the syntax, RFC 9110 for what a
//! proxy is allowed to do with it.
//!
//! This is how a browser is carried over the link, for http and https alike,
//! and on a slow call it is the right way to do it. RFC 9112 3.2.2 has the
//! browser put the whole target in the request-line, so the first thing it
//! says is already the request, and the connection behind it can be opened
//! while it is still arriving. A protocol with a handshake of its own in front
//! would cost a round trip or two per connection first, and a round trip on
//! this line is more than a second.
//!
//! What this does not do is cache. RFC 9112 3.2.2 offers the proxy that
//! choice -- "service that request from a valid cache, if possible, or make
//! the same request on the client's behalf" -- and the second is the whole of
//! what happens here.
//!
//! Whoever drives it does the same four things, whichever end of the call it
//! is on: feed it what the browser sent, ask what it wants opened, tell it
//! what happened, take what it has to say.

use std::fmt;

/// How much of a request head to hold before deciding there is no end to it.
///
/// A real one is a few hundred octets; a few kilobytes of cookies is not
/// unusual. Past this, something is wrong and holding more of it only makes
/// the wrongness larger.
const MOST_HEAD: usize = 64 * 1024;

/// Where a request wants to go, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Host and port, as the request-target gave them.
    pub authority: String,
    /// A CONNECT: RFC 9112 3.2.3 and RFC 9110 9.3.6. Nothing above this end
    /// reads what crosses after it -- which is the point of it, since what
    /// crosses is TLS.
    pub tunnel: bool,
}

/// What became of the attempt to open it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Opened,
    /// The far side said no.
    Refused,
    /// No such name, or nothing at that address.
    Unreachable,
    /// It never answered in time.
    TimedOut,
    /// Anything else.
    Failed,
}

/// What the conversation is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Reading a request head.
    Reading,
    /// A head has been read and its socket is being opened.
    Waiting,
    /// Passing a request body through.
    Sending,
    /// A tunnel: everything either way belongs to it.
    Tunnelling,
    /// Nothing more will be understood.
    Failed,
}

/// Where the body of the request being forwarded has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    /// RFC 9112 6.3 item 7: a request with neither framing field has none.
    None,
    /// Item 6: a Content-Length, and this many octets of it still to come.
    Fixed(u64),
    /// Item 4: chunked, which has to be walked through to find its end.
    Chunked(Chunk),
}

/// The chunked transfer coding of RFC 9112 7.1, as something to be passed
/// through rather than decoded.
///
/// Nothing here wants the content -- it goes to the origin server exactly as
/// it arrived. What it wants is the one thing only the framing can say: where
/// this request stops and the browser's next one starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chunk {
    /// Reading `chunk-size [ chunk-ext ] CRLF`.
    Size,
    /// Reading `chunk-data`, with this much of it left.
    Data(u64),
    /// The CRLF that follows chunk-data.
    Ending,
    /// After the last chunk: trailer fields until an empty line.
    Trailer,
}

/// One browser connection, as the far end sees it.
pub struct Session {
    state: State,
    /// What has come from the browser and not been dealt with.
    pending: Vec<u8>,
    /// A rewritten head whose socket is not open yet.
    parked: Vec<u8>,
    /// Waiting to go back to the browser.
    out: Vec<u8>,
    /// Where the open socket goes, so a second request to the same place does
    /// not open a second one. This is the whole reason a browser is allowed to
    /// keep the connection: RFC 9110 7.6.1's persistence, spent on the link
    /// where it is worth the most.
    open_to: Option<String>,
    /// A target the far end has not opened yet.
    wanted: Option<Target>,
    /// What the head that is parked said about its body.
    body: Body,
    trouble: Option<&'static str>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("state", &self.state)
            .field("open_to", &self.open_to)
            .field("wanted", &self.wanted)
            .finish_non_exhaustive()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: State::Reading,
            pending: Vec::new(),
            parked: Vec::new(),
            out: Vec::new(),
            open_to: None,
            wanted: None,
            body: Body::None,
            trouble: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// A target that needs a socket, if one does.
    pub fn request(&self) -> Option<&Target> {
        self.wanted.as_ref()
    }

    /// Whether a socket is open and being used.
    pub fn open(&self) -> bool {
        self.open_to.is_some()
    }

    /// What went wrong, if anything did.
    pub fn trouble(&self) -> Option<&'static str> {
        self.trouble
    }

    /// Octets for the browser: a tunnel being agreed to, or a refusal.
    pub fn take_out(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Octets from the browser. Returns what should go to the origin server.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        self.work()
    }

    /// What became of the socket this end asked for. Returns anything that was
    /// waiting on it.
    pub fn answer(&mut self, answer: Answer) -> Vec<u8> {
        let Some(target) = self.wanted.take() else {
            return Vec::new();
        };
        if answer != Answer::Opened {
            // The browser is told in HTTP, which is what makes it show a page
            // saying what happened rather than an empty one. RFC 9110 15.6.3
            // is the gateway's own failure and 15.6.5 is the one where the far
            // side never answered.
            let (status, why) = match answer {
                Answer::Refused => (502, "the far end refused the connection"),
                Answer::Unreachable => (502, "the far end could not be reached"),
                Answer::TimedOut => (504, "the far end did not answer in time"),
                Answer::Opened | Answer::Failed => (502, "the connection could not be opened"),
            };
            self.reply(status, why);
            self.state = State::Failed;
            self.trouble = Some("the far end would not open");
            return Vec::new();
        }
        self.open_to = Some(target.authority);
        if target.tunnel {
            // RFC 9110 9.3.6: a 2xx says the tunnel is up, and RFC 9112 6.3
            // item 2 says everything after the blank line belongs to it.
            self.out
                .extend_from_slice(b"HTTP/1.1 200 Connection established\r\n\r\n");
            self.state = State::Tunnelling;
            return self.work();
        }
        let mut first = std::mem::take(&mut self.parked);
        self.state = if self.body == Body::None { State::Reading } else { State::Sending };
        first.extend(self.work());
        first
    }

    /// Everything that can be done with what is in hand.
    fn work(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            match self.state {
                State::Failed | State::Waiting => break,
                State::Tunnelling => {
                    out.append(&mut self.pending);
                    break;
                }
                State::Reading => {
                    if !self.read_head(&mut out) {
                        break;
                    }
                }
                State::Sending => {
                    if !self.pass_body(&mut out) {
                        break;
                    }
                }
            }
        }
        out
    }

    /// Take one request head, if a whole one is here.
    ///
    /// Returns whether there is any point going round again.
    fn read_head(&mut self, out: &mut Vec<u8>) -> bool {
        let Some(end) = head_end(&self.pending) else {
            if self.pending.len() > MOST_HEAD {
                self.fail(400, "a request head with no end to it");
            }
            return false;
        };
        let head: Vec<u8> = self.pending.drain(..end).collect();
        let Some(parsed) = parse(&head) else {
            self.fail(400, "a request this end could not read");
            return false;
        };
        self.body = parsed.body;
        if parsed.target.tunnel {
            // Nothing is forwarded: the tunnel starts when the socket opens.
            self.parked.clear();
            self.wanted = Some(parsed.target);
            self.state = State::Waiting;
            return false;
        }
        // A second request to the same place goes down the socket that is
        // already there, which is the saving this whole module exists for.
        if self.open_to.as_deref() == Some(parsed.target.authority.as_str()) {
            out.extend_from_slice(&parsed.head);
            self.state = if self.body == Body::None { State::Reading } else { State::Sending };
            return true;
        }
        // Somewhere else, or nowhere yet. A browser talking to a proxy is
        // entitled to change host on the same connection, so this is ordinary
        // rather than exceptional -- and it is why the head has to wait here
        // instead of going out: it belongs to a socket that does not exist.
        self.parked = parsed.head;
        self.wanted = Some(parsed.target);
        self.state = State::Waiting;
        false
    }

    /// Move as much of the body through as has arrived.
    fn pass_body(&mut self, out: &mut Vec<u8>) -> bool {
        match self.body {
            Body::None => {
                self.state = State::Reading;
                true
            }
            Body::Fixed(left) => {
                let take = left.min(self.pending.len() as u64) as usize;
                out.extend(self.pending.drain(..take));
                let left = left - take as u64;
                self.body = Body::Fixed(left);
                if left == 0 {
                    self.state = State::Reading;
                    return true;
                }
                false
            }
            Body::Chunked(chunk) => self.pass_chunk(chunk, out),
        }
    }

    /// One step of RFC 9112 7.1, passed through rather than decoded.
    fn pass_chunk(&mut self, chunk: Chunk, out: &mut Vec<u8>) -> bool {
        match chunk {
            Chunk::Size => {
                let Some(end) = line_end(&self.pending) else {
                    if self.pending.len() > MOST_HEAD {
                        self.fail(400, "a chunk size with no end to it");
                    }
                    return false;
                };
                let line: Vec<u8> = self.pending.drain(..end).collect();
                // "chunk-size = 1*HEXDIG", with an optional chunk-ext after a
                // semicolon which nothing here needs to understand.
                let digits: &[u8] = line
                    .split(|&b| b == b';')
                    .next()
                    .unwrap_or(&line);
                let text = String::from_utf8_lossy(digits);
                let Ok(size) = u64::from_str_radix(text.trim(), 16) else {
                    self.fail(400, "a chunk size that is not a number");
                    return false;
                };
                out.extend_from_slice(&line);
                // 7.1: "complete when a chunk with a chunk-size of zero is
                // received, possibly followed by a trailer section, and
                // finally terminated by an empty line".
                self.body =
                    Body::Chunked(if size == 0 { Chunk::Trailer } else { Chunk::Data(size) });
                true
            }
            Chunk::Data(left) => {
                let take = left.min(self.pending.len() as u64) as usize;
                out.extend(self.pending.drain(..take));
                let left = left - take as u64;
                self.body = Body::Chunked(if left == 0 { Chunk::Ending } else { Chunk::Data(left) });
                left == 0
            }
            Chunk::Ending => {
                let Some(end) = line_end(&self.pending) else {
                    return false;
                };
                out.extend(self.pending.drain(..end));
                self.body = Body::Chunked(Chunk::Size);
                true
            }
            Chunk::Trailer => {
                let Some(end) = line_end(&self.pending) else {
                    return false;
                };
                let line: Vec<u8> = self.pending.drain(..end).collect();
                let blank = line.iter().all(|b| matches!(b, b'\r' | b'\n'));
                out.extend_from_slice(&line);
                if blank {
                    self.body = Body::None;
                    self.state = State::Reading;
                }
                true
            }
        }
    }

    /// Tell the browser, and stop.
    fn fail(&mut self, status: u16, why: &'static str) {
        self.reply(status, why);
        self.state = State::Failed;
        self.trouble = Some(why);
        self.pending.clear();
    }

    /// A short response of this end's own, which is the only kind it writes.
    fn reply(&mut self, status: u16, why: &str) {
        let reason = match status {
            400 => "Bad Request",
            504 => "Gateway Timeout",
            _ => "Bad Gateway",
        };
        let body = format!("{reason}: {why}.\r\n");
        self.out.extend_from_slice(
            format!(
                "HTTP/1.1 {status} {reason}\r\n\
                 Content-Type: text/plain; charset=utf-8\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
    }
}

/// A request head, read and made fit to forward.
#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    target: Target,
    /// The head as it should reach the origin server.
    head: Vec<u8>,
    body: Body,
}

/// Where a head ends: the first empty line (RFC 9112 2.1).
fn head_end(bytes: &[u8]) -> Option<usize> {
    // Both endings are accepted on the way in. 2.2 asks a recipient to accept
    // a bare LF as a line terminator, which some clients still send.
    let crlf = find(bytes, b"\r\n\r\n").map(|i| i + 4);
    let lf = find(bytes, b"\n\n").map(|i| i + 2);
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Where one line ends, terminator included.
fn line_end(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| b == b'\n').map(|i| i + 1)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Read a request head and write the one to forward.
fn parse(head: &[u8]) -> Option<Parsed> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split('\n');
    let request_line = lines.next()?.trim_end_matches('\r');
    // "request-line = method SP request-target SP HTTP-version" (9112 3).
    let mut parts = request_line.split(' ');
    let method = parts.next()?;
    let request_target = parts.next()?;
    let version = parts.next()?;
    if method.is_empty() || !version.starts_with("HTTP/") {
        return None;
    }

    if method == "CONNECT" {
        // 3.2.3 authority-form: "only the uri-host and port number of the
        // tunnel destination, separated by a colon". A port is not optional
        // here, so a target without one is not a CONNECT this end can serve.
        let authority = request_target.to_owned();
        if !authority.contains(':') {
            return None;
        }
        return Some(Parsed {
            target: Target { authority, tunnel: true },
            head: Vec::new(),
            body: Body::None,
        });
    }

    let (authority, path) = split_absolute(request_target)?;

    let mut out = format!("{method} {path} {version}\r\n");
    let mut length: Option<u64> = None;
    let mut chunked = false;
    // Fields the Connection field names are for this hop only and go no
    // further (RFC 9110 7.6.1).
    let named = connection_options(&text);

    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            // 3.2.2: "the proxy MUST ignore the received Host header field (if
            // any) and instead replace it with the host information of the
            // request-target".
            "host" => continue,
            // 7.6.1's own list of what an intermediary removes whether or not
            // the Connection field named it.
            "proxy-connection" | "keep-alive" | "te" | "upgrade" | "connection"
            | "proxy-authorization" | "proxy-authenticate" => continue,
            // Held back rather than written out. Item 3 has an intermediary
            // forwarding a message with both framings "first remove the
            // received Content-Length field", and whether there is a
            // Transfer-Encoding as well is not known until the head is done.
            "content-length" => {
                // Item 5: invalid framing is unrecoverable.
                length = Some(value.trim().parse::<u64>().ok()?);
                continue;
            }
            "transfer-encoding" => {
                // 6.3 item 4. The coding is kept rather than removed: this end
                // forwards the body exactly as it arrives, so the field that
                // frames it has to go with it.
                chunked = value.to_ascii_lowercase().contains("chunked");
            }
            _ => {}
        }
        if named.iter().any(|option| option == &lower) {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }

    let framing = if chunked {
        // Item 3: the Transfer-Encoding overrides the Content-Length, and the
        // Content-Length is the one that goes. It was never written out, so
        // that is already done.
        Body::Chunked(Chunk::Size)
    } else if let Some(n) = length {
        // Put back, now that it is known to be the only framing there is.
        out.push_str(&format!("Content-Length: {n}\r\n"));
        if n > 0 { Body::Fixed(n) } else { Body::None }
    } else {
        // Item 7: a request with neither has no body at all.
        Body::None
    };

    // The replacement Host, from the request-target, as 3.2.2 requires.
    let head = format!("{out}Host: {authority}\r\n\r\n").into_bytes();
    Some(Parsed {
        target: Target { authority, tunnel: false },
        head,
        body: framing,
    })
}

/// Split an absolute-form request-target into its authority and its path.
///
/// RFC 9112 3.2.2: a client "MUST send the target URI in absolute-form" to a
/// proxy. Anything else arriving here is a browser that thinks this is an
/// origin server, and there is nothing to be done with it.
fn split_absolute(target: &str) -> Option<(String, String)> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("HTTP://"))?;
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(cut);
    // Userinfo is not sent to the origin server.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return None;
    }
    let authority = if authority.contains(':') {
        authority.to_owned()
    } else {
        // No port: http is 80.
        format!("{authority}:80")
    };
    // 3.2.1: "if the path component is empty, the client MUST send / as the
    // path" -- and a proxy writing the origin-form owes the same.
    let path = if path.is_empty() { "/".to_owned() } else { path.to_owned() };
    Some((authority, path))
}

/// The field names a Connection header field lists (RFC 9110 7.6.1).
fn connection_options(head: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in head.split('\n').skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if name == "connection" || name == "proxy-connection" {
            for option in value.split(',') {
                out.push(option.trim().to_ascii_lowercase());
            }
        }
    }
    out
}

/// Whether the first octet of a connection is a browser speaking HTTP.
///
/// Every HTTP method is uppercase letters (RFC 9110 9), so one octet says. The
/// one worth recognising as not HTTP is a browser still set up for SOCKS,
/// whose greeting opens with 5 (RFC 1928 3) and which is otherwise left
/// waiting for an answer that never comes.
pub fn speaks_http(first: u8) -> bool {
    first.is_ascii_uppercase()
}

#[cfg(test)]
mod tests;
