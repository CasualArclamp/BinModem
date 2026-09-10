//! Modified Huffman coding, T.4 clause 4.1.
//!
//! A fax page is a run-length code and nothing more clever than that. Each
//! line is a sequence of alternating white and black runs, starting with
//! white, and each run length is one code word out of two tables: a
//! terminating code for a length under 64, and a make-up code for the
//! multiple of 64 below it followed by a terminating code for the remainder.
//! The two colours have their own tables, because the statistics of a page of
//! text are not symmetric -- black runs are short and common, so 2 black is
//! two bits and 2 white is four.
//!
//! Every table here is Table 2, 3a and 3b of T.4, transcribed. Nothing is
//! derived: Huffman codes have no structure to derive them from, which is the
//! point of them, and a table that is nearly right produces a page that is
//! nearly a page.

/// One code word: the bits, most significant first, and how many there are.
///
/// Thirteen bits is the longest in any of the tables (black 512's make-up),
/// so a `u16` holds anything here with room to spare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code {
    pub bits: u16,
    pub len: u8,
}

const fn c(bits: u16, len: u8) -> Code {
    Code { bits, len }
}

/// End of line: eleven zeros and a one (4.1.2).
///
/// It cannot occur inside any other code word or any concatenation of them,
/// which is what lets a receiver find the start of a line again after an
/// error rather than losing the rest of the page.
pub const EOL: Code = c(0b0000_0000_0001, 12);

/// Table 2/T.4, white, runs of 0 to 63.
pub const WHITE_TERMINATING: [Code; 64] = [
    c(0b00110101, 8), c(0b000111, 6), c(0b0111, 4), c(0b1000, 4),
    c(0b1011, 4), c(0b1100, 4), c(0b1110, 4), c(0b1111, 4),
    c(0b10011, 5), c(0b10100, 5), c(0b00111, 5), c(0b01000, 5),
    c(0b001000, 6), c(0b000011, 6), c(0b110100, 6), c(0b110101, 6),
    c(0b101010, 6), c(0b101011, 6), c(0b0100111, 7), c(0b0001100, 7),
    c(0b0001000, 7), c(0b0010111, 7), c(0b0000011, 7), c(0b0000100, 7),
    c(0b0101000, 7), c(0b0101011, 7), c(0b0010011, 7), c(0b0100100, 7),
    c(0b0011000, 7), c(0b00000010, 8), c(0b00000011, 8), c(0b00011010, 8),
    c(0b00011011, 8), c(0b00010010, 8), c(0b00010011, 8), c(0b00010100, 8),
    c(0b00010101, 8), c(0b00010110, 8), c(0b00010111, 8), c(0b00101000, 8),
    c(0b00101001, 8), c(0b00101010, 8), c(0b00101011, 8), c(0b00101100, 8),
    c(0b00101101, 8), c(0b00000100, 8), c(0b00000101, 8), c(0b00001010, 8),
    c(0b00001011, 8), c(0b01010010, 8), c(0b01010011, 8), c(0b01010100, 8),
    c(0b01010101, 8), c(0b00100100, 8), c(0b00100101, 8), c(0b01011000, 8),
    c(0b01011001, 8), c(0b01011010, 8), c(0b01011011, 8), c(0b01001010, 8),
    c(0b01001011, 8), c(0b00110010, 8), c(0b00110011, 8), c(0b00110100, 8),
];

