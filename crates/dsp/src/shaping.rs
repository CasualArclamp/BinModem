//! Pulse shaping and symbol timing.
//!
//! V.22bis 2.4 asks for "the square root of a raised cosine shaping with 75%
//! roll-off". Splitting the raised cosine between transmitter and receiver puts
//! a matched filter at each end: the pair multiply to a full raised cosine,
//! which is free of intersymbol interference at the sampling instants, and the
//! receiver gets the best noise rejection available.

use std::f64::consts::PI;

/// Root-raised-cosine taps, `span` symbols long at `sps` samples per symbol.
///
/// Normalised to unit energy so the filter neither amplifies nor attenuates.
pub fn rrc_taps(sps: f64, rolloff: f64, span: usize) -> Vec<f64> {
    let len = (span as f64 * sps).round() as usize | 1; // odd, so there is a centre tap
    let mid = (len / 2) as f64;
    let mut taps = Vec::with_capacity(len);
    for i in 0..len {
        let t = (i as f64 - mid) / sps;
        taps.push(rrc_at(t, rolloff));
    }
    let energy: f64 = taps.iter().map(|x| x * x).sum::<f64>().sqrt();
    for t in &mut taps {
        *t /= energy;
    }
    taps
}

/// The root-raised-cosine impulse response at `t` symbol periods from centre.
///
/// Public because a transmitter working at a sample rate that is not a whole
/// multiple of the symbol rate has to evaluate the pulse at arbitrary offsets
/// rather than index a fixed tap table. 16 kHz against 600 baud is exactly that
/// case, at 26.67 samples per symbol.
pub fn rrc_at(t: f64, beta: f64) -> f64 {
    // Both closed-form singularities are removed by their limits.
    if t.abs() < 1e-9 {
        return 1.0 + beta * (4.0 / PI - 1.0);
    }
    if beta > 0.0 {
        let edge = 1.0 / (4.0 * beta);
        if (t.abs() - edge).abs() < 1e-9 {
            let a = (1.0 + 2.0 / PI) * (PI / (4.0 * beta)).sin();
            let b = (1.0 - 2.0 / PI) * (PI / (4.0 * beta)).cos();
            return beta / 2f64.sqrt() * (a + b);
        }
    }
    let num = (PI * t * (1.0 - beta)).sin()
        + 4.0 * beta * t * (PI * t * (1.0 + beta)).cos();
    let den = PI * t * (1.0 - (4.0 * beta * t).powi(2));
    num / den
}

/// A real finite impulse response filter with a sliding history.
#[derive(Debug, Clone)]
pub struct Fir {
    taps: Vec<f64>,
    history: Vec<f64>,
    pos: usize,
}

impl Fir {
    pub fn new(taps: Vec<f64>) -> Self {
        let n = taps.len();
        Self { taps, history: vec![0.0; n], pos: 0 }
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// Group delay in samples: half the filter length.
    pub fn delay(&self) -> usize {
        self.taps.len() / 2
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let n = self.history.len();
        self.history[self.pos] = x;
        self.pos = (self.pos + 1) % n;
        // Walk the history newest-first against the taps.
        let mut acc = 0.0;
        let mut idx = self.pos;
        for &tap in self.taps.iter().rev() {
            acc += tap * self.history[idx];
            idx = (idx + 1) % n;
        }
        acc
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.pos = 0;
    }
}

/// The same filter applied to both halves of a complex signal.
#[derive(Debug, Clone)]
pub struct ComplexFir {
    re: Fir,
    im: Fir,
}

impl ComplexFir {
    pub fn new(taps: Vec<f64>) -> Self {
        Self { re: Fir::new(taps.clone()), im: Fir::new(taps) }
    }

    #[inline]
    pub fn process(&mut self, x: (f64, f64)) -> (f64, f64) {
        (self.re.process(x.0), self.im.process(x.1))
    }

    pub fn delay(&self) -> usize {
        self.re.delay()
    }

    pub fn reset(&mut self) {
        self.re.reset();
        self.im.reset();
    }
}

/// Gardner symbol timing recovery.
///
/// Chosen over an early-late gate because its error estimate does not depend on
/// carrier phase, so timing can be recovered before the carrier loop has locked.
/// It needs two samples per symbol, taking one at the symbol instant and one
/// halfway between.
#[derive(Debug, Clone)]
pub struct Gardner {
    /// Samples per symbol, which need not be a whole number.
    sps: f64,
    /// Fractional position of the next sample to take.
    phase: f64,
    gain: f64,
    /// Previous symbol and the midpoint before it.
    previous: (f64, f64),
    midpoint: (f64, f64),
    /// Toggles between midpoint and symbol.
    at_symbol: bool,
    last_error: f64,
    /// Running mean symbol power, used to normalise the error.
    mean_power: f64,
}

impl Gardner {
    pub fn new(sps: f64, gain: f64) -> Self {
        Self {
            sps,
            phase: 0.0,
            gain,
            previous: (0.0, 0.0),
            midpoint: (0.0, 0.0),
            at_symbol: true,
            last_error: 0.0,
            mean_power: 1.0,
        }
    }

    /// Interval to the next sample, in samples.
    pub fn interval(&self) -> f64 {
        self.sps / 2.0 + self.phase
    }

