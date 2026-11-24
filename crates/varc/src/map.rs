//! Bits to constellation points and back.
//!
//! Every V.ARC modulation puts its bits on the same thing, a Gray-coded PAM
//! ladder: once for the baseband mode, twice at right angles for QAM, and
//! twice per carrier for the multicarrier mode. One ladder for all of them
//! means one slicer to get right.
//!
//! A carrier asked for an odd number of bits gets a rectangle, one more bit on
//! the in-phase axis than the quadrature one, rather than the cross a V.34
//! would use. A cross is about a decibel better and a great deal fiddlier,
//! and a decibel is a third of a bit on one carrier out of hundreds.

use dsp::Complex;

/// The most bits one QAM point may carry: two 4096-level ladders.
pub const MAX_BITS: u8 = 24;

/// The level, an odd integer between `-(M-1)` and `M-1`, for `value` on a
/// ladder of `bits` bits. Neighbouring levels differ in one bit.
pub fn pam_level(value: u32, bits: u32) -> i32 {
    if bits == 0 {
        return 0;
    }
    // Gray decode: the level index whose Gray code is `value`.
    let mut index = value;
    let mut shift = 1;
    while shift < bits {
        index ^= index >> shift;
        shift <<= 1;
    }
    let m = 1i32 << bits;
    2 * index as i32 - (m - 1)
}

/// The value on the nearest level to `x`, which is in the same units as the
/// levels, and that level.
pub fn pam_slice(x: f64, bits: u32) -> (u32, i32) {
    if bits == 0 {
        return (0, 0);
    }
    let m = 1i64 << bits;
    let index = (((x + (m - 1) as f64) / 2.0).round() as i64).clamp(0, m - 1) as u32;
    (index ^ (index >> 1), 2 * index as i32 - (m as i32 - 1))
}

/// How a QAM point of `bits` bits splits between the two axes.
pub fn split(bits: u8) -> (u32, u32) {
    (u32::from(bits).div_ceil(2), u32::from(bits) / 2)
}

/// What to multiply integer levels by so that points of `bits` bits average
/// unit power.
pub fn qam_scale(bits: u8) -> f64 {
    let (bi, bq) = split(bits);
    let mi = (1u64 << bi) as f64;
    let mq = (1u64 << bq) as f64;
    let power = ((mi * mi - 1.0) + (mq * mq - 1.0)) / 3.0;
    if power > 0.0 { 1.0 / power.sqrt() } else { 1.0 }
}

/// The unit-power point for the low `bits` bits of `value`.
pub fn qam_point(value: u32, bits: u8) -> Complex {
    let (bi, bq) = split(bits);
    let scale = qam_scale(bits);
    let q = pam_level(value & ((1u32 << bq) - 1), bq);
    let i = pam_level(value >> bq, bi);
    Complex::new(f64::from(i) * scale, f64::from(q) * scale)
}

/// The value nearest to `z`, and the unit-power point it stands for.
pub fn qam_slice(z: Complex, bits: u8) -> (u32, Complex) {
    let (bi, bq) = split(bits);
    let scale = qam_scale(bits);
    let (vi, li) = pam_slice(z.re / scale, bi);
    let (vq, lq) = pam_slice(z.im / scale, bq);
    ((vi << bq) | vq, Complex::new(f64::from(li) * scale, f64::from(lq) * scale))
}

/// Bits waiting to go onto symbols, least significant first out of each octet.
#[derive(Debug, Default, Clone)]
pub struct BitReader {
    octets: Vec<u8>,
    at: usize,
}

impl BitReader {
    pub fn new(octets: Vec<u8>) -> Self {
        Self { octets, at: 0 }
    }

    /// The next `count` bits, zeros once the octets have run out.
    pub fn take(&mut self, count: u32) -> u32 {
        let mut value = 0u32;
        for i in 0..count {
            let byte = self.at / 8;
            if byte < self.octets.len() && self.octets[byte] >> (self.at % 8) & 1 != 0 {
                value |= 1 << i;
            }
            self.at += 1;
        }
        value
    }
}

/// Bits coming off symbols, gathered back into octets the same way round.
#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    octets: Vec<u8>,
    at: usize,
}

impl BitWriter {
    pub fn put(&mut self, value: u32, count: u32) {
        for i in 0..count {
            if self.at.is_multiple_of(8) {
                self.octets.push(0);
            }
            if value >> i & 1 != 0 {
                *self.octets.last_mut().expect("just pushed") |= 1 << (self.at % 8);
            }
            self.at += 1;
        }
    }

    /// The whole octets gathered so far, truncated or zero-filled to `len`,
    /// and start again.
    pub fn finish(&mut self, len: usize) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.octets);
        out.resize(len, 0);
        self.at = 0;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_on_a_ladder_has_its_own_level_and_slices_back() {
        for bits in 1..=12u32 {
            let mut seen = std::collections::HashSet::new();
            for value in 0..1u32 << bits {
                let level = pam_level(value, bits);
                assert!(seen.insert(level));
                assert_eq!(level.rem_euclid(2), 1);
                assert_eq!(pam_slice(f64::from(level) + 0.4, bits), (value, level));
            }
        }
    }

    #[test]
    fn neighbouring_levels_differ_in_one_bit() {
        for bits in 1..=10u32 {
            let m = 1i32 << bits;
            let mut by_level = vec![0u32; m as usize];
            for value in 0..1u32 << bits {
                by_level[((pam_level(value, bits) + m - 1) / 2) as usize] = value;
            }
            for pair in by_level.windows(2) {
                assert_eq!((pair[0] ^ pair[1]).count_ones(), 1);
            }
        }
    }

    #[test]
    fn points_average_unit_power_and_slice_back() {
        for bits in 1..=14u8 {
            let mut power = 0.0;
            for value in 0..1u32 << bits {
                let p = qam_point(value, bits);
                power += p.norm_sqr();
                assert_eq!(qam_slice(p, bits).0, value);
            }
            let mean = power / f64::from(1u32 << bits);
            assert!((mean - 1.0).abs() < 1e-9, "{bits} bits: {mean}");
        }
    }

    #[test]
    fn the_biggest_point_survives_a_round_trip() {
        let value = 0x00a5_c3f1 & ((1 << MAX_BITS) - 1);
        assert_eq!(qam_slice(qam_point(value, MAX_BITS), MAX_BITS).0, value);
    }

    #[test]
    fn bits_come_back_out_in_the_order_they_went_in() {
        let octets = vec![0xa5, 0x3c, 0xff, 0x01, 0x80];
        let mut reader = BitReader::new(octets.clone());
        let mut writer = BitWriter::default();
        for &count in &[3u32, 7, 1, 13, 9, 7] {
            writer.put(reader.take(count), count);
        }
        assert_eq!(writer.finish(5), octets);
    }
}
