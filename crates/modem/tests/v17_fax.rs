//! V.17 in a whole fax call, through the modem crate.
//!
//! Two of our ends that both offer V.17 put it in their DIS, and T.30 picks
//! its fastest rung, 14 400 (`fax::t30`'s `best_shared`). The page then goes
//! as the Recommendations have it: a long train in front of the training
//! check, a resync in front of the page (T.30 5.1, Note 5), the receiver
//! reading the resync with the taps the long train left. A line too noisy
//! for 14 400 fails its training check, and the call steps down the ladder
//! until one passes.
//!
//! The figures each test measures are printed; `cargo test -p modem
//! --release --test v17_fax -- --nocapture` shows them.

use std::f64::consts::TAU;

use fax::call::Phase;
use fax::page::{Page, Resolution};
use fax::t30::Modulation;
use modem::{FaxCall, Modem};

const FS: f64 = 16_000.0;

const WITH_V17: [Modulation; 3] = [Modulation::V27ter, Modulation::V29, Modulation::V17];

/// A page with something recognisable on it, as `faxcall.rs`'s tests draw.
fn a_page(lines: usize) -> Page {
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines)
            .map(|y| (0..width).map(|x| (x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30).collect())
            .collect(),
        resolution: Resolution::Standard,
    }
}

/// A seeded generator: xorshift64*, with Box-Muller.
struct Rng(u64);

impl Rng {
    fn gaussian(&mut self) -> f64 {
        let mut next = || {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        };
        let (u1, u2) = (next().max(1e-18), next());
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

/// What a call came to.
#[derive(Debug)]
struct Outcome {
    page: Option<Page>,
    /// Every speed a training check or a page went out at, in order.
    speeds: Vec<(Modulation, u32)>,
    /// What the answering end's scope was told it was drawing, in order.
    shapes: Vec<&'static str>,
}

/// Two fax calls against each other, with white noise on the line both ways
/// at `snr_db` of Es/N0 against a page carrier's power, which is 0.707 root
/// mean square from every transmitter here.
fn call(caller: &mut FaxCall, answerer: &mut FaxCall, snr_db: f64, seconds: f64) -> Outcome {
    let sigma = if snr_db.is_finite() { ((FS / 2.0) / 2400.0 * 0.5 * 10f64.powf(-snr_db / 10.0)).sqrt() } else { 0.0 };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut to_caller, mut to_answerer) = (0.0, 0.0);
    let mut outcome = Outcome { page: None, speeds: Vec::new(), shapes: Vec::new() };
    let mut was = Phase::Calling;
    for _ in 0..(seconds * FS) as usize {
        let a = caller.step(to_caller);
        let b = answerer.step(to_answerer);
        to_caller = b + sigma * rng.gaussian();
        to_answerer = a + sigma * rng.gaussian();
        let phase = caller.phase();
        if phase != was && matches!(phase, Phase::Training | Phase::Sending) {
            let speed = caller.speed();
            outcome.speeds.push((speed.modulation, speed.bits_per_second));
        }
        was = phase;
        let shape = answerer.shape();
        if outcome.shapes.last() != Some(&shape) {
            outcome.shapes.push(shape);
        }
        if outcome.page.is_none() {
            outcome.page = answerer.take_received().map(|(_, page)| page);
        }
        if caller.phase().is_over() && answerer.phase().is_over() {
            break;
        }
    }
    outcome
}

#[test]
fn two_ends_offering_v17_send_the_page_at_14400() {
    // With error correction and without: under it the page is a burst of
    // frames, without it the page itself, and both follow a resync.
    for ecm in [true, false] {
        let page = a_page(40);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()))
            .offering(&WITH_V17)
            .with_error_correction(ecm);
        let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17).with_error_correction(ecm);
        let got = call(&mut caller, &mut answerer, f64::INFINITY, 60.0);
        eprintln!("error correction {ecm}: speeds {:?}, the scope drew {:?}", got.speeds, got.shapes);
        assert_eq!(caller.phase(), Phase::Done, "the caller ended at {} ({:?})", caller.phase().name(), caller.trouble());
        assert_eq!(answerer.phase(), Phase::Done, "the answerer ended at {} ({:?})", answerer.phase().name(), answerer.trouble());
        let arrived = got.page.expect("no page arrived");
        assert_eq!(arrived.lines, page.lines, "the page came out different");
        // The training check and the page, both at the top rate, first time.
        assert_eq!(got.speeds, vec![(Modulation::V17, 14_400); 2], "error correction {ecm}");
        assert!(got.shapes.contains(&"128TCM"), "{:?}", got.shapes);
        assert_eq!(caller.error_correction(), ecm);
    }
}

