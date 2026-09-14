//! The line probing signals of 10.1.2.4, and what a receiver makes of them.
//!
//! L1 and L2 are the same signal at two levels: twenty-one cosines, 150 Hz
//! apart from 150 to 3750 Hz, with the tones at 900, 1200, 1800 and 2400 left
//! out and the starting phases of Table 17. It repeats 150 times a second, so
//! a receiver that looks at a whole number of repetitions sees every tone land
//! in a bin of its own, and can read the channel's gain and noise at each of
//! them. That is what decides the symbol rate, the carrier, the pre-emphasis
//! and the projected data rate that INFO1c and INFO1a carry.
//!
//! What a modem makes of the probing is its own business -- the
//! recommendation says what the results are, not how to reach them -- and so
//! is most of what is here. The signal itself is not.

use super::info::{Info0, Probed, SymbolRate};

/// Table 17, read off the PDF: each tone's frequency and its starting phase.
///
/// The extracted text of this table is wrong in six rows -- 450 Hz lost its
/// phase and every row after it took its neighbour's -- which is not something
/// a loopback would ever show, since both ends of one would agree.
pub const TONES: [(f64, f64); 21] = [
    (150.0, 0.0),
    (300.0, 180.0),
    (450.0, 0.0),
    (600.0, 0.0),
    (750.0, 0.0),
    (1050.0, 0.0),
    (1350.0, 0.0),
    (1500.0, 0.0),
    (1650.0, 180.0),
    (1950.0, 0.0),
    (2100.0, 0.0),
    (2250.0, 180.0),
    (2550.0, 0.0),
    (2700.0, 180.0),
    (2850.0, 0.0),
    (3000.0, 180.0),
    (3150.0, 180.0),
    (3300.0, 180.0),
    (3450.0, 180.0),
    (3600.0, 0.0),
    (3750.0, 0.0),
];

/// "L1 is transmitted for 160 ms (24 repetitions) at 6 dB above the nominal
/// power level."
pub const L1_SECONDS: f64 = 0.160;

/// "L2 is the same as L1 but is transmitted for no longer than 550 ms plus a
/// round trip delay at the nominal power level."
pub const L2_SECONDS: f64 = 0.550;

/// The repetition rate, "150 ± 0.01% Hz".
pub const REPETITION: f64 = 150.0;

/// Each tone's amplitude at the nominal level.
///
/// Every transmitter here leaves at a root-mean-square of 0.707. Twenty-one
/// equal cosines have 21/2 times the power of one, so each is 0.707 over the
/// square root of 10.5.
fn nominal_amplitude() -> f64 {
    std::f64::consts::FRAC_1_SQRT_2 / (TONES.len() as f64 / 2.0).sqrt()
}

/// L1 and L2, as line samples.
#[derive(Debug, Clone)]
pub struct Generator {
    fs: f64,
    n: u64,
}

impl Generator {
    pub fn new(fs: f64) -> Self {
        Self { fs, n: 0 }
    }

    /// The next sample, of L1 if `loud` and of L2 if not.
    pub fn next_sample(&mut self, loud: bool) -> f64 {
        let t = self.n as f64 / self.fs;
        self.n += 1;
        let gain = if loud { 2.0 } else { 1.0 } * nominal_amplitude();
        TONES
            .iter()
            .map(|&(f, phase)| (std::f64::consts::TAU * f * t + phase.to_radians()).cos())
            .sum::<f64>()
            * gain
    }
}

/// What the line did to one probing tone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    pub frequency: f64,
    /// Received level against the level it was sent at, in dB.
    pub gain_db: f64,
    /// Signal against the noise around it, in dB.
    pub snr_db: f64,
}

/// What the probing found across the band.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub tones: Vec<Tone>,
    /// The 1050 Hz tone as received, less 1050 Hz, when it could be measured.
    pub frequency_offset: Option<f64>,
}

/// Samples in the shortest stretch that holds a whole number of repetitions
/// and a whole number of samples: 320 at 16 kHz, which is three repetitions.
fn window(fs: f64) -> usize {
    (1..=64)
        .map(|m| fs * f64::from(m) / REPETITION)
        .find(|n| (n - n.round()).abs() < 1e-6)
        .map_or((fs / REPETITION).round() as usize, |n| n.round() as usize)
}

