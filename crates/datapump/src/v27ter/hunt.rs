//! Finding a V.27 ter burst: the turn-on sequence's reversals and its
//! conditioning pattern, where they are, and what they say about the far
//! carrier.
//!
//! The receiver this replaced found a burst by its level alone, against a
//! fixed threshold 53 dB under it, and measured the carrier once, over the
//! 48 symbols after the edge. Any hiss above the threshold was a burst, the
//! measurement was spent on it, and nothing ever measured again
//! (fax-qam.md 3.1, 3.2). So here a burst is what V.27 ter says the front of
//! one is (2.5.1): continuous 180 degree reversals, and then the equaliser
//! conditioning pattern, symbol for symbol as Table 4 prints it. Hiss is
//! neither, an unmodulated carrier in front for talker echo protection is
//! neither (5.2.1), and the hunt never stops, so whatever it took for a burst
//! that was not one, the next real one is found anyway.
//!
//! Reversals on the line are two tones, half the symbol rate either side of
//! the carrier: an alternating sequence through a Nyquist pulse is a cosine
//! at half the symbol rate. Their share of the in-band power says reversals
//! are arriving, which is what raises the carrier flag. Where the symbols are
//! centred comes from the signal's power, which swells at every centre and
//! so has a line at the symbol rate whatever the symbols are -- reversals,
//! the two-phase conditioning, or data. With the symbols centred, each
//! symbol's change from the one before says reversal or not, and the pattern
//! of changes segment 4 begins with marks exactly where it starts: it is
//! every third bit of a maximal-length sequence, itself one, so no other
//! alignment of it matches.
//!
//! The pattern is also found without the reversals in front of it. The fax
//! layer turns the line round to listen when its procedure says to, and on a
//! page that has been measured at 75 ms after the far end's turn-on began --
//! after the whole of segment 3. Segment 4's changes repeat every 127
//! symbols, 1074 being eight periods and 58 over, so any 32 of them say where
//! in that period they are, and the conditioning from there on is as known,
//! but for a half turn, as it is from the start.
//!
//! And squaring a two-phase symbol removes its phase, so the squares over
//! the reversals and the conditioning turn by twice the carrier offset,
//! which is all the core needs to be told to train on the rest (dsp::qam,
//! train.rs).

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};

use dsp::{Complex, ComplexFir, Nco, rrc_taps};

use super::{CARRIER, ROLLOFF, SPAN, conditioning_changes};

/// How long the reversals' tones are averaged over, in symbols: a quarter of
/// the short turn-on's fourteen, so that they are heard well inside it.
const AVERAGE_SYMBOLS: f64 = 4.0;

/// The share of the in-band power the two tones must carry for reversals to
/// be arriving, and the share below which they have stopped. Hiss puts about
/// a sixteenth there, data and the conditioning pattern as much, and the
/// unmodulated carrier of talker echo protection nothing.
const REVERSING: f64 = 0.7;
const STOPPED: f64 = 0.45;

/// How unequal the two tones may be. Reversals put the same power in each,
/// less whatever tilt the line has between 1000 and 2600 Hz (1200 and 2400
/// at 1200 baud); a single tone that happens to sit on one of them -- a
/// Bell 202 mark is 1200 Hz -- puts it all in one.
const BALANCE_ON: f64 = 0.1;
const BALANCE_OFF: f64 = 0.05;

/// Symbols the reversals must hold for before they count.
const HOLD_SYMBOLS: f64 = 3.0;

/// How long the symbol-rate line in the signal's power is averaged over, in
/// symbols: long enough to steady it on data, short enough that the short
/// turn-on's fourteen reversals move it most of the way.
const TIMING_SYMBOLS: f64 = 16.0;

/// Reversals leading into segment 4, and symbols of its pattern, that must
/// match for its start to be found; and how well, as a share of a perfect
/// match. Thirty-two symbols of a maximal-length sequence match any other
/// alignment of it by a quarter at most.
const PREFIX: usize = 8;
const PATTERN: usize = 32;
const MATCHED: f64 = 0.75;

/// Segment 4's period, in symbols: its pattern is every third bit of a
/// sequence of period 127, and three and 127 have no factor in common.
const PERIOD: usize = 127;

/// Symbols after reversals were heard for which segment 4's start is looked
/// for, before they are taken for something else.
const PATIENCE: usize = 160;

