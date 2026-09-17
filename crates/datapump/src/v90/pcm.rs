//! The analogue modem's downstream receiver: PCM codewords back out of what
//! the far codec made of them.
//!
//! There is no carrier to take off and no constellation plane. The digital
//! modem hands the network one codeword every 125 microseconds, the codec at
//! the far end of the loop turns each into a voltage, and the line smears the
//! voltages together. So the receiver is an interpolator making two samples a
//! symbol on a clock it steers, an equaliser that undoes the smear, and a
//! decision about which of the codewords the result is nearest.
//!
//! Three jobs, one after another, as V.34's receiver does them.
//!
//! Hunting Sd. Sd repeats every six symbols -- {+W, +0, +W, -W, -0, -W}
//! (8.4.4) -- and S-bar-d is the same turned over. A half-symbol sample set
//! against the one six symbols before matches all through Sd and is its
//! opposite across the change, whatever the timing and the line. And the
//! change falls on a data frame boundary: Sd starts in data frame interval 0
//! and lasts 64 frames, so where it turns over is where frames start.
//!
//! Training. TRN1d follows S-bar-d at once, and is known from its first
//! symbol: UINFO, signed by a scrambler started at zero with ones going in
//! (8.4.5). So the equaliser is solved for outright, by least squares, at
//! each alignment near where the reversal put it, and the best kept -- and it
//! comes out scaled, because the sequence is the actual codeword and not just
//! its sign. Read off a real server on a real line, this finds TRN1d, reads
//! Jd with its CRC checking, and lines the DIL the modem asked for up against
//! what arrived.
//!
//! Tracking. From there the equaliser follows by normalised least mean
//! squares on the decisions, and the clock by the error's share along the
//! output's rate of change. The line a real server was recorded on ran 73 ppm
//! off the recorder, which is a symbol every 1.7 seconds: an equaliser left to
//! absorb that walks off its own end.

use std::collections::VecDeque;

use crate::v32::{Mode, Scrambler};

use super::INTERVALS;
use super::ucode::{self, Law};

/// Symbols a second, from the network (5.2).
pub const BAUD: f64 = 8000.0;

/// The interpolator: taps, and the fractional positions its table holds.
const FILTER_TAPS: usize = 32;
const FILTER_PHASES: usize = 512;

/// Equaliser taps either side of the centre, in half symbols: 63 taps.
pub const REACH: usize = 31;

/// Decisions fed back, in symbols.
///
/// A linear equaliser alone cannot give back a band the line has taken away,
/// and every line takes some: the codec's reconstruction filter cuts off
/// short of 4 kHz, and so does whatever resamples a VoIP call's audio. On a
/// line that loses 3.8 to 4 kHz and nothing else, a linear equaliser settles
/// at 27 dB however clean the line is. What it cannot undo after the symbol
/// is known exactly once the symbol is decided, because a decision is a
/// codeword, so it is taken off instead of equalised.
pub const FEEDBACK: usize = 24;

/// Half-symbol samples kept.
const KEPT: usize = 8192;

/// Line kept, in seconds: enough to sample the whole of training again once
/// its clock is known.
const HISTORY_SECONDS: f64 = 1.0;


/// Sd's period, in half-symbol samples.
const PERIOD: usize = 2 * INTERVALS;

/// Symbols of S-bar-d, between the reversal and TRN1d (8.4.4).
const SD_BAR_SYMBOLS: usize = 48;

/// Symbols of TRN1d trained on: the first few left out while the line's
/// memory of S-bar-d clears, and the rest well inside the 2040 symbols TRN1d
/// is sent for at least (9.3.1.4) -- which 9.3.2.5 has the analogue modem
/// train on.
const TRAIN_FROM: usize = 64;
const TRAIN_TO: usize = 1600;

/// Stretches of TRN1d tried before the training is given up, and how far
/// either way of where it should be a later one is looked for, in symbols:
/// forty milliseconds.
const TRAIN_TRIES: u32 = 3;
const COARSE_SYMBOLS: i64 = 320;

/// Half symbols either side of where the reversal puts TRN1d that training
/// tries.
const SEARCH: i64 = 12;

/// Where the alignment search looks: a short stretch, for choosing, before
/// the full solve at the one chosen.
const SEARCH_FROM: usize = TRAIN_FROM;
const SEARCH_TO: usize = 448;

/// Signal to noise below which training on TRN1d is taken to have trained on
/// something else.
const KNOWN_ENOUGH: f64 = 9.0;

/// Least power a half-symbol sample has to carry to be Sd: 50 dB under full
/// scale.
const AUDIBLE: f64 = 1e-5;

/// Normalised least-mean-squares step.
const STEP: f64 = 0.004;

/// Timing loop gains, a symbol at a time.
const TIMING_GAIN: f64 = 0.004;
const DRIFT_GAIN: f64 = 2e-6;

/// Symbols the decisions' recent error is judged over, and how many times
/// what it settled to it has to reach before the signal is taken to have
/// jumped. A dense constellation reads garbage at about a twelfth of a
/// spacing squared, eight times the noise it was built for.
const JUDGED: usize = 32;
const LOST_AT: f64 = 4.0;
const FOUND_AT: f64 = 2.0;

/// Symbols of holding still after which the errors are the line's own: half
/// a second, where a slip's burst is gone in a few tens of milliseconds.
const HELD_AT_MOST: u32 = 4000;

/// Symbols between looks at where the equaliser's weight has got to, and how
/// much of its movement goes into the clock's rate.
const CENTRE_EVERY: u64 = 16;
const CENTRE_DRIFT_GAIN: f64 = 0.3;

/// What the decisions that keep the loops going are made against.
#[derive(Debug, Clone, PartialEq)]
pub enum Slicer {
    /// One codeword, either sign: TRN1d, Jd, J'd and R.
    Binary(f64),
    /// The symbols the caller has said are coming, in order, and then
    /// nothing to learn from until it says more: a DIL. A symbol given as
    /// NaN is one not to learn from. `first` is the receiver's count at the
    /// first of them, so that they stay with the symbols they were said for
    /// however many are decided at once.
    Known { first: u64, levels: VecDeque<f64> },
    /// The nearest of each data frame interval's signed levels, largest
    /// first: phase 4 and data mode.
    Levels(Box<[Vec<f64>; INTERVALS]>),
    /// Nothing to learn from.
    Free,
}

