//! The compressor and the decompressor against each other, and against the
//! header shapes RFC 1144 3.2.2 says its own traffic produces.

use super::*;

const HERE: [u8; 4] = [10, 0, 0, 2];
const THERE: [u8; 4] = [10, 0, 0, 1];

/// One TCP segment inside one datagram, with everything nameable.
#[derive(Debug, Clone, Copy)]
struct Seg {
    sport: u16,
    dport: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    urgent: u16,
    sum: u16,
    id: u16,
}

impl Seg {
    /// The ordinary case: an acknowledging segment on the usual connection.
    fn new(seq: u32, ack: u32, id: u16) -> Self {
        Self {
            sport: 1234,
            dport: 80,
            seq,
            ack,
            flags: ACK,
            window: 4096,
            urgent: 0,
            sum: 0xbeef,
            id,
        }
    }

    fn build(&self, data: &[u8]) -> Vec<u8> {
        let mut tcp = Vec::with_capacity(20 + data.len());
        tcp.extend_from_slice(&self.sport.to_be_bytes());
        tcp.extend_from_slice(&self.dport.to_be_bytes());
        tcp.extend_from_slice(&self.seq.to_be_bytes());
        tcp.extend_from_slice(&self.ack.to_be_bytes());
        // Five words of header, no options.
        tcp.push(5 << 4);
        tcp.push(self.flags);
        tcp.extend_from_slice(&self.window.to_be_bytes());
        tcp.extend_from_slice(&self.sum.to_be_bytes());
        tcp.extend_from_slice(&self.urgent.to_be_bytes());
        tcp.extend_from_slice(data);
        ip::build(HERE, THERE, ip::PROTOCOL_TCP, &tcp, self.id)
    }
}

fn pair() -> (Compressor, Decompressor) {
    (
        Compressor::new(Params::DEFAULT),
        Decompressor::new(Params::DEFAULT),
    )
}

/// Compress and decompress one datagram, asserting it survives, and give back
/// what went on the line.
fn across(c: &mut Compressor, d: &mut Decompressor, datagram: &[u8]) -> (Kind, Vec<u8>) {
    let (kind, wire) = c.compress(datagram);
    let back = d.decompress(kind, &wire).expect("nothing came back");
    assert_eq!(back, datagram, "what came back was not what went in");
    (kind, wire)
}

#[test]
fn the_first_packet_on_a_connection_goes_whole() {
    // 3.2.3: with no state matching the packet, "an UNCOMPRESSED_TCP packet is
    // sent" -- which is how the far end learns the connection number.
    let (mut c, mut d) = pair();
    let one = Seg::new(1000, 2000, 100).build(b"hello");
    let (kind, wire) = across(&mut c, &mut d, &one);
    assert_eq!(kind, Kind::Uncompressed);
    assert_eq!(kind.protocol(), 0x002f, "RFC 1332 4 names this one");
    assert_eq!(wire.len(), one.len(), "it is the same packet, not a smaller one");
    // "The IP protocol field (byte 9) is changed from 6 to a connection
    // number." Slot zero, being the first.
    assert_eq!(wire[9], 0);
    assert_eq!(one[9], ip::PROTOCOL_TCP);
}

/// 3.2.2: "compressed terminal traffic usually looks like (in hex): 0B c c d,
/// where the 0B indicates case (1), c c is the two byte TCP checksum and d is
/// the character typed."
#[test]
fn echoed_terminal_traffic_is_the_four_octets_the_document_describes() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let typed = Seg::new(1001, 2001, 101).build(b"b");
    let (kind, wire) = across(&mut c, &mut d, &typed);
    assert_eq!(kind, Kind::Compressed);
    assert_eq!(wire, vec![0x0b, 0xbe, 0xef, b'b'], "got {wire:02x?}");
    assert_eq!(
        typed.len() - wire.len(),
        41 - 4,
        "forty-one octets should have become four"
    );
}

/// The other special case: "packets in the data transfer direction of an FTP
/// put or get look like 0F c c d ...".
#[test]
fn a_one_way_transfer_is_the_other_special_case() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"wxyz"));
    let more = Seg::new(1004, 2000, 101).build(b"abcd");
    let (kind, wire) = across(&mut c, &mut d, &more);
    assert_eq!(kind, Kind::Compressed);
    assert_eq!(wire, vec![0x0f, 0xbe, 0xef, b'a', b'b', b'c', b'd'], "got {wire:02x?}");
}

