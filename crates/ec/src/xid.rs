//! XID parameter negotiation (V.42 8.10 and 12.2, V.42bis Annex A).
//!
//! The information field of an XID frame carries a format identifier followed
//! by subfields, each a group identifier, a two-octet group length, and a run
//! of parameter identifier / length / value triples. The user data subfield,
//! last when it is there, is the exception: it has no group length and runs to
//! the end of the field (V.42 12.2.1.3).
//!
//! This is how N401, the window size, the frame check sequence width and the
//! V.42bis parameters are actually agreed. Without it both ends simply run on
//! defaults, which works but leaves compression switched off.

use crate::frame::Kind;
use crate::{v42bis, v44};

/// The ISO "general purpose" format identifier (V.42 12.2.2).
pub const FI_GENERAL_PURPOSE: u8 = 0b1000_0010;

/// Group identifier of the parameter negotiation subfield.
pub const GI_PARAMETER: u8 = 0b1000_0000;
/// Group identifier of the private parameter negotiation subfield.
pub const GI_PRIVATE: u8 = 0b1111_0000;
/// Group identifier of the user data subfield, where V.44 lives.
///
/// V.44 7.3: "Parameters within the user data subfield, in addition to those
/// defined in ITU-T V.42, shall be used for this purpose. The user data
/// subfield shall appear in the XID frame immediately before the FCS." A
/// different subfield from V.42bis's, which is what lets one XID offer both
/// and let the far end pick.
///
/// Unlike the other two it carries no group length. V.42 12.2.1.3: "This
/// subfield, which follows all data link layer subfields as Figure 11 shows,
/// does not contain a GL. The subsequent information is bounded by the frame's
/// FCS field." V.44 Table A.1 has the same, the parameter set identifier
/// straight after the group identifier.
pub const GI_USER_DATA: u8 = 0b1111_1111;

/// Parameter identifiers within the parameter negotiation subfield (Table 11a).
mod pi {
    pub const HDLC_OPTIONAL: u8 = 3;
    pub const N401_TRANSMIT: u8 = 5;
    pub const N401_RECEIVE: u8 = 6;
    pub const WINDOW_TRANSMIT: u8 = 7;
    pub const WINDOW_RECEIVE: u8 = 8;
}

/// Parameter identifiers within the private subfield (Table 11b).
mod private_pi {
    pub const PARAMETER_SET: u8 = 0;
    pub const COMPRESSION_REQUEST: u8 = 1;
    pub const CODEWORDS: u8 = 2;
    pub const MAX_STRING: u8 = 3;
}

/// Parameter identifiers within the user data subfield (V.44 Table A.1).
mod user_pi {
    pub const PARAMETER_SET: u8 = 0x40;
    /// C0, the capability byte.
    pub const CAPABILITY: u8 = 0x41;
    /// P0, which directions are wanted.
    pub const REQUEST: u8 = 0x42;
    /// P1T and P1R, the number of codewords each way.
    pub const CODEWORDS_TRANSMIT: u8 = 0x43;
    pub const CODEWORDS_RECEIVE: u8 = 0x44;
    /// P2T and P2R, the maximum string length each way.
    pub const MAX_STRING_TRANSMIT: u8 = 0x45;
    pub const MAX_STRING_RECEIVE: u8 = 0x46;
    /// P3T and P3R, the length of history each way.
    pub const HISTORY_TRANSMIT: u8 = 0x47;
    pub const HISTORY_RECEIVE: u8 = 0x48;
}

/// Identifier marking the user data subfield as V.44, ASCII "V44".
pub const PARAMETER_SET_V44: [u8; 3] = *b"V44";

/// C0 with every optional bit clear: no packet methods, and parameters
/// negotiated here in the XID rather than after the link is up.
///
/// Table A.1: "00 Neither packet method nor multi-packet method supported: for
/// modem connections only", and "0 Parameter negotiation using XID exchange
/// and parameters below". Both are what a modem wants, and the second is the
/// default 7.4 names.
const CAPABILITY_MODEM: u8 = 0b0000_0000;

/// Identifier marking the private subfield as V.42bis, ASCII "V42".
///
/// V.42 12.2.2 Note 2 gives the first octet as `00101010`, which is `*` and
/// spells nothing alongside the `4` and `2` that follow. V.42bis Annex A
/// Table A-1 gives `01010110`, which is `V`. The latter is plainly right and is
/// what implementations use, so the V.42 text appears to be in error.
pub const PARAMETER_SET_V42: [u8; 3] = *b"V42";

/// N401 when nobody proposes one (V.42 9.2.3), in octets as the field carries.
const N401_DEFAULT: u16 = crate::lapm::DEFAULT_N401 as u16;
/// The window size when nobody proposes one (V.42 9.2.4).
const K_DEFAULT: u8 = crate::lapm::DEFAULT_K;

/// Bits of the HDLC optional functions mask that V.42 12.2.2 Note 1 names.
///
/// Bit 1 is the low-order bit of the first octet and is transmitted first.
mod hdlc_bit {
    /// Single-frame selective retransmission.
    ///
    /// V.42 12.2.2 Note 1 writes this one as "3A" where its three companions
    /// are plain numbers, which is ISO/IEC 8885's own sub-lettering for the
    /// variants of an option rather than anything about the mask: the four
    /// entries in the note are bit positions in it, and this is position 3.
    /// The reading is confirmed by what surrounds it -- the note requires a
    /// transmitter to set positions 2, 4, 8, 9, 12 and 16 whatever it
    /// supports, and 3 is the one gap in that run.
    pub const SREJ_SINGLE: u32 = 3;
    pub const TEST_FRAME: u32 = 14;
    pub const FCS32: u32 = 17;
    pub const SREJ_MULTIPLE: u32 = 24;
    /// Bit positions the encoding rules require a transmitter to set, whatever
    /// it actually supports. Receivers are told to ignore them.
    pub const REQUIRED: [u32; 6] = [2, 4, 8, 9, 12, 16];
    /// The one of them a response clears when it agrees to 32 bits.
    pub const CLEARED_BY_FCS32: u32 = 16;
}

/// Which directions V.42bis compression is requested for (V.42 Table 11b, P0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    Neither,
    InitiatorToResponder,
    ResponderToInitiator,
    Both,
}

impl Compression {
    fn from_bits(v: u8) -> Self {
        match v & 0b11 {
            1 => Self::InitiatorToResponder,
            2 => Self::ResponderToInitiator,
            3 => Self::Both,
            _ => Self::Neither,
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::Neither => 0,
            Self::InitiatorToResponder => 1,
            Self::ResponderToInitiator => 2,
            Self::Both => 3,
        }
    }

