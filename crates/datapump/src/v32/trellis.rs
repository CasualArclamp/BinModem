//! The trellis code of V.32 2.4.1.2 and V.32bis 2.3, at four rates.
//!
//! 9600 has two modulations. The other one, V.32 2.4.1.1, puts four bits on
//! sixteen points and is what [`super`] carried until the trellis arrived; 1 e)
//! makes it mandatory for interworking, so it ought to be enough. Against a
//! real modem it is not: a V.32bis far end reads an E calling for 9600 without
//! trellis and stops transmitting one round trip later. This is the coding it
//! will talk -- and, with the three constellations V.32bis adds, the coding
//! that carries 7200, 12 000 and 14 400 as well.
//!
//! One code, four sizes. V.32bis 2.3.1 to 2.3.4 say the same thing four times
//! over: the first two bits of each group are differentially encoded by
//! Table 1/V.32bis, those two go into the same systematic convolutional
//! encoder to make a redundant Y0, and Y0 with the information bits chooses a
//! point. Only the number of bits that ride through untouched changes -- one
//! at 7200, four at 14 400 -- which is exactly what Figure 1/V.32bis draws:
//! four parallel lines each labelled with the rates it exists for.
//!
//! The redundant bit buys back more than the larger constellation costs. The
//! points sit as close as the lattice allows, but the ones sharing a Y0 Y1 Y2
//! are four times further apart than that, and a decoder that follows the
//! code's own state cannot be pushed off a path by less than the larger
//! distance.
//!
//! ## Where the numbers come from
//!
//! Every table here was read off the Recommendations' own figures rather than
//! out of extracted text, because the extraction loses the sign of every
//! coordinate -- V.32's Table 3 renders as magnitudes and a run of replacement
//! characters, and the V.32bis figures come out as rows of labels with their
//! axes shuffled through them. A constellation read from either would be wrong
//! in a way no test of our own two ends could notice, because both ends would
//! be wrong together.
//!
//! `tools/read_constellation.py` reads them by position instead: the scale
//! from the spacing of the axis ticks, the origin from the constellation's own
//! quarter-turn symmetry, and every label placed against those. What it
//! produces is checked rather than trusted -- see the tests, which put every
//! constellation back through its own set partition and its own mean power.
//!
//! The strongest check is that the tool, run on Figure 2-3/V.32bis, gives back
//! exactly the thirty-two points that were read off V.32's Figure 3 by hand in
//! an earlier session and verified three other ways. Two documents, two
//! methods, the same table.
//!
//! Figure 2/V.32 and Figure 1/V.32bis are the same encoder, and both were read
//! from the drawing: the main row is `T1 -> + -> + -> T2 -> + -> + -> T3` with
//! Y0 taken from the last delay and carried back round to the first, the two
//! curved gates are ANDs and the squares exclusive-ors.


/// Table 2/V.32, which is Table 1/V.32bis: differential quadrant coding for
/// the trellis alternative.
///
/// Indexed by `[Q1 Q2][previous Y1 Y2]`, giving `Y1 Y2`. Not the same table as
/// 4800 bit/s uses -- that is Table 1/V.32, and this one is only for the
/// trellis coding.
const DIFFERENTIAL: [[u8; 4]; 4] = [
    [0b00, 0b01, 0b10, 0b11], // Q1 Q2 = 0 0
    [0b01, 0b00, 0b11, 0b10], // Q1 Q2 = 0 1
    [0b10, 0b11, 0b01, 0b00], // Q1 Q2 = 1 0
    [0b11, 0b10, 0b00, 0b01], // Q1 Q2 = 1 1
];

/// Figure 2-4/V.32bis: the sixteen points of 7200 bit/s, indexed by
/// Y0 Y1 Y2 Q3 with Y0 most significant.
///
/// Four by four on the odd integers, which averages 10 -- the same as
/// Figure 1/V.32's four points and sixteen, so nothing has to be scaled.
const POINTS_16: [(f64, f64); 16] = [
    (3.0,  -3.0), // 0000
    (-1.0,  1.0), // 0001
    (-3.0,  3.0), // 0010
    (1.0,  -1.0), // 0011
    (3.0,   1.0), // 0100
    (-1.0, -3.0), // 0101
    (-3.0, -1.0), // 0110
    (1.0,   3.0), // 0111
    (-1.0,  3.0), // 1000
    (3.0,  -1.0), // 1001
    (1.0,  -3.0), // 1010
    (-3.0,  1.0), // 1011
    (-3.0, -3.0), // 1100
    (1.0,   1.0), // 1101
    (3.0,   3.0), // 1110
    (-1.0, -1.0), // 1111
];

