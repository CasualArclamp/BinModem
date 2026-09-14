//! V.27 ter: 4800 and 2400 bit/s, differentially encoded phase.
//!
//! The modulation every group 3 fax machine must have. A fax call agrees its
//! capabilities over 300 bit/s and then drops that carrier and raises this one
//! to send the page, so this is the half of a fax that carries the picture.
//!
//! Nothing here is adaptive in the way V.32 is. There is no rate negotiation,
//! no trellis, no echo canceller and no second station talking at the same
//! time: the line is half duplex and turns around between messages, so a burst
//! is a training sequence, then data, then silence. What makes that work is
//! the training, which is long enough to teach an equaliser the line from
//! nothing every single time.
//!
//! Two rates, differing only in how many bits ride on each symbol and how fast
//! the symbols go: three bits on eight phases at 1600 baud, or two bits on
//! four phases at 1200 baud. The carrier, the shaping, the scrambler and every
//! training segment are shared.

use dsp::filter::OnePole;
use dsp::{ComplexFir, Equalizer, Gardner, Nco, fir_lowpass, rrc_at, rrc_taps};

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

/// The quietest thing that may be called a carrier at all, and the level it
/// has to fall below before it is called gone.
///
/// Sixty decibels below full scale with five decibels of hysteresis, which is
/// what every other carrier detector in this modem uses. Only a floor: what
/// actually decides is the ratio below, because a fixed level cannot be both
/// low enough for a quiet line and high enough to ignore the noise on a noisy
/// one. Sixty decibels down was above the carrier on a real line at an
/// ordinary drive setting, and below the noise on a line with any hiss in it.
const CARRIER_ON: f64 = 1.0e-3;
const CARRIER_OFF: f64 = 5.62e-4;

/// How far above the quiet line a carrier has to be. Twelve decibels.
const ON_ABOVE_FLOOR: f64 = 4.0;

/// How far a carrier has to fall below its own loudest to be called gone.
///
/// Its own, because that is the only reference that is always available and
/// always right. Measuring the end of a burst against a fixed level, or
/// against an estimate of the noise, needs the noise to be known -- and the
/// only time it can be measured is while there is no carrier, which is
/// exactly what cannot be established when the detector is stuck on. A burst
/// that has stopped is twelve decibels down on the burst that was there, on
/// any line at any level, and nothing has to be known in advance.
const OFF_BELOW_LOUDEST: f64 = 0.25;

/// How fast the loudest-so-far is forgotten, per sample at 16 kHz.
///
/// About two seconds, so a burst that fades over a long page is followed
/// rather than cut off at the first quiet stretch.
const LOUDEST_DECAY: f64 = 3.1e-5;

/// How fast the estimate of the quiet line follows what it hears, going down
/// and going up, per sample at 16 kHz.
///
/// Down in a tenth of a second, so a burst ending is noticed; up over five
/// seconds, because what it is measuring is the noise on a line and that does
/// not change quickly. It only moves at all while there is no carrier, so
/// what it follows is only ever the quiet line.
const FLOOR_FALL: f64 = 6.25e-4;
const FLOOR_RISE: f64 = 1.25e-5;

/// Where the floor goes when a burst ends, as a fraction of the level then.
const FLOOR_AFTER_BURST: f64 = 0.5;

/// Symbols of a burst ignored while the filters fill.
const SETTLING: u64 = 8;

/// Symbols the carrier is then measured over.
///
/// Everything at the front of a turn-on sequence is two-phase: the plain
/// carrier of segment 1, the reversals of segment 3, the conditioning pattern
/// of segment 4. The short sequence has seventy-two symbols of that, so eight
/// and forty-eight is inside even the short one.
const ACQUIRING: u64 = 48;

/// What one symbol should come off the equaliser at.
const UNIT: f64 = 1.0;

/// Ceiling on the gain control, so silence does not become noise at full
/// scale while the far end is between bursts.
///
/// High enough not to be reached by a quiet line, which is a receiver
/// refusing to work rather than a receiver protecting itself. What stops
/// silence being amplified is the carrier detector, not this.
const MAX_GAIN: f64 = 400.0;

