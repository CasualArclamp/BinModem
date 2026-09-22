//! A V.32bis call over a sound card loopback that misbehaves the way a real
//! one does.
//!
//! `soundcard_loop.rs` puts two modems on an ideal cable: their outputs summed
//! onto one wire at `modem-loop`'s headroom and heard back by both one
//! crossing later, every sample exactly where it was written. A real card is
//! not that. It writes on one clock and reads on another, a few parts per
//! million apart, so what comes back drifts against what went out; it drops a
//! sample now and then when a buffer is late; and when the output runs dry it
//! plays silence until it is fed again, which delays everything after.
//!
//! On that cable the modem's own signal comes back at full strength, drifting
//! and slipping, and the far end is only as loud as it. This is
//! `docs/design/v32-rebuild/design.md` §9.2's cable check, made through the
//! modem crate as `modem-loop` makes it: the call has to come up at 14 400,
//! carry the greeting, and ride out every fault without a retrain and without
//! the terminal ever being told NO CARRIER.

use std::collections::VecDeque;

use dsp::Resampler;
use modem::{Modem, State};

const FS: f64 = 16_000.0;

/// Headroom for the sum of two modems on one pair, as `modem-loop` applies it.
const HEADROOM: f64 = 0.45;

/// One crossing of the cable, in samples, as `soundcard_loop.rs` measured it.
const CROSSING: usize = 700;

/// How far apart the card's two clocks are.
const PPM: f64 = 20.0;

/// Samples the resampler holds back while its kernel fills.
const RESAMPLER_LEAD: usize = 16;

/// What the card does wrong, once.
#[derive(Debug, Clone, Copy)]
enum Fault {
    /// One sample read and lost.
    Drop,
    /// The output ran dry for this many samples and played silence.
    Gap(usize),
}

/// Two modems on one cable, with the card between them.
struct Cable {
    caller: Modem,
    host: Modem,
    /// What was written, read back on the other clock.
    clock: Resampler,
    read: Vec<f64>,
    /// The samples crossing: the card's buffers, both ways.
    wire: VecDeque<f64>,
    /// Faults to come, soonest first, by the sample they happen at.
    faults: VecDeque<(usize, Fault)>,
    now: usize,
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
}

impl Cable {
    fn new() -> Self {
        let mut caller = Modem::new(FS);
        let mut host = Modem::new(FS);
        for m in [&mut caller, &mut host] {
            // One modulation over one line: with automode on the two ends
            // would settle on whatever they liked best.
            for b in "AT+MS=V32B,0\r".bytes() {
                m.feed_dte(b);
            }
            m.take_dte();
        }
        for b in b"ATA\r" {
            host.feed_dte(*b);
        }
        for b in b"ATD5551234\r" {
            caller.feed_dte(*b);
        }
        Self {
            caller,
            host,
            // The card reads faster than it writes, so the crossing lengthens
            // through the call.
            clock: Resampler::new(FS, FS * (1.0 + PPM / 1e6)),
            read: Vec::new(),
            wire: VecDeque::from(vec![0.0; CROSSING - 1 + RESAMPLER_LEAD]),
            faults: VecDeque::new(),
            now: 0,
            at_caller: Vec::new(),
            at_host: Vec::new(),
        }
    }

    fn step(&mut self) {
        while let Some(&(at, fault)) = self.faults.front() {
            if at > self.now {
                break;
            }
            self.faults.pop_front();
            match fault {
                Fault::Drop => {
                    self.wire.pop_front();
                }
                Fault::Gap(n) => {
                    for _ in 0..n {
                        self.wire.push_front(0.0);
                    }
                }
            }
        }
        let heard = self.wire.pop_front().unwrap_or(0.0);
        let a = self.caller.step(heard);
        let b = self.host.step(heard);
        self.read.clear();
        self.clock.process((a + b) * HEADROOM, &mut self.read);
        self.wire.extend(self.read.iter().copied());
        self.at_caller.extend(self.caller.take_dte());
        self.at_host.extend(self.host.take_dte());
        self.now += 1;
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            self.step();
        }
    }

    fn up(&self) -> bool {
        self.caller.state() == State::Data && self.host.state() == State::Data
    }

    fn caller_saw(&self) -> String {
        String::from_utf8_lossy(&self.at_caller).into_owned()
    }

    fn host_saw(&self) -> String {
        String::from_utf8_lossy(&self.at_host).into_owned()
    }
}

/// Moments for `count` faults between `from` and `to` seconds after `start`,
/// drawn from a fixed seed so that the call is the same every time.
fn moments(start: usize, from: f64, to: f64, count: usize, mut seed: u64) -> Vec<usize> {
    let mut at: Vec<usize> = (0..count)
        .map(|_| {
            // xorshift64
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let unit = (seed >> 11) as f64 / (1u64 << 53) as f64;
            start + ((from + unit * (to - from)) * FS) as usize
        })
        .collect();
    at.sort_unstable();
    at
}

/// The whole of it: the card's clocks 20 ppm apart, five samples dropped and
/// one 160-sample underrun, all while the call is carrying data. Today the
/// frozen echo canceller cannot follow the drift at all (contract.md §7 F:
/// 20 ppm leaves the call not connected at the end), and a single dropped
/// sample is a retrain and a lower rate.
#[test]
fn a_v32bis_call_rides_out_the_faults_of_a_real_sound_card() {
    let mut cable = Cable::new();
    for _ in 0..(30.0 * FS) as usize {
        if cable.up() {
            break;
        }
        cable.step();
    }
    assert!(
        cable.up(),
        "never connected: the caller stopped at {} and the host at {}",
        cable.caller.line_phase(),
        cable.host.line_phase()
    );
    assert!(cable.caller_saw().contains("CONNECT"), "the terminal was never told: {:?}", cable.caller_saw());
    assert_eq!(cable.caller.rate(), Some(14_400), "came up at the wrong rate");

    // Two seconds in, and over the next twelve: five drops and one underrun.
    let start = cable.now;
    let mut faults: Vec<(usize, Fault)> = moments(start, 2.0, 14.0, 5, 0x5EED_CAB1E)
        .into_iter()
        .map(|at| (at, Fault::Drop))
        .collect();
    faults.extend(moments(start, 2.0, 14.0, 1, 0x0DD_6A9).into_iter().map(|at| (at, Fault::Gap(160))));
    faults.sort_by_key(|&(at, _)| at);
    cable.faults = faults.into();
    cable.run(16.0);

    let greeting = "Welcome to phl6-dial1.popsite.net\r\nlogin:";
    for b in greeting.bytes() {
        cable.host.feed_dte(b);
    }
    cable.run(14.0);

    for (name, saw) in [("caller", cable.caller_saw()), ("host", cable.host_saw())] {
        assert!(!saw.contains("NO CARRIER"), "the {name}'s terminal was told NO CARRIER: {saw:?}");
    }
    assert!(
        cable.up(),
        "the call did not last: the caller is at {} and the host at {}",
        cable.caller.line_phase(),
        cable.host.line_phase()
    );
    assert!(
        cable.caller_saw().contains(greeting),
        "connected, but the greeting did not come through: {:?}",
        cable.caller_saw()
    );
    assert_eq!(cable.caller.rate(), Some(14_400), "not at 14 400 any more");
    assert_eq!(
        (cable.caller.retrains(), cable.host.retrains()),
        (0, 0),
        "the faults cost a retrain"
    );
}
