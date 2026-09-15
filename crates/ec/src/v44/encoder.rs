//! The V.44 encoder (6.3), and the transfer rules that put its codes on the
//! line (6.6).
//!
//! The loop is four steps and Table 1 is the whole of it. Match the longest
//! string the dictionary holds; send the codeword for its last string-segment;
//! try to carry on matching against the raw history and send how many more
//! characters matched; then learn something from whichever of those failed.
//! Every path through it creates at most one new node, and the character that
//! ended it starts the next match.
//!
//! The part that is easy to get subtly wrong is which character a node's
//! history index points at. A node created by extension points at "the
//! position in the history of the first of the most recent input characters
//! used to extend the string" (6.3.2) -- the new copy, not the old one it was
//! matched against. Both spell the same characters, so a loopback test cannot
//! tell them apart; a real far end can, because the two ends' histories then
//! disagree about where later segments begin.

use crate::bits::BitWriter;

use super::{ALPHABET, Mode, N5, Params, control, length};

/// One string-segment (6.2.1): a run of characters somewhere in the history,
/// and the two links that make the node-tree a tree.
#[derive(Debug, Clone, Copy)]
struct Node {
    /// Where the segment's first character sits in the history.
    at: u32,
    /// How many characters it covers.
    len: u8,
    /// The length of the whole string from the root through this segment.
    ///
    /// Not something the node-tree needs to match with, and kept because the
    /// decoder's side of the dictionary is strings rather than segments: the
    /// note under Table 2 says "strings of length greater than N7R shall not
    /// be created", and an encoder that made a node where the decoder made no
    /// string would be one codeword ahead for the rest of the connection.
    total: u16,
    /// "Points to a node that represents a string-segment that follows this
    /// string-segment."
    down: Option<u16>,
    /// "Points to a node that represents a string-segment at the same level."
    side: Option<u16>,
}

/// A node that 6.3.3 calls for and whose character has not arrived yet.
///
/// Every step of Table 1 creates exactly one string-segment, and two of the
/// three make it out of "the unmatched character" -- the one that ended the
/// match. At the end of what the link has handed over so far there is no such
/// character yet, and the node is owed rather than skipped: the decoder
/// creates its matching string when the next code arrives whatever happens
/// here, so an encoder that quietly dropped one would be a codeword behind for
/// the rest of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owed {
    Nothing,
    /// 6.3.3's first two cases: hang it off this character's root.
    Root(u8),
    /// The third: hang it off the last completely matched segment.
    Under(u16),
}

/// What a match found, if anything.
struct Match {
    /// The node whose codeword to send: the last completely matched segment.
    node: u16,
    /// Characters consumed by the match, counting the first character.
    used: usize,
}

/// The sending half.
pub struct Encoder {
    params: Params,
    mode: Mode,
    /// Every character that has been through the encoder since the dictionary
    /// was last started again (6.2.1 item 3).
    history: Vec<u8>,
    /// One down-index per character of the alphabet (6.2.1 item 1).
    root: [Option<u16>; ALPHABET],
    /// The node-tree. Node `i` carries codeword `N5 + i`.
    nodes: Vec<Node>,
    /// C4, "current position in history": how far along the history the
    /// encoder has got. Everything past it has arrived and not been encoded.
    ///
    /// 6.3 puts input into the history as it arrives -- "input characters are
    /// placed into the next available locations in the history and processed
    /// by the encoder" -- rather than when it is encoded, which is what lets a
    /// node created now point at a character that has not been sent yet.
    at: usize,
    writer: BitWriter,
    /// C2, the current codeword size, and C5, the current ordinal size (7.5.1).
    c2: u32,
    c5: u32,
    /// C3, "threshold for changing the codeword size".
    c3: u32,
    /// 6.6.3: the prefix an ordinal or an extension takes depends on whether a
    /// codeword went immediately before.
    after_codeword: bool,
    /// A string-segment owed to a character that has not arrived.
    owed: Owed,
    /// 7.14's ESCAPE, used only in transparent mode.
    escape: u8,
    /// What the compressibility test is measuring (7.11.5).
    chars_in: u32,
    bits_out: u32,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("mode", &self.mode)
            .field("c1", &self.c1())
            .field("c2", &self.c2)
            .field("history", &self.history.len())
            .finish_non_exhaustive()
    }
}

/// How often compressibility is reconsidered, in characters.
///
/// 7.11.5 requires the test and does not specify it: "the nature of the test is
/// not specified in this Recommendation". The window and the threshold are
/// therefore ours.
const TEST_WINDOW: u32 = 512;
/// Leave compressed mode when the codes cost this fraction of the characters.
const GIVE_UP_PERCENT: u32 = 100;

