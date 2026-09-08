//! A call over the line a sound card makes, which is a strange one.
//!
//! `modem-loop` puts two modems on one virtual cable: their outputs are summed
//! and written to it, and what comes back is read by both. That is a two-wire
//! pair, correctly, and it is the reason a single cable is enough to place a
//! real call. What is not ordinary is the shape of it. A hybrid reflects in a
//! millisecond or two and is well down on what it reflects; a sound card puts
//! everything back at full strength, tens of milliseconds later, having taken
//! it through a rate conversion each way.
//!
//! V.22bis does not care, because it hears the far end through a filter that
//! discards its own band along with the echo in it. V.32 cares entirely: both
//! directions share the band, and every threshold in the start-up is a ratio
//! to the level of the whole line, which an uncancelled echo at full strength
//! is half of.
//!
//! So this is the test for whether the echo canceller can be pointed far
//! enough away to be any use here. The delay is the one measured on real
//! hardware: 87 ms of round trip, which is a single crossing of about 44.

use modem::{Modem, State};
use std::collections::VecDeque;

const FS: f64 = 16_000.0;

/// Headroom for the sum of two modems on one pair, as `modem-loop` applies it.
const HEADROOM: f64 = 0.45;

/// One crossing of the cable, in samples.
///
/// Measured on the rig this is meant to predict: `modem-loop` reported a round
/// trip of 87 ms, and a loopback returns a signal after one crossing where the
/// measurement counts two.
const CROSSING: usize = 700;

/// Two modems on one cable, hearing everything on it including themselves.
struct Cable {
    caller: Modem,
    host: Modem,
    wire: VecDeque<f64>,
    at_caller: Vec<u8>,
}

