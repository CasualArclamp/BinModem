//! V.29: 9600, 7200 and 4800 bit/s, sixteen points at 2400 baud.
//!
//! Written for four-wire leased circuits, and adopted by T.30 as the middle
//! of the three modulations a fax can carry a page with: twice V.27 ter's
//! speed, and a third of V.17's complication. There is no trellis code and no
//! rate negotiation. A burst is a synchronizing signal, then data, then
//! silence, exactly as V.27 ter's is.
//!
//! The constellation is the part that is not like anything else here. It is
//! not a grid: the points sit on the eight phases of V.27 ter, at two radii,
//! and the radii are different on the axes from on the diagonals -- 3 and 5 on
//! the one, the square root of 2 and three times it on the other. Three of the
//! four bits of a symbol are a phase change, coded exactly as V.27 ter codes
//! its tribits, and the fourth says which of the two radii.
//!
//! Everything numerical here was read off the figures in the PDF rather than
//! off the extracted text, which turns the square root of 2 into "2" and three
//! times it into "32".

mod hunt;

use std::sync::OnceLock;

use dsp::qam::{Band, Constellation, Core, Heard, Options, Slicer, Training, Via, Window};
use dsp::{Complex, Nco, rrc_at};

use crate::v27ter::{TRIBIT_TURN, TURN_TRIBIT};
use crate::v32;
use hunt::{Found, Hunt, Level};

/// 2.1: "The carrier frequency is to be 1700 +/- 1 Hz."
pub const CARRIER: f64 = 1700.0;

/// Clause 3: "The modulation rate is 2400 bauds", at every data rate.
pub const BAUD: f64 = 2400.0;

/// The roll-off of the shaping, split equally between the two ends.
///
/// Clause 11 fixes the attenuation at 500 and 2900 Hz, which is the carrier
/// plus and minus half the baud rate, to 4.5 dB +/- 2.5 dB. A root raised
/// cosine is 3 dB down there at any roll-off, so the Recommendation leaves the
/// roll-off itself to the implementation. A quarter is what the V.32 pump here
/// uses at the same baud rate, and that one has been proven down a real line.
pub const ROLLOFF: f64 = 0.25;

/// Symbols either side of centre that the shaping pulse reaches.
pub const SPAN: usize = 6;

/// Which of the three rates is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rate {
    /// Quadbits: all sixteen points of Figure 1 (2.2.1).
    #[default]
    R9600,
    /// Tribits: the inner eight points, Figure 2 (2.2.2).
    R7200,
    /// Dibits: the four points on the axes, Figure 3 (2.2.3). T.30 never
    /// uses it -- a fax at 4800 uses V.27 ter -- but it is part of the
    /// Recommendation, and it costs four lines.
    R4800,
}

impl Rate {
    /// Data bits carried by each symbol.
    pub fn bits(self) -> usize {
        match self {
            Self::R9600 => 4,
            Self::R7200 => 3,
            Self::R4800 => 2,
        }
    }

    pub fn bits_per_second(self) -> u32 {
        match self {
            Self::R9600 => 9600,
            Self::R7200 => 7200,
            Self::R4800 => 4800,
        }
    }

    /// Every point this rate can send.
    pub fn constellation(self) -> Vec<Point> {
        let eighths: Vec<u8> = match self {
            Self::R4800 => vec![0, 2, 4, 6],
            _ => (0..8).collect(),
        };
        let rings: &[bool] = match self {
            Self::R9600 => &[false, true],
            _ => &[false],
        };
        rings
            .iter()
            .flat_map(|&outer| eighths.iter().map(move |&e| Point { eighths: e, outer }))
            .collect()
    }

    /// Root mean square of the constellation, in the units of Figure 1.
    ///
    /// The square roots of 13.5, 5.5 and 9. Everything is divided by this
    /// before it goes out, so the three rates leave at the same power.
    pub fn rms(self) -> f64 {
        let points = self.constellation();
        let power: f64 = points.iter().map(|p| p.power()).sum();
        (power / points.len() as f64).sqrt()
    }
}

/// One point of Figure 1: which of the eight phases it lies on, and whether
/// it is on the outer ring.
///
/// Two numbers rather than two coordinates, because that is how Table 2 thinks
/// of them: the phase is what the bits change, and the ring is one bit on its
/// own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    /// The absolute phase, in eighths of a turn from 0 degrees.
    pub eighths: u8,
    /// Q1 of Table 2.
    pub outer: bool,
}

impl Point {
    /// Where the point is, in the units of Figure 1.
    ///
    /// Table 2: 3 or 5 on the axes, the square root of 2 or three times it on
    /// the diagonals -- which are the points (1, 1) and (3, 3) and their
    /// reflections.
    pub fn xy(self) -> (f64, f64) {
        let e = self.eighths & 7;
        if e.is_multiple_of(2) {
            let r = if self.outer { 5.0 } else { 3.0 };
            match e {
                0 => (r, 0.0),
                2 => (0.0, r),
                4 => (-r, 0.0),
                _ => (0.0, -r),
            }
        } else {
            let k = if self.outer { 3.0 } else { 1.0 };
            match e {
                1 => (k, k),
                3 => (-k, k),
                5 => (-k, -k),
                _ => (k, -k),
            }
        }
    }

    fn power(self) -> f64 {
        let (x, y) = self.xy();
        x * x + y * y
    }
}

/// The synchronizing signal, Table 5.
pub mod train {
    /// Segment 1: no transmitted energy.
    pub const SILENCE: u32 = 48;
    /// Segment 2: ABAB, for the timing and the carrier.
    pub const ALTERNATIONS: u32 = 128;
    /// Segment 3: C and D in a pseudo-random order, for the equaliser.
    pub const CONDITIONING: u32 = 384;
    /// Segment 4: scrambled ONEs, coded as data.
    pub const ONES: u32 = 48;
    /// "608" symbol intervals, "253" ms.
    pub const TOTAL: u32 = SILENCE + ALTERNATIONS + CONDITIONING + ONES;
}

/// Figure 4: A, "a relative amplitude of 3 and ... the absolute phase
/// reference of 180 degrees" (8.1).
pub const A: Point = Point { eighths: 4, outer: false };

/// Figure 4: C, "a relative amplitude of 3 and absolute phase of 0 degrees"
/// (8.2).
pub const C: Point = Point { eighths: 0, outer: false };

