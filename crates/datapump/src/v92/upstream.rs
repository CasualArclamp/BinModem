//! The analogue modem's data-mode encoder, end to end (6.3, 6.4): scrambled
//! bits in, line levels out, with the memory resets of 8.7.1, E2u (8.7.2), RM
//! through the same chain (8.7.4) and FB1u on the old parameters (8.7.7).
//!
//! V92-19 fills this in: one place where 6.3's scrambler, 6.4.1's
//! twelve-interval modulus encoder and 6.4.2 to 6.4.4's precoder, prefilter,
//! constellations and trellis are strung together in the order the
//! Recommendation prints them. Call [`UpEncoder::next_symbol`] once per line
//! symbol and it does the rest: at the first symbol of a data frame it takes K
//! bits from wherever they come from, scrambles them, runs the modulus encoder
//! to twelve Ki, and hands them to the chain one symbol at a time.
//!
//! ```text
//! bits -> GPA -> modulus encoder -> Ki[12] -> chain (precoder, prefilter, G) -> levels
//! ```
//!
//! The same chain carries every upstream sequence, which is why 8.7.1 has the
//! memories zeroed before B1u and why 8.7.4 sends RM "through the precoder"
//! rather than beside it: the digital modem's decoder sees one continuous
//! signal from Ru to the cleardown, and only this end knows where the
//! sequences change.

use crate::v32::{Mode, Scrambler};

use super::modulus::{self, FrameBits, Moduli12};
use super::precoder::{Chain, Symbol};
use super::{Parameters, UP_INTERVALS, up_bits};

/// The twelve-symbol upstream encoder: 6.3 and 6.4 in one object.
#[derive(Debug, Clone)]
pub struct UpEncoder {
    chain: Chain,
    /// The chain the last rate change moved away from, kept because FB1u is
    /// sent "using the parameters of the previous data mode" (8.7.7).
    old: Option<Chain>,
    modulus: modulus::Encoder,
    scrambler: Scrambler,
    moduli: Moduli12,
    /// K, the data bits in one data frame (6.1).
    bits: u32,
    /// The Ki of the frame being sent, and how many of its symbols have gone.
    frame: [u8; UP_INTERVALS],
    sent: usize,
    frames: u64,
}

