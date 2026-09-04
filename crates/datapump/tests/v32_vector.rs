//! What a real V.32bis call looks like, measured against the recommendation.
//!
//! The capture runs at 14 400 bit/s, which this crate cannot yet demodulate:
//! that rate uses a 128-point constellation under a trellis code. The start-up
//! of clause 5.4 is the same at every rate, and its opening is made of plain
//! tones, which a correlator can find with no demodulation at all.
//!
//! That opening settles where the four signal states are, which mattered
//! because they had to be recovered rather than read: the table in the
//! recommendation lost its sign column to the text extractor, and the figure
//! is a rendering whose labels are shifted to stop them colliding. Clause 5.4
//! predicts two things from the placement. A modem repeating one state never
//! turns, so it puts out the bare carrier at 1800 Hz, which is what 5.4.2 has
//! the answering modem listen for. A modem alternating states A and C reverses
//! phase every symbol, which is double sideband with the carrier suppressed:
//! 1200 Hz either side of 1800, or exactly the "600 Hz and 3000 Hz" that 5.4.1
//! has the calling modem listen for. Both are in the capture.
//!
//! The second is the one that pins the geometry, and what it turns on is the
//! suppression rather than the sidebands. Any alternation between two states
//! puts energy 1200 Hz either side, because any alternation repeats every two
//! symbols; what decides whether anything is left in the middle is whether the
//! two states average to nothing. Only an antipodal pair does. States a quarter
//! turn apart would leave the carrier standing 3 dB above each sideband, and
//! measured, the whole band between the two sidebands sits 46 dB below them.
//!
//! The conditioning signal of 5.2 would have said the same thing about states
//! A and B, and could not be found: it falls in the part of the start-up where
//! both modems transmit at once, and this is a two-wire tap carrying their
//! sum, so each modem's signal arrives underneath the other's. Separating them
//! needs a working V.32 receiver, which is the thing that wanted confirming.
//! The tones above avoid that because they are the half-duplex part, where the
//! two ends take turns.

use std::f64::consts::TAU;

const VECTOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/vectors/v32bis-14400.wav"
);

/// Amplitude at one frequency over a window, by direct correlation.
fn tone(samples: &[f32], fs: f64, freq: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &s) in samples.iter().enumerate() {
        let w = TAU * freq * n as f64 / fs;
        re += s as f64 * w.cos();
        im -= s as f64 * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / samples.len() as f64
}

/// Where `freq` is strongest between `from` and `to` seconds, and how strong.
///
/// Swept rather than assumed: the timings in 5.4 are counted in symbol
/// intervals from events in the exchange, not from the start of the call, and
/// this recording opens with the answering tone.
fn strongest_window(samples: &[f32], fs: f64, freq: f64, from: f64, to: f64) -> (f64, f64) {
    let width = (0.04 * fs) as usize;
    let mut best = (0.0, 0.0);
    let mut at = (from * fs) as usize;
    let end = ((to * fs) as usize).min(samples.len());
    while at + width < end {
        let a = tone(&samples[at..at + width], fs, freq);
        if a > best.1 {
            best = (at as f64 / fs, a);
        }
        at += width / 2;
    }
    best
}

#[test]
fn the_call_opens_with_the_v25_answering_tone() {
    // 5.1 requires it, and it is the one part of the start-up that needs no
    // demodulation at all to check.
    let wav = line::wav::read(VECTOR).expect("read V.32bis vector");
    let fs = wav.sample_rate as f64;
    let samples = wav.mono();
    let (when, level) = strongest_window(&samples, fs, 2100.0, 0.0, 10.0);
    assert!(
        level > 0.05,
        "the answering tone is only {level:.4} at its strongest"
    );
    assert!(
        when < 4.0,
        "the answering tone does not appear until {when:.2} s"
    );
}

#[test]
fn the_signal_fills_the_band_the_recommendation_gives_it() {
    // 2.2 puts the transmitted energy between 600 and 3000 Hz, and 2.1 the
    // carrier in the middle of that at 1800. This says nothing about the
    // constellation, but it does confirm the band and the carrier against a
    // real call, which is what the transmitter here was built to.
    let wav = line::wav::read(VECTOR).expect("read V.32bis vector");
    let fs = wav.sample_rate as f64;
    let samples = wav.mono();
    // Well into the call, where both modems are sending data.
    let from = (12.0 * fs) as usize;
    let window = &samples[from..from + (0.2 * fs) as usize];

    let inside: f64 = [900.0, 1200.0, 1800.0, 2400.0, 2700.0]
        .iter()
        .map(|&f| tone(window, fs, f))
        .sum::<f64>()
        / 5.0;
    for outside in [300.0, 400.0, 3600.0, 3800.0] {
        let level = tone(window, fs, outside);
        assert!(
            level < inside / 8.0,
            "{outside} Hz carries {level:.4} against {inside:.4} inside the band"
        );
    }
}

