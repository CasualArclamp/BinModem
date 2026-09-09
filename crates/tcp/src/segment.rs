//! The segment: RFC 9293 3.1's header, and 3.2's options.
//!
//! Nothing here knows what a connection is. A segment goes out or comes in,
//! and the only judgement this file makes is whether the octets it was handed
//! are a segment at all.

/// 3.1's control bits, "also known as flags".
///
/// CWR and ECE belong to explicit congestion notification (RFC 3168), which
/// this does not do; they are named so that a segment carrying them reads back
/// as what it was rather than as a corrupt one.
pub mod flag {
    pub const CWR: u8 = 0x80;
    pub const ECE: u8 = 0x40;
    pub const URG: u8 = 0x20;
    pub const ACK: u8 = 0x10;
    pub const PSH: u8 = 0x08;
    pub const RST: u8 = 0x04;
    pub const SYN: u8 = 0x02;
    pub const FIN: u8 = 0x01;
}

/// 3.2's option kinds, which are the three an implementation "MUST support"
/// (MUST-4) and no others.
pub mod option {
    /// End of Option List.
    pub const END: u8 = 0;
    /// No-Operation, for aligning what follows.
    pub const NOP: u8 = 1;
    /// Maximum Segment Size, four octets including these two.
    pub const MSS: u8 = 2;
}

/// RFC 790's protocol number for TCP, which the pseudo-header carries.
pub const PROTOCOL_TCP: u8 = 6;

/// The header with no options in it.
pub const HEADER_LEN: usize = 20;

/// 3.7.1: "If an MSS Option is not received at connection setup, TCP
/// implementations MUST assume a default send MSS of 536 (576 - 40) for IPv4"
/// (MUST-15).
pub const DEFAULT_MSS: u16 = 536;

/// One segment, in the terms 3.1 gives its fields.
///
/// The checksum is not here. It is a function of the segment and of the two
/// addresses under it, so it is computed on the way out and checked on the way
/// in rather than carried about as a field that might be stale.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    pub source_port: u16,
    pub destination_port: u16,
    /// SEG.SEQ. "When a SYN is present, then SEG.SEQ is the sequence number of
    /// the SYN" (3.4).
    pub sequence: u32,
    /// SEG.ACK, which means nothing unless the ACK flag is set.
    pub acknowledgment: u32,
    pub flags: u8,
    /// SEG.WND. Held as sixteen bits because that is what is on the wire; REC-1
    /// asks for thirty-two in the connection record, and that is where it is.
    pub window: u16,
    pub urgent: u16,
    /// The Maximum Segment Size option, if the segment carried one. MUST-65:
    /// it "MUST NOT be sent in other segments" than a SYN.
    pub mss: Option<u16>,
    pub payload: Vec<u8>,
}

impl Segment {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub fn syn(&self) -> bool {
        self.has(flag::SYN)
    }

    pub fn ack(&self) -> bool {
        self.has(flag::ACK)
    }

    pub fn rst(&self) -> bool {
        self.has(flag::RST)
    }

    pub fn fin(&self) -> bool {
        self.has(flag::FIN)
    }

    /// SEG.LEN: 3.4's "number of octets occupied by the data in the segment
    /// (counting SYN and FIN)".
    ///
    /// The counting is the point. A SYN and a FIN each occupy one place in the
    /// sequence space although neither is an octet of anybody's data, which is
    /// what lets them be retransmitted and acknowledged like everything else.
    pub fn length(&self) -> u32 {
        self.payload.len() as u32
            + u32::from(self.syn())
            + u32::from(self.fin())
    }

    /// How long the header will be, options included: 3.1's Data Offset in
    /// octets rather than in words.
    fn header_len(&self) -> usize {
        // Only the MSS option is ever sent, and it is four octets, which is
        // already a whole word.
        HEADER_LEN + if self.mss.is_some() { 4 } else { 0 }
    }

    /// Write the segment out, with the checksum over it and the two addresses.
    pub fn to_bytes(&self, from: [u8; 4], to: [u8; 4]) -> Vec<u8> {
        let header = self.header_len();
        let mut out = Vec::with_capacity(header + self.payload.len());
        out.extend_from_slice(&self.source_port.to_be_bytes());
        out.extend_from_slice(&self.destination_port.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.acknowledgment.to_be_bytes());
        // Data Offset in thirty-two bit words, and the four reserved bits,
        // which "must be zero in generated segments" (3.1).
        out.push(((header / 4) as u8) << 4);
        out.push(self.flags);
        out.extend_from_slice(&self.window.to_be_bytes());
        out.extend_from_slice(&[0, 0]); // Checksum, filled in below
        out.extend_from_slice(&self.urgent.to_be_bytes());
        if let Some(mss) = self.mss {
            out.push(option::MSS);
            out.push(4);
            out.extend_from_slice(&mss.to_be_bytes());
        }
        debug_assert_eq!(out.len(), header, "the header is not the length it said");
        out.extend_from_slice(&self.payload);
        let sum = checksum(from, to, &out);
        out[16..18].copy_from_slice(&sum.to_be_bytes());
        out
    }

