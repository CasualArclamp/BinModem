//! The link to the next hop: RFC 3261 18, and the two transports under it.
//!
//! Everything above this file is dialogs, transactions and timers, and none of
//! that changes with the transport. Three things do, and they are the three
//! things this module exists to keep in one place:
//!
//! - **Framing (7.5).** Over UDP a datagram is exactly one message and there is
//!   nothing to decide. Over TCP there are no edges: a message is found by
//!   reading to the end of the headers and then taking exactly Content-Length
//!   octets of body. One read may hold two messages, or a third of one, and
//!   7.5 is explicit that Content-Length is mandatory over a stream -- without
//!   it there is no way to say where a message stops, and so no way to find
//!   the one after it.
//! - **Whether the protocol retransmits (17.1.1.2, 17.1.2.2).** Over UDP
//!   nothing underneath will try again, so the transaction layer does. Over TCP
//!   something underneath already has, and a request sent twice is a duplicate
//!   the far end has to disentangle for no reason at all. [`Link::reliable`] is
//!   what the timers ask.
//! - **What the Via says (18.1.1)**, which is [`Link::transport`]'s job to
//!   answer and the agent's to write.
//!
//! # Reading without stopping the clock
//!
//! The agent is a set of timers serviced by one thread, and that thread wakes
//! every 20 ms because the UDP socket's read timeout says so. TCP keeps the
//! same arrangement: the stream carries the same read timeout, and a read that
//! finds nothing returns to the timers just as an empty datagram socket does.
//!
//! The other shape -- a reader thread feeding a queue, as [`crate::media`] is
//! built -- would work too, and was not chosen for two reasons. The framing
//! state and the octets it is scanning would then live behind a mutex, on the
//! far side of a queue from the only code that understands them; and a queue
//! that is never empty would have to be paced by something other than the read
//! itself, which is the one thing that currently guarantees the timers are
//! looked at. A thread buys concurrency that a single call at a time, on one
//! connection, has no use for.
//!
//! The media path is not affected by any of this. RTP is UDP whatever SIP
//! travels over, which is [`crate::media`]'s business and stays there.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use crate::account::Transport;
use crate::message::Message;

/// How long a quiet link waits before it hands control back.
///
/// Every timer in the agent is serviced on this cadence, so it is the same
/// figure over either transport: 17.1.1.2's shortest interval is T1, half a
/// second, and a loop that wakes twenty-five times inside one of those is
/// prompt enough for anything in the protocol.
pub const WAKE: Duration = Duration::from_millis(20);

/// How long to wait for a connection to the trunk to come up.
///
/// It is a stall in the timer loop, which is why it is short. A trunk on the
/// far side of this line answers a SYN in well under this; one that does not
/// is down, and the answer that matters then is the quick one.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(400);

/// And how often it is worth trying again. A trunk that is refusing
/// connections must not be asked twenty-five times a second, and a person
/// watching the log should see something that reads like a retry rather than a
/// fault.
const RETRY_EVERY: Duration = Duration::from_secs(1);

/// A write that has not completed in this long is a far end that has stopped
/// reading. Worth giving up on, because the alternative is a timer loop that
/// has stopped as well.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// The largest body this will assemble from a stream. Nothing SIP sends comes
/// near it; what it is here for is a far end that declares a Content-Length of
/// four gigabytes, which must not become four gigabytes of ours.
const MAX_BODY: usize = 128 * 1024;

/// The most a set of headers may run to before the conclusion is that the
/// blank line is never coming.
const MAX_HEADERS: usize = 16 * 1024;

/// And the most a start line may run to. 7.1's request line is a method, a URI
/// and a version; anything of this length is something else.
const MAX_START_LINE: usize = 1024;

/// One connection, or one socket, and the framing that goes with it.
///
/// The agent holds one of these and asks it for two things: put these octets
/// on the wire, and give me the next message that has arrived, or nothing.
#[derive(Debug)]
pub enum Link {
    Udp(UdpLink),
    Tcp(TcpLink),
}

