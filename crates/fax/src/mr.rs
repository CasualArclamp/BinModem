//! Modified READ: T.4's two-dimensional coding, 4.2.
//!
//! A line coded against the one above it. Most of a page is the same from one
//! scan line to the next -- the stroke of a letter carries on down, the edge of
//! a box stays where it was -- so instead of coding each line's runs from
//! scratch, this codes where each colour change on the line is relative to the
//! nearest change on the line before. A change directly under one above is a
//! single bit.
//!
//! Three modes, and T.4's own names for the positions they work between:
//!
//! - **a0** is where coding has got to on this line, and its colour is the
//!   colour of the run starting there.
//! - **a1** and **a2** are the next two changes on this line after a0.
//! - **b1** is the first change on the line above that is past a0 and turns
//!   to the colour opposite a0's; **b2** is the change after b1.
//!
//! Pass mode, when b2 is left of a1: the line above has a whole run this line
//! does not, and a0 moves to under b2. Vertical mode, when a1 is within three
//! of b1: say how far, and a0 moves to a1. Otherwise horizontal mode: code the
//! next two runs as Modified Huffman does, and a0 moves to a2.
//!
//! This is also the whole of T.6's coding with the end-of-line codes taken
//! out, which is why it is its own module.

use crate::t4::{self, Bits, Colour, EOL, Spoiled};

/// A position on a line, which can be one before the first pel: that is where
/// every line's a0 starts (4.2.1.3.4).
type Position = isize;

/// Table 4: pass mode.
pub const PASS: t4::Code = t4::Code { bits: 0b0001, len: 4 };
/// Table 4: horizontal mode, followed by the two runs.
pub const HORIZONTAL: t4::Code = t4::Code { bits: 0b001, len: 3 };

/// Table 4: vertical mode, indexed by a1 minus b1 plus three, so from VL(3)
/// through V(0) to VR(3).
///
/// Read off the table in the PDF. The extracted text has every word from
/// VR(1)'s down slid one row below its notation, so VR(2) sits beside 011 and
/// VL(1)'s 010 ends up beside the extension row.
pub const VERTICAL: [t4::Code; 7] = [
    t4::Code { bits: 0b0000010, len: 7 }, // VL(3)
    t4::Code { bits: 0b000010, len: 6 },  // VL(2)
    t4::Code { bits: 0b010, len: 3 },     // VL(1)
    t4::Code { bits: 0b1, len: 1 },       // V(0)
    t4::Code { bits: 0b011, len: 3 },     // VR(1)
    t4::Code { bits: 0b000011, len: 6 },  // VR(2)
    t4::Code { bits: 0b0000011, len: 7 }, // VR(3)
];

/// The maximum K of 4.2.1.1: how many lines apart the one-dimensional lines
/// may be, "in order to limit the disturbed area in the event of transmission
/// errors".
pub fn k_for(resolution: crate::page::Resolution) -> usize {
    match resolution {
        crate::page::Resolution::Standard => 2,
        crate::page::Resolution::Fine => 4,
    }
}

/// Where the colour changes along a line: every position whose pel differs
/// from the one before it, the one before the first being white.
pub fn changes(line: &[bool]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut previous = false;
    for (i, &pel) in line.iter().enumerate() {
        if pel != previous {
            out.push(i);
            previous = pel;
        }
    }
    out
}

/// b1 and b2 on a reference line, for coding that has reached `a0` with the
/// colour `black`.
///
/// `from` is where the search may start in `reference`, and comes back moved
/// on: a0 never moves left, so neither does the first change past it, and a
/// line of alternating pels would otherwise be searched from its start once
/// per pel.
///
/// The reference line starts white, so its changes alternate to black, to
/// white, to black: the k-th turns to black exactly when k is even. b1 has to
/// turn to the colour opposite a0's, which is at most one change further on.
/// A change that is not there is the imaginary one just past the last pel.
fn b1_b2(reference: &[usize], width: usize, a0: Position, black: bool, from: &mut usize) -> (usize, usize) {
    while *from < reference.len() && reference[*from] as Position <= a0 {
        *from += 1;
    }
    let mut k = *from;
    if k < reference.len() && k.is_multiple_of(2) == black {
        k += 1;
    }
    let at = |i: usize| reference.get(i).copied().unwrap_or(width);
    (at(k), at(k + 1))
}

