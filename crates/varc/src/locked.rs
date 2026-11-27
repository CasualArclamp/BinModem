//! V.ARC/PCM on a call in G.711: a level of the codec in each sample.
//!
//! A call in G.711 is 8000 codewords a second. V.ARC/PCM as a waveform
//! runs at 5333 baud, which is not the rate of the codec. Its levels fall
//! between the codec's levels, and the codec's rounding is noise at 38 dB
//! under the signal. That is the most a waveform gets through G.711.
//!
//! V.90 gets past it one way: the digital modem sends the codec's own
//! levels, one in each sample, and the far end learns what each arrives as.
//! Here both ends are digital, so both directions do it.
//!
//! - **8000 symbols a second.** Each symbol is one codeword of the sender's
//!   law.
//! - **A superframe is 1000 samples**: a sync word of 48, then a block of
//!   952. There are eight a second.
//! - **The sync word is in the signs.** Whatever the path does to a level,
//!   it keeps its sign.
//! - **The starting table is one bit a sample**, in the sign. The size of
//!   each sample is not data. It steps through all 128 sizes in an order
//!   both ends know, so the receiver learns what each of the sender's 256
//!   codewords arrives as.
//! - **The receiver selects the set.** It keeps the codewords that arrive
//!   apart from each other, most apart first, as many as a power of two:
//!   128 at most, which is seven bits a sample. It sends the set to the far
//!   end as its table.
//! - **A line that is worse gets a smaller set.** A path that changes one
//!   law into the other, a digital pad, or a robbed bit makes some
//!   codewords arrive alike. Those do not go in the set.
//!
//! A path that is not a sample for a sample has no set worth the name: a
//! filter, another codec, a wire. The receiver then asks for nothing, and
//! whoever chose this mode goes back to the waveform.

use std::collections::VecDeque;

use crate::frame::Layout;
use crate::link::{Arrived, Bin, Health, Receiver, Sender, Table};
use crate::octets::Levels;

/// Samples in a sync word, and in a block.
const SYNC: usize = 48;
const BLOCK: usize = 952;

/// Octets a block carries for each bit a sample.
const OCTETS_A_BIT: usize = BLOCK / 8;

/// The most bits a sample, and the fewest worth this mode: under three the
/// waveform carries more.
pub const MOST: u8 = 7;
pub const FEWEST: u8 = 3;

/// The low seven bits of each sample of a sync word: a codeword that is
/// large in both laws.
const SYNC_SIZE: u8 = 0x2a;

/// Check octets in each codeword to start with, and while packets go
/// missing: one lost packet in a block is rebuilt.
const PARITY: u8 = 48;

/// Check octets while nothing has gone missing for a while.
const LIGHT: u8 = 0;

/// Blocks in a row with nothing missing before the lighter code is asked
/// for: five seconds.
const CLEAN: usize = 40;

/// Samples in a row at the level of silence before they are taken for a
/// packet that never came.
const HOLE: usize = 8;

/// Sync words that may go missing in a row before the receiver stops
/// trusting where it is.
const MISSED: usize = 4;

/// Signs of a sync word that may be wrong where one is expected.
const SLACK: u32 = 4;

/// Blocks of the starting table heard before a set is selected, and times
/// a codeword has to have been heard to go in it.
const LEARNT: usize = 8;
const SEEN: u32 = 3;

/// How far apart two codewords of a set arrive, against how far either
/// moves about where it arrives.
const CLEAR: f64 = 3.0;

/// One sample in this many is kept for a symbol diagram, and no more than
/// `SHOWN` of them wait to be taken.
const EVERY: usize = 8;
const SHOWN: usize = 4096;

/// The low seven bits of the `place`th sample of a block of the starting
/// table: every size in turn, in an order that is not the order of size.
///
/// Each time round, a size comes one sample sooner. Its places in a block
/// are then 127 samples apart, which is no multiple of two, three or six,
/// so anything a path does to every sixth sample is done to every size.
fn size_at(place: usize) -> u8 {
    (((place + place / 128) * 53) % 128) as u8
}

/// The signs of the sync word for a mark, the first in the top bit of 48.
fn sync(mark: u8) -> u64 {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15 ^ u64::from(mark).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    for _ in 0..4 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
    }
    state >> 16
}

