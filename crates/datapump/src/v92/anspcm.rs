//! Short Phase 1 as it sounds on the line: QTS, ANSpcm and TONEq.
//!
//! V.90's digital modem answers a call the way every modem since V.25 has
//! done, with a tone its own synthesiser makes. V.92's does not. Once both
//! ends have said in their QC sequences that they want the short start-up, the
//! digital modem stops being a tone generator and becomes what it is: the end
//! that owns the network's codewords. Everything it sends from that moment is
//! a list of octets out of Table 1/V.90, and the three signals in this module
//! are the first of them.
//!
//! They come in a fixed order, with no gap anywhere in it (Figures 3 to 6):
//!
//! ```text
//! QCA1d/QCA2d  75 +/- 5 ms silence  QTS 768T  QTS\ 48T  ANSpcm ... TONEq heard
//!                                   |                              |
//!                                   data frame interval 0          75 ms, Phase 2
//! ```
//!
//! **QTS** (8.3.6) is 128 repetitions of a six-symbol pattern and **QTS\\** is
//! eight repetitions of the same pattern inverted. Its job is not to be heard
//! but to be *placed*: "the first symbol of QTS is defined to be transmitted in
//! data frame interval 0. The digital modem shall keep data frame alignment
//! from this point on", so the sign reversal 768 symbols in is a mark on the
//! line that says exactly where the downstream frame grid sits -- and that grid
//! then has to survive the whole of Phase 2 and be the same grid Sd lands on in
//! Phase 3. [`QtsWatch`] is the analogue modem's end of that: it finds the
//! reversal to a fraction of a symbol.
//!
//! **ANSpcm** (8.3.1) is the answering tone, made of codewords rather than of
//! a sine: 301 octets repeating, which is 79 cycles, which is 2099.67 Hz -- and
//! a phase reversal every 3612 symbols, which is 451.5 ms. Both numbers sit
//! inside V.8's tolerances for ANSam on purpose, because the network between
//! the two modems has to hear an answering tone and disable its echo control.
//! What it has *not* got is ANSam's 15 Hz amplitude modulation, so to an
//! ordinary V.8 ear it is V.25's plain ANS, and the only thing that tells the
//! two apart on the analogue side is the QTS burst in front of it
//! ([`AnspcmWatch`]).
//!
//! **TONEq** (8.2.5) is the whole of the analogue modem's reply: "signal TONEq
//! is a 980 Hz tone", and nothing else is said about it anywhere. 980 Hz is
//! also the V.21(L) mark frequency, so the detector here cannot simply look for
//! 980 Hz -- a modem holding a V.21(L) mark is doing that too, and a QC1a with
//! U_QTS `1111` contains a run of fourteen of them.
//!
//! The octet tables are Tables 7 to 10 copied out, and the generator of 8.3.1
//! is kept only as a test. That is the right way round: 8.3.1 says the
//! generated output "shall equal the output defined in Tables 7 to Table 10",
//! so the tables are the requirement and the equation is a way of arriving at
//! it. The equation also needs a G.711 quantiser that G.711 itself is not in
//! `docs/specs` to settle, which is exactly the sort of thing to prove against
//! a printed table once rather than to trust on every call.

use dsp::filter::OnePole;
use dsp::{Nco, ToneDetector};
use v8::ansam::AnswerTone;
use v8::quick::AnspcmLevel;

use crate::v90::ucode::{self, Law};

// ---------------------------------------------------------------------------
// ANSpcm (8.3.1, Tables 6 to 10)
// ---------------------------------------------------------------------------

/// "The sequence repeats every 301 symbols" (8.3.1), which is what Tables 7 to
/// 10 print: one column of octets for k = 0 to 300.
pub const ANSPCM_PERIOD: usize = 301;

/// How many cycles of tone those 301 symbols hold, from the `79/301` inside
/// the cosine of 8.3.1. 301 is 7 x 43 and shares no factor with 79, so the
/// period really is 301 symbols and not a shorter one.
pub const ANSPCM_CYCLES: usize = 79;

/// "A phase reversal added to it every 3612 symbols" (8.3.1): 451.5 ms, inside
/// V.8's 450 +/- 25 ms for ANSam.
///
/// 3612 is 12 x [`ANSPCM_PERIOD`], so a reversal always lands on the start of
/// a period, and 602 x 6, so it always lands on a downstream data frame
/// boundary as well -- which is what lets the digital modem keep the alignment
/// QTS established while it sends an answering tone.
pub const ANSPCM_REVERSAL: usize = 3612;

/// Where ANSpcm's first reversal falls, and which polarity comes first.
///
/// `true` is the reading fixed in section 4 of the plan: the sequence starts
/// with the octets as Tables 7 to 10 print them and the first reversal comes
/// after a full [`ANSPCM_REVERSAL`] symbols, counted from the first ANSpcm
/// symbol. 8.3.1 says only that a reversal is "added to it every 3612 symbols"
/// and never says where the count starts or which way round the first block
/// goes, so the alternative -- the inverted octets first -- is one `false`
/// away, and a capture of a real V.92 server would settle it (P1D Q2).
///
/// Getting it wrong is not audible: a far end that reads it the other way
/// still hears a 2100 Hz tone reversing every 451.5 ms, which is all an
/// answering tone has to be, and [`AnspcmWatch`] measures the tone rather than
/// the polarity. It matters only to a modem that checks the channel against
/// the codewords it knows were sent -- the "may be used to verify that assumed
/// channel characteristics are correct" of 8.3.1 -- and for that both ends
/// have to agree.
pub const ANSPCM_TABLE_POLARITY_FIRST: bool = true;

/// theta, the phase offset every row of Table 6 carries: `0.25 x pi / 301`.
///
/// Only the generator in the tests uses it, because the tables are what
/// reaches the wire. It is here so that the one number the four rows share is
/// written down once.
pub const ANSPCM_THETA: f64 = 0.25 * std::f64::consts::PI / ANSPCM_PERIOD as f64;

/// ANSpcm's tone, in hertz: 8000 x 79 / 301 = 2099.668.
///
/// Derived from [`ANSPCM_CYCLES`] over [`ANSPCM_PERIOD`] at the 8000
/// symbol/s of clause 5, not printed: 8.3.1 says only "approximately 2100 Hz".
/// It is 0.33 Hz low, and V.8 7.2 allows ANSam 2100 +/- 1 Hz, so a network
/// looking for an answering tone finds one.
pub const ANSPCM_HZ: f64 = 8000.0 * ANSPCM_CYCLES as f64 / ANSPCM_PERIOD as f64;

/// Table 7/V.92, the -9.5 dBm0 sequence (LM `00`), as mu-law octets
/// for k = 0 to 300 with scl = 1334 (rendered p.17).
const TABLE_7_MU: [u8; ANSPCM_PERIOD] = [
    0xA1, 0x58, 0x22, 0xC2, 0xA3, 0x38, 0x25, 0xB0, 0xA7, 0x2C, 0x2A, 0xA9, 0xAE, 0x26, 0x34, 0xA4, 0xBC, 0x22, 0x4B, 0xA2,
    0xFC, 0x22, 0xCB, 0xA2, 0x3C, 0x24, 0xB4, 0xA6, 0x2E, 0x28, 0xAA, 0xAC, 0x27, 0x2F, 0xA5, 0xB8, 0x23, 0x41, 0xA2, 0xD7,
    0x21, 0xDB, 0xA2, 0x43, 0x23, 0xB8, 0xA5, 0x30, 0x27, 0xAC, 0xAA, 0x29, 0x2D, 0xA6, 0xB3, 0x24, 0x3C, 0xA2, 0xCA, 0x22,
    0x72, 0xA2, 0x4D, 0x22, 0xBD, 0xA4, 0x34, 0x26, 0xAE, 0xA8, 0x2A, 0x2B, 0xA7, 0xAF, 0x25, 0x37, 0xA3, 0xC0, 0x22, 0x55,
    0xA2, 0x5D, 0x22, 0xC4, 0xA3, 0x39, 0x24, 0xB1, 0xA7, 0x2C, 0x2A, 0xA9, 0xAD, 0x26, 0x33, 0xA4, 0xBB, 0x22, 0x48, 0xA2,
    0xEC, 0x22, 0xCE, 0xA2, 0x3E, 0x23, 0xB5, 0xA5, 0x2E, 0x28, 0xAB, 0xAB, 0x28, 0x2F, 0xA5, 0xB6, 0x23, 0x3F, 0xA2, 0xD2,
    0x22, 0xE0, 0xA2, 0x45, 0x23, 0xBA, 0xA4, 0x31, 0x27, 0xAD, 0xA9, 0x29, 0x2D, 0xA6, 0xB2, 0x24, 0x3A, 0xA3, 0xC7, 0x22,
    0x67, 0xA2, 0x4F, 0x22, 0xBE, 0xA3, 0x36, 0x25, 0xAF, 0xA8, 0x2B, 0x2B, 0xA8, 0xAF, 0x25, 0x36, 0xA3, 0xBF, 0x22, 0x50,
    0xA2, 0x65, 0x22, 0xC6, 0xA3, 0x3A, 0x24, 0xB2, 0xA6, 0x2D, 0x29, 0xA9, 0xAD, 0x27, 0x31, 0xA4, 0xBA, 0x23, 0x46, 0xA2,
    0xE2, 0x22, 0xD1, 0xA2, 0x3F, 0x23, 0xB6, 0xA5, 0x2F, 0x28, 0xAB, 0xAB, 0x28, 0x2E, 0xA5, 0xB5, 0x23, 0x3E, 0xA2, 0xCE,
    0x22, 0xEA, 0xA2, 0x48, 0x23, 0xBB, 0xA4, 0x32, 0x26, 0xAD, 0xA9, 0x2A, 0x2C, 0xA7, 0xB1, 0x24, 0x39, 0xA3, 0xC5, 0x22,
    0x5E, 0xA2, 0x53, 0x22, 0xBF, 0xA3, 0x37, 0x25, 0xAF, 0xA8, 0x2B, 0x2B, 0xA8, 0xAE, 0x26, 0x35, 0xA4, 0xBD, 0x22, 0x4D,
    0xA2, 0x6F, 0x22, 0xC9, 0xA2, 0x3B, 0x24, 0xB3, 0xA6, 0x2D, 0x29, 0xAA, 0xAC, 0x27, 0x30, 0xA5, 0xB9, 0x23, 0x43, 0xA2,
    0xDC, 0x22, 0xD6, 0xA2, 0x40, 0x23, 0xB7, 0xA5, 0x2F, 0x27, 0xAC, 0xAA, 0x28, 0x2E, 0xA6, 0xB4, 0x24, 0x3D, 0xA2, 0xCC,
    0x22, 0xF7, 0xA2, 0x4A, 0x22, 0xBC, 0xA4, 0x33, 0x26, 0xAE, 0xA9, 0x2A, 0x2C, 0xA7, 0xB0, 0x25, 0x38, 0xA3, 0xC2, 0x22,
    0x59,
];

