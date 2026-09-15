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

use crate::engine::{Control, FFT_SIZE, Ring, SCOPE_LEN, SPECTRUM_BINS, SYMBOL_HISTORY, scope_depth};
use crate::dialin::{Login, Next as LoginNext, Settings as DialIn};
use crate::network::{Networking, Request as NetRequest, View as NetView};

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

/// Input devices that are one half of a two-wire line, best first.
///
/// One cable is a two-wire line: everything written to it comes back, so a
/// modem on one hears its own transmission at full strength. That is a fine
/// model of a telephone pair with two modems across it and useless for
/// reaching anything outside the machine, where what is wanted is a hybrid and
/// there is none. Two cables are the hybrid: A carries what the softphone
/// plays, B carries what this modem says, and neither modem hears itself.
pub const LINE_IN: &[&str] = &["CABLE-A Output", "CABLE Output"];
/// And the other half.
pub const LINE_OUT: &[&str] = &["CABLE-B Input", "CABLE Input"];

/// Where in `names` the first of `wanted` appears, if any of them does.
pub fn named(names: &[String], wanted: &[&str]) -> Option<usize> {
    wanted
        .iter()
        .find_map(|want| names.iter().position(|n| n.contains(want)))
}

/// The two cables a machine set up for this has, if it has them.
///
/// Named devices only, and nothing is guessed at. Falling back to whatever
/// device happens to be first would open the line on the speakers, and a
/// handshake played through speakers is no use to anyone -- so a machine
/// without the cables gets an empty picker and a person to fill it in, which
/// is the honest answer to not knowing.
///
/// Called from the line's own thread, which is the thread that opens the
/// device. Enumerating audio devices initialises COM, and doing that on the
/// main thread before the window and its graphics context exist is worth not
/// doing on general Windows principle -- but only on principle. It was moved
/// here while chasing a fault that turned out to be a telephone routed
/// somewhere else, and it fixed nothing.
fn preferred_line() -> Option<(String, String)> {
    let inputs = line::input_devices();
    let outputs = line::output_devices();
    let input = inputs.get(named(&inputs, LINE_IN)?)?.clone();
    let output = outputs.get(named(&outputs, LINE_OUT)?)?.clone();
    Some((input, output))
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
    /// Mean power going out and mean power coming back, both as a fraction of
    /// full scale, over the last little while.
    ///
    /// The pair rather than either alone, because what matters about a
    /// transmit level is how it compares with the far end's. A modem sending
    /// nine decibels louder than the signal arriving is a modem whose own
    /// signal is being distorted somewhere in the path -- and the way that
    /// shows is not silence but a far end that answers the robust parts of a
    /// handshake and none of the delicate ones.
    pub tx_rms: f32,
    pub rx_rms: f32,
}

/// A transfer in progress: one half of a ZMODEM session and its bookkeeping.
///
/// The terminal does not see any of this. While a transfer runs it owns the
/// byte stream in both directions -- what the modem hands up goes to the
/// protocol rather than to the screen, and what the protocol says goes down
/// the line -- because a board sending a file is not saying anything a person
/// wants to read, and a keystroke in the middle of it would be data.
#[derive(Debug)]
enum Job {
    Sending(Box<transfer::zmodem::Sender>),
    Receiving(Box<transfer::zmodem::Receiver>, std::path::PathBuf),
}

/// What a file transfer is doing, for the window to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransferView {
    /// Whether this end is sending or receiving.
    pub sending: bool,
    pub name: String,
    pub position: u64,
    pub total: Option<u64>,
    /// Times the protocol had to go back over ground it had covered.
    pub rewinds: u32,
    /// Bytes sent a second time because of those.
    pub resent: u64,
    /// Subpackets that failed their check sequence.
    pub damaged: u32,
    /// Bytes a second over the last few seconds, and over the file so far --
    /// both from when the file started moving, and counting new ground only.
    pub recent: Option<f64>,
    pub average: Option<f64>,
    /// Seconds since the file started moving, and left at the recent rate.
    pub elapsed: f64,
    pub remaining: Option<f64>,
    /// The line's own rate in bit/s, for what share of it the file is getting.
    pub line_bps: Option<u32>,
    /// Empty while it runs; what happened, once it is over.
    pub outcome: String,
    pub finished: bool,
    /// Where a received file was written.
    pub written_to: Option<String>,
}

/// What the window has asked a transfer to do.
#[derive(Debug, Clone)]
enum TransferRequest {
    /// Send this file.
    Send(std::path::PathBuf),
    /// Take whatever the far end offers, into this directory.
    Receive(std::path::PathBuf),
    Cancel,
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
    /// Set when the window asks for the call to be put down, however it is on
    /// the line -- a data call, a fax, a handshake. The line thread hangs up
    /// and clears it.
    hang_up: AtomicBool,
    /// Set when the window asks for a retrain: V.34 goes back through phase 2
    /// on the same call. The line thread asks the modem and clears it.
    retrain: AtomicBool,
    /// A transfer the window has asked for, until the line thread takes it.
    transfer_request: Mutex<Option<TransferRequest>>,
    /// What the transfer is doing, for the window to read.
    transfer: Mutex<Option<TransferView>>,
    /// Something the window has asked the PPP link to do.
    network_request: Mutex<Option<NetRequest>>,
    /// And what it is doing, once there is one.
    network: Mutex<Option<NetView>>,
    /// The account, and whether calls are answered with a login prompt.
    dialin: Mutex<DialIn>,
    /// Where a login on the call has got to, while one is running.
    login: Mutex<Option<String>>,
    /// Whether web traffic should be carried, kept here rather than only
    /// sent to a link, because a dial-in caller's link starts without anyone
    /// at this end pressing anything.
    carry_web: AtomicBool,
    /// What this end calls itself in a fax call, sent as a TSI.
    ///
    /// A setting of the machine rather than of the call, which is why it
    /// lives here beside the drive and not in a dial string. T.30 allows it
    /// to be blank and plenty of machines send nothing at all.
    fax_identification: Mutex<String>,
    /// The modulations the window allows a fax call to use.
    fax_offer: Mutex<Vec<fax::t30::Modulation>>,
    /// Whether the window allows error correction mode.
    fax_error_correction: AtomicBool,
    /// A page the window has loaded, waiting for the line thread to take it.
    ///
    /// Taken rather than read, and a page is megabytes of booleans, so it
    /// crosses the two threads exactly once.
    fax_page: Mutex<Option<fax::page::Page>>,
    /// A page that arrived, waiting for the window to take it.
    fax_received: Mutex<Option<fax::page::Page>>,
    /// Lines of a page that is still arriving, waiting for the window.
    fax_arriving: Mutex<Option<Arriving>>,
}

