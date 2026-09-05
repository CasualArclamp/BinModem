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
    carrier: bool,
}

/// Level at which a carrier is declared present, and the lower level at which
/// it is declared gone.
///
/// Five decibels apart, as V.22bis 6.5.2 asks for, and deliberately the same
/// pair the V.22bis receiver uses: both measure the magnitude of a
/// band-filtered complex baseband, so they are the same quantity on the same
/// scale and there is no reason for them to disagree.
const CARRIER_ON: f64 = 1.0e-3;
const CARRIER_OFF: f64 = 5.62e-4;

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
            carrier: false,
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
        let level = self.fast_env.process(mag);
        self.slow_env.process(mag);
        self.carrier = if self.carrier {
            level > CARRIER_OFF
        } else {
            level > CARRIER_ON
        };

        // atan2(0,0) returns 0, which reads as band centre: correct when idle.
        let hz = im.atan2(re) * self.fs / std::f64::consts::TAU;
        self.post.process(hz) / self.deviation
    }

    /// True while a carrier is present in this band.
    ///
    /// An absolute level with hysteresis, which sounds unambitious beside
    /// something that adapts to the line, and is the only thing that works.
    ///
    /// This used to compare a 5 ms envelope against a 250 ms one, on the
    /// reasoning that a ratio needs no threshold and so handles any line
    /// level. What it actually does is answer yes to every steady signal
    /// there is: the two envelopes of anything steady are equal, and equal
    /// passes every ratio test that is not one. The only floor under it was
    /// -80 dB, which no real line has ever been quieter than. So it found a
    /// carrier in the noise on an idle line, and a 300 bit/s modem duly
    /// reported CONNECT to a far end that had not said anything at all, then
    /// NO CARRIER the moment the noise moved. The test it passed fed it
    /// digital silence, which is the one quiet thing a line never is.
    pub fn carrier(&self) -> bool {
        self.carrier
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

    #[test]
    fn a_line_with_nothing_on_it_but_noise_has_no_carrier() {
        // The case the test above misses, and the one that matters. A real
        // line is never digitally silent: there is a noise floor, and on
        // anything carried over a network there is comfort noise put there
        // deliberately. A detector that only knows how to compare the signal
        // against itself says yes to all of it.
        let (fs, baud) = (16000.0, 300.0);
        let mut det = FskDetector::new(2025.0, 2225.0, baud, fs);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut noise = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // About -46 dBFS, which is an ordinary quiet line and far above
            // anything that should read as a carrier.
            ((state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 0.005
        };
        for _ in 0..(fs as usize) {
            det.feed(noise());
        }
        // The noise has to be loud enough that the old detector would have
        // called it a carrier, or this test proves nothing at all: its only
        // floor was 1e-4, and everything above that passed.
        assert!(
            det.level() > 1.0e-4,
            "the noise here reads {:.2e}, which is too quiet to be the test it              is meant to be",
            det.level()
        );
        assert!(
            !det.carrier(),
            "found a carrier in the noise on an idle line, at a level of {:.2e}",
            det.level()
        );
    }

    #[test]
    fn carrier_detect_holds_on_through_a_dip() {
        // Hysteresis, as V.22bis 6.5.2 asks for: five decibels between the
        // level that declares a carrier and the level that gives up on one.
        // Without the gap a signal sitting near the threshold chatters, and
        // every drop resets the framing.
        let (fs, baud) = (16000.0, 300.0);
        let mut det = FskDetector::new(2025.0, 2225.0, baud, fs);
        let strong = fsk(&[1; 200], 2025.0, 2225.0, baud, fs);
        for &x in &strong {
            det.feed(x);
        }
        assert!(det.carrier());
        // Down four decibels, which is inside the hysteresis.
        for &x in &strong {
            det.feed(x * 0.63);
        }
        assert!(det.carrier(), "gave up on a carrier that only dipped");
    }
}
