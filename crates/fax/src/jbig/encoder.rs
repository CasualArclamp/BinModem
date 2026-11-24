//! The encoding half: lines in, a BIE out.

use super::arith::{self, Contexts};
use super::{
    ATMOVE, CONTEXTS, ESC, Header, NEWLEN, Options, Rows, SDNORM, STUFF, first_free, slntp_context,
};

/// Code a page as one BIE: the header, then each stripe's SDE, with an ATMOVE
/// in front of any stripe the AT pixel moves for and, under
/// `variable_length`, a NEWLEN in front of the last.
///
/// Every line is taken as wide as the first, white past the end of a shorter
/// one. A page with no lines is a header saying so and nothing else.
pub fn encode(lines: &[Vec<bool>], options: Options) -> Vec<u8> {
    let width = lines.first().map_or(crate::page::WIDTH, Vec::len);
    let height = lines.len();
    let stripe = options.stripe.max(1) as usize;
    let header = Header {
        width: width as u32,
        height: if options.variable_length { u32::MAX } else { height as u32 },
        stripe: stripe as u32,
        mx: options.mx.min(127),
        my: 0,
        two_line: options.two_line,
        variable_length: options.variable_length,
        typical_prediction: options.typical_prediction,
    };
    let mut out = header.to_bytes().to_vec();
    let mut page = Page::new(&header);
    let stripes = height.div_ceil(stripe);
    if options.variable_length && stripes == 0 {
        new_length(&mut out, 0);
    }
    for s in 0..stripes {
        if let Some(at) = page.switch.take() {
            // Annex C's suggestion, which 7.2.2's third test follows: a switch
            // found while coding one stripe takes effect at the top of the
            // next, so the marker goes in front of it with yAT = 0 and nothing
            // has to be held back.
            at_move(&mut out, 0, at);
            page.at = at;
        }
        if options.variable_length && s + 1 == stripes {
            new_length(&mut out, height as u32);
        }
        let first = s * stripe;
        let scd = page.stripe(&lines[first..height.min(first.saturating_add(stripe))]);
        for byte in scd {
            out.push(byte);
            if byte == ESC {
                out.push(STUFF);
            }
        }
        out.extend_from_slice(&[ESC, SDNORM]);
    }
    out
}

/// ATMOVE (Table 14): the line of the next stripe it takes effect at, then τX
/// and τY. τY is always 0 here, and τX 0 for the default place.
fn at_move(out: &mut Vec<u8>, line: u32, at: u8) {
    out.extend_from_slice(&[ESC, ATMOVE]);
    out.extend_from_slice(&line.to_be_bytes());
    out.extend_from_slice(&[at, 0]);
}

/// NEWLEN (Table 15): the height, packed as it is in the header.
fn new_length(out: &mut Vec<u8>, height: u32) {
    out.extend_from_slice(&[ESC, NEWLEN]);
    out.extend_from_slice(&height.to_be_bytes());
}

/// Everything that runs on from one stripe to the next.
#[derive(Debug)]
struct Page {
    width: usize,
    two_line: bool,
    typical_prediction: bool,
    mx: u8,
    contexts: Contexts,
    slntp: usize,
    rows: Rows,
    /// LNTP of the line above, 1 above the top of the image (6.5.1).
    lntp_above: bool,
    /// Where the AT pixel is, as τX, and where Annex C has decided it should
    /// go from the next stripe.
    at: u8,
    switch: Option<u8>,
}

impl Page {
    fn new(header: &Header) -> Self {
        Self {
            width: header.width as usize,
            two_line: header.two_line,
            typical_prediction: header.typical_prediction,
            mx: header.mx,
            contexts: Contexts::new(CONTEXTS),
            slntp: slntp_context(header.two_line),
            rows: Rows::new(header.width as usize),
            lntp_above: true,
            at: 0,
            switch: None,
        }
    }

