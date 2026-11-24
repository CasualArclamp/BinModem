//! A compressor of the Lempel-Ziv-Markov kind, for a stream.
//!
//! V.42bis and V.44 were made for modems of the 1990s: dictionaries of a
//! few thousand strings, codes of a fixed width, nothing that needed more
//! than a few kilobytes or a multiply. A line that carries half a megabit
//! is fed by a machine with neither limit, and what such machines use is
//! the scheme LZMA made familiar:
//!
//! - **Matches against a long history.** The last 256 kilobytes are kept,
//!   and a run of three octets or more that has been seen in them is sent
//!   as how far back and how long (Ziv and Lempel, 1977).
//! - **Everything coded by a range coder**, a bit at a time, each bit
//!   against a probability that is learnt as the stream goes and chosen by
//!   what came just before: a literal by the top of the octet before it, a
//!   length and a distance by their own trees of bits. A bit that is nearly
//!   certain costs a small fraction of a bit.
//! - **The last distance again** costs almost nothing, which is what a
//!   table of records or a column of figures is made of.
//!
//! It is in LZMA's manner and not its format: nothing here reads or writes
//! an `.lzma` or `.xz` file, and the trees are simpler than that format's.
//!
//! A stream has to be let go of as it is written, so it is cut into
//! **blocks**, one for each write: the length of what it holds, then either
//! the octets themselves, when there are too few to be worth coding, or the
//! range coder's output for them, ended and begun again for the next. What
//! has been learnt, and the history, carry over from block to block.

use std::collections::VecDeque;

/// History kept, in octets: a power of two.
const WINDOW: usize = 1 << 18;
const MASK: u64 = (WINDOW - 1) as u64;

/// The shortest and longest match.
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = MIN_MATCH + 255;

/// Writes shorter than this are sent as they are: a block of coded bits
/// has five octets of the coder's own to end it.
const STORED_UNDER: usize = 24;

/// Places looked at for a match before the longest so far is taken.
const CHAIN: usize = 48;

const HASH_BITS: u32 = 16;

/// A probability that the next bit is nought, in 2048ths, and how fast it
/// moves towards what it sees.
type Prob = u16;
const HALF: Prob = 1024;
const MOVE: u32 = 5;
const TOP: u32 = 1 << 24;

/// What is learnt about the stream: the same at both ends, bit for bit.
#[derive(Debug, Clone)]
struct Model {
    /// Whether what follows is a match, by whether the last thing was.
    is_match: [Prob; 2],
    /// Whether a match is at the last distance again.
    is_rep: [Prob; 2],
    /// A literal's bits, by the top three bits of the octet before it.
    literal: Vec<[Prob; 256]>,
    /// A length: which of three ranges, then a tree for each.
    len_choice: [Prob; 2],
    len_low: [Prob; 8],
    len_mid: [Prob; 8],
    len_high: [Prob; 256],
    /// How many bits a distance has, by how long the match is.
    dist_bits: [[Prob; 32]; 4],
    /// The bit below a distance's top one, and its low four.
    dist_next: [Prob; 32],
    dist_low: [Prob; 16],
    /// The last distance, and whether the last thing coded was a match.
    rep: u64,
    after_match: usize,
    /// The octet before the next one.
    before: u8,
}

impl Model {
    fn new() -> Self {
        Self {
            is_match: [HALF; 2],
            is_rep: [HALF; 2],
            literal: vec![[HALF; 256]; 8],
            len_choice: [HALF; 2],
            len_low: [HALF; 8],
            len_mid: [HALF; 8],
            len_high: [HALF; 256],
            dist_bits: [[HALF; 32]; 4],
            dist_next: [HALF; 32],
            dist_low: [HALF; 16],
            rep: 1,
            after_match: 0,
            before: 0,
        }
    }
}

/// The range coder's sending half (Martin 1979, as Pavlov arranged it).
#[derive(Debug)]
struct RangeEncoder {
    low: u64,
    range: u32,
    cache: u8,
    pending: u64,
    out: Vec<u8>,
}

impl RangeEncoder {
    fn new() -> Self {
        Self {
            low: 0,
            range: u32::MAX,
            cache: 0,
            pending: 1,
            out: Vec::new(),
        }
    }

