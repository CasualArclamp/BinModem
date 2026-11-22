//! V.17 end to end: our transmitter into our receiver, through a line.
//!
//! A fax call's high-speed carrier is a burst at a time. The first at a rate
//! is a long train in front of the training check, and the page after it
//! has a resync (T.30 5.1, Note 5), which the receiver can only read with the
//! taps the long train left it. So every test here sends the pair, the way a
//! call does, and looks for the payload in each: a long train and its data,
//! a gap, then a resync and its data.
//!
//! The line is `v32_receiver.rs`'s: the carrier moved by a Hilbert shifter,
//! the far clock by a resampler, and white noise at an Es/N0 against the
//! power of the burst (design.md 9.1), at each rate's working signal to
//! noise (design.md 9.1's table, 3 dB above where an ideal decoder reaches
//! 1e-5). Slips are what a jitter buffer does: 20 ms made up, as a faded
//! repeat, comfort noise or silence, or 20 ms dropped.
//!
//! The figures each test measures are printed; `cargo test -p datapump
//! --release --test v17_loopback -- --nocapture` shows them.

use std::f64::consts::{PI, TAU};

use datapump::v17::{BAUD, Rate, Receiver, Stage, Training, Transmitter};
use dsp::Resampler;

const FS: f64 = 16_000.0;

const RATES: [Rate; 4] = [Rate::R14400, Rate::R12000, Rate::R9600, Rate::R7200];

/// Each rate's working signal to noise, in decibels of Es/N0: design.md
/// 9.1's figures for V.32bis's trellis rates, which are these.
fn working_snr(rate: Rate) -> f64 {
    match rate {
        Rate::R14400 => 27.0,
        Rate::R12000 => 24.0,
        Rate::R9600 => 21.0,
        Rate::R7200 => 18.0,
    }
}

// ---------------------------------------------------------------------------
// The line.

/// A seeded generator (lock_sweep's): xorshift64*, with Box-Muller.
#[derive(Debug, Clone)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let u1 = self.unit().max(1e-18);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next_u64() >> 56) as u8).collect()
    }
}

/// A single-sideband frequency shift, copied from `lock_sweep.rs`: the
/// analytic signal by a Hilbert transformer, turned, and its real part.
#[derive(Debug)]
struct Shifter {
    taps: Vec<f64>,
    hist: Vec<f64>,
    pos: usize,
    centre: usize,
    phase: f64,
    step: f64,
}

impl Shifter {
    fn new(hz: f64) -> Self {
        const N: usize = 255;
        let m = (N - 1) / 2;
        let mut taps = vec![0.0; N];
        for (d, tap) in taps.iter_mut().enumerate() {
            let k = d as isize - m as isize;
            if k % 2 == 0 {
                continue;
            }
            let w = 0.54 - 0.46 * (TAU * d as f64 / (N - 1) as f64).cos();
            *tap = 2.0 / (PI * k as f64) * w;
        }
        Self { taps, hist: vec![0.0; N], pos: 0, centre: m, phase: 0.0, step: hz / FS }
    }

    fn process(&mut self, x: f64) -> f64 {
        let n = self.hist.len();
        self.pos = (self.pos + 1) % n;
        self.hist[self.pos] = x;
        let mut q = 0.0;
        for (d, &tap) in self.taps.iter().enumerate() {
            if tap != 0.0 {
                q += tap * self.hist[(self.pos + n - d) % n];
            }
        }
        let i = self.hist[(self.pos + n - self.centre) % n];
        let turn = self.phase * TAU;
        self.phase += self.step;
        self.phase -= self.phase.floor();
        i * turn.cos() - q * turn.sin()
    }
}

