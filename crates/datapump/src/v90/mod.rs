//! ITU-T V.90: a digital modem and an analogue modem, 56 000 down and 33 600 up.
//!
//! The asymmetry is the whole idea, and it is not about bandwidth. V.90 1
//! describes "two different modems, one a digital modem and the other an
//! analogue modem": the digital end is wired straight into the telephone
//! network's own digital path, so downstream it does not modulate anything at
//! all. It chooses G.711 codewords and the network carries them as codewords
//! (5.2: "the downstream symbol rate shall be 8000 established by timing from
//! the digital network interface"). The analogue end samples what comes out of
//! the far-end codec and decides which codeword each sample was.
//!
//! So there is no constellation downstream in the sense V.34 has one. There is
//! no I and Q and no carrier: there is a list of amplitudes the network can
//! represent exactly, and the only question per sample is which one. Upstream
//! is ordinary V.34 (1 e), which is why a V.90 call is a V.34 call in one
//! direction and something else entirely in the other.
//!
//! What limits the downstream rate is not noise in the usual sense but how
//! finely the analogue end can tell those amplitudes apart, and how many of
//! them survive the route. Two things take them away: the quiet codes are
//! eight units apart at the bottom of Uchord 1 and no receiver can separate
//! them, and a route that steals a bit for signalling halves what one of the
//! six data frame intervals can carry. That second one costs exactly one bit
//! per data frame, which is exactly one step of the rate ladder -- see [`RATE_STEP`].

pub mod analogue;
/// Named from `datapump::v92` as well as from here: the V.92 analogue and
/// digital modems watch the far end with this same watch (AD-1), and a
/// private module could not be reached from a sibling of `v90`.
pub(crate) mod carrier;
pub mod dil;
pub mod digital;
pub(crate) mod downstream;
pub mod encoder;
pub mod modulus;
pub mod network;
pub mod pcm;
pub mod sequences;
pub mod server;
pub mod sign;
pub mod startup;
pub mod ucode;

use crate::v34::info::{Info0d, UINFO_HIGHEST, UINFO_LOWEST};

/// Table 15/V.90: the root of the average power a constellation set may have,
/// in Table 1's units, for each maximum transmit power INFO0d can name --
/// -0.5 dBm0 first, down to -16 in half decibels.
pub const POWER_LIMITS: [u32; 32] = [
    15124, 14276, 13480, 12724, 12012, 11340, 10708, 10108, 9544, 9008, 8504, 8028, 7580, 7156, 6756, 6380, 6020,
    5684, 5368, 5068, 4784, 4516, 4264, 4024, 3800, 3588, 3388, 3196, 3020, 2852, 2692, 2540,
];

/// The limit Table 15 gives for a digital modem's INFO0d.
pub fn power_limit(far: &Info0d) -> u32 {
    POWER_LIMITS[usize::from(far.max_power).min(POWER_LIMITS.len() - 1)]
}

/// The highest UINFO a V.92 INFO1a may name.
///
/// Tables 18 and 19/V.92 print only the lower bound, "U_INFO shall be greater
/// than 66". The upper one comes from the signal that uses it: 8.6.7/V.92
/// defers to 8.4.4/V.90, where Sd is built from "the PCM codeword whose Ucode
/// is 16 + U_INFO", and there are only 128 Ucodes. A U_INFO above this names a
/// codeword the digital modem could not send at all, so the end that *chooses*
/// one stays inside (P3S 5.7, Sd-5).
///
/// It is [`crate::v34::info::UINFO_HIGHEST`] rather than a second 111: the
/// INFO codec owns the range a sequence may carry, and this is that range's
/// ceiling under the name the PCM side of the modem asks for it by.
pub const V92_MAX_UINFO: u8 = UINFO_HIGHEST;

/// The loudest Ucode at or below `highest` whose amplitude stays inside
/// `limit`, and never below [`UINFO_LOWEST`].
///
/// The whole of UINFO's choice, with the one thing V.92 changes -- the
/// ceiling -- as an argument, so that the two callers differ in nothing else.
fn loudest_codeword(law: ucode::Law, limit: i32, highest: u8) -> u8 {
    (UINFO_LOWEST..=highest).rev().find(|&u| ucode::linear(law, u) <= limit).unwrap_or(UINFO_LOWEST)
}

