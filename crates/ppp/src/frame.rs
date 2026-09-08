//! PPP's HDLC-like framing over an asynchronous link (RFC 1662).
//!
//! The layer between a modem that carries octets and a protocol that wants
//! packets. It marks where a frame begins and ends, keeps the marker out of
//! the data, and says whether what arrived is what was sent.
//!
//! There is a second escaping problem here that a synchronous link does not
//! have, and it is the reason for the Async-Control-Character-Map. Anything
//! between the two ends may treat a control character as a control character:
//! a modem watching for XON and XOFF, a terminal server watching for its own
//! escape. So each end tells the other which octets it cannot receive
//! literally, and they are sent escaped -- 4.2 in this Recommendation's terms,
//! and the same idea as V.42bis 9.2 solving the same problem for its own
//! escape character.

use ec::hdlc::Crc16;

/// 4.1: every frame begins and ends with this.
pub const FLAG: u8 = 0x7e;
/// 4.2: the octet that says the next one has been altered.
pub const ESCAPE: u8 = 0x7d;
/// 4.2: and what it was altered by. Bit 5, counting from zero.
pub const ESCAPE_XOR: u8 = 0x20;
/// 3.1: the All-Stations address, the only one PPP assigns.
pub const ADDRESS: u8 = 0xff;
/// 3.1: Unnumbered Information with the poll/final bit clear.
pub const CONTROL: u8 = 0x03;

/// Which octets have to be escaped, one bit each for 0x00 to 0x1f.
///
/// Sent as a Configure-Request option and negotiated per direction, so the two
/// ends need not agree: this is what *this* end must not see literally, and
/// the peer has its own.
///
/// Everything, until the peer says otherwise. RFC 1662 A: "The default value
/// is 0xffffffff", and asking for less before the peer has agreed to it is
/// asking a link that may be carrying XON to carry XON.
pub const DEFAULT_ACCM: u32 = 0xffff_ffff;

/// One frame's worth of protocol and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Which protocol the information field belongs to: 0xc021 for LCP,
    /// 0x0021 for IP, and so on.
    pub protocol: u16,
    pub payload: Vec<u8>,
}

/// Wraps packets for the line.
#[derive(Debug, Clone)]
pub struct Framer {
    accm: u32,
    /// Whether the peer has agreed to receive frames without the address and
    /// control octets, which are the same two octets on every frame.
    acfc: bool,
    /// The same for a protocol number small enough to fit in one octet.
    pfc: bool,
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framer {
    pub fn new() -> Self {
        Self { accm: DEFAULT_ACCM, acfc: false, pfc: false }
    }

    /// The map the peer asked this end to send with.
    pub fn set_accm(&mut self, accm: u32) {
        self.accm = accm;
    }

    /// Whether the peer has agreed to the two compressions of 3.2.
    pub fn set_compression(&mut self, address_control: bool, protocol: bool) {
        self.acfc = address_control;
        self.pfc = protocol;
    }

    /// True if this octet has to go out escaped.
    ///
    /// The flag and the escape always, whatever the map says: 4.2 makes those
    /// two the minimum any sender must escape, and a frame that put a literal
    /// flag in its data would end where it did not mean to.
    fn must_escape(&self, byte: u8) -> bool {
        byte == FLAG
            || byte == ESCAPE
            || (byte < 0x20 && self.accm & (1 << byte) != 0)
    }

    fn put(&self, byte: u8, out: &mut Vec<u8>) {
        if self.must_escape(byte) {
            out.push(ESCAPE);
            out.push(byte ^ ESCAPE_XOR);
        } else {
            out.push(byte);
        }
    }

    /// Put one packet on the line, flags and all.
    pub fn frame(&self, packet: &Packet, out: &mut Vec<u8>) {
        // The frame as the check sequence sees it: 3.1 computes the FCS over
        // the address, control, protocol and information, and over none of the
        // escaping, so it is built once here and escaped on the way out.
        let mut body = Vec::with_capacity(packet.payload.len() + 4);
        if !self.acfc {
            body.push(ADDRESS);
            body.push(CONTROL);
        }
        // 3.2: a protocol number whose upper octet is zero and whose lower
        // octet is odd may be sent as one octet. Both halves of that matter --
        // the odd bit is what lets a receiver tell a compressed protocol from
        // the first octet of an uncompressed one.
        if self.pfc && packet.protocol < 0x100 && packet.protocol & 1 == 1 {
            body.push(packet.protocol as u8);
        } else {
            body.push((packet.protocol >> 8) as u8);
            body.push(packet.protocol as u8);
        }
        body.extend_from_slice(&packet.payload);

        let mut fcs = Crc16::new();
        fcs.update_all(&body);

        out.push(FLAG);
        for &b in &body {
            self.put(b, out);
        }
        // 3.1: least significant octet first.
        for b in fcs.to_bytes() {
            self.put(b, out);
        }
        out.push(FLAG);
    }
}

/// Why a frame between two flags was thrown away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discarded {
    /// 4.3: fewer than four octets, which cannot hold a protocol and an FCS.
    TooShort,
    /// 4.3: an escape with nothing after it before the closing flag.
    EscapeAtEnd,
    /// The check sequence did not come out.
    BadFcs,
}

