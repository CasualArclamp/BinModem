//! The control channel of V.34's half-duplex mode (10.2.3.3, 10.2.4): the
//! full-duplex channel that every T.30 frame of a V.34 fax goes over.
//!
//! Half-duplex V.34 has two channels, used in turn (clause 12). The primary
//! channel carries the page one way at up to 33 600 bit/s. The control
//! channel carries everything else both ways at once: the MPh sequences that
//! settle the page's rate, and then T.30's frames. It is QAM at 600 baud,
//! two bits a symbol or four (1200 or 2400 bit/s), uncoded, scrambled by
//! clause 7's scrambler and differentially encoded, on 1200 Hz from the call
//! modem at the nominal level and on 2400 Hz from the answer modem 1 dB below
//! it, with an 1800 Hz guard tone 7 dB below (10.2.4). The two directions are
//! kept apart by frequency and nothing else, as phase 2's are (`dpsk.rs`):
//! V.34 has no echo canceller, and a two-wire line brings this end's own
//! signal back into its receiver.
//!
//! The signals are 10.2's, every one of them on 10.2.4's modulation:
//!
//! - PPh (10.2.4.5), four periods of an eight-symbol sequence, which starts a
//!   control channel and trains its receiver;
//! - ALT (10.2.4.2), scrambled alternations of 0 and 1 from a scrambler
//!   started at zero, which follows PPh and Sh, S-bar-h;
//! - MPh (10.2.4.4), any bits the caller gives, sent whole and repeated;
//! - E (10.2.4.3), twenty scrambled ones, after which comes the user's data;
//! - Sh and S-bar-h (10.2.3.3), which resynchronise a control channel that
//!   has been silent for a page;
//! - AC (10.2.4.1), which asks the far end for a control-channel retrain
//!   (12.8);
//! - and 12.6.3's 4T of scrambled ones, with which a control channel turns
//!   off.
//!
//! [`Transmitter`] sends them from a queue of [`Segment`]s. [`Receiver`] picks
//! the far end's band out from under this end's own signal, says what it
//! hears ([`Hearing`]), trains on PPh or on Sh and S-bar-h, and hands up the
//! bits. [`Modem`] is the two together. What to send when -- the procedures of
//! 12.4, 12.6 and 12.8 -- is not here.
//!
//! Three things the Recommendation leaves open are settled here (see
//! `docs/design/superg3/plan.md` 8):
//!
//! - PPh is built as 10-2 defines its terms, with I, and the printed formula
//!   is kept beside it ([`Reading`]); the receiver recognises either and says
//!   which it heard.
//! - ALT starts with a 0, and the differential encoder starts ALT at Z = 0;
//!   a receiver of a modem that does otherwise loses a symbol at most.
//! - Data after E stays differentially encoded.
//!
//! And one more: every signal leaves at the same mean power, a unit-power
//! symbol, which is the nominal level (10.2.4 gives the levels but not how
//! PPh, AC, Sh and the two constellations stand against each other).

mod receiver;
mod transmitter;
mod watch;

#[cfg(test)]
mod tests;

use dsp::Complex;

pub use receiver::{Heard, Phase, Receiver, Reference};
pub use transmitter::{Kind, Segment, Sent, Transmitter};
pub use watch::Hearing;

use super::constellation::{Point, counterclockwise, quarter};
use super::dpsk::Side;
use super::receiver::unit;
use super::signals::{self, Size};
use crate::v32::Mode;

/// Symbols a second: "600 ± 0.01% symbol/s" (10.2.4), phase 2's own rate.
pub const BAUD: f64 = super::dpsk::BAUD;

/// PPh is "four periods of an 8-symbol sequence" (10.2.4.5).
pub const PPH_PERIOD: usize = 8;
pub const PPH_SYMBOLS: usize = 4 * PPH_PERIOD;

/// Sh and S-bar-h, in symbols: 24T and 8T, from 12.6.1.1 and 12.6.2.2 (10.2
/// defines the signals and leaves their lengths to clause 12).
pub const SH_SYMBOLS: usize = 24;
pub const SH_BAR_SYMBOLS: usize = 8;

/// The least ALT a modem sends after PPh or S-bar-h, and the most before its
/// MPh or E (12.4.1.1, 12.4.2.3, 12.6.1.4).
pub const ALT_LEAST: usize = 16;
pub const ALT_MOST: usize = 120;

/// E is "a 20-bit sequence of scrambled binary ones ... at 1200 bit/s"
/// (10.2.4.3): ten symbols.
pub const E_BITS: usize = signals::E_BITS;
pub const E_SYMBOLS: usize = E_BITS / 2;

/// The scrambled ones a control channel turns off with (12.6.3.1, 12.6.3.2).
pub const TURN_OFF_SYMBOLS: usize = 4;

/// The 70 ms of silence clause 12 puts before PPh and Sh (12.4.1.1,
/// 12.6.1.1): 42 symbols exactly.
pub const SILENCE_SYMBOLS: usize = 42;

/// How long AC has to have been heard for before the far end is answered
/// with PPh: "more than 100 ms" (12.8.2).
pub const AC_SECONDS: f64 = 0.1;

/// The control channel's two data rates (10.2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rate {
    /// Two bits a symbol, on point 0 turned: the rate every training and
    /// synchronising signal goes at.
    #[default]
    R1200,
    /// Four bits a symbol: the first two turn, the last two pick one of
    /// Figure 5's points 0 to 3.
    R2400,
}

impl Rate {
    pub fn bits(self) -> usize {
        self.size().bits()
    }

    /// The constellation it is sent on.
    pub fn size(self) -> Size {
        match self {
            Self::R1200 => Size::Four,
            Self::R2400 => Size::Sixteen,
        }
    }

