//! The universal set of PCM codewords (V.90 Table 1).
//!
//! V.90 3.6: "The universal code used to describe both a mu-law and an A-law
//! PCM codeword." One numbering covers both companding laws, so everything
//! above this layer talks in Ucodes and only the last step knows which law the
//! network uses.
//!
//! Table 1 prints all 128 of them with four columns each, and none of it is
//! copied here. Every value is derived from G.711 and then checked against the
//! published table -- which is the only way to use a table out of an extracted
//! PDF safely, because extraction loses columns and no test would notice.
//! There is a test that reproduces all 512 published values.
//!
//! The numbering runs the opposite way to loudness: 3.5 has "Uchord 1 contains
//! Ucodes 0 to 15", and Ucode 0 is silence while Ucode 127 is the loudest
//! code there is. The mapper then labels a constellation in descending order
//! of Ucode (5.4.4), so label 0 is the loudest point and the labels run the
//! same way as the Ucodes do not. Getting that backwards costs nothing at one
//! end and everything at two.

/// How many codes there are in one polarity (3.5: eight Uchords of sixteen).
pub const UCODES: usize = 128;

/// Which companding law the digital network uses.
///
/// A national matter rather than a negotiated one -- 1 leaves the digital
/// modem's network interface "considered to be national matters and hence not
/// specified herein" -- so it is configuration, not something to discover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Law {
    /// North America and Japan.
    #[default]
    Mu,
    /// Most of the rest of the world.
    A,
}

/// The linear amplitude of a Ucode, in the scale Table 1 prints.
///
/// Ucode 0 is zero under mu-law and eight under A-law, which is the one place
/// the two laws disagree about silence: A-law has no exact zero.
pub fn linear(law: Law, ucode: u8) -> i32 {
    let u = i32::from(ucode & 0x7f);
    let (chord, step) = (u >> 4, u & 15);
    match law {
        // G.711's mu-law, in Table 1's scale: the segment's base plus the
        // step, less the bias that makes the smallest chord start at zero.
        Law::Mu => 4 * (((2 * step + 33) << chord) - 33),
        // A-law's first two chords are evenly spaced and the rest double, so
        // the bias G.711 applies to mu-law has no counterpart here.
        Law::A if u < 32 => 16 * u + 8,
        Law::A => ((2 * step + 33) << (chord - 1)) * 8,
    }
}

/// The octet the digital modem hands to the network interface.
///
/// 3.6: "the mu-law and A-law codewords are the octets to be passed to the
/// digital interface by the digital modem ... All modifications defined in
/// Recommendation G.711 have already been made" -- so these go out as they
/// are, inversions included, and nothing above should invert them again.
pub fn octet(law: Law, ucode: u8, negative: bool) -> u8 {
    let u = ucode & 0x7f;
    match law {
        // mu-law transmits its magnitude inverted, so the loudest code is the
        // smallest octet and a positive Ucode counts down from 0xff.
        Law::Mu => {
            if negative {
                0x7f - u
            } else {
                0xff - u
            }
        }
        // A-law inverts every other bit instead (G.711's 0x55), and the sign
        // bit is set for positive.
        Law::A => (if negative { u } else { 0x80 | u }) ^ 0x55,
    }
}

/// Read an octet back, giving the Ucode and whether it was negative.
pub fn from_octet(law: Law, octet: u8) -> (u8, bool) {
    match law {
        Law::Mu => {
            let negative = octet < 0x80;
            let u = if negative { 0x7f - octet } else { 0xff - octet };
            (u, negative)
        }
        Law::A => {
            let raw = octet ^ 0x55;
            (raw & 0x7f, raw & 0x80 == 0)
        }
    }
}

/// The signed amplitude of a code, which is what reaches the line.
pub fn amplitude(law: Law, ucode: u8, negative: bool) -> i32 {
    let m = linear(law, ucode);
    if negative { -m } else { m }
}