impl Slicer {
    /// The level to learn from for symbol `index`, in `interval`, if there
    /// is one.
    fn decide(&mut self, y: f64, interval: usize, index: u64) -> Option<f64> {
        match self {
            Self::Known { first, levels } => {
                if index < *first {
                    return None;
                }
                for _ in *first..index {
                    levels.pop_front();
                }
                *first = index + 1;
                levels.pop_front().filter(|v| v.is_finite())
            }
            Self::Binary(level) => Some(if y < 0.0 { -*level } else { *level }),
            Self::Levels(levels) => nearest(&levels[interval], y),
            Self::Free => None,
        }
    }
}

/// The nearest of `levels`, sorted either way, to `y`.
fn nearest(levels: &[f64], y: f64) -> Option<f64> {
    levels.iter().copied().min_by(|a, b| (a - y).abs().total_cmp(&(b - y).abs()))
}

/// What the receiver has to report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Heard {
    /// Sd, and then the change to S-bar-d at about this half-symbol sample.
    Reversal { at: u64 },
    /// Trained on TRN1d, with the signal to noise the training left and
    /// whether the line turns the signal over.
    Trained { snr_db: f64, inverted: bool },
    /// Nothing trained: TRN1d was not where the reversal said.
    Untrained,
    Symbol(Symbol),
    /// The decisions went bad all at once -- a jitter buffer's slip, most
    /// likely -- and the loops are holding still.
    Lost,
    /// And they are good again.
    Found,
}

/// One symbol, equalised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Symbol {
    /// Symbols since TRN1d began, TRN1d's first being 0, and moved on by
    /// the frame offset: what [`Self::interval`] is worked out from.
    pub index: u64,
    /// The same count as this receiver kept it, with no offset.
    pub raw: u64,
    /// The equaliser's output, in the scale of [`ucode::level`].
    pub value: f64,
    /// What the slicer decided it was, if it decided.
    pub decided: Option<f64>,
}

impl Symbol {
    /// The data frame interval it was sent in. Sd began in interval 0 and
    /// TRN1d follows 72 whole frames later (8.4.4).
    pub fn interval(&self) -> usize {
        (self.index % INTERVALS as u64) as usize
    }

    /// Whether it was sent positive.
    pub fn positive(&self) -> bool {
        self.decided.unwrap_or(self.value) >= 0.0
    }
}

#[derive(Debug, Clone)]
enum Stage {
    Idle,
    Hunting(Hunt),
    /// TRN1d arriving: the half-symbol sample its first symbol is near, the
    /// symbol of it the training stretch begins at, and how many stretches
    /// have been tried.
    Collecting { start: u64, base: usize, tries: u32 },
    Trained,
}

/// Looks for Sd and its change to S-bar-d in half-symbol samples.
#[derive(Debug, Clone, Default)]
struct Hunt {
    recent: VecDeque<f64>,
    /// The last period's products with a period before, and powers.
    products: VecDeque<(f64, f64)>,
    held: usize,
    /// Halves left in which a fall from Sd is still taken for its change.
    armed: usize,
    /// Where the match went through zero on the way down.
    crossed: Option<u64>,
}

impl Hunt {
    /// Halves of Sd in a row before a reversal is looked for: eight periods.
    const HELD: usize = 8 * PERIOD;

    /// One half-symbol sample. The index of the first sample of S-bar-d, if
    /// this is where the change is sure.
    fn feed(&mut self, half: f64, index: u64) -> Option<u64> {
        self.recent.push_back(half);
        if self.recent.len() <= PERIOD {
            return None;
        }
        let before = self.recent.pop_front().unwrap_or(0.0);
        self.products.push_back((half * before, half * half));
        if self.products.len() > PERIOD {
            self.products.pop_front();
        }
        let (together, power) = self.products.iter().fold((0.0, 0.0), |(c, p), &(x, y)| (c + x, p + y));
        let share = together / power.max(1e-30);
        let mean_power = power / self.products.len() as f64;
        if mean_power < AUDIBLE {
            self.held = 0;
            self.armed = 0;
            return None;
        }
        if share > 0.7 {
            self.held += 1;
            if self.held >= Self::HELD {
                self.armed = PERIOD + PERIOD / 2;
                self.crossed = None;
            }
            return None;
        }
        self.held = 0;
        if self.armed == 0 {
            return None;
        }
        // S-bar-d is Sd turned over, which for a pattern this symmetric is
        // also Sd half a period on: so the match does not flip, it falls, one
        // sample at a time as the turned samples come into the window. It is
        // through zero when half of them are in.
        if share < 0.0 && self.crossed.is_none() {
            self.crossed = Some(index);
        }
        if share < -0.5 {
            self.armed = 0;
            let crossed = self.crossed.unwrap_or(index);
            return Some(crossed.saturating_sub(PERIOD as u64 / 2 - 1));
        }
        self.armed -= 1;
        None
    }
}

/// An equaliser training came to.
#[derive(Debug, Clone)]
struct Solution {
    taps: Vec<f64>,
    feedback: Vec<f64>,
    /// The half-symbol sample TRN1d's first symbol is centred on.
    origin: u64,
    mse: f64,
}

/// The downstream receiver.
#[derive(Debug, Clone)]
pub struct Receiver {
    law: Law,
    /// Line samples, newest last, and the index of the oldest.
    history: VecDeque<f64>,
    history_first: u64,
    history_kept: usize,
    taken: u64,
    /// Where the next half-symbol sample falls, in line samples, and how far
    /// apart they are with the timing loop's correction.
    due: f64,
    half: f64,
    drift: f64,
    table: Vec<f64>,

    halves: VecDeque<f64>,
    /// Where on the line each half-symbol sample was taken.
    times: VecDeque<f64>,
    first: u64,
    made: u64,

    stage: Stage,
    heard: VecDeque<Heard>,
    uinfo: u8,

