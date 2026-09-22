//! The V.17 receiver, on the shared QAM core.
//!
//! Everything that brings the far end's symbols back -- the fixed mixer, the
//! stored samples, the equaliser at two samples a symbol, the three loops and
//! their one gate, losing the signal and finding it again -- is `dsp::qam`, as
//! it is for V.32's receiver, whose driver this follows. What is left here is
//! V.17's own: which of Table 3's trainings a burst begins with, what segment
//! 2 is, where segment 4 begins, how bits come out of the points, and when to
//! say there is a carrier.
//!
//! A burst is found the way V.32's S and S-bar are. Segment 1 is A and B in
//! turn, two points a quarter turn apart, and segment 2 opens C D C D, the
//! same two turned half a revolution (Table 4): to the core's hunt that is S
//! turning into S-bar, which says how fast the carrier turns and where
//! segment 2 begins. Then one of two things.
//!
//! - A long train's segment 2 is 2976 symbols known before they arrive, and
//!   the equaliser, the gain and the carrier's phase and turn are solved for
//!   outright by least squares on it, as V.32's are on TRN.
//! - A resync's is 38, far too few for 31 taps, and 5.1 makes it for a
//!   receiver that has trained already: the line is the one the last long
//!   train taught, and only where the symbols fall and how the carrier is
//!   turned are new. So the taps are kept, and those two are found by the
//!   core's resync search on the tail of segment 1 and the start of segment
//!   2, which are all on the four states.
//!
//! A receiver cannot know which is coming. T.30's Note 5 to 5.1 has the long
//! train in front of a training check and after CTC/CTR and the resync
//! everywhere else, but a receiver told what to expect is one that fails on
//! the first far end that does otherwise. So once there are taps to keep,
//! every burst is first taken for a resync. The dozen symbols the search
//! hands on are checked against what segment 2 says they are, which proves
//! the reversal was the real one and says exactly where the symbols are;
//! then the symbols after segment 2 of a resync would end say which it is:
//! points of the channel's constellation mean segment 4 has begun and it was
//! a resync, and more of the four states mean segment 2 goes on and it is a
//! long train, which is then solved for as any other.

use std::f64::consts::TAU;
use std::sync::OnceLock;

use dsp::Complex;
use dsp::qam::{Band, Constellation, Core, Heard, Options, Point, Slicer, Training as Solve, Via, Window};

use super::super::v32::trellis::Decoder;
use super::super::v32::{CONSTELLATION_RMS, Scrambler};
use super::{BAUD, CARRIER, Rate, State, Training, scrambler, segment_two};

/// The least level at which the far carrier is taken to be present, and the
/// least at which it is still there: V.32's receiver's, on the same core's
/// envelope, with five decibels between them where 3.7 asks for at least two.
const CARRIER_ON: f64 = 2.0e-3;
const CARRIER_OFF: f64 = 1.124e-3;

/// Below the burst's own level, as a share of its amplitude, the level has
/// to fall for the burst to be going, and the share it has to rise back
/// above to be there again: twelve decibels down, six decibels between.
///
/// Measured against the burst rather than against anything fixed, as V.29's
/// is, for two reasons. On a line with hiss the fixed levels are below the
/// hiss and would never be crossed (fax-qam.md 3.1). And the core's envelope
/// is a one-pole average with a 20 ms time constant, which takes 136 ms to
/// fall from a burst to the fixed level but 28 ms to fall twelve decibels:
/// with the filters in front and [`CARRIER_HOLD`] after, circuit 109 goes
/// off 30 to 50 ms after the signal, as 3.6 asks. Twelve decibels is also
/// under the hiss of every rate's working signal to noise, 18 dB at worst,
/// and more than a 20 ms concealment's silence takes the envelope down,
/// which is one time constant, nine decibels.
const OFF_BELOW: f64 = 0.25;
const ON_ABOVE: f64 = 0.5;

/// How fast the burst's level is forgotten, a share a sample: a time
/// constant of two seconds, V.29's.
const LOUDEST_DECAY: f64 = 3.1e-5;

/// How long the level has to have been down before circuit 109 goes off, in
/// seconds: enough for a hole a little longer than a concealment's to pass.
const CARRIER_HOLD: f64 = 0.005;

