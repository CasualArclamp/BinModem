//! The V.32 receiver on its own: trained on a conditioning signal, ours and a
//! real modem's, and started blind on our own transmitter.
//!
//! A call trains its receiver on what 5.2 gives it -- S, then S-bar, then a
//! TRN the receiver knows symbol for symbol -- and this is that, with no
//! start-up around it: the receiver told to hunt, as the start-up tells it
//! when S is due, and then left to find S, the change to S-bar and TRN, and
//! to read the rate signal after. The criteria are design.md 9.3's.
//!
//! The real modem is `tests/vectors/v32bis-14400.wav`, a Conexant softmodem
//! calling another with both ends on one tap, whose first conditioning
//! signals are each alone on the line (`tests/v32_trn.rs` says where, and what
//! is odd about the calling one). It is V.32's counterpart of V.34's check of
//! its training against two real modems.
//!
//! The figures each test measures are printed; `cargo test -p datapump
//! --release --test v32_receiver -- --nocapture` shows them.

use std::f64::consts::{PI, TAU};

use datapump::v32::startup::{RateDetector, Rates, describe_sequence, is_rate_signal, rate_signal};
use datapump::v32::{BAUD, Coding, Mode, Receiver, Signal, Transmitter};
use dsp::Resampler;
use dsp::qam::{Stage, Via};

const FS: f64 = 16_000.0;

const VECTOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v32bis-14400.wav");

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
}

/// A single-sideband frequency shift, copied from `lock_sweep.rs`: the
/// analytic signal by a Hilbert transformer, turned, and its real part. It
/// moves the carrier and nothing else, as a carrier system on a trunk does.
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

/// What a line does to a stretch of signal: the carrier moved, the far clock
/// `ppm` fast, and white noise at `snr_db` of Es/N0 (design.md 9.1) against
/// the power of the stretch from sample `measured` on.
fn through(signal: &[f64], hz: f64, ppm: f64, snr_db: f64, measured: usize, seed: u64) -> Vec<f64> {
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
        let tail = &shifted[measured.min(shifted.len())..];
        let power = tail.iter().map(|x| x * x).sum::<f64>() / tail.len().max(1) as f64;
        let sigma = ((FS / 2.0) / BAUD * power * 10f64.powf(-snr_db / 10.0)).sqrt();
        let mut rng = Rng::new(seed);
        for x in &mut shifted {
            *x += sigma * rng.gaussian();
        }
    }
    shifted
}

// ---------------------------------------------------------------------------
// The far end, and the near end listening.

/// Samples for `symbols` of a signal from our transmitter.
fn send(tx: &mut Transmitter, signal: Signal, symbols: usize, into: &mut Vec<f64>) {
    tx.set_signal(signal);
    let samples = (symbols as f64 * FS / BAUD).round() as usize;
    into.extend((0..samples).map(|_| tx.next_sample()));
}

/// The rate signal our conditioning signals are followed by.
fn r1() -> u16 {
    rate_signal(Rates::between(4800, 14_400))
}

/// Our own conditioning signal from the end at `mode`, as 5.4.2 sends it
/// after its gap: a little silence, S, S-bar, a 1280-symbol TRN, and then
/// R1 for long enough to be read.
fn our_conditioning(mode: Mode) -> (Vec<f64>, usize) {
    let mut tx = Transmitter::new(mode, FS);
    let mut line = Vec::new();
    send(&mut tx, Signal::Silent, 200, &mut line);
    let s_starts = line.len();
    send(&mut tx, Signal::ConditioningS, 256, &mut line);
    send(&mut tx, Signal::ConditioningSbar, 16, &mut line);
    send(&mut tx, Signal::Trn, 1280, &mut line);
    send(&mut tx, Signal::Rate(r1()), 400, &mut line);
    (line, s_starts)
}

/// What a receiver told to hunt made of a line.
#[derive(Debug)]
struct Heard {
    trained: Option<(f64, Via)>,
    s_hz: Option<f64>,
    /// Every rate sequence read, without repeats.
    sequences: Vec<u16>,
    stage: Stage,
}

