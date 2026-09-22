//! Tracking: each symbol equalised, decided, and its error fed back into the
//! equaliser, the carrier's phase and the symbol timing.
//!
//! Copied from `v34/receiver.rs:909-1013`, split in two so that the driver can
//! make the decision the loops track: [`Core::next`] equalises the symbol
//! and [`Core::settle`] does everything that follows from the decision.
//!
//! The equaliser follows by normalised least mean squares in its own frame,
//! the error turned back by the carrier's phase first, so that the taps model
//! a static line and do not chase the carrier. The phase follows by a
//! second-order loop, critically damped with a double root at 0.98: 50
//! symbols, 30.5 Hz at 2400 baud. The timing follows by a data-aided detector
//! -- the error's share along the output's rate of change -- in a loop
//! critically damped with a double root at 0.995: 200 symbols. The equaliser
//! takes about 870 (core.md 3). Each loop is about four times faster than the
//! equaliser it shares a degree of freedom with, so it takes that freedom
//! first.
//!
//! The gain control added here shares its degree of freedom with the
//! equaliser too, and takes it first as well. It is bounded by an average of
//! every symbol's power over 32, 128 or 256 symbols with no decision in it,
//! so no run of wrong decisions can take the gain anywhere, and trimmed
//! within the bound from the decisions over 64 ([`Agc`]). While the signal
//! is lost it is frozen, because it would otherwise wind itself up on
//! silence.

use super::{Core, EARLIER_EVERY, EARLIER_KEPT, Mode, Point, REACH, RESYNC_EVERY, Symbol, apply};
use crate::Complex;

/// Normalised least-mean-squares step (`v34/receiver.rs:127`).
const STEP: f64 = 0.02;

/// Carrier loop gains, a symbol at a time (`v34/receiver.rs:130-131`).
const PHASE_GAIN: f64 = 0.04;
const FREQUENCY_GAIN: f64 = 4e-4;

/// Timing loop gains (`v34/receiver.rs:138-139`): how much of each symbol's
/// timing error is taken out of the sampling at once, and how much goes into
/// its rate.
const TIMING_GAIN: f64 = 0.01;
const DRIFT_GAIN: f64 = 1.25e-5;

/// The relative gate: an error passes if under this many times what the
/// errors settle to, which a Gaussian error does with probability 1 - e^-9;
/// but never for less than the least distance squared over this, so that a
/// 50 dB line does not refuse symbols over nothing (design.md 2.3).
const RELATIVE: f64 = 9.0;
const GATE_FLOOR: f64 = 400.0;

/// How far above what the errors had settled to a while ago the recent ones
/// may climb before the signal is taken to be lost: 6 dB.
const SLID: f64 = 4.0;

/// How far either way the gain control may take the gain from what training
/// set: 20 dB.
const GAIN_RANGE: f64 = 10.0;

/// The share of each accepted symbol's gain error the trim takes: 64
/// symbols, thirteen times faster than the equaliser, which shares the
/// degree of freedom.
const TRIM: f64 = 1.0 / 64.0;

/// Symbols between looks at the taps' centre of weight, and the most of the
/// taps' energy the tap a move drops may carry: 40 dB down.
const ANCHOR_EVERY: u64 = 1024;
const ANCHOR_SPARE: f64 = 1e-4;

/// The gain control: a gain, bounded without decisions and trimmed with
/// them.
///
/// The bound is the gain control design.md 2.2 asks for. `power` is the
/// output's mean power before the gain, against the constellation's, a
/// one-pole average of every symbol with no decision in it, and the gain is
/// never let stray further from the one that makes that power right than
/// the average itself wanders on its own ([`super::Slicer::agc_zone`]). So
/// no run of wrong decisions can take the gain anywhere, and anything that
/// moves the level beyond the bound moves the gain with it.
///
/// Within the bound the gain is trimmed from the decisions the gate lets
/// through, a little faster than the carrier loop. The average alone cannot
/// be both quick and steady: every point's power goes into it, and a
/// constellation's own spread of power is noise to it -- on sixteen points
/// at 128 symbols, 1.8% of gain rms, which held a clean line to 36.6 dB. And
/// left to the equaliser, a gain the bound had stopped at its edge took the
/// equaliser 2.3 s to finish, measured after a -6 dB ramp. The trim has no
/// such noise, and the bound keeps it honest: within it on sixteen points the
/// gain is out by at most 0.45 dB, where the decisions are still right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Agc {
    gain: f64,
    power: f64,
    /// How far the gain may be from the power's, as a natural log.
    zone: f64,
    trained: f64,
}

impl Agc {
    pub(super) fn unity() -> Self {
        Self::at(1.0)
    }

