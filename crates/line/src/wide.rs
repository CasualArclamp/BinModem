//! A sound card used for all it has: both channels, at its own rate.
//!
//! [`crate::Duplex`] makes a sound card look like a telephone line: one
//! channel, converted to whatever rate the modulation likes. V.ARC wants the
//! opposite, the card as it is. Two channels kept apart, because a stereo
//! lead is two lines and each can carry its own signal; and no rate
//! conversion at all, because the modulation is built on the card's rate and
//! a converter in the path would only be a filter between it and the band it
//! is trying to use.
//!
//! What it shares with the other is the manners: a cushion in front of the
//! output so that a late writer is not a hole in the signal, the input
//! thrown away until the device has settled, and silence, counted, when
//! there is truly nothing to send.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Channels carried, whatever the device has: left and right.
pub const CHANNELS: usize = 2;

/// Frames kept in each direction before the oldest are dropped. Two seconds
/// at 48 kHz: this is a line for moving data, not for measuring an echo, and
/// what matters is that a busy moment in the program loses nothing.
const BUFFER: usize = 96_000;

/// Frames of silence put in front of the output: about forty milliseconds.
const PRIME: usize = 2048;

/// Frames of input thrown away while the device settles.
const SETTLE: usize = 12_000;

#[derive(Debug, Default)]
struct Counters {
    dropped_in: AtomicU64,
    dropped_out: AtomicU64,
    underruns: AtomicU64,
}

#[derive(Debug)]
struct Shared {
    /// Frames from the line, interleaved left and right.
    incoming: Mutex<VecDeque<f32>>,
    settling: Mutex<usize>,
    delivering: AtomicBool,
    outgoing: Mutex<VecDeque<f32>>,
    counters: Counters,
}

/// A running two-channel line at the device's own rate. Dropping it stops
/// both streams.
pub struct Wide {
    _input: cpal::Stream,
    _output: cpal::Stream,
    shared: Arc<Shared>,
    pub input_device: String,
    pub output_device: String,
    /// The rate the input runs at.
    pub rate: u32,
    /// The rate the output runs at. Two devices need not agree, and each
    /// is used at its own.
    pub output_rate: u32,
    /// Channels each device really has; one means left and right are the
    /// same signal.
    pub input_channels: usize,
    pub output_channels: usize,
}

impl std::fmt::Debug for Wide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wide")
            .field("input_device", &self.input_device)
            .field("output_device", &self.output_device)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

fn find(devices: impl Iterator<Item = cpal::Device>, name: &str, kind: &str) -> Result<cpal::Device, String> {
    let all: Vec<cpal::Device> = devices.collect();
    // The whole name first, then a name that contains what was asked for,
    // since nobody types "CABLE-A Output (VB-Audio Virtual Cable A)" twice.
    let lower = name.to_lowercase();
    let position = all
        .iter()
        .position(|d| d.to_string().eq_ignore_ascii_case(name))
        .or_else(|| all.iter().position(|d| d.to_string().to_lowercase().contains(&lower)));
    match position {
        Some(i) => Ok(all.into_iter().nth(i).expect("position came from this list")),
        None => Err(format!("no {kind} device named {name:?}")),
    }
}