/// How long it has to have been down before the burst is given up and the
/// next one listened for. Longer than circuit 109's, so that a hole in the
/// middle of a page, which a jitter buffer can leave, costs the symbols in
/// it and not the rest of the page: the core, still tracking, finds the
/// signal again when it comes back. And under the fifth of a second the fax
/// call waits after the carrier goes before it takes a burst to be over.
const BURST_GONE: f64 = 0.150;

/// Symbols decided as the nearest point, whatever the coding, after anything
/// that leaves the trellis decoder's paths meaningless for a while: the
/// start of segment 4, or a slip found again (V.32's `FRESH`).
const FRESH: u32 = 16;

/// Symbols of segment 2, counted from its first, that a long train's
/// equaliser is solved over: V.32's two tries on TRN, both well inside the
/// 2976 symbols (`v32/receiver.rs`).
const FIRST: Window = Window { align: (16, 256), solve: (16, 512), search: 8 };
const RETRY: Window = Window { align: (640, 1152), solve: (640, 1152), search: 200 };

/// The one try made when segment 1 ends without turning into segment 2,
/// which a slip across the join does: the join is taken to be LAPSED_AHEAD
/// half symbols after segment 1 ended and searched a long way either side,
/// on a window well inside segment 2 whatever the slip did (V.32's LAPSED).
const LAPSED: Window = Window { align: (272, 528), solve: (272, 656), search: 330 };
const LAPSED_AHEAD: u64 = 48;

/// Signal to noise, in decibels, below which a fit is taken to be the wrong
/// alignment and the next try is made (`v34/receiver.rs:90`).
const ACCEPT_DB: f64 = 12.0;

/// Symbols of segment 2 in by which a resync's search runs: its window, 48
/// symbols ending five short of the next symbol, then ends at segment 2's
/// symbol 19 and reaches back 29 symbols into segment 1, every one of them
/// on the four states whatever the training. The symbols after it are made
/// from here.
const RESYNC_AT: u64 = 24;

/// The symbol by which the ones from [`RESYNC_AT`] have been checked against
/// what segment 2 says they are, and how many of them may be wrong.
///
/// The check is what makes a resync trustworthy, because nothing else about
/// it is. A concealment's comfort noise in the middle of segment 1 can read
/// to the hunt as the reversal into segment 2, and the search, run so soon
/// after it, then finds segment 1 itself, which is on the four states too.
/// Its symbols are A and B in turn, and segment 2's are scrambled: a dozen
/// of them agree with segment 2 turned any of the four ways, and started a
/// symbol or two either side, only where they are segment 2. The one that
/// agrees also says exactly which symbol the next one is, where the search
/// could only say to within one.
const CHECKED: u64 = 36;
const CHECK_MISSES: usize = 2;

/// Where the symbols that tell a resync from a long train are kept from, in
/// symbols of segment 2, where they are judged from, and where the judging
/// is done. They are segment 4's first ten if this is a resync; the first
/// two are left out of the judging, the pulse of the last of segment 2
/// being still in them.
const PROBE: (u64, u64, u64) = (38, 40, 48);

/// Mean squared distance from the four states, at unit power, under which
/// the symbols after a resync's segment 2 are more of segment 2: a
/// sixteenth of the states' closest spacing squared. The channel's own
/// constellations read at 0.3 or more against them -- even 7200's, which has
/// the four states among its sixteen and twelve others each 0.4 from one --
/// and the four states themselves at the line's noise.
const LONG_BELOW: f64 = 0.125;

/// The constellations at unit power, as tables for the core.
#[derive(Debug)]
struct Tables {
    /// A to D, labelled by the state's order.
    four: Slicer,
    /// The trellis codings, by code, in the order of the bits a symbol
    /// carries: 7200, 9600, 12 000, 14 400.
    coded: [Slicer; 4],
    /// Segment 2 of a long train, as states and at unit power.
    states: Vec<State>,
    known: Vec<Complex>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let unit = |(x, y): (f64, f64)| Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS);
        let four = Slicer::table(Constellation::new(State::ALL.iter().map(|s| s.unit()).collect()));
        let coded = [Rate::R7200, Rate::R9600, Rate::R12000, Rate::R14400].map(|rate| {
            let coded = rate.coded();
            Slicer::table(Constellation::new((0..coded.size()).map(|code| unit(coded.point(code))).collect()))
        });
        let states = segment_two();
        let known = states.iter().map(|s| s.unit()).collect();
        Tables { four, coded, states, known }
    })
}