    /// Read a segment, if the octets are one and the checksum agrees.
    ///
    /// "The sender MUST generate it (MUST-2) and the receiver MUST check it
    /// (MUST-3)", so a segment that fails is not a segment.
    pub fn parse(from: [u8; 4], to: [u8; 4], bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN {
            return None;
        }
        let header = usize::from(bytes[12] >> 4) * 4;
        if header < HEADER_LEN || bytes.len() < header {
            return None;
        }
        // A good checksum over a block that includes the checksum field sums
        // to zero, the same way RFC 1071 3 has it for every other one.
        if checksum(from, to, bytes) != 0 {
            return None;
        }
        Some(Self {
            source_port: u16::from_be_bytes([bytes[0], bytes[1]]),
            destination_port: u16::from_be_bytes([bytes[2], bytes[3]]),
            sequence: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            acknowledgment: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            flags: bytes[13],
            window: u16::from_be_bytes([bytes[14], bytes[15]]),
            urgent: u16::from_be_bytes([bytes[18], bytes[19]]),
            mss: read_mss(&bytes[HEADER_LEN..header]),
            payload: bytes[header..].to_vec(),
        })
    }
}

/// Walk the options for a Maximum Segment Size.
///
/// MUST-6: "ignore without error any TCP Option it does not implement,
/// assuming that the option has a length field". MUST-7 asks for an illegal
/// length to be handled; there is nothing to reset from in here, so walking
/// stops, which leaves the segment readable and any option after the bad one
/// unread -- and an option list that cannot be walked is not one worth
/// believing the rest of.
fn read_mss(mut options: &[u8]) -> Option<u16> {
    while let Some((&kind, rest)) = options.split_first() {
        match kind {
            // "This is used at the end of all options" (3.2).
            option::END => return None,
            // Alignment, and the only other option with no length octet.
            option::NOP => options = rest,
            _ => {
                let &length = rest.first()?;
                let length = usize::from(length);
                if length < 2 || length > options.len() {
                    return None;
                }
                if kind == option::MSS && length == 4 {
                    return Some(u16::from_be_bytes([options[2], options[3]]));
                }
                options = &options[length..];
            }
        }
    }
    None
}

