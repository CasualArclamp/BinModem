//! Two modems calling each other through the sound card.
//!
//! The tests place calls between two modems in memory, which proves the
//! modulation and the protocols and nothing at all about audio. This places
//! the same call through a real device: out through one, back in through the
//! other, with whatever rate conversion, buffering and clock drift the machine
//! actually has.
//!
//! A single virtual cable is enough, and is in fact exactly right. What comes
//! back from a cable is what was written to it, a little later, which is
//! precisely what a two-wire line does: both directions on one pair, summed,
//! delayed by however long the path takes. The two modems hear each other
//! because they hear everything, which is the situation each of them was
//! designed for.
//!
//! What this cannot do is reach another machine. For that the output has to
//! go to a softphone's microphone and the input come from its speaker, which
//! is two cables, because one cable can only be pointed one way.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use modem::{Modem, State};

const FS: f64 = 16_000.0;

/// Headroom for the sum of two modems on one pair.
///
/// Each peaks well above its own average because of the pulse shaping, and two
/// of them add. Measured on the same call written to a file, the sum reaches
/// about one and a half, so a half leaves room and a little over.
const HEADROOM: f32 = 0.45;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut carrier = "V22B".to_owned();
    let mut seconds = 30.0f64;

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--in" => input = Some(value()),
            "--out" => output = Some(value()),
            "--carrier" => carrier = value().to_ascii_uppercase(),
            "--seconds" => seconds = value().parse().unwrap_or(seconds),
            "--help" | "-h" => {
                println!(
                    "modem-loop --in <device> --out <device> [--carrier V22B|V32] \
                     [--seconds <n>]\n\
                     \n\
                     Places a call between two modems through the sound card. Point\n\
                     --out at a virtual cable and --in at the same cable's other end.\n\
                     Both devices must be named; the default output usually has\n\
                     speakers on it."
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
        "out: {} at {} Hz\nin:  {} at {} Hz\ncarrier {carrier}",
        audio.output_device, audio.output_rate, audio.input_device, audio.input_rate
    );

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

    let greeting = "Welcome to phl6-dial1.popsite.net\r\nlogin:";
    let mut seen: Vec<u8> = Vec::new();
    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let (mut connected_at, mut spoken) = (f64::NAN, false);
    let started = Instant::now();
    let mut last_state = ("", "");

    while started.elapsed().as_secs_f64() < seconds {
        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        to_line.clear();
        for &s in &from_line {
            // Both modems hear the whole line, which is what a two-wire pair
            // gives them and what each is built to pick its own direction out
            // of.
            let heard = f64::from(s);
            let a = caller.step(heard);
            let b = host.step(heard);
            to_line.push(((a + b) as f32) * HEADROOM);
        }
        audio.transmit(&to_line);

        seen.extend(caller.take_dte());
        host.take_dte();

        let now = (caller.line_phase(), host.line_phase());
        if now != last_state {
            last_state = now;
            println!(
                "[{:>6.2}s caller {}, host {}{}]",
                started.elapsed().as_secs_f64(),
                now.0,
                now.1,
                match caller.rate() {
                    Some(r) => format!(", {r} bit/s"),
                    None => String::new(),
                }
            );
        }

        let up = caller.state() == State::Data && host.state() == State::Data;
        if up && connected_at.is_nan() {
            connected_at = started.elapsed().as_secs_f64();
        }
        if up && !spoken && started.elapsed().as_secs_f64() > connected_at + 2.0 {
            spoken = true;
            for byte in greeting.bytes() {
                host.feed_dte(byte);
            }
        }
    }

    println!(
        "\nline: {} samples lost coming in, {} underruns",
        audio.dropped_in(),
        audio.underruns()
    );
    if let Some(rt) = caller.round_trip_symbols() {
        // 2400 baud, so a symbol is a little over four hundred microseconds.
        // This is the number that decides whether V.32 has any chance here: an
        // echo canceller reaches back a fixed distance, and a reflection
        // further away than that cannot be cancelled at all.
        println!(
            "round trip, as the caller measured it: {rt} symbols, about {:.0} ms",
            rt as f64 / 2400.0 * 1000.0
        );
    }
    if let Some(loss) = caller.echo_return_loss() {
        println!("echo return loss at the caller: {loss:.1} dB");
    }
    let text = String::from_utf8_lossy(&seen);
    println!("the caller's terminal saw:\n{text}");

    if !text.contains("CONNECT") {
        eprintln!("\nthe call never connected");
        return ExitCode::FAILURE;
    }
    if !text.contains("phl6-dial1") {
        eprintln!("\nconnected, but the greeting did not come through");
        return ExitCode::FAILURE;
    }
    println!(
        "\nA call was placed and carried data, through the sound card, in {:.1} s.",
        connected_at
    );
    ExitCode::SUCCESS
}
