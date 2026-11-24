//! JBIG: T.82's single-progression sequential coding, as T.85 profiles it for
//! a fax.
//!
//! Where T.4 and T.6 describe a line by its runs, JBIG predicts each pel from
//! the ten around it that are already known -- two lines above and two pels to
//! the left, or one line above and four to the left -- and codes whether the
//! prediction held with an adaptive arithmetic coder (6.8). Every one of the
//! 1024 patterns those ten pels can make is a context with its own running
//! estimate of how likely ink is, so the coder learns the page as it goes: the
//! inside of a letter, the edge of a rule, the grain of a halftone. On text
//! that comes to somewhat less than MMR; on a dithered photograph it comes to
//! a fraction of it (T.82 Intro. 1).
//!
//! What T.85 takes of T.82 is the lowest resolution layer on its own (4.1):
//! D = 0 and P = 1, so no resolution reduction, no differential layers and no
//! deterministic prediction, and the page goes in one pass from the top. Three
//! things are left to the sender. The template, three lines or two (LRLTWO,
//! 6.7.1). Typical prediction, which sends a line that repeats the one above
//! as a single decision (TPBON, 6.5). And the adaptive template pixel, the one
//! pel of the ten that may move -- up to 127 pels to the left under T.85 --
//! to where a halftone's period makes it the best predictor (6.7.3, ATMOVE).
//!
//! The page is cut into stripes of L0 lines, 128 unless the far end offered
//! otherwise (T.85 Table 1, Note 4). The coder is flushed at the end of each,
//! so a stripe's data can be found in the stream by its marker, but the
//! estimates run on across them.
//!
//! A pel of ink is a 1: 6.1.1 makes 1 the foreground colour, which on paper is
//! the ink, and `true` here as everywhere else in the crate.
//!
//! One wrong bit and every pel after it is guessed from the wrong estimates,
//! with nothing to find its place again by: "the use of error free
//! transmission is mandatory" (T.85 clause 3), which in a group 3 fax means
//! error correction mode (T.4 4.4). The same clause sends every octet least
//! significant bit first.

mod arith;
mod decoder;
mod encoder;

pub use decoder::Decoder;
pub use encoder::encode;

/// The marker bytes of Table 2/T.82. Every marker is ESC and one of these,
/// and ESC in the coded data itself goes out as ESC STUFF (6.2.5).
pub const ESC: u8 = 0xff;
pub const STUFF: u8 = 0x00;
pub const RESERVE: u8 = 0x01;
pub const SDNORM: u8 = 0x02;
pub const SDRST: u8 = 0x03;
pub const ABORT: u8 = 0x04;
pub const NEWLEN: u8 = 0x05;
pub const ATMOVE: u8 = 0x06;
pub const COMMENT: u8 = 0x07;

/// The bits of the BIH's options byte, Table 8/T.82, from the most
/// significant: fill, LRLTWO, VLENGTH, TPDON, TPBON, DPON, DPPRIV, DPLAST.
const LRLTWO: u8 = 0x40;
const VLENGTH: u8 = 0x20;
const TPBON: u8 = 0x08;
const DPON: u8 = 0x04;
const DPPRIV: u8 = 0x02;
const DPLAST: u8 = 0x01;

/// L0 as T.85 calls it basic: 128 lines to a stripe, which every machine that
/// takes JBIG takes (Table 1, Note 4). Any other needs the far end to have
/// said so, with bit 79 of its DIS.
pub const BASIC_STRIPE: u32 = 128;

/// The size of the private DP table a BIH may carry (6.6.3, equation 7).
///
/// T.85 has DPON at 0, so a fax never sends one. A decoder that meets one all
/// the same has to step over it to find the image data.
const DP_TABLE: usize = 1728;

/// The widest line this will code or decode.
///
/// T.82 allows four thousand million pels across; T.4's widest line is
/// 14 592, A3 at 1200 pels to the inch (Table 1/T.4). The bound is what a
/// header from a far end is held to before anything is made that size.
pub const MAX_WIDTH: u32 = 1 << 15;

/// The longest page this will decode, in lines.
///
/// A header may say four thousand million, and T.85 has a sender that does not
/// know how long its page is say exactly that (Amendment 1, Appendix I). This
/// is eight and a half metres of paper at 7.7 lines to the millimetre, and what
/// it guards against is a stream that never stops producing lines.
pub const MAX_LINES: usize = 1 << 16;