/// Which of the four states a point is nearest, by its label in the table.
fn four_label(z: Complex) -> usize {
    let Slicer::Table(four) = &tables().four else { unreachable!("the four states are a table") };
    four.nearest(z)
}

fn table(rate: Rate) -> Slicer {
    tables().coded[rate.bits() - 3].clone()
}

/// What the receiver is doing with the burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Listening for segment 1 turning into segment 2, or collecting the
    /// training it is about to solve on.
    Listening,
    /// Following a long train's four states on to segment 4.
    Training,
    /// Following the four states with the old taps, and waiting to see
    /// whether segment 4 comes after 38 of them.
    Telling,
    /// Segment 4 and the data after it.
    Data,
}

/// What the core has been asked to train on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    /// A long train's segment 2, where the reversal said.
    Long,
    /// A long train's segment 2, somewhere near where segment 1 ended.
    Lapsed,
    /// Nothing: the old taps and a resync search.
    Resync,
}

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Waiting,
    /// Deciding against the four states until segment 4 begins, at symbol
    /// `data` of segment 2 counting from its first.
    Training { data: u64 },
    /// Following the four states with the old taps: the points from
    /// [`RESYNC_AT`] kept until they have been checked against segment 2,
    /// and then the points from [`PROBE`]'s first on, held until they say
    /// which training this is.
    Telling { kept: Vec<(u64, Complex)>, checked: bool },
    Data,
}

/// V.17 receiver, at every rate.
#[derive(Debug)]
pub struct Receiver {
    core: Core,
    fs: f64,
    rate: Rate,
    trellis: Decoder,
    descrambler: Scrambler,
    bits: Vec<bool>,
    /// Symbols still to be decided as the nearest point: see [`FRESH`].
    fresh: u32,
    /// Slips the core had found at the last symbol.
    slips: u32,
    /// The last symbol made, at unit power.
    last: Complex,
    /// Circuit 109, and the level under it: the burst's own, whether it is
    /// up to that, and for how many samples it has not been.
    carrier: bool,
    loudest: f64,
    loud: bool,
    quiet: usize,
    part: Part,
    /// Which symbol of the burst the next one made is, counted from the first
    /// of segment 2.
    symbol: u64,
    /// The half-symbol sample the core's hunt heard segment 2 begin at.
    at: u64,
    /// How many half symbols after the reversal segment 2's first symbol
    /// was, the last time a long train was solved: where the old taps put it.
    offset: i64,
    /// A long train has been solved for, so there are taps worth keeping.
    taught: bool,
    /// The training a reversal or a lapse calls for, held back while the hunt
    /// goes on listening (V.32's), and what the core is doing now.
    pending: Option<(Attempt, Solve, u64)>,
    attempt: Option<Attempt>,
    /// The carrier's turn and the far clock's drift as segment 1 had them.
    measured: Option<(f64, f64)>,
    /// What the last burst turned out to begin with, and how the receiver
    /// came to be following it.
    heard: Option<Training>,
    via: Option<Via>,
    s_offset_hz: Option<f64>,
}

impl Receiver {
    pub fn new(fs: f64) -> Self {
        let rate = Rate::default();
        Self {
            core: Core::new(Band::new(fs, BAUD, CARRIER), Options::fixed(), tables().four.clone()),
            fs,
            rate,
            trellis: Decoder::new(rate.coded()),
            descrambler: scrambler(),
            bits: Vec::new(),
            fresh: FRESH,
            slips: 0,
            last: Complex::ZERO,
            carrier: false,
            loudest: 0.0,
            loud: false,
            quiet: usize::MAX,
            part: Part::Waiting,
            symbol: 0,
            at: 0,
            offset: 0,
            taught: false,
            pending: None,
            attempt: None,
            measured: None,
            heard: None,
            via: None,
            s_offset_hz: None,
        }
    }

