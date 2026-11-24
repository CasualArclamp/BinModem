//! The modulus encoder and mapper (V.90 5.4.3, 5.4.4).
//!
//! The problem it solves is that the six data frame intervals of a data frame
//! do not hold a whole number of bits each. Interval i can carry one of M_i
//! amplitudes, and M_i is whatever the analogue modem found usable on this
//! particular route -- 47, or 89, or anything else. Six such intervals between
//! them carry the product of the moduli, which is not a power of two, so the
//! bits cannot be split up and handed out.
//!
//! 5.4.3 treats the whole data frame as one integer instead and writes it in a
//! mixed radix: divide by M_0 and the remainder is interval 0's label, divide
//! what is left by M_1 for interval 1, and so on. That is how a frame can
//! carry, say, 33 bits across six intervals whose sizes have nothing to do
//! with powers of two -- and it is why the rate ladder climbs in single bits
//! rather than in whole bits per symbol.
//!
//! The constraint 5.4.3 gives is the obvious one: the product of the moduli
//! has to be at least 2^K, or there are more messages than there are ways to
//! send them.

use super::INTERVALS;

/// The six mapping moduli of a data frame (5.4.1).
///
/// M_i "is equal to the number of positive levels in the constellation to be
/// used in data frame interval i as signalled by the analogue modem using the
/// CP sequences defined in 8.5.2" -- so these are the receiver's opinion of
/// the route, not anything the sender chose.
pub type Moduli = [u16; INTERVALS];

/// Whether K bits fit in these moduli (5.4.3).
///
/// "The values of M_i and K shall satisfy the inequality 2^K <= product of
/// M_i." Checked in 128 bits because six moduli of up to 128 multiply to more
/// than a 32-bit number can hold, and silently wrapping would turn a rate that
/// cannot be carried into one that appears to be.
pub fn fits(moduli: Moduli, k: u32) -> bool {
    if k >= 128 {
        return false;
    }
    let product: u128 = moduli.iter().map(|&m| u128::from(m)).product();
    product >= 1u128 << k
}

/// The largest K these moduli can carry.
pub fn capacity(moduli: Moduli) -> u32 {
    let product: u128 = moduli.iter().map(|&m| u128::from(m)).product();
    if product == 0 {
        return 0;
    }
    // The number of whole bits below the product.
    (0..=127).rev().find(|&k| product >= 1u128 << k).unwrap_or(0)
}

/// 5.4.3: K bits in, six labels out.
///
/// The bits arrive as `b0` first in time, and step 1 makes that the *least*
/// significant: "R0 = b0 + b1*2^1 + b2*2^2 + ... + b(K-1)*2^(K-1)". First in
/// time is lowest in value, which is the opposite of how a codeword is
/// written down, and getting it backwards produces a frame that decodes to
/// something plausible and wrong.
pub fn encode(bits: &[bool], moduli: Moduli) -> [u16; INTERVALS] {
    debug_assert!(bits.len() < 128, "a data frame does not hold that many bits");
    let mut r: u128 = 0;
    for (i, &bit) in bits.iter().enumerate() {
        if bit {
            r |= 1u128 << i;
        }
    }
    let mut labels = [0u16; INTERVALS];
    for (label, &m) in labels.iter_mut().zip(moduli.iter()) {
        let m = u128::from(m.max(1));
        // "Ki = Ri modulo Mi, where 0 <= Ki < Mi; R(i+1) = (Ri - Ki) / Mi".
        *label = (r % m) as u16;
        r /= m;
    }
    labels
}

/// And back: six labels to the K bits they carry.
///
/// The analogue modem's side of the same arithmetic. Reconstructing the
/// integer is the mixed radix read the other way, most significant interval
/// first.
pub fn decode(labels: [u16; INTERVALS], moduli: Moduli, k: u32) -> Vec<bool> {
    let mut r: u128 = 0;
    for (label, &m) in labels.iter().zip(moduli.iter()).rev() {
        r = r * u128::from(m.max(1)) + u128::from(*label);
    }
    (0..k).map(|i| r >> i & 1 == 1).collect()
}

/// One interval's constellation: the Ucodes of its positive levels (5.4.4).
///
/// 5.4.4 fixes the labelling and it is worth quoting, because it runs against
/// the numbering: "the members of C_i shall be labelled in descending order so
/// that label 0 corresponds to the largest PCM code in C_i, label M_i - 1
/// corresponds to the smallest PCM code in C_i". Largest PCM code means
/// largest Ucode, which is the loudest -- so label 0 is the loudest point and
/// the labels count downwards in amplitude.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Constellation {
    /// Ucodes, sorted so that index 0 is the largest.
    points: Vec<u8>,
}

impl Constellation {
    /// Build one from the Ucodes the analogue modem asked for, in any order.
    pub fn new(mut ucodes: Vec<u8>) -> Self {
        ucodes.sort_unstable();
        ucodes.dedup();
        ucodes.reverse();
        Self { points: ucodes }
    }

    /// M_i, "the number of members in the PCM code sets".
    pub fn modulus(&self) -> u16 {
        self.points.len() as u16
    }

    /// 5.4.4: "each mapper takes Ki and forms Ui by choosing the constellation
    /// point in Ci labelled by Ki".
    pub fn point(&self, label: u16) -> Option<u8> {
        self.points.get(usize::from(label)).copied()
    }

    /// The label of a point, which is what a receiver needs.
    pub fn label(&self, ucode: u8) -> Option<u16> {
        self.points.iter().position(|&u| u == ucode).map(|i| i as u16)
    }

