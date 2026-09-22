//! Finding the signal again after a jump.
//!
//! A VoIP call's jitter buffer now and then plays made-up audio, or drops
//! some, and a sound card now and then drops or repeats a sample; everything
//! after arrives that much later or earlier, and turned by whatever that
//! stretch of carrier comes to. No loop follows a jump like that, and a loop
//! that tries learns garbage. So once the loss has held every loop, the
//! newest few dozen symbols are read afresh from the raw samples at each
//! fraction of a symbol either side, and the receiver takes up again at
//! whichever reads back as the constellation (`v34/receiver.rs:35-44`).
//!
//! Only the fraction has to be searched. Where the symbols fall in whole
//! half symbols does not matter to anything reading them, and the carrier's
//! phase need only be found to within a quarter turn: data that is coded
//! differentially, as V.32's Table 1 and its trellis code are, does not mind
//! either ambiguity (`v34/receiver.rs:1142-1147`; spec.md 2.4).
//!
//! Copied from `v34/receiver.rs:1015-1211`. The plain search is for four and
//! sixteen points, whose fourth power gives the phase; the dense one is for
//! constellations where that is near nought and a degree or a percent of
//! gain out reads as noise.

use super::{Core, Density, REACH, RESYNC_WINDOW, angle_between, apply};
use crate::Complex;

/// Fractions of a half symbol tried either side, when looking on four or
/// sixteen points (`v34/receiver.rs:106`).
const RESYNC_STEPS: usize = 8;

/// Symbols a resync on a dense constellation is judged over, all of them
/// after the jump that made it necessary (`v34/receiver.rs:117`).
const DENSE_WINDOW: usize = 64;

/// Steps a half symbol is tried in, either side, on a dense constellation;
/// the phase steps, in degrees, it turns through a quarter in; and the finer
/// steps of a half symbol it then tries either side of the best
/// (`v34/receiver.rs:122-124`).
const DENSE_STEPS: usize = 16;
const DENSE_DEGREES: f64 = 1.5;
const DENSE_FINE: usize = 64;

/// Half symbols short of the next symbol that the plain search's window ends
/// at, as V.34 has it and as it is fixed: V.34's last half-symbol sample is
/// the newest made, so a later shift needs samples not taken yet, and a
/// timing jump of +0.25 to +0.35 symbol was never found (core.md 4.3). Four
/// symbols short, as the dense search is.
const SHORT_V34: u64 = 2;
const SHORT: u64 = 10;

/// Of the power the line carried before the signal was lost, the least a
/// window has to carry to be searched. An eighth, 9 dB down: far above a
/// silent line, and far enough below a -6 dB step for that to be found.
const LEVEL_FLOOR: f64 = 0.125;

/// The most a resync may raise the settled error by, as a factor: 6 dB.
const SETTLED_RISE: f64 = 4.0;

/// A reading that fitted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Found {
    /// Its error, and the median of every shift's.
    pub(super) mse: f64,
    pub(super) median: f64,
    /// The first half-symbol sample read again, and how many samples later.
    pub(super) from: u64,
    pub(super) moved: f64,
    /// The carrier's phase at the middle of the window.
    pub(super) turned: f64,
    /// What the gain was found to be out by, if it was fitted.
    pub(super) gain: Option<f64>,
    /// Symbols from the middle of the window to the next symbol, and the
    /// window's length.
    pub(super) distance: f64,
    pub(super) window: usize,
}

/// The half-symbol samples a window reads.
struct Span {
    from: u64,
    last: u64,
    start_time: f64,
    count: usize,
}

impl Core {
    /// A resync, from the loss watch: the search the constellation's density
    /// calls for, and a take-up if it finds the signal clear of the rest.
    pub(super) fn resync(&mut self) {
        let dense = self.slicer.density() == Density::Dense;
        if self.options.resync_after_loss && !self.after_loss(dense) {
            return;
        }
        if self.options.level_gate && !self.loud_enough(dense) {
            return;
        }
        self.resyncs += 1;
        let found = if dense { self.search_dense(self.turn) } else { self.search_sparse(self.turn) };
        let Some(found) = found else { return };
        // The shifts that are wrong read as what the signal reads out of step;
        // a real jump found stands clear of them.
        if found.mse > self.slicer.found_level(self.settled) || found.mse > 0.6 * found.median {
            return;
        }
        self.take_up(&found, true);
    }

    /// Whether the window a resync would read begins no earlier than the
    /// first symbol that showed the loss: symbols from before the jump fit
    /// nothing after it, and the next look will be clear of them.
    fn after_loss(&self, dense: bool) -> bool {
        let (window, short) = if dense { (DENSE_WINDOW, SHORT) } else { (RESYNC_WINDOW, self.short()) };
        self.span(window, short).is_none_or(|span| span.from + REACH as u64 + 1 >= self.lost_from)
    }

