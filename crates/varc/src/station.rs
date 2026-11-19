//! A whole V.ARC end: octets in, octets out, over however many channels.
//!
//! Three things happen above the modulation and they happen here.
//!
//! **Tables.** Each end measures the line it is listening to and works out
//! what that line will bear, and then has to tell the *other* end, since it
//! is the other end that sends on it. So the table goes back the opposite
//! way as a frame, over and over until a superframe arrives marked as sent
//! with it. There is no acknowledgement and no countdown: the mark on the
//! sync symbol is the acknowledgement. If blocks then start arriving that
//! cannot be corrected, the receiver asks again with more in hand, and if it
//! finds itself listening to a table it has never heard of -- it was
//! restarted, say, and the far end was not -- it asks for the starting table.
//!
//! The awkward case is both directions failing at once, because then neither
//! end can tell the other anything. So an end that can read nothing does
//! two things without waiting to be asked: it goes back to the starting
//! table itself, which the far end can read if it can read anything, and on
//! that it asks the far end to do the same.
//!
//! **Retransmission.** The Reed-Solomon code deals with a wrong decision
//! here and there. It does not deal with a click that takes out a whole
//! symbol, and on a loaded line a whole symbol is a whole block. Data frames
//! are numbered, the far end says which it has, and the ones it has not are
//! sent again.
//!
//! **Bonding.** A stereo lead is two lines. Each is its own channel with its
//! own tables, loaded for what it measured, and the numbered frames are
//! dealt out to whichever has room next. They are put back in order by
//! their numbers at the far end, which retransmission needed anyway.

use std::collections::{BTreeMap, VecDeque};

use crate::frame::{Packer, Unpacker};
use crate::link::{Arrived, Receiver, Sender, Table};

/// "Send with this table": a mark, then the table.
const TABLE: u8 = 2;
/// A numbered piece of the octet stream.
const DATA: u8 = 3;
/// Which numbered pieces have arrived.
const ACK: u8 = 4;
/// "Go back to the starting table; I cannot read you."
const RESET: u8 = 5;

/// Marks a table may be given. Zero is the starting table.
const MARKS: u8 = 4;

/// Data frames that may be out and unacknowledged at once.
const WINDOW: usize = 512;

/// The most octets of the stream in one data frame.
const CHUNK: usize = 1024;

#[derive(Debug)]
struct Sent {
    seq: u32,
    body: Vec<u8>,
    first: u64,
    last: u64,
    sends: u32,
    acked: bool,
    /// The far end has a later frame and not this one.
    overtaken: bool,
}

/// Selective-repeat retransmission of an octet stream.
#[derive(Debug)]
pub struct Arq {
    fs: f64,
    outbox: VecDeque<u8>,
    next_seq: u32,
    unacked: VecDeque<Sent>,
    /// The round trip, smoothed, in samples.
    srtt: f64,
    expected: u32,
    held: BTreeMap<u32, Vec<u8>>,
    inbox: Vec<u8>,
    ack_due: bool,
    /// Data frames sent a second time or more.
    pub resent: u64,
    /// Octets of the stream handed to the line for the first time.
    pub sent: u64,
    /// Octets of the stream delivered in order.
    pub delivered: u64,
}

