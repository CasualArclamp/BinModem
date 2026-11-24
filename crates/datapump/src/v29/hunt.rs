//! Finding a V.29 burst: segment 2 heard, and the moment it turns into
//! segment 3.
//!
//! Table 5's segment 2 alternates two known points, A and B, for 128 symbol
//! intervals, and 8.2 starts segment 3 with "CDCDCDC". C is A turned half
//! round and D is B turned half round (Figure 4), so to a receiver the join
//! is the alternation reversed: exactly what V.32's S turning into S-bar is,
//! and just as good a time reference, though it lasts seven symbols rather
//! than sixteen.
//!
//! An alternation of two points is a signal that repeats every two symbols,
//! so its power lies on three lines and nowhere else: the carrier itself, the
//! mean of A and B, and the carrier plus and minus half the symbol rate, their
//! difference -- 1700, 500 and 2900 Hz on the line, where clause 11 puts the
//! pulse's 3 dB points. That is what is looked for, six symbols at a time
//! (three whole repeats, 40 samples at 16 kHz):
//!
//! - most of what arrives in the band is on the three lines;
//! - the carrier's line has a share of them that an alternation of V.29's A
//!   and B can have, between 0.13 and 0.71 of it for a pulse whose band edge
//!   is anywhere clause 11 allows -- which a plain carrier (all of it, as
//!   talker-echo protection sends) and a tone off the carrier (none of it, as
//!   the answer tone and V.21 are) cannot;
//! - the six symbols are the six before them again, turned by no more than a
//!   carrier offset of 25 Hz would turn them.
//!
//! Noise has no lines, so no level of it passes, and nothing here is measured
//! against a level at all: a line full of hiss is not a burst however loud it
//! is (fax-qam.md 3.1). Twenty-four symbols of it in a row, a fifth of
//! segment 2, and a burst is arriving.
//!
//! The join is where the six symbols stop being the six before them and
//! become the same six turned half round. Their correlation falls from one to
//! minus one as the newer six cross it, and is nought when half of them have:
//! that crossing, found to a fraction of a sample, puts segment 3's first
//! symbol within a sample or two, and the least-squares training searches
//! either side of it anyway. And the carrier's offset comes from how far the
//! alternation turns from one stretch of it to the next, which needs nothing
//! decided: forty-two symbols apart for the fine figure, six apart to say
//! which turn of the fine one it is.

use std::collections::VecDeque;

use dsp::{Complex, ComplexFir, Nco, fir_lowpass};

use super::{BAUD, CARRIER};

/// Symbols in the window the lines are measured over: three repeats of the
/// alternation.
const WINDOW_SYMBOLS: f64 = 6.0;

/// Repeats of the window between the two stretches the fine frequency is
/// measured from: forty-two symbols, which is unambiguous to 28 Hz either
/// side of the coarse figure.
const FINE_LAG: f64 = 7.0;

/// Symbols of segment 2 in a row before a burst is taken to be arriving.
const ARMING_SYMBOLS: f64 = 24.0;

/// The least share of the band's energy the three lines must carry.
const LINE_SHARE: f64 = 0.6;

/// The carrier line's share of the three: an alternation of V.29's A and B
/// through a pulse whose band edge is 2 to 7 dB down (clause 11) has 0.13 to
/// 0.71 of it, and a line that loses more at 500 and 2900 Hz than at 1700
/// moves it up; a plain carrier has all of it, less the noise's share, and a
/// tone off the carrier none.
const CARRIER_SHARE: (f64, f64) = (0.04, 0.95);

/// How like the six symbols before them the newest six must be, and the most
/// carrier offset that may turn them apart.
const REPEATS: f64 = 0.6;
const WIDEST_OFFSET_HZ: f64 = 25.0;

/// How far the correlation must fall below nought to be the join, and the
/// least share of the lines the reversed six must still carry at the bottom.
const REVERSED: f64 = -0.5;
const REVERSED_LEAST: f64 = -0.7;
const REVERSED_SHARE: f64 = 0.5;

/// How long segment 2 may be missing before it is taken to have ended some
/// other way than into segment 3: longer than a twenty-millisecond
/// concealment, which the alternation is heard again after.
const HOLE_SECONDS: f64 = 0.040;

