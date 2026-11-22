//! The trellis code of 9.6.3: subset labels, the symbol-to-bit converter, the
//! three convolutional encoders, the modulo encoder and superframe
//! synchronisation's bit inversions.
//!
//! Everything here was read off the PDF's figures. Figure 9 labels the sixteen
//! odd-coordinate points nearest the origin; the labelling is periodic under
//! shifts of (4, 4) and (4, -4), so the sixteen cover the whole grid. Three
//! properties of it carry the decoder, and each is checked below over the
//! whole superconstellation rather than assumed:
//!
//! - A quarter turn clockwise adds one to a label's low two bits and leaves
//!   the high bit alone.
//! - Every point of Figure 5's quarter has low bits 00.
//! - Moving a point by an even vector (2a, 2b) flips the label's low bit
//!   exactly when a + b is odd -- which is what the modulo encoder undoes.
//!
//! Together they mean a 4D symbol's two labels say its rotations: the first's
//! low bits are Z, and the low bits' parity across the pair is Y0 with the
//! bit inversion on it.

use super::constellation::Point;

/// Figure 9, for x and y each of -3, -1, 1 and 3, row by row from y = 3 down.
const FIGURE_9: [[u8; 4]; 4] = [
    // x = -3     -1     1      3
    [0b001, 0b110, 0b101, 0b010], // y = 3
    [0b100, 0b011, 0b000, 0b111], // y = 1
    [0b101, 0b010, 0b001, 0b110], // y = -1
    [0b000, 0b111, 0b100, 0b011], // y = -3
];

/// The 3-bit subset label of an odd-coordinate point.
pub fn label(point: Point) -> u8 {
    // Reduce each coordinate into -3..=3 by eights, which the labelling's
    // period allows.
    let column = |v: i32| ((v + 3).rem_euclid(8) / 2) as usize;
    let x = column(point.0);
    let y = 3 - column(point.1);
    FIGURE_9[y][x]
}

/// Table 13: Y4 Y3 Y2 Y1 for the labels of y(2m) (rows) and y(2m + 1)
/// (columns), Y1 the least significant bit.
const TABLE_13: [[u8; 8]; 8] = [
    [0b0000, 0b0000, 0b0001, 0b0001, 0b1000, 0b1000, 0b1001, 0b1001],
    [0b0011, 0b0010, 0b0010, 0b0011, 0b1011, 0b1010, 0b1010, 0b1011],
    [0b0101, 0b0101, 0b0100, 0b0100, 0b1101, 0b1101, 0b1100, 0b1100],
    [0b0110, 0b0111, 0b0111, 0b0110, 0b1110, 0b1111, 0b1111, 0b1110],
    [0b1000, 0b1000, 0b1001, 0b1001, 0b0000, 0b0000, 0b0001, 0b0001],
    [0b1011, 0b1010, 0b1010, 0b1011, 0b0011, 0b0010, 0b0010, 0b0011],
    [0b1101, 0b1101, 0b1100, 0b1100, 0b0101, 0b0101, 0b0100, 0b0100],
    [0b1110, 0b1111, 0b1111, 0b1110, 0b0110, 0b0111, 0b0111, 0b0110],
];

/// The symbol-to-bit converter of 9.6.3.1: [Y4, Y3, Y2, Y1] as bits 3 to 0.
pub fn convert(first: u8, second: u8) -> u8 {
    TABLE_13[first as usize & 7][second as usize & 7]
}

/// The three convolutional codes of 9.6.3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Code {
    #[default]
    States16,
    States32,
    States64,
}

impl Code {
    pub fn states(self) -> usize {
        match self {
            Self::States16 => 16,
            Self::States32 => 32,
            Self::States64 => 64,
        }
    }

    /// Which of Y1 to Y4 the code takes in: 16 states use Y1 and Y2, 32 add
    /// Y4, and 64 all four.
    pub fn inputs(self) -> u8 {
        match self {
            Self::States16 => 0b0011,
            Self::States32 => 0b1011,
            Self::States64 => 0b1111,
        }
    }

    /// Y0 out of a state: the last delay's output.
    pub fn output(self, state: u8) -> bool {
        match self {
            Self::States16 => state >> 3 & 1 == 1,
            Self::States32 => state >> 4 & 1 == 1,
            Self::States64 => state >> 5 & 1 == 1,
        }
    }

