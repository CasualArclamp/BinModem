//! Bell 212A calling Bell 212A, and Bell 212A meeting a Bell 103 at either
//! end of the call.

use datapump::bell103::{self, Role};
use datapump::bell212::{self, Status};
use datapump::framing::AsyncBits;
use datapump::v22bis::handshake as v22;

const FS: f64 = 16_000.0;
/// A two-wire line, as in the Bell 103 loopback: each end hears the other
/// attenuated and its own transmitter louder, as a hybrid hands it back.
const ECHO: f64 = 0.251;
const FAR: f64 = 0.1;

/// Either kind of modem at either end.
enum End {
    A212(Box<bell212::Modem>),
    B103(Box<bell103::Modem>),
    /// A V.22bis modem, and the character framing over its bits.
    V22(Box<v22::Modem>, AsyncBits),
}

impl End {
    fn step(&mut self, line: f64) -> f64 {
        match self {
            Self::A212(m) => m.step(line),
            Self::B103(m) => m.step(line),
            Self::V22(m, _) => m.step(line),
        }
    }

    fn rate(&self) -> Option<u32> {
        match self {
            Self::A212(m) => match m.status() {
                Status::Connected(r) => Some(r),
                _ => None,
            },
            Self::B103(m) => match m.status() {
                bell103::Status::Connected(r) => Some(r),
                _ => None,
            },
            Self::V22(m, _) => match m.status() {
                v22::Status::Connected(r) => Some(r.bits_per_second()),
                _ => None,
            },
        }
    }

    fn phase(&self) -> &'static str {
        match self {
            Self::A212(m) => m.phase(),
            Self::B103(m) => m.line_phase(),
            Self::V22(m, _) => m.phase(),
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        match self {
            Self::A212(m) => m.send(bytes),
            Self::B103(m) => m.send(bytes),
            Self::V22(m, framing) => {
                let bits: Vec<bool> = bytes.iter().flat_map(|&b| framing.encode(b)).collect();
                m.send_bits(&bits);
            }
        }
    }

    fn take_bytes(&mut self) -> Vec<u8> {
        match self {
            Self::A212(m) => m.take_bytes(),
            Self::B103(m) => m.take_bytes(),
            Self::V22(m, framing) => {
                let bits = m.take_bits();
                if m.status() == v22::Status::Negotiating {
                    return Vec::new();
                }
                bits.into_iter().filter_map(|b| framing.feed(b)).collect()
            }
        }
    }
}

struct Pair {
    caller: End,
    host: End,
    from_caller: f64,
    from_host: f64,
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
    connected_at: f64,
    t: f64,
    /// RMS of white noise added at each end's input, and its generator.
    noise: f64,
    seed: u64,
}

impl Pair {
    fn new(caller: End, host: End) -> Self {
        Self {
            caller,
            host,
            from_caller: 0.0,
            from_host: 0.0,
            at_caller: Vec::new(),
            at_host: Vec::new(),
            connected_at: f64::NAN,
            t: 0.0,
            noise: 0.0,
            seed: 0x2125_1200,
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            let (na, nb) = (self.gaussian() * self.noise, self.gaussian() * self.noise);
            self.from_caller = self.caller.step(b * FAR + a * ECHO + na);
            self.from_host = self.host.step(a * FAR + b * ECHO + nb);
            self.at_caller.extend(self.caller.take_bytes());
            self.at_host.extend(self.host.take_bytes());
            self.t += 1.0 / FS;
            if self.connected_at.is_nan() && self.up() {
                self.connected_at = self.t;
            }
        }
    }