/// UINFO for a digital modem: the loudest codeword whose power stays inside
/// the digital modem's maximum (Table 10/V.90: "The power of this point shall
/// not exceed the maximum digital modem transmit power. UINFO shall be
/// greater than 66").
///
/// A two-point train has the power of its one codeword, so the codeword's
/// size is held to Table 15's root. A Conexant modem training a server whose
/// ceiling is -12 dBm0 asked for 78, one under the 79 this picks.
pub fn training_codeword(far: &Info0d) -> u8 {
    let law = if far.a_law { ucode::Law::A } else { ucode::Law::Mu };
    loudest_codeword(law, power_limit(far) as i32, (ucode::UCODES - 1) as u8)
}

/// The same choice for a V.92 INFO1a, stopped at [`V92_MAX_UINFO`].
///
/// Table 15's loudest limit is (15124)^2, which no Ucode above 109 stays
/// inside under either companding law, so today this returns exactly what
/// [`training_codeword`] returns for every one of the thirty-two maximum
/// powers an INFO0d can name. The ceiling is here because Table 18's own rule
/// stops at "greater than 66" and says nothing about the top: it is Sd that
/// cannot be built above 111, and a modem that ever reads Table 15's limits
/// differently -- a wider table, a law with louder codes -- must not be able
/// to ask for a codeword the far end could not send.
pub fn training_codeword_v92(far: &Info0d) -> u8 {
    let law = if far.a_law { ucode::Law::A } else { ucode::Law::Mu };
    loudest_codeword(law, power_limit(far) as i32, V92_MAX_UINFO)
}

/// Data frame intervals per data frame (5.4): "data frames in the digital
/// modem have a six-symbol structure".
pub const INTERVALS: usize = 6;

/// The downstream symbol rate (5.2), fixed by the network rather than chosen.
pub const SYMBOL_RATE: u32 = 8000;

/// The rate ladder's step, in bit/s: 8000 symbols a second over six symbols
/// to a data frame is 1333 1/3 data frames a second, so one bit per data
/// frame is 1333 1/3 bit/s.
///
/// Kept as a fraction because it is not a whole number and rounding it makes
/// the ladder drift: 1 a) has the rates running "from 28 000 bit/s to
/// 56 000 bit/s in increments of 8000/6 bit/s".
pub const RATE_STEP: (u32, u32) = (8000, 6);

/// The lowest and highest downstream rates (5.1).
pub const SLOWEST: u32 = 28_000;
pub const FASTEST: u32 = 56_000;

/// Table 2's bounds on K, the modulus encoder's input bits.
pub const K_RANGE: (u32, u32) = (15, 39);
/// And on S, the sign bits carrying user data.
pub const S_RANGE: (u32, u32) = (3, 6);
/// And on D = S + K, which is what settles the rate.
pub const D_RANGE: (u32, u32) = (21, 42);

/// Whether Table 2 has a row for this K and S.
///
/// The table prints twenty-five rows with a range of S against each, and all
/// of it is these three bounds. K = 15 has only S = 6 because anything less
/// would fall below 28 000; K = 39 has only S = 3 because anything more would
/// pass 56 000. Everything between is whatever keeps D on the ladder.
pub fn table_2_has(k: u32, s: u32) -> bool {
    (K_RANGE.0..=K_RANGE.1).contains(&k)
        && (S_RANGE.0..=S_RANGE.1).contains(&s)
        && (D_RANGE.0..=D_RANGE.1).contains(&(k + s))
}

/// The largest K Table 2 allows alongside this many sign bits.
pub fn largest_k(s: u32) -> u32 {
    K_RANGE.1.min(D_RANGE.1.saturating_sub(s))
}

