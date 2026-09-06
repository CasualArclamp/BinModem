//! A board over a socket, for exercising the terminal with no line under it.
//!
//! The third thing that can be on the screen, after a capture and a live line.
//! There is no modem here at all: a TCP connection to a bulletin board, the
//! telnet option negotiation on top of it, and the bytes handed straight to the
//! same [`terminal::Terminal`] a call would feed.
//!
//! It earns its place because the two halves of "the board looked wrong" are
//! otherwise impossible to tell apart. A screen full of rubbish over a modem
//! could be an escape byte the line dropped or an escape sequence the terminal
//! does not implement, and the only way to know is to remove one of them.
//! Over a socket every byte arrives, so anything still wrong is ours -- and
//! anything right here that is wrong over a call is the line's.
//!
//! It is also simply the fastest way to have a board on the screen. No cable,
//! no softphone, no trunk, no waiting for a handshake that may not converge.
//!
//! The shape is [`crate::live`]'s, deliberately: a session the window writes
//! requests into, a thread that owns the connection, and telemetry back. What
//! differs is that a socket is not a clock, so this loop paces itself off the
//! read timeout instead of off arriving samples.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use telemetry::{CallState, Direction, Leds, Publisher};

use crate::engine::Control;

/// What the far end is told this terminal is.
///
/// The answer that decides whether the art arrives. Boards branch on the
/// terminal type, and one that is told nothing sends the plain ASCII menus --
/// which would make a terminal with a broken ANSI parser look as though it
/// were working perfectly.
const TERMINAL_TYPE: &str = "ANSI";

/// The port a board is on when the address does not say.
const TELNET_PORT: u16 = 23;

/// How long to wait for a connection before giving up on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a read waits before the loop goes round anyway.
///
/// This is the whole clock of the thing. Short enough that what is typed goes
/// out promptly, long enough that an idle connection is not a spin loop.
const READ_TIMEOUT: Duration = Duration::from_millis(20);

/// What the window has asked for.
#[derive(Debug, Clone)]
enum Request {
    Connect(String),
    Disconnect,
}

/// What the connection is doing, for the window to show.
#[derive(Debug, Clone, Default)]
pub struct NetState {
    pub connected: bool,
    /// Where it went, resolved, which is not always where it was asked to go.
    pub host: String,
    pub peer: String,
    pub error: Option<String>,
    /// Whether the far end took on echoing. Worth showing because a board that
    /// did not is one where nothing typed will appear until it does.
    pub echo: bool,
    /// Whether eight-bit data was agreed. Without it the art arrives with its
    /// top bits stripped and every box is drawn out of question marks.
    pub binary: bool,
}

/// The one thing the window and the connection thread share.
#[derive(Debug, Default)]
pub struct Session {
    typed: Mutex<Vec<u8>>,
    request: Mutex<Option<Request>>,
    state: Mutex<NetState>,
    /// Whether to put everything that arrives in the transcript as well as on
    /// the screen.
    ///
    /// Off by default, because a board's opening screen is several thousand
    /// bytes and would bury everything else. On, it is the thing this mode is
    /// for: the sequence that drew wrongly, next to what it drew.
    logging: AtomicBool,
}

impl Session {
    pub fn connect(&self, host: &str) {
        self.ask(Request::Connect(host.to_owned()));
    }

    pub fn disconnect(&self) {
        self.ask(Request::Disconnect);
    }

    pub fn type_bytes(&self, bytes: &[u8]) {
        if let Ok(mut q) = self.typed.lock() {
            q.extend_from_slice(bytes);
        }
    }

    pub fn state(&self) -> NetState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn logging(&self) -> bool {
        self.logging.load(Ordering::Relaxed)
    }

    pub fn set_logging(&self, on: bool) {
        self.logging.store(on, Ordering::Relaxed);
    }

    fn ask(&self, request: Request) {
        if let Ok(mut slot) = self.request.lock() {
            *slot = Some(request);
        }
    }

    fn take_request(&self) -> Option<Request> {
        self.request.lock().ok().and_then(|mut r| r.take())
    }

    fn take_typed(&self) -> Vec<u8> {
        self.typed.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }

    fn set_state(&self, state: NetState) {
        if let Ok(mut slot) = self.state.lock() {
            *slot = state;
        }
    }
}

/// Start the connection thread. It has nowhere to go until the window says.
pub fn spawn(tx: Publisher, control: Arc<Control>, session: Arc<Session>) -> JoinHandle<()> {
    thread::spawn(move || run(tx, control, session))
}

