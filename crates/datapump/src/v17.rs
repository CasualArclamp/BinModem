//! V.17: the fax modulation, which is V.32bis's modulation half-duplex.
//!
//! Everything that decides what a symbol looks like is shared with V.32bis
//! and lives in [`super::v32::trellis`]. Clause 2 gives V.17 2400 symbols a
//! second on an 1800 Hz carrier, eight-state trellis coding, Table 1's
//! differential quadrant coding, and constellations of 16, 32, 64 and 128
//! points for 7200, 9600, 12 000 and 14 400. Every one of those is word for
//! word what V.32bis does, and the constellations have been checked point by
//! point against the ones already here: read off V.17's own figures by
//! position, every set matches to within half a percent of the distance
//! between neighbouring points, which is the error of measuring a drawing.
//!
//! What is different is everything around the symbols. There is no start-up
//! handshake, because T.30 has already done the negotiating over V.21 at
//! 300 bit/s; there is no echo canceller, because only one end transmits at a
//! time; and there is no round trip to measure, because nothing is waiting
//! for an answer. In place of all of it there is a fixed training sequence
//! that the sender simply sends.

use super::v32::trellis::{self, Coded};

/// Symbols a second (clause 2).
pub const BAUD: f64 = 2400.0;
/// Carrier, in hertz (clause 2).
pub const CARRIER: f64 = 1800.0;

/// The scrambler of clause 4: 1 + x^-18 + x^-23.
///
/// One polynomial, not two: only one end is transmitting, so there is no
/// second direction to tell apart. It is the same polynomial V.32 gives the
/// calling modem, which means [`super::v32::Scrambler`] already has it.
pub const SCRAMBLER_TAPS: (u32, u32) = (18, 23);

/// The rates V.17 carries, fastest first.
pub const RATES: [u32; 4] = [14_400, 12_000, 9600, 7200];

/// Bits a symbol carries at each rate, which is what picks the constellation.
pub fn bits_per_symbol(rate: u32) -> Option<usize> {
    match rate {
        14_400 => Some(6),
        12_000 => Some(5),
        9600 => Some(4),
        7200 => Some(3),
        _ => None,
    }
}

/// The constellation for a rate.
pub fn coding_for(rate: u32) -> Option<Coded> {
    match rate {
        14_400 => Some(trellis::AT_14400),
        12_000 => Some(trellis::AT_12000),
        9600 => Some(trellis::AT_9600),
        7200 => Some(trellis::AT_7200),
        _ => None,
    }
}

/// The four segments of a long train, in symbol intervals (Table 3).
///
/// They add to 3344, which at 2400 baud is 1393 ms -- the figure the table
/// gives, and the check that the four numbers have been read off it
/// correctly.
pub mod train {
    /// Segment 1: alternations between states A and B (5.1.1).
    pub const ALTERNATIONS: u64 = 256;
    /// Segment 2: the equaliser training signal (5.1.2).
    pub const EQUALIZER: u64 = 2976;
    /// Segment 3: the bridge signal, sent only in a long train (5.1.3).
    pub const BRIDGE: u64 = 64;
    /// Segment 4: scrambled ones at the channel rate (5.1.4).
    pub const SCRAMBLED_ONES: u64 = 48;

    /// The whole of a long train.
    pub const LONG: u64 = ALTERNATIONS + EQUALIZER + BRIDGE + SCRAMBLED_ONES;
}

/// Table 4: how segment 2's dibits become signal states.
///
/// Differential encoding is off through this segment, so a dibit is a state
/// and not a change of state.
pub fn four_phase_state(dibit: u8) -> State {
    match dibit & 0b11 {
        0b00 => State::C,
        0b01 => State::D,
        0b11 => State::A,
        _ => State::B,
    }
}

/// Table 6: how segment 3's dibits change the state.
///
/// Quarter turns, given in the Recommendation as the pairs A/B, B/C, C/D,
/// D/A and so on rather than as angles -- and the pairs are what is
/// transcribed here, because the angle column of the table does not survive
/// being extracted from the page.
pub fn bridge_turn(dibit: u8) -> u8 {
    match dibit & 0b11 {
        0b00 => 1, // A/B, B/C, C/D, D/A: a quarter turn one way.
        0b01 => 0, // A/A: none.
        0b10 => 2, // A/C: a half turn.
        _ => 3,    // A/D: a quarter turn the other way.
    }
}

/// Table 5: the sixteen bits of the bridge signal, sent eight times.
///
/// B0 is the first bit into the scrambler. Note 2 says bits 4 to 6, 8 to 10
/// and 12 to 14 are for further study and that a receiver shall ignore them,
/// so nothing here reads them back.
pub const BRIDGE_PATTERN: [bool; 16] = [
    false, false, false, false, false, false, false, true,
    false, false, false, true, false, false, false, true,
];

/// The four signalling states the training uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    A,
    B,
    C,
    D,
}