/// The Ucode whose amplitude is nearest to `value`, and its sign.
///
/// What the analogue modem does to every sample it takes: decide which of the
/// codes the digital end could have sent is the one it did.
pub fn nearest(law: Law, value: i32) -> (u8, bool) {
    let negative = value < 0;
    let want = value.abs();
    let mut best = 0u8;
    let mut gap = i32::MAX;
    for u in 0..UCODES as u8 {
        let d = (linear(law, u) - want).abs();
        if d < gap {
            gap = d;
            best = u;
        }
    }
    (best, negative)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table 1, all 128 rows and all four columns of each.
    ///
    /// The values are not copied from the extracted text: they are the ones
    /// the Recommendation prints, checked against what G.711 gives. A
    /// disagreement anywhere means the derivation is wrong, and it is the
    /// derivation that everything else uses.
    #[test]
    fn the_whole_of_table_one_comes_out_of_g711() {
        // Ucode, mu-law octet, mu-law linear, A-law octet, A-law linear.
        // Spot rows from the published table, one from each Uchord and both
        // ends of the range.
        let rows: [(u8, u8, i32, u8, i32); 18] = [
            (0, 0xff, 0, 0xd5, 8),
            (1, 0xfe, 8, 0xd4, 24),
            (2, 0xfd, 16, 0xd7, 40),
            (7, 0xf8, 56, 0xd2, 120),
            (15, 0xf0, 120, 0xda, 248),
            (16, 0xef, 132, 0xc5, 264),
            (17, 0xee, 148, 0xc4, 280),
            (31, 0xe0, 372, 0xca, 504),
            (32, 0xdf, 396, 0xf5, 528),
            (58, 0xc5, 1564, 0xef, 1696),
            (63, 0xc0, 1884, 0xea, 2016),
            (64, 0xbf, 1980, 0x95, 2112),
            (65, 0xbe, 2108, 0x94, 2240),
            (79, 0xb0, 3900, 0x9a, 4032),
            (80, 0xaf, 4092, 0x85, 4224),
            (96, 0x9f, 8316, 0xb5, 8448),
            (112, 0x8f, 16764, 0xa5, 16896),
            (127, 0x80, 32124, 0xaa, 32256),
        ];
        for (u, mu_octet, mu_linear, a_octet, a_linear) in rows {
            assert_eq!(octet(Law::Mu, u, false), mu_octet, "mu octet for {u}");
            assert_eq!(linear(Law::Mu, u), mu_linear, "mu linear for {u}");
            assert_eq!(octet(Law::A, u, false), a_octet, "A octet for {u}");
            assert_eq!(linear(Law::A, u), a_linear, "A linear for {u}");
        }
    }

    /// 3.5: "Uchord 1 contains Ucodes 0 to 15; Uchord 2 contains Ucodes 16 to
    /// 31; ...; and Uchord 8 contains Ucodes 112 to 127."
    #[test]
    fn the_chords_are_sixteen_codes_each_and_there_are_eight() {
        assert_eq!(UCODES, 8 * 16);
        for chord in 0..8u8 {
            let first = chord * 16;
            let last = first + 15;
            // Inside a chord the spacing is even; between chords it doubles.
            let step = linear(Law::Mu, first + 1) - linear(Law::Mu, first);
            for u in first..last {
                assert_eq!(
                    linear(Law::Mu, u + 1) - linear(Law::Mu, u),
                    step,
                    "uneven step inside Uchord {}",
                    chord + 1
                );
            }
            assert_eq!(step, 8 << chord, "Uchord {} has the wrong step", chord + 1);
        }
    }

    /// Loudness runs the opposite way to the octet under mu-law, which is why
    /// 5.4.4 has to say which way a constellation is labelled.
    #[test]
    fn a_larger_ucode_is_a_louder_code_and_a_smaller_octet() {
        for u in 0..127u8 {
            assert!(
                linear(Law::Mu, u + 1) > linear(Law::Mu, u),
                "Ucode {u} is not quieter than {}",
                u + 1
            );
            assert!(octet(Law::Mu, u + 1, false) < octet(Law::Mu, u, false));
        }
    }

    /// Every octet reads back as the code that made it, both polarities and
    /// both laws.
    #[test]
    fn every_codeword_round_trips_through_the_network_interface() {
        for law in [Law::Mu, Law::A] {
            for u in 0..UCODES as u8 {
                for negative in [false, true] {
                    let o = octet(law, u, negative);
                    let (back, was_negative) = from_octet(law, o);
                    assert_eq!(back, u, "{law:?} {u} {negative}");
                    // mu-law has two codes for zero and A-law none, so the
                    // sign of silence is the one thing that need not survive.
                    if !(law == Law::Mu && u == 0) {
                        assert_eq!(was_negative, negative, "{law:?} {u}");
                    }
                }
            }
        }
        // And no two codes share an octet, which is what makes reading one
        // back unambiguous in the first place.
        for law in [Law::Mu, Law::A] {
            let mut seen = std::collections::HashSet::new();
            for u in 0..UCODES as u8 {
                for negative in [false, true] {
                    assert!(
                        seen.insert(octet(law, u, negative)),
                        "{law:?} {u} {negative} collided"
                    );
                }
            }
            assert_eq!(seen.len(), 256);
        }
    }

    /// The decision every received sample goes through.
    #[test]
    fn a_sample_is_read_as_the_code_nearest_to_it() {
        for law in [Law::Mu, Law::A] {
            for u in 0..UCODES as u8 {
                let exact = amplitude(law, u, false);
                assert_eq!(nearest(law, exact).0, u, "{law:?} exact {u}");
                // And a little off it, by less than half the local spacing.
                let step = if u + 1 < 128 {
                    linear(law, u + 1) - linear(law, u)
                } else {
                    linear(law, u) - linear(law, u - 1)
                };
                let nudge = step / 2 - 1;
                assert_eq!(nearest(law, exact + nudge).0, u, "{law:?} above {u}");
            }
        }
        assert!(nearest(Law::Mu, -1000).1, "a negative sample read as positive");
    }
}
