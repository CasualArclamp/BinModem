//! A single-carrier modulation fitted to a line it was not built for.
//!
//! [`crate::sc`] is built for three samples a symbol. Played sample for
//! sample on a line, its baud is therefore a third of the line's rate and
//! its band is four-fifths of what the line passes. On a sound card that is
//! a choice. On a call at 8 kHz it is 2666 baud between 150 and 3350 Hz,
//! where the call passes everything up to 4 kHz, and the baud is what the
//! rate of a single carrier is made of.
//!
//! So the modulation is run at a rate of its own, three times whatever baud
//! is wanted, and brought to the line's rate and back by the resampler
//! here. Nothing in the modulation changes: it is given a carrier and an
//! excess bandwidth that put its band where the line is ([`crate::sc::Shape::fitted`]).
//!
//! The resampler is a windowed sinc, long, because the band now runs to
//! within a few per cent of half the line's rate and a short kernel's
//! answer there depends on where between two samples it is asked.

use std::collections::VecDeque;
use std::f64::consts::PI;

use crate::interp::bessel_i0;
use crate::link::{Receiver, Sender};

/// Input samples the kernel reaches either side of the point it is asked
/// for, at a cutoff of half the input rate; more where the cutoff is lower.
const REACH: usize = 72;

/// Places between two input samples the kernel is worked out for. Between
/// those it is drawn straight.
const PHASES: usize = 256;

/// The window's shape: some 70 dB down outside the band.
const BETA: f64 = 7.0;

/// From one sample rate to another, by any ratio.
#[derive(Clone)]
pub struct Resampler {
    /// `PHASES + 1` rows of `2 * half` taps.
    table: Vec<f64>,
    half: usize,
    /// The inputs the kernel can reach, oldest first.
    past: VecDeque<f64>,
    /// Input samples to one output sample.
    step: f64,
    /// Where the next output falls, in input samples, counted from the
    /// oldest input kept, as the kernel's centre.
    at: f64,
}

impl std::fmt::Debug for Resampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resampler").field("step", &self.step).finish_non_exhaustive()
    }
}

impl Resampler {
    /// From `from` samples a second to `to`. What is passed is everything
    /// below half the slower of the two.
    pub fn new(from: f64, to: f64) -> Self {
        // The cutoff, as a share of the input rate's half.
        let cutoff = (to / from).min(1.0);
        let half = (REACH as f64 / cutoff).ceil() as usize;
        let taps = 2 * half;
        let norm = bessel_i0(BETA);
        let mut table = vec![0.0; (PHASES + 1) * taps];
        for phase in 0..=PHASES {
            let frac = phase as f64 / PHASES as f64;
            let row = &mut table[phase * taps..(phase + 1) * taps];
            for (j, tap) in row.iter_mut().enumerate() {
                // Tap j is the input j - (half - 1) samples on from the one
                // at or before the point asked for.
                let x = j as f64 - (half as f64 - 1.0) - frac;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (PI * cutoff * x).sin() / (PI * cutoff * x) };
                let r = x / half as f64;
                let window = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(BETA * (1.0 - r * r).sqrt()) / norm };
                *tap = cutoff * sinc * window;
            }
        }
        Self { table, half, past: VecDeque::from(vec![0.0; taps]), step: from / to, at: (half - 1) as f64 }
    }

    /// One input sample, and each output sample it completes.
    pub fn feed(&mut self, x: f64, mut out: impl FnMut(f64)) {
        self.past.push_back(x);
        let taps = 2 * self.half;
        // An output wants `half` inputs after the one at or before it.
        while self.at.floor() as usize + self.half < self.past.len() {
            let whole = self.at.floor();
            let place = (self.at - whole) * PHASES as f64;
            let phase = (place.floor() as usize).min(PHASES - 1);
            let part = place - phase as f64;
            let first = whole as usize + 1 - self.half;
            let (low, high) = (&self.table[phase * taps..(phase + 1) * taps], &self.table[(phase + 1) * taps..(phase + 2) * taps]);
            let mut sum = 0.0;
            for (j, sample) in self.past.iter().skip(first).take(taps).enumerate() {
                sum += sample * (low[j] + part * (high[j] - low[j]));
            }
            out(sum);
            self.at += self.step;
        }
        // Let go of what no later output reaches back to.
        let spare = (self.at.floor() as usize + 1).saturating_sub(self.half);
        for _ in 0..spare {
            self.past.pop_front();
        }
        self.at -= spare as f64;
    }

    /// Whether another input is wanted before there is another output.
    fn wants(&self) -> bool {
        self.at.floor() as usize + self.half >= self.past.len()
    }
}

/// A transmitter built for one rate, sending on a line at another.
pub struct Fitted {
    inner: Box<dyn Sender>,
    resampler: Resampler,
    ready: VecDeque<f64>,
    /// How many times faster than the rate it believes in the inner
    /// modulation's samples really go by.
    pace: f64,
}

impl std::fmt::Debug for Fitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fitted").field("pace", &self.pace).finish_non_exhaustive()
    }
}

