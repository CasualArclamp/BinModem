//! V.ARC/L16: no modulation at all.
//!
//! The other three modes are ways of getting bits through a channel that
//! does something to the signal. A call that carries linear PCM between two
//! of these does nothing to it: every sample arrives as the sixteen bits
//! that left. So the samples are the data, two octets each, and what is left
//! of a modem is the part that deals with samples that do not arrive.
//!
//! A superframe is a **sync word** of 64 samples both ends know, then a
//! **block** of 2000 samples that are 4000 octets of Reed-Solomon
//! codewords, eight times a second.
//!
//! What goes missing on a call is a packet: twenty milliseconds, 320
//! samples, 640 octets in a row. The line fills the hole with silence so
//! that everything after it is still where it should be, and silence is the
//! one thing scrambled data never is, so a run of samples that are exactly
//! nought is a hole, and the octets in it are *erasures*: wrong, and known
//! to be. A code corrects twice as many of those as of errors it has to find
//! for itself. Forty-eight check octets a codeword rebuild one lost packet
//! in every block.
//!
//! A path that is not exact -- one that rescaled the samples, or resampled
//! them, or put them through any coder -- never produces a sync word, so a
//! receiver here simply never finds one, and that is how the two ends learn
//! to use something else.

use std::collections::VecDeque;

use crate::frame::Layout;
use crate::link::{Arrived, Bin, Health, Receiver, Sender, Table};

/// Samples in a sync word.
const SYNC: usize = 64;

/// Samples in a block, and so half the octets.
const BLOCK: usize = 2000;

/// Check octets in each codeword: one lost packet in a block is forty
/// erasures in each.
const PARITY: u8 = 48;

/// Samples in a row that have to be exactly nought before they are taken
/// for a hole rather than for data.
const HOLE: usize = 8;

/// One sample in this many is kept for a symbol diagram, and no more than
/// `SHOWN` of them wait to be taken.
const EVERY: usize = 8;
const SHOWN: usize = 4096;

/// Sync words that may go missing in a row, with silence where they should
/// be, before the receiver stops trusting where it is.
const MISSED: usize = 4;

/// Check octets in each codeword while no packet has gone missing for a
/// while: none. Forty-eight of them are there to rebuild a lost packet, and
/// on a line that is not losing any they are a fifth of the line spent on
/// nothing, since what does arrive arrives exactly. A lost packet then
/// costs its block, which is asked for again, and the next blocks go back
/// to the full count.
const LIGHT: u8 = 0;

/// Blocks in a row with nothing missing before the lighter code is asked
/// for: five seconds.
const CLEAN: usize = 40;

fn table() -> Table {
    table_of(PARITY)
}

fn table_of(parity: u8) -> Table {
    Table { parity, bits: vec![16] }
}

fn layout_of(parity: u8) -> Layout {
    Layout::new(2 * BLOCK, usize::from(parity))
        .or_else(|| Layout::new(2 * BLOCK, usize::from(PARITY)))
        .expect("a block holds codewords")
}

/// The sync word for a mark: sixteen-bit words from a generator, none of
/// them nought.
fn sync(mark: u8) -> Vec<i16> {
    let mut state: u32 = 0xa5c3_3c5a ^ u32::from(mark).wrapping_mul(0x9e37_79b9);
    (0..SYNC)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let word = (state >> 8) as u16 as i16;
            if word == 0 { 0x5a5a } else { word }
        })
        .collect()
}

/// A sample as a point of a symbol diagram.
///
/// There is no carrier and so no symbol in the sense the other modes have
/// one, but a sample is sixteen bits sent at once, which is what a symbol
/// is. Its two octets are drawn as the two axes: the first across and the
/// second up, each as a number from nought to 255 laid over -1 to 1. That is
/// a grid of 65 536 points, every one of them in use, and scrambled data
/// fills it evenly. A hole is a point in the middle of the bottom edge.
fn point(word: i16) -> (f64, f64) {
    let [across, up] = word.to_be_bytes();
    // The first octet is the signed half: nought is the middle.
    let across = f64::from(across as i8) + 0.5;
    let up = f64::from(up) - 127.5;
    (across / 127.5, up / 127.5)
}

/// Bits a second on the line, and of frames.
#[cfg(test)]
fn rates(fs: f64) -> (f64, f64) {
    rates_of(fs, PARITY)
}

