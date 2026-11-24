//! The x2 server's downstream from the client's J to data mode, and its
//! answer to the client's rate change: G.711 mu-law octets, one an 8 kHz
//! sample.
//!
//! libv90modem's `docs/history/x2-facts.md`, "Phase 3 script" (the I-modem's
//! script table at DSP `c915`-`c935`, matching D-Modem's notes and
//! courier-emu's), "Records", "Mapping" and "Rate change":
//!
//! ```text
//! silence    7f until the client's J, then without a gap:
//! lead       20 octets of 7f
//! preamble   363: 22 octets 16 times, then their first 11 with the sign turned
//! TRN        20004 scrambled ones (GPC from state 0), b1 for a 0, 31 for a 1
//! J words    1200: 09B0 99 times then 099F, 12 bits low-order first, scrambled, b1 / 31
//! ones       6000 scrambled ones, c1 / 41
//! alignment  1152: 192 frames of 9e 28 a1 21 a8 1e, frame f turned f places left
//! sync       mapped frames of ones in levels A, 1667 at least, until the client's MP
//! record     our record in levels A, whole records, until the client's E
//! start      681 frames of ones: one in levels A, then 680 in the data levels
//! data       mapped frames of the data
//! ```
//!
//! GPC runs on from TRN's first octet through every mapped frame; the
//! alignment does not use it. A rate change of the client's, answered at the
//! next frame boundary:
//!
//! ```text
//! marker     8 frames + + + - - - and one - - - + + +, each position at its
//!            largest level in the rate in use, no differential signs
//! records    GPC from zero, the sign positive: our record in index 1's data
//!            levels, 10 times without the acknowledge, 33 times with it
//! start      682 frames of ones: one in index 1's, then 681 in the new levels
//!            (the start-up's has 681: one in levels A, then 680)
//! data
//! ```
//!
//! From the client's MP on everything is sent for the pad it reports.

use super::bank::{Bank, FRAME};
use super::map::Mapper;
use super::record;
use crate::v32::{Mode, Scrambler};

/// The preamble's 22 octets: signs from Barker-11 (`+++---+--+-`), the
/// magnitude `ab` or `bd` by `0712` taken low-order first, complemented every
/// other 11 (the I-modem's DSP `c9d9`-`ca21`; D-Modem's table).
const PREAMBLE: [u8; 22] = [
    0xab, 0xbd, 0xab, 0x2b, 0x3d, 0x2b, 0xab, 0x2b, 0x3d, 0xbd, 0x3d, //
    0xbd, 0xab, 0xbd, 0x3d, 0x2b, 0x3d, 0xbd, 0x3d, 0x2b, 0xab, 0x2b,
];
const ALIGNMENT: [u8; FRAME] = [0x9e, 0x28, 0xa1, 0x21, 0xa8, 0x1e];

const LEAD: usize = 20;
const PRE: usize = 363;
const TRN: usize = 20004;
const J_WORDS: usize = 100;
const ONES: usize = 6000;
const ALIGN_FRAMES: usize = 192;
/// Sync frames at least: 10 002 octets.
const SYNC_FRAMES: usize = 1667;
const START_FRAMES: usize = 681;
const MARKER_FRAMES: usize = 9;
const RECORDS_NAK: usize = 10;
const RECORDS: usize = 43;

/// TRN and J words: magnitude `b1` (3772 linear); ones `c1`, 6.3 dB lower
/// (x2-facts, "Disagreements settled" 1).
const TRN_LEVEL: u8 = 0xb1;
const ONES_LEVEL: u8 = 0xc1;
const SILENCE: u8 = 0x7f;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Silence,
    Lead,
    Preamble,
    Trn,
    JWords,
    Ones,
    Alignment,
    Sync,
    Record,
    Start,
    Data,
    Marker,
    RateRecords,
}

/// The server's transmitter.
#[derive(Debug, Clone)]
pub struct Script {
    segment: Segment,
    /// Octets sent of a fixed segment, frames of a framed one.
    n: usize,
    scrambler: Scrambler,
    mapper: Mapper,
    frame: [u8; FRAME],
    at: usize,
    /// Whether the client's MP (in sync) or E (in records) has come.
    go: bool,
    /// The bank the data goes in, once an MP has said.
    data: Option<&'static Bank>,
    /// The bank the rate change's records and first start frame go in.
    pad_b: Option<&'static Bank>,
    /// A rate change's marker levels: the rate in use, sent for the new pad.
    marker: [u8; FRAME],
    /// A rate change waiting for the frame being sent to end.
    change: bool,
    record: Vec<bool>,
    record_at: usize,
    /// The start segment is a rate change's: one frame longer.
    again: bool,
}

