//! Dump the descrambled bit stream of one channel of a capture.
//!
//! Ignored, because it needs a capture.
//!
//! ```text
//! V32_CAPTURE=... V32_CHANNEL=1 V32_BITS=target/probe/sent.bits \
//!     cargo test -p datapump --test v32_bits -- --ignored --nocapture
//! ```
//!
//! Channel 1 is what this modem sent, which arrives at the far end's receiver
//! looking exactly like this. Writing it out lets the rate exchange be read as
//! aligned 16-bit words rather than through a detector that resynchronises on
//! every bit.

use datapump::v32::{Mode, Receiver};

const FS: f64 = 16_000.0;

#[test]
#[ignore = "needs a capture"]
fn dump_bits() {
    let path = std::env::var("V32_CAPTURE").expect("set V32_CAPTURE");
    let out = std::env::var("V32_BITS").expect("set V32_BITS");
    let channel: usize = std::env::var("V32_CHANNEL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "built for 16 kHz");

    // Channel 1 was sent by the calling modem, so it is descrambled by a
    // receiver belonging to the answering end, and the reverse.
    let mode = if channel == 1 { Mode::Answer } else { Mode::Call };
    let x = wav.channel(channel);
    let mut rx = Receiver::new(mode, FS);
    let mut bits: Vec<u8> = Vec::new();
    let mut when: Vec<f32> = Vec::new();
    for (i, &s) in x.iter().enumerate() {
        rx.feed(f64::from(s));
        for b in rx.take_bits() {
            bits.push(u8::from(b));
            when.push(i as f32 / FS as f32);
        }
    }
    println!("{} bits from channel {channel} of {path}", bits.len());
    let text: String = bits.iter().map(|b| char::from(b'0' + b)).collect();
    std::fs::write(&out, &text).unwrap();
    let times: String =
        when.iter().map(|t| format!("{t:.4}\n")).collect::<Vec<_>>().concat();
    std::fs::write(format!("{out}.time"), times).unwrap();
    println!("written to {out}");
}