impl Encoder {
    pub fn new(params: Params) -> Self {
        let mut me = Self {
            params,
            mode: Mode::Compressed,
            history: Vec::new(),
            root: [None; ALPHABET],
            nodes: Vec::new(),
            at: 0,
            writer: BitWriter::new(),
            c2: 6,
            c5: 7,
            c3: 64,
            after_codeword: false,
            owed: Owed::Nothing,
            escape: 0,
            chars_in: 0,
            bits_out: 0,
        };
        me.reinitialize();
        me
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }


    /// C1, "codeword value of next available entry of node-tree".
    fn c1(&self) -> u16 {
        N5 + self.nodes.len() as u16
    }

    /// 7.5.1: the state a dictionary starts in, and returns to.
    fn reinitialize(&mut self) {
        // 7.11.4: "the next input character shall be placed in the first
        // position of the history". What has arrived and not been encoded has
        // not reached the far end either, so it moves to the front and the far
        // end will build the same positions for it from nothing.
        self.history.drain(..self.at);
        self.at = 0;
        self.root = [None; ALPHABET];
        self.nodes.clear();
        self.c2 = 6;
        self.c3 = 64;
        self.c5 = 7;
        self.after_codeword = false;
        self.owed = Owed::Nothing;
    }

    /// Hand characters to the encoder.
    pub fn encode(&mut self, input: &[u8], out: &mut Vec<u8>) {
        self.history.extend_from_slice(input);
        self.chars_in += input.len() as u32;
        self.run(false, out);
    }

    /// Characters that have arrived and not been encoded.
    fn waiting(&self) -> usize {
        self.history.len() - self.at
    }

    /// 7.13: end whatever is part-done, put it on the line, and align.
    pub fn flush(&mut self, out: &mut Vec<u8>) {
        self.run(true, out);
        if self.mode == Mode::Compressed && self.writer.pending() > 0 {
            self.put_control(control::FLUSH, out);
            self.writer.align(out);
        }
    }

    /// Work through what is pending. When `all` is set nothing is held back
    /// waiting for more characters, which is what a flush means.
    fn run(&mut self, all: bool, out: &mut Vec<u8>) {
        loop {
            if self.mode == Mode::Transparent {
                // 6.5: "the characters input to the encoder are transferred
                // without modification, octet-aligned".
                self.transparently(out);
                self.consider_mode(out);
                if self.mode == Mode::Transparent {
                    return;
                }
                continue;
            }
            if self.waiting() == 0 || (!all && !self.enough()) {
                return;
            }
            self.settle_owed();
            self.step(out);
            self.consider_mode(out);
        }
    }

    /// Everything waiting, as itself (6.5), with 7.14's ESCAPE handling.
    fn transparently(&mut self, out: &mut Vec<u8>) {
        while self.at < self.history.len() {
            let c = self.history[self.at];
            self.at += 1;
            out.push(c);
            if c == self.escape {
                // "The detected ESCAPE shall be transferred and followed
                // immediately by the EID command. The current value of the
                // ESCAPE is then changed by adding to it 51, the addition
                // performed modulo 256."
                out.push(super::command::EID);
                self.escape = self.escape.wrapping_add(51);
            }
        }
    }

    /// Whether there is enough in hand to be sure a match has ended.
    ///
    /// A match can always be extended by another character, so with a few
    /// characters in hand and more coming the encoder cannot yet know what it
    /// is looking at. The bound is the longest string there could be.
    fn enough(&self) -> bool {
        self.waiting() > usize::from(self.params.n7)
    }

    /// Make the node the last step could not, now that its character is here.
    fn settle_owed(&mut self) {
        let at = self.at as u32;
        match std::mem::replace(&mut self.owed, Owed::Nothing) {
            Owed::Nothing => {}
            Owed::Root(first) => self.append_to_root(first, at),
            Owed::Under(parent) => self.add_child(parent, at, 1),
        }
    }

    /// One pass of Table 1.
    fn step(&mut self, out: &mut Vec<u8>) {
        // 7.11.3 and 7.11.4: no room for another codeword, or the history has
        // reached its end. Either way the dictionary starts again.
        if self.at >= usize::from(self.params.n8) || !self.room() {
            self.restart(out);
        }
        // Whatever was owed belongs to the dictionary that has just gone.
        if self.at >= self.history.len() {
            return;
        }
        let first = self.history[self.at];
        match self.find() {
            // "String-Matching Failure": send the ordinal, and learn the
            // character that follows it.
            None => {
                self.at += 1;
                self.put_ordinal(first, out);
                // 6.3.3: "a 1-character string-segment is created using the
                // unmatched character, branching from the root of the first
                // character". Owed until that character is here.
                self.owed = Owed::Root(first);
            }
            Some(found) => {
                self.at += found.used;
                self.put_codeword(N5 + found.node, out);
                let room = usize::from(self.params.n7).saturating_sub(found.used);
                let extension = self.extension(&found, room);
                if extension > 0 {
                    // 6.3.2: the index is "the position in the history of the
                    // first of the most recent input characters used to extend
                    // the string" -- the copy just matched, not the older one
                    // it was matched against. Both spell the same characters,
                    // so only a real far end can tell them apart.
                    let at = self.at as u32;
                    self.at += extension;
                    self.put_extension(extension as u16, out);
                    self.add_child(found.node, at, extension as u8);
                } else {
                    // "String-Extension Failure": nothing is sent, and the
                    // unmatched character is appended to the matched segment.
                    self.owed = Owed::Under(found.node);
                }
            }
        }
    }