    /// Set the rate the burst about to arrive is at, which a fax receiver
    /// always knows from the DCS that came before it. Everything the line
    /// taught is kept.
    pub fn set_rate(&mut self, rate: Rate) {
        self.rate = rate;
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Forget the burst just gone and listen for the next.
    ///
    /// The taps a long train left are kept, unlike V.29's and V.27 ter's,
    /// which start every burst from nothing: a resync is made for exactly
    /// that, and without them a page after its training check could not be
    /// read at all.
    pub fn restart(&mut self) {
        self.end_burst();
        self.carrier = false;
        self.loudest = 0.0;
        self.loud = false;
        self.quiet = usize::MAX;
        self.bits.clear();
    }

    fn end_burst(&mut self) {
        self.core.hold(false);
        self.core.hunt();
        self.pending = None;
        self.attempt = None;
        self.part = Part::Waiting;
    }

    pub fn feed(&mut self, sample: f64) {
        self.core.feed(sample);
        self.watch_level();
        self.listen();
        // A training nothing has overtaken: its window is in.
        if self.pending.as_ref().is_some_and(|(_, _, due)| self.core.halves() >= *due)
            && let Some((attempt, solve, _)) = self.pending.take()
        {
            self.attempt = Some(attempt);
            self.core.train(solve);
        }
        while let Some(point) = self.core.next() {
            let target = self.decide(&point);
            let Some(symbol) = self.core.settle(target) else { break };
            self.last = symbol.point;
            if self.core.slips() != self.slips {
                // Found again after a slip: the trellis decoder's paths
                // straddle the jump.
                self.slips = self.core.slips();
                self.fresh = FRESH;
            }
            self.fresh = self.fresh.saturating_sub(1);
            self.symbol += 1;
            self.moved_on();
            // A training asked for from the symbols themselves.
            if !self.core.is_tracking() {
                break;
            }
        }
    }

    /// Circuit 109, and the end of a burst.
    fn watch_level(&mut self) {
        let envelope = self.core.envelope();
        self.loudest = envelope.max(self.loudest * (1.0 - LOUDEST_DECAY));
        self.loud = if self.loud {
            envelope > CARRIER_OFF.max(OFF_BELOW * self.loudest)
        } else {
            envelope > CARRIER_ON.max(ON_ABOVE * self.loudest)
        };
        if self.loud {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        // Each once a quiet spell.
        if self.quiet == (CARRIER_HOLD * self.fs) as usize {
            self.carrier = false;
        }
        if self.quiet == (BURST_GONE * self.fs) as usize {
            // Whatever the burst was doing, it is over, or never came:
            // listen for the next one.
            self.end_burst();
        }
        // 5.1.4: "Circuit 109 shall be turned ON during the reception of
        // segment 4", and not before, so that a far end's talker-echo
        // protection or a training that never trained is not a carrier.
        if !self.carrier && self.loud && self.part == Part::Data {
            self.carrier = true;
        }
    }

    /// Act on what the core has heard.
    fn listen(&mut self) {
        while let Some(heard) = self.core.heard() {
            match heard {
                // S again: what stopped it, or turned it, was a hole in
                // segment 1 and not its end.
                Heard::S => self.pending = None,
                Heard::Reversal { at, turn, drift } => {
                    // 5.1.2's C D C D against segment 1's A B A B: segment 2
                    // begins where the reversal is heard, give or take the
                    // search.
                    self.s_offset_hz = Some(turn * BAUD / TAU);
                    self.measured = Some((turn, drift));
                    self.at = at;
                    self.pending = Some(if self.taught { self.resync_at(at) } else { self.long_at(at, Attempt::Long) });
                    self.part = Part::Waiting;
                    // Held back while the hunt goes on, as V.32's is: a
                    // concealment that repeats a fragment of segment 1 has
                    // phase jumps in it that read as the reversal, and if
                    // segment 1 goes on afterwards the hunt hears it.
                    self.core.hunt();
                }
                Heard::Lapsed { at } => {
                    self.measured = self.core.s_measured();
                    let start = at + LAPSED_AHEAD;
                    self.at = start;
                    let solve = self.solve(start, LAPSED, None);
                    self.pending = Some((Attempt::Lapsed, solve, due(start, LAPSED)));
                    self.part = Part::Waiting;
                    self.core.hunt();
                }
                Heard::Trained { via, .. } => self.trained(via),
                Heard::Untrained => match self.attempt {
                    // The old taps found nothing on the four states. The
                    // line may have changed under a long train, which can be
                    // solved for anyway; or the reversal was a hole in
                    // segment 1, which then goes on. So the solve waits for
                    // its window with the hunt listening, as a reversal's
                    // does, and segment 1 heard again puts it aside.
                    Some(Attempt::Resync) => {
                        self.pending = Some(self.long_at(self.at, Attempt::Long));
                        self.attempt = None;
                        self.part = Part::Waiting;
                        self.core.hunt();
                    }
                    // Nothing fitted where segment 2 was said to be. The far
                    // end may be starting again; listen for the next.
                    _ => self.end_burst(),
                },
            }
        }
    }

    /// A long train's solve from the reversal at `at`.
    fn long_at(&self, at: u64, attempt: Attempt) -> (Attempt, Solve, u64) {
        (attempt, self.solve(at, FIRST, Some(RETRY)), due(at, FIRST))
    }

    fn solve(&self, start: u64, first: Window, retry: Option<Window>) -> Solve {
        Solve {
            targets: tables().known.clone(),
            start,
            first,
            retry,
            turn: self.measured.map(|(turn, _)| turn),
            drift: self.measured.map(|(_, drift)| drift),
            accept_db: ACCEPT_DB,
            slicer: tables().four.clone(),
            fallback: self.taught,
        }
    }

    /// A resync from the reversal at `at`: nothing to solve for, so the core
    /// falls back on the taps it has and searches for where the symbols are
    /// with them, as it does for a training that fits nothing.
    ///
    /// The search runs once RESYNC_AT symbols of segment 2 are in, and the
    /// core then makes symbols from there on, centred where the old taps say
    /// segment 2's symbols fall: `at` and the offset the last long train
    /// found, and RESYNC_AT symbols on.
    fn resync_at(&self, at: u64) -> (Attempt, Solve, u64) {
        let start = at.saturating_add_signed(self.offset);
        let window = Window { align: (0, RESYNC_AT as usize), solve: (0, RESYNC_AT as usize), search: 0 };
        let solve = Solve {
            targets: Vec::new(),
            start,
            first: window,
            retry: None,
            turn: None,
            drift: None,
            accept_db: ACCEPT_DB,
            slicer: tables().four.clone(),
            fallback: true,
        };
        // The core looks once the half after its window's reach is made,
        // and centres the next symbol seventeen halves short of the newest:
        // this makes that symbol segment 2's RESYNC_AT.
        (Attempt::Resync, solve, start + 2 * RESYNC_AT + 16)
    }

    /// The core is tracking.
    fn trained(&mut self, via: Via) {
        self.via = Some(via);
        self.fresh = FRESH;
        self.slips = self.core.slips();
        let long_data = Training::Long.before_segment_four();
        match (self.attempt, via) {
            (Some(attempt @ (Attempt::Long | Attempt::Lapsed)), Via::First | Via::Retry) => {
                let window = match (attempt, via) {
                    (Attempt::Lapsed, _) => LAPSED,
                    (_, Via::First) => FIRST,
                    _ => RETRY,
                };
                let end = window.solve.1 as u64;
                // The next symbol is the one after the window, exactly: the
                // solve found where segment 2's first symbol is.
                self.symbol = end;
                if attempt == Attempt::Long {
                    let origin = self.core.next_half() as i64 - 2 * end as i64;
                    self.offset = origin - self.at as i64;
                }
                self.taught = true;
                self.heard = Some(Training::Long);
                self.part = Part::Training { data: long_data };
            }
            (Some(Attempt::Resync), _) => {
                self.symbol = self.place();
                self.part = Part::Telling { kept: Vec::new(), checked: false };
            }
            // A long train that fitted nothing, followed with the old taps.
            _ => {
                self.symbol = self.place();
                self.heard = Some(Training::Long);
                if self.symbol >= long_data {
                    self.enter_data();
                } else {
                    self.part = Part::Training { data: long_data };
                }
            }
        }
    }

    /// Which symbol of segment 2 the next one made is, from where it is
    /// centred and where the old taps put segment 2's first symbol.
    fn place(&self) -> u64 {
        let origin = self.at as i64 + self.offset;
        let halves = self.core.next_half() as i64 - origin;
        (halves as f64 / 2.0).round().max(0.0) as u64
    }

    /// What the loops should take one symbol to be, and its bits.
    fn decide(&mut self, point: &Point) -> Complex {
        match &mut self.part {
            Part::Waiting | Part::Training { .. } => point.nearest,
            Part::Telling { kept, checked } => {
                if (!*checked && self.symbol >= RESYNC_AT) || self.symbol >= PROBE.0 {
                    kept.push((self.symbol, point.z));
                }
                point.nearest
            }
            Part::Data => {
                let target = self.data(point.z);
                if self.fresh > 0 { point.nearest } else { target.unwrap_or(point.nearest) }
            }
        }
    }

    /// One symbol of segment 4 or the data through the trellis decoder: its
    /// bits once the decoder is sure of them, and its best guess at the
    /// symbol now (design.md 2.6).
    fn data(&mut self, z: Complex) -> Option<Complex> {
        let coded = self.rate.coded();
        let at = z.scale(CONSTELLATION_RMS);
        if let Some(group) = self.trellis.decode((at.re, at.im)) {
            for &bit in &group[..coded.bits] {
                let out = self.descrambler.descramble(bit);
                self.bits.push(out);
            }
        }
        let code = self.trellis.tentative()?;
        let (x, y) = coded.point(code);
        Some(Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS))
    }

    /// Where the symbols have got to, after each.
    fn moved_on(&mut self) {
        match &mut self.part {
            Part::Training { data } if self.symbol >= *data => self.enter_data(),
            Part::Telling { kept, checked: false } if self.symbol >= CHECKED => {
                let kept = std::mem::take(kept);
                match check(&kept) {
                    Some(late) => {
                        self.symbol = self.symbol.saturating_add_signed(late);
                        self.part = Part::Telling { kept: Vec::new(), checked: true };
                        self.moved_on();
                    }
                    // Not segment 2 after all: listen for the real one.
                    None => self.end_burst(),
                }
            }
            Part::Telling { checked: true, .. } if self.symbol == PROBE.0 => {
                // Nothing learns from these until it is known what they are.
                self.core.hold(true);
            }
            Part::Telling { kept, checked: true } if self.symbol >= PROBE.2 => {
                let kept = std::mem::take(kept);
                self.tell(kept);
            }
            _ => {}
        }
    }

    /// Tell a resync from a long train by what came after 38 symbols of
    /// segment 2.
    fn tell(&mut self, kept: Vec<(u64, Complex)>) {
        self.core.hold(false);
        let Slicer::Table(four) = &tables().four else { unreachable!("the four states are a table") };
        let judged: Vec<f64> = kept
            .iter()
            .filter(|(k, _)| (PROBE.1..PROBE.2).contains(k))
            .map(|(_, z)| (*z - four.points()[four.nearest(*z)]).norm_sqr())
            .collect();
        let mean = judged.iter().sum::<f64>() / judged.len().max(1) as f64;
        if !judged.is_empty() && mean < LONG_BELOW {
            // More of segment 2: a long train, sent by a far end starting
            // afresh, and solved for as though nothing were known.
            let (attempt, solve, _) = self.long_at(self.at, Attempt::Long);
            self.attempt = Some(attempt);
            self.part = Part::Waiting;
            self.core.train(solve);
            return;
        }
        // Segment 4: a resync. What was held goes to the trellis decoder as
        // the start of it.
        self.heard = Some(Training::Resync);
        self.enter_data();
        for (_, z) in kept {
            self.data(z);
        }
    }

    /// Segment 4: the channel's own constellation from here on.
    fn enter_data(&mut self) {
        self.core.set_slicer(table(self.rate));
        self.trellis.set_coding(self.rate.coded());
        self.fresh = FRESH;
        self.part = Part::Data;
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// Take whole octets, most significant bit first, leaving any remainder.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        let whole = self.bits.len() / 8;
        let bits: Vec<bool> = self.bits.drain(..whole * 8).collect();
        bits.as_chunks::<8>().0.iter().map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b))).collect()
    }

    /// Circuit 109: on from segment 4 while the level holds, off 30 to 50 ms
    /// after it falls (3.6).
    pub fn carrier(&self) -> bool {
        self.carrier
    }

    /// The in-band amplitude of what arrives, over 20 ms.
    pub fn level(&self) -> f64 {
        self.core.envelope()
    }

    /// The last symbol made, equalised, at the constellation's own unit
    /// power; it changes once for every symbol made.
    pub fn constellation_point(&self) -> (f64, f64) {
        (self.last.re, self.last.im)
    }

    /// How far out the rate's constellation reaches in those units.
    pub fn constellation_peak(&self) -> f64 {
        self.rate.peak()
    }

    /// Mean distance of the symbols from the nearest point, in the units
    /// [`point_spacing`](Self::point_spacing) is measured in.
    pub fn residual_error(&self) -> f64 {
        self.core.residual_error()
    }

    /// How far apart the rate's closest two points are, in the units
    /// [`residual_error`](Self::residual_error) is measured in.
    pub fn point_spacing(&self) -> f64 {
        self.rate.coded().closest() / CONSTELLATION_RMS
    }

    pub fn stage(&self) -> Stage {
        match self.part {
            Part::Waiting => Stage::Listening,
            Part::Training { .. } => Stage::Training,
            Part::Telling { .. } => Stage::Telling,
            Part::Data => Stage::Data,
        }
    }

    /// Which training the last burst followed began with.
    pub fn heard(&self) -> Option<Training> {
        self.heard
    }

    /// How the receiver last came to be tracking.
    pub fn trained_via(&self) -> Option<Via> {
        self.via
    }

    /// Signal to noise of every symbol against its nearest point, in
    /// decibels.
    pub fn snr_db(&self) -> f64 {
        self.core.snr_db()
    }

    /// What the last training or resync came to, in decibels.
    pub fn trained_snr_db(&self) -> f64 {
        self.core.trained_snr_db()
    }

    /// The far carrier's offset from 1800 Hz as the last segment 1 said it.
    pub fn s_offset_hz(&self) -> Option<f64> {
        self.s_offset_hz
    }

    /// The far carrier's offset as the carrier loop has it, in hertz.
    pub fn offset_hz(&self) -> f64 {
        self.core.offset_hz()
    }

    /// The far clock's rate against this end's, in parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.core.drift_ppm()
    }

    /// Slips found and followed.
    pub fn slips(&self) -> u32 {
        self.core.slips()
    }

    /// Whether the signal has jumped and not been found again yet.
    pub fn is_lost(&self) -> bool {
        self.core.is_lost()
    }
}