impl Link {
    /// Open the link an account asks for.
    ///
    /// The TCP connection is attempted here rather than left entirely to the
    /// first send, so that the first request out carries a sent-by with a real
    /// port in it. A failure is not fatal: a trunk that is down at start-up is
    /// an ordinary Tuesday, and the link retries as soon as anything is sent
    /// or read.
    pub fn open(
        transport: Transport,
        local_port: u16,
        hop: SocketAddr,
    ) -> Result<Self, String> {
        match transport {
            Transport::Udp => {
                let socket = UdpSocket::bind(("0.0.0.0", local_port))
                    .map_err(|e| format!("could not bind the SIP port: {e}"))?;
                socket
                    .set_read_timeout(Some(WAKE))
                    .map_err(|e| format!("could not set a read timeout: {e}"))?;
                // The port the system chose, when the account left it to the
                // system: it is the one the far end will answer to.
                let port = socket.local_addr().map_or(local_port, |a| a.port());
                Ok(Self::Udp(UdpLink::new(socket, routed_source(hop, port)?)))
            }
            Transport::Tcp => {
                // std cannot bind a source port for an outgoing connection, so
                // `local_port` is not honoured over TCP, and 5060 stands in
                // until there is a connection to read the real one off. It
                // costs nothing that matters: 18.2.1 sends every response back
                // over the connection the request arrived on, so the port in
                // our Via is read by nobody, and there is no listener on it
                // either way.
                let port = if local_port == 0 { 5060 } else { local_port };
                let mut link = TcpLink::new(hop, routed_source(hop, port)?);
                if let Err(e) = link.connect() {
                    link.notes
                        .push(format!("the trunk is not answering on TCP yet: {e}"));
                }
                Ok(Self::Tcp(link))
            }
        }
    }

    /// A link over an already-bound socket, for the tests that need to be the
    /// far end as well as this end.
    #[cfg(test)]
    pub fn on_socket(socket: UdpSocket, local: SocketAddr) -> Self {
        Self::Udp(UdpLink::new(socket, local))
    }

    pub fn transport(&self) -> Transport {
        match self {
            Self::Udp(_) => Transport::Udp,
            Self::Tcp(_) => Transport::Tcp,
        }
    }

    /// Whether the transport underneath delivers what it accepts.
    ///
    /// 17.1.1.2 and 17.1.2.2: a client transaction over a reliable transport
    /// does not run Timer A or Timer E at all, and 13.3.1.4's retransmission of
    /// a 2xx is the same rule seen from the other end. Timer B and Timer F --
    /// the 64*T1 give-up -- run whatever this says, because a far end that
    /// never answers is not a transport problem.
    pub fn reliable(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    /// Where our messages appear to come from: what goes in a Via's sent-by
    /// and a Contact (18.1.1).
    pub fn local(&self) -> SocketAddr {
        match self {
            Self::Udp(link) => link.local,
            Self::Tcp(link) => link.local(),
        }
    }

    /// Put these octets on the wire.
    ///
    /// Over TCP the address is where they were always going: there is one
    /// connection, to the next hop, and 18.2.1 has a response go back over the
    /// connection its request arrived on, which is that one.
    pub fn send(&mut self, bytes: &[u8], to: SocketAddr) -> io::Result<()> {
        match self {
            Self::Udp(link) => link.socket.send_to(bytes, to).map(|_| ()),
            Self::Tcp(link) => link.send(bytes),
        }
    }

    /// The next message that has arrived, or nothing.
    ///
    /// Waits at most [`WAKE`] for one, so that a caller can treat this as the
    /// pacing of its loop. When several have arrived together it returns them
    /// one to a call and does not wait at all, which is what keeps a busy
    /// moment from being spread over a call each 20 ms.
    pub fn receive(&mut self) -> Option<(Message, SocketAddr)> {
        match self {
            Self::Udp(link) => link.receive(),
            Self::Tcp(link) => link.receive(),
        }
    }

    /// Anything worth a line in the transcript, taken.
    pub fn notes(&mut self) -> Vec<String> {
        let notes = match self {
            Self::Udp(link) => &mut link.notes,
            Self::Tcp(link) => &mut link.notes,
        };
        std::mem::take(notes)
    }

    /// Whether the connection has been remade since this was last asked.
    ///
    /// Never true over UDP, which has no connection to lose. Over TCP it is
    /// the one thing above this file that has to know about the transport,
    /// because a connection that broke took the delivery guarantee with it --
    /// see `Worker::connection_remade`.
    pub fn remade(&mut self) -> bool {
        match self {
            Self::Udp(_) => false,
            Self::Tcp(link) => std::mem::take(&mut link.remade),
        }
    }
}

/// The datagram case: unchanged, and the one the timers were written around.
#[derive(Debug)]
pub struct UdpLink {
    socket: UdpSocket,
    local: SocketAddr,
    buffer: Vec<u8>,
    notes: Vec<String>,
}

impl UdpLink {
    fn new(socket: UdpSocket, local: SocketAddr) -> Self {
        Self {
            socket,
            local,
            buffer: vec![0u8; 8192],
            notes: Vec::new(),
        }
    }

