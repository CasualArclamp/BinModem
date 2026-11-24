//! V.27 ter's receiver against the line a fax call meets, one burst at a time.
//!
//! Everything here goes through the receiver's public interface only --
//! `new`, `set_rate`, `restart`, `feed`, `take_bits`, `carrier` -- which is all
//! `modem::faxcall` uses, so the same harness measured the receiver this one
//! replaced and measures this one (docs/design/slow-modes/fax-qam.md 7). The
//! far end is this crate's own transmitter, and the line does to it what a
//! real one does: a carrier offset, a far clock off ours, white noise, an
//! echo, and a VoIP jitter buffer's 20 ms of made-up audio.
//!
//! Signal to noise is Es/N0: white Gaussian noise at 16 kHz of variance
//! (fs/2)/baud x P x 10^(-SNR/10), P the burst's power as received. The
//! working figures are where a perfect coherent receiver with differential
//! decoding would make one bit error in ten thousand: 17 dB for eight phases
//! at 1600 baud and 12 dB for four at 1200.
//!
//! Each test prints what it measured; `cargo test --release -p datapump --test
//! v27ter_receiver -- --nocapture --test-threads 1` shows them in order.

use std::f64::consts::{PI, TAU};

use datapump::v27ter::{Rate, Receiver, Training, Transmitter};
use dsp::Resampler;
use dsp::qam::Via;

const FS: f64 = 16_000.0;

/// The transmitter's power: every one in this modem leaves at a root mean
/// square of 0.707.
const POWER: f64 = 0.5;

// ---------------------------------------------------------------------------
// Random numbers, seeded, so that a run can be looked at again.

#[derive(Debug, Clone)]
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2.0 * a.ln()).sqrt() * (TAU * b).cos()
    }
}

fn payload(seed: u64, bytes: usize) -> Vec<u8> {
    let mut random = Random::new(seed ^ 0x5eed);
    (0..bytes).map(|_| (random.next_u64() >> 56) as u8).collect()
}

fn bits_of(bytes: &[u8]) -> Vec<bool> {
    bytes.iter().flat_map(|&b| (0..8).rev().map(move |i| b >> i & 1 != 0)).collect()
}

// ---------------------------------------------------------------------------
// The far end and the line.

/// One burst as the far end puts it on the line: the turn-on sequence, the
/// data, the turn-off, and nothing after.
fn burst(rate: Rate, training: Training, echo: bool, data: &[bool]) -> Vec<f64> {
    let mut tx = Transmitter::new(FS);
    tx.set_echo_protection(echo);
    tx.start(rate, training);
    let mut out = Vec::new();
    let mut fed = 0;
    loop {
        // Fed a little at a time, as the fax layer feeds it.
        while tx.pending_bits() < 32 && fed < data.len() {
            tx.push_bits(&data[fed..(fed + 32).min(data.len())]);
            fed = (fed + 32).min(data.len());
        }
        if tx.trained() && tx.pending_bits() == 0 && fed == data.len() {
            tx.stop();
        }
        out.push(tx.next_sample());
        if !tx.is_transmitting() {
            break;
        }
        assert!(out.len() < (FS * 60.0) as usize, "the burst never ended");
    }
    out
}

/// A single-sideband frequency shift: the analytic signal by a Hilbert
/// transformer, turned, and its real part (fax-qam.md 7). The in-phase path
/// is delayed by exactly the transformer's centre, or the wrong sideband
/// comes out above the wanted one.
fn shifted(samples: &[f64], hz: f64) -> Vec<f64> {
    if hz == 0.0 {
        return samples.to_vec();
    }
    const N: usize = 255;
    let m = (N - 1) / 2;
    let taps: Vec<f64> = (0..N)
        .map(|d| {
            let k = d as isize - m as isize;
            if k % 2 == 0 {
                0.0
            } else {
                2.0 / (PI * k as f64) * (0.54 - 0.46 * (TAU * d as f64 / (N - 1) as f64).cos())
            }
        })
        .collect();
    let at = |n: isize| if n >= 0 && (n as usize) < samples.len() { samples[n as usize] } else { 0.0 };
    (0..samples.len() + m)
        .map(|n| {
            let n = n as isize;
            let q: f64 = taps.iter().enumerate().filter(|(_, t)| **t != 0.0).map(|(d, t)| t * at(n - d as isize)).sum();
            let i = at(n - m as isize);
            let turn = TAU * hz * n as f64 / FS;
            i * turn.cos() - q * turn.sin()
        })
        .collect()
}