/// Figure 3/V.32, which is Figure 2-3/V.32bis: the thirty-two points of
/// 9600 bit/s, indexed by Y0 Y1 Y2 Q3 Q4 with Y0 most significant.
///
/// A cross rather than a square, on the points whose coordinates sum to an
/// odd number. Also mean power 10.
const POINTS_32: [(f64, f64); 32] = [
    (-4.0, 1.0),  // 00000
    (0.0, -3.0),  // 00001
    (0.0, 1.0),   // 00010
    (4.0, 1.0),   // 00011
    (4.0, -1.0),  // 00100
    (0.0, 3.0),   // 00101
    (0.0, -1.0),  // 00110
    (-4.0, -1.0), // 00111
    (-2.0, 3.0),  // 01000
    (-2.0, -1.0), // 01001
    (2.0, 3.0),   // 01010
    (2.0, -1.0),  // 01011
    (2.0, -3.0),  // 01100
    (2.0, 1.0),   // 01101
    (-2.0, -3.0), // 01110
    (-2.0, 1.0),  // 01111
    (-3.0, -2.0), // 10000
    (1.0, -2.0),  // 10001
    (-3.0, 2.0),  // 10010
    (1.0, 2.0),   // 10011
    (3.0, 2.0),   // 10100
    (-1.0, 2.0),  // 10101
    (3.0, -2.0),  // 10110
    (-1.0, -2.0), // 10111
    (1.0, 4.0),   // 11000
    (-3.0, 0.0),  // 11001
    (1.0, 0.0),   // 11010
    (1.0, -4.0),  // 11011
    (-1.0, -4.0), // 11100
    (3.0, 0.0),   // 11101
    (-1.0, 0.0),  // 11110
    (-1.0, 4.0),  // 11111
];

/// Figure 2-2/V.32bis: the sixty-four points of 12 000 bit/s, indexed by
/// Y0 Y1 Y2 Q3 Q4 Q5.
///
/// Eight by eight on the odd integers, which averages 42 in the figure's
/// own units and is scaled to 10 on the way out.
const POINTS_64: [(f64, f64); 64] = [
    (7.0,   1.0), // 000000
    (3.0,   5.0), // 000001
    (7.0,  -7.0), // 000010
    (-5.0,  5.0), // 000011
    (3.0,  -3.0), // 000100
    (-1.0,  1.0), // 000101
    (-1.0, -7.0), // 000110
    (-5.0, -3.0), // 000111
    (-7.0, -1.0), // 001000
    (-3.0, -5.0), // 001001
    (-7.0,  7.0), // 001010
    (5.0,  -5.0), // 001011
    (-3.0,  3.0), // 001100
    (1.0,  -1.0), // 001101
    (1.0,   7.0), // 001110
    (5.0,   3.0), // 001111
    (-1.0,  5.0), // 010000
    (-5.0,  1.0), // 010001
    (7.0,   5.0), // 010010
    (-5.0, -7.0), // 010011
    (3.0,   1.0), // 010100
    (-1.0, -3.0), // 010101
    (7.0,  -3.0), // 010110
    (3.0,  -7.0), // 010111
    (1.0,  -5.0), // 011000
    (5.0,  -1.0), // 011001
    (-7.0, -5.0), // 011010
    (5.0,   7.0), // 011011
    (-3.0, -1.0), // 011100
    (1.0,   3.0), // 011101
    (-7.0,  3.0), // 011110
    (-3.0,  7.0), // 011111
    (-5.0, -1.0), // 100000
    (-1.0, -5.0), // 100001
    (-5.0,  7.0), // 100010
    (7.0,  -5.0), // 100011
    (-1.0,  3.0), // 100100
    (3.0,  -1.0), // 100101
    (3.0,   7.0), // 100110
    (7.0,   3.0), // 100111
    (5.0,   1.0), // 101000
    (1.0,   5.0), // 101001
    (5.0,  -7.0), // 101010
    (-7.0,  5.0), // 101011
    (1.0,  -3.0), // 101100
    (-3.0,  1.0), // 101101
    (-3.0, -7.0), // 101110
    (-7.0, -3.0), // 101111
    (1.0,  -7.0), // 110000
    (5.0,  -3.0), // 110001
    (-7.0, -7.0), // 110010
    (5.0,   5.0), // 110011
    (-3.0, -3.0), // 110100
    (1.0,   1.0), // 110101
    (-7.0,  1.0), // 110110
    (-3.0,  5.0), // 110111
    (-1.0,  7.0), // 111000
    (-5.0,  3.0), // 111001
    (7.0,   7.0), // 111010
    (-5.0, -5.0), // 111011
    (3.0,   3.0), // 111100
    (-1.0, -1.0), // 111101
    (7.0,  -1.0), // 111110
    (3.0,  -5.0), // 111111
];