/// What a line does: the carrier moved by `hz`, the far clock `ppm` fast, and
/// white noise at `snr_db` of Es/N0 against `power`, the bursts' own.
fn through(signal: &[f64], hz: f64, ppm: f64, snr_db: f64, power: f64, seed: u64) -> Vec<f64> {
    let mut shifted: Vec<f64> = if hz == 0.0 {
        signal.to_vec()
    } else {
        let mut shifter = Shifter::new(hz);
        signal.iter().map(|&x| shifter.process(x)).collect()
    };
    if ppm != 0.0 {
        let mut clock = Resampler::new(FS * (1.0 + ppm * 1e-6), FS);
        let mut out = Vec::with_capacity(shifted.len());
        for &x in &shifted {
            clock.process(x, &mut out);
        }
        shifted = out;
    }
    if snr_db.is_finite() {
        let sigma = ((FS / 2.0) / BAUD * power * 10f64.powf(-snr_db / 10.0)).sqrt();
        let mut rng = Rng::new(seed);
        for x in &mut shifted {
            *x += sigma * rng.gaussian();
        }
    }
    shifted
}

/// What a jitter buffer plays into a 20 ms hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// The last 20 ms again, faded in and out over 40 samples.
    Repeat,
    /// Noise at the level of the last 20 ms.
    Comfort,
    Silence,
}

