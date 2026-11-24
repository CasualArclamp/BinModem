//! V.ARC/G.711: no modulation, on a call that carries G.711.
//!
//! A telephone call is 8000 octets a second, each a G.711 codeword. Between
//! two of these on a direct SIP call nothing touches them: the octet that
//! left is the octet that arrives. So the octets are the data, as the
//! samples are in [`crate::exact`], and V.90's whole apparatus for getting
//! 56 000 bit/s one way through a codec is not needed. Both directions are
//! digital, and both carry the same.
//!
//! A superframe is a **sync word** of 48 octets both ends know, then a
//! **block** of 952 octets of Reed-Solomon codewords, eight times a second.
//!
//! Two things are not quite exact.
//!
//! mu-law has two codewords for nought, `0xFF` and `0x7F`, and anything that
//! decodes one and encodes it again, as the path between the modem and the
//! packet does, turns `0x7F` into `0xFF`. So `0xFF` arriving may have been
//! either. It is treated as not having arrived: an *erasure*, wrong and
//! known to be, of which a code corrects twice as many as of errors it has
//! to find. Two octets in 256 are one or the other, which costs two of each
//! codeword's check octets. A-law has no such pair.
//!
//! And a packet that never comes is filled with silence, which in mu-law is
//! that same `0xFF`, and in A-law is `0xD5`: a run of those is a hole, and
//! erased the same way. Forty-eight check octets a codeword rebuild one lost
//! packet in every block.
//!
//! A path that is not exact -- a gateway that decodes to linear and encodes
//! again in the other law, a transcoder -- never produces a sync word, so a
//! receiver here never finds one, and the call goes on as an ordinary
//! modem's.

use std::collections::VecDeque;

use crate::frame::Layout;
use crate::link::{Arrived, Bin, Health, Receiver, Sender, Table};

/// The level each of a law's 256 codewords is, by codeword.
pub type Levels = [f64; 256];

/// Octets in a sync word.
const SYNC: usize = 48;

/// Octets in a block.
const BLOCK: usize = 952;

/// What is sent when there is nothing to send: any octet that is not
/// silence in either law.
const FILL: u8 = 0x55;

/// Check octets in each codeword: one lost packet in a block is forty
/// erasures in each, and mu-law's two noughts are two more.
const PARITY: u8 = 48;

/// Octets in a row that have to be silence before they are taken for a
/// hole rather than for data.
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

/// The same on a line that cannot tell silence from another codeword,
/// where two octets in 256 are erasures whatever the line does: enough
/// over those two that a codeword with a dozen of them is still read.
const LIGHT_BLIND: u8 = 16;

/// Blocks in a row with nothing missing before the lighter code is asked
/// for: five seconds.
const CLEAN: usize = 40;

fn table() -> Table {
    table_of(PARITY)
}

fn table_of(parity: u8) -> Table {
    Table { parity, bits: vec![8] }
}

fn layout_of(parity: u8) -> Layout {
    Layout::new(BLOCK, usize::from(parity))
        .or_else(|| Layout::new(BLOCK, usize::from(PARITY)))
        .expect("a block holds codewords")
}

/// The sync word for a mark: octets from a generator, none of them a
/// nought or silence in either law, since those do not arrive as sent.
fn sync(mark: u8) -> Vec<u8> {
    let mut state: u32 = 0x3c5a_a5c3 ^ u32::from(mark).wrapping_mul(0x9e37_79b9);
    (0..SYNC)
        .map(|_| loop {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let octet = (state >> 8) as u8;
            if !matches!(octet, 0xFF | 0x7F | 0xD5 | 0x55) {
                break octet;
            }
        })
        .collect()
}

/// A pair of octets as a point of a symbol diagram: one across and the
/// next up, each as a number from nought to 255 laid over -1 to 1. A grid
/// of 65 536 points, which scrambled data fills evenly.
fn point(across: u8, up: u8) -> (f64, f64) {
    ((f64::from(across) - 127.5) / 127.5, (f64::from(up) - 127.5) / 127.5)
}

/// Bits a second on the line, and of frames.
#[cfg(test)]
fn rates(fs: f64) -> (f64, f64) {
    rates_of(fs, PARITY)
}

fn rates_of(fs: f64, parity: u8) -> (f64, f64) {
    let superframe = (SYNC + BLOCK) as f64 / fs;
    ((8 * BLOCK) as f64 / superframe, (8 * layout_of(parity).payload()) as f64 / superframe)
}

/// The transmitter.
#[derive(Debug)]
pub struct Tx {
    fs: f64,
    sync: Vec<Vec<u8>>,
    /// The level each codeword is, which is what is handed to the line.
    levels: Levels,
    id: u8,
    next: Option<u8>,
    /// Which octet of the superframe is sent next.
    at: usize,
    block: Vec<u8>,
    wanting: bool,
    /// Check octets a codeword under each mark, as the far end asked.
    parity: [u8; 4],
}