    /// The points, loudest first.
    pub fn points(&self) -> &[u8] {
        &self.points
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::ucode::{Law, linear};

    /// The worked arithmetic of 5.4.3, done by hand on small moduli.
    #[test]
    fn the_labels_are_the_frame_written_in_a_mixed_radix() {
        let moduli = [3u16, 5, 7, 2, 2, 2];
        // R0 = 1 + 0*2 + 1*4 + 1*8 = 13.
        let bits = [true, false, true, true];
        let labels = encode(&bits, moduli);
        // 13 mod 3 = 1, remainder 4; 4 mod 5 = 4, remainder 0; the rest zero.
        assert_eq!(labels, [1, 4, 0, 0, 0, 0]);
        assert_eq!(decode(labels, moduli, 4), bits);
    }

    /// Step 1 makes the first bit in time the least significant, which is the
    /// opposite of how the number is written.
    #[test]
    fn the_first_bit_in_time_is_the_lowest_in_value() {
        let moduli = [128u16; INTERVALS];
        // b0 set and nothing else is one, not a large power of two.
        assert_eq!(encode(&[true, false, false, false], moduli)[0], 1);
        // b3 set alone is eight.
        assert_eq!(encode(&[false, false, false, true], moduli)[0], 8);
    }

    /// Every frame a set of moduli can carry survives the round trip.
    #[test]
    fn every_frame_comes_back_out_of_the_labels() {
        // Moduli that are not powers of two, which is the whole point.
        for moduli in [
            [47u16, 47, 47, 47, 47, 47],
            [89, 89, 89, 89, 89, 89],
            [128, 128, 128, 128, 128, 128],
            // One interval halved, which is what a robbed bit does.
            [89, 89, 89, 44, 89, 89],
            [3, 5, 7, 11, 13, 17],
        ] {
            let k = capacity(moduli).min(24);
            assert!(fits(moduli, k), "{moduli:?} cannot carry {k}");
            for value in 0..(1u32 << k.min(14)) {
                let bits: Vec<bool> = (0..k).map(|i| value >> i & 1 == 1).collect();
                let labels = encode(&bits, moduli);
                for (label, &m) in labels.iter().zip(moduli.iter()) {
                    assert!(*label < m, "label {label} outside modulus {m}");
                }
                assert_eq!(decode(labels, moduli, k), bits, "{moduli:?} value {value}");
            }
        }
    }

    /// 5.4.3's inequality, and what it costs to get it wrong.
    #[test]
    fn a_frame_larger_than_the_moduli_can_hold_does_not_fit() {
        // Six intervals of 128 hold 42 bits exactly, which is the top of the
        // ladder: 42 bits a frame is 56 000 bit/s.
        let full = [128u16; INTERVALS];
        assert_eq!(capacity(full), 42);
        assert!(fits(full, 42));
        assert!(!fits(full, 43));
        assert_eq!(super::super::rate_for(42), 56_000);

        // Halving one interval takes exactly one bit, and so exactly one rung.
        let robbed = [128u16, 128, 128, 64, 128, 128];
        assert_eq!(capacity(robbed), 41);
        // One step of the ladder, which is 8000/6 exactly. Reported rates are
        // floored, so the step reads as 1333 or 1334 depending on where on the
        // ladder it is taken -- the exact arithmetic is the assertion worth
        // making, and it is 8000 bit/s over six symbols.
        let step = super::super::rate_for(42) - super::super::rate_for(41);
        assert!(step == 1333 || step == 1334, "a robbed bit cost {step}");
        assert_eq!(42 * 8000 / 6 - 41 * 8000 / 6, step);
    }

    /// 5.4.4: label 0 is the largest PCM code, not the smallest.
    #[test]
    fn a_constellation_is_labelled_loudest_first() {
        let c = Constellation::new(vec![40, 8, 96, 64, 8]);
        assert_eq!(c.modulus(), 4, "the repeat was not dropped");
        assert_eq!(c.points(), &[96, 64, 40, 8]);
        assert_eq!(c.point(0), Some(96), "label 0 is not the largest code");
        assert_eq!(c.point(3), Some(8), "the last label is not the smallest");
        assert_eq!(c.point(4), None);
        assert_eq!(c.label(96), Some(0));
        assert_eq!(c.label(8), Some(3));
        assert_eq!(c.label(50), None);
        // And the largest Ucode really is the loudest amplitude, which is what
        // makes "largest PCM code" and "label 0" the same point.
        assert!(linear(Law::Mu, 96) > linear(Law::Mu, 8));
    }

    /// A whole data frame, end to end: bits in, Ucodes out, bits back.
    #[test]
    fn a_data_frame_of_ucodes_carries_its_bits_both_ways() {
        // Six constellations of different sizes, as a real route gives.
        let sets: [Constellation; INTERVALS] = [
            Constellation::new((20..100).collect()),
            Constellation::new((20..100).collect()),
            Constellation::new((20..100).collect()),
            // The robbed interval: half as many points.
            Constellation::new((20..100).step_by(2).collect()),
            Constellation::new((20..100).collect()),
            Constellation::new((20..100).collect()),
        ];
        let moduli: Moduli = std::array::from_fn(|i| sets[i].modulus());
        let k = capacity(moduli).min(20);

        for value in [0u32, 1, 12345, (1 << 20) - 1] {
            let bits: Vec<bool> = (0..k).map(|i| value >> i & 1 == 1).collect();
            let labels = encode(&bits, moduli);
            let sent: Vec<u8> = labels
                .iter()
                .zip(&sets)
                .map(|(&l, c)| c.point(l).expect("label outside the constellation"))
                .collect();
            // The receiver's side: Ucodes back to labels, labels back to bits.
            let heard: [u16; INTERVALS] = std::array::from_fn(|i| {
                sets[i].label(sent[i]).expect("a code that is not in the set")
            });
            assert_eq!(heard, labels);
            assert_eq!(decode(heard, moduli, k), bits, "value {value}");
        }
    }
}