#[test]
fn the_start_up_carries_the_tones_clause_5_4_names() {
    // 5.4.1: the calling modem transmits state A repetitively, and the
    // answering modem is looking for "an incoming tone at 1800 Hz". A repeated
    // state never turns, so all that is left is the carrier.
    //
    // 5.4.2: the answering modem transmits alternate states A and C, and the
    // calling modem is looking for "one of two incoming tones at frequencies
    // 600 Hz and 3000 Hz". Alternating two states half a turn apart reverses
    // the phase every symbol, which at 2400 baud is 1200 Hz either side of an
    // 1800 Hz carrier: exactly 600 and 3000.
    //
    // Both predictions come out of where the four states were placed, and
    // neither would hold if A and C were not antipodal. Finding them in a real
    // call is the confirmation the conditioning signal could not give, and it
    // works for the same reason: these are the half-duplex part of the
    // start-up, where the two modems take turns rather than talk over one
    // another.
    let wav = line::wav::read(VECTOR).expect("read V.32bis vector");
    let fs = wav.sample_rate as f64;
    let samples = wav.mono();

    // Where each is loudest, and how loud everything is at that moment.
    let profile = |at: f64| {
        let width = (0.04 * fs) as usize;
        let start = (at * fs) as usize;
        let window = &samples[start..(start + width).min(samples.len())];
        let level = |f: f64| tone(window, fs, f);
        (
            level(600.0),
            level(3000.0),
            [900.0, 1200.0, 1500.0, 1800.0, 2100.0, 2400.0]
                .iter()
                .map(|&f| level(f))
                .fold(0.0f64, f64::max),
        )
    };

    let (carrier_at, carrier) = strongest_window(&samples, fs, 1800.0, 0.5, 12.0);
    let (pair_at, _) = strongest_window(&samples, fs, 3000.0, 0.5, 12.0);
    let (low, high, between) = profile(pair_at);
    println!("  1800 Hz peaks at {carrier:.4}, {carrier_at:.2} s");
    println!("  at {pair_at:.2} s: 600 Hz {low:.4}, 3000 Hz {high:.4}, between {between:.4}");

    // A state repeated never turns, so nothing is left of it but the carrier.
    //
    // The two modems overlap here: 5.4.2 has the answering modem alternating A
    // and C while it waits to detect the calling modem's 1800 Hz tone, so all
    // three tones are on the line at once and the picture to check is that
    // there are exactly three. Anything at 1200 or 2400 would mean a pair of
    // states a quarter turn apart somewhere, which is what a misreading of the
    // figure would produce.
    //
    // 2100 Hz is left out of that count, because it is the answering tone of
    // 5.1 and is still going: 5.4.1 has the calling modem connect to line and
    // begin transmitting after hearing only a second of it. Measured, the line
    // at this instant carries those two tones and nothing else within 30 dB.
    let elsewhere = {
        let width = (0.04 * fs) as usize;
        let start = (carrier_at * fs) as usize;
        let window = &samples[start..(start + width).min(samples.len())];
        [900.0, 1200.0, 1500.0, 2400.0, 2700.0]
            .iter()
            .map(|&f| tone(window, fs, f))
            .fold(0.0f64, f64::max)
    };
    println!("  at {carrier_at:.2} s: 1800 Hz {carrier:.4}, elsewhere {elsewhere:.4}");
    assert!(
        carrier > 4.0 * elsewhere,
        "the 1800 Hz tone of 5.4.1 does not stand clear: {carrier:.4} against          {elsewhere:.4} at frequencies where nothing should be"
    );

    // Alternating two antipodal states is phase reversal at half the symbol
    // rate, which is double sideband with the carrier suppressed: energy at
    // 600 and at 3000 and nothing in between. The suppression is the whole
    // test. Sidebands 1200 Hz out would appear for any alternation at all,
    // since any alternation repeats every two symbols; what says the two
    // states are opposite is that they average to nothing, and a pair a
    // quarter turn apart would leave the carrier standing 3 dB above each
    // sideband instead.
    assert!(
        low > 4.0 * between && high > 4.0 * between,
        "the sidebands at 600 and 3000 Hz ({low:.4}, {high:.4}) do not stand          clear of the band between them ({between:.4}), so states A and C are          not half a turn apart"
    );
    assert!(
        (low / high).max(high / low) < 3.0,
        "the two sidebands are lopsided at {low:.4} and {high:.4}, which is not          a suppressed carrier"
    );
}

