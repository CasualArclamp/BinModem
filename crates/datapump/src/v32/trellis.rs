//! The trellis-coded alternative at 9600 bit/s (V.32 2.4.1.2).
//!
//! 9600 has two modulations. The other one, 2.4.1.1, puts four bits on sixteen
//! points and is what [`super`] has carried until now; 1 e) makes it mandatory
//! for interworking, so it ought to be enough. Against a real modem it is not:
//! a V.32bis far end reads an E calling for 9600 without trellis and stops
//! transmitting one round trip later. This is the coding it will talk.
//!
//! Four information bits per symbol still, but on thirty-two points instead of
//! sixteen, with a fifth bit Y0 generated from the two differentially encoded
//! ones and three delay elements. The redundant bit buys back more than the
//! larger constellation costs: the thirty-two points sit as close as
//! d^2 = 2, but the four points sharing any one Y0 Y1 Y2 are d^2 = 16 apart,
//! and a decoder that follows the code's own state cannot be pushed off a path
//! by less than that.
//!
//! ## Where the numbers come from
//!
//! Every table here was read off the Recommendation's own figures rather than
//! out of extracted text, because the extraction loses the sign of every
//! coordinate -- Table 3 renders as magnitudes and a run of replacement
//! characters, and a constellation read from it would be wrong in a way no
//! test of our own two ends could notice.
//!
//! Figure 3 is a drawing, so the points were taken from where the labels sit
//! on the page against the axis ticks, and then checked twice over: the
//! magnitudes agree with Table 3 for all thirty-two, and the eight subsets
//! come out with the partition the code needs (see the tests). Figure 2, the
//! encoder, was read from the drawing itself.

/// Table 2: differential quadrant coding for the trellis alternative.
///
/// Indexed by `[Q1 Q2][previous Y1 Y2]`, giving `Y1 Y2`. Not the same table as
/// 4800 bit/s uses -- that is Table 1, and this one is only for 2.4.1.2.
const DIFFERENTIAL: [[u8; 4]; 4] = [
    [0b00, 0b01, 0b10, 0b11], // Q1 Q2 = 0 0
    [0b01, 0b00, 0b11, 0b10], // Q1 Q2 = 0 1
    [0b10, 0b11, 0b01, 0b00], // Q1 Q2 = 1 0
    [0b11, 0b10, 0b00, 0b01], // Q1 Q2 = 1 1
];