/// The bi-level image header, BIH: the twenty bytes in front of the coded
/// page (6.2.2, Tables 6 to 8).
///
/// Only what T.85 lets vary is here. DL and D are 0, P is 1, and the order
/// byte and the options for differential layers and deterministic prediction
/// are 0 (T.85 Table 1, Note 1): written as that, and not needed to read a
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// XD: pels across a line.
    pub width: u32,
    /// YD: lines down the page -- or, with `variable_length`, the most it
    /// might come to, until a NEWLEN says (6.2.6.2).
    pub height: u32,
    /// L0: lines to a stripe.
    pub stripe: u32,
    /// MX: the furthest the AT pixel may move from where it starts. 0 to 127
    /// under T.85, and 0 says it never will (6.7.3).
    pub mx: u8,
    /// MY: always 0 under T.85, where the AT pixel only moves along the line
    /// being coded.
    pub my: u8,
    /// LRLTWO: the two-line template of Figure 15 rather than the three-line
    /// one of Figure 14.
    pub two_line: bool,
    /// VLENGTH: a NEWLEN marker segment may change the height.
    pub variable_length: bool,
    /// TPBON: typical prediction in the lowest resolution layer (6.5).
    pub typical_prediction: bool,
}

impl Header {
    pub const LEN: usize = 20;

    /// The header as it goes on the line: DL, D, P, a fill byte, then XD, YD
    /// and L0 as four bytes each, the most significant first, then MX, MY, the
    /// order byte and the options byte (6.2.2).
    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[2] = 1;
        out[4..8].copy_from_slice(&self.width.to_be_bytes());
        out[8..12].copy_from_slice(&self.height.to_be_bytes());
        out[12..16].copy_from_slice(&self.stripe.to_be_bytes());
        out[16] = self.mx;
        out[17] = self.my;
        out[19] = if self.two_line { LRLTWO } else { 0 }
            | if self.variable_length { VLENGTH } else { 0 }
            | if self.typical_prediction { TPBON } else { 0 };
        out
    }

    /// Read a header, and say whether a private DP table follows it.
    ///
    /// Refused, with the reason, is anything a single-progression sequential
    /// decoder cannot follow: a first layer other than 0, differential layers,
    /// more than one bit plane (T.85 Table 1), no pels across or more than
    /// [`MAX_WIDTH`], or no lines to a stripe (Table 9/T.82). The order byte
    /// and TPDON and DPON's meaning are "not necessary to recognize" (T.85
    /// Table 1, Note 1), and are not.
    pub fn read(bytes: &[u8; Self::LEN]) -> Result<(Self, bool), &'static str> {
        let word = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        let (dl, d, p) = (bytes[0], bytes[1], bytes[2]);
        if dl != 0 || d != 0 {
            return Err("more than the lowest resolution layer, which T.85 does not have");
        }
        if p != 1 {
            return Err("more than one bit plane");
        }
        let header = Self {
            width: word(4),
            height: word(8),
            stripe: word(12),
            mx: bytes[16],
            my: bytes[17],
            two_line: bytes[19] & LRLTWO != 0,
            variable_length: bytes[19] & VLENGTH != 0,
            typical_prediction: bytes[19] & TPBON != 0,
        };
        if header.width == 0 || header.width > MAX_WIDTH {
            return Err("a line width this cannot decode");
        }
        if header.stripe == 0 {
            return Err("stripes of no lines");
        }
        let options = bytes[19];
        let table = options & (DPON | DPPRIV | DPLAST) == DPON | DPPRIV;
        Ok((header, table))
    }
}

/// How a page is to be coded: the parameters T.85 Table 1 leaves to the
/// sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// L0, lines to a stripe. 128 is T.85's basic value, and anything from 1
    /// to the height of the page is its option, for a far end that has said
    /// it takes one (T.30 bit 79).
    pub stripe: u32,
    /// MX: how far to the left the AT pixel may be moved, 0 to 127. Where it
    /// goes within that is Annex C's decision, made once a stripe.
    pub mx: u8,
    /// LRLTWO: the two-line template, which T.82 puts at about 5 % bigger for
    /// being quicker to compute (6.7.1, the note to Figure 15).
    pub two_line: bool,
    /// TPBON: typical prediction.
    pub typical_prediction: bool,
    /// VLENGTH: the header gives the height as 0xffffffff, the most it could
    /// be, and a NEWLEN in front of the last stripe gives the real one -- what
    /// a machine that scans as it sends has to do (T.85 Amendment 1, I.1). A
    /// page coded here always knows its height, so this is for exercising a
    /// decoder rather than for a call.
    pub variable_length: bool,
}