    taps: Vec<f64>,
    feedback: Vec<f64>,
    /// The levels decided for the last symbols, newest first.
    past: VecDeque<f64>,
    /// The half-symbol sample the next symbol is centred on, and its index.
    next_half: u64,
    next_symbol: u64,
    slicer: Slicer,
    /// Mean squared size of the output's rate of change.
    slope: f64,
    /// Mean squared error of the decisions, and what training left.
    error: f64,
    trained_snr: f64,
    inverted: bool,
    /// Everything the timing loop has moved the clock by, in line samples.
    timed: f64,
    /// Where training left the equaliser's weight, in taps.
    centre: f64,
    /// Added to a symbol's index to give its data frame interval, once a slip
    /// has moved the frames.
    frame_offset: u64,
    /// The last symbols' squared errors, what they settle to, and whether the
    /// loops are holding.
    recent: VecDeque<f64>,
    settled: f64,
    lost: bool,
    held: u32,
    slips: u32,
}

impl Receiver {
    /// A receiver taking line samples at `fs`, for a network using `law`.
    pub fn new(law: Law, fs: f64) -> Self {
        let cutoff = (0.45 * fs).min(7200.0);
        let mut table = vec![0.0; FILTER_PHASES * FILTER_TAPS];
        for ph in 0..FILTER_PHASES {
            let row = &mut table[ph * FILTER_TAPS..(ph + 1) * FILTER_TAPS];
            for (i, tap) in row.iter_mut().enumerate() {
                let tau = ph as f64 / FILTER_PHASES as f64 + (FILTER_TAPS / 2) as f64 - 1.0 - i as f64;
                let x = 2.0 * cutoff * tau / fs;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let edge = (FILTER_TAPS / 2) as f64;
                let taper = if tau.abs() >= edge { 0.0 } else { kaiser(tau / edge, 8.0) };
                *tap = sinc * taper;
            }
            let sum: f64 = row.iter().sum();
            for tap in row.iter_mut() {
                *tap /= sum;
            }
        }
        let mut taps = vec![0.0; 2 * REACH + 1];
        taps[REACH] = 1.0;
        Self {
            law,
            history: VecDeque::with_capacity((HISTORY_SECONDS * fs) as usize),
            history_first: 0,
            history_kept: (HISTORY_SECONDS * fs) as usize,
            taken: 0,
            due: FILTER_TAPS as f64,
            half: fs / BAUD / 2.0,
            drift: 0.0,
            table,
            halves: VecDeque::with_capacity(KEPT),
            times: VecDeque::with_capacity(KEPT),
            first: 0,
            made: 0,
            stage: Stage::Idle,
            heard: VecDeque::new(),
            uinfo: 0,
            taps,
            feedback: vec![0.0; FEEDBACK],
            past: VecDeque::from(vec![0.0; FEEDBACK]),
            next_half: 0,
            next_symbol: 0,
            slicer: Slicer::Free,
            slope: 1.0,
            error: 1.0,
            trained_snr: 0.0,
            inverted: false,
            timed: 0.0,
            centre: 0.0,
            frame_offset: 0,
            recent: VecDeque::with_capacity(JUDGED),
            settled: 0.0,
            lost: false,
            held: 0,
            slips: 0,
        }
    }

    pub fn law(&self) -> Law {
        self.law
    }

    /// Listen for Sd, to train on the TRN1d of `uinfo` after it.
    pub fn hunt(&mut self, uinfo: u8) {
        self.uinfo = uinfo;
        self.stage = Stage::Hunting(Hunt::default());
    }

    /// Stop making symbols.
    pub fn idle(&mut self) {
        self.stage = Stage::Idle;
    }

    pub fn is_trained(&self) -> bool {
        matches!(self.stage, Stage::Trained)
    }

    /// What the loops learn from from here on, forgetting what the last one
    /// made of the errors.
    pub fn set_slicer(&mut self, slicer: Slicer) {
        self.slicer = slicer;
        self.recent.clear();
        self.settled = 0.0;
        self.lost = false;
    }

    /// Move where the data frames start, by `offset` symbols: a slip has put
    /// every symbol after it in a different interval.
    pub fn set_frame_offset(&mut self, offset: u64) {
        self.frame_offset = offset % INTERVALS as u64;
    }

    pub fn frame_offset(&self) -> u64 {
        self.frame_offset
    }

    /// Whether the loops are holding through a burst of bad decisions.
    pub fn is_lost(&self) -> bool {
        self.lost
    }

    /// Bursts held through so far.
    pub fn slips(&self) -> u32 {
        self.slips
    }

    /// Say what the symbols from the receiver's count `first` are, for
    /// [`Slicer::Known`], forgetting what was said before: a slip has moved
    /// them.
    pub fn expect_from(&mut self, first: u64, levels: impl IntoIterator<Item = f64>) {
        self.set_slicer(Slicer::Known { first, levels: levels.into_iter().collect() });
    }

    /// Say what the next symbols are, for [`Slicer::Known`].
    pub fn expect(&mut self, levels: impl IntoIterator<Item = f64>) {
        match &mut self.slicer {
            Slicer::Known { levels: queue, .. } => queue.extend(levels),
            other => *other = Slicer::Known { first: self.next_symbol, levels: levels.into_iter().collect() },
        }
    }

    /// Signal to noise of the decisions, against the training codeword's
    /// power.
    pub fn snr_db(&self) -> f64 {
        let level = ucode::level(self.law, self.uinfo);
        10.0 * (level * level / self.error.max(1e-18)).log10()
    }

    pub fn trained_snr_db(&self) -> f64 {
        self.trained_snr
    }

    /// Whether the line turns the signal over. Nothing in V.90 cares --
    /// everything signed is differential or comes with its opposite -- but
    /// it is worth knowing.
    pub fn inverted(&self) -> bool {
        self.inverted
    }

    /// How far off the far clock this end's is, as the timing loop has it.
    pub fn drift_ppm(&self) -> f64 {
        self.drift * 1e6
    }

    /// The equaliser, for looking at.
    pub fn taps(&self) -> &[f64] {
        &self.taps
    }

    pub fn heard(&mut self) -> Option<Heard> {
        self.heard.pop_front()
    }

    pub fn feed(&mut self, sample: f64) {
        self.history.push_back(sample);
        if self.history.len() > self.history_kept {
            self.history.pop_front();
            self.history_first += 1;
        }
        self.taken += 1;
        while let Some(value) = self.interpolate(self.due) {
            let at = self.due;
            self.due += self.half * (1.0 + self.drift);
            self.on_half(value, at);
        }
    }

