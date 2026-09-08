//! V.32 at 4800 bit/s: 2400 baud, four points, one band both ways.
//!
//! The step up from V.22bis is not the speed. V.22bis fits two directions into
//! one telephone channel by giving each half of it, which is why its receiver
//! can be handed the whole line and simply filter: everything the far end
//! sends is in one band, everything this end sends is in the other, and the
//! filter that selects the first discards the second along with the modem's
//! own echo of it.
//!
//! V.32 gives both directions the whole channel at once (2.1: one carrier, at
//! 1800 Hz, in each direction). Nothing in the received signal distinguishes
//! the far end from our own echo by frequency, so no filter can separate them
//! and one has to be subtracted instead. That is what [`dsp::EchoCanceller`]
//! is for, and it is why the two arrived together.
//!
//! 4800 and 9600 bit/s are implemented. At 4800 the scrambled data is taken
//! two bits at a time and differentially encoded into a quadrant (2.4.2 with
//! Table 1), one point to a quadrant. At 9600 it is taken four at a time: the
//! first two do the same job and the other two choose between four points
//! inside that quadrant (2.4.1.1). Twice the data at the same 2400 baud, and
//! nothing else about the modem changes -- the same scrambler, the same
//! differential coding, the same start-up conducted entirely in the four
//! states, and even the same mean power for the gain control to hold.
//!
//! What is not implemented is the other 9600, the trellis-coded alternative
//! of 2.4.1.2, which spends a redundant bit on a convolutional code over
//! thirty-two points and buys about four decibels with it. It is optional:
//! 1 e) requires every modem offering 9600 to be able to fall back on the
//! sixteen-state alternative, so this interworks. B8 of the rate signal is
//! what advertises it, and stays clear here.

pub mod startup;
pub mod trellis;

use dsp::filter::OnePole;
use dsp::{ComplexFir, Equalizer, Gardner, Nco, fir_lowpass, rrc_at, rrc_taps};

/// Modulation rate (2.3): 2400 baud, to within a hundredth of a per cent.
pub const BAUD: f64 = 2400.0;

/// Carrier frequency (2.1), the same in both directions.
pub const CARRIER: f64 = 1800.0;

/// Excess bandwidth of the pulse shaping.
///
/// The recommendation does not name one. It states the spectrum instead (2.2):
/// with continuous ones into the scrambler, the energy at 600 Hz and 3000 Hz
/// shall be 4.5 dB down on the maximum, give or take 2.5. Those two
/// frequencies are exactly the carrier plus and minus half the symbol rate, so
/// they are where a root-raised-cosine sits 3 dB down whatever roll-off it is
/// given, and the requirement is met by construction. A quarter is the usual
/// choice and puts the skirts at 300 and 3300 Hz.
pub const ROLLOFF: f64 = 0.25;

/// Symbols each side of centre in the shaping filter.
const SPAN: usize = 6;

/// Symbols between a state being chosen and its pulse appearing on the line.
///
/// The shaper centres each pulse on a symbol that has already arrived, so the
/// output runs this far behind. It is part of what any measurement of the
/// round trip actually measures, at both ends, and has to come off.
pub const SHAPING_DELAY: u64 = SPAN as u64;

/// Which of the two modulations 9600 bit/s is using.
///
/// 2.4.1 defines both and the rate signals choose between them: Table 6's B8
/// says trellis coding is available at the highest rate offered, and 5.4.2 has
/// R3 settle "the data rate, coding and any special operational modes to be
/// used by both modems". Below 9600 there is only one coding and this is
/// always [`Coding::Uncoded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coding {
    /// 2.4.1.1, sixteen points and four bits, no redundancy.
    #[default]
    Uncoded,
    /// 2.4.1.2, thirty-two points with a fifth bit from the convolutional
    /// encoder.
    Trellis,
}

/// Which end of the call this modem is.
///
/// It selects the scrambler (4): each direction uses a different polynomial,
/// unlike V.22bis where one serves both. Two modems with the same polynomial
/// would descramble each other's echo into their own data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Placed the call. Scrambles with 1 + x^-18 + x^-23.
    Call,
    /// Took the call. Scrambles with 1 + x^-5 + x^-23.
    Answer,
}

impl Mode {
    /// The two feedback taps of this direction's polynomial, in bits back.
    fn taps(self) -> (u32, u32) {
        match self {
            Self::Call => (18, 23),
            Self::Answer => (5, 23),
        }
    }

    /// What this end listens with: the far end's polynomial.
    pub fn peer(self) -> Self {
        match self {
            Self::Call => Self::Answer,
            Self::Answer => Self::Call,
        }
    }
}

/// The four signal states of 4800 bit/s (2.4.2, Figure 1).
///
/// One point to a quadrant, all at the root of ten, ninety degrees apart, and
/// arranged so that C is the negative of A and D the negative of B. That last
/// is not decoration. The conditioning signal of 5.2 is an alternation between
/// A and B followed by an alternation between C and D, written S and S-bar,
/// and the bar means what it says: the receiver takes its time reference from
/// the moment the signal inverts, which exists only because the second pair is
/// the first pair negated. The start-up of 5.4 then leans on the same fact
/// from the other side, alternating A with C so that the two cancel and leave
/// the carrier suppressed, which is what makes 600 and 3000 Hz the only things
/// on the line.
const STATES: [(f64, f64); 4] = [
    (-3.0, -1.0), // A
    (1.0, -3.0),  // B
    (3.0, 1.0),   // C
    (-1.0, 3.0),  // D
];

/// Index into [`STATES`] of each named state.
pub const STATE_A: usize = 0;
pub const STATE_B: usize = 1;
pub const STATE_C: usize = 2;
pub const STATE_D: usize = 3;

