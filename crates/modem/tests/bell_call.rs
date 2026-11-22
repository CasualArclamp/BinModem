//! Bell 212A and Bell 202 calls, driven from the terminal with `+MS`.

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

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(b);
            self.from_host = self.host.step(a);
            self.at_caller.extend(self.caller.take_dte());
            self.at_host.extend(self.host.take_dte());
        }
    }

    fn caller_saw(&self) -> String {
        String::from_utf8_lossy(&self.at_caller).into_owned()
    }

    fn host_saw(&self) -> String {
        String::from_utf8_lossy(&self.at_host).into_owned()
    }

    /// Set each end's `+MS`, answer and dial, and wait.
    fn call(caller_ms: &str, host_ms: &str, seconds: f64) -> Self {
        let mut p = Self::new();
        Self::type_at(&mut p.host, &format!("AT+MS={host_ms}"));
        Self::type_at(&mut p.caller, &format!("AT+MS={caller_ms}"));
        p.run(0.01);
        Self::type_at(&mut p.host, "ATA");
        Self::type_at(&mut p.caller, "ATD5551234");
        p.run(seconds);
        assert_eq!(p.caller.state(), State::Data, "caller: {:?}", p.caller_saw());
        assert_eq!(p.host.state(), State::Data, "host: {:?}", p.host_saw());
        p
    }

    /// Type at each end in turn and see it come out at the other.
    fn converse(&mut self, settle: f64, wait: f64) {
        self.run(settle);
        self.at_caller.clear();
        self.at_host.clear();
        for b in b"cactus\r\n" {
            self.caller.feed_dte(*b);
        }
        self.run(wait);
        assert!(self.host_saw().contains("cactus"), "host saw {:?}", self.host_saw());
        for b in b"Password:" {
            self.host.feed_dte(*b);
        }
        self.run(wait);
        assert!(self.caller_saw().contains("Password:"), "caller saw {:?}", self.caller_saw());
    }
}

#[test]
fn b212_at_both_ends_connects_at_1200() {
    let mut p = Pair::call("B212", "B212", 8.0);
    assert_eq!(p.caller.rate(), Some(1200));
    assert_eq!(p.caller.standard(), "Bell 212A");
    assert_eq!(p.host.standard(), "Bell 212A");
    assert!(p.caller_saw().contains("1200"), "{:?}", p.caller_saw());
    p.converse(2.0, 2.0);
}

#[test]
fn b212_calling_a_b103_falls_back_to_300() {
    let mut p = Pair::call("B212", "B103", 8.0);
    assert_eq!(p.caller.rate(), Some(300));
    assert_eq!(p.caller.standard(), "Bell 103");
    assert!(p.caller_saw().contains("300"), "{:?}", p.caller_saw());
    // 300 bit/s carries no error control, so nothing to wait for.
    p.converse(0.5, 2.0);
}

#[test]
fn b103_calling_a_b212_is_answered_at_300() {
    let mut p = Pair::call("B103", "B212", 8.0);
    assert_eq!(p.host.rate(), Some(300));
    assert_eq!(p.host.standard(), "Bell 103");
    p.converse(0.5, 2.0);
}

#[test]
fn b202_at_both_ends_takes_turns_at_1200() {
    let mut p = Pair::call("B202", "B202", 8.0);
    assert_eq!(p.caller.rate(), Some(1200));
    assert_eq!(p.caller.standard(), "Bell 202");
    assert!(!p.caller.error_controlled(), "error control over half duplex");
    p.converse(0.2, 1.0);
}
