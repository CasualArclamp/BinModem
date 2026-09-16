//! A route between a V.90 digital modem and an analogue modem, simulated.
//!
//! The digital modem hands the network a level every 125 microseconds, and
//! the network carries it as a G.711 codeword -- so anything that is not a
//! codeword already is quantised to one. Towards the analogue modem the codec
//! turns codewords into a waveform through its reconstruction filter, and the
//! loop adds its noise; the other way the codec filters and samples the
//! analogue modem's waveform and quantises it. A T1 on the way can rob a bit
//! from every sixth octet, and a digital pad can scale every level.
//!
//! Two things a VoIP call adds are here too. The analogue modem's sound card
//! runs on its own clock, some tens of parts per million off the network's,
//! so every waveform is resampled between the two. And the jitter buffer in
//! the softphone now and then plays twenty milliseconds of made-up audio, or
//! drops twenty, which shifts everything after it by 160 codewords.
//!
//! Nothing here is a claim about any real network, only about what V.90 has
//! to get through.

use std::collections::VecDeque;

use super::ucode::{self, Law};

/// The network's rate.
const NETWORK_FS: f64 = 8000.0;

/// Codewords either side of an instant the codec's reconstruction reaches:
/// short, since a reconstruction filter that rings on for longer than an
/// equaliser reaches is not one any codec has.
const DOWN_REACH: i64 = 20;

/// Line samples either side the codec's anti-alias filter reaches, in
/// seconds of line: long, since the upstream's band runs close to 4 kHz.
const UP_REACH: f64 = 0.008;

/// A slip's length, in codewords: one twenty-millisecond packet.
pub const SLIP: usize = 160;

/// A route. The analogue side runs at `fs`.
#[derive(Debug, Clone)]
pub struct Network {
    law: Law,
    fs: f64,
    /// How far the analogue modem's clock is off the network's, as a
    /// fraction: its samples are `(1 + skew) / fs` apart in network time.
    skew: f64,
    /// Downstream: carried codewords with the network time of the first, and
    /// where the next analogue sample falls.
    down_levels: VecDeque<f64>,
    down_first: f64,
    down_next: f64,
    down_delay: f64,
    /// Upstream: the analogue modem's samples with the analogue time of the
    /// first, and the next network sample's index.
    up_samples: VecDeque<f64>,
    up_first: f64,
    up_next: f64,
    up_delay: f64,
    /// Network time: codewords sent downstream so far.
    now: u64,
    noise: f64,
    seed: u64,
    /// Which of the six octets a robbed bit lands on, if one does.
    robbed: Option<usize>,
    octets: usize,
    /// A digital pad, as a gain on every downstream level.
    pad: f64,
    /// Whether the upstream is quantised to G.711.
    quantised: bool,
    /// What the analogue modem's line level is to the codec's full scale.
    ///
    /// Every modem here leaves at a root-mean-square of 0.707, a full-scale
    /// sine, and a codec quantising that clips every peak. A telephone line
    /// delivers a modem's -9 to -12 dBm to the codec well inside its range;
    /// this is where that happens.
    up_gain: f64,
    /// Downstream slips: how often, and whether audio is made up or lost.
    slips: Option<(u64, bool)>,
    /// Codewords of a lost stretch still to drop.
    dropping: usize,
    /// The last slip's worth of codewords, for concealment to repeat.
    recent: VecDeque<f64>,
    slip_count: u32,
}

impl Network {
    pub fn new(law: Law, fs: f64) -> Self {
        Self {
            law,
            fs,
            skew: 0.0,
            down_levels: VecDeque::new(),
            down_first: 0.0,
            down_next: 0.0,
            down_delay: 0.0,
            up_samples: VecDeque::new(),
            up_first: 0.0,
            up_next: 0.0,
            up_delay: 0.0,
            now: 0,
            noise: 0.0,
            seed: 0x2545_f491_4f6c_dd1d,
            robbed: None,
            octets: 0,
            pad: 1.0,
            quantised: true,
            up_gain: 0.25,
            slips: None,
            dropping: 0,
            recent: VecDeque::with_capacity(SLIP),
            slip_count: 0,
        }
    }

    /// Each way's delay, in seconds of line.
    pub fn with_delay(mut self, seconds: f64, _fs: f64) -> Self {
        self.down_delay = seconds;
        self.up_delay = seconds;
        self
    }

    /// The analogue modem's clock `ppm` parts per million fast.
    pub fn with_clock(mut self, ppm: f64) -> Self {
        self.skew = -ppm * 1e-6;
        self
    }

