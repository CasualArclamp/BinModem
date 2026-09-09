//! SOCKS version 5 (RFC 1928), the server half.
//!
//! What it is for here: the machine that answered the call has the internet
//! and the machine that dialled wants it. A browser on the dialling machine is
//! pointed at a proxy; everything it says crosses the modem to this, which
//! opens the connection it asks for and passes the two streams through each
//! other. Nothing routes, nothing needs a driver, and nothing needs
//! administrator rights -- which is the whole reason for choosing a proxy over
//! a network interface.
//!
//! This file is only the conversation. It never opens a socket: it reads what
//! the client asked for, says so, and waits to be told what happened. Whoever
//! owns the sockets does that, and a version of this that could reach the
//! network would be a version that could not be tested without one.
//!
//! Only CONNECT is implemented. BIND is for protocols that ask the far end to
//! call back, which nothing has done in decades, and UDP ASSOCIATE cannot be
//! carried by a proxy that has only a stream to work with.

use std::fmt;

/// 3: "The VER field is set to X'05' for this version of the protocol."
pub const VERSION: u8 = 5;

/// 3's method identifiers.
pub mod method {
    pub const NONE: u8 = 0x00;
    pub const GSSAPI: u8 = 0x01;
    pub const PASSWORD: u8 = 0x02;
    /// "If the selected METHOD is X'FF', none of the methods listed by the
    /// client are acceptable, and the client MUST close the connection."
    pub const UNACCEPTABLE: u8 = 0xff;
}

/// 4's commands.
pub mod command {
    pub const CONNECT: u8 = 0x01;
    pub const BIND: u8 = 0x02;
    pub const UDP_ASSOCIATE: u8 = 0x03;
}

/// 5's address types.
pub mod address {
    pub const IPV4: u8 = 0x01;
    pub const DOMAIN: u8 = 0x03;
    pub const IPV6: u8 = 0x04;
}

/// 6's reply field, in the document's own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Succeeded = 0x00,
    GeneralFailure = 0x01,
    NotAllowed = 0x02,
    NetworkUnreachable = 0x03,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    TtlExpired = 0x06,
    CommandNotSupported = 0x07,
    AddressNotSupported = 0x08,
}

impl Reply {
    /// What a failure to open a socket should be reported as.
    ///
    /// The distinctions matter to a browser: "connection refused" is shown as
    /// a page saying so, where a general failure is shown as the proxy being
    /// broken.
    pub fn for_error(kind: std::io::ErrorKind) -> Self {
        use std::io::ErrorKind;
        match kind {
            ErrorKind::ConnectionRefused => Reply::ConnectionRefused,
            ErrorKind::TimedOut | ErrorKind::HostUnreachable => Reply::HostUnreachable,
            ErrorKind::NetworkUnreachable | ErrorKind::NetworkDown => Reply::NetworkUnreachable,
            _ => Reply::GeneralFailure,
        }
    }
}

/// Where the client wants to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Address([u8; 4]),
    /// 5: "the address field contains a fully-qualified domain name", which is
    /// the usual case and the one that matters -- a proxy that resolves names
    /// itself is a proxy the dialling machine needs no resolver for.
    Name(String),
    V6([u8; 16]),
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Destination::Address([a, b, c, d]) => write!(f, "{a}.{b}.{c}.{d}"),
            Destination::Name(name) => write!(f, "{name}"),
            Destination::V6(octets) => {
                let groups: Vec<String> = octets
                    .chunks(2)
                    .map(|p| format!("{:x}", u16::from_be_bytes([p[0], p[1]])))
                    .collect();
                write!(f, "[{}]", groups.join(":"))
            }
        }
    }
}

/// One CONNECT, as asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub destination: Destination,
    pub port: u16,
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.destination, self.port)
    }
}

/// Where the conversation has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for 3's version identifier and method list.
    Greeting,
    /// Waiting for 4's request.
    Waiting,
    /// The request has been read and this end has said nothing yet.
    Asked,
    /// A reply has gone out and the two streams are now one another's.
    Open,
    /// Over: either a reply refusing it, or something that was not SOCKS.
    Done,
}

