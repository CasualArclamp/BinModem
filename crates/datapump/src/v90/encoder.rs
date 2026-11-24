//! The digital modem's encoder (V.90 5.4), end to end.
//!
//! Figure 1 in one place: D serial bits in, six signed PCM codewords out. The
//! parts are 5.4.2's bit parser, 5.4.3's modulus encoder, 5.4.4's six mappers,
//! 5.4.5's sign coding and 5.4.7's mux, and the only thing this adds is the
//! order they go in.
//!
//! What comes out is not a waveform. 5.4.7 has the codewords "transmitted from
//! the digital modem sequentially with PCM0 being first in time", and what
//! transmits them is the digital network interface -- a PRI or a BRI, which
//! takes octets and not samples. The analogue end is the only one of the pair
//! that ever deals in amplitude.

use std::collections::VecDeque;

use super::INTERVALS;
use super::modulus::{self, Constellation, Moduli};
use super::sequences::Cp;
use super::sign::{Differential, Redundancy, ShapingFrame, Shaper, SignDecoder, Signs};
use super::ucode::{self, Law};

/// One data frame's worth of output: six codewords with their signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// The Ucodes, PCM0 first in time (5.4.7).
    pub ucodes: [u8; INTERVALS],
    /// 5.4.6: true is a positive voltage.
    pub positive: Signs,
}

impl Frame {
    /// The octets to hand to the network interface.
    pub fn octets(&self, law: Law) -> [u8; INTERVALS] {
        std::array::from_fn(|i| ucode::octet(law, self.ucodes[i], !self.positive[i]))
    }

    /// The amplitudes a far-end codec will produce from them.
    pub fn amplitudes(&self, law: Law) -> [i32; INTERVALS] {
        std::array::from_fn(|i| ucode::amplitude(law, self.ucodes[i], !self.positive[i]))
    }
}

/// What training settled on: the six constellations and how the signs are
/// spent (5.4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    /// C0 to C5, "specified by the analogue modem during training procedures".
    pub sets: [Constellation; INTERVALS],
    /// K, the modulus encoder's input bits per data frame.
    pub k: u32,
    /// Sr, and with it S.
    pub redundancy: Redundancy,
    /// ld, the shaper's look-ahead in shaping frames.
    pub lookahead: usize,
    /// a1, a2, b1 and b2 of the shaping filter, in CP's Q1.6.
    pub shaping: [i8; 4],
}

impl Mapping {
    /// The best mapping a route allows: as many bits as both the moduli and
    /// Table 2 will take.
    ///
    /// Two ceilings, and either can be the binding one. The moduli say how
    /// many distinct messages the six intervals can express between them; Table
    /// 2 says the rate ladder stops at 56 000 whatever the line could manage,
    /// because downstream a data frame cannot carry more than 42 bits when the
    /// network carries 8000 codewords a second and each is eight bits.
    pub fn best(sets: [Constellation; INTERVALS], redundancy: Redundancy) -> Self {
        let moduli: Moduli = std::array::from_fn(|i| sets[i].modulus());
        let s = redundancy.data_bits() as u32;
        let k = modulus::capacity(moduli).min(super::largest_k(s));
        Self { sets, k, redundancy, lookahead: 0, shaping: [0; 4] }
    }

    /// What a CP or a CPt asks for (8.5.2): its constellations, its rate, and
    /// its shaping. None if the rate is more than the constellations can
    /// carry, which 5.4.3's inequality forbids.
    pub fn from_cp(cp: &Cp) -> Option<Self> {
        let sets: [Constellation; INTERVALS] = std::array::from_fn(|i| Constellation::new(cp.points(i)));
        let s = cp.redundancy.data_bits();
        let k = cp.frame_bits().checked_sub(s)? as u32;
        let mapping = Self { sets, k, redundancy: cp.redundancy, lookahead: usize::from(cp.lookahead), shaping: cp.shaping };
        modulus::fits(mapping.moduli(), k).then_some(mapping)
    }

