//! Sign assignment and the spectral shaper's coding (V.90 5.4.5, 5.4.6).
//!
//! The six PCM codes of a data frame carry their magnitudes in the mapper's
//! output and their signs here. How many of those six signs carry user data is
//! negotiated: S of them do and Sr of them are spent on shaping the transmitted
//! spectrum, with S + Sr = 6 (5.4.1). With Sr = 0 nothing is spent and
//! "spectral shaping is disabled".
//!
//! Every mode differentially encodes, and that is not incidental. A receiver
//! recovers the data from the *difference* between successive signs, so a run
//! of inverted signs cancels out -- which is what lets the shaper invert signs
//! to flatten the spectrum without disturbing what they carry. The trellis of
//! 5.4.5.5 exists to keep those inversions inside the set the differential
//! coding can undo.
//!
//! One thing worth reading twice, from 5.4.6: "a sign bit of 0 means the
//! transmitted PCM codeword will represent a negative voltage and a sign bit
//! of 1 means it will represent a positive voltage". A set bit is positive,
//! which is the opposite way round from the sign bit of almost everything
//! else, G.711's own octets included.

use super::INTERVALS;

/// How many of the six sign bits are spent on shaping (5.4.1).
///
/// "The redundancy, Sr, is specified by the analogue modem during training
/// procedures and can be 0, 1, 2 or 3."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Redundancy {
    /// Sr = 0, S = 6: shaping off.
    #[default]
    None,
    /// Sr = 1, S = 5: one six-bit shaping frame per data frame.
    One,
    /// Sr = 2, S = 4: two three-bit shaping frames.
    Two,
    /// Sr = 3, S = 3: three two-bit shaping frames.
    Three,
}

impl Redundancy {
    /// Sr.
    pub fn spent(self) -> usize {
        match self {
            Self::None => 0,
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
        }
    }

    /// S, the sign bits left for user data. 5.4.1: "S + Sr = 6".
    pub fn data_bits(self) -> usize {
        INTERVALS - self.spent()
    }

    /// How many shaping frames make up one data frame, and how long each is.
    ///
    /// Read straight off Table 3: one frame of six, two of three, three of
    /// two. The product is always six, which is the data frame.
    pub fn frames(self) -> (usize, usize) {
        match self {
            Self::None => (0, 0),
            Self::One => (1, 6),
            Self::Two => (2, 3),
            Self::Three => (3, 2),
        }
    }
}

/// The sign bits of one data frame, as 5.4.6 means them: true is positive.
pub type Signs = [bool; INTERVALS];

/// 5.4.5.1, the whole of shaping-disabled mode.
///
/// "$0 = s0 XOR ($5 of the previous data frame); and $i = si XOR $(i-1)".
/// One running chain across the whole connection, so the signs a receiver sees
/// carry the data in their differences rather than in themselves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Differential {
    /// $5 of the previous data frame, which is where the next frame starts.
    last: bool,
}

impl Differential {
    pub fn new() -> Self {
        Self::default()
    }

    /// Six input sign bits to six PCM code sign bits.
    pub fn encode(&mut self, s: Signs) -> Signs {
        let mut out = [false; INTERVALS];
        let mut previous = self.last;
        for (i, &si) in s.iter().enumerate() {
            out[i] = si ^ previous;
            previous = out[i];
        }
        self.last = previous;
        out
    }

    /// And back, which is what the analogue modem does.
    pub fn decode(&mut self, dollars: Signs) -> Signs {
        let mut out = [false; INTERVALS];
        let mut previous = self.last;
        for (i, &d) in dollars.iter().enumerate() {
            out[i] = d ^ previous;
            previous = d;
        }
        self.last = previous;
        out
    }
}

/// Table 3: where the S input sign bits sit inside the shaping frames.
///
/// Every shaping frame's bit 0 is a constant zero -- that is the redundancy,
/// one bit per frame, which is why Sr is also the number of frames. The rest
/// take s0, s1, ... in order.
pub fn parse_to_frames(sr: Redundancy, s: &[bool]) -> Vec<Vec<bool>> {
    let (count, width) = sr.frames();
    let mut out = Vec::with_capacity(count);
    let mut next = 0usize;
    for _ in 0..count {
        let mut frame = Vec::with_capacity(width);
        // "pj(0) = 0" in every column of Table 3.
        frame.push(false);
        for _ in 1..width {
            frame.push(s.get(next).copied().unwrap_or(false));
            next += 1;
        }
        out.push(frame);
    }
    out
}

