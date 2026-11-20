//! V.ARC/QAM and V.ARC/PCM: the single-carrier modes, at 16 000 baud.
//!
//! The multicarrier mode is the one that gets the most out of a line. These
//! are the ones that look like a modem: one carrier, one constellation, an
//! adaptive equaliser, sixteen thousand symbols a second where a V.34 sends
//! three thousand four hundred.
//!
//! **V.ARC/QAM** puts the carrier at 10.5 kHz, which with a root raised
//! cosine of 20% excess fills 900 Hz to 20.1 kHz: the whole of what a 48 kHz
//! converter passes cleanly. From two bits a symbol to twenty-four.
//!
//! **V.ARC/PCM** has no carrier at all. The symbols are levels, sent as they
//! are, one ladder of them from DC to 9.6 kHz: the same idea as a V.90
//! downstream, which is a sequence of PCM codewords, with a ladder of our
//! own choosing in place of G.711's. A sound card does not pass DC, so one
//! symbol in every eight gives up its sign to say whether the eight were
//! sent upside down, and they are sent whichever way up pulls the running
//! sum back towards nought: the spectral shaping of V.90 5.4.5 in its
//! simplest form.
//!
//! Both are the same machine, and the second is the first with the carrier
//! at nought and the quadrature axis left empty.
//!
//! A superframe is 4096 symbols, a quarter of a second or so:
//!
//! - a **sync word** of 128 known symbols, in four quarters. Which of the
//!   second and third quarters are sent inverted says which bit table the
//!   rest of the superframe uses. The receiver finds the word by
//!   correlation before it has an equaliser, and afterwards times itself by
//!   it: whether the word arrives early or late against the sample clock is
//!   measured *ahead* of the equaliser, where the equaliser cannot hide it;
//! - sixteen **groups** of 248 data symbols, which are blocks of
//!   Reed-Solomon codewords singly or a few together, as in the
//!   multicarrier mode.

use std::collections::VecDeque;
use std::f64::consts::TAU;

use dsp::{Complex, rrc_taps};

use crate::frame::Layout;
use crate::interp::{Interp, bessel_i0};
use crate::link::{Arrived, Bin, Health, POINTS_KEPT, Receiver, Sender, Table};
use crate::map::{BitReader, BitWriter, MAX_BITS, pam_level, pam_slice, qam_point, qam_slice};

/// Symbols a second.
pub const BAUD: f64 = 16_000.0;

/// The carrier of the QAM mode, in hertz.
const CARRIER: f64 = 10_500.0;

/// Excess bandwidth of the root raised cosine.
const ROLLOFF: f64 = 0.2;

/// Symbols the pulse runs either side of its centre.
///
/// Long, and not for the sake of the pulse. Bringing the QAM signal down
/// from its carrier leaves a second copy of it at twice the carrier, and at
/// 48 kHz that copy folds back to start 1.8 kHz above the top of the wanted
/// band. The receive filter is all that stands between the two, and a root
/// raised cosine cut off after a dozen symbols lets the copy through only
/// 40 dB down, which is a floor under everything. Twenty-four symbols under
/// a Kaiser window puts it past 80.
const HALF_SPAN: usize = 24;

/// The window's shape.
const PULSE_BETA: f64 = 8.0;

/// Symbols in a sync word, and in each of its quarters.
const SYNC: usize = 128;
const QUARTER: usize = SYNC / 4;

/// Data symbols in a group, and groups in a superframe.
const GROUP: usize = 248;
const GROUPS: usize = 16;

/// Symbols in a superframe.
const SUPER: usize = SYNC + GROUP * GROUPS;

/// Symbols that share one polarity flag in the baseband mode.
const DC_BLOCK: usize = 8;

/// Octets a block has to hold, as in the multicarrier mode.
const BLOCK_OCTETS: usize = 200;

/// The most bits a level of the baseband mode may carry.
const MAX_PAM_BITS: u8 = 12;

const DEFAULT_PARITY: u8 = 32;
const LOADED_PARITY: u8 = 16;

/// Equaliser taps, a third of a symbol apart: twenty symbols either side.
const TAPS: usize = 121;
const CENTRE: usize = TAPS / 2;

/// Symbols of settling before the line is measured, at the least, and of
/// measuring.
const SETTLE: usize = 24_000;
const MEASURE: usize = 24_000;

/// Sync words in a row that have to arrive on time before the settling is
/// taken to be over, and what on time is, in samples at 48 kHz.
const QUIET_WORDS: usize = 4;
const QUIET: f64 = 0.04;

/// How far either side of where it is expected a sync word is looked for,
/// in thirds of a symbol.
const SEARCH: usize = 6;

/// The level a transmitter sends at, RMS against full scale. One carrier
/// peaks far less than hundreds do, so this is higher than the multicarrier
/// mode's and still leaves the largest constellation's corners in hand.
pub const LEVEL: f64 = 0.2;

/// As [`crate::dmt::GAP_DB`].
pub const GAP_DB: f64 = 8.0;

/// Which single-carrier mode, at what sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub fs: f64,
    /// Samples a symbol.
    pub sps: usize,
    /// The carrier, or nought for the baseband mode.
    pub carrier: f64,
    /// Levels on one axis only: the baseband mode.
    pub real: bool,
    /// Which set of known symbols. See [`crate::dmt::Plan::coloured`].
    pub colour: u8,
}

impl Shape {
    fn at(fs: f64, real: bool) -> Option<Self> {
        let sps = (fs / BAUD).round() as usize;
        // A whole number of samples a symbol, and a multiple of three so
        // that the equaliser's three samples a symbol are whole ones too.
        if sps == 0 || (fs - sps as f64 * BAUD).abs() > 1.0 || !sps.is_multiple_of(3) {
            return None;
        }
        Some(Self { fs, sps, carrier: if real { 0.0 } else { CARRIER }, real, colour: 0 })
    }

    /// V.ARC/QAM at this sample rate, which has to be a multiple of 48 kHz.
    pub fn qam(fs: f64) -> Option<Self> {
        Self::at(fs, false)
    }

    /// V.ARC/PCM at this sample rate, which has to be a multiple of 48 kHz.
    pub fn pcm(fs: f64) -> Option<Self> {
        Self::at(fs, true)
    }

    pub fn coloured(mut self, colour: u8) -> Self {
        self.colour = colour;
        self
    }

    pub fn superframe(&self) -> f64 {
        SUPER as f64 / BAUD
    }

