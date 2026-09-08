//! Carries live modem state from the signal path to a user interface.
//!
//! The producer will eventually be an audio callback, so publishing must never
//! block and must never allocate. Two channels with different characters:
//!
//! - [`Frame`] is *latest-wins*. The UI redraws at 60 Hz and only ever wants the
//!   newest scope data, so a frame the UI is too busy to collect is dropped
//!   rather than queued. Publishing uses `try_lock` and skips on contention.
//! - [`LogEntry`] is a bounded queue. Transcript lines must not be dropped just
//!   because the UI was mid-redraw, but they come from the control thread rather
//!   than the audio callback, so an ordinary lock is safe there.
//!
//! No `unsafe`: the workspace denies it. A `Mutex` whose critical section is a
//! memcpy of a few kilobytes, contended at 60 Hz by one reader, costs nothing
//! measurable next to the DSP it is reporting on.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Front-panel lamps, in the order a real modem's faceplate carried them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Leds {
    /// Modem Ready — powered and not in a fault state.
    pub mr: bool,
    /// Terminal Ready — circuit 108, the DTE has raised DTR.
    pub tr: bool,
    /// Send Data — circuit 103 activity.
    pub sd: bool,
    /// Receive Data — circuit 104 activity.
    pub rd: bool,
    /// Carrier Detect — circuit 109, a far-end carrier is present.
    pub cd: bool,
    /// Off Hook — the DAA has seized the line.
    pub oh: bool,
    /// Auto Answer — S0 is non-zero.
    pub aa: bool,
    /// High Speed — connected at 9600 bps or above.
    pub hs: bool,
    /// Error Control — V.42 or MNP is active.
    pub ec: bool,
}

/// Where the modem is in a call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CallState {
    #[default]
    Idle,
    OffHook,
    Dialling,
    Ringing,
    Negotiating,
    Training,
    Connected,
    Disconnecting,
}

impl CallState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::OffHook => "off hook",
            Self::Dialling => "dialling",
            Self::Ringing => "ringing",
            Self::Negotiating => "negotiating",
            Self::Training => "training",
            Self::Connected => "connected",
            Self::Disconnecting => "disconnecting",
        }
    }
}

/// One snapshot of the signal path, sized once and then written in place.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Increments on every publish; lets the UI tell a stale frame from a new one.
    pub seq: u64,
    pub sample_rate: f64,
    /// Recent line samples, oldest first.
    pub waveform: Vec<f32>,
    /// Recent demodulator output. For FSK this is the frequency discriminator,
    /// where +1 is a mark and -1 a space; for QAM it is unused.
    pub baseband: Vec<f32>,
    /// Received constellation points as (I, Q). Empty for FSK, which has none.
    pub constellation: Vec<(f32, f32)>,
    /// Recent slicer decisions, one per recovered bit, newest last.
    ///
    /// For FSK this is the discriminator level at each bit centre: `+1` and
    /// `-1` are the ideal tones and `0` is the decision threshold, so distance
    /// from zero is the margin the slicer had. This is the FSK counterpart of a
    /// constellation and is what the symbol scope plots.
    pub symbols: Vec<f32>,
    /// How many tones or points the modulation uses, which sets how many arms
    /// the symbol scope draws: 2 for Bell 103, 16 for V.22bis.
    pub tones: usize,
    /// The largest coordinate any point of this constellation can reach, in
    /// the units the constellation points are reported in.
    ///
    /// One means it fits the scope's box exactly, which every constellation
    /// did until V.32's trellis code: its points are normalised by a
    /// root-mean-square of sqrt(10) and eight of the thirty-two have a
    /// coordinate of four, so they land a quarter of the way outside the box
    /// and get drawn off the edge of it. Twenty-four dots for a thirty-two
    /// point constellation, and nothing on the screen to say why.
    pub constellation_peak: f32,
    /// What to call the modulation on the symbol scope, such as "2FSK" or
    /// "16QAM". The scope cannot infer it: a constellation could be phase or
    /// quadrature amplitude modulation.
    pub symbol_label: &'static str,
    /// Magnitude spectrum in dBFS, DC to Nyquist.
    pub spectrum_db: Vec<f32>,
    pub hz_per_bin: f64,
    /// Received level in dBFS.
    pub rx_level_db: f32,
    /// Estimated signal-to-noise ratio in dB, if the datapump can measure one.
    pub snr_db: Option<f32>,
    pub carrier: bool,
    pub leds: Leds,
    pub state: CallState,
    /// Modulation currently in use, e.g. "Bell 103".
    pub modulation: &'static str,
    /// Where inside that modulation's own procedure the call has got to.
    ///
    /// The most informative thing on a window during a start-up and the
    /// hardest to get any other way: a call that is not coming up is always
    /// stuck somewhere particular, and "negotiating" for twenty seconds says
    /// nothing about which somewhere.
    pub line_phase: &'static str,
    /// What the far end has said about itself, as label and value.
    ///
    /// Owned strings rather than borrowed, because most of these are numbers
    /// the far end chose and none of them are known at compile time.
    pub distant: Vec<(&'static str, String)>,
    /// Negotiated line rate once connected.
    pub bit_rate: Option<u32>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// How much of this end's own transmission the echo canceller is taking
    /// out, in decibels, and where on the line it found it to take out.
    ///
    /// On a two-wire pair this is the difference between hearing the far end
    /// and hearing yourself, and there is no way to tell from the constellation
    /// alone which of the two is being looked at. A number for it is the
    /// difference between suspecting the canceller and knowing.
    pub echo_loss_db: Option<f64>,
    /// Where the reflection was found, in samples, and how strong it was.
    pub echo_at: Option<(usize, f64)>,
}

