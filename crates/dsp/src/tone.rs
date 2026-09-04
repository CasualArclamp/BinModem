//! Detecting single tones, and the reversals of phase in them.
//!
//! Modems mark time with tones. V.32's start-up (5.4) is conducted almost
//! entirely in them: one modem repeats a constellation state, which turns the
//! signal into the bare carrier, while the other alternates two opposite
//! states, which suppresses the carrier and leaves a pair of sidebands. Each
//! end then measures how long its own transmission takes to come back by
//! reversing the phase of what it is sending and waiting to hear the reversal
//! arrive. Nothing about that needs a demodulator, and doing it with one would
//! be the wrong way round: the delay being measured is what the demodulator
//! will need in order to work.

use crate::Nco;
use crate::filter::OnePole;
use std::collections::VecDeque;

/// Narrowband detector for one frequency.
///
/// A correlator: multiply the input by the conjugate of the tone and average.
/// Anything at that frequency comes to rest as a steady phasor whose length is
/// the amplitude and whose angle is the phase; everything else keeps turning
/// and averages away, the faster the further off it is. The averaging time is
/// therefore also the selectivity, and the two cannot be chosen separately.
#[derive(Debug, Clone)]
pub struct ToneDetector {
    nco: Nco,
    re: OnePole,
    im: OnePole,
}

impl ToneDetector {
    /// `bandwidth` is the half-power width of the detector, in hertz. It sets
    /// how long the detector takes to respond as much as how sharp it is: 50 Hz
    /// settles in about twenty milliseconds and rejects a tone 300 Hz away by
    /// some fifteen decibels.
    pub fn new(freq: f64, bandwidth: f64, fs: f64) -> Self {
        let tau = 1.0 / (std::f64::consts::TAU * bandwidth.max(1.0));
        Self {
            nco: Nco::new(freq, fs),
            re: OnePole::new(tau, fs),
            im: OnePole::new(tau, fs),
        }
    }

    pub fn feed(&mut self, x: f64) {
        let (cos, sin) = self.nco.step();
        self.re.process(x * cos);
        self.im.process(x * -sin);
    }

    /// The phasor: length is amplitude, angle is phase.
    pub fn phasor(&self) -> (f64, f64) {
        (self.re.value(), self.im.value())
    }

    /// Amplitude of the tone, on the same scale as the input.
    ///
    /// Twice the phasor, because multiplying a real cosine by a complex
    /// exponential puts half its energy at the sum frequency, which the
    /// averaging removes.
    pub fn amplitude(&self) -> f64 {
        let (re, im) = self.phasor();
        2.0 * (re * re + im * im).sqrt()
    }

    pub fn phase(&self) -> f64 {
        self.im.value().atan2(self.re.value())
    }
}

/// Watches one tone for the reversals of phase V.32 uses as timing marks.
///
/// A reversal is abrupt and a frequency offset is steady, which is the whole
/// difference between them and the only thing worth measuring. So the phasor
/// is compared against itself a short while ago rather than against a fixed
/// direction: half a turn in a few milliseconds is a reversal, while the seven
/// hertz of offset 2.1 allows for moves the phase by only a few tens of
/// degrees over the same interval and never accumulates, because the
/// comparison slides along with it.
///
/// The delay has to be longer than the detector takes to settle, or the
/// comparison is made against a phasor that had not finished arriving, and
/// short enough that an offset cannot turn a quarter within it.
#[derive(Debug, Clone)]
pub struct ReversalDetector {
    tone: ToneDetector,
    /// Phasor directions, oldest at the back.
    history: VecDeque<Option<(f64, f64)>>,
    threshold: f64,
    /// Samples the phasor has been opposed to its past for.
    opposed: u32,
    confirm: u32,
    /// Samples to ignore after declaring one, so a single reversal is counted
    /// once rather than for as long as it sits in the comparison window.
    refractory: u32,
    quiet: u32,
    count: u32,
    /// Slow envelope of the amplitude, for deciding the tone is there.
    envelope: OnePole,
    latency: u32,
}

