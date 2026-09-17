//! The analogue modem from phase 3 on (9.3.2, 9.4.2): V.34 going up, PCM
//! coming down.
//!
//! ```text
//! analogue S S' PP TRN Ja ...          (quiet)       S ... S S'  (quiet)   S S' CPt ... CP CP' E B1 data
//! digital                 Sd S'd TRN1d Jd ... Jd J'd DIL ... ... DIL Ri ... R'i TRN2d MP MP' Ed B1d data
//! ```
//!
//! A rate renegotiation (9.6) goes back to phase 4 from data mode, the
//! frames kept in step throughout: Rd and its turn to R-bar-d from the
//! digital modem, S, S-bar and CP from this end, and phase 4 from there.
//!
//! Everything upstream is V.34's, at the symbol rate, carrier and
//! pre-emphasis phase 2 settled: S, S-bar and PP as phase 3 sends them, TRN
//! at four points, Ja in J's modulation (8.3.1), CP, SCR and E as MP goes
//! (8.5.2), and then V.34's data mode as the digital modem's MP asks. What
//! comes down is read by [`super::pcm`], and what it means is worked out
//! here: Jd and J'd from the signs, the route from the DIL, R from its sign
//! pattern, and TRN2d, MP, Ed, B1d and data from whole data frames.

use std::collections::VecDeque;

use dsp::Complex;

use crate::v32::{Mode, Scrambler};
use crate::v34::constellation::Point;
use crate::v34::data::{Encoder as UpstreamEncoder, Params};
use crate::v34::frame::Framing;
use crate::v34::info::{Info0d, Info1aPcm, Info1c};
use crate::v34::mp::{Finder, Found, Mp, Trellis};
use crate::v34::qam::{Band, Transmitter};
use crate::v34::receiver;
use crate::v34::phase2::Role;
use crate::v34::signals::{self, Sender, Size};
use crate::v34::training::RetrainWatch;
use crate::v34::trellis::Code;

use super::INTERVALS;
use super::dil::{self, Analysis, Choice, Route};
use super::digital;
use super::encoder::{Decoder, Frame, Mapping};
use super::pcm::{self, Heard, Slicer};
use super::sequences::{self, Cp, Descriptor, JD_BITS, JD_PRIME_BITS, Jd};
use super::ucode::{self, Law};

/// TRN in phase 3: "at least 512T" (9.3.2.3), and a far receiver trains
/// better on more, as V.34's own phase 3 has found.
const PHASE3_TRN: f64 = 1.0;

/// "70 +- 5 ms" of silence after INFO1a (9.3.2.1).
const SILENCE_BEFORE_S: f64 = 0.070;

/// Sd: "within 1500 ms from the start of Ja" (9.3.2.4), which a digital
/// modem that "may wait for up to 500 ms" after reading Ja (9.3.1.3) cannot
/// meet across a VoIP call's second-long round trip. A live server's came
/// two seconds after Ja began; waiting is cheaper than retraining.
const SD_WAIT: f64 = 2.0;

/// Jd: "Within 4000 ms of starting to transmit TRN1d the digital modem shall
/// transmit Jd" (9.3.1.4), and S is wanted back "within 5100 ms plus a
/// round-trip delay from the start of TRN1d" (9.3.1.5). A live server sent
/// Jd at the last moment and gave up on S a second later, round trip or
/// none: one that takes a second to cross could not answer Jd in time. So
/// S goes before Jd arrives, to reach the digital modem this long after the
/// latest it can have begun Jd -- S goes on until J'd, and a digital modem
/// not yet listening for it hears it when it is.
const JD_LATEST: f64 = 4.0;
const S_AFTER_JD: f64 = 0.1;

/// Frames of R in a row before it is believed, and of R a whole number of
/// symbols out of step before the frames are taken to have moved.
const R_HEARD: usize = 8;
const R_MOVED: usize = 6;

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

/// Finding the DIL when J'd went unread: how long after the last Jd before
/// looking, how often, how much is kept to look in, and how much of it a
/// start is judged on.
const JD_GONE: u64 = 3 * JD_BITS as u64;
const DIL_START_KEPT: usize = 1200;
const DIL_START_WINDOW: usize = 480;

/// B1d: "48 data frames" (8.6.1).
const B1D_FRAMES: usize = 48;

/// A renegotiation's Ed: "within 5000 ms plus 2 round-trip delays after
/// sending the S-bar-to-S transition" (9.6.2).
const RENEGOTIATION_ED: f64 = 5.0;

/// Whole CPs asking for nothing sent before a cleardown is over.
const CLEARDOWN_CPS: usize = 4;

/// Data mode's levels closer than this many of the receiver's RMS errors
/// make errors often enough to be worth a slower rate: a frame in some
/// hundreds at six, none in a quarter of a million bits at eight.
const MARGIN: f64 = 7.0;

/// How often the margin is looked at, how many looks in a row it must be
/// short for, and how long data mode runs before the first.
const MARGIN_EVERY: f64 = 0.25;
const MARGIN_SHORT: u32 = 4;
const MARGIN_SETTLE: f64 = 2.0;

/// Frames over which a read of impossible numbers is counted, how many make
/// it a lost place, and symbols kept for finding the place again.
const PLACE_WINDOW: usize = 24;
const PLACE_LOST: usize = 3;
const PLACE_KEPT: usize = 48 * INTERVALS;

/// How phases 3 and 4 are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// In data mode: downstream and upstream rates in bit/s.
    Connected { downstream: u32, upstream: u32 },
    /// One end or the other asked for a rate of nothing (9.7).
    ClearedDown,
    Failed(&'static str),
}

/// What phase 2 settled, as the analogue modem needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub law: Law,
    pub uinfo: u8,
    pub server: Info0d,
    /// This end's transmitter, as INFO1a chose it and INFO1d set it up.
    pub upstream: Band,
    pub pre_emphasis: u8,
    pub power_reduction: u8,
    pub round_trip: f64,
    /// Whether both ends have the 1664-point constellation the upstream's
    /// top rates need.
    pub wide: bool,
    /// What V.34 would carry downstream instead, as phase 2's probe put it:
    /// a V.90 slower than that is not worth having.
    pub v34_receive: u32,
}

impl Settings {
    /// From the digital modem's INFO0d and INFO1d, and this end's INFO1a.
    pub fn new(server: &Info0d, info1d: &Info1c, asked: &Info1aPcm, round_trip: f64, ours_wide: bool) -> Self {
        let probed = info1d.probed[asked.upstream.index() as usize];
        Self {
            law: if server.a_law { Law::A } else { Law::Mu },
            uinfo: asked.uinfo,
            server: *server,
            upstream: Band::new(asked.upstream, probed.high_carrier),
            pre_emphasis: probed.pre_emphasis,
            power_reduction: info1d.min_power_reduction,
            round_trip,
            wide: ours_wide && server.v34.constellation_1664,
            v34_receive: 0,
        }
    }
}

/// What goes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Up {
    Silence,
    S,
    SBar,
    Pp,
    Trn,
    Ja,
    Cp,
    E,
    Data,
}

fn grid(point: Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(receiver::unit(size))
}