    /// What TRN2d, MP and Ed go out on in a rate renegotiation (8.6): "the
    /// spectral shaping parameters as used in the preceding data mode along
    /// with the K previously derived from CPt", and CPt's constellations.
    pub fn for_renegotiation(cpt: &Cp, data_mode: &Cp) -> Option<Self> {
        let training = Self::from_cp(cpt)?;
        Some(Self {
            redundancy: data_mode.redundancy,
            lookahead: usize::from(data_mode.lookahead),
            shaping: data_mode.shaping,
            ..training
        })
    }

    /// The moduli these constellations give (5.4.3: "Mi is equal to the number
    /// of members in the PCM code sets").
    pub fn moduli(&self) -> Moduli {
        std::array::from_fn(|i| self.sets[i].modulus())
    }

    /// D, "equal to S + K" (5.4.2).
    pub fn frame_bits(&self) -> usize {
        self.k as usize + self.redundancy.data_bits()
    }

    /// The signalling rate this mapping carries.
    pub fn rate(&self) -> u32 {
        super::rate_for(self.frame_bits() as u32)
    }

    /// Whether this mapping is one a V.90 connection could actually use.
    ///
    /// Both conditions, because they are independent: 5.4.3's inequality says
    /// the moduli can express K bits, and Table 2 says K and S are a
    /// combination the Recommendation defines. A route good enough for more
    /// than 56 000 is not rare -- six intervals of 88 codes would carry 58 666
    /// -- and the ladder simply stops.
    pub fn valid(&self) -> bool {
        modulus::fits(self.moduli(), self.k)
            && super::table_2_has(self.k, self.redundancy.data_bits() as u32)
    }
}

/// The digital modem's transmitting half.
///
/// Built afresh wherever the Recommendation starts the coding again -- TRN2d
/// and B1d both begin with "the scrambler, differential encoder and spectral
/// shape filter memory ... initialized to zero" -- so there is no reset.
///
/// With look-ahead, what goes in and what comes out are not in step. The
/// shaper chooses a frame's signs from "the PCM symbol magnitudes produced by
/// the mapper for spectral shaping frames j, j+1, ..., j+ld" (5.4.5.5), so a
/// frame is mapped, and its bits taken, up to ld shaping frames before it can
/// go: [`Self::push`] maps, and [`Self::pop`] gives up what is ready.
#[derive(Debug, Clone)]
pub struct Encoder {
    mapping: Mapping,
    law: Law,
    signs: Differential,
    shaper: Shaper,
    /// Mapped frames whose signs are not all chosen yet: their magnitudes, and
    /// their shaping frames still to go.
    magnitudes: VecDeque<[u8; INTERVALS]>,
    shaping: VecDeque<ShapingFrame>,
    /// Unshaped frames, whose signs are chosen as they are mapped.
    ready: VecDeque<Frame>,
}

impl Encoder {
    pub fn new(mapping: Mapping, law: Law) -> Self {
        let shaper = Shaper::new(mapping.redundancy, mapping.lookahead, mapping.shaping);
        Self {
            mapping,
            law,
            signs: Differential::new(),
            shaper,
            magnitudes: VecDeque::new(),
            shaping: VecDeque::new(),
            ready: VecDeque::new(),
        }
    }

    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// D, the bits a data frame takes.
    pub fn frame_bits(&self) -> usize {
        self.mapping.frame_bits()
    }

    /// Map one data frame's D bits, as far as the magnitudes and the initial
    /// signs.
    ///
    /// 5.4.2 does the parsing and the order matters: "d0 to d(S-1) form s0 to
    /// s(S-1) and dS to d(D-1) form b0 to b(K-1)". The sign bits come first in
    /// time and the modulus encoder's bits follow, which is the opposite of
    /// the order Figure 1 draws them in.
    fn map(&mut self, bits: &[bool]) -> ([u8; INTERVALS], Vec<bool>) {
        let s_count = self.mapping.redundancy.data_bits();
        let s: Vec<bool> = (0..s_count).map(|i| bits.get(i).copied().unwrap_or(false)).collect();
        let b: Vec<bool> = bits.iter().skip(s_count).copied().collect();
        let labels = modulus::encode(&b, self.mapping.moduli());
        let ucodes = std::array::from_fn(|i| {
            // 5.4.4: the mapper "forms Ui by choosing the constellation point
            // in Ci labelled by Ki". A label outside the set cannot happen
            // while 5.4.3's inequality holds, and the quietest point is the
            // safe thing to send if it ever did.
            self.mapping.sets[i]
                .point(labels[i])
                .or_else(|| self.mapping.sets[i].points().last().copied())
                .unwrap_or(0)
        });
        (ucodes, s)
    }

