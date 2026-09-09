//! IPv4, and just enough ICMP to send a ping and recognise the answer.
//!
//! Not a whole network layer: nothing here reassembles a fragment or holds a
//! route, because a link with one hop and a fixed pair of addresses has
//! neither to deal with. What it does is carry a datagram of any protocol in
//! either direction, and answer an echo, which between them are what it takes
//! to show a link carries IP and to put something useful on top of it.
//!
//! RFC 791 for the datagram, RFC 792 for the echo, RFC 1071 for the sum that
//! covers both.

/// RFC 792: the two message types this understands.
pub const ECHO_REPLY: u8 = 0;
pub const ECHO_REQUEST: u8 = 8;
/// RFC 790's protocol numbers, for the two that cross this link.
pub const PROTOCOL_ICMP: u8 = 1;
pub const PROTOCOL_TCP: u8 = 6;

/// The header this builds and the only length it accepts: five words, no
/// options.
pub const HEADER_LEN: usize = 20;

/// RFC 1071's internet checksum: the one's complement of the one's complement
/// sum of the data taken as sixteen-bit words.
///
/// The carries fold back in rather than being dropped, which is what makes it
/// one's complement arithmetic and what makes the sum of a block and its own
/// checksum come out as all ones.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let (pairs, odd) = data.as_chunks::<2>();
    for pair in pairs {
        sum += u32::from(u16::from_be_bytes(*pair));
    }
    // An odd length pads with a zero octet, which RFC 1071 4.1 allows: the
    // padding is not transmitted and does not change the sum.
    if let [last] = odd {
        sum += u32::from(u16::from_be_bytes([*last, 0]));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// One ICMP echo, either direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Echo {
    pub reply: bool,
    /// RFC 792: the two fields an echo carries back untouched, so a sender can
    /// tell its own pings apart.
    pub id: u16,
    pub sequence: u16,
    pub payload: Vec<u8>,
}

impl Echo {
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + 8);
        out.push(if self.reply { ECHO_REPLY } else { ECHO_REQUEST });
        out.push(0); // Code
        out.extend_from_slice(&[0, 0]); // Checksum, filled in below
        out.extend_from_slice(&self.id.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.payload);
        let sum = checksum(&out);
        out[2..4].copy_from_slice(&sum.to_be_bytes());
        out
    }

    /// Read one out of a datagram's payload.
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < 8 || checksum(body) != 0 {
            return None;
        }
        let reply = match body[0] {
            ECHO_REPLY => true,
            ECHO_REQUEST => false,
            _ => return None,
        };
        Some(Self {
            reply,
            id: u16::from_be_bytes([body[4], body[5]]),
            sequence: u16::from_be_bytes([body[6], body[7]]),
            payload: body[8..].to_vec(),
        })
    }

    /// The answer this echo deserves: RFC 792 has the reply carry the
    /// request's identifier, sequence and data back unchanged.
    pub fn to_reply(&self) -> Self {
        Self { reply: true, ..self.clone() }
    }
}

/// One datagram, whatever is inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub from: [u8; 4],
    pub to: [u8; 4],
    pub protocol: u8,
    pub payload: Vec<u8>,
}

