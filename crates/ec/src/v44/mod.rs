//! ITU-T V.44 data compression (LZJH).
//!
//! The successor to V.42bis, and better on the things a modem actually
//! carries: the Recommendation's own summary claims "a better performance than
//! V.42 bis on many types of data", and the reason is structural rather than a
//! matter of tuning. V.42bis stores one character per dictionary entry and
//! climbs a tree a character at a time, so a long repeated string costs a
//! codeword for every character of it that was learned separately. V.44 stores
//! *string-segments* -- a node holds a run of characters and a length -- and
//! adds a second trick on top: after matching the longest string it knows, it
//! tries to carry on matching against the raw history and sends only how many
//! more characters matched (6.3.2). So a long repeat that the dictionary has
//! never seen whole still goes as one codeword and a small length.
//!
//! The dictionary is three parts rather than one (6.2.1): a root array of 256
//! entries, a node-tree of string-segments, and the history itself, which is
//! every character that has been through the encoder since the last
//! reinitialisation. Nodes point into the history rather than holding
//! characters, which is what makes a segment of any length cost the same.
//!
//! Two things it does not do that V.42bis did: there is no node recovery, and
//! no dictionary reset short of starting again. When the node-tree or the
//! history fills, the whole dictionary is reinitialised (7.11.3, 7.11.4).

pub mod length;

/// N5 (Table 11): "Number of control codes & first available codeword".
///
/// Both at once, and that is what makes a code prefix of "1" unambiguous: a
/// value below this is one of the control codes in Table 8, and anything from
/// here up is a codeword.
pub const N5: u16 = 4;

/// Table 8: the control codes, sent in compressed mode at the current codeword
/// size.
pub mod control {
    pub const ETM: u16 = 0;
    pub const FLUSH: u16 = 1;
    pub const STEPUP: u16 = 2;
    pub const REINIT: u16 = 3;
}

/// Table 9: the commands, sent in transparent mode after the ESCAPE.
pub mod command {
    pub const ECM: u8 = 0;
    pub const EID: u8 = 1;
    pub const EPM: u8 = 2;
}

/// N3 and N4 (Table 11): eight-bit characters, so 256 of them.
pub const ALPHABET: usize = 256;

/// Table 10's defaults for the negotiable parameters.
pub const DEFAULT_N2: u16 = 1024;
pub const DEFAULT_N7: u8 = 255;

/// What this end proposes.
///
/// The defaults are the Recommendation's own, and Appendix I.1 is where the
/// argument for a larger dictionary lives. A modem call is not short of memory
/// by any modern measure, and the cost of more codewords is a wider codeword;
/// the default of 1024 keeps codewords at ten bits.
pub const OFFERED_N2: u16 = 2048;
pub const OFFERED_N7: u8 = 255;

/// The negotiated parameters for one direction (Table 10, Table 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// N2: "Maximum number of codewords (including control codes)".
    pub n2: u16,
    /// N7: "Maximum string length".
    pub n7: u8,
    /// N8: "History size". Table 10 gives the default as three times the
    /// number of codewords.
    pub n8: u16,
}

impl Default for Params {
    fn default() -> Self {
        Self::of(DEFAULT_N2, DEFAULT_N7)
    }
}

impl Params {
    /// The pair that is negotiated, with the history Table 10 pairs with them.
    pub fn of(n2: u16, n7: u8) -> Self {
        Self { n2, n7, n8: n2.saturating_mul(3) }
    }

    /// N1 (Table 11): "Maximum codeword size (in bits)", derived from N2.
    ///
    /// Enough bits to name every codeword up to N2 - 1, which is what limits
    /// how far 7.11.2's STEPUP may climb.
    pub fn max_code_bits(self) -> u32 {
        let mut bits = 6;
        while (1u32 << bits) < u32::from(self.n2) {
            bits += 1;
        }
        bits
    }

    /// 7.4: "any attempt to specify a value less than the minimum is a
    /// procedural error". Table 10 puts the floors at 256 codewords, a string
    /// length of 32, and a history of 512.
    pub fn valid(self) -> bool {
        self.n2 >= 256 && self.n7 >= 32 && self.n8 >= 512
    }

    /// 7.4: "In negotiating parameter values, the lesser value of two
    /// proposals is used."
    pub fn resolve(self, theirs: Self) -> Self {
        Self {
            n2: self.n2.min(theirs.n2),
            n7: self.n7.min(theirs.n7),
            n8: self.n8.min(theirs.n8),
        }
    }
}

/// What the decoder gave up on (7.15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// "Receipt of a STEPUP control code that would cause the value of C2 to
    /// exceed N1."
    CodewordTooLarge,
    /// "Receipt of a STEPUP control code that would cause the value of C5 to
    /// exceed 8."
    OrdinalTooLarge,
    /// "Receipt of a codeword greater than C1" (6.4.1 item 6).
    UnknownCodeword(u16),
    /// A string-extension length where none can be interpreted, or one whose
    /// codeword names no string.
    BadExtension,
    /// A command code Table 9 leaves undefined.
    ReservedCommand(u8),
}

/// Transparent or compressed operation (6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Transparent,
    Compressed,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table 11: N1 is "derived from N2T, N2R", and it has to be wide enough
    /// for the largest codeword the dictionary can hold.
    #[test]
    fn the_codeword_size_is_derived_from_the_number_of_codewords() {
        assert_eq!(Params::of(256, 255).max_code_bits(), 8);
        assert_eq!(Params::of(1024, 255).max_code_bits(), 10);
        assert_eq!(Params::of(2048, 255).max_code_bits(), 11);
        assert_eq!(Params::of(65535, 255).max_code_bits(), 16);
        // 7.5.1 starts at six bits whatever the ceiling, so a small dictionary
        // never asks for fewer.
        assert_eq!(Params::of(256, 32).max_code_bits(), 8);
    }

    /// 7.4: "the lesser value of two proposals is used".
    #[test]
    fn negotiation_takes_the_lower_of_each() {
        let mine = Params { n2: 2048, n7: 255, n8: 6144 };
        let theirs = Params { n2: 1024, n7: 64, n8: 8192 };
        let agreed = mine.resolve(theirs);
        assert_eq!(agreed, Params { n2: 1024, n7: 64, n8: 6144 });
        assert!(agreed.valid());
    }

    /// Table 10's floors, which 7.4 makes a disconnection rather than a
    /// haggle.
    #[test]
    fn a_proposal_below_the_minimum_is_not_valid() {
        assert!(Params::of(256, 32).valid());
        assert!(!Params { n2: 255, n7: 32, n8: 765 }.valid());
        assert!(!Params { n2: 1024, n7: 31, n8: 3072 }.valid());
        assert!(!Params { n2: 1024, n7: 32, n8: 511 }.valid());
    }

    /// Table 10: the default history is three times the codewords.
    #[test]
    fn the_defaults_are_the_documents_own() {
        let p = Params::default();
        assert_eq!(p.n2, 1024);
        assert_eq!(p.n7, 255);
        assert_eq!(p.n8, 3072);
    }

    /// Table 11 again: N5 is both the number of control codes and the first
    /// codeword, which is what lets one prefix bit serve for both.
    #[test]
    fn the_control_codes_sit_below_the_first_codeword() {
        assert_eq!(N5, 4);
        assert!(control::REINIT < N5);
        assert_eq!(control::ETM, 0);
        assert_eq!(control::FLUSH, 1);
        assert_eq!(control::STEPUP, 2);
        assert_eq!(control::REINIT, 3);
    }
}