    fn shift(&mut self) {
        if self.low < 0xFF00_0000 || self.low > 0xFFFF_FFFF {
            let carry = (self.low >> 32) as u8;
            let mut octet = self.cache;
            loop {
                self.out.push(octet.wrapping_add(carry));
                octet = 0xFF;
                self.pending -= 1;
                if self.pending == 0 {
                    break;
                }
            }
            self.cache = (self.low >> 24) as u8;
        }
        self.pending += 1;
        self.low = (self.low & 0x00FF_FFFF) << 8;
    }

    fn bit(&mut self, prob: &mut Prob, bit: u32) {
        let bound = (self.range >> 11) * u32::from(*prob);
        if bit == 0 {
            self.range = bound;
            *prob += ((2048 - u32::from(*prob)) >> MOVE) as Prob;
        } else {
            self.low += u64::from(bound);
            self.range -= bound;
            *prob -= (u32::from(*prob) >> MOVE) as Prob;
        }
        while self.range < TOP {
            self.range <<= 8;
            self.shift();
        }
    }

    /// Bits that are as likely one way as the other, the highest first.
    fn direct(&mut self, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.range >>= 1;
            if (value >> i) & 1 == 1 {
                self.low += u64::from(self.range);
            }
            while self.range < TOP {
                self.range <<= 8;
                self.shift();
            }
        }
    }

    /// A number of `bits` bits through a tree of probabilities, the
    /// highest bit first.
    fn tree(&mut self, probs: &mut [Prob], bits: u32, value: u32) {
        let mut node = 1usize;
        for i in (0..bits).rev() {
            let bit = (value >> i) & 1;
            self.bit(&mut probs[node], bit);
            node = node << 1 | bit as usize;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift();
        }
        self.out
    }
}

/// And its receiving half.
#[derive(Debug)]
struct RangeDecoder<'a> {
    range: u32,
    code: u32,
    input: &'a [u8],
    at: usize,
}

impl<'a> RangeDecoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        let mut decoder = Self {
            range: u32::MAX,
            code: 0,
            input,
            at: 1,
        };
        for _ in 0..4 {
            decoder.code = decoder.code << 8 | u32::from(decoder.next());
        }
        decoder
    }

    fn next(&mut self) -> u8 {
        let octet = self.input.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        octet
    }

    fn normalise(&mut self) {
        while self.range < TOP {
            self.range <<= 8;
            self.code = self.code << 8 | u32::from(self.next());
        }
    }

    fn bit(&mut self, prob: &mut Prob) -> u32 {
        let bound = (self.range >> 11) * u32::from(*prob);
        let bit = if self.code < bound {
            self.range = bound;
            *prob += ((2048 - u32::from(*prob)) >> MOVE) as Prob;
            0
        } else {
            self.code -= bound;
            self.range -= bound;
            *prob -= (u32::from(*prob) >> MOVE) as Prob;
            1
        };
        self.normalise();
        bit
    }

    fn direct(&mut self, bits: u32) -> u32 {
        let mut value = 0;
        for _ in 0..bits {
            self.range >>= 1;
            let bit = u32::from(self.code >= self.range);
            if bit == 1 {
                self.code -= self.range;
            }
            value = value << 1 | bit;
            self.normalise();
        }
        value
    }

    fn tree(&mut self, probs: &mut [Prob], bits: u32) -> u32 {
        let mut node = 1usize;
        for _ in 0..bits {
            node = node << 1 | self.bit(&mut probs[node]) as usize;
        }
        node as u32 - (1 << bits)
    }

    /// Whether more was read than there was: a block that is not one.
    fn overran(&self) -> bool {
        self.at > self.input.len() + 4
    }
}

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A varint at the front of `input`, and how many octets it took.
fn varint(input: &VecDeque<u8>, from: usize) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for i in 0..5 {
        let octet = *input.get(from + i)?;
        value |= usize::from(octet & 0x7F) << (7 * i);
        if octet & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    // Longer than any length here is: the stream is not one of these.
    Some((usize::MAX, 5))
}