    fn receive(&mut self) -> Option<(Message, SocketAddr)> {
        match self.socket.recv_from(&mut self.buffer) {
            // 7.5: one datagram, one message, and Content-Length is trusted
            // only as far as the datagram goes.
            //
            // A datagram that will not parse is dropped rather than answered:
            // 3261 has nothing to say back to something it cannot read, and
            // the usual sender is a scanner.
            Ok((n, from)) => Message::parse(&self.buffer[..n]).map(|message| (message, from)),
            Err(e) if would_block(&e) => None,
            // Windows reports an ICMP port-unreachable from an earlier datagram
            // as a ConnectionReset on the *next* read of a connectionless
            // socket. It says nothing about this socket's health, and the read
            // after it is usually fine.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => None,
            Err(e) => {
                self.notes.push(format!("socket error: {e}"));
                // The read returned without waiting, so the wait it would have
                // done is done here instead: a socket that fails instantly must
                // not turn the agent's loop into a spin.
                thread::sleep(WAKE);
                None
            }
        }
    }
}

/// The stream case: one connection to the next hop, and the octets that have
/// arrived on it but are not yet a whole message.
#[derive(Debug)]
pub struct TcpLink {
    hop: SocketAddr,
    /// What to call ourselves before there is a connection to ask.
    fallback: SocketAddr,
    stream: Option<TcpStream>,
    /// 7.5's problem, in a field: what has arrived and not yet been framed.
    buffer: Vec<u8>,
    /// Not before this. A trunk that is refusing connections is asked once a
    /// second and not oftener.
    next_attempt: Instant,
    /// Whether a connection has ever been up, so that the first one is not
    /// reported as a reconnection.
    had_one: bool,
    remade: bool,
    notes: Vec<String>,
}

impl TcpLink {
    fn new(hop: SocketAddr, fallback: SocketAddr) -> Self {
        Self {
            hop,
            fallback,
            stream: None,
            buffer: Vec::new(),
            next_attempt: Instant::now(),
            had_one: false,
            remade: false,
            notes: Vec::new(),
        }
    }

    fn local(&self) -> SocketAddr {
        self.stream
            .as_ref()
            .and_then(|s| s.local_addr().ok())
            .unwrap_or(self.fallback)
    }