    /// Noise on the loop, as a level: one is full scale.
    pub fn with_noise(mut self, level: f64) -> Self {
        self.noise = level;
        self
    }

    /// The loop's noise from now on: a line that goes bad in the middle of a
    /// call.
    pub fn set_noise(&mut self, level: f64) {
        self.noise = level;
    }

    /// A robbed bit on every sixth downstream octet, starting at `phase`.
    pub fn with_robbed_bit(mut self, phase: usize) -> Self {
        self.robbed = Some(phase % 6);
        self
    }

    /// A digital pad of `db` on the downstream.
    pub fn with_pad(mut self, db: f64) -> Self {
        self.pad = 10f64.powf(-db / 20.0);
        self
    }

    /// The analogue modem's level at the codec, against its own.
    pub fn with_upstream_gain(mut self, gain: f64) -> Self {
        self.up_gain = gain;
        self
    }

    /// An upstream carried as it is, with no codec's quantising: for finding
    /// out what the quantising costs.
    pub fn unquantised(mut self) -> Self {
        self.quantised = false;
        self
    }

    /// A downstream slip every `seconds`: twenty milliseconds made up if
    /// `inserted`, lost otherwise.
    pub fn with_slips(mut self, seconds: f64, inserted: bool) -> Self {
        self.slips = Some(((seconds * NETWORK_FS) as u64, inserted));
        self
    }

    /// Slips so far.
    pub fn slips(&self) -> u32 {
        self.slip_count
    }

    fn gaussian(&mut self) -> f64 {
        let mut sum = 0.0;
        for _ in 0..4 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            sum += (self.seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
        }
        sum * 3f64.sqrt()
    }

    fn quantise(&self, level: f64) -> f64 {
        let (u, negative) = ucode::nearest(self.law, (level * 32768.0).round() as i32);
        ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 }
    }

    /// What the network makes of one downstream level: the nearest codeword,
    /// through whatever the route does to it.
    fn carry(&mut self, level: f64) -> f64 {
        let (u, negative) = ucode::nearest(self.law, (level * 32768.0).round() as i32);
        let mut octet = ucode::octet(self.law, u, negative);
        if self.robbed == Some(self.octets % 6) {
            octet |= 1;
        }
        self.octets += 1;
        let (u, negative) = ucode::from_octet(self.law, octet);
        let level = ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 };
        if self.pad == 1.0 { level } else { self.quantise(level * self.pad) }
    }

    /// One downstream level in, and whatever line samples the analogue modem
    /// hears by then out.
    pub fn down(&mut self, level: f64) -> Vec<f64> {
        let carried = self.carry(level);
        self.now += 1;
        // The jitter buffer, between the network and the sound card.
        if let Some((every, inserted)) = self.slips
            && self.now.is_multiple_of(every)
        {
            self.slip_count += 1;
            if inserted {
                // Twenty milliseconds of the last twenty, fading: what packet
                // loss concealment makes up.
                let tail: Vec<f64> = self.recent.iter().copied().collect();
                for (k, v) in tail.iter().enumerate() {
                    self.down_levels.push_back(v * (1.0 - k as f64 / tail.len() as f64));
                }
            } else {
                self.dropping = SLIP;
            }
        }
        if self.dropping > 0 {
            self.dropping -= 1;
        } else {
            self.down_levels.push_back(carried);
            if self.recent.len() == SLIP {
                self.recent.pop_front();
            }
            self.recent.push_back(carried);
        }
        let mut out = Vec::new();
        if self.down_delay > 0.0 {
            // The delay, as silence first.
            out.extend(std::iter::repeat_n(0.0, (self.down_delay * self.fs).round() as usize));
            self.down_delay = 0.0;
        }
        // Line samples up to where the reconstruction has everything it
        // needs, in the buffer's own time.
        let step = (1.0 + self.skew) * NETWORK_FS / self.fs;
        let last = self.down_first + self.down_levels.len() as f64 - 1.0;
        while self.down_next + DOWN_REACH as f64 <= last {
            let t = self.down_next;
            let centre = t.floor() as i64;
            let mut sum = 0.0;
            for j in centre - DOWN_REACH..=centre + DOWN_REACH {
                let index = j as f64 - self.down_first;
                if index < 0.0 {
                    continue;
                }
                let Some(&v) = self.down_levels.get(index as usize) else { continue };
                sum += v * kernel(t - j as f64, 3800.0 / NETWORK_FS, DOWN_REACH as f64 + 1.0);
            }
            let noise = self.noise * self.gaussian();
            out.push(sum + noise);
            self.down_next += step;
        }
        while self.down_first + (DOWN_REACH as f64) + 1.0 < self.down_next.floor() && self.down_levels.len() > 1 {
            self.down_levels.pop_front();
            self.down_first += 1.0;
        }
        out
    }

    /// The analogue modem's line samples since the last call in, and the
    /// level the digital modem gets out for this network sample.
    pub fn up(&mut self, samples: &[f64]) -> f64 {
        if self.up_delay > 0.0 {
            // The delay, as silence ahead of everything the modem says.
            self.up_samples.extend(std::iter::repeat_n(0.0, (self.up_delay * self.fs).round() as usize));
            self.up_delay = 0.0;
        }
        for &x in samples {
            let noise = self.noise * self.gaussian();
            self.up_samples.push_back(self.up_gain * x + noise);
        }
        // This network sample's instant, in the analogue modem's samples:
        // late enough that the modem has said everything the filter reaches,
        // since what it says answers a downstream that is itself late by the
        // reconstruction's reach.
        let per = self.fs / ((1.0 + self.skew) * NETWORK_FS);
        let reach = UP_REACH * self.fs;
        let lag = DOWN_REACH as f64 + 2.0 + UP_REACH * NETWORK_FS;
        let t = (self.up_next - lag).max(0.0) * per;
        self.up_next += 1.0;
        let centre = t.floor() as i64;
        let cutoff = 3700.0 / self.fs;
        let mut sum = 0.0;
        for j in centre - reach as i64..=centre + reach as i64 {
            let index = j as f64 - self.up_first;
            if index < 0.0 {
                continue;
            }
            let Some(&v) = self.up_samples.get(index as usize) else { continue };
            sum += v * kernel(t - j as f64, cutoff, reach + 1.0);
        }
        while self.up_first + reach + 2.0 < t && self.up_samples.len() > 1 {
            self.up_samples.pop_front();
            self.up_first += 1.0;
        }
        if self.quantised { self.quantise(sum) } else { sum }
    }
}

