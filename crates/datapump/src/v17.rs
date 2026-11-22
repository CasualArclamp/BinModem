//! V.17: the fax modulation, which is V.32bis's modulation half-duplex.
//!
//! Everything that decides what a symbol looks like is shared with V.32bis
//! and lives in [`super::v32::trellis`]. Clause 2 gives V.17 2400 symbols a
//! second on an 1800 Hz carrier, eight-state trellis coding, Table 1's
//! differential quadrant coding, and constellations of 16, 32, 64 and 128
//! points for 7200, 9600, 12 000 and 14 400. Every one of those is word for
//! word what V.32bis does, and the constellations have been checked point by
//! point against the ones already here: read off V.17's own figures by
//! position, every set matches to within half a percent of the distance
//! between neighbouring points, which is the error of measuring a drawing.
//!
//! What is different is everything around the symbols. There is no start-up
//! handshake, because T.30 has already done the negotiating over V.21 at
//! 300 bit/s; there is no echo canceller, because only one end transmits at a
//! time; and there is no round trip to measure, because nothing is waiting
//! for an answer. In place of all of it there is a fixed training sequence
//! that the sender simply sends (clause 5), in one of two lengths: a long
//! train of 1.4 seconds to teach a receiver the line from nothing, and a
//! resync of 142 ms for a receiver that already knows it. T.30's Note 5 to
//! 5.1 says which goes where: the long one for a training check and the
//! first message after CTC/CTR, the short one for everything else.
//!
//! The transmitter is [`Transmitter`]. The receiver, [`Receiver`], is a
//! driver on the shared QAM core, `dsp::qam`, as V.32's is: segment 1's
//! quarter-turn alternation turning into segment 2 is the same event to the
//! core's hunt as V.32's S turning into S-bar, and segment 2 is known symbol
//! for symbol, as V.32's TRN is, so the equaliser is solved for outright.

mod receiver;
mod transmitter;

pub use receiver::{Receiver, Stage};
pub use transmitter::Transmitter;

use super::v32::trellis::{self, Coded};
use super::v32::{CONSTELLATION_RMS, Mode, Scrambler};

/// Symbols a second (clause 2).
pub const BAUD: f64 = 2400.0;
/// Carrier, in hertz (clause 2).
pub const CARRIER: f64 = 1800.0;

/// The scrambler of clause 4: 1 + x^-18 + x^-23.
///
/// One polynomial, not two: only one end is transmitting, so there is no
/// second direction to tell apart. It is the same polynomial V.32 gives the
/// calling modem, which means [`super::v32::Scrambler`] already has it.
pub const SCRAMBLER_TAPS: (u32, u32) = (18, 23);

/// The scrambler clause 4 asks for, from all zeros.
fn scrambler() -> Scrambler {
    Scrambler::new(Mode::Call)
}

/// The rates V.17 carries, fastest first.
pub const RATES: [u32; 4] = [14_400, 12_000, 9600, 7200];

/// Bits a symbol carries at each rate, which is what picks the constellation.
pub fn bits_per_symbol(rate: u32) -> Option<usize> {
    Rate::of(rate).map(Rate::bits)
}

/// The constellation for a rate.
pub fn coding_for(rate: u32) -> Option<Coded> {
    Rate::of(rate).map(Rate::coded)
}

/// One of the four rates, for the pumps' interfaces: T.30's DCS names one,
/// and the burst is at it from segment 4 to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rate {
    #[default]
    R14400,
    R12000,
    R9600,
    R7200,
}

impl Rate {
    pub fn of(bits_per_second: u32) -> Option<Self> {
        match bits_per_second {
            14_400 => Some(Self::R14400),
            12_000 => Some(Self::R12000),
            9600 => Some(Self::R9600),
            7200 => Some(Self::R7200),
            _ => None,
        }
    }

    pub fn bits_per_second(self) -> u32 {
        match self {
            Self::R14400 => 14_400,
            Self::R12000 => 12_000,
            Self::R9600 => 9600,
            Self::R7200 => 7200,
        }
    }

    /// Information bits a symbol carries: six at 14 400 down to three at
    /// 7200 (2.3.1 to 2.3.4).
    pub fn bits(self) -> usize {
        self.coded().bits
    }

    /// The trellis coding and constellation, Figures 2 to 5.
    pub fn coded(self) -> Coded {
        match self {
            Self::R14400 => trellis::AT_14400,
            Self::R12000 => trellis::AT_12000,
            Self::R9600 => trellis::AT_9600,
            Self::R7200 => trellis::AT_7200,
        }
    }

