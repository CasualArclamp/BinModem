//! Reed-Solomon over GF(256), correcting errors and erasures.
//!
//! The one code every V.ARC mode shares. On a sound card it mops up the
//! occasional wrong decision on a densely loaded carrier; across RTP packets
//! it is used the other way round, where a lost packet is not a wrong octet
//! but a missing one whose position the sequence numbers give away, and a code
//! with `2t` check octets rebuilds `2t` of those where it could only correct
//! `t` it had to find for itself.
//!
//! The field is the usual one, x^8 + x^4 + x^3 + x^2 + 1 with alpha = 2, and
//! the generator's roots are alpha^0 .. alpha^(2t-1). Nothing here has to
//! agree with anybody else's code, since V.ARC is ours at both ends; these
//! are simply the choices with the most textbooks behind them.
//!
//! A codeword is written highest power first: the data octets, then the check
//! octets. A shortened code is the full one with leading zeros nobody sends.

/// The field polynomial, x^8 + x^4 + x^3 + x^2 + 1.
const FIELD: u16 = 0x11d;

/// The longest codeword the field allows.
pub const MAX_BLOCK: usize = 255;

/// A block the decoder could not put right: more went wrong than the check
/// octets cover. The block is left as it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uncorrectable;

/// A Reed-Solomon code with a fixed number of check octets.
#[derive(Debug, Clone)]
pub struct ReedSolomon {
    parity: usize,
    exp: [u8; 512],
    log: [u8; 256],
    /// The generator polynomial, highest power first, without its leading 1.
    generator: Vec<u8>,
}

