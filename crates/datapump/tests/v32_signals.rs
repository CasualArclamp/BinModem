//! The signals V.32's start-up is conducted in, measured on the line.
//!
//! Every one of them is a fixed pattern of constellation states chosen for
//! what it looks like as a waveform, so what it looks like as a waveform is
//! the thing worth testing. Each modem recognises the other by these alone,
//! before either has a working demodulator: that is the point of them.

use datapump::v32::startup::Rates;
use datapump::v32::{BAUD, CARRIER, Mode, Signal, Transmitter};
use dsp::ReversalDetector;
use std::f64::consts::TAU;

const FS: f64 = 16_000.0;

/// Half the symbol rate, which is where an alternating pattern puts its
/// sidebands: 600 and 3000 Hz about the 1800 Hz carrier.
const OFFSET: f64 = BAUD / 2.0;

/// Amplitude at one frequency, over samples of one signal.
fn tone(samples: &[f64], freq: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &s) in samples.iter().enumerate() {
        let w = TAU * freq * n as f64 / FS;
        re += s * w.cos();
        im -= s * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / samples.len() as f64
}

/// Half a second of one signal, past the filter's start-up transient.
fn emit(signal: Signal) -> Vec<f64> {
    let mut tx = Transmitter::new(Mode::Call, FS);
    tx.set_signal(signal);
    let mut out: Vec<f64> = (0..(FS as usize / 2)).map(|_| tx.next_sample()).collect();
    out.drain(..(FS as usize / 50));
    out
}

/// The three places a start-up signal can put energy, and what is elsewhere.
fn lines(samples: &[f64]) -> (f64, f64, f64, f64) {
    let carrier = tone(samples, CARRIER);
    let low = tone(samples, CARRIER - OFFSET);
    let high = tone(samples, CARRIER + OFFSET);
    let elsewhere = [900.0, 1200.0, 1500.0, 2100.0, 2400.0, 2700.0]
        .iter()
        .map(|&f| tone(samples, f))
        .fold(0.0f64, f64::max);
    (carrier, low, high, elsewhere)
}

#[test]
fn a_repeated_state_is_the_bare_carrier() {
    // 5.4.1 has the calling modem repeat state A, and 5.4.2 has the answering
    // modem listen for "an incoming tone at 1800 Hz". A state that never
    // changes never turns the phasor, so nothing is left but the carrier.
    for signal in [Signal::StateA, Signal::StateC] {
        let (carrier, low, high, elsewhere) = lines(&emit(signal));
        assert!(carrier > 0.5, "{signal:?} carries only {carrier:.3} at 1800 Hz");
        for (name, level) in [("600", low), ("3000", high), ("elsewhere", elsewhere)] {
            assert!(
                level < carrier / 20.0,
                "{signal:?} puts {level:.4} at {name} against {carrier:.4} at the carrier"
            );
        }
    }
}

#[test]
fn alternating_opposite_states_suppresses_the_carrier() {
    // 5.4.2 has the answering modem alternate A and C, and 5.4.1 has the
    // calling modem listen for "one of two incoming tones at 600 Hz and
    // 3000 Hz". Both sidebands appear because the pattern repeats every two
    // symbols; what leaves nothing in between is that the two states are
    // opposite and average to nothing.
    for signal in [Signal::AlternateAC, Signal::AlternateCA] {
        let (carrier, low, high, elsewhere) = lines(&emit(signal));
        assert!(low > 0.3 && high > 0.3, "{signal:?}: {low:.3} and {high:.3}");
        assert!(
            (low / high).max(high / low) < 1.2,
            "{signal:?} is lopsided: {low:.3} against {high:.3}"
        );
        assert!(
            carrier < low / 20.0,
            "{signal:?} leaves {carrier:.4} at the carrier, so its two states \
             do not cancel"
        );
        assert!(elsewhere < low / 20.0, "{signal:?} spills {elsewhere:.4}");
    }
}

#[test]
fn the_conditioning_signal_leaves_the_carrier_standing() {
    // Segment 1 of 5.2 alternates A with B, a quarter turn apart rather than
    // half, so the pair averages to something rather than nothing and the
    // carrier survives. This is the contrast that makes the suppression above
    // mean what it is taken to mean: the two signals differ in exactly that.
    for signal in [Signal::ConditioningS, Signal::ConditioningSbar] {
        let (carrier, low, high, elsewhere) = lines(&emit(signal));
        assert!(
            carrier > 0.3,
            "{signal:?} suppressed the carrier at {carrier:.4}, which only an \
             opposite pair should do"
        );
        assert!(low > 0.2 && high > 0.2, "{signal:?}: {low:.3} and {high:.3}");
        assert!(elsewhere < carrier / 20.0, "{signal:?} spills {elsewhere:.4}");
    }
}

