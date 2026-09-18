//! The V.92 framed sequences: everything Phase 3 and Phase 4 say to each other
//! inside a frame sync, start bits and a CRC.
//!
//! V.92 invents no framing. Every sequence here is built the way V.34 builds
//! MP and V.90 builds CP: seventeen ones, then the information in sixteen-bit
//! words each behind a zero start bit, then the CRC of 10.1.2.3.2/V.34 behind
//! one more, then fill. `v90::sequences::{frame, unframe, put, get}` already do
//! that, against real servers, so this module is only about where the fields
//! sit and what they mean.
//!
//! What V.92 *does* invent is two dispatch decisions, and both have to be made
//! before a single field is read.
//!
//! * **Bit 47 in J.** Jd (Table 21) and Jp (Table 22) are the same seventy-two
//!   bits, framed the same way, both with a good CRC. Where Jd carries the
//!   downstream rate mask, Jp carries epsilon -- the fraction of a symbol the
//!   analogue modem is to shift its transmitter by. A reader that does not look
//!   at bit 47 first reads a Jp as a Jd full of nonsense rates. [`J`] looks.
//! * **Bit 18, then the type at 19:20.** Bit 18 tells the CP family (0) from
//!   the SUV family (1), and for a CP the type says which table: 0 = CPt,
//!   1 = CPu, 2 = CPus. CPd has no type there at all -- its bits 19:21 are the
//!   flags saying which of its three optional parts are present -- so the
//!   *direction* decides which reading applies, and that is why the upstream
//!   family ([`CpFamily`]) and the downstream pair ([`DownFamily`]) are two
//!   enumerations and not one.
//!
//! CPd (Table 30) is the only sequence whose length is not known in advance.
//! Its modulus, filter and constellation parts are each present or wholly
//! absent, everything after an absent part moves up, and the lengths inside the
//! parts are themselves carried in the parts. So it is read by walking word by
//! word -- never by an absolute bit offset past bit 50 -- and the finders take
//! a length callback that says "not yet" until the walk can finish.
//!
//! The upstream sequences fill to a multiple of twelve symbols and the
//! downstream ones to six, which in bits is whatever the modulation carrying
//! them puts in a symbol: twenty-four or thirty-six on TRN2u, K upstream in
//! data mode, D downstream.

use crate::v90::sequences::{
    BLOCK, CP_MARK, CP_TYPE_CPU, Cp, CpFinder, JD_BITS, Jd, J_IDENTIFIER, Layout, SYNC_ONES, frame, get, is_jd, is_jp,
    put, unframe,
};

use super::{
    CONSTELLATION_FRAME, CONSTELLATION_POINTS, CONSTELLATION_SETS, FilterLimits, Filters, Parameters, Trellis,
    UP_INTERVALS, UP_RATES, four_g_from_gain, gain_from_4g, signed_q, to_signed_q,
};

// ---------------------------------------------------------------------------
// The frame, shared by every sequence here (P4A 0.4, P4D 2.3)
// ---------------------------------------------------------------------------

/// One word: the start bit and the sixteen information bits behind it. Start
/// bits therefore fall at bit 17, 34, 51, ... -- every multiple of seventeen --
/// in every table of clause 8, including the variable-length parts of CPd,
/// because alpha and beta are multiples of seventeen too (8.8.3).
const WORD: usize = BLOCK + 1;

/// Where word `w` begins, counting from the frame sync.
fn word_at(w: usize) -> usize {
    SYNC_ONES + w * WORD
}

/// The sixteen information bits of word `w`, or `None` if `bits` does not
/// reach them.
///
/// The start bit is not checked here: [`unframe`] checks every one of them
/// when the sequence is read, and this is also used to work out how long a
/// sequence is going to be, where refusing early would only mean refusing
/// twice.
fn word_value(bits: &[bool], w: usize) -> Option<u32> {
    let at = word_at(w);
    (bits.len() >= at + WORD).then(|| get(bits, at + 1, BLOCK))
}

/// How long a sequence of `words` information words is before its fill: the
/// sync, the words, the CRC word and "Fill bit: 0" (Tables 21 to 31).
pub fn sequence_bits(words: usize) -> usize {
    word_at(words + 1) + 1
}

/// The length of a short information sequence before its fill: SUVu
/// (Table 27), SUVd (Table 31) and CPus (Table 24) each carry one information
/// word, so all three are fifty-two bits.
pub const SHORT_BITS: usize = 52;

/// "Fill bits: 0s to extend the sequence length to the next multiple of 12
/// symbols" upstream and "of 6 symbols" downstream, where `unit` is what those
/// symbols carry in bits.
///
/// A unit of zero would leave a sequence unpadded and cost the far end its
/// frame boundary with no complaint, so it is an assertion rather than a silent
/// shrug; the `max` is only so that a shipped build limps instead of dividing
/// by zero.
fn pad_to(bits: &mut Vec<bool>, unit: usize) {
    debug_assert!(unit > 0, "a V.92 sequence fills to a whole number of symbols, not to none");
    let unit = unit.max(1);
    bits.resize(bits.len().div_ceil(unit) * unit, false);
}

/// What twelve upstream symbols carry on four-point TRN2u: two bits a symbol
/// (Table 28, whose four rows are labelled `00` to `11`).
pub const TRN2U_FOUR_BITS: usize = 2 * UP_INTERVALS;

/// And on eight-point TRN2u: three bits a symbol (Table 29).
pub const TRN2U_EIGHT_BITS: usize = 3 * UP_INTERVALS;

/// The fill unit of an upstream sequence riding TRN2u modulation, which Jp bit
/// 48 chooses for training and bit 49 for a rate renegotiation (Table 22).
///
/// The other upstream unit is data mode's: in a fast parameter exchange CPu,
/// CPus and SUVu ride "the same modulation parameters as data mode" (8.7.3,
/// 8.7.5), so the unit is K = `v92::up_bits(drn)`, the bits a twelve-symbol
/// data frame carries. Downstream the unit is D, the bits in a six-symbol
/// frame: `v90::sequences::training_bits` in Phase 4 and `data_bits` in data
/// mode. Each is passed to `to_bits` as it stands.
pub fn trn2u_unit(eight_point: bool) -> usize {
    if eight_point { TRN2U_EIGHT_BITS } else { TRN2U_FOUR_BITS }
}

/// A short information sequence: one information word, the CRC over it, and
/// fill (Tables 24, 27 and 31).
fn short_sequence(word0: u32, unit: usize) -> Vec<bool> {
    let mut information = Vec::with_capacity(BLOCK);
    put(&mut information, word0, BLOCK);
    let mut bits = frame(&information);
    bits.push(false); // "Fill bit: 0"
    pad_to(&mut bits, unit);
    bits
}

/// The one information word of a short sequence, if the frame and the CRC are
/// what they should be.
fn short_word(bits: &[bool]) -> Option<u32> {
    unframe(bits, 1).map(|information| get(&information, 0, BLOCK))
}

// ---------------------------------------------------------------------------
// Jd and Jp (8.6.2, 8.6.3, 8.6.4; Tables 21 and 22)
// ---------------------------------------------------------------------------

/// Jp', "12 binary zeroes" that terminate Jp (8.6.4), sent as the sign of the
/// UINFO codeword like Jp itself and seeded from Jp's final symbol.
///
/// It is the same length as V.90's J'd, which V.92 Phase 3 no longer sends:
/// there Jd runs into Jp and Jp' closes the pair (P3S 5.4). Twelve downstream
/// symbols are two data frames.
pub const JP_PRIME_BITS: usize = 12;

/// Jp (Table 22): the sampling phase the digital modem wants the analogue
/// modem's transmitter to shift to, and the size of the upstream training
/// constellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Jp {
    /// Bits 18:33, "the fractional amount that signal S-bar-u corresponding to
    /// the Jp to Jp' transition needs to be extended. 16-bit unsigned integer
    /// covering the range [0, 1) symbol or [0, T) seconds".
    ///
    /// The sixteen bits travel here as they are; what a code is worth in
    /// symbols belongs to `v92::epsilon`, which owns that reading.
    pub epsilon: u16,
    /// Bit 48: "size of constellation used to transmit CPu, E2u, SUVu and
    /// TRN2u during training sequences: 0 = 4-point, 1 = 8-point".
    pub eight_in_training: bool,
    /// Bit 49: the same "during rate renegotiation procedures".
    pub eight_in_renegotiation: bool,
}

impl Jp {
    /// The seventy-two bits of one Jp repetition, ending in "Fill bits: 0000".
    ///
    /// Bits 35:46 and bit 50 go out as the zeros Table 22 asks for, and bit 47
    /// as the one that marks a Jp.
    pub fn to_bits(&self) -> Vec<bool> {
        let mut information = Vec::with_capacity(2 * BLOCK);
        put(&mut information, u32::from(self.epsilon), BLOCK);
        put(&mut information, 0, 12); // 35:46 "Reserved for the ITU"
        information.push(true); // 47: "Jd/Jp identifier: 1 = Jp"
        information.push(self.eight_in_training); // 48
        information.push(self.eight_in_renegotiation); // 49
        information.push(false); // 50 "Reserved for the ITU"
        let mut bits = frame(&information);
        bits.resize(JD_BITS, false);
        bits
    }

    /// The Jp `bits` hold, or `None` if bit 47 says they are a Jd or the CRC
    /// does not check.
    ///
    /// Bits 35:46 and 50 are ignored rather than refused: Table 22 has the
    /// digital modem send zeros there and the analogue modem not interpret
    /// them, so a far end using a later ITU extension still connects.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if !is_jp(bits) {
            return None;
        }
        let information = unframe(bits, 2)?;
        Some(Self {
            epsilon: get(&information, 0, BLOCK) as u16,
            eight_in_training: information[29],
            eight_in_renegotiation: information[30],
        })
    }
}

/// The digital modem's Phase 3 information sequence, whichever of the two it
/// is (8.6.2, 8.6.3).
///
/// Bit 47 is read before anything else, which is the whole point of this type:
/// a V.90 reader takes bit 47 for "sixteen points in training" and would hand
/// back a Jd whose rate mask is really an epsilon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum J {
    /// Bit 47 clear: Table 21, the downstream rate mask.
    Jd(Jd),
    /// Bit 47 set: Table 22, epsilon and the TRN2u sizes.
    Jp(Jp),
}

impl J {
    /// The Jd a V.92 digital modem sends: Table 21 keeps V.90's rate mask and
    /// look-ahead where they were, but bit 47 is now the identifier and bit 48
    /// is reserved, so both of V.90's constellation flags go out clear. The
    /// choice they used to make is Jp bits 48 and 49, over four or eight points
    /// rather than four or sixteen.
    pub fn jd(rates: u32, lookahead: u8) -> Self {
        Self::Jd(Jd { rates, sixteen_in_training: false, sixteen_in_renegotiation: false, lookahead })
    }