/// The four points of one quadrant, chosen by the two bits that are *not*
/// differentially encoded: Q3 and Q4 of 2.4.1.1, which is what carries 9600
/// bit/s where 4800 carries only the quadrant.
///
/// Reading this out of the Recommendation takes some care. The extracted text
/// of Table 3 has lost the sign of every coordinate — it renders the whole
/// table negative, "-0" included — and a constellation read off that would be
/// wrong in a way no test of our own two ends could ever notice. What survives
/// the extraction is the magnitudes, and those turn out to be enough: every
/// one of the sixteen points sits at a combination of 1 and 3, so its quadrant
/// supplies the signs and nothing is left to guess.
///
/// Table 3 gives four Q3Q4-to-magnitude patterns, one per quadrant, and they
/// are one pattern turned through quarter circles. They have to be. A
/// differentially coded receiver resolves the constellation only up to a
/// quarter turn, so Q3 and Q4 must read the same whichever way up it lands.
///
/// That leaves exactly one thing undetermined — which of a quadrant's four
/// points is the one 4800 bit/s uses — and Figure 1 settles it. Each of the
/// letters A, B, C and D is followed by a code two columns to its right: 0001,
/// 0101, 1101, 1001. All four end in 01. With that anchor the whole thing
/// falls out, and reproduces both the magnitudes of Table 3 and the
/// left-to-right order of all sixteen labels in Figure 1.
const WITHIN_QUADRANT: [(f64, f64); 4] = [
    (-1.0, -1.0), // Q3 Q4 = 0 0
    (-3.0, -1.0), // 0 1 — state A, and the point every lower rate keeps to
    (-1.0, -3.0), // 1 0
    (-3.0, -3.0), // 1 1
];

/// The one of the four that the start-up and 4800 bit/s use.
const WITHIN_4800: usize = 1;

/// Turn a point through whole quarter circles.
const fn rotate(p: (f64, f64), quarters: usize) -> (f64, f64) {
    match quarters & 3 {
        0 => p,
        1 => (-p.1, p.0),
        2 => (-p.0, -p.1),
        _ => (p.1, -p.0),
    }
}

/// The point for a quadrant and a choice within it (Figure 1).
fn signal_point(state: usize, within: usize) -> (f64, f64) {
    rotate(WITHIN_QUADRANT[within], state)
}

/// Root mean square of the constellation, which is one radius here.
///
/// The same for four points and for sixteen, which is not luck: the sixteen
/// average 2, 10, 10 and 18 in equal numbers, and the four are all 10.
pub const CONSTELLATION_RMS: f64 = 3.162_277_660_168_379_5;

/// Mean power of the constellation.
pub const CONSTELLATION_MEAN_POWER: f64 = 10.0;

/// Quadrant change for each dibit (Table 1).
///
/// Dibit 00 turns a quarter, 01 stays put, 10 turns a half and 11 turns three
/// quarters. The same table V.22bis uses, which is no coincidence: it is the
/// convention for differential quadrant coding across the whole series.
const QUADRANT_CHANGE: [u8; 4] = [1, 0, 2, 3];

/// The inverse, for the receiver.
const CHANGE_TO_DIBIT: [u8; 4] = [0b01, 0b00, 0b10, 0b11];

/// Ceiling on the receiver's gain, so silence is not amplified to infinity.
const MAX_GAIN: f64 = 400.0;

/// Level at which a carrier is declared present, and the lower level at which
/// it is declared gone. Five decibels apart, as V.22bis 6.5.2 asks for.
const CARRIER_ON: f64 = 1.0e-3;
const CARRIER_OFF: f64 = 5.62e-4;

/// Which of the four states a received point is nearest.
///
/// The states are a quarter turn apart, so the decision is which quarter the
/// point falls in, with the boundaries midway between neighbours rather than
/// on the axes. Turning the point by half that angle first puts the boundaries
/// where an ordinary test of signs finds them.
fn nearest_state(p: (f64, f64)) -> usize {
    // Half of ninety degrees away from state A's own angle.
    const COS: f64 = 0.923_879_532_511_286_8;
    const SIN: f64 = 0.382_683_432_365_089_8;
    // Bring A to just inside the first quadrant, then read the quadrant off.
    let turned = (p.0 * COS - p.1 * SIN, p.0 * SIN + p.1 * COS);
    let from_a = match (turned.0 >= 0.0, turned.1 >= 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    };
    // A sits in the third quadrant, so the count starts from there.
    (from_a + 2) & 3
}

/// Which of the sixteen points a received one is nearest, as a quadrant and a
/// choice within it.
///
/// Searched rather than sliced. The points do lie on a grid that could be
/// quantised coordinate by coordinate, but sixteen distances at 2400 baud is
/// nothing, and this stays right if the constellation ever stops being one.
fn nearest_point(p: (f64, f64)) -> (usize, usize) {
    let mut best = (0, 0);
    let mut nearest = f64::INFINITY;
    for state in 0..4 {
        for within in 0..4 {
            let q = signal_point(state, within);
            let away = (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2);
            if away < nearest {
                nearest = away;
                best = (state, within);
            }
        }
    }
    best
}

/// How many bits a symbol carries at a given rate (2.4.1 and 2.4.2).
fn bits_per_symbol(bits_per_second: u32) -> u32 {
    if bits_per_second >= 9600 { 4 } else { 2 }
}

/// The self-synchronising scrambler of clause 4.
///
/// One polynomial for each direction. The transmitter divides by it and the
/// receiver multiplies back, which is what makes the descrambler synchronise
/// itself: it needs no agreement on where the sequence began, only the last
/// twenty-three bits of what actually arrived.
#[derive(Debug, Clone)]
pub struct Scrambler {
    register: u32,
    first: u32,
    second: u32,
}

impl Scrambler {
    pub fn new(mode: Mode) -> Self {
        let (first, second) = mode.taps();
        Self {
            register: 0,
            first,
            second,
        }
    }

