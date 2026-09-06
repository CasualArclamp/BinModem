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
    let crossing: usize = std::env::var("V32_CROSSING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(700);
    const HEADROOM: f64 = 0.45;
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut wire: VecDeque<f64> = VecDeque::from(vec![0.0; crossing]);
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

/// Run a call with an offer at each end and report what happened.
///
/// Returns the rate agreed, the bits per second the calling end actually
/// recovered once settled, and what each terminal received.
fn exchange(calling: u16, answering: u16, payload: &[u8]) -> (u32, f64, Vec<u8>, Vec<u8>) {
    let mut caller = Modem::new(Role::Calling, calling, FS);
    let mut host = Modem::new(Role::Answering, answering, FS);
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let (mut sent, mut settled) = (false, f64::NAN);
    let (mut counting_from, mut counted) = (f64::NAN, 0usize);

    for i in 0..(40.0 * FS) as usize {
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b * FAR + a * ECHO);
        from_host = host.step(a * FAR + b * ECHO);
        let now = i as f64 / FS;

        // One drain, used for both jobs: take_bits and take_bytes share a
        // buffer, so calling both leaves the second with nothing.
        let arrived = caller.take_bytes();
        let bits = arrived.len() * 8;
        at_caller.extend(arrived);
        at_host.extend(host.take_bytes());

        let up = matches!(caller.status(), Status::Connected(_))
            && matches!(host.status(), Status::Connected(_));
        if up && settled.is_nan() {
            settled = now;
        }
        // Count over a whole second, starting once both ends have settled and
        // are sending scrambled ones, which run at the agreed rate like data.
        if up && now > settled + 0.5 {
            if counting_from.is_nan() {
                counting_from = now;
            } else if now < counting_from + 1.0 {
                counted += bits;
            }
        }
        if up && !sent && now > settled + 1.5 {
            sent = true;
            caller.send(payload);
            host.send(payload);
        }
    }

    let rate = match caller.status() {
        Status::Connected(r) => r,
        other => panic!("the call did not connect: {other:?}"),
    };
    (rate, counted as f64, at_caller, at_host)
}

#[test]
fn nine_thousand_six_hundred_carries_four_bits_to_the_symbol() {
    // 2.4.1.1: the scrambled stream in groups of four, two differentially
    // encoded into the quadrant and two choosing a point inside it. Twice the
    // data at the same 2400 baud, which is the whole of what the extra twelve
    // points buy.
    let both = rate_signal(true, true);
    let payload = b"the quick brown fox jumps over the lazy dog, 0123456789";
    let (rate, bits, at_caller, at_host) = exchange(both, both, payload);

    assert_eq!(rate, 9600, "the two ends did not settle on the faster rate");
    assert!(
        (9000.0..10_200.0).contains(&bits),
        "the line carried {bits:.0} bit/s, which is not 9600"
    );
    assert!(
        contains_at_any_bit_offset(&at_host, payload),
        "the answering end did not receive what was sent at 9600"
    );
    assert!(
        contains_at_any_bit_offset(&at_caller, payload),
        "the calling end did not receive what was sent at 9600"
    );
}

#[test]
fn a_far_end_that_can_only_do_4800_gets_4800() {
    // 5.3: each rate signal narrows what the last one offered, and R3 settles
    // it. The E that follows has to carry what was settled rather than what
    // was offered -- Table 7 says its rate bits "relate to the transmission of
    // scrambled binary ones immediately following signal E" -- because it is
    // the E that tells the far end how to demodulate what comes next. A modem
    // that put its whole offer in E would tell this one to read 9600 off a
    // line carrying 4800.
    let payload = b"login: cactus";
    let (rate, bits, at_caller, at_host) =
        exchange(rate_signal(true, true), rate_signal(true, false), payload);

    assert_eq!(rate, 4800, "the faster end did not come down to the slower");
    assert!(
        (4400.0..5200.0).contains(&bits),
        "the line carried {bits:.0} bit/s, which is not 4800"
    );
    assert!(contains_at_any_bit_offset(&at_host, payload));
    assert!(contains_at_any_bit_offset(&at_caller, payload));
}

