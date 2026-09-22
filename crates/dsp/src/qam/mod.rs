//! A QAM receiver's machinery, for any modulation that sends a known
//! sequence to train on: the far end's points brought back from the line.
//!
//! Copied from V.34's receiver (`crates/datapump/src/v34/receiver.rs`),
//! which is live-proven, and which trains to 54 dB at V.32's own 2400 baud on
//! 1800 Hz (core.md 0, 6). What is copied is the part that is not V.34's own
//! (core.md 5, right-hand column): the front end, the hunt for S and S-bar,
//! least squares on a known sequence, the three loops and their one gate,
//! loss, rewind and the two resyncs. V.34's own parts -- its sequences and
//! windows, its constellations' grid units, `reacquire` -- stay with V.34.
//! V.34 itself is not moved onto this yet; that is its own job, behind a
//! bit-exact golden test (core.md 8).
//!
//! The receiver is V.34's, and so is why it holds up:
//!
//! - a fixed mixer, so that everything after it can be read again from stored
//!   samples: after a slip, or as a least-squares solve's rows;
//! - one interpolating low-pass that both rejects the image and places every
//!   sample exactly where the timing asks;
//! - an equaliser sampling twice a symbol, which makes it indifferent to where
//!   in the symbol the sampling falls, so that the timing loop only has to
//!   stop the drift;
//! - the equaliser solved for outright on the known sequence, with no blind
//!   stage in a call;
//! - three loops four times apart in speed -- carrier 50 symbols, timing 200,
//!   equaliser about 870 -- each faster than the equaliser it shares a degree
//!   of freedom with, so it takes that freedom first (core.md 3.4);
//! - one gate that freezes every loop together;
//! - loss detection, rewind, and reading the raw samples afresh.
//!
//! V.34's measured defects are fixed in the copy (core.md 4.3, 9; design.md
//! 2-4), each behind a field of [`Options`] whose default is V.34's
//! behaviour, so that V.34 can move onto the core mechanically later. Some
//! were found while proving the others, and are fixed the same way.
//!
//! - The plain resync's window ends four symbols short of the newest symbol,
//!   as the dense one's does, which closes the look-ahead hole a timing jump
//!   of +0.25 to +0.35 symbol fell into ([`Options::resync_margin`]); and
//!   neither resync reads symbols from before the loss began
//!   ([`Options::resync_after_loss`]).
//! - Both resyncs fit the gain as well as the phase, from the window's own
//!   power ([`Options::resync_gain`]); take the carrier's turn out across the
//!   window ([`Options::resync_derotate`]); place the plain one's timing
//!   between its steps ([`Options::resync_fine`]); and carry the phase on to
//!   the symbol that comes next, not the window's middle
//!   ([`Options::extrapolate`]).
//! - A gain control holds the output at the constellation's mean power,
//!   bounded by a decision-free average and trimmed by the decisions within
//!   the bound ([`Options::agc`]).
//! - The gate is relative to what the errors settle to, and the signal is
//!   lost when the gate refuses most symbols or the errors climb 6 dB above
//!   what they settled to a while before ([`Options::relative_gate`]).
//! - A rewind with nothing old enough goes back as far as it can
//!   ([`Options::rewind_safely`]); it always says whether it found a copy.
//! - The hunt tells S from V.32's preamble tones ([`Options::discriminator`])
//!   and always measures the carrier's frequency and the far clock's drift
//!   from S, which training uses when told to ([`Training::turn`],
//!   [`Training::drift`]); and training takes the most central of the
//!   alignments that fit nearly as well as the best
//!   ([`Options::centred_training`]).
//! - A table's phase error is not divided by the point's power
//!   ([`Options::unweighted_phase`]), and the carrier loop's frequency is
//!   limited ([`Options::turn_limit_hz`]).
//! - A resync is not tried on a line that has gone quiet ([`Options::level_gate`]).
//! - The taps' centre of weight is kept near the middle ([`Options::anchor`]).
//!
//! It is pull-style, so that the driver can supply the decision the loops
//! track: [`Core::feed`] takes a sample, [`Core::next`] gives the next
//! equalised point once its samples are in, and [`Core::settle`] takes the
//! driver's decision for it and runs everything that follows from one
//! symbol. The core knows nothing about trellis codes; it slices to the
//! nearest point itself for loss, resync and the signal-to-noise figures.