    fn feedback(&self) -> bool {
        let a = (self.register >> (self.first - 1)) & 1;
        let b = (self.register >> (self.second - 1)) & 1;
        (a ^ b) != 0
    }

    /// Divide by the polynomial: the output feeds back.
    pub fn scramble(&mut self, bit: bool) -> bool {
        let out = bit ^ self.feedback();
        self.register = (self.register << 1) | u32::from(out);
        out
    }

    /// Multiply by the polynomial: the input feeds back.
    pub fn descramble(&mut self, bit: bool) -> bool {
        let out = bit ^ self.feedback();
        self.register = (self.register << 1) | u32::from(bit);
        out
    }

    pub fn reset(&mut self) {
        self.register = 0;
    }
}

/// Encoding for the TRN segment after its first 256 symbols (Table 5).
const TRN_STATES: [usize; 4] = [STATE_A, STATE_B, STATE_C, STATE_D];

/// What the transmitter puts on the line.
///
/// The start-up of 5.4 is made of these. The first several are not data at all
/// but fixed patterns of constellation states, chosen for what they look like
/// on the line rather than for what they carry: a repeated state is the bare
/// carrier, and an alternation between opposite states is that carrier
/// suppressed, leaving a pair of sidebands half the symbol rate apart. Each
/// modem knows the other by which of these it hears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Signal {
    /// Nothing. 5.4.1 has the calling modem cease transmitting partway
    /// through, and the drop is how the answering modem knows to move on.
    Silent,
    /// The answering tone of V.25, 2100 Hz (5.1).
    AnswerTone,
    /// State A repeated: the carrier alone, at 1800 Hz. Called AA in Figure 4.
    StateA,
    /// State C repeated. The change from AA to CC is a phase reversal, which
    /// is the mark 5.4.1 times against.
    StateC,
    /// Alternating A and C: carrier suppressed, sidebands at 600 and 3000 Hz.
    AlternateAC,
    /// The same alternation begun on the other state, so that the change from
    /// AC to CA is again a reversal (5.4.2).
    AlternateCA,
    /// Segment 1 of the conditioning signal (5.2.1): A alternating with B.
    ConditioningS,
    /// Segment 2 (5.2.2): C alternating with D, which is segment 1 negated.
    ConditioningSbar,
    /// Segment 3 (5.2.3): scrambled ones at 4800 with the differential
    /// encoding disabled, for training the far equaliser and the near echo
    /// canceller.
    Trn,
    /// A rate signal: the 16 bits of Table 6 or 7, repeated, scrambled and
    /// differentially encoded (5.3).
    Rate(u16),
    /// Scrambled binary ones, which is what fills a connection between one
    /// byte of data and the next.
    #[default]
    ScrambledOnes,
}

/// Frequency of the V.25 answering tone.
pub const ANSWER_TONE: f64 = 2100.0;

/// Symbols of TRN sent as A or C before Table 5 takes over (5.2.3).
pub const TRN_BINARY_SYMBOLS: u32 = 256;

/// V.32 transmitter at 4800 bit/s.
#[derive(Debug)]
pub struct Transmitter {
    fs: f64,
    nco: Nco,
    scrambler: Scrambler,
    quadrant: u8,
    /// Symbols still contributing to the pulse, oldest first.
    history: Vec<(f64, f64)>,
    /// Position within the current symbol period, in symbols.
    phase: f64,
    pending: Vec<bool>,
    signal: Signal,
    /// Generator for the answering tone, which is not modulation.
    answer: Nco,
    /// Symbols sent, ever. Free-running on purpose: see [`Self::set_signal`].
    tick: u64,
    /// Symbols sent since the current signal began, for TRN's change of
    /// encoding partway through.
    since_change: u64,
    /// Position in the repeating rate sequence.
    rate_bit: u32,
    /// A rate sequence that will replace the current one at its next boundary
    /// (5.3.2).
    next_rate: Option<u16>,
    /// Symbols of the current rate sequence sent since it took effect, which
    /// is not the same as since the signal was asked for.
    rate_symbols: u64,
    /// Which of the quadrant's four points the current symbol is on. Only
    /// 9600 bit/s ever moves it off the one the start-up uses.
    within: usize,
    /// Bits carried by each symbol: two at 4800, four at 9600.
    bits: u32,
    /// Which of the two 9600 modulations is in use.
    coding: Coding,
    /// The convolutional encoder, used only by [`Coding::Trellis`].
    trellis: trellis::Encoder,
    /// The sample most recently produced, for the echo canceller.
    last_sample: f64,
}

impl Transmitter {
    pub fn new(mode: Mode, fs: f64) -> Self {
        Self {
            fs,
            nco: Nco::new(CARRIER, fs),
            scrambler: Scrambler::new(mode),
            quadrant: 0,
            history: vec![(0.0, 0.0); 2 * SPAN + 1],
            phase: 0.0,
            pending: Vec::new(),
            signal: Signal::default(),
            answer: Nco::new(ANSWER_TONE, fs),
            tick: 0,
            since_change: 0,
            rate_bit: 0,
            next_rate: None,
            rate_symbols: 0,
            within: WITHIN_4800,
            bits: 2,
            coding: Coding::Uncoded,
            trellis: trellis::Encoder::new(),
            last_sample: 0.0,
        }
    }

    /// Change the rate the data itself is coded at.
    ///
    /// 5.4 puts this at one exact moment: the E sequence a modem sends says
    /// what the scrambled ones immediately following it are coded at, so the
    /// transmitter changes as it stops sending E and not before. Everything
    /// earlier -- the conditioning signal, the training segment, the rate
    /// exchange -- is two bits to the symbol whatever is being negotiated.
    pub fn set_data_rate(&mut self, bits_per_second: u32) {
        self.bits = bits_per_symbol(bits_per_second);
    }