/// How long the level the carrier detector reads is averaged over.
const LEVEL_SECONDS: f64 = 0.010;

/// Samples kept, of the line and of the baseband: 256 ms at 16 kHz, enough
/// for the whole of segment 2 and a lapse's wait behind it.
const KEPT: usize = 4096;

/// The quietest a window may be and still be listened to: 100 dB under a
/// burst, so only exact silence is refused.
const AUDIBLE: f64 = 1e-11;

/// How loud segment 2 was, in the baseband's units: a line signal's
/// amplitude squared.
///
/// And how much of that was the line's own noise. Segment 2 is all on its
/// three lines and noise is everywhere in the band, so what is not on the
/// lines is noise, and the noise on them is the lines' share of the band's
/// width. A burst is over when its signal has fallen 6 dB, which on a line
/// with the noise half as loud as the burst is not the level falling 6 dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Level {
    pub(super) power: f64,
    pub(super) noise: f64,
}

/// What the hunt has found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Found {
    /// Segment 2, long enough to be sure of: a burst is arriving.
    Alternations { level: Level },
    /// Segment 2 turned into segment 3, whose first symbol is centred on line
    /// sample `at` (counted from the first sample taken, the first being
    /// nought). `turn` is the carrier's turn a symbol against ours, in
    /// radians, as segment 2 had it.
    Reversal { at: f64, turn: f64, level: Level },
    /// Segment 2 stopped without turning into segment 3. `at` is where its
    /// first symbol would have been, had segment 3 begun as segment 2 ended.
    Lapsed { at: f64, turn: f64, level: Level },
}

/// A sum over the last `len` values pushed.
#[derive(Debug, Clone)]
struct Sliding {
    values: VecDeque<Complex>,
    len: usize,
    sum: Complex,
    pushed: usize,
}

impl Sliding {
    fn new(len: usize) -> Self {
        Self { values: VecDeque::with_capacity(len + 1), len, sum: Complex::ZERO, pushed: 0 }
    }

    fn push(&mut self, value: Complex) -> Complex {
        self.values.push_back(value);
        self.sum += value;
        if self.values.len() > self.len {
            let old = self.values.pop_front().unwrap_or(Complex::ZERO);
            self.sum -= old;
        }
        // Added and taken away for ever, the sum drifts from what it is the
        // sum of; so every so often it is added up again.
        self.pushed += 1;
        if self.pushed.is_multiple_of(64 * self.len) {
            self.sum = self.values.iter().fold(Complex::ZERO, |s, v| s + *v);
        }
        self.sum
    }
}

/// What one window of six symbols looks like.
#[derive(Debug, Clone, Copy)]
struct Look {
    /// Share of the band's energy on the three lines, and the carrier's share
    /// of that.
    share: f64,
    carrier: f64,
    /// The window's correlation with the one before it, over their powers.
    repeats: Complex,
    /// That correlation unnormalised, and the window's mean power.
    lag: Complex,
    power: f64,
}

impl Look {
    /// The window's power that is not on the three lines.
    fn off(&self) -> f64 {
        (1.0 - self.share).max(0.0) * self.power
    }

    /// Whether the window carries an alternation's three lines, `share` of
    /// its energy at least.
    fn lines(&self, share: f64) -> bool {
        self.power > AUDIBLE && self.share > share && (CARRIER_SHARE.0..=CARRIER_SHARE.1).contains(&self.carrier)
    }
}

#[derive(Debug, Clone)]
struct Armed {
    /// The first sample of the run of windows that armed it, and the last
    /// window that was still segment 2.
    from: u64,
    last_good: u64,
    /// The correlation of each window of segment 2 with the one before, added
    /// up: its angle is the carrier's turn over a window.
    lag: Complex,
    /// Segment 2's windows' power, and the part of it off the three lines,
    /// added up.
    energy: f64,
    off: f64,
    windows: f64,
    /// The last correlation, and where it last crossed from nought or more to
    /// below it, to a fraction of a sample.
    previous: f64,
    crossing: Option<f64>,
    dip: Option<Dip>,
}

