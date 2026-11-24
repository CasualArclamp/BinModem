//! Training: the equaliser solved for outright, by least squares, from a
//! sequence known from its first symbol.
//!
//! Copied from `v34/receiver.rs:741-845`. V.34's and V.32's training
//! sequences are both scrambled ones from a scrambler started at zero, so
//! they are as known as a fixed pattern; each alignment of the sequence
//! either side of where S-bar put it is tried, the equaliser solved at each,
//! and the best kept. No adaptive equaliser converging from nothing, and no
//! blind stage. A 31-tap equaliser at two samples a symbol comes out with
//! the line's gain and the carrier's absolute phase in it, and the carrier's
//! turn a symbol besides (core.md 2.2).
//!
//! What is V.34's own stays with V.34: the sequences (the caller gives the
//! targets), the windows (the caller gives them, and [`Training::new`] has
//! V.34's for TRN alone), and `reacquire`, which reads V.34's TRN.
//!
//! Two things are added. The carrier's turn can be given, as the hunt
//! measured it from S, and the targets are turned by it before anything is
//! solved: the alignment search then fits a signal that is not turning, and
//! the early-against-late estimate V.34 makes measures only what is left. At
//! 7 Hz V.34's static first fit is 15 dB, and its estimate aliases at 10 Hz
//! (core.md E2). And when neither window fits, the taps an earlier training
//! left can be found again by a resync search (design.md 3.2).

use super::{Core, Heard, Mode, REACH, Slicer, Via, apply, centroid};
use crate::{Complex, least_squares};

/// Half symbols either side of where the sequence is said to start that the
/// first try searches, and the second (`v34/receiver.rs:81`, `:86`).
const SEARCH: i64 = 8;
const WIDE_SEARCH: i64 = 200;

/// Searches wider than this set the samples against the sequence first, and
/// solve only at the few alignments around the best (`v34/receiver.rs:789-807`).
const PRESCORED_BEYOND: i64 = 16;

/// Signal to noise below which a training is taken to have trained on
/// nothing at all (`v34/receiver.rs:767`).
const LEAST_DB: f64 = 6.0;

/// How much worse than the best alignment's fit a more central one may be
/// and still be taken, when the options ask for central taps.
const CENTRED_MARGIN_DB: f64 = 0.5;

/// Symbol ranges of the sequence, `from..to`, that one try aligns and solves
/// over, and the half symbols either side it searches for the alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub align: (usize, usize),
    pub solve: (usize, usize),
    pub search: i64,
}

/// A known sequence to train on, and where to find it.
#[derive(Debug, Clone, PartialEq)]
pub struct Training {
    /// The sequence from its first symbol, in the units of the constellation
    /// tracked afterwards: at unit power, for a table at unit power.
    pub targets: Vec<Complex>,
    /// The half-symbol sample the first target is centred on, give or take
    /// the search. For a sequence that starts sixteen symbols after S-bar,
    /// as V.32's TRN and V.34's do, that is S-bar's `at` plus 32.
    pub start: u64,
    pub first: Window,
    /// A second try, further in and searched wide, for when the first fits
    /// nothing: a slip in the middle of the first window spoils its fit.
    pub retry: Option<Window>,
    /// The carrier's turn a symbol, in radians, if it is known already.
    /// None is V.34's behaviour: the turn is estimated from the fit alone.
    pub turn: Option<f64>,
    /// The far clock's drift as the timing loop counts it, if it is known
    /// already: the rows are read again on a grid that follows it, and the
    /// timing loop starts from it. None is V.34's behaviour: the rows as they
    /// were sampled, and the timing loop as it was.
    pub drift: Option<f64>,
    /// Signal to noise below which a try is taken to have found the wrong
    /// alignment (`v34/receiver.rs:90`).
    pub accept_db: f64,
    /// What to decide against once trained.
    pub slicer: Slicer,
    /// If nothing fits and an earlier training did, look for the signal with
    /// the taps it left before giving up.
    pub fallback: bool,
}

