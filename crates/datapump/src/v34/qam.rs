//! The line signal of phases 3 and 4, and of data after them: points of the
//! superconstellation, at one of Table 1's symbol rates, about one of Table 2's
//! carriers, shaped and pre-emphasised (clause 5).
//!
//! What the recommendation fixes is the rate, the carrier and the spectrum's
//! slope. The pulse is left to the modem, but not by much: Figures 1 and 2
//! hold the spectrum to a template from 0.45 of a symbol rate below the
//! carrier to 0.45 above it, and a raised-cosine pulse is flat to exactly that
//! width when its roll-off is 10%. So 10% is what is used, which also keeps
//! 3429 symbols a second about 1959 Hz inside 73 to 3845 Hz.

use std::collections::VecDeque;

use dsp::{Complex, Fir, rrc_at};

use super::info::SymbolRate;
use super::probe;

/// Roll-off of the root-raised-cosine pulse.
pub const ROLLOFF: f64 = 0.1;

/// Symbols either side of the centre that the pulse is carried for. A 10%
/// pulse dies away slowly, and cutting it short spreads the spectrum.
const SPAN: usize = 20;

/// Taps in the pre-emphasis filter: at 16 kHz, 250 Hz of resolution, which is
/// enough for a slope and its knees to land within Figure 1's decibel.
const EMPHASIS_TAPS: usize = 63;

/// One direction of a call: the symbol rate and which of the two carriers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub rate: SymbolRate,
    pub high_carrier: bool,
}

impl Band {
    pub fn new(rate: SymbolRate, high_carrier: bool) -> Self {
        Self { rate, high_carrier }
    }

    /// Symbols a second.
    pub fn baud(self) -> f64 {
        probe::symbols_per_second(self.rate)
    }

    /// The carrier, in hertz.
    pub fn carrier(self) -> f64 {
        let (low, high) = probe::carriers(self.rate);
        if self.high_carrier { high } else { low }
    }

    /// Samples a symbol at `fs`, as a fraction in lowest terms.
    ///
    /// Every symbol rate is 2400 times a/c, so at a whole-numbered sample rate
    /// the ratio is exact: 14/3 samples a symbol at 3429 and 16 kHz, 5 at
    /// 3200, 20/3 at 2400.
    pub fn samples_per_symbol(self, fs: f64) -> (u64, u64) {
        let (a, c) = ratio(self.rate);
        let fs = fs.round() as u64;
        let (p, q) = (fs * c, 2400 * a);
        let g = gcd(p, q);
        (p / g, q / g)
    }
}