/// The codewords of a set, lowest octet first: a table's 32 octets are a
/// bit for each codeword.
fn set_of(table: &Table) -> Option<Vec<u8>> {
    if table.bits.len() != 32 {
        return None;
    }
    let set: Vec<u8> = (0..=255u8).filter(|&octet| table.bits[usize::from(octet >> 3)] >> (octet & 7) & 1 != 0).collect();
    (set.len().is_power_of_two() && (2..=1 << MOST).contains(&set.len())).then_some(set)
}

fn table_of(parity: u8, set: &[u8]) -> Table {
    let mut bits = vec![0u8; 32];
    for &octet in set {
        bits[usize::from(octet >> 3)] |= 1 << (octet & 7);
    }
    Table { parity, bits }
}

/// Bits a sample with a set of this many codewords.
fn bits_of(set: &[u8]) -> usize {
    set.len().trailing_zeros() as usize
}

fn layout_of(bits: usize, parity: u8) -> Layout {
    Layout::new(OCTETS_A_BIT * bits, usize::from(parity))
        .or_else(|| Layout::new(OCTETS_A_BIT * bits, 16))
        .expect("a block holds codewords")
}

/// Bits a second on the line, and of frames.
fn rates_of(fs: f64, bits: usize, parity: u8) -> (f64, f64) {
    let superframe = (SYNC + BLOCK) as f64 / fs;
    ((8 * OCTETS_A_BIT * bits) as f64 / superframe, (8 * layout_of(bits, parity).payload()) as f64 / superframe)
}

/// The transmitter.
#[derive(Debug)]
pub struct Tx {
    fs: f64,
    /// The level each codeword is, which is what is handed to the line.
    levels: Levels,
    id: u8,
    next: Option<u8>,
    /// Which sample of the superframe is sent next.
    at: usize,
    block: Vec<u8>,
    wanting: bool,
    /// The set and the check octets under each mark, as the far end asked.
    /// The first mark is the starting table and has no set.
    sets: [Vec<u8>; 4],
    parity: [u8; 4],
}

impl Tx {
    /// A transmitter for a line of `fs` codewords a second, in the law
    /// whose levels these are.
    pub fn new(fs: f64, levels: &Levels) -> Self {
        Self { fs, levels: *levels, id: 0, next: None, at: 0, block: Vec::new(), wanting: true, sets: Default::default(), parity: [PARITY; 4] }
    }

    fn bits(&self) -> usize {
        let set = &self.sets[usize::from(self.id & 3)];
        if self.id == 0 || set.is_empty() { 1 } else { bits_of(set) }
    }

    /// Bit `place` of the block, the low bit of each octet first. A block
    /// that was not given is ones and noughts in turn.
    fn bit(&self, place: usize) -> bool {
        self.block.get(place / 8).map_or(place.is_multiple_of(2), |octet| octet >> (place % 8) & 1 != 0)
    }
}

impl Sender for Tx {
    fn wants(&self) -> Option<Layout> {
        self.wanting.then(|| layout_of(self.bits(), self.parity[usize::from(self.id & 3)]))
    }

    fn give(&mut self, line: Vec<u8>) {
        self.block = line;
        self.wanting = false;
    }

    /// The next codeword, as the level it is.
    fn next_sample(&mut self) -> f64 {
        let octet = if self.at < SYNC {
            let sign = sync(self.id) >> (SYNC - 1 - self.at) & 1 != 0;
            u8::from(sign) << 7 | SYNC_SIZE
        } else {
            let place = self.at - SYNC;
            let bits = self.bits();
            if bits == 1 && (self.id == 0 || self.sets[usize::from(self.id & 3)].is_empty()) {
                u8::from(self.bit(place)) << 7 | size_at(place)
            } else {
                let which = (0..bits).fold(0, |sum, i| sum | usize::from(self.bit(place * bits + i)) << i);
                self.sets[usize::from(self.id & 3)][which]
            }
        };
        self.at += 1;
        if self.at == SYNC + BLOCK {
            self.at = 0;
            if let Some(id) = self.next.take() {
                self.id = id;
            }
            self.block.clear();
            self.wanting = true;
        }
        self.levels[usize::from(octet)]
    }