/// Table 2/T.4, black, runs of 0 to 63.
pub const BLACK_TERMINATING: [Code; 64] = [
    c(0b0000110111, 10), c(0b010, 3), c(0b11, 2), c(0b10, 2),
    c(0b011, 3), c(0b0011, 4), c(0b0010, 4), c(0b00011, 5),
    c(0b000101, 6), c(0b000100, 6), c(0b0000100, 7), c(0b0000101, 7),
    c(0b0000111, 7), c(0b00000100, 8), c(0b00000111, 8), c(0b000011000, 9),
    c(0b0000010111, 10), c(0b0000011000, 10), c(0b0000001000, 10),
    c(0b00001100111, 11), c(0b00001101000, 11), c(0b00001101100, 11),
    c(0b00000110111, 11), c(0b00000101000, 11), c(0b00000010111, 11),
    c(0b00000011000, 11), c(0b000011001010, 12), c(0b000011001011, 12),
    c(0b000011001100, 12), c(0b000011001101, 12), c(0b000001101000, 12),
    c(0b000001101001, 12), c(0b000001101010, 12), c(0b000001101011, 12),
    c(0b000011010010, 12), c(0b000011010011, 12), c(0b000011010100, 12),
    c(0b000011010101, 12), c(0b000011010110, 12), c(0b000011010111, 12),
    c(0b000001101100, 12), c(0b000001101101, 12), c(0b000011011010, 12),
    c(0b000011011011, 12), c(0b000001010100, 12), c(0b000001010101, 12),
    c(0b000001010110, 12), c(0b000001010111, 12), c(0b000001100100, 12),
    c(0b000001100101, 12), c(0b000001010010, 12), c(0b000001010011, 12),
    c(0b000000100100, 12), c(0b000000110111, 12), c(0b000000111000, 12),
    c(0b000000100111, 12), c(0b000000101000, 12), c(0b000001011000, 12),
    c(0b000001011001, 12), c(0b000000101011, 12), c(0b000000101100, 12),
    c(0b000001011010, 12), c(0b000001100110, 12), c(0b000001100111, 12),
];

/// Table 3a/T.4, white, runs of 64 to 1728 in steps of 64.
pub const WHITE_MAKEUP: [Code; 27] = [
    c(0b11011, 5), c(0b10010, 5), c(0b010111, 6), c(0b0110111, 7),
    c(0b00110110, 8), c(0b00110111, 8), c(0b01100100, 8), c(0b01100101, 8),
    c(0b01101000, 8), c(0b01100111, 8), c(0b011001100, 9), c(0b011001101, 9),
    c(0b011010010, 9), c(0b011010011, 9), c(0b011010100, 9), c(0b011010101, 9),
    c(0b011010110, 9), c(0b011010111, 9), c(0b011011000, 9), c(0b011011001, 9),
    c(0b011011010, 9), c(0b011011011, 9), c(0b010011000, 9), c(0b010011001, 9),
    c(0b010011010, 9), c(0b011000, 6), c(0b010011011, 9),
];

/// Table 3a/T.4, black, runs of 64 to 1728 in steps of 64.
pub const BLACK_MAKEUP: [Code; 27] = [
    c(0b0000001111, 10), c(0b000011001000, 12), c(0b000011001001, 12),
    c(0b000001011011, 12), c(0b000000110011, 12), c(0b000000110100, 12),
    c(0b000000110101, 12), c(0b0000001101100, 13), c(0b0000001101101, 13),
    c(0b0000001001010, 13), c(0b0000001001011, 13), c(0b0000001001100, 13),
    c(0b0000001001101, 13), c(0b0000001110010, 13), c(0b0000001110011, 13),
    c(0b0000001110100, 13), c(0b0000001110101, 13), c(0b0000001110110, 13),
    c(0b0000001110111, 13), c(0b0000001010010, 13), c(0b0000001010011, 13),
    c(0b0000001010100, 13), c(0b0000001010101, 13), c(0b0000001011010, 13),
    c(0b0000001011011, 13), c(0b0000001100100, 13), c(0b0000001100101, 13),
];

/// Table 3b/T.4: 1792 to 2560, the same codes for both colours.
///
/// Added for the wider papers of the note under Table 3a, and reached here
/// only by a page wider than 1728 pels.
pub const EXTENDED_MAKEUP: [Code; 13] = [
    c(0b00000001000, 11), c(0b00000001100, 11), c(0b00000001101, 11),
    c(0b000000010010, 12), c(0b000000010011, 12), c(0b000000010100, 12),
    c(0b000000010101, 12), c(0b000000010110, 12), c(0b000000010111, 12),
    c(0b000000011100, 12), c(0b000000011101, 12), c(0b000000011110, 12),
    c(0b000000011111, 12),
];

/// Whether a run is of the paper or of the ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    White,
    Black,
}

