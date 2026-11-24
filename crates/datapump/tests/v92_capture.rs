//! Diagnostics over a captured call. Not tests: each one needs a file, and
//! they are `#[ignore]`d so that `cargo test` never asks for one.
//!
//! The captures come from the interop rigs, which write the far end's own
//! transmit stream as s16le at 8000 Hz (`agr-tx.raw` from the Agere engine,
//! `caller.raw` from the HSF one -- note that `bin_bridge.py` writes *our*
//! transmit stream as `caller.raw` on the Agere rig, so the far end's is the
//! one to feed here).
//!
//! ```bash
//! V92_CAPTURE=/tmp/agr-v90-interop/<run>/agr-tx.raw \
//!     cargo test -p datapump --test v92_capture -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use datapump::v32::Mode;
use datapump::v90::sequences::CpFinder;
use datapump::v92::receiver::{Heard, Receiver};
use datapump::v92::up_signals::TwoPointReader;

/// The raw file named by `V92_CAPTURE`, as levels in the units the receiver
/// works in.
fn capture() -> Option<Vec<f64>> {
    let path = PathBuf::from(std::env::var_os("V92_CAPTURE")?);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Some(bytes.as_chunks::<2>().0.iter().map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0).collect())
}

/// Whether a CPt of any kind can be read out of a capture at all.
///
/// It slices the far end's stream at the sample instants -- one sample a
/// symbol on both rigs -- differentially decodes it and descrambles with GPA,
/// which is what a CPt is (8.5.1), and then asks [`CpFinder`] whether Table 23's
/// frame and CRC are in there anywhere. It needs no equaliser, so it answers the
/// one question the digital modem's silence during a real call cannot: is the
/// CPt arriving, and if so how wrong are the bits?
#[test]
#[ignore = "needs V92_CAPTURE=<file.raw>"]
fn a_capture_carries_a_cpt() {
    let Some(levels) = capture() else {
        eprintln!("set V92_CAPTURE to a raw s16le file at 8000 Hz");
        return;
    };
    eprintln!("{} samples, {:.2} s at 8000 Hz", levels.len(), levels.len() as f64 / 8000.0);
    let mut reader = TwoPointReader::differential_with(Mode::Answer);
    let mut finder = CpFinder::v92(12);
    let mut bits = 0u64;
    let mut ones = 0u64;
    let mut found = 0;
    for (n, &level) in levels.iter().enumerate() {
        let Some(bit) = reader.feed(level) else { continue };
        bits += 1;
        ones += u64::from(bit);
        if let Some(cp) = finder.feed(bit) {
            found += 1;
            if found <= 8 {
                eprintln!(
                    "CPt at bit {bits} (sample {n}, {:.3} s): drn {}, {} set(s), type {}, lookahead {}",
                    n as f64 / 8000.0,
                    cp.drn,
                    cp.constellations.len(),
                    cp.data_mode,
                    cp.lookahead
                );
            }
        }
    }
    eprintln!("{found} CPt(s) in {bits} bits, {}", 100.0 * ones as f64 / bits as f64);
}

/// The same question through the receiver's own chain: find where the far end's
/// TRN1u is, fit the equaliser there, and then read CPt out of what follows.
///
/// This is the one that matters. Slicing signs straight off the capture finds
/// nothing because the far end's transmit filter puts the transitions where the
/// sample instants are not; a forty-eight tap equaliser is what puts them back.
#[test]
#[ignore = "needs V92_CAPTURE=<file.raw>"]
fn the_receiver_chain_reads_a_cpt_from_a_capture() {
    let Some(levels) = capture() else {
        eprintln!("set V92_CAPTURE to a raw s16le file at 8000 Hz");
        return;
    };
    // Where is the far end's TRN1u? Fit at every candidate anchor the history
    // allows and keep the best.
    let mut best = (f64::NEG_INFINITY, 0usize);
    {
        let mut rx = Receiver::new();
        for (n, &level) in levels.iter().enumerate() {
            rx.feed(level);
            // A fit costs 129 least squares, so this asks once every half
            // second and searches only a little either side of the middle of
            // the window.
            if n % 1200 != 0 || rx.symbols() < 1400 {
                continue;
            }
            // `train_near` indexes the receiver's *history*, not the stream:
            // its window is the last 1200 symbols of it.
            let history = rx.history_len() as u64;
            let mut rx = rx.clone();
            let anchor = history.saturating_sub(1200);
            if rx.train_near(anchor, 64) && rx.snr_db() > best.0 {
                best = (rx.snr_db(), n);
            }
        }
    }
    let (snr, at) = best;
    eprintln!("best TRN1u fit {snr:.1} dB around sample {at} ({:.3} s)", at as f64 / 8000.0);
    if snr < 12.0 {
        eprintln!("nothing to train on: no CPt was looked for");
        return;
    }
    // Stream again, train on the same anchor, and read the rest as CPt.
    let mut rx = Receiver::new();
    let mut reader = TwoPointReader::differential_with(Mode::Answer);
    let mut finder = CpFinder::v92(12);
    let mut trained = false;
    let mut bits = 0u64;
    let mut found = 0;
    for (n, &level) in levels.iter().enumerate() {
        rx.feed(level);
        if n == at {
            let history = rx.history_len() as u64;
            let anchor = history.saturating_sub(1200);
            trained = rx.train_near(anchor, 64);
            eprintln!("training at sample {n}: {trained}, {:.1} dB", rx.snr_db());
        }
        if !trained {
            continue;
        }
        while let Some(heard) = rx.heard() {
            if let Heard::Symbol(symbol) = heard
                && let Some(bit) = reader.feed(symbol.value)
            {
                bits += 1;
                if let Some(cp) = finder.feed(bit) {
                    found += 1;
                    if found <= 6 {
                        eprintln!(
                            "CPt at bit {bits}: drn {}, {} set(s), type {}, lookahead {}",
                            cp.drn,
                            cp.constellations.len(),
                            cp.data_mode,
                            cp.lookahead
                        );
                    }
                }
            }
        }
    }
    eprintln!("{found} CPt(s) in {bits} symbols after training");
}