    /// Starting at gain `g`, and kept within 20 dB of it.
    pub(super) fn at(g: f64) -> Self {
        Self { gain: g, power: 1.0 / (g * g), zone: 0.0, trained: g }
    }

    pub(super) fn value(&self) -> f64 {
        self.gain.clamp(self.trained / GAIN_RANGE, self.trained * GAIN_RANGE)
    }

    /// A gain found outright, by a resync's fit: the power estimate starts
    /// again from it.
    pub(super) fn refit(&mut self, g: f64) {
        self.gain = g;
        self.power = 1.0 / (g * g);
    }

    /// One symbol's output before the gain, `unscaled` its power, against a
    /// constellation of power `mean`: the average taking `rate` of it, and the
    /// gain held to within `zone` (a share of the power) of what it says.
    fn follow(&mut self, unscaled: f64, mean: f64, rate: f64, zone: f64) {
        self.power += rate * (unscaled / mean - self.power);
        self.zone = 0.5 * (1.0 + zone).ln();
        let says = 1.0 / self.power.max(1e-30).sqrt();
        self.gain = self.gain.clamp(says * (-self.zone).exp(), says * self.zone.exp());
    }

    /// One accepted symbol's decision: `e` the error of `z` from it, against
    /// a constellation of power `mean`. Least squares over the symbols, so an
    /// outer point says more than an inner one, as it should.
    fn trim(&mut self, e: Complex, z: Complex, mean: f64) {
        self.gain *= 1.0 - TRIM * (e * z.conj()).re / mean;
    }
}

/// The loops at one symbol, for putting back (`v34/receiver.rs:441-451`).
#[derive(Debug, Clone)]
pub(super) struct Loops {
    symbol: u64,
    taps: Vec<Complex>,
    rotation: f64,
    turn: f64,
    drift: f64,
    slope: f64,
    timed: f64,
    settled: f64,
    error: f64,
    gain: Agc,
    anchored: i64,
}

/// A symbol equalised and not yet settled.
#[derive(Debug, Clone)]
pub(super) struct Pending {
    /// The row of half-symbol samples and one more either side.
    wide: Vec<Complex>,
    /// The output's rate of change, before the carrier and the gain.
    rate: Complex,
    spin: Complex,
    /// The output's power before the gain.
    unscaled: f64,
    gain: f64,
    point: Point,
}

impl Core {
    /// The next symbol, equalised, once its samples are in: the same one
    /// again until it is settled. None unless tracking.
    ///
    /// Not an iterator's `next`: nothing moves on until [`Core::settle`].
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Point> {
        if let Some(pending) = &self.pending {
            return Some(pending.point);
        }
        if !matches!(self.mode, Mode::Tracking) {
            return None;
        }
        loop {
            // As `v34/receiver.rs:711-716`.
            while self.next_symbol + REACH as u64 + 2 <= self.front.made {
                if self.next_symbol < self.front.first + REACH as u64 + 1 {
                    self.next_symbol += 2;
                    continue;
                }
                return Some(self.equalise());
            }
            // The samples may be in already: after a resync has read the
            // newest halves again on a moved grid, they are made afresh.
            let (half, index) = self.front.make_half()?;
            self.on_half(half, index);
            if !matches!(self.mode, Mode::Tracking) {
                return None;
            }
        }
    }

    /// Equalise the symbol at `next_symbol` (`v34/receiver.rs:910-923`).
    fn equalise(&mut self) -> Point {
        let wide = self.front.samples(self.next_symbol, REACH + 1).expect("the caller checked the samples are here");
        self.next_symbol += 2;
        let row = &wide[1..wide.len() - 1];
        let y = apply(&self.taps, row);
        // How fast the output is changing: the same filter over the samples'
        // central differences.
        let rate = self
            .taps
            .iter()
            .enumerate()
            .fold(Complex::ZERO, |sum, (i, w)| sum + *w * (wide[i + 2] - wide[i]).scale(0.5));
        let spin = Complex::from_polar(1.0, -self.rotation);
        let gain = self.gain.value();
        let z = (y * spin).scale(gain);
        let (label, nearest) = self.slicer.decide(z);
        let point = Point { z, nearest, label, index: self.produced };
        self.pending = Some(Pending { wide, rate, spin, unscaled: y.norm_sqr(), gain, point });
        point
    }

