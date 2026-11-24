//! Octets on the line: what a superframe carries, whatever carried it.
//!
//! Each V.ARC modulation ends up with the same thing to offer, which is room
//! for so many octets every superframe. This is what goes in the room, and it
//! is the same for all of them.
//!
//! From the outside in:
//!
//! - **Scrambling.** The whole superframe is exclusive-ored with a fixed
//!   pseudo-random sequence, so that a line with nothing to say does not send
//!   the same point on every carrier.
//! - **Reed-Solomon.** The room is cut into as few codewords as will fit,
//!   all within an octet of the same length, and they are sent a column at a
//!   time: the first octet of each, then the second of each. Whatever wrecks
//!   a stretch of the line wrecks a little of every codeword rather than all
//!   of one.
//! - **A header** of three octets: a count that goes up by one every
//!   superframe, and a pointer to where the first frame *starting* in this
//!   superframe starts. A receiver that lost the last superframe picks the
//!   stream up again at the pointer.
//! - **Frames**, end to end: a kind, a length, a body and a CRC-32. A frame
//!   runs on into the next superframe if it has to. A zero where a kind
//!   should be means nothing more is coming in this superframe, and what
//!   follows the zero is filler that is different every time. It has to be:
//!   a line with nothing to say would otherwise say the same nothing in
//!   every block, and a signal that repeats is a set of tones. Two lines
//!   side by side both repeating would each leak into the other as a fixed
//!   offset on every carrier rather than as noise, and a receiver's running
//!   estimate of the line drifts off after it.

use std::collections::VecDeque;

use ec::hdlc::{Fcs, append_fcs, fcs_ok};

use crate::rs::{MAX_BLOCK, ReedSolomon};

/// Octets of header at the front of every superframe's payload.
const HEADER: usize = 3;

/// The pointer value that says no frame starts in this superframe.
const NO_FRAME: u16 = 0xffff;

/// Kind, length, and the CRC that follows the body.
const FRAME_OVERHEAD: usize = 3 + 4;

/// The longest body a frame may have.
pub const MAX_BODY: usize = 8192;

/// How a superframe's room is divided into codewords.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The length of each codeword, check octets included.
    words: Vec<usize>,
    parity: usize,
}

impl Layout {
    /// Divide `capacity` octets among codewords of `parity` check octets, or
    /// `None` if that leaves nothing worth having.
    pub fn new(capacity: usize, parity: usize) -> Option<Self> {
        if capacity < parity + HEADER + FRAME_OVERHEAD + 1 {
            return None;
        }
        let count = capacity.div_ceil(MAX_BLOCK);
        let base = capacity / count;
        let extra = capacity % count;
        if base <= parity + 1 {
            return None;
        }
        let words = (0..count).map(|i| base + usize::from(i < extra)).collect();
        Some(Self { words, parity })
    }

    /// The code as it would be written, (n, k): octets in the longest
    /// codeword and how many of them are data. The others are an octet
    /// shorter at most.
    pub fn code(&self) -> (usize, usize) {
        let n = self.words.iter().copied().max().unwrap_or(0);
        (n, n.saturating_sub(self.parity))
    }

    /// Octets of payload, header included, the superframe carries.
    pub fn payload(&self) -> usize {
        self.words.iter().map(|w| w - self.parity).sum()
    }

    pub fn capacity(&self) -> usize {
        self.words.iter().sum()
    }

    pub fn words(&self) -> usize {
        self.words.len()
    }

    /// Where on the line each octet of each codeword goes: `order[w][i]` is
    /// the place of octet `i` of codeword `w`.
    fn order(&self) -> Vec<Vec<usize>> {
        let mut order: Vec<Vec<usize>> = self.words.iter().map(|&n| Vec::with_capacity(n)).collect();
        let longest = self.words.iter().copied().max().unwrap_or(0);
        let mut at = 0;
        for i in 0..longest {
            for (w, &n) in self.words.iter().enumerate() {
                if i < n {
                    order[w].push(at);
                    at += 1;
                }
            }
        }
        order
    }
}