/// V.27 ter receiver.
///
/// It never decides where the training ended. It cannot usefully: nothing in
/// the turn-on sequence marks its own last symbol, and T.30 does not need it
/// to. The training check is a run of zeros and is recognised as one; a page
/// begins with an end-of-line code and is found by looking for it. So this
/// hands up descrambled bits from the moment a carrier is there, and whoever
/// is above finds its own place in them.
#[derive(Debug)]
pub struct Receiver {
    fs: f64,
    rate: Rate,
    nco: Nco,
    select: ComplexFir,
    matched: ComplexFir,
    gardner: Gardner,
    countdown: f64,
    previous_filtered: (f64, f64),
    /// Carrier phase and frequency offset, in turns and turns per symbol.
    phase: f64,
    frequency: f64,
    agc: OnePole,
    equalizer: Equalizer,
    level: OnePole,
    /// What the line sounds like with nothing on it.
    floor: f64,
    /// The loudest the burst in hand has been.
    loudest: f64,
    carrier: bool,
    symbols: u64,
    /// The phase the previous symbol landed on, in eighths of a turn.
    eighths: Option<u8>,
    descrambler: Scrambler,
    bits: Vec<bool>,
    last_symbol: (f64, f64),
    /// The averaged phase error the carrier loop works on.
    track: f64,
    /// The front of the burst as it arrived, before the carrier loop has
    /// touched it.
    front: Vec<(f64, f64)>,
    /// What the timing loop's input is multiplied by.
    timing_scale: f64,
}

