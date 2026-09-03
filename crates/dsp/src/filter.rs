//! Biquad sections and Butterworth cascade design.
//!
//! Every filter here is a streaming, stateful object: one sample in, one
//! sample out. Nothing in the signal path is allowed to be block-oriented,
//! because a modem's loops must stay converged across block boundaries.

use std::f64::consts::PI;

/// One second-order section in transposed direct-form II.
///
/// TDF-II is preferred over DF-I here: it needs two state words instead of
/// four and has better numerical behaviour at the low corner frequencies we
/// use for loop filters.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b0: f64, b1: f64, b2: f64,
    a1: f64, a2: f64,
    z1: f64, z2: f64,
}

impl Biquad {
    /// Build from unnormalised coefficients, dividing through by `a0`.
    fn new(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self { b0: b0/a0, b1: b1/a0, b2: b2/a0, a1: a1/a0, a2: a2/a0, z1: 0.0, z2: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) { self.z1 = 0.0; self.z2 = 0.0; }
}

/// A cascade of biquads, evaluated in order.
#[derive(Debug, Clone, Default)]
pub struct Cascade {
    sections: Vec<Biquad>,
}

impl Cascade {
    pub fn from_sections(sections: Vec<Biquad>) -> Self { Self { sections } }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        self.sections.iter_mut().fold(x, |acc, s| s.process(acc))
    }

    pub fn reset(&mut self) { for s in &mut self.sections { s.reset(); } }
    pub fn len(&self) -> usize { self.sections.len() }
    pub fn is_empty(&self) -> bool { self.sections.is_empty() }
}

/// Damping ratios of the conjugate pole pairs of an order-`n` Butterworth
/// prototype. For even `n` there are `n/2` pairs and no real pole.
///
/// The poles sit on the unit circle at angles pi*(2k+1)/(2n) from the
/// imaginary axis, so each quadratic factor is `s^2 + 2*zeta*s + 1`.
fn butterworth_zetas(n: usize) -> Vec<f64> {
    assert!(n >= 2 && n.is_multiple_of(2), "only even Butterworth orders are supported");
    (0..n / 2)
        .map(|k| (PI * (2 * k + 1) as f64 / (2.0 * n as f64)).sin())
        .collect()
}

