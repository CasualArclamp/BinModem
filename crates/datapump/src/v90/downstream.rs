//! What comes down a PCM call, read: the signs, the DIL, R, and whole data
//! frames.
//!
//! Downstream there is no constellation and no carrier, so none of V.34's
//! reading applies. Everything the analogue modem learns about the route and
//! everything the digital modem tells it arrives as one of four things, and
//! this module is all four:
//!
//! 1. **Signs.** Through TRN1d, Jd and J'd the line carries one codeword at
//!    either sign, so the only information in a symbol is whether it was
//!    positive. [`JdReader`] descrambles those signs, switches to differential
//!    decoding where Jd begins, and hands out each Jd it can read whole.
//! 2. **Levels the receiver was told to expect.** The DIL is a list of
//!    codewords both ends already agree on, sent so that the analogue end can
//!    see what the route does to each. [`DilReader`] counts them off, and --
//!    because a jitter buffer's slip moves every symbol after it -- knows how
//!    to find the list again when it has moved.
//! 3. **A sign pattern.** R is one frame of codewords repeated, `+ + + - - -`,
//!    and R-bar is the same turned over. [`RWatch`] finds it whatever
//!    rotation it arrives at, which is how a slip inside phase 4 is measured.
//! 4. **Whole data frames.** From TRN2d on, [`Frames`] slices each interval
//!    against the constellation in use, decodes the frame and descrambles it,
//!    and [`find_place`] puts the frames back where they belong when the
//!    numbers stop being numbers the digital modem could have sent.
//!
//! It lives on its own because V.92's analogue modem reads exactly the same
//! things. V.92 rewrites the upstream and takes the downstream over by
//! reference, in one sentence each: its DIL is 8.6.1 -> 8.4.1/V.90, its Ri is
//! 8.6.5 -> 8.6.4/V.90, and its Rd and Rt are 8.8.4 -> 8.6.4/V.90, which is
//! where R and R-bar themselves are defined. So the only V.92-shaped seams
//! here are the three a caller supplies -- which frames a [`JdReader`] will
//! decode at all, how long a [`RWatch`]'s sign pattern is and where it
//! starts, and what sequence finder a [`Frames`] runs the descrambled bits
//! through -- and nothing in this module needs to know which Recommendation
//! is calling it.

use std::collections::VecDeque;

use crate::v32::{Mode, Scrambler};
use crate::v34::mp::{Finder, Mp};

use super::INTERVALS;
use super::dil::{Analysis, Route};
use super::encoder::{Decoder, Frame, Mapping};
use super::pcm::{self, Slicer};
use super::sequences::{Cp, Descriptor, JD_BITS, JD_PRIME_BITS, Jd};
use super::ucode::{self, Law};

/// The end of a Jd that J'd is found after: its CRC and fill.
const JD_TAIL: usize = 24;

/// Frames of R in a row before it is believed, and of R a whole number of
/// symbols out of step before the frames are taken to have moved.
const R_HEARD: usize = 8;
const R_MOVED: usize = 6;

/// How long R-bar runs, in symbols. Neither 8.6.4 nor 8.8.4/V.92 prints a
/// duration for it: both count repetitions. 8.6.4 says "R-bar consists of 4
/// repetitions of the 6-symbol sequence", which is 24 symbols, and 8.8.4/V.92
/// says "R-bar-f consists of 2 repetitions of the 12-symbol sequence", which
/// is 24 again although its signs repeat every 4 symbols rather than every 6.
/// The clause that does print a duration is 9.9.1.1.1/V.92, "signal Rf for
/// 384T followed by R-bar-f for 24T", and it agrees. The same length whatever
/// the pattern, so it is counted in symbols.
const R_BAR_SYMBOLS: u64 = 24;

/// The DIL: symbols read before they are counted, so that what arrived just
/// before a loss was noticed is held with what comes after it; symbols a
/// search for where the DIL went looks at; and how far a slip can move it.
const DIL_DELAY: usize = 96;
const DIL_SEARCH: usize = 128;
const DIL_MOST_MOVED: i64 = 400;

/// When to look for where the DIL went, and how often after that: a buffer
/// that made up what it lost plays a faded copy of what went before for
/// twenty milliseconds or more, and nothing fits that until it is over. What
/// is kept meanwhile, and how closely the DIL must fit what arrived, as the
/// error's power against the signal's.
const DIL_FIRST_LOOK: usize = 288;
const DIL_LOOK_EVERY: usize = 32;
const DIL_KEPT_LOST: usize = 2048;
const DIL_FIT: f64 = 0.01;

/// DIL levels above this are not learned from or judged by: the loudest
/// codewords are where a softphone's conversion runs out of headroom, and
/// they come back wherever it leaves them. As a fraction of full scale.
const DIL_TRUSTED: f64 = 0.3;

/// Symbols at the start of a DIL segment that a loud one before it spoils:
/// its references, and the frame after them, which on a live call still
/// carried a few thousandths of full scale of it.
const DIL_SPILL: usize = 2 * INTERVALS;

/// A segment after one more than this many times as loud, and louder than
/// this, is spilled into too: where one sweep up the codewords ends and the
/// next begins.
const DIL_SPILL_RATIO: f64 = 4.0;
const DIL_SPILL_LEVEL: f64 = 0.01;

/// Training symbols a stretch of the DIL has to have, loud enough to judge,
/// before where it falls can be judged from it.
const DIL_TRAINED_JUDGED: usize = 32;

/// Signs a move has to agree with, of the DIL symbols it is judged on.
const DIL_SIGNS: f64 = 0.9;

/// Finding the DIL when J'd went unread: how much is kept to look in, and how
/// much of it a start is judged on.
const DIL_START_KEPT: usize = 1200;
const DIL_START_WINDOW: usize = 480;

/// Frames over which a read of impossible numbers is counted, how many make
/// it a lost place, and symbols kept for finding the place again.
const PLACE_WINDOW: usize = 24;
const PLACE_LOST: usize = 3;
const PLACE_KEPT: usize = 48 * INTERVALS;

