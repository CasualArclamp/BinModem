//! RTP: RFC 3550 5.1's fixed header, and the stream that fills one in.
//!
//! This is the whole of the packet layer. It parses a datagram into a payload
//! and the four fields that say where the payload belongs -- payload type,
//! sequence, timestamp, SSRC -- and it builds one going the other way. It does
//! not reorder, does not conceal, does not time anything: that is [`jitter`]'s
//! job, and keeping the two apart is what makes either of them testable.
//!
//! What is deliberately missing. No RTCP is sent: RFC 3550 6 wants reports,
//! and a trunk that is carrying one modem call for one user agent has nothing
//! to do with them except count. What is here instead is [`is_rtcp`], because
//! a far end that does not honour the port pair will send its reports to the
//! RTP port, and a report decoded as G.711 is a burst of loud noise straight
//! into the modem. Recognising them is not politeness, it is the difference
//! between a call that stays up and one that does not.
//!
//! Nothing here generates padding, a contributing source list or a header
//! extension -- there is one source, the payload is already octet-aligned, and
//! no profile in use has an extension. All three are *parsed*, because what a
//! far end sends is not this end's choice, and a payload read at the wrong
//! offset is not audio.
//!
//! [`jitter`]: crate::jitter
//! [`is_rtcp`]: is_rtcp

use crate::rand;

/// 5.1's fixed header: two octets of flags and type, then sequence, timestamp
/// and synchronisation source.
pub const HEADER_LEN: usize = 12;

/// "This memorandum defines RTP version 2" (5.1). Version 1 was a draft and
/// version 0 an experiment; neither is on a telephone trunk, and a datagram
/// claiming either is something else that arrived on this port.
const VERSION: u8 = 2;

/// RFC 3550 5.1's fixed header, and what follows it.
///
/// The header's own flags are not fields here. Padding has been trimmed off
/// the payload, an extension has been skipped, and a contributing source list
/// belongs to a mixer and there is no mixer in this path -- so by the time a
/// packet is one of these, all three have been dealt with and what is left is
/// the audio.
#[derive(Debug, Clone)]
pub struct Packet {
    /// RFC 3551's static type number: 0 for mu-law, 8 for A-law. Seven bits,
    /// so the marker bit above it is never in here.
    pub payload_type: u8,
    /// 5.1: "for audio it marks the first packet of a talkspurt".
    pub marker: bool,
    pub sequence: u16,
    /// In sampling instants, which for G.711 is one per payload octet.
    pub timestamp: u32,
    pub ssrc: u32,
    /// The codewords themselves, padding removed.
    pub payload: Vec<u8>,
}

impl Packet {
    /// None when the datagram is not RTP at all: wrong version, too short,
    /// a header that claims more than arrived.
    ///
    /// Every one of those is something that happens on a real trunk -- a
    /// stray RTCP report, a probe, a packet cut short by a middlebox -- so
    /// none of them is a panic, and none of them is a short payload quietly
    /// handed on as if it were audio. A payload that is not all there is
    /// worse than no payload: the modem cannot tell it apart from the line.
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        if datagram.len() < HEADER_LEN {
            return None;
        }
        let first = datagram[0];
        if first >> 6 != VERSION {
            return None;
        }
        let padded = first & 0x20 != 0;
        let extended = first & 0x10 != 0;
        let csrc_count = usize::from(first & 0x0F);

        let second = datagram[1];
        let marker = second & 0x80 != 0;
        let payload_type = second & 0x7F;

        // The contributing sources a mixer would have listed. Skipped rather
        // than read: this end is not a mixer and has nothing to do with them.
        let mut start = HEADER_LEN + csrc_count * 4;
        if datagram.len() < start {
            return None;
        }

        if extended {
            // 5.3.1: sixteen bits the profile defines, then a length "in
            // 32-bit units, not including the four-octet extension header".
            if datagram.len() < start + 4 {
                return None;
            }
            let words = usize::from(u16::from_be_bytes([datagram[start + 2], datagram[start + 3]]));
            start += 4 + words * 4;
            if datagram.len() < start {
                return None;
            }
        }

        let mut end = datagram.len();
        if padded {
            // 5.1: "the last octet of the padding contains a count of how many
            // padding octets should be ignored, including itself". So zero is
            // not a legal count -- the counting octet is itself padding -- and
            // a count that reaches back into the header is a lie.
            let count = usize::from(datagram[end - 1]);
            if count == 0 || end < start + count {
                return None;
            }
            end -= count;
        }