    /// The line at `time` samples, filtered, if every sample the filter
    /// reaches is here.
    fn interpolate(&self, time: f64) -> Option<f64> {
        let floor = time.floor();
        let mut ph = ((time - floor) * FILTER_PHASES as f64).round() as usize;
        let mut base = floor as i64;
        if ph == FILTER_PHASES {
            ph = 0;
            base += 1;
        }
        let from = base - (FILTER_TAPS / 2) as i64 + 1;
        let to = base + (FILTER_TAPS / 2) as i64;
        if from < self.history_first as i64 || to >= self.taken as i64 {
            return None;
        }
        let offset = (from - self.history_first as i64) as usize;
        let row = &self.table[ph * FILTER_TAPS..(ph + 1) * FILTER_TAPS];
        Some(row.iter().enumerate().map(|(i, tap)| self.history[offset + i] * tap).sum())
    }

    fn on_half(&mut self, half: f64, at: f64) {
        let index = self.made;
        self.made += 1;
        self.halves.push_back(half);
        self.times.push_back(at);
        if self.halves.len() > KEPT {
            self.halves.pop_front();
            self.times.pop_front();
            self.first += 1;
        }
        match &mut self.stage {
            Stage::Idle => {}
            Stage::Hunting(hunt) => {
                if let Some(at) = hunt.feed(half, index) {
                    self.heard.push_back(Heard::Reversal { at });
                    // TRN1d follows S-bar-d's 48 symbols.
                    self.stage = Stage::Collecting { start: at + 2 * SD_BAR_SYMBOLS as u64, base: 0, tries: 0 };
                }
            }
            Stage::Collecting { start, base, tries } => {
                let (start, base, tries) = (*start, *base, *tries);
                // A later stretch is looked for a slip either way of where
                // it should be, and all of that has to have arrived.
                let reach = if tries > 0 { 2 * COARSE_SYMBOLS as u64 } else { 0 };
                let needed = start + 2 * base as u64 + reach + SEARCH as u64 + 2 * TRAIN_TO as u64 + REACH as u64 + 1;
                if self.made > needed {
                    self.finish_training(start, base, tries);
                }
            }
            Stage::Trained => self.symbols(),
        }
    }

    /// Every symbol whose samples are all here.
    fn symbols(&mut self) {
        while self.next_half + REACH as u64 + 2 <= self.made {
            if self.next_half < self.first + REACH as u64 + 1 {
                self.next_half += 2;
                self.next_symbol += 1;
                continue;
            }
            let symbol = self.symbol();
            self.heard.push_back(Heard::Symbol(symbol));
        }
    }

    /// The half-symbol samples around the one at `centre`, `reach` either
    /// side.
    fn samples(&self, centre: u64, reach: usize) -> Option<Vec<f64>> {
        let from = centre.checked_sub(reach as u64)?.checked_sub(self.first)? as usize;
        let to = from + 2 * reach + 1;
        (to <= self.halves.len()).then(|| self.halves.range(from..to).copied().collect())
    }

    /// The TRN1d symbols from `from` to `to`, as levels.
    fn trn1d(&self, from: usize, to: usize) -> Vec<f64> {
        let level = ucode::level(self.law, self.uinfo);
        let mut scrambler = Scrambler::new(Mode::Call);
        (0..to)
            .map(|_| if scrambler.scramble(true) { level } else { -level })
            .skip(from)
            .collect()
    }

    /// Where a stretch of known symbols sits, as half-symbol samples from
    /// `start`, by where it correlates best with the line within a slip's
    /// reach either way.
    fn coarse(&self, start: u64, targets: &[f64]) -> Option<i64> {
        let reach = 2 * COARSE_SYMBOLS;
        let mut best: Option<(f64, i64)> = None;
        for lag in -reach..=reach {
            let mut sum = 0.0;
            for (k, &target) in targets.iter().enumerate().skip(TRAIN_FROM) {
                let Some(at) = (start + 2 * k as u64).checked_add_signed(lag).and_then(|a| a.checked_sub(self.first)) else { continue };
                let Some(&v) = self.halves.get(at as usize) else { continue };
                sum += v * target;
            }
            if best.is_none_or(|(b, _)| sum.abs() > b) {
                best = Some((sum.abs(), lag));
            }
        }
        best.map(|(_, lag)| lag)
    }

    /// How far the far clock is off this one, as a fraction, from where the
    /// line's pulse sits in four stretches of TRN1d.
    ///
    /// TRN1d is as good as white, so the line set against it is the line's
    /// pulse, and a far clock running slow against this one moves the pulse a
    /// little later in this end's samples with every symbol. At 40 ppm that is
    /// a tenth of a half-symbol sample across training -- which a single fit
    /// over the whole of it smears into an error 25 dB up on a clean line.
    /// Where each stretch's peak is, to a fraction of a sample, is biased by
    /// the pulse's shape; the bias is the same in every stretch, and the line
    /// through them is not.
    fn drift_across(&self, origin: u64, targets: &[f64]) -> Option<f64> {
        let length = (TRAIN_TO - TRAIN_FROM) / 4;
        let pulse = |from: usize, lag: i64| -> Option<f64> {
            let mut sum = 0.0;
            for (k, &target) in targets.iter().enumerate().skip(from).take(length) {
                let at = (origin + 2 * k as u64).checked_add_signed(lag)?.checked_sub(self.first)? as usize;
                sum += self.halves.get(at)? * target;
            }
            Some(sum)
        };
        // The pulse's main lag, from the first stretch.
        let reach = REACH as i64;
        let peak = (-reach..=reach)
            .filter_map(|lag| pulse(TRAIN_FROM, lag).map(|v| (lag, v.abs())))
            .max_by(|a, b| a.1.total_cmp(&b.1))?
            .0;
        let mut points = Vec::with_capacity(4);
        for n in 0..4 {
            let from = TRAIN_FROM + n * length;
            let (a, b, c) = (pulse(from, peak - 1)?.abs(), pulse(from, peak)?.abs(), pulse(from, peak + 1)?.abs());
            let bend = a - 2.0 * b + c;
            let frac = if bend.abs() > 1e-30 { 0.5 * (a - c) / bend } else { 0.0 };
            // Half-symbol samples of where the stretch's middle is, and where
            // its peak is.
            points.push((2.0 * (from + length / 2) as f64, frac));
        }
        let n = points.len() as f64;
        let (mx, my) = points.iter().fold((0.0, 0.0), |(x, y), p| (x + p.0 / n, y + p.1 / n));
        let (sxy, sxx) = points.iter().fold((0.0, 0.0), |(xy, xx), p| (xy + (p.0 - mx) * (p.1 - my), xx + (p.0 - mx).powi(2)));
        Some(sxy / sxx.max(1e-30))
    }