    /// Offer a sample taken at the interval this returned last time.
    ///
    /// Yields a symbol on every second call, once at the symbol instant.
    pub fn feed(&mut self, sample: (f64, f64)) -> Option<(f64, f64)> {
        self.at_symbol = !self.at_symbol;
        if !self.at_symbol {
            self.midpoint = sample;
            return None;
        }

        // Gardner's error: the midpoint should sit where the two symbols cross,
        // so it correlates with the change between them when timing is off.
        let error = self.midpoint.0 * (sample.0 - self.previous.0)
            + self.midpoint.1 * (sample.1 - self.previous.1);

        // Normalise against a *running* mean power rather than this symbol's
        // own. Gardner's detector assumes a constant modulus; a sixteen-point
        // constellation has magnitudes spanning three to one, so dividing by
        // the instantaneous power turns an ordinary amplitude change into a
        // huge apparent timing error and the loop thrashes. Clamping keeps a
        // single outlier from throwing the sampling instant across a symbol.
        let power = sample.0 * sample.0 + sample.1 * sample.1;
        self.mean_power += 0.02 * (power - self.mean_power);
        self.last_error = (error / (self.mean_power + 1e-9)).clamp(-1.0, 1.0);
        self.phase = (-self.gain * self.last_error).clamp(-self.sps / 4.0, self.sps / 4.0);
        self.previous = sample;
        Some(sample)
    }

    pub fn error(&self) -> f64 {
        self.last_error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrc_taps_have_unit_energy() {
        let taps = rrc_taps(16.0, 0.75, 8);
        let energy: f64 = taps.iter().map(|x| x * x).sum();
        assert!((energy - 1.0).abs() < 1e-12, "energy {energy}");
    }

    #[test]
    fn rrc_is_symmetric_with_a_central_peak() {
        let taps = rrc_taps(16.0, 0.75, 8);
        let n = taps.len();
        assert_eq!(n % 2, 1, "an odd length gives a true centre tap");
        for i in 0..n / 2 {
            assert!(
                (taps[i] - taps[n - 1 - i]).abs() < 1e-12,
                "asymmetric at {i}"
            );
        }
        let peak = taps.iter().cloned().fold(f64::MIN, f64::max);
        assert!((taps[n / 2] - peak).abs() < 1e-12, "peak should be central");
    }

    #[test]
    fn the_singularities_are_finite() {
        // The closed form divides by zero at t=0 and t=1/(4*beta); both are
        // replaced by their limits.
        for beta in [0.25, 0.35, 0.5, 0.75, 1.0] {
            assert!(rrc_at(0.0, beta).is_finite(), "t=0, beta={beta}");
            let edge = 1.0 / (4.0 * beta);
            assert!(rrc_at(edge, beta).is_finite(), "t=edge, beta={beta}");
            assert!(rrc_at(-edge, beta).is_finite());
        }
    }

    /// Two root-raised-cosine filters in series make a raised cosine, which is
    /// zero at every symbol instant but the centre. That is the property the
    /// whole scheme rests on.
    #[test]
    fn a_matched_pair_has_no_intersymbol_interference() {
        let sps = 8usize;
        let taps = rrc_taps(sps as f64, 0.75, 10);
        // Convolve the filter with itself.
        let n = taps.len();
        let mut full = vec![0.0; 2 * n - 1];
        for (i, a) in taps.iter().enumerate() {
            for (j, b) in taps.iter().enumerate() {
                full[i + j] += a * b;
            }
        }
        let centre = n - 1;
        let peak = full[centre];
        assert!(peak > 0.0);
        for k in 1..=4 {
            let at = full[centre + k * sps].abs() / peak;
            assert!(at < 0.02, "symbol {k} away carries {at} of the peak");
        }
    }

    #[test]
    fn the_filter_passes_a_constant_through() {
        let mut f = Fir::new(vec![0.25; 4]);
        for _ in 0..8 {
            f.process(1.0);
        }
        assert!((f.process(1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_filter_reproduces_a_known_convolution() {
        let mut f = Fir::new(vec![1.0, 2.0, 3.0]);
        // Impulse in, taps out, newest tap first.
        assert_eq!(f.process(1.0), 1.0);
        assert_eq!(f.process(0.0), 2.0);
        assert_eq!(f.process(0.0), 3.0);
        assert_eq!(f.process(0.0), 0.0);
    }

    /// Gardner should pull the sampling instant onto the symbol centre from a
    /// deliberate offset.
    #[test]
    fn timing_recovery_converges() {
        let sps = 8.0;
        let symbols: Vec<f64> = (0..400)
            .map(|i| if (i * 7 + 3) % 5 < 2 { 1.0 } else { -1.0 })
            .collect();
        // A shaped waveform, sampled with an offset the loop has to remove.
        let taps = rrc_taps(sps, 0.75, 8);
        let mut shaped = Vec::new();
        let mut fir = Fir::new(taps);
        for &s in &symbols {
            shaped.push(fir.process(s));
            for _ in 1..sps as usize {
                shaped.push(fir.process(0.0));
            }
        }

        let mut g = Gardner::new(sps, 0.05);
        let mut position = 3.0f64; // deliberately off the symbol centre
        let mut errors = Vec::new();
        while (position as usize) < shaped.len() {
            let sample = shaped[position as usize];
            g.feed((sample, 0.0));
            errors.push(g.error().abs());
            position += g.interval();
        }
        let early: f64 = errors[..40].iter().sum::<f64>() / 40.0;
        let late: f64 = errors[errors.len() - 40..].iter().sum::<f64>() / 40.0;
        assert!(
            late < early,
            "timing error did not settle: {early} at the start, {late} at the end"
        );
    }
}