fn run(tx: Publisher, control: Arc<Control>, session: Arc<Session>) {
    tx.log(Direction::Note, "telnet: no line, no modem, just the terminal");
    tx.log(Direction::Note, "enter a host above and connect");

    let mut socket: Option<TcpStream> = None;
    let mut proto = telnet::Telnet::new(
        TERMINAL_TYPE,
        terminal::DEFAULT_COLS as u16,
        terminal::DEFAULT_ROWS as u16,
    );
    let mut buf = [0u8; 4096];
    let mut data: Vec<u8> = Vec::with_capacity(4096);
    let mut out: Vec<u8> = Vec::with_capacity(4096);
    let (mut rx_bytes, mut tx_bytes) = (0u64, 0u64);
    let mut typed_recently = Instant::now() - Duration::from_secs(1);
    let mut heard_recently = typed_recently;

    let publish_every = Duration::from_millis(16);
    let mut next_publish = Instant::now();

    while !control.quit.load(Ordering::Relaxed) {
        if let Some(request) = session.take_request() {
            // Dropping the stream closes it, which is what a disconnect is and
            // is also what has to happen before another one opens.
            socket = None;
            // A fresh negotiation for a fresh connection. Carrying the option
            // state over would leave this end believing the new board had
            // already agreed to things it has never been asked.
            proto = telnet::Telnet::new(
                TERMINAL_TYPE,
                terminal::DEFAULT_COLS as u16,
                terminal::DEFAULT_ROWS as u16,
            );
            let mut state = NetState::default();
            match request {
                Request::Connect(host) => {
                    tx.log(Direction::Note, format!("connecting to {host}"));
                    match dial(&host) {
                        Ok(stream) => {
                            let peer = stream
                                .peer_addr()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|_| host.clone());
                            tx.log(Direction::Note, format!("connected to {peer}"));
                            state = NetState {
                                connected: true,
                                host: host.clone(),
                                peer,
                                ..NetState::default()
                            };
                            socket = Some(stream);
                        }
                        Err(e) => {
                            tx.log(Direction::Note, format!("could not connect: {e}"));
                            state.host = host;
                            state.error = Some(e);
                        }
                    }
                }
                Request::Disconnect => tx.log(Direction::Note, "disconnected"),
            }
            session.set_state(state);
        }

        let typed = session.take_typed();
        if !typed.is_empty() {
            typed_recently = Instant::now();
        }

        let Some(stream) = socket.as_mut() else {
            // Nothing typed at a closed connection goes anywhere, which is the
            // honest answer: there is no modem here to answer it.
            thread::sleep(READ_TIMEOUT);
            publish(&tx, &mut next_publish, publish_every, false, rx_bytes, tx_bytes,
                    Leds::default(), false, false);
            continue;
        };

        // What the far end is owed by the protocol, then what was typed. In
        // that order: an answer that arrived behind a keystroke is an answer
        // the board may already have given up waiting for.
        out.clear();
        out.extend_from_slice(&proto.take_reply());
        if !typed.is_empty() {
            proto.encode(&typed, &mut out);
            tx_bytes += typed.len() as u64;
        }
        let mut lost: Option<String> = None;
        if !out.is_empty()
            && let Err(e) = stream.write_all(&out)
        {
            lost = Some(e.to_string());
        }

        if lost.is_none() {
            match stream.read(&mut buf) {
                Ok(0) => lost = Some("the far end hung up".into()),
                Ok(n) => {
                    rx_bytes += n as u64;
                    heard_recently = Instant::now();
                    data.clear();
                    proto.feed_bytes(&buf[..n], &mut data);
                    if !data.is_empty() {
                        if session.logging() {
                            tx.log_bytes(Direction::FromLine, &data);
                        }
                        tx.line_data(&data);
                    }
                }
                // The read timeout, which is this loop's clock rather than a
                // fault. Windows calls it one thing and everything else calls
                // it the other.
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(e) => lost = Some(e.to_string()),
            }
        }

        if let Some(why) = lost {
            tx.log(Direction::Note, format!("connection closed: {why}"));
            socket = None;
            let mut state = session.state();
            state.connected = false;
            state.error = Some(why);
            session.set_state(state);
            continue;
        }

        // Only when the negotiation has settled somewhere new, so the window
        // is not taking this lock sixty times a second for nothing.
        {
            let state = session.state();
            if state.echo != proto.echo() || state.binary != proto.binary() {
                session.set_state(NetState {
                    echo: proto.echo(),
                    binary: proto.binary(),
                    ..state
                });
            }
        }

        let recent = |at: Instant| at.elapsed() < Duration::from_millis(250);
        publish(
            &tx,
            &mut next_publish,
            publish_every,
            true,
            rx_bytes,
            tx_bytes,
            Leds {
                mr: true,
                tr: true,
                sd: recent(typed_recently),
                rd: recent(heard_recently),
                // There is no carrier and never will be. Lighting these anyway
                // would be inventing a line that does not exist; the front
                // panel is not shown in this mode for the same reason.
                ..Leds::default()
            },
            proto.echo(),
            proto.binary(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn publish(
    tx: &Publisher,
    next: &mut Instant,
    every: Duration,
    connected: bool,
    rx_bytes: u64,
    tx_bytes: u64,
    leds: Leds,
    echo: bool,
    binary: bool,
) {
    if Instant::now() < *next {
        return;
    }
    *next = Instant::now() + every;
    tx.publish(|f| {
        f.state = if connected { CallState::Connected } else { CallState::Idle };
        f.modulation = "telnet";
        // Not a line rate. There is no line, and a number here would be a
        // guess dressed up as a measurement.
        f.bit_rate = None;
        f.rx_bytes = rx_bytes;
        f.tx_bytes = tx_bytes;
        f.carrier = connected;
        f.leds = leds;
        f.symbol_label = match (echo, binary) {
            (true, true) => "echo 8-bit",
            (true, false) => "echo 7-bit",
            (false, true) => "local 8-bit",
            (false, false) => "local 7-bit",
        };
    });
}

/// Open a connection to `host`, which may or may not name a port.
fn dial(host: &str) -> Result<TcpStream, String> {
    let target = with_port(host);
    let addrs: Vec<SocketAddr> = target
        .to_socket_addrs()
        .map_err(|e| format!("{target}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("{target}: no address"));
    }
    let mut last = String::new();
    for addr in &addrs {
        match TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                // Every keystroke on its own, rather than waiting to fill a
                // segment. A terminal is the case Nagle's algorithm was never
                // meant for, and with it on the board sees typing in bursts.
                let _ = stream.set_nodelay(true);
                stream
                    .set_read_timeout(Some(READ_TIMEOUT))
                    .map_err(|e| e.to_string())?;
                return Ok(stream);
            }
            Err(e) => last = format!("{addr}: {e}"),
        }
    }
    Err(last)
}

/// Add the default port unless the address already carries one.
///
/// Told apart by the colons rather than by parsing: a bare IPv6 address has
/// several and a host name with a port has exactly one, and a bracketed IPv6
/// address only has a port if there is a colon after the bracket.
fn with_port(host: &str) -> String {
    let host = host.trim();
    match host.rfind(']') {
        // Bracketed, so the brackets say where the address ends and the only
        // thing that can follow them is a port.
        Some(bracket) if host[bracket + 1..].starts_with(':') => host.to_owned(),
        Some(_) => format!("{host}:{TELNET_PORT}"),
        // Unbracketed: one colon is a port, and several are a bare IPv6
        // address, which has to be bracketed before a port can be stuck on the
        // end of it without the two running together.
        None if host.matches(':').count() == 1 => host.to_owned(),
        None if host.contains(':') => format!("[{host}]:{TELNET_PORT}"),
        None => format!("{host}:{TELNET_PORT}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read from `server` until `done` is happy with what has arrived.
    ///
    /// A socket hands over whatever happened to have arrived, so no single
    /// read can be expected to contain a whole anything.
    fn read_until(
        server: &mut TcpStream,
        done: impl Fn(&[u8]) -> bool,
        what: &str,
    ) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        let mut buf = [0u8; 512];
        while !done(&got) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {got:?}");
            match server.read(&mut buf) {
                Ok(0) => panic!("the client hung up waiting for {what}"),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(e) => panic!("{what}: {e}"),
            }
        }
        got
    }

    fn holds(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_connection_negotiates_and_carries_bytes_both_ways() {
        // Everything above this is tested without a socket, and should be.
        // This is the one thing that cannot be: that the two halves are wired
        // to each other the right way round, on a real connection, with the
        // reads and writes arriving in an order neither end chose.
        //
        // A listener on the loopback rather than a board, so the test needs
        // no network, no board that is still up, and nobody else's machine.
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");

        let (tx, rx) = telemetry::channel(64, 32, 8_000.0);
        let control = Arc::new(Control::default());
        let session = Arc::new(Session::default());
        let thread = spawn(tx, control.clone(), session.clone());
        session.connect(&addr.to_string());

        let (mut server, _) = listener.accept().expect("accept");
        server.set_read_timeout(Some(Duration::from_millis(20))).expect("timeout");

        // The client opens the negotiation rather than waiting to be asked,
        // because a board that never asks would otherwise leave the
        // connection in seven-bit line mode.
        let greeting = read_until(
            &mut server,
            |got| holds(got, &[telnet::IAC, telnet::WILL, telnet::option::TERMINAL_TYPE]),
            "the greeting",
        );
        assert!(
            holds(&greeting, &[telnet::IAC, telnet::DO, telnet::option::BINARY]),
            "did not ask for eight-bit data: {greeting:?}"
        );

        // Agree to echo, and ask what it is talking to.
        server
            .write_all(&[
                telnet::IAC,
                telnet::WILL,
                telnet::option::ECHO,
                telnet::IAC,
                telnet::SB,
                telnet::option::TERMINAL_TYPE,
                1,
                telnet::IAC,
                telnet::SE,
            ])
            .expect("write");
        let answer = read_until(
            &mut server,
            |got| holds(got, TERMINAL_TYPE.as_bytes()),
            "the terminal type",
        );
        assert!(
            holds(&answer, &[telnet::IAC, telnet::DO, telnet::option::ECHO]),
            "did not accept being echoed to: {answer:?}"
        );

        // A board sends its opening screen, with a 255 in it -- CP437 has a
        // character there and board art is full of them, so this is the byte
        // most likely to be mangled on the way through.
        server.write_all(b"Welcome").expect("write");
        server.write_all(&[telnet::IAC, telnet::IAC]).expect("write");
        server.write_all(b"\x1b[0m\r\n").expect("write");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut screen = Vec::new();
        while !holds(&screen, b"Welcome") {
            assert!(Instant::now() < deadline, "the screen never arrived: {screen:?}");
            screen.extend(rx.take_line_data());
            thread::sleep(Duration::from_millis(5));
        }
        while !holds(&screen, b"\r\n") && Instant::now() < deadline {
            screen.extend(rx.take_line_data());
            thread::sleep(Duration::from_millis(5));
        }
        let mut want = b"Welcome".to_vec();
        want.push(255);
        want.extend_from_slice(b"\x1b[0m\r\n");
        assert_eq!(screen, want, "the screen is not what was sent");

        // And typing goes the other way, escaped on the way out. The far end
        // has not agreed to eight-bit data, so the return is still padded.
        session.type_bytes(&[b'h', b'i', 255, b'\r']);
        let typed = read_until(&mut server, |got| holds(got, b"hi"), "what was typed");
        let mut want = b"hi".to_vec();
        want.extend_from_slice(&[telnet::IAC, telnet::IAC, b'\r', 0]);
        assert_eq!(typed, want);

        control.quit.store(true, Ordering::Relaxed);
        thread.join().expect("the connection thread panicked");
    }

    #[test]
    fn a_connection_that_is_refused_is_reported_rather_than_hung() {
        // Bound and then dropped, so nothing is listening on a port that
        // certainly existed a moment ago. A refusal has to come back as
        // something the window can show, not as a thread that never returns.
        use std::net::TcpListener;

        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr")
        };

        let (tx, _rx) = telemetry::channel(64, 32, 8_000.0);
        let control = Arc::new(Control::default());
        let session = Arc::new(Session::default());
        let thread = spawn(tx, control.clone(), session.clone());
        session.connect(&addr.to_string());

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = session.state();
            if state.error.is_some() {
                assert!(!state.connected);
                break;
            }
            assert!(Instant::now() < deadline, "no answer either way");
            thread::sleep(Duration::from_millis(5));
        }

        control.quit.store(true, Ordering::Relaxed);
        thread.join().expect("the connection thread panicked");
    }

    #[test]
    fn a_bare_host_gets_the_telnet_port() {
        assert_eq!(with_port("bbs.example.org"), "bbs.example.org:23");
    }

    #[test]
    fn a_port_that_was_given_is_kept() {
        // Boards are on 23 far less often than the folklore suggests; most of
        // the surviving ones moved once their host started blocking it.
        assert_eq!(with_port("bbs.example.org:2323"), "bbs.example.org:2323");
    }

    #[test]
    fn surrounding_space_is_not_part_of_the_host() {
        assert_eq!(with_port("  bbs.example.org  "), "bbs.example.org:23");
    }

    #[test]
    fn a_bare_address_of_either_family_gets_the_port() {
        assert_eq!(with_port("192.0.2.10"), "192.0.2.10:23");
        assert_eq!(with_port("2001:db8::1"), "[2001:db8::1]:23");
    }

    #[test]
    fn a_bracketed_address_is_read_by_its_bracket() {
        // The case the colon count gets wrong on its own, in both directions.
        assert_eq!(with_port("[2001:db8::1]"), "[2001:db8::1]:23");
        assert_eq!(with_port("[2001:db8::1]:2323"), "[2001:db8::1]:2323");
    }
}