/// The far end's samples on our clock, its oscillator `ppm` fast.
fn clocked(samples: &[f64], ppm: f64) -> Vec<f64> {
    if ppm == 0.0 {
        return samples.to_vec();
    }
    let mut resampler = Resampler::new(FS * (1.0 + ppm / 1e6), FS);
    let mut out = Vec::with_capacity(samples.len() + 64);
    for &x in samples {
        resampler.process(x, &mut out);
    }
    out
}

/// The signal and a copy of it `gain` as loud and `ms` later: a talker echo
/// or a reflection, which is what an equaliser is there for.
fn echoed(samples: &[f64], gain: f64, ms: f64) -> Vec<f64> {
    let delay = (ms * FS / 1000.0).round() as usize;
    (0..samples.len() + delay)
        .map(|n| {
            let direct = samples.get(n).copied().unwrap_or(0.0);
            let late = n.checked_sub(delay).and_then(|k| samples.get(k)).copied().unwrap_or(0.0);
            direct + gain * late
        })
        .collect()
}

/// A jitter buffer's 20 ms: `inserted`, the 20 ms before played again and
/// faded in and out over 40 samples, so that everything after arrives 20 ms
/// late; or the 20 ms from `at` dropped (`v34/receiver.rs:1572-1587`). What
/// is played again from before the signal began is the silence there was.
fn slipped(samples: &[f64], at: usize, inserted: bool) -> Vec<f64> {
    let n = (0.020 * FS) as usize;
    let mut out = samples[..at].to_vec();
    if inserted {
        let fade = 40;
        let mut before = vec![0.0; n.saturating_sub(at)];
        before.extend_from_slice(&samples[at.saturating_sub(n)..at]);
        out.extend(before.iter().enumerate().map(|(i, x)| {
            let edge = i.min(n - 1 - i);
            if edge < fade { x * edge as f64 / fade as f64 } else { *x }
        }));
        out.extend_from_slice(&samples[at..]);
    } else {
        out.extend_from_slice(&samples[at + n..]);
    }
    out
}

/// White Gaussian noise for a burst of `rate` at `snr_db`, Es/N0.
#[derive(Debug, Clone)]
struct Noise {
    random: Random,
    sigma: f64,
}

impl Noise {
    fn new(seed: u64, rate: Rate, snr_db: f64, power: f64) -> Self {
        let variance = (FS / 2.0) / rate.baud() * power * 10f64.powf(-snr_db / 10.0);
        Self { random: Random::new(seed), sigma: variance.sqrt() }
    }

    fn none() -> Self {
        Self { random: Random::new(1), sigma: 0.0 }
    }

    fn sample(&mut self) -> f64 {
        if self.sigma == 0.0 { 0.0 } else { self.sigma * self.random.gaussian() }
    }
}

// ---------------------------------------------------------------------------
// Listening.

/// What a receiver made of a stretch of line.
#[derive(Debug, Default)]
struct Heard {
    bits: Vec<bool>,
    /// How many times the carrier came on, and the sample it first did.
    edges: usize,
    first_on: Option<usize>,
    /// Samples it was on for, and whether it was, sample by sample.
    on: usize,
    flags: Vec<bool>,
}

/// Feed `samples` plus noise, as `modem::faxcall` feeds a receiver while it
/// listens: every sample in, the bits taken as they come.
fn listen(rx: &mut Receiver, samples: &[f64], noise: &mut Noise) -> Heard {
    let mut heard = Heard::default();
    let mut was = rx.carrier();
    for (i, &x) in samples.iter().enumerate() {
        rx.feed(x + noise.sample());
        heard.bits.extend(rx.take_bits());
        let now = rx.carrier();
        if now && !was {
            heard.edges += 1;
            heard.first_on.get_or_insert(i);
        }
        if now {
            heard.on += 1;
        }
        heard.flags.push(now);
        was = now;
    }
    heard
}

