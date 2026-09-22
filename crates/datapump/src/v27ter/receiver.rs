//! The V.27 ter receiver, on the shared QAM core.
//!
//! Everything that brings the far end's symbols back -- the fixed mixer, the
//! stored samples, the equaliser at two samples a symbol solved outright on
//! the known turn-on sequence, the three loops and their one gate, losing the
//! signal and finding it again -- is `dsp::qam`, the V.34 receiver that V.32
//! was rebuilt on. What is left here is what is V.27 ter's own: finding the
//! turn-on sequence ([`super::hunt`]), which of its two lengths it is, what
//! its symbols are, when a burst has ended, and how bits come out of a phase
//! change.
//!
//! The receiver this replaced trusted a single measurement of the carrier
//! over 48 symbols after a level threshold fired, then taught a 31-tap
//! equaliser from nothing by a blind criterion that could not hand over to
//! decisions in less than 139 symbols -- longer than the whole short turn-on
//! (fax-qam.md 2.5, 2.7). It lost every page on a line with hiss on it
//! (3.1), and every short turn-on through an echo (3.5).
//!
//! So a burst is trained where the Recommendation puts the means to: segment
//! 3's reversals say where the symbols are and how fast the carrier turns,
//! the start of segment 4's pattern says which symbol is which, and segments
//! 3 to 5 are known symbol for symbol before they arrive (2.5.1, Table 4), so
//! the equaliser, the gain, the carrier's phase and its turn are solved for
//! by least squares on them. The long turn-on is solved on 256 of its
//! symbols. The short one's 72 are too few to solve 31 taps from with
//! anything to spare, and it is sent to a receiver that has heard a long one
//! already (2.5.1), so it keeps the taps the long one left and finds only
//! where the symbols fall and how the carrier is turned, by the core's resync
//! search -- unless there are no such taps, when it solves on what it has.
//!
//! Which of the two is arriving cannot be told until segment 4 has gone on
//! longer than the short one's, by which time a short one's data has begun.
//! Both begin the same way for their last fourteen reversals and first 58
//! symbols of conditioning (Table 4: "beginning and ending PRS and symbol
//! sequences are the same for both lengths"), so every burst is first trained
//! on that, in time for a short one's data, and a long one is trained again
//! once it is plainly long, on a window of its own.
//!
//! And a burst may be half over before anyone listens. The fax layer turns
//! the line round to listen when its procedure says to, and a page's burst
//! has been measured arriving 75 ms before that, its reversals gone. Segment
//! 4 is known from anywhere in it, though, so the hunt finds it without its
//! start, the long turn-on is trained on the rest of it, and a short one is
//! found with the taps a long one left.

use std::collections::VecDeque;
use std::sync::OnceLock;

use dsp::Complex;
use dsp::qam::{Band, Constellation, Core, Heard, Options, Slicer, Training, Via, Window};

use super::hunt::{Found, Hunt};
use super::{CARRIER, Rate, Scrambler, TURN_DIBIT, TURN_TRIBIT, Training as TurnOn, Transmitter};

/// The quietest level that may be called a carrier, and the level it has to
/// fall below to be called gone, in the unit-gain mixer's terms: sixty
/// decibels under full scale with five of hysteresis, as every other carrier
/// detector in this modem has them. Only floors: what decides that a burst
/// has begun is its reversals, and what decides it has ended is its own
/// level falling away from where it was.
const CARRIER_ON: f64 = 1.0e-3;
const CARRIER_OFF: f64 = 5.62e-4;

/// How far a burst has to fall below its own loudest to be called gone:
/// twelve decibels, on any line at any level. On a line whose hiss is within
/// twelve decibels of the burst the hiss alone is above that, so the level
/// has also to fall to the geometric mean of the burst and the quiet line as
/// it was before the burst -- though never below half the burst, in case
/// what was taken for the quiet line was not.
const OFF_BELOW_LOUDEST: f64 = 0.25;
const OFF_BELOW_LOUDEST_AT_MOST: f64 = 0.5;

/// How far above the level it went off at a burst has to come back to be on
/// again: three decibels.
const BACK_ON: f64 = 1.41;

/// How fast the loudest-so-far is forgotten: about two seconds, so that a
/// burst that fades over a long page is followed rather than cut off.
const LOUDEST_SECONDS: f64 = 2.0;

