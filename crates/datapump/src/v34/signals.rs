//! The signals of phases 3 and 4 (10.1.3), as constellation points.
//!
//! S and PP are fixed patterns. Everything else is scrambled bits: TRN is
//! scrambled ones mapped straight onto the points, and J, J', MP and E are
//! scrambled bits whose quadrant is differentially encoded, so a receiver that
//! has the constellation a quarter turn out still reads them right.

use super::constellation::{Point, clockwise, counterclockwise, quarter};
use crate::v32::{Mode, Scrambler};

/// Symbols of S in phase 3 (Figure 19), and of S-bar after it.
pub const S_SYMBOLS: usize = 128;
pub const S_BAR_SYMBOLS: usize = 16;

/// PP is six periods of a 48-symbol sequence (10.1.3.6).
pub const PP_PERIOD: usize = 48;
pub const PP_SYMBOLS: usize = 6 * PP_PERIOD;

/// TRN in phases 3 and 4 is "transmitted for at least 512T".
pub const TRN_SYMBOLS: usize = 512;

/// E is "a 20-bit sequence of binary ones" (10.1.3.2).
pub const E_BITS: usize = 20;

/// Table 18: J asking for the four-point constellation.
pub const J_FOUR: [bool; 16] = bits16("0000100110010001");
/// Table 18: J asking for the sixteen-point constellation.
pub const J_SIXTEEN: [bool; 16] = bits16("0000110110010001");
/// Table 19: J', which ends J and is sent once.
pub const J_PRIME: [bool; 16] = bits16("1111100110010001");

const fn bits16(text: &str) -> [bool; 16] {
    let bytes = text.as_bytes();
    let mut out = [false; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = bytes[i] == b'1';
        i += 1;
    }
    out
}

/// The constellation J asks the far end to use in phase 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Four,
    Sixteen,
}

impl Size {
    /// Bits a symbol carries.
    pub fn bits(self) -> usize {
        match self {
            Self::Four => 2,
            Self::Sixteen => 4,
        }
    }

    pub fn j(self) -> [bool; 16] {
        match self {
            Self::Four => J_FOUR,
            Self::Sixteen => J_SIXTEEN,
        }
    }
}

/// Point 0, turned counterclockwise by quarter turns.
fn point0_ccw(quarters: u32) -> Point {
    counterclockwise(quarter(0), quarters)
}

/// Symbol `n` of S: "alternating between point 0 ... and the same point
/// rotated counterclockwise by 90 degrees". Even symbols are point 0, so S of
/// 128 symbols "shall end with the transmission of point 0 rotated
/// counterclockwise by 90 degrees".
pub fn s(n: usize) -> Point {
    point0_ccw(if n.is_multiple_of(2) { 0 } else { 1 })
}

/// Symbol `n` of S-bar: point 0 rotated by 180 and by 270 degrees, and it
/// "shall begin with the transmission of point 0 rotated by 180 degrees".
pub fn s_bar(n: usize) -> Point {
    point0_ccw(if n.is_multiple_of(2) { 2 } else { 3 })
}

/// Symbol `i` of PP (10-1), a point on the unit circle.
///
/// With i = 4k + I: exp(j pi (kI + 4) / 6) when k mod 3 is 1, and
/// exp(j pi kI / 6) otherwise. The period is 48 because moving k on by 12 adds
/// 2 pi I to every angle.
pub fn pp(i: usize) -> (f64, f64) {
    let (k, big_i) = ((i % PP_PERIOD) / 4, (i % PP_PERIOD) % 4);
    let turns = if k % 3 == 1 { k * big_i + 4 } else { k * big_i };
    let angle = std::f64::consts::PI * turns as f64 / 6.0;
    (angle.cos(), angle.sin())
}

/// The transmitting side of TRN, J, J', MP and E: one scrambler and one
/// differential encoder carried through all of them.
#[derive(Debug, Clone)]
pub struct Sender {
    scrambler: Scrambler,
    /// Z of the last symbol sent, which J and MP are differentially encoded
    /// from.
    z: u32,
}

impl Sender {
    /// "The scrambler is initialized to zero prior to transmission of the TRN
    /// signal" (10.1.3.8), and the scrambler is this end's own: the call modem
    /// divides by GPC and the answer modem by GPA (clause 7).
    pub fn new(mode: Mode) -> Self {
        Self { scrambler: Scrambler::new(mode), z: 0 }
    }