/// Figure 4: B, the second point of segment 2, which depends on the rate.
///
/// (0, -3) at 4800, (1, -1) at 7200 and (3, -3) at 9600: a point of each
/// rate's own constellation, so the receiver can slice the training with the
/// same slicer as the data.
pub fn b(rate: Rate) -> Point {
    match rate {
        Rate::R4800 => Point { eighths: 6, outer: false },
        Rate::R7200 => Point { eighths: 7, outer: false },
        Rate::R9600 => Point { eighths: 7, outer: true },
    }
}

/// Figure 4: D, the second point of segment 3, always opposite B.
pub fn d(rate: Rate) -> Point {
    match rate {
        Rate::R4800 => Point { eighths: 2, outer: false },
        Rate::R7200 => Point { eighths: 3, outer: false },
        Rate::R9600 => Point { eighths: 3, outer: true },
    }
}

/// The pseudo-random sequence of segment 3, Appendix I.
///
/// `1 + x^-6 + x^-7`, the same polynomial as V.27 ter's training, but clocked
/// once a symbol rather than three times, and started from 0101010. The
/// appendix lists the first four conditions of the register, and 8.2 says the
/// segment "begins with the sequence CDCDCDC"; reading the last stage as the
/// output, and shifting towards it, is the one arrangement that gives both.
#[derive(Debug, Clone)]
pub struct Conditioning {
    /// Stage 1 in bit 6, stage 7 in bit 0.
    register: u8,
}

impl Default for Conditioning {
    fn default() -> Self {
        Self::new()
    }
}

impl Conditioning {
    /// "The initial condition of the generator is 0101010."
    const INITIAL: u8 = 0b010_1010;

    pub fn new() -> Self {
        Self { register: Self::INITIAL }
    }

    /// The register as Appendix I writes it, stage 1 first.
    pub fn condition(&self) -> u8 {
        self.register
    }

    /// The next bit: a ZERO sends C and a ONE sends D.
    pub fn next_bit(&mut self) -> bool {
        let out = self.register & 1 != 0;
        let fed = (self.register ^ (self.register >> 1)) & 1;
        self.register = (self.register >> 1) | (fed << 6);
        out
    }
}

/// Clause 9's scrambler, `1 + x^-18 + x^-23`.
///
/// The same polynomial V.32 gives the calling modem, and already written. V.29
/// has only the one, whichever way the data is going: it was designed for
/// four-wire circuits, where the two directions never meet, and a fax is half
/// duplex, where they never overlap. Appendix II has the register fed with
/// zeros through segments 1 to 3, which is a register that starts segment 4
/// empty.
fn scrambler() -> v32::Scrambler {
    v32::Scrambler::new(v32::Mode::Call)
}

/// Where a burst has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Silent,
    /// Segment 1.
    Quiet(u32),
    /// Segment 2, counting down.
    Alternations(u32),
    /// Segment 3.
    Conditioning(u32),
    /// Segment 4.
    Ones(u32),
    Data,
    /// The last of the data, then scrambled ONEs until the shaping filters at
    /// both ends have let go of the final symbol.
    TurnOff(u32),
}

/// Symbols of scrambled ONEs after the last data.
///
/// 5.3 asks only that the carrier stay up long enough "to ensure that all
/// valid signal elements have been transmitted". Twice the reach of the
/// shaping pulse does that at both ends, and is ten milliseconds.
const TURN_OFF_SYMBOLS: u32 = 2 * SPAN as u32 + 12;

/// The level a symbol goes out at, once divided by the rate's own root mean
/// square: a root mean square of 0.707 on the line, like every other
/// transmitter in this modem.
const LEVEL: f64 = 1.0;

/// V.29 transmitter.
#[derive(Debug)]
pub struct Transmitter {
    fs: f64,
    rate: Rate,
    nco: Nco,
    scrambler: v32::Scrambler,
    conditioning: Conditioning,
    stage: Stage,
    /// The point last sent. Every data symbol is a phase change from it, and
    /// segment 4's first one is a change from the last point of segment 3.
    point: Point,
    history: Vec<(f64, f64)>,
    phase: f64,
    pending: Vec<bool>,
    /// The symbol most recently put on the line, in the units of Figure 1.
    sent: (f64, f64),
}