        Some(Self {
            payload_type,
            marker,
            sequence: u16::from_be_bytes([datagram[2], datagram[3]]),
            timestamp: u32::from_be_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]),
            ssrc: u32::from_be_bytes([datagram[8], datagram[9], datagram[10], datagram[11]]),
            payload: datagram[start..end].to_vec(),
        })
    }

    /// Write the packet into `out`, which is cleared first.
    ///
    /// One packet is one datagram, so there is nothing to append to and a
    /// buffer handed in with something already in it is a mistake rather than
    /// a compound packet. The buffer is handed in at all so that a call that
    /// sends fifty packets a second for an hour allocates once.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(HEADER_LEN + self.payload.len());
        // Version 2, and no padding, no extension, no contributing sources.
        out.push(VERSION << 6);
        out.push((u8::from(self.marker) << 7) | (self.payload_type & 0x7F));
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.ssrc.to_be_bytes());
        out.extend_from_slice(&self.payload);
    }
}

/// The stream we send: one SSRC, a sequence and a timestamp that only ever
/// go forwards.
///
/// One source, so no mixing and no contributing sources; and no collision
/// detection either, which 8.2 wants -- it is worth having when an SSRC is
/// shared out among a conference, and there is no conference here, only a
/// point-to-point call over a trunk.
#[derive(Debug)]
pub struct Stream {
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    packets: u64,
    octets: u64,
}

