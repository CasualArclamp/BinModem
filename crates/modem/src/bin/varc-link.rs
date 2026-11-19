//! V.ARC over a sound card, with nothing else in the way.
//!
//! The other tools here place a call: they want a telephone line, or
//! something pretending to be one, and a far end that answers. V.ARC has no
//! call to place. It wants a stereo lead, and this is the shortest path from
//! the modulation to one: open a device at its own rate, run an end or two
//! against it, push a known stream through and say what came out.
//!
//! ```text
//! varc-link --list
//! varc-link --in "CABLE-A Output" --out "CABLE-A Input" --loop
//! varc-link --in "Line In" --out "Speakers" --oneway --seconds 60
//! varc-link --in "Line In" --out "Speakers" --loop --mode qam
//! ```
//!
//! `--mode` is `dmt`, the multicarrier mode and the default, or `qam` or
//! `pcm`, the two at 16 000 baud.
//!
//! `--loop` is two ends in one program on one lead: one sends on the left
//! channel and listens on the right, the other the reverse. It needs the
//! lead to join each output channel to the same input channel, and it is a
//! whole V.ARC link with each end loading the other's line.
//!
//! `--oneway` measures what both channels together will carry: one end
//! sends on left and right, the other listens on both, and the replies
//! (the bit tables, the acknowledgements) go back inside the program.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use line::Wide;
use varc::dmt::{self, Plan};
use varc::link::{Receiver, Sender};
use varc::sc::{self, Shape};
use varc::station::Station;

type End = Station<Box<dyn Sender>, Box<dyn Receiver>>;

/// Which modulation, and so what to build an end out of.
#[derive(Clone)]
enum Mode {
    Dmt(Plan),
    Single(Shape),
}

impl Mode {
    /// One line's two halves: a transmitter using the known points
    /// numbered `sends`, and a receiver listening for those numbered
    /// `hears`.
    fn pair(&self, sends: u8, hears: u8, level: Option<f64>, gap: Option<f64>) -> (Box<dyn Sender>, Box<dyn Receiver>) {
        match self {
            Mode::Dmt(plan) => {
                let mut rx = dmt::Rx::new(plan.clone().coloured(hears));
                rx.set_gap_db(gap.unwrap_or(dmt::GAP_DB));
                let tx = dmt::Tx::at_level(plan.clone().coloured(sends), level.unwrap_or(dmt::LEVEL));
                (Box::new(tx), Box::new(rx))
            }
            Mode::Single(shape) => {
                let mut rx = sc::Rx::new(shape.clone().coloured(hears));
                rx.set_gap_db(gap.unwrap_or(sc::GAP_DB));
                let tx = sc::Tx::at_level(shape.clone().coloured(sends), level.unwrap_or(sc::LEVEL));
                (Box::new(tx), Box::new(rx))
            }
        }
    }
}

/// A stream both ends can make for themselves, so what arrives can be
/// checked without it having been kept.
struct Pattern(u64);

impl Pattern {
    fn next(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 56) as u8
    }
}

/// One direction of the test: octets written at one end and checked at the
/// other.
struct Flow {
    name: &'static str,
    source: Pattern,
    check: Pattern,
    arrived: u64,
    wrong: u64,
    last: u64,
}

impl Flow {
    fn new(name: &'static str, seed: u64) -> Self {
        Self { name, source: Pattern(seed), check: Pattern(seed), arrived: 0, wrong: 0, last: 0 }
    }

    /// Keep the sending end with plenty to send.
    fn feed(&mut self, end: &mut End) {
        if end.arq.pending() < 200_000 {
            let more: Vec<u8> = (0..100_000).map(|_| self.source.next()).collect();
            end.write(&more);
        }
    }

    fn take(&mut self, end: &mut End) {
        for octet in end.read() {
            self.arrived += 1;
            if octet != self.check.next() {
                self.wrong += 1;
            }
        }
    }
}

