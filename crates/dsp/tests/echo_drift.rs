//! The echo canceller on a line whose echo does not stay where it was learned.
//!
//! A sound card looped back on itself with a cable is the line in question.
//! Both directions of it cross two clocks a few parts per million apart, so
//! this end's own signal comes back a little later, or a little earlier,
//! every second, and at full strength: as loud as the far end. Every so
//! often the card drops a sample, repeats one, or runs dry and plays a
//! buffer of silence, and the echo moves all at once.
//!
//! Frozen after training, as it always was in data mode, the canceller was
//! cancelling where the echo used to be. On a 20 ppm cable that is nothing
//! at all within seconds. These are the proofs that following it works, and
//! that it changes nothing where there is nothing to follow.

use dsp::{EchoCanceller, EchoFinder, Fir, Resampler, fir_lowpass, rrc_at};
use std::collections::VecDeque;
use std::f64::consts::TAU;

const FS: f64 = 16_000.0;

/// Pseudorandom and deterministic, so every run sees the same line.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

/// Noise in a V.32 signal's band: two baseband noises kept under 1200 Hz and
/// put on an 1800 Hz carrier, so it fills 600 to 3000 Hz and nothing else,
/// with no pattern in it for a canceller to be fooled by.
struct Band {
    noise: Noise,
    i: Fir,
    q: Fir,
    n: u64,
}

impl Band {
    fn new(seed: u64) -> Self {
        Self {
            noise: Noise(seed),
            i: Fir::new(fir_lowpass(1200.0, 101, FS)),
            q: Fir::new(fir_lowpass(1200.0, 101, FS)),
            n: 0,
        }
    }

    fn next(&mut self) -> f64 {
        let i = self.i.process(self.noise.next());
        let q = self.q.process(self.noise.next());
        let w = TAU * 1800.0 * self.n as f64 / FS;
        self.n += 1;
        // Scaled to about unit power.
        3.0 * (i * w.cos() - q * w.sin())
    }
}

/// A modem's signal, or near enough: random 16-point symbols at 2400 baud,
/// root-raised-cosine shaped and put on an 1800 Hz carrier, at unit power.
///
/// Noise in the same band would do for the canceller's taps, but not for its
/// gate, which watches what is left for a rise over 5 ms. The far end is most
/// of what is left, and a modem's power over 5 ms is far steadier than
/// noise's: its symbols come from a handful of levels, where a noise's
/// envelope is Rayleigh. With noise for the far end the gate held the loop
/// six tenths of the time.
struct Qam {
    noise: Noise,
    /// Symbols from `first` on, as far as the pulse reaches.
    symbols: VecDeque<(f64, f64)>,
    first: i64,
    n: u64,
}

/// Symbols either side of the present the pulse is evaluated over.
const PULSE_REACH: i64 = 8;
const BAUD: f64 = 2400.0;

impl Qam {
    fn new(seed: u64) -> Self {
        Self {
            noise: Noise(seed),
            symbols: VecDeque::new(),
            first: -PULSE_REACH,
            n: 0,
        }
    }

    fn level(&mut self) -> f64 {
        [-3.0, -1.0, 1.0, 3.0][((self.noise.next() + 1.0) * 2.0).min(3.999) as usize]
    }

    fn next(&mut self) -> f64 {
        let t = self.n as f64 * BAUD / FS;
        self.n += 1;
        let last = t.floor() as i64 + PULSE_REACH;
        while self.first + (self.symbols.len() as i64) <= last {
            let symbol = (self.level(), self.level());
            self.symbols.push_back(symbol);
        }
        while self.first < t.floor() as i64 - PULSE_REACH {
            self.symbols.pop_front();
            self.first += 1;
        }
        let (mut re, mut im) = (0.0, 0.0);
        for (k, &(a, b)) in self.symbols.iter().enumerate() {
            let p = rrc_at(t - (self.first + k as i64) as f64, 0.25);
            re += a * p;
            im += b * p;
        }
        let w = TAU * 1800.0 * t / BAUD;
        // Ten for the symbols, halved by the carrier.
        (re * w.cos() - im * w.sin()) / 5.0f64.sqrt()
    }
}

