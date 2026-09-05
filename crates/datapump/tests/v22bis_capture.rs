//! Replay a recorded live call through the V.22bis receiver.
//!
//! Ignored, because it needs a capture and captures are not in the repository:
//! they are large, and the ones worth having come off somebody's telephone
//! line. Point it at one with
//!
//! ```text
//! V22_CAPTURE=captures/live-1788613347.wav cargo test -p datapump \
//!     --test v22bis_capture -- --ignored --nocapture
//! ```
//!
//! The file has two channels: what arrived on the first and what this modem
//! was transmitting at the same instant on the second. Only the first is fed
//! in — the receiver is being asked to do exactly what it did on the day, with
//! the far end's own signal, as many times as it takes.

use datapump::v22bis::handshake::{Modem, Role, Status};

const FS: f64 = 16_000.0;

fn capture() -> Option<(Vec<f32>, Vec<f32>)> {
    let path = std::env::var("V22_CAPTURE").ok()?;
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.channels, 2, "wanted a recording of both directions");
    assert_eq!(
        wav.sample_rate as f64, FS,
        "the receiver is built for 16 kHz"
    );
    println!(
        "{path}: {:.1} s, {} channels at {} Hz",
        wav.duration_secs(),
        wav.channels,
        wav.sample_rate
    );
    Some((wav.channel(0), wav.channel(1)))
}

#[test]
#[ignore]
fn replay() {
    let Some((heard, sent)) = capture() else {
        println!("set V22_CAPTURE to a recording; nothing to do");
        return;
    };

    let mut modem = Modem::new(Role::Calling, FS);
    let mut last = Status::Negotiating;
    let mut bytes: Vec<u8> = Vec::new();
    let mut framer = datapump::AsyncBits::new(8);
    let mut connected_at = f64::NAN;
    // Residual error every quarter second, which is what says whether the
    // receiver is looking at the constellation it thinks it is.
    let block = (FS / 4.0) as usize;

    for (i, &s) in heard.iter().enumerate() {
        modem.step(f64::from(s));
        // The pump hands up bits; without error control the characters are
        // found by their own start and stop bits, exactly as the modem does.
        for bit in modem.take_bits() {
            if let Some(c) = framer.feed(bit) {
                bytes.push(c);
            }
        }

        let now = modem.status();
        if now != last {
            println!(
                "{:>7.2}s  {last:?} -> {now:?}",
                i as f64 / FS
            );
            if matches!(now, Status::Connected(_)) && connected_at.is_nan() {
                connected_at = i as f64 / FS;
            }
            last = now;
        }
        if i % block == 0 && i > 0 && matches!(now, Status::Connected(_)) {
            let (re, im) = modem.constellation_point();
            println!(
                "{:>7.2}s  err {:.3}  point ({re:>6.2}, {im:>6.2})  {} bytes",
                i as f64 / FS,
                modem.residual_error(),
                bytes.len()
            );
        }
    }

    println!("\nfinal: {:?}, {} bytes recovered", modem.status(), bytes.len());
    let text: String = bytes
        .iter()
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
        .collect();
    for line in text.as_bytes().chunks(72).take(20) {
        println!("  {}", String::from_utf8_lossy(line));
    }
    let printable = bytes.iter().filter(|b| (0x20..0x7f).contains(&(**b as u32))).count();
    println!(
        "\n{printable} of {} bytes printable ({:.0}%). What we transmitted meanwhile \
         had rms {:.3}.",
        bytes.len(),
        100.0 * printable as f64 / bytes.len().max(1) as f64,
        (sent.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>()
            / sent.len() as f64)
            .sqrt()
    );
}