    pub fn default_table(&self) -> Table {
        // Half of each word is checks: the starting table has to carry the
        // first made-to-measure one over a line nothing is known about,
        // and a single carrier has no copies of anything to add together.
        let mut table = Table { parity: DEFAULT_PARITY, bits: vec![if self.real { 1 } else { 2 }] };
        let capacity = self.block(&table) * self.group_bits(self.bits_of(&table)) / 8;
        table.parity = crate::link::checks_for(0.5, capacity);
        table
    }

    fn max_bits(&self) -> u8 {
        if self.real { MAX_PAM_BITS } else { MAX_BITS }
    }

    fn bits_of(&self, table: &Table) -> u8 {
        table.bits.first().copied().unwrap_or(0).min(self.max_bits())
    }

    /// Bits a group carries with `bits` to the symbol.
    fn group_bits(&self, bits: u8) -> usize {
        if bits == 0 {
            0
        } else if self.real {
            // One sign in every eight is the polarity flag.
            GROUP / DC_BLOCK * (DC_BLOCK * usize::from(bits) - 1)
        } else {
            GROUP * usize::from(bits)
        }
    }

    /// Groups in each block sent with `table`.
    fn block(&self, table: &Table) -> usize {
        let per = self.group_bits(self.bits_of(table));
        let mut groups = 1;
        while groups < GROUPS && groups * per / 8 < BLOCK_OCTETS {
            groups *= 2;
        }
        groups
    }

    pub fn layout(&self, table: &Table) -> Option<Layout> {
        let bits = self.block(table) * self.group_bits(self.bits_of(table));
        Layout::new(bits / 8, usize::from(table.parity))
    }

    /// Bits a second `table` carries on the line, coding and all.
    pub fn gross(&self, table: &Table) -> f64 {
        (GROUPS * self.group_bits(self.bits_of(table))) as f64 / self.superframe()
    }

    /// Bits a second of frames `table` carries.
    pub fn net(&self, table: &Table) -> f64 {
        let blocks = GROUPS / self.block(table);
        self.layout(table).map_or(0.0, |l| (blocks * l.payload() * 8) as f64 / self.superframe())
    }

    /// The sync word for a mark.
    ///
    /// Four quarters, the second and third inverted or not by the mark's two
    /// bits and the fourth by both. Every mark's word is then made of the
    /// same four quarters, so whatever one word's own symbols do to the
    /// timing measured from it, they all do.
    fn sync(&self, mark: u8) -> Vec<Complex> {
        let mut state: u32 = 0x00c0_ffee ^ u32::from(self.colour).wrapping_mul(0x9e37_79b9);
        let r = std::f64::consts::FRAC_1_SQRT_2;
        let signs = quarter_signs(mark);
        (0..SYNC)
            .map(|i| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let a = if state & 0x100 != 0 { 1.0 } else { -1.0 };
                let b = if state & 0x200 != 0 { 1.0 } else { -1.0 };
                let point = if self.real { Complex::new(a, 0.0) } else { Complex::new(a * r, b * r) };
                point * signs[i / QUARTER]
            })
            .collect()
    }

    /// The shaping filter, unit power out for unit-power symbols in.
    fn pulse(&self) -> Vec<f64> {
        let mut taps = rrc_taps(self.sps as f64, ROLLOFF, 2 * HALF_SPAN);
        taps.truncate(2 * HALF_SPAN * self.sps);
        // The window takes a little off the pulse's claim to cross nought
        // at every other symbol. The equaliser is there for that.
        let mid = (HALF_SPAN * self.sps) as f64;
        for (i, tap) in taps.iter_mut().enumerate() {
            let r = (i as f64 - mid) / mid;
            *tap *= bessel_i0(PULSE_BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(PULSE_BETA);
        }
        let energy: f64 = taps.iter().map(|t| t * t).sum();
        let scale = (self.sps as f64 / energy).sqrt();
        taps.iter().map(|t| t * scale).collect()
    }

    /// The unit-power point for `value`, and the reverse.
    fn point(&self, value: u32, bits: u8) -> Complex {
        if self.real {
            Complex::new(f64::from(pam_level(value, u32::from(bits))) * pam_scale(bits), 0.0)
        } else {
            qam_point(value, bits)
        }
    }

    fn slice(&self, z: Complex, bits: u8) -> (u32, Complex) {
        if self.real {
            let scale = pam_scale(bits);
            let (value, level) = pam_slice(z.re / scale, u32::from(bits));
            (value, Complex::new(f64::from(level) * scale, 0.0))
        } else {
            qam_slice(z, bits)
        }
    }
}

fn quarter_signs(mark: u8) -> [f64; 4] {
    let a = if mark & 1 != 0 { -1.0 } else { 1.0 };
    let b = if mark & 2 != 0 { -1.0 } else { 1.0 };
    [1.0, a, b, a * b]
}

/// What to multiply a ladder's odd-integer levels by for unit power.
fn pam_scale(bits: u8) -> f64 {
    let m = (1u64 << bits) as f64;
    (3.0 / (m * m - 1.0)).sqrt()
}

/// The transmitter.
pub struct Tx {
    shape: Shape,
    sync: Vec<Vec<Complex>>,
    table: Table,
    id: u8,
    next: Option<(u8, Table)>,
    /// Which symbol of the superframe is sent next.
    at: usize,
    reader: BitReader,
    wanting: bool,
    /// Data symbols made and waiting their turn: a polarity block's worth.
    queue: VecDeque<Complex>,
    /// The running sum of every level sent, which the polarity holds down.
    sum: f64,
    /// The symbols under the pulse, newest first.
    recent: VecDeque<Complex>,
    phase: usize,
    pulse: Vec<f64>,
    turn: f64,
    step: f64,
    gain: f64,
    /// What the far end has asked the level be multiplied by; one or less.
    trim: f64,
    /// Samples that reached full scale and were held there.
    pub clipped: u64,
}

impl std::fmt::Debug for Tx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tx").field("shape", &self.shape).field("id", &self.id).finish_non_exhaustive()
    }
}

impl Tx {
    pub fn new(shape: Shape) -> Self {
        Self::at_level(shape, LEVEL)
    }

