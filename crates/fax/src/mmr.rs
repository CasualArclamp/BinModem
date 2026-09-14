//! MMR: T.6's basic coding scheme, which T.4 4.3 lets a group 3 machine use
//! under error correction mode.
//!
//! T.4's two-dimensional coding with everything taken out that is only there
//! because a line might arrive damaged. No end-of-line codes, since nothing
//! ever needs finding again; no one-dimensional line every K, since nothing
//! ever needs starting again from. The first line is coded against "an
//! imaginary white line" (2.2.1), every line after it against the line before,
//! and the block ends in EOFB (2.4.1.1).
//!
//! What that buys is size: a page in MMR is typically well under the same page
//! in Modified READ. What it costs is that one wrong bit spoils every line
//! after it, with nothing to resynchronize on -- which is why T.6 is "specified
//! assuming that transmission errors are corrected by control procedures at a
//! lower level" (1.2.1), and why T.4 4.3 "is limited to the use of the error
//! correction mode".

use crate::mr;
use crate::t4::{Bits, EOL, Reader, Spoiled};

/// 2.4.1.1: EOFB is "000000000001000000000001", which is two EOLs.
pub const EOFB_EOLS: usize = 2;

/// Zeros that no line can start with.
///
/// Every line starts with one of Table 1's code words, and the most zeros any
/// of them starts with is the extension's six: 0000001xxx. Seven zeros where a
/// line should start can only be EOFB.
const NO_LINE_STARTS: usize = 7;

/// The longest a line is let grow while it waits for the rest of itself.
///
/// The same bound as T.4's decoder, for the same reason: a line of 1728
/// alternating pels is under ten thousand bits, and what this guards against
/// is a stream that never finishes a line at all.
const LONGEST_LINE: usize = 65_536;

/// EOFB as bits.
fn eofb() -> Vec<bool> {
    let mut out = Bits::new();
    for _ in 0..EOFB_EOLS {
        out.push_code(EOL);
    }
    out.to_bits()
}

/// A page coded as 2.2 codes it, ended with EOFB.
///
/// No pad bits: 2.4.1.2 allows zeros after EOFB to fill out an octet, and the
/// frames of error correction mode add those as they pack the page.
pub fn encode(lines: &[Vec<bool>]) -> Bits {
    let mut out = Bits::new();
    let width = lines.first().map_or(crate::page::WIDTH, Vec::len);
    // 2.2.1: "The reference line for the first coding line in a page is an
    // imaginary white line."
    let mut above = vec![false; width];
    for line in lines {
        mr::write_line(&mut out, &above, line);
        above.clone_from(line);
    }
    for _ in 0..EOFB_EOLS {
        out.push_code(EOL);
    }
    out
}

/// MMR going the other way, as the bits arrive.
///
/// There is nothing to find a line by, so a line is read as soon as the whole
/// of it is here -- and the only way to know that is to try. A read that runs
/// out of bits is put back and tried again when more have come.
#[derive(Debug, Clone)]
pub struct Decoder {
    width: usize,
    /// The bits that have arrived, and how far into them the lines have got.
    bits: Vec<bool>,
    at: usize,
    /// The line the next one is coded against.
    above: Vec<bool>,
    lines: Vec<Vec<bool>>,
    done: bool,
    /// Something that was neither a line nor EOFB arrived. Nothing after it
    /// can be read, since nothing after it says where a line starts.
    spoiled: bool,
}

impl Decoder {
    pub fn new(width: usize) -> Self {
        Self {
            width,
            bits: Vec::new(),
            at: 0,
            above: vec![false; width],
            lines: Vec::new(),
            done: false,
            spoiled: false,
        }
    }

    /// The lines decoded so far.
    pub fn lines(&self) -> &[Vec<bool>] {
        &self.lines
    }