/// Table 7/V.92, the -9.5 dBm0 sequence (LM `00`), as A-law octets
/// for k = 0 to 300 with scl = 667 (rendered p.17).
///
/// k = 82 is the cell the page prints as a bare "8" where every other cell has
/// two digits; it is `0x08`, and the test says so.
const TABLE_7_A: [u8; ANSPCM_PERIOD] = [
    0x88, 0x76, 0x08, 0xEE, 0x89, 0x13, 0x0F, 0x9B, 0x82, 0x06, 0x01, 0x83, 0x84, 0x0C, 0x1F, 0x8E, 0x97, 0x09, 0x67, 0x88,
    0xD4, 0x08, 0xE7, 0x89, 0x17, 0x0E, 0x9F, 0x8C, 0x04, 0x03, 0x81, 0x86, 0x02, 0x1A, 0x8F, 0x93, 0x0E, 0x69, 0x88, 0xF1,
    0x08, 0xF5, 0x88, 0x6F, 0x09, 0x93, 0x8F, 0x1B, 0x0D, 0x87, 0x80, 0x03, 0x04, 0x8D, 0x9E, 0x0E, 0x17, 0x89, 0xE6, 0x08,
    0x53, 0x88, 0x65, 0x09, 0x94, 0x8E, 0x1F, 0x0C, 0x85, 0x83, 0x01, 0x06, 0x82, 0x9A, 0x0F, 0x12, 0x8E, 0xE8, 0x08, 0x73,
    0x88, 0x49, 0x08, 0xEC, 0x89, 0x10, 0x0F, 0x98, 0x8D, 0x07, 0x00, 0x83, 0x84, 0x0D, 0x1E, 0x8F, 0x96, 0x09, 0x60, 0x88,
    0xDE, 0x08, 0xFA, 0x89, 0x15, 0x0E, 0x9C, 0x8C, 0x05, 0x03, 0x81, 0x86, 0x02, 0x05, 0x8C, 0x9D, 0x0E, 0x6B, 0x89, 0xFC,
    0x08, 0xC2, 0x88, 0x6D, 0x09, 0x91, 0x8F, 0x18, 0x0D, 0x87, 0x80, 0x00, 0x07, 0x8D, 0x99, 0x0F, 0x11, 0x89, 0xE3, 0x08,
    0x45, 0x88, 0x79, 0x09, 0x95, 0x8E, 0x1D, 0x0C, 0x85, 0x82, 0x01, 0x06, 0x82, 0x85, 0x0C, 0x1D, 0x8E, 0xEA, 0x09, 0x7E,
    0x88, 0x47, 0x08, 0xE3, 0x89, 0x11, 0x0F, 0x99, 0x8D, 0x07, 0x00, 0x80, 0x87, 0x0D, 0x18, 0x8F, 0x91, 0x09, 0x62, 0x88,
    0xC0, 0x08, 0xFF, 0x89, 0x6A, 0x0E, 0x9D, 0x8C, 0x05, 0x02, 0x86, 0x81, 0x02, 0x05, 0x8C, 0x9C, 0x0E, 0x15, 0x89, 0xFB,
    0x08, 0xD8, 0x88, 0x60, 0x09, 0x96, 0x8F, 0x19, 0x0D, 0x84, 0x80, 0x00, 0x07, 0x8D, 0x98, 0x0F, 0x10, 0x89, 0xED, 0x08,
    0x4F, 0x88, 0x7D, 0x08, 0xEB, 0x8E, 0x12, 0x0C, 0x9A, 0x82, 0x06, 0x01, 0x83, 0x85, 0x0C, 0x1C, 0x8E, 0x94, 0x09, 0x65,
    0x88, 0x5D, 0x08, 0xE1, 0x89, 0x16, 0x0E, 0x9E, 0x8D, 0x04, 0x03, 0x80, 0x87, 0x0D, 0x1B, 0x8F, 0x90, 0x09, 0x6C, 0x88,
    0xCB, 0x08, 0xF0, 0x88, 0x69, 0x0E, 0x92, 0x8F, 0x1A, 0x02, 0x86, 0x81, 0x03, 0x04, 0x8C, 0x9F, 0x0E, 0x14, 0x89, 0xE4,
    0x08, 0xD6, 0x88, 0x66, 0x09, 0x97, 0x8E, 0x1E, 0x0C, 0x84, 0x83, 0x01, 0x06, 0x82, 0x9B, 0x0F, 0x13, 0x89, 0xEE, 0x08,
    0x74,
];

/// Table 8/V.92, the -12 dBm0 sequence (LM `01`), as mu-law octets
/// for k = 0 to 300 with scl = 1000 (rendered p.18).
const TABLE_8_MU: [u8; ANSPCM_PERIOD] = [
    0xA9, 0x5D, 0x29, 0xC9, 0xAA, 0x3D, 0x2B, 0xB7, 0xAD, 0x32, 0x2F, 0xAE, 0xB4, 0x2C, 0x3A, 0xAB, 0xC2, 0x29, 0x4F, 0xA9,
    0xFD, 0x29, 0xD0, 0xA9, 0x42, 0x2B, 0xBB, 0xAC, 0x35, 0x2E, 0xAF, 0xB2, 0x2D, 0x37, 0xAB, 0xBD, 0x2A, 0x48, 0xA9, 0xDC,
    0x29, 0xDF, 0xA9, 0x4A, 0x2A, 0xBE, 0xAB, 0x38, 0x2D, 0xB2, 0xAF, 0x2E, 0x34, 0xAC, 0xBA, 0x2B, 0x41, 0xAA, 0xCE, 0x29,
    0x76, 0xA9, 0x52, 0x29, 0xC3, 0xAA, 0x3B, 0x2C, 0xB5, 0xAE, 0x30, 0x31, 0xAD, 0xB7, 0x2B, 0x3D, 0xAA, 0xC7, 0x29, 0x5A,
    0xA9, 0x62, 0x29, 0xCA, 0xAA, 0x3E, 0x2B, 0xB8, 0xAD, 0x32, 0x2F, 0xAE, 0xB4, 0x2C, 0x3A, 0xAB, 0xC0, 0x2A, 0x4E, 0xA9,
    0xEF, 0x29, 0xD4, 0xA9, 0x44, 0x2A, 0xBB, 0xAC, 0x35, 0x2E, 0xB0, 0xB1, 0x2D, 0x36, 0xAC, 0xBC, 0x2A, 0x46, 0xA9, 0xD8,
    0x29, 0xE6, 0xA9, 0x4B, 0x2A, 0xBF, 0xAB, 0x39, 0x2D, 0xB3, 0xAF, 0x2F, 0x33, 0xAD, 0xB9, 0x2B, 0x3F, 0xAA, 0xCD, 0x29,
    0x6B, 0xA9, 0x56, 0x29, 0xC5, 0xAA, 0x3C, 0x2C, 0xB6, 0xAE, 0x30, 0x31, 0xAE, 0xB6, 0x2C, 0x3C, 0xAA, 0xC5, 0x29, 0x57,
    0xA9, 0x69, 0x29, 0xCC, 0xAA, 0x3F, 0x2B, 0xB9, 0xAD, 0x33, 0x2F, 0xAF, 0xB3, 0x2D, 0x39, 0xAB, 0xBF, 0x2A, 0x4C, 0xA9,
    0xE7, 0x29, 0xD8, 0xA9, 0x46, 0x2A, 0xBC, 0xAC, 0x36, 0x2E, 0xB1, 0xB0, 0x2E, 0x36, 0xAC, 0xBC, 0x2A, 0x45, 0xA9, 0xD5,
    0x29, 0xED, 0xA9, 0x4D, 0x2A, 0xC0, 0xAB, 0x39, 0x2C, 0xB4, 0xAF, 0x2F, 0x33, 0xAD, 0xB8, 0x2B, 0x3F, 0xAA, 0xCB, 0x29,
    0x64, 0xA9, 0x59, 0x29, 0xC7, 0xAA, 0x3D, 0x2C, 0xB7, 0xAD, 0x31, 0x30, 0xAE, 0xB5, 0x2C, 0x3B, 0xAA, 0xC4, 0x29, 0x53,
    0xA9, 0x72, 0x29, 0xCE, 0xAA, 0x41, 0x2B, 0xBA, 0xAC, 0x34, 0x2E, 0xAF, 0xB2, 0x2D, 0x38, 0xAB, 0xBE, 0x2A, 0x4A, 0xA9,
    0xE0, 0x29, 0xDB, 0xA9, 0x48, 0x2A, 0xBD, 0xAB, 0x37, 0x2D, 0xB1, 0xB0, 0x2E, 0x35, 0xAC, 0xBB, 0x2A, 0x43, 0xA9, 0xD1,
    0x29, 0xF9, 0xA9, 0x4F, 0x2A, 0xC2, 0xAB, 0x3A, 0x2C, 0xB4, 0xAE, 0x2F, 0x32, 0xAD, 0xB8, 0x2B, 0x3E, 0xAA, 0xC9, 0x29,
    0x5E,
];

/// Table 8/V.92, the -12 dBm0 sequence (LM `01`), as A-law octets
/// for k = 0 to 300 with scl = 500 (rendered p.18).
const TABLE_8_A: [u8; ANSPCM_PERIOD] = [
    0x83, 0x49, 0x00, 0xE1, 0x80, 0x15, 0x06, 0x92, 0x84, 0x19, 0x1A, 0x85, 0x9F, 0x07, 0x11, 0x81, 0xEE, 0x00, 0x79, 0x83,
    0xD4, 0x03, 0xFE, 0x80, 0x6E, 0x01, 0x96, 0x87, 0x1C, 0x05, 0x9A, 0x99, 0x04, 0x12, 0x86, 0x94, 0x01, 0x60, 0x80, 0xCB,
    0x03, 0xCC, 0x80, 0x66, 0x00, 0x95, 0x86, 0x13, 0x07, 0x99, 0x9A, 0x05, 0x1F, 0x87, 0x91, 0x01, 0x69, 0x80, 0xF8, 0x03,
    0x51, 0x83, 0x7C, 0x00, 0xEF, 0x81, 0x16, 0x06, 0x9C, 0x84, 0x1B, 0x18, 0x84, 0x92, 0x06, 0x14, 0x81, 0xE3, 0x00, 0x74,
    0x83, 0x40, 0x00, 0xE6, 0x80, 0x15, 0x06, 0x93, 0x87, 0x19, 0x1A, 0x85, 0x9F, 0x07, 0x11, 0x81, 0xE8, 0x00, 0x7A, 0x80,
    0xDD, 0x03, 0xF2, 0x80, 0x6C, 0x01, 0x96, 0x86, 0x1C, 0x04, 0x9B, 0x98, 0x04, 0x1D, 0x86, 0x97, 0x01, 0x62, 0x80, 0xF6,
    0x03, 0xC4, 0x80, 0x67, 0x00, 0xEA, 0x86, 0x10, 0x07, 0x9E, 0x85, 0x05, 0x1E, 0x87, 0x90, 0x01, 0x6B, 0x80, 0xE5, 0x00,
    0x59, 0x83, 0x70, 0x00, 0xED, 0x81, 0x17, 0x06, 0x9D, 0x84, 0x1B, 0x18, 0x84, 0x9D, 0x06, 0x17, 0x81, 0xE2, 0x00, 0x71,
    0x83, 0x5B, 0x00, 0xE4, 0x80, 0x6B, 0x01, 0x90, 0x87, 0x1E, 0x05, 0x85, 0x9E, 0x07, 0x10, 0x81, 0xEB, 0x00, 0x64, 0x80,
    0xDA, 0x03, 0xF6, 0x80, 0x62, 0x01, 0x97, 0x86, 0x1D, 0x04, 0x98, 0x9B, 0x04, 0x1D, 0x86, 0x97, 0x01, 0x6D, 0x80, 0xF3,
    0x03, 0xDF, 0x80, 0x65, 0x00, 0xE8, 0x81, 0x10, 0x07, 0x9F, 0x85, 0x05, 0x1E, 0x87, 0x93, 0x06, 0x6A, 0x80, 0xE7, 0x00,
    0x46, 0x83, 0x77, 0x00, 0xE3, 0x81, 0x14, 0x06, 0x92, 0x84, 0x18, 0x1B, 0x84, 0x9C, 0x06, 0x16, 0x81, 0xEC, 0x00, 0x7D,
    0x83, 0x53, 0x03, 0xFB, 0x80, 0x69, 0x01, 0x91, 0x87, 0x1F, 0x05, 0x9A, 0x99, 0x07, 0x13, 0x86, 0x95, 0x00, 0x66, 0x80,
    0xC2, 0x03, 0xF5, 0x80, 0x60, 0x01, 0x94, 0x86, 0x12, 0x04, 0x98, 0x9B, 0x05, 0x1C, 0x87, 0x96, 0x01, 0x6F, 0x80, 0xFF,
    0x03, 0xD6, 0x83, 0x78, 0x00, 0xEE, 0x81, 0x11, 0x07, 0x9F, 0x85, 0x1A, 0x19, 0x84, 0x93, 0x06, 0x15, 0x80, 0xE1, 0x00,
    0x4F,
];