impl Fitted {
    /// `inner` believes it runs at `believed` samples a second. It really
    /// runs at `own`, and the line at `line`.
    pub fn new(inner: Box<dyn Sender>, believed: f64, own: f64, line: f64) -> Self {
        Self { inner, resampler: Resampler::new(own, line), ready: VecDeque::new(), pace: own / believed }
    }
}

impl Sender for Fitted {
    fn wants(&self) -> Option<crate::frame::Layout> {
        self.inner.wants()
    }
    fn give(&mut self, line: Vec<u8>) {
        self.inner.give(line)
    }
    fn next_sample(&mut self) -> f64 {
        // One of the modulation's samples at a time, so that it is asked
        // for its next block's octets no sooner than it would have been.
        if self.ready.is_empty() && self.resampler.wants() {
            let sample = self.inner.next_sample();
            let ready = &mut self.ready;
            self.resampler.feed(sample, |y| ready.push_back(y));
        }
        while self.ready.is_empty() {
            let sample = self.inner.next_sample();
            let ready = &mut self.ready;
            self.resampler.feed(sample, |y| ready.push_back(y));
        }
        self.ready.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0)
    }
    fn adopt(&mut self, id: u8, table: crate::link::Table) {
        self.inner.adopt(id, table)
    }
    fn id(&self) -> u8 {
        self.inner.id()
    }
    fn reset(&mut self) {
        self.inner.reset()
    }
    fn rates(&self) -> (f64, f64) {
        let (line, frames) = self.inner.rates();
        (line * self.pace, frames * self.pace)
    }
    fn clipped(&self) -> u64 {
        self.inner.clipped()
    }
    fn trim(&mut self, db: f64) {
        self.inner.trim(db)
    }
}

/// The receiving half of [`Fitted`].
pub struct Unfitted {
    inner: Box<dyn Receiver>,
    resampler: Resampler,
    /// This end's own signal, on a line that is one pair of wires, brought
    /// to the modulation's rate as the line is.
    own: Option<Resampler>,
    own_ready: VecDeque<f64>,
    own_rates: (f64, f64),
    pace: f64,
}

impl std::fmt::Debug for Unfitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Unfitted").field("pace", &self.pace).finish_non_exhaustive()
    }
}

impl Unfitted {
    /// The receiving half of [`Fitted::new`].
    pub fn new(inner: Box<dyn Receiver>, believed: f64, own: f64, line: f64) -> Self {
        Self {
            inner,
            resampler: Resampler::new(line, own),
            own: None,
            own_ready: VecDeque::new(),
            own_rates: (line, own),
            pace: own / believed,
        }
    }
}

