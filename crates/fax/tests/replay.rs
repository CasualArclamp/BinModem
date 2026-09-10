//! Read a recorded fax call with the code that will place one.
//!
//! ```text
//! FAX_CAPTURE=F:/dialupmodem2/dist/captures/live-1788944274.wav \
//!     cargo test -p fax --test replay -- --ignored --nocapture
//! ```
//!
//! Ignored, because it needs a recording. The point of it is that nothing in
//! here is a test harness: it is the V.21 receiver, the HDLC decoder and the
//! T.30 frame reader that a real call will use, pointed at a real call
//! somebody else made. A fax that cannot read a recording of a fax is not
//! going to do better against a live one.

use datapump::v21;
use fax::{frames, t30};

/// Both directions, on one timeline.
fn read(path: &str) -> Vec<(f64, usize, frames::Message)> {
    let wav = line::wav::read(path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    let mut out = Vec::new();
    for channel in 0..2 {
        let samples = wav.channel(channel);
        if samples.is_empty() {
            continue;
        }
        let mut rx = v21::Receiver::new(fs);
        let mut reader = frames::Reader::new();
        for (i, &s) in samples.iter().enumerate() {
            if let Some(bit) = rx.feed(f64::from(s))
                && let Some(message) = reader.feed(bit)
            {
                out.push((i as f64 / fs, channel, message));
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    // A recording of a two-wire line has both directions in both channels,
    // one of them faintly, so a frame loud enough to read twice is read
    // twice. The second copy is not a second frame.
    let mut once: Vec<(f64, usize, frames::Message)> = Vec::new();
    for (at, channel, m) in out {
        if once
            .iter()
            .any(|(t, _, seen)| at - t < 0.5 && *seen == m)
        {
            continue;
        }
        once.push((at, channel, m));
    }
    once
}

#[test]
#[ignore = "needs a recording"]
fn what_the_two_ends_said() {
    let path = std::env::var("FAX_CAPTURE").expect("set FAX_CAPTURE");
    let messages = read(&path);
    println!("\n{path}\n");
    for (at, channel, m) in &messages {
        let side = if *channel == 0 { "far " } else { "near" };
        let text: String = m
            .fif
            .iter()
            .map(|&c| if (32..127).contains(&c) { c as char } else { '.' })
            .collect();
        println!(
            "{at:7.2}s {side} {:<4} {}{}",
            m.frame.name(),
            m.frame.meaning(),
            if m.fif.is_empty() {
                String::new()
            } else {
                format!("  [{}]  |{text}|", m.fif.len())
            }
        );
        if m.frame == t30::Frame::Dis {
            for (k, v) in t30::capabilities(&m.fif).rows() {
                println!("            {k:<18} {v}");
            }
        }
        if m.frame == t30::Frame::Dcs {
            match t30::command_rate(&m.fif) {
                Some((how, rate)) => println!(
                    "            {:<18} {} at {rate} bit/s",
                    "the page will be",
                    how.name()
                ),
                None => println!("            {:<18} a rate this does not know", "the page will be"),
            }
            let caps = t30::capabilities(&m.fif);
            println!(
                "            {:<18} {}",
                "resolution",
                if caps.fine_resolution { "7.7 lines/mm" } else { "3.85 lines/mm" }
            );
        }
        if matches!(m.frame, t30::Frame::Csi | t30::Frame::Tsi) {
            println!("            {:<18} {}", "identification", t30::identification(&m.fif));
        }
    }
    println!("\n{} frames", messages.len());
    assert!(
        !messages.is_empty(),
        "read no frames at all out of a fax call"
    );
}
