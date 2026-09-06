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

pub mod echo;
pub mod equalizer;
pub mod fft;
pub mod filter;
pub mod fsk;
pub mod nco;
pub mod resample;
pub mod shaping;
pub mod tone;

pub use echo::{EchoCanceller, EchoFinder, Reflection};
pub use equalizer::Equalizer;
pub use fft::{Fft, Spectrum};
pub use filter::{Biquad, Cascade, OnePole, bandpass, butter_highpass, butter_lowpass};
pub use fsk::FskDetector;
pub use nco::Nco;
pub use resample::Resampler;
pub use shaping::{
    ComplexFir, Fir, Gardner, fir_lowpass, fir_lowpass_kaiser, rrc_at, rrc_taps,
};
pub use tone::{ReversalDetector, ToneDetector};
