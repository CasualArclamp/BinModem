//! The line side: everything between the datapump and the physical world.
//!
//! File I/O feeds the golden test vectors. The audio module monitors a line so
//! a person can hear it; the duplex module is the line itself, carrying signal
//! both ways between a modem and whatever the sound card is wired to.

pub mod audio;
pub mod duplex;
pub mod wav;

pub use audio::{AudioSink, Monitor, listen, output_devices};
pub use duplex::{Duplex, input_devices};
pub use wav::Wav;