/// "Acks for that FTP look like 04 c c a where a is the amount of data being
/// acked."
#[test]
fn an_acknowledgement_on_its_own_is_four_octets() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b""));
    let acking = Seg::new(1000, 2050, 101).build(b"");
    let (kind, wire) = across(&mut c, &mut d, &acking);
    assert_eq!(kind, Kind::Compressed);
    assert_eq!(wire, vec![0x04, 0xbe, 0xef, 50], "got {wire:02x?}");
}

/// A whole conversation, compared octet for octet at the far end.
#[test]
fn a_stream_of_segments_comes_back_exactly() {
    let (mut c, mut d) = pair();
    let mut seq = 1000u32;
    let mut ack = 2000u32;
    let mut id = 100u16;
    let mut compressed = 0;
    let mut saved = 0usize;
    for i in 0..200u32 {
        let data: Vec<u8> = (0..(i % 37) as u8).collect();
        let mut seg = Seg::new(seq, ack, id);
        // A window that opens now and then, and a push on the short ones.
        if i % 17 == 0 {
            seg.window = 4096 + (i as u16);
        }
        if data.len() < 8 {
            seg.flags |= PSH;
        }
        seg.sum = (0x1000 + i) as u16;
        let datagram = seg.build(&data);
        let (kind, wire) = across(&mut c, &mut d, &datagram);
        if kind == Kind::Compressed {
            compressed += 1;
            saved += datagram.len() - wire.len();
        }
        seq = seq.wrapping_add(data.len() as u32);
        ack = ack.wrapping_add(if i % 3 == 0 { 40 } else { 0 });
        id = id.wrapping_add(1);
    }
    assert!(compressed > 190, "only {compressed} of 200 compressed");
    // Forty octets a header, nearly all of it gone.
    assert!(saved > 190 * 33, "only {saved} octets saved");
}

/// 3.2.3: SYN, FIN and RST are "uncompressible" and the ACK test goes with
/// them. All four leave the compressor's state alone.
#[test]
fn a_connection_opening_or_closing_is_not_compressed() {
    for flags in [SYN, SYN | ACK, FIN | ACK, RST, 0] {
        let (mut c, mut d) = pair();
        let mut seg = Seg::new(1000, 2000, 100);
        seg.flags = flags;
        let (kind, wire) = across(&mut c, &mut d, &seg.build(b""));
        assert_eq!(kind, Kind::Ip, "flags {flags:02x} were compressed");
        assert_eq!(wire[9], ip::PROTOCOL_TCP, "the datagram was altered");
    }
}

/// "If the packet is not protocol TCP, send it as TYPE_IP", and the state is
/// not touched -- so a ping between two segments changes nothing.
#[test]
fn something_that_is_not_tcp_is_left_alone() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let echo = ip::Echo { reply: false, id: 7, sequence: 1, payload: vec![0; 56] };
    let ping = ip::datagram(HERE, THERE, &echo, 500);
    let (kind, wire) = across(&mut c, &mut d, &ping);
    assert_eq!(kind, Kind::Ip);
    assert_eq!(wire, ping);
    // And the connection carries on as though it had not happened.
    let (kind, wire) = across(&mut c, &mut d, &Seg::new(1001, 2001, 101).build(b"b"));
    assert_eq!(kind, Kind::Compressed);
    assert_eq!(wire[0], 0x0b, "the ping disturbed the state");
}

/// "Only the first fragment contains the TCP header... it seems reasonable to
/// send all IP fragments uncompressed."
#[test]
fn a_fragment_is_left_alone() {
    let (mut c, mut d) = pair();
    let mut more_fragments = Seg::new(1000, 2000, 100).build(b"abcdefgh");
    more_fragments[6] |= 0x20;
    assert_eq!(c.compress(&more_fragments).0, Kind::Ip);

    let mut offset = Seg::new(1000, 2000, 100).build(b"abcdefgh");
    offset[7] = 4;
    assert_eq!(c.compress(&offset).0, Kind::Ip);
    let _ = &mut d;
}