impl Training {
    /// Train on `targets` from half `start`, with V.34's windows for TRN
    /// sent alone (`v34/receiver.rs:1222-1223`) and none of the additions.
    pub fn new(targets: Vec<Complex>, start: u64, slicer: Slicer) -> Self {
        Self {
            targets,
            start,
            first: Window { align: (16, 256), solve: (16, 384), search: SEARCH },
            retry: Some(Window { align: (320, 512), solve: (320, 512), search: WIDE_SEARCH }),
            turn: None,
            drift: None,
            accept_db: 12.0,
            slicer,
            fallback: false,
        }
    }
}

/// The half-symbol samples a training try reads, from `first` on.
struct Rows {
    first: u64,
    halves: Vec<Complex>,
}

impl Rows {
    /// The `REACH` samples either side of half `centre`, and it.
    fn row(&self, centre: u64) -> Option<&[Complex]> {
        let from = centre.checked_sub(REACH as u64)?.checked_sub(self.first)? as usize;
        self.halves.get(from..from + 2 * REACH + 1)
    }

    fn at(&self, index: u64) -> Option<Complex> {
        self.halves.get(index.checked_sub(self.first)? as usize).copied()
    }
}

#[derive(Debug, Clone)]
pub(super) struct Collecting {
    training: Training,
    second: bool,
}

impl Collecting {
    fn window(&self) -> Window {
        match (self.second, self.training.retry) {
            (true, Some(retry)) => retry,
            _ => self.training.first,
        }
    }

    /// Half-symbol samples that must be in before the try can be made
    /// (`v34/receiver.rs:703-706`).
    pub(super) fn needed(&self) -> u64 {
        let window = self.window();
        let end = window.solve.1.max(window.align.1);
        self.training.start + window.search.max(0) as u64 + 2 * end as u64 + REACH as u64
    }
}

/// An equaliser a training came to (`v34/receiver.rs:262-274`).
#[derive(Debug, Clone)]
struct Solution {
    taps: Vec<Complex>,
    /// The carrier's turn a symbol, and its phase at the first symbol after
    /// the window.
    turn: f64,
    rotation: f64,
    /// The half-symbol sample the sequence's first symbol is centred on, and
    /// the symbol after the window.
    origin: u64,
    end: usize,
    mse: f64,
}

fn db(mse: f64) -> f64 {
    -10.0 * mse.max(1e-9).log10()
}

impl Core {
    /// Train on `training`, once enough of it has come.
    pub fn train(&mut self, training: Training) {
        self.slicer = training.slicer.clone();
        self.mode = Mode::Training(Box::new(Collecting { training, second: false }));
        self.pending = None;
    }

    /// As `v34/receiver.rs:741-783`, less V.34's `reacquire`.
    pub(super) fn finish_training(&mut self) {
        let Mode::Training(collecting) = std::mem::replace(&mut self.mode, Mode::Idle) else { return };
        let Collecting { training, second } = *collecting;
        let window = if second { training.retry.unwrap_or(training.first) } else { training.first };
        let solution = self.solve_known(&training, window);
        let enough = |s: &Solution| db(s.mse) >= training.accept_db;
        if !second && training.retry.is_some() && solution.as_ref().is_none_or(|s| !enough(s)) {
            // Nothing fitted where S-bar said. A slip in the middle of the
            // window spoils a fit that way; so try again further in,
            // searching wide for where it went.
            self.mode = Mode::Training(Box::new(Collecting { training, second: true }));
            return;
        }
        if training.fallback && self.ever_trained && solution.as_ref().is_none_or(|s| !enough(s)) {
            self.slicer = training.slicer.clone();
            if self.fall_back() {
                return;
            }
        }
        let Some(solution) = solution else {
            self.heard.push_back(Heard::Untrained);
            return;
        };
        self.taps = solution.taps;
        self.turn = solution.turn;
        self.rotation = solution.rotation;
        self.next_symbol = solution.origin + 2 * solution.end as u64;
        self.error = solution.mse;
        self.residual = solution.mse.sqrt();
        self.trained_snr = db(solution.mse);
        if self.trained_snr < LEAST_DB {
            self.heard.push_back(Heard::Untrained);
            return;
        }
        if let Some(drift) = training.drift {
            // The taps were solved on a grid that follows the far clock, so
            // everything from the sequence's start on is read again on it,
            // and the timing loop starts from its drift.
            self.front.regrid(training.start.max(self.front.first), self.front.half * (1.0 + drift));
            self.front.drift = drift;
        }
        self.ever_trained = true;
        self.lost = None;
        self.recent.clear();
        self.refused.clear();
        self.settled = solution.mse;
        // The least-squares solution has the line's gain in it.
        self.gain = super::Agc::unity();
        self.slicer = training.slicer;
        if self.options.rewind_safely {
            // Copies of loops from before a training are of another line.
            self.earlier.clear();
        }
        self.mode = Mode::Tracking;
        let via = if second { Via::Retry } else { Via::First };
        self.heard.push_back(Heard::Trained { snr_db: self.trained_snr, via });
    }