    fn adopt(&mut self, id: u8, table: Table) {
        let id = id & 3;
        if id == 0 {
            self.next = Some(0);
        } else if let Some(set) = set_of(&table) {
            self.sets[usize::from(id)] = set;
            self.parity[usize::from(id)] = table.parity;
            self.next = Some(id);
        }
    }

    fn id(&self) -> u8 {
        self.next.unwrap_or(self.id)
    }

    fn reset(&mut self) {
        self.next = Some(0);
    }

    fn rates(&self) -> (f64, f64) {
        rates_of(self.fs, self.bits(), self.parity[usize::from(self.id & 3)])
    }

    fn clipped(&self) -> u64 {
        0
    }
}

/// The most levels one codeword may arrive as before it is taken to move
/// about and not to be one of a few.
const WAYS: usize = 4;

/// What one of the sender's codewords has arrived as.
///
/// On a path that is all numbers a codeword arrives as one level, or as
/// one of a few: a robbed bit gives it two. Those are kept as they are. A
/// codeword that arrives as more than a few moves about, and is known by
/// its mean and how far it goes from it.
#[derive(Debug, Clone, Default)]
struct Seen {
    times: u32,
    sum: f64,
    least: f64,
    most: f64,
    ways: Vec<f64>,
    moves: bool,
}

impl Seen {
    fn add(&mut self, level: f64) {
        if self.times == 0 {
            (self.least, self.most) = (level, level);
        }
        self.times += 1;
        self.sum += level;
        self.least = self.least.min(level);
        self.most = self.most.max(level);
        if !self.moves && !self.ways.contains(&level) {
            if self.ways.len() == WAYS {
                self.moves = true;
                self.ways.clear();
            } else {
                self.ways.push(level);
            }
        }
    }

    fn mean(&self) -> f64 {
        self.sum / f64::from(self.times.max(1))
    }

    /// How far it moves about where it arrives; nought for one that
    /// arrives as one of a few levels.
    fn spread(&self) -> f64 {
        if self.moves { (self.most - self.least) / 2.0 } else { 0.0 }
    }

    /// The levels a sample of it is looked for at.
    fn places(&self) -> Vec<f64> {
        if self.moves { vec![self.mean()] } else { self.ways.clone() }
    }
}

/// A set as the receiver reads it: where each codeword arrives, lowest
/// first, with its number in the set, and how far from there a sample may
/// be and still be it.
#[derive(Debug, Clone, Default)]
struct Slicer {
    arrivals: Vec<(f64, usize)>,
    near: f64,
    bits: usize,
}

/// The receiver.
#[derive(Debug)]
pub struct Rx {
    fs: f64,
    /// The level of silence on this line: what a packet that never came is
    /// filled with. And the largest level there is, for drawing.
    quiet: f64,
    full: f64,
    /// The signs of the last 48 samples, the newest in the low bit.
    signs: u64,
    /// Samples of the block gathered so far, once a sync word has been
    /// found; `None` while hunting.
    block: Option<Vec<f64>>,
    /// Samples still expected of the sync word that follows a block.
    between: usize,
    id: Option<u8>,
    missed: usize,
    arrived: VecDeque<Arrived>,
    points: Vec<(f64, f64)>,
    /// What each of the sender's codewords arrives as, and blocks of the
    /// starting table that went into that.
    seen: Vec<Seen>,
    learnt: usize,
    /// The most bits a sample to ask for: it comes down when a set fails.
    most: u8,
    /// The set last asked for, and whether there is none to ask for.
    asked_set: Vec<u8>,
    hopeless: bool,
    proposal: Option<Table>,
    /// The set under each mark, as this end asked, and its check octets.
    slicers: [Slicer; 4],
    parity: [u8; 4],
    /// Check octets last asked for, and blocks in a row since anything
    /// went missing.
    asked: u8,
    clean: usize,
    /// Blocks that arrived, and samples in them that were holes or were no
    /// codeword of the set.
    pub blocks: u64,
    pub erased: u64,
    /// Times the receiver has lost its place and gone back to hunting.
    pub losses: u64,
}

