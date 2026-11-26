//! V.34 between two of these modems on one pair of wires.
//!
//! Both directions of V.34 are in the same band. On a line that brings this
//! end's own signal back, the only thing that separates the far end from the
//! echo is an echo canceller. Before V.34 had one, an echo 20 dB below the
//! far end took a call from 33 600 bit/s to 14 400, and an echo 10 dB below
//! it ended the call in NO CARRIER.

use modem::{Modem, State};
use std::collections::VecDeque;

const FS: f64 = 16_000.0;

/// Headroom for the sum of two modems on one pair, as `modem-loop` applies it.
const HEADROOM: f64 = 0.45;

/// One crossing of a sound card's cable, in samples: 44 ms, as measured.
const CROSSING: usize = 700;

/// What an end's own signal comes back as.
#[derive(Clone)]
struct Echo {
    /// Samples before it comes back.
    delay: usize,
    /// What it comes back through.
    shape: Vec<f64>,
    /// How fast it moves, in parts per million: a sound card with two clocks.
    drift_ppm: f64,
}

impl Echo {
    /// A copy at `level`, `delay` samples later.
    fn copy(delay: usize, level: f64) -> Self {
        Self { delay, shape: vec![level], drift_ppm: 0.0 }
    }

    /// A hybrid: it reflects at once, through a filter of a millisecond.
    fn hybrid(level: f64) -> Self {
        let shape = (0..16).map(|i| level * (-(i as f64) / 4.0).exp() * (0.8 * i as f64).cos()).collect();
        Self { delay: 2, shape, drift_ppm: 0.0 }
    }
}

/// One end's echo path.
struct Path {
    echo: Echo,
    sent: VecDeque<f64>,
    moved: f64,
}

impl Path {
    fn new(echo: Echo) -> Self {
        let reach = echo.delay + echo.shape.len() + 64;
        Self { echo, sent: VecDeque::from(vec![0.0; reach]), moved: 0.0 }
    }

    /// What comes back now, and then the sample that was just sent goes in.
    fn back(&mut self, sent: f64) -> f64 {
        self.moved += self.echo.drift_ppm * 1e-6;
        let (whole, part) = (self.moved.floor() as usize, self.moved.fract());
        let at = |lag: usize| -> f64 { self.echo.shape.iter().enumerate().map(|(i, s)| s * self.sent[self.echo.delay + lag + i]).sum() };
        let heard = (1.0 - part) * at(whole) + part * at(whole + 1);
        self.sent.pop_back();
        self.sent.push_front(sent);
        heard
    }
}

/// Two modems on one line. Each hears the far end after a crossing, and its
/// own signal through its echo path.
struct Pair {
    caller: Modem,
    host: Modem,
    to_host: VecDeque<f64>,
    to_caller: VecDeque<f64>,
    own_caller: Path,
    own_host: Path,
    last: (f64, f64),
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
}

