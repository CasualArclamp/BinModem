//! Read a captured 8 kHz s16le line file the way a modem would and say what is
//! actually on it.
//!
//! This exists to answer one question about the bench: when an Agere SV92PP
//! reports `V34 exception: 60096212` -- its Phase 4 state waiting for an SUVd
//! running out its 80000-sample deadline -- was the SUVd never sent, or sent and
//! not readable? `agr-rx.raw` is what reached its receiver, so running the same
//! reader over that file gives the answer the far end would have given.
//!
//! ```text
//! cargo run --release --example v92_capture -- FILE [MODE]
//!   --levels            level and peak, a quarter second at a time
//!   --sync FROM TO      look for the frame sync of a framed sequence
//!   --down              list the sequences a two-point reader finds
//!   --up                the same, on the analogue modem's symbol rate
//! ```
//!
//! `--sync` is the diagnostic of last resort: it tries the four readings a
//! two-point stream admits (each polynomial, differential or not) and counts
//! runs of seventeen ones, which is the frame sync every CP and SUV sequence
//! opens with. A stream that shows none is not carrying sequences at all, no
//! matter what the transmitter thinks it is sending.

use std::env;
use std::fs;

use datapump::v32::{Mode, Scrambler};
use datapump::v92::sequences::{DownFamily, DownFinder, UpFinder, trn2u_unit};
use datapump::v92::up_signals::TwoPointReader;

/// Samples per upstream symbol during training (V.92 8.7.2: six).
const UP_SPB: usize = 6;

/// The frame sync every framed sequence opens with (Tables 27, 30, 31):
/// seventeen ones in the descrambled stream.
const SYNC: usize = 17;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut path = None;
    let mut mode = "down";
    let mut numbers = Vec::new();
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--up" => mode = "up",
            "--down" => mode = "down",
            "--levels" => mode = "levels",
            "--sync" => mode = "sync",
            other => match other.parse::<f64>() {
                Ok(value) => numbers.push(value),
                Err(_) => path = Some(other.to_string()),
            },
        }
    }
    let Some(path) = path else {
        eprintln!("usage: v92_capture FILE [--levels|--sync FROM TO|--down|--up]");
        return Err("no file".into());
    };
    let bytes = fs::read(&path)?;
    let samples: Vec<f64> = bytes
        .as_chunks::<2>().0.iter()
        .map(|p| f64::from(i16::from_le_bytes([p[0], p[1]])))
        .collect();
    let seconds = samples.len() as f64 / 8000.0;
    println!("{path}: {} samples = {seconds:.2} s at 8000 Hz", samples.len());
    let span = match numbers.as_slice() {
        [from, to] => (*from, *to),
        _ => (0.0, seconds),
    };

    match mode {
        // A quarter of a second at a time: where the line goes quiet and where
        // it comes back is the timetable, and the level says which signal.
        "levels" => {
            for (n, block) in samples.chunks(2000).enumerate() {
                let rms = (block.iter().map(|s| s * s).sum::<f64>() / block.len() as f64).sqrt();
                let peak = block.iter().fold(0.0f64, |a, s| a.max(s.abs()));
                if rms > 1.0 {
                    println!("{:8.3} s  rms {:8.1}  peak {:8.1}", n as f64 * 0.25, rms, peak);
                }
            }
        }
        "sync" => sync(&samples, span),
        "down" => {
            // One symbol per sample, and the digital modem's polynomial (6.3).
            let mut reader = TwoPointReader::differential_with(Mode::Call);
            let mut finder = DownFinder::new();
            let (from, to) = limits(span, seconds);
            let mut found = 0;
            for (n, &level) in samples.iter().enumerate().take(to).skip(from) {
                let Some(bit) = reader.feed(level) else { continue };
                if let Some(family) = finder.feed(bit) {
                    found += 1;
                    let at = n as f64 / 8000.0;
                    match family {
                        DownFamily::Suvd(s) => {
                            println!("{at:9.4} s  SUVd  ack {} silence {}", s.ack, s.silence);
                        }
                        DownFamily::Cpd(c) => println!("{at:9.4} s  CPd   {c:?}"),
                    }
                }
            }
            println!("{found} downstream sequences");
        }
        _ => {
            // Six samples per symbol: the middle of each is the eye.
            let mut reader = TwoPointReader::new();
            let mut finder = UpFinder::new(trn2u_unit(false));
            let (from, to) = limits(span, seconds);
            let mut found = 0;
            for (n, symbol) in samples.as_chunks::<UP_SPB>().0.iter().enumerate() {
                if n * UP_SPB < from || n * UP_SPB >= to {
                    continue;
                }
                let level = symbol[UP_SPB / 2];
                let Some(bit) = reader.feed(level) else { continue };
                if let Some(family) = finder.feed(bit) {
                    found += 1;
                    println!("{:9.4} s  {family:?}", (n * UP_SPB) as f64 / 8000.0);
                }
            }
            println!("{found} upstream sequences");
        }
    }
    Ok(())
}