/// 4.1: after a frame the link could not read, "packets are discarded until
/// the receiver gets an explicit connection number".
///
/// Without this the changes in the next packet are applied to whichever
/// conversation went last, and the TCP checksum has one chance in 65536 of
/// letting it through.
#[test]
fn a_damaged_frame_makes_the_far_end_wait_for_a_connection_number() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));

    // The framer saw a bad check sequence.
    d.error();
    assert!(d.tossing());

    // The next compressed packet has no connection number, so it goes.
    let (kind, wire) = c.compress(&Seg::new(1001, 2001, 101).build(b"b"));
    assert_eq!(kind, Kind::Compressed);
    assert_eq!(wire[0] & 0x40, 0, "the compressor named the connection anyway");
    assert_eq!(d.decompress(kind, &wire), None, "it was believed");
    assert!(d.tossing(), "it stopped tossing without being told the connection");

    // An uncompressed packet says which connection, and things go on.
    let recovered = Seg::new(1002, 2002, 102).build(b"c");
    let (kind, wire) = c.compress(&recovered);
    assert_eq!(kind, Kind::Compressed, "this one is still compressible");
    // It is not believed either, because the compressor does not know. What
    // fixes it is the far end's TCP giving up and retransmitting, which goes
    // backwards and so cannot be compressed.
    assert_eq!(d.decompress(kind, &wire), None);
    let retransmit = Seg::new(1001, 2002, 103).build(b"b");
    let (kind, wire) = c.compress(&retransmit);
    assert_eq!(kind, Kind::Uncompressed, "a sequence number that went back was compressed");
    assert_eq!(d.decompress(kind, &wire), Some(retransmit));
    assert!(!d.tossing(), "it is still throwing packets away");
}

/// 3.2.3: "a negative sequence number change probably indicates a
/// retransmission. Since this may be due to the decompressor having dropped a
/// packet, an uncompressed packet is sent to re-sync the decompressor."
#[test]
fn a_sequence_number_that_goes_backwards_resynchronises_the_far_end() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(10_000, 2000, 100).build(b"abcd"));
    across(&mut c, &mut d, &Seg::new(10_004, 2000, 101).build(b"efgh"));
    let (kind, _) = across(&mut c, &mut d, &Seg::new(10_000, 2000, 102).build(b"abcd"));
    assert_eq!(kind, Kind::Uncompressed);
}

/// And a jump forward of more than 64K cannot be encoded either.
#[test]
fn a_jump_too_large_to_encode_goes_whole() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let (kind, _) = across(&mut c, &mut d, &Seg::new(1000 + 70_000, 2000, 101).build(b"b"));
    assert_eq!(kind, Kind::Uncompressed);
}

/// The C bit: "if C is clear, the connection is assumed to be the same as for
/// the last compressed or uncompressed packet."
#[test]
fn the_connection_number_is_sent_only_when_it_changes() {
    let (mut c, mut d) = pair();
    let other = |seq: u32, ack: u32, id: u16| {
        let mut s = Seg::new(seq, ack, id);
        s.sport = 5555;
        s
    };
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    across(&mut c, &mut d, &other(500, 600, 200).build(b"z"));

    // Back to the first: the number has to be said again.
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1001, 2001, 101).build(b"b"));
    assert_ne!(wire[0] & 0x40, 0, "the connection changed and was not named");
    assert_eq!(wire[1], 0, "the first connection took slot zero");

    // And again on it, the number is left out.
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1002, 2002, 102).build(b"c"));
    assert_eq!(wire[0] & 0x40, 0, "the number was sent for no reason");
    assert_eq!(wire, vec![0x0b, 0xbe, 0xef, b'c']);
}

/// Comp-Slot-Id of zero (RFC 1332 4.1): "all compressed TCP packets must set
/// the C bit in every change mask, and must include the slot identifier".
#[test]
fn a_far_end_that_will_not_have_the_number_left_out_is_obeyed() {
    let params = Params { max_slot: 15, compress_slot: false };
    let mut c = Compressor::new(params);
    let mut d = Decompressor::new(params);
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1001, 2001, 101).build(b"b"));
    assert_ne!(wire[0] & 0x40, 0, "the number was left out");
    assert_eq!(wire, vec![0x4b, 0x00, 0xbe, 0xef, b'b']);
}

