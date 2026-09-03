//! Drives the receiver and publishes telemetry.
//!
//! Today the sample source is a WAV file paced to real time. That is
//! deliberate: the loop below has the same shape a WASAPI callback will have,
//! so replacing the source with live audio changes where samples come from and
//! nothing else.
//!
//! A 2-wire capture carries both directions summed, so two receivers run in
//! parallel — one on each Bell 103 band — and the transcript shows both sides
//! of the conversation separately.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use datapump::Bell103Rx;
use line::AudioSink;
use dsp::Spectrum;
use telemetry::{CallState, Direction, Leds, Publisher};

pub const SCOPE_LEN: usize = 1024;
pub const FFT_SIZE: usize = 1024;
pub const SPECTRUM_BINS: usize = FFT_SIZE / 2;
/// Symbols kept for the scope: roughly eight characters at 8N1, enough to see
/// the cluster spread without smearing the display with ancient history.
pub const SYMBOL_HISTORY: usize = 80;

/// Shared controls the UI writes and the engine reads.
#[derive(Debug)]
pub struct Control {
    pub running: AtomicBool,
    pub restart: AtomicBool,
    pub quit: AtomicBool,
    /// Playback speed in percent, so a call can be slowed down to watch.
    pub speed_pct: AtomicU32,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(true),
            restart: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            speed_pct: AtomicU32::new(100),
        }
    }
}

/// Accumulates received bytes into transcript lines.
struct LineAssembler {
    direction: Direction,
    buffer: Vec<u8>,
}

impl LineAssembler {
    fn new(direction: Direction) -> Self {
        Self { direction, buffer: Vec::new() }
    }

    fn push(&mut self, byte: u8, tx: &Publisher) {
        // Flush on either terminator, and swallow the paired one so CRLF does
        // not produce an empty second line.
        if byte == b'\r' || byte == b'\n' {
            self.flush(tx);
        } else {
            self.buffer.push(byte);
            if self.buffer.len() >= 160 {
                self.flush(tx);
            }
        }
    }

    fn flush(&mut self, tx: &Publisher) {
        if !self.buffer.is_empty() {
            tx.log_bytes(self.direction, &self.buffer);
            self.buffer.clear();
        }
    }
}

/// A fixed-capacity ring the scopes are drawn from.
struct Ring {
    data: Vec<f32>,
    write: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self { data: vec![0.0; len], write: 0 }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.data[self.write] = x;
        self.write = (self.write + 1) % self.data.len();
    }

    /// Copy out oldest-first.
    fn copy_into(&self, out: &mut [f32]) {
        let n = self.data.len();
        for (i, slot) in out.iter_mut().enumerate().take(n) {
            *slot = self.data[(self.write + i) % n];
        }
    }
}

/// Start the engine on its own thread.
pub fn spawn(
    path: &Path,
    tx: Publisher,
    control: Arc<Control>,
    sink: Arc<AudioSink>,
) -> std::io::Result<JoinHandle<()>> {
    let wav = line::wav::read(path)?;
    let samples = wav.mono();
    let fs = wav.sample_rate as f64;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(thread::spawn(move || {
        run(samples, fs, name, tx, control, sink);
    }))
}

