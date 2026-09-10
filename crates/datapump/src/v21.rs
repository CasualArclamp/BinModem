//! V.21 channel 2 and the two fax tones, which is all a fax call needs
//! before the page.
//!
//! T.30 runs its whole conversation over 300 bit/s frequency shift keying in
//! the high channel of V.21: 1650 Hz for a mark and 1850 Hz for a space. The
//! same two tones the V.8 negotiation uses, and the same detector, but the
//! bits are not framed into characters here. A fax sends HDLC, which is
//! synchronous, so what this hands up is a bit at a time and whoever is above
//! it finds the flags.
//!
//! The two single tones belong here too, because they are the first thing on
//! a fax call and they are how the two ends know what kind of call it is. A
//! calling fax sends 1100 Hz in bursts; an answering one sends 2100 Hz
//! steadily. Neither carries any information beyond its own presence.

use dsp::FskDetector;

use crate::bell103::Bell103Tx;

/// V.21 channel 2, as `(space, mark)`.
///
/// The channel T.30 uses in both directions. V.21 gives channel 1 to the
/// calling modem and channel 2 to the answering one, but a fax call is half
/// duplex and only one end speaks at a time, so both ends use the high
/// channel and there is nothing to collide with.
pub const CHANNEL_2: (f64, f64) = (1850.0, 1650.0);

pub const BAUD: f64 = 300.0;

/// The calling tone: 1100 Hz, half a second on and three seconds off
/// (T.30 5.1.1).
///
/// It is what tells a machine that answered a voice call that a fax is
/// waiting, and it is optional -- a fax that never sends it still works
/// against a fax that is expecting one.
pub const CNG: f64 = 1100.0;
/// On and off, in seconds.
pub const CNG_ON: f64 = 0.5;
pub const CNG_OFF: f64 = 3.0;

/// The called tone: 2100 Hz for two and a half to three seconds (T.30 5.1.2).
///
/// The same frequency as V.25's answer tone and V.8's, and told apart from
/// V.8's by what is not in it: V.8 reverses the phase of its tone every 450
/// ms and a fax does not. A recording of a real fax machine answering settles
/// it -- there are no reversals in there at all, which is why a fax call
/// never reaches the V.8 negotiation.
pub const CED: f64 = 2100.0;
pub const CED_SECONDS: f64 = 3.0;

/// A single tone, for CNG and CED.
#[derive(Debug, Clone)]
pub struct Tone {
    phase: f64,
    step: f64,
    amplitude: f64,
}

impl Tone {
    pub fn new(hz: f64, fs: f64) -> Self {
        Self {
            phase: 0.0,
            step: std::f64::consts::TAU * hz / fs,
            // The same level everything else here goes out at.
            amplitude: 0.35,
        }
    }

    pub fn next_sample(&mut self) -> f64 {
        let v = self.amplitude * self.phase.sin();
        self.phase += self.step;
        if self.phase > std::f64::consts::TAU {
            self.phase -= std::f64::consts::TAU;
        }
        v
    }
}

/// Sends bits down V.21 channel 2.
///
/// A thin wrapper on the 300 bit/s transmitter that already exists, with the
/// tones fixed and the start-stop framing left out: what goes in is bits and
/// what comes out is those bits, because HDLC does its own framing.
#[derive(Debug)]
pub struct Sender {
    tx: Bell103Tx,
}

impl Sender {
    pub fn new(fs: f64) -> Self {
        let (space, mark) = CHANNEL_2;
        let mut tx = Bell103Tx::with_tones(space, mark, fs);
        tx.set_transmitting(false);
        Self { tx }
    }

    pub fn set_transmitting(&mut self, on: bool) {
        self.tx.set_transmitting(on);
    }

    pub fn is_transmitting(&self) -> bool {
        self.tx.is_transmitting()
    }

    pub fn push_bits(&mut self, bits: &[bool]) {
        self.tx.push_bits(bits);
    }

    pub fn pending_bits(&self) -> usize {
        self.tx.pending_bits()
    }

    pub fn next_sample(&mut self) -> f64 {
        self.tx.next_sample()
    }
}