    /// Whether the window a resync would read carries enough of the power the
    /// line carried before the signal was lost to be worth reading.
    fn loud_enough(&self, dense: bool) -> bool {
        let (window, short) = if dense { (DENSE_WINDOW, SHORT) } else { (RESYNC_WINDOW, self.short()) };
        let Some(span) = self.span(window, short) else { return true };
        let from = (span.from - self.front.first) as usize;
        let power = self.front.halves.range(from..from + span.count).map(|h| h.norm_sqr()).sum::<f64>() / span.count as f64;
        power >= LEVEL_FLOOR * self.power_before
    }

    fn short(&self) -> u64 {
        if self.options.resync_margin { SHORT } else { SHORT_V34 }
    }

    /// The newest window of `window` symbols that every shift can be read
    /// for, its last centred `short` halves before the next symbol.
    fn span(&self, window: usize, short: u64) -> Option<Span> {
        let reach = REACH as u64 + 1;
        let last = self.next_symbol.checked_sub(short)?;
        let first_centre = last.checked_sub(2 * (window as u64 - 1))?;
        let from = first_centre.checked_sub(reach)?;
        if from < self.front.first || last + reach >= self.front.made {
            return None;
        }
        let start_time = self.front.times[(from - self.front.first) as usize];
        let count = (last + reach - from + 1) as usize;
        Some(Span { from, last, start_time, count })
    }

    /// Symbols from the middle of a window to the next symbol.
    fn distance(&self, span: &Span, window: usize) -> f64 {
        (window as f64 - 1.0) / 2.0 + (self.next_symbol - span.last) as f64 / 2.0
    }

    /// The equaliser's outputs over a window read from `read`, the gain in
    /// front of them, and the carrier's `turn` taken out across it about its
    /// middle if the options say so; and what they were scaled by besides.
    ///
    /// When the gain is to be fitted, the outputs are first brought to the
    /// constellation's mean power, from their own power and no decisions. A
    /// fit against decisions only finds a gain the decisions are nearly right
    /// for: at -6 dB every sixteen-point symbol decides as the point nearest
    /// the centre in its quadrant, and the fit settles on that. Over 48 or 64
    /// symbols the power alone is within about 0.3 dB.
    fn outputs(&self, read: &[Complex], window: usize, turn: f64) -> (Vec<Complex>, f64) {
        let gain = self.gain.value();
        let middle = (window as f64 - 1.0) / 2.0;
        let mut outputs: Vec<Complex> = (0..window)
            .map(|j| {
                let y = apply(&self.taps, &read[2 * j + 1..2 * j + 2 + 2 * REACH]).scale(gain);
                if self.options.resync_derotate { y * Complex::from_polar(1.0, -turn * (j as f64 - middle)) } else { y }
            })
            .collect();
        let mut scale = 1.0;
        if self.options.resync_gain {
            let power = outputs.iter().map(|y| y.norm_sqr()).sum::<f64>() / window as f64;
            if power > 0.0 {
                scale = (self.slicer.mean_power() / power).sqrt();
                for y in &mut outputs {
                    *y = y.scale(scale);
                }
            }
        }
        (outputs, scale)
    }

    /// Mean squared error of `outputs` turned and scaled by `c`.
    fn mse(&self, outputs: &[Complex], c: Complex) -> f64 {
        outputs.iter().map(|y| (*y * c - self.slicer.decide(*y * c).1).norm_sqr()).sum::<f64>() / outputs.len() as f64
    }

    /// Gain and phase, least squares against the decisions, a few rounds
    /// (`v34/receiver.rs:1049-1059`).
    fn fit(&self, outputs: &[Complex], mut c: Complex) -> (f64, Complex) {
        for _ in 0..4 {
            let (num, den) = outputs.iter().fold((Complex::ZERO, 0.0), |(num, den), y| {
                (num + self.slicer.decide(*y * c).1 * y.conj(), den + y.norm_sqr())
            });
            if den > 0.0 {
                c = num.scale(1.0 / den);
            }
        }
        (self.mse(outputs, c), c)
    }

