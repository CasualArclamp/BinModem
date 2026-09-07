//! Link escape encoding (7.2).
//!
//! ZMODEM gets data transparency by extending the character set with escape
//! sequences built on ZDLE, rather than by counting bytes. 7.2 says why that
//! is worth the overhead: "link escape coding permits variable length data
//! subpackets without the overhead of a separate byte count. It allows the
//! beginning of frames to be detected without special timing techniques,
//! facilitating rapid error recovery."
//!
//! The worst case is a file made entirely of characters that need escaping,
//! which costs fifty per cent. Ordinary files cost almost nothing.

use super::ZDLE;

/// What a byte becomes when escaped: bit 6 inverted (7.2).
///
/// "The receiving program decodes any sequence of ZDLE followed by a byte with
/// bit 6 set and bit 5 reset (upper case letter, either parity) to the
/// equivalent control character by inverting bit 6."
const FLIP: u8 = 0o100;

/// ZDLE itself, escaped: `030 ^ 0100` is `X`. 7.2 calls this ZDLEE.
pub const ZDLEE: u8 = ZDLE ^ FLIP;

/// Escapes for rubout and its parity twin, which 7.2 says the receiver
/// recognises "should these characters need to be escaped".
///
/// Recognised here and never produced: nothing on this side needs them, and
/// sending an escape a far end might not expect is a worse bargain than the
/// two bytes it saves.
pub const ZRUB0: u8 = b'l';
pub const ZRUB1: u8 = b'm';

/// Whether a byte has to be escaped in a data subpacket (7.2).
///
/// "ZMODEM software escapes ZDLE, 020, 0220, 021, 0221, 023, and 0223" -- the
/// escape itself, and the four flow control characters with and without their
/// eighth bit, which is what makes the protocol survive a path doing XON/XOFF.
pub fn must_escape(byte: u8) -> bool {
    matches!(byte, ZDLE | 0o20 | 0o220 | 0o21 | 0o221 | 0o23 | 0o223)
}

/// Whether a byte has to be escaped because of what came before it (7.2).
///
/// "If preceded by 0100 or 0300 (@), 015 and 0215 are also escaped to protect
/// the Telenet command escape CR-@-CR." A carriage return after an at sign is
/// how a packet switch of the era was told to pay attention, and a file
/// containing that sequence would otherwise hang up on itself.
pub fn must_escape_after(byte: u8, previous: u8) -> bool {
    matches!(byte, 0o15 | 0o215) && matches!(previous & 0o177, 0o100)
}

/// Escape one byte into `out`, given what went before it.
pub fn push(out: &mut Vec<u8>, byte: u8, previous: u8) {
    if must_escape(byte) || must_escape_after(byte, previous) {
        out.push(ZDLE);
        out.push(byte ^ FLIP);
    } else {
        out.push(byte);
    }
}

/// Escape a run of bytes.
///
/// `previous` is the last byte sent before this run, because the carriage
/// return rule looks one byte back and a run boundary is not a reason to
/// forget what it saw.
pub fn encode(data: &[u8], previous: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 8);
    let mut last = previous;
    for &b in data {
        push(&mut out, b, last);
        last = b;
    }
    out
}

