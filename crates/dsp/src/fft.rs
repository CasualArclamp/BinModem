//! Radix-2 FFT and a windowed spectrum analyser.
//!
//! Hand-rolled rather than pulled from a crate because the modem needs this
//! anyway for V.34 line probing (Recommendation V.34 clause 10.1.3 measures
//! channel response from a comb of tones), and the transform is small.

use std::f64::consts::TAU;

/// An in-place radix-2 Cooley-Tukey FFT of a fixed power-of-two size.
///
/// Twiddle factors and the bit-reversal permutation are precomputed, so
/// transforming allocates nothing.
#[derive(Debug, Clone)]
pub struct Fft {
    n: usize,
    /// Twiddles for each stage, flattened: cos and sin of -2*pi*k/n.
    twiddle_re: Vec<f64>,
    twiddle_im: Vec<f64>,
    reversal: Vec<u32>,
}

impl Fft {
    /// `n` must be a power of two of at least 2.
    pub fn new(n: usize) -> Self {
        assert!(n >= 2 && n.is_power_of_two(), "FFT size must be a power of two");
        let half = n / 2;
        let mut twiddle_re = Vec::with_capacity(half);
        let mut twiddle_im = Vec::with_capacity(half);
        for k in 0..half {
            let angle = -TAU * k as f64 / n as f64;
            twiddle_re.push(angle.cos());
            twiddle_im.push(angle.sin());
        }
        let bits = n.trailing_zeros();
        let reversal = (0..n)
            .map(|i| (i as u32).reverse_bits() >> (32 - bits))
            .collect();
        Self { n, twiddle_re, twiddle_im, reversal }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Transform in place. `re` and `im` must both be `len()` long.
    pub fn process(&self, re: &mut [f64], im: &mut [f64]) {
        assert_eq!(re.len(), self.n);
        assert_eq!(im.len(), self.n);

        // Reorder into bit-reversed index order.
        for i in 0..self.n {
            let j = self.reversal[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }

        // Butterflies, doubling the block size each stage.
        let mut size = 2;
        while size <= self.n {
            let half = size / 2;
            let stride = self.n / size;
            for start in (0..self.n).step_by(size) {
                for k in 0..half {
                    let wr = self.twiddle_re[k * stride];
                    let wi = self.twiddle_im[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size <<= 1;
        }
    }
}

/// Accumulates line samples and produces a windowed magnitude spectrum.
///
/// Holds its own scratch space so a spectrum can be taken without allocating,
/// which matters when this is driven from an audio callback.
#[derive(Debug, Clone)]
pub struct Spectrum {
    fft: Fft,
    window: Vec<f64>,
    /// Circular buffer of the most recent samples.
    history: Vec<f64>,
    write: usize,
    filled: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    /// Coherent gain of the window, for amplitude correction.
    window_gain: f64,
    sample_rate: f64,
}

impl Spectrum {
    pub fn new(size: usize, sample_rate: f64) -> Self {
        // Hann window: good sidelobe suppression, and the 1070/1270 Hz pair is
        // only 200 Hz apart so leakage matters more than resolution here.
        let window: Vec<f64> = (0..size)
            .map(|i| 0.5 - 0.5 * (TAU * i as f64 / size as f64).cos())
            .collect();
        let window_gain = window.iter().sum::<f64>() / size as f64;
        Self {
            fft: Fft::new(size),
            window,
            history: vec![0.0; size],
            write: 0,
            filled: 0,
            re: vec![0.0; size],
            im: vec![0.0; size],
            window_gain,
            sample_rate,
        }
    }

    pub fn size(&self) -> usize {
        self.history.len()
    }

    /// Frequency of bin `k`.
    pub fn bin_frequency(&self, k: usize) -> f64 {
        k as f64 * self.sample_rate / self.size() as f64
    }

    /// True once enough samples have arrived for a meaningful transform.
    pub fn ready(&self) -> bool {
        self.filled >= self.history.len()
    }

    /// Push one sample.
    #[inline]
    pub fn push(&mut self, x: f64) {
        self.history[self.write] = x;
        self.write = (self.write + 1) % self.history.len();
        self.filled = (self.filled + 1).min(self.history.len());
    }

    /// Compute the magnitude spectrum in dBFS into `out`, which receives
    /// `size()/2` bins covering DC to Nyquist.
    pub fn magnitudes_db(&mut self, out: &mut [f64]) {
        let n = self.history.len();
        assert_eq!(out.len(), n / 2);

        // Unwrap the circular buffer oldest-first and apply the window.
        for i in 0..n {
            let x = self.history[(self.write + i) % n];
            self.re[i] = x * self.window[i];
            self.im[i] = 0.0;
        }
        self.fft.process(&mut self.re, &mut self.im);

        // Single-sided: double every bin except DC to conserve energy.
        let scale = 1.0 / (n as f64 * self.window_gain);
        for (k, slot) in out.iter_mut().enumerate() {
            let mag = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * scale;
            let mag = if k == 0 { mag } else { mag * 2.0 };
            *slot = 20.0 * (mag + 1e-12).log10();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Index of the largest bin.
    fn peak_bin(bins: &[f64]) -> usize {
        bins.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0
    }

    #[test]
    fn dc_transforms_to_bin_zero() {
        let fft = Fft::new(8);
        let mut re = vec![1.0; 8];
        let mut im = vec![0.0; 8];
        fft.process(&mut re, &mut im);
        assert!((re[0] - 8.0).abs() < 1e-12, "DC bin {}", re[0]);
        for k in 1..8 {
            assert!(re[k].abs() < 1e-12 && im[k].abs() < 1e-12, "bin {k} not empty");
        }
    }

    #[test]
    fn a_pure_bin_frequency_lands_in_one_bin() {
        let n = 64;
        let fft = Fft::new(n);
        let k0 = 5;
        let mut re: Vec<f64> = (0..n).map(|i| (TAU * k0 as f64 * i as f64 / n as f64).cos()).collect();
        let mut im = vec![0.0; n];
        fft.process(&mut re, &mut im);
        let mags: Vec<f64> = (0..n / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .collect();
        assert_eq!(peak_bin(&mags), k0);
        assert!((mags[k0] - (n as f64 / 2.0)).abs() < 1e-9);
    }

    #[test]
    fn parseval_energy_is_conserved() {
        let n = 128;
        let fft = Fft::new(n);
        let mut re: Vec<f64> = (0..n).map(|i| ((i * 37) % 19) as f64 - 9.0).collect();
        let mut im = vec![0.0; n];
        let time_energy: f64 = re.iter().map(|x| x * x).sum();
        fft.process(&mut re, &mut im);
        let freq_energy: f64 =
            re.iter().zip(&im).map(|(r, i)| r * r + i * i).sum::<f64>() / n as f64;
        assert!(
            (time_energy - freq_energy).abs() / time_energy < 1e-12,
            "time {time_energy} vs freq {freq_energy}"
        );
    }

    #[test]
    fn spectrum_finds_the_bell103_answer_tones() {
        let fs = 16000.0;
        let mut sp = Spectrum::new(2048, fs);
        // A mark tone at 2225 Hz.
        for i in 0..4096 {
            sp.push((TAU * 2225.0 * i as f64 / fs).sin());
        }
        assert!(sp.ready());
        let mut bins = vec![0.0; sp.size() / 2];
        sp.magnitudes_db(&mut bins);
        let peak = peak_bin(&bins);
        let f = sp.bin_frequency(peak);
        assert!((f - 2225.0).abs() < sp.sample_rate / sp.size() as f64,
                "peak at {f} Hz, expected 2225");
    }

    #[test]
    fn full_scale_sine_reads_near_zero_dbfs() {
        // The window gain correction should make a unit-amplitude sine read
        // about 0 dBFS rather than being attenuated by the Hann window.
        let fs = 8000.0;
        let mut sp = Spectrum::new(1024, fs);
        for i in 0..2048 {
            sp.push((TAU * 1000.0 * i as f64 / fs).sin());
        }
        let mut bins = vec![0.0; sp.size() / 2];
        sp.magnitudes_db(&mut bins);
        let peak = bins[peak_bin(&bins)];
        assert!(peak > -1.0 && peak < 1.0, "peak read {peak} dBFS");
    }

    #[test]
    fn silence_reads_far_below_full_scale() {
        let mut sp = Spectrum::new(256, 8000.0);
        for _ in 0..256 {
            sp.push(0.0);
        }
        let mut bins = vec![0.0; 128];
        sp.magnitudes_db(&mut bins);
        assert!(bins.iter().all(|&d| d < -200.0), "silence was not quiet");
    }
}
