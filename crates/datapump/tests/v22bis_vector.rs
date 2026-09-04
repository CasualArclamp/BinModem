//! End-to-end decode of a real V.22bis call.
//!
//! The vector is cut from a direct line capture of a ~2005 Conexant V.92
//! softmodem forced to V.22bis with `AT+MS`. Both directions are summed on the
//! one tap, as on a real 2-wire line.
//!
//! Unlike the Bell 103 call, this one runs error control: what the modems
//! exchange is not characters but V.42 frames, so the receiver has to be right
//! at the bit for a frame check sequence to hold. That makes this the strictest
//! ground truth the project has. The host is the same one the Bell 103 capture
//! reaches, which is what says the decode is genuine rather than a lucky
//! arrangement of noise.

use datapump::v22bis::{Channel, Rate, Receiver};
use ec::hdlc::{Decoder, Fcs};

const VECTOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/vectors/v22bis-2400.wav"
);

/// Every frame that passes its check sequence, in the given direction.
///
/// `Channel::Calling` means listening as the calling modem does, to the
/// answering modem on the high carrier, which carries the host's output.
fn frames(channel: Channel) -> Vec<Vec<u8>> {
    let wav = line::wav::read(VECTOR).expect("read V.22bis vector");
    assert_eq!(wav.sample_rate, 16_000, "vector should be 16 kHz");

    let mut rx = Receiver::new(channel, wav.sample_rate as f64);
    let mut decoder = Decoder::new(Fcs::Bits16);
    let mut out = Vec::new();
    for s in wav.mono() {
        rx.feed(s as f64);
        for bit in rx.take_bits() {
            if let Some(Ok(frame)) = decoder.feed(bit) {
                out.push(frame);
            }
        }
    }
    out
}

#[test]
fn the_call_carries_v42_frames() {
    let frames = frames(Channel::Calling);
    assert!(
        frames.len() >= 4,
        "only {} frames survived their check sequence",
        frames.len()
    );
    // Every frame begins with an address and a control field.
    for f in &frames {
        assert!(f.len() >= 2, "runt frame {f:02x?}");
        assert!(
            f[0] == 0x01 || f[0] == 0x03,
            "address {:02x} is neither of the two V.42 uses",
            f[0]
        );
    }
}

#[test]
fn the_exchange_opens_with_an_xid_offering_v42() {
    // V.42 A.2: the XID frame carries a parameter group identified by the
    // three characters V42, which is how each modem says what it can do.
    let frames = frames(Channel::Calling);
    let xid = frames
        .iter()
        .find(|f| f[1] == 0xaf)
        .expect("no XID frame recovered");
    assert!(
        xid.windows(3).any(|w| w == b"V42"),
        "XID frame carries no V42 identifier: {xid:02x?}"
    );
}

#[test]
fn the_host_greeting_comes_through_intact() {
    // The same host the Bell 103 capture reaches, so this is checkable against
    // something outside the modem.
    let frames = frames(Channel::Calling);
    let text: String = frames
        .iter()
        .flat_map(|f| f[2..].iter())
        .map(|&b| b as char)
        .collect();
    for expected in ["Welcome to phl6-dial1.popsite.net", "login:"] {
        assert!(
            text.contains(expected),
            "missing {expected:?}; recovered:\n{text:?}"
        );
    }
}

#[test]
fn the_rate_in_use_is_reported_as_1200() {
    // Despite the file's name, both modems settle on 1200 bit/s: the
    // constellation is four points on one ring, not sixteen on three.
    let wav = line::wav::read(VECTOR).expect("read V.22bis vector");
    let fs = wav.sample_rate as f64;
    let mut rx = Receiver::new(Channel::Calling, fs);
    // Ask while the call is up. The capture runs on past the hangup, and with
    // no carrier there is nothing for the question to mean.
    for (i, s) in wav.mono().into_iter().enumerate() {
        rx.feed(s as f64);
        if i == (12.0 * fs) as usize {
            assert_eq!(rx.rate(), Rate::Bps1200, "twelve seconds in");
        }
    }
}

/// Almost nothing on this call is decoded wrongly.
///
/// Counting frames that fail their check sequence is the strictest measure
/// available, since a single wrong bit anywhere in a frame fails it, and the
/// answer for both directions is one.
///
/// Counting everything the deframer rejects would give a much worse-looking
/// number and a wrong one. Most of what it rejects on the low channel is the
/// seven contiguous ones that mean an abort, which is also what an idle line
/// carries: the caller of this call typed nothing after logging in, so the low
/// channel is idle for most of its length and being told so is correct.
#[test]
fn the_frames_that_arrive_are_not_corrupted() {
    let wav = line::wav::read(VECTOR).expect("read V.22bis vector");
    let fs = wav.sample_rate as f64;
    for channel in [Channel::Calling, Channel::Answering] {
        let mut rx = Receiver::new(channel, fs);
        let mut decoder = Decoder::new(Fcs::Bits16);
        let (mut good, mut failed) = (0usize, 0usize);
        for s in wav.mono() {
            rx.feed(s as f64);
            for bit in rx.take_bits() {
                match decoder.feed(bit) {
                    Some(Ok(_)) => good += 1,
                    Some(Err(ec::hdlc::FrameError::BadFcs)) => failed += 1,
                    _ => {}
                }
            }
        }
        assert!(good >= 4, "{channel:?} recovered only {good} frames");
        assert!(
            failed <= 1,
            "{channel:?} had {failed} frames fail their check sequence"
        );
    }
}