/// Feed `line` to a receiver at the end at `mode`, idle until sample
/// `hunt_from` and then hunting, as the start-up's cues would have it, and
/// stopping at sample `until`.
fn listen(line: &[f64], mode: Mode, hunt_from: usize, until: usize) -> Heard {
    let mut rx = Receiver::new(mode, FS);
    rx.idle();
    let mut rates = RateDetector::new();
    let mut sequences: Vec<u16> = Vec::new();
    for (n, &x) in line.iter().enumerate().take(until) {
        if n == hunt_from {
            rx.hunt();
        }
        rx.feed(x);
        for bit in rx.take_bits() {
            if let Some(s) = rates.feed(bit)
                && sequences.last() != Some(&s)
            {
                sequences.push(s);
            }
        }
    }
    let trained = rx.trained_via().map(|via| (rx.trained_snr_db(), via));
    Heard { trained, s_hz: rx.s_offset_hz(), sequences, stage: rx.stage() }
}

fn describe(heard: &Heard) -> String {
    let sequences: Vec<String> = heard.sequences.iter().map(|&s| describe_sequence(s)).collect();
    format!(
        "trained {}, S {}, {:?}; read {}",
        heard.trained.map_or("never".into(), |(db, via)| format!("{db:.1} dB via {via:?}")),
        heard.s_hz.map_or("unheard".into(), |hz| format!("{:+.3} Hz", hz)),
        heard.stage,
        if sequences.is_empty() { "nothing".into() } else { sequences.join("; ") }
    )
}

// ---------------------------------------------------------------------------
// The tests.

#[test]
fn our_own_conditioning_signal_trains_the_receiver_above_45_db() {
    for mode in [Mode::Call, Mode::Answer] {
        let (line, s_starts) = our_conditioning(mode);
        let heard = listen(&line, mode.peer(), s_starts / 2, line.len());
        println!("our {mode:?} end's conditioning signal, clean: {}", describe(&heard));
        let (db, via) = heard.trained.expect("never trained");
        assert!(db >= 45.0, "{mode:?}: trained to {db:.1} dB");
        assert_eq!(via, Via::First, "{mode:?}");
        let s_hz = heard.s_hz.expect("S unheard");
        assert!(s_hz.abs() <= 0.1, "{mode:?}: S read {s_hz:+.3} Hz");
        assert!(heard.sequences.contains(&r1()), "{mode:?}: R1 not read: {}", describe(&heard));
    }
}

#[test]
fn our_own_training_holds_at_35_db_with_seven_hertz_and_200_ppm() {
    for (k, (hz, ppm)) in [(7.0, 200.0), (-7.0, -200.0), (7.0, -200.0), (-7.0, 200.0)].into_iter().enumerate() {
        for mode in [Mode::Call, Mode::Answer] {
            let (clean, s_starts) = our_conditioning(mode);
            let line = through(&clean, hz, ppm, 35.0, s_starts, 0x9300 + k as u64);
            let heard = listen(&line, mode.peer(), s_starts / 2, line.len());
            println!("our {mode:?} end's, {hz:+} Hz and {ppm:+} ppm at 35 dB: {}", describe(&heard));
            let (db, _) = heard.trained.expect("never trained");
            assert!(db >= 34.0, "{mode:?}, {hz:+} Hz, {ppm:+} ppm: trained to {db:.1} dB");
            // A far clock fast by `ppm` runs its carrier fast with it, as a
            // real modem's one crystal does: 200 ppm of 1807 Hz is 0.36 Hz.
            let offset = (1800.0 + hz) * (1.0 + ppm * 1e-6) - 1800.0;
            let s_hz = heard.s_hz.expect("S unheard");
            assert!(
                (s_hz - offset).abs() <= 0.1,
                "{mode:?}, {hz:+} Hz, {ppm:+} ppm: S read {s_hz:+.3} Hz against {offset:+.3}"
            );
            assert!(heard.sequences.contains(&r1()), "{mode:?}, {hz:+} Hz, {ppm:+} ppm: R1 not read");
        }
    }
}

