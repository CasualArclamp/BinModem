//! Headers (7.3).
//!
//! Every frame begins with one. Whichever form it takes it carries the same
//! raw information: a type byte, and four bytes of flags or a file position.
//!
//! 7.3, Figure 1 gives the order those four go in, and it is the one thing
//! here that is easy to get backwards -- the same four bytes are read as flags
//! most-significant-first and as a position least-significant-first:
//!
//! ```text
//!     TYPE  F3 F2 F1 F0
//!     TYPE  P0 P1 P2 P3
//! ```

use super::crc::{Crc16, Crc32};
use super::escape;
use super::{Kind, ZBIN, ZBIN32, ZDLE, ZHEX, ZPAD};

/// Which of the three forms a header takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Binary with a 16-bit check (7.3.1).
    Binary16,
    /// Binary with a 32-bit check (7.3.2). Only when the receiver has said it
    /// can, with the FC32 capability bit.
    Binary32,
    /// Hex (7.3.3). What the receiver answers in, and what the sender uses
    /// where no data subpacket follows.
    Hex,
}

/// A frame header: what kind, and its four bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    /// The four bytes, in the order they go on the line: F3 F2 F1 F0, which is
    /// also P0 P1 P2 P3.
    pub data: [u8; 4],
}

/// Why a header could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    /// Not enough bytes yet; ask again when more have arrived.
    Incomplete,
    /// The check sequence did not hold.
    BadCrc,
    /// A type byte that is not a frame type.
    UnknownKind(u8),
    /// A format byte that is not ZBIN, ZBIN32 or ZHEX.
    UnknownStyle(u8),
    /// A hex digit that is not one, or an escape that decodes to nothing.
    Malformed,
}

impl Header {
    /// A header carrying flags, most significant first (Figure 1).
    pub fn flags(kind: Kind, f3: u8, f2: u8, f1: u8, f0: u8) -> Self {
        Self { kind, data: [f3, f2, f1, f0] }
    }

    /// A header carrying a file position, least significant first (Figure 1).
    pub fn position(kind: Kind, at: u32) -> Self {
        Self { kind, data: at.to_le_bytes() }
    }

    /// Read the four bytes as a file position.
    pub fn to_position(self) -> u32 {
        u32::from_le_bytes(self.data)
    }

    /// ZF0, the least significant flags byte, which is the one most frames use.
    pub fn zf0(self) -> u8 {
        self.data[3]
    }

    /// ZF1, the next one up.
    pub fn zf1(self) -> u8 {
        self.data[2]
    }

    /// Put the header on the line.
    pub fn encode(self, style: Style) -> Vec<u8> {
        let body =
            [self.kind.to_byte(), self.data[0], self.data[1], self.data[2], self.data[3]];
        match style {
            Style::Hex => self.encode_hex(&body),
            Style::Binary16 => {
                let mut crc = Crc16::new();
                crc.update_all(&body);
                Self::encode_binary(ZBIN, &body, &crc.to_bytes())
            }
            Style::Binary32 => {
                let mut crc = Crc32::new();
                crc.update_all(&body);
                Self::encode_binary(ZBIN32, &body, &crc.to_bytes())
            }
        }
    }

    /// Figures 2 and 3: `ZPAD ZDLE <format> TYPE F3 F2 F1 F0 CRC...`, with
    /// everything after the format byte escaped.
    fn encode_binary(format: u8, body: &[u8; 5], crc: &[u8]) -> Vec<u8> {
        let mut out = vec![ZPAD, ZDLE, format];
        out.extend(escape::encode(body, 0));
        // Carried on from the body, because the carriage return rule looks at
        // the byte before and a run boundary is not a reason to forget it.
        let previous = *body.last().unwrap_or(&0);
        out.extend(escape::encode(crc, previous));
        out
    }