    /// How far the match can be carried on against the history (6.3.2).
    ///
    /// NOTE 1 offers a shortcut -- a partially matched sibling has already
    /// done these comparisons -- and it is deliberately not taken. The literal
    /// rule is what the decoder does on its side (6.4.1 item 5: "the
    /// characters in the history immediately following the string represented
    /// by that codeword"), so doing the same thing here makes the two provably
    /// agree rather than agreeing by an argument in a note.
    fn extension(&self, found: &Match, room: usize) -> usize {
        let node = self.nodes[usize::from(found.node)];
        let after = node.at as usize + usize::from(node.len);
        let mut n = 0usize;
        // The two runs may overlap, and that is the point: a string of one
        // character repeated is matched against itself one place along, which
        // is how a long run costs a codeword and a length. The decoder copies
        // the same way, a character at a time, so it produces what this
        // matched even where the source runs into the destination.
        while n < room
            && self.at + n < self.history.len()
            && after + n < self.history.len()
            && self.history[after + n] == self.history[self.at + n]
        {
            n += 1;
        }
        n
    }

    /// The longest string match starting at the first pending character
    /// (6.3.1), or nothing if the match fails at the top.
    fn find(&self) -> Option<Match> {
        let first = self.history[self.at];
        let mut level = self.root[usize::from(first)]?;
        let mut used = 1usize;
        let mut last: Option<u16> = None;
        loop {
            let mut best: Option<u16> = None;
            let mut node = Some(level);
            while let Some(index) = node {
                let n = self.nodes[usize::from(index)];
                let seg = &self.history[n.at as usize..n.at as usize + usize::from(n.len)];
                let have = &self.history[(self.at + used).min(self.history.len())..];
                let same = seg.iter().zip(have).take_while(|(a, b)| a == b).count();
                if same == seg.len() && same > 0 {
                    best = Some(index);
                    break;
                }
                node = n.side;
            }
            match best {
                Some(index) => {
                    let n = self.nodes[usize::from(index)];
                    used += usize::from(n.len);
                    last = Some(index);
                    match n.down {
                        Some(next) if self.at + used < self.history.len() => level = next,
                        _ => break,
                    }
                }
                None => break,
            }
        }
        // 6.3.1: the match fails if "no second-level node matches the second
        // character", which is a match that never got past the root.
        last.map(|node| Match { node, used })
    }

    /// Whether the node-tree has room for another string-segment.
    fn room(&self) -> bool {
        self.c1() < self.params.n2
    }

    fn new_node(&mut self, at: u32, len: u8, total: u16) -> Option<u16> {
        if !self.room() || total > u16::from(self.params.n7) {
            return None;
        }
        self.nodes.push(Node { at, len, down: None, side: None, total });
        // The index into the node-tree, which is the codeword less N5. The two
        // are a constant apart and telling them apart matters: the links below
        // are indices, and what goes on the line is the codeword.
        Some((self.nodes.len() - 1) as u16)
    }

    /// Hang a one-character segment under a root (6.3.3, "Append").
    fn append_to_root(&mut self, first: u8, at: u32) {
        // The root character and the one segment character.
        let Some(node) = self.new_node(at, 1, 2) else { return };
        let slot = &mut self.root[usize::from(first)];
        match *slot {
            None => *slot = Some(node),
            Some(head) => self.link_side(head, node),
        }
    }

    /// Hang the extension under the matched segment (6.3.3, "Extend").
    fn add_child(&mut self, parent: u16, at: u32, len: u8) {
        let total = self.nodes[usize::from(parent)].total + u16::from(len);
        let Some(node) = self.new_node(at, len, total) else { return };
        self.link_down(parent, node);
    }

    fn link_down(&mut self, parent: u16, node: u16) {
        match self.nodes[usize::from(parent)].down {
            None => self.nodes[usize::from(parent)].down = Some(node),
            Some(head) => self.link_side(head, node),
        }
    }

    fn link_side(&mut self, head: u16, node: u16) {
        let mut at = head;
        while let Some(next) = self.nodes[usize::from(at)].side {
            at = next;
        }
        self.nodes[usize::from(at)].side = Some(node);
    }