/// Table 9/V.92, the -15 dBm0 sequence (LM `10`), as mu-law octets
/// for k = 0 to 300 with scl = 708 (rendered p.19).
const TABLE_9_MU: [u8; ANSPCM_PERIOD] = [
    0xAF, 0x63, 0x30, 0xCF, 0xB1, 0x45, 0x33, 0xBE, 0xB5, 0x3A, 0x38, 0xB7, 0xBC, 0x34, 0x41, 0xB2, 0xCA, 0x30, 0x57, 0xAF,
    0xFD, 0x2F, 0xD8, 0xB0, 0x4A, 0x31, 0xC1, 0xB4, 0x3C, 0x37, 0xB8, 0xBA, 0x35, 0x3E, 0xB3, 0xC5, 0x31, 0x4E, 0xB0, 0xE2,
    0x2F, 0xE6, 0xB0, 0x4F, 0x31, 0xC6, 0xB2, 0x3E, 0x35, 0xBA, 0xB8, 0x37, 0x3C, 0xB4, 0xC0, 0x32, 0x49, 0xB0, 0xD6, 0x2F,
    0x78, 0xAF, 0x59, 0x30, 0xCB, 0xB1, 0x42, 0x34, 0xBC, 0xB6, 0x39, 0x3A, 0xB5, 0xBE, 0x33, 0x44, 0xB1, 0xCE, 0x30, 0x5F,
    0xAF, 0x68, 0x2F, 0xD0, 0xB1, 0x47, 0x32, 0xBF, 0xB5, 0x3B, 0x38, 0xB7, 0xBC, 0x34, 0x40, 0xB2, 0xC9, 0x30, 0x55, 0xAF,
    0xF3, 0x2F, 0xDB, 0xB0, 0x4C, 0x31, 0xC2, 0xB3, 0x3D, 0x36, 0xB9, 0xBA, 0x36, 0x3D, 0xB3, 0xC4, 0x31, 0x4D, 0xB0, 0xDE,
    0x2F, 0xEB, 0xAF, 0x52, 0x30, 0xC7, 0xB2, 0x3F, 0x35, 0xBB, 0xB8, 0x37, 0x3B, 0xB4, 0xBF, 0x32, 0x48, 0xB0, 0xD4, 0x2F,
    0x6F, 0xAF, 0x5C, 0x30, 0xCC, 0xB1, 0x43, 0x33, 0xBD, 0xB6, 0x39, 0x39, 0xB6, 0xBD, 0x33, 0x43, 0xB1, 0xCC, 0x30, 0x5D,
    0xAF, 0x6D, 0x2F, 0xD3, 0xB0, 0x48, 0x32, 0xBF, 0xB4, 0x3B, 0x37, 0xB8, 0xBB, 0x34, 0x3F, 0xB2, 0xC8, 0x30, 0x52, 0xAF,
    0xEC, 0x2F, 0xDD, 0xB0, 0x4D, 0x31, 0xC4, 0xB3, 0x3D, 0x36, 0xB9, 0xB9, 0x36, 0x3D, 0xB3, 0xC3, 0x31, 0x4C, 0xB0, 0xDB,
    0x2F, 0xF0, 0xAF, 0x54, 0x30, 0xC8, 0xB2, 0x3F, 0x34, 0xBB, 0xB7, 0x38, 0x3B, 0xB5, 0xBF, 0x32, 0x47, 0xB0, 0xD1, 0x2F,
    0x69, 0xAF, 0x5F, 0x30, 0xCD, 0xB1, 0x44, 0x33, 0xBE, 0xB6, 0x3A, 0x39, 0xB6, 0xBD, 0x33, 0x42, 0xB1, 0xCB, 0x30, 0x5A,
    0xAF, 0x76, 0x2F, 0xD6, 0xB0, 0x49, 0x32, 0xC0, 0xB4, 0x3C, 0x37, 0xB8, 0xBB, 0x35, 0x3F, 0xB2, 0xC6, 0x31, 0x50, 0xAF,
    0xE7, 0x2F, 0xE0, 0xB0, 0x4E, 0x31, 0xC5, 0xB3, 0x3E, 0x35, 0xBA, 0xB9, 0x36, 0x3C, 0xB4, 0xC2, 0x31, 0x4B, 0xB0, 0xD9,
    0x2F, 0xFB, 0xAF, 0x57, 0x30, 0xCA, 0xB2, 0x41, 0x34, 0xBC, 0xB7, 0x38, 0x3A, 0xB5, 0xBE, 0x33, 0x46, 0xB1, 0xCF, 0x30,
    0x64,
];

/// Table 9/V.92, the -15 dBm0 sequence (LM `10`), as A-law octets
/// for k = 0 to 300 with scl = 354 (rendered p.19).
const TABLE_9_A: [u8; ANSPCM_PERIOD] = [
    0x9A, 0x41, 0x1B, 0xF8, 0x98, 0x6D, 0x1E, 0x95, 0x9C, 0x11, 0x13, 0x92, 0x97, 0x1F, 0x69, 0x99, 0xE6, 0x1B, 0x76, 0x9A,
    0xD5, 0x1A, 0xF6, 0x9B, 0x66, 0x19, 0xE9, 0x9F, 0x17, 0x12, 0x93, 0x91, 0x1C, 0x15, 0x9E, 0xED, 0x18, 0x7B, 0x9B, 0xC0,
    0x1A, 0xC4, 0x9B, 0x79, 0x18, 0xE2, 0x99, 0x15, 0x1C, 0x91, 0x93, 0x12, 0x17, 0x9F, 0xE8, 0x19, 0x61, 0x9B, 0xF0, 0x1A,
    0x56, 0x9A, 0x77, 0x1B, 0xE7, 0x98, 0x6E, 0x1F, 0x97, 0x9D, 0x10, 0x11, 0x9C, 0x95, 0x1E, 0x6D, 0x98, 0xFA, 0x1B, 0x4D,
    0x9A, 0x5A, 0x1A, 0xFE, 0x98, 0x63, 0x19, 0xEA, 0x9C, 0x16, 0x13, 0x92, 0x97, 0x1F, 0x68, 0x99, 0xE1, 0x1B, 0x73, 0x9A,
    0xD3, 0x1A, 0xF5, 0x9B, 0x64, 0x18, 0xEE, 0x9E, 0x14, 0x1D, 0x90, 0x91, 0x1D, 0x14, 0x9E, 0xEC, 0x18, 0x65, 0x9B, 0xCF,
    0x1A, 0xD9, 0x9A, 0x7C, 0x1B, 0xE3, 0x99, 0x6A, 0x1C, 0x96, 0x93, 0x12, 0x16, 0x9F, 0xEB, 0x19, 0x60, 0x9B, 0xF2, 0x1A,
    0x5D, 0x9A, 0x4B, 0x1B, 0xE4, 0x98, 0x6F, 0x1E, 0x94, 0x9D, 0x10, 0x10, 0x9D, 0x94, 0x1E, 0x6F, 0x98, 0xE5, 0x1B, 0x48,
    0x9A, 0x5F, 0x1A, 0xFD, 0x9B, 0x60, 0x19, 0xEB, 0x9F, 0x16, 0x12, 0x93, 0x96, 0x1C, 0x6B, 0x99, 0xE0, 0x1B, 0x7C, 0x9A,
    0xDE, 0x1A, 0xC9, 0x9B, 0x65, 0x18, 0xEC, 0x9E, 0x14, 0x1D, 0x90, 0x90, 0x1D, 0x14, 0x9E, 0xEF, 0x18, 0x64, 0x9B, 0xF5,
    0x1A, 0xD2, 0x9A, 0x72, 0x1B, 0xE0, 0x99, 0x68, 0x1F, 0x96, 0x92, 0x13, 0x16, 0x9C, 0xEA, 0x19, 0x63, 0x9B, 0xFF, 0x1A,
    0x58, 0x9A, 0x4C, 0x1B, 0xE5, 0x98, 0x6C, 0x1E, 0x95, 0x9D, 0x11, 0x10, 0x9D, 0x94, 0x1E, 0x6E, 0x98, 0xE7, 0x1B, 0x74,
    0x9A, 0x51, 0x1A, 0xF0, 0x9B, 0x61, 0x19, 0xE8, 0x9F, 0x17, 0x12, 0x93, 0x96, 0x1C, 0x6A, 0x99, 0xE2, 0x18, 0x7E, 0x9B,
    0xC5, 0x1A, 0xC2, 0x9B, 0x7B, 0x18, 0xED, 0x9E, 0x15, 0x1C, 0x91, 0x90, 0x1D, 0x17, 0x9F, 0xEE, 0x18, 0x67, 0x9B, 0xF7,
    0x1A, 0xD7, 0x9A, 0x71, 0x1B, 0xE6, 0x99, 0x69, 0x1F, 0x97, 0x92, 0x13, 0x11, 0x9C, 0x95, 0x1E, 0x62, 0x98, 0xF9, 0x1B,
    0x46,
];

/// Table 10/V.92, the -18 dBm0 sequence (LM `11`), as mu-law octets
/// for k = 0 to 300 with scl = 500 (rendered p.20).
const TABLE_10_MU: [u8; ANSPCM_PERIOD] = [
    0xB8, 0x69, 0x39, 0xD7, 0xB9, 0x4C, 0x3B, 0xC6, 0xBD, 0x41, 0x3F, 0xBE, 0xC3, 0x3C, 0x49, 0xBA, 0xD0, 0x39, 0x5D, 0xB8,
    0xFE, 0x38, 0xDE, 0xB9, 0x50, 0x3A, 0xCA, 0xBC, 0x44, 0x3E, 0xBF, 0xC1, 0x3D, 0x46, 0xBB, 0xCC, 0x39, 0x56, 0xB9, 0xE8,
    0x38, 0xEB, 0xB9, 0x57, 0x39, 0xCD, 0xBB, 0x47, 0x3C, 0xC1, 0xBF, 0x3E, 0x43, 0xBC, 0xC9, 0x3A, 0x4F, 0xB9, 0xDC, 0x38,
    0x7A, 0xB8, 0x5F, 0x39, 0xD1, 0xBA, 0x4A, 0x3B, 0xC4, 0xBD, 0x3F, 0x40, 0xBD, 0xC6, 0x3B, 0x4C, 0xBA, 0xD5, 0x39, 0x66,
    0xB8, 0x6D, 0x39, 0xD8, 0xB9, 0x4D, 0x3B, 0xC7, 0xBC, 0x41, 0x3F, 0xBE, 0xC3, 0x3C, 0x48, 0xBA, 0xCF, 0x39, 0x5B, 0xB8,
    0xF6, 0x38, 0xE0, 0xB9, 0x52, 0x3A, 0xCA, 0xBB, 0x44, 0x3D, 0xBF, 0xC0, 0x3D, 0x45, 0xBB, 0xCB, 0x3A, 0x54, 0xB9, 0xE4,
    0x38, 0xEE, 0xB9, 0x59, 0x39, 0xCE, 0xBA, 0x47, 0x3C, 0xC2, 0xBE, 0x3E, 0x42, 0xBC, 0xC8, 0x3A, 0x4E, 0xB9, 0xDB, 0x38,
    0x73, 0xB8, 0x61, 0x39, 0xD3, 0xBA, 0x4B, 0x3B, 0xC5, 0xBD, 0x3F, 0x3F, 0xBD, 0xC5, 0x3B, 0x4B, 0xBA, 0xD3, 0x39, 0x62,
    0xB8, 0x71, 0x39, 0xDA, 0xB9, 0x4E, 0x3A, 0xC8, 0xBC, 0x42, 0x3E, 0xBE, 0xC2, 0x3C, 0x48, 0xBA, 0xCE, 0x39, 0x5A, 0xB9,
    0xEF, 0x38, 0xE3, 0xB9, 0x54, 0x3A, 0xCB, 0xBB, 0x45, 0x3D, 0xC0, 0xBF, 0x3D, 0x45, 0xBB, 0xCB, 0x3A, 0x53, 0xB9, 0xE1,
    0x38, 0xF5, 0xB8, 0x5B, 0x39, 0xCF, 0xBA, 0x48, 0x3C, 0xC2, 0xBE, 0x3E, 0x42, 0xBC, 0xC7, 0x3B, 0x4E, 0xB9, 0xD9, 0x39,
    0x6D, 0xB8, 0x65, 0x39, 0xD5, 0xBA, 0x4C, 0x3B, 0xC6, 0xBD, 0x40, 0x3F, 0xBD, 0xC4, 0x3B, 0x4A, 0xBA, 0xD2, 0x39, 0x5F,
    0xB8, 0x78, 0x38, 0xDC, 0xB9, 0x4F, 0x3A, 0xC9, 0xBC, 0x43, 0x3E, 0xBF, 0xC1, 0x3C, 0x47, 0xBB, 0xCD, 0x39, 0x58, 0xB9,
    0xEC, 0x38, 0xE7, 0xB9, 0x56, 0x3A, 0xCC, 0xBB, 0x46, 0x3D, 0xC0, 0xBF, 0x3E, 0x44, 0xBC, 0xCA, 0x3A, 0x51, 0xB9, 0xDE,
    0x38, 0xFC, 0xB8, 0x5D, 0x39, 0xCF, 0xBA, 0x49, 0x3C, 0xC3, 0xBE, 0x3F, 0x41, 0xBD, 0xC6, 0x3B, 0x4D, 0xB9, 0xD7, 0x39,
    0x6A,
];

