//! A modem on a real line, driven from the keyboard.
//!
//! Audio comes in from one device, goes through the modem, and goes back out
//! to another. Type `AT` commands and they are answered; type `ATD` and it
//! dials; once connected, what is typed goes down the line and what comes back
//! is printed.
//!
//! The wiring this expects, for a call over a softphone:
//!
//! ```text
//!   softphone speaker  ->  virtual cable A  ->  --in   (what the far end says)
//!   --out              ->  virtual cable B  ->  softphone microphone
//! ```
//!
//! Two separate cables, and the reason is that one cable would join the
//! modem's own output straight back to its own input. A V.22bis modem would
//! survive that, since the two directions live in different halves of the band
//! and the receiver filters; a V.32 modem would not, because both directions
//! share the band and the echo canceller is trained against a far end that is
//! required to be silent, which its own transmitter is not.
//!
//! The line side is clocked by the input device. Samples arrive, the modem
//! takes one step for each, and the results go out. If the two devices run at
//! different rates the output eventually runs dry and sends silence, which is
//! counted and reported: on a long call it is the number worth watching.

use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use modem::{Modem, State};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut rate = 16_000.0f64;

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--list" => {
                list_devices();
                return ExitCode::SUCCESS;
            }
            "--in" => input = Some(value()),
            "--out" => output = Some(value()),
            "--rate" => rate = value().parse().unwrap_or(rate),
            "--help" | "-h" => {
                println!(
                    "modem-dial [--list] [--in <device>] [--out <device>] [--rate <hz>]\n\
                     \n\
                     Type AT commands. ATD to dial, ATA to answer, +++ then ATH to hang up.\n\
                     The input and output should be two different virtual cables; one\n\
                     cable joins the modem's output to its own input."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let line = match line::Duplex::open(input.as_deref(), output.as_deref(), rate) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not open the line: {e}");
            eprintln!("try --list to see what is available");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "in:  {} at {} Hz\nout: {} at {} Hz\nmodem at {rate} Hz. Type AT commands; \
         Ctrl-C to quit.",
        line.input_device, line.input_rate, line.output_device, line.output_rate
    );
    if line.input_device == line.output_device {
        eprintln!(
            "warning: the same device for both directions joins this modem's output\n\
             to its own input, which is not a telephone line."
        );
    }

    run(line, rate)
}

fn list_devices() {
    println!("input devices (default first):");
    for name in line::input_devices() {
        println!("  {name}");
    }
    println!("output devices (default first):");
    for name in line::output_devices() {
        println!("  {name}");
    }
}

fn run(audio: line::Duplex, rate: f64) -> ExitCode {
    let mut modem = Modem::new(rate);
    let keys = spawn_keyboard();

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut was = State::Command;
    let mut last_report = Instant::now();

    loop {
        // Whatever the terminal has typed. In command state a modem answers it
        // and in data state it goes down the line; either way the modem
        // decides, not this loop.
        while let Ok(chunk) = keys.try_recv() {
            for byte in chunk {
                modem.feed_dte(byte);
            }
        }

        // The line is the clock: one step of the modem for each sample that
        // arrives, and one sample back out for each step.
        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            std::thread::sleep(Duration::from_millis(2));
        } else {
            to_line.clear();
            for &s in &from_line {
                to_line.push(modem.step(f64::from(s)) as f32);
            }
            audio.transmit(&to_line);
        }

        let out = modem.take_dte();
        if !out.is_empty() {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&out);
            let _ = stdout.flush();
        }

        if modem.state() != was {
            was = modem.state();
            eprintln!(
                "\n[{was:?}{}]",
                match modem.rate() {
                    Some(r) => format!(
                        ", {r} bit/s, {}{}",
                        if modem.error_controlled() { "V.42" } else { "no error control" },
                        if modem.compressing() { " with V.42bis" } else { "" }
                    ),
                    None => String::new(),
                }
            );
        }

        if last_report.elapsed() >= Duration::from_secs(10) {
            last_report = Instant::now();
            let (lost, under) = (audio.dropped_in(), audio.underruns());
            if lost > 0 || under > 0 {
                eprintln!("[line: {lost} samples lost coming in, {under} underruns]");
            }
        }
    }
}

/// Read the keyboard on its own thread, since reading it blocks and the line
/// will not wait.
fn spawn_keyboard() -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 256];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    // A terminal in its usual mode hands over a whole line at
                    // once and strips nothing, so the newline arrives as the
                    // platform writes it. A modem wants a carriage return.
                    let mut chunk = Vec::with_capacity(n);
                    for &b in &buf[..n] {
                        if b == b'\n' {
                            chunk.push(b'\r');
                        } else {
                            chunk.push(b);
                        }
                    }
                    if tx.send(chunk).is_err() {
                        return;
                    }
                }
            }
        }
    });
    rx
}
