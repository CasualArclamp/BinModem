//! Two modems in an HST call, driven from their terminals.
//!
//! `AT+MS=HST` at both ends, `ATA` at one and `ATD` at the other: the answer
//! tone, the session after it, MNP, and a `CONNECT .../ARQ/HST` on each
//! terminal. Then characters typed at either end come out at the other, which
//! in HST means over whichever channel that end holds at the time.

use modem::{Modem, State};

const FS: f64 = 16_000.0;

struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: f64,
    from_host: f64,
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
}

impl Pair {
    fn new() -> Self {
        Self {
            caller: Modem::new(FS),
            host: Modem::new(FS),
            from_caller: 0.0,
            from_host: 0.0,
            at_caller: Vec::new(),
            at_host: Vec::new(),
        }
    }

    fn type_at(modem: &mut Modem, line: &str) {
        for b in line.bytes() {
            modem.feed_dte(b);
        }
        modem.feed_dte(b'\r');
    }

    /// Run the line, a little loss each way, until `done` or `seconds`.
    fn run_until(&mut self, seconds: f64, mut done: impl FnMut(&Self) -> bool) -> bool {
        for i in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(0.5 * b);
            self.from_host = self.host.step(0.5 * a);
            self.at_caller.extend(self.caller.take_dte());
            self.at_host.extend(self.host.take_dte());
            if i % 160 == 0 && done(self) {
                return true;
            }
        }
        done(self)
    }

    fn saw(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn connect(caller_ms: &str, host_ms: &str) -> Pair {
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, &format!("AT+MS={host_ms}"));
    Pair::type_at(&mut p.caller, &format!("AT+MS={caller_ms}"));
    p.run_until(0.05, |_| false);
    p.at_caller.clear();
    p.at_host.clear();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    let up = p.run_until(30.0, |p| p.caller.state() == State::Data && p.host.state() == State::Data);
    assert!(
        up,
        "no connection: caller {:?} said {:?}, host {:?} said {:?}",
        p.caller.state(),
        Pair::saw(&p.at_caller),
        p.host.state(),
        Pair::saw(&p.at_host)
    );
    p
}

#[test]
fn hst_is_a_modulation_the_terminal_can_ask_for() {
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+MS=?");
    p.run_until(0.05, |_| false);
    assert!(Pair::saw(&p.at_caller).contains("HST"), "{:?}", Pair::saw(&p.at_caller));
}

#[test]
fn an_hst_call_connects_at_14400_and_says_so_like_a_courier() {
    let p = connect("HST", "HST");
    for (name, saw) in [("caller", Pair::saw(&p.at_caller)), ("host", Pair::saw(&p.at_host))] {
        assert!(saw.contains("CONNECT 14400/ARQ/HST"), "the {name} was told {saw:?}");
    }
    assert_eq!(p.caller.rate(), Some(14_400));
    assert_eq!(p.host.rate(), Some(14_400));
}

#[test]
fn ms_max_rate_limits_what_is_offered() {
    // The caller will go no faster than 9600, so neither end does.
    let p = connect("HST,0,0,9600", "HST");
    assert!(Pair::saw(&p.at_caller).contains("CONNECT 9600/ARQ/HST"), "{:?}", Pair::saw(&p.at_caller));
    assert_eq!(p.host.rate(), Some(9600));
}

#[test]
fn typing_crosses_an_hst_call_both_ways_and_ath_hangs_up() {
    let mut p = connect("HST", "HST");
    p.at_caller.clear();
    p.at_host.clear();
    // The host holds the forward channel; the caller types over the back.
    for b in b"from the caller\r\n" {
        p.caller.feed_dte(*b);
    }
    let text: Vec<u8> = (0..1500).map(|i| b"the host sends a screenful. "[i % 28]).collect();
    for &b in &text {
        p.host.feed_dte(b);
    }
    let crossed = p.run_until(20.0, |p| {
        Pair::saw(&p.at_host).contains("from the caller") && p.at_caller.len() >= text.len()
    });
    assert!(
        crossed,
        "host got {:?}; caller got {} of {} octets",
        Pair::saw(&p.at_host),
        p.at_caller.len(),
        text.len()
    );
    assert_eq!(&p.at_caller[..text.len()], &text[..]);

    // Escape, hang up, and the far end notices.
    p.run_until(1.2, |_| false);
    for b in b"+++" {
        p.caller.feed_dte(*b);
    }
    p.run_until(1.5, |_| false);
    assert_eq!(p.caller.state(), State::OnlineCommand, "{:?}", Pair::saw(&p.at_caller));
    Pair::type_at(&mut p.caller, "ATH");
    let gone = p.run_until(15.0, |p| p.caller.state() == State::Command && p.host.state() == State::Command);
    assert!(gone, "host is still {:?} and said {:?}", p.host.state(), Pair::saw(&p.at_host));
    assert!(Pair::saw(&p.at_host).contains("NO CARRIER"), "{:?}", Pair::saw(&p.at_host));
}
