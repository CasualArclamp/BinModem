//! The line side: everything between the datapump and the physical world.
//!
//! Today that is only file I/O, which is what the golden test vectors need.
//! Live audio (WASAPI via the virtual cable into the softphone), rate
//! conversion and clock-drift tracking land here as later milestones.

pub mod wav;

pub use wav::Wav;
