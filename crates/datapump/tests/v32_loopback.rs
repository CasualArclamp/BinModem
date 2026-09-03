//! V.32 at 4800 bit/s, transmitter into receiver.
//!
//! The last test is the one that matters, and is the reason V.32 needed an
//! echo canceller before it needed anything else: with both directions in the
//! one band there is no filter that can separate the far end from this end's
//! own reflection, and without cancelling it nothing gets through at all.

use datapump::v32::{BAUD, Mode, Receiver, Transmitter};
use dsp::EchoCanceller;

const FS: f64 = 16_000.0;

/// Send `payload` from one end to the other and return what came out.
fn loopback(payload: &[u8], lead_in: usize, mode: Mode) -> Vec<u8> {
    let mut tx = Transmitter::new(mode, FS);
    let mut rx = Receiver::new(mode.peer(), FS);

    tx.push_bytes(&vec![0x55; lead_in]);
    tx.push_bytes(payload);
    tx.push_bytes(&[0x55; 64]);

    let symbols = (lead_in + payload.len() + 64) * 4;
    let samples = (symbols as f64 * FS / BAUD).ceil() as usize;
    let mut out = Vec::new();
    for _ in 0..samples {
        rx.feed(tx.next_sample());
        out.extend(rx.take_bytes());
    }
    out
}

/// Find `needle` at any bit offset, since the receiver has no way to know
/// where the far end considered a byte to begin.
fn contains_at_any_bit_offset(haystack: &[u8], needle: &[u8]) -> bool {
    let bits: Vec<bool> = haystack
        .iter()
        .flat_map(|b| (0..8).rev().map(move |i| b & (1 << i) != 0))
        .collect();
    let want: Vec<bool> = needle
        .iter()
        .flat_map(|b| (0..8).rev().map(move |i| b & (1 << i) != 0))
        .collect();
    bits.windows(want.len()).any(|w| w == want.as_slice())
}

#[test]
fn a_transmitter_and_receiver_agree() {
    let payload = b"V.32 carries this at 4800 bits per second.";
    let got = loopback(payload, 128, Mode::Call);
    assert!(
        contains_at_any_bit_offset(&got, payload),
        "payload not recovered; got {} bytes",
        got.len()
    );
}

#[test]
fn the_answering_direction_works_the_same_way() {
    // Only the scrambler differs: same carrier, same band, same everything
    // else, which is exactly what makes the echo a problem.
    let payload = b"and this in the other direction";
    let got = loopback(payload, 128, Mode::Answer);
    assert!(contains_at_any_bit_offset(&got, payload));
}

#[test]
fn a_long_transfer_stays_locked() {
    let payload: Vec<u8> = (0..800).map(|i| (i % 251) as u8).collect();
    let got = loopback(&payload, 128, Mode::Call);
    assert!(
        contains_at_any_bit_offset(&got, &payload),
        "a long transfer drifted out of lock"
    );
}

#[test]
fn a_constant_payload_survives_the_scrambler() {
    let payload = vec![0x00u8; 300];
    let got = loopback(&payload, 128, Mode::Call);
    assert!(contains_at_any_bit_offset(&got, &payload));
}

#[test]
fn all_ones_survive_too() {
    let payload = vec![0xffu8; 300];
    let got = loopback(&payload, 128, Mode::Call);
    assert!(contains_at_any_bit_offset(&got, &payload));
}

#[test]
fn the_signal_may_arrive_at_any_moment() {
    // Timing has to be acquired rather than assumed, and 6.7 samples per
    // symbol means a whole symbol of arrival phase is covered in seven steps.
    let payload = b"acquired from a standing start";
    for quiet in 0..8 {
        let mut tx = Transmitter::new(Mode::Call, FS);
        let mut rx = Receiver::new(Mode::Answer, FS);
        tx.push_bytes(&[0x55; 128]);
        tx.push_bytes(payload);
        tx.push_bytes(&[0x55; 64]);
        let mut out = Vec::new();
        for _ in 0..quiet {
            rx.feed(0.0);
        }
        let samples = ((128 + payload.len() + 64) * 4) as f64 * FS / BAUD;
        for _ in 0..samples as usize {
            rx.feed(tx.next_sample());
            out.extend(rx.take_bytes());
        }
        assert!(
            contains_at_any_bit_offset(&out, payload),
            "not acquired when the carrier started {quiet} samples in"
        );
    }
}