impl Rx {
    /// A receiver for a line in the law whose levels these are, and whose
    /// silence is the codeword `quiet`.
    pub fn new(fs: f64, levels: &Levels, quiet: u8) -> Self {
        Self {
            fs,
            quiet: levels[usize::from(quiet)],
            full: levels.iter().fold(0.0f64, |most, level| most.max(level.abs())),
            signs: 0,
            block: None,
            between: 0,
            id: None,
            missed: 0,
            arrived: VecDeque::new(),
            points: Vec::new(),
            seen: vec![Seen::default(); 256],
            learnt: 0,
            most: MOST,
            asked_set: Vec::new(),
            hopeless: false,
            proposal: None,
            slicers: Default::default(),
            parity: [PARITY; 4],
            asked: PARITY,
            clean: 0,
            blocks: 0,
            erased: 0,
            losses: 0,
        }
    }

    /// Bits a sample of the set this end last asked for; nought for none.
    pub fn asked_bits(&self) -> u8 {
        if self.asked_set.is_empty() { 0 } else { bits_of(&self.asked_set) as u8 }
    }

    /// The mark of the sync word the last samples are, if they are one,
    /// with no more than `slack` of its signs wrong.
    fn word(&self, slack: u32) -> Option<u8> {
        let signs = self.signs & ((1 << SYNC) - 1);
        (0..4u8).find(|&mark| (signs ^ sync(mark)).count_ones() <= slack)
    }

    fn found(&mut self, mark: u8) {
        self.id = Some(mark);
        self.missed = 0;
        self.block = Some(Vec::with_capacity(BLOCK));
        self.between = 0;
    }

    /// The largest set of codewords that arrive apart, cut down to a power
    /// of two by taking out the closest: most bits a sample first.
    fn select(&self) -> Vec<u8> {
        // Each codeword that was heard, by where it arrives: the lowest
        // and the highest it arrives as, and how far it moves about. Not
        // one that arrives as silence does: that is what a hole looks like.
        let mut heard: Vec<(f64, f64, f64, u8)> = (0..=255u8)
            .map(|octet| (&self.seen[usize::from(octet)], octet))
            .filter(|(seen, _)| seen.times >= SEEN && !(seen.least..=seen.most).contains(&self.quiet))
            .map(|(seen, octet)| (seen.least, seen.most, seen.spread(), octet))
            .collect();
        heard.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        // From the bottom, each one that is clear of the last kept.
        let mut kept: Vec<(f64, f64, f64, u8)> = Vec::new();
        for one in heard {
            if kept.last().is_none_or(|last| one.0 > last.1 + CLEAR * (one.2 + last.2)) {
                kept.push(one);
            }
        }
        let want = 1usize << self.most.min(MOST);
        let want = if kept.len() >= want { want } else { kept.len().checked_ilog2().map_or(0, |bits| 1 << bits) };
        // Take out one of the closest pair, the one nearer its other
        // neighbour, until the count is right.
        while kept.len() > want {
            let gap = |i: usize| if i + 1 < kept.len() { kept[i + 1].0 - kept[i].1 } else { f64::MAX };
            let closest = (0..kept.len() - 1).min_by(|&a, &b| gap(a).total_cmp(&gap(b))).unwrap_or(0);
            let below = if closest > 0 { gap(closest - 1) } else { f64::MAX };
            let above = gap(closest + 1);
            kept.remove(if below < above { closest } else { closest + 1 });
        }
        let mut set: Vec<u8> = kept.iter().map(|k| k.3).collect();
        set.sort_unstable();
        set
    }

    /// Work out the set to ask for, and ask if it is not what was asked
    /// for last.
    fn choose(&mut self) {
        let set = self.select();
        if set.len() < 1 << FEWEST {
            self.hopeless = true;
            return;
        }
        self.hopeless = false;
        if set != self.asked_set {
            self.asked_set = set;
            self.proposal = Some(table_of(self.asked, &self.asked_set));
        }
    }

    /// Ask for another count of check octets on the set in use.
    fn code(&mut self, parity: u8) {
        if self.asked != parity {
            self.asked = parity;
            if !self.asked_set.is_empty() {
                self.proposal = Some(table_of(parity, &self.asked_set));
            }
        }
    }