    /// How many points the constellation has.
    pub fn points(self) -> usize {
        self.coded().size()
    }

    /// How far out the constellation reaches with its mean power made one,
    /// which is how a scope has to be scaled to draw it.
    pub fn peak(self) -> f64 {
        self.coded().peak() / CONSTELLATION_RMS
    }

    /// How much louder the data is than the four training states.
    ///
    /// The figures are the only statement of it, as they are in V.32bis: each
    /// draws A, B, C and D in its own units, at (-6, -2) and its quarter
    /// turns (see [`State::point`]), a power of 40. The data around them
    /// averages 40 at 7200 and 9600, 42 at 12 000 (Figure 3) and 41 at
    /// 14 400 (Figure 2): a fifth and a tenth of a decibel above the
    /// training. `trellis` brings every constellation to one mean power, and
    /// is shared with V.32, so the difference goes back on here, as V.32's
    /// transmitter puts it back on for itself.
    pub fn lift(self) -> f64 {
        match self {
            Self::R14400 => (41.0f64 / 40.0).sqrt(),
            Self::R12000 => (42.0f64 / 40.0).sqrt(),
            Self::R9600 | Self::R7200 => 1.0,
        }
    }
}

/// Which of Table 3's two sequences a burst begins with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Training {
    /// For a receiver that has to learn the line from nothing: segments 1 to
    /// 4, 3344 symbol intervals.
    #[default]
    Long,
    /// For one that learned it on a long train earlier in the call and has
    /// only to find where the symbols fall and how the carrier is turned:
    /// 342 symbol intervals.
    Resync,
}

impl Training {
    /// Segment 2's length.
    pub fn equalizer(self) -> u64 {
        match self {
            Self::Long => train::EQUALIZER,
            Self::Resync => train::RESYNC_EQUALIZER,
        }
    }

    /// Segment 3's: only a long train has the bridge.
    pub fn bridge(self) -> u64 {
        match self {
            Self::Long => train::BRIDGE,
            Self::Resync => 0,
        }
    }

    /// Symbol intervals from the first of segment 1 to the last of segment
    /// 4: Table 3's total.
    pub fn symbols(self) -> u64 {
        train::ALTERNATIONS + self.equalizer() + self.bridge() + train::SCRAMBLED_ONES
    }

    /// Symbols of segment 2 onwards before segment 4 begins, which is where
    /// the channel's own rate takes over.
    pub fn before_segment_four(self) -> u64 {
        self.equalizer() + self.bridge()
    }
}

/// The segments of Table 3, in symbol intervals, as printed on the rendered
/// page (PDF page 10).
///
/// Long, they add to 3344, which at 2400 baud is 1393 ms -- the figure the
/// table gives, and the check that the four numbers have been read off it
/// correctly. The resync row is 256, 38 and 48, which add to the 342 and
/// 142 ms it prints. The same row also prints 64 in the bridge column, and
/// the extracted text of the page gives it as 256 / 2938 / 64 / 48 / 3342 /
/// 1142, digits the page does not draw at all. Neither 64 belongs to it: the
/// printed totals leave the bridge out, 5.1.3 says the bridge is "used only
/// during an initial long train", and 5.1.4 has the resync's differential
/// encoder start from "the last symbol of segment 2", which is only where
/// segment 4 starts if nothing comes between (fax-qam.md 5.1).
pub mod train {
    /// Segment 1: alternations between states A and B (5.1.1).
    pub const ALTERNATIONS: u64 = 256;
    /// Segment 2: the equaliser training signal (5.1.2), long.
    pub const EQUALIZER: u64 = 2976;
    /// Segment 2 of a resync.
    pub const RESYNC_EQUALIZER: u64 = 38;
    /// Segment 3: the bridge signal, sent only in a long train (5.1.3).
    pub const BRIDGE: u64 = 64;
    /// Segment 4: scrambled ones at the channel rate (5.1.4).
    pub const SCRAMBLED_ONES: u64 = 48;

    /// The whole of a long train.
    pub const LONG: u64 = ALTERNATIONS + EQUALIZER + BRIDGE + SCRAMBLED_ONES;
    /// The whole of a resync.
    pub const RESYNC: u64 = ALTERNATIONS + RESYNC_EQUALIZER + SCRAMBLED_ONES;

