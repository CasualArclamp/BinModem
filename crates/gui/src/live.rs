//! A real modem on a real line, published to the scope.
//!
//! The sibling of [`crate::engine`], which replays a capture. The loop here has
//! the same shape and does the same publishing; what differs is where the
//! samples come from and that there is somewhere for them to go back to. A
//! capture is a recording of somebody else's call and can only be watched. This
//! is a call of our own, and the terminal on the other side of the window is
//! the DTE: what is typed there goes to `feed_dte` and is answered by the AT
//! interpreter inside the modem, exactly as if it had arrived down a wire.
//!
//! The line is clocked by the input device. A sample arrives, the modem takes
//! one step, and what it hands back goes out. Nothing here paces itself against
//! a wall clock, because the sound card already is one.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dsp::Spectrum;
use line::AudioSink;
use modem::{Modem, Role, State};
use telemetry::{CallState, Direction, Leds, Publisher};

use crate::engine::{Control, FFT_SIZE, Ring, SCOPE_LEN, SPECTRUM_BINS, SYMBOL_HISTORY};

/// The rate the modem runs at, whatever the sound card is doing.
///
/// Enough for a 3400 Hz channel several times over, and the rate every data
/// pump in this workspace has been tested at. The device's own rate is
/// converted to and from this inside [`line::Duplex`].
const FS: f64 = 16_000.0;

/// What the window has asked the line to do.
#[derive(Debug, Clone)]
enum Request {
    Open { input: String, output: String },
    Close,
}

/// What the line is doing, for the window to show.
#[derive(Debug, Clone, Default)]
pub struct LineState {
    pub open: bool,
    pub input: String,
    pub output: String,
    /// Rates the two devices are actually running at, which are rarely the
    /// modem's and are converted on the way through.
    pub input_rate: u32,
    pub output_rate: u32,
    /// Why the last attempt to open failed, if it did.
    pub error: Option<String>,
    /// Samples the modem was not there to take. Any at all is a fault.
    pub dropped: u64,
    /// Times the line had nothing to send and sent silence instead. The one
    /// that decides whether a call works, and quite separate from the monitor
    /// running dry, which only decides whether it sounds nice in the room.
    pub underruns: u64,
    /// Characters that arrived with their stop bit in the wrong place, and how
    /// fast that is happening. A steady trickle is noise on the line; a burst
    /// is a network that dropped something, and they want different answers.
    pub framing_errors: u64,
    pub framing_errors_per_second: f64,
    /// Seconds of call recorded so far, if a recording is running.
    pub recording: Option<f64>,
    /// Where the last recording was written.
    pub recorded_to: Option<String>,
    /// Loudest sample put on the line lately, as a fraction of full scale.
    ///
    /// Worth watching, because the modulations differ enormously in how peaky
    /// they are at the same average power. Frequency shift keying has a
    /// constant envelope and sits at its peak permanently; a shaped
    /// constellation spends most of its time well below one and then goes
    /// nearly three times higher than its own average. A drive setting that
    /// suits one clips the other.
    pub tx_peak: f32,
}

/// The one thing the window and the line thread share.
///
/// Everything crossing between them is here: what has been typed, what the
/// line has been asked to do, and what it is doing. Mutexes rather than
/// channels because every one of these is "the current value" or "take what
/// there is" rather than a stream to be buffered — a keystroke queue that has
/// fallen behind is a bug, not something to grow.
///
/// The audio streams themselves cannot cross: on Windows a cpal stream is not
/// `Send` and has to live on the thread that made it. That is the whole reason
/// the line is opened by request rather than handed over.
#[derive(Debug)]
pub struct Session {
    typed: Mutex<Vec<u8>>,
    request: Mutex<Option<Request>>,
    state: Mutex<LineState>,
    /// How hard to drive the line, as an f32 in its bit pattern.
    drive: AtomicU32,
    /// Whether to keep what goes past, for looking at afterwards.
    recording: AtomicBool,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            typed: Mutex::default(),
            request: Mutex::default(),
            state: Mutex::default(),
            drive: AtomicU32::new(DEFAULT_DRIVE.to_bits()),
            recording: AtomicBool::new(false),
        }
    }
}