/// How fast the quiet line's level is followed, down and up, while there is
/// no burst. It is only ever measured then, and a burst is only ever begun by
/// its reversals, so whatever is on the line between bursts -- hiss, V.21, an
/// unmodulated carrier -- is what it measures, and nothing it measures can
/// start a burst.
const FLOOR_FALL_SECONDS: f64 = 0.1;
const FLOOR_RISE_SECONDS: f64 = 0.5;

/// How long the level has to have been down for a burst to be over. A fifth
/// of a second is what the fax layer bridges (`fax::call`,
/// `FAST_CARRIER_GONE`), and a jitter buffer's 20 ms hole in the middle of a
/// page is far inside it: the burst goes on, and the core finds the signal
/// again when it comes back.
const ENDED_SECONDS: f64 = 0.25;

/// Reversals of the long turn-on ahead of the fourteen both lengths end with.
const LONG_AHEAD: usize = 50 - 14;

/// The first training, on what both turn-ons have in common: fourteen
/// reversals and 58 symbols of conditioning, symbols 0 to 71 of the short
/// one. Aligned and solved over the last ten reversals and all of segment 4,
/// whose pattern is what pins the alignment: reversals alone fit as well a
/// symbol either way. It is done by symbol 76, inside the short turn-on's
/// segment 5, and the core tracks from symbol 72, so a short burst's data,
/// from symbol 80, is not missed.
const COMMON: Window = Window { align: (4, 72), solve: (4, 72), search: 8 };

/// The long turn-on's own training, counted from the same symbol: 256
/// symbols of segment 4, solved by symbol 280 with 800 of conditioning still
/// to come for the loops to settle on; and further in, searched a hundred
/// symbols either way, for when a jitter buffer's slip spoils the first.
const LONG: Window = Window { align: (20, 276), solve: (20, 276), search: 8 };
const LONG_RETRY: Window = Window { align: (420, 676), solve: (420, 676), search: 200 };

/// The training for a burst whose conditioning was found without its start,
/// counted from the newest symbol it was found by: 256 symbols from eight
/// after it, and the retry further in, all well inside the long turn-on's
/// conditioning for a start found up to 400 symbols into it.
const LATE: Window = Window { align: (8, 264), solve: (8, 264), search: 8 };
const LATE_RETRY: Window = Window { align: (300, 556), solve: (300, 556), search: 200 };

/// Segment 4's period, in symbols, and the symbols of it that a burst found
/// without its start was found by (`hunt.rs`).
const PERIOD: usize = 127;
const LATE_BY: usize = 32;

/// Symbols from the first of those that a burst found without its start is
/// looked for with the taps a long one left: the core searches the newest 48
/// symbols but ten, and until 64 are in, some of those may be from before the
/// receiver was listening at all.
const LATE_LOOK: usize = 64;

/// Signal to noise, in decibels, below which a training's fit is taken to be
/// the wrong alignment (`v34/receiver.rs:90`).
const ACCEPT_DB: f64 = 12.0;

/// Line samples a half-symbol sample is made after the one it is centred on:
/// the core's interpolating filter reaches 32 samples ahead, and a half is
/// made on the first sample that lets it.
const HALF_LAG: f64 = 31.5;

/// The eight phases and the four, each a table at unit power labelled by its
/// phase: in eighths of a turn for the eight, quarters for the four.
struct Tables {
    eight: Slicer,
    four: Slicer,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        // Written out rather than turned, so that every point's fourth power
        // is exactly real: the core's resync reads the phase from the fourth
        // power, which for eight phases is plus or minus one, and gives it to
        // within an eighth of a turn only if the table's own comes out on the
        // axis.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let eight = [(1.0, 0.0), (h, h), (0.0, 1.0), (-h, h), (-1.0, 0.0), (-h, -h), (0.0, -1.0), (h, -h)];
        let four = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
        let table = |points: &[(f64, f64)]| Slicer::table(Constellation::new(points.iter().map(|&p| p.into()).collect()));
        Tables { eight: table(&eight), four: table(&four) }
    })
}

fn slicer_for(rate: Rate) -> Slicer {
    match rate {
        Rate::R4800 => tables().eight.clone(),
        Rate::R2400 => tables().four.clone(),
    }
}

