//! V.27 ter: 4800 and 2400 bit/s, differentially encoded phase.
//!
//! The modulation every group 3 fax machine must have. A fax call agrees its
//! capabilities over 300 bit/s and then drops that carrier and raises this one
//! to send the page, so this is the half of a fax that carries the picture.
//!
//! There is no rate negotiation, no trellis, no echo canceller and no second
//! station talking at the same time: the line is half duplex and turns around
//! between messages, so a burst is a training sequence, then data, then
//! silence. What makes that work is the training, every symbol of which is
//! known before it arrives, and which is long enough to solve an equaliser
//! for from nothing -- or, the short one, to find a burst again with the one
//! a long one left.
//!
//! Two rates, differing only in how many bits ride on each symbol and how fast
//! the symbols go: three bits on eight phases at 1600 baud, or two bits on
//! four phases at 1200 baud. The carrier, the shaping, the scrambler and every
//! training segment are shared.
//!
//! The transmitter is here. The receiver ([`Receiver`]) is a driver on the
//! shared QAM core, `dsp::qam`, in `v27ter/receiver.rs`, and finds each burst
//! by its turn-on sequence in `v27ter/hunt.rs`.

mod hunt;
mod receiver;

pub use receiver::Receiver;

use dsp::{Nco, rrc_at};

/// 2.1: "The carrier frequency is to be 1800 +/- 1 Hz."
pub const CARRIER: f64 = 1800.0;

/// 2.1.1: fifty per cent raised cosine, "equally divided between the receiver
/// and transmitter", which is a root raised cosine of that roll-off at each
/// end.
pub const ROLLOFF: f64 = 0.5;

/// Symbols either side of centre that the shaping pulse reaches.
pub const SPAN: usize = 6;

/// Which of the two rates is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rate {
    /// 4800 bit/s: tribits on eight phases at 1600 baud (2.3).
    #[default]
    R4800,
    /// 2400 bit/s: dibits on four phases at 1200 baud (2.4). The fall-back,
    /// and where a fax goes when the line will not carry more.
    R2400,
}

impl Rate {
    pub fn baud(self) -> f64 {
        match self {
            Self::R4800 => 1600.0,
            Self::R2400 => 1200.0,
        }
    }

    /// Bits to the symbol: a tribit or a dibit.
    pub fn bits(self) -> usize {
        match self {
            Self::R4800 => 3,
            Self::R2400 => 2,
        }
    }

    pub fn bits_per_second(self) -> u32 {
        match self {
            Self::R4800 => 4800,
            Self::R2400 => 2400,
        }
    }

    /// How many of the eight phases this rate uses.
    pub fn phases(self) -> u8 {
        match self {
            Self::R4800 => 8,
            Self::R2400 => 4,
        }
    }
}

/// Which turn-on sequence to send (2.5.1).
///
/// The long one teaches an equaliser a line it has never seen; the short one
/// refreshes what it already knows. T.30 leaves the choice to the sender for
/// V.27 ter, and a fax turns the line around between every message, so this
/// sends the long one and the far end never has to remember anything across a
/// turnaround.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Training {
    Short,
    #[default]
    Long,
}

impl Training {
    /// Segment 3: continuous 180 degree reversals, for clock acquisition
    /// (2.5.1.1).
    pub fn reversals(self) -> u32 {
        match self {
            Self::Short => 14,
            Self::Long => 50,
        }
    }

    /// Segment 4: the two-phase equaliser conditioning pattern (2.5.1.2).
    pub fn conditioning(self) -> u32 {
        match self {
            Self::Short => 58,
            Self::Long => 1074,
        }
    }

    /// Every symbol of the turn-on sequence, without the echo protection of
    /// segments 1 and 2.
    ///
    /// Table 3 gives the totals as 50 ms and 708 ms at 4800, and 66 ms and
    /// 943 ms at 2400. Those are these counts over the two baud rates.
    pub fn symbols(self) -> u32 {
        self.reversals() + self.conditioning() + SCRAMBLED_ONES
    }
}

