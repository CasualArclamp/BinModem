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