    /// Figure 4: `ZPAD ZPAD ZDLE ZHEX TYPE F3 F2 F1 F0 CRC-1 CRC-2 CR LF XON`,
    /// with each of those sent as two lower-case hex digits.
    fn encode_hex(self, body: &[u8; 5]) -> Vec<u8> {
        let mut crc = Crc16::new();
        crc.update_all(body);
        let mut out = vec![ZPAD, ZPAD, ZDLE, ZHEX];
        for b in body.iter().chain(crc.to_bytes().iter()) {
            out.push(HEX[usize::from(b >> 4)]);
            out.push(HEX[usize::from(b & 0x0f)]);
        }
        // 7.3.3: "a carriage return and line feed are sent with HEX headers",
        // and then "an XON character is appended to all HEX packets except
        // ZACK and ZFIN" -- not after ZACK, because that would interfere with
        // flow control while streaming, and not after ZFIN, so that a session
        // can be closed cleanly.
        out.push(b'\r');
        out.push(b'\n');
        if !matches!(self.kind, Kind::Ack | Kind::Fin) {
            out.push(0o21);
        }
        out
    }
}

/// 7.3.3: "the type byte, the four position/flag bytes, and the 16 bit CRC
/// thereof are sent in hex using the character set 01234567890abcdef. Upper
/// case hex digits are not allowed; they false trigger XMODEM and YMODEM
/// programs."
const HEX: [u8; 16] = *b"0123456789abcdef";

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        // Read either case even though only one is written. 7.3.3 forbids
        // sending upper case; nothing says to refuse it on the way in, and a
        // far end that sends it is still telling us something.
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// One byte pulled off an escaped stream, and how far it advanced.
fn unescape_at(bytes: &[u8], at: usize) -> Result<(u8, usize), HeaderError> {
    let b = *bytes.get(at).ok_or(HeaderError::Incomplete)?;
    if b != ZDLE {
        return Ok((b, 1));
    }
    let next = *bytes.get(at + 1).ok_or(HeaderError::Incomplete)?;
    escape::decode(next).map(|d| (d, 2)).ok_or(HeaderError::Malformed)
}

/// Read a header from the start of `bytes`.
///
/// Returns what it read and how many bytes it consumed, so the caller can go
/// on to whatever follows -- which for three frame types is a data subpacket.
///
/// The caller is expected to have found the start already: 7.3.3 says "the
/// zgethdr routine synchronizes with the ZPAD-ZDLE sequence", and hunting for
/// that in a stream is [`find`]'s job rather than this one's.
pub fn decode(bytes: &[u8]) -> Result<(Header, Style, usize), HeaderError> {
    let mut at = 0;
    while bytes.get(at) == Some(&ZPAD) {
        at += 1;
    }
    if at == 0 {
        return Err(HeaderError::Malformed);
    }
    if bytes.get(at) != Some(&ZDLE) {
        return Err(if at >= bytes.len() {
            HeaderError::Incomplete
        } else {
            HeaderError::Malformed
        });
    }
    at += 1;
    let format = *bytes.get(at).ok_or(HeaderError::Incomplete)?;
    at += 1;
    match format {
        ZHEX => decode_hex(bytes, at),
        ZBIN => decode_binary(bytes, at, Style::Binary16, 2),
        ZBIN32 => decode_binary(bytes, at, Style::Binary32, 4),
        other => Err(HeaderError::UnknownStyle(other)),
    }
}

fn decode_binary(
    bytes: &[u8],
    mut at: usize,
    style: Style,
    crc_len: usize,
) -> Result<(Header, Style, usize), HeaderError> {
    let mut body = [0u8; 5];
    for slot in &mut body {
        let (b, used) = unescape_at(bytes, at)?;
        *slot = b;
        at += used;
    }
    let mut check = [0u8; 4];
    for slot in check.iter_mut().take(crc_len) {
        let (b, used) = unescape_at(bytes, at)?;
        *slot = b;
        at += used;
    }
    let ok = match style {
        Style::Binary32 => {
            let mut crc = Crc32::new();
            crc.update_all(&body);
            crc.to_bytes() == check
        }
        _ => {
            let mut crc = Crc16::new();
            crc.update_all(&body);
            crc.to_bytes() == check[..2]
        }
    };
    if !ok {
        return Err(HeaderError::BadCrc);
    }
    let kind = Kind::from_byte(body[0]).ok_or(HeaderError::UnknownKind(body[0]))?;
    Ok((Header { kind, data: [body[1], body[2], body[3], body[4]] }, style, at))
}