/// Lines of a page on their way from the line to the window.
///
/// Only the new ones cross, a batch at a time. A page is a megabyte or two of
/// booleans by the end, and handing the whole of it over every time another
/// line came off the decoder would copy it hundreds of times over.
#[derive(Debug, Clone, PartialEq)]
pub struct Arriving {
    /// Which page these lines belong to. A different number is a different
    /// page, and whatever the window had drawn of the last one goes.
    pub page: u64,
    pub resolution: fax::page::Resolution,
    /// Where in the page the first of `lines` goes.
    pub from: usize,
    pub lines: Vec<Vec<bool>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            typed: Mutex::default(),
            request: Mutex::default(),
            state: Mutex::default(),
            drive: AtomicU32::new(DEFAULT_DRIVE.to_bits()),
            transfer_request: Mutex::default(),
            transfer: Mutex::default(),
            network_request: Mutex::default(),
            network: Mutex::default(),
            dialin: Mutex::default(),
            login: Mutex::default(),
            carry_web: AtomicBool::new(false),
            fax_identification: Mutex::default(),
            fax_offer: Mutex::new(fax::call::OUR_MODULATIONS.to_vec()),
            fax_error_correction: AtomicBool::new(true),
            fax_page: Mutex::default(),
            fax_received: Mutex::default(),
            fax_arriving: Mutex::default(),
            recording: AtomicBool::new(false),
            hang_up: AtomicBool::new(false),
            retrain: AtomicBool::new(false),
        }
    }
}

/// How hard to drive the line by default, as a multiple of what the modem
/// hands over: twenty decibels down.
///
/// Headroom first. Pulse shaping puts the peak of a modem well above its own
/// average, so a modem written out at unity clips on the peaks, and a clipped
/// constellation is one whose outer points have all moved inwards together --
/// which is to say a receiver that will train happily on a constellation that
/// is not the one being sent.
///
/// Every transmitter here leaves at a root mean square of 0.707, and the
/// shaped constellations peak close to 2 -- tests/levels.rs in the data pump
/// measures both. At the old default of half, V.32's peaks reached the very
/// top of the scale. Twenty down leaves them fourteen decibels under it, and is
/// the level the calls through a softphone have been placed at.
const DEFAULT_DRIVE: f32 = 0.1;