    /// Take every half-symbol sample from `from` on again, on a clock `drift`
    /// off the line's, from the line kept. False if the line kept does not
    /// reach back that far.
    fn resample(&mut self, from: u64, drift: f64) -> bool {
        let Some(k0) = from.checked_sub(self.first).map(|k| k as usize) else { return false };
        let Some(&t0) = self.times.get(k0) else { return false };
        let step = self.half * (1.0 + drift);
        let count = self.halves.len() - k0;
        let mut again = Vec::with_capacity(count);
        for n in 0..count {
            let at = t0 + n as f64 * step;
            match self.interpolate(at) {
                Some(v) => again.push((v, at)),
                None => return false,
            }
        }
        for (n, (v, at)) in again.into_iter().enumerate() {
            self.halves[k0 + n] = v;
            self.times[k0 + n] = at;
        }
        self.drift = drift;
        self.due = t0 + count as f64 * step;
        true
    }

    /// Train on the stretch of TRN1d from symbol `base`, near half-symbol
    /// sample `start + 2 * base`.
    ///
    /// A stretch a softphone's jitter buffer cut into does not fit, and on a
    /// live call one did not. TRN1d can go on for four seconds, so a stretch
    /// that fails is followed by the next, a few times over, before the
    /// training is given up.
    fn finish_training(&mut self, first: u64, base: usize, tries: u32) {
        let mut start = first + 2 * base as u64;
        // A later stretch may not be where the first was: the cut that
        // spoiled the first moved everything after it.
        if tries > 0
            && let Some(lag) = self.coarse(start, &self.trn1d(base, base + TRAIN_TO))
        {
            start = start.saturating_add_signed(lag);
        }
        // The clock first, and the samples taken again on it, before the
        // equaliser is solved for. Where a peak sits between samples is read
        // a little wrong, and more wrong the further it has moved, so the
        // estimate is made again on the samples taken again, until what is
        // left is under half a part per million.
        if let Some(origin) = self.align(start, base) {
            let targets = self.trn1d(base, base + TRAIN_TO);
            for _ in 0..8 {
                let Some(off) = self.drift_across(origin, &targets) else { break };
                if off.abs() < 0.5e-6 {
                    break;
                }
                let from = origin.saturating_sub((SEARCH + REACH as i64 + 1) as u64);
                if !self.resample(from, self.drift + off) {
                    break;
                }
            }
        }
        match self.solve(start, base) {
            Some(solution) if -10.0 * (solution.mse / self.power_of_uinfo()).max(1e-18).log10() >= KNOWN_ENOUGH => {
                self.trained_snr = -10.0 * (solution.mse / self.power_of_uinfo()).max(1e-18).log10();
                // Which way round the line has the signal: the main tap's
                // sign, against a line that passes it straight through.
                let main = self.taps_peak(&solution.taps);
                self.inverted = main < 0.0;
                self.taps = solution.taps;
                self.centre = centre_of(&self.taps);
                self.feedback = solution.feedback;
                let known = self.trn1d(base + TRAIN_TO - FEEDBACK, base + TRAIN_TO);
                self.past = known.into_iter().rev().collect();
                self.error = solution.mse;
                self.next_half = solution.origin + 2 * TRAIN_TO as u64;
                self.next_symbol = (base + TRAIN_TO) as u64;
                self.slicer = Slicer::Binary(ucode::level(self.law, self.uinfo));
                self.stage = Stage::Trained;
                self.heard.push_back(Heard::Trained { snr_db: self.trained_snr, inverted: self.inverted });
                self.symbols();
            }
            _ if tries + 1 < TRAIN_TRIES => {
                self.stage = Stage::Collecting { start: first, base: base + TRAIN_TO, tries: tries + 1 };
            }
            _ => {
                self.stage = Stage::Idle;
                self.heard.push_back(Heard::Untrained);
            }
        }
    }

    fn power_of_uinfo(&self) -> f64 {
        ucode::level(self.law, self.uinfo).powi(2)
    }

    /// The tap of largest size, with its sign.
    fn taps_peak(&self, taps: &[f64]) -> f64 {
        taps.iter().copied().fold(0.0, |best, t| if t.abs() > best.abs() { t } else { best })
    }

    /// Where TRN1d's first symbol is, near `start`: the alignment whose
    /// short fit fits best.
    fn align(&self, start: u64, base: usize) -> Option<u64> {
        let targets = self.trn1d(base, base + SEARCH_TO);
        let mut best: Option<(f64, u64)> = None;
        for delta in -SEARCH..=SEARCH {
            let Some(origin) = start.checked_add_signed(delta) else { continue };
            let Some(solution) = self.fit(origin, &targets, SEARCH_FROM, SEARCH_TO) else { continue };
            if best.is_none_or(|(mse, _)| solution.mse < mse) {
                best = Some((solution.mse, origin));
            }
        }
        best.map(|(_, origin)| origin)
    }

    /// The equaliser solved for from TRN1d, at the best alignment near
    /// `start`.
    ///
    /// Twice, if the route moved some of TRN1d onto other codewords. A
    /// robbed bit moves UINFO onto its neighbour in one data frame interval
    /// of the six, and a fit to what was sent leans a sixth of the way
    /// towards what arrived, in gain and in everything after: the DIL then
    /// reads loud codewords a percent or two out, everywhere. So symbols the
    /// first fit puts squarely on another codeword are taken to be that one,
    /// and the fit made again.
    fn solve(&self, start: u64, base: usize) -> Option<Solution> {
        let origin = self.align(start, base)?;
        let mut targets = self.trn1d(base, base + TRAIN_TO);
        let first = self.fit(origin, &targets, TRAIN_FROM, TRAIN_TO)?;
        let outputs = self.outputs(origin, &targets, &first, TRAIN_FROM, TRAIN_TO)?;
        let mut misses: Vec<f64> = outputs.iter().map(|&(k, y)| (y - targets[k]).abs()).collect();
        misses.sort_by(f64::total_cmp);
        // The noise, from the middle of the misses: a robbed sixth cannot
        // move that.
        let noise = 1.4826 * misses.get(misses.len() / 2).copied().unwrap_or(0.0);
        let mut moved = 0;
        for &(k, y) in &outputs {
            let arrived = self.nearest_codeword(y);
            let apart = (arrived - targets[k]).abs();
            if self.neighbours(arrived, targets[k]) && (y - arrived).abs() < 0.4 * apart && apart > 2.5 * noise {
                targets[k] = arrived;
                moved += 1;
            }
        }
        if moved == 0 {
            return Some(first);
        }
        self.fit(origin, &targets, TRAIN_FROM, TRAIN_TO)
    }