    fn finish(&mut self, samples: &[f64]) {
        // Runs of silence are packets that never came.
        let mut hole = vec![false; BLOCK];
        let mut run = 0;
        for (i, &sample) in samples.iter().enumerate() {
            run = if sample == self.quiet { run + 1 } else { 0 };
            if run == HOLE {
                hole[i + 1 - HOLE..=i].fill(true);
            } else if run > HOLE {
                hole[i] = true;
            }
        }
        let mark = self.id.unwrap_or(0) & 3;
        let slicer = &self.slicers[usize::from(mark)];
        let bits = if mark == 0 || slicer.bits == 0 { 1 } else { slicer.bits };
        let mut line = vec![0u8; OCTETS_A_BIT * bits];
        let mut erased = vec![false; line.len()];
        let mut bad = 0u64;
        for (place, &sample) in samples.iter().enumerate() {
            let (which, wrong) = if bits == 1 && (mark == 0 || slicer.bits == 0) {
                // The sign is the bit, and the size is one more look at
                // what that codeword arrives as.
                let sign = sample >= 0.0;
                if !hole[place] {
                    self.seen[usize::from(u8::from(sign) << 7 | size_at(place))].add(sample);
                }
                (usize::from(sign), hole[place])
            } else {
                let at = slicer.arrivals.partition_point(|(level, _)| *level < sample);
                let below = at.checked_sub(1).and_then(|i| slicer.arrivals.get(i));
                let nearest = match (below, slicer.arrivals.get(at)) {
                    (Some(below), Some(above)) => if sample - below.0 < above.0 - sample { below } else { above },
                    (Some(only), None) | (None, Some(only)) => only,
                    (None, None) => &(0.0, 0),
                };
                (nearest.1, hole[place] || (sample - nearest.0).abs() > slicer.near)
            };
            for i in 0..bits {
                let bit = place * bits + i;
                line[bit / 8] |= ((which >> i & 1) as u8) << (bit % 8);
                erased[bit / 8] |= wrong;
            }
            bad += u64::from(wrong);
        }
        self.blocks += 1;
        self.erased += bad;
        // What a block with a sample known to be wrong in it, or without,
        // says about the check octets the next ones want.
        if bad > 0 {
            self.clean = 0;
            self.code(PARITY);
        } else {
            self.clean += 1;
            if self.clean >= CLEAN {
                self.code(LIGHT);
            }
        }
        if mark == 0 {
            self.learnt += 1;
            if self.learnt >= LEARNT && self.learnt.is_multiple_of(LEARNT) {
                self.choose();
            }
        }
        let layout = layout_of(bits, self.parity[usize::from(mark)]);
        self.arrived.push_back(Arrived::Octets { line, layout, erased: Some(erased) });
    }
}

impl Receiver for Rx {
    fn feed(&mut self, sample: f64) {
        self.signs = self.signs << 1 | u64::from(sample >= 0.0);

        if let Some(block) = self.block.as_mut() {
            if block.len() % EVERY == 1 && self.points.len() < SHOWN {
                self.points.push((block[block.len() - 1] / self.full, sample / self.full));
            }
            block.push(sample);
            if block.len() == BLOCK {
                let samples = self.block.take().unwrap_or_default();
                self.finish(&samples);
                self.between = SYNC;
            }
            return;
        }
        if self.between > 0 {
            // The sync word that should follow the block just ended.
            self.between -= 1;
            if self.between > 0 {
                return;
            }
            if let Some(mark) = self.word(SLACK) {
                self.found(mark);
            } else if self.missed < MISSED {
                // A hole where the word should be, or a few samples of
                // one. The line keeps its place through a lost packet, so
                // the block is still where it would have been.
                self.missed += 1;
                self.block = Some(Vec::with_capacity(BLOCK));
            } else {
                self.losses += 1;
                self.id = None;
                self.arrived.push_back(Arrived::Lost);
            }
            return;
        }
        // Hunting: a sync word wherever one turns up, every sign right.
        if let Some(mark) = self.word(0) {
            self.found(mark);
        }
    }

    fn take(&mut self) -> Option<Arrived> {
        self.arrived.pop_front()
    }

    fn proposal(&mut self) -> Option<Table> {
        self.proposal.take()
    }