impl Options {
    /// What a fax call sends: T.85's basic 128 lines a stripe, the
    /// three-line template, typical prediction, and the AT pixel free to move
    /// up to eight pels -- the parameters of 7.2.2's third test, the one that
    /// turns on everything that makes a page smaller.
    pub const FAX: Self = Self {
        stripe: BASIC_STRIPE,
        mx: 8,
        two_line: false,
        typical_prediction: true,
        variable_length: false,
    };
}

impl Default for Options {
    fn default() -> Self {
        Self::FAX
    }
}

/// The artificial image of 7.2.1: 1960 pels by 1951 lines, made by Figure 38
/// from a sixteen-bit shift register.
///
/// Every test of 7.2 codes this image, and its byte counts are exact: white
/// for the first 192 lines, which typical prediction takes whole; noise down
/// to line 1023; and then noise that repeats every eight pels in three
/// columns out of four, which is what moves the AT pixel.
pub fn artificial_image() -> Vec<Vec<bool>> {
    let mut prsg: u16 = 1;
    let mut repeat = [false; 8];
    (0..1951u32)
        .map(|j| {
            (0..1960u32)
                .map(|i| {
                    if j < 192 {
                        return false;
                    }
                    if j < 1023 || (i >> 3) & 3 == 0 {
                        let sum = (prsg & 1) + (prsg >> 2 & 1) + (prsg >> 11 & 1) + (prsg >> 15 & 1);
                        prsg = prsg << 1 | (sum & 1);
                        let pel = prsg & 3 == 0;
                        repeat[(i & 7) as usize] = pel;
                        pel
                    } else {
                        repeat[(i & 7) as usize]
                    }
                })
                .collect()
        })
        .collect()
}

/// Background either side of a row, so that every pel a template or the AT
/// pixel can reach is an ordinary index: 128 to the left, past the furthest
/// AT offset T.85 allows, and 4 to the right, past the default AT pixel's two.
const LEFT: usize = 128;
const RIGHT: usize = 4;

/// The three lines a template reaches: two above the one being coded, and it.
///
/// One byte a pel, 0 or 1, with background around them: 6.1.2 has the image
/// bordered with background to the top, left and right, which is what a row
/// of zeros above the first line and the padding either side are. Lines above
/// a stripe are the real ones -- "a pixel reference in a stripe above the
/// current one shall return the actual value" -- so these simply carry on
/// down the page, and start again only at an SDRST.
#[derive(Debug, Clone)]
struct Rows {
    width: usize,
    above2: Vec<u8>,
    above: Vec<u8>,
    row: Vec<u8>,
}

impl Rows {
    fn new(width: usize) -> Self {
        let blank = vec![0u8; LEFT + width + RIGHT];
        Self {
            width,
            above2: blank.clone(),
            above: blank.clone(),
            row: blank,
        }
    }

    /// Down a line: the row being coded becomes the one above, and a blank
    /// one takes its place.
    fn next(&mut self) {
        std::mem::swap(&mut self.above2, &mut self.above);
        std::mem::swap(&mut self.above, &mut self.row);
        self.pels_mut().fill(0);
    }

    /// Back to the top of the image, with background above.
    fn clear(&mut self) {
        for row in [&mut self.above2, &mut self.above, &mut self.row] {
            row.fill(0);
        }
    }

    fn pels(&self) -> &[u8] {
        &self.row[LEFT..LEFT + self.width]
    }

    fn pels_mut(&mut self) -> &mut [u8] {
        &mut self.row[LEFT..LEFT + self.width]
    }

    /// Whether the row repeats the one above: the typical line of 6.5.1.
    fn typical(&self) -> bool {
        self.pels() == &self.above[LEFT..LEFT + self.width]
    }

    /// Make the row the one above again, as a typical line is decoded.
    fn repeat(&mut self) {
        let (row, above) = (&mut self.row, &self.above);
        row[LEFT..LEFT + self.width].copy_from_slice(&above[LEFT..LEFT + self.width]);
    }