/// What an escaped byte decodes to, or `None` if it is not an escape ZMODEM
/// defines.
///
/// The escape range is "bit 6 set and bit 5 reset", which is `0100..0137` and
/// its parity twin -- the upper case letters. Anything else after a ZDLE is
/// either a frame end, a header introducer, or a line hit, and this is not the
/// place that tells those apart.
pub fn decode(escaped: u8) -> Option<u8> {
    match escaped {
        ZRUB0 => Some(0o177),
        ZRUB1 => Some(0o377),
        b if b & 0o100 != 0 && b & 0o40 == 0 => Some(b ^ FLIP),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_escape_of_the_escape_is_the_letter_x() {
        // 7.2: "if a ZDLE character appears in binary data, it is prefixed
        // with ZDLE, then sent as ZDLEE". Which falls out of inverting bit 6
        // of 030, and comes to 0130.
        assert_eq!(ZDLEE, b'X');
        assert_eq!(decode(ZDLEE), Some(ZDLE));
    }

    #[test]
    fn flow_control_characters_are_escaped_with_and_without_their_parity() {
        // 7.2 names seven: ZDLE, 020, 0220, 021, 0221, 023, 0223. The pairs
        // are XON and XOFF and DLE, each with the eighth bit set and clear,
        // because a path that strips or adds parity must not be able to turn
        // data into flow control.
        for b in [ZDLE, 0o20, 0o220, 0o21, 0o221, 0o23, 0o223] {
            assert!(must_escape(b), "{b:#o} should be escaped");
        }
        // And nothing else is, or the overhead would be the fifty per cent
        // worst case all the time.
        let escaped = (0u8..=255).filter(|b| must_escape(*b)).count();
        assert_eq!(escaped, 7);
    }

    #[test]
    fn a_carriage_return_after_an_at_sign_is_escaped_and_otherwise_is_not() {
        // The Telenet command escape CR-@-CR. Only in that context: escaping
        // every carriage return would cost a text file a byte a line.
        assert!(must_escape_after(0o15, b'@'));
        assert!(must_escape_after(0o215, b'@'));
        assert!(must_escape_after(0o15, 0o300));
        assert!(!must_escape_after(0o15, b'A'));
        assert!(!must_escape_after(b'A', b'@'));
    }

    /// Undo an escaped run, so a round trip can be checked.
    fn unescape(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut iter = bytes.iter().copied();
        while let Some(b) = iter.next() {
            if b == ZDLE {
                let next = iter.next().expect("an escape with nothing after it");
                out.push(decode(next).expect("an escape that decodes to nothing"));
            } else {
                out.push(b);
            }
        }
        out
    }

    #[test]
    fn every_byte_value_survives_the_round_trip() {
        let all: Vec<u8> = (0u8..=255).collect();
        assert_eq!(unescape(&encode(&all, 0)), all);
    }

    #[test]
    fn the_telenet_sequence_survives_it_too() {
        // The one case where what a byte becomes depends on the byte before
        // it, so the one case a round trip can get wrong in only one place.
        let data = b"before@\rafter@\r\n@\r".to_vec();
        let sent = encode(&data, 0);
        assert_eq!(unescape(&sent), data);
        assert!(sent.len() > data.len(), "nothing was escaped at all");
    }

    #[test]
    fn a_run_remembers_what_came_before_it() {
        // The rule looks one byte back, and a run boundary is not a reason to
        // forget. Split across two calls, the carriage return must still be
        // escaped by the at sign that ended the first.
        let whole = encode(b"@\r", 0);
        let split = encode(b"\r", b'@');
        assert_eq!(whole.len(), 3, "at sign, escape, carriage return");
        assert_eq!(split.len(), 2, "escape, carriage return");
        assert_eq!(&whole[1..], &split[..]);
    }

    #[test]
    fn ordinary_text_costs_nothing() {
        let text = b"MAIN MENU\r\n[1] Messages\r\n[2] Files\r\n";
        assert_eq!(encode(text, 0).len(), text.len());
    }

    #[test]
    fn the_worst_case_is_the_fifty_per_cent_the_document_names() {
        // 7.2: "the worst case, a file consisting entirely of escaped
        // characters, would incur a 50% overhead".
        let worst = [ZDLE; 64];
        assert_eq!(encode(&worst, 0).len(), worst.len() * 2);
    }

    #[test]
    fn rubout_escapes_are_read_but_not_written() {
        // 7.2 has the receiver recognise these; nothing here needs to send
        // them, and an escape the far end might not expect is a worse bargain
        // than the byte it saves.
        assert_eq!(decode(ZRUB0), Some(0o177));
        assert_eq!(decode(ZRUB1), Some(0o377));
        assert!(!must_escape(0o177) && !must_escape(0o377));
    }

    #[test]
    fn a_frame_end_is_not_an_escape() {
        // The subpacket terminators are lower case, which is bit 5 set -- the
        // one bit that keeps them out of the escape range. If they were in it,
        // a data byte could end a subpacket.
        for &end in b"hijk" {
            assert_eq!(decode(end), None, "{} decoded as an escape", end as char);
        }
    }
}