/// Segment 5: continuous scrambled ONEs, eight symbols (2.5.1.3).
pub const SCRAMBLED_ONES: u32 = 8;

/// Table 1: a tribit's phase change, in eighths of a turn.
///
/// Indexed by the tribit read as a binary number, the left-hand digit being
/// the one that entered the modulator first.
pub(crate) const TRIBIT_TURN: [u8; 8] = [1, 0, 2, 3, 6, 7, 5, 4];

/// Table 1 backwards: eighths of a turn to the tribit that asked for it.
pub(crate) const TURN_TRIBIT: [u8; 8] = [0b001, 0b000, 0b010, 0b011, 0b111, 0b110, 0b100, 0b101];

/// Table 2: a dibit's phase change, in eighths of a turn.
///
/// The same units as the tribit table so one modulator serves both, which is
/// why these run 0, 2, 6, 4 rather than 0, 1, 3, 2.
const DIBIT_TURN: [u8; 4] = [0, 2, 6, 4];

/// Table 2 backwards, indexed by quarters of a turn.
const TURN_DIBIT: [u8; 4] = [0b00, 0b01, 0b11, 0b10];

/// The scrambler's seven stages at the start of a turn-on sequence.
///
/// Appendix I: "the first seven stages of the scrambler should be loaded with
/// 0011110 (right-hand-most first in time)". Earliest first, that is this.
/// Loading it and then holding the input at ONE produces exactly the
/// pseudo-random sequence Table 4 prints, at both ends and at both lengths.
const TRAINING_SEED: [bool; 7] = [false, true, true, true, true, false, false];

/// The self-synchronizing scrambler of clause 9: `1 + x^-6 + x^-7`.
///
/// The recommendation asks for guards against repeating patterns of 1, 2, 3,
/// 4, 6, 8, 9 and 12 bits on top of this. They are left out. They exist to
/// stop a pathological input putting a repeating pattern on the line, and
/// nothing a fax sends is pathological: the training is fixed, the training
/// check is zeros through a divider, which is a maximal-length sequence, and
/// the page is compressed. The far end's descrambler is multiplicative and
/// recovers from any state within seven bits, so leaving them out cannot
/// desynchronize anybody.
#[derive(Debug, Clone, Default)]
pub struct Scrambler {
    /// The last seven bits through the register, most recent in bit 0.
    history: u8,
}

impl Scrambler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the register for a turn-on sequence.
    pub fn seeded() -> Self {
        let mut me = Self::default();
        for &bit in &TRAINING_SEED {
            me.push(bit);
        }
        me
    }

    fn taps(&self) -> bool {
        // x^-6 and x^-7 are the sixth and seventh most recent bits.
        (self.history >> 5) & 1 != (self.history >> 6) & 1
    }

    fn push(&mut self, bit: bool) {
        self.history = ((self.history << 1) | u8::from(bit)) & 0x7f;
    }

    /// Divide by the generating polynomial: the register holds what went out.
    pub fn scramble(&mut self, bit: bool) -> bool {
        let out = bit ^ self.taps();
        self.push(out);
        out
    }

    /// Multiply by it again: the register holds what came in.
    pub fn descramble(&mut self, bit: bool) -> bool {
        let out = bit ^ self.taps();
        self.push(bit);
        out
    }

    pub fn reset(&mut self) {
        self.history = 0;
    }
}

/// The first `count` phase changes of segment 4, as a receiver sees them: one
/// for a symbol that did not change and minus one for a reversal, which is
/// the real part of each symbol times the conjugate of the one before.
///
/// 2.5.1.2: "every third bit of the pseudo-random sequence", a ZERO for
/// 0 degrees and a ONE for 180, from the scrambler loaded as Appendix I asks
/// and fed ONEs -- the same register the transmitter runs, so the two cannot
/// disagree.
fn conditioning_changes(count: usize) -> Vec<f64> {
    let mut scrambler = Scrambler::seeded();
    (0..count)
        .map(|_| {
            let bit = scrambler.scramble(true);
            scrambler.scramble(true);
            scrambler.scramble(true);
            if bit { -1.0 } else { 1.0 }
        })
        .collect()
}