impl Arq {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            outbox: VecDeque::new(),
            next_seq: 0,
            unacked: VecDeque::new(),
            srtt: fs,
            expected: 0,
            held: BTreeMap::new(),
            inbox: Vec::new(),
            ack_due: false,
            resent: 0,
            sent: 0,
            delivered: 0,
        }
    }

    pub fn write(&mut self, octets: &[u8]) {
        self.outbox.extend(octets);
    }

    pub fn read(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inbox)
    }

    /// Octets written and not yet acknowledged by the far end.
    pub fn pending(&self) -> usize {
        self.outbox.len() + self.unacked.iter().filter(|s| !s.acked).map(|s| s.body.len() - 4).sum::<usize>()
    }

    /// The round trip as last measured, in seconds.
    pub fn round_trip(&self) -> f64 {
        self.srtt / self.fs
    }

    /// The next frame worth sending, if there is one: an acknowledgement
    /// that is owed, a frame that needs sending again, or new data.
    fn next_frame(&mut self, now: u64, chunk: usize) -> Option<(u8, Vec<u8>)> {
        if self.ack_due {
            self.ack_due = false;
            let mut body = self.expected.to_le_bytes().to_vec();
            if let Some((&last, _)) = self.held.last_key_value() {
                let span = last.wrapping_sub(self.expected) as usize;
                let mut map = vec![0u8; span.div_ceil(8).min(WINDOW / 8)];
                for &seq in self.held.keys() {
                    let i = seq.wrapping_sub(self.expected) as usize - 1;
                    if i / 8 < map.len() {
                        map[i / 8] |= 1 << (i % 8);
                    }
                }
                body.extend_from_slice(&map);
            }
            return Some((ACK, body));
        }
        // Twice the round trip and a little, or at once if the far end has
        // been seen to have a later frame without this one.
        let patience = (2.0 * self.srtt + 0.05 * self.fs) as u64;
        let quick = self.srtt as u64;
        for sent in &mut self.unacked {
            let waited = now.saturating_sub(sent.last);
            if !sent.acked && (waited > patience || (sent.overtaken && waited > quick)) {
                sent.last = now;
                sent.sends += 1;
                sent.overtaken = false;
                self.resent += 1;
                return Some((DATA, sent.body.clone()));
            }
        }
        if self.outbox.is_empty() || self.unacked.len() >= WINDOW {
            return None;
        }
        let take = self.outbox.len().min(chunk).min(CHUNK);
        let mut body = self.next_seq.to_le_bytes().to_vec();
        body.extend(self.outbox.drain(..take));
        self.sent += take as u64;
        self.unacked.push_back(Sent {
            seq: self.next_seq,
            body: body.clone(),
            first: now,
            last: now,
            sends: 1,
            acked: false,
            overtaken: false,
        });
        self.next_seq = self.next_seq.wrapping_add(1);
        Some((DATA, body))
    }

    fn on_data(&mut self, body: &[u8]) {
        if body.len() < 4 {
            return;
        }
        self.ack_due = true;
        let seq = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        let ahead = seq.wrapping_sub(self.expected);
        if ahead as usize >= WINDOW {
            // Behind what has been delivered, or absurdly far in front.
            return;
        }
        self.held.entry(seq).or_insert_with(|| body[4..].to_vec());
        while let Some(piece) = self.held.remove(&self.expected) {
            self.delivered += piece.len() as u64;
            self.inbox.extend_from_slice(&piece);
            self.expected = self.expected.wrapping_add(1);
        }
    }

    fn on_ack(&mut self, now: u64, body: &[u8]) {
        if body.len() < 4 {
            return;
        }
        let expected = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        let map = &body[4..];
        let mut highest: Option<u32> = None;
        for sent in &mut self.unacked {
            let behind = expected.wrapping_sub(sent.seq);
            let has = if behind != 0 && behind < 1 << 31 {
                true
            } else {
                let i = sent.seq.wrapping_sub(expected) as usize;
                i >= 1 && (i - 1) / 8 < map.len() && map[(i - 1) / 8] >> ((i - 1) % 8) & 1 != 0
            };
            if has && !sent.acked {
                sent.acked = true;
                if sent.sends == 1 {
                    let rtt = now.saturating_sub(sent.first) as f64;
                    self.srtt += (rtt - self.srtt) / 8.0;
                }
            }
            if has {
                highest = Some(sent.seq);
            }
        }
        if let Some(highest) = highest {
            for sent in &mut self.unacked {
                if !sent.acked && highest.wrapping_sub(sent.seq) < 1 << 31 {
                    sent.overtaken = true;
                }
            }
        }
        while self.unacked.front().is_some_and(|s| s.acked) {
            self.unacked.pop_front();
        }
    }
}

/// What a channel has seen go by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Blocks of octets that arrived.
    pub blocks: u64,
    /// Blocks that could not be read at all.
    pub lost: u64,
    /// Codewords in the blocks that arrived, and how many were beyond repair.
    pub words: u64,
    pub failed: u64,
    /// Octets the code put right.
    pub corrected: u64,
    /// Tables this end has asked the far end to send with.
    pub proposals: u64,
}