    pub fn at_level(shape: Shape, level: f64) -> Self {
        Self {
            sync: (0..4).map(|m| shape.sync(m)).collect(),
            table: shape.default_table(),
            id: 0,
            next: None,
            at: 0,
            reader: BitReader::default(),
            wanting: false,
            queue: VecDeque::new(),
            sum: 0.0,
            recent: VecDeque::from(vec![Complex::ZERO; 2 * HALF_SPAN]),
            phase: 0,
            pulse: shape.pulse(),
            turn: 0.0,
            step: TAU * shape.carrier / shape.fs,
            // The real part of a complex envelope has half its power.
            gain: if shape.real { level } else { level * std::f64::consts::SQRT_2 },
            trim: 1.0,
            clipped: 0,
            shape,
        }
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    /// The next eight levels of the baseband mode, the right way up or not.
    fn polarity_block(&mut self, bits: u8) {
        let top = 1u32 << (bits - 1);
        let mut values = [0u32; DC_BLOCK];
        for (i, value) in values.iter_mut().enumerate() {
            // The first symbol's sign is the flag, so it brings one bit
            // fewer.
            *value = if i == 0 { self.reader.take(u32::from(bits) - 1) } else { self.reader.take(u32::from(bits)) };
        }
        let level = |v: u32| f64::from(pam_level(v, u32::from(bits))) * pam_scale(bits);
        let total: f64 = values.iter().map(|&v| level(v)).sum();
        // Flipping a value's top bit mirrors its place on a Gray ladder,
        // which is to say negates the level.
        let flip = total * self.sum > 0.0 || (total == 0.0 && self.sum > 0.0);
        for value in values {
            let v = if flip { value ^ top } else { value };
            self.sum += level(v);
            self.queue.push_back(Complex::new(level(v), 0.0));
        }
    }

    fn symbol(&mut self) -> Complex {
        let bits = self.shape.bits_of(&self.table);
        let symbol = if self.at < SYNC {
            self.sync[usize::from(self.id)][self.at]
        } else {
            if self.queue.is_empty() {
                if bits == 0 {
                    self.queue.push_back(Complex::ZERO);
                } else if self.shape.real {
                    self.polarity_block(bits);
                } else {
                    let value = self.reader.take(u32::from(bits));
                    self.queue.push_back(self.shape.point(value, bits));
                }
            }
            self.queue.pop_front().expect("just filled")
        };
        if self.at < SYNC {
            self.sum += symbol.re;
        }
        self.at += 1;
        if self.at == SUPER {
            self.at = 0;
            if let Some((id, table)) = self.next.take() {
                self.id = id;
                self.table = table;
            }
        }
        // Ask for the next block's octets as the symbol before it goes out.
        let block = self.shape.block(&self.table) * GROUP;
        if self.at >= SYNC && (self.at - SYNC).is_multiple_of(block) {
            self.reader = BitReader::default();
            self.wanting = true;
        }
        symbol
    }
}

impl Sender for Tx {
    fn wants(&self) -> Option<Layout> {
        if self.wanting { self.shape.layout(&self.table) } else { None }
    }

    fn give(&mut self, line: Vec<u8>) {
        self.reader = BitReader::new(line);
        self.wanting = false;
    }

    fn next_sample(&mut self) -> f64 {
        if self.phase == 0 {
            let symbol = self.symbol();
            self.recent.pop_back();
            self.recent.push_front(symbol);
        }
        let sps = self.shape.sps;
        let mut envelope = Complex::ZERO;
        for (j, &symbol) in self.recent.iter().enumerate() {
            envelope += symbol * self.pulse[j * sps + self.phase];
        }
        self.phase = (self.phase + 1) % sps;
        let s = if self.shape.real {
            envelope.re * self.gain * self.trim
        } else {
            let (sin, cos) = self.turn.sin_cos();
            self.turn = (self.turn + self.step) % TAU;
            (envelope.re * cos - envelope.im * sin) * self.gain * self.trim
        };
        if s.abs() >= 1.0 {
            self.clipped += 1;
        }
        s.clamp(-1.0, 1.0)
    }

    fn adopt(&mut self, id: u8, mut table: Table) {
        table.bits.truncate(1);
        let least = if self.shape.real { 1 } else { 2 };
        let bits = self.shape.bits_of(&table).max(least);
        table.bits = vec![bits];
        self.next = Some((id & 3, table));
    }

    fn id(&self) -> u8 {
        self.next.as_ref().map_or(self.id, |(id, _)| *id)
    }

    fn reset(&mut self) {
        self.next = Some((0, self.shape.default_table()));
    }

    fn trim(&mut self, db: f64) {
        self.trim = 10f64.powf(-db.max(0.0) / 20.0);
    }

    fn rates(&self) -> (f64, f64) {
        (self.shape.gross(&self.table), self.shape.net(&self.table))
    }

    fn clipped(&self) -> u64 {
        self.clipped
    }
}

/// Where a receiver has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Looking for a sync word.
    Hunting,
    /// In step, and measuring the line.
    Training,
    /// In step, with the line measured.
    Steady,
}

/// A sync word found while hunting, kept until it is plain it is the peak.
#[derive(Debug, Clone, Copy)]
struct Found {
    quality: f64,
    gain: Complex,
    mark: u8,
    age: usize,
}

/// The receiver.
pub struct Rx {
    shape: Shape,
    sync: Vec<Vec<Complex>>,
    interp: Interp,
    turn: f64,
    step: f64,
    pulse: Vec<f64>,
    /// What has come off the line and down to baseband, newest last, twice
    /// over so a run of the filter never wraps.
    line: Vec<Complex>,
    line_at: usize,
    /// Samples until the matched filter is next read: it is wanted three
    /// times a symbol and the line may run at four times that.
    skip: usize,
    /// The matched filter's output at three samples a symbol, newest first.
    x: VecDeque<Complex>,

    stage: Stage,
    found: Option<Found>,
    /// Thirds of a symbol until the equaliser is next read.
    tick: usize,
    /// Which symbol of the superframe the equaliser gives next.
    at: usize,
    taps: Vec<Complex>,
    phase: f64,
    drift: f64,
    wander: f64,
    whole_sync: bool,
    /// The sync word as the equaliser gave it, against the word as sent,
    /// a quarter at a time.
    heard: [Complex; 4],
    missed: usize,
    /// Sync words in a row that arrived when expected.
    quiet: usize,

    tables: [Option<Table>; 4],
    id: Option<u8>,
    writer: BitWriter,
    /// Sliced values of the polarity block being gathered.
    block: Vec<u32>,
    groups_in: usize,
    arrived: VecDeque<Arrived>,

