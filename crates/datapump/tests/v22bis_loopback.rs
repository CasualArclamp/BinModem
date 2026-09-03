//! V.22bis transmitter into V.22bis receiver.
//!
//! The receiver has to acquire symbol timing, lock a carrier and synchronise a
//! self-synchronising descrambler before anything readable emerges, so every
//! test sends a lead-in first and looks for the payload in what follows.

use datapump::v22bis::{BAUD, Channel, Receiver, Transmitter};

const FS: f64 = 16_000.0;

/// Run `payload` through a transmitter and receiver, returning recovered bytes.
///
/// `lead_in` bytes of filler go first, giving the loops time to settle.
fn loopback(payload: &[u8], lead_in: usize, channel: Channel) -> Vec<u8> {
    let mut tx = Transmitter::new(channel, FS);
    let peer = match channel {
        Channel::Calling => Channel::Answering,
        Channel::Answering => Channel::Calling,
    };
    let mut rx = Receiver::new(peer, FS);

    tx.push_bytes(&vec![0x55; lead_in]);
    tx.push_bytes(payload);
    // Trailing filler so the last payload symbols clear the filters.
    tx.push_bytes(&[0x55; 32]);

    let symbols = (lead_in + payload.len() + 32) * 2;
    let samples = (symbols as f64 * FS / BAUD).ceil() as usize;
    let mut out = Vec::new();
    for _ in 0..samples {
        rx.feed(tx.next_sample());
        out.extend(rx.take_bytes());
    }
    out
}

/// Find `needle` in `haystack`, allowing for the unknown bit offset a receiver
/// starts at: the recovered stream may be shifted by any number of bits.
fn contains_at_any_bit_offset(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
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
    let payload = b"V.22bis carries this at 2400 bits per second.";
    let got = loopback(payload, 96, Channel::Calling);
    assert!(
        contains_at_any_bit_offset(&got, payload),
        "payload not recovered; got {} bytes: {:?}",
        got.len(),
        String::from_utf8_lossy(&got[got.len().saturating_sub(80)..])
    );
}

#[test]
fn the_answering_channel_works_the_same_way() {
    // The high channel differs only in carrier frequency.
    let payload = b"the high channel answers on 2400 Hz";
    let got = loopback(payload, 96, Channel::Answering);
    assert!(
        contains_at_any_bit_offset(&got, payload),
        "payload not recovered on the high channel"
    );
}

#[test]
fn a_long_transfer_stays_locked() {
    // Timing and carrier loops must hold, not merely acquire.
    let payload: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
    let got = loopback(&payload, 96, Channel::Calling);
    assert!(
        contains_at_any_bit_offset(&got, &payload),
        "a long transfer drifted out of lock"
    );
}

#[test]
fn a_constant_payload_survives_the_scrambler() {
    // Runs of identical bits are what the scrambler exists to break up, and
    // they are also what makes timing recovery hardest.
    let payload = vec![0x00u8; 200];
    let got = loopback(&payload, 96, Channel::Calling);
    assert!(
        contains_at_any_bit_offset(&got, &payload),
        "a constant payload was not recovered"
    );
}

#[test]
fn all_ones_survive_too() {
    let payload = vec![0xffu8; 200];
    let got = loopback(&payload, 96, Channel::Calling);
    assert!(contains_at_any_bit_offset(&got, &payload));
}

#[test]
fn the_transmitted_signal_sits_in_its_own_channel() {
    // The two directions share the line, so each must stay inside its band or
    // the frequency-division duplex that V.22bis relies on breaks down.
    let mut tx = Transmitter::new(Channel::Calling, FS);
    tx.push_bytes(&vec![0x5a; 400]);
    let n = 8192;
    let samples: Vec<f64> = (0..n).map(|_| tx.next_sample()).collect();

    let mut spectrum = dsp::Spectrum::new(4096, FS);
    for &s in &samples {
        spectrum.push(s);
    }
    let mut bins = vec![0.0f64; 2048];
    spectrum.magnitudes_db(&mut bins);

    let bin_at = |hz: f64| (hz / (FS / 4096.0)).round() as usize;
    let peak_between = |lo: f64, hi: f64| {
        (bin_at(lo)..=bin_at(hi))
            .map(|k| bins[k])
            .fold(f64::MIN, f64::max)
    };

    let own = peak_between(700.0, 1700.0);
    let other = peak_between(1900.0, 2900.0);
    assert!(
        own - other > 25.0,
        "low channel leaked into the high channel: {own:.1} dB against {other:.1} dB"
    );
}