/// Reads Jd and J'd off the signs (8.4.2, 8.4.3).
///
/// What counts as a readable frame is the caller's: [`Self::feed`] takes an
/// acceptance closure over the raw framed bits and calls [`Jd::from_bits`]
/// only where it says yes. V.90 says yes to everything, because a Jd is all
/// that arrives here. V.92 sends a Jp straight after the Jds -- another 72
/// framed bits, in the same modulation, with a CRC of its own (V.92 8.6.3,
/// Table 22) -- so a V.90-shaped parser would read one as a Jd full of rates
/// nobody offered. A V.92 analogue modem passes a closure that refuses it.
#[derive(Debug, Clone)]
pub(crate) struct JdReader {
    descrambler: Scrambler,
    differential: bool,
    previous: bool,
    bits: VecDeque<bool>,
    /// The last Jd read whole, and the symbol after it.
    last: Option<(u64, Jd)>,
    /// Its bits, as they went.
    jd_bits: Vec<bool>,
}

impl JdReader {
    pub(crate) fn new() -> Self {
        Self {
            descrambler: Scrambler::new(Mode::Call),
            differential: false,
            previous: false,
            bits: VecDeque::new(),
            last: None,
            jd_bits: Vec::new(),
        }
    }

    /// The last Jd read whole, and the symbol after it.
    pub(crate) fn last(&self) -> Option<(u64, Jd)> {
        self.last
    }

    /// One symbol's sign, with `accept` deciding which framed bits are worth
    /// decoding at all. True when this symbol ended a J'd.
    pub(crate) fn feed(&mut self, index: u64, positive: bool, accept: impl Fn(&[bool]) -> bool) -> bool {
        let before = self.descrambler.clone();
        let mut bit = self.descrambler.descramble(if self.differential { positive ^ self.previous } else { positive });
        if !self.differential && !bit {
            // TRN1d descrambles to ones: the first zero is Jd, which is
            // differential from here -- and was for the symbol that showed
            // it.
            self.differential = true;
            self.descrambler = before;
            bit = self.descrambler.descramble(positive ^ self.previous);
        }
        self.previous = positive;
        self.bits.push_back(bit);
        if self.bits.len() > JD_BITS + JD_PRIME_BITS {
            self.bits.pop_front();
        }
        let n = self.bits.len();
        let bits = self.bits.make_contiguous();
        if n >= JD_BITS
            && accept(&bits[n - JD_BITS..])
            && let Some(jd) = Jd::from_bits(&bits[n - JD_BITS..])
        {
            self.last = Some((index + 1, jd));
            self.jd_bits = bits[n - JD_BITS..].to_vec();
        }
        // "12 binary zeroes" where the next Jd's sync would start: after the
        // end of a Jd, wherever that fell. A softphone that cut a few
        // milliseconds out of the last Jd leaves it unreadable whole, but
        // its tail is the tail of every other.
        self.jd_bits.len() == JD_BITS
            && n >= JD_TAIL + JD_PRIME_BITS
            && bits[n - JD_PRIME_BITS..].iter().all(|b| !*b)
            && bits[n - JD_PRIME_BITS - JD_TAIL..n - JD_PRIME_BITS] == self.jd_bits[JD_BITS - JD_TAIL..]
    }
}

/// Watches for R and its turn to R-bar (8.6.4).
///
/// The equaliser has already turned a line that inverts the signal back
/// over, so R and R-bar are told apart by their signs. What the watch cannot
/// take for granted is where the frames are: a jitter buffer's slip moves
/// every symbol after it by a whole number of symbols, and R, which is the
/// same frame over and over, shows by how many. R-bar is R moved by half its
/// pattern, which no slip of whole milliseconds does.
///
/// The pattern's period is the caller's, and so is where it starts. 6 for R,
/// Ri, Rd and Rt, whose signs are `+ + + - - -` (8.6.4); 4 for V.92's Rf,
/// which 8.8.4/V.92 prints as the 12-symbol sequence
/// `+ + - - + + - - + + - -`, four symbols of sign repeated three times.
///
/// The start matters because the two are not the same length. R's period is
/// the data frame's own, so R begins wherever a frame does and
/// [`pcm::Symbol::index`], which is frame-aligned, is phase enough. Rf
/// "shall begin on the boundary of a data frame" (9.9.1.1.1/V.92) as well,
/// but a data frame is six symbols and its pattern is four, so Rf's first
/// symbol falls as often two symbols into the pattern as none. A watch that
/// took the index alone for its phase would read every group of such an Rf
/// as the pattern already rotated by half -- R-bar's rotation -- and, never
/// having heard Rf, would refuse that and reset: it would hear nothing for
/// the whole 384T of Rf and the 24T of R-bar-f. Hence `origin`, the index of
/// the pattern's first symbol.
#[derive(Debug, Clone)]
pub(crate) struct RWatch {
    period: usize,
    /// Where the pattern's first symbol falls in the symbol count, reduced
    /// to the pattern.
    phase: u64,
    frame: Vec<f64>,
    /// Frames in a row of R moved by `moved` symbols.
    run: usize,
    moved: usize,
    heard: bool,
    /// Whether the last whole frame looked like R or R-bar, however moved.
    looked: bool,
}

/// What the watch made of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RSeen {
    Nothing,
    /// R has turned into R-bar: what follows it begins at this symbol.
    Turned(u64),
    /// R is arriving this many symbols late: the frames have moved. Only a
    /// watch whose pattern is a data frame long reports it, because only
    /// then is a rotation of the pattern a rotation of the frame.
    Moved(usize),
}

impl RWatch {
    /// A watch for a sign pattern `period` symbols long whose first symbol
    /// is at `origin`. The period is an even number: the pattern is half
    /// positive then half negative, and its turn is itself rotated by half.
    /// Only `origin` modulo the period matters, so V.90 passes 0 -- R, Ri,
    /// Rd and Rt all begin on a data frame boundary and their period is the
    /// data frame's own, which makes every boundary the same phase.
    pub(crate) fn new(period: usize, origin: u64) -> Self {
        debug_assert!(period >= 2 && period.is_multiple_of(2), "a sign pattern has two halves");
        let phase = origin % period as u64;
        Self { period, phase, frame: vec![0.0; period], run: 0, moved: 0, heard: false, looked: false }
    }

    /// Whether R itself has been heard for long enough to believe.
    pub(crate) fn heard(&self) -> bool {
        self.heard
    }

    /// Whether the last whole frame looked like R or R-bar, however moved.
    pub(crate) fn looked(&self) -> bool {
        self.looked
    }

