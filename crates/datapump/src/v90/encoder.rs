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

use super::modulus::{self, Constellation, Moduli};
use super::sign::{self, Differential, Redundancy, Signs};
use super::ucode::{self, Law};
use super::INTERVALS;

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
#[derive(Debug, Clone)]
pub struct Mapping {
    /// C0 to C5, "specified by the analogue modem during training procedures".
    pub sets: [Constellation; INTERVALS],
    /// K, the modulus encoder's input bits per data frame.
    pub k: u32,
    /// Sr, and with it S.
    pub redundancy: Redundancy,
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
        Self { sets, k, redundancy }
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
#[derive(Debug)]
pub struct Encoder {
    mapping: Mapping,
    signs: Differential,
}

impl Encoder {
    pub fn new(mapping: Mapping) -> Self {
        Self { mapping, signs: Differential::new() }
    }

    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// One data frame: D bits in, six codewords out.
    ///
    /// 5.4.2 does the parsing and the order matters: "d0 to d(S-1) form s0 to
    /// s(S-1) and dS to d(D-1) form b0 to b(K-1)". The sign bits come first in
    /// time and the modulus encoder's bits follow, which is the opposite of
    /// the order Figure 1 draws them in.
    pub fn frame(&mut self, bits: &[bool]) -> Frame {
        let s_count = self.mapping.redundancy.data_bits();
        let mut s: Signs = [false; INTERVALS];
        for (i, slot) in s.iter_mut().enumerate().take(s_count) {
            *slot = bits.get(i).copied().unwrap_or(false);
        }
        let b: Vec<bool> = bits.iter().skip(s_count).copied().collect();

        let labels = modulus::encode(&b, self.mapping.moduli());
        let ucodes: [u8; INTERVALS] = std::array::from_fn(|i| {
            // 5.4.4: the mapper "forms Ui by choosing the constellation point
            // in Ci labelled by Ki". A label outside the set cannot happen
            // while 5.4.3's inequality holds, and the quietest point is the
            // safe thing to send if it ever did.
            self.mapping.sets[i]
                .point(labels[i])
                .or_else(|| self.mapping.sets[i].points().last().copied())
                .unwrap_or(0)
        });

        let positive = match self.mapping.redundancy {
            // 5.4.5.1 is the whole of shaping-disabled mode.
            Redundancy::None => self.signs.encode(s),
            // The shaper's own coding, up to but not including the rule
            // selection of 5.4.5.5 -- see the note on [`super::sign`].
            sr => {
                let parsed = sign::parse_to_frames(sr, &s[..s_count]);
                sign::to_signs(&parsed)
            }
        };
        Frame { ucodes, positive }
    }
}

/// The analogue modem's receiving half of the same arithmetic.
#[derive(Debug)]
pub struct Decoder {
    mapping: Mapping,
    signs: Differential,
}

impl Decoder {
    pub fn new(mapping: Mapping) -> Self {
        Self { mapping, signs: Differential::new() }
    }

    /// Six codewords back to the D bits they carried.
    pub fn frame(&mut self, frame: Frame) -> Vec<bool> {
        let labels: [u16; INTERVALS] = std::array::from_fn(|i| {
            self.mapping.sets[i].label(frame.ucodes[i]).unwrap_or(0)
        });
        let b = modulus::decode(labels, self.mapping.moduli(), self.mapping.k);
        let s_count = self.mapping.redundancy.data_bits();
        let s = match self.mapping.redundancy {
            Redundancy::None => self.signs.decode(frame.positive),
            _ => frame.positive,
        };
        let mut out: Vec<bool> = s[..s_count].to_vec();
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
        let mut tx = Encoder::new(mapping.clone());
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

    /// And through the amplitudes, which is what the analogue end actually
    /// receives -- on a clean line, where every sample lands on its codepoint.
    #[test]
    fn the_same_frame_survives_being_read_off_the_line() {
        let mapping = route();
        let d = mapping.frame_bits();
        let mut tx = Encoder::new(mapping.clone());
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
        let mut tx = Encoder::new(mapping.clone());

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