/// Figure 2-1/V.32bis: the hundred and twenty-eight points of 14 400
/// bit/s, indexed by Y0 Y1 Y2 Q3 Q4 Q5 Q6.
///
/// A cross again, on the odd-sum points, averaging 41 in the figure's own
/// units.
const POINTS_128: [(f64, f64); 128] = [
    (-8.0, -3.0), // 0000000
    (8.0,  -3.0), // 0000001
    (4.0,  -3.0), // 0000010
    (4.0,  -7.0), // 0000011
    (-4.0, -3.0), // 0000100
    (-4.0, -7.0), // 0000101
    (0.0,  -3.0), // 0000110
    (0.0,  -7.0), // 0000111
    (-8.0,  1.0), // 0001000
    (8.0,   1.0), // 0001001
    (4.0,   1.0), // 0001010
    (4.0,   5.0), // 0001011
    (-4.0,  1.0), // 0001100
    (-4.0,  5.0), // 0001101
    (0.0,   1.0), // 0001110
    (0.0,   5.0), // 0001111
    (8.0,   3.0), // 0010000
    (-8.0,  3.0), // 0010001
    (-4.0,  3.0), // 0010010
    (-4.0,  7.0), // 0010011
    (4.0,   3.0), // 0010100
    (4.0,   7.0), // 0010101
    (0.0,   3.0), // 0010110
    (0.0,   7.0), // 0010111
    (8.0,  -1.0), // 0011000
    (-8.0, -1.0), // 0011001
    (-4.0, -1.0), // 0011010
    (-4.0, -5.0), // 0011011
    (4.0,  -1.0), // 0011100
    (4.0,  -5.0), // 0011101
    (0.0,  -1.0), // 0011110
    (0.0,  -5.0), // 0011111
    (2.0,  -9.0), // 0100000
    (2.0,   7.0), // 0100001
    (2.0,   3.0), // 0100010
    (6.0,   3.0), // 0100011
    (2.0,  -5.0), // 0100100
    (6.0,  -5.0), // 0100101
    (2.0,  -1.0), // 0100110
    (6.0,  -1.0), // 0100111
    (-2.0, -9.0), // 0101000
    (-2.0,  7.0), // 0101001
    (-2.0,  3.0), // 0101010
    (-6.0,  3.0), // 0101011
    (-2.0, -5.0), // 0101100
    (-6.0, -5.0), // 0101101
    (-2.0, -1.0), // 0101110
    (-6.0, -1.0), // 0101111
    (-2.0,  9.0), // 0110000
    (-2.0, -7.0), // 0110001
    (-2.0, -3.0), // 0110010
    (-6.0, -3.0), // 0110011
    (-2.0,  5.0), // 0110100
    (-6.0,  5.0), // 0110101
    (-2.0,  1.0), // 0110110
    (-6.0,  1.0), // 0110111
    (2.0,   9.0), // 0111000
    (2.0,  -7.0), // 0111001
    (2.0,  -3.0), // 0111010
    (6.0,  -3.0), // 0111011
    (2.0,   5.0), // 0111100
    (6.0,   5.0), // 0111101
    (2.0,   1.0), // 0111110
    (6.0,   1.0), // 0111111
    (9.0,   2.0), // 1000000
    (-7.0,  2.0), // 1000001
    (-3.0,  2.0), // 1000010
    (-3.0,  6.0), // 1000011
    (5.0,   2.0), // 1000100
    (5.0,   6.0), // 1000101
    (1.0,   2.0), // 1000110
    (1.0,   6.0), // 1000111
    (9.0,  -2.0), // 1001000
    (-7.0, -2.0), // 1001001
    (-3.0, -2.0), // 1001010
    (-3.0, -6.0), // 1001011
    (5.0,  -2.0), // 1001100
    (5.0,  -6.0), // 1001101
    (1.0,  -2.0), // 1001110
    (1.0,  -6.0), // 1001111
    (-9.0, -2.0), // 1010000
    (7.0,  -2.0), // 1010001
    (3.0,  -2.0), // 1010010
    (3.0,  -6.0), // 1010011
    (-5.0, -2.0), // 1010100
    (-5.0, -6.0), // 1010101
    (-1.0, -2.0), // 1010110
    (-1.0, -6.0), // 1010111
    (-9.0,  2.0), // 1011000
    (7.0,   2.0), // 1011001
    (3.0,   2.0), // 1011010
    (3.0,   6.0), // 1011011
    (-5.0,  2.0), // 1011100
    (-5.0,  6.0), // 1011101
    (-1.0,  2.0), // 1011110
    (-1.0,  6.0), // 1011111
    (-3.0,  8.0), // 1100000
    (-3.0, -8.0), // 1100001
    (-3.0, -4.0), // 1100010
    (-7.0, -4.0), // 1100011
    (-3.0,  4.0), // 1100100
    (-7.0,  4.0), // 1100101
    (-3.0,  0.0), // 1100110
    (-7.0,  0.0), // 1100111
    (1.0,   8.0), // 1101000
    (1.0,  -8.0), // 1101001
    (1.0,  -4.0), // 1101010
    (5.0,  -4.0), // 1101011
    (1.0,   4.0), // 1101100
    (5.0,   4.0), // 1101101
    (1.0,   0.0), // 1101110
    (5.0,   0.0), // 1101111
    (3.0,  -8.0), // 1110000
    (3.0,   8.0), // 1110001
    (3.0,   4.0), // 1110010
    (7.0,   4.0), // 1110011
    (3.0,  -4.0), // 1110100
    (7.0,  -4.0), // 1110101
    (3.0,   0.0), // 1110110
    (7.0,   0.0), // 1110111
    (-1.0, -8.0), // 1111000
    (-1.0,  8.0), // 1111001
    (-1.0,  4.0), // 1111010
    (-5.0,  4.0), // 1111011
    (-1.0, -4.0), // 1111100
    (-5.0, -4.0), // 1111101
    (-1.0,  0.0), // 1111110
    (-5.0,  0.0), // 1111111
];

