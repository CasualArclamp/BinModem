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
//! Only 4800 bit/s is implemented here. At that rate the scrambled data is
//! taken two bits at a time and differentially encoded into a quadrant (2.4.2
//! with Table 1), one point to a quadrant, which makes it structurally the
//! same problem as V.22bis at 1200 and the right place to start. The 9600 and
//! 14 400 rates add more points and, in their trellis-coded forms, a
//! convolutional code over them.

pub mod startup;

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

/// Root mean square of the constellation, which is one radius here.
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
        }
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
        self.signal = signal;
        self.since_change = 0;
        self.rate_bit = 0;
        if signal == Signal::Trn {
            // 5.2.3: the scrambler starts from all zeros for the segment.
            self.scrambler.reset();
        }
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
            Signal::Rate(sequence) => {
                // 5.3: the 16 bits repeat, scrambled, and are differentially
                // encoded as data is.
                let mut dibit = [false; 2];
                for slot in &mut dibit {
                    let bit = sequence & (1 << (15 - self.rate_bit)) != 0;
                    self.rate_bit = (self.rate_bit + 1) % 16;
                    *slot = self.scrambler.scramble(bit);
                }
                return self.turn(dibit);
            }
            Signal::ScrambledOnes => {
                let mut dibit = [false; 2];
                for slot in &mut dibit {
                    let bit = if self.pending.is_empty() {
                        true
                    } else {
                        self.pending.remove(0)
                    };
                    *slot = self.scrambler.scramble(bit);
                }
                return self.turn(dibit);
            }
        };
        self.quadrant = state as u8;
        state
    }

    /// Apply the differential quadrant coding of Table 1 and land on a state.
    fn turn(&mut self, dibit: [bool; 2]) -> usize {
        let change = QUADRANT_CHANGE[usize::from(dibit[0]) << 1 | usize::from(dibit[1])];
        self.quadrant = (self.quadrant + change) & 3;
        self.quadrant as usize
    }

    fn next_symbol(&mut self) -> (f64, f64) {
        STATES[self.next_state()]
    }

    pub fn next_sample(&mut self) -> f64 {
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
        }
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
        let mean_power = self.agc.process(power);
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
        // delay stays outside it.
        let coarse = STATES[nearest_state(point)];
        let error = (point.1 * coarse.0 - point.0 * coarse.1)
            / (coarse.0 * coarse.0 + coarse.1 * coarse.1 + 1e-9);
        // Second order, so the seven hertz of offset 2.1 allows for is removed
        // rather than merely tracked.
        self.frequency += -1.5e-5 * error;
        self.frequency = self.frequency.clamp(-0.02, 0.02);
        self.phase += -0.008 * error + self.frequency;
        self.phase -= self.phase.floor();

        let normalized = (point.0 / CONSTELLATION_RMS, point.1 / CONSTELLATION_RMS);
        let equalized = self.equalizer.equalize(normalized);
        let scaled = (
            equalized.0 * CONSTELLATION_RMS,
            equalized.1 * CONSTELLATION_RMS,
        );
        let state = nearest_state(scaled);
        let decision = STATES[state];

        self.symbols += 1;
        if self.symbols > 64 && self.carrier {
            self.equalizer.adapt(
                equalized,
                (
                    decision.0 / CONSTELLATION_RMS,
                    decision.1 / CONSTELLATION_RMS,
                ),
            );
        }
        self.last_symbol = scaled;

        // Every state sits in its own quadrant, so the state index is the
        // quadrant and the turn between two of them is the difference.
        let quadrant = state as u8;
        let Some(previous) = self.quadrant.replace(quadrant) else {
            return;
        };
        let change = (quadrant + 4 - previous) & 3;
        let dibit = CHANGE_TO_DIBIT[change as usize];
        for bit in [dibit & 0b10 != 0, dibit & 0b01 != 0] {
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

    pub fn level(&self) -> f64 {
        self.level.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turn a point through whole quadrants.
    ///
    /// Only the tests need this. The transmitter turns by adding to an index
    /// instead, which it can do because the four states are exactly the
    /// quarter turns of the first: the invariant that makes that legitimate is
    /// what the test below checks.
    fn rotate(point: (f64, f64), quadrant: u8) -> (f64, f64) {
        match quadrant & 3 {
            0 => point,
            1 => (-point.1, point.0),
            2 => (-point.0, -point.1),
            _ => (point.1, -point.0),
        }
    }

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
}
