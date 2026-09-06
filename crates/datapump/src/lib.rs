//! Datapumps: the modulation layer.
//!
//! Each modulation is a streaming object. A receiver consumes line samples one
//! at a time and yields recovered data as it becomes available; nothing here
//! accepts a block and re-acquires from scratch.

pub mod bell103;
pub mod framing;
pub mod v22bis;
pub mod v32;
pub mod v8;

pub use bell103::{Bell103Rx, Role};
pub use framing::{AsyncBits, AsyncFramer};
pub use v22bis::{Channel, Receiver as V22bisRx, Transmitter as V22bisTx};
pub use v32::{Mode as V32Mode, Receiver as V32Rx, Transmitter as V32Tx};
