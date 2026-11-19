//! V.ARC/DMT: the multicarrier mode, loaded a carrier at a time.
//!
//! A sound card joined to another by a cable is nothing like a telephone
//! line. It is ten times as wide, it is quiet beyond anything a line code for
//! telephones was built to exploit, and it is not at all the same from one
//! end of its band to the other: flat through the middle, falling away at the
//! top where the converters' filters are, and useless at the very bottom where
//! the coupling capacitors are. The way to use a channel like that is the way
//! ADSL uses a pair of wires. Cut the band into a few hundred narrow carriers,
//! find out how quiet each one is, and put on each as many bits as it will
//! bear.
//!
//! The line is a run of *symbols*, each `n` samples of signal with the last
//! sixteenth of itself repeated in front as a guard, so that whatever the
//! cable and the filters smear out of one symbol lands in the next one's
//! guard and nowhere that matters. Thirty-three symbols make a *superframe*:
//!
//! - one **sync symbol**, every carrier sending a point both ends know. It is
//!   how a receiver finds the superframe, and which of four sequences it is
//!   sent with says which bit table the thirty-two that follow were sent
//!   with, so a change of table needs no countdown;
//! - thirty-two **data symbols**. A carrier with bits to carry carries them
//!   as a QAM point. A carrier with none, and every thirty-second carrier
//!   regardless, sends a known point, which is what the receiver times
//!   itself against and how it goes on measuring carriers nothing is using.
//!
//! The octets do not wait for a superframe. The data symbols are taken in
//! *groups*, each group a block of Reed-Solomon codewords on its own, and a
//! group is as few symbols as hold a codeword's worth: one, on a line that
//! has been measured and loaded, so that an octet is on its way a fiftieth
//! of a second after it was offered and not three quarters of a second.
//!
//! Nothing is modulated onto a carrier frequency: the signal is real and at
//! baseband, so there is no frequency offset between two ends to find, only
//! the difference between their sample clocks. See [`crate::interp`].

use std::collections::VecDeque;
use std::f64::consts::TAU;

use dsp::{Complex, Fft};

use crate::frame::Layout;
use crate::interp::Interp;
use crate::link::{Arrived, Bin, Health, Receiver, Sender, Table};
use crate::map::{BitReader, BitWriter, MAX_BITS, qam_point, qam_slice};

/// How many bit tables can be told apart by the sync symbol. Table zero is
/// the one both ends start with.
pub const TABLES: usize = 4;

/// Every carrier whose number leaves this remainder over [`PILOT_EVERY`]
/// carries a known point in every symbol.
const PILOT_AT: usize = 16;
const PILOT_EVERY: usize = 32;

/// Check octets a codeword gets under the starting table, which has to work
/// before anything is known about the line.
const DEFAULT_PARITY: u8 = 32;

/// And under a table made to measure.
const LOADED_PARITY: u8 = 16;

/// Octets a group of symbols has to hold before it is a block by itself.
const GROUP_OCTETS: usize = 200;

/// Symbols spent letting the channel estimate settle before measuring.
const SETTLE: usize = 44;

/// Symbols the noise on each carrier is measured over: two seconds at
/// 48 kHz, which puts each carrier's figure within about half a decibel.
const MEASURE: usize = 88;

/// The shape of the signal at one sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub fs: f64,
    /// Samples in a symbol's body, and so twice the number of carriers.
    pub n: usize,
    /// Samples of guard in front of each body.
    pub cp: usize,
    /// Data symbols in a superframe.
    pub symbols: usize,
    /// Which set of known points this line uses. See [`Plan::coloured`].
    pub colour: u8,
}

impl Plan {
    /// The plan for a sample rate: carriers 30 to 50 Hz apart whatever the
    /// rate, so a symbol is always about a fiftieth of a second.
    pub fn for_rate(fs: f64) -> Self {
        let n = if fs < 30_000.0 {
            512
        } else if fs < 60_000.0 {
            1024
        } else if fs < 120_000.0 {
            2048
        } else {
            4096
        };
        Self { fs, n, cp: n / 16, symbols: 32, colour: 0 }
    }

    /// The same plan with a different set of known points.
    ///
    /// Two lines that run side by side -- the two channels of a stereo lead
    /// -- leak into each other, a little, and what leaks is the other
    /// line's signal. If both were sending the same known points, in step,
    /// the leak would be a faint copy of what the receiver was expecting and
    /// would measure as a slightly stronger line; then data would start,
    /// the two would differ, and the leak would be noise nobody had allowed
    /// for. So each line gets its own points, and the leak is noise from the
    /// first symbol, there to be measured.
    pub fn coloured(mut self, colour: u8) -> Self {
        self.colour = colour;
        self
    }

