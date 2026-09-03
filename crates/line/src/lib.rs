//! The line side: everything between the datapump and the physical world.
//!
//! File I/O feeds the golden test vectors; the audio module is the beginning of
//! the live path that will eventually carry signal to and from the virtual
//! cable into the softphone.

pub mod audio;
pub mod wav;

pub use audio::{AudioSink, Monitor, listen, output_devices};
pub use wav::Wav;