/// Exclusive-or `octets` with the scrambling sequence, which undoes itself.
///
/// x^15 + x^14 + 1, the generator a DVB transport stream is randomised with,
/// restarted at every superframe so that losing one costs nothing in the next.
fn scramble(octets: &mut [u8]) {
    let mut state: u16 = 0x4a80;
    for octet in octets {
        let mut mask = 0u8;
        for _ in 0..8 {
            let bit = (state >> 14 ^ state >> 13) & 1;
            state = (state << 1 | bit) & 0x7fff;
            mask = mask << 1 | bit as u8;
        }
        *octet ^= mask;
    }
}

fn encode_frame(kind: u8, body: &[u8]) -> Vec<u8> {
    assert!(kind != 0, "kind zero is the idle marker");
    assert!(body.len() <= MAX_BODY, "frame body too long");
    let mut frame = Vec::with_capacity(body.len() + FRAME_OVERHEAD);
    frame.push(kind);
    frame.extend_from_slice(&(body.len() as u16).to_le_bytes());
    frame.extend_from_slice(body);
    append_fcs(&frame, Fcs::Bits32)
}

/// Fills superframes with frames.
#[derive(Debug, Default)]
pub struct Packer {
    /// Frames that jump the queue: the ones the link itself runs on.
    urgent: VecDeque<Vec<u8>>,
    ordinary: VecDeque<Vec<u8>>,
    /// What is left of a frame the last superframe had no room to finish.
    carried: Vec<u8>,
    count: u8,
    /// Where the filler has got to.
    filler: u32,
    codes: Vec<ReedSolomon>,
}

impl Packer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A packer whose filler is its own. Two lines in one lead want
    /// different ones, for the reason the filler is there at all.
    pub fn seeded(seed: u32) -> Self {
        Self { filler: seed, ..Self::default() }
    }

    /// Queue a frame behind whatever is already waiting.
    pub fn push(&mut self, kind: u8, body: &[u8]) {
        self.ordinary.push_back(encode_frame(kind, body));
    }

    /// Queue a frame ahead of everything not already part-sent.
    pub fn push_urgent(&mut self, kind: u8, body: &[u8]) {
        self.urgent.push_back(encode_frame(kind, body));
    }

    /// Octets waiting in ordinary frames, overheads included.
    pub fn backlog(&self) -> usize {
        self.carried.len() + self.ordinary.iter().map(Vec::len).sum::<usize>()
    }

    /// Whether an urgent frame is still waiting to start.
    pub fn urgent_waiting(&self) -> bool {
        !self.urgent.is_empty()
    }

    /// The next superframe: exactly `layout.capacity()` octets for the line.
    pub fn fill(&mut self, layout: &Layout) -> Vec<u8> {
        let room = layout.payload();
        let mut payload = Vec::with_capacity(room);
        payload.push(self.count);
        self.count = self.count.wrapping_add(1);
        payload.extend_from_slice(&NO_FRAME.to_le_bytes());

        let take = self.carried.len().min(room - HEADER);
        payload.extend(self.carried.drain(..take));
        let mut pointer = NO_FRAME;
        while self.carried.is_empty() && payload.len() < room {
            let Some(frame) = self.urgent.pop_front().or_else(|| self.ordinary.pop_front()) else {
                break;
            };
            if pointer == NO_FRAME {
                pointer = (payload.len() - HEADER) as u16;
            }
            let take = frame.len().min(room - payload.len());
            payload.extend_from_slice(&frame[..take]);
            self.carried = frame[take..].to_vec();
        }
        payload[1..HEADER].copy_from_slice(&pointer.to_le_bytes());
        if payload.len() < room {
            payload.push(0);
        }
        while payload.len() < room {
            self.filler = self.filler.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            payload.push((self.filler >> 24) as u8);
        }

        // No check octets is no code: the payload goes as it is.
        let code = (layout.parity > 0).then(|| code_for(&mut self.codes, layout.parity).clone());
        let order = layout.order();
        let mut line = vec![0u8; layout.capacity()];
        let mut parity = vec![0u8; layout.parity];
        let mut at = 0;
        for (w, &n) in layout.words.iter().enumerate() {
            let k = n - layout.parity;
            let data = &payload[at..at + k];
            if let Some(code) = code.as_ref() {
                code.encode(data, &mut parity);
            }
            for (i, &octet) in data.iter().chain(parity.iter()).enumerate() {
                line[order[w][i]] = octet;
            }
            at += k;
        }
        scramble(&mut line);
        line
    }
}