/// Bits going out, most significant first.
///
/// A fax is a bit stream and not a byte stream: code words are 2 to 13 bits
/// long and nothing lines up. What eventually goes on the line is whatever
/// number of bits there are, padded at the very end.
#[derive(Debug, Default, Clone)]
pub struct Bits {
    bytes: Vec<u8>,
    /// Bits used in the last byte, 0 to 7.
    spare: u8,
}

impl Bits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.bytes.len() * 8 - usize::from(self.spare)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, bit: bool) {
        if self.spare == 0 {
            self.bytes.push(0);
            self.spare = 8;
        }
        self.spare -= 1;
        if bit {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 1 << self.spare;
        }
    }

    pub fn push_code(&mut self, code: Code) {
        for i in (0..code.len).rev() {
            self.push(code.bits >> i & 1 == 1);
        }
    }

    /// The bits so far, packed into octets with the last one padded with
    /// zeros.
    ///
    /// Zeros rather than ones, because a fill of zeros before an EOL is what
    /// 4.1.2 already allows and a receiver has to tolerate; a tail of ones
    /// would be a run of 1728 black at the end of a page nobody asked for.
    pub fn octets(&self) -> &[u8] {
        &self.bytes
    }

    /// How many bits of the last octet are padding.
    pub fn padding(&self) -> u8 {
        self.spare
    }

    /// Every bit in order, without the padding of the last octet.
    ///
    /// What goes on a line is a bit stream, not a byte stream. The octets
    /// exist for writing a page to a file, and the padding at the end of the
    /// last one would be a run of white nobody asked for.
    pub fn to_bits(&self) -> Vec<bool> {
        let mut out = Vec::with_capacity(self.len());
        for (i, &byte) in self.bytes.iter().enumerate() {
            let last = i + 1 == self.bytes.len();
            let count = if last { 8 - self.spare } else { 8 };
            for b in 0..count {
                out.push(byte >> (7 - b) & 1 != 0);
            }
        }
        out
    }
}

/// Write one run length as make-up plus terminating codes (4.1.1).
///
/// The note under Table 3b: a run of 2624 or more is coded by as many 2560
/// make-up codes as it takes to bring the remainder under 2560, and then by
/// the ordinary make-up and terminating pair.
pub fn write_run(out: &mut Bits, colour: Colour, mut run: u32) {
    let (terminating, makeup): (&[Code; 64], &[Code; 27]) = match colour {
        Colour::White => (&WHITE_TERMINATING, &WHITE_MAKEUP),
        Colour::Black => (&BLACK_TERMINATING, &BLACK_MAKEUP),
    };
    while run >= 2624 {
        out.push_code(EXTENDED_MAKEUP[12]);
        run -= 2560;
    }
    if run >= 1792 {
        let step = ((run - 1792) / 64) as usize;
        out.push_code(EXTENDED_MAKEUP[step]);
        run -= 1792 + 64 * step as u32;
    } else if run >= 64 {
        let step = (run / 64) as usize;
        out.push_code(makeup[step - 1]);
        run -= 64 * step as u32;
    }
    out.push_code(terminating[run as usize]);
}

/// The runs in one scan line, starting with white.
///
/// A line that begins with a black pel begins with a white run of zero, which
/// is a real code word and not an omission: the decoder alternates colours
/// unconditionally, so leaving it out would paint the whole line the wrong
/// way round.
pub fn runs(line: &[bool]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut colour = false;
    let mut run = 0u32;
    for &pel in line {
        if pel == colour {
            run += 1;
        } else {
            out.push(run);
            colour = pel;
            run = 1;
        }
    }
    out.push(run);
    out
}

/// Code one scan line, EOL first.
pub fn write_line(out: &mut Bits, line: &[bool]) {
    out.push_code(EOL);
    let mut colour = Colour::White;
    for run in runs(line) {
        write_run(out, colour, run);
        colour = match colour {
            Colour::White => Colour::Black,
            Colour::Black => Colour::White,
        };
    }
}

/// Return to control: six EOLs, which end phase C (4.1.2 and Figure 2).
pub fn write_rtc(out: &mut Bits) {
    for _ in 0..6 {
        out.push_code(EOL);
    }
}

