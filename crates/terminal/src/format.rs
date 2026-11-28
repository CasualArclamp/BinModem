//! Character formats (8N1, 7E1 and the others) and flow control.
//!
//! The line under the terminal carries octets. On a start-stop line with ten
//! bits for each character, 8N1 and 7E1 are the same bits: the eighth data
//! bit of 8N1 is the parity bit of 7E1. So a seven-bit format is applied here,
//! on the octets, and it works the same over a modem, V.42 and Telnet:
//!
//! - Received octets lose their eighth bit before they are drawn.
//! - Sent octets get the parity of the format in their eighth bit.
//!
//! The same is true of 7N2: its second stop bit is in the place of a parity
//! bit, and a stop bit is always mark. Formats that need eleven bits for each
//! character (8E1, 8N2, 7E2) cannot go in an octet, and
//! [`CharFormat::carried`] is false for them.

/// The parity bit of a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
    /// Always one.
    Mark,
    /// Always zero.
    Space,
}

impl Parity {
    pub const ALL: [Self; 5] = [Self::None, Self::Even, Self::Odd, Self::Mark, Self::Space];

    /// The name a settings dialog shows, such as "Even".
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Even => "Even",
            Self::Odd => "Odd",
            Self::Mark => "Mark",
            Self::Space => "Space",
        }
    }

    /// The letter in a short name such as "7E1".
    pub fn letter(self) -> char {
        match self {
            Self::None => 'N',
            Self::Even => 'E',
            Self::Odd => 'O',
            Self::Mark => 'M',
            Self::Space => 'S',
        }
    }
}

/// Data bits, parity and stop bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharFormat {
    /// 7 or 8.
    pub data_bits: u8,
    pub parity: Parity,
    /// 1 or 2.
    pub stop_bits: u8,
}

impl Default for CharFormat {
    /// 8N1, which every board expects.
    fn default() -> Self {
        Self { data_bits: 8, parity: Parity::None, stop_bits: 1 }
    }
}

impl CharFormat {
    /// The data bits a settings dialog offers.
    pub const DATA_BITS: [u8; 2] = [7, 8];
    /// The stop bits a settings dialog offers.
    pub const STOP_BITS: [u8; 2] = [1, 2];

    /// The usual short name, such as "7E1".
    pub fn name(self) -> String {
        format!("{}{}{}", self.data_bits, self.parity.letter(), self.stop_bits)
    }

    /// The format with this short name, in upper or lower case.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_uppercase();
        let mut chars = name.chars();
        let data_bits = chars.next()?.to_digit(10)? as u8;
        let letter = chars.next()?;
        let stop_bits = chars.next()?.to_digit(10)? as u8;
        if chars.next().is_some() {
            return None;
        }
        let parity = Parity::ALL.into_iter().find(|p| p.letter() == letter)?;
        let format = Self { data_bits, parity, stop_bits };
        (Self::DATA_BITS.contains(&data_bits) && Self::STOP_BITS.contains(&stop_bits) && format.carried())
            .then_some(format)
    }

    /// Whether a character in this format fits in one octet of the line.
    ///
    /// A start bit, the data, the parity bit if any, and the stop bits must
    /// come to ten, or nine for 7N1. The octet carries everything but the
    /// start bit and the first stop bit.
    pub fn carried(self) -> bool {
        let extra = u8::from(self.parity != Parity::None) + (self.stop_bits - 1);
        self.data_bits + extra <= 8
    }

    /// A received octet, as the terminal is to read it.
    pub fn decode(self, byte: u8) -> u8 {
        if self.data_bits == 8 { byte } else { byte & 0x7f }
    }

    /// An octet to send, with the parity bit, or the second stop bit, of this
    /// format in its eighth bit.
    pub fn encode(self, byte: u8) -> u8 {
        if self.data_bits == 8 {
            return byte;
        }
        let low = byte & 0x7f;
        let odd = low.count_ones() % 2 == 1;
        let high = match self.parity {
            // A second stop bit is mark, as a stop bit always is.
            Parity::None => self.stop_bits == 2,
            Parity::Mark => true,
            Parity::Space => false,
            // The parity bit makes the count of ones even, or odd.
            Parity::Even => odd,
            Parity::Odd => !odd,
        };
        low | (u8::from(high) << 7)
    }
}

/// Flow control from the far end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flow {
    #[default]
    None,
    /// XOFF (DC3, ^S) from the far end stops what is typed until XON (DC1,
    /// ^Q). Neither is drawn.
    XonXoff,
}

impl Flow {
    pub const ALL: [Self; 2] = [Self::None, Self::XonXoff];

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::XonXoff => "XON/XOFF",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// XON (DC1).
pub const XON: u8 = 0x11;
/// XOFF (DC3).
pub const XOFF: u8 = 0x13;

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> CharFormat {
        CharFormat::parse(name).unwrap()
    }

    #[test]
    fn eight_n1_changes_nothing() {
        let format = CharFormat::default();
        assert_eq!(format.name(), "8N1");
        for b in 0..=255u8 {
            assert_eq!(format.encode(b), b);
            assert_eq!(format.decode(b), b);
        }
    }

    #[test]
    fn parity_is_put_in_the_eighth_bit() {
        // 'A' is 0x41, two ones: even already.
        assert_eq!(f("7E1").encode(b'A'), 0x41);
        assert_eq!(f("7O1").encode(b'A'), 0xc1);
        // 'C' is 0x43, three ones.
        assert_eq!(f("7E1").encode(b'C'), 0xc3);
        assert_eq!(f("7O1").encode(b'C'), 0x43);
        assert_eq!(f("7M1").encode(b'A'), 0xc1);
        assert_eq!(f("7S1").encode(0xc1), 0x41);
        assert_eq!(f("7N1").encode(0xc1), 0x41);
        // The second stop bit is mark.
        assert_eq!(f("7N2").encode(b'A'), 0xc1);
    }

    #[test]
    fn every_seven_bit_format_reads_back_the_character() {
        for name in ["7N1", "7N2", "7E1", "7O1", "7M1", "7S1"] {
            let format = f(name);
            for b in 0..0x80u8 {
                assert_eq!(format.decode(format.encode(b)), b, "{name}");
            }
        }
    }

    #[test]
    fn eleven_bit_formats_are_not_carried() {
        for name in ["8E1", "8O1", "8M1", "8N2", "7E2", "7O2"] {
            assert_eq!(CharFormat::parse(name), None, "{name}");
        }
    }

    #[test]
    fn names_go_both_ways() {
        for name in ["8N1", "7N1", "7N2", "7E1", "7O1", "7M1", "7S1"] {
            assert_eq!(f(name).name(), name);
        }
        assert_eq!(f("7e1"), f("7E1"));
        assert_eq!(Flow::parse("xon/xoff"), Some(Flow::XonXoff));
    }
}