/// The real modem's two conditioning signals, each heard by the end it was
/// sent to: the answering modem's by a receiver at the calling end, cued to
/// hunt just after its AC, and the calling modem's by one at the answering
/// end, cued as the answering modem's R1 is ending.
///
/// What the receiver found in the recording, which is more than was known of
/// it (contract.md 5, `tests/v32_trn.rs`):
///
/// - **The answering modem** sends S, S-bar and a long TRN, which 5.2.3
///   allows to 8192 symbols. At 4.93 s the recording jumps -- the carrier
///   turns by about 36 degrees and the symbols move, as one sample dropped
///   would do -- and TRN goes on after it, half a turn round, until R1 at
///   7.08 s: the whole V.32bis offer. The receiver loses the signal at the
///   jump, finds it again, and tracks on.
/// - **The calling modem's** S runs into its TRN with no S-bar between them,
///   so the hunt lapses and the receiver trains where S ended. Its carrier is
///   a fifth of a hertz high and its clock about 100 ppm fast, as
///   `tests/v32_trn.rs` measured. R2 at 9.36 s is V.32's table, not
///   V.32bis's: 4800 and 9600 with trellis. The call in the recording is at
///   9600, whatever the file is called.
/// - Both S signals read 1800 Hz to within a hundredth of a hertz.
///   Contract.md 5's 1798.125 Hz for one of the carriers was read off the
///   answering tones' alternation, and S does not bear it out.
#[test]
fn a_real_modems_trn_trains_the_receiver() {
    let wav = line::wav::read(VECTOR).expect("read the V.32bis vector");
    assert_eq!(f64::from(wav.sample_rate), FS);
    let line: Vec<f64> = wav.mono().iter().map(|&s| f64::from(s)).collect();
    let at = |seconds: f64| (seconds * FS) as usize;
    for (name, mode, from, until, rate_signal) in [
        ("the answering modem's", Mode::Call, 3.75, 7.25, 0b0000_1111_1111_1001),
        ("the calling modem's", Mode::Answer, 7.2, 9.6, 0b0000_0111_1001_0001),
    ] {
        let heard = listen(&line, mode, at(from), at(until));
        println!("{name}, heard at the {mode:?} end: {}", describe(&heard));
        let (db, _) = heard.trained.expect("never trained");
        assert!(db >= 25.0, "{name}: trained to {db:.1} dB");
        let s_hz = heard.s_hz.expect("S unheard");
        assert!(s_hz.abs() <= 0.3, "{name}: S read {:.3} Hz", 1800.0 + s_hz);
        assert!(is_rate_signal(rate_signal));
        assert!(
            heard.sequences.contains(&rate_signal),
            "{name}: the rate signal after it was not read: {}",
            describe(&heard)
        );
    }
}

/// The blind start (design.md 3.8), which only a bare receiver uses: our own
/// transmitter's data from its first symbol, at eight arrival phases. The
/// bare-receiver contract is 4800 at every phase and 9600 with the trellis
/// code at phase 0 (contract.md 1.2); the rest is printed.
#[test]
fn the_blind_start_finds_our_own_transmitter() {
    for (bps, coding) in [
        (4800, Coding::Uncoded),
        (9600, Coding::Uncoded),
        (7200, Coding::Trellis),
        (9600, Coding::Trellis),
        (12_000, Coding::Trellis),
        (14_400, Coding::Trellis),
    ] {
        let mut row = String::new();
        let mut found_everywhere = true;
        for phase in 0..8 {
            let mut tx = Transmitter::new(Mode::Call, FS);
            tx.set_data_rate(bps);
            tx.set_coding(coding);
            let mut rx = Receiver::new(Mode::Answer, FS);
            rx.set_data_rate(bps);
            rx.set_coding(coding);
            let symbols = 2400;
            let mut found = None;
            for _ in 0..phase {
                rx.feed(0.0);
            }
            for n in 0..(symbols as f64 * FS / BAUD) as usize {
                rx.feed(tx.next_sample());
                rx.take_bits();
                if found.is_none() && rx.trained_via() == Some(Via::Blind) {
                    found = Some(n as f64 * BAUD / FS);
                }
            }
            found_everywhere &= found.is_some() && rx.stage() == Stage::Tracking;
            row += &match found {
                Some(at) => format!(" {phase}: {at:.0} sym {:.1}/{:.1} dB,", rx.trained_snr_db(), rx.snr_db()),
                None => format!(" {phase}: never,"),
            };
            let contract = bps == 4800 || (bps == 9600 && coding == Coding::Trellis && phase == 0);
            if contract {
                assert!(found.is_some_and(|at| at <= 300.0), "{bps} {coding:?}, phase {phase}: found at {found:?}");
                assert!(rx.snr_db() >= 30.0, "{bps} {coding:?}, phase {phase}: tracking at {:.1} dB", rx.snr_db());
            }
        }
        println!("blind, {bps} {coding:?} (found, trained / tracking):{row} everywhere {found_everywhere}");
    }
}