/// One line each way: a transmitter, a receiver, and the tables between
/// them.
#[derive(Debug)]
pub struct Channel<T, R> {
    pub tx: T,
    pub rx: R,
    packer: Packer,
    unpacker: Unpacker,
    next_mark: u8,
    /// The table the far end has been asked to send with, until it does.
    offered: Option<(u8, Table)>,
    lost_run: usize,
    trouble: u32,
    /// Since when nearly every codeword has been beyond repair, if they are.
    starved_since: Option<u64>,
    /// The far end is to be asked for the starting table until it sends it.
    wants_reset: bool,
    /// When a table was last proposed, so that asking again waits to see
    /// what the last one did.
    proposed_at: u64,
    pub tally: Tally,
}

impl<T: Sender, R: Receiver> Channel<T, R> {
    pub fn new(tx: T, rx: R) -> Self {
        Self::numbered(tx, rx, 0)
    }

    /// A channel that is one of several in the same lead.
    fn numbered(tx: T, rx: R, number: u32) -> Self {
        Self {
            tx,
            rx,
            packer: Packer::seeded(number.wrapping_mul(0x9e37_79b9)),
            unpacker: Unpacker::new(),
            next_mark: 1,
            offered: None,
            lost_run: 0,
            trouble: 0,
            starved_since: None,
            wants_reset: false,
            proposed_at: 0,
            tally: Tally::default(),
        }
    }

    /// Whether both directions of this channel are running on tables made
    /// to measure: the far end is sending with the one this end asked for,
    /// and this end with one the far end asked for.
    pub fn loaded(&self) -> bool {
        self.offered.as_ref().is_some_and(|(mark, _)| self.rx.id() == Some(*mark)) && self.tx.id() != 0
    }

    /// The table the far end has been asked for and is not yet using.
    pub fn waiting_for(&self) -> Option<u8> {
        self.offered.as_ref().map(|(mark, _)| *mark).filter(|&mark| self.rx.id() != Some(mark))
    }

    fn step(&mut self, input: f64, arq: &mut Arq, now: u64, fs: f64) -> f64 {
        self.rx.feed(input);
        if let Some(table) = self.rx.proposal() {
            let mark = self.next_mark;
            self.next_mark = if mark + 1 >= MARKS { 1 } else { mark + 1 };
            self.rx.expect(mark, table.clone());
            self.offered = Some((mark, table));
            self.proposed_at = now;
            self.tally.proposals += 1;
        }
        while let Some(arrived) = self.rx.take() {
            match arrived {
                Arrived::Octets { line, layout } => {
                    let report = self.unpacker.take(&line, &layout, None);
                    self.tally.blocks += 1;
                    self.tally.words += report.words as u64;
                    self.tally.failed += report.failed as u64;
                    self.tally.corrected += report.corrected as u64;
                    self.lost_run = 0;
                    self.trouble = if report.failed > 0 { self.trouble + 4 } else { self.trouble.saturating_sub(1) };
                    if report.failed * 4 >= report.words * 3 {
                        self.starved_since.get_or_insert(now);
                    } else {
                        self.starved_since = None;
                    }
                }
                Arrived::Lost => {
                    self.tally.lost += 1;
                    self.lost_run += 1;
                    self.unpacker.lost();
                }
            }
        }
        // Three bad blocks close together on a table of this end's own
        // choosing means it chose too high. Two seconds between changes of
        // mind, so that the last change has been heard before the next.
        let own = self.offered.as_ref().is_some_and(|(mark, _)| self.rx.id() == Some(*mark));
        if own && self.starved_since.is_some_and(|since| now - since > fs as u64) {
            // A second of nothing readable. The table was far too high, not
            // a little, and the far end may be as deaf as this one: start
            // again from the starting table, both ways.
            self.starved_since = None;
            self.trouble = 0;
            self.offered = None;
            self.wants_reset = true;
            self.tx.reset();
            self.rx.remeasure(3.0);
        } else if self.trouble >= 12 && own && now - self.proposed_at > (2.0 * fs) as u64 {
            self.trouble = 0;
            self.rx.back_off(1.5);
        }
        if self.rx.id() == Some(0) {
            self.wants_reset = false;
        }
        while let Some((kind, body)) = self.unpacker.pop() {
            match kind {
                TABLE => {
                    if let Some((&mark, rest)) = body.split_first()
                        && (1..MARKS).contains(&mark)
                        && mark != self.tx.id()
                        && let Some(table) = Table::decode(rest)
                    {
                        self.tx.adopt(mark, table);
                    }
                }
                RESET => {
                    if self.tx.id() != 0 {
                        self.tx.reset();
                    }
                }
                DATA => arq.on_data(&body),
                ACK => arq.on_ack(now, &body),
                _ => {}
            }
        }

        if let Some(layout) = self.tx.wants() {
            if !self.packer.urgent_waiting() {
                if self.lost_run >= 2 || self.wants_reset {
                    self.packer.push_urgent(RESET, &[]);
                } else if let Some((mark, table)) = &self.offered
                    && self.rx.id() != Some(*mark)
                {
                    let mut body = vec![*mark];
                    body.extend_from_slice(&table.encode());
                    self.packer.push_urgent(TABLE, &body);
                }
            }
            let room = layout.payload();
            let chunk = room.saturating_sub(16).clamp(32, CHUNK);
            // The stream goes only down a channel that is alive. A lead with
            // one channel connected is a perfectly good line, and the other
            // channel's transmitter would otherwise take its share of the
            // frames and send them to nobody. This end cannot see whether
            // the far end hears it; it can see whether it hears the far
            // end on the same channel, and a channel is a wire each way.
            let live = self.rx.health().locked;
            while live && self.packer.backlog() < room {
                match arq.next_frame(now, chunk) {
                    Some((ACK, body)) => self.packer.push_urgent(ACK, &body),
                    Some((kind, body)) => self.packer.push(kind, &body),
                    None => break,
                }
            }
            let line = self.packer.fill(&layout);
            self.tx.give(line);
        }
        self.tx.next_sample()
    }
}