    /// Whichever of the two `bits` hold, if the CRC checks.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if bits.len() <= J_IDENTIFIER {
            return None;
        }
        if bits[J_IDENTIFIER] {
            Jp::from_bits(bits).map(Self::Jp)
        } else {
            Jd::from_bits_if(bits, is_jd).map(Self::Jd)
        }
    }

    /// The seventy-two bits of one repetition.
    pub fn to_bits(&self) -> Vec<bool> {
        match self {
            Self::Jd(jd) => jd.to_bits(),
            Self::Jp(jp) => jp.to_bits(),
        }
    }
}

// ---------------------------------------------------------------------------
// The SUV family (8.7.5, Table 27; 8.8.5, Table 31)
// ---------------------------------------------------------------------------

/// Table 27 bit 18 and Table 31 bit 18: "SUVu: 1", "SUVd: 1", against the
/// CP family's 0 in the same place.
pub const SUV_MARK: bool = true;

/// Table 24 bits 19:20, "CPus: 2", the third type of the CP family. V.90's
/// sequences module owns the other two, 0 = CPt and 1 = CPu.
pub const CP_TYPE_CPUS: u32 = 2;

/// SUVu bits 27:31 are "signed Q2.2 format (sxx.xx)", five bits with two after
/// the point.
pub const LEVEL_WIDTH: u32 = 5;
/// And two of those five are the fraction, so the step is 0.25 dB.
pub const LEVEL_FRACTION: u32 = 2;

/// "The value 16 (-4.00) indicates that no measurement has been taken"
/// (Table 27), which is why a measurement is clamped to [`LEVEL_LARGEST`]
/// either way: -4.00 is not a level, it is the absence of one.
pub const LEVEL_NONE: u32 = 16;

/// The largest level SUVu can report, and the magnitude a measurement is
/// clamped to: the field runs -4.00 to +3.75 and -4.00 is spoken for
/// (P4A A8).
pub const LEVEL_LARGEST: f64 = 3.75;

/// SUVu (Table 27): the analogue modem's "I am ready and listening", with what
/// it has measured of its own transmit level.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Suvu {
    /// Bit 26: "the analogue modem wishes the digital modem to wait for a CPu
    /// before sending a CPd. The digital modem is not required to comply".
    pub wait_for_cpu: bool,
    /// Bits 27:31: "20 x log10(L) where L is the measured RMS level of the
    /// prefilter output multiplied by G", in dB. `None` is the printed 16,
    /// "no measurement has been taken", which is what initial training sends.
    pub level: Option<f64>,
    /// Bit 32: "a silent period is requested. This may be used during rate
    /// renegotiation".
    pub silence: bool,
    /// Bit 33: "received CPd from the digital modem". Set, this is SUVu'.
    pub ack: bool,
}

impl Suvu {
    /// The information word, bits 18:33.
    fn word(&self) -> u32 {
        let level = match self.level {
            None => LEVEL_NONE,
            Some(level) => to_signed_q(level.clamp(-LEVEL_LARGEST, LEVEL_LARGEST), LEVEL_WIDTH, LEVEL_FRACTION),
        };
        // Bits 19:25 go out as the zeros Table 27 asks for.
        1 | u32::from(self.wait_for_cpu) << 8 | level << 9 | u32::from(self.silence) << 14 | u32::from(self.ack) << 15
    }

    /// The sequence, filled to a multiple of twelve symbols: `unit` is what
    /// twelve symbols carry, from [`trn2u_unit`] in training and a rate
    /// renegotiation, or K in a fast parameter exchange.
    pub fn to_bits(&self, unit: usize) -> Vec<bool> {
        short_sequence(self.word(), unit)
    }

    /// The SUVu `bits` hold. Bits 19:25 are ignored, not refused.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let word = short_word(bits)?;
        if word & 1 == 0 {
            return None; // bit 18 clear: a CP sequence, not an SUV
        }
        let level = word >> 9 & ((1 << LEVEL_WIDTH) - 1);
        Some(Self {
            wait_for_cpu: word >> 8 & 1 == 1,
            level: (level != LEVEL_NONE).then(|| signed_q(level, LEVEL_WIDTH, LEVEL_FRACTION)),
            silence: word >> 14 & 1 == 1,
            ack: word >> 15 & 1 == 1,
        })
    }
}

/// SUVd (Table 31): the digital modem's half of the same handshake.
///
/// It has no level field and no drn -- which is why 9.11's "drn = 0 in SUVu or
/// SUVd" cannot mean what it says; the cleardown field is CPd bits 22:26 and
/// CPu bits 21:25 (P4A E2, MOH Q1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Suvd {
    /// Bit 32: "a silent period is requested. This may be used during rate
    /// renegotiation".
    pub silence: bool,
    /// Bit 33: "received CPu from the analogue modem". Set, this is SUVd'.
    pub ack: bool,
}

impl Suvd {
    /// The information word, bits 18:33. Bits 19:31 go out as zeros.
    fn word(&self) -> u32 {
        1 | u32::from(self.silence) << 14 | u32::from(self.ack) << 15
    }

    /// The sequence, filled to a multiple of six symbols: `unit` is D, what a
    /// downstream data frame carries.
    pub fn to_bits(&self, unit: usize) -> Vec<bool> {
        short_sequence(self.word(), unit)
    }

    /// The SUVd `bits` hold. Bits 19:31 are ignored, not refused.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let word = short_word(bits)?;
        (word & 1 == 1).then_some(Self { silence: word >> 14 & 1 == 1, ack: word >> 15 & 1 == 1 })
    }
}

// ---------------------------------------------------------------------------
// CPus (8.7.3, Table 24)
// ---------------------------------------------------------------------------

/// CPus (Table 24): a CPu carrying nothing but a rate and an acknowledge, for
/// a rate renegotiation or a fast parameter exchange "when the digital modem's
/// modulation parameters are not changed".
///
/// It carries no Sr and no ld, so the previous CPu's stay in force (P4A A9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cpus {
    /// Bits 21:25: "selected digital modem to analogue modem data signalling
    /// rate, an integer, drn, between 0 and 22. drn = 0 indicates cleardown."
    pub drn: u8,
    /// Bit 33: "received CPd from the digital modem".
    pub ack: bool,
}

impl Cpus {
    /// The information word, bits 18:33. Bit 18 is the CP family's 0, bits
    /// 19:20 the type 2, and bits 26:32 the zeros Table 24 asks for.
    fn word(&self) -> u32 {
        CP_TYPE_CPUS << 1 | u32::from(self.drn & 0x1f) << 3 | u32::from(self.ack) << 15
    }

    /// The sequence, filled to a multiple of twelve symbols.
    pub fn to_bits(&self, unit: usize) -> Vec<bool> {
        short_sequence(self.word(), unit)
    }

    /// The CPus `bits` hold, or `None` if bit 18 makes them an SUV or the type
    /// is not 2. Bits 26:32 are ignored, not refused.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let word = short_word(bits)?;
        if word & 1 == 1 || word >> 1 & 3 != CP_TYPE_CPUS {
            return None;
        }
        Some(Self { drn: (word >> 3 & 0x1f) as u8, ack: word >> 15 & 1 == 1 })
    }
}

// ---------------------------------------------------------------------------
// CPd (8.8.3, Table 30)
// ---------------------------------------------------------------------------

/// The modulus encoder part is "transmitted using 6 words" (8.8.3), two of the
/// twelve moduli to a word.
const MODULUS_WORDS: usize = UP_INTERVALS / 2;

/// The filter part opens with four words of lengths -- LZ1, LP1, LZ2, LP2,
/// each "up to Lmax" in the low nine bits with seven reserved above -- and then
/// one word per coefficient: "4 + LZ1 + LP1 + LZ2 + LP2 words" (8.8.3).
const FILTER_LENGTH_WORDS: usize = 4;

/// Each length field is nine bits (Table 30, 154:162 and its three fellows).
const LENGTH_BITS: usize = 9;

/// The constellation part opens with five words -- two of indices, three of the
/// six LCs -- and then one word per point: "5 + LC1 + ... + LC6 words".
const SET_HEADER_WORDS: usize = 5;

/// z1 and z2 are "signed Q0.15 (s.xxxxxxxxxxxxxxx)" (Table 30), sixteen bits
/// with fifteen after the point.
pub const Z_FRACTION: u32 = 15;

/// p1 and p2 are "signed Q1.14 (sx.xxxxxxxxxxxxxx)", sixteen bits with
/// fourteen after the point.
pub const P_FRACTION: u32 = 14;

/// Every coefficient and every constellation point is one sixteen-bit word.
pub const COEFFICIENT_WIDTH: u32 = 16;

/// The most positive points this end puts in a constellation set it sends.
///
/// "The number of points in a constellation set shall not exceed 128" (8.8.3),
/// and 6.4.2 calls the set size N = 2 x LC while Table 30 calls LC "the number
/// of positive points". The two readings differ by a factor of two, so this end
/// sends 2 x LC <= 128, which satisfies both, and accepts up to
/// [`CONSTELLATION_POINTS`] on reception, which is the looser one (P4D Q3).
pub const SET_POINTS_SENT: usize = CONSTELLATION_POINTS / 2;

/// The longest CPd there can be: Ltot's 384 coefficients (INFO1a bits 14:15)
/// and six sets of [`CONSTELLATION_POINTS`] points, which is 1169 words and
/// 19 908 bits.
///
/// The nine- and eight-bit length fields would let a corrupt header ask for
/// three times that, so a candidate asking for more than this is dropped rather
/// than waited on for seconds of bits that cannot pass the CRC.
pub const CPD_WORDS_MOST: usize =
    2 + MODULUS_WORDS + FILTER_LENGTH_WORDS + 384 + SET_HEADER_WORDS + CONSTELLATION_SETS * CONSTELLATION_POINTS;

/// The constellation sets a CPd carries, and which of them each interval uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sets {
    /// Bits 222+alpha onward: "an integer between 0 and 5 denoting the index of
    /// the constellation to be used in data frame intervals 0 and 6", and so on
    /// for the other five pairs. Index j serves intervals j and j + 6, because
    /// the constellation frame is six symbols and the data frame twelve.
    pub index: [u8; CONSTELLATION_FRAME],
    /// The sets themselves, each the positive magnitudes of one set "in
    /// increasing magnitude", the (v+1)-th set at index v.
    ///
    /// A set of LC points is N = 2 x LC levels: see [`set_levels`] for the
    /// reading of what one of these words is worth.
    pub points: Vec<Vec<u16>>,
}

