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
/// Averaging one-poles per axis. Two, for the reason in [`ToneDetector::new`].
const POLES: usize = 2;

/// Where two cascaded one-poles are half power, as a fraction of the width one
/// of them would be. `sqrt(2^(1/2) - 1)`.
const CASCADE_CORNER: f64 = 0.643_594_252_905_582_5;

/// Where the cascade's mix of old phase and new changes sign, in units of one
/// pole's own time constant.
///
/// A single pole settles as `1 - exp(-u)`, so during a reversal the phasor is
/// the new phase less what is left of the old and goes as `1 - 2exp(-u)`,
/// which crosses zero at `ln 2`. Two poles settle as `1 - (1 + u)exp(-u)`, so
/// the same difference goes as `1 - 2(1 + u)exp(-u)` and crosses where
/// `(1 + u)exp(-u) = 1/2`. There is no closed form; this is the root.
const CASCADE_CROSSING: f64 = 1.678_347;

/// How far the phasor may turn across the comparison window and still be
/// judged.
///
/// [`ReversalDetector::new`] works out that seven hertz of carrier offset
/// turns forty degrees in that window, and a hundred and thirty-five degrees
/// is what counts as opposed -- so about twenty-four hertz of offset is enough
/// to make a detector watching a perfectly steady tone report a reversal as
/// often as it is allowed to. Forty-five degrees is a little more than the
/// design already contemplated and a long way short of the angle that would
/// fire it on its own.
///
/// Not a theoretical worry. On a recorded call the far end's carrier sat 22 Hz
/// off and then 36 Hz off through the two seconds before its real reversal,
/// and the detector declared fifty-one of them at exactly its own floor of
/// 18.6 ms; through the ten seconds after, the same measurement read 0.0 Hz
/// and the detector declared one.
const MAX_CARRY: f64 = std::f64::consts::FRAC_PI_4;

#[derive(Debug, Clone)]
pub struct ToneDetector {
    nco: Nco,
    re: [OnePole; POLES],
    im: [OnePole; POLES],
}

impl ToneDetector {
    /// `bandwidth` is the half-power width of the detector, in hertz.
    ///
    /// Two poles rather than one, at the same half-power width. A single pole
    /// falls away at six decibels an octave, which is barely falling away at
    /// all: a 60 Hz detector still passes a twentieth of a tone 1200 Hz off.
    /// That twentieth is not a rounding error here. It is exactly the distance
    /// from V.32's carrier to its sidebands, and a calling modem in state AA is
    /// putting its whole transmission at the carrier while listening for the
    /// far end at the sidebands -- so a twentieth of its own signal lands
    /// precisely where it is trying to hear somebody else.
    ///
    /// On a line with a hybrid that is survivable, since the hybrid has already
    /// taken twelve decibels off the echo. Written to a virtual cable there is
    /// no hybrid, the echo comes back at full strength, and a twentieth of it
    /// is a steady phasor large enough that the far end reversing its own
    /// phase barely moves the sum. The modem sits in AA waiting for a reversal
    /// it can no longer see.
    ///
    /// Two poles cost about a factor of two in that leakage and gain a factor
    /// of eight in rejection: each is widened by [`CASCADE_CORNER`] so the pair
    /// is still half power at `bandwidth`, and the skirt then falls at twelve
    /// decibels an octave instead of six.
    pub fn new(freq: f64, bandwidth: f64, fs: f64) -> Self {
        // Each pole widened, so that the cascade is half power where one pole
        // of `bandwidth` would have been.
        let each = bandwidth.max(1.0) / CASCADE_CORNER;
        let tau = 1.0 / (std::f64::consts::TAU * each);
        Self {
            nco: Nco::new(freq, fs),
            re: std::array::from_fn(|_| OnePole::new(tau, fs)),
            im: std::array::from_fn(|_| OnePole::new(tau, fs)),
        }
    }

    pub fn feed(&mut self, x: f64) {
        let (cos, sin) = self.nco.step();
        let mut r = x * cos;
        let mut i = x * -sin;
        for pole in &mut self.re {
            r = pole.process(r);
        }
        for pole in &mut self.im {
            i = pole.process(i);
        }
    }

