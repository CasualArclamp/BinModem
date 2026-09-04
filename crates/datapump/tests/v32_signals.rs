//! The signals V.32's start-up is conducted in, measured on the line.
//!
//! Every one of them is a fixed pattern of constellation states chosen for
//! what it looks like as a waveform, so what it looks like as a waveform is
//! the thing worth testing. Each modem recognises the other by these alone,
//! before either has a working demodulator: that is the point of them.

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
    let mut d = ReversalDetector::new(CARRIER + OFFSET, 50.0, 0.05, 64, FS);
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
    let mut d = ReversalDetector::new(CARRIER, 50.0, 0.05, 64, FS);
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