/// Code one line against the line above it (4.2.1.3.3 and Figure 7).
///
/// Only the line: no end-of-line code or tag bit, which belong to whatever is
/// putting lines together.
pub fn write_line(out: &mut Bits, reference: &[bool], coding: &[bool]) {
    let width = coding.len();
    let above = changes(reference);
    let here = changes(coding);
    let mut a0: Position = -1;
    let mut black = false;
    let mut from_above = 0usize;
    let mut from_here = 0usize;
    while a0 < width as Position {
        while from_here < here.len() && here[from_here] as Position <= a0 {
            from_here += 1;
        }
        let a1 = here.get(from_here).copied().unwrap_or(width);
        let (b1, b2) = b1_b2(&above, width, a0, black, &mut from_above);
        if b2 < a1 {
            // Step 1: pass mode, and a0 to just under b2.
            out.push_code(PASS);
            a0 = b2 as Position;
        } else if (a1 as Position - b1 as Position).abs() <= 3 {
            // Step 2 ii: vertical mode, and a0 to a1.
            let offset = a1 as Position - b1 as Position;
            out.push_code(VERTICAL[(offset + 3) as usize]);
            a0 = a1 as Position;
            black = !black;
        } else {
            // Step 2 iii: horizontal mode, both runs, and a0 to a2. The first
            // run on a line starts at the pel a0 is just before, which is
            // 4.2.1.3.4's a0a1 - 1.
            let a2 = here.get(from_here + 1).copied().unwrap_or(width);
            let start = a0.max(0) as usize;
            out.push_code(HORIZONTAL);
            let (first, second) = if black {
                (Colour::Black, Colour::White)
            } else {
                (Colour::White, Colour::Black)
            };
            t4::write_run(out, first, (a1 - start) as u32);
            t4::write_run(out, second, (a2 - a1) as u32);
            a0 = a2 as Position;
        }
    }
}

/// One of Table 4's modes, as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pass,
    Horizontal,
    Vertical(Position),
}

/// Read one of Table 4's code words.
///
/// The table is prefix-free, so the first word that matches is the word: one
/// bit decides V(0), three decide VR(1), VL(1) and horizontal, four decide
/// pass, and the rest are six or seven.
fn read_mode(reader: &mut t4::Reader<'_>) -> Result<Mode, Spoiled> {
    let mut value: u16 = 0;
    for len in 1..=7u8 {
        let bit = reader.next().ok_or(Spoiled::Short)?;
        value = value << 1 | u16::from(bit);
        if len == HORIZONTAL.len && value == HORIZONTAL.bits {
            return Ok(Mode::Horizontal);
        }
        if len == PASS.len && value == PASS.bits {
            return Ok(Mode::Pass);
        }
        if let Some(i) = VERTICAL.iter().position(|c| c.len == len && c.bits == value) {
            return Ok(Mode::Vertical(i as Position - 3));
        }
    }
    Err(Spoiled::NotACode)
}

