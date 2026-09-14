//! Hearing phases 3 and 4: the far end's QAM brought back to points.
//!
//! Three jobs, one after another.
//!
//! Hunting for S. S alternates two points, so it repeats every two symbols
//! and nothing else V.34 sends does; S-bar is S turned half a revolution. A
//! half-symbol sample set against the one two symbols before it is the same
//! throughout S and its opposite across the change to S-bar, whatever the
//! timing and the carrier's phase, so the change can be found before anything
//! has been trained.
//!
//! Training. What follows S-bar is known from its first symbol: PP, a
//! sequence chosen for exactly this, and TRN, which is scrambled ones from a
//! scrambler started at zero -- so it is as known as PP is. Checked against
//! two real modems, a Conexant softmodem's recording and a modem answering a
//! call over VoIP: each one's TRN is the zero-started scrambler's, symbol for
//! symbol, from the symbol after PP. With the sequence known, the equaliser is
//! solved for outright by least squares over a few hundred symbols, trying
//! each alignment of the sequence either side of where S-bar put it and
//! keeping the best. No adaptive equaliser converging from nothing, and no
//! blind stage.
//!
//! Tracking. From there the equaliser, the carrier's phase and the symbol
//! timing are carried along by the decisions: the equaliser by normalised
//! least mean squares, the phase by a second-order loop, and the timing by
//! keeping the equaliser's weight where training left it. A sound card
//! talking to a VoIP call runs a hundred parts per million off the far end's
//! clock, which is a third of a symbol a second, and an equaliser left to
//! absorb that would walk off its own end in seconds.
//!
//! The equaliser samples twice a symbol. That makes it indifferent to where in
//! the symbol the sampling falls, which is why a timing loop has so little to
//! do: it only has to stop the drift, not find the eye.

use std::collections::VecDeque;

use dsp::{Complex, least_squares};

use super::constellation::Point;
use super::qam::{Band, ROLLOFF};
use super::signals::{self, Size};
use crate::v32::Mode;

/// Taps of the interpolating low-pass filter, and the fractional positions its
/// table is made for.
const FILTER_TAPS: usize = 64;
const FILTER_PHASES: usize = 256;

/// Equaliser taps either side of the centre, in half symbols: 31 taps,
/// fifteen and a half symbols of the line's memory.
const REACH: usize = 15;

/// Half-symbol samples kept for training: 0.6 s at 3429 symbols a second.
const KEPT: usize = 4096;

/// Symbols of PP not trained on while the line's memory fills.
const PP_SKIPPED: usize = 48;

/// How much of TRN goes into training after PP, and how much of it when TRN
/// comes alone. Both well inside the 512 symbols TRN is sent for at least.
const TRN_AFTER_PP: usize = 64;
const TRN_ALONE: usize = 384;

/// Symbols of TRN left out of the alignment search when it comes alone, while
/// the line's memory of S-bar clears.
const TRN_SKIPPED: usize = 16;

/// Half symbols either side of where S-bar puts the training sequence that
/// the search tries.
const SEARCH: i64 = 8;

/// Least signal a half-symbol sample has to carry to be S: 37 dB under the
/// nominal level.
const AUDIBLE: f64 = 4e-4;

/// Normalised least-mean-squares step.
const STEP: f64 = 0.02;

/// Carrier loop gains, a symbol at a time.
const PHASE_GAIN: f64 = 0.04;
const FREQUENCY_GAIN: f64 = 4e-4;

/// Timing loop gains: how much of each symbol's timing error is taken out of
/// the sampling at once, and how much goes into its rate. A second-order loop
/// critically damped a few hundred symbols wide: quick enough that the
/// equaliser, which takes the best part of a thousand symbols to move, is
/// never the one following the drift.
const TIMING_GAIN: f64 = 0.01;
const DRIFT_GAIN: f64 = 1.25e-5;