    /// Choose between the two modulations 9600 bit/s has (2.4.1).
    ///
    /// Set at the same moment as the rate and for the same reason: the E
    /// sequence says what the scrambled ones after it are coded at, so the
    /// change belongs where that sequence ends. The convolutional encoder
    /// starts from zero, which is where a far end's decoder assumes nothing
    /// and converges anyway.
    pub fn set_coding(&mut self, coding: Coding) {
        if coding != self.coding {
            self.trellis.reset();
        }
        self.coding = coding;
    }

    /// What to send.
    ///
    /// The symbol count is deliberately *not* restarted, and everything that
    /// alternates counts from it. Restarting it would make the change from one
    /// alternating pattern to another a reversal only half the time, since
    /// A,C,A,C followed by a fresh C,A,C,A gives a doubled state at the join
    /// only if the join falls on the right parity; on the other parity the two
    /// run straight on and nothing happens at all.
    ///
    /// This is what 5.4.2 is guarding when it requires the alternations to
    /// last "an even number of symbol intervals", and 5.2 depends on the same
    /// thing: segment 2 of the conditioning signal is segment 1 negated, which
    /// it only is if the two are counted from the same place. The reversal at
    /// the join is the time reference the far receiver sets its clock by.
    pub fn set_signal(&mut self, signal: Signal) {
        if signal == self.signal {
            return;
        }
        // 5.3.2: "the modem shall first complete the transmission of the
        // current 16-bit rate sequence, and then transmit one 16-bit sequence
        // E". One rate signal replacing another therefore waits for the
        // boundary rather than cutting in where it is asked for.
        //
        // Cutting in is what this did, and R3 does not arrive on a boundary --
        // it arrives whenever the line brings it. A call to a real modem cut
        // the last R2 short after twelve of its sixteen bits and put the E
        // there, so the far end, whose framing is locked to the sequences it
        // has been reading, saw 0101000100011111 at its own alignment: B0-3
        // neither 0000 nor 1111, which is neither a rate signal nor an E. It
        // waited for an E that never came at a boundary and gave up one round
        // trip later, every time.
        if let (Signal::Rate(_), Signal::Rate(next)) = (self.signal, signal)
            && self.rate_bit != 0
        {
            self.next_rate = Some(next);
            return;
        }
        self.signal = signal;
        self.since_change = 0;
        self.rate_bit = 0;
        self.next_rate = None;
        self.rate_symbols = 0;
        if signal == Signal::Trn {
            // 5.2.3: the scrambler starts from all zeros for the segment.
            self.scrambler.reset();
        }
    }

    /// Symbols sent of the rate sequence now going out.
    pub fn rate_symbols(&self) -> u64 {
        self.rate_symbols
    }

    /// Whether a rate sequence has been asked for and is waiting for the
    /// current one to finish (5.3.2).
    ///
    /// The pair matters to anything timing a sequence: eight symbols of E
    /// means eight symbols after the E began, not after it was asked for, and
    /// between those two moments the symbols going out belong to the sequence
    /// before it.
    pub fn rate_pending(&self) -> bool {
        self.next_rate.is_some()
    }

    pub fn signal(&self) -> Signal {
        self.signal
    }