/// Read one line coded against the line above it.
///
/// The exact reverse of [`write_line`]: the same a0, b1 and b2, found the same
/// way, with the modes read rather than chosen. Reading stops at the end of the
/// line and leaves whatever follows -- fill, an end-of-line code -- unread.
pub fn read_line(reader: &mut t4::Reader<'_>, reference: &[bool]) -> Result<Vec<bool>, Spoiled> {
    let width = reference.len();
    let above = changes(reference);
    let mut line = Vec::with_capacity(width);
    let mut a0: Position = -1;
    let mut black = false;
    let mut from_above = 0usize;
    while a0 < width as Position {
        let (b1, b2) = b1_b2(&above, width, a0, black, &mut from_above);
        let start = a0.max(0) as usize;
        match read_mode(reader)? {
            Mode::Pass => {
                // b2 is left of a1 by definition, so it cannot be the end of
                // the line: nothing past it has been coded yet.
                if b2 >= width {
                    return Err(Spoiled::PastTheEnd);
                }
                line.resize(b2, black);
                a0 = b2 as Position;
            }
            Mode::Vertical(offset) => {
                let a1 = b1 as Position + offset;
                if a1 > width as Position || a1 < start as Position || (a1 == a0 && a0 >= 0) {
                    return Err(Spoiled::PastTheEnd);
                }
                line.resize(a1 as usize, black);
                a0 = a1;
                black = !black;
            }
            Mode::Horizontal => {
                let (first, second) = if black {
                    (Colour::Black, Colour::White)
                } else {
                    (Colour::White, Colour::Black)
                };
                let one = t4::read_run(reader, first)? as usize;
                let two = t4::read_run(reader, second)? as usize;
                let a1 = start + one;
                let a2 = a1 + two;
                if a2 > width {
                    return Err(Spoiled::PastTheEnd);
                }
                line.resize(a1, black);
                line.resize(a2, !black);
                a0 = a2 as Position;
            }
        }
    }
    Ok(line)
}