/// The correlation gone below [`REVERSED`], and its lowest point so far.
#[derive(Debug, Clone, Copy)]
struct Dip {
    crossing: f64,
    least: f64,
    at: u64,
    lines: bool,
}

/// The hunt for segment 2 and its end.
#[derive(Debug, Clone)]
pub(super) struct Hunt {
    fs: f64,
    sps: f64,
    /// Samples in a window, exactly and rounded; samples between the fine
    /// frequency's two stretches.
    period: f64,
    window: usize,
    fine: usize,
    mixer: Nco,
    select: ComplexFir,
    /// The select filter's delay, in samples.
    delay: f64,
    /// Half the symbol rate, which the side lines are taken down by.
    side: Nco,
    /// Samples taken.
    taken: u64,
    /// The line as it came, for reading again into a receiver.
    raw: VecDeque<f64>,
    /// The baseband, and whether the window ending at each sample was segment
    /// 2, newest last; the first is sample `taken - len`.
    base: VecDeque<Complex>,
    good: VecDeque<bool>,
    carrier_line: Sliding,
    upper: Sliding,
    lower: Sliding,
    lag: Sliding,
    energy: Sliding,
    older: Sliding,
    /// The baseband's power, over [`LEVEL_SECONDS`].
    power: f64,
    keep: f64,
    /// Windows of segment 2 in a row; their power, the part of it off the
    /// lines and their correlations with the windows before them, added up.
    run: usize,
    run_energy: f64,
    run_off: f64,
    run_lag: Complex,
    /// The share of the band's noise that falls on the three lines.
    on_lines: f64,
    armed: Option<Armed>,
}

impl Hunt {
    pub(super) fn new(fs: f64) -> Self {
        let sps = fs / BAUD;
        let period = WINDOW_SYMBOLS * sps;
        let window = period.round() as usize;
        let fine = (FINE_LAG * period).round() as usize;
        // The band is the carrier and 1500 Hz either side. This has to keep
        // the mixer's image and the noise beyond the band out of the shares
        // -- the image of segment 2's 500 Hz line lands at -2200 Hz -- and
        // leave the side lines at 1200 Hz as they came, since what they are
        // against the carrier's line is what is judged. Linear phase, so the
        // join is where it was, only later.
        let taps = fir_lowpass(1600.0, (fs / 200.0) as usize | 1, fs);
        let delay = (taps.len() / 2) as f64;
        // White noise through the filter lands on each of a window's
        // frequencies by the filter's gain there, so that is the share of it
        // the lines take.
        let gain = |hz: f64| {
            let w = std::f64::consts::TAU * hz / fs;
            let response = taps.iter().enumerate().fold(Complex::ZERO, |sum, (i, t)| sum + Complex::from_polar(*t, -w * i as f64));
            response.norm_sqr()
        };
        let bins: f64 = (0..window).map(|k| gain(k as f64 * fs / window as f64)).sum();
        let on_lines = (gain(0.0) + 2.0 * gain(BAUD / 2.0)) / bins;
        Self {
            fs,
            sps,
            period,
            window,
            fine,
            mixer: Nco::new(CARRIER, fs),
            select: ComplexFir::new(taps),
            delay,
            side: Nco::new(BAUD / 2.0, fs),
            taken: 0,
            raw: VecDeque::with_capacity(KEPT + 1),
            base: VecDeque::with_capacity(KEPT + 1),
            good: VecDeque::with_capacity(KEPT + 1),
            carrier_line: Sliding::new(window),
            upper: Sliding::new(window),
            lower: Sliding::new(window),
            lag: Sliding::new(window),
            energy: Sliding::new(window),
            older: Sliding::new(window),
            power: 0.0,
            keep: (-1.0 / (LEVEL_SECONDS * fs)).exp(),
            run: 0,
            run_energy: 0.0,
            run_off: 0.0,
            run_lag: Complex::ZERO,
            on_lines,
            armed: None,
        }
    }

    /// Samples taken so far.
    pub(super) fn taken(&self) -> u64 {
        self.taken
    }

    /// The line sample `index` as it came, if it is still kept.
    pub(super) fn raw(&self, index: u64) -> Option<f64> {
        let first = self.taken - self.raw.len() as u64;
        self.raw.get(index.checked_sub(first)? as usize).copied()
    }