/// The rate a data frame of `bits` carries, rounded down to whole bit/s.
///
/// Table 2 prints the same numbers as mixed fractions -- 29 1/3, 30 2/3 -- and
/// this is the floor of them, which is what a modem reports.
pub fn rate_for(bits: u32) -> u32 {
    bits * RATE_STEP.0 / RATE_STEP.1
}

/// How many data bits a data frame carries at `rate`.
pub fn bits_for(rate: u32) -> u32 {
    // The inverse of `rate_for`, taken on the exact ladder rather than by
    // dividing, so a reported rate that was rounded still lands on its rung.
    (0..=42).min_by_key(|&d| rate_for(d).abs_diff(rate)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 d): "from 28 000 bit/s to 56 000 bit/s in increments of 8000/6 bit/s".
    #[test]
    fn the_ladder_runs_from_twenty_eight_thousand_to_fifty_six() {
        // Table 2's first row is K = 15, S = 6, and its last is K = 39, S = 3.
        assert_eq!(rate_for(15 + 6), SLOWEST);
        assert_eq!(rate_for(39 + 3), FASTEST);
        // Twenty-one rungs between them, which is what makes one bit a frame
        // one step (and is why robbed-bit signalling costs exactly one).
        assert_eq!(42 - 21, 21);
        assert_eq!(rate_for(22), 29_333, "29 1/3 rounded down");
        assert_eq!(rate_for(23), 30_666, "30 2/3 rounded down");
        assert_eq!(rate_for(24), 32_000);
    }

    /// Table 2, read the other way: every rate it prints comes back as the
    /// number of bits that makes it.
    #[test]
    fn a_rate_names_the_bits_that_carry_it() {
        for d in 21..=42u32 {
            assert_eq!(bits_for(rate_for(d)), d, "{d} bits");
        }
        // And a rate quoted as the spec prints it, whole thousands and all,
        // still lands on the right rung.
        assert_eq!(bits_for(56_000), 42);
        assert_eq!(bits_for(28_000), 21);
        assert_eq!(bits_for(44_000), 33);
    }

    /// Table 2, all twenty-five rows of it, against the two inequalities.
    ///
    /// The table gives a range of S for each K and the endpoints of the rate
    /// it produces. Every row is reproduced here from the bounds alone, which
    /// is what says the bounds are the table rather than an approximation of
    /// it.
    #[test]
    fn table_two_is_two_inequalities() {
        // K, the lowest S, the highest S, and the rates at each end, in
        // thousands with the fractions the table prints as sixths.
        let rows: [(u32, u32, u32); 25] = [
            (15, 6, 6),
            (16, 5, 6),
            (17, 4, 6),
            (18, 3, 6),
            (19, 3, 6),
            (20, 3, 6),
            (21, 3, 6),
            (22, 3, 6),
            (23, 3, 6),
            (24, 3, 6),
            (25, 3, 6),
            (26, 3, 6),
            (27, 3, 6),
            (28, 3, 6),
            (29, 3, 6),
            (30, 3, 6),
            (31, 3, 6),
            (32, 3, 6),
            (33, 3, 6),
            (34, 3, 6),
            (35, 3, 6),
            (36, 3, 6),
            (37, 3, 5),
            (38, 3, 4),
            (39, 3, 3),
        ];
        for (k, lowest, highest) in rows {
            for s in 0..=8u32 {
                let allowed = (lowest..=highest).contains(&s);
                assert_eq!(table_2_has(k, s), allowed, "K {k}, S {s}");
            }
            assert_eq!(largest_k(highest), k.max(largest_k(highest)));
        }
        // The first row's rate and the last row's, which are the ends of 5.1.
        assert_eq!(rate_for(15 + 6), SLOWEST);
        assert_eq!(rate_for(39 + 3), FASTEST);
        // And nothing outside K's own range has a row at all.
        assert!(!table_2_has(14, 6));
        assert!(!table_2_has(40, 3));
    }

    /// Table 15 is a half-decibel ladder from -0.5 dBm0, which is what its
    /// numbers are: each about 0.944 of the one before.
    #[test]
    fn table_fifteen_falls_half_a_decibel_a_row() {
        for pair in POWER_LIMITS.windows(2) {
            let ratio = f64::from(pair[1]) / f64::from(pair[0]);
            assert!((ratio - 10f64.powf(-0.5 / 20.0)).abs() < 0.002, "{pair:?}");
        }
        // -12 dBm0 is 23 rows down, and -6 dBm0 is 11.
        assert_eq!(POWER_LIMITS[23], 4024);
        assert_eq!(POWER_LIMITS[11], 8028);
    }

    /// The training codeword for the server on the recording, whose
    /// ceiling is -12 dBm0.
    #[test]
    fn uinfo_is_the_loudest_codeword_under_the_ceiling() {
        let mut far = Info0d { max_power: 23, ..Info0d::default() };
        assert_eq!(training_codeword(&far), 79);
        assert!(ucode::linear(ucode::Law::Mu, 79) <= 4024);
        assert!(ucode::linear(ucode::Law::Mu, 80) > 4024);
        // Louder allowed, louder asked for; and never 66 or under.
        far.max_power = 0;
        assert!(training_codeword(&far) > 79);
        far.max_power = 31;
        assert!(training_codeword(&far) > 66);
        far.a_law = true;
        assert!(ucode::linear(ucode::Law::A, training_codeword(&far)) <= 2540);
    }

    /// Sd is 16 + UINFO (8.4.4/V.90, kept by 8.6.7/V.92), so a V.92 INFO1a's
    /// UINFO has to leave that codeword inside the 128 there are.
    ///
    /// Table 15/V.90's loudest limit is (15124)^2, and no Ucode above 109
    /// stays inside it under either law, so the V.92 ceiling costs nothing
    /// against a conforming INFO0d: every one of the thirty-two maximum
    /// powers gives the same answer as V.90's own choice, which is what says
    /// the cap has not quietly changed V.90. What the cap is for is the
    /// search itself, tested here with the limit taken away.
    #[test]
    fn uinfo_never_exceeds_111_so_sd_s_codeword_exists() {
        for a_law in [false, true] {
            let law = if a_law { ucode::Law::A } else { ucode::Law::Mu };
            for max_power in 0..32u8 {
                let far = Info0d { max_power, a_law, ..Info0d::default() };
                let v92 = training_codeword_v92(&far);
                assert!((UINFO_LOWEST..=V92_MAX_UINFO).contains(&v92), "{max_power}, a-law {a_law}: {v92}");
                assert!(usize::from(16 + v92) < ucode::UCODES, "Sd's codeword 16 + {v92} does not exist");
                assert_eq!(v92, training_codeword(&far), "{max_power}, a-law {a_law}: V.90's own choice moved");
                assert!(ucode::linear(law, v92) <= power_limit(&far) as i32);
            }
            // And with every limit removed, which is the case the ceiling is
            // there for: V.90 goes to the loudest codeword there is, V.92
            // stops where Sd runs out.
            assert_eq!(loudest_codeword(law, i32::MAX, (ucode::UCODES - 1) as u8), 127);
            assert_eq!(loudest_codeword(law, i32::MAX, V92_MAX_UINFO), 111);
            // A limit under even the quietest allowed codeword still names a
            // sendable one.
            assert_eq!(loudest_codeword(law, 0, V92_MAX_UINFO), UINFO_LOWEST);
        }
        assert_eq!(V92_MAX_UINFO, 111);
    }

    /// One bit a data frame is one step, which is the arithmetic that makes a
    /// robbed bit cost exactly one rung.
    #[test]
    fn one_bit_a_frame_is_one_step_of_the_ladder() {
        for d in 21..42u32 {
            let step = rate_for(d + 1) - rate_for(d);
            assert!(
                step == 1333 || step == 1334,
                "{d} bits to {} was a step of {step}",
                d + 1
            );
        }
        // Six symbols a frame at 8000 a second.
        assert_eq!(SYMBOL_RATE / INTERVALS as u32, 1333);
        assert_eq!(RATE_STEP, (8000, 6));
    }
}