/// A whole page as Modified READ: every K-th line one-dimensional, the ones
/// between coded against the line above, and each line's fill brought up to
/// `min_bits` (4.2).
///
/// 4.2.2: "EOL plus the tag bit 1 signal will occur prior to the first data
/// line of a page", EOL + 1 before a one-dimensional line and EOL + 0 before a
/// two-dimensional one. 4.2.4: RTC is six of EOL + 1.
pub fn encode(lines: &[Vec<bool>], k: usize, min_bits: usize) -> Bits {
    let k = k.max(1);
    let mut out = Bits::new();
    let mut above: Vec<bool> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let before = out.len();
        let two_dimensional = !i.is_multiple_of(k);
        out.push_code(EOL);
        out.push(!two_dimensional);
        if two_dimensional {
            write_line(&mut out, &above, line);
        } else {
            t4::write_runs(&mut out, line);
        }
        // 4.2.3: fill between the data and the next EOL, counted over the
        // whole coded line including its own EOL and tag.
        for _ in out.len() - before..min_bits {
            out.push(false);
        }
        above.clone_from(line);
    }
    for _ in 0..t4::RTC_EOLS {
        out.push_code(EOL);
        out.push(true);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 64;

    fn line_of(spec: &[(usize, usize)], width: usize) -> Vec<bool> {
        let mut line = vec![false; width];
        for &(from, to) in spec {
            for pel in &mut line[from..to] {
                *pel = true;
            }
        }
        line
    }

    fn round_trip(reference: &[bool], coding: &[bool]) -> Vec<bool> {
        let mut out = Bits::new();
        write_line(&mut out, reference, coding);
        let bits = out.to_bits();
        let mut reader = t4::Reader::new(&bits);
        let line = read_line(&mut reader, reference).expect("it did not read back");
        assert_eq!(reader.position(), bits.len(), "not every bit written was read");
        line
    }

    #[test]
    fn a_line_the_same_as_the_one_above_is_a_bit_a_change() {
        // Every change directly under one above: V(0) for each, and one more
        // for the imaginary change past the end. Nothing else.
        let line = line_of(&[(10, 20), (30, 31), (50, 60)], WIDTH);
        let mut out = Bits::new();
        write_line(&mut out, &line, &line);
        assert_eq!(out.len(), changes(&line).len() + 1);
        assert_eq!(round_trip(&line, &line), line);
    }

    #[test]
    fn a_blank_line_under_a_blank_line_is_one_bit() {
        let blank = vec![false; WIDTH];
        let mut out = Bits::new();
        write_line(&mut out, &blank, &blank);
        assert_eq!(out.to_bits(), vec![true], "V(0) for the end of the line");
    }

    #[test]
    fn every_vertical_offset_reads_back() {
        let above = line_of(&[(20, 40)], WIDTH);
        for shift in -3isize..=3 {
            let from = (20 + shift) as usize;
            let to = (40 + shift) as usize;
            let coding = line_of(&[(from, to)], WIDTH);
            assert_eq!(round_trip(&above, &coding), coding, "shifted {shift}");
        }
    }

    #[test]
    fn a_run_above_that_is_not_here_is_passed() {
        // A short black run on the line above and nothing under it: b2 is
        // left of a1, so it goes by in pass mode.
        let above = line_of(&[(10, 14)], WIDTH);
        let coding = line_of(&[(40, 50)], WIDTH);
        let mut out = Bits::new();
        write_line(&mut out, &above, &coding);
        let bits = out.to_bits();
        assert_eq!(bits[..4], [false, false, false, true], "pass mode first");
        assert_eq!(round_trip(&above, &coding), coding);
    }

    #[test]
    fn a_change_far_from_anything_above_is_coded_horizontally() {
        let above = vec![false; WIDTH];
        let coding = line_of(&[(30, 35)], WIDTH);
        let mut out = Bits::new();
        write_line(&mut out, &above, &coding);
        let bits = out.to_bits();
        assert_eq!(bits[..3], [false, false, true], "horizontal mode first");
        assert_eq!(round_trip(&above, &coding), coding);
    }

    #[test]
    fn a_line_that_starts_black_starts_with_a_white_run_of_nothing() {
        // 4.2.1.3.4: a0 starts just before the first pel, so a black first
        // pel coded horizontally is a white run of zero first.
        let above = vec![false; WIDTH];
        let coding = line_of(&[(0, 9)], WIDTH);
        assert_eq!(round_trip(&above, &coding), coding);
        let under = line_of(&[(0, 9)], WIDTH);
        assert_eq!(round_trip(&under, &coding), coding);
    }

    #[test]
    fn lines_that_run_to_the_edge_read_back() {
        let above = line_of(&[(0, 1), (62, 64)], WIDTH);
        for coding in [
            line_of(&[(0, 64)], WIDTH),
            line_of(&[(63, 64)], WIDTH),
            line_of(&[(0, 1), (60, 64)], WIDTH),
            vec![true; WIDTH],
        ] {
            assert_eq!(round_trip(&above, &coding), coding);
        }
    }

    #[test]
    fn alternating_pels_under_anything_read_back() {
        // The worst case for the search: a change at every pel.
        let checker: Vec<bool> = (0..WIDTH).map(|x| x % 2 == 1).collect();
        let other: Vec<bool> = (0..WIDTH).map(|x| x % 2 == 0).collect();
        for (above, coding) in [(&checker, &other), (&other, &checker), (&checker, &checker)] {
            assert_eq!(&round_trip(above, coding), coding);
        }
        assert_eq!(round_trip(&[false; WIDTH], &checker), checker);
    }

    #[test]
    fn many_pairs_of_lines_read_back() {
        // A cheap generator, so the pairs are the same every run.
        let mut seed = 0x9e37_79b9u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..2000 {
            let density = next() % 8;
            let mut make = || -> Vec<bool> {
                let mut pel = false;
                (0..WIDTH)
                    .map(|_| {
                        if next() % 16 < density {
                            pel = !pel;
                        }
                        pel
                    })
                    .collect()
            };
            let above = make();
            let coding = make();
            assert_eq!(round_trip(&above, &coding), coding);
        }
    }

    #[test]
    fn no_run_of_these_code_words_makes_an_end_of_line() {
        // An EOL has to be findable in the middle of a page after an error,
        // which only works if no sequence of real code words contains eleven
        // zeros. Table 4's words are the new ones here: none may end in so
        // many zeros that the start of any other word completes an EOL.
        let trailing = |c: &t4::Code| (0..c.len).take_while(|i| c.bits >> i & 1 == 0).count();
        let leading = |c: &t4::Code| (0..c.len).rev().take_while(|i| c.bits >> i & 1 == 0).count();
        let mut words: Vec<t4::Code> = VERTICAL.to_vec();
        words.push(PASS);
        words.push(HORIZONTAL);
        words.extend(t4::WHITE_TERMINATING);
        words.extend(t4::BLACK_TERMINATING);
        words.extend(t4::WHITE_MAKEUP);
        words.extend(t4::BLACK_MAKEUP);
        words.extend(t4::EXTENDED_MAKEUP);
        let worst_end = words.iter().map(trailing).max().unwrap();
        let worst_start = words.iter().map(leading).max().unwrap();
        assert!(
            worst_end + worst_start < 11,
            "{worst_end} trailing and {worst_start} leading zeros make an EOL"
        );
    }


    fn page(rows: usize, width: usize) -> Vec<Vec<bool>> {
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
    fn a_page_of_modified_read_decodes_at_both_values_of_k() {
        for k in [2, 4] {
            let lines = page(40, crate::page::WIDTH);
            let bits = encode(&lines, k, 0).to_bits();
            let mut decoder =
                t4::Decoder::with_scheme(crate::page::WIDTH, t4::Scheme::TwoDimensional);
            decoder.feed_bits(&bits);
            assert!(decoder.is_done(), "K = {k}: no return to control");
            assert_eq!(decoder.damaged(), 0, "K = {k}");
            assert_eq!(decoder.lines(), lines.as_slice(), "K = {k}");
        }
    }

    /// Something like type: strokes that carry on down the page for a few
    /// lines at a time, drifting a pel now and then, with white between rows.
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

    #[test]
    fn modified_read_is_smaller_than_modified_huffman_for_a_page_with_depth() {
        // What it is for. A page where the lines are mostly like the ones above
        // them should come out well under the one-dimensional coding.
        let lines = text(240, crate::page::WIDTH);
        let mh = t4::encode(&lines).len();
        let mr = encode(&lines, 4, 0).len();
        assert!(
            mr * 10 < mh * 7,
            "{mr} bits of Modified READ against {mh} of Huffman"
        );
    }

    #[test]
    fn fill_and_noise_do_not_get_into_a_modified_read_page() {
        let width = 128;
        let lines = page(16, width);
        let mut stream: Vec<bool> = (0..300u32)
            .map(|i| !i.wrapping_mul(2_654_435_761).is_multiple_of(3))
            .collect();
        stream.extend(encode(&lines, 2, 400).to_bits());
        let mut decoder = t4::Decoder::with_scheme(width, t4::Scheme::TwoDimensional);
        decoder.feed_bits(&stream);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), lines.as_slice());
    }

    #[test]
    fn damage_stops_at_the_next_one_dimensional_line() {
        // 4.2.1.1: K is there "to limit the disturbed area in the event of
        // transmission errors". A spoiled line takes the two-dimensional lines
        // coded against it, and the next one-dimensional line is read cleanly
        // again.
        let width = 128;
        let lines = page(24, width);
        let clean = encode(&lines, 4, 0).to_bits();
        let mut damaged = clean.clone();
        let at = damaged.len() / 3;
        for bit in &mut damaged[at..at + 16] {
            *bit = !*bit;
        }
        let mut decoder = t4::Decoder::with_scheme(width, t4::Scheme::TwoDimensional);
        decoder.feed_bits(&damaged);
        assert!(decoder.is_done(), "the page never finished");
        assert!(decoder.damaged() > 0, "the damage went unnoticed");
        assert!(
            decoder.lines().len() >= lines.len() - 5,
            "{} of {} lines survived one error",
            decoder.lines().len(),
            lines.len()
        );
        assert_eq!(decoder.lines().last(), lines.last(), "the end of the page was lost");
    }

    #[test]
    fn a_page_of_modified_read_is_every_kth_line_one_dimensional() {
        let lines: Vec<Vec<bool>> = (0..9).map(|y| line_of(&[(y, y + 10)], WIDTH)).collect();
        let bits = encode(&lines, 4, 0).to_bits();
        // Find each EOL and the tag bit after it.
        let mut tags = Vec::new();
        let mut zeros = 0;
        let mut i = 0;
        while i < bits.len() {
            if bits[i] && zeros >= 11 {
                tags.push(bits[i + 1]);
                i += 2;
                zeros = 0;
                continue;
            }
            zeros = if bits[i] { 0 } else { zeros + 1 };
            i += 1;
        }
        let want: Vec<bool> = (0..9).map(|y: usize| y.is_multiple_of(4)).chain([true; 6]).collect();
        assert_eq!(tags, want, "EOL + 1 before every fourth line and RTC");
    }
}