fn limits(span: (f64, f64), seconds: f64) -> (usize, usize) {
    let from = (span.0 * 8000.0) as usize;
    let to = ((span.1.min(seconds)) * 8000.0) as usize;
    (from, to)
}

/// The four readings a two-point stream admits, counted by frame sync.
///
/// A scrambler is self-synchronising, so a window can be read on its own after
/// twenty-three symbols; what cannot be guessed from the line is which
/// polynomial and whether the symbols are differentially encoded, and a wrong
/// guess turns the stream into noise. So all four are tried and the answer is
/// whichever shows the seventeen ones.
fn sync(samples: &[f64], span: (f64, f64)) {
    let (from, to) = limits(span, samples.len() as f64 / 8000.0);
    let window = &samples[from..to];
    println!(
        "window {:.3}..{:.3} s, {} samples",
        from as f64 / 8000.0,
        to as f64 / 8000.0,
        window.len()
    );
    for (mode, name) in [(Mode::Call, "Call"), (Mode::Answer, "Answer")] {
        for differential in [true, false] {
            for invert in [false, true] {
                let mut scrambler = Scrambler::new(mode);
                let mut previous = false;
                let mut run = 0;
                let mut runs = Vec::new();
                for (n, &level) in window.iter().enumerate() {
                    let sign = (level < 0.0) ^ invert;
                    let received = if differential { sign ^ previous } else { sign };
                    previous = sign;
                    let bit = scrambler.descramble(received);
                    if bit {
                        run += 1;
                    } else {
                        if run >= SYNC {
                            runs.push((n, run));
                        }
                        run = 0;
                    }
                }
                println!(
                    "{name:6} differential {differential:<5} invert {invert:<5} -> {:4} syncs",
                    runs.len()
                );
                for &(end, length) in runs.iter().take(4) {
                    println!(
                        "        at {:.4} s, run {length}",
                        end as f64 / 8000.0
                    );
                    // What the sequence says after its sync, read with this
                    // same hypothesis: the first word of every framed sequence
                    // is its own table's, so the bits themselves are the test.
                    let mut scrambler = Scrambler::new(mode);
                    let mut previous = false;
                    let start = end.saturating_sub(120).min(end);
                    let mut bits = String::new();
                    for &level in window.iter().take(end + 120).skip(start) {
                        let sign = (level < 0.0) ^ invert;
                        let received = if differential { sign ^ previous } else { sign };
                        previous = sign;
                        let bit = scrambler.descramble(received);
                        if bits.len() >= 120 {
                            break;
                        }
                        if bits.len() + (end - start) >= (end - start) {
                            bits.push(if bit { '1' } else { '0' });
                        }
                    }
                    println!("        sync +120 bits: {bits}");
                }
            }
        }
    }
}