    /// The same directions seen from the other end of the line.
    ///
    /// V.42bis names its directions absolutely -- initiator to responder --
    /// and needs no such thing. V.44's P0 is relative to whoever sent it
    /// (Table 10: "01 only in transmit direction"), and 7.4 spells out what
    /// that means for an answer: "the complementary response to one entity's
    /// P0 value of 01 ... is a P0 value of 10". So a V.44 proposal has to be
    /// turned round before it can be compared with this end's.
    pub fn flipped(self) -> Self {
        match self {
            Self::InitiatorToResponder => Self::ResponderToInitiator,
            Self::ResponderToInitiator => Self::InitiatorToResponder,
            other => other,
        }
    }

    /// The directions both ends agree on.
    pub fn intersect(self, other: Self) -> Self {
        let bits = self.to_bits() & other.to_bits();
        Self::from_bits(bits)
    }
}

/// What an XID frame proposes or reports.
///
/// Absent values mean the parameter was not mentioned, which V.42 12.2.2 Note 3
/// says leaves any previously negotiated value unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Xid {
    /// N401 in octets for the transmit direction (encoded in bits on the wire).
    pub n401_transmit: Option<u16>,
    pub n401_receive: Option<u16>,
    pub window_transmit: Option<u8>,
    pub window_receive: Option<u8>,
    /// A 32-bit frame check sequence is requested.
    pub fcs32: bool,
    /// The loop-back TEST frame procedure is supported.
    /// Selective retransmission, one frame at a time (V.42 8.4.5.1).
    pub srej_single: bool,
    pub test_frame: bool,
    /// Selective retransmission with a span list is supported.
    pub srej_multiple: bool,
    /// V.42bis compression, present only when the private subfield appears.
    pub compression: Option<Compression>,
    /// P1, total codewords.
    pub codewords: Option<u16>,
    /// P2, maximum string length.
    pub max_string: Option<u8>,
    /// V.44, present only when the user data subfield names it.
    pub v44: Option<V44Offer>,
}

/// What one end proposes for V.44 (Table A.1).
///
/// The two directions are proposed separately and settled against each other
/// crosswise: 7.4 has "the proposed P2T from one entity ... compared with the
/// proposed P2R from the other entity", because one end's transmitting is the
/// other end's receiving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V44Offer {
    /// P0: which directions this end wants compressed.
    pub compression: Compression,
    /// P1T, P2T, P3T.
    pub transmit: v44::Params,
    /// P1R, P2R, P3R.
    pub receive: v44::Params,
}

impl V44Offer {
    /// Everything this implementation will do.
    pub fn proposal(compression: Compression) -> Self {
        let params = v44::Params::of(v44::OFFERED_N2, v44::OFFERED_N7);
        Self { compression, transmit: params, receive: params }
    }