impl Receiver for Unfitted {
    fn feed(&mut self, sample: f64) {
        let (inner, own_ready) = (&mut self.inner, &mut self.own_ready);
        self.resampler.feed(sample, |y| {
            if let Some(own) = own_ready.pop_front() {
                inner.echo_under(own);
            }
            inner.feed(y);
        });
    }
    fn echo_under(&mut self, sample: f64) {
        let (from, to) = self.own_rates;
        let own_ready = &mut self.own_ready;
        self.own.get_or_insert_with(|| Resampler::new(from, to)).feed(sample, |y| own_ready.push_back(y));
    }
    fn echo_again(&mut self) {
        self.inner.echo_again()
    }
    fn echo_left_db(&self) -> Option<f64> {
        self.inner.echo_left_db()
    }
    fn take(&mut self) -> Option<crate::link::Arrived> {
        self.inner.take()
    }
    fn proposal(&mut self) -> Option<crate::link::Table> {
        self.inner.proposal()
    }
    fn expect(&mut self, id: u8, table: crate::link::Table) {
        self.inner.expect(id, table)
    }
    fn id(&self) -> Option<u8> {
        self.inner.id()
    }
    fn back_off(&mut self, db: f64) {
        self.inner.back_off(db)
    }
    fn remeasure(&mut self, db: f64) {
        self.inner.remeasure(db)
    }
    fn health(&self) -> crate::link::Health {
        self.inner.health()
    }
    fn rates(&self) -> (f64, f64) {
        let (line, frames) = self.inner.rates();
        (line * self.pace, frames * self.pace)
    }
    fn spectrum(&self) -> Vec<crate::link::Bin> {
        let mut bins = self.inner.spectrum();
        for bin in &mut bins {
            bin.hz *= self.pace;
        }
        bins
    }
    fn points(&mut self) -> Vec<(f64, f64)> {
        self.inner.points()
    }
    fn shown_bits(&self) -> u8 {
        self.inner.shown_bits()
    }
    fn code(&self) -> Option<(usize, usize)> {
        self.inner.code()
    }
    fn fix(&mut self, bits: Option<u8>, code: Option<f64>) {
        self.inner.fix(bits, code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sc::{Rx, Shape, Tx};
    use crate::station::Station;

    /// G.711 mu-law as a channel: a level in, the level its codeword
    /// stands for out (G.711 Table 2a, by its segments).
    pub(crate) fn mu_law(x: f64) -> f64 {
        let sample = (x * 32768.0).round().clamp(-32768.0, 32767.0) as i32;
        let sign = sample < 0;
        let magnitude = (sample.abs().min(32635) + 132) as u32;
        let segment = (31 - (magnitude >> 7).leading_zeros()).min(7);
        let step = (magnitude >> (segment + 3)) & 15;
        // The middle of the step, less the bias: what a decoder gives.
        let level = (((step << 1) + 33) << (segment + 2)) as i32 - 132;
        f64::from(if sign { -level } else { level }) / 32768.0
    }

    type End = Station<Box<dyn Sender>, Box<dyn Receiver>>;

    /// One end of a single-carrier call on a line at `line` samples a
    /// second, at `baud` with its carrier at `carrier` and this much
    /// excess bandwidth.
    fn end(line: f64, baud: f64, carrier: f64, rolloff: f64, sends: u8, hears: u8) -> End {
        let own = 3.0 * baud;
        let shape = Shape::fitted(carrier * 48_000.0 / own, rolloff);
        let tx: Box<dyn Sender> = Box::new(Fitted::new(Box::new(Tx::new(shape.clone().coloured(sends))), 48_000.0, own, line));
        let rx: Box<dyn Receiver> = Box::new(Unfitted::new(Box::new(Rx::new(shape.coloured(hears))), 48_000.0, own, line));
        Station::new(line, vec![(tx, rx)])
    }

    #[test]
    fn a_tone_comes_through_a_change_of_rate_as_it_went_in() {
        for (from, to, hz) in [(9_600.0, 8_000.0, 3_850.0), (8_000.0, 9_600.0, 3_850.0), (8_000.0, 10_800.0, 150.0), (10_800.0, 8_000.0, 2_000.0)] {
            let mut resampler = Resampler::new(from, to);
            let mut out = Vec::new();
            for n in 0..6000 {
                resampler.feed((std::f64::consts::TAU * hz * f64::from(n) / from).sin(), |y| out.push(y));
            }
            // Against the same tone at the new rate, wherever it starts.
            let (mut s, mut c) = (0.0, 0.0);
            let body = &out[1000..out.len() - 200];
            for (n, y) in body.iter().enumerate() {
                let turn = std::f64::consts::TAU * hz * n as f64 / to;
                s += y * turn.sin();
                c += y * turn.cos();
            }
            let n = body.len() as f64;
            let amplitude = 2.0 * (s * s + c * c).sqrt() / n;
            let power: f64 = body.iter().map(|y| y * y).sum::<f64>() / n;
            let rest = (power - amplitude * amplitude / 2.0).max(1e-30);
            assert!((amplitude - 1.0).abs() < 0.05, "{hz} Hz from {from} to {to}: amplitude {amplitude}");
            assert!(10.0 * (0.5 / rest).log10() > 55.0, "{hz} Hz from {from} to {to}: {:.1} dB", 10.0 * (0.5 / rest).log10());
        }
    }

    /// What each baud carries through G.711: for looking at.
    #[test]
    #[ignore = "a measurement, not a test"]
    fn what_each_baud_carries_through_g711() {
        let line = 8_000.0;
        for (baud, carrier, rolloff) in [
            (8000.0 / 3.0, 1750.0, 0.2),
            (3000.0, 1950.0, 0.2),
            (3200.0, 2000.0, 0.15),
            (3200.0, 2000.0, 0.2),
            (3429.0, 2000.0, 0.1),
            (3429.0, 2000.0, 0.12),
            (3600.0, 2000.0, 0.06),
            (3600.0, 2000.0, 0.08),
            (3700.0, 2000.0, 0.05),
        ] {
            let (mut a, mut b) = (end(line, baud, carrier, rolloff, 0, 1), end(line, baud, carrier, rolloff, 1, 0));
            let (mut from_a, mut from_b) = ([0.0], [0.0]);
            let mut loaded = None;
            for n in 0..(60.0 * line) as usize {
                let (to_a, to_b) = ([mu_law(from_b[0])], [mu_law(from_a[0])]);
                a.step(&to_a, &mut from_a);
                b.step(&to_b, &mut from_b);
                a.read();
                b.read();
                if loaded.is_none() && a.channels[0].loaded() && b.channels[0].loaded() {
                    loaded = Some(n as f64 / line);
                }
            }
            let health = b.channels[0].rx.health();
            println!(
                "{baud:6.0} baud, carrier {carrier:.0}, rolloff {rolloff}: band {:.0} to {:.0} Hz, loaded at {loaded:?} s, {:.1} dB, {:.0} bit/s, code {:?}, grid of {} bits",
                carrier - baud * (1.0 + rolloff) / 2.0,
                carrier + baud * (1.0 + rolloff) / 2.0,
                health.snr_db,
                b.channels[0].rx.rates().1,
                b.channels[0].rx.code(),
                b.channels[0].rx.shown_bits(),
            );
        }
    }
}