    /// The turn-off of Table 7: scrambled ones, then no energy.
    pub const TURN_OFF_ONES: u64 = 32;
    pub const TURN_OFF_QUIET: u64 = 48;

    /// Talker-echo protection (5.3): 185 to 200 ms of unmodulated carrier
    /// then 20 to 25 ms of silence, taken at the middle of each, 192.5 ms and
    /// 22.5 ms, as V.27 ter's is here.
    pub const ECHO_CARRIER: u64 = 462;
    pub const ECHO_QUIET: u64 = 54;
}

/// Table 4: how segment 2's dibits become signal states.
///
/// Differential encoding is off through this segment, so a dibit is a state
/// and not a change of state. The first bit in time is written first.
pub fn four_phase_state(dibit: u8) -> State {
    match dibit & 0b11 {
        0b00 => State::C,
        0b01 => State::D,
        0b11 => State::A,
        _ => State::B,
    }
}

/// Table 6: how segment 3's dibits change the state.
///
/// Quarter turns, given in the Recommendation as the pairs A/B, B/C, C/D,
/// D/A and so on alongside the angles, and the pairs are what is transcribed
/// here: 00 is +90 degrees, A to B.
pub fn bridge_turn(dibit: u8) -> u8 {
    match dibit & 0b11 {
        0b00 => 1, // A/B, B/C, C/D, D/A: a quarter turn one way.
        0b01 => 0, // A/A: none.
        0b10 => 2, // A/C: a half turn.
        _ => 3,    // A/D: a quarter turn the other way.
    }
}

/// Table 5: the sixteen bits of the bridge signal, sent eight times.
///
/// B0 is the first bit into the scrambler. Note 2 says bits 4 to 6, 8 to 10
/// and 12 to 14 are for further study and that a receiver shall ignore them,
/// so nothing here reads them back.
pub const BRIDGE_PATTERN: [bool; 16] = [
    false, false, false, false, false, false, false, true,
    false, false, false, true, false, false, false, true,
];

/// Table 4 as printed: the scrambler's first sixteen dibits in segment 2,
/// with binary ones going in, "00 01 00 01 ... 10 01 10 01", which it gives
/// as the states C D C D ... B D B D.
pub const TABLE_4: [u8; 16] = [
    0b00, 0b01, 0b00, 0b01, 0b00, 0b01, 0b00, 0b01,
    0b00, 0b01, 0b00, 0b01, 0b10, 0b01, 0b10, 0b01,
];

/// Table 1/V.17, differential coding for the trellis code: `[Q1 Q2][Y1 Y2
/// before]` gives Y1 Y2.
///
/// The same table as V.32bis's, read again off V.17's own page 2, and
/// needed here for one thing the shared encoder does not do: start from a
/// state other than 00 (5.1.4).
const TABLE_1: [[u8; 4]; 4] = [
    [0b00, 0b01, 0b10, 0b11],
    [0b01, 0b00, 0b11, 0b10],
    [0b10, 0b11, 0b01, 0b00],
    [0b11, 0b10, 0b00, 0b01],
];

/// The four signalling states the training uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    A,
    B,
    C,
    D,
}

impl State {
    pub const ALL: [State; 4] = [State::A, State::B, State::C, State::D];

    /// Where the state is, in the units the data constellations are at the
    /// power of: the same four points at every rate.
    ///
    /// This is what fax-qam.md 5.2 left open, and the rendered figures settle
    /// it. Measured with PyMuPDF from the drawings' own geometry -- the dots,
    /// the letter circles and the axis ticks as coordinates, each figure in
    /// its own units -- Figures 2, 3 and 4/V.17 (PDF pages 5, 6 and 7, for
    /// 14 400, 12 000 and 9600) put the centres of the circled letters
    /// within 0.4 of a unit of (-6, -2), (2, -6), (6, 2) and (-2, 6), and
    /// every one of those twelve circles is empty: none of the three
    /// lattices has a point there, the nearest dot being 0.9 to 1.7 units
    /// off. The letters are not labelling data points at all. They are drawn
    /// at positions of their own, the same positions in every figure, and in
    /// Figure 5 (PDF page 8, 7200), whose sixteen points on the odd integers
    /// do include (-6, -2) and its turns, the circles sit beside those four
    /// dots, 0.8 to 1.5 units off, because the dots are there to be named.
    /// The earlier reading tried to match each circle to its nearest dot,
    /// which is why no orbit fitted better than another.
    ///
    /// That is V.32bis exactly, whose constellations these are: its training
    /// states A to D are V.32's four at every rate, and at 9600 and above not
    /// points of the data constellation (spec.md 2.1, from Figures 2-1 to
    /// 2-4/V.32bis). In each figure's units (-6, -2) has power 40, which is
    /// the mean power of the 7200 and 9600 data, so in the units shared with
    /// `trellis` -- every constellation at mean power 10 -- the states are
    /// V.32's (-3, -1) and its quarter turns, and the data at 12 000 and
    /// 14 400 sits a little above them ([`Rate::lift`]).
    pub fn point(self) -> (f64, f64) {
        match self {
            Self::A => (-3.0, -1.0),
            Self::B => (1.0, -3.0),
            Self::C => (3.0, 1.0),
            Self::D => (-1.0, 3.0),
        }
    }