    noise: f64,
    measured: f64,
    trained: usize,
    proposal: Option<Table>,
    /// Bits a symbol in the table last proposed, and for how many sync
    /// words in a row the line has looked good for more.
    proposed: u8,
    better: usize,
    gap_db: f64,
    remeasure: bool,
    /// Bits a symbol and a code to ask for whatever the line measures, where
    /// somebody has said so.
    fixed: (Option<u8>, Option<f64>),
    /// Points for a symbol diagram, the bits a symbol of those shown, and
    /// the level before this one, which the baseband mode plots it against.
    points: Vec<(f64, f64)>,
    shown: u8,
    before: f64,
    /// The timing error at the last sync word, in samples.
    pub timing: f64,
    /// Times the receiver has lost the signal and gone back to hunting.
    pub losses: u64,
}

impl std::fmt::Debug for Rx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rx")
            .field("shape", &self.shape)
            .field("stage", &self.stage)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// How much of the matched filter's output is kept: a sync word and the
/// equaliser's reach beyond it.
const KEPT: usize = 3 * SYNC + TAPS + SEARCH + 8;

impl Rx {
    pub fn new(shape: Shape) -> Self {
        let pulse = shape.pulse();
        let mut tables: [Option<Table>; 4] = Default::default();
        tables[0] = Some(shape.default_table());
        Self {
            sync: (0..4).map(|m| shape.sync(m)).collect(),
            interp: Interp::new(),
            turn: 0.0,
            step: TAU * shape.carrier / shape.fs,
            line: vec![Complex::ZERO; 2 * pulse.len()],
            line_at: 0,
            skip: 0,
            pulse,
            x: VecDeque::from(vec![Complex::ZERO; KEPT]),
            stage: Stage::Hunting,
            found: None,
            tick: 0,
            at: 0,
            taps: vec![Complex::ZERO; TAPS],
            phase: 0.0,
            drift: 0.0,
            wander: 0.0,
            whole_sync: false,
            heard: [Complex::ZERO; 4],
            missed: 0,
            quiet: 0,
            tables,
            id: None,
            writer: BitWriter::default(),
            block: Vec::with_capacity(DC_BLOCK),
            groups_in: 0,
            arrived: VecDeque::new(),
            noise: 1.0,
            measured: 0.0,
            trained: 0,
            proposal: None,
            proposed: 0,
            better: 0,
            gap_db: GAP_DB,
            remeasure: false,
            fixed: (None, None),
            points: Vec::new(),
            shown: 0,
            before: 0.0,
            timing: 0.0,
            losses: 0,
            shape,
        }
    }