/// Finds packets in a stream of octets.
#[derive(Debug, Clone)]
pub struct Deframer {
    accm: u32,
    body: Vec<u8>,
    /// Between flags. Nothing before the first one is part of anything.
    open: bool,
    /// The last octet was an escape, so the next is altered.
    escaped: bool,
    /// Frames thrown away, and why, because a link that is not working looks
    /// exactly like a link with nothing on it from outside.
    pub short: u64,
    pub bad_fcs: u64,
    pub aborted: u64,
}

impl Default for Deframer {
    fn default() -> Self {
        Self::new()
    }
}

impl Deframer {
    pub fn new() -> Self {
        Self {
            accm: DEFAULT_ACCM,
            body: Vec::with_capacity(1600),
            open: false,
            escaped: false,
            short: 0,
            bad_fcs: 0,
            aborted: 0,
        }
    }

    /// The map this end asked the peer to send with.
    pub fn set_accm(&mut self, accm: u32) {
        self.accm = accm;
    }

    /// Offer one octet from the line. Gives back a packet at each closing flag
    /// that held one.
    pub fn feed(&mut self, byte: u8) -> Result<Option<Packet>, Discarded> {
        // 4.2: "On reception, prior to FCS computation, each octet with value
        // less than hexadecimal 0x20 is checked. If it is flagged in the
        // receiving ACCM, it is simply removed (it may have been inserted by
        // intervening data communications equipment)." Which is the whole
        // point of the map: something in the middle is allowed to have put it
        // there.
        if byte < 0x20 && byte != FLAG && self.accm & (1 << byte) != 0 {
            return Ok(None);
        }
        if byte == FLAG {
            // 4.3: an escape immediately before the closing flag aborts.
            let aborted = self.escaped;
            self.escaped = false;
            let body = std::mem::take(&mut self.body);
            self.body = Vec::with_capacity(1600);
            let was_open = self.open;
            self.open = true;
            if !was_open || body.is_empty() {
                // 4.1: "Two consecutive Flag Sequences constitute an empty
                // frame, which is silently discarded, and not counted as a FCS
                // error." And anything before the first flag was never a frame.
                return Ok(None);
            }
            if aborted {
                self.aborted += 1;
                return Err(Discarded::EscapeAtEnd);
            }
            return self.finish(body);
        }
        if !self.open {
            return Ok(None);
        }
        if self.escaped {
            self.escaped = false;
            self.body.push(byte ^ ESCAPE_XOR);
        } else if byte == ESCAPE {
            self.escaped = true;
        } else {
            self.body.push(byte);
        }
        Ok(None)
    }