    /// The state last put on the line, as an index into the four.
    pub fn state(&self) -> usize {
        self.quadrant as usize
    }

    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend_from_slice(bits);
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            for i in (0..8).rev() {
                self.pending.push(b & (1 << i) != 0);
            }
        }
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }

    /// Choose the next state to transmit.
    ///
    /// Turning state A through whole quadrants gives B, C and D in order, so
    /// the quadrant number and the index of the state are the same thing, and
    /// the differential coding of 2.4.2 lands on a point without a table.
    fn next_state(&mut self) -> usize {
        let tick = self.tick;
        let since_change = self.since_change;
        self.tick += 1;
        self.since_change += 1;
        // Everything but data sits on the point the four states are made of.
        self.within = WITHIN_4800;
        let alternate = |even, odd| if tick.is_multiple_of(2) { even } else { odd };
        let state = match self.signal {
            Signal::Silent | Signal::AnswerTone => return self.quadrant as usize,
            Signal::StateA => STATE_A,
            Signal::StateC => STATE_C,
            Signal::AlternateAC => alternate(STATE_A, STATE_C),
            Signal::AlternateCA => alternate(STATE_C, STATE_A),
            Signal::ConditioningS => alternate(STATE_A, STATE_B),
            Signal::ConditioningSbar => alternate(STATE_C, STATE_D),
            Signal::Trn => {
                // 5.2.3: scrambled ones with the differential encoding
                // disabled. For the first 256 symbols the leading bit of each
                // dibit chooses between A and C; after that the whole dibit
                // chooses, by Table 5.
                let first = self.scrambler.scramble(true);
                let second = self.scrambler.scramble(true);
                if since_change < u64::from(TRN_BINARY_SYMBOLS) {
                    if first { STATE_C } else { STATE_A }
                } else {
                    TRN_STATES[usize::from(first) << 1 | usize::from(second)]
                }
            }
            Signal::Rate(mut sequence) => {
                // 5.3: the 16 bits repeat, scrambled, and are differentially
                // encoded as data is.
                let mut dibit = [false; 2];
                for slot in &mut dibit {
                    // The boundary a replacement has been waiting for.
                    if self.rate_bit == 0
                        && let Some(next) = self.next_rate.take()
                    {
                        sequence = next;
                        self.signal = Signal::Rate(next);
                        self.rate_symbols = 0;
                    }
                    let bit = sequence & (1 << (15 - self.rate_bit)) != 0;
                    self.rate_bit = (self.rate_bit + 1) % 16;
                    *slot = self.scrambler.scramble(bit);
                }
                self.rate_symbols += 1;
                return self.turn(dibit);
            }
            Signal::ScrambledOnes => {
                // 2.4.1: the scrambled stream is divided into groups of four,
                // of which the first two are differentially encoded into the
                // quadrant and the second two choose a point inside it. At
                // 4800 the group is two long and the point is fixed (2.4.2).
                let group = self.next_group();
                if self.bits == 4 {
                    self.within = usize::from(group[2]) << 1 | usize::from(group[3]);
                }
                return self.turn([group[0], group[1]]);
            }
        };
        self.quadrant = state as u8;
        state
    }

    /// The next group of scrambled bits, as many as the rate carries.
    ///
    /// Between one byte from the terminal and the next there is nothing to
    /// send and the line carries ones, which is what 5.4 asks for: the far end
    /// stays trained on a signal that never stops.
    fn next_group(&mut self) -> [bool; 4] {
        let mut group = [false; 4];
        for slot in group.iter_mut().take(self.bits as usize) {
            let bit = if self.pending.is_empty() {
                true
            } else {
                self.pending.remove(0)
            };
            *slot = self.scrambler.scramble(bit);
        }
        group
    }

    /// Apply the differential quadrant coding of Table 1 and land on a state.
    fn turn(&mut self, dibit: [bool; 2]) -> usize {
        let change = QUADRANT_CHANGE[usize::from(dibit[0]) << 1 | usize::from(dibit[1])];
        self.quadrant = (self.quadrant + change) & 3;
        self.quadrant as usize
    }

    fn next_symbol(&mut self) -> (f64, f64) {
        // 2.4.1.2 replaces the quadrant machinery outright: the differential
        // coding is Table 2 rather than Table 1, a fifth bit comes from the
        // convolutional encoder, and the point is one of thirty-two rather
        // than one of four quadrants times one of four places inside it. Only
        // the data phase is affected -- the conditioning signal, the training
        // segment and both rate exchanges are four points whatever was agreed.
        if self.coding == Coding::Trellis
            && self.bits == 4
            && self.signal == Signal::ScrambledOnes
        {
            let group = self.next_group();
            let code = self.trellis.encode(group);
            // The quadrant tracker is deliberately left alone. Half the
            // thirty-two points sit on an axis and belong to no quadrant, and
            // nothing reads it while this coding is running: Table 2's
            // differential state lives inside the encoder instead.
            return trellis::point(code);
        }
        let state = self.next_state();
        signal_point(state, self.within)
    }

    /// The sample most recently put on the line.
    ///
    /// An echo canceller needs it: what comes back is a filtered copy of what
    /// went out, and the only way to subtract it is to be handed the original.
    pub fn last_sample(&self) -> f64 {
        self.last_sample
    }

    pub fn next_sample(&mut self) -> f64 {
        let sample = self.produce();
        self.last_sample = sample;
        sample
    }

    fn produce(&mut self) -> f64 {
        // Two of the signals are not modulation at all.
        match self.signal {
            Signal::Silent => return 0.0,
            Signal::AnswerTone => {
                let (cos, _) = self.answer.step();
                return cos;
            }
            _ => {}
        }
        self.phase += BAUD / self.fs;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol();
            self.history.push(symbol);
        }

        // The pulse, summed over every symbol still in range. The offset grows
        // with the phase so that when the phase wraps and the history shifts,
        // the two cancel and the pulse advances smoothly.
        let centre = SPAN as f64;
        let mut baseband = (0.0, 0.0);
        for (i, &(re, im)) in self.history.iter().enumerate() {
            let offset = self.phase + centre - i as f64;
            let tap = rrc_at(offset, ROLLOFF);
            baseband.0 += re * tap;
            baseband.1 += im * tap;
        }

        let (cos, sin) = self.nco.step();
        (baseband.0 * cos - baseband.1 * sin) / CONSTELLATION_RMS
    }
}

/// V.32 receiver at 4800 bit/s.
///
/// Structurally the V.22bis receiver, with the channel-selecting filter turned
/// into a plain anti-alias low-pass: there is no neighbouring channel to
/// select against, because the far end is not in a neighbouring channel. It is
/// in this one, on top of us.
#[derive(Debug)]
pub struct Receiver {
    nco: Nco,
    /// Baseband low-pass. Wide, because the signal fills the band.
    select: ComplexFir,
    matched: ComplexFir,
    gardner: Gardner,
    countdown: f64,
    previous_filtered: (f64, f64),
    phase: f64,
    frequency: f64,
    agc: OnePole,
    equalizer: Equalizer,
    symbols: u64,
    quadrant: Option<u8>,
    descrambler: Scrambler,
    bits: Vec<bool>,
    last_symbol: (f64, f64),
    level: OnePole,
    carrier: bool,
    /// Whether the equaliser may learn from what is arriving.
    adapting: bool,
    /// Bits each arriving symbol carries: two at 4800, four at 9600.
    carried: u32,
    /// Which of the two 9600 modulations is in use.
    coding: Coding,
    /// The Viterbi decoder, used only by [`Coding::Trellis`].
    trellis: trellis::Decoder,
}

