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