#[test]
fn the_receiver_reports_a_settled_constellation() {
    let mut tx = Transmitter::new(Channel::Calling, FS);
    let mut rx = Receiver::new(Channel::Answering, FS);
    tx.push_bytes(&vec![0x6c; 400]);
    let samples = (800.0 * FS / BAUD) as usize;
    let mut errors = Vec::new();
    for i in 0..samples {
        rx.feed(tx.next_sample());
        if i % 27 == 0 {
            errors.push(rx.phase_error().abs());
        }
    }
    let settled: f64 = errors[errors.len() - 100..].iter().sum::<f64>() / 100.0;
    assert!(
        settled < 0.25,
        "carrier loop did not settle; residual error {settled}"
    );

    let (i, q) = rx.constellation_point();
    let magnitude = (i * i + q * q).sqrt();
    assert!(
        (0.2..=1.6).contains(&magnitude),
        "constellation point at {magnitude}, so gain control is off"
    );
}

// -- 1200 bit/s (V.22bis 2.5.2.2) --------------------------------------------

use datapump::v22bis::Rate;

fn loopback_at(payload: &[u8], lead_in: usize, rate: Rate) -> (Vec<u8>, Rate) {
    let mut tx = Transmitter::at_rate(Channel::Calling, rate, FS);
    let mut rx = Receiver::new(Channel::Answering, FS);
    tx.push_bytes(&vec![0x55; lead_in]);
    tx.push_bytes(payload);
    tx.push_bytes(&[0x55; 64]);

    // At 1200 a symbol carries half as much, so twice as many are needed.
    let bits = (lead_in + payload.len() + 64) * 8;
    let symbols = bits / rate.bits_per_symbol();
    let samples = (symbols as f64 * FS / BAUD).ceil() as usize;
    let mut out = Vec::new();
    for _ in 0..samples {
        rx.feed(tx.next_sample());
        out.extend(rx.take_bytes());
    }
    (out, rx.rate())
}

#[test]
fn twelve_hundred_bits_per_second_round_trips() {
    let payload = b"V.22 compatibility mode carries this at 1200.";
    let (got, _) = loopback_at(payload, 160, Rate::Bps1200);
    assert!(
        contains_at_any_bit_offset(&got, payload),
        "payload not recovered at 1200 bit/s"
    );
}

#[test]
fn the_receiver_works_out_which_rate_is_in_use() {
    // Decoding 1200 as though it were 2400 gives two real bits and two
    // meaningless ones, which descrambles into convincing noise rather than an
    // obvious failure. The constellation is what distinguishes them: four
    // clusters at one radius against sixteen at three.
    let (_, detected) = loopback_at(b"rate detection", 200, Rate::Bps1200);
    assert_eq!(detected, Rate::Bps1200, "should have fallen back to 1200");

    let (_, detected) = loopback_at(b"rate detection", 200, Rate::Bps2400);
    assert_eq!(detected, Rate::Bps2400, "should have stayed at 2400");
}

#[test]
fn the_two_rates_carry_the_same_average_power() {
    // V.22bis 2.5.2.2 picks the 01 point for 1200 precisely so this holds.
    let mut slow = Transmitter::at_rate(Channel::Calling, Rate::Bps1200, FS);
    let mut fast = Transmitter::at_rate(Channel::Calling, Rate::Bps2400, FS);
    slow.push_bytes(&vec![0x6b; 600]);
    fast.push_bytes(&vec![0x6b; 600]);
    let n = 40_000;
    let power = |tx: &mut Transmitter| {
        (0..n).map(|_| tx.next_sample().powi(2)).sum::<f64>() / n as f64
    };
    let (a, b) = (power(&mut slow), power(&mut fast));
    assert!(
        (a / b - 1.0).abs() < 0.05,
        "1200 carries {a:.5} and 2400 carries {b:.5}"
    );
}