/// The upstream, one symbol at a time.
#[derive(Debug, Clone)]
struct Source {
    sender: Sender,
    up: Up,
    count: usize,
    /// How long S runs: to a count, or until changed.
    s_length: Option<usize>,
    after_s_bar: Up,
    trn_length: usize,
    pending: Option<Up>,
    queue: VecDeque<bool>,
    ja: Vec<bool>,
    cp: Vec<bool>,
    cp_is_ack: bool,
    next_cp: Option<(Vec<bool>, bool)>,
    /// Whole CP sequences sent, and whole CP' sequences.
    cps: usize,
    acknowledged: usize,
    restarted: bool,
    size: Size,
    encoder: Option<UpstreamEncoder>,
    data: VecDeque<bool>,
    hold: usize,
    silent: usize,
}

impl Source {
    fn new(trn_length: usize) -> Self {
        Self {
            // The analogue modem scrambles with GPA (8.3).
            sender: Sender::new(Mode::Answer),
            up: Up::Silence,
            count: 0,
            s_length: Some(signals::S_SYMBOLS),
            after_s_bar: Up::Pp,
            trn_length,
            pending: None,
            queue: VecDeque::new(),
            ja: Vec::new(),
            cp: Vec::new(),
            cp_is_ack: false,
            next_cp: None,
            cps: 0,
            acknowledged: 0,
            restarted: false,
            size: Size::Four,
            encoder: None,
            data: VecDeque::new(),
            hold: 0,
            silent: 0,
        }
    }

    fn start(&mut self, up: Up) {
        self.up = up;
        self.count = 0;
        self.silent = 0;
        self.queue.clear();
        match up {
            Up::Trn => self.sender.restart(),
            // "The scrambler and differential encoder are initialized to zero
            // prior to the transmission of the first CPt sequence" (8.5.2).
            Up::Cp if !self.restarted => {
                self.restarted = true;
                self.sender.restart();
            }
            Up::E => self.queue.extend(std::iter::repeat_n(true, signals::E_BITS)),
            _ => {}
        }
    }

    fn change(&mut self, up: Up) {
        self.pending = Some(up);
    }

    fn differential(&mut self, size: Size) -> Complex {
        let bits: Vec<bool> = (0..size.bits()).map(|_| self.queue.pop_front().unwrap_or(true)).collect();
        self.count += 1;
        grid(self.sender.differential(&bits), size)
    }

