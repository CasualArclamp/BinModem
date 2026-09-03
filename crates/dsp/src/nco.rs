//! Numerically-controlled oscillator.

use std::f64::consts::TAU;

/// Fixed- or variable-frequency oscillator producing a unit complex phasor.
///
/// Phase is kept as a fraction of a turn in `[0,1)` rather than radians, so it
/// can run indefinitely without the precision loss an unbounded radian
/// accumulator would suffer.
#[derive(Debug, Clone, Copy)]
pub struct Nco {
    phase: f64,
    step: f64,
    fs: f64,
}

impl Nco {
    pub fn new(freq_hz: f64, fs: f64) -> Self {
        Self { phase: 0.0, step: freq_hz / fs, fs }
    }

    pub fn set_frequency(&mut self, freq_hz: f64) { self.step = freq_hz / self.fs; }
    pub fn frequency(&self) -> f64 { self.step * self.fs }

    /// Nudge the frequency, as a carrier-tracking loop would.
    pub fn adjust(&mut self, delta_hz: f64) { self.step += delta_hz / self.fs; }

    /// Advance one sample and return `(cos, sin)` of the new phase.
    #[inline]
    pub fn step(&mut self) -> (f64, f64) {
        self.phase += self.step;
        // Wrap by subtracting the floor: the accumulator stays exact for any
        // run length, unlike a radian accumulator that grows without bound.
        self.phase -= self.phase.floor();
        let r = self.phase * TAU;
        (r.cos(), r.sin())
    }

    pub fn reset(&mut self) { self.phase = 0.0; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_the_requested_frequency() {
        let fs = 16000.0;
        let mut nco = Nco::new(1000.0, fs);
        // Count zero crossings of the sine over one second: 1000 Hz gives 2000.
        let mut prev = 0.0;
        let mut crossings = 0;
        for _ in 0..fs as usize {
            let (_, s) = nco.step();
            if (prev <= 0.0 && s > 0.0) || (prev >= 0.0 && s < 0.0) {
                crossings += 1;
            }
            prev = s;
        }
        assert!((crossings as i64 - 2000).abs() <= 2, "crossings {crossings}");
    }

    #[test]
    fn phase_stays_bounded_over_a_long_run() {
        let mut nco = Nco::new(1234.5, 8000.0);
        for _ in 0..8_000_000 {
            nco.step();
        }
        assert!((0.0..1.0).contains(&nco.phase), "phase escaped: {}", nco.phase);
    }

    #[test]
    fn magnitude_is_unity() {
        let mut nco = Nco::new(777.0, 8000.0);
        for _ in 0..1000 {
            let (c, s) = nco.step();
            assert!((c * c + s * s - 1.0).abs() < 1e-12);
        }
    }
}