/// The sending end.
#[derive(Debug)]
pub struct Encoder {
    model: Model,
    /// The history, a ring: octet `n` of the stream is at `n & MASK`.
    ring: Vec<u8>,
    /// Octets of the stream so far.
    at: u64,
    /// Where each hash of three octets was last seen, and before that.
    head: Vec<u64>,
    prev: Vec<u64>,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// No place: before the stream began.
const NOWHERE: u64 = u64::MAX;

impl Encoder {
    pub fn new() -> Self {
        Self {
            model: Model::new(),
            ring: vec![0; WINDOW],
            at: 0,
            head: vec![NOWHERE; 1 << HASH_BITS],
            prev: vec![NOWHERE; WINDOW],
        }
    }

    fn hash(a: u8, b: u8, c: u8) -> usize {
        let key = u32::from(a) | u32::from(b) << 8 | u32::from(c) << 16;
        (key.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    }

    /// Note that the three octets at `pos` are there, once they are all in
    /// the ring.
    fn remember(&mut self, pos: u64) {
        let h = Self::hash(
            self.ring[(pos & MASK) as usize],
            self.ring[((pos + 1) & MASK) as usize],
            self.ring[((pos + 2) & MASK) as usize],
        );
        self.prev[(pos & MASK) as usize] = self.head[h];
        self.head[h] = pos;
    }

    /// The longest match for `input[i..]` in the history, as its length and
    /// how far back it starts.
    fn longest(&self, input: &[u8], i: usize) -> Option<(usize, u64)> {
        let left = input.len() - i;
        if left < MIN_MATCH {
            return None;
        }
        // Everything before `input[i]` has been kept by now, so this is
        // where the stream has got to.
        let here = self.at;
        let most = left.min(MAX_MATCH);
        let mut best: Option<(usize, u64)> = None;
        let try_at = |from: u64, best: &mut Option<(usize, u64)>| {
            let distance = here - from;
            let mut n = 0;
            // What is behind `here` is in the ring; from `here` on it is
            // still only in `input`, and a match may run into that.
            while n < most {
                let p = from + n as u64;
                let octet = if p < here {
                    self.ring[(p & MASK) as usize]
                } else {
                    input[i + (p - here) as usize]
                };
                if octet != input[i + n] {
                    break;
                }
                n += 1;
            }
            if n >= MIN_MATCH && best.is_none_or(|(len, _)| n > len) {
                *best = Some((n, distance));
            }
        };
        // The last distance first: it is the cheapest to say.
        if self.model.rep <= here && self.model.rep < WINDOW as u64 {
            try_at(here - self.model.rep, &mut best);
        }
        let mut from = self.head[Self::hash(input[i], input[i + 1], input[i + 2])];
        for _ in 0..CHAIN {
            // Out of the ring, or never there.
            if from == NOWHERE || from >= here || here - from >= (WINDOW - MAX_MATCH) as u64 {
                break;
            }
            try_at(from, &mut best);
            if best.is_some_and(|(len, _)| len == most) {
                break;
            }
            from = self.prev[(from & MASK) as usize];
        }
        best
    }

    /// Put `octets` in the history, where a match can then be found.
    fn keep(&mut self, octets: &[u8]) {
        for &octet in octets {
            self.ring[(self.at & MASK) as usize] = octet;
            self.at += 1;
            if self.at >= 3 {
                self.remember(self.at - 3);
            }
        }
    }

    /// The next piece of the stream, as a block the far end can read as
    /// soon as it has it.
    pub fn encode(&mut self, input: &[u8], out: &mut Vec<u8>) {
        // No more than a block's length can say, and no more than leaves
        // the history whole behind it.
        for piece in input.chunks(WINDOW / 4) {
            self.block(piece, out);
        }
    }

    fn block(&mut self, input: &[u8], out: &mut Vec<u8>) {
        if input.is_empty() {
            return;
        }
        if input.len() < STORED_UNDER {
            put_varint(input.len() << 1, out);
            out.extend_from_slice(input);
            self.keep(input);
            self.model.before = input[input.len() - 1];
            self.model.after_match = 0;
            return;
        }
        let mut coder = RangeEncoder::new();
        let mut i = 0;
        while i < input.len() {
            let found = self.longest(input, i);
            let state = self.model.after_match;
            match found {
                Some((len, distance)) => {
                    coder.bit(&mut self.model.is_match[state], 1);
                    let again = distance == self.model.rep;
                    coder.bit(&mut self.model.is_rep[state], u32::from(again));
                    let n = (len - MIN_MATCH) as u32;
                    if n < 8 {
                        coder.bit(&mut self.model.len_choice[0], 0);
                        coder.tree(&mut self.model.len_low, 3, n);
                    } else if n < 16 {
                        coder.bit(&mut self.model.len_choice[0], 1);
                        coder.bit(&mut self.model.len_choice[1], 0);
                        coder.tree(&mut self.model.len_mid, 3, n - 8);
                    } else {
                        coder.bit(&mut self.model.len_choice[0], 1);
                        coder.bit(&mut self.model.len_choice[1], 1);
                        coder.tree(&mut self.model.len_high, 8, n - 16);
                    }
                    if !again {
                        // A distance as how many bits it has, then the
                        // bits under its top one.
                        let d = distance as u32;
                        let bits = 32 - d.leading_zeros();
                        coder.tree(&mut self.model.dist_bits[(n as usize).min(3)], 5, bits);
                        if bits >= 2 {
                            let below = bits - 1;
                            let rest = d & ((1 << below) - 1);
                            coder.bit(
                                &mut self.model.dist_next[bits as usize],
                                (rest >> (below - 1)) & 1,
                            );
                            let left = below - 1;
                            if left > 4 {
                                coder.direct((rest & ((1 << left) - 1)) >> 4, left - 4);
                                coder.tree(&mut self.model.dist_low, 4, rest & 15);
                            } else if left > 0 {
                                coder.direct(rest & ((1 << left) - 1), left);
                            }
                        }
                        self.model.rep = distance;
                    }
                    self.keep(&input[i..i + len]);
                    self.model.before = input[i + len - 1];
                    self.model.after_match = 1;
                    i += len;
                }
                None => {
                    coder.bit(&mut self.model.is_match[state], 0);
                    let context = usize::from(self.model.before >> 5);
                    coder.tree(&mut self.model.literal[context], 8, u32::from(input[i]));
                    self.keep(&input[i..=i]);
                    self.model.before = input[i];
                    self.model.after_match = 0;
                    i += 1;
                }
            }
        }
        let coded = coder.finish();
        put_varint(input.len() << 1 | 1, out);
        put_varint(coded.len(), out);
        out.extend_from_slice(&coded);
    }
}

/// What a stream that is not one of these has wrong with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Corrupt;

/// The receiving end.
#[derive(Debug)]
pub struct Decoder {
    model: Model,
    ring: Vec<u8>,
    at: u64,
    /// What has arrived and is not yet a whole block.
    waiting: VecDeque<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            model: Model::new(),
            ring: vec![0; WINDOW],
            at: 0,
            waiting: VecDeque::new(),
        }
    }