/// A windowed sinc at `cutoff` cycles a sample, reaching `edge` samples,
/// with unit gain at DC.
fn kernel(t: f64, cutoff: f64, edge: f64) -> f64 {
    if t.abs() >= edge {
        return 0.0;
    }
    let x = 2.0 * cutoff * t;
    let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
    let window = 0.42 + 0.5 * (std::f64::consts::PI * t / edge).cos() + 0.08 * (2.0 * std::f64::consts::PI * t / edge).cos();
    2.0 * cutoff * sinc * window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codeword_goes_down_and_comes_back_up_as_itself() {
        let mut net = Network::new(Law::Mu, 16_000.0).with_upstream_gain(1.0);
        let level = ucode::level(Law::Mu, 90);
        // A steady level: the filters settle to it both ways.
        let mut up = 0.0;
        for _ in 0..400 {
            let heard = net.down(level);
            up = net.up(&heard);
        }
        assert!((up - level).abs() < 0.01 * level, "{up} against {level}");
    }

    #[test]
    fn a_fast_clock_hears_more_samples() {
        let mut net = Network::new(Law::Mu, 16_000.0).with_clock(500.0);
        let mut heard = 0usize;
        for _ in 0..80_000 {
            heard += net.down(0.1).len();
        }
        // Ten seconds of network at 16 kHz is 160 000 samples; half a
        // thousandth fast is 80 more.
        assert!((heard as i64 - 160_080).abs() < 60, "{heard}");
    }

    #[test]
    fn a_slip_moves_everything_after_it_by_160_codewords() {
        for inserted in [true, false] {
            let mut net = Network::new(Law::Mu, 16_000.0).with_slips(1.0, inserted);
            let mut heard = 0usize;
            for _ in 0..12_000 {
                heard += net.down(0.1).len();
            }
            assert_eq!(net.slips(), 1);
            let expected = 24_000i64 + if inserted { 320 } else { -320 };
            assert!((heard as i64 - expected).abs() < 60, "{inserted}: {heard}");
        }
    }

    #[test]
    fn a_robbed_bit_moves_every_other_codeword_in_one_octet_of_six() {
        let mut net = Network::new(Law::Mu, 16_000.0).with_robbed_bit(3);
        let mut moved = [0usize; 6];
        for n in 0..600 {
            let u = (n % 128) as u8;
            let level = ucode::level(Law::Mu, u);
            let carried = net.carry(level);
            if (carried - level).abs() > 1e-9 {
                moved[n % 6] += 1;
            }
        }
        assert_eq!(moved[0], 0);
        assert!(moved[3] > 40, "{moved:?}");
        assert_eq!(moved.iter().sum::<usize>(), moved[3]);
    }
}
