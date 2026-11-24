//! Check the audio path before trusting a call to it.
//!
//! A modem on a real line depends on a lot of wiring being right, and when it
//! is not the symptom is a modem that will not connect, which looks exactly
//! like a modem that does not work. This measures the path instead of guessing
//! at it: it sends tones out and reports what comes back, and everything it
//! reports is about the audio, not about any modem.
//!
//! Point the output at a virtual cable and the input at the other end of the
//! same one and it measures the cable. Point them at the two ends of a call
//! through a softphone and it measures the call: what the network does to a
//! telephone band is exactly what a modem has to live with, and seeing it
//! first saves a great deal of wondering.
//!
//! Both devices must be named. There is no default, deliberately: the default
//! output on a desktop machine is usually something with speakers attached,
//! and a modem tone at full level through those is not a good way to find out
//! that the arguments were wrong.

use std::f64::consts::TAU;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The rate the measurement runs at. Nothing here depends on it beyond being
/// comfortably above the telephone band.
const FS: f64 = 16_000.0;

/// Level to send at. Well below full scale, because the point is to measure
/// the path rather than to find out where it clips.
const LEVEL: f64 = 0.25;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut seconds = 1.0f64;

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--in" => input = Some(value()),
            "--out" => output = Some(value()),
            "--seconds" => seconds = value().parse().unwrap_or(seconds),
            "--help" | "-h" => {
                println!(
                    "line-check --in <device> --out <device> [--seconds <n>]\n\
                     \n\
                     Sends tones across the telephone band and reports what comes\n\
                     back: level, delay, and how flat the path is. Both devices must\n\
                     be named; there is no default, because the default output\n\
                     usually has speakers on it."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("both --in and --out are required; see --help");
        return ExitCode::FAILURE;
    };

    let audio = match line::Duplex::open(Some(&input), Some(&output), FS) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not open the line: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "out: {} at {} Hz\nin:  {} at {} Hz",
        audio.output_device, audio.output_rate, audio.input_device, audio.input_rate
    );

    // Let the streams settle before believing anything they say, draining as
    // we wait. Sleeping through it and reading afterwards loses everything
    // that arrived in the meantime and reports it as samples lost, which is
    // true, but is the test's own doing and says nothing about the line.
    {
        let mut scratch = Vec::new();
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until {
            audio.receive(&mut scratch);
            scratch.clear();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    println!("\n  Hz     returned    relative");
    let mut results: Vec<(f64, f64)> = Vec::new();
    for freq in [300.0, 600.0, 1_000.0, 1_500.0, 1_800.0, 2_400.0, 3_000.0, 3_400.0] {
        let level = measure(&audio, freq, seconds);
        results.push((freq, level));
        println!("{freq:>6.0}  {level:>10.4}", );
    }

    let peak = results.iter().map(|r| r.1).fold(0.0f64, f64::max);
    if peak < 1.0e-3 {
        println!(
            "\nNothing came back. The output and input are not joined: check that\n\
             --out feeds whatever --in is listening to."
        );
        return ExitCode::FAILURE;
    }

    println!("\nrelative to the strongest:");
    for (freq, level) in &results {
        let db = 20.0 * (level / peak).log10();
        let bar = "#".repeat(((db + 40.0).max(0.0) / 2.0) as usize);
        println!("{freq:>6.0}  {db:>6.1} dB  {bar}");
    }

    let band: Vec<f64> = results
        .iter()
        .filter(|(f, _)| (600.0..=3_000.0).contains(f))
        .map(|(_, l)| *l)
        .collect();
    let low = band.iter().fold(f64::MAX, |m, &l| m.min(l));
    let high = band.iter().fold(0.0f64, |m, &l| m.max(l));
    let tilt = 20.0 * (high / low.max(1.0e-12)).log10();
    println!(
        "\nacross 600 to 3000 Hz the path varies by {tilt:.1} dB, and the loudest\n\
         return was {peak:.3} against the {LEVEL} that was sent."
    );
    if tilt > 12.0 {
        println!(
            "That is a lot of tilt. An equaliser will spend itself undoing it and\n\
             have nothing left for the line."
        );
    }
    println!(
        "\nline: {} samples lost coming in, {} discarded going out, {} underruns, {} contended",
        audio.dropped_in(),
        audio.dropped_out(),
        audio.underruns(),
        audio.contended()
    );
    ExitCode::SUCCESS
}

/// Send one tone for `seconds` and report the amplitude that came back.
fn measure(audio: &line::Duplex, freq: f64, seconds: f64) -> f64 {
    let mut phase = 0.0f64;
    let mut heard: Vec<f32> = Vec::new();
    let mut block = Vec::new();
    let started = Instant::now();

    while started.elapsed().as_secs_f64() < seconds {
        // Read every time round, not only when the output needs feeding. What
        // arrives keeps arriving whatever this loop is busy with, and a reader
        // that goes away for as long as it takes to fill an output buffer
        // comes back to find the input has overflowed. That is not a fault in
        // the line; it is a fault in the thing reading it, and a modem loop
        // has exactly the same obligation.
        audio.receive(&mut heard);
        // Keep the output fed a block at a time. Sending far ahead of the
        // device only adds delay to what comes back.
        if audio.pending() < 2048 {
            block.clear();
            for _ in 0..512 {
                block.push((LEVEL * (TAU * phase).sin()) as f32);
                phase += freq / FS;
                if phase >= 1.0 {
                    phase -= 1.0;
                }
            }
            audio.transmit(&block);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    audio.receive(&mut heard);

    if heard.len() < 4_000 {
        return 0.0;
    }
    // The second half only: the first is whatever was still in flight from the
    // tone before, and the path's own delay is not known in advance.
    let settled = &heard[heard.len() / 2..];
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &s) in settled.iter().enumerate() {
        let w = TAU * freq * n as f64 / FS;
        re += f64::from(s) * w.cos();
        im -= f64::from(s) * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / settled.len() as f64
}