/// A V.ARC end: one octet stream over one channel or several.
#[derive(Debug)]
pub struct Station<T, R> {
    pub channels: Vec<Channel<T, R>>,
    pub arq: Arq,
    fs: f64,
    now: u64,
}

impl<T: Sender, R: Receiver> Station<T, R> {
    /// An end with a channel for each transmitter and receiver given.
    pub fn new(fs: f64, pairs: Vec<(T, R)>) -> Self {
        Self {
            channels: pairs.into_iter().zip(0..).map(|((tx, rx), c)| Channel::numbered(tx, rx, c)).collect(),
            arq: Arq::new(fs),
            fs,
            now: 0,
        }
    }

    /// One sample of every channel in, one of every channel out.
    pub fn step(&mut self, input: &[f64], output: &mut [f64]) {
        self.now += 1;
        for ((channel, &x), y) in self.channels.iter_mut().zip(input).zip(output.iter_mut()) {
            *y = channel.step(x, &mut self.arq, self.now, self.fs);
        }
    }

    /// Octets for the far end.
    pub fn write(&mut self, octets: &[u8]) {
        self.arq.write(octets);
    }

    /// Octets from the far end, in the order they were written there.
    pub fn read(&mut self) -> Vec<u8> {
        self.arq.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dmt::{self, Plan};
    use crate::interp::Interp;

    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gaussian(&mut self) -> f64 {
            (-2.0 * self.uniform().ln()).sqrt() * (std::f64::consts::TAU * self.uniform()).cos()
        }
    }

    /// A wire one way: a clock that disagrees, noise, and a switch to cut it.
    struct Wire {
        clock: Interp,
        noise: Noise,
        sigma: f64,
        cut: bool,
        pending: VecDeque<f64>,
    }

    impl Wire {
        fn new(seed: u64, snr_db: f64, ppm: f64) -> Self {
            let mut clock = Interp::new();
            clock.adjust_step(ppm * 1e-6);
            Self {
                clock,
                noise: Noise(seed),
                sigma: dmt::LEVEL * 10f64.powf(-snr_db / 20.0),
                cut: false,
                // A real far end samples at its own rate and never runs
                // short. Here both ends are stepped together, so the wire
                // starts with enough in hand that a slow clock does not
                // empty it before the test is over.
                pending: VecDeque::from(vec![0.0; 256]),
            }
        }