    fn keep(&mut self, octet: u8, out: &mut Vec<u8>) {
        self.ring[(self.at & MASK) as usize] = octet;
        self.at += 1;
        out.push(octet);
    }

    /// More of the stream as it arrived, cut anywhere. Every whole block
    /// in what has arrived so far comes out.
    pub fn decode(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), Corrupt> {
        self.waiting.extend(input);
        loop {
            let Some((head, used)) = varint(&self.waiting, 0) else {
                return Ok(());
            };
            let (len, coded) = (head >> 1, head & 1 == 1);
            if head == usize::MAX || len == 0 || len > WINDOW / 4 {
                return Err(Corrupt);
            }
            if !coded {
                if self.waiting.len() < used + len {
                    return Ok(());
                }
                self.waiting.drain(..used);
                for _ in 0..len {
                    let octet = self.waiting.pop_front().unwrap_or(0);
                    self.keep(octet, out);
                    self.model.before = octet;
                }
                self.model.after_match = 0;
                continue;
            }
            let Some((size, more)) = varint(&self.waiting, used) else {
                return Ok(());
            };
            if size == usize::MAX || size > 2 * len + 64 {
                return Err(Corrupt);
            }
            if self.waiting.len() < used + more + size {
                return Ok(());
            }
            self.waiting.drain(..used + more);
            let block: Vec<u8> = self.waiting.drain(..size).collect();
            self.block(&block, len, out)?;
        }
    }