    /// Make sure there is a connection, or say why there is not.
    fn connect(&mut self) -> io::Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let now = Instant::now();
        if now < self.next_attempt {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the connection to the trunk is down",
            ));
        }
        self.next_attempt = now + RETRY_EVERY;
        let stream = TcpStream::connect_timeout(&self.hop, CONNECT_TIMEOUT)?;
        // The read timeout is what keeps the agent's timers on their cadence;
        // the write timeout is what stops a far end that has stopped reading
        // from stopping them instead.
        stream.set_read_timeout(Some(WAKE))?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        // A SIP message is a whole thought and wants to leave now. Nagle would
        // hold the last part of one back waiting for more to say, which on a
        // line that carries 750 ms each way is half a second nobody has.
        stream.set_nodelay(true)?;
        // Whatever was half-read belonged to the connection that went.
        self.buffer.clear();
        if self.had_one {
            self.remade = true;
            self.notes
                .push("the connection to the trunk was remade".to_owned());
        }
        self.had_one = true;
        self.stream = Some(stream);
        Ok(())
    }

    /// Let go of the connection, and say why.
    fn drop_connection(&mut self, why: String) {
        if self.stream.take().is_some() {
            self.notes.push(why);
        }
        // Half a message is worth nothing without the connection it was
        // arriving on: the rest of it is never coming, and keeping it would
        // put the next connection's first message out of step.
        self.buffer.clear();
    }

    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.connect()?;
        let written = match self.stream.as_ref() {
            Some(stream) => {
                let mut writer = stream;
                writer.write_all(bytes)
            }
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the connection to the trunk is down",
            )),
        };
        if let Err(e) = written {
            // A message half written leaves the stream out of step: the far
            // end has no way of telling where the next one starts, and 7.5
            // gives it nothing to resynchronise on. So the connection goes
            // rather than the framing.
            self.drop_connection(format!("a message could not be sent: {e}"));
            return Err(e);
        }
        Ok(())
    }

    fn receive(&mut self) -> Option<(Message, SocketAddr)> {
        let until = Instant::now() + WAKE;
        let mut chunk = [0u8; 4096];
        loop {
            // Whatever is already here first. Several messages arriving in one
            // read is ordinary on a busy trunk, and the second of them must not
            // wait for a third to turn up.
            match frame(&self.buffer) {
                Frame::Skip(n) => {
                    self.buffer.drain(..n);
                    continue;
                }
                Frame::Whole(n) => {
                    let message = Message::parse(&self.buffer[..n]);
                    self.buffer.drain(..n);
                    match message {
                        Some(message) => return Some((message, self.hop)),
                        // The framing was sound and the content was not. The
                        // stream is still in step, so this costs one message
                        // rather than the connection.
                        None => continue,
                    }
                }
                Frame::Broken(why) => {
                    self.drop_connection(format!("{why}; the connection was closed"));
                    return None;
                }
                Frame::Incomplete => {}
            }

            if Instant::now() >= until {
                // The timers are owed a look. What is in the buffer stays
                // there and is picked up on the next call.
                return None;
            }
            if self.stream.is_none() {
                // Reconnecting here as well as on the way out: a trunk sends
                // an inbound call over the connection it has, and an agent
                // that only connected when it had something to say would not
                // have one to send it over.
                if self.connect().is_err() {
                    thread::sleep(WAKE);
                    return None;
                }
            }
            let read = {
                // Borrowed only for as long as the read takes: what is done
                // about a failure needs the whole of `self` back.
                let mut reader = self.stream.as_ref()?;
                reader.read(&mut chunk)
            };
            match read {
                Ok(0) => {
                    // 18.3: a trunk closing an idle connection is ordinary, and
                    // so is one closing it because it has been restarted. The
                    // next thing to be sent opens another.
                    self.drop_connection("the trunk closed the connection".to_owned());
                    return None;
                }
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) if would_block(&e) => return None,
                Err(e) => {
                    self.drop_connection(format!("the connection failed: {e}"));
                    return None;
                }
            }
        }
    }
}

/// How much of the front of a buffer is one message: 7.5's question, and the
/// whole of the difference a stream transport makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// Not all of a message is here yet. Read more and ask again.
    Incomplete,
    /// This many octets at the front belong to no message at all. 7.5 has a
    /// receiver ignore any CRLF before a start line, and RFC 5626 3.5.1 makes
    /// a doubled one a keep-alive that several trunks send every half minute.
    Skip(usize),
    /// One whole message: this many octets from the front.
    Whole(usize),
    /// The stream cannot be made sense of. There is no framing left to find
    /// the next message by, so the only answer is to close the connection --
    /// which is what 7.5 says to do about the one case that actually happens,
    /// a message with no Content-Length on it.
    Broken(&'static str),
}