/// Table 10/V.92, the -18 dBm0 sequence (LM `11`), as A-law octets
/// for k = 0 to 300 with scl = 250 (rendered p.20).
const TABLE_10_A: [u8; ANSPCM_PERIOD] = [
    0x93, 0x5B, 0x10, 0xF1, 0x90, 0x64, 0x16, 0xE2, 0x94, 0x69, 0x6A, 0x95, 0xEF, 0x17, 0x61, 0x91, 0xFE, 0x10, 0x49, 0x93,
    0xD5, 0x13, 0xCE, 0x90, 0x7E, 0x11, 0xE6, 0x97, 0x6C, 0x15, 0xEA, 0xE9, 0x14, 0x62, 0x96, 0xE4, 0x10, 0x70, 0x90, 0xDA,
    0x13, 0xD9, 0x90, 0x71, 0x10, 0xE5, 0x96, 0x63, 0x17, 0xE9, 0xEA, 0x15, 0x6F, 0x97, 0xE1, 0x11, 0x79, 0x90, 0xCB, 0x13,
    0x57, 0x93, 0x4C, 0x10, 0xFF, 0x91, 0x66, 0x16, 0xEC, 0x94, 0x6B, 0x68, 0x94, 0xE2, 0x16, 0x64, 0x91, 0xF3, 0x10, 0x44,
    0x93, 0x5F, 0x10, 0xF6, 0x90, 0x65, 0x16, 0xE3, 0x97, 0x69, 0x6A, 0x95, 0xEF, 0x17, 0x61, 0x91, 0xF8, 0x10, 0x4A, 0x93,
    0xD1, 0x13, 0xC2, 0x90, 0x7C, 0x11, 0xE6, 0x96, 0x6C, 0x14, 0xEB, 0xE8, 0x14, 0x6D, 0x96, 0xE7, 0x11, 0x72, 0x90, 0xC6,
    0x13, 0xDC, 0x90, 0x77, 0x10, 0xFA, 0x91, 0x60, 0x17, 0xEE, 0x95, 0x15, 0x6E, 0x97, 0xE0, 0x11, 0x7B, 0x90, 0xF5, 0x10,
    0x53, 0x93, 0x40, 0x10, 0xFD, 0x91, 0x67, 0x16, 0xED, 0x94, 0x6B, 0x68, 0x94, 0xED, 0x16, 0x67, 0x91, 0xFD, 0x10, 0x41,
    0x93, 0x52, 0x10, 0xF4, 0x90, 0x7B, 0x11, 0xE0, 0x97, 0x6E, 0x15, 0x95, 0xEE, 0x17, 0x60, 0x91, 0xFA, 0x10, 0x74, 0x90,
    0xDD, 0x13, 0xC1, 0x90, 0x72, 0x11, 0xE7, 0x96, 0x6D, 0x14, 0xE8, 0xEB, 0x14, 0x6D, 0x96, 0xE7, 0x11, 0x7D, 0x90, 0xC3,
    0x13, 0xD0, 0x93, 0x75, 0x10, 0xF8, 0x91, 0x60, 0x17, 0xEF, 0x95, 0x15, 0x6E, 0x97, 0xE3, 0x16, 0x7A, 0x90, 0xF7, 0x10,
    0x5C, 0x93, 0x47, 0x10, 0xF3, 0x91, 0x64, 0x16, 0xE2, 0x94, 0x68, 0x6B, 0x94, 0xEC, 0x16, 0x66, 0x91, 0xFC, 0x10, 0x4D,
    0x93, 0x56, 0x13, 0xCB, 0x90, 0x79, 0x11, 0xE1, 0x97, 0x6F, 0x15, 0xEA, 0xE9, 0x17, 0x63, 0x96, 0xE5, 0x10, 0x76, 0x90,
    0xDE, 0x13, 0xC5, 0x90, 0x70, 0x11, 0xE4, 0x96, 0x62, 0x14, 0xE8, 0xEB, 0x15, 0x6C, 0x97, 0xE6, 0x11, 0x7F, 0x90, 0xCF,
    0x13, 0xD4, 0x93, 0x48, 0x10, 0xF9, 0x91, 0x61, 0x17, 0xEF, 0x95, 0x6A, 0x69, 0x94, 0xE3, 0x16, 0x65, 0x90, 0xF1, 0x10,
    0x58,
];

/// The 301 octets ANSpcm repeats at `level` on a network of this `law`.
///
/// Tables 7 to 10 are printed as octets in the form of Table 1/V.90 --
/// "the mu-law and A-law codewords are the octets to be passed to the digital
/// interface" (3.6) -- so what is here goes out unaltered, and nothing above
/// should invert it again. The tables' own heading calls the thing a "Ucode
/// sequence"; it is not, and reading it as one sends a signal that is nothing
/// like an answering tone.
pub fn anspcm_table(level: AnspcmLevel, law: Law) -> &'static [u8; ANSPCM_PERIOD] {
    match (level, law) {
        (AnspcmLevel::Minus9_5, Law::Mu) => &TABLE_7_MU,
        (AnspcmLevel::Minus9_5, Law::A) => &TABLE_7_A,
        (AnspcmLevel::Minus12, Law::Mu) => &TABLE_8_MU,
        (AnspcmLevel::Minus12, Law::A) => &TABLE_8_A,
        (AnspcmLevel::Minus15, Law::Mu) => &TABLE_9_MU,
        (AnspcmLevel::Minus15, Law::A) => &TABLE_9_A,
        (AnspcmLevel::Minus18, Law::Mu) => &TABLE_10_MU,
        (AnspcmLevel::Minus18, Law::A) => &TABLE_10_A,
    }
}

/// Whether symbol `n` of ANSpcm falls in a reversed block, counting n = 0 from
/// the first ANSpcm symbol.
pub fn anspcm_reversed(n: u64) -> bool {
    let block = n / ANSPCM_REVERSAL as u64;
    block.is_multiple_of(2) != ANSPCM_TABLE_POLARITY_FIRST
}

/// Symbol `n` of ANSpcm, counting n = 0 from the first ANSpcm symbol.
///
/// A phase reversal of a sampled tone negates every sample, and with the
/// symmetric G.711 encodings of Tables 7 to 10 that is exactly the polarity
/// bit: `octet ^ 0x80` in both laws, since A-law's `0x55` mask does not touch
/// bit 7. Re-quantising the negated value would be the same answer more slowly
/// -- no value in the tables sits on a rounding boundary -- and adding pi
/// inside the cosine gives the identical sequence.
pub fn anspcm_octet(level: AnspcmLevel, law: Law, n: u64) -> u8 {
    let k = (n % ANSPCM_PERIOD as u64) as usize;
    let octet = anspcm_table(level, law)[k];
    if anspcm_reversed(n) { octet ^ 0x80 } else { octet }
}

// ---------------------------------------------------------------------------
// QTS and QTS-bar (8.3.6)
// ---------------------------------------------------------------------------

/// The six-symbol pattern QTS repeats, and with it the downstream data frame:
/// "a data frame is 6 symbols" (clause 5, through V.90 5.4).
pub const QTS_PATTERN: usize = crate::v90::INTERVALS;

/// "Signal QTS consists of 128 repetitions of the sequence" (8.3.6).
pub const QTS_REPEATS: usize = 128;

/// "QTS\\ consists of 8 repetitions" of the same sequence inverted (8.3.6).
pub const QTS_BAR_REPEATS: usize = 8;

/// QTS, in symbols: 768T, which Figures 3 to 6 label and which is 96 ms.
pub const QTS_SYMBOLS: usize = QTS_REPEATS * QTS_PATTERN;

/// QTS-bar, in symbols: 48T, 6 ms.
pub const QTS_BAR_SYMBOLS: usize = QTS_BAR_REPEATS * QTS_PATTERN;

/// The two together, after which ANSpcm begins with no gap: 816 symbols, all
/// of them whole data frames, so ANSpcm also starts in interval 0.
pub const QTS_TOTAL: usize = QTS_SYMBOLS + QTS_BAR_SYMBOLS;

/// One symbol of the QTS pattern: interval `i` of `{+V, +0, +V, -V, -0, -V}`,
/// or of its inverse `{-V, -0, -V, +V, +0, +V}` when `inverted` (8.3.6).
///
/// `uqts` is the Ucode the analogue modem asked for in WXYZ, which
/// `v8::quick::Uqts::ucode` gives; the cleardown pattern `1111` names no Ucode
/// and never reaches here. The zeros are "the PCM codeword with Ucode 0", and
/// the positive and negative ones are *different octets* -- mu-law `FF` and
/// `7F`, A-law `D5` and `55` -- so they cannot be collapsed into one silence.
/// On A-law they are not even silent: Ucode 0 is linear +8 and -8.
pub fn qts_symbol(law: Law, uqts: u8, i: usize, inverted: bool) -> u8 {
    // {+V, +0, +V, -V, -0, -V}: the pattern satisfies x(i + 3) = -x(i), which
    // is what leaves it with energy only at 1333.3 Hz and at 4000 Hz.
    let (ucode, positive) = match i % QTS_PATTERN {
        0 => (uqts, true),
        1 => (0, true),
        2 => (uqts, true),
        3 => (uqts, false),
        4 => (0, false),
        _ => (uqts, false),
    };
    // QTS-bar is QTS with every sign turned over, which for a pattern whose
    // second half is already the first half negated is the same as starting
    // three symbols along -- so nothing inside either signal tells them
    // apart, and it is the join at symbol 768 that marks the reversal.
    let negative = !(positive ^ inverted);
    ucode::octet(law, ucode, negative)
}

/// Symbol `n` of QTS and then QTS-bar, or `None` once both are done.
///
/// n = 0 is the first QTS symbol, which 8.3.6 puts in data frame interval 0
/// and from which "the digital modem shall keep data frame alignment ... on".
/// That makes n itself the frame counter: interval `n % 6`, with the QTS to
/// QTS-bar inversion at n = 768 landing on a frame boundary and ANSpcm
/// starting at n = 816 on another.
pub fn qts_octet(law: Law, uqts: u8, n: u64) -> Option<u8> {
    let n = usize::try_from(n).ok()?;
    match n {
        n if n < QTS_SYMBOLS => Some(qts_symbol(law, uqts, n, false)),
        n if n < QTS_TOTAL => Some(qts_symbol(law, uqts, n, true)),
        _ => None,
    }
}

/// The codeword the digital modem is silent with.
///
/// 9.2 never says what silence is, but 9.8.1.1.3 does, for the silent period
/// of a rate renegotiation: the digital modem sends codewords of Ucode 0
/// magnitude and keeps data frame alignment while it does. The same rule is
/// taken for the 75 +/- 5 ms silences of short Phase 1 and for the silence
/// after ANSpcm (section 4). Constant *positive* Ucode 0, so that the silence
/// is one steady octet rather than a signal of its own, and a PCM modem that
/// has "stopped transmitting" still clocks octets out.
pub fn silence_octet(law: Law) -> u8 {
    ucode::octet(law, 0, false)
}

// ---------------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------------

/// The level of a sine wave that just reaches G.711's mu-law overload point,
/// in dBm0.
///
/// G.711 is not in `docs/specs` and V.92 never states this, so this and its
/// A-law twin are the only two numbers in the module that come from neither.
/// What holds them in place is a test: with these, [`anspcm_power_dbm0`]
/// reproduces every figure of the measured table in P1D ANS-17 to a hundredth
/// of a decibel, and those figures in turn sit within 0.11 dB of the four
/// levels Table 6 names.
pub const OVERLOAD_MU_DBM0: f64 = 3.17;

/// The same for A-law. See [`OVERLOAD_MU_DBM0`].
///
/// It is 3.14 decibels and not pi, which clippy is right to ask about and
/// wrong about here.
#[allow(clippy::approx_constant)]
pub const OVERLOAD_A_DBM0: f64 = 3.14;

/// Where G.711's mu-law scale overloads, on the sixteen-bit scale Table 1/V.90
/// prints -- which is four times G.711's own, so 8159 becomes 32 636.
///
/// It is *not* `ucode::linear(Law::Mu, 127)`, which is 32 124: that is the
/// value the decoder reconstructs for the loudest code, the middle of its
/// interval, and using it as the overload point would report every level
/// 0.14 dB loud. The 8159 is the top of that interval, and it is the same
/// 8159 the quantiser of 8.3.1 clamps to (P1D 3.4).
pub const OVERLOAD_MU: f64 = 8159.0 * 4.0;

