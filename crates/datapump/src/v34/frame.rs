//! How data mode's bits are laid out in time (clause 8, 9.2).
//!
//! A superframe is 280 ms, of J data frames. A data frame is P mapping frames,
//! and a mapping frame is four 4D symbols, each two 2D symbols. Every data
//! frame carries exactly N bits, and since P rarely divides N the mapping
//! frames carry b or b - 1 bits by a switching pattern that spreads the high
//! frames evenly. From b follow the rest: K bits for the shell mapper, 2q bits
//! a 4D symbol straight into the point's label, and M rings to shape over.
//!
//! None of it is read from Tables 7 to 10. Each table's rule is in the text
//! beside it -- the counter that makes SWP and AMP, the formulas for K, M and
//! L -- and the rules are what is here, checked against the tables as printed.

use super::info::SymbolRate;

/// What one direction's data mode runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Framing {
    pub rate: SymbolRate,
    /// Primary channel bit/s.
    pub primary: u32,
    /// Whether the 200 bit/s auxiliary channel rides along.
    pub auxiliary: bool,
    /// Expanded rather than minimum shaping (MP bit 32 of the receiving end).
    pub expanded: bool,
    /// Data frames a superframe, and mapping frames a data frame (Table 7).
    pub j: usize,
    pub p: usize,
    /// Bits a data frame, and a high mapping frame (8-1, Table 8).
    pub n: usize,
    pub b: usize,
    /// High mapping frames in a data frame, and which ones: bit P-1 is the
    /// first mapping frame (Table 8's SWP).
    pub r: usize,
    pub swp: u16,
    /// Auxiliary bits a data frame, and where they go (Table 9).
    pub w: usize,
    pub amp: u16,
    /// Shell mapper bits a mapping frame, label bits a 2D symbol, rings, and
    /// points in the 2D constellation (9.2, Table 10).
    pub k: usize,
    pub q: usize,
    pub m: usize,
    pub l: usize,
}

/// Table 7's J and P.
pub fn j_and_p(rate: SymbolRate) -> (usize, usize) {
    match rate {
        SymbolRate::S2400 => (7, 12),
        SymbolRate::S2743 => (8, 12),
        SymbolRate::S2800 => (7, 14),
        SymbolRate::S3000 => (7, 15),
        SymbolRate::S3200 => (7, 16),
        SymbolRate::S3429 => (8, 15),
    }
}

/// Table 9's W: auxiliary bits a data frame.
fn auxiliary_bits(rate: SymbolRate) -> usize {
    match rate {
        SymbolRate::S2743 | SymbolRate::S3429 => 7,
        _ => 8,
    }
}

/// The counter of 8.2 and 8.3: add `step` at each of `p` frames, and mark the
/// frame when it reaches `p`. The first frame is the most significant bit.
fn pattern(step: usize, p: usize) -> u16 {
    let mut counter = 0;
    let mut bits = 0u16;
    for _ in 0..p {
        counter += step;
        bits <<= 1;
        if counter >= p {
            counter -= p;
            bits |= 1;
        }
    }
    bits
}

impl Framing {
    /// Data mode at `primary` bit/s, or None if the symbol rate has no row for
    /// it in Table 8.
    pub fn new(rate: SymbolRate, primary: u32, auxiliary: bool, expanded: bool) -> Option<Self> {
        let (j, p) = j_and_p(rate);
        let lowest = if rate == SymbolRate::S2400 { 2400 } else { 4800 };
        let highest = match rate {
            SymbolRate::S2400 => 21_600,
            SymbolRate::S2743 | SymbolRate::S2800 => 26_400,
            SymbolRate::S3000 => 28_800,
            SymbolRate::S3200 => 31_200,
            SymbolRate::S3429 => 33_600,
        };
        if !primary.is_multiple_of(2400) || !(lowest..=highest).contains(&primary) {
            return None;
        }
        let total = primary + if auxiliary { 200 } else { 0 };
        // N = R x 0.28 / J, which is whole for every row of the table.
        let n = (total as usize * 28) / (100 * j);
        let b = n.div_ceil(p);
        let r = n - (b - 1) * p;
        let w = auxiliary_bits(rate);
        // 9-1: K = b - 12 - 8q, q the least that brings it under 32.
        let (k, q) = if b <= 12 {
            (0, 0)
        } else {
            let mut q = 0;
            while b - 12 - 8 * q >= 32 {
                q += 1;
            }
            (b - 12 - 8 * q, q)
        };
        let root = 2f64.powf(k as f64 / 8.0);
        let minimum = root.ceil() as usize;
        let m = if expanded { ((1.25 * root).round() as usize).max(minimum) } else { minimum };
        Some(Self {
            rate,
            primary,
            auxiliary,
            expanded,
            j,
            p,
            n,
            b,
            r,
            swp: pattern(r, p),
            w,
            amp: pattern(w, p),
            k,
            q,
            m,
            l: (4 * m) << q,
        })
    }