/// A slip across the join of S and S-bar still trains the receiver.
///
/// A jitter buffer dropping a packet at the join takes S-bar and TRN's first
/// 32 symbols with it; TRN's A and C then turn S's template over as S-bar
/// would, 48 symbols late, and the first try, which looks eight half symbols
/// either side, finds nothing where it says TRN is -- the retry, searched
/// wide, does. One that fills a packet with comfort noise there leaves S-bar
/// after the noise, too short for a hunt starting over to see; the hunt
/// lapses, and the receiver trains anyway where S ended (design.md 3.2).
#[test]
fn a_slip_across_the_join_of_s_and_s_bar_still_trains() {
    let packet = (0.020 * FS) as usize;
    for mode in [Mode::Call, Mode::Answer] {
        let (clean, s_starts) = our_conditioning(mode);
        // The first sample of S-bar's first symbol, near enough.
        let join = s_starts + (256.0 * FS / BAUD) as usize;
        let power = clean[s_starts..join].iter().map(|x| x * x).sum::<f64>() / (join - s_starts) as f64;
        let mut rng = Rng::new(0x5117);
        let noise: Vec<f64> = (0..packet).map(|_| power.sqrt() * rng.gaussian()).collect();
        for (what, line) in [
            ("a packet dropped", [&clean[..join], &clean[join + packet..]].concat()),
            ("a packet of comfort noise", [&clean[..join], &noise[..], &clean[join..]].concat()),
        ] {
            let heard = listen(&line, mode.peer(), s_starts / 2, line.len());
            println!("our {mode:?} end's, {what} at the join: {}", describe(&heard));
            let (db, _) = heard.trained.expect("never trained");
            assert!(db >= 34.0, "{mode:?}, {what}: trained to {db:.1} dB");
            assert!(heard.sequences.contains(&r1()), "{mode:?}, {what}: R1 not read");
        }
    }
}

/// A probe: what the receiver makes of one of the vector's conditioning
/// signals, every 25 ms, and every rate sequence it reads, cued as the
/// start-up would cue it.
///
/// ```text
/// cargo test -p datapump --release --test v32_receiver trace_the_vector -- --ignored --nocapture
/// ```
///
/// The answering modem's, heard at the calling end, by default;
/// `V32_END=answer` for the calling modem's heard at the answering end.
/// `V32_UNTIL` stops it somewhere else, in seconds, and `V32_STATES=1` prints
/// the four-point decisions made in each 25 ms.
#[test]
#[ignore = "a probe"]
fn trace_the_vector() {
    let wav = line::wav::read(VECTOR).expect("read the V.32bis vector");
    let line: Vec<f64> = wav.mono().iter().map(|&s| f64::from(s)).collect();
    let (mode, from, until) = match std::env::var("V32_END").as_deref() {
        Ok("answer") => (Mode::Answer, 7.2, 9.6),
        _ => (Mode::Call, 3.75, 7.25),
    };
    let until = std::env::var("V32_UNTIL").ok().and_then(|t| t.parse().ok()).unwrap_or(until);
    let states = std::env::var("V32_STATES").is_ok();
    let mut rx = Receiver::new(mode, FS);
    rx.idle();
    let mut rates = RateDetector::new();
    let (mut decided, mut last) = (String::new(), (0.0, 0.0));
    for (n, &x) in line.iter().enumerate().take((until * FS) as usize) {
        if n == (from * FS) as usize {
            rx.hunt();
        }
        rx.feed(x);
        let point = rx.constellation_point();
        if states && point != last {
            last = point;
            // Which of the four the point is nearest, by its angle from A's
            // (-3, -1): a quarter turn to each state after it.
            let turn = (point.1.atan2(point.0) - (-1f64).atan2(-3.0)).rem_euclid(TAU);
            decided.push(char::from(b"ABCD"[((turn / (TAU / 4.0)).round() as usize) % 4]));
        }
        for bit in rx.take_bits() {
            if let Some(s) = rates.feed(bit) {
                println!("{:.3} s: {}", n as f64 / FS, describe_sequence(s));
            }
        }
        if n >= (from * FS) as usize && n % 400 == 0 {
            println!(
                "{:.3} s {:?} snr {:.1} dB, lost for {}, slips {}, rotation {:.1} deg, offset {:+.3} Hz, drift {:+.1} ppm, gain {:+.2} dB, residual {:.3}, carrier {}",
                n as f64 / FS,
                rx.stage(),
                rx.snr_db(),
                rx.lost_for(),
                rx.slips(),
                rx.rotation_degrees(),
                rx.offset_hz(),
                rx.drift_ppm(),
                rx.gain_db(),
                rx.residual_error(),
                rx.carrier(),
            );
            if states {
                println!("  {}", std::mem::take(&mut decided));
            }
        }
    }
}