impl Tx {
    /// A transmitter for a line of `fs` codewords a second, in the law
    /// whose levels these are.
    pub fn new(fs: f64, levels: &Levels) -> Self {
        Self { fs, sync: (0..4).map(sync).collect(), levels: *levels, id: 0, next: None, at: 0, block: Vec::new(), wanting: true, parity: [PARITY; 4] }
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

    /// The next codeword, as the level it is.
    fn next_sample(&mut self) -> f64 {
        let octet = if self.at < SYNC {
            self.sync[usize::from(self.id)][self.at]
        } else {
            self.block.get(self.at - SYNC).copied().unwrap_or(FILL)
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
    sync: Vec<Vec<u8>>,
    /// Every level the law has, lowest first, and the codeword it is.
    ladder: Vec<(f64, u8)>,
    /// The codeword silence is, on this line: what a packet that never
    /// came is filled with.
    quiet: u8,
    /// Whether that codeword is also what another one arrives as, so that
    /// any one of it is not to be believed: mu-law.
    blind: bool,
    /// The last sync word's worth of octets, newest last.
    recent: VecDeque<u8>,
    /// Octets of the block gathered so far, once a sync word has been
    /// found; `None` while hunting.
    block: Option<Vec<u8>>,
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
    /// A receiver for a line in the law whose levels these are, and whose
    /// silence is the codeword `quiet`: `0xFF` for mu-law, `0xD5` for
    /// A-law. A law with two codewords for the level silence is, which is
    /// mu-law, cannot tell them apart on arrival, and is treated so.
    pub fn new(fs: f64, levels: &Levels, quiet: u8) -> Self {
        let mut ladder: Vec<(f64, u8)> = (0..=255u8).map(|octet| (levels[usize::from(octet)], octet)).collect();
        // Silence first among equals, so that it is what a shared level
        // reads as.
        ladder.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1 != quiet).cmp(&(b.1 != quiet))));
        let before = ladder.len();
        ladder.dedup_by(|later, kept| later.0 == kept.0);
        let blind = ladder.len() < before && levels.iter().filter(|l| **l == levels[usize::from(quiet)]).count() > 1;
        Self {
            fs,
            sync: (0..4).map(sync).collect(),
            ladder,
            quiet,
            blind,
            recent: VecDeque::from(vec![quiet; SYNC]),
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

    fn finish(&mut self, octets: &[u8]) {
        let mut erased = vec![false; BLOCK];
        let (mut run, mut hole) = (0, false);
        for (i, &octet) in octets.iter().enumerate() {
            run = if octet == self.quiet { run + 1 } else { 0 };
            hole |= run >= HOLE;
            if self.blind && octet == self.quiet {
                erased[i] = true;
            } else if run == HOLE {
                for mark in &mut erased[i + 1 - HOLE..=i] {
                    *mark = true;
                }
            } else if run > HOLE {
                erased[i] = true;
            }
        }
        self.blocks += 1;
        self.erased += erased.iter().filter(|e| **e).count() as u64;
        self.judge(hole, if self.blind { LIGHT_BLIND } else { LIGHT });
        let layout = layout_of(self.parity[usize::from(self.id.unwrap_or(0) & 3)]);
        self.arrived.push_back(Arrived::Octets { line: octets.to_vec(), layout, erased: Some(erased) });
    }
}

impl Receiver for Rx {
    fn feed(&mut self, sample: f64) {
        // The codeword whose level is nearest. On a line that carries
        // codewords it is that level exactly.
        let at = self.ladder.partition_point(|(level, _)| *level < sample);
        let word = match (at.checked_sub(1).and_then(|i| self.ladder.get(i)), self.ladder.get(at)) {
            (Some(below), Some(above)) => if sample - below.0 < above.0 - sample { below.1 } else { above.1 },
            (Some(only), None) | (None, Some(only)) => only.1,
            (None, None) => self.quiet,
        };
        self.recent.pop_front();
        self.recent.push_back(word);

        if let Some(block) = self.block.as_mut() {
            if block.len() % EVERY == 1 && self.points.len() < SHOWN {
                self.points.push(point(block[block.len() - 1], word));
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
            } else if self.recent.iter().any(|&w| w == self.quiet) && self.missed < MISSED {
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
        // Eight bits that arrive as sent or not at all: what G.711 would
        // give a waveform is beside the point, and this is what the word
        // length is worth.
        Health { locked, steady: locked, snr_db: if locked { 50.0 } else { 0.0 }, clock_ppm: 0.0 }
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
        vec![Bin { hz: 0.0, snr_db: self.health().snr_db, bits: 8 }]
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
    use crate::station::Station;

    fn stream(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + seed * 17 + (i >> 5) * 3 + (i >> 11)) as u8).collect()
    }

    type End = Station<Tx, Rx>;

    /// A law's levels, near enough: evenly spaced, and for mu-law with the
    /// two noughts it has.
    fn levels(mu: bool) -> Levels {
        let mut levels = [0.0; 256];
        for (octet, level) in levels.iter_mut().enumerate() {
            *level = (octet as f64 - 127.5) / 128.0;
        }
        if mu {
            levels[0x7F] = 0.0;
            levels[0xFF] = 0.0;
        }
        levels
    }

    /// Two ends and a line each way that carries codewords, in mu-law or in
    /// A-law, loses packets when told to, and can be made to recode.
    struct Pair {
        a: End,
        b: End,
        from_a: f64,
        from_b: f64,
        at_a: Vec<u8>,
        at_b: Vec<u8>,
        sample: usize,
        mu: bool,
        /// Lose the packet of 160 octets that starts at each of these.
        lose: Vec<usize>,
        /// A gateway in the way that turns every codeword into another.
        recoded: bool,
    }

    impl Pair {
        fn new(mu: bool) -> Self {
            let levels = levels(mu);
            let quiet = if mu { 0xFF } else { 0xD5 };
            let end = || Station::new(8000.0, vec![(Tx::new(8000.0, &levels), Rx::new(8000.0, &levels, quiet))]);
            let idle = levels[usize::from(FILL)];
            Self { a: end(), b: end(), from_a: idle, from_b: idle, at_a: vec![], at_b: vec![], sample: 0, mu, lose: vec![], recoded: false }
        }

        fn run(&mut self, seconds: f64) {
            let levels = levels(self.mu);
            let quiet = levels[if self.mu { 0xFF } else { 0xD5 }];
            for _ in 0..(seconds * 8000.0) as usize {
                let lost = self.lose.iter().any(|&at| (at..at + 160).contains(&self.sample));
                // A level is what the line carries, and mu-law's two
                // noughts are one level: nothing has to be done to lose
                // the difference.
                let to_b = if lost {
                    quiet
                } else if self.recoded {
                    0.97 * self.from_a + 0.0004
                } else {
                    self.from_a
                };
                let to_a = self.from_b;
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
    fn the_codewords_are_the_data_in_either_law_and_the_same_both_ways() {
        let (gross, net) = rates(8000.0);
        assert!(gross > 60_000.0 && net > 47_000.0, "{gross} and {net}");
        for mu in [false, true] {
            let mut pair = Pair::new(mu);
            let (to_b, to_a) = (stream(1, 60_000), stream(2, 60_000));
            pair.a.write(&to_b);
            pair.b.write(&to_a);
            pair.run(14.0);
            assert!(pair.at_b == to_b, "mu {mu}: {} of {} octets", pair.at_b.len(), to_b.len());
            assert!(pair.at_a == to_a, "mu {mu}: {} of {} octets", pair.at_a.len(), to_a.len());
            assert!(pair.a.channels[0].loaded() && pair.b.channels[0].loaded());
            assert_eq!(pair.b.channels[0].tally.failed, 0, "mu {mu}");
            assert_eq!(pair.a.arq.resent, 0, "mu {mu}");
        }
    }

    #[test]
    fn mu_law_s_two_noughts_are_erasures_and_cost_nothing_but_check_octets() {
        let mut pair = Pair::new(true);
        pair.a.write(&stream(7, 40_000));
        pair.run(10.0);
        let b = &pair.b.channels[0];
        // Two octets in 256 of what was sent, more or less.
        let share = b.rx.erased as f64 / (b.rx.blocks as f64 * BLOCK as f64);
        assert!((0.003..0.02).contains(&share), "{share}");
        assert_eq!(b.tally.failed, 0, "{:?}", b.tally);
    }

    #[test]
    fn a_lost_packet_in_a_block_is_rebuilt_without_being_asked_for_again() {
        for mu in [false, true] {
            let mut pair = Pair::new(mu);
            // One every third of a second, wherever in a block or a sync
            // word that falls.
            pair.lose = (0..24).map(|i| 10_000 + i * 2_667).collect();
            let to_b = stream(3, 40_000);
            pair.a.write(&to_b);
            pair.run(12.0);
            assert!(pair.at_b == to_b, "mu {mu}: {} of {} octets", pair.at_b.len(), to_b.len());
            let b = &pair.b.channels[0];
            assert!(b.rx.erased >= 24 * 120, "{} octets marked as holes", b.rx.erased);
            assert_eq!(b.tally.failed, 0, "mu {mu}: {:?}", b.tally);
            assert_eq!(pair.a.arq.resent, 0, "mu {mu}");
            assert_eq!(b.rx.losses, 0, "mu {mu}");
        }
    }

    #[test]
    fn a_path_that_recodes_is_never_mistaken_for_one_that_does_not() {
        let mut pair = Pair::new(false);
        pair.recoded = true;
        pair.a.write(&stream(5, 10_000));
        pair.run(5.0);
        assert!(pair.at_b.is_empty());
        assert!(!pair.b.channels[0].rx.health().locked);
        assert!(!pair.a.channels[0].loaded());
    }

    #[test]
    fn no_sync_word_has_an_octet_that_does_not_arrive_as_sent() {
        for mark in 0..4 {
            assert!(sync(mark).iter().all(|o| !matches!(o, 0xFF | 0x7F | 0xD5)));
        }
        assert_ne!(sync(0), sync(1));
    }
}