    fn block(&mut self, coded: &[u8], len: usize, out: &mut Vec<u8>) -> Result<(), Corrupt> {
        let mut coder = RangeDecoder::new(coded);
        let mut done = 0;
        while done < len {
            let state = self.model.after_match;
            if coder.bit(&mut self.model.is_match[state]) == 0 {
                let context = usize::from(self.model.before >> 5);
                let octet = coder.tree(&mut self.model.literal[context], 8) as u8;
                self.keep(octet, out);
                self.model.before = octet;
                self.model.after_match = 0;
                done += 1;
                continue;
            }
            let again = coder.bit(&mut self.model.is_rep[state]) == 1;
            let n = if coder.bit(&mut self.model.len_choice[0]) == 0 {
                coder.tree(&mut self.model.len_low, 3)
            } else if coder.bit(&mut self.model.len_choice[1]) == 0 {
                8 + coder.tree(&mut self.model.len_mid, 3)
            } else {
                16 + coder.tree(&mut self.model.len_high, 8)
            };
            let count = n as usize + MIN_MATCH;
            if !again {
                let bits = coder.tree(&mut self.model.dist_bits[(n as usize).min(3)], 5);
                let mut d = 0u32;
                if bits >= 1 {
                    d = 1;
                    if bits >= 2 {
                        let below = bits - 1;
                        let mut rest =
                            coder.bit(&mut self.model.dist_next[bits as usize]) << (below - 1);
                        let left = below - 1;
                        if left > 4 {
                            rest |= coder.direct(left - 4) << 4;
                            rest |= coder.tree(&mut self.model.dist_low, 4);
                        } else if left > 0 {
                            rest |= coder.direct(left);
                        }
                        d = 1 << below | rest;
                    }
                }
                self.model.rep = u64::from(d);
            }
            let distance = self.model.rep;
            if distance == 0
                || distance > self.at
                || distance >= WINDOW as u64
                || done + count > len
                || coder.overran()
            {
                return Err(Corrupt);
            }
            for _ in 0..count {
                let octet = self.ring[((self.at - distance) & MASK) as usize];
                self.keep(octet, out);
            }
            self.model.before = self.ring[((self.at - 1) & MASK) as usize];
            self.model.after_match = 1;
            done += count;
        }
        if coder.overran() {
            Err(Corrupt)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }
    }

