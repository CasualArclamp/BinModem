//! Two V.22bis modems connecting to each other.
//!
//! The two run on one line, as they do on a real two-wire circuit: each sample
//! either modem hears is the sum of what both are sending, so each has to pick
//! its own direction out of the other's presence rather than being handed a
//! clean signal. Nothing tells either end what the other can do.

use datapump::v22bis::handshake::{Handshake, Role, Status};
use datapump::v22bis::{Channel, Rate, Receiver, Transmitter};

const FS: f64 = 16_000.0;

/// One end of the call.
struct Modem {
    tx: Transmitter,
    rx: Receiver,
    hs: Handshake,
    /// The most recent sample this end put on the line.
    out: f64,
    received: Vec<u8>,
}

impl Modem {
    fn new(role: Role) -> Self {
        // A channel is named for the end that uses it, and it says both what
        // that end transmits in and what it listens to: the calling modem
        // transmits low and receives high, the answering modem the reverse.
        let mine = match role {
            Role::Calling => Channel::Calling,
            Role::Answering => Channel::Answering,
        };
        Self {
            tx: Transmitter::at_rate(mine, Rate::Bps1200, FS),
            rx: Receiver::new(mine, FS),
            hs: Handshake::new(role, FS),
            out: 0.0,
            received: Vec::new(),
        }
    }
}

/// Run both ends until they connect or the line goes quiet, and return how far
/// each got along with how long it took.
fn call(seconds: f64) -> (Status, Status, f64) {
    let mut calling = Modem::new(Role::Calling);
    let mut answering = Modem::new(Role::Answering);
    let mut connected_at = f64::NAN;

    for i in 0..(seconds * FS) as usize {
        // Everything on the line at this instant: both directions summed.
        let line = calling.out + answering.out;
        for m in [&mut calling, &mut answering] {
            m.rx.feed(line);
            m.received.extend(m.rx.take_bytes());
            m.hs.step(&mut m.tx, &mut m.rx);
            m.out = m.tx.next_sample();
        }
        if connected_at.is_nan()
            && matches!(calling.hs.status(), Status::Connected(_))
            && matches!(answering.hs.status(), Status::Connected(_))
        {
            connected_at = i as f64 / FS;
        }
    }
    (calling.hs.status(), answering.hs.status(), connected_at)
}

#[test]
fn two_modems_agree_on_2400() {
    // Both can do it, so the handshake should reach it: 6.3.1.1 exists for
    // exactly this case and every other path in clause 6.3 is a fallback.
    let (calling, answering, at) = call(8.0);
    assert_eq!(calling, Status::Connected(Rate::Bps2400), "the calling end");
    assert_eq!(
        answering,
        Status::Connected(Rate::Bps2400),
        "the answering end"
    );
    assert!(
        at.is_finite() && at < 7.0,
        "took {at} s, which is longer than the sequence allows for"
    );
}

#[test]
fn the_answering_tone_comes_first_and_at_2100_hertz() {
    // V.22bis 6.2 requires the V.25 answering sequence on the international
    // network, and it is the only thing on the line to begin with: the calling
    // modem is required to stay silent until it has heard something.
    let mut calling = Modem::new(Role::Calling);
    let mut answering = Modem::new(Role::Answering);
    let mut line = Vec::new();
    for _ in 0..(2.0 * FS) as usize {
        let sample = calling.out + answering.out;
        line.push(sample);
        for m in [&mut calling, &mut answering] {
            m.rx.feed(sample);
            m.hs.step(&mut m.tx, &mut m.rx);
            m.out = m.tx.next_sample();
        }
    }
    // Nothing but the tone, so correlating against it should account for
    // essentially all of the energy.
    let window = &line[FS as usize..];
    let at = |f: f64| {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &s) in window.iter().enumerate() {
            let w = std::f64::consts::TAU * f * n as f64 / FS;
            re += s * w.cos();
            im -= s * w.sin();
        }
        (re * re + im * im).sqrt() / window.len() as f64
    };
    let tone = at(2100.0);
    assert!(tone > 0.4, "the answering tone is only {tone:.3} strong");
    for other in [1200.0, 1800.0, 2400.0, 2700.0] {
        assert!(
            at(other) < tone / 50.0,
            "{other} Hz carries {:.4} against {tone:.4} at the answering tone",
            at(other)
        );
    }
}