impl UpEncoder {
    /// The encoder these parameters describe, with every memory zero and n =
    /// 0, which is the state 8.7.1 requires before B1u.
    ///
    /// The scrambler is GPA's, "1 + x^-5 + x^-23 ... regardless of which modem
    /// placed the call" (6.3), which is [`Mode::Answer`] in the V.32 terms this
    /// crate's scrambler is written in.
    pub fn new(parameters: &Parameters) -> Result<Self, &'static str> {
        let moduli = Moduli12::new(parameters.moduli);
        if !moduli.fits(parameters.bits()) {
            return Err("the data frame carries more bits than the moduli can hold");
        }
        Ok(Self {
            chain: Chain::new(parameters)?,
            old: None,
            modulus: modulus::Encoder::new(),
            scrambler: Scrambler::new(Mode::Answer),
            moduli,
            bits: parameters.bits(),
            frame: [0; UP_INTERVALS],
            sent: 0,
            frames: 0,
        })
    }

    /// A new data mode's parameters (9.9): the old chain is kept for FB1u, and
    /// the new one starts with its memories zeroed and n = 0, because B1u
    /// "shall begin data frame interval 0" all over again (8.7.1).
    pub fn retune(&mut self, parameters: &Parameters) -> Result<(), &'static str> {
        let moduli = Moduli12::new(parameters.moduli);
        if !moduli.fits(parameters.bits()) {
            return Err("the data frame carries more bits than the moduli can hold");
        }
        let chain = Chain::new(parameters)?;
        self.old = Some(std::mem::replace(&mut self.chain, chain));
        self.moduli = moduli;
        self.bits = parameters.bits();
        self.frame = [0; UP_INTERVALS];
        self.sent = 0;
        self.frames = 0;
        Ok(())
    }

    /// One line symbol of data mode, pulling bits from `bit` as they are
    /// needed. K of them go per twelve symbols, all at the frame's first.
    pub fn next_symbol(&mut self, mut bit: impl FnMut() -> bool) -> Symbol {
        if self.sent == 0 {
            self.frame = self.encode_frame(&mut bit);
            self.frames += 1;
        }
        let ki = self.frame[self.sent];
        self.sent = (self.sent + 1) % UP_INTERVALS;
        self.chain.step(ki)
    }

    /// One frame's K bits through GPA and 6.4.1, which is where every frame
    /// the encoder sends comes from.
    fn encode_frame(&mut self, bit: &mut impl FnMut() -> bool) -> [u8; UP_INTERVALS] {
        let mut bits = Vec::with_capacity(self.bits as usize);
        for _ in 0..self.bits {
            let raw = bit();
            bits.push(self.scrambler.scramble(raw));
        }
        // The frame's bits arrive b0 first, and b0 is "the least significant
        // bit of R" (6.4.1 step 1), which is the order `from_bits` keeps.
        self.modulus
            .encode(FrameBits::from_bits(&bits), &self.moduli)
            .expect("`new` proved the moduli carry this many bits")
    }

    /// B1u (8.7.1): 48 data frames of ones, "the scrambler, the differential
    /// encoder, the precoder and the prefilter memories ... initialized to
    /// zero", and the first symbol on data frame interval 0.
    pub fn b1u(&mut self) -> Vec<Symbol> {
        self.reset_all();
        self.frames_of(48, true)
    }

    /// E2u (8.7.2): one data frame of zeros, or thirteen symbols of it when
    /// CPd's bit 29 asks for the extension.
    ///
    /// The extra symbol is one more from the chain with its label held at the
    /// frame's first interval. That is what "extend E2u by 1 symbol" can mean
    /// on a chain whose interval comes from n: the symbol count moves on by
    /// thirteen, so everything after starts a symbol further from a frame
    /// boundary and the decoder has to be told, which is why bit 29 exists.
    pub fn e2u(&mut self, extended: bool) -> Vec<Symbol> {
        let mut out = self.frames_of(1, false);
        if extended {
            out.push(self.chain.step(0));
        }
        out
    }

    /// FB1u (8.7.7): 48 data frames of ones on the *previous* data mode's
    /// parameters, with nothing reset -- the memories of that chain are where
    /// the rate renegotiation left them, and the scrambler has not stopped.
    ///
    /// Refused when no [`Self::retune`] has left an old chain behind, because
    /// a rate renegotiation is the only thing that sends FB1u.
    pub fn fb1u(&mut self) -> Result<Vec<Symbol>, &'static str> {
        let Some(mut chain) = self.old.take() else {
            return Err("there is no previous data mode to send fb1u on");
        };
        let mut out = Vec::with_capacity(48 * UP_INTERVALS);
        for _ in 0..48 {
            // On the old mode's moduli, which the old chain was built for.
            let frame = self.encode_frame(&mut || true);
            for ki in frame {
                out.push(chain.step(ki));
            }
        }
        self.old = Some(chain);
        Ok(out)
    }

    /// RM or RM' through the same chain (8.7.4): twelve symbols carrying the
    /// pattern's Ki, with no data bits consumed and the modulus encoder's
    /// differential encoding not clocked, so the sign chain picks up where
    /// data mode left it. The precoder, prefilter and trellis carry over
    /// because it is the same chain.
    pub fn force_k(&mut self, pattern: [u8; UP_INTERVALS]) -> Vec<Symbol> {
        pattern.into_iter().map(|ki| self.chain.step(ki)).collect()
    }

    /// `frames` data frames from a constant input, for the sequences.
    fn frames_of(&mut self, frames: usize, ones: bool) -> Vec<Symbol> {
        let mut out = Vec::with_capacity(frames * UP_INTERVALS);
        for _ in 0..frames {
            let frame = self.encode_frame(&mut || ones);
            self.frames += 1;
            for ki in frame {
                out.push(self.chain.step(ki));
            }
        }
        self.sent = 0;
        out
    }

    /// Every memory zero, for B1u (8.7.1) and a fast parameter exchange
    /// (9.9.2.1.2): "the scrambler ... initialized to zero" and the
    /// differential encoder with it.
    pub fn reset_all(&mut self) {
        self.reset_scrambler_and_differential();
        self.chain.reset();
        self.frame = [0; UP_INTERVALS];
        self.sent = 0;
        self.frames = 0;
    }

    /// "The scrambler and the differential encoder shall be reset" (9.9.2.1.2),
    /// leaving the chain's own memories alone -- a fast parameter exchange
    /// continues the same signal.
    pub fn reset_scrambler_and_differential(&mut self) {
        self.scrambler.reset();
        self.modulus.reset();
    }

    /// K, the bits one data frame carries (6.1).
    pub fn frame_bits(&self) -> u32 {
        self.bits
    }

    /// The rate those bits come to, in bit/s: twelve symbols carry them and
    /// there are eight thousand symbols a second.
    pub fn rate(&self) -> u32 {
        self.bits * 8000 / UP_INTERVALS as u32
    }

    /// The rung of 6.1's ladder this encoder was last built for.
    pub fn drn(&self) -> u8 {
        drn_for_bits(self.bits).unwrap_or(0)
    }

    /// Data frames sent since the last reset, the sequences' included.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The differential encoder's d(f - 1), for a caller that has to carry it
    /// across something the encoder did not see.
    pub fn d(&self) -> bool {
        self.modulus.d()
    }

    /// Whether a previous data mode is still held for FB1u.
    pub fn has_old(&self) -> bool {
        self.old.is_some()
    }

    /// The chain, for a caller that wants to look at what it just produced.
    pub fn chain(&self) -> &Chain {
        &self.chain
    }
}