    /// What a fit makes of symbols `from` to `to`, as (symbol, output).
    fn outputs(&self, origin: u64, targets: &[f64], solution: &Solution, from: usize, to: usize) -> Option<Vec<(usize, f64)>> {
        let mut out = Vec::with_capacity(to - from);
        for k in from.max(FEEDBACK)..to.min(targets.len()) {
            let row = self.samples(origin + 2 * k as u64, REACH)?;
            let fed: Vec<f64> = (1..=FEEDBACK).map(|m| targets[k - m]).collect();
            out.push((k, apply(&solution.taps, &row) - apply(&solution.feedback, &fed)));
        }
        Some(out)
    }

    /// Least squares over symbols `from` to `to` of `targets`, the first of
    /// them centred on half-symbol sample `origin`.
    fn fit(&self, origin: u64, targets: &[f64], from: usize, to: usize) -> Option<Solution> {
        let forward = 2 * REACH + 1;
        let n = forward + FEEDBACK;
        let mut a = vec![0.0; n * n];
        let mut b = vec![0.0; n];
        let mut energy = 0.0;
        let mut rows = Vec::with_capacity(to - from);
        for (k, &target) in targets.iter().enumerate().take(to).skip(from.max(FEEDBACK)) {
            let mut row = self.samples(origin + 2 * k as u64, REACH)?;
            // The symbols before, taken off.
            row.extend((1..=FEEDBACK).map(|m| -targets[k - m]));
            for j in 0..n {
                b[j] += row[j] * target;
                for i in j..n {
                    a[j * n + i] += row[j] * row[i];
                }
            }
            energy += row[..forward].iter().map(|x| x * x).sum::<f64>();
            rows.push((row, target));
        }
        // A ridge a thousandth of the line's own weight, on the line's taps
        // only: the decisions fed back are on a scale of their own.
        let ridge = 1e-3 * energy / forward as f64;
        for j in 0..n {
            if j < forward {
                a[j * n + j] += ridge;
            }
            for i in 0..j {
                a[j * n + i] = a[i * n + j];
            }
        }
        let mut taps = solve_symmetric(&a, &b)?;
        let mse = rows.iter().map(|(row, target)| (apply(&taps, row) - target).powi(2)).sum::<f64>() / rows.len().max(1) as f64;
        let feedback = taps.split_off(forward);
        Some(Solution { taps, feedback, origin, mse })
    }

    /// Equalise, decide and track the symbol at `next_half`.
    fn symbol(&mut self) -> Symbol {
        let wide = self.samples(self.next_half, REACH + 1).expect("the caller checked the samples are here");
        let index = self.next_symbol;
        self.next_half += 2;
        self.next_symbol += 1;
        let row = &wide[1..wide.len() - 1];
        let y = apply(&self.taps, row) - apply(&self.feedback, self.past.make_contiguous());
        let rate: f64 = self.taps.iter().enumerate().map(|(i, w)| w * 0.5 * (wide[i + 2] - wide[i])).sum();
        let interval = ((index + self.frame_offset) % INTERVALS as u64) as usize;
        let mut decided = self.slicer.decide(y, interval, index);
        if let (Some(sent), Slicer::Known { .. }) = (decided, &self.slicer) {
            // A known symbol the route moved onto the codeword next to it -- a
            // robbed bit does, to every other one in its interval -- arrived
            // as that codeword, and learning from the one that was sent would
            // teach the equaliser the route's doing and feed it back into the
            // next symbol.
            // Only where codewords stand far enough apart for the noise
            // training found to be no reason for it.
            let arrived = self.nearest_codeword(y);
            let apart = (arrived - sent).abs();
            let floor = ucode::level(self.law, self.uinfo) * 10f64.powf(-self.trained_snr / 20.0);
            if self.neighbours(arrived, sent) && (y - arrived).abs() < 0.25 * apart && apart > 4.0 * floor {
                decided = Some(arrived);
            }
        }
        if let Some(target) = decided {
            let e = y - target;
            if self.watch(e * e) {
                // A known sequence a slip has moved is no guide to what went
                // before, and feeding it back would spoil every output after
                // it; whatever was sent was a codeword, and the nearest one is
                // a better guess. But a known sequence nothing moved is the
                // truth, on a noisy line where the nearest codeword is often
                // not: it stands wherever the output is anywhere near it.
                // Two levels are no guide to a signal that has stopped being
                // two levels, either, though they are the best there is for
                // one that has not.
                let guess = match self.slicer {
                    Slicer::Known { .. } => e * e > 16.0 * self.settled,
                    Slicer::Binary(level) => (y.abs() - level).abs() > 0.3 * level,
                    _ => false,
                };
                let fed = if guess { self.nearest_codeword(y) } else { target };
                self.past.pop_back();
                self.past.push_front(fed);
                return Symbol { index: index + self.frame_offset, raw: index, value: y, decided };
            }
            let energy: f64 = row.iter().chain(self.past.iter()).map(|x| x * x).sum::<f64>() + 1e-18;
            let back = e * STEP / energy;
            for (tap, x) in self.taps.iter_mut().zip(row) {
                *tap -= back * x;
            }
            for (tap, d) in self.feedback.iter_mut().zip(self.past.iter()) {
                *tap += back * d;
            }
            // An output sampled late by a fraction of a half symbol is out by
            // that fraction of its rate of change.
            self.slope += 0.001 * (rate * rate - self.slope);
            let late = (e * rate / self.slope.max(1e-18)).clamp(-0.5, 0.5);
            self.due -= TIMING_GAIN * late * self.half;
            self.timed -= TIMING_GAIN * late * self.half;
            self.drift = (self.drift - DRIFT_GAIN * late).clamp(-0.002, 0.002);
            self.error += 0.002 * (e * e - self.error);
        }
        if index.is_multiple_of(CENTRE_EVERY) && decided.is_some() {
            self.hold_centre();
        }
        self.past.pop_back();
        // A known sequence with nothing to say about this symbol still had a
        // codeword in it, and the nearest one is a better guess to feed back
        // than an output with all its noise.
        let fed = match (decided, &self.slicer) {
            (Some(target), _) => target,
            (None, Slicer::Known { .. } | Slicer::Free) => self.nearest_codeword(y),
            (None, _) => y,
        };
        self.past.push_front(fed);
        Symbol { index: index + self.frame_offset, raw: index, value: y, decided }
    }