    pub fn shape(&self) -> &Shape {
        &self.shape
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn clock_ppm(&self) -> f64 {
        self.interp.ppm()
    }

    /// Signal to noise at the slicer, in decibels.
    pub fn snr_db(&self) -> f64 {
        -10.0 * self.noise.max(1e-30).log10()
    }

    pub fn set_gap_db(&mut self, db: f64) {
        self.gap_db = db;
    }

    /// The table this line will bear, by what has been measured.
    ///
    /// Worked out for each code there is, the stronger ones with less in
    /// hand, and the one that carries most is the one asked for: see
    /// [`crate::link::CODES`].
    pub fn load(&self) -> Table {
        let extra = self.gap_db - GAP_DB;
        let snr = 1.0 / self.noise.max(1e-30);
        let least = if self.shape.real { 1 } else { 2 };
        let mut best: Option<(f64, bool, Table)> = None;
        for (rate, gap_db) in crate::link::CODES {
            // A code that was asked for is the only one there is: the
            // nearest of these to it.
            if let Some(fixed) = self.fixed.1 {
                let nearest = crate::link::CODES
                    .iter()
                    .map(|(r, _)| r.unwrap_or(0.93))
                    .min_by(|a, b| (a - fixed).abs().total_cmp(&(b - fixed).abs()))
                    .unwrap_or(0.93);
                if (rate.unwrap_or(0.93) - nearest).abs() > 1e-9 {
                    continue;
                }
            }
            let gap = 10f64.powf((gap_db + extra) / 10.0);
            let capacity = (1.0 + snr / gap).log2();
            // A ladder on one axis holds half what a point on two does.
            let room = if self.shape.real { capacity / 2.0 } else { capacity };
            // Bits that were asked for are the bits, whatever the line says.
            let room = self.fixed.0.map_or(room, f64::from);
            // Whether the line bears even the smallest grid with this code,
            // or is being given it regardless because there is none smaller.
            let fits = room >= f64::from(least);
            let bits = (room.floor() as i64).clamp(i64::from(least), i64::from(self.shape.max_bits())) as u8;
            let mut table = Table { parity: LOADED_PARITY, bits: vec![bits] };
            if let Some(rate) = rate {
                let capacity = self.shape.block(&table) * self.shape.group_bits(bits) / 8;
                table.parity = crate::link::checks_for(rate, capacity);
            }
            let net = self.shape.net(&table);
            // A code the line bears beats one it does not, whatever each
            // would carry if it worked; then whichever carries most.
            let better = match best.as_ref() {
                None => true,
                Some((most, fitted, _)) => (fits && !fitted) || (fits == *fitted && net > *most),
            };
            if better {
                best = Some((net, fits, table));
            }
        }
        best.map(|(_, _, table)| table).expect("there are codes")
    }

    fn propose(&mut self) {
        let table = self.load();
        self.proposed = table.bits[0];
        self.better = 0;
        self.proposal = Some(table);
    }

    fn hunt_again(&mut self) {
        self.stage = Stage::Hunting;
        self.found = None;
        self.id = None;
    }

    /// The sync word against the matched filter's output, with the word's
    /// last symbol taken to be `back` samples behind the newest: each
    /// quarter's correlation, and the power under the whole word.
    fn correlate(&self, back: usize) -> ([Complex; 4], f64) {
        let word = &self.sync[0];
        let mut quarters = [Complex::ZERO; 4];
        let mut power = 0.0;
        for (i, &known) in word.iter().enumerate() {
            let y = self.x[back + 3 * (SYNC - 1 - i)];
            quarters[i / QUARTER] += y * known.conj();
            power += y.norm_sqr();
        }
        (quarters, power)
    }

    /// The word put back together from its quarters, whichever way up each
    /// was sent, and the mark that says.
    fn combine(quarters: &[Complex; 4]) -> (Complex, u8) {
        let a = (quarters[1] * quarters[0].conj()).re < 0.0;
        let b = (quarters[2] * quarters[0].conj()).re < 0.0;
        let mark = u8::from(a) | u8::from(b) << 1;
        let signs = quarter_signs(mark);
        let mut sum = Complex::ZERO;
        for (q, s) in quarters.iter().zip(signs) {
            sum += *q * s;
        }
        (sum, mark)
    }

    fn hunt(&mut self) {
        let (quarters, power) = self.correlate(0);
        let (sum, mark) = Self::combine(&quarters);
        let quality = if power > 1e-12 { sum.norm_sqr() / (SYNC as f64 * power) } else { 0.0 };
        let better = self.found.is_none_or(|f| quality > f.quality);
        if quality > 0.5 && better {
            self.found = Some(Found { quality, gain: sum / SYNC as f64, mark, age: 0 });
            return;
        }
        let Some(found) = self.found.as_mut() else { return };
        found.age += 1;
        if found.age < 3 {
            return;
        }
        // That was the peak, three samples ago: a symbol's worth, so the
        // equaliser is read now and every third sample after. Its centre tap
        // looks ten symbols back, at symbol 118 of the word just found.
        let found = *found;
        self.found = None;
        self.taps.fill(Complex::ZERO);
        self.taps[CENTRE] = found.gain.conj() / found.gain.norm_sqr();
        self.tick = 0;
        self.at = SYNC - CENTRE / 3;
        self.phase = 0.0;
        self.drift = 0.0;
        self.wander = 0.0;
        self.whole_sync = false;
        self.missed = 0;
        self.quiet = 0;
        self.id = Some(found.mark);
        self.stage = Stage::Training;
        self.trained = 0;
        self.measured = 0.0;
        self.noise = 1.0;
        self.block.clear();
        self.groups_in = 0;
        self.writer = BitWriter::default();
    }

    /// One more sample of the matched filter's output.
    fn third(&mut self, y: Complex) {
        self.x.pop_back();
        self.x.push_front(y);
        if self.stage == Stage::Hunting {
            self.hunt();
            if self.stage == Stage::Hunting {
                return;
            }
        }
        if self.tick > 0 {
            self.tick -= 1;
            return;
        }
        self.tick = 2;
        self.symbol();
    }

    fn rate(&self) -> f64 {
        match self.stage {
            // Fast while the clocks are still sliding: the taps have to
            // follow the signal across them until the timing loop stops it.
            Stage::Training if self.trained < 4_000 || self.quiet < 2 => 0.2,
            Stage::Training if self.trained < SETTLE => 0.05,
            Stage::Training => 0.02,
            _ => 0.005,
        }
    }

    fn symbol(&mut self) {
        let mut z = Complex::ZERO;
        let mut power = 0.0;
        for (i, &tap) in self.taps.iter().enumerate() {
            z += tap * self.x[i];
            power += self.x[i].norm_sqr();
        }
        let turn = Complex::from_polar(1.0, -self.phase);
        let mut seen = z * turn;
        seen.re -= self.wander;

        let id = usize::from(self.id.unwrap_or(0));
        let table = self.tables[id].clone();
        let in_sync = self.at < SYNC;
        if in_sync {
            if self.at == 0 {
                self.heard = [Complex::ZERO; 4];
            }
            self.heard[self.at / QUARTER] += seen * self.sync[0][self.at].conj();
        }
        let least = if self.shape.real { 1 } else { 2 };
        let bits = if in_sync { least } else { table.as_ref().map_or(0, |t| self.shape.bits_of(t)) };

        // The diagram shows the data. The sync word is on the smallest grid
        // whatever the data is on, so it is shown only until there is data.
        if bits > 0 && (!in_sync || table.is_none()) {
            if bits != self.shown {
                self.shown = bits;
                self.points.clear();
            }
            if self.shape.real {
                // Levels have no plane: each against the one before it.
                let level = seen.re / (pam_scale(bits) * ((1u64 << bits) - 1) as f64);
                if self.points.len() < POINTS_KEPT {
                    self.points.push((self.before, level));
                }
                self.before = level;
            } else if self.points.len() < POINTS_KEPT {
                let (bi, _) = crate::map::split(bits);
                let reach = crate::map::qam_scale(bits) * ((1u64 << bi) - 1) as f64;
                self.points.push((seen.re / reach, seen.im / reach));
            }
        }

        if bits > 0 {
            let (value, point) = self.shape.slice(seen, bits);
            let mut error = point - seen;
            if self.shape.real {
                error.im = 0.0;
            }
            let e2 = error.norm_sqr();
            if self.stage == Stage::Training && self.trained >= SETTLE {
                self.measured += e2;
            } else if self.stage == Stage::Steady {
                self.noise += (e2 - self.noise) / 8192.0;
            }
            // The equaliser sees the error turned back to where it works.
            let rate = self.rate();
            let push = error * turn.conj() * (rate / (power + 1e-12));
            for (i, tap) in self.taps.iter_mut().enumerate() {
                // The faintest of leaks. Three taps a symbol is more than the
                // signal's band needs, so there are settings of them that the
                // signal cannot tell apart and the taps are free to wander
                // among; this is what stops them. It also pulls against the
                // signal, so it is kept far below what the adaptation pushes.
                *tap = *tap * (1.0 - 1e-9) + push * self.x[i].conj();
            }
            if self.shape.real {
                self.wander -= 0.002 * error.re;
            } else {
                // A second-order loop about ten hertz wide on whatever
                // rotation is left.
                let lean = (seen * point.conj()).im / point.norm_sqr().max(0.3);
                self.drift += 1.5e-5 * lean;
                self.phase = (self.phase + 5.5e-3 * lean + self.drift) % TAU;
            }
            if !in_sync {
                self.data(value, bits);
            }
            // Measuring waits for the timing to have come to rest: an
            // equaliser still chasing a sliding clock reads as noise.
            let waiting = self.trained + 1 == SETTLE && self.quiet < QUIET_WORDS;
            if self.stage == Stage::Training && !waiting {
                self.trained += 1;
                if self.trained == SETTLE + MEASURE {
                    self.noise = (self.measured / MEASURE as f64).max(1e-30);
                    self.stage = Stage::Steady;
                    self.propose();
                }
            }
        } else if self.stage == Stage::Training {
            // Data sent with a table this end has never seen. Nothing to
            // slice it against, so nothing to learn from it.
            self.trained = 0;
            self.measured = 0.0;
        }

        self.at += 1;
        if self.at == SYNC {
            self.end_of_sync();
        } else if self.at >= SYNC && (self.at - SYNC).is_multiple_of(GROUP) {
            self.end_of_group();
        }
        if self.at == SUPER {
            self.at = 0;
        }
    }

    fn data(&mut self, value: u32, bits: u8) {
        if !self.shape.real {
            self.writer.put(value, u32::from(bits));
            return;
        }
        self.block.push(value);
        if self.block.len() < DC_BLOCK {
            return;
        }
        let top = 1u32 << (bits - 1);
        let flip = if self.block[0] & top != 0 { top } else { 0 };
        for (i, &v) in self.block.iter().enumerate() {
            let v = v ^ flip;
            if i == 0 {
                self.writer.put(v, u32::from(bits) - 1);
            } else {
                self.writer.put(v, u32::from(bits));
            }
        }
        self.block.clear();
    }

    fn end_of_group(&mut self) {
        let id = usize::from(self.id.unwrap_or(0));
        let table = self.tables[id].as_ref();
        self.groups_in += 1;
        let block = table.map_or(GROUPS, |t| self.shape.block(t));
        if self.groups_in < block {
            return;
        }
        self.groups_in = 0;
        match table.and_then(|t| self.shape.layout(t)) {
            Some(layout) => {
                let line = self.writer.finish(layout.capacity());
                self.arrived.push_back(Arrived::Octets { line, layout, erased: None });
            }
            None => {
                self.writer = BitWriter::default();
                self.arrived.push_back(Arrived::Lost);
            }
        }
    }

    fn end_of_sync(&mut self) {
        self.block.clear();
        self.groups_in = 0;
        self.writer = BitWriter::default();
        if !self.whole_sync {
            // The tail of the word the hunt found: the mark is known from
            // the hunt and the timing is as good as the hunt made it.
            self.whole_sync = true;
            return;
        }
        // Everything about the word is read ahead of the equaliser, where
        // the hunt read it: whether it is there at all, which mark it
        // carries, and whether it came early or late against the sample
        // clock. The equaliser would simply follow a word that slid, and
        // say nothing until it ran out of taps; and while the clocks are
        // still being pulled together its own output is no judge of
        // anything.
        //
        // The word's last symbol is under the equaliser's centre tap now.
        // Newer samples are nearer the front, so a word that arrived late
        // is found at a smaller index than expected.
        let at = CENTRE;
        let looks: Vec<(f64, u8, f64)> = (at - SEARCH..=at + SEARCH)
            .map(|back| {
                let (quarters, power) = self.correlate(back);
                let (sum, mark) = Self::combine(&quarters);
                (sum.abs(), mark, power)
            })
            .collect();
        let (best, &(size, mark, power)) =
            looks.iter().enumerate().max_by(|a, b| a.1.0.total_cmp(&b.1.0)).expect("a range");
        if power < 1e-12 || size * size / (SYNC as f64 * power) < 0.3 {
            self.missed += 1;
            self.quiet = 0;
            if self.missed >= 3 {
                self.losses += 1;
                self.hunt_again();
            }
            return;
        }
        self.missed = 0;
        self.id = Some(mark);
        if self.remeasure && mark == 0 && self.stage == Stage::Steady {
            self.remeasure = false;
            self.stage = Stage::Training;
            self.trained = SETTLE - 1;
            self.measured = 0.0;
        }
        self.upright(mark);
        let best = best.clamp(1, 2 * SEARCH - 1);
        let (later, on, earlier) = (looks[best - 1].0, looks[best].0, looks[best + 1].0);
        let bend = 2.0 * on - earlier - later;
        if bend <= 0.0 {
            return;
        }
        // In samples at the line's rate: positive when the word peaked
        // later than the sample it was expected on.
        let thirds = SEARCH as f64 - best as f64 + 0.5 * (later - earlier) / bend;
        let offset = thirds * (self.shape.sps / 3) as f64;
        self.timing = offset;
        self.quiet = if offset.abs() < QUIET * self.shape.fs / 48_000.0 { self.quiet + 1 } else { 0 };
        // Three settings of a second-order loop. Until a word has arrived on
        // time, take out the whole error and half of it again from the rate,
        // which halves what is left at every word; then more gently; and
        // once the line is measured, so gently that what noise there is in
        // the measurement does not reach the equaliser as jitter.
        let (now, slow) = match self.stage {
            Stage::Steady => (0.05, 0.004),
            _ if self.quiet == 0 => (1.0, 0.5),
            _ => (0.5, 0.1),
        };
        self.interp.nudge(now * offset);
        self.interp.adjust_step(slow * offset / (SUPER * self.shape.sps) as f64);

        // The first measurement is taken as soon as the timing has come to
        // rest, and an equaliser with this many taps can go on improving for
        // seconds after that. A line that has looked good for more bits for
        // two seconds running is asked for them.
        if self.stage == Stage::Steady {
            if self.load().bits[0] > self.proposed {
                self.better += 1;
                if self.better >= 8 {
                    self.propose();
                }
            } else {
                self.better = 0;
            }
        }
    }

    /// Turn the constellation the right way up, by the sync word.
    ///
    /// An equaliser and a phase loop steered by their own decisions are
    /// content with a constellation a quarter turn round, or a ladder upside
    /// down: every decision is as confident as before and every one is
    /// wrong. They can be left that way round by a clock that slid while
    /// they were settling. The sync word is the one stretch of signal whose
    /// symbols are known, so it says which way up things are.
    fn upright(&mut self, mark: u8) {
        let signs = quarter_signs(mark);
        let mut sum = Complex::ZERO;
        for (q, s) in self.heard.iter().zip(signs) {
            sum += *q * s;
        }
        if self.shape.real {
            if sum.re < 0.0 {
                for tap in &mut self.taps {
                    *tap = -*tap;
                }
                self.wander = -self.wander;
            }
        } else if sum.arg().abs() > 0.3 {
            self.phase = (self.phase + sum.arg()).rem_euclid(TAU);
        }
    }

    /// One sample on the transmitter's clock.
    fn sample(&mut self, s: f64) {
        let y = if self.shape.real {
            Complex::new(s, 0.0)
        } else {
            let (sin, cos) = self.turn.sin_cos();
            self.turn = (self.turn + self.step) % TAU;
            // Twice, so that what is left after the filter takes away the
            // image at twice the carrier is the envelope and not half of it.
            Complex::new(2.0 * s * cos, -2.0 * s * sin)
        };
        let len = self.pulse.len();
        self.line[self.line_at] = y;
        self.line[self.line_at + len] = y;
        self.line_at = (self.line_at + 1) % len;
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        self.skip = self.shape.sps / 3 - 1;
        let window = &self.line[self.line_at..self.line_at + len];
        let mut out = Complex::ZERO;
        for (&v, &h) in window.iter().zip(self.pulse.iter().rev()) {
            out += v * h;
        }
        self.third(out / self.shape.sps as f64);
    }
}

impl Receiver for Rx {
    fn feed(&mut self, sample: f64) {
        let mut out = [0.0; 2];
        let mut count = 0;
        self.interp.feed(sample, |y| {
            if count < 2 {
                out[count] = y;
                count += 1;
            }
        });
        for &y in &out[..count] {
            self.sample(y);
        }
    }

