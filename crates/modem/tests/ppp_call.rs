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
//!
//! Two lines are modelled, because two are what people have. A four-wire pair
//! is the ordinary case, where each end hears only the other. One virtual
//! cable is the case somebody trying this on a single machine will hit, where
//! both ends hear everything on it including themselves, tens of milliseconds
//! later and at full strength -- and where the two instances of this program
//! meant to talk to each other actually meet.

use modem::{Modem, State};
use ppp::link::Link;
use ppp::ping::Pinger;
use std::collections::VecDeque;

const FS: f64 = 16_000.0;

/// How the two ends decide what they are called.
///
/// The end that answered the call is the end with addresses to give, which is
/// what dialling a provider was. RFC 1332 3.3 leaves it open; a modem call
/// settles it, because one end of one has already answered.
const SERVER: [u8; 4] = [10, 0, 0, 1];
const CLIENT: [u8; 4] = [10, 0, 0, 2];

/// One crossing of a virtual cable, and the headroom the sum needs.
///
/// Both from `soundcard_loop.rs`, which measured them on the rig this is
/// meant to predict.
const CROSSING: usize = 700;
const HEADROOM: f64 = 0.45;

/// What is between the two modems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// Each end hears only the other, which is what a telephone pair with a
    /// hybrid at each end comes to.
    Pair,
    /// One virtual cable: everything on it is heard by both, a crossing later.
    Cable,
}

/// What came of it.
struct Outcome {
    /// The address the calling end ended up with, which it was not given at
    /// the start.
    client_address: [u8; 4],
    server_address: [u8; 4],
    sent: u32,
    received: u32,
    rate: Option<u32>,
    /// Seconds of line the call took to reach the network phase.
    up_at: f64,
    /// The last round trip, in milliseconds of line time.
    round_trip_ms: u64,
}

/// Two modems on `line`, PPP over the call, and echoes across it until `count`
/// have come back.
fn ping_across(carrier: &str, line: Line, count: u32) -> Option<Outcome> {
    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        for b in format!("AT+MS={carrier},0\r").bytes() {
            m.feed_dte(b);
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
    // A cable this long is most of a second of round trip before anything has
    // been asked, and there is no point calling an echo lost that is still on
    // its way.
    pinger.timeout_ms = 20_000;

    let mut started = false;
    let mut up_at = None;
    let per_ms = FS as usize / 1000;

    // What each said last, which is what the other hears now on a pair; and
    // the cable itself, which delays what is on it by a crossing.
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let mut wire = VecDeque::from(vec![0.0; CROSSING]);

    for i in 0..(180.0 * FS) as usize {
        match line {
            Line::Pair => {
                let (a, b) = (from_caller, from_host);
                from_caller = caller.step(b);
                from_host = host.step(a);
            }
            Line::Cable => {
                let heard = wire.pop_front().unwrap_or(0.0);
                let a = caller.step(heard);
                let b = host.step(heard);
                wire.push_back((a + b) * HEADROOM);
            }
        }

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
        rate: caller.rate(),
        up_at,
        round_trip_ms: pinger.stats.last_ms,
    })
}

/// Everything the two ends had to agree on before an echo could cross, and
/// then the echo.
fn check(what: &str, outcome: Option<Outcome>, count: u32) {
    let outcome = outcome.unwrap_or_else(|| panic!("{what}: never reached the network phase"));

    // The calling end was told what it is called, over the modem, by the end
    // that answered. Nothing configured it: it asked with the zeroes RFC 1332
    // 3.3 makes the question, and this came back in a Configure-Nak.
    assert_eq!(
        outcome.client_address, CLIENT,
        "{what}: it was not given an address"
    );
    assert_eq!(outcome.server_address, SERVER);
    assert!(
        outcome.received >= count,
        "{what}: only {} of {} came back",
        outcome.received,
        outcome.sent
    );
    assert!(
        outcome.sent >= outcome.received,
        "{what}: more answers than questions"
    );
    println!(
        "  {what}: {} bit/s, network phase {:.1} s in, round trip {} ms over {} sent",
        outcome.rate.unwrap_or(0),
        outcome.up_at,
        outcome.round_trip_ms,
        outcome.sent
    );
}

#[test]
fn a_ping_crosses_a_call() {
    check("V.32", ping_across("V32", Line::Pair, 3), 3);
}

/// The same over V.22bis, which is the modulation that has actually carried a
/// full session over a real trunk.
#[test]
fn a_ping_crosses_a_slow_call() {
    check("V.22bis", ping_across("V22B", Line::Pair, 2), 2);
}

/// And over one virtual cable, which is two copies of this program on one
/// machine talking to each other.
///
/// The interesting number is the round trip. A crossing of the cable is
/// forty-four milliseconds and there are two of them in an answer, before the
/// modem, the error control and the frames have had their share.
#[test]
fn two_ends_on_one_cable_can_ping_each_other() {
    check("one cable", ping_across("V22B", Line::Cable, 2), 2);
}