/// A training sequence, known from its first symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    /// PP, and then TRN at four points: phase 3 (10.1.3.6, 10.1.3.8).
    PpThenTrn,
    /// TRN alone, at the size given: phase 4.
    Trn(Size),
}

/// What the receiver has to report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Heard {
    /// S, and then the change to S-bar. `at` is the half-symbol sample S-bar
    /// is reckoned to start at, give or take one.
    Reversal { at: u64 },
    /// Trained, with the signal to noise the training left.
    Trained { snr_db: f64 },
    /// Nothing trained: the sequence was not where S-bar said.
    Untrained,
    Symbol(Symbol),
}

/// One symbol, equalised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Symbol {
    /// The point, scaled so the constellation in use has unit mean power.
    pub point: Complex,
    /// The nearest point of that constellation, on the grid of Figure 5.
    pub decided: Point,
    /// Squared distance from it, at the same scale.
    pub error: f64,
}

#[derive(Debug, Clone)]
enum Mode3 {
    Idle,
    Hunting(Hunt),
    Collecting { reference: Reference, far: Mode, start: u64 },
    Trained,
}

/// Looks for S and its change to S-bar in half-symbol samples.
///
/// S is found by each sample matching the one two symbols before it. Its
/// change to S-bar is not found that way, because the change is not sharp:
/// the far end's pulse and this end's filter spread it across a symbol or two,
/// and a modem on the far side of a VoIP call smeared it over the whole two
/// symbols that comparison looks back. So once S is sure, its four samples are
/// learned as a template, and S-bar is the template turned round.
#[derive(Debug, Clone, Default)]
struct Hunt {
    /// The last four samples: two symbols.
    recent: VecDeque<Complex>,
    /// The last eight correlations with two symbols before, and powers.
    correlations: VecDeque<(Complex, f64)>,
    held: usize,
    /// S's four half-symbol samples, averaged, once it is sure.
    template: [Complex; 4],
    armed: bool,
    /// The last four samples against the template, and the template's power.
    matches: VecDeque<(f64, f64)>,
    lapsed: usize,
}

impl Hunt {
    /// Halves of S in a row before its template is trusted: twenty symbols.
    const HELD: usize = 40;

    fn feed(&mut self, half: Complex, index: u64) -> Option<u64> {
        let phase = (index % 4) as usize;
        if self.armed {
            let t = self.template[phase];
            self.matches.push_back(((half * t.conj()).re, t.norm_sqr()));
            if self.matches.len() > 4 {
                self.matches.pop_front();
            }
            let (along, power) = self.matches.iter().fold((0.0, 0.0), |(a, p), &(x, y)| (a + x, p + y));
            let ratio = along / power.max(1e-12);
            if self.matches.len() == 4 && ratio < -0.5 {
                // The first of the four that turned it.
                return Some(index.saturating_sub(3));
            }
            if ratio > 0.5 {
                self.template[phase] = self.template[phase].scale(0.9) + half.scale(0.1);
                self.lapsed = 0;
            } else {
                self.lapsed += 1;
                if self.lapsed > 24 {
                    *self = Self::default();
                }
            }
            self.recent.push_back(half);
            if self.recent.len() > 4 {
                self.recent.pop_front();
            }
            return None;
        }
        if self.recent.len() == 4 {
            let before = self.recent[0];
            let c = half * before.conj();
            let p = (half.norm_sqr() + before.norm_sqr()) / 2.0;
            self.correlations.push_back((c, p));
            if self.correlations.len() > 8 {
                self.correlations.pop_front();
            }
            let (sum_c, sum_p) =
                self.correlations.iter().fold((Complex::ZERO, 0.0), |(sc, sp), &(c, p)| (sc + c, sp + p));
            let s_like = sum_c.re > 0.7 * sum_p && sum_p / self.correlations.len() as f64 > AUDIBLE;
            if s_like {
                self.held += 1;
                // Averaged over the last stretch of S, weighted to the newest.
                self.template[phase] = self.template[phase].scale(0.8) + half.scale(0.2);
            } else {
                self.held = 0;
                self.template = [Complex::ZERO; 4];
            }
            if self.held >= Self::HELD {
                self.armed = true;
                self.lapsed = 0;
                self.matches.clear();
            }
            self.recent.pop_front();
        }
        self.recent.push_back(half);
        None
    }
}

