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

use dsp::filter::OnePole;
use dsp::{ComplexFir, Equalizer, Gardner, Nco, fir_lowpass, rrc_at, rrc_taps};

use crate::v27ter::{TRIBIT_TURN, TURN_TRIBIT};
use crate::v32;

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
        }
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

/// The quietest thing that may be called a carrier, and the level it has to
/// fall below. The same floor as every other detector here.
const CARRIER_ON: f64 = 1.0e-3;
const CARRIER_OFF: f64 = 5.62e-4;

/// Twelve decibels above the quiet line to begin, twelve below the burst's
/// own loudest to end. See the V.27 ter receiver for why the end is measured
/// against the burst rather than against anything fixed.
const ON_ABOVE_FLOOR: f64 = 4.0;
const OFF_BELOW_LOUDEST: f64 = 0.25;
const LOUDEST_DECAY: f64 = 3.1e-5;
const FLOOR_FALL: f64 = 6.25e-4;
const FLOOR_RISE: f64 = 1.25e-5;

const MAX_GAIN: f64 = 400.0;

/// The least the carrier loop divides a phase error by, as a fraction of the
/// mean power.
///
/// The error is the cross product of what arrived with what it was decided to
/// be, over the decision's power -- and the inner diagonal points have a
/// seventh of the mean, so without a floor they arrive in the loop seven times
/// as loud as the rest and are the ones a slicer gets wrong most often.
const MIN_DECISION_POWER: f64 = 0.5;

/// V.29 receiver.
///
/// Like the V.27 ter one, it never decides where the synchronizing signal
/// ended. A training check is found as a run of zeros and a page by its first
/// end-of-line code, so everything is descrambled from the moment there is a
/// carrier and whoever is above finds its own place.
#[derive(Debug)]
pub struct Receiver {
    rate: Rate,
    /// The constellation divided by its root mean square, for the slicer.
    points: Vec<(Point, (f64, f64))>,
    nco: Nco,
    select: ComplexFir,
    matched: ComplexFir,
    gardner: Gardner,
    countdown: f64,
    previous_filtered: (f64, f64),
    phase: f64,
    frequency: f64,
    agc: OnePole,
    equalizer: Equalizer,
    level: OnePole,
    floor: f64,
    loudest: f64,
    carrier: bool,
    symbols: u64,
    /// The point the previous symbol was decided to be.
    previous: Option<Point>,
    descrambler: v32::Scrambler,
    bits: Vec<bool>,
    last_symbol: (f64, f64),
    track: f64,
}