    /// The context of pel `x` of the row: the template of Figure 14, or of
    /// Figure 15 for `two_line`, with the AT pixel `at` pels to the left of it
    /// on the same line, or where the figure puts it for 0 (6.7.3: the
    /// default location "shall always be coded by" τX = τY = 0).
    ///
    /// 6.7.1 lets the pels go to the bits of the context in any order. They go
    /// here row by row from the top and left to right, with the AT pixel
    /// lowest:
    ///
    /// ```text
    /// three lines:  y-2   x-1 x x+1              bits 9 8 7
    ///               y-1   x-2 x-1 x x+1 [x+2]    bits 6 5 4 3 [0]
    ///               y     x-2 x-1                bits 2 1
    /// two lines:    y-1   x-3 x-2 x-1 x x+1 [x+2]  bits 9 8 7 6 5 [0]
    ///               y     x-4 x-3 x-2 x-1          bits 4 3 2 1
    /// ```
    #[inline]
    fn context(&self, two_line: bool, x: usize, at: u8) -> usize {
        let i = LEFT + x;
        let (a2, a1, r) = (&self.above2, &self.above, &self.row);
        let at = if at == 0 { a1[i + 2] } else { r[i - usize::from(at)] };
        let pel = |row: &[u8], k: usize| usize::from(row[k]);
        let template = if two_line {
            pel(a1, i - 3) << 9
                | pel(a1, i - 2) << 8
                | pel(a1, i - 1) << 7
                | pel(a1, i) << 6
                | pel(a1, i + 1) << 5
                | pel(r, i - 4) << 4
                | pel(r, i - 3) << 3
                | pel(r, i - 2) << 2
                | pel(r, i - 1) << 1
        } else {
            pel(a2, i - 1) << 9
                | pel(a2, i) << 8
                | pel(a2, i + 1) << 7
                | pel(a1, i - 2) << 6
                | pel(a1, i - 1) << 5
                | pel(a1, i) << 4
                | pel(a1, i + 1) << 3
                | pel(r, i - 2) << 2
                | pel(r, i - 1) << 1
        };
        template | usize::from(at)
    }
}

/// Contexts in the lowest resolution layer: ten pels, 1024 patterns (6.7.1).
const CONTEXTS: usize = 1024;

/// Figure 11: the context SLNTP is coded in with the three-line template, as
/// the figure draws it -- B for background, F for foreground, columns x - 2
/// to x + 2, and the AT pixel at its default place on the right of the middle
/// row.
const FIGURE_11: [&str; 3] = [" BBF ", "FFBBF", "BF?  "];

/// Figure 12: the same for the two-line template, columns x - 4 to x + 2.
const FIGURE_12: [&str; 2] = [" BFFBBF", "BFBF?  "];

/// The context typical prediction's pseudo-pixel SLNTP is coded in (6.5.1).
///
/// Not a context of its own but the one an ordinary pel would have in the
/// neighbourhood of Figure 11 or 12, chosen for being rare. It is worked out
/// here from the figure rather than written down as a number, since the
/// number depends on the order the pels go into a context and the figure does
/// not. The AT bit is the figure's "F" wherever the AT pixel has moved to:
/// the context is a pattern of the template's bits, not a place on the page.
fn slntp_context(two_line: bool) -> usize {
    let (figure, x): (&[&str], usize) = if two_line { (&FIGURE_12, 4) } else { (&FIGURE_11, 2) };
    let mut rows = Rows::new(figure[0].len());
    let paint = |row: &mut Vec<u8>, drawn: &str| {
        for (i, c) in drawn.bytes().enumerate() {
            row[LEFT + i] = u8::from(c == b'F');
        }
    };
    let (last, earlier) = figure.split_last().expect("a figure has rows");
    match earlier {
        [above] => paint(&mut rows.above, above),
        [above2, above] => {
            paint(&mut rows.above2, above2);
            paint(&mut rows.above, above);
        }
        _ => unreachable!("two or three rows"),
    }
    paint(&mut rows.row, last);
    rows.context(two_line, x, 0)
}