    /// One symbol, and the level R has in each interval of the **data
    /// frame**. Not of the pattern: the signs repeat on the pattern's period,
    /// but the codewords are "the highest power PCM codeword from the data
    /// mode constellation of each data frame interval" (8.6.4, and 8.8.4/V.92
    /// for Rf in the same words), which repeat on the frame's six however
    /// long the pattern is. So the levels are always six, which is also what
    /// keeps a caller from handing a four-symbol pattern four of them.
    pub(crate) fn feed(&mut self, symbol: &pcm::Symbol, levels: &[f64; INTERVALS]) -> RSeen {
        let period = self.period;
        let i = ((symbol.index + period as u64 - self.phase) % period as u64) as usize;
        self.frame[i] = symbol.value;
        if i != period - 1 {
            return RSeen::Nothing;
        }
        let frame_start = symbol.index + 1 - period as u64;
        // R moved by m: "+ + + - - -", or whatever the period makes of it,
        // starting m symbols in. A symbol found m late was sent m earlier, so
        // it carries the codeword of the interval m before the one it landed
        // in -- which for V.90, whose pattern and frame are both six and
        // start together, is the pattern position k itself.
        let late = (0..period).find(|&m| {
            (0..period).all(|j| {
                let k = (j + period - m) % period;
                let back = (m % INTERVALS) as u64;
                let sent = ((frame_start + j as u64 + INTERVALS as u64 - back) % INTERVALS as u64) as usize;
                let (v, level) = (self.frame[j], levels[sent]);
                (v >= 0.0) == (k < period / 2) && (0.5 * level..1.5 * level).contains(&v.abs())
            })
        });
        self.looked = late.is_some();
        let turn = period / 2;
        match late {
            Some(0) => {
                self.run = if self.moved == 0 { self.run + 1 } else { 1 };
                self.moved = 0;
                if self.run >= R_HEARD {
                    self.heard = true;
                }
                RSeen::Nothing
            }
            Some(m) if m == turn && self.heard => {
                // R-bar's first frame, and R-bar runs 24 symbols.
                RSeen::Turned(frame_start + R_BAR_SYMBOLS)
            }
            Some(m) if m != turn => {
                self.run = if self.moved == m { self.run + 1 } else { 1 };
                self.moved = m;
                if self.run < R_MOVED {
                    return RSeen::Nothing;
                }
                self.run = 0;
                self.moved = 0;
                // A rotation of the pattern is a move of the data frames
                // only where the pattern is a data frame long. Rf's four
                // symbols would say the slip was m of four, which no caller
                // can turn into the move of six `Modem::move_frames` wants.
                if period == INTERVALS { RSeen::Moved(m) } else { RSeen::Nothing }
            }
            _ => {
                if !self.heard {
                    self.run = 0;
                }
                RSeen::Nothing
            }
        }
    }
}

/// Signed levels of each interval's constellation, as the route delivers
/// them: (level, Ucode, positive).
pub(crate) type Levels = [Vec<(f64, u8, bool)>; INTERVALS];

/// A CP's constellations as this route delivers them, either sign.
pub(crate) fn levels_for(cp: &Cp, route: &Route) -> Levels {
    std::array::from_fn(|i| {
        cp.points(i)
            .into_iter()
            .flat_map(|u| {
                let level = route.levels[i][usize::from(u)];
                [(level, u, true), (-level, u, false)]
            })
            .collect()
    })
}

/// The least distance between two of a CP's levels, either sign, as the
/// route delivers them.
pub(crate) fn least_gap(cp: &Cp, route: &Route) -> f64 {
    (0..INTERVALS)
        .map(|i| {
            let mut levels: Vec<f64> =
                cp.points(i).iter().flat_map(|&u| [route.levels[i][usize::from(u)], -route.levels[i][usize::from(u)]]).collect();
            levels.sort_by(f64::total_cmp);
            levels.windows(2).map(|w| w[1] - w[0]).fold(f64::INFINITY, f64::min)
        })
        .fold(f64::INFINITY, f64::min)
}

/// The nearest of an interval's levels to `value`, as (Ucode, positive).
pub(crate) fn nearest(levels: &[(f64, u8, bool)], value: f64) -> (u8, bool) {
    levels
        .iter()
        .min_by(|a, b| (a.0 - value).abs().total_cmp(&(b.0 - value).abs()))
        .map_or((0, false), |l| (l.1, l.2))
}

/// The receiver's slicer for those levels: what phase 4 and data mode decide
/// each interval against, and learn from.
pub(crate) fn slicer_for(levels: &Levels) -> Slicer {
    Slicer::Levels(Box::new(std::array::from_fn(|i| levels[i].iter().map(|l| l.0).collect())))
}

/// What a DIL symbol can be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trust {
    /// Learned from, judged by, and counted.
    Yes,
    /// A codeword too loud to trust: only counted, so that the route shows
    /// what happened to it.
    Loud,
    /// The start of a segment a loud codeword spilled into: none of those.
    Spoiled,
}

/// Which of a DIL's symbols can be learned from and judged by.
pub(crate) fn trusted_symbols(descriptor: &Descriptor, law: Law) -> Vec<Trust> {
    let level = |u: u8| ucode::level(law, u);
    let loud = |u: u8| level(u) > DIL_TRUSTED;
    // What a louder segment leaves behind is small, but not beside a
    // segment a great deal quieter.
    let spills = |before: u8, u: u8| loud(before) || level(before) > (DIL_SPILL_RATIO * level(u)).max(DIL_SPILL_LEVEL);
    let mut out = Vec::with_capacity(descriptor.len());
    // The DIL repeats, so the first segment comes after the last.
    let mut before = descriptor.ucodes.last().copied();
    for &u in &descriptor.ucodes {
        let spoiled = before.is_some_and(|b| spills(b, u));
        for n in 0..descriptor.segment_length(u) {
            out.push(if spoiled && n < DIL_SPILL {
                Trust::Spoiled
            } else if loud(u) {
                Trust::Loud
            } else {
                Trust::Yes
            });
        }
        before = Some(u);
    }
    out
}

/// Where the DIL was found to be when J'd went unread: the receiver's count
/// at its first symbol, and the symbols already arrived that belong to it.
#[derive(Debug, Clone)]
pub(crate) struct DilStart {
    pub(crate) first: u64,
    pub(crate) already: Vec<(u64, f64)>,
}

