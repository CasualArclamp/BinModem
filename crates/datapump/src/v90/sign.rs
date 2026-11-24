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

/// The four sign inversion rules of 5.4.5.5.
///
/// Figure 2's trellis says which may follow which: from state 0 only A, which
/// stays there, and B, which goes to state 1; from state 1 only C, back to 0,
/// and D, which stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// "Do nothing."
    A,
    /// "Invert all sign bits in spectral shaping frame j."
    B,
    /// "Invert even-numbered [tj(0), tj(2), etc.] sign bits."
    C,
    /// "Invert odd-numbered [tj(1), tj(3), etc.] sign bits."
    D,
}

impl Rule {
    /// The two rules allowed from a state (Figure 2).
    pub fn allowed(state: bool) -> [Self; 2] {
        if state { [Self::C, Self::D] } else { [Self::A, Self::B] }
    }

    /// The state after this rule: B and D arrive at state 1, A and C at 0.
    pub fn next(self) -> bool {
        matches!(self, Self::B | Self::D)
    }

    /// Whether this rule inverts bit `k` of its frame.
    pub fn inverts(self, k: usize) -> bool {
        match self {
            Self::A => false,
            Self::B => true,
            Self::C => k.is_multiple_of(2),
            Self::D => k % 2 == 1,
        }
    }
}

/// The spectral shaping metric's filter (5.4.5.6).
///
/// F(z) = (1 - b1 z^-1)(1 - b2 z^-1) / ((1 - a1 z^-1)(1 - a2 z^-1)), which is
/// the inverse of the shape the analogue modem wants. The shaper keeps the
/// power of the signal through it low, so the signal itself ends up weakest
/// where F is strongest.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShapeFilter {
    a1: f64,
    a2: f64,
    b1: f64,
    b2: f64,
    x1: f64,
    y1: f64,
    v1: f64,
}

impl ShapeFilter {
    /// From CP's a1, a2, b1 and b2, "in the 8-bit two's-complement format
    /// with 6 bits after the binary point".
    pub fn new(coefficients: [i8; 4]) -> Self {
        let q = |c: i8| f64::from(c) / 64.0;
        Self {
            a1: q(coefficients[0]),
            a2: q(coefficients[1]),
            b1: q(coefficients[2]),
            b2: q(coefficients[3]),
            ..Self::default()
        }
    }

    /// One sample in, and v[n] squared -- the step w[n] takes -- out:
    ///
    /// 1) y[n] = x[n] - b1 x[n-1] + a1 y[n-1]
    /// 2) v[n] = y[n] - b2 y[n-1] + a2 v[n-1]
    /// 3) w[n] = v^2[n] + w[n-1]
    pub fn step(&mut self, x: f64) -> f64 {
        let y = x - self.b1 * self.x1 + self.a1 * self.y1;
        let v = y - self.b2 * self.y1 + self.a2 * self.v1;
        self.x1 = x;
        self.y1 = y;
        self.v1 = v;
        v * v
    }
}

/// One shaping frame as the shaper sees it: the levels its symbols will have
/// and its initial sign assignment t.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapingFrame {
    pub magnitudes: Vec<f64>,
    pub t: Vec<bool>,
}

/// The digital modem's sign coding when shaping is on (5.4.5.2 to 5.4.5.5):
/// parsing into shaping frames, both differential codings, and the choice of
/// inversions that keeps the spectral metric low.
#[derive(Debug, Clone)]
pub struct Shaper {
    redundancy: Redundancy,
    /// ld, in shaping frames.
    lookahead: usize,
    state: bool,
    filter: ShapeFilter,
    odd: OddChain,
    chain: FrameChain,
}

impl Shaper {
    pub fn new(redundancy: Redundancy, lookahead: usize, coefficients: [i8; 4]) -> Self {
        Self {
            redundancy,
            lookahead: lookahead.min(3),
            // "The initial state of the spectral shaper does not affect the
            // performance of the analogue modem and is therefore left to the
            // implementor."
            state: false,
            filter: ShapeFilter::new(coefficients),
            odd: OddChain::new(),
            chain: FrameChain::new(),
        }
    }

    pub fn redundancy(&self) -> Redundancy {
        self.redundancy
    }

    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// Shaping frames per data frame.
    pub fn frames_per_data_frame(&self) -> usize {
        self.redundancy.frames().0
    }

    /// A data frame's S sign bits and the six levels its magnitudes map to,
    /// as the shaping frames the shaper will choose over. The chains move on:
    /// this is to be called once for each data frame, in order.
    pub fn prepare(&mut self, s: &[bool], levels: [f64; INTERVALS]) -> Vec<ShapingFrame> {
        let parsed = parse_to_frames(self.redundancy, s);
        let coded = self.odd.encode(&parsed);
        let t = self.chain.encode(&coded);
        let width = self.redundancy.frames().1;
        t.into_iter()
            .enumerate()
            .map(|(j, t)| ShapingFrame { magnitudes: levels[j * width..(j + 1) * width].to_vec(), t })
            .collect()
    }

