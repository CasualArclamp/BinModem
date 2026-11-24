//! Binary data subpackets (7.4).
//!
//! "Binary data subpackets immediately follow the associated binary header
//! packet. A binary data packet contains 0 to 1024 bytes of data ... the data
//! bytes are ZDLE encoded and transmitted. A ZDLE and frameend are then sent,
//! followed by two or four ZDLE encoded CRC bytes. The CRC accumulates the
//! data bytes and the frameend."
//!
//! That last clause is the one to read twice: the terminator is part of what
//! is checked, so a line hit that turns one kind of ending into another does
//! not pass.

use super::crc::{Crc16, Crc32};
use super::escape;
use super::ZDLE;

/// How a subpacket ends, and what the sender expects to happen next.
///
/// 8.2 and 9.1 describe what each is for. The four values are given in the
/// document only by name; they are the four lower-case letters `h` to `k`, in
/// the order the document introduces them, and they are outside the escape
/// range of 7.2 precisely so that a data byte cannot be mistaken for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ending {
    /// End of frame, no response expected. 9.1: "an empty ZCRCE data subpacket
    /// is sent" to close a frame the sender is stepping away from.
    End = b'h',
    /// The frame goes on: "a data subpacket terminated by ZCRCG and CRC does
    /// not elicit a response" (8.2), which is what streaming is made of.
    Go = b'i',
    /// Expects a ZACK, and the sender carries on without waiting for it: 8.2,
    /// "ZCRCQ data subpackets expect a ZACK response with the ... subpacket
    /// continues immediately".
    Query = b'j',
    /// Expects a ZACK and waits for it. 8.2: "ZCRCW data subpackets expect a
    /// response before the next frame is sent." The one that turns streaming
    /// back into stop-and-wait, used where the receiver has said it cannot
    /// take data while writing to disk.
    Wait = b'k',
}

impl Ending {
    pub fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            b'h' => Self::End,
            b'i' => Self::Go,
            b'j' => Self::Query,
            b'k' => Self::Wait,
            _ => return None,
        })
    }

    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Whether the sender should stop and wait for a ZACK.
    pub fn waits(self) -> bool {
        self == Self::Wait
    }

    /// Whether the receiver owes a ZACK at all.
    pub fn acknowledged(self) -> bool {
        matches!(self, Self::Query | Self::Wait)
    }
}

/// The largest a subpacket may be (7.4).
///
/// "A binary data packet contains 0 to 1024 bytes of data."
pub const MAX_DATA: usize = 1024;

/// What to send at a given line rate (7.4).
///
/// "Recommended length values are 256 bytes below 2400 bps, 512 at 2400 bps,
/// and 1024 above 4800 bps or when the data link is known to be relatively
/// error free."
///
/// The reasoning is not stated but is not hard: a subpacket spoiled by a line
/// hit is a subpacket sent again, so the right size is a trade between the
/// per-packet overhead and how much is lost when one goes wrong.
pub fn recommended_length(bits_per_second: u32) -> usize {
    match bits_per_second {
        0..=2399 => 256,
        2400..=4800 => 512,
        _ => MAX_DATA,
    }
}

/// One decoded subpacket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subpacket {
    pub data: Vec<u8>,
    pub ending: Ending,
}

/// Why a subpacket could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubpacketError {
    /// Not enough bytes yet.
    Incomplete,
    /// The check sequence did not hold.
    BadCrc,
    /// A ZDLE followed by something that is neither an escape nor an ending.
    Malformed,
    /// More than 7.4 allows, which means the ending was missed and the
    /// stream is being read as data.
    TooLong,
}