    /// Carriers there are numbers for; carrier zero is DC and never used.
    pub fn half(&self) -> usize {
        self.n / 2
    }

    /// Samples from one symbol to the next.
    pub fn span(&self) -> usize {
        self.n + self.cp
    }

    pub fn frequency(&self, carrier: usize) -> f64 {
        carrier as f64 * self.fs / self.n as f64
    }

    /// The highest carrier that is ever given bits.
    ///
    /// The receiver's resampler is flat to about 0.87 of Nyquist and above
    /// that its response moves as the two ends' clocks slide past each
    /// other. On average those carriers look usable; for the few symbols in
    /// each slide when they are at their worst they are not, and the errors
    /// come all at once. Converters have little to offer up there anyway.
    pub fn top(&self) -> usize {
        self.half() * 43 / 50
    }

    pub fn is_pilot(carrier: usize) -> bool {
        carrier % PILOT_EVERY == PILOT_AT
    }

    /// Seconds a superframe lasts.
    pub fn superframe(&self) -> f64 {
        ((self.symbols + 1) * self.span()) as f64 / self.fs
    }

    /// The table both ends start with: two bits on every carrier between
    /// 500 Hz and 6 kHz, which any audio path at all will pass.
    pub fn default_table(&self) -> Table {
        let bits = (0..self.half())
            .map(|k| {
                let f = self.frequency(k);
                u8::from((500.0..=6000.0).contains(&f) && !Self::is_pilot(k)) * 2
            })
            .collect();
        Table { parity: DEFAULT_PARITY, bits }
    }

    /// Data symbols in each group sent with `table`: the fewest, in powers
    /// of two, that between them hold most of a codeword.
    pub fn group(&self, table: &Table) -> usize {
        let mut group = 1;
        while group < self.symbols && group * table.total() / 8 < GROUP_OCTETS {
            group *= 2;
        }
        group
    }

    /// How a group sent with `table` is laid out, if it holds anything.
    pub fn layout(&self, table: &Table) -> Option<Layout> {
        Layout::new(self.group(table) * table.total() / 8, usize::from(table.parity))
    }

    /// Bits a second `table` carries on the line, coding and all.
    pub fn gross(&self, table: &Table) -> f64 {
        (self.symbols * table.total()) as f64 / self.superframe()
    }

    /// Bits a second of frames `table` carries.
    pub fn net(&self, table: &Table) -> f64 {
        let groups = self.symbols / self.group(table);
        self.layout(table).map_or(0.0, |l| (groups * l.payload() * 8) as f64 / self.superframe())
    }

    /// The known points: a row for each sync sequence, then a row for each
    /// data symbol of a superframe.
    fn known(&self) -> Vec<Vec<Complex>> {
        let mut state: u32 = 0x5eed_a5c1 ^ u32::from(self.colour).wrapping_mul(0x9e37_79b9);
        let r = std::f64::consts::FRAC_1_SQRT_2;
        (0..TABLES + self.symbols)
            .map(|_| {
                (0..self.half())
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        let i = if state & 0x100 != 0 { r } else { -r };
                        let q = if state & 0x200 != 0 { r } else { -r };
                        Complex::new(i, q)
                    })
                    .collect()
            })
            .collect()
    }
}

/// The transmitter.
pub struct Tx {
    plan: Plan,
    fft: Fft,
    known: Vec<Vec<Complex>>,
    table: Table,
    id: u8,
    /// A table to change to at the next superframe.
    next: Option<(u8, Table)>,
    /// Which symbol of the superframe is built next; zero is the sync.
    symbol: usize,
    reader: BitReader,
    wanting: bool,
    out: Vec<f64>,
    cursor: usize,
    scale: f64,
    re: Vec<f64>,
    im: Vec<f64>,
    /// Samples that reached full scale and were held there.
    pub clipped: u64,
}

impl std::fmt::Debug for Tx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tx").field("plan", &self.plan).field("id", &self.id).finish_non_exhaustive()
    }
}

/// The level a transmitter sends at, RMS against full scale.
///
/// Sixteen decibels down. A multicarrier signal is as near Gaussian as makes
/// no difference, so however low the level some peak will reach full scale;
/// at this one it is a sample in some hundreds of millions, and what the
/// room costs is noise floor nobody on a cable is short of.
pub const LEVEL: f64 = 0.16;

impl Tx {
    pub fn new(plan: Plan) -> Self {
        Self::at_level(plan, LEVEL)
    }

