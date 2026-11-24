//! Place a call between two modems and write what the line carried.
//!
//! The two ends run against each other exactly as they do in the tests, and
//! everything they put on the line is summed and written to a WAVE file. The
//! result is a recording of a call this program made, in the same form as the
//! recordings of other people's calls in `tests/vectors`, so the same scope can
//! be pointed at it and the same decoder run over it.
//!
//! That closes a loop worth closing. Up to now the only calls this project
//! could look at were ones somebody else made, which meant every disagreement
//! between the code and a recording could be blamed on either.

use std::process::ExitCode;

use modem::{Modem, State};

const FS: f64 = 16_000.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut carrier = "V22B".to_owned();
    let mut path = "call.wav".to_owned();
    let mut text = "Welcome to phl6-dial1.popsite.net\r\nlogin:".to_owned();
    let mut seconds = 20.0;

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--carrier" => carrier = value().to_ascii_uppercase(),
            "--out" => path = value(),
            "--text" => text = value(),
            "--seconds" => seconds = value().parse().unwrap_or(seconds),
            "--help" | "-h" => {
                println!(
                    "modem-call [--carrier V22B|V32] [--out call.wav] \
                     [--text <what the host sends>] [--seconds <n>]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let (mut line, transcript, rate) = place(&carrier, &text, seconds);
    // Scale to fit before writing. Two modems summed on one pair peak well
    // above full scale, and a clipped recording is a recording of something
    // else.
    let gain = modem::fit_to_scale(&mut line, 0.9);
    if let Err(e) = line::wav::write(&path, &line, FS as u32) {
        eprintln!("could not write {path}: {e}");
        return ExitCode::FAILURE;
    }

    match rate {
        Some(rate) => println!("connected at {rate} bit/s using {carrier}"),
        None => {
            eprintln!("the call never connected");
            return ExitCode::FAILURE;
        }
    }
    println!(
        "wrote {:.1} s to {path}, scaled by {gain:.2} to fit",
        line.len() as f64 / FS
    );
    println!("the caller's terminal saw:\n{transcript}");
    ExitCode::SUCCESS
}

/// Run a call and return the line, what the caller's terminal saw, and the rate.
fn place(carrier: &str, text: &str, seconds: f64) -> (Vec<f32>, String, Option<u32>) {
    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        for b in format!("AT+MS={carrier}\r").bytes() {
            m.feed_dte(b);
        }
        m.take_dte();
    }
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }
    for b in b"ATD5551234\r" {
        caller.feed_dte(*b);
    }

    let mut line = Vec::with_capacity((seconds * FS) as usize);
    let mut seen: Vec<u8> = Vec::new();
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let mut settled = f64::NAN;
    let mut spoken = false;

    for i in 0..(seconds * FS) as usize {
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);
        // What a tap on the two-wire line would hear: both directions at once.
        line.push(((a + b) * 0.5) as f32);
        seen.extend(caller.take_dte());
        host.take_dte();

        let up = caller.state() == State::Data && host.state() == State::Data;
        if up && settled.is_nan() {
            settled = i as f64 / FS;
        }
        // Let error control establish before the host says anything, since a
        // greeting sent into a link that is still coming up is retransmitted
        // rather than lost but arrives all at once and reads oddly.
        if up && !spoken && i as f64 / FS > settled + 1.5 {
            spoken = true;
            for byte in text.bytes() {
                host.feed_dte(byte);
            }
        }
    }
    (line, String::from_utf8_lossy(&seen).into_owned(), caller.rate())
}