/// As `long_call`, but with the two reflections given rather than fixed.
fn custom_call(
    seconds: f64,
    delay: usize,
    echo: f64,
    far: f64,
) -> (Modem, Modem, f64) {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut a = std::collections::VecDeque::from(vec![0.0; 2 * delay + 1]);
    let mut b = std::collections::VecDeque::from(vec![0.0; 2 * delay + 1]);
    let (mut from_calling, mut from_answering) = (0.0, 0.0);
    let mut at = f64::NAN;
    for i in 0..(seconds * FS) as usize {
        a.pop_back();
        a.push_front(from_calling);
        b.pop_back();
        b.push_front(from_answering);
        let there_and_back = 2 * delay;
        let to_calling = echo * a[0] + far * b[delay] + TALKER * a[there_and_back];
        let to_answering = echo * b[0] + far * a[delay] + TALKER * b[there_and_back];
        from_calling = calling.step(to_calling);
        from_answering = answering.step(to_answering);
        if at.is_nan() && matches!(calling.status(), Status::Connected(_)) {
            at = i as f64 / FS;
        }
    }
    (calling, answering, at)
}

#[test]
fn a_call_survives_the_round_trip_a_packet_network_adds() {
    // The line this was written against is a hybrid twenty milliseconds away.
    // A call carried over VoIP is nothing like that: the round trip is
    // hundreds of milliseconds, most of it jitter buffer, and V.32 measures
    // the round trip in clause 5.4 precisely because it has to place the
    // canceller's second run of taps at the far hybrid.
    for round_trip_ms in [40.0, 125.0, 200.0, 300.0, 400.0] {
        let one_way = (round_trip_ms / 2.0 / 1000.0 * FS) as usize;
        let (calling, _, at) = custom_call(30.0, one_way, ECHO, FAR);
        assert!(
            matches!(calling.status(), Status::Connected(4800)),
            "a {round_trip_ms:.0} ms round trip left the call at {:?}",
            calling.status()
        );
        assert!(at < 20.0, "{round_trip_ms:.0} ms took {at:.1} s to connect");
    }
}

#[test]
fn a_call_survives_an_echo_as_loud_as_what_was_sent() {
    // What one virtual cable returns, measured on a recorded call: our own
    // transmit came back at +0.26 dB, where a hybrid would have given -12, and
    // the far end arrived only 5 dB below it. There is no hybrid in a cable --
    // what is written to it is what comes back -- so the canceller is doing all
    // the work rather than finishing what a transformer started.
    //
    // It manages, and it is worth knowing that it manages, because it means a
    // V.32 call that will not come up on such a line is not failing for want of
    // a quieter transmitter.
    const CABLE_ECHO: f64 = 1.03;
    const TRUNK_FAR: f64 = 0.575;
    for round_trip_ms in [40.0, 200.0, 400.0] {
        let one_way = (round_trip_ms / 2.0 / 1000.0 * FS) as usize;
        let (calling, _, at) = custom_call(30.0, one_way, CABLE_ECHO, TRUNK_FAR);
        assert!(
            matches!(calling.status(), Status::Connected(4800)),
            "an echo at unity over {round_trip_ms:.0} ms left the call at {:?}",
            calling.status()
        );
        assert!(at < 20.0, "took {at:.1} s to connect");
    }
}

/// A call where the answering modem does not start until `quiet_ms` in.
///
/// Which is every real call: the calling modem goes off hook, the network
/// takes its time, and the far end answers when it answers.
fn late_call(
    seconds: f64,
    delay: usize,
    echo: f64,
    far: f64,
    quiet_ms: f64,
) -> (Modem, f64) {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut a = std::collections::VecDeque::from(vec![0.0; 2 * delay + 1]);
    let mut b = std::collections::VecDeque::from(vec![0.0; 2 * delay + 1]);
    let (mut from_calling, mut from_answering) = (0.0, 0.0);
    let mut at = f64::NAN;
    let starts = (quiet_ms / 1000.0 * FS) as usize;
    for i in 0..(seconds * FS) as usize {
        a.pop_back();
        a.push_front(from_calling);
        b.pop_back();
        b.push_front(from_answering);
        let there_and_back = 2 * delay;
        let to_calling = echo * a[0] + far * b[delay] + TALKER * a[there_and_back];
        let to_answering = echo * b[0] + far * a[delay] + TALKER * b[there_and_back];
        from_calling = calling.step(to_calling);
        // The far end is not on the line yet.
        from_answering = if i < starts { answering.step(0.0) * 0.0 } else { answering.step(to_answering) };
        if at.is_nan() && matches!(calling.status(), Status::Connected(_)) {
            at = i as f64 / FS;
        }
    }
    (calling, at)
}