    fn expect(&mut self, id: u8, table: Table) {
        let id = usize::from(id & 3);
        let Some(set) = set_of(&table).filter(|_| id != 0) else { return };
        let mut arrivals: Vec<(f64, usize)> = set
            .iter()
            .enumerate()
            .flat_map(|(which, &octet)| self.seen[usize::from(octet)].places().into_iter().map(move |level| (level, which)))
            .collect();
        arrivals.sort_by(|a, b| a.0.total_cmp(&b.0));
        // A sample is a codeword of the set if it is within what that
        // codeword moves about, and a little. Anything else is known to be
        // wrong, which a code mends twice as many of.
        let spread = set.iter().map(|&octet| self.seen[usize::from(octet)].spread()).fold(0.0f64, f64::max);
        let gap = arrivals.windows(2).filter(|pair| pair[0].1 != pair[1].1).map(|pair| pair[1].0 - pair[0].0).fold(f64::MAX, f64::min);
        self.slicers[id] = Slicer { arrivals, near: (1.5 * spread + 1e-9).min(gap / 2.0), bits: bits_of(&set) };
        self.parity[id] = table.parity;
    }

    fn id(&self) -> Option<u8> {
        self.id
    }

    fn back_off(&mut self, _db: f64) {
        // Codewords are failing on a set of this end's choosing: the full
        // count of check octets first, and then a bit a sample less.
        if self.asked != PARITY {
            self.clean = 0;
            self.code(PARITY);
        } else if self.most > 1 && !self.asked_set.is_empty() {
            self.most = (bits_of(&self.asked_set) as u8).saturating_sub(1).max(1);
            self.choose();
        }
    }

    fn remeasure(&mut self, db: f64) {
        // The line is not what it was measured as: learn it again. And if
        // nothing of the last set could be read, ask for less next time.
        if db > 0.0 && !self.asked_set.is_empty() {
            self.most = (bits_of(&self.asked_set) as u8).saturating_sub(1).max(1);
        }
        self.seen = vec![Seen::default(); 256];
        self.learnt = 0;
        self.asked_set.clear();
        self.asked = PARITY;
        self.clean = 0;
    }

    fn health(&self) -> Health {
        let locked = self.id.is_some();
        // A level that arrives as it is expected or not at all: what the
        // number of levels is worth as a signal to noise ratio.
        let bits = f64::from(self.asked_bits());
        Health { locked, steady: locked && !self.asked_set.is_empty(), snr_db: if locked { 8.0 + 6.02 * bits } else { 0.0 }, clock_ppm: 0.0 }
    }

    fn rates(&self) -> (f64, f64) {
        match self.id {
            Some(id) => {
                let slicer = &self.slicers[usize::from(id & 3)];
                rates_of(self.fs, if id & 3 == 0 { 1 } else { slicer.bits.max(1) }, self.parity[usize::from(id & 3)])
            }
            None => (0.0, 0.0),
        }
    }

    fn code(&self) -> Option<(usize, usize)> {
        self.id.map(|id| {
            let slicer = &self.slicers[usize::from(id & 3)];
            layout_of(if id & 3 == 0 { 1 } else { slicer.bits.max(1) }, self.parity[usize::from(id & 3)]).code()
        })
    }

    fn spectrum(&self) -> Vec<Bin> {
        vec![Bin { hz: 0.0, snr_db: self.health().snr_db, bits: self.shown_bits() }]
    }

    fn points(&mut self) -> Vec<(f64, f64)> {
        std::mem::take(&mut self.points)
    }

    fn shown_bits(&self) -> u8 {
        match self.id {
            Some(id) if id & 3 != 0 => self.slicers[usize::from(id & 3)].bits as u8,
            Some(_) => 1,
            None => 0,
        }
    }
}

