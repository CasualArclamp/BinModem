//! Two PPP ends on one link, from silence to a ping and back.
//!
//! The point of the whole crate stated once: octets go in one end and an echo
//! comes back out of the other, having crossed LCP's negotiation, IPCP's
//! addresses, and an IP datagram in each direction.

use ppp::link::{Link, Phase};

/// Run the two ends against each other until both are up or the time runs out.
///
/// Octets cross whole rather than one at a time, which is what a modem
/// delivers: a buffer's worth arrives at once.
fn connect(a: &mut Link, b: &mut Link, ms: u32) -> u32 {
    a.open();
    b.open();
    for elapsed in 0..ms {
        let from_a = a.take_line();
        let from_b = b.take_line();
        if !from_a.is_empty() {
            b.feed(&from_a);
        }
        if !from_b.is_empty() {
            a.feed(&from_b);
        }
        a.tick(1);
        b.tick(1);
        if a.up() && b.up() {
            return elapsed;
        }
    }
    panic!(
        "never came up: a is {:?} and b is {:?}",
        a.phase(),
        b.phase()
    );
}

fn exchange(a: &mut Link, b: &mut Link, rounds: u32) {
    for _ in 0..rounds {
        let from_a = a.take_line();
        let from_b = b.take_line();
        if !from_a.is_empty() {
            b.feed(&from_a);
        }
        if !from_b.is_empty() {
            a.feed(&from_b);
        }
        a.tick(1);
        b.tick(1);
    }
}

#[test]
fn two_ends_agree_and_then_one_pings_the_other() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    let took = connect(&mut a, &mut b, 30_000);
    println!("  up in {took} ms of link time");

    assert_eq!(a.phase(), Phase::Network);
    assert_eq!(b.phase(), Phase::Network);
    assert_eq!(a.addresses(), ([10, 0, 0, 1], [10, 0, 0, 2]));
    assert_eq!(b.addresses(), ([10, 0, 0, 2], [10, 0, 0, 1]));
    let _ = a.take_arrived();
    let _ = b.take_arrived();

    assert!(a.ping(0x4269, 1, b"binmodem says hello"), "the ping was refused");
    exchange(&mut a, &mut b, 50);

    // It arrived at the far end as a request...
    let at_b = b.take_arrived();
    assert_eq!(at_b.len(), 1, "the far end did not see it");
    assert!(!at_b[0].echo.reply);
    assert_eq!(at_b[0].from, [10, 0, 0, 1]);
    assert_eq!(at_b[0].to, [10, 0, 0, 2]);
    assert_eq!(at_b[0].echo.payload, b"binmodem says hello");

    // ...and came back as a reply, with everything it was sent with.
    let at_a = a.take_arrived();
    assert_eq!(at_a.len(), 1, "no answer came back");
    assert!(at_a[0].echo.reply);
    assert_eq!(at_a[0].from, [10, 0, 0, 2]);
    assert_eq!(at_a[0].echo.id, 0x4269);
    assert_eq!(at_a[0].echo.sequence, 1);
    assert_eq!(at_a[0].echo.payload, b"binmodem says hello");
}

/// An end that has no address is given one (RFC 1332 3.3), which is what
/// dialling an internet provider is.
#[test]
fn an_end_with_no_address_is_told_what_it_is_called() {
    let mut server = Link::new([192, 168, 9, 1], [192, 168, 9, 40]);
    let mut client = Link::new([0, 0, 0, 0], [0, 0, 0, 0]);
    connect(&mut server, &mut client, 30_000);

    assert_eq!(client.addresses(), ([192, 168, 9, 40], [192, 168, 9, 1]));
    assert_eq!(server.addresses(), ([192, 168, 9, 1], [192, 168, 9, 40]));

    let _ = server.take_arrived();
    let _ = client.take_arrived();
    assert!(client.ping(7, 1, b"and back"));
    exchange(&mut server, &mut client, 50);
    let back = client.take_arrived();
    assert_eq!(back.len(), 1);
    assert!(back[0].echo.reply);
    assert_eq!(back[0].from, [192, 168, 9, 1]);
}

/// Several pings in a row keep their own identity.
#[test]
fn each_ping_is_answered_with_its_own_sequence() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    connect(&mut a, &mut b, 30_000);
    let _ = a.take_arrived();
    let _ = b.take_arrived();

    for sequence in 1..=8u16 {
        assert!(a.ping(0x1234, sequence, format!("ping {sequence}").as_bytes()));
        exchange(&mut a, &mut b, 20);
    }
    let replies: Vec<_> = a.take_arrived().into_iter().filter(|p| p.echo.reply).collect();
    assert_eq!(replies.len(), 8, "not every ping came back");
    for (i, reply) in replies.iter().enumerate() {
        let sequence = i as u16 + 1;
        assert_eq!(reply.echo.sequence, sequence);
        assert_eq!(reply.echo.payload, format!("ping {sequence}").as_bytes());
    }
}