        /// A sample in at one end; a sample out at the other, the far end's
        /// clock being what it is.
        fn carry(&mut self, x: f64) -> f64 {
            let x = if self.cut { 0.0 } else { x };
            let (pending, noise, sigma) = (&mut self.pending, &mut self.noise, self.sigma);
            self.clock.feed(x, |y| pending.push_back(y + sigma * noise.gaussian()));
            self.pending.pop_front().expect("the wire ran dry: the test is too long for its slack")
        }
    }

    type End = Station<dmt::Tx, dmt::Rx>;

    fn end(channels: usize) -> End {
        let plan = Plan::for_rate(48_000.0);
        Station::new(48_000.0, (0..channels).map(|_| (dmt::Tx::new(plan.clone()), dmt::Rx::new(plan.clone()))).collect())
    }

    fn stream(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + seed * 17 + (i >> 5) * 3 + (i >> 11)) as u8).collect()
    }

    struct Pair {
        a: End,
        b: End,
        /// Wires from A to B and from B to A, one of each a channel.
        there: Vec<Wire>,
        back: Vec<Wire>,
        from_a: Vec<f64>,
        from_b: Vec<f64>,
        at_a: Vec<u8>,
        at_b: Vec<u8>,
    }

    impl Pair {
        fn new(channels: usize, snr_db: &[f64], ppm: f64) -> Self {
            Self {
                a: end(channels),
                b: end(channels),
                there: (0..channels).map(|c| Wire::new(11 + c as u64, snr_db[c], ppm)).collect(),
                back: (0..channels).map(|c| Wire::new(23 + c as u64, snr_db[c], -ppm)).collect(),
                from_a: vec![0.0; channels],
                from_b: vec![0.0; channels],
                at_a: Vec::new(),
                at_b: Vec::new(),
            }
        }

        fn run(&mut self, seconds: f64) {
            let channels = self.there.len();
            let mut to_a = vec![0.0; channels];
            let mut to_b = vec![0.0; channels];
            for _ in 0..(seconds * 48_000.0) as usize {
                for c in 0..channels {
                    to_b[c] = self.there[c].carry(self.from_a[c]);
                    to_a[c] = self.back[c].carry(self.from_b[c]);
                }
                self.a.step(&to_a, &mut self.from_a);
                self.b.step(&to_b, &mut self.from_b);
                self.at_a.extend(self.a.read());
                self.at_b.extend(self.b.read());
            }
        }
    }

    #[test]
    fn two_ends_load_each_others_lines_and_carry_a_stream_each_way() {
        let mut pair = Pair::new(1, &[50.0], 40.0);
        let (to_b, to_a) = (stream(1, 400_000), stream(2, 150_000));
        pair.a.write(&to_b);
        pair.b.write(&to_a);
        pair.run(25.0);
        assert_eq!(pair.at_b.len(), to_b.len());
        assert!(pair.at_b == to_b);
        assert!(pair.at_a == to_a);
        for end in [&pair.a, &pair.b] {
            let channel = &end.channels[0];
            assert_eq!(channel.tally.proposals, 1);
            assert_eq!(channel.tx.id(), 1);
            assert_eq!(channel.waiting_for(), None);
            assert_eq!(channel.tally.failed, 0);
        }
        assert!(pair.a.arq.round_trip() < 0.3, "{} s", pair.a.arq.round_trip());
    }

    #[test]
    fn a_cut_in_the_line_costs_time_and_no_octets() {
        let mut pair = Pair::new(1, &[45.0], -25.0);
        let to_b = stream(3, 300_000);
        pair.a.write(&to_b);
        pair.run(8.0);
        // A fifth of a second of nothing: several whole blocks gone.
        pair.there[0].cut = true;
        pair.run(0.2);
        pair.there[0].cut = false;
        pair.run(20.0);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
        assert!(pair.a.arq.resent > 0);
    }

    #[test]
    fn a_stereo_pair_is_two_lines_each_loaded_for_itself() {
        let mut pair = Pair::new(2, &[60.0, 35.0], 15.0);
        let to_b = stream(4, 600_000);
        pair.a.write(&to_b);
        pair.run(20.0);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
        let plan = Plan::for_rate(48_000.0);
        let rate = |c: usize| plan.net(pair.a.channels[c].tx.table());
        assert!(rate(0) > 1.5 * rate(1), "{} and {}", rate(0), rate(1));
    }

    #[test]
    fn an_end_that_starts_again_is_sent_the_starting_table_and_recovers() {
        let mut pair = Pair::new(1, &[50.0], 0.0);
        pair.run(6.0);
        assert_eq!(pair.a.channels[0].tx.id(), 1);
        // B is switched off and on. A is still sending with a table the new
        // B has never seen.
        pair.b = end(1);
        pair.at_b.clear();
        let to_b = stream(5, 100_000);
        pair.a.write(&to_b);
        pair.run(15.0);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
        assert!(pair.b.channels[0].tally.lost > 0);
    }

    #[test]
    fn a_table_chosen_too_high_is_chosen_again_lower() {
        let mut pair = Pair::new(1, &[45.0], 0.0);
        pair.run(6.0);
        let plan = Plan::for_rate(48_000.0);
        let before = plan.net(pair.a.channels[0].tx.table());
        // The line gets six decibels noisier than it was when measured.
        pair.there[0].sigma *= 2.0;
        let to_b = stream(6, 200_000);
        pair.a.write(&to_b);
        pair.run(30.0);
        let after = plan.net(pair.a.channels[0].tx.table());
        assert!(after < before, "{before} then {after}");
        assert!(pair.b.channels[0].tally.proposals > 1);
        assert!(pair.at_b == to_b, "{} of {} octets", pair.at_b.len(), to_b.len());
    }
}