    pub fn at_level(plan: Plan, level: f64) -> Self {
        let table = plan.default_table();
        let carriers = (plan.half() - 1) as f64;
        Self {
            fft: Fft::new(plan.n),
            known: plan.known(),
            table,
            id: 0,
            next: None,
            symbol: 0,
            reader: BitReader::default(),
            wanting: true,
            out: Vec::new(),
            cursor: 0,
            // Each carrier and its mirror image put two units of power into
            // the unnormalised transform.
            scale: level / (2.0 * carriers).sqrt(),
            re: vec![0.0; plan.n],
            im: vec![0.0; plan.n],
            clipped: 0,
            plan,
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    fn build(&mut self) {
        let (n, half) = (self.plan.n, self.plan.half());
        let row = if self.symbol == 0 { usize::from(self.id) } else { TABLES + self.symbol - 1 };
        self.re.fill(0.0);
        self.im.fill(0.0);
        for k in 1..half {
            let bits = self.table.bits[k];
            let x = if self.symbol > 0 && bits > 0 {
                qam_point(self.reader.take(u32::from(bits)), bits)
            } else {
                self.known[row][k]
            };
            // The conjugate goes in, so that the forward transform comes out
            // as the inverse one would have, and its mirror image with it so
            // that what comes out is real.
            self.re[k] = x.re;
            self.im[k] = -x.im;
            self.re[n - k] = x.re;
            self.im[n - k] = x.im;
        }
        self.fft.process(&mut self.re, &mut self.im);
        self.out.clear();
        self.cursor = 0;
        let cp = self.plan.cp;
        for i in (n - cp..n).chain(0..n) {
            let s = self.re[i] * self.scale;
            if s.abs() >= 1.0 {
                self.clipped += 1;
            }
            self.out.push(s.clamp(-1.0, 1.0));
        }
        // The octets for a group are asked for as the last symbol of the
        // one before goes out, and a change of table waits for the end of
        // a superframe, where the sync symbol can announce it.
        let done = self.symbol;
        let group = self.plan.group(&self.table);
        self.symbol += 1;
        if self.symbol > self.plan.symbols {
            self.symbol = 0;
            if let Some((id, table)) = self.next.take() {
                self.id = id;
                self.table = table;
            }
        }
        if done > 0 && done.is_multiple_of(group) {
            self.reader = BitReader::default();
            self.wanting = true;
        }
    }
}

impl Sender for Tx {
    fn wants(&self) -> Option<Layout> {
        if self.wanting { self.plan.layout(&self.table) } else { None }
    }

    fn give(&mut self, line: Vec<u8>) {
        self.reader = BitReader::new(line);
        self.wanting = false;
    }

    fn next_sample(&mut self) -> f64 {
        if self.cursor >= self.out.len() {
            self.build();
        }
        let s = self.out[self.cursor];
        self.cursor += 1;
        s
    }

    fn adopt(&mut self, id: u8, mut table: Table) {
        assert!(usize::from(id) < TABLES);
        table.bits.resize(self.plan.half(), 0);
        for (k, b) in table.bits.iter_mut().enumerate() {
            if k == 0 || Plan::is_pilot(k) {
                *b = 0;
            }
            *b = (*b).min(MAX_BITS);
        }
        self.next = Some((id, table));
    }

    fn id(&self) -> u8 {
        self.next.as_ref().map_or(self.id, |(id, _)| *id)
    }

    fn reset(&mut self) {
        self.next = Some((0, self.plan.default_table()));
    }

    fn rates(&self) -> (f64, f64) {
        (self.plan.gross(&self.table), self.plan.net(&self.table))
    }

    fn clipped(&self) -> u64 {
        self.clipped
    }
}

/// Where a receiver has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Looking for the rhythm of guard and body.
    Hunting,
    /// Symbols found; looking for a sync symbol among them.
    Framing,
    /// In step, and measuring the line.
    Training,
    /// In step, with the line measured.
    Steady,
}

/// The receiver.
pub struct Rx {
    plan: Plan,
    fft: Fft,
    known: Vec<Vec<Complex>>,
    interp: Interp,
    /// Samples on the transmitter's clock, `base` being the number of the
    /// first one still held.
    buf: VecDeque<f64>,
    base: i64,
    count: i64,

    stage: Stage,
    // Hunting: the guard correlated against the end of the body it copies,
    // and the power under it, summed over a guard's length and then folded
    // onto one symbol's span.
    products: Vec<(f64, f64)>,
    sum: (f64, f64),
    folded: Vec<(f64, f64)>,
    hunted: usize,