/// Recovers bits from V.21 channel 2.
///
/// Synchronous, unlike the start-stop receiver next door: there are no start
/// bits to re-acquire on, so the clock is set going by the first transition
/// after the carrier appears and then held to every transition after that.
///
/// Holding it matters more than it looks. A burst opens with a second of
/// flags, which is three hundred bits, and the frame that matters comes
/// after all of them -- so a clock left free-running has drifted for a second
/// before it reads anything. On a real call to a real fax that cost the
/// identification frame: the first six octets came off correctly and the rest
/// was rubbish, and the frame check caught it, which is the good outcome of a
/// bad situation. The far end's own bit rate is exact and its transitions say
/// where it is; there is no reason to ignore them.
#[derive(Debug)]
pub struct Receiver {
    detector: FskDetector,
    sps: f64,
    /// Samples until the middle of the next bit, once running.
    countdown: f64,
    running: bool,
    last: f64,
    /// The level at the last bit centre, for a scope to draw.
    level: f64,
}

/// How much of the error each transition takes out of the clock.
///
/// A transition says where the middle of the next bit should be, and the
/// clock is nudged an eighth of the way there rather than jumped: a
/// transition arrives with the noise of one bit on it, and a clock that
/// believes any single one of them follows the noise instead of the far end.
const PULL: f64 = 0.125;

impl Receiver {
    pub fn new(fs: f64) -> Self {
        let (space, mark) = CHANNEL_2;
        Self {
            detector: FskDetector::new(space, mark, BAUD, fs),
            sps: fs / BAUD,
            countdown: 0.0,
            running: false,
            last: 0.0,
            level: 0.0,
        }
    }

    /// Feed one sample; yields a bit when one is due.
    pub fn feed(&mut self, sample: f64) -> Option<bool> {
        let level = self.detector.feed(sample);
        let carrier = self.detector.carrier();
        if !carrier {
            self.running = false;
            self.last = level;
            return None;
        }
        // Start the clock half a bit after the first crossing there is, so
        // the first sample lands in the middle of a bit rather than on its
        // edge. Nothing is lost by starting late: a frame begins with flags,
        // and there are always more flags than one.
        if !self.running {
            if (level > 0.0) == (self.last > 0.0) {
                self.last = level;
                return None;
            }
            self.running = true;
            self.countdown = self.sps / 2.0;
        }
        // Every transition is a bit boundary, so the next sample is due
        // half a bit after it. Nudge rather than jump.
        if (level > 0.0) != (self.last > 0.0) {
            let want = self.sps / 2.0;
            self.countdown += PULL * (want - self.countdown);
        }
        self.last = level;
        self.countdown -= 1.0;
        if self.countdown > 0.0 {
            return None;
        }
        self.countdown += self.sps;
        self.level = level;
        // A mark is a one. The detector is positive towards the mark tone.
        Some(level > 0.0)
    }

    pub fn carrier(&self) -> bool {
        self.detector.carrier()
    }

    /// Decision margin at the last bit, which is what a scope plots.
    pub fn level(&self) -> f64 {
        self.level
    }
}

/// Hears a single tone, for telling CED from silence and CNG from a voice.
#[derive(Debug)]
pub struct ToneDetector {
    inner: dsp::ToneDetector,
    level: dsp::filter::OnePole,
}

impl ToneDetector {
    pub fn new(hz: f64, fs: f64) -> Self {
        Self {
            // Wide enough for the 2100 +/- 15 Hz T.30 allows and no wider.
            inner: dsp::ToneDetector::new(hz, 40.0, fs),
            level: dsp::filter::OnePole::new(0.030, fs),
        }
    }

    pub fn feed(&mut self, sample: f64) {
        self.inner.feed(sample);
        self.level.process(self.inner.amplitude());
    }