/// Nothing goes out before the link is up, because there is nowhere to send it.
#[test]
fn a_ping_before_the_link_is_up_is_refused_rather_than_lost() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    assert!(!a.ping(1, 1, b"too early"));
    a.open();
    assert!(!a.ping(1, 1, b"still too early"), "sent before IPCP agreed");
}

/// A link nobody answers gives up rather than trying for ever.
#[test]
fn an_end_talking_to_nothing_stops_talking() {
    let mut alone = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    alone.open();
    let mut sent = 0;
    for _ in 0..120_000 {
        sent += alone.take_line().len();
        alone.tick(1);
    }
    assert!(sent > 0, "it never said anything at all");
    assert_ne!(alone.phase(), Phase::Network, "it came up against nobody");
    // 4.6's counter: ten requests, not a stream of them for two minutes.
    let quiet: usize = (0..10_000)
        .map(|_| {
            alone.tick(1);
            alone.take_line().len()
        })
        .sum();
    assert_eq!(quiet, 0, "it was still asking after it had given up");
}

/// The same two ends, with what one says handed over before the other has had
/// its turn.
///
/// A modem does not deliver in neat alternating rounds, and this ordering is
/// the one that catches the mistake: the far end's Configure-Request and its
/// Configure-Ack of ours arrive in one buffer, so this end acknowledges and
/// comes up in the same breath. What it acknowledged with has to be the old
/// framing -- the far end is still down and still entitled to strip control
/// octets out of anything it is sent -- and what it says afterwards has to be
/// the new. Getting that the wrong way round deadlocks the link with both
/// ends waiting for an acknowledgement the other has already sent.
#[test]
fn it_comes_up_however_the_two_ends_are_interleaved() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    a.open();
    b.open();
    for _ in 0..2_000 {
        let from_a = a.take_line();
        if !from_a.is_empty() {
            b.feed(&from_a);
        }
        let from_b = b.take_line();
        if !from_b.is_empty() {
            a.feed(&from_b);
        }
        a.tick(1);
        b.tick(1);
        if a.up() && b.up() {
            return;
        }
    }
    panic!("stuck at {:?} and {:?}", a.phase(), b.phase());
}

/// And one octet at a time, which is what a slow line delivers.
#[test]
fn it_comes_up_one_octet_at_a_time() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    a.open();
    b.open();
    let (mut to_a, mut to_b) = (Vec::new(), Vec::new());
    for _ in 0..20_000 {
        to_b.extend(a.take_line());
        to_a.extend(b.take_line());
        if !to_b.is_empty() {
            b.feed(&[to_b.remove(0)]);
        }
        if !to_a.is_empty() {
            a.feed(&[to_a.remove(0)]);
        }
        a.tick(1);
        b.tick(1);
        if a.up() && b.up() {
            return;
        }
    }
    panic!("stuck at {:?} and {:?}", a.phase(), b.phase());
}

/// One TCP segment, enough of one for the layer below to recognise.
fn tcp_segment(seq: u32, ack: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + data.len());
    out.extend_from_slice(&1234u16.to_be_bytes());
    out.extend_from_slice(&80u16.to_be_bytes());
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&ack.to_be_bytes());
    out.push(5 << 4);
    out.push(0x10);
    out.extend_from_slice(&4096u16.to_be_bytes());
    out.extend_from_slice(&0xbeefu16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(data);
    out
}

const ROUNDS: u32 = 40;
const PAYLOAD: usize = 32;

/// Send the same conversation over a pair of links and say how many octets it
/// put on the line, checking every segment arrived whole.
fn a_download_across(a: &mut Link, b: &mut Link) -> usize {
    let mut seq = 1000u32;
    let mut on_the_line = 0usize;
    for round in 0..ROUNDS {
        let data: Vec<u8> = (0..PAYLOAD as u8).map(|i| i.wrapping_add(round as u8)).collect();
        assert!(a.send_payload(ppp::ip::PROTOCOL_TCP, &tcp_segment(seq, 5000, &data)));
        let wire = a.take_line();
        on_the_line += wire.len();
        b.feed(&wire);
        a.feed(&b.take_line());
        seq = seq.wrapping_add(PAYLOAD as u32);
    }
    let carried = b.take_carried();
    assert_eq!(carried.len(), ROUNDS as usize, "not everything arrived");
    for (round, datagram) in carried.iter().enumerate() {
        assert_eq!(datagram.protocol, ppp::ip::PROTOCOL_TCP);
        let expected: Vec<u8> = (0..PAYLOAD as u8).map(|i| i.wrapping_add(round as u8)).collect();
        assert_eq!(&datagram.payload[20..], &expected[..], "round {round} came back wrong");
    }
    on_the_line
}