    fn take(&mut self) -> Option<Arrived> {
        self.arrived.pop_front()
    }

    fn proposal(&mut self) -> Option<Table> {
        self.proposal.take()
    }

    fn expect(&mut self, id: u8, table: Table) {
        self.tables[usize::from(id & 3)] = Some(table);
    }

    fn id(&self) -> Option<u8> {
        self.id
    }

    fn back_off(&mut self, db: f64) {
        self.gap_db += db;
        if self.stage == Stage::Steady {
            self.propose();
        }
    }

    fn remeasure(&mut self, db: f64) {
        self.gap_db += db;
        self.remeasure = true;
    }

    fn health(&self) -> Health {
        let steady = self.stage == Stage::Steady;
        Health {
            locked: self.stage != Stage::Hunting,
            steady,
            snr_db: if steady { self.snr_db() } else { 0.0 },
            clock_ppm: self.clock_ppm(),
        }
    }

    fn rates(&self) -> (f64, f64) {
        match self.id.and_then(|id| self.tables[usize::from(id)].as_ref()) {
            Some(table) => (self.shape.gross(table), self.shape.net(table)),
            None => (0.0, 0.0),
        }
    }

    fn code(&self) -> Option<(usize, usize)> {
        let table = self.id.and_then(|id| self.tables[usize::from(id)].as_ref())?;
        self.shape.layout(table).map(|layout| layout.code())
    }