/// The server side of one SOCKS conversation.
///
/// Octets from the client go in; octets for the client come out. When
/// [`Session::request`] gives something back, whoever owns the sockets should
/// try to open it and then say what happened with [`Session::answer`].
#[derive(Debug)]
pub struct Session {
    state: State,
    /// What has arrived and not yet been made sense of. A stream delivers a
    /// message in as many pieces as it likes, and every one of these is short.
    pending: Vec<u8>,
    request: Option<Request>,
    out: Vec<u8>,
    /// What went wrong, for a log rather than for the client -- the client
    /// gets a reply code.
    trouble: Option<&'static str>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// The longest anything before the relay can be: a version octet, a command,
/// a reserved octet, a type, at most a 255-octet name with its length, and a
/// port.
const MOST_PENDING: usize = 512;

impl Session {
    pub fn new() -> Self {
        Self {
            state: State::Greeting,
            pending: Vec::new(),
            request: None,
            out: Vec::new(),
            trouble: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// What the client asked for, once it has asked.
    pub fn request(&self) -> Option<&Request> {
        self.request.as_ref()
    }

    /// Why it ended, if it ended badly.
    pub fn trouble(&self) -> Option<&'static str> {
        self.trouble
    }

    /// Whether the two streams are now joined and everything is data.
    pub fn open(&self) -> bool {
        self.state == State::Open
    }

    /// Octets for the client.
    pub fn take_out(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Octets from the client.
    ///
    /// Before the relay begins, these are the protocol and nothing comes back
    /// out of here. Once it has begun they are the far end's, and they come
    /// straight back as what to forward.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        match self.state {
            State::Open => return bytes.to_vec(),
            State::Done => return Vec::new(),
            _ => {}
        }
        if self.pending.len() + bytes.len() > MOST_PENDING {
            return self.give_up("the greeting was longer than any greeting");
        }
        self.pending.extend_from_slice(bytes);
        loop {
            let before = self.pending.len();
            match self.state {
                State::Greeting => self.read_greeting(),
                State::Waiting => self.read_request(),
                // Nothing more is read until the caller has answered, and
                // anything the client sends meanwhile is the relay's -- it is
                // held in `pending` and handed over when the relay opens.
                State::Asked | State::Open | State::Done => break,
            }
            if self.pending.len() == before {
                break;
            }
        }
        // Anything left over was sent before the reply and belongs to the
        // far end, which does not exist yet. `answer` hands it back.
        Vec::new()
    }

    /// Say what became of the connection the client asked for.
    ///
    /// `bound` is 6's BND.ADDR and BND.PORT: "the port number that the server
    /// assigned to connect to the target host" and the address that goes with
    /// it. A client that does not care -- and a browser does not -- is happy
    /// with zeroes.
    ///
    /// What comes back is anything the client sent before waiting for this,
    /// which is the first thing the far end should be given. A client is
    /// entitled to send its request immediately after asking, and a proxy that
    /// dropped it would work with every client that waits and no client that
    /// does not.
    #[must_use = "what comes back is the client's first data and is lost if dropped"]
    pub fn answer(&mut self, reply: Reply, bound: ([u8; 4], u16)) -> Vec<u8> {
        if self.state != State::Asked {
            return Vec::new();
        }
        let (address, port) = bound;
        let mut out = vec![VERSION, reply as u8, 0x00, address::IPV4];
        out.extend_from_slice(&address);
        out.extend_from_slice(&port.to_be_bytes());
        self.out.extend_from_slice(&out);
        if reply == Reply::Succeeded {
            self.state = State::Open;
            return std::mem::take(&mut self.pending);
        }
        self.state = State::Done;
        self.pending.clear();
        Vec::new()
    }

    fn read_greeting(&mut self) {
        if self.pending.len() < 2 {
            return;
        }
        if self.pending[0] != VERSION {
            self.give_up("not SOCKS 5");
            return;
        }
        let count = usize::from(self.pending[1]);
        if self.pending.len() < 2 + count {
            return;
        }
        let methods = &self.pending[2..2 + count];
        // "The server selects from one of the methods given in METHODS."
        // There is only one on offer: the link under this has already decided
        // who may use it, since reaching it at all means having placed a call.
        let acceptable = methods.contains(&method::NONE);
        self.pending.drain(..2 + count);
        self.out.extend_from_slice(&[
            VERSION,
            if acceptable {
                method::NONE
            } else {
                method::UNACCEPTABLE
            },
        ]);
        if acceptable {
            self.state = State::Waiting;
        } else {
            // "the client MUST close the connection", and there is nothing
            // more to say to it.
            self.state = State::Done;
            self.trouble = Some("the client would not go without authentication");
        }
    }

