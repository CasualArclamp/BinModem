//! Two Bell 202s taking turns on one band.

use datapump::bell103::Role;
use datapump::bell202::{Modem, Status};

const FS: f64 = 16_000.0;
const ECHO: f64 = 0.251;
const FAR: f64 = 0.1;

struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: f64,
    from_host: f64,
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
    both_keyed: usize,
}

impl Pair {
    fn new() -> Self {
        Self {
            caller: Modem::new(Role::Originate, FS),
            host: Modem::new(Role::Answer, FS),
            from_caller: 0.0,
            from_host: 0.0,
            at_caller: Vec::new(),
            at_host: Vec::new(),
            both_keyed: 0,
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(b * FAR + a * ECHO);
            self.from_host = self.host.step(a * FAR + b * ECHO);
            self.at_caller.extend(self.caller.take_bytes());
            self.at_host.extend(self.host.take_bytes());
            if self.caller.transmitting() && self.host.transmitting() {
                self.both_keyed += 1;
            }
        }
    }

    fn up(&self) -> bool {
        matches!(self.caller.status(), Status::Connected(1200))
            && matches!(self.host.status(), Status::Connected(1200))
    }
}

#[test]
fn a_call_comes_up_and_both_ends_go_quiet() {
    let mut p = Pair::new();
    p.run(6.0);
    assert!(
        p.up(),
        "caller {} host {}",
        p.caller.phase(),
        p.host.phase()
    );
    assert!(!p.caller.transmitting() && !p.host.transmitting());
    assert!(
        p.at_caller.is_empty() && p.at_host.is_empty(),
        "nothing was sent"
    );
}

#[test]
fn each_end_takes_its_turn() {
    let mut p = Pair::new();
    p.run(6.0);
    assert!(p.up());
    let banner = b"\r\nCONNECT 1200 - half duplex, one at a time.\r\nName: ";
    p.host.send(banner);
    p.run(1.0);
    assert_eq!(
        String::from_utf8_lossy(&p.at_caller),
        String::from_utf8_lossy(banner)
    );
    assert!(
        p.at_host.is_empty(),
        "the host heard itself: {:?}",
        p.at_host
    );
    let typed = b"guest\r";
    p.caller.send(typed);
    p.run(0.5);
    assert_eq!(
        String::from_utf8_lossy(&p.at_host),
        String::from_utf8_lossy(typed)
    );
    assert_eq!(
        String::from_utf8_lossy(&p.at_caller),
        String::from_utf8_lossy(banner),
        "the caller heard itself"
    );
    assert_eq!(p.caller.framing_errors() + p.host.framing_errors(), 0);
}

#[test]
fn two_ends_with_something_to_say_at_once_do_not_both_key() {
    let mut p = Pair::new();
    p.run(6.0);
    assert!(p.up());
    let a = b"from the caller, sent at the same moment\r\n";
    let b = b"from the host, sent at the same moment\r\n";
    p.caller.send(a);
    p.host.send(b);
    p.run(2.0);
    assert_eq!(p.both_keyed, 0, "both ends had the line at once");
    assert_eq!(
        String::from_utf8_lossy(&p.at_host),
        String::from_utf8_lossy(a)
    );
    assert_eq!(
        String::from_utf8_lossy(&p.at_caller),
        String::from_utf8_lossy(b)
    );
}