/// Reads L2 off the line, a window at a time.
#[derive(Debug, Clone)]
pub struct Analyzer {
    fs: f64,
    window: usize,
    /// The window being filled.
    buffer: Vec<f64>,
    /// Each finished window's reading of every tone, as a complex amplitude.
    readings: Vec<Vec<(f64, f64)>>,
}

impl Analyzer {
    pub fn new(fs: f64) -> Self {
        let window = window(fs);
        Self { fs, window, buffer: Vec::with_capacity(window), readings: Vec::new() }
    }

    /// Start again, for a new stretch of probing.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.readings.clear();
    }

    /// Windows read so far.
    pub fn windows(&self) -> usize {
        self.readings.len()
    }

    pub fn feed(&mut self, sample: f64) {
        self.buffer.push(sample);
        if self.buffer.len() < self.window {
            return;
        }
        let n = self.window as f64;
        let reading = TONES
            .iter()
            .map(|&(f, _)| {
                let (mut re, mut im) = (0.0, 0.0);
                for (i, &x) in self.buffer.iter().enumerate() {
                    let angle = std::f64::consts::TAU * f * i as f64 / self.fs;
                    re += x * angle.cos();
                    im -= x * angle.sin();
                }
                (2.0 * re / n, 2.0 * im / n)
            })
            .collect();
        self.readings.push(reading);
        self.buffer.clear();
    }

    /// What the windows read so far come to. None until there are enough of
    /// them to say anything about the noise.
    pub fn reading(&self) -> Option<Reading> {
        let windows = self.readings.len();
        if windows < 4 {
            return None;
        }
        // A frequency offset turns every tone by the same angle from one
        // window to the next. Measured on 1050 Hz, the tone the tables name,
        // and taken out of all of them before anything is averaged.
        let at_1050 = TONES.iter().position(|&(f, _)| f == 1050.0).expect("1050 Hz is a probing tone");
        let mut turn = (0.0, 0.0);
        for pair in self.readings.windows(2) {
            let (a, b) = (pair[0][at_1050], pair[1][at_1050]);
            turn.0 += b.0 * a.0 + b.1 * a.1;
            turn.1 += b.1 * a.0 - b.0 * a.1;
        }
        let per_window = turn.1.atan2(turn.0);
        let seconds = self.window as f64 / self.fs;
        let offset = per_window / (std::f64::consts::TAU * seconds);
        let sent = nominal_amplitude();
        let tones = TONES
            .iter()
            .enumerate()
            .map(|(t, &(frequency, _))| {
                let derotated: Vec<(f64, f64)> = self
                    .readings
                    .iter()
                    .enumerate()
                    .map(|(k, r)| {
                        let angle = -per_window * k as f64;
                        let (re, im) = r[t];
                        (re * angle.cos() - im * angle.sin(), re * angle.sin() + im * angle.cos())
                    })
                    .collect();
                let mean = derotated.iter().fold((0.0, 0.0), |m, x| (m.0 + x.0, m.1 + x.1));
                let mean = (mean.0 / windows as f64, mean.1 / windows as f64);
                let power = mean.0 * mean.0 + mean.1 * mean.1;
                let noise = derotated
                    .iter()
                    .map(|x| (x.0 - mean.0).powi(2) + (x.1 - mean.1).powi(2))
                    .sum::<f64>()
                    / (windows - 1) as f64;
                Tone {
                    frequency,
                    gain_db: 10.0 * (power / (sent * sent)).max(1e-12).log10(),
                    snr_db: 10.0 * (power / noise.max(power * 1e-6).max(1e-18)).log10(),
                }
            })
            .collect();
        // The field's own accuracy is a quarter of a hertz, and one window's
        // worth of turn is only unambiguous to half a repetition's width.
        let frequency_offset = (offset.abs() < 10.0).then_some(offset);
        Some(Reading { tones, frequency_offset })
    }
}

/// Table 2: the low and the high carrier at each symbol rate, in hertz.
pub fn carriers(rate: SymbolRate) -> (f64, f64) {
    let s = symbols_per_second(rate);
    let (low, high) = match rate {
        SymbolRate::S2400 => ((2.0, 3.0), (3.0, 4.0)),
        SymbolRate::S2743 | SymbolRate::S2800 | SymbolRate::S3000 => ((3.0, 5.0), (2.0, 3.0)),
        SymbolRate::S3200 => ((4.0, 7.0), (3.0, 5.0)),
        SymbolRate::S3429 => ((4.0, 7.0), (4.0, 7.0)),
    };
    (s * low.0 / low.1, s * high.0 / high.1)
}