fn rates_of(fs: f64, parity: u8) -> (f64, f64) {
    let superframe = (SYNC + BLOCK) as f64 / fs;
    ((16 * BLOCK) as f64 / superframe, (8 * layout_of(parity).payload()) as f64 / superframe)
}

/// The transmitter.
#[derive(Debug)]
pub struct Tx {
    fs: f64,
    sync: Vec<Vec<i16>>,
    id: u8,
    next: Option<u8>,
    /// Which sample of the superframe is sent next.
    at: usize,
    block: Vec<u8>,
    wanting: bool,
    /// Check octets a codeword under each mark, as the far end asked.
    parity: [u8; 4],
}

impl Tx {
    /// A transmitter for a line of exact samples at `fs` a second.
    pub fn new(fs: f64) -> Self {
        Self { fs, sync: (0..4).map(sync).collect(), id: 0, next: None, at: 0, block: Vec::new(), wanting: true, parity: [PARITY; 4] }
    }
}

impl Sender for Tx {
    fn wants(&self) -> Option<Layout> {
        self.wanting.then(|| layout_of(self.parity[usize::from(self.id & 3)]))
    }

    fn give(&mut self, line: Vec<u8>) {
        self.block = line;
        self.wanting = false;
    }

    fn next_sample(&mut self) -> f64 {
        let word = if self.at < SYNC {
            self.sync[usize::from(self.id)][self.at]
        } else {
            let i = 2 * (self.at - SYNC);
            match self.block.get(i..i + 2) {
                Some(pair) => i16::from_be_bytes([pair[0], pair[1]]),
                // Nothing was given: not silence, which would read as a
                // hole, but any word that is not.
                None => 0x0101,
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
        f64::from(word) / 32768.0
    }

    fn adopt(&mut self, id: u8, table: Table) {
        // All a table says here is how many check octets a codeword has.
        // The mark changes with it, which is how the far end knows it was
        // heard; and the first mark is always the full count, since that is
        // what both ends start from and go back to.
        let id = id & 3;
        if id != 0 {
            self.parity[usize::from(id)] = table.parity;
        }
        self.next = Some(id);
    }

    fn id(&self) -> u8 {
        self.next.unwrap_or(self.id)
    }

    fn reset(&mut self) {
        self.next = Some(0);
    }

    fn rates(&self) -> (f64, f64) {
        rates_of(self.fs, self.parity[usize::from(self.id & 3)])
    }

    fn clipped(&self) -> u64 {
        0
    }
}

/// The receiver.
#[derive(Debug)]
pub struct Rx {
    fs: f64,
    sync: Vec<Vec<i16>>,
    /// The last sync word's worth of samples, newest last.
    recent: VecDeque<i16>,
    /// Samples of the block gathered so far, once a sync word has been
    /// found; `None` while hunting.
    block: Option<Vec<i16>>,
    /// Samples still expected of the sync word that follows a block.
    between: usize,
    id: Option<u8>,
    missed: usize,
    arrived: VecDeque<Arrived>,
    /// Samples of blocks, as points for a symbol diagram.
    points: Vec<(f64, f64)>,
    proposed: bool,
    proposal: Option<Table>,
    /// Check octets a codeword under each mark, as this end asked.
    parity: [u8; 4],
    /// How many were last asked for, and blocks in a row since anything
    /// went missing.
    asked: u8,
    clean: usize,
    /// Blocks that arrived, and octets in them that were holes.
    pub blocks: u64,
    pub erased: u64,
    /// Times the receiver has lost its place and gone back to hunting.
    pub losses: u64,
}

impl Rx {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            sync: (0..4).map(sync).collect(),
            recent: VecDeque::from(vec![0; SYNC]),
            block: None,
            between: 0,
            id: None,
            missed: 0,
            arrived: VecDeque::new(),
            points: Vec::new(),
            proposed: false,
            proposal: None,
            parity: [PARITY; 4],
            asked: PARITY,
            clean: 0,
            blocks: 0,
            erased: 0,
            losses: 0,
        }
    }

    /// The mark of the sync word the last samples are, if they are one.
    fn word(&self) -> Option<u8> {
        let newest = *self.recent.back()?;
        (0..4u8).find(|&mark| {
            let word = &self.sync[usize::from(mark)];
            // The last sample first: it rules out nearly every position
            // there is without looking at the other sixty-three.
            word[SYNC - 1] == newest && self.recent.iter().zip(word).all(|(a, b)| a == b)
        })
    }

