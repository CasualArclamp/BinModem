//! A whole V.32 call, over a line that behaves like a two-wire one.
//!
//! Each modem hears the far end attenuated by the network and its own signal
//! reflected off the hybrid, the second louder than the first. That is the
//! situation V.32 is designed for and the reason it carries an echo canceller
//! at all: with both directions in the one band, no filter can tell the two
//! apart, and there is nothing to fall back on.

use datapump::v32::startup::{Modem, Role, Status, rate_signal};

const FS: f64 = 16_000.0;

/// Reflection off the hybrid: 12 dB down, which is ordinary.
const ECHO: f64 = 0.251;
/// The far end after crossing the network: 20 dB down, so eight decibels
/// quieter than our own reflection.
const FAR: f64 = 0.1;

/// Run a call and return the two modems along with when they both connected.
fn call(seconds: f64, echo: f64) -> (Modem, Modem, f64) {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let (mut from_calling, mut from_answering) = (0.0, 0.0);
    let mut at = f64::NAN;

    for i in 0..(seconds * FS) as usize {
        let (a, b) = (from_calling, from_answering);
        from_calling = calling.step(b * FAR + a * echo);
        from_answering = answering.step(a * FAR + b * echo);
        if at.is_nan()
            && matches!(calling.status(), Status::Connected(_))
            && matches!(answering.status(), Status::Connected(_))
        {
            at = i as f64 / FS;
        }
    }
    (calling, answering, at)
}

#[test]
fn a_call_completes_over_a_hybrid() {
    let (calling, answering, at) = call(30.0, ECHO);
    assert_eq!(
        calling.status(),
        Status::Connected(4800),
        "the calling end stopped at {} ({:?})",
        calling.phase(),
        calling.status()
    );
    assert_eq!(
        answering.status(),
        Status::Connected(4800),
        "the answering end stopped at {} ({:?})",
        answering.phase(),
        answering.status()
    );
    assert!(at.is_finite(), "never both connected at once");
    println!(
        "connected after {at:.2} s, echo return loss {:.1} and {:.1} dB",
        calling.echo_return_loss(),
        answering.echo_return_loss()
    );
}

#[test]
fn the_echo_canceller_learns_during_the_training_segment() {
    // Note 3 to 5.4.2: the TRN segment "is suitable for training the echo
    // canceller in the transmitting modem". It is the one stretch of the
    // start-up where the far end is required to be silent, so it is the only
    // stretch where what comes back can be assumed to be all our own.
    let (calling, answering, _) = call(30.0, ECHO);
    for (name, loss) in [
        ("calling", calling.echo_return_loss()),
        ("answering", answering.echo_return_loss()),
    ] {
        assert!(
            loss > 15.0,
            "the {name} end is removing only {loss:.1} dB of its own echo"
        );
    }
}

#[test]
fn data_flows_in_both_directions_once_connected() {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let (mut from_calling, mut from_answering) = (0.0, 0.0);
    let (to_host, to_caller) = (b"login: cactus\r\n", b"Password:");
    let (mut sent, mut settled) = (false, f64::NAN);
    let (mut at_host, mut at_caller) = (Vec::new(), Vec::new());

    for i in 0..(40.0 * FS) as usize {
        let (a, b) = (from_calling, from_answering);
        from_calling = calling.step(b * FAR + a * ECHO);
        from_answering = answering.step(a * FAR + b * ECHO);
        at_caller.extend(calling.take_bytes());
        at_host.extend(answering.take_bytes());

        let up = matches!(calling.status(), Status::Connected(_))
            && matches!(answering.status(), Status::Connected(_));
        if up && settled.is_nan() {
            settled = i as f64 / FS;
        }
        // Give both receivers a moment on scrambled ones before speaking:
        // there is an equaliser to converge and a descrambler to synchronise.
        if up && !sent && i as f64 / FS > settled + 1.0 {
            sent = true;
            calling.send(to_host);
            answering.send(to_caller);
        }
    }

    assert!(sent, "never connected, so nothing was sent");
    assert!(
        contains_at_any_bit_offset(&at_host, to_host),
        "the answering end did not receive what was typed"
    );
    assert!(
        contains_at_any_bit_offset(&at_caller, to_caller),
        "the calling end did not receive the host's reply"
    );
}