/// Where G.711's A-law scale overloads, on Table 1/V.90's scale: 4096 at eight
/// times, which is exactly full scale. See [`OVERLOAD_MU`].
pub const OVERLOAD_A: f64 = 4096.0 * 8.0;

/// The root-mean-square level, as a fraction of full scale, that is 0 dBm0.
///
/// Everything else in this repository measures a line in fractions of full
/// scale, and Table 1/V.90's linear column is on a sixteen-bit scale, so this
/// is the divisor that turns one into the other: the overload sine, less the
/// decibels the overload point stands above 0 dBm0.
pub fn zero_dbm0(law: Law) -> f64 {
    let (peak, overload) = match law {
        Law::Mu => (OVERLOAD_MU, OVERLOAD_MU_DBM0),
        Law::A => (OVERLOAD_A, OVERLOAD_A_DBM0),
    };
    (peak / 32768.0) / std::f64::consts::SQRT_2 / 10.0f64.powf(overload / 20.0)
}

/// The power of a run of codewords, in dBm0, measured from what G.711
/// reconstructs rather than from what was aimed at.
///
/// This is what the far end hears, and it is not quite what the level says:
/// the quantiser moves every sample a little, and at -15 dBm0 on mu-law that
/// comes to a twentieth of a decibel.
pub fn power_dbm0(law: Law, octets: &[u8]) -> f64 {
    if octets.is_empty() {
        return f64::NEG_INFINITY;
    }
    let sum: f64 = octets
        .iter()
        .map(|&octet| {
            let (ucode, _) = ucode::from_octet(law, octet);
            let level = ucode::level(law, ucode);
            level * level
        })
        .sum();
    let rms = (sum / octets.len() as f64).sqrt();
    20.0 * (rms / zero_dbm0(law)).log10()
}

/// The power ANSpcm actually goes out at, for one of Table 6's four levels.
///
/// The whole of a period is one measurement, which is exact: the sequence is
/// 79 whole cycles of tone in 301 symbols, so there is no part-cycle to
/// average badly.
pub fn anspcm_power_dbm0(level: AnspcmLevel, law: Law) -> f64 {
    power_dbm0(law, anspcm_table(level, law))
}

// ---------------------------------------------------------------------------
// TONEq (8.2.5)
// ---------------------------------------------------------------------------

/// "Signal TONEq is a 980 Hz tone" (8.2.5). That is the whole clause: no
/// level, no tolerance, no duration and no phase.
///
/// It is written out rather than taken from [`crate::v8::LOW`] because V.92
/// defines it on its own and a V.21 channel has nothing to do with it -- but
/// the two are the same number, and that is the whole difficulty this end of
/// the module has. V.21 clause 3, from the rendered page: channel 1's mean is
/// 1080 Hz, the deviation is +/- 100 Hz, and "the higher characteristic
/// frequency (FA) corresponds to a binary 0", so a mark is 980 Hz and a space
/// is 1180 Hz.
pub const TONEQ_HZ: f64 = 980.0;

/// How loudly TONEq goes out, as a fraction of full scale.
///
/// 8.2.5 gives no level. V.90 8.1 has the Phase 1 signals sent at the nominal
/// transmit power, so this is the figure `datapump::v8` already uses for
/// ANSam, and for the same reason: the Recommendations talk about power into a
/// line, and this leaves the data pumps' own headroom.
pub const TONEQ_LEVEL: f64 = 0.35;

/// The least TONEq the far end will ever send: "at least 50 ms" (9.2.1.3,
/// 9.2.3.3).
///
/// A detector slower than this would still work, because the analogue modem
/// goes on sending TONEq "until ANSpcm is no longer detected" and ANSpcm stops
/// only when TONEq has been heard -- so the sender waits for the receiver.
/// It is here as the floor a conforming far end is entitled to assume.
pub const TONEQ_MINIMUM: f64 = 0.050;

/// How long [`ToneqDetector`] wants 980 Hz to stand still before it believes
/// it, in seconds.
///
/// 980 Hz is also V.21(L)'s mark frequency, and a modem holding a mark is
/// indistinguishable from TONEq by frequency alone. The longest run of marks
/// short Phase 1 can put on the line is the fourteen inside a QC1a whose U_QTS
/// is `1111` -- bits 26 to 39, its four one-bits running straight into the ten
/// ONEs of the next frame -- which is 46.7 ms at 300 bit/s. Sixty milliseconds
/// clears that with room (P1A 11.6).
///
/// It is longer than [`TONEQ_MINIMUM`], which looks like a hazard and is not:
/// 50 ms is a floor on TONEq, not a length. The analogue modem holds it "until
/// ANSpcm is no longer detected", and ANSpcm stops only once TONEq has been
/// heard, so a slower ear lengthens TONEq and nothing else.
pub const TONEQ_STEADY: f64 = 0.060;

/// How far 980 Hz has to stand above everything else on the line before it is
/// a tone rather than the skirt of one.
///
/// A sine wave's amplitude is `pi/2` times its own mean rectified value, so a
/// clean tone measures about 1.57 against this and nothing else comes near.
const TONEQ_STANDING: f64 = 0.75;

/// The quietest 980 Hz worth calling a tone, as a fraction of full scale.
const TONEQ_AUDIBLE: f64 = 0.002;

/// The most 1180 Hz there may be, against the 980 Hz, for the line to be
/// holding a tone rather than carrying V.21(L) data.
///
/// A V.21(L) transmitter is at one frequency or the other, never at neither,
/// so a quarter is a wide margin -- what it has to survive is the tail of the
/// last space bit leaking through a detector that takes a few milliseconds to
/// forget it.
const TONEQ_SPACE_SHARE: f64 = 0.25;

/// How wide each of the two detectors is, in hertz.
///
/// The mark and space of V.21(L) are 200 Hz apart, so a detector this wide
/// hears about a twelfth of the other one -- well under
/// [`TONEQ_SPACE_SHARE`] either way round.
const TONEQ_BANDWIDTH: f64 = 60.0;

/// TONEq, as the analogue modem sends it: exactly 980 Hz for as long as it is
/// stepped.
#[derive(Debug, Clone)]
pub struct ToneqGenerator {
    nco: Nco,
    level: f64,
}

impl ToneqGenerator {
    /// At [`TONEQ_LEVEL`].
    pub fn new(fs: f64) -> Self {
        Self { nco: Nco::new(TONEQ_HZ, fs), level: TONEQ_LEVEL }
    }

    /// At a level of this end's choosing, for a modem whose transmit power has
    /// been turned down.
    pub fn with_level(mut self, level: f64) -> Self {
        self.level = level;
        self
    }

    /// One line sample.
    pub fn step(&mut self) -> f64 {
        let (cos, _) = self.nco.step();
        self.level * cos
    }
}

/// The digital modem's ear for TONEq, and the analogue answerer's for the
/// TONEq of a second analogue modem.
///
/// Two detectors rather than one, because the question is not "is there 980 Hz
/// on the line" -- CM, QC1a and the modem's own V.21(L) preamble all put 980 Hz
/// on the line whenever they send a ONE -- but "is 980 Hz *all* there is".
/// V.21(L) answers no within a bit or two, since its other frequency is never
/// far behind.
#[derive(Debug, Clone)]
pub struct ToneqDetector {
    mark: ToneDetector,
    space: ToneDetector,
    /// Everything on the line, to weigh the tone against.
    power: OnePole,
    /// Samples the line has been holding 980 Hz and nothing else for.
    run: u64,
    need: u64,
    heard: bool,
    fs: f64,
}

impl ToneqDetector {
    pub fn new(fs: f64) -> Self {
        Self {
            mark: ToneDetector::new(TONEQ_HZ, TONEQ_BANDWIDTH, fs),
            // V.21(L)'s space, which is what TONEq never has and V.21(L) data
            // always does: `crate::v8::LOW` is (FA, Fz), space then mark.
            space: ToneDetector::new(crate::v8::LOW.0, TONEQ_BANDWIDTH, fs),
            power: OnePole::new(0.050, fs),
            run: 0,
            need: (TONEQ_STEADY * fs) as u64,
            heard: false,
            fs,
        }
    }

    /// One line sample.
    pub fn feed(&mut self, x: f64) {
        self.mark.feed(x);
        self.space.feed(x);
        self.power.process(x.abs());
        let mark = self.mark.amplitude();
        let steady = mark > TONEQ_AUDIBLE
            && mark > TONEQ_STANDING * self.power.value()
            && self.space.amplitude() < TONEQ_SPACE_SHARE * mark;
        self.run = if steady { self.run + 1 } else { 0 };
        if self.run >= self.need {
            self.heard = true;
        }
    }

    /// Whether TONEq has been heard. It latches: 9.2.2.3 and 9.2.4.3 both act
    /// on the first detection and never ask again.
    pub fn heard(&self) -> bool {
        self.heard
    }

    /// How long the line has been holding 980 Hz and nothing else, in seconds.
    pub fn steady(&self) -> f64 {
        self.run as f64 / self.fs
    }

    /// Whether 980 Hz is on the line at this instant, whatever the run has
    /// reached. What the analogue modem watches to know its own TONEq has been
    /// answered, and what the digital modem's silence detector is not.
    pub fn present(&self) -> bool {
        self.run > 0
    }

    /// Forget everything, for a modem going round the loop again.
    pub fn reset(&mut self) {
        *self = Self::new(self.fs);
    }
}

// ---------------------------------------------------------------------------
// Hearing QTS, and telling ANSpcm from the other answering tones
// ---------------------------------------------------------------------------

/// QTS's fundamental, in hertz: 8000/6 = 1333.3.
///
/// The six-symbol pattern satisfies x(i + 3) = -x(i), so it has only odd
/// harmonics of 8000/6 -- 1333.3 Hz and 4000 Hz, nothing at DC and nothing at
/// 2666.7 Hz. The 4000 Hz part sits on the codec's own Nyquist frequency and
/// mostly does not survive the reconstruction filter, so what arrives is very
/// nearly a 1333.3 Hz tone at about two thirds of V.
pub const QTS_HZ: f64 = 8000.0 / QTS_PATTERN as f64;

/// The quietest QTS worth tracking, in the units the boxcar reports: half the
/// amplitude of the 1333.3 Hz tone, as a fraction of full scale.
///
/// The faintest QTS there is -- U_QTS `0000`, Ucode 61, the bottom of Table 2
/// -- leaves the codec at about -21 dBm0 and reaches this measurement at about
/// 0.018, so the floor is some 33 dB below the quietest thing that could be
/// sent and 43 dB below the loudest. What it must stay above is the boxcar's
/// own answer to a quiet line, which for the 0.002 of noise the impairment
/// tests use is about 0.0006.
///
/// It is only half the guard in any case. What really separates QTS from the
/// ANSpcm that follows it is [`QTS_STEADY`], because a 2100 Hz tone can be as
/// loud as it likes and still not hold still at 1333.3 Hz.
const QTS_AUDIBLE: f64 = 0.0004;

/// How many periods of a steady phasor make a lock.
///
/// Four is 3 ms, which is nothing against QTS's own 96 ms and short enough
/// that a lock is always established long before the reversal at 768.
const QTS_LOCK_PERIODS: usize = 4;

/// How long the phasor may collapse without the lock being given up, in
/// periods. A reversal takes the phasor through zero on its way round, so a
/// watch that unlocked on a quiet moment would unlock on the very thing it is
/// waiting for.
const QTS_QUIET_PERIODS: usize = 2;

/// How nearly two phasors a period apart have to agree for the tone to count
/// as steady, as a fraction of the product of their lengths.
///
/// 0.8 is about 37 degrees. ANSpcm, the other thing on the line in short
/// Phase 1, turns 207 degrees in one QTS period at this detector's frequency,
/// so it can never look steady however loud it is.
const QTS_STEADY: f64 = 0.8;

/// Finds the QTS-to-QTS-bar reversal, which is where the digital modem's data
/// frame grid is.
///
/// The measurement has to be good to a symbol and it has 6 ms to make it in,
/// which rules out the usual approach of comparing a phasor with itself a
/// settling-time ago ([`dsp::ReversalDetector`] needs tens of milliseconds).
/// So the correlation is a boxcar exactly one QTS period long instead: it has
/// no settling time at all, it puts an exact null on QTS's own 4000 Hz
/// component, and across the reversal its output walks in a straight line from
/// one end of the phasor to the other. Where that line crosses zero is half a
/// window after the reversal, and interpolating between the two samples either
/// side of the crossing puts the answer inside a tenth of a symbol on a quiet
/// line and inside a symbol on a noisy one.
#[derive(Debug, Clone)]
pub struct QtsWatch {
    fs: f64,
    /// Samples in one period of [`QTS_HZ`].
    period: usize,
    /// The reference oscillator the samples are mixed against.
    nco: Nco,
    /// The last `period` products, and their running sum: the boxcar.
    ring: Vec<(f64, f64)>,
    sum: (f64, f64),
    /// The boxcar's output a period ago, for judging steadiness.
    history: Vec<(f64, f64)>,
    at: u64,
    /// The direction the phasor has settled in, once it has.
    reference: Option<(f64, f64)>,
    steady: usize,
    quiet: usize,
    previous: Option<f64>,
    reversal: Option<f64>,
}