/// The DIL as it is read (9.3.2.9), and what the route did to it.
///
/// Every method that has to steer the receiver takes it as an argument rather
/// than owning it, because the modem owns the receiver and drives the rest of
/// the start-up with it.
#[derive(Debug, Clone)]
pub(crate) struct DilReader {
    law: Law,
    uinfo: u8,
    /// The DIL this end asked for, symbol by symbol, and whether each symbol
    /// can be learned from and judged by: not in a segment of a codeword too
    /// loud to trust, and not at the start of the segment after one, which
    /// what the loud one did spills into.
    dil: Vec<(u8, bool)>,
    trusted: Vec<Trust>,
    /// The receiver's count at the DIL's first symbol, moved by any slip
    /// since; the frame interval that symbol was in; which symbols have been
    /// read, and how many are left.
    base: i64,
    interval: usize,
    read: Vec<bool>,
    left: usize,
    /// Symbols not yet counted, and since a slip was noticed, how many have
    /// been gathered to find where the DIL went.
    recent: VecDeque<(u64, f64)>,
    lost: Option<usize>,
    moved: u32,
    /// Symbols while J'd is awaited, for finding the DIL if J'd goes unread,
    /// and whether it was found that way.
    before: VecDeque<(u64, f64)>,
    found_late: bool,
    analysis: Analysis,
}

impl DilReader {
    /// A reader for the DIL `descriptor` describes, on a line of this law,
    /// whose references are UINFO.
    pub(crate) fn new(descriptor: &Descriptor, law: Law, uinfo: u8) -> Self {
        let dil: Vec<(u8, bool)> = descriptor.symbols().collect();
        let left = dil.len();
        Self {
            law,
            uinfo,
            trusted: trusted_symbols(descriptor, law),
            dil,
            base: 0,
            interval: 0,
            read: Vec::new(),
            left,
            recent: VecDeque::new(),
            lost: None,
            moved: 0,
            before: VecDeque::new(),
            found_late: false,
            analysis: Analysis::new(),
        }
    }

    /// Where the DIL's first symbol is, as the receiver counts, and the frame
    /// interval it is in.
    pub(crate) fn start(&self) -> (i64, usize) {
        (self.base, self.interval)
    }

    /// Times a slip moved the DIL and it was found again.
    pub(crate) fn moved(&self) -> u32 {
        self.moved
    }

    /// Whether the DIL had to be found without J'd.
    pub(crate) fn found_late(&self) -> bool {
        self.found_late
    }

    /// How much of the DIL has been read, of how much there is, and whether
    /// the reading is waiting to find where it went.
    pub(crate) fn progress(&self) -> (usize, usize, bool) {
        (self.dil.len() - self.left, self.dil.len(), self.lost.is_some())
    }

    /// What the whole pass showed of the route.
    pub(crate) fn route(&self) -> Route {
        self.analysis.route()
    }

    /// Whether a slip has been noticed and not yet made sense of.
    pub(crate) fn is_lost(&self) -> bool {
        self.lost.is_some()
    }

    /// A slip: what arrived just before it was noticed is held with the rest
    /// until it is known whether anything moved.
    pub(crate) fn lost(&mut self) {
        self.lost = Some(0);
    }

    /// One symbol while J'd is still awaited, kept in case J'd goes unread.
    pub(crate) fn keep(&mut self, raw: u64, value: f64) {
        self.before.push_back((raw, value));
        if self.before.len() > DIL_START_KEPT {
            self.before.pop_front();
        }
    }

    /// Whether enough has arrived since J'd was due to judge where the DIL
    /// starts.
    pub(crate) fn ready_to_look(&self) -> bool {
        self.before.len() >= DIL_START_WINDOW + INTERVALS
    }

    /// The DIL's start, from what has arrived since J'd was due: where the
    /// DIL fits what came after it closely and far better than anywhere else.
    pub(crate) fn find_start(&mut self) -> Option<DilStart> {
        let arrived: Vec<(u64, f64)> = self.before.iter().copied().collect();
        let &(newest, _) = arrived.last()?;
        let oldest = arrived[0].0;
        let mut fits = Vec::new();
        for first in oldest..=newest.saturating_sub(DIL_START_WINDOW as u64) {
            let from = (first - oldest) as usize;
            let window = &arrived[from..(from + DIL_START_WINDOW).min(arrived.len())];
            if let Some((fit, agree)) = self.fit(window, first as i64)
                && agree >= DIL_SIGNS
            {
                fits.push((first, fit));
            }
        }
        let &(first, fit) = fits.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
        let next = fits.iter().filter(|f| f.0.abs_diff(first) > 1).map(|f| f.1).fold(f64::INFINITY, f64::min);
        if fit > DIL_FIT || next < 4.0 * fit {
            return None;
        }
        self.found_late = true;
        let from = (first - oldest) as usize;
        Some(DilStart { first, already: arrived[from..].to_vec() })
    }

    /// The DIL from the receiver's count `first` (9.3.2.8): the frames put
    /// where the DIL says they are, and the symbols in `already` read as its
    /// first. True if that was the whole pass.
    pub(crate) fn begin(&mut self, rx: &mut pcm::Receiver, first: u64, already: &[(u64, f64)]) -> bool {
        self.base = first as i64;
        // J'd ends on a frame boundary -- Jd does, and J'd is two frames --
        // so the DIL's first symbol is in interval 0, whatever a slip did to
        // the frames on the way.
        self.interval = 0;
        rx.set_frame_offset((INTERVALS as u64 - first % INTERVALS as u64) % INTERVALS as u64);
        self.read = vec![false; self.dil.len()];
        self.left = self.dil.len();
        self.recent.clear();
        self.lost = None;
        self.before.clear();
        let next = already.last().map_or(first, |s| s.0 + 1);
        // Two passes: one, and what a slip loses of it read again.
        let levels = self.levels(next as i64, 2 * self.dil.len());
        rx.expect_from(next, levels);
        for &(raw, value) in already {
            if self.symbol(rx, raw, value) {
                return true;
            }
        }
        false
    }