    fn points(&mut self) -> Vec<(f64, f64)> {
        std::mem::take(&mut self.points)
    }

    fn shown_bits(&self) -> u8 {
        self.shown
    }

    fn fix(&mut self, bits: Option<u8>, code: Option<f64>) {
        let least = if self.shape.real { 1 } else { 2 };
        self.fixed = (bits.map(|b| b.clamp(least, self.shape.max_bits())), code);
        if self.stage == Stage::Steady {
            self.propose();
        }
    }

    fn spectrum(&self) -> Vec<Bin> {
        let table = self.id.and_then(|id| self.tables[usize::from(id)].as_ref());
        vec![Bin {
            hz: self.shape.carrier,
            snr_db: if self.stage == Stage::Steady { self.snr_db() } else { 0.0 },
            bits: table.map_or(0, |t| self.shape.bits_of(t)),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::Station;

    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gaussian(&mut self) -> f64 {
            (-2.0 * self.uniform().ln()).sqrt() * (TAU * self.uniform()).cos()
        }
    }

    fn stream(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + seed * 17 + (i >> 5) * 3 + (i >> 11)) as u8).collect()
    }

    /// Two ends, a wire each way with noise and a clock of its own, and a
    /// coupling capacitor if asked for.
    struct Pair {
        a: Station<Tx, Rx>,
        b: Station<Tx, Rx>,
        clocks: [Interp; 2],
        slack: [VecDeque<f64>; 2],
        blocked: [(f64, f64); 2],
        corner: f64,
        noise: Noise,
        sigma: f64,
        from: [f64; 2],
        at_a: Vec<u8>,
        at_b: Vec<u8>,
        fs: f64,
    }

    impl Pair {
        fn new(shape: Shape, snr_db: f64, ppm: f64, highpass_hz: f64) -> Self {
            let fs = shape.fs;
            let end = |tx: u8, rx: u8| {
                Station::new(fs, vec![(Tx::new(shape.clone().coloured(tx)), Rx::new(shape.clone().coloured(rx)))])
            };
            let mut clocks = [Interp::new(), Interp::new()];
            clocks[0].adjust_step(ppm * 1e-6);
            clocks[1].adjust_step(-ppm * 1e-6);
            Self {
                a: end(0, 1),
                b: end(1, 0),
                clocks,
                slack: [VecDeque::from(vec![0.0; 256]), VecDeque::from(vec![0.0; 256])],
                blocked: [(0.0, 0.0); 2],
                corner: (-TAU * highpass_hz / fs).exp(),
                noise: Noise(5),
                sigma: LEVEL * 10f64.powf(-snr_db / 20.0),
                from: [0.0; 2],
                at_a: Vec::new(),
                at_b: Vec::new(),
                fs,
            }
        }

        fn carry(&mut self, way: usize, x: f64) -> f64 {
            // One pole of high-pass: what a coupling capacitor does.
            let (last_in, last_out) = self.blocked[way];
            let passed = if self.corner < 1.0 { self.corner * (last_out + x - last_in) } else { x };
            self.blocked[way] = (x, passed);
            let (slack, noise, sigma) = (&mut self.slack[way], &mut self.noise, self.sigma);
            self.clocks[way].feed(passed, |y| slack.push_back(y + sigma * noise.gaussian()));
            self.slack[way].pop_front().expect("the wire ran dry")
        }

        fn run(&mut self, seconds: f64) {
            for _ in 0..(seconds * self.fs) as usize {
                let to_b = self.carry(0, self.from[0]);
                let to_a = self.carry(1, self.from[1]);
                let (mut out_a, mut out_b) = ([0.0], [0.0]);
                self.a.step(&[to_a], &mut out_a);
                self.b.step(&[to_b], &mut out_b);
                self.from = [out_a[0], out_b[0]];
                self.at_a.extend(self.a.read());
                self.at_b.extend(self.b.read());
            }
        }
    }

