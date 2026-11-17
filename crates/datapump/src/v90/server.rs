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
use crate::v34::phase2::V92Wish;

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

/// What this server wants of V.92, for [`Line::with_v92`]: capable, and PCM
/// upstream if the analogue modem asks for it with a Table 18 INFO1a.
///
/// Short phase 2 stays off until its own stages are built, and is the only
/// thing this leaves out: it buys a second or two of start-up, not a rate.
pub fn wish(pcm_upstream: bool) -> V92Wish {
    V92Wish { capable: true, short_phase2: false, pcm_upstream, ..V92Wish::default() }
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
    /// The levels the digital modem chose, at the network's own rate and not
    /// yet taken: what a line that *is* the network wants instead of `out`
    /// (see [`Line::take_exact`]).
    exact: Vec<f64>,
    /// What the network delivered, at its own rate and not yet used: see
    /// [`Line::give_exact`].
    exact_in: VecDeque<f64>,
}

impl Line {
    /// From the end of V.8, with INFO0d saying `info0d`.
    pub fn new(fs: f64, info0d: Info0d, lead_in: f64) -> Self {
        Self {
            modem: Digital::new(info0d, lead_in),
            down: Resampler::new(fs, digital::FS),
            up: Resampler::new(digital::FS, fs),
            arrived: Vec::with_capacity(4),
            made: Vec::with_capacity(8),
            out: VecDeque::from(vec![0.0; SLACK]),
            exact: Vec::new(),
            exact_in: VecDeque::new(),
        }
    }

    /// What the network delivered, one level for every 8000 Hz sample, for
    /// the samples [`Line::step`] is about to be given.
    ///
    /// The other half of [`Line::take_exact`]. The samples `step` takes are
    /// the network's put up to the line's rate, and it brings them down again
    /// with a filter of its own; on a line that carries G.711 itself that is
    /// two filters between the codec and a receiver whose signal is the
    /// codec's levels, and the second of them takes the top of the band with
    /// it. Given these, `step` keeps its own conversion for the clock alone
    /// and hands the digital modem the level that arrived. It also lets the
    /// V.92 receiver see that what it hears are codewords, which is how it
    /// knows there is an A/D converter's rounding to allow for.
    pub fn give_exact(&mut self, levels: &[f64]) {
        self.exact_in.extend(levels.iter().copied());
        // The conversion's own delay is all that should ever stand here.
        while self.exact_in.len() > 4096 {
            self.exact_in.pop_front();
        }
    }

    /// The digital modem's own levels since the last call, one for every
    /// 8000 Hz sample it has made, appended to `into`.
    ///
    /// The samples [`Line::step`] returns are these put through a rate
    /// conversion, for a line that runs at some other rate and has a
    /// softphone's encoder beyond it. A line that carries G.711 itself has no
    /// use for the conversion and every reason to avoid it: taken down to
    /// 8000 Hz again by a second filter, the levels come back a little short
    /// of what they were at the top of the band, which is where a PCM signal
    /// keeps half of what it has to say. Handed these, the encoder turns each
    /// level into exactly the codeword it was chosen to be.
    pub fn take_exact(&mut self, into: &mut Vec<f64>) {
        into.append(&mut self.exact);
    }

    /// Offer V.92 from this server: INFO0d's capability bits and what this
    /// end wants of PCM upstream (see [`Digital::with_v92`]).
    ///
    /// A V.92 digital modem is a V.90 one with more to say in phase 2 --
    /// "the digital modem ... is as defined in clause 5 of V.90" (5/V.92) --
    /// so everything here stays as it was and only what is *said* changes.
    pub fn with_v92(mut self, wish: V92Wish) -> Self {
        self.modem = self.modem.with_v92(wish);
        self
    }

    /// Begin with the rest of short Phase 1 after this end's QCA1d (see
    /// [`Digital::after_quick`]). Build the line with V.8's handover silence
    /// as its lead-in: it is the silence that follows TONEq.
    pub fn after_quick(mut self, uqts: u8, level: v8::quick::AnspcmLevel) -> Self {
        self.modem = self.modem.after_quick(uqts, level);
        self
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
            let arrived = self.exact_in.pop_front().unwrap_or(self.arrived[i]);
            let level = self.modem.step(arrived);
            self.exact.push(level);
            // Nobody taking them is the ordinary case on a sound card.
            if self.exact.len() > 4096 {
                self.exact.drain(..2048);
            }
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
