//! Two V.32 modems finding each other.
//!
//! The first test uses a line that carries each direction to the other end and
//! nothing back, which is not a two-wire line but does isolate the start-up
//! from the echo. The later ones put the hybrid back.

use datapump::v32::startup::{Heard, Role, Startup, Status, endpoints, rate_signal};
use datapump::v32::{BAUD, Receiver, Transmitter};

const FS: f64 = 16_000.0;

/// One end of the call.
struct Modem {
    tx: Transmitter,
    rx: Receiver,
    up: Startup,
    /// The most recent sample this end put on the line.
    out: f64,
}

impl Modem {
    fn new(role: Role, offer: u16) -> Self {
        let (tx, rx) = endpoints(role, FS);
        Self {
            tx,
            rx,
            up: Startup::new(role, offer, FS),
            out: 0.0,
        }
    }
}

/// Run a call for `seconds` and report where each end got to.
///
/// `hear` is given (what the far end sent, what this end sent) and returns
/// what arrives at this end's receiver, which is how the hybrid is modelled.
fn call(seconds: f64, hear: impl Fn(f64, f64) -> f64) -> (Modem, Modem, f64) {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer);
    let mut answering = Modem::new(Role::Answering, offer);
    let mut connected_at = f64::NAN;

    for i in 0..(seconds * FS) as usize {
        let (from_calling, from_answering) = (calling.out, answering.out);
        calling.up.step(
            hear(from_answering, from_calling),
            &mut calling.tx,
            &mut calling.rx,
        );
        calling.out = calling.tx.next_sample();
        answering.up.step(
            hear(from_calling, from_answering),
            &mut answering.tx,
            &mut answering.rx,
        );
        answering.out = answering.tx.next_sample();

        if connected_at.is_nan()
            && matches!(calling.up.status(), Status::Connected(_))
            && matches!(answering.up.status(), Status::Connected(_))
        {
            connected_at = i as f64 / FS;
        }
    }
    (calling, answering, connected_at)
}

/// A line with no reflection at all: each end hears only the other.
fn clean(far: f64, _near: f64) -> f64 {
    far
}

#[test]
fn two_modems_reach_a_connection() {
    let (calling, answering, at) = call(30.0, clean);
    assert_eq!(
        calling.up.status(),
        Status::Connected(4800),
        "the calling end stopped at {:?}",
        calling.up.status()
    );
    assert_eq!(
        answering.up.status(),
        Status::Connected(4800),
        "the answering end stopped at {:?}",
        answering.up.status()
    );
    assert!(at.is_finite(), "never both connected at once");
    println!("connected after {at:.2} s");
}

#[test]
fn the_answering_modem_speaks_first_and_the_calling_one_waits() {
    // 5.4.1 a): the calling modem "shall initially remain silent", and joins in
    // only "after receiving the answer tone for a period of at least 1 s".
    // Talking over the answering tone would be heard by the network as well as
    // by the far end, and the tone is what disables the echo control devices
    // along the way.
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer);
    let mut answering = Modem::new(Role::Answering, offer);
    let mut first_sound = None;
    for i in 0..(2.0 * FS) as usize {
        let (a, b) = (calling.out, answering.out);
        calling.up.step(b, &mut calling.tx, &mut calling.rx);
        calling.out = calling.tx.next_sample();
        answering.up.step(a, &mut answering.tx, &mut answering.rx);
        answering.out = answering.tx.next_sample();
        if calling.out != 0.0 && first_sound.is_none() {
            first_sound = Some(i as f64 / FS);
        }
        if i as f64 / FS < 0.5 {
            assert_eq!(
                calling.out, 0.0,
                "the calling modem transmitted at {:.3} s, before it could \
                 have heard a second of tone",
                i as f64 / FS
            );
        }
        // From a tenth of a second, by which time the tone detectors have
        // settled. Before that there is genuinely nothing to hear yet.
        if (0.1..1.0).contains(&(i as f64 / FS)) {
            assert_eq!(
                calling.up.heard(),
                Heard::AnswerTone,
                "the calling modem is not hearing the answering tone"
            );
        }
    }
    let at = first_sound.expect("the calling modem never answered at all");
    assert!(
        (1.0..1.2).contains(&at),
        "the calling modem answered at {at:.3} s rather than just after the \
         second of tone 5.4.1 asks it to wait for"
    );
}

