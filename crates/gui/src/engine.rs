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
use datapump::v22bis::{Channel, Receiver as V22bisRx};
use line::AudioSink;
use dsp::Spectrum;
use telemetry::{CallState, Direction, Leds, Publisher};

pub const SCOPE_LEN: usize = 1024;
pub const FFT_SIZE: usize = 1024;
pub const SPECTRUM_BINS: usize = FFT_SIZE / 2;
/// Symbols kept for the scope: roughly eight characters at 8N1, enough to see
/// the cluster spread without smearing the display with ancient history.
pub const SYMBOL_HISTORY: usize = 80;

/// Which standard a capture holds, and whether we can yet demodulate it.
///
/// Running the Bell 103 receiver against a V.22bis capture produces confident
/// nonsense: a plausible byte count, a plausible quality figure, and none of it
/// real. Naming the standard up front means the display can say what it is
/// actually doing rather than reporting noise as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    Bell103,
    V22bis,
    V32bis,
    V34,
    V90,
    V92,
    Unknown,
}

impl Standard {
    /// Identify from the vector's file name.
    pub fn from_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.contains("bell103") {
            Self::Bell103
        } else if n.contains("v22bis") {
            Self::V22bis
        } else if n.contains("v32bis") {
            Self::V32bis
        } else if n.contains("v34") {
            Self::V34
        } else if n.contains("v90") {
            Self::V90
        } else if n.contains("v92") {
            Self::V92
        } else {
            Self::Unknown
        }
    }

    /// True only where a receiver actually exists.
    pub fn has_receiver(self) -> bool {
        matches!(self, Self::Bell103 | Self::V22bis)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bell103 => "Bell 103",
            Self::V22bis => "V.22bis",
            Self::V32bis => "V.32bis (no receiver)",
            Self::V34 => "V.34 (no receiver)",
            Self::V90 => "V.90 (no receiver)",
            Self::V92 => "V.92 (no receiver)",
            Self::Unknown => "unknown",
        }
    }

    pub fn bit_rate(self) -> Option<u32> {
        match self {
            Self::Bell103 => Some(300),
            Self::V22bis => Some(2400),
            Self::V32bis => Some(14400),
            Self::V34 => Some(33600),
            Self::V90 | Self::V92 => Some(56000),
            Self::Unknown => None,
        }
    }
}

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

/// The pair of receivers for whichever modulation a capture holds.
///
/// A two-wire tap carries both directions at once, so each modulation needs one
/// receiver per direction. Which band belongs to which end differs: Bell 103
/// splits by tone pair and V.22bis by carrier, with the answering modem always
/// on the higher of the two.
enum Demod {
    // Both boxed. A receiver carries filter state by the hundred taps, and the
    // two modulations differ enough in size that an unboxed enum would be as
    // large as its biggest arm whichever one is in use.
    Bell103 { host: Box<Bell103Rx>, caller: Box<Bell103Rx> },
    V22bis { host: Box<V22bisRx>, caller: Box<V22bisRx> },
}

impl Demod {
    fn new(standard: Standard, fs: f64) -> Option<Self> {
        match standard {
            Standard::Bell103 => Some(Self::Bell103 {
                host: Box::new(Bell103Rx::with_tones(2025.0, 2225.0, fs)),
                caller: Box::new(Bell103Rx::with_tones(1070.0, 1270.0, fs)),
            }),
            Standard::V22bis => Some(Self::V22bis {
                // The answering modem transmits the high channel, so listening
                // to it means presenting as the calling modem.
                host: Box::new(V22bisRx::new(Channel::Calling, fs)),
                caller: Box::new(V22bisRx::new(Channel::Answering, fs)),
            }),
            _ => None,
        }
    }

    fn feed(&mut self, x: f64, from_host: &mut Vec<u8>, from_caller: &mut Vec<u8>) {
        match self {
            Self::Bell103 { host, caller } => {
                if let Some(b) = host.feed(x) {
                    from_host.push(b);
                }
                if let Some(b) = caller.feed(x) {
                    from_caller.push(b);
                }
            }
            Self::V22bis { host, caller } => {
                host.feed(x);
                caller.feed(x);
                from_host.extend(host.take_bytes());
                from_caller.extend(caller.take_bytes());
            }
        }
    }