    fn read_request(&mut self) {
        if self.pending.len() < 4 {
            return;
        }
        let (version, cmd, kind) = (self.pending[0], self.pending[1], self.pending[3]);
        if version != VERSION {
            self.give_up("not SOCKS 5");
            return;
        }
        // How long the address is, and where the port begins after it.
        let (destination, length) = match kind {
            address::IPV4 => {
                if self.pending.len() < 4 + 4 + 2 {
                    return;
                }
                let mut octets = [0u8; 4];
                octets.copy_from_slice(&self.pending[4..8]);
                (Destination::Address(octets), 4)
            }
            address::DOMAIN => {
                let count = usize::from(self.pending[4]);
                if self.pending.len() < 5 + count + 2 {
                    return;
                }
                let Ok(name) = std::str::from_utf8(&self.pending[5..5 + count]) else {
                    self.refuse(Reply::AddressNotSupported, "the name was not text");
                    return;
                };
                (Destination::Name(name.to_owned()), 1 + count)
            }
            address::IPV6 => {
                if self.pending.len() < 4 + 16 + 2 {
                    return;
                }
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&self.pending[4..20]);
                (Destination::V6(octets), 16)
            }
            _ => {
                self.refuse(Reply::AddressNotSupported, "an address type nobody uses");
                return;
            }
        };
        let at = 4 + length;
        let port = u16::from_be_bytes([self.pending[at], self.pending[at + 1]]);
        self.pending.drain(..at + 2);

        if cmd != command::CONNECT {
            // BIND asks the proxy to listen for a call back, and UDP ASSOCIATE
            // wants datagrams, neither of which a stream through a modem is.
            self.refuse(Reply::CommandNotSupported, "only CONNECT is implemented");
            return;
        }
        self.request = Some(Request { destination, port });
        self.state = State::Asked;
    }

    /// Answer a request this end will not carry out, and stop.
    fn refuse(&mut self, reply: Reply, why: &'static str) {
        self.state = State::Asked;
        let _ = self.answer(reply, ([0, 0, 0, 0], 0));
        self.state = State::Done;
        self.trouble = Some(why);
    }