impl Receiver {
    pub fn new(fs: f64) -> Self {
        let rate = Rate::default();
        let sps = fs / rate.baud();
        Self {
            fs,
            rate,
            nco: Nco::new(CARRIER, fs),
            // The signal reaches half the baud rate plus the roll-off either
            // side of the carrier: 1200 Hz at baseband for the faster rate.
            select: ComplexFir::new(fir_lowpass(1400.0, 121, fs)),
            matched: ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN)),
            gardner: Gardner::new(sps, 0.1),
            countdown: sps / 2.0,
            previous_filtered: (0.0, 0.0),
            phase: 0.0,
            frequency: 0.0,
            agc: OnePole::starting_at(1.0, 0.030, rate.baud()),
            // Long enough to reach across the delay spread of a telephone
            // circuit at 1600 baud, which is what segment 4 is for.
            equalizer: Equalizer::new(31, UNIT),
            level: OnePole::new(0.010, fs),
            floor: 0.0,
            loudest: 0.0,
            carrier: false,
            symbols: 0,
            eighths: None,
            descrambler: Scrambler::new(),
            bits: Vec::new(),
            last_symbol: (0.0, 0.0),
            track: 0.0,
            front: Vec::new(),
            timing_scale: 1.0,
        }
    }

    /// Set the rate the burst about to arrive is at.
    ///
    /// A fax receiver always knows this in advance: the DCS frame that came
    /// over V.21 named it, and the high-speed carrier that follows is at that
    /// rate and no other. Nothing here has to guess.
    pub fn set_rate(&mut self, rate: Rate) {
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        let sps = self.fs / rate.baud();
        self.matched = ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN));
        self.gardner = Gardner::new(sps, 0.1);
        self.countdown = sps / 2.0;
        self.agc = OnePole::starting_at(1.0, 0.030, rate.baud());
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Whether the far end's carrier is on the line.
    pub fn carrier(&self) -> bool {
        self.carrier
    }

    pub fn level(&self) -> f64 {
        self.level.value()
    }

    /// Where the last symbol landed, for a constellation display.
    pub fn constellation_point(&self) -> (f64, f64) {
        self.last_symbol
    }

    /// Mean distance from the decisions being made, in the same units the
    /// constellation is drawn in.
    pub fn residual_error(&self) -> f64 {
        self.equalizer.error()
    }

    /// How far apart two neighbouring points are.
    ///
    /// Every point sits on the unit circle, so the gap between neighbours is
    /// the chord: twice the sine of half the angle between them. Three
    /// quarters of a unit at 4800 and nearly one and a half at 2400, which is
    /// most of why the slower rate carries a page down a worse line.
    pub fn point_spacing(&self) -> f64 {
        let phases = f64::from(self.rate.phases());
        2.0 * (std::f64::consts::PI / phases).sin()
    }

    /// Forget the burst just gone and be ready for the next one.
    ///
    /// Everything that belongs to one burst goes -- the differential
    /// reference, the descrambler, the equaliser, any bits not yet taken, and
    /// the carrier detector along with them. The detector especially: what is
    /// on the line at the moment somebody starts listening for a burst is the
    /// tail of the last one, and a detector that carries its own state across a
    /// turnaround reports that tail as a burst that arrived and ended.
    pub fn restart(&mut self) {
        self.new_burst();
        self.carrier = false;
        self.loudest = 0.0;
        self.level.reset();
    }

    /// The same, less the detector.
    ///
    /// What the detector does when it finds a carrier: throw away the
    /// decoding state left over from the last burst, and keep its own, since
    /// it is the thing that just decided there is a burst at all.
    fn new_burst(&mut self) {
        // The equaliser starts again as well. Every burst carries a training
        // sequence built to teach one from nothing, so nothing is lost by it,
        // and keeping the old one turns a moment's trouble into a lasting one:
        // a burst of noise walks its taps off, and a receiver that carries
        // those taps into the next burst cannot read that one either, or the
        // retransmission that was meant to put things right.
        self.equalizer.reset();
        self.eighths = None;
        self.descrambler.reset();
        self.bits.clear();
        self.symbols = 0;
        self.front.clear();
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    pub fn feed(&mut self, sample: f64) {
        let (cos, sin) = self.nco.step();
        let selected = self.select.process((sample * cos, sample * -sin));
        let level = self
            .level
            .process((selected.0 * selected.0 + selected.1 * selected.1).sqrt());
        if self.carrier {
            self.loudest = self.loudest.max(level) * (1.0 - LOUDEST_DECAY);
        } else {
            self.loudest = 0.0;
            let k = if level < self.floor { FLOOR_FALL } else { FLOOR_RISE };
            self.floor += k * (level - self.floor);
        }
        let was = self.carrier;
        self.carrier = if self.carrier {
            level > (self.loudest * OFF_BELOW_LOUDEST).max(CARRIER_OFF)
        } else {
            level > (self.floor * ON_ABOVE_FLOOR).max(CARRIER_ON)
        };
        if self.carrier && !was {
            self.new_burst();
        }
        if !self.carrier && was {
            // The burst just went. Whatever is on the line now is the line
            // with nothing on it, or on its way there, so the floor starts
            // from half of where the level is rather than from wherever it was
            // left. Left at nothing -- which it is, for the first burst of a
            // call, since this receiver hears nothing between bursts -- the
            // noise on the line clears the threshold the moment the carrier
            // drops, the carrier comes straight back, and the burst never
            // ends. Half puts the way back on at half the burst's own level:
            // out of reach of the noise, and well within reach of a burst that
            // stopped for twenty milliseconds on purpose and carried on. It
            // falls from there to the real noise within a fraction of a second.
            self.floor = self.floor.max(level * FLOOR_AFTER_BURST);
        }
        let filtered = self.matched.process(selected);

        let previous = std::mem::replace(&mut self.previous_filtered, filtered);
        let before = self.countdown;
        self.countdown -= 1.0;
        if self.countdown > 0.0 {
            return;
        }
        let mu = before.clamp(0.0, 1.0);
        let at = (
            previous.0 + mu * (filtered.0 - previous.0),
            previous.1 + mu * (filtered.1 - previous.1),
        );
        self.countdown += self.gardner.interval();
        // The timing loop is handed the signal at about unit level, whatever
        // the line delivered: it divides its error by a power estimate that
        // starts at one and moves slowly, and a short training thirty
        // decibels down is over before that estimate has come down far enough
        // to let it move. Held while there is no carrier, so silence is not
        // scaled up into something to lock onto.
        if self.carrier {
            self.timing_scale = 1.0 / self.level.value().max(CARRIER_OFF);
        }
        let scale = self.timing_scale;
        let Some(scaled) = self.gardner.feed((at.0 * scale, at.1 * scale)) else {
            return;
        };
        self.on_symbol((scaled.0 / scale, scaled.1 / scale));
    }

    /// Measure the carrier from the two-phase front of the burst.
    ///
    /// Squaring a symbol that is either a point or its opposite leaves the
    /// same thing either way: twice the carrier's phase and none of the data.
    /// So the squares, each times the conjugate of the one before, turn by
    /// twice the carrier's frequency, and their sum points at twice its phase.
    /// Halving both gives the phase to within half a turn, which a
    /// differential code cannot tell from the truth.
    ///
    /// The loop that steers by its own decisions could not do this. At 2400,
    /// seven hertz is two degrees of turn a symbol and the loop corrects less
    /// than that for any error it can see, so it slipped from one phase to the
    /// next for the whole of the training and never caught up: every one of
    /// forty tries at plus or minus seven hertz failed.
    fn acquire(&mut self) {
        let squares: Vec<(f64, f64)> = self
            .front
            .iter()
            .map(|&(x, y)| (x * x - y * y, 2.0 * x * y))
            .collect();
        if squares.len() < 2 {
            return;
        }
        let conj_times = |r: (f64, f64), p: (f64, f64)| {
            (r.0 * p.0 + r.1 * p.1, r.1 * p.0 - r.0 * p.1)
        };
        let twice = squares
            .windows(2)
            .map(|w| conj_times(w[1], w[0]))
            .fold((0.0, 0.0), |a, z| (a.0 + z.0, a.1 + z.1));
        let per_symbol = twice.1.atan2(twice.0) / 2.0;
        let last = (squares.len() - 1) as f64;
        let middle = last / 2.0;
        let (re, im) = squares.iter().enumerate().fold((0.0, 0.0), |a, (n, z)| {
            let back = -2.0 * per_symbol * (n as f64 - middle);
            let (c, s) = (back.cos(), back.sin());
            (a.0 + z.0 * c - z.1 * s, a.1 + z.0 * s + z.1 * c)
        });
        let now = im.atan2(re) / 2.0 + per_symbol * (last - middle);
        let tau = std::f64::consts::TAU;
        self.frequency = -per_symbol / tau;
        self.phase = (-now / tau).rem_euclid(1.0);
        self.track = 0.0;
    }

    fn on_symbol(&mut self, symbol: (f64, f64)) {
        if self.carrier && self.symbols >= SETTLING && self.symbols < SETTLING + ACQUIRING {
            self.front.push(symbol);
            if self.symbols + 1 == SETTLING + ACQUIRING {
                self.acquire();
            }
        }
        let acquired = self.symbols >= SETTLING + ACQUIRING;
        let power = symbol.0 * symbol.0 + symbol.1 * symbol.1;
        let mean = if self.carrier {
            self.agc.process(power)
        } else {
            self.agc.value()
        };
        let gain = (UNIT * UNIT / mean.max(1e-12)).sqrt().clamp(0.0, MAX_GAIN);

        let turn = self.phase * std::f64::consts::TAU;
        let (c, s) = (turn.cos(), turn.sin());
        let point = (
            (symbol.0 * c - symbol.1 * s) * gain,
            (symbol.0 * s + symbol.1 * c) * gain,
        );

        // The carrier loop reads the unequalised symbol, so the equaliser's
        // own delay stays outside it.
        //
        // Eight phases, whatever the rate. At 2400 only four of them are ever
        // sent, but the training in front of the data is two-phase at both
        // rates, and a decision over four points would read a reversal as a
        // quarter turn of error. Eight is right for everything either rate
        // sends.
        let coarse = nearest_eighth(point);
        let want = point_at(coarse);
        let raw = point.1 * want.0 - point.0 * want.1;
        self.track += 0.20 * (raw - self.track);
        if self.carrier && acquired {
            // Second order, so what is left of the seven hertz clause 3
            // allows for is removed rather than merely followed.
            self.frequency = (self.frequency - 2.0e-5 * self.track).clamp(-0.02, 0.02);
            self.phase -= 0.010 * self.track;
        }
        self.phase += self.frequency;
        self.phase -= self.phase.floor();

        let equalized = self.equalizer.equalize(point);
        let decided = nearest_eighth(equalized);
        let decision = point_at(decided);

        self.symbols += 1;
        if self.carrier && acquired {
            self.equalizer.adapt(equalized, decision);
        }
        self.last_symbol = equalized;

        if !self.carrier {
            return;
        }

        let Some(previous) = self.eighths.replace(decided) else {
            // The first symbol of a burst is only a reference; a difference
            // needs two.
            return;
        };
        let change = (decided + 8 - previous) & 7;
        let (group, count) = match self.rate {
            Rate::R4800 => (TURN_TRIBIT[change as usize], 3),
            // A quarter turn is two eighths. Anything odd is an error, and
            // rounding it down is the nearest legal answer.
            Rate::R2400 => (TURN_DIBIT[(change >> 1) as usize], 2),
        };
        for i in (0..count).rev() {
            let bit = group >> i & 1 != 0;
            let out = self.descrambler.descramble(bit);
            self.bits.push(out);
        }
    }
}

/// The point one of the eight phases sits on.
fn point_at(eighths: u8) -> (f64, f64) {
    let angle = std::f64::consts::TAU * f64::from(eighths) / 8.0;
    (angle.cos(), angle.sin())
}

/// Which of the eight phases a point is nearest.
fn nearest_eighth(point: (f64, f64)) -> u8 {
    let angle = point.1.atan2(point.0) / std::f64::consts::TAU * 8.0;
    (angle.round() as i64).rem_euclid(8) as u8
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