/// Code a whole page: every line, then RTC.
///
/// `line` is one row of the page, one `bool` per pel, true for black.
pub fn encode(lines: &[Vec<bool>]) -> Bits {
    encode_padded(lines, 0)
}

/// Code a page, stretching every line to at least `min_bits`.
///
/// The minimum scan line time of T.30 Table 2, bits 21 to 23, is not a
/// property of the picture but of the paper going through the far end: a
/// thermal head can only print so fast, and a line that arrives sooner than
/// it can be printed is a line lost. 4.1.2 allows fill for exactly this, as
/// zeros between the end of one line and the EOL that starts the next, which
/// is where they go here.
///
/// The fill is counted per line including its own EOL, which is what the
/// receiver's clock sees.
pub fn encode_padded(lines: &[Vec<bool>], min_bits: usize) -> Bits {
    let mut out = Bits::new();
    for line in lines {
        let before = out.len();
        write_line(&mut out, line);
        for _ in out.len() - before..min_bits {
            out.push(false);
        }
    }
    write_rtc(&mut out);
    out
}

/// Six EOLs in a row: the return to control that ends a page (4.1.2).
pub const RTC_EOLS: u32 = 6;

/// The longest code word in any of the tables, in bits.
const LONGEST: u8 = 13;

/// The fewest zeros in a row that can only be an EOL.
///
/// The end-of-line code is eleven zeros and a one, and no other code word in
/// any table has eleven leading zeros. That is the whole reason 4.1.2 chose
/// it: fill may be added in front of it, and a receiver that has lost its
/// place can find the next line by counting zeros.
const EOL_ZEROS: usize = 11;

/// What a code word turned out to mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// A terminating code: this many pels, and then the other colour.
    Run(u32),
    /// A make-up code: this many pels, and another code word to come.
    MakeUp(u32),
}

fn table(colour: Colour) -> std::collections::HashMap<(u8, u16), Word> {
    let (terminating, makeup): (&[Code; 64], &[Code; 27]) = match colour {
        Colour::White => (&WHITE_TERMINATING, &WHITE_MAKEUP),
        Colour::Black => (&BLACK_TERMINATING, &BLACK_MAKEUP),
    };
    let mut map = std::collections::HashMap::new();
    for (i, code) in terminating.iter().enumerate() {
        map.insert((code.len, code.bits), Word::Run(i as u32));
    }
    for (i, code) in makeup.iter().enumerate() {
        map.insert((code.len, code.bits), Word::MakeUp(64 * (i as u32 + 1)));
    }
    for (i, code) in EXTENDED_MAKEUP.iter().enumerate() {
        map.insert((code.len, code.bits), Word::MakeUp(1792 + 64 * i as u32));
    }
    map
}

