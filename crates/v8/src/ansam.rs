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

/// How long the tone is averaged over to say whether it is there now, in
/// seconds.
///
/// Shorter than the 50 ms the rest of the line is averaged over, and that is
/// the point of it. On digital silence -- a call being picked up, a packet the
/// network lost -- every average decays towards nothing at its own rate, so a
/// ratio of two of them ends up wherever their time constants send it. The
/// tone's 0.4 s average against the line's 50 ms one grows without limit, and
/// silence stands above itself: on `live-1790039606` a noisy line that went to
/// exact zeros as the call was picked up read as an answering tone for 40 ms,
/// and the calling modem gave up on V.8 two seconds before the far end sent
/// one. Averaged faster than the line, a tone that has stopped falls under it
/// within about ten milliseconds whatever was on the line before.
///
/// Not so fast that a phase reversal takes the tone away. The tone detector's
/// phasor passes through nothing for a few milliseconds at each one; averaged
/// over this, a reversal at the bottom of ANSam's envelope still leaves the
/// tone 1.4 times the floor it has to clear.
const NOW: f64 = 0.010;

/// Watches for an answering tone and says which of the two it is.
#[derive(Debug)]
pub struct AnswerTone {
    tone: ToneDetector,
    /// Everything on the line, to weigh the tone against.
    power: OnePole,
    /// Average of the tone's amplitude: the carrier of the envelope.
    level: OnePole,
    /// The tone's amplitude over the last few milliseconds: whether it is
    /// there now, rather than whether it has been.
    now: OnePole,
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
            now: OnePole::new(NOW, fs),
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
        self.now.process(envelope);
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
    /// to be the thing on it rather than the skirt of something else -- both
    /// over the last few milliseconds, which says it is there now, and over
    /// the 0.4 s the depth is measured against, which says it has been there
    /// long enough for the depth to mean something. Either alone is wrong
    /// somewhere. The short one is satisfied by the first moments of a tone,
    /// when the envelope has just stepped up from nothing and the correlator
    /// is reading the step. The long one is satisfied by silence, for the
    /// reason given on `NOW`.
    pub fn present(&self) -> bool {
        let floor = STANDING * self.power.value();
        self.level.value() > AUDIBLE
            && self.level.value() > floor
            && self.now.value() > AUDIBLE
            && self.now.value() > floor
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

    /// One sample of an answering tone, `t` seconds into it.
    fn answering_tone(am: f64, reversal_s: f64, level: f64, t: f64) -> f64 {
        let flips = if reversal_s > 0.0 { (t / reversal_s) as u64 } else { 0 };
        let sign = if flips % 2 == 0 { 1.0 } else { -1.0 };
        let envelope = 1.0 + am * (std::f64::consts::TAU * MODULATION_RATE * t).sin();
        level * envelope * sign * (std::f64::consts::TAU * ANSWER_TONE * t).sin()
    }

    /// The gain of a tone that stops at `stop`, cut off or faded out over
    /// `fade` seconds.
    fn ending(t: f64, stop: f64, fade: f64) -> f64 {
        if t < stop {
            1.0
        } else if fade > 0.0 {
            (1.0 - (t - stop) / fade).max(0.0)
        } else {
            0.0
        }
    }

    /// Flat, deterministic noise between -1 and 1.
    fn noise() -> impl FnMut() -> f64 {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    #[test]
    fn a_noisy_line_that_falls_silent_is_not_an_answering_tone() {
        // A call being picked up: line noise, and then the exact zeros of a
        // digital network carrying nothing yet. Every average in here decays
        // on silence, each at its own rate, and on `live-1790039606` the ratio
        // of two of them passed for a tone standing clear of the line for
        // 40 ms -- enough for the calling modem to decide that the far end did
        // not do V.8, two seconds before it answered.
        let mut d = AnswerTone::new(FS);
        let mut hiss = noise();
        for _ in 0..(FS * 2.0) as usize {
            d.feed(0.05 * hiss());
            assert!(!d.present(), "heard noise as an answering tone");
        }
        // Loud enough in the detector's band to clear the absolute floor, so
        // that only the comparison with the rest of the line stands between
        // this and a tone.
        assert!(d.amplitude() > AUDIBLE, "the noise was too quiet to test anything");
        for i in 0..(FS * 1.0) as usize {
            d.feed(0.0);
            assert!(
                !d.present(),
                "silence read as a tone {:.3} s after the noise stopped",
                i as f64 / FS
            );
        }
    }

    #[test]
    fn a_tone_that_stops_is_gone_at_once() {
        // The same fault from the other side. The tone's average used to be
        // eight times slower than the line's, so a tone outlived itself by two
        // seconds of silence -- and a calling modem that had not yet made up
        // its mind about it made it up on nothing. Cut off, and faded out over
        // the 50 ms a network's concealment might take over it.
        for fade in [0.0, 0.050] {
            for reversal in [0.0, 0.450] {
                let mut d = AnswerTone::new(FS);
                let stop = 2.0;
                for i in 0..(FS * 3.0) as usize {
                    let t = i as f64 / FS;
                    d.feed(ending(t, stop, fade) * answering_tone(0.0, reversal, 0.3, t));
                    if t > stop + fade + 0.020 {
                        assert!(!d.present(), "a tone {fade} s in fading was still there at {t:.3} s");
                    }
                }
            }
        }
    }

    #[test]
    fn the_end_of_a_plain_tone_is_never_ansam() {
        // The envelope stepping down to nothing is a step, and a step has
        // some of every frequency in it, 15 Hz included. Read as modulation it
        // sends a CM to a modem that has just said, with the tone that
        // stopped, that it has never heard of V.8.
        for fade in [0.0, 0.050] {
            for reversal in [0.0, 0.450] {
                let mut d = AnswerTone::new(FS);
                let stop = 2.0;
                for i in 0..(FS * 3.0) as usize {
                    let t = i as f64 / FS;
                    d.feed(ending(t, stop, fade) * answering_tone(0.0, reversal, 0.3, t));
                    if t > 1.0 {
                        assert!(
                            !d.is_ansam(),
                            "the end of a plain tone faded over {fade} s read as ANSam at \
                             {t:.3} s, depth {:.3}",
                            d.depth()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_reversal_does_not_interrupt_either_tone() {
        // What the calling modem decides it decides on a reading held without
        // a break, so the reading has to be steady through everything a real
        // tone does -- and a reversal takes the tone detector's phasor through
        // nothing twice a second. Were it to take the tone away with it, a
        // hold longer than 450 ms would never be completed at all.
        for db in [0.0, -10.0, -20.0, -30.0] {
            let level = 0.3 * 10.0f64.powf(db / 20.0);
            for reversal in [0.0, 0.450] {
                let mut ansam = AnswerTone::new(FS);
                let mut plain = AnswerTone::new(FS);
                for i in 0..(FS * 3.0) as usize {
                    let t = i as f64 / FS;
                    ansam.feed(answering_tone(NOMINAL_DEPTH, reversal, level, t));
                    plain.feed(answering_tone(0.0, reversal, level, t));
                    if t > 0.6 {
                        assert!(
                            ansam.is_ansam(),
                            "ANSam at {db} dB lost at {t:.3} s, depth {:.3}",
                            ansam.depth()
                        );
                        assert!(plain.is_plain(), "the plain tone at {db} dB lost at {t:.3} s");
                    }
                }
            }
        }
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
