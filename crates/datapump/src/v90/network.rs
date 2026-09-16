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
//! Nothing here is a claim about any real network, only about what V.90 has
//! to get through.

use std::collections::VecDeque;

use super::ucode::{self, Law};

/// Taps either side of the centre of the codec's filters, in line samples:
/// short towards the analogue modem, since a reconstruction filter that
/// rings on for longer than an equaliser reaches is not one any codec has,
/// and long the other way, where the upstream's band runs close to 4 kHz.
const DOWN_HALF_TAPS: usize = 40;
const UP_HALF_TAPS: usize = 120;

/// A route. The analogue side runs at `fs`, a whole multiple of 8 kHz.
#[derive(Debug, Clone)]
pub struct Network {
    law: Law,
    ratio: usize,
    down_filter: Vec<f64>,
    down_line: VecDeque<f64>,
    down_delay: VecDeque<f64>,
    up_filter: Vec<f64>,
    up_line: VecDeque<f64>,
    up_delay: VecDeque<f64>,
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
}

impl Network {
    pub fn new(law: Law, fs: f64) -> Self {
        let ratio = (fs / 8000.0).round().max(1.0) as usize;
        Self {
            law,
            ratio,
            down_filter: lowpass(3800.0, fs, ratio as f64, DOWN_HALF_TAPS),
            down_line: VecDeque::from(vec![0.0; 2 * DOWN_HALF_TAPS + 1]),
            down_delay: VecDeque::new(),
            up_filter: lowpass(3700.0, fs, 1.0, UP_HALF_TAPS),
            up_line: VecDeque::from(vec![0.0; 2 * UP_HALF_TAPS + 1]),
            up_delay: VecDeque::new(),
            noise: 0.0,
            seed: 0x2545_f491_4f6c_dd1d,
            robbed: None,
            octets: 0,
            pad: 1.0,
            quantised: true,
            up_gain: 0.25,
        }
    }

    /// Each way's delay, in seconds of line.
    pub fn with_delay(mut self, seconds: f64, fs: f64) -> Self {
        let samples = (seconds * fs).round() as usize;
        self.down_delay = VecDeque::from(vec![0.0; samples]);
        self.up_delay = VecDeque::from(vec![0.0; samples]);
        self
    }

    /// Noise on the loop, as a level: one is full scale.
    pub fn with_noise(mut self, level: f64) -> Self {
        self.noise = level;
        self
    }

    /// A robbed bit on every sixth downstream octet, starting at `phase`.
    pub fn with_robbed_bit(mut self, phase: usize) -> Self {
        self.robbed = Some(phase % 6);
        self
    }

    /// The analogue modem's level at the codec, against its own.
    pub fn with_upstream_gain(mut self, gain: f64) -> Self {
        self.up_gain = gain;
        self
    }

    /// An upstream carried as it is, with no codec's quantising: for
    /// finding out what the quantising costs.
    pub fn unquantised(mut self) -> Self {
        self.quantised = false;
        self
    }

    /// A digital pad of `db` on the downstream.
    pub fn with_pad(mut self, db: f64) -> Self {
        self.pad = 10f64.powf(-db / 20.0);
        self
    }

    fn gaussian(&mut self) -> f64 {
        // Two uniforms make a triangle, four make something close enough to a
        // bell for a test.
        let mut sum = 0.0;
        for _ in 0..4 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            sum += (self.seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
        }
        sum * 3f64.sqrt()
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
        if self.pad == 1.0 {
            return level;
        }
        let (u, negative) = ucode::nearest(self.law, (level * self.pad * 32768.0).round() as i32);
        ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 }
    }

    /// One downstream level in, and the line samples the analogue modem hears
    /// for it out.
    pub fn down(&mut self, level: f64) -> Vec<f64> {
        let carried = self.carry(level);
        let mut out = Vec::with_capacity(self.ratio);
        for n in 0..self.ratio {
            self.down_line.pop_front();
            self.down_line.push_back(if n == 0 { carried } else { 0.0 });
            let filtered: f64 = self.down_filter.iter().zip(&self.down_line).map(|(h, x)| h * x).sum();
            let noisy = filtered + self.noise * self.gaussian();
            self.down_delay.push_back(noisy);
            out.push(self.down_delay.pop_front().unwrap_or(0.0));
        }
        out
    }

    /// The analogue modem's line samples for one octet in, and the level the
    /// digital modem gets out.
    pub fn up(&mut self, samples: &[f64]) -> f64 {
        let mut level = 0.0;
        for (n, &x) in samples.iter().enumerate() {
            let noisy = self.up_gain * x + self.noise * self.gaussian();
            self.up_delay.push_back(noisy);
            let delayed = self.up_delay.pop_front().unwrap_or(0.0);
            self.up_line.pop_front();
            self.up_line.push_back(delayed);
            if n == 0 {
                level = self.up_filter.iter().zip(&self.up_line).map(|(h, x)| h * x).sum();
            }
        }
        if !self.quantised {
            return level;
        }
        let (u, negative) = ucode::nearest(self.law, (level * 32768.0).round() as i32);
        ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 }
    }
}

/// A windowed-sinc low-pass at `cutoff`, with a gain of `gain` at DC.
fn lowpass(cutoff: f64, fs: f64, gain: f64, half: usize) -> Vec<f64> {
    let n = 2 * half + 1;
    let mut taps: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 - half as f64;
            let x = 2.0 * cutoff / fs * t;
            let sinc = if t == 0.0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            let window = 0.42 + 0.5 * (std::f64::consts::PI * t / half as f64).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * t / half as f64).cos();
            sinc * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap *= gain / sum;
    }
    taps
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
        for _ in 0..200 {
            let heard = net.down(level);
            up = net.up(&heard);
        }
        assert!((up - level).abs() < 0.01 * level, "{up} against {level}");
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