mod blind;
mod front;
mod hunt;
mod resync;
mod slicer;
mod track;
mod train;

use std::collections::VecDeque;

pub use slicer::{Constellation, Density, Slicer};
pub use train::{Training, Window};

use crate::Complex;
use front::Front;
use hunt::{Hunt, Hunted};
use track::{Agc, Loops, Pending};

/// Equaliser taps either side of the centre, in half symbols: 31 taps,
/// fifteen and a half symbols of the line's memory (`v34/receiver.rs:62`).
const REACH: usize = 15;

/// Least signal a half-symbol sample has to carry to be S: 37 dB under the
/// nominal level (`v34/receiver.rs:94`).
const AUDIBLE: f64 = 4e-4;

/// Symbols read afresh to find where the signal went after a slip, and how
/// often to look while it is lost (`v34/receiver.rs:102-103`).
const RESYNC_WINDOW: usize = 48;
const RESYNC_EVERY: usize = 32;

/// Symbols between the copies of the loops kept for putting back, and how
/// many are kept (`v34/receiver.rs:112-113`).
const EARLIER_EVERY: u64 = 16;
const EARLIER_KEPT: usize = 24;

/// The part of a line, in hertz, a receiver listens to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// Samples a second on the line.
    pub fs: f64,
    /// Symbols a second.
    pub baud: f64,
    /// The carrier, which the fixed mixer takes out.
    pub carrier: f64,
    /// The interpolating low-pass filter's cutoff.
    pub cutoff: f64,
}

impl Band {
    /// A band with V.34's cutoff: half the symbol rate with a tenth of
    /// roll-off, and 300 Hz to spare, but no more than 0.45 of the sampling
    /// rate (`v34/receiver.rs:456`). At 2400 baud that is 1620 Hz, which
    /// leaves V.32's image at least 39 dB down at 16 kHz (core.md 1).
    pub fn new(fs: f64, baud: f64, carrier: f64) -> Self {
        let cutoff = (0.5 * baud * 1.1 + 300.0).min(0.45 * fs);
        Self { fs, baud, carrier, cutoff }
    }
}