impl Default for Script {
    fn default() -> Self {
        Self::new()
    }
}

impl Script {
    /// Silent until [`start`](Self::start).
    pub fn new() -> Self {
        Self {
            segment: Segment::Silence,
            n: 0,
            scrambler: Scrambler::new(Mode::Call),
            mapper: Mapper::levels_a(),
            frame: [SILENCE; FRAME],
            at: FRAME,
            go: false,
            data: None,
            pad_b: None,
            marker: [0; FRAME],
            change: false,
            record: Vec::new(),
            record_at: 0,
            again: false,
        }
    }

    pub fn segment(&self) -> Segment {
        self.segment
    }

    /// The client's J has come: the script begins with the next octet.
    pub fn start(&mut self) {
        if self.segment == Segment::Silence {
            self.enter(Segment::Lead);
        }
    }

    /// The client's MP at the start-up: its bank is the data levels, and the
    /// sync frames end once enough have gone.
    pub fn mp(&mut self, bank: &'static Bank) {
        self.data = Some(bank);
    }

    /// The client's E: the records end at the next whole record.
    pub fn e(&mut self) {
        if self.segment == Segment::Record {
            self.go = true;
        }
    }

    /// A rate change, in data mode: to `bank`, at `pad`, from the next frame.
    pub fn rate_change(&mut self, bank: &'static Bank, pad: u8) {
        let current = self.mapper.bank();
        // Each position's largest level in the rate in use, sent so as to
        // arrive as that level behind the new pad.
        let behind = (1..=15)
            .filter_map(|n| Bank::get(n, 0).map(|b| (n, b)))
            .find(|(_, b)| b.b == current.b)
            .and_then(|(n, _)| Bank::get(n, pad))
            .unwrap_or(current);
        for (i, level) in self.marker.iter_mut().enumerate() {
            *level = behind.list(i).sent[0];
        }
        self.pad_b = Bank::get(1, pad);
        self.data = Some(bank);
        self.change = true;
    }

    /// Whether data octets are going out.
    pub fn in_data(&self) -> bool {
        self.segment == Segment::Data
    }

