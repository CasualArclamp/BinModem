//! String-extension lengths (V.44 6.6.2, Tables 3 and 4).
//!
//! The one code here that is not a fixed width. An extension of one character
//! costs a single bit, two to four cost three, five to twelve cost seven, and
//! only past that does it cost enough to notice -- which is the right shape,
//! because the whole point of the extension is that short ones are common.
//!
//! Each subfield is written low-order bit first, like every other code (6.6),
//! which is why Table 3 prints "01" in its subfield column and "10" in its
//! order-of-transmission column. Reading the two columns as disagreeing is the
//! easy mistake; they are the same bits written down twice.

use crate::bits::{BitReader, BitWriter};

/// The largest extension there is: Table 4's range stops at 253, and the note
/// says why -- "the maximum length of a string extension is (N7T - 2), because
/// the minimum length of a string is 2 characters".
pub const MOST: u16 = 253;

/// How wide the final subfield is for lengths of thirteen and up, which
/// Table 4 makes a function of the agreed maximum string length.
fn tail_bits(n7: u8) -> u32 {
    match n7 {
        0..=46 => 5,
        47..=78 => 6,
        79..=142 => 7,
        _ => 8,
    }
}

/// The largest extension `n7` can express, which is the smaller of Table 4's
/// range and the note's N7 - 2.
pub fn most_for(n7: u8) -> u16 {
    // Table 4's ranges start at thirteen, so a tail of w bits reaches
    // 13 + 2^w - 1: five bits gives the table's 13-44, eight gives 13-268,
    // which the note then cuts to 253.
    let by_table = 13 + (1u16 << tail_bits(n7)) - 1;
    by_table.min(u16::from(n7).saturating_sub(2)).min(MOST)
}

/// Write one extension length.
pub fn write(length: u16, n7: u8, w: &mut BitWriter, out: &mut Vec<u8>) {
    debug_assert!(length >= 1, "an extension of nothing is not transferred");
    match length {
        // Table 3: "1".
        1 => w.write(1, 1, out),
        // "0" then two bits holding the length less one, so 2, 3 and 4 are
        // "01", "10" and "11".
        2..=4 => {
            w.write(0, 1, out);
            w.write(length - 1, 2, out);
        }
        // "0" "00" "0" then three bits holding the length less five.
        5..=12 => {
            w.write(0, 1, out);
            w.write(0, 2, out);
            w.write(0, 1, out);
            w.write(length - 5, 3, out);
        }
        // Table 4: "0" "00" "1" then N = length - 13, in as many bits as the
        // agreed maximum string length calls for.
        _ => {
            w.write(0, 1, out);
            w.write(0, 2, out);
            w.write(1, 1, out);
            w.write(length - 13, tail_bits(n7), out);
        }
    }
}

/// Read one back, or nothing if the bits have not all arrived.
///
/// Reading is all-or-nothing: a length that is half here is left alone rather
/// than half consumed, so the caller can try again when more has arrived.
pub fn read(r: &mut BitReader, n7: u8) -> Option<u16> {
    let tail = tail_bits(n7);
    // The longest a length can be: "0" "00" "1" and the tail.
    if r.available() < 1 {
        return None;
    }
    let mut peek = r.clone();
    let first = peek.read(1)?;
    if first == 1 {
        *r = peek;
        return Some(1);
    }
    let two = peek.read(2)?;
    if two != 0 {
        *r = peek;
        return Some(two + 1);
    }
    let which = peek.read(1)?;
    let value = peek.read(if which == 0 { 3 } else { tail })?;
    *r = peek;
    Some(value + if which == 0 { 5 } else { 13 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect the bits a length writes, as a string of '0' and '1' in the
    /// order they go on the line, which is the column Table 3 prints.
    fn transmitted(length: u16, n7: u8) -> String {
        let mut w = BitWriter::new();
        let mut out = Vec::new();
        write(length, n7, &mut w, &mut out);
        let pending = w.pending();
        w.align(&mut out);
        let total = out.len() * 8 - if pending == 0 { 0 } else { 8 - pending as usize };
        let mut bits = String::new();
        for i in 0..total {
            let octet = out[i / 8];
            bits.push(if octet >> (i % 8) & 1 == 1 { '1' } else { '0' });
        }
        bits
    }

    /// Table 3, every row of it, in the order of transmission the table gives.
    #[test]
    fn lengths_one_to_twelve_are_the_bits_table_three_prints() {
        let rows: [(u16, &str); 12] = [
            (1, "1"),
            (2, "010"),
            (3, "001"),
            (4, "011"),
            (5, "0000000"),
            (6, "0000100"),
            (7, "0000010"),
            (8, "0000110"),
            (9, "0000001"),
            (10, "0000101"),
            (11, "0000011"),
            (12, "0000111"),
        ];
        for (length, expected) in rows {
            assert_eq!(transmitted(length, 255), expected, "length {length}");
        }
    }

    /// Table 4: past twelve the last subfield widens with the agreed maximum
    /// string length, and carries N = length - 13.
    #[test]
    fn longer_extensions_carry_the_length_less_thirteen() {
        // 32 <= N7T <= 46: five bits, so lengths 13 to 44.
        assert_eq!(transmitted(13, 46), "0001".to_owned() + "00000");
        assert_eq!(transmitted(44, 46), "0001".to_owned() + "11111");
        // 142 < N7T <= 255: eight bits, so up to 253.
        assert_eq!(transmitted(13, 255), "0001".to_owned() + "00000000");
        // 253 - 13 is 240, which is 11110000 and goes out low-order first.
        assert_eq!(transmitted(253, 255), "0001".to_owned() + "00001111");
        // And the width really does follow the parameter.
        assert_eq!(transmitted(13, 78).len(), 4 + 6);
        assert_eq!(transmitted(13, 142).len(), 4 + 7);
    }

    /// Every length the parameter allows, written and read back.
    #[test]
    fn every_length_round_trips_at_every_width() {
        for n7 in [32u8, 46, 47, 78, 79, 142, 143, 255] {
            let top = most_for(n7);
            for length in 1..=top {
                let mut w = BitWriter::new();
                let mut out = Vec::new();
                write(length, n7, &mut w, &mut out);
                w.align(&mut out);
                let mut r = BitReader::new();
                for b in out {
                    r.push_octet(b);
                }
                assert_eq!(read(&mut r, n7), Some(length), "n7 {n7} length {length}");
            }
        }
    }

    /// The note under Table 4: an extension cannot take a string past N7,
    /// and a string is at least two characters to begin with.
    #[test]
    fn the_longest_extension_is_two_short_of_the_longest_string() {
        assert_eq!(most_for(255), 253);
        assert_eq!(most_for(32), 30);
        // Table 4's first row: "32 <= N7T <= 46" gives "13-44".
        assert_eq!(most_for(46), 44);
        // Past that the tail widens to six bits and reaches 76, so N7 - 2 is
        // what limits it again.
        assert_eq!(most_for(47), 45);
    }

    /// A length split across arrivals is left alone until all of it is here,
    /// rather than half consumed.
    #[test]
    fn a_length_that_has_not_all_arrived_is_not_taken() {
        let mut w = BitWriter::new();
        let mut out = Vec::new();
        write(200, 255, &mut w, &mut out);
        w.align(&mut out);
        assert_eq!(out.len(), 2, "twelve bits is two octets");

        let mut r = BitReader::new();
        r.push_octet(out[0]);
        assert_eq!(read(&mut r, 255), None, "it read a length that was half here");
        r.push_octet(out[1]);
        assert_eq!(read(&mut r, 255), Some(200));
    }
}