    /// Something arrived that was not this protocol. There is nothing to say
    /// that the far end would understand, so nothing is said.
    fn give_up(&mut self, why: &'static str) -> Vec<u8> {
        self.state = State::Done;
        self.trouble = Some(why);
        self.pending.clear();
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole conversation, as a browser has it.
    #[test]
    fn a_browser_asks_for_a_name_and_is_connected() {
        let mut session = Session::new();
        // "VER, NMETHODS, METHODS", offering no authentication and one other.
        assert!(session.feed(&[VERSION, 2, method::NONE, method::PASSWORD]).is_empty());
        assert_eq!(session.take_out(), vec![VERSION, method::NONE]);
        assert_eq!(session.state(), State::Waiting);

        // "VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT" for example.org:80.
        let mut request = vec![VERSION, command::CONNECT, 0x00, address::DOMAIN, 11];
        request.extend_from_slice(b"example.org");
        request.extend_from_slice(&80u16.to_be_bytes());
        session.feed(&request);
        assert_eq!(session.state(), State::Asked);
        assert_eq!(
            session.request(),
            Some(&Request {
                destination: Destination::Name("example.org".into()),
                port: 80
            })
        );
        assert_eq!(session.request().unwrap().to_string(), "example.org:80");

        assert!(session.answer(Reply::Succeeded, ([10, 0, 0, 1], 40_000)).is_empty());
        let reply = session.take_out();
        assert_eq!(&reply[..4], &[VERSION, 0x00, 0x00, address::IPV4]);
        assert_eq!(&reply[4..8], &[10, 0, 0, 1]);
        assert_eq!(&reply[8..10], &40_000u16.to_be_bytes());
        assert!(session.open());

        // From here everything is the far end's.
        assert_eq!(session.feed(b"GET / HTTP/1.1\r\n"), b"GET / HTTP/1.1\r\n");
    }

    /// A stream delivers a message in whatever pieces it likes, including one
    /// octet at a time.
    #[test]
    fn the_conversation_survives_being_delivered_one_octet_at_a_time() {
        let mut whole = vec![VERSION, 1, method::NONE];
        let mut request = vec![VERSION, command::CONNECT, 0x00, address::IPV4];
        request.extend_from_slice(&[93, 184, 216, 34]);
        request.extend_from_slice(&443u16.to_be_bytes());
        whole.extend_from_slice(&request);

        let mut session = Session::new();
        for octet in whole {
            session.feed(&[octet]);
        }
        assert_eq!(session.state(), State::Asked);
        assert_eq!(
            session.request().unwrap().destination,
            Destination::Address([93, 184, 216, 34])
        );
        assert_eq!(session.request().unwrap().port, 443);
    }

    /// And a client that sends everything at once, including data before the
    /// reply, gets that data forwarded rather than eaten.
    #[test]
    fn data_sent_before_the_reply_is_not_lost() {
        let mut session = Session::new();
        let mut all = vec![VERSION, 1, method::NONE, VERSION, command::CONNECT, 0x00, address::IPV4];
        all.extend_from_slice(&[1, 2, 3, 4]);
        all.extend_from_slice(&80u16.to_be_bytes());
        all.extend_from_slice(b"GET / HTTP/1.0\r\n\r\n");
        assert!(session.feed(&all).is_empty(), "it forwarded before it was open");
        assert_eq!(session.state(), State::Asked);

        // The request that was waiting behind the handshake comes out now.
        let early = session.answer(Reply::Succeeded, ([0, 0, 0, 0], 0));
        assert_eq!(early, b"GET / HTTP/1.0\r\n\r\n");
    }

    /// A destination that could not be reached is said so in the client's own
    /// terms, which is what makes a browser show the right page.
    #[test]
    fn a_refusal_is_reported_as_a_refusal() {
        let mut session = Session::new();
        session.feed(&[VERSION, 1, method::NONE]);
        session.take_out();
        let mut request = vec![VERSION, command::CONNECT, 0x00, address::IPV4];
        request.extend_from_slice(&[127, 0, 0, 1]);
        request.extend_from_slice(&9u16.to_be_bytes());
        session.feed(&request);

        assert!(
            session
                .answer(Reply::ConnectionRefused, ([0, 0, 0, 0], 0))
                .is_empty()
        );
        assert_eq!(session.take_out()[1], Reply::ConnectionRefused as u8);
        assert_eq!(session.state(), State::Done);
        assert!(!session.open());
    }

    /// The two commands this does not carry out are refused with the code that
    /// says so, rather than by going quiet.
    #[test]
    fn bind_and_udp_are_refused_by_name() {
        for cmd in [command::BIND, command::UDP_ASSOCIATE] {
            let mut session = Session::new();
            session.feed(&[VERSION, 1, method::NONE]);
            session.take_out();
            let mut request = vec![VERSION, cmd, 0x00, address::IPV4];
            request.extend_from_slice(&[1, 1, 1, 1]);
            request.extend_from_slice(&53u16.to_be_bytes());
            session.feed(&request);
            let reply = session.take_out();
            assert_eq!(reply[1], Reply::CommandNotSupported as u8);
            assert_eq!(session.state(), State::Done);
        }
    }

    /// A client that will not go without authentication is told so and left
    /// to close, which is what 3 asks of it.
    #[test]
    fn a_client_that_insists_on_authentication_is_told_there_is_none() {
        let mut session = Session::new();
        session.feed(&[VERSION, 1, method::PASSWORD]);
        assert_eq!(session.take_out(), vec![VERSION, method::UNACCEPTABLE]);
        assert_eq!(session.state(), State::Done);
        assert!(session.trouble().is_some());
    }

    /// Something that is not SOCKS at all is dropped rather than answered.
    #[test]
    fn a_stream_that_is_not_socks_is_dropped() {
        let mut session = Session::new();
        session.feed(b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(session.state(), State::Done);
        assert!(session.take_out().is_empty(), "it answered a browser in SOCKS");
        assert_eq!(session.trouble(), Some("not SOCKS 5"));
    }

    /// A name field long enough to be a nuisance is bounded.
    #[test]
    fn a_greeting_that_never_ends_is_given_up_on() {
        let mut session = Session::new();
        session.feed(&[VERSION, 255]);
        for _ in 0..8 {
            session.feed(&[0u8; 200]);
        }
        assert_eq!(session.state(), State::Done);
    }

    /// An address in the sixth version reads back as one, even though nothing
    /// here can reach it yet.
    #[test]
    fn an_address_of_the_other_kind_is_read_rather_than_misread() {
        let mut session = Session::new();
        session.feed(&[VERSION, 1, method::NONE]);
        session.take_out();
        let mut request = vec![VERSION, command::CONNECT, 0x00, address::IPV6];
        let octets: [u8; 16] = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        request.extend_from_slice(&octets);
        request.extend_from_slice(&443u16.to_be_bytes());
        session.feed(&request);
        assert_eq!(session.state(), State::Asked);
        assert_eq!(session.request().unwrap().destination, Destination::V6(octets));
        assert_eq!(
            session.request().unwrap().to_string(),
            "[2001:db8:0:0:0:0:0:1]:443"
        );
    }
}