/// Table 1: S = (a/c) x 2400.
pub fn symbols_per_second(rate: SymbolRate) -> f64 {
    let (a, c) = match rate {
        SymbolRate::S2400 => (1.0, 1.0),
        SymbolRate::S2743 => (8.0, 7.0),
        SymbolRate::S2800 => (7.0, 6.0),
        SymbolRate::S3000 => (5.0, 4.0),
        SymbolRate::S3200 => (4.0, 3.0),
        SymbolRate::S3429 => (10.0, 7.0),
    };
    2400.0 * a / c
}

/// The highest data rate Table 8 has a row for at each symbol rate, as a
/// multiple of 2400: 21 600, 26 400, 26 400, 28 800, 31 200 and 33 600.
pub fn ceiling(rate: SymbolRate) -> u8 {
    match rate {
        SymbolRate::S2400 => 9,
        SymbolRate::S2743 | SymbolRate::S2800 => 11,
        SymbolRate::S3000 => 12,
        SymbolRate::S3200 => 13,
        SymbolRate::S3429 => 14,
    }
}

/// The lowest: 2400 at 2400 symbols a second, 4800 at the rest (Table 8).
fn floor(rate: SymbolRate) -> u8 {
    if rate == SymbolRate::S2400 { 1 } else { 2 }
}

/// Decibels of signal to noise a bit of data costs above Shannon's limit.
///
/// Six: what 4D trellis-coded QAM with shaping gives up against the bound at
/// the error rates a modem runs at, less nothing for margin. On a line with
/// 36 dB of signal to noise across the band it projects 33 600 at 3429
/// symbols a second, which is about what real modems ask of a line for it.
const GAP_DB: f64 = 6.0;

/// Whether the far end's transmitter can use a symbol rate and carrier, by
/// its INFO0 (Table 14, bits 12 to 19).
fn transmits(far: &Info0, rate: SymbolRate, high: bool) -> bool {
    match rate {
        SymbolRate::S2400 => true,
        SymbolRate::S2743 => far.rate_2743,
        SymbolRate::S2800 => far.rate_2800,
        SymbolRate::S3000 => if high { far.high_carrier_3000 } else { far.low_carrier_3000 },
        SymbolRate::S3200 => if high { far.high_carrier_3200 } else { far.low_carrier_3200 },
        SymbolRate::S3429 => far.rate_3429 && far.transmit_3429,
    }
}

impl Reading {
    /// Signal to noise at a frequency, in linear terms, interpolated between
    /// the tones either side. The four tones L1 leaves out are bridged over.
    fn snr_at(&self, f: f64) -> f64 {
        let linear = |t: &Tone| 10f64.powf(t.snr_db / 10.0);
        let first = &self.tones[0];
        let last = &self.tones[self.tones.len() - 1];
        if f <= first.frequency {
            return linear(first);
        }
        if f >= last.frequency {
            return linear(last);
        }
        let above = self.tones.iter().position(|t| t.frequency >= f).unwrap_or(self.tones.len() - 1);
        let (lo, hi) = (&self.tones[above - 1], &self.tones[above]);
        let x = (f - lo.frequency) / (hi.frequency - lo.frequency);
        linear(lo) * (1.0 - x) + linear(hi) * x
    }

    fn gain_at(&self, f: f64) -> f64 {
        let above = self.tones.iter().position(|t| t.frequency >= f).unwrap_or(self.tones.len() - 1).max(1);
        let (lo, hi) = (&self.tones[above - 1], &self.tones[above]);
        let x = ((f - lo.frequency) / (hi.frequency - lo.frequency)).clamp(0.0, 1.0);
        lo.gain_db * (1.0 - x) + hi.gain_db * x
    }

    /// Bits a 2D symbol could carry across the band a symbol rate occupies
    /// about a carrier.
    fn bits(&self, rate: SymbolRate, carrier: f64) -> f64 {
        let s = symbols_per_second(rate);
        let gap = 10f64.powf(GAP_DB / 10.0);
        let steps = 64;
        (0..steps)
            .map(|i| {
                let f = carrier - s / 2.0 + s * (i as f64 + 0.5) / steps as f64;
                (1.0 + self.snr_at(f) / gap).log2()
            })
            .sum::<f64>()
            / steps as f64
    }