/// One rate's share of the trellis code: how many bits it carries and where
/// it puts them.
#[derive(Debug, Clone, Copy)]
pub struct Coded {
    /// Information bits per symbol. Three at 7200 and six at 14 400.
    pub bits: usize,
    /// Figure 2-1 to 2-4/V.32bis, in the coordinates the figures are drawn in.
    points: &'static [(f64, f64)],
    /// What those coordinates are multiplied by.
    ///
    /// The four figures are not drawn to one scale -- the two cross-shaped
    /// ones average 41 and 10 in their own units -- and a modem does not
    /// change its transmit level when it changes rate. So each is brought to
    /// the same mean power, and the tables stay as the figures have them so
    /// that they can be read against the figures.
    scale: f64,
    /// The square of the closest distance between two points, in the figure's
    /// units. Two for a cross and four for a square, at every size.
    closest_squared: f64,
}

impl Coded {
    /// Bits that ride through the encoder untouched: Q3 upwards.
    pub const fn uncoded(&self) -> usize {
        self.bits - 2
    }

    /// How many points the constellation has.
    pub fn size(&self) -> usize {
        self.points.len()
    }

    /// The point a `bits + 1` bit code names, at transmit scale.
    pub fn point(&self, code: usize) -> (f64, f64) {
        let (x, y) = self.points[code & (self.points.len() - 1)];
        (x * self.scale, y * self.scale)
    }

    /// How far a symbol may move before it is taken for another one.
    ///
    /// What the carrier loop's willingness to believe one symbol is scaled by:
    /// the closer the points, the noisier a decision is and the less any one
    /// of them should be allowed to say.
    pub fn closest(&self) -> f64 {
        self.closest_squared.sqrt() * self.scale
    }

    /// The furthest a point gets from the origin.
    ///
    /// What a scope has to draw inside. A cross constellation reaches well
    /// beyond its own root-mean-square -- at 14 400 by nearly half again --
    /// and a plot scaled to the average clips the corners off.
    pub fn peak(&self) -> f64 {
        self.points
            .iter()
            .map(|&(x, y)| (x * x + y * y).sqrt() * self.scale)
            .fold(0.0, f64::max)
    }

    /// The nearest point, as an index.
    ///
    /// Not how the data is decoded -- that is [`Decoder`], which follows the
    /// code rather than the point. This is for the equaliser, which needs
    /// something to measure this symbol against while it is still this symbol.
    pub fn nearest(&self, at: (f64, f64)) -> usize {
        let mut best = (f64::INFINITY, 0);
        for (code, &(x, y)) in self.points.iter().enumerate() {
            let (x, y) = (x * self.scale, y * self.scale);
            let d = (at.0 - x).powi(2) + (at.1 - y).powi(2);
            if d < best.0 {
                best = (d, code);
            }
        }
        best.1
    }
}

/// Every figure is normalised to the power of Figure 1/V.32, so that changing
/// rate does not change the level on the line.
const fn scaled(
    bits: usize,
    points: &'static [(f64, f64)],
    figure_power: f64,
    closest_squared: f64,
) -> Coded {
    // No square root in a constant, so the factor is written out and the tests
    // check every one of them against `CONSTELLATION_MEAN_POWER / power`.
    let scale = match () {
        _ if figure_power == 10.0 => 1.0,
        _ if figure_power == 41.0 => 0.4938647983247948,
        _ if figure_power == 42.0 => 0.4879500364742666,
        _ => panic!("no scale for this figure"),
    };
    Coded { bits, points, scale, closest_squared }
}

/// 7200 bit/s: three bits a symbol on Figure 2-4/V.32bis.
pub const AT_7200: Coded = scaled(3, &POINTS_16, 10.0, 4.0);
/// 9600: four, on Figure 3/V.32, which is Figure 2-3/V.32bis.
pub const AT_9600: Coded = scaled(4, &POINTS_32, 10.0, 2.0);
/// 12 000: five, on Figure 2-2/V.32bis.
pub const AT_12000: Coded = scaled(5, &POINTS_64, 42.0, 4.0);
/// 14 400: six, on Figure 2-1/V.32bis.
pub const AT_14400: Coded = scaled(6, &POINTS_128, 41.0, 2.0);