/// Where the message at the front of `buffer` ends, if it can be told yet.
///
/// Pure: it reads a buffer and says what is in it, and that is deliberate.
/// Framing is where a stream transport is got wrong, the faults are the ones
/// that only show up when the line is busy -- two messages in one read, a body
/// arriving after its headers -- and a function that needs a socket to be
/// tested is a function those cases do not get tested on.
pub fn frame(buffer: &[u8]) -> Frame {
    let stray = buffer
        .iter()
        .take_while(|b| **b == b'\r' || **b == b'\n')
        .count();
    if stray > 0 {
        return Frame::Skip(stray);
    }
    if let Err(why) = start_line_is_plausible(buffer) {
        return Frame::Broken(why);
    }
    let Some((head_end, body_start)) = header_end(buffer) else {
        return if buffer.len() > MAX_HEADERS {
            Frame::Broken("the headers went on past anything a SIP message has in it")
        } else {
            Frame::Incomplete
        };
    };
    let Some(length) = declared_length(&buffer[..head_end]) else {
        // 7.5: a stream-based transport MUST use Content-Length, and a request
        // that arrives over one without it is an error. It has to be: the body
        // runs to wherever the far end decided, so not only is this message
        // unreadable, every message after it on this connection is too.
        //
        // 3261 has a server answer 400 and close the connection. Only the
        // closing is done here: an answer would have to be built from headers
        // that have already proved untrustworthy, and sent over a connection
        // that is about to go anyway.
        return Frame::Broken("a message arrived over TCP with no Content-Length (7.5)");
    };
    if length > MAX_BODY {
        return Frame::Broken(
            "the Content-Length is larger than any SIP message this will assemble",
        );
    }
    match body_start.checked_add(length) {
        Some(end) if buffer.len() >= end => Frame::Whole(end),
        _ => Frame::Incomplete,
    }
}

/// The blank line that ends the headers: where they stop, and where the body
/// starts. CRLF CRLF by the letter of 7, and LF LF as well, because a far end
/// that writes bare line feeds is not worth refusing -- [`Message::parse`]
/// accepts both and this has to agree with it or the two would disagree about
/// where the body began.
fn header_end(buffer: &[u8]) -> Option<(usize, usize)> {
    for at in 0..buffer.len() {
        if buffer[at..].starts_with(b"\r\n\r\n") {
            return Some((at, at + 4));
        }
        if buffer[at..].starts_with(b"\n\n") {
            return Some((at, at + 2));
        }
    }
    None
}

/// The Content-Length a header block declares, in 7.3.3's compact form as
/// well. None when there is none, or when what is there is not a number.
///
/// Read out of the octets rather than out of a parsed message, for two
/// reasons: the message cannot be parsed until this has said where it ends,
/// and a far end that puts a Latin-1 display name in a From should cost one
/// unreadable message rather than a connection that can no longer be framed.
fn declared_length(head: &[u8]) -> Option<usize> {
    let mut lines = head.split(|b| *b == b'\n').map(strip_cr).peekable();
    // The start line is not a header, and a request line has a colon in it.
    lines.next()?;
    while let Some(line) = lines.next() {
        // A line beginning with whitespace is 7.3.1's continuation of the one
        // before, not a header of its own, whatever it has a colon in.
        if line.first().is_some_and(|b| *b == b' ' || *b == b'\t') {
            continue;
        }
        let Some(colon) = line.iter().position(|b| *b == b':') else {
            continue;
        };
        let name = trim(&line[..colon]);
        if !name.eq_ignore_ascii_case(b"content-length") && !name.eq_ignore_ascii_case(b"l") {
            continue;
        }
        let mut value = line[colon + 1..].to_vec();
        // 7.3.1 again: the value may be folded onto the lines that follow, and
        // a fold means a single space.
        while lines
            .peek()
            .is_some_and(|next| next.first().is_some_and(|b| *b == b' ' || *b == b'\t'))
        {
            if let Some(more) = lines.next() {
                value.push(b' ');
                value.extend_from_slice(more);
            }
        }
        return std::str::from_utf8(trim(&value)).ok()?.parse().ok();
    }
    None
}