fn run(
    samples: Vec<f32>,
    fs: f64,
    name: String,
    tx: Publisher,
    control: Arc<Control>,
    sink: Arc<AudioSink>,
) {
    tx.log(Direction::Note, format!("loaded {name} ({:.1}s at {fs:.0} Hz)", samples.len() as f64 / fs));
    tx.log(Direction::Note, "Bell 103: originate 1070/1270, answer 2025/2225");

    // The originating modem hears the answering modem, and vice versa. Running
    // both gives each direction of a 2-wire capture.
    let mut host = Bell103Rx::with_tones(2025.0, 2225.0, fs); // answer band
    let mut caller = Bell103Rx::with_tones(1070.0, 1270.0, fs); // originate band
    let mut host_line = LineAssembler::new(Direction::FromLine);
    let mut caller_line = LineAssembler::new(Direction::ToLine);

    let mut spectrum = Spectrum::new(FFT_SIZE, fs);
    let mut waveform = Ring::new(SCOPE_LEN);
    let mut baseband = Ring::new(SCOPE_LEN);
    let mut bins = vec![0.0f64; SPECTRUM_BINS];
    let mut symbols: VecDeque<f32> = VecDeque::with_capacity(SYMBOL_HISTORY);
    // Batched once per tick rather than per sample, to keep the monitor off the
    // hot loop.
    let mut monitor_block: Vec<f32> = Vec::with_capacity(4096);
    // Raw bytes for the terminal, kept separate from the rendered transcript.
    let mut rx_block: Vec<u8> = Vec::with_capacity(256);

    let mut pos = 0usize;
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;
    let mut connected_since: Option<Instant> = None;

    // Publish at about 60 Hz; process in matching chunks.
    let publish_every = Duration::from_millis(16);
    let mut next_publish = Instant::now();
    let mut clock = Instant::now();
    let mut carry = 0.0f64;

    while !control.quit.load(Ordering::Relaxed) {
        if control.restart.swap(false, Ordering::Relaxed) {
            pos = 0;
            rx_bytes = 0;
            tx_bytes = 0;
            connected_since = None;
            symbols.clear();
            host = Bell103Rx::with_tones(2025.0, 2225.0, fs);
            caller = Bell103Rx::with_tones(1070.0, 1270.0, fs);
            tx.log(Direction::Note, "restarted");
            clock = Instant::now();
            carry = 0.0;
        }

        if !control.running.load(Ordering::Relaxed) {
            clock = Instant::now();
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        // Work out how many samples real time has earned us since last round.
        let speed = control.speed_pct.load(Ordering::Relaxed).max(1) as f64 / 100.0;
        let elapsed = clock.elapsed().as_secs_f64();
        clock = Instant::now();
        let want = elapsed * fs * speed + carry;
        let count = want.floor();
        carry = want - count;
        // Cap so a stall does not turn into a burst that outruns the display.
        let count = (count as usize).min((fs * 0.25) as usize);

        monitor_block.clear();
        rx_block.clear();
        for _ in 0..count {
            if pos >= samples.len() {
                break;
            }
            let x = samples[pos] as f64;
            monitor_block.push(samples[pos]);
            pos += 1;

            if let Some(b) = host.feed(x) {
                rx_bytes += 1;
                host_line.push(b, &tx);
                rx_block.push(b);
            }
            // One entry per recovered bit, taken at the bit centre: the slicer
            // margin the symbol scope plots.
            if let Some(sym) = host.take_symbol() {
                if symbols.len() == SYMBOL_HISTORY {
                    symbols.pop_front();
                }
                symbols.push_back(sym as f32);
            }
            if let Some(b) = caller.feed(x) {
                tx_bytes += 1;
                caller_line.push(b, &tx);
            }

            spectrum.push(x);
            waveform.push(x as f32);
            // Show whichever band currently has a carrier; the host side is the
            // more interesting one for this capture.
            baseband.push(host.level() as f32);
        }

        // Feed the monitor exactly what the demodulator saw, so what you hear
        // is the signal being decoded rather than a separate playback path.
        sink.push(&monitor_block);
        tx.line_data(&rx_block);

        if pos >= samples.len() {
            host_line.flush(&tx);
            caller_line.flush(&tx);
            if control.running.swap(false, Ordering::Relaxed) {
                tx.log(Direction::Note, "end of capture");
            }
        }

        let carrier = host.carrier() || caller.carrier();
        if carrier && connected_since.is_none() {
            connected_since = Some(Instant::now());
        }

        if Instant::now() >= next_publish {
            next_publish = Instant::now() + publish_every;
            if spectrum.ready() {
                spectrum.magnitudes_db(&mut bins);
            }
            let level = host.amplitude().max(caller.amplitude());
            let level_db = 20.0 * (level + 1e-9).log10();

            tx.publish(|f| {
                f.sample_rate = fs;
                waveform.copy_into(&mut f.waveform);
                baseband.copy_into(&mut f.baseband);
                for (slot, &v) in f.spectrum_db.iter_mut().zip(bins.iter()) {
                    *slot = v as f32;
                }
                f.hz_per_bin = fs / FFT_SIZE as f64;
                f.rx_level_db = level_db as f32;
                f.carrier = carrier;
                f.state = if pos >= samples.len() {
                    CallState::Idle
                } else if carrier {
                    CallState::Connected
                } else {
                    CallState::Negotiating
                };
                f.modulation = "Bell 103";
                f.bit_rate = carrier.then_some(300);
                f.rx_bytes = rx_bytes;
                f.tx_bytes = tx_bytes;
                f.tones = 2; // Bell 103 is binary FSK: one axis, two arms.
                f.symbols.clear();
                f.symbols.extend(symbols.iter().copied());
                f.leds = Leds {
                    mr: true,
                    tr: true,
                    sd: caller.carrier(),
                    rd: host.carrier(),
                    cd: carrier,
                    oh: pos < samples.len(),
                    aa: false,
                    // Bell 103 is 300 bps, so HS never lights: it means 9600+.
                    hs: false,
                    ec: false,
                };
            });
        }

        thread::sleep(Duration::from_millis(4));
    }
}