    pub fn bits_per_second(self) -> u32 {
        match self {
            Self::R1200 => 1200,
            Self::R2400 => 2400,
        }
    }
}

/// Which PPh: the one equation 10-2 means, or the one it prints.
///
/// 10.2.4.5 sets i = 2k + I, "k = 0, 1, 2, ..., 15; and I = 0, 1 for each k",
/// and then prints PPh(i) = exp(j pi [2k(k-1)+1] / 4): a digit one where the
/// clause has just defined I and never uses it (read at 700 dpi off PDF page
/// 46; the glyph is the flagged one of the "0, 1" beside it, not the serif
/// capital). As printed, PPh is p p p p n n n n four times over, a square wave
/// on one diagonal whose periodic autocorrelation has sidelobes of a half and
/// of one -- a poor thing to train an equaliser on, which is what 12.4 trains
/// on it. Read with I it is a perfect sequence on the four diagonal points,
/// every periodic sidelobe nought, of duplex PP's family (10-1 is built on k
/// times I the same way). So I is what is sent, and the printed reading is
/// kept by name: the receiver recognises both and says which it heard, and
/// the first recording of a real modem's control channel settles it
/// (`plan.md` 8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reading {
    /// exp(j pi (2k(k - I) + 1) / 4).
    #[default]
    WithI,
    /// exp(j pi (2k(k - 1) + 1) / 4), as the 02/98 PDF prints it.
    AsPrinted,
}

impl Reading {
    /// Symbol `i` of PPh in this reading, at unit magnitude.
    pub fn point(self, i: usize) -> Complex {
        match self {
            Self::WithI => pph(i),
            Self::AsPrinted => pph_as_printed(i),
        }
    }
}

/// Symbol `i` of PPh (10-2) read with I, i = 2k + I: exp(j pi (2k(k - I) +
/// 1) / 4). PPh(0) is sent first.
///
/// The exponent is always an odd number of eighths of a turn, so every symbol
/// is one of the four diagonal points, and it repeats every eight: moving k on
/// by 4 adds a multiple of 8 to 2k(k - I).
pub fn pph(i: usize) -> Complex {
    let (k, big_i) = ((i / 2) as i64, (i % 2) as i64);
    eighth_turns(2 * k * (k - big_i) + 1)
}

/// Symbol `i` of PPh as 10-2 prints it: exp(j pi (2k(k - 1) + 1) / 4), with no
/// I in it, so each value comes twice.
pub fn pph_as_printed(i: usize) -> Complex {
    let k = (i / 2) as i64;
    eighth_turns(2 * k * (k - 1) + 1)
}

/// exp(j pi n / 4).
fn eighth_turns(n: i64) -> Complex {
    Complex::from_polar(1.0, std::f64::consts::PI * n.rem_euclid(8) as f64 / 4.0)
}

/// Symbol `n` of Sh: "alternating between point 0 of the quarter-
/// superconstellation of Figure 5 and the same point rotated counterclockwise
/// by 90 degrees" (10.2.3.3), point 0 first, so that 24 of them end on the
/// quarter turn as the clause asks. Duplex S's pattern exactly
/// (`signals::s`), at 600 baud.
pub fn sh(n: usize) -> Complex {
    unit_point(signals::s(n), Size::Four)
}

/// Symbol `n` of S-bar-h: point 0 turned 180 and 270 degrees
/// counterclockwise, alternately, beginning with 180 (10.2.3.3): Sh turned
/// round.
pub fn sh_bar(n: usize) -> Complex {
    unit_point(signals::s_bar(n), Size::Four)
}

/// Symbol `n` of AC: "the alternating transmission of point 0 ... and point 0
/// rotated by 180 degrees" (10.2.4.1).
pub fn ac(n: usize) -> Complex {
    unit_point(counterclockwise(quarter(0), 2 * (n % 2) as u32), Size::Four)
}

/// A point of the four- or sixteen-point set, at unit mean power: the
/// grid over the square root of two or of ten.
pub(crate) fn unit_point(point: Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(unit(size))
}

/// The scrambler a side sends with: "GPC for the call modem, GPA for the
/// answer modem" (clause 7).
fn scrambler_of(side: Side) -> Mode {
    match side {
        Side::Call => Mode::Call,
        Side::Answer => Mode::Answer,
    }
}

/// The other end of the call.
fn far(side: Side) -> Side {
    match side {
        Side::Call => Side::Answer,
        Side::Answer => Side::Call,
    }
}

/// Samples a symbol at `fs`, as a fraction in lowest terms: 80/3 at 16 kHz.
fn samples_per_symbol(fs: f64) -> (u64, u64) {
    let (mut a, mut b) = (fs.round() as u64, BAUD as u64);
    let (p, q) = (a, b);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    (p / a, q / a)
}

/// One end of a control channel: a transmitter and a receiver on one pair of
/// wires, one sample in and one out at a time.
///
/// The two halves share nothing but the line. Each is the caller's to drive
/// directly; this only steps them together.
#[derive(Debug)]
pub struct Modem {
    pub transmitter: Transmitter,
    pub receiver: Receiver,
}

impl Modem {
    /// The call or answer end, at `fs` samples a second.
    pub fn new(side: Side, fs: f64) -> Self {
        Self { transmitter: Transmitter::new(side, fs), receiver: Receiver::new(side, fs) }
    }

    pub fn side(&self) -> Side {
        self.transmitter.side()
    }

    /// One sample of the line in, and one of this end's out.
    pub fn step(&mut self, sample: f64) -> f64 {
        self.receiver.feed(sample);
        self.transmitter.next_sample()
    }
}