/// FNV-1a, over the exact bits of what comes out.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn take(&mut self, x: f64) {
        for byte in x.to_bits().to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Everything the canceller could do before it could follow drift, in the
/// order a V.32 call does it, with every output folded into one number.
/// `touch` is called on the canceller before anything else.
fn todays_canceller(touch: impl Fn(&mut EchoCanceller)) -> u64 {
    let mut sent = Band::new(0x2545_f491_4f6c_dd1d);
    let mut far = Band::new(0x9e37_79b9_7f4a_7c15);
    // A hybrid, and a reflection 700 samples back smeared over three.
    let mut path = vec![0.0; 704];
    path[1] = 0.25;
    path[2] = 0.1;
    path[3] = -0.05;
    path[700] = 0.3;
    path[701] = 0.12;
    path[702] = -0.04;
    let mut history = VecDeque::from(vec![0.0; path.len()]);
    let mut ec = EchoCanceller::new(128, 0.5);
    touch(&mut ec);
    let mut digest = Digest::new();

    for n in 0..60_000 {
        let x = sent.next();
        history.pop_back();
        history.push_front(x);
        let echo: f64 = path.iter().zip(&history).map(|(h, x)| h * x).sum();
        // Silent for the training, then talking over it, as a far end is.
        let talking = n >= 24_000;
        let heard = echo + if talking { 0.5 * far.next() } else { 0.0 };
        match n {
            // Halfway through the training, as the start-up places them.
            8_000 => ec.watch_far_echo(668, 64),
            24_000 => ec.set_adapting(false),
            40_000 => {
                digest.take(ec.echo_return_loss());
                ec.reset_meters();
            }
            // Adapting through double talk, which it is not meant to do.
            48_000 => ec.set_adapting(true),
            54_000 => {
                digest.take(ec.echo_return_loss());
                ec.reset();
            }
            _ => {}
        }
        digest.take(ec.process(x, heard));
    }
    digest.take(ec.echo_return_loss());
    digest.take(ec.span() as f64);
    digest.take(ec.len() as f64);
    digest.0
}

/// Recorded on the canceller as it was before it could follow drift, at
/// d7b914b, in both a debug and a release build.
const TODAY: u64 = 0xe256_edd2_ab32_732f;

#[test]
fn with_drift_left_alone_the_canceller_is_the_one_it_always_was() {
    // Every other mode that uses the canceller, and V.32 until its own first
    // training ends, gets the same numbers to the last bit.
    assert_eq!(todays_canceller(|_| {}), TODAY);
    // Switching off what was never on changes nothing either.
    assert_eq!(todays_canceller(|ec| ec.follow_drift(false)), TODAY);
    // And neither does switching it on and off again before anything runs.
    assert_eq!(
        todays_canceller(|ec| {
            ec.follow_drift(true);
            ec.follow_drift(false);
        }),
        TODAY
    );
}

/// How loud each modem is on the wire: two of them summed at 0.45, as
/// `modem-loop` writes them, so this end's echo and the far end arrive at
/// the same level.
const HEADROOM: f64 = 0.45;

/// One crossing of the cable, in samples: the rig's 87 ms round trip counts
/// two of them.
const CROSSING: usize = 700;

/// What goes wrong on the way, besides the clocks.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fault {
    /// The input loses a sample, so everything arrives one sooner.
    Drop,
    /// The input repeats one, so everything arrives one later.
    Repeat,
    /// The output runs dry and plays this much silence, so everything
    /// arrives that much later.
    Gap(usize),
}

/// A sound card's cable, as one end hears it: its own signal back after a
/// crossing, the far end on top at the same level, both taken from the
/// output's clock to the input's, and whatever faults are asked for.
struct Cable {
    sent: Qam,
    far: Qam,
    /// The echo and the far end cross separately, through the same clock,
    /// so that what is left can be split into the far end and what is not.
    clock: (Resampler, Resampler),
    arriving: (VecDeque<f64>, VecDeque<f64>),
    last: (f64, f64),
    out: Vec<f64>,
    talking: bool,
}

