//! The shared QAM core, `dsp::qam`, proved on a modulator of its own.
//!
//! Everything here is at V.32's band: 2400 baud on 1800 Hz, sampled at
//! 16 kHz. The far end is a root-raised-cosine modulator of a quarter's
//! roll-off, V.32's usual, sending what V.32 sends before data (V.32 5.2):
//! S, S-bar, and a 1280-symbol TRN whose first 256 symbols are two points and
//! the rest four -- made from a PRBS rather than V.32's scrambler, since the
//! core is told the sequence and does not care what it is. Data follows on
//! one of four tables: V.32's four states, sixteen points, and the 32- and
//! 128-point crosses, turned 45 degrees as V.32's are.
//!
//! Signal to noise is Es/N0 as design.md 9.1 has it: white Gaussian noise at
//! 16 kHz of variance (fs/2)/baud x P x 10^(-SNR/10), P the far signal's
//! power as received.
//!
//! The figures each test measures are printed; `cargo test --release --test
//! qam_core -- --nocapture` shows them.

use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

use dsp::qam::{Band, Constellation, Core, Heard, Options, Slicer, Stage, Training, Via, Window};
use dsp::{Complex, rrc_at};

const FS: f64 = 16_000.0;
const BAUD: f64 = 2400.0;
const CARRIER: f64 = 1800.0;
const ROLLOFF: f64 = 0.25;

/// The far end's level: 20 dB under a unit-amplitude carrier.
const AMPLITUDE: f64 = 0.1;

/// Symbols of silence before S, and the lengths of S, S-bar and TRN.
const SILENCE: usize = 200;
const S_LENGTH: usize = 256;
const S_BAR_LENGTH: usize = 16;
const TRN_LENGTH: usize = 1280;

fn band() -> Band {
    Band::new(FS, BAUD, CARRIER)
}

// ---------------------------------------------------------------------------
// Random numbers.

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

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn bit(&mut self) -> bool {
        self.next_u64() >> 63 == 1
    }

    fn gaussian(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2.0 * a.ln()).sqrt() * (TAU * b).cos()
    }
}

// ---------------------------------------------------------------------------
// Constellations, at unit mean power.

fn unit(points: Vec<Complex>) -> Constellation {
    let power = points.iter().map(|p| p.norm_sqr()).sum::<f64>() / points.len() as f64;
    Constellation::new(points.into_iter().map(|p| p.scale(1.0 / power.sqrt())).collect())
}

/// V.32's synchronising states A, B, C and D (spec.md 2.1): turned 26.57
/// degrees from the axes.
fn states() -> [Complex; 4] {
    let r = 10f64.sqrt();
    [Complex::new(-3.0 / r, -1.0 / r), Complex::new(1.0 / r, -3.0 / r), Complex::new(3.0 / r, 1.0 / r), Complex::new(-1.0 / r, 3.0 / r)]
}

fn four() -> Constellation {
    Constellation::new(states().to_vec())
}

fn sixteen() -> Constellation {
    let odd = [-3.0, -1.0, 1.0, 3.0];
    unit(odd.iter().flat_map(|&x| odd.iter().map(move |&y| Complex::new(x, y))).collect())
}

/// The odd grid out to `reach`, less its corners beyond `corner`, turned 45
/// degrees: in V.32's figures the crosses are the points whose coordinates
/// add up to an odd number (core.md 7.1).
fn cross(reach: i32, corner: i32) -> Constellation {
    let turn = Complex::from_polar(1.0, PI / 4.0);
    let mut points = Vec::new();
    for x in (-reach..=reach).step_by(2) {
        for y in (-reach..=reach).step_by(2) {
            if x.abs() > corner && y.abs() > corner {
                continue;
            }
            points.push(Complex::new(f64::from(x), f64::from(y)) * turn);
        }
    }
    unit(points)
}

fn cross32() -> Constellation {
    cross(5, 3)
}

fn cross128() -> Constellation {
    cross(11, 7)
}

fn tables() -> Vec<(&'static str, Constellation)> {
    vec![("4", four()), ("16", sixteen()), ("32", cross32()), ("128", cross128())]
}

// ---------------------------------------------------------------------------
// The far end.

/// Symbols either side the pulse reaches, and its table's steps a symbol.
const SPAN: i64 = 16;
const STEPS: usize = 1024;

/// The root-raised-cosine pulse, tapered by a Hann window across its span.
fn pulse() -> &'static [f64] {
    static PULSE: OnceLock<Vec<f64>> = OnceLock::new();
    PULSE.get_or_init(|| {
        (0..=2 * SPAN as usize * STEPS)
            .map(|i| {
                let t = i as f64 / STEPS as f64 - SPAN as f64;
                rrc_at(t, ROLLOFF) * (0.5 + 0.5 * (PI * t / SPAN as f64).cos())
            })
            .collect()
    })
}

fn pulse_at(t: f64) -> f64 {
    let x = (t + SPAN as f64) * STEPS as f64;
    if x < 0.0 || x >= (2 * SPAN as usize * STEPS) as f64 {
        return 0.0;
    }
    let (i, frac) = (x.floor() as usize, x - x.floor());
    let table = pulse();
    table[i] * (1.0 - frac) + table[i + 1] * frac
}

/// The pulse's energy a symbol: the complex envelope's mean power for
/// symbols at unit power.
fn pulse_energy() -> f64 {
    pulse().iter().map(|p| p * p).sum::<f64>() / STEPS as f64
}

/// The far signal's power on the line.
fn signal_power() -> f64 {
    AMPLITUDE * AMPLITUDE * pulse_energy() / 2.0
}

