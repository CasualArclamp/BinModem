//! The codings a page can go down the line in, and one decoder for all of them.
//!
//! Which one a page is in is settled by the DCS: the sender picks from what the
//! receiver's DIS said it could read, and says which in the command. Everything
//! after that -- coding the page at one end, reading it at the other -- only
//! needs to know the answer.

use crate::page::{Page, Resolution};
use crate::{ecm, jbig, mmr, mr, t4};

/// How a page is coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coding {
    /// T.4 4.1: every line on its own, which every group 3 machine reads.
    #[default]
    ModifiedHuffman,
    /// T.4 4.2: lines coded against the line above, with a one-dimensional
    /// line every K and an EOL in front of every line. DIS and DCS bit 16.
    ModifiedRead,
    /// T.6 2.2, by way of T.4 4.3: every line coded against the one above and
    /// no EOLs at all. Only under error correction mode. DIS and DCS bit 31.
    Mmr,
    /// T.82's JBIG, as T.85 profiles it and T.4 4.4 allows it: every pel
    /// predicted from its neighbours and arithmetic coded. Only under error
    /// correction mode. DIS and DCS bit 78, and 79 for stripes of other than
    /// 128 lines.
    Jbig,
}

impl Coding {
    pub fn name(self) -> &'static str {
        match self {
            Self::ModifiedHuffman => "MH",
            Self::ModifiedRead => "MR",
            Self::Mmr => "MMR",
            Self::Jbig => "JBIG",
        }
    }

    /// Whether T.4 allows it only under error correction mode.
    ///
    /// JBIG for the same reason as MMR and more so: "the use of error free
    /// transmission is mandatory" (T.85 clause 3), since a wrong bit throws
    /// every estimate after it off as well as every line.
    pub fn needs_error_correction(self) -> bool {
        matches!(self, Self::Mmr | Self::Jbig)
    }

    /// Code a page.
    ///
    /// `min_bits` is the receiver's minimum scan line time as bits, which the
    /// two T.4 codings meet with fill. MMR and JBIG have no fill, and no need
    /// of it: they only run under error correction mode, where the time is
    /// zero. JBIG goes in T.85's basic stripes of 128 lines; [`jbig_bits`]
    /// codes it in others.
    pub fn encode(self, lines: &[Vec<bool>], resolution: Resolution, min_bits: usize) -> Vec<bool> {
        match self {
            Self::ModifiedHuffman => t4::encode_padded(lines, min_bits).to_bits(),
            Self::ModifiedRead => mr::encode(lines, mr::k_for(resolution), min_bits).to_bits(),
            Self::Mmr => mmr::encode(lines).to_bits(),
            Self::Jbig => jbig_bits(lines, jbig::Options::FAX),
        }
    }
}

/// A page in JBIG as the bits that go on the line: the BIE, each octet least
/// significant bit first (T.85 clause 3).
pub fn jbig_bits(lines: &[Vec<bool>], options: jbig::Options) -> Vec<bool> {
    ecm::unpack(&jbig::encode(lines, options))
}

/// Bits in and lines out, whichever coding the page is in.
#[derive(Debug)]
pub enum Decoder {
    T4(t4::Decoder),
    Mmr(mmr::Decoder),
    Jbig(Box<jbig::Decoder>),
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(Coding::default())
    }
}

impl Decoder {
    /// A decoder for a page of 1728 pels across.
    pub fn new(coding: Coding) -> Self {
        Self::with_width(crate::page::WIDTH, coding)
    }

    pub fn with_width(width: usize, coding: Coding) -> Self {
        match coding {
            Coding::ModifiedHuffman => {
                Self::T4(t4::Decoder::with_scheme(width, t4::Scheme::OneDimensional))
            }
            Coding::ModifiedRead => {
                Self::T4(t4::Decoder::with_scheme(width, t4::Scheme::TwoDimensional))
            }
            Coding::Mmr => Self::Mmr(mmr::Decoder::new(width)),
            Coding::Jbig => Self::Jbig(Box::new(jbig::Decoder::new(width))),
        }
    }

    pub fn coding(&self) -> Coding {
        match self {
            Self::T4(d) if d.scheme() == t4::Scheme::TwoDimensional => Coding::ModifiedRead,
            Self::T4(_) => Coding::ModifiedHuffman,
            Self::Mmr(_) => Coding::Mmr,
            Self::Jbig(_) => Coding::Jbig,
        }
    }