    /// The phasor: length is amplitude, angle is phase.
    pub fn phasor(&self) -> (f64, f64) {
        (self.re[POLES - 1].value(), self.im[POLES - 1].value())
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
        self.im[POLES - 1].value().atan2(self.re[POLES - 1].value())
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
    /// The direction on the previous sample, for measuring how fast the
    /// phasor is turning.
    previous: Option<(f64, f64)>,
    /// Radians per sample the phasor is turning by, averaged slowly.
    ///
    /// A carrier that is simply off frequency turns steadily, and the
    /// comparison this detector makes cannot tell that from a phase that
    /// stepped: [`ReversalDetector::new`] works out that seven hertz of offset
    /// turns forty degrees in the comparison window, so about twenty-four
    /// turns the hundred and thirty-five that counts as opposed. Past that a
    /// detector watching a perfectly steady tone reports a reversal as often
    /// as it is allowed to.
    ///
    /// Measured on a real call, this is not a theoretical worry. In the two
    /// seconds before one far end's genuine reversal its carrier sat 22 Hz
    /// off, then 36 Hz off, and the detector declared fifty-one reversals at
    /// exactly its own floor of 18.6 ms; through the ten seconds after it, the
    /// offset measured 0.0 Hz and the detector declared one.
    ///
    /// So the turn is taken out before the comparison. The average is slow
    /// enough -- half a second -- that the reversal's own half-turn moves it
    /// by about a hertz, and steady enough that an offset is gone from the
    /// comparison within a second of arriving.
    drift: f64,
    drift_rate: f64,
    /// Samples the drift estimate has been forming over.
    ///
    /// An exponential average is worth nothing until it has run for its own
    /// time constant, and half a second of not knowing is half a second of the
    /// fault this exists to prevent. So it runs as a plain mean until it has
    /// as many samples as the average is long, and as an average after that.
    settled: u32,
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
            previous: None,
            drift: 0.0,
            settled: 0,
            // Half a second, in the one-pole form used everywhere else here.
            drift_rate: 1.0 - (-1.0 / (0.5 * fs)).exp(),
            // While the average still holds some of the old phase, the
            // phasor is the new one less what is left of the old. Where that
            // mix changes sign is where opposition begins, and it then has to
            // hold for a further tau before it is believed.
            //
            // The crossing is [`CASCADE_CROSSING`] of one pole's own time
            // constant, and each pole is [`CASCADE_CORNER`] narrower in time
            // than a single pole of the same half-power width would be. The
            // two together come to a little over twice tau rather than the
            // 1.69 a single pole gave -- which is not a detail: this number is
            // subtracted from every round trip V.32 measures, and getting it
            // wrong by twenty samples put five symbols on the answer.
            latency: ((CASCADE_CROSSING * CASCADE_CORNER + 1.0) * tau).round() as u32,
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
        // anything, and when there is a tone for it to be the direction of.
        //
        // At the instant of a reversal there is not enough phasor: the average
        // holds equal parts of the old phase and the new and they cancel, and
        // the hole that leaves is passed over below rather than started again
        // from, since it is exactly where a reversal lives.
        //
        // The presence test is there because a line with nothing on it is not
        // silent, it is noisy, and noise has a phase like anything else. A
        // history filled while waiting holds directions that are perfectly
        // well defined and mean nothing whatever, and the honest value for a
        // direction nobody sent is no direction at all.
        //
        // It changes no outcome that is presently known. The output was
        // already gated on the same test, and the envelope deciding it takes
        // about a tenth of a second to cross while the history is sixteen
        // milliseconds deep -- so by the time a tone counts as present, the
        // history it will be compared against is already the tone. This is
        // saying the thing the code meant rather than fixing something it
        // measurably got wrong.
        let now = if self.present() && magnitude > 1.0e-9 {
            Some((re / magnitude, im / magnitude))
        } else {
            None
        };
        // How fast the phasor is turning, before anything is compared. Taken
        // from consecutive samples, where a reversal is a step of pi spread
        // over a couple of time constants and so contributes about a hertz to
        // an average half a second long.
        if let (Some(now), Some(prev)) = (now, self.previous) {
            let turn = (prev.0 * now.1 - prev.1 * now.0)
                .atan2(prev.0 * now.0 + prev.1 * now.1);
            self.settled = self.settled.saturating_add(1);
            let rate = self.drift_rate.max(1.0 / f64::from(self.settled));
            self.drift += (turn - self.drift) * rate;
        } else {
            // No tone, so nothing to measure and nothing worth keeping: what
            // it was turning at before it went away says nothing about what it
            // will be turning at when it comes back.
            self.settled = 0;
            self.drift = 0.0;
        }
        self.previous = now;

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

        // How far a tone simply sitting off frequency would have turned
        // between the two directions being compared. Past a point that is no
        // longer a correction to make but a reason to say nothing: the two
        // cases are not distinguishable from a single comparison, and a tone
        // turning that fast is not one this was built to measure.
        //
        // Turning it out instead of refusing was tried, and doubled the count
        // on a real call. The estimate has to come from the same noisy phasor,
        // and de-rotating by a wrong angle manufactures oppositions of its
        // own; refusing can only ever remove one.
        if (self.drift * self.history.len() as f64).abs() > MAX_CARRY {
            self.opposed = 0;
            return false;
        }

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

    /// Run a tone `offset` hertz away from where the detector is looking, with
    /// an optional phase reversal half-way through, and count what it finds.
    fn offset_tone(offset: f64, reverse: bool) -> u32 {
        let fs = 16_000.0;
        let seconds = 4.0;
        let mut d = ReversalDetector::new(1800.0, 60.0, 0.008, fs);
        let mut found = 0;
        for i in 0..(fs * seconds) as usize {
            let t = i as f64 / fs;
            let flip = if reverse && t > seconds / 2.0 {
                std::f64::consts::PI
            } else {
                0.0
            };
            let x = 0.2 * (std::f64::consts::TAU * (1800.0 + offset) * t + flip).sin();
            if d.feed(x) {
                found += 1;
            }
        }
        found
    }

    #[test]
    fn a_tone_off_frequency_is_not_reversing() {
        // The fault this cost a real call. The comparison is between the
        // phasor now and the phasor six time constants ago, and a carrier that
        // is simply off frequency turns steadily through the angle that counts
        // as opposed and keeps going. `new` works out that seven hertz turns
        // forty degrees in that window, so around twenty-four reaches a
        // hundred and thirty-five -- and then the detector fires as often as
        // it is allowed to, on a tone that never did anything.
        assert_eq!(offset_tone(30.0, false), 0, "a steady tone, thirty hertz off");
        assert_eq!(offset_tone(-30.0, false), 0);
        assert_eq!(offset_tone(120.0, false), 0, "and a long way off");
    }

    #[test]
    fn a_small_offset_does_not_hide_a_real_reversal() {
        // The other half of it. Refusing to judge a phasor that is turning is
        // only worth doing if it still judges the ones that are not, and no
        // real line puts a carrier exactly where it belongs.
        assert_eq!(offset_tone(0.0, true), 1, "on frequency");
        assert_eq!(offset_tone(3.0, true), 1, "three hertz off");
        assert_eq!(offset_tone(-3.0, true), 1);
    }

    /// A tone arriving is not a tone reversing.
    ///
    /// The case a clean test line cannot produce, because a clean test line is
    /// silent before the signal and silence has no phase. A real line has
    /// noise, noise has a phase, and the phase it has is not the one the
    /// signal will arrive with.
    ///
    /// This passes without the presence test in `feed` as well as with it, so
    /// it is a property being written down rather than a bug being pinned.
    #[test]
    fn a_tone_appearing_out_of_noise_is_not_a_reversal() {
        let fs = 16_000.0;
        let mut d = ReversalDetector::new(600.0, 60.0, 0.008, fs);
        let mut rng = 12_345u64;
        let mut noise = || {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            ((rng >> 33) as f64 / (1u64 << 31) as f64 - 1.0) * 0.05
        };
        // Two seconds of a line with nothing on it but noise.
        for _ in 0..(fs as usize * 2) {
            assert!(!d.feed(noise()), "found a reversal in noise");
        }
        // Then the far end starts, at a settled phase of its own.
        let mut found = 0;
        for i in 0..(fs as usize * 2) {
            let t = i as f64 / fs;
            let x = 0.2 * (std::f64::consts::TAU * 600.0 * t + 1.1).sin() + noise();
            if d.feed(x) {
                found += 1;
            }
        }
        assert_eq!(found, 0, "read the arrival of a tone as {found} reversals");
    }

    /// And it still finds a real one afterwards.
    #[test]
    fn a_tone_that_arrives_and_then_reverses_is_still_caught() {
        let fs = 16_000.0;
        let mut d = ReversalDetector::new(600.0, 60.0, 0.008, fs);
        let mut rng = 999u64;
        let mut noise = || {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            ((rng >> 33) as f64 / (1u64 << 31) as f64 - 1.0) * 0.05
        };
        for _ in 0..(fs as usize) {
            d.feed(noise());
        }
        let mut at = None;
        let turn = fs as usize;
        for i in 0..(fs as usize * 2) {
            let t = i as f64 / fs;
            let sign = if i < turn { 1.0 } else { -1.0 };
            let x = 0.2 * sign * (std::f64::consts::TAU * 600.0 * t + 1.1).sin()
                + noise();
            if d.feed(x) && at.is_none() {
                at = Some(i);
            }
        }
        let at = at.expect("missed a reversal that really happened");
        let late = at as i64 - turn as i64;
        assert!(
            (0..fs as i64 / 10).contains(&late),
            "reported {late} samples from where the reversal was"
        );
    }

    /// The number V.32's start-up turns on.
    ///
    /// A calling modem in AA puts its whole transmission at 1800 Hz and listens
    /// for the far end 1200 Hz away, at the sidebands. Whatever fraction of the
    /// carrier reaches that detector is a steady phasor sitting exactly where
    /// the far end's reversal has to be seen, and on a line with no hybrid the
    /// carrier reaching it is the modem's own transmission at full strength.
    ///
    /// One pole gave a twentieth, which was enough to hide a far end ten
    /// decibels down. Two give better than a two-hundredth.
    #[test]
    fn a_carrier_does_not_reach_the_sideband_detector() {
        let fs = 16_000.0;
        let mut at_sideband = ToneDetector::new(1800.0 - 1200.0, 60.0, fs);
        let mut at_carrier = ToneDetector::new(1800.0, 60.0, fs);
        for i in 0..(fs as usize) {
            let x = (std::f64::consts::TAU * 1800.0 * i as f64 / fs).sin();
            at_sideband.feed(x);
            at_carrier.feed(x);
        }
        let leak = at_sideband.amplitude() / at_carrier.amplitude();
        assert!(
            leak < 0.005,
            "a carrier 1200 Hz away still reaches the detector at {leak:.4}"
        );
    }

    #[test]
    fn the_detector_is_still_half_power_where_it_says_it_is() {
        // Two poles rather than one, but each widened so the pair keeps the
        // half-power width it was asked for. Otherwise every timing derived
        // from that width -- and in the reversal detector all of them are --
        // would quietly mean something else.
        let fs = 16_000.0;
        let amplitude = |offset: f64| {
            let mut d = ToneDetector::new(1800.0, 60.0, fs);
            for i in 0..(fs as usize * 2) {
                d.feed((std::f64::consts::TAU * (1800.0 + offset) * i as f64 / fs).sin());
            }
            d.amplitude()
        };
        let at_centre = amplitude(0.0);
        let at_corner = amplitude(60.0);
        let db = 20.0 * (at_corner / at_centre).log10();
        assert!(
            (db + 3.0).abs() < 0.6,
            "the corner is {db:.2} dB down rather than three"
        );
    }
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