    /// Settle this end's proposal against the far end's.
    ///
    /// 7.4: "if both values are valid, the lesser value shall be used ... The
    /// final agreed value is set into N7T by the entity proposing P2T and into
    /// N7R by the entity proposing P2R." So this end's transmit settings are
    /// its own proposal against what the far end said it could receive.
    pub fn resolve(self, other: Self) -> Option<Self> {
        let compression = self.compression.intersect(other.compression.flipped());
        if compression == Compression::Neither {
            return None;
        }
        let transmit = self.transmit.resolve(other.receive);
        let receive = self.receive.resolve(other.transmit);
        // 7.4: "any attempt to specify a value less than the minimum is a
        // procedural error". Refusing V.44 is kinder than disconnecting, and
        // leaves V.42bis or nothing.
        if !transmit.valid() || !receive.valid() {
            return None;
        }
        Some(Self { compression, transmit, receive })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XidError {
    /// The information field ended mid-structure.
    Truncated,
    /// A format identifier other than the general purpose one.
    UnknownFormat(u8),
    /// A parameter carried a length its type does not allow.
    BadLength { pi: u8, len: u8 },
}

impl Xid {
    /// Everything this implementation supports, as an opening proposal.
    pub fn proposal(compression: Compression) -> Self {
        Self {
            n401_transmit: Some(crate::lapm::DEFAULT_N401 as u16),
            n401_receive: Some(crate::lapm::DEFAULT_N401 as u16),
            window_transmit: Some(crate::lapm::DEFAULT_K),
            window_receive: Some(crate::lapm::DEFAULT_K),
            // Offered, because the alternative is worse than it looks. A
            // 16-bit check sequence lets about one damaged frame in 65536
            // through undetected, and on a line damaging hundreds a minute
            // that is a byte the terminal reads wrongly and nothing anywhere
            // notices. What runs is the intersection, so a far end without it
            // simply keeps 16.
            fcs32: true,
            // Offered. Go-back-N asks for the lost frame and everything
            // sent after it, which on a full window is fifteen frames to
            // recover one; this asks for the one. What runs is the
            // intersection, so a far end without it loses nothing.
            srej_single: true,
            test_frame: true,
            srej_multiple: false,
            compression: Some(compression),
            codewords: Some(v42bis::OFFERED_N2),
            max_string: Some(v42bis::OFFERED_N7),
            // Both are offered in the one XID. 7.3: "the responder shall
            // include parameters for at most one compression algorithm
            // (V.42 bis or V.44) in the response XID", so the far end picks
            // and a far end that has never heard of V.44 skips the subfield it
            // does not know and answers about V.42bis.
            v44: Some(V44Offer::proposal(compression)),
        }
    }

    /// Encode as the information field of an XID command or response.
    ///
    /// Which one matters to a single bit of the HDLC optional functions mask,
    /// and nowhere else.
    pub fn encode(&self, kind: Kind) -> Vec<u8> {
        let mut out = vec![FI_GENERAL_PURPOSE];

        // Parameter negotiation subfield.
        let mut params = Vec::new();
        push_param(&mut params, pi::HDLC_OPTIONAL, &self.hdlc_mask(kind).to_le_bytes());
        // V.42 12.2.2 Note 3: N401 is in octets, but negotiated in bits.
        if let Some(n) = self.n401_transmit {
            push_param(&mut params, pi::N401_TRANSMIT, &(n * 8).to_be_bytes());
        }
        if let Some(n) = self.n401_receive {
            push_param(&mut params, pi::N401_RECEIVE, &(n * 8).to_be_bytes());
        }
        if let Some(k) = self.window_transmit {
            push_param(&mut params, pi::WINDOW_TRANSMIT, &[k]);
        }
        if let Some(k) = self.window_receive {
            push_param(&mut params, pi::WINDOW_RECEIVE, &[k]);
        }
        push_subfield(&mut out, GI_PARAMETER, &params);

        // Private parameter negotiation subfield, carrying V.42bis.
        if let Some(compression) = self.compression {
            let mut private = Vec::new();
            // Note 2: the parameter set identifier always comes first.
            push_param(&mut private, private_pi::PARAMETER_SET, &PARAMETER_SET_V42);
            push_param(&mut private, private_pi::COMPRESSION_REQUEST, &[compression.to_bits()]);
            if let Some(n2) = self.codewords {
                push_param(&mut private, private_pi::CODEWORDS, &n2.to_be_bytes());
            }
            if let Some(n7) = self.max_string {
                push_param(&mut private, private_pi::MAX_STRING, &[n7]);
            }
            push_subfield(&mut out, GI_PRIVATE, &private);
        }

        // User data subfield, carrying V.44. 7.3 puts it "immediately before
        // the FCS", which is to say last.
        if let Some(v44) = self.v44 {
            let mut user = Vec::new();
            push_param(&mut user, user_pi::PARAMETER_SET, &PARAMETER_SET_V44);
            push_param(&mut user, user_pi::CAPABILITY, &[CAPABILITY_MODEM]);
            push_param(&mut user, user_pi::REQUEST, &[v44.compression.to_bits()]);
            push_param(&mut user, user_pi::CODEWORDS_TRANSMIT, &v44.transmit.n2.to_be_bytes());
            push_param(&mut user, user_pi::CODEWORDS_RECEIVE, &v44.receive.n2.to_be_bytes());
            push_param(&mut user, user_pi::MAX_STRING_TRANSMIT, &[v44.transmit.n7]);
            push_param(&mut user, user_pi::MAX_STRING_RECEIVE, &[v44.receive.n7]);
            push_param(&mut user, user_pi::HISTORY_TRANSMIT, &v44.transmit.n8.to_be_bytes());
            push_param(&mut user, user_pi::HISTORY_RECEIVE, &v44.receive.n8.to_be_bytes());
            // No group length. With one, a far end reading this as V.42
            // describes saw a parameter 0x00 of 33 octets, which is to say
            // nothing at all, and never learnt that V.44 was on offer.
            out.push(GI_USER_DATA);
            out.extend_from_slice(&user);
        }
        out
    }

    /// The 32-bit HDLC optional functions mask (V.42 Table 11a, Note 1).
    ///
    /// "The transmitter of an XID command frame shall set bit positions 2, 4,
    /// 8, 9, 12 and 16 to 1. The transmitter of an XID response frame shall
    /// also set these bit positions to 1, except bit position 16 shall be set
    /// to 0 if bit position 17 is set to 1." So bit 16 depends on which of the
    /// two this is: a command asking for 32 bits still sets it, and only an
    /// answer agreeing to them clears it. This end cleared it in its commands
    /// too, and the far ends it called on real lines never answered them.
    fn hdlc_mask(&self, kind: Kind) -> u32 {
        let mut mask = 0u32;
        for bit in hdlc_bit::REQUIRED {
            mask |= 1 << (bit - 1);
        }
        if self.srej_single {
            mask |= 1 << (hdlc_bit::SREJ_SINGLE - 1);
        }
        if self.test_frame {
            mask |= 1 << (hdlc_bit::TEST_FRAME - 1);
        }
        if self.fcs32 {
            mask |= 1 << (hdlc_bit::FCS32 - 1);
            if kind == Kind::Response {
                mask &= !(1 << (hdlc_bit::CLEARED_BY_FCS32 - 1));
            }
        }
        if self.srej_multiple {
            mask |= 1 << (hdlc_bit::SREJ_MULTIPLE - 1);
        }
        mask
    }

    /// Decode an XID information field.
    ///
    /// V.42 12.2.2: fields that are not recognized are ignored, so unknown
    /// groups and parameters are skipped rather than rejected.
    pub fn decode(body: &[u8]) -> Result<Self, XidError> {
        let mut xid = Self::default();
        let mut cursor = body.iter().copied();
        let fi = cursor.next().ok_or(XidError::Truncated)?;
        if fi != FI_GENERAL_PURPOSE {
            return Err(XidError::UnknownFormat(fi));
        }
        let mut pos = 1usize;

        while pos < body.len() {
            let gi = body[pos];
            if gi == GI_USER_DATA {
                // 12.2.1.3: no group length, and everything to the end of the
                // field is its own. Read as a group, "40 03" is a length of
                // 16387, and the XID of any far end offering V.44 was thrown
                // away whole.
                xid.read_user_data(&body[pos + 1..])?;
                break;
            }
            if pos + 3 > body.len() {
                return Err(XidError::Truncated);
            }
            // Group length is two octets, high-order first.
            let gl = u16::from_be_bytes([body[pos + 1], body[pos + 2]]) as usize;
            let start = pos + 3;
            let end = start.checked_add(gl).ok_or(XidError::Truncated)?;
            if end > body.len() {
                return Err(XidError::Truncated);
            }
            match gi {
                GI_PARAMETER => xid.read_parameters(&body[start..end])?,
                GI_PRIVATE => xid.read_private(&body[start..end])?,
                _ => {} // an unrecognized group is skipped whole
            }
            pos = end;
        }
        Ok(xid)
    }

    fn read_parameters(&mut self, mut field: &[u8]) -> Result<(), XidError> {
        while let Some((pi, value, rest)) = take_param(field)? {
            field = rest;
            match pi {
                pi::HDLC_OPTIONAL => {
                    // Table 11a Note 1 makes this four octets, and real
                    // modems send three: both ends of the call in the V.22bis
                    // vector do, and each of their XIDs was thrown away whole
                    // for it. Bit 24 is the last one the note names and three
                    // octets hold it, so a shorter mask is read with the rest
                    // as zero.
                    if value.is_empty() || value.len() > 4 {
                        return Err(XidError::BadLength { pi, len: value.len() as u8 });
                    }
                    let mut octets = [0u8; 4];
                    octets[..value.len()].copy_from_slice(value);
                    let mask = u32::from_le_bytes(octets);
                    // The options and nothing else. Note 1: "A receiver of
                    // these frames should ignore these bit positions" -- the
                    // ones the encoding rules fix, of which bit 16 differs
                    // between a command and a response.
                    self.srej_single = mask & (1 << (hdlc_bit::SREJ_SINGLE - 1)) != 0;
                    self.test_frame = mask & (1 << (hdlc_bit::TEST_FRAME - 1)) != 0;
                    self.fcs32 = mask & (1 << (hdlc_bit::FCS32 - 1)) != 0;
                    self.srej_multiple = mask & (1 << (hdlc_bit::SREJ_MULTIPLE - 1)) != 0;
                }
                pi::N401_TRANSMIT => self.n401_transmit = Some(be_u16(pi, value)? / 8),
                pi::N401_RECEIVE => self.n401_receive = Some(be_u16(pi, value)? / 8),
                pi::WINDOW_TRANSMIT => self.window_transmit = Some(be_u16(pi, value)? as u8),
                pi::WINDOW_RECEIVE => self.window_receive = Some(be_u16(pi, value)? as u8),
                _ => {}
            }
        }
        Ok(())
    }

    fn read_private(&mut self, mut field: &[u8]) -> Result<(), XidError> {
        let mut is_v42bis = false;
        while let Some((pi, value, rest)) = take_param(field)? {
            field = rest;
            match pi {
                private_pi::PARAMETER_SET => is_v42bis = value == PARAMETER_SET_V42,
                // Everything after the identifier belongs to whichever set it
                // named, so ignore the rest if it was not V.42bis.
                private_pi::COMPRESSION_REQUEST if is_v42bis => {
                    let v = *value.first().ok_or(XidError::Truncated)?;
                    self.compression = Some(Compression::from_bits(v));
                }
                private_pi::CODEWORDS if is_v42bis => {
                    self.codewords = Some(be_u16(pi, value)?);
                }
                private_pi::MAX_STRING if is_v42bis => {
                    self.max_string = Some(*value.first().ok_or(XidError::Truncated)?);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The V.44 half of the user data subfield (Table A.1).
    fn read_user_data(&mut self, mut field: &[u8]) -> Result<(), XidError> {
        let mut is_v44 = false;
        let mut offer = V44Offer::proposal(Compression::Neither);
        let mut said_anything = false;
        while let Some((pi, value, rest)) = take_param(field)? {
            field = rest;
            if pi == user_pi::PARAMETER_SET {
                is_v44 = value == PARAMETER_SET_V44;
                continue;
            }
            if !is_v44 {
                // Everything after the identifier belongs to whatever set it
                // named, and this one is not V.44.
                continue;
            }
            match pi {
                user_pi::REQUEST => {
                    let v = *value.first().ok_or(XidError::Truncated)?;
                    offer.compression = Compression::from_bits(v);
                    said_anything = true;
                }
                user_pi::CODEWORDS_TRANSMIT => offer.transmit.n2 = be_u16(pi, value)?,
                user_pi::CODEWORDS_RECEIVE => offer.receive.n2 = be_u16(pi, value)?,
                user_pi::MAX_STRING_TRANSMIT => {
                    offer.transmit.n7 = *value.first().ok_or(XidError::Truncated)?;
                }
                user_pi::MAX_STRING_RECEIVE => {
                    offer.receive.n7 = *value.first().ok_or(XidError::Truncated)?;
                }
                user_pi::HISTORY_TRANSMIT => offer.transmit.n8 = be_u16(pi, value)?,
                user_pi::HISTORY_RECEIVE => offer.receive.n8 = be_u16(pi, value)?,
                // C0's packet-method bits are "ignored for modem connections".
                _ => {}
            }
        }
        if is_v44 && said_anything {
            self.v44 = Some(offer);
        }
        Ok(())
    }

    /// What the two ends settled on for V.44, if anything.
    ///
    /// None means it is not running, and the connection falls back to whatever
    /// V.42bis agreed -- which 7.3 allows for by having the responder name at
    /// most one of the two.
    pub fn v44_params(&self, other: &Self) -> Option<V44Offer> {
        self.v44?.resolve(other.v44?)
    }

    /// Settle a proposal against a reply.
    ///
    /// V.42 9.2.3, 9.2.4 and V.42bis 5.1 all say the same thing for their own
    /// parameters: where the two ends differ, the lower value is used.
    pub fn resolve(&self, other: &Self) -> Self {
        Self {
            n401_transmit: lower(self.n401_transmit, other.n401_transmit, N401_DEFAULT),
            n401_receive: lower(self.n401_receive, other.n401_receive, N401_DEFAULT),
            window_transmit: lower(self.window_transmit, other.window_transmit, K_DEFAULT),
            window_receive: lower(self.window_receive, other.window_receive, K_DEFAULT),
            // A capability is used only if both ends offer it.
            fcs32: self.fcs32 && other.fcs32,
            srej_single: self.srej_single && other.srej_single,
            test_frame: self.test_frame && other.test_frame,
            srej_multiple: self.srej_multiple && other.srej_multiple,
            compression: match (self.compression, other.compression) {
                (Some(a), Some(b)) => Some(a.intersect(b)),
                _ => None,
            },
            codewords: lower(self.codewords, other.codewords, v42bis::DEFAULT_N2),
            max_string: lower(self.max_string, other.max_string, v42bis::DEFAULT_N7),
            // V.44's two directions are settled crosswise rather than by
            // taking the lower of matching fields, so it has its own.
            v44: self.v44_params(other),
        }
    }

    /// V.42bis parameters implied by a settled negotiation.
    pub fn v42bis_params(&self) -> Option<v42bis::Params> {
        let compression = self.compression?;
        if compression == Compression::Neither {
            return None;
        }
        Some(v42bis::Params {
            n2: self.codewords.unwrap_or(v42bis::DEFAULT_N2),
            n7: self.max_string.unwrap_or(v42bis::DEFAULT_N7),
        })
    }
}

/// The lower of two proposals, where an absent one is not silence.
///
/// Every parameter here has a value its Recommendation gives it when nobody
/// proposes one: N401 is 128 and k is 15 (V.42 9.2.3, 9.2.4), N2 is 512 and N7
/// is 6 (V.42bis 6.4). A far end that sends no P1 has not declined to have an
/// opinion -- it is using 512, and an end that reads the absence as "whatever
/// you like" comes away with a dictionary the far end does not have and
/// delivers nonsense built out of it.
///
/// Nothing went wrong while every value proposed here was already the default.
/// It would have gone wrong the moment one of them was not.
fn lower<T: Ord + Copy>(a: Option<T>, b: Option<T>, default: T) -> Option<T> {
    Some(a.unwrap_or(default).min(b.unwrap_or(default)))
}

fn be_u16(pi: u8, value: &[u8]) -> Result<u16, XidError> {
    match value.len() {
        1 => Ok(u16::from(value[0])),
        2 => Ok(u16::from_be_bytes([value[0], value[1]])),
        len => Err(XidError::BadLength { pi, len: len as u8 }),
    }
}

fn push_param(out: &mut Vec<u8>, pi: u8, value: &[u8]) {
    out.push(pi);
    out.push(value.len() as u8);
    out.extend_from_slice(value);
}

fn push_subfield(out: &mut Vec<u8>, gi: u8, params: &[u8]) {
    out.push(gi);
    out.extend_from_slice(&(params.len() as u16).to_be_bytes());
    out.extend_from_slice(params);
}

/// One parameter taken off the front of a field: its identifier, its value,
/// and whatever follows it.
type Parameter<'a> = (u8, &'a [u8], &'a [u8]);

fn take_param(field: &[u8]) -> Result<Option<Parameter<'_>>, XidError> {
    if field.is_empty() {
        return Ok(None);
    }
    if field.len() < 2 {
        return Err(XidError::Truncated);
    }
    let pi = field[0];
    let pl = field[1] as usize;
    let end = 2 + pl;
    if end > field.len() {
        return Err(XidError::Truncated);
    }
    Ok(Some((pi, &field[2..end], &field[end..])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proposal_round_trips() {
        let xid = Xid::proposal(Compression::Both);
        let decoded = Xid::decode(&xid.encode(Kind::Command)).unwrap();
        assert_eq!(decoded, xid);
    }

    #[test]
    fn the_format_identifier_is_the_general_purpose_one() {
        // V.42 12.2.2.
        let bytes = Xid::proposal(Compression::Both).encode(Kind::Command);
        assert_eq!(bytes[0], 0b1000_0010);
    }

    #[test]
    fn subfields_carry_the_specified_group_identifiers() {
        let bytes = Xid::proposal(Compression::Both).encode(Kind::Command);
        assert!(bytes.contains(&GI_PARAMETER), "parameter subfield missing");
        assert!(bytes.contains(&GI_PRIVATE), "private subfield missing");
        assert_eq!(GI_PARAMETER, 0b1000_0000);
        assert_eq!(GI_PRIVATE, 0b1111_0000);
    }

    #[test]
    fn n401_is_carried_in_bits_not_octets() {
        // V.42 12.2.2 Note 3.
        let xid = Xid { n401_transmit: Some(128), ..Default::default() };
        let bytes = xid.encode(Kind::Command);
        let at = bytes
            .windows(2)
            .position(|w| w == [pi::N401_TRANSMIT, 2])
            .expect("N401 parameter not found");
        assert_eq!(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]), 1024);
        // And it comes back in octets.
        assert_eq!(Xid::decode(&bytes).unwrap().n401_transmit, Some(128));
    }

    #[test]
    fn multi_octet_values_are_high_order_first() {
        // V.42 12.2.2 Note 4.
        let xid = Xid {
            compression: Some(Compression::Both),
            codewords: Some(0x0400),
            ..Default::default()
        };
        let bytes = xid.encode(Kind::Command);
        let at = bytes
            .windows(2)
            .position(|w| w == [private_pi::CODEWORDS, 2])
            .expect("codewords parameter not found");
        assert_eq!(bytes[at + 2], 0x04, "high-order octet should come first");
        assert_eq!(bytes[at + 3], 0x00);
    }

    #[test]
    fn the_parameter_set_identifier_spells_v42() {
        // V.42bis Annex A Table A-1. V.42 12.2.2 Note 2 gives 0x2A for the
        // first octet, which spells nothing; 0x56 is 'V' and is what is used.
        assert_eq!(PARAMETER_SET_V42, [0x56, 0x34, 0x32]);
        assert_eq!(&PARAMETER_SET_V42, b"V42");
    }

    #[test]
    fn the_parameter_set_identifier_comes_first_in_the_private_subfield() {
        // V.42 12.2.2 Note 2 requires it.
        let bytes = Xid::proposal(Compression::Both).encode(Kind::Command);
        let gi_at = bytes.iter().position(|&b| b == GI_PRIVATE).unwrap();
        assert_eq!(bytes[gi_at + 3], private_pi::PARAMETER_SET);
    }

    #[test]
    fn compression_direction_encodes_as_two_bits() {
        for (direction, bits) in [
            (Compression::Neither, 0),
            (Compression::InitiatorToResponder, 1),
            (Compression::ResponderToInitiator, 2),
            (Compression::Both, 3),
        ] {
            assert_eq!(direction.to_bits(), bits);
            assert_eq!(Compression::from_bits(bits), direction);
        }
    }

    #[test]
    fn the_hdlc_mask_sets_the_positions_the_encoding_rules_demand() {
        // V.42 12.2.2 Note 1.
        let mask = Xid { test_frame: false, ..Default::default() }.hdlc_mask(Kind::Command);
        for bit in hdlc_bit::REQUIRED {
            assert!(mask & (1 << (bit - 1)) != 0, "bit {bit} should be set");
        }
    }

    #[test]
    fn a_command_asking_for_32_bits_still_sets_bit_16() {
        // V.42 Table 11a Note 1: "the transmitter of an XID command frame
        // shall set bit positions 2, 4, 8, 9, 12 and 16 to 1", with no
        // exception for bit 17.
        let mask = Xid { fcs32: true, ..Default::default() }.hdlc_mask(Kind::Command);
        assert!(mask & (1 << 16) != 0, "bit 17 should be set");
        assert!(mask & (1 << 15) != 0, "bit 16 was cleared in a command");
    }

    #[test]
    fn a_response_agreeing_to_32_bits_clears_bit_16() {
        // The same note: a response sets them too, "except bit position 16
        // shall be set to 0 if bit position 17 is set to 1".
        let wide = Xid { fcs32: true, ..Default::default() }.hdlc_mask(Kind::Response);
        assert!(wide & (1 << 16) != 0, "bit 17 should be set");
        assert!(wide & (1 << 15) == 0, "bit 16 should have been cleared");
        let narrow = Xid { fcs32: false, ..Default::default() }.hdlc_mask(Kind::Response);
        assert!(narrow & (1 << 15) != 0, "bit 16 cleared without bit 17");
    }

    #[test]
    fn a_receiver_ignores_the_positions_the_encoding_rules_fix() {
        // Note 1: "A receiver of these frames should ignore these bit
        // positions." A far end's command and its response differ in bit 16,
        // and neither says anything about what it can do.
        let field = |mask: u32| {
            let mut params = Vec::new();
            push_param(&mut params, pi::HDLC_OPTIONAL, &mask.to_le_bytes());
            let mut bytes = vec![FI_GENERAL_PURPOSE];
            push_subfield(&mut bytes, GI_PARAMETER, &params);
            Xid::decode(&bytes).expect("did not decode")
        };
        let fcs32 = 1 << (hdlc_bit::FCS32 - 1);
        assert_eq!(field(required_mask()), field(0));
        assert_eq!(field(required_mask() | fcs32), field(fcs32));
        assert_eq!(field((required_mask() & !(1 << 15)) | fcs32), field(fcs32));
        assert!(field(fcs32).fcs32);
        assert!(!field(required_mask()).fcs32);
    }

    #[test]
    fn a_command_and_its_response_agree_on_everything_but_bit_16() {
        let xid = Xid::proposal(Compression::Both);
        let command = xid.encode(Kind::Command);
        let response = xid.encode(Kind::Response);
        assert_eq!(Xid::decode(&command), Xid::decode(&response));
        let differing: Vec<usize> =
            (0..command.len()).filter(|&i| command[i] != response[i]).collect();
        assert_eq!(differing.len(), 1, "{command:02x?} against {response:02x?}");
        assert_eq!(command[differing[0]] ^ response[differing[0]], 0x80, "not bit 16");
    }

    #[test]
    fn capabilities_survive_a_round_trip() {
        let xid = Xid {
            fcs32: true,
            test_frame: true,
            srej_multiple: true,
            ..Default::default()
        };
        let back = Xid::decode(&xid.encode(Kind::Command)).unwrap();
        assert!(back.fcs32 && back.test_frame && back.srej_multiple);
    }

    #[test]
    fn the_lower_value_wins() {
        // V.42 9.2.3, 9.2.4 and V.42bis 5.1.
        let mine = Xid {
            n401_transmit: Some(256),
            window_transmit: Some(15),
            compression: Some(Compression::Both),
            codewords: Some(2048),
            max_string: Some(32),
            ..Default::default()
        };
        let theirs = Xid {
            n401_transmit: Some(128),
            window_transmit: Some(7),
            compression: Some(Compression::Both),
            codewords: Some(512),
            max_string: Some(6),
            ..Default::default()
        };
        let agreed = mine.resolve(&theirs);
        assert_eq!(agreed.n401_transmit, Some(128));
        assert_eq!(agreed.window_transmit, Some(7));
        assert_eq!(agreed.codewords, Some(512));
        assert_eq!(agreed.max_string, Some(6));
    }

    #[test]
    fn a_capability_needs_both_ends() {
        let mine = Xid { fcs32: true, test_frame: true, ..Default::default() };
        let theirs = Xid { fcs32: false, test_frame: true, ..Default::default() };
        let agreed = mine.resolve(&theirs);
        assert!(!agreed.fcs32, "one end declining should settle it");
        assert!(agreed.test_frame);
    }

    #[test]
    fn compression_directions_intersect() {
        let one_way = Xid {
            compression: Some(Compression::InitiatorToResponder),
            ..Default::default()
        };
        let both = Xid { compression: Some(Compression::Both), ..Default::default() };
        assert_eq!(
            one_way.resolve(&both).compression,
            Some(Compression::InitiatorToResponder)
        );

        let other_way = Xid {
            compression: Some(Compression::ResponderToInitiator),
            ..Default::default()
        };
        assert_eq!(
            one_way.resolve(&other_way).compression,
            Some(Compression::Neither),
            "opposite single directions leave nothing in common"
        );
    }

    #[test]
    fn settled_parameters_configure_the_compressor() {
        let agreed = Xid {
            compression: Some(Compression::Both),
            codewords: Some(1024),
            max_string: Some(16),
            ..Default::default()
        };
        let params = agreed.v42bis_params().unwrap();
        assert_eq!(params.n2, 1024);
        assert_eq!(params.n7, 16);

        let off = Xid { compression: Some(Compression::Neither), ..Default::default() };
        assert!(off.v42bis_params().is_none());
    }

    #[test]
    fn unrecognized_groups_and_parameters_are_ignored() {
        // V.42 12.2.2: "Fields that are not recognized are ignored."
        let proposal = Xid::proposal(Compression::Both).encode(Kind::Command);
        // A group nobody has defined, where 12.2.1.2's ascending order puts
        // it: ahead of the others, since the user data subfield after them
        // runs to the end of the field.
        let mut bytes = vec![proposal[0], 0x55];
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        bytes.extend_from_slice(&proposal[1..]);
        let decoded = Xid::decode(&bytes).unwrap();
        assert_eq!(decoded.window_transmit, Some(crate::lapm::DEFAULT_K));
        assert!(decoded.v44.is_some(), "what followed it was lost");
    }

    #[test]
    fn a_private_subfield_for_another_standard_is_ignored() {
        // The identifier says which set the parameters belong to, so a subfield
        // naming something else must not be read as V.42bis.
        let mut private = Vec::new();
        push_param(&mut private, private_pi::PARAMETER_SET, b"XYZ");
        push_param(&mut private, private_pi::CODEWORDS, &4096u16.to_be_bytes());
        let mut bytes = vec![FI_GENERAL_PURPOSE];
        push_subfield(&mut bytes, GI_PRIVATE, &private);

        let decoded = Xid::decode(&bytes).unwrap();
        assert_eq!(decoded.codewords, None, "another standard's value was taken");
    }

    #[test]
    fn truncated_fields_are_rejected() {
        assert_eq!(Xid::decode(&[]), Err(XidError::Truncated));
        assert_eq!(
            Xid::decode(&[FI_GENERAL_PURPOSE, GI_PARAMETER, 0]),
            Err(XidError::Truncated)
        );
        // A group length longer than what follows.
        assert_eq!(
            Xid::decode(&[FI_GENERAL_PURPOSE, GI_PARAMETER, 0, 40, 1]),
            Err(XidError::Truncated)
        );
    }

    #[test]
    fn a_foreign_format_identifier_is_rejected() {
        assert_eq!(Xid::decode(&[0x01]), Err(XidError::UnknownFormat(0x01)));
    }

    #[test]
    fn selective_reject_sits_at_the_bit_position_the_note_names() {
        // V.42 12.2.2 Note 1 lists four bits of the HDLC optional functions
        // mask and writes one of them as "3A" where the others are plain
        // numbers. The "A" is ISO/IEC 8885's sub-lettering for the variants of
        // an option, not part of the position: all four entries are positions
        // in the same 32-bit mask, and this is position 3.
        //
        // Which the surrounding text confirms. The note requires a transmitter
        // to set positions 2, 4, 8, 9, 12 and 16 whatever it supports, and 3
        // is the one gap in that run -- the position left for the option the
        // note is describing.
        let mask = Xid { srej_single: true, ..Default::default() }.hdlc_mask(Kind::Command);
        assert_eq!(mask & !required_mask(), 1 << 2, "bit 3 counting from one");
    }

    #[test]
    fn selective_reject_is_agreed_and_not_announced() {
        // 8.4.5.1 has an end that did not agree treat an SREJ as an
        // unrecognized command/response control field, which under 8.5.5 ends
        // the connection. Sending one uninvited does not degrade the link, it
        // drops it.
        let asking = Xid::proposal(Compression::Both);
        let silent = Xid { srej_single: false, ..Xid::proposal(Compression::Both) };
        assert!(asking.srej_single, "this end should be offering it");
        assert!(asking.resolve(&asking).srej_single, "both offered");
        assert!(!asking.resolve(&silent).srej_single, "the far end did not");
        assert!(!silent.resolve(&asking).srej_single, "this end did not");
    }

    /// The information fields of the two XIDs in `tests/vectors/v22bis-2400.wav`,
    /// a Conexant softmodem calling an ISP: its command, and the host's
    /// response. Both carry a three-octet option mask, and the command ends
    /// with a V.44 offer laid out as Table A.1 has it.
    const CALLER_XID: &[u8] = &[
        0x82, 0x80, 0x00, 0x13, 0x03, 0x03, 0x8a, 0x89, 0x00, 0x05, 0x02, 0x04, 0x00, 0x06, 0x02,
        0x04, 0x00, 0x07, 0x01, 0x0f, 0x08, 0x01, 0x0f, 0xf0, 0x00, 0x0f, 0x00, 0x03, 0x56, 0x34,
        0x32, 0x01, 0x01, 0x03, 0x02, 0x02, 0x08, 0x00, 0x03, 0x01, 0x20, 0xff, 0x40, 0x03, 0x56,
        0x34, 0x34, 0x41, 0x01, 0x00, 0x42, 0x01, 0x03, 0x43, 0x02, 0x08, 0x00, 0x44, 0x02, 0x08,
        0x00, 0x45, 0x01, 0x8e, 0x46, 0x01, 0x8e, 0x47, 0x02, 0x20, 0x00, 0x48, 0x02, 0x20, 0x00,
    ];
    const HOST_XID: &[u8] = &[
        0x82, 0x80, 0x00, 0x13, 0x03, 0x03, 0x8a, 0x89, 0x00, 0x05, 0x02, 0x04, 0x00, 0x06, 0x02,
        0x04, 0x00, 0x07, 0x01, 0x0f, 0x08, 0x01, 0x0f, 0xf0, 0x00, 0x0f, 0x00, 0x03, 0x56, 0x34,
        0x32, 0x01, 0x01, 0x03, 0x02, 0x02, 0x08, 0x00, 0x03, 0x01, 0x20,
    ];

    #[test]
    fn the_xids_of_a_real_call_are_read() {
        let host = Xid::decode(HOST_XID).expect("the host's XID was refused");
        assert_eq!(
            host,
            Xid {
                n401_transmit: Some(128),
                n401_receive: Some(128),
                window_transmit: Some(15),
                window_receive: Some(15),
                fcs32: false,
                srej_single: false,
                test_frame: false,
                srej_multiple: false,
                compression: Some(Compression::Both),
                codewords: Some(2048),
                max_string: Some(32),
                v44: None,
            }
        );
        let caller = Xid::decode(CALLER_XID).expect("the caller's XID was refused");
        let offer = v44::Params { n2: 2048, n7: 142, n8: 8192 };
        assert_eq!(
            caller,
            Xid {
                v44: Some(V44Offer { compression: Compression::Both, transmit: offer, receive: offer }),
                ..host
            }
        );
        // And answered, this end would have agreed V.44 with it.
        assert!(Xid::proposal(Compression::Both).resolve(&caller).v44.is_some());
    }

    #[test]
    fn an_option_mask_shorter_than_four_octets_is_read_as_far_as_it_goes() {
        let field = |mask: &[u8]| {
            let mut params = Vec::new();
            push_param(&mut params, pi::HDLC_OPTIONAL, mask);
            let mut bytes = vec![FI_GENERAL_PURPOSE];
            push_subfield(&mut bytes, GI_PARAMETER, &params);
            Xid::decode(&bytes)
        };
        // Bit 3, single-frame selective reject, in a one-octet mask.
        assert!(field(&[0x04]).expect("one octet").srej_single);
        // Bit 24 in a three-octet one.
        assert!(field(&[0, 0, 0x80]).expect("three octets").srej_multiple);
        assert_eq!(field(&[]), Err(XidError::BadLength { pi: pi::HDLC_OPTIONAL, len: 0 }));
        assert_eq!(field(&[0; 5]), Err(XidError::BadLength { pi: pi::HDLC_OPTIONAL, len: 5 }));
    }

    /// The bits 12.2.2 Note 1 requires a transmitter to set whatever it does.
    fn required_mask() -> u32 {
        hdlc_bit::REQUIRED.iter().fold(0, |m, b| m | 1 << (b - 1))
    }

    #[test]
    fn an_absent_parameter_stays_absent() {
        // V.42 12.2.2 Note 3: absence leaves a previously negotiated value
        // unchanged, so it must be distinguishable from a value of zero.
        let sparse = Xid { window_receive: Some(7), ..Default::default() };
        let back = Xid::decode(&sparse.encode(Kind::Command)).unwrap();
        assert_eq!(back.window_receive, Some(7));
        assert_eq!(back.window_transmit, None);
        assert_eq!(back.n401_transmit, None);
    }
}

#[cfg(test)]
mod v44_negotiation {
    use super::*;

    /// Table A.1: the user data subfield opens with the parameter set
    /// identifier, and it spells "V44".
    #[test]
    fn the_user_data_subfield_names_the_recommendation_first() {
        assert_eq!(PARAMETER_SET_V44, [0x56, 0x34, 0x34]);
        let bytes = Xid::proposal(Compression::Both).encode(Kind::Command);
        // Walked rather than searched for: 0xff is a perfectly ordinary
        // parameter value and looking for the octet finds one of those. The
        // data link layer subfields have lengths, and the user data subfield
        // is what is left.
        let mut at = 1usize;
        let mut groups = Vec::new();
        while at < bytes.len() && bytes[at] != GI_USER_DATA {
            let len = u16::from_be_bytes([bytes[at + 1], bytes[at + 2]]) as usize;
            groups.push(bytes[at]);
            at += 3 + len;
        }
        assert_eq!(groups, [GI_PARAMETER, GI_PRIVATE]);
        // 7.3 puts it "immediately before the FCS", which is to say last.
        assert_eq!(bytes.get(at), Some(&GI_USER_DATA), "no user data subfield");
        // Group identifier, then the first parameter: 12.2.1.3 gives this
        // subfield no group length.
        assert_eq!(bytes[at + 1], user_pi::PARAMETER_SET);
        assert_eq!(bytes[at + 2], 3, "the identifier is three octets");
        assert_eq!(&bytes[at + 3..at + 6], &PARAMETER_SET_V44);
        // And the parameters run to the end of the field.
        let mut field = &bytes[at + 1..];
        let mut last = 0;
        while let Some((pi, _, rest)) = take_param(field).expect("the parameters do not tile") {
            last = pi;
            field = rest;
        }
        assert_eq!(last, user_pi::HISTORY_RECEIVE);
    }

    /// V.44 Table A.1 laid out by hand, as a far end that follows it sends it:
    /// the group identifier and then the parameters, with no length between.
    #[test]
    fn a_v44_offer_laid_out_as_table_a1_is_read() {
        let mut bytes = vec![FI_GENERAL_PURPOSE];
        let mut params = Vec::new();
        push_param(&mut params, pi::WINDOW_TRANSMIT, &[15]);
        push_subfield(&mut bytes, GI_PARAMETER, &params);
        bytes.extend_from_slice(&[
            0xff, // GI 11111111
            0x40, 0x03, b'V', b'4', b'4',
            0x41, 0x01, 0x00,
            0x42, 0x01, 0x03,
            0x43, 0x02, 0x08, 0x00,
            0x44, 0x02, 0x04, 0x00,
            0x45, 0x01, 0xff,
            0x46, 0x01, 0x40,
            0x47, 0x02, 0x18, 0x00,
            0x48, 0x02, 0x0c, 0x00,
        ]);
        let xid = Xid::decode(&bytes).expect("did not decode");
        assert_eq!(xid.window_transmit, Some(15));
        assert_eq!(
            xid.v44,
            Some(V44Offer {
                compression: Compression::Both,
                transmit: v44::Params { n2: 2048, n7: 255, n8: 6144 },
                receive: v44::Params { n2: 1024, n7: 64, n8: 3072 },
            })
        );
    }

    /// An XID offers both algorithms, because 7.3 has the responder pick one.
    #[test]
    fn one_proposal_offers_both_algorithms() {
        let xid = Xid::proposal(Compression::Both);
        assert!(xid.compression.is_some(), "V.42bis was not offered");
        assert!(xid.v44.is_some(), "V.44 was not offered");
        let back = Xid::decode(&xid.encode(Kind::Command)).expect("did not decode");
        assert_eq!(back.v44, xid.v44);
        assert_eq!(back.compression, xid.compression);
        assert_eq!(back.codewords, xid.codewords);
    }

    /// Everything in Table A.1 survives the round trip, including values that
    /// are not the defaults.
    #[test]
    fn the_parameters_come_back_as_they_went() {
        let mut xid = Xid::proposal(Compression::Both);
        xid.v44 = Some(V44Offer {
            compression: Compression::InitiatorToResponder,
            transmit: v44::Params { n2: 4096, n7: 200, n8: 9000 },
            receive: v44::Params { n2: 1024, n7: 64, n8: 3072 },
        });
        let back = Xid::decode(&xid.encode(Kind::Command)).expect("did not decode");
        assert_eq!(back.v44, xid.v44);
    }

    /// 7.4: the directions are relative to whoever sent them, so a proposal
    /// and its complementary answer agree rather than cancelling out.
    #[test]
    fn the_directions_are_read_from_each_ends_own_point_of_view() {
        let one = V44Offer::proposal(Compression::InitiatorToResponder);
        // "The complementary response to one entity's P0 value of 01 ... is a
        // P0 value of 10."
        let other = V44Offer::proposal(Compression::ResponderToInitiator);
        let agreed = one.resolve(other).expect("they did not agree");
        assert_eq!(agreed.compression, Compression::InitiatorToResponder);

        // And an answer that proposes the same direction as the question is
        // proposing the opposite one, which leaves nothing agreed.
        assert_eq!(one.resolve(one), None);
    }

    /// 7.4: "the proposed P2T from one entity is compared with the proposed
    /// P2R from the other entity" -- crosswise, because one end transmitting
    /// is the other end receiving.
    #[test]
    fn each_direction_is_settled_against_the_other_ends_opposite() {
        let mine = V44Offer {
            compression: Compression::Both,
            transmit: v44::Params { n2: 4096, n7: 255, n8: 12288 },
            receive: v44::Params { n2: 4096, n7: 255, n8: 12288 },
        };
        let theirs = V44Offer {
            compression: Compression::Both,
            // What they will send, and so what this end must read.
            transmit: v44::Params { n2: 512, n7: 64, n8: 1536 },
            // What they can read, and so what this end may send.
            receive: v44::Params { n2: 1024, n7: 100, n8: 3072 },
        };
        let agreed = mine.resolve(theirs).expect("they did not agree");
        assert_eq!(agreed.transmit, v44::Params { n2: 1024, n7: 100, n8: 3072 });
        assert_eq!(agreed.receive, v44::Params { n2: 512, n7: 64, n8: 1536 });
    }

    /// A far end that has never heard of V.44 leaves the subfield out, and
    /// what is left is an ordinary V.42bis negotiation.
    #[test]
    fn a_far_end_that_does_not_know_it_is_not_pressed() {
        let mine = Xid::proposal(Compression::Both);
        let mut theirs = Xid::proposal(Compression::Both);
        theirs.v44 = None;
        let agreed = mine.resolve(&theirs);
        assert_eq!(agreed.v44, None, "V.44 was agreed with an end that never offered it");
        assert!(agreed.v42bis_params().is_some(), "V.42bis was lost as well");
    }

    /// A user data subfield belonging to something else is skipped whole,
    /// rather than read as V.44 parameters.
    #[test]
    fn a_user_data_subfield_for_something_else_is_ignored() {
        let mut user = Vec::new();
        push_param(&mut user, user_pi::PARAMETER_SET, b"XYZ");
        push_param(&mut user, user_pi::REQUEST, &[0b11]);
        push_param(&mut user, user_pi::MAX_STRING_TRANSMIT, &[99]);
        let mut bytes = vec![FI_GENERAL_PURPOSE, GI_USER_DATA];
        bytes.extend_from_slice(&user);
        let xid = Xid::decode(&bytes).expect("did not decode");
        assert_eq!(xid.v44, None);
    }

    /// 7.4 makes a proposal below Table 10's minimum "a procedural error".
    /// Declining V.44 leaves V.42bis, which beats dropping the call.
    #[test]
    fn a_proposal_below_the_minimum_is_declined_rather_than_taken() {
        let mine = V44Offer::proposal(Compression::Both);
        let theirs = V44Offer {
            compression: Compression::Both,
            transmit: v44::Params { n2: 64, n7: 8, n8: 16 },
            receive: v44::Params { n2: 64, n7: 8, n8: 16 },
        };
        assert_eq!(mine.resolve(theirs), None);
    }

    /// The capability byte a modem sends: Table A.1's "neither packet method
    /// nor multi-packet method supported: for modem connections only", and
    /// parameters negotiated here rather than after the link is up.
    #[test]
    fn the_capability_byte_says_modem_and_xid() {
        assert_eq!(CAPABILITY_MODEM, 0);
        let bytes = Xid::proposal(Compression::Both).encode(Kind::Command);
        let at = bytes
            .windows(3)
            .position(|w| w == [user_pi::CAPABILITY, 1, CAPABILITY_MODEM])
            .expect("no capability parameter");
        let _ = at;
    }
}
