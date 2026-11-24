//! The front end: a fixed mixer, the mixed samples kept for reading again, and
//! one filter that both rejects the mixer's image and places each sample
//! where the timing asks for it.
//!
//! Copied from `v34/receiver.rs:454-511` and `:642-690`. The mixer is never
//! corrected: a carrier offset appears only as a slow rotation, taken out
//! after the equaliser, so the equaliser's input is a fixed linear function
//! of the line and anything stored can be read again at any offset later --
//! after a slip, or as the rows of a least-squares solve (core.md 1, row 1).
//! There is no matched filter; the equaliser is the matched filter.

use std::collections::VecDeque;
use std::sync::Arc;

use super::Band;
use crate::Complex;

/// Taps of the interpolating low-pass filter, and the fractional positions its
/// table is made for (`v34/receiver.rs:57-58`).
pub(super) const FILTER_TAPS: usize = 64;
const FILTER_PHASES: usize = 256;

/// Half-symbol samples kept: 0.85 s at 2400 symbols a second, enough for a
/// training window and its retry (`v34/receiver.rs:65`).
const KEPT: usize = 4096;

/// Mixed-down samples kept for reading again after a slip: a second at
/// 16 kHz (`v34/receiver.rs:98`).
const HISTORY: usize = 16_384;

/// How long the carrier-present envelope takes to follow the signal. The
/// V.32 receiver this core is first built for reported its carrier from a
/// 20 ms envelope of the in-band amplitude, and a hang-up is heard by it
/// going (contract.md 1.3).
const ENVELOPE_SECONDS: f64 = 0.020;

#[derive(Debug, Clone)]
pub(super) struct Front {
    /// Mixer phase, as a fraction of a turn, and its step a sample.
    phase: f64,
    step: f64,
    /// Mixed-down samples, newest last, and the index of the oldest.
    history: VecDeque<Complex>,
    history_first: u64,
    /// Samples taken in.
    pub(super) taken: u64,
    /// Where the next half-symbol sample falls, in samples since the start.
    pub(super) due: f64,
    /// Samples a half symbol, nominally, and the timing loop's correction to
    /// it as a fraction.
    pub(super) half: f64,
    pub(super) drift: f64,
    table: Arc<[f64]>,
    pub(super) halves: VecDeque<Complex>,
    /// When each of them was taken, in samples.
    pub(super) times: VecDeque<f64>,
    /// Index of the first sample in `halves`, and of the next to be made.
    pub(super) first: u64,
    pub(super) made: u64,
    /// The half-symbol samples' mean power, slowly: whether anything is
    /// arriving at all (`v34/receiver.rs:681`).
    pub(super) power: f64,
    /// Their mean amplitude over 20 ms, and the share of it each half keeps.
    envelope: f64,
    envelope_keep: f64,
}

impl Front {
    pub(super) fn new(band: Band) -> Self {
        // The filter table as `v34/receiver.rs:456-472`, with the cutoff
        // the band's own.
        let cutoff = band.cutoff;
        let mut table = vec![0.0; FILTER_PHASES * FILTER_TAPS];
        for ph in 0..FILTER_PHASES {
            let row = &mut table[ph * FILTER_TAPS..(ph + 1) * FILTER_TAPS];
            for (i, tap) in row.iter_mut().enumerate() {
                let tau = ph as f64 / FILTER_PHASES as f64 + (FILTER_TAPS / 2) as f64 - 1.0 - i as f64;
                let x = 2.0 * cutoff * tau / band.fs;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let edge = (FILTER_TAPS / 2) as f64;
                let taper = if tau.abs() >= edge { 0.0 } else { kaiser(tau / edge, 8.0) };
                *tap = sinc * taper;
            }
            let sum: f64 = row.iter().sum();
            for tap in row.iter_mut() {
                *tap /= sum;
            }
        }
        Self {
            phase: 0.0,
            step: band.carrier / band.fs,
            history: VecDeque::with_capacity(HISTORY),
            history_first: 0,
            taken: 0,
            due: FILTER_TAPS as f64,
            half: band.fs / band.baud / 2.0,
            drift: 0.0,
            table: table.into(),
            halves: VecDeque::with_capacity(KEPT),
            times: VecDeque::with_capacity(KEPT),
            first: 0,
            made: 0,
            power: 0.0,
            envelope: 0.0,
            envelope_keep: (-1.0 / (ENVELOPE_SECONDS * 2.0 * band.baud)).exp(),
        }
    }