/// Butterworth low-pass, order `n` (even), corner `fc` Hz at rate `fs`.
pub fn butter_lowpass(n: usize, fc: f64, fs: f64) -> Cascade {
    let w0 = 2.0 * PI * (fc / fs).clamp(1e-6, 0.4999);
    let (cw, sw) = (w0.cos(), w0.sin());
    let sections = butterworth_zetas(n).into_iter().map(|zeta| {
        let alpha = sw * zeta;                       // alpha = sin(w0)/(2Q), Q = 1/(2*zeta)
        let b1 = 1.0 - cw;
        Biquad::new(b1 / 2.0, b1, b1 / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
    }).collect();
    Cascade::from_sections(sections)
}

/// Butterworth high-pass, order `n` (even), corner `fc` Hz at rate `fs`.
pub fn butter_highpass(n: usize, fc: f64, fs: f64) -> Cascade {
    let w0 = 2.0 * PI * (fc / fs).clamp(1e-6, 0.4999);
    let (cw, sw) = (w0.cos(), w0.sin());
    let sections = butterworth_zetas(n).into_iter().map(|zeta| {
        let alpha = sw * zeta;
        let b1 = 1.0 + cw;
        Biquad::new(b1 / 2.0, -b1, b1 / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
    }).collect();
    Cascade::from_sections(sections)
}

/// Band-pass built as a high-pass followed by a low-pass.
///
/// This is not a textbook Butterworth band-pass (which would come from the
/// LP->BP frequency transformation), but for band isolation ahead of a
/// detector the cascade is easier to reason about and has a flatter passband.
pub fn bandpass(n: usize, f_lo: f64, f_hi: f64, fs: f64) -> Cascade {
    let mut sections = butter_highpass(n, f_lo, fs).into_sections();
    sections.extend(butter_lowpass(n, f_hi, fs).into_sections());
    Cascade::from_sections(sections)
}

impl Cascade {
    fn into_sections(self) -> Vec<Biquad> { self.sections }
}

/// One-pole smoother, `tau` in seconds.
#[derive(Debug, Clone, Copy)]
pub struct OnePole { a: f64, y: f64 }

impl OnePole {
    pub fn new(tau: f64, fs: f64) -> Self {
        Self { a: (-1.0 / (tau * fs)).exp(), y: 0.0 }
    }

    /// Start from `initial` rather than zero.
    ///
    /// Matters wherever the smoothed value is a divisor: a gain control that
    /// starts at zero produces an enormous gain for its first few inputs, which
    /// is long enough to destabilise anything adapting downstream.
    pub fn starting_at(initial: f64, tau: f64, fs: f64) -> Self {
        Self { a: (-1.0 / (tau * fs)).exp(), y: initial }
    }
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        self.y = self.a * self.y + (1.0 - self.a) * x;
        self.y
    }
    pub fn value(&self) -> f64 { self.y }
    pub fn reset(&mut self) { self.y = 0.0; }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Magnitude response by driving a sinusoid through and measuring output.
    fn gain_at(mut f: Cascade, freq: f64, fs: f64) -> f64 {
        let n = (fs * 0.5) as usize;
        let mut peak: f64 = 0.0;
        for i in 0..n {
            let y = f.process((2.0 * PI * freq * i as f64 / fs).sin());
            if i > n / 2 { peak = peak.max(y.abs()); }   // skip the transient
        }
        peak
    }

    #[test]
    fn lowpass_is_minus_3db_at_corner() {
        let g = gain_at(butter_lowpass(4, 1000.0, 16000.0), 1000.0, 16000.0);
        assert!((20.0 * g.log10() + 3.0).abs() < 0.35, "corner gain {} dB", 20.0*g.log10());
    }

    #[test]
    fn lowpass_rejects_stopband() {
        let g = gain_at(butter_lowpass(4, 1000.0, 16000.0), 4000.0, 16000.0);
        assert!(20.0 * g.log10() < -40.0, "stopband {} dB", 20.0 * g.log10());
    }

    #[test]
    fn highpass_is_minus_3db_at_corner() {
        let g = gain_at(butter_highpass(4, 1000.0, 16000.0), 1000.0, 16000.0);
        assert!((20.0 * g.log10() + 3.0).abs() < 0.35, "corner gain {} dB", 20.0*g.log10());
    }

    /// The case that actually matters: in originate mode our own 1070/1270
    /// transmitter leaks through the hybrid into the 2025/2225 receiver only
    /// 10-15 dB down, so the band filter carries the burden of separating them.
    /// This is why `FskDetector` uses order 8 rather than order 4.
    #[test]
    fn bandpass_separates_the_two_bell103_bands() {
        let fs = 16000.0;
        let answer = || bandpass(8, 1755.0, 2495.0, fs);
        for f in [2025.0, 2125.0, 2225.0] {
            let g = gain_at(answer(), f, fs);
            assert!(g > 0.7, "answer tone {f} Hz attenuated to {g}");
        }
        // Budget: the hybrid leaks our transmitter about 12 dB below the far
        // end, and the far end arrives roughly 20 dB below our own transmit
        // level, so 20 dB of filter rejection on the worst-case tone still
        // leaves a comfortable signal-to-interference ratio for FSK. 1270 Hz
        // sits closest to the band edge and so sets the limit at about 24 dB;
        // the tones further out get 30 dB or better.
        for (f, min_db) in [(1070.0, 30.0), (1170.0, 26.0), (1270.0, 20.0)] {
            let db = -20.0 * gain_at(answer(), f, fs).log10();
            assert!(db > min_db, "originate tone {f} Hz rejected by only {db:.1} dB");
        }
    }

    /// A 4th-order Butterworth rolls off at 24 dB/octave, so pin the response
    /// to that rather than to an arbitrary threshold.
    #[test]
    fn lowpass_rolloff_is_24db_per_octave() {
        let fs = 32000.0;
        let one = gain_at(butter_lowpass(4, 1000.0, fs), 2000.0, fs);
        let two = gain_at(butter_lowpass(4, 1000.0, fs), 4000.0, fs);
        let octave_db = 20.0 * one.log10() - 20.0 * two.log10();
        assert!((octave_db - 24.0).abs() < 1.5, "second octave fell {octave_db} dB");
    }
}