/// Where the core differs from V.34's receiver. The default is V.34's
/// behaviour in every field; [`Options::fixed`] turns every fix on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// End the plain resync's window four symbols short of the newest
    /// symbol, as the dense one's is. V.34's ends at the newest, so any shift
    /// later than the sample or so of slack needs samples not yet taken, and
    /// is skipped: on sixteen points at 2400 baud a timing jump of +0.25 to
    /// +0.35 symbol was never recovered (core.md 4.3, E3e).
    pub resync_margin: bool,
    /// Fit the gain, as well as the phase, in the plain resync, as the dense
    /// one does; and start both fits from the gain that brings the window's
    /// outputs to the constellation's mean power, not from the gain as it
    /// was. V.34's plain resync fits no gain, and on sixteen points a 3 dB
    /// step was lost for good (core.md 7.2, E4); and a fit started from the
    /// old gain after a -6 dB step settles on the gain the wrong decisions
    /// make, every point decided as the inner one of its quadrant.
    pub resync_gain: bool,
    /// Take the carrier's known turn out across a resync's window, so that
    /// one phase fits the whole window at any frequency offset. V.34 fits
    /// one phase to the raw window, which at 7 Hz is 50 degrees of turn over
    /// 48 symbols.
    pub resync_derotate: bool,
    /// Place the plain resync's timing between its eighth-of-a-half-symbol
    /// steps, by the parabola through the best step's error and its
    /// neighbours'. V.34 takes the best step, and leaves the rest to the
    /// timing loop's 200 symbols: measured over every drop of 1 to 340
    /// samples on sixteen points at 35 dB, the 200 symbols after the slip
    /// read 0.1 dB better on the mean and 0.2 dB at worst with it.
    pub resync_fine: bool,
    /// Read only symbols from after the signal was lost in a resync. V.34's
    /// first plain resync, 32 symbols after the loss, reads a window that
    /// begins about ten symbols before the jump: those fit nothing, and a
    /// reading that passes anyway has its phase, timing and -- once the gain
    /// is fitted -- gain pulled off by them. On sixteen points at 35 dB it
    /// left the gain a quarter of a decibel out and the timing loop 26 ppm
    /// off after a slip.
    pub resync_after_loss: bool,
    /// Hold the output's mean power to the constellation's: a gain kept by an
    /// average of every symbol's power, with no decisions in it, to within
    /// how far that average wanders on its own, and trimmed within that from
    /// the decisions the gate accepts. V.34 has no gain control: all its gain
    /// lives in the taps, and the gate's tolerance of a gain error falls to
    /// 0.64 dB on V.32's 128 points (core.md 7.2). The average alone, at
    /// design.md 2.2's time constants, held a clean sixteen-point line to
    /// 36.6 dB with its own noise.
    pub agc: bool,
    /// Gate on `min(d²/4, max(9·settled, d²/400))` rather than V.34's `d²/4`;
    /// declare the signal lost when the gate refuses three quarters of the
    /// symbols judged, or when the errors climb 6 dB above what they had
    /// settled to up to 384 symbols before; and take a resync's error as the
    /// new settled error, but no more than 6 dB worse than the old. V.34's
    /// absolute gate let a -6 dB ramp on sixteen points lock falsely at
    /// 12 dB, with `settled` rising to meet it (core.md 4.1, E4b); a relative
    /// gate alone let a ramp on the 128-point cross do the same, a little at
    /// a time.
    pub relative_gate: bool,
    /// A rewind with no copy old enough restores the oldest there is, and
    /// the copies from before a training or a resync are dropped. V.34's
    /// rewind silently did nothing (`v34/receiver.rs:999`), and could restore
    /// loops from before a training.
    pub rewind_safely: bool,
    /// Carry the carrier's phase after a resync on to the next symbol. V.34
    /// sets it for the middle of the window and half a window on, which on
    /// the dense resync leaves it 4.5 symbols of turn stale (core.md 9, 7.6).
    pub extrapolate: bool,
    /// Tell S from V.32's AA, CC, AC and CA by where its power is (core.md
    /// E6, design.md 3.4).
    pub discriminator: bool,
    /// Do not divide a table's phase error by the decided point's power
    /// (core.md 9, 7.9). V.34's grid keeps its own form either way.
    pub unweighted_phase: bool,
    /// Try a resync only when the line carries at least an eighth of the power
    /// it carried before the signal was lost, so that a trained receiver
    /// facing silence waits rather than searching it (core.md N4). An eighth
    /// rather than design.md 4.1's quarter: a -6 dB step leaves a quarter,
    /// give or take the noise, and must be searched.
    pub level_gate: bool,
    /// Every 1024 symbols, if the taps' centre of weight is more than a half
    /// symbol from the middle, move the taps a half symbol the other way and
    /// the symbols' centre a half symbol with them, unless the tap that would
    /// fall off the end carries more than -40 dB of the taps' energy
    /// (core.md N6, design.md 2.5).
    pub anchor: bool,
    /// Of the alignments of a training sequence that fit within half a
    /// decibel of the best, take the one that leaves the taps most central.
    /// V.34 takes the best, which on a line without long echoes is a matter
    /// of noise among a dozen that fit as well, and measured three half
    /// symbols out at 200 ppm.
    pub centred_training: bool,
    /// The most the carrier loop's frequency may reach, in hertz; V.34 has no
    /// limit.
    pub turn_limit_hz: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            resync_margin: false,
            resync_gain: false,
            resync_derotate: false,
            resync_fine: false,
            resync_after_loss: false,
            agc: false,
            relative_gate: false,
            rewind_safely: false,
            extrapolate: false,
            discriminator: false,
            unweighted_phase: false,
            level_gate: false,
            anchor: false,
            centred_training: false,
            turn_limit_hz: f64::INFINITY,
        }
    }
}

