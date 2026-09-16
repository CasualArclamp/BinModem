//! The digital modem on a sound card: what answers a V.90 call when the line
//! is a VoIP call rather than a trunk.
//!
//! V.90's digital modem is "connected to a digital network" (1), choosing the
//! codewords the network carries. A softphone's G.711 encoder is a network
//! interface of a kind: whatever arrives at it as a linear sample goes out as
//! the codeword nearest it. So the digital modem's levels are sent as samples
//! at exactly those values, at unity gain, and the encoder turns them back
//! into the codewords they were -- as nearly as the softphone's own sample
//! rate conversion allows.
//!
//! That conversion is the part nobody here controls. Between this modem and
//! the encoder there are resamplers, and they sample the signal at whatever
//! phase they happen to, and band-limit it on the way. To the analogue modem
//! at the far end, all of that is one more linear stretch of line before the
//! codec, plus the codec's quantising noise on top of it: its equaliser
//! learns the stretch on TRN1d, and its DIL measures the noise, and it picks
//! a rate that stands clear of it. With nothing resampling on the way, it
//! comes to what a trunk would give.
//!
//! The digital modem itself runs at the network's 8000 Hz. This converts
//! both ways between that and the line's rate, with windowed-sinc
//! interpolation that puts every codeword on an output sample exactly when
//! the line runs at a multiple of 8000.

use std::collections::VecDeque;

use dsp::Resampler;

use crate::v34::info::{Info0, Info0d};

use super::digital;
use super::startup::{Digital, Status};

/// Output samples kept in hand, so that a conversion that comes out a
/// sample early or late never leaves the line without one.
const SLACK: usize = 8;

/// What this end says it is in INFO0d: μ-law, the 1664-point upstream
/// constellation, and the powers of a real server's (−10 dBm0 nominal, a
/// ceiling of −12 dBm0 on what the analogue modem may ask for).
pub fn ours() -> Info0d {
    Info0d {
        v34: Info0 { constellation_1664: true, ..Info0::default() },
        nominal_power: 4,
        max_power: 23,
        power_at_codec: true,
        a_law: false,
        upstream_3429: false,
    }
}

/// The digital modem, at the line's rate.
#[derive(Debug, Clone)]
pub struct Line {
    modem: Digital,
    down: Resampler,
    up: Resampler,
    arrived: Vec<f64>,
    made: Vec<f64>,
    out: VecDeque<f64>,
}

impl Line {
    /// From the end of V.8, with INFO0d saying `info0d`.
    pub fn new(fs: f64, info0d: Info0d) -> Self {
        Self {
            modem: Digital::new(info0d),
            down: Resampler::new(fs, digital::FS),
            up: Resampler::new(digital::FS, fs),
            arrived: Vec::with_capacity(4),
            made: Vec::with_capacity(8),
            out: VecDeque::from(vec![0.0; SLACK]),
        }
    }

    pub fn modem(&self) -> &Digital {
        &self.modem
    }

    pub fn modem_mut(&mut self) -> &mut Digital {
        &mut self.modem
    }

    /// One line sample in, one out.
    pub fn step(&mut self, line: f64) -> f64 {
        self.arrived.clear();
        self.down.process(line, &mut self.arrived);
        for i in 0..self.arrived.len() {
            let level = self.modem.step(self.arrived[i]);
            self.made.clear();
            self.up.process(level, &mut self.made);
            self.out.extend(self.made.iter().copied());
        }
        // Never more than the slack behind, whatever the two conversions do
        // between them over a long call.
        while self.out.len() > 2 * SLACK {
            self.out.pop_front();
        }
        self.out.pop_front().unwrap_or(0.0)
    }

    pub fn status(&self) -> Status {
        self.modem.status()
    }

    pub fn phase(&self) -> &'static str {
        self.modem.phase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At twice the network's rate every other sample out is a level the
    /// digital modem chose, exactly: what a softphone that simply drops every
    /// other sample would hand its encoder.
    #[test]
    fn at_twice_the_rate_every_other_sample_is_the_level_itself() {
        let mut up = Resampler::new(8000.0, 16_000.0);
        let levels: Vec<f64> = (0..400).map(|k| ((k * 37 % 101) as f64 - 50.0) / 60.0).collect();
        let mut out = Vec::new();
        for &l in &levels {
            up.process(l, &mut out);
        }
        // Find the delay, then check every level lands on a sample.
        let delay = (0..64).find(|&d| (out[d] - levels[0]).abs() < 1e-9 && (out[d + 2] - levels[1]).abs() < 1e-9).expect("no alignment");
        for (k, &l) in levels.iter().enumerate().take(300) {
            assert!((out[delay + 2 * k] - l).abs() < 1e-9, "level {k}: {} for {l}", out[delay + 2 * k]);
        }
    }
}
