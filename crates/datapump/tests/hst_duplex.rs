//! HST end to end: both channels at once on one two-wire line.
//!
//! The answering end holds the forward channel and the calling end the back
//! channel, as a call starts (the notes' "first seconds of a call"). Each end
//! hears the far end through the line's loss, its own transmitter through its
//! hybrid, and noise. The forward channel's guard filters and the back
//! channel's band are what keep the two apart.

use std::f64::consts::TAU;

use datapump::hst::{BackRate, BackRx, BackTx, ForwardRx, ForwardTx, Rate};

/// A seeded generator: xorshift64* with Box-Muller.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gaussian(&mut self) -> f64 {
        let u1 = self.unit().max(1e-18);
        (-2.0 * u1.ln()).sqrt() * (TAU * self.unit()).cos()
    }
    fn bits(&mut self, n: usize) -> Vec<bool> {
        (0..n).map(|_| self.next() >> 63 == 1).collect()
    }
}

struct Line {
    /// Far end's signal as heard: the line's loss.
    loss: f64,
    /// Own transmitter as heard back through the hybrid.
    echo: f64,
    /// Noise, as a standard deviation on the line.
    noise: f64,
}

struct Outcome {
    forward_errors: Option<usize>,
    back_errors: Option<usize>,
    forward_snr: f64,
    back_snr: f64,
}

/// Fewest errors over any alignment of `want` within `got`.
fn best_errors(got: &[bool], want: &[bool]) -> Option<usize> {
    if got.len() < want.len() {
        return None;
    }
    (0..=got.len() - want.len())
        .map(|o| got[o..o + want.len()].iter().zip(want).filter(|(a, b)| a != b).count())
        .min()
}

fn call(fs: f64, rate: Rate, back: BackRate, line: &Line, seed: u64) -> Outcome {
    let mut rng = Rng(seed | 1);
    let mut answer_tx = ForwardTx::new(fs);
    let mut answer_rx = BackRx::with_rate(fs, back);
    let mut call_tx = BackTx::new(fs);
    let mut call_rx = ForwardRx::new(fs);
    call_rx.set_rate(rate);

    let forward_data = rng.bits(12_000);
    let back_data = rng.bits(600);
    answer_tx.start(rate);
    answer_tx.push_bits(&forward_data);
    call_tx.start(back);
    call_tx.push_bits(&back_data);

    let seconds = 1.6 + forward_data.len() as f64 / f64::from(rate.bits_per_second()) + 0.4;
    let seconds = seconds.max(0.6 + back_data.len() as f64 / f64::from(back.bits_per_second()));
    let (mut forward_got, mut back_got) = (Vec::new(), Vec::new());
    let (mut a_out, mut c_out) = (0.0, 0.0);
    for _ in 0..(seconds * fs) as usize {
        let at_caller = line.loss * a_out + line.echo * c_out + line.noise * rng.gaussian();
        let at_answerer = line.loss * c_out + line.echo * a_out + line.noise * rng.gaussian();
        call_rx.feed(at_caller);
        answer_rx.feed(at_answerer);
        forward_got.extend(call_rx.take_bits());
        back_got.extend(answer_rx.take_bits());
        a_out = answer_tx.next_sample();
        c_out = call_tx.next_sample();
    }
    Outcome {
        forward_errors: best_errors(&forward_got, &forward_data),
        back_errors: best_errors(&back_got, &back_data),
        forward_snr: call_rx.snr_db(),
        back_snr: answer_rx.snr_db(),
    }
}

fn check(fs: f64, rate: Rate, back: BackRate, line: &Line) {
    let out = call(fs, rate, back, line, 0x5EED);
    println!(
        "{fs} Hz {rate:?} + {back:?}: forward errors {:?} (snr {:.1} dB), back errors {:?} (snr {:.1} dB)",
        out.forward_errors, out.forward_snr, out.back_errors, out.back_snr
    );
    assert_eq!(out.forward_errors, Some(0), "forward channel");
    assert_eq!(out.back_errors, Some(0), "back channel");
}

/// A quiet line with a poor hybrid: our own transmitter comes back as loud
/// as the far end does.
const ECHOEY: Line = Line { loss: 0.3, echo: 0.3, noise: 0.003 };

#[test]
fn both_channels_carry_data_at_once_at_9600_and_450() {
    check(16_000.0, Rate::R9600, BackRate::B450, &ECHOEY);
}

#[test]
fn both_channels_carry_data_at_once_at_8000_hz() {
    check(8000.0, Rate::R9600, BackRate::B450, &ECHOEY);
}