/// Whether what is at the front of the buffer is, or could still become, a
/// start line (7.1, 7.2).
///
/// Over UDP something that is not a SIP message is one bad datagram. Over TCP
/// it is a position in a stream that nothing can be resynchronised from, so it
/// is worth saying early and plainly rather than waiting out a Content-Length
/// that is never going to arrive. What actually turns up here is a port scan,
/// a TLS ClientHello sent to the plain port, or an HTTP request.
fn start_line_is_plausible(buffer: &[u8]) -> Result<(), &'static str> {
    let ends_at = buffer.iter().position(|b| *b == b'\n');
    let line = match ends_at {
        Some(at) => strip_cr(&buffer[..at]),
        None => buffer,
    };
    // 7.1 and 7.2 are text. A control octet in the first line is the surest
    // sign of a stream that is not SIP at all, and it is worth catching before
    // the length limit below, because a ClientHello is short.
    if line.iter().any(|b| *b < 0x20 && *b != b'\t') {
        return Err("what arrived on the SIP connection is not text");
    }
    if ends_at.is_none() {
        // Still arriving. A start line this long is not one, but a short one
        // that has not got to its line ending yet is perfectly ordinary.
        return if line.len() > MAX_START_LINE {
            Err("the first line went on longer than any start line")
        } else {
            Ok(())
        };
    }
    let line = trim(line);
    // 7.2: a status line begins with the version. 7.1: a request line ends
    // with it.
    if line.starts_with(b"SIP/2.0") {
        return Ok(());
    }
    let last = line.rsplit(|b| *b == b' ').next().unwrap_or_default();
    if last.eq_ignore_ascii_case(b"SIP/2.0") {
        return Ok(());
    }
    Err("what arrived on the SIP connection does not start with a SIP message")
}

fn strip_cr(line: &[u8]) -> &[u8] {
    match line.split_last() {
        Some((b'\r', rest)) => rest,
        _ => line,
    }
}

fn trim(text: &[u8]) -> &[u8] {
    let from = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(text.len());
    let to = text
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(from, |at| at + 1);
    &text[from..to]
}

/// Which of this machine's addresses reaches the far end, with the port we are
/// known by written onto it.
///
/// Asked of the routing table rather than guessed at: connecting a UDP socket
/// sends nothing, but it does make the system choose a source address, which
/// is the one that will be on our packets and so the one that belongs in the
/// Via and the Contact.
pub fn routed_source(hop: SocketAddr, port: u16) -> Result<SocketAddr, String> {
    let probe = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    probe.connect(hop).map_err(|e| e.to_string())?;
    let chosen = probe.local_addr().map_err(|e| e.to_string())?;
    Ok(SocketAddr::new(chosen.ip(), port))
}