#[test]
fn a_quiet_far_end_is_heard_through_an_echo_at_full_strength() {
    // The stage a calling modem used to sit in forever on a virtual cable.
    //
    // In AA it transmits state A, which puts everything at the carrier and
    // nothing at the sidebands, and listens at the sidebands for the far end
    // to reverse its alternation. It is meant to be deaf to its own reflection
    // by construction. It was not: the tone detectors were a single pole, which
    // falls away at six decibels an octave, so a twentieth of the carrier still
    // reached the sideband detector 1200 Hz away. Behind a hybrid that is
    // twelve decibels down already and does not matter; on a cable the echo
    // comes back whole, and a twentieth of it is a steady phasor large enough
    // that the far end reversing its phase barely moved the sum.
    //
    // The modem waited for a reversal it could no longer see, which is exactly
    // what it looked like from the outside: sometimes it does not detect the
    // answering modem and the call never starts.
    const CABLE_ECHO: f64 = 1.03;
    for quiet_ms in [0.0, 500.0, 2000.0] {
        let (calling, _) = late_call(12.0, 320, CABLE_ECHO, 0.1, quiet_ms);
        let phase = calling.phase();
        assert!(
            !matches!(phase, "listening" | "AA"),
            "after a {quiet_ms:.0} ms pause the calling modem was still in {phase}"
        );
    }
}

/// Play an answering tone at `db` and report the loudest thing we said back.
///
/// `reversal_s` is how often the tone turns its phase over, which is what
/// separates V.25's answering tone from V.8's, and `am` whether it also carries
/// V.8's fifteen hertz of amplitude modulation.
fn answered_with(reversal_s: f64, am: bool, db: f64) -> (f64, &'static str) {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let level = 0.3 * 10.0f64.powf(db / 20.0);
    let mut peak = 0.0f64;
    let mut phase = 0.0f64;
    for i in 0..(FS * 4.0) as usize {
        let t = i as f64 / FS;
        let flips = if reversal_s > 0.0 { (t / reversal_s) as u64 } else { 0 };
        let sign = if flips % 2 == 0 { 1.0 } else { -1.0 };
        let envelope = if am {
            1.0 + 0.2 * (std::f64::consts::TAU * 15.0 * t).sin()
        } else {
            1.0
        };
        phase += std::f64::consts::TAU * 2100.0 / FS;
        let out = calling.step(level * envelope * sign * phase.sin());
        // After the second 5.4.1 requires, and a little to spare.
        if t > 1.5 {
            peak = peak.max(out.abs());
        }
    }
    (peak, calling.phase())
}

#[test]
fn a_modern_answering_tone_still_gets_answered() {
    // 5.4.1: having heard the answering tone for a second, the calling modem
    // "shall repetitively transmit carrier state A". It starts talking off the
    // answering tone alone, before any 600 or 3000 Hz tone has arrived.
    //
    // V.25's answering tone is a plain 2100 Hz. V.8's -- which is what every
    // answering modem made since 1994 sends -- is the same tone with a phase
    // reversal every 450 ms, and the reversals are the entire point of it:
    // they are how the far end says it can do V.8.
    //
    // A reversal takes a tone detector's phasor through zero, so measured on
    // the phasor the tone stops existing for a few milliseconds twice a
    // second. The second it has to be heard for arrived in 450 ms instalments
    // and the counter went back to nought at every one, so the calling modem
    // stayed mute -- and the far end, hearing nothing, took it for something
    // that was not a V.32 modem and moved on. From the outside, a call that
    // never starts and a modem that never transmits.
    //
    // Judged on the envelope instead, a reversal is a ripple.
    for db in [0.0, -6.0, -12.0, -20.0, -26.0] {
        for (what, reversal, am) in [
            ("V.25, plain", 0.0, false),
            ("V.8 ANSam, reversals", 0.450, false),
            ("V.8 ANSam, reversals and AM", 0.450, true),
        ] {
            let (peak, phase) = answered_with(reversal, am, db);
            assert!(
                peak > 1.0e-3,
                "{what} at {db:.0} dB left us silent, still in {phase}"
            );
        }
    }
}

#[test]
fn a_line_with_nothing_on_it_is_not_answered() {
    // The other half of it. Riding through a reversal must not turn into
    // hearing a tone that was never there: 5.4.1 has the calling modem silent
    // until the far end speaks, and a modem that transmits into silence would
    // be talking over the answering tone it is supposed to be waiting for.
    let (peak, phase) = answered_with(0.0, false, -120.0);
    assert!(peak < 1.0e-6, "transmitted into a silent line, reaching {phase}");
    assert_eq!(phase, "listening");
}

