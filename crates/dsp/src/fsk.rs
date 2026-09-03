//! Frequency-shift-keying detection.

use crate::filter::{Cascade, OnePole, bandpass, butter_lowpass};
use crate::nco::Nco;

/// Streaming FSK frequency discriminator with carrier detection.
///
/// Why a discriminator rather than two matched tone filters: Bell 103 places
/// its tones 200 Hz apart at 300 baud, a modulation index of 0.67. The tones
/// are therefore *not* orthogonal, and a dual-tone energy detector suffers
/// heavy cross-leakage between them. Measuring instantaneous frequency avoids
/// the problem entirely, and is what real FSK receivers do.
///
/// Chain: band isolation, quadrature downconversion to band centre, complex
/// low-pass, phase differencing, then post-detection low-pass.
#[derive(Debug)]
pub struct FskDetector {
    nco: Nco,
    band: Cascade,
    lp_i: Cascade,
    lp_q: Cascade,
    post: Cascade,
    prev_i: f64,
    prev_q: f64,
    fs: f64,
    deviation: f64,
    fast_env: OnePole,
    slow_env: OnePole,
}

impl FskDetector {
    /// `f_space` and `f_mark` are the two signalling tones; `baud` sets the
    /// post-detection bandwidth.
    pub fn new(f_space: f64, f_mark: f64, baud: f64, fs: f64) -> Self {
        let centre = (f_space + f_mark) / 2.0;
        let deviation = (f_mark - f_space).abs() / 2.0;
        // Wide enough for the shifted tones plus modulation sidebands, tight
        // enough to reject the opposite direction, which on a 2-wire tap is
        // present at full strength in the other band.
        let half = deviation + baud * 0.9;
        // Order 8, not 4. The dominant interferer is not the far end but our
        // own transmitter: a 2-wire hybrid typically leaks near-end signal only
        // 10-15 dB below the received level. Order 4 rejects the opposite Bell
        // 103 band by just 17 dB, which leaves no margin; order 8 gives 34 dB.
        // The added group delay costs nothing here because async framing
        // re-acquires on every start bit.
        Self {
            nco: Nco::new(centre, fs),
            band: bandpass(8, (centre - half).max(60.0), centre + half, fs),
            lp_i: butter_lowpass(4, baud * 1.3, fs),
            lp_q: butter_lowpass(4, baud * 1.3, fs),
            post: butter_lowpass(2, baud * 0.8, fs),
            prev_i: 0.0,
            prev_q: 0.0,
            fs,
            deviation,
            fast_env: OnePole::new(0.005, fs),
            slow_env: OnePole::new(0.250, fs),
        }
    }

    /// Feed one line sample; returns the normalised frequency offset, where
    /// `+1` is a mark and `-1` a space.
    #[inline]
    pub fn feed(&mut self, x: f64) -> f64 {
        let b = self.band.process(x);
        let (c, s) = self.nco.step();
        // Downconvert by multiplying with exp(-j*2*pi*fc*t).
        let i = self.lp_i.process(b * c);
        let q = self.lp_q.process(b * -s);

        // Instantaneous frequency is arg(z * conj(z_prev)) * fs / 2pi.
        let (prev_i, prev_q) = (self.prev_i, self.prev_q);
        let re = i * prev_i + q * prev_q;
        let im = q * prev_i - i * prev_q;
        self.prev_i = i;
        self.prev_q = q;

        let mag = (i * i + q * q).sqrt();
        self.fast_env.process(mag);
        self.slow_env.process(mag);

        // atan2(0,0) returns 0, which reads as band centre: correct when idle.
        let hz = im.atan2(re) * self.fs / std::f64::consts::TAU;
        self.post.process(hz) / self.deviation
    }

    /// True while a carrier is present in this band.
    ///
    /// Compares a 5 ms envelope against a 250 ms average, so it tracks level
    /// changes across a call instead of relying on an absolute threshold.
    pub fn carrier(&self) -> bool {
        let slow = self.slow_env.value();
        slow > 1e-4 && self.fast_env.value() > 0.35 * slow
    }

    pub fn level(&self) -> f64 {
        self.fast_env.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// Generate a continuous-phase FSK burst for the given bit pattern.
    fn fsk(bits: &[u8], f_space: f64, f_mark: f64, baud: f64, fs: f64) -> Vec<f64> {
        let sps = (fs / baud) as usize;
        let mut phase = 0.0;
        let mut out = Vec::with_capacity(bits.len() * sps);
        for &b in bits {
            let f = if b == 1 { f_mark } else { f_space };
            for _ in 0..sps {
                phase += TAU * f / fs;
                out.push(phase.sin());
            }
        }
        out
    }

    #[test]
    fn discriminates_bell103_answer_tones() {
        let (fs, baud) = (16000.0, 300.0);
        let (space, mark) = (2025.0, 2225.0);
        let bits: Vec<u8> = [1u8, 1, 0, 0, 1, 0, 1, 0, 0, 1].repeat(6);
        let sig = fsk(&bits, space, mark, baud, fs);
        let mut det = FskDetector::new(space, mark, baud, fs);
        let sps = (fs / baud) as usize;
        let d: Vec<f64> = sig.iter().map(|&x| det.feed(x)).collect();

        // The chain has real group delay, so search for the sampling offset
        // that decodes cleanly rather than assuming zero. A receiver never
        // needs this: async framing re-syncs on every start bit.
        let score = |lag: usize| {
            let (mut errors, mut checked) = (0usize, 0usize);
            for (i, &b) in bits.iter().enumerate().skip(6) {
                let idx = i * sps + sps / 2 + lag;
                if idx >= d.len() {
                    break;
                }
                errors += usize::from(u8::from(d[idx] > 0.0) != b);
                checked += 1;
            }
            (errors, checked)
        };

        let best = (0..3 * sps)
            .map(|lag| (score(lag), lag))
            .filter(|((_, checked), _)| *checked > 40)
            .min_by_key(|((errors, _), _)| *errors)
            .expect("no usable sampling offset");
        let ((errors, checked), lag) = best;

        assert_eq!(errors, 0, "{errors}/{checked} bits wrong at best lag {lag}");
        assert!(
            lag < 2 * sps,
            "group delay {lag} samples is over two bit periods"
        );
    }

    #[test]
    fn carrier_detect_follows_the_signal() {
        let (fs, baud) = (16000.0, 300.0);
        let mut det = FskDetector::new(2025.0, 2225.0, baud, fs);
        for _ in 0..(fs as usize / 2) {
            det.feed(0.0);
        }
        assert!(!det.carrier(), "claimed carrier on silence");
        let sig = fsk(&[1; 200], 2025.0, 2225.0, baud, fs);
        for &x in &sig {
            det.feed(x);
        }
        assert!(det.carrier(), "missed a strong carrier");
    }
}
