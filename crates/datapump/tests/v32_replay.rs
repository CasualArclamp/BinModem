//! Replay a recorded V.32 call through this modem's own start-up.
//!
//! Ignored, because it needs a capture.
//!
//! ```text
//! V32_CAPTURE=F:/dialupmodem2/dist/captures/live-1788836496.wav \
//!     cargo test -p datapump --test v32_replay -- --ignored --nocapture
//! ```
//!
//! Channel 0 of a live recording is what came back off the line: the far
//! modem, plus whatever the network returned of our own signal. Feeding it to
//! a `Modem` is the call happening again, with every decision this end made
//! visible and repeatable -- which a live call is not.
//!
//! What the transmitter produces is thrown away. It cannot be put back on the
//! line, so the far end in the recording is answering the call that was made
//! rather than this one; the replay is faithful up to the first point where
//! the two would differ, and that point is what is being looked for.

use datapump::v32::startup::{Modem, Role, Status, rate_signal};

const FS: f64 = 16_000.0;

#[test]
#[ignore = "needs a capture"]
fn what_this_end_made_of_it() {
    let path = std::env::var("V32_CAPTURE").expect("set V32_CAPTURE");
    let offer = std::env::var("V32_OFFER").ok();
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "built for 16 kHz");

    let arrived = wav.channel(0);
    let offer = match offer.as_deref() {
        Some("4800") => rate_signal(true, false),
        Some("9600") => rate_signal(false, true),
        _ => rate_signal(true, true),
    };
    println!(
        "\n{path}: {:.1} s, replaying channel 0, offering {offer:016b}\n",
        wav.duration_secs()
    );

    let mut modem = Modem::new(Role::Calling, offer, FS);
    let mut phase = "";
    let mut status = Status::Negotiating;
    let mut carrier = false;
    let mut bytes = Vec::new();

    for (i, &x) in arrived.iter().enumerate() {
        let _ = modem.step(f64::from(x));
        let at = i as f64 / FS;
        if modem.phase() != phase {
            phase = modem.phase();
            println!("{at:8.3}s  phase {phase}");
        }
        if modem.status() != status {
            status = modem.status();
            println!("{at:8.3}s  status {status:?}");
        }
        if modem.carrier() != carrier {
            carrier = modem.carrier();
            println!("{at:8.3}s  carrier {carrier}");
        }
        if matches!(status, Status::Connected(_)) {
            bytes.extend(modem.take_bytes());
        }
    }

    println!(
        "\nround trip {} symbols, echo return loss {:.1} dB",
        modem.round_trip(),
        modem.echo_return_loss()
    );
    let printable = bytes
        .iter()
        .filter(|c| (32..127).contains(*c) || **c == 10 || **c == 13)
        .count();
    println!(
        "{} octets after connecting, {printable} of them printable",
        bytes.len()
    );
    let text: String = bytes
        .iter()
        .take(400)
        .map(|&c| if (32..127).contains(&c) || c == 10 || c == 13 { c as char } else { '.' })
        .collect();
    println!("{text}");
}