fn decode_hex(bytes: &[u8], mut at: usize) -> Result<(Header, Style, usize), HeaderError> {
    let mut raw = [0u8; 7];
    for slot in &mut raw {
        let hi = from_hex(*bytes.get(at).ok_or(HeaderError::Incomplete)?)
            .ok_or(HeaderError::Malformed)?;
        let lo = from_hex(*bytes.get(at + 1).ok_or(HeaderError::Incomplete)?)
            .ok_or(HeaderError::Malformed)?;
        *slot = (hi << 4) | lo;
        at += 2;
    }
    let mut crc = Crc16::new();
    crc.update_all(&raw[..5]);
    if crc.to_bytes() != raw[5..7] {
        return Err(HeaderError::BadCrc);
    }
    // 7.3.3: "the receive routine expects to see at least one of these
    // characters, two if the first is CR", and an XON may follow. Eaten here
    // so the caller is left at whatever really comes next.
    for trailer in [b'\r', b'\n', 0o21] {
        if bytes.get(at) == Some(&trailer) {
            at += 1;
        }
    }
    let kind = Kind::from_byte(raw[0]).ok_or(HeaderError::UnknownKind(raw[0]))?;
    Ok((Header { kind, data: [raw[1], raw[2], raw[3], raw[4]] }, Style::Hex, at))
}