#[test]
fn the_round_trip_is_measured_and_the_line_delay_comes_out_of_it() {
    // The point of the AA/CC and AC/CA exchange. Each end reverses its own
    // transmission and waits to hear the far end's answering reversal come
    // back; what is left after taking off the 64 symbols the far end owes is
    // the time the line adds. An echo canceller needs it to know how far back
    // to look for a reflection.
    //
    // The hybrid has to be here. Without it each modem hears only the far end
    // and the measurement cannot go wrong; with it each modem also hears its
    // own reversal come straight back, which arrives first and is the wrong
    // one to stop the clock on. Both ends did exactly that, and read a round
    // trip of zero on a line hundreds of miles long, for as long as this test
    // ran on a line that could not reflect.
    const HYBRID: f64 = 0.251;
    let delay_symbols = 40usize;
    let delay = (delay_symbols as f64 * FS / BAUD) as usize;

    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer);
    let mut answering = Modem::new(Role::Answering, offer);
    let mut to_calling = vec![0.0; delay];
    let mut to_answering = vec![0.0; delay];

    for i in 0..(20.0 * FS) as usize {
        let heard_by_calling = to_calling[i % delay] + HYBRID * calling.out;
        let heard_by_answering = to_answering[i % delay] + HYBRID * answering.out;
        to_calling[i % delay] = answering.out;
        to_answering[i % delay] = calling.out;

        calling.up.step(heard_by_calling, &mut calling.tx, &mut calling.rx);
        calling.out = calling.tx.next_sample();
        answering
            .up
            .step(heard_by_answering, &mut answering.tx, &mut answering.rx);
        answering.out = answering.tx.next_sample();
    }

    // Each direction is delayed, so the round trip is twice it.
    let want = 2 * delay_symbols as u64;
    // Measured: both ends read 81 against a true 80, so the tolerance here is
    // about twenty times tighter than the error would be if any one of the
    // four overheads had been left out.
    for (name, got) in [
        ("calling", calling.up.round_trip()),
        ("answering", answering.up.round_trip()),
    ] {
        let out = got as i64 - want as i64;
        assert!(
            out.abs() <= 4,
            "the {name} end measured {got} symbols of round trip against {want}"
        );
    }
}

#[test]
fn a_modem_that_hears_nothing_gives_up() {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer);
    let mut status = Status::Negotiating;
    for _ in 0..(65.0 * FS) as usize {
        status = calling.up.step(0.0, &mut calling.tx, &mut calling.rx);
        if status == Status::Failed {
            break;
        }
    }
    assert_eq!(status, Status::Failed);
}

#[test]
#[ignore]
fn trace() {
    let offer = rate_signal(true, false);
    let mut calling = Modem::new(Role::Calling, offer);
    let mut answering = Modem::new(Role::Answering, offer);
    let (mut cp, mut ap) = ("", "");
    for i in 0..(30.0 * FS) as usize {
        let (a, b) = (calling.out, answering.out);
        calling.up.step(b, &mut calling.tx, &mut calling.rx);
        calling.out = calling.tx.next_sample();
        answering.up.step(a, &mut answering.tx, &mut answering.rx);
        answering.out = answering.tx.next_sample();
        if calling.up.phase() != cp || answering.up.phase() != ap {
            cp = calling.up.phase();
            ap = answering.up.phase();
            println!(
                "{:>7.3}s  call {cp:>12} (hears {:?})   answer {ap:>12} (hears {:?})",
                i as f64 / FS, calling.up.heard(), answering.up.heard()
            );
        }
    }
    println!("round trip: call {} answer {}", calling.up.round_trip(), answering.up.round_trip());
}