impl Cable {
    fn new(carrier: &str, crossing: usize) -> Self {
        let mut caller = Modem::new(FS);
        let mut host = Modem::new(FS);
        for m in [&mut caller, &mut host] {
            // Automode off. These are about one modulation over one line, and
        // with it on the modem would negotiate its way to whichever
        // modulation the two ends liked best -- which is the right
        // answer to a different question.
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
        Self {
            caller,
            host,
            wire: VecDeque::from(vec![0.0; crossing]),
            at_caller: Vec::new(),
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let heard = self.wire.pop_front().unwrap_or(0.0);
            let a = self.caller.step(heard);
            let b = self.host.step(heard);
            self.wire.push_back((a + b) * HEADROOM);
            self.at_caller.extend(self.caller.take_dte());
            self.host.take_dte();
        }
    }

    fn up(&self) -> bool {
        self.caller.state() == State::Data && self.host.state() == State::Data
    }
}

#[test]
fn a_v32_call_goes_through_a_sound_card_loopback() {
    let mut cable = Cable::new("V32", CROSSING);
    cable.run(25.0);

    println!(
        "caller {} / host {}; round trip {:?} symbols; reflection {:?}; \
         echo return loss {:?}",
        cable.caller.line_phase(),
        cable.host.line_phase(),
        cable.caller.round_trip_symbols(),
        cable.caller.reflection(),
        cable.caller.echo_return_loss().map(|d| (d * 10.0).round() / 10.0),
    );
    assert!(
        cable.up(),
        "never connected: the caller stopped at {} and the host at {}",
        cable.caller.line_phase(),
        cable.host.line_phase()
    );

    let greeting = "Welcome to phl6-dial1.popsite.net\r\nlogin:";
    for b in greeting.bytes() {
        cable.host.feed_dte(b);
    }
    cable.run(25.0);
    let seen = String::from_utf8_lossy(&cable.at_caller).into_owned();
    assert!(
        seen.contains("CONNECT"),
        "the terminal was never told about the connection: {seen:?}"
    );
    assert!(
        seen.contains(greeting),
        "connected, but the greeting did not come through: {seen:?}"
    );
    // And at the faster of the two rates V.32 offers, over a line that reflects
    // everything back at full strength forty-four milliseconds later.
    assert_eq!(cable.caller.rate(), Some(9600));
}

#[test]
fn the_cable_is_found_at_a_single_crossing_rather_than_the_round_trip() {
    // The distinction that would be easy to get wrong and impossible to see.
    // On a real line our own signal comes back off the far hybrid, a whole
    // round trip away; on a cable it comes back after one crossing, because
    // the cable is the far end. Taps placed at the round trip would be sitting
    // on empty line, which is why the delay is looked for rather than assumed.
    let mut cable = Cable::new("V32", CROSSING);
    cable.run(25.0);
    let found = cable
        .caller
        .reflection()
        .expect("the caller found nothing coming back at all");
    let off = found.delay as i64 - CROSSING as i64;
    assert!(
        off.abs() <= 16,
        "put the cable {off} samples from a single crossing of it"
    );
    assert!(
        found.strength > 0.8,
        "a cable returns everything written to it, so the reflection should be \
         nearly all of what arrives, not {:.2}",
        found.strength
    );
}

#[test]
fn a_loopback_short_enough_to_need_no_far_taps_works_too() {
    // The other end of the range, and the one that was hardest to see. Four
    // milliseconds is inside the near taps, so nothing goes looking for a
    // reflection and there is nothing for the second run of taps to do -- and
    // it still failed, because the receiver was being held still only in its
    // equaliser while its timing loop, carrier loop and gain went on tracking
    // the modem's own echo. Nothing about that is particular to a long line;
    // the long line only made it visible.
    let mut cable = Cable::new("V32", 64);
    cable.run(25.0);
    assert!(
        cable.up(),
        "the caller stopped at {} and the host at {}",
        cable.caller.line_phase(),
        cable.host.line_phase()
    );
    assert_eq!(
        cable.caller.reflection(),
        None,
        "went looking for a reflection the near taps already cover"
    );
}

#[test]
fn v22bis_still_goes_through_it_without_needing_any_of_that() {
    // The control. V.22bis puts the two directions in separate bands, so the
    // filter that selects the far one throws the echo away with the near one
    // and none of this arises.
    let mut cable = Cable::new("V22B", CROSSING);
    cable.run(25.0);
    assert!(
        cable.up(),
        "the caller stopped at {} and the host at {}",
        cable.caller.line_phase(),
        cable.host.line_phase()
    );
}

#[test]
#[ignore]
fn trace() {
    let mut cable = Cable::new("V32", CROSSING);
    let mut last = ("", "");
    for i in 0..(25.0 * FS) as usize {
        let heard = cable.wire.pop_front().unwrap_or(0.0);
        let a = cable.caller.step(heard);
        let b = cable.host.step(heard);
        cable.wire.push_back((a + b) * HEADROOM);
        cable.caller.take_dte();
        cable.host.take_dte();
        let now = (cable.caller.line_phase(), cable.host.line_phase());
        if now != last {
            last = now;
            println!(
                "{:>7.3}s  caller {:>13}   host {:>13}",
                i as f64 / FS,
                now.0,
                now.1
            );
        }
    }
    println!(
        "caller: round trip {:?}, reflection {:?}, loss {:?}",
        cable.caller.round_trip_symbols(),
        cable.caller.reflection(),
        cable.caller.echo_return_loss()
    );
    println!(
        "host:   round trip {:?}, reflection {:?}, loss {:?}",
        cable.host.round_trip_symbols(),
        cable.host.reflection(),
        cable.host.echo_return_loss()
    );
}

#[test]
#[ignore]
fn sweep_the_crossing() {
    // What the delay actually is on a given machine depends on buffer sizes
    // nothing here chooses. This says how much of that range works.
    for crossing in [64, 160, 320, 480, 700, 900, 1200, 1600, 2000] {
        let mut cable = Cable::new("V32", crossing);
        cable.run(25.0);
        println!(
            "{crossing:>5} samples ({:>5.1} ms): {:>13} / {:>13}  reflection {:?}",
            crossing as f64 / FS * 1000.0,
            cable.caller.line_phase(),
            cable.host.line_phase(),
            cable.caller.reflection().map(|r| (r.delay, (r.strength * 100.0).round() / 100.0)),
        );
    }
}

#[test]
fn three_hundred_baud_goes_through_it_as_well() {
    // What a board from 1985 would hear. Bell 103 puts the two directions
    // 750 Hz apart, so a cable that hands each modem its own signal back at
    // full strength is of no interest to either: the band filter throws the
    // echo away with the half of the spectrum it lives in.
    let mut cable = Cable::new("B103", CROSSING);
    cable.run(25.0);
    assert!(
        cable.up(),
        "the caller stopped at {} and the host at {}",
        cable.caller.line_phase(),
        cable.host.line_phase()
    );
    assert_eq!(cable.caller.rate(), Some(300));

    let banner = "\r\nThe Dead Zone BBS\r\nLogin: ";
    for b in banner.bytes() {
        cable.host.feed_dte(b);
    }
    cable.run(25.0);
    let seen = String::from_utf8_lossy(&cable.at_caller).into_owned();
    assert!(seen.contains(banner), "the banner did not arrive: {seen:?}");
}
