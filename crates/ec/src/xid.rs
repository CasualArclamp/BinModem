//! XID parameter negotiation (V.42 8.10 and 12.2, V.42bis Annex A).
//!
//! The information field of an XID frame carries a format identifier followed
//! by subfields, each a group identifier, a two-octet group length, and a run
//! of parameter identifier / length / value triples.
//!
//! This is how N401, the window size, the frame check sequence width and the
//! V.42bis parameters are actually agreed. Without it both ends simply run on
//! defaults, which works but leaves compression switched off.

use crate::v42bis;

/// The ISO "general purpose" format identifier (V.42 12.2.2).
pub const FI_GENERAL_PURPOSE: u8 = 0b1000_0010;

/// Group identifier of the parameter negotiation subfield.
pub const GI_PARAMETER: u8 = 0b1000_0000;
/// Group identifier of the private parameter negotiation subfield.
pub const GI_PRIVATE: u8 = 0b1111_0000;

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
    pub const TEST_FRAME: u32 = 14;
    pub const FCS32: u32 = 17;
    pub const SREJ_MULTIPLE: u32 = 24;
    /// Bit positions the encoding rules require a transmitter to set, whatever
    /// it actually supports. Receivers are told to ignore them.
    pub const REQUIRED: [u32; 6] = [2, 4, 8, 9, 12, 16];
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
    pub test_frame: bool,
    /// Selective retransmission with a span list is supported.
    pub srej_multiple: bool,
    /// V.42bis compression, present only when the private subfield appears.
    pub compression: Option<Compression>,
    /// P1, total codewords.
    pub codewords: Option<u16>,
    /// P2, maximum string length.
    pub max_string: Option<u8>,
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
            test_frame: true,
            srej_multiple: false,
            compression: Some(compression),
            codewords: Some(v42bis::OFFERED_N2),
            max_string: Some(v42bis::OFFERED_N7),
        }
    }

    /// Encode as the information field of an XID frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FI_GENERAL_PURPOSE];

        // Parameter negotiation subfield.
        let mut params = Vec::new();
        push_param(&mut params, pi::HDLC_OPTIONAL, &self.hdlc_mask().to_le_bytes());
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
        out
    }

    /// The 32-bit HDLC optional functions mask (V.42 12.2.2 Note 1).
    fn hdlc_mask(&self) -> u32 {
        let mut mask = 0u32;
        for bit in hdlc_bit::REQUIRED {
            mask |= 1 << (bit - 1);
        }
        if self.test_frame {
            mask |= 1 << (hdlc_bit::TEST_FRAME - 1);
        }
        if self.fcs32 {
            mask |= 1 << (hdlc_bit::FCS32 - 1);
            // Note 1: bit 16 is cleared when bit 17 is set.
            mask &= !(1 << 15);
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
                    if value.len() != 4 {
                        return Err(XidError::BadLength { pi, len: value.len() as u8 });
                    }
                    let mask = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
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
            test_frame: self.test_frame && other.test_frame,
            srej_multiple: self.srej_multiple && other.srej_multiple,
            compression: match (self.compression, other.compression) {
                (Some(a), Some(b)) => Some(a.intersect(b)),
                _ => None,
            },
            codewords: lower(self.codewords, other.codewords, v42bis::DEFAULT_N2),
            max_string: lower(self.max_string, other.max_string, v42bis::DEFAULT_N7),
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
        let decoded = Xid::decode(&xid.encode()).unwrap();
        assert_eq!(decoded, xid);
    }

    #[test]
    fn the_format_identifier_is_the_general_purpose_one() {
        // V.42 12.2.2.
        let bytes = Xid::proposal(Compression::Both).encode();
        assert_eq!(bytes[0], 0b1000_0010);
    }

    #[test]
    fn subfields_carry_the_specified_group_identifiers() {
        let bytes = Xid::proposal(Compression::Both).encode();
        assert!(bytes.contains(&GI_PARAMETER), "parameter subfield missing");
        assert!(bytes.contains(&GI_PRIVATE), "private subfield missing");
        assert_eq!(GI_PARAMETER, 0b1000_0000);
        assert_eq!(GI_PRIVATE, 0b1111_0000);
    }

    #[test]
    fn n401_is_carried_in_bits_not_octets() {
        // V.42 12.2.2 Note 3.
        let xid = Xid { n401_transmit: Some(128), ..Default::default() };
        let bytes = xid.encode();
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
        let bytes = xid.encode();
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
        let bytes = Xid::proposal(Compression::Both).encode();
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
        let mask = Xid { test_frame: false, ..Default::default() }.hdlc_mask();
        for bit in hdlc_bit::REQUIRED {
            assert!(mask & (1 << (bit - 1)) != 0, "bit {bit} should be set");
        }
    }

    #[test]
    fn requesting_a_32_bit_fcs_clears_bit_16() {
        // V.42 12.2.2 Note 1: bit 16 is cleared when bit 17 is set.
        let mask = Xid { fcs32: true, ..Default::default() }.hdlc_mask();
        assert!(mask & (1 << 16) != 0, "bit 17 should be set");
        assert!(mask & (1 << 15) == 0, "bit 16 should have been cleared");
    }

    #[test]
    fn capabilities_survive_a_round_trip() {
        let xid = Xid {
            fcs32: true,
            test_frame: true,
            srej_multiple: true,
            ..Default::default()
        };
        let back = Xid::decode(&xid.encode()).unwrap();
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
        let mut bytes = Xid::proposal(Compression::Both).encode();
        // Append a group nobody has defined.
        bytes.push(0x55);
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        let decoded = Xid::decode(&bytes).unwrap();
        assert_eq!(decoded.window_transmit, Some(crate::lapm::DEFAULT_K));
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
    fn an_absent_parameter_stays_absent() {
        // V.42 12.2.2 Note 3: absence leaves a previously negotiated value
        // unchanged, so it must be distinguishable from a value of zero.
        let sparse = Xid { window_receive: Some(7), ..Default::default() };
        let back = Xid::decode(&sparse.encode()).unwrap();
        assert_eq!(back.window_receive, Some(7));
        assert_eq!(back.window_transmit, None);
        assert_eq!(back.n401_transmit, None);
    }
}