/// A receiver ready for a burst at `rate`, as the fax layer leaves it when
/// the line turns round to listen.
fn receiver(rate: Rate) -> Receiver {
    let mut rx = Receiver::new(FS);
    rx.set_rate(rate);
    rx.restart();
    rx
}

/// Nothing but noise for `seconds`.
fn quiet(seconds: f64) -> Vec<f64> {
    vec![0.0; (seconds * FS) as usize]
}

/// Which of the eight eighths of `sent` arrived exactly, somewhere in `got`.
/// Eighths rather than the whole, so that a slip in the middle does not read
/// as the whole lost (fax-qam.md 7).
fn eighths(sent: &[bool], got: &[bool]) -> usize {
    let part = sent.len() / 8;
    (0..8).filter(|&k| got.windows(part).any(|w| w == &sent[k * part..(k + 1) * part])).count()
}

/// Bit errors in `sent` as it came out of `got`, lined up by the first
/// sixty-four bits that arrived exactly, or by any eighth that did; every
/// bit of `sent` missing from the end counts as wrong. None if nothing lines
/// up at all.
fn errors(sent: &[bool], got: &[bool]) -> Option<usize> {
    let part = sent.len() / 8;
    let start = (0..8).find_map(|k| {
        let piece = &sent[k * part..k * part + 64.min(part)];
        got.windows(piece.len()).position(|w| w == piece).and_then(|at| at.checked_sub(k * part))
    })?;
    let wrong = sent.iter().enumerate().filter(|&(i, &b)| got.get(start + i) != Some(&b)).count();
    Some(wrong)
}

fn name(rate: Rate) -> &'static str {
    match rate {
        Rate::R4800 => "4800",
        Rate::R2400 => "2400",
    }
}

/// The lines most proofs are made on: a clean one, and one with hiss 30 dB
/// down, which today's telephone circuits are all worse than.
const LINES: [Option<f64>; 2] = [None, Some(30.0)];

fn line_name(snr_db: Option<f64>) -> String {
    match snr_db {
        None => "clean".to_owned(),
        Some(snr) => format!("{snr} dB"),
    }
}

fn working_snr(rate: Rate) -> f64 {
    match rate {
        Rate::R4800 => 17.0,
        Rate::R2400 => 12.0,
    }
}

/// One burst down `line`, heard by a fresh receiver after `lead` seconds of
/// the line with nothing on it: how many eighths of the payload arrived, and
/// what else the receiver did.
struct Run {
    rate: Rate,
    training: Training,
    echo_protection: bool,
    bytes: usize,
    lead: f64,
    snr_db: Option<f64>,
    hz: f64,
    ppm: f64,
    /// A slip, at a fraction of the way through the burst's samples, and
    /// whether it was an insert.
    slip: Option<(f64, bool)>,
    /// A slip at this many samples into the burst instead.
    slip_at: Option<(usize, bool)>,
    reflection: Option<(f64, f64)>,
    /// Whether a long turn-on burst came down the same line first, as it
    /// does before every short one in a call that keeps to 2.5.1.
    after_long: bool,
    /// Samples of the burst gone by before the receiver starts listening, as
    /// the fax layer starts it when its procedure turns the line round.
    skip: usize,
    seed: u64,
}

impl Run {
    fn new(rate: Rate, training: Training) -> Self {
        Self {
            rate,
            training,
            echo_protection: false,
            bytes: 300,
            lead: 0.1,
            snr_db: Some(30.0),
            hz: 0.0,
            ppm: 0.0,
            slip: None,
            slip_at: None,
            reflection: None,
            after_long: false,
            skip: 0,
            seed: 1,
        }
    }

    /// The burst as it arrives, noise not yet added.
    fn line(&self, data: &[bool]) -> Vec<f64> {
        let mut line = self.channel(burst(self.rate, self.training, self.echo_protection, data));
        if let Some((at, inserted)) = self.slip {
            line = slipped(&line, (at * line.len() as f64) as usize, inserted);
        }
        if let Some((at, inserted)) = self.slip_at {
            line = slipped(&line, at, inserted);
        }
        line
    }

