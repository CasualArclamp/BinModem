//! Two of these modems call each other, and each end's transmission is
//! written to its own channel of a WAV: a recording with an end a channel,
//! of a call whose every particular is known.
//!
//! ```text
//! cargo run --release -p review --example make_call -- out.wav [V34] [seconds] [retrain-at]
//! ```
//!
//! The caller is on the first channel. Both terminals type a line once the
//! call is up, and again after a retrain if one was asked for.

use modem::{Modem, State};

const FS: f64 = 16_000.0;
/// What a sample of the line is multiplied by on its way into the file.
const LEVEL: f32 = 0.25;

fn type_at(modem: &mut Modem, line: &str) {
    for b in line.bytes() {
        modem.feed_dte(b);
    }
    modem.feed_dte(b'\r');
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("make_call out.wav [carrier] [seconds] [retrain-at]");
        std::process::exit(2);
    };
    let carrier = args.get(1).map_or("V34", String::as_str);
    let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let retrain_at: Option<f64> = args.get(3).and_then(|s| s.parse().ok());

    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        type_at(m, &format!("AT+MS={carrier}"));
        // Compression off, so that what was typed is in the frames as typed.
        type_at(m, "AT+DS=0;+DS44=0");
        m.take_dte();
    }
    type_at(&mut host, "ATA");
    type_at(&mut caller, "ATD5551234");

    let mut samples = Vec::with_capacity((seconds * FS) as usize * 2);
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let (mut up_at, mut spoken, mut retrained, mut spoken_again) = (None, false, false, false);
    for n in 0..(seconds * FS) as usize {
        let now = n as f64 / FS;
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);
        // The modem's own scale has its signal near full scale and its peaks
        // past it: a quarter of that is what goes in the file.
        samples.push(LEVEL * from_caller as f32);
        samples.push(LEVEL * from_host as f32);
        let caller_said = caller.take_dte();
        let host_said = host.take_dte();
        for (who, said) in [("caller", caller_said), ("host", host_said)] {
            if !said.is_empty() {
                eprintln!("{now:8.3} {who}: {:?}", String::from_utf8_lossy(&said));
            }
        }
        let up = caller.state() == State::Data && host.state() == State::Data;
        if up && up_at.is_none() {
            up_at = Some(now);
            eprintln!("{now:8.3} both in data mode, caller at {:?} bit/s", caller.rate());
        }
        if let Some(at) = up_at {
            if !spoken && now > at + 1.5 {
                spoken = true;
                for b in b"Hello from the calling modem.\r\n" {
                    caller.feed_dte(*b);
                }
                for b in b"And hello back from the answering one.\r\n" {
                    host.feed_dte(*b);
                }
            }
            if let Some(when) = retrain_at {
                if !retrained && now > when {
                    retrained = true;
                    eprintln!("{now:8.3} the caller asks for a retrain");
                    caller.retrain();
                }
                if retrained && !spoken_again && now > when + 14.0 {
                    spoken_again = true;
                    for b in b"Still here after the retrain.\r\n" {
                        caller.feed_dte(*b);
                    }
                }
            }
        }
    }
    line::wav::write_channels(path, &samples, 2, FS as u32).expect("could not write the recording");
    eprintln!("{path}: {seconds} s, caller on channel 1, answerer on channel 2");
}