/// How the far end's clocks and carrier differ from ours.
#[derive(Debug, Clone, Copy)]
struct Far {
    /// Its clock against ours: symbols and carrier both.
    clock: f64,
    /// Its carrier's offset, in hertz.
    offset_hz: f64,
    /// Samples before its first symbol.
    delay: f64,
    /// From this sample on, its symbols this many symbols later, its carrier
    /// not moved at all: a pure timing jump.
    jump: Option<(usize, f64)>,
}

impl Default for Far {
    fn default() -> Self {
        Self { clock: 1.0, offset_hz: 0.0, delay: 0.0, jump: None }
    }
}

fn line_sample(symbols: &[Complex], far: Far, n: usize) -> f64 {
    let jumped = match far.jump {
        Some((at, d)) if n >= at => d,
        _ => 0.0,
    };
    let t = (n as f64 - far.delay) * BAUD * far.clock / FS - jumped;
    let low = ((t - SPAN as f64).ceil().max(0.0)) as usize;
    let high = (t + SPAN as f64).floor();
    if high < 0.0 {
        return 0.0;
    }
    let high = (high as usize).min(symbols.len().saturating_sub(1));
    let mut b = Complex::ZERO;
    for (k, s) in symbols.iter().enumerate().take(high + 1).skip(low) {
        b += s.scale(pulse_at(t - k as f64));
    }
    let turns = ((CARRIER + far.offset_hz) * far.clock * n as f64 / FS).fract();
    let (sin, cos) = (TAU * turns).sin_cos();
    AMPLITUDE * (b.re * cos - b.im * sin)
}

fn modulate(symbols: &[Complex], far: Far) -> Vec<f64> {
    let per = FS / (BAUD * far.clock);
    let length = ((symbols.len() as f64 + SPAN as f64) * per + far.delay) as usize;
    (0..length).map(|n| line_sample(symbols, far, n)).collect()
}

/// The noise's standard deviation for `snr_db` of Es/N0.
fn noise_sigma(snr_db: f64) -> f64 {
    ((FS / 2.0) / BAUD * signal_power() * 10f64.powf(-snr_db / 10.0)).sqrt()
}

fn add_noise(samples: &mut [f64], snr_db: f64, seed: u64) {
    if snr_db.is_infinite() {
        return;
    }
    let sigma = noise_sigma(snr_db);
    let mut random = Random::new(seed);
    for x in samples {
        *x += sigma * random.gaussian();
    }
}

/// What a V.32 far end sends before its data, S to TRN, and then `data`;
/// with where things are.
struct Sent {
    symbols: Vec<Complex>,
    trn: Vec<Complex>,
    s_bar: usize,
    data: usize,
}

fn startup(data: &[Complex], seed: u64) -> Sent {
    let [a, b, c, d] = states();
    let mut random = Random::new(seed ^ 0x5eed);
    let mut symbols = vec![Complex::ZERO; SILENCE];
    symbols.extend((0..S_LENGTH).map(|n| if n % 2 == 0 { a } else { b }));
    let s_bar = symbols.len();
    symbols.extend((0..S_BAR_LENGTH).map(|n| if n % 2 == 0 { c } else { d }));
    let trn: Vec<Complex> = (0..TRN_LENGTH)
        .map(|n| {
            let (first, second) = (random.bit(), random.bit());
            if n < 256 {
                if first { c } else { a }
            } else {
                match (first, second) {
                    (false, false) => a,
                    (false, true) => b,
                    (true, true) => c,
                    (true, false) => d,
                }
            }
        })
        .collect();
    symbols.extend(&trn);
    let data_start = symbols.len();
    symbols.extend_from_slice(data);
    symbols.extend(std::iter::repeat_n(Complex::ZERO, 40));
    Sent { symbols, trn, s_bar, data: data_start }
}

fn random_data(table: &Constellation, n: usize, seed: u64) -> Vec<Complex> {
    let mut random = Random::new(seed);
    (0..n).map(|_| table.points()[random.below(table.len())]).collect()
}

/// The half-symbol sample the receiver will have centred on symbol `k`,
/// before any timing correction: its halves are at 64 + n x fs/baud/2.
fn half_of_symbol(k: usize, far: Far) -> f64 {
    let centre = far.delay + k as f64 * FS / (BAUD * far.clock);
    (centre - 64.0) / (FS / BAUD / 2.0)
}

// ---------------------------------------------------------------------------
// The near end: a driver of the core, as V.32's will be, but knowing only
// the start-up above.

#[derive(Debug, Clone, Copy)]
struct Record {
    /// Samples fed when the symbol was settled.
    at: usize,
    error: f64,
    lost: bool,
}

#[derive(Debug, Clone)]
struct Listener {
    core: Core,
    trn: Vec<Complex>,
    four: Slicer,
    data: Slicer,
    /// Whether training is told the carrier's turn and the clock's drift as S had them.
    from_s: bool,
    fed: usize,
    s_heard: usize,
    reversal: Option<(u64, f64, f64)>,
    trained: Option<(f64, Via)>,
    untrained: bool,
    /// Which symbol of TRN, counting on into data, the next one made is, and
    /// whether the data's constellation has been switched to.
    index: Option<usize>,
    switched: bool,
    records: Vec<Record>,
}

impl Listener {
    fn new(options: Options, trn: Vec<Complex>, data: Constellation) -> Self {
        let four = Slicer::table(four());
        let mut core = Core::new(band(), options, four.clone());
        core.hunt();
        Self {
            core,
            trn,
            four,
            data: Slicer::table(data),
            from_s: true,
            fed: 0,
            s_heard: 0,
            reversal: None,
            trained: None,
            untrained: false,
            index: None,
            switched: false,
            records: Vec::new(),
        }
    }

