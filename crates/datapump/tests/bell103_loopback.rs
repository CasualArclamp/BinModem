//! Two 300 bit/s modems calling each other.
//!
//! The simplest call in the whole ladder, and worth having for exactly that
//! reason: the two directions are in different bands, so there is no echo to
//! cancel; the tones are read by a discriminator, so there is no carrier to
//! recover; and the framing re-acquires on every start bit, so there is no
//! timing loop to hold still. If a call will not go through here, nothing
//! above it is the reason.

use datapump::bell103::{Modem, Role, Status};

const FS: f64 = 16_000.0;

/// A two-wire line: both ends hear the sum, each attenuated on its way.
///
/// The near end comes back louder than the far one, as a hybrid delivers it.
/// It does not matter here and that is the point — the two directions are
/// 750 Hz apart and the band filter throws one away with the other's echo in
/// it — but a test that left it out would not be testing that.
const ECHO: f64 = 0.251;
const FAR: f64 = 0.1;

struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: f64,
    from_host: f64,
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
    connected_at: f64,
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
            connected_at: f64::NAN,
        }
    }

    fn run(&mut self, seconds: f64) {
        let start = self.connected_at;
        let _ = start;
        for i in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(b * FAR + a * ECHO);
            self.from_host = self.host.step(a * FAR + b * ECHO);
            self.at_caller.extend(self.caller.take_bytes());
            self.at_host.extend(self.host.take_bytes());
            if self.connected_at.is_nan() && self.up() {
                self.connected_at = i as f64 / FS;
            }
        }
    }

    fn up(&self) -> bool {
        matches!(self.caller.status(), Status::Connected(_))
            && matches!(self.host.status(), Status::Connected(_))
    }
}

#[test]
fn a_call_comes_up_without_anything_that_could_be_called_a_handshake() {
    let mut p = Pair::new();
    p.run(6.0);
    assert!(
        p.up(),
        "the caller stopped at {} and the host at {}",
        p.caller.line_phase(),
        p.host.line_phase()
    );
    assert_eq!(p.caller.status(), Status::Connected(300));
    println!("connected after {:.2} s", p.connected_at);
    // The answering end waits a second before whistling, the calling end waits
    // out half a second of that whistle before answering it, and both want a
    // moment of steady mark after that. Anything much longer means something
    // is being waited for that should not be.
    assert!(
        p.connected_at < 3.0,
        "took {:.2} s to bring up a call with no handshake in it",
        p.connected_at
    );
}

#[test]
fn typing_at_one_end_comes_out_at_the_other() {
    let mut p = Pair::new();
    p.run(6.0);
    assert!(p.up(), "never connected");

    let banner = b"\r\nThe Dead Zone BBS - 14.4k - Est. 1991\r\nLogin: ";
    let typed = b"guest\r";
    p.host.send(banner);
    p.caller.send(typed);
    // 300 bit/s is thirty characters a second, and there are forty-seven of
    // them, so this needs to be generous or it is testing the clock.
    p.run(3.0);

    assert_eq!(
        String::from_utf8_lossy(&p.at_caller),
        String::from_utf8_lossy(banner),
        "the calling end did not get the banner"
    );
    assert_eq!(
        String::from_utf8_lossy(&p.at_host),
        String::from_utf8_lossy(typed),
        "the answering end did not get what was typed"
    );
}

#[test]
fn a_call_to_nobody_gives_up_rather_than_waiting_for_ever() {
    let mut caller = Modem::new(Role::Originate, FS);
    let mut status = Status::Negotiating;
    for _ in 0..(62.0 * FS) as usize {
        caller.step(0.0);
        status = caller.status();
        if status == Status::Failed {
            break;
        }
    }
    assert_eq!(status, Status::Failed);
}

#[test]
fn an_answer_tone_is_waited_out_rather_than_talked_over() {
    // 2100 Hz sits inside the answering band and trips its carrier detector,
    // but below the band centre, so it reads as a space and never as the idle
    // mark that says the far end is ready. A modem that watched only for
    // carrier would declare a connection three seconds early and put its login
    // into the tone.
    let mut caller = Modem::new(Role::Originate, FS);
    let mut phase = 0.0f64;
    for _ in 0..(3.0 * FS) as usize {
        phase += 2100.0 / FS;
        phase -= phase.floor();
        caller.step((phase * std::f64::consts::TAU).cos());
    }
    assert_eq!(
        caller.status(),
        Status::Negotiating,
        "connected to an answer tone, at {}",
        caller.line_phase()
    );
    // It should have answered the tone with its own carrier, though: that is
    // what a calling modem is supposed to do, and what a far end is listening
    // for before it starts sending data.
    assert_eq!(caller.line_phase(), "mark");
}
