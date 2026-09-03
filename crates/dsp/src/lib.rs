//! Streaming DSP primitives shared by every modulation.
//!
//! Design rule for this crate: everything is sample-at-a-time and stateful.
//! A modem's timing recovery, carrier tracking, equaliser and echo canceller
//! are continuous adaptive loops that must never be reset at a buffer
//! boundary, so no public API here takes or returns a block of samples.
//!
//! This is the single most important departure from the previous attempt,
//! whose `modulate(bits) -> samples` / `demodulate(samples) -> bits` shape
//! forced every loop to re-acquire on each block.

pub mod fft;
pub mod filter;
pub mod fsk;
pub mod nco;

pub use fft::{Fft, Spectrum};
pub use filter::{Biquad, Cascade, OnePole, bandpass, butter_highpass, butter_lowpass};
pub use fsk::FskDetector;
pub use nco::Nco;