impl Transmitter {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            rate: Rate::default(),
            nco: Nco::new(CARRIER, fs),
            scrambler: scrambler(),
            conditioning: Conditioning::new(),
            stage: Stage::Silent,
            point: C,
            history: vec![(0.0, 0.0); 2 * SPAN + 1],
            phase: 0.0,
            pending: Vec::new(),
            sent: (0.0, 0.0),
        }
    }

    /// The point most recently sent, scaled as the receiver reports points,
    /// or `None` while nothing is going out.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        if !self.is_transmitting() || self.sent == (0.0, 0.0) {
            return None;
        }
        let rms = self.rate.rms();
        Some((self.sent.0 / rms, self.sent.1 / rms))
    }

    /// Raise the carrier and begin the synchronizing signal.
    pub fn start(&mut self, rate: Rate) {
        self.rate = rate;
        self.scrambler = scrambler();
        self.conditioning = Conditioning::new();
        self.stage = Stage::Quiet(train::SILENCE);
        self.point = C;
        self.phase = 0.0;
        self.history.fill((0.0, 0.0));
        self.pending.clear();
    }

    /// Finish the burst: whatever is queued, then scrambled ONEs, then off.
    ///
    /// A second call is not a second turn-off. Whoever is above cannot see
    /// symbol boundaries and asks on every sample until the carrier goes.
    pub fn stop(&mut self) {
        if !matches!(self.stage, Stage::Silent | Stage::TurnOff(_)) {
            self.stage = Stage::TurnOff(TURN_OFF_SYMBOLS);
        }
    }

    /// Drop the carrier now.
    pub fn abort(&mut self) {
        self.stage = Stage::Silent;
        self.pending.clear();
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    pub fn is_transmitting(&self) -> bool {
        self.stage != Stage::Silent
    }

    /// Whether the synchronizing signal is over and data is going out.
    pub fn trained(&self) -> bool {
        matches!(self.stage, Stage::Data | Stage::TurnOff(_))
    }

    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend_from_slice(bits);
    }

    /// Push octets most significant bit first.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            for i in (0..8).rev() {
                self.pending.push(byte >> i & 1 != 0);
            }
        }
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }

    /// The next scrambled bit: from the queue, or a ONE when it is empty.
    fn data_bit(&mut self) -> bool {
        let bit = if self.pending.is_empty() {
            true
        } else {
            self.pending.remove(0)
        };
        self.scrambler.scramble(bit)
    }

    /// One symbol coded as 2.2 codes data.
    ///
    /// `ones` takes scrambled ONEs instead of the queue, for segment 4 and the
    /// turn-off. 8.3 puts the first read of the queue at the end of segment 4,
    /// and reading it any earlier throws the start of a page away.
    fn coded(&mut self, ones: bool) -> Point {
        let rate = self.rate;
        let mut next = || -> bool {
            if ones {
                self.scrambler.scramble(true)
            } else {
                self.data_bit()
            }
        };
        let (q1, q2, q3, q4) = match rate {
            Rate::R9600 => {
                let q1 = next();
                let q2 = next();
                let q3 = next();
                let q4 = next();
                (q1, q2, q3, q4)
            }
            // 2.2.2: "Q1 of the modulator quadbit is a data ZERO".
            Rate::R7200 => {
                let q2 = next();
                let q3 = next();
                let q4 = next();
                (false, q2, q3, q4)
            }
            // 2.2.3: Q4 "is determined by inverting the modulo 2 sum of
            // Q2 + Q3", which keeps every change a quarter turn.
            Rate::R4800 => {
                let q2 = next();
                let q3 = next();
                (false, q2, q3, !(q2 ^ q3))
            }
        };
        let tribit = usize::from(q2) << 2 | usize::from(q3) << 1 | usize::from(q4);
        self.point = Point {
            eighths: (self.point.eighths + TRIBIT_TURN[tribit]) & 7,
            outer: q1,
        };
        self.point
    }

    /// The next symbol, or `None` for one of no energy.
    fn next_symbol(&mut self) -> Option<Point> {
        let point = match self.stage {
            Stage::Silent => return None,
            Stage::Quiet(left) => {
                self.stage = if left > 1 {
                    Stage::Quiet(left - 1)
                } else {
                    Stage::Alternations(train::ALTERNATIONS)
                };
                return None;
            }
            Stage::Alternations(left) => {
                self.stage = if left > 1 {
                    Stage::Alternations(left - 1)
                } else {
                    Stage::Conditioning(train::CONDITIONING)
                };
                // Counting down from 128, so the first is even: A first.
                if (train::ALTERNATIONS - left).is_multiple_of(2) {
                    A
                } else {
                    b(self.rate)
                }
            }
            Stage::Conditioning(left) => {
                self.stage = if left > 1 {
                    Stage::Conditioning(left - 1)
                } else {
                    Stage::Ones(train::ONES)
                };
                if self.conditioning.next_bit() {
                    d(self.rate)
                } else {
                    C
                }
            }
            Stage::Ones(left) => {
                self.stage = if left > 1 {
                    Stage::Ones(left - 1)
                } else {
                    Stage::Data
                };
                return Some(self.coded(true));
            }
            Stage::Data => return Some(self.coded(false)),
            Stage::TurnOff(left) => {
                if !self.pending.is_empty() {
                    return Some(self.coded(false));
                }
                self.stage = if left > 1 {
                    Stage::TurnOff(left - 1)
                } else {
                    Stage::Silent
                };
                return Some(self.coded(true));
            }
        };
        // Segments 2 and 3 send absolute points. The data that follows is a
        // change from whichever of them went last, so it is remembered.
        self.point = point;
        Some(point)
    }

    pub fn next_sample(&mut self) -> f64 {
        if self.stage == Stage::Silent {
            self.nco.step();
            return 0.0;
        }
        self.phase += BAUD / self.fs;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol().map_or((0.0, 0.0), Point::xy);
            self.history.push(symbol);
            self.sent = symbol;
        }

        let centre = SPAN as f64;
        let mut baseband = (0.0, 0.0);
        for (i, &(re, im)) in self.history.iter().enumerate() {
            let offset = self.phase + centre - i as f64;
            let tap = rrc_at(offset, ROLLOFF);
            baseband.0 += re * tap;
            baseband.1 += im * tap;
        }

        let (cos, sin) = self.nco.step();
        LEVEL * (baseband.0 * cos - baseband.1 * sin) / self.rate.rms()
    }
}

/// Symbols of the known sequence, counted from segment 3's first, that the
/// equaliser is solved over: most of segment 3 and half of segment 4, with
/// the alignment searched four symbols either side of where the join put it.
///
/// Segment 3 is 384 symbols the receiver knows before they come (Appendix
/// I), and segment 4 another 48, since its scrambler starts empty and is fed
/// ONEs (Appendix II): 432 in all, and then the data. The solve ends 24
/// symbols short of the data, which is time for the training to be done
/// before the data begins, and 48 bits or more for the descrambler to have
/// the far end's register before the first data bit reaches it.
const FIRST: Window = Window { align: (8, 200), solve: (8, 408), search: 8 };

/// The second try, when the first fits nothing: the second half of the known
/// sequence, searched a hundred symbols either way. A slip in the first
/// half -- twenty milliseconds of a concealment is 48 symbols -- spoils the
/// first try and moves the second half, which this finds.
const RETRY: Window = Window { align: (216, 424), solve: (216, 424), search: 200 };

/// The first try when segment 2 ended without turning into segment 3, which
/// is then only guessed to begin where segment 2 stopped: searched sixty
/// symbols either way, which a concealment across the join is well inside.
const LAPSED: Window = Window { align: (8, 200), solve: (8, 408), search: 120 };

/// Symbols of segments 3 and 4 together: everything the far end sends that
/// the receiver knows in advance, after segment 2.
const KNOWN: usize = (train::CONDITIONING + train::ONES) as usize;

/// Where segment 4 begins in the known sequence.
const ONES_FROM: usize = train::CONDITIONING as usize;

/// Signal to noise, in decibels, below which a training's fit is taken to be
/// the wrong alignment and the second try is made.
///
/// A wrong alignment of segment 3 fits nothing but the mean of C and D, which
/// is a sixth of the power at 9600 and half of it at 4800, so it fits at 3 dB
/// at best. Well above that, and well below the 12 dB 4800 works at.
const ACCEPT_DB: f64 = 7.0;