    /// The next state from `state` for inputs `y` ([Y4, Y3, Y2, Y1] as bits 3
    /// to 0). A state's bit i is the output of the delay i + 1 from the left of
    /// the figure.
    pub fn next(self, state: u8, y: u8) -> u8 {
        let bit = |s: u8, i: u32| s >> i & 1;
        let (y1, y2, y3, y4) = (y & 1, y >> 1 & 1, y >> 2 & 1, y >> 3 & 1);
        match self {
            // Figure 10: the output fed back into the first delay and into the
            // adder after it; Y2 into the first two adders, Y1 into the third.
            Self::States16 => {
                let (d1, d2, d3, d4) = (bit(state, 0), bit(state, 1), bit(state, 2), bit(state, 3));
                let n1 = d4;
                let n2 = d1 ^ y2 ^ d4;
                let n3 = d2 ^ y2;
                let n4 = d3 ^ y1;
                n1 | n2 << 1 | n3 << 2 | n4 << 3
            }
            // Figure 11: the output fed back into the first delay only; Y2, Y1,
            // Y4 and Y2 into the adders in turn.
            Self::States32 => {
                let (d1, d2, d3, d4, d5) = (bit(state, 0), bit(state, 1), bit(state, 2), bit(state, 3), bit(state, 4));
                let n1 = d5;
                let n2 = d1 ^ y2;
                let n3 = d2 ^ y1;
                let n4 = d3 ^ y4;
                let n5 = d4 ^ y2;
                n1 | n2 << 1 | n3 << 2 | n4 << 3 | n5 << 4
            }
            // Figure 12, traced wire by wire. The first delay's input is Y4
            // added to the first adder's output and to an AND of the third
            // delay's output with the third adder's; the second delay's is the
            // first adder's output, the fourth delay's, and Y3 added to an AND
            // of Y2 with the third delay's output; the fifth delay takes the
            // output back, and the sixth the fifth's output with Y2 and the
            // third delay's.
            Self::States64 => {
                let (d1, d2, d3, d4, d5, d6) =
                    (bit(state, 0), bit(state, 1), bit(state, 2), bit(state, 3), bit(state, 4), bit(state, 5));
                let s1 = d1 ^ d2;
                let s3 = d2 ^ y1;
                let n1 = y4 ^ s1 ^ (d3 & s3);
                let n2 = s1 ^ y3 ^ (y2 & d3) ^ d4;
                let n3 = s3 ^ d3;
                let n4 = d3;
                let n5 = d6;
                let n6 = d5 ^ y2 ^ d3;
                n1 | n2 << 1 | n3 << 2 | n4 << 3 | n5 << 4 | n6 << 5
            }
        }
    }
}

/// The modulo encoder of 9.6.3.3: whether c(2m)/2 and c(2m + 1)/2 have real
/// and imaginary parts summing to numbers of different parity. `c` is in the
/// grid's own units.
pub fn modulo(c0: (i64, i64), c1: (i64, i64)) -> bool {
    let parity = |c: (i64, i64)| (c.0 / 2 + c.1 / 2).rem_euclid(2);
    parity(c0) != parity(c1)
}