impl Pair {
    fn new(echo: Echo) -> Self {
        let mut caller = Modem::new(FS);
        let mut host = Modem::new(FS);
        for m in [&mut caller, &mut host] {
            for b in b"AT+MS=V34\r" {
                m.feed_dte(*b);
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
            to_host: VecDeque::from(vec![0.0; CROSSING]),
            to_caller: VecDeque::from(vec![0.0; CROSSING]),
            own_caller: Path::new(echo.clone()),
            own_host: Path::new(echo),
            last: (0.0, 0.0),
            at_caller: Vec::new(),
            at_host: Vec::new(),
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let at_caller = self.to_caller.pop_front().unwrap_or(0.0) + self.own_caller.back(self.last.0);
            let at_host = self.to_host.pop_front().unwrap_or(0.0) + self.own_host.back(self.last.1);
            let a = self.caller.step(at_caller) * HEADROOM;
            let b = self.host.step(at_host) * HEADROOM;
            self.to_host.push_back(a);
            self.to_caller.push_back(b);
            self.last = (a, b);
            self.at_caller.extend(self.caller.take_dte());
            self.at_host.extend(self.host.take_dte());
        }
    }

    fn up(&self) -> bool {
        self.caller.state() == State::Data && self.host.state() == State::Data
    }

    /// Run until both ends are in data mode, then send a line each way.
    fn connect_and_talk(&mut self, what: &str) {
        for _ in 0..40 {
            if self.up() {
                break;
            }
            self.run(1.0);
        }
        assert!(
            self.up(),
            "{what}: the caller stopped at {} and the host at {}
{}
--
{}
{}",
            self.caller.line_phase(),
            self.host.line_phase(),
            self.caller.take_line_notes().join("
"),
            self.host.take_line_notes().join("
"),
            String::from_utf8_lossy(&self.at_caller)
        );
        self.run(1.0);
        self.at_caller.clear();
        self.at_host.clear();
        for b in b"login: " {
            self.host.feed_dte(*b);
        }
        for b in b"guest\r" {
            self.caller.feed_dte(*b);
        }
        self.run(4.0);
        assert_eq!(String::from_utf8_lossy(&self.at_caller), "login: ", "{what}");
        assert_eq!(String::from_utf8_lossy(&self.at_host), "guest\r", "{what}");
    }
}

/// The line that a sound card's cable makes: each end's own signal comes
/// back as loud as the far end, a crossing later. Neither rate falls.
#[test]
fn an_echo_as_loud_as_the_far_end_costs_nothing() {
    let mut pair = Pair::new(Echo::copy(CROSSING, 1.0));
    pair.connect_and_talk("a cable");
    assert_eq!((pair.caller.rate(), pair.host.rate()), (Some(33_600), Some(33_600)));
    for (who, modem) in [("caller", &pair.caller), ("host", &pair.host)] {
        // The reflection is found where it is, past the first run of taps.
        let found = modem.reflection().unwrap_or_else(|| panic!("{who}: no reflection found"));
        assert!(found.delay.abs_diff(CROSSING) < 4, "{who}: found at {}", found.delay);
        // The least mean squares rule leaves some 30 dB of the training
        // segment; the fit takes all of it.
        let (learned, fitted) = modem.echo_fit().unwrap_or_else(|| panic!("{who}: no fit"));
        assert!(learned < 45.0 && fitted > 60.0, "{who}: {learned:.1} dB learned, {fitted:.1} dB fitted");
        assert!(!modem.echo_following(), "{who}: the echo does not move");
    }
}

/// A faint echo is found too. It was not, at the call modem: the answer
/// modem's J was still arriving when the search started, and next to a
/// faint echo that was most of the line.
#[test]
fn a_faint_echo_is_found_and_taken_out() {
    let mut pair = Pair::new(Echo::copy(CROSSING, 0.03));
    pair.connect_and_talk("a faint echo");
    assert_eq!((pair.caller.rate(), pair.host.rate()), (Some(33_600), Some(33_600)));
    assert!(pair.caller.reflection().is_some() && pair.host.reflection().is_some());
}

/// A hybrid: the echo comes back at once, through a filter. The first run
/// of taps has it.
#[test]
fn a_hybrid_echo_is_taken_out() {
    let mut pair = Pair::new(Echo::hybrid(0.5));
    pair.connect_and_talk("a hybrid");
    assert_eq!((pair.caller.rate(), pair.host.rate()), (Some(33_600), Some(33_600)));
    for modem in [&pair.caller, &pair.host] {
        assert!(modem.echo_fit().is_some_and(|(_, fitted)| fitted > 60.0), "{:?}", modem.echo_fit());
    }
}

/// A line with no echo has no canceller: one that trained on nothing has
/// learned the line's noise.
#[test]
fn a_line_with_no_echo_keeps_no_canceller() {
    let mut pair = Pair::new(Echo::copy(CROSSING, 0.0));
    pair.connect_and_talk("no echo");
    assert_eq!((pair.caller.rate(), pair.host.rate()), (Some(33_600), Some(33_600)));
    assert_eq!((pair.caller.echo_fit(), pair.host.echo_fit()), (None, None));
    assert_eq!(pair.caller.reflection(), None);
}

/// A sound card with two clocks: the echo comes back later and later. Taps
/// that stay where training put them are useless in seconds. The canceller
/// finds that out and changes to taps that follow the echo, and the call
/// stays up.
#[test]
fn an_echo_that_moves_is_followed() {
    let mut pair = Pair::new(Echo { drift_ppm: 20.0, ..Echo::copy(CROSSING, 1.0) });
    pair.connect_and_talk("a moving echo");
    pair.run(20.0);
    assert!(pair.up(), "the caller is in {} and the host in {}", pair.caller.line_phase(), pair.host.line_phase());
    assert!(pair.caller.echo_following() && pair.host.echo_following());
    pair.at_caller.clear();
    for b in b"still here" {
        pair.host.feed_dte(*b);
    }
    pair.run(4.0);
    assert_eq!(String::from_utf8_lossy(&pair.at_caller), "still here");
    eprintln!("moving echo: {:?} and {:?} bit/s", pair.caller.rate(), pair.host.rate());
}
