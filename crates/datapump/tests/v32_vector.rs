//! What a real V.32bis call looks like, measured against the recommendation.
//!
//! The capture runs at 14 400 bit/s, which this crate cannot yet demodulate:
//! that rate uses a 128-point constellation under a trellis code. Parts of the
//! start-up are plain enough to find with nothing but a correlator, though,
//! and those are checkable now.
//!
//! One thing hoped for here did not work out. The four signal states had to be
//! recovered rather than read, since the table in the recommendation lost its
//! sign column to the text extractor and the figure is a rendering whose
//! labels are shifted to stop them colliding. The conditioning signal of 5.2
//! alternates between states a quarter turn apart, which would put its energy
//! at a quarter of the symbol rate off the carrier, so a tone at 1200 or
//! 2400 Hz during start-up would have confirmed the placement against a real
//! line. Measured, both frequencies stand only about twice their neighbours
//! during start-up, which is not a tone. The reason is the same property that
//! made V.32 need an echo canceller: both directions occupy the one band at
//! the one time, and the capture is a two-wire tap carrying their sum, so each
//! modem's conditioning signal arrives underneath the other's. Separating them
//! needs a working V.32 receiver, which is the thing that wanted confirming.
//!
//! The placement rests instead on 5.2 itself, where the two segments of the
//! conditioning signal are written S and S-bar: the second is the first
//! negated, so C must be the negative of A and D of B, and only one reading of
//! the figure satisfies that. There is a unit test for the consequence.

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

/// Recorded rather than asserted: see the note at the top of this file.
#[test]
#[ignore]
fn what_the_start_up_looks_like_at_a_quarter_of_the_symbol_rate() {
    let wav = line::wav::read(VECTOR).expect("read V.32bis vector");
    let fs = wav.sample_rate as f64;
    let samples = wav.mono();
    for freq in [1200.0, 2400.0] {
        let (when, level) = strongest_window(&samples, fs, freq, 1.0, 10.0);
        let width = (0.04 * fs) as usize;
        let start = (when * fs) as usize;
        let window = &samples[start..(start + width).min(samples.len())];
        let elsewhere = [900.0, 1500.0, 1800.0, 2100.0, 2700.0, 3000.0]
            .iter()
            .filter(|&&f| (f - freq).abs() > 200.0)
            .map(|&f| tone(window, fs, f))
            .fold(0.0f64, f64::max);
        println!(
            "  {freq} Hz peaks at {level:.4} at {when:.2} s, \
             against {elsewhere:.4} nearby: a ratio of {:.1}",
            level / elsewhere
        );
    }
}