impl Cable {
    fn new(ppm: f64) -> Self {
        let clock = || Resampler::new(FS, FS * (1.0 + ppm * 1.0e-6));
        let crossing = || VecDeque::from(vec![0.0; CROSSING]);
        Self {
            sent: Qam::new(0x2545_f491_4f6c_dd1d),
            far: Qam::new(0x9e37_79b9_7f4a_7c15),
            clock: (clock(), clock()),
            arriving: (crossing(), crossing()),
            last: (0.0, 0.0),
            out: Vec::new(),
            talking: false,
        }
    }

    fn play(&mut self, echo: f64, far: f64) {
        self.out.clear();
        self.clock.0.process(echo, &mut self.out);
        self.arriving.0.extend(self.out.iter());
        self.out.clear();
        self.clock.1.process(far, &mut self.out);
        self.arriving.1.extend(self.out.iter());
    }

    /// One sample: what this end sends, what it hears, and how much of what
    /// it hears is the far end.
    fn step(&mut self, fault: Option<Fault>) -> (f64, f64, f64) {
        let x = self.sent.next();
        let y = if self.talking { self.far.next() } else { 0.0 };
        if let Some(Fault::Gap(n)) = fault {
            for _ in 0..n {
                self.play(0.0, 0.0);
            }
        }
        self.play(HEADROOM * x, HEADROOM * y);
        if fault == Some(Fault::Drop) {
            self.arriving.0.pop_front();
            self.arriving.1.pop_front();
        }
        let heard = if fault == Some(Fault::Repeat) {
            self.last
        } else {
            (
                self.arriving.0.pop_front().unwrap_or(0.0),
                self.arriving.1.pop_front().unwrap_or(0.0),
            )
        };
        self.last = heard;
        (x, heard.0 + heard.1, heard.1)
    }
}

/// Samples of training: 2 s with the far end silent, the first of them spent
/// finding where the echo is, as the V.32 start-up spends half its training
/// segment.
const TRAINING: usize = 32_000;

/// How the canceller's taps are laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Layout {
    /// A millisecond of near taps, which is all of the near echo that stays
    /// put, and a far run where the finder puts it. What a cable asks for,
    /// and what the proofs of following use: trained on a cable at 100 ppm
    /// it leaves the echo 33 dB down, so what is measured is the following
    /// and not the training.
    Cable,
    /// The V.32 modem's: 8 ms of near taps, for a hybrid and what follows
    /// it, and the same far run. On a cable the near taps have nothing to
    /// learn and slow least mean squares down, and at 100 ppm it cannot keep
    /// up with the echo moving under it: the training leaves it 29 dB down
    /// before any following begins.
    V32,
}

/// Energies over one 10 ms stretch.
#[derive(Debug, Clone, Copy, Default)]
struct Stretch {
    /// Of what is left that is not the far end.
    echo: f64,
    /// Of the far end.
    far: f64,
}

const STRETCH: usize = 160;
const PER_SECOND: usize = FS as usize / STRETCH;

/// What a run did, stretch by stretch from the moment following began.
struct Run {
    stretches: Vec<Stretch>,
    jumps: u32,
    ppm: f64,
}

impl Run {
    /// Residual echo under the far end in decibels, over `n` stretches from
    /// the `from`th.
    fn residual(&self, from: usize, n: usize) -> f64 {
        let (echo, far) = self.stretches[from..from + n]
            .iter()
            .fold((0.0, 0.0), |(e, f), s| (e + s.echo, f + s.far));
        10.0 * (echo / far).log10()
    }

    /// The same over each second, from `from` seconds in.
    fn seconds(&self, from: usize) -> Vec<f64> {
        (from..self.stretches.len() / PER_SECOND)
            .map(|s| self.residual(s * PER_SECOND, PER_SECOND))
            .collect()
    }
}