    /// Box-Muller over a little LCG: enough noise for a test, no crate.
    fn gaussian(&mut self) -> f64 {
        let mut uniform = || {
            self.seed = self
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (u, v) = (uniform(), uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    fn up(&self) -> bool {
        self.caller.rate().is_some() && self.host.rate().is_some()
    }

    fn connect(&mut self) {
        self.run(8.0);
        assert!(
            self.up(),
            "the caller stopped at {} and the host at {}",
            self.caller.phase(),
            self.host.phase()
        );
        println!("connected after {:.2} s", self.connected_at);
    }

    /// Both ways at once, which is what full duplex is for.
    fn exchange(&mut self, seconds: f64) {
        let banner = b"\r\nWelcome to the board. 1200 baud, 8N1.\r\nName: ";
        let typed = b"guest\r";
        self.host.send(banner);
        self.caller.send(typed);
        self.run(seconds);
        assert_eq!(
            String::from_utf8_lossy(&self.at_caller),
            String::from_utf8_lossy(banner)
        );
        assert_eq!(
            String::from_utf8_lossy(&self.at_host),
            String::from_utf8_lossy(typed)
        );
    }
}

fn a212(role: Role) -> End {
    End::A212(Box::new(bell212::Modem::new(role, FS)))
}

fn b103(role: Role) -> End {
    End::B103(Box::new(bell103::Modem::new(role, FS)))
}

#[test]
fn two_212as_connect_at_1200_and_carry_data_both_ways() {
    let mut p = Pair::new(a212(Role::Originate), a212(Role::Answer));
    p.connect();
    assert_eq!(p.caller.rate(), Some(1200));
    assert_eq!(p.host.rate(), Some(1200));
    // Billing delay 2 s, tone heard 155 ms, pause 456 ms, then 270 ms of
    // the offer heard and 765 ms of settling: a little under four seconds.
    assert!(p.connected_at < 5.0, "took {:.2} s", p.connected_at);
    if let End::A212(m) = &p.caller {
        assert_eq!(m.standard(), "Bell 212A");
    }
    p.exchange(1.0);
}

#[test]
fn a_212a_calling_a_103_falls_back_to_300() {
    let mut p = Pair::new(a212(Role::Originate), b103(Role::Answer));
    p.connect();
    assert_eq!(p.caller.rate(), Some(300));
    assert_eq!(p.host.rate(), Some(300));
    if let End::A212(m) = &p.caller {
        assert!(m.is_bell103());
        assert_eq!(m.standard(), "Bell 103");
    }
    p.run(0.5);
    p.exchange(3.0);
}

#[test]
fn a_103_calling_a_212a_is_answered_at_300() {
    let mut p = Pair::new(b103(Role::Originate), a212(Role::Answer));
    p.connect();
    assert_eq!(p.caller.rate(), Some(300));
    assert_eq!(p.host.rate(), Some(300));
    if let End::A212(m) = &p.host {
        assert!(m.is_bell103());
    }
    p.run(0.5);
    p.exchange(3.0);
}

#[test]
fn a_call_to_nobody_gives_up() {
    let mut caller = bell212::Modem::new(Role::Originate, FS);
    for _ in 0..(61.0 * FS) as usize {
        caller.step(0.0);
    }
    assert_eq!(caller.status(), Status::Failed);
}

/// V.22 is the 212A's modulation with a different handshake in front of it,
/// and real 212As and V.22 modems did reach each other at 1200: the calling
/// 212A takes V.22's unscrambled ones (2250 Hz) for its answer tone, and a
/// V.22bis answering modem that hears scrambled ones and no offer of 2400
/// settles at 1200.
#[test]
fn a_212a_calling_a_v22bis_modem_connects_at_1200() {
    let mut p = Pair::new(
        a212(Role::Originate),
        End::V22(
            Box::new(v22::Modem::new(v22::Role::Answering, FS)),
            AsyncBits::new(8),
        ),
    );
    p.connect();
    assert_eq!(p.caller.rate(), Some(1200));
    assert_eq!(p.host.rate(), Some(1200));
    p.exchange(1.0);
}

/// 20 dB below the far signal -- a poor line, but one 1200 bit/s DPSK works
/// on -- and the offer of 1200 must still not be taken for a 103's mark.
#[test]
fn two_212as_on_a_noisy_line_still_connect_at_1200() {
    let mut p = Pair::new(a212(Role::Originate), a212(Role::Answer));
    // The far signal is 0.1 of unit power: 20 dB below it is 0.01 RMS.
    p.noise = FAR * 0.1;
    p.connect();
    assert_eq!(p.caller.rate(), Some(1200));
    assert_eq!(p.host.rate(), Some(1200));
    p.exchange(1.0);
}
