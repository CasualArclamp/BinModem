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
        // A frame that fails its check is not a frame, but it is worth knowing
        // there was one: a long burst that reads as one short DIS has had most
        // of itself lost.
        if reader.bad > 0 {
            println!("channel {channel}: {} frames failed their check", reader.bad);
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

/// What the high-speed receiver makes of a recording, level by level.
///
/// ```text
/// FAX_CAPTURE=... cargo test -p fax --test replay -- --ignored --nocapture
/// ```
///
/// The one number that decides whether a page can arrive at all is the level
/// the carrier detector sees, because everything downstream is held still
/// until it says there is a carrier. A threshold set from a loopback, where
/// the far end is exactly as loud as this end wrote it, is a threshold set
/// from the one case that cannot go wrong.
#[test]
#[ignore = "needs a recording"]
fn what_the_fast_receiver_saw() {
    let path = std::env::var("FAX_CAPTURE").expect("set FAX_CAPTURE");
    let wav = line::wav::read(&path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    println!("\n{path}\n");
    for channel in 0..2 {
        let samples = wav.channel(channel);
        if samples.is_empty() {
            continue;
        }
        let mut rx = datapump::v27ter::Receiver::new(fs);
        rx.set_rate(datapump::v27ter::Rate::R4800);
        let mut peak = 0.0f64;
        let mut spans: Vec<(f64, f64, f64)> = Vec::new();
        let mut up: Option<(f64, f64)> = None;
        // The loudest tenth of a second anywhere, so a threshold can be
        // judged against what actually arrived rather than against nothing.
        let mut window = 0.0f64;
        let mut best_quiet = 0.0f64;
        for (i, &s) in samples.iter().enumerate() {
            rx.feed(f64::from(s));
            let level = rx.level();
            peak = peak.max(level);
            window = window.max(level);
            let at = i as f64 / fs;
            match (rx.carrier(), up) {
                (true, None) => up = Some((at, level)),
                (true, Some((from, loudest))) => up = Some((from, loudest.max(level))),
                (false, Some((from, loudest))) => {
                    spans.push((from, at, loudest));
                    up = None;
                }
                (false, None) => best_quiet = best_quiet.max(level),
            }
        }
        if let Some((from, loudest)) = up {
            spans.push((from, samples.len() as f64 / fs, loudest));
        }
        let side = if channel == 0 { "far " } else { "near" };
        println!(
            "{side}  loudest {peak:.5}  loudest while it thought the line was \
             quiet {best_quiet:.5}"
        );
        for (from, to, loudest) in &spans {
            println!("      carrier {from:7.2}s to {to:7.2}s   peak {loudest:.5}");
        }
        if spans.is_empty() {
            println!("      no carrier found at all");
        }
        // And the whole trace, half a second at a time, so a threshold can be
        // put somewhere between the quiet and the loud rather than guessed.
        let mut rx = datapump::v27ter::Receiver::new(fs);
        rx.set_rate(datapump::v27ter::Rate::R4800);
        let step = (fs * 0.5) as usize;
        let mut line = String::new();
        for (i, &s) in samples.iter().enumerate() {
            rx.feed(f64::from(s));
            if i % step == step - 1 {
                line.push_str(&format!(" {:.4}", rx.level()));
                if line.len() > 100 {
                    println!("      {:6.1}s {line}", (i + 1) as f64 / fs);
                    line.clear();
                }
            }
        }
        if !line.is_empty() {
            println!("      end     {line}");
        }
    }
}

/// Every stretch of page carrier in a recording, and what was in it.
///
/// ```text
/// FAX_CAPTURE=... FAX_FROM=2.0 FAX_TO=4.8 cargo test -p fax --test replay \
///     what_the_page_carrier_carried -- --ignored --nocapture
/// ```
///
/// Reads the far end's channel with the V.27 ter receiver across the window
/// given, as one burst however the carrier detector chops it up, and reports
/// what a training check reader would: where the zeros start, how long they
/// run, and what fraction of the next second and a half is zeros.
#[test]
#[ignore = "needs a recording"]
fn what_the_page_carrier_carried() {
    let path = std::env::var("FAX_CAPTURE").expect("set FAX_CAPTURE");
    let from: f64 = std::env::var("FAX_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let to: f64 = std::env::var("FAX_TO").ok().and_then(|v| v.parse().ok()).unwrap_or(f64::MAX);
    let wav = line::wav::read(&path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    let samples = wav.channel(0);
    let start = (from * fs) as usize;
    let end = ((to * fs) as usize).min(samples.len());

    let mut rx = datapump::v27ter::Receiver::new(fs);
    rx.set_rate(datapump::v27ter::Rate::R4800);
    let mut bits = Vec::new();
    for &s in &samples[start..end] {
        rx.feed(f64::from(s));
        bits.extend(rx.take_bits());
    }

    let mut run = 0usize;
    let mut longest = 0usize;
    let mut first32 = None;
    for (i, &b) in bits.iter().enumerate() {
        run = if b { 0 } else { run + 1 };
        longest = longest.max(run);
        if run == 32 && first32.is_none() {
            first32 = Some(i + 1 - 32);
        }
    }
    println!("\n{path}  {from:.2}s to {to:.2}s");
    println!("  bits out           {}", bits.len());
    println!("  longest zeros      {longest}");
    match first32 {
        None => println!("  no run of 32 zeros anywhere"),
        Some(at) => {
            let window = &bits[at..(at + 7200).min(bits.len())];
            let zeros = window.iter().filter(|b| !**b).count();
            println!(
                "  zeros start at bit {at}, and {zeros} of the next {} are zeros ({:.1}%)",
                window.len(),
                100.0 * zeros as f64 / window.len().max(1) as f64
            );
        }
    }
    println!(
        "  residual error     {:.3} of a gap",
        rx.residual_error() / rx.point_spacing()
    );
}