/// 20 ms made up at `at`, everything after it that much later.
fn insert(line: &mut Vec<f64>, at: usize, fill: Fill, seed: u64) {
    let n = (0.020 * FS) as usize;
    let last = line[at - n..at].to_vec();
    let made: Vec<f64> = match fill {
        Fill::Repeat => {
            let fade = 40;
            last.iter()
                .enumerate()
                .map(|(i, &x)| {
                    let edge = i.min(n - 1 - i);
                    x * if edge < fade { edge as f64 / fade as f64 } else { 1.0 }
                })
                .collect()
        }
        Fill::Comfort => {
            let rms = (last.iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt();
            let mut rng = Rng::new(seed);
            (0..n).map(|_| rms * rng.gaussian()).collect()
        }
        Fill::Silence => vec![0.0; n],
    };
    line.splice(at..at, made);
}

/// 20 ms lost at `at`.
fn drop_at(line: &mut Vec<f64>, at: usize) {
    line.drain(at..at + (0.020 * FS) as usize);
}

// ---------------------------------------------------------------------------
// The far end, and the near end listening.

/// One burst: the training, `payload`, and the turn-off, as the fax call
/// sends it -- the queue topped up while the carrier is up, and the
/// transmitter stopped once it is empty.
fn burst(tx: &mut Transmitter, rate: Rate, training: Training, payload: &[u8]) -> Vec<f64> {
    tx.start(rate, training);
    tx.push_bytes(payload);
    let mut out = Vec::new();
    while tx.is_transmitting() {
        if tx.trained() && tx.pending_bits() == 0 {
            tx.stop();
        }
        out.push(tx.next_sample());
    }
    out
}

/// Where each part of a call's line is, in samples.
#[derive(Debug, Clone, Copy)]
struct Layout {
    long: (usize, usize),
    resync: (usize, usize),
}

/// A long train with `first` after it, three quarters of a second of
/// nothing -- the V.21 exchange of a real call -- and a resync with `second`,
/// with `quiet` samples in front.
fn pair(rate: Rate, first: &[u8], second: &[u8], quiet: usize, echo_protection: bool) -> (Vec<f64>, Layout) {
    let mut tx = Transmitter::new(FS);
    tx.set_echo_protection(echo_protection);
    let mut line = vec![0.0; 1600 + quiet];
    let a = line.len();
    line.extend(burst(&mut tx, rate, Training::Long, first));
    let b = line.len();
    line.extend(vec![0.0; 12_000]);
    let c = line.len();
    line.extend(burst(&mut tx, rate, Training::Resync, second));
    let d = line.len();
    line.extend(vec![0.0; 3200]);
    (line, Layout { long: (a, b), resync: (c, d) })
}

/// The power the bursts arrive at, for the noise to be set against: segment
/// 2 of the first.
fn burst_power(line: &[f64], layout: Layout) -> f64 {
    let from = layout.long.0 + (0.2 * FS) as usize;
    let to = from + (0.8 * FS) as usize;
    line[from..to].iter().map(|x| x * x).sum::<f64>() / (to - from) as f64
}

/// What the receiver made of a line: the bits of each burst, with the
/// training each was heard as.
#[derive(Debug, Default)]
struct Heard {
    bursts: Vec<(Option<Training>, Vec<bool>)>,
    /// When circuit 109 went on and off, in samples.
    carrier: Vec<(usize, bool)>,
    trained_snr: Vec<f64>,
    /// The signal to noise the receiver measured near the end of each burst's
    /// data.
    snr: Vec<f64>,
}

fn listen(rate: Rate, line: &[f64]) -> Heard {
    let mut rx = Receiver::new(FS);
    rx.set_rate(rate);
    rx.restart();
    let mut heard = Heard::default();
    let mut bits = Vec::new();
    let mut carrier = false;
    // The receiver's own signal to noise every 10 ms of the burst, so that
    // what it was a tenth of a second before the carrier went -- data still
    // arriving, and the loops long settled -- can be looked up.
    let mut snr = Vec::new();
    for (i, &s) in line.iter().enumerate() {
        rx.feed(s);
        if rx.carrier() {
            bits.extend(rx.take_bits());
            if i % 160 == 0 && rx.stage() == Stage::Data {
                snr.push(rx.snr_db());
            }
        } else {
            rx.take_bits();
        }
        if rx.carrier() != carrier {
            carrier = rx.carrier();
            heard.carrier.push((i, carrier));
            if carrier {
                heard.trained_snr.push(rx.trained_snr_db());
            } else {
                heard.bursts.push((rx.heard(), std::mem::take(&mut bits)));
                heard.snr.push(snr.len().checked_sub(10).map_or(f64::NAN, |k| snr[k]));
                snr.clear();
            }
        }
    }
    heard
}

fn bits_of(bytes: &[u8]) -> Vec<bool> {
    bytes.iter().flat_map(|b| (0..8).rev().map(move |i| b >> i & 1 != 0)).collect()
}

/// Where `needle` sits in `hay` exactly, if it does.
fn find(hay: &[bool], needle: &[bool]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Bit errors in `payload` as it arrived in `got`, aligned where the two
/// agree best over the first few hundred bits.
fn errors(got: &[bool], payload: &[bool]) -> Option<usize> {
    let probe = 512.min(payload.len());
    if got.len() < payload.len() {
        return None;
    }
    let (offset, _) = (0..got.len().saturating_sub(payload.len()) + 1)
        .map(|o| (o, got[o..o + probe].iter().zip(&payload[..probe]).filter(|(a, b)| a != b).count()))
        .min_by_key(|(_, e)| *e)?;
    Some(got[offset..].iter().zip(payload).filter(|(a, b)| a != b).count())
}

/// How many of the payload's `chunks` pieces arrived exactly, anywhere.
fn chunks_arrived(got: &[bool], payload: &[bool], chunks: usize) -> usize {
    let size = payload.len() / chunks;
    (0..chunks).filter(|c| find(got, &payload[c * size..(c + 1) * size]).is_some()).count()
}

// ---------------------------------------------------------------------------

#[test]
fn every_rate_carries_a_long_train_and_a_resync_at_every_arrival_phase() {
    // 6.7 samples a symbol, so eight steps of one sample cover a whole
    // symbol of arrival phase, and each is 40.5 degrees of 1800 Hz besides.
    let mut rng = Rng::new(17);
    for rate in RATES {
        for quiet in 0..8 {
            let (first, second) = (rng.bytes(400), rng.bytes(400));
            let (line, _) = pair(rate, &first, &second, quiet, false);
            let heard = listen(rate, &line);
            let case = format!("{rate:?}, {quiet} samples in");
            assert_eq!(heard.bursts.len(), 2, "{case}: {} bursts", heard.bursts.len());
            assert_eq!(heard.bursts[0].0, Some(Training::Long), "{case}");
            assert_eq!(heard.bursts[1].0, Some(Training::Resync), "{case}");
            assert!(find(&heard.bursts[0].1, &bits_of(&first)).is_some(), "{case}: the long train's data");
            assert!(find(&heard.bursts[1].1, &bits_of(&second)).is_some(), "{case}: the resync's data");
            if quiet == 0 {
                eprintln!("{rate:?}: trained {:.1} dB on the long train, {:.1} dB on the resync", heard.trained_snr[0], heard.trained_snr[1]);
            }
        }
    }
}

#[test]
fn a_long_train_after_a_long_train_is_solved_afresh() {
    // A training check refused and sent again, or the first message after
    // CTC: a long train to a receiver that already has taps, which takes it
    // for a resync until segment 2 goes on past 38 symbols.
    let mut rng = Rng::new(3);
    for rate in [Rate::R14400, Rate::R7200] {
        let mut tx = Transmitter::new(FS);
        let (first, second) = (rng.bytes(300), rng.bytes(300));
        let mut line = vec![0.0; 1600];
        line.extend(burst(&mut tx, rate, Training::Long, &first));
        line.extend(vec![0.0; 12_000]);
        line.extend(burst(&mut tx, rate, Training::Long, &second));
        line.extend(vec![0.0; 3200]);
        let heard = listen(rate, &line);
        assert_eq!(heard.bursts.len(), 2, "{rate:?}");
        assert_eq!(heard.bursts[1].0, Some(Training::Long), "{rate:?}: the second heard as a resync");
        assert!(find(&heard.bursts[1].1, &bits_of(&second)).is_some(), "{rate:?}: the second long train's data");
        eprintln!("{rate:?}: second long train trained {:.1} dB", heard.trained_snr[1]);
    }
}

#[test]
fn circuit_109_is_on_from_segment_4_and_off_30_to_50_ms_after_the_signal() {
    // 5.1.4 has it come on during segment 4, and 3.6 go off 30 to 50 ms
    // after the level falls. The talker-echo protection in front must not
    // raise it: an unmodulated carrier is not a burst (fax-qam.md 3.4).
    let payload = vec![0x5au8; 200];
    for echo in [false, true] {
        let (line, layout) = pair(Rate::R14400, &payload, &payload, 0, echo);
        let heard = listen(Rate::R14400, &line);
        assert_eq!(heard.carrier.len(), 4, "echo {echo}: {:?}", heard.carrier);
        let echo_symbols = if echo { 462 + 54 } else { 0 };
        for (k, (from, to), training) in [(0, layout.long, Training::Long), (2, layout.resync, Training::Resync)] {
            let symbol = |n: u64| from + ((echo_symbols + n) as f64 * FS / BAUD) as usize;
            let segment_4 = 256 + training.before_segment_four();
            let (on, off) = (heard.carrier[k].0, heard.carrier[k + 1].0);
            assert!(on >= symbol(segment_4) && on < symbol(segment_4 + 48), "echo {echo} {training:?}: on at {on}, segment 4 from {}", symbol(segment_4));
            // The energy ends with Table 7's 32 symbols of ones; its 48 of
            // nothing follow.
            let energy_ends = to - (48.0 * FS / BAUD) as usize;
            let after = (off - energy_ends) as f64 / FS * 1000.0;
            eprintln!("echo {echo} {training:?}: on {:.1} ms into segment 4, off {after:.1} ms after the signal", (on - symbol(segment_4)) as f64 / 16.0);
            assert!((30.0..=50.0).contains(&after), "echo {echo} {training:?}: off {after:.1} ms after");
        }
    }
}

#[test]
fn every_rate_holds_at_its_working_snr() {
    // Seeded pseudo-random data, two seconds of it in each burst.
    for rate in RATES {
        let mut rng = Rng::new(u64::from(rate.bits_per_second()));
        let bytes = (2.0 * f64::from(rate.bits_per_second()) / 8.0) as usize;
        let (first, second) = (rng.bytes(bytes), rng.bytes(bytes));
        let (clean, layout) = pair(rate, &first, &second, 3, false);
        let line = through(&clean, 0.0, 0.0, working_snr(rate), burst_power(&clean, layout), 99);
        let heard = listen(rate, &line);
        assert_eq!(heard.bursts.len(), 2, "{rate:?}");
        let e1 = errors(&heard.bursts[0].1, &bits_of(&first)).expect("the long train's data");
        let e2 = errors(&heard.bursts[1].1, &bits_of(&second)).expect("the resync's data");
        let ber = (e1 + e2) as f64 / (2 * bytes * 8) as f64;
        eprintln!(
            "{rate:?} at {} dB: {e1} + {e2} errors in {} bits, BER {ber:.1e}; trained {:.1} / {:.1} dB, holding {:.1} / {:.1} dB",
            working_snr(rate),
            2 * bytes * 8,
            heard.trained_snr[0],
            heard.trained_snr[1],
            heard.snr[0],
            heard.snr[1]
        );
        assert!(ber <= 1e-4, "{rate:?}: BER {ber:.1e}");
    }
}

#[test]
fn seven_hertz_and_two_hundred_ppm_either_way() {
    // 2.1: "The receiver must be able to operate with received frequency
    // offsets of up to +/- 7 Hz", and the clock 200 ppm out, twice 2.2's
    // hundredth of a per cent. At each rate's working signal to noise.
    for rate in [Rate::R14400, Rate::R9600, Rate::R7200] {
        for (hz, ppm) in [(7.0, 200.0), (-7.0, -200.0), (7.0, -200.0), (-7.0, 200.0)] {
            let mut rng = Rng::new(7 + u64::from(rate.bits_per_second()));
            let bytes = (1.5 * f64::from(rate.bits_per_second()) / 8.0) as usize;
            let (first, second) = (rng.bytes(bytes), rng.bytes(bytes));
            let (clean, layout) = pair(rate, &first, &second, 5, false);
            let line = through(&clean, hz, ppm, working_snr(rate), burst_power(&clean, layout), 5);
            let heard = listen(rate, &line);
            let case = format!("{rate:?} {hz:+} Hz {ppm:+} ppm");
            assert_eq!(heard.bursts.len(), 2, "{case}");
            let e1 = errors(&heard.bursts[0].1, &bits_of(&first)).expect("the long train's data");
            let e2 = errors(&heard.bursts[1].1, &bits_of(&second)).expect("the resync's data");
            let ber = (e1 + e2) as f64 / (2 * bytes * 8) as f64;
            eprintln!("{case}: {e1} + {e2} errors, BER {ber:.1e}, trained {:.1} / {:.1} dB", heard.trained_snr[0], heard.trained_snr[1]);
            assert!(ber <= 1e-4, "{case}: BER {ber:.1e}");
        }
    }
}

#[test]
fn a_concealment_slip_in_the_data_costs_only_the_symbols_inside_it() {
    // 20 ms is 48 symbols and 36 turns of 1800 Hz exactly, so after it the
    // symbols are where they were and only the made-up stretch is garbage
    // (core.md 4.7). Half a second into each burst's data, at working signal
    // to noise plus three: the payload in 64 pieces, and at most three lost
    // to each slip.
    for rate in [Rate::R14400, Rate::R9600] {
        for slip in [Some(Fill::Repeat), Some(Fill::Comfort), Some(Fill::Silence), None] {
            let mut rng = Rng::new(20);
            let bytes = (1.5 * f64::from(rate.bits_per_second()) / 8.0) as usize;
            let (first, second) = (rng.bytes(bytes), rng.bytes(bytes));
            let (clean, layout) = pair(rate, &first, &second, 1, false);
            let power = burst_power(&clean, layout);
            let mut line = through(&clean, 0.0, 0.0, working_snr(rate) + 3.0, power, 11);
            // The later burst first, so the earlier one's place holds.
            for (from, training) in [(layout.resync.0, Training::Resync), (layout.long.0, Training::Long)] {
                let at = from + ((training.symbols() as f64 + 0.5 * BAUD) * FS / BAUD) as usize;
                match slip {
                    Some(fill) => insert(&mut line, at, fill, 3),
                    None => drop_at(&mut line, at),
                }
            }
            let heard = listen(rate, &line);
            let case = format!("{rate:?} {slip:?}");
            assert_eq!(heard.bursts.len(), 2, "{case}");
            let a = chunks_arrived(&heard.bursts[0].1, &bits_of(&first), 64);
            let b = chunks_arrived(&heard.bursts[1].1, &bits_of(&second), 64);
            eprintln!("{case}: {a} and {b} of 64 pieces");
            assert!(a >= 61 && b >= 61, "{case}: {a} and {b} of 64");
        }
    }
}

#[test]
fn a_concealment_slip_in_segment_1_still_trains() {
    // The hunt loses segment 1 in the hole and finds it again after, with
    // more than a hundred symbols of it to spare; the reversal into segment 2
    // is where it always was. Both trainings, every fill, at 14 400.
    let rate = Rate::R14400;
    for slip in [Some(Fill::Repeat), Some(Fill::Comfort), Some(Fill::Silence), None] {
        let mut rng = Rng::new(21);
        let (first, second) = (rng.bytes(1500), rng.bytes(1500));
        let (clean, layout) = pair(rate, &first, &second, 2, false);
        let power = burst_power(&clean, layout);
        let mut line = through(&clean, 0.0, 0.0, working_snr(rate) + 3.0, power, 12);
        for from in [layout.resync.0, layout.long.0] {
            let at = from + (100.0 * FS / BAUD) as usize;
            match slip {
                Some(fill) => insert(&mut line, at, fill, 4),
                None => drop_at(&mut line, at),
            }
        }
        let heard = listen(rate, &line);
        let case = format!("{slip:?} in segment 1");
        assert_eq!(heard.bursts.len(), 2, "{case}");
        let e1 = errors(&heard.bursts[0].1, &bits_of(&first)).expect("the long train's data");
        let e2 = errors(&heard.bursts[1].1, &bits_of(&second)).expect("the resync's data");
        eprintln!("{case}: {e1} and {e2} errors, trained {:.1} / {:.1} dB, heard {:?} / {:?}", heard.trained_snr[0], heard.trained_snr[1], heard.bursts[0].0, heard.bursts[1].0);
        assert_eq!((heard.bursts[0].0, heard.bursts[1].0), (Some(Training::Long), Some(Training::Resync)), "{case}");
        assert!(e1 + e2 <= 2, "{case}: {e1} and {e2} errors");
    }
}

#[test]
fn a_line_thirty_decibels_down_is_read_the_same() {
    // About what a real line delivered to the V.29 receiver (faxcall.rs's
    // test of it); the gain is in the least-squares solve and the resync's
    // fit, and the carrier's levels are well under it.
    let mut rng = Rng::new(30);
    for rate in [Rate::R14400, Rate::R7200] {
        let (first, second) = (rng.bytes(500), rng.bytes(500));
        let (clean, layout) = pair(rate, &first, &second, 4, false);
        let quiet: Vec<f64> = clean.iter().map(|x| x * 0.0316).collect();
        let line = through(&quiet, 0.0, 0.0, working_snr(rate) + 3.0, burst_power(&quiet, layout), 30);
        let heard = listen(rate, &line);
        assert_eq!(heard.bursts.len(), 2, "{rate:?}");
        let e1 = errors(&heard.bursts[0].1, &bits_of(&first)).expect("the long train's data");
        let e2 = errors(&heard.bursts[1].1, &bits_of(&second)).expect("the resync's data");
        eprintln!("{rate:?} 30 dB down: {e1} + {e2} errors");
        assert_eq!(e1 + e2, 0, "{rate:?}");
    }
}

#[test]
fn a_whole_burst_is_as_long_as_table_3_and_table_7_say() {
    // Nothing queued: the training, then straight into the turn-off.
    for (training, symbols) in [(Training::Long, 3344), (Training::Resync, 342)] {
        let mut tx = Transmitter::new(FS);
        let samples = burst(&mut tx, Rate::R9600, training, &[]).len();
        let want = ((symbols + 32 + 48) as f64 * FS / BAUD).round() as usize;
        assert!(samples.abs_diff(want) <= 7, "{training:?}: {samples} samples, Table 3 and Table 7 make {want}");
    }
}