    /// The point at unit power, as a receiver's table has it.
    pub fn unit(self) -> dsp::Complex {
        let (x, y) = self.point();
        dsp::Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS)
    }

    /// Y1 Y2 of the state, as 7200's labels name it: Figure 5 prints A at
    /// 0110, B at 0101, C at 0010 and D at 0001, Q3 Y2 Y1 Y0 with Q3 first.
    ///
    /// What segment 4's differential encoder starts from (5.1.4). A quarter
    /// turn of any constellation moves Y1 Y2 along the same cycle -- the code
    /// is built to be turned -- so the naming holds at every rate, including
    /// the three where the states are not data points.
    pub fn y1y2(self) -> u8 {
        match self {
            Self::A => 0b11,
            Self::B => 0b01,
            Self::C => 0b10,
            Self::D => 0b00,
        }
    }

    /// A quarter turn on, which is how the four are related.
    pub fn turned(self, quarters: u8) -> Self {
        Self::ALL[(self as usize + quarters as usize) % 4]
    }
}

/// Segment 2, one state at a time: binary ones through the scrambler,
/// started from the state that makes Table 4's output, two bits to a
/// symbol and no differential coding (5.1.2).
///
/// One generator for both ends: the transmitter sends what this says, and a
/// receiver trains against what this says, as V.32's `TrnSequence` does.
#[derive(Debug, Clone)]
pub struct Conditioning {
    scrambler: Scrambler,
}

impl Default for Conditioning {
    fn default() -> Self {
        Self::new()
    }
}

impl Conditioning {
    pub fn new() -> Self {
        Self { scrambler: table_4_scrambler() }
    }

    /// The next dibit, the first bit in time the higher.
    pub fn next_dibit(&mut self) -> u8 {
        let first = self.scrambler.scramble(true);
        let second = self.scrambler.scramble(true);
        u8::from(first) << 1 | u8::from(second)
    }

    /// The next state.
    pub fn next_state(&mut self) -> State {
        four_phase_state(self.next_dibit())
    }

    /// The scrambler as the segment has left it, which the next one carries
    /// on with: 5.1.4's "the initial scrambler state is that state produced
    /// by the last symbol interval of the previous segment", and 5.1.3's
    /// bridge, which is scrambled, the same.
    pub fn into_scrambler(self) -> Scrambler {
        self.scrambler
    }
}

/// The scrambler state 5.1.2 asks for: the one that, with ones going in,
/// makes Table 4's output.
///
/// Worked out rather than searched for. The register is the last 23 bits
/// out, so the table's first 23 fix everything after them -- the other nine
/// the table prints are a check, and they pass -- and running the
/// recurrence `out(n) = 1 + out(n-18) + out(n-23)` backwards from them gives
/// the 23 before, which is the state. The shared scrambler has no way to be
/// set, but multiplying back puts what goes in straight into its register,
/// so descrambling those 23 leaves it there.
fn table_4_scrambler() -> Scrambler {
    const KNOWN: usize = 23;
    let mut out = [false; 2 * KNOWN];
    for (i, dibit) in TABLE_4.iter().enumerate() {
        for (j, slot) in [2 * i, 2 * i + 1].into_iter().enumerate() {
            if slot < KNOWN {
                out[KNOWN + slot] = dibit >> (1 - j) & 1 != 0;
            }
        }
    }
    // out[KNOWN + n] is output n; the 23 before output 0 are out[..KNOWN].
    for n in (0..KNOWN).rev() {
        out[KNOWN + n - 23] = !(out[KNOWN + n] ^ out[KNOWN + n - 18]);
    }
    let mut scrambler = scrambler();
    for &bit in &out[..KNOWN] {
        scrambler.descramble(bit);
    }
    scrambler
}