    #[test]
    fn the_sync_words_differ_only_in_which_quarters_are_upside_down() {
        let shape = Shape::qam(48_000.0).unwrap();
        let plain = shape.sync(0);
        for mark in 1..4 {
            let word = shape.sync(mark);
            let signs = quarter_signs(mark);
            for (i, (&w, &p)) in word.iter().zip(&plain).enumerate() {
                assert_eq!(w, p * signs[i / QUARTER]);
            }
        }
        assert!(Shape::qam(44_100.0).is_none());
        assert!(Shape::pcm(32_000.0).is_none());
        assert_eq!(Shape::pcm(192_000.0).unwrap().sps, 12);
    }

    #[test]
    fn the_baseband_mode_sends_no_dc_whatever_it_is_given() {
        let shape = Shape::pcm(48_000.0).unwrap();
        let mut tx = Tx::new(shape.clone());
        tx.adopt(1, Table { parity: 16, bits: vec![6] });
        let mut worst: f64 = 0.0;
        for _ in 0..(SUPER * 3 * 12) {
            if tx.wants().is_some() {
                // All ones, which unshaped would be the same level for ever.
                tx.give(vec![0xff; 4000]);
            }
            tx.next_sample();
            worst = worst.max(tx.sum.abs());
        }
        assert_eq!(tx.id(), 1);
        // The sum of every level ever sent never gets beyond one block's.
        assert!(worst < 2.0 * DC_BLOCK as f64, "running sum reached {worst}");
    }

    #[test]
    fn qam_measures_a_line_loads_it_and_carries_a_stream_each_way() {
        for &(snr, ppm) in &[(45.0, 0.0), (45.0, 70.0), (62.0, -40.0)] {
            let mut pair = Pair::new(Shape::qam(48_000.0).unwrap(), snr, ppm, 0.0);
            let (to_b, to_a) = (stream(1, 200_000), stream(2, 60_000));
            pair.a.write(&to_b);
            pair.b.write(&to_a);
            pair.run(22.0);
            let rx = &pair.b.channels[0].rx;
            // The noise is spread over the whole 24 kHz and the receive
            // filter passes 16 kHz of it.
            let at_slicer = snr + 10.0 * (24_000.0 / BAUD).log10();
            assert!(
                (rx.snr_db() - at_slicer).abs() < 1.5,
                "expected {at_slicer:.1}, measured {:.1} at {ppm} ppm",
                rx.snr_db()
            );
            assert!((rx.clock_ppm() - ppm).abs() < 2.0, "asked {ppm}, found {:.2}", rx.clock_ppm());
            let bits = pair.a.channels[0].tx.table().bits[0];
            let expect = ((1.0 + 10f64.powf((at_slicer - GAP_DB) / 10.0)).log2()).floor() as u8;
            let b = &pair.b.channels[0];
            assert!(
                bits.abs_diff(expect) <= 1,
                "{bits} bits at {snr} dB {ppm} ppm, expected {expect}: {:?}, clipped {}, losses {}, got {}",
                b.tally,
                pair.a.channels[0].tx.clipped,
                b.rx.losses,
                pair.at_b.len()
            );
            assert!(pair.at_b == to_b, "{} of {} octets at {snr} dB, {ppm} ppm", pair.at_b.len(), to_b.len());
            assert!(pair.at_a == to_a);
            // A block or two may go by before the sync word has said which
            // way up the constellation is.
            assert!(pair.b.channels[0].tally.failed < 10, "{snr} dB, {ppm} ppm");
        }
    }

    /// A line with 8 dB at the slicer, where a code of sixteen check octets
    /// carries nothing: a sixteenth of four points' worth come out wrong.
    /// The receiver asks for a code that is mostly checks, and the stream
    /// arrives.
    #[test]
    fn a_noisy_line_gets_a_code_that_is_mostly_checks() {
        // 8 dB at the slicer is 6.2 dB over the whole band.
        let mut pair = Pair::new(Shape::qam(48_000.0).unwrap(), 6.2, 0.0, 0.0);
        let to_b = stream(3, 8_000);
        pair.a.write(&to_b);
        pair.run(30.0);
        let rx = &pair.b.channels[0].rx;
        assert!((rx.snr_db() - 8.0).abs() < 1.5, "measured {:.1} dB", rx.snr_db());
        let table = pair.a.channels[0].tx.table().clone();
        let layout = rx.shape().layout(&table).expect("a layout");
        let (n, k) = layout.code();
        assert_eq!(table.bits[0], 2);
        assert!(k * 2 <= n + 2, "a code of {k}/{n} at 8 dB");
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
    }

    #[test]
    fn pcm_carries_a_stream_through_a_coupling_capacitor() {
        for &(snr, ppm) in &[(40.0, 0.0), (55.0, 50.0)] {
            let mut pair = Pair::new(Shape::pcm(48_000.0).unwrap(), snr, ppm, 10.0);
            let (to_b, to_a) = (stream(3, 80_000), stream(4, 40_000));
            pair.a.write(&to_b);
            pair.b.write(&to_a);
            pair.run(22.0);
            let bits = pair.a.channels[0].tx.table().bits[0];
            assert!(bits >= 4, "{bits} bits a level at {snr} dB");
            let b = &pair.b.channels[0];
            assert!(
                pair.at_b == to_b,
                "{} of {} octets at {snr} dB, {ppm} ppm: {bits} bits, {:?}, snr {:.1}, resent {}",
                pair.at_b.len(),
                to_b.len(),
                b.tally,
                b.rx.snr_db(),
                pair.a.arq.resent
            );
            let a = &pair.a.channels[0];
            assert!(
                pair.at_a == to_a,
                "{} of {} octets back at {snr} dB, {ppm} ppm: {} bits, {:?}, snr {:.1}, clock {:.1}, losses {}",
                pair.at_a.len(),
                to_a.len(),
                pair.b.channels[0].tx.table().bits[0],
                a.tally,
                a.rx.snr_db(),
                a.rx.clock_ppm(),
                a.rx.losses
            );
        }
    }

    #[test]
    fn the_modes_run_at_four_times_the_rate_too() {
        let mut pair = Pair::new(Shape::qam(192_000.0).unwrap(), 50.0, 20.0, 0.0);
        let to_b = stream(5, 60_000);
        pair.a.write(&to_b);
        pair.run(12.0);
        let b = &pair.b.channels[0];
        assert!(
            pair.at_b == to_b,
            "{} of {} octets: {:?}, snr {:.1}, clock {:.1}",
            pair.at_b.len(),
            to_b.len(),
            b.tally,
            b.rx.snr_db(),
            b.rx.clock_ppm()
        );
    }
}