/// More conversations than slots: the one used longest ago gives way, and the
/// one that took its place is announced uncompressed.
#[test]
fn more_connections_than_slots_reuses_the_one_used_longest_ago() {
    let params = Params { max_slot: 2, compress_slot: true };
    let mut c = Compressor::new(params);
    let mut d = Decompressor::new(params);
    let at = |port: u16, seq: u32, id: u16| {
        let mut s = Seg::new(seq, 2000, id);
        s.sport = port;
        s
    };
    for (i, port) in [7001u16, 7002, 7003].iter().enumerate() {
        let (kind, _) = across(&mut c, &mut d, &at(*port, 1000, 100 + i as u16).build(b"a"));
        assert_eq!(kind, Kind::Uncompressed, "port {port} should have been new");
    }
    // Touch the first two so the third is the oldest.
    across(&mut c, &mut d, &at(7001, 1001, 110).build(b"b"));
    across(&mut c, &mut d, &at(7002, 1001, 111).build(b"b"));
    // A fourth arrives and something has to give. 3.2.3 says which: "some
    // state is reclaimed (which should probably be the least recently used)".
    let (kind, _) = across(&mut c, &mut d, &at(7004, 1000, 112).build(b"a"));
    assert_eq!(kind, Kind::Uncompressed);

    // The two that were touched kept theirs.
    let (kind, _) = across(&mut c, &mut d, &at(7001, 1002, 114).build(b"c"));
    assert_eq!(kind, Kind::Compressed, "7001 lost a slot it had just used");
    let (kind, _) = across(&mut c, &mut d, &at(7002, 1002, 115).build(b"c"));
    assert_eq!(kind, Kind::Compressed, "7002 lost a slot it had just used");
    // And 7003, untouched since it was first seen, is the one that went.
    let (kind, _) = across(&mut c, &mut d, &at(7003, 1001, 113).build(b"b"));
    assert_eq!(kind, Kind::Uncompressed, "7003 kept a slot it should have lost");

    // Everything above is why RFC 1332 Appendix A asks for "at least 4 slots,
    // usually 16": with three, four conversations spend their time evicting
    // each other, and a browser opens more than four.
}

/// The window "is also the difference between the current and previous
/// values. However, either positive or negative changes are allowed."
#[test]
fn the_window_may_move_either_way() {
    let (mut c, mut d) = pair();
    let with = |window: u16, seq: u32, id: u16| {
        let mut s = Seg::new(seq, 2000, id);
        s.window = window;
        s
    };
    across(&mut c, &mut d, &with(8000, 1000, 100).build(b"a"));
    let (_, wire) = across(&mut c, &mut d, &with(8500, 1001, 101).build(b"b"));
    assert_ne!(wire[0] & 0x02, 0, "the window change was not sent");
    // Downwards, which wraps and must still come out right.
    let (_, wire) = across(&mut c, &mut d, &with(200, 1002, 102).build(b"c"));
    assert_ne!(wire[0] & 0x02, 0);
}

/// "The packet ID is typically incremented by one for each packet sent so a
/// change of zero is very unlikely. A change of one is likely."
#[test]
fn an_identifier_that_goes_up_by_one_costs_nothing_and_anything_else_is_sent() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1001, 2001, 101).build(b"b"));
    assert_eq!(wire[0] & 0x20, 0, "a step of one was sent anyway");

    // A jump, because the machine has other conversations.
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1002, 2002, 109).build(b"c"));
    assert_ne!(wire[0] & 0x20, 0, "a jump of eight was not sent");
    // And backwards, which the encoding has to survive.
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1003, 2003, 104).build(b"d"));
    assert_ne!(wire[0] & 0x20, 0);
}

/// "If the URG flag is set, the urgent data field is encoded (note that it may
/// be zero)... if URG is clear, the urgent data field must be checked against
/// the previous packet and, if it changes, an UNCOMPRESSED_TCP packet is
/// sent."
#[test]
fn urgent_data_is_carried_and_a_stale_pointer_is_not_believed() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let mut urgent = Seg::new(1001, 2000, 101);
    urgent.flags |= URG;
    urgent.urgent = 9;
    let (kind, wire) = across(&mut c, &mut d, &urgent.build(b"b"));
    assert_eq!(kind, Kind::Compressed);
    assert_ne!(wire[0] & 0x01, 0, "the urgent pointer was not sent");

    // URG clear but the field moved: it cannot be represented, so the whole
    // datagram goes.
    let mut stale = Seg::new(1002, 2000, 102);
    stale.urgent = 11;
    let (kind, _) = across(&mut c, &mut d, &stale.build(b"c"));
    assert_eq!(kind, Kind::Uncompressed);
}

/// A push "can (and does) change in any datagram", so it travels as its own
/// bit rather than forcing the header out whole.
#[test]
fn a_push_is_a_bit_in_the_mask() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let mut pushed = Seg::new(1001, 2001, 101);
    pushed.flags |= PSH;
    let (kind, wire) = across(&mut c, &mut d, &pushed.build(b"b"));
    assert_eq!(kind, Kind::Compressed);
    assert_eq!(wire[0], 0x1b, "got {wire:02x?}");
    // And it clears again.
    let (_, wire) = across(&mut c, &mut d, &Seg::new(1002, 2002, 102).build(b"c"));
    assert_eq!(wire[0], 0x0b);
}

/// 3.2.3: "if nothing changed, check if this packet has no user data... or if
/// the previous packet contained user data... In either of these cases, send
/// an UNCOMPRESSED_TCP packet."
#[test]
fn a_duplicate_acknowledgement_is_sent_whole() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b""));
    let (kind, _) = across(&mut c, &mut d, &Seg::new(1000, 2000, 101).build(b""));
    assert_eq!(kind, Kind::Uncompressed, "a duplicate ack was compressed");
}