    /// One slicer margin per recovered bit, for the frequency-shift scope.
    fn take_symbol(&mut self) -> Option<f32> {
        match self {
            Self::Bell103 { host, .. } => host.take_symbol().map(|v| v as f32),
            Self::V22bis { .. } => None,
        }
    }

    /// The latest constellation point, for the quadrature scope.
    fn constellation(&self) -> Option<(f32, f32)> {
        match self {
            Self::Bell103 { .. } => None,
            Self::V22bis { host, .. } => {
                let (i, q) = host.constellation_point();
                Some((i as f32, q as f32))
            }
        }
    }

    /// Trace for the baseband scope.
    fn baseband(&self) -> f32 {
        match self {
            Self::Bell103 { host, .. } => host.level() as f32,
            Self::V22bis { host, .. } => host.constellation_point().0 as f32,
        }
    }

    /// Carrier present, as (host, caller).
    fn carriers(&self) -> (bool, bool) {
        match self {
            Self::Bell103 { host, caller } => (host.carrier(), caller.carrier()),
            Self::V22bis { host, caller } => {
                // No explicit carrier detector yet, so a settled equaliser
                // stands in as evidence of a signal worth believing.
                (!host.equalizer_blind(), !caller.equalizer_blind())
            }
        }
    }

    fn level(&self) -> f64 {
        match self {
            Self::Bell103 { host, caller } => host.amplitude().max(caller.amplitude()),
            Self::V22bis { host, caller } => host.level().max(caller.level()),
        }
    }

    /// Mean distance from decisions, where the receiver can measure it.
    fn residual(&self) -> Option<f32> {
        match self {
            Self::Bell103 { .. } => None,
            Self::V22bis { host, .. } => Some(host.residual_error() as f32),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Bell103 { .. } => "2FSK",
            Self::V22bis { .. } => "16QAM",
        }
    }

    fn tones(&self) -> usize {
        match self {
            Self::Bell103 { .. } => 2,
            Self::V22bis { .. } => 16,
        }
    }

    /// The rate actually in use, which for V.22bis the receiver works out from
    /// the constellation rather than being told.
    fn bit_rate(&self) -> u32 {
        match self {
            Self::Bell103 { .. } => 300,
            Self::V22bis { host, .. } => host.rate().bits_per_second(),
        }
    }
}

/// Groups received bytes into transcript lines.
///
/// Characters are published the instant they are decoded rather than held back
/// until a terminator arrives. At 300 bps a line takes seconds to come in, and
/// waiting for its end makes a live session look frozen.
struct LineAssembler {
    direction: Direction,
    width: usize,
}

impl LineAssembler {
    fn new(direction: Direction) -> Self {
        Self { direction, width: 0 }
    }

    fn push(&mut self, byte: u8, tx: &Publisher) {
        // Either terminator ends the line, and the paired one is swallowed so
        // CRLF does not produce an empty second line.
        if byte == b'\r' || byte == b'\n' {
            self.end(tx);
            return;
        }
        tx.log_partial(self.direction, &telemetry::render_bytes(&[byte]));
        self.width += 1;
        if self.width >= 160 {
            self.end(tx);
        }
    }