/// The far end's signal, one end of phases 3 and 4.
#[derive(Debug, Clone)]
pub struct Receiver {
    band: Band,
    /// Mixer phase, as a fraction of a turn, and its step a sample.
    phase: f64,
    step: f64,
    /// The last few mixed-down samples, newest last.
    mixed: VecDeque<Complex>,
    /// Samples taken in.
    taken: u64,
    /// Where the next half-symbol sample falls, in samples since the start.
    due: f64,
    /// Samples a half symbol, nominally, and the timing loop's correction to
    /// it as a fraction.
    half: f64,
    drift: f64,
    table: Vec<f64>,

    halves: VecDeque<Complex>,
    /// Index of the first sample in `halves`, and of the next to be made.
    first: u64,
    made: u64,

    mode: Mode3,
    heard: VecDeque<Heard>,

    taps: Vec<Complex>,
    /// The half-symbol sample the next symbol is centred on.
    next_symbol: u64,
    size: Size,
    /// Carrier phase to take out of the next symbol, and its turn a symbol,
    /// in radians.
    rotation: f64,
    turn: f64,
    /// Mean squared size of the equaliser output's rate of change, a half
    /// symbol at a time, which turns an error into a timing error.
    slope: f64,
    /// Mean squared error of the decisions.
    error: f64,
    trained_snr: f64,
}

impl Receiver {
    pub fn new(band: Band, fs: f64) -> Self {
        let baud = band.baud();
        let cutoff = (0.5 * baud * (1.0 + ROLLOFF) + 300.0).min(0.45 * fs);
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
        let mut taps = vec![Complex::ZERO; 2 * REACH + 1];
        taps[REACH] = Complex::ONE;
        Self {
            band,
            phase: 0.0,
            step: band.carrier() / fs,
            mixed: std::iter::repeat_n(Complex::ZERO, FILTER_TAPS).collect(),
            taken: 0,
            due: FILTER_TAPS as f64,
            half: fs / baud / 2.0,
            drift: 0.0,
            table,
            halves: VecDeque::with_capacity(KEPT),
            first: 0,
            made: 0,
            mode: Mode3::Idle,
            heard: VecDeque::new(),
            taps,
            next_symbol: 0,
            size: Size::Four,
            rotation: 0.0,
            turn: 0.0,
            slope: 1.0,
            error: 1.0,
            trained_snr: 0.0,
        }
    }

    pub fn band(&self) -> Band {
        self.band
    }

    /// Look for S and the change to S-bar.
    pub fn hunt(&mut self) {
        self.mode = Mode3::Hunting(Hunt::default());
    }

    /// Train on `reference`, which starts sixteen symbols after the S-bar at
    /// half-symbol sample `s_bar`, and is sent by a modem whose scrambler is
    /// `far`'s.
    pub fn train(&mut self, reference: Reference, far: Mode, s_bar: u64) {
        let start = s_bar + 2 * signals::S_BAR_SYMBOLS as u64;
        self.mode = Mode3::Collecting { reference, far, start };
        self.size = match reference {
            Reference::PpThenTrn => Size::Four,
            Reference::Trn(size) => size,
        };
    }

    /// Stop listening.
    pub fn idle(&mut self) {
        self.mode = Mode3::Idle;
    }

    pub fn is_trained(&self) -> bool {
        matches!(self.mode, Mode3::Trained)
    }

    /// The constellation decisions are made against from here on.
    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// Signal to noise of the decisions, in decibels.
    pub fn snr_db(&self) -> f64 {
        -10.0 * self.error.max(1e-9).log10()
    }

