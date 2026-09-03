//! LAPM address and control fields (V.42 8.2.1, 8.2.2, 8.2.4).
//!
//! LAPM is modulo 128 throughout, so I and S frames carry a two-octet control
//! field and sequence numbers occupy seven bits. Unlike LAPB there is no
//! modulo-8 variant to select between.

/// Sequence numbers are modulo 128 (V.42 Table 7).
pub const MODULUS: u8 = 128;

/// DTE-to-DTE data, the only DLCI this implementation carries (V.42 Table 10).
pub const DLCI_DATA: u8 = 0;
/// Control-function to control-function, listed as for further study.
pub const DLCI_CONTROL: u8 = 63;

/// Which end of the call this entity is.
///
/// It matters for addressing: the C/R bit encodes command versus response
/// differently at each end, so the same octet means opposite things depending
/// on who sent it (V.42 Table 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Originator,
    Answerer,
}

impl Role {
    pub fn peer(self) -> Self {
        match self {
            Self::Originator => Self::Answerer,
            Self::Answerer => Self::Originator,
        }
    }
}

/// Whether a frame is a command or a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Command,
    Response,
}

/// The address field (V.42 8.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Address {
    pub dlci: u8,
    pub kind: Kind,
}

impl Address {
    /// Encode as a single octet: DLCI in bits 8-3, C/R in bit 2, EA in bit 1.
    ///
    /// EA is set because this implementation never uses the optional second
    /// address octet (V.42 8.2.1.3).
    pub fn encode(&self, sender: Role) -> u8 {
        (self.dlci << 2) | (u8::from(self.cr_bit(sender)) << 1) | 1
    }

    /// The C/R bit for this frame, from the sender's point of view.
    ///
    /// V.42 Table 6: the originator marks its commands 0 and its responses 1;
    /// the answerer does the opposite.
    fn cr_bit(&self, sender: Role) -> bool {
        match (sender, self.kind) {
            (Role::Originator, Kind::Command) => false,
            (Role::Originator, Kind::Response) => true,
            (Role::Answerer, Kind::Command) => true,
            (Role::Answerer, Kind::Response) => false,
        }
    }

    /// Decode an address octet received by `receiver`.
    pub fn decode(octet: u8, receiver: Role) -> Result<Self, DecodeError> {
        if octet & 1 == 0 {
            // A clear EA bit promises a second address octet, which V.42 leaves
            // for further study and no real modem sends.
            return Err(DecodeError::ExtendedAddress);
        }
        let dlci = octet >> 2;
        let cr = octet & 0x02 != 0;
        // The sender is our peer, so invert the table.
        let kind = match (receiver.peer(), cr) {
            (Role::Originator, false) | (Role::Answerer, true) => Kind::Command,
            (Role::Originator, true) | (Role::Answerer, false) => Kind::Response,
        };
        Ok(Self { dlci, kind })
    }
}

/// Unnumbered frame encodings with the P/F bit cleared (V.42 Table 8).
mod code {
    pub const SABME: u8 = 0x6f;
    pub const DM: u8 = 0x0f;
    pub const UI: u8 = 0x03;
    pub const DISC: u8 = 0x43;
    pub const UA: u8 = 0x63;
    pub const FRMR: u8 = 0x87;
    pub const XID: u8 = 0xaf;
    pub const TEST: u8 = 0xe3;
    /// The poll/final bit sits in bit 5 of the unnumbered control octet.
    pub const PF: u8 = 0x10;
}

/// Supervisory function codes, the first control octet (V.42 Table 8).
mod supervisory {
    pub const RR: u8 = 0x01;
    pub const RNR: u8 = 0x05;
    pub const REJ: u8 = 0x09;
    pub const SREJ: u8 = 0x0d;
}

/// A LAPM frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Numbered information transfer (V.42 8.2.4.2).
    I { ns: u8, nr: u8, poll: bool, info: Vec<u8> },
    /// Receive ready.
    Rr { nr: u8, pf: bool },
    /// Receive not ready.
    Rnr { nr: u8, pf: bool },
    /// Reject: retransmit from N(R).
    Rej { nr: u8, pf: bool },
    /// Selective reject: retransmit only N(R). V.42 requires P/F to be 0 here.
    Srej { nr: u8 },
    /// Set asynchronous balanced mode extended (V.42 8.2.4.3).
    Sabme { poll: bool },
    /// Disconnect (V.42 8.2.4.4).
    Disc { poll: bool },
    /// Unnumbered acknowledgement.
    Ua { final_bit: bool },
    /// Disconnected mode.
    Dm { final_bit: bool },
    /// Frame reject, carrying the rejection cause.
    Frmr { final_bit: bool, info: Vec<u8> },
    /// Unnumbered information.
    Ui { pf: bool, info: Vec<u8> },
    /// Exchange identification, used for parameter negotiation (V.42 8.10).
    Xid { pf: bool, info: Vec<u8> },
    /// Loop-back test (V.42 8.11).
    Test { pf: bool, info: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Too few octets for an address and control field.
    Truncated,
    /// The address field promised a second octet.
    ExtendedAddress,
    /// A control field encoding not listed in V.42 Table 8 (see V.42 8.5.5).
    UndefinedControl(u8),
    /// A frame type that permits no information field carried one.
    UnexpectedInfo,
}