/// Table 4: the odd bits are differentially encoded, the even ones are not.
///
/// Reading the three columns together gives one rule rather than three. Each
/// odd-numbered bit is added to the odd-numbered bit before it -- the previous
/// one in the frame where there is one, and otherwise the last odd bit of the
/// frame before, which is what carries the chain from one data frame to the
/// next. Sr = 1 has three odd bits in its one long frame; Sr = 2 and Sr = 3
/// have one each in their short ones, so for them every link of the chain
/// crosses a frame boundary.
#[derive(Debug, Clone, Copy, Default)]
pub struct OddChain {
    last: bool,
}

impl OddChain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Encode the frames of one data frame in place, returning p'.
    pub fn encode(&mut self, frames: &[Vec<bool>]) -> Vec<Vec<bool>> {
        let mut out = Vec::with_capacity(frames.len());
        for frame in frames {
            let mut coded = Vec::with_capacity(frame.len());
            for (k, &bit) in frame.iter().enumerate() {
                if k % 2 == 1 {
                    let value = bit ^ self.last;
                    self.last = value;
                    coded.push(value);
                } else {
                    coded.push(bit);
                }
            }
            out.push(coded);
        }
        out
    }

    /// The reverse.
    pub fn decode(&mut self, frames: &[Vec<bool>]) -> Vec<Vec<bool>> {
        let mut out = Vec::with_capacity(frames.len());
        for frame in frames {
            let mut plain = Vec::with_capacity(frame.len());
            for (k, &bit) in frame.iter().enumerate() {
                if k % 2 == 1 {
                    plain.push(bit ^ self.last);
                    self.last = bit;
                } else {
                    plain.push(bit);
                }
            }
            out.push(plain);
        }
        out
    }
}

/// The second differential encoding of 5.4.5.2 to 5.4.5.4.
///
/// "tj(k) = p'j(k) XOR t(j-1)(k)", frame against the frame before it, bit
/// position against the same bit position. Where a data frame holds more than
/// one shaping frame the chain runs through them in order, so t(j+1) is
/// measured against t(j) and not against the previous data frame.
#[derive(Debug, Clone, Default)]
pub struct FrameChain {
    last: Vec<bool>,
}

impl FrameChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&mut self, frames: &[Vec<bool>]) -> Vec<Vec<bool>> {
        let mut out = Vec::with_capacity(frames.len());
        for frame in frames {
            let coded: Vec<bool> = frame
                .iter()
                .enumerate()
                .map(|(k, &bit)| bit ^ self.last.get(k).copied().unwrap_or(false))
                .collect();
            self.last = coded.clone();
            out.push(coded);
        }
        out
    }

    pub fn decode(&mut self, frames: &[Vec<bool>]) -> Vec<Vec<bool>> {
        let mut out = Vec::with_capacity(frames.len());
        for frame in frames {
            let plain: Vec<bool> = frame
                .iter()
                .enumerate()
                .map(|(k, &bit)| bit ^ self.last.get(k).copied().unwrap_or(false))
                .collect();
            self.last = frame.clone();
            out.push(plain);
        }
        out
    }
}