impl Options {
    /// V.34's receiver, as it is.
    pub fn v34() -> Self {
        Self::default()
    }

    /// Every fix on: the receiver V.32 is built on (design.md 2-4).
    pub fn fixed() -> Self {
        Self {
            resync_margin: true,
            resync_gain: true,
            resync_derotate: true,
            resync_fine: true,
            resync_after_loss: true,
            agc: true,
            relative_gate: true,
            rewind_safely: true,
            extrapolate: true,
            discriminator: true,
            unweighted_phase: true,
            level_gate: true,
            anchor: true,
            centred_training: true,
            turn_limit_hz: 20.0,
        }
    }
}

/// What the receiver is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Only keeping samples.
    Idle,
    /// Looking for S and S-bar.
    Hunting,
    /// Collecting the known sequence to train on.
    Training,
    /// Making symbols, the loops following them.
    Tracking,
    /// Looking for the constellation with no training sequence.
    Blind,
}

/// How a receiver came to be tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Least squares on the known sequence where it was expected.
    First,
    /// Least squares further into the sequence, searched wide.
    Retry,
    /// Neither fitted, and the taps an earlier training left were found
    /// again by a resync search.
    Fallback,
    /// The blind start.
    Blind,
}

/// What the receiver has to report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Heard {
    /// S, sure enough to have learned its template. Said once a hunt.
    S,
    /// S, and then the change to S-bar. `at` is the half-symbol sample S-bar
    /// is reckoned to start at, give or take one; `turn` is the carrier's
    /// turn a symbol against ours, in radians, and `drift` the far clock's
    /// against ours as the timing loop counts it (a fraction; the far clock
    /// fast makes it negative), both as S had them.
    Reversal { at: u64, turn: f64, drift: f64 },
    /// S stopped at half-symbol sample `at` without turning into S-bar. The
    /// hunt goes on.
    Lapsed { at: u64 },
    /// Tracking, with the signal to noise it started at.
    Trained { snr_db: f64, via: Via },
    /// Nothing trained: the sequence was not where it was said to be.
    Untrained,
}

/// A symbol equalised and waiting for its decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// The point, gain-controlled and with the carrier taken out, in the
    /// constellation's units.
    pub z: Complex,
    /// The nearest point of the constellation in use, and its label if the
    /// slicer is a table.
    pub nearest: Complex,
    pub label: Option<usize>,
    /// Symbols made before this one.
    pub index: u64,
}

/// A symbol settled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Symbol {
    pub point: Complex,
    /// The decision the loops were given.
    pub target: Complex,
    pub nearest: Complex,
    pub label: Option<usize>,
    /// Squared distance from the nearest point.
    pub error: f64,
    /// Whether the gate let it teach the loops.
    pub accepted: bool,
}

#[derive(Debug, Clone)]
enum Mode {
    Idle,
    Hunting(Box<Hunt>),
    Training(Box<train::Collecting>),
    Tracking,
    Blind(blind::Blind),
}

/// The far end's signal, brought back to points.
#[derive(Debug, Clone)]
pub struct Core {
    band: Band,
    options: Options,
    front: Front,
    mode: Mode,
    heard: VecDeque<Heard>,