#[test]
fn the_change_between_the_two_alternations_is_a_phase_reversal() {
    // 5.4.2: the answering modem changes from AC to CA, and 5.4.1 has the
    // calling modem detect a phase reversal in the tone it is hearing. The
    // reversal is the timing mark the round trip is measured against, so if
    // the change did not produce one there would be nothing to measure.
    let mut tx = Transmitter::new(Mode::Answer, FS);
    tx.set_signal(Signal::AlternateAC);
    let mut d = ReversalDetector::new(CARRIER + OFFSET, 50.0, 0.05, FS);
    let mut at = Vec::new();
    for i in 0..(FS as usize) {
        // Change over halfway through, on a symbol boundary.
        if i == FS as usize / 2 {
            tx.set_signal(Signal::AlternateCA);
        }
        if d.feed(tx.next_sample()) {
            at.push(i);
        }
    }
    assert_eq!(at.len(), 1, "reversals found at {at:?}");
    let late = at[0] - FS as usize / 2;
    assert!(
        late < (0.05 * FS) as usize,
        "the reversal was reported {late} samples late, which is more than the \
         detector should need"
    );
}

#[test]
fn the_change_from_a_repeated_state_to_its_opposite_is_a_reversal() {
    // 5.4.1: the calling modem changes from repeating A to repeating C, and
    // "the time delay between the reception of this phase reversal at the line
    // terminals and the transmitted AA to CC transition appearing at the line
    // terminals shall be 64 plus or minus 2 symbol periods".
    let mut tx = Transmitter::new(Mode::Call, FS);
    tx.set_signal(Signal::StateA);
    let mut d = ReversalDetector::new(CARRIER, 50.0, 0.05, FS);
    let mut at = Vec::new();
    for i in 0..(FS as usize) {
        if i == FS as usize / 2 {
            tx.set_signal(Signal::StateC);
        }
        if d.feed(tx.next_sample()) {
            at.push(i);
        }
    }
    assert_eq!(at.len(), 1, "reversals found at {at:?}");
}

#[test]
fn the_training_segment_looks_like_noise_rather_than_a_tone() {
    // Segment 3 of 5.2 is scrambled ones with the differential encoding
    // disabled, and it is what the far equaliser and the near echo canceller
    // train on. Both need a signal that fills the band: an adaptive filter
    // learns nothing about frequencies its input does not visit, which is why
    // the segment is scrambled rather than being another fixed pattern.
    let samples = emit(Signal::Trn);
    let (carrier, low, high, elsewhere) = lines(&samples);
    let peak = carrier.max(low).max(high).max(elsewhere);
    for (name, level) in [
        ("1800", carrier),
        ("600", low),
        ("3000", high),
        ("elsewhere", elsewhere),
    ] {
        assert!(
            level < 0.15,
            "TRN stands at {level:.3} at {name}, which is a tone and not the \
             spread signal an equaliser can train on"
        );
    }
    // And there really is a signal there, spread rather than absent.
    let power: f64 = samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64;
    assert!(
        power > 0.05,
        "TRN carries almost no power at all: {power:.4}, peak line {peak:.4}"
    );
}

#[test]
fn a_rate_signal_repeats_its_sixteen_bits() {
    // 5.3: the rate signal is "a whole number of repeated 16-bit binary
    // sequences", scrambled and differentially encoded. Scrambling means the
    // states do not repeat, so what is checked here is that the same sequence
    // in gives the same states out when the scrambler is in the same place.
    let states = |sequence: u16| {
        let mut tx = Transmitter::new(Mode::Call, FS);
        tx.set_signal(Signal::Rate(sequence));
        let mut seen = Vec::new();
        let mut last = usize::MAX;
        for _ in 0..(FS as usize / 4) {
            tx.next_sample();
            if tx.state() != last {
                last = tx.state();
            }
            seen.push(tx.state());
        }
        seen
    };
    // Table 6 sync bits with 4800 and 9600 offered; Table 7 differs only in
    // the four leading bits, which is how the two are told apart.
    let r = states(0b0000_0110_0000_1001);
    let e = states(0b1111_0110_0000_1001);
    assert_eq!(r.len(), e.len());
    assert_ne!(r, e, "signal E came out identical to the rate signal");
    assert_eq!(r, states(0b0000_0110_0000_1001), "not reproducible");
}