/// Train as V.32 does on a cable at `ppm`, then follow the drift, or not, for
/// `seconds` with the far end talking and `faults` at the given samples from
/// when following began.
fn run(layout: Layout, ppm: f64, seconds: f64, follow: bool, faults: &[(usize, Fault)]) -> Run {
    let mut cable = Cable::new(ppm);
    let near = match layout {
        Layout::Cable => 16,
        Layout::V32 => 128,
    };
    let mut ec = EchoCanceller::new(near, 0.5);
    let mut finder = Some(EchoFinder::new(128, 1024));
    for n in 0..TRAINING {
        let (x, heard, _) = cable.step(None);
        let left = ec.process(x, heard);
        if let Some(f) = finder.as_mut() {
            f.feed(x, left);
        }
        if n == TRAINING / 2 {
            let found = finder.take().and_then(|f| f.best()).expect("no echo found");
            ec.watch_far_echo(found.delay.saturating_sub(32).max(ec.span()), 64);
        }
    }
    ec.set_adapting(false);
    ec.follow_drift(follow);
    cable.talking = true;

    let mut stretches = Vec::new();
    let mut now = Stretch::default();
    for n in 0..(seconds * FS) as usize {
        let fault = faults.iter().find(|f| f.0 == n).map(|f| f.1);
        let (x, heard, far) = cable.step(fault);
        let left = ec.process(x, heard);
        now.echo += (left - far) * (left - far);
        now.far += far * far;
        if (n + 1).is_multiple_of(STRETCH) {
            stretches.push(now);
            now = Stretch::default();
        }
    }
    Run {
        stretches,
        jumps: ec.jumps(),
        ppm: ec.drift_ppm(),
    }
}

fn worst(seconds: &[f64]) -> f64 {
    seconds.iter().copied().fold(f64::MIN, f64::max)
}

