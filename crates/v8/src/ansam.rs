//! Telling V.8's answering tone from V.25's.
//!
//! 7.2: "modified answer tone ANSam consists of a sinewave signal at
//! 2100 +/- 1 Hz with phase reversals at an interval of 450 +/- 25 ms,
//! amplitude-modulated by a sinewave at 15 +/- 0.1 Hz. The modulated envelope
//! shall range in amplitude between (0.8 +/- 0.01) and (1.2 +/- 0.01) times its
//! average amplitude."
//!
//! This matters because of the sentence in 7.2 that governs everything after
//! it: "a call DCE shall not transmit a signal CM unless ANSam has been
//! detected." A calling modem that cannot tell the two tones apart either
//! never negotiates, or talks V.8 at a modem that has never heard of it.
//!
//! The reversals are not the thing to look for, though they are the obvious
//! candidate. 7.2 again: "when network echo canceller disabling is not
//! required, phase reversals shall not be imparted to the ANSam signal" -- they
//! are there to knock out network echo cancellers, exactly as in V.25, and an
//! ANSam without them is a legal ANSam. What is always present is the
//! modulation, and that is what this looks for.
//!
//! Note 1 to 7.2 is worth reading before choosing time constants: "detector
//! design needs to allow for transient variations in the received answer-tone
//! amplitude and phase that may be generated occasionally by network
//! equipment". A reversal is such a transient, and it is imparted by the far
//! end deliberately twice a second.

use dsp::filter::OnePole;
use dsp::{Nco, ToneDetector};

/// The answering tone, 2100 Hz (V.25).
pub const ANSWER_TONE: f64 = 2100.0;

/// The rate ANSam's envelope is modulated at (7.2).
pub const MODULATION_RATE: f64 = 15.0;

/// The depth 7.2 asks for: an envelope between 0.8 and 1.2 of its average.
pub const NOMINAL_DEPTH: f64 = 0.2;

/// Half of nominal, which a tone has to beat to be called modulated.
///
/// Set low rather than near the nominal figure. What is being separated is a
/// modulated tone from an unmodulated one, and an unmodulated one measures
/// nothing at all here -- so the room is better spent on a network that has
/// flattened some of the modulation than on rejecting a tone that has none.
const MODULATED: f64 = 0.08;

/// The amplitude the tone must reach before any of this means anything.
const AUDIBLE: f64 = 0.002;

/// How far the tone has to stand above the rest of the line.
///
/// An absolute floor is not enough on its own. A detector 300 Hz wide of a
/// loud tone still hears a tenth of it, and 1800 Hz -- where V.32 puts its
/// carrier -- is exactly 300 Hz from 2100. Judged only by whether something
/// crossed a threshold, a modem's own start-up reads as an answering tone.
///
/// A sine wave has an amplitude of `pi/2` times its own mean rectified value,
/// so a clean tone measures about 1.57 here and nothing else comes close: a
/// loud 1800 Hz carrier measures about 0.15.
const STANDING: f64 = 0.75;

/// Watches for an answering tone and says which of the two it is.
#[derive(Debug)]
pub struct AnswerTone {
    tone: ToneDetector,
    /// Everything on the line, to weigh the tone against.
    power: OnePole,
    /// Average of the tone's amplitude: the carrier of the envelope.
    level: OnePole,
    /// A correlator at 15 Hz, run on the envelope rather than on the line.
    nco: Nco,
    re: OnePole,
    im: OnePole,
}

impl AnswerTone {
    pub fn new(fs: f64) -> Self {
        Self {
            // Wide enough to pass a 15 Hz modulation without flattening the
            // thing being measured, narrow enough to be about 2100 Hz and not
            // about the rest of the line.
            tone: ToneDetector::new(ANSWER_TONE, 60.0, fs),
            power: OnePole::new(0.050, fs),
            // Long against 15 Hz, so this is the envelope's average and not
            // the envelope.
            level: OnePole::new(0.400, fs),
            nco: Nco::new(MODULATION_RATE, fs),
            // Narrow, because what it has to reject is the comb a phase
            // reversal every 450 ms puts across the whole envelope. That comb
            // has lines every 2.22 Hz and none of them lands on 15.
            re: OnePole::new(0.400, fs),
            im: OnePole::new(0.400, fs),
        }
    }

    pub fn feed(&mut self, x: f64) {
        self.tone.feed(x);
        self.power.process(x.abs());
        let envelope = self.tone.amplitude();
        let mean = self.level.process(envelope);
        // Correlate what is left after the average is taken out. A tone with
        // no modulation leaves nothing here but the ripple of its own
        // detector, which is far above 15 Hz and averages away.
        let (cos, sin) = self.nco.step();
        let ac = envelope - mean;
        self.re.process(ac * cos);
        self.im.process(ac * -sin);
    }

    /// Amplitude of the answering tone.
    pub fn amplitude(&self) -> f64 {
        self.level.value()
    }

    /// Whether a 2100 Hz tone is there at all.
    ///
    /// Loud enough to hear, and standing far enough above the rest of the line
    /// to be the thing on it rather than the skirt of something else.
    pub fn present(&self) -> bool {
        self.level.value() > AUDIBLE
            && self.level.value() > STANDING * self.power.value()
    }

    /// How deeply the envelope is modulated, as a fraction of its average.
    ///
    /// Twice the phasor for the same reason a tone detector doubles its own:
    /// multiplying a real sinusoid by a complex exponential puts half of it at
    /// the sum frequency, where the averaging removes it.
    pub fn depth(&self) -> f64 {
        let (re, im) = (self.re.value(), self.im.value());
        2.0 * re.hypot(im) / self.level.value().max(1.0e-12)
    }