/// Segment 2 of a long train, every symbol: what a receiver trains against,
/// and the first 38 of which are a resync's.
pub fn segment_two() -> Vec<State> {
    let mut conditioning = Conditioning::new();
    (0..train::EQUALIZER).map(|_| conditioning.next_state()).collect()
}

/// The first group of segment 4 as the shared encoder must be given it, for
/// the differential coding to run on from `from` rather than from 00.
///
/// The shared encoder always begins as though the symbol before had Y1 Y2 =
/// 00, and Table 1's 00 column is Q1 Q2 itself. So the group it is handed
/// has its first two bits replaced by the Y1 Y2 that Table 1 gives against
/// `from`: it encodes those as they are, remembers them, and runs on from
/// there exactly as though it had started from `from` (5.1.4).
fn from_state(group: &mut [bool], from: State) {
    let q = usize::from(group[0]) << 1 | usize::from(group[1]);
    let y = TABLE_1[q][usize::from(from.y1y2())];
    group[0] = y & 0b10 != 0;
    group[1] = y & 0b01 != 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_segments_add_up_to_the_times_table_3_prints() {
        assert_eq!(train::LONG, 3344);
        assert_eq!(Training::Long.symbols(), 3344);
        let seconds = train::LONG as f64 / BAUD;
        assert!(
            (seconds - 1.393).abs() < 0.001,
            "a long train is {seconds:.3} s, and Table 3 says 1393 ms"
        );
        // The resync as the page prints it: 256, 38, no bridge, 48, and the
        // 342 and 142 ms of its total columns.
        assert_eq!(
            (train::ALTERNATIONS, Training::Resync.equalizer(), Training::Resync.bridge(), train::SCRAMBLED_ONES),
            (256, 38, 0, 48)
        );
        assert_eq!(train::RESYNC, 342);
        assert_eq!(Training::Resync.symbols(), 342);
        let seconds = train::RESYNC as f64 / BAUD;
        assert!((seconds - 0.142).abs() < 0.001, "a resync is {seconds:.4} s");
        // And the turn-off of Table 7: 32 and 48, 80 symbol intervals, 33 ms.
        let off = train::TURN_OFF_ONES + train::TURN_OFF_QUIET;
        assert_eq!(off, 80);
        assert!((off as f64 / BAUD - 0.033).abs() < 0.001);
    }

    #[test]
    fn every_rate_has_a_constellation_of_the_right_size() {
        for rate in RATES {
            let bits = bits_per_symbol(rate).expect("a rate V.17 carries");
            let coded = coding_for(rate).expect("and a constellation for it");
            // The redundant bit makes the set twice as big as the data.
            assert_eq!(coded.size(), 1 << (bits + 1), "{rate}");
            assert_eq!(coded.bits, bits, "{rate}");
            assert_eq!(Rate::of(rate).map(Rate::bits_per_second), Some(rate));
        }
        assert_eq!(Rate::of(4800), None);
    }

    #[test]
    fn at_7200_the_states_are_the_data_points_figure_5_labels_them() {
        // Figure 5/V.17 labels the four Q3 Y2 Y1 Y0: A 0110, B 0101, C 0010,
        // D 0001. `Coded` indexes Y0 Y1 Y2 Q3, the label backwards, and the
        // point it names there is the state's.
        let printed = [(State::A, 0b0110usize), (State::B, 0b0101), (State::C, 0b0010), (State::D, 0b0001)];
        for (state, label) in printed {
            let code = (0..4).fold(0, |a, i| a | ((label >> i & 1) << (3 - i)));
            assert_eq!(trellis::AT_7200.point(code), state.point(), "{state:?}");
            // And its Y1 Y2 is what the label says.
            let (y2, y1) = ((label >> 2) & 1, (label >> 1) & 1);
            assert_eq!(state.y1y2(), (y1 << 1 | y2) as u8, "{state:?}");
        }
    }

    #[test]
    fn above_7200_the_states_are_not_data_points() {
        // The empty circles of Figures 2 to 4: every state is at least half
        // the closest spacing from every point of the three larger sets.
        for rate in [Rate::R9600, Rate::R12000, Rate::R14400] {
            let coded = rate.coded();
            for state in State::ALL {
                let (x, y) = state.point();
                let nearest = (0..coded.size())
                    .map(|c| {
                        let (px, py) = coded.point(c);
                        ((px - x).powi(2) + (py - y).powi(2)).sqrt()
                    })
                    .fold(f64::INFINITY, f64::min);
                assert!(nearest > 0.4 * coded.closest(), "{rate:?} {state:?} is {nearest} from a point");
            }
        }
    }

    #[test]
    fn a_quarter_turn_of_a_state_turns_its_y1y2_as_table_1_does() {
        // Table 1's Q1 Q2 = 11 is the quarter turn the code is invariant to:
        // every state turned once has the Y1 Y2 that row gives.
        for state in State::ALL {
            assert_eq!(state.turned(1).y1y2(), TABLE_1[0b11][usize::from(state.y1y2())], "{state:?}");
        }
    }

    #[test]
    fn the_four_states_are_quarter_turns_of_one_another() {
        // The property that makes them usable for a differentially coded
        // training signal, and the one that catches a state read off the
        // wrong point: three of four can look plausible and still not turn.
        for state in State::ALL {
            let here = state.point();
            let next = state.turned(1).point();
            assert_eq!((-here.1, here.0), next, "{state:?} turned a quarter is not {:?}", state.turned(1));
        }
    }

    #[test]
    fn segment_two_maps_dibits_to_the_states_table_4_gives() {
        // 00 01 00 01 ... 10 01 10 01 comes out as C D C D ... B D B D, which
        // is the worked example under 5.1.2.
        let got: Vec<State> = TABLE_4.iter().map(|d| four_phase_state(*d)).collect();
        let want = [
            State::C, State::D, State::C, State::D, State::C, State::D,
            State::C, State::D, State::C, State::D, State::C, State::D,
            State::B, State::D, State::B, State::D,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn the_scrambler_starts_where_table_4_says_it_does() {
        // All sixteen printed dibits come out of the generator, not just the
        // twenty-three bits it was worked out from.
        let mut conditioning = Conditioning::new();
        let got: Vec<u8> = (0..16).map(|_| conditioning.next_dibit()).collect();
        assert_eq!(got, TABLE_4);
        // And from there on it is the ordinary scrambler with ones going in:
        // after 23 bits its output is its own register, so a descrambler fed
        // it reads back ones.
        let mut conditioning = Conditioning::new();
        let mut descrambler = scrambler();
        let bits: Vec<bool> = (0..200)
            .flat_map(|_| {
                let d = conditioning.next_dibit();
                [d & 2 != 0, d & 1 != 0]
            })
            .map(|b| descrambler.descramble(b))
            .collect();
        assert!(bits[23..].iter().all(|b| *b), "segment 2 is not scrambled ones");
    }

    #[test]
    fn segment_three_turns_by_the_quarters_table_6_gives() {
        assert_eq!(bridge_turn(0b01), 0, "A/A is no change");
        assert_eq!(bridge_turn(0b00), 1, "A/B is a quarter");
        assert_eq!(bridge_turn(0b10), 2, "A/C is a half");
        assert_eq!(bridge_turn(0b11), 3, "A/D is three quarters");
        assert_eq!(State::A.turned(bridge_turn(0b00)), State::B);
        assert_eq!(State::A.turned(bridge_turn(0b11)), State::D);
    }

    #[test]
    fn the_bridge_pattern_is_the_sixteen_bits_of_table_5() {
        let ones: Vec<usize> = BRIDGE_PATTERN.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect();
        assert_eq!(ones, vec![7, 11, 15], "B7, B11 and B15 and no others");
    }

    #[test]
    fn the_first_group_of_segment_4_runs_on_from_the_state_named() {
        // Encoding a group from `from` by hand with Table 1, and through the
        // shared encoder with the first two bits turned, gives the same Y1 Y2.
        for from in State::ALL {
            for q in 0..4u8 {
                let mut group = [q & 2 != 0, q & 1 != 0, false, false, false, false];
                from_state(&mut group, from);
                let mut encoder = trellis::Encoder::new();
                let code = encoder.encode(&trellis::AT_7200, &group);
                let y = (code >> 1) & 0b11;
                assert_eq!(y as u8, TABLE_1[usize::from(q)][usize::from(from.y1y2())], "{from:?} {q:02b}");
            }
        }
    }

    #[test]
    fn the_scrambler_is_the_one_v32_gives_its_calling_end() {
        // Which is the whole reason there is nothing to write for it.
        assert_eq!(SCRAMBLER_TAPS, (18, 23));
    }
}