/// How far two-phase the last 32 symbols have to be for segment 4 to be
/// looked for anywhere in its period, and how far from it for the search to
/// be armed again once a burst has been found: the conditioning is two-phase
/// throughout, and a burst's data is not.
const TWO_PHASE: f64 = 0.6;
const NOT_TWO_PHASE: f64 = 0.3;

/// Where the long turn-on and the short one first differ: the short one's
/// segment 5 begins 58 symbols into segment 4, where the long one's
/// conditioning goes on for another 1016. Whether the squares still line up
/// is judged over 64 symbols from 40 after that, so that a jitter buffer's
/// 20 ms repeated inside a short one's conditioning -- 32 symbols at 1600
/// baud -- is not taken for more of it. Found without its start, the
/// conditioning is judged from just after the symbols it was found by.
const SHORT_CONDITIONING: usize = 58;
const LENGTH_FROM: usize = SHORT_CONDITIONING + 40;
const LATE_FROM: usize = PATTERN + 8;
const LENGTH_OVER: usize = 64;
const COHERENT: f64 = 0.5;

/// Symbols kept, the newest last: enough for the reversals before segment 4,
/// all of what is measured after it, and some.
const READS_KEPT: usize = 512;

/// Symbols of reversals before segment 4 the carrier's turn is measured
/// over, at most: the long turn-on's fifty.
const TURN_BEFORE: usize = 50;

/// Line samples kept, filtered, for reading symbols from.
const KEPT: usize = 512;

/// The in-band amplitude over which a sample counts as signal at all: 80 dB
/// under a burst. Only digital silence is below it.
const AUDIBLE: f64 = 1e-4;

/// What the hunt has found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Found {
    /// Reversals, as segment 3 has them.
    Reversals,
    /// Segment 4's start. `start` is the line sample, fractional, that the
    /// centre of the fourteenth symbol before segment 4 arrived at -- the
    /// short turn-on's first -- and `turn` the far carrier's turn a symbol
    /// against ours, in radians, from the squares so far.
    Conditioning { start: f64, turn: f64 },
    /// Segment 4 without its start: the symbol centred on line sample `at`
    /// is symbol `phase` of its period, or that plus some whole periods, and
    /// the first of the 32 it was found by was centred on `first`. Its
    /// changes repeat every period; its phases every other, a period holding
    /// 63 reversals.
    Late { at: f64, phase: usize, first: f64, turn: f64 },
    /// Whether the conditioning went on well past where it was found -- past
    /// where the short turn-on's ends, if its start was found -- and the turn
    /// from all of it.
    Length { long: bool, turn: f64 },
    /// Reversals that segment 4 did not follow.
    Nothing,
}

/// One symbol read at the centres the signal's power gives.
#[derive(Debug, Clone, Copy)]
struct Read {
    /// The line sample it was centred on.
    at: f64,
    z: Complex,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    /// Looking for segment 4: its start, if reversals were heard in the
    /// last while, and anywhere in its period if not.
    Looking,
    /// Segment 4 found at symbol `from` (a count of the symbols read), its
    /// start or not: reading on to see how long it goes on.
    Measuring { from: usize, late: bool },
    /// A burst found: nothing more is until its conditioning is over.
    Found,
}

#[derive(Debug, Clone)]
pub(super) struct Hunt {
    sps: f64,
    nco: Nco,
    matched: ComplexFir,
    /// The matched filter's delay, in samples.
    delay: f64,
    level_keep: f64,
    level: f64,
    /// The reversals' two tones, each brought to nought and averaged, the
    /// power they are a share of, and whether they are arriving.
    keep: f64,
    tone_step: f64,
    plus: Complex,
    minus: Complex,
    power: f64,
    reversing: bool,
    held: usize,
    /// The symbol-rate line in the signal's power, averaged.
    timing_keep: f64,
    timing: Complex,
    /// Filtered samples, the newest last, and the line sample the newest was
    /// taken at.
    kept: VecDeque<Complex>,
    newest: u64,
    /// Where the next symbol is centred, as a line sample; the symbols read
    /// so far, newest last, and how many have been.
    next: f64,
    reads: VecDeque<Read>,
    count: usize,
    /// The count at which reversals were last heard.
    reversals: Option<usize>,
    stage: Stage,
    /// Segment 4's changes through one period: 1 for none, -1 for a
    /// reversal.
    pattern: Vec<f64>,
}