    // -- transfer (6.6) -----------------------------------------------------

    fn write(&mut self, value: u16, bits: u32, out: &mut Vec<u8>) {
        self.writer.write(value, bits, out);
        self.bits_out += bits;
    }

    /// The same, for bits that say nothing about whether the data compresses.
    ///
    /// Control codes are not measured. A FLUSH goes out every time the layer
    /// above hands over a few characters, and counting those would make the
    /// compressibility test a measure of how often the link flushes rather
    /// than of the data -- which is what made a terminal typing one character
    /// at a time look incompressible and fall out of compressed mode.
    fn write_control(&mut self, value: u16, bits: u32, out: &mut Vec<u8>) {
        self.writer.write(value, bits, out);
    }

    /// 6.6.3: a control code always takes the prefix "1".
    fn put_control(&mut self, code: u16, out: &mut Vec<u8>) {
        self.write_control(1, 1, out);
        self.write_control(code, self.c2, out);
        self.after_codeword = false;
    }

    /// 7.11.1: ordinals start at seven bits and step up once, when one above
    /// 127 has to go.
    fn put_ordinal(&mut self, value: u8, out: &mut Vec<u8>) {
        if value > 127 && self.c5 == 7 {
            self.put_control(control::STEPUP, out);
            self.c5 = 8;
        }
        // "0" on its own, or "0" "0" when a codeword went immediately before.
        self.write(0, if self.after_codeword { 2 } else { 1 }, out);
        self.write(u16::from(value), self.c5, out);
        self.after_codeword = false;
    }

    /// 7.11.2: widen the codeword until the value fits, announcing each step.
    fn put_codeword(&mut self, code: u16, out: &mut Vec<u8>) {
        while u32::from(code) >= self.c3 && self.c2 < self.params.max_code_bits() {
            self.put_control(control::STEPUP, out);
            self.c2 += 1;
            self.c3 *= 2;
        }
        self.write(1, 1, out);
        self.write(code, self.c2, out);
        self.after_codeword = true;
    }

    /// An extension only ever follows a codeword, so its prefix is "0" "1".
    fn put_extension(&mut self, len: u16, out: &mut Vec<u8>) {
        self.write(0, 1, out);
        self.write(1, 1, out);
        let n7 = self.params.n7;
        length::write(len, n7, &mut self.writer, out);
        self.after_codeword = false;
    }

    /// 7.12: start the dictionary again, telling the far end if it is
    /// listening in compressed mode.
    fn restart(&mut self, out: &mut Vec<u8>) {
        if self.mode == Mode::Compressed {
            self.put_control(control::REINIT, out);
        }
        self.reinitialize();
    }

    /// 7.11.5's test, whose nature "is not specified in this Recommendation".
    fn consider_mode(&mut self, out: &mut Vec<u8>) {
        if self.chars_in < TEST_WINDOW {
            return;
        }
        let plain = self.chars_in * 8;
        match self.mode {
            Mode::Compressed if self.bits_out * 100 >= plain * GIVE_UP_PERCENT => {
                self.enter_transparent(out)
            }
            // 7.11.5 has the monitoring continue in transparent mode, and
            // leaves what it consists of open. Rather than keep a second
            // encoder running to find out, this gives compression another go
            // every window: the dictionary starts again either way (6.5.2), so
            // a fresh attempt costs one ECM and the next window decides.
            Mode::Transparent => self.enter_compressed(out),
            Mode::Compressed => {}
        }
        self.chars_in = 0;
        self.bits_out = 0;
    }

    /// 6.5.1, on demand rather than because the test in 7.11.5 said so.
    ///
    /// The compressibility test is ours and deliberately not specified, so a
    /// test that wants to watch the transition has to be able to ask for it.
    pub fn enter_transparent_now(&mut self, out: &mut Vec<u8>) {
        self.run(true, out);
        if self.mode == Mode::Compressed {
            self.enter_transparent(out);
        }
    }

    /// 6.5.1: finish what is in hand, say so, and align.
    fn enter_transparent(&mut self, out: &mut Vec<u8>) {
        self.put_control(control::ETM, out);
        self.writer.align(out);
        self.mode = Mode::Transparent;
    }

    /// 6.5.2: say so, and start the dictionary again on the way in.
    ///
    /// The order matters and the clause gives it: reinitialise, then the
    /// ESCAPE, then ECM. A far end that reinitialised at a different moment
    /// would build a different dictionary from the same characters.
    pub fn enter_compressed(&mut self, out: &mut Vec<u8>) {
        if self.mode == Mode::Compressed {
            return;
        }
        self.reinitialize();
        out.push(self.escape);
        out.push(super::command::ECM);
        self.mode = Mode::Compressed;
        self.escape = 0;
    }
}
