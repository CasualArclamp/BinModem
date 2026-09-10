//! A fax, from an image to the bits that go on the line.
//!
//! Two halves, and they are independent. T.4 says what a page is and how it
//! is coded; T.30 says what the two machines say to each other around it.
//! Neither of them is a modulation: the control channel is V.21 at 300 bit/s
//! and the page rides on whichever of V.27ter, V.29 or V.17 the two ends
//! agree on, and all three of those live in the data pump.

pub mod frames;
pub mod page;
pub mod t30;
pub mod t4;