    /// Mix one line sample down and keep it (`v34/receiver.rs:643-652`).
    pub(super) fn push(&mut self, sample: f64) {
        let angle = std::f64::consts::TAU * self.phase;
        let mixed = Complex::new(angle.cos(), -angle.sin()).scale(2.0 * sample);
        self.phase += self.step;
        self.phase -= self.phase.floor();
        self.history.push_back(mixed);
        if self.history.len() > HISTORY {
            self.history.pop_front();
            self.history_first += 1;
        }
        self.taken += 1;
    }

    /// The next half-symbol sample, if every sample the filter needs for it
    /// is here, and its index (`v34/receiver.rs:653-657`, `:680-690`).
    pub(super) fn make_half(&mut self) -> Option<(Complex, u64)> {
        let value = self.interpolate(self.due)?;
        let at = self.due;
        self.due += self.half * (1.0 + self.drift);
        self.power += 0.002 * (value.norm_sqr() - self.power);
        self.envelope = self.envelope_keep * self.envelope + (1.0 - self.envelope_keep) * value.abs();
        let index = self.made;
        self.made += 1;
        self.halves.push_back(value);
        self.times.push_back(at);
        if self.halves.len() > KEPT {
            self.halves.pop_front();
            self.times.pop_front();
            self.first += 1;
        }
        Some((value, index))
    }

    /// Where half-symbol sample `index` falls on a grid `step` samples apart
    /// that meets the stored one at `anchor`, if `anchor` is still stored.
    pub(super) fn regridded(&self, anchor: u64, step: f64, index: u64) -> Option<f64> {
        let at = anchor.checked_sub(self.first)? as usize;
        let t0 = *self.times.get(at)?;
        Some(t0 + (index as f64 - anchor as f64) * step)
    }

    /// Every half-symbol sample from `anchor` on read again on a grid `step`
    /// samples apart, which meets the old one at `anchor`, and the rest made
    /// on it when their samples come. For a grid that training has found the
    /// far clock's drift for.
    pub(super) fn regrid(&mut self, anchor: u64, step: f64) {
        let Some(start) = anchor.checked_sub(self.first).map(|s| s as usize) else { return };
        if start >= self.halves.len() {
            return;
        }
        let t0 = self.times[start];
        let mut keep = self.halves.len();
        for m in start..self.halves.len() {
            let time = t0 + (m - start) as f64 * step;
            match self.interpolate(time) {
                Some(value) => {
                    self.halves[m] = value;
                    self.times[m] = time;
                }
                None => {
                    keep = m;
                    break;
                }
            }
        }
        self.due = t0 + (keep - start) as f64 * step;
        self.halves.truncate(keep);
        self.times.truncate(keep);
        self.made = self.first + keep as u64;
    }

    /// Whether the half-symbol samples have fallen so far behind the line
    /// that they must be made whether or not anyone is taking symbols: half
    /// the history, after which the oldest samples they need start to go.
    pub(super) fn behind(&self) -> bool {
        self.taken as f64 - self.due > (HISTORY / 2) as f64
    }

    /// The mixed-down signal at `time` samples, filtered, if every sample the
    /// filter reaches is here (`v34/receiver.rs:662-678`).
    pub(super) fn interpolate(&self, time: f64) -> Option<Complex> {
        let floor = time.floor();
        let mut ph = ((time - floor) * FILTER_PHASES as f64).round() as usize;
        let mut base = floor as i64;
        if ph == FILTER_PHASES {
            ph = 0;
            base += 1;
        }
        let from = base - (FILTER_TAPS / 2) as i64 + 1;
        let to = base + (FILTER_TAPS / 2) as i64;
        if from < self.history_first as i64 || to >= self.taken as i64 {
            return None;
        }
        let offset = (from - self.history_first as i64) as usize;
        let row = &self.table[ph * FILTER_TAPS..(ph + 1) * FILTER_TAPS];
        Some(row.iter().enumerate().fold(Complex::ZERO, |sum, (i, tap)| sum + self.history[offset + i] * *tap))
    }

    /// The `reach` half-symbol samples either side of `centre`, and it
    /// (`v34/receiver.rs:735-739`).
    pub(super) fn samples(&self, centre: u64, reach: usize) -> Option<Vec<Complex>> {
        let from = centre.checked_sub(reach as u64)?.checked_sub(self.first)? as usize;
        let to = from + 2 * reach + 1;
        (to <= self.halves.len()).then(|| self.halves.range(from..to).copied().collect())
    }

    /// The mean amplitude of the half-symbol samples over the last 20 ms.
    pub(super) fn envelope(&self) -> f64 {
        self.envelope
    }
}

/// The Kaiser window at `x` from -1 to 1 (`v34/receiver.rs:1286-1288`).
fn kaiser(x: f64, beta: f64) -> f64 {
    bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / bessel_i0(beta)
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= half / k as f64;
        let add = term * term;
        sum += add;
        if add < sum * 1e-16 {
            break;
        }
    }
    sum
}