    /// Whether what is on the line is ANSam, and so whether the far end can be
    /// told anything at all.
    pub fn is_ansam(&self) -> bool {
        self.present() && self.depth() > MODULATED
    }

    /// Whether it is the plain answering tone of V.25, which says the far end
    /// does not do V.8 and the call has to be started the old way.
    pub fn is_plain(&self) -> bool {
        self.present() && !self.is_ansam()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    /// Play an answering tone and report what the detector made of it.
    ///
    /// `am` is the modulation depth, `reversal_s` how often the phase turns
    /// over, and `level` how loud it arrives.
    fn listen(am: f64, reversal_s: f64, level: f64, seconds: f64) -> AnswerTone {
        let mut detector = AnswerTone::new(FS);
        let mut phase = 0.0f64;
        for i in 0..(FS * seconds) as usize {
            let t = i as f64 / FS;
            let flips = if reversal_s > 0.0 { (t / reversal_s) as u64 } else { 0 };
            let sign = if flips % 2 == 0 { 1.0 } else { -1.0 };
            let envelope =
                1.0 + am * (std::f64::consts::TAU * MODULATION_RATE * t).sin();
            phase += std::f64::consts::TAU * ANSWER_TONE / FS;
            detector.feed(level * envelope * sign * phase.sin());
        }
        detector
    }

    #[test]
    fn a_plain_answering_tone_is_not_ansam() {
        // V.25's tone: 2100 Hz and nothing else. A far end sending this does
        // not speak V.8, and 7.2 forbids sending it a CM.
        let d = listen(0.0, 0.0, 0.3, 3.0);
        assert!(d.present(), "did not hear the tone at all");
        assert!(d.is_plain(), "a plain tone read as ANSam at depth {:.3}", d.depth());
        assert!(!d.is_ansam());
    }

    #[test]
    fn a_modulated_answering_tone_is_ansam() {
        let d = listen(NOMINAL_DEPTH, 0.0, 0.3, 3.0);
        assert!(d.is_ansam(), "ANSam read as plain at depth {:.3}", d.depth());
    }

    #[test]
    fn the_depth_measured_is_the_depth_sent() {
        // 7.2 asks for an envelope between 0.8 and 1.2 of its average, which
        // is a depth of a fifth. Measuring it rather than merely deciding on
        // it is what makes the threshold something to reason about.
        let d = listen(NOMINAL_DEPTH, 0.0, 0.3, 3.0);
        assert!(
            (d.depth() - NOMINAL_DEPTH).abs() < 0.03,
            "measured {:.3} against {NOMINAL_DEPTH}",
            d.depth()
        );
    }

    #[test]
    fn the_reversals_do_not_decide_it_either_way() {
        // The trap. Reversals are the obvious thing to look for and the wrong
        // one: 7.2 has them only when echo cancellers need disabling, so a
        // tone can be ANSam without them and can carry them without being
        // ANSam. Both mistakes are tested here.
        let modulated_no_reversals = listen(NOMINAL_DEPTH, 0.0, 0.3, 3.0);
        assert!(modulated_no_reversals.is_ansam(), "ANSam needs no reversals");

        let reversals_no_modulation = listen(0.0, 0.450, 0.3, 3.0);
        assert!(
            reversals_no_modulation.is_plain(),
            "reversals alone read as ANSam at depth {:.3}",
            reversals_no_modulation.depth()
        );
    }

    #[test]
    fn ansam_with_reversals_is_still_ansam() {
        // What a modem on a line with echo cancellers actually sends, and the
        // case a detector looking at the envelope has to survive: a reversal
        // takes the envelope to nothing twice a second.
        let d = listen(NOMINAL_DEPTH, 0.450, 0.3, 3.0);
        assert!(d.is_ansam(), "read as plain at depth {:.3}", d.depth());
    }

    #[test]
    fn it_works_across_the_levels_a_network_delivers() {
        // Some 34 dB between a short call and a long one, and the decision is
        // a ratio, so none of it should matter until the tone is too quiet to
        // hear at all.
        for db in [0.0, -10.0, -20.0, -30.0] {
            let level = 0.3 * 10.0f64.powf(db / 20.0);
            let ansam = listen(NOMINAL_DEPTH, 0.450, level, 3.0);
            assert!(ansam.is_ansam(), "ANSam missed at {db} dB");
            let plain = listen(0.0, 0.450, level, 3.0);
            assert!(plain.is_plain(), "plain tone read as ANSam at {db} dB");
        }
    }

    #[test]
    fn a_quiet_line_is_neither() {
        let d = listen(NOMINAL_DEPTH, 0.450, 0.0, 2.0);
        assert!(!d.present());
        assert!(!d.is_ansam());
        assert!(!d.is_plain());
    }

    #[test]
    fn something_that_is_not_the_answering_tone_is_not_heard_as_one() {
        // A modem's own data, or the far end's, is not a 2100 Hz tone and must
        // not read as one: what follows a decision here is either a V.8
        // negotiation or a modem start-up, and there is no going back.
        let mut d = AnswerTone::new(FS);
        let mut phase = 0.0f64;
        for i in 0..(FS * 3.0) as usize {
            // 1800 Hz, which is where V.32 puts its carrier.
            phase += std::f64::consts::TAU * 1800.0 / FS;
            let _ = i;
            d.feed(0.3 * phase.sin());
        }
        assert!(!d.present(), "heard 1800 Hz as an answering tone");
    }
}