    /// The first line sample still kept.
    pub(super) fn first_kept(&self) -> u64 {
        self.taken - self.raw.len() as u64
    }

    /// The in-band power of what is arriving, over the last ten milliseconds,
    /// in the baseband's units: a line signal's amplitude squared.
    pub(super) fn power(&self) -> f64 {
        self.power
    }

    /// The newest baseband sample, a line signal's own amplitude and phase.
    pub(super) fn newest(&self) -> Complex {
        self.base.back().copied().unwrap_or(Complex::ZERO)
    }

    /// Whether segment 2 is arriving, or was a moment ago.
    pub(super) fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Take one sample of the line.
    pub(super) fn feed(&mut self, sample: f64) -> Option<Found> {
        let n = self.taken;
        self.taken += 1;
        self.raw.push_back(sample);
        if self.raw.len() > KEPT {
            self.raw.pop_front();
        }
        let (cos, sin) = self.mixer.step();
        let (re, im) = self.select.process((2.0 * sample * cos, -2.0 * sample * sin));
        let b = Complex::new(re, im);
        self.power = self.keep * self.power + (1.0 - self.keep) * b.norm_sqr();
        let (cos, sin) = self.side.step();
        let down = Complex::new(cos, -sin);
        self.base.push_back(b);
        if self.base.len() > KEPT {
            self.base.pop_front();
            self.good.pop_front();
        }
        let before = n.checked_sub(self.window as u64).and_then(|m| self.baseband(m)).unwrap_or(Complex::ZERO);
        let dc = self.carrier_line.push(b);
        let up = self.upper.push(b * down);
        let low = self.lower.push(b * down.conj());
        let lag = self.lag.push(b * before.conj());
        let energy = self.energy.push(Complex::new(b.norm_sqr(), 0.0)).re;
        let older = self.older.push(Complex::new(before.norm_sqr(), 0.0)).re;
        let w = self.window as f64;
        let lines = dc.norm_sqr() + up.norm_sqr() + low.norm_sqr();
        let look = Look {
            share: lines / (w * energy).max(1e-300),
            carrier: dc.norm_sqr() / lines.max(1e-300),
            repeats: lag.scale(1.0 / (energy * older).sqrt().max(1e-300)),
            lag,
            power: energy / w,
        };
        let limit = std::f64::consts::TAU * WIDEST_OFFSET_HZ * self.period / self.fs;
        let good = look.lines(LINE_SHARE) && look.repeats.abs() > REPEATS && look.repeats.arg().abs() < limit;
        self.good.push_back(good);
        self.follow(n, good, &look)
    }

    /// Baseband sample `index`, if it is kept.
    fn baseband(&self, index: u64) -> Option<Complex> {
        let first = self.taken - self.base.len() as u64;
        self.base.get(index.checked_sub(first)? as usize).copied()
    }

    /// Whether the window ending at sample `index` was segment 2.
    fn was_good(&self, index: u64) -> bool {
        let first = self.taken - self.good.len() as u64;
        index.checked_sub(first).and_then(|i| self.good.get(i as usize)).copied().unwrap_or(false)
    }

