//! A full-duplex audio line: what a modem is actually plugged into.
//!
//! Two streams, which on a two-wire line means two directions sharing one pair
//! and on a sound card means two devices that have never heard of each other.
//! Between them and the modem sit two rate conversions, because a modem runs
//! at a rate that suits its modulation and a sound card runs at 48 kHz and is
//! not open to discussion.
//!
//! The awkward part is that the two devices have their own clocks, and neither
//! is the modem's. Over a long call the input delivers slightly more or fewer
//! samples than the output consumes, and something has to give. What gives
//! here is the output: when the buffer runs dry it emits silence and counts
//! the fact. That is honest and recoverable — a far-end modem sees a brief
//! dropout, which is what error control is for — where quietly dropping or
//! repeating samples would corrupt the signal in a way nothing downstream
//! could distinguish from a bad line.
//!
//! Using one device for both directions avoids the drift entirely, and is
//! worth doing wherever the wiring allows it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dsp::Resampler;

/// How much audio to keep buffered in each direction, in modem samples.
///
/// Enough to ride out a scheduling hiccup and short enough that the delay it
/// adds does not matter. A modem's own round-trip measurement is in tens of
/// milliseconds, so this must stay well under that or it becomes the round
/// trip.
const BUFFER: usize = 4096;

/// Silence put in front of the modem's own output, in modem samples.
///
/// Without it the output queue is fed exactly as fast as it is drained, so it
/// sits at zero and every scheduling hiccup is a hole in the transmitted
/// signal. That is not a theoretical worry: on a real call this ran dry on
/// essentially every callback, some five thousand of them, and what went out
/// on the line was mostly silence with a modem in the gaps. Nothing can
/// negotiate through that.
///
/// Sixty-four milliseconds is a few output callbacks' worth, which is enough
/// to absorb the writer being late without putting a noticeable delay in the
/// line. It does become part of the round trip, which is why it is not larger:
/// an echo canceller has to reach back over it.
const PRIME: usize = 1024;

/// How far past the current callback to convert while the queue is in hand,
/// in output samples. One ordinary callback's worth.
const RESERVE: usize = 2048;

/// Input to throw away before handing any over, in modem samples.
///
/// A quarter of a second. Neither the device nor the rate conversion produces
/// anything meaningful the instant it starts: the resampler's kernel is still
/// filling, and hardware settles into its own rhythm. What comes out in the
/// meantime is a transient, and a modem handed a transient believes it.
///
/// That is not a hypothetical. Given it, a V.32 modem ran its entire
/// round-trip measurement inside the first fifth of a second, against a far
/// end that had not started transmitting: it heard its own start-up in the
/// detectors and read the tones and phase reversals it was waiting for out of
/// them. Nothing downstream can defend against that, because by the time the
/// samples reach a modem they are indistinguishable from a line.
const SETTLE: usize = 4000;

#[derive(Debug, Default)]
struct Counters {
    /// Input samples dropped because whatever is reading was not keeping up.
    /// Fatal for a modem: timing recovery has no way to know a sample went
    /// missing, and reads the gap as the clock having moved.
    dropped_in: AtomicU64,
    /// Output samples dropped because nothing was draining them, which means
    /// the writer is running ahead of the device and only adding delay.
    dropped_out: AtomicU64,
    /// Output blocks that found nothing to send and sent silence.
    underruns: AtomicU64,
    /// Times a callback could not take the lock and gave up.
    contended: AtomicU64,
}

#[derive(Debug)]
struct Shared {
    /// Arrived from the line, at the modem's rate.
    incoming: Mutex<VecDeque<f32>>,
    /// Samples still to be thrown away while the path settles.
    settling: Mutex<usize>,
    /// Whether the input has finished settling and is handing samples over.
    ///
    /// The output waits for it. Until the input delivers, the modem is not
    /// being stepped and so has nothing whatever to say, and a cushion spent
    /// covering that silence is a cushion that has been used up by the time
    /// the call it was for begins. Which is what happened: a quarter second of
    /// settling put two dozen holes in the line, all of them in the opening
    /// moments of the handshake, which is the one stretch where a far end has
    /// nothing to fall back on and no error control yet to hide them.
    delivering: AtomicBool,
    /// Waiting to go to the line, at the modem's rate.
    outgoing: Mutex<VecDeque<f32>>,
    counters: Counters,
}

/// A running full-duplex line. Dropping it stops both streams.
pub struct Duplex {
    _input: cpal::Stream,
    _output: cpal::Stream,
    shared: Arc<Shared>,
    pub input_device: String,
    pub output_device: String,
    pub input_rate: u32,
    pub output_rate: u32,
}

impl std::fmt::Debug for Duplex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Duplex")
            .field("input_device", &self.input_device)
            .field("output_device", &self.output_device)
            .field("input_rate", &self.input_rate)
            .field("output_rate", &self.output_rate)
            .finish_non_exhaustive()
    }
}

/// Names of the available input devices, default first.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|d| d.to_string());
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    if let Some(d) = default
        && let Some(i) = names.iter().position(|n| *n == d)
    {
        names.swap(0, i);
    }
    names
}

fn find_input(want: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match want {
        Some(name) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.to_string().eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no input device named {name:?}")),
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input device".to_string()),
    }
}

fn find_output(want: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match want {
        Some(name) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.to_string().eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no output device named {name:?}")),
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string()),
    }
}