/// Table 5: which shaping frame bit becomes which data frame sign bit.
///
/// For Sr = 1 the one frame maps straight across. For Sr = 2 and Sr = 3 the
/// frames follow one another through the six intervals, three bits each or two
/// bits each.
pub fn to_signs(frames: &[Vec<bool>]) -> Signs {
    let mut out = [false; INTERVALS];
    let mut at = 0usize;
    for frame in frames {
        for &bit in frame {
            if at < INTERVALS {
                out[at] = bit;
                at += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5.4.1: "S + Sr = 6", and Table 3's frames tile the data frame exactly.
    #[test]
    fn the_shaping_frames_fill_the_data_frame() {
        for sr in [Redundancy::None, Redundancy::One, Redundancy::Two, Redundancy::Three] {
            assert_eq!(sr.spent() + sr.data_bits(), INTERVALS);
            let (count, width) = sr.frames();
            if sr != Redundancy::None {
                assert_eq!(count * width, INTERVALS, "{sr:?} does not tile");
                // One redundant bit per frame is exactly Sr of them.
                assert_eq!(count, sr.spent());
            }
        }
        assert_eq!(Redundancy::One.frames(), (1, 6));
        assert_eq!(Redundancy::Two.frames(), (2, 3));
        assert_eq!(Redundancy::Three.frames(), (3, 2));
    }

    /// 5.4.5.1, worked by hand. "$0 = s0 XOR ($5 of the previous data frame)".
    #[test]
    fn the_signs_are_a_running_difference_across_frames() {
        let mut d = Differential::new();
        // Starting from nothing, all-zero data leaves all-zero signs.
        assert_eq!(d.encode([false; 6]), [false; 6]);
        // A single set bit flips everything after it and stays flipped into
        // the next frame, which is what makes it a running chain.
        let mut d = Differential::new();
        let out = d.encode([true, false, false, false, false, false]);
        assert_eq!(out, [true; 6]);
        let next = d.encode([false; 6]);
        assert_eq!(next, [true; 6], "the chain did not carry into the next frame");
    }

    /// Every frame comes back, including across the frame boundary.
    #[test]
    fn the_running_difference_undoes_itself() {
        let mut tx = Differential::new();
        let mut rx = Differential::new();
        for value in 0..64u8 {
            let s: Signs = std::array::from_fn(|i| value >> i & 1 == 1);
            let sent = tx.encode(s);
            assert_eq!(rx.decode(sent), s, "frame {value}");
        }
    }

    /// Table 3: bit 0 of every shaping frame is the redundant zero, and the
    /// rest take the input sign bits in order.
    #[test]
    fn the_input_signs_land_where_table_three_puts_them() {
        // Sr = 1: pj(0) = 0, pj(1) = s0 ... pj(5) = s4.
        let s = [true, false, true, true, false];
        let frames = parse_to_frames(Redundancy::One, &s);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], vec![false, true, false, true, true, false]);

        // Sr = 2: two frames of three, the second starting again with zero.
        let s = [true, false, true, true];
        let frames = parse_to_frames(Redundancy::Two, &s);
        assert_eq!(frames, vec![vec![false, true, false], vec![false, true, true]]);

        // Sr = 3: three frames of two.
        let s = [true, false, true];
        let frames = parse_to_frames(Redundancy::Three, &s);
        assert_eq!(
            frames,
            vec![vec![false, true], vec![false, false], vec![false, true]]
        );
    }

    /// Table 4: odd bits are chained, even bits are not.
    #[test]
    fn only_the_odd_bits_are_differentially_encoded() {
        let mut chain = OddChain::new();
        // One six-bit frame. Even positions pass through; odd positions
        // accumulate: 1, then 1^1 = 0, then 0^1 = 1.
        let coded = chain.encode(&[vec![false, true, true, true, true, true]]);
        assert_eq!(
            coded[0],
            vec![false, true, true, false, true, true],
            "even bits pass through and odd bits accumulate"
        );
    }

    /// And the chain carries across frames, which for Sr = 2 and Sr = 3 is
    /// where every link of it is.
    #[test]
    fn the_odd_chain_carries_from_one_shaping_frame_to_the_next() {
        let mut chain = OddChain::new();
        let coded = chain.encode(&[vec![false, true], vec![false, true], vec![false, true]]);
        // 1, then 1^1 = 0, then 0^1 = 1.
        assert_eq!(
            [coded[0][1], coded[1][1], coded[2][1]],
            [true, false, true]
        );
    }

    /// Both chains undo themselves, which is what a receiver relies on.
    #[test]
    fn both_chains_undo_themselves_over_a_long_run() {
        for sr in [Redundancy::One, Redundancy::Two, Redundancy::Three] {
            let mut tx_odd = OddChain::new();
            let mut tx_frame = FrameChain::new();
            let mut rx_frame = FrameChain::new();
            let mut rx_odd = OddChain::new();
            for value in 0..64u8 {
                let s: Vec<bool> = (0..sr.data_bits()).map(|i| value >> i & 1 == 1).collect();
                let parsed = parse_to_frames(sr, &s);
                let coded = tx_odd.encode(&parsed);
                let t = tx_frame.encode(&coded);
                // Straight back down the same two chains.
                let back_coded = rx_frame.decode(&t);
                assert_eq!(back_coded, coded, "{sr:?} frame chain, value {value}");
                let back = rx_odd.decode(&back_coded);
                assert_eq!(back, parsed, "{sr:?} odd chain, value {value}");
            }
        }
    }

    /// Table 5: the shaping frames land on the six sign bits in order.
    #[test]
    fn the_shaping_frames_map_onto_the_six_sign_bits() {
        // Sr = 2: tj(0..2) are $0..$2 and tj+1(0..2) are $3..$5.
        let signs = to_signs(&[vec![false, true, false], vec![true, true, false]]);
        assert_eq!(signs, [false, true, false, true, true, false]);
        // Sr = 3: two bits each.
        let signs = to_signs(&[vec![true, false], vec![false, true], vec![true, true]]);
        assert_eq!(signs, [true, false, false, true, true, true]);
    }
}