/// Wrap a payload of any protocol in an IPv4 datagram.
pub fn build(from: [u8; 4], to: [u8; 4], protocol: u8, payload: &[u8], id: u16) -> Vec<u8> {
    let total = HEADER_LEN + payload.len();
    let mut out = Vec::with_capacity(total);
    // Version 4, header length 5 words: no options, which is every datagram
    // this sends.
    out.push(0x45);
    out.push(0); // Differentiated services, unused here
    out.extend_from_slice(&(total as u16).to_be_bytes());
    out.extend_from_slice(&id.to_be_bytes());
    // No fragmentation: Don't Fragment set, offset zero. A link with a 1500
    // octet MRU and a datagram built to fit it has nothing to fragment.
    out.extend_from_slice(&[0x40, 0x00]);
    out.push(64); // Time to live
    out.push(protocol);
    out.extend_from_slice(&[0, 0]); // Header checksum, filled in below
    out.extend_from_slice(&from);
    out.extend_from_slice(&to);
    let sum = checksum(&out[..HEADER_LEN]);
    out[10..12].copy_from_slice(&sum.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Wrap an ICMP echo in one.
pub fn datagram(from: [u8; 4], to: [u8; 4], echo: &Echo, id: u16) -> Vec<u8> {
    build(from, to, PROTOCOL_ICMP, &echo.to_bytes(), id)
}

/// What arrived, if it was an echo addressed to somebody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrived {
    pub from: [u8; 4],
    pub to: [u8; 4],
    pub echo: Echo,
}

/// Read a datagram, whatever protocol it carries.
///
/// A header this end cannot check, or a fragment, is not an error and not
/// this layer's business. It gives back nothing and the caller carries on.
pub fn read(datagram: &[u8]) -> Option<Carried> {
    if datagram.len() < HEADER_LEN || datagram[0] >> 4 != 4 {
        return None;
    }
    let header = usize::from(datagram[0] & 0x0f) * 4;
    if header < HEADER_LEN || datagram.len() < header {
        return None;
    }
    // RFC 791: the header checksum covers the header alone, and a good one
    // sums to zero including the field itself.
    if checksum(&datagram[..header]) != 0 {
        return None;
    }
    let total = usize::from(u16::from_be_bytes([datagram[2], datagram[3]]));
    if total < header || total > datagram.len() {
        return None;
    }
    // A datagram that is part of something larger cannot be read on its own.
    // Nothing here fragments and the link's MRU is larger than anything sent
    // over it, so one that arrives fragmented came from somewhere unexpected.
    let fragmented = datagram[6] & 0x1f != 0 || datagram[7] != 0;
    if fragmented {
        return None;
    }
    Some(Carried {
        from: datagram[12..16].try_into().ok()?,
        to: datagram[16..20].try_into().ok()?,
        protocol: datagram[9],
        payload: datagram[header..total].to_vec(),
    })
}

/// Read a datagram, taking only an echo out of it.
pub fn parse(datagram: &[u8]) -> Option<Arrived> {
    let carried = read(datagram)?;
    if carried.protocol != PROTOCOL_ICMP {
        return None;
    }
    Some(Arrived {
        from: carried.from,
        to: carried.to,
        echo: Echo::parse(&carried.payload)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1071 3: the sum of a block and its own checksum is all ones, so
    /// checking is the same operation as computing.
    #[test]
    fn a_block_and_its_own_checksum_sum_to_nothing() {
        let mut block = vec![0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06];
        let sum = checksum(&block);
        block.extend_from_slice(&sum.to_be_bytes());
        assert_eq!(checksum(&block), 0);
    }

    /// The worked example in RFC 1071 3, digit for digit.
    #[test]
    fn the_documents_own_example_comes_out() {
        // "the 16-bit 1's complement sum of 00 01 f2 03 f4 f5 f6 f7" is
        // ddf2, so the checksum is its complement.
        let sum = checksum(&[0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7]);
        assert_eq!(sum, 0x220d, "got {sum:04x}");
        assert_eq!(!sum, 0xddf2);
    }

    /// An odd number of octets pads with a zero that is not sent.
    #[test]
    fn an_odd_length_is_summed_as_if_padded() {
        assert_eq!(checksum(&[0x12, 0x34, 0x56]), checksum(&[0x12, 0x34, 0x56, 0x00]));
    }

    #[test]
    fn a_ping_reads_back_as_it_was_built() {
        let echo = Echo {
            reply: false,
            id: 0x1234,
            sequence: 7,
            payload: b"binmodem".to_vec(),
        };
        let bytes = datagram([10, 0, 0, 1], [10, 0, 0, 2], &echo, 99);
        let got = parse(&bytes).expect("did not read back");
        assert_eq!(got.from, [10, 0, 0, 1]);
        assert_eq!(got.to, [10, 0, 0, 2]);
        assert_eq!(got.echo, echo);
    }

    /// And the reply carries the identifier, sequence and data back.
    #[test]
    fn a_reply_is_the_request_turned_round() {
        let echo = Echo { reply: false, id: 1, sequence: 2, payload: vec![3, 4] };
        let reply = echo.to_reply();
        assert!(reply.reply);
        assert_eq!((reply.id, reply.sequence, &reply.payload), (1, 2, &vec![3, 4]));
        let bytes = datagram([1, 1, 1, 1], [2, 2, 2, 2], &reply, 1);
        assert!(parse(&bytes).unwrap().echo.reply);
    }

    /// A datagram with a bit flipped anywhere in its header is not read.
    #[test]
    fn a_damaged_header_is_not_believed() {
        let echo = Echo { reply: false, id: 1, sequence: 1, payload: vec![0; 32] };
        let good = datagram([10, 0, 0, 1], [10, 0, 0, 2], &echo, 1);
        for i in 0..20 {
            let mut bad = good.clone();
            bad[i] ^= 0x01;
            assert!(parse(&bad).is_none(), "a flip at octet {i} was accepted");
        }
    }

    /// And one with a bit flipped in the echo itself.
    #[test]
    fn a_damaged_echo_is_not_believed() {
        let echo = Echo { reply: false, id: 1, sequence: 1, payload: vec![7; 16] };
        let good = datagram([10, 0, 0, 1], [10, 0, 0, 2], &echo, 1);
        for i in 20..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0x80;
            assert!(parse(&bad).is_none(), "a flip at octet {i} was accepted");
        }
    }

    /// Anything that is not an echo is not this layer's business.
    #[test]
    fn other_traffic_is_left_alone() {
        let mut udp = datagram([1, 1, 1, 1], [2, 2, 2, 2], &Echo {
            reply: false,
            id: 0,
            sequence: 0,
            payload: vec![],
        }, 1);
        udp[9] = 17; // UDP
        udp[10..12].copy_from_slice(&[0, 0]);
        let sum = checksum(&udp[..20]);
        udp[10..12].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(parse(&udp), None, "the echo reader took something else");
        // It is still a datagram, and the layer that wants it can have it.
        assert_eq!(read(&udp).unwrap().protocol, 17);
    }

    /// Anything at all crosses, which is what makes something other than a
    /// ping possible on top.
    #[test]
    fn a_payload_of_any_protocol_reads_back() {
        let segment = b"not really a TCP segment, but it is opaque here";
        let bytes = build([10, 0, 0, 1], [10, 0, 0, 2], PROTOCOL_TCP, segment, 42);
        let got = read(&bytes).expect("did not read back");
        assert_eq!(got.protocol, PROTOCOL_TCP);
        assert_eq!(got.from, [10, 0, 0, 1]);
        assert_eq!(got.to, [10, 0, 0, 2]);
        assert_eq!(got.payload, segment);
    }

    /// And a header damaged on the way is still not believed.
    #[test]
    fn a_damaged_header_is_not_believed_whatever_it_carries() {
        let good = build([10, 0, 0, 1], [10, 0, 0, 2], PROTOCOL_TCP, &[0; 40], 1);
        for i in 0..HEADER_LEN {
            let mut bad = good.clone();
            bad[i] ^= 0x01;
            assert!(read(&bad).is_none(), "a flip at octet {i} was accepted");
        }
    }
}