    fn training(&self, at: u64, turn: f64, drift: f64) -> Training {
        Training {
            targets: self.trn.clone(),
            start: at + 2 * S_BAR_LENGTH as u64,
            first: Window { align: (16, 256), solve: (16, 512), search: 8 },
            retry: Some(Window { align: (640, 1152), solve: (640, 1152), search: 200 }),
            turn: self.from_s.then_some(turn),
            drift: self.from_s.then_some(drift),
            accept_db: 12.0,
            slicer: self.four.clone(),
            fallback: true,
        }
    }

    fn feed(&mut self, x: f64) {
        self.core.feed(x);
        self.fed += 1;
        while let Some(heard) = self.core.heard() {
            match heard {
                Heard::S => self.s_heard += 1,
                Heard::Reversal { at, turn, drift } => {
                    self.reversal = Some((at, turn, drift));
                    let training = self.training(at, turn, drift);
                    self.core.train(training);
                }
                Heard::Trained { snr_db, via } => {
                    self.trained = Some((snr_db, via));
                    self.index = match via {
                        Via::First => Some(512),
                        Via::Retry => Some(1152),
                        Via::Fallback | Via::Blind => None,
                    };
                }
                Heard::Untrained => self.untrained = true,
                Heard::Lapsed { .. } => {}
            }
        }
        loop {
            if !self.switched && self.index.is_some_and(|i| i >= TRN_LENGTH) {
                self.core.set_slicer(self.data.clone());
                self.switched = true;
            }
            let Some(point) = self.core.next() else { break };
            self.core.settle(point.nearest);
            if let Some(index) = &mut self.index {
                *index += 1;
            }
            self.records.push(Record { at: self.fed, error: (point.z - point.nearest).norm_sqr(), lost: self.core.is_lost() });
        }
    }

    fn feed_all(&mut self, samples: &[f64]) {
        for &x in samples {
            self.feed(x);
        }
    }

    fn trained_db(&self) -> f64 {
        self.trained.map_or(f64::NEG_INFINITY, |t| t.0)
    }

    /// Signal to noise over the records fed between samples `from` and `to`.
    fn snr_between(&self, from: usize, to: usize) -> f64 {
        let chosen: Vec<f64> = self.records.iter().filter(|r| r.at >= from && r.at < to).map(|r| r.error).collect();
        if chosen.is_empty() {
            return f64::NEG_INFINITY;
        }
        -10.0 * (chosen.iter().sum::<f64>() / chosen.len() as f64).log10()
    }
}

/// The sample symbol `k` is centred on.
fn sample_of(k: usize, far: Far) -> usize {
    (far.delay + k as f64 * FS / (BAUD * far.clock)) as usize
}

/// A whole start-up and `data_symbols` of data on `table`, heard through a
/// line `far` with `snr_db` of noise: the listener as it ends, and what was
/// sent.
fn call(options: Options, table: &Constellation, far: Far, snr_db: f64, data_symbols: usize, seed: u64) -> (Listener, Sent) {
    let data = random_data(table, data_symbols, seed);
    let sent = startup(&data, seed);
    let mut samples = modulate(&sent.symbols, far);
    add_noise(&mut samples, snr_db, seed.wrapping_add(7));
    let mut listener = Listener::new(options, sent.trn.clone(), table.clone());
    listener.feed_all(&samples);
    (listener, sent)
}

/// Signal to noise over the last stretch of data: from `skip` symbols into
/// it to 60 symbols before its end.
fn tracked_db(listener: &Listener, sent: &Sent, far: Far, skip: usize) -> f64 {
    let end = sent.symbols.len() - 40;
    // A symbol is settled about nine symbols after its centre goes by.
    let lag = 10;
    listener.snr_between(sample_of(sent.data + skip + lag, far), sample_of(end - 60 + lag, far))
}

/// A listener that has trained and gone `into` symbols into data, sharable
/// by every variant of what comes next; and the whole line it was taken from.
struct Prefix {
    listener: Listener,
    samples: Vec<f64>,
    /// Samples fed so far.
    at: usize,
    sent: Sent,
    far: Far,
}

fn prefix(options: Options, table: &Constellation, far: Far, snr_db: f64, data_symbols: usize, into: usize, seed: u64) -> Prefix {
    let data = random_data(table, data_symbols, seed);
    let sent = startup(&data, seed);
    let mut samples = modulate(&sent.symbols, far);
    add_noise(&mut samples, snr_db, seed.wrapping_add(7));
    let mut listener = Listener::new(options, sent.trn.clone(), table.clone());
    let at = sample_of(sent.data + into, far);
    listener.feed_all(&samples[..at]);
    assert!(listener.core.is_tracking() && listener.switched, "not tracking data by symbol {into} of it");
    Prefix { listener, samples, at, sent, far }
}

impl Prefix {
    /// Signal to noise over the `symbols` before the prefix ends.
    fn before_db(&self, symbols: usize) -> f64 {
        self.listener.snr_between(self.at - sample_of(symbols, Far::default()), self.at)
    }
}

/// Symbols settled after sample `from` until the first of `run` in a row
/// that are within `good` of their nearest point with the signal held; None
/// if that never comes.
fn recovery(records: &[Record], from: usize, good: f64, run: usize) -> Option<usize> {
    let mut streak = 0;
    for (i, record) in records.iter().filter(|r| r.at > from).enumerate() {
        if record.error < good && !record.lost {
            streak += 1;
            if streak == run {
                return Some(i + 1 - run);
            }
        } else {
            streak = 0;
        }
    }
    None
}