    taps: Vec<Complex>,
    /// The half-symbol sample the next symbol is centred on.
    next_symbol: u64,
    slicer: Slicer,
    /// Carrier phase to take out of the next symbol, and its turn a symbol,
    /// in radians.
    rotation: f64,
    turn: f64,
    /// Mean squared size of the equaliser output's rate of change, a half
    /// symbol at a time, which turns an error into a timing error.
    slope: f64,
    /// Mean squared error of every symbol against its nearest point, and mean
    /// distance from it.
    error: f64,
    residual: f64,
    trained_snr: f64,
    /// The last symbol, equalised.
    last: Complex,
    /// Whether anything has trained this receiver yet.
    ever_trained: bool,
    /// The last few symbols' squared errors, whether the gate refused each,
    /// and what the errors come to when all is well.
    recent: VecDeque<f64>,
    refused: VecDeque<bool>,
    settled: f64,
    /// Symbols since the error jumped, while every loop is held, and the
    /// line's power when it did.
    lost: Option<usize>,
    power_before: f64,
    /// The half-symbol sample the first symbol judged lost was centred on.
    lost_from: u64,
    /// Every loop held from outside.
    held: bool,
    gain: Agc,
    /// Slips found and followed, resyncs tried, and rewinds that found no
    /// copy old enough.
    slips: u32,
    resyncs: u32,
    short_rewinds: u32,
    /// Everything the timing loop has moved the clock by, in samples, half
    /// symbols the anchor has moved the symbols by, and copies of the loops
    /// as they were while the signal was being followed.
    timed: f64,
    anchored: i64,
    earlier: VecDeque<Loops>,
    /// Symbols made.
    produced: u64,
    pending: Option<Pending>,
    /// The carrier's turn and the far clock's drift as the last S heard had
    /// them, whether it ended in S-bar or lapsed.
    s_measured: Option<(f64, f64)>,
}

impl Core {
    pub fn new(band: Band, options: Options, slicer: Slicer) -> Self {
        let mut taps = vec![Complex::ZERO; 2 * REACH + 1];
        taps[REACH] = Complex::ONE;
        Self {
            band,
            options,
            front: Front::new(band),
            mode: Mode::Idle,
            heard: VecDeque::new(),
            taps,
            next_symbol: 0,
            slicer,
            rotation: 0.0,
            turn: 0.0,
            slope: 1.0,
            error: 1.0,
            residual: 1.0,
            trained_snr: 0.0,
            last: Complex::ZERO,
            ever_trained: false,
            recent: VecDeque::new(),
            refused: VecDeque::new(),
            settled: 1.0,
            lost: None,
            power_before: 0.0,
            lost_from: 0,
            held: false,
            gain: Agc::unity(),
            slips: 0,
            resyncs: 0,
            short_rewinds: 0,
            timed: 0.0,
            anchored: 0,
            earlier: VecDeque::new(),
            produced: 0,
            pending: None,
            s_measured: None,
        }
    }

    pub fn band(&self) -> Band {
        self.band
    }

    pub fn options(&self) -> Options {
        self.options
    }

    /// Take one sample of the line, or of an echo canceller's residual.
    ///
    /// While tracking, half-symbol samples are made only up to the next
    /// symbol due, so that the timing loop's correction from each symbol
    /// lands before the next sample is placed, as it did in V.34's receiver;
    /// [`Core::next`] makes the rest. A driver that stops taking symbols has
    /// them made anyway once half a second has gone by, and loses them.
    pub fn feed(&mut self, sample: f64) {
        self.front.push(sample);
        loop {
            if self.symbol_waiting() && !self.front.behind() {
                break;
            }
            let Some((half, index)) = self.front.make_half() else { break };
            self.on_half(half, index);
        }
    }

    fn symbol_waiting(&self) -> bool {
        matches!(self.mode, Mode::Tracking)
            && (self.pending.is_some() || self.next_symbol + REACH as u64 + 2 <= self.front.made)
    }

