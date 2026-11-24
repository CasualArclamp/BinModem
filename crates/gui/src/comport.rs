//! The modem on a COM port: for DOSBox, and for callers who telnet in.
//!
//! `binmodem --com <port>` listens for DOSBox's network serial port
//! (`serial1=nullmodem server:<host> port:<port>`), so a DOS program on COM1
//! talks to this modem the way it talked to a Courier on the back of a PC:
//! `AT` commands, `ATDT`, `CONNECT 14400/ARQ/HST`, and data. With `--in` and
//! `--out` the modem is on a sound-card line and dials and answers over it.
//!
//! `--telnet-in <port>` adds the other way a call can arrive today. Someone
//! who telnets to that port rings the DOS program: `RING` every six seconds,
//! answered by `ATA`, or by itself after the number of rings `ATS0=` set,
//! as a BBS on a modem always did. Then the caller and the DOS program are
//! connected, and the program sees carrier until either end hangs up.
//!
//! DOSBox's nullmodem carries the RS-232 lines as well as the data, unless
//! told `transparent:1`: an 0xFF escape, then a byte whose bit 0 is the
//! sender's RTS and bit 1 its DTR, which the far end sees as CTS and as
//! DSR and carrier. So the DOS program sees DCD when there is a call, CTS
//! drop when the modem's buffer is full, and dropping DTR hangs up -- which
//! is how most BBS software ends a call.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use modem::{Modem, State};

const FS: f64 = 16_000.0;
/// How loud to write to the line; see `answer.rs`.
const LEVEL: f32 = 0.45;

/// The North American ring cadence: a RING every six seconds.
const RING_EVERY_MS: u32 = 6000;
/// How many rings before a telnet caller is told nobody answered.
const RINGS_BEFORE_GIVING_UP: u32 = 10;
/// Octets queued toward the far end before CTS drops.
const ROOM: usize = 4096;
/// The quiet either side of `+++` (S12's default, a second).
const GUARD_MS: u32 = 1000;

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const ECHO: u8 = 1;
const SGA: u8 = 3;

/// DOSBox's nullmodem framing, both ways.
#[derive(Debug)]
pub struct Dte {
    transparent: bool,
    escaped: bool,
    /// The DOS program's DTR, as last told.
    pub dtr: bool,
    /// The lines last sent: (CTS, DCD).
    sent: Option<(bool, bool)>,
}

impl Dte {
    pub fn new(transparent: bool) -> Self {
        Self { transparent, escaped: false, dtr: true, sent: None }
    }

    /// Bytes from DOSBox: data appended to `out`, line changes taken.
    pub fn decode(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            if self.transparent {
                out.push(b);
            } else if self.escaped {
                self.escaped = false;
                if b == 0xFF {
                    out.push(0xFF);
                } else {
                    self.dtr = b & 2 != 0;
                }
            } else if b == 0xFF {
                self.escaped = true;
            } else {
                out.push(b);
            }
        }
    }

    pub fn encode(&self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            if b == 0xFF && !self.transparent {
                out.push(0xFF);
            }
            out.push(b);
        }
    }

    /// Tell DOSBox the lines, if they changed: CTS on our RTS, DCD (and DSR)
    /// on our DTR.
    pub fn lines(&mut self, cts: bool, dcd: bool, out: &mut Vec<u8>) {
        if self.transparent || self.sent == Some((cts, dcd)) {
            return;
        }
        self.sent = Some((cts, dcd));
        out.extend_from_slice(&[0xFF, u8::from(cts) | u8::from(dcd) << 1]);
    }
}

/// The server's half of telnet: enough that a client goes to character at
/// a time with the far end echoing, which is what a BBS expects, and that
/// option traffic never reaches the DOS program.
#[derive(Debug, Default)]
pub struct TelnetServer {
    state: TnState,
    after_cr: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum TnState {
    #[default]
    Data,
    Iac,
    Verb(u8),
    Sub,
    SubIac,
}

impl TelnetServer {
    /// What goes to a client as it connects: this end echoes, and nobody
    /// waits for go-aheads.
    pub fn greeting() -> Vec<u8> {
        vec![IAC, WILL, ECHO, IAC, WILL, SGA]
    }