/// Where a header starts in `bytes`, if one does.
///
/// 7.3.3: the receiver "synchronizes with the ZPAD-ZDLE sequence". A binary
/// header opens with one ZPAD and a hex header with two, so what to hunt for
/// is a ZPAD with a ZDLE one or two bytes after it.
pub fn find(bytes: &[u8]) -> Option<usize> {
    (0..bytes.len()).find(|&i| {
        if bytes[i] != ZPAD {
            return false;
        }
        let pads = bytes[i..].iter().take_while(|&&b| b == ZPAD).count();
        pads <= 2 && bytes.get(i + pads) == Some(&ZDLE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_bytes_read_both_ways_round() {
        // Figure 1, and the one thing here that is easy to get backwards: the
        // same four bytes are flags most significant first and a position
        // least significant first.
        let h = Header::position(Kind::Rpos, 0x0403_0201);
        assert_eq!(h.data, [0x01, 0x02, 0x03, 0x04], "P0 P1 P2 P3");
        assert_eq!(h.to_position(), 0x0403_0201);

        let h = Header::flags(Kind::Rinit, 4, 3, 2, 1);
        assert_eq!(h.data, [4, 3, 2, 1], "F3 F2 F1 F0");
        assert_eq!(h.zf0(), 1);
        assert_eq!(h.zf1(), 2);
    }

    #[test]
    fn a_hex_header_looks_like_figure_four() {
        // ZPAD ZPAD ZDLE ZHEX, then seven bytes as fourteen lower case hex
        // digits, then CR LF, then XON.
        let out = Header::position(Kind::Rpos, 0).encode(Style::Hex);
        assert_eq!(&out[..4], &[ZPAD, ZPAD, ZDLE, ZHEX]);
        assert_eq!(out.len(), 4 + 14 + 3);
        assert_eq!(&out[out.len() - 3..], b"\r\n\x11");
        let digits = &out[4..18];
        assert!(
            digits.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)),
            "upper case hex false triggers XMODEM programs"
        );
    }

    #[test]
    fn the_two_frames_that_must_not_carry_an_xon_do_not() {
        // 7.3.3: "XON is not sent after ZACK headers to protect flow control
        // in streaming situations. XON is not sent after a ZFIN header to
        // allow clean session cleanup."
        for kind in [Kind::Ack, Kind::Fin] {
            let out = Header::position(kind, 0).encode(Style::Hex);
            assert_eq!(&out[out.len() - 2..], b"\r\n", "{kind:?} got an XON");
        }
        let out = Header::position(Kind::Rpos, 0).encode(Style::Hex);
        assert_eq!(out[out.len() - 1], 0o21, "everything else gets one");
    }

    #[test]
    fn a_binary_header_looks_like_figures_two_and_three() {
        let out = Header::position(Kind::Data, 0).encode(Style::Binary16);
        assert_eq!(&out[..3], &[ZPAD, ZDLE, ZBIN]);
        let out = Header::position(Kind::Data, 0).encode(Style::Binary32);
        assert_eq!(&out[..3], &[ZPAD, ZDLE, ZBIN32]);
    }

    #[test]
    fn every_style_round_trips_every_frame_type() {
        for kind in (0u8..=19).filter_map(Kind::from_byte) {
            for style in [Style::Hex, Style::Binary16, Style::Binary32] {
                let sent = Header::flags(kind, 0x12, 0x34, 0x56, 0x78);
                let bytes = sent.encode(style);
                let (got, back, used) = decode(&bytes).expect("should decode");
                assert_eq!(got, sent, "{kind:?} in {style:?}");
                assert_eq!(back, style);
                assert_eq!(used, bytes.len(), "{kind:?} in {style:?} left bytes over");
            }
        }
    }

    #[test]
    fn a_position_that_needs_escaping_survives() {
        // A file offset is four arbitrary bytes and some of them are the flow
        // control characters 7.2 escapes. An offset of 0x13131313 is four
        // XOFFs, and a header that sent those raw would stop the line it was
        // travelling on.
        for at in [0x1313_1313u32, 0x1818_1818, 0x9111_1091, 0xFFFF_FFFF] {
            let sent = Header::position(Kind::Rpos, at);
            for style in [Style::Binary16, Style::Binary32] {
                let bytes = sent.encode(style);
                assert!(
                    !bytes[3..].contains(&0o23) && !bytes[3..].contains(&0o21),
                    "flow control went out raw for {at:#x} in {style:?}"
                );
                let (got, _, _) = decode(&bytes).expect("should decode");
                assert_eq!(got.to_position(), at);
            }
        }
    }

    #[test]
    fn a_corrupted_header_is_refused_rather_than_guessed_at() {
        let bytes = Header::position(Kind::Data, 4096).encode(Style::Binary32);
        for i in 3..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 0x01;
            if let Ok((got, _, _)) = decode(&bad) {
                assert_ne!(
                    got.to_position(),
                    4096,
                    "a flipped bit at {i} passed as the same header"
                );
            }
        }
    }

    #[test]
    fn a_header_cut_short_asks_for_more_rather_than_failing() {
        // The stream arrives a few bytes at a time, and a header split across
        // two reads is the ordinary case rather than an error.
        let bytes = Header::position(Kind::File, 0).encode(Style::Binary16);
        for n in 1..bytes.len() {
            assert_eq!(
                decode(&bytes[..n]),
                Err(HeaderError::Incomplete),
                "cut at {n} should have asked for more"
            );
        }
    }

    #[test]
    fn a_header_is_found_in_a_stream_of_other_things() {
        // What a receiver actually faces: a board saying something, and then a
        // header in the middle of it.
        let mut stream = b"Ready to send. Press Ctrl-X to abort.\r\n".to_vec();
        let at = stream.len();
        stream.extend(Header::position(Kind::Rqinit, 0).encode(Style::Hex));
        assert_eq!(find(&stream), Some(at));
        let (got, _, _) = decode(&stream[at..]).expect("should decode");
        assert_eq!(got.kind, Kind::Rqinit);
    }

    #[test]
    fn a_lone_asterisk_is_not_a_header() {
        // Boards are full of them. Three in a row is not one either: one pad
        // for a binary header, two for a hex one, and no more than that.
        assert_eq!(find(b"*** NEW FILES ***\r\n"), None);
        assert_eq!(find(b"* nothing here"), None);
        assert_eq!(find(b"nothing at all"), None);
    }
}