    /// Start TRN again, with the scrambler back at zero.
    pub fn restart(&mut self) {
        self.scrambler.reset();
        self.z = 0;
    }

    fn scrambled(&mut self, bit: bool) -> u32 {
        u32::from(self.scrambler.scramble(bit))
    }

    /// One symbol of TRN: scrambled ones, not differentially encoded
    /// (10.1.3.8).
    pub fn trn(&mut self, size: Size) -> Point {
        let i = self.scrambled(true) | self.scrambled(true) << 1;
        // "The differential encoder shall be initialized using the final symbol
        // of the transmitted TRN sequence."
        self.z = i;
        match size {
            Size::Four => clockwise(quarter(0), i),
            Size::Sixteen => {
                let q = self.scrambled(true) | self.scrambled(true) << 1;
                clockwise(quarter(q as usize), i)
            }
        }
    }

    /// One symbol of J, J', MP or E: `bits` (two or four of them, first in
    /// time first) scrambled, the first two differentially encoded into a
    /// quadrant and the other two, if there are two more, choosing the point.
    pub fn differential(&mut self, bits: &[bool]) -> Point {
        let i = self.scrambled(bits[0]) | self.scrambled(bits[1]) << 1;
        self.z = (self.z + i) % 4;
        let label = if bits.len() == 4 { self.scrambled(bits[2]) | self.scrambled(bits[3]) << 1 } else { 0 };
        clockwise(quarter(label as usize), self.z)
    }

    /// A whole sequence of bits, as symbols of `size`.
    pub fn sequence(&mut self, bits: &[bool], size: Size) -> Vec<Point> {
        bits.chunks(size.bits())
            .map(|chunk| {
                if chunk.len() == size.bits() {
                    self.differential(chunk)
                } else {
                    // Padded with ones to a whole symbol. Nothing V.34 sends
                    // is ever other than a whole number of symbols long, since
                    // J is 16 bits, E 20 and MP 88 or 188.
                    let mut whole = chunk.to_vec();
                    whole.resize(size.bits(), true);
                    self.differential(&whole)
                }
            })
            .collect()
    }
}

/// The receiving side: decided points back to bits.
#[derive(Debug, Clone)]
pub struct Reader {
    descrambler: Scrambler,
    z: u32,
}

/// The nearest point of the four- or sixteen-point set to `symbol`, where the
/// four points sit at (+-1, +-1) and the sixteen at (+-1 or +-3, +-1 or +-3).
pub fn decide(symbol: (f64, f64), size: Size) -> Point {
    let axis = |v: f64| match size {
        Size::Four => if v < 0.0 { -1 } else { 1 },
        Size::Sixteen => {
            if v < -2.0 {
                -3
            } else if v < 0.0 {
                -1
            } else if v < 2.0 {
                1
            } else {
                3
            }
        }
    };
    (axis(symbol.0), axis(symbol.1))
}

/// Which quarter-constellation label and clockwise turn make `point`.
fn unmap(point: Point, size: Size) -> (u32, u32) {
    let labels = match size {
        Size::Four => 1,
        Size::Sixteen => 4,
    };
    for label in 0..labels {
        for turn in 0..4 {
            if clockwise(quarter(label as usize), turn) == point {
                return (label, turn);
            }
        }
    }
    (0, 0)
}

impl Reader {
    /// A reader for what the far end sends, which is scrambled by the far
    /// end's polynomial: `mode` is the far end's.
    pub fn new(mode: Mode) -> Self {
        Self { descrambler: Scrambler::new(mode), z: 0 }
    }

    /// Bits out of a TRN symbol, which should all descramble to ones once the
    /// descrambler has seen 23 of them.
    pub fn trn(&mut self, point: Point, size: Size) -> Vec<bool> {
        let (label, turn) = unmap(point, size);
        self.z = turn;
        let mut out = vec![self.descrambler.descramble(turn & 1 == 1), self.descrambler.descramble(turn & 2 == 2)];
        if size == Size::Sixteen {
            out.push(self.descrambler.descramble(label & 1 == 1));
            out.push(self.descrambler.descramble(label & 2 == 2));
        }
        out
    }