impl Receiver {
    pub fn new(fs: f64) -> Self {
        let rate = Rate::default();
        let sps = fs / BAUD;
        let mut me = Self {
            rate,
            points: Vec::new(),
            nco: Nco::new(CARRIER, fs),
            // Half the baud rate and the roll-off either side of the carrier
            // is 1500 Hz at baseband; this only has to keep twice the carrier
            // out of the loops.
            select: ComplexFir::new(fir_lowpass(1600.0, 121, fs)),
            matched: ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN)),
            gardner: Gardner::new(sps, 0.1),
            countdown: sps / 2.0,
            previous_filtered: (0.0, 0.0),
            phase: 0.0,
            frequency: 0.0,
            agc: OnePole::starting_at(1.0, 0.030, BAUD),
            equalizer: Equalizer::new(31, 1.0),
            level: OnePole::new(0.010, fs),
            floor: 0.0,
            loudest: 0.0,
            carrier: false,
            symbols: 0,
            previous: None,
            descrambler: scrambler(),
            bits: Vec::new(),
            last_symbol: (0.0, 0.0),
            track: 0.0,
        };
        me.follow(rate);
        me
    }

    /// Set the rate the burst about to arrive is at, which a fax receiver
    /// always knows from the DCS that came before it.
    pub fn set_rate(&mut self, rate: Rate) {
        if rate != self.rate {
            self.follow(rate);
        }
    }

    fn follow(&mut self, rate: Rate) {
        self.rate = rate;
        let rms = rate.rms();
        self.points = rate
            .constellation()
            .into_iter()
            .map(|p| {
                let (x, y) = p.xy();
                (p, (x / rms, y / rms))
            })
            .collect();
        // The constant-modulus target: the fourth moment of the constellation
        // over the square of its second, which with the second made one is
        // the mean of the squared powers.
        let modulus = self
            .points
            .iter()
            .map(|(_, (x, y))| (x * x + y * y).powi(2))
            .sum::<f64>()
            / self.points.len() as f64;
        self.equalizer = Equalizer::new(31, modulus);
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    pub fn carrier(&self) -> bool {
        self.carrier
    }

    pub fn level(&self) -> f64 {
        self.level.value()
    }

    /// Where the last symbol landed, scaled so the mean power is one.
    pub fn constellation_point(&self) -> (f64, f64) {
        self.last_symbol
    }

    /// How far out the constellation reaches in those units.
    ///
    /// Five over the square root of 13.5 at 9600, which is a third beyond the
    /// unit circle a scope draws its box at.
    pub fn constellation_peak(&self) -> f64 {
        self.points
            .iter()
            .map(|(_, (x, y))| x.abs().max(y.abs()))
            .fold(0.0, f64::max)
    }

    pub fn residual_error(&self) -> f64 {
        self.equalizer.error()
    }

    /// The distance between the two closest points, in the same units.
    pub fn point_spacing(&self) -> f64 {
        let mut closest = f64::INFINITY;
        for (i, (_, a)) in self.points.iter().enumerate() {
            for (_, b) in self.points.iter().skip(i + 1) {
                closest = closest.min(((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt());
            }
        }
        closest
    }

    /// Forget the burst just gone, detector and all, but keep the equaliser.
    pub fn restart(&mut self) {
        self.new_burst();
        self.carrier = false;
        self.loudest = 0.0;
        self.level.reset();
    }

    fn new_burst(&mut self) {
        self.previous = None;
        self.descrambler.reset();
        self.bits.clear();
        self.symbols = 0;
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    fn nearest(&self, at: (f64, f64)) -> (Point, (f64, f64)) {
        let mut best = self.points[0];
        let mut distance = f64::INFINITY;
        for &(point, (x, y)) in &self.points {
            let d = (at.0 - x).powi(2) + (at.1 - y).powi(2);
            if d < distance {
                distance = d;
                best = (point, (x, y));
            }
        }
        best
    }

    pub fn feed(&mut self, sample: f64) {
        let (cos, sin) = self.nco.step();
        let selected = self.select.process((sample * cos, sample * -sin));
        let level = self
            .level
            .process((selected.0 * selected.0 + selected.1 * selected.1).sqrt());
        if self.carrier {
            self.loudest = self.loudest.max(level) * (1.0 - LOUDEST_DECAY);
        } else {
            self.loudest = 0.0;
            let k = if level < self.floor { FLOOR_FALL } else { FLOOR_RISE };
            self.floor += k * (level - self.floor);
        }
        let was = self.carrier;
        self.carrier = if self.carrier {
            level > (self.loudest * OFF_BELOW_LOUDEST).max(CARRIER_OFF)
        } else {
            level > (self.floor * ON_ABOVE_FLOOR).max(CARRIER_ON)
        };
        if self.carrier && !was {
            self.new_burst();
        }
        let filtered = self.matched.process(selected);

        let previous = std::mem::replace(&mut self.previous_filtered, filtered);
        let before = self.countdown;
        self.countdown -= 1.0;
        if self.countdown > 0.0 {
            return;
        }
        let mu = before.clamp(0.0, 1.0);
        let at = (
            previous.0 + mu * (filtered.0 - previous.0),
            previous.1 + mu * (filtered.1 - previous.1),
        );
        self.countdown += self.gardner.interval();
        let Some(symbol) = self.gardner.feed(at) else {
            return;
        };
        self.on_symbol(symbol);
    }

    fn on_symbol(&mut self, symbol: (f64, f64)) {
        let power = symbol.0 * symbol.0 + symbol.1 * symbol.1;
        let mean = if self.carrier {
            self.agc.process(power)
        } else {
            self.agc.value()
        };
        let gain = (1.0 / mean.max(1e-12)).sqrt().clamp(0.0, MAX_GAIN);

        let turn = self.phase * std::f64::consts::TAU;
        let (c, s) = (turn.cos(), turn.sin());
        let point = (
            (symbol.0 * c - symbol.1 * s) * gain,
            (symbol.0 * s + symbol.1 * c) * gain,
        );

        // The carrier loop reads the unequalised symbol, so the equaliser's
        // delay stays outside it. The constellation has four-fold symmetry and
        // no more -- a point turned an eighth lands between rings rather than
        // on one -- so the loop can settle a quarter turn out and no other
        // way, and the phase coding is differential exactly so that a quarter
        // turn out does not matter.
        let (_, want) = self.nearest(point);
        let d2 = (want.0 * want.0 + want.1 * want.1).max(MIN_DECISION_POWER);
        let raw = (point.1 * want.0 - point.0 * want.1) / d2;
        self.track += 0.20 * (raw - self.track);
        if self.carrier {
            self.frequency = (self.frequency - 1.5e-5 * self.track).clamp(-0.02, 0.02);
            self.phase -= 0.008 * self.track;
        }
        self.phase += self.frequency;
        self.phase -= self.phase.floor();

        let equalized = self.equalizer.equalize(point);
        let (decided, decision) = self.nearest(equalized);

        self.symbols += 1;
        if self.carrier && self.symbols > 16 {
            self.equalizer.adapt(equalized, decision);
        }
        self.last_symbol = equalized;

        if !self.carrier {
            return;
        }
        let Some(previous) = self.previous.replace(decided) else {
            return;
        };

        // 2.2 backwards: the change of phase is Q2 Q3 Q4 through Table 1,
        // and the ring is Q1.
        let change = (decided.eighths + 8 - previous.eighths) & 7;
        let tribit = TURN_TRIBIT[usize::from(change)];
        let q = [
            decided.outer,
            tribit & 0b100 != 0,
            tribit & 0b010 != 0,
            tribit & 0b001 != 0,
        ];
        let carried: &[bool] = match self.rate {
            Rate::R9600 => &q,
            Rate::R7200 => &q[1..],
            // Q4 is only the other two inverted and added, so it is not data.
            Rate::R4800 => &q[1..3],
        };
        for &bit in carried {
            let out = self.descrambler.descramble(bit);
            self.bits.push(out);
        }
    }
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
    fn silence_is_not_a_carrier() {
        let mut rx = Receiver::new(FS);
        for _ in 0..(FS * 0.5) as usize {
            rx.feed(0.0);
        }
        assert!(!rx.carrier());
        assert!(rx.take_bits().is_empty());
    }
}
