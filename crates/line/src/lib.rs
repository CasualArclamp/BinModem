//! The line side: everything between the datapump and the physical world.
//!
//! File I/O feeds the golden test vectors. The audio module monitors a line so
//! a person can hear it; the duplex module is the line itself, carrying signal
//! both ways between a modem and whatever the sound card is wired to.

pub mod audio;
pub mod duplex;
pub mod wav;

pub use audio::{AudioSink, Monitor, listen, output_devices};

/// The audio system devices are listed from and opened on.
///
/// cpal's own choice everywhere but Linux. There it is the sound server when
/// one is running -- PipeWire or PulseAudio, through the protocol both speak
/// -- so that a device this program plays through or listens on is still
/// there for everything else; opened through ALSA directly, the card is this
/// program's alone until it lets go, and the desktop's own mixer loses it.
/// `BINMODEM_AUDIO_HOST=alsa` asks for the hardware all the same, which a
/// modem wired to a real line through a sound card may well want: one layer
/// fewer between it and the codec.
pub(crate) fn host() -> cpal::Host {
    #[cfg(target_os = "linux")]
    if std::env::var("BINMODEM_AUDIO_HOST").is_ok_and(|name| name.eq_ignore_ascii_case("alsa"))
        && let Ok(host) = cpal::host_from_id(cpal::HostId::Alsa)
    {
        return host;
    }
    cpal::default_host()
}
pub use duplex::{Duplex, input_devices};
pub use wav::Wav;