/// The first AT offset along the line that is not already in the template:
/// the three-line template has the two pels to the left of the one being
/// coded, and the two-line template four (6.7.3: "the new AT location shall
/// not overlap any regular pixels in the template").
fn first_free(two_line: bool) -> u8 {
    if two_line { 5 } else { 3 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_the_one_seven_three_prints_bar_its_second_byte() {
        // 7.3's sample BIH for single-progression sequential coding of a page
        // 1728 by 2376 in one stripe, every binary parameter 0. The rendered
        // page prints its second byte, D, as 0x01 -- the same as its two
        // progressive samples under it, and a number that would give the page
        // a differential layer while the text says it has none and the sample
        // has one SDE. T.85 Table 1 fixes D at 0, and 0 is what goes here.
        let printed: [u8; 20] = [
            0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x06, 0xc0, 0x00, 0x00, 0x09, 0x48,
            0x00, 0x00, 0x09, 0x48, 0x00, 0x00, 0x00, 0x00,
        ];
        let header = Header {
            width: 1728,
            height: 2376,
            stripe: 2376,
            mx: 0,
            my: 0,
            two_line: false,
            variable_length: false,
            typical_prediction: false,
        };
        let ours = header.to_bytes();
        assert_eq!(ours[1], 0x00, "D");
        assert_eq!(ours[0], printed[0]);
        assert_eq!(ours[2..], printed[2..]);
        assert_eq!(Header::read(&ours), Ok((header, false)));
        // And the page as printed is a page this cannot read, and says why.
        assert!(Header::read(&printed).is_err());
    }

    #[test]
    fn the_options_byte_is_table_8() {
        let header = Header {
            width: 1728,
            height: 10,
            stripe: 128,
            mx: 8,
            my: 0,
            two_line: true,
            variable_length: true,
            typical_prediction: true,
        };
        let bytes = header.to_bytes();
        // Fill, LRLTWO, VLENGTH, TPDON, TPBON, DPON, DPPRIV, DPLAST.
        assert_eq!(bytes[19], 0b0110_1000);
        assert_eq!(bytes[16], 8);
        assert_eq!(bytes[12..16], [0, 0, 0, 128]);
        assert_eq!(Header::read(&bytes).map(|(h, _)| h), Ok(header));
    }

    #[test]
    fn a_header_with_a_private_dp_table_says_so() {
        let mut bytes = Options::FAX_HEADER.to_bytes();
        bytes[19] |= DPON | DPPRIV;
        assert_eq!(Header::read(&bytes).map(|(_, table)| table), Ok(true));
        bytes[19] |= DPLAST;
        assert_eq!(Header::read(&bytes).map(|(_, table)| table), Ok(false), "the last table again");
    }

    #[test]
    fn a_header_this_cannot_follow_is_refused() {
        let good = Options::FAX_HEADER.to_bytes();
        for (at, value) in [(0, 1u8), (1, 1), (2, 2), (4, 0x80), (15, 0)] {
            let mut bytes = good;
            bytes[at] = value;
            if at == 15 {
                bytes[12..16].fill(0);
            }
            assert!(Header::read(&bytes).is_err(), "byte {at} as {value:#x}");
        }
        let mut narrow = good;
        narrow[4..8].fill(0);
        assert!(Header::read(&narrow).is_err(), "no pels across");
    }

    #[test]
    fn slntp_is_coded_in_the_contexts_the_figures_draw() {
        // Worked by hand from Figures 11 and 12 in the bit order `context`
        // documents: rows 001, 1100 and 01 with the AT bit 1 after them, and
        // rows 01100 and 0101 with the AT bit 1.
        assert_eq!(slntp_context(false), 0b00_1110_0011);
        assert_eq!(slntp_context(true), 0b01_1000_1011);
    }

    #[test]
    fn the_artificial_image_is_the_size_seven_two_one_says() {
        let image = artificial_image();
        assert_eq!(image.len(), 1951);
        assert!(image.iter().all(|line| line.len() == 1960));
        let ink: usize = image.iter().flatten().filter(|&&p| p).count();
        assert_eq!(ink, 861_965, "foreground");
        assert_eq!(1960 * 1951 - ink, 2_961_995, "background");
    }

    impl Options {
        /// A header of the kind a fax call sends, for tests that want one.
        const FAX_HEADER: Header = Header {
            width: 1728,
            height: 2287,
            stripe: 128,
            mx: 8,
            my: 0,
            two_line: false,
            variable_length: false,
            typical_prediction: true,
        };
    }
}