/// The rate signal one real modem put on the line, read again.
///
/// This is what came back from a V.32bis far end over a VoIP trunk, and for a
/// long time this modem read it as "2400, 4800 and 9600 with trellis" -- which
/// is what Table 6/V.32 says it means. It is not what it means. B4 and B8
/// together are Note 1's mark of V.32bis, and under Table 5/V.32bis the bits
/// this modem was ignoring say 7200, 12 000 and 14 400 as well. The far end
/// had been offering 14 400 the whole time.
#[test]
fn a_real_far_ends_offer_reads_as_every_rate_there_is() {
    use datapump::v32::startup::{is_v32bis, rates_offered};
    let theirs = 0b0000_1111_1111_1001;
    assert!(is_v32bis(theirs), "B4 and B8 are what say so");
    let rates = rates_offered(theirs);
    assert_eq!(
        rates,
        Rates {
            at_4800: true,
            at_7200: true,
            at_9600: true,
            at_12000: true,
            at_14400: true,
        }
    );
    assert_eq!(rates.highest(), 14_400);
}

/// Two V.32bis modems settle on the fastest rate they both offer, coded.
#[test]
fn two_v32bis_ends_meet_at_the_fastest_rate_they_share() {
    use datapump::v32::Coding;
    use datapump::v32::startup::{agreed_coding, rate_signal, usable_rate};
    let theirs = 0b0000_1111_1111_1001;
    let ours = rate_signal(Rates::between(4800, 14_400));
    assert_eq!(usable_rate(theirs, ours), 14_400);
    assert_eq!(agreed_coding(theirs, ours, 14_400), Coding::Trellis);

    // And no faster than this end was told to go.
    let capped = rate_signal(Rates::between(4800, 9600));
    assert_eq!(usable_rate(theirs, capped), 9600);
    assert_eq!(agreed_coding(theirs, capped, 9600), Coding::Trellis);

    // 4800 is the one rate V.32bis leaves uncoded (2.3.5).
    let slow = rate_signal(Rates::only(4800));
    assert_eq!(usable_rate(theirs, slow), 4800);
    assert_eq!(agreed_coding(theirs, slow, 4800), Coding::Uncoded);
}

/// A far end that is not V.32bis is read by V.32's table and answered in it.
///
/// Note 1 to Table 5/V.32bis: "When B4 or B8 is set to zero, in a transmitted
/// or received rate signal, then interworking can proceed only in accordance
/// with Recommendation V.32."
#[test]
fn a_v32_far_end_is_read_and_answered_by_the_older_table() {
    use datapump::v32::Coding;
    use datapump::v32::startup::{
        agreed_coding, is_v32bis, rate_signal, rate_signal_for, rate_signal_v32, rates_offered,
        usable_rate,
    };
    // 4800 and 9600, with trellis, and none of V.32bis.
    let theirs = rate_signal_v32(Rates { at_4800: true, at_9600: true, ..Rates::default() }, true);
    assert!(!is_v32bis(theirs));
    let rates = rates_offered(theirs);
    assert!(rates.at_4800 && rates.at_9600);
    assert!(
        !rates.at_7200 && !rates.at_12000 && !rates.at_14400,
        "V.32's B9, B10 and B12 are not rates"
    );

    let ours = rate_signal(Rates::between(4800, 14_400));
    assert_eq!(usable_rate(theirs, ours), 9600);
    assert_eq!(agreed_coding(theirs, ours, 9600), Coding::Trellis);

    // What this end sends back has to be in the table that end can read: B4
    // there means 2400, which nothing here can do.
    let answer = rate_signal_for(9600, Coding::Trellis, is_v32bis(theirs));
    assert!(!is_v32bis(answer), "it answered V.32 in the newer table");
    assert_eq!(rates_offered(answer).highest(), 9600);
}

/// A far end without B8 gets the sixteen-point alternative, which 1 e) makes
/// mandatory for anything offering 9600 at all.
#[test]
fn a_far_end_without_trellis_gets_the_other_9600() {
    use datapump::v32::Coding;
    use datapump::v32::startup::{agreed_coding, rate_signal, rate_signal_v32, usable_rate};
    let theirs =
        rate_signal_v32(Rates { at_4800: true, at_9600: true, ..Rates::default() }, false);
    let ours = rate_signal(Rates::between(4800, 14_400));
    assert_eq!(usable_rate(theirs, ours), 9600);
    assert_eq!(agreed_coding(theirs, ours, 9600), Coding::Uncoded);
}

/// Two ends with nothing in common call for the connection to be cleared down
/// (Note 3 to Table 5/V.32bis).
#[test]
fn two_ends_with_no_rate_in_common_ask_to_hang_up() {
    use datapump::v32::startup::{rate_signal, usable_rate};
    let theirs = rate_signal(Rates::only(14_400));
    let ours = rate_signal(Rates::only(4800));
    assert_eq!(usable_rate(theirs, ours), 0);
}