impl QtsWatch {
    pub fn new(fs: f64) -> Self {
        let period = (fs / QTS_HZ).round().max(2.0) as usize;
        Self {
            fs,
            period,
            nco: Nco::new(QTS_HZ, fs),
            ring: vec![(0.0, 0.0); period],
            sum: (0.0, 0.0),
            history: vec![(0.0, 0.0); period],
            at: 0,
            reference: None,
            steady: 0,
            quiet: 0,
            previous: None,
            reversal: None,
        }
    }

    /// One line sample.
    pub fn feed(&mut self, x: f64) {
        let n = self.at;
        self.at += 1;
        let (cos, sin) = self.nco.step();
        let here = (x * cos, -x * sin);
        let slot = (n as usize) % self.period;
        let gone = self.ring[slot];
        self.ring[slot] = here;
        self.sum = (self.sum.0 + here.0 - gone.0, self.sum.1 + here.1 - gone.1);
        let before = self.history[slot];
        self.history[slot] = self.sum;
        if n < 2 * self.period as u64 || self.reversal.is_some() {
            return;
        }
        let length = self.sum.0.hypot(self.sum.1);
        let amplitude = length / self.period as f64;
        let Some(reference) = self.reference else {
            let older = before.0.hypot(before.1);
            let same = self.sum.0 * before.0 + self.sum.1 * before.1;
            if amplitude > QTS_AUDIBLE && same > QTS_STEADY * length * older {
                self.steady += 1;
            } else {
                self.steady = 0;
            }
            if self.steady >= QTS_LOCK_PERIODS * self.period {
                self.reference = Some((self.sum.0 / length, self.sum.1 / length));
            }
            return;
        };
        // How far the boxcar still points the way it did before the reversal.
        let along = (self.sum.0 * reference.0 + self.sum.1 * reference.1) / self.period as f64;
        if amplitude < QTS_AUDIBLE {
            self.quiet += 1;
            if self.quiet > QTS_QUIET_PERIODS * self.period {
                self.give_up();
                return;
            }
        } else {
            self.quiet = 0;
        }
        if let Some(previous) = self.previous
            && previous > 0.0
            && along <= 0.0
        {
            // Linear between the last sample that still pointed forwards and
            // the first that did not, then back half a window to where the
            // reversal itself was.
            let fraction = previous / (previous - along);
            let crossing = (n - 1) as f64 + fraction;
            self.reversal = Some(crossing - self.period as f64 / 2.0 + 1.0);
            self.previous = None;
            return;
        }
        // While the tone is plainly still itself, follow any slow turn the
        // line puts on it. Slowly enough that the reversal, which takes one
        // window, is not followed at all.
        if along > 0.5 * amplitude {
            let tracked = (
                reference.0 * 0.999 + 0.001 * self.sum.0 / length,
                reference.1 * 0.999 + 0.001 * self.sum.1 / length,
            );
            let size = tracked.0.hypot(tracked.1);
            self.reference = Some((tracked.0 / size, tracked.1 / size));
        }
        self.previous = Some(along);
    }

    fn give_up(&mut self) {
        self.reference = None;
        self.steady = 0;
        self.quiet = 0;
        self.previous = None;
    }

    /// Whether the watch has a steady 1333.3 Hz tone to measure.
    pub fn locked(&self) -> bool {
        self.reference.is_some()
    }

    /// Where the QTS-to-QTS-bar reversal fell, in line samples since the first
    /// sample fed -- fractional, because the estimate is better than a sample.
    ///
    /// The digital modem's first QTS symbol is 768 symbols before this, and its
    /// data frame interval 0 falls there and every six symbols afterwards.
    pub fn reversal(&self) -> Option<f64> {
        self.reversal
    }

    /// The same instant in seconds since the first sample fed.
    pub fn reversal_seconds(&self) -> Option<f64> {
        self.reversal.map(|at| at / self.fs)
    }

    /// How many line samples have been fed.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// Forget everything, including the sample count, for a modem that has
    /// gone back to V.8 and is starting short Phase 1 again.
    pub fn reset(&mut self) {
        *self = Self::new(self.fs);
    }
}

/// Which answering tone is on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerSignal {
    /// V.8's ANSam: 2100 Hz with the 15 Hz amplitude modulation.
    Ansam,
    /// V.25's plain answer tone, which says the far end does not do V.8.
    Ans,
    /// V.92's ANSpcm: an unmodulated 2100 Hz behind a QTS burst.
    Anspcm,
}

/// How long after the QTS reversal an unmodulated 2100 Hz tone is still
/// ANSpcm, in seconds.
///
/// The signals themselves leave no room at all -- ANSpcm begins 48 symbols, 6
/// ms, after the reversal -- but the ear does. The answering-tone detector
/// averages its envelope over 400 ms, so its verdict on a tone that has just
/// started arrives a few hundred milliseconds after the tone does. A second is
/// room for that and for a slow line, and there is nothing else in short
/// Phase 1 that a QTS burst could be followed by.
pub const QTS_TO_ANSPCM: f64 = 1.0;

/// The analogue modem's ear for the digital modem's answer.
///
/// Three tones can arrive at this point in a call and the modem does something
/// different for each. ANSam means the far end fell back to ordinary V.8, so
/// the short start-up is off and CM has to go out. ANS means it is not a V.8
/// modem at all. ANSpcm means the quick connect was accepted and TONEq is the
/// reply.
///
/// The first two are told apart by the 15 Hz modulation, which is what
/// `v8::ansam` exists for. ANSpcm and ANS cannot be told apart that way,
/// because ANSpcm has no modulation either: 8.3.1 builds it out of codewords
/// at a constant envelope, so spectrally it is V.25's tone with phase
/// reversals. What separates them is that ANSpcm never arrives alone -- QTS
/// and QTS-bar run into it with no gap (Figures 3 to 6) -- so this watches for
/// the QTS reversal and remembers it.
///
/// It is `Debug` but not `Clone`, because `v8::ansam::AnswerTone` is not. A
/// modem that has to be `Clone` and wants one of these needs that derive
/// added, which is a one-line change in a file no work package of this plan
/// owns.
#[derive(Debug)]
pub struct AnspcmWatch {
    tone: AnswerTone,
    qts: QtsWatch,
    fs: f64,
    at: u64,
    window: u64,
    /// Latched once an unmodulated tone has been heard behind a QTS burst.
    anspcm: bool,
}

impl AnspcmWatch {
    pub fn new(fs: f64) -> Self {
        Self {
            tone: AnswerTone::new(fs),
            qts: QtsWatch::new(fs),
            fs,
            at: 0,
            window: (QTS_TO_ANSPCM * fs) as u64,
            anspcm: false,
        }
    }

    /// One line sample.
    pub fn feed(&mut self, x: f64) {
        self.tone.feed(x);
        self.qts.feed(x);
        self.at += 1;
        if !self.anspcm
            && self.tone.is_plain()
            && let Some(reversal) = self.qts.reversal()
            && self.at.saturating_sub(reversal as u64) <= self.window
        {
            self.anspcm = true;
        }
    }

    /// What is on the line, while anything is.
    pub fn signal(&self) -> Option<AnswerSignal> {
        if self.tone.is_ansam() {
            return Some(AnswerSignal::Ansam);
        }
        if !self.tone.present() {
            return None;
        }
        Some(if self.anspcm { AnswerSignal::Anspcm } else { AnswerSignal::Ans })
    }

    /// Whether what is on the line is the digital modem's ANSpcm.
    pub fn is_anspcm(&self) -> bool {
        self.signal() == Some(AnswerSignal::Anspcm)
    }

    /// Where the QTS reversal fell, in line samples since the first sample
    /// fed. See [`QtsWatch::reversal`].
    pub fn qts_reversal(&self) -> Option<f64> {
        self.qts.reversal()
    }

    /// The answering-tone detector, for a caller that wants the amplitude or
    /// the modulation depth it measured.
    pub fn tone(&self) -> &AnswerTone {
        &self.tone
    }

    /// The QTS watch, for a caller that wants the frame grid rather than the
    /// verdict.
    pub fn qts(&self) -> &QtsWatch {
        &self.qts
    }