/// Figure 3: the thirty-two signal states, indexed by Y0 Y1 Y2 Q3 Q4 with Y0
/// most significant.
///
/// Coordinates are in the same units as Figure 1, so the mean power is 10 --
/// the same as the four-point and sixteen-point constellations, which is what
/// lets everything upstream of the mapping stay as it is.
const POINTS: [(f64, f64); 32] = [
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

/// The point a five-bit code names.
pub fn point(code: usize) -> (f64, f64) {
    POINTS[code & 31]
}

/// How many states the code has: three delay elements in Figure 2.
pub const STATES: usize = 8;

/// The state of the convolutional encoder, as `[T1, T2, T3]`.
type State = [u8; 3];

fn pack(s: State) -> usize {
    usize::from(s[0]) << 2 | usize::from(s[1]) << 1 | usize::from(s[2])
}

fn unpack(s: usize) -> State {
    [((s >> 2) & 1) as u8, ((s >> 1) & 1) as u8, (s & 1) as u8]
}

/// The convolutional encoder of Figure 2, one symbol.
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

/// Turns groups of four scrambled bits into signal points (2.4.1.2).
#[derive(Debug, Clone)]
pub struct Encoder {
    /// Y1 Y2 of the previous group, which Table 2 encodes against.
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

    /// One group of four bits, in the order they were scrambled, to the index
    /// of the point to transmit.
    pub fn encode(&mut self, q: [bool; 4]) -> usize {
        let q1q2 = usize::from(q[0]) << 1 | usize::from(q[1]);
        let y = DIFFERENTIAL[q1q2][usize::from(self.previous)];
        self.previous = y;
        let (y1, y2) = (y >> 1, y & 1);
        let (y0, next) = advance(self.state, y1, y2);
        self.state = next;
        usize::from(y0) << 4
            | usize::from(y1) << 3
            | usize::from(y2) << 2
            | usize::from(q[2]) << 1
            | usize::from(q[3])
    }
}

/// Table 2 undone: `[previous Y1 Y2][Y1 Y2]` gives back Q1 Q2.
///
/// Every row of Table 2 permutes the four quadrants, so this exists and is
/// unique -- which is also what makes the coding differential: a receiver that
/// has resolved the constellation only up to a quarter turn still recovers the
/// data, because only the change between one symbol and the next is read.
fn undo_differential(previous: u8, y: u8) -> u8 {
    for q in 0..4u8 {
        if DIFFERENTIAL[usize::from(q)][usize::from(previous)] == y {
            return q;
        }
    }
    unreachable!("Table 2 is a permutation of the quadrants")
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
    /// Y1 Y2 Q3 Q4, the four bits this transition carried.
    bits: u8,
}

/// Recovers groups of four bits from received points (clause 8, and 2.4.1.2
/// backwards).
///
/// A slicer would take the nearest of the thirty-two points and be wrong
/// whenever the noise exceeded d^2 = 2. This follows the code instead: the
/// only sequences it will consider are the ones the encoder could have
/// produced, and the closest wrong one of those is d^2 = 16 away in the
/// uncoded bits and further still in the coded ones.
#[derive(Debug, Clone)]
pub struct Decoder {
    metrics: [f64; STATES],
    history: std::collections::VecDeque<[Step; STATES]>,
    /// Y1 Y2 of the last group given back, which Table 2 is undone against.
    previous: u8,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        // Every state equally likely: the encoder starts at zero but a
        // receiver joins a connection already running.
        Self {
            metrics: [0.0; STATES],
            history: std::collections::VecDeque::with_capacity(DEPTH + 1),
            previous: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// How many symbols the decoder is holding before it will commit.
    pub const fn depth() -> usize {
        DEPTH
    }

    /// Offer one received point. Gives back a group of four bits once enough
    /// symbols have arrived for the paths to have merged.
    pub fn decode(&mut self, at: (f64, f64)) -> Option<[bool; 4]> {
        // What each of the eight subsets costs, and which of its four points
        // is the one being paid for. Q3 and Q4 are not coded, so this is the
        // whole of their decision.
        let mut subset = [(f64::INFINITY, 0u8); 8];
        for (k, best) in subset.iter_mut().enumerate() {
            for q in 0..4u8 {
                let (x, y) = POINTS[k << 2 | usize::from(q)];
                let d = (at.0 - x).powi(2) + (at.1 - y).powi(2);
                if d < best.0 {
                    *best = (d, q);
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
                    let (cost, q) = subset[k];
                    let metric = self.metrics[from] + cost;
                    let to = pack(to);
                    if metric < next[to] {
                        next[to] = metric;
                        step[to] = Step {
                            from: from as u8,
                            bits: y1 << 3 | y2 << 2 | q,
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
        let bits = oldest[usize::from(state)].bits;
        let (y1, y2) = ((bits >> 3) & 1, (bits >> 2) & 1);
        let y = y1 << 1 | y2;
        let q1q2 = undo_differential(self.previous, y);
        self.previous = y;
        Some([
            q1q2 & 2 != 0,
            q1q2 & 1 != 0,
            bits & 2 != 0,
            bits & 1 != 0,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The partition the code is built on (2.4.1.2 with Figure 3).
    ///
    /// Thirty-two points as close as d^2 = 2, but the four sharing a Y0 Y1 Y2
    /// are d^2 = 16 apart. Those four differ only in Q3 Q4, which the code does
    /// not protect, so that distance is the whole of what an uncoded bit gets
    /// -- and it is eight times the raw minimum. A single point read off the
    /// figure wrongly would show up here and nowhere else.
    #[test]
    fn every_subset_is_four_times_as_far_apart_as_the_constellation() {
        let d2 = |a: (f64, f64), b: (f64, f64)| {
            (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
        };
        let mut whole = f64::INFINITY;
        for (i, &a) in POINTS.iter().enumerate() {
            for &b in &POINTS[i + 1..] {
                whole = whole.min(d2(a, b));
            }
        }
        assert_eq!(whole, 2.0, "the thirty-two points sit at d^2 = 2");

        for subset in 0..8 {
            let pts: Vec<_> = (0..4).map(|q| POINTS[subset << 2 | q]).collect();
            let mut inside = f64::INFINITY;
            for i in 0..4 {
                for j in i + 1..4 {
                    inside = inside.min(d2(pts[i], pts[j]));
                }
            }
            assert_eq!(
                inside, 16.0,
                "subset {subset:03b} is not the partition the code needs: {pts:?}"
            );
        }
    }

    /// Every point is used exactly once, and the constellation is the cross of
    /// Figure 3: integer coordinates with an odd sum, none beyond four.
    #[test]
    fn the_thirty_two_points_are_the_cross_of_figure_3() {
        let mut seen = std::collections::HashSet::new();
        for &(x, y) in &POINTS {
            assert!(seen.insert((x as i32, y as i32)), "({x}, {y}) twice");
            assert!(x.abs() <= 4.0 && y.abs() <= 4.0);
            assert_eq!(
                (x as i32 + y as i32).rem_euclid(2),
                1,
                "({x}, {y}) is off the lattice"
            );
        }
        assert_eq!(seen.len(), 32);
    }

    /// The same mean power as everything else in this modem.
    ///
    /// Not a coincidence and worth keeping: the automatic gain control, the
    /// equaliser and the level meter are all set from the training segment,
    /// which is four points, and none of them has to be told that the data
    /// which follows is thirty-two.
    #[test]
    fn the_mean_power_is_the_one_the_receiver_is_already_scaled_to() {
        let mean: f64 =
            POINTS.iter().map(|(x, y)| x * x + y * y).sum::<f64>() / 32.0;
        assert!((mean - 10.0).abs() < 1e-12, "mean power {mean}");
    }

    /// Table 2 is a group: every Q1 Q2 permutes the four quadrants, so a
    /// receiver can always undo it.
    #[test]
    fn the_differential_table_is_reversible() {
        for (q, row) in DIFFERENTIAL.iter().enumerate() {
            let mut seen = [false; 4];
            for &y in row {
                let y = usize::from(y);
                assert!(!seen[y], "Q1Q2={q:02b} sends two quadrants to {y:02b}");
                seen[y] = true;
            }
        }
        // And 0 0 is the identity: no change of quadrant.
        for (previous, &y) in DIFFERENTIAL[0].iter().enumerate() {
            assert_eq!(usize::from(y), previous);
        }
    }

    /// Each state leads to four others, one per Y1 Y2, and all eight states
    /// are reachable. An encoder whose trellis collapsed would still encode
    /// and would decode to noise.
    #[test]
    fn the_trellis_is_the_eight_state_one_figure_2_draws() {
        let mut reached = [false; STATES];
        for s in 0..STATES {
            let mut next = std::collections::HashSet::new();
            for y1 in 0..2 {
                for y2 in 0..2 {
                    let (y0, n) = advance(unpack(s), y1, y2);
                    assert_eq!(
                        y0,
                        (s & 1) as u8,
                        "Y0 is T3's contents, which is the state's last bit"
                    );
                    next.insert(pack(n));
                    reached[pack(n)] = true;
                }
            }
            assert_eq!(next.len(), 4, "state {s} does not fan out to four");
        }
        assert!(reached.iter().all(|&r| r), "not every state is reachable");
    }

    /// A deterministic bit stream to encode, since the crate has no random
    /// number generator and a test should not depend on one.
    fn bits(n: usize) -> Vec<[bool; 4]> {
        let mut x = 0x1234_5678_9abc_def0u64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                [x & 1 != 0, x & 2 != 0, x & 4 != 0, x & 8 != 0]
            })
            .collect()
    }

    /// Encode, decode, get it back.
    #[test]
    fn what_the_encoder_sends_the_decoder_reads() {
        let input = bits(400);
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        let mut out = Vec::new();
        for group in &input {
            let code = enc.encode(*group);
            if let Some(got) = dec.decode(point(code)) {
                out.push(got);
            }
        }
        assert_eq!(out.len(), input.len() - Decoder::depth());
        assert_eq!(&out[..], &input[..out.len()]);
    }

    /// And gets it back through noise that would defeat a slicer.
    ///
    /// This is the whole reason for the code. The thirty-two points sit at
    /// d^2 = 2, so noise of half that amplitude puts a bare decision on the
    /// wrong point regularly; the decoder is choosing between sequences the
    /// encoder could have produced, and the nearest wrong one is far away.
    #[test]
    fn it_reads_through_noise_that_moves_points_past_their_neighbours() {
        const NOISE: f64 = 1.5;
        let input = bits(4000);
        let mut enc = Encoder::new();
        let mut dec = Decoder::new();
        let mut nearest_wrong = 0usize;
        let mut wrong = 0usize;
        let mut given = 0usize;
        let mut x = 0xdead_beef_cafe_1234u64;
        let mut noise = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        for (i, group) in input.iter().enumerate() {
            let code = enc.encode(*group);
            let (px, py) = point(code);
            // Uniform noise on each axis, reaching well beyond half the
            // distance between neighbouring points.
            let at = (px + NOISE * noise(), py + NOISE * noise());
            // What a decision without the code would have made of it.
            let slice = (0..32)
                .min_by(|&a, &b| {
                    let d = |c: usize| {
                        let (x, y) = POINTS[c];
                        (at.0 - x).powi(2) + (at.1 - y).powi(2)
                    };
                    d(a).total_cmp(&d(b))
                })
                .unwrap();
            if slice != code {
                nearest_wrong += 1;
            }
            if let Some(got) = dec.decode(at) {
                if got != input[given] {
                    wrong += 1;
                }
                given += 1;
            }
            let _ = i;
        }
        assert!(
            nearest_wrong * 10 > given,
            "the noise was too gentle to be worth the test: only              {nearest_wrong} of {given} symbols would have been sliced wrongly"
        );
        assert_eq!(
            wrong, 0,
            "{wrong} of {given} groups came back changed, where a bare              decision on the nearest point would have got {nearest_wrong}              of them wrong"
        );
    }

    /// The decoder does not need to be told where the encoder started.
    ///
    /// A modem joins a connection in the middle of it: the start-up hands over
    /// somewhere in the far end's scrambled ones and nothing says which state
    /// the encoder is in. The differential coding of Table 2 is what makes
    /// that recoverable, and the trellis converges on its own.
    #[test]
    fn a_decoder_that_joins_late_catches_up() {
        let input = bits(500);
        let mut enc = Encoder::new();
        let points: Vec<_> = input.iter().map(|g| point(enc.encode(*g))).collect();

        let skip = 101;
        let mut dec = Decoder::new();
        let mut out = Vec::new();
        for p in &points[skip..] {
            if let Some(got) = dec.decode(*p) {
                out.push(got);
            }
        }
        // The first group is the one whose Q1 Q2 is measured against a quadrant
        // the decoder never saw, so only that one is allowed to differ.
        let expected = &input[skip..skip + out.len()];
        let differing = out
            .iter()
            .zip(expected)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        assert!(
            differing.iter().all(|&i| i == 0),
            "still wrong after the first group: {differing:?}"
        );
    }
}