    /// What training left.
    pub fn trained_snr_db(&self) -> f64 {
        self.trained_snr
    }

    /// The far clock's rate against this end's, as the timing loop has it, in
    /// parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.drift * 1e6
    }

    /// Half-symbol samples made so far.
    pub fn halves(&self) -> u64 {
        self.made
    }

    /// The next thing heard, if there is one.
    pub fn heard(&mut self) -> Option<Heard> {
        self.heard.pop_front()
    }

    pub fn feed(&mut self, sample: f64) {
        let angle = std::f64::consts::TAU * self.phase;
        let mixed = Complex::new(angle.cos(), -angle.sin()).scale(2.0 * sample);
        self.phase += self.step;
        self.phase -= self.phase.floor();
        self.mixed.pop_front();
        self.mixed.push_back(mixed);
        self.taken += 1;
        let newest = self.taken - 1;
        // The filter's taps reach FILTER_TAPS/2 samples past the time it is
        // evaluated at.
        while self.due.floor() as u64 + (FILTER_TAPS / 2) as u64 <= newest {
            let base = self.due.floor();
            let mut ph = ((self.due - base) * FILTER_PHASES as f64).round() as usize;
            let mut base = base as u64;
            if ph == FILTER_PHASES {
                ph = 0;
                base += 1;
                if base + (FILTER_TAPS / 2) as u64 > newest {
                    break;
                }
            }
            let offset = (base + (FILTER_TAPS / 2) as u64 - newest) as usize;
            let row = &self.table[ph * FILTER_TAPS..(ph + 1) * FILTER_TAPS];
            let mut value = Complex::ZERO;
            for (i, tap) in row.iter().enumerate() {
                if let Some(x) = self.mixed.get(offset + i) {
                    value += *x * *tap;
                }
            }
            self.due += self.half * (1.0 + self.drift);
            self.on_half(value);
        }
    }

    fn on_half(&mut self, half: Complex) {
        let index = self.made;
        self.made += 1;
        self.halves.push_back(half);
        if self.halves.len() > KEPT {
            self.halves.pop_front();
            self.first += 1;
        }
        match &mut self.mode {
            Mode3::Idle => {}
            Mode3::Hunting(hunt) => {
                if let Some(at) = hunt.feed(half, index) {
                    self.mode = Mode3::Idle;
                    self.heard.push_back(Heard::Reversal { at });
                }
            }
            Mode3::Collecting { reference, far, start } => {
                let (reference, far, start) = (*reference, *far, *start);
                let (_, end) = windows(reference).1;
                let needed = start + SEARCH as u64 + 2 * end as u64 + REACH as u64;
                if self.made > needed {
                    self.finish_training(reference, far, start);
                }
            }
            Mode3::Trained => {
                while self.next_symbol + REACH as u64 + 2 <= self.made {
                    if self.next_symbol < self.first + REACH as u64 + 1 {
                        self.next_symbol += 2;
                        continue;
                    }
                    let symbol = self.symbol();
                    self.heard.push_back(Heard::Symbol(symbol));
                }
            }
        }
    }

    /// The half-symbol samples around the one at `centre`.
    fn row(&self, centre: u64) -> Option<Vec<Complex>> {
        self.samples(centre, REACH)
    }

    /// The `reach` half-symbol samples either side of `centre`, and it.
    fn samples(&self, centre: u64, reach: usize) -> Option<Vec<Complex>> {
        let from = centre.checked_sub(reach as u64)?.checked_sub(self.first)? as usize;
        let to = from + 2 * reach + 1;
        (to <= self.halves.len()).then(|| self.halves.range(from..to).copied().collect())
    }

    fn finish_training(&mut self, reference: Reference, far: Mode, start: u64) {
        let ((search_from, search_to), (from, to)) = windows(reference);
        let targets = sequence(reference, far, to);
        // Each alignment either side of where S-bar put the sequence.
        let mut best: Option<(f64, i64, Vec<Complex>)> = None;
        for delta in -SEARCH..=SEARCH {
            let Some(origin) = start.checked_add_signed(delta) else { continue };
            let rows: Option<Vec<Vec<Complex>>> = (search_from..search_to).map(|k| self.row(origin + 2 * k as u64)).collect();
            let Some(rows) = rows else { continue };
            let refs: Vec<&[Complex]> = rows.iter().map(Vec::as_slice).collect();
            let wanted = &targets[search_from..search_to];
            let Some(taps) = least_squares(&refs, wanted, ridge(&rows)) else { continue };
            let mse = residual(&rows, wanted, &taps);
            if best.as_ref().is_none_or(|b| mse < b.0) {
                best = Some((mse, delta, taps));
            }
        }
        let Some((_, delta, taps)) = best else {
            self.mode = Mode3::Idle;
            self.heard.push_back(Heard::Untrained);
            return;
        };
        let origin = start.saturating_add_signed(delta);
        // How fast the constellation turns, from the rough fit's residual
        // phase early and late in its window.
        let rows: Vec<Vec<Complex>> = (search_from..search_to).filter_map(|k| self.row(origin + 2 * k as u64)).collect();
        let middle = rows.len() / 2;
        let lean = |range: std::ops::Range<usize>| {
            range.fold(Complex::ZERO, |sum, i| sum + apply(&taps, &rows[i]) * targets[search_from + i].conj())
        };
        let (early, late) = (lean(0..middle), lean(middle..rows.len()));
        let turn = (late * early.conj()).arg() / middle.max(1) as f64;
        // The whole window, with the turn put into the targets for the
        // equaliser to follow and the carrier loop to take back out.
        let rows: Option<Vec<Vec<Complex>>> = (from..to).map(|k| self.row(origin + 2 * k as u64)).collect();
        let Some(rows) = rows else {
            self.mode = Mode3::Idle;
            self.heard.push_back(Heard::Untrained);
            return;
        };
        let turned: Vec<Complex> = (from..to).map(|k| targets[k] * Complex::from_polar(1.0, turn * k as f64)).collect();
        let refs: Vec<&[Complex]> = rows.iter().map(Vec::as_slice).collect();
        let Some(taps) = least_squares(&refs, &turned, ridge(&rows)) else {
            self.mode = Mode3::Idle;
            self.heard.push_back(Heard::Untrained);
            return;
        };
        let mse = residual(&rows, &turned, &taps);
        self.taps = taps;
        self.turn = turn;
        self.rotation = turn * to as f64;
        self.next_symbol = origin + 2 * to as u64;
        self.error = mse;
        self.trained_snr = -10.0 * mse.max(1e-9).log10();
        if self.trained_snr < 6.0 {
            self.mode = Mode3::Idle;
            self.heard.push_back(Heard::Untrained);
            return;
        }
        self.mode = Mode3::Trained;
        self.heard.push_back(Heard::Trained { snr_db: self.trained_snr });
        // Everything already here past the window.
        while self.next_symbol + REACH as u64 + 2 <= self.made {
            let symbol = self.symbol();
            self.heard.push_back(Heard::Symbol(symbol));
        }
    }

    /// Equalise, decide and track the symbol at `next_symbol`.
    fn symbol(&mut self) -> Symbol {
        let wide = self.samples(self.next_symbol, REACH + 1).expect("the caller checked the samples are here");
        self.next_symbol += 2;
        let row = &wide[1..wide.len() - 1];
        let y = apply(&self.taps, row);
        // How fast the output is changing: the same filter over the samples'
        // central differences.
        let rate = self
            .taps
            .iter()
            .enumerate()
            .fold(Complex::ZERO, |sum, (i, w)| sum + *w * (wide[i + 2] - wide[i]).scale(0.5));
        let spin = Complex::from_polar(1.0, -self.rotation);
        let z = y * spin;
        let (decided, target) = decide(z, self.size);
        let e = z - target;
        // The equaliser learns in its own frame, before the carrier is taken
        // out.
        let energy: f64 = row.iter().map(|x| x.norm_sqr()).sum::<f64>() + 1e-9;
        let back = e * spin.conj() * (STEP / energy);
        for (tap, x) in self.taps.iter_mut().zip(row) {
            *tap -= back * x.conj();
        }
        let power = target.norm_sqr().max(0.1);
        let wrong = (z * target.conj()).im / power;
        self.turn += FREQUENCY_GAIN * wrong;
        self.rotation += self.turn + PHASE_GAIN * wrong;
        self.rotation = self.rotation.rem_euclid(std::f64::consts::TAU);
        // Timing. An output sampled late by a fraction of a half symbol is out
        // by that fraction of its rate of change, so the error's share along
        // the rate of change is how late.
        let rate = rate * spin;
        self.slope += 0.01 * (rate.norm_sqr() - self.slope);
        let late = ((e * rate.conj()).re / self.slope.max(1e-9)).clamp(-0.5, 0.5);
        self.due -= TIMING_GAIN * late * self.half;
        self.drift = (self.drift - DRIFT_GAIN * late).clamp(-0.001, 0.001);
        self.error += 0.01 * (e.norm_sqr() - self.error);
        Symbol { point: z, decided, error: e.norm_sqr() }
    }
}