impl Hunt {
    pub(super) fn new(fs: f64, baud: f64) -> Self {
        let sps = fs / baud;
        // The matched filter, at unit gain for a steady signal, so that the
        // level reads in the same units a plain low-pass would.
        let mut taps = rrc_taps(sps, ROLLOFF, SPAN);
        let sum: f64 = taps.iter().sum();
        for t in &mut taps {
            *t /= sum;
        }
        let delay = (taps.len() / 2) as f64;
        Self {
            sps,
            nco: Nco::new(CARRIER, fs),
            matched: ComplexFir::new(taps),
            delay,
            level_keep: (-1.0 / (0.010 * fs)).exp(),
            level: 0.0,
            keep: (-1.0 / (AVERAGE_SYMBOLS * sps)).exp(),
            tone_step: 0.5 / sps,
            plus: Complex::ZERO,
            minus: Complex::ZERO,
            power: 0.0,
            reversing: false,
            held: 0,
            timing_keep: (-1.0 / (TIMING_SYMBOLS * sps)).exp(),
            timing: Complex::ZERO,
            kept: VecDeque::with_capacity(KEPT + 1),
            newest: 0,
            next: 0.0,
            reads: VecDeque::with_capacity(READS_KEPT + 1),
            count: 0,
            reversals: None,
            stage: Stage::Looking,
            pattern: conditioning_changes(PERIOD),
        }
    }

    /// The in-band amplitude, over 10 ms.
    pub(super) fn level(&self) -> f64 {
        self.level
    }

    /// Start listening again from nothing, the line as it is now.
    pub(super) fn reset(&mut self) {
        self.matched.reset();
        self.level = 0.0;
        self.plus = Complex::ZERO;
        self.minus = Complex::ZERO;
        self.power = 0.0;
        self.reversing = false;
        self.held = 0;
        self.timing = Complex::ZERO;
        self.kept.clear();
        self.reads.clear();
        self.reversals = None;
        self.stage = Stage::Looking;
    }

    /// Take line sample `index`.
    pub(super) fn feed(&mut self, sample: f64, index: u64) -> Option<Found> {
        let (cos, sin) = self.nco.step();
        let y: Complex = self.matched.process((sample * cos, -sample * sin)).into();
        if self.kept.is_empty() {
            // Reading starts from the first sample there is.
            self.next = self.line_time(index) + self.sps;
        }
        self.newest = index;
        self.kept.push_back(y);
        if self.kept.len() > KEPT {
            self.kept.pop_front();
        }
        self.level = self.level_keep * self.level + (1.0 - self.level_keep) * y.abs();
        // Both counted from line sample nought, so that their phases say
        // where things are in line samples.
        let at = index as f64 * self.tone_step;
        self.timing = self.timing.scale(self.timing_keep)
            + Complex::from_polar(y.norm_sqr() * (1.0 - self.timing_keep), -TAU * (2.0 * at).fract());
        let mut found = self.listen_for_reversals(y, at);
        // Every symbol whose samples are in.
        while let Some(z) = self.read(self.next) {
            self.reads.push_back(Read { at: self.next, z });
            if self.reads.len() > READS_KEPT {
                self.reads.pop_front();
            }
            self.count += 1;
            self.next = self.align(self.next + self.sps);
            if let Some(f) = self.on_symbol() {
                found = Some(f);
            }
        }
        found
    }

    /// The reversals' tones, and whether they are arriving: said once, when
    /// they have held for long enough to count.
    fn listen_for_reversals(&mut self, y: Complex, at: f64) -> Option<Found> {
        let spin = Complex::from_polar(1.0, -TAU * at.fract());
        let k = 1.0 - self.keep;
        self.plus += (y * spin - self.plus).scale(k);
        self.minus += (y * spin.conj() - self.minus).scale(k);
        self.power += k * (y.norm_sqr() - self.power);
        let (p, m) = (self.plus.norm_sqr(), self.minus.norm_sqr());
        let share = (p + m) / self.power.max(AUDIBLE * AUDIBLE);
        let balance = p.min(m) / p.max(m).max(1e-30);
        self.reversing = if self.reversing {
            share > STOPPED && balance > BALANCE_OFF
        } else {
            share > REVERSING && balance > BALANCE_ON && self.power > AUDIBLE * AUDIBLE
        };
        self.held = if self.reversing { self.held + 1 } else { 0 };
        if self.held as f64 >= HOLD_SYMBOLS * self.sps && !matches!(self.stage, Stage::Measuring { .. }) {
            // Reversals: a burst's front, or a new burst's once the last
            // one's conditioning is over. Not while a burst's length is
            // being measured: the conditioning has runs of six reversals in
            // it, and that is nearly enough to be heard as reversals.
            let new = self.reversals.is_none_or(|r| self.count > r + PATIENCE) || self.stage != Stage::Looking;
            self.reversals = Some(self.count);
            if new {
                self.stage = Stage::Looking;
                return Some(Found::Reversals);
            }
        }
        None
    }