fn median(seconds: &[f64]) -> f64 {
    let mut sorted = seconds.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// Following for 60 s at one drift. Every second from the third on is at
/// least `bar` under the far end, nothing is taken to have jumped, and the
/// drift is read to within a fifth.
fn follows(ppm: f64, bar: f64) {
    let run = run(Layout::Cable, ppm, 60.0, true, &[]);
    let seconds = run.seconds(3);
    let worst = worst(&seconds);
    println!(
        "{ppm} ppm: residual echo {:.1} dB in the fourth second, median {:.1}, \
         worst {worst:.1}, last {:.1}; drift read {:.1} ppm; {} jumps",
        seconds[0],
        median(&seconds),
        seconds[seconds.len() - 1],
        run.ppm,
        run.jumps
    );
    assert!(
        worst <= bar,
        "at {ppm} ppm the residual echo reached {worst:.1} dB of the far end"
    );
    assert_eq!(run.jumps, 0, "a clean line at {ppm} ppm was taken to have jumped");
    assert!(
        (run.ppm - ppm).abs() < 0.2 * ppm.abs().max(5.0),
        "at {ppm} ppm the drift was read as {:.1}",
        run.ppm
    );
}

#[test]
fn a_cable_with_no_drift_is_followed() {
    follows(0.0, -28.0);
}

#[test]
fn five_parts_per_million_are_followed() {
    follows(5.0, -28.0);
}

#[test]
fn twenty_parts_per_million_are_followed() {
    follows(20.0, -28.0);
}

#[test]
fn a_hundred_parts_per_million_are_followed() {
    // The first training's floor, not the following's. Least mean squares
    // trains here on an echo moving 1.6 samples a second and cannot keep up:
    // it leaves the echo 33 dB down (38 the other way, for reasons of its
    // own and the same on an ideal drifting echo with no resampler in the
    // way), and what it lagged in differs across the band, which no single
    // delay makes up. Followed with the far end silent the echo sits 30 dB
    // down, and with it talking at 28. A retrain, trained with the delay
    // moving under it, gets past that; see below.
    follows(100.0, -26.0);
}

#[test]
fn a_hundred_parts_per_million_the_other_way_are_followed() {
    // The echo arriving earlier and earlier walks the retimed taps towards
    // the present, which is the direction with a limit in it.
    follows(-100.0, -28.0);
}

#[test]
fn a_retrain_on_a_drifting_cable_trains_as_if_it_did_not_drift() {
    // After a minute of following at 100 ppm the rate is known, and a
    // retrain's training segment is taught with the delay moving on at it:
    // the echo stands still under the taps, and they learn it as well as
    // on a cable that does not drift.
    let mut cable = Cable::new(100.0);
    let mut ec = EchoCanceller::new(16, 0.5);
    let mut finder = Some(EchoFinder::new(128, 1024));
    let seconds = |s: f64| (s * FS) as usize;
    let mut stretches = Vec::new();
    let mut now = Stretch::default();
    let retrain = seconds(2.0 + 20.0);
    let total = retrain + TRAINING + seconds(30.0);
    for n in 0..total {
        // Our training, then following, then the retrain's training with the
        // far end quiet as 5.4 has it, then following again.
        let training = n < TRAINING || (retrain..retrain + TRAINING).contains(&n);
        cable.talking = !training;
        if n == TRAINING || n == retrain + TRAINING {
            ec.set_adapting(false);
            ec.follow_drift(true);
        }
        if n == retrain {
            ec.set_adapting(true);
        }
        let (x, heard, far) = cable.step(None);
        let left = ec.process(x, heard);
        if let Some(f) = finder.as_mut() {
            f.feed(x, left);
        }
        if n == TRAINING / 2 {
            let found = finder.take().and_then(|f| f.best()).expect("no echo found");
            ec.watch_far_echo(found.delay.saturating_sub(32).max(ec.span()), 64);
        }
        if n >= retrain + TRAINING {
            now.echo += (left - far) * (left - far);
            now.far += far * far;
            if (n + 1 - retrain - TRAINING).is_multiple_of(STRETCH) {
                stretches.push(now);
                now = Stretch::default();
            }
        }
    }
    let run = Run {
        stretches,
        jumps: ec.jumps(),
        ppm: ec.drift_ppm(),
    };
    let after = run.seconds(3);
    let worst = worst(&after);
    println!(
        "retrained at 100 ppm: fourth second {:.1} dB, median {:.1}, worst {worst:.1}; \
         drift read {:.1} ppm",
        after[0],
        median(&after),
        run.ppm
    );
    assert!(worst <= -28.0, "after the retrain the echo reached {worst:.1} dB");
    assert_eq!(run.jumps, 0);
}

#[test]
fn the_v32_modems_own_taps_are_followed_down_to_what_they_were_trained_to() {
    // The same cable with the modem's own layout, whose training is the
    // floor here, and at 100 ppm a high one. Following holds each drift near
    // it, where frozen the echo is as loud as the far end within seconds.
    for (ppm, bar) in [(0.0, -28.0), (20.0, -28.0), (100.0, -24.0)] {
        let run = run(Layout::V32, ppm, 30.0, true, &[]);
        let seconds = run.seconds(3);
        let worst = worst(&seconds);
        println!(
            "V.32 layout, {ppm} ppm: fourth second {:.1} dB, median {:.1}, worst {worst:.1}",
            seconds[0],
            median(&seconds)
        );
        assert!(
            worst <= bar,
            "with the V.32 layout at {ppm} ppm the residual echo reached {worst:.1} dB"
        );
        assert_eq!(run.jumps, 0);
    }
}

#[test]
fn frozen_the_canceller_loses_a_drifting_echo() {
    // The problem, measured the same way: without following, 20 ppm leaves
    // the echo louder than the far end within half a minute.
    let run = run(Layout::Cable, 20.0, 30.0, false, &[]);
    let seconds = run.seconds(3);
    let last = seconds[seconds.len() - 1];
    println!("frozen at 20 ppm: {:.1} dB in the fourth second, {last:.1} dB at 30 s", seconds[0]);
    assert!(last > -10.0, "frozen, the echo was still {last:.1} dB down");
}

#[test]
fn a_slip_or_an_underrun_is_found_again_within_a_tenth_of_a_second() {
    // A sample dropped, a sample repeated and a buffer of silence, on a
    // 20 ppm cable with the far end as loud as the echo.
    const AT: [(usize, Fault); 3] = [
        (8 * 16_000, Fault::Drop),
        (16 * 16_000, Fault::Repeat),
        (24 * 16_000, Fault::Gap(160)),
    ];
    // Measured over 50 ms, sliding by 10.
    const WINDOW: usize = 5;
    let run = run(Layout::Cable, 20.0, 32.0, true, &AT);
    for (k, (at, fault)) in AT.into_iter().enumerate() {
        // Counted from when the fault reaches the canceller. The input's
        // faults do at once; the output's silence has the cable to cross.
        let heard_at = match fault {
            Fault::Gap(_) => at + CROSSING,
            _ => at,
        };
        let fault_at = heard_at / STRETCH;
        let until = AT.get(k + 1).map_or(run.stretches.len(), |f| f.0 / STRETCH) - WINDOW;
        // From the end of the last window over 23 dB under, to the next fault.
        let recovered = (fault_at..until)
            .rev()
            .find(|&s| run.residual(s, WINDOW) > -23.0)
            .map_or(fault_at, |s| s + WINDOW);
        let recovered_ms = (recovered - fault_at) * 10;
        let settled = run.residual(fault_at + PER_SECOND / 5, PER_SECOND);
        println!(
            "{fault:?}: under 23 dB in every 50 ms from {recovered_ms} ms after; \
             the second from 200 ms after {settled:.1} dB"
        );
        assert!(
            recovered_ms <= 100,
            "{fault:?}: 23 dB under the far end only from {recovered_ms} ms after"
        );
    }
    assert_eq!(run.jumps, 3, "three faults, {} jumps", run.jumps);
}

#[test]
fn with_no_echo_to_follow_following_changes_nothing() {
    // A VoIP call, whose network returns nothing measurable of what is sent.
    // Trained on a silent line the taps stay at nothing, the estimate is
    // nothing, and the far end comes out exactly as it went in.
    let mut far = Qam::new(0x9e37_79b9_7f4a_7c15);
    let mut sent = Qam::new(0x2545_f491_4f6c_dd1d);
    let mut ec = EchoCanceller::new(128, 0.5);
    ec.watch_far_echo(668, 64);
    for _ in 0..TRAINING {
        ec.process(sent.next(), 0.0);
    }
    ec.set_adapting(false);
    ec.follow_drift(true);
    for _ in 0..(10.0 * FS) as usize {
        let heard = 0.45 * far.next();
        assert_eq!(ec.process(sent.next(), heard), heard);
    }
    assert_eq!(ec.jumps(), 0);
    assert_eq!(ec.drift_ppm(), 0.0);
    assert!(!ec.is_holding());
}

#[test]
fn a_line_that_does_not_drift_is_cancelled_as_well_as_ever() {
    // An ordinary two-wire line: the near hybrid inside a millisecond, and
    // the far one a round trip away and 10 dB under the far modem, all on
    // one clock. Nothing drifts, so following can only cost: the loop's own
    // noise on the far reflection, 2 dB here, and nothing on the near one,
    // whose taps are never moved at all.
    let measure = |follow: bool| {
        let mut sent = Qam::new(0x2545_f491_4f6c_dd1d);
        let mut far = Qam::new(0x9e37_79b9_7f4a_7c15);
        let mut path = vec![0.0; 645];
        path[1] = 0.25;
        path[2] = 0.1;
        path[3] = -0.05;
        path[4] = 0.02;
        path[640] = 0.03;
        path[641] = 0.012;
        let mut history = VecDeque::from(vec![0.0; path.len()]);
        let mut ec = EchoCanceller::new(128, 0.5);
        let (mut echo, mut heard_far) = (0.0, 0.0);
        for n in 0..(20.0 * FS) as usize {
            let x = sent.next();
            history.pop_back();
            history.push_front(x);
            let echo_now: f64 = path.iter().zip(&history).map(|(h, x)| h * x).sum();
            if n == TRAINING / 2 {
                ec.watch_far_echo(640 - 32, 64);
            }
            if n == TRAINING {
                ec.set_adapting(false);
                ec.follow_drift(follow);
            }
            let y = if n >= TRAINING { 0.1 * far.next() } else { 0.0 };
            let left = ec.process(x, echo_now + y);
            if n >= TRAINING + 3 * FS as usize {
                echo += (left - y) * (left - y);
                heard_far += y * y;
            }
        }
        (10.0 * (echo / heard_far).log10(), ec.jumps())
    };
    let (frozen, _) = measure(false);
    let (followed, jumps) = measure(true);
    println!("no drift: frozen {frozen:.1} dB, following {followed:.1} dB, {jumps} jumps");
    assert!(
        followed <= frozen + 2.5 && followed <= -28.0,
        "following left {followed:.1} dB where frozen left {frozen:.1}"
    );
    assert_eq!(jumps, 0);
}