    fn next(&mut self) -> Complex {
        loop {
            match self.up {
                Up::Silence => {
                    if self.silent >= self.hold
                        && let Some(next) = self.pending.take()
                    {
                        self.hold = 0;
                        self.start(next);
                        continue;
                    }
                    self.silent += 1;
                    return Complex::ZERO;
                }
                Up::S => {
                    let done = match self.s_length {
                        Some(n) => self.count >= n,
                        // Until something else is asked for, and then at the
                        // end of a pair, so S-bar follows in step.
                        None => self.pending.is_some() && self.count.is_multiple_of(2),
                    };
                    if done {
                        let next = if self.s_length.is_some() { Up::SBar } else { self.pending.take().unwrap_or(Up::SBar) };
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s(self.count - 1), Size::Four);
                }
                Up::SBar => {
                    if self.count == signals::S_BAR_SYMBOLS {
                        let next = self.after_s_bar;
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s_bar(self.count - 1), Size::Four);
                }
                Up::Pp => {
                    if self.count == signals::PP_SYMBOLS {
                        self.start(Up::Trn);
                        continue;
                    }
                    self.count += 1;
                    return signals::pp(self.count - 1).into();
                }
                Up::Trn => {
                    if self.count >= self.trn_length {
                        self.start(Up::Ja);
                        continue;
                    }
                    self.count += 1;
                    return grid(self.sender.trn(Size::Four), Size::Four);
                }
                Up::Ja => {
                    // "Transmission of sequence Ja may be terminated without
                    // completing the final DIL descriptor" (8.3.1).
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    if self.queue.is_empty() {
                        self.queue.extend(self.ja.iter().copied());
                    }
                    return self.differential(Size::Four);
                }
                Up::Cp => {
                    if self.queue.is_empty() {
                        if self.count > 0 {
                            self.cps += 1;
                            if self.cp_is_ack {
                                self.acknowledged += 1;
                            }
                        }
                        if let Some(next) = self.pending.take() {
                            self.start(next);
                            continue;
                        }
                        if let Some((cp, ack)) = self.next_cp.take() {
                            self.cp = cp;
                            self.cp_is_ack = ack;
                        }
                        self.queue.extend(self.cp.iter().copied());
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::E => {
                    if self.queue.is_empty() {
                        let next = if self.encoder.is_some() { Up::Data } else { Up::Silence };
                        self.start(next);
                        continue;
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::Data => {
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Up::Silence);
                        continue;
                    };
                    // B1 is one data frame of scrambled ones (8.5.1, and
                    // 10.1.3.1/V.34).
                    let b1 = encoder.mapping_frames() < encoder.params().framing.p as u64;
                    let data = &mut self.data;
                    self.count += 1;
                    return encoder.next_symbol(&mut || if b1 { true } else { data.pop_front().unwrap_or(true) });
                }
            }
        }
    }
}

/// Reads Jd and J'd off the signs (8.4.2, 8.4.3).
#[derive(Debug, Clone)]
struct JdReader {
    descrambler: Scrambler,
    differential: bool,
    previous: bool,
    bits: VecDeque<bool>,
    /// The last Jd read whole, and the symbol after it.
    last: Option<(u64, Jd)>,
    /// Its bits, as they went.
    jd_bits: Vec<bool>,
}

/// The end of a Jd that J'd is found after: its CRC and fill.
const JD_TAIL: usize = 24;

impl JdReader {
    fn new() -> Self {
        Self {
            descrambler: Scrambler::new(Mode::Call),
            differential: false,
            previous: false,
            bits: VecDeque::new(),
            last: None,
            jd_bits: Vec::new(),
        }
    }

    /// One symbol's sign. True when this symbol ended a J'd.
    fn feed(&mut self, index: u64, positive: bool) -> bool {
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
/// same frame over and over, shows by how many. R-bar is R moved by three,
/// which no slip of whole milliseconds does.
#[derive(Debug, Clone, Default)]
struct RWatch {
    frame: [f64; INTERVALS],
    /// Frames in a row of R moved by `moved` symbols.
    run: usize,
    moved: usize,
    heard: bool,
    /// Whether the last whole frame looked like R or R-bar, however moved.
    looked: bool,
}

/// What the watch made of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RSeen {
    Nothing,
    /// R has turned into R-bar: TRN2d begins at this symbol.
    Turned(u64),
    /// R is arriving this many symbols late: the frames have moved.
    Moved(usize),
}

impl RWatch {
    /// One symbol, and the level R has in each interval.
    fn feed(&mut self, symbol: &pcm::Symbol, levels: &[f64; INTERVALS]) -> RSeen {
        let i = symbol.interval();
        self.frame[i] = symbol.value;
        if i != INTERVALS - 1 {
            return RSeen::Nothing;
        }
        // R moved by m: "+ + + - - -" starting m symbols in.
        let late = (0..INTERVALS).find(|&m| {
            (0..INTERVALS).all(|j| {
                let k = (j + INTERVALS - m) % INTERVALS;
                let v = self.frame[j];
                (v >= 0.0) == (k < 3) && (0.5 * levels[k]..1.5 * levels[k]).contains(&v.abs())
            })
        });
        self.looked = late.is_some();
        match late {
            Some(0) => {
                self.run = if self.moved == 0 { self.run + 1 } else { 1 };
                self.moved = 0;
                if self.run >= R_HEARD {
                    self.heard = true;
                }
                RSeen::Nothing
            }
            Some(3) if self.heard => {
                // R-bar's first frame. TRN2d starts after the other three.
                let frame_start = symbol.index + 1 - INTERVALS as u64;
                RSeen::Turned(frame_start + 4 * INTERVALS as u64)
            }
            Some(m) if m != 3 => {
                self.run = if self.moved == m { self.run + 1 } else { 1 };
                self.moved = m;
                if self.run < R_MOVED {
                    return RSeen::Nothing;
                }
                self.run = 0;
                self.moved = 0;
                RSeen::Moved(m)
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
type Levels = [Vec<(f64, u8, bool)>; INTERVALS];

fn levels_for(cp: &Cp, route: &Route) -> Levels {
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
fn least_gap(cp: &Cp, route: &Route) -> f64 {
    (0..INTERVALS)
        .map(|i| {
            let mut levels: Vec<f64> =
                cp.points(i).iter().flat_map(|&u| [route.levels[i][usize::from(u)], -route.levels[i][usize::from(u)]]).collect();
            levels.sort_by(f64::total_cmp);
            levels.windows(2).map(|w| w[1] - w[0]).fold(f64::INFINITY, f64::min)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Which of a DIL's symbols can be learned from and judged by (see
/// `Modem::dil_trusted`).
fn trusted_symbols(descriptor: &Descriptor, law: Law) -> Vec<Trust> {
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

/// What a DIL symbol can be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    /// Learned from, judged by, and counted.
    Yes,
    /// A codeword too loud to trust: only counted, so that the route shows
    /// what happened to it.
    Loud,
    /// The start of a segment a loud codeword spilled into: none of those.
    Spoiled,
}

/// The nearest of an interval's levels to `value`, as (Ucode, positive).
fn nearest(levels: &[(f64, u8, bool)], value: f64) -> (u8, bool) {
    levels
        .iter()
        .min_by(|a, b| (a.0 - value).abs().total_cmp(&(b.0 - value).abs()))
        .map_or((0, false), |l| (l.1, l.2))
}

/// Where the frames are, as a shift from where they were taken to be: the one
/// under which the symbols kept make the fewest numbers the digital modem
/// could not have sent. None if that is where they already are.
fn find_place(frames: &Frames) -> Option<u64> {
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

fn slicer_for(levels: &Levels) -> Slicer {
    Slicer::Levels(Box::new(std::array::from_fn(|i| levels[i].iter().map(|l| l.0).collect())))
}

/// Downstream data frames: TRN2d, MP, Ed, B1d and data.
#[derive(Debug, Clone)]
struct Frames {
    from: u64,
    decoder: Decoder,
    levels: Levels,
    frame: [(u8, bool); INTERVALS],
    descrambler: Scrambler,
    finder: Finder,
    mp: Option<Mp>,
    far_acknowledged: bool,
    zero_frames: usize,
    ed: bool,
    b1d_left: usize,
    data: bool,
    /// The last symbols, as (index, value), and whether each of the last
    /// frames was one the digital modem could have sent.
    history: VecDeque<(u64, f64)>,
    impossible: VecDeque<bool>,
    /// Times the frames were found somewhere else.
    moved: u32,
    /// Data from frames that looked like Rd, kept back until the next frame
    /// shows whether Rd is what they were.
    held: VecDeque<Vec<bool>>,
}

impl Frames {
    /// Frames from symbol `from` on, read with `mapping` against `levels`,
    /// with the scrambler, differential decoder and shaper started afresh.
    fn new(from: u64, mapping: Mapping, levels: Levels, moved: u32) -> Self {
        Self {
            from,
            decoder: Decoder::new(mapping),
            levels,
            frame: [(0, false); INTERVALS],
            descrambler: Scrambler::new(Mode::Call),
            finder: Finder::new(),
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

/// Where the analogue modem has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    SendTraining,
    AwaitSd,
    Training,
    AwaitJd,
    AwaitJdPrime,
    Dil,
    Phase4,
    Data,
    Finished,
}

/// The analogue modem, phase 3 on.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    fs: f64,
    now: u64,
    stage: Stage,
    status: Status,
    deadline: Option<(u64, &'static str)>,
    /// B1d "within 15 s plus 5 round-trip delays after sending INFO1a".
    start_deadline: (u64, &'static str),
    /// When Sd is to have come by.
    sd_deadline: (u64, &'static str),
    /// Whether S is going out ahead of Jd.
    sending_s: bool,
    tx: Transmitter,
    source: Source,
    rx: pcm::Receiver,
    descriptor: Descriptor,
    jd: JdReader,
    far_jd: Option<Jd>,
    dil: Vec<(u8, bool)>,
    /// Whether each DIL symbol can be learned from and judged by: not in a
    /// segment of a codeword too loud to trust, and not at the start of the
    /// segment after one, which what the loud one did spills into.
    dil_trusted: Vec<Trust>,
    /// The DIL as it is read (9.3.2.9): the receiver's count at its first
    /// symbol, moved by any slip since; the frame interval that symbol was
    /// in; which symbols have been read, and how many are left.
    dil_base: i64,
    dil_interval: usize,
    dil_read: Vec<bool>,
    dil_left: usize,
    /// Symbols not yet counted, and since a slip was noticed, how many have
    /// been gathered to find where the DIL went.
    dil_recent: VecDeque<(u64, f64)>,
    dil_lost: Option<usize>,
    dil_moved: u32,
    /// Symbols while J'd is awaited, for finding the DIL if J'd goes unread,
    /// and whether it was found that way.
    before_dil: VecDeque<(u64, f64)>,
    dil_found_late: bool,
    jd_gone: bool,
    /// Times R showed the frames had moved.
    r_moved: u32,
    analysis: Analysis,
    route: Option<Route>,
    choice: Option<Choice>,
    r_watch: RWatch,
    trn2d_from: Option<u64>,
    frames: Option<Frames>,
    received: Vec<bool>,
    upstream_rate: u32,
    downstream_rate: u32,
    /// The last two symbols as the equaliser gave them, for the scope.
    last: [f64; 2],
    /// The last symbol whole, for anything reading a call back.
    last_symbol: Option<pcm::Symbol>,
    heard_any: bool,
    /// The digital modem's tone B, which starts a retrain (9.5.2.2), and
    /// whether one is wanted.
    retrain_watch: RetrainWatch,
    wants_retrain: bool,
    /// Since the receiver last held a place, in samples.
    lost_since: Option<u64>,
    /// The CP data mode is running on, which Rd and a renegotiation's
    /// shaping are taken from.
    in_use: Option<Cp>,
    /// Rd and its turn, in data mode and a renegotiation (9.6.2).
    rd_watch: RWatch,
    renegotiating: bool,
    /// Whether this end began the renegotiation, and whether R-bar-d is
    /// still to come in it.
    initiated: bool,
    awaiting_turn: bool,
    clearing: bool,
    renegotiations: u32,
    /// The least distance between data mode's levels as the route delivers
    /// them, when the margin is next looked at, and looks in a row it was
    /// short.
    least_gap: f64,
    margin_at: u64,
    short: u32,
    /// How much worse than the DIL showed data mode has found the line.
    worse: f64,
}

impl Modem {
    /// Phase 3 from its start: the moment INFO1a has gone.
    pub fn new(settings: Settings, fs: f64) -> Self {
        let trn_length = (PHASE3_TRN * settings.upstream.baud()) as usize;
        let mut source = Source::new(trn_length);
        let silence = (SILENCE_BEFORE_S * settings.upstream.baud()).round() as usize;
        source.hold = silence.saturating_sub(Transmitter::lookahead());
        source.after_s_bar = Up::Pp;
        source.change(Up::S);
        let descriptor = dil::design(settings.law, settings.uinfo);
        source.ja = descriptor.to_bits();
        // Nothing to hunt for until Ja: before then, the digital modem's
        // tone from phase 2 can still be arriving, and a tone near 1333 Hz
        // looks enough like Sd to set a hunt off.
        let rx = pcm::Receiver::new(settings.law, fs);
        let mut modem = Self {
            settings,
            fs,
            now: 0,
            stage: Stage::SendTraining,
            status: Status::Running,
            deadline: None,
            start_deadline: (0, ""),
            sd_deadline: (u64::MAX, ""),
            sending_s: false,
            tx: Transmitter::new(settings.upstream, settings.pre_emphasis, settings.power_reduction, fs),
            source,
            rx,
            dil: descriptor.symbols().collect(),
            dil_trusted: trusted_symbols(&descriptor, settings.law),
            descriptor,
            jd: JdReader::new(),
            far_jd: None,
            dil_base: 0,
            dil_interval: 0,
            dil_read: Vec::new(),
            dil_left: 0,
            dil_recent: VecDeque::new(),
            dil_lost: None,
            dil_moved: 0,
            before_dil: VecDeque::new(),
            dil_found_late: false,
            jd_gone: false,
            r_moved: 0,
            analysis: Analysis::new(),
            route: None,
            choice: None,
            r_watch: RWatch::default(),
            trn2d_from: None,
            frames: None,
            received: Vec::new(),
            upstream_rate: 0,
            downstream_rate: 0,
            last: [0.0; 2],
            last_symbol: None,
            heard_any: false,
            // The digital modem takes V.34's call side, and tone B is its.
            retrain_watch: RetrainWatch::new(Role::Call, fs),
            wants_retrain: false,
            lost_since: None,
            in_use: None,
            rd_watch: RWatch::default(),
            renegotiating: false,
            initiated: false,
            awaiting_turn: false,
            clearing: false,
            renegotiations: 0,
            least_gap: f64::INFINITY,
            margin_at: 0,
            short: 0,
            worse: 1.0,
        };
        // 9.4.2: B1d "within 15 s plus 5 round-trip delays after sending
        // INFO1a".
        modem.dil_left = modem.dil.len();
        modem.start_deadline = (modem.samples(15.0 + 5.0 * settings.round_trip), "no B1d from the digital modem");
        modem.deadline = Some(modem.start_deadline);
        modem
    }

    fn samples(&self, seconds: f64) -> u64 {
        self.now + (seconds * self.fs).round() as u64
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::SendTraining | Stage::AwaitSd | Stage::Training => "V.90 phase 3: training",
            Stage::AwaitJd | Stage::AwaitJdPrime => "V.90 phase 3: Jd",
            Stage::Dil => "V.90 phase 3: DIL",
            Stage::Phase4 => "V.90 phase 4",
            Stage::Data if self.renegotiating => "V.90 rate renegotiation",
            Stage::Data => "V.90 data",
            Stage::Finished => "V.90 finished",
        }
    }

    /// The downstream receiver, for looking at.
    pub fn receiver(&self) -> &pcm::Receiver {
        &self.rx
    }

    /// The last two downstream symbols, each against the one after it, for a
    /// scope: there is no plane to plot PCM in, but a sample set against the
    /// next one lays the levels out on a grid of its own. Scaled so the
    /// loudest level in use is one.
    pub fn pair(&self) -> Option<(f64, f64)> {
        self.heard_any.then(|| (self.last[0] / self.scale(), self.last[1] / self.scale()))
    }

    /// What the scope's one is.
    fn scale(&self) -> f64 {
        let law = self.settings.law;
        let loudest = |cp: &Cp| (0..INTERVALS).flat_map(|i| cp.points(i)).map(|u| ucode::level(law, u)).fold(0.0, f64::max);
        match (self.frames.as_ref(), self.choice.as_ref()) {
            (Some(f), Some(c)) if f.ed => loudest(&c.data),
            (Some(_), Some(c)) => loudest(&c.training),
            _ => 1.5 * ucode::level(law, self.settings.uinfo),
        }
        .max(1e-6)
    }

    /// Signed levels in the constellation being read, for a scope's legend.
    pub fn points(&self) -> usize {
        match (self.frames.as_ref(), self.choice.as_ref()) {
            (Some(f), Some(c)) if f.ed => 2 * c.data.points(0).len(),
            (Some(_), Some(c)) => 2 * c.training.points(0).len(),
            _ => 2,
        }
    }

    /// The last downstream symbol the receiver gave, and which stage of the
    /// start-up read it: for reading a call back.
    pub fn last_symbol(&self) -> Option<(pcm::Symbol, &'static str)> {
        self.last_symbol.map(|s| (s, self.phase()))
    }

    /// Where the DIL's first symbol is, as the receiver counts, and the
    /// frame interval it is in.
    pub fn dil_start(&self) -> (i64, usize) {
        (self.dil_base, self.dil_interval)
    }

    /// The DIL this end asked for.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// The digital modem's Jd.
    pub fn far_jd(&self) -> Option<Jd> {
        self.far_jd
    }

    /// What the DIL showed of the route.
    pub fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// What this end asked for.
    pub fn choice(&self) -> Option<&Choice> {
        self.choice.as_ref()
    }

    /// The digital modem's MP.
    pub fn far_mp(&self) -> Option<Mp> {
        self.frames.as_ref().and_then(|f| f.mp)
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    /// Whether V.90's phase 2 should be run again: read once, and cleared.
    pub fn take_retrain(&mut self) -> bool {
        std::mem::take(&mut self.wants_retrain)
    }

    /// Start a retrain (9.5.2.1).
    pub fn start_retrain(&mut self) {
        self.wants_retrain = true;
    }

    /// Rate renegotiations and cleardowns since the call began, from either
    /// end.
    pub fn renegotiations(&self) -> u32 {
        self.renegotiations
    }

    /// Start a rate renegotiation from data mode (9.6.2.1), asking for the
    /// fastest downstream the route carries at no more than `most` bit/s.
    /// False, and nothing done, outside data mode or if the route carries no
    /// rate that slow.
    pub fn renegotiate(&mut self, most: u32) -> bool {
        if !self.in_data_mode() {
            return false;
        }
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return false };
        let law = self.settings.law;
        let limit = super::power_limit(&self.settings.server);
        let jd = self.far_jd.unwrap_or_default();
        let slow_enough = |drn: u8| jd.enables(drn) && sequences::data_rate(drn).is_some_and(|rate| rate <= most);
        let mut route = route.clone();
        for spread in route.spread.iter_mut() {
            *spread *= self.worse;
        }
        let Some(new) = dil::choose(&route, law, limit, slow_enough) else { return false };
        let mut data = new.data;
        self.finish_cp(&mut data);
        let training = choice.training.clone();
        self.choice = Some(Choice { data, training });
        self.begin_renegotiation(true);
        true
    }

    /// End the call from data mode (9.7): a renegotiation whose CP asks for
    /// nothing. False, and nothing done, outside data mode.
    pub fn clear_down(&mut self) -> bool {
        if !self.in_data_mode() {
            return false;
        }
        let Some(choice) = self.choice.as_mut() else { return false };
        choice.data.drn = 0;
        self.clearing = true;
        self.begin_renegotiation(true);
        true
    }

    fn in_data_mode(&self) -> bool {
        matches!(self.status, Status::Connected { .. }) && !self.renegotiating
    }

    /// Back from data mode to phase 4 (9.6.2.1.1, 9.6.2.2.1).
    fn begin_renegotiation(&mut self, initiating: bool) {
        self.renegotiations += 1;
        self.renegotiating = true;
        self.initiated = initiating;
        self.awaiting_turn = true;
        self.status = Status::Running;
        if initiating {
            // The digital modem's data is data until its Rd.
            self.rd_watch = RWatch::default();
            self.send_s_then_cp();
        } else {
            self.clamp();
        }
        let wait = RENEGOTIATION_ED + 2.0 * self.settings.round_trip + 0.1;
        self.deadline = Some((self.samples(wait), "no Ed in the rate renegotiation"));
    }

    /// Rd: circuit 104 clamped, and what was held back for it dropped.
    fn clamp(&mut self) {
        if let Some(frames) = self.frames.as_mut() {
            frames.data = false;
            frames.held.clear();
        }
    }

    /// S for 128T, S-bar for 16T, and CP (9.6.2.1.1 to 9.6.2.1.3).
    fn send_s_then_cp(&mut self) {
        let Some(choice) = self.choice.as_ref() else { return };
        let sixteen = self.far_jd.is_some_and(|jd| jd.sixteen_in_renegotiation);
        let source = &mut self.source;
        source.s_length = Some(signals::S_SYMBOLS);
        source.after_s_bar = Up::Cp;
        source.cp = choice.data.to_bits();
        source.cp_is_ack = false;
        source.next_cp = None;
        source.cps = 0;
        source.acknowledged = 0;
        source.restarted = false;
        source.size = if sixteen { Size::Sixteen } else { Size::Four };
        source.encoder = None;
        source.change(Up::S);
    }

    /// R-bar-d has begun: TRN2d, MP and Ed start at `from`, on CPt's
    /// constellations with data mode's shaping (8.6).
    fn turned(&mut self, from: u64) {
        self.awaiting_turn = false;
        let (Some(route), Some(choice), Some(in_use)) = (self.route.as_ref(), self.choice.as_ref(), self.in_use.as_ref()) else {
            return;
        };
        let Some(mapping) = Mapping::for_renegotiation(&choice.training, in_use) else {
            self.fail("the renegotiation has no mapping to train on");
            return;
        };
        let levels = levels_for(&choice.training, route);
        let moved = self.frames.as_ref().map_or(0, |f| f.moved);
        self.frames = Some(Frames::new(from, mapping, levels, moved));
        if !self.initiated {
            // 9.6.2.2.2: "transmit S for 128T".
            self.send_s_then_cp();
        }
    }

    /// R has shown the frames arriving `late` symbols later than they were
    /// taken to.
    fn move_frames(&mut self, late: usize) {
        let offset = (self.rx.frame_offset() + (INTERVALS - late) as u64) % INTERVALS as u64;
        self.rx.set_frame_offset(offset);
        self.r_moved += 1;
        if let Some(frames) = self.frames.as_mut() {
            frames.impossible.clear();
            frames.history.clear();
        }
    }

    /// Rd's level in each interval: "the highest power PCM codeword from the
    /// data mode constellation" (8.6.4), as the route delivers it.
    fn rd_levels(&self) -> [f64; INTERVALS] {
        let (Some(cp), Some(route)) = (self.in_use.as_ref(), self.route.as_ref()) else {
            return [f64::INFINITY; INTERVALS];
        };
        std::array::from_fn(|i| cp.points(i).last().map_or(f64::INFINITY, |&u| route.levels[i][usize::from(u)]))
    }

    /// One end has asked for nothing: the call is over (9.7).
    fn cleared_down(&mut self) {
        self.status = Status::ClearedDown;
        self.stage = Stage::Finished;
        self.renegotiating = false;
        self.source.pending = None;
        self.source.start(Up::Silence);
        self.rx.idle();
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.source.data.extend(bits.iter().copied());
    }

    /// Data waiting to go, less what the next mapping frame takes at once.
    pub fn pending_bits(&self) -> usize {
        let frame = self.source.encoder.as_ref().map_or(0, |e| e.params().framing.b);
        self.source.data.len().saturating_sub(frame)
    }

    fn fail(&mut self, why: &'static str) {
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
        self.source.pending = None;
        self.source.start(Up::Silence);
        self.rx.idle();
    }

    /// One line sample in, one out.
    pub fn step(&mut self, line: f64) -> f64 {
        self.now += 1;
        self.rx.feed(line);
        // 9.3.2, 9.4.2 and 9.6.2: tone B, in phase 3, phase 4 or data mode, is
        // the digital modem retraining.
        if self.stage != Stage::Finished && self.retrain_watch.feed(line, self.fs) {
            self.wants_retrain = true;
        }
        // A receiver that has held still for three seconds is not going to
        // find its place again: 9.5.2.1, "The analogue modem may initiate a
        // retrain at any time".
        match (self.rx.is_lost(), self.lost_since) {
            (true, None) => self.lost_since = Some(self.now),
            (false, Some(_)) => self.lost_since = None,
            (true, Some(since))
                if self.now - since > (3.0 * self.fs) as u64 && self.stage == Stage::Data && !self.renegotiating =>
            {
                self.wants_retrain = true;
                self.lost_since = None;
            }
            _ => {}
        }
        while let Some(heard) = self.rx.heard() {
            if self.stage != Stage::Finished {
                self.heard(heard);
            }
        }
        if let Some((at, why)) = self.deadline
            && self.now > at
            && self.status == Status::Running
        {
            if self.renegotiating {
                // 9.6.2: a renegotiation that goes nowhere is a retrain.
                self.deadline = None;
                self.wants_retrain = true;
            } else {
                self.fail(why);
            }
        }
        self.stage_step();
        let source = &mut self.source;
        self.tx.next_sample(|| source.next())
    }

    fn stage_step(&mut self) {
        match self.stage {
            Stage::SendTraining if self.source.up == Up::Ja => {
                self.stage = Stage::AwaitSd;
                self.rx.hunt(self.settings.uinfo);
                // 9.3.2.4: S-bar-d within 1500 ms of the start of Ja.
                self.sd_deadline = (self.samples(SD_WAIT + 2.0 * self.settings.round_trip), "no Sd from the digital modem");
                self.deadline = Some(self.sd_deadline);
            }
            Stage::Data if self.clearing => {
                if self.source.up == Up::Cp && self.source.cps >= CLEARDOWN_CPS {
                    self.cleared_down();
                }
            }
            Stage::Phase4 | Stage::Data => {
                // 9.4.2.4: a CP' sent, and MP' or Ed heard: E once the
                // current CP' is whole.
                let heard_back = !self.awaiting_turn && self.frames.as_ref().is_some_and(|f| f.far_acknowledged || f.ed);
                if self.source.up == Up::Cp && self.source.acknowledged >= 1 && heard_back && self.source.pending.is_none() {
                    self.prepare_upstream();
                    self.source.change(Up::E);
                }
                let receiving = self.frames.as_ref().is_some_and(|f| f.data);
                if self.status == Status::Running && !self.renegotiating && receiving && self.source.up == Up::Data {
                    self.status = Status::Connected { downstream: self.downstream_rate, upstream: self.upstream_rate };
                    self.deadline = None;
                    self.margin_at = self.samples(MARGIN_SETTLE);
                    self.short = 0;
                }
                if self.in_data_mode() && self.now >= self.margin_at {
                    self.margin_at = self.samples(MARGIN_EVERY);
                    self.watch_margin();
                }
            }
            _ => {}
        }
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Reversal { .. } => {
                if self.stage == Stage::AwaitSd {
                    // 9.3.2.4: "terminate Ja and transmit silence".
                    self.source.change(Up::Silence);
                    self.stage = Stage::Training;
                    self.deadline = Some((self.samples(4.5 + self.settings.round_trip), "no Jd from the digital modem"));
                }
            }
            Heard::Trained { .. } => {
                if self.stage == Stage::Training {
                    self.stage = Stage::AwaitJd;
                }
            }
            Heard::Untrained if self.stage == Stage::Training && self.now < self.sd_deadline.0 => {
                // Something that was not Sd set the hunt off: back to Ja
                // and the hunt, while Sd can still come.
                self.stage = Stage::AwaitSd;
                self.source.change(Up::Ja);
                self.rx.hunt(self.settings.uinfo);
                self.deadline = Some(self.sd_deadline);
            }
            Heard::Untrained => self.fail("the digital modem's TRN1d did not train this end"),
            Heard::Symbol(symbol) => self.symbol(symbol),
            Heard::Lost if self.stage == Stage::Dil && self.dil_lost.is_none() => {
                // What arrived just before the loss was noticed is held with
                // the rest until it is known whether anything moved.
                self.dil_lost = Some(0);
            }
            // A slip, or something like one: the receiver holds its loops,
            // and where the frames went is worked out from what comes after.
            Heard::Lost | Heard::Found => {}
        }
    }

    fn symbol(&mut self, symbol: pcm::Symbol) {
        self.last = [self.last[1], symbol.value];
        self.last_symbol = Some(symbol);
        self.heard_any = true;
        match self.stage {
            Stage::AwaitJd | Stage::AwaitJdPrime => {
                let jd_prime = self.jd.feed(symbol.index, symbol.positive());
                let early = (JD_LATEST + S_AFTER_JD - self.settings.round_trip).max(0.0) * digital::FS;
                if self.stage == Stage::AwaitJd && !self.sending_s && symbol.raw as f64 >= early {
                    self.send_s();
                }
                if self.stage == Stage::AwaitJd
                    && let Some((_, jd)) = self.jd.last
                {
                    // 9.3.2.7: S, and listen for J'd.
                    self.far_jd = Some(jd);
                    self.source.size = if jd.sixteen_in_training { Size::Sixteen } else { Size::Four };
                    if !self.sending_s {
                        self.send_s();
                    }
                    self.stage = Stage::AwaitJdPrime;
                    self.deadline = Some(self.start_deadline);
                }
                if self.stage != Stage::AwaitJdPrime {
                    return;
                }
                if jd_prime {
                    self.begin_dil(symbol.raw + 1, &[]);
                    return;
                }
                self.before_dil.push_back((symbol.raw, symbol.value));
                if self.before_dil.len() > DIL_START_KEPT {
                    self.before_dil.pop_front();
                }
                // A Jd stream that has stopped with no J'd read: the DIL is
                // under way, and is looked for in what has arrived.
                let gone = self.jd.last.is_some_and(|(end, _)| symbol.index + 1 > end + JD_GONE);
                if gone && !self.jd_gone {
                    // Whatever is arriving is not two levels any more, and
                    // learning from it as if it were would ruin the
                    // equaliser before the DIL is found.
                    self.jd_gone = true;
                    self.rx.set_slicer(Slicer::Free);
                }
                if gone && self.before_dil.len() >= DIL_START_WINDOW + INTERVALS && symbol.raw.is_multiple_of(64) {
                    self.find_dil_start();
                }
            }
            Stage::Dil => self.dil_symbol(symbol.raw, symbol.value),
            Stage::Phase4 | Stage::Data => self.phase4_symbol(symbol),
            _ => {}
        }
    }

    /// S until J'd (9.3.2.7).
    fn send_s(&mut self) {
        self.sending_s = true;
        self.source.s_length = None;
        self.source.change(Up::S);
    }

    /// The DIL from the receiver's count `first` (9.3.2.8): S-bar for 16T,
    /// the frames put where the DIL says they are, and the symbols in
    /// `already` read as its first.
    fn begin_dil(&mut self, first: u64, already: &[(u64, f64)]) {
        self.source.after_s_bar = Up::Silence;
        self.source.change(Up::SBar);
        self.stage = Stage::Dil;
        self.dil_base = first as i64;
        // J'd ends on a frame boundary -- Jd does, and J'd is two frames --
        // so the DIL's first symbol is in interval 0, whatever a slip did to
        // the frames on the way.
        self.dil_interval = 0;
        self.rx.set_frame_offset((INTERVALS as u64 - first % INTERVALS as u64) % INTERVALS as u64);
        self.dil_read = vec![false; self.dil.len()];
        self.dil_left = self.dil.len();
        self.dil_recent.clear();
        self.dil_lost = None;
        self.before_dil.clear();
        let next = already.last().map_or(first, |s| s.0 + 1);
        // Two passes: one, and what a slip loses of it read again.
        let levels = self.dil_levels(next as i64, 2 * self.dil.len());
        self.rx.expect_from(next, levels);
        for &(raw, value) in already {
            self.dil_symbol(raw, value);
        }
    }

    /// The DIL's start, from what has arrived since J'd was due: where the
    /// DIL fits what came after it closely and far better than anywhere else.
    fn find_dil_start(&mut self) {
        let arrived: Vec<(u64, f64)> = self.before_dil.iter().copied().collect();
        let Some(&(newest, _)) = arrived.last() else { return };
        let oldest = arrived[0].0;
        let mut fits = Vec::new();
        for first in oldest..=newest.saturating_sub(DIL_START_WINDOW as u64) {
            let from = (first - oldest) as usize;
            let window = &arrived[from..(from + DIL_START_WINDOW).min(arrived.len())];
            if let Some((fit, agree)) = self.fit_dil(window, first as i64)
                && agree >= DIL_SIGNS
            {
                fits.push((first, fit));
            }
        }
        let Some(&(first, fit)) = fits.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { return };
        let next = fits.iter().filter(|f| f.0.abs_diff(first) > 1).map(|f| f.1).fold(f64::INFINITY, f64::min);
        if fit > DIL_FIT || next < 4.0 * fit {
            return;
        }
        self.dil_found_late = true;
        let from = (first - oldest) as usize;
        self.begin_dil(first, &arrived[from..]);
    }

    /// How well `window` fits the DIL taken to start at the receiver's count
    /// `first`, over the symbols it can be judged on: the error's power
    /// against the DIL's, and the share of signs that agree. None if too few
    /// of the symbols are training symbols to tell one segment from another:
    /// every segment's references are alike.
    fn fit_dil(&self, window: &[(u64, f64)], first: i64) -> Option<(f64, f64)> {
        let law = self.settings.law;
        let len = self.dil.len() as i64;
        let (mut cost, mut power, mut agree, mut judged, mut trained) = (0.0, 0.0, 0usize, 0usize, 0usize);
        for &(raw, v) in window {
            let at = (raw as i64 - first).rem_euclid(len) as usize;
            let (u, positive) = self.dil[at];
            let level = ucode::level(law, u);
            // Too quiet for a sign to mean anything, or too near a loud
            // codeword to trust.
            if self.dil_trusted[at] != Trust::Yes || level < 0.004 {
                continue;
            }
            let e = if positive { level } else { -level };
            cost += (v - e).powi(2);
            power += e * e;
            judged += 1;
            if u != self.settings.uinfo {
                trained += 1;
            }
            if (v >= 0.0) == positive {
                agree += 1;
            }
        }
        (trained >= DIL_TRAINED_JUDGED && power > 0.0).then(|| (cost / power, agree as f64 / judged as f64))
    }

    /// Whether the DIL had to be found without J'd.
    pub fn dil_found_late(&self) -> bool {
        self.dil_found_late
    }

    /// The DIL's signed levels from the receiver's count `from`, for `n`
    /// symbols, as the DIL now stands against that count: NaN where a level
    /// is too loud to learn from.
    fn dil_levels(&self, from: i64, n: usize) -> Vec<f64> {
        let law = self.settings.law;
        let len = self.dil.len() as i64;
        (0..n as i64)
            .map(|k| {
                let at = (from + k - self.dil_base).rem_euclid(len) as usize;
                let (u, positive) = self.dil[at];
                let level = ucode::level(law, u);
                if self.dil_trusted[at] != Trust::Yes {
                    f64::NAN
                } else if positive {
                    level
                } else {
                    -level
                }
            })
            .collect()
    }

    /// Times a slip moved the DIL and it was found again.
    pub fn dil_moved(&self) -> u32 {
        self.dil_moved
    }

    /// How much of the DIL has been read, of how much there is, and whether
    /// the reading is waiting to find where it went.
    pub fn dil_progress(&self) -> (usize, usize, bool) {
        (self.dil.len() - self.dil_left, self.dil.len(), self.dil_lost.is_some())
    }

    fn dil_symbol(&mut self, raw: u64, value: f64) {
        self.dil_recent.push_back((raw, value));
        if let Some(gathered) = self.dil_lost {
            if self.dil_recent.len() > DIL_KEPT_LOST {
                self.dil_recent.pop_front();
            }
            self.dil_lost = Some(gathered + 1);
            if gathered + 1 >= DIL_FIRST_LOOK && (gathered + 1).is_multiple_of(DIL_LOOK_EVERY) {
                self.find_dil();
            }
            return;
        }
        while self.dil_recent.len() > DIL_DELAY {
            let Some((raw, value)) = self.dil_recent.pop_front() else { break };
            self.count_dil(raw, value);
            if self.stage != Stage::Dil {
                return;
            }
        }
    }

    /// One DIL symbol read, wherever in the DIL it falls: the DIL repeats,
    /// so a symbol a slip spoiled comes round again.
    fn count_dil(&mut self, raw: u64, value: f64) {
        let len = self.dil.len() as i64;
        let at = (raw as i64 - self.dil_base).rem_euclid(len) as usize;
        if self.dil_read[at] {
            return;
        }
        self.dil_read[at] = true;
        self.dil_left -= 1;
        let (u, positive) = self.dil[at];
        if self.dil_trusted[at] != Trust::Spoiled {
            self.analysis.feed(u, positive, (at + self.dil_interval) % INTERVALS, value);
        }
        if self.dil_left == 0 {
            self.finish_dil();
        }
    }

    /// Where the DIL went after a loss: the move that makes what has arrived
    /// lately most like it, once one does so clearly -- closely, and far
    /// better than any other. No move at all, if that fits: a route that
    /// robs a bit, or a burst of noise, spoils the reading without moving
    /// anything.
    fn find_dil(&mut self) {
        let window: Vec<(u64, f64)> = self.dil_recent.iter().skip(self.dil_recent.len().saturating_sub(DIL_SEARCH)).copied().collect();
        // (move, relative error) for each move whose signs agree.
        let fits: Vec<(i64, f64)> = (-DIL_MOST_MOVED..=DIL_MOST_MOVED)
            .filter_map(|m| {
                let (fit, agree) = self.fit_dil(&window, self.dil_base + m)?;
                (agree >= DIL_SIGNS).then_some((m, fit))
            })
            .collect();
        let Some(&(moved, fit)) = fits.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { return };
        let next = fits.iter().filter(|f| (f.0 - moved).abs() > 1).map(|f| f.1).fold(f64::INFINITY, f64::min);
        // A move is only a move against staying put: where the stretch is
        // too quiet to say whether it is where it was, wait for one that
        // is not.
        if moved != 0 && !fits.iter().any(|f| f.0 == 0) && self.fit_dil(&window, self.dil_base).is_none() {
            return;
        }
        if fit > DIL_FIT || (moved != 0 && next < 4.0 * fit) {
            return;
        }
        self.dil_lost = None;
        // Nothing moved: everything held is good. A move: only what it was
        // found from is sure to be past the slip, and the next pass has the
        // rest.
        let recent: Vec<(u64, f64)> = if moved == 0 { self.dil_recent.drain(..).collect() } else { window };
        self.dil_recent.clear();
        let next = recent.last().map_or(0, |r| r.0 as i64 + 1);
        if moved != 0 {
            self.dil_moved += 1;
            self.dil_base += moved;
            // The frames moved with it.
            let offset = (self.rx.frame_offset() as i64 - moved).rem_euclid(INTERVALS as i64) as u64;
            self.rx.set_frame_offset(offset);
        }
        let levels = self.dil_levels(next, 2 * self.dil.len());
        self.rx.expect_from(next as u64, levels);
        for (raw, value) in recent {
            self.count_dil(raw, value);
            if self.stage != Stage::Dil {
                return;
            }
        }
    }

    /// A whole pass of the DIL is in: choose, and say so (9.3.2.10).
    fn finish_dil(&mut self) {
        let route = self.analysis.route();
        let law = self.settings.law;
        let limit = super::power_limit(&self.settings.server);
        let jd = self.far_jd.unwrap_or_default();
        let Some(mut choice) = dil::choose(&route, law, limit, |drn| jd.enables(drn)) else {
            self.route = Some(route);
            self.fail("the route cannot carry V.90's slowest rate");
            return;
        };
        let rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
        if rate < self.settings.v34_receive {
            // A route that is an ordinary line with G.711's noise on it --
            // a softphone that converted the sample rate on the way to its
            // encoder -- carries V.34 at least as well.
            self.route = Some(route);
            self.fail("V.34 carries more than V.90 on this route");
            return;
        }
        self.finish_cp(&mut choice.data);
        self.finish_cp(&mut choice.training);
        self.downstream_rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
        // "S for 128T followed by S-bar for 16T", and phase 4: CPt.
        self.source.s_length = Some(signals::S_SYMBOLS);
        self.source.after_s_bar = Up::Cp;
        self.source.cp = choice.training.to_bits();
        self.source.cp_is_ack = false;
        self.source.change(Up::S);
        self.route = Some(route);
        self.choice = Some(choice);
        // What comes down now is more DIL, and then R: nothing to learn from
        // until R is sure.
        self.rx.set_slicer(Slicer::Free);
        self.stage = Stage::Phase4;
    }

    fn phase4_symbol(&mut self, symbol: pcm::Symbol) {
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return };
        if self.trn2d_from.is_none() {
            let level = ucode::level(self.settings.law, self.settings.uinfo);
            let was_heard = self.r_watch.heard;
            let seen = self.r_watch.feed(&symbol, &[level; INTERVALS]);
            if let RSeen::Moved(late) = seen {
                self.move_frames(late);
                return;
            }
            if let RSeen::Turned(from) = seen {
                // 9.4.2.2: the current CPt whole, then CP.
                self.trn2d_from = Some(from);
                self.source.next_cp = Some((choice.data.to_bits(), false));
                let Some(training) = Mapping::from_cp(&choice.training) else {
                    self.fail("this end's CPt is not a mapping");
                    return;
                };
                let levels = levels_for(&choice.training, route);
                self.frames = Some(Frames::new(from, training, levels, 0));
            } else if self.r_watch.heard && !was_heard {
                self.rx.set_slicer(Slicer::Binary(level));
            }
            return;
        }
        // Data mode, and a renegotiation until R-bar-d: Rd, from either end's
        // asking (9.6.2.1.4, 9.6.2.2.1).
        let receiving = self.frames.as_ref().is_some_and(|f| f.data);
        let mut looked = false;
        if self.stage == Stage::Data && (receiving || self.awaiting_turn) {
            let levels = self.rd_levels();
            let was_heard = self.rd_watch.heard;
            match self.rd_watch.feed(&symbol, &levels) {
                RSeen::Turned(from) => {
                    self.turned(from);
                    return;
                }
                RSeen::Moved(late) => {
                    self.move_frames(late);
                    return;
                }
                RSeen::Nothing => {}
            }
            if self.rd_watch.heard {
                if !was_heard {
                    if self.renegotiating {
                        self.clamp();
                    } else {
                        self.begin_renegotiation(false);
                    }
                }
                return;
            }
            looked = self.rd_watch.looked && symbol.interval() == INTERVALS - 1;
        }
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return };
        let Some(frames) = self.frames.as_mut() else { return };
        if symbol.index + 1 == frames.from {
            // TRN2d's first symbol is next: decide against CPt's levels.
            self.rx.set_slicer(slicer_for(&frames.levels));
            return;
        }
        if symbol.index < frames.from {
            return;
        }
        let i = symbol.interval();
        frames.frame[i] = nearest(&frames.levels[i], symbol.value);
        if frames.history.len() == PLACE_KEPT {
            frames.history.pop_front();
        }
        frames.history.push_back((symbol.index, symbol.value));
        if i != INTERVALS - 1 {
            return;
        }
        let frame = Frame {
            ucodes: std::array::from_fn(|k| frames.frame[k].0),
            positive: std::array::from_fn(|k| frames.frame[k].1),
        };
        if frames.impossible.len() == PLACE_WINDOW {
            frames.impossible.pop_front();
        }
        // Rd is no frame of data, and says nothing about where the frames are.
        frames.impossible.push_back(!looked && !frames.decoder.could_have_sent(&frame));
        if frames.impossible.iter().filter(|x| **x).count() >= PLACE_LOST
            && let Some(shift) = find_place(frames)
        {
            // The frames are somewhere else: a slip has moved every symbol
            // after it by a whole twenty milliseconds.
            frames.moved += 1;
            frames.impossible.clear();
            frames.history.clear();
            let offset = self.rx.frame_offset() + shift;
            self.rx.set_frame_offset(offset);
            return;
        }
        let bits: Vec<bool> = frames.decoder.frame(frame).into_iter().map(|b| frames.descrambler.descramble(b)).collect();
        if frames.data {
            if looked {
                frames.held.push_back(bits);
            } else {
                for held in frames.held.drain(..) {
                    self.received.extend(held);
                }
                self.received.extend(bits);
            }
            return;
        }
        if frames.ed {
            // B1d: 48 frames of scrambled ones.
            frames.b1d_left -= 1;
            if frames.b1d_left == 0 {
                frames.data = true;
                self.stage = Stage::Data;
                self.renegotiating = false;
                self.rd_watch = RWatch::default();
            }
            return;
        }
        let zeros = bits.iter().all(|b| !*b);
        frames.zero_frames = if zeros && frames.mp.is_some() { frames.zero_frames + 1 } else { 0 };
        if frames.zero_frames == 2 {
            // Ed: B1d next, at data mode's constellation, with the coding
            // started afresh (8.6.1).
            frames.ed = true;
            frames.b1d_left = B1D_FRAMES;
            self.deadline = None;
            self.in_use = Some(choice.data.clone());
            self.least_gap = least_gap(&choice.data, route);
            self.downstream_rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
            let Some(data) = Mapping::from_cp(&choice.data) else { return };
            frames.decoder = Decoder::new(data);
            frames.levels = levels_for(&choice.data, route);
            let slicer = slicer_for(&frames.levels);
            self.rx.set_slicer(slicer);
            return;
        }
        for bit in bits {
            if let Some(Found::Mp(mp)) = frames.finder.feed(bit) {
                if mp.answer_to_call == 0 {
                    // 9.7: the digital modem has cleared down.
                    self.cleared_down();
                    return;
                }
                if mp.acknowledge {
                    frames.far_acknowledged = true;
                }
                if frames.mp.is_none() {
                    // 9.4.2.3: "complete sending the current CP sequence,
                    // and then send CP' sequences".
                    self.source.next_cp = Some((choice.data.acknowledged().to_bits(), true));
                }
                frames.mp = Some(mp);
            }
        }
    }

    /// Whether data mode's levels still stand far enough apart for the error
    /// the receiver is making, and a slower rate if they have not for a while.
    fn watch_margin(&mut self) {
        let law = self.settings.law;
        let rms = ucode::level(law, self.settings.uinfo) / 10f64.powf(self.rx.snr_db() / 20.0);
        // A slip's burst is not the line.
        if self.rx.is_lost() {
            return;
        }
        self.short = if self.least_gap < MARGIN * rms { self.short + 1 } else { 0 };
        if self.short < MARGIN_SHORT {
            return;
        }
        self.short = 0;
        if self.least_gap < 2.0 * rms {
            // Nothing is being read at all: that is a receiver to train again.
            self.wants_retrain = true;
            return;
        }
        // The next rate is chosen for the line as data mode finds it.
        if let Some(route) = self.route.as_ref() {
            let limit = f64::from(super::power_limit(&self.settings.server)) / 32768.0;
            self.worse = self.worse.max(rms / route.noise_at(law, limit));
        }
        let most = self.downstream_rate.saturating_sub(1);
        if !self.renegotiate(most) {
            // Nothing slower the route carries: train again from phase 2.
            self.wants_retrain = true;
        }
    }

    /// What every CP this end sends says besides its constellations and rate.
    fn finish_cp(&self, cp: &mut Cp) {
        cp.a_law = self.settings.law == Law::A;
        cp.upstream_rates = if self.settings.wide { 0x1fff } else { 0x07ff };
        cp.lookahead = 0;
    }

    /// Times the downstream frames were found somewhere else after a slip,
    /// from the data frames themselves or from R.
    pub fn frames_moved(&self) -> u32 {
        self.frames.as_ref().map_or(0, |f| f.moved) + self.r_moved
    }

    /// Data mode's upstream, as the digital modem's MP asks for it.
    fn prepare_upstream(&mut self) {
        let (Some(mp), Some(choice)) = (self.far_mp(), self.choice.as_ref()) else { return };
        let rate = super::digital::upstream_rate(&choice.data, &mp);
        self.upstream_rate = u32::from(rate) * 2400;
        let Some(framing) = Framing::new(self.settings.upstream.rate, self.upstream_rate, false, mp.expanded_shaping) else {
            self.fail("no upstream rate both ends allow");
            return;
        };
        let params = Params {
            framing,
            code: match mp.trellis {
                Trellis::States16 => Code::States16,
                Trellis::States32 => Code::States32,
                Trellis::States64 => Code::States64,
            },
            nonlinear: mp.non_linear,
            precoding: mp.precoding.unwrap_or([(0, 0); 3]),
            mode: Mode::Answer,
        };
        self.source.encoder = Some(UpstreamEncoder::new(params));
    }
}

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
        signs.iter().enumerate().find_map(|(i, &s)| reader.feed(i as u64, s).then_some(i + 1))
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
}
