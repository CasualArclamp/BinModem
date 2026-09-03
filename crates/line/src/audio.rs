//! Audio output, for monitoring the line.
//!
//! This is the first piece of the live-line path. The same device plumbing will
//! later carry the transmit signal into the virtual cable feeding the softphone,
//! so the rate conversion and drift handling here are not throwaway.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// A bounded buffer between the modem and the audio device.
///
/// The producer is the modem; the consumer is the device callback. The callback
/// must never block, so it takes the lock with `try_lock` and emits silence if
/// the producer happens to hold it. The critical section is a memcpy touched
/// once per engine tick, so losing the race is rare and costs one buffer of
/// silence rather than an audio glitch that stalls the whole stream.
#[derive(Debug)]
pub struct AudioSink {
    buf: Mutex<VecDeque<f32>>,
    enabled: AtomicBool,
    capacity: usize,
    /// Samples dropped because the buffer was full: the monitor is behind.
    overruns: AtomicU64,
    /// Callbacks that found too little data: the monitor is starved.
    underruns: AtomicU64,
}

impl AudioSink {
    /// `capacity` is in samples at the source rate. About a quarter second is
    /// enough to ride out scheduling jitter without adding audible latency.
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(capacity)),
            enabled: AtomicBool::new(false),
            capacity,
            overruns: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on && let Ok(mut b) = self.buf.lock() {
            b.clear();
        }
    }

    /// Queue samples for playback. Ignored when monitoring is off.
    pub fn push(&self, samples: &[f32]) {
        if !self.enabled() {
            return;
        }
        if let Ok(mut b) = self.buf.lock() {
            for &s in samples {
                if b.len() >= self.capacity {
                    // Drop the oldest: a monitor should track the present, not
                    // fall further and further behind.
                    b.pop_front();
                    self.overruns.fetch_add(1, Ordering::Relaxed);
                }
                b.push_back(s);
            }
        }
    }

    /// Take up to `out.len()` samples. Returns how many were written.
    fn drain(&self, out: &mut [f32]) -> usize {
        match self.buf.try_lock() {
            Ok(mut b) => {
                let n = out.len().min(b.len());
                for slot in out.iter_mut().take(n) {
                    *slot = b.pop_front().unwrap_or(0.0);
                }
                if n < out.len() {
                    self.underruns.fetch_add(1, Ordering::Relaxed);
                }
                n
            }
            Err(_) => 0,
        }
    }

    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

/// Names of the available output devices, default first.
pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_output_device().map(|d| d.to_string());
    let mut names: Vec<String> = host
        .output_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    if let Some(d) = default
        && let Some(i) = names.iter().position(|n| *n == d)
    {
        names.swap(0, i);
    }
    names
}

/// A running output stream. Dropping it stops playback.
pub struct Monitor {
    stream: cpal::Stream,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("device", &self.device)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl Monitor {
    pub fn play(&self) -> Result<(), String> {
        self.stream.play().map_err(|e| e.to_string())
    }
}

/// Open an output stream that plays whatever the modem pushes into `sink`.
///
/// `source_rate` is the modem's sample rate. The device almost never runs at
/// that rate — 48 kHz is typical against our 16 kHz — so the callback
/// resamples. Linear interpolation is sufficient here: this is a monitor, and
/// the signal is already band-limited well below the output Nyquist.
pub fn listen(
    sink: Arc<AudioSink>,
    device_name: Option<&str>,
    source_rate: f64,
) -> Result<Monitor, String> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(want) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.to_string() == want)
            .ok_or_else(|| format!("no output device named {want:?}"))?,
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?,
    };

    let name = device.to_string();
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let sample_rate = config.sample_rate();
    let channels = config.channels();
    let step = source_rate / sample_rate as f64;

    // Interpolation state, owned by the callback.
    let mut pending: Vec<f32> = Vec::new();
    let mut pos = 0.0f64;

    let err = |e| eprintln!("audio output error: {e}");
    let stream = device
        .build_output_stream(
            config.config(),
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels as usize;

                // Pull enough source samples to cover this block, plus one for
                // the interpolator to read ahead into.
                let needed = (pos + step * frames as f64).ceil() as usize + 2;
                if pending.len() < needed {
                    let mut extra = vec![0.0f32; needed - pending.len()];
                    let got = sink.drain(&mut extra);
                    extra.truncate(got);
                    pending.extend_from_slice(&extra);
                }

                for frame in out.chunks_mut(channels as usize) {
                    let i = pos as usize;
                    let sample = if i + 1 < pending.len() {
                        let frac = (pos - i as f64) as f32;
                        pending[i] * (1.0 - frac) + pending[i + 1] * frac
                    } else {
                        0.0
                    };
                    for slot in frame.iter_mut() {
                        *slot = sample;
                    }
                    pos += step;
                }

                // Discard what has been consumed, keeping the fractional phase.
                let consumed = pos as usize;
                if consumed > 0 && consumed <= pending.len() {
                    pending.drain(..consumed);
                    pos -= consumed as f64;
                } else if consumed > pending.len() {
                    pending.clear();
                    pos = 0.0;
                }
            },
            err,
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    Ok(Monitor { stream, device: name, sample_rate, channels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_sink_accepts_nothing() {
        let sink = AudioSink::new(64);
        sink.push(&[1.0; 32]);
        let mut out = [0.0; 32];
        assert_eq!(sink.drain(&mut out), 0);
    }

    #[test]
    fn samples_come_back_in_order() {
        let sink = AudioSink::new(64);
        sink.set_enabled(true);
        sink.push(&[1.0, 2.0, 3.0]);
        let mut out = [0.0; 3];
        assert_eq!(sink.drain(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_full_sink_drops_the_oldest() {
        let sink = AudioSink::new(4);
        sink.set_enabled(true);
        sink.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut out = [0.0; 4];
        assert_eq!(sink.drain(&mut out), 4);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0], "should keep the newest");
        assert_eq!(sink.overruns(), 2);
    }

    #[test]
    fn a_short_read_is_recorded_as_an_underrun() {
        let sink = AudioSink::new(64);
        sink.set_enabled(true);
        sink.push(&[1.0, 2.0]);
        let mut out = [0.0; 8];
        assert_eq!(sink.drain(&mut out), 2);
        assert_eq!(sink.underruns(), 1);
    }

    #[test]
    fn disabling_discards_whatever_was_queued() {
        let sink = AudioSink::new(64);
        sink.set_enabled(true);
        sink.push(&[1.0; 16]);
        sink.set_enabled(false);
        sink.set_enabled(true);
        let mut out = [0.0; 16];
        assert_eq!(sink.drain(&mut out), 0, "stale audio should not resume");
    }

    /// Device enumeration must not panic on a machine with no sound card,
    /// which is the case in most continuous integration environments.
    #[test]
    fn enumerating_devices_is_safe() {
        let _ = output_devices();
    }
}
