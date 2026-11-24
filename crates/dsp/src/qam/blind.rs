//! The blind start: finding a constellation with no training sequence.
//!
//! V.34's receiver has none, and a call never needs one: every V.32 start-up
//! and retrain sends S, S-bar and a known TRN, and those are what a call
//! trains on. But a bare receiver fed a far end's data from the first sample
//! has to find it somehow, and the parts are all here (core.md 7.8). The
//! equaliser starts as the pulse's matched filter, which on a flat line
//! leaves the symbols clean enough to decide; the gain is set from the
//! output's power; and the resync search itself, which finds where the
//! symbols fall and how the carrier is turned from nothing but the samples,
//! is run on the newest window every 32 symbols until one reading stands
//! clear of the rest (design.md 3.8).
//!
//! The carrier's frequency is tried at nought, and once there is enough
//! signal for it, at what the symbols' fourth power says: raised to the
//! fourth, any constellation that is the same a quarter turn round leaves a
//! line at four times the offset. On a cross that line is only about a
//! seventh of the fourth power (design.md P2), so it wants 256 symbols.

use super::{AUDIBLE, Core, Density, Heard, Mode, REACH, Slicer, Via, apply};
use crate::{Complex, rrc_at};

/// Symbols of signal before the first look, and between looks.
const FIRST_LOOK: usize = 64;
const LOOK_EVERY: usize = 32;

/// Symbols of signal before the fourth power's turn is tried.
const TURN_AFTER: usize = 256;

#[derive(Debug, Clone, Default)]
pub(super) struct Blind {
    /// The newest half symbols' power, over about sixteen of them.
    quick: f64,
    /// Half symbols of signal in a row, and half symbols since the last look.
    signal: usize,
    since: usize,
}

impl Core {
    /// Look for `slicer`'s constellation with no training sequence, from a far
    /// end whose pulse is a root raised cosine of `rolloff`, or near enough.
    pub fn acquire_blind(&mut self, slicer: Slicer, rolloff: f64) {
        self.taps = matched(rolloff);
        self.slicer = slicer;
        self.turn = 0.0;
        self.rotation = 0.0;
        self.lost = None;
        self.recent.clear();
        self.refused.clear();
        self.gain = super::Agc::unity();
        self.pending = None;
        self.mode = Mode::Blind(Blind::default());
    }

    pub(super) fn blind_half(&mut self, half: Complex) {
        let Mode::Blind(blind) = &mut self.mode else { return };
        blind.quick += (half.norm_sqr() - blind.quick) / 16.0;
        if blind.quick >= AUDIBLE {
            blind.signal += 1;
        } else {
            blind.signal = 0;
        }
        blind.since += 1;
        if blind.signal < 2 * FIRST_LOOK || blind.since < 2 * LOOK_EVERY {
            return;
        }
        blind.since = 0;
        let long = blind.signal >= 2 * TURN_AFTER;
        self.look(long);
    }

    /// One look at the newest window.
    fn look(&mut self, long: bool) {
        self.next_symbol = self.front.made.saturating_sub(REACH as u64 + 2);
        // The gain, from the power of what the matched filter makes of the
        // newest symbols against the constellation's.
        let outputs = self.outputs_before(if long { TURN_AFTER } else { FIRST_LOOK });
        let power = outputs.iter().map(|y| y.norm_sqr()).sum::<f64>() / outputs.len().max(1) as f64;
        if power <= 0.0 {
            return;
        }
        let taps = self.taps.clone();
        let g = (self.slicer.mean_power() / power).sqrt();
        if self.options.agc {
            self.gain = super::Agc::at(g);
        } else {
            for tap in &mut self.taps {
                *tap = tap.scale(g);
            }
        }
        let mut turns = vec![0.0];
        if long {
            let fourth = outputs
                .windows(2)
                .fold(Complex::ZERO, |sum, pair| sum + pair[1] * pair[1] * pair[1] * pair[1] * (pair[0] * pair[0] * pair[0] * pair[0]).conj());
            turns.push(fourth.arg() / 4.0);
        }
        let dense = self.slicer.density() == Density::Dense;
        for turn in turns {
            let found = if dense { self.search_dense(turn) } else { self.search_sparse(turn) };
            let Some(found) = found else { continue };
            // No settled error to judge by yet: the reading has to stand
            // clear of the other shifts, and be well clear of garbage.
            if found.mse > 0.6 * found.median || found.mse > 0.5 * self.slicer.garbage() {
                continue;
            }
            self.turn = turn;
            self.take_up(&found, false);
            self.settled = found.mse;
            self.error = found.mse;
            self.residual = found.mse.sqrt();
            self.trained_snr = -10.0 * found.mse.max(1e-9).log10();
            self.ever_trained = true;
            self.mode = Mode::Tracking;
            self.heard.push_back(Heard::Trained { snr_db: self.trained_snr, via: Via::Blind });
            return;
        }
        // Not found: the matched filter as it was, for the next look to
        // scale afresh.
        self.taps = taps;
    }

    /// The equaliser's outputs at the newest `count` symbols on the grid as
    /// it stands, oldest first, before the gain.
    fn outputs_before(&self, count: usize) -> Vec<Complex> {
        (1..=count as u64)
            .rev()
            .filter_map(|k| self.next_symbol.checked_sub(2 * k))
            .filter_map(|centre| self.front.samples(centre, REACH))
            .map(|row| apply(&self.taps, &row))
            .collect()
    }
}

/// The pulse's matched filter at two samples a symbol, scaled so that a
/// symbol sent through that pulse comes out at its own size.
fn matched(rolloff: f64) -> Vec<Complex> {
    let pulse: Vec<f64> = (0..=2 * REACH).map(|i| rrc_at((i as f64 - REACH as f64) / 2.0, rolloff)).collect();
    let energy: f64 = pulse.iter().map(|p| p * p).sum();
    pulse.iter().map(|p| Complex::new(p / energy, 0.0)).collect()
}