/// Where a burst has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Off the line.
    Silent,
    /// Segment 1, with protection against talker echo: unmodulated carrier.
    Unmodulated(u32),
    /// Segment 2, the same: no transmitted energy.
    Gap(u32),
    /// Segment 3.
    Reversals(u32),
    /// Segment 4.
    Conditioning(u32),
    /// Segment 5.
    Ones(u32),
    /// Data, until whoever is above says to stop.
    Data,
    /// The turn-off sequence: scrambled ones for 5 to 10 ms, then nothing
    /// (Table 5). Counted in symbols.
    TurnOff(u32),
}

/// Table 3's segment 1 with protection against talker echo: "185 ms to 200
/// ms" of unmodulated carrier. The middle of that.
const UNMODULATED_SECONDS: f64 = 0.1925;

/// Segment 2: "20 ms to 25 ms" of no transmitted energy.
const GAP_SECONDS: f64 = 0.0225;

/// A duration as whole symbols at a rate.
fn symbols_in(seconds: f64, rate: Rate) -> u32 {
    (seconds * rate.baud()).round() as u32
}

/// Segment A of the turn-off sequence, in symbols.
///
/// Table 5 asks for 5 to 10 ms of scrambled ones after the last data bit. Ten
/// symbols is 6.3 ms at 1600 baud and 8.3 ms at 1200, inside the window at
/// both rates.
const TURN_OFF_SYMBOLS: u32 = 10;

/// The level a symbol goes out at.
///
/// Every transmitter in this modem leaves at a root mean square of 0.707, and
/// the peak is allowed to go where the shaping puts it -- close to 2 for the
/// crowded V.32 constellations, and about 1.5 here. That is the convention
/// V.2 asks for as well: a transmit level is a power, and a shaped signal and
/// a constant-envelope one with the same power do not have the same peak.
///
/// Getting it wrong here was worth nine decibels. Every V.21 burst in a fax
/// call arrived three times louder than the V.27 ter burst next to it, so the
/// far end had a nine decibel step to chase at every single turnaround.
const LEVEL: f64 = 1.0;

/// V.27 ter transmitter.
#[derive(Debug)]
pub struct Transmitter {
    fs: f64,
    rate: Rate,
    training: Training,
    nco: Nco,
    scrambler: Scrambler,
    stage: Stage,
    /// Whether segments 1 and 2 go in front of each burst.
    echo_protection: bool,
    /// The symbol most recently put on the line.
    sent: (f64, f64),
    /// The phase the last symbol went out at, in eighths of a turn. Every
    /// symbol is a change from this one, which is what differential encoding
    /// means.
    eighths: u8,
    /// Symbols still contributing to the shaping pulse, oldest first.
    history: Vec<(f64, f64)>,
    /// Position within the current symbol period, in symbols.
    phase: f64,
    pending: Vec<bool>,
}