    fn follow(&mut self, n: u64, good: bool, look: &Look) -> Option<Found> {
        let Some(mut armed) = self.armed.take() else {
            if good {
                self.run += 1;
                self.run_energy += look.power;
                self.run_off += look.off();
                self.run_lag += look.lag;
            } else {
                self.run = 0;
                self.run_energy = 0.0;
                self.run_off = 0.0;
                self.run_lag = Complex::ZERO;
            }
            if self.run as f64 >= ARMING_SYMBOLS * self.sps {
                let level = self.level(self.run_energy, self.run_off, self.run as f64);
                self.armed = Some(Armed {
                    from: n + 1 - self.run as u64,
                    last_good: n,
                    lag: self.run_lag,
                    energy: self.run_energy,
                    off: self.run_off,
                    windows: self.run as f64,
                    previous: 1.0,
                    crossing: None,
                    dip: None,
                });
                self.run = 0;
                self.run_energy = 0.0;
                self.run_off = 0.0;
                self.run_lag = Complex::ZERO;
                return Some(Found::Alternations { level });
            }
            return None;
        };
        if good {
            armed.last_good = n;
            armed.lag += look.lag;
            armed.energy += look.power;
            armed.off += look.off();
            armed.windows += 1.0;
        }
        // The correlation with the window before, turned back by the turn
        // segment 2 has shown so far: one while it goes on, minus one once the
        // newest six symbols are the same six turned half round.
        let turned = (look.repeats * armed.lag.conj()).re / armed.lag.abs().max(1e-300);
        if turned < 0.0 && armed.previous >= 0.0 {
            armed.crossing = Some((n - 1) as f64 + armed.previous / (armed.previous - turned));
        }
        armed.previous = turned;
        match armed.dip {
            None if turned < REVERSED => {
                if let Some(crossing) = armed.crossing {
                    armed.dip = Some(Dip { crossing, least: turned, at: n, lines: look.lines(REVERSED_SHARE) });
                }
            }
            None => {}
            Some(mut dip) => {
                if turned < dip.least {
                    dip.least = turned;
                    dip.at = n;
                    dip.lines = look.lines(REVERSED_SHARE);
                }
                if turned > dip.least + 0.25 || n - dip.at > self.window as u64 / 4 {
                    // The bottom of the dip: the newest six reversed and the
                    // six before them still segment 2, and the reversed six
                    // still carrying the lines.
                    armed.dip = None;
                    let before = self.was_good(dip.at.saturating_sub(self.window as u64));
                    if dip.least < REVERSED_LEAST && dip.lines && before {
                        // Of the products the correlation adds up, those past
                        // the join have turned over: it is nought when the
                        // newest window holds as many of them as not, which
                        // is when the join is a sample less than half a
                        // window behind the newest sample.
                        let join = dip.crossing - (self.window as f64 / 2.0 - 1.0);
                        let turn = self.turn(&armed, (join as u64).saturating_sub(2));
                        let join = join - self.delay;
                        let level = self.level(armed.energy, armed.off, armed.windows);
                        return Some(Found::Reversal { at: join + self.sps / 2.0, turn, level });
                    }
                } else {
                    armed.dip = Some(dip);
                }
            }
        }
        if (n - armed.last_good) as f64 > HOLE_SECONDS * self.fs {
            // Segment 2 has gone, and not into segment 3: a slip across the
            // join, or a far end that ended it some other way. Where it
            // stopped is where segment 3 is guessed to begin, and the training
            // searches wide around it.
            let join = armed.last_good as f64 - self.delay;
            let turn = self.turn(&armed, armed.last_good);
            let level = self.level(armed.energy, armed.off, armed.windows);
            return Some(Found::Lapsed { at: join + self.sps / 2.0, turn, level });
        }
        self.armed = Some(armed);
        None
    }

    /// Segment 2's level, from its windows' power and the part of it off the
    /// lines, added up over `windows` of them.
    fn level(&self, energy: f64, off: f64, windows: f64) -> Level {
        let power = energy / windows;
        let noise = (off / windows / (1.0 - self.on_lines)).min(power);
        Level { power, noise }
    }

    /// The carrier's turn a symbol, in radians, from segment 2 up to baseband
    /// sample `end`: the fine figure from stretches forty-two symbols apart,
    /// unwrapped around the coarse one from windows six apart.
    fn turn(&self, armed: &Armed, end: u64) -> f64 {
        let coarse = armed.lag.arg() / self.window as f64;
        let mut sum = Complex::ZERO;
        let mut count = 0usize;
        let lag = self.fine as u64;
        for m in (armed.from + lag)..end {
            if self.was_good(m)
                && self.was_good(m - lag)
                && let (Some(now), Some(then)) = (self.baseband(m), self.baseband(m - lag))
            {
                sum += now * then.conj();
                count += 1;
            }
        }
        let per_sample = if count >= self.window {
            let lag = self.fine as f64;
            let wraps = ((coarse * lag - sum.arg()) / std::f64::consts::TAU).round();
            (sum.arg() + wraps * std::f64::consts::TAU) / lag
        } else {
            coarse
        };
        per_sample * self.sps
    }
}