    /// Settle the symbol [`Core::next`] gave, the loops tracking `target`:
    /// the nearest point for an uncoded constellation, or a trellis
    /// decoder's best guess. Its error against the nearest point is what
    /// loss, resync and the signal-to-noise figures are judged by, so that
    /// their thresholds keep their meaning whatever the decision
    /// (design.md 2.6). None if there was no symbol to settle.
    pub fn settle(&mut self, target: Complex) -> Option<Symbol> {
        let Pending { wide, rate, spin, unscaled, gain, point } = self.pending.take()?;
        let row = &wide[1..wide.len() - 1];
        let z = point.z;
        let e = z - target;
        let squared = e.norm_sqr();
        let near = (z - point.nearest).norm_sqr();
        // Half the way to the nearest other point, squared: an error past it
        // is more likely a wrong decision than a right one.
        let d2 = self.slicer.min_distance_squared();
        let doubtful = 0.25 * d2;
        let gate = if self.options.relative_gate {
            doubtful.min((RELATIVE * self.settled).max(d2 / GATE_FLOOR))
        } else {
            doubtful
        };
        let (_, judged) = self.slicer.window();
        self.recent.push_back(near);
        while self.recent.len() > judged {
            self.recent.pop_front();
        }
        if self.options.relative_gate {
            self.refused.push_back(squared >= gate);
            while self.refused.len() > judged {
                self.refused.pop_front();
            }
        }
        let recent = self.recent.iter().sum::<f64>() / self.recent.len() as f64;
        // The relative gate needs this second way to be lost. A one-sample
        // slip at four points turns the constellation 40.5 degrees: an error
        // under the loss threshold, and every symbol refused by a gate nine
        // times the settled error, so nothing would ever learn or resync.
        let refusing = self.refused.len() == judged && self.refused.iter().filter(|r| **r).count() * 4 >= judged * 3;
        // And a third. Errors that grow gradually are let through a gate
        // relative to what the errors settle to, and the settled error rises
        // to meet them: a -6 dB ramp over half a second on the 128-point
        // cross slid from 34 dB to a false lock at 21 dB with the gate
        // refusing almost nothing. So the recent errors are also set against
        // what the errors had settled to up to 384 symbols before, in the
        // oldest copy of the loops kept.
        let slid = self.options.relative_gate
            && self.earlier.front().is_some_and(|l| recent > SLID * l.settled.max(self.slicer.min_distance_squared() / GATE_FLOOR));
        if !self.held {
            match self.lost {
                None if self.recent.len() == judged
                    && (recent > self.slicer.lost_threshold(self.settled) || refusing || slid) =>
                {
                    // The signal has jumped, or gone. Hold everything -- as it
                    // was before the symbols that showed it, which every loop
                    // has been learning from as though they were right.
                    self.lost = Some(0);
                    self.power_before = self.front.power;
                    self.lost_from = self.next_symbol.saturating_sub(2 * judged as u64);
                    self.rewind(judged as u64 + EARLIER_EVERY);
                }
                Some(n) => self.lost = Some(n + 1),
                None => {}
            }
        }
        self.rotation += self.turn;
        let accepted = self.lost.is_none() && !self.held && squared < gate;
        if accepted {
            // The equaliser learns in its own frame, before the carrier is
            // taken out, and the gain in front of it is taken back out of
            // its step so that its speed does not depend on it.
            let energy: f64 = row.iter().map(|x| x.norm_sqr()).sum::<f64>() + 1e-9;
            let back = e * spin.conj() * (STEP / (energy * gain));
            for (tap, x) in self.taps.iter_mut().zip(row) {
                *tap -= back * x.conj();
            }
            let power = self.slicer.phase_power(target, self.options.unweighted_phase);
            let wrong = (z * target.conj()).im / power;
            self.turn += FREQUENCY_GAIN * wrong;
            if self.options.turn_limit_hz.is_finite() {
                let limit = std::f64::consts::TAU * self.options.turn_limit_hz / self.band.baud;
                self.turn = self.turn.clamp(-limit, limit);
            }
            self.rotation += PHASE_GAIN * wrong;
            // Timing. An output sampled late by a fraction of a half symbol is
            // out by that fraction of its rate of change, so the error's share
            // along the rate of change is how late.
            let rate = (rate * spin).scale(gain);
            self.slope += 0.01 * (rate.norm_sqr() - self.slope);
            let late = ((e * rate.conj()).re / self.slope.max(1e-9)).clamp(-0.5, 0.5);
            self.front.due -= TIMING_GAIN * late * self.front.half;
            self.timed -= TIMING_GAIN * late * self.front.half;
            self.front.drift = (self.front.drift - DRIFT_GAIN * late).clamp(-0.001, 0.001);
            self.settled += 0.01 * (squared - self.settled);
        }
        if self.options.agc && self.lost.is_none() && !self.held {
            let mean = self.slicer.mean_power();
            if accepted {
                self.gain.trim(e, z, mean);
            }
            let zone = self.slicer.agc_zone(self.settled);
            self.gain.follow(unscaled, mean, self.slicer.agc_rate(), zone);
        }
        let symbol = self.next_symbol / 2;
        if self.lost.is_none() && symbol.is_multiple_of(EARLIER_EVERY) {
            self.snapshot();
        }
        self.rotation = self.rotation.rem_euclid(std::f64::consts::TAU);
        self.error += 0.01 * (near - self.error);
        self.residual += 0.01 * (near.sqrt() - self.residual);
        self.last = z;
        self.produced += 1;
        if self.options.anchor && self.lost.is_none() && !self.held && self.produced.is_multiple_of(ANCHOR_EVERY) {
            self.anchor();
        }
        if !self.held
            && let Some(lost) = self.lost
            && lost >= self.slicer.window().0 / 2
            && lost % RESYNC_EVERY == 0
        {
            self.resync();
        }
        Some(Symbol { point: z, target, nearest: point.nearest, label: point.label, error: near, accepted })
    }