    /// A burst from the far end through the line's offset, clock and
    /// reflection, and a little silence after it.
    fn channel(&self, mut far: Vec<f64>) -> Vec<f64> {
        far.extend(quiet(0.1));
        let line = clocked(&shifted(&far, self.hz), self.ppm);
        match self.reflection {
            Some((gain, ms)) => echoed(&line, gain, ms),
            None => line,
        }
    }

    fn noise(&self) -> Noise {
        match self.snr_db {
            Some(snr) => Noise::new(self.seed.wrapping_mul(7919), self.rate, snr, POWER),
            None => Noise::none(),
        }
    }

    /// Eighths of the payload that arrived, the payload's bit errors, and
    /// what the receiver did in the lead-in and in the burst.
    fn go(&self) -> (usize, Option<usize>, Heard, Heard) {
        self.go_with().0
    }

    /// The same, and the receiver as the burst left it.
    fn go_with(&self) -> ((usize, Option<usize>, Heard, Heard), Receiver) {
        let data = bits_of(&payload(self.seed, self.bytes));
        let line = self.line(&data);
        let mut noise = self.noise();
        let mut rx = receiver(self.rate);
        if self.after_long {
            let before = bits_of(&payload(self.seed + 1000, 300));
            let far = self.channel(burst(self.rate, Training::Long, self.echo_protection, &before));
            listen(&mut rx, &quiet(self.lead), &mut noise);
            listen(&mut rx, &far, &mut noise);
            rx.restart();
        }
        let lead = listen(&mut rx, &quiet(self.lead), &mut noise);
        if self.skip > 0 {
            rx.restart();
        }
        let heard = listen(&mut rx, &line[self.skip.min(line.len())..], &mut noise);
        let mut all = lead.bits.clone();
        all.extend_from_slice(&heard.bits);
        ((eighths(&data, &all), errors(&data, &all), lead, heard), rx)
    }
}

// ---------------------------------------------------------------------------
// The proofs.
//
// Each is checked unless `V27TER_MEASURE` is set, when a failure is only
// said: so that a receiver that fails every proof can still be measured by
// all of them, which is how the one this replaced was.

macro_rules! check {
    ($ok:expr, $($what:tt)+) => {
        let ok: bool = $ok;
        if !ok {
            let what = format!($($what)+);
            if std::env::var_os("V27TER_MEASURE").is_some() {
                eprintln!("  (would fail: {what})");
            } else {
                panic!("{what}");
            }
        }
    };
}

#[test]
fn both_rates_at_eight_arrival_phases_long_and_short() {
    let mut worst = 8;
    for snr_db in LINES {
        for rate in [Rate::R4800, Rate::R2400] {
            for training in [Training::Long, Training::Short] {
                let mut whole = 0;
                let mut got = Vec::new();
                for phase in 0..8u64 {
                    let mut run = Run::new(rate, training);
                    run.snr_db = snr_db;
                    run.lead = 0.05 + phase as f64 / FS;
                    run.seed = 10 + phase;
                    let (e, _, _, _) = run.go();
                    whole += usize::from(e == 8);
                    got.push(e);
                    worst = worst.min(e);
                }
                eprintln!("arrival {:>4} {training:?} {}: {whole}/8 whole, eighths {got:?}", name(rate), line_name(snr_db));
            }
        }
    }
    check!(worst == 8, "an arrival phase lost part of a page");
}

#[test]
fn seven_hertz_and_two_hundred_ppm_either_way() {
    let mut worst = 8;
    for snr_db in LINES {
        for rate in [Rate::R4800, Rate::R2400] {
            for training in [Training::Long, Training::Short] {
                let mut got = Vec::new();
                for (hz, ppm) in [(7.0, 200.0), (7.0, -200.0), (-7.0, 200.0), (-7.0, -200.0)] {
                    let mut run = Run::new(rate, training);
                    run.snr_db = snr_db;
                    run.hz = hz;
                    run.ppm = ppm;
                    run.bytes = 600;
                    run.seed = 20 + (hz as i64 + 10) as u64 * 3 + u64::from(ppm > 0.0);
                    let (e, errs, _, _) = run.go();
                    got.push((e, errs));
                    worst = worst.min(e);
                }
                eprintln!(
                    "offset {:>4} {training:?} {}: (eighths, errors) at +7/+200 +7/-200 -7/+200 -7/-200 {got:?}",
                    name(rate),
                    line_name(snr_db)
                );
            }
        }
    }
    check!(worst == 8, "an offset lost part of a page");
}

