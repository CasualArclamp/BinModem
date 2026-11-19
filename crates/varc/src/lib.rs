//! V.ARC: BinModem's own wideband mode.

// Nearly every loop in here is over carrier numbers or tap numbers, and the
// number is wanted for itself as well as for the several arrays it indexes.
#![allow(clippy::needless_range_loop)]

pub mod dmt;
pub mod frame;
pub mod interp;
pub mod link;
pub mod map;
pub mod rs;