    /// Where the next symbol's transform starts.
    next_start: i64,
    /// Which symbol of the superframe it is; zero is the sync.
    symbol: usize,
    searched: usize,
    refined: bool,
    missed: usize,

    h: Vec<Complex>,
    /// The channel's delay when it was first measured, which the estimate
    /// is held to afterwards.
    anchor: f64,
    weight: Vec<f64>,
    /// Noise on each carrier after equalising, against unit signal.
    noise: Vec<f64>,
    measured: Vec<f64>,
    trained: usize,

    tables: [Option<Table>; TABLES],
    id: Option<u8>,
    writer: BitWriter,
    arrived: VecDeque<Arrived>,
    proposal: Option<Table>,
    gap_db: f64,
    /// Measure the line again as soon as the starting table is heard.
    remeasure: bool,
    /// The signal to noise ratio over the band in use, in decibels, as of
    /// the last symbol. Kept because it is asked for far more often than it
    /// changes, and is a logarithm a carrier to work out.
    mean_snr_db: f64,

    re: Vec<f64>,
    im: Vec<f64>,
    /// The timing error on the last symbol, in samples.
    pub timing: f64,
    /// Times the receiver has lost the signal and gone back to hunting.
    pub losses: u64,
}

impl std::fmt::Debug for Rx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rx")
            .field("plan", &self.plan)
            .field("stage", &self.stage)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// How far below the Shannon bound a carrier is loaded, in decibels.
///
/// Uncoded QAM wants 9.8 dB for one error in ten million. The Reed-Solomon
/// code is content with one octet in five hundred wrong, which is worth
/// about four of those, and the rest of this is margin for whatever the
/// two seconds of measuring did not see.
pub const GAP_DB: f64 = 8.0;

impl Rx {
    pub fn new(plan: Plan) -> Self {
        let half = plan.half();
        let span = plan.span();
        let mut tables: [Option<Table>; TABLES] = Default::default();
        tables[0] = Some(plan.default_table());
        Self {
            fft: Fft::new(plan.n),
            known: plan.known(),
            interp: Interp::new(),
            buf: VecDeque::with_capacity(4 * span),
            base: 0,
            count: 0,
            stage: Stage::Hunting,
            products: vec![(0.0, 0.0); plan.cp],
            sum: (0.0, 0.0),
            folded: vec![(0.0, 0.0); span],
            hunted: 0,
            next_start: 0,
            symbol: 0,
            searched: 0,
            refined: false,
            missed: 0,
            h: vec![Complex::ZERO; half],
            anchor: 0.0,
            weight: vec![0.0; half],
            noise: vec![1.0; half],
            measured: vec![0.0; half],
            trained: 0,
            tables,
            id: None,
            writer: BitWriter::default(),
            arrived: VecDeque::new(),
            proposal: None,
            gap_db: GAP_DB,
            remeasure: false,
            mean_snr_db: 0.0,
            re: vec![0.0; plan.n],
            im: vec![0.0; plan.n],
            timing: 0.0,
            losses: 0,
            plan,
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// How far the far end's sample clock is from this one, in parts per
    /// million.
    pub fn clock_ppm(&self) -> f64 {
        self.interp.ppm()
    }

    /// The signal to noise ratio on a carrier, in decibels, once measured.
    pub fn snr_db(&self, carrier: usize) -> f64 {
        -10.0 * self.noise[carrier].max(1e-30).log10()
    }

    /// The table a superframe marked `id` is read with.
    pub fn table(&self, id: u8) -> Option<&Table> {
        self.tables[usize::from(id)].as_ref()
    }

    /// The gap a proposal is worked out with, in decibels.
    pub fn set_gap_db(&mut self, db: f64) {
        self.gap_db = db;
    }

    fn at(&self, index: i64) -> f64 {
        self.buf[(index - self.base) as usize]
    }

    fn hunt_again(&mut self) {
        self.stage = Stage::Hunting;
        self.products.fill((0.0, 0.0));
        self.sum = (0.0, 0.0);
        self.folded.fill((0.0, 0.0));
        self.hunted = 0;
        self.id = None;
    }

    fn sample(&mut self, x: f64) {
        self.buf.push_back(x);
        self.count += 1;
        let span = self.plan.span() as i64;
        match self.stage {
            Stage::Hunting => {
                self.hunt();
                while self.buf.len() as i64 > 4 * span {
                    self.buf.pop_front();
                    self.base += 1;
                }
            }
            _ => {
                while self.stage != Stage::Hunting && self.count >= self.next_start + self.plan.n as i64 {
                    self.symbol_at(self.next_start);
                }
                let keep = (self.next_start - span).min(self.count - span);
                while self.base < keep {
                    self.buf.pop_front();
                    self.base += 1;
                }
            }
        }
    }

    fn hunt(&mut self) {
        let (n, cp, span) = (self.plan.n as i64, self.plan.cp, self.plan.span());
        let t = self.count - 1;
        if t - n < self.base {
            return;
        }
        let (a, b) = (self.at(t), self.at(t - n));
        let slot = (t as usize) % cp;
        let fresh = (a * b, 0.5 * (a * a + b * b));
        let old = std::mem::replace(&mut self.products[slot], fresh);
        self.sum.0 += fresh.0 - old.0;
        self.sum.1 += fresh.1 - old.1;
        self.hunted += 1;
        if self.hunted <= cp {
            return;
        }
        let fold = &mut self.folded[(t as usize) % span];
        fold.0 += self.sum.0;
        fold.1 += self.sum.1;
        if self.hunted < cp + 8 * span {
            return;
        }
        let (best, &(p, e)) = self
            .folded
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
            .expect("a span has samples in it");
        if e > 1e-9 && p > 0.5 * e {
            // The correlation peaks on a symbol's last sample, so the next
            // one's guard starts on the sample after.
            let backoff = (self.plan.cp / 3) as i64;
            let phase = (best as i64 + 1 + cp as i64 - backoff).rem_euclid(span as i64);
            let from = (self.count - 2 * span as i64).max(self.base);
            self.next_start = from + (phase - from).rem_euclid(span as i64);
            self.stage = Stage::Framing;
            self.searched = 0;
            self.refined = false;
        } else {
            self.products.fill((0.0, 0.0));
            self.sum = (0.0, 0.0);
            self.folded.fill((0.0, 0.0));
            self.hunted = 0;
        }
    }

    /// How like each sync sequence the symbol in `re`/`im` is: the best
    /// match, and how good it is between nought and one.
    ///
    /// Each carrier is compared with its neighbour rather than with anything
    /// absolute, so the answer does not depend on where in the guard the
    /// transform started or on a channel that has yet to be measured.
    fn sync_match(&self) -> (u8, f64) {
        let half = self.plan.half();
        let mut sums = [Complex::ZERO; TABLES];
        let mut norm = 0.0;
        let mut last = Complex::new(self.re[1], self.im[1]);
        for k in 2..half {
            let y = Complex::new(self.re[k], self.im[k]);
            let step = y * last.conj();
            norm += step.abs();
            for (id, sum) in sums.iter_mut().enumerate() {
                let p = &self.known[id];
                *sum += step * (p[k] * p[k - 1].conj()).conj();
            }
            last = y;
        }
        let (id, best) = sums
            .iter()
            .map(|s| s.abs())
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("there are tables");
        (id as u8, if norm > 0.0 { best / norm } else { 0.0 })
    }

    /// The channel estimate's delay, in samples.
    fn delay_of(&self) -> f64 {
        let mut sum = Complex::ZERO;
        for k in 2..self.plan.half() {
            sum += self.h[k] * self.h[k - 1].conj();
        }
        -sum.arg() * self.plan.n as f64 / TAU
    }

    fn transform(&mut self, start: i64) {
        for i in 0..self.plan.n {
            self.re[i] = self.at(start + i as i64);
            self.im[i] = 0.0;
        }
        self.fft.process(&mut self.re, &mut self.im);
    }

    fn symbol_at(&mut self, start: i64) {
        let span = self.plan.span() as i64;
        self.transform(start);
        if self.stage == Stage::Framing {
            let (id, quality) = self.sync_match();
            if quality < 0.5 {
                self.next_start += span;
                self.searched += 1;
                if self.searched > 3 * (self.plan.symbols + 1) {
                    self.hunt_again();
                }
                return;
            }
            let row = &self.known[usize::from(id)];
            for k in 1..self.plan.half() {
                self.h[k] = Complex::new(self.re[k], self.im[k]) * row[k].conj();
            }
            if !self.refined {
                // The guard correlation is good to a few samples. The channel
                // itself says where the transform ought to start: with the
                // signal a third of a guard into the window, so that what the
                // filters ring out ahead of it and behind it both fall inside.
                self.refined = true;
                let shift = (self.delay_of() - (self.plan.cp / 3) as f64).round() as i64;
                if shift != 0 && start + shift >= self.base {
                    self.next_start = start + shift;
                    return;
                }
            }
            self.anchor = self.delay_of();
            let peak = self.h.iter().map(|h| h.norm_sqr()).fold(0.0, f64::max).max(1e-30);
            // Until the line has been measured the only guide to which
            // carriers to time against is how strongly they arrive, and that
            // is no guide at the very top of the band: the resampler's own
            // response up there moves as the two clocks slide past each
            // other, which reads as timing error and is not.
            for k in 0..self.plan.half() {
                self.weight[k] = if k <= self.plan.top() { self.h[k].norm_sqr() / peak } else { 0.0 };
            }
            self.noise.fill(1.0);
            self.measured.fill(0.0);
            self.trained = 0;
            self.stage = Stage::Training;
            self.id = Some(id);
            self.symbol = 1;
            self.missed = 0;
            self.writer = BitWriter::default();
            self.next_start += span;
            return;
        }

        if self.symbol == 0 {
            let (id, quality) = self.sync_match();
            if quality < 0.4 {
                self.missed += 1;
                if self.missed >= 3 {
                    self.losses += 1;
                    self.hunt_again();
                    return;
                }
            } else {
                self.missed = 0;
                self.id = Some(id);
            }
            self.hold_anchor();
        }
        let id = usize::from(self.id.unwrap_or(0));
        if self.remeasure && id == 0 && self.stage == Stage::Steady {
            self.remeasure = false;
            self.stage = Stage::Training;
            self.trained = 0;
            self.measured.fill(0.0);
        }
        let row = if self.symbol == 0 { id } else { TABLES + self.symbol - 1 };
        self.demodulate(row);

        let table = self.tables[id].as_ref();
        let group = table.map_or(self.plan.symbols, |t| self.plan.group(t));
        if self.symbol > 0 && self.symbol.is_multiple_of(group) {
            match table.and_then(|t| self.plan.layout(t)) {
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
        self.symbol = if self.symbol == self.plan.symbols { 0 } else { self.symbol + 1 };
        self.next_start += span;
    }

    /// Keep the channel estimate's delay where it was first found.
    ///
    /// The estimate and the timing loop can each account for a signal that
    /// arrives a little late, and left to themselves they slowly trade: the
    /// estimate takes on delay, the loop reports none, and the window drifts
    /// off the symbol. Once a superframe whatever delay has crept into the
    /// estimate is taken out of it and given to the resampler instead.
    fn hold_anchor(&mut self) {
        let crept = self.delay_of() - self.anchor;
        if crept.abs() < 1e-4 || crept.abs() > 2.0 {
            return;
        }
        let turn = TAU * crept / self.plan.n as f64;
        for k in 1..self.plan.half() {
            self.h[k] = self.h[k] * Complex::from_polar(1.0, turn * k as f64);
        }
        self.interp.nudge(crept);
    }

    fn demodulate(&mut self, row: usize) {
        let (n, half) = (self.plan.n as f64, self.plan.half());
        let is_sync = self.symbol == 0;
        let id = usize::from(self.id.unwrap_or(0));
        // Borrowed apart from `self` so the carriers can be walked while the
        // estimates are updated.
        let table = self.tables[id].clone();
        let bits_at = |k: usize| -> u8 {
            if is_sync { 0 } else { table.as_ref().map_or(0, |t| t.bits[k]) }
        };
        let unknown = table.is_none() && !is_sync;

        // The timing error, from every carrier whose point is known: a late
        // signal turns carrier k back by k times as much as carrier one.
        let (mut num, mut den) = (0.0, 0.0);
        for k in 1..half {
            if bits_at(k) > 0 || (unknown && !Plan::is_pilot(k)) {
                continue;
            }
            let y = Complex::new(self.re[k], self.im[k]);
            let z = y * (self.h[k] * self.known[row][k]).conj();
            let w = self.weight[k] * k as f64;
            num += w * z.arg();
            den += w * k as f64;
        }
        let late = if den > 0.0 { -num / den * n / TAU } else { 0.0 };
        self.timing = late;

        let rate = match self.stage {
            Stage::Training if self.trained < SETTLE => 0.15,
            Stage::Training => 0.03,
            _ => 0.01,
        };
        let measuring = self.stage == Stage::Training && self.trained >= SETTLE;
        let turn = Complex::from_polar(1.0, TAU * late / n);
        let mut back = Complex::ONE;
        for k in 1..half {
            back = back * turn;
            if self.h[k].norm_sqr() < 1e-30 {
                continue;
            }
            let z = Complex::new(self.re[k], self.im[k]) * back / self.h[k];
            let bits = bits_at(k);
            let reference = if bits > 0 {
                let (value, point) = qam_slice(z, bits);
                self.writer.put(value, u32::from(bits));
                point
            } else if unknown && !Plan::is_pilot(k) {
                continue;
            } else {
                self.known[row][k]
            };
            let error = z - reference;
            let e2 = error.norm_sqr();
            if measuring {
                self.measured[k] += e2;
            } else if self.stage == Stage::Steady {
                self.noise[k] += (e2 - self.noise[k]) / 64.0;
            }
            let lean = error * reference.conj() / reference.norm_sqr().max(0.5);
            self.h[k] = self.h[k] * (Complex::ONE + lean * rate);
        }

        if self.stage == Stage::Steady {
            let top = self.plan.top();
            self.mean_snr_db = (1..=top).map(|k| self.snr_db(k)).sum::<f64>() / top as f64;
        }

        // A second-order loop about a hertz wide: most of each error is
        // taken out at once, and a fiftieth of it changes the rate.
        self.interp.nudge(0.2 * late);
        self.interp.adjust_step(0.02 * late / self.plan.span() as f64);

        if self.stage == Stage::Training && unknown {
            // Sent with a table this end has never seen, so most carriers
            // hold points it cannot name and there is nothing to measure
            // them against. Wait for the far end to go back to the start.
            self.trained = 0;
            self.measured.fill(0.0);
        } else if self.stage == Stage::Training && !is_sync {
            self.trained += 1;
            if self.trained == SETTLE + MEASURE {
                for k in 1..half {
                    self.noise[k] = (self.measured[k] / MEASURE as f64).max(1e-30);
                    self.weight[k] = (1.0 / self.noise[k]).min(1e12);
                }
                let peak = self.weight.iter().copied().fold(0.0, f64::max).max(1e-30);
                for w in &mut self.weight {
                    *w /= peak;
                }
                self.stage = Stage::Steady;
                self.proposal = Some(self.load());
            }
        }
    }

    /// The table this line will bear, by what has been measured.
    pub fn load(&self) -> Table {
        let gap = 10f64.powf(self.gap_db / 10.0);
        let bits = (0..self.plan.half())
            .map(|k| {
                if k == 0 || k > self.plan.top() || Plan::is_pilot(k) {
                    return 0;
                }
                let snr = 1.0 / self.noise[k];
                ((1.0 + snr / gap).log2().floor() as i64).clamp(0, i64::from(MAX_BITS)) as u8
            })
            .collect();
        Table { parity: LOADED_PARITY, bits }
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
        self.tables[usize::from(id)] = Some(table);
    }

    fn id(&self) -> Option<u8> {
        self.id
    }

    fn back_off(&mut self, db: f64) {
        self.gap_db += db;
        if self.stage == Stage::Steady {
            self.proposal = Some(self.load());
        }
    }

    fn remeasure(&mut self, db: f64) {
        self.gap_db += db;
        self.remeasure = true;
    }

    fn health(&self) -> Health {
        let steady = self.stage == Stage::Steady;
        Health {
            locked: matches!(self.stage, Stage::Training | Stage::Steady),
            steady,
            snr_db: if steady { self.mean_snr_db } else { 0.0 },
            clock_ppm: self.clock_ppm(),
        }
    }

    fn rates(&self) -> (f64, f64) {
        match self.id.and_then(|id| self.tables[usize::from(id)].as_ref()) {
            Some(table) => (self.plan.gross(table), self.plan.net(table)),
            None => (0.0, 0.0),
        }
    }

    fn spectrum(&self) -> Vec<Bin> {
        let table = self.id.and_then(|id| self.tables[usize::from(id)].as_ref());
        (1..self.plan.half())
            .map(|k| Bin {
                hz: self.plan.frequency(k),
                snr_db: if self.stage == Stage::Steady { self.snr_db(k) } else { 0.0 },
                bits: table.map_or(0, |t| t.bits[k]),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Packer, Unpacker};

    /// Noise that is the same every run.
    pub(crate) struct Noise(u64);
    impl Noise {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn uniform(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        pub(crate) fn gaussian(&mut self) -> f64 {
            (-2.0 * self.uniform().ln()).sqrt() * (TAU * self.uniform()).cos()
        }
    }

    /// One direction of a line: a transmitter, a channel and a receiver,
    /// with frames going in one end and coming out of the other.
    struct Line {
        tx: Tx,
        rx: Rx,
        packer: Packer,
        unpacker: Unpacker,
        clock: Interp,
        noise: Noise,
        sigma: f64,
        failed: usize,
        words: usize,
        lost: usize,
        got: Vec<(u8, Vec<u8>)>,
    }

    impl Line {
        fn new(fs: f64, snr_db: f64, ppm: f64) -> Self {
            let plan = Plan::for_rate(fs);
            let mut clock = Interp::new();
            clock.adjust_step(ppm * 1e-6);
            Self {
                tx: Tx::new(plan.clone()),
                rx: Rx::new(plan),
                packer: Packer::new(),
                unpacker: Unpacker::new(),
                clock,
                noise: Noise::new(7),
                sigma: LEVEL * 10f64.powf(-snr_db / 20.0),
                failed: 0,
                words: 0,
                lost: 0,
                got: Vec::new(),
            }
        }

        fn run(&mut self, seconds: f64) {
            for _ in 0..(seconds * self.tx.plan.fs) as usize {
                if let Some(layout) = self.tx.wants() {
                    let line = self.packer.fill(&layout);
                    self.tx.give(line);
                }
                let s = self.tx.next_sample();
                let (rx, noise, sigma) = (&mut self.rx, &mut self.noise, self.sigma);
                self.clock.feed(s, |y| rx.feed(y + sigma * noise.gaussian()));
                while let Some(arrived) = self.rx.take() {
                    match arrived {
                        Arrived::Octets { line, layout, .. } => {
                            let report = self.unpacker.take(&line, &layout, None);
                            self.failed += report.failed;
                            self.words += report.words;
                        }
                        Arrived::Lost => {
                            self.lost += 1;
                            self.unpacker.lost();
                        }
                    }
                }
                while let Some(frame) = self.unpacker.pop() {
                    self.got.push(frame);
                }
            }
        }
    }

    #[test]
    fn the_starting_table_carries_frames_over_a_quiet_line() {
        let mut line = Line::new(48_000.0, 60.0, 0.0);
        for i in 0..20u8 {
            line.packer.push(3, &[i; 200]);
        }
        line.run(8.0);
        assert_eq!(line.rx.stage(), Stage::Steady);
        assert_eq!(line.failed, 0);
        // Whatever went by before the receiver found its feet is gone, and
        // the rest arrives in order.
        assert!(line.got.len() >= 12, "{} frames", line.got.len());
        assert!(line.got.windows(2).all(|w| w[0].1[0] + 1 == w[1].1[0]));
        assert_eq!(line.got.last().unwrap().1[0], 19);
    }

    #[test]
    fn a_line_is_measured_for_what_it_is() {
        for &snr in &[30.0, 50.0, 70.0] {
            let mut line = Line::new(48_000.0, snr, 0.0);
            line.run(5.0);
            let table = line.rx.proposal().expect("a proposal after training");
            // White noise at this level against a signal spread evenly over
            // the carriers gives every carrier the same ratio as the whole.
            let mid: Vec<f64> = (40..400).map(|k| line.rx.snr_db(k)).collect();
            let mean = mid.iter().sum::<f64>() / mid.len() as f64;
            assert!((mean - snr).abs() < 1.0, "asked {snr}, measured {mean:.1}");
            let expect = ((1.0 + 10f64.powf((snr - GAP_DB) / 10.0)).log2().floor()) as u8;
            let near = (40..400).filter(|&k| !Plan::is_pilot(k)).filter(|&k| table.bits[k].abs_diff(expect) <= 1).count();
            assert!(near > 340, "{near} carriers near {expect} bits at {snr} dB");
        }
    }

    #[test]
    fn a_loaded_table_carries_frames_between_clocks_that_disagree() {
        for &ppm in &[0.0, 60.0, -120.0] {
            let mut line = Line::new(48_000.0, 55.0, ppm);
            line.run(5.0);
            assert_eq!(line.rx.stage(), Stage::Steady, "{ppm} ppm");
            assert!((line.rx.clock_ppm() - ppm).abs() < 2.0, "asked {ppm}, found {:.2}", line.rx.clock_ppm());
            let table = line.rx.proposal().expect("a proposal after training");
            line.rx.expect(1, table.clone());
            line.tx.adopt(1, table.clone());
            let rate = line.rx.plan().net(&table);
            assert!(rate > 230_000.0, "{rate} bit/s");
            line.got.clear();
            line.run(1.5);
            let sent: Vec<Vec<u8>> = (0..300usize).map(|i| (0..1000).map(|j| (i * 7 + j * 13 + (j >> 4)) as u8).collect()).collect();
            for body in &sent {
                line.packer.push(3, body);
            }
            line.failed = 0;
            line.run(12.0);
            assert_eq!(line.failed, 0, "{ppm} ppm");
            assert_eq!(line.lost, 0, "{ppm} ppm");
            let got: Vec<Vec<u8>> = line.got.iter().filter(|f| f.0 == 3).map(|f| f.1.clone()).collect();
            assert_eq!(got.len(), sent.len(), "{ppm} ppm");
            assert!(got == sent, "{ppm} ppm");
        }
    }
}
