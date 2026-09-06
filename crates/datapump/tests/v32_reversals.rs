//! What the reversal detectors made of a real call.
//!
//! Ignored, because it needs a capture and captures are not in the repository:
//! they are large, and the ones worth having come off somebody's telephone
//! line. Point it at one with
//!
//! ```text
//! V32_CAPTURE=captures/live-1788682720.wav cargo test -p datapump \
//!     --test v32_reversals -- --ignored --nocapture
//! ```
//!
//! The question it exists to answer. On one recorded call the calling modem
//! declared a sideband reversal at 59.36 s of session time -- which was the
//! moment the far end's alternation *began*, its sidebands going from a
//! thousandth to a tenth with a settled phase, and not a reversal at all. The
//! far end's only real reversal came at 61.00 s, and was taken for the second.
//! The round-trip measurement made from that pair describes nothing.
//!
//! An earlier commit guessed at the cause and was wrong, and said so. This
//! runs the real detectors over the real recording instead of guessing again.
//!
//! What it found was that the far end's carrier was 22 to 36 Hz off frequency
//! through the stretch in question, and that the detector could not tell that
//! from a phase that had stepped. Across every recording in hand, before the
//! fix that came out of this and after it:
//!
//! ```text
//!                     carrier 1800    lower 600    upper 3000
//!   live-1788682720      51 -> 5       44 -> 1        1 -> 1
//!   live-1788682165      53 -> 5       48 -> 3        2 -> 2
//!   live-1788671074       2 -> 1       33 -> 0        0 -> 0
//!   live-1788681310      57 -> 5       35 -> 1        2 -> 1
//! ```
//!
//! The upper sideband is the column to read for whether anything real was
//! lost, because it was the one that was not chattering to begin with: three
//! of its four counts survive unchanged. The fourth went from two to one and
//! is not accounted for.

use dsp::{ReversalDetector, ToneDetector};

const FS: f64 = 16_000.0;

/// V.32 5.4.1: the answering modem's AC and the calling modem's CA are 1800 Hz
/// carriers alternating in phase, and what is watched for is the alternation
/// -- the sidebands 600 Hz either side of it, which appear because a pattern
/// repeating every two symbols is a 600 Hz modulation of the carrier.
const CARRIER: f64 = 1800.0;
/// Half the symbol rate. A pattern repeating every two symbols modulates the
/// carrier at 1200 Hz, so the sidebands are at 600 and 3000 Hz -- not at 600
/// Hz either side, which is what the first run of this probe looked for and
/// why it found nothing at all.
const SIDEBAND: f64 = 1200.0;
/// What the start-up itself uses (`AUDIBLE` in startup.rs). The first run used
/// 0.05, twenty times higher, and neither sideband ever crossed it.
const THRESHOLD: f64 = 0.008;
const BANDWIDTH: f64 = 60.0;

fn capture() -> Option<(Vec<f32>, f64)> {
    let path = std::env::var("V32_CAPTURE").ok()?;
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "the detectors are built for 16 kHz");
    let offset: f64 = std::env::var("V32_OFFSET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    println!(
        "\n{path}: {:.1} s, {} channels at {} Hz, session offset {offset:.2} s",
        wav.duration_secs(),
        wav.channels,
        wav.sample_rate
    );
    Some((wav.channel(0), offset))
}