    /// Map one data frame's D bits. Unshaped, it is ready at once; shaped, once
    /// the shaper has seen ld shaping frames past it.
    pub fn push(&mut self, bits: &[bool]) {
        let (ucodes, s) = self.map(bits);
        if self.mapping.redundancy == Redundancy::None {
            let s: Signs = std::array::from_fn(|i| s[i]);
            let positive = self.signs.encode(s);
            self.ready.push_back(Frame { ucodes, positive });
            return;
        }
        let levels = std::array::from_fn(|i| ucode::level(self.law, ucodes[i]));
        let frames = self.shaper.prepare(&s, levels);
        self.magnitudes.push_back(ucodes);
        self.shaping.extend(frames);
    }

    /// The next frame out, if the shaper has seen far enough past it; or,
    /// `finishing` -- nothing more to be mapped before the coding starts
    /// again -- with however far there is to see.
    pub fn pop(&mut self, finishing: bool) -> Option<Frame> {
        if let Some(frame) = self.ready.pop_front() {
            return Some(frame);
        }
        let per = self.shaper.frames_per_data_frame();
        let wanted = if finishing { per } else { per + self.shaper.lookahead() };
        if per == 0 || self.shaping.len() < wanted {
            return None;
        }
        let mut positive = Vec::with_capacity(INTERVALS);
        for _ in 0..per {
            let frames = self.shaping.make_contiguous();
            positive.extend(self.shaper.choose(frames));
            self.shaping.pop_front();
        }
        let ucodes = self.magnitudes.pop_front()?;
        Some(Frame { ucodes, positive: std::array::from_fn(|i| positive[i]) })
    }

    /// The next data frame, pulling D bits from `bit` for each frame that has
    /// to be mapped to get it out: the frame itself, and with look-ahead the
    /// frames after it, whose magnitudes the shaper needs to see.
    pub fn next_frame(&mut self, mut bit: impl FnMut() -> bool) -> Frame {
        loop {
            if let Some(frame) = self.pop(false) {
                return frame;
            }
            let bits: Vec<bool> = (0..self.frame_bits()).map(|_| bit()).collect();
            self.push(&bits);
        }
    }

    /// One data frame of `bits`, for an encoder that needs nothing ahead of
    /// the frame it is sending: no shaping, or shaping with no look-ahead.
    pub fn frame(&mut self, bits: &[bool]) -> Frame {
        debug_assert!(
            self.mapping.redundancy == Redundancy::None || self.mapping.lookahead == 0,
            "an encoder with look-ahead takes its bits through next_frame"
        );
        let mut given = bits.iter().copied();
        self.next_frame(|| given.next().unwrap_or(false))
    }
}

/// The analogue modem's receiving half of the same arithmetic.
#[derive(Debug, Clone)]
pub struct Decoder {
    mapping: Mapping,
    signs: SignDecoder,
}

impl Decoder {
    pub fn new(mapping: Mapping) -> Self {
        let signs = SignDecoder::new(mapping.redundancy);
        Self { mapping, signs }
    }

    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// Whether a frame's six codewords make a number the modulus encoder
    /// could have made: less than 2^K, where the constellations can say more.
    ///
    /// A frame read with its intervals in the wrong places often makes one it
    /// could not, and one read in the right places never does.
    pub fn could_have_sent(&self, frame: &Frame) -> bool {
        let moduli = self.mapping.moduli();
        let mut r: u128 = 0;
        for i in (0..INTERVALS).rev() {
            let label = self.mapping.sets[i].label(frame.ucodes[i]).unwrap_or(0);
            r = r * u128::from(moduli[i].max(1)) + u128::from(label);
        }
        self.mapping.k >= 128 || r < 1u128 << self.mapping.k
    }

