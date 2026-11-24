//! What the slower modes do today, measured rather than guessed.
//!
//! Every test in this file is `#[ignore]`d: it is a measurement harness, not a
//! check, and a full sweep takes minutes. Run it deliberately, in release:
//!
//! ```text
//! cargo test -p datapump --release --test lock_sweep -- --ignored --nocapture
//! ```
//!
//! The harness answers one question for each of Bell 103, V.21, V.22, V.22 bis,
//! V.32, V.32 bis, V.29 and V.27 ter: how far can the line be pushed before the
//! receiver stops holding? It is deliberately built out of nothing but the
//! published receivers and `dsp` — no file outside this one was changed to
//! obtain any number it prints.
//!
//! # The line
//!
//! One chain, in the order `docs/design/slow-modes/evidence.md` §4.2 fixes, and
//! for the reasons it gives there:
//!
//! 1. **The far end's sampling clock**, applied by resampling its samples onto
//!    our grid — `Resampler::new(fs * (1 + ppm/1e6), fs)`. This is a real clock
//!    offset and not the trick `v22bis_loopback.rs:269` plays of lying to the
//!    receiver about `fs`, which moves the receiver's own down-converting NCO
//!    and its samples-per-symbol together and so applies a carrier offset at
//!    the same time. Note that a genuine clock offset does carry a
//!    *proportional* carrier offset with it, because the far end's carrier is
//!    derived from the same oscillator: at +200 ppm a 2400 Hz carrier arrives
//!    0.48 Hz high and an 1800 Hz one 0.36 Hz high. That is physics, not
//!    contamination, and it is what V.22 bis 2.5.1's ±0.01% actually means.
//! 2. **Carrier frequency offset**, as a true single-sideband shift: a 255-tap
//!    Hamming-windowed Hilbert transformer builds the analytic signal and a
//!    complex rotation moves it. The symbol rate is left alone, which is the
//!    whole point — this impairment and the one above are then separable.
//! 3. **An echo**, delayed and attenuated. For the duplex modes (Bell 103,
//!    V.22, V.22 bis, V.32) it is an echo of *this* end's own transmitter,
//!    which is what a hybrid returns. For the half-duplex ones (V.21 as T.30
//!    uses it, V.29, V.27 ter) this end is silent while it listens, so the only
//!    echo there can be is the far end's own signal reflected off the far
//!    hybrid, and that is what is applied.
//! 4. **A level step and a dropout**, applied to the sum, which is what a
//!    change in the network path does.
//! 5. **Additive white Gaussian noise**, seeded, scaled so that its power
//!    inside a 3.1 kHz band is the stated number of decibels below the mean
//!    power of the arriving signal.
//!
//! **No echo canceller is used anywhere in this harness.** That matters for one
//! mode only: V.32 puts both directions in the same band and a real V.32 modem
//! cancels its own echo before the receiver sees anything
//! (`crates/datapump/src/v32/startup.rs`, and `v32_loopback.rs:157
//! through_a_hybrid` is where that is already tested). The V.32 echo rows below
//! therefore measure the bare receiver's tolerance of a co-band echo, and
//! understate what a whole V.32 modem does. Every other mode's echo lands out
//! of band or is a listener echo, and its rows mean what they say.
//!
//! # What the Recommendations ask for
//!
//! Each figure below was read from the rendered page, not from
//! `docs/specs/text`, which loses signs and columns.
//!
//! | mode | carrier offset | clock |
//! |---|---|---|
//! | V.21 §3 (Fascicle VIII.1 p. 2) | "the demodulation equipment must tolerate drifts of ± 12 Hz between the frequencies received and their nominal values" | — |
//! | Bell 103 | not an ITU Recommendation; its ±100 Hz shift makes a few hertz negligible | — |
//! | V.22 2.6 (Fascicle VIII.1 p. 3) | "the receiver shall be able to accept errors of at least ± 7 Hz in the received frequencies" | 2.5.1: 600 baud ± 0.01% |
//! | V.22 bis 2.6 (Fascicle VIII.1 p. 4) | "The receiver shall be able to operate with received frequency offsets of up to ± 7 Hz." | 2.5.1: 1200/2400 bit/s ± 0.01%, 600 baud ± 0.01% |
//! | V.32 2.1 (Rec. V.32 (03/93) p. 1) | "The carrier frequency is to be 1800 ± 1 Hz … The receiver must be able to operate with received frequency offsets of up to ± 7 Hz." | 2.3: 2400 bauds ± 0.01% |
//! | V.32 bis 2.1 (Rec. V.32 bis p. 1) | "The receiver must be able to operate with a maximum received frequency offset of up to ± 7 Hz." | 2.1: 2400 symbols/s ± 0.01% |
//! | V.29 §4 (Fascicle VIII.1 p. 4) | "the receiver must be able to accept errors of at least ± 7 Hz in the received signal frequency" | §3: 2400 bauds ± 0.01% |
//! | V.27 ter §3 (Fascicle VIII.1 p. 5) | "the receiver must be able to accept errors of at least ± 7 Hz in the received frequencies" | 2.3.2: 1600/1200 bauds ± 0.01% |
//!
//! Two conforming ends may each be 0.01% out, so ±200 ppm is the worst two
//! legal modems can be apart. That is why the clock sweep stops there.
//!
//! # What is measured
//!
//! The recovered stream is offset from what was sent by an unknown number of
//! bits — differential coding and a self-synchronising descrambler see to that
//! — so it is aligned once, at the lag that best matches over three probe
//! windows, and everything is counted from there. A receiver that never locked
//! finds no lag that agrees and scores a bit error rate near 0.5 rather than an
//! accidental match.
//!
//! * **lock** — seconds from the far end's carrier first appearing on the line
//!   to the first correct bit of a run of 200 with no error in it.
//! * **slicer SNR** — mean |decision|² over mean |received − decision|², in
//!   decibels, taken over the steady state (the last 60% of the run) and
//!   measured against the alphabet that was actually transmitted rather than
//!   the one the receiver believes in. Directly comparable across rates, which
//!   a residual error is not.
//! * **margin** — the mean distance from a decision over half the distance
//!   between the two closest points. One means the average symbol is sitting on
//!   the decision boundary.
//! * **SER** — symbol error rate: the fraction of consecutive groups of
//!   *bits per symbol* aligned bits that contain at least one wrong bit.
//! * **BER** — bit error rate over the payload.
//! * **1st err** — seconds from lock to the first wrong bit.
//! * **lost** — whether the carrier was ever declared gone after being found.
//!
//! Bell 103 is the one mode whose receiver hands out characters rather than
//! bits (start-stop framing re-acquires on every start bit, so there is no bit
//! stream to align). Its unit is a character, its SER is a character error
//! rate, and its BER counts bits differing inside aligned characters.

#![allow(clippy::type_complexity)]

use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

use datapump::framing::AsyncBits;
use datapump::{bell103, v21, v22bis, v27ter, v29, v32};
use dsp::Resampler;

const FS: f64 = 16_000.0;

/// The band a signal-to-noise ratio is quoted in, as every modem Recommendation
/// quotes it.
const NOISE_BAND_HZ: f64 = 3100.0;

// ---------------------------------------------------------------------------
// A seeded generator, so every number here can be produced again.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, spare: None }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*, which is short, fast and has no bad seeds but zero.
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

    fn bit(&mut self) -> bool {
        self.next_u64() & 1 != 0
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 24) as u8
    }

    /// Box-Muller, with the second of each pair kept for next time.
    fn gaussian(&mut self) -> f64 {
        if let Some(g) = self.spare.take() {
            return g;
        }
        let u1 = self.unit().max(1e-18);
        let u2 = self.unit();
        let r = (-2.0 * u1.ln()).sqrt();
        self.spare = Some(r * (TAU * u2).sin());
        r * (TAU * u2).cos()
    }
}

// ---------------------------------------------------------------------------
// The line
// ---------------------------------------------------------------------------