    /// Forget everything, for a modem that has fallen back to V.8 and may come
    /// round to short Phase 1 again.
    pub fn reset(&mut self) {
        *self = Self::new(self.fs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::network::Network;

    const FS: f64 = 16_000.0;

    /// Every level Table 6 defines, with the table it names.
    const LEVELS: [AnspcmLevel; 4] = [
        AnspcmLevel::Minus9_5,
        AnspcmLevel::Minus12,
        AnspcmLevel::Minus15,
        AnspcmLevel::Minus18,
    ];

    /// The G.711 quantiser of 8.3.1, as P1D 3.4 pinned it down against the
    /// printed tables: decision intervals with the lower bound inclusive, so a
    /// value sitting exactly on one goes to the louder interval.
    ///
    /// It is a test and not the module's own code on purpose. 8.3.1 says the
    /// generated output "shall equal the output defined in Tables 7 to
    /// Table 10", so the tables are the requirement; and G.711 is not in
    /// `docs/specs`, so this rule is itself a reading, proved here by the
    /// 2408 octets it reproduces rather than quoted from anywhere.
    fn quantise(law: Law, x: i64) -> u8 {
        let negative = x < 0;
        let magnitude = x.unsigned_abs();
        let ucode = match law {
            Law::Mu => {
                let biased = magnitude.min(8159) + 33;
                let chord = biased.ilog2() - 5;
                let step = (biased >> (chord + 1)) & 15;
                16 * chord as u8 + step as u8
            }
            Law::A if magnitude < 32 => (magnitude >> 1) as u8,
            Law::A => {
                let magnitude = magnitude.min(4095);
                let chord = magnitude.ilog2() - 4;
                let step = (magnitude >> chord) & 15;
                16 * chord as u8 + step as u8
            }
        };
        ucode::octet(law, ucode, negative)
    }

    /// x for symbol k, from the equation of 8.3.1 with Table 6's scl and
    /// theta: `floor(scl x sqrt(2) x cos(2 pi k x 79/301 + theta) + 0.5)`.
    ///
    /// In f64, because the closest any value inside the floor comes to a
    /// rounding boundary is about a ten-thousandth -- comfortable for f64 and
    /// marginal for f32 at the magnitudes near 1900.
    fn sample(level: AnspcmLevel, law: Law, k: usize) -> i64 {
        let scl = f64::from(match law {
            Law::Mu => level.scl_mu_law(),
            Law::A => level.scl_a_law(),
        });
        let angle = std::f64::consts::TAU * k as f64 * ANSPCM_CYCLES as f64 / ANSPCM_PERIOD as f64;
        (scl * std::f64::consts::SQRT_2 * (angle + ANSPCM_THETA).cos() + 0.5).floor() as i64
    }

    /// The whole of 8.3.1 for symbol k: the equation, then G.711.
    fn generate(level: AnspcmLevel, law: Law, k: usize) -> u8 {
        quantise(law, sample(level, law, k))
    }

    /// 8.3.1: "The resulting output shall equal the output defined in Tables 7
    /// to Table 10 depending on the value of scl."
    #[test]
    fn the_generator_reproduces_all_2408_tabled_octets() {
        let mut checked = 0;
        for level in LEVELS {
            for law in [Law::Mu, Law::A] {
                let table = anspcm_table(level, law);
                for (k, &tabled) in table.iter().enumerate() {
                    assert_eq!(generate(level, law, k), tabled, "{level:?} {law:?} k = {k}");
                    checked += 1;
                }
            }
        }
        // 301 octets, two laws, four levels.
        assert_eq!(checked, 2408);

        // x is never zero, so every octet in every table carries a polarity
        // that means something -- which is what makes a phase reversal the
        // plain `^ 0x80` of `anspcm_octet`. It does not follow that the Ucode
        // is never zero: A-law's first chord puts magnitudes 0 and 1 in the
        // same code, and the quietest samples of Tables 9 and 10 are 1, so
        // "+0" and "-0" appear in ANSpcm as well as in QTS and are as
        // different there as they are here.
        let mut zero_codes = 0;
        for level in LEVELS {
            for law in [Law::Mu, Law::A] {
                for k in 0..ANSPCM_PERIOD {
                    let x = sample(level, law, k);
                    assert_ne!(x, 0, "{level:?} {law:?} k = {k}");
                    let (ucode, negative) = ucode::from_octet(law, anspcm_table(level, law)[k]);
                    assert_eq!(negative, x < 0, "{level:?} {law:?} k = {k} lost its sign");
                    zero_codes += usize::from(ucode == 0);
                }
            }
        }
        assert!(zero_codes > 0, "A-law's quietest samples should land in Ucode 0");
    }

    /// The cell of Table 7 the rendered page prints as a bare "8" where every
    /// other cell carries two hex digits (P1D, sources note 2).
    #[test]
    fn table_7_k82_a_law_is_08() {
        assert_eq!(anspcm_table(AnspcmLevel::Minus9_5, Law::A)[82], 0x08);
        assert_eq!(generate(AnspcmLevel::Minus9_5, Law::A, 82), 0x08);
        // And its mu-law neighbour on the same row, so a transcription that
        // had slipped a column would show.
        assert_eq!(anspcm_table(AnspcmLevel::Minus9_5, Law::Mu)[82], 0x22);
        // The first and last octet of every table, which is where a truncated
        // copy would give itself away.
        for (level, law, first, last) in [
            (AnspcmLevel::Minus9_5, Law::Mu, 0xA1, 0x59),
            (AnspcmLevel::Minus9_5, Law::A, 0x88, 0x74),
            (AnspcmLevel::Minus12, Law::Mu, 0xA9, 0x5E),
            (AnspcmLevel::Minus12, Law::A, 0x83, 0x4F),
            (AnspcmLevel::Minus15, Law::Mu, 0xAF, 0x64),
            (AnspcmLevel::Minus15, Law::A, 0x9A, 0x46),
            (AnspcmLevel::Minus18, Law::Mu, 0xB8, 0x6A),
            (AnspcmLevel::Minus18, Law::A, 0x93, 0x58),
        ] {
            let table = anspcm_table(level, law);
            assert_eq!((table[0], table[ANSPCM_PERIOD - 1]), (first, last), "{level:?} {law:?}");
        }
    }

    /// 8.3.1: "The sequence repeats every 301 symbols and has a phase reversal
    /// added to it every 3612 symbols."
    #[test]
    fn a_reversal_flips_only_the_polarity_bit_every_3612_symbols_and_lands_on_a_frame_boundary() {
        let (level, law) = (AnspcmLevel::Minus12, Law::Mu);
        let table = anspcm_table(level, law);

        // The blocks alternate: one carries the table as printed and the next
        // carries every octet with its polarity bit turned over, nothing else
        // changed. Which of the two comes first is section 4's reading, and
        // the expectation is written in terms of it, so flipping the constant
        // moves the test with it instead of breaking it.
        assert_eq!(
            ANSPCM_TABLE_POLARITY_FIRST,
            anspcm_octet(level, law, 0) == table[0],
            "the first block should be the printed polarity"
        );
        for n in 0..3u64 * ANSPCM_REVERSAL as u64 {
            let octet = anspcm_octet(level, law, n);
            let plain = table[(n % ANSPCM_PERIOD as u64) as usize];
            let block = n / ANSPCM_REVERSAL as u64;
            let printed = block.is_multiple_of(2) == ANSPCM_TABLE_POLARITY_FIRST;
            let expected = if printed { plain } else { plain ^ 0x80 };
            assert_eq!(octet, expected, "symbol {n}");
            // A reversal is a negation and nothing else: the same Ucode comes
            // back with the other sign.
            let (u, sign) = ucode::from_octet(law, octet);
            let (v, other) = ucode::from_octet(law, anspcm_octet(level, law, n + ANSPCM_REVERSAL as u64));
            assert_eq!(u, v, "symbol {n} changed codeword, not polarity");
            assert_ne!(sign, other, "symbol {n} did not change polarity");
        }

        // 3612 is twelve whole periods, so a reversal never cuts one short.
        assert_eq!(ANSPCM_REVERSAL, 12 * ANSPCM_PERIOD);
        assert!(ANSPCM_REVERSAL.is_multiple_of(ANSPCM_PERIOD));
        // And it lands on a downstream data frame boundary -- 602 frames of
        // six -- which is how the alignment QTS set up survives the answering
        // tone. It is a whole upstream frame too.
        assert_eq!(ANSPCM_REVERSAL / crate::v90::INTERVALS, 602);
        assert!(ANSPCM_REVERSAL.is_multiple_of(crate::v90::INTERVALS));
        assert!(ANSPCM_REVERSAL.is_multiple_of(super::super::UP_INTERVALS));
        // 451.5 ms, inside V.8 7.2's 450 +/- 25 ms for ANSam.
        let interval = ANSPCM_REVERSAL as f64 / 8000.0;
        assert!((interval - 0.4515).abs() < 1e-9);
        assert!((interval - 0.450).abs() <= 0.025);
        // 2099.668 Hz, inside V.8 7.2's 2100 +/- 1 Hz.
        assert!((ANSPCM_HZ - 2099.668).abs() < 0.001, "{ANSPCM_HZ}");
        assert!((ANSPCM_HZ - 2100.0).abs() <= 1.0);
        // 301 = 7 x 43 shares nothing with 79, so 301 symbols is the period.
        assert_eq!(ANSPCM_PERIOD, 7 * 43);
        for divisor in 2..ANSPCM_PERIOD {
            assert!(
                !(ANSPCM_PERIOD.is_multiple_of(divisor) && ANSPCM_CYCLES.is_multiple_of(divisor)),
                "{divisor} divides both"
            );
        }
    }

    /// P1D ANS-17: the measured power of each tabled sequence, against the
    /// four levels Table 6 names.
    #[test]
    fn anspcm_power_is_the_level_lm_names() {
        // The figures ANS-17 lists, which were computed from the G.711
        // reconstruction values of Table 1/V.90.
        for (level, mu, a) in [
            (AnspcmLevel::Minus9_5, -9.56, -9.60),
            (AnspcmLevel::Minus12, -12.04, -12.10),
            (AnspcmLevel::Minus15, -15.01, -15.10),
            (AnspcmLevel::Minus18, -18.06, -18.11),
        ] {
            for (law, want) in [(Law::Mu, mu), (Law::A, a)] {
                let got = anspcm_power_dbm0(level, law);
                // The plan asks for a fifth of a decibel; the agreement is a
                // hundredth, because ANS-17 was computed the same way from the
                // same reconstruction values. Anything looser would not notice
                // a table copied out one level along.
                assert!((got - want).abs() < 0.01, "{level:?} {law:?} measured {got:.4}, ANS-17 has {want}");
                // And the level the modem announced in LM, which is what the
                // far end will scale its receiver by.
                assert!(
                    (got - level.dbm0()).abs() < 0.2,
                    "{level:?} {law:?} measured {got:.2} against an announced {}",
                    level.dbm0()
                );
            }
        }
        // The four levels are three decibels apart at the bottom and two and a
        // half at the top, in the order Table 6 prints them.
        let ladder: Vec<f64> = LEVELS.iter().map(|&l| anspcm_power_dbm0(l, Law::Mu)).collect();
        for pair in ladder.windows(2) {
            assert!(pair[1] < pair[0], "the ladder does not descend: {ladder:?}");
        }
        // scl is an RMS on G.711's own scale, four times smaller than Table
        // 1/V.90's on mu-law and eight times on A-law. Generating against the
        // wrong scale is the mistake that sends ANSpcm 12 dB too quiet, and
        // this is what would catch it.
        let rms = zero_dbm0(Law::Mu) * 10.0f64.powf(anspcm_power_dbm0(AnspcmLevel::Minus12, Law::Mu) / 20.0);
        assert!((rms * 32768.0 / 4.0 - 1000.0).abs() < 5.0, "scl came out {}", rms * 32768.0 / 4.0);

        // The overload point is the top of the loudest interval, not the value
        // the decoder reconstructs for it. They are 0.14 dB apart on mu-law,
        // which is more than the whole test's tolerance, so taking the wrong
        // one would show up in every figure above.
        assert_eq!(ucode::linear(Law::Mu, 127), 32_124);
        assert!(OVERLOAD_MU > f64::from(ucode::linear(Law::Mu, 127)));
        let mistake = 20.0 * (OVERLOAD_MU / f64::from(ucode::linear(Law::Mu, 127))).log10();
        assert!((mistake - 0.137).abs() < 0.002, "the two differ by {mistake:.3} dB");
        // A-law's overload is exactly full scale on Table 1/V.90's sixteen-bit
        // column, which is why that column's two laws agree to half a percent
        // while Table 6's scl values differ by exactly two.
        assert_eq!(OVERLOAD_A, 32_768.0);
        assert_eq!(OVERLOAD_MU / 4.0, 8159.0);
        for level in LEVELS {
            assert_eq!(level.scl_mu_law(), 2 * level.scl_a_law());
        }
    }

    /// 8.3.6: "The first symbol of QTS is defined to be transmitted in data
    /// frame interval 0", 128 repetitions of `{+V, +0, +V, -V, -0, -V}`, then
    /// 8 of the inverse.
    #[test]
    fn qts_puts_its_first_symbol_in_interval_zero_and_reverses_at_symbol_768() {
        let (law, uqts) = (Law::Mu, 70u8);
        let v_plus = ucode::octet(law, uqts, false);
        let v_minus = ucode::octet(law, uqts, true);
        let zero_plus = ucode::octet(law, 0, false);
        let zero_minus = ucode::octet(law, 0, true);
        // "+0" and "-0" are different octets and must not be merged: mu-law
        // FF and 7F, A-law D5 and 55.
        assert_eq!((zero_plus, zero_minus), (0xFF, 0x7F));
        assert_eq!(
            (ucode::octet(Law::A, 0, false), ucode::octet(Law::A, 0, true)),
            (0xD5, 0x55)
        );

        let frame = [v_plus, zero_plus, v_plus, v_minus, zero_minus, v_minus];
        for n in 0..QTS_SYMBOLS as u64 {
            let want = frame[(n % QTS_PATTERN as u64) as usize];
            assert_eq!(qts_octet(law, uqts, n), Some(want), "QTS symbol {n}");
        }
        for n in QTS_SYMBOLS as u64..QTS_TOTAL as u64 {
            let want = frame[(n % QTS_PATTERN as u64) as usize] ^ 0x80;
            assert_eq!(qts_octet(law, uqts, n), Some(want), "QTS-bar symbol {n}");
        }
        assert_eq!(qts_octet(law, uqts, QTS_TOTAL as u64), None, "ANSpcm follows");

        // The first symbol is +V and it is interval 0; the reversal is at 768,
        // which is a frame boundary, and ANSpcm starts at 816, which is
        // another. 768T and 48T are what Figures 3 to 6 label them.
        assert_eq!(qts_octet(law, uqts, 0), Some(v_plus));
        assert_eq!((QTS_SYMBOLS, QTS_BAR_SYMBOLS, QTS_TOTAL), (768, 48, 816));
        assert!(QTS_SYMBOLS.is_multiple_of(QTS_PATTERN));
        assert!(QTS_TOTAL.is_multiple_of(QTS_PATTERN));
        assert_eq!(QTS_PATTERN, crate::v90::INTERVALS);
        // 96 ms and 6 ms at 8000 symbol/s.
        assert!((QTS_SYMBOLS as f64 / 8000.0 - 0.096).abs() < 1e-9);
        assert!((QTS_BAR_SYMBOLS as f64 / 8000.0 - 0.006).abs() < 1e-9);
        // x(i + 3) = -x(i), which is why the pattern has nothing at DC and
        // nothing at 2666.7 Hz.
        for i in 0..3 {
            let (a, _) = ucode::from_octet(law, frame[i]);
            let (b, _) = ucode::from_octet(law, frame[i + 3]);
            assert_eq!(a, b);
            assert_eq!(frame[i] ^ 0x80, frame[i + 3], "interval {i}");
        }
        // The same construction on A-law, where "0" is not silence: Ucode 0 is
        // linear +8 and -8.
        assert_eq!(qts_octet(Law::A, uqts, 1), Some(0xD5));
        assert_eq!(ucode::linear(Law::A, 0), 8);
        assert_eq!(ucode::linear(Law::Mu, 0), 0);
        // Every U_QTS Table 2 offers works, at both laws.
        for uqts in [61u8, 62, 63, 66, 67, 70, 71, 74, 75, 78, 79, 82, 83, 86, 87] {
            for law in [Law::Mu, Law::A] {
                assert_eq!(qts_octet(law, uqts, 0), Some(ucode::octet(law, uqts, false)));
                assert_eq!(qts_octet(law, uqts, 3), Some(ucode::octet(law, uqts, true)));
            }
        }
    }

    /// 9.8.1.1.3, read across to short Phase 1 (section 4): silence from the
    /// digital modem is a constant positive Ucode 0, with frame alignment kept.
    #[test]
    fn silence_is_the_positive_ucode_zero_codeword() {
        assert_eq!(silence_octet(Law::Mu), 0xFF);
        assert_eq!(silence_octet(Law::A), 0xD5);
        for law in [Law::Mu, Law::A] {
            let (ucode, negative) = ucode::from_octet(law, silence_octet(law));
            assert_eq!(ucode, 0);
            assert!(!negative, "silence is the positive codeword, not the negative one");
        }
    }

    /// Play a run of octets down a network and hand back the line samples.
    fn play(net: &mut Network, law: Law, octets: impl Iterator<Item = u8>) -> Vec<f64> {
        let mut line = Vec::new();
        for octet in octets {
            let (ucode, negative) = ucode::from_octet(law, octet);
            let level = ucode::level(law, ucode) * if negative { -1.0 } else { 1.0 };
            line.extend(net.down(level));
        }
        line
    }

    /// 8.3.6 with Figures 3 to 6: the reversal at symbol 768 is the mark that
    /// tells the analogue modem where the digital modem's data frames are, so
    /// finding it to worse than a symbol would put the grid in the wrong
    /// place.
    #[test]
    fn the_qts_reversal_is_found_to_within_a_symbol() {
        // Every law, a loud U_QTS and the quietest Table 2 offers, and a
        // silence in front of the burst so that the answer is not simply the
        // watch's own start-up.
        for law in [Law::Mu, Law::A] {
            for uqts in [61u8, 70, 87] {
                for lead in [0u64, 137, 600] {
                    let mut net = Network::new(law, FS);
                    let mut watch = QtsWatch::new(FS);
                    let octets = (0..lead)
                        .map(|_| silence_octet(law))
                        .chain((0..QTS_TOTAL as u64 + 400).map(|n| {
                            qts_octet(law, uqts, n).unwrap_or_else(|| silence_octet(law))
                        }));
                    for x in play(&mut net, law, octets) {
                        watch.feed(x);
                    }
                    let found = watch.reversal().expect("the reversal was not found");
                    // A line sample sits at network time i x 8000/fs, and the
                    // network's reconstruction is symmetric, so codeword
                    // `lead + 768` is line sample (lead + 768) x fs/8000.
                    let want = (lead + QTS_SYMBOLS as u64) as f64 * FS / 8000.0;
                    let error = (found - want) * 8000.0 / FS;
                    // A tenth of a symbol on a quiet line: the plan asks for a
                    // symbol, and the margin is worth having, because the
                    // grid this fixes has to still be right after Phase 2.
                    assert!(
                        error.abs() <= 0.1,
                        "{law:?} U{uqts} lead {lead}: {error:.3} symbols out"
                    );
                    assert!(watch.locked());
                }
            }
        }

        // And on lines a real call gives it: noise, where the interpolation
        // has to stay honest rather than merely precise, and 20 dB of loss on
        // the quietest U_QTS Table 2 offers, which is the faintest QTS that
        // can arrive at all.
        for (uqts, noise, pad) in [(70u8, 0.002, 0.0), (61, 0.0, -20.0), (61, 0.0005, -20.0)] {
            let law = Law::Mu;
            let mut net = Network::new(law, FS).with_noise(noise).with_pads(pad, 0.0);
            let mut watch = QtsWatch::new(FS);
            let octets = (0..200u64).map(|_| silence_octet(law)).chain(
                (0..QTS_TOTAL as u64 + 400)
                    .map(|n| qts_octet(law, uqts, n).unwrap_or_else(|| silence_octet(law))),
            );
            for x in play(&mut net, law, octets) {
                watch.feed(x);
            }
            let found = watch
                .reversal()
                .unwrap_or_else(|| panic!("U{uqts} at {pad} dB with {noise} of noise was not found"));
            let want = (200 + QTS_SYMBOLS) as f64 * FS / 8000.0;
            let error = (found - want) * 8000.0 / FS;
            assert!(error.abs() <= 1.0, "U{uqts} at {pad} dB: {error:.3} symbols out");
        }
    }

    /// One bit of V.21(L), as the QC and CM sequences of short Phase 1 put it
    /// on the line: 980 Hz for a ONE, 1180 Hz for a ZERO, at 300 bit/s.
    fn v21_low(bits: &[bool], fs: f64) -> Vec<f64> {
        let mut out = Vec::new();
        let mut phase = 0.0f64;
        let per_bit = fs / crate::v8::BAUD;
        let (space, mark) = crate::v8::LOW;
        assert_eq!(mark, TONEQ_HZ, "a V.21(L) mark is TONEq's own frequency");
        let mut at = 0.0;
        for &bit in bits {
            let hz = if bit { mark } else { space };
            let until = at + per_bit;
            while at < until {
                phase += std::f64::consts::TAU * hz / fs;
                // The same nominal transmit power TONEq itself goes out at.
                out.push(TONEQ_LEVEL * phase.sin());
                at += 1.0;
            }
        }
        out
    }

    /// 8.2.5 with P1A 11.6: TONEq is 980 Hz, which is also V.21(L)'s mark, so
    /// the detector has to want it steady for longer than the longest run of
    /// marks short Phase 1 can produce.
    #[test]
    fn toneq_is_heard_after_60_ms_and_v21_marking_is_not_taken_for_it() {
        let mut generator = ToneqGenerator::new(FS);
        let mut detector = ToneqDetector::new(FS);
        let mut heard_at = None;
        for n in 0..(0.5 * FS) as usize {
            detector.feed(generator.step());
            if detector.heard() && heard_at.is_none() {
                heard_at = Some(n as f64 / FS);
            }
        }
        let heard_at = heard_at.expect("TONEq was never heard");
        assert!(heard_at >= TONEQ_STEADY, "heard after only {heard_at:.3} s");
        assert!(heard_at < 0.120, "took {heard_at:.3} s to hear a clean tone");

        // The trap: a QC1a whose U_QTS is the cleardown code 1111 carries a
        // run of fourteen ONEs, bits 26 to 39, and a V.21(L) transmitter holds
        // 980 Hz for every one of them.
        let qc = v8::quick::Qc::qc1a(v8::quick::Uqts::Cleardown, true);
        let bits = qc.bits();
        let longest = bits
            .iter()
            .fold((0usize, 0usize), |(best, run), &bit| {
                let run = if bit { run + 1 } else { 0 };
                (best.max(run), run)
            })
            .0;
        assert_eq!(longest, 14, "the cleardown QC1a should hold fourteen marks");
        assert!(
            (longest as f64 / crate::v8::BAUD) < TONEQ_STEADY,
            "fourteen bits is {:.1} ms",
            longest as f64 / crate::v8::BAUD * 1000.0
        );
        let mut detector = ToneqDetector::new(FS);
        for x in v21_low(&bits, FS) {
            detector.feed(x);
        }
        assert!(
            !detector.heard(),
            "a cleardown QC1a read as TONEq after {:.3} s of mark",
            detector.steady()
        );

        // Nor does an ordinary QC1a, nor seventeen marks -- three bits more
        // than short Phase 1 can produce, 56.7 ms, and exactly what a detector
        // set at the 50 ms of `TONEQ_MINIMUM` would have fallen for.
        let ordinary = v8::quick::Qc::qc1a(v8::quick::Uqts::from_ucode(70).expect("Table 2 lists 70"), true);
        let mut detector = ToneqDetector::new(FS);
        for x in v21_low(&ordinary.bits(), FS) {
            detector.feed(x);
        }
        assert!(!detector.heard());
        let mut detector = ToneqDetector::new(FS);
        let marks: Vec<bool> = std::iter::repeat_n(true, 17).collect();
        for x in v21_low(&marks, FS) {
            detector.feed(x);
        }
        assert!(!detector.heard(), "17 bits of mark is {:.1} ms", 17.0 / crate::v8::BAUD * 1000.0);

        // Nothing at all, and the far end's own ANSpcm, are not TONEq either:
        // a digital modem arms this detector while it is sending 2100 Hz.
        let mut detector = ToneqDetector::new(FS);
        for _ in 0..(0.3 * FS) as usize {
            detector.feed(0.0);
        }
        assert!(!detector.heard() && !detector.present());
        let mut net = Network::new(Law::Mu, FS);
        let mut detector = ToneqDetector::new(FS);
        let level = AnspcmLevel::Minus12;
        let octets = (0..4000u64).map(|n| anspcm_octet(level, Law::Mu, n));
        for x in play(&mut net, Law::Mu, octets) {
            detector.feed(x);
        }
        assert!(!detector.heard(), "ANSpcm read as TONEq");
    }

    /// 8.3.1 and Figures 3 to 6: ANSpcm has no 15 Hz modulation, so an ANSam
    /// detector calls it ANS. What makes it ANSpcm is the QTS burst that runs
    /// into it with no gap.
    #[test]
    fn anspcm_is_told_from_ansam_by_the_missing_15_hz_and_from_ans_by_the_qts_burst() {
        let (law, uqts, level) = (Law::Mu, 70u8, AnspcmLevel::Minus12);
        let seconds = 2.0;
        let anspcm = (QTS_TOTAL as u64..(seconds * 8000.0) as u64)
            .map(|n| anspcm_octet(level, law, n - QTS_TOTAL as u64));

        // The real thing: 75 ms of silence, QTS, QTS-bar, then ANSpcm.
        let mut net = Network::new(law, FS);
        let mut watch = AnspcmWatch::new(FS);
        let octets = (0..600u64)
            .map(|_| silence_octet(law))
            .chain((0..QTS_TOTAL as u64).map(|n| qts_octet(law, uqts, n).expect("inside QTS")))
            .chain(anspcm);
        for x in play(&mut net, law, octets) {
            watch.feed(x);
        }
        assert_eq!(watch.signal(), Some(AnswerSignal::Anspcm));
        assert!(watch.is_anspcm());
        // The reversal is still reported, and still in the right place: the
        // 600 codewords of silence and then 768 of QTS.
        let found = watch.qts_reversal().expect("the reversal was not found");
        let want = (600 + QTS_SYMBOLS) as f64 * FS / 8000.0;
        assert!(((found - want) * 8000.0 / FS).abs() <= 1.0, "{found} against {want}");
        // And the ANSam ear agrees it is a tone with no modulation.
        assert!(watch.tone().is_plain());

        // The same ANSpcm with no QTS in front of it is V.25's ANS as far as
        // anything on the line can tell, and that is the honest answer.
        let mut net = Network::new(law, FS);
        let mut watch = AnspcmWatch::new(FS);
        let octets = (0..(seconds * 8000.0) as u64).map(|n| anspcm_octet(level, law, n));
        for x in play(&mut net, law, octets) {
            watch.feed(x);
        }
        assert_eq!(watch.signal(), Some(AnswerSignal::Ans));
        assert!(watch.qts_reversal().is_none());

        // ANSam, which is what a far end that declined the quick connect
        // sends, is told apart by its 15 Hz modulation whether or not a QTS
        // burst went before.
        let mut watch = AnspcmWatch::new(FS);
        let mut phase = 0.0f64;
        for n in 0..(seconds * FS) as usize {
            let t = n as f64 / FS;
            let envelope = 1.0
                + v8::ansam::NOMINAL_DEPTH * (std::f64::consts::TAU * v8::ansam::MODULATION_RATE * t).sin();
            let sign = if ((t / 0.450) as u64).is_multiple_of(2) { 1.0 } else { -1.0 };
            phase += std::f64::consts::TAU * v8::ansam::ANSWER_TONE / FS;
            watch.feed(0.35 * envelope * sign * phase.sin());
        }
        assert_eq!(watch.signal(), Some(AnswerSignal::Ansam));
        assert!(!watch.is_anspcm());

        // A quiet line is none of them.
        let mut watch = AnspcmWatch::new(FS);
        for _ in 0..(FS as usize) {
            watch.feed(0.0);
        }
        assert_eq!(watch.signal(), None);
    }
}