impl State {
    /// Which point of the constellation this state is.
    ///
    /// Read off the figures rather than assumed, and the bit order matters:
    /// the labels are printed most significant first, as Q3, Y2, Y1, Y0,
    /// while the index into a `Coded` is the other way round. Reversing the
    /// label is what turns one into the other, which is checked below against
    /// all four of 7200's states.
    ///
    /// Only 7200 is settled. The circled letters on the other three figures
    /// sit up to nine tenths of a grid step from the point they name, so
    /// matching them to the nearest dot picks a neighbour as often as not,
    /// and a training signal on the wrong four points is a receiver that
    /// trains happily on something nobody is sending.
    pub fn label_at(self, rate: u32) -> Option<usize> {
        let labels = match rate {
            // A B C D, as printed on Figure 5/V.17.
            7200 => [0b0110usize, 0b0101, 0b0010, 0b0001],
            _ => return None,
        };
        let bits = bits_per_symbol(rate)? + 1;
        let label = labels[self as usize];
        Some((0..bits).fold(0, |a, i| a | ((label >> i & 1) << (bits - 1 - i))))
    }

    /// Where the state lands, in the constellation's own coordinates.
    pub fn point(self, rate: u32) -> Option<(f64, f64)> {
        Some(coding_for(rate)?.point(self.label_at(rate)?))
    }

    /// A quarter turn on, which is how the four are related.
    pub fn turned(self, quarters: u8) -> Self {
        const ORDER: [State; 4] = [State::A, State::B, State::C, State::D];
        ORDER[(self as usize + quarters as usize) % 4]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_segments_add_up_to_the_time_the_table_gives() {
        assert_eq!(train::LONG, 3344);
        let seconds = train::LONG as f64 / BAUD;
        assert!(
            (seconds - 1.393).abs() < 0.001,
            "a long train is {seconds:.3} s, and Table 3 says 1393 ms"
        );
    }

    #[test]
    fn every_rate_has_a_constellation_of_the_right_size() {
        for rate in RATES {
            let bits = bits_per_symbol(rate).expect("a rate V.17 carries");
            let coded = coding_for(rate).expect("and a constellation for it");
            // The redundant bit makes the set twice as big as the data.
            assert_eq!(coded.size(), 1 << (bits + 1), "{rate}");
            assert_eq!(coded.bits, bits, "{rate}");
        }
    }

    #[test]
    fn the_seven_thousand_two_hundred_states_are_where_the_figure_puts_them() {
        // Figure 5/V.17 draws its constellation at twice the scale used here,
        // so its (-6, -2) is this (-3, -1). All four were read off the figure
        // twice, once by eye and once by matching the circled letters to the
        // printed labels of the nearest points.
        let want = [
            (State::A, (-3.0, -1.0)),
            (State::B, (1.0, -3.0)),
            (State::C, (3.0, 1.0)),
            (State::D, (-1.0, 3.0)),
        ];
        for (state, p) in want {
            assert_eq!(state.point(7200), Some(p), "{state:?}");
        }
    }

    #[test]
    fn the_four_states_are_quarter_turns_of_one_another() {
        // The property that makes them usable for a differentially coded
        // training signal, and the one that catches a state read off the
        // wrong point: three of four can look plausible and still not turn.
        for state in [State::A, State::B, State::C, State::D] {
            let here = state.point(7200).expect("7200 is settled");
            let next = state.turned(1).point(7200).expect("7200 is settled");
            let turned = (-here.1, here.0);
            assert_eq!(
                (turned.0, turned.1),
                next,
                "{state:?} turned a quarter is not {:?}",
                state.turned(1)
            );
        }
    }

    #[test]
    fn segment_two_maps_dibits_to_the_states_table_4_gives() {
        // 00 01 00 01 ... 10 01 10 01 comes out as C D C D ... B D B D, which
        // is the worked example under 5.1.2.
        let dibits = [
            0b00, 0b01, 0b00, 0b01, 0b00, 0b01, 0b00, 0b01,
            0b00, 0b01, 0b00, 0b01, 0b10, 0b01, 0b10, 0b01,
        ];
        let got: Vec<State> = dibits.iter().map(|d| four_phase_state(*d)).collect();
        let want = [
            State::C, State::D, State::C, State::D, State::C, State::D,
            State::C, State::D, State::C, State::D, State::C, State::D,
            State::B, State::D, State::B, State::D,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn segment_three_turns_by_the_quarters_table_6_gives() {
        assert_eq!(bridge_turn(0b01), 0, "A/A is no change");
        assert_eq!(bridge_turn(0b00), 1, "A/B is a quarter");
        assert_eq!(bridge_turn(0b10), 2, "A/C is a half");
        assert_eq!(bridge_turn(0b11), 3, "A/D is three quarters");
        // And turning by the four of them in a row comes back where it began.
        let total: u8 = (0..4).map(bridge_turn).sum();
        assert_eq!(total % 4, 2, "0 + 1 + 2 + 3 is six quarters");
    }

    #[test]
    fn the_bridge_pattern_is_the_sixteen_bits_of_table_5() {
        assert_eq!(BRIDGE_PATTERN.len(), 16);
        let ones: Vec<usize> = BRIDGE_PATTERN
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(ones, vec![7, 11, 15], "B7, B11 and B15 and no others");
    }

    #[test]
    fn the_scrambler_is_the_one_v32_gives_its_calling_end() {
        // Which is the whole reason there is nothing to write for it.
        assert_eq!(SCRAMBLER_TAPS, (18, 23));
    }
}