#[test]
fn noise_at_each_rates_working_signal_to_noise() {
    // A short turn-on after a long one, as a call sends it (2.5.1), and one
    // heard by a receiver that has never heard a long one: 68 symbols are
    // too few to solve 31 taps from, and that is only said.
    for rate in [Rate::R4800, Rate::R2400] {
        for (training, after_long) in [(Training::Long, false), (Training::Short, true), (Training::Short, false)] {
            let (mut wrong, mut sent, mut whole) = (0, 0, 0);
            for seed in 0..4 {
                let mut run = Run::new(rate, training);
                run.after_long = after_long;
                run.snr_db = Some(working_snr(rate));
                run.bytes = 1000;
                run.seed = 30 + seed;
                let (e, errs, _, _) = run.go();
                sent += run.bytes * 8;
                wrong += errs.unwrap_or(run.bytes * 8);
                whole += usize::from(e == 8);
            }
            let ber = wrong as f64 / sent as f64;
            let what = match (training, after_long) {
                (Training::Long, _) => "long",
                (_, true) => "short after long",
                (_, false) => "short, fresh",
            };
            eprintln!(
                "noise {:>4} {what} at {} dB: bit errors {wrong} of {sent} ({ber:.1e}), whole payloads {whole}/4",
                name(rate),
                working_snr(rate)
            );
            if training == Training::Long || after_long {
                check!(ber < 2e-3, "{} {what}: {ber:.1e} of the bits wrong", name(rate));
            }
        }
    }
}

#[test]
fn a_twenty_millisecond_slip_in_the_data_and_in_the_reversals() {
    for snr_db in LINES {
        for rate in [Rate::R4800, Rate::R2400] {
            let mut row = Vec::new();
            for inserted in [true, false] {
                for seed in 0..2 {
                    // In the data, well clear of the training.
                    let mut run = Run::new(rate, Training::Long);
                    run.snr_db = snr_db;
                    run.bytes = 900;
                    run.slip = Some((0.7, inserted));
                    run.seed = 40 + seed;
                    let (e, _, _, _) = run.go();
                    row.push(("data", inserted, e));
                    check!(e >= 6, "{} a slip in the data cost {} eighths", name(rate), 8 - e);
                }
            }
            // Inside segment 3's reversals: forty symbols into the long
            // one's fifty, and ten into the short one's fourteen; and across
            // the long one's join into segment 4, which spoils its start. The
            // transmitter's pulse puts a symbol's centre six symbols after it
            // begins.
            let places = [("long reversals", Training::Long, 40.0), ("short reversals", Training::Short, 10.0), ("long join", Training::Long, 58.0)];
            for (place, training, into) in places {
                for inserted in [true, false] {
                    let mut run = Run::new(rate, training);
                    run.snr_db = snr_db;
                    run.lead = 0.05;
                    run.bytes = 300;
                    run.slip_at = Some(((into * FS / rate.baud()) as usize, inserted));
                    run.seed = 45;
                    let (e, _, _, _) = run.go();
                    row.push((place, inserted, e));
                    if inserted || training == Training::Long {
                        // A drop there takes most of the short turn-on with
                        // it, and a receiver that has heard no long one has
                        // nothing to find the rest with.
                        check!(e == 8, "{} {place}: a slip cost {} eighths", name(rate), 8 - e);
                    }
                }
            }
            eprintln!("slip {:>4} {}: (where, inserted, eighths) {row:?}", name(rate), line_name(snr_db));
        }
    }
}