/// The coding a rate uses, for the rates that have one.
///
/// 4800 has no trellis alternative: V.32 2.4.1 gives it four points and two
/// bits, and V.32bis 2.3.5 repeats that unchanged.
pub fn for_rate(bits_per_second: u32) -> Option<Coded> {
    match bits_per_second {
        7200 => Some(AT_7200),
        9600 => Some(AT_9600),
        12_000 => Some(AT_12000),
        14_400 => Some(AT_14400),
        _ => None,
    }
}

/// How many states the code has: three delay elements in Figure 1/V.32bis.
pub const STATES: usize = 8;

/// The state of the convolutional encoder, as `[T1, T2, T3]`.
type State = [u8; 3];

fn pack(s: State) -> usize {
    usize::from(s[0]) << 2 | usize::from(s[1]) << 1 | usize::from(s[2])
}

fn unpack(s: usize) -> State {
    [((s >> 2) & 1) as u8, ((s >> 1) & 1) as u8, (s & 1) as u8]
}

/// The convolutional encoder of Figure 1/V.32bis, one symbol.
///
/// Read off the drawing. The main row is `T1 -> + -> + -> T2 -> + -> + -> T3`,
/// with Y0 taken from T3's output and carried back round to T1's input; the
/// two curved gates are ANDs and the four squares exclusive-ors, which the
/// symbol truth table beside the figure settles. The two AND terms are why
/// this code is not linear.
///
/// Returns the redundant bit for this symbol and the state that follows. Y0 is
/// the delay element's *current* contents, so it is decided before the inputs
/// of this symbol touch anything.
fn advance(state: State, y1: u8, y2: u8) -> (u8, State) {
    let [s1, s2, s3] = state;
    let y0 = s3;
    // The node between the third and fourth gates, which one AND gate reads.
    let w = s2 ^ y2;
    let next = [s3, s1 ^ y1 ^ y2 ^ (s3 & w), w ^ (y1 & s3)];
    (y0, next)
}

/// Turns groups of scrambled bits into signal points.
#[derive(Debug, Clone)]
pub struct Encoder {
    /// Y1 Y2 of the previous group, which Table 1/V.32bis encodes against.
    previous: u8,
    state: State,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    pub fn new() -> Self {
        Self { previous: 0, state: [0; 3] }
    }

    /// Start again from the state a fresh connection has.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// One group of bits, in the order they were scrambled, to the index of
    /// the point to transmit.
    ///
    /// `q` holds Q1 first. Its length is the rate's, and anything past it is
    /// not looked at -- the caller keeps one array for every rate.
    pub fn encode(&mut self, coded: &Coded, q: &[bool]) -> usize {
        let q1q2 = usize::from(q[0]) << 1 | usize::from(q[1]);
        let y = DIFFERENTIAL[q1q2][usize::from(self.previous)];
        self.previous = y;
        let (y1, y2) = (y >> 1, y & 1);
        let (y0, next) = advance(self.state, y1, y2);
        self.state = next;
        // Q3 upwards, in the order they came, below the three coded bits.
        let mut uncoded = 0usize;
        for &bit in &q[2..coded.bits] {
            uncoded = uncoded << 1 | usize::from(bit);
        }
        usize::from(y0) << coded.bits
            | usize::from(y1) << (coded.bits - 1)
            | usize::from(y2) << (coded.bits - 2)
            | uncoded
    }
}

/// Table 1/V.32bis undone: `[previous Y1 Y2][Y1 Y2]` gives back Q1 Q2.
///
/// Every row of the table permutes the four quadrants, so this exists and is
/// unique -- which is also what makes the coding differential: a receiver that
/// has resolved the constellation only up to a quarter turn still recovers the
/// data, because only the change between one symbol and the next is read.
fn undo_differential(previous: u8, y: u8) -> u8 {
    for q in 0..4u8 {
        if DIFFERENTIAL[usize::from(q)][usize::from(previous)] == y {
            return q;
        }
    }
    unreachable!("the table is a permutation of the quadrants")
}

/// How far back the decoder looks before committing to a symbol.
///
/// The paths through an eight-state trellis have merged long before this, and
/// the cost of it is latency: at 2400 baud, twenty-four symbols is 10 ms, which
/// is nothing beside the round trip of any line this modem will see.
const DEPTH: usize = 24;

/// One symbol's decision for one state.
#[derive(Debug, Clone, Copy, Default)]
struct Step {
    /// The state this path came from.
    from: u8,
    /// Y1 Y2 and the uncoded bits this transition carried, Y1 highest.
    bits: u8,
}