/// An echo: how far behind it comes back, and at what amplitude relative to
/// what was sent.
#[derive(Debug, Clone, Copy)]
struct Echo {
    delay_s: f64,
    amplitude: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Impairments {
    /// Carrier frequency offset in hertz, applied as a single-sideband shift.
    carrier_hz: f64,
    /// The far end's sampling clock against ours, in parts per million.
    clock_ppm: f64,
    /// Signal to noise ratio in decibels, measured in a 3.1 kHz band.
    snr_db: Option<f64>,
    echo: Option<Echo>,
    /// A level change part way through: how many decibels, applied from the
    /// middle of the payload onwards.
    step_db: Option<f64>,
    /// A stretch of silence in the middle of the payload, in seconds.
    dropout_s: Option<f64>,
    /// How long the far end's signal takes to arrive. The project's own line is
    /// 1.1 s round trip.
    delay_s: f64,
}

/// A single-sideband frequency shift: build the analytic signal with a Hilbert
/// transformer, rotate it, take the real part.
///
/// A plain multiply by a cosine would produce both sidebands and a plain
/// re-modulation would move the symbol rate along with the carrier. This moves
/// the carrier and nothing else, which is what a frequency-translating carrier
/// system on a real trunk does.
#[derive(Debug)]
struct Shifter {
    /// `h[d]`, paired with `x[n-d]`.
    taps: Vec<f64>,
    hist: Vec<f64>,
    pos: usize,
    centre: usize,
    phase: f64,
    step: f64,
}

impl Shifter {
    fn new(hz: f64, fs: f64) -> Self {
        const N: usize = 255;
        let m = (N - 1) / 2;
        let mut taps = vec![0.0; N];
        for (d, tap) in taps.iter_mut().enumerate() {
            let k = d as isize - m as isize;
            if k % 2 == 0 {
                continue;
            }
            // Hamming, over the whole length.
            let w = 0.54 - 0.46 * (TAU * d as f64 / (N - 1) as f64).cos();
            *tap = 2.0 / (PI * k as f64) * w;
        }
        Self {
            taps,
            hist: vec![0.0; N],
            pos: 0,
            centre: m,
            phase: 0.0,
            step: hz / fs,
        }
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

/// Put the far end's samples on our clock.
///
/// The far modem believes its sample rate is `fs`; if its oscillator actually
/// runs `ppm` parts per million fast, its waveform reaches us compressed in
/// time by that factor, so `1 + ppm/1e6` of its samples fall in one of ours.
fn apply_clock(far: &[f64], ppm: f64, fs: f64) -> Vec<f64> {
    if ppm == 0.0 {
        return far.to_vec();
    }
    let mut rs = Resampler::new(fs * (1.0 + ppm / 1e6), fs);
    let mut out = Vec::with_capacity(far.len() + 64);
    for &x in far {
        rs.process(x, &mut out);
    }
    out
}

fn apply_carrier(far: &[f64], hz: f64, fs: f64) -> Vec<f64> {
    if hz == 0.0 {
        return far.to_vec();
    }
    let mut sh = Shifter::new(hz, fs);
    far.iter().map(|&x| sh.process(x)).collect()
}

// ---------------------------------------------------------------------------
// The modes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Bell103,
    V21,
    /// V.22: 1200 bit/s, receiver left to work the rate out for itself.
    ///
    /// V.22 bis 2.5.2.2 has the 1200 bit/s signal use the one point V.22 uses
    /// "irrespective of the quadrant concerned … This ensure compatibility with
    /// Recommendation V.22", so the two put the same waveform on the line and
    /// this project has one receiver for both. The only thing that separates
    /// the rows below is whether the receiver was told the rate.
    V22,
    V22bis1200,
    V22bis2400,
    V32 { bps: u32, trellis: bool },
    V29 { bps: u32 },
    V27ter { bps: u32 },
}

impl Mode {
    fn label(self) -> String {
        match self {
            Mode::Bell103 => "Bell 103 300".into(),
            Mode::V21 => "V.21 300".into(),
            Mode::V22 => "V.22 1200".into(),
            Mode::V22bis1200 => "V.22bis 1200".into(),
            Mode::V22bis2400 => "V.22bis 2400".into(),
            Mode::V32 { bps, trellis } => {
                let family = if bps > 9600 || (bps == 7200) { "V.32bis" } else { "V.32" };
                if trellis {
                    format!("{family} {bps}T")
                } else {
                    format!("{family} {bps}")
                }
            }
            Mode::V29 { bps } => format!("V.29 {bps}"),
            Mode::V27ter { bps } => format!("V.27ter {bps}"),
        }
    }

    fn baud(self) -> f64 {
        match self {
            Mode::Bell103 | Mode::V21 => 300.0,
            Mode::V22 | Mode::V22bis1200 | Mode::V22bis2400 => v22bis::BAUD,
            Mode::V32 { .. } | Mode::V29 { .. } => 2400.0,
            Mode::V27ter { bps: 4800 } => 1600.0,
            Mode::V27ter { .. } => 1200.0,
        }
    }

    fn bits_per_symbol(self) -> usize {
        match self {
            Mode::Bell103 | Mode::V21 => 1,
            Mode::V22 | Mode::V22bis1200 => 2,
            Mode::V22bis2400 => 4,
            Mode::V32 { bps, .. } => (bps / 2400) as usize,
            Mode::V29 { bps } => (bps / 2400) as usize,
            Mode::V27ter { bps: 4800 } => 3,
            Mode::V27ter { .. } => 2,
        }
    }

    /// Is this end transmitting while it listens? Only then is there a talker
    /// echo to model; otherwise the echo can only be the far end's own signal
    /// reflected back off the far hybrid.
    fn duplex(self) -> bool {
        matches!(
            self,
            Mode::Bell103 | Mode::V22 | Mode::V22bis1200 | Mode::V22bis2400 | Mode::V32 { .. }
        )
    }

    /// The unit the receiver hands out. Bell 103's start-stop framer gives
    /// whole characters and nothing else.
    fn unit_bits(self) -> usize {
        match self {
            Mode::Bell103 => 8,
            _ => 1,
        }
    }

    /// Seconds of filler before the payload, for the loops to settle, and
    /// seconds of payload to count errors over.
    fn plan(self) -> (f64, f64) {
        match self {
            // Five seconds of payload, because 300 bit/s carries 1500 bits in
            // it and a bit error rate of a thousandth needs that many to mean
            // anything at all.
            Mode::Bell103 | Mode::V21 => (0.35, 5.00),
            Mode::V22 | Mode::V22bis1200 | Mode::V22bis2400 => (0.70, 1.00),
            Mode::V32 { .. } => (0.70, 0.80),
            // The training is 608 symbols, 253 ms (Table 5/V.29).
            Mode::V29 { .. } => (0.50, 0.80),
            // Long training: 708 ms at 4800, 943 ms at 2400 (Table 3/V.27 ter).
            Mode::V27ter { bps: 4800 } => (0.90, 0.80),
            Mode::V27ter { .. } => (1.15, 0.80),
        }
    }