#[test]
fn a_burst_after_a_long_quiet_stretch_of_line_noise() {
    // fax-qam.md 3.1: a detector whose threshold the hiss is over latches on
    // the hiss before the burst arrives, and measures the carrier from it.
    for rate in [Rate::R4800, Rate::R2400] {
        for snr in [40.0, 30.0] {
            let (mut whole, mut on) = (0, 0.0);
            for seed in 0..6 {
                let mut run = Run::new(rate, Training::Long);
                run.lead = 3.0;
                run.snr_db = Some(snr);
                run.seed = 50 + seed;
                let (e, _, lead, _) = run.go();
                whole += usize::from(e == 8);
                on += lead.on as f64 / (run.lead * FS);
            }
            eprintln!(
                "quiet {:>4} at {snr} dB after 3 s: {whole}/6 whole, carrier on for {:.0}% of the quiet line",
                name(rate),
                100.0 * on / 6.0
            );
            check!(whole == 6, "{} at {snr} dB: a page lost after a quiet line", name(rate));
            check!(on == 0.0, "{} at {snr} dB: carrier on line noise", name(rate));
        }
    }
}

#[test]
fn talker_echo_protection_before_the_burst() {
    // fax-qam.md 3.4 and V.27 ter 5.2.1: circuit 109 is not to come on for
    // the unmodulated carrier of segment 1.
    for snr_db in LINES {
        for rate in [Rate::R4800, Rate::R2400] {
            for training in [Training::Long, Training::Short] {
                let mut row = Vec::new();
                for seed in 0..3 {
                    let mut run = Run::new(rate, training);
                    run.snr_db = snr_db;
                    run.echo_protection = true;
                    run.seed = 60 + seed;
                    let (e, _, lead, heard) = run.go();
                    // Segments 1 and 2 last 215 ms. The carrier on at any
                    // time in the first 185 ms is the phantom, and so is its
                    // going off and on again.
                    let phantom = heard.flags[..(0.185 * FS) as usize].iter().any(|&on| on);
                    let edges = lead.edges + heard.edges;
                    row.push((e, edges, phantom));
                    check!(e == 8, "{} {training:?}: page lost behind echo protection", name(rate));
                    check!(!phantom, "{} {training:?}: carrier on during the unmodulated carrier", name(rate));
                    check!(edges == 1, "{} {training:?}: the carrier came on {edges} times", name(rate));
                }
                eprintln!(
                    "echo protection {:>4} {training:?} {}: (eighths, carrier edges, on in segment 1) {row:?}",
                    name(rate),
                    line_name(snr_db)
                );
            }
        }
    }
}

#[test]
fn several_bursts_in_a_row_as_a_fax_call_makes_them() {
    // A training check of zeros, then pages: a long turn-on, a short one
    // after it, and a short one on a line with an echo on it -- the one the
    // short training's 58 symbols could not teach a fresh equaliser
    // (fax-qam.md 3.5) -- each after a turn of the line with only its noise
    // on it, and the receiver restarted as the fax layer does.
    for rate in [Rate::R4800, Rate::R2400] {
        for reflection in [None, Some((0.35, 0.88)), Some((0.5, 0.44))] {
            let mut row = Vec::new();
            let mut rx = receiver(rate);
            for seed in 0..2u64 {
                let mut noise = Noise::new(70 + seed, rate, 25.0, POWER);
                let check = vec![false; (rate.bits_per_second() as f64 * 1.5) as usize];
                let pages: Vec<(Training, Vec<bool>)> = vec![
                    (Training::Long, check),
                    (Training::Long, bits_of(&payload(71 + seed, 300))),
                    (Training::Short, bits_of(&payload(72 + seed, 300))),
                    (Training::Short, bits_of(&payload(73 + seed, 300))),
                ];
                for (training, data) in pages {
                    let mut far = burst(rate, training, false, &data);
                    far.extend(quiet(0.1));
                    if let Some((gain, ms)) = reflection {
                        far = echoed(&far, gain, ms);
                    }
                    rx.set_rate(rate);
                    rx.restart();
                    let mut got = listen(&mut rx, &quiet(0.3), &mut noise).bits;
                    got.extend(listen(&mut rx, &far, &mut noise).bits);
                    let e = if data.iter().all(|b| !b) {
                        // The check: its longest run of zeros, in eighths.
                        let (mut run, mut longest) = (0, 0);
                        for &b in &got {
                            run = if b { 0 } else { run + 1 };
                            longest = longest.max(run);
                        }
                        (8 * longest / data.len()).min(8)
                    } else {
                        eighths(&data, &got)
                    };
                    row.push(e);
                }
            }
            eprintln!(
                "bursts {:>4} echo {reflection:?}: eighths (check, long, short, short) x2 {row:?}",
                name(rate)
            );
            if reflection.is_none() || rate == Rate::R2400 {
                check!(row.iter().all(|&e| e == 8), "{} {reflection:?}: {row:?}", name(rate));
            }
        }
    }
}