impl Frame {
    /// Allocate a frame with fixed capacities. Publishing never resizes these.
    pub fn new(scope_len: usize, spectrum_bins: usize, sample_rate: f64) -> Self {
        Self {
            seq: 0,
            sample_rate,
            waveform: vec![0.0; scope_len],
            baseband: vec![0.0; scope_len],
            constellation: Vec::with_capacity(256),
            symbols: Vec::with_capacity(256),
            // Ten rows is more than the far end has ever had to say, and
            // publishing never resizes what it was given.
            distant: Vec::with_capacity(10),
            tones: 2,
            constellation_peak: 1.0,
            symbol_label: "-",
            spectrum_db: vec![-120.0; spectrum_bins],
            hz_per_bin: sample_rate / (spectrum_bins as f64 * 2.0),
            rx_level_db: -120.0,
            snr_db: None,
            carrier: false,
            leds: Leds::default(),
            state: CallState::Idle,
            modulation: "-",
            line_phase: "-",
            bit_rate: None,
            rx_bytes: 0,
            echo_loss_db: None,
            echo_at: None,
            tx_bytes: 0,
        }
    }

    /// Frequency at the centre of spectrum bin `k`.
    pub fn bin_frequency(&self, k: usize) -> f64 {
        k as f64 * self.hz_per_bin
    }

    /// Symbol quality on a 0-100 scale, in the manner ARDOP reports it.
    ///
    /// The mean absolute slicer margin: 100 means every symbol landed on an
    /// ideal tone, 0 means every symbol landed on the decision threshold and
    /// the decisions were coin flips. Returns `None` with no symbols to judge.
    pub fn symbol_quality(&self) -> Option<u32> {
        if self.symbols.is_empty() {
            return None;
        }
        let mean = self.symbols.iter().map(|v| v.abs().min(1.0)).sum::<f32>()
            / self.symbols.len() as f32;
        Some((mean * 100.0).round() as u32)
    }
}

/// Which way a transcript line was travelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// DTE to DCE: a command typed at the modem.
    ToDce,
    /// DCE to DTE: a response or echo.
    ToDte,
    /// Received from the far end over the line.
    FromLine,
    /// Sent to the far end over the line.
    ToLine,
    /// An internal note: state change, negotiation step, error.
    Note,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: Duration,
    pub direction: Direction,
    pub text: String,
    /// Monotonic identity, so a reader can ask for what it has not seen without
    /// counting entries. Counting breaks once the bounded log starts evicting.
    pub seq: u64,
    /// False while characters may still be appended to this line.
    ///
    /// A 300 bps line delivers about thirty characters a second, and waiting
    /// for a terminator before showing anything makes the display feel dead.
    /// An incomplete entry is shown as it grows.
    pub complete: bool,
}

#[derive(Debug)]
struct Shared {
    frame: Mutex<Frame>,
    log: Mutex<VecDeque<LogEntry>>,
    log_capacity: usize,
    /// Bytes recovered from the far end, for a terminal to render.
    ///
    /// Separate from the transcript because a terminal needs the raw stream,
    /// escape sequences intact, not lines rendered for human reading.
    rx_data: Mutex<VecDeque<u8>>,
    rx_capacity: usize,
    seq: AtomicU64,
    log_seq: AtomicU64,
    /// Counts frames dropped because the reader held the lock.
    dropped: AtomicU64,
    started: Instant,
}

