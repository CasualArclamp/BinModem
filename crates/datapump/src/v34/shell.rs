//! The shell mapper of 9.4, and its inverse.
//!
//! K bits a mapping frame choose eight ring indices, one for each 2D symbol of
//! the frame's four 4D symbols. The rings are concentric shells of the 2D
//! constellation, and the mapping hands out the cheap combinations first: all
//! eight-ring combinations are put in order of their total ring index, and the
//! K bits, read as a number, pick one from the front of that order. So small
//! rings -- low energy -- come up more often than large ones, which is the
//! shaping gain.
//!
//! The ordering is fixed exactly by 9.4's counting functions. g2(p) is how many
//! pairs of rings sum to p, g4 and g8 the same for four and eight, z8(p) how
//! many eight-ring combinations sum to less than p. The algorithm peels the
//! number apart by those counts, halving the problem each time: which total,
//! how the total splits between the two halves of the frame, how each half's
//! splits between its two 4D symbols, and which pair of rings makes each 4D
//! symbol's share.

/// The counting functions for a number of rings.
#[derive(Debug, Clone)]
pub struct Shell {
    m: usize,
    g2: Vec<u64>,
    g4: Vec<u64>,
    g8: Vec<u64>,
    z8: Vec<u64>,
}

impl Shell {
    pub fn new(m: usize) -> Self {
        let m = m.max(1);
        let g2: Vec<u64> = (0..=2 * (m - 1)).map(|p| (m as i64 - (p as i64 - m as i64 + 1).abs()) as u64).collect();
        let convolve = |a: &[u64]| -> Vec<u64> {
            (0..2 * a.len() - 1)
                .map(|p| (0..=p).filter(|&i| i < a.len() && p - i < a.len()).map(|i| a[i] * a[p - i]).sum())
                .collect()
        };
        let g4 = convolve(&g2);
        let g8 = convolve(&g4);
        let mut z8 = Vec::with_capacity(g8.len() + 1);
        let mut total = 0;
        for &count in &g8 {
            z8.push(total);
            total += count;
        }
        z8.push(total);
        Self { m, g2, g4, g8, z8 }
    }

    pub fn rings(&self) -> usize {
        self.m
    }

    fn g2(&self, p: i64) -> u64 {
        if p < 0 { 0 } else { self.g2.get(p as usize).copied().unwrap_or(0) }
    }

    fn g4(&self, p: i64) -> u64 {
        if p < 0 { 0 } else { self.g4.get(p as usize).copied().unwrap_or(0) }
    }

    /// Eight-ring combinations there are in all.
    pub fn combinations(&self) -> u64 {
        *self.z8.last().expect("z8 always has its end")
    }

    /// The eight ring indices for `r0`, the K bits read least significant
    /// first: m(0,0), m(0,1), m(1,0) ... m(3,1).
    pub fn map(&self, r0: u64) -> [usize; 8] {
        let m = self.m as i64;
        // 2) The largest A with z8(A) <= R0.
        let a = self.z8.iter().rposition(|&z| z <= r0).expect("z8(0) is 0");
        let a = a.min(self.g8.len() - 1) as i64;
        // 3) The largest B with R1 >= 0.
        let mut r1 = r0 - self.z8[a as usize];
        let mut b = 0;
        while b < a {
            let take = self.g4(b) * self.g4(a - b);
            if take > r1 {
                break;
            }
            r1 -= take;
            b += 1;
        }
        // 4)
        let r2 = r1 % self.g4(b);
        let r3 = r1 / self.g4(b);
        // 5.1) and 5.2)
        let split = |mut rest: u64, total: i64| {
            let mut c = 0;
            while c < total {
                let take = self.g2(c) * self.g2(total - c);
                if take > rest {
                    break;
                }
                rest -= take;
                c += 1;
            }
            (c, rest)
        };
        let (c, r4) = split(r2, b);
        let (d, r5) = split(r3, a - b);
        // 6.1) and 6.2)
        let (e, f) = (r4 % self.g2(c), r4 / self.g2(c));
        let (g, h) = (r5 % self.g2(d), r5 / self.g2(d));
        // 9-17 to 9-24: each 4D symbol's share and which pair of rings makes it.
        let pair = |total: i64, index: u64| -> (usize, usize) {
            if total < m {
                let first = index as i64;
                (first as usize, (total - first) as usize)
            } else {
                let second = m - 1 - index as i64;
                ((total - second) as usize, second as usize)
            }
        };
        let (m00, m01) = pair(c, e);
        let (m10, m11) = pair(b - c, f);
        let (m20, m21) = pair(d, g);
        let (m30, m31) = pair(a - b - d, h);
        [m00, m01, m10, m11, m20, m21, m30, m31]
    }