impl ReversalDetector {
    /// `threshold` is the amplitude the tone must reach to be believed at all.
    ///
    /// Everything else follows from the bandwidth, and has to: the comparison
    /// is between the phasor now and the phasor a fixed time ago, and those
    /// two are only opposed during the stretch that begins once the new phase
    /// has settled and ends once the old one has fallen out of the window. Set
    /// the delay too short, or ask for the opposition to persist too long, and
    /// that stretch closes up entirely. The first attempt at this had them
    /// within a factor of two of each other and left a window twenty-one
    /// samples wide for a condition that had to hold for sixty-four.
    pub fn new(freq: f64, bandwidth: f64, threshold: f64, fs: f64) -> Self {
        let tau = fs / (std::f64::consts::TAU * bandwidth.max(1.0));
        // Six time constants back: the old phase is still there long after the
        // new one has arrived. Seven hertz of carrier offset turns forty
        // degrees in that time, which is nowhere near the hundred and thirty
        // five a reversal has to reach.
        let delay = (6.0 * tau).ceil() as usize;
        // And one for the opposition to persist, which sits comfortably inside
        // the three and a half the two conditions leave open.
        let confirm = tau.ceil() as u32;
        Self {
            tone: ToneDetector::new(freq, bandwidth, fs),
            history: VecDeque::from(vec![None; delay.max(1)]),
            threshold,
            opposed: 0,
            confirm,
            refractory: delay as u32,
            quiet: 0,
            count: 0,
            envelope: OnePole::new(0.100, fs),
            // While the average still holds some of the old phase, the
            // phasor is the new one less what is left of the old: a mix that
            // goes as 1 - 2exp(-t/tau) and therefore changes sign at tau ln 2.
            // Opposition begins there, and has to hold for a further tau.
            latency: ((std::f64::consts::LN_2 + 1.0) * tau).round() as u32,
        }
    }

    /// How long after a reversal the detector reports it, in samples.
    ///
    /// While the average still holds some of the old phase, the phasor is the
    /// new phase less what remains of the old, which goes as 1 - 2exp(-t/tau)
    /// and so changes sign at tau ln 2. That is when the two directions become
    /// opposed, and the opposition then has to hold for a further tau before
    /// it is believed.
    ///
    /// Anything measuring an interval between two reversals it detected itself
    /// carries this twice, once at each end. V.32's round-trip measurement is
    /// exactly such an interval, and at the bandwidths used here the two
    /// together come to some fifty symbol periods, which is most of the answer
    /// on a short line.
    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// Feed one sample. Returns true on the sample a reversal is confirmed.
    pub fn feed(&mut self, x: f64) -> bool {
        self.tone.feed(x);
        let (re, im) = self.tone.phasor();
        let magnitude = (re * re + im * im).sqrt();
        self.envelope.process(self.tone.amplitude());

        // Direction, when there is enough of a phasor for one to mean
        // anything. At the instant of a reversal there is not: the average
        // holds equal parts of the old phase and the new, and they cancel.
        let now = if magnitude > 1.0e-9 {
            Some((re / magnitude, im / magnitude))
        } else {
            None
        };
        let then = self.history.pop_back().flatten();
        self.history.push_front(now);

        if self.quiet > 0 {
            self.quiet -= 1;
            self.opposed = 0;
            return false;
        }
        if !self.present() {
            self.opposed = 0;
            return false;
        }
        // Too small at either end to compare: pass over it rather than start
        // again, because the hole in the middle of a reversal is exactly where
        // this happens and starting again there would mean never seeing one.
        let (Some(now), Some(then)) = (now, then) else {
            return false;
        };

        if now.0 * then.0 + now.1 * then.1 < -0.7 {
            self.opposed += 1;
            if self.opposed >= self.confirm {
                self.opposed = 0;
                self.quiet = self.refractory;
                self.count += 1;
                return true;
            }
        } else {
            self.opposed = 0;
        }
        false
    }

    pub fn amplitude(&self) -> f64 {
        self.tone.amplitude()
    }

    /// How many reversals have been seen since the detector was made.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Whether the tone is there at all.
    ///
    /// Judged on a slow envelope rather than the instant, so that the dip a
    /// reversal puts in the middle of itself does not read as the tone going
    /// away.
    pub fn present(&self) -> bool {
        self.envelope.value() >= self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const FS: f64 = 16_000.0;

    /// A tone that reverses phase at each of `at` (in samples).
    fn reversing(freq: f64, amplitude: f64, at: &[usize], n: usize) -> Vec<f64> {
        let mut sign = 1.0;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            if at.contains(&i) {
                sign = -sign;
            }
            out.push(sign * amplitude * (TAU * freq * i as f64 / FS).cos());
        }
        out
    }