fn code_for(codes: &mut Vec<ReedSolomon>, parity: usize) -> &ReedSolomon {
    if let Some(i) = codes.iter().position(|c| c.parity() == parity) {
        return &codes[i];
    }
    codes.push(ReedSolomon::new(parity));
    codes.last().expect("just pushed")
}

/// What became of one superframe's codewords.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Decoded {
    pub words: usize,
    /// Codewords beyond repair.
    pub failed: usize,
    /// Octets put right across the ones that were not.
    pub corrected: usize,
}

/// Takes superframes apart into frames.
#[derive(Debug, Default)]
pub struct Unpacker {
    /// The frame being gathered, as far as it has got.
    partial: Vec<u8>,
    /// The count the next superframe should carry, if the last one was good
    /// to its end and so a frame may run on from it.
    expect: Option<u8>,
    frames: VecDeque<(u8, Vec<u8>)>,
    codes: Vec<ReedSolomon>,
    /// Frames thrown away for a bad CRC.
    pub bad_frames: u64,
}

impl Unpacker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next whole frame: its kind and its body.
    pub fn pop(&mut self) -> Option<(u8, Vec<u8>)> {
        self.frames.pop_front()
    }

    /// A superframe that never arrived or could not be read at all.
    pub fn lost(&mut self) {
        self.partial.clear();
        self.expect = None;
    }

    /// One superframe off the line. `erased` marks octets known to be
    /// missing, where the carrier can tell.
    pub fn take(&mut self, line: &[u8], layout: &Layout, erased: Option<&[bool]>) -> Decoded {
        let bad = self.bad_frames;
        let mut report = self.unpack(line, layout, erased);
        // With no check octets there is no codeword to fail, and the only
        // sign of a line that is not bearing it is a frame that fails its
        // own check. That counts as one, or nothing would ever ask for a
        // code again.
        if layout.parity == 0 && self.bad_frames > bad {
            report.failed = report.failed.max(1);
        }
        report
    }

    fn unpack(&mut self, line: &[u8], layout: &Layout, erased: Option<&[bool]>) -> Decoded {
        assert_eq!(line.len(), layout.capacity());
        let mut line = line.to_vec();
        scramble(&mut line);
        let code = (layout.parity > 0).then(|| code_for(&mut self.codes, layout.parity).clone());
        let order = layout.order();
        let mut payload = Vec::with_capacity(layout.payload());
        let mut report = Decoded { words: layout.words.len(), ..Decoded::default() };
        // How much of the payload can be trusted: up to the first codeword
        // that would not decode.
        let mut good = usize::MAX;
        let mut word = Vec::with_capacity(MAX_BLOCK);
        let mut holes = Vec::new();
        for (w, &n) in layout.words.iter().enumerate() {
            word.clear();
            word.extend(order[w].iter().map(|&at| line[at]));
            holes.clear();
            if let Some(erased) = erased {
                holes.extend((0..n).filter(|&i| erased[order[w][i]]));
            }
            let read = match code.as_ref() {
                Some(code) => code.decode(&mut word, &holes).map_err(|_| ()),
                // Nothing to put a missing octet back with.
                None if holes.is_empty() => Ok(0),
                None => Err(()),
            };
            match read {
                Ok(changed) => report.corrected += changed,
                Err(()) => {
                    report.failed += 1;
                    good = good.min(payload.len());
                }
            }
            payload.extend_from_slice(&word[..n - layout.parity]);
        }
        let good = good.min(payload.len());
        if good < HEADER {
            self.lost();
            return report;
        }

        let count = payload[0];
        let pointer = u16::from_le_bytes([payload[1], payload[2]]);
        let body = &payload[HEADER..good];
        let fresh = if pointer == NO_FRAME { usize::MAX } else { usize::from(pointer) };
        let mut at = 0;
        if self.expect == Some(count) && !self.partial.is_empty() {
            // A frame running on from the last superframe, which ends where
            // the first new one starts or not in this superframe at all.
            let end = fresh.min(body.len());
            self.gather(&body[..end]);
            at = end;
        } else {
            self.partial.clear();
        }
        if fresh <= body.len() {
            if at != fresh {
                self.partial.clear();
            }
            at = fresh;
            while at < body.len() {
                if self.partial.is_empty() && body[at] == 0 {
                    break;
                }
                let used = self.gather(&body[at..]);
                at += used;
            }
        }
        if good == payload.len() {
            self.expect = Some(count.wrapping_add(1));
        } else {
            self.lost();
        }
        report
    }

    /// Feed octets to the frame being gathered, stopping at its end. Returns
    /// how many were used.
    fn gather(&mut self, octets: &[u8]) -> usize {
        let mut used = 0;
        while used < octets.len() {
            let want = if self.partial.len() < 3 {
                3
            } else {
                let len = usize::from(u16::from_le_bytes([self.partial[1], self.partial[2]]));
                if len > MAX_BODY {
                    // Not a frame. Give up on this superframe's stream.
                    self.partial.clear();
                    self.bad_frames += 1;
                    return octets.len();
                }
                len + FRAME_OVERHEAD
            };
            let take = (want - self.partial.len()).min(octets.len() - used);
            self.partial.extend_from_slice(&octets[used..used + take]);
            used += take;
            if self.partial.len() >= 3 {
                let len = usize::from(u16::from_le_bytes([self.partial[1], self.partial[2]]));
                if len <= MAX_BODY && self.partial.len() == len + FRAME_OVERHEAD {
                    let frame = std::mem::take(&mut self.partial);
                    if fcs_ok(&frame, Fcs::Bits32) {
                        self.frames.push_back((frame[0], frame[3..3 + len].to_vec()));
                    } else {
                        self.bad_frames += 1;
                    }
                    return used;
                }
            }
        }
        used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + seed * 7 + (i >> 3)) as u8).collect()
    }

    #[test]
    fn a_layout_uses_all_the_room_in_codewords_of_nearly_equal_length() {
        for capacity in [30, 255, 256, 700, 1021, 46_080] {
            let layout = Layout::new(capacity, 16).expect("room enough");
            assert_eq!(layout.capacity(), capacity);
            let longest = layout.words.iter().max().unwrap();
            let shortest = layout.words.iter().min().unwrap();
            assert!(longest - shortest <= 1 && *longest <= MAX_BLOCK);
            let mut places: Vec<usize> = layout.order().into_iter().flatten().collect();
            places.sort_unstable();
            assert_eq!(places, (0..capacity).collect::<Vec<_>>());
        }
        assert!(Layout::new(20, 16).is_none());
    }

    #[test]
    fn frames_cross_superframes_and_come_out_whole_and_in_order() {
        let layout = Layout::new(300, 16).unwrap();
        let mut packer = Packer::new();
        let mut unpacker = Unpacker::new();
        let sent: Vec<(u8, Vec<u8>)> =
            (0..40).map(|i| (1 + (i % 5) as u8, body(i, [0, 1, 17, 250, 700, 2000][i % 6]))).collect();
        for (kind, b) in &sent {
            packer.push(*kind, b);
        }
        let mut got = Vec::new();
        for _ in 0..200 {
            let line = packer.fill(&layout);
            let report = unpacker.take(&line, &layout, None);
            assert_eq!(report.failed, 0);
            while let Some(frame) = unpacker.pop() {
                got.push(frame);
            }
        }
        assert_eq!(got, sent);
        assert_eq!(unpacker.bad_frames, 0);
    }

    #[test]
    fn an_idle_line_never_says_the_same_thing_twice() {
        let layout = Layout::new(300, 16).unwrap();
        let mut packer = Packer::new();
        let mut unpacker = Unpacker::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let line = packer.fill(&layout);
            unpacker.take(&line, &layout, None);
            assert!(seen.insert(line[40..].to_vec()));
        }
        assert_eq!(unpacker.pop(), None);
        assert_eq!(unpacker.bad_frames, 0);
    }

    #[test]
    fn an_urgent_frame_goes_ahead_of_the_queue_but_not_into_a_frame_under_way() {
        let layout = Layout::new(120, 16).unwrap();
        let mut packer = Packer::new();
        let mut unpacker = Unpacker::new();
        packer.push(1, &body(1, 300));
        packer.push(1, &body(2, 10));
        let first = packer.fill(&layout);
        packer.push_urgent(2, &body(3, 5));
        let mut got = Vec::new();
        unpacker.take(&first, &layout, None);
        for _ in 0..8 {
            let line = packer.fill(&layout);
            unpacker.take(&line, &layout, None);
            while let Some((kind, b)) = unpacker.pop() {
                got.push((kind, b.len()));
            }
        }
        assert_eq!(got, vec![(1, 300), (2, 5), (1, 10)]);
    }

    #[test]
    fn a_burst_across_the_line_is_shared_out_and_corrected() {
        let layout = Layout::new(2000, 16).unwrap();
        let mut packer = Packer::new();
        let mut unpacker = Unpacker::new();
        packer.push(3, &body(9, 1500));
        let mut line = packer.fill(&layout);
        // Eight codewords, so sixty octets in a row is eight from each.
        for octet in &mut line[400..460] {
            *octet ^= 0x5a;
        }
        let report = unpacker.take(&line, &layout, None);
        assert_eq!(report.failed, 0);
        assert_eq!(report.corrected, 60);
        assert_eq!(unpacker.pop(), Some((3, body(9, 1500))));
    }

    #[test]
    fn missing_octets_that_are_pointed_out_cost_half_as_much() {
        let layout = Layout::new(2000, 16).unwrap();
        let mut packer = Packer::new();
        let mut unpacker = Unpacker::new();
        packer.push(3, &body(4, 1500));
        let mut line = packer.fill(&layout);
        let mut erased = vec![false; line.len()];
        for at in 1000..1120 {
            line[at] = 0;
            erased[at] = true;
        }
        let report = unpacker.take(&line, &layout, Some(&erased));
        assert_eq!(report.failed, 0);
        assert_eq!(unpacker.pop(), Some((3, body(4, 1500))));
    }

    #[test]
    fn a_lost_superframe_costs_the_frames_in_it_and_no_others() {
        let layout = Layout::new(300, 16).unwrap();
        let mut packer = Packer::new();
        let mut unpacker = Unpacker::new();
        for i in 0..30 {
            packer.push(1, &[i as u8; 100]);
        }
        let mut got = Vec::new();
        for n in 0..20 {
            let mut line = packer.fill(&layout);
            if n == 5 {
                for octet in &mut line {
                    *octet ^= 0xff;
                }
            }
            unpacker.take(&line, &layout, None);
            while let Some((_, b)) = unpacker.pop() {
                assert!(b.iter().all(|&x| x == b[0]));
                got.push(b[0]);
            }
        }
        // Whatever touched the wrecked superframe is gone; nothing else is,
        // and nothing arrives twice or out of order.
        assert!(got.len() >= 26 && got.len() < 30, "{got:?}");
        assert!(got.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(*got.last().unwrap(), 29);
    }
}