    /// The line sample a filtered sample taken at `index` is centred on.
    fn line_time(&self, index: u64) -> f64 {
        index as f64 - self.delay
    }

    /// The symbol centre the signal's power gives that is nearest line
    /// sample `near`, a quarter of the way there.
    ///
    /// Through a Nyquist pulse the power swells at every symbol's centre, so
    /// its line at the symbol rate peaks there: a line of phase
    /// -2 pi n0 / sps for centres at n0, counted on the filtered samples,
    /// whose centres lie `delay` samples earlier on the line. A quarter at a
    /// time, so that one reading off in the noise does not move the symbols
    /// by much.
    fn align(&self, near: f64) -> f64 {
        if self.timing.norm_sqr() == 0.0 {
            return near;
        }
        let centre = -self.sps * self.timing.arg() / TAU - self.delay;
        let off = (near - centre).rem_euclid(self.sps);
        let off = if off > self.sps / 2.0 { off - self.sps } else { off };
        near - 0.25 * off
    }

    /// The filtered signal at line sample `at`, by linear interpolation, once
    /// the samples either side are in.
    fn read(&self, at: f64) -> Option<Complex> {
        let t = at + self.delay;
        let first = self.newest as f64 - (self.kept.len() as f64 - 1.0);
        let offset = t - first;
        if offset < 0.0 {
            return Some(Complex::ZERO);
        }
        let i = offset.floor() as usize;
        let (a, b) = (self.kept.get(i)?, self.kept.get(i + 1)?);
        let f = offset - offset.floor();
        Some(*a + (*b - *a).scale(f))
    }

    /// The symbol read `back` before the newest.
    fn back(&self, back: usize) -> Complex {
        self.reads.len().checked_sub(1 + back).map_or(Complex::ZERO, |i| self.reads[i].z)
    }

    /// Where the symbol that was the `count`th read is among those kept.
    fn index(&self, count: usize) -> Option<usize> {
        let first = self.count - self.reads.len();
        count.checked_sub(first).filter(|&i| i < self.reads.len())
    }

    /// Whether the symbol `back` before the newest changed from the one
    /// before it, softly: one for no change, minus one for a reversal.
    fn change(&self, back: usize) -> f64 {
        let (a, b) = (self.back(back + 1), self.back(back));
        (b * a.conj()).re / (a.abs() * b.abs()).max(1e-30)
    }

    /// How far the last `n` symbols' squares point one way.
    fn two_phase(&self, n: usize) -> f64 {
        let (sum, total) = (0..n).fold((Complex::ZERO, 0.0), |(s, t), k| {
            let z = self.back(k);
            (s + z * z, t + z.norm_sqr())
        });
        sum.abs() / total.max(1e-30)
    }

    fn on_symbol(&mut self) -> Option<Found> {
        match self.stage {
            Stage::Looking => self.look(),
            Stage::Measuring { from, late } => self.measure(from, late),
            Stage::Found => {
                if self.two_phase(PATTERN) < NOT_TWO_PHASE {
                    self.stage = Stage::Looking;
                }
                None
            }
        }
    }

    fn look(&mut self) -> Option<Found> {
        if let Some(heard) = self.reversals
            && self.count <= heard + PATIENCE
        {
            if self.starts_segment_4() {
                let from = self.count - PATTERN;
                let i = self.index(from)?;
                let start = self.reads[i].at - 14.0 * self.sps;
                let turn = turn_of(self.reads.range(i.saturating_sub(TURN_BEFORE)..));
                self.stage = Stage::Measuring { from, late: false };
                self.reversals = None;
                return Some(Found::Conditioning { start, turn });
            }
            return (self.count == heard + PATIENCE).then_some(Found::Nothing);
        }
        // Looked for anywhere in its period only now and then, and only
        // when the symbols are two-phase at all.
        if !self.count.is_multiple_of(4) || self.two_phase(PATTERN) < TWO_PHASE {
            return None;
        }
        let phase = self.anywhere_in_segment_4()?;
        let from = self.count - PATTERN;
        let i = self.index(from)?;
        let turn = turn_of(self.reads.range(i..));
        self.stage = Stage::Measuring { from, late: true };
        // Said of the newest symbol, which the receiver surely has samples
        // for, rather than the first, which may have been heard only in part.
        let newest = self.reads.len() - 1;
        let (at, first) = (self.reads[newest].at, self.reads[i].at);
        Some(Found::Late { at, phase: (phase + PATTERN - 1) % PERIOD, first, turn })
    }