impl Wide {
    /// Open both directions and start them. A device is named in full or by
    /// any part of its name; `None` is the system's default.
    pub fn open(input: Option<&str>, output: Option<&str>) -> Result<Self, String> {
        let host = crate::host();
        let in_device = match input {
            Some(name) => find(host.input_devices().map_err(|e| e.to_string())?, name, "input")?,
            None => host.default_input_device().ok_or("no default input device")?,
        };
        let out_device = match output {
            Some(name) => find(host.output_devices().map_err(|e| e.to_string())?, name, "output")?,
            None => host.default_output_device().ok_or("no default output device")?,
        };
        let in_name = in_device.to_string();
        let out_name = out_device.to_string();
        let in_config = in_device.default_input_config().map_err(|e| e.to_string())?;
        let out_config = out_device.default_output_config().map_err(|e| e.to_string())?;
        let rate = in_config.sample_rate();
        let output_rate = out_config.sample_rate();
        let in_channels = in_config.channels() as usize;
        let out_channels = out_config.channels() as usize;

        let shared = Arc::new(Shared {
            incoming: Mutex::new(VecDeque::with_capacity(BUFFER * CHANNELS)),
            settling: Mutex::new(SETTLE),
            delivering: AtomicBool::new(false),
            outgoing: Mutex::new(VecDeque::from(vec![0.0; PRIME * CHANNELS])),
            counters: Counters::default(),
        });

        let capture = Arc::clone(&shared);
        let input_stream = in_device
            .build_input_stream(
                in_config.config(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Blocking here is right: the reader holds this only to
                    // drain it, and a frame dropped is a slip no modulation
                    // this dense survives.
                    let Ok(mut queue) = capture.incoming.lock() else { return };
                    let Ok(mut left) = capture.settling.lock() else { return };
                    for frame in data.chunks(in_channels) {
                        if *left > 0 {
                            *left -= 1;
                            if *left == 0 {
                                capture.delivering.store(true, Ordering::Relaxed);
                            }
                            continue;
                        }
                        if queue.len() >= BUFFER * CHANNELS {
                            queue.drain(..CHANNELS);
                            capture.counters.dropped_in.fetch_add(1, Ordering::Relaxed);
                        }
                        queue.push_back(frame[0]);
                        queue.push_back(frame[if in_channels > 1 { 1 } else { 0 }]);
                    }
                },
                move |e| eprintln!("audio input error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        let playback = Arc::clone(&shared);
        let output_stream = out_device
            .build_output_stream(
                out_config.config(),
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if !playback.delivering.load(Ordering::Relaxed) {
                        out.fill(0.0);
                        return;
                    }
                    let Ok(mut queue) = playback.outgoing.lock() else {
                        out.fill(0.0);
                        return;
                    };
                    if queue.len() < out.len() / out_channels * CHANNELS {
                        playback.counters.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    for frame in out.chunks_mut(out_channels) {
                        let l = queue.pop_front().unwrap_or(0.0);
                        let r = queue.pop_front().unwrap_or(0.0);
                        frame.fill(0.0);
                        frame[0] = l;
                        if out_channels > 1 {
                            frame[1] = r;
                        }
                    }
                },
                move |e| eprintln!("audio output error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        input_stream.play().map_err(|e| e.to_string())?;
        output_stream.play().map_err(|e| e.to_string())?;

        Ok(Self {
            _input: input_stream,
            _output: output_stream,
            shared,
            input_device: in_name,
            output_device: out_name,
            rate,
            output_rate,
            input_channels: in_channels,
            output_channels: out_channels,
        })
    }

    /// Take every frame that has arrived, interleaved left and right.
    pub fn receive(&self, into: &mut Vec<f32>) {
        if let Ok(mut queue) = self.shared.incoming.lock() {
            into.extend(queue.drain(..));
        }
    }

    /// Queue frames for the line, interleaved left and right.
    pub fn transmit(&self, frames: &[f32]) {
        let Ok(mut queue) = self.shared.outgoing.lock() else { return };
        for frame in frames.as_chunks::<CHANNELS>().0 {
            if queue.len() >= BUFFER * CHANNELS {
                queue.drain(..CHANNELS);
                self.shared.counters.dropped_out.fetch_add(1, Ordering::Relaxed);
            }
            queue.extend(frame);
        }
    }

    /// Frames the output would take now to have a sixteenth of a second in
    /// hand. For a writer that is paced by the output device and not by the
    /// input: two devices have two clocks, and one that sent a frame for
    /// every frame heard would run the other dry or drown it in the end.
    pub fn wanted(&self) -> usize {
        (self.output_rate as usize / 16).saturating_sub(self.pending())
    }

    /// Frames waiting to go out.
    pub fn pending(&self) -> usize {
        self.shared.outgoing.lock().map(|q| q.len() / CHANNELS).unwrap_or(0)
    }

    /// Frames of input lost because nothing read them in time.
    pub fn dropped_in(&self) -> u64 {
        self.shared.counters.dropped_in.load(Ordering::Relaxed)
    }

    /// Frames of output discarded because the device was not taking them.
    pub fn dropped_out(&self) -> u64 {
        self.shared.counters.dropped_out.load(Ordering::Relaxed)
    }

    /// Output callbacks that ran short and sent silence.
    pub fn underruns(&self) -> u64 {
        self.shared.counters.underruns.load(Ordering::Relaxed)
    }
}