    fn end(&mut self, tx: &Publisher) {
        if self.width > 0 {
            tx.log_end(self.direction);
            self.width = 0;
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

    let standard = Standard::from_name(&name);
    Ok(thread::spawn(move || {
        run(samples, fs, name, standard, tx, control, sink);
    }))
}

fn run(
    samples: Vec<f32>,
    fs: f64,
    name: String,
    standard: Standard,
    tx: Publisher,
    control: Arc<Control>,
    sink: Arc<AudioSink>,
) {
    tx.log(Direction::Note, format!("loaded {name} ({:.1}s at {fs:.0} Hz)", samples.len() as f64 / fs));
    if standard.has_receiver() {
        tx.log(Direction::Note, "Bell 103: originate 1070/1270, answer 2025/2225");
    } else {
        tx.log(
            Direction::Note,
            format!(
                "{} is not implemented yet - showing waterfall, spectrum and level only",
                standard.label().split(" (").next().unwrap_or("this modulation")
            ),
        );
    }
    let decoding = standard.has_receiver();

    // The originating modem hears the answering modem, and vice versa. Running
    // both gives each direction of a 2-wire capture.
    let mut demod = Demod::new(standard, fs);
    let mut host_line = LineAssembler::new(Direction::FromLine);
    let mut caller_line = LineAssembler::new(Direction::ToLine);

    let mut spectrum = Spectrum::new(FFT_SIZE, fs);
    let mut waveform = Ring::new(SCOPE_LEN);
    let mut baseband = Ring::new(SCOPE_LEN);
    let mut bins = vec![0.0f64; SPECTRUM_BINS];
    let mut symbols: VecDeque<f32> = VecDeque::with_capacity(SYMBOL_HISTORY);
    let mut points: VecDeque<(f32, f32)> = VecDeque::with_capacity(SYMBOL_HISTORY);
    let mut host_bytes: Vec<u8> = Vec::with_capacity(64);
    let mut caller_bytes: Vec<u8> = Vec::with_capacity(64);
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
            points.clear();
            demod = Demod::new(standard, fs);
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

            // Only run the receiver where one exists for this modulation.
            if let Some(d) = demod.as_mut() {
                host_bytes.clear();
                caller_bytes.clear();
                d.feed(x, &mut host_bytes, &mut caller_bytes);
                for &b in &host_bytes {
                    rx_bytes += 1;
                    host_line.push(b, &tx);
                    rx_block.push(b);
                }
                for &b in &caller_bytes {
                    tx_bytes += 1;
                    caller_line.push(b, &tx);
                }
                // One entry per recovered bit, for the frequency-shift scope.
                if let Some(sym) = d.take_symbol() {
                    if symbols.len() == SYMBOL_HISTORY {
                        symbols.pop_front();
                    }
                    symbols.push_back(sym);
                }
                // One point per symbol, for the quadrature scope. Only taken
                // when it changes, so a motionless constellation is not filled
                // with copies of a single point.
                if let Some(p) = d.constellation()
                    && points.back() != Some(&p) {
                        if points.len() == SYMBOL_HISTORY {
                            points.pop_front();
                        }
                        points.push_back(p);
                    }
                baseband.push(d.baseband());
            }

            spectrum.push(x);
            waveform.push(x as f32);
        }

        // Feed the monitor exactly what the demodulator saw, so what you hear
        // is the signal being decoded rather than a separate playback path.
        sink.push(&monitor_block);
        tx.line_data(&rx_block);

        if pos >= samples.len() {
            host_line.end(&tx);
            caller_line.end(&tx);
            if control.running.swap(false, Ordering::Relaxed) {
                tx.log(Direction::Note, "end of capture");
            }
        }

        let (host_carrier, caller_carrier) =
            demod.as_ref().map(Demod::carriers).unwrap_or((false, false));
        let carrier = host_carrier || caller_carrier;
        if carrier && connected_since.is_none() {
            connected_since = Some(Instant::now());
        }

        if Instant::now() >= next_publish {
            next_publish = Instant::now() + publish_every;
            if spectrum.ready() {
                spectrum.magnitudes_db(&mut bins);
            }
            // Without a receiver there is no band filter to take a level from,
            // so measure the raw line instead.
            let level = if let Some(d) = demod.as_ref() {
                d.level()
            } else {
                let n = waveform.data.len();
                (waveform.data.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>()
                    / n as f64)
                    .sqrt()
            };
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
                f.state = if pos >= samples.len() || !decoding {
                    // With no receiver there is nothing to be connected to;
                    // saying "negotiating" would imply progress that is not
                    // happening.
                    CallState::Idle
                } else if carrier {
                    CallState::Connected
                } else {
                    CallState::Negotiating
                };
                f.modulation = standard.label();
                f.bit_rate = if carrier {
                    // Report what the receiver found, not what the
                    // capture's name promised.
                    demod.as_ref().map(Demod::bit_rate).or_else(|| standard.bit_rate())
                } else {
                    None
                };
                f.rx_bytes = rx_bytes;
                f.tx_bytes = tx_bytes;
                f.tones = demod.as_ref().map(Demod::tones).unwrap_or(2);
                f.symbol_label = demod.as_ref().map(Demod::label).unwrap_or("-");
                f.snr_db = demod.as_ref().and_then(Demod::residual).map(|e| {
                    // Report the decision margin as a decibel figure, so a
                    // tighter constellation reads as a larger number.
                    -20.0 * (e.max(1e-3)).log10()
                });
                f.symbols.clear();
                f.symbols.extend(symbols.iter().copied());
                f.constellation.clear();
                f.constellation.extend(points.iter().copied());
                f.leds = Leds {
                    mr: true,
                    tr: true,
                    sd: caller_carrier,
                    rd: host_carrier,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standards_are_identified_from_the_vector_name() {
        for (name, want) in [
            ("bell103-300.wav", Standard::Bell103),
            ("v22bis-2400.wav", Standard::V22bis),
            ("v32bis-14400.wav", Standard::V32bis),
            ("v34-33600.wav", Standard::V34),
            ("v90-56k.wav", Standard::V90),
            ("v92-56k.wav", Standard::V92),
            ("something-else.wav", Standard::Unknown),
        ] {
            assert_eq!(Standard::from_name(name), want, "{name}");
        }
    }

    #[test]
    fn the_bis_variants_are_not_mistaken_for_their_base_standard() {
        // "v32bis" contains neither "v34" nor a bare "v32" test, but the
        // ordering of the checks still has to put the longer name first.
        assert_eq!(Standard::from_name("v32bis-14400.wav"), Standard::V32bis);
        assert_eq!(Standard::from_name("v22bis-2400.wav"), Standard::V22bis);
    }

    #[test]
    fn only_implemented_modulations_claim_a_receiver() {
        assert!(Standard::Bell103.has_receiver());
        assert!(Standard::V22bis.has_receiver());
        for s in [
            Standard::V32bis,
            Standard::V34,
            Standard::V90,
            Standard::V92,
            Standard::Unknown,
        ] {
            assert!(!s.has_receiver(), "{s:?} should not claim a receiver");
        }
    }

    #[test]
    fn labels_say_plainly_when_there_is_no_receiver() {
        // The display must not imply it is demodulating something it cannot,
        // nor disclaim one it can.
        assert_eq!(Standard::Bell103.label(), "Bell 103");
        assert_eq!(Standard::V22bis.label(), "V.22bis");
        for s in [Standard::V32bis, Standard::V34, Standard::V90, Standard::V92] {
            assert!(
                s.label().contains("no receiver"),
                "{s:?} label {:?} does not say so",
                s.label()
            );
        }
        // Every label that disclaims a receiver must match a standard that
        // really has none, and the other way round.
        for s in [
            Standard::Bell103,
            Standard::V22bis,
            Standard::V32bis,
            Standard::V34,
            Standard::V90,
            Standard::V92,
        ] {
            assert_eq!(
                s.has_receiver(),
                !s.label().contains("no receiver"),
                "{s:?} label and capability disagree"
            );
        }
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    #[test]
    fn characters_appear_before_the_line_ends() {
        // The point of the change: at 300 bps a line takes seconds, so it has
        // to be visible while it is still arriving.
        let (tx, rx) = telemetry::channel(8, 4, 16000.0);
        let mut line = LineAssembler::new(Direction::FromLine);
        for b in b"Welcome" {
            line.push(*b, &tx);
        }
        let log = rx.log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].text, "Welcome");
        assert!(!log[0].complete, "line is still arriving");
    }

    #[test]
    fn a_terminator_completes_the_line_without_starting_an_empty_one() {
        let (tx, rx) = telemetry::channel(8, 4, 16000.0);
        let mut line = LineAssembler::new(Direction::FromLine);
        for b in b"login:\r\n" {
            line.push(*b, &tx);
        }
        let log = rx.log();
        assert_eq!(log.len(), 1, "CRLF should not produce a second, empty line");
        assert_eq!(log[0].text, "login:");
        assert!(log[0].complete);
    }

    #[test]
    fn successive_lines_are_separate_entries() {
        let (tx, rx) = telemetry::channel(8, 4, 16000.0);
        let mut line = LineAssembler::new(Direction::FromLine);
        for b in b"one\r\ntwo\r\n" {
            line.push(*b, &tx);
        }
        let log = rx.log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].text, "one");
        assert_eq!(log[1].text, "two");
    }