/// 3.1's checksum: the ones' complement of the ones' complement sum over the
/// pseudo-header, the header and the text.
///
/// The pseudo-header of Figure 2 is not transmitted. It is the two addresses,
/// a zero octet, the protocol number and the length, and including it is what
/// "gives the TCP connection protection against misrouted segments" -- a
/// segment that arrives from somewhere other than where it claims fails the
/// sum even though every octet of it is intact.
///
/// The sum itself is RFC 1071's, computed here rather than borrowed because
/// TCP is defined over any network layer and the thing it sums is not an IP
/// datagram.
pub fn checksum(from: [u8; 4], to: [u8; 4], segment: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut add = |data: &[u8]| {
        let (pairs, odd) = data.as_chunks::<2>();
        for pair in pairs {
            sum += u32::from(u16::from_be_bytes(*pair));
        }
        // "If a segment contains an odd number of header and text octets,
        // alignment can be achieved by padding the last octet with zeros on
        // its right... The pad is not transmitted as part of the segment."
        if let [last] = odd {
            sum += u32::from(u16::from_be_bytes([*last, 0]));
        }
    };
    add(&from);
    add(&to);
    add(&[0, PROTOCOL_TCP]);
    add(&(segment.len() as u16).to_be_bytes());
    add(segment);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(segment: &Segment) -> Segment {
        let bytes = segment.to_bytes([10, 0, 0, 1], [10, 0, 0, 2]);
        Segment::parse([10, 0, 0, 1], [10, 0, 0, 2], &bytes).expect("did not read back")
    }

    #[test]
    fn a_segment_reads_back_the_way_it_was_written() {
        let segment = Segment {
            source_port: 1080,
            destination_port: 40_000,
            sequence: 0xdead_beef,
            acknowledgment: 0x0bad_f00d,
            flags: flag::ACK | flag::PSH,
            window: 4096,
            urgent: 0,
            mss: None,
            payload: b"GET / HTTP/1.0\r\n\r\n".to_vec(),
        };
        assert_eq!(round_trip(&segment), segment);
    }

    /// A SYN carries the option, and 3.2 says exactly where in the header.
    #[test]
    fn a_syn_carries_its_maximum_segment_size() {
        let syn = Segment {
            source_port: 1,
            destination_port: 2,
            sequence: 100,
            flags: flag::SYN,
            window: 8192,
            mss: Some(1460),
            ..Segment::default()
        };
        let bytes = syn.to_bytes([1, 1, 1, 1], [2, 2, 2, 2]);
        assert_eq!(bytes.len(), 24, "the option did not go in");
        assert_eq!(bytes[12] >> 4, 6, "the data offset does not count it");
        assert_eq!(&bytes[20..24], &[option::MSS, 4, 0x05, 0xb4]);
        assert_eq!(round_trip(&syn).mss, Some(1460));
    }

    /// SEG.LEN counts the two controls that occupy sequence space, which is
    /// what lets them be acknowledged.
    #[test]
    fn a_syn_and_a_fin_each_take_a_place_in_the_sequence_space() {
        let empty = Segment::default();
        assert_eq!(empty.length(), 0);

        let syn = Segment { flags: flag::SYN, ..Segment::default() };
        assert_eq!(syn.length(), 1);

        let data_and_fin = Segment {
            flags: flag::FIN,
            payload: vec![0; 10],
            ..Segment::default()
        };
        assert_eq!(data_and_fin.length(), 11);

        // The only segment that carries both is one that opens and closes at
        // once, which is legal and pointless.
        let both = Segment {
            flags: flag::SYN | flag::FIN,
            ..Segment::default()
        };
        assert_eq!(both.length(), 2);
    }

    /// The pseudo-header is what makes a misrouted segment fail: every octet
    /// is intact and it is still not believed.
    #[test]
    fn a_segment_that_came_from_somewhere_else_is_not_believed() {
        let segment = Segment {
            source_port: 80,
            destination_port: 1024,
            sequence: 1,
            flags: flag::ACK,
            payload: b"hello".to_vec(),
            ..Segment::default()
        };
        let bytes = segment.to_bytes([10, 0, 0, 1], [10, 0, 0, 2]);
        assert!(Segment::parse([10, 0, 0, 1], [10, 0, 0, 2], &bytes).is_some());
        assert!(
            Segment::parse([10, 0, 0, 3], [10, 0, 0, 2], &bytes).is_none(),
            "it was accepted from the wrong sender"
        );
        assert!(
            Segment::parse([10, 0, 0, 1], [10, 0, 0, 9], &bytes).is_none(),
            "it was accepted for the wrong recipient"
        );
    }

    /// And a bit flipped anywhere in it fails too.
    #[test]
    fn a_damaged_segment_is_not_believed() {
        let segment = Segment {
            source_port: 1,
            destination_port: 2,
            sequence: 0x1234_5678,
            acknowledgment: 0x9abc_def0,
            flags: flag::ACK,
            window: 1000,
            payload: b"the quick brown fox".to_vec(),
            ..Segment::default()
        };
        let good = segment.to_bytes([10, 0, 0, 1], [10, 0, 0, 2]);
        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0x01;
            assert!(
                Segment::parse([10, 0, 0, 1], [10, 0, 0, 2], &bad).is_none(),
                "a flip at octet {i} was accepted"
            );
        }
    }

    /// An odd number of octets is summed as if padded, and the pad is not
    /// sent.
    #[test]
    fn an_odd_length_segment_still_checks_out() {
        let segment = Segment {
            source_port: 7,
            destination_port: 7,
            payload: b"odd".to_vec(),
            ..Segment::default()
        };
        let bytes = segment.to_bytes([1, 2, 3, 4], [5, 6, 7, 8]);
        assert_eq!(bytes.len(), HEADER_LEN + 3);
        assert_eq!(round_trip(&segment).payload, b"odd");
    }

    /// Options this end does not implement are walked past rather than
    /// stumbled over (MUST-6).
    #[test]
    fn an_unknown_option_is_ignored_and_the_next_one_is_still_read() {
        // A window scale (kind 3, length 3), a no-operation for alignment,
        // then the size. All three are legal and only the last is understood.
        assert_eq!(read_mss(&[3, 3, 7, option::NOP, option::MSS, 4, 0x05, 0xb4]), Some(1460));
        assert_eq!(read_mss(&[option::NOP, option::NOP, option::END]), None);
        assert_eq!(read_mss(&[]), None);
    }

    /// An option list that cannot be walked stops the walk rather than
    /// running off the end of it (MUST-7).
    #[test]
    fn an_illegal_option_length_is_survived() {
        assert_eq!(read_mss(&[3, 0, 0, 0]), None, "a zero length looped");
        assert_eq!(read_mss(&[3, 99]), None, "a length past the end was believed");
        assert_eq!(read_mss(&[option::MSS]), None, "a truncated option was read");
    }

    /// A header shorter than the fixed part, or one whose data offset points
    /// past the octets that arrived, is not a segment.
    #[test]
    fn a_header_that_does_not_fit_is_not_a_segment() {
        assert_eq!(Segment::parse([1, 1, 1, 1], [2, 2, 2, 2], &[0; 19]), None);
        let mut bytes = Segment::default().to_bytes([1, 1, 1, 1], [2, 2, 2, 2]);
        // Fifteen words of header on twenty octets of segment.
        bytes[12] = 0xf0;
        assert_eq!(Segment::parse([1, 1, 1, 1], [2, 2, 2, 2], &bytes), None);
    }
}