    /// Ask for the full count of check octets, unless that is what was
    /// asked for last.
    fn strong(&mut self) {
        self.clean = 0;
        if self.asked != PARITY {
            self.asked = PARITY;
            self.proposal = Some(table());
        }
    }

    /// A block has arrived, with a hole in it or without: what that says
    /// about how many check octets the next ones want.
    fn judge(&mut self, hole: bool, light: u8) {
        if hole {
            self.strong();
            return;
        }
        self.clean += 1;
        if self.clean >= CLEAN && self.asked != light {
            self.asked = light;
            self.proposal = Some(table_of(light));
        }
    }

    fn found(&mut self, mark: u8) {
        self.id = Some(mark);
        self.missed = 0;
        self.block = Some(Vec::with_capacity(BLOCK));
        self.between = 0;
        if !self.proposed {
            self.proposed = true;
            self.proposal = Some(table());
        }
    }

    fn finish(&mut self, samples: &[i16]) {
        let mut line = Vec::with_capacity(2 * BLOCK);
        let mut erased = vec![false; 2 * BLOCK];
        let mut run = 0;
        for (i, &word) in samples.iter().enumerate() {
            line.extend_from_slice(&word.to_be_bytes());
            run = if word == 0 { run + 1 } else { 0 };
            if run == HOLE {
                for j in i + 1 - HOLE..=i {
                    erased[2 * j] = true;
                    erased[2 * j + 1] = true;
                }
            } else if run > HOLE {
                erased[2 * i] = true;
                erased[2 * i + 1] = true;
            }
        }
        self.blocks += 1;
        let holes = erased.iter().filter(|e| **e).count();
        self.erased += holes as u64;
        self.judge(holes > 0, LIGHT);
        let layout = layout_of(self.parity[usize::from(self.id.unwrap_or(0) & 3)]);
        self.arrived.push_back(Arrived::Octets { line, layout, erased: Some(erased) });
    }
}