    #[test]
    fn an_over_long_line_is_broken_rather_than_growing_without_limit() {
        let (tx, rx) = telemetry::channel(8, 4, 16000.0);
        let mut line = LineAssembler::new(Direction::FromLine);
        for _ in 0..400 {
            line.push(b'x', &tx);
        }
        assert!(rx.log().len() >= 2, "a runaway line should be wrapped");
    }
}

#[cfg(test)]
mod demod_tests {
    use super::*;

    /// The scope showed an empty constellation while bytes were flowing, so
    /// pin the path that feeds it.
    #[test]
    fn v22bis_produces_moving_constellation_points() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v22bis-2400.wav");
        let wav = line::wav::read(path).expect("vector");
        let fs = wav.sample_rate as f64;
        let mono = wav.mono();
        let mut d = Demod::new(Standard::V22bis, fs).expect("V.22bis has a receiver");

        let mut host_bytes = Vec::new();
        let mut caller_bytes = Vec::new();
        let mut points: Vec<(f32, f32)> = Vec::new();
        // Well into the data, as the engine would be by then.
        for &s in mono.iter().skip((6.0 * fs) as usize).take((4.0 * fs) as usize) {
            host_bytes.clear();
            caller_bytes.clear();
            d.feed(s as f64, &mut host_bytes, &mut caller_bytes);
            if let Some(p) = d.constellation()
                && points.last() != Some(&p)
            {
                points.push(p);
            }
        }
        assert!(!points.is_empty(), "no constellation points at all");
        assert!(
            points.len() > 1000,
            "only {} points from four seconds of 600 baud",
            points.len()
        );
        let spread = points
            .iter()
            .map(|p| (p.0 * p.0 + p.1 * p.1).sqrt())
            .fold(0.0f32, f32::max);
        assert!(spread > 0.1, "points are all at the origin: largest {spread}");
    }

    /// The scope starts at the beginning of the capture, not part way in: the
    /// answer tone and the near-silence before it are part of what the receiver
    /// has to survive.
    #[test]
    fn v22bis_points_stay_finite_from_the_start_of_a_capture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v22bis-2400.wav");
        let wav = line::wav::read(path).expect("vector");
        let fs = wav.sample_rate as f64;
        let mono = wav.mono();
        let mut d = Demod::new(Standard::V22bis, fs).expect("V.22bis has a receiver");

        let mut host_bytes = Vec::new();
        let mut caller_bytes = Vec::new();
        let mut first_bad: Option<usize> = None;
        for (n, &s) in mono.iter().enumerate() {
            host_bytes.clear();
            caller_bytes.clear();
            d.feed(s as f64, &mut host_bytes, &mut caller_bytes);
            if let Some(p) = d.constellation()
                && (!p.0.is_finite() || !p.1.is_finite())
                && first_bad.is_none()
            {
                first_bad = Some(n);
            }
        }
        assert!(
            first_bad.is_none(),
            "constellation went non-finite at sample {} ({:.2}s in)",
            first_bad.unwrap(),
            first_bad.unwrap() as f64 / fs
        );
    }
}