    /// Whether mapping frame `index` of a data frame is a high frame.
    pub fn high(&self, index: usize) -> bool {
        self.swp >> (self.p - 1 - index % self.p) & 1 == 1
    }

    /// Whether mapping frame `index` of a data frame carries an auxiliary bit.
    pub fn auxiliary_in(&self, index: usize) -> bool {
        self.auxiliary && self.amp >> (self.p - 1 - index % self.p) & 1 == 1
    }

    /// Bits in mapping frame `index` of a data frame.
    pub fn bits_in(&self, index: usize) -> usize {
        if self.high(index) { self.b } else { self.b - 1 }
    }

    /// The precoder's scale factor w of 9-29: 1 below 56 bits a high frame and
    /// 2 from it.
    pub fn precoder_scale(&self) -> i64 {
        if self.b < 56 { 1 } else { 2 }
    }

    /// 2D symbols in a data frame.
    pub fn symbols_per_data_frame(&self) -> usize {
        8 * self.p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b_and_swp_are_table_8s() {
        // Rows read off the PDF, across every symbol rate and both ends of
        // each column, the auxiliary rows among them.
        let rows = [
            (SymbolRate::S2400, 2400, false, 8, 0xfff),
            (SymbolRate::S2400, 2400, true, 9, 0x6db),
            (SymbolRate::S2400, 21_600, false, 72, 0xfff),
            (SymbolRate::S2400, 21_600, true, 73, 0x6db),
            (SymbolRate::S2743, 4800, false, 14, 0xfff),
            (SymbolRate::S2743, 26_400, true, 78, 0x56b),
            (SymbolRate::S2800, 4800, false, 14, 0x1bb7),
            (SymbolRate::S2800, 9600, true, 28, 0x3fff),
            (SymbolRate::S2800, 26_400, false, 76, 0x0a95),
            (SymbolRate::S3000, 4800, false, 13, 0x3def),
            (SymbolRate::S3000, 12_000, true, 33, 0x2aab),
            (SymbolRate::S3000, 28_800, true, 78, 0x1249),
            (SymbolRate::S3200, 7200, true, 19, 0x5555),
            (SymbolRate::S3200, 31_200, false, 78, 0xffff),
            (SymbolRate::S3429, 4800, false, 12, 0x0421),
            (SymbolRate::S3429, 14_400, true, 35, 0x0001),
            (SymbolRate::S3429, 31_200, false, 73, 0x3def),
            (SymbolRate::S3429, 31_200, true, 74, 0x0889),
            (SymbolRate::S3429, 33_600, false, 79, 0x14a5),
            (SymbolRate::S3429, 33_600, true, 79, 0x3f7f),
        ];
        for (rate, primary, auxiliary, b, swp) in rows {
            let f = Framing::new(rate, primary, auxiliary, false).unwrap();
            assert_eq!((f.b, f.swp), (b, swp), "{rate:?} at {primary} {auxiliary}");
            // The right-most bit is always a high frame, and the average is N/P.
            assert!(f.high(f.p - 1));
            let total: usize = (0..f.p).map(|i| f.bits_in(i)).sum();
            assert_eq!(total, f.n);
        }
        assert!(Framing::new(SymbolRate::S3200, 33_600, false, false).is_none());
        assert!(Framing::new(SymbolRate::S2743, 2400, false, false).is_none());
    }

    #[test]
    fn w_and_amp_are_table_9s() {
        let rows = [
            (SymbolRate::S2400, 8, 12, 0x6db),
            (SymbolRate::S2743, 7, 12, 0x56b),
            (SymbolRate::S2800, 8, 14, 0x15ab),
            (SymbolRate::S3000, 8, 15, 0x2aab),
            (SymbolRate::S3200, 8, 16, 0x5555),
            (SymbolRate::S3429, 7, 15, 0x1555),
        ];
        for (rate, w, p, amp) in rows {
            let f = Framing::new(rate, 4800, true, false).unwrap();
            assert_eq!((f.w, f.p, f.amp), (w, p, amp), "{rate:?}");
            // 200 bit/s is 56 bits a superframe: W a data frame, J of them.
            assert_eq!(f.w * f.j, 56);
        }
    }

    #[test]
    fn k_m_and_l_are_table_10s() {
        let rows = [
            // (rate, primary, auxiliary, K, M minimum, M expanded, L minimum, L expanded)
            (SymbolRate::S2400, 2400, false, 0, 1, 1, 4, 4),
            (SymbolRate::S2400, 7200, false, 12, 3, 4, 12, 16),
            (SymbolRate::S2400, 9600, false, 20, 6, 7, 24, 28),
            (SymbolRate::S2400, 21_600, true, 29, 13, 15, 832, 960),
            (SymbolRate::S2743, 4800, false, 2, 2, 2, 8, 8),
            (SymbolRate::S2743, 14_400, false, 30, 14, 17, 56, 68),
            (SymbolRate::S2743, 26_400, false, 25, 9, 11, 1152, 1408),
            (SymbolRate::S3200, 17_000 - 200, true, 31, 15, 18, 60, 72),
            (SymbolRate::S3200, 31_200, false, 26, 10, 12, 1280, 1536),
            (SymbolRate::S3200, 31_200, true, 27, 11, 13, 1408, 1664),
            (SymbolRate::S3429, 4800, false, 0, 1, 1, 4, 4),
            (SymbolRate::S3429, 9600, true, 11, 3, 3, 12, 12),
            (SymbolRate::S3429, 19_200, false, 25, 9, 11, 72, 88),
            (SymbolRate::S3429, 28_800, false, 24, 8, 10, 512, 640),
            (SymbolRate::S3429, 31_200, false, 29, 13, 15, 832, 960),
            (SymbolRate::S3429, 31_200, true, 30, 14, 17, 896, 1088),
            (SymbolRate::S3429, 33_600, false, 27, 11, 13, 1408, 1664),
            (SymbolRate::S3429, 33_600, true, 27, 11, 13, 1408, 1664),
        ];
        for (rate, primary, auxiliary, k, m_min, m_exp, l_min, l_exp) in rows {
            let minimum = Framing::new(rate, primary, auxiliary, false).unwrap();
            let expanded = Framing::new(rate, primary, auxiliary, true).unwrap();
            assert_eq!(
                (minimum.k, minimum.m, expanded.m, minimum.l, expanded.l),
                (k, m_min, m_exp, l_min, l_exp),
                "{rate:?} at {primary} {auxiliary}"
            );
        }
    }

    #[test]
    fn every_mapping_frame_splits_into_shell_bits_and_four_equal_groups() {
        for rate in SymbolRate::ALL {
            for primary in (2400..=33_600).step_by(2400) {
                for auxiliary in [false, true] {
                    let Some(f) = Framing::new(rate, primary, auxiliary, false) else { continue };
                    if f.b > 12 {
                        // b - K is four groups of 3 + 2q.
                        assert_eq!(f.b - f.k, 4 * (3 + 2 * f.q), "{rate:?} at {primary}");
                    } else {
                        assert!(matches!(f.b, 8 | 9 | 11 | 12), "{rate:?} at {primary}: b {}", f.b);
                    }
                    assert!(f.l <= 1664, "{rate:?} at {primary}: L {}", f.l);
                }
            }
        }
    }
}