    /// Choose the rule for `frames[0]`, looking at up to ld frames after it,
    /// and give the signs it goes out with (5.4.5.5).
    pub fn choose(&mut self, frames: &[ShapingFrame]) -> Vec<bool> {
        let depth = frames.len().min(self.lookahead + 1).max(1);
        let mut best: Option<(f64, Rule)> = None;
        // Every path through the trellis from the current state, `depth`
        // frames long: two ways out of every state.
        for path in 0..1usize << depth {
            let mut filter = self.filter;
            let mut state = self.state;
            let mut cost = 0.0;
            let mut first = Rule::A;
            for (depth_index, frame) in frames.iter().take(depth).enumerate() {
                let rule = Rule::allowed(state)[path >> depth_index & 1];
                if depth_index == 0 {
                    first = rule;
                }
                state = rule.next();
                for (k, (&m, &t)) in frame.magnitudes.iter().zip(&frame.t).enumerate() {
                    let positive = t ^ rule.inverts(k);
                    cost += filter.step(if positive { m } else { -m });
                }
            }
            if best.is_none_or(|(c, _)| cost < c) {
                best = Some((cost, first));
            }
        }
        let rule = best.map_or(Rule::A, |(_, r)| r);
        self.apply(&frames[0], rule)
    }

    /// Send `frame` with `rule`, whatever the metric says: for a test.
    pub fn apply(&mut self, frame: &ShapingFrame, rule: Rule) -> Vec<bool> {
        debug_assert!(Rule::allowed(self.state).contains(&rule), "{rule:?} from state {}", self.state);
        self.state = rule.next();
        frame
            .magnitudes
            .iter()
            .zip(&frame.t)
            .enumerate()
            .map(|(k, (&m, &t))| {
                let positive = t ^ rule.inverts(k);
                self.filter.step(if positive { m } else { -m });
                positive
            })
            .collect()
    }

    pub fn state(&self) -> bool {
        self.state
    }
}

/// The analogue modem's side of every sign coding, shaped or not.
///
/// It never needs to know which inversions the shaper chose. Undoing the
/// second differential coding leaves each shaping frame off by the
/// difference of two consecutive inversions, and the trellis keeps that
/// difference to one of four patterns: nothing, everything, the even bits or
/// the odd bits -- the same bit throughout the even positions, and the same
/// throughout the odd. Bit 0 is always even and was always zero, so it says
/// what the even bits are off by. And the odd bits are off by the same, once
/// the odd chain is undone across the frame boundary: within a frame their
/// errors cancel, and across one they come to the state before the last
/// frame against the state after this one -- which is also what the even bits
/// are off by, because an inversion of the even bits is a change of state.
#[derive(Debug, Clone, Default)]
pub struct SignDecoder {
    redundancy: Redundancy,
    plain: Differential,
    /// The previous shaping frame's received signs, and the last odd bit
    /// after the second differential coding was undone.
    last_t: Vec<bool>,
    last_odd: bool,
}

impl SignDecoder {
    pub fn new(redundancy: Redundancy) -> Self {
        Self { redundancy, ..Self::default() }
    }

    /// The S sign bits a data frame's six received signs carry.
    pub fn decode(&mut self, dollars: Signs) -> Vec<bool> {
        if self.redundancy == Redundancy::None {
            return self.plain.decode(dollars).to_vec();
        }
        let (count, width) = self.redundancy.frames();
        let mut out = Vec::with_capacity(self.redundancy.data_bits());
        for j in 0..count {
            let t = &dollars[j * width..(j + 1) * width];
            // p' = t xor the previous frame's t, position by position.
            let p: Vec<bool> = t
                .iter()
                .enumerate()
                .map(|(k, &bit)| bit ^ self.last_t.get(k).copied().unwrap_or(false))
                .collect();
            self.last_t = t.to_vec();
            // p'(0) was zero when it went: what it is now is how far off the
            // even bits are, and the first odd bit too.
            let off = p[0];
            for k in 1..width {
                if k % 2 == 1 {
                    let previous = if k == 1 { self.last_odd } else { p[k - 2] };
                    let bit = p[k] ^ previous ^ if k == 1 { off } else { false };
                    out.push(bit);
                } else {
                    out.push(p[k] ^ off);
                }
            }
            // The last odd bit of this frame is what the next frame's first
            // odd bit was chained to.
            self.last_odd = p[if width.is_multiple_of(2) { width - 1 } else { width - 2 }];
        }
        out
    }
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