    /// On four or sixteen points: read the newest symbols again at each
    /// eighth of a half symbol either side, turn each reading by its fourth
    /// power to within a quarter, then by the decisions it allows, and fit
    /// its gain if the options say so (`v34/receiver.rs:1148-1199`).
    pub(super) fn search_sparse(&self, turn: f64) -> Option<Found> {
        let half = self.front.half * (1.0 + self.front.drift);
        let (window, _) = self.slicer.window();
        let span = self.span(window, self.short())?;
        let distance = self.distance(&span, window);
        // The phase the loop would have had at the window's middle, if the
        // turn has been taken out across it; V.34 compares with the newest.
        let reference = if self.options.resync_derotate { self.rotation - turn * distance } else { self.rotation };
        let fourth_reference = self.slicer.fourth();
        let mut readings = Vec::new();
        let mut by_step = [None; 2 * RESYNC_STEPS];
        let mut best: Option<(f64, f64, f64, Option<f64>)> = None;
        for (step, slot) in by_step.iter_mut().enumerate() {
            let shift = (step as f64 - RESYNC_STEPS as f64) / RESYNC_STEPS as f64;
            let read: Option<Vec<Complex>> =
                (0..span.count).map(|m| self.front.interpolate(span.start_time + (m as f64 + shift) * half)).collect();
            let Some(read) = read else { continue };
            let (outputs, scale) = self.outputs(&read, window, turn);
            let fourth = outputs.iter().fold(Complex::ZERO, |sum, y| sum + *y * *y * *y * *y);
            let base = (fourth.arg() - fourth_reference) / 4.0;
            // Of the four turns that fit, the one nearest the turn before.
            let mut turned = (0..4)
                .map(|q| base + std::f64::consts::FRAC_PI_2 * f64::from(q))
                .min_by(|a, b| angle_between(*a, reference).total_cmp(&angle_between(*b, reference)))
                .unwrap_or(base);
            // The fourth power only gets close on a dense constellation; the
            // decisions it then allows take the rest of the way.
            for _ in 0..3 {
                let spin = Complex::from_polar(1.0, -turned);
                let lean = outputs.iter().fold(Complex::ZERO, |sum, y| {
                    let z = *y * spin;
                    sum + z * self.slicer.decide(z).1.conj()
                });
                turned += lean.arg();
            }
            let (mse, gain) = if self.options.resync_gain {
                let (mse, c) = self.fit(&outputs, Complex::from_polar(1.0, -turned));
                turned = -c.arg();
                (mse, Some(scale * c.abs()))
            } else {
                let spin = Complex::from_polar(1.0, -turned);
                let mse = outputs.iter().map(|y| (*y * spin - self.slicer.decide(*y * spin).1).norm_sqr()).sum::<f64>()
                    / window as f64;
                (mse, None)
            };
            readings.push(mse);
            *slot = Some(mse);
            if best.is_none_or(|b| mse < b.0) {
                best = Some((mse, shift, turned, gain));
            }
        }
        let (mse, mut shift, turned, gain) = best?;
        if self.options.resync_fine {
            // An eighth of a half symbol is as near as the steps get, and on
            // sixteen points at 35 dB a sixteenth of one costs a few decibels
            // for the few hundred symbols the timing loop then takes to find
            // the rest. Near the best, the error grows as the square of the
            // timing's error, so the parabola through it and its neighbours
            // has its bottom where the timing is.
            let step = ((shift + 1.0) * RESYNC_STEPS as f64).round() as usize;
            if let (Some(Some(early)), Some(Some(late))) = (step.checked_sub(1).map(|s| by_step[s]), by_step.get(step + 1)) {
                let curve = early - 2.0 * mse + late;
                if curve > 0.0 {
                    let off = (0.5 * (early - late) / curve).clamp(-0.5, 0.5);
                    shift += off / RESYNC_STEPS as f64;
                }
            }
        }
        readings.sort_by(f64::total_cmp);
        let median = readings.get(readings.len() / 2).copied().unwrap_or(f64::MAX);
        Some(Found { mse, median, from: span.from, moved: shift * half, turned, gain, distance, window })
    }

