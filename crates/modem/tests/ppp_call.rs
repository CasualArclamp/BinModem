//! A ping crossing a call, through everything.
//!
//! The sibling of `file_transfer.rs`, one layer taller. V.8 agrees a
//! modulation, the data pump carries the bits, V.42 makes them reliable,
//! V.42bis compresses them, PPP turns the octets back into frames, IPCP gives
//! the two ends addresses, and an ICMP echo goes from one to the other and
//! comes back -- with nothing knowing about anything below it.
//!
//! Everything in this is simulated except the modems, which are the same ones
//! that go on a line. There is no sound card and no telephone network: what
//! one modem writes, the other hears, one sample at a time.

use modem::{Modem, State};
use ppp::link::Link;
use ppp::ping::Pinger;

const FS: f64 = 16_000.0;

/// How the two ends decide what they are called.
///
/// The end that answered the call is the end with addresses to give, which is
/// what dialling a provider was. RFC 1332 3.3 leaves it open; a modem call
/// settles it, because one end of one has already answered.
const SERVER: [u8; 4] = [10, 0, 0, 1];
const CLIENT: [u8; 4] = [10, 0, 0, 2];

/// What came of it.
struct Outcome {
    /// The address the calling end ended up with, which it was not given at
    /// the start.
    client_address: [u8; 4],
    server_address: [u8; 4],
    sent: u32,
    received: u32,
    /// Seconds of line the call took to reach the network phase.
    up_at: f64,
    /// The last round trip, in milliseconds of line time.
    round_trip_ms: u64,
}

/// Two modems on one line, PPP over the call, and `count` echoes across it.
fn ping_across(count: u32) -> Option<Outcome> {
    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        for b in b"AT+MS=V32,0,9600,9600\r" {
            m.feed_dte(*b);
        }
        m.take_dte();
    }
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }
    for b in b"ATD5551234\r" {
        caller.feed_dte(*b);
    }

    // The answering end knows both addresses. The calling end knows neither
    // and asks with zeroes, which 3.3 makes the question rather than an
    // address.
    let mut server = Link::new(SERVER, CLIENT);
    let mut client = Link::new([0, 0, 0, 0], [0, 0, 0, 0]);
    let mut pinger = Pinger::new(0x0b17);
    pinger.every_ms = 250;
    pinger.timeout_ms = 20_000;

    let mut started = false;
    let mut up_at = None;
    let per_ms = FS as usize / 1000;

    // What each said last, which is what the other hears now: the whole of
    // the line, one sample of delay in each direction.
    let (mut from_caller, mut from_host) = (0.0, 0.0);

    for i in 0..(120.0 * FS) as usize {
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);

        let connected = caller.state() == State::Data && host.state() == State::Data;
        if !connected {
            caller.take_dte();
            host.take_dte();
            continue;
        }
        if !started {
            started = true;
            client.open();
            server.open();
            pinger.start();
        }

        client.feed(&caller.take_dte());
        server.feed(&host.take_dte());

        if i % per_ms == 0 {
            client.tick(1);
            server.tick(1);
            let _ = pinger.poll(&mut client, 1);
        }

        for byte in client.take_line() {
            caller.feed_dte(byte);
        }
        for byte in server.take_line() {
            host.feed_dte(byte);
        }

        if up_at.is_none() && client.up() && server.up() {
            up_at = Some(i as f64 / FS);
        }
        if pinger.stats.received >= count {
            break;
        }
    }

    up_at.map(|up_at| Outcome {
        client_address: client.addresses().0,
        server_address: client.addresses().1,
        sent: pinger.stats.sent,
        received: pinger.stats.received,
        up_at,
        round_trip_ms: pinger.stats.last_ms,
    })
}

#[test]
fn a_ping_crosses_a_call() {
    let outcome = ping_across(3).expect("PPP never reached the network phase");

    // The calling end was told what it is called, over the modem, by the end
    // that answered. Nothing configured it: it asked with zeroes and this
    // came back in a Configure-Nak.
    assert_eq!(outcome.client_address, CLIENT, "it was not given an address");
    assert_eq!(outcome.server_address, SERVER);

    assert!(outcome.received >= 3, "only {} came back", outcome.received);
    assert!(
        outcome.sent >= outcome.received,
        "more answers than questions"
    );
    println!(
        "  network phase {:.1} s into the call, round trip {} ms over {} sent",
        outcome.up_at, outcome.round_trip_ms, outcome.sent
    );
}
