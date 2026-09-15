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

pub mod modulus;
pub mod sign;
pub mod ucode;

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