#[cfg(test)]
mod crosstalk {
    use super::*;
    use crate::dmt::{self, Plan};

    type End = Station<dmt::Tx, dmt::Rx>;

    fn stream(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + seed * 17 + (i >> 5) * 3 + (i >> 11)) as u8).collect()
    }

    /// Two ends on one stereo lead, one sending on each channel, with a
    /// little of each channel leaking into the other: what a loop test on a
    /// real sound card is.
    fn leaky(coloured: bool, seconds: f64) -> (End, End, Vec<u8>, Vec<u8>) {
        let plan = Plan::for_rate(48_000.0);
        let end = |tx: u8, rx: u8| {
            Station::new(
                48_000.0,
                vec![(dmt::Tx::new(plan.clone().coloured(tx)), dmt::Rx::new(plan.clone().coloured(rx)))],
            )
        };
        let (mut a, mut b) = if coloured { (end(0, 1), end(1, 0)) } else { (end(0, 0), end(0, 0)) };
        let sent = stream(9, 150_000);
        a.write(&sent);
        let leak = 10f64.powf(-45.0 / 20.0);
        let (mut from_a, mut from_b) = ([0.0], [0.0]);
        let mut got = Vec::new();
        for _ in 0..(seconds * 48_000.0) as usize {
            let (left, right) = (from_a[0] + leak * from_b[0], from_b[0] + leak * from_a[0]);
            a.step(&[right], &mut from_a);
            b.step(&[left], &mut from_b);
            got.extend(b.read());
        }
        (a, b, sent, got)
    }

    #[test]
    fn a_leak_from_the_other_channel_is_measured_when_the_lines_are_told_apart() {
        let (a, b, sent, got) = leaky(true, 12.0);
        assert!(got == sent, "{} of {} octets", got.len(), sent.len());
        assert_eq!(b.channels[0].tally.proposals, 1);
        assert_eq!(b.channels[0].tally.failed, 0);
        let snr = b.channels[0].rx.health().snr_db;
        assert!((snr - 45.0).abs() < 2.0, "measured {snr:.1} dB against a leak 45 dB down");
        assert_eq!(a.channels[0].tx.id(), 1);
    }

    #[test]
    fn two_ends_that_deafen_each_other_both_start_again_and_recover() {
        // Without their own known points the two ends measure a perfect
        // line, load it to the limit and can then read nothing, either way.
        let (a, b, sent, got) = leaky(false, 40.0);
        assert!(b.channels[0].tally.proposals > 1);
        assert!(b.channels[0].tally.failed > 0);
        assert!(got == sent, "{} of {} octets", got.len(), sent.len());
        assert_ne!(a.channels[0].tx.id(), 0);
    }
}