    /// Whether two levels are codewords one step apart, on the same side of
    /// zero: all a robbed bit ever moves one by.
    fn neighbours(&self, a: f64, b: f64) -> bool {
        let code = |v: f64| ucode::nearest(self.law, (v * 32768.0).round().clamp(-32768.0, 32767.0) as i32);
        let ((ua, na), (ub, nb)) = (code(a), code(b));
        na == nb && ua.abs_diff(ub) == 1
    }

    /// The G.711 level nearest `y`.
    fn nearest_codeword(&self, y: f64) -> f64 {
        let (u, negative) = ucode::nearest(self.law, (y * 32768.0).round().clamp(-32768.0, 32767.0) as i32);
        ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 }
    }

    /// Judge one decision's squared error against what the errors settled to.
    /// True while the loops are to hold still.
    fn watch(&mut self, squared: f64) -> bool {
        self.recent.push_back(squared);
        if self.recent.len() > JUDGED {
            self.recent.pop_front();
        }
        let recent = self.recent.iter().sum::<f64>() / self.recent.len() as f64;
        if self.recent.len() < JUDGED {
            self.settled = recent;
            return false;
        }
        if !self.lost && recent > LOST_AT * self.settled {
            self.lost = true;
            self.held = 0;
            self.slips += 1;
            self.heard.push_back(Heard::Lost);
        } else if self.lost && recent < FOUND_AT * self.settled {
            self.lost = false;
            self.heard.push_back(Heard::Found);
        } else if self.lost {
            self.held += 1;
            if self.held > HELD_AT_MOST {
                // Not a slip: the line has got worse, and this is it now.
                self.lost = false;
                self.settled = recent;
                self.heard.push_back(Heard::Found);
            }
        }
        if !self.lost {
            self.settled += 0.005 * (recent - self.settled);
        }
        self.lost
    }

    /// Keep the equaliser's weight where training left it.
    ///
    /// A clock drifting against the far one moves the pulse along this end's
    /// samples, and an adaptive equaliser follows it: its weight walks, the
    /// decisions stay good, and the timing loop is never told -- until the
    /// weight walks off the end. So the walk is read off the taps and given
    /// to the clock: the sampling moves by as much as the weight did, the
    /// taps move back to match, and how fast it is walking goes into the
    /// clock's rate.
    fn hold_centre(&mut self) {
        let moved = centre_of(&self.taps) - self.centre;
        if moved.abs() < 1e-4 {
            return;
        }
        let moved = moved.clamp(-0.25, 0.25);
        self.due += moved * self.half;
        self.timed += moved * self.half;
        self.drift = (self.drift + CENTRE_DRIFT_GAIN * moved / (2 * CENTRE_EVERY) as f64).clamp(-0.002, 0.002);
        // The taps at i + moved, to first order.
        let old = self.taps.clone();
        for i in 1..old.len() - 1 {
            self.taps[i] = old[i] + moved * 0.5 * (old[i + 1] - old[i - 1]);
        }
    }
}

/// Where a set of taps has its weight, in taps.
fn centre_of(taps: &[f64]) -> f64 {
    let (moment, weight) = taps.iter().enumerate().fold((0.0, 0.0), |(m, w), (i, t)| (m + i as f64 * t * t, w + t * t));
    moment / weight.max(1e-30)
}

fn apply(taps: &[f64], row: &[f64]) -> f64 {
    taps.iter().zip(row).map(|(w, x)| w * x).sum()
}

/// Solve `A x = b` for a symmetric positive definite `A`, row by row, by
/// Cholesky factorisation.
fn solve_symmetric(a: &[f64], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut l = vec![0.0; n * n];
    for j in 0..n {
        let mut diagonal = a[j * n + j];
        for k in 0..j {
            diagonal -= l[j * n + k] * l[j * n + k];
        }
        if diagonal <= 1e-12 * a[j * n + j].abs() || !diagonal.is_finite() {
            return None;
        }
        let root = diagonal.sqrt();
        l[j * n + j] = root;
        for i in j + 1..n {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            l[i * n + j] = sum / root;
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * y[k];
        }
        y[i] = sum / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in i + 1..n {
            sum -= l[k * n + i] * x[k];
        }
        x[i] = sum / l[i * n + i];
    }
    Some(x)
}