/// Recovers groups of bits from received points (V.32 clause 8, and 2.3
/// backwards).
///
/// A slicer would take the nearest point and be wrong whenever the noise
/// exceeded half the distance between two of them. This follows the code
/// instead: the only sequences it will consider are the ones the encoder could
/// have produced, and the closest wrong one of those is four times further
/// away in the uncoded bits and further still in the coded ones.
#[derive(Debug, Clone)]
pub struct Decoder {
    coded: Coded,
    metrics: [f64; STATES],
    history: std::collections::VecDeque<[Step; STATES]>,
    /// Y1 Y2 of the last group given back, which the table is undone against.
    previous: u8,
    /// What each subset costs this symbol, and which of its points is the one
    /// being paid for. Kept here rather than made afresh, because at 14 400 it
    /// is sixteen points in each of eight subsets, every symbol.
    subset: [(f64, u8); STATES],
}

impl Decoder {
    pub fn new(coded: Coded) -> Self {
        // Every state equally likely: the encoder starts at zero but a
        // receiver joins a connection already running.
        Self {
            coded,
            metrics: [0.0; STATES],
            history: std::collections::VecDeque::with_capacity(DEPTH + 1),
            previous: 0,
            subset: [(f64::INFINITY, 0); STATES],
        }
    }

    /// Change rate, which throws away everything held for the old one.
    pub fn set_coding(&mut self, coded: Coded) {
        *self = Self::new(coded);
    }

    pub fn reset(&mut self) {
        let coded = self.coded;
        *self = Self::new(coded);
    }

    /// How many symbols the decoder is holding before it will commit.
    pub const fn depth() -> usize {
        DEPTH
    }

    /// Offer one received point. Gives back a group of bits once enough
    /// symbols have arrived for the paths to have merged.
    ///
    /// The group is Q1 first, and only the rate's own number of them mean
    /// anything.
    pub fn decode(&mut self, at: (f64, f64)) -> Option<[bool; 6]> {
        let uncoded = self.coded.uncoded();
        let within = 1usize << uncoded;
        // What each of the eight subsets costs, and which of its points is the
        // one being paid for. The uncoded bits are not coded, so this is the
        // whole of their decision.
        for (k, best) in self.subset.iter_mut().enumerate() {
            *best = (f64::INFINITY, 0);
            for q in 0..within {
                let (x, y) = self.coded.point(k << uncoded | q);
                let d = (at.0 - x).powi(2) + (at.1 - y).powi(2);
                if d < best.0 {
                    *best = (d, q as u8);
                }
            }
        }

        let mut next = [f64::INFINITY; STATES];
        let mut step = [Step::default(); STATES];
        for from in 0..STATES {
            for y1 in 0..2u8 {
                for y2 in 0..2u8 {
                    let (y0, to) = advance(unpack(from), y1, y2);
                    let k = usize::from(y0) << 2
                        | usize::from(y1) << 1
                        | usize::from(y2);
                    let (cost, q) = self.subset[k];
                    let metric = self.metrics[from] + cost;
                    let to = pack(to);
                    if metric < next[to] {
                        next[to] = metric;
                        step[to] = Step {
                            from: from as u8,
                            bits: y1 << 7 | y2 << 6 | q,
                        };
                    }
                }
            }
        }
        // Metrics only ever grow, so the smallest comes off all of them. What
        // decides a path is the difference between them.
        let floor = next.iter().copied().fold(f64::INFINITY, f64::min);
        for m in &mut next {
            *m -= floor;
        }
        self.metrics = next;
        self.history.push_back(step);
        if self.history.len() <= DEPTH {
            return None;
        }

        // Walk the best path back to the oldest symbol still held.
        let mut state = self
            .metrics
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(s, _)| s as u8)
            .unwrap_or(0);
        // All but the oldest, so that `state` ends up being where the
        // surviving path stood at the symbol about to be given back rather
        // than one before it.
        let walk = self.history.len() - 1;
        for steps in self.history.iter().rev().take(walk) {
            state = steps[usize::from(state)].from;
        }
        let oldest = self.history.pop_front()?;
        let held = oldest[usize::from(state)].bits;
        let (y1, y2) = ((held >> 7) & 1, (held >> 6) & 1);
        let y = y1 << 1 | y2;
        let q1q2 = undo_differential(self.previous, y);
        self.previous = y;

        let mut group = [false; 6];
        group[0] = q1q2 & 2 != 0;
        group[1] = q1q2 & 1 != 0;
        for (i, slot) in group[2..self.coded.bits].iter_mut().enumerate() {
            *slot = held & (1 << (uncoded - 1 - i)) != 0;
        }
        Some(group)
    }
}

#[cfg(test)]
mod tests {
    use super::super::CONSTELLATION_MEAN_POWER;
    use super::*;