/// Segments 3 to 5 at a rate, symbol by symbol, from the transmitter itself:
/// the short turn-on whole, and the long one from its last fourteen
/// reversals, so that both start on the same symbol.
struct Sequences {
    short: Vec<Complex>,
    long: Vec<Complex>,
}

fn sequences(rate: Rate) -> &'static Sequences {
    static AT: OnceLock<[Sequences; 2]> = OnceLock::new();
    let both = AT.get_or_init(|| {
        [Rate::R4800, Rate::R2400].map(|rate| {
            let of = |turn_on: TurnOn| -> Vec<Complex> {
                let mut tx = Transmitter::new(16_000.0);
                tx.start(rate, turn_on);
                (0..turn_on.symbols()).map(|_| tx.next_symbol().into()).collect()
            };
            Sequences { short: of(TurnOn::Short), long: of(TurnOn::Long).split_off(LONG_AHEAD) }
        })
    });
    &both[usize::from(rate == Rate::R2400)]
}

/// Segment 4 from symbol `phase` of its period, `count` symbols of it.
///
/// Its changes come round every 127 symbols, but its phases come round
/// turned half a turn, a period holding 63 reversals: which of the two the
/// first is does not matter to a solve for the carrier's phase, only that
/// the rest follow it.
fn late_targets(rate: Rate, phase: usize, count: usize) -> Vec<Complex> {
    let conditioning = &sequences(rate).long[14..14 + PERIOD];
    (0..count)
        .map(|k| {
            let z = conditioning[(phase + k) % PERIOD];
            if (phase % PERIOD + k) / PERIOD % 2 == 1 { -z } else { z }
        })
        .collect()
}

/// Where the receiver is with the burst on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Burst {
    /// Nothing arriving.
    Quiet,
    /// Reversals, and nothing yet to say they are a turn-on sequence.
    Reversals,
    /// A turn-on sequence found and being trained on.
    Training,
    /// Symbols being made and read.
    Tracking,
    /// A turn-on sequence that nothing fitted.
    Failed,
}

/// V.27 ter receiver.
///
/// It never decides where the training ended, as the one before it did not:
/// nothing in the turn-on sequence marks its own last symbol that survives a
/// slip, and T.30 does not need it to. The training check is a run of zeros
/// and is recognised as one; a page begins with an end-of-line code and is
/// found by looking for it. So this hands up descrambled bits from the first
/// symbol it tracks, and whoever is above finds its own place in them.
#[derive(Debug)]
pub struct Receiver {
    fs: f64,
    rate: Rate,
    core: Core,
    hunt: Hunt,
    /// Line samples taken, counted as the core counts them.
    taken: u64,
    /// The line sample each recent half-symbol sample of the core's is
    /// centred on, oldest first, and how many halves that accounts for.
    halves: VecDeque<(u64, f64)>,
    seen: u64,
    burst: Burst,
    /// The half-symbol sample the burst's turn-on is trained from.
    start: Option<u64>,
    /// A training held back until its window is in, and the half by which it
    /// is.
    pending: Option<(Training, u64)>,
    /// Whether a long turn-on has trained this receiver at this rate, so that
    /// a short one may keep its taps.
    taught: bool,
    /// Whether the training in hand is the long turn-on's own.
    long: bool,
    /// Where in segment 4's period the burst's conditioning was found, when
    /// it was found without its start.
    late: Option<usize>,
    via: Option<Via>,
    /// The phase the previous symbol landed on, in the table's labels.
    previous: Option<u8>,
    slips: u32,
    descrambler: Scrambler,
    bits: Vec<bool>,
    last: Complex,
    carrier: bool,
    /// What the line sounds like with no burst on it, the loudest the burst
    /// in hand has been, the level it went off at, and samples since.
    floor: f64,
    loudest: f64,
    went_off: f64,
    off_for: u64,
}

impl Receiver {
    pub fn new(fs: f64) -> Self {
        let rate = Rate::default();
        Self {
            fs,
            rate,
            core: core_for(fs, rate),
            hunt: Hunt::new(fs, rate.baud()),
            taken: 0,
            halves: VecDeque::new(),
            seen: 0,
            burst: Burst::Quiet,
            start: None,
            pending: None,
            taught: false,
            long: false,
            late: None,
            via: None,
            previous: None,
            slips: 0,
            descrambler: Scrambler::new(),
            bits: Vec::new(),
            last: Complex::ZERO,
            carrier: false,
            floor: 0.0,
            loudest: 0.0,
            went_off: 0.0,
            off_for: 0,
        }
    }