    /// One DIL symbol from the receiver. True if that was the whole pass.
    pub(crate) fn symbol(&mut self, rx: &mut pcm::Receiver, raw: u64, value: f64) -> bool {
        self.recent.push_back((raw, value));
        if let Some(gathered) = self.lost {
            if self.recent.len() > DIL_KEPT_LOST {
                self.recent.pop_front();
            }
            self.lost = Some(gathered + 1);
            if gathered + 1 >= DIL_FIRST_LOOK && (gathered + 1).is_multiple_of(DIL_LOOK_EVERY) {
                return self.find(rx);
            }
            return false;
        }
        while self.recent.len() > DIL_DELAY {
            let Some((raw, value)) = self.recent.pop_front() else { break };
            if self.count(raw, value) {
                return true;
            }
        }
        false
    }

    /// One DIL symbol read, wherever in the DIL it falls: the DIL repeats,
    /// so a symbol a slip spoiled comes round again. True if that was the
    /// last of them.
    fn count(&mut self, raw: u64, value: f64) -> bool {
        let len = self.dil.len() as i64;
        let at = (raw as i64 - self.base).rem_euclid(len) as usize;
        if self.read[at] {
            return false;
        }
        self.read[at] = true;
        self.left -= 1;
        let (u, positive) = self.dil[at];
        if self.trusted[at] != Trust::Spoiled {
            self.analysis.feed(u, positive, (at + self.interval) % INTERVALS, value);
        }
        self.left == 0
    }

    /// How well `window` fits the DIL taken to start at the receiver's count
    /// `first`, over the symbols it can be judged on: the error's power
    /// against the DIL's, and the share of signs that agree. None if too few
    /// of the symbols are training symbols to tell one segment from another:
    /// every segment's references are alike.
    fn fit(&self, window: &[(u64, f64)], first: i64) -> Option<(f64, f64)> {
        let law = self.law;
        let len = self.dil.len() as i64;
        let (mut cost, mut power, mut agree, mut judged, mut trained) = (0.0, 0.0, 0usize, 0usize, 0usize);
        for &(raw, v) in window {
            let at = (raw as i64 - first).rem_euclid(len) as usize;
            let (u, positive) = self.dil[at];
            let level = ucode::level(law, u);
            // Too quiet for a sign to mean anything, or too near a loud
            // codeword to trust.
            if self.trusted[at] != Trust::Yes || level < 0.004 {
                continue;
            }
            let e = if positive { level } else { -level };
            cost += (v - e).powi(2);
            power += e * e;
            judged += 1;
            if u != self.uinfo {
                trained += 1;
            }
            if (v >= 0.0) == positive {
                agree += 1;
            }
        }
        (trained >= DIL_TRAINED_JUDGED && power > 0.0).then(|| (cost / power, agree as f64 / judged as f64))
    }

    /// The DIL's signed levels from the receiver's count `from`, for `n`
    /// symbols, as the DIL now stands against that count: NaN where a level
    /// is too loud to learn from.
    fn levels(&self, from: i64, n: usize) -> Vec<f64> {
        let law = self.law;
        let len = self.dil.len() as i64;
        (0..n as i64)
            .map(|k| {
                let at = (from + k - self.base).rem_euclid(len) as usize;
                let (u, positive) = self.dil[at];
                let level = ucode::level(law, u);
                if self.trusted[at] != Trust::Yes {
                    f64::NAN
                } else if positive {
                    level
                } else {
                    -level
                }
            })
            .collect()
    }

    /// Where the DIL went after a loss: the move that makes what has arrived
    /// lately most like it, once one does so clearly -- closely, and far
    /// better than any other. No move at all, if that fits: a route that
    /// robs a bit, or a burst of noise, spoils the reading without moving
    /// anything. True if the recount finished the pass.
    fn find(&mut self, rx: &mut pcm::Receiver) -> bool {
        let window: Vec<(u64, f64)> = self.recent.iter().skip(self.recent.len().saturating_sub(DIL_SEARCH)).copied().collect();
        // (move, relative error) for each move whose signs agree.
        let fits: Vec<(i64, f64)> = (-DIL_MOST_MOVED..=DIL_MOST_MOVED)
            .filter_map(|m| {
                let (fit, agree) = self.fit(&window, self.base + m)?;
                (agree >= DIL_SIGNS).then_some((m, fit))
            })
            .collect();
        let Some(&(moved, fit)) = fits.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { return false };
        let next = fits.iter().filter(|f| (f.0 - moved).abs() > 1).map(|f| f.1).fold(f64::INFINITY, f64::min);
        // A move is only a move against staying put. Where the stretch is
        // too quiet to say whether it is where it was, everything since the
        // loss is asked instead -- waiting for a louder stretch can wait
        // until the DIL has wrapped round to its own quiet start, where the
        // true move cannot be judged either -- and failing that, wait.
        if moved != 0 && !fits.iter().any(|f| f.0 == 0) && self.fit(&window, self.base).is_none() {
            let gathered = self.lost.unwrap_or(0);
            let since: Vec<(u64, f64)> = self.recent.iter().skip(self.recent.len().saturating_sub(gathered)).copied().collect();
            match self.fit(&since, self.base) {
                Some((fit, agree)) if fit > DIL_FIT || agree < DIL_SIGNS => {}
                _ => return false,
            }
        }
        if fit > DIL_FIT || (moved != 0 && next < 4.0 * fit) {
            return false;
        }
        self.lost = None;
        // Nothing moved: everything held is good. A move: only what it was
        // found from is sure to be past the slip, and the next pass has the
        // rest.
        let recent: Vec<(u64, f64)> = if moved == 0 { self.recent.drain(..).collect() } else { window };
        self.recent.clear();
        let next = recent.last().map_or(0, |r| r.0 as i64 + 1);
        if moved != 0 {
            self.moved += 1;
            self.base += moved;
            // The frames moved with it.
            let offset = (rx.frame_offset() as i64 - moved).rem_euclid(INTERVALS as i64) as u64;
            rx.set_frame_offset(offset);
        }
        let levels = self.levels(next, 2 * self.dil.len());
        rx.expect_from(next as u64, levels);
        for (raw, value) in recent {
            if self.count(raw, value) {
                return true;
            }
        }
        false
    }
}