    /// Six codewords back to the D bits they carried.
    pub fn frame(&mut self, frame: Frame) -> Vec<bool> {
        let labels: [u16; INTERVALS] = std::array::from_fn(|i| self.mapping.sets[i].label(frame.ucodes[i]).unwrap_or(0));
        let b = modulus::decode(labels, self.mapping.moduli(), self.mapping.k);
        let mut out = self.signs.decode(frame.positive);
        out.extend(b);
        out
    }

    /// The same, from what the codec at this end produced.
    ///
    /// This is the only place amplitude comes into it: the analogue modem sees
    /// samples and has to decide which codeword each was.
    pub fn from_amplitudes(&mut self, law: Law, samples: [i32; INTERVALS]) -> Vec<bool> {
        let mut ucodes = [0u8; INTERVALS];
        let mut positive = [false; INTERVALS];
        for i in 0..INTERVALS {
            let (u, negative) = ucode::nearest(law, samples[i]);
            ucodes[i] = u;
            positive[i] = !negative;
        }
        self.frame(Frame { ucodes, positive })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mapping like a real route gives: most intervals full, one of them
    /// halved by a robbed bit, and the quietest codes left out because no
    /// receiver could separate them.
    fn route() -> Mapping {
        let usable: Vec<u8> = (24..112).collect();
        let robbed: Vec<u8> = (24..112).step_by(2).collect();
        let sets: [Constellation; INTERVALS] = [
            Constellation::new(usable.clone()),
            Constellation::new(usable.clone()),
            Constellation::new(usable.clone()),
            Constellation::new(robbed),
            Constellation::new(usable.clone()),
            Constellation::new(usable),
        ];
        Mapping::best(sets, Redundancy::None)
    }

    /// The whole of Figure 1, there and back.
    #[test]
    fn a_data_frame_goes_out_as_codewords_and_comes_back_as_bits() {
        let mapping = route();
        assert!(mapping.valid(), "5.4.3's inequality does not hold");
        let d = mapping.frame_bits();
        let mut tx = Encoder::new(mapping.clone(), Law::Mu);
        let mut rx = Decoder::new(mapping);

        let mut x: u64 = 0x1234_5678_9abc_def0;
        for _ in 0..500 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let bits: Vec<bool> = (0..d).map(|i| x >> i & 1 == 1).collect();
            let frame = tx.frame(&bits);
            assert_eq!(rx.frame(frame), bits);
        }
    }

    /// Shaping on, with every look-ahead there is: the frames come out in
    /// order, and every bit comes back.
    #[test]
    fn a_shaped_stream_comes_back_whatever_the_look_ahead() {
        for sr in [Redundancy::One, Redundancy::Two, Redundancy::Three] {
            for lookahead in 0..=3 {
                let mut mapping = route();
                mapping.redundancy = sr;
                mapping.lookahead = lookahead;
                mapping.shaping = [-40, 20, 60, -64];
                mapping.k = mapping.k.min(super::super::largest_k(sr.data_bits() as u32));
                assert!(mapping.valid());
                let d = mapping.frame_bits();
                let mut tx = Encoder::new(mapping.clone(), Law::Mu);
                let mut rx = Decoder::new(mapping);
                let mut x: u64 = 0xdead_beef ^ lookahead as u64;
                let mut sent: std::collections::VecDeque<bool> = std::collections::VecDeque::new();
                let mut next = || {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x & 1 == 1
                };
                for n in 0..300 {
                    let frame = tx.next_frame(|| {
                        let b = next();
                        sent.push_back(b);
                        b
                    });
                    let wanted: Vec<bool> = sent.drain(..d).collect();
                    assert_eq!(rx.frame(frame), wanted, "Sr {sr:?}, ld {lookahead}, frame {n}");
                }
            }
        }
    }