/// The rung of 6.1's ladder these bits per frame stand for. The ladder's step
/// is two bits, so an odd K is nobody's.
pub fn drn_for_bits(bits: u32) -> Option<u8> {
    (1..=super::UP_RATES as u8).find(|&drn| up_bits(drn) == bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::{CONSTELLATION_FRAME, Filters, Trellis};

    /// Parameters with one constellation set of `levels` positive points,
    /// every interval on modulus `m`, and a prefilter that passes x straight
    /// through, so the levels the chain emits are the class's own.
    fn parameters(drn: u8, levels: usize, gain: f64) -> Parameters {
        // Each of the twelve intervals carries K/12 bits, which is what a
        // modulus of 2^(K/12) holds exactly.
        #[allow(clippy::cast_possible_truncation)]
        let m = 1u8 << (up_bits(drn) / UP_INTERVALS as u32);
        Parameters {
            drn,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            gain,
            moduli: [m; UP_INTERVALS],
            filters: Filters { z2: vec![1.0], ..Filters::default() },
            sets: vec![(0..levels).map(|index| (2 * index + 1) as u16).collect()],
            indices: [0; CONSTELLATION_FRAME],
        }
    }

    /// A deterministic bit source, so the tests do not pull in a crate for it.
    struct Random(u64);

    impl Random {
        fn new(seed: u64) -> Self {
            Self(seed | 1)
        }

        fn next(&mut self) -> bool {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 63 == 0
        }
    }

    /// 6.1 and 6.4.1: twelve symbols carry K = 2 x (drn + 17) bits, and the
    /// ladder runs from 24 000 to 48 000 bit/s in steps of 8000/6.
    #[test]
    fn a_frame_carries_the_ladder_s_bits_and_no_others() {
        for drn in [1u8, 7, 13, 19] {
            let bits = up_bits(drn);
            let encoder = UpEncoder::new(&parameters(drn, 128, 1.0)).expect("feasible");
            assert_eq!(encoder.frame_bits(), bits, "drn {drn}");
            assert_eq!(encoder.rate(), crate::v92::up_rate(drn), "drn {drn}");
            assert_eq!(encoder.drn(), drn, "the ladder goes back as it came");
            assert_eq!(drn_for_bits(bits), Some(drn));
        }
    }

    /// 8.7.1: B1u is the same 48 frames every time, because every memory it
    /// starts from is zero and its data is ones. A fixed vector, so that a
    /// change to the scrambler, the modulus encoder or the chain shows up here
    /// rather than as a slightly worse error rate three packages on.
    #[test]
    fn b1u_is_the_same_five_hundred_and_seventy_six_symbols_every_time() {
        let parameters = parameters(13, 128, 1.0);
        let mut first = UpEncoder::new(&parameters).expect("feasible");
        let once = first.b1u();
        let mut second = UpEncoder::new(&parameters).expect("feasible");
        let twice = second.b1u();
        assert_eq!(once.len(), 48 * UP_INTERVALS, "B1u is 48 data frames");
        assert_eq!(once, twice, "two B1u runs differ");

        // The interval is n mod 12 from the first symbol (8.7.1), and every
        // symbol came out of the chain in order.
        for (n, symbol) in once.iter().enumerate() {
            assert_eq!(symbol.n, n as u64, "symbol {n} is not the nth");
            assert_eq!(crate::v92::interval(symbol.n), n % UP_INTERVALS);
        }
        // 6.4.3 and P-4: the parity of the sum of the four indices in each
        // trellis frame is Y0, which the chain reads before choosing them.
        // This is the property the decoder's Viterbi relies on.
        for frame in once.chunks(crate::v92::TRELLIS_FRAME) {
            let sum: i32 = frame.iter().map(|symbol| symbol.eta).sum();
            assert_eq!(sum.rem_euclid(2) == 1, frame[0].y0, "trellis frame at n {}", frame[0].n);
        }
    }

    /// 8.7.2: E2u is one data frame, and CPd's bit 29 makes it thirteen
    /// symbols. The extra symbol moves everything after it, which is why the
    /// decoder is told about it.
    #[test]
    fn e2u_grows_by_one_symbol_when_the_digital_modem_asks() {
        let parameters = parameters(13, 128, 1.0);
        let mut plain = UpEncoder::new(&parameters).expect("feasible");
        let mut extended = UpEncoder::new(&parameters).expect("feasible");
        assert_eq!(plain.e2u(false).len(), UP_INTERVALS);
        assert_eq!(extended.e2u(true).len(), UP_INTERVALS + 1);
        // And the data frame after it starts one symbol further round.
        let after_plain = plain.next_symbol(|| false);
        let after_extended = extended.next_symbol(|| false);
        assert_eq!(crate::v92::interval(after_plain.n), 0, "twelve symbols is a whole frame");
        assert_eq!(crate::v92::interval(after_extended.n), (UP_INTERVALS + 1) % UP_INTERVALS);
    }

    /// 8.7.4: RM and RM' consume no data bits, do not clock the differential
    /// encoder, and carry the precoder, prefilter and trellis over from data
    /// mode. A bit source that would be visible if it were called, and the
    /// chain's own symbol count, are the whole check.
    #[test]
    fn rm_takes_no_bits_and_leaves_the_sign_chain_where_it_was() {
        let mut encoder = UpEncoder::new(&parameters(13, 128, 1.0)).expect("feasible");
        let mut random = Random::new(11);
        for _ in 0..4 {
            encoder.next_symbol(|| random.next());
        }
        let before = encoder.d();
        let pattern = [3, 1, 2, 0, 3, 2, 1, 0, 3, 1, 2, 0];
        let symbols = encoder.force_k(pattern);
        assert_eq!(symbols.len(), UP_INTERVALS);
        assert_eq!(encoder.d(), before, "the differential encoder was clocked by RM");
        // And the chain did not reset: n carried on, so the interval did too.
        assert_eq!(symbols[0].n, 4);
    }

    /// 8.7.7: FB1u goes out on the previous data mode's parameters, and there
    /// is nothing to send it on before a rate change.
    #[test]
    fn fb1u_needs_a_previous_data_mode_and_uses_it() {
        let mut encoder = UpEncoder::new(&parameters(13, 128, 1.0)).expect("feasible");
        assert!(encoder.fb1u().is_err(), "there was no previous mode to refuse");
        assert!(!encoder.has_old());
        encoder.retune(&parameters(7, 128, 1.0)).expect("feasible");
        assert!(encoder.has_old());
        let symbols = encoder.fb1u().expect("a rate change left an old chain");
        assert_eq!(symbols.len(), 48 * UP_INTERVALS);
        assert_eq!(encoder.rate(), crate::v92::up_rate(7), "the new mode is the current one");
    }

    /// 9.9.2.1.2: a fast parameter exchange resets the scrambler and the
    /// differential encoder, and nothing else.
    #[test]
    fn a_reset_starts_the_scrambler_over_without_touching_the_chain() {
        let mut encoder = UpEncoder::new(&parameters(13, 128, 1.0)).expect("feasible");
        let mut random = Random::new(3);
        for _ in 0..24 {
            encoder.next_symbol(|| random.next());
        }
        encoder.reset_scrambler_and_differential();
        assert!(!encoder.d(), "the differential encoder kept its memory");
        let mut again = Random::new(3);
        let symbol = encoder.next_symbol(|| again.next());
        assert!(symbol.n >= 24, "the chain was reset with the scrambler");
    }

    /// 8.8.3: G is the one free scale in the chain -- it is chosen so that the
    /// mean square of G x v(n) is one -- so nothing else may depend on it. Two
    /// encoders that differ only in G produce the same v(n) and outputs in the
    /// ratio of their gains.
    ///
    /// (Where that mean square lands is the design's business, V92-62: with no
    /// precoder tail to steer by, a class's member nearest zero is chosen, and
    /// the levels are then small rather than uniform.)
    #[test]
    fn the_gain_scales_the_output_and_nothing_else() {
        let mut quiet = UpEncoder::new(&parameters(13, 128, 1.0)).expect("feasible");
        let mut loud = UpEncoder::new(&parameters(13, 128, 2.0)).expect("feasible");
        let (mut one, mut other) = (Random::new(5), Random::new(5));
        for _ in 0..(40 * UP_INTERVALS) {
            let (a, b) = (quiet.next_symbol(|| one.next()), loud.next_symbol(|| other.next()));
            assert_eq!(a.eta, b.eta, "the gain changed the point chosen");
            assert_eq!(a.v, b.v, "the gain changed the prefilter's output");
            // The one thing that is not linear is 8.8.3's bound on G x v(n),
            // which is there to keep a badly designed filter off the line.
            if a.v.abs() * 2.0 < crate::v92::precoder::OUTPUT_LIMIT {
                assert!((b.out - 2.0 * a.out).abs() < 1e-12, "out is not G x v: {} against {}", b.out, a.out);
            }
        }
    }
}
