//! x2's downstream mapper: MD + B scrambled bits into one frame of six
//! mu-law octets, and back.
//!
//! The rule (libv90modem's `docs/history/x2-facts.md`, "Mapping" and "Free
//! sign", from D-Modem's `docs/dsp/x2-details.md` "Data" and courier-emu's
//! x2 specification, 10 and 11; facts only, no code of any of them):
//!
//! 1. The first MD bits of a frame are signs; the next B, low-order first,
//!    are a number x.
//! 2. Position i (0 to 5) takes the digit x mod n_i, then x /= n_i: mixed
//!    radix, position 0 the lowest digit, n_i the length of its list.
//! 3. The digit picks a level from the position's list, the largest first.
//! 4. The position with the largest level (of equal levels the last) carries
//!    the free sign; the other five, in order, the MD signs, each as a change
//!    from the last one sent: `last ^= bit`, positive while `last` is set.
//! 5. The free sign carries nothing and the client does not check it. In
//!    levels A it is negative when the other five sum to more than 805 in
//!    14-bit mu-law units (every frame the I-modem sent agrees); in the data
//!    levels negative when the running sum of everything sent, this frame's
//!    five included, is above zero (D-Modem's choice; the I-modem's own is a
//!    shaping search nobody has lifted, and agrees with it 87 % of the time).

use super::bank::{Bank, FRAME, MD};
use crate::v90::ucode::{self, Law};

/// Levels A's free sign threshold: 805 in 14-bit units, which are a quarter
/// of [`ucode::linear`]'s.
const FRAME_DC: i32 = 805 * 4;

/// The mapper's state across frames.
#[derive(Debug, Clone)]
pub struct Mapper {
    bank: &'static Bank,
    levels_a: bool,
    /// The last sign that carried a bit: true positive.
    last: bool,
    /// The running sum of every value sent, in [`ucode::linear`]'s scale.
    rds: i64,
}

impl Mapper {
    /// Levels A, the differential sign positive and the running sum zero, as
    /// at the first sync frame.
    pub fn levels_a() -> Self {
        Self {
            bank: &Bank::LEVELS_A,
            levels_a: true,
            last: true,
            rds: 0,
        }
    }

    /// A data bank, from a fresh start (a rate change's scrambler reset).
    pub fn data(bank: &'static Bank) -> Self {
        Self {
            bank,
            levels_a: false,
            last: true,
            rds: 0,
        }
    }

    /// Change to a data bank, keeping the signs and the running sum: the
    /// start segment goes from levels A to the data levels without a reset.
    pub fn switch_to(&mut self, bank: &'static Bank) {
        self.bank = bank;
        self.levels_a = false;
    }

    pub fn bank(&self) -> &'static Bank {
        self.bank
    }

    /// Bits a frame takes.
    pub fn bits(&self) -> usize {
        MD + self.bank.b
    }

    /// One frame from [`bits`](Self::bits) scrambled bits.
    pub fn frame(&mut self, bits: &[bool]) -> [u8; FRAME] {
        assert_eq!(bits.len(), self.bits());
        // B is at most 37: the number fits 64 bits.
        let mut x: u64 = bits[MD..]
            .iter()
            .enumerate()
            .fold(0, |x, (k, &b)| x | (u64::from(b) << k));
        let mut level = [0u8; FRAME];
        let mut sent = [0u8; FRAME];
        for i in 0..FRAME {
            let list = self.bank.list(i);
            let n = list.len() as u64;
            let d = (x % n) as usize;
            x /= n;
            level[i] = list.recv[d];
            sent[i] = list.sent[d];
        }
        let free = free_position(&level);
        let mut out = [0u8; FRAME];
        let mut sum = 0i32;
        let mut k = 0;
        for i in 0..FRAME {
            if i == free {
                continue;
            }
            self.last ^= bits[k];
            k += 1;
            out[i] = if self.last { sent[i] } else { sent[i] & 0x7f };
            sum += value(out[i]);
        }
        let negative = if self.levels_a {
            sum > FRAME_DC
        } else {
            self.rds + i64::from(sum) > 0
        };
        out[free] = if negative { sent[free] & 0x7f } else { sent[free] };
        self.rds += i64::from(sum + value(out[free]));
        out
    }

    /// The bits of one frame as the client receives it, for the simulated
    /// client and the tests: `None` when an octet is not a level of its
    /// position, or the digits come to more than B bits. Uses the sign
    /// state only; keep a mapper of its own for it.
    pub fn unframe(&mut self, octets: &[u8; FRAME]) -> Option<Vec<bool>> {
        let mut level = [0u8; FRAME];
        let mut digit = [0usize; FRAME];
        for (i, &octet) in octets.iter().enumerate() {
            level[i] = octet | 0x80;
            digit[i] = self.bank.list(i).recv.iter().position(|&l| l == level[i])?;
        }
        let free = free_position(&level);
        let mut bits = Vec::with_capacity(self.bits());
        for (_, &octet) in octets.iter().enumerate().filter(|&(i, _)| i != free) {
            let positive = octet & 0x80 != 0;
            bits.push(positive != self.last);
            self.last = positive;
        }
        let mut x: u64 = 0;
        for i in (0..FRAME).rev() {
            x = x * self.bank.list(i).len() as u64 + digit[i] as u64;
        }
        if x >> self.bank.b != 0 {
            return None;
        }
        bits.extend((0..self.bank.b).map(|k| (x >> k) & 1 != 0));
        Some(bits)
    }
}