/// How fast the phasor at `freq` turns, in hertz, second by second.
///
/// The question the reversal detector cannot answer for itself. It compares
/// the phasor now against the phasor six time constants ago and calls them
/// opposed when they are more than a hundred and thirty-five degrees apart --
/// and a carrier that is simply off frequency turns steadily through that
/// angle and keeps going. `ReversalDetector::new` works out that seven hertz
/// of offset turns forty degrees in the comparison window, so a hundred and
/// thirty-five needs about twenty-four. Past that, a detector watching a
/// perfectly steady tone reports a reversal as often as it is allowed to.
#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_carrier_offset() {
    let Some((line, offset)) = capture() else {
        println!("set V32_CAPTURE to a recording to run this");
        return;
    };
    let mut tones = [
        ("carrier 1800", ToneDetector::new(CARRIER, BANDWIDTH, FS)),
        ("lower 600", ToneDetector::new(CARRIER - SIDEBAND, BANDWIDTH, FS)),
        ("upper 3000", ToneDetector::new(CARRIER + SIDEBAND, BANDWIDTH, FS)),
    ];

    // A second at a time, because the interesting stretches last seconds and
    // an offset that changes within one is not an offset.
    const WINDOW: usize = FS as usize;
    println!(
        "  {:>9}  {:<13} {:>10} {:>12}",
        "session s", "tone", "amplitude", "offset"
    );
    let mut turned = [0.0f64; 3];
    let mut previous = [f64::NAN; 3];
    for (i, s) in line.iter().enumerate() {
        for (k, (_, t)) in tones.iter_mut().enumerate() {
            t.feed(f64::from(*s));
            let phase = t.phase();
            if previous[k].is_finite() {
                let mut step = phase - previous[k];
                while step > std::f64::consts::PI {
                    step -= std::f64::consts::TAU;
                }
                while step < -std::f64::consts::PI {
                    step += std::f64::consts::TAU;
                }
                turned[k] += step;
            }
            previous[k] = phase;
        }
        if i % WINDOW == WINDOW - 1 {
            let t = i as f64 / FS + offset;
            for (k, (name, d)) in tones.iter().enumerate() {
                // Only where there is a tone to be off frequency. Noise turns
                // as fast as it likes and means nothing by it.
                if d.amplitude() > 0.008 {
                    println!(
                        "  {t:>9.3}  {name:<13} {:>10.5} {:>9.1} Hz",
                        d.amplitude(),
                        turned[k] / std::f64::consts::TAU
                    );
                }
            }
            turned = [0.0; 3];
        }
    }
    println!();
}

#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_replay_reversals() {
    let Some((line, offset)) = capture() else {
        println!("set V32_CAPTURE to a recording to run this");
        return;
    };

    // The three the start-up watches: the carrier itself, and the two
    // sidebands whose phase carries the alternation.
    let mut detectors = [
        ("carrier 1800", ReversalDetector::new(CARRIER, BANDWIDTH, THRESHOLD, FS)),
        ("lower 600", ReversalDetector::new(CARRIER - SIDEBAND, BANDWIDTH, THRESHOLD, FS)),
        ("upper 3000", ReversalDetector::new(CARRIER + SIDEBAND, BANDWIDTH, THRESHOLD, FS)),
    ];

    println!(
        "  {:>9}  {:<13} {:>8} {:>8} {:>8} {:>8} {:>7}",
        "session s", "fired", "1800", "600", "3000", "since", "count"
    );
    let mut last = [f64::NAN; 3];
    for (i, s) in line.iter().enumerate() {
        let t = i as f64 / FS + offset;
        let mut fired = [false; 3];
        for (k, (_, d)) in detectors.iter_mut().enumerate() {
            fired[k] = d.feed(f64::from(*s));
        }
        // All three amplitudes at every firing, because the question is not
        // how big the tone that reversed was but whether it was the one the
        // far end was sending. During its alternation the carrier is
        // suppressed and the sidebands carry the signal; while it repeats a
        // state the reverse. A reversal in the quieter of the two is a
        // reversal in a place that is empty.
        let amps: Vec<f64> = detectors.iter().map(|(_, d)| d.amplitude()).collect();
        for (k, (name, d)) in detectors.iter().enumerate() {
            if !fired[k] {
                continue;
            }
            let gap = t - last[k];
            last[k] = t;
            println!(
                "  {t:>9.3}  {name:<13} {:>8.5} {:>8.5} {:>8.5} {:>6.1}ms {:>7}",
                amps[0],
                amps[1],
                amps[2],
                gap * 1000.0,
                d.count()
            );
        }
    }

    // The floor the detector imposes on itself, so that a run at exactly
    // that spacing can be recognised for what it is: not a signal
    // reversing, but a detector firing as fast as it is allowed to. It
    // waits six time constants before comparing directions again and
    // needs one more of opposition before it believes what it sees, and
    // the time constant is fs over two pi times the bandwidth.
    let tau = FS / (std::f64::consts::TAU * BANDWIDTH);
    println!(
        "\n  the detector cannot fire faster than {:.1} ms apart",
        7.0 * tau / FS * 1000.0
    );
    println!("  totals");
    for (name, d) in &detectors {
        println!("  {name:<13} {:>3} reversals", d.count());
    }
    println!();
}