    /// The bank the data goes in now.
    pub fn bank(&self) -> &'static Bank {
        self.mapper.bank()
    }

    fn enter(&mut self, s: Segment) {
        self.segment = s;
        self.n = 0;
        self.go = false;
    }

    /// The next octet; `data` gives the next data bit, called only in data
    /// mode.
    pub fn next(&mut self, data: &mut dyn FnMut() -> bool) -> u8 {
        match self.segment {
            Segment::Silence => SILENCE,
            Segment::Lead => self.fixed(LEAD, Segment::Preamble, |_, _| SILENCE),
            Segment::Preamble => self.fixed(PRE, Segment::Trn, |_, n| {
                let o = PREAMBLE[n % 22];
                if n >= 352 { o ^ 0x80 } else { o }
            }),
            Segment::Trn => self.fixed(TRN, Segment::JWords, |s, _| s.scrambled(true, TRN_LEVEL)),
            Segment::JWords => self.fixed(12 * J_WORDS, Segment::Ones, |s, n| {
                let word: u16 = if n / 12 == J_WORDS - 1 { 0x099f } else { 0x09b0 };
                s.scrambled((word >> (n % 12)) & 1 != 0, TRN_LEVEL)
            }),
            Segment::Ones => self.fixed(ONES, Segment::Alignment, |s, _| s.scrambled(true, ONES_LEVEL)),
            Segment::Alignment => self.fixed(FRAME * ALIGN_FRAMES, Segment::Sync, |_, n| {
                let (f, k) = (n / FRAME, n % FRAME);
                ALIGNMENT[(k + f) % FRAME]
            }),
            _ => self.framed(data),
        }
    }

    fn fixed(&mut self, len: usize, then: Segment, f: impl Fn(&mut Self, usize) -> u8) -> u8 {
        let o = f(self, self.n);
        self.n += 1;
        if self.n == len {
            self.enter(then);
            if then == Segment::Sync {
                self.mapper = Mapper::levels_a();
            }
        }
        o
    }

    /// One scrambled bit at a level: positive for a 0.
    fn scrambled(&mut self, bit: bool, level: u8) -> u8 {
        if self.scrambler.scramble(bit) { level & 0x7f } else { level }
    }

    fn framed(&mut self, data: &mut dyn FnMut() -> bool) -> u8 {
        if self.at == FRAME {
            self.frame = self.next_frame(data);
            self.at = 0;
        }
        let o = self.frame[self.at];
        self.at += 1;
        o
    }

    fn ones(&mut self) -> [u8; FRAME] {
        let bits: Vec<bool> = (0..self.mapper.bits()).map(|_| self.scrambler.scramble(true)).collect();
        self.mapper.frame(&bits)
    }

    fn record_frame(&mut self) -> [u8; FRAME] {
        let n = self.mapper.bits();
        let bits: Vec<bool> = (0..n)
            .map(|_| {
                let b = self.record[self.record_at];
                self.record_at = (self.record_at + 1) % self.record.len();
                self.scrambler.scramble(b)
            })
            .collect();
        self.mapper.frame(&bits)
    }

    fn next_frame(&mut self, data: &mut dyn FnMut() -> bool) -> [u8; FRAME] {
        if self.change && self.segment == Segment::Data {
            self.change = false;
            self.enter(Segment::Marker);
        }
        match self.segment {
            Segment::Sync => {
                let f = self.ones();
                self.n += 1;
                // The MP may have come before the sync began.
                if self.data.is_some() && self.n >= SYNC_FRAMES {
                    self.enter(Segment::Record);
                    self.record = record::server(true);
                    self.record_at = 0;
                }
                f
            }
            Segment::Record => {
                let f = self.record_frame();
                self.n += 1;
                // Whole records: 96 bits are four frames of 24.
                if self.go && self.record_at == 0 {
                    self.enter(Segment::Start);
                }
                f
            }
            Segment::Start => {
                if self.n == 1 {
                    self.mapper.switch_to(self.data.unwrap_or(Bank::get(1, 0).unwrap()));
                }
                let f = self.ones();
                self.n += 1;
                if self.n == START_FRAMES + usize::from(self.again) {
                    self.again = false;
                    self.enter(Segment::Data);
                }
                f
            }
            Segment::Data => {
                let bits: Vec<bool> = (0..self.mapper.bits()).map(|_| self.scrambler.scramble(data())).collect();
                self.mapper.frame(&bits)
            }
            Segment::Marker => {
                let last = self.n == MARKER_FRAMES - 1;
                let f = std::array::from_fn(|i| {
                    let positive = (i < 3) != last;
                    if positive { self.marker[i] } else { self.marker[i] & 0x7f }
                });
                self.n += 1;
                if self.n == MARKER_FRAMES {
                    self.enter(Segment::RateRecords);
                    self.scrambler.reset();
                    self.mapper = Mapper::data(self.pad_b.unwrap_or(Bank::get(1, 0).unwrap()));
                    self.record = record::server(false);
                    self.record_at = 0;
                }
                f
            }
            Segment::RateRecords => {
                if self.record_at == 0 && self.n == RECORDS_NAK * 4 {
                    self.record = record::server(true);
                }
                let f = self.record_frame();
                self.n += 1;
                if self.n == RECORDS * 4 {
                    // Then the start: one frame of ones in index 1's levels.
                    self.enter(Segment::Start);
                    self.again = true;
                }
                f
            }
            _ => unreachable!("not a framed segment"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Script, n: usize) -> Vec<u8> {
        let mut ones = || true;
        (0..n).map(|_| s.next(&mut ones)).collect()
    }

    #[test]
    fn the_preamble_follows_its_rule() {
        // Signs Barker-11 low-order first from 05b8, magnitudes from 0712
        // low-order first (0 ab, 1 bd), complemented every other 11.
        for (n, &o) in PREAMBLE.iter().enumerate() {
            let k = n % 11;
            let negative = (0x05b8 >> k) & 1 != 0;
            let loud = ((0x0712 >> k) & 1 != 0) != (n >= 11);
            let level = if loud { 0xbd } else { 0xab };
            assert_eq!(o, if negative { level & 0x7f } else { level }, "octet {n}");
        }
    }

    #[test]
    fn the_script_is_silent_until_the_clients_j_then_keeps_its_lengths() {
        let mut s = Script::new();
        assert!(run(&mut s, 1000).iter().all(|&o| o == SILENCE));
        s.start();
        let lead = run(&mut s, LEAD);
        assert!(lead.iter().all(|&o| o == SILENCE));
        assert_eq!(s.segment(), Segment::Preamble);
        let pre = run(&mut s, PRE);
        assert_eq!(&pre[..22], &PREAMBLE);
        assert_eq!(pre[352], PREAMBLE[0] ^ 0x80);
        assert_eq!(s.segment(), Segment::Trn);
        let trn = run(&mut s, TRN);
        assert!(trn.iter().all(|&o| o | 0x80 == 0xb1));
        // GPC from zero on ones: the first 18 bits come out ones' own.
        assert!(trn[..5].iter().all(|&o| o == 0x31));
        assert_eq!(s.segment(), Segment::JWords);
        run(&mut s, 1200);
        let ones = run(&mut s, ONES);
        assert!(ones.iter().all(|&o| o | 0x80 == 0xc1));
        let align = run(&mut s, 1152);
        assert_eq!(&align[..6], &ALIGNMENT);
        assert_eq!(&align[6..12], &[0x28, 0xa1, 0x21, 0xa8, 0x1e, 0x9e]);
        assert_eq!(s.segment(), Segment::Sync);
    }

    #[test]
    fn the_j_words_descramble_to_09b0_then_099f() {
        let mut s = Script::new();
        s.start();
        run(&mut s, LEAD + PRE + TRN);
        let j = run(&mut s, 1200);
        // The client's descrambler, run from TRN's start.
        let mut d = Scrambler::new(Mode::Call);
        let mut tx = Script::new();
        tx.start();
        let trn = &run(&mut tx, LEAD + PRE + TRN)[LEAD + PRE..];
        for &o in trn {
            d.descramble(o & 0x80 == 0);
        }
        let bits: Vec<bool> = j.iter().map(|&o| d.descramble(o & 0x80 == 0)).collect();
        for (w, chunk) in bits.chunks(12).enumerate() {
            let word = chunk.iter().enumerate().fold(0u16, |v, (k, &b)| v | u16::from(b) << k);
            assert_eq!(word, if w == 99 { 0x099f } else { 0x09b0 }, "word {w}");
        }
    }

    /// Sync until the MP, records until E, the start, data: whole frames,
    /// the sync at least 1667 frames, the records whole.
    #[test]
    fn the_framed_segments_end_on_the_clients_signals() {
        let mut s = Script::new();
        s.start();
        let fixed = LEAD + PRE + TRN + 1200 + ONES + 1152;
        run(&mut s, fixed);
        s.mp(Bank::get(1, 0).unwrap());
        let mut sync = 0;
        while s.segment() == Segment::Sync {
            run(&mut s, FRAME);
            sync += 1;
        }
        assert_eq!(sync, SYNC_FRAMES);
        run(&mut s, 7 * FRAME);
        s.e();
        let mut records = 7;
        while s.segment() == Segment::Record {
            run(&mut s, FRAME);
            records += 1;
        }
        assert_eq!(records % 4, 0, "the records end whole");
        let mut start = 0;
        while s.segment() == Segment::Start {
            run(&mut s, FRAME);
            start += 1;
        }
        assert_eq!(start, START_FRAMES);
        assert!(s.in_data());
    }

    #[test]
    fn a_rate_change_sends_its_marker_records_and_start_then_the_new_levels() {
        let mut s = Script::new();
        s.start();
        run(&mut s, LEAD + PRE + TRN + 1200 + ONES + 1152);
        s.mp(Bank::get(1, 0).unwrap());
        run(&mut s, SYNC_FRAMES * FRAME);
        s.e();
        while !s.in_data() {
            run(&mut s, FRAME);
        }
        run(&mut s, 3 * FRAME + 2);
        // To index 14, behind 3 dB.
        s.rate_change(Bank::get(14, 1).unwrap(), 1);
        run(&mut s, 4); // the frame being sent ends
        assert_eq!(s.segment(), Segment::Data);
        let marker = run(&mut s, MARKER_FRAMES * FRAME);
        // Index 1's largest level, a5, as sent behind 3 dB: 9d.
        assert_eq!(&marker[..6], &[0x9d, 0x9d, 0x9d, 0x1d, 0x1d, 0x1d]);
        assert_eq!(&marker[48..], &[0x1d, 0x1d, 0x1d, 0x9d, 0x9d, 0x9d]);
        assert_eq!(s.segment(), Segment::RateRecords);
        run(&mut s, RECORDS * 4 * FRAME);
        assert_eq!(s.segment(), Segment::Start);
        let mut frames = 0;
        while !s.in_data() {
            run(&mut s, FRAME);
            frames += 1;
        }
        assert_eq!(frames, START_FRAMES + 1, "one frame of index 1's, then 681 of the new");
        assert_eq!(s.bank().b, 36);
        // In all 9 + 172 + 1 frames to the switch to the new levels.
        assert_eq!(MARKER_FRAMES + RECORDS * 4 + 1, 182);
    }
}