    /// The points this mode actually puts on the line, scaled the way the
    /// receiver's `constellation_point` scales what it reports.
    ///
    /// `None` for the two frequency-shift modes, which have no constellation:
    /// their slicer margin is the discriminator level and is handled apart.
    fn alphabet(self) -> Option<Vec<(f64, f64)>> {
        let rms10 = 10f64.sqrt();
        match self {
            Mode::Bell103 | Mode::V21 => None,
            // The four points of V.22 bis Figure 2 that 1200 bit/s uses: the
            // one labelled 01, at (3,1), turned into each quadrant.
            Mode::V22 | Mode::V22bis1200 => Some(
                [(3.0, 1.0), (-1.0, 3.0), (-3.0, -1.0), (1.0, -3.0)]
                    .iter()
                    .map(|&(x, y)| (x / rms10, y / rms10))
                    .collect(),
            ),
            // All sixteen of Figure 2/V.22 bis, which is also Figure 2/V.32:
            // a grid of one and three.
            Mode::V22bis2400 => Some(grid16(rms10)),
            Mode::V32 { bps, trellis } => {
                let coding = if trellis { v32::Coding::Trellis } else { v32::Coding::Uncoded };
                match v32::coding_for(bps, coding) {
                    Some(coded) => Some(
                        (0..coded.size())
                            .map(|i| {
                                let (x, y) = coded.point(i);
                                (x / rms10, y / rms10)
                            })
                            .collect(),
                    ),
                    // 9600 without the trellis is the sixteen of Figure 2;
                    // 4800 and everything below is A B C D of Figure 1.
                    None if bps >= 9600 => Some(grid16(rms10)),
                    None => Some(
                        [(-3.0, -1.0), (1.0, -3.0), (3.0, 1.0), (-1.0, 3.0)]
                            .iter()
                            .map(|&(x, y)| (x / rms10, y / rms10))
                            .collect(),
                    ),
                }
            }
            Mode::V29 { bps } => {
                let rate = v29_rate(bps);
                let rms = rate.rms();
                Some(
                    rate.constellation()
                        .iter()
                        .map(|p| {
                            let (x, y) = p.xy();
                            (x / rms, y / rms)
                        })
                        .collect(),
                )
            }
            // Eight phases on the unit circle, which is what the V.27 ter
            // slicer decides over at both rates: the training in front of the
            // data is two-phase whatever the rate, so four would read a
            // reversal as a quarter turn of error.
            Mode::V27ter { .. } => Some(
                (0..8)
                    .map(|k| {
                        let a = TAU * f64::from(k) / 8.0;
                        (a.cos(), a.sin())
                    })
                    .collect(),
            ),
        }
    }
}

fn grid16(rms10: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(16);
    for &x in &[-3.0f64, -1.0, 1.0, 3.0] {
        for &y in &[-3.0f64, -1.0, 1.0, 3.0] {
            out.push((x / rms10, y / rms10));
        }
    }
    out
}

fn v29_rate(bps: u32) -> v29::Rate {
    match bps {
        9600 => v29::Rate::R9600,
        7200 => v29::Rate::R7200,
        _ => v29::Rate::R4800,
    }
}

fn v27ter_rate(bps: u32) -> v27ter::Rate {
    match bps {
        4800 => v27ter::Rate::R4800,
        _ => v27ter::Rate::R2400,
    }
}

/// Half the distance between the two closest points of a mode's alphabet, which
/// is its decision boundary and the only number every judgement about lock has
/// to be divided by first.
fn half_spacing(points: &[(f64, f64)]) -> f64 {
    let mut closest = f64::INFINITY;
    for (i, a) in points.iter().enumerate() {
        for b in points.iter().skip(i + 1) {
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
            closest = closest.min(d);
        }
    }
    closest / 2.0
}

fn nearest(points: &[(f64, f64)], at: (f64, f64)) -> (f64, f64) {
    let mut best = points[0];
    let mut d2 = f64::INFINITY;
    for &p in points {
        let d = (p.0 - at.0).powi(2) + (p.1 - at.1).powi(2);
        if d < d2 {
            d2 = d;
            best = p;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Transmitters
// ---------------------------------------------------------------------------

/// Generate `samples` of one mode's transmitter, and every data bit pushed into
/// it in the order it went in.
///
/// The queue is topped up as it drains rather than filled once: three of these
/// transmitters hold their pending bits in a `Vec` and pop from the front, so a
/// hundred thousand bits pushed in one go would be quadratic.
fn transmit(mode: Mode, samples: usize, lead_in: usize, seed: u64) -> (Vec<f64>, Vec<bool>) {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(samples);
    let mut sent = Vec::new();

    match mode {
        Mode::Bell103 => {
            let mut tx = bell103::Bell103Tx::new(bell103::Role::Answer, FS);
            let framer = AsyncBits::new(8);
            tx.set_transmitting(true);
            for n in 0..samples {
                // Idle mark for the lead-in; the framer's start bits are what
                // the receiver re-acquires on, so nothing is needed in front.
                if n >= lead_in && tx.pending_bits() < 64 {
                    let byte = rng.byte();
                    for i in (0..8).rev() {
                        sent.push(byte >> i & 1 != 0);
                    }
                    tx.push_bits(&framer.encode(byte));
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V21 => {
            let mut tx = v21::Sender::new(FS);
            tx.set_transmitting(true);
            for n in 0..samples {
                if tx.pending_bits() < 64 {
                    // Flags through the lead-in, so the bit clock has a
                    // transition every two bits to start on, then payload.
                    // Eight bytes at a time and not thirty-two: the queue is
                    // topped up when it runs low, so a large chunk overshoots
                    // the moment the lead-in was meant to end by its own
                    // length, and at 300 bit/s that is seconds.
                    let mut chunk = Vec::with_capacity(64);
                    for _ in 0..8 {
                        let byte = if n < lead_in { 0x7e } else { rng.byte() };
                        for i in (0..8).rev() {
                            let bit = byte >> i & 1 != 0;
                            chunk.push(bit);
                            sent.push(bit);
                        }
                    }
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V22 | Mode::V22bis1200 | Mode::V22bis2400 => {
            let rate = if mode == Mode::V22bis2400 {
                v22bis::Rate::Bps2400
            } else {
                v22bis::Rate::Bps1200
            };
            let mut tx = v22bis::Transmitter::at_rate(v22bis::Channel::Answering, rate, FS);
            for _ in 0..samples {
                if tx.pending_bits() < 256 {
                    let mut chunk = Vec::with_capacity(256);
                    for _ in 0..256 {
                        let bit = rng.bit();
                        chunk.push(bit);
                        sent.push(bit);
                    }
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V32 { bps, trellis } => {
            let mut tx = v32::Transmitter::new(v32::Mode::Answer, FS);
            tx.set_data_rate(bps);
            tx.set_coding(if trellis { v32::Coding::Trellis } else { v32::Coding::Uncoded });
            for _ in 0..samples {
                if tx.pending_bits() < 256 {
                    let mut chunk = Vec::with_capacity(256);
                    for _ in 0..256 {
                        let bit = rng.bit();
                        chunk.push(bit);
                        sent.push(bit);
                    }
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V29 { bps } => {
            let mut tx = v29::Transmitter::new(FS);
            tx.start(v29_rate(bps));
            for _ in 0..samples {
                if tx.pending_bits() < 256 {
                    let mut chunk = Vec::with_capacity(256);
                    for _ in 0..256 {
                        let bit = rng.bit();
                        chunk.push(bit);
                        sent.push(bit);
                    }
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V27ter { bps } => {
            let mut tx = v27ter::Transmitter::new(FS);
            tx.start(v27ter_rate(bps), v27ter::Training::Long);
            for _ in 0..samples {
                if tx.pending_bits() < 256 {
                    let mut chunk = Vec::with_capacity(256);
                    for _ in 0..256 {
                        let bit = rng.bit();
                        chunk.push(bit);
                        sent.push(bit);
                    }
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
    }
    (out, sent)
}

/// This end's own transmitter, for the echo path. Only the duplex modes have
/// one; the rest are silent while they listen.
fn own_transmitter(mode: Mode, samples: usize, seed: u64) -> Vec<f64> {
    let mut rng = Rng::new(seed ^ 0xA5A5_5A5A_A5A5_5A5A);
    let mut out = Vec::with_capacity(samples);
    match mode {
        Mode::Bell103 => {
            let mut tx = bell103::Bell103Tx::new(bell103::Role::Originate, FS);
            let framer = AsyncBits::new(8);
            tx.set_transmitting(true);
            for _ in 0..samples {
                if tx.pending_bits() < 64 {
                    tx.push_bits(&framer.encode(rng.byte()));
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V22 | Mode::V22bis1200 | Mode::V22bis2400 => {
            let rate = if mode == Mode::V22bis2400 {
                v22bis::Rate::Bps2400
            } else {
                v22bis::Rate::Bps1200
            };
            let mut tx = v22bis::Transmitter::at_rate(v22bis::Channel::Calling, rate, FS);
            for _ in 0..samples {
                if tx.pending_bits() < 256 {
                    let chunk: Vec<bool> = (0..256).map(|_| rng.bit()).collect();
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
        Mode::V32 { bps, trellis } => {
            let mut tx = v32::Transmitter::new(v32::Mode::Call, FS);
            tx.set_data_rate(bps);
            tx.set_coding(if trellis { v32::Coding::Trellis } else { v32::Coding::Uncoded });
            for _ in 0..samples {
                if tx.pending_bits() < 256 {
                    let chunk: Vec<bool> = (0..256).map(|_| rng.bit()).collect();
                    tx.push_bits(&chunk);
                }
                out.push(tx.next_sample());
            }
        }
        _ => out.resize(samples, 0.0),
    }
    out
}

// ---------------------------------------------------------------------------
// Receivers, behind one door
// ---------------------------------------------------------------------------

/// The six receivers differ by nearly a kilobyte in size, which matters
/// nowhere here: one is built per run and then fed for tens of thousands of
/// samples.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum Rx {
    Bell103(bell103::Bell103Rx),
    V21(v21::Receiver),
    V22bis(v22bis::Receiver),
    V32(v32::Receiver),
    V29(v29::Receiver),
    V27ter(v27ter::Receiver),
}

/// One feed's worth of what came out: the units recovered, the symbol the
/// slicer saw if a new one arrived, and whether a carrier is up.
#[derive(Debug, Default)]
struct Step {
    units: Vec<u16>,
    point: Option<(f64, f64)>,
    level: Option<f64>,
}

impl Rx {
    fn new(mode: Mode) -> Self {
        match mode {
            Mode::Bell103 => Rx::Bell103(bell103::Bell103Rx::new(bell103::Role::Originate, FS)),
            Mode::V21 => Rx::V21(v21::Receiver::new(FS)),
            Mode::V22 => Rx::V22bis(v22bis::Receiver::new(v22bis::Channel::Calling, FS)),
            Mode::V22bis1200 | Mode::V22bis2400 => {
                let mut rx = v22bis::Receiver::new(v22bis::Channel::Calling, FS);
                rx.set_rate(if mode == Mode::V22bis2400 {
                    v22bis::Rate::Bps2400
                } else {
                    v22bis::Rate::Bps1200
                });
                Rx::V22bis(rx)
            }
            Mode::V32 { bps, trellis } => {
                let mut rx = v32::Receiver::new(v32::Mode::Call, FS);
                rx.set_data_rate(bps);
                rx.set_coding(if trellis { v32::Coding::Trellis } else { v32::Coding::Uncoded });
                Rx::V32(rx)
            }
            Mode::V29 { bps } => {
                let mut rx = v29::Receiver::new(FS);
                rx.set_rate(v29_rate(bps));
                Rx::V29(rx)
            }
            Mode::V27ter { bps } => {
                let mut rx = v27ter::Receiver::new(FS);
                rx.set_rate(v27ter_rate(bps));
                Rx::V27ter(rx)
            }
        }
    }

    fn carrier(&self) -> bool {
        match self {
            Rx::Bell103(r) => r.carrier(),
            Rx::V21(r) => r.carrier(),
            Rx::V22bis(r) => r.carrier(),
            Rx::V32(r) => r.carrier(),
            Rx::V29(r) => r.carrier(),
            Rx::V27ter(r) => r.carrier(),
        }
    }

    /// The equalised symbol the slicer last saw, or `None` if it has not moved
    /// since the last look.
    ///
    /// Change detection rather than a counter, because no receiver here reports
    /// a symbol boundary: two consecutive equalised outputs being bit-identical
    /// is not something a line produces.
    fn point(&self, last: &mut (f64, f64)) -> Option<(f64, f64)> {
        let p = match self {
            Rx::V22bis(r) => r.constellation_point(),
            Rx::V32(r) => r.constellation_point(),
            Rx::V29(r) => r.constellation_point(),
            Rx::V27ter(r) => r.constellation_point(),
            _ => return None,
        };
        if p == *last {
            return None;
        }
        *last = p;
        Some(p)
    }

    fn feed(&mut self, x: f64, out: &mut Step) {
        out.units.clear();
        out.point = None;
        out.level = None;
        match self {
            Rx::Bell103(r) => {
                if let Some(byte) = r.feed(x) {
                    out.units.push(u16::from(byte));
                }
                out.level = r.take_symbol();
            }
            Rx::V21(r) => {
                if let Some(bit) = r.feed(x) {
                    out.units.push(u16::from(bit));
                }
                out.level = r.take_symbol();
            }
            Rx::V22bis(r) => {
                r.feed(x);
                out.units.extend(r.take_bits().into_iter().map(u16::from));
            }
            Rx::V32(r) => {
                r.feed(x);
                out.units.extend(r.take_bits().into_iter().map(u16::from));
            }
            Rx::V29(r) => {
                r.feed(x);
                out.units.extend(r.take_bits().into_iter().map(u16::from));
            }
            Rx::V27ter(r) => {
                r.feed(x);
                out.units.extend(r.take_bits().into_iter().map(u16::from));
            }
        }
    }

    /// What the receiver believes the signalling rate is, where it decides for
    /// itself rather than being told.
    fn believed_rate(&self) -> Option<u32> {
        match self {
            Rx::V22bis(r) => Some(r.rate().bits_per_second()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The numbers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Metrics {
    lock_s: Option<f64>,
    first_error_s: Option<f64>,
    slicer_snr_db: f64,
    margin: f64,
    ser: f64,
    ber: f64,
    carrier_lost: bool,
    carrier_down_s: f64,
    believed_rate: Option<u32>,
    units: usize,
    /// How many times the recovered stream jumped relative to what was sent.
    /// One is a receiver that lost symbols and carried on; more than one, or
    /// one on an otherwise clean line, is a timing loop slipping.
    slips: usize,
}

impl Metrics {
    /// Did this receiver carry the payload at all? A tenth of a per cent of
    /// bits wrong is a link V.42 would keep alive; anything worse is not.
    fn held(&self) -> bool {
        self.lock_s.is_some() && self.ber < 1e-3
    }
}

/// One run: one transmitter into one receiver through one line.
fn run(mode: Mode, imp: Impairments, arrival: usize, seed: u64) -> Metrics {
    let (lead_in_s, payload_s) = mode.plan();
    let lead_in = (lead_in_s * FS) as usize;
    let far_len = ((lead_in_s + payload_s + 0.15) * FS) as usize;

    let (far, sent) = transmit(mode, far_len, lead_in, seed);
    let far = apply_clock(&far, imp.clock_ppm, FS);
    let far = apply_carrier(&far, imp.carrier_hz, FS);

    // Silence in front: the arrival phase, and then however long the far end's
    // signal takes to get here.
    let quiet = arrival + (imp.delay_s * FS) as usize;
    let total = quiet + far.len();
    let carrier_at = quiet as f64 / FS;

    // Mean power of what actually arrives, which is what a signal-to-noise
    // ratio is quoted against.
    let power = far.iter().map(|x| x * x).sum::<f64>() / far.len().max(1) as f64;
    let sigma = match imp.snr_db {
        Some(db) => (power * (FS / 2.0 / NOISE_BAND_HZ) / 10f64.powf(db / 10.0)).sqrt(),
        None => 0.0,
    };

    let own = if mode.duplex() && imp.echo.is_some() {
        own_transmitter(mode, total, seed)
    } else {
        Vec::new()
    };
    let echo_delay = imp.echo.map(|e| (e.delay_s * FS) as usize).unwrap_or(0);

    // The step and the dropout land in the middle of the payload.
    let payload_at = quiet + lead_in;
    let mid = payload_at + (payload_s * 0.5 * FS) as usize;
    let step_gain = imp.step_db.map(|db| 10f64.powf(db / 20.0)).unwrap_or(1.0);
    let dropout = imp.dropout_s.map(|s| (mid, mid + (s * FS) as usize));

    let alphabet = mode.alphabet();
    let boundary = alphabet.as_ref().map(|a| half_spacing(a)).unwrap_or(0.0);

    let mut rx = Rx::new(mode);
    let mut rng = Rng::new(seed ^ 0xD1B5_4A32_D192_ED03);
    let mut step = Step::default();
    let mut last_point = (f64::NAN, f64::NAN);

    let mut units: Vec<u16> = Vec::new();
    let mut times: Vec<f64> = Vec::new();

    // Slicer statistics, taken over the steady state only: a receiver that is
    // still acquiring is not what this number is about.
    let settled = payload_at + (payload_s * 0.4 * FS) as usize;
    let (mut sig, mut err, mut dist, mut count) = (0.0f64, 0.0f64, 0.0f64, 0usize);

    let mut seen_carrier = false;
    let mut carrier_lost = false;
    let mut down = 0usize;

    for n in 0..total {
        let mut x = if n >= quiet { far[n - quiet] } else { 0.0 };
        if let Some(e) = imp.echo
            && n >= echo_delay
        {
            let source = if mode.duplex() {
                own.get(n - echo_delay).copied().unwrap_or(0.0)
            } else if n - echo_delay >= quiet {
                far[n - echo_delay - quiet]
            } else {
                0.0
            };
            x += e.amplitude * source;
        }
        if n >= mid {
            x *= step_gain;
        }
        if let Some((a, b)) = dropout
            && n >= a
            && n < b
        {
            x = 0.0;
        }
        if sigma != 0.0 {
            x += sigma * rng.gaussian();
        }

        rx.feed(x, &mut step);
        let t = n as f64 / FS;
        for &u in &step.units {
            units.push(u);
            times.push(t);
        }

        let carrier = rx.carrier();
        if carrier {
            seen_carrier = true;
        } else if seen_carrier {
            carrier_lost = true;
            down += 1;
        }

        if n >= settled {
            if let Some(points) = alphabet.as_ref() {
                if let Some(p) = rx.point(&mut last_point) {
                    let q = nearest(points, p);
                    sig += q.0 * q.0 + q.1 * q.1;
                    let d2 = (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2);
                    err += d2;
                    dist += d2.sqrt();
                    count += 1;
                }
            } else if let Some(level) = step.level {
                // Frequency shift keying has no constellation. The
                // discriminator level is the whole of the eye: the decision is
                // its sign at the mean magnitude, and the boundary is zero.
                sig += level * level;
                dist += level.abs();
                count += 1;
            }
        }
    }

    // Frequency shift keying: the error is what is left of each reading after
    // the mean magnitude it should have had, and the boundary is that mean.
    let (slicer_snr_db, margin) = if alphabet.is_some() {
        if count == 0 || err <= 0.0 {
            (f64::INFINITY, 0.0)
        } else {
            (10.0 * (sig / err).log10(), (dist / count as f64) / boundary)
        }
    } else if count == 0 {
        (f64::NEG_INFINITY, f64::INFINITY)
    } else {
        let mean_mag = dist / count as f64;
        let mean_sq = sig / count as f64;
        // Variance about ±mean_mag, which is the slicer's own alphabet.
        let noise = (mean_sq - mean_mag * mean_mag).max(1e-18);
        (10.0 * (mean_mag * mean_mag / noise).log10(), noise.sqrt() / mean_mag.max(1e-18))
    };

    let mut m = score(mode, &sent, &units, &times, carrier_at);
    m.slicer_snr_db = slicer_snr_db;
    m.margin = margin;
    m.carrier_lost = carrier_lost;
    m.carrier_down_s = down as f64 / FS;
    m.believed_rate = rx.believed_rate();
    m
}

/// Align what came out against what went in, then count.
///
/// Two things make this harder than a subtraction. The recovered stream is
/// offset from the sent one by an unknown number of bits — differential coding
/// and a self-synchronising descrambler see to that, and a receiver that starts
/// listening before the far end starts talking emits a stretch of nonsense
/// first. And the offset can *change* part way through: a dropout costs the
/// receiver some symbols, and every bit after it is shifted. Counting that as
/// a hundred per cent of the rest of the call wrong would say nothing about the
/// receiver and everything about the arithmetic.
///
/// So: one global lag, found over three probe windows taken from after the far
/// end's carrier arrived, and then a walk in windows of 128 units which is
/// allowed to move the lag — but only when the window it has is plainly broken
/// and the one it would move to is plainly right. A receiver that has genuinely
/// lost the signal finds no such lag and scores a bit error rate near a half,
/// which is what it deserves. A receiver that slipped a symbol and carried on
/// scores its real error rate, and the slip is counted and reported on its own.
fn score(mode: Mode, sent: &[bool], got: &[u16], times: &[f64], carrier_at: f64) -> Metrics {
    let unit_bits = mode.unit_bits();
    let mut m = Metrics { units: got.len(), ..Metrics::default() };

    // What went in, as units of the same size as what came out.
    let sent_units: Vec<u16> = if unit_bits == 1 {
        sent.iter().map(|&b| u16::from(b)).collect()
    } else {
        sent.chunks_exact(unit_bits)
            .map(|c| c.iter().fold(0u16, |a, &b| a << 1 | u16::from(b)))
            .collect()
    };

    // Nothing before the far end's carrier reached the line can be data. It is
    // counted nowhere: a receiver handing bits up out of silence is a fault of
    // its own (see the carrier columns), not a bit error rate.
    let after = times.partition_point(|&t| t < carrier_at);
    let live = got.len().saturating_sub(after);
    if live < 128 || sent_units.len() < 128 {
        m.ber = 0.5;
        m.ser = 1.0;
        return m;
    }

    // How far the two streams can be apart. Negative, and by more than the
    // latency: everything the receiver emitted before the far end's first data
    // bit sits in front of it — the silence, and, for the two fax modes, a
    // whole training sequence the harness never pushed any bits for.
    let slack: isize = match mode {
        Mode::V29 { .. } | Mode::V27ter { .. } => 4000 / unit_bits.max(1) as isize,
        _ => 800 / unit_bits.max(1) as isize,
    };
    let lo = -(after as isize) - slack;
    let hi = slack;

    let window = 256usize.min(live / 3);
    // Scored over three windows at once and not one at a time. A lead-in of
    // flags is periodic, and a lag wrong by a whole period matches it
    // perfectly: only a window of real data can tell the right lag from the
    // others, and summing across the three settles it.
    let probes: Vec<usize> = [0.25f64, 0.50, 0.75]
        .iter()
        .map(|&f| after + ((live as f64 * f) as usize).min(live - window))
        .collect();

    let agreement = |probe: usize, lag: isize, width: usize| -> Option<usize> {
        let start = probe as isize + lag;
        if start < 0 || start as usize + width > sent_units.len() || probe + width > got.len() {
            return None;
        }
        let start = start as usize;
        let mut agree = 0usize;
        for i in 0..width {
            if got[probe + i] == sent_units[start + i] {
                agree += 1;
            }
        }
        Some(agree)
    };

    let mut best = (isize::MIN, 0usize);
    for lag in lo..=hi {
        let mut agree = 0usize;
        let mut usable = false;
        for &probe in &probes {
            if let Some(a) = agreement(probe, lag, window) {
                usable = true;
                agree += a;
            }
        }
        if usable && agree > best.1 {
            best = (lag, agree);
        }
    }
    if best.0 == isize::MIN {
        m.ber = 0.5;
        m.ser = 1.0;
        return m;
    }

    // Walk it, allowing the lag to move where it plainly has to.
    //
    // The window is a sixteenth of the run, between sixteen units and a
    // hundred and twenty-eight: a window longer than the stretch between two
    // slips can never be matched by any single lag, and Bell 103 hands out a
    // hundred and fifty characters in five seconds where V.32 bis hands out
    // twenty thousand bits in two.
    let win = (live / 16).clamp(16, 128);
    // A window this well matched is the right lag; one this badly matched is
    // not. Between the two nothing moves, so noise never buys a re-alignment.
    let good = win * 9 / 10;
    let bad = win * 3 / 4;
    const REACH: isize = 64;

    let mut lag = best.0;
    let mut wrong: Vec<Option<u16>> = vec![None; got.len()];
    let mut pos = after;
    while pos < got.len() {
        let width = win.min(got.len() - pos);
        // Only a window that lies wholly inside what was sent is evidence of
        // anything. One that hangs off either end cannot be scored, and
        // treating an unscorable window as a badly matched one is how the
        // first window of a run learns to re-align onto a periodic lead-in: a
        // stream of flags matches itself at any multiple of eight bits, and
        // the alias scores perfectly until the data starts.
        if width == win
            && let Some(here) = agreement(pos, lag, win)
            && here < bad
        {
            let mut best_local = (lag, here);
            for l in (lag - REACH).max(lo)..=(lag + REACH).min(hi) {
                if let Some(a) = agreement(pos, l, win)
                    && a > best_local.1
                {
                    best_local = (l, a);
                }
            }
            if best_local.1 >= good && best_local.0 != lag {
                lag = best_local.0;
                m.slips += 1;
            }
        }
        for i in pos..pos + width {
            let j = i as isize + lag;
            if j >= 0 && (j as usize) < sent_units.len() {
                wrong[i] = Some(got[i] ^ sent_units[j as usize]);
            }
        }
        pos += width;
    }

    // Lock is a run of 200 bits with nothing wrong in it.
    let need = (200 / unit_bits.max(1)).max(1);
    let mut run_ok = 0usize;
    let mut lock_at: Option<usize> = None;
    for (i, w) in wrong.iter().enumerate().skip(after) {
        match w {
            Some(0) => {
                run_ok += 1;
                if run_ok >= need {
                    lock_at = Some(i + 1 - run_ok);
                    break;
                }
            }
            _ => run_ok = 0,
        }
    }
    m.lock_s = lock_at.map(|at| (times[at] - carrier_at).max(0.0));

    // Everything counted from lock onwards. Counting the acquisition transient
    // in with the payload would put a floor under every bit error rate set by
    // how long the receiver took to arrive, which is the other column.
    let from = lock_at.unwrap_or(after);
    let mut wrong_bits = 0usize;
    let mut total_bits = 0usize;
    let mut wrong_units = 0usize;
    let mut total_units = 0usize;
    let mut first_error: Option<f64> = None;
    for (i, w) in wrong.iter().enumerate().skip(from) {
        let Some(diff) = w else { continue };
        let bad = diff.count_ones() as usize;
        total_bits += unit_bits;
        wrong_bits += bad;
        total_units += 1;
        if bad > 0 {
            wrong_units += 1;
            if first_error.is_none() && lock_at.is_some() {
                first_error = Some((times[i] - times[from]).max(0.0));
            }
        }
    }
    m.first_error_s = first_error;
    m.ber = if total_bits == 0 { 0.5 } else { wrong_bits as f64 / total_bits as f64 };

    // Symbol error rate: the fraction of consecutive groups of one symbol's
    // worth of aligned bits that carry at least one wrong bit.
    let per_symbol = mode.bits_per_symbol();
    if unit_bits == 1 {
        let mut groups = 0usize;
        let mut bad_groups = 0usize;
        let mut in_group = 0usize;
        let mut group_bad = false;
        for w in wrong.iter().skip(from) {
            let Some(diff) = w else { continue };
            if *diff != 0 {
                group_bad = true;
            }
            in_group += 1;
            if in_group == per_symbol {
                groups += 1;
                if group_bad {
                    bad_groups += 1;
                }
                in_group = 0;
                group_bad = false;
            }
        }
        m.ser = if groups == 0 { 1.0 } else { bad_groups as f64 / groups as f64 };
    } else {
        // Bell 103: the unit is a character, so this is a character error rate.
        m.ser = if total_units == 0 { 1.0 } else { wrong_units as f64 / total_units as f64 };
    }
    m
}

// ---------------------------------------------------------------------------
// The sweep
// ---------------------------------------------------------------------------

const MODES: &[Mode] = &[
    Mode::Bell103,
    Mode::V21,
    Mode::V22,
    Mode::V22bis1200,
    Mode::V22bis2400,
    Mode::V32 { bps: 4800, trellis: false },
    Mode::V32 { bps: 9600, trellis: false },
    Mode::V32 { bps: 7200, trellis: true },
    Mode::V32 { bps: 9600, trellis: true },
    Mode::V32 { bps: 12000, trellis: true },
    Mode::V32 { bps: 14400, trellis: true },
    Mode::V29 { bps: 7200 },
    Mode::V29 { bps: 9600 },
    Mode::V27ter { bps: 2400 },
    Mode::V27ter { bps: 4800 },
];

const SEED: u64 = 0x5107_1A7E_D1A1_0000;
/// The front matter of the document this harness leaves behind: what the
/// columns are, what the line was, and what each figure is being measured
/// against. Kept here rather than in the doc so that the numbers and the
/// explanation of them can never drift apart.
const PREAMBLE: &[&str] = &[
    "Nothing outside `crates/datapump/tests/lock_sweep.rs` was changed to obtain any",
    "number here. Read that file's module comment for the whole of the method; what",
    "follows is enough to read the tables.",
    "",
    "## The columns",
    "",
    "| column | what it is |",
    "|---|---|",
    "| lock | from the far end's carrier first appearing on the line to the first correct bit of a run of 200 with nothing wrong in it |",
    r"| slicer SNR dB | mean \|decision\|² over mean \|received − decision\|², over the steady state, against the alphabet that was actually transmitted rather than the one the receiver believes in. Comparable across rates, which a residual error is not |",
    "| margin | mean distance from a decision, over half the distance between the two closest points. One means the average symbol is sitting on the decision boundary |",
    "| SER | fraction of consecutive groups of one symbol's worth of aligned bits carrying at least one wrong bit. For Bell 103, whose receiver hands out whole characters and no bit stream, a character error rate |",
    "| BER | bit error rate from lock onwards. The acquisition transient is not counted in it: that is what the lock column is for |",
    "| 1st err | from lock to the first wrong bit |",
    "| carrier lost | whether the receiver ever declared the carrier gone after finding it, and for how long |",
    "| slips | how many times the recovered stream jumped relative to what was sent. One is a receiver that lost symbols and carried on; one on an otherwise clean line is a timing loop slipping |",
    "",
    "A mode **held** a case when it locked and its bit error rate from lock was under",
    "one in a thousand. Every cell is the middle of three seeds by bit error rate, so",
    "no single draw of one noise sequence decides a threshold.",
    "",
    "## The line",
    "",
    "In the order `evidence.md` §4.2 fixes: the far end's sampling clock (by",
    "resampling, so this is a real clock offset and not the trick of lying to the",
    "receiver about `fs`), then a carrier offset applied as a true single-sideband",
    "shift through a 255-tap Hilbert transformer so the symbol rate is untouched,",
    "then an echo, then a level step and a dropout, then seeded white Gaussian noise",
    "scaled so its power in a 3.1 kHz band is the stated number of decibels below the",
    "mean power of the arriving signal.",
    "",
    "Three things to keep in mind when reading the echo, clock and round-trip rows.",
    "",
    "- **No echo canceller is used anywhere here.** That matters for V.32 alone: both",
    "  directions share the band and a real V.32 modem cancels its own echo before the",
    "  receiver sees anything (`crates/datapump/src/v32/startup.rs`, exercised by",
    "  `v32_loopback.rs:157`). The V.32 echo rows measure the bare receiver and",
    "  understate what a whole V.32 modem does. Every other mode's echo is either out",
    "  of band or a listener echo, and its rows mean what they say.",
    "- **The echo is a talker echo for the duplex modes and a listener echo for the",
    "  rest.** Bell 103, V.22, V.22 bis and V.32 transmit while they listen, so the",
    "  echo is of this end's own transmitter. V.21 as T.30 uses it, V.29 and V.27 ter",
    "  are half duplex and silent while receiving, so the only echo there can be is",
    "  the far end's own signal off the far hybrid. Note that 0.57 at 120 ms is the",
    "  project's measurement of its *virtual cable's* talker echo; as a listener echo",
    "  it is a reflection at 4.9 dB down and 144 to 288 symbol periods out, which no",
    "  equaliser in this project spans — they reach ±4 to ±17 ms.",
    "- **A clock offset carries a proportional carrier offset with it**, because the",
    "  far end's carrier comes off the same oscillator. At ±200 ppm that is ±0.48 Hz",
    "  on a 2400 Hz carrier and ±0.36 Hz on an 1800 Hz one. That is what ±0.01% means",
    "  and it is not contamination.",
    "",
    "## What the Recommendations ask for",
    "",
    "Each read from the rendered page, never from `docs/specs/text`, which loses",
    "signs and columns.",
    "",
    "| mode | carrier offset the receiver must take | clock |",
    "|---|---|---|",
    "| V.21 §3 (Fascicle VIII.1, p. 2) | \"the demodulation equipment must tolerate drifts of ± 12 Hz between the frequencies received and their nominal values\" | — |",
    "| Bell 103 | not an ITU Recommendation. Its shift is ±100 Hz, which makes a few hertz of offset negligible by construction | — |",
    "| V.22 2.6 (Fascicle VIII.1, p. 3) | \"the receiver shall be able to accept errors of at least ± 7 Hz in the received frequencies\" | 2.5.1: 1200 bit/s ± 0.01%, 600 baud ± 0.01% |",
    "| V.22 bis 2.6 (Fascicle VIII.1, p. 4) | \"The receiver shall be able to operate with received frequency offsets of up to ± 7 Hz.\" | 2.5.1: 1200/2400 bit/s ± 0.01%, 600 baud ± 0.01% |",
    "| V.32 2.1 (Rec. V.32 (03/93), p. 1) | \"The carrier frequency is to be 1800 ± 1 Hz … The receiver must be able to operate with received frequency offsets of up to ± 7 Hz.\" | 2.3: 2400 bauds ± 0.01% |",
    "| V.32 bis 2.1 (Rec. V.32 bis, p. 1) | \"The receiver must be able to operate with a maximum received frequency offset of up to ± 7 Hz.\" | 2.1: 2400 symbols/s ± 0.01% |",
    "| V.29 §4 (Fascicle VIII.1, p. 4) | \"the receiver must be able to accept errors of at least ± 7 Hz in the received signal frequency\" | §3: 2400 bauds ± 0.01% |",
    "| V.27 ter §3 (Fascicle VIII.1, p. 5) | \"the receiver must be able to accept errors of at least ± 7 Hz in the received frequencies\" | 2.3.2: 1600/1200 bauds ± 0.01% |",
    "",
    "Two conforming ends may each be 0.01% out, so ±200 ppm is the worst two legal",
    "modems can be apart. That is why the clock sweep stops there.",
    "",
    "V.22 and V.22 bis at 1200 bit/s put the same waveform on the line — V.22 bis",
    "2.5.2.2 nominates the one point V.22 uses \"irrespective of the quadrant",
    "concerned … This ensure compatibility with Recommendation V.22\" — and this",
    "project has one receiver for both. The only thing separating the two rows below",
    "is whether the receiver was told the rate or left to work it out.",
    "",
];


/// The eight arrival phases: eight samples at 16 kHz covers more than a whole
/// symbol at V.32's 6.67 samples per symbol, which is where this matters.
const PHASES: usize = 8;

fn cases() -> Vec<(String, Impairments)> {
    let mut out: Vec<(String, Impairments)> = Vec::new();
    out.push(("clean".into(), Impairments::default()));
    for hz in [-7.0, -3.0, -1.0, 1.0, 3.0, 7.0] {
        out.push((format!("carrier {hz:+.0} Hz"), Impairments { carrier_hz: hz, ..Default::default() }));
    }
    for ppm in [-200.0, -120.0, -50.0, 50.0, 120.0, 200.0] {
        out.push((format!("clock {ppm:+.0} ppm"), Impairments { clock_ppm: ppm, ..Default::default() }));
    }
    for db in [36.0, 33.0, 30.0, 27.0, 24.0, 21.0, 18.0, 15.0, 12.0, 9.0, 6.0] {
        out.push((format!("SNR {db:.0} dB"), Impairments { snr_db: Some(db), ..Default::default() }));
    }
    out.push((
        "echo 0.57 @ 120 ms".into(),
        Impairments { echo: Some(Echo { delay_s: 0.120, amplitude: 0.57 }), ..Default::default() },
    ));
    out.push((
        "echo -25 dB @ 1.1 s".into(),
        Impairments {
            echo: Some(Echo { delay_s: 1.100, amplitude: 10f64.powf(-25.0 / 20.0) }),
            ..Default::default()
        },
    ));
    out.push(("level +6 dB".into(), Impairments { step_db: Some(6.0), ..Default::default() }));
    out.push(("level -6 dB".into(), Impairments { step_db: Some(-6.0), ..Default::default() }));
    out.push(("dropout 20 ms".into(), Impairments { dropout_s: Some(0.020), ..Default::default() }));
    out.push(("round trip 1.1 s".into(), Impairments { delay_s: 1.100, ..Default::default() }));
    out
}

fn show_time(t: Option<f64>) -> String {
    match t {
        Some(s) => format!("{:.0} ms", s * 1000.0),
        None => "never".into(),
    }
}

fn show_db(v: f64) -> String {
    if v.is_infinite() && v > 0.0 {
        "exact".into()
    } else if v.is_infinite() {
        "-".into()
    } else {
        format!("{v:.1}")
    }
}

fn show_rate(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else if v < 1e-4 {
        format!("{v:.1e}")
    } else {
        format!("{v:.4}")
    }
}

/// Measure everything, print the table, and leave it in
/// `docs/design/slow-modes/before.md`.
#[test]
#[ignore = "a measurement harness, not a check: minutes, and only meaningful in release"]
fn every_slow_mode_against_every_impairment() {
    let cases = cases();
    let mut doc = String::new();
    let mut summary: Vec<(String, usize, Option<f64>, Vec<String>)> = Vec::new();

    writeln!(doc, "# The slower modes, before anything is changed").unwrap();
    writeln!(doc).unwrap();
    writeln!(
        doc,
        "Produced by `crates/datapump/tests/lock_sweep.rs`, which is `#[ignore]`d:"
    )
    .unwrap();
    writeln!(doc).unwrap();
    writeln!(
        doc,
        "```text\ncargo test -p datapump --release --test lock_sweep -- --ignored --nocapture\n```"
    )
    .unwrap();
    writeln!(doc).unwrap();
    for line in PREAMBLE {
        writeln!(doc, "{line}").unwrap();
    }

    for &mode in MODES {
        let label = mode.label();
        eprintln!("\n=== {label} ===");

        // The arrival phase first: it is the one variable the older receivers
        // are silently sensitive to, and a table measured at one unlucky phase
        // would say nothing about anything else.
        let mut phase_ok = Vec::new();
        let mut best_phase = 0usize;
        let mut best_ber = f64::INFINITY;
        for phase in 0..PHASES {
            let m = run(mode, Impairments::default(), phase, SEED);
            if m.held() {
                phase_ok.push(phase);
            }
            if m.ber < best_ber {
                best_ber = m.ber;
                best_phase = phase;
            }
        }
        eprintln!(
            "arrival phases that carried the payload: {}/{PHASES} {:?}",
            phase_ok.len(),
            phase_ok
        );

        writeln!(doc, "## {label}").unwrap();
        writeln!(doc).unwrap();
        writeln!(
            doc,
            "{:.0} baud, {} bit{} to the symbol.",
            mode.baud(),
            mode.bits_per_symbol(),
            if mode.bits_per_symbol() == 1 { "" } else { "s" }
        )
        .unwrap();
        writeln!(doc).unwrap();
        writeln!(
            doc,
            "Arrival phase, clean line: **{}/{PHASES}** of the sample phases \
             carried the payload ({:?} worked). Everything below was run at \
             phase {best_phase}, the best of the eight.",
            phase_ok.len(),
            phase_ok
        )
        .unwrap();
        writeln!(doc).unwrap();
        writeln!(
            doc,
            "| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |"
        )
        .unwrap();
        writeln!(doc, "|---|---|---|---|---|---|---|---|---|").unwrap();

        let mut failures: Vec<String> = Vec::new();
        let mut snr_floor: Option<f64> = None;
        let mut snr_still_holding = true;
        let mut rate_flip = false;

        for (name, imp) in &cases {
            // Three seeds, and the middle one by bit error rate. One draw of
            // one noise sequence decides a signal-to-noise threshold far too
            // often to be reported as if it were the receiver's.
            let mut three: Vec<Metrics> = (0..3)
                .map(|k| run(mode, *imp, best_phase, SEED ^ (k * 0x9E37_79B9)))
                .collect();
            three.sort_by(|a, b| a.ber.partial_cmp(&b.ber).unwrap());
            let m = three[1].clone();
            let want = match mode {
                Mode::V22bis2400 => 2400,
                Mode::V22 | Mode::V22bis1200 => 1200,
                _ => 0,
            };
            if want != 0 && m.believed_rate.is_some_and(|r| r != want) {
                rate_flip = true;
            }
            // The cases run from the quietest line down, so the floor is the
            // last one that held before the first that did not. Taking the
            // smallest number that held anywhere would let one lucky draw
            // three decibels below the real edge report itself as the edge.
            if name.starts_with("SNR") {
                let db: f64 = name[4..name.len() - 3].trim().parse().unwrap_or(99.0);
                if m.held() && snr_still_holding {
                    snr_floor = Some(db);
                } else {
                    snr_still_holding = false;
                }
            }
            if !m.held() && !name.starts_with("SNR") {
                failures.push(format!("{name} (BER {:.3})", m.ber));
            }
            writeln!(
                doc,
                "| {} | {} | {} | {:.2} | {} | {} | {} | {} | {} |",
                name,
                show_time(m.lock_s),
                show_db(m.slicer_snr_db),
                m.margin,
                show_rate(m.ser),
                show_rate(m.ber),
                show_time(m.first_error_s),
                if m.carrier_lost {
                    format!("yes, {:.0} ms", m.carrier_down_s * 1000.0)
                } else {
                    "no".into()
                },
                m.slips
            )
            .unwrap();
            eprintln!(
                "  {name:<20} lock {:<8} snr {:>6} margin {:>5.2} ser {:>8} ber {:>8}",
                show_time(m.lock_s),
                show_db(m.slicer_snr_db),
                m.margin,
                show_rate(m.ser),
                show_rate(m.ber)
            );
        }
        writeln!(doc).unwrap();
        match snr_floor {
            Some(db) => {
                writeln!(doc, "Lowest signal-to-noise ratio that still carried the payload: **{db:.0} dB**.").unwrap();
            }
            None => {
                writeln!(doc, "No signal-to-noise ratio in the sweep carried the payload.").unwrap();
            }
        }
        if rate_flip {
            writeln!(doc, "The receiver's own rate detector disagreed with the rate in use in at least one case.").unwrap();
        }
        writeln!(doc).unwrap();

        summary.push((label, phase_ok.len(), snr_floor, failures));
    }

    writeln!(doc, "## Where it stands, worst first").unwrap();
    writeln!(doc).unwrap();
    writeln!(
        doc,
        "Ranked by how much of the sweep the mode did not survive: two points          for every arrival phase that carried nothing, one for every impairment          case that failed."
    )
    .unwrap();
    writeln!(doc).unwrap();
    writeln!(doc, "| mode | arrival phases | SNR floor | failed | what failed |").unwrap();
    writeln!(doc, "|---|---|---|---|---|").unwrap();
    let mut ordered = summary.clone();
    ordered.sort_by_key(|(_, ok, _, f)| std::cmp::Reverse((PHASES - ok) * 2 + f.len()));
    for (label, ok, floor, what) in &ordered {
        writeln!(
            doc,
            "| {label} | {ok}/{PHASES} | {} | {} | {} |",
            match floor {
                Some(db) => format!("{db:.0} dB"),
                None => "none held".into(),
            },
            what.len(),
            if what.is_empty() { "nothing".into() } else { what.join(", ") }
        )
        .unwrap();
    }

    // Two of the failures above are worth pinning down rather than leaving as a
    // cell in a table, because each is a threshold and the threshold is the
    // useful number.
    writeln!(doc).unwrap();
    writeln!(doc, "## Two of those, pinned down").unwrap();
    writeln!(doc).unwrap();
    writeln!(
        doc,
        "### What does a silence before the carrier do to V.32?\n\n\
         Every V.32 row's `round trip 1.1 s` failure is this. The far end's \
         signal is unchanged; the only difference is how long the receiver \
         listened to nothing first. V.22 bis, which is structurally the same \
         receiver, is run beside it as the control.\n\n\
         It is not a clean threshold — the silence and the arrival phase \
         interact, which is the §3.1 fault of `evidence.md` showing through — \
         but the effect is unmistakable and it is the only impairment in this \
         whole sweep that breaks V.32 at 4800, which is otherwise eight \
         phases out of eight on everything."
    )
    .unwrap();
    writeln!(doc).unwrap();
    writeln!(
        doc,
        "| silence before the carrier | V.32 4800 phases held | V.32 4800 BER at phase 0 | V.32 4800 lock | V.22bis 2400 phases held |"
    )
    .unwrap();
    writeln!(doc, "|---|---|---|---|---|").unwrap();
    for &delay in &[0.0f64, 0.05, 0.10, 0.15, 0.20, 0.25, 0.30, 0.40, 0.60, 1.10] {
        let imp = Impairments { delay_s: delay, ..Default::default() };
        let v32 = Mode::V32 { bps: 4800, trellis: false };
        let a = run(v32, imp, 0, SEED);
        let ok32 = (0..PHASES).filter(|&q| run(v32, imp, q, SEED).held()).count();
        let ok22 = (0..PHASES).filter(|&q| run(Mode::V22bis2400, imp, q, SEED).held()).count();
        writeln!(
            doc,
            "| {:.0} ms | {ok32}/{PHASES} | {} | {} | {ok22}/{PHASES} |",
            delay * 1000.0,
            show_rate(a.ber),
            show_time(a.lock_s)
        )
        .unwrap();
    }
    writeln!(doc).unwrap();
    writeln!(doc).unwrap();
    writeln!(
        doc,
        "### Is a carrier or clock failure the offset, or the arrival phase again?\n\n\
         Above 9600 the clean line already only carries the payload at some of \
         the eight arrival phases, so a single-phase row cannot tell an \
         impairment the receiver genuinely cannot take from the same lottery \
         re-rolled. Each cell below is how many of the eight phases carried \
         the payload with that impairment applied, so a column that falls to \
         0/8 everywhere is the impairment and one that merely wobbles is the \
         phase."
    )
    .unwrap();
    writeln!(doc).unwrap();
    let axes: [(&str, Impairments); 9] = [
        ("clean", Impairments::default()),
        ("-7 Hz", Impairments { carrier_hz: -7.0, ..Default::default() }),
        ("-3 Hz", Impairments { carrier_hz: -3.0, ..Default::default() }),
        ("-1 Hz", Impairments { carrier_hz: -1.0, ..Default::default() }),
        ("+1 Hz", Impairments { carrier_hz: 1.0, ..Default::default() }),
        ("+3 Hz", Impairments { carrier_hz: 3.0, ..Default::default() }),
        ("+7 Hz", Impairments { carrier_hz: 7.0, ..Default::default() }),
        ("-200 ppm", Impairments { clock_ppm: -200.0, ..Default::default() }),
        ("+200 ppm", Impairments { clock_ppm: 200.0, ..Default::default() }),
    ];
    write!(doc, "| mode |").unwrap();
    for (name, _) in &axes {
        write!(doc, " {name} |").unwrap();
    }
    writeln!(doc).unwrap();
    writeln!(doc, "|---|---|---|---|---|---|---|---|---|---|").unwrap();
    for &mode in MODES {
        if matches!(mode, Mode::Bell103 | Mode::V21 | Mode::V22bis1200) {
            continue;
        }
        write!(doc, "| {} |", mode.label()).unwrap();
        for (_, imp) in &axes {
            let ok = (0..PHASES).filter(|&q| run(mode, *imp, q, SEED).held()).count();
            write!(doc, " {ok}/{PHASES} |").unwrap();
        }
        writeln!(doc).unwrap();
    }
    writeln!(doc).unwrap();

    writeln!(
        doc,
        "### How long a hole can each mode take?\n\n\
         `dropout 20 ms` is the one case every mode in the sweep failed. It is \
         not one failure but two. Below about 10 ms nothing declares the \
         carrier gone and the cost is proportional to the hole. At 20 ms the \
         two fax modes' carrier detectors correctly drop — and dropping it is \
         what ends the burst, because a half-duplex receiver has one training \
         sequence in front of it and no way back to it. V.22 bis and V.32 \
         never drop the carrier at all, at any length of hole, so nothing \
         above them is ever told."
    )
    .unwrap();
    writeln!(doc).unwrap();
    write!(doc, "| mode |").unwrap();
    for &hole in &[0.002f64, 0.005, 0.010, 0.020, 0.050] {
        write!(doc, " {:.0} ms |", hole * 1000.0).unwrap();
    }
    writeln!(doc, "\n|---|---|---|---|---|---|").unwrap();
    for &mode in &[
        Mode::V27ter { bps: 4800 },
        Mode::V29 { bps: 9600 },
        Mode::V22bis2400,
        Mode::V32 { bps: 4800, trellis: false },
    ] {
        write!(doc, "| {} |", mode.label()).unwrap();
        for &hole in &[0.002f64, 0.005, 0.010, 0.020, 0.050] {
            let m = run(mode, Impairments { dropout_s: Some(hole), ..Default::default() }, 0, SEED);
            write!(
                doc,
                " BER {}, carrier {} |",
                show_rate(m.ber),
                if m.carrier_lost { "lost" } else { "held" }
            )
            .unwrap();
        }
        writeln!(doc).unwrap();
    }
    writeln!(doc).unwrap();

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/design/slow-modes/before.md");
    std::fs::write(path, &doc).expect("could not write before.md");
    eprintln!("\nwrote {path}");
    println!("{doc}");
}

/// One mode, one case, for working on the harness itself without waiting for
/// the whole sweep.
#[test]
#[ignore = "a probe for developing the harness"]
fn one_mode_clean() {
    for &mode in MODES {
        let mut ok = 0;
        for phase in 0..PHASES {
            let m = run(mode, Impairments::default(), phase, SEED);
            if m.held() {
                ok += 1;
            }
            eprintln!(
                "{:<16} phase {phase}  lock {:<8} snr {:>6} ser {:>9} ber {:>9} units {}",
                mode.label(),
                show_time(m.lock_s),
                show_db(m.slicer_snr_db),
                show_rate(m.ser),
                show_rate(m.ber),
                m.units
            );
        }
        eprintln!("{:<16} {ok}/{PHASES}\n", mode.label());
    }
}

/// Two things the sweep turns up that are worth looking at on their own.
#[test]
#[ignore = "a probe for developing the harness"]
fn two_things_worth_a_closer_look() {
    // How long a silence in front of the carrier does V.32 need before it stops
    // acquiring? `v32.rs:1073` gates the equaliser on `self.symbols > 64`
    // counted from construction rather than from the carrier, which V.22 bis
    // fixed in `v22bis.rs:646`; if that is the mechanism, a short silence works
    // and a long one does not, with nothing else changed.
    for &delay in &[0.0f64, 0.05, 0.10, 0.15, 0.20, 0.25, 0.30, 0.40, 0.60, 1.10] {
        let imp = Impairments { delay_s: delay, ..Default::default() };
        let a = run(Mode::V32 { bps: 4800, trellis: false }, imp, 0, SEED);
        let b = run(Mode::V22bis2400, imp, 0, SEED);
        eprintln!(
            "silence {:>5.0} ms   V.32 4800 ber {:>8}  lock {:<8}   V.22bis 2400 ber {:>8}  lock {}",
            delay * 1000.0,
            show_rate(a.ber),
            show_time(a.lock_s),
            show_rate(b.ber),
            show_time(b.lock_s)
        );
    }

    // A fax burst is half duplex and has one training sequence in front of it
    // and no way back to it. How long a hole does it take to end one?
    eprintln!();
    for &mode in &[
        Mode::V27ter { bps: 4800 },
        Mode::V29 { bps: 9600 },
        Mode::V22bis2400,
        Mode::V32 { bps: 4800, trellis: false },
    ] {
        let mut line = format!("{:<14}", mode.label());
        for &hole in &[0.002f64, 0.005, 0.010, 0.020, 0.050] {
            let m = run(mode, Impairments { dropout_s: Some(hole), ..Default::default() }, 0, SEED);
            line.push_str(&format!(
                "  {:>3.0} ms: ber {:>8} slips {} carrier {}",
                hole * 1000.0,
                show_rate(m.ber),
                m.slips,
                if m.carrier_lost { "lost" } else { "held" }
            ));
        }
        eprintln!("{line}");
    }

    eprintln!();
    for &mode in &[
        Mode::V32 { bps: 4800, trellis: false },
        Mode::V32 { bps: 9600, trellis: false },
        Mode::V32 { bps: 14400, trellis: true },
        Mode::V22bis2400,
    ] {
        let mut line = format!("{:<16}", mode.label());
        for &d in &[0.0f64, 0.3, 1.1] {
            let imp = Impairments { delay_s: d, ..Default::default() };
            let ok = (0..PHASES).filter(|&q| run(mode, imp, q, SEED).held()).count();
            line.push_str(&format!("  silence {:>4.0} ms: {ok}/{PHASES}", d * 1000.0));
        }
        eprintln!("{line}");
    }

    // Above 9600 the clean line already only works at some arrival phases. Is a
    // carrier-offset failure the offset, or the same lottery re-rolled? Run the
    // offset at every phase and count.
    eprintln!();
    for &mode in &[
        Mode::V22bis2400,
        Mode::V32 { bps: 9600, trellis: false },
        Mode::V32 { bps: 9600, trellis: true },
        Mode::V32 { bps: 12000, trellis: true },
        Mode::V32 { bps: 14400, trellis: true },
    ] {
        let mut line = format!("{:<16}", mode.label());
        for &hz in &[0.0f64, 1.0, 3.0, 7.0, -7.0] {
            let imp = Impairments { carrier_hz: hz, ..Default::default() };
            let ok = (0..PHASES).filter(|&q| run(mode, imp, q, SEED).held()).count();
            line.push_str(&format!("  {hz:+.0} Hz: {ok}/{PHASES}"));
        }
        eprintln!("{line}");
    }

    // Where do V.21's two slips fall, and what do they cost?
    eprintln!();
    let mode = Mode::V21;
    let (lead_in_s, payload_s) = mode.plan();
    let lead_in = (lead_in_s * FS) as usize;
    let (far, sent) = transmit(mode, ((lead_in_s + payload_s + 0.15) * FS) as usize, lead_in, SEED);
    let mut rx = Rx::new(mode);
    let mut step = Step::default();
    let mut got = Vec::new();
    let mut times = Vec::new();
    for (n, &x) in far.iter().enumerate() {
        rx.feed(x, &mut step);
        for &u in &step.units {
            got.push(u);
            times.push(n as f64 / FS);
        }
    }
    // The receiver drops nothing at the front, so lag zero is the truth until
    // something slips; walk it and say where it stops being.
    let mut lag: isize = 0;
    let mut reported = 0;
    for i in 0..got.len() {
        let j = i as isize + lag;
        if j < 0 || j as usize >= sent.len() {
            break;
        }
        if got[i] != u16::from(sent[j as usize]) {
            // Try a one-bit slip either way over the next 64 bits.
            let score = |l: isize| -> usize {
                (0..64)
                    .filter(|&k| {
                        let jj = i as isize + k + l;
                        jj >= 0
                            && (jj as usize) < sent.len()
                            && i + (k as usize) < got.len()
                            && got[i + (k as usize)] == u16::from(sent[jj as usize])
                    })
                    .count()
            };
            let best = [-2isize, -1, 0, 1, 2].into_iter().max_by_key(|&l| score(l)).unwrap();
            eprintln!(
                "V.21 mismatch at bit {i} ({:.0} ms), lag {lag} -> {}, agreement over the next \
                 64 bits {}/64",
                times[i] * 1000.0,
                lag + best,
                score(best)
            );
            lag += best;
            reported += 1;
            if reported > 8 {
                break;
            }
        }
    }
    eprintln!("V.21 produced {} bits for {} sent", got.len(), sent.len());
    let m = score(mode, &sent, &got, &times, 0.0);
    eprintln!("score(): {m:?}");
}