fn would_block(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message, with the Content-Length it deserves.
    fn message(body: &str) -> Vec<u8> {
        format!(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/TCP 192.0.2.4:5060;branch=z9hG4bK{}\r\n\
             Call-ID: call-{}\r\n\
             CSeq: 1 REGISTER\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {body}",
            body.len(),
            body.len(),
            body.len()
        )
        .into_bytes()
    }

    /// The far end wrote two messages and the network put them in one segment.
    /// Both have to come out, and the second must not wait for a third to
    /// arrive behind it.
    #[test]
    fn two_messages_in_one_read_are_two_messages() {
        let mut buffer = message("v=0\r\n");
        buffer.extend_from_slice(&message("v=1\r\n"));

        let Frame::Whole(first) = frame(&buffer) else {
            panic!("the first message was not found in a buffer holding two");
        };
        let one = Message::parse(&buffer[..first]).expect("the first message would not parse");
        assert_eq!(one.body(), b"v=0\r\n");

        let rest = &buffer[first..];
        let Frame::Whole(second) = frame(rest) else {
            panic!("the second message was left in the buffer");
        };
        let two = Message::parse(&rest[..second]).expect("the second message would not parse");
        assert_eq!(two.body(), b"v=1\r\n");
        assert_eq!(second, rest.len(), "the second message did not end where it ends");
    }

    /// And the other way about: one message spread over three reads, which is
    /// what a body of any size does on a path with an MTU.
    #[test]
    fn one_message_split_across_three_reads_is_one_message() {
        let whole = message("v=0\r\ns=-\r\n");
        let first = 20;
        let second = whole.len() - 4;

        let mut buffer = whole[..first].to_vec();
        assert_eq!(
            frame(&buffer),
            Frame::Incomplete,
            "a message cut off inside its headers was taken for a whole one"
        );
        buffer.extend_from_slice(&whole[first..second]);
        assert_eq!(
            frame(&buffer),
            Frame::Incomplete,
            "a message whose body is still arriving was taken for a whole one"
        );
        buffer.extend_from_slice(&whole[second..]);
        assert_eq!(frame(&buffer), Frame::Whole(whole.len()));
    }

    /// The case 7.5 is really about: the headers are all here, the blank line
    /// is here, and the body is not. Everything needed to parse the message
    /// has arrived, which is exactly why a receiver that goes by the blank line
    /// alone gets this wrong and hands up a message with no body in it.
    #[test]
    fn a_body_that_arrives_after_its_headers_is_waited_for() {
        let whole = message("v=0\r\n");
        let headers = whole.len() - 5;
        let mut buffer = whole[..headers].to_vec();
        assert!(
            buffer.ends_with(b"\r\n\r\n"),
            "this test is meant to stop at the blank line"
        );
        assert_eq!(
            frame(&buffer),
            Frame::Incomplete,
            "a message was handed up before its body arrived"
        );
        buffer.extend_from_slice(&whole[headers..]);
        assert_eq!(frame(&buffer), Frame::Whole(whole.len()));
        let parsed = Message::parse(&buffer[..whole.len()]).unwrap();
        assert_eq!(parsed.body(), b"v=0\r\n");
    }

    /// A message with no body at all -- which is most of them: an ACK, a BYE, a
    /// 100, a REGISTER. It ends at the blank line and the next message starts
    /// immediately after it.
    #[test]
    fn a_message_with_no_body_ends_at_the_blank_line() {
        let mut buffer = message("");
        let alone = buffer.len();
        assert_eq!(frame(&buffer), Frame::Whole(alone));
        assert_eq!(&buffer[alone - 4..], b"\r\n\r\n");

        // And what follows it is found, rather than swallowed as a body.
        buffer.extend_from_slice(&message("v=0\r\n"));
        assert_eq!(frame(&buffer), Frame::Whole(alone));
        let Frame::Whole(next) = frame(&buffer[alone..]) else {
            panic!("the message after an empty-bodied one was lost");
        };
        assert_eq!(
            Message::parse(&buffer[alone..alone + next]).unwrap().body(),
            b"v=0\r\n"
        );
    }

    /// 7.5 again: over a stream, a message with no Content-Length cannot be
    /// framed, and neither can anything after it. There is nothing to do but
    /// close the connection.
    #[test]
    fn a_message_with_no_content_length_is_the_end_of_the_connection() {
        let no_length = b"SIP/2.0 200 OK\r\nCall-ID: call-1\r\nCSeq: 1 REGISTER\r\n\r\n";
        assert!(
            matches!(frame(no_length), Frame::Broken(_)),
            "a message with no Content-Length was framed anyway, which means \
             guessing where the next one starts"
        );

        // The compact form is the same header (7.3.3) and is not missing.
        let compact = b"SIP/2.0 200 OK\r\nCall-ID: call-1\r\nl: 0\r\n\r\n";
        assert_eq!(frame(compact), Frame::Whole(compact.len()));

        // Nor is a folded one, however odd it is to write it that way.
        let folded = b"SIP/2.0 200 OK\r\nContent-Length:\r\n 4\r\nCSeq: 1 BYE\r\n\r\nv=0\n";
        assert_eq!(frame(folded), Frame::Whole(folded.len()));
    }

    /// And a far end that is not a far end: a scanner, a TLS ClientHello sent
    /// to the plain port, an HTTP request. None of it can be resynchronised
    /// from, so it ends the connection rather than filling the buffer for ever.
    #[test]
    fn rubbish_ends_the_connection_rather_than_being_waited_on() {
        // A TLS record header, which is what an https:// in a browser sends.
        assert!(matches!(
            frame(b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03"),
            Frame::Broken(_)
        ));
        // Somebody's web scanner.
        assert!(matches!(
            frame(b"GET / HTTP/1.1\r\nHost: 192.0.2.4\r\n\r\n"),
            Frame::Broken(_)
        ));
        // A line of prose, which is what a person testing with telnet sends.
        assert!(matches!(frame(b"hello?\r\n"), Frame::Broken(_)));
        // But an incomplete first line is not yet rubbish: it is a read that
        // stopped in the middle of a perfectly good request line.
        assert_eq!(frame(b"INVITE sip:1000@pbx.local SI"), Frame::Incomplete);
        assert_eq!(frame(b"SIP/2.0 4"), Frame::Incomplete);
        // Nor is a request line that is all there.
        assert_eq!(
            frame(b"OPTIONS sip:1000@pbx.local SIP/2.0\r\n"),
            Frame::Incomplete
        );
    }

    /// 7.5's stray CRLFs, and RFC 5626 3.5.1's keep-alive, which is two of them
    /// and arrives every half minute from several trunks. Neither is a message
    /// and neither may be allowed to look like the start of one.
    #[test]
    fn crlfs_before_a_start_line_are_skipped() {
        assert_eq!(frame(b"\r\n\r\n"), Frame::Skip(4));
        let mut buffer = b"\r\n\r\n".to_vec();
        buffer.extend_from_slice(&message("v=0\r\n"));
        let Frame::Skip(n) = frame(&buffer) else {
            panic!("a keep-alive was taken for the start of a message");
        };
        assert_eq!(n, 4);
        assert!(matches!(frame(&buffer[n..]), Frame::Whole(_)));
    }

    /// A trunk closing an idle connection is ordinary (18.3), and so is one
    /// that has just been restarted. The link has to notice and open another,
    /// and it has to say that it did: the flag is what sends the agent's
    /// in-flight transactions down the new connection, and a reconnection that
    /// went unreported would be a call sitting out the whole of Timer B
    /// waiting for an answer to a request that nothing ever delivered.
    #[test]
    fn a_connection_that_drops_is_opened_again_and_said_to_have_been() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("the trunk's listener");
        let hop = listener.local_addr().unwrap();
        let far_end = thread::spawn(move || {
            // The first connection is taken and dropped at once, which is what
            // a trunk being restarted looks like from this end.
            let (first, _) = listener.accept().expect("nothing connected at all");
            drop(first);
            // The second is held, so there is something for the link to find.
            let (second, _) = listener.accept().expect("nothing connected again");
            thread::sleep(Duration::from_millis(200));
            drop(second);
        });

        let mut link = TcpLink::new(hop, hop);
        link.connect().expect("the first connection would not open");
        assert!(!link.remade, "the first connection is not a reconnection");

        // Reading is how the close is noticed and how the next connection is
        // opened, which is why the agent's loop reads whether it has anything
        // to say or not.
        let deadline = Instant::now() + Duration::from_secs(8);
        while !link.remade && Instant::now() < deadline {
            link.receive();
        }
        assert!(
            link.remade,
            "the connection went and nothing opened another, so every timer \
             above this would have run out against a socket that was not there"
        );
        assert!(
            link.notes.iter().any(|n| n.contains("closed")),
            "the connection went without a word in the transcript: {:?}",
            link.notes
        );
        let _ = far_end.join();
    }

    /// A Content-Length that is a lie about the size of what is coming is not
    /// worth the memory it would take to believe.
    #[test]
    fn an_impossible_content_length_is_refused() {
        let silly = b"SIP/2.0 200 OK\r\nContent-Length: 4294967295\r\n\r\n";
        assert!(matches!(frame(silly), Frame::Broken(_)));
        let nonsense = b"SIP/2.0 200 OK\r\nContent-Length: soon\r\n\r\n";
        assert!(matches!(frame(nonsense), Frame::Broken(_)));
    }
}