/// Put a subpacket on the line.
///
/// `previous` is the last byte already sent -- the header's final byte, or the
/// end of the last subpacket -- because 7.2's carriage return rule looks one
/// byte back and does not care where a buffer began.
pub fn encode(data: &[u8], ending: Ending, thirty_two_bit: bool, previous: u8) -> Vec<u8> {
    let mut out = escape::encode(data, previous);
    out.push(ZDLE);
    out.push(ending.to_byte());

    // "The CRC accumulates the data bytes and the frameend" -- the terminator
    // is inside the check, so a hit that turns a ZCRCG into a ZCRCW cannot
    // pass for one.
    let last = data.last().copied().unwrap_or(previous);
    if thirty_two_bit {
        let mut crc = Crc32::new();
        crc.update_all(data);
        crc.update(ending.to_byte());
        out.extend(escape::encode(&crc.to_bytes(), last));
    } else {
        let mut crc = Crc16::new();
        crc.update_all(data);
        crc.update(ending.to_byte());
        out.extend(escape::encode(&crc.to_bytes(), last));
    }
    out
}

/// Read a subpacket from the start of `bytes`, returning it and how much was
/// consumed.
pub fn decode(bytes: &[u8], thirty_two_bit: bool) -> Result<(Subpacket, usize), SubpacketError> {
    let mut data = Vec::new();
    let mut at = 0;
    let ending = loop {
        let b = *bytes.get(at).ok_or(SubpacketError::Incomplete)?;
        at += 1;
        if b != ZDLE {
            data.push(b);
            if data.len() > MAX_DATA {
                return Err(SubpacketError::TooLong);
            }
            continue;
        }
        let next = *bytes.get(at).ok_or(SubpacketError::Incomplete)?;
        at += 1;
        if let Some(ending) = Ending::from_byte(next) {
            break ending;
        }
        let plain = escape::decode(next).ok_or(SubpacketError::Malformed)?;
        data.push(plain);
        if data.len() > MAX_DATA {
            return Err(SubpacketError::TooLong);
        }
    };

    let width = if thirty_two_bit { 4 } else { 2 };
    let mut check = [0u8; 4];
    for slot in check.iter_mut().take(width) {
        let b = *bytes.get(at).ok_or(SubpacketError::Incomplete)?;
        at += 1;
        *slot = if b == ZDLE {
            let next = *bytes.get(at).ok_or(SubpacketError::Incomplete)?;
            at += 1;
            escape::decode(next).ok_or(SubpacketError::Malformed)?
        } else {
            b
        };
    }

    let ok = if thirty_two_bit {
        let mut crc = Crc32::new();
        crc.update_all(&data);
        crc.update(ending.to_byte());
        crc.to_bytes() == check
    } else {
        let mut crc = Crc16::new();
        crc.update_all(&data);
        crc.update(ending.to_byte());
        crc.to_bytes() == check[..2]
    };
    if !ok {
        return Err(SubpacketError::BadCrc);
    }
    Ok((Subpacket { data, ending }, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endings_are_the_four_letters_outside_the_escape_range() {
        // 7.2 escapes to "a byte with bit 6 set and bit 5 reset", which is the
        // upper case letters. These four are lower case, which is bit 5 set --
        // the single bit that stops a data byte ending a subpacket.
        for ending in [Ending::End, Ending::Go, Ending::Query, Ending::Wait] {
            let b = ending.to_byte();
            assert!(b.is_ascii_lowercase(), "{b:#04x} is in the escape range");
            assert_eq!(Ending::from_byte(b), Some(ending));
        }
        assert_eq!(
            [Ending::End, Ending::Go, Ending::Query, Ending::Wait].map(Ending::to_byte),
            *b"hijk"
        );
    }

    #[test]
    fn only_two_endings_ask_for_anything_back() {
        // 8.2: ZCRCG "does not elicit a response"; ZCRCQ expects a ZACK and
        // carries on; ZCRCW expects one "before the next frame is sent".
        assert!(!Ending::End.acknowledged() && !Ending::Go.acknowledged());
        assert!(Ending::Query.acknowledged() && Ending::Wait.acknowledged());
        assert!(Ending::Wait.waits() && !Ending::Query.waits());
    }

    #[test]
    fn the_recommended_lengths_are_the_ones_in_the_document() {
        // 7.4: "256 bytes below 2400 bps, 512 at 2400 bps, and 1024 above
        // 4800 bps".
        assert_eq!(recommended_length(1200), 256);
        assert_eq!(recommended_length(2400), 512);
        assert_eq!(recommended_length(4800), 512);
        assert_eq!(recommended_length(9600), 1024);
        assert_eq!(recommended_length(33600), MAX_DATA);
    }

    #[test]
    fn a_subpacket_round_trips_at_both_check_widths() {
        let data = b"MAIN MENU\r\n[1] Messages\r\n".to_vec();
        for wide in [false, true] {
            for ending in [Ending::End, Ending::Go, Ending::Query, Ending::Wait] {
                let bytes = encode(&data, ending, wide, 0);
                let (got, used) = decode(&bytes, wide).expect("should decode");
                assert_eq!(got.data, data);
                assert_eq!(got.ending, ending);
                assert_eq!(used, bytes.len());
            }
        }
    }

    #[test]
    fn every_byte_value_survives_a_subpacket() {
        // What a binary file is: all 256 values, including the seven 7.2
        // escapes and the four endings.
        let data: Vec<u8> = (0u8..=255).collect();
        for wide in [false, true] {
            let bytes = encode(&data, Ending::Go, wide, 0);
            let (got, _) = decode(&bytes, wide).expect("should decode");
            assert_eq!(got.data, data);
        }
    }

    #[test]
    fn the_ending_is_inside_the_check() {
        // 7.4: "the CRC accumulates the data bytes and the frameend". Which
        // means a line hit that turns one ending into another is caught -- and
        // it matters, because the difference between ZCRCG and ZCRCW is
        // whether the sender waits, and a sender that stops waiting when it
        // should not have is a transfer that hangs.
        let data = b"whatever".to_vec();
        let mut bytes = encode(&data, Ending::Go, false, 0);
        let at = bytes.iter().rposition(|&b| b == Ending::Go.to_byte()).expect("an ending");
        bytes[at] = Ending::Wait.to_byte();
        assert_eq!(decode(&bytes, false), Err(SubpacketError::BadCrc));
    }

    #[test]
    fn a_corrupted_subpacket_is_refused() {
        let data = b"the quick brown fox".to_vec();
        for wide in [false, true] {
            let bytes = encode(&data, Ending::End, wide, 0);
            for i in 0..bytes.len() {
                let mut bad = bytes.clone();
                bad[i] ^= 0x01;
                if let Ok((got, _)) = decode(&bad, wide) {
                    assert_ne!(got.data, data, "a flipped bit at {i} passed as the same data");
                }
            }
        }
    }

    #[test]
    fn a_subpacket_cut_short_asks_for_more() {
        let bytes = encode(b"partial", Ending::Go, true, 0);
        for n in 0..bytes.len() {
            assert_eq!(
                decode(&bytes[..n], true),
                Err(SubpacketError::Incomplete),
                "cut at {n} should have asked for more"
            );
        }
    }

    #[test]
    fn a_stream_with_no_ending_in_it_is_given_up_on() {
        // A missed ending would otherwise read the rest of the connection as
        // one subpacket. 7.4 caps a subpacket at 1024 bytes, so anything past
        // that is not a subpacket that is still arriving.
        let runaway = vec![b'x'; MAX_DATA * 2];
        assert_eq!(decode(&runaway, false), Err(SubpacketError::TooLong));
    }

    #[test]
    fn an_empty_subpacket_is_a_subpacket() {
        // 9.1 sends one: "an empty ZCRCE data subpacket is sent" to close a
        // frame the sender is stepping away from.
        let bytes = encode(&[], Ending::End, false, 0);
        let (got, used) = decode(&bytes, false).expect("should decode");
        assert!(got.data.is_empty());
        assert_eq!(got.ending, Ending::End);
        assert_eq!(used, bytes.len());
    }
}