/// A TCP option appearing or going away changes the header length, and 3.2.3
/// sends the datagram whole rather than trying to describe it.
#[test]
fn a_header_that_changes_shape_goes_whole() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    // Six words of TCP header with a two-word option in it.
    let mut with_option = Seg::new(1001, 2001, 101).build(b"b");
    let tcp = ip::HEADER_LEN;
    with_option[tcp + 12] = 6 << 4;
    with_option.insert(tcp + 20, 0);
    with_option.insert(tcp + 20, 0);
    with_option.insert(tcp + 20, 4);
    with_option.insert(tcp + 20, 2);
    let total = with_option.len() as u16;
    with_option[2..4].copy_from_slice(&total.to_be_bytes());
    with_option[10..12].copy_from_slice(&[0, 0]);
    let sum = ip::checksum(&with_option[..tcp]);
    with_option[10..12].copy_from_slice(&sum.to_be_bytes());
    let (kind, _) = across(&mut c, &mut d, &with_option);
    assert_eq!(kind, Kind::Uncompressed);
}

/// The variable-length encoding of 3.2.2, in both directions and including the
/// two cases the text gives values for: "decimal 15 is encoded as hex 0f, 255
/// as ff, 65534 as 00 ff fe, and zero as 00 00 00".
#[test]
fn the_number_encoding_matches_the_examples_in_the_document() {
    let cases: [(u16, &[u8]); 4] = [
        (15, &[0x0f]),
        (255, &[0xff]),
        (65534, &[0x00, 0xff, 0xfe]),
        (0, &[0x00, 0x00, 0x00]),
    ];
    for (value, expected) in cases {
        let mut out = Vec::new();
        encode(value, &mut out);
        assert_eq!(out, expected, "{value} encoded as {out:02x?}");
        let mut at = 0;
        assert_eq!(decode(&out, &mut at), Some(value));
        assert_eq!(at, expected.len());
    }
    // Every value round trips, not only the four with examples.
    for value in 0..=u16::MAX {
        let mut out = Vec::new();
        encode(value, &mut out);
        let mut at = 0;
        assert_eq!(decode(&out, &mut at), Some(value), "{value} did not survive");
    }
}

/// A truncated compressed packet is refused rather than guessed at.
#[test]
fn a_packet_that_stops_early_is_not_believed() {
    let (mut c, mut d) = pair();
    across(&mut c, &mut d, &Seg::new(1000, 2000, 100).build(b"a"));
    let (kind, wire) = c.compress(&Seg::new(1005, 2300, 108).build(b"b"));
    assert_eq!(kind, Kind::Compressed);
    for cut in 0..wire.len().min(6) {
        let mut d = Decompressor::new(Params::DEFAULT);
        // Seed it so the failure is the truncation and not the missing slot.
        let (k, w) = Compressor::new(Params::DEFAULT).compress(&Seg::new(1000, 2000, 100).build(b"a"));
        d.decompress(k, &w);
        assert_eq!(d.decompress(Kind::Compressed, &wire[..cut]), None, "cut at {cut}");
    }
}

/// A connection number no uncompressed packet ever named cannot be rebuilt.
#[test]
fn a_slot_nothing_has_seeded_is_refused() {
    let mut d = Decompressor::new(Params::DEFAULT);
    assert_eq!(d.decompress(Kind::Compressed, &[0x4b, 0x05, 0xbe, 0xef, b'x']), None);
    assert!(d.tossing());
    // And one outside the agreed range, which is a far end disagreeing about
    // what was negotiated.
    let mut d = Decompressor::new(Params { max_slot: 3, compress_slot: true });
    assert_eq!(d.decompress(Kind::Compressed, &[0x40, 0x09, 0xbe, 0xef]), None);
}

/// The protocol numbers of RFC 1332 4, both ways round.
#[test]
fn the_three_protocol_numbers_name_the_three_kinds() {
    assert_eq!(Kind::Ip.protocol(), 0x0021);
    assert_eq!(Kind::Compressed.protocol(), 0x002d);
    assert_eq!(Kind::Uncompressed.protocol(), 0x002f);
    assert_eq!(Kind::from_protocol(0x0021), Some(Kind::Ip));
    assert_eq!(Kind::from_protocol(0x002d), Some(Kind::Compressed));
    assert_eq!(Kind::from_protocol(0x002f), Some(Kind::Uncompressed));
    assert_eq!(Kind::from_protocol(0x8021), None);
}