    pub fn amplitude(&self) -> f64 {
        self.level.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    #[test]
    fn the_channel_is_the_high_one() {
        // T.30 5.3.2: "binary 1 (mark) 1650 Hz, binary 0 (space) 1850 Hz".
        assert_eq!(CHANNEL_2, (1850.0, 1650.0));
    }

    #[test]
    fn bits_go_out_and_come_back() {
        let mut tx = Sender::new(FS);
        let mut rx = Receiver::new(FS);
        // Flags first, as a frame would begin, then something with runs in it
        // so a stuck bit clock shows up.
        let mut sent: Vec<bool> = Vec::new();
        for _ in 0..8 {
            sent.extend([false, true, true, true, true, true, true, false]);
        }
        for i in 0..64 {
            sent.push(i % 5 == 0);
        }
        tx.set_transmitting(true);
        tx.push_bits(&sent);

        let mut got = Vec::new();
        for _ in 0..(FS * 0.8) as usize {
            let s = tx.next_sample();
            if let Some(bit) = rx.feed(s) {
                got.push(bit);
            }
        }
        // Everything sent has to come back, in order and together. Not the
        // whole of what came back, though: the carrier coming up puts a bit
        // or two of nothing at the front, and the transmitter idles at a mark
        // once the queue is empty, which is a tail of ones. What is being
        // asserted is that the bits handed in appear unbroken somewhere in
        // between.
        assert!(
            got.windows(sent.len()).any(|w| w == sent.as_slice()),
            "what was sent is not in what came back: {} bits in, {} out",
            sent.len(),
            got.len()
        );
    }

    #[test]
    fn the_clock_holds_over_a_burst_as_long_as_a_real_one() {
        // A second of flags and then a frame, which is what a fax sends and
        // what a free-running clock loses the end of.
        let mut tx = Sender::new(FS);
        let mut rx = Receiver::new(FS);
        let mut sent: Vec<bool> = Vec::new();
        for _ in 0..37 {
            sent.extend([false, true, true, true, true, true, true, false]);
        }
        // Twenty octets of an identification field, sent low bit first as
        // HDLC does, with the runs of ones that stuffing would break up.
        for octet in b"       909 863  0031" {
            for i in 0..8 {
                sent.push(octet >> i & 1 == 1);
            }
        }
        tx.set_transmitting(true);
        tx.push_bits(&sent);
        let mut got = Vec::new();
        for _ in 0..(FS * 2.5) as usize {
            let s = tx.next_sample();
            if let Some(bit) = rx.feed(s) {
                got.push(bit);
            }
        }
        assert!(
            got.windows(sent.len()).any(|w| w == sent.as_slice()),
            "the frame after a second of flags did not survive: {} bits in,              {} out",
            sent.len(),
            got.len()
        );
    }

    #[test]
    fn silence_yields_no_bits() {
        let mut rx = Receiver::new(FS);
        let mut any = false;
        for _ in 0..(FS as usize) {
            any |= rx.feed(0.0).is_some();
        }
        assert!(!any, "made bits out of nothing");
        assert!(!rx.carrier());
    }

    #[test]
    fn the_called_tone_is_heard_and_the_calling_one_is_not_mistaken_for_it() {
        // The two are a thousand hertz apart and both are single tones, so a
        // detector that is not narrow enough answers to both.
        let mut ced = Tone::new(CED, FS);
        let mut hear_ced = ToneDetector::new(CED, FS);
        let mut hear_cng = ToneDetector::new(CNG, FS);
        for _ in 0..(FS * 0.3) as usize {
            let s = ced.next_sample();
            hear_ced.feed(s);
            hear_cng.feed(s);
        }
        assert!(
            hear_ced.amplitude() > 0.2,
            "did not hear the called tone: {}",
            hear_ced.amplitude()
        );
        assert!(
            hear_cng.amplitude() < hear_ced.amplitude() / 20.0,
            "heard the calling tone in the called one: {} against {}",
            hear_cng.amplitude(),
            hear_ced.amplitude()
        );
    }

    #[test]
    fn a_transmitter_that_is_not_transmitting_puts_nothing_on_the_line() {
        let mut tx = Sender::new(FS);
        tx.push_bits(&[true, false, true]);
        let loudest = (0..1000).fold(0.0f64, |m, _| m.max(tx.next_sample().abs()));
        assert_eq!(loudest, 0.0);
    }
}
