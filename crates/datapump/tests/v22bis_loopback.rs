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
    loopback_with(payload, lead_in, channel, 0, FS)
}

/// As `loopback`, but the signal arrives `quiet` samples in, and the receiver
/// believes the line runs at `rx_fs`.
///
/// Both are things a real call decides for us. Nothing says a carrier will
/// start on a sample boundary convenient to the receiver, and two modems keep
/// their own clocks.
fn loopback_with(
    payload: &[u8],
    lead_in: usize,
    channel: Channel,
    quiet: usize,
    rx_fs: f64,
) -> Vec<u8> {
    let mut tx = Transmitter::new(channel, FS);
    let peer = match channel {
        Channel::Calling => Channel::Answering,
        Channel::Answering => Channel::Calling,
    };
    let mut rx = Receiver::new(peer, rx_fs);

    let mut out = Vec::new();
    for _ in 0..quiet {
        rx.feed(0.0);
        out.extend(rx.take_bytes());
    }

    tx.push_bytes(&vec![0x55; lead_in]);
    tx.push_bytes(payload);
    // Trailing filler so the last payload symbols clear the filters.
    tx.push_bytes(&[0x55; 32]);

    let symbols = (lead_in + payload.len() + 32) * 2;
    let samples = (symbols as f64 * FS / BAUD).ceil() as usize;
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
    let power =
        |tx: &mut Transmitter| (0..n).map(|_| tx.next_sample().powi(2)).sum::<f64>() / n as f64;
    let (a, b) = (power(&mut slow), power(&mut fast));
    assert!(
        (a / b - 1.0).abs() < 0.05,
        "1200 carries {a:.5} and 2400 carries {b:.5}"
    );
}

#[test]
fn the_signal_may_arrive_at_any_moment() {
    // Symbol timing has to be *acquired*, not assumed. Delaying the carrier by
    // a fraction of a symbol moves the instant the receiver is looking for, and
    // twenty-six samples covers a whole symbol at 600 baud on a 16 kHz line.
    //
    // This is the test an earlier receiver would have failed. Its timing loop
    // could only creep, so it sampled wherever the group delay of the filters
    // ahead of it happened to leave it, and whether that worked was decided by
    // how long those filters were rather than by anything it did.
    let payload = b"acquired from a standing start";
    for quiet in 0..27 {
        let got = loopback_with(payload, 96, Channel::Calling, quiet, FS);
        assert!(
            contains_at_any_bit_offset(&got, payload),
            "not acquired when the carrier started {quiet} samples in"
        );
    }
}

#[test]
fn the_two_clocks_need_not_agree() {
    // V.22bis 2.6 allows the carrier, and with it the symbol clock, to be out
    // by a hundred parts per million. Twice that is asked for here, so the
    // requirement is met with room to spare rather than exactly.
    //
    // Measured, the receiver holds from -300 to +400 ppm, and the limit does
    // not move with the length of the transfer: past it acquisition costs one
    // slipped symbol and everything after is offset, rather than the clocks
    // slowly drifting apart. The asymmetry is unexplained.
    let payload: Vec<u8> = (0..400).map(|i| (i % 251) as u8).collect();
    for ppm in [-200.0, -100.0, 100.0, 200.0] {
        let got = loopback_with(&payload, 96, Channel::Calling, 0, FS * (1.0 + ppm / 1e6));
        assert!(
            contains_at_any_bit_offset(&got, &payload),
            "lost the payload with the clocks {ppm} ppm apart"
        );
    }
}