#[test]
fn without_an_echo_the_call_still_works() {
    // The canceller must not be doing harm on a line that has nothing for it
    // to cancel, which is the case a leased four-wire circuit presents.
    let (calling, answering, at) = call(30.0, 0.0);
    assert_eq!(calling.status(), Status::Connected(4800));
    assert_eq!(answering.status(), Status::Connected(4800));
    assert!(at.is_finite());
}

/// Find `needle` at any bit offset, since the receiver has no way to know
/// where the far end considered a byte to begin.
fn contains_at_any_bit_offset(haystack: &[u8], needle: &[u8]) -> bool {
    let bits: Vec<bool> = haystack
        .iter()
        .flat_map(|b| (0..8).rev().map(move |i| b & (1 << i) != 0))
        .collect();
    let want: Vec<bool> = needle
        .iter()
        .flat_map(|b| (0..8).rev().map(move |i| b & (1 << i) != 0))
        .collect();
    bits.windows(want.len()).any(|w| w == want.as_slice())
}

#[test]
#[ignore]
fn trace() {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let (mut a, mut b) = (0.0, 0.0);
    let (mut cp, mut ap) = ("", "");
    for i in 0..(20.0 * FS) as usize {
        let (pa, pb) = (a, b);
        a = calling.step(pb * FAR + pa * ECHO);
        b = answering.step(pa * FAR + pb * ECHO);
        if calling.phase() != cp || answering.phase() != ap {
            cp = calling.phase();
            ap = answering.phase();
            println!(
                "{:>7.3}s  call {cp:>12} (err {:.2}, echo {:>5.1} dB)   answer {ap:>12} (err {:.2}, echo {:>5.1} dB)",
                i as f64 / FS,
                calling.residual_error(), calling.echo_return_loss(),
                answering.residual_error(), answering.echo_return_loss()
            );
        }
    }
    println!(
        "round trip: call {} answer {}; echo loss {:.1} / {:.1} dB",
        calling.round_trip(), answering.round_trip(),
        calling.echo_return_loss(), answering.echo_return_loss()
    );
}

/// Reflection off the far end, which comes back a whole round trip later.
///
/// A hybrid at each end of a connection means two reflections, not one, and
/// the second is as far away as the line is long. It is the reason V.32
/// measures the round trip at all.
const TALKER: f64 = 0.15;

/// A line with length: what goes down it takes time to arrive, what the near
/// hybrid returns comes back at once, and what the far hybrid returns comes
/// back a whole trip later.
struct Line {
    a: std::collections::VecDeque<f64>,
    b: std::collections::VecDeque<f64>,
    delay: usize,
}

impl Line {
    fn new(delay: usize) -> Self {
        let empty = || std::collections::VecDeque::from(vec![0.0; 2 * delay + 1]);
        Self {
            a: empty(),
            b: empty(),
            delay,
        }
    }

    /// Give each end what the other said, plus both reflections of its own.
    fn step(&mut self, from_a: f64, from_b: f64) -> (f64, f64) {
        self.a.pop_back();
        self.a.push_front(from_a);
        self.b.pop_back();
        self.b.push_front(from_b);
        let there_and_back = 2 * self.delay;
        (
            ECHO * self.a[0] + FAR * self.b[self.delay] + TALKER * self.a[there_and_back],
            ECHO * self.b[0] + FAR * self.a[self.delay] + TALKER * self.b[there_and_back],
        )
    }
}