/// The position of the free sign: the largest level, which is the smallest
/// octet; of equal levels the last.
fn free_position(level: &[u8; FRAME]) -> usize {
    (1..FRAME).fold(0, |f, i| if level[i] <= level[f] { i } else { f })
}

fn value(octet: u8) -> i32 {
    let (u, negative) = ucode::from_octet(Law::Mu, octet);
    ucode::amplitude(Law::Mu, u, negative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v32::{Mode, Scrambler};

    /// D-Modem's mapper run as a black box (libv90modem's `test/test_x2map.c`,
    /// "oracle run", 2026-11-20: D-Modem's `X2Map` at bbeb8741e6c1, built and
    /// run outside that repository): for each index and pad, 4000 frames of
    /// xorshift32's low bits from 0x2545F491, scrambled by GPC from state 0,
    /// the sign positive and the running sum zero at the first frame. Frames
    /// 0, 1 and 3999, and the FNV-1a of all 24 000 octets, as it printed them.
    /// Index, pad, frames 0, 1 and 3999, FNV-1a.
    type Run = (u8, u8, [u8; 6], [u8; 6], [u8; 6], u32);

    const ORACLE: [Run; 12] = [
        (1, 0, [0xb3, 0x2e, 0x33, 0x5f, 0xa8, 0x28], [0xae, 0xdf, 0xa8, 0xdf, 0xcb, 0x33], [0x2b, 0xbf, 0xbf, 0x33, 0xa8, 0x4b], 0xcab5c166),
        (8, 0, [0xc0, 0x26, 0x29, 0xa5, 0x49, 0xc6], [0x2f, 0x27, 0xbc, 0xd9, 0xa7, 0x49], [0xba, 0xa3, 0x30, 0xa5, 0x23, 0xac], 0x351348e8),
        (12, 0, [0xb9, 0xa1, 0x2a, 0x3e, 0x2a, 0xb1], [0x38, 0x3a, 0x39, 0xd4, 0x5a, 0xa7], [0xbb, 0x41, 0xd1, 0xbd, 0x23, 0xaf], 0x029f3fd8),
        (14, 0, [0xd7, 0x3f, 0xaf, 0x3e, 0x4d, 0xb3], [0xd1, 0x42, 0x31, 0xce, 0xca, 0xb2], [0x3c, 0xa5, 0x4b, 0x3e, 0x26, 0xbc], 0x7933dbbf),
        (1, 1, [0xac, 0x27, 0x2c, 0x5b, 0x9f, 0x1f], [0xa7, 0xdb, 0x9f, 0xdb, 0xc5, 0x2c], [0x23, 0xba, 0xba, 0x2c, 0x9f, 0x45], 0xacb8ca2b),
        (8, 1, [0xab, 0x35, 0x32, 0x9a, 0x3e, 0xbe], [0x48, 0x3e, 0xc9, 0xd2, 0x1c, 0x41], [0xc2, 0x1c, 0xc5, 0x49, 0xb5, 0xa3], 0xb18f29dd),
        (12, 1, [0xa8, 0x24, 0x30, 0x4d, 0x60, 0xab], [0x20, 0x54, 0x22, 0xd4, 0x42, 0x9f], [0xc6, 0x9f, 0x26, 0xbe, 0xcd, 0xa8], 0xf0211d00),
        (14, 1, [0xaa, 0x3e, 0x26, 0x4f, 0x1f, 0xad], [0xa0, 0x42, 0xb7, 0xd0, 0x1e, 0xac], [0x56, 0x9a, 0x28, 0x3b, 0x2d, 0xb7], 0xccf1a30f),
        (1, 2, [0xa4, 0x1e, 0x24, 0x53, 0x98, 0x18], [0x9e, 0xd3, 0x98, 0xd3, 0xbc, 0x24], [0x1b, 0xb0, 0xb0, 0x24, 0x98, 0x3c], 0xd486af04),
        (8, 2, [0xb1, 0x16, 0x19, 0x95, 0x3a, 0xb7], [0x1f, 0x17, 0xad, 0xcb, 0x97, 0x3a], [0xab, 0x93, 0x21, 0x95, 0x13, 0x9c], 0xc948ee63),
        (12, 2, [0xaa, 0x91, 0x1a, 0x2f, 0x1a, 0xa2], [0x29, 0x2b, 0x2a, 0xc6, 0x4c, 0x97], [0xac, 0x32, 0xc3, 0xae, 0x13, 0x9f], 0xdf56708b),
        (14, 2, [0xc9, 0x30, 0x9f, 0x2f, 0x3e, 0xa4], [0xc3, 0x33, 0x22, 0xbf, 0xbb, 0xa3], [0x2d, 0x95, 0x3c, 0x2f, 0x16, 0xad], 0xf26183ad),
    ];

    #[test]
    fn the_octets_are_d_modems_at_every_rate_and_pad_it_was_run_at() {
        for (index, pad, f0, f1, f3999, fnv) in ORACLE {
            let mut m = Mapper::data(Bank::get(index, pad).unwrap());
            // GPC, the calling modem's polynomial (x2-facts "The scrambler").
            let mut s = Scrambler::new(Mode::Call);
            let mut x: u32 = 0x2545_f491;
            let mut h: u32 = 0x811c_9dc5;
            for f in 0..4000 {
                let bits: Vec<bool> = (0..m.bits())
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        s.scramble(x & 1 != 0)
                    })
                    .collect();
                let out = m.frame(&bits);
                for o in out {
                    h = (h ^ u32::from(o)).wrapping_mul(0x0100_0193);
                }
                match f {
                    0 => assert_eq!(out, f0, "index {index} pad {pad} frame 0"),
                    1 => assert_eq!(out, f1, "index {index} pad {pad} frame 1"),
                    3999 => assert_eq!(out, f3999, "index {index} pad {pad} frame 3999"),
                    _ => {}
                }
            }
            assert_eq!(h, fnv, "index {index} pad {pad}: FNV");
        }
    }

    #[test]
    fn every_bank_unframes_what_it_framed() {
        for pad in 0..3 {
            for index in 1..=15 {
                let bank = Bank::get(index, pad).unwrap();
                let mut tx = Mapper::data(bank);
                let mut rx = Mapper::data(bank);
                let mut x: u32 = 0x1234_5678 ^ u32::from(index) << 8 ^ u32::from(pad);
                for _ in 0..500 {
                    let bits: Vec<bool> = (0..tx.bits())
                        .map(|_| {
                            x ^= x << 13;
                            x ^= x >> 17;
                            x ^= x << 5;
                            x & 1 != 0
                        })
                        .collect();
                    let sent = tx.frame(&bits);
                    // What arrives: each octet sent as the level it stands for.
                    let mut arrived = [0u8; FRAME];
                    for i in 0..FRAME {
                        let list = bank.list(i);
                        let d = list.sent.iter().position(|&o| o == sent[i] | 0x80).unwrap();
                        arrived[i] = list.recv[d] & if sent[i] & 0x80 != 0 { 0xff } else { 0x7f };
                    }
                    assert_eq!(rx.unframe(&arrived).as_deref(), Some(&bits[..]), "index {index} pad {pad}");
                }
            }
        }
    }

    #[test]
    fn levels_a_free_sign_follows_the_frames_own_sum() {
        // All ones scrambled from zero: the sync segment's frames. Every free
        // sign is negative exactly when the other five sum past 805 x 4.
        let mut m = Mapper::levels_a();
        let mut s = Scrambler::new(Mode::Call);
        for _ in 0..200 {
            let bits: Vec<bool> = (0..m.bits()).map(|_| s.scramble(true)).collect();
            let out = m.frame(&bits);
            let level = out.map(|o| o | 0x80);
            let free = free_position(&level);
            let others: i32 = (0..FRAME).filter(|&i| i != free).map(|i| value(out[i])).sum();
            assert_eq!(out[free] & 0x80 == 0, others > FRAME_DC);
        }
    }
}