/// An end whose channels send with known points numbered from `sends` and
/// listen for ones numbered from `hears`, so that every line in the lead has
/// its own and each receiver is told its transmitter's.
fn end(mode: &Mode, fs: f64, channels: u8, sends: u8, hears: u8, level: Option<f64>, gap: Option<f64>) -> End {
    Station::new(fs, (0..channels).map(|c| mode.pair(sends + c, hears + c, level, gap)).collect())
}

fn describe(name: &str, end: &End) -> String {
    let mut out = format!("{name}:");
    for channel in &end.channels {
        let health = channel.rx.health();
        let (_, net) = channel.tx.rates();
        let state = if health.steady {
            "steady"
        } else if health.locked {
            "training"
        } else {
            "hunting"
        };
        out += &format!(
            " [{state} snr {:.1} dB clock {:+.1} ppm, sending table {} at {:.0} kbit/s, {}/{} words bad{}]",
            health.snr_db,
            health.clock_ppm,
            channel.tx.id(),
            net / 1000.0,
            channel.tally.failed,
            channel.tally.words,
            match channel.tx.clipped() {
                0 => String::new(),
                n => format!(", {n} samples clipped"),
            },
        );
    }
    out
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: varc-link --list\n       varc-link --in NAME --out NAME (--loop | --oneway) [--mode dmt|qam|pcm] [--seconds N] [--level RMS] [--gap DB] [--csv FILE]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input = None;
    let mut output = None;
    let mut oneway = None;
    let mut seconds = 30.0;
    let mut level = None;
    let mut gap = None;
    let mut csv = None;
    let mut mode = String::from("dmt");
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned();
        match arg.as_str() {
            "--list" => {
                println!("inputs:");
                for name in line::input_devices() {
                    println!("  {name}");
                }
                println!("outputs:");
                for name in line::output_devices() {
                    println!("  {name}");
                }
                return ExitCode::SUCCESS;
            }
            "--in" => input = value(),
            "--out" => output = value(),
            "--loop" => oneway = Some(false),
            "--oneway" => oneway = Some(true),
            "--csv" => csv = value(),
            "--mode" => match value() {
                Some(name) => mode = name,
                None => return usage(),
            },
            "--seconds" | "--level" | "--gap" => {
                let Some(number) = value().and_then(|v| v.parse::<f64>().ok()) else {
                    return usage();
                };
                match arg.as_str() {
                    "--seconds" => seconds = number,
                    "--level" => level = Some(number),
                    _ => gap = Some(number),
                }
            }
            _ => return usage(),
        }
    }
    let Some(oneway) = oneway else { return usage() };

    let audio = match Wide::open(input.as_deref(), output.as_deref()) {
        Ok(audio) => audio,
        Err(e) => {
            eprintln!("could not open the sound devices: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "in:  {} ({} ch)\nout: {} ({} ch)\nat {} Hz",
        audio.input_device, audio.input_channels, audio.output_device, audio.output_channels, audio.rate
    );
    let fs = f64::from(audio.rate);
    let mode = match mode.as_str() {
        "dmt" => {
            let plan = Plan::for_rate(fs);
            println!(
                "V.ARC/DMT: {} carriers {:.1} Hz apart, {:.1} ms a symbol",
                plan.half() - 1,
                plan.frequency(1),
                plan.span() as f64 / plan.fs * 1000.0
            );
            Mode::Dmt(plan)
        }
        "qam" | "pcm" => {
            let shape = if mode == "qam" { Shape::qam(fs) } else { Shape::pcm(fs) };
            let Some(shape) = shape else {
                eprintln!("the 16 000 baud modes want a sample rate that is a multiple of 48 kHz, not {fs} Hz");
                return ExitCode::FAILURE;
            };
            println!(
                "V.ARC/{}: {} baud, {} samples a symbol",
                if shape.real { "PCM" } else { "QAM" },
                sc::BAUD,
                shape.sps
            );
            Mode::Single(shape)
        }
        _ => return usage(),
    };

    // A sends to B over the line. In a loop B sends back over the other
    // channel; one way, it sends back through `inside`.
    let channels = if oneway { 2 } else { 1 };
    let mut a = end(&mode, fs, channels as u8, 0, 2, level, gap);
    let mut b = end(&mode, fs, channels as u8, 2, 0, level, gap);
    let mut a_to_b = Flow::new("A to B", 1);
    let mut b_to_a = Flow::new("B to A", 2);
    let mut inside = vec![0.0f64; channels];
    let mut from_a = vec![0.0f64; channels];
    let mut from_b = vec![0.0f64; channels];

    let mut heard: Vec<f32> = Vec::new();
    let mut send: Vec<f32> = Vec::new();
    let started = Instant::now();
    let mut reported = Instant::now();
    let mut peak = 0.0f32;
    while started.elapsed().as_secs_f64() < seconds {
        heard.clear();
        audio.receive(&mut heard);
        if heard.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        send.clear();
        for frame in heard.as_chunks::<2>().0 {
            peak = peak.max(frame[0].abs()).max(frame[1].abs());
            let (left, right) = (f64::from(frame[0]), f64::from(frame[1]));
            if oneway {
                a.step(&inside, &mut from_a);
                b.step(&[left, right], &mut from_b);
                inside.copy_from_slice(&from_b);
                send.push(from_a[0] as f32);
                send.push(from_a[1] as f32);
            } else {
                a.step(&[right], &mut from_a);
                b.step(&[left], &mut from_b);
                send.push(from_a[0] as f32);
                send.push(from_b[0] as f32);
            }
        }
        audio.transmit(&send);
        a_to_b.feed(&mut a);
        a_to_b.take(&mut b);
        if !oneway {
            b_to_a.feed(&mut b);
            b_to_a.take(&mut a);
        }

        if reported.elapsed() >= Duration::from_secs(1) {
            let span = reported.elapsed().as_secs_f64();
            reported = Instant::now();
            println!("{:5.1}s  input peak {:.3}", started.elapsed().as_secs_f64(), peak);
            peak = 0.0;
            println!("  {}", describe("A", &a));
            println!("  {}", describe("B", &b));
            for flow in [&mut a_to_b, &mut b_to_a] {
                if flow.arrived > 0 {
                    println!(
                        "  {}: {:.0} kbit/s arriving, {} octets, {} wrong",
                        flow.name,
                        (flow.arrived - flow.last) as f64 * 8.0 / span / 1000.0,
                        flow.arrived,
                        flow.wrong
                    );
                }
                flow.last = flow.arrived;
            }
        }
    }

    println!();
    println!(
        "line: {} input frames dropped, {} output frames dropped, {} output underruns",
        audio.dropped_in(),
        audio.dropped_out(),
        audio.underruns()
    );
    println!("  {}", describe("A", &a));
    println!("  {}", describe("B", &b));
    println!("  A resent {} frames, B resent {}", a.arq.resent, b.arq.resent);
    let mut good = true;
    for flow in [&a_to_b, &b_to_a] {
        if flow.name == "B to A" && oneway {
            continue;
        }
        println!(
            "  {}: {} octets, {} wrong, {:.0} kbit/s over the whole run",
            flow.name,
            flow.arrived,
            flow.wrong,
            flow.arrived as f64 * 8.0 / seconds / 1000.0
        );
        good &= flow.arrived > 0 && flow.wrong == 0;
    }
    if let Some(path) = csv {
        let mut text = String::from("end,channel,hz,snr_db,bits\n");
        for (name, end) in [("A", &a), ("B", &b)] {
            for (c, channel) in end.channels.iter().enumerate() {
                for bin in channel.rx.spectrum() {
                    text += &format!("{name},{c},{:.1},{:.2},{}\n", bin.hz, bin.snr_db, bin.bits);
                }
            }
        }
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("could not write {path}: {e}");
        }
    }
    if good { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