    /// Bytes from the client: data to `data`, anything owed to `reply`.
    pub fn feed(&mut self, bytes: &[u8], data: &mut Vec<u8>, reply: &mut Vec<u8>) {
        for &b in bytes {
            self.state = match self.state {
                TnState::Data => match b {
                    IAC => TnState::Iac,
                    // Enter arrives as CR LF or CR NUL; a BBS wants the CR.
                    b'\n' | 0 if self.after_cr => {
                        self.after_cr = false;
                        TnState::Data
                    }
                    _ => {
                        self.after_cr = b == b'\r';
                        data.push(b);
                        TnState::Data
                    }
                },
                TnState::Iac => match b {
                    IAC => {
                        data.push(IAC);
                        TnState::Data
                    }
                    WILL | WONT | DO | DONT => TnState::Verb(b),
                    SB => TnState::Sub,
                    _ => TnState::Data,
                },
                TnState::Verb(verb) => {
                    match (verb, b) {
                        // Already offered in the greeting.
                        (DO, ECHO | SGA) => {}
                        (DO, opt) => reply.extend_from_slice(&[IAC, WONT, opt]),
                        (WILL, SGA) => reply.extend_from_slice(&[IAC, DO, SGA]),
                        (WILL, opt) => reply.extend_from_slice(&[IAC, DONT, opt]),
                        _ => {}
                    }
                    TnState::Data
                }
                TnState::Sub => {
                    if b == IAC {
                        TnState::SubIac
                    } else {
                        TnState::Sub
                    }
                }
                TnState::SubIac => {
                    if b == SE {
                        TnState::Data
                    } else {
                        TnState::Sub
                    }
                }
            };
        }
    }

    pub fn encode(bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            if b == IAC {
                out.push(IAC);
            }
            out.push(b);
        }
    }
}

/// `+++` with a second's quiet either side, on a call this end carries
/// itself rather than through the modem.
#[derive(Debug, Default)]
struct Escape {
    quiet_ms: u32,
    pluses: u8,
}

impl Escape {
    /// A byte from the DOS program; true if it may be one of the three.
    fn byte(&mut self, b: u8) {
        if b == b'+' && (self.pluses > 0 || self.quiet_ms >= GUARD_MS) && self.pluses < 3 {
            self.pluses += 1;
        } else {
            self.pluses = 0;
        }
        self.quiet_ms = 0;
    }

    /// Time passing; true when the escape is complete.
    fn tick(&mut self, ms: u32) -> bool {
        self.quiet_ms = self.quiet_ms.saturating_add(ms);
        if self.pluses == 3 && self.quiet_ms >= GUARD_MS {
            self.pluses = 0;
            return true;
        }
        false
    }
}

/// A call that came in over telnet rather than the line.
#[derive(Debug)]
enum TelnetCall {
    None,
    Ringing { socket: TcpStream, since_ms: u32, rings: u32 },
    Up { socket: TcpStream, server: TelnetServer, to_caller: Vec<u8>, escape: Escape, online: bool },
}

/// The modem, the DOS program on its COM port, and anyone telnetting in.
pub struct Station {
    pub modem: Modem,
    dte: Option<(TcpStream, Dte)>,
    transparent: bool,
    to_dte: Vec<u8>,
    telnet_in: Option<TcpListener>,
    call: TelnetCall,
    /// The command line being typed, while it may matter to this end:
    /// for `ATS0=`, and for `ATA` while a telnet call rings.
    line: Vec<u8>,
    /// Rings before answering by itself, from `ATS0=`.
    s0: u32,
    /// The text after CONNECT on a telnet call.
    connect: String,
    /// The DOS program's DTR, last seen.
    dtr_was: bool,
    pub log: Vec<String>,
}

impl Station {
    pub fn new(modem: Modem, telnet_in: Option<TcpListener>, transparent: bool, connect: String) -> Self {
        Self {
            modem,
            dte: None,
            transparent,
            to_dte: Vec::new(),
            telnet_in,
            call: TelnetCall::None,
            line: Vec::new(),
            s0: 0,
            connect,
            dtr_was: true,
            log: Vec::new(),
        }
    }