    /// The distance written into each constant is the one the constellation
    /// actually has, measured.
    #[test]
    fn the_closest_distance_is_what_it_says_it_is() {
        for (rate, coded) in EVERY {
            let mut closest = f64::INFINITY;
            for a in 0..coded.size() {
                for b in 0..coded.size() {
                    if a != b {
                        let (ax, ay) = coded.point(a);
                        let (bx, by) = coded.point(b);
                        closest = closest.min((ax - bx).powi(2) + (ay - by).powi(2));
                    }
                }
            }
            assert!(
                (closest.sqrt() - coded.closest()).abs() < 1e-9,
                "{rate}: says {} and is {}",
                coded.closest(),
                closest.sqrt()
            );
        }
        // And the four are in the order the rates are: the faster the rate,
        // the more crowded the constellation.
        let d: Vec<f64> = EVERY.iter().map(|(_, c)| c.closest()).collect();
        assert!(d[0] > d[1] && d[1] > d[2] && d[2] > d[3], "{d:?}");
    }

    const EVERY: [(u32, Coded); 4] = [
        (7200, AT_7200),
        (9600, AT_9600),
        (12_000, AT_12000),
        (14_400, AT_14400),
    ];

    /// Every constellation leaves the modem at the same power, whatever the
    /// figure it was drawn in.
    #[test]
    fn every_rate_is_sent_at_the_same_power() {
        for (rate, coded) in EVERY {
            let total: f64 = (0..coded.size())
                .map(|c| {
                    let (x, y) = coded.point(c);
                    x * x + y * y
                })
                .sum();
            let mean = total / coded.size() as f64;
            assert!(
                (mean - CONSTELLATION_MEAN_POWER).abs() < 1e-9,
                "{rate} averages {mean} rather than {CONSTELLATION_MEAN_POWER}"
            );
        }
    }

    /// A constellation drawn crookedly would not be closed under a quarter
    /// turn, and the differential coding depends on it being so.
    #[test]
    fn every_constellation_is_the_same_after_a_quarter_turn() {
        for (rate, coded) in EVERY {
            let points: Vec<(i64, i64)> = (0..coded.size())
                .map(|c| {
                    let (x, y) = coded.points[c];
                    (x as i64, y as i64)
                })
                .collect();
            let here: std::collections::HashSet<_> = points.iter().copied().collect();
            let turned: std::collections::HashSet<_> =
                points.iter().map(|&(x, y)| (-y, x)).collect();
            assert_eq!(here, turned, "{rate} is not symmetric");
            assert_eq!(here.len(), coded.size(), "{rate} has a point twice");
        }
    }

    /// The point of the code: the four -- or two, or sixteen -- points sharing
    /// one Y0 Y1 Y2 are far apart, even though the constellation as a whole is
    /// packed as tightly as it can be.
    ///
    /// This is Ungerboeck's set partitioning, and it is what the redundant bit
    /// buys. A constellation read wrongly would fail this even if its mean
    /// power came out right.
    #[test]
    fn the_subsets_are_far_apart_and_the_whole_is_not() {
        for (rate, coded) in EVERY {
            let raw = |a: usize, b: usize| {
                let (ax, ay) = coded.points[a];
                let (bx, by) = coded.points[b];
                (ax - bx).powi(2) + (ay - by).powi(2)
            };
            let mut closest = f64::INFINITY;
            for a in 0..coded.size() {
                for b in 0..coded.size() {
                    if a != b {
                        closest = closest.min(raw(a, b));
                    }
                }
            }
            let within = 1usize << coded.uncoded();
            let mut subset_closest = f64::INFINITY;
            for k in 0..STATES {
                for a in 0..within {
                    for b in 0..within {
                        if a != b {
                            subset_closest =
                                subset_closest.min(raw(k * within + a, k * within + b));
                        }
                    }
                }
            }
            // Four times the squared distance is six decibels, and it is
            // the same four at three of the rates -- which is what makes one
            // code serve all of them. 7200 does better because there is
            // nowhere else for it to go: eight subsets over sixteen points
            // leaves two in each, and the only way to put two points in a
            // subset of a four-by-four square is diagonally opposite corners.
            // Eight times the squared distance, which is nine decibels, and
            // the same eight at every rate. That is what makes one code serve
            // all four: each larger constellation is the smaller one's lattice
            // taken a level finer, so the partition keeps its depth as the
            // points crowd in.
            assert!(
                (subset_closest / closest - 8.0).abs() < 1e-9,
                "{rate}: closest {closest}, within a subset {subset_closest}, \
                 which is {} times rather than eight",
                subset_closest / closest
            );
        }
    }

    /// What goes in comes out, at every rate.
    #[test]
    fn a_group_survives_the_encoder_and_the_decoder() {
        for (rate, coded) in EVERY {
            let mut encoder = Encoder::new();
            let mut decoder = Decoder::new(coded);
            // A sequence long enough to fill the decoder and then some, and
            // not periodic in anything the code cares about.
            let mut lfsr = 0x1234_5678u32;
            let mut sent = Vec::new();
            let mut got = Vec::new();
            for _ in 0..600 {
                let mut group = [false; 6];
                for slot in group[..coded.bits].iter_mut() {
                    lfsr = lfsr.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    *slot = lfsr & 0x8000_0000 != 0;
                }
                sent.push(group);
                let code = encoder.encode(&coded, &group);
                if let Some(back) = decoder.decode(coded.point(code)) {
                    got.push(back);
                }
            }
            assert!(got.len() > 500, "{rate}: only {} came back", got.len());
            for (i, back) in got.iter().enumerate() {
                assert_eq!(
                    &back[..coded.bits],
                    &sent[i][..coded.bits],
                    "{rate}: group {i} came back changed"
                );
            }
        }
    }

