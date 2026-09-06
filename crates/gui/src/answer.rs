//! A modem sitting on the line waiting to be dialled: a board to call.
//!
//! The companion to `--live`, and in the same program as it. That window has
//! a modem in it and a terminal wired to it, and nothing to ring. This is the
//! thing that answers.
//!
//! Both point at the same virtual cable, and that is not a compromise: what
//! comes back from a cable is what was written to it, a little later, summed
//! with whatever else is writing. Which is a two-wire pair, exactly, with two
//! modems across it. Each hears the other and its own reflection, which is the
//! situation every one of these modulations was designed for.
//!
//! Once connected it behaves like the simplest possible board: a banner, and
//! then an echo of whatever is typed, so that what comes back on the screen is
//! proof it went down the line and returned rather than proof the terminal can
//! draw its own keystrokes.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use modem::{Modem, State};

const FS: f64 = 16_000.0;

/// How loud to write to the line.
///
/// Two modems share the cable and it sums them, so each has to leave room for
/// the other. Half is generous: pulse shaping puts the peak of a single modem
/// well above its own average, and a clipped handshake is a failed one.
const LEVEL: f32 = 0.45;

/// Run the answering modem. `args` is what followed `--answer`.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut carrier = "V22B".to_owned();
    let mut banner =
        "\r\n\r\n*** THE DEAD ZONE BBS ***\r\n  1200 baud - 24 hours - SysOp: nobody\r\n\r\nlogin: "
            .to_owned();

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--in" => input = Some(value()),
            "--out" => output = Some(value()),
            "--carrier" => carrier = value().to_ascii_uppercase(),
            "--banner" => banner = value(),
            "--help" | "-h" => {
                println!(
                    "modem-scope --answer --in <device> --out <device> \
                     [--carrier B103|V22B|V32] [--banner <text>]\n\
                     \n\
                     Answers calls on a virtual cable and echoes what is typed,\n\
                     so that `modem-scope --live` on the same cable has\n\
                     something to dial. Both devices must be named."
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
        "out: {} at {} Hz\nin:  {} at {} Hz\nanswering as {carrier}; ctrl-c to stop",
        audio.output_device, audio.output_rate, audio.input_device, audio.input_rate
    );

    let mut host = Modem::new(FS);
    for b in format!("AT+MS={carrier}\r").bytes() {
        host.feed_dte(b);
    }
    host.take_dte();
    // Off hook and waiting. There is no ring on a virtual cable, so this
    // answers straight away and waits for a calling modem to appear.
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut greeted = false;
    let mut connected_at = Instant::now();
    let mut last_phase = "";
    let started = Instant::now();

    loop {
        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        to_line.clear();
        for &s in &from_line {
            to_line.push(host.step(f64::from(s)) as f32 * LEVEL);
        }
        audio.transmit(&to_line);

        let phase = host.line_phase();
        if phase != last_phase {
            last_phase = phase;
            println!("[{:>6.2}s {phase}]", started.elapsed().as_secs_f64());
        }

        let heard = host.take_dte();
        if host.state() == State::Data {
            if !greeted {
                greeted = true;
                connected_at = Instant::now();
                println!(
                    "[{:>6.2}s connected at {} bit/s, error control {}]",
                    started.elapsed().as_secs_f64(),
                    host.rate().unwrap_or(0),
                    if host.error_controlled() { "on" } else { "off" }
                );
            }
            // A moment before speaking. Both ends have a receiver that has
            // only just stopped training, and a banner sent into that is a
            // banner half of which is never seen.
            if greeted && connected_at.elapsed() > Duration::from_millis(500) && !banner.is_empty()
            {
                for b in banner.bytes() {
                    host.feed_dte(b);
                }
                banner.clear();
            }
            if !heard.is_empty() {
                print!("{}", String::from_utf8_lossy(&heard));
                use std::io::Write;
                let _ = std::io::stdout().flush();
                // Echo it back, which is what a board does and what makes the
                // characters appear on the caller's screen at all.
                for b in &heard {
                    host.feed_dte(*b);
                    // A bare return from a terminal wants a line feed with it.
                    if *b == b'\r' {
                        host.feed_dte(b'\n');
                    }
                }
            }
        } else if greeted && host.state() == State::Command {
            println!("\n[caller hung up]");
            greeted = false;
            banner = "\r\nlogin: ".to_owned();
            for b in b"ATA\r" {
                host.feed_dte(*b);
            }
        }
    }
}