/// Where training searches for alignment, and the whole window it trains on,
/// as symbol ranges of the reference.
fn windows(reference: Reference) -> ((usize, usize), (usize, usize)) {
    match reference {
        Reference::PpThenTrn => ((PP_SKIPPED, signals::PP_SYMBOLS), (PP_SKIPPED, signals::PP_SYMBOLS + TRN_AFTER_PP)),
        Reference::Trn(_) => ((TRN_SKIPPED, 256), (TRN_SKIPPED, TRN_ALONE)),
    }
}

/// The first `length` symbols of a training sequence, at unit mean power.
fn sequence(reference: Reference, far: Mode, length: usize) -> Vec<Complex> {
    let mut sender = signals::Sender::new(far);
    let point = |p: Point, size: Size| Complex::new(f64::from(p.0), f64::from(p.1)).scale(unit(size));
    (0..length)
        .map(|k| match reference {
            Reference::PpThenTrn if k < signals::PP_SYMBOLS => signals::pp(k).into(),
            Reference::PpThenTrn => point(sender.trn(Size::Four), Size::Four),
            Reference::Trn(size) => point(sender.trn(size), size),
        })
        .collect()
}

/// What a constellation's grid is multiplied by to give it unit mean power:
/// four points at (+-1, +-1) have a mean power of 2, and sixteen out to 3 of
/// 10.
pub fn unit(size: Size) -> f64 {
    match size {
        Size::Four => std::f64::consts::FRAC_1_SQRT_2,
        Size::Sixteen => 1.0 / 10f64.sqrt(),
    }
}