impl Receiver {
    /// `mode` is this modem's own end; the descrambler is set to the far
    /// end's polynomial, since that is what will arrive.
    pub fn new(mode: Mode, fs: f64) -> Self {
        let sps = fs / BAUD;
        Self {
            nco: Nco::new(CARRIER, fs),
            // The signal reaches 1200 Hz plus the roll-off either side of the
            // carrier, so 1500 Hz at baseband. Nothing sits beyond it that a
            // filter could usefully remove, so this is only keeping the image
            // at twice the carrier out of the loops.
            select: ComplexFir::new(fir_lowpass(1600.0, 121, fs)),
            matched: ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN)),
            gardner: Gardner::new(sps, 0.1),
            countdown: sps / 2.0,
            previous_filtered: (0.0, 0.0),
            phase: 0.0,
            frequency: 0.0,
            agc: OnePole::starting_at(CONSTELLATION_MEAN_POWER, 0.050, fs / sps),
            equalizer: Equalizer::new(21, 1.0),
            symbols: 0,
            quadrant: None,
            descrambler: Scrambler::new(mode.peer()),
            bits: Vec::new(),
            last_symbol: (0.0, 0.0),
            level: OnePole::new(0.020, fs),
            carrier: false,
            adapting: true,
            carried: 2,
            coding: Coding::Uncoded,
            trellis: trellis::Decoder::new(),
        }
    }

    /// Change the rate the arriving data is coded at.
    ///
    /// A different moment from the transmitter's, and 5.4 is explicit about
    /// which: "When the modem detects an incoming 16-bit E sequence ... it
    /// shall condition itself to receive data at the rate and with the coding
    /// indicated by the E sequence." The far end changes as it finishes
    /// sending that E, so the two land on the same place in the stream.
    pub fn set_data_rate(&mut self, bits_per_second: u32) {
        self.carried = bits_per_symbol(bits_per_second);
    }

    /// Choose between the two modulations 9600 bit/s has (2.4.1).
    pub fn set_coding(&mut self, coding: Coding) {
        if coding != self.coding {
            self.trellis.reset();
        }
        self.coding = coding;
    }

    pub fn feed(&mut self, sample: f64) {
        let (cos, sin) = self.nco.step();
        let selected = self.select.process((sample * cos, sample * -sin));
        let level = self
            .level
            .process((selected.0 * selected.0 + selected.1 * selected.1).sqrt());
        self.carrier = if self.carrier {
            level > CARRIER_OFF
        } else {
            level > CARRIER_ON
        };
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
        let Some(symbol) = self.gardner.feed(at) else { return };
        self.on_symbol(symbol);
    }

    fn on_symbol(&mut self, symbol: (f64, f64)) {
        let power = symbol.0 * symbol.0 + symbol.1 * symbol.1;
        // Every loop in here is held still together, not just the equaliser.
        // A receiver that goes on gaining, timing and tracking carrier while
        // the far end is silent does all three on its own echo, and a carrier
        // loop wound onto the wrong signal does not unwind: the frequency term
        // is an integrator, and what it has learned it keeps.
        let mean_power = if self.adapting {
            self.agc.process(power)
        } else {
            self.agc.value()
        };
        let gain = (CONSTELLATION_MEAN_POWER / mean_power.max(1e-9))
            .sqrt()
            .clamp(0.0, MAX_GAIN);

        let turn = self.phase * std::f64::consts::TAU;
        let (c, s) = (turn.cos(), turn.sin());
        let point = (
            (symbol.0 * c - symbol.1 * s) * gain,
            (symbol.0 * s + symbol.1 * c) * gain,
        );

        // The carrier loop works on the unequalised symbol, so the equaliser's
        // delay stays outside it. The decision has to be over whichever
        // constellation is in use: sixteen points read against the four would
        // put the error at a quarter of a turn for a point sitting exactly
        // where it belongs.
        let coarse = match (self.carried, self.coding) {
            (4, Coding::Trellis) => trellis::point(trellis::nearest(point)),
            (4, Coding::Uncoded) => {
                let (state, within) = nearest_point(point);
                signal_point(state, within)
            }
            _ => STATES[nearest_state(point)],
        };
        let error = (point.1 * coarse.0 - point.0 * coarse.1)
            / (coarse.0 * coarse.0 + coarse.1 * coarse.1 + 1e-9);
        if self.adapting {
            // Second order, so the seven hertz of offset 2.1 allows for is
            // removed rather than merely tracked.
            self.frequency = (self.frequency - 1.5e-5 * error).clamp(-0.02, 0.02);
            self.phase -= 0.008 * error;
        }
        // The offset already found goes on being taken out even while the loop
        // is held still. It belongs to the far end's oscillator, which does not
        // stop running when the far end stops talking.
        self.phase += self.frequency;
        self.phase -= self.phase.floor();

        let normalized = (point.0 / CONSTELLATION_RMS, point.1 / CONSTELLATION_RMS);
        let equalized = self.equalizer.equalize(normalized);
        let scaled = (
            equalized.0 * CONSTELLATION_RMS,
            equalized.1 * CONSTELLATION_RMS,
        );
        // 2.4.1.2: the decision is a whole sequence rather than a point, so
        // the trellis decoder is asked for it and the nearest of the
        // thirty-two serves only to keep the equaliser learning. That is what
        // a decision-directed equaliser wants anyway -- something to compare
        // this symbol against now, rather than the right answer two dozen
        // symbols later.
        let trellis_coded = self.coding == Coding::Trellis && self.carried == 4;
        let decision = if trellis_coded {
            trellis::point(trellis::nearest(scaled))
        } else {
            let (state, within) = if self.carried == 4 {
                nearest_point(scaled)
            } else {
                (nearest_state(scaled), WITHIN_4800)
            };
            signal_point(state, within)
        };

        self.symbols += 1;
        if self.symbols > 64 && self.carrier && self.adapting {
            self.equalizer.adapt(
                equalized,
                (
                    decision.0 / CONSTELLATION_RMS,
                    decision.1 / CONSTELLATION_RMS,
                ),
            );
        }
        self.last_symbol = scaled;

        if trellis_coded {
            if let Some(group) = self.trellis.decode(scaled) {
                for bit in group {
                    let out = self.descrambler.descramble(bit);
                    self.bits.push(out);
                }
            }
            return;
        }

        // Every state sits in its own quadrant, so the state index is the
        // quadrant and the turn between two of them is the difference.
        let (state, within) = if self.carried == 4 {
            nearest_point(scaled)
        } else {
            (nearest_state(scaled), WITHIN_4800)
        };
        let quadrant = state as u8;
        let Some(previous) = self.quadrant.replace(quadrant) else {
            return;
        };
        let change = (quadrant + 4 - previous) & 3;
        let dibit = CHANGE_TO_DIBIT[change as usize];
        // 2.4.1 in reverse: the quadrant carries the first two bits of the
        // group and the point within it the other two, in that order.
        let group = [
            dibit & 0b10 != 0,
            dibit & 0b01 != 0,
            within & 0b10 != 0,
            within & 0b01 != 0,
        ];
        for &bit in group.iter().take(self.carried as usize) {
            let out = self.descrambler.descramble(bit);
            self.bits.push(out);
        }
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// Take whole octets, most significant bit first, leaving any remainder.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        let whole = self.bits.len() / 8;
        let bits: Vec<bool> = self.bits.drain(..whole * 8).collect();
        bits.as_chunks::<8>().0.iter()
            .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)))
            .collect()
    }

    pub fn constellation_point(&self) -> (f64, f64) {
        (
            self.last_symbol.0 / CONSTELLATION_RMS,
            self.last_symbol.1 / CONSTELLATION_RMS,
        )
    }

    pub fn residual_error(&self) -> f64 {
        self.equalizer.error()
    }

    pub fn equalizer_blind(&self) -> bool {
        self.equalizer.is_blind()
    }

    pub fn carrier(&self) -> bool {
        self.carrier
    }

    /// Whether the equaliser may learn from what is arriving.
    ///
    /// It must not while this modem is sending its training segment. The far
    /// end is required to be silent through that, which is the whole point of
    /// it, so everything heard is this modem's own echo and everything the
    /// equaliser learns is about a path the far end's signal will never take.
    ///
    /// The two ends are not equally exposed to getting this wrong. After its
    /// training segment the answering modem falls silent and has a clear
    /// stretch of the far end's conditioning signal to correct itself on; the
    /// calling modem goes straight into sending its rate signal and keeps
    /// sending until answered, so it never has a quiet moment and carries
    /// whatever it learned into the rest of the call.
    pub fn set_adapting(&mut self, adapting: bool) {
        self.adapting = adapting;
        self.gardner.set_adapting(adapting);
    }

    pub fn level(&self) -> f64 {
        self.level.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_states_are_a_quarter_turn_apart_at_one_radius() {
        for (i, &(re, im)) in STATES.iter().enumerate() {
            let r = (re * re + im * im).sqrt();
            assert!(
                (r - CONSTELLATION_RMS).abs() < 1e-12,
                "state {i} is at radius {r}"
            );
        }
        // Each is the one before it turned a quarter, which is what lets the
        // transmitter treat the quadrant number and the index of the state as
        // the same thing.
        for i in 0..4 {
            let turned = rotate(STATES[i], 1);
            assert_eq!(
                (turned.0, turned.1),
                STATES[(i + 1) & 3],
                "turning state {i} does not give the next"
            );
        }
    }

    #[test]
    fn the_conditioning_signal_inverts_between_its_two_segments() {
        // 5.2: segment one alternates A with B and segment two alternates C
        // with D, written S and S-bar. The bar is the whole point of the
        // arrangement, since the receiver takes its time reference from the
        // inversion, so C has to be exactly the negative of A and D of B.
        assert_eq!(STATES[STATE_C], (-STATES[STATE_A].0, -STATES[STATE_A].1));
        assert_eq!(STATES[STATE_D], (-STATES[STATE_B].0, -STATES[STATE_B].1));
    }

    #[test]
    fn slicing_returns_each_state_from_its_own_neighbourhood() {
        for (i, &(re, im)) in STATES.iter().enumerate() {
            assert_eq!(nearest_state((re, im)), i, "state {i} itself");
            // A fifth of the way towards each neighbour, and still itself.
            for &(nre, nim) in &STATES {
                let probe = (re + (nre - re) * 0.2, im + (nim - im) * 0.2);
                assert_eq!(nearest_state(probe), i, "state {i} nudged towards a neighbour");
            }
        }
    }

    #[test]
    fn each_direction_scrambles_with_its_own_polynomial() {
        // 4: the two directions must differ, or each modem would descramble
        // its own echo into what looks like the far end's data.
        let mut call = Scrambler::new(Mode::Call);
        let mut answer = Scrambler::new(Mode::Answer);
        let input: Vec<bool> = (0..200).map(|i| (i * 37 + 11) % 5 < 2).collect();
        let a: Vec<bool> = input.iter().map(|&b| call.scramble(b)).collect();
        let b: Vec<bool> = input.iter().map(|&b| answer.scramble(b)).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn the_scrambler_is_undone_by_the_descrambler() {
        for mode in [Mode::Call, Mode::Answer] {
            let mut tx = Scrambler::new(mode);
            let mut rx = Scrambler::new(mode);
            let input: Vec<bool> = (0..2000).map(|i| (i * 37 + 11) % 5 < 2).collect();
            let out: Vec<bool> = input
                .iter()
                .map(|&b| rx.descramble(tx.scramble(b)))
                .collect();
            assert_eq!(out, input, "{mode:?}");
        }
    }

    #[test]
    fn the_descrambler_synchronises_itself_from_any_starting_state() {
        // The point of dividing rather than adding: a receiver that joins a
        // call already in progress needs no agreement about where the sequence
        // began, only the last twenty-three bits of what arrived.
        let mut tx = Scrambler::new(Mode::Call);
        let mut rx = Scrambler::new(Mode::Call);
        rx.register = 0x0055_aa55;
        let input: Vec<bool> = (0..500).map(|i| (i * 37 + 11) % 5 < 2).collect();
        let out: Vec<bool> = input
            .iter()
            .map(|&b| rx.descramble(tx.scramble(b)))
            .collect();
        // The first twenty-three are wrong, and everything after is right.
        assert_eq!(out[23..], input[23..]);
    }

    #[test]
    fn continuous_ones_do_not_come_out_as_a_constant() {
        // What the scrambler is for: a run of identical bits would otherwise
        // sit the transmitter on one point and give timing recovery nothing to
        // work from.
        let mut s = Scrambler::new(Mode::Call);
        let out: Vec<bool> = (0..2000).map(|_| s.scramble(true)).collect();
        let mut longest = 0;
        let mut run = 0;
        let mut previous = None;
        for b in out {
            if Some(b) == previous {
                run += 1;
            } else {
                run = 1;
                previous = Some(b);
            }
            longest = longest.max(run);
        }
        assert!(longest < 30, "a run of {longest} identical bits got through");
    }

    /// Which quadrant Y1Y2 names, from the codes Figure 1 puts against the
    /// four letters: A is 0001, B 0101, C 1101, D 1001.
    fn quadrant_of(y1: u8, y2: u8) -> usize {
        match (y1, y2) {
            (0, 0) => STATE_A,
            (0, 1) => STATE_B,
            (1, 1) => STATE_C,
            _ => STATE_D,
        }
    }

    #[test]
    fn the_sixteen_points_have_the_magnitudes_table_3_gives_them() {
        // The check that the constellation was read correctly out of a text
        // extraction that lost every sign in the table. These are the |Re| and
        // |Im| columns of Table 3, nonredundant coding, in the order the table
        // lists them: Y1 Y2 Q3 Q4 counting up from 0000. Nothing here depends
        // on a sign, which is the point -- the signs are the quadrant's, and
        // the quadrant comes from Y1Y2.
        const TABLE_3: [(f64, f64); 16] = [
            (1.0, 1.0), (3.0, 1.0), (1.0, 3.0), (3.0, 3.0), // Y1Y2 = 00
            (1.0, 1.0), (1.0, 3.0), (3.0, 1.0), (3.0, 3.0), // 01
            (1.0, 1.0), (1.0, 3.0), (3.0, 1.0), (3.0, 3.0), // 10
            (1.0, 1.0), (3.0, 1.0), (1.0, 3.0), (3.0, 3.0), // 11
        ];
        for (row, want) in TABLE_3.iter().enumerate() {
            let (y1, y2) = ((row >> 3) as u8 & 1, (row >> 2) as u8 & 1);
            let within = row & 0b11;
            let got = signal_point(quadrant_of(y1, y2), within);
            assert_eq!(
                (got.0.abs(), got.1.abs()),
                *want,
                "Y1Y2Q3Q4 = {y1}{y2}{:02b} came out at {got:?}",
                within
            );
        }
    }

    #[test]
    fn the_points_sit_in_the_quadrant_their_first_two_bits_name() {
        // The other half of the reading. Table 3 gives magnitudes; the signs
        // have to come from somewhere, and they come from the quadrant Y1Y2
        // names, which is the same quadrant 4800 bit/s would have landed in.
        for y1 in 0..2u8 {
            for y2 in 0..2u8 {
                let state = quadrant_of(y1, y2);
                let corner = STATES[state];
                for within in 0..4 {
                    let p = signal_point(state, within);
                    assert_eq!(
                        (p.0.signum(), p.1.signum()),
                        (corner.0.signum(), corner.1.signum()),
                        "{y1}{y2} with Q3Q4 {within:02b} left its quadrant"
                    );
                }
            }
        }
    }

    #[test]
    fn every_state_is_the_same_point_at_both_rates() {
        // 2.4.2 and the caption to Figure 1: the four states of 4800 bit/s are
        // a subset of the sixteen, and are the ones the whole start-up is
        // conducted in. If these ever came apart, a 9600 connection would
        // train on one constellation and carry data on another.
        for (state, &point) in STATES.iter().enumerate() {
            assert_eq!(signal_point(state, WITHIN_4800), point);
        }
    }

    #[test]
    fn the_labelling_survives_a_quarter_turn() {
        // Why the within-quadrant labelling has to rotate with the quadrant
        // rather than being fixed in the plane. A differentially coded
        // receiver resolves the constellation only up to a quarter turn, so if
        // Q3 and Q4 did not turn with it, a receiver that happened to land a
        // quadrant out would read every one of them wrong while the
        // differential decoding of Q1 and Q2 carried on perfectly.
        for state in 0..4 {
            for within in 0..4 {
                assert_eq!(
                    rotate(signal_point(state, within), 1),
                    signal_point((state + 1) & 3, within)
                );
            }
        }
    }

    #[test]
    fn the_sixteen_points_are_the_grid_and_are_all_different() {
        use std::collections::BTreeSet;
        let mut seen = BTreeSet::new();
        for state in 0..4 {
            for within in 0..4 {
                let (re, im) = signal_point(state, within);
                assert!(
                    [1.0, 3.0].contains(&re.abs()) && [1.0, 3.0].contains(&im.abs()),
                    "({re}, {im}) is not on the grid Table 3 describes"
                );
                seen.insert((re as i64, im as i64));
            }
        }
        assert_eq!(seen.len(), 16, "two labels landed on one point");
    }

    #[test]
    fn the_sixteen_have_the_same_mean_power_as_the_four() {
        // Which is why the receiver's gain control needs no telling about the
        // rate: it is holding the same number either way.
        let mut total = 0.0;
        for state in 0..4 {
            for within in 0..4 {
                let (re, im) = signal_point(state, within);
                total += re * re + im * im;
            }
        }
        assert!((total / 16.0 - CONSTELLATION_MEAN_POWER).abs() < 1e-12);
    }

    #[test]
    fn a_point_is_decided_as_the_one_it_is() {
        for state in 0..4 {
            for within in 0..4 {
                let p = signal_point(state, within);
                assert_eq!(nearest_point(p), (state, within));
                // And still, nudged a third of the way to a neighbour.
                let nudged = (p.0 + 0.6, p.1 - 0.6);
                assert_eq!(nearest_point(nudged), (state, within));
            }
        }
    }
}