/// Table 1's a and c.
fn ratio(rate: SymbolRate) -> (u64, u64) {
    match rate {
        SymbolRate::S2400 => (1, 1),
        SymbolRate::S2743 => (8, 7),
        SymbolRate::S2800 => (7, 6),
        SymbolRate::S3000 => (5, 4),
        SymbolRate::S3200 => (4, 3),
        SymbolRate::S3429 => (10, 7),
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The transmit spectrum's shape for a pre-emphasis index, in decibels, at a
/// frequency given as a fraction of the symbol rate (Figures 1 and 2, Tables 3
/// and 4).
///
/// Indices 0 to 5 are a straight slope, rising alpha decibels from zero
/// frequency to the symbol rate. Indices 6 to 10 are flat to 0.4 of the symbol
/// rate and then rise, by beta at 0.8 and beta plus gamma at 1.2; the figure
/// leaves the stretch between 0.4 and 0.8 to the modem, and it is joined with
/// a straight line.
pub fn emphasis_db(index: u8, normalized: f64) -> f64 {
    let f = normalized.max(0.0);
    match index {
        0..=5 => 2.0 * f64::from(index) * f,
        6..=10 => {
            let beta = 0.5 * f64::from(index - 5);
            let gamma = 2.0 * beta;
            if f <= 0.4 {
                0.0
            } else if f <= 0.8 {
                beta * (f - 0.4) / 0.4
            } else {
                beta + gamma * (f.min(1.2) - 0.8) / 0.4
            }
        }
        _ => 0.0,
    }
}

/// A linear-phase filter with the spectrum of a pre-emphasis index, scaled so
/// the signal's power through it is unchanged; None for index 0, which is
/// flat.
pub fn emphasis_filter(index: u8, band: Band, fs: f64) -> Option<Vec<f64>> {
    if index == 0 || index > 10 {
        return None;
    }
    let s = band.baud();
    let n = EMPHASIS_TAPS;
    let mid = (n / 2) as f64;
    // Frequency sampling on a fine grid, windowed.
    let grid = 1024;
    let mut taps = vec![0.0; n];
    for k in 0..grid {
        let f = (k as f64 + 0.5) / grid as f64 * fs / 2.0;
        let gain = 10f64.powf(emphasis_db(index, f / s) / 20.0);
        for (i, tap) in taps.iter_mut().enumerate() {
            *tap += gain * (std::f64::consts::TAU * f * (i as f64 - mid) / fs).cos();
        }
    }
    for (i, tap) in taps.iter_mut().enumerate() {
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
        *tap *= window / grid as f64;
    }
    // The power a flat signal filling the band keeps, and that taken back out.
    let carrier = band.carrier();
    let steps = 64;
    let power = (0..steps)
        .map(|k| {
            let f = carrier - s / 2.0 + s * (k as f64 + 0.5) / steps as f64;
            response(&taps, f, fs).powi(2)
        })
        .sum::<f64>()
        / steps as f64;
    let scale = 1.0 / power.sqrt();
    for tap in &mut taps {
        *tap *= scale;
    }
    Some(taps)
}

/// The magnitude of a filter's response at `f` hertz.
pub fn response(taps: &[f64], f: f64, fs: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &tap) in taps.iter().enumerate() {
        let angle = std::f64::consts::TAU * f * i as f64 / fs;
        re += tap * angle.cos();
        im -= tap * angle.sin();
    }
    (re * re + im * im).sqrt()
}

/// Symbols in, line samples out.
///
/// Symbols are asked for as they are needed, one at a time, so whatever
/// decides them -- a start-up sequence, a data encoder -- counts them exactly.
/// A symbol given at unit mean power leaves at the level every transmitter
/// here uses, a root-mean-square of 0.707, less any power reduction.
#[derive(Debug, Clone)]
pub struct Transmitter {
    band: Band,
    /// Samples a symbol, as p/q.
    p: u64,
    q: u64,
    /// Where the next sample falls past the centre symbol, in 1/p of a symbol.
    frac: u64,
    /// The pulse at each of the p places a sample can fall, 2 SPAN + 1 taps
    /// each.
    table: Vec<f64>,
    /// The symbols the pulse reaches, oldest first, the centre one in the
    /// middle.
    history: VecDeque<Complex>,
    /// Carrier phase, as a fraction of a turn, and its step a sample.
    phase: f64,
    step: f64,
    emphasis: Option<Fir>,
    gain: f64,
    symbols: u64,
}

impl Transmitter {
    /// A transmitter for `band` at `fs`, with pre-emphasis `index` and its
    /// power reduced by `reduction` decibels.
    pub fn new(band: Band, pre_emphasis: u8, reduction: u8, fs: f64) -> Self {
        let (p, q) = band.samples_per_symbol(fs);
        let width = 2 * SPAN + 1;
        let mut table = vec![0.0; p as usize * width];
        let edge = SPAN as f64 + 1.0;
        for frac in 0..p as usize {
            for i in 0..width {
                let t = frac as f64 / p as f64 + SPAN as f64 - i as f64;
                // A Hann taper to zero past the last symbol carried, so the
                // cut does not ring.
                let taper = 0.5 + 0.5 * (std::f64::consts::PI * t / edge).cos();
                table[frac * width + i] = rrc_at(t, ROLLOFF) * taper;
            }
        }
        // Unit energy: averaged over where a sample can fall, the squares of
        // the taps it uses come to one symbol's worth.
        let energy = table.iter().map(|t| t * t).sum::<f64>() / p as f64;
        for tap in &mut table {
            *tap /= energy.sqrt();
        }
        Self {
            band,
            p,
            q,
            frac: 0,
            table,
            history: std::iter::repeat_n(Complex::ZERO, width).collect(),
            phase: 0.0,
            step: band.carrier() / fs,
            emphasis: emphasis_filter(pre_emphasis, band, fs).map(Fir::new),
            gain: 10f64.powf(-f64::from(reduction) / 20.0),
            symbols: 0,
        }
    }

    pub fn band(&self) -> Band {
        self.band
    }

    /// Symbols asked for so far.
    pub fn symbols(&self) -> u64 {
        self.symbols
    }

    /// Symbols asked for that have not reached the line yet: the pulse
    /// reaches this far ahead of the sample going out.
    pub fn lookahead() -> usize {
        SPAN
    }

    /// The next sample, asking `next` for symbols as the pulse needs them.
    pub fn next_sample(&mut self, mut next: impl FnMut() -> Complex) -> f64 {
        let width = 2 * SPAN + 1;
        let row = &self.table[self.frac as usize * width..(self.frac as usize + 1) * width];
        let mut baseband = Complex::ZERO;
        for (tap, symbol) in row.iter().zip(&self.history) {
            baseband += *symbol * *tap;
        }
        let angle = std::f64::consts::TAU * self.phase;
        let mut sample = baseband.re * angle.cos() - baseband.im * angle.sin();
        self.phase += self.step;
        self.phase -= self.phase.floor();
        self.frac += self.q;
        while self.frac >= self.p {
            self.frac -= self.p;
            self.history.pop_front();
            self.history.push_back(next());
            self.symbols += 1;
        }
        if let Some(filter) = self.emphasis.as_mut() {
            sample = filter.process(sample);
        }
        sample * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::signals;

    const FS: f64 = 16_000.0;

    fn tone(samples: &[f64], f: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in samples.iter().enumerate() {
            let angle = std::f64::consts::TAU * f * i as f64 / FS;
            re += x * angle.cos();
            im += x * angle.sin();
        }
        2.0 * (re * re + im * im).sqrt() / samples.len() as f64
    }

    #[test]
    fn every_symbol_rate_is_a_whole_ratio_of_16_khz() {
        let expect = [
            (SymbolRate::S2400, (20, 3)),
            (SymbolRate::S2743, (35, 6)),
            (SymbolRate::S2800, (40, 7)),
            (SymbolRate::S3000, (16, 3)),
            (SymbolRate::S3200, (5, 1)),
            (SymbolRate::S3429, (14, 3)),
        ];
        for (rate, ratio) in expect {
            let band = Band::new(rate, false);
            assert_eq!(band.samples_per_symbol(FS), ratio, "{rate:?}");
            assert!((FS * ratio.1 as f64 / ratio.0 as f64 - band.baud()).abs() < 1e-9);
        }
    }

    #[test]
    fn random_points_leave_at_the_nominal_level_and_inside_the_band() {
        let band = Band::new(SymbolRate::S3429, true);
        let mut tx = Transmitter::new(band, 0, 0, FS);
        let mut seed = 99u32;
        let mut out = Vec::new();
        for _ in 0..(2.0 * FS) as usize {
            out.push(tx.next_sample(|| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let x = if seed & 1 == 1 { 1.0 } else { -1.0 };
                let y = if seed & 2 == 2 { 1.0 } else { -1.0 };
                Complex::new(x, y).scale(std::f64::consts::FRAC_1_SQRT_2)
            }));
        }
        let settled = &out[4000..];
        let rms = (settled.iter().map(|x| x * x).sum::<f64>() / settled.len() as f64).sqrt();
        assert!((rms - 0.707).abs() < 0.02, "rms {rms}");
        // Nothing much above the band's top edge at 3845 Hz: look at 4100 to
        // 7900 Hz in 100 Hz steps against the carrier's neighbourhood.
        let inside = tone(settled, 1959.0).max(tone(settled, 1500.0)).max(1e-9);
        let outside = (41..79).map(|k| tone(settled, 100.0 * k as f64)).fold(0.0, f64::max);
        assert!(20.0 * (outside / inside).log10() < -35.0, "{} dB", 20.0 * (outside / inside).log10());
    }

    #[test]
    fn s_is_the_carrier_and_the_two_band_edges() {
        let band = Band::new(SymbolRate::S3429, false);
        let mut tx = Transmitter::new(band, 0, 0, FS);
        let mut n = 0;
        let out: Vec<f64> = (0..16_000)
            .map(|_| {
                tx.next_sample(|| {
                    let (x, y) = signals::s(n);
                    n += 1;
                    Complex::new(f64::from(x), f64::from(y)).scale(std::f64::consts::FRAC_1_SQRT_2)
                })
            })
            .collect();
        let settled = &out[2000..];
        let s = band.baud();
        let carrier = tone(settled, band.carrier());
        let lower = tone(settled, band.carrier() - s / 2.0);
        let upper = tone(settled, band.carrier() + s / 2.0);
        let elsewhere = tone(settled, 1000.0).max(tone(settled, 3000.0));
        assert!(carrier > 0.3, "carrier {carrier}");
        // A root-raised cosine is 3 dB down at half the symbol rate.
        for edge in [lower, upper] {
            assert!((20.0 * (edge / carrier).log10() + 3.0).abs() < 0.5, "{edge} against {carrier}");
        }
        assert!(elsewhere < carrier * 0.01, "{elsewhere}");
    }

    #[test]
    fn pre_emphasis_follows_its_template_within_a_decibel() {
        for rate in SymbolRate::ALL {
            for high in [false, true] {
                let band = Band::new(rate, high);
                let s = band.baud();
                let centre = band.carrier() / s;
                for index in 1..=10 {
                    let taps = emphasis_filter(index, band, FS).unwrap();
                    // The template is a shape; the level is the power's to set.
                    let points: Vec<(f64, f64)> = (0..=18)
                        .map(|k| {
                            let f = centre - 0.45 + 0.05 * k as f64;
                            let got = 20.0 * response(&taps, f * s, FS).log10();
                            (got, emphasis_db(index, f))
                        })
                        .collect();
                    let offset = points.iter().map(|(g, w)| g - w).sum::<f64>() / points.len() as f64;
                    for (k, (got, want)) in points.iter().enumerate() {
                        assert!(
                            (got - want - offset).abs() < 1.0,
                            "{rate:?} high {high} index {index} at {:.2}: {got:.2} against {want:.2}",
                            centre - 0.45 + 0.05 * k as f64
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_power_reduction_is_that_many_decibels() {
        let band = Band::new(SymbolRate::S3000, true);
        let level = |reduction| {
            let mut tx = Transmitter::new(band, 3, reduction, FS);
            let mut k = 0u32;
            let out: Vec<f64> = (0..32_000)
                .map(|_| {
                    tx.next_sample(|| {
                        k = k.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        Complex::from_polar(1.0, f64::from(k >> 16) * 0.001)
                    })
                })
                .collect();
            (out[4000..].iter().map(|x| x * x).sum::<f64>() / 28_000.0).sqrt()
        };
        let (full, less) = (level(0), level(6));
        assert!((20.0 * (less / full).log10() + 6.0).abs() < 0.05);
        assert!((full - 0.707).abs() < 0.05, "pre-emphasis keeps the power: {full}");
    }
}