    fn finish(&mut self, body: Vec<u8>) -> Result<Option<Packet>, Discarded> {
        // 4.3: "Frames which are too short (less than 4 octets when using the
        // 16-bit FCS)".
        if body.len() < 4 {
            self.short += 1;
            return Err(Discarded::TooShort);
        }
        let mut fcs = Crc16::new();
        fcs.update_all(&body);
        if fcs.residue() != Crc16::GOOD {
            self.bad_fcs += 1;
            return Err(Discarded::BadFcs);
        }
        let body = &body[..body.len() - 2];

        // 3.1 has every frame carrying the All-Stations address and UI
        // control, and 3.2 lets a peer that has negotiated it leave them off.
        // A receiver takes them if they are there whether or not it agreed to
        // anything, because it costs nothing to and the alternative is reading
        // 0xff 0x03 as a protocol number.
        let body = if body.len() >= 2 && body[0] == ADDRESS && body[1] == CONTROL {
            &body[2..]
        } else {
            body
        };
        if body.is_empty() {
            self.short += 1;
            return Err(Discarded::TooShort);
        }

        // 3.2: an odd first octet is a protocol number by itself. Every
        // protocol PPP assigns has an odd lower octet and an even upper one,
        // which is what makes the two cases tell themselves apart.
        let (protocol, rest) = if body[0] & 1 == 1 {
            (u16::from(body[0]), &body[1..])
        } else if body.len() >= 2 {
            (u16::from(body[0]) << 8 | u16::from(body[1]), &body[2..])
        } else {
            self.short += 1;
            return Err(Discarded::TooShort);
        };
        Ok(Some(Packet { protocol, payload: rest.to_vec() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(packet: &Packet, framer: &Framer, deframer: &mut Deframer) -> Packet {
        let mut wire = Vec::new();
        framer.frame(packet, &mut wire);
        let mut got = None;
        for b in wire {
            if let Ok(Some(p)) = deframer.feed(b) {
                assert!(got.is_none(), "one frame produced two packets");
                got = Some(p);
            }
        }
        got.expect("no packet came out")
    }

    #[test]
    fn a_packet_survives_the_wire() {
        let packet = Packet { protocol: 0xc021, payload: b"hello".to_vec() };
        let got = round_trip(&packet, &Framer::new(), &mut Deframer::new());
        assert_eq!(got, packet);
    }

    /// The examples 4.2 gives, exactly.
    #[test]
    fn the_escapes_are_the_ones_the_document_lists() {
        let framer = Framer::new();
        let mut wire = Vec::new();
        // Wrapped in a packet, so the payload is what is being looked at.
        framer.frame(
            &Packet { protocol: 0x0021, payload: vec![0x7e, 0x7d, 0x03, 0x11, 0x13] },
            &mut wire,
        );
        for pair in [
            [0x7d, 0x5e], // Flag Sequence
            [0x7d, 0x5d], // Control Escape
            [0x7d, 0x23], // ETX
            [0x7d, 0x31], // XON
            [0x7d, 0x33], // XOFF
        ] {
            assert!(
                wire.windows(2).any(|w| w == pair),
                "{:02x} was not sent as {:02x} {:02x}: {wire:02x?}",
                pair[1] ^ ESCAPE_XOR,
                pair[0],
                pair[1]
            );
        }
        // And no bare flag anywhere but the two ends.
        assert_eq!(wire.first(), Some(&FLAG));
        assert_eq!(wire.last(), Some(&FLAG));
        assert!(
            !wire[1..wire.len() - 1].contains(&FLAG),
            "a flag got into the middle of the frame"
        );
    }

    /// Every octet, so nothing is escaped by accident or missed.
    #[test]
    fn all_two_hundred_and_fifty_six_survive() {
        let payload: Vec<u8> = (0..=255).collect();
        let packet = Packet { protocol: 0x0021, payload };
        let got = round_trip(&packet, &Framer::new(), &mut Deframer::new());
        assert_eq!(got.payload.len(), 256);
        assert!(got.payload.iter().copied().eq(0..=255));
    }

    /// A map that asks for less still carries everything.
    ///
    /// The flag and the escape are not the map's to give away (4.2), and this
    /// is the setting a peer asks for when it knows the path is clean.
    #[test]
    fn an_empty_map_still_escapes_the_two_that_matter() {
        let mut framer = Framer::new();
        framer.set_accm(0);
        let mut deframer = Deframer::new();
        deframer.set_accm(0);
        let payload: Vec<u8> = (0..=255).collect();
        let packet = Packet { protocol: 0x0021, payload: payload.clone() };
        let got = round_trip(&packet, &framer, &mut deframer);
        assert_eq!(got.payload, payload);

        let mut wire = Vec::new();
        framer.frame(&packet, &mut wire);
        // Only the two, so the frame is barely longer than the packet.
        let escapes = wire.iter().filter(|&&b| b == ESCAPE).count();
        assert_eq!(escapes, 2, "an empty map escaped more than the flag and the escape");
    }

    /// What a modem watching for XON would otherwise eat.
    #[test]
    fn a_control_character_the_peer_cannot_receive_goes_escaped() {
        let mut framer = Framer::new();
        // Only XON and XOFF, which is the map a terminal server asks for.
        framer.set_accm((1 << 0x11) | (1 << 0x13));
        let mut wire = Vec::new();
        framer.frame(&Packet { protocol: 0x0021, payload: vec![0x11, 0x05] }, &mut wire);
        assert!(wire.windows(2).any(|w| w == [ESCAPE, 0x11 ^ ESCAPE_XOR]));
        assert!(wire.contains(&0x05), "0x05 was escaped and the map did not ask for it");
    }

    /// And what such a thing inserting one looks like from this end.
    #[test]
    fn a_control_character_the_line_inserted_is_dropped() {
        let framer = Framer::new();
        let mut deframer = Deframer::new();
        let packet = Packet { protocol: 0xc021, payload: b"abc".to_vec() };
        let mut wire = Vec::new();
        framer.frame(&packet, &mut wire);
        // Something in the middle helpfully sends flow control.
        let mut meddled = Vec::new();
        for (i, b) in wire.iter().enumerate() {
            if i == 4 {
                meddled.push(0x13);
            }
            meddled.push(*b);
        }
        let mut got = None;
        for b in meddled {
            if let Ok(Some(p)) = deframer.feed(b) {
                got = Some(p);
            }
        }
        assert_eq!(got.as_ref(), Some(&packet), "the inserted octet was not removed");
    }

    #[test]
    fn a_damaged_frame_is_refused_rather_than_delivered() {
        let framer = Framer::new();
        let mut deframer = Deframer::new();
        let mut wire = Vec::new();
        framer.frame(&Packet { protocol: 0xc021, payload: b"hello".to_vec() }, &mut wire);
        let middle = wire.len() / 2;
        wire[middle] ^= 0x40;
        let mut outcome = Ok(None);
        for b in wire {
            let r = deframer.feed(b);
            if r.is_err() || matches!(r, Ok(Some(_))) {
                outcome = r;
            }
        }
        assert_eq!(outcome, Err(Discarded::BadFcs));
        assert_eq!(deframer.bad_fcs, 1);
    }

    /// 4.1: two flags in a row are an empty frame and not an error.
    #[test]
    fn empty_frames_are_not_errors() {
        let mut deframer = Deframer::new();
        for _ in 0..8 {
            assert_eq!(deframer.feed(FLAG), Ok(None));
        }
        assert_eq!((deframer.short, deframer.bad_fcs, deframer.aborted), (0, 0, 0));
    }

    /// 4.3: an escape with the closing flag behind it.
    #[test]
    fn a_frame_that_ends_mid_escape_is_thrown_away() {
        let mut deframer = Deframer::new();
        let _ = deframer.feed(FLAG);
        for b in [ADDRESS, CONTROL, 0xc0, 0x21, 0x01, ESCAPE] {
            let _ = deframer.feed(b);
        }
        assert_eq!(deframer.feed(FLAG), Err(Discarded::EscapeAtEnd));
        assert_eq!(deframer.aborted, 1);
    }

    /// 3.2's two compressions, which a peer may leave off and this end must
    /// still read.
    #[test]
    fn a_frame_without_its_address_and_control_is_still_read() {
        let mut framer = Framer::new();
        framer.set_compression(true, true);
        let packet = Packet { protocol: 0x0021, payload: b"ip".to_vec() };
        let mut wire = Vec::new();
        framer.frame(&packet, &mut wire);
        // Flag, one protocol octet, two of payload, two of FCS, flag.
        assert_eq!(wire.len(), 7, "not compressed: {wire:02x?}");
        let got = round_trip(&packet, &framer, &mut Deframer::new());
        assert_eq!(got, packet);
    }

    /// The rubbish before the first flag is nobody's frame.
    #[test]
    fn what_arrives_before_the_first_flag_is_ignored() {
        let mut deframer = Deframer::new();
        for b in b"NO CARRIER\r\n" {
            assert_eq!(deframer.feed(*b), Ok(None));
        }
        let framer = Framer::new();
        let packet = Packet { protocol: 0xc021, payload: b"x".to_vec() };
        let mut wire = Vec::new();
        framer.frame(&packet, &mut wire);
        let mut got = None;
        for b in wire {
            if let Ok(Some(p)) = deframer.feed(b) {
                got = Some(p);
            }
        }
        assert_eq!(got, Some(packet));
        assert_eq!(deframer.bad_fcs, 0, "the noise was counted as a broken frame");
    }
}