impl Duplex {
    /// Open both directions and start them.
    ///
    /// `modem_rate` is the rate the modem wants to see; everything either side
    /// of this is converted to and from it.
    pub fn open(
        input: Option<&str>,
        output: Option<&str>,
        modem_rate: f64,
    ) -> Result<Self, String> {
        let in_device = find_input(input)?;
        let out_device = find_output(output)?;
        let in_name = in_device.to_string();
        let out_name = out_device.to_string();

        let in_config = in_device.default_input_config().map_err(|e| e.to_string())?;
        let out_config = out_device
            .default_output_config()
            .map_err(|e| e.to_string())?;
        let in_rate = in_config.sample_rate();
        let out_rate = out_config.sample_rate();
        let in_channels = in_config.channels() as usize;
        let out_channels = out_config.channels() as usize;

        let shared = Arc::new(Shared {
            incoming: Mutex::new(VecDeque::with_capacity(BUFFER)),
            settling: Mutex::new(SETTLE),
            delivering: AtomicBool::new(false),
            outgoing: Mutex::new(VecDeque::from(vec![0.0; PRIME])),
            counters: Counters::default(),
        });

        // ---- the line into the modem -----------------------------------
        let capture = Arc::clone(&shared);
        let mut down = Resampler::new(f64::from(in_rate), modem_rate);
        let mut converted: Vec<f64> = Vec::with_capacity(1024);
        let input_stream = in_device
            .build_input_stream(
                in_config.config(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    converted.clear();
                    for frame in data.chunks(in_channels) {
                        // A two-wire tap is mono however many channels the
                        // card insists on presenting.
                        let mono = frame.iter().sum::<f32>() / in_channels as f32;
                        down.process(f64::from(mono), &mut converted);
                    }
                    let Ok(mut queue) = capture.incoming.try_lock() else {
                        capture.counters.contended.fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    let mut left = capture.settling.try_lock().ok();
                    for &s in converted.iter() {
                        if let Some(n) = left.as_deref_mut()
                            && *n > 0
                        {
                            *n -= 1;
                            if *n == 0 {
                                capture.delivering.store(true, Ordering::Relaxed);
                            }
                            continue;
                        }
                        if queue.len() >= BUFFER {
                            // The modem is not keeping up, which is a fault in
                            // the modem and not something to hide by growing
                            // without limit until the machine runs out.
                            queue.pop_front();
                            capture.counters.dropped_in.fetch_add(1, Ordering::Relaxed);
                        }
                        queue.push_back(s as f32);
                    }
                },
                move |e| eprintln!("audio input error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        // ---- the modem out onto the line -------------------------------
        let playback = Arc::clone(&shared);
        let mut up = Resampler::new(modem_rate, f64::from(out_rate));
        let mut ready: VecDeque<f64> = VecDeque::with_capacity(1024);
        let mut produced: Vec<f64> = Vec::with_capacity(64);
        let output_stream = out_device
            .build_output_stream(
                out_config.config(),
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = out.len() / out_channels;
                    if !playback.delivering.load(Ordering::Relaxed) {
                        // The input has not started, so the modem has not been
                        // stepped, so there is nothing to send and silence is
                        // the correct thing to send. Not a fault, and not
                        // something to spend the cushion on: leaving the queue
                        // alone keeps it full for the moment it is wanted.
                        out.fill(0.0);
                        return;
                    }
                    // Convert past what this callback needs, so that a lock
                    // missed next time is already covered. The writer holds
                    // this mutex for a whole block at a time, so missing it is
                    // ordinary rather than exceptional, and treating every
                    // miss as silence would put holes in the signal for no
                    // reason at all.
                    match playback.outgoing.try_lock() {
                        Ok(mut queue) => {
                            while ready.len() < frames + RESERVE {
                                let Some(next) = queue.pop_front() else { break };
                                produced.clear();
                                up.process(f64::from(next), &mut produced);
                                ready.extend(produced.iter().copied());
                            }
                        }
                        Err(_) => {
                            playback.counters.contended.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if ready.len() < frames {
                        // Genuinely nothing to send. Silence is the honest
                        // thing to put out -- a far end hears a dropout, which
                        // is what error control is for -- but it is a fault
                        // and is counted as one.
                        playback.counters.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    for frame in out.chunks_mut(out_channels) {
                        let s = ready.pop_front().unwrap_or(0.0) as f32;
                        for slot in frame.iter_mut() {
                            *slot = s;
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
            input_rate: in_rate,
            output_rate: out_rate,
        })
    }

    /// Whether the path has settled and is delivering.
    pub fn settled(&self) -> bool {
        self.shared.settling.lock().map(|n| *n == 0).unwrap_or(false)
    }

    /// Take everything that has arrived from the line, at the modem's rate.
    pub fn receive(&self, into: &mut Vec<f32>) {
        let Ok(mut queue) = self.shared.incoming.lock() else {
            return;
        };
        into.extend(queue.drain(..));
    }

    /// Queue samples for the line, at the modem's rate.
    pub fn transmit(&self, samples: &[f32]) {
        let Ok(mut queue) = self.shared.outgoing.lock() else {
            return;
        };
        for &s in samples {
            if queue.len() >= BUFFER {
                // Nothing is consuming this, so adding to it only adds delay.
                self.shared
                    .counters
                    .dropped_out
                    .fetch_add(1, Ordering::Relaxed);
                queue.pop_front();
            }
            queue.push_back(s);
        }
    }

    /// How many samples are waiting to go out, which is the delay this adds.
    pub fn pending(&self) -> usize {
        self.shared.outgoing.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Input samples lost because nothing read them in time.
    pub fn dropped_in(&self) -> u64 {
        self.shared.counters.dropped_in.load(Ordering::Relaxed)
    }

    /// Output samples discarded because nothing was sending them.
    pub fn dropped_out(&self) -> u64 {
        self.shared.counters.dropped_out.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u64 {
        self.shared.counters.underruns.load(Ordering::Relaxed)
    }

    pub fn contended(&self) -> u64 {
        self.shared.counters.contended.load(Ordering::Relaxed)
    }
}