/// Where a fresh [`Core`] centres its first half-symbol sample, in samples
/// from the first it is fed: the length of its interpolating filter, which it
/// needs whole before it can make one (`dsp/src/qam/front.rs`).
const CORE_FIRST_HALF: f64 = 64.0;

/// Half-symbol samples of line read into a new core in front of segment 3's
/// first symbol: the first try's search and the equaliser's reach, and some
/// to spare.
const LEAD_HALVES: f64 = 40.0;

/// How far the burst's signal, as a share of what segment 2 had, has to fall
/// for the line to be taken to have gone quiet: a quarter, 6 dB down. On top
/// of the line's own noise, which is there whether the burst is or not.
const QUIET_BELOW: f64 = 0.25;

/// How long quiet before the carrier is said to have gone. The level takes
/// 14 ms to fall 6 dB, so the carrier goes about 30 ms after the signal does,
/// the middle of the 30 +/- 9 ms 5.2.2 asks for -- and all the data is out by
/// then, since the receiver's own delay is 6 ms and the turn-off ten.
const QUIET_OFF_SECONDS: f64 = 0.016;

/// How long quiet before the burst is over and forgotten: the gap a fax call
/// bridges before it takes a burst to have ended (`fax::call`'s
/// `FAST_CARRIER_GONE`). Anything shorter is a hole in the burst, which the
/// receiver follows the signal across.
const QUIET_OVER_SECONDS: f64 = 0.200;

/// Symbols the signal may be lost for in a row before whatever is arriving is
/// taken not to be the burst at all: half a second.
const LOST_OVER: usize = 1200;

/// The rate's points at unit power, as the core decides against them,
/// labelled as [`Rate::constellation`] lists them. Made once: a constellation
/// works out what garbage reads against it when it is made.
fn table(rate: Rate) -> Slicer {
    static TABLES: OnceLock<[Slicer; 3]> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        [Rate::R9600, Rate::R7200, Rate::R4800].map(|rate| {
            let rms = rate.rms();
            let points = rate
                .constellation()
                .into_iter()
                .map(|p| {
                    let (x, y) = p.xy();
                    Complex::new(x / rms, y / rms)
                })
                .collect();
            Slicer::table(Constellation::new(points))
        })
    });
    match rate {
        Rate::R9600 => tables[0].clone(),
        Rate::R7200 => tables[1].clone(),
        Rate::R4800 => tables[2].clone(),
    }
}

/// Segments 3 and 4 at `rate`, as our own transmitter sends them, which is
/// as Table 5, 8.2, 8.3 and the appendices have them: the one generator for
/// both ends, so that they cannot disagree. Symbols, not samples, so the
/// transmitter's sampling rate does not matter.
fn known_sequence(rate: Rate) -> Vec<Point> {
    let mut tx = Transmitter::new(8000.0);
    tx.start(rate);
    let skipped = train::SILENCE + train::ALTERNATIONS;
    (0..skipped + KNOWN as u32).filter_map(|_| tx.next_symbol()).skip(train::ALTERNATIONS as usize).collect()
}

/// Q1 to Q4 of a symbol, by 2.2 backwards: the change of phase from the one
/// before is Q2 Q3 Q4 through Table 1, and the ring is Q1. And which of them
/// are the rate's data bits.
fn carried(rate: Rate, previous: Point, decided: Point) -> ([bool; 4], std::ops::Range<usize>) {
    let change = (decided.eighths + 8 - previous.eighths) & 7;
    let tribit = TURN_TRIBIT[usize::from(change)];
    let q = [decided.outer, tribit & 0b100 != 0, tribit & 0b010 != 0, tribit & 0b001 != 0];
    match rate {
        Rate::R9600 => (q, 0..4),
        // 2.2.2: Q1 is always a ZERO.
        Rate::R7200 => (q, 1..4),
        // Q4 is only the other two inverted and added, so it is not data.
        Rate::R4800 => (q, 1..3),
    }
}

/// A burst being heard.
#[derive(Debug, Clone, Copy)]
struct Burst {
    /// How loud it was as segment 2 had it, and how loud the line's noise.
    level: Level,
    /// Samples the line has been quiet for.
    quiet: usize,
    /// Whether the carrier is said to be there.
    heard: bool,
}

/// V.29 receiver, on the shared QAM core.
///
/// Everything that brings the far end's symbols back -- the fixed mixer, the
/// stored samples, the equaliser at two samples a symbol, the three loops and
/// their one gate, the gain control, losing the signal and finding it again --
/// is `dsp::qam`, which V.32bis is built on too. What is here is what is
/// V.29's own: finding a burst and where its segment 3 begins
/// (`v29/hunt.rs`), what segments 3 and 4 are, which constellation a symbol
/// is decided against, how bits come out of the points, and whether there is
/// a carrier.
///
/// The receiver this replaced acquired its carrier from a single measurement
/// over the front of segment 2, armed by a carrier detector with a fixed
/// threshold 53 dB under a burst, and never checked or repeated either
/// (fax-qam.md 3.1, 3.2): on a line with any hiss the detector latched on to
/// the noise before the burst, the measurement was made on noise, and the page
/// was lost whole. Its equaliser then had to open a two-radius eye blind.
///
/// Here nothing is armed by a level. A burst is heard when segment 2 is, by
/// what an alternation of two points is and noise is not, and the join into
/// segment 3 says where the known sequence begins. A new core is made for
/// each burst and given the line again from just before that, and the known
/// 432 symbols are solved against outright, by least squares, for the
/// equaliser, the gain, the carrier's absolute phase and its turn, the turn
/// having been measured already from segment 2. The hunt goes on the whole
/// time, so a false start, a slip or a far end starting again is found again,
/// and nothing is ever decided until it has been trained for.
///
/// Like the one before, it never decides where the data begins. A training
/// check is found as a run of zeros and a page by its first end-of-line
/// code, so every symbol after the training is decoded and whoever is above
/// finds its own place.
#[derive(Debug)]
pub struct Receiver {
    fs: f64,
    rate: Rate,
    /// The rate's points, by label, and the table the core decides against.
    labels: Vec<Point>,
    slicer: Slicer,
    /// Segments 3 and 4, as points and at unit power.
    known: Vec<Point>,
    targets: Vec<Complex>,
    hunt: Hunt,
    /// The core reading the burst being heard, made at its join.
    core: Option<Core>,
    /// The training the join calls for, held back until its first window is
    /// in while the hunt goes on listening, with the half-symbol sample by
    /// which it is; and the windows of the first try and the second.
    pending: Option<(Training, u64)>,
    windows: (Window, Window),
    burst: Option<Burst>,
    /// The power of the burst being heard, as segment 2 measured it, in the
    /// hunt's baseband units: what the carrier's going is judged against.
    power: f64,
    /// The point the previous symbol was decided to be.
    previous: Option<Point>,
    descrambler: v32::Scrambler,
    bits: Vec<bool>,
    last_symbol: (f64, f64),
    residual: f64,
    /// Samples to the next point shown while the burst is heard and not yet
    /// trained for.
    showing: f64,
}