    fn random_bits(seed: &mut u64, n: usize) -> Vec<bool> {
        (0..n)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                *seed & 1 == 1
            })
            .collect()
    }

    /// Whatever inversions the trellis allows, the analogue modem gets the
    /// sign bits back without knowing which were chosen.
    #[test]
    fn any_path_through_the_trellis_decodes_without_being_told_it() {
        for sr in [Redundancy::One, Redundancy::Two, Redundancy::Three] {
            let mut seed = 0x9e37_79b9_7f4a_7c15u64 ^ sr.spent() as u64;
            let mut shaper = Shaper::new(sr, 0, [0; 4]);
            let mut decoder = SignDecoder::new(sr);
            for frame in 0..400 {
                let s = random_bits(&mut seed, sr.data_bits());
                let frames = shaper.prepare(&s, [1.0; INTERVALS]);
                let mut dollars = Vec::new();
                for f in &frames {
                    // Any allowed rule, at random.
                    let pick = random_bits(&mut seed, 1)[0];
                    let rule = Rule::allowed(shaper.state())[usize::from(pick)];
                    dollars.extend(shaper.apply(f, rule));
                }
                let signs: Signs = std::array::from_fn(|i| dollars[i]);
                assert_eq!(decoder.decode(signs), s, "{sr:?} frame {frame}");
            }
        }
    }

    /// Unshaped, the decoder is 5.4.5.1's running difference.
    #[test]
    fn with_shaping_off_the_decoder_is_the_running_difference() {
        let mut seed = 7u64;
        let mut tx = Differential::new();
        let mut rx = SignDecoder::new(Redundancy::None);
        for _ in 0..100 {
            let bits = random_bits(&mut seed, 6);
            let s: Signs = std::array::from_fn(|i| bits[i]);
            assert_eq!(rx.decode(tx.encode(s)), bits);
        }
    }

    /// Figure 2: A and B from state 0, C and D from state 1.
    #[test]
    fn the_trellis_is_figure_two() {
        assert_eq!(Rule::allowed(false), [Rule::A, Rule::B]);
        assert_eq!(Rule::allowed(true), [Rule::C, Rule::D]);
        assert!(!Rule::A.next() && Rule::B.next() && !Rule::C.next() && Rule::D.next());
        let inverted = |r: Rule| (0..6).filter(|&k| r.inverts(k)).collect::<Vec<_>>();
        assert_eq!(inverted(Rule::A), Vec::<usize>::new());
        assert_eq!(inverted(Rule::B), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(inverted(Rule::C), vec![0, 2, 4]);
        assert_eq!(inverted(Rule::D), vec![1, 3, 5]);
    }

    /// The metric does what it is for: a filter strongest at DC pushes the
    /// signal's power away from DC, and more look-ahead pushes harder.
    #[test]
    fn shaping_takes_power_away_from_where_the_filter_is_strong() {
        let dc_share = |sr: Redundancy, lookahead: usize, shape: bool| {
            // b1 = b2 = -1: F(z) = (1 + z^-1)^2, strong at DC and nothing at
            // 4 kHz.
            let coefficients = if shape { [0, 0, -64, -64] } else { [0; 4] };
            let mut shaper = Shaper::new(sr, lookahead, coefficients);
            let mut decoder = SignDecoder::new(sr);
            let mut seed = 0x1234_5678u64;
            let mut out = Vec::new();
            let mut queue: std::collections::VecDeque<ShapingFrame> = std::collections::VecDeque::new();
            let mut sent: Vec<Vec<bool>> = Vec::new();
            let mut decoded: Vec<Vec<bool>> = Vec::new();
            for _ in 0..3000 {
                let s = random_bits(&mut seed, sr.data_bits());
                let magnitudes: [f64; INTERVALS] = std::array::from_fn(|i| 0.5 + 0.1 * i as f64);
                sent.push(s.clone());
                queue.extend(shaper.prepare(&s, magnitudes));
                let per = shaper.frames_per_data_frame();
                while queue.len() >= per + lookahead {
                    let mut signs = Vec::new();
                    for _ in 0..per {
                        let frames: Vec<ShapingFrame> = queue.iter().cloned().collect();
                        signs.extend(shaper.choose(&frames));
                        let f = queue.pop_front().unwrap();
                        let _ = f;
                    }
                    let dollars: Signs = std::array::from_fn(|i| signs[i]);
                    decoded.push(decoder.decode(dollars));
                    for (i, &p) in signs.iter().enumerate() {
                        out.push(if p { magnitudes[i] } else { -magnitudes[i] });
                    }
                }
            }
            // Every frame that went out came back.
            for (d, s) in decoded.iter().zip(&sent) {
                assert_eq!(d, s);
            }
            let mean = out.iter().sum::<f64>() / out.len() as f64;
            let low: f64 = out.windows(8).map(|w| w.iter().sum::<f64>().powi(2)).sum::<f64>() / out.len() as f64;
            let power: f64 = out.iter().map(|x| x * x).sum::<f64>() / out.len() as f64;
            let _ = mean;
            low / power / 8.0
        };
        for sr in [Redundancy::One, Redundancy::Two, Redundancy::Three] {
            let flat = dc_share(sr, 0, false);
            let shaped = dc_share(sr, 0, true);
            let deeper = dc_share(sr, 2, true);
            println!("{sr:?}: low-band share {flat:.3} unshaped, {shaped:.3} shaped, {deeper:.3} with look-ahead 2");
            assert!(shaped < 0.85 * flat, "{sr:?} did not shape: {shaped:.3} against {flat:.3}");
            assert!(deeper <= shaped + 0.01, "{sr:?}: look-ahead made it worse");
        }
    }
}
