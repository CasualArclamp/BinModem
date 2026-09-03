//! Error control: ITU-T V.42 (LAPM) and V.42bis compression.
//!
//! What `CONNECT 33600/V42BIS` actually reports, and a prerequisite for
//! credible interoperation with real modems.

pub mod frame;
pub mod hdlc;
pub mod lapm;
pub mod v42bis;
pub mod xid;

pub use frame::{Address, Frame, Kind, Role};
pub use lapm::{Cause, Event, Lapm, Params, State};
pub use xid::{Compression, Xid};
pub use hdlc::{Crc16, Crc32, Decoder, Encoder, Fcs, FrameError};
