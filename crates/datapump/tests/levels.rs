//! Every transmitter leaves the modem at the same power.
//!
//! Not a property any Recommendation states, because each of them states its
//! own level in dBm at the line and says nothing about the others. It matters
//! here because one call uses several of these in turn: a fax call alternates
//! 300 bit/s frames, two single tones and a V.27 ter carrier, turning the line
//! around between each, and the machine at the far end has one gain control
//! for all of them.
//!
//! A level set nine decibels low is invisible in a loopback, where the far end
//! is exactly as loud as this end wrote it, and it costs a page on a real
//! line.
//!
//! The figure is a root mean square, not a peak. V.2 states a transmit level
//! as a power, and a shaped constellation and a constant-envelope tone with
//! the same power do not have the same peak: the peaks below run from 1.0 for
//! a tone to nearly 2 for the crowded V.32 constellations, which is what the
//! headroom in the drive setting is for.

use datapump::{v21, v27ter, v32};

const FS: f64 = 16_000.0;

/// What every transmitter here should come out at.
const WANT_RMS: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// How far off is allowed, as a fraction. Half a decibel.
const TOLERANCE: f64 = 0.06;

fn rms(x: &[f64]) -> f64 {
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

fn check(what: &str, samples: &[f64]) {
    let got = rms(samples);
    let peak = samples.iter().fold(0.0f64, |a, b| a.max(b.abs()));
    let decibels = 20.0 * (got / WANT_RMS).log10();
    assert!(
        (got - WANT_RMS).abs() <= TOLERANCE * WANT_RMS,
        "{what} leaves at {got:.4} rms, {decibels:+.1} dB from the {WANT_RMS:.4} \
         everything else does (peak {peak:.3})"
    );
    assert!(peak < 2.1, "{what} peaks at {peak:.3}, which will clip");
}

#[test]
fn the_three_hundred_bit_per_second_channel() {
    let mut tx = v21::Sender::new(FS);
    tx.set_transmitting(true);
    let mut out = Vec::new();
    for i in 0..(FS * 1.0) as usize {
        if tx.pending_bits() < 8 {
            let byte = i as u8 ^ 0x5a;
            let bits: Vec<bool> = (0..8).map(|b| byte >> b & 1 == 1).collect();
            tx.push_bits(&bits);
        }
        out.push(tx.next_sample());
    }
    check("V.21 channel 2", &out);
}

#[test]
fn the_two_tones_a_fax_call_starts_with() {
    for (name, hz) in [("the calling tone", v21::CNG), ("the called tone", v21::CED)] {
        let mut tone = v21::Tone::new(hz, FS);
        let out: Vec<f64> = (0..(FS * 0.5) as usize).map(|_| tone.next_sample()).collect();
        check(name, &out);
    }
}

#[test]
fn the_page_carrier() {
    for rate in [v27ter::Rate::R4800, v27ter::Rate::R2400] {
        let mut tx = v27ter::Transmitter::new(FS);
        tx.start(rate, v27ter::Training::Long);
        // Past the training, so this is data and not two-phase conditioning.
        let mut out = Vec::new();
        for i in 0..(FS * 2.0) as usize {
            if tx.pending_bits() < 32 {
                tx.push_bytes(&[i as u8, 0x5a, 0xc3]);
            }
            let sample = tx.next_sample();
            if tx.trained() {
                out.push(sample);
            }
        }
        check(&format!("V.27 ter at {}", rate.bits_per_second()), &out);
    }
}

#[test]
fn the_data_carriers() {
    for rate in [4800u32, 9600, 14_400] {
        let mut tx = v32::Transmitter::new(v32::Mode::Call, FS);
        tx.set_data_rate(rate);
        tx.set_signal(v32::Signal::ScrambledOnes);
        let out: Vec<f64> = (0..(FS * 1.0) as usize).map(|_| tx.next_sample()).collect();
        check(&format!("V.32 at {rate}"), &out);
    }
}