    /// Bits out of a differentially encoded symbol of J, MP or E.
    pub fn differential(&mut self, point: Point, size: Size) -> Vec<bool> {
        let (label, turn) = unmap(point, size);
        let i = (turn + 4 - self.z) % 4;
        self.z = turn;
        let mut out = vec![self.descrambler.descramble(i & 1 == 1), self.descrambler.descramble(i & 2 == 2)];
        if size == Size::Sixteen {
            out.push(self.descrambler.descramble(label & 1 == 1));
            out.push(self.descrambler.descramble(label & 2 == 2));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s_alternates_point_0_with_its_quarter_turn_and_s_bar_is_it_turned_round() {
        assert_eq!(s(0), (1, 1));
        assert_eq!(s(1), (-1, 1));
        assert_eq!(s(S_SYMBOLS - 1), (-1, 1), "S ends on point 0 turned 90 degrees");
        assert_eq!(s_bar(0), (-1, -1), "S-bar begins on point 0 turned 180");
        assert_eq!(s_bar(1), (1, -1));
        for n in 0..8 {
            assert_eq!(s_bar(n), (-s(n).0, -s(n).1));
        }
    }

    #[test]
    fn pp_is_on_the_unit_circle_repeats_every_48_and_correlates_to_a_spike() {
        for i in 0..PP_SYMBOLS {
            let (re, im) = pp(i);
            assert!((re * re + im * im - 1.0).abs() < 1e-12);
            let (a, b) = (pp(i), pp(i % PP_PERIOD));
            assert!((a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
        }
        // What makes it good for training an equaliser: against a copy of
        // itself shifted round by anything but a whole period, it averages to
        // nothing.
        for shift in 0..PP_PERIOD {
            let (mut re, mut im) = (0.0, 0.0);
            for i in 0..PP_PERIOD {
                let (a, b) = (pp(i), pp(i + shift));
                re += a.0 * b.0 + a.1 * b.1;
                im += a.1 * b.0 - a.0 * b.1;
            }
            let magnitude = (re * re + im * im).sqrt() / PP_PERIOD as f64;
            if shift == 0 {
                assert!((magnitude - 1.0).abs() < 1e-12);
            } else {
                assert!(magnitude < 1e-9, "shift {shift}: {magnitude}");
            }
        }
    }

    #[test]
    fn trn_j_and_mp_come_back_through_a_reader_at_both_sizes() {
        for size in [Size::Four, Size::Sixteen] {
            let mut sender = Sender::new(Mode::Answer);
            let mut reader = Reader::new(Mode::Answer);
            // TRN: scrambled ones, all ones again once the descrambler is in.
            let mut got = Vec::new();
            for _ in 0..600 {
                let point = sender.trn(size);
                got.extend(reader.trn(point, size));
            }
            assert!(got[23..].iter().all(|b| *b), "{size:?} TRN did not descramble to ones");
            // Then J, which carries straight on from TRN.
            let bits: Vec<bool> = Size::Sixteen.j().repeat(4);
            let points = sender.sequence(&bits, Size::Four);
            let mut back = Vec::new();
            for p in points {
                back.extend(reader.differential(p, Size::Four));
            }
            assert_eq!(back, bits, "{size:?} J");
            // And MP-like bits at the size J asked for.
            let bits: Vec<bool> = (0..88).map(|i| (i * 7) % 3 == 0).collect();
            let points = sender.sequence(&bits, size);
            let mut back = Vec::new();
            for p in points {
                back.extend(reader.differential(p, size));
            }
            assert_eq!(back, bits, "{size:?} MP");
        }
    }

    #[test]
    fn a_differential_symbol_reads_the_same_a_quarter_turn_out() {
        // Once the descrambler has 23 bits of the right history in it: TRN
        // is not differentially encoded, so a quarter turn spoils it, and
        // that is how a receiver knows the turn is there to take out.
        let mut sender = Sender::new(Mode::Call);
        let mut reader = Reader::new(Mode::Call);
        let bits: Vec<bool> = J_PRIME.repeat(3);
        for _ in 0..40 {
            let p = sender.trn(Size::Four);
            reader.trn(clockwise(p, 1), Size::Four);
        }
        let points = sender.sequence(&bits, Size::Four);
        let mut back = Vec::new();
        for p in points {
            back.extend(reader.differential(clockwise(p, 1), Size::Four));
        }
        assert_eq!(back[23..], bits[23..]);
    }

    #[test]
    fn the_patterns_are_tables_18_and_19() {
        let text = |b: &[bool; 16]| b.iter().map(|&x| if x { '1' } else { '0' }).collect::<String>();
        assert_eq!(text(&J_FOUR), "0000100110010001");
        assert_eq!(text(&J_SIXTEEN), "0000110110010001");
        assert_eq!(text(&J_PRIME), "1111100110010001");
    }
}