/// Two ends agree header compression, and the same conversation costs less.
///
/// The measurement is the point, and it is made against the same traffic with
/// the option turned off rather than against a guess: forty octets of header
/// on every segment, which RFC 1144 3.2.2 gets down to three or four.
#[test]
fn header_compression_is_agreed_and_shrinks_what_crosses() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    connect(&mut a, &mut b, 30_000);

    // RFC 1332 4: each direction is asked for separately, and between two of
    // these both are granted.
    let agreed = a.header_compression();
    assert!(agreed.sending.is_some(), "nothing agreed towards the far end");
    assert!(agreed.receiving.is_some(), "nothing agreed towards this end");
    assert_eq!(agreed.sending, b.header_compression().receiving);

    let compressed = a_download_across(&mut a, &mut b);

    let mut c = Link::new([10, 0, 0, 1], [10, 0, 0, 2]).without_header_compression();
    let mut d = Link::new([10, 0, 0, 2], [10, 0, 0, 1]).without_header_compression();
    connect(&mut c, &mut d, 30_000);
    let plain = a_download_across(&mut c, &mut d);

    let saved = plain - compressed;
    let each = saved / ROUNDS as usize;
    println!("  {plain} octets became {compressed}: {each} saved on each of {ROUNDS}");
    // Forty octets of header became three plus a checksum. The framing and the
    // payload are unchanged, so the saving is the header and nothing else.
    assert!(
        each >= 33,
        "only {each} octets a datagram: {compressed} against {plain}"
    );
}

/// The traffic above is 3.2.2's second special case, and it is worth pinning
/// what the header actually looks like: "packets in the data transfer
/// direction of an FTP put or get look like 0F c c d ...".
#[test]
fn a_one_way_transfer_crosses_as_a_mask_and_a_checksum() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    connect(&mut a, &mut b, 30_000);

    // The first names the connection; the second is the one to look at.
    assert!(a.send_payload(ppp::ip::PROTOCOL_TCP, &tcp_segment(1000, 5000, b"abcd")));
    exchange(&mut a, &mut b, 20);
    assert_eq!(b.take_carried().len(), 1, "the first one did not arrive");
    assert!(a.send_payload(ppp::ip::PROTOCOL_TCP, &tcp_segment(1004, 5000, b"efgh")));
    let wire = a.take_line();
    b.feed(&wire);

    // Between the flags: the protocol field, then the compressed header.
    // 002d with protocol-field compression is one octet.
    assert!(
        wire.windows(4).any(|w| w == [0x2d, 0x0f, 0xbe, 0xef]),
        "no 0F c c header in {wire:02x?}"
    );
    // Four octets of data, four of header and protocol, and the framing.
    assert!(wire.len() <= 14, "{} octets for four of data", wire.len());
    let carried = b.take_carried();
    assert_eq!(carried.len(), 1);
    assert_eq!(&carried[0].payload[20..], b"efgh");
}

/// An end that will not do it is taken at its word, and everything still
/// crosses -- uncompressed, which is what RFC 1332 4 leaves behind.
#[test]
fn a_far_end_that_will_not_compress_headers_still_carries_them() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]).without_header_compression();
    connect(&mut a, &mut b, 30_000);

    // b asked for nothing, and refused what a asked for, so neither direction
    // compresses.
    assert_eq!(b.header_compression().receiving, None);
    assert_eq!(b.header_compression().sending, None);
    assert_eq!(a.header_compression().sending, None);
    assert_eq!(a.header_compression().receiving, None);

    let segment = tcp_segment(1000, 5000, b"the ordinary case");
    assert!(a.send_payload(ppp::ip::PROTOCOL_TCP, &segment));
    exchange(&mut a, &mut b, 20);
    let carried = b.take_carried();
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].payload, segment);
}

/// A ping still crosses with compression running, and leaves it alone.
///
/// 3.2.3: anything that is not TCP "is an unmodified copy of the input packet
/// and processing it doesn't change the compressor's state in any way".
#[test]
fn a_ping_crosses_a_compressed_link_untouched() {
    let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
    connect(&mut a, &mut b, 30_000);

    assert!(a.send_payload(ppp::ip::PROTOCOL_TCP, &tcp_segment(1000, 5000, b"abcd")));
    exchange(&mut a, &mut b, 20);
    let _ = b.take_carried();

    assert!(a.ping(0x1234, 7, b"binmodem"));
    exchange(&mut a, &mut b, 40);
    let replies = a.take_arrived();
    assert!(
        replies.iter().any(|r| r.echo.reply && r.echo.sequence == 7),
        "the ping did not come back: {replies:?}"
    );

    // And the conversation carries on from where it was.
    assert!(a.send_payload(ppp::ip::PROTOCOL_TCP, &tcp_segment(1004, 5000, b"efgh")));
    exchange(&mut a, &mut b, 20);
    let carried = b.take_carried();
    assert_eq!(carried.len(), 1);
    assert_eq!(&carried[0].payload[20..], b"efgh");
}