/// Run a call over a line of the given one-way delay in samples.
fn long_call(seconds: f64, delay: usize) -> (Modem, Modem, f64) {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut line = Line::new(delay);
    let (mut from_calling, mut from_answering) = (0.0, 0.0);
    let mut at = f64::NAN;

    for i in 0..(seconds * FS) as usize {
        let (to_calling, to_answering) = line.step(from_calling, from_answering);
        from_calling = calling.step(to_calling);
        from_answering = answering.step(to_answering);
        if at.is_nan()
            && matches!(calling.status(), Status::Connected(_))
            && matches!(answering.status(), Status::Connected(_))
        {
            at = i as f64 / FS;
        }
    }
    (calling, answering, at)
}

/// Twenty milliseconds each way, which is a few hundred miles of it.
const DELAY: usize = 320;

#[test]
fn a_call_completes_over_a_line_with_length() {
    let (calling, answering, at) = long_call(40.0, DELAY);
    println!(
        "connected after {at:.2} s; round trip {} and {} symbols; \
         echo return loss {:.1} and {:.1} dB",
        calling.round_trip(),
        answering.round_trip(),
        calling.echo_return_loss(),
        answering.echo_return_loss(),
    );
    for (name, modem) in [("calling", &calling), ("answering", &answering)] {
        println!("{name} found {:?}", modem.reflection());
        assert_eq!(
            modem.status(),
            Status::Connected(4800),
            "the {name} end stopped at {} ({:?})",
            modem.phase(),
            modem.status()
        );
    }
    assert!(at.is_finite(), "never both connected at once");
}

#[test]
fn the_far_hybrid_is_found_where_it_actually_is() {
    // The point of measuring the round trip. What comes back off the far end
    // arrives a whole trip later, and taps placed anywhere else model nothing.
    let (calling, answering, _) = long_call(40.0, DELAY);
    for (name, modem) in [("calling", &calling), ("answering", &answering)] {
        let found = modem
            .reflection()
            .unwrap_or_else(|| panic!("the {name} end found no reflection at all"));
        let off = found.delay as i64 - 2 * DELAY as i64;
        assert!(
            off.abs() <= 8,
            "the {name} end put the far hybrid {off} samples from where it is"
        );
        assert!(
            found.strength > 0.3,
            "the {name} end found it at only {:.2} of what arrives",
            found.strength
        );
    }
}

#[test]
fn the_second_run_of_taps_is_what_makes_the_long_line_work() {
    // The whole case for the split canceller. Both ends are cancelling the
    // near hybrid either way; the difference is whether the far one is left
    // on the line, and on this line it is eight decibels below the far modem.
    let (calling, answering, _) = long_call(40.0, DELAY);
    for (name, modem) in [("calling", &calling), ("answering", &answering)] {
        let loss = modem.echo_return_loss();
        assert!(
            loss > 20.0,
            "the {name} end removed only {loss:.1} dB of its own echo, which \
             is about what the near taps manage on their own"
        );
    }
}

#[test]
#[ignore]
fn trace_cable() {
    // The sound-card loopback at the data pump level, where the receiver can
    // be seen: both modems summed onto one wire and heard by both, delayed.
    use std::collections::VecDeque;
    const CROSSING: usize = 700;
    const HEADROOM: f64 = 0.45;
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut wire: VecDeque<f64> = VecDeque::from(vec![0.0; CROSSING]);
    let (mut cp, mut ap) = ("", "");
    for i in 0..(25.0 * FS) as usize {
        let heard = wire.pop_front().unwrap_or(0.0);
        let a = calling.step(heard);
        let b = answering.step(heard);
        wire.push_back((a + b) * HEADROOM);
        let changed = calling.phase() != cp || answering.phase() != ap;
        if changed || i % (FS as usize / 2) == 0 {
            cp = calling.phase();
            ap = answering.phase();
            println!(
                "{:>7.3}s  call {cp:>12} err {:>6.3}   answer {ap:>12} err {:>6.3}",
                i as f64 / FS,
                calling.residual_error(),
                answering.residual_error(),
            );
        }
    }
}