/// Whether the points kept from a resync are segment 2's, and if they are,
/// how many symbols later in it they are than they were counted to be.
///
/// Every quarter turn is tried, because the resync's phase is only known to
/// within one, and every start a couple of symbols either side of the count.
fn check(kept: &[(u64, Complex)]) -> Option<i64> {
    let states = &tables().states;
    if kept.len() < 8 {
        return None;
    }
    let mut best = (usize::MAX, 0);
    for late in -2i64..=2 {
        for turn in 0..4 {
            let misses = kept
                .iter()
                .filter(|(k, z)| {
                    let Some(state) = k.checked_add_signed(late).and_then(|k| states.get(k as usize)) else { return true };
                    four_label(*z) != (*state as usize + turn) % 4
                })
                .count();
            if misses < best.0 {
                best = (misses, late);
            }
        }
    }
    (best.0 <= CHECK_MISSES).then_some(best.1)
}

/// The half-symbol sample by which a training from `start` has all of its
/// first window in: the core's own reckoning, with a little over its
/// equaliser's reach to spare (V.32's `due`).
fn due(start: u64, window: Window) -> u64 {
    let end = window.solve.1.max(window.align.1) as u64;
    start + window.search.max(0) as u64 + 2 * end + 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_is_at_unit_power_and_sliced_as_by_search() {
        let tables = tables();
        for slicer in std::iter::once(&tables.four).chain(&tables.coded) {
            let Slicer::Table(table) = slicer else { panic!("V.17's constellations are tables") };
            assert!((table.power() - 1.0).abs() < 1e-12, "{} points at power {}", table.len(), table.power());
            let mut seed = 0x2545_f491u32;
            for _ in 0..20_000 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let a = f64::from(seed) / f64::from(u32::MAX) - 0.5;
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let b = f64::from(seed) / f64::from(u32::MAX) - 0.5;
                let z = Complex::new(a, b).scale(4.0);
                assert_eq!(table.nearest(z), table.nearest_exhaustive(z), "{} points at {z:?}", table.len());
            }
        }
    }

    #[test]
    fn the_known_segment_two_is_the_transmitters() {
        let known = &tables().known;
        assert_eq!(known.len(), 2976);
        // Table 4's opening, at unit power.
        use State::{B, C, D};
        for (k, state) in [C, D, C, D, C, D, C, D, C, D, C, D, B, D, B, D].into_iter().enumerate() {
            assert!((known[k] - state.unit()).norm_sqr() < 1e-24, "symbol {k}");
        }
    }
}