/// The levels a CPd constellation set names, in index order -- eta from -N/2 to
/// N/2 - 1, so the largest negative magnitude first and the largest positive
/// last.
///
/// This is the reading of Table 30's "linear value of the 1st (smallest
/// magnitude) constellation point" fixed by the plan: the sixteen bits are an
/// **unsigned** magnitude on the linear scale of Table 1/V.90, which runs to
/// 32 124 in mu-law and 32 256 in A-law, and the negative half of the
/// constellation mirrors the positive half, a(-eta-1) = -a(eta) (6.4.2). The
/// alternative is a signed sixteen-bit value, under which a point above 32 767
/// would be a negative level and the mirror would already be on the wire; the
/// Recommendation gives the field no format at all (P4D Q1, INTRO Q-4). Only
/// the ratio between the points, G and the coefficients matters to the analogue
/// modem, so a capture that disagrees changes this one function.
pub fn set_levels(points: &[u16]) -> Vec<i32> {
    let negative = points.iter().rev().map(|&point| -i32::from(point));
    negative.chain(points.iter().map(|&point| i32::from(point))).collect()
}

/// CPd (Table 30): the modulation parameters the analogue modem is to transmit
/// with, which is the whole of the 6.4 chain.
///
/// Three of its four parts are optional. An absent part means "keep what the
/// last CPd said" ([`Cpd::merged_over`]); the first CPd of a training has no
/// previous values to keep, so it must carry every part (P4D Q2, and section 4
/// of the plan).
#[derive(Debug, Clone, PartialEq)]
pub struct Cpd {
    /// Bits 22:26: "selected analogue modem to digital modem data signalling
    /// rate, an integer, drn, between 0 and 19. drn = 0 shall indicate
    /// cleardown."
    pub drn: u8,
    /// Bits 27:28: which trellis encoder "the digital modem receiver requires
    /// the analogue modem transmitter to use".
    pub trellis: Trellis,
    /// Bit 29: "extend the length of the E2u sequence ... by 1 symbol". It
    /// "shall be set to zero during rate renegotiation and fast parameter
    /// exchange procedures".
    pub extend_e2u: bool,
    /// Bit 33: "received CPu from the analogue modem". Set, this is CPd'.
    pub ack: bool,
    /// Bits 35:50: "4 x G > 0", four times the gain at the prefilter output, in
    /// unsigned Q0.16. It is kept as the field rather than as G so that a CPd
    /// read off the wire and written back out is the same CPd;
    /// `v92::gain_from_4g` turns it into a number.
    pub gain4: u16,
    /// The modulus encoder part, present if bit 19 is set: M0 to M11, "the
    /// modulus encoder parameter" for each of the twelve data frame intervals.
    pub moduli: Option<[u8; UP_INTERVALS]>,
    /// The precoder and prefilter part, present if bit 20 is set.
    pub filters: Option<Filters>,
    /// The constellation part, present if bit 21 is set.
    pub sets: Option<Sets>,
}

impl Cpd {
    /// A cleardown: "drn = 0 shall indicate cleardown" (Table 30), and such a
    /// CPd "needs no optional parts" because there is no data mode left to give
    /// parameters to (9.11, P4D DC-7).
    ///
    /// 9.11 puts the field in "SUVu or SUVd", which have no drn at all. It
    /// means this one and CPu's bits 21:25 (P4A E2, MOH Q1).
    pub fn cleardown(ack: bool) -> Self {
        Self {
            drn: 0,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            ack,
            // "4 x G > 0", so even a sequence that settles nothing carries the
            // smallest step rather than the zero Table 30 forbids.
            gain4: 1,
            moduli: None,
            filters: None,
            sets: None,
        }
    }

    /// Whether every optional part is here, which the first CPd of a training
    /// must be: there is nothing for an absent part to fall back on, and a CPd
    /// that leaves one out there is a retrain rather than a guess.
    pub fn complete(&self) -> bool {
        self.moduli.is_some() && self.filters.is_some() && self.sets.is_some()
    }

    /// This CPd with each absent part taken from `previous`.
    ///
    /// "All the bits contained in a part are removed from the CPd sequence when
    /// it is indicated that the part is not present" (8.8.3) is all the
    /// Recommendation says; that an absent part means "unchanged" is the
    /// reading fixed in section 4 of the plan, and it is the only one under
    /// which a rate renegotiation that changes the rate alone can leave the
    /// other three parts out.
    pub fn merged_over(&self, previous: &Self) -> Self {
        Self {
            moduli: self.moduli.or(previous.moduli),
            filters: self.filters.clone().or_else(|| previous.filters.clone()),
            sets: self.sets.clone().or_else(|| previous.sets.clone()),
            ..self.clone()
        }
    }

    /// The information word 0, bits 18:33. Bits 30:32 go out as zeros.
    fn word0(&self) -> u32 {
        u32::from(self.moduli.is_some()) << 1
            | u32::from(self.filters.is_some()) << 2
            | u32::from(self.sets.is_some()) << 3
            | u32::from(self.drn & 0x1f) << 4
            | u32::from(self.trellis.code()) << 9
            | u32::from(self.extend_e2u) << 11
            | u32::from(self.ack) << 15
    }

    /// The sequence, filled to a multiple of six symbols: `unit` is D, the bits
    /// a downstream data frame carries in the modulation carrying this CPd.
    ///
    /// Everything is written in order, part by part, so the parts that are
    /// absent simply do not appear and everything after them moves up. No
    /// position past bit 50 is ever computed.
    pub fn to_bits(&self, unit: usize) -> Vec<bool> {
        let mut information = Vec::new();
        put(&mut information, self.word0(), BLOCK);
        put(&mut information, u32::from(self.gain4), BLOCK);
        if let Some(moduli) = &self.moduli {
            // Two moduli to a word, the lower-numbered in the low byte.
            for pair in moduli.chunks(2) {
                put(&mut information, u32::from(pair[0]), 8);
                put(&mut information, u32::from(pair[1]), 8);
            }
        }
        if let Some(filters) = &self.filters {
            let sections = [
                (&filters.z1, Z_FRACTION),
                (&filters.p1, P_FRACTION),
                (&filters.z2, Z_FRACTION),
                (&filters.p2, P_FRACTION),
            ];
            // Nine bits to a length, seven reserved above it. A section longer
            // than nine bits can count is what [`Cpd::check`] refuses; here it
            // is cut to what the field can name, so that the lengths and the
            // coefficients after them always agree.
            let lengths = sections.map(|(section, _)| section.len().min((1 << LENGTH_BITS) - 1));
            for length in lengths {
                put(&mut information, length as u32, LENGTH_BITS);
                put(&mut information, 0, BLOCK - LENGTH_BITS); // reserved for the ITU
            }
            for ((section, fraction), length) in sections.into_iter().zip(lengths) {
                for &coefficient in &section[..length] {
                    put(&mut information, to_signed_q(coefficient, COEFFICIENT_WIDTH, fraction), BLOCK);
                }
            }
        }
        if let Some(sets) = &self.sets {
            let index = |j: usize| u32::from(sets.index[j] & 0xf);
            put(&mut information, index(0) | index(1) << 4 | index(2) << 8 | index(3) << 12, BLOCK);
            // Then intervals 4 and 10, 5 and 11, and eight reserved bits.
            put(&mut information, index(4) | index(5) << 4, BLOCK);
            // LC1 to LC6, two to a word, zero for a set that is not there.
            // Table 30 has room for six sets and eight bits to count each; a
            // CPd with more is what [`Cpd::check`] refuses, and what goes out
            // here is always the sets the lengths name and no others.
            let mut lengths = [0usize; CONSTELLATION_SETS];
            for (length, points) in lengths.iter_mut().zip(&sets.points) {
                *length = points.len().min(usize::from(u8::MAX));
            }
            for pair in lengths.chunks(2) {
                put(&mut information, (pair[0] | pair[1] << 8) as u32, BLOCK);
            }
            for (points, length) in sets.points.iter().zip(lengths) {
                for &point in &points[..length] {
                    put(&mut information, u32::from(point), BLOCK);
                }
            }
        }
        let mut bits = frame(&information);
        bits.push(false); // "Fill bit: 0"
        pad_to(&mut bits, unit);
        bits
    }