/// A jitter buffer's made-up audio, as `v34/receiver.rs:1572-1587` makes
/// it: the `n` samples before, faded in and out over 40 samples.
fn concealment(before: &[f64]) -> Vec<f64> {
    let n = before.len();
    let fade = 40;
    before
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let edge = i.min(n - 1 - i);
            if edge < fade { x * edge as f64 / fade as f64 } else { *x }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The tests.

#[test]
fn nearest_by_lattice_is_nearest_by_search() {
    let mut random = Random::new(1);
    for (name, table) in tables() {
        assert!(table.on_lattice(), "{name}: no lattice found");
        let peak = table.points().iter().map(|p| p.abs()).fold(0.0, f64::max);
        let mut checked = 0;
        for i in 0..200_000 {
            // Near the points, all over the plane round them, and far out.
            let z = match i % 3 {
                0 => table.points()[random.below(table.len())] + Complex::new(random.gaussian(), random.gaussian()).scale(0.2),
                1 => Complex::new(random.uniform() - 0.5, random.uniform() - 0.5).scale(4.0 * peak),
                _ => Complex::new(random.uniform() - 0.5, random.uniform() - 0.5).scale(40.0 * peak),
            };
            assert_eq!(table.nearest(z), table.nearest_exhaustive(z), "{name}: at {z:?}");
            checked += 1;
        }
        eprintln!(
            "{name:>3} points: d2min {:.4}, power {:.4}, garbage {:.4} (d2min/{:.1}), fourth-power line {:.3}; {checked} points agree",
            table.d2min(),
            table.power(),
            table.garbage(),
            table.d2min() / table.garbage(),
            table.fourth_line()
        );
    }
}

#[test]
fn the_defaults_are_v34s_receiver() {
    let options = Options::default();
    assert_eq!(options, Options::v34());
    assert!(!options.resync_margin && !options.resync_gain && !options.resync_derotate);
    assert!(!options.agc && !options.relative_gate && !options.rewind_safely && !options.extrapolate);
    assert!(!options.discriminator && !options.unweighted_phase && !options.level_gate && !options.anchor);
    assert!(options.turn_limit_hz.is_infinite());
    assert_eq!(band().cutoff, 1620.0);
}

#[test]
fn a_clean_line_trains_and_tracks_above_50_db() {
    for (name, table) in tables() {
        let far = Far::default();
        let (listener, sent) = call(Options::fixed(), &table, far, f64::INFINITY, 3000, 11);
        let trained = listener.trained_db();
        let tracked = tracked_db(&listener, &sent, far, 1000);
        eprintln!("clean, {name:>3} points: trained {trained:.1} dB, tracked {tracked:.1} dB, via {:?}", listener.trained.map(|t| t.1));
        assert!(trained >= 50.0, "{name}: trained to {trained:.1} dB");
        assert!(tracked >= 50.0, "{name}: tracked at {tracked:.1} dB");
    }
}

#[test]
fn thirty_db_trains_to_28_5_and_tracks_within_a_decibel() {
    for (name, table) in tables() {
        let far = Far::default();
        let (listener, sent) = call(Options::fixed(), &table, far, 30.0, 3000, 12);
        let trained = listener.trained_db();
        let tracked = tracked_db(&listener, &sent, far, 500);
        eprintln!("30 dB, {name:>3} points: trained {trained:.2} dB, tracked {tracked:.2} dB");
        assert!(trained >= 28.5, "{name}: trained to {trained:.2} dB");
        assert!((tracked - 30.0).abs() <= 1.0, "{name}: tracked at {tracked:.2} dB");
    }
}

#[test]
fn three_and_seven_hertz_either_way_at_35_db() {
    for (name, table) in tables() {
        for offset in [-7.0, -3.0, 3.0, 7.0] {
            let far = Far { offset_hz: offset, ..Far::default() };
            let (listener, sent) = call(Options::fixed(), &table, far, 35.0, 2500, 13);
            let trained = listener.trained_db();
            let tracked = tracked_db(&listener, &sent, far, 500);
            let heard = listener.reversal.map_or(f64::NAN, |(_, turn, _)| turn * BAUD / TAU);
            eprintln!(
                "{offset:+} Hz, {name:>3} points: S read {heard:+.3} Hz, trained {trained:.1} dB, tracked {tracked:.1} dB, loop at {:+.3} Hz",
                listener.core.offset_hz()
            );
            assert!((heard - offset).abs() <= 0.1, "{name} {offset} Hz: S read {heard} Hz");
            assert!(trained >= 33.0, "{name} {offset} Hz: trained to {trained:.1} dB");
            assert!(tracked >= 33.0, "{name} {offset} Hz: tracked at {tracked:.1} dB");
        }
    }
}

#[test]
fn two_hundred_and_a_thousand_ppm_either_way() {
    for (name, table) in tables() {
        for ppm in [-1000.0, -200.0, 200.0, 1000.0] {
            let far = Far { clock: 1.0 + ppm * 1e-6, ..Far::default() };
            let (listener, sent) = call(Options::fixed(), &table, far, 35.0, 6000, 14);
            let trained = listener.trained_db();
            let tracked = tracked_db(&listener, &sent, far, 3000);
            let drift = listener.core.drift_ppm();
            let from_s = listener.reversal.map_or(f64::NAN, |(_, _, d)| d * 1e6);
            eprintln!(
                "{ppm:+} ppm, {name:>3} points: S read {from_s:+.1} ppm, trained {trained:.1} dB via {:?}, tracked {tracked:.1} dB, loop at {drift:+.1} ppm, slips {}",
                listener.trained.map(|t| t.1),
                listener.core.slips()
            );
            assert!(trained >= 33.0, "{name} {ppm} ppm: trained to {trained:.1} dB");
            assert!(tracked >= 33.0, "{name} {ppm} ppm: tracked at {tracked:.1} dB");
            // A far clock fast by `ppm` makes our half symbols that much
            // shorter.
            assert!((drift + ppm).abs() <= 0.03 * ppm.abs(), "{name} {ppm} ppm: drift read {drift:.1} ppm");
        }
    }
}

/// Every length of made-up audio inserted, or of audio dropped, from 1 to
/// 340 samples, 1000 symbols into data on `table` at 35 dB: the symbols
/// until 48 in a row are clean again, counted from where the line is whole
/// again -- for an insertion, the end of what was made up.
fn every_slip(name: &str, table: &Constellation, inserted: bool) {
    let p = prefix(Options::fixed(), table, Far::default(), 35.0, 3000, 1000, 31);
    let before = p.before_db(500);
    let good = table.d2min() / 16.0;
    let (mut worst, mut resynced, mut failed) = ((0, 0), 0, Vec::new());
    for n in 1..=340 {
        let mut listener = p.listener.clone();
        let slips = listener.core.slips();
        let tail: Vec<f64> = if inserted {
            let mut made = concealment(&p.samples[p.at - n..p.at]);
            made.extend_from_slice(&p.samples[p.at..p.at + 2400]);
            made
        } else {
            p.samples[p.at + n..p.at + n + 2400].to_vec()
        };
        listener.feed_all(&tail);
        let whole = p.at + if inserted { n } else { 0 };
        let back = recovery(&listener.records, whole, good, 48);
        let after = listener.snr_between(listener.fed - sample_of(200, Far::default()), listener.fed);
        resynced += usize::from(listener.core.slips() > slips);
        match back {
            Some(symbols) if symbols <= 150 && after >= before - 2.0 => {
                if symbols > worst.0 {
                    worst = (symbols, n);
                }
            }
            _ => failed.push((n, back, after)),
        }
    }
    let kind = if inserted { "inserted" } else { "dropped" };
    eprintln!(
        "{name:>3} points, 1-340 samples {kind}: {} of 340 back within 150 symbols; slowest {} symbols (at {} samples); {resynced} resynced; {before:.1} dB before",
        340 - failed.len(),
        worst.0,
        worst.1
    );
    assert!(failed.is_empty(), "{name} points, {kind}: not back within 150 symbols at (length, symbols, dB after) {failed:?}");
}

#[test]
fn every_insertion_on_sixteen_points_is_recovered() {
    every_slip("16", &sixteen(), true);
}

#[test]
fn every_drop_on_sixteen_points_is_recovered() {
    every_slip("16", &sixteen(), false);
}

#[test]
fn every_insertion_on_the_128_point_cross_is_recovered() {
    every_slip("128", &cross128(), true);
}

#[test]
fn every_drop_on_the_128_point_cross_is_recovered() {
    every_slip("128", &cross128(), false);
}

/// The line from where a prefix ends to sample `to`, its symbol clock jumped
/// by `jump` symbols from there and its carrier not.
fn jumped(p: &Prefix, jump: f64, to: usize) -> Vec<f64> {
    let far = Far { jump: Some((p.at, jump)), ..p.far };
    let mut random = Random::new(0x1eaf);
    let sigma = noise_sigma(35.0);
    (p.at..to).map(|n| line_sample(&p.sent.symbols, far, n) + sigma * random.gaussian()).collect()
}

#[test]
fn a_timing_jump_of_a_quarter_to_a_third_of_a_symbol_is_recovered() {
    // The band V.34's plain resync could never reach on sixteen points
    // (core.md E3e, E3f), and the constellations either side of it.
    for (name, table) in [("4", four()), ("16", sixteen()), ("128", cross128())] {
        let p = prefix(Options::fixed(), &table, Far::default(), 35.0, 3000, 1000, 32);
        let good = table.d2min() / 16.0;
        let mut line = String::new();
        for step in 0..=4 {
            let jump = 0.25 + 0.025 * f64::from(step);
            let mut listener = p.listener.clone();
            listener.feed_all(&jumped(&p, jump, p.at + 2400));
            let back = recovery(&listener.records, p.at, good, 48);
            line += &format!(" +{jump:.3}: {back:?}");
            assert!(back.is_some_and(|b| b <= 150), "{name} points, +{jump} symbol: back after {back:?} symbols");
        }
        eprintln!("{name:>3} points, symbols until clean after a jump of:{line}");
    }
}

/// The line from where a prefix ends, its level moved by `step_db` over
/// `ramp` seconds, noise and all, for `seconds`: a softphone's gain control,
/// or its limiter.
fn stepped(p: &Prefix, step_db: f64, ramp: f64, seconds: f64) -> Vec<f64> {
    let length = (ramp * FS) as usize;
    let end = (p.at + (seconds * FS) as usize).min(p.samples.len());
    (p.at..end)
        .map(|n| {
            let done = if length == 0 { 1.0 } else { ((n - p.at) as f64 / length as f64).min(1.0) };
            p.samples[n] * 10f64.powf(step_db * done / 20.0)
        })
        .collect()
}

#[test]
fn gain_steps_and_ramps_are_followed_without_a_false_lock() {
    for (name, table) in [("16", sixteen()), ("128", cross128())] {
        let p = prefix(Options::fixed(), &table, Far::default(), 35.0, 6000, 1000, 33);
        let before = p.before_db(500);
        let good = table.d2min() / 16.0;
        for step_db in [-6.0, -3.0, 3.0, 6.0] {
            let mut line = String::new();
            for ramp in [0.0, 0.1, 0.25, 0.5] {
                let mut listener = p.listener.clone();
                let slips = listener.core.slips();
                listener.feed_all(&stepped(&p, step_db, ramp, ramp + 1.2));
                let end = p.at + (ramp * FS) as usize;
                let back = recovery(&listener.records, end, good, 48);
                let after = listener.snr_between(end + (0.5 * FS) as usize, end + FS as usize);
                let seconds = back.map(|b| b as f64 / BAUD);
                line += &format!(
                    " {ramp} s: back {}, {after:.1} dB, {} resyncs;",
                    seconds.map_or("never".into(), |s| format!("{s:.3} s")),
                    listener.core.slips() - slips
                );
                assert!(seconds.is_some_and(|s| s <= 0.5), "{name} points, {step_db} dB over {ramp} s: back after {seconds:?} s");
                assert!(after >= before - 1.0, "{name} points, {step_db} dB over {ramp} s: {after:.1} dB after, {before:.1} before");
            }
            eprintln!("{name:>3} points, {step_db:+} dB ({before:.1} dB before):{line}");
        }
    }
}

/// What a hunting core hears in `samples`: how many times S, and where
/// S-bar was found each time.
fn hunted(options: Options, samples: &[f64]) -> (usize, Vec<(u64, f64, f64)>) {
    let mut core = Core::new(band(), options, Slicer::table(four()));
    core.hunt();
    let (mut s, mut reversals) = (0, Vec::new());
    for &x in samples {
        core.feed(x);
        while let Some(heard) = core.heard() {
            match heard {
                Heard::S => s += 1,
                Heard::Reversal { at, turn, drift } => {
                    reversals.push((at, turn, drift));
                    core.hunt();
                }
                _ => {}
            }
        }
    }
    (s, reversals)
}

#[test]
fn only_s_is_taken_for_s_and_s_bar_is_placed_to_a_half_symbol() {
    let [a, b, c, d] = states();
    let quiet = |n| vec![Complex::ZERO; n];
    let mut random = Random::new(41);
    let sixteen = sixteen();
    // V.32's preamble tones, each followed by the reversal that ends it
    // (V.32 5.4.1, 5.4.2), and a broadband signal like the optional echo
    // canceller training (5.4 Note 3): random sixteen-point symbols.
    let aa: Vec<Complex> = [quiet(100), vec![a; 600], vec![c; 300], quiet(100)].concat();
    let ac: Vec<Complex> =
        [quiet(100), (0..600).map(|n| if n % 2 == 0 { a } else { c }).collect(), (0..300).map(|n| if n % 2 == 0 { c } else { a }).collect(), quiet(100)].concat();
    let broadband: Vec<Complex> = [quiet(100), (0..2400).map(|_| sixteen.points()[random.below(16)]).collect(), quiet(100)].concat();
    for (name, symbols) in [("AA then CC", &aa), ("AC then CA", &ac), ("a broadband sequence", &broadband)] {
        let mut samples = modulate(symbols, Far::default());
        add_noise(&mut samples, 30.0, 42);
        let (s, reversals) = hunted(Options::fixed(), &samples);
        let (s_v34, reversals_v34) = hunted(Options::v34(), &samples);
        eprintln!(
            "{name}: S heard {s} times, S-bar {} (V.34's hunt: S {s_v34} times, S-bar {})",
            reversals.len(),
            reversals_v34.len()
        );
        assert_eq!((s, reversals.len()), (0, 0), "{name} taken for S");
    }
    // S itself, at any arrival phase and offset, with its band edges as far
    // down as V.32 2.2 lets them be and no further: 2 dB, and 7. The pulse
    // leaves them 3 dB down; the rest is S's band-edge lines scaled.
    let mut worst: f64 = 0.0;
    let mut cases = 0;
    for edges_db in [2.0, 3.0, 7.0] {
        let g = 10f64.powf(-(edges_db - 3.0) / 20.0);
        let line = |k: usize, point: Complex| {
            let (dc, edge) = (point * Complex::new(0.5, 0.5), point * Complex::new(0.5, -0.5));
            dc + edge.scale(if k.is_multiple_of(2) { g } else { -g })
        };
        let data = random_data(&four(), 400, 43);
        let mut sent = startup(&data, 43);
        for k in SILENCE..sent.s_bar + S_BAR_LENGTH {
            let base = if k < sent.s_bar { a } else { c };
            sent.symbols[k] = line(k - SILENCE, base);
        }
        for delay in [0.0, 1.3, 2.6, 3.9, 5.2] {
            for offset in [-7.0, 0.0, 7.0] {
                let far = Far { delay, offset_hz: offset, ..Far::default() };
                let mut samples = modulate(&sent.symbols, far);
                add_noise(&mut samples, 20.0, 44 + delay as u64);
                let (_, reversals) = hunted(Options::fixed(), &samples);
                // S-bar begins half a symbol before its first symbol's centre.
                let expected = half_of_symbol(sent.s_bar, far) - 1.0;
                assert_eq!(reversals.len(), 1, "edges {edges_db} dB, delay {delay}, {offset} Hz: {reversals:?}");
                let off = reversals[0].0 as f64 - expected;
                worst = worst.max(off.abs());
                cases += 1;
                assert!(off.abs() <= 1.0, "edges {edges_db} dB, delay {delay}, {offset} Hz: S-bar at {} against {expected:.2}", reversals[0].0);
            }
        }
    }
    let _ = (b, d);
    eprintln!("S with its edges 2, 3 and 7 dB down, 5 arrival phases, 0 and 7 Hz either way, at 20 dB: {cases} found, S-bar at most {worst:.2} half symbols out");
}

#[test]
fn a_silent_far_end_after_training_starts_no_storm_of_resyncs() {
    for (name, table) in [("4", four()), ("16", sixteen()), ("128", cross128())] {
        let mut results = Vec::new();
        for (label, options) in [("fixed", Options::fixed()), ("V.34", Options::v34())] {
            let p = prefix(options, &table, Far::default(), 35.0, 8000, 1000, 51);
            let mut listener = p.listener.clone();
            let (slips, resyncs, gain) = (listener.core.slips(), listener.core.resyncs(), listener.core.gain_db());
            // Two seconds of the line with nothing but its noise on it.
            let mut quiet = vec![0.0; 2 * FS as usize];
            add_noise(&mut quiet, 35.0, 52);
            listener.feed_all(&quiet);
            let lost = listener.core.is_lost();
            let tried = listener.core.resyncs() - resyncs;
            let slipped = listener.core.slips() - slips;
            let moved = listener.core.gain_db() - gain;
            // And the far end back, its clock having run on through the gap.
            let back_at = p.at + quiet.len();
            let fed = listener.fed;
            listener.feed_all(&p.samples[back_at..back_at + FS as usize / 2]);
            let back = recovery(&listener.records, fed, table.d2min() / 16.0, 48);
            results.push(format!("{label}: lost {lost}, {tried} resyncs tried and {slipped} taken in 2 s, gain moved {moved:+.2} dB, back {back:?} symbols after"));
            if label == "fixed" {
                assert!(lost, "{name}: silence not seen");
                assert_eq!((tried, slipped), (0, 0), "{name}: resyncs on silence");
                assert!(moved.abs() < 0.1, "{name}: the gain wound {moved} dB on silence");
                assert!(back.is_some_and(|b| b <= 200), "{name}: not found again, {back:?}");
            }
        }
        eprintln!("{name:>3} points: {}", results.join("; "));
    }
}

#[test]
fn blind_starts_find_four_points_at_eight_arrival_phases_and_sixteen_and_the_32_cross() {
    let mut cases: Vec<(String, Constellation, f64)> = (0..8).map(|k| (format!("4 points, phase {k}"), four(), f64::from(k))).collect();
    cases.push(("16 points".into(), sixteen(), 0.0));
    cases.push(("32-point cross".into(), cross32(), 0.0));
    for (name, table, delay) in cases {
        let far = Far { delay, ..Far::default() };
        let lead = 100;
        let data = random_data(&table, 2400, 61);
        let symbols = [vec![Complex::ZERO; lead], data].concat();
        let mut samples = modulate(&symbols, far);
        add_noise(&mut samples, 30.0, 62);
        let slicer = Slicer::table(table.clone());
        let mut core = Core::new(band(), Options::fixed(), slicer.clone());
        core.acquire_blind(slicer, ROLLOFF);
        let mut found = None;
        let mut errors = Vec::new();
        for (n, &x) in samples.iter().enumerate() {
            core.feed(x);
            while let Some(heard) = core.heard() {
                if let Heard::Trained { snr_db, via: Via::Blind } = heard {
                    found = Some((n, snr_db));
                }
            }
            while let Some(point) = core.next() {
                core.settle(point.nearest);
                errors.push((n, (point.z - point.nearest).norm_sqr()));
            }
        }
        let (at, snr) = found.unwrap_or_else(|| panic!("{name}: never found"));
        let symbols_in = (at as f64 - sample_of(lead, far) as f64) / (FS / BAUD);
        let late: Vec<f64> = errors.iter().filter(|(n, _)| *n > samples.len() - sample_of(1040, far)).map(|e| e.1).collect();
        let tracked = -10.0 * (late[..late.len() - 40].iter().sum::<f64>() / (late.len() - 40) as f64).log10();
        eprintln!("blind, {name}: found {symbols_in:.0} symbols in at {snr:.1} dB, tracked {tracked:.1} dB, stage {:?}", core.stage());
        assert!(symbols_in <= 200.0, "{name}: found {symbols_in:.0} symbols in");
        assert!(tracked >= 28.5, "{name}: tracked at {tracked:.1} dB");
        assert_eq!(core.stage(), Stage::Tracking);
    }
}

#[test]
fn ten_minutes_at_200_ppm_hold_the_signal_to_noise_and_the_taps_centred() {
    // An hour-long file transfer is what the core is for; nothing ties the
    // equaliser's weight to its middle but the anchor (core.md N6). The
    // 128-point cross at 30 dB, the far clock 200 ppm fast.
    let table = cross128();
    let far = Far { clock: 1.0 + 200e-6, ..Far::default() };
    let minutes = 10.0;
    let data = random_data(&table, (minutes * 60.0 * BAUD) as usize, 71);
    let sent = startup(&data, 71);
    let length = sample_of(sent.symbols.len() - 40, far);
    let sigma = noise_sigma(30.0);
    let mut random = Random::new(72);
    let mut listener = Listener::new(Options::fixed(), sent.trn.clone(), table.clone());
    let mut n = 0;
    while !listener.switched || listener.records.len() < 2000 {
        listener.feed(line_sample(&sent.symbols, far, n) + sigma * random.gaussian());
        n += 1;
    }
    let mut core = listener.core;
    let block = (10.0 * FS) as usize;
    let (mut sum, mut count, mut blocks, mut centroids) = (0.0, 0usize, Vec::new(), (f64::MAX, f64::MIN));
    let mut start = n;
    while n < length {
        core.feed(line_sample(&sent.symbols, far, n) + sigma * random.gaussian());
        while let Some(point) = core.next() {
            core.settle(point.nearest);
            sum += (point.z - point.nearest).norm_sqr();
            count += 1;
        }
        n += 1;
        if n - start == block {
            blocks.push(-10.0 * (sum / count as f64).log10());
            let centroid = core.tap_centroid();
            centroids = (centroids.0.min(centroid), centroids.1.max(centroid));
            (sum, count, start) = (0.0, 0, n);
        }
    }
    let first = blocks[0];
    let (low, high) = blocks.iter().fold((f64::MAX, f64::MIN), |(l, h), &b| (l.min(b), h.max(b)));
    eprintln!(
        "{minutes} minutes at 200 ppm, 128 points, 30 dB: {} blocks of 10 s from {low:.2} to {high:.2} dB (first {first:.2}); taps' centroid {:+.2} to {:+.2} half symbols; drift read {:+.1} ppm; {} slips, {} resyncs",
        blocks.len(),
        centroids.0,
        centroids.1,
        core.drift_ppm(),
        core.slips(),
        core.resyncs()
    );
    assert!(blocks.iter().all(|b| (b - first).abs() <= 0.5), "signal to noise by 10 s: {blocks:.2?}");
    assert!(centroids.0 >= -2.0 && centroids.1 <= 2.0, "taps' centroid from {} to {}", centroids.0, centroids.1);
}

#[test]
fn an_s_that_ends_without_s_bar_says_where_it_ended() {
    // For a driver to train anyway where S ended, as design.md 3.2 has a
    // hunt that lapses do: a slip across the join can hide S-bar.
    let [a, b, _, _] = states();
    let symbols = [vec![Complex::ZERO; 100], (0..S_LENGTH).map(|n| if n % 2 == 0 { a } else { b }).collect(), vec![Complex::ZERO; 600]].concat();
    let far = Far::default();
    let mut samples = modulate(&symbols, far);
    add_noise(&mut samples, 25.0, 92);
    let mut core = Core::new(band(), Options::fixed(), Slicer::table(four()));
    core.hunt();
    let (mut s, mut lapsed, mut reversed) = (0, Vec::new(), 0);
    for &x in &samples {
        core.feed(x);
        while let Some(heard) = core.heard() {
            match heard {
                Heard::S => s += 1,
                Heard::Lapsed { at } => lapsed.push(at),
                Heard::Reversal { .. } => reversed += 1,
                _ => {}
            }
        }
    }
    let end = half_of_symbol(100 + S_LENGTH, far) - 1.0;
    eprintln!("S heard {s} times, lapsed at {lapsed:?} against S's end at {end:.1}, {reversed} reversals");
    assert_eq!((s, reversed), (1, 0));
    assert_eq!(lapsed.len(), 1);
    assert!((lapsed[0] as f64 - end).abs() <= 4.0, "lapsed at {} against {end:.1}", lapsed[0]);
}

#[test]
fn a_training_that_fits_nothing_falls_back_on_the_taps_it_had() {
    // A second training whose sequence is not where it is said to be, on a
    // line the first one trained on: the old taps, a resync search on the
    // newest symbols, and tracking again (design.md 3.2).
    for (name, table) in [("16", sixteen()), ("128", cross128())] {
        let p = prefix(Options::fixed(), &table, Far::default(), 30.0, 8000, 1000, 93);
        let mut listener = p.listener.clone();
        let mut wrong = listener.training(p.at as u64 / 3, 0.0, 0.0);
        let mut random = Random::new(94);
        wrong.targets = (0..TRN_LENGTH).map(|_| states()[random.below(4)]).collect();
        wrong.slicer = Slicer::table(table.clone());
        listener.index = None;
        listener.trained = None;
        let start = listener.core.halves();
        wrong.start = start + 32;
        listener.core.train(wrong);
        let fed = listener.fed;
        listener.feed_all(&p.samples[p.at..p.at + 3 * FS as usize / 2]);
        let back = recovery(&listener.records, fed, table.d2min() / 16.0, 48);
        eprintln!("{name:>3} points: a training on the wrong sequence came to {:?}; then clean {back:?} symbols after", listener.trained);
        assert!(matches!(listener.trained, Some((_, Via::Fallback))), "{name}: {:?}", listener.trained);
        assert!(back.is_some(), "{name}: not tracking after the fallback");
    }
}

#[test]
fn the_v34_defaults_keep_v34s_measured_defects() {
    // Options::default() is V.34's receiver: the fixes are the options, and
    // without them V.34's own measured failures come back (core.md E3e, E4).
    // Sixteen points at 35 dB, training told nothing from S, as V.34's is.
    let table = sixteen();
    let far = Far::default();
    let good = table.d2min() / 16.0;
    for (label, options) in [("V.34", Options::v34()), ("fixed", Options::fixed())] {
        let data = random_data(&table, 4000, 81);
        let sent = startup(&data, 81);
        let mut samples = modulate(&sent.symbols, far);
        add_noise(&mut samples, 35.0, 88);
        let mut listener = Listener::new(options, sent.trn.clone(), table.clone());
        listener.from_s = false;
        let at = sample_of(sent.data + 1000, far);
        listener.feed_all(&samples[..at]);
        let p = Prefix { listener, samples, at, sent, far };
        // A timing jump of 0.3 symbol, in the look-ahead hole.
        let mut jumped_listener = p.listener.clone();
        jumped_listener.feed_all(&jumped(&p, 0.3, p.at + 8000));
        let after_jump = recovery(&jumped_listener.records, p.at, good, 48);
        // A 3 dB step.
        let mut stepped_listener = p.listener.clone();
        stepped_listener.feed_all(&stepped(&p, 3.0, 0.0, 0.5));
        let after_step = recovery(&stepped_listener.records, p.at, good, 48);
        eprintln!("{label}: back {after_jump:?} symbols after a 0.3-symbol timing jump, {after_step:?} after a 3 dB step (of 1200)");
        if label == "V.34" {
            assert!(after_jump.is_none(), "V.34's defaults found the jump the look-ahead hole hides");
            assert!(after_step.is_none(), "V.34's defaults followed a 3 dB step");
        } else {
            assert!(after_jump.is_some_and(|b| b <= 150) && after_step.is_some_and(|b| b <= 150));
        }
    }
}