    fn on_half(&mut self, half: Complex, index: u64) {
        match &mut self.mode {
            Mode::Idle | Mode::Tracking => {}
            Mode::Hunting(hunt) => match hunt.feed(half, index) {
                Some(Hunted::S) => self.heard.push_back(Heard::S),
                Some(Hunted::Reversal { at, turn, drift }) => {
                    // S was heard on the grid the timing loop last left, so
                    // what it showed is on top of that.
                    let drift = (1.0 + self.front.drift) * (1.0 + drift) - 1.0;
                    self.s_measured = Some((turn, drift));
                    self.mode = Mode::Idle;
                    self.heard.push_back(Heard::Reversal { at, turn, drift });
                }
                Some(Hunted::Lapsed { at, turn, drift }) => {
                    self.s_measured = Some((turn, (1.0 + self.front.drift) * (1.0 + drift) - 1.0));
                    self.heard.push_back(Heard::Lapsed { at });
                }
                None => {}
            },
            Mode::Training(collecting) => {
                if self.front.made > collecting.needed() {
                    self.finish_training();
                }
            }
            Mode::Blind(_) => self.blind_half(half),
        }
    }

    /// The next thing heard, if there is one.
    pub fn heard(&mut self) -> Option<Heard> {
        self.heard.pop_front()
    }

    /// Stop listening, but go on keeping samples.
    pub fn idle(&mut self) {
        self.mode = Mode::Idle;
        self.pending = None;
    }

    /// Look for S and the change to S-bar.
    pub fn hunt(&mut self) {
        self.mode = Mode::Hunting(Box::new(Hunt::new(self.options.discriminator)));
        self.pending = None;
    }

    /// Go on making symbols with the equaliser as training last left it, the
    /// first centred on half-symbol sample `first`, the carrier turned on by
    /// as many symbols as have gone by (`v34/receiver.rs:548-559`). False if
    /// nothing has trained this receiver.
    pub fn resume(&mut self, first: u64) -> bool {
        if !self.ever_trained {
            return false;
        }
        let gone = first.saturating_sub(self.next_symbol) / 2;
        self.rotation = (self.rotation + self.turn * gone as f64).rem_euclid(std::f64::consts::TAU);
        self.next_symbol = first;
        self.lost = None;
        self.recent.clear();
        self.refused.clear();
        self.pending = None;
        self.mode = Mode::Tracking;
        true
    }

    /// Decide against `slicer` from here, keeping the taps, the carrier, the
    /// timing, the gain and what the errors settle to, and forgetting what the
    /// last slicer made of the recent errors (`v34/receiver.rs:581-594`).
    ///
    /// A loss judged against the old constellation is not a loss against the
    /// new one. Noise at unit power does not depend on the constellation, so
    /// the settled error carries across as it is (core.md 7.5).
    pub fn set_slicer(&mut self, slicer: Slicer) {
        self.slicer = slicer;
        self.lost = None;
        self.recent.clear();
        self.refused.clear();
    }

    pub fn slicer(&self) -> &Slicer {
        &self.slicer
    }