    /// The CPd `bits` hold, walked part by part the way P4D 5.3 walks it.
    ///
    /// Bit 18 set means an SUVd and is refused, and so is a trellis field of 3:
    /// that code is "reserved for the ITU" and names no encoder, and every other
    /// field of the sequence tells the analogue modem how to transmit with an
    /// encoder it has not been given. Reserved *bits* go the other way and are
    /// ignored, because there the risk is refusing a conforming far end.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let Length::Known(words) = cpd_words(bits) else {
            return None;
        };
        if bits[CP_MARK] {
            return None; // "SUVd: 1" in the same place
        }
        let information = unframe(bits, words)?;
        let word0 = get(&information, 0, BLOCK);
        let mut at = 2 * BLOCK;
        let moduli = (word0 >> 1 & 1 == 1).then(|| {
            let moduli = std::array::from_fn(|i| get(&information, at + 8 * i, 8) as u8);
            at += MODULUS_WORDS * BLOCK;
            moduli
        });
        let filters = (word0 >> 2 & 1 == 1).then(|| {
            let lengths: [usize; FILTER_LENGTH_WORDS] =
                std::array::from_fn(|k| get(&information, at + k * BLOCK, LENGTH_BITS) as usize);
            at += FILTER_LENGTH_WORDS * BLOCK;
            let mut section = |length: usize, fraction: u32| {
                let taps = (0..length)
                    .map(|k| signed_q(get(&information, at + k * BLOCK, BLOCK), COEFFICIENT_WIDTH, fraction))
                    .collect();
                at += length * BLOCK;
                taps
            };
            Filters {
                z1: section(lengths[0], Z_FRACTION),
                p1: section(lengths[1], P_FRACTION),
                z2: section(lengths[2], Z_FRACTION),
                p2: section(lengths[3], P_FRACTION),
            }
        });
        let sets = (word0 >> 3 & 1 == 1).then(|| {
            let index = std::array::from_fn(|j| {
                let (word, nibble) = if j < 4 { (0, j) } else { (1, j - 4) };
                get(&information, at + word * BLOCK + 4 * nibble, 4) as u8
            });
            at += 2 * BLOCK;
            let lengths: [usize; CONSTELLATION_SETS] =
                std::array::from_fn(|k| get(&information, at + (k / 2) * BLOCK + (k % 2) * 8, 8) as usize);
            at += (SET_HEADER_WORDS - 2) * BLOCK;
            let mut points = Vec::new();
            for length in lengths {
                points.push((0..length).map(|k| get(&information, at + k * BLOCK, BLOCK) as u16).collect::<Vec<_>>());
                at += length * BLOCK;
            }
            // A set of zero size carries no words at all. Trailing ones are
            // dropped so that a CPd read back is the CPd that was written;
            // one in the middle is kept, so that index v still selects the
            // (v+1)-th set, and `check` reports it for what it is.
            while points.last().is_some_and(Vec::is_empty) {
                points.pop();
            }
            Sets { index, points }
        });
        Some(Self {
            drn: (word0 >> 4 & 0x1f) as u8,
            trellis: Trellis::from_code((word0 >> 9 & 3) as u8)?,
            extend_e2u: word0 >> 11 & 1 == 1,
            ack: word0 >> 15 & 1 == 1,
            gain4: get(&information, BLOCK, BLOCK) as u16,
            moduli,
            filters,
            sets,
        })
    }

    /// Whether these parameters are ones the analogue modem can be asked for.
    ///
    /// `limits` is what INFO1a announced (Table 18 bits 12:17) and `up_bits` is
    /// K, the bits a data frame must carry -- normally `v92::up_bits(self.drn)`,
    /// passed in because a caller that has merged a partial CPd over an earlier
    /// one knows which rate is really in force. Zero skips the rate check.
    ///
    /// The checks are the 8.8.3 SHALLs -- no zero point, non-zero sets listed
    /// first, Ltot, Lmax, the sections INFO1a offered, 128 points to a set --
    /// and the constraints 8.8 leaves to other clauses: 2^K <= product of the
    /// moduli (6.4.1), and an equivalence class with a member for every Ki,
    /// which needs N >= Mi and, in the interval where k = 3, N >= 2 x Mi
    /// (6.4.2). Every one of them past the wire's own limits is
    /// `Parameters::fits`, so a rule has one home whether it arrived as a CPd or
    /// was chosen by the design.
    ///
    /// A partial CPd is merged over the previous one before it is checked; on
    /// its own it has no parameters to check.
    pub fn check(&self, limits: &FilterLimits, up_bits: u32) -> Result<(), &'static str> {
        if usize::from(self.drn) > UP_RATES {
            return Err("the upstream rate is not a rung of the ladder");
        }
        if self.gain4 == 0 {
            return Err("the gain is zero, which Table 30 forbids");
        }
        if let Some(filters) = &self.filters {
            filters.fits(limits)?;
        }
        if self.drn == 0 {
            // A cleardown settles no modulation, so there is nothing else to
            // hold it to (9.11).
            return Ok(());
        }
        let parameters = Parameters::try_from(self)?;
        parameters.fits()?;
        if up_bits > 0 && (u128::from(up_bits) >= 128 || (1u128 << up_bits) > parameters.product()) {
            return Err("the data frame carries more bits than the moduli can hold");
        }
        Ok(())
    }
}

impl From<&Parameters> for Cpd {
    /// The CPd that says what these parameters say. The acknowledge bit is not
    /// a parameter: the SUV/CP/E exchange sets it as it goes.
    fn from(parameters: &Parameters) -> Self {
        Self {
            drn: parameters.drn,
            trellis: parameters.trellis,
            extend_e2u: parameters.extend_e2u,
            ack: false,
            gain4: four_g_from_gain(parameters.gain),
            moduli: Some(parameters.moduli),
            filters: Some(parameters.filters.clone()),
            sets: Some(Sets { index: parameters.indices, points: parameters.sets.clone() }),
        }
    }
}

impl TryFrom<&Cpd> for Parameters {
    type Error = &'static str;

    /// What a CPd means, once every part is there. A CPd that leaves one out
    /// is merged over the previous one first ([`Cpd::merged_over`]).
    fn try_from(cpd: &Cpd) -> Result<Self, Self::Error> {
        let moduli = cpd.moduli.ok_or("the CPd carries no modulus encoder parameters")?;
        let filters = cpd.filters.clone().ok_or("the CPd carries no precoder or prefilter coefficients")?;
        let sets = cpd.sets.clone().ok_or("the CPd carries no constellation sets")?;
        Ok(Self {
            drn: cpd.drn,
            trellis: cpd.trellis,
            extend_e2u: cpd.extend_e2u,
            gain: gain_from_4g(cpd.gain4),
            moduli,
            filters,
            sets: sets.points,
            indices: sets.index,
        })
    }
}

// ---------------------------------------------------------------------------
// RM and RM' (8.7.4, Tables 25 and 26)
// ---------------------------------------------------------------------------

/// RM's pattern repeats every four data frame intervals: two at Mi - 1, then
/// two at 0 (Tables 25 and 26). Four is also the trellis frame, but that is a
/// coincidence: the tables are printed interval by interval.
pub const RM_PERIOD: usize = 4;

/// The modulus encoder output Ki that RM forces in data frame interval `i`,
/// with `m` the modulus in force there, or RM' when `prime` is set.
///
/// Tables 25 and 26 print two intervals at Mi - 1 and two at 0, in a pattern of
/// period four, RM' being RM shifted by two. Row 11 of Table 25 reads "u11 = 0"
/// and row 11 of Table 26 "k11 = M11 - 1"; both are lowercase slips for K11,
/// and the pattern's own period settles what they mean (P4A E1).
///
/// A modulus of zero is no modulus -- `Parameters::fits` refuses one -- so the
/// subtraction saturates rather than wrapping to 255.
pub fn rm_k(i: usize, m: u8, prime: bool) -> u32 {
    let high = (i % RM_PERIOD < RM_PERIOD / 2) != prime;
    if high { u32::from(m).saturating_sub(1) } else { 0 }
}

// ---------------------------------------------------------------------------
// Telling the sequences apart, and finding them in a bit stream
// ---------------------------------------------------------------------------

/// What an upstream framed sequence turned out to be (P4A 3.9): bit 18 tells
/// the CP family from the SUV family, and the type at 19:20 tells the three
/// CPs apart.
///
/// There is no `Eq`, because SUVu's level is a measurement in decibels.
#[derive(Debug, Clone, PartialEq)]
pub enum CpFamily {
    /// Type 0, Table 23 read for Phase 3: the training constellations.
    Cpt(Cp),
    /// Type 1, the same table read for data mode.
    Cpu(Cp),
    /// Type 2, Table 24.
    Cpus(Cpus),
    /// Bit 18 set, Table 27.
    Suvu(Suvu),
}

impl CpFamily {
    /// Whichever of the four `bits` hold, if the CRC checks. A type of 3 is
    /// unassigned and is refused, which is the one place a reserved value stops
    /// a sequence being read: it is a dispatch field, not a spare bit.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if bits.len() <= CP_MARK + 2 {
            return None;
        }
        if bits[CP_MARK] == SUV_MARK {
            return Suvu::from_bits(bits).map(Self::Suvu);
        }
        match get(bits, CP_MARK + 1, 2) {
            CP_TYPE_CPUS => Cpus::from_bits(bits).map(Self::Cpus),
            type_bits if type_bits <= CP_TYPE_CPU => Cp::from_bits_in(Layout::V92, bits).map(|cp| {
                if cp.data_mode { Self::Cpu(cp) } else { Self::Cpt(cp) }
            }),
            _ => None,
        }
    }
}

/// And the downstream pair, told apart by the same bit 18 (Tables 30 and 31).
///
/// CPd's bits 19:21 are its part flags, not a type, so nothing but the
/// direction tells a CPd from a CPu: a receiver knows which way it is looking.
#[derive(Debug, Clone, PartialEq)]
pub enum DownFamily {
    /// Bit 18 clear, Table 30.
    Cpd(Cpd),
    /// Bit 18 set, Table 31.
    Suvd(Suvd),
}

impl DownFamily {
    /// Whichever of the two `bits` hold, if the CRC checks.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if bits.len() <= CP_MARK {
            return None;
        }
        if bits[CP_MARK] == SUV_MARK {
            Suvd::from_bits(bits).map(Self::Suvd)
        } else {
            Cpd::from_bits(bits).map(Self::Cpd)
        }
    }
}

/// What a candidate's first bits say about how long it is going to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Length {
    /// Not enough bits yet to know.
    Waiting,
    /// Long enough to know that this is not one of ours.
    Refused,
    /// This many bits, fill included.
    Known(usize),
}

/// How many information words a CPd holds, worked out from its header words the
/// way P4D 5.3 walks them: word 0's flags, then the four filter lengths, then
/// the six LCs, each of which is itself inside a part.
fn cpd_words(bits: &[bool]) -> Length {
    let Some(word0) = word_value(bits, 0) else {
        return Length::Waiting;
    };
    let mut words = 2;
    if word0 >> 1 & 1 == 1 {
        words += MODULUS_WORDS;
    }
    if word0 >> 2 & 1 == 1 {
        let mut coefficients = 0;
        for k in 0..FILTER_LENGTH_WORDS {
            let Some(length) = word_value(bits, words + k) else {
                return Length::Waiting;
            };
            coefficients += (length & ((1 << LENGTH_BITS) - 1)) as usize;
        }
        words += FILTER_LENGTH_WORDS + coefficients;
        // Asked for before the constellation part is walked: a header that has
        // already overrun says so now, rather than waiting for words that are
        // never coming.
        if words > CPD_WORDS_MOST {
            return Length::Refused;
        }
    }
    if word0 >> 3 & 1 == 1 {
        let mut points = 0;
        // The two index words say nothing about the length; the three after
        // them carry LC1 to LC6, two to a word.
        for k in 2..SET_HEADER_WORDS {
            let Some(lengths) = word_value(bits, words + k) else {
                return Length::Waiting;
            };
            points += (lengths & 0xff) as usize + (lengths >> 8 & 0xff) as usize;
        }
        words += SET_HEADER_WORDS + points;
    }
    if words > CPD_WORDS_MOST { Length::Refused } else { Length::Known(words) }
}

/// The same as bits, fill included.
fn cpd_length(bits: &[bool]) -> Length {
    match cpd_words(bits) {
        Length::Known(words) => Length::Known(sequence_bits(words)),
        other => other,
    }
}

/// A short sequence is fifty-two bits however it is padded, and the fill is not
/// read, so a finder needs no more than that.
fn short_length(_bits: &[bool]) -> Length {
    Length::Known(SHORT_BITS)
}