    /// The number the eight ring indices came from.
    pub fn unmap(&self, rings: [usize; 8]) -> u64 {
        let m = self.m as i64;
        let r = rings.map(|x| x as i64);
        // Which of a share's pairs a pair is, the other way round.
        let index = |first: i64, second: i64| -> u64 {
            let total = first + second;
            if total < m { first as u64 } else { (m - 1 - second) as u64 }
        };
        let c = r[0] + r[1];
        let b = c + r[2] + r[3];
        let d = r[4] + r[5];
        let a = b + d + r[6] + r[7];
        let (e, f) = (index(r[0], r[1]), index(r[2], r[3]));
        let (g, h) = (index(r[4], r[5]), index(r[6], r[7]));
        let r4 = e + f * self.g2(c);
        let r5 = g + h * self.g2(d);
        let r2 = r4 + (0..c).map(|p| self.g2(p) * self.g2(b - p)).sum::<u64>();
        let r3 = r5 + (0..d).map(|p| self.g2(p) * self.g2(a - b - p)).sum::<u64>();
        let r1 = r2 + r3 * self.g4(b);
        r1 + self.z8[a as usize] + (0..b).map(|p| self.g4(p) * self.g4(a - p)).sum::<u64>()
    }

    /// How often each ring comes up at each of the eight places, over every
    /// value of `k` bits: the numbers the average energy of a shaped
    /// constellation is weighed by.
    ///
    /// Exact where the combinations used are whole classes of total ring
    /// index, and the part-class at the end is weighed as its class is on
    /// average -- which is a fraction of one class out of dozens.
    pub fn ring_shares(&self, k: usize) -> Vec<f64> {
        let used = 1u64 << k;
        let m = self.m;
        // Seven-ring counts, to count how often ring x sits at one place among
        // the combinations of a total.
        let g7 = {
            let mut g = vec![1u64];
            for _ in 0..7 {
                let mut next = vec![0u64; g.len() + m - 1];
                for (i, &count) in g.iter().enumerate() {
                    for x in 0..m {
                        next[i + x] += count;
                    }
                }
                g = next;
            }
            g
        };
        let mut shares = vec![0.0; m];
        for (total, &count) in self.g8.iter().enumerate() {
            let before = self.z8[total];
            if before >= used {
                break;
            }
            let fraction = ((used - before) as f64 / count as f64).min(1.0);
            for (x, share) in shares.iter_mut().enumerate() {
                if let Some(&rest) = total.checked_sub(x).and_then(|t| g7.get(t)) {
                    *share += fraction * rest as f64;
                }
            }
        }
        let sum: f64 = shares.iter().sum();
        shares.iter().map(|s| s / sum).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counting_functions_count_what_they_say() {
        let shell = Shell::new(3);
        // Pairs of rings 0..2 summing to 0..4: 1, 2, 3, 2, 1.
        assert_eq!(shell.g2, vec![1, 2, 3, 2, 1]);
        // Every combination of eight rings is counted once.
        assert_eq!(shell.combinations(), 3u64.pow(8));
        assert_eq!(Shell::new(18).combinations(), 18u64.pow(8));
    }

    #[test]
    fn every_number_maps_to_rings_and_back() {
        for (m, k) in [(2, 5), (3, 11), (4, 13), (5, 17), (8, 23)] {
            let shell = Shell::new(m);
            assert!(shell.combinations() >= 1 << k, "M {m} cannot carry {k} bits");
            let step = ((1u64 << k) / 5000).max(1);
            let mut r0 = 0;
            while r0 < 1 << k {
                let rings = shell.map(r0);
                assert!(rings.iter().all(|&x| x < m), "M {m}: {r0} gave {rings:?}");
                assert_eq!(shell.unmap(rings), r0, "M {m}: {rings:?}");
                r0 += step;
            }
            let last = (1 << k) - 1;
            assert_eq!(shell.unmap(shell.map(last)), last);
        }
    }

    #[test]
    fn the_largest_constellations_map_their_largest_numbers() {
        // K of 31 at M 15 and 18, and K of 27 at M 13, the 33 600 row.
        for (m, k) in [(15, 31), (18, 31), (11, 27), (13, 27), (13, 29), (15, 29)] {
            let shell = Shell::new(m);
            for r0 in [0, 1, (1u64 << k) / 3, (1 << k) - 2, (1 << k) - 1] {
                let rings = shell.map(r0);
                assert!(rings.iter().all(|&x| x < m));
                assert_eq!(shell.unmap(rings), r0, "M {m} K {k}: {r0}");
            }
        }
    }

    #[test]
    fn the_cheapest_combination_comes_first_and_order_follows_the_total() {
        let shell = Shell::new(4);
        assert_eq!(shell.map(0), [0; 8]);
        // The next eight are one ring up at each place in turn.
        let totals: Vec<usize> = (0..500).map(|r0| shell.map(r0).iter().sum()).collect();
        assert!(totals.windows(2).all(|w| w[0] <= w[1]), "totals do not rise");
        assert_eq!(totals[1..9], [1; 8]);
    }

    #[test]
    fn ring_shares_favour_the_inner_rings_and_add_up() {
        let shell = Shell::new(13);
        let shares = shell.ring_shares(29);
        assert!((shares.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(shares.windows(2).all(|w| w[0] >= w[1]), "{shares:?}");
        // Checked against counting the rings of a sample of real frames.
        let mut counts = [0usize; 13];
        let mut seed = 0x1234_5678u64;
        for _ in 0..20_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            for ring in shell.map((seed >> 20) & ((1 << 29) - 1)) {
                counts[ring] += 1;
            }
        }
        for (x, &count) in counts.iter().enumerate() {
            let measured = count as f64 / 160_000.0;
            assert!((measured - shares[x]).abs() < 0.01, "ring {x}: {measured} against {}", shares[x]);
        }
    }
}
