//! An answered call passed through to a bulletin board over telnet.
//!
//! What a sysop with a modem on the wall did, and what a board like WWIV
//! still expects to be reached by: the modem answers, and whoever called is
//! put through to the board as though they had dialled it directly. Modern
//! boards take their callers over telnet, so the far side here is a socket
//! and the [`telnet`] crate's option handling; the near side is the modem's
//! DTE, which is what a serial port to the board used to be.
//!
//! Nothing is interpreted in either direction. The board's ANSI and CP437 go
//! to the caller as they are, and what the caller types goes to the board.
//! Either end going away ends it: the board closing the socket hangs the
//! modem up, and the call ending closes the socket.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use modem::Modem;
use telnet::Telnet;

/// How long to wait for the board to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Octets queued in the modem before the board is read any further. The
/// board then waits on TCP, which is how a board on a serial port was held
/// back by CTS: an HST at 14 400 empties this in a second or so.
const MODEM_ROOM: usize = 2048;

/// The terminal the board is told the caller has, and its size. A caller
/// on BinModem's own terminal is ANSI at 80 by 25, and so was nearly every
/// caller a board ever had.
const TERMINAL: (&str, u16, u16) = ("ANSI", 80, 25);

/// Why the board is no longer on the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gone {
    /// The board closed the connection: logged off, or dropped the caller.
    Closed,
    /// The socket failed.
    Failed(String),
}

/// One caller's connection to the board.
#[derive(Debug)]
pub struct Bridge {
    socket: TcpStream,
    telnet: Telnet,
    /// Encoded for the socket and not yet taken by it.
    to_board: Vec<u8>,
    /// Octets each way, for the log.
    pub sent: u64,
    pub received: u64,
}

impl Bridge {
    /// Connect to the board at `address`: a host, or a host and port, with
    /// telnet's 23 when no port is given.
    pub fn connect(address: &str) -> Result<Self, String> {
        let with_port = if address.contains(':') { address.to_owned() } else { format!("{address}:23") };
        let addrs = with_port
            .to_socket_addrs()
            .map_err(|e| format!("cannot find {with_port}: {e}"))?;
        let mut last = format!("no address for {with_port}");
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(socket) => {
                    socket.set_nonblocking(true).map_err(|e| e.to_string())?;
                    let _ = socket.set_nodelay(true);
                    let (kind, cols, rows) = TERMINAL;
                    return Ok(Self {
                        socket,
                        telnet: Telnet::new(kind, cols, rows),
                        to_board: Vec::new(),
                        sent: 0,
                        received: 0,
                    });
                }
                Err(e) => last = format!("{addr}: {e}"),
            }
        }
        Err(last)
    }

    /// Carry what the caller typed to the board, and what the board sent to
    /// the caller. Call it often; it never blocks.
    pub fn pump(&mut self, host: &mut Modem, from_caller: &[u8]) -> Result<(), Gone> {
        if !from_caller.is_empty() {
            self.telnet.encode(from_caller, &mut self.to_board);
            self.sent += from_caller.len() as u64;
        }

        // From the board, as much as the modem has room for.
        let mut buf = [0u8; 1024];
        while host.queued() < MODEM_ROOM {
            match self.socket.read(&mut buf) {
                Ok(0) => return Err(Gone::Closed),
                Ok(n) => {
                    let mut data = Vec::with_capacity(n);
                    self.telnet.feed_bytes(&buf[..n], &mut data);
                    self.received += data.len() as u64;
                    for b in data {
                        host.feed_dte(b);
                    }
                    // Option replies go back ahead of anything typed.
                    let reply = self.telnet.take_reply();
                    if !reply.is_empty() {
                        self.to_board.splice(0..0, reply);
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(Gone::Failed(e.to_string())),
            }
        }

        // To the board, as much as the socket will take.
        while !self.to_board.is_empty() {
            match self.socket.write(&self.to_board) {
                Ok(0) => return Err(Gone::Closed),
                Ok(n) => {
                    self.to_board.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(Gone::Failed(e.to_string())),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modem::State;
    use std::net::TcpListener;

    const FS: f64 = 16_000.0;

    fn type_at(m: &mut Modem, line: &str) {
        for b in line.bytes() {
            m.feed_dte(b);
        }
        m.feed_dte(b'\r');
    }

    /// A caller dials an HST modem that is passing calls to a board; the
    /// board's banner reaches the caller's terminal, what the caller types
    /// reaches the board, and when the board logs them off its goodbye
    /// arrives before the call ends.
    #[test]
    fn an_hst_caller_reaches_the_board_and_the_board_hears_them() {
        // The board: says who it is, then waits for a name.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let board = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"\x1b[1;37mWWIV test board\r\nNN: ").unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 64];
            s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
            while !got.windows(5).any(|w| w == b"SYSOP") {
                let n = s.read(&mut buf).unwrap();
                assert!(n > 0, "the caller went away; got {got:?}");
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"\r\nGoodbye.\r\n").unwrap();
            got
        });

        let mut caller = Modem::new(FS);
        let mut host = Modem::new(FS);
        type_at(&mut caller, "AT+MS=HST");
        type_at(&mut host, "AT+MS=HST");
        type_at(&mut host, "ATA");
        type_at(&mut caller, "ATD1");
        let (mut a, mut b) = (0.0, 0.0);
        let mut bridge: Option<Bridge> = None;
        let mut screen = Vec::new();
        let mut typed = false;
        let mut closed = false;
        let mut heard = Vec::new();
        for i in 0..(60.0 * FS) as usize {
            let (x, y) = (a, b);
            a = caller.step(0.5 * y);
            b = host.step(0.5 * x);
            screen.extend(caller.take_dte());
            heard.extend(host.take_dte());
            if i % 16 != 0 {
                continue;
            }
            if bridge.is_none() && host.state() == State::Data && !closed {
                bridge = Some(Bridge::connect(&address).expect("the board is listening"));
            }
            if let Some(br) = bridge.as_mut()
                && br.pump(&mut host, &std::mem::take(&mut heard)) == Err(Gone::Closed)
            {
                bridge = None;
                closed = true;
            }
            let text = String::from_utf8_lossy(&screen).into_owned();
            if !typed && text.contains("NN: ") {
                typed = true;
                for c in b"SYSOP\r" {
                    caller.feed_dte(*c);
                }
            }
            // The board has gone: hang up once its last words are out,
            // as `--answer --bbs` does.
            if closed && host.state() == State::Data && host.queued() == 0 {
                host.hang_up();
            }
            if text.contains("NO CARRIER") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&screen).into_owned();
        assert!(text.contains("CONNECT 14400/ARQ/HST"), "{text:?}");
        assert!(text.contains("\x1b[1;37mWWIV test board"), "the banner, colour and all: {text:?}");
        assert!(text.contains("Goodbye."), "the board's last screen before the line went: {text:?}");
        // MNP's LD hangs the caller up at once rather than after a silence.
        assert!(text.contains("NO CARRIER"), "{text:?}");
        assert_eq!(caller.state(), State::Command);
        let got = board.join().unwrap();
        assert!(got.windows(5).any(|w| w == b"SYSOP"), "{got:?}");
    }
}