    pub fn feed_bits(&mut self, bits: &[bool]) {
        match self {
            Self::T4(d) => d.feed_bits(bits),
            Self::Mmr(d) => d.feed_bits(bits),
            Self::Jbig(d) => d.feed_bits(bits),
        }
    }

    /// The lines decoded so far.
    pub fn lines(&self) -> &[Vec<bool>] {
        match self {
            Self::T4(d) => d.lines(),
            Self::Mmr(d) => d.lines(),
            Self::Jbig(d) => d.lines(),
        }
    }

    /// Whether the page has ended: RTC for the T.4 codings, EOFB for MMR, and
    /// for JBIG every line its header -- or a NEWLEN -- says it has.
    pub fn is_done(&self) -> bool {
        match self {
            Self::T4(d) => d.is_done(),
            Self::Mmr(d) => d.is_done(),
            Self::Jbig(d) => d.is_done(),
        }
    }

    /// Lines that could not be read. For MMR and JBIG, one if the page
    /// stopped making sense part way down, since every line after that is
    /// gone with it.
    pub fn damaged(&self) -> usize {
        match self {
            Self::T4(d) => d.damaged(),
            Self::Mmr(d) => d.damaged(),
            Self::Jbig(d) => d.damaged(),
        }
    }

    /// Everything decoded, as a page.
    pub fn page(&self, resolution: Resolution) -> Page {
        Page {
            lines: self.lines().to_vec(),
            resolution,
        }
    }

    /// Start again, for a page in `coding`.
    pub fn reset_to(&mut self, coding: Coding) {
        let width = match self {
            Self::T4(d) => d.width(),
            Self::Mmr(d) => d.width(),
            Self::Jbig(d) => d.width(),
        };
        *self = Self::with_width(width, coding);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_page(rows: usize) -> Vec<Vec<bool>> {
        (0..rows)
            .map(|y| {
                (0..crate::page::WIDTH)
                    .map(|x| (x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn every_coding_decodes_what_it_encodes() {
        let lines = a_page(50);
        for coding in [Coding::ModifiedHuffman, Coding::ModifiedRead, Coding::Mmr, Coding::Jbig] {
            for resolution in [Resolution::Standard, Resolution::Fine] {
                let bits = coding.encode(&lines, resolution, 0);
                let mut decoder = Decoder::new(coding);
                assert_eq!(decoder.coding(), coding);
                decoder.feed_bits(&bits);
                assert!(decoder.is_done(), "{coding:?} {resolution:?}: the page never ended");
                assert_eq!(decoder.damaged(), 0, "{coding:?} {resolution:?}");
                assert_eq!(decoder.lines(), lines.as_slice(), "{coding:?} {resolution:?}");
            }
        }
    }

    #[test]
    fn a_decoder_reset_to_another_coding_reads_that_one() {
        let lines = a_page(10);
        let mut decoder = Decoder::new(Coding::ModifiedHuffman);
        decoder.feed_bits(&Coding::ModifiedHuffman.encode(&lines, Resolution::Standard, 0));
        decoder.reset_to(Coding::Mmr);
        assert!(decoder.lines().is_empty(), "the last page's lines are still there");
        decoder.feed_bits(&Coding::Mmr.encode(&lines, Resolution::Standard, 0));
        assert_eq!(decoder.lines(), lines.as_slice());
        decoder.reset_to(Coding::Jbig);
        assert_eq!(decoder.coding(), Coding::Jbig);
        assert!(decoder.lines().is_empty(), "the last page's lines are still there");
        decoder.feed_bits(&Coding::Jbig.encode(&lines, Resolution::Standard, 0));
        assert_eq!(decoder.lines(), lines.as_slice());
    }

    #[test]
    fn only_mmr_and_jbig_need_error_correction() {
        assert!(Coding::Mmr.needs_error_correction());
        assert!(Coding::Jbig.needs_error_correction());
        assert!(!Coding::ModifiedRead.needs_error_correction());
        assert!(!Coding::ModifiedHuffman.needs_error_correction());
    }

    #[test]
    fn a_jbig_page_goes_least_significant_bit_first() {
        // T.85 clause 3. The BIH's third byte is P = 1, so the page's
        // seventeenth bit on the line is a one and the twenty-fourth a zero.
        let bits = Coding::Jbig.encode(&a_page(4), Resolution::Standard, 0);
        assert_eq!(bits[..24].iter().filter(|&&b| b).count(), 1);
        assert!(bits[16], "P's low bit is not first");
    }
}