    /// Set the rate the burst about to arrive is at.
    ///
    /// A fax receiver always knows this in advance: the DCS frame that came
    /// over V.21 named it, and the high-speed carrier that follows is at that
    /// rate and no other. Nothing here has to guess. A new rate is a new
    /// symbol rate, so everything learned at the old one goes.
    pub fn set_rate(&mut self, rate: Rate) {
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        self.core = core_for(self.fs, rate);
        self.hunt = Hunt::new(self.fs, rate.baud());
        self.taken = 0;
        self.halves.clear();
        self.seen = 0;
        self.taught = false;
        self.restart();
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Whether the far end's carrier is on the line: from a burst's reversals
    /// until its level falls away, and not for anything else.
    pub fn carrier(&self) -> bool {
        self.carrier
    }

    /// The in-band amplitude, over 10 ms.
    pub fn level(&self) -> f64 {
        self.hunt.level()
    }

    /// Where the last symbol landed, for a constellation display, at the
    /// constellation's own unit power.
    pub fn constellation_point(&self) -> (f64, f64) {
        self.last.into()
    }

    /// Mean distance of the symbols from the nearest phase, in the units the
    /// constellation is drawn in, over about a hundred symbols.
    pub fn residual_error(&self) -> f64 {
        self.core.residual_error()
    }

    /// How far apart two neighbouring points are.
    ///
    /// Every point sits on the unit circle, so the gap between neighbours is
    /// the chord: twice the sine of half the angle between them. Three
    /// quarters of a unit at 4800 and nearly one and a half at 2400, which is
    /// most of why the slower rate carries a page down a worse line.
    pub fn point_spacing(&self) -> f64 {
        let phases = f64::from(self.rate.phases());
        2.0 * (std::f64::consts::PI / phases).sin()
    }

    /// Forget the burst just gone and be ready for the next one.
    ///
    /// Everything that belongs to one burst goes -- the differential
    /// reference, the descrambler, any bits not yet taken, the carrier flag,
    /// and what the hunt had heard, since what is on the line at the moment
    /// somebody starts listening for a burst is the tail of the last one. The
    /// taps a long turn-on left stay: a short one after it is sent to be
    /// heard with them (2.5.1).
    pub fn restart(&mut self) {
        self.core.idle();
        self.hunt.reset();
        self.burst = Burst::Quiet;
        self.start = None;
        self.pending = None;
        self.long = false;
        self.late = None;
        self.previous = None;
        self.descrambler.reset();
        self.bits.clear();
        self.carrier = false;
        self.loudest = 0.0;
        self.off_for = 0;
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    pub fn feed(&mut self, sample: f64) {
        let index = self.taken;
        self.taken += 1;
        self.core.feed(sample);
        if let Some(found) = self.hunt.feed(sample, index) {
            self.on_found(found);
        }
        self.follow_level();
        if self.pending.as_ref().is_some_and(|(_, due)| self.core.halves() >= *due)
            && let Some((training, _)) = self.pending.take()
        {
            self.core.train(training);
        }
        self.listen();
        while let Some(point) = self.core.next() {
            let Some(symbol) = self.core.settle(point.nearest) else { break };
            self.last = symbol.point;
            if self.core.slips() != self.slips {
                // Found again after a jump: the phase is found to within a
                // phase of the table, so the symbol before is no reference.
                self.slips = self.core.slips();
                self.previous = None;
            }
            if self.burst == Burst::Tracking {
                self.read(symbol.label.unwrap_or(0));
            }
        }
        self.note_halves(index);
    }

    /// Keep where the core's newest halves are centred, so that a place on
    /// the line the hunt found can be named as a half.
    fn note_halves(&mut self, index: u64) {
        let made = self.core.halves();
        if made < self.seen {
            // A resync read the newest again on a moved grid and has yet to
            // make some of them.
            while self.halves.back().is_some_and(|&(h, _)| h >= made) {
                self.halves.pop_back();
            }
            self.seen = made;
        }
        for h in self.seen..made {
            self.halves.push_back((h, index as f64 - HALF_LAG));
        }
        self.seen = made;
        while self.halves.len() > 8192 {
            self.halves.pop_front();
        }
    }

    /// The half-symbol sample centred nearest line sample `at`, if the core
    /// has one within a half symbol of it, or will soon. At 1600 baud the
    /// hunt's matched filter reaches 30 samples past a symbol and the core's
    /// interpolator 32, so the symbol the hunt names may be a half ahead of
    /// the core's newest; the halves are evenly spaced while the core is not
    /// tracking, which is when a burst is found.
    fn half_at(&self, at: f64) -> Option<u64> {
        let spacing = self.fs / self.rate.baud() / 2.0;
        if let Some(&(h, t)) = self.halves.back()
            && at > t
        {
            return (at - t < 4.0 * spacing).then(|| h + ((at - t) / spacing).round() as u64);
        }
        let after = self.halves.partition_point(|&(_, t)| t < at);
        let near = |i: usize| self.halves.get(i).map(|&(h, t)| ((t - at).abs(), h));
        let nearest = match (after.checked_sub(1).and_then(near), near(after)) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (a, b) => a.or(b),
        }?;
        (nearest.0 <= spacing).then_some(nearest.1)
    }

    fn on_found(&mut self, found: Found) {
        match found {
            Found::Reversals => {
                if matches!(self.burst, Burst::Quiet | Burst::Failed) {
                    self.burst = Burst::Reversals;
                    self.begin();
                }
            }
            Found::Conditioning { start, turn } => {
                // A turn-on sequence, whatever was going on before: a burst
                // that ended unnoticed, or reversals that were not one.
                let Some(start) = self.half_at(start) else { return };
                if self.burst != Burst::Reversals {
                    self.begin();
                }
                self.core.idle();
                self.burst = Burst::Training;
                self.start = Some(start);
                self.long = false;
                self.late = None;
                self.previous = None;
                let training = Training {
                    targets: sequences(self.rate).short.clone(),
                    start,
                    first: COMMON,
                    retry: None,
                    turn: Some(turn),
                    drift: None,
                    // Where a long turn-on has left taps, a short one keeps
                    // them: the core looks for the signal with them before
                    // taking what 68 rows solve to.
                    accept_db: if self.taught { f64::INFINITY } else { ACCEPT_DB },
                    slicer: slicer_for(self.rate),
                    fallback: true,
                };
                self.pending = Some((training, due(start, COMMON)));
            }
            Found::Late { at, phase, first, turn } => {
                // Only for a burst not already found: the conditioning of one
                // that was goes on being segment 4 for a thousand symbols.
                if !matches!(self.burst, Burst::Quiet | Burst::Reversals | Burst::Failed) {
                    return;
                }
                let Some(start) = self.half_at(at) else { return };
                if self.burst != Burst::Reversals {
                    self.begin();
                }
                self.core.idle();
                self.burst = Burst::Training;
                self.start = Some(start);
                self.long = false;
                self.late = Some(phase);
                self.previous = None;
                // Trained on the conditioning once the hunt has measured the
                // carrier's turn over more of it than it was found by, if it
                // goes on for long enough to be the long turn-on's. It may be
                // a short one's, whose data is close behind, so where a long
                // one has left taps they look for it as soon as they can, as
                // they do for any short one: the core's search with them on
                // the newest symbols, the solve on the few before being
                // refused whatever it comes to.
                if self.taught
                    && let Some(from) = self.half_at(first)
                {
                    let window = Window { align: (0, LATE_LOOK), solve: (0, LATE_LOOK), search: 0 };
                    let training = Training {
                        targets: late_targets(self.rate, phase + PERIOD - (LATE_BY - 1), LATE_LOOK),
                        start: from,
                        first: window,
                        retry: None,
                        turn: Some(turn),
                        drift: None,
                        accept_db: f64::INFINITY,
                        slicer: slicer_for(self.rate),
                        fallback: true,
                    };
                    self.pending = Some((training, due(from, window)));
                }
            }
            Found::Length { long, turn } => {
                if !matches!(self.burst, Burst::Training | Burst::Tracking | Burst::Failed) {
                    return;
                }
                let Some(start) = self.start else { return };
                if !long {
                    // The short turn-on: its training is the one in hand. And
                    // found without its start, nothing but the taps a long one
                    // left could have found it.
                    if self.late.is_some() && self.burst != Burst::Tracking {
                        self.burst = Burst::Failed;
                    }
                    return;
                }
                let (targets, first, retry) = match self.late {
                    None => (sequences(self.rate).long.clone(), LONG, LONG_RETRY),
                    Some(phase) => (late_targets(self.rate, phase, LATE_RETRY.solve.1), LATE, LATE_RETRY),
                };
                self.long = true;
                let training = Training {
                    targets,
                    start,
                    first,
                    retry: Some(retry),
                    turn: Some(turn),
                    drift: None,
                    accept_db: ACCEPT_DB,
                    slicer: slicer_for(self.rate),
                    fallback: true,
                };
                self.pending = Some((training, due(start, first)));
            }
            Found::Nothing => {
                if self.burst == Burst::Reversals {
                    self.burst = Burst::Quiet;
                    self.carrier = false;
                }
            }
        }
    }

    /// A burst's front: the carrier flag up, the level it is judged by from
    /// here.
    fn begin(&mut self) {
        self.carrier = true;
        self.loudest = self.hunt.level();
        self.off_for = 0;
    }

    /// The carrier flag, and the end of a burst.
    fn follow_level(&mut self) {
        let level = self.hunt.level();
        if self.burst == Burst::Quiet {
            let seconds = if level < self.floor { FLOOR_FALL_SECONDS } else { FLOOR_RISE_SECONDS };
            self.floor += (level - self.floor) / (seconds * self.fs);
            return;
        }
        self.loudest = self.loudest.max(level) * (1.0 - 1.0 / (LOUDEST_SECONDS * self.fs));
        let off = (OFF_BELOW_LOUDEST * self.loudest)
            .max((self.floor * self.loudest).sqrt().min(OFF_BELOW_LOUDEST_AT_MOST * self.loudest))
            .max(CARRIER_OFF);
        if self.carrier {
            if level < off {
                self.carrier = false;
                self.went_off = off;
            }
        } else if level > (BACK_ON * self.went_off).min(0.8 * self.loudest).max(CARRIER_ON) {
            self.carrier = true;
        }
        self.off_for = if self.carrier { 0 } else { self.off_for + 1 };
        if self.off_for as f64 > ENDED_SECONDS * self.fs {
            // Over. The line from here is the quiet line again.
            self.core.idle();
            self.burst = Burst::Quiet;
            self.pending = None;
            self.start = None;
            self.floor = level;
        }
    }

    /// Act on what the core has heard.
    fn listen(&mut self) {
        while let Some(heard) = self.core.heard() {
            match heard {
                Heard::Trained { via, .. } => {
                    self.burst = Burst::Tracking;
                    self.via = Some(via);
                    self.previous = None;
                    self.slips = self.core.slips();
                    if self.long && via != Via::Fallback {
                        self.taught = true;
                    }
                }
                // Nothing fitted. The burst is lost unless a training still
                // to come fits -- the long turn-on's own, once the hunt says
                // it is one -- and the hunt goes on listening for the next.
                Heard::Untrained if self.pending.is_none() => self.burst = Burst::Failed,
                _ => {}
            }
        }
    }

    /// One symbol's phase change, as bits.
    fn read(&mut self, label: usize) {
        let (phases, count) = match self.rate {
            Rate::R4800 => (8, 3),
            Rate::R2400 => (4, 2),
        };
        let now = label as u8;
        let Some(previous) = self.previous.replace(now) else {
            // The first symbol is only a reference; a difference needs two.
            return;
        };
        if !self.carrier {
            return;
        }
        let change = usize::from((now + phases - previous) % phases);
        let group = match self.rate {
            Rate::R4800 => TURN_TRIBIT[change],
            Rate::R2400 => TURN_DIBIT[change],
        };
        for i in (0..count).rev() {
            let out = self.descrambler.descramble(group >> i & 1 != 0);
            self.bits.push(out);
        }
    }

    /// What the last training, or the resync a short turn-on is found by,
    /// came to, in decibels.
    pub fn trained_snr_db(&self) -> f64 {
        self.core.trained_snr_db()
    }

    /// Signal to noise of every symbol against its nearest phase, in
    /// decibels.
    pub fn snr_db(&self) -> f64 {
        self.core.snr_db()
    }

    /// How this receiver last came to be tracking: trained on the turn-on
    /// sequence, or found with the taps a long one left.
    pub fn trained_via(&self) -> Option<Via> {
        self.via
    }

    /// Whether symbols are being made and read.
    pub fn is_tracking(&self) -> bool {
        self.burst == Burst::Tracking && self.core.is_tracking()
    }

    /// Slips found and followed: a jitter buffer's 20 ms, a hole, a jump.
    pub fn slips(&self) -> u32 {
        self.core.slips()
    }

    /// The far carrier's offset from 1800 Hz, as the carrier loop has it.
    pub fn offset_hz(&self) -> f64 {
        self.core.offset_hz()
    }

    /// The far clock's rate against this end's, in parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.core.drift_ppm()
    }
}