    /// DOSBox has connected to the COM port.
    pub fn attach(&mut self, socket: TcpStream) {
        let _ = socket.set_nonblocking(true);
        let _ = socket.set_nodelay(true);
        self.log.push(format!("COM port: {}", socket.peer_addr().map_or("?".into(), |a| a.to_string())));
        self.dte = Some((socket, Dte::new(self.transparent)));
        self.to_dte.clear();
        self.dtr_was = true;
    }

    pub fn attached(&self) -> bool {
        self.dte.is_some()
    }

    fn say(&mut self, text: &str) {
        if let Some((_, dte)) = self.dte.as_ref() {
            dte.encode(text.as_bytes(), &mut self.to_dte);
        }
    }

    fn telnet_carrier(&self) -> bool {
        matches!(self.call, TelnetCall::Up { .. })
    }

    /// Socket work and the clock, `ms` since last time. Never blocks.
    pub fn service(&mut self, ms: u32) {
        self.accept_telnet();
        let from_dte = self.read_dte();
        if let Some((_, dte)) = self.dte.as_ref() {
            let dtr = dte.dtr;
            if self.dtr_was && !dtr {
                self.dtr_dropped();
            }
            self.dtr_was = dtr;
        }

        let online = match &self.call {
            TelnetCall::Up { online, .. } => Some(*online),
            _ => None,
        };
        match online {
            Some(true) => self.carry_telnet(&from_dte, ms),
            Some(false) => {
                // Escaped: a little command state of its own.
                for b in from_dte {
                    self.command_byte(b);
                }
                self.carry_telnet(&[], ms);
            }
            None => {
                for b in from_dte {
                    self.command_byte(b);
                }
                self.ring(ms);
            }
        }

        // What the modem has for the DOS program.
        let from_modem = self.modem.take_dte();
        if !from_modem.is_empty()
            && let Some((_, dte)) = self.dte.as_ref()
        {
            dte.encode(&from_modem, &mut self.to_dte);
        }

        let dcd = self.telnet_carrier() || matches!(self.modem.state(), State::Data | State::OnlineCommand);
        let cts = match &self.call {
            TelnetCall::Up { socket: _, to_caller, .. } => to_caller.len() < ROOM,
            _ => self.modem.queued() < ROOM,
        };
        if let Some((_, dte)) = self.dte.as_mut() {
            dte.lines(cts, dcd, &mut self.to_dte);
        }
        self.write_dte();
    }