    /// The pre-emphasis index that best flattens the band (Tables 3 and 4).
    ///
    /// Indices 0 to 5 tilt the transmit spectrum up by 0, 2, ... 10 dB across
    /// a symbol rate's width (Figure 1). What the probing sees is the line's
    /// tilt the other way, so the index is the one nearest to cancelling it.
    /// The shelves of indices 6 to 10 are for a line that falls away at the
    /// top edge, and are not chosen here.
    fn pre_emphasis(&self, rate: SymbolRate, carrier: f64) -> u8 {
        let s = symbols_per_second(rate);
        let low = self.gain_at(carrier - 0.45 * s);
        let high = self.gain_at(carrier + 0.45 * s);
        let tilt = (low - high) / 0.9;
        (tilt / 2.0).round().clamp(0.0, 5.0) as u8
    }

    /// INFO1c's nine bits for one symbol rate: what the far end's transmitter
    /// should use to reach this receiver, and how fast it could go.
    ///
    /// `far` is the far end's INFO0, whose transmitter this is about, and
    /// `wide` whether both ends have the 1664-point constellation that rates
    /// above 12 times 2400 need.
    pub fn probed(&self, rate: SymbolRate, far: &Info0, wide: bool) -> Probed {
        let (low, high) = carriers(rate);
        let options = [(false, low), (true, high)];
        let best = options
            .iter()
            .filter(|(is_high, _)| transmits(far, rate, *is_high))
            .map(|&(is_high, carrier)| (is_high, carrier, self.bits(rate, carrier)))
            .max_by(|a, b| a.2.total_cmp(&b.2));
        let Some((high_carrier, carrier, bits)) = best else {
            return Probed::default();
        };
        let s = symbols_per_second(rate);
        let mut max = ((bits * s) / 2400.0).floor() as u8;
        max = max.min(ceiling(rate));
        if !wide {
            max = max.min(12);
        }
        if max < floor(rate) {
            return Probed::default();
        }
        Probed { high_carrier, pre_emphasis: self.pre_emphasis(rate, carrier), max_rate: max }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    #[test]
    fn l2_leaves_at_the_nominal_level_and_l1_six_decibels_over_it() {
        let mut g = Generator::new(FS);
        let period = window(FS);
        let l2: Vec<f64> = (0..period * 10).map(|_| g.next_sample(false)).collect();
        let rms = (l2.iter().map(|x| x * x).sum::<f64>() / l2.len() as f64).sqrt();
        assert!((rms - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-3, "L2 at {rms}");
        let l1: Vec<f64> = (0..period * 10).map(|_| g.next_sample(true)).collect();
        let loud = (l1.iter().map(|x| x * x).sum::<f64>() / l1.len() as f64).sqrt();
        assert!((20.0 * (loud / rms).log10() - 6.02).abs() < 0.05);
    }

    #[test]
    fn the_signal_repeats_150_times_a_second_and_leaves_out_four_tones() {
        assert_eq!(window(FS), 320, "three repetitions at 16 kHz");
        assert_eq!(window(48_000.0), 320, "one at 48 kHz");
        let mut g = Generator::new(FS);
        let first: Vec<f64> = (0..320).map(|_| g.next_sample(false)).collect();
        let second: Vec<f64> = (0..320).map(|_| g.next_sample(false)).collect();
        for (a, b) in first.iter().zip(&second) {
            assert!((a - b).abs() < 1e-9);
        }
        let frequencies: Vec<f64> = TONES.iter().map(|t| t.0).collect();
        for missing in [900.0, 1200.0, 1800.0, 2400.0] {
            assert!(!frequencies.contains(&missing));
        }
        assert_eq!(TONES.len(), 25 - 4);
    }

    #[test]
    fn a_clean_line_reads_flat_and_quiet() {
        let mut g = Generator::new(FS);
        let mut a = Analyzer::new(FS);
        for _ in 0..8000 {
            a.feed(g.next_sample(false));
        }
        let reading = a.reading().expect("no reading");
        for tone in &reading.tones {
            assert!(tone.gain_db.abs() < 0.1, "{tone:?}");
            assert!(tone.snr_db > 50.0, "{tone:?}");
        }
        assert!(reading.frequency_offset.unwrap().abs() < 0.05);
    }

    #[test]
    fn a_tilted_noisy_shifted_line_reads_as_what_it_is() {
        // 20 dB down, a straight tilt of 6 dB across the band, noise about
        // 30 dB under the signal, and every tone 1.5 Hz high.
        let mut a = Analyzer::new(FS);
        let mut seed = 7u32;
        let n = 8000;
        for i in 0..n {
            let t = i as f64 / FS;
            let signal: f64 = TONES
                .iter()
                .map(|&(f, phase)| {
                    let tilt = 10f64.powf(-6.0 * (f - 150.0) / 3600.0 / 20.0);
                    tilt * (std::f64::consts::TAU * (f + 1.5) * t + phase.to_radians()).cos()
                })
                .sum::<f64>()
                * nominal_amplitude()
                * 0.1;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 0.0045;
            a.feed(signal + noise);
        }
        let reading = a.reading().expect("no reading");
        let offset = reading.frequency_offset.expect("no offset");
        assert!((offset - 1.5).abs() < 0.25, "offset {offset}");
        let first = reading.tones[0];
        let last = reading.tones[20];
        assert!((first.gain_db + 20.0).abs() < 0.5, "{first:?}");
        assert!((last.gain_db + 26.0).abs() < 0.8, "{last:?}");
        assert!(reading.tones.iter().all(|t| t.snr_db > 20.0 && t.snr_db < 45.0), "{reading:?}");
    }

    #[test]
    fn the_carriers_are_table_2s() {
        let rounded = |r| {
            let (l, h) = carriers(r);
            (l.round() as u32, h.round() as u32)
        };
        assert_eq!(rounded(SymbolRate::S2400), (1600, 1800));
        assert_eq!(rounded(SymbolRate::S2743), (1646, 1829));
        assert_eq!(rounded(SymbolRate::S2800), (1680, 1867));
        assert_eq!(rounded(SymbolRate::S3000), (1800, 2000));
        assert_eq!(rounded(SymbolRate::S3200), (1829, 1920));
        assert_eq!(rounded(SymbolRate::S3429), (1959, 1959));
    }

    fn everything() -> Info0 {
        Info0 {
            rate_2743: true,
            rate_2800: true,
            rate_3429: true,
            low_carrier_3000: true,
            high_carrier_3000: true,
            low_carrier_3200: true,
            high_carrier_3200: true,
            transmit_3429: true,
            can_reduce_power: true,
            asymmetry: 5,
            constellation_1664: true,
            ..Info0::default()
        }
    }

    fn flat(snr_db: f64) -> Reading {
        Reading {
            tones: TONES.iter().map(|&(frequency, _)| Tone { frequency, gain_db: 0.0, snr_db }).collect(),
            frequency_offset: Some(0.0),
        }
    }

    #[test]
    fn a_clean_wide_line_projects_33600_and_a_noisy_one_much_less() {
        let clean = flat(40.0).probed(SymbolRate::S3429, &everything(), true);
        assert_eq!(clean.max_rate, 14);
        // Without the 1664-point constellation at both ends, twelve at most.
        assert_eq!(flat(40.0).probed(SymbolRate::S3429, &everything(), false).max_rate, 12);
        let noisy = flat(18.0).probed(SymbolRate::S3429, &everything(), true);
        assert!(noisy.max_rate > 0 && noisy.max_rate < 9, "{noisy:?}");
        // A far end that will not transmit 3429 gets nothing projected for it.
        let mut no_3429 = everything();
        no_3429.transmit_3429 = false;
        assert_eq!(flat(40.0).probed(SymbolRate::S3429, &no_3429, true), Probed::default());
    }

    #[test]
    fn a_line_that_falls_away_gets_pre_emphasis_to_match() {
        let mut tilted = flat(40.0);
        for t in &mut tilted.tones {
            // 8 dB lower at the top of the band than the bottom, per symbol
            // rate's width at 3200.
            t.gain_db = -8.0 * (t.frequency - 150.0) / 3200.0;
        }
        let probed = tilted.probed(SymbolRate::S3200, &everything(), true);
        assert_eq!(probed.pre_emphasis, 4);
        assert_eq!(flat(40.0).probed(SymbolRate::S3200, &everything(), true).pre_emphasis, 0);
    }
}