/// The core for a rate: its symbol rate on V.27 ter's 1800 Hz.
fn core_for(fs: f64, rate: Rate) -> Core {
    Core::new(Band::new(fs, rate.baud(), CARRIER), Options::fixed(), slicer_for(rate))
}

/// The half-symbol sample by which a training from `start` has its window
/// in, but for the equaliser's reach beyond its last symbol. The core waits
/// for that much more itself, so handing the training over a little early
/// means it is solved on the very half that completes the window, and a short
/// turn-on's data, which follows closely, loses nothing to lateness.
fn due(start: u64, window: Window) -> u64 {
    let end = window.solve.1.max(window.align.1) as u64;
    start + window.search.max(0) as u64 + 2 * end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_turn_ons_begin_the_same_way_for_72_symbols() {
        // What the first training of every burst rests on: fourteen
        // reversals and all 58 of the short one's conditioning are the long
        // one's too, phase for phase (Table 4's note).
        for rate in [Rate::R4800, Rate::R2400] {
            let Sequences { short, long } = sequences(rate);
            assert_eq!(short.len(), 80);
            assert_eq!(long.len(), 50 + 1074 + 8 - LONG_AHEAD);
            assert_eq!(short[..72], long[..72], "{rate:?}");
            // Reversals first, then segment 4 starting with no change.
            for k in 1..14 {
                assert!((short[k] + short[k - 1]).abs() < 1e-12, "{rate:?} symbol {k} is not a reversal");
            }
            assert!((short[14] - short[13]).abs() < 1e-12, "{rate:?} segment 4 does not start with 0 degrees");
            assert_eq!(short[COMMON.solve.1 - 1], long[COMMON.solve.1 - 1]);
        }
    }

    #[test]
    fn segment_4_found_anywhere_is_segment_4_to_its_end() {
        // Found without its start, the conditioning is made from one period
        // of it, each period after turned half a turn: 63 reversals to the
        // period. Every one of its 1074 symbols, from wherever it is taken.
        for rate in [Rate::R4800, Rate::R2400] {
            let conditioning = &sequences(rate).long[14..14 + 1074];
            for from in [0, 1, 58, 126, 127, 300, 700] {
                let made = late_targets(rate, from, 1074 - from);
                let sign = made[0] * conditioning[from].conj();
                assert!((sign.abs() - 1.0).abs() < 1e-12 && sign.im.abs() < 1e-12, "{rate:?} from {from}");
                for (k, z) in made.iter().enumerate() {
                    assert!((*z - conditioning[from + k] * sign).abs() < 1e-12, "{rate:?} from {from}, symbol {k}");
                }
            }
        }
    }

    #[test]
    fn each_phase_is_its_own_label() {
        // Labels are phases -- eighths at 4800, quarters at 2400 -- which is
        // what the phase change is read from.
        for (rate, phases) in [(Rate::R4800, 8), (Rate::R2400, 4)] {
            let Slicer::Table(table) = slicer_for(rate) else { unreachable!() };
            assert_eq!(table.len(), phases);
            assert!((table.power() - 1.0).abs() < 1e-12);
            for k in 0..phases {
                let z = Complex::from_polar(1.0, std::f64::consts::TAU * k as f64 / phases as f64);
                assert_eq!(table.nearest(z), k, "{rate:?}");
                assert_eq!(table.nearest(z * Complex::from_polar(1.0, 0.3 * std::f64::consts::PI / phases as f64)), k);
            }
        }
    }
}