impl Rx {
    /// Whether the line has been learnt and has no set worth asking for.
    pub fn hopeless(&self) -> bool {
        self.hopeless
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::Station;

    fn stream(seed: usize, len: usize) -> Vec<u8> {
        let mut state = 0x1234_5678_9abc_def1u64 ^ (seed as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    /// The levels of G.711, as sixteen-bit numbers over 32 768.
    fn levels(mu: bool) -> Levels {
        let mut levels = [0.0; 256];
        for (octet, level) in levels.iter_mut().enumerate() {
            let octet = octet as i32;
            let value = if mu {
                let u = !octet & 0xff;
                let size = ((((u & 15) << 3) + 0x84) << ((u >> 4) & 7)) - 0x84;
                if u & 0x80 != 0 { -size } else { size }
            } else {
                let a = octet ^ 0x55;
                let (segment, step) = ((a >> 4) & 7, a & 15);
                let size = if segment == 0 { (step << 4) + 8 } else { ((step << 4) + 0x108) << (segment - 1) };
                if a & 0x80 != 0 { size } else { -size }
            };
            *level = f64::from(value) / 32768.0;
        }
        levels
    }

    /// The nearest level of a law to `x`, and its codeword.
    fn coded(levels: &Levels, x: f64) -> (f64, u8) {
        let mut best = (levels[0], 0u8);
        for (octet, &level) in levels.iter().enumerate() {
            if (level - x).abs() < (best.0 - x).abs() {
                best = (level, octet as u8);
            }
        }
        best
    }

    /// What a path does to each sample.
    #[derive(Clone, Copy, PartialEq)]
    #[repr(u8)]
    enum Path {
        Exact,
        /// A gateway that changes the law to the other and back.
        Recoded,
        /// A digital pad of 6 dB, and the result coded again.
        Padded,
        /// The low bit of every sixth codeword is forced to one.
        Robbed,
        /// A filter, and the result coded again: not a sample for a sample.
        Filtered,
    }

    type End = Station<Tx, Rx>;

    struct Pair {
        a: End,
        b: End,
        from_a: f64,
        from_b: f64,
        at_a: Vec<u8>,
        at_b: Vec<u8>,
        sample: usize,
        /// The law at each end, and what is between them from a to b.
        mu: (bool, bool),
        path: Path,
        lose: Vec<usize>,
        last: f64,
    }

    impl Pair {
        fn new(mu: (bool, bool), path: Path) -> Self {
            let end = |mu: bool| {
                let levels = levels(mu);
                let quiet = if mu { 0xFF } else { 0xD5 };
                let mut station = Station::new(8000.0, vec![(Tx::new(8000.0, &levels), Rx::new(8000.0, &levels, quiet))]);
                // The samples are the data: there is no level to trim.
                station.level_is_fixed(true);
                station
            };
            Self { a: end(mu.0), b: end(mu.1), from_a: 0.0, from_b: 0.0, at_a: vec![], at_b: vec![], sample: 0, mu, path, lose: vec![], last: 0.0 }
        }

        fn run(&mut self, seconds: f64) {
            let (law_a, law_b) = (levels(self.mu.0), levels(self.mu.1));
            let other = levels(!self.mu.0);
            for _ in 0..(seconds * 8000.0) as usize {
                let lost = self.lose.iter().any(|&at| (at..at + 160).contains(&self.sample));
                let sent = match self.path {
                    Path::Exact => self.from_a,
                    Path::Recoded => coded(&law_a, coded(&other, self.from_a).0).0,
                    Path::Padded => coded(&law_a, 0.5 * self.from_a).0,
                    Path::Robbed if self.sample.is_multiple_of(6) => law_a[usize::from(coded(&law_a, self.from_a).1 | 1)],
                    Path::Robbed => self.from_a,
                    Path::Filtered => {
                        self.last = 0.6 * self.last + 0.4 * self.from_a;
                        coded(&law_a, self.last).0
                    }
                };
                // The end at b hears in its own law, whatever a sent in.
                let to_b = if lost { law_b[if self.mu.1 { 0xFF } else { 0xD5 }] } else { coded(&law_b, sent).0 };
                let to_a = coded(&law_a, self.from_b).0;
                let (mut out_a, mut out_b) = ([0.0], [0.0]);
                self.a.step(&[to_a], &mut out_a);
                self.b.step(&[to_b], &mut out_b);
                self.from_a = out_a[0];
                self.from_b = out_b[0];
                self.at_a.extend(self.a.read());
                self.at_b.extend(self.b.read());
                self.sample += 1;
            }
        }

        /// Send 30 000 octets each way and see them arrive: the bits a
        /// sample b asked for, and the frame rate it hears at.
        fn carry(&mut self, seconds: f64) -> (u8, f64) {
            let (to_b, to_a) = (stream(1, 30_000), stream(2, 30_000));
            self.a.write(&to_b);
            self.b.write(&to_a);
            self.run(seconds);
            assert!(self.at_b == to_b, "{} of {} octets at b", self.at_b.len(), to_b.len());
            assert!(self.at_a == to_a, "{} of {} octets at a", self.at_a.len(), to_a.len());
            let rx = &self.b.channels[0].rx;
            (rx.shown_bits(), Receiver::rates(rx).1)
        }
    }

    #[test]
    fn a_call_that_carries_codewords_carries_seven_bits_a_sample_each_way() {
        for mu in [false, true] {
            let mut pair = Pair::new((mu, mu), Path::Exact);
            let (bits, rate) = pair.carry(16.0);
            assert_eq!(bits, 7, "mu {mu}");
            // Five seconds with nothing lost, and the check octets go.
            assert!(rate > 52_000.0, "mu {mu}: {rate} bit/s");
            assert!(pair.a.channels[0].loaded() && pair.b.channels[0].loaded());
            assert_eq!(pair.b.channels[0].tally.failed, 0, "mu {mu}");
            assert_eq!(pair.b.channels[0].rx.erased, 0, "mu {mu}");
            assert_eq!(pair.a.arq.resent, 0, "mu {mu}");
        }
    }

    #[test]
    fn the_two_ends_need_not_be_in_the_same_law() {
        // A gateway between them that changes one law into the other.
        for mu in [(true, false), (false, true)] {
            let mut pair = Pair::new(mu, Path::Exact);
            let (bits, rate) = pair.carry(16.0);
            assert!(bits >= 6, "{mu:?}: {bits} bits");
            assert!(rate > 38_000.0, "{mu:?}: {rate} bit/s");
            assert_eq!(pair.b.channels[0].tally.failed, 0, "{mu:?}");
        }
    }

    #[test]
    fn a_worse_path_gets_a_smaller_set_and_still_no_errors() {
        // What each path leaves of the 255 levels of mu-law, in bits.
        for (path, least, most) in [(Path::Recoded, 6, 7), (Path::Padded, 6, 7), (Path::Robbed, 6, 7)] {
            let mut pair = Pair::new((true, true), path);
            let (bits, rate) = pair.carry(18.0);
            assert!((least..=most).contains(&bits), "{bits} bits");
            assert!(rate > 7_000.0 * f64::from(bits), "{rate} bit/s at {bits} bits");
            let b = &pair.b.channels[0];
            assert_eq!(b.tally.failed, 0, "{:?}", b.tally);
            assert_eq!(pair.a.arq.resent, 0);
        }
    }

    #[test]
    fn a_lost_packet_in_a_block_is_rebuilt_without_being_asked_for_again() {
        let mut pair = Pair::new((true, true), Path::Exact);
        // One every third of a second, wherever in a block or a sync word
        // that falls, from when the set is in use.
        pair.lose = (0..30).map(|i| 30_000 + i * 2_667).collect();
        let (bits, _) = pair.carry(20.0);
        assert_eq!(bits, 7);
        let b = &pair.b.channels[0];
        assert!(b.rx.erased >= 30 * 100, "{} samples marked as holes", b.rx.erased);
        assert_eq!(b.tally.failed, 0, "{:?}", b.tally);
        assert_eq!(pair.a.arq.resent, 0);
        assert_eq!(b.rx.losses, 0);
    }

    #[test]
    fn a_path_that_is_not_a_sample_for_a_sample_gets_no_set() {
        let mut pair = Pair::new((true, true), Path::Filtered);
        pair.a.write(&stream(5, 10_000));
        pair.run(8.0);
        let b = &pair.b.channels[0];
        // The signs do not survive it, so there is not even a sync word.
        assert_eq!(b.rx.asked_bits(), 0);
        assert!(pair.at_b.is_empty());
        assert!(!b.loaded() && !pair.a.channels[0].loaded());
    }

    #[test]
    fn a_set_is_a_table_and_a_table_is_a_set() {
        let set: Vec<u8> = (0..=255u8).filter(|octet| octet % 2 == 1).collect();
        let table = table_of(16, &set);
        assert_eq!(Table::decode(&table.encode()), Some(table.clone()));
        assert_eq!(set_of(&table), Some(set));
        // A count that is not a power of two is no set.
        assert_eq!(set_of(&table_of(16, &[1, 2, 3])), None);
        // And no two of the four sync words are near each other.
        for a in 0..4u8 {
            assert!(sync(a) < 1 << SYNC);
            for b in a + 1..4 {
                assert!((sync(a) ^ sync(b)).count_ones() > 2 * SLACK + 4, "{a} {b}");
            }
        }
    }
}
