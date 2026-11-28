//! Character formats: 8N1, 7E1 and the others a terminal program offers.
//!
//! The line under the terminal carries octets. On a start-stop line with ten
//! bits for each character, 8N1 and 7E1 are the same bits: the eighth data
//! bit of 8N1 is the parity bit of 7E1. So a seven-bit format is applied here,
//! on the octets, and it works the same over a modem, V.42 and Telnet:
//!
//! - Received octets lose their eighth bit before they are drawn.
//! - Sent octets get the parity of the format in their eighth bit.
//!
//! Formats with eight data bits and a parity bit (8E1, 8O1) need eleven bits
//! for each character, and an octet stream cannot carry them. They are not
//! here for that reason.

/// The parity of a seven-bit format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharFormat {
    /// Eight data bits, no parity. The default, and what every board expects.
    #[default]
    EightN1,
    /// Seven data bits. The eighth bit is zero when sent and ignored when
    /// received.
    SevenN1,
    /// Seven data bits and even parity.
    SevenE1,
    /// Seven data bits and odd parity.
    SevenO1,
    /// Seven data bits and a parity bit that is always one (mark).
    SevenM1,
    /// Seven data bits and a parity bit that is always zero (space).
    SevenS1,
}

impl CharFormat {
    /// All formats, in the order that a menu shows them.
    pub const ALL: [Self; 6] =
        [Self::EightN1, Self::SevenN1, Self::SevenE1, Self::SevenO1, Self::SevenM1, Self::SevenS1];

    /// The usual short name, such as "7E1".
    pub fn name(self) -> &'static str {
        match self {
            Self::EightN1 => "8N1",
            Self::SevenN1 => "7N1",
            Self::SevenE1 => "7E1",
            Self::SevenO1 => "7O1",
            Self::SevenM1 => "7M1",
            Self::SevenS1 => "7S1",
        }
    }

    /// The format with this short name, in upper or lower case.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Whether the eighth bit carries data.
    pub fn eight_bit(self) -> bool {
        self == Self::EightN1
    }

    /// A received octet, as the terminal is to read it.
    pub fn decode(self, byte: u8) -> u8 {
        if self.eight_bit() { byte } else { byte & 0x7f }
    }

    /// An octet to send, with the parity of this format in its eighth bit.
    pub fn encode(self, byte: u8) -> u8 {
        let low = byte & 0x7f;
        let odd = low.count_ones() % 2 == 1;
        let parity = match self {
            Self::EightN1 => return byte,
            Self::SevenN1 | Self::SevenS1 => false,
            Self::SevenM1 => true,
            // The parity bit makes the count of ones even, or odd.
            Self::SevenE1 => odd,
            Self::SevenO1 => !odd,
        };
        low | (u8::from(parity) << 7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_n1_changes_nothing() {
        for b in 0..=255u8 {
            assert_eq!(CharFormat::EightN1.encode(b), b);
            assert_eq!(CharFormat::EightN1.decode(b), b);
        }
    }

    #[test]
    fn parity_is_put_in_the_eighth_bit() {
        // 'A' is 0x41, two ones: even already.
        assert_eq!(CharFormat::SevenE1.encode(b'A'), 0x41);
        assert_eq!(CharFormat::SevenO1.encode(b'A'), 0xc1);
        // 'C' is 0x43, three ones.
        assert_eq!(CharFormat::SevenE1.encode(b'C'), 0xc3);
        assert_eq!(CharFormat::SevenO1.encode(b'C'), 0x43);
        assert_eq!(CharFormat::SevenM1.encode(b'A'), 0xc1);
        assert_eq!(CharFormat::SevenS1.encode(0xc1), 0x41);
        assert_eq!(CharFormat::SevenN1.encode(0xc1), 0x41);
    }

    #[test]
    fn every_seven_bit_format_reads_back_the_character() {
        for f in CharFormat::ALL {
            for b in 0..0x80u8 {
                assert_eq!(f.decode(f.encode(b)), b, "{}", f.name());
            }
        }
    }

    #[test]
    fn names_go_both_ways() {
        for f in CharFormat::ALL {
            assert_eq!(CharFormat::parse(f.name()), Some(f));
        }
        assert_eq!(CharFormat::parse("7e1"), Some(CharFormat::SevenE1));
        assert_eq!(CharFormat::parse("8E1"), None);
    }
}