/// The nearest point to `z` of the constellation, on the grid and at unit
/// power.
fn decide(z: Complex, size: Size) -> (Point, Complex) {
    let scale = unit(size);
    let grid = signals::decide((z.re / scale, z.im / scale), size);
    (grid, Complex::new(f64::from(grid.0), f64::from(grid.1)).scale(scale))
}

fn apply(taps: &[Complex], row: &[Complex]) -> Complex {
    taps.iter().zip(row).fold(Complex::ZERO, |sum, (w, x)| sum + *w * *x)
}

fn residual(rows: &[Vec<Complex>], targets: &[Complex], taps: &[Complex]) -> f64 {
    rows.iter().zip(targets).map(|(row, &d)| (apply(taps, row) - d).norm_sqr()).sum::<f64>() / rows.len().max(1) as f64
}

/// A ridge a thousandth of the signal's own weight on the diagonal.
fn ridge(rows: &[Vec<Complex>]) -> f64 {
    let energy: f64 = rows.iter().flatten().map(|x| x.norm_sqr()).sum();
    1e-3 * energy / (2 * REACH + 1) as f64
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
    use crate::v34::info::SymbolRate;
    use crate::v34::qam::Transmitter;
    use crate::v34::signals::{J_SIXTEEN, Reader, Sender};

    const FS: f64 = 16_000.0;

    /// What an answer modem sends in phase 3: S, S-bar, PP, TRN and J.
    fn phase3(band: Band, trn: usize, js: usize) -> Vec<f64> {
        let mut tx = Transmitter::new(band, 0, 0, FS);
        let mut sender = Sender::new(Mode::Answer);
        let mut symbols: VecDeque<Complex> = VecDeque::new();
        let grid = |p: Point, size: Size| Complex::new(f64::from(p.0), f64::from(p.1)).scale(unit(size));
        symbols.extend(std::iter::repeat_n(Complex::ZERO, 400));
        symbols.extend((0..signals::S_SYMBOLS).map(|n| grid(signals::s(n), Size::Four)));
        symbols.extend((0..signals::S_BAR_SYMBOLS).map(|n| grid(signals::s_bar(n), Size::Four)));
        symbols.extend((0..signals::PP_SYMBOLS).map(|n| Complex::from(signals::pp(n))));
        symbols.extend((0..trn).map(|_| grid(sender.trn(Size::Four), Size::Four)));
        let j: Vec<bool> = J_SIXTEEN.repeat(js);
        symbols.extend(sender.sequence(&j, Size::Four).into_iter().map(|p| grid(p, Size::Four)));
        symbols.extend(std::iter::repeat_n(Complex::ZERO, 200));
        let total = symbols.len();
        let mut out = Vec::new();
        while tx.symbols() < total as u64 + 50 {
            out.push(tx.next_sample(|| symbols.pop_front().unwrap_or(Complex::ZERO)));
        }
        out
    }

    /// A clock `ppm` parts per million slow, and the line's loss, noise and
    /// delay.
    fn line(samples: &[f64], ppm: f64, loss_db: f64, noise_db: f64) -> Vec<f64> {
        let mut resampler = dsp::Resampler::new(FS, FS * (1.0 + ppm * 1e-6));
        let mut out = Vec::new();
        for &x in samples {
            resampler.process(x, &mut out);
        }
        let gain = 10f64.powf(-loss_db / 20.0);
        let noise = 10f64.powf(-noise_db / 20.0) * 0.707;
        let mut seed = 0x2545_f491_u32;
        out.iter()
            .map(|x| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                x * gain + (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 3.464 * noise * gain
            })
            .collect()
    }

    struct Heard3 {
        reversal: Option<u64>,
        snr: Option<f64>,
        j: Option<usize>,
        errors: Vec<f64>,
    }

    /// Hear phase 3 as a call modem would: find S-bar, train, and read J.
    fn listen(samples: &[f64], band: Band) -> Heard3 {
        let mut rx = Receiver::new(band, FS);
        rx.hunt();
        let mut reader = Reader::new(Mode::Answer);
        let mut in_trn = true;
        let mut bits: Vec<bool> = Vec::new();
        let mut result = Heard3 { reversal: None, snr: None, j: None, errors: Vec::new() };
        for &x in samples {
            rx.feed(x);
            while let Some(heard) = rx.heard() {
                match heard {
                    Heard::Reversal { at } => {
                        result.reversal = Some(at);
                        rx.train(Reference::PpThenTrn, Mode::Answer, at);
                    }
                    Heard::Trained { snr_db } => result.snr = Some(snr_db),
                    Heard::Untrained => panic!("training failed"),
                    Heard::Symbol(symbol) => {
                        result.errors.push(symbol.error);
                        if in_trn {
                            let got = reader.trn(symbol.decided, Size::Four);
                            if got.iter().all(|b| *b) || bits.len() < 46 {
                                bits.extend(got);
                                continue;
                            }
                            in_trn = false;
                        }
                        bits.extend(reader.differential(symbol.decided, Size::Four));
                        let tail = &bits[bits.len().saturating_sub(32)..];
                        if result.j.is_none() && tail.len() == 32 && tail[..16] == J_SIXTEEN && tail[16..] == J_SIXTEEN {
                            result.j = Some(bits.len());
                        }
                    }
                }
            }
        }
        result
    }

    #[test]
    fn a_clean_phase_3_trains_and_its_j_is_read() {
        let band = Band::new(SymbolRate::S3429, false);
        let heard = listen(&line(&phase3(band, 1000, 40), 0.0, 20.0, 50.0), band);
        assert!(heard.reversal.is_some(), "S-bar never found");
        let snr = heard.snr.expect("never trained");
        assert!(snr > 35.0, "trained to {snr:.1} dB");
        assert!(heard.j.is_some(), "no J");
        // The last few hundred symbols are the silence after J.
        let late = &heard.errors[heard.errors.len() / 2..heard.errors.len() - 300];
        let snr_late = -10.0 * (late.iter().sum::<f64>() / late.len() as f64).log10();
        assert!(snr_late > 35.0, "tracked at {snr_late:.1} dB");
    }

    #[test]
    fn a_far_clock_114_ppm_out_is_followed() {
        // The first real call's offset, across three seconds of TRN: a symbol
        // of drift, which an untracked equaliser would not survive.
        let band = Band::new(SymbolRate::S3429, false);
        //
        // Two hundred is as far apart as two clocks V.34 holds to 0.01% can
        // be. Training sees the drift across its few hundred symbols as noise,
        // and the timing loop takes it out of everything after.
        for ppm in [-114.0, 114.0, 200.0] {
            let heard = listen(&line(&phase3(band, 10_000, 40), ppm, 20.0, 45.0), band);
            let snr = heard.snr.expect("never trained");
            assert!(snr > 28.0, "{ppm} ppm trained to {snr:.1} dB");
            assert!(heard.j.is_some(), "{ppm} ppm: no J");
            let late = &heard.errors[heard.errors.len() - 2300..heard.errors.len() - 300];
            let snr_late = -10.0 * (late.iter().sum::<f64>() / late.len() as f64).log10();
            assert!(snr_late > 38.0, "{ppm} ppm tracked at {snr_late:.1} dB");
        }
    }

    #[test]
    fn a_voip_call_at_8_khz_trains_and_reads_j() {
        // A G.711 call carries 8000 samples a second, so everything above
        // 4 kHz is gone and every transition is smeared by the filters either
        // side. The modem answering the first real call over one had its
        // change from S to S-bar spread across two whole symbols.
        let band = Band::new(SymbolRate::S3429, false);
        let sent = phase3(band, 1500, 40);
        let (mut down, mut up) = (dsp::Resampler::new(FS, 8000.0), dsp::Resampler::new(8000.0, FS));
        let (mut narrow, mut back) = (Vec::new(), Vec::new());
        for &x in &sent {
            down.process(x, &mut narrow);
        }
        for &x in &narrow {
            up.process(x, &mut back);
        }
        let heard = listen(&line(&back, 114.0, 15.0, 45.0), band);
        let snr = heard.snr.expect("never trained");
        assert!(snr > 28.0, "trained to {snr:.1} dB");
        assert!(heard.j.is_some(), "no J");
    }

    #[test]
    fn every_symbol_rate_and_carrier_trains() {
        for rate in SymbolRate::ALL {
            for high in [false, true] {
                let band = Band::new(rate, high);
                let heard = listen(&line(&phase3(band, 800, 20), 50.0, 10.0, 40.0), band);
                let snr = heard.snr.unwrap_or_else(|| panic!("{rate:?} high {high} never trained"));
                assert!(snr > 28.0, "{rate:?} high {high}: {snr:.1} dB");
                assert!(heard.j.is_some(), "{rate:?} high {high}: no J");
            }
        }
    }
}