impl Session {
    /// What this end calls itself in a fax call.
    pub fn fax_identification(&self) -> String {
        self.fax_identification.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn set_fax_identification(&self, who: &str) {
        if let Ok(mut v) = self.fax_identification.lock() {
            who.clone_into(&mut v);
        }
    }

    /// The modulations a fax call may use.
    pub fn fax_offer(&self) -> Vec<fax::t30::Modulation> {
        self.fax_offer.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn set_fax_offer(&self, offer: &[fax::t30::Modulation]) {
        if let Ok(mut v) = self.fax_offer.lock() {
            *v = offer.to_vec();
        }
    }

    /// Whether a fax call may use error correction mode.
    pub fn fax_error_correction(&self) -> bool {
        self.fax_error_correction.load(Ordering::Relaxed)
    }

    pub fn set_fax_error_correction(&self, on: bool) {
        self.fax_error_correction.store(on, Ordering::Relaxed);
    }

    /// Leave a page for the next fax call that dials.
    pub fn set_fax_page(&self, page: Option<fax::page::Page>) {
        if let Ok(mut v) = self.fax_page.lock() {
            *v = page;
        }
    }

    fn take_fax_page(&self) -> Option<fax::page::Page> {
        self.fax_page.lock().ok().and_then(|mut v| v.take())
    }

    fn set_fax_received(&self, page: fax::page::Page) {
        if let Ok(mut v) = self.fax_received.lock() {
            *v = Some(page);
        }
    }

    /// A page that arrived, once and only once.
    pub fn take_fax_received(&self) -> Option<fax::page::Page> {
        self.fax_received.lock().ok().and_then(|mut v| v.take())
    }

    /// More lines of the page arriving.
    ///
    /// Added to whatever the window has not taken yet if they carry straight on
    /// from it, and in place of it if they are a new page: a window that fell
    /// behind wants the page that is arriving, not the one before.
    fn push_fax_lines(&self, arriving: Arriving) {
        let Ok(mut waiting) = self.fax_arriving.lock() else { return };
        match waiting.as_mut() {
            Some(w) if w.page == arriving.page && w.from + w.lines.len() == arriving.from => {
                w.resolution = arriving.resolution;
                w.lines.extend(arriving.lines);
            }
            _ => *waiting = Some(arriving),
        }
    }

    /// The lines that have arrived since the window last asked.
    pub fn take_fax_lines(&self) -> Option<Arriving> {
        self.fax_arriving.lock().ok().and_then(|mut v| v.take())
    }

    /// How hard the line is being driven.
    pub fn drive(&self) -> f32 {
        f32::from_bits(self.drive.load(Ordering::Relaxed))
    }

    /// Whether the call is being kept.
    pub fn recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Put the call down, whatever it is: a data call, a fax, a handshake that
    /// has not finished. The window's own hang-up, for when there is no
    /// terminal to type `ATH` at -- a fax call in particular.
    pub fn hang_up(&self) {
        self.hang_up.store(true, Ordering::Relaxed);
    }

    fn take_hang_up(&self) -> bool {
        self.hang_up.swap(false, Ordering::Relaxed)
    }

    /// Ask the modem to retrain the line: V.34 back through phase 2, on the
    /// same call.
    pub fn retrain(&self) {
        self.retrain.store(true, Ordering::Relaxed);
    }

    fn take_retrain(&self) -> bool {
        self.retrain.swap(false, Ordering::Relaxed)
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

    /// Send a file over the connection.
    pub fn send_file(&self, path: std::path::PathBuf) {
        self.ask_transfer(TransferRequest::Send(path));
    }

    /// Take whatever the far end offers, into this directory.
    pub fn receive_into(&self, directory: std::path::PathBuf) {
        self.ask_transfer(TransferRequest::Receive(directory));
    }

    /// Stop, with 8.4's cancel sequence.
    pub fn cancel_transfer(&self) {
        self.ask_transfer(TransferRequest::Cancel);
    }

    fn ask_transfer(&self, request: TransferRequest) {
        if let Ok(mut slot) = self.transfer_request.lock() {
            *slot = Some(request);
        }
    }

    fn take_transfer_request(&self) -> Option<TransferRequest> {
        self.transfer_request.lock().ok().and_then(|mut s| s.take())
    }

    /// Bring PPP up over the call, so the two ends can carry IP.
    pub fn start_network(&self) {
        self.ask_network(NetRequest::Start);
    }

    /// Put it down and give the terminal its bytes back.
    pub fn stop_network(&self) {
        self.ask_network(NetRequest::Stop);
    }

    /// One echo to the far end.
    pub fn ping_once(&self) {
        self.ask_network(NetRequest::PingOnce);
    }

    /// Or a stream of them, until told otherwise.
    pub fn ping_repeatedly(&self, on: bool) {
        self.ask_network(NetRequest::PingRepeatedly(on));
    }

    /// Carry web traffic over the link, or stop.
    pub fn carry_web(&self, on: bool) {
        self.carry_web.store(on, Ordering::Relaxed);
        self.ask_network(NetRequest::Proxy(on));
    }

    /// Log in to the far end at its prompts, then bring PPP up.
    pub fn log_in(&self) {
        self.ask_network(NetRequest::LogIn);
    }

    /// What to log in with, and whether to answer with a login prompt.
    pub fn set_dialin(&self, settings: DialIn) {
        if let Ok(mut slot) = self.dialin.lock() {
            *slot = settings;
        }
    }

    fn dialin(&self) -> DialIn {
        self.dialin.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Where a login on the call has got to, if one is running.
    pub fn login(&self) -> Option<String> {
        self.login.lock().ok().and_then(|s| s.clone())
    }

    fn set_login(&self, stage: Option<String>) {
        if let Ok(mut slot) = self.login.lock()
            && *slot != stage
        {
            *slot = stage;
        }
    }

    /// What the link is doing, if there is one.
    pub fn network(&self) -> Option<NetView> {
        self.network.lock().ok().and_then(|s| s.clone())
    }

    fn ask_network(&self, request: NetRequest) {
        if let Ok(mut slot) = self.network_request.lock() {
            *slot = Some(request);
        }
    }

    fn take_network_request(&self) -> Option<NetRequest> {
        self.network_request.lock().ok().and_then(|mut s| s.take())
    }

    fn set_network(&self, view: Option<NetView>) {
        if let Ok(mut slot) = self.network.lock() {
            *slot = view;
        }
    }

    /// What the transfer is doing, if one is.
    pub fn transfer(&self) -> Option<TransferView> {
        self.transfer.lock().ok().and_then(|s| s.clone())
    }

    fn set_transfer(&self, view: Option<TransferView>) {
        if let Ok(mut slot) = self.transfer.lock() {
            *slot = view;
        }
    }

    pub fn state(&self) -> LineState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Whether a request is already waiting, so that one made before the
    /// thread started is not quietly replaced by the one it would have chosen.
    fn has_request(&self) -> bool {
        self.request.lock().map(|s| s.is_some()).unwrap_or(false)
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
    // The line to open when nobody named one. Decided here rather than in
    // `main` because this is the thread that opens the device, so it is the
    // one that should go looking for it.
    if !session.has_request() {
        if let Some((input, output)) = preferred_line() {
            session.open(&input, &output);
        } else {
            tx.log(
                Direction::Note,
                "no VB-Audio cables found; choose a line in the window",
            );
        }
    }
    tx.log(Direction::Note, "modem ready");
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
    // What the scope was last drawing. A data call is one modulation from
    // start to finish, so this never changed and nothing had to notice. A fax
    // call changes eight or ten times: 300 bit/s frames, then a page carrier,
    // then frames again, and the two want different pictures. Carrying the
    // points of one into the other draws a constellation over an eye.
    let mut drawing = modem.shape();
    // How much of the page arriving the window has been handed, and which
    // page that was. A page that starts again -- a new call, or the next page
    // of this one -- has fewer lines than were handed over, and gets a new
    // number.
    let (mut fax_page_number, mut fax_lines_handed) = (0u64, 0usize);

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
    // What crossed inside the error control, written beside the audio. The
    // recording says what was on the line and the terminal says what came out
    // of it, and neither says what the far end sent -- which on a link that
    // establishes and then carries nothing is the only question there is.
    let mut frames: Vec<String> = Vec::new();
    let mut was_recording = false;
    let mut tx_peak = 0.0f32;
    let (mut tx_rms, mut rx_rms) = (0.0f32, 0.0f32);
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
    // Compressed streams that would not decode. The one fault on this call
    // that says nothing about itself: the link simply goes, and everything
    // still on the panel -- rate, level, carrier -- looks perfect, so it reads
    // as a line fault rather than as the two ends disagreeing about what a
    // codeword means.
    let mut last_undecodable = 0;
    // Whether the line was retraining last time round, and what it was
    // carrying before it started.
    let mut was_retraining = false;
    let mut rate_before = 0;
    // The transfer, while there is one, when it started, and how fast it is
    // going.
    let mut job: Option<Job> = None;
    let mut job_started = Instant::now();
    let mut meter = crate::speed::Speedometer::new();
    // The PPP link, while one is up. Like a transfer it owns the byte stream
    // while it runs, and for the same reason.
    let mut networking: Option<Networking> = None;
    // A login in front of it: the terminal server for a caller, or the script
    // logging in to a far end. It owns the stream too.
    let mut login: Option<Login> = None;
    // Whether there was a call last time round, for noticing a new one.
    let mut was_online = false;
    // When typing was last turned away, so saying so does not fill the
    // transcript.
    let mut refused_typing: Option<Instant> = None;
    // Line time the link has not been told about yet, in milliseconds.
    //
    // The loop turns over on audio arriving, so a round is a block and not a
    // millisecond, and the blocks are not all the same size. Everywhere else
    // that is near enough; here it is not. A Restart timer that runs at a
    // tenth of real time will resend a Configure-Request half a minute after
    // it should, and a round trip measured in rounds of this loop is not a
    // round trip. So the link is given the line's own clock, which is the
    // same one the modem keeps: samples divided by the rate.
    let mut owed_ms = 0.0f64;

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

        // What the window has asked the PPP link to do.
        if let Some(request) = session.take_network_request() {
            match request {
                NetRequest::Start | NetRequest::LogIn if networking.is_some() => {}
                NetRequest::Start | NetRequest::LogIn if !modem.is_online() => {
                    tx.log(Direction::Note, "ppp: there is no call to run it over");
                }
                NetRequest::Start | NetRequest::LogIn if job.is_some() => {
                    // Both want the whole byte stream, and neither would
                    // survive the other having half of it.
                    tx.log(Direction::Note, "ppp: not while a transfer is running");
                }
                NetRequest::Start => {
                    login = None;
                    let link = start_link(&modem, &session, Vec::new(), None, &tx);
                    networking = Some(link);
                }
                NetRequest::LogIn => {
                    login = Some(Login::call(&session.dialin(), &tx));
                }
                NetRequest::Stop => {
                    if login.take().is_some() {
                        tx.log(Direction::Note, "login: stopped; the terminal is yours again");
                    }
                    if let Some(mut link) = networking.take() {
                        for b in link.stop(&tx) {
                            modem.feed_dte(b);
                        }
                    }
                    session.set_network(None);
                }
                NetRequest::PingOnce => match networking.as_mut() {
                    Some(link) => link.ping_once(&tx),
                    None => tx.log(Direction::Note, "ppp: there is no link to ping over"),
                },
                NetRequest::PingRepeatedly(on) => {
                    if let Some(link) = networking.as_mut() {
                        link.ping_repeatedly(on);
                    }
                }
                NetRequest::Proxy(on) => match networking.as_mut() {
                    Some(link) => link.carry_web(on, &tx),
                    None => tx.log(Direction::Note, "proxy: there is no link to carry it"),
                },
            }
        }

        // A call that has ended takes the link with it: RFC 1661 3.7 calls
        // that the layer below going down, and there is nothing to negotiate
        // with once the carrier has gone.
        if networking.is_some() && !modem.is_online() {
            networking = None;
            session.set_network(None);
            tx.log(Direction::Note, "ppp: the call ended");
        }
        if !modem.is_online() {
            login = None;
        }

        // A call has just come up. If this end answered it and the window
        // says to, the caller meets a login prompt rather than a terminal.
        let online = modem.is_online();
        if online && !was_online {
            let settings = session.dialin();
            if settings.serve && modem.role() == Role::Answering && networking.is_none() && job.is_none() {
                login = Some(Login::serve(&settings, &tx));
            }
        }
        was_online = online;

        // The line's own time since last round, handed to whatever is running
        // above the modem. Taken every round whether or not anything is, so
        // that a link started a minute into a call is not handed that minute
        // all at once -- which fired its restart timer the moment it opened,
        // and sent a second Configure-Request before the first was answered.
        let ms = owed_ms as u32;
        owed_ms -= f64::from(ms);

        if let Some(running) = login.as_mut() {
            let (out, next) = running.step(ms, &tx);
            for b in out {
                modem.feed_dte(b);
            }
            session.set_login(Some(running.stage()));
            match next {
                None => {}
                Some(LoginNext::Ppp { user, early }) => {
                    let serving = running.serving();
                    login = None;
                    let mut link = start_link(&modem, &session, early, serving.then_some(user), &tx);
                    link.hang_up_after = serving;
                    networking = Some(link);
                }
                Some(LoginNext::HangUp(why)) => {
                    login = None;
                    tx.log(Direction::Note, format!("dial-in: hanging up: {why}"));
                    modem.hang_up();
                }
                Some(LoginNext::GiveBack(why)) => {
                    login = None;
                    tx.log(Direction::Note, format!("login: {why}; the terminal is yours again"));
                }
            }
        }
        if login.is_none() {
            session.set_login(None);
        }

        // Drive the link. Its frames go down the line the same way a keystroke
        // does, because to the modem that is what they are.
        if let Some(link) = networking.as_mut() {
            for b in link.step(ms, &tx) {
                modem.feed_dte(b);
            }
            session.set_network(Some(link.view()));
            // A dial-in caller's link that has ended is the end of the call,
            // the way a provider's modem hung up when PPP did.
            if link.hang_up_after && link.ended() {
                tx.log(Direction::Note, "dial-in: the link is down, hanging up");
                modem.hang_up();
                networking = None;
                session.set_network(None);
            }
        }

        // A transfer the window has asked for.
        if let Some(request) = session.take_transfer_request() {
            match request {
                _ if networking.is_some() => {
                    tx.log(Direction::Note, "not while the PPP link is up");
                }
                TransferRequest::Send(path) => match read_to_send(&path) {
                    Ok((info, data)) => {
                        tx.log(
                            Direction::Note,
                            format!("sending {} ({} bytes)", info.name, data.len()),
                        );
                        let rate = modem.rate().unwrap_or(2400);
                        job = Some(Job::Sending(Box::new(
                            transfer::zmodem::Sender::new(info, data, rate),
                        )));
                        job_started = Instant::now();
                        meter = crate::speed::Speedometer::new();
                    }
                    Err(e) => tx.log(Direction::Note, format!("cannot send it: {e}")),
                },
                TransferRequest::Receive(into) => {
                    tx.log(Direction::Note, "waiting for the far end to send");
                    job = Some(Job::Receiving(Box::default(), into));
                    job_started = Instant::now();
                    meter = crate::speed::Speedometer::new();
                }
                TransferRequest::Cancel => {
                    match job.as_mut() {
                        Some(Job::Sending(s)) => s.cancel(),
                        Some(Job::Receiving(r, _)) => r.cancel(),
                        None => {}
                    }
                    tx.log(Direction::Note, "transfer cancelled");
                }
            }
        }

        // Drive whatever is running. Its output goes down the line the same
        // way a keystroke does, because to the modem it is the same thing.
        if let Some(active) = job.as_mut() {
            // How much more the line will take. Two seconds of it: enough to
            // keep the modem busy through any scheduling hiccup, and short
            // enough that when the far end asks the sender to go back, what
            // has to drain first is two seconds and not the rest of the file.
            let ahead = (modem.rate().unwrap_or(2400) as usize / 4).max(1024);
            let room = ahead.saturating_sub(modem.queued());
            let (out, mut done) = step_job(active, &tx, room);
            for b in out {
                modem.feed_dte(b);
            }
            meter.update(job_started.elapsed().as_secs_f64(), done.0.position);
            done.0.recent = meter.recent();
            done.0.average = meter.average();
            done.0.elapsed = meter.elapsed();
            done.0.remaining = done.0.total.and_then(|total| meter.remaining(total));
            done.0.line_bps = modem.rate();
            session.set_transfer(Some(done.0));
            if done.1 {
                // The last of it stays on the window: what it came to, and how
                // fast, is what anyone who watched it will want to read.
                job = None;
            }
        }

        // Everything the terminal has typed since last time. This goes in
        // whether or not there is a line at all: a modem answers `AT` with
        // `OK` sitting on a desk with nothing plugged into it, and a terminal
        // that had to wait for audio before its own modem would talk to it
        // would feel broken.
        // Before anything typed is acted on, because one of the things that
        // can be typed is the dial that starts a fax call, and the call takes
        // a copy of this as it is built. Set after, it is a block too late
        // and the first call of a session goes out anonymous -- which is what
        // it did, twice, against a real machine.
        //
        // Every block rather than at the dial: the window is a different
        // thread, and a call can be placed by typing at the terminal instead
        // of by pressing the button.
        modem.fax_identification = session.fax_identification();
        modem.fax_offer = session.fax_offer();
        modem.fax_error_correction = session.fax_error_correction();
        if let Some(page) = session.take_fax_page() {
            modem.fax_page = Some(page);
        }
        if let Some(call) = modem
            .fax_call()
            .filter(|c| c.role() == fax::call::Role::Answerer)
        {
            let lines = call.lines();
            if lines.len() < fax_lines_handed {
                fax_page_number += 1;
                fax_lines_handed = 0;
            }
            if lines.len() > fax_lines_handed {
                session.push_fax_lines(Arriving {
                    page: fax_page_number,
                    resolution: call.resolution(),
                    from: fax_lines_handed,
                    lines: lines[fax_lines_handed..].to_vec(),
                });
                fax_lines_handed = lines.len();
            }
        }
        if let Some(page) = modem.take_received_page() {
            session.set_fax_received(page);
        }

        if session.take_hang_up() {
            tx.log(Direction::Note, "putting the call down");
            modem.hang_up();
        }
        if session.take_retrain() {
            tx.log(Direction::Note, "retraining the line");
            modem.retrain();
        }

        let typed = session.take_typed();
        if !typed.is_empty() {
            // A keystroke in the middle of a frame is a frame that fails its
            // check, and one typed at a caller's login prompt is typed into
            // somebody else's session. So while anything above the modem has
            // the stream, typing goes nowhere -- except the escape, which is
            // how the call is put down by hand.
            let taken = modem.state() == State::Data
                && (networking.is_some() || login.is_some() || job.is_some());
            if taken && typed != b"+++" {
                // Once every few seconds, not once a keystroke.
                if refused_typing.is_none_or(|at| at.elapsed() > Duration::from_secs(5)) {
                    refused_typing = Some(Instant::now());
                    tx.log(Direction::Note, "not typed: the call is carrying PPP or a login, not the terminal");
                }
            } else {
                typed_recently = Instant::now();
                tx_bytes += typed.len() as u64;
                for b in &typed {
                    modem.feed_dte(*b);
                }
            }
        }

        let Some(audio) = audio.as_ref() else {
            drain_dte(
            &mut modem,
            &tx,
            &mut rx_bytes,
            &mut heard_recently,
            job.as_mut(),
            networking.as_mut(),
            login.as_mut(),
        );
            thread::sleep(Duration::from_millis(8));
            continue;
        };

        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            // Nothing has arrived, so nothing can be stepped: the line is the
            // clock. Still hand the terminal whatever the modem said in the
            // meantime, which is how `OK` gets back before a call exists.
            drain_dte(
            &mut modem,
            &tx,
            &mut rx_bytes,
            &mut heard_recently,
            job.as_mut(),
            networking.as_mut(),
            login.as_mut(),
        );
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        to_line.clear();
        let drive = session.drive();
        let depth = scope_depth(modem.states());
        for &s in &from_line {
            let heard = f64::from(s);
            to_line.push(modem.step(heard) as f32 * drive);

            if modem.shape() != drawing {
                drawing = modem.shape();
                symbols.clear();
                points.clear();
            }
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
                    // Down to the depth, which falls when data mode ends.
                    while points.len() >= depth {
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
        owed_ms += from_line.len() as f64 / FS * 1000.0;

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
            let at = recording.len() as f64 / 2.0 / FS;
            for f in modem.take_frame_log() {
                frames.push(frame_line(at, &f));
            }
        } else if was_recording {
            keep(&recording, &frames, &tx, &session);
            recording = Vec::new();
            frames = Vec::new();
        }
        was_recording = recording_now;

        // Decays rather than resets, so a peak stays up long enough to read
        // instead of flickering past between repaints.
        let block_peak = to_line.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        tx_peak = (tx_peak * 0.90).max(block_peak);
        // Averaged slowly, and only over blocks that carry something: a mean
        // that includes the gaps is a mean of how much of the time the modem
        // was talking, which is not the question.
        let mean = |b: &[f32]| {
            (b.iter().map(|s| s * s).sum::<f32>() / b.len().max(1) as f32).sqrt()
        };
        let (tx_now, rx_now) = (mean(&to_line), mean(&from_line));
        if let Some((tx, rx)) = both_carrying(tx_now, rx_now) {
            tx_rms = tx_rms * 0.95 + tx * 0.05;
            rx_rms = rx_rms * 0.95 + rx * 0.05;
        }
        // What the monitor plays is what the modem heard, so the ear and the
        // scopes are looking at the same thing.
        sink.push(&from_line);

        drain_dte(
            &mut modem,
            &tx,
            &mut rx_bytes,
            &mut heard_recently,
            job.as_mut(),
            networking.as_mut(),
            login.as_mut(),
        );

        // A retrain is not a new call and the terminal is told nothing about
        // it, so the transcript is the only place it shows. Which rate it
        // comes back at is the interesting part: a line that has got worse
        // gives back a slower one, and that is the whole point of the
        // procedure.
        let retraining = modem.retraining();
        if retraining != was_retraining {
            if retraining {
                rate_before = modem.rate().unwrap_or(0);
                tx.log(
                    Direction::Note,
                    format!("retraining, was {rate_before} bit/s"),
                );
            } else {
                let now = modem.rate().unwrap_or(0);
                let note = match now.cmp(&rate_before) {
                    std::cmp::Ordering::Less => "slower",
                    std::cmp::Ordering::Greater => "faster",
                    std::cmp::Ordering::Equal => "the same",
                };
                tx.log(
                    Direction::Note,
                    format!(
                        "retrained: {now} bit/s, {note} ({} so far)",
                        modem.retrains()
                    ),
                );
            }
            was_retraining = retraining;
        }

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

        let undecodable = modem.undecodable_streams();
        if undecodable > last_undecodable {
            last_undecodable = undecodable;
            tx.log(
                Direction::Note,
                "V.42bis: what arrived would not decode, so the link is being put down"
                    .to_owned(),
            );
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
                        modem.error_control_detail(),
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
                f.distant.clear();
                f.distant.extend(modem.distant());
                f.bit_rate = rate;
                f.rx_bytes = rx_bytes;
                f.tx_bytes = tx_bytes;
                f.echo_loss_db = modem.echo_return_loss_now();
                f.reception = modem.reception();
                f.fax_class = modem.is_fax_class();
                if let Some(call) = modem.fax_call() {
                    f.fax_phase = Some(call.phase().name());
                    f.fax_identity = call.identity().to_owned();
                    f.fax_non_standard = call.non_standard().map(<[u8]>::to_vec);
                    f.fax_capabilities = call.capability_field().map(<[u8]>::to_vec);
                    f.fax_progress = call.progress();
                    f.fax_rate = call.rate();
                    f.fax_lines = call.lines_received();
                    f.fax_error_correction = call.error_correction();
                    f.fax_coding = call.coding().name();
                    f.fax_sending = call.role() == fax::call::Role::Caller;
                    f.fax_trouble = call.trouble().map(str::to_owned);
                }
                f.echo_at = modem.reflection().map(|r| (r.delay, r.strength));
                f.tones = modem.states();
                f.constellation_peak = modem.constellation_peak();
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
                state.tx_rms = tx_rms;
                state.rx_rms = rx_rms;
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

    // The line is closing with a recording still running, which is what
    // happens when somebody shuts the window on a call rather than pressing
    // stop first. Writing it out here is the difference between having the
    // call and not: it is over, it is the one that was worth keeping, and the
    // obvious thing to do next is close the window.
    if was_recording {
        keep(&recording, &frames, &tx, &session);
    }
}

/// The two levels, when comparing them means anything.
///
/// Both or neither, and that is the whole of it. These two numbers exist to be
/// divided by each other, and averaging them over different stretches of time
/// makes the quotient a comparison of two different moments.
///
/// Gated separately, which is how this was written, the reading drifts on its
/// own after a call ends: a far end that has hung up leaves line noise at
/// sixty decibels down, which still clears any threshold worth having, so its
/// average walks toward the floor -- while this end stops transmitting exactly
/// and freezes at its last real value. The gap grows with nothing behind it.
/// A recorded call where the two ends were within half a decibel of each other
/// was being shown as +11.6 dB, and the drive was being set by it.
fn both_carrying(tx: f32, rx: f32) -> Option<(f32, f32)> {
    const CARRYING: f32 = 1.0e-4;
    (tx > CARRYING && rx > CARRYING).then_some((tx, rx))
}

/// One frame, as a line of a log: when, which way, and every octet of it.
///
/// The address and control are left in. Naming them here would mean decoding
/// them twice, and the frames worth reading in this file are the ones that did
/// not decode -- so what it holds is what arrived, and the reading is done by
/// whoever opens it.
fn frame_line(at: f64, f: &ec::stack::Crossed) -> String {
    let way = match (f.outbound, f.intact) {
        (true, _) => "tx",
        (false, true) => "rx",
        (false, false) => "!!",
    };
    let hex: String =
        f.body.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let text: String = f
        .body
        .iter()
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
        .collect();
    format!("{at:9.3}  {way}  {:3}  {hex}  |{text}|", f.body.len())
}

/// Write a recording out and say so, wherever the decision to keep it was made.
fn keep(
    recording: &[f32],
    frames: &[String],
    tx: &Publisher,
    session: &Arc<Session>,
) {
    if recording.is_empty() {
        return;
    }
    let seconds = recording.len() as f64 / 2.0 / FS;
    match save(recording) {
        Ok(path) => {
            tx.log(Direction::Note, format!("kept {seconds:.1} s as {path}"));
            if !frames.is_empty() {
                let beside = format!("{path}.frames.txt");
                let head = "        s  way  len  frame
";
                let body: String = frames.join("
");
                match std::fs::write(&beside, format!("{head}{body}
")) {
                    Ok(()) => tx.log(
                        Direction::Note,
                        format!("{} frames as {beside}", frames.len()),
                    ),
                    Err(e) => tx.log(
                        Direction::Note,
                        format!("could not write the frames: {e}"),
                    ),
                }
            }
            if let Ok(mut state) = session.state.lock() {
                state.recorded_to = Some(path);
            }
        }
        Err(e) => tx.log(Direction::Note, format!("could not write it: {e}")),
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
    // Absolute, because "captures" is relative to wherever the program was
    // started from -- which since it became one file to hand somebody is the
    // directory that file sits in, and not the one the source is in. A
    // recording nobody can find is a recording that was not kept.
    Ok(std::fs::canonicalize(&path)
        .unwrap_or(path)
        .display()
        .to_string()
        .trim_start_matches(r"\?\")
        .to_owned())
}

/// Hand the terminal everything the modem has to say.
/// Read a file and describe it, for a ZFILE frame.
fn read_to_send(path: &std::path::Path) -> Result<(transfer::zmodem::FileInfo, Vec<u8>), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    // Clause 13's modification date: seconds since 1970 UTC, and 0 where it is
    // not known -- which the far end is told to read as "the date it arrived".
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let length = Some(data.len() as u64);
    Ok((transfer::zmodem::FileInfo { name, length, modified, mode: 0 }, data))
}

/// One round of a transfer: what it wants to say, and where it has got to.
///
/// Returns the view for the window and whether the job is over.
fn step_job(job: &mut Job, tx: &Publisher, room: usize) -> (Vec<u8>, (TransferView, bool)) {
    use transfer::zmodem::State;
    let (out, mut view, over) = match job {
        Job::Sending(s) => {
            s.tick(TICK_MS);
            s.set_room(room);
            let p = s.progress();
            let state = s.state();
            (
                s.take_out(),
                TransferView {
                    sending: true,
                    name: p.name,
                    position: p.position,
                    total: p.total,
                    rewinds: p.rewinds,
                    resent: p.resent,
                    damaged: 0,
                    outcome: describe(state),
                    finished: matches!(state, State::Done | State::Failed(_)),
                    written_to: None,
                    ..TransferView::default()
                },
                matches!(state, State::Done | State::Failed(_)),
            )
        }
        Job::Receiving(r, into) => {
            r.tick(TICK_MS);
            let p = r.progress();
            let state = r.state();
            let mut written = None;
            if let Some(got) = r.finished() {
                // 8.2 leaves the name to the receiver's judgement, and a board
                // is not a trusted party: `safe_name` is what keeps a
                // directory traversal out of the file system.
                let path = into.join(got.file.safe_name());
                let _ = std::fs::create_dir_all(into);
                match std::fs::write(&path, &got.data) {
                    Ok(()) => {
                        let shown = std::fs::canonicalize(&path)
                            .unwrap_or(path)
                            .display()
                            .to_string()
                            .trim_start_matches(LONG_PATH)
                            .to_owned();
                        tx.log(
                            Direction::Note,
                            format!("kept {} bytes as {shown}", got.data.len()),
                        );
                        written = Some(shown);
                    }
                    Err(e) => tx.log(Direction::Note, format!("could not write it: {e}")),
                }
            }
            (
                r.take_out(),
                TransferView {
                    sending: false,
                    name: p.name,
                    position: p.position,
                    total: p.total,
                    rewinds: p.rewinds,
                    resent: 0,
                    damaged: r.damaged(),
                    outcome: describe(state),
                    finished: matches!(state, State::Done | State::Failed(_)),
                    written_to: written,
                    ..TransferView::default()
                },
                matches!(state, State::Done | State::Failed(_)),
            )
        }
    };
    if over && view.outcome.is_empty() {
        view.outcome = "over".to_owned();
    }
    (out, (view, over))
}

/// What to show for a state, in words rather than in its own terms.
fn describe(state: transfer::zmodem::State) -> String {
    use transfer::zmodem::send::Failure;
    use transfer::zmodem::State;
    match state {
        State::Greeting => "starting".to_owned(),
        State::Offering => "offering the file".to_owned(),
        State::Sending => String::new(),
        State::Finishing => "finishing".to_owned(),
        State::Done => "done".to_owned(),
        State::Failed(Failure::NoAnswer) => "the far end never answered".to_owned(),
        State::Failed(Failure::Cancelled) => "cancelled".to_owned(),
        State::Failed(Failure::Skipped) => "the far end did not want it".to_owned(),
        State::Failed(Failure::FarEndError) => "the far end could not write it".to_owned(),
    }
}

/// Windows' own prefix on a canonical path, which nobody wants to read.
const LONG_PATH: &str = r"\\?\";

/// How long a round of the loop is worth calling, for the protocol's timers.
///
/// The loop turns over on audio arriving rather than on a clock, and a
/// millisecond a round is near enough at the block sizes involved.
const TICK_MS: u32 = 1;

/// Everything the modem has to say, and who it is for.
///
/// A transfer or a PPP link takes the stream while it runs; otherwise it goes
/// to the screen. The two cannot both be running, so the order they are tried
/// in here decides nothing.
fn drain_dte(
    modem: &mut Modem,
    tx: &Publisher,
    rx_bytes: &mut u64,
    heard: &mut Instant,
    job: Option<&mut Job>,
    network: Option<&mut Networking>,
    login: Option<&mut Login>,
) {
    let out = modem.take_dte();
    if out.is_empty() {
        return;
    }
    *rx_bytes += out.len() as u64;
    *heard = Instant::now();
    match (job, network, login) {
        (Some(Job::Sending(s)), _, _) => s.feed(&out),
        (Some(Job::Receiving(r, _)), _, _) => r.feed(&out),
        (None, Some(link), _) => link.feed(&out),
        (None, None, Some(running)) => running.feed(&out, tx),
        (None, None, None) => tx.line_data(&out),
    }
}

/// Bring PPP up over the call, with whatever authentication the window's
/// settings and the way the call started call for.
///
/// `served` is Some for a caller who came in through the login prompt: with a
/// name in it if they logged in there, which is enough, and None inside if
/// they went straight to PPP, which then has to ask. Otherwise an answering
/// end asks only if the window says calls are answered with a login, and a
/// calling end offers the account to a far end that asks.
fn start_link(
    modem: &Modem,
    session: &Session,
    early: Vec<u8>,
    served: Option<Option<String>>,
    tx: &Publisher,
) -> Networking {
    let settings = session.dialin();
    let role = modem.role();
    let asks = match &served {
        Some(user) => user.is_none(),
        None => role == Role::Answering && settings.serve,
    };
    let authentication = ppp::link::Authentication {
        account: (!settings.account.name.is_empty()).then(|| settings.account.clone()),
        callers: asks.then(|| settings.callers()),
        name: "binmodem".to_owned(),
        seed: crate::dialin::challenge_seed(),
    };
    let mut link = Networking::start(role, authentication, tx);
    if session.carry_web.load(Ordering::Relaxed) {
        link.carry_web(true, tx);
    }
    if !early.is_empty() {
        link.feed(&early);
    }
    link
}


#[cfg(test)]
mod level_tests {
    use super::both_carrying;

    /// A comparison of two averages taken over different moments is not a
    /// comparison, and the one place it shows is after a call.
    #[test]
    fn the_levels_are_compared_only_where_both_are_there() {
        // Both talking: the ordinary case, and the only one worth averaging.
        assert_eq!(both_carrying(0.05, 0.04), Some((0.05, 0.04)));
        // The far end has hung up and left the line hissing. Sixty decibels
        // down is still above any threshold, and following it alone is what
        // made the reading drift.
        assert_eq!(both_carrying(0.05, 0.0), None);
        assert_eq!(both_carrying(0.0, 0.04), None);
        assert_eq!(both_carrying(0.0, 0.0), None);
    }
}

#[cfg(test)]
mod arriving_tests {
    use super::{Arriving, Session};
    use fax::page::Resolution;

    fn batch(page: u64, from: usize, count: usize) -> Arriving {
        Arriving {
            page,
            resolution: Resolution::Standard,
            from,
            lines: (from..from + count).map(|y| vec![y % 2 == 0; 4]).collect(),
        }
    }

    #[test]
    fn lines_the_window_has_not_taken_yet_are_kept_together() {
        let session = Session::default();
        session.push_fax_lines(batch(0, 0, 3));
        session.push_fax_lines(batch(0, 3, 2));
        assert_eq!(session.take_fax_lines(), Some(batch(0, 0, 5)));
        assert_eq!(session.take_fax_lines(), None, "taken twice");
    }

    #[test]
    fn a_new_page_replaces_what_was_waiting_of_the_last_one() {
        // A window that fell behind wants the page arriving, not the last.
        let session = Session::default();
        session.push_fax_lines(batch(0, 40, 3));
        session.push_fax_lines(batch(1, 0, 2));
        assert_eq!(session.take_fax_lines(), Some(batch(1, 0, 2)));
    }
}