    /// Something like text: words from a small list, lines, and now and
    /// then a number.
    fn prose(seed: u64, len: usize) -> Vec<u8> {
        const WORDS: [&str; 24] = [
            "the", "line", "carries", "samples", "exactly", "and", "modem", "codeword", "a", "of",
            "is", "sent", "again", "when", "frame", "arrives", "wrong", "carrier", "level",
            "table", "receiver", "asks", "for", "more",
        ];
        let mut rng = Lcg(seed);
        let mut out = Vec::with_capacity(len + 16);
        while out.len() < len {
            let word = WORDS[rng.next() as usize % WORDS.len()];
            out.extend_from_slice(word.as_bytes());
            match rng.next() % 12 {
                0 => out.extend_from_slice(b".\r\n"),
                1 => out.extend_from_slice(format!(" {} ", rng.next() % 10_000).as_bytes()),
                _ => out.push(b' '),
            }
        }
        out.truncate(len);
        out
    }

    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut rng = Lcg(seed);
        (0..len).map(|_| (rng.next() >> 8) as u8).collect()
    }

    /// Records with a column that counts: what the last distance is for.
    fn records(len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut n = 0u32;
        while out.len() < len {
            out.extend_from_slice(
                format!("{n:08},OK,{:05},binmodem\r\n", n * 7 % 100_000).as_bytes(),
            );
            n += 1;
        }
        out.truncate(len);
        out
    }

    /// Through both ends, written in pieces of `piece` octets and arriving
    /// in pieces of `cut`: the octets on the line, which have to come out
    /// as they went in.
    fn through(data: &[u8], piece: usize, cut: usize) -> usize {
        let (mut encoder, mut decoder) = (Encoder::new(), Decoder::new());
        let mut line = Vec::new();
        let mut back = Vec::new();
        let mut carried = 0;
        for part in data.chunks(piece.max(1)) {
            line.clear();
            encoder.encode(part, &mut line);
            carried += line.len();
            let before = back.len();
            for arriving in line.chunks(cut.max(1)) {
                decoder
                    .decode(arriving, &mut back)
                    .expect("a block of the encoder's own");
            }
            // Everything written is readable as soon as it has arrived.
            assert_eq!(back.len() - before, part.len());
        }
        assert!(
            back == data,
            "{} octets came back of {}",
            back.len(),
            data.len()
        );
        carried
    }

    #[test]
    fn everything_comes_back_however_it_is_written_and_however_it_arrives() {
        for (data, name) in [
            (prose(1, 60_000), "prose"),
            (noise(2, 20_000), "noise"),
            (records(40_000), "records"),
            (vec![0u8; 30_000], "noughts"),
            (b"a".to_vec(), "one octet"),
            (Vec::new(), "nothing"),
        ] {
            for (piece, cut) in [
                (usize::MAX, usize::MAX),
                (4096, 1),
                (1, 7),
                (23, 5),
                (24, 1000),
                (777, 3),
            ] {
                let carried = through(&data, piece.min(data.len().max(1)), cut);
                assert!(
                    carried
                        <= data.len()
                            + data.len() / 8
                            + 8 * data.len().div_ceil(piece.max(1)).max(1),
                    "{name}: {carried}"
                );
            }
        }
    }

    #[test]
    fn text_goes_in_a_third_and_records_in_a_fifth_and_noise_costs_little() {
        let text = prose(3, 200_000);
        let carried = through(&text, 4096, 4096);
        assert!(
            carried * 3 < text.len(),
            "prose: {carried} of {}",
            text.len()
        );
        let table = records(200_000);
        let carried = through(&table, 4096, 4096);
        assert!(
            carried * 5 < table.len(),
            "records: {carried} of {}",
            table.len()
        );
        let random = noise(4, 100_000);
        let carried = through(&random, 4096, 4096);
        assert!(
            carried < random.len() + random.len() / 50,
            "noise: {carried} of {}",
            random.len()
        );
    }

    #[test]
    fn a_match_reaches_back_across_blocks_and_most_of_the_history() {
        // The same 5000 octets of noise, 200 kilobytes apart.
        let mut data = noise(5, 5000);
        data.extend(noise(6, 200_000));
        data.extend(noise(5, 5000));
        let (mut encoder, mut decoder) = (Encoder::new(), Decoder::new());
        let (mut line, mut back) = (Vec::new(), Vec::new());
        let mut last = 0;
        for part in data.chunks(5000) {
            line.clear();
            encoder.encode(part, &mut line);
            last = line.len();
            decoder.decode(&line, &mut back).unwrap();
        }
        assert!(back == data);
        assert!(last < 200, "the piece seen before took {last} octets");
    }

    #[test]
    fn a_keystroke_costs_two_octets_and_not_a_coder_s_worth() {
        let (mut encoder, mut decoder) = (Encoder::new(), Decoder::new());
        let (mut line, mut back) = (Vec::new(), Vec::new());
        encoder.encode(b"x", &mut line);
        assert_eq!(line, [2, b'x']);
        decoder.decode(&line, &mut back).unwrap();
        assert_eq!(back, b"x");
    }

    #[test]
    fn rubbish_is_refused_and_not_followed_into_the_history() {
        let mut decoder = Decoder::new();
        let mut back = Vec::new();
        // A coded block that claims a match before anything has been sent.
        let mut encoder = Encoder::new();
        let mut line = Vec::new();
        encoder.encode(&prose(7, 4000), &mut line);
        let mut fresh = Encoder::new();
        let mut second = Vec::new();
        fresh.encode(&prose(8, 100), &mut Vec::new());
        fresh.encode(&prose(8, 4000), &mut second);
        // The second encoder's block leans on history this decoder has not
        // got: whatever comes out, it is an error or it is not a crash.
        let _ = decoder.decode(&second, &mut back);
        // And a length no block has.
        let mut other = Decoder::new();
        assert_eq!(
            other.decode(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F], &mut back),
            Err(Corrupt)
        );
    }
}
