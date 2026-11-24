//! What each end of a V.32 call actually put on the line, and when.
//!
//! Ignored, because it needs a capture and captures are not in the repository.
//!
//! ```text
//! V32_CAPTURE=dist/captures/live-1788836496.wav cargo test -p datapump \
//!     --test v32_both_ends -- --ignored --nocapture
//! ```
//!
//! A live recording keeps the two directions apart -- channel 0 is what
//! arrived and channel 1 is what was sent at the same instant -- so both can
//! be read without having to separate them. Each channel gets a `Listener`,
//! which says what shape the spectrum is in, and a `Receiver` set to
//! descramble that direction, whose bits go to a `RateDetector`.
//!
//! The question it exists to answer is the one a waterfall cannot: whether the
//! rate sequences crossed, and what they said.

use datapump::v32::startup::{Heard, Listener, RateDetector, is_end_signal, is_rate_signal, offered_rate};
use datapump::v32::{Mode, Receiver};

const FS: f64 = 16_000.0;
/// How often the spectrum shape is reported.
const WINDOW: usize = 800; // 50 ms

fn describe(s: u16) -> String {
    let bit = |b: u32| s & (1 << (15 - b)) != 0;
    let kind = if is_end_signal(s) {
        "E "
    } else if is_rate_signal(s) {
        "R "
    } else {
        "? "
    };
    let mut modes = Vec::new();
    if bit(4) {
        modes.push("2400");
    }
    if bit(5) {
        modes.push("4800");
    }
    if bit(6) {
        modes.push("9600");
    }
    if modes.is_empty() {
        modes.push("cleardown");
    }
    format!(
        "{kind}{s:016b}  rates {:<16} trellis {}  highest {}",
        modes.join("/"),
        if bit(8) { "yes" } else { "no" },
        offered_rate(s)
    )
}

struct End {
    name: &'static str,
    listener: Listener,
    receiver: Receiver,
    rates: RateDetector,
    heard: Heard,
    since: usize,
}

impl End {
    /// `mode` is the end being *listened to as if from*: a `Receiver` built
    /// for Call descrambles what an answering modem sent, and the reverse.
    fn new(name: &'static str, mode: Mode) -> Self {
        Self {
            name,
            listener: Listener::new(FS),
            receiver: Receiver::new(mode, FS),
            rates: RateDetector::new(),
            heard: Heard::Nothing,
            since: 0,
        }
    }
}

#[test]
#[ignore = "needs a capture"]
fn both_ends_of_a_call() {
    let path = std::env::var("V32_CAPTURE").expect("set V32_CAPTURE");
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "built for 16 kHz");
    assert!(wav.channels >= 2, "needs a two-channel live recording");

    let arrived = wav.channel(0);
    let sent = wav.channel(1);
    println!(
        "\n{path}: {:.1} s, channel 0 arrived, channel 1 sent\n",
        wav.duration_secs()
    );

    // Channel 0 carries what the answering modem sent, so it is read by a
    // receiver belonging to the calling end. Channel 1 is the reverse.
    let mut ends = [
        End::new("far ", Mode::Call),
        End::new("near", Mode::Answer),
    ];

    for i in 0..arrived.len().min(sent.len()) {
        for (end, x) in ends.iter_mut().zip([arrived[i], sent[i]]) {
            let x = f64::from(x);
            end.listener.feed(x);
            end.receiver.feed(x);
            for bit in end.receiver.take_bits() {
                if let Some(word) = end.rates.feed(bit) {
                    println!(
                        "{:8.3}s  {}  {}",
                        i as f64 / FS,
                        end.name,
                        describe(word)
                    );
                }
            }
            if i % WINDOW == 0 {
                let now = end.listener.classify();
                if now != end.heard {
                    if end.since > 0 {
                        println!(
                            "{:8.3}s  {}  {:?} for {:.2} s",
                            end.since as f64 / FS,
                            end.name,
                            end.heard,
                            (i - end.since) as f64 / FS
                        );
                    }
                    end.heard = now;
                    end.since = i;
                }
            }
        }
    }
}