    /// Hold every loop, or let them go again. Symbols go on being made, the
    /// carrier's phase goes on turning, and nothing is judged lost.
    pub fn hold(&mut self, held: bool) {
        self.held = held;
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    pub fn stage(&self) -> Stage {
        match self.mode {
            Mode::Idle => Stage::Idle,
            Mode::Hunting(_) => Stage::Hunting,
            Mode::Training(_) => Stage::Training,
            Mode::Tracking => Stage::Tracking,
            Mode::Blind(_) => Stage::Blind,
        }
    }

    pub fn is_tracking(&self) -> bool {
        matches!(self.mode, Mode::Tracking)
    }

    /// Signal to noise of every symbol against its nearest point, in
    /// decibels.
    pub fn snr_db(&self) -> f64 {
        -10.0 * self.error.max(1e-9).log10()
    }

    /// Signal to noise of the symbols the gate let through.
    pub fn settled_snr_db(&self) -> f64 {
        -10.0 * self.settled.max(1e-9).log10()
    }

    /// What training, or the blind start, left.
    pub fn trained_snr_db(&self) -> f64 {
        self.trained_snr
    }

    /// Mean distance of every symbol from its nearest point, in the
    /// constellation's units, over about a hundred symbols. It goes on
    /// following the symbols while the signal is lost.
    pub fn residual_error(&self) -> f64 {
        self.residual
    }

    /// The last symbol made, equalised, while tracking.
    pub fn last_point(&self) -> Option<Complex> {
        self.is_tracking().then_some(self.last)
    }

    /// The half-symbol samples' mean power, slowly.
    pub fn level(&self) -> f64 {
        self.front.power
    }

    /// The half-symbol samples' mean amplitude over the last 20 ms: what a
    /// carrier detector wants.
    pub fn envelope(&self) -> f64 {
        self.front.envelope()
    }

    /// Slips found and followed.
    pub fn slips(&self) -> u32 {
        self.slips
    }

    /// Resyncs tried, found or not.
    pub fn resyncs(&self) -> u32 {
        self.resyncs
    }

    /// Rewinds that found no copy of the loops old enough.
    pub fn short_rewinds(&self) -> u32 {
        self.short_rewinds
    }

    /// Whether the signal has jumped and not been found again yet.
    pub fn is_lost(&self) -> bool {
        self.lost.is_some()
    }

    /// Symbols since the signal was lost; nought while it is not.
    pub fn lost_for(&self) -> usize {
        self.lost.unwrap_or(0)
    }

    /// The far clock's rate against this end's, as the timing loop has it, in
    /// parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.front.drift * 1e6
    }

    /// The far carrier's offset from ours, as the carrier loop has it.
    pub fn offset_hz(&self) -> f64 {
        self.turn * self.band.baud / std::f64::consts::TAU
    }

    /// The carrier's turn a symbol, in radians, and the far clock's drift as
    /// the timing loop counts it, as the last S heard had them: what a
    /// [`Heard::Reversal`] carries, kept, and kept too for an S that lapsed
    /// without S-bar, whose [`Heard::Lapsed`] says only where it ended.
    pub fn s_measured(&self) -> Option<(f64, f64)> {
        self.s_measured
    }

    /// The carrier phase taken out of the next symbol, in radians.
    pub fn rotation(&self) -> f64 {
        self.rotation
    }

    /// The gain control's gain, in decibels.
    pub fn gain_db(&self) -> f64 {
        20.0 * self.gain.value().log10()
    }

    /// The equaliser's taps, a half symbol apart.
    pub fn taps(&self) -> &[Complex] {
        &self.taps
    }

    /// Where the taps' weight is centred, in half symbols from the middle.
    pub fn tap_centroid(&self) -> f64 {
        centroid(&self.taps)
    }

    /// Half-symbol samples made so far.
    pub fn halves(&self) -> u64 {
        self.front.made
    }

    /// Symbols made so far.
    pub fn symbols(&self) -> u64 {
        self.produced
    }

    /// The half-symbol sample the next symbol is centred on.
    pub fn next_half(&self) -> u64 {
        self.next_symbol
    }
}

fn apply(taps: &[Complex], row: &[Complex]) -> Complex {
    taps.iter().zip(row).fold(Complex::ZERO, |sum, (w, x)| sum + *w * *x)
}

/// Where taps' weight is centred, in half symbols from the middle.
fn centroid(taps: &[Complex]) -> f64 {
    let (moment, energy) =
        taps.iter().enumerate().fold((0.0, 0.0), |(m, e), (i, w)| (m + i as f64 * w.norm_sqr(), e + w.norm_sqr()));
    moment / energy.max(1e-30) - (taps.len() / 2) as f64
}

/// How far apart two angles are, the short way round
/// (`v34/receiver.rs:1266-1269`).
fn angle_between(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    d.min(std::f64::consts::TAU - d)
}