impl Receiver {
    pub fn new(fs: f64) -> Self {
        let rate = Rate::default();
        let mut me = Self {
            fs,
            rate,
            labels: Vec::new(),
            slicer: table(rate),
            known: Vec::new(),
            targets: Vec::new(),
            hunt: Hunt::new(fs),
            core: None,
            pending: None,
            windows: (FIRST, RETRY),
            burst: None,
            power: 0.0,
            previous: None,
            descrambler: scrambler(),
            bits: Vec::new(),
            last_symbol: (0.0, 0.0),
            residual: 1.0,
            showing: 0.0,
        };
        me.follow(rate);
        me
    }

    /// Set the rate the burst about to arrive is at, which a fax receiver
    /// always knows from the DCS that came before it.
    pub fn set_rate(&mut self, rate: Rate) {
        if rate != self.rate {
            self.follow(rate);
            // A burst being read at the old rate is not one to go on with.
            self.core = None;
            self.pending = None;
        }
    }

    fn follow(&mut self, rate: Rate) {
        self.rate = rate;
        self.labels = rate.constellation();
        self.slicer = table(rate);
        self.known = known_sequence(rate);
        let rms = rate.rms();
        self.targets = self
            .known
            .iter()
            .map(|p| {
                let (x, y) = p.xy();
                Complex::new(x / rms, y / rms)
            })
            .collect();
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Whether the far end's carrier is there: from the moment its segment 2
    /// has been heard for 24 symbols, until the burst's signal has been 6 dB
    /// below what segment 2 had, over the line's own noise, for 16 ms. A plain
    /// carrier, a tone, noise at any level, are none of them segment 2, and
    /// do not raise it.
    pub fn carrier(&self) -> bool {
        self.burst.is_some_and(|b| b.heard)
    }

    /// The in-band level of what is arriving, as an amplitude after a mixer
    /// of unit gain, over the last ten milliseconds.
    pub fn level(&self) -> f64 {
        self.hunt.power().sqrt() / 2.0
    }

    /// Where the last symbol landed, scaled so the mean power is one.
    ///
    /// The equaliser's output once the burst is trained for. Before that,
    /// while segments 2 and 3 arrive and the training waits for them, the
    /// line itself a symbol apart, scaled by segment 2's power and turned by
    /// nothing: the points as they arrive, before anything has been learned.
    pub fn constellation_point(&self) -> (f64, f64) {
        self.last_symbol
    }

    /// How far out the constellation reaches in those units.
    ///
    /// Five over the square root of 13.5 at 9600, which is a third beyond the
    /// unit circle a scope draws its box at.
    pub fn constellation_peak(&self) -> f64 {
        let rms = self.rate.rms();
        self.labels.iter().map(|p| p.xy()).map(|(x, y)| x.abs().max(y.abs()) / rms).fold(0.0, f64::max)
    }

    /// Mean distance of the symbols from the nearest point, in the units
    /// [`point_spacing`](Self::point_spacing) is measured in, over about a
    /// hundred symbols. It stops where it is when the line goes quiet, since
    /// there is then nothing to be near or far from.
    pub fn residual_error(&self) -> f64 {
        self.residual
    }

    /// The distance between the two closest points, in the same units.
    pub fn point_spacing(&self) -> f64 {
        match &self.slicer {
            Slicer::Table(table) => table.d2min().sqrt(),
            Slicer::Grid { .. } => unreachable!("V.29's constellations are tables"),
        }
    }

    /// Forget the burst just gone, and everything learned from it.
    ///
    /// Every burst carries a training sequence built to teach a receiver from
    /// nothing, so nothing is lost by it, and keeping anything turns a
    /// moment's trouble into a lasting one.
    pub fn restart(&mut self) {
        self.hunt = Hunt::new(self.fs);
        self.core = None;
        self.pending = None;
        self.burst = None;
        self.previous = None;
        self.descrambler.reset();
        self.bits.clear();
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    pub fn feed(&mut self, sample: f64) {
        match self.hunt.feed(sample) {
            Some(Found::Alternations { level }) => {
                // A burst, or the far end starting again in the middle of
                // one (8, 10): whatever was being read is over either way.
                self.core = None;
                self.pending = None;
                self.burst = Some(Burst { level, quiet: 0, heard: true });
                self.power = level.power;
            }
            Some(Found::Reversal { at, turn, level }) => self.join(at, turn, level, FIRST),
            Some(Found::Lapsed { at, turn, level }) => self.join(at, turn, level, LAPSED),
            None => {}
        }
        if let Some(core) = &mut self.core {
            core.feed(sample);
            // The training the join called for, now that its first window is
            // all in, from the samples the core kept.
            if self.pending.as_ref().is_some_and(|(_, due)| core.halves() >= *due)
                && let Some((training, _)) = self.pending.take()
            {
                core.train(training);
            }
        }
        self.listen();
        self.symbols();
        self.follow_level();
    }

    /// Segment 2 has ended, into segment 3 or not: make a core, read it the
    /// line from a little before segment 3's first symbol, centred on line
    /// sample `at`, and train it there once the first window is in.
    fn join(&mut self, at: f64, turn: f64, level: Level, first: Window) {
        let half = self.fs / BAUD / 2.0;
        let now = self.hunt.taken() - 1;
        let from = (at - CORE_FIRST_HALF - LEAD_HALVES * half).floor().max(self.hunt.first_kept() as f64) as u64;
        let mut core = Core::new(Band::new(self.fs, BAUD, CARRIER), Options::fixed(), self.slicer.clone());
        for index in from..now {
            core.feed(self.hunt.raw(index).unwrap_or(0.0));
        }
        let start = ((at - from as f64 - CORE_FIRST_HALF) / half).round().max(0.0) as u64;
        let training = Training {
            targets: self.targets.clone(),
            start,
            first,
            retry: Some(RETRY),
            turn: Some(turn),
            drift: None,
            accept_db: ACCEPT_DB,
            slicer: self.slicer.clone(),
            fallback: false,
        };
        self.pending = Some((training, due(start, first)));
        self.windows = (first, RETRY);
        self.core = Some(core);
        let burst = self.burst.get_or_insert(Burst { level, quiet: 0, heard: true });
        burst.level = level;
        self.power = level.power;
    }

    /// Act on what the core has heard.
    fn listen(&mut self) {
        let Some(core) = &mut self.core else { return };
        let mut heard = Vec::new();
        while let Some(h) = core.heard() {
            heard.push(h);
        }
        for h in heard {
            match h {
                Heard::Trained { via, .. } => {
                    let end = if via == Via::Retry { self.windows.1.solve.1 } else { self.windows.0.solve.1 };
                    self.prime(end);
                }
                // Nothing fitted: the join was not one, or the burst is too
                // spoiled to train on. Either way segment 2 was heard, so
                // the burst is there until the line goes quiet, and the
                // carrier with it: a fax receiver judges a training check
                // when its carrier goes, and one that went here would be
                // judged, and answered, while the far end was still sending
                // it. The hunt is still listening, for segment 2 again after
                // a false join or for the next burst.
                Heard::Untrained => {
                    self.core = None;
                    self.pending = None;
                }
                _ => {}
            }
        }
    }

    /// Trained, and the first symbol to come is symbol `end` of the known
    /// sequence: the symbols before it are known, so the phase the first is
    /// a change from is known, and so is every bit the descrambler has been
    /// fed with since segment 4 began.
    fn prime(&mut self, end: usize) {
        let end = end.clamp(ONES_FROM + 1, KNOWN);
        self.descrambler.reset();
        for k in ONES_FROM..end {
            let (q, data) = carried(self.rate, self.known[k - 1], self.known[k]);
            for &bit in &q[data] {
                self.descrambler.descramble(bit);
            }
        }
        self.previous = Some(self.known[end - 1]);
    }

    /// Every symbol the core can make now: decided as the nearest point,
    /// which is all an uncoded constellation needs, and read as bits.
    fn symbols(&mut self) {
        let quiet = self.burst.is_none_or(|b| b.quiet > 0);
        let Some(core) = self.core.as_mut() else {
            self.show_arriving();
            return;
        };
        if !core.is_tracking() {
            self.show_arriving();
            return;
        }
        while let Some(point) = core.next() {
            let Some(symbol) = core.settle(point.nearest) else { break };
            let decided = self.labels[point.label.unwrap_or(0)];
            if let Some(previous) = self.previous.replace(decided) {
                let (q, data) = carried(self.rate, previous, decided);
                for &bit in &q[data] {
                    let out = self.descrambler.descramble(bit);
                    // Nothing once the line has gone quiet: the burst has
                    // ended, and what is decided now is the noise after it.
                    if !quiet {
                        self.bits.push(out);
                    }
                }
            }
            if !quiet {
                self.last_symbol = (symbol.point.re, symbol.point.im);
                self.residual = core.residual_error();
            }
        }
    }

    /// While the burst is heard and nothing is trained yet, the line as it
    /// arrives, a symbol apart.
    fn show_arriving(&mut self) {
        let Some(burst) = self.burst.filter(|b| b.heard && b.quiet == 0) else { return };
        self.showing -= 1.0;
        if self.showing <= 0.0 {
            self.showing += self.fs / BAUD;
            let z = self.hunt.newest().scale(1.0 / burst.level.power.max(1e-30).sqrt());
            self.last_symbol = (z.re, z.im);
        }
    }

    /// Whether the burst is still there, and the carrier with it.
    fn follow_level(&mut self) {
        let Some(burst) = &mut self.burst else { return };
        let Level { power, noise } = burst.level;
        if self.hunt.power() < noise + QUIET_BELOW * (power - noise) {
            burst.quiet += 1;
        } else {
            burst.quiet = 0;
        }
        let tracking = self.core.as_ref().is_some_and(|c| c.is_tracking() && !c.is_lost());
        if burst.quiet as f64 >= QUIET_OFF_SECONDS * self.fs {
            burst.heard = false;
        } else if burst.quiet == 0 && !burst.heard && (tracking || self.hunt.is_armed()) {
            // Back after a hole, and the signal found again across it.
            burst.heard = true;
        }
        let lost = self.core.as_ref().is_some_and(|c| c.lost_for() > LOST_OVER);
        if burst.quiet as f64 >= QUIET_OVER_SECONDS * self.fs || lost {
            self.burst = None;
            self.core = None;
            self.pending = None;
        }
    }
}

/// The half-symbol sample by which a training from `start` has all of its
/// first window in: the core's own reckoning, with a little over its
/// equaliser's reach to spare.
fn due(start: u64, window: Window) -> u64 {
    let end = window.solve.1.max(window.align.1) as u64;
    start + window.search.max(0) as u64 + 2 * end + 32
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    fn bits_of(bytes: &[u8]) -> Vec<bool> {
        bytes
            .iter()
            .flat_map(|&byte| (0..8).rev().map(move |i| byte >> i & 1 != 0))
            .collect()
    }

    fn find(haystack: &[bool], needle: &[bool]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn loopback(rate: Rate, data: &[u8]) -> Vec<bool> {
        let mut tx = Transmitter::new(FS);
        let mut rx = Receiver::new(FS);
        rx.set_rate(rate);
        tx.start(rate);
        tx.push_bytes(data);
        let mut out = Vec::new();
        let samples = (FS * 2.0) as usize
            + data.len() * 8 * (FS / f64::from(rate.bits_per_second())) as usize;
        for _ in 0..samples {
            if tx.trained() && tx.pending_bits() == 0 {
                tx.stop();
            }
            rx.feed(tx.next_sample());
            out.extend(rx.take_bits());
            if !tx.is_transmitting() && !rx.carrier() && !out.is_empty() {
                break;
            }
        }
        out
    }

    #[test]
    fn table_2_gives_the_radii_and_figures_1_to_3_the_counts() {
        let radius = |p: Point| p.power().sqrt();
        let sqrt2 = std::f64::consts::SQRT_2;
        for (eighths, outer, want) in [
            (0, false, 3.0),
            (0, true, 5.0),
            (1, false, sqrt2),
            (1, true, 3.0 * sqrt2),
        ] {
            for turn in [0, 2, 4, 6] {
                let p = Point { eighths: eighths + turn, outer };
                assert!(
                    (radius(p) - want).abs() < 1e-12,
                    "{p:?} is at {}, Table 2 says {want}",
                    radius(p)
                );
            }
        }
        assert_eq!(Rate::R9600.constellation().len(), 16);
        assert_eq!(Rate::R7200.constellation().len(), 8);
        assert_eq!(Rate::R4800.constellation().len(), 4);
        assert!(
            Rate::R4800.constellation().iter().all(|p| (radius(*p) - 3.0).abs() < 1e-12),
            "2.2.3: the amplitude is constant with a relative value of 3"
        );
    }

    #[test]
    fn the_points_are_where_the_figures_draw_them() {
        let at = |eighths, outer| Point { eighths, outer }.xy();
        assert_eq!(at(0, false), (3.0, 0.0));
        assert_eq!(at(2, true), (0.0, 5.0));
        assert_eq!(at(1, false), (1.0, 1.0));
        assert_eq!(at(3, true), (-3.0, 3.0));
        assert_eq!(at(5, false), (-1.0, -1.0));
        assert_eq!(at(7, true), (3.0, -3.0));
    }

    #[test]
    fn figure_4_puts_the_training_points_where_it_does() {
        assert_eq!(A.xy(), (-3.0, 0.0));
        assert_eq!(C.xy(), (3.0, 0.0));
        assert_eq!(b(Rate::R4800).xy(), (0.0, -3.0));
        assert_eq!(d(Rate::R4800).xy(), (0.0, 3.0));
        assert_eq!(b(Rate::R7200).xy(), (1.0, -1.0));
        assert_eq!(d(Rate::R7200).xy(), (-1.0, 1.0));
        assert_eq!(b(Rate::R9600).xy(), (3.0, -3.0));
        assert_eq!(d(Rate::R9600).xy(), (-3.0, 3.0));
        for rate in [Rate::R4800, Rate::R7200, Rate::R9600] {
            let points = rate.constellation();
            assert!(points.contains(&b(rate)) && points.contains(&d(rate)));
            assert!(points.contains(&A) && points.contains(&C));
        }
    }

    #[test]
    fn the_training_is_as_loud_as_the_data_it_trains_for() {
        // Not stated anywhere, and true at all three rates: A and B together
        // have exactly the mean power of the constellation. Which is what lets
        // a receiver's gain control settle on the training and still be right
        // for the data that follows.
        for rate in [Rate::R4800, Rate::R7200, Rate::R9600] {
            let training = (A.power() + b(rate).power()) / 2.0;
            let data = rate.rms().powi(2);
            assert!(
                (training - data).abs() < 1e-12,
                "{rate:?}: training {training}, data {data}"
            );
        }
    }

    #[test]
    fn table_5_is_608_symbols_or_253_milliseconds() {
        assert_eq!(train::TOTAL, 608);
        let ms = 1000.0 * f64::from(train::TOTAL) / BAUD;
        assert!((ms - 253.0).abs() < 1.0, "{ms}");
    }

    #[test]
    fn appendix_i_lists_these_four_conditions_and_8_2_this_start() {
        let mut g = Conditioning::new();
        let mut conditions = Vec::new();
        let mut symbols = String::new();
        for _ in 0..7 {
            conditions.push(format!("{:07b}", g.condition()));
            symbols.push(if g.next_bit() { 'D' } else { 'C' });
        }
        assert_eq!(conditions[..4], ["0101010", "1010101", "1101010", "1110101"]);
        assert_eq!(symbols, "CDCDCDC");
    }

    #[test]
    fn table_3_codes_4800_as_it_prints() {
        // Data bits to phase change: 00 none, 01 a quarter, 11 a half, 10
        // three quarters -- by way of Q4, which is Q2 plus Q3 inverted.
        //
        // The scrambler is in the way, but only after eighteen bits: an empty
        // register adds nothing to the first of them, so two bits pushed into
        // a fresh one come out as they went in.
        for (bits, turn) in [
            ([false, false], 0u8),
            ([false, true], 2),
            ([true, true], 4),
            ([true, false], 6),
        ] {
            let mut tx = Transmitter::new(FS);
            tx.start(Rate::R4800);
            tx.stage = Stage::Data;
            let before = tx.point;
            tx.push_bits(&bits);
            let after = tx.coded(false);
            assert_eq!((after.eighths + 8 - before.eighths) & 7, turn, "{bits:?}");
            assert!(!after.outer, "4800 is only ever the inner ring");
        }
    }

    #[test]
    fn a_burst_comes_back_out_at_every_rate() {
        for rate in [Rate::R9600, Rate::R7200, Rate::R4800] {
            let data = b"V.29 carries a page at twice the speed of V.27 ter.";
            let bits = loopback(rate, data);
            assert!(
                find(&bits, &bits_of(data)).is_some(),
                "{rate:?}: the message did not survive ({} bits back)",
                bits.len()
            );
        }
    }

    #[test]
    fn the_training_check_arrives_as_zeros() {
        for rate in [Rate::R9600, Rate::R7200] {
            let mut tx = Transmitter::new(FS);
            let mut rx = Receiver::new(FS);
            rx.set_rate(rate);
            tx.start(rate);
            let count = (1.5 * f64::from(rate.bits_per_second())) as usize;
            tx.push_bits(&vec![false; count]);
            let mut bits = Vec::new();
            for _ in 0..(FS * 2.5) as usize {
                rx.feed(tx.next_sample());
                bits.extend(rx.take_bits());
            }
            let mut longest = 0;
            let mut run = 0;
            for &bit in &bits {
                run = if bit { 0 } else { run + 1 };
                longest = longest.max(run);
            }
            assert!(
                longest >= count - 100,
                "{rate:?}: longest run {longest} of {count}"
            );
        }
    }

    #[test]
    fn the_carrier_comes_and_goes_with_the_burst() {
        let mut tx = Transmitter::new(FS);
        let mut rx = Receiver::new(FS);
        rx.set_rate(Rate::R9600);
        tx.start(Rate::R9600);
        tx.push_bytes(b"a short page");
        let (mut up, mut down) = (None, None);
        for i in 0..(FS * 2.0) as usize {
            if tx.trained() && tx.pending_bits() == 0 {
                tx.stop();
            }
            rx.feed(tx.next_sample());
            if up.is_none() && rx.carrier() {
                up = Some(i);
            }
            if up.is_some() && down.is_none() && !rx.carrier() {
                down = Some(i);
            }
        }
        let up = up.expect("no carrier found");
        // Segment 1 is twenty milliseconds of nothing, so not before that.
        assert!((up as f64) > FS * 0.019, "found a carrier in the silence");
        assert!((up as f64) < FS * 0.1, "took {} ms", up as f64 * 1000.0 / FS);
        assert!(down.is_some(), "the carrier never went away");
    }


    #[test]
    fn a_line_forty_decibels_down_is_the_same_line_quieter() {
        // The receiver is linear from the line to the slicer, so how loud the
        // line is should make no difference at all -- and did, because the
        // gain control started at one and was still four times too high by
        // the time the data began at forty decibels down. The equaliser had
        // learned the training at the wrong gain.
        let data = b"V.29 carries a page at twice the speed of V.27 ter.";
        let mut reference = None;
        for scale in [1.0, 0.1, 0.0316, 0.01] {
            let mut tx = Transmitter::new(FS);
            let mut rx = Receiver::new(FS);
            rx.set_rate(Rate::R9600);
            tx.start(Rate::R9600);
            tx.push_bytes(data);
            let mut out = Vec::new();
            let mut at_data = None;
            for _ in 0..(FS * 1.5) as usize {
                if tx.trained() && tx.pending_bits() == 0 {
                    tx.stop();
                }
                let was = tx.trained();
                rx.feed(tx.next_sample() * scale);
                if !was && tx.trained() && at_data.is_none() {
                    at_data = Some(rx.power / (scale * scale));
                }
                out.extend(rx.take_bits());
            }
            assert!(
                find(&out, &bits_of(data)).is_some(),
                "nothing came back at {:.0} dB",
                20.0 * scale.log10()
            );
            let power = at_data.expect("the data never started");
            let reference = *reference.get_or_insert(power);
            assert!(
                (power / reference - 1.0).abs() < 0.01,
                "at {:.0} dB the gain control had {power} where full level had {reference}",
                20.0 * scale.log10()
            );
        }
    }


    /// Run one burst through a transmitter whose carrier is `hz` off, into a
    /// receiver that started `skew` samples before it, down a line `scale`
    /// times as loud. Returns whether the data came back.
    fn survives(hz: f64, skew: usize, scale: f64) -> bool {
        let data: Vec<u8> = (0..120u32).map(|i| (i * 37 + 11) as u8).collect();
        let mut tx = Transmitter::new(FS);
        tx.nco = Nco::new(CARRIER + hz, FS);
        let mut rx = Receiver::new(FS);
        rx.set_rate(Rate::R9600);
        for _ in 0..skew {
            rx.feed(0.0);
        }
        tx.start(Rate::R9600);
        tx.push_bytes(&data);
        let mut out = Vec::new();
        for _ in 0..(FS * 1.0) as usize {
            if tx.trained() && tx.pending_bits() == 0 {
                tx.stop();
            }
            rx.feed(tx.next_sample() * scale);
            out.extend(rx.take_bits());
        }
        find(&out, &bits_of(&data)).is_some()
    }

    #[test]
    fn the_carrier_is_found_wherever_it_starts() {
        // Clause 4: a receiver "must be able to accept errors of at least +/- 7
        // Hz". And nothing makes two modems start their oscillators on the same
        // sample. Every loopback before this had both, which is why a receiver
        // that could only find a carrier it was already locked to passed every
        // one of them -- and failed forty-five of these eighty.
        for hz in [0.0, 7.0, -7.0] {
            for skew in [0, 3, 5] {
                for scale in [1.0, 0.0316] {
                    assert!(
                        survives(hz, skew, scale),
                        "{hz} Hz off, {skew} samples late, {:.0} dB",
                        20.0 * scale.log10()
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "eighty bursts; run it in release"]
    fn the_carrier_is_found_wherever_it_starts_every_way() {
        let mut failed = Vec::new();
        for hz in [0.0, 3.0, -3.0, 7.0, -7.0] {
            for skew in 0..8 {
                for scale in [1.0, 0.0316] {
                    if !survives(hz, skew, scale) {
                        failed.push((hz, skew, scale));
                    }
                }
            }
        }
        assert!(failed.is_empty(), "{} failed: {failed:?}", failed.len());
    }

    #[test]
    fn a_fresh_core_centres_its_first_half_symbol_where_the_receiver_says() {
        // The receiver hands the core a training whose start is a half-symbol
        // sample worked out from where the hunt heard the join, which rests on
        // where a fresh core puts its first one: CORE_FIRST_HALF samples in,
        // made as soon as the interpolating filter's other half is there too.
        // If the core changed that, every burst would be trained in the wrong
        // place, so it is held here.
        let mut core = Core::new(Band::new(FS, BAUD, CARRIER), Options::fixed(), table(Rate::R9600));
        let reach = CORE_FIRST_HALF as usize / 2;
        for _ in 0..CORE_FIRST_HALF as usize + reach {
            core.feed(0.0);
        }
        assert_eq!(core.halves(), 0);
        core.feed(0.0);
        assert_eq!(core.halves(), 1);
    }

    #[test]
    fn silence_is_not_a_carrier() {
        let mut rx = Receiver::new(FS);
        for _ in 0..(FS * 0.5) as usize {
            rx.feed(0.0);
        }
        assert!(!rx.carrier());
        assert!(rx.take_bits().is_empty());
    }
}