#[test]
fn every_forward_rate_works_alongside_the_back_channel() {
    for rate in [Rate::R7200, Rate::R9600, Rate::R12000, Rate::R14400] {
        check(16_000.0, rate, BackRate::B450, &ECHOEY);
    }
}

#[test]
fn the_slower_back_channel_works_too() {
    check(16_000.0, Rate::R9600, BackRate::B300, &ECHOEY);
}

/// Noise 24 dB under the forward channel, which 9600 needs about 21 for, and
/// a hybrid that sends our own transmitter back twice as loud as the far end.
const ROUGH: Line = Line { loss: 0.3, echo: 0.6, noise: 0.012 };

#[test]
fn it_survives_noise_and_a_bad_hybrid_at_9600() {
    check(16_000.0, Rate::R9600, BackRate::B450, &ROUGH);
}

// ---------------------------------------------------------------------------
// Frames: HDLC both ways, as the Courier frames its error control.

use ec::hdlc::{Decoder, Encoder, Fcs};

/// Keeps a transmitter's queue topped up from a framer: the frames in turn,
/// then flags.
struct Framer {
    encoder: Encoder,
    frames: std::collections::VecDeque<Vec<u8>>,
}

impl Framer {
    fn new(frames: Vec<Vec<u8>>) -> Self {
        Self { encoder: Encoder::new(Fcs::Bits16), frames: frames.into() }
    }

    fn top_up(&mut self, pending: usize, push: &mut dyn FnMut(&[bool])) {
        if pending >= 64 {
            return;
        }
        let mut bits = Vec::new();
        while bits.len() < 64 {
            if self.encoder.is_empty() {
                match self.frames.pop_front() {
                    Some(f) => self.encoder.frame(&f),
                    None => self.encoder.idle(1),
                }
            }
            bits.extend(std::iter::from_fn(|| self.encoder.next_bit()).take(64 - bits.len()));
        }
        push(&bits);
    }
}

#[test]
fn hdlc_frames_cross_both_channels_at_once() {
    let fs = 16_000.0;
    let rate = Rate::R9600;
    let back = BackRate::B450;
    let line = ROUGH;
    let mut rng = Rng(0xF4A3E5 | 1);

    // Forward: data frames in the Courier's optimized layout, [4][seq] + 64
    // octets. Back: acknowledgements, [5][seq][credit].
    let forward: Vec<Vec<u8>> = (0..12u8)
        .map(|seq| {
            let mut f = vec![0x04, seq];
            f.extend((0..64).map(|_| (rng.next() >> 56) as u8));
            f
        })
        .collect();
    let backward: Vec<Vec<u8>> = (0..12u8).map(|seq| vec![0x05, seq, 8]).collect();

    let mut answer_tx = ForwardTx::new(fs);
    let mut answer_rx = BackRx::with_rate(fs, back);
    let mut call_tx = BackTx::new(fs);
    let mut call_rx = ForwardRx::new(fs);
    call_rx.set_rate(rate);
    answer_tx.start(rate);
    call_tx.start(back);

    let mut fwd = Framer::new(forward.clone());
    let mut bck = Framer::new(backward.clone());
    let mut fwd_dec = Decoder::new(Fcs::Bits16);
    let mut bck_dec = Decoder::new(Fcs::Bits16);
    let (mut fwd_got, mut bck_got) = (Vec::new(), Vec::new());
    let (mut a_out, mut c_out) = (0.0, 0.0);
    for _ in 0..(4.5 * fs) as usize {
        let at_caller = line.loss * a_out + line.echo * c_out + line.noise * rng.gaussian();
        let at_answerer = line.loss * c_out + line.echo * a_out + line.noise * rng.gaussian();
        call_rx.feed(at_caller);
        answer_rx.feed(at_answerer);
        for b in call_rx.take_bits() {
            if let Some(Ok(f)) = fwd_dec.feed(b) {
                fwd_got.push(f);
            }
        }
        for b in answer_rx.take_bits() {
            if let Some(Ok(f)) = bck_dec.feed(b) {
                bck_got.push(f);
            }
        }
        // Frames only once the channel is through its training, so none is
        // spent on a receiver that is not yet listening.
        if answer_tx.trained() {
            fwd.top_up(answer_tx.pending_bits(), &mut |b| answer_tx.push_bits(b));
        }
        if call_tx.trained() {
            bck.top_up(call_tx.pending_bits(), &mut |b| call_tx.push_bits(b));
        }
        a_out = answer_tx.next_sample();
        c_out = call_tx.next_sample();
    }
    println!("forward: {} of {} frames; back: {} of {}", fwd_got.len(), forward.len(), bck_got.len(), backward.len());
    assert_eq!(fwd_got, forward, "forward frames");
    assert_eq!(bck_got, backward, "back-channel frames");
}