impl Stream {
    /// Sequence and timestamp start somewhere unpredictable, as 5.1 asks.
    ///
    /// The reason given there is that the initial values should not be
    /// guessable if the stream is ever encrypted, which this one is not. It
    /// is worth doing anyway for a duller reason: a far end that has just
    /// seen a previous call from this host must not be able to mistake the
    /// new stream's packets for stale ones from the old, and starting both at
    /// zero is the way to make exactly that happen.
    pub fn new(payload_type: u8) -> Self {
        let seed = rand::number();
        Self {
            payload_type: payload_type & 0x7F,
            ssrc: rand::number() as u32,
            sequence: seed as u16,
            timestamp: (seed >> 16) as u32,
            packets: 0,
            octets: 0,
        }
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    pub fn payload_type(&self) -> u8 {
        self.payload_type
    }

    /// Build the next packet into `out` (cleared first). `marker` is set on
    /// the first packet of a talkspurt; for a modem there is one talkspurt.
    /// The timestamp advances by the number of samples in the payload, which
    /// for G.711 is one per octet.
    ///
    /// Both counters wrap rather than saturate, which is the protocol's own
    /// arithmetic: a sequence wraps every 65536 packets -- about twenty-two
    /// minutes at 20 ms -- and a timestamp every six hours or so. A call runs
    /// straight through both.
    pub fn next(&mut self, payload: &[u8], marker: bool, out: &mut Vec<u8>) {
        let packet = Packet {
            payload_type: self.payload_type,
            marker,
            sequence: self.sequence,
            timestamp: self.timestamp,
            ssrc: self.ssrc,
            // Borrowed rather than copied would be neater; the payload is 160
            // octets fifty times a second, which is not where this crate's
            // time goes.
            payload: payload.to_vec(),
        };
        packet.write(out);
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(payload.len() as u32);
        self.packets += 1;
        self.octets += payload.len() as u64;
    }

    pub fn packets_sent(&self) -> u64 {
        self.packets
    }

    /// Payload octets only, which is what 6.4.1's "sender's octet count"
    /// means: headers and padding are not what was carried.
    pub fn octets_sent(&self) -> u64 {
        self.octets
    }
}

/// A datagram that is RTCP rather than RTP, which arrives on the RTP port
/// when a far end does not honour the port pair. Recognised so it can be
/// dropped without being decoded as audio.
///
/// The test is the one RFC 5761 4 states for telling the two apart in a single
/// stream: RTCP's packet type occupies the whole second octet, and the values
/// assigned to it are 192 to 223, which as an RTP payload type would be 64 to
/// 95 with the marker bit set. RFC 3551 6 keeps those payload types unassigned
/// for precisely this reason. No payload this modem will ever offer is in
/// that range -- G.711 is 0 and 8 -- so the test cannot misread audio as a
/// report.
pub fn is_rtcp(datagram: &[u8]) -> bool {
    // A report is at least a four-octet header and the SSRC that follows it.
    datagram.len() >= 8 && datagram[0] >> 6 == VERSION && (192..=223).contains(&datagram[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::g711::{PCMA, PCMU};

    fn sample_packet() -> Packet {
        Packet {
            payload_type: PCMU,
            marker: true,
            sequence: 0x1234,
            timestamp: 0xdead_beef,
            ssrc: 0x0bad_f00d,
            payload: (0..160u16).map(|n| n as u8).collect(),
        }
    }

    #[test]
    fn a_packet_reads_back_the_way_it_was_written() {
        let packet = sample_packet();
        let mut bytes = Vec::new();
        packet.write(&mut bytes);
        assert_eq!(bytes.len(), HEADER_LEN + 160);
        let read = Packet::parse(&bytes).expect("did not read back");
        assert_eq!(read.payload_type, packet.payload_type);
        assert!(read.marker);
        assert_eq!(read.sequence, packet.sequence);
        assert_eq!(read.timestamp, packet.timestamp);
        assert_eq!(read.ssrc, packet.ssrc);
        assert_eq!(read.payload, packet.payload);
    }

    /// The marker is the top bit of the second octet and the payload type is
    /// the seven under it, so a type with its own top bit set must not leak
    /// into the flag.
    #[test]
    fn the_marker_bit_is_not_part_of_the_payload_type() {
        let mut bytes = Vec::new();
        Packet { marker: false, ..sample_packet() }.write(&mut bytes);
        assert_eq!(bytes[1], PCMU, "a quiet marker still set a bit");
        assert!(!Packet::parse(&bytes).unwrap().marker);

        let mut loud = Vec::new();
        Packet { marker: true, payload_type: PCMA, ..sample_packet() }.write(&mut loud);
        assert_eq!(loud[1], 0x80 | PCMA);
        let read = Packet::parse(&loud).unwrap();
        assert!(read.marker);
        assert_eq!(read.payload_type, PCMA);
    }

    /// 5.1's padding: the count is in the last octet and includes itself.
    #[test]
    fn padding_is_trimmed_off_the_payload() {
        let mut bytes = Vec::new();
        Packet { payload: vec![1, 2, 3], ..sample_packet() }.write(&mut bytes);
        bytes[0] |= 0x20;
        bytes.extend_from_slice(&[0, 0, 0, 4]);
        let read = Packet::parse(&bytes).expect("did not read back");
        assert_eq!(read.payload, vec![1, 2, 3]);

        // A count of zero would mean padding that does not include the octet
        // that counts it, which 5.1 does not allow and which a reader that
        // subtracted it blindly would accept.
        let mut zero = bytes.clone();
        *zero.last_mut().unwrap() = 0;
        assert!(Packet::parse(&zero).is_none());

        // And a count that reaches back past the payload into the header.
        let mut greedy = bytes.clone();
        *greedy.last_mut().unwrap() = 200;
        assert!(Packet::parse(&greedy).is_none());
    }

    /// A mixer's contributing source list sits between the header and the
    /// payload, and the count of them is the bottom four bits of octet zero.
    #[test]
    fn a_contributing_source_list_is_skipped() {
        let mut bytes = Vec::new();
        Packet { payload: vec![9, 9, 9], ..sample_packet() }.write(&mut bytes);
        let mut with_csrc = bytes[..HEADER_LEN].to_vec();
        with_csrc[0] = (VERSION << 6) | 2;
        with_csrc.extend_from_slice(&[0xaa; 4]);
        with_csrc.extend_from_slice(&[0xbb; 4]);
        with_csrc.extend_from_slice(&[9, 9, 9]);
        assert_eq!(Packet::parse(&with_csrc).unwrap().payload, vec![9, 9, 9]);

        // Two sources claimed and one delivered.
        assert!(Packet::parse(&with_csrc[..HEADER_LEN + 4]).is_none());
    }

    /// 5.3.1's extension: a profile-defined half-word, a length in 32-bit
    /// words that does not count the four octets of the header itself, then
    /// the words.
    #[test]
    fn a_header_extension_is_skipped() {
        let mut bytes = Vec::new();
        Packet { payload: vec![7, 7], ..sample_packet() }.write(&mut bytes);
        let mut with_ext = bytes[..HEADER_LEN].to_vec();
        with_ext[0] = (VERSION << 6) | 0x10;
        with_ext.extend_from_slice(&[0xbe, 0xde, 0x00, 0x02]);
        with_ext.extend_from_slice(&[0x11; 8]);
        with_ext.extend_from_slice(&[7, 7]);
        assert_eq!(Packet::parse(&with_ext).unwrap().payload, vec![7, 7]);

        // A length longer than what arrived. Read without checking, this is
        // a slice past the end of the datagram.
        let mut lying = with_ext.clone();
        lying[15] = 0xff;
        assert!(Packet::parse(&lying).is_none());
    }

    /// Both at once, which is the case where getting one offset wrong still
    /// leaves a payload of the right length and the wrong contents.
    #[test]
    fn padding_and_an_extension_together_still_leave_the_payload() {
        let mut bytes = vec![(VERSION << 6) | 0x20 | 0x10 | 1, PCMU];
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[0xcc; 4]); // one contributing source
        bytes.extend_from_slice(&[0xbe, 0xde, 0x00, 0x01]); // one word of extension
        bytes.extend_from_slice(&[0xee; 4]);
        bytes.extend_from_slice(&[0x55, 0x66]); // the audio
        bytes.extend_from_slice(&[0, 0, 3]); // three octets of padding, counting the count
        assert_eq!(Packet::parse(&bytes).unwrap().payload, vec![0x55, 0x66]);
    }

    #[test]
    fn a_short_or_wrong_version_datagram_is_not_a_packet() {
        assert!(Packet::parse(&[]).is_none());
        assert!(Packet::parse(&[0x80; HEADER_LEN - 1]).is_none());
        // Exactly a header and nothing else is legal, and is a packet with no
        // audio in it rather than a malformed one.
        assert_eq!(Packet::parse(&[0x80; HEADER_LEN]).unwrap().payload.len(), 0);

        for version in [0u8, 1, 3] {
            let mut bytes = Vec::new();
            sample_packet().write(&mut bytes);
            bytes[0] = version << 6;
            assert!(Packet::parse(&bytes).is_none(), "version {version} was read as RTP");
        }
    }

    #[test]
    fn a_stream_numbers_its_packets_and_counts_what_it_sent() {
        let mut stream = Stream::new(PCMU);
        let mut out = Vec::new();
        let payload = [0xffu8; 160];

        stream.next(&payload, true, &mut out);
        let first = Packet::parse(&out).unwrap();
        assert!(first.marker, "the talkspurt did not start");
        assert_eq!(first.ssrc, stream.ssrc());

        stream.next(&payload, false, &mut out);
        let second = Packet::parse(&out).unwrap();
        assert!(!second.marker);
        assert_eq!(second.sequence, first.sequence.wrapping_add(1));
        // One timestamp tick per sample, and one sample per octet.
        assert_eq!(second.timestamp, first.timestamp.wrapping_add(160));
        assert_eq!(second.ssrc, first.ssrc, "the source changed mid-call");

        assert_eq!(stream.packets_sent(), 2);
        assert_eq!(stream.octets_sent(), 320);
    }

    /// Twenty-two minutes into a call the sequence wraps, and the packet
    /// after 65535 is 0. Nothing may stumble over that, here or in the
    /// buffer at the other end.
    #[test]
    fn a_stream_runs_through_the_sequence_wrapping() {
        let mut stream = Stream::new(PCMU);
        stream.sequence = 65534;
        let mut out = Vec::new();
        let mut seen = Vec::new();
        for _ in 0..4 {
            stream.next(&[0; 160], false, &mut out);
            seen.push(Packet::parse(&out).unwrap().sequence);
        }
        assert_eq!(seen, vec![65534, 65535, 0, 1]);
    }

    /// Two streams started back to back must not look like one stream, which
    /// is the whole point of 5.1's random start.
    #[test]
    fn two_streams_do_not_start_in_the_same_place() {
        let a = Stream::new(PCMU);
        let b = Stream::new(PCMU);
        assert_ne!(a.ssrc(), b.ssrc());
        assert_ne!((a.sequence, a.timestamp), (b.sequence, b.timestamp));
    }

    /// A report that arrives on the audio port is loud noise if it is decoded
    /// as audio, so it has to be recognised before anything else looks at it.
    #[test]
    fn a_report_that_came_to_the_wrong_port_is_recognised() {
        // A sender report: version 2, no sources, type 200, one word of
        // length, then the SSRC.
        let sr = [0x80u8, 200, 0x00, 0x06, 0xde, 0xad, 0xbe, 0xef];
        assert!(is_rtcp(&sr));
        // A receiver report and a goodbye, the other two a trunk sends.
        assert!(is_rtcp(&[0x81u8, 201, 0, 7, 1, 2, 3, 4]));
        assert!(is_rtcp(&[0x81u8, 203, 0, 1, 1, 2, 3, 4]));

        // And audio is not one, either law.
        let mut bytes = Vec::new();
        sample_packet().write(&mut bytes);
        assert!(!is_rtcp(&bytes));
        Packet { payload_type: PCMA, ..sample_packet() }.write(&mut bytes);
        assert!(!is_rtcp(&bytes));
        // Nor is anything too short to be either.
        assert!(!is_rtcp(&[0x80, 200]));
    }
}