impl Transmitter {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            rate: Rate::default(),
            training: Training::default(),
            nco: Nco::new(CARRIER, fs),
            scrambler: Scrambler::new(),
            stage: Stage::Silent,
            echo_protection: false,
            sent: (0.0, 0.0),
            eighths: 0,
            history: vec![(0.0, 0.0); 2 * SPAN + 1],
            phase: 0.0,
            pending: Vec::new(),
        }
    }

    /// Raise the carrier and begin the turn-on sequence.
    ///
    /// Segments 1 and 2, unmodulated carrier and then a gap to turn echo
    /// suppressors around, are sent only if asked for. Table 3 makes them
    /// optional, and on a fax call the far end has just stopped talking, so
    /// the suppressors are already pointing this way.
    pub fn start(&mut self, rate: Rate, training: Training) {
        self.rate = rate;
        self.training = training;
        self.scrambler = Scrambler::seeded();
        self.stage = if self.echo_protection {
            Stage::Unmodulated(symbols_in(UNMODULATED_SECONDS, rate))
        } else {
            Stage::Reversals(training.reversals())
        };
        self.eighths = 0;
        self.phase = 0.0;
        self.history.fill((0.0, 0.0));
        self.pending.clear();
    }

    /// Send segments 1 and 2 in front of every burst from now on.
    ///
    /// Not what this modem does by default, and nothing a fax needs from it.
    /// It is what real machines send, though -- a public fax service sends it
    /// in front of every training check -- and a receiver that has only ever
    /// heard its own transmitter has never heard a carrier that comes up, goes
    /// away for twenty milliseconds, and comes back.
    pub fn set_echo_protection(&mut self, on: bool) {
        self.echo_protection = on;
    }

    /// Finish the burst: whatever is queued, then scrambled ones, then off.
    ///
    /// Asking twice is not asking for twice as long. Whoever is above cannot
    /// see symbol boundaries and will call this on every sample until the
    /// carrier goes, so a second call has to be nothing.
    pub fn stop(&mut self) {
        if !matches!(self.stage, Stage::Silent | Stage::TurnOff(_)) {
            self.stage = Stage::TurnOff(TURN_OFF_SYMBOLS);
        }
    }

    /// Drop the carrier now, without a turn-off sequence.
    pub fn abort(&mut self) {
        self.stage = Stage::Silent;
        self.pending.clear();
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// The point most recently sent, for a constellation display, or `None`
    /// while nothing is going out.
    ///
    /// A fax is half duplex, so while this end is sending there is nothing
    /// arriving to draw -- and what is going out is the one constellation on
    /// the line.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        (self.is_transmitting() && self.sent != (0.0, 0.0)).then_some(self.sent)
    }

    pub fn is_transmitting(&self) -> bool {
        self.stage != Stage::Silent
    }

    /// Whether the turn-on sequence is over and data is going out.
    pub fn trained(&self) -> bool {
        matches!(self.stage, Stage::Data | Stage::TurnOff(_))
    }

    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend_from_slice(bits);
    }

    /// Push octets most significant bit first.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            for i in (0..8).rev() {
                self.pending.push(byte >> i & 1 != 0);
            }
        }
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }

    /// The next scrambled bit, filling with ONEs when nothing is queued.
    fn next_bit(&mut self) -> bool {
        let bit = if self.pending.is_empty() {
            true
        } else {
            self.pending.remove(0)
        };
        self.scrambler.scramble(bit)
    }

    /// A scrambled ONE, leaving anything queued where it is.
    ///
    /// The whole turn-on sequence runs on these. 2.5.1.3 puts the moment the
    /// queue is first read at the very end of it: "At the end of Segment 5 ...
    /// user data are applied to the input of the data scrambler." Reading the
    /// queue any earlier throws away real data -- segment 4 alone would eat
    /// 3222 bits of it, which at 4800 bit/s is two thirds of a second of the
    /// page.
    fn training_bit(&mut self) -> bool {
        self.scrambler.scramble(true)
    }

    /// Turn by the given eighths and return the point that lands on.
    fn turn(&mut self, eighths: u8) -> (f64, f64) {
        self.eighths = (self.eighths + eighths) & 7;
        let angle = std::f64::consts::TAU * f64::from(self.eighths) / 8.0;
        (angle.cos(), angle.sin())
    }

    fn next_symbol(&mut self) -> (f64, f64) {
        match self.stage {
            Stage::Silent => (0.0, 0.0),
            Stage::Unmodulated(left) => {
                self.stage = if left > 1 {
                    Stage::Unmodulated(left - 1)
                } else {
                    Stage::Gap(symbols_in(GAP_SECONDS, self.rate))
                };
                // The carrier at the reference phase, turned by nothing.
                self.turn(0)
            }
            Stage::Gap(left) => {
                self.stage = if left > 1 {
                    Stage::Gap(left - 1)
                } else {
                    Stage::Reversals(self.training.reversals())
                };
                (0.0, 0.0)
            }
            Stage::Reversals(left) => {
                self.stage = if left > 1 {
                    Stage::Reversals(left - 1)
                } else {
                    Stage::Conditioning(self.training.conditioning())
                };
                self.turn(4)
            }
            Stage::Conditioning(left) => {
                self.stage = if left > 1 {
                    Stage::Conditioning(left - 1)
                } else {
                    Stage::Ones(SCRAMBLED_ONES)
                };
                // 2.5.1.2: every third bit of the sequence the scrambler makes
                // from continuous ONEs, a ZERO meaning no phase change and a
                // ONE meaning a reversal. The other two are generated and
                // thrown away, which keeps the register running at three bits
                // to the symbol right through segment 4 and hands segment 5
                // the state Table 4 prints.
                let bit = self.training_bit();
                self.training_bit();
                self.training_bit();
                self.turn(if bit { 4 } else { 0 })
            }
            Stage::Ones(left) => {
                self.stage = if left > 1 {
                    Stage::Ones(left - 1)
                } else {
                    Stage::Data
                };
                self.symbol(true)
            }
            Stage::Data => self.symbol(false),
            Stage::TurnOff(left) => {
                // Table 5: "Remaining data followed by continuous scrambled
                // ONEs". Anything still queued goes out first, and the count
                // only starts once there is nothing left but ones.
                if !self.pending.is_empty() {
                    return self.symbol(false);
                }
                self.stage = if left > 1 {
                    Stage::TurnOff(left - 1)
                } else {
                    Stage::Silent
                };
                self.symbol(true)
            }
        }
    }

    /// One symbol's worth of bits, encoded as a phase change.
    ///
    /// `ones` chooses the source: the queue, or the continuous ONEs the
    /// training and the turn-off run on.
    fn symbol(&mut self, ones: bool) -> (f64, f64) {
        let bit = |me: &mut Self| {
            if ones {
                me.training_bit()
            } else {
                me.next_bit()
            }
        };
        let eighths = match self.rate {
            Rate::R4800 => {
                let a = bit(self);
                let b = bit(self);
                let c = bit(self);
                TRIBIT_TURN[usize::from(a) << 2 | usize::from(b) << 1 | usize::from(c)]
            }
            Rate::R2400 => {
                let a = bit(self);
                let b = bit(self);
                DIBIT_TURN[usize::from(a) << 1 | usize::from(b)]
            }
        };
        self.turn(eighths)
    }

    pub fn next_sample(&mut self) -> f64 {
        if self.stage == Stage::Silent {
            // The carrier goes on running off the line, so a burst that
            // follows starts from a continuous phase rather than a step.
            self.nco.step();
            return 0.0;
        }
        self.phase += self.rate.baud() / self.fs;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol();
            self.history.push(symbol);
            self.sent = symbol;
        }

        let centre = SPAN as f64;
        let mut baseband = (0.0, 0.0);
        for (i, &(re, im)) in self.history.iter().enumerate() {
            let offset = self.phase + centre - i as f64;
            let tap = rrc_at(offset, ROLLOFF);
            baseband.0 += re * tap;
            baseband.1 += im * tap;
        }

        let (cos, sin) = self.nco.step();
        LEVEL * (baseband.0 * cos - baseband.1 * sin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    /// Run a burst through a transmitter and back out of a receiver.
    fn loopback(rate: Rate, training: Training, data: &[u8]) -> Vec<bool> {
        let mut tx = Transmitter::new(FS);
        let mut rx = Receiver::new(FS);
        rx.set_rate(rate);
        tx.start(rate, training);
        tx.push_bytes(data);
        let mut out = Vec::new();
        // Long enough for the training, the data, the turn-off and the delay
        // through both filters.
        let samples = (FS * 3.0) as usize
            + (data.len() * 8) * (FS / rate.bits_per_second() as f64) as usize;
        for _ in 0..samples {
            if tx.trained() && tx.pending_bits() == 0 {
                tx.stop();
            }
            let sample = tx.next_sample();
            rx.feed(sample);
            out.extend(rx.take_bits());
            if !tx.is_transmitting() && !rx.carrier() {
                break;
            }
        }
        out
    }

    /// Find `needle` in `haystack`, as a run of bits.
    fn find(haystack: &[bool], needle: &[bool]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn bits_of(bytes: &[u8]) -> Vec<bool> {
        let mut bits = Vec::new();
        for &byte in bytes {
            for i in (0..8).rev() {
                bits.push(byte >> i & 1 != 0);
            }
        }
        bits
    }


    #[test]
    fn the_training_sequence_is_the_length_table_3_gives() {
        // 708 ms at 4800 and 943 ms at 2400, long; 50 and 66 ms, short.
        let cases = [
            (Rate::R4800, Training::Long, 708.0),
            (Rate::R2400, Training::Long, 943.0),
            (Rate::R4800, Training::Short, 50.0),
            (Rate::R2400, Training::Short, 66.0),
        ];
        for (rate, training, want_ms) in cases {
            let ms = 1000.0 * f64::from(training.symbols()) / rate.baud();
            assert!(
                (ms - want_ms).abs() < 1.0,
                "{rate:?} {training:?} trains for {ms:.0} ms, Table 3 says {want_ms:.0}"
            );
        }
    }

    #[test]
    fn with_echo_protection_the_turn_on_is_as_long_as_table_3_says() {
        // 923 ms at 4800 and 1158 ms at 2400 for the long sequence, which is
        // segments 1 and 2 in front of the 708 and 943 without them. The
        // table's figures are nominal and segments 1 and 2 are ranges, so
        // within a few milliseconds.
        for (rate, want_ms) in [(Rate::R4800, 923.0), (Rate::R2400, 1158.0)] {
            let mut tx = Transmitter::new(FS);
            tx.set_echo_protection(true);
            tx.start(rate, Training::Long);
            let mut samples = 0usize;
            while !tx.trained() {
                tx.next_sample();
                samples += 1;
                assert!(samples < FS as usize * 2, "the training never ended");
            }
            let ms = 1000.0 * samples as f64 / FS;
            assert!(
                (ms - want_ms).abs() < 8.0,
                "{rate:?} took {ms:.0} ms to train, Table 3 says {want_ms:.0}"
            );
        }
    }

    #[test]
    fn with_echo_protection_there_is_a_carrier_then_a_silence_then_the_training() {
        let mut tx = Transmitter::new(FS);
        tx.set_echo_protection(true);
        tx.start(Rate::R4800, Training::Long);
        let power = |tx: &mut Transmitter, seconds: f64| -> f64 {
            let n = (FS * seconds) as usize;
            (0..n).map(|_| tx.next_sample().powi(2)).sum::<f64>() / n as f64
        };
        let carrier = power(&mut tx, 0.15);
        // Past the end of segment 1 and the shaping pulse's tail.
        power(&mut tx, 0.048);
        let gap = power(&mut tx, 0.008);
        let training = power(&mut tx, 0.2);
        assert!(carrier > 0.1, "no carrier in segment 1: {carrier}");
        assert!(gap < carrier / 100.0, "segment 2 was not silent: {gap}");
        assert!(training > 0.1, "no training after the gap: {training}");
    }

    #[test]
    fn segment_4_starts_the_way_table_4_prints_it() {
        // 0, 180, 180, 180, 180, 180, 0 degrees, and ending 180, 180, 0, 0.
        let mut s = Scrambler::seeded();
        let mut turns = Vec::new();
        for _ in 0..Training::Long.conditioning() {
            let bit = s.scramble(true);
            s.scramble(true);
            s.scramble(true);
            turns.push(if bit { 180 } else { 0 });
        }
        assert_eq!(turns[..7], [0, 180, 180, 180, 180, 180, 0]);
        assert_eq!(turns[turns.len() - 4..], [180, 180, 0, 0]);
    }

    #[test]
    fn segment_5_is_the_same_scrambler_still_running() {
        // Table 4 prints segment 5 as 270, 225, 315, 90, 45, 45, 180, 180 at
        // 4800, and the tribits that ask for those. Nothing sets it up: it is
        // what continuous ONEs give once segment 4 has finished.
        for conditioning in [Training::Short, Training::Long] {
            let mut s = Scrambler::seeded();
            for _ in 0..conditioning.conditioning() * 3 {
                s.scramble(true);
            }
            let mut tribits = Vec::new();
            for _ in 0..SCRAMBLED_ONES {
                let a = s.scramble(true);
                let b = s.scramble(true);
                let c = s.scramble(true);
                tribits.push(usize::from(a) << 2 | usize::from(b) << 1 | usize::from(c));
            }
            assert_eq!(
                tribits,
                [0b100, 0b110, 0b101, 0b010, 0b000, 0b000, 0b111, 0b111],
                "{conditioning:?}"
            );
            let degrees: Vec<u32> = tribits
                .iter()
                .map(|&t| u32::from(TRIBIT_TURN[t]) * 45)
                .collect();
            assert_eq!(degrees, [270, 225, 315, 90, 45, 45, 180, 180]);
        }
    }

    #[test]
    fn the_tribit_table_and_its_reverse_agree() {
        for tribit in 0..8u8 {
            let turn = TRIBIT_TURN[usize::from(tribit)];
            assert_eq!(TURN_TRIBIT[usize::from(turn)], tribit);
        }
        for dibit in 0..4u8 {
            let turn = DIBIT_TURN[usize::from(dibit)];
            assert_eq!(TURN_DIBIT[usize::from(turn >> 1)], dibit);
        }
    }

    #[test]
    fn the_scrambler_and_the_descrambler_are_inverses() {
        let mut tx = Scrambler::new();
        // A descrambler that starts in the wrong state still catches up, so
        // this one is deliberately not the transmitter's.
        let mut rx = Scrambler::seeded();
        let input: Vec<bool> = (0..200).map(|i| i % 5 == 0 || i % 7 == 3).collect();
        let out: Vec<bool> = input
            .iter()
            .map(|&b| rx.descramble(tx.scramble(b)))
            .collect();
        assert_eq!(
            out[7..],
            input[7..],
            "it should agree once the register has filled"
        );
    }

    #[test]
    fn a_burst_at_4800_comes_back_out() {
        let data = b"BinModem sends a page over V.27 ter at four thousand eight hundred.";
        let bits = loopback(Rate::R4800, Training::Long, data);
        assert!(
            find(&bits, &bits_of(data)).is_some(),
            "the message did not survive the round trip ({} bits back)",
            bits.len()
        );
    }

    #[test]
    fn a_burst_at_2400_comes_back_out() {
        let data = b"And at two thousand four hundred, which is the fall-back.";
        let bits = loopback(Rate::R2400, Training::Long, data);
        assert!(
            find(&bits, &bits_of(data)).is_some(),
            "the message did not survive the round trip ({} bits back)",
            bits.len()
        );
    }

    #[test]
    fn the_short_training_is_enough_on_a_clean_line() {
        let data = b"Short training carries data too.";
        let bits = loopback(Rate::R4800, Training::Short, data);
        assert!(find(&bits, &bits_of(data)).is_some());
    }

    #[test]
    fn the_training_check_arrives_as_zeros() {
        // T.30 6.2.6: TCF is a series of ZEROs for 1.5 s. It is the one
        // message whose content is its own test, so it is worth proving that
        // what goes in as zeros comes back as zeros.
        let mut tx = Transmitter::new(FS);
        let mut rx = Receiver::new(FS);
        rx.set_rate(Rate::R4800);
        tx.start(Rate::R4800, Training::Long);
        tx.push_bits(&vec![false; 7200]);
        let mut bits = Vec::new();
        for _ in 0..(FS * 3.0) as usize {
            rx.feed(tx.next_sample());
            bits.extend(rx.take_bits());
        }
        // The training is in front of it and the fill is behind it, so what
        // matters is the longest unbroken run rather than any fixed window.
        let mut longest = 0;
        let mut run = 0;
        for &bit in &bits {
            run = if bit { 0 } else { run + 1 };
            longest = longest.max(run);
        }
        assert!(
            longest >= 7100,
            "the longest run of zeros was {longest}, and 7200 were sent"
        );
    }


    /// One burst from a transmitter `hz` off, into a receiver started `skew`
    /// samples early, down a line `scale` times as loud.
    fn survives(rate: Rate, training: Training, echo: bool, hz: f64, skew: usize, scale: f64) -> bool {
        let data: Vec<u8> = (0..120u32).map(|i| (i * 37 + 11) as u8).collect();
        let mut tx = Transmitter::new(FS);
        tx.nco = Nco::new(CARRIER + hz, FS);
        tx.set_echo_protection(echo);
        let mut rx = Receiver::new(FS);
        rx.set_rate(rate);
        for _ in 0..skew {
            rx.feed(0.0);
        }
        tx.start(rate, training);
        tx.push_bytes(&data);
        let mut out = Vec::new();
        for _ in 0..(FS * 2.0) as usize {
            if tx.trained() && tx.pending_bits() == 0 {
                tx.stop();
            }
            rx.feed(tx.next_sample() * scale);
            out.extend(rx.take_bits());
        }
        find(&out, &bits_of(&data)).is_some()
    }

    #[test]
    fn the_carrier_is_found_wherever_it_starts() {
        // Clause 3 wants a receiver to accept seven hertz of error, and two
        // modems never start their oscillators on the same sample. At 2400,
        // seven hertz defeated a loop that steered by its own decisions in
        // every one of forty tries.
        for rate in [Rate::R4800, Rate::R2400] {
            for (training, echo) in [(Training::Long, false), (Training::Short, false), (Training::Long, true)] {
                for (hz, skew) in [(0.0, 0), (7.0, 5), (-7.0, 7)] {
                    for scale in [1.0, 0.0316] {
                        assert!(
                            survives(rate, training, echo, hz, skew, scale),
                            "{rate:?} {training:?} echo {echo}: {hz} Hz off, {skew} late, {:.0} dB",
                            20.0 * scale.log10()
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "hundreds of bursts; run it in release"]
    fn the_carrier_is_found_wherever_it_starts_every_way() {
        let mut failed = Vec::new();
        for rate in [Rate::R4800, Rate::R2400] {
            for (training, echo) in [(Training::Long, false), (Training::Short, false), (Training::Long, true)] {
                for hz in [0.0, 3.0, -3.0, 7.0, -7.0] {
                    for skew in 0..10 {
                        for scale in [1.0, 0.0316] {
                            if !survives(rate, training, echo, hz, skew, scale) {
                                failed.push((rate, training, echo, hz, skew, scale));
                            }
                        }
                    }
                }
            }
        }
        assert!(failed.is_empty(), "{} failed: {failed:?}", failed.len());
    }

    #[test]
    fn silence_is_not_a_carrier() {
        let mut rx = Receiver::new(FS);
        for _ in 0..(FS * 0.5) as usize {
            rx.feed(0.0);
        }
        assert!(!rx.carrier());
        assert!(rx.take_bits().is_empty());
    }

    #[test]
    fn the_carrier_goes_up_during_training_and_down_after_the_turn_off() {
        let mut tx = Transmitter::new(FS);
        let mut rx = Receiver::new(FS);
        rx.set_rate(Rate::R4800);
        tx.start(Rate::R4800, Training::Long);
        tx.push_bytes(b"a short page");
        let mut up_at = None;
        let mut down_at = None;
        for i in 0..(FS * 3.0) as usize {
            if tx.trained() && tx.pending_bits() == 0 {
                tx.stop();
            }
            rx.feed(tx.next_sample());
            if up_at.is_none() && rx.carrier() {
                up_at = Some(i);
            }
            if up_at.is_some() && down_at.is_none() && !rx.carrier() {
                down_at = Some(i);
            }
        }
        let up = up_at.expect("no carrier was ever found");
        let down = down_at.expect("the carrier never went away");
        assert!(
            (up as f64) < FS * 0.1,
            "took {:.0} ms to find the carrier",
            1000.0 * up as f64 / FS
        );
        assert!(down > up);
    }
}