impl Receiver for Rx {
    fn feed(&mut self, sample: f64) {
        let word = (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        self.recent.pop_front();
        self.recent.push_back(word);

        if let Some(block) = self.block.as_mut() {
            if block.len() % EVERY == 0 && self.points.len() < SHOWN {
                self.points.push(point(word));
            }
            block.push(word);
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
            if let Some(mark) = self.word() {
                self.found(mark);
            } else if self.recent.iter().any(|&w| w == 0) && self.missed < MISSED {
                // A hole where the word should be. The line keeps its place
                // through a lost packet, so the block is still where it
                // would have been.
                self.missed += 1;
                self.block = Some(Vec::with_capacity(BLOCK));
            } else {
                self.losses += 1;
                self.id = None;
                self.arrived.push_back(Arrived::Lost);
            }
            return;
        }
        // Hunting: a sync word wherever one turns up.
        if let Some(mark) = self.word() {
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
        if id & 3 != 0 {
            self.parity[usize::from(id & 3)] = table.parity;
        }
    }

    fn id(&self) -> Option<u8> {
        self.id
    }

    fn back_off(&mut self, _db: f64) {
        // Codewords are failing: the full count of check octets, if that
        // is not what is in use. Past that there is nothing lower to ask
        // for; what is failing is the path, and that is for whoever chose
        // this mode to notice.
        self.strong();
    }

    fn remeasure(&mut self, _db: f64) {
        self.asked = LIGHT;
        self.strong();
    }

    fn health(&self) -> Health {
        let locked = self.id.is_some();
        // Sixteen bits against their own rounding: there is nothing else.
        Health { locked, steady: locked, snr_db: if locked { 98.0 } else { 0.0 }, clock_ppm: 0.0 }
    }

    fn rates(&self) -> (f64, f64) {
        match self.id {
            Some(id) => rates_of(self.fs, self.parity[usize::from(id & 3)]),
            None => (0.0, 0.0),
        }
    }

    fn code(&self) -> Option<(usize, usize)> {
        self.id.map(|id| layout_of(self.parity[usize::from(id & 3)]).code())
    }

    fn spectrum(&self) -> Vec<Bin> {
        vec![Bin { hz: 0.0, snr_db: self.health().snr_db, bits: 16 }]
    }

    fn points(&mut self) -> Vec<(f64, f64)> {
        std::mem::take(&mut self.points)
    }

    fn shown_bits(&self) -> u8 {
        16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_is_a_point_inside_the_square_and_the_corners_are_the_corners() {
        assert_eq!(point(i16::MAX), (1.0, 1.0));
        assert_eq!(point(i16::MIN), (-1.0, -1.0));
        let (x, y) = point(0);
        assert!(x.abs() < 0.01 && (y + 1.0).abs() < 1e-9);
    }
    use crate::station::Station;

    fn stream(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + seed * 17 + (i >> 5) * 3 + (i >> 11)) as u8).collect()
    }

    type End = Station<Tx, Rx>;

    fn end() -> End {
        Station::new(16_000.0, vec![(Tx::new(16_000.0), Rx::new(16_000.0))])
    }

    /// Two ends and a line each way that passes samples as sixteen-bit
    /// integers, loses packets when told to, and can be made inexact.
    struct Pair {
        a: End,
        b: End,
        from_a: f64,
        from_b: f64,
        at_a: Vec<u8>,
        at_b: Vec<u8>,
        sample: usize,
        /// Lose the packet of 320 samples that starts at each of these.
        lose: Vec<usize>,
        gain: f64,
    }

    impl Pair {
        fn new() -> Self {
            Self { a: end(), b: end(), from_a: 0.0, from_b: 0.0, at_a: vec![], at_b: vec![], sample: 0, lose: vec![], gain: 1.0 }
        }

        fn run(&mut self, seconds: f64) {
            for _ in 0..(seconds * 16_000.0) as usize {
                let lost = self.lose.iter().any(|&at| (at..at + 320).contains(&self.sample));
                let carry = |x: f64, gain: f64| ((x * 32768.0 * gain).round() / 32768.0).clamp(-1.0, 1.0);
                let to_b = if lost { 0.0 } else { carry(self.from_a, self.gain) };
                let to_a = carry(self.from_b, 1.0);
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
    }

    #[test]
    fn the_samples_are_the_data_at_two_hundred_kilobits() {
        let (gross, net) = rates(16_000.0);
        assert!(gross > 245_000.0 && net > 195_000.0, "{gross} and {net}");
        let mut pair = Pair::new();
        let (to_b, to_a) = (stream(1, 200_000), stream(2, 50_000));
        pair.a.write(&to_b);
        pair.b.write(&to_a);
        pair.run(10.0);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
        assert!(pair.at_a == to_a);
        assert!(pair.a.channels[0].loaded() && pair.b.channels[0].loaded());
        assert_eq!(pair.b.channels[0].tally.failed, 0);
        assert_eq!(pair.a.arq.resent, 0);
    }

    #[test]
    fn a_lost_packet_in_a_block_is_rebuilt_without_being_asked_for_again() {
        let mut pair = Pair::new();
        // One every third of a second, wherever in a block or a sync word
        // that falls.
        pair.lose = (0..24).map(|i| 20_000 + i * 5_333).collect();
        let to_b = stream(3, 150_000);
        pair.a.write(&to_b);
        pair.run(10.0);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
        let b = &pair.b.channels[0];
        assert!(b.rx.erased >= 24 * 600, "{} octets marked as holes", b.rx.erased);
        assert_eq!(b.tally.failed, 0, "{:?}", b.tally);
        assert_eq!(pair.a.arq.resent, 0);
        assert_eq!(b.rx.losses, 0);
    }

    #[test]
    fn two_packets_lost_together_cost_a_block_and_no_octets() {
        let mut pair = Pair::new();
        pair.lose = vec![30_000, 30_320, 30_640, 61_000, 61_320];
        let to_b = stream(4, 150_000);
        pair.a.write(&to_b);
        pair.run(10.0);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
        assert!(pair.b.channels[0].tally.failed > 0);
        assert!(pair.a.arq.resent > 0);
    }

    #[test]
    fn a_path_that_is_not_exact_is_never_mistaken_for_one() {
        let mut pair = Pair::new();
        // Half a decibel of gain: inaudible, and every sample is different.
        pair.gain = 0.944;
        pair.a.write(&stream(5, 10_000));
        pair.run(5.0);
        assert!(pair.at_b.is_empty());
        assert!(!pair.b.channels[0].rx.health().locked);
        assert!(!pair.a.channels[0].loaded());
    }
}