/// Downstream data frames: TRN2d, MP, Ed, B1d and data.
///
/// The sequence finder is the caller's type parameter. V.90 reads MP and E
/// out of the descrambled bits with [`crate::v34::mp::Finder`]; V.92's phase 4
/// carries CPd, SUVd and Ed in the same place (V.92 8.8), so its analogue
/// modem plugs its own finder in and everything else here is unchanged.
#[derive(Debug, Clone)]
pub(crate) struct Frames<F = Finder> {
    pub(crate) from: u64,
    pub(crate) decoder: Decoder,
    pub(crate) levels: Levels,
    pub(crate) frame: [(u8, bool); INTERVALS],
    pub(crate) descrambler: Scrambler,
    pub(crate) finder: F,
    pub(crate) mp: Option<Mp>,
    pub(crate) far_acknowledged: bool,
    pub(crate) zero_frames: usize,
    pub(crate) ed: bool,
    pub(crate) b1d_left: usize,
    pub(crate) data: bool,
    /// The last symbols, as (index, value), and whether each of the last
    /// frames was one the digital modem could have sent.
    history: VecDeque<(u64, f64)>,
    impossible: VecDeque<bool>,
    /// Times the frames were found somewhere else.
    pub(crate) moved: u32,
    /// Data from frames that looked like Rd, kept back until the next frame
    /// shows whether Rd is what they were.
    pub(crate) held: VecDeque<Vec<bool>>,
}

impl<F: Default> Frames<F> {
    /// Frames from symbol `from` on, read with `mapping` against `levels`,
    /// with the scrambler, differential decoder and shaper started afresh.
    pub(crate) fn new(from: u64, mapping: Mapping, levels: Levels, moved: u32) -> Self {
        Self {
            from,
            decoder: Decoder::new(mapping),
            levels,
            frame: [(0, false); INTERVALS],
            descrambler: Scrambler::new(Mode::Call),
            finder: F::default(),
            mp: None,
            far_acknowledged: false,
            zero_frames: 0,
            ed: false,
            b1d_left: 0,
            data: false,
            history: VecDeque::with_capacity(PLACE_KEPT),
            impossible: VecDeque::with_capacity(PLACE_WINDOW),
            moved,
            held: VecDeque::new(),
        }
    }
}

impl<F> Frames<F> {
    /// One symbol, kept in case the place has to be found again.
    pub(crate) fn keep(&mut self, index: u64, value: f64) {
        if self.history.len() == PLACE_KEPT {
            self.history.pop_front();
        }
        self.history.push_back((index, value));
    }

    /// Whether the frame just read was one the digital modem could have sent.
    pub(crate) fn note(&mut self, impossible: bool) {
        if self.impossible.len() == PLACE_WINDOW {
            self.impossible.pop_front();
        }
        self.impossible.push_back(impossible);
    }

    /// Whether enough of the last frames were numbers nobody could have sent
    /// for the place to be worth looking for.
    pub(crate) fn lost_place(&self) -> bool {
        self.impossible.iter().filter(|x| **x).count() >= PLACE_LOST
    }

    /// Everything kept about where the frames are, thrown away: they have
    /// just been moved.
    pub(crate) fn forget_place(&mut self) {
        self.impossible.clear();
        self.history.clear();
    }
}

/// Where the frames are, as a shift from where they were taken to be: the one
/// under which the symbols kept make the fewest numbers the digital modem
/// could not have sent. None if that is where they already are.
pub(crate) fn find_place<F>(frames: &Frames<F>) -> Option<u64> {
    let mut best: Option<(usize, u64)> = None;
    for shift in 0..INTERVALS as u64 {
        let mut current = [(0u8, false); INTERVALS];
        let mut have = 0usize;
        let mut impossible = 0usize;
        for &(index, value) in &frames.history {
            let i = ((index + shift) % INTERVALS as u64) as usize;
            current[i] = nearest(&frames.levels[i], value);
            have = if i == 0 { 1 } else if have > 0 { have + 1 } else { 0 };
            if i == INTERVALS - 1 && have == INTERVALS {
                let frame = Frame {
                    ucodes: std::array::from_fn(|k| current[k].0),
                    positive: std::array::from_fn(|k| current[k].1),
                };
                if !frames.decoder.could_have_sent(&frame) {
                    impossible += 1;
                }
            }
        }
        if best.is_none_or(|(count, _)| impossible < count) {
            best = Some((impossible, shift));
        }
    }
    best.map(|(_, shift)| shift).filter(|&shift| shift != 0)
}

/// AD-1 assumes `datapump::v92` can name `carrier::Watch`. A re-export at
/// crate visibility is only legal if `mod carrier` is itself `pub(crate)`
/// (E0365), which is what makes this a compile-level check rather than a
/// reachability one: a unit test here is a descendant of `v90` and could name
/// a private module either way.
#[cfg(test)]
pub(crate) use super::carrier;

#[cfg(test)]
mod tests {
    use super::*;