/// How hard to drive the line by default, as a multiple of what the modem
/// hands over.
///
/// Six decibels of headroom. Pulse shaping puts the peak of a modem well above
/// its own average, so a modem written out at unity clips on the peaks, and a
/// clipped constellation is one whose outer points have all moved inwards
/// together -- which is to say a receiver that will train happily on a
/// constellation that is not the one being sent.
const DEFAULT_DRIVE: f32 = 0.5;

impl Session {
    /// How hard the line is being driven.
    pub fn drive(&self) -> f32 {
        f32::from_bits(self.drive.load(Ordering::Relaxed))
    }

    /// Whether the call is being kept.
    pub fn recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Start or stop keeping it. Stopping writes the file.
    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::Relaxed);
    }

    /// Set it. A real modem has a transmit level and it is not decoration:
    /// too low and the far end cannot hear it over the noise the network adds,
    /// too high and everything between here and there clips or turns its
    /// automatic gain control down on the whole call.
    pub fn set_drive(&self, drive: f32) {
        self.drive.store(drive.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn type_bytes(&self, bytes: &[u8]) {
        if let Ok(mut q) = self.typed.lock() {
            q.extend_from_slice(bytes);
        }
    }

    /// Ask for the line to be opened on these two devices.
    ///
    /// Replaces any line already open, which is what changing a device in the
    /// window means.
    pub fn open(&self, input: &str, output: &str) {
        self.ask(Request::Open {
            input: input.to_owned(),
            output: output.to_owned(),
        });
    }

    /// Put the line down. The modem stays: `AT` still answers `OK`.
    pub fn close(&self) {
        self.ask(Request::Close);
    }

    pub fn state(&self) -> LineState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn ask(&self, request: Request) {
        if let Ok(mut slot) = self.request.lock() {
            *slot = Some(request);
        }
    }

    fn take_request(&self) -> Option<Request> {
        self.request.lock().ok().and_then(|mut r| r.take())
    }

    fn take_typed(&self) -> Vec<u8> {
        self.typed.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }

    fn set_state(&self, state: LineState) {
        if let Ok(mut slot) = self.state.lock() {
            *slot = state;
        }
    }
}

/// Start the modem. It has no line until the window gives it one.
///
/// There is no device here and no default, deliberately. The default output on
/// a desktop machine is whatever the speakers are plugged into, and a modem
/// handshake played through speakers is both useless and unpleasant.
pub fn spawn(
    tx: Publisher,
    control: Arc<Control>,
    session: Arc<Session>,
    sink: Arc<AudioSink>,
) -> JoinHandle<()> {
    thread::spawn(move || run(tx, control, session, sink))
}

fn run(tx: Publisher, control: Arc<Control>, session: Arc<Session>, sink: Arc<AudioSink>) {
    tx.log(Direction::Note, "modem ready; choose a line and open it");
    tx.log(Direction::Note, "type AT commands; ATD to dial, ATA to answer, +++ to escape");

    // Opened and closed on request, and never handed across a thread: on
    // Windows a cpal stream is not Send and has to stay where it was made.
    let mut audio: Option<line::Duplex> = None;
    let mut modem = Modem::new(FS);
    let mut spectrum = Spectrum::new(FFT_SIZE, FS);
    let mut waveform = Ring::new(SCOPE_LEN);
    let mut baseband = Ring::new(SCOPE_LEN);
    let mut bins = vec![0.0f64; SPECTRUM_BINS];
    let mut symbols: std::collections::VecDeque<f32> =
        std::collections::VecDeque::with_capacity(SYMBOL_HISTORY);
    let mut points: std::collections::VecDeque<(f32, f32)> =
        std::collections::VecDeque::with_capacity(SYMBOL_HISTORY);

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;
    // What arrived and what was sent, interleaved, so the two stay lined up
    // sample for sample. That pairing is the whole value of the thing: a
    // capture of somebody else's two-wire call has both directions already
    // summed and no filter can pull them apart again, whereas this can be run
    // through a receiver as many times as it takes with the other half of the
    // conversation there to check the answer against.
    let mut recording: Vec<f32> = Vec::new();
    let mut was_recording = false;
    let mut tx_peak = 0.0f32;
    let (mut errors_before, mut errors_at) = (0u64, Instant::now());
    let mut typed_recently = Instant::now() - Duration::from_secs(1);
    let mut heard_recently = typed_recently;
    let mut last_state = State::Command;
    // Where inside a start-up the call has got to, logged as it changes. A
    // call that will not come up is always stuck somewhere particular, and a
    // timestamped list of where it went is the difference between debugging it
    // and describing it.
    let mut last_phase = "";
    // The same, for the error control that runs on top of whatever the line
    // settled on.
    let mut last_ec = "";

    let publish_every = Duration::from_millis(16);
    let mut next_publish = Instant::now();

    while !control.quit.load(Ordering::Relaxed) {
        if let Some(request) = session.take_request() {
            // Dropping the old one stops its streams, which has to happen
            // before the new ones open on the same device.
            audio = None;
            let mut state = LineState::default();
            match request {
                Request::Open { input, output } => {
                    match line::Duplex::open(Some(&input), Some(&output), FS) {
                        Ok(open) => {
                            tx.log(
                                Direction::Note,
                                format!(
                                    "line open: out {} at {} Hz, in {} at {} Hz",
                                    open.output_device,
                                    open.output_rate,
                                    open.input_device,
                                    open.input_rate
                                ),
                            );
                            state = LineState {
                                open: true,
                                input: open.input_device.clone(),
                                output: open.output_device.clone(),
                                input_rate: open.input_rate,
                                output_rate: open.output_rate,
                                ..LineState::default()
                            };
                            audio = Some(open);
                        }
                        Err(e) => {
                            tx.log(Direction::Note, format!("could not open the line: {e}"));
                            state.error = Some(e);
                        }
                    }
                }
                Request::Close => tx.log(Direction::Note, "line closed"),
            }
            session.set_state(state);
        }

        // Everything the terminal has typed since last time. This goes in
        // whether or not there is a line at all: a modem answers `AT` with
        // `OK` sitting on a desk with nothing plugged into it, and a terminal
        // that had to wait for audio before its own modem would talk to it
        // would feel broken.
        let typed = session.take_typed();
        if !typed.is_empty() {
            typed_recently = Instant::now();
            tx_bytes += typed.len() as u64;
            for b in &typed {
                modem.feed_dte(*b);
            }
        }

        let Some(audio) = audio.as_ref() else {
            drain_dte(&mut modem, &tx, &mut rx_bytes, &mut heard_recently);
            thread::sleep(Duration::from_millis(8));
            continue;
        };

        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            // Nothing has arrived, so nothing can be stepped: the line is the
            // clock. Still hand the terminal whatever the modem said in the
            // meantime, which is how `OK` gets back before a call exists.
            drain_dte(&mut modem, &tx, &mut rx_bytes, &mut heard_recently);
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        to_line.clear();
        let drive = session.drive();
        for &s in &from_line {
            let heard = f64::from(s);
            to_line.push(modem.step(heard) as f32 * drive);

            if let Some(sym) = modem.take_symbol() {
                if symbols.len() == SYMBOL_HISTORY {
                    symbols.pop_front();
                }
                symbols.push_back(sym as f32);
            }
            if let Some(p) = modem.constellation_point() {
                let p = (p.0 as f32, p.1 as f32);
                // Only when it moves, so a motionless constellation is not
                // filled with copies of one point.
                if points.back() != Some(&p) {
                    if points.len() == SYMBOL_HISTORY {
                        points.pop_front();
                    }
                    points.push_back(p);
                }
            }
            baseband.push(modem.discriminator().unwrap_or(0.0) as f32);
            spectrum.push(heard);
            waveform.push(s);
        }
        audio.transmit(&to_line);

        let recording_now = session.recording();
        if recording_now {
            if !was_recording {
                recording.clear();
                tx.log(Direction::Note, "recording");
            }
            // Half an hour at sixteen thousand samples a second in two
            // channels is a hundred and fifteen megabytes, which is where
            // this stops rather than filling the machine. A modem call worth
            // looking at is over in minutes.
            const LIMIT: usize = 16_000 * 2 * 60 * 30;
            if recording.len() < LIMIT {
                for (heard, sent) in from_line.iter().zip(to_line.iter()) {
                    recording.push(*heard);
                    recording.push(*sent);
                }
            }
        } else if was_recording {
            let seconds = recording.len() as f64 / 2.0 / FS;
            match save(&recording) {
                Ok(path) => {
                    tx.log(Direction::Note, format!("kept {seconds:.1} s as {path}"));
                    if let Ok(mut state) = session.state.lock() {
                        state.recorded_to = Some(path);
                    }
                }
                Err(e) => tx.log(Direction::Note, format!("could not write it: {e}")),
            }
            recording = Vec::new();
        }
        was_recording = recording_now;

        // Decays rather than resets, so a peak stays up long enough to read
        // instead of flickering past between repaints.
        let block_peak = to_line.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        tx_peak = (tx_peak * 0.90).max(block_peak);
        // What the monitor plays is what the modem heard, so the ear and the
        // scopes are looking at the same thing.
        sink.push(&from_line);

        drain_dte(&mut modem, &tx, &mut rx_bytes, &mut heard_recently);

        let phase = modem.line_phase();
        if phase != last_phase {
            if phase != "on hook" {
                tx.log(Direction::Note, format!("{}: {phase}", modem.standard()));
            }
            last_phase = phase;
        }

        // The layer above the line, traced the same way. A call that connects
        // without error control has failed at one of these steps, and which
        // one is the whole of the diagnosis.
        let ec = modem.error_control_phase();
        if ec != last_ec {
            if !ec.is_empty() {
                let detail = if modem.compressing() {
                    ", V.42bis"
                } else if ec == "connected" {
                    ", no compression"
                } else {
                    ""
                };
                tx.log(Direction::Note, format!("V.42: {ec}{detail}"));
            }
            last_ec = ec;
        }

        let state = modem.state();
        if state != last_state {
            let note = match state {
                State::Command => "on hook".to_owned(),
                State::Handshaking => "handshaking".to_owned(),
                // The one moment worth reporting in full. Everything here is
                // settled by then and none of it is visible from the terminal
                // side, which sees a CONNECT and a rate and nothing else.
                State::Data => {
                    let mut s = format!(
                        "connected: {} at {} bit/s, error control {}, compression {}",
                        modem.standard(),
                        modem.rate().unwrap_or(0),
                        if modem.error_controlled() { "V.42" } else { "off" },
                        if modem.compressing() { "V.42bis" } else { "off" }
                    );
                    // A count that climbs while the terminal still reads
                    // correctly is LAPM doing its job, and is the only view of
                    // how hard it is having to work.
                    let damaged = modem.damaged_frames();
                    if damaged > 0 {
                        s.push_str(&format!(", {damaged} frames damaged"));
                    }
                    if let Some(r) = modem.reflection() {
                        s.push_str(&format!(
                            ", echo found {:.0} ms away at {:.2} of the line",
                            r.delay as f64 / FS * 1000.0,
                            r.strength
                        ));
                    }
                    s
                }
                State::OnlineCommand => "escaped to command state".to_owned(),
            };
            tx.log(Direction::Note, note);
            last_state = state;
        }

        if Instant::now() >= next_publish {
            next_publish = Instant::now() + publish_every;
            if spectrum.ready() {
                spectrum.magnitudes_db(&mut bins);
            }
            let level = {
                let n = waveform.data.len().max(1);
                (waveform.data.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>()
                    / n as f64)
                    .sqrt()
            };
            let rate = modem.rate();
            let recent = |at: Instant| at.elapsed() < Duration::from_millis(250);

            tx.publish(|f| {
                f.sample_rate = FS;
                waveform.copy_into(&mut f.waveform);
                baseband.copy_into(&mut f.baseband);
                for (slot, &v) in f.spectrum_db.iter_mut().zip(bins.iter()) {
                    *slot = v as f32;
                }
                f.hz_per_bin = FS / FFT_SIZE as f64;
                f.rx_level_db = (20.0 * (level + 1e-9).log10()) as f32;
                f.carrier = modem.carrier();
                f.state = match state {
                    State::Command if modem.off_hook() => CallState::OffHook,
                    State::Command => CallState::Idle,
                    State::Handshaking => CallState::Negotiating,
                    State::Data => CallState::Connected,
                    // The call is still up; the terminal has stepped back to
                    // talking to the modem rather than through it. Off hook is
                    // exactly what that is, and keeping it separate from
                    // connected is what lets anything watching tell whether an
                    // escape has already happened.
                    State::OnlineCommand => CallState::OffHook,
                };
                f.modulation = modem.standard();
                f.line_phase = modem.line_phase();
                f.bit_rate = rate;
                f.rx_bytes = rx_bytes;
                f.tx_bytes = tx_bytes;
                f.tones = modem.states();
                f.symbol_label = modem.shape();
                f.snr_db = modem.residual_error().map(|e| {
                    // The decision margin as decibels, so a tighter
                    // constellation reads as a larger number.
                    (-20.0 * e.max(1e-3).log10()) as f32
                });
                f.symbols.clear();
                f.symbols.extend(symbols.iter().copied());
                f.constellation.clear();
                f.constellation.extend(points.iter().copied());
                f.leds = Leds {
                    mr: true,
                    tr: true,
                    sd: recent(typed_recently),
                    rd: recent(heard_recently),
                    cd: modem.carrier(),
                    oh: modem.off_hook(),
                    aa: modem.role() == Role::Answering,
                    hs: rate.is_some_and(|r| r >= 9600),
                    ec: modem.error_controlled(),
                };
            });

            // A line that loses samples is one the modem is not keeping up
            // with, and timing recovery has no way to know a sample went
            // missing: it reads the gap as the clock having moved. Worth
            // showing while it is happening rather than in a summary nobody
            // reads.
            let dropped = audio.dropped_in();
            let underruns = audio.underruns();
            if let Ok(mut state) = session.state.lock() {
                state.dropped = dropped;
                state.underruns = underruns;
                state.tx_peak = tx_peak;
                state.recording = recording_now
                    .then(|| recording.len() as f64 / 2.0 / FS);
                let errors = modem.framing_errors();
                let since = errors_at.elapsed().as_secs_f64();
                if since >= 1.0 {
                    state.framing_errors_per_second =
                        (errors - errors_before) as f64 / since;
                    errors_before = errors;
                    errors_at = Instant::now();
                }
                state.framing_errors = errors;
            }
        }
    }
}

/// Write a recording out, and say where it went.
///
/// Named by the clock rather than by anything about the call, because what
/// makes one of these worth keeping is usually not known until afterwards.
fn save(samples: &[f32]) -> Result<String, String> {
    let dir = std::path::Path::new("captures");
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("live-{stamp}.wav"));
    // Two channels: what arrived, and what was sent at the same instant.
    line::wav::write_channels(&path, samples, 2, FS as u32).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Hand the terminal everything the modem has to say.
fn drain_dte(modem: &mut Modem, tx: &Publisher, rx_bytes: &mut u64, heard: &mut Instant) {
    let out = modem.take_dte();
    if out.is_empty() {
        return;
    }
    *rx_bytes += out.len() as u64;
    *heard = Instant::now();
    tx.line_data(&out);
}