    /// Code one stripe's lines, and return its SCD with the trailing zeros
    /// gone.
    ///
    /// Figure 21 for every pel: typical prediction says which need coding at
    /// all, the template gives each a context, and the coder does the rest.
    fn stripe(&mut self, lines: &[Vec<bool>]) -> Vec<u8> {
        let mut coder = arith::Encoder::new();
        let mut watch = (self.mx >= first_free(self.two_line))
            .then(|| Coincidences::new(self.mx, first_free(self.two_line)));
        for line in lines {
            self.rows.next();
            for (pel, &ink) in self.rows.pels_mut().iter_mut().zip(line) {
                *pel = u8::from(ink);
            }
            let coded = !self.typical_prediction || {
                // 6.5.1: LNTP is whether this line differs from the one above,
                // and what is coded is whether that is the same answer as
                // last time -- SLNTP -- since it seldom changes line to line.
                let lntp = !self.rows.typical();
                coder.encode(&mut self.contexts, self.slntp, lntp == self.lntp_above);
                self.lntp_above = lntp;
                lntp
            };
            if coded {
                for x in 0..self.width {
                    let cx = self.rows.context(self.two_line, x, self.at);
                    let pel = self.rows.row[super::LEFT + x];
                    coder.encode(&mut self.contexts, cx, pel == 1);
                    if let Some(watch) = watch.as_mut() {
                        watch.count(&self.rows, x);
                    }
                }
            }
            // Figure C.1: the check is at the end of a line, once enough pels
            // have been counted, and once a stripe.
            if let Some(counted) = watch.as_ref()
                && counted.all > CHECK_AFTER
            {
                self.switch = counted.check(self.at);
                watch = None;
            }
        }
        coder.flush()
    }
}

/// How many pels Figure C.1 counts before it checks: "c_all > 2048".
///
/// C.2's words say "greater than or equal to"; the figure is the procedure.
/// 7.2.2 cannot tell the two apart -- its lines count 1950 pels each, so the
/// check comes at 3900 either way -- and nor can a fax page with MX at 8,
/// whose lines count 1718.
const CHECK_AFTER: u32 = 2048;

/// Annex C's counters for one stripe: how often each place the AT pixel could
/// be agrees with the pel being coded.
///
/// Counted over the pels that are actually coded -- TPVALUE 2 -- and only
/// from MX to two short of the right edge, so that every candidate is a real
/// pel (C.3's changes to Figure C.1). Candidate 0 is the default place, up a
/// line and two to the right; candidate n, from the first that is not already
/// in the template up to MX, is n pels to the left on the same line.
#[derive(Debug)]
struct Coincidences {
    mx: usize,
    first: usize,
    /// c_all.
    all: u32,
    /// c_n, indexed by n; the entries between 0 and `first` are never used.
    counts: Vec<u32>,
}

impl Coincidences {
    fn new(mx: u8, first: u8) -> Self {
        Self {
            mx: usize::from(mx),
            first: usize::from(first),
            all: 0,
            counts: vec![0; usize::from(mx) + 1],
        }
    }

    fn count(&mut self, rows: &Rows, x: usize) {
        if x < self.mx || x + 2 >= rows.width {
            return;
        }
        let i = super::LEFT + x;
        let pel = rows.row[i];
        self.all += 1;
        self.counts[0] += u32::from(rows.above[i + 2] == pel);
        for n in self.first..=self.mx {
            self.counts[n] += u32::from(rows.row[i - n] == pel);
        }
    }