/// Table 12: the bit inversion at the start of half data frame `half` of a
/// superframe of `j` data frames.
pub fn inversion(j: usize, half: usize) -> bool {
    let pattern: &[u8] = if j == 8 { b"0111011111111010" } else { b"01110111111110" };
    pattern[half % pattern.len()] == b'1'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::constellation::{QUARTER, clockwise, quarter};

    fn grid() -> impl Iterator<Item = Point> {
        (-45..=45).step_by(2).flat_map(|x| (-45..=45).step_by(2).map(move |y| (x, y)))
    }

    #[test]
    fn the_labels_near_the_origin_are_figure_9s() {
        assert_eq!(label((1, 1)), 0b000);
        assert_eq!(label((3, 1)), 0b111);
        assert_eq!(label((-3, 3)), 0b001);
        assert_eq!(label((-1, -3)), 0b111);
        assert_eq!(label((3, -3)), 0b011);
        // And the period carries them out: (1, 1) and (5, 5) and (-7, 1).
        assert_eq!(label((5, 5)), 0b000);
        assert_eq!(label((-7, 1)), label((1, 1)));
    }

    #[test]
    fn the_labelling_has_the_three_properties_the_decoder_rests_on() {
        for p in grid() {
            let (a, b) = (label(p), label(clockwise(p, 1)));
            assert_eq!(b >> 2, a >> 2, "{p:?}");
            assert_eq!(b & 3, (a + 1) & 3, "{p:?} turned");
            for (dx, dy) in [(2, 0), (0, 2), (2, 2), (-2, 4), (4, 0), (6, -2)] {
                let moved = label((p.0 + dx, p.1 + dy));
                let odd = (dx / 2 + dy / 2) % 2 != 0;
                assert_eq!((moved ^ a) & 1 == 1, odd, "{p:?} by ({dx}, {dy})");
            }
        }
        for n in 0..QUARTER {
            assert_eq!(label(quarter(n)) & 3, 0, "label {n} at {:?}", quarter(n));
        }
    }

    #[test]
    fn table_13_uses_every_input_four_times() {
        let mut counts = [0; 16];
        for first in 0..8 {
            for second in 0..8 {
                counts[convert(first, second) as usize] += 1;
            }
        }
        assert_eq!(counts, [4; 16]);
        // Y4 is the two labels' high bits added.
        for first in 0..8u8 {
            for second in 0..8u8 {
                assert_eq!(convert(first, second) >> 3, (first >> 2) ^ (second >> 2));
            }
        }
    }

    #[test]
    fn each_code_is_a_proper_trellis() {
        for code in [Code::States16, Code::States32, Code::States64] {
            let n = code.states();
            // Every state is reached, and by as many inputs as leave each one.
            let inputs = 1 << code.inputs().count_ones();
            let mut arrivals = vec![0; n];
            for state in 0..n as u8 {
                for y in (0..16u8).filter(|y| y & !code.inputs() == 0) {
                    let next = code.next(state, y);
                    assert!((next as usize) < n);
                    arrivals[next as usize] += 1;
                }
            }
            assert!(arrivals.iter().all(|&a| a == inputs), "{code:?}: {arrivals:?}");
        }
    }

    /// The least squared distance between two odd-grid points with labels
    /// `a` and `b`, other than a point and itself.
    fn between(a: u8, b: u8) -> i64 {
        // One point labelled a; the labelling repeats, so one will do.
        let origin = grid().find(|&p| label(p) == a).expect("every label is used");
        grid()
            .filter(|&p| label(p) == b && p != origin)
            .map(|p| i64::from((p.0 - origin.0).pow(2) + (p.1 - origin.1).pow(2)))
            .min()
            .expect("every label is used")
    }

    /// The code's free distance, in units where neighbouring grid points are
    /// four apart: the least total distance between two paths through the
    /// trellis that part and meet again, or between two parallel branches.
    fn free_distance(code: Code) -> i64 {
        // For each 4D subset -- Y0 and the code's inputs, the other bits free
        // -- the label pairs in it.
        let pairs = |y0: bool, y: u8| -> Vec<(u8, u8)> {
            let mut out = Vec::new();
            for first in 0..8u8 {
                for second in 0..8u8 {
                    if convert(first, second) & code.inputs() == y && ((first ^ second) & 1 == 1) == y0 {
                        out.push((first, second));
                    }
                }
            }
            out
        };
        let mut same = [[0i64; 8]; 8];
        for a in 0..8u8 {
            for b in 0..8u8 {
                same[a as usize][b as usize] = if a == b { 0 } else { between(a, b) };
            }
        }
        let subset_distance = |pa: &[(u8, u8)], pb: &[(u8, u8)]| -> i64 {
            let mut best = i64::MAX;
            for &(a1, a2) in pa {
                for &(b1, b2) in pb {
                    best = best.min(same[a1 as usize][b1 as usize] + same[a2 as usize][b2 as usize]);
                }
            }
            best
        };
        let inputs: Vec<u8> = (0..16u8).filter(|y| y & !code.inputs() == 0).collect();
        // Parallel branches: two different points of one subset.
        let mut best = i64::MAX;
        for y0 in [false, true] {
            for &y in &inputs {
                for &(a1, a2) in &pairs(y0, y) {
                    for &(b1, b2) in &pairs(y0, y) {
                        let d = if (a1, a2) == (b1, b2) {
                            between(a1, a1).min(between(a2, a2))
                        } else {
                            same[a1 as usize][b1 as usize] + same[a2 as usize][b2 as usize]
                        };
                        best = best.min(d);
                    }
                }
            }
        }
        // Diverging paths, from every pair of start states (the 64-state code
        // is not linear), by Dijkstra over pairs of states.
        let n = code.states();
        let mut dist = vec![i64::MAX; n * n];
        let mut queue = std::collections::BinaryHeap::new();
        for s in 0..n as u8 {
            let y0 = code.output(s);
            for &ya in &inputs {
                for &yb in &inputs {
                    if ya == yb {
                        continue;
                    }
                    let d = subset_distance(&pairs(y0, ya), &pairs(y0, yb));
                    let (na, nb) = (code.next(s, ya), code.next(s, yb));
                    if na == nb {
                        best = best.min(d);
                        continue;
                    }
                    let at = na as usize * n + nb as usize;
                    if d < dist[at] {
                        dist[at] = d;
                        queue.push(std::cmp::Reverse((d, na, nb)));
                    }
                }
            }
        }
        while let Some(std::cmp::Reverse((d, a, b))) = queue.pop() {
            if d >= best || d > dist[a as usize * n + b as usize] {
                continue;
            }
            for &ya in &inputs {
                for &yb in &inputs {
                    let step = subset_distance(&pairs(code.output(a), ya), &pairs(code.output(b), yb));
                    let total = d + step;
                    let (na, nb) = (code.next(a, ya), code.next(b, yb));
                    if na == nb {
                        best = best.min(total);
                    } else if total < dist[na as usize * n + nb as usize] {
                        dist[na as usize * n + nb as usize] = total;
                        queue.push(std::cmp::Reverse((total, na, nb)));
                    }
                }
            }
        }
        best
    }

    #[test]
    fn the_codes_free_distances_are_four_and_five_times_the_uncoded() {
        // Uncoded, neighbouring points are 4 apart; the 4D codes give four
        // times that at 16 and 32 states and five at 64 -- less the half bit
        // a 2D symbol the code costs, the 4.5 and 5.5 dB such codes are known
        // for. A wire misread off Figure 12 takes the 64-state code's to 16.
        let d: Vec<i64> = [Code::States16, Code::States32, Code::States64].iter().map(|&c| free_distance(c)).collect();
        assert_eq!(d, vec![16, 16, 20]);
    }

    #[test]
    fn each_code_accepts_its_own_outputs_turned_a_quarter() {
        // A quarter turn of every channel output adds one to both labels' low
        // bits. For differential encoding to make the turn harmless, the
        // turned sequence has to be a path through the trellis too: the inputs
        // Table 13 gives for the turned labels have to follow from the
        // unturned ones, and some matching of states has to carry every branch
        // onto a branch with the same Y0. The AND gates of Figure 12 are there
        // for exactly this, and without either the matching does not exist.
        let turn = |label: u8| (label & 4) | ((label + 1) & 3);
        for code in [Code::States16, Code::States32, Code::States64] {
            let mut turned = [None::<u8>; 16];
            for first in 0..8u8 {
                for second in 0..8u8 {
                    let y = convert(first, second) & code.inputs();
                    let z = convert(turn(first), turn(second)) & code.inputs();
                    match turned[y as usize] {
                        None => turned[y as usize] = Some(z),
                        Some(earlier) => assert_eq!(earlier, z, "{code:?}: inputs {y:04b} turn two ways"),
                    }
                }
            }
            let n = code.states();
            let inputs: Vec<u8> = (0..16u8).filter(|y| y & !code.inputs() == 0).collect();
            let matched = (0..n as u8).any(|start| {
                let mut map = vec![None::<u8>; n];
                map[0] = Some(start);
                let mut queue = vec![0u8];
                while let Some(state) = queue.pop() {
                    let other = map[state as usize].expect("queued states are mapped");
                    if code.output(state) != code.output(other) {
                        return false;
                    }
                    for &y in &inputs {
                        let a = code.next(state, y);
                        let b = code.next(other, turned[y as usize].expect("every input is used"));
                        match map[a as usize] {
                            None => {
                                map[a as usize] = Some(b);
                                queue.push(a);
                            }
                            Some(mapped) if mapped != b => return false,
                            _ => {}
                        }
                    }
                }
                true
            });
            assert!(matched, "{code:?} is not invariant to a quarter turn");
        }
    }

    #[test]
    fn inputs_the_code_ignores_change_nothing() {
        for state in 0..16u8 {
            for y in 0..16u8 {
                assert_eq!(Code::States16.next(state, y), Code::States16.next(state, y & 0b0011));
            }
        }
        for state in 0..32u8 {
            for y in 0..16u8 {
                assert_eq!(Code::States32.next(state, y), Code::States32.next(state, y & 0b1011));
            }
        }
    }

    #[test]
    fn the_inversion_patterns_are_table_12s() {
        let eight: String = (0..16).map(|h| if inversion(8, h) { '1' } else { '0' }).collect();
        assert_eq!(eight, "0111011111111010");
        let seven: String = (0..14).map(|h| if inversion(7, h) { '1' } else { '0' }).collect();
        assert_eq!(seven, "01110111111110");
    }

    #[test]
    fn the_modulo_encoder_compares_the_parities_of_half_of_c() {
        assert!(!modulo((0, 0), (2, 2)));
        assert!(modulo((2, 0), (0, 0)));
        assert!(!modulo((-2, 4), (4, -2)));
        assert!(modulo((4, 0), (2, 4)));
    }
}