#[test]
fn the_calling_modem_says_nothing_until_it_is_spoken_to() {
    // 6.3.1.1.1 a): it is conditioned to receive, and stays silent. A modem
    // that starts talking over the answering tone would be heard by the
    // network as well as by the far end.
    let mut calling = Modem::new(Role::Calling);
    let mut answering = Modem::new(Role::Answering);
    for _ in 0..(2.0 * FS) as usize {
        let line = calling.out + answering.out;
        for m in [&mut calling, &mut answering] {
            m.rx.feed(line);
            m.hs.step(&mut m.tx, &mut m.rx);
            m.out = m.tx.next_sample();
        }
        assert_eq!(
            calling.out, 0.0,
            "the calling modem transmitted during the answering tone"
        );
    }
}

#[test]
fn data_flows_once_the_handshake_is_done() {
    // The point of the exercise. Connect, then send in both directions and
    // read it back at the other end.
    let mut calling = Modem::new(Role::Calling);
    let mut answering = Modem::new(Role::Answering);
    let (to_host, to_caller) = (b"login: cactus\r\n", b"Password:");
    let (mut queued, mut settled) = (false, f64::NAN);

    for i in 0..(14.0 * FS) as usize {
        let line = calling.out + answering.out;
        for m in [&mut calling, &mut answering] {
            m.rx.feed(line);
            m.received.extend(m.rx.take_bytes());
            m.hs.step(&mut m.tx, &mut m.rx);
            m.out = m.tx.next_sample();
        }
        let up = matches!(calling.hs.status(), Status::Connected(_))
            && matches!(answering.hs.status(), Status::Connected(_));
        if up && settled.is_nan() {
            settled = i as f64 / FS;
        }
        // Let both ends settle before speaking: the receivers have equalisers
        // to converge and a descrambler to synchronise, and scrambled ones are
        // what does it.
        if up && !queued && i as f64 / FS > settled + 0.5 {
            queued = true;
            calling.tx.push_bytes(to_host);
            answering.tx.push_bytes(to_caller);
        }
    }

    assert!(queued, "never connected, so nothing was sent");
    assert!(
        contains_at_any_bit_offset(&answering.received, to_host),
        "the answering end did not receive what was typed; got {:02x?}",
        &answering.received[answering.received.len().saturating_sub(64)..]
    );
    assert!(
        contains_at_any_bit_offset(&calling.received, to_caller),
        "the calling end did not receive the host's reply"
    );
}

#[test]
fn a_modem_that_hears_nothing_gives_up() {
    // No far end at all: the calling modem has to stop waiting eventually,
    // because something upstream is waiting on it.
    let mut calling = Modem::new(Role::Calling);
    let mut rx = Receiver::new(Channel::Answering, FS);
    let mut status = Status::Negotiating;
    // Rather than run a full minute of silence, step the handshake with the
    // clock it was given: it counts samples, not wall time.
    for _ in 0..(61.0 * FS) as usize {
        rx.feed(0.0);
        status = calling.hs.step(&mut calling.tx, &mut rx);
        if status == Status::Failed {
            break;
        }
    }
    assert_eq!(status, Status::Failed);
}

#[test]
fn a_hybrid_puts_our_own_signal_over_the_far_one_and_it_still_connects() {
    // A two-wire line is joined to the four-wire innards of a modem by a
    // hybrid transformer, which is never perfectly balanced: some of what we
    // transmit comes straight back at us. Twelve decibels down is ordinary.
    // Meanwhile the far end has crossed the network and arrives twenty down.
    // Our own echo is therefore the loudest thing on the line by eight
    // decibels, and the receiver has to work under it.
    //
    // For V.22bis nothing special is needed, because the two directions are in
    // different bands and the filter that separates them removes the echo as a
    // side effect of removing the far channel. That is only true of a modem
    // duplexed by frequency; V.32 and above share the band and have to cancel
    // the echo instead.
    const ECHO: f64 = 0.251; // -12 dB
    const FAR: f64 = 0.1; //  -20 dB

    let mut calling = Modem::new(Role::Calling);
    let mut answering = Modem::new(Role::Answering);
    for _ in 0..(8.0 * FS) as usize {
        let (from_calling, from_answering) = (calling.out, answering.out);
        calling.rx.feed(from_answering * FAR + from_calling * ECHO);
        calling.hs.step(&mut calling.tx, &mut calling.rx);
        calling.out = calling.tx.next_sample();

        answering.rx.feed(from_calling * FAR + from_answering * ECHO);
        answering.hs.step(&mut answering.tx, &mut answering.rx);
        answering.out = answering.tx.next_sample();
    }
    assert_eq!(
        calling.hs.status(),
        Status::Connected(Rate::Bps2400),
        "the calling end did not get through its own echo"
    );
    assert_eq!(
        answering.hs.status(),
        Status::Connected(Rate::Bps2400),
        "the answering end did not get through its own echo"
    );
}

/// Find `needle` in `haystack` at any bit offset, since a receiver has no way
/// to know where the far end considered a byte to begin.
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