    /// A CP's constellations and rate are a mapping, and one asking for more
    /// than its constellations hold is not.
    #[test]
    fn a_cp_is_a_mapping() {
        use super::super::sequences::Cp;
        let mask: super::super::sequences::Mask = (24..112).fold(0, |m, u| m | 1 << u);
        let cp = Cp { data_mode: true, drn: 22, constellations: vec![mask], ..Cp::default() };
        let mapping = Mapping::from_cp(&cp).expect("56 000 fits 88 codes a frame");
        assert_eq!(mapping.frame_bits(), 42);
        assert_eq!(mapping.k, 36);
        assert_eq!(mapping.rate(), 56_000);
        let small: super::super::sequences::Mask = (100..108).fold(0, |m, u| m | 1 << u);
        let too_much = Cp { constellations: vec![small], ..cp };
        assert_eq!(Mapping::from_cp(&too_much), None, "eight codes cannot carry 36 bits");
    }

    /// And through the amplitudes, which is what the analogue end actually
    /// receives -- on a clean line, where every sample lands on its codepoint.
    #[test]
    fn the_same_frame_survives_being_read_off_the_line() {
        let mapping = route();
        let d = mapping.frame_bits();
        let mut tx = Encoder::new(mapping.clone(), Law::Mu);
        let mut rx = Decoder::new(mapping);

        for value in 0..200u64 {
            let bits: Vec<bool> = (0..d).map(|i| value.wrapping_mul(2654435761) >> i & 1 == 1).collect();
            let frame = tx.frame(&bits);
            let samples = frame.amplitudes(Law::Mu);
            assert_eq!(rx.from_amplitudes(Law::Mu, samples), bits, "value {value}");
        }
    }

    /// 5.4.2: the sign bits are first in time and the modulus encoder's bits
    /// follow, which is the opposite of the order Figure 1 draws them.
    #[test]
    fn the_sign_bits_come_off_the_front_of_the_frame() {
        let mut mapping = route();
        mapping.k = 12;
        let d = mapping.frame_bits();
        assert_eq!(d, 12 + 6, "S is six when shaping is off");
        let mut tx = Encoder::new(mapping.clone(), Law::Mu);

        // Everything zero but the very first bit, which is s0. With the
        // differential chain starting from nothing, s0 set makes every sign
        // positive and leaves the magnitudes at their lowest label.
        let mut bits = vec![false; d];
        bits[0] = true;
        let frame = tx.frame(&bits);
        assert_eq!(frame.positive, [true; INTERVALS]);
        // Label 0 is the largest code (5.4.4), and a modulus input of zero is
        // label 0 in every interval.
        for i in 0..INTERVALS {
            assert_eq!(frame.ucodes[i], mapping.sets[i].points()[0]);
        }
    }

    /// A robbed bit costs a rung only when the line was the thing limiting
    /// the rate, which is not the same as always.
    ///
    /// Halving one interval's alphabet always costs exactly one bit of
    /// capacity. Whether that shows up as a slower connection depends on which
    /// ceiling was binding: on a route good enough to reach 56 000 with room
    /// to spare, the ladder stops before the line does and the lost bit is
    /// spare capacity. On a route that was already at its limit, it is a rung.
    #[test]
    fn a_robbed_bit_costs_a_rung_only_when_the_line_was_the_limit() {
        let build = |usable: std::ops::Range<u8>, rob: bool| {
            let sets: [Constellation; INTERVALS] = std::array::from_fn(|i| {
                if rob && i == 3 {
                    Constellation::new(usable.clone().step_by(2).collect())
                } else {
                    Constellation::new(usable.clone().collect())
                }
            });
            Mapping::best(sets, Redundancy::None)
        };

        // Room to spare: both reach the top of the ladder.
        let plenty = build(24..112, false);
        let plenty_robbed = build(24..112, true);
        assert_eq!(plenty.rate(), super::super::FASTEST);
        assert_eq!(
            plenty_robbed.rate(),
            super::super::FASTEST,
            "a robbed bit cost a rung that was spare capacity"
        );

        // A poorer route, where capacity is what settles the rate.
        let tight = build(60..92, false);
        let tight_robbed = build(60..92, true);
        assert!(tight.rate() < super::super::FASTEST, "this route is not tight");
        assert_eq!(tight.k - tight_robbed.k, 1, "halving an interval cost more than a bit");
        let step = tight.rate() - tight_robbed.rate();
        assert!(step == 1333 || step == 1334, "it cost {step} bit/s");
    }