    fn measure(&mut self, from: usize, late: bool) -> Option<Found> {
        let first = if late { LATE_FROM } else { LENGTH_FROM };
        if self.count < from + first + LENGTH_OVER {
            return None;
        }
        let i = self.index(from)?;
        let (before, known) = if late { (0, PATTERN) } else { (TURN_BEFORE, SHORT_CONDITIONING) };
        let turn = turn_of(self.reads.range(i.saturating_sub(before)..i + known));
        // Whether the squares, the carrier's turn taken out, still point one
        // way: two-phase symbols do, eight or four phases do not.
        let window = self.reads.range(i + first..i + first + LENGTH_OVER);
        let (sum, total) = window.enumerate().fold((Complex::ZERO, 0.0), |(s, t), (k, r)| {
            let back = Complex::from_polar(1.0, -2.0 * turn * k as f64);
            (s + r.z * r.z * back, t + r.z.norm_sqr())
        });
        let long = sum.abs() > COHERENT * total;
        let turn = if long { turn_of(self.reads.range(i.saturating_sub(before)..)) } else { turn };
        self.stage = Stage::Found;
        Some(Found::Length { long, turn })
    }

    /// Whether the newest symbols are segment 4's first thirty-two changes,
    /// with reversals leading into them.
    ///
    /// Each judged on its own. Inside the long turn-on's conditioning, every
    /// 127 symbols, the pattern's first thirty-two come round again behind
    /// eight changes of which half are reversals, and the two together pass
    /// for the start at 0.8.
    fn starts_segment_4(&self) -> bool {
        if self.reads.len() < PREFIX + PATTERN + 1 {
            return false;
        }
        let lead: f64 = (PATTERN..PATTERN + PREFIX).map(|b| -self.change(b)).sum();
        let pattern: f64 = (0..PATTERN).map(|i| self.change(PATTERN - 1 - i) * self.pattern[i]).sum();
        lead / PREFIX as f64 > MATCHED && pattern / PATTERN as f64 > MATCHED
    }

    /// Where in segment 4's period the newest thirty-two changes are, if
    /// they are any part of it: the period's symbol the first of them is.
    fn anywhere_in_segment_4(&self) -> Option<usize> {
        if self.reads.len() < PATTERN + 1 {
            return None;
        }
        let changes: Vec<f64> = (0..PATTERN).map(|i| self.change(PATTERN - 1 - i)).collect();
        let (best, phase) = (0..PERIOD)
            .map(|p| (changes.iter().enumerate().map(|(i, c)| c * self.pattern[(p + i) % PERIOD]).sum::<f64>(), p))
            .fold((f64::NEG_INFINITY, 0), |a, b| if b.0 > a.0 { b } else { a });
        (best / PATTERN as f64 > MATCHED).then_some(phase)
    }
}

/// The far carrier's turn a symbol, in radians, from two-phase symbols:
/// their squares turn by twice it. Each square against the one before gives
/// it to within a quarter turn either way; against the one a dozen or so
/// before, far more finely but only to within that many times less, so the
/// first says which of the second's answers is the one.
fn turn_of<'a>(reads: impl Iterator<Item = &'a Read>) -> f64 {
    let squares: Vec<Complex> = reads.map(|r| r.z * r.z).collect();
    let lagged = |lag: usize| squares.windows(lag + 1).fold(Complex::ZERO, |sum, w| sum + w[lag] * w[0].conj());
    if squares.len() < 2 {
        return 0.0;
    }
    let coarse = lagged(1).arg() / 2.0;
    let lag = (squares.len() / 4).clamp(1, 16);
    let fine = lagged(lag).arg() / 2.0;
    let wraps = ((lag as f64 * coarse - fine) / PI).round();
    (fine + wraps * PI) / lag as f64
}