    /// The equaliser solved for from the known sequence
    /// (`v34/receiver.rs:786-845`).
    fn solve_known(&self, training: &Training, window: Window) -> Option<Solution> {
        let ((search_from, search_to), (from, to)) = (window.align, window.solve);
        let start = training.start;
        let length = to.max(search_to);
        if training.targets.len() < length {
            return None;
        }
        let rows = self.training_rows(training, window);
        // The targets turned as the carrier is known to turn, if it is.
        let known: Vec<Complex> = match training.turn {
            Some(turn) => {
                training.targets[..length].iter().enumerate().map(|(k, t)| *t * Complex::from_polar(1.0, turn * k as f64)).collect()
            }
            None => training.targets[..length].to_vec(),
        };
        let deltas: Vec<i64> = if window.search > PRESCORED_BEYOND {
            // Too many places to solve at each, so the samples themselves are
            // set against the sequence first -- on a line with one strong
            // path, as a VoIP call is, that alone points to the place -- and
            // only the few around the best are solved at.
            let mut scored: Vec<(f64, i64)> = (-window.search..=window.search)
                .filter_map(|delta| {
                    let origin = start.checked_add_signed(delta)?;
                    let mut sum = Complex::ZERO;
                    for (k, target) in known.iter().enumerate().take(search_to).skip(search_from) {
                        sum += rows.at(origin + 2 * k as u64)? * target.conj();
                    }
                    Some((sum.norm_sqr(), delta))
                })
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            let best = scored.first()?.1;
            (best - 3..=best + 3).collect()
        } else {
            (-window.search..=window.search).collect()
        };
        // Each alignment either side of where the sequence was said to be.
        let mut fits: Vec<(f64, i64, Vec<Complex>)> = Vec::new();
        for delta in deltas {
            let Some(origin) = start.checked_add_signed(delta) else { continue };
            let refs: Option<Vec<&[Complex]>> = (search_from..search_to).map(|k| rows.row(origin + 2 * k as u64)).collect();
            let Some(refs) = refs else { continue };
            let wanted = &known[search_from..search_to];
            let Some(taps) = least_squares(&refs, wanted, ridge(&refs)) else { continue };
            let mse = residual(&refs, wanted, &taps);
            fits.push((mse, delta, taps));
        }
        let best = fits.iter().map(|f| f.0).fold(f64::INFINITY, f64::min);
        let chosen = if self.options.centred_training {
            // On a line without long echoes every alignment within several
            // half symbols fits about as well as every other, the equaliser
            // moving its weight to wherever the sequence was put, and the
            // best of them is a matter of noise: measured, all seventeen
            // within a quarter of a decibel, and the one chosen three half
            // symbols out on one run and not on the next. Taps trained off
            // centre have less room on one side for the line and for the
            // anchor, so of those that fit nearly as well as the best, the
            // most central is taken.
            let margin = 10f64.powf(CENTRED_MARGIN_DB / 10.0);
            fits.iter()
                .filter(|f| f.0 <= best * margin)
                .min_by(|a, b| centroid(&a.2).abs().total_cmp(&centroid(&b.2).abs()))
        } else {
            // The first of the best, as V.34 kept it.
            fits.iter().find(|f| f.0 == best)
        };
        let (_, delta, taps) = chosen?.clone();
        let origin = start.saturating_add_signed(delta);
        // How fast the constellation turns, from the rough fit's residual
        // phase early and late in its window: all of the turn, or what is
        // left of it once the known turn is out.
        let refs: Vec<&[Complex]> = (search_from..search_to).filter_map(|k| rows.row(origin + 2 * k as u64)).collect();
        let middle = refs.len() / 2;
        let lean = |range: std::ops::Range<usize>| {
            range.fold(Complex::ZERO, |sum, i| sum + apply(&taps, refs[i]) * known[search_from + i].conj())
        };
        let (early, late) = (lean(0..middle), lean(middle..refs.len()));
        let left = (late * early.conj()).arg() / middle.max(1) as f64;
        let turn = match training.turn {
            Some(known) => known + left,
            None => left,
        };
        // The whole window, with the turn put into the targets for the
        // equaliser to follow and the carrier loop to take back out.
        let refs: Option<Vec<&[Complex]>> = (from..to).map(|k| rows.row(origin + 2 * k as u64)).collect();
        let refs = refs?;
        let turned: Vec<Complex> =
            (from..to).map(|k| training.targets[k] * Complex::from_polar(1.0, turn * k as f64)).collect();
        let taps = least_squares(&refs, &turned, ridge(&refs))?;
        let mse = residual(&refs, &turned, &taps);
        Some(Solution { taps, turn, rotation: turn * to as f64, origin, end: to, mse })
    }