    /// Halving one interval always costs exactly one bit of capacity, whatever
    /// the rate ends up being.
    #[test]
    fn a_robbed_interval_costs_one_bit_of_capacity() {
        let full: Vec<u8> = (24..112).collect();
        let sets: [Constellation; INTERVALS] =
            std::array::from_fn(|_| Constellation::new(full.clone()));
        // Deliberately not capped at Table 2's ceiling here: what is being
        // measured is what the moduli can carry, and a route this good runs
        // into the top of the ladder rather than into the line.
        let clean_k = modulus::capacity(std::array::from_fn(|i| sets[i].modulus()));
        let robbed_sets: [Constellation; INTERVALS] = std::array::from_fn(|i| {
            if i == 3 {
                Constellation::new((24..112).step_by(2).collect())
            } else {
                Constellation::new(full.clone())
            }
        });
        let robbed_k = modulus::capacity(std::array::from_fn(|i| robbed_sets[i].modulus()));
        assert_eq!(clean_k - robbed_k, 1, "halving one interval cost {} bits", clean_k - robbed_k);
        let step = super::super::rate_for(clean_k + 6) - super::super::rate_for(robbed_k + 6);
        assert!(step == 1333 || step == 1334, "it cost {step} bit/s");
        let _ = robbed_sets;
    }

    /// The codewords that go to the network interface are G.711 octets, and
    /// 5.4.6's sign convention is the one that reaches them.
    #[test]
    fn the_output_is_g711_octets_with_a_set_bit_for_positive() {
        let frame = Frame {
            ucodes: [0, 64, 127, 1, 2, 3],
            positive: [true, false, true, false, true, false],
        };
        let octets = frame.octets(Law::Mu);
        // Ucode 0 positive is 0xff and Ucode 127 positive is 0x80 (Table 1).
        assert_eq!(octets[0], 0xff);
        assert_eq!(octets[2], 0x80);
        // A negative code sits in the other half of the octet range.
        assert_eq!(octets[1], 0x7f - 64);
        // And the amplitudes carry the sign the other way round from the bit.
        let a = frame.amplitudes(Law::Mu);
        assert!(a[0] >= 0 && a[1] < 0 && a[2] > 0);
    }
}

#[cfg(test)]
mod rates {
    use super::*;

    /// What a few plausible routes come out at, as a sanity check on the
    /// arithmetic rather than a claim about any real line.
    #[test]
    fn plausible_routes_land_on_the_ladder() {
        let cases: [(&str, Vec<u8>, bool); 3] = [
            ("every code above the noise", (24..112).collect(), false),
            ("a quieter line", (40..104).collect(), false),
            ("every code above the noise, one interval robbed", (24..112).collect(), true),
        ];
        for (what, usable, robbed) in cases {
            let sets: [Constellation; INTERVALS] = std::array::from_fn(|i| {
                if robbed && i == 3 {
                    Constellation::new(usable.iter().copied().step_by(2).collect())
                } else {
                    Constellation::new(usable.clone())
                }
            });
            let moduli: Moduli = std::array::from_fn(|i| sets[i].modulus());
            let mapping = Mapping::best(sets, Redundancy::None);
            assert!(mapping.valid(), "{what} produced a mapping V.90 does not define");
            let rate = mapping.rate();
            println!("  {what}: M {:?} -> K {} -> {rate} bit/s", moduli, mapping.k);
            assert!(
                (super::super::SLOWEST..=super::super::FASTEST).contains(&rate),
                "{what} came out at {rate}, off the ladder"
            );
        }
    }
}