/// The Kaiser window at `x` from -1 to 1.
fn kaiser(x: f64, beta: f64) -> f64 {
    bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / bessel_i0(beta)
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= half / k as f64;
        let add = term * term;
        sum += add;
        if add < sum * 1e-16 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::sequences::{JD_BITS, Jd};

    const FS: f64 = 16_000.0;
    const UINFO: u8 = 78;

    /// What the digital modem sends in phase 3 from Sd, as levels: Sd,
    /// S-bar-d, TRN1d, Jd `repeats` times and J'd (8.4).
    fn phase3_levels(law: Law, trn: usize, jd: &Jd, repeats: usize) -> Vec<f64> {
        let w = ucode::level(law, 16 + UINFO);
        let zero = ucode::level(law, 0);
        let a = ucode::level(law, UINFO);
        let mut out = Vec::new();
        for _ in 0..64 {
            out.extend([w, zero, w, -w, -zero, -w]);
        }
        for _ in 0..8 {
            out.extend([-w, -zero, -w, w, zero, w]);
        }
        let mut scrambler = Scrambler::new(Mode::Call);
        let mut sign = false;
        for _ in 0..trn {
            sign = scrambler.scramble(true);
            out.push(if sign { a } else { -a });
        }
        // Jd and J'd: scrambled, then differentially encoded from TRN1d's
        // last sign (8.4.2, 8.4.3).
        let mut bits: Vec<bool> = Vec::new();
        for _ in 0..repeats {
            bits.extend(jd.to_bits());
        }
        bits.extend([false; 12]);
        for bit in bits {
            sign ^= scrambler.scramble(bit);
            out.push(if sign { a } else { -a });
        }
        out
    }

    /// Levels at 8 kHz to a line at 16 kHz, through a codec's reconstruction
    /// filter and a loop's loss, with the far clock `ppm` off and noise
    /// `snr_db` under the signal.
    fn line(levels: &[f64], ppm: f64, gain: f64, snr_db: f64, inverted: bool) -> Vec<f64> {
        // A windowed sinc at 3.8 kHz, sampled wherever the line sample falls
        // between codewords.
        let cutoff = 3800.0 / 8000.0;
        let span = 24i64;
        let ratio = 2.0 * (1.0 + ppm * 1e-6);
        let samples = (levels.len() as f64 * ratio) as usize;
        let mut noise = 0x2545_f491_4f6c_dd1du64;
        let power = levels.iter().map(|x| x * x).sum::<f64>() / levels.len() as f64 * gain * gain;
        let sigma = (power / 10f64.powf(snr_db / 10.0)).sqrt();
        let sign = if inverted { -gain } else { gain };
        (0..samples + 400)
            .map(|n| {
                let t = (n as f64 - 200.0) / ratio;
                let k0 = t.floor() as i64;
                let mut sum = 0.0;
                for k in k0 - span..=k0 + span {
                    if k < 0 || k as usize >= levels.len() {
                        continue;
                    }
                    let tau = t - k as f64;
                    let x = 2.0 * cutoff * tau;
                    let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                    let taper = kaiser(tau / (span as f64 + 1.0), 6.0);
                    sum += levels[k as usize] * 2.0 * cutoff * sinc * taper;
                }
                noise ^= noise << 13;
                noise ^= noise >> 7;
                noise ^= noise << 17;
                let u = (noise >> 11) as f64 / (1u64 << 53) as f64;
                sign * sum + sigma * (u - 0.5) * 12f64.sqrt()
            })
            .collect()
    }

    /// Everything the receiver makes of a line.
    fn listen(samples: &[f64]) -> (Vec<Heard>, Receiver) {
        let mut rx = Receiver::new(Law::Mu, FS);
        rx.hunt(UINFO);
        let mut heard = Vec::new();
        for &x in samples {
            rx.feed(x);
            while let Some(h) = rx.heard() {
                heard.push(h);
            }
        }
        (heard, rx)
    }

    /// Jd read out of the symbols after training, as 8.4.2 sends it.
    fn read_jd(heard: &[Heard]) -> Vec<Jd> {
        let mut descrambler = Scrambler::new(Mode::Call);
        let mut differential = false;
        let mut previous = false;
        let mut bits: Vec<bool> = Vec::new();
        let mut found = Vec::new();
        for h in heard {
            let Heard::Symbol(s) = h else { continue };
            let sign = s.positive();
            let before = descrambler.clone();
            let mut bit = descrambler.descramble(if differential { sign ^ previous } else { sign });
            if !differential && !bit {
                // TRN1d descrambles to ones; the first zero is Jd, which is
                // differential from here -- and was so for the symbol that
                // showed it.
                differential = true;
                descrambler = before;
                bit = descrambler.descramble(sign ^ previous);
            }
            previous = sign;
            bits.push(bit);
            if bits.len() >= JD_BITS
                && let Some(jd) = Jd::from_bits(&bits[bits.len() - JD_BITS..])
            {
                found.push(jd);
            }
        }
        found
    }

    #[test]
    fn a_clean_line_trains_and_reads_jd() {
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() };
        let levels = phase3_levels(Law::Mu, 2400, &jd, 6);
        let samples = line(&levels, 0.0, 0.3, 50.0, false);
        let (heard, rx) = listen(&samples);
        let trained = heard.iter().find_map(|h| if let Heard::Trained { snr_db, .. } = h { Some(*snr_db) } else { None });
        assert!(trained.is_some_and(|snr| snr > 45.0), "trained to {trained:?}: {:?}", &heard[..heard.len().min(3)]);
        assert!(!rx.inverted());
        let jds = read_jd(&heard);
        assert!(jds.len() >= 4, "{} Jd read", jds.len());
        assert!(jds.iter().all(|j| *j == jd));
    }

    #[test]
    fn a_turned_over_line_with_a_drifting_clock_and_noise_still_reads_jd() {
        let jd = Jd { rates: 0x155555, lookahead: 2, sixteen_in_training: true, ..Jd::default() };
        let levels = phase3_levels(Law::Mu, 16_000, &jd, 20);
        let samples = line(&levels, 120.0, 0.1, 25.0, true);
        let (heard, rx) = listen(&samples);
        assert!(rx.is_trained(), "{:?}", heard.iter().find(|h| !matches!(h, Heard::Symbol(_))));
        assert!(rx.inverted());
        let jds = read_jd(&heard);
        assert!(jds.len() >= 15, "{} Jd read", jds.len());
        assert!(jds.iter().all(|j| *j == jd));
        // The clock was followed rather than absorbed.
        assert!((rx.drift_ppm() - 120.0).abs() < 30.0, "drift read as {:.1} ppm", rx.drift_ppm());
    }

    #[test]
    fn the_symbols_are_scaled_to_the_codewords() {
        let jd = Jd::default();
        let levels = phase3_levels(Law::Mu, 3000, &jd, 1);
        let samples = line(&levels, 0.0, 0.05, 45.0, false);
        let (heard, _) = listen(&samples);
        let a = ucode::level(Law::Mu, UINFO);
        let values: Vec<f64> = heard
            .iter()
            .filter_map(|h| if let Heard::Symbol(s) = h { Some(s.value.abs()) } else { None })
            .take(500)
            .collect();
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        assert!((mean / a - 1.0).abs() < 0.02, "UINFO came out at {:.4} of itself", mean / a);
    }

    #[test]
    fn nothing_trains_on_silence_or_on_noise() {
        let quiet = vec![0.0; 32_000];
        let (heard, _) = listen(&quiet);
        assert!(heard.is_empty(), "{heard:?}");
        let mut x = 1u64;
        let noise: Vec<f64> = (0..48_000)
            .map(|_| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((x >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.1
            })
            .collect();
        let (heard, _) = listen(&noise);
        assert!(!heard.iter().any(|h| matches!(h, Heard::Trained { .. })), "{heard:?}");
    }
}

