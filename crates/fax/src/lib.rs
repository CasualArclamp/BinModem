//! A fax, from an image to the bits that go on the line.
//!
//! Two halves, and they are independent. T.4 says what a page is and how it
//! is coded; T.30 says what the two machines say to each other around it.
//! Neither of them is a modulation: the control channel is V.21 at 300 bit/s
//! and the page rides on whichever of V.27ter, V.29 or V.17 the two ends
//! agree on, and all three of those live in the data pump.

pub mod call;
pub mod frames;
pub mod mr;
pub mod page;
pub mod t30;
pub mod t4;

/// The calling tone, on and off, in seconds (T.30 5.1.1).
///
/// Kept here rather than in the data pump because it is a rule of the
/// procedure and not a property of a signal: what 1100 Hz sounds like belongs
/// to the modulation, when to send it belongs to T.30.
pub const CNG_ON: f64 = 0.5;
pub const CNG_OFF: f64 = 3.0;
