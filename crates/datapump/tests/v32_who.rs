//! Which end of a V.32 call a stretch of a recording came from.
//!
//! Ignored, because it needs a capture and captures are not in the repository.
//! Point it at one with
//!
//! ```text
//! V32_CAPTURE=dist/captures/live-1788760125.wav cargo test -p datapump \
//!     --test v32_who -- --ignored --nocapture
//! ```
//!
//! The question a mono recording of a V.32 call cannot otherwise answer. Both
//! directions share the whole band, so there is no filter that separates them
//! and no channel to read one off -- and a recording made off a line, or a
//! reference recording somebody put on the internet, is one signal containing
//! two modems.
//!
//! What separates them is that they are two machines. Their carriers come from
//! independent crystals and land a hertz or two apart, and that difference
//! survives everything on top of it. The alternation phase hands both
//! frequencies over for nothing: the calling modem repeats one state, which is
//! a bare line at its own carrier, and the answering modem alternates two
//! opposite states, which puts nothing at its carrier and a line 1200 Hz
//! either side, so the midpoint of that pair is the answering modem's.
//!
//! After that the signal is data and its carrier is suppressed -- but the
//! start-up's conditioning and training segments are QPSK, four points a
//! quarter turn apart, and a QPSK signal raised to the fourth power puts the
//! modulation back at one phase and leaves a line at four times the carrier
//! offset. So every segment before the trellis coding starts can be attributed
//! to the modem that sent it.
//!
//! Run over a reference recording of two real modems, against the question of
//! whether a real answering modem trains first or waits to be trained at:
//!
//! ```text
//!   alternations    3.25- 4.00 s   calling      0.045 against 0.021
//!   first burst     4.25- 7.00 s   answering    0.031 against 0.009
//!   second burst    7.75-10.50 s   calling      0.031 against 0.007
//!   data           10.75 s on      neither, and no line at either
//! ```
//!
//! Its carriers came out 1800.000 Hz calling and 1798.125 Hz answering, 1.9 Hz
//! apart, read off the alternation as above.
//!
//! Which is Figure 4 exactly, and worth having measured rather than assumed:
//! the answering modem sends the first S, S-bar, TRN and R1 while the calling
//! modem is silent, the calling modem answers with its own, and only then do
//! both transmit at once. The last row is the trellis coding starting -- a
//! 16-point constellation is not four points a quarter turn apart, so the
//! fourth power has nothing to put back and there is no line to find.

use std::f64::consts::TAU;

const FS: f64 = 16_000.0;
/// V.32 2.4: the carrier both ends aim at.
const CARRIER: f64 = 1800.0;
/// Half the symbol rate: where an alternation of two opposite states puts its
/// pair of lines, either side of the carrier that is not there.
const SIDEBAND: f64 = 1200.0;

fn capture() -> Option<Vec<f32>> {
    let path = std::env::var("V32_CAPTURE").ok()?;
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "this is built for 16 kHz");
    let channel: usize = std::env::var("V32_CHANNEL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    println!(
        "\n{path}: {:.1} s, {} channels at {} Hz, reading channel {channel}",
        wav.duration_secs(),
        wav.channels,
        wav.sample_rate
    );
    Some(wav.channel(channel.min(wav.channels as usize - 1)))
}