    fn read_dte(&mut self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut gone = false;
        if let Some((socket, dte)) = self.dte.as_mut() {
            let mut buf = [0u8; 512];
            loop {
                match socket.read(&mut buf) {
                    Ok(0) => {
                        gone = true;
                        break;
                    }
                    Ok(n) => dte.decode(&buf[..n], &mut data),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        gone = true;
                        break;
                    }
                }
            }
        }
        if gone {
            // DOSBox closed: the cable is out, which drops DTR.
            self.log.push("COM port closed".into());
            self.dte = None;
            self.dtr_dropped();
        }
        data
    }

    fn write_dte(&mut self) {
        let Some((socket, _)) = self.dte.as_mut() else {
            self.to_dte.clear();
            return;
        };
        while !self.to_dte.is_empty() {
            match socket.write(&self.to_dte) {
                Ok(n) if n > 0 => {
                    self.to_dte.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                _ => break,
            }
        }
    }

    /// One byte typed at the modem while no call is being carried here.
    fn command_byte(&mut self, b: u8) {
        let held = matches!(self.call, TelnetCall::Ringing { .. } | TelnetCall::Up { online: false, .. });
        if !held {
            // The modem's: passed on, and the command line watched for ATS0=.
            let watching = matches!(self.modem.state(), State::Command | State::OnlineCommand);
            self.modem.feed_dte(b);
            if watching {
                match b {
                    b'\r' => {
                        self.take_line();
                    }
                    8 | 127 => {
                        self.line.pop();
                    }
                    _ => self.line.push(b),
                }
            }
            return;
        }
        // Held: this end decides what the line is for, and echoes as the
        // modem would.
        match b {
            b'\r' => {
                let line = self.take_line();
                self.held_line(&line);
            }
            8 | 127 => {
                self.line.pop();
                self.echo(b);
            }
            _ => {
                self.line.push(b);
                self.echo(b);
            }
        }
    }

    fn echo(&mut self, b: u8) {
        if let Some((_, dte)) = self.dte.as_ref() {
            dte.encode(&[b], &mut self.to_dte);
        }
    }

    /// The command line just ended, upper case without spaces; ATS0= noted.
    fn take_line(&mut self) -> String {
        let line: String = String::from_utf8_lossy(&self.line)
            .to_ascii_uppercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        self.line.clear();
        if let Some(at) = line.find("S0=") {
            let digits: String = line[at + 3..].chars().take_while(char::is_ascii_digit).collect();
            self.s0 = digits.parse().unwrap_or(0);
        }
        line
    }

    fn held_line(&mut self, line: &str) {
        if matches!(self.call, TelnetCall::Ringing { .. }) {
            if line == "ATA" {
                self.say("\r");
                self.answer_telnet();
            } else {
                // Not for this end after all: the modem has it.
                for c in line.bytes().chain(std::iter::once(b'\r')) {
                    self.modem.feed_dte(c);
                }
            }
            return;
        }
        self.say("\r\n");
        match line {
            "ATO" | "ATO0" => {
                self.say("CONNECT\r\n");
                if let TelnetCall::Up { online, .. } = &mut self.call {
                    *online = true;
                }
            }
            l if l.starts_with("ATH") => self.end_telnet("OK"),
            "" => {}
            _ => self.say("OK\r\n"),
        }
    }

    fn accept_telnet(&mut self) {
        let Some(listener) = self.telnet_in.as_ref() else { return };
        let Ok((mut socket, from)) = listener.accept() else { return };
        let busy = !matches!(self.call, TelnetCall::None)
            || self.modem.off_hook()
            || self.dte.is_none();
        if busy {
            let _ = socket.write_all(b"\r\nBUSY\r\n");
            self.log.push(format!("telnet call from {from}: busy"));
            return;
        }
        let _ = socket.set_nonblocking(true);
        let _ = socket.set_nodelay(true);
        self.log.push(format!("telnet call from {from}: ringing"));
        self.line.clear();
        // The first ring at once.
        self.call = TelnetCall::Ringing { socket, since_ms: RING_EVERY_MS, rings: 0 };
    }

    fn ring(&mut self, ms: u32) {
        let TelnetCall::Ringing { socket, since_ms, rings } = &mut self.call else { return };
        // A caller who gave up while it rang.
        let mut probe = [0u8; 64];
        let hung_up = match socket.read(&mut probe) {
            Ok(0) => true,
            Err(e) => e.kind() != ErrorKind::WouldBlock,
            Ok(_) => false,
        };
        if hung_up {
            self.log.push("telnet caller hung up while it rang".into());
            self.call = TelnetCall::None;
            return;
        }
        *since_ms += ms;
        if *since_ms < RING_EVERY_MS {
            return;
        }
        *since_ms = 0;
        *rings += 1;
        let rings = *rings;
        if rings > RINGS_BEFORE_GIVING_UP {
            if let TelnetCall::Ringing { socket, .. } = &mut self.call {
                let _ = socket.write_all(b"\r\nNO ANSWER\r\n");
            }
            self.log.push("telnet call not answered".into());
            self.call = TelnetCall::None;
            return;
        }
        self.say("\r\nRING\r\n");
        if self.s0 > 0 && rings >= self.s0 {
            self.answer_telnet();
        }
    }

    fn answer_telnet(&mut self) {
        let TelnetCall::Ringing { socket, .. } = std::mem::replace(&mut self.call, TelnetCall::None) else {
            return;
        };
        self.log.push("telnet call answered".into());
        let connect = format!("\r\n{}\r\n", self.connect);
        self.say(&connect);
        self.call = TelnetCall::Up {
            socket,
            server: TelnetServer::default(),
            to_caller: TelnetServer::greeting(),
            escape: Escape::default(),
            online: true,
        };
    }

    fn carry_telnet(&mut self, from_dte: &[u8], ms: u32) {
        let mut escaped_now = false;
        let mut gone = false;
        let mut data = Vec::new();
        {
            let TelnetCall::Up { socket, server, to_caller, escape, online } = &mut self.call else { return };
            if *online {
                for &b in from_dte {
                    escape.byte(b);
                }
                TelnetServer::encode(from_dte, to_caller);
                if escape.tick(ms) {
                    *online = false;
                    escaped_now = true;
                }
            }
            // Escaped, the caller's keys wait in the socket until ATO.
            if *online {
                let mut reply = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    match socket.read(&mut buf) {
                        Ok(0) => {
                            gone = true;
                            break;
                        }
                        Ok(n) => server.feed(&buf[..n], &mut data, &mut reply),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        Err(_) => {
                            gone = true;
                            break;
                        }
                    }
                }
                to_caller.splice(0..0, reply);
            }
            while !to_caller.is_empty() {
                match socket.write(to_caller) {
                    Ok(n) if n > 0 => {
                        to_caller.drain(..n);
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    _ => {
                        gone = true;
                        break;
                    }
                }
            }
        }
        if escaped_now {
            self.say("\r\nOK\r\n");
        }
        if !data.is_empty()
            && let Some((_, dte)) = self.dte.as_ref()
        {
            dte.encode(&data, &mut self.to_dte);
        }
        if gone {
            self.end_telnet("NO CARRIER");
        }
    }

    fn end_telnet(&mut self, result: &str) {
        if let TelnetCall::Up { socket, to_caller, .. } = &mut self.call {
            let _ = socket.set_nonblocking(false);
            let _ = socket.set_write_timeout(Some(Duration::from_millis(500)));
            let _ = socket.write_all(to_caller);
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if !matches!(self.call, TelnetCall::None) {
            self.log.push(format!("telnet call ended: {result}"));
        }
        self.call = TelnetCall::None;
        let text = format!("\r\n{result}\r\n");
        self.say(&text);
    }

    /// The DOS program dropped DTR: the call ends, as with `AT&D2`.
    fn dtr_dropped(&mut self) {
        match self.call {
            TelnetCall::Up { .. } => self.end_telnet("NO CARRIER"),
            _ => {
                if self.modem.off_hook() {
                    self.modem.hang_up();
                }
            }
        }
    }
}

/// `binmodem --com <port> [--telnet-in <port>] [--in <dev> --out <dev>]`.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut com: u16 = 5000;
    let mut telnet_in: Option<u16> = None;
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut carrier = "HST".to_owned();
    let mut transparent = false;
    // The COM port is the modem's control: only this machine, unless asked.
    // Telnet callers come from anywhere, which is the point of them.
    let mut bind = "127.0.0.1".to_owned();

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--com" => com = value().parse().unwrap_or(5000),
            "--telnet-in" => telnet_in = value().parse().ok(),
            "--in" => input = Some(value()),
            "--out" => output = Some(value()),
            "--carrier" => carrier = value().to_ascii_uppercase(),
            "--transparent" => transparent = true,
            "--bind" => bind = value(),
            "--help" | "-h" => {
                println!(
                    "binmodem --com <port> [--telnet-in <port>] [--carrier HST|HST96|V32B|...]\n\
                     \x20             [--in <device> --out <device>] [--transparent] [--bind <address>]\n\
                     \n\
                     A modem on a COM port for DOSBox: in dosbox.conf,\n\
                     \x20   serial1=nullmodem server:<this machine> port:<port>\n\
                     With --in and --out it dials and answers on that sound-card line.\n\
                     With --telnet-in, a telnet connection to that port rings the DOS\n\
                     program (RING), which answers with ATA or after ATS0= rings.\n\
                     --transparent if DOSBox is set transparent:1 (no carrier detect then).\n\
                     The COM port listens on 127.0.0.1 only; --bind 0.0.0.0 if DOSBox\n\
                     runs on another machine. Telnet calls are taken from anywhere."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let com_listener = match TcpListener::bind((bind.as_str(), com)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {bind}:{com}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let _ = com_listener.set_nonblocking(true);
    let telnet_listener = match telnet_in {
        Some(port) => match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => {
                let _ = l.set_nonblocking(true);
                Some(l)
            }
            Err(e) => {
                eprintln!("cannot listen for telnet on port {port}: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let audio = match (&input, &output) {
        (Some(i), Some(o)) => match line::Duplex::open(Some(i), Some(o), FS) {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("could not open the line: {e}");
                return ExitCode::FAILURE;
            }
        },
        (None, None) => None,
        _ => {
            eprintln!("--in and --out go together");
            return ExitCode::FAILURE;
        }
    };

    let mut modem = Modem::new(FS);
    for b in format!("AT+MS={carrier}\r").bytes() {
        modem.feed_dte(b);
    }
    modem.take_dte();
    let connect = match carrier.as_str() {
        "HST" => "CONNECT 14400/ARQ/HST",
        "HST96" => "CONNECT 9600/ARQ/HST",
        _ => "CONNECT 14400",
    };
    let mut station = Station::new(modem, telnet_listener, transparent, connect.to_owned());

    println!(
        "COM port on {bind}:{com}{}; modem {carrier} {}; ctrl-c to stop",
        telnet_in.map(|p| format!(", telnet calls on port {p}")).unwrap_or_default(),
        match &audio {
            Some(a) => format!("on {} / {}", a.input_device, a.output_device),
            None => "with no line (telnet calls only)".to_owned(),
        }
    );

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut last = Instant::now();
    let mut owed = 0.0f64;
    let mut last_phase = "";
    loop {
        if let Ok((socket, _)) = com_listener.accept() {
            if station.attached() {
                // One cable, one PC.
                drop(socket);
            } else {
                station.attach(socket);
            }
        }
        let now = Instant::now();
        let elapsed = now.duration_since(last);
        last = now;
        match &audio {
            Some(audio) => {
                from_line.clear();
                audio.receive(&mut from_line);
                to_line.clear();
                for &s in &from_line {
                    let level = if station.modem.exact_levels() { 1.0 } else { LEVEL };
                    to_line.push(station.modem.step(f64::from(s)) as f32 * level);
                }
                audio.transmit(&to_line);
            }
            None => {
                // No line: the modem's clocks still have to run.
                owed += elapsed.as_secs_f64() * FS;
                while owed >= 1.0 {
                    station.modem.step(0.0);
                    owed -= 1.0;
                }
            }
        }
        station.service(elapsed.as_millis().min(1000) as u32);
        let phase = station.modem.line_phase();
        if phase != last_phase {
            last_phase = phase;
            println!("[line: {phase}]");
        }
        for note in station.log.drain(..) {
            println!("[{note}]");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dosbox_lines_and_escapes_round_trip() {
        let mut dte = Dte::new(false);
        let mut data = Vec::new();
        dte.decode(&[b'A', 0xFF, 0xFF, b'T', 0xFF, 0x01], &mut data);
        assert_eq!(data, [b'A', 0xFF, b'T']);
        assert!(!dte.dtr, "control byte with bit 1 clear drops DTR");
        dte.decode(&[0xFF, 0x03], &mut data);
        assert!(dte.dtr);
        let mut out = Vec::new();
        dte.encode(&[0xFF, b'x'], &mut out);
        assert_eq!(out, [0xFF, 0xFF, b'x']);
        out.clear();
        dte.lines(true, true, &mut out);
        dte.lines(true, true, &mut out);
        assert_eq!(out, [0xFF, 0x03], "sent once, when it changes");
    }

    #[test]
    fn telnet_options_never_reach_the_program() {
        let mut t = TelnetServer::default();
        let (mut data, mut reply) = (Vec::new(), Vec::new());
        // PuTTY-ish: WILL NAWS, DO ECHO, a subnegotiation, then "hi" and Enter.
        t.feed(&[IAC, WILL, 31, IAC, DO, ECHO, IAC, SB, 24, 0, b'x', IAC, SE, b'h', b'i', b'\r', b'\n'], &mut data, &mut reply);
        assert_eq!(data, b"hi\r");
        assert_eq!(reply, [IAC, DONT, 31]);
    }

    /// DOSBox on the COM port, a telnet caller rings it, `ATA`, the two talk,
    /// and the DOS program dropping DTR ends the call.
    #[test]
    fn a_telnet_call_rings_the_dos_program_and_dtr_hangs_it_up() {
        let com = TcpListener::bind("127.0.0.1:0").unwrap();
        let telnet = TcpListener::bind("127.0.0.1:0").unwrap();
        telnet.set_nonblocking(true).unwrap();
        let (com_addr, telnet_addr) = (com.local_addr().unwrap(), telnet.local_addr().unwrap());

        let mut modem = Modem::new(FS);
        for b in b"AT+MS=HST\r" {
            modem.feed_dte(*b);
        }
        modem.take_dte();
        let mut station = Station::new(modem, Some(telnet), false, "CONNECT 14400/ARQ/HST".into());

        // "DOSBox".
        let mut dosbox = TcpStream::connect(com_addr).unwrap();
        dosbox.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
        station.attach(com.accept().unwrap().0);
        let mut seen = Vec::new();
        let run = |station: &mut Station, ms: u32, seen: &mut Vec<u8>, dosbox: &mut TcpStream| {
            for _ in 0..ms / 10 {
                for _ in 0..160 {
                    station.modem.step(0.0);
                }
                station.service(10);
                let mut buf = [0u8; 256];
                if let Ok(n) = dosbox.read(&mut buf) {
                    seen.extend_from_slice(&buf[..n]);
                }
            }
        };
        run(&mut station, 50, &mut seen, &mut dosbox);
        assert!(seen.ends_with(&[0xFF, 0x01]), "CTS up, no carrier: {seen:?}");

        // A caller telnets in.
        let mut caller = TcpStream::connect(telnet_addr).unwrap();
        caller.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
        run(&mut station, 100, &mut seen, &mut dosbox);
        assert!(String::from_utf8_lossy(&seen).contains("RING"), "{seen:?}");

        dosbox.write_all(b"ATA\r").unwrap();
        run(&mut station, 100, &mut seen, &mut dosbox);
        let text = String::from_utf8_lossy(&seen).into_owned();
        assert!(text.contains("CONNECT 14400/ARQ/HST"), "{text:?}");
        assert!(seen.ends_with(&[0xFF, 0x03]), "carrier now: {seen:?}");

        // The board speaks, the caller answers.
        dosbox.write_all(b"Welcome to the board\r\n").unwrap();
        caller.write_all(b"guest\r\n").unwrap();
        seen.clear();
        run(&mut station, 100, &mut seen, &mut dosbox);
        assert_eq!(seen, b"guest\r", "the caller's keys, Enter as a CR");
        let mut heard = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(n) = caller.read(&mut buf) {
            if n == 0 {
                break;
            }
            heard.extend_from_slice(&buf[..n]);
        }
        assert!(heard.starts_with(&TelnetServer::greeting()), "{heard:?}");
        assert!(String::from_utf8_lossy(&heard).contains("Welcome to the board"));

        // The BBS drops DTR: the call is over at both ends.
        dosbox.write_all(&[0xFF, 0x01]).unwrap();
        seen.clear();
        run(&mut station, 100, &mut seen, &mut dosbox);
        assert!(String::from_utf8_lossy(&seen).contains("NO CARRIER"), "{seen:?}");
        assert!(seen.ends_with(&[0xFF, 0x01]), "carrier gone: {seen:?}");
        caller.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        assert!(matches!(caller.read(&mut buf), Ok(0)), "the caller's connection closed");
    }

    #[test]
    fn ats0_answers_by_itself() {
        let com = TcpListener::bind("127.0.0.1:0").unwrap();
        let telnet = TcpListener::bind("127.0.0.1:0").unwrap();
        telnet.set_nonblocking(true).unwrap();
        let (com_addr, telnet_addr) = (com.local_addr().unwrap(), telnet.local_addr().unwrap());
        let mut station = Station::new(Modem::new(FS), Some(telnet), false, "CONNECT 9600/ARQ/HST".into());
        let mut dosbox = TcpStream::connect(com_addr).unwrap();
        dosbox.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
        station.attach(com.accept().unwrap().0);
        dosbox.write_all(b"ATS0=2\r").unwrap();
        let mut _caller = None;
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        for i in 0..1400 {
            if i == 20 {
                _caller = Some(TcpStream::connect(telnet_addr).unwrap());
            }
            for _ in 0..160 {
                station.modem.step(0.0);
            }
            station.service(10);
            if let Ok(n) = dosbox.read(&mut buf) {
                seen.extend_from_slice(&buf[..n]);
            }
        }
        let text = String::from_utf8_lossy(&seen).into_owned();
        assert_eq!(text.matches("RING").count(), 2, "{text:?}");
        assert!(text.contains("CONNECT 9600/ARQ/HST"), "{text:?}");
    }

    /// The DOS program dials an HST over the sound-card line: it gets the
    /// Courier's CONNECT, and carrier on its DCD.
    #[test]
    fn the_dos_program_dials_an_hst_over_the_line() {
        let com = TcpListener::bind("127.0.0.1:0").unwrap();
        let com_addr = com.local_addr().unwrap();
        let mut modem = Modem::new(FS);
        for b in b"AT+MS=HST\r" {
            modem.feed_dte(*b);
        }
        modem.take_dte();
        let mut station = Station::new(modem, None, false, String::new());
        let mut dosbox = TcpStream::connect(com_addr).unwrap();
        dosbox.set_nonblocking(true).unwrap();
        station.attach(com.accept().unwrap().0);

        let mut board = Modem::new(FS);
        for b in b"AT+MS=HST\rATA\r" {
            board.feed_dte(*b);
        }
        dosbox.write_all(b"ATDT5551212\r").unwrap();
        let (mut a, mut b) = (0.0, 0.0);
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        for _ in 0..(20.0 * FS) as usize / 160 {
            for _ in 0..160 {
                let (x, y) = (a, b);
                a = station.modem.step(0.5 * y);
                b = board.step(0.5 * x);
            }
            board.take_dte();
            station.service(10);
            if let Ok(n) = dosbox.read(&mut buf) {
                seen.extend_from_slice(&buf[..n]);
            }
            if seen.ends_with(&[0xFF, 0x03]) {
                break;
            }
        }
        let text = String::from_utf8_lossy(&seen).into_owned();
        assert!(text.contains("CONNECT 14400/ARQ/HST"), "{text:?}");
        assert!(seen.ends_with(&[0xFF, 0x03]), "carrier on DCD: {seen:?}");
    }

    /// `+++` with its guard time, then `ATH`, ends a telnet call.
    #[test]
    fn plus_plus_plus_and_ath_hang_up_a_telnet_call() {
        let com = TcpListener::bind("127.0.0.1:0").unwrap();
        let telnet = TcpListener::bind("127.0.0.1:0").unwrap();
        telnet.set_nonblocking(true).unwrap();
        let (com_addr, telnet_addr) = (com.local_addr().unwrap(), telnet.local_addr().unwrap());
        let mut station = Station::new(Modem::new(FS), Some(telnet), true, "CONNECT 14400/ARQ/HST".into());
        let mut dosbox = TcpStream::connect(com_addr).unwrap();
        dosbox.set_nonblocking(true).unwrap();
        station.attach(com.accept().unwrap().0);
        let mut caller = TcpStream::connect(telnet_addr).unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        let mut tick = |station: &mut Station, seen: &mut Vec<u8>, dosbox: &mut TcpStream, ms: u32| {
            for _ in 0..ms / 10 {
                station.service(10);
                if let Ok(n) = dosbox.read(&mut buf) {
                    seen.extend_from_slice(&buf[..n]);
                }
            }
        };
        tick(&mut station, &mut seen, &mut dosbox, 50);
        dosbox.write_all(b"ATA\r").unwrap();
        tick(&mut station, &mut seen, &mut dosbox, 1500);
        dosbox.write_all(b"+++").unwrap();
        tick(&mut station, &mut seen, &mut dosbox, 1500);
        assert!(String::from_utf8_lossy(&seen).ends_with("OK\r\n"), "{:?}", String::from_utf8_lossy(&seen));
        dosbox.write_all(b"ATH\r").unwrap();
        tick(&mut station, &mut seen, &mut dosbox, 100);
        assert!(String::from_utf8_lossy(&seen).ends_with("ATH\r\n\r\nOK\r\n"), "{:?}", String::from_utf8_lossy(&seen));
        caller.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut rest = Vec::new();
        let _ = caller.read_to_end(&mut rest);
        assert!(String::from_utf8_lossy(&rest).ends_with("+++"), "the caller saw the pluses, then the line close");
    }
}