impl Frame {
    /// True for frames an entity sends as commands.
    pub fn kind(&self) -> Kind {
        match self {
            Self::I { .. } | Self::Sabme { .. } | Self::Disc { .. } => Kind::Command,
            Self::Ua { .. } | Self::Dm { .. } | Self::Frmr { .. } => Kind::Response,
            // RR, RNR, REJ, SREJ, UI, XID and TEST exist in both directions;
            // the sender decides, so default to command and let callers say.
            _ => Kind::Command,
        }
    }

    /// The poll or final bit.
    pub fn pf(&self) -> bool {
        match self {
            Self::I { poll, .. } | Self::Sabme { poll, .. } | Self::Disc { poll, .. } => *poll,
            Self::Rr { pf, .. }
            | Self::Rnr { pf, .. }
            | Self::Rej { pf, .. }
            | Self::Ui { pf, .. }
            | Self::Xid { pf, .. }
            | Self::Test { pf, .. } => *pf,
            Self::Ua { final_bit }
            | Self::Dm { final_bit }
            | Self::Frmr { final_bit, .. } => *final_bit,
            Self::Srej { .. } => false,
        }
    }

    /// Encode address and control fields plus any information field.
    ///
    /// The frame check sequence is added by the HDLC layer, not here.
    pub fn encode(&self, dlci: u8, sender: Role, kind: Kind) -> Vec<u8> {
        let address = Address { dlci, kind }.encode(sender);
        let mut out = vec![address];
        match self {
            Self::I { ns, nr, poll, info } => {
                out.push(ns << 1); // bit 1 clear marks an I frame
                out.push((nr << 1) | u8::from(*poll));
                out.extend_from_slice(info);
            }
            Self::Rr { nr, pf } => Self::push_s(&mut out, supervisory::RR, *nr, *pf),
            Self::Rnr { nr, pf } => Self::push_s(&mut out, supervisory::RNR, *nr, *pf),
            Self::Rej { nr, pf } => Self::push_s(&mut out, supervisory::REJ, *nr, *pf),
            // V.42 Table 8 fixes P/F at 0 for SREJ.
            Self::Srej { nr } => Self::push_s(&mut out, supervisory::SREJ, *nr, false),
            Self::Sabme { poll } => out.push(Self::u(code::SABME, *poll)),
            Self::Disc { poll } => out.push(Self::u(code::DISC, *poll)),
            Self::Ua { final_bit } => out.push(Self::u(code::UA, *final_bit)),
            Self::Dm { final_bit } => out.push(Self::u(code::DM, *final_bit)),
            Self::Frmr { final_bit, info } => {
                out.push(Self::u(code::FRMR, *final_bit));
                out.extend_from_slice(info);
            }
            Self::Ui { pf, info } => {
                out.push(Self::u(code::UI, *pf));
                out.extend_from_slice(info);
            }
            Self::Xid { pf, info } => {
                out.push(Self::u(code::XID, *pf));
                out.extend_from_slice(info);
            }
            Self::Test { pf, info } => {
                out.push(Self::u(code::TEST, *pf));
                out.extend_from_slice(info);
            }
        }
        out
    }

    fn push_s(out: &mut Vec<u8>, function: u8, nr: u8, pf: bool) {
        out.push(function);
        out.push((nr << 1) | u8::from(pf));
    }

    fn u(base: u8, pf: bool) -> u8 {
        if pf { base | code::PF } else { base }
    }