    /// On a dense constellation: read the newest symbols again at every
    /// sixteenth of a half symbol either side, turn each reading through a
    /// quarter in steps of a degree and a half, fit its gain and phase to the
    /// decisions by least squares, and try a little either side of the best
    /// (`v34/receiver.rs:1015-1091`).
    ///
    /// All of that because a dense constellation gives nothing less a hold:
    /// its points read as noise a sixty-fourth of a symbol out, a degree or
    /// two turned, or with the gain a percent off, and the fourth power a
    /// sparse resync turns by is near nought on it.
    pub(super) fn search_dense(&self, turn: f64) -> Option<Found> {
        let half = self.front.half * (1.0 + self.front.drift);
        // Four symbols short of the newest, so that reading them later still
        // has samples to read.
        let span = self.span(DENSE_WINDOW, SHORT)?;
        let distance = self.distance(&span, DENSE_WINDOW);
        let reference = if self.options.resync_derotate { self.rotation - turn * distance } else { self.rotation };
        let outputs_at = |moved: f64| -> Option<(Vec<Complex>, f64)> {
            let read: Vec<Complex> = (0..span.count)
                .map(|m| self.front.interpolate(span.start_time + m as f64 * half + moved))
                .collect::<Option<_>>()?;
            Some(self.outputs(&read, DENSE_WINDOW, turn))
        };
        let settle = |outputs: &[Complex]| {
            let turns = (90.0 / DENSE_DEGREES) as usize;
            let coarse = (0..turns)
                .map(|q| Complex::from_polar(1.0, -(reference + (q as f64 * DENSE_DEGREES).to_radians())))
                .map(|c| (self.mse(outputs, c), c))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map_or(Complex::ONE, |(_, c)| c);
            self.fit(outputs, coarse)
        };
        let mut readings = Vec::new();
        let mut best: Option<(f64, f64, Complex, f64)> = None;
        for step in 0..2 * DENSE_STEPS {
            let moved = (step as f64 / DENSE_STEPS as f64 - 1.0) * half;
            let Some((outputs, scale)) = outputs_at(moved) else { continue };
            let (found, c) = settle(&outputs);
            readings.push(found);
            if best.is_none_or(|b| found < b.0) {
                best = Some((found, moved, c, scale));
            }
        }
        let (mut found, mut moved, mut c, mut scale) = best?;
        let coarse = moved;
        for fine in 1..DENSE_FINE / DENSE_STEPS {
            for sign in [-1.0, 1.0] {
                let tried = coarse + sign * fine as f64 * half / DENSE_FINE as f64;
                let Some((outputs, s)) = outputs_at(tried) else { continue };
                // The fit goes on from the best so far, in its own scale.
                let (e, g) = self.fit(&outputs, c.scale(scale / s));
                if e < found {
                    (found, moved, c, scale) = (e, tried, g, s);
                }
            }
        }
        readings.sort_by(f64::total_cmp);
        let median = readings[readings.len() / 2];
        Some(Found {
            mse: found,
            median,
            from: span.from,
            moved,
            turned: -c.arg(),
            gain: Some(scale * c.abs()),
            distance,
            window: DENSE_WINDOW,
        })
    }

    /// Carry on from a reading that fitted: every half symbol from its window
    /// on read again on the moved grid, anything the moved grid needs samples
    /// for that have not come yet made again when they have, and the gain
    /// and the carrier's phase as the reading found them
    /// (`v34/receiver.rs:1097-1136`).
    pub(super) fn take_up(&mut self, found: &Found, slip: bool) {
        if let Some(gain) = found.gain {
            if self.options.agc {
                // The gain goes in front of the equaliser, where the gain
                // control can go on from it; V.34 put it into the taps.
                self.gain.refit(self.gain.value() * gain);
            } else {
                for tap in &mut self.taps {
                    *tap = tap.scale(gain);
                }
            }
        }
        let redo = (found.from - self.front.first) as usize;
        let mut keep = self.front.halves.len();
        for m in redo..self.front.halves.len() {
            let time = self.front.times[m] + found.moved;
            match self.front.interpolate(time) {
                Some(value) => {
                    self.front.halves[m] = value;
                    self.front.times[m] = time;
                }
                None => {
                    keep = m;
                    break;
                }
            }
        }
        if keep < self.front.halves.len() {
            self.front.due = self.front.times[keep] + found.moved;
            self.front.halves.truncate(keep);
            self.front.times.truncate(keep);
            self.front.made = self.front.first + keep as u64;
        } else {
            self.front.due += found.moved;
        }
        // V.34 carried the phase half a window on from the window's middle;
        // the next symbol is further on than that.
        let ahead = if self.options.extrapolate { found.distance } else { found.window as f64 / 2.0 };
        self.rotation = (found.turned + self.turn * ahead).rem_euclid(std::f64::consts::TAU);
        self.lost = None;
        self.recent.clear();
        self.refused.clear();
        if slip {
            self.slips += 1;
        }
        if self.options.relative_gate {
            // What the errors settle to after a jump is what the reading
            // found: the settled error from before would refuse every symbol
            // of a line that has got noisier, and nothing would ever learn.
            // But no more than 6 dB worse than before, the rewind having put
            // that back. A reading made in the middle of a -6 dB ramp found
            // 23 dB, and a gate nine times that let the rest of the ramp
            // teach every loop, which then tracked at 25 dB for seconds. A
            // line that really has got noisier gets there in a few resyncs.
            self.settled = found.mse.min(SETTLED_RISE * self.settled);
        }
        if self.options.rewind_safely {
            // Copies from before the jump would put the old timing back.
            self.earlier.clear();
            self.snapshot();
        }
    }
}