    /// And survives noise that would defeat a slicer.
    ///
    /// The noise here is more than half the distance between neighbouring
    /// points, so taking the nearest one would be wrong regularly. Following
    /// the code is not.
    #[test]
    fn a_group_survives_noise_a_slicer_would_not() {
        for (rate, coded) in EVERY {
            let mut encoder = Encoder::new();
            let mut decoder = Decoder::new(coded);
            let mut lfsr = 0x9e37_79b9u32;
            let mut noise = 0x85eb_ca6bu32;
            let random = move |n: &mut u32| {
                *n = n.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f64::from(*n >> 8) / f64::from(1u32 << 24) - 0.5
            };
            // Enough noise, each way, that taking the nearest point is wrong
            // often: the decision boundary is half the distance between two
            // points, and this is a little over it, so a slicer crosses
            // regularly and never by much.
            let spread = coded.closest() * 1.1;

            let mut sent = Vec::new();
            let mut wrong = 0;
            let mut back = Vec::new();
            for _ in 0..2000 {
                let mut group = [false; 6];
                for slot in group[..coded.bits].iter_mut() {
                    lfsr = lfsr.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    *slot = lfsr & 0x8000_0000 != 0;
                }
                sent.push(group);
                let code = encoder.encode(&coded, &group);
                let (x, y) = coded.point(code);
                let heard = (x + random(&mut noise) * spread, y + random(&mut noise) * spread);
                if coded.nearest(heard) != code {
                    wrong += 1;
                }
                if let Some(g) = decoder.decode(heard) {
                    back.push(g);
                }
            }
            let slips = back
                .iter()
                .enumerate()
                .filter(|(i, g)| g[..coded.bits] != sent[*i][..coded.bits])
                .count();
            assert!(
                wrong > 50,
                "{rate}: the noise was too small to trouble a slicer ({wrong})"
            );
            // Not "fewer" -- none. Noise that a slicer gets wrong hundreds of
            // times in two thousand symbols does not defeat the code once,
            // because it never approaches the nine decibels between one
            // sequence the encoder could have produced and the next.
            assert_eq!(
                slips, 0,
                "{rate}: a slicer got {wrong} of {} wrong and the decoder {slips}",
                back.len()
            );
            // Not zero: enough noise to defeat a slicer a tenth of the time
            // will eventually defeat anything. An order of magnitude fewer is
            // the coding gain, and it is what the redundant bit was spent on.
            assert!(
                slips * 10 < wrong,
                "{rate}: a slicer got {wrong} of {} wrong and the decoder {slips}",
                back.len()
            );
        }
    }

    /// The differential coding is what lets a receiver that has the
    /// constellation a quarter turn out still read the data.
    #[test]
    fn a_quarter_turn_does_not_change_what_arrives() {
        for (rate, coded) in EVERY {
            let mut encoder = Encoder::new();
            let mut straight = Decoder::new(coded);
            let mut turned = Decoder::new(coded);
            let mut lfsr = 0xdead_beefu32;
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for _ in 0..300 {
                let mut group = [false; 6];
                for slot in group[..coded.bits].iter_mut() {
                    lfsr = lfsr.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    *slot = lfsr & 0x8000_0000 != 0;
                }
                let (x, y) = coded.point(encoder.encode(&coded, &group));
                if let Some(g) = straight.decode((x, y)) {
                    a.push(g);
                }
                if let Some(g) = turned.decode((-y, x)) {
                    b.push(g);
                }
            }
            assert!(!a.is_empty());
            // The first group after a turn is the one that cannot be right --
            // there is no previous symbol to have changed from.
            assert_eq!(a[1..], b[1..], "{rate}: a quarter turn changed the data");
        }
    }

    /// `for_rate` and the four constants agree, and 4800 has no trellis.
    #[test]
    fn the_rates_that_have_a_coding_have_the_right_one() {
        assert_eq!(for_rate(7200).unwrap().bits, 3);
        assert_eq!(for_rate(9600).unwrap().bits, 4);
        assert_eq!(for_rate(12_000).unwrap().bits, 5);
        assert_eq!(for_rate(14_400).unwrap().bits, 6);
        assert!(for_rate(4800).is_none(), "4800 has no trellis alternative");
        assert!(for_rate(2400).is_none());
        for (_, coded) in EVERY {
            assert_eq!(coded.size(), 1 << (coded.bits + 1));
        }
    }
}