    /// Decode address, control and information from a frame body.
    ///
    /// The body is what the HDLC layer yields: everything between the flags
    /// with the frame check sequence already removed.
    pub fn decode(body: &[u8], receiver: Role) -> Result<(Address, Self), DecodeError> {
        if body.len() < 2 {
            return Err(DecodeError::Truncated);
        }
        let address = Address::decode(body[0], receiver)?;
        let control = body[1];

        // V.42 Table 7: bit 1 clear is I format, "01" is S format, "11" is U.
        if control & 0x01 == 0 {
            if body.len() < 3 {
                return Err(DecodeError::Truncated);
            }
            let frame = Self::I {
                ns: control >> 1,
                nr: body[2] >> 1,
                poll: body[2] & 1 != 0,
                info: body[3..].to_vec(),
            };
            return Ok((address, frame));
        }

        if control & 0x03 == 0x01 {
            if body.len() < 3 {
                return Err(DecodeError::Truncated);
            }
            let nr = body[2] >> 1;
            let pf = body[2] & 1 != 0;
            if body.len() > 3 {
                // Supervisory frames carry no information field.
                return Err(DecodeError::UnexpectedInfo);
            }
            let frame = match control {
                supervisory::RR => Self::Rr { nr, pf },
                supervisory::RNR => Self::Rnr { nr, pf },
                supervisory::REJ => Self::Rej { nr, pf },
                supervisory::SREJ => Self::Srej { nr },
                other => return Err(DecodeError::UndefinedControl(other)),
            };
            return Ok((address, frame));
        }

        let pf = control & code::PF != 0;
        let info = body[2..].to_vec();
        let has_info = !info.is_empty();
        let frame = match control & !code::PF {
            code::SABME if !has_info => Self::Sabme { poll: pf },
            code::DISC if !has_info => Self::Disc { poll: pf },
            code::UA if !has_info => Self::Ua { final_bit: pf },
            code::DM if !has_info => Self::Dm { final_bit: pf },
            code::SABME | code::DISC | code::UA | code::DM => {
                // V.42 8.2.4.3, 8.2.4.4 and 8.2.4.5 all forbid an information
                // field on these.
                return Err(DecodeError::UnexpectedInfo);
            }
            code::FRMR => Self::Frmr { final_bit: pf, info },
            code::UI => Self::Ui { pf, info },
            code::XID => Self::Xid { pf, info },
            code::TEST => Self::Test { pf, info },
            other => return Err(DecodeError::UndefinedControl(other)),
        };
        Ok((address, frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(frame: &Frame, sender: Role, kind: Kind) -> Vec<u8> {
        frame.encode(DLCI_DATA, sender, kind)
    }

    #[test]
    fn address_encodes_dlci_cr_and_ea() {
        // DLCI 0, command from the originator: C/R clear, EA set.
        let a = Address { dlci: DLCI_DATA, kind: Kind::Command };
        assert_eq!(a.encode(Role::Originator), 0x01);
        assert_eq!(a.encode(Role::Answerer), 0x03);
        let r = Address { dlci: DLCI_DATA, kind: Kind::Response };
        assert_eq!(r.encode(Role::Originator), 0x03);
        assert_eq!(r.encode(Role::Answerer), 0x01);
    }

    #[test]
    fn the_same_octet_means_opposite_things_at_each_end() {
        // V.42 Table 6 is why: 0x01 from an originator is a command, but the
        // identical octet from an answerer is a response.
        let from_originator = Address::decode(0x01, Role::Answerer).unwrap();
        assert_eq!(from_originator.kind, Kind::Command);
        let from_answerer = Address::decode(0x01, Role::Originator).unwrap();
        assert_eq!(from_answerer.kind, Kind::Response);
    }

    #[test]
    fn address_round_trips_for_both_roles() {
        for sender in [Role::Originator, Role::Answerer] {
            for kind in [Kind::Command, Kind::Response] {
                let a = Address { dlci: DLCI_DATA, kind };
                let octet = a.encode(sender);
                let back = Address::decode(octet, sender.peer()).unwrap();
                assert_eq!(back, a, "{sender:?} {kind:?}");
            }
        }
    }

    #[test]
    fn a_clear_extension_bit_is_rejected() {
        assert_eq!(
            Address::decode(0x00, Role::Originator),
            Err(DecodeError::ExtendedAddress)
        );
    }

    #[test]
    fn unnumbered_encodings_match_table_8() {
        let cases: [(Frame, u8); 6] = [
            (Frame::Sabme { poll: false }, 0x6f),
            (Frame::Disc { poll: false }, 0x43),
            (Frame::Ua { final_bit: false }, 0x63),
            (Frame::Dm { final_bit: false }, 0x0f),
            (Frame::Ui { pf: false, info: vec![] }, 0x03),
            (Frame::Xid { pf: false, info: vec![] }, 0xaf),
        ];
        for (frame, want) in cases {
            let bytes = encode(&frame, Role::Originator, Kind::Command);
            assert_eq!(bytes[1], want, "{frame:?}");
        }
        assert_eq!(
            encode(&Frame::Frmr { final_bit: false, info: vec![] }, Role::Originator, Kind::Response)[1],
            0x87
        );
        assert_eq!(
            encode(&Frame::Test { pf: false, info: vec![] }, Role::Originator, Kind::Command)[1],
            0xe3
        );
    }

    #[test]
    fn the_poll_bit_sits_in_bit_five() {
        assert_eq!(
            encode(&Frame::Sabme { poll: true }, Role::Originator, Kind::Command)[1],
            0x7f
        );
        assert_eq!(
            encode(&Frame::Ua { final_bit: true }, Role::Answerer, Kind::Response)[1],
            0x73
        );
    }

    #[test]
    fn supervisory_encodings_match_table_8() {
        for (frame, want) in [
            (Frame::Rr { nr: 0, pf: false }, 0x01),
            (Frame::Rnr { nr: 0, pf: false }, 0x05),
            (Frame::Rej { nr: 0, pf: false }, 0x09),
            (Frame::Srej { nr: 0 }, 0x0d),
        ] {
            let bytes = encode(&frame, Role::Originator, Kind::Command);
            assert_eq!(bytes[1], want, "{frame:?}");
            assert_eq!(bytes.len(), 3, "supervisory control field is two octets");
        }
    }

    #[test]
    fn sequence_numbers_occupy_seven_bits() {
        // Modulo 128, so N(R) 127 must survive intact rather than wrapping.
        let bytes = encode(&Frame::Rr { nr: 127, pf: true }, Role::Originator, Kind::Command);
        assert_eq!(bytes[2], 0xff);
        let (_, back) = Frame::decode(&bytes, Role::Answerer).unwrap();
        assert_eq!(back, Frame::Rr { nr: 127, pf: true });
    }

    #[test]
    fn information_frames_carry_both_sequence_numbers() {
        let frame = Frame::I { ns: 5, nr: 9, poll: false, info: b"payload".to_vec() };
        let bytes = encode(&frame, Role::Originator, Kind::Command);
        assert_eq!(bytes[1] & 1, 0, "bit 1 clear marks an I frame");
        assert_eq!(bytes[1] >> 1, 5);
        assert_eq!(bytes[2] >> 1, 9);
        let (_, back) = Frame::decode(&bytes, Role::Answerer).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn every_frame_type_round_trips() {
        let frames = vec![
            Frame::I { ns: 0, nr: 0, poll: false, info: vec![] },
            Frame::I { ns: 127, nr: 126, poll: true, info: b"data".to_vec() },
            Frame::Rr { nr: 3, pf: true },
            Frame::Rnr { nr: 4, pf: false },
            Frame::Rej { nr: 5, pf: true },
            Frame::Srej { nr: 6 },
            Frame::Sabme { poll: true },
            Frame::Disc { poll: true },
            Frame::Ua { final_bit: true },
            Frame::Dm { final_bit: false },
            Frame::Frmr { final_bit: true, info: vec![1, 2, 3, 4, 5] },
            Frame::Ui { pf: false, info: b"note".to_vec() },
            Frame::Xid { pf: true, info: b"params".to_vec() },
            Frame::Test { pf: false, info: b"loop".to_vec() },
        ];
        for frame in frames {
            let bytes = encode(&frame, Role::Originator, Kind::Command);
            let (_, back) = Frame::decode(&bytes, Role::Answerer).unwrap();
            assert_eq!(back, frame, "round trip failed");
        }
    }

    #[test]
    fn srej_never_sets_the_poll_bit() {
        // V.42 Table 8 fixes P/F at 0 for SREJ.
        let bytes = encode(&Frame::Srej { nr: 7 }, Role::Originator, Kind::Command);
        assert_eq!(bytes[2] & 1, 0);
    }

    #[test]
    fn frames_that_forbid_an_information_field_are_rejected() {
        // V.42 8.2.4.3: no information field is permitted with SABME.
        let bad = [0x01u8, 0x6f, 0xaa];
        assert_eq!(
            Frame::decode(&bad, Role::Answerer),
            Err(DecodeError::UnexpectedInfo)
        );
    }

    #[test]
    fn supervisory_frames_reject_an_information_field() {
        let bad = [0x01u8, 0x01, 0x00, 0xaa];
        assert_eq!(
            Frame::decode(&bad, Role::Answerer),
            Err(DecodeError::UnexpectedInfo)
        );
    }

    #[test]
    fn undefined_control_fields_are_reported() {
        // V.42 8.5.5 covers what to do with these; decoding must name them.
        let bad = [0x01u8, 0x1b];
        assert!(matches!(
            Frame::decode(&bad, Role::Answerer),
            Err(DecodeError::UndefinedControl(_))
        ));
    }

    #[test]
    fn truncated_frames_are_rejected() {
        assert_eq!(Frame::decode(&[], Role::Answerer), Err(DecodeError::Truncated));
        assert_eq!(Frame::decode(&[0x01], Role::Answerer), Err(DecodeError::Truncated));
        // An I frame needs a second control octet.
        assert_eq!(Frame::decode(&[0x01, 0x00], Role::Answerer), Err(DecodeError::Truncated));
    }
}