#[test]
fn listening_that_begins_after_the_reversals() {
    // The fax layer turns the line round to listen when its procedure says
    // to, which on a page is some 75 ms after the far end's turn-on began:
    // the whole of segment 3 and a little of segment 4 go by unheard. A long
    // turn-on is trained on its conditioning from wherever it was found; a
    // short one, after a long one, is found with the taps the long one left.
    for snr_db in LINES {
        for rate in [Rate::R4800, Rate::R2400] {
            let mut row = Vec::new();
            let cases = [(Training::Long, false, 10), (Training::Long, false, 70), (Training::Long, true, 300), (Training::Short, true, 4)];
            for (training, after_long, into) in cases {
                let mut run = Run::new(rate, training);
                run.snr_db = snr_db;
                run.after_long = after_long;
                run.seed = 90 + into as u64;
                // The transmitter's pulse puts a symbol's centre six symbols
                // after it begins.
                let symbols = 6 + training.reversals() as usize + into;
                run.skip = (symbols as f64 * FS / rate.baud()) as usize;
                let (e, _, _, _) = run.go();
                row.push((format!("{training:?}{} +{into}", if after_long { " after long" } else { "" }), e));
                // A short one found so late has its first few data symbols
                // go by before the taps can look for it: the core's search
                // reads 48 symbols, and they must all be from after the
                // receiver began to listen.
                let whole = if training == Training::Short { 7 } else { 8 };
                check!(e >= whole, "{} {training:?} after_long {after_long}, {into} into segment 4: {e} eighths", name(rate));
            }
            eprintln!("late {:>4} {}: (joined at, eighths) {row:?}", name(rate), line_name(snr_db));
        }
    }
}

#[test]
fn a_short_turn_on_after_a_long_one_keeps_its_taps() {
    // 2.5.1: the short sequence is for "subsequent turn-around", once the
    // long one has conditioned the receiver. Its 58 symbols of conditioning
    // are too few to solve 31 taps from; the long one's are found again with
    // it instead, through the echo a fresh equaliser could not learn in time
    // (fax-qam.md 3.5).
    for rate in [Rate::R4800, Rate::R2400] {
        for reflection in [None, Some((0.35, 0.88))] {
            let mut run = Run::new(rate, Training::Short);
            run.after_long = true;
            run.reflection = reflection;
            run.seed = 100;
            let ((e, errs, _, _), rx) = run.go_with();
            eprintln!(
                "kept taps {:>4} echo {reflection:?}: {e} eighths, {errs:?} errors, via {:?} at {:.1} dB",
                name(rate),
                rx.trained_via(),
                rx.trained_snr_db()
            );
            check!(e == 8, "{} {reflection:?}: {e} eighths", name(rate));
            check!(rx.trained_via() == Some(Via::Fallback), "{} {reflection:?}: via {:?}", name(rate), rx.trained_via());
        }
    }
}

#[test]
fn line_noise_alone_is_never_a_carrier() {
    for rate in [Rate::R4800, Rate::R2400] {
        for snr in [40.0, 20.0, 10.0, 0.0] {
            let mut rx = receiver(rate);
            let mut noise = Noise::new(80, rate, snr, POWER);
            let heard = listen(&mut rx, &quiet(5.0), &mut noise);
            eprintln!(
                "noise alone {:>4} (as at {snr} dB): carrier on {:.0}% of 5 s, {} bits",
                name(rate),
                100.0 * heard.on as f64 / (5.0 * FS),
                heard.bits.len()
            );
            check!(heard.on == 0, "{} {snr} dB", name(rate));
        }
    }
}