/// Modified Huffman going the other way: bits in, scan lines out.
///
/// It starts out looking for an end-of-line code and throwing away everything
/// else, which is not a special case but the ordinary way a page is found.
/// What arrives before the first line is the tail of a training sequence and
/// whatever the descrambler made of it, and 4.1.2 puts an EOL in front of the
/// first line precisely so that a receiver can start there.
#[derive(Debug)]
pub struct Decoder {
    white: std::collections::HashMap<(u8, u16), Word>,
    black: std::collections::HashMap<(u8, u16), Word>,
    /// Bits not yet decoded, oldest first.
    bits: std::collections::VecDeque<bool>,
    colour: Colour,
    /// Make-up length waiting for the terminating code that completes it.
    carried: u32,
    line: Vec<bool>,
    lines: Vec<Vec<bool>>,
    /// EOLs seen in a row, with no line between them.
    eols: u32,
    /// Whether everything up to the next EOL is being thrown away.
    hunting: bool,
    done: bool,
    damaged: usize,
    width: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self::with_width(crate::page::WIDTH)
    }

    pub fn with_width(width: usize) -> Self {
        Self {
            white: table(Colour::White),
            black: table(Colour::Black),
            bits: std::collections::VecDeque::new(),
            colour: Colour::White,
            carried: 0,
            line: Vec::new(),
            lines: Vec::new(),
            eols: 0,
            hunting: true,
            done: false,
            damaged: 0,
            width,
        }
    }

    /// The lines decoded so far.
    pub fn lines(&self) -> &[Vec<bool>] {
        &self.lines
    }

    /// Whether the return to control has arrived and the page is complete.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Lines that did not come out the width they should have.
    ///
    /// What T.30 6.3.2 wants counted: a receiver totals its bad lines and
    /// decides from that whether to accept the page or ask for it again.
    pub fn damaged(&self) -> usize {
        self.damaged
    }

    /// Everything decoded, as a page.
    pub fn page(&self, resolution: crate::page::Resolution) -> crate::page::Page {
        crate::page::Page {
            lines: self.lines.clone(),
            resolution,
        }
    }

    /// Start again for the next page, keeping nothing.
    pub fn reset(&mut self) {
        let width = self.width;
        *self = Self::with_width(width);
    }

    pub fn feed(&mut self, bit: bool) {
        if self.done {
            return;
        }
        self.bits.push_back(bit);
        while self.step() {}
    }

    pub fn feed_bits(&mut self, bits: &[bool]) {
        for &bit in bits {
            self.feed(bit);
        }
    }

    /// Take one code word off the front. Returns whether anything was taken.
    fn step(&mut self) -> bool {
        if self.done {
            self.bits.clear();
            return false;
        }
        // An EOL first, always: it outranks every code word, and while hunting
        // it is the only thing being looked for.
        let zeros = self.bits.iter().take_while(|b| !**b).count();
        if zeros >= EOL_ZEROS {
            if self.bits.len() > zeros {
                self.bits.drain(..zeros + 1);
                self.end_of_line();
                return true;
            }
            // Nothing but zeros so far. Fill can be any length, so keep only
            // as many as it takes to still recognise the EOL when the one
            // finally arrives.
            if zeros > EOL_ZEROS {
                self.bits.drain(..zeros - EOL_ZEROS);
            }
            return false;
        }
        if self.hunting {
            // Everything with eleven zeros in front of it went to the branch
            // above, so what is here either starts with a one inside the
            // first eleven bits or is all zeros and too short to tell yet.
            // Waiting on the second is the whole point: dropping a bit at a
            // time throws away the leading zeros of the very code being
            // looked for, and the hunt then never ends.
            if zeros == self.bits.len() {
                return false;
            }
            // No end of line can begin at any of these, so all of them go.
            self.bits.drain(..zeros + 1);
            return true;
        }

        let table = match self.colour {
            Colour::White => &self.white,
            Colour::Black => &self.black,
        };
        let mut value: u16 = 0;
        for len in 1..=LONGEST {
            let Some(&bit) = self.bits.get(usize::from(len) - 1) else {
                // Not enough bits yet to rule the rest of the table out.
                return false;
            };
            value = value << 1 | u16::from(bit);
            if let Some(&word) = table.get(&(len, value)) {
                self.bits.drain(..usize::from(len));
                self.apply(word);
                return true;
            }
        }
        // Thirteen bits that are no code word at all. The line is spoiled, and
        // the answer is to find the next EOL and carry on from there, which
        // loses one line rather than the rest of the page.
        self.spoil();
        true
    }

    fn apply(&mut self, word: Word) {
        match word {
            Word::MakeUp(n) => self.carried += n,
            Word::Run(n) => {
                let total = self.carried + n;
                self.carried = 0;
                let ink = self.colour == Colour::Black;
                // A line longer than the paper is a decoding error rather than
                // a wider page: stop growing it, and let the width check at
                // the end of the line notice.
                let room = (self.width * 2).saturating_sub(self.line.len());
                let count = (total as usize).min(room);
                self.line.extend(std::iter::repeat_n(ink, count));
                self.colour = match self.colour {
                    Colour::White => Colour::Black,
                    Colour::Black => Colour::White,
                };
            }
        }
    }

    /// Give up on the line in hand and look for the next EOL.
    fn spoil(&mut self) {
        self.damaged += 1;
        self.hunting = true;
        self.line.clear();
        self.carried = 0;
        self.colour = Colour::White;
        self.bits.pop_front();
    }

    fn end_of_line(&mut self) {
        if self.line.is_empty() {
            self.eols += 1;
        } else {
            self.eols = 1;
            let mut line = std::mem::take(&mut self.line);
            if line.len() != self.width {
                self.damaged += 1;
            }
            line.resize(self.width, false);
            self.lines.push(line);
        }
        self.colour = Colour::White;
        self.carried = 0;
        self.line.clear();
        self.hunting = false;
        if self.eols >= RTC_EOLS {
            self.done = true;
            self.bits.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page with something on it: text-like bars, a solid block and some
    /// single-pel speckle, which between them reach every kind of code word.
    fn a_page(lines: usize, width: usize) -> Vec<Vec<bool>> {
        (0..lines)
            .map(|y| {
                (0..width)
                    .map(|x| match y % 5 {
                        0 => false,
                        1 => x % 97 < 3,
                        2 => (100..width.saturating_sub(100)).contains(&x),
                        3 => x % 2 == 0,
                        _ => x.wrapping_mul(y) % 31 == 0,
                    })
                    .collect()
            })
            .collect()
    }

    fn bits_of(coded: &Bits) -> Vec<bool> {
        let mut out = Vec::new();
        for (i, &byte) in coded.octets().iter().enumerate() {
            let last = i + 1 == coded.octets().len();
            let count = if last && coded.padding() > 0 {
                8 - coded.padding()
            } else {
                8
            };
            for b in 0..count {
                out.push(byte >> (7 - b) & 1 != 0);
            }
        }
        out
    }

    #[test]
    fn a_page_survives_being_coded_and_decoded() {
        let width = crate::page::WIDTH;
        let lines = a_page(40, width);
        let coded = encode(&lines);
        let mut decoder = Decoder::with_width(width);
        decoder.feed_bits(&bits_of(&coded));
        assert!(decoder.is_done(), "the return to control was not found");
        assert_eq!(decoder.damaged(), 0, "lines came out the wrong width");
        assert_eq!(decoder.lines(), lines.as_slice());
    }

    #[test]
    fn the_decoder_finds_the_page_through_whatever_came_before_it() {
        // What arrives in front of a page is the tail of a training sequence
        // through a descrambler, which is noise. The first EOL is where the
        // page starts and nothing before it should reach the paper.
        let width = 128;
        let lines = a_page(12, width);
        let coded = encode(&lines);
        let mut stream: Vec<bool> = (0..500)
            .map(|i: u32| !i.wrapping_mul(2_654_435_761).is_multiple_of(3))
            .collect();
        stream.extend(bits_of(&coded));
        let mut decoder = Decoder::with_width(width);
        decoder.feed_bits(&stream);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), lines.as_slice());
    }

    #[test]
    fn fill_in_front_of_an_end_of_line_is_not_part_of_the_line() {
        // 4.1.2 allows fill between the data and the EOL, so that a line can
        // be stretched to the minimum scan line time the far end asked for.
        // It is zeros, and the EOL still ends eleven zeros and a one.
        let width = 64;
        let lines = a_page(6, width);
        let mut padded = Bits::new();
        for line in &lines {
            for _ in 0..37 {
                padded.push(false);
            }
            write_line(&mut padded, line);
        }
        write_rtc(&mut padded);
        let mut decoder = Decoder::with_width(width);
        decoder.feed_bits(&bits_of(&padded));
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), lines.as_slice());
    }

    #[test]
    fn a_spoiled_line_costs_one_line_and_not_the_page() {
        let width = 128;
        let lines = a_page(20, width);
        let mut coded = bits_of(&encode(&lines));
        // Break something in the middle, well past the first line.
        let at = coded.len() / 2;
        for bit in &mut coded[at..at + 24] {
            *bit = !*bit;
        }
        let mut decoder = Decoder::with_width(width);
        decoder.feed_bits(&coded);
        assert!(decoder.is_done(), "the page never finished");
        assert!(decoder.damaged() > 0, "the damage went unnoticed");
        assert!(
            decoder.lines().len() >= lines.len() - 3,
            "lost {} lines to one bad run",
            lines.len() - decoder.lines().len()
        );
        assert_eq!(
            decoder.lines()[0],
            lines[0],
            "damage in the middle spoiled the start"
        );
    }

    #[test]
    fn nothing_but_zeros_does_not_grow_without_bound() {
        // Fill can be any length at all, and a receiver that keeps every bit
        // of it while waiting for the one that ends it has a leak.
        let mut decoder = Decoder::with_width(64);
        for _ in 0..100_000 {
            decoder.feed(false);
        }
        assert!(decoder.bits.len() <= EOL_ZEROS + 1);
        assert!(!decoder.is_done());
    }

    #[test]
    fn six_end_of_lines_in_a_row_end_the_page() {
        let width = 64;
        let mut coded = Bits::new();
        write_line(&mut coded, &vec![false; width]);
        write_rtc(&mut coded);
        let mut decoder = Decoder::with_width(width);
        let bits = bits_of(&coded);
        decoder.feed_bits(&bits);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines().len(), 1);
        // And nothing after it is taken as more page.
        decoder.feed_bits(&bits);
        assert_eq!(decoder.lines().len(), 1);
    }

    #[test]
    fn the_tables_are_the_length_the_recommendation_gives_them() {
        assert_eq!(WHITE_TERMINATING.len(), 64);
        assert_eq!(BLACK_TERMINATING.len(), 64);
        assert_eq!(WHITE_MAKEUP.len(), 27, "64 to 1728 in steps of 64");
        assert_eq!(BLACK_MAKEUP.len(), 27);
        assert_eq!(EXTENDED_MAKEUP.len(), 13, "1792 to 2560");
    }

    #[test]
    fn no_code_word_is_longer_than_its_own_length_says() {
        // A transcription slip that drops a leading zero shortens the word
        // and leaves the value unchanged, which no test of the value alone
        // can see. This catches the opposite slip -- a value too large for
        // the length claimed -- and the pair of them is checked by the
        // prefix test below.
        let all = WHITE_TERMINATING
            .iter()
            .chain(BLACK_TERMINATING.iter())
            .chain(WHITE_MAKEUP.iter())
            .chain(BLACK_MAKEUP.iter())
            .chain(EXTENDED_MAKEUP.iter())
            .chain(std::iter::once(&EOL));
        for code in all {
            assert!(code.len >= 2 && code.len <= 13, "{code:?}");
            assert!(
                u32::from(code.bits) < 1u32 << code.len,
                "{code:?} does not fit in {} bits",
                code.len
            );
        }
    }

    /// The property that makes the tables usable at all.
    fn prefix_free(set: &[Code]) -> Option<(Code, Code)> {
        for (i, a) in set.iter().enumerate() {
            for b in set.iter().skip(i + 1) {
                let (short, long) = if a.len <= b.len { (a, b) } else { (b, a) };
                let shifted = long.bits >> (long.len - short.len);
                if shifted == short.bits {
                    return Some((*short, *long));
                }
            }
        }
        None
    }

    #[test]
    fn each_colour_is_a_prefix_free_code() {
        // Huffman codes are decodable because no word begins another. If a
        // digit of the transcription is wrong the property usually breaks,
        // which is what makes this worth more than reading the table twice.
        let mut white: Vec<Code> = WHITE_TERMINATING.to_vec();
        white.extend_from_slice(&WHITE_MAKEUP);
        white.extend_from_slice(&EXTENDED_MAKEUP);
        white.push(EOL);
        assert_eq!(prefix_free(&white), None, "white");

        let mut black: Vec<Code> = BLACK_TERMINATING.to_vec();
        black.extend_from_slice(&BLACK_MAKEUP);
        black.extend_from_slice(&EXTENDED_MAKEUP);
        black.push(EOL);
        assert_eq!(prefix_free(&black), None, "black");
    }

    #[test]
    fn the_words_the_recommendation_quotes_are_the_words_here() {
        // Spot checks against Table 2 and 3a, at both ends and in the middle.
        assert_eq!(WHITE_TERMINATING[0], c(0b00110101, 8));
        assert_eq!(WHITE_TERMINATING[63], c(0b00110100, 8));
        assert_eq!(BLACK_TERMINATING[0], c(0b0000110111, 10));
        assert_eq!(BLACK_TERMINATING[2], c(0b11, 2), "the shortest word there is");
        assert_eq!(BLACK_TERMINATING[63], c(0b000001100111, 12));
        assert_eq!(WHITE_MAKEUP[0], c(0b11011, 5), "64 white");
        assert_eq!(WHITE_MAKEUP[25], c(0b011000, 6), "1664 white");
        assert_eq!(WHITE_MAKEUP[26], c(0b010011011, 9), "1728 white");
        assert_eq!(BLACK_MAKEUP[26], c(0b0000001100101, 13), "1728 black");
        assert_eq!(EXTENDED_MAKEUP[0], c(0b00000001000, 11), "1792");
        assert_eq!(EXTENDED_MAKEUP[12], c(0b000000011111, 12), "2560");
    }

    #[test]
    fn a_run_is_a_make_up_and_a_terminating_code() {
        let mut bits = Bits::new();
        write_run(&mut bits, Colour::White, 1728);
        // 1728 is a make-up of its own with a terminating zero after it.
        assert_eq!(bits.len(), 9 + 8, "1728 white then 0 white");

        let mut bits = Bits::new();
        write_run(&mut bits, Colour::White, 100);
        assert_eq!(bits.len(), 5 + 8, "64 white then 36 white");

        let mut bits = Bits::new();
        write_run(&mut bits, Colour::Black, 2);
        assert_eq!(bits.len(), 2, "the whole of 2 black");
    }

    #[test]
    fn a_blank_line_is_one_white_run_of_the_whole_width() {
        let line = vec![false; 1728];
        assert_eq!(runs(&line), vec![1728]);
        let mut bits = Bits::new();
        write_line(&mut bits, &line);
        assert_eq!(bits.len(), 12 + 9 + 8, "EOL, 1728 white, 0 white");
    }

    #[test]
    fn a_line_starting_black_starts_with_a_white_run_of_nothing() {
        let mut line = vec![false; 10];
        line[..3].fill(true);
        assert_eq!(runs(&line), vec![0, 3, 7]);
    }

    #[test]
    fn runs_alternate_and_add_up_to_the_width() {
        let mut line = vec![false; 1728];
        for (i, pel) in line.iter_mut().enumerate() {
            *pel = (i / 37) % 2 == 1;
        }
        let runs = runs(&line);
        assert_eq!(runs.iter().sum::<u32>(), 1728);
    }

    #[test]
    fn a_page_ends_in_return_to_control() {
        let page = vec![vec![false; 1728]; 4];
        let coded = encode(&page);
        // Six EOLs at the end, and nothing else after them.
        let mut expected = Bits::new();
        write_rtc(&mut expected);
        let tail = coded.len() - expected.len();
        assert_eq!(tail, 4 * (12 + 9 + 8), "four blank lines before the RTC");
    }

    #[test]
    fn the_bit_writer_puts_the_first_bit_at_the_top_of_the_first_octet() {
        let mut bits = Bits::new();
        bits.push_code(c(0b1, 1));
        assert_eq!(bits.octets(), &[0b1000_0000]);
        assert_eq!(bits.len(), 1);
        assert_eq!(bits.padding(), 7);
    }

    #[test]
    fn an_eol_cannot_be_made_by_any_run_of_code_words() {
        // 4.1.2: eleven zeros and a one occurs nowhere else, which is what
        // lets a receiver find the next line after an error. Checked by
        // coding a page of awkward runs and looking for eleven zeros
        // anywhere the EOLs are not.
        let mut line = vec![false; 1728];
        for (i, pel) in line.iter_mut().enumerate() {
            *pel = i % 3 == 0 || (100..163).contains(&i);
        }
        let mut bits = Bits::new();
        // No EOL: just the runs, so any eleven zeros found are a real fault.
        let mut colour = Colour::White;
        for run in runs(&line) {
            write_run(&mut bits, colour, run);
            colour = match colour {
                Colour::White => Colour::Black,
                Colour::Black => Colour::White,
            };
        }
        let mut zeros = 0;
        for i in 0..bits.len() {
            let bit = bits.octets()[i / 8] >> (7 - i % 8) & 1;
            zeros = if bit == 0 { zeros + 1 } else { 0 };
            assert!(zeros < 11, "eleven zeros at bit {i}, which is an EOL");
        }
    }
}
