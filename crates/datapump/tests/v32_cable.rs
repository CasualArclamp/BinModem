//! Two whole V.32 modems on a sound card's cable whose two clocks disagree.
//!
//! `modem-loop` puts both modems on one cable: their outputs are summed and
//! played, and what the card records is heard by both, so each hears the
//! other and itself at the same level. The card plays on one clock and
//! records on another, a few parts per million to a hundred apart, and that
//! is what this adds to the cable `v32_call` and `soundcard_loop` already
//! model: everything recorded is the played signal taken from the one clock
//! to the other.
//!
//! The far end drifting is the receiver's business, and it follows 200 ppm
//! on a direct line. The echo drifting was the canceller's, and it could
//! not: trained and then frozen, it was cancelling where the echo used to
//! be. On this cable at 5 ppm the call came up at 14 400 and retrained six
//! times at each end inside the minute; at 20 and 100 ppm it was not up
//! after 30 s.
//!
//! Slips are the receiver's to survive, and are not asserted here, and nor
//! are the canceller's jumps. A clean cable can still show some at the
//! answering end: it is silent for 2 s after R1, before it has had time to
//! learn the drift, and at 50 ppm and over the echo moves whole samples in
//! that time, which the search puts right when it speaks again.

use datapump::v32::startup::{Modem, Rates, Role, Status, rate_signal};
use dsp::Resampler;
use std::collections::VecDeque;

const FS: f64 = 16_000.0;

/// Headroom for the sum of two modems on one pair, as `modem-loop` applies it.
const HEADROOM: f64 = 0.45;

/// One crossing of the cable, in samples, as measured on the rig.
const CROSSING: usize = 700;

/// How long the rate must hold once both ends are up.
const HELD_FOR: f64 = 60.0;

/// Longest the start-up may take before the call counts as not connecting.
const CONNECT_WITHIN: f64 = 30.0;

/// The cable: what both modems play, summed, taken from the output's clock to
/// the input's, and heard by both a crossing later.
struct Cable {
    clock: Resampler,
    wire: VecDeque<f64>,
    out: Vec<f64>,
}

impl Cable {
    fn new(ppm: f64) -> Self {
        Self {
            clock: Resampler::new(FS, FS * (1.0 + ppm * 1.0e-6)),
            wire: VecDeque::from(vec![0.0; CROSSING]),
            out: Vec::new(),
        }
    }

    fn heard(&mut self) -> f64 {
        self.wire.pop_front().unwrap_or(0.0)
    }

    fn play(&mut self, a: f64, b: f64) {
        self.out.clear();
        self.clock.process((a + b) * HEADROOM, &mut self.out);
        self.wire.extend(self.out.iter());
    }
}

/// What a call on the cable did.
#[derive(Debug)]
struct Call {
    /// When both ends were first connected, in seconds.
    connected_at: f64,
    /// The rate both ends were connected at then.
    rate: u32,
    /// Seconds of the held stretch that both ends were still connected at
    /// that rate for, counted until the first that either was not.
    held: f64,
    retrains: (u32, u32),
    /// Reception (residual over spacing) at each end, median over the held
    /// stretch's seconds, and the worst second.
    reception: [(f64, f64); 2],
    drift: (f64, f64),
    jumps: (u32, u32),
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
}

/// Reception each second: the residual error against the point spacing,
/// the figure the retrain rule reads at a quarter.
fn reception(modem: &Modem) -> f64 {
    modem.residual_error() / modem.point_spacing()
}

/// Place a call offering every rate from 4800 to 14 400 at both ends on a
/// cable at `ppm`, and hold it for a minute once it is up.
fn call(ppm: f64) -> Call {
    let offer = rate_signal(Rates::between(4800, 14_400));
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut cable = Cable::new(ppm);
    let second = FS as usize;

    let mut connected_at = None;
    let mut rate = 0;
    let mut held = 0.0;
    let mut broken = false;
    let mut seconds: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut n = 0usize;
    loop {
        let heard = cable.heard();
        let a = calling.step(heard);
        let b = answering.step(heard);
        cable.play(a, b);
        n += 1;
        let t = n as f64 / FS;

        match connected_at {
            None => {
                if let (Status::Connected(x), Status::Connected(y)) =
                    (calling.status(), answering.status())
                    && x == y
                {
                    connected_at = Some(t);
                    rate = x;
                }
                if t > CONNECT_WITHIN {
                    break;
                }
            }
            Some(at) => {
                let up = calling.status() == Status::Connected(rate)
                    && answering.status() == Status::Connected(rate);
                if !up {
                    broken = true;
                }
                if !broken {
                    held = t - at;
                }
                if n.is_multiple_of(second) {
                    seconds[0].push(reception(&calling));
                    seconds[1].push(reception(&answering));
                }
                if t - at >= HELD_FOR {
                    break;
                }
            }
        }
    }

    let reception = [0, 1].map(|end| {
        (
            median(seconds[end].clone()),
            seconds[end].iter().copied().fold(f64::NAN, f64::max),
        )
    });
    Call {
        connected_at: connected_at.unwrap_or(f64::NAN),
        rate,
        held,
        retrains: (calling.retrains(), answering.retrains()),
        reception,
        drift: (calling.echo_drift_ppm(), answering.echo_drift_ppm()),
        jumps: (calling.echo_jumps(), answering.echo_jumps()),
    }
}

/// 14 400 held for a minute with no retrain at either end.
fn holds(ppm: f64) {
    let call = call(ppm);
    println!(
        "{ppm} ppm: up at {:.2} s at {}, held {:.1} s, retrains {:?}; reception \
         median/worst {:.3}/{:.3} calling, {:.3}/{:.3} answering; echo drift read \
         {:.1} and {:.1} ppm, jumps {:?}",
        call.connected_at,
        call.rate,
        call.held,
        call.retrains,
        call.reception[0].0,
        call.reception[0].1,
        call.reception[1].0,
        call.reception[1].1,
        call.drift.0,
        call.drift.1,
        call.jumps,
    );
    assert!(
        call.connected_at.is_finite(),
        "at {ppm} ppm the call was not up within {CONNECT_WITHIN} s"
    );
    assert_eq!(call.rate, 14_400, "at {ppm} ppm the call came up at {}", call.rate);
    assert_eq!(
        call.retrains,
        (0, 0),
        "at {ppm} ppm the call retrained after {:.1} s",
        call.held
    );
    assert!(
        call.held >= HELD_FOR - 0.01,
        "at {ppm} ppm 14 400 held for only {:.1} s",
        call.held
    );
}

#[test]
fn five_parts_per_million_hold_fourteen_four() {
    holds(5.0);
}

#[test]
fn twenty_parts_per_million_hold_fourteen_four() {
    holds(20.0);
}

#[test]
fn a_hundred_parts_per_million_hold_fourteen_four() {
    holds(100.0);
}