    /// TRN1d's signs, `repeats` Jds and J'd, as the digital modem sends them
    /// (8.4.2, 8.4.3, 8.4.5), and then signs that mean nothing.
    fn phase3_signs(jd: &Jd, repeats: usize) -> (Vec<bool>, usize) {
        let mut scrambler = Scrambler::new(Mode::Call);
        let mut sign = false;
        let mut signs = Vec::new();
        for _ in 0..2400 {
            sign = scrambler.scramble(true);
            signs.push(sign);
        }
        for _ in 0..repeats {
            for bit in jd.to_bits() {
                sign ^= scrambler.scramble(bit);
                signs.push(sign);
            }
        }
        for _ in 0..JD_PRIME_BITS {
            sign ^= scrambler.scramble(false);
            signs.push(sign);
        }
        let end = signs.len();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..500 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            signs.push(x & 1 == 1);
        }
        (signs, end)
    }

    /// Where a reader says J'd ended, if it does.
    fn jd_prime_at(signs: &[bool]) -> Option<usize> {
        let mut reader = JdReader::new();
        signs.iter().enumerate().find_map(|(i, &s)| reader.feed(i as u64, s, |_| true).then_some(i + 1))
    }

    #[test]
    fn j_prime_is_read_after_whole_jds() {
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() };
        let (signs, end) = phase3_signs(&jd, 12);
        assert_eq!(jd_prime_at(&signs), Some(end));
    }

    /// A softphone that cut ten milliseconds out of the Jds, into the start
    /// of the last, leaves that one unreadable whole; J'd is still found
    /// after its tail. (A cut nearer J'd than the descrambler's memory is
    /// beyond reading at all, and the DIL is found from its own levels.)
    #[test]
    fn j_prime_is_read_after_a_jd_a_slip_cut_into() {
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() };
        let (mut signs, end) = phase3_signs(&jd, 12);
        let last = end - JD_PRIME_BITS - JD_BITS;
        signs.drain(last - 60..last + 20);
        assert_eq!(jd_prime_at(&signs), Some(end - 80));
    }

    /// V.92 puts a Jp straight after the Jds (8.6.3) and tells them apart by
    /// bit 47, the "Jd/Jp identifier", which is 0 in a Jd (Table 21/V.92) and
    /// 1 in a Jp. A closure over the raw framed bits stops such a frame
    /// before `Jd::from_bits` is called at all, so nothing is recorded;
    /// `|_| true`, which is what V.90 passes, reads the very same bits as a
    /// Jd full of rates.
    ///
    /// Both frames here carry a good CRC, so it is the closure and nothing
    /// else that refuses one of them. (Framed bit 47 is the second block's
    /// twelfth information bit, which in a V.90 Jd is `sixteen_in_training`;
    /// `to_bits` is asked here rather than counted, so the test says what a
    /// caller would see.)
    #[test]
    fn a_jd_reader_with_a_callers_acceptance_closure_ignores_frames_it_rejects() {
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() };
        let looks_like_jp = Jd { sixteen_in_training: true, ..jd };
        assert!(!jd.to_bits()[47] && looks_like_jp.to_bits()[47], "bit 47 is not what tells them apart");

        // Bit 47 clear: the closure is asked, says yes, and the Jd is read.
        let asked = std::cell::Cell::new(0usize);
        let accept = |bits: &[bool]| {
            asked.set(asked.get() + 1);
            !bits[47]
        };
        let (signs, _) = phase3_signs(&jd, 12);
        let mut reader = JdReader::new();
        for (i, &s) in signs.iter().enumerate() {
            reader.feed(i as u64, s, accept);
        }
        assert!(asked.get() > 0, "the closure was never asked");
        assert_eq!(reader.last().map(|(_, jd)| jd), Some(jd));

        // Bit 47 set: the closure refuses every frame, so nothing is read --
        // while the same signs with `|_| true` are read whole.
        let (signs, _) = phase3_signs(&looks_like_jp, 12);
        let (mut refusing, mut open) = (JdReader::new(), JdReader::new());
        for (i, &s) in signs.iter().enumerate() {
            refusing.feed(i as u64, s, |bits| !bits[47]);
            open.feed(i as u64, s, |_| true);
        }
        assert_eq!(refusing.last(), None, "a frame the closure refused was decoded anyway");
        assert_eq!(open.last().map(|(_, jd)| jd), Some(looks_like_jp));
    }

    /// Symbols of a repeating sign pattern at `level`, whose first symbol is
    /// at index `from`: the pattern's own phase, not the symbol count's.
    fn pattern(from: u64, n: usize, level: f64, signs: &[bool]) -> Vec<pcm::Symbol> {
        (0..n as u64)
            .map(|k| {
                let index = from + k;
                let positive = signs[(k as usize) % signs.len()];
                pcm::Symbol { index, raw: index, value: if positive { level } else { -level }, decided: None }
            })
            .collect()
    }

    /// 8.6.4: R is "+ + + - - -" repeated, and R-bar is the same six
    /// codewords with "- - - + + +". Eight frames make R believed, and the
    /// turn says TRN2d begins 24 symbols on.
    #[test]
    fn an_r_watch_hears_the_six_symbol_pattern_and_its_turn() {
        let mut watch = RWatch::new(INTERVALS, 0);
        let levels = [0.2; INTERVALS];
        let r = [true, true, true, false, false, false];
        for s in pattern(0, 8 * INTERVALS, 0.2, &r) {
            assert_eq!(watch.feed(&s, &levels), RSeen::Nothing);
        }
        assert!(watch.heard());
        let r_bar = [false, false, false, true, true, true];
        let mut turned = None;
        for s in pattern(8 * INTERVALS as u64, INTERVALS, 0.2, &r_bar) {
            if let RSeen::Turned(at) = watch.feed(&s, &levels) {
                turned = Some(at);
            }
        }
        // 8.6.4: R-bar is "4 repetitions of the 6-symbol sequence", so TRN2d
        // begins 24 symbols after R-bar's first. Counted out here -- 48 and
        // 24 -- rather than taken from the constant this is meant to hold.
        assert_eq!(turned, Some(72));
    }

    /// 8.8.4/V.92 transmits Rf by "repeating the 12-symbol sequence
    /// containing the PCM codewords with the sign pattern" it then prints as
    /// twelve signs: `+ + - -` three times over. So the watch has to take
    /// the period from its caller rather than assume 8.6.4's six.
    ///
    /// Both polarities, because "neither R nor R-bar are differentially
    /// encoded" (8.6.4 NOTE): the watch finds a rotation of the pattern in
    /// every frame of either, and reads as Rf itself the polarity the
    /// equaliser has settled the line to by phase 4 -- an inverted Rf is
    /// exactly R-bar-f, which is the same reading the six-symbol watch has
    /// always made of an inverted R.
    #[test]
    fn an_r_watch_finds_a_four_symbol_pattern_too() {
        let levels = [0.15; INTERVALS];
        let plus = [true, true, false, false];
        let minus = [false, false, true, true];
        for signs in [plus, minus] {
            let mut watch = RWatch::new(4, 0);
            for s in pattern(0, 8 * 4, 0.15, &signs) {
                assert_eq!(watch.feed(&s, &levels), RSeen::Nothing, "{signs:?}");
            }
            assert!(watch.looked(), "{signs:?} was not found at all");
            assert_eq!(watch.heard(), signs == plus, "{signs:?}");
        }
        // Rf heard, then its turn: 8.8.4/V.92 makes R-bar-f "2 repetitions of
        // the 12-symbol sequence", which is the 24T 9.9.1.1.1/V.92 prints, so
        // what follows begins 24 symbols after R-bar-f's first -- 32 and 24
        // counted out, not taken from the constant this is meant to hold.
        let mut watch = RWatch::new(4, 0);
        for s in pattern(0, 8 * 4, 0.15, &plus) {
            watch.feed(&s, &levels);
        }
        let mut turned = None;
        for s in pattern(32, 4, 0.15, &minus) {
            if let RSeen::Turned(at) = watch.feed(&s, &levels) {
                turned = Some(at);
            }
        }
        assert_eq!(turned, Some(56));
        // And a six-symbol watch reads the same stream as nothing at all.
        let mut six = RWatch::new(INTERVALS, 0);
        for s in pattern(0, 16 * INTERVALS, 0.15, &plus) {
            assert_eq!(six.feed(&s, &[0.15; INTERVALS]), RSeen::Nothing);
        }
        assert!(!six.looked() && !six.heard(), "a four-symbol pattern was read as R");
    }

    /// 9.9.1.1.1/V.92: "The signal Rf shall begin on the boundary of a data
    /// frame." A data frame is six symbols and Rf's signs repeat every four,
    /// so every other boundary starts Rf two symbols into its pattern -- and
    /// two of four is exactly the rotation that turns Rf into R-bar-f. A
    /// watch told where the pattern began hears it anyway; one left to read
    /// its phase off the symbol count finds that rotation in every group,
    /// refuses it as a turn it never heard the start of, and hears nothing
    /// at all.
    #[test]
    fn an_r_watch_hears_a_four_symbol_pattern_that_began_on_an_odd_frame() {
        let levels = [0.15; INTERVALS];
        let plus = [true, true, false, false];
        let minus = [false, false, true, true];
        // A data frame boundary two symbols into the four-symbol pattern.
        let start = INTERVALS as u64;
        let mut watch = RWatch::new(4, start);
        for s in pattern(start, 8 * 4, 0.15, &plus) {
            assert_eq!(watch.feed(&s, &levels), RSeen::Nothing);
        }
        assert!(watch.heard(), "an Rf beginning two symbols into its pattern was not heard");
        let mut turned = None;
        for s in pattern(start + 32, 4, 0.15, &minus) {
            if let RSeen::Turned(at) = watch.feed(&s, &levels) {
                turned = Some(at);
            }
        }
        // 6 symbols in, 32 of Rf, then R-bar-f's 24.
        assert_eq!(turned, Some(62));
        // The same symbols, to a watch told the pattern began where the
        // count did: every group is found, every group is the turn, and Rf
        // is never heard.
        let mut blind = RWatch::new(4, 0);
        for s in pattern(start, 8 * 4, 0.15, &plus) {
            assert_eq!(blind.feed(&s, &levels), RSeen::Nothing);
        }
        assert!(blind.looked(), "the rotated pattern was not found at all");
        assert!(!blind.heard(), "Rf was heard against an origin it does not have");
    }

    /// 8.8.4/V.92 gives Rf "the highest power PCM codeword from the data mode
    /// constellation of each data frame interval as passed in CPu" -- the
    /// words 8.6.4 gives Rd. So the codewords come round every six symbols
    /// while the signs come round every four, and the levels a watch judges
    /// by belong to the data frame whatever its pattern's period is.
    #[test]
    fn a_four_symbol_pattern_is_judged_against_the_data_frames_levels() {
        let levels: [f64; INTERVALS] = [0.05, 0.1, 0.15, 0.2, 0.25, 0.3];
        let plus = [true, true, false, false];
        let start = INTERVALS as u64;
        // Rf from `start`: its signs on the pattern, its levels on the frame,
        // with `shift` intervals of error in the levels.
        let rf = |shift: usize| -> Vec<pcm::Symbol> {
            (0..8 * 4u64)
                .map(|k| {
                    let index = start + k;
                    let level = levels[(index as usize + shift) % INTERVALS];
                    let value = if plus[k as usize % plus.len()] { level } else { -level };
                    pcm::Symbol { index, raw: index, value, decided: None }
                })
                .collect()
        };
        let mut watch = RWatch::new(4, start);
        for s in rf(0) {
            assert_eq!(watch.feed(&s, &levels), RSeen::Nothing);
        }
        assert!(watch.heard(), "Rf at the data frame's own levels was not heard");
        // The same signs, every level one interval round: not Rf.
        let mut wrong = RWatch::new(4, start);
        for s in rf(1) {
            assert_eq!(wrong.feed(&s, &levels), RSeen::Nothing);
        }
        assert!(!wrong.heard(), "Rf was heard with its levels in the wrong intervals");
    }

    /// The levels are the caller's: R is "PCM codewords" the CP named
    /// (8.6.4), which means Ri's UINFO, Rd's loudest data codeword or Rt's
    /// loudest training one. The watch believes a frame only within half to
    /// one and a half of what it was told, so the same symbols are R against
    /// one set of levels and nothing against another.
    #[test]
    fn an_r_watch_takes_its_levels_from_the_caller() {
        let r = [true, true, true, false, false, false];
        let symbols = pattern(0, 8 * INTERVALS, 0.2, &r);
        let mut told = RWatch::new(INTERVALS, 0);
        for s in &symbols {
            assert_eq!(told.feed(s, &[0.2; INTERVALS]), RSeen::Nothing);
        }
        assert!(told.heard());
        // Three times too loud a level, and the same symbols say nothing.
        let mut wrong = RWatch::new(INTERVALS, 0);
        for s in &symbols {
            assert_eq!(wrong.feed(s, &[0.6; INTERVALS]), RSeen::Nothing);
        }
        assert!(!wrong.heard(), "R was heard against levels it does not have");
        // And a level per interval, as Rd has: the watch tests each on its
        // own, so one interval told wrong is enough to refuse the frame.
        let mut mixed = RWatch::new(INTERVALS, 0);
        let mut levels = [0.2; INTERVALS];
        levels[4] = 0.02;
        for s in &symbols {
            assert_eq!(mixed.feed(s, &levels), RSeen::Nothing);
        }
        assert!(!mixed.heard(), "one interval's level was not looked at");
    }

    /// AD-1: `datapump::v92` reuses the V.90 carrier watch, so `mod carrier`
    /// has to be `pub(crate)`. The re-export above is what proves it; this
    /// builds one through that name so the import is used.
    #[test]
    fn the_carrier_watch_can_be_named_from_outside_v90() {
        let watch: carrier::Watch = carrier::Watch::new(8000.0);
        assert!(!watch.gone());
    }
}