/// Writes telemetry. Held by the modem.
#[derive(Debug, Clone)]
pub struct Publisher {
    shared: Arc<Shared>,
}

/// Reads telemetry. Held by the user interface.
#[derive(Debug, Clone)]
pub struct Subscriber {
    shared: Arc<Shared>,
}

/// Create a connected publisher and subscriber.
pub fn channel(scope_len: usize, spectrum_bins: usize, sample_rate: f64) -> (Publisher, Subscriber) {
    let shared = Arc::new(Shared {
        frame: Mutex::new(Frame::new(scope_len, spectrum_bins, sample_rate)),
        log: Mutex::new(VecDeque::new()),
        log_capacity: 2000,
        rx_data: Mutex::new(VecDeque::new()),
        // A screenful many times over: enough that a UI stall cannot lose BBS
        // output, small enough that a runaway sender cannot grow without bound.
        rx_capacity: 64 * 1024,
        seq: AtomicU64::new(0),
        log_seq: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        started: Instant::now(),
    });
    (Publisher { shared: shared.clone() }, Subscriber { shared })
}

impl Publisher {
    /// Update the frame in place.
    ///
    /// Returns false if the reader held the lock and the update was skipped.
    /// Dropping a frame is the correct outcome: the next one supersedes it.
    pub fn publish<F>(&self, fill: F) -> bool
    where
        F: FnOnce(&mut Frame),
    {
        match self.shared.frame.try_lock() {
            Ok(mut frame) => {
                fill(&mut frame);
                frame.seq = self.shared.seq.fetch_add(1, Ordering::Relaxed) + 1;
                true
            }
            Err(_) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Append a complete transcript line. Called from the control thread, not
    /// the audio callback, so blocking briefly here is acceptable.
    pub fn log(&self, direction: Direction, text: impl Into<String>) {
        self.push_entry(direction, text.into(), true);
    }

    /// Append characters to the line in progress, starting one if there is none.
    ///
    /// Lets a reader watch a line build up rather than waiting for its
    /// terminator. A new line is started when the previous entry came from a
    /// different direction, so the two sides of a conversation stay separate.
    pub fn log_partial(&self, direction: Direction, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Ok(mut log) = self.shared.log.lock()
            && let Some(last) = log.back_mut()
            && !last.complete
            && last.direction == direction
        {
            last.text.push_str(text);
            return;
        }
        self.push_entry(direction, text.to_string(), false);
    }

    /// Mark the line in progress finished, so the next character starts a new one.
    pub fn log_end(&self, direction: Direction) {
        if let Ok(mut log) = self.shared.log.lock()
            && let Some(last) = log.back_mut()
            && !last.complete
            && last.direction == direction
        {
            last.complete = true;
        }
    }

    fn push_entry(&self, direction: Direction, text: String, complete: bool) {
        let entry = LogEntry {
            at: self.shared.started.elapsed(),
            direction,
            text,
            seq: self.shared.log_seq.fetch_add(1, Ordering::Relaxed) + 1,
            complete,
        };
        if let Ok(mut log) = self.shared.log.lock() {
            if log.len() == self.shared.log_capacity {
                log.pop_front();
            }
            log.push_back(entry);
        }
    }

    /// Hand bytes recovered from the far end to whatever is rendering them.
    ///
    /// Oldest are dropped if the reader falls far enough behind to fill the
    /// buffer, which loses screen content rather than stalling the receiver.
    pub fn line_data(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Ok(mut q) = self.shared.rx_data.lock() {
            for &b in bytes {
                if q.len() >= self.shared.rx_capacity {
                    q.pop_front();
                }
                q.push_back(b);
            }
        }
    }

    /// Log bytes, rendering control characters readably.
    pub fn log_bytes(&self, direction: Direction, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.log(direction, render_bytes(bytes));
    }
}

impl Subscriber {
    /// Copy the latest frame into `into`, resizing it to match.
    ///
    /// Returns false if the publisher held the lock; the caller should keep
    /// showing the frame it already has.
    pub fn read(&self, into: &mut Frame) -> bool {
        match self.shared.frame.try_lock() {
            Ok(frame) => {
                into.clone_from(&frame);
                true
            }
            Err(_) => false,
        }
    }

    /// Snapshot of the transcript.
    pub fn log(&self) -> Vec<LogEntry> {
        self.shared
            .log
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Transcript entries newer than `seq`.
    ///
    /// Identified by sequence rather than by count, because the log is bounded
    /// and a count becomes wrong as soon as eviction starts.
    pub fn log_after(&self, seq: u64) -> Vec<LogEntry> {
        self.shared
            .log
            .lock()
            .map(|l| l.iter().filter(|e| e.seq > seq).cloned().collect())
            .unwrap_or_default()
    }

    /// Take everything received from the far end since the last call.
    pub fn take_line_data(&self) -> Vec<u8> {
        self.shared
            .rx_data
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn log_len(&self) -> usize {
        self.shared.log.lock().map(|l| l.len()).unwrap_or(0)
    }

    /// How many frames were dropped through lock contention. A steadily rising
    /// count means the UI is too slow, not that anything is wrong with the modem.
    pub fn dropped_frames(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    pub fn sequence(&self) -> u64 {
        self.shared.seq.load(Ordering::Relaxed)
    }
}

/// Render bytes for a transcript, showing control characters symbolically.
pub fn render_bytes(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\r' => s.push_str("<CR>"),
            b'\n' => s.push_str("<LF>"),
            0x08 => s.push_str("<BS>"),
            0x09 => s.push_str("<TAB>"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("<{b:02X}>")),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_frame_reaches_the_subscriber() {
        let (tx, rx) = channel(64, 32, 16000.0);
        assert!(tx.publish(|f| {
            f.carrier = true;
            f.modulation = "Bell 103";
            f.waveform[0] = 0.5;
        }));
        let mut frame = Frame::new(0, 0, 0.0);
        assert!(rx.read(&mut frame));
        assert!(frame.carrier);
        assert_eq!(frame.modulation, "Bell 103");
        assert_eq!(frame.waveform[0], 0.5);
        assert_eq!(frame.seq, 1);
    }

    #[test]
    fn sequence_increases_with_each_publish() {
        let (tx, rx) = channel(8, 4, 8000.0);
        for _ in 0..5 {
            tx.publish(|_| {});
        }
        assert_eq!(rx.sequence(), 5);
    }

    #[test]
    fn publishing_never_blocks_when_the_reader_holds_the_lock() {
        let (tx, rx) = channel(8, 4, 8000.0);
        // Simulate the reader being mid-copy by taking the lock ourselves.
        let held = rx.shared.frame.lock().unwrap();
        assert!(!tx.publish(|f| f.carrier = true), "publish should have skipped");
        assert_eq!(rx.dropped_frames(), 1);
        drop(held);
        assert!(tx.publish(|f| f.carrier = true), "publish should resume");
    }

    #[test]
    fn latest_wins_rather_than_queueing() {
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.publish(|f| f.rx_level_db = -30.0);
        tx.publish(|f| f.rx_level_db = -20.0);
        tx.publish(|f| f.rx_level_db = -10.0);
        let mut frame = Frame::new(0, 0, 0.0);
        rx.read(&mut frame);
        assert_eq!(frame.rx_level_db, -10.0, "should hold only the newest");
    }

    #[test]
    fn the_log_preserves_order_and_direction() {
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.log(Direction::ToDce, "ATDT5551234");
        tx.log(Direction::ToDte, "CONNECT 300");
        let log = rx.log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].direction, Direction::ToDce);
        assert_eq!(log[1].text, "CONNECT 300");
    }

    #[test]
    fn the_log_is_bounded_and_drops_oldest_first() {
        let (tx, rx) = channel(8, 4, 8000.0);
        for i in 0..2500 {
            tx.log(Direction::Note, format!("line {i}"));
        }
        let log = rx.log();
        assert_eq!(log.len(), 2000);
        assert_eq!(log[0].text, "line 500", "oldest entries should have gone");
        assert_eq!(log[1999].text, "line 2499");
    }

    #[test]
    fn log_after_returns_only_newer_entries() {
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.log(Direction::Note, "a");
        let seen = rx.log().last().unwrap().seq;
        tx.log(Direction::Note, "b");
        let new = rx.log_after(seen);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].text, "b");
    }

    #[test]
    fn a_partial_line_grows_in_place() {
        let (tx, rx) = channel(8, 4, 8000.0);
        for c in ["W", "e", "l", "c", "o", "m", "e"] {
            tx.log_partial(Direction::FromLine, c);
        }
        let log = rx.log();
        assert_eq!(log.len(), 1, "each character should not make its own entry");
        assert_eq!(log[0].text, "Welcome");
        assert!(!log[0].complete, "still growing");

        tx.log_end(Direction::FromLine);
        assert!(rx.log()[0].complete);
        tx.log_partial(Direction::FromLine, "next");
        assert_eq!(rx.log().len(), 2, "a finished line should not be extended");
    }

    #[test]
    fn the_other_direction_starts_its_own_line() {
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.log_partial(Direction::FromLine, "host");
        tx.log_partial(Direction::ToLine, "user");
        tx.log_partial(Direction::FromLine, "more");
        let log = rx.log();
        assert_eq!(log.len(), 3, "interleaved directions must not merge");
        assert_eq!(log[0].text, "host");
        assert_eq!(log[2].text, "more");
    }

    #[test]
    fn sequence_numbers_survive_eviction() {
        // The log is bounded, so counting entries goes wrong once it wraps;
        // sequence numbers do not.
        let (tx, rx) = channel(8, 4, 8000.0);
        for i in 0..2500 {
            tx.log(Direction::Note, format!("{i}"));
        }
        let log = rx.log();
        assert_eq!(log.len(), 2000);
        assert_eq!(log.last().unwrap().seq, 2500);
        assert_eq!(rx.log_after(2499).len(), 1);
    }

    #[test]
    fn line_data_round_trips_in_order() {
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.line_data(b"Welcome");
        tx.line_data(b" to the BBS");
        assert_eq!(rx.take_line_data(), b"Welcome to the BBS");
        assert!(rx.take_line_data().is_empty(), "draining should empty it");
    }

    #[test]
    fn line_data_preserves_escape_sequences_verbatim() {
        // A terminal needs the raw stream; the transcript is the rendered view.
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.line_data(b"[2J[1;1HX");
        assert_eq!(rx.take_line_data(), b"[2J[1;1HX");
    }

    #[test]
    fn control_characters_render_readably() {
        assert_eq!(render_bytes(b"AT\r\n"), "AT<CR><LF>");
        assert_eq!(render_bytes(&[0x01, b'X']), "<01>X");
        assert_eq!(render_bytes(b""), "");
    }

    #[test]
    fn empty_byte_logs_are_skipped() {
        let (tx, rx) = channel(8, 4, 8000.0);
        tx.log_bytes(Direction::ToDte, b"");
        assert_eq!(rx.log_len(), 0);
    }

    #[test]
    fn bin_frequency_spans_dc_to_nyquist() {
        let f = Frame::new(16, 512, 16000.0);
        assert_eq!(f.bin_frequency(0), 0.0);
        // The last bin sits just below Nyquist.
        let top = f.bin_frequency(511);
        assert!(top > 7900.0 && top < 8000.0, "top bin {top} Hz");
    }

    #[test]
    fn telemetry_crosses_threads() {
        let (tx, rx) = channel(32, 16, 8000.0);
        let producer = std::thread::spawn(move || {
            for i in 0..1000 {
                tx.publish(|f| f.rx_bytes = i);
                tx.log(Direction::Note, format!("{i}"));
            }
            // Publishing may be skipped when the reader holds the lock, and
            // that is the intended behaviour because the next frame
            // supersedes the one lost. There is no next frame after the last,
            // so this one has to be insisted on; without that the test asks
            // for a guarantee the channel does not make and fails whenever
            // the final write happens to collide with a read.
            while !tx.publish(|f| f.rx_bytes = 999) {}
        });
        let mut frame = Frame::new(0, 0, 0.0);
        for _ in 0..1000 {
            rx.read(&mut frame);
        }
        producer.join().unwrap();
        rx.read(&mut frame);
        assert_eq!(frame.rx_bytes, 999);
        assert_eq!(rx.log_len(), 1000);
    }
}

#[cfg(test)]
mod constellation_tests {
    use super::*;

    /// The scope drew an empty constellation while the engine was producing
    /// points, so pin the hop between them.
    #[test]
    fn constellation_points_survive_the_channel() {
        let (tx, rx) = channel(64, 32, 16000.0);
        let sent: Vec<(f32, f32)> = (0..80)
            .map(|i| (i as f32 * 0.01 - 0.4, 0.3 - i as f32 * 0.005))
            .collect();
        tx.publish(|f| {
            f.tones = 16;
            f.symbol_label = "16QAM";
            f.constellation.clear();
            f.constellation.extend(sent.iter().copied());
        });

        // A reader starts from a frame sized for a different modulation, which
        // is exactly what the scope does.
        let mut frame = Frame::new(64, 32, 16000.0);
        assert!(frame.constellation.is_empty());
        assert!(rx.read(&mut frame));
        assert_eq!(frame.tones, 16);
        assert_eq!(frame.symbol_label, "16QAM");
        assert_eq!(frame.constellation, sent, "points did not cross the channel");
    }
}