/// One kind of framed sequence, found in a stream of descrambled bits.
///
/// The V.92 rule is a zero after **at least** seventeen ones, taking the last
/// seventeen as the sync: CPt and Ja's descriptors arrive behind twenty-four-one
/// preambles, and the exact-seventeen rule walks straight past them (P3S 9.2).
/// Every place that could be a start is kept until enough bits have arrived to
/// read it there; the CRC throws out the false starts.
#[derive(Debug, Clone)]
struct Finder<T> {
    bits: Vec<bool>,
    ones: usize,
    starts: Vec<usize>,
    length: fn(&[bool]) -> Length,
    parse: fn(&[bool]) -> Option<T>,
}

impl<T> Finder<T> {
    fn new(length: fn(&[bool]) -> Length, parse: fn(&[bool]) -> Option<T>) -> Self {
        Self { bits: Vec::new(), ones: 0, starts: Vec::new(), length, parse }
    }

    fn feed(&mut self, bit: bool) -> Option<T> {
        if !bit && self.ones >= SYNC_ONES {
            self.starts.push(self.bits.len() - SYNC_ONES);
        }
        self.ones = if bit { self.ones + 1 } else { 0 };
        self.bits.push(bit);
        let mut found = None;
        let bits = &self.bits;
        let (length, parse) = (self.length, self.parse);
        self.starts.retain(|&start| {
            if found.is_some() {
                return false;
            }
            match length(&bits[start..]) {
                Length::Waiting => true,
                Length::Refused => false,
                Length::Known(needed) => {
                    if bits.len() - start < needed {
                        return true;
                    }
                    found = parse(&bits[start..start + needed]);
                    false
                }
            }
        });
        if found.is_some() {
            self.starts.clear();
        }
        // Keep what the oldest candidate needs, or the last sync's worth.
        let keep_from = self.starts.first().copied().unwrap_or(self.bits.len().saturating_sub(SYNC_ONES));
        if keep_from > 4096 {
            self.bits.drain(..keep_from);
            for start in &mut self.starts {
                *start -= keep_from;
            }
        }
        found
    }
}

/// Finds the analogue modem's framed sequences: CPt, CPu, CPus and SUVu.
///
/// The long CPs go through the V.92 layout of `v90::sequences`, whose finder
/// already knows how to read their constellation masks and how long they are;
/// the two fifty-two-bit sequences are found beside it, because their table is
/// a different one and their length is fixed.
#[derive(Debug, Clone)]
pub struct UpFinder {
    cps: CpFinder,
    shorts: Finder<CpFamily>,
}

impl UpFinder {
    /// `unit` is what twelve upstream symbols carry, as [`trn2u_unit`] gives it
    /// in training and a rate renegotiation and K gives it in a fast parameter
    /// exchange: a long CP is padded to it, and the finder has to know where
    /// the padding ends.
    pub fn new(unit: usize) -> Self {
        Self { cps: CpFinder::v92(unit), shorts: Finder::new(short_length, short_family) }
    }

    /// One more descrambled bit, and the sequence it completed if it completed
    /// one.
    pub fn feed(&mut self, bit: bool) -> Option<CpFamily> {
        let cp = self.cps.feed(bit).map(|cp| if cp.data_mode { CpFamily::Cpu(cp) } else { CpFamily::Cpt(cp) });
        let short = self.shorts.feed(bit);
        cp.or(short)
    }
}

/// A short upstream sequence: SUVu, or CPus.
fn short_family(bits: &[bool]) -> Option<CpFamily> {
    if bits[CP_MARK] == SUV_MARK {
        Suvu::from_bits(bits).map(CpFamily::Suvu)
    } else {
        Cpus::from_bits(bits).map(CpFamily::Cpus)
    }
}

/// Finds the digital modem's framed sequences: CPd and SUVd.
#[derive(Debug, Clone)]
pub struct DownFinder(Finder<DownFamily>);

impl Default for DownFinder {
    fn default() -> Self {
        Self::new()
    }
}

impl DownFinder {
    /// No unit is needed: an SUVd is read from its fifty-two bits and a CPd
    /// from its own header words, and neither reads its fill.
    pub fn new() -> Self {
        Self(Finder::new(down_length, DownFamily::from_bits))
    }

    /// One more descrambled bit, and the sequence it completed if it completed
    /// one.
    pub fn feed(&mut self, bit: bool) -> Option<DownFamily> {
        self.0.feed(bit)
    }
}