impl ReedSolomon {
    /// A code with `parity` check octets, correcting `parity / 2` errors or
    /// `parity` erasures or any mixture where twice the errors plus the
    /// erasures is no more than `parity`.
    pub fn new(parity: usize) -> Self {
        assert!(
            (2..MAX_BLOCK).contains(&parity),
            "check octets out of range"
        );
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        for (i, slot) in exp.iter_mut().enumerate().take(255) {
            *slot = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= FIELD;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        let mut code = Self {
            parity,
            exp,
            log,
            generator: Vec::new(),
        };
        // g(x) = (x - a^0)(x - a^1)...(x - a^(2t-1)), built lowest power
        // first and then turned round.
        let mut g = vec![1u8];
        for i in 0..parity {
            let root = code.exp[i];
            let mut next = vec![0u8; g.len() + 1];
            for (j, &c) in g.iter().enumerate() {
                next[j + 1] ^= c;
                next[j] ^= code.mul(c, root);
            }
            g = next;
        }
        g.pop();
        g.reverse();
        code.generator = g;
        code
    }

    /// How many check octets a block carries.
    pub fn parity(&self) -> usize {
        self.parity
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }

    fn inv(&self, a: u8) -> u8 {
        self.exp[255 - self.log[a as usize] as usize]
    }

    /// alpha to the power `p`, for any `p` including negative ones.
    fn alpha(&self, p: i32) -> u8 {
        self.exp[p.rem_euclid(255) as usize]
    }

    /// The check octets for `data`, which with them must fit in a block.
    pub fn encode(&self, data: &[u8], parity: &mut [u8]) {
        assert_eq!(parity.len(), self.parity);
        assert!(data.len() + self.parity <= MAX_BLOCK, "block too long");
        parity.fill(0);
        for &d in data {
            let feedback = d ^ parity[0];
            parity.copy_within(1.., 0);
            parity[self.parity - 1] = 0;
            if feedback != 0 {
                for (p, &g) in parity.iter_mut().zip(&self.generator) {
                    *p ^= self.mul(feedback, g);
                }
            }
        }
    }

    fn syndromes(&self, block: &[u8], out: &mut [u8]) -> bool {
        let mut clean = true;
        for (j, s) in out.iter_mut().enumerate() {
            let root = self.exp[j];
            let mut acc = 0u8;
            for &b in block {
                acc = self.mul(acc, root) ^ b;
            }
            *s = acc;
            clean &= acc == 0;
        }
        clean
    }

    /// Put a block right in place. `erasures` are the indices of octets known
    /// to be missing, whatever value they hold. Returns how many octets were
    /// changed.
    pub fn decode(&self, block: &mut [u8], erasures: &[usize]) -> Result<usize, Uncorrectable> {
        let n = block.len();
        let two_t = self.parity;
        assert!(n > two_t && n <= MAX_BLOCK, "block length out of range");
        if erasures.len() > two_t || erasures.iter().any(|&i| i >= n) {
            return Err(Uncorrectable);
        }
        let mut s = vec![0u8; two_t];
        if self.syndromes(block, &mut s) {
            return Ok(0);
        }

        // The erasure locator, lowest power first: a root at X^-1 for every
        // erased position X = alpha^(n-1-index).
        let mut lambda = vec![0u8; two_t + 2];
        lambda[0] = 1;
        for (degree, &index) in erasures.iter().enumerate() {
            let x = self.alpha((n - 1 - index) as i32);
            for j in (1..=degree + 1).rev() {
                lambda[j] ^= self.mul(lambda[j - 1], x);
            }
        }

        // Berlekamp-Massey, started from the erasure locator so that what it
        // goes on to find is only the errors nobody pointed out (Blahut,
        // "Algebraic Codes for Data Transmission", 7.6).
        let e = erasures.len();
        let mut l = e;
        let mut b = lambda.clone();
        for r in e..two_t {
            let mut delta = 0u8;
            for j in 0..=l.min(r) {
                delta ^= self.mul(lambda[j], s[r - j]);
            }
            // x * B, wanted whichever way this step goes.
            let mut shifted = vec![0u8; b.len()];
            shifted[1..].copy_from_slice(&b[..b.len() - 1]);
            if delta == 0 {
                b = shifted;
                continue;
            }
            let mut next = lambda.clone();
            for (t, &c) in next.iter_mut().zip(&shifted) {
                *t ^= self.mul(delta, c);
            }
            if 2 * l <= r + e {
                let scale = self.inv(delta);
                b = lambda.iter().map(|&c| self.mul(c, scale)).collect();
                l = r + 1 + e - l;
            } else {
                b = shifted;
            }
            lambda = next;
        }
        if l > two_t || lambda[l] == 0 {
            return Err(Uncorrectable);
        }

        // Chien search over the positions that exist. A root anywhere else
        // means the locator is not a locator, and the count will not match.
        let mut found: Vec<usize> = Vec::with_capacity(l);
        for p in 0..n {
            let x_inv = self.alpha(-(p as i32));
            let mut acc = 0u8;
            for &c in lambda[..=l].iter().rev() {
                acc = self.mul(acc, x_inv) ^ c;
            }
            if acc == 0 {
                found.push(p);
            }
        }
        if found.len() != l {
            return Err(Uncorrectable);
        }

        // The evaluator, S(x) * Lambda(x) mod x^2t.
        let mut omega = vec![0u8; two_t];
        for (i, slot) in omega.iter_mut().enumerate() {
            let mut acc = 0u8;
            for j in 0..=i.min(l) {
                acc ^= self.mul(lambda[j], s[i - j]);
            }
            *slot = acc;
        }

        // Forney. With the first root at alpha^0 the magnitude is
        // X * Omega(X^-1) / Lambda'(X^-1).
        let mut fixed = block.to_vec();
        let mut changed = 0;
        for &p in &found {
            let x = self.alpha(p as i32);
            let x_inv = self.alpha(-(p as i32));
            let mut num = 0u8;
            for &c in omega.iter().rev() {
                num = self.mul(num, x_inv) ^ c;
            }
            // The formal derivative keeps only the odd powers.
            let mut den = 0u8;
            let x_inv2 = self.mul(x_inv, x_inv);
            let mut power = 1u8;
            for j in (1..=l).step_by(2) {
                den ^= self.mul(lambda[j], power);
                power = self.mul(power, x_inv2);
            }
            if den == 0 {
                return Err(Uncorrectable);
            }
            let magnitude = self.mul(x, self.mul(num, self.inv(den)));
            if magnitude != 0 {
                changed += 1;
            }
            fixed[n - 1 - p] ^= magnitude;
        }

        // A decoder past its limit can land on some other codeword's
        // neighbourhood and "correct" towards it. That cannot be told from
        // here, but a result that is not a codeword at all can.
        if !self.syndromes(&fixed, &mut s) {
            return Err(Uncorrectable);
        }
        block.copy_from_slice(&fixed);
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small generator so the tests say the same thing every run.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
        fn below(&mut self, n: usize) -> usize {
            self.next() as usize % n
        }
    }

    fn block(code: &ReedSolomon, rng: &mut Lcg, n: usize) -> Vec<u8> {
        let k = n - code.parity();
        let mut block: Vec<u8> = (0..k).map(|_| rng.next() as u8).collect();
        let mut parity = vec![0u8; code.parity()];
        code.encode(&block, &mut parity);
        block.extend_from_slice(&parity);
        block
    }

    /// `count` distinct positions in a block of `n`.
    fn positions(rng: &mut Lcg, n: usize, count: usize) -> Vec<usize> {
        let mut all: Vec<usize> = (0..n).collect();
        for i in 0..count {
            let j = i + rng.below(n - i);
            all.swap(i, j);
        }
        all.truncate(count);
        all
    }

    #[test]
    fn a_clean_block_is_left_alone() {
        let code = ReedSolomon::new(16);
        let mut rng = Lcg(1);
        let sent = block(&code, &mut rng, 255);
        let mut got = sent.clone();
        assert_eq!(code.decode(&mut got, &[]), Ok(0));
        assert_eq!(got, sent);
    }

    #[test]
    fn errors_up_to_half_the_check_octets_are_corrected() {
        let mut rng = Lcg(2);
        for &parity in &[2usize, 8, 16, 32] {
            let code = ReedSolomon::new(parity);
            for &n in &[255usize, 200, parity + 1, parity + 17] {
                for errors in 0..=parity / 2 {
                    let sent = block(&code, &mut rng, n);
                    let mut got = sent.clone();
                    for p in positions(&mut rng, n, errors.min(n)) {
                        got[p] ^= 1 + rng.below(255) as u8;
                    }
                    assert!(
                        code.decode(&mut got, &[]).is_ok(),
                        "parity {parity} n {n} errors {errors}"
                    );
                    assert_eq!(got, sent, "parity {parity} n {n} errors {errors}");
                }
            }
        }
    }

    #[test]
    fn erasures_up_to_all_the_check_octets_are_filled_in() {
        let mut rng = Lcg(3);
        let code = ReedSolomon::new(16);
        for erased in 0..=16 {
            let sent = block(&code, &mut rng, 255);
            let mut got = sent.clone();
            let gone = positions(&mut rng, 255, erased);
            for &p in &gone {
                got[p] = 0;
            }
            assert!(code.decode(&mut got, &gone).is_ok(), "{erased} erasures");
            assert_eq!(got, sent, "{erased} erasures");
        }
    }

    #[test]
    fn errors_and_erasures_together_are_corrected_to_the_bound() {
        let mut rng = Lcg(4);
        let code = ReedSolomon::new(16);
        for erased in 0..=16usize {
            let errors = (16 - erased) / 2;
            for _ in 0..20 {
                let n = 40 + rng.below(216);
                let sent = block(&code, &mut rng, n);
                let mut got = sent.clone();
                let hit = positions(&mut rng, n, erased + errors);
                let (gone, wrong) = hit.split_at(erased);
                for &p in gone {
                    got[p] = rng.next() as u8;
                }
                for &p in wrong {
                    got[p] ^= 1 + rng.below(255) as u8;
                }
                assert!(
                    code.decode(&mut got, gone).is_ok(),
                    "{erased} erased, {errors} wrong"
                );
                assert_eq!(got, sent, "{erased} erased, {errors} wrong");
            }
        }
    }

    #[test]
    fn too_many_errors_are_reported_and_the_block_left_as_it_came() {
        let mut rng = Lcg(5);
        let code = ReedSolomon::new(16);
        let mut miscorrected = 0;
        for _ in 0..200 {
            let sent = block(&code, &mut rng, 255);
            let mut got = sent.clone();
            for p in positions(&mut rng, 255, 12) {
                got[p] ^= 1 + rng.below(255) as u8;
            }
            let arrived = got.clone();
            match code.decode(&mut got, &[]) {
                Err(Uncorrectable) => assert_eq!(got, arrived),
                Ok(_) => miscorrected += 1,
            }
        }
        // Twelve errors against a limit of eight lands inside another
        // codeword's sphere about once in 8! tries.
        assert_eq!(miscorrected, 0);
    }
}