    #[test]
    fn a_tone_is_measured_at_its_own_frequency_and_not_elsewhere() {
        for freq in [600.0, 1800.0, 3000.0] {
            let mut d = ToneDetector::new(freq, 50.0, FS);
            for i in 0..8000 {
                d.feed(0.3 * (TAU * freq * i as f64 / FS).cos());
            }
            assert!(
                (d.amplitude() - 0.3).abs() < 0.01,
                "{freq} Hz measured {:.4} rather than 0.3",
                d.amplitude()
            );
        }
    }

    #[test]
    fn a_tone_elsewhere_is_rejected() {
        let mut d = ToneDetector::new(1800.0, 50.0, FS);
        for i in 0..8000 {
            // The neighbouring sideband of V.32's alternating states.
            d.feed(0.3 * (TAU * 3000.0 * i as f64 / FS).cos());
        }
        assert!(
            d.amplitude() < 0.03,
            "1200 Hz away still reads {:.4}",
            d.amplitude()
        );
    }

    #[test]
    fn reversals_are_found_where_they_were_put() {
        // 5.4.1 has the calling modem wait for one reversal, then a second,
        // and the time between them is the round trip it is measuring. Getting
        // the count wrong would mean measuring the wrong interval.
        let at = [4000usize, 9000];
        let samples = reversing(3000.0, 0.3, &at, 14_000);
        let mut d = ReversalDetector::new(3000.0, 50.0, 0.05, FS);
        let mut found = Vec::new();
        for (i, &x) in samples.iter().enumerate() {
            if d.feed(x) {
                found.push(i);
            }
        }
        assert_eq!(found.len(), 2, "found reversals at {found:?}");
        for (got, want) in found.iter().zip(at.iter()) {
            // The detector cannot report a reversal before its averaging has
            // caught up with it, so it is always late. How late is what
            // `latency` claims, and anything measuring an interval between two
            // detections has to take that off twice.
            let late = *got as i64 - *want as i64;
            let claimed = i64::from(d.latency());
            assert!(
                (late - claimed).abs() < claimed / 8,
                "a reversal at {want} was reported at {got}, {late} samples \
                 late, against the {claimed} claimed"
            );
        }
    }

    #[test]
    fn a_steady_tone_produces_no_reversals() {
        let samples = reversing(1800.0, 0.3, &[], 20_000);
        let mut d = ReversalDetector::new(1800.0, 50.0, 0.05, FS);
        for x in samples {
            assert!(!d.feed(x));
        }
        assert_eq!(d.count(), 0);
    }

    #[test]
    fn a_frequency_offset_is_not_mistaken_for_a_reversal() {
        // V.32 2.1 allows the received carrier to be out by seven hertz, which
        // turns the phasor right round every seventh of a second. A detector
        // that compared against a fixed direction would call that a reversal
        // several times a second.
        let mut d = ReversalDetector::new(1800.0, 50.0, 0.05, FS);
        for i in 0..(FS as usize * 3) {
            d.feed(0.3 * (TAU * 1807.0 * i as f64 / FS).cos());
        }
        assert_eq!(
            d.count(),
            0,
            "seven hertz of offset was read as {} reversals",
            d.count()
        );
    }

    #[test]
    fn silence_clears_the_reference_rather_than_reversing() {
        // A tone that stops and starts again has not turned over, and 5.4.1
        // has the calling modem cease transmitting partway through.
        let mut d = ReversalDetector::new(1800.0, 50.0, 0.05, FS);
        for i in 0..8000 {
            d.feed(0.3 * (TAU * 1800.0 * i as f64 / FS).cos());
        }
        for _ in 0..8000 {
            d.feed(0.0);
        }
        // Back again, in the opposite phase.
        for i in 0..8000 {
            d.feed(-0.3 * (TAU * 1800.0 * i as f64 / FS).cos());
        }
        assert_eq!(d.count(), 0, "a gap was read as a reversal");
    }
}