    /// CHECK, Figure C.2: where the AT pixel should move to, if anywhere.
    ///
    /// All seven of the figure's conditions have to hold: the best place
    /// predicts all but an eighth of the pels, it beats the place in use by
    /// more than it misses by and by a sixteenth of the pels -- and beats it
    /// inverted as well, since a pel that always disagrees predicts as well as
    /// one that always agrees -- the places differ by a quarter of the pels,
    /// and, moving away from the default, the places along the line differ by
    /// an eighth among themselves.
    ///
    /// Two misprints in the rendered figure are read the way the rest of it
    /// and Table 28 need. The third condition is printed with "<" rather than
    /// ">", which would ask the new place to be barely better than the old
    /// where the rest ask for much better, and which Table 28's own counters,
    /// the ones a switch was made on, fail by 1198 to 243. And the switch goes
    /// to "τ_lmax", which the figure never defines; what it defines is τ_max,
    /// the smallest τ with the largest count, over the same candidates c_max
    /// is taken over, and that is where the conditions say the gain is. In
    /// Table 28 the two are the same place.
    fn check(&self, old: u8) -> Option<u8> {
        let candidates = || std::iter::once(0).chain(self.first..=self.mx);
        let count = |n: usize| i64::from(self.counts[n]);
        let all = i64::from(self.all);
        let max = candidates().map(count).max()?;
        let min = candidates().map(count).min()?;
        let lmax = (self.first..=self.mx).map(count).max()?;
        let lmin = (self.first..=self.mx).map(count).min()?;
        let c_old = count(usize::from(old));
        let best = candidates().find(|&n| count(n) == max)?;
        let switch = all - max < all >> 3
            && max - c_old > all - max
            && max - c_old > all >> 4
            && max - (all - c_old) > all - max
            && max - (all - c_old) > all >> 4
            && max - min > all >> 2
            && (old != 0 || lmax - lmin > all >> 3);
        switch.then_some(best as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Decoder, artificial_image};
    use super::*;

    #[test]
    fn annex_c_counts_what_table_28_counts_and_moves_where_it_moves() {
        // Table 28: the third test of 7.2.2 moves the AT pixel to τX = 8 at
        // the top of stripe 9, on counters taken at the start of stripe 8 --
        // lines 1024 and 1025, the first two of the part of the image that
        // repeats every eight pels, and two lines of 1950 counted pels each.
        let image = artificial_image();
        let mut rows = Rows::new(1960);
        let mut watch = Coincidences::new(8, 3);
        for (y, line) in image.iter().enumerate().take(1026).skip(1022) {
            rows.next();
            for (pel, &ink) in rows.pels_mut().iter_mut().zip(line) {
                *pel = u8::from(ink);
            }
            if y >= 1024 {
                for x in 0..1960 {
                    watch.count(&rows, x);
                }
            }
        }
        assert_eq!(watch.all, 3900, "c_all");
        // Every counter as the rendered table prints it but one: c_6 is
        // printed 2442, and counts 2422. It is the same loop as the six that
        // agree to the pel, and no candidate from 3 to 8 comes near an edge,
        // so the twenty is a misprint. It changes nothing in CHECK: c_6 is
        // c_lmin either way, and the one condition that uses it holds with
        // either by more than six hundred.
        let table_28 = [(0, 2336), (3, 2456), (4, 2472), (5, 2446), (6, 2422), (7, 2730), (8, 3534)];
        for (n, want) in table_28 {
            assert_eq!(watch.counts[n], want, "c_{n}");
        }
        assert_eq!(watch.check(0), Some(8));
        // And once it is there, the same counts are no reason to move again.
        assert_eq!(watch.check(8), None);
    }

    #[test]
    fn a_page_of_one_colour_moves_nothing() {
        let mut watch = Coincidences::new(8, 3);
        let mut rows = Rows::new(300);
        for _ in 0..10 {
            rows.next();
            for x in 0..300 {
                watch.count(&rows, x);
            }
        }
        assert!(watch.all > CHECK_AFTER);
        assert_eq!(watch.check(0), None, "every candidate agrees with every pel");
    }

    fn text(rows: usize, width: usize) -> Vec<Vec<bool>> {
        (0..rows)
            .map(|y| {
                let within = y % 24;
                (0..width)
                    .map(|x| within < 18 && ((x + y / 24 * 7) % 29 < 4 || (within == 8 && x % 29 < 20)))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_stripe_ends_in_an_escape_and_sdnorm_and_nothing_else_is_escaped() {
        let bie = encode(&text(300, 1728), Options::FAX);
        assert_eq!(bie[..20], Header::read(bie[..20].try_into().unwrap()).unwrap().0.to_bytes());
        let body = &bie[20..];
        let mut ends = 0;
        let mut i = 0;
        while i < body.len() {
            if body[i] == ESC {
                match body[i + 1] {
                    STUFF => {}
                    SDNORM => ends += 1,
                    ATMOVE => i += 6,
                    other => panic!("ESC {other:#04x} at {i}"),
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        assert_eq!(ends, 300usize.div_ceil(128), "one SDE a stripe");
    }

    #[test]
    fn every_option_comes_back_as_it_went() {
        // Quick ones, in debug: the full image is the integration tests' job.
        let page = text(90, 400);
        for stripe in [1, 7, 32, 90, 1000] {
            for two_line in [false, true] {
                for typical_prediction in [false, true] {
                    for mx in [0, 8, 127] {
                        let options = Options { stripe, mx, two_line, typical_prediction, variable_length: false };
                        let bie = encode(&page, options);
                        let mut decoder = Decoder::new(400);
                        decoder.feed_bytes(&bie);
                        assert!(decoder.is_done(), "{options:?}");
                        assert_eq!(decoder.damaged(), 0, "{options:?}");
                        assert_eq!(decoder.lines(), page.as_slice(), "{options:?}");
                    }
                }
            }
        }
    }
}
