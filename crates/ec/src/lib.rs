//! Error control: ITU-T V.42 (LAPM) and V.42bis compression.
//!
//! What `CONNECT 33600/V42BIS` actually reports, and a prerequisite for
//! credible interoperation with real modems.

pub mod hdlc;

pub use hdlc::{Crc16, Crc32, Decoder, Encoder, Fcs, FrameError};