/// Magnitude of one frequency in a window, normalised to the window's energy.
///
/// A plain sum against a complex exponential, which is all a single-frequency
/// DFT bin is, with a raised cosine to stop the window's own edges producing
/// lines of their own.
///
/// The mean comes off first, and has to. One of the two carriers being looked
/// for is the nominal one, whose line after the fourth power sits at zero
/// hertz -- so without this the measurement for that end is the signal's own
/// DC, which is large for reasons that have nothing to do with a carrier, and
/// every stretch of the recording is attributed to the calling modem.
fn line_at(x: &[(f64, f64)], freq: f64) -> f64 {
    let n = x.len() as f64;
    let mean = (
        x.iter().map(|p| p.0).sum::<f64>() / n,
        x.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let (mut re, mut im, mut power) = (0.0, 0.0, 0.0);
    for (i, &(xr, xi)) in x.iter().enumerate() {
        let (xr, xi) = (xr - mean.0, xi - mean.1);
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * freq * i as f64 / FS;
        let (c, s) = (phase.cos(), phase.sin());
        re += w * (xr * c + xi * s);
        im += w * (xi * c - xr * s);
        power += xr * xr + xi * xi;
    }
    let magnitude = (re * re + im * im).sqrt();
    // Against what the same window would give if every sample agreed, so that
    // a loud stretch and a quiet one are read alike.
    let coherent = (power / n).sqrt() * n * 0.5;
    magnitude / (coherent + 1.0e-12)
}

/// The analytic signal, so that a real recording can be shifted in frequency.
///
/// The negative half of the spectrum is thrown away and the positive half
/// doubled, which leaves a signal whose phase is the one the modem sent rather
/// than that phase and its mirror image at once.
fn analytic(x: &[f32]) -> Vec<(f64, f64)> {
    let n = x.len();
    // A quadrature pair from a Hilbert transform is one FFT away, and this
    // crate has no FFT. What it needs instead is available directly: mixing
    // against a complex exponential and low-passing is the same operation, and
    // that is what a tone detector already is.
    let mut wide = dsp::ToneDetector::new(CARRIER, SIDEBAND * 1.4, FS);
    let mut out = Vec::with_capacity(n);
    for &v in x {
        wide.feed(f64::from(v));
        out.push(wide.phasor());
    }
    out
}

/// Raise a complex baseband signal to the fourth power.
///
/// Four points a quarter turn apart become one point, so whatever the data
/// was, the modulation is gone and a carrier offset is left turning four times
/// as fast as it was.
fn fourth_power(z: &[(f64, f64)]) -> Vec<(f64, f64)> {
    z.iter()
        .map(|&(re, im)| {
            let (a, b) = (re * re - im * im, 2.0 * re * im);
            (a * a - b * b, 2.0 * a * b)
        })
        .collect()
}

/// The two carriers, and then who is holding the line second by second.
#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_who_is_talking() {
    let Some(line) = capture() else {
        println!("set V32_CAPTURE to a recording to run this");
        return;
    };
    // The two carriers, named on the command line because finding the
    // alternation automatically is a second problem and the phase it lives in
    // is a second long and obvious in any spectrogram.
    let calling: f64 = std::env::var("V32_CALLING")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(CARRIER);
    let answering: f64 = std::env::var("V32_ANSWERING")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(CARRIER);
    println!(
        "  calling carrier {calling:.3} Hz, answering carrier {answering:.3} Hz\n\
         \x20 set V32_CALLING and V32_ANSWERING from the alternation phase: the\n\
         \x20 bare line is the calling modem, the midpoint of the pair 1200 Hz\n\
         \x20 either side of it is the answering one\n"
    );

    let z = analytic(&line);
    let q = fourth_power(&z);

    let window = (0.5 * FS) as usize;
    let hop = (0.25 * FS) as usize;
    println!("  {:>7}  {:>9}  {:>9}   whose", "t", "calling", "answering");
    let mut i = 0;
    while i + window <= q.len() {
        let slice = &q[i..i + window];
        let energy: f64 =
            slice.iter().map(|p| p.0 * p.0 + p.1 * p.1).sum::<f64>() / window as f64;
        if energy > 1.0e-18 {
            // Four times the offset, because the fourth power put it there.
            let c = line_at(slice, 4.0 * (calling - CARRIER));
            let a = line_at(slice, 4.0 * (answering - CARRIER));
            let whose = if c > a * 1.5 {
                "calling"
            } else if a > c * 1.5 {
                "answering"
            } else {
                "-"
            };
            println!("  {:>7.2}  {c:>9.4}  {a:>9.4}   {whose}", i as f64 / FS);
        }
        i += hop;
    }
    println!(
        "\n  A dash is either silence, both at once, or trellis-coded data,\n  \
         which is not four points and leaves no line at all.\n"
    );
}