/// An SUVd is fifty-two bits; a CPd says how long it is in its header words.
fn down_length(bits: &[bool]) -> Length {
    if bits.len() <= CP_MARK {
        return Length::Waiting;
    }
    if bits[CP_MARK] == SUV_MARK { Length::Known(SHORT_BITS) } else { cpd_length(bits) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::info::crc;
    use crate::v90::sequences::{ALL_PCM_UPSTREAM_RATES, Descriptor, DescriptorFinder, JD_PRIME_BITS};
    use crate::v92::TRELLIS_FRAME;

    fn bits_of(text: &str) -> Vec<bool> {
        text.bytes().filter(|b| !b.is_ascii_whitespace()).map(|b| b == b'1').collect()
    }

    fn text(bits: &[bool]) -> String {
        bits.iter().map(|&b| if b { '1' } else { '0' }).collect()
    }

    /// The sixteen CRC bits of a framed sequence of `words` words, as they sit
    /// in `bits`: the CRC is the word after the last information word.
    fn crc_of(bits: &[bool], words: usize) -> u32 {
        get(bits, word_at(words) + 1, BLOCK)
    }

    /// `bits` with every position in `set` made a one and the CRC put right
    /// again, as a far end using a later ITU extension would send it.
    fn with_bits_set(bits: &[bool], words: usize, set: &[usize]) -> Vec<bool> {
        let mut out = bits.to_vec();
        for &p in set {
            out[p] = true;
        }
        let mut information = Vec::new();
        for w in 0..words {
            let at = word_at(w) + 1;
            information.extend_from_slice(&out[at..at + BLOCK]);
        }
        let value = u32::from(crc(&information));
        let crc_at = word_at(words) + 1;
        for i in 0..BLOCK {
            out[crc_at + i] = value >> i & 1 == 1;
        }
        out
    }

    /// How many information words the [`parameters`] CPd takes: the two
    /// mandatory ones, six of moduli, four filter lengths and eight
    /// coefficients, and five set headers and forty-four points.
    const FULL_WORDS: usize = 2 + 6 + 4 + 8 + 5 + 44;

    /// Alpha, which is seventeen bits for each of those eight coefficients.
    const FULL_ALPHA: usize = 17 * 8;

    /// A parameter set the whole of Table 30 can carry: twelve moduli, all four
    /// filter sections, and two constellation sets.
    ///
    /// The moduli are ones drn 13's K = 60 bits fit in and the two sets are big
    /// enough for: 48 and 40 levels, so the intervals where k = 3 -- 3, 7 and
    /// 11, all of them on the smaller set here -- keep N >= 2 x Mi.
    fn parameters() -> Parameters {
        Parameters {
            drn: 13,
            trellis: Trellis::ThirtyTwo,
            extend_e2u: true,
            gain: 1.0 / 16.0,
            moduli: [48, 40, 48, 20, 48, 40, 48, 20, 48, 40, 48, 20],
            filters: Filters {
                z1: vec![0.5, -0.25],
                p1: vec![-1.5, 0.75, 0.125],
                z2: vec![1.0 - 1.0 / 32768.0, 0.5],
                p2: vec![-2.0],
            },
            sets: vec![(1..=24).map(|k| k * 400).collect(), (1..=20).map(|k| k * 500).collect()],
            indices: [0, 1, 0, 1, 0, 1],
        }
    }

    /// 8.6.2 and Table 21: bit 47 is "the Jd/Jp identifier: 0 = Jd", where
    /// V.90's Table 13 had a constellation flag. The two vectors are P3S 5.2's.
    #[test]
    fn jd_vectors_check_and_say_jd_in_bit_47() {
        for (lookahead, crc, printed) in [
            (1, 0x776E, "11111111111111111 0 1111111111111111 0 1111110000000010 0 0111011011101110 0000"),
            (3, 0xF366, "11111111111111111 0 1111111111111111 0 1111110000000011 0 0110011011001111 0000"),
        ] {
            let jd = Jd { rates: Jd::ALL_RATES, lookahead, ..Jd::default() };
            let bits = jd.to_bits();
            assert_eq!(text(&bits), text(&bits_of(printed)), "look-ahead {lookahead}");
            assert_eq!(crc_of(&bits, 2), crc);
            assert!(!bits[J_IDENTIFIER], "a Jd carries 0 in bit 47");
            assert!(!bits[48], "and 0 in the bit V.90 used for a rate renegotiation");
            assert_eq!(J::from_bits(&bits), Some(J::Jd(jd)));
            assert_eq!(J::from_bits(&bits).map(|j| j.to_bits()), Some(bits));
            // The constructor a V.92 sender uses leaves both flags clear.
            assert_eq!(J::jd(Jd::ALL_RATES, lookahead), J::Jd(jd));
        }
    }

    /// 8.6.3 and Table 22: epsilon in bits 18:33, the training constellation in
    /// bit 48 and the rate renegotiation one in 49. The extracted text shifts
    /// these rows; the vectors are P3S 5.3's, read off the rendered page.
    #[test]
    fn jp_carries_epsilon_in_bits_18_to_33_and_the_trn2u_sizes_in_48_and_49() {
        for (jp, crc, printed) in [
            (
                Jp { epsilon: 0x8000, eight_in_training: false, eight_in_renegotiation: false },
                0x1F4C,
                "11111111111111111 0 0000000000000001 0 0000000000001000 0 0011001011111000 0000",
            ),
            (
                Jp { epsilon: 0x8000, eight_in_training: true, eight_in_renegotiation: false },
                0x3E4E,
                "11111111111111111 0 0000000000000001 0 0000000000001100 0 0111001001111100 0000",
            ),
            (
                Jp { epsilon: 0, eight_in_training: true, eight_in_renegotiation: true },
                0x70A6,
                "11111111111111111 0 0000000000000000 0 0000000000001110 0 0110010100001110 0000",
            ),
        ] {
            let bits = jp.to_bits();
            assert_eq!(bits.len(), JD_BITS, "Jp is Jd's seventy-two bits");
            assert_eq!(text(&bits), text(&bits_of(printed)));
            assert_eq!(crc_of(&bits, 2), crc);
            assert!(bits[J_IDENTIFIER], "a Jp carries 1 in bit 47");
            // Epsilon's LSB is bit 18 and its MSB bit 33.
            assert_eq!(get(&bits, 18, 16) as u16, jp.epsilon);
            assert_eq!(bits[48], jp.eight_in_training);
            assert_eq!(bits[49], jp.eight_in_renegotiation);
            assert_eq!(J::from_bits(&bits), Some(J::Jp(jp)));
        }
        // Jp' is twelve zeros, the same length as V.90's J'd, which V.92's
        // Phase 3 no longer sends.
        assert_eq!(JP_PRIME_BITS, JD_PRIME_BITS);
    }

    /// 8.6.2 with 8.6.3: the two are the same length, framed the same way, and
    /// both carry a good CRC, so only bit 47 tells them apart.
    #[test]
    fn a_jp_is_never_read_as_a_jd_and_back() {
        let jp = Jp { epsilon: 0xFFFF, eight_in_training: true, eight_in_renegotiation: false };
        let bits = jp.to_bits();
        // V.90's own reader takes it for a Jd: epsilon's sixteen bits become
        // the first sixteen rates of the mask, and bit 47 becomes the training
        // constellation flag.
        let misread = Jd::from_bits(&bits).expect("a Jp does carry a good CRC");
        assert_eq!(misread.rates, 0xFFFF);
        assert!(misread.sixteen_in_training, "this is the hazard, in one line");
        // The V.92 reader does not make that mistake, in either direction.
        assert_eq!(Jd::from_bits_if(&bits, is_jd), None);
        assert_eq!(J::from_bits(&bits), Some(J::Jp(jp)));
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 2, ..Jd::default() };
        assert_eq!(Jp::from_bits(&jd.to_bits()), None);
        assert_eq!(J::from_bits(&jd.to_bits()), Some(J::Jd(jd)));
    }

    /// Table 31 with the CRC of 10.1.2.3.2/V.34: the four P4D 2.4 vectors,
    /// which are the four states of bits 32 and 33.
    #[test]
    fn suvd_vectors_match() {
        for (suvd, crc, printed) in [
            (Suvd { silence: false, ack: false }, 0xE960, "11111111111111111 0 1000000000000000 0 0000011010010111 0"),
            (Suvd { silence: false, ack: true }, 0x6D68, "11111111111111111 0 1000000000000001 0 0001011010110110 0"),
            (Suvd { silence: true, ack: false }, 0xAB64, "11111111111111111 0 1000000000000010 0 0010011011010101 0"),
            (Suvd { silence: true, ack: true }, 0x2F6C, "11111111111111111 0 1000000000000011 0 0011011011110100 0"),
        ] {
            // D = 42 bits a frame, so the fifty-two bits round up to two
            // frames; the vector is the sequence before that fill.
            let bits = suvd.to_bits(42);
            assert_eq!(text(&bits[..SHORT_BITS]), text(&bits_of(printed)));
            assert_eq!(crc_of(&bits, 1), crc);
            assert!(bits[CP_MARK], "SUVd: 1");
            assert_eq!(Suvd::from_bits(&bits), Some(suvd));
            assert_eq!(DownFamily::from_bits(&bits), Some(DownFamily::Suvd(suvd)));
        }
    }

    /// 8.8.3 with Table 30: a CPd with no optional part is two words, which is
    /// sixty-nine bits before the fill. The vector is P4D 2.4's.
    #[test]
    fn the_smallest_cpd_is_69_bits() {
        let cpd = Cpd {
            drn: 19,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            ack: false,
            gain4: 0x4000,
            moduli: None,
            filters: None,
            sets: None,
        };
        let bits = cpd.to_bits(9);
        assert_eq!(sequence_bits(2), 69);
        assert_eq!(
            text(&bits[..69]),
            text(&bits_of("11111111111111111 0 0000110010000000 0 0000000000000010 0 1101000011101010 0"))
        );
        assert_eq!(crc_of(&bits, 2), 0x570B);
        // D = 9 bits a frame at the slowest TRN2d rate, so sixty-nine bits
        // round up to eight frames.
        assert_eq!(bits.len(), 72);
        assert_eq!(Cpd::from_bits(&bits), Some(cpd.clone()));
        // And the same sequence primed.
        let primed = Cpd { ack: true, ..cpd };
        assert_eq!(crc_of(&primed.to_bits(9), 2), 0x5BE7);
        assert_eq!(Cpd::from_bits(&primed.to_bits(9)), Some(primed));
    }

    /// Table 30 read end to end: every part present, back to the same
    /// `Parameters`, with every start bit on a multiple of seventeen.
    #[test]
    fn a_cpd_with_every_part_round_trips_through_parameters_and_keeps_start_bits_on_multiples_of_17() {
        let wanted = parameters();
        let cpd = Cpd::from(&wanted);
        assert!(cpd.complete());
        let bits = cpd.to_bits(30);
        let words = FULL_WORDS;
        assert_eq!(bits.len(), sequence_bits(words).div_ceil(30) * 30);
        for w in 0..=words {
            let at = word_at(w);
            assert_eq!(at % WORD, 0, "word {w} does not start on a multiple of seventeen");
            assert!(!bits[at], "the start bit of word {w} is not a zero");
        }
        let read = Cpd::from_bits(&bits).expect("the CRC did not check");
        assert_eq!(read, cpd);
        assert_eq!(Parameters::try_from(&read), Ok(wanted));
        // And the parts are where Table 30 puts them when all three are there:
        // M0 at 52:59, LZ1 at 154:162, the first index at 222+alpha.
        assert_eq!(get(&bits, 52, 8), 48);
        assert_eq!(get(&bits, 154, LENGTH_BITS), 2);
        let alpha = FULL_ALPHA;
        assert_eq!(get(&bits, 222 + alpha, 4), 0);
        assert_eq!(get(&bits, 226 + alpha, 4), 1);
        assert_eq!(get(&bits, 256 + alpha, 8), 24, "LC1");
        assert_eq!(get(&bits, 264 + alpha, 8), 20, "LC2");
    }

    /// 8.8.3: "all the bits contained in a part are removed from the CPd
    /// sequence when it is indicated that the part is not present", so the CRC
    /// and everything else move up by the whole part.
    #[test]
    fn an_absent_part_moves_everything_after_it() {
        let full = Cpd::from(&parameters());
        let without = Cpd { moduli: None, ..full.clone() };
        // Unpadded, so that the fill of the carrying modulation does not blur
        // what the part itself is worth.
        let (full_bits, short_bits) = (full.to_bits(1), without.to_bits(1));
        // Six words of moduli, at seventeen bits each.
        assert_eq!(full_bits.len() - short_bits.len(), 6 * WORD);
        // The filter lengths now sit where the moduli were.
        assert_eq!(get(&short_bits, 52, LENGTH_BITS), 2, "LZ1 has moved to bit 52");
        assert_eq!(Cpd::from_bits(&short_bits), Some(without.clone()));
        // An absent part means the previous one stands.
        let merged = without.merged_over(&full);
        assert_eq!(merged, full);
        assert!(!without.complete() && full.complete());
        // Any one part can go, and the sequence still reads.
        for cpd in [
            Cpd { filters: None, ..full.clone() },
            Cpd { sets: None, ..full.clone() },
            Cpd { moduli: None, filters: None, sets: None, ..full.clone() },
        ] {
            let bits = cpd.to_bits(30);
            assert_eq!(Cpd::from_bits(&bits), Some(cpd.clone()));
            assert_eq!(cpd.merged_over(&full), full);
        }
    }

    /// The 8.8.3 SHALLs and the constraints 8.8 leaves to 6.4: what a CPd may
    /// ask an analogue modem for, given what INFO1a said it can do.
    #[test]
    fn cpd_limits_are_checked() {
        let limits = FilterLimits::from_info1a(3, 3, 3); // every section, Ltot 384, Lmax 320
        let good = Cpd::from(&parameters());
        let bits = super::super::up_bits(good.drn);
        assert_eq!(good.check(&limits, bits), Ok(()));
        let long = |n: usize| vec![0.5; n];
        let refused: [(Cpd, &str); 8] = [
            (
                Cpd {
                    filters: Some(Filters { z1: long(100), p1: long(100), z2: long(100), p2: long(100) }),
                    ..good.clone()
                },
                "the filters have more coefficients than this end offered",
            ),
            (
                Cpd { filters: Some(Filters { z1: vec![], p1: long(330), z2: long(2), p2: vec![] }), ..good.clone() },
                "a filter section is longer than this end offered",
            ),
            (
                Cpd { sets: Some(Sets { index: [0; 6], points: vec![vec![0, 400]] }), ..good.clone() },
                "a constellation set contains the zero point",
            ),
            (
                Cpd {
                    sets: Some(Sets { index: [0, 2, 0, 2, 0, 2], points: vec![vec![400], vec![], vec![500]] }),
                    ..good.clone()
                },
                "a constellation set is empty",
            ),
            (
                Cpd { sets: Some(Sets { index: [3; CONSTELLATION_FRAME], points: vec![vec![400]] }), ..good.clone() },
                "a constellation index points past the sets",
            ),
            (
                Cpd { moduli: Some([200; UP_INTERVALS]), ..good.clone() },
                "an interval's constellation is too small for its modulus",
            ),
            (Cpd { moduli: Some([2; UP_INTERVALS]), ..good.clone() }, "the data frame carries more bits than the moduli can hold"),
            (Cpd { gain4: 0, ..good.clone() }, "the gain is zero, which Table 30 forbids"),
        ];
        for (cpd, reason) in refused {
            assert_eq!(cpd.check(&limits, bits), Err(reason));
        }
        // A section the far end never offered, which is the other half of
        // Table 18 bits 12:13.
        let neither = FilterLimits::from_info1a(0, 3, 3);
        assert_eq!(
            good.check(&neither, bits),
            Err("the precoder has a feed-forward section this end did not offer")
        );
        // N >= Mi everywhere, but N >= 2 x Mi where k = 3: the same modulus is
        // fine in interval 1 and too large in interval 3, both of which use the
        // forty-level set.
        let raise = |i: usize| {
            let mut moduli = parameters().moduli;
            moduli[i] = 21;
            Cpd { moduli: Some(moduli), ..good.clone() }
        };
        assert_eq!(raise(1).check(&limits, bits), Ok(()));
        assert_eq!(
            raise(TRELLIS_FRAME - 1).check(&limits, bits),
            Err("an interval's constellation is too small for its modulus")
        );
        // A partial CPd has no parameters of its own to check.
        let partial = Cpd { moduli: None, filters: None, sets: None, ..good.clone() };
        assert_eq!(partial.check(&limits, bits), Err("the CPd carries no modulus encoder parameters"));
        assert_eq!(partial.merged_over(&good).check(&limits, bits), Ok(()));
        // And a rate off the ladder is not one drn names.
        assert_eq!(
            Cpd { drn: 20, ..good }.check(&limits, bits),
            Err("the upstream rate is not a rung of the ladder")
        );
    }

    /// Table 27 bits 27:31: "20 x log10(L) ... in signed Q2.2 format (sxx.xx).
    /// The value 16 (-4.00) indicates that no measurement has been taken."
    #[test]
    fn suvu_level_is_signed_q2_2_and_16_means_none() {
        let unmeasured = Suvu { wait_for_cpu: true, level: None, silence: false, ack: false };
        let bits = unmeasured.to_bits(TRN2U_FOUR_BITS);
        // Bit 31 set and 27:30 clear is the printed 16.
        assert_eq!(get(&bits, 27, 5), LEVEL_NONE);
        assert!(bits[26], "the wait-for-CPu request");
        assert_eq!(Suvu::from_bits(&bits), Some(unmeasured));
        // Every step of the field, and the two ends of it.
        for code in 0..32u32 {
            let level = signed_q(code, LEVEL_WIDTH, LEVEL_FRACTION);
            let suvu = Suvu { level: Some(level), ..Suvu::default() };
            let read = Suvu::from_bits(&suvu.to_bits(TRN2U_EIGHT_BITS)).expect("the CRC did not check");
            if code == LEVEL_NONE {
                // -4.00 is not a level: a measurement there is clamped to the
                // largest magnitude the field can otherwise report.
                assert_eq!(read.level, Some(-LEVEL_LARGEST));
            } else {
                assert_eq!(read.level, Some(level), "code {code}");
                assert!((-LEVEL_LARGEST..=LEVEL_LARGEST).contains(&level));
            }
        }
        assert_eq!(signed_q(LEVEL_NONE, LEVEL_WIDTH, LEVEL_FRACTION), -4.0);
        assert_eq!(signed_q(0x0F, LEVEL_WIDTH, LEVEL_FRACTION), LEVEL_LARGEST);
        // A design asking for more than the field can hold still sends a legal
        // one, at the top of the range rather than wrapping to the bottom.
        let loud = Suvu { level: Some(9.0), ..Suvu::default() };
        assert_eq!(get(&loud.to_bits(TRN2U_FOUR_BITS), 27, 5), 0x0F);
        let quiet = Suvu { level: Some(-9.0), ..Suvu::default() };
        assert_eq!(Suvu::from_bits(&quiet.to_bits(TRN2U_FOUR_BITS)).and_then(|s| s.level), Some(-LEVEL_LARGEST));
    }

    /// Table 24: CPus is the CP family's type 2, and its CRC covers bits 18:33
    /// alone, because it has one information word.
    #[test]
    fn cpus_is_type_2_with_its_crc_over_bits_18_to_33() {
        let cpus = Cpus { drn: 22, ack: true };
        let bits = cpus.to_bits(TRN2U_EIGHT_BITS);
        assert!(!bits[CP_MARK], "CP: 0");
        assert_eq!(get(&bits, CP_MARK + 1, 2), CP_TYPE_CPUS);
        // drn 22 is 10110 in bits 21:25, least significant bit first.
        assert_eq!(text(&bits[21..26]), "01101");
        assert_eq!(get(&bits, 21, 5), 22);
        assert!(bits[33], "the acknowledge bit");
        let information: Vec<bool> = bits[18..34].to_vec();
        assert_eq!(crc_of(&bits, 1), u32::from(crc(&information)));
        assert_eq!(Cpus::from_bits(&bits), Some(cpus));
        assert_eq!(CpFamily::from_bits(&bits), Some(CpFamily::Cpus(cpus)));
        // drn = 0 is the cleardown form (9.11).
        let clear = Cpus { drn: 0, ack: false };
        assert_eq!(get(&clear.to_bits(TRN2U_FOUR_BITS), 21, 5), 0);
        assert_eq!(Cpus::from_bits(&clear.to_bits(TRN2U_FOUR_BITS)), Some(clear));
    }

    /// The fill of Tables 23, 24, 27, 30 and 31: "to the next multiple of 12
    /// symbols" upstream and "of 6 symbols" downstream, in whatever bits the
    /// modulation carrying the sequence puts in a symbol.
    #[test]
    fn sequences_pad_to_24_36_k_or_d_bits() {
        assert_eq!((trn2u_unit(false), trn2u_unit(true)), (24, 36));
        let suvu = Suvu::default();
        // Fifty-two bits become seventy-two at both TRN2u sizes: three
        // twelve-symbol frames of twenty-four bits, or two of thirty-six.
        assert_eq!(suvu.to_bits(TRN2U_FOUR_BITS).len(), 72);
        assert_eq!(suvu.to_bits(TRN2U_EIGHT_BITS).len(), 72);
        assert_eq!(Cpus::default().to_bits(TRN2U_FOUR_BITS).len(), 72);
        // In a fast parameter exchange the unit is K, which runs 36 to 72.
        for drn in 1..=UP_RATES as u8 {
            let k = super::super::up_bits(drn) as usize;
            let bits = suvu.to_bits(k);
            assert_eq!(bits.len(), SHORT_BITS.div_ceil(k) * k, "drn {drn}");
            assert!(bits.len() % k == 0 && bits.len() >= SHORT_BITS);
        }
        // Downstream the unit is D, six symbols' worth.
        for (d, length) in [(9, 54), (30, 60), (42, 84)] {
            assert_eq!(Suvd::default().to_bits(d).len(), length, "D = {d}");
        }
        // Every one of them keeps the fill bit Table 31 asks for, whatever the
        // padding does.
        assert!(!Suvd::default().to_bits(42)[SHORT_BITS - 1]);
    }

    /// P4A 3.9: bit 18 tells the CP family from the SUV family, and the type at
    /// 19:20 tells CPt, CPu and CPus apart.
    #[test]
    fn the_cp_family_is_told_apart_by_bit_18_and_type() {
        let cpt = Cp { data_mode: false, drn: 16, ..Cp::default() };
        let cpu = Cp { data_mode: true, drn: 22, ..Cp::default() };
        let cpus = Cpus { drn: 22, ack: false };
        let suvu = Suvu { silence: true, ..Suvu::default() };
        let unit = trn2u_unit(false);
        assert_eq!(CpFamily::from_bits(&cpt.to_bits_in(Layout::V92, unit)), Some(CpFamily::Cpt(cpt.clone())));
        assert_eq!(CpFamily::from_bits(&cpu.to_bits_in(Layout::V92, unit)), Some(CpFamily::Cpu(cpu.clone())));
        assert_eq!(CpFamily::from_bits(&cpus.to_bits(unit)), Some(CpFamily::Cpus(cpus)));
        assert_eq!(CpFamily::from_bits(&suvu.to_bits(unit)), Some(CpFamily::Suvu(suvu)));
        // Type 3 is unassigned: a dispatch field this end cannot read is the
        // one thing that stops a sequence being read at all.
        let mut three = cpus.to_bits(unit);
        let words = 1;
        three = with_bits_set(&three, words, &[CP_MARK + 1]);
        assert_eq!(CpFamily::from_bits(&three), None);
        // And an SUVu is never a CPus, however its bits fall.
        assert_eq!(Cpus::from_bits(&suvu.to_bits(unit)), None);
        assert_eq!(Suvu::from_bits(&cpus.to_bits(unit)), None);
        // Downstream the same bit 18 separates CPd from SUVd.
        let cpd = Cpd::cleardown(false);
        assert_eq!(DownFamily::from_bits(&cpd.to_bits(30)), Some(DownFamily::Cpd(cpd)));
        assert_eq!(DownFamily::from_bits(&Suvd::default().to_bits(30)), Some(DownFamily::Suvd(Suvd::default())));
    }

    /// Table 23/V.92 against Table 14/V.90: the type and drn moved, so a CP
    /// read in the wrong layout is a CP with the wrong rate (CD 4.4).
    #[test]
    fn a_v90_cp_is_not_read_as_a_v92_cp() {
        let cpu = Cp { data_mode: true, drn: 12, ..Cp::default() };
        let v92 = cpu.to_bits_in(Layout::V92, trn2u_unit(false));
        // V.90's reader looks for drn at 20:24, one place below where V.92
        // puts it, so it reads the rate doubled.
        let misread = Cp::from_bits(&v92).expect("the CRC does check either way");
        assert_eq!(misread.drn, 2 * cpu.drn, "the doubled drn of the wrong layout");
        // And the other way, a V.90 CP read as V.92: the drn comes out halved,
        // and the type field has swallowed its lowest bit.
        let v90 = cpu.to_bits();
        let other = Cp::from_bits_in(Layout::V92, &v90).expect("the CRC does check either way");
        assert_eq!(other.drn, cpu.drn / 2);
        // With an odd drn that lowest bit makes the type 3, which is
        // unassigned, so the sequence is refused outright rather than misread.
        let odd = Cp { drn: 11, ..cpu.clone() };
        assert_eq!(Cp::from_bits_in(Layout::V92, &odd.to_bits()), None);
        assert_eq!(CpFamily::from_bits(&odd.to_bits()), None);
        // Read in its own layout it is itself.
        assert_eq!(CpFamily::from_bits(&v92), Some(CpFamily::Cpu(cpu)));
    }

    /// Tables 25 and 26: two intervals at Mi - 1, two at 0, in a pattern of
    /// period four, with RM' the same pattern shifted by two.
    #[test]
    fn rm_and_rm_prime_follow_tables_25_and_26() {
        let moduli = [10u8, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21];
        let printed = |i: usize| moduli[i] as u32 - 1;
        // Table 25, row by row, including row 11's "u11 = 0".
        let rm: Vec<u32> = (0..UP_INTERVALS).map(|i| rm_k(i, moduli[i], false)).collect();
        assert_eq!(
            rm,
            vec![printed(0), printed(1), 0, 0, printed(4), printed(5), 0, 0, printed(8), printed(9), 0, 0]
        );
        // Table 26, whose row 11 reads "k11 = M11 - 1".
        let rm_prime: Vec<u32> = (0..UP_INTERVALS).map(|i| rm_k(i, moduli[i], true)).collect();
        assert_eq!(
            rm_prime,
            vec![0, 0, printed(2), printed(3), 0, 0, printed(6), printed(7), 0, 0, printed(10), printed(11)]
        );
        // The transition the digital modem watches for: RM ends with two zeros
        // and RM' starts with two, so four in a row (RRF 2.5).
        assert_eq!((rm[10], rm[11], rm_prime[0], rm_prime[1]), (0, 0, 0, 0));
        // An interval whose modulus is one carries nothing either way, which is
        // what a detector has to mask.
        assert_eq!(rm_k(0, 1, false), 0);
        assert_eq!(rm_k(0, 0, false), 0, "no modulus at all, and no wrap to 255");
    }

    /// 9.11: "cleardown is signalled by drn = 0 in a rate sequence". The
    /// printed text says SUVu and SUVd, which have no drn; the fields are CPd
    /// bits 22:26 and CPu or CPus bits 21:25 (P4A 5.4, E2).
    #[test]
    fn a_cleardown_cpd_needs_no_parts() {
        let clear = Cpd::cleardown(true);
        assert_eq!(clear.drn, 0);
        assert!(!clear.complete(), "a cleardown settles no modulation");
        let bits = clear.to_bits(30);
        assert_eq!(bits.len(), sequence_bits(2).div_ceil(30) * 30);
        assert_eq!(get(&bits, 22, 5), 0);
        assert!(bits[33], "the acknowledge bit still travels");
        assert_eq!(Cpd::from_bits(&bits), Some(clear.clone()));
        // A cleardown passes every check there is, because there is nothing
        // left to hold it to.
        assert_eq!(clear.check(&FilterLimits::from_info1a(0, 0, 0), 0), Ok(()));
        // The upstream forms, in bits 21:25 of the two CP sequences.
        let cpu = Cp { data_mode: true, drn: 0, ..Cp::default() };
        let bits = cpu.to_bits_in(Layout::V92, trn2u_unit(true));
        assert_eq!(get(&bits, 21, 5), 0);
        assert_eq!(CpFamily::from_bits(&bits), Some(CpFamily::Cpu(cpu)));
        let cpus = Cpus { drn: 0, ack: true };
        assert_eq!(get(&cpus.to_bits(trn2u_unit(true)), 21, 5), 0);
        // Neither SUV has a drn field at all, which is why 9.11 cannot mean
        // what it prints: an SUVu is fifty-two bits of flags and a CRC.
        assert_eq!(Suvu::default().to_bits(24).len(), 72);
    }

    /// Every "reserved for the ITU" run of Tables 21, 22, 23, 27, 30 and 31:
    /// "set to 0 ... and not interpreted", so a far end using a later extension
    /// still connects.
    #[test]
    fn reserved_bits_set_by_a_far_end_are_ignored_not_rejected() {
        // Jd bits 41:46 and 48, and Jp bits 35:46 and 50.
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 2, ..Jd::default() };
        let mut set: Vec<usize> = (41..=46).collect();
        set.push(48);
        let loud = with_bits_set(&jd.to_bits(), 2, &set);
        assert_eq!(J::from_bits(&loud), Some(J::Jd(Jd { sixteen_in_renegotiation: true, ..jd })));
        let jp = Jp { epsilon: 0x1234, eight_in_training: true, eight_in_renegotiation: false };
        let mut set: Vec<usize> = (35..=46).collect();
        set.push(50);
        let loud = with_bits_set(&jp.to_bits(), 2, &set);
        assert_eq!(J::from_bits(&loud), Some(J::Jp(jp)), "Jp's reserved runs change nothing");
        // SUVu bits 19:25, and SUVd bits 19:31.
        let suvu = Suvu { level: Some(1.5), ack: true, ..Suvu::default() };
        let loud = with_bits_set(&suvu.to_bits(24), 1, &(19..=25).collect::<Vec<_>>());
        assert_eq!(Suvu::from_bits(&loud), Some(suvu));
        let suvd = Suvd { silence: true, ack: false };
        let loud = with_bits_set(&suvd.to_bits(30), 1, &(19..=31).collect::<Vec<_>>());
        assert_eq!(Suvd::from_bits(&loud), Some(suvd));
        // CPus bits 26:32.
        let cpus = Cpus { drn: 7, ack: true };
        let loud = with_bits_set(&cpus.to_bits(24), 1, &(26..=32).collect::<Vec<_>>());
        assert_eq!(Cpus::from_bits(&loud), Some(cpus));
        // CPd bits 30:32, and the reserved bits inside its filter and set
        // headers: 163:169, 180:186, 197:203, 214:220 and 247+alpha:254+alpha.
        let cpd = Cpd::from(&parameters());
        let alpha = FULL_ALPHA;
        let mut set: Vec<usize> = (30..=32).collect();
        for from in [163, 180, 197, 214] {
            set.extend(from..from + 7);
        }
        set.extend(247 + alpha..=254 + alpha);
        let loud = with_bits_set(&cpd.to_bits(30), FULL_WORDS, &set);
        assert_eq!(Cpd::from_bits(&loud), Some(cpd), "CPd's reserved runs change nothing");
    }

    /// 8.8.3: a trellis field of 3 is "reserved for the ITU" and names no
    /// encoder, so a CPd carrying it is not one this end can transmit under.
    /// Reserved *bits* are ignored; a dispatch field that cannot be read is
    /// not.
    #[test]
    fn a_reserved_trellis_code_names_no_encoder() {
        let cpd = Cpd::from(&parameters());
        let reserved = with_bits_set(&cpd.to_bits(30), FULL_WORDS, &[27, 28]);
        assert_eq!(get(&reserved, 27, 2), 3);
        assert_eq!(Cpd::from_bits(&reserved), None);
        assert_eq!(DownFamily::from_bits(&reserved), None);
        // The three codes that do name one are read as they are printed.
        for (code, trellis) in [(0, Trellis::Sixteen), (1, Trellis::ThirtyTwo), (2, Trellis::SixtyFour)] {
            let cpd = Cpd { trellis, ..Cpd::cleardown(false) };
            let bits = cpd.to_bits(30);
            assert_eq!(get(&bits, 27, 2), u32::from(code as u8));
            assert_eq!(Cpd::from_bits(&bits).map(|read| read.trellis), Some(trellis));
        }
    }

    /// 6.4.2 with Table 30: a set is sent as its positive magnitudes and the
    /// negative half mirrors it, a(-eta-1) = -a(eta).
    #[test]
    fn a_set_is_the_mirror_of_its_printed_magnitudes() {
        let points = [400u16, 1200, 3000];
        let levels = set_levels(&points);
        assert_eq!(levels, vec![-3000, -1200, -400, 400, 1200, 3000]);
        assert_eq!(levels.len(), 2 * points.len(), "N = 2 x LC");
        // eta runs from -N/2 to N/2 - 1, so index eta + N/2 is a(eta).
        let n = levels.len() as i32;
        for eta in -n / 2..n / 2 {
            let level = levels[(eta + n / 2) as usize];
            assert_eq!(level, -levels[(-eta - 1 + n / 2) as usize], "eta {eta}");
        }
        // This end sends 2 x LC <= 128 and accepts LC <= 128, which is the two
        // readings of "the number of points in a constellation set shall not
        // exceed 128".
        assert_eq!(SET_POINTS_SENT, 64);
        assert_eq!(CONSTELLATION_POINTS, 128);
    }

    /// P3S 9.2: CPt and Ja's first descriptor sit behind twenty-four-one
    /// preambles, so a sequence starts at the first zero after **at least**
    /// seventeen ones.
    #[test]
    fn a_sequence_behind_a_long_run_of_ones_is_still_found() {
        let unit = trn2u_unit(false);
        let cpt = Cp { data_mode: false, drn: 16, ..Cp::default() };
        let suvu = Suvu { wait_for_cpu: true, level: Some(-0.5), silence: false, ack: true };
        let cpus = Cpus { drn: 21, ack: true };
        let mut stream = vec![true; 24];
        stream.extend(cpt.to_bits_in(Layout::V92, unit));
        stream.extend(vec![true; 41]);
        stream.extend(suvu.to_bits(unit));
        stream.extend(cpus.to_bits(unit));
        let mut finder = UpFinder::new(unit);
        let found: Vec<CpFamily> = stream.iter().filter_map(|&bit| finder.feed(bit)).collect();
        assert_eq!(found, vec![CpFamily::Cpt(cpt), CpFamily::Suvu(suvu), CpFamily::Cpus(cpus)]);
        // The V.90 descriptor finder's exact-seventeen rule is what this
        // replaces, and V92-03's V.92 finder has the same relaxation.
        let mut ja = vec![true; 24];
        ja.extend(Descriptor::none().to_bits_in(Some(ALL_PCM_UPSTREAM_RATES)));
        let mut loose = DescriptorFinder::v92();
        assert!(ja.iter().filter_map(|&bit| loose.feed(bit)).count() == 1);
    }

    /// A downstream stream is read the same way, and a CPd's length comes out
    /// of its own header words rather than from a table.
    #[test]
    fn cpd_and_suvd_are_found_back_to_back_in_a_stream() {
        let full = Cpd::from(&parameters());
        let partial = Cpd { moduli: None, filters: None, ..full.clone() };
        let suvd = Suvd { silence: false, ack: true };
        let mut stream = vec![true; 20];
        stream.extend(suvd.to_bits(30));
        stream.extend(full.to_bits(30));
        stream.extend(partial.to_bits(30));
        stream.extend(Cpd::cleardown(true).to_bits(30));
        let mut finder = DownFinder::new();
        let found: Vec<DownFamily> = stream.iter().filter_map(|&bit| finder.feed(bit)).collect();
        assert_eq!(
            found,
            vec![
                DownFamily::Suvd(suvd),
                DownFamily::Cpd(full),
                DownFamily::Cpd(partial),
                DownFamily::Cpd(Cpd::cleardown(true)),
            ]
        );
    }

    /// The nine- and eight-bit length fields of Table 30 can ask for three
    /// times the longest CPd there can be, and a finder that believed them
    /// would wait seconds for bits that cannot pass the CRC.
    #[test]
    fn a_header_asking_for_more_than_a_cpd_can_hold_is_dropped() {
        assert_eq!(CPD_WORDS_MOST, 1169);
        assert_eq!(sequence_bits(CPD_WORDS_MOST), 19_908);
        // A word 0 with every part flagged, and filter lengths of 511 each.
        let mut information = Vec::new();
        put(&mut information, 0b1110, BLOCK);
        put(&mut information, 0x4000, BLOCK);
        for _ in 0..MODULUS_WORDS {
            put(&mut information, 0, BLOCK);
        }
        for _ in 0..FILTER_LENGTH_WORDS {
            put(&mut information, (1 << LENGTH_BITS) - 1, BLOCK);
        }
        let bits = frame(&information);
        assert_eq!(cpd_words(&bits), Length::Refused);
        // The finder drops it rather than holding it, so the next sequence is
        // still found.
        let mut stream = bits.clone();
        stream.extend(Suvd::default().to_bits(30));
        let mut finder = DownFinder::new();
        let found: Vec<DownFamily> = stream.iter().filter_map(|&bit| finder.feed(bit)).collect();
        assert_eq!(found, vec![DownFamily::Suvd(Suvd::default())]);
        // A header that is merely incomplete says so rather than refusing.
        assert_eq!(cpd_words(&bits[..word_at(3)]), Length::Waiting);
    }
}