#[test]
fn the_spectrum_sits_where_the_recommendation_puts_it() {
    // 2.2: with continuous ones into the scrambler, 600 Hz and 3000 Hz should
    // be 4.5 dB down on the maximum, give or take 2.5. Those are the carrier
    // plus and minus half the symbol rate.
    let mut tx = Transmitter::new(Mode::Call, FS);
    let samples: Vec<f64> = (0..(FS as usize)).map(|_| tx.next_sample()).collect();
    let power_at = |f: f64| {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &s) in samples.iter().enumerate() {
            let w = std::f64::consts::TAU * f * n as f64 / FS;
            re += s * w.cos();
            im -= s * w.sin();
        }
        (re * re + im * im) / (samples.len() * samples.len()) as f64
    };
    // Average over a few bins each side, since the signal is noise-like and a
    // single bin of it is a lottery.
    let around = |centre: f64| {
        let mut total = 0.0;
        let mut n = 0;
        let mut f = centre - 100.0;
        while f <= centre + 100.0 {
            total += power_at(f);
            n += 1;
            f += 10.0;
        }
        total / n as f64
    };
    let peak = around(1800.0);
    for edge in [600.0, 3000.0] {
        let db = 10.0 * (around(edge) / peak).log10();
        assert!(
            (-7.0..=-2.0).contains(&db),
            "{edge} Hz is {db:.1} dB down, outside the 4.5 plus or minus 2.5 asked for"
        );
    }
}

#[test]
fn without_cancellation_our_own_echo_drowns_the_far_end() {
    // The premise, stated as a test so that the next one means something. Both
    // directions share the band, so the receiver hears the sum and no filter
    // it could apply would tell the two apart.
    let (got, _) = through_a_hybrid(false);
    let payload = b"this will not get through";
    assert!(
        !contains_at_any_bit_offset(&got, payload),
        "the echo turned out not to matter, which would make the canceller pointless"
    );
}

#[test]
fn with_cancellation_the_far_end_comes_through() {
    let (got, loss) = through_a_hybrid(true);
    let payload = b"this will not get through";
    assert!(
        contains_at_any_bit_offset(&got, payload),
        "the canceller did not clear the echo; it reports {loss:.1} dB"
    );
    assert!(loss > 20.0, "only {loss:.1} dB of echo return loss");
}

/// One end of a call on a two-wire line, listening through its own echo.
///
/// The far end arrives 20 dB down after crossing the network; our own signal
/// comes back off the hybrid 12 dB down, which makes it the louder of the two
/// by eight decibels.
///
/// The call is run in two parts, as a real one is. First this end transmits
/// alone while the far end is silent, which is what the start-up procedure
/// provides for and is the only time the echo can be measured on its own: with
/// both ends talking, everything unexplained looks like echo the canceller has
/// not learned yet, and adapting on it drags the taps off the answer. Then the
/// far end speaks, with the taps held still.
///
/// Returns what this end recovered and how much echo was being removed,
/// measured at the end of training where the figure means something.
fn through_a_hybrid(cancel: bool) -> (Vec<u8>, f64) {
    const ECHO: f64 = 0.251;
    const FAR: f64 = 0.1;
    let payload = b"this will not get through";

    // This end, talking throughout.
    let mut near = Transmitter::new(Mode::Call, FS);
    near.push_bytes(&vec![0xa3; 8192]);

    let mut rx = Receiver::new(Mode::Call, FS);
    // Long enough for a near reflection and a little of the tail behind it.
    let mut ec = EchoCanceller::new(64, 0.5);

    // Training: the far end is silent and the canceller learns the path.
    for _ in 0..(FS as usize) {
        let sent = near.next_sample();
        let heard = sent * ECHO;
        if cancel {
            ec.process(sent, heard);
        }
    }
    let loss = ec.echo_return_loss();
    ec.set_adapting(false);

    // Data: the far end sends the payload and this end keeps talking over it.
    let mut far = Transmitter::new(Mode::Answer, FS);
    far.push_bytes(&[0x55; 128]);
    far.push_bytes(payload);
    far.push_bytes(&[0x55; 128]);

    let samples = ((128 + payload.len() + 128) * 4) as f64 * FS / BAUD;
    let mut out = Vec::new();
    for _ in 0..samples as usize {
        let sent = near.next_sample();
        let heard = far.next_sample() * FAR + sent * ECHO;
        let left = if cancel { ec.process(sent, heard) } else { heard };
        rx.feed(left);
        out.extend(rx.take_bytes());
    }
    (out, loss)
}
