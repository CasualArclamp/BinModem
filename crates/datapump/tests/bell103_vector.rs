//! End-to-end decode of a real Bell 103 call.
//!
//! The vector is cut from a direct line capture of a ~2005 Conexant V.92
//! softmodem forced to Bell 103 with `AT+MS`. Both directions are summed on the
//! one tap, as on a real 2-wire line, so the receiver has to pull each
//! direction out of the other's presence rather than working on a clean signal.
//!
//! This is the project's ground truth: the plaintext of this call is known.

use datapump::Bell103Rx;
use datapump::bell103::Role;

const VECTOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/vectors/bell103-300.wav"
);

/// Decode one direction of the call and return it as text.
fn decode(role: Role) -> String {
    let wav = line::wav::read(VECTOR).expect("read Bell 103 vector");
    assert_eq!(wav.sample_rate, 16_000, "vector should be 16 kHz");

    let mut rx = Bell103Rx::new(role, wav.sample_rate as f64);
    let mut out = Vec::new();
    for s in wav.mono() {
        if let Some(byte) = rx.feed(s as f64) {
            out.push(byte);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn answer_band_carries_the_host_session() {
    // The originating modem listens to the answering modem: the host's output.
    let text = decode(Role::Originate);
    println!("--- host -> caller ---\n{text}\n");

    for expected in [
        "Welcome to phl6-dial1.popsite.net",
        "login:",
        "Password:",
        "% Authentication failed",
    ] {
        assert!(
            text.contains(expected),
            "answer band missing {expected:?}; decoded:\n{text}"
        );
    }
}

#[test]
fn originate_band_carries_what_the_caller_typed() {
    // The answering modem listens to the originating modem: the user's keystrokes.
    // The host never echoes the password, so both CACTUS strings live here and
    // only the first is echoed back in the answer band.
    let text = decode(Role::Answer);
    println!("--- caller -> host ---\n{text}\n");

    assert_eq!(
        text.matches("CACTUS").count(),
        2,
        "expected the username and the password; decoded:\n{text}"
    );
}

#[test]
fn password_is_not_echoed_by_the_host() {
    let host = decode(Role::Originate);
    let caller = decode(Role::Answer);
    assert_eq!(caller.matches("CACTUS").count(), 2);
    assert_eq!(
        host.matches("CACTUS").count(),
        1,
        "the host should echo the username but never the password"
    );
}