/// Run `payload` through while our own channel comes back on top of it.
///
/// One virtual cable is not a hybrid. What is written to it is what comes back,
/// so a modem on one hears its own transmit at full strength, in the channel it
/// is not listening to, from the moment it starts sending -- which is well
/// before the far end has said anything at all.
///
/// `quiet_ms` is how long that goes on for before the far end speaks, and it is
/// the variable that matters: every real call has a pause of some length there,
/// for the answer tone and the handshake, and no two calls have the same one.
fn with_own_channel(payload: &[u8], lead_in: usize, own_level: f64, quiet_ms: f64) -> Vec<u8> {
    let mut far = Transmitter::new(Channel::Answering, FS);
    let mut own = Transmitter::new(Channel::Calling, FS);
    // A calling modem: listens on the high channel, transmits on the low one.
    let mut rx = Receiver::new(Channel::Calling, FS);
    own.push_bytes(&vec![0x5a; 8192]);

    let mut out = Vec::new();
    for _ in 0..(FS * quiet_ms / 1000.0) as usize {
        rx.feed(own.next_sample() * own_level);
        out.extend(rx.take_bytes());
    }

    far.push_bytes(&vec![0x55; lead_in]);
    far.push_bytes(payload);
    far.push_bytes(&[0x55; 32]);
    let symbols = (lead_in + payload.len() + 32) * 2;
    for _ in 0..(symbols as f64 * FS / BAUD).ceil() as usize {
        rx.feed(far.next_sample() + own.next_sample() * own_level);
        out.extend(rx.take_bytes());
    }
    out
}

/// How often the far end is heard, over many different pauses before it starts.
///
/// One pause is not a measurement. Its length decides where in the symbol the
/// carrier appears and how far gain control has run down, and a single value
/// answers for that one alignment and no other. These are spaced so that no two
/// land at the same point of a symbol.
fn acquisitions(own_level: f64) -> (usize, usize) {
    let payload = b"the far end is saying this while we talk over it";
    let quiets = (0..16).map(|i| 131.0 * f64::from(i) + 0.37 * f64::from(i));
    let mut heard = 0;
    let mut tried = 0;
    for quiet_ms in quiets {
        tried += 1;
        if contains_at_any_bit_offset(&with_own_channel(payload, 96, own_level, quiet_ms), payload) {
            heard += 1;
        }
    }
    (heard, tried)
}

#[test]
fn a_carrier_that_arrives_after_a_pause_is_still_acquired() {
    // Every real call has a pause before the far end's carrier: the answer
    // tone, the silence after it, the handshake. This is where V.22bis was
    // falling over, and the cause was a guard that had already expired.
    //
    // The equaliser is held still for its first sixty-four symbols to keep it
    // away from the acquisition transient -- gain control pinned to its clamp
    // by silence, a carrier loop that has not yet found the phase. Those
    // symbols used to be counted from when the receiver was built rather than
    // from when a carrier appeared, so after any pause longer than about a
    // tenth of a second the guard was long gone, and the equaliser adapted
    // straight into the transient it exists to avoid. It then spent the call
    // unlearning it.
    //
    // Below a tenth of a second it worked, which is why every loopback test
    // here passed: they all start the signal at once.
    let (heard, tried) = acquisitions(0.0);
    assert!(
        heard >= tried - 2,
        "heard the far end after only {heard} of {tried} pauses"
    );
}

#[test]
fn our_own_channel_does_not_stop_us_hearing_the_other() {
    // The same, with our own transmit coming back at the strength one cable
    // returns it at, which is all of it. Band selection has to hold that out
    // of the loops for the whole of the pause as well as during the call:
    // there is no hybrid here to help it.
    let (heard, tried) = acquisitions(1.0);
    assert!(
        heard >= tried - 2,
        "our own channel cost us the far end in {} of {tried} pauses",
        tried - heard
    );
}

#[test]
fn the_channel_we_transmit_in_is_not_heard_as_a_carrier() {
    // Nothing this modem sends should ever look like an incoming call. On one
    // cable it all comes straight back, so the only thing separating the two
    // is the selectivity of the band filter.
    let mut own = Transmitter::new(Channel::Calling, FS);
    let mut rx = Receiver::new(Channel::Calling, FS);
    own.push_bytes(&vec![0x5a; 4096]);

    let mut sent = 0.0f64;
    let n = (FS * 0.5) as usize;
    for _ in 0..n {
        let s = own.next_sample();
        sent += s * s;
        rx.feed(s);
    }
    let sent = (sent / n as f64).sqrt();
    assert!(!rx.carrier(), "heard its own transmit as an incoming carrier");
    // Where the two channels sit, 55 dB is what the filter is designed for and
    // 60 is what it should comfortably beat end to end.
    let rejection = 20.0 * (sent / rx.level().max(1e-12)).log10();
    assert!(
        rejection > 60.0,
        "our own channel is only {rejection:.1} dB down after band selection"
    );
}
