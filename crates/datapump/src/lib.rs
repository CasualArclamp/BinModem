//! Datapumps: the modulation layer.
//!
//! Each modulation is a streaming object. A receiver consumes line samples one
//! at a time and yields recovered data as it becomes available; nothing here
//! accepts a block and re-acquires from scratch.

pub mod bell103;
pub mod framing;

pub use bell103::{Bell103Rx, Role};
pub use framing::AsyncFramer;