#[test]
fn a_page_crosses_at_v17_between_two_modems() {
    // The whole modem: AT+FCLASS=1, a dial and an answer, and the fax offer
    // the window's V.17 box puts in `fax_offer`.
    let page = a_page(24);
    let mut caller = Modem::new(FS);
    caller.fax_page = Some(page.clone());
    caller.fax_offer = WITH_V17.to_vec();
    let mut answerer = Modem::new(FS);
    answerer.fax_offer = WITH_V17.to_vec();
    for (modem, dial) in [(&mut caller, "ATD1"), (&mut answerer, "ATA")] {
        for line in ["AT+FCLASS=1", dial] {
            for b in line.bytes() {
                modem.feed_dte(b);
            }
            modem.feed_dte(b'\r');
        }
    }
    let (mut to_caller, mut to_answerer) = (0.0, 0.0);
    let mut arrived = None;
    let mut speed = None;
    for _ in 0..(FS * 40.0) as usize {
        let a = caller.step(to_caller);
        let b = answerer.step(to_answerer);
        to_caller = b;
        to_answerer = a;
        let _ = caller.take_dte();
        let _ = answerer.take_dte();
        if let Some(call) = caller.fax_call()
            && call.phase() == Phase::Sending
        {
            speed = Some((call.speed().modulation, call.speed().bits_per_second));
        }
        if arrived.is_none() {
            arrived = answerer.take_received_page().map(|(_, page)| page);
        }
        let over = |m: &Modem| m.fax_call().is_some_and(|c| c.phase().is_over());
        if arrived.is_some() && over(&caller) && over(&answerer) {
            break;
        }
    }
    assert_eq!(arrived.expect("no page arrived").lines, page.lines, "the page came out different");
    assert_eq!(speed, Some((Modulation::V17, 14_400)));
}

#[test]
fn a_line_that_will_not_carry_14400_steps_down_to_12000_or_9600() {
    // 14 400's working signal to noise is 27 dB. At 20 dB its training check
    // is refused and 12 000 carries the page; at 16 dB 12 000 is refused too,
    // and the page goes at 9600. The ladder puts V.29's 9600 ahead of V.17's,
    // and measured, V.29's is refused there and V.17's, trellis coded, is not.
    for (snr, want) in [(20.0, 12_000), (16.0, 9600)] {
        let page = a_page(40);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone())).offering(&WITH_V17);
        let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17);
        let got = call(&mut caller, &mut answerer, snr, 120.0);
        eprintln!("{snr} dB: speeds {:?}", got.speeds);
        let arrived = got.page.unwrap_or_else(|| panic!("{snr} dB: no page arrived ({:?})", caller.trouble()));
        assert_eq!(arrived.lines, page.lines, "{snr} dB: the page came out different");
        assert_eq!(got.speeds.first(), Some(&(Modulation::V17, 14_400)), "{snr} dB: V.17 14 400 was not tried first");
        assert!(got.speeds.iter().all(|(_, rate)| *rate >= want), "{snr} dB: went below {want}: {:?}", got.speeds);
        assert_eq!(caller.speed().bits_per_second, want, "{snr} dB: the page went at {:?}", caller.speed());
    }
}

#[test]
fn v17_goes_only_where_both_ends_offer_it() {
    // One end offering it and the other not is V.29 at 9600, as before.
    let page = a_page(16);
    let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone())).offering(&WITH_V17);
    let mut answerer = FaxCall::answer(FS, "61388880000");
    let got = call(&mut caller, &mut answerer, f64::INFINITY, 40.0);
    assert_eq!(got.page.expect("no page arrived").lines, page.lines);
    assert_eq!(got.speeds.first(), Some(&(Modulation::V29, 9600)));
}