    /// Keep a copy of the loops as they are (`v34/receiver.rs:971-986`).
    pub(super) fn snapshot(&mut self) {
        if self.earlier.len() == EARLIER_KEPT {
            self.earlier.pop_front();
        }
        self.earlier.push_back(Loops {
            symbol: self.next_symbol / 2,
            taps: self.taps.clone(),
            rotation: self.rotation,
            turn: self.turn,
            drift: self.front.drift,
            slope: self.slope,
            timed: self.timed,
            settled: self.settled,
            error: self.error,
            gain: self.gain,
            anchored: self.anchored,
        });
    }

    /// Put the loops back as they were at least `back` symbols ago, carrying
    /// the carrier's phase on by the turn as it was then: for when the far
    /// end's symbols have been decided against the wrong constellation, and
    /// every loop has learnt from the decisions (`v34/receiver.rs:997-1013`).
    ///
    /// False if no copy was old enough. V.34's did nothing then, and so does
    /// this unless [`super::Options::rewind_safely`], when the oldest copy
    /// there is goes back instead.
    pub fn rewind(&mut self, back: u64) -> bool {
        let now = self.next_symbol / 2;
        let (kept, found) = match self.earlier.iter().rposition(|l| now.saturating_sub(l.symbol) >= back) {
            Some(kept) => (kept, true),
            None if self.options.rewind_safely && !self.earlier.is_empty() => (0, false),
            None => {
                self.short_rewinds += 1;
                return false;
            }
        };
        if !found {
            self.short_rewinds += 1;
        }
        // The copies after it were made from the loops being put right.
        self.earlier.truncate(kept + 1);
        let loops = self.earlier[kept].clone();
        let elapsed = now.saturating_sub(loops.symbol) as f64;
        self.taps.clone_from(&loops.taps);
        self.turn = loops.turn;
        self.front.drift = loops.drift;
        self.slope = loops.slope;
        self.rotation = (loops.rotation + loops.turn * elapsed).rem_euclid(std::f64::consts::TAU);
        self.front.due += loops.timed - self.timed;
        self.timed = loops.timed;
        self.settled = loops.settled;
        self.error = loops.error;
        self.gain = loops.gain;
        // Taps from before the anchor last moved them belong to symbols
        // centred where they were then.
        self.next_symbol = self.next_symbol.saturating_add_signed(loops.anchored - self.anchored);
        self.anchored = loops.anchored;
        found
    }

    /// Keep the taps' weight near their middle.
    ///
    /// The timing loop and the equaliser share the delay between them, and
    /// nothing ties down how they split it: noise could walk the taps' weight
    /// off towards one end over an hour-long call while the timing walked the
    /// other way (core.md N6). If the weight has moved more than a half
    /// symbol, the taps move back a half symbol and the symbols are centred a
    /// half symbol the other way, which leaves every output as it was.
    fn anchor(&mut self) {
        let centroid = self.tap_centroid();
        // Moving the taps drops the one at the end they move away from, and
        // leaves the output as it was only if that one is next to nothing.
        // Taps trained on a drifting line carry weight all the way out, in
        // combinations that cancel on the signal and do not once one of them
        // is cut off: at 200 ppm, the second tap from the end cut off cost
        // 2.5 dB at once. Such a tap waits for the next look.
        let energy: f64 = self.taps.iter().map(|w| w.norm_sqr()).sum();
        let spare = |w: &Complex| w.norm_sqr() <= ANCHOR_SPARE * energy;
        if centroid > 1.0 && spare(&self.taps[0]) {
            // Every tap one earlier, and the symbols one half later: the
            // output at half n + 1 with the moved taps is the output at n
            // with the old.
            self.taps.rotate_left(1);
            if let Some(last) = self.taps.last_mut() {
                *last = Complex::ZERO;
            }
            self.next_symbol += 1;
            self.anchored += 1;
        } else if centroid < -1.0 && spare(&self.taps[2 * REACH]) {
            self.taps.rotate_right(1);
            self.taps[0] = Complex::ZERO;
            self.next_symbol -= 1;
            self.anchored -= 1;
        }
    }
}