    /// Every half-symbol sample a try at `window` can read: as they were
    /// sampled, or, if the far clock's drift is known, read again on a grid
    /// that follows it and meets the old one where the sequence starts.
    fn training_rows(&self, training: &Training, window: Window) -> Rows {
        let reach = window.search.max(0) as u64 + REACH as u64 + 1;
        let first = training.start.saturating_sub(reach).max(self.front.first);
        let last = (training.start + 2 * window.solve.1.max(window.align.1) as u64 + reach).min(self.front.made);
        let halves = match training.drift {
            None => {
                let (a, b) = ((first - self.front.first) as usize, (last.max(first) - self.front.first) as usize);
                self.front.halves.range(a..b).copied().collect()
            }
            Some(drift) => {
                let step = self.front.half * (1.0 + drift);
                let anchor = training.start.max(self.front.first);
                (first..last)
                    .map_while(|h| self.front.regridded(anchor, step, h).and_then(|time| self.front.interpolate(time)))
                    .collect()
            }
        };
        Rows { first, halves }
    }

    /// Nothing fitted, but an earlier training did: the line is likely the
    /// same, and only where the symbols fall and how the carrier is turned
    /// are new, which is what a resync finds (design.md 3.2). The search is
    /// on the newest symbols, and on success the receiver tracks from there.
    fn fall_back(&mut self) -> bool {
        self.next_symbol = self.front.made.saturating_sub(REACH as u64 + 2);
        self.lost = None;
        let dense = self.slicer.density() == super::Density::Dense;
        let found = if dense { self.search_dense(self.turn) } else { self.search_sparse(self.turn) };
        let Some(found) = found else { return false };
        if found.mse > self.slicer.found_level(self.settled) || found.mse > 0.6 * found.median {
            return false;
        }
        self.take_up(&found, false);
        self.settled = found.mse;
        self.error = found.mse;
        self.residual = found.mse.sqrt();
        self.trained_snr = db(found.mse);
        self.mode = Mode::Tracking;
        self.heard.push_back(Heard::Trained { snr_db: self.trained_snr, via: Via::Fallback });
        true
    }
}

fn residual(rows: &[&[Complex]], targets: &[Complex], taps: &[Complex]) -> f64 {
    rows.iter().zip(targets).map(|(row, &d)| (apply(taps, row) - d).norm_sqr()).sum::<f64>() / rows.len().max(1) as f64
}

/// A ridge a thousandth of the signal's own weight on the diagonal
/// (`v34/receiver.rs:1280-1283`).
fn ridge(rows: &[&[Complex]]) -> f64 {
    let energy: f64 = rows.iter().flat_map(|row| row.iter()).map(|x| x.norm_sqr()).sum();
    1e-3 * energy / (2 * REACH + 1) as f64
}