    /// Whether EOFB has arrived and the page is complete.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Whether the page stopped making sense part way down: one if it did,
    /// and every line after that point is lost, and none if not.
    pub fn damaged(&self) -> usize {
        usize::from(self.spoiled)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn feed_bits(&mut self, bits: &[bool]) {
        if self.done || self.spoiled {
            return;
        }
        self.bits.extend_from_slice(bits);
        let eofb = eofb();
        loop {
            let rest = &self.bits[self.at..];
            if rest.len() >= NO_LINE_STARTS && !rest[..NO_LINE_STARTS].contains(&true) {
                // Not a line, so EOFB or nothing. After it come pad bits, and
                // after those nothing that belongs to this page.
                let seen = rest.len().min(eofb.len());
                if rest[..seen] != eofb[..seen] {
                    self.spoiled = true;
                } else if seen == eofb.len() {
                    self.done = true;
                }
                break;
            }
            let mut reader = Reader::new(rest);
            match mr::read_line(&mut reader, &self.above) {
                Ok(line) => {
                    self.at += reader.position();
                    self.above.clone_from(&line);
                    self.lines.push(line);
                }
                Err(Spoiled::Short) if rest.len() <= LONGEST_LINE => break,
                Err(_) => {
                    self.spoiled = true;
                    break;
                }
            }
        }
        // What has been read is no use to anybody. Let it go now and then
        // rather than every time, since every time is a copy of what is left.
        if self.at > LONGEST_LINE {
            self.bits.drain(..self.at);
            self.at = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t4;

    /// Something like type, as `mr`'s tests have it: strokes that carry on
    /// down the page, drifting a pel now and then, with white between rows.
    fn text(rows: usize, width: usize) -> Vec<Vec<bool>> {
        (0..rows)
            .map(|y| {
                let row = y / 24;
                let within = y % 24;
                (0..width)
                    .map(|x| {
                        if within >= 18 {
                            return false;
                        }
                        let drift = (within / 6 + row) % 3;
                        let x = x + drift;
                        (x + row * 7) % 29 < 4 || (within == 8 && (x + row) % 29 < 20)
                    })
                    .collect()
            })
            .collect()
    }

    /// A page that reaches every mode: bars, a solid block, alternating pels,
    /// and runs that wander.
    fn busy(rows: usize, width: usize) -> Vec<Vec<bool>> {
        (0..rows)
            .map(|y| {
                (0..width)
                    .map(|x| match y % 7 {
                        0 => false,
                        1 | 2 => (x + y) % 23 < 9,
                        3 => (10..width - 10).contains(&x),
                        4 => x % 3 == 0,
                        _ => (x / 5 + y) % 4 == 0,
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn eofb_is_the_twenty_four_bits_t6_prints() {
        let printed = "000000000001000000000001";
        let bits: String = encode(&[]).to_bits().iter().map(|&b| if b { '1' } else { '0' }).collect();
        assert_eq!(bits, printed);
    }

    #[test]
    fn the_first_line_is_coded_against_a_white_line() {
        // 2.2.1. A white line under an imaginary white line is one V(0), for
        // the change just past the end that both lines have.
        let width = crate::page::WIDTH;
        let bits = encode(&[vec![false; width]]).to_bits();
        assert_eq!(bits.len(), 1 + 24);
        assert!(bits[0], "not V(0)");
        // And a line with a run in it codes exactly as Modified READ codes it
        // against a white line.
        let mut line = vec![false; width];
        line[100..140].fill(true);
        let mut against_white = Bits::new();
        mr::write_line(&mut against_white, &vec![false; width], &line);
        let bits = encode(std::slice::from_ref(&line)).to_bits();
        assert_eq!(bits[..against_white.len()], against_white.to_bits()[..]);
    }

    #[test]
    fn there_is_no_end_of_line_anywhere_in_a_page() {
        // What makes it smaller, and what makes it fragile: eleven zeros and a
        // one never appear until EOFB.
        let lines = busy(60, crate::page::WIDTH);
        let bits = encode(&lines).to_bits();
        let body = &bits[..bits.len() - 24];
        let eol = eofb()[..12].to_vec();
        assert!(!body.windows(12).any(|w| w == eol.as_slice()), "an EOL inside the page");
    }

    #[test]
    fn a_page_comes_back_as_it_went() {
        for lines in [busy(80, crate::page::WIDTH), text(120, crate::page::WIDTH), Vec::new()] {
            let bits = encode(&lines).to_bits();
            let mut decoder = Decoder::new(crate::page::WIDTH);
            decoder.feed_bits(&bits);
            assert!(decoder.is_done(), "no EOFB found");
            assert_eq!(decoder.damaged(), 0);
            assert_eq!(decoder.lines(), lines.as_slice());
        }
    }

    #[test]
    fn a_page_fed_a_bit_at_a_time_reads_the_same_and_arrives_line_by_line() {
        let lines = busy(40, crate::page::WIDTH);
        let bits = encode(&lines).to_bits();
        let mut decoder = Decoder::new(crate::page::WIDTH);
        let mut heights = vec![0];
        for bit in &bits {
            decoder.feed_bits(std::slice::from_ref(bit));
            if heights.last() != Some(&decoder.lines().len()) {
                heights.push(decoder.lines().len());
            }
        }
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), lines.as_slice());
        assert_eq!(heights, (0..=lines.len()).collect::<Vec<_>>(), "not a line at a time");
    }

    #[test]
    fn pad_bits_after_eofb_are_nothing() {
        // 2.4.1.2: "a variable length string of 0s", to an octet boundary or
        // the end of a frame.
        let lines = text(30, crate::page::WIDTH);
        let mut bits = encode(&lines).to_bits();
        bits.extend([false; 2000]);
        let mut decoder = Decoder::new(crate::page::WIDTH);
        decoder.feed_bits(&bits);
        decoder.feed_bits(&[true; 64]);
        assert!(decoder.is_done());
        assert_eq!(decoder.damaged(), 0);
        assert_eq!(decoder.lines(), lines.as_slice());
    }

    #[test]
    fn mmr_is_smaller_than_modified_read() {
        // Every line two-dimensional and no EOLs: on a page of type, this is
        // what it is for.
        let lines = text(240, crate::page::WIDTH);
        let mr = mr::encode(&lines, 4, 0).len();
        let mmr = encode(&lines).len();
        assert!(mmr * 10 < mr * 9, "MMR {mmr} bits against Modified READ's {mr}");
        assert!(mmr * 2 < t4::encode(&lines).len(), "not half of Modified Huffman");
    }

    #[test]
    fn a_wrong_bit_loses_the_rest_of_the_page_and_nothing_before_it() {
        let lines = busy(60, crate::page::WIDTH);
        let clean = encode(&lines).to_bits();
        // Where each line ends, to know which lines come before the damage.
        let mut ends = Vec::new();
        {
            let mut decoder = Decoder::new(crate::page::WIDTH);
            for (i, bit) in clean.iter().enumerate() {
                let before = decoder.lines().len();
                decoder.feed_bits(std::slice::from_ref(bit));
                if decoder.lines().len() > before {
                    ends.push(i + 1);
                }
            }
        }
        for at in [clean.len() / 5, clean.len() / 2, clean.len() * 4 / 5] {
            let mut bits = clean.clone();
            bits[at] = !bits[at];
            let mut decoder = Decoder::new(crate::page::WIDTH);
            decoder.feed_bits(&bits);
            let intact = ends.iter().filter(|&&end| end <= at).count();
            assert_eq!(
                decoder.lines()[..intact],
                lines[..intact],
                "a line before the damage at bit {at} changed"
            );
            assert_ne!(decoder.lines(), lines.as_slice(), "the damage at bit {at} went unnoticed");
            assert!(!decoder.is_done() || decoder.damaged() == 0);
        }
    }

    #[test]
    fn a_stream_that_never_finishes_a_line_is_given_up_on() {
        // Horizontal mode and then make-up codes of 2560 for ever: every one
        // of them a valid code word, and no line ever finished.
        let mut out = Bits::new();
        out.push_code(mr::HORIZONTAL);
        let mut decoder = Decoder::new(crate::page::WIDTH);
        decoder.feed_bits(&out.to_bits());
        let makeup = t4::EXTENDED_MAKEUP[12];
        let mut more = Bits::new();
        for _ in 0..600 {
            more.push_code(makeup);
        }
        for _ in 0..20 {
            decoder.feed_bits(&more.to_bits());
        }
        assert_eq!(decoder.damaged(), 1, "still waiting");
        assert!(decoder.bits.len() <= 2 * LONGEST_LINE + more.len(), "kept everything");
    }
}
