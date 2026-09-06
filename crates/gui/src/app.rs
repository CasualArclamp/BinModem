//! The scope window.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, FontId, RichText};
use line::{AudioSink, Monitor};
use telemetry::{Direction, Frame, LogEntry, Subscriber};

use crate::console::{self, Console, Mode};
use crate::engine::{Control, FFT_SIZE, SCOPE_LEN, SPECTRUM_BINS};
use crate::live;
use crate::net;
use crate::scopes::{self, Waterfall};

/// Where what is on the scope comes from.
pub enum Source {
    /// A recording of a call someone else placed. It can be watched, paused
    /// and slowed down, and nothing typed at it can have any effect.
    Capture,
    /// A modem of our own on a real line. The terminal below is its DTE: what
    /// is typed goes to the modem, and the modem answers for itself.
    Live(Arc<live::Session>),
    /// A board over a socket, with no modem and no line anywhere in it. For
    /// working on the terminal itself: every byte a board sends arrives
    /// intact, so anything that draws wrongly is the terminal's fault and
    /// nothing else's.
    Telnet(Arc<net::Session>),
}

impl Source {
    fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    fn is_telnet(&self) -> bool {
        matches!(self, Self::Telnet(_))
    }

    /// Whether something at the far end owns the state.
    ///
    /// True of both a call and a socket, and the distinction that matters to
    /// the console: with a far end, what arrives is drawn exactly as it
    /// arrives and nothing on this side interprets it. A capture has no far
    /// end, so this side has to play one.
    fn is_line(&self) -> bool {
        !matches!(self, Self::Capture)
    }

    /// Send bytes to whatever is at the far end, if anything is.
    fn send(&self, bytes: &[u8]) {
        match self {
            Self::Live(session) => session.type_bytes(bytes),
            Self::Telnet(session) => session.type_bytes(bytes),
            Self::Capture => {}
        }
    }
}

/// The subparameters of `AT+MS`, as V.250 6.4.1 defines them.
///
/// The command carries four things: which modulation, whether the modem may
/// fall back to another on its own, and the range of line rates it is allowed
/// to use. The last two are the ones worth having a window for.
///
/// A rate ceiling is not a speed limit for the timid. The sixteen points of
/// V.22bis at 2400 need something like 20 dB of signal to noise to be told
/// apart, and the four at 1200 need about 13. On a line that cannot give the
/// first, 2400 is not the faster connection -- it is the one that carries
/// nothing, byte after byte of it, while 1200 would have carried the call.
/// Measured on a recorded call through a real trunk, 2400 got the far end
/// nought times in eight and 1200 got it eight.
#[derive(Debug, Clone, Copy)]
struct Modulation {
    /// Whether the modem may choose a different modulation than the one asked
    /// for. The Recommendation defaults this on.
    automode: bool,
    min_rate: u32,
    max_rate: u32,
}

impl Default for Modulation {
    fn default() -> Self {
        // V.250 6.4.1: automode on, and both rates unspecified. Zero is not a
        // rate -- "if unspecified (set to 0), they are determined by the
        // modulation means selected" -- so this is the widest range there is,
        // and `fit` turns it into the chosen modulation's own the moment the
        // window opens.
        Self { automode: true, min_rate: 0, max_rate: 0 }
    }
}

impl Modulation {
    /// The line rates a modulation actually has.
    ///
    /// This is what makes the window worth opening rather than typing the
    /// command: the rates are not free numbers, they belong to the modulation,
    /// and asking Bell 103 for 2400 is not a slow connection but an error.
    fn rates(carrier: usize) -> &'static [u32] {
        match carrier {
            0 => &[300],
            1 => &[1200, 2400],
            _ => &[4800, 9600],
        }
    }

    /// Move the range inside what this modulation can do.
    ///
    /// Called whenever the modulation changes, so the boxes can never be left
    /// showing a rate the chosen modulation has never heard of.
    fn fit(&mut self, carrier: usize) {
        let rates = Self::rates(carrier);
        let (lowest, highest) = (rates[0], rates[rates.len() - 1]);
        // Membership first, and no clamping to the nearest. A rate the new
        // modulation does not have says nothing about what was wanted, so the
        // answer is its widest range rather than whichever of its numbers the
        // old one happened to be closest to -- otherwise stepping through
        // Bell 103 on the way to V.32 would leave V.32 held to 4800 by a
        // 300 nobody meant as a ceiling.
        if !rates.contains(&self.min_rate) {
            self.min_rate = lowest;
        }
        if !rates.contains(&self.max_rate) {
            self.max_rate = highest;
        }
        // 5.4.2 makes a minimum above the maximum an error, so it is not
        // something to let the window compose in the first place.
        if self.min_rate > self.max_rate {
            self.min_rate = self.max_rate;
        }
    }

    /// The command this composes.
    ///
    /// The rates are left off when nobody has chosen any. V.250 6.4.1 makes an
    /// omitted rate unspecified -- "determined by the modulation means
    /// selected" -- and that is not the same as naming the chosen modulation's
    /// own range. Naming V.22bis's rates would hold a negotiation to V.22bis,
    /// which is the opposite of what a terminal that has not asked for a
    /// ceiling wants.
    fn command(&self, carrier: &str) -> String {
        let automode = u8::from(self.automode);
        if self.min_rate == 0 && self.max_rate == 0 {
            return format!("AT+MS={carrier},{automode}");
        }
        format!(
            "AT+MS={carrier},{automode},{},{}",
            self.min_rate, self.max_rate
        )
    }
}

/// Which view fills the lower panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Terminal,
    Transcript,
}

const WATERFALL_W: usize = 720;
const WATERFALL_H: usize = 260;
const PANEL_W: f32 = 280.0;

pub struct ScopeApp {
    rx: Subscriber,
    control: Arc<Control>,
    frame: Frame,
    waterfall: Waterfall,
    log: Vec<LogEntry>,
    last_seq: u64,
    /// Sequence of the newest transcript line known to be finished. Anything
    /// after it may still be growing and is re-read each frame.
    frozen_seq: u64,
    follow_log: bool,
    // Monitoring. The cpal stream is not Send on Windows, so it has to live on
    // the thread that created it: this one.
    sink: Arc<AudioSink>,
    monitor: Option<Monitor>,
    devices: Vec<String>,
    chosen_device: usize,
    audio_error: Option<String>,
    sample_rate: f64,
    console: Console,
    source: Source,
    /// The line side of a live call: which devices are picked in the boxes,
    /// and which modulation the next call will use.
    line_inputs: Vec<String>,
    line_outputs: Vec<String>,
    chosen_input: usize,
    chosen_output: usize,
    carrier: usize,
    /// The `AT+MS` subparameters the advanced window is composing, and whether
    /// it is open.
    modulation: Modulation,
    advanced: bool,
    /// Where a telnet connection is aimed.
    host: String,
    tab: Tab,
    font_size: f32,
    last_repaint: std::time::Instant,
}

impl ScopeApp {
    pub fn new(
        rx: Subscriber,
        control: Arc<Control>,
        sink: Arc<AudioSink>,
        sample_rate: f64,
        source: Source,
    ) -> Self {
        let inputs = line::input_devices();
        let outputs = line::output_devices();
        // Two cables if there are two, and the right way round.
        //
        // One cable is a two-wire line: everything written to it comes back,
        // so a modem on one hears its own transmission at full strength. That
        // is a fine model of a telephone pair with two modems across it and
        // useless for reaching anything outside the machine, where what is
        // wanted is a hybrid and there is none. Two cables are the hybrid: the
        // far end's audio arrives on one and ours leaves on the other, and
        // neither modem ever hears itself.
        //
        // So A carries what the softphone plays, and B carries what this modem
        // says. Named first because a machine with A and B has usually got
        // them for this, and the plain names are what a single-cable
        // installation offers.
        let pick = |names: &[String], wanted: &[&str]| {
            wanted
                .iter()
                .find_map(|want| names.iter().position(|n| n.contains(want)))
                .unwrap_or(0)
        };
        let chosen_in = pick(&inputs, &["CABLE-A Output", "CABLE Output"]);
        let chosen_out = pick(&outputs, &["CABLE-B Input", "CABLE Input"]);
        Self {
            rx,
            control,
            frame: Frame::new(SCOPE_LEN, SPECTRUM_BINS, sample_rate),
            waterfall: Waterfall::new(WATERFALL_W, WATERFALL_H),
            log: Vec::new(),
            last_seq: 0,
            frozen_seq: 0,
            follow_log: true,
            sink,
            monitor: None,
            devices: line::output_devices(),
            chosen_device: 0,
            audio_error: None,
            sample_rate,
            // A live console is a dumb terminal onto a modem that
            // answers for itself; a capture console has to pretend to
            // be one, so they open with different things to say.
            console: match source {
                Source::Live(_) => Console::live(),
                Source::Telnet(_) => Console::telnet(),
                Source::Capture => Console::new(),
            },
            host: Self::BOARDS[0].to_owned(),
            source,
            line_inputs: inputs,
            line_outputs: outputs,
            // A virtual cable is almost always the right answer, so it starts
            // selected where there is one.
            chosen_input: chosen_in,
            chosen_output: chosen_out,
            carrier: 1,
            modulation: Modulation::default(),
            advanced: false,
            tab: Tab::Terminal,
            font_size: 14.0,
            last_repaint: std::time::Instant::now(),
        }
    }

    /// Carry out what the AT layer asked for.
    fn perform(&mut self, actions: Vec<at::Action>) {
        for action in actions {
            match action {
                at::Action::Dial(number) => {
                    // A dial here replays the capture: what this window is for
                    // is looking at a recording of a call, so the far end is
                    // the recording. Placing a real one is the `modem` crate's
                    // business and wants a line to place it down.
                    self.control.restart.store(true, Ordering::Relaxed);
                    self.control.running.store(true, Ordering::Relaxed);
                    self.console.connect("300");
                    self.console
                        .term
                        .feed_bytes(format!("[replaying capture for {number}]
").as_bytes());
                }
                at::Action::Answer => {
                    self.control.restart.store(true, Ordering::Relaxed);
                    self.control.running.store(true, Ordering::Relaxed);
                    self.console.connect("300");
                }
                at::Action::HangUp => {
                    self.control.running.store(false, Ordering::Relaxed);
                    if self.console.mode == Mode::Online {
                        self.console.disconnect(at::result::ResultCode::NoCarrier);
                    }
                }
                at::Action::ReturnOnline => self.console.resume_online(),
                // Settings that apply to the next call rather than this one.
                // The interpreter has already recorded them; the scope has no
                // call of its own to apply them to, since what it is looking at
                // is a recording of somebody else's.
                at::Action::SelectModulation(_)
                | at::Action::SelectErrorControl(_)
                | at::Action::SelectCompression(_)
                | at::Action::OffHook
                | at::Action::ResetProfile(_)
                | at::Action::FactoryDefaults(_) => {}
            }
        }
    }

    fn terminal_pane(&mut self, ui: &mut egui::Ui) {
        let view = console::view(ui, &self.console.term, self.font_size);
        let response = view.response;
        if response.clicked() {
            response.request_focus();
        }
        // Straight into the terminal, which knows which of these the far end
        // asked for and drops the rest. What it decides to report joins the
        // answerback in the same queue and goes out by the same route, so a
        // board hears about the mouse over a call exactly as it does over a
        // socket.
        for event in view.mouse {
            self.console.term.mouse(event);
        }
        if response.has_focus() {
            let typed = console::keys_to_bytes(ui);
            if !typed.is_empty() {
                match &self.source {
                    // Straight to the modem, which has an AT interpreter of
                    // its own and will echo, answer, and decide for itself
                    // what is a command and what is data. Nothing is parsed
                    // on this side of the line.
                    // Straight to the modem, which has an AT interpreter of
                    // its own and will echo, answer, and decide for itself
                    // what is a command and what is data. Nothing is parsed
                    // on this side of the line.
                    Source::Live(_) => self.source.send(&typed),
                    Source::Telnet(session) => {
                        let session = std::sync::Arc::clone(session);
                        session.type_bytes(&typed);
                        // RFC 857: until the far end says it will echo, this
                        // end has to, or typing goes into a screen that never
                        // changes. Boards almost always do, so this is the
                        // path taken for the first moment of a connection and
                        // then not again -- but that moment is the login
                        // prompt, and a login prompt that swallows what is
                        // typed at it looks exactly like a dead connection.
                        if !session.state().echo {
                            for &b in &typed {
                                // A bare return leaves the cursor on the same
                                // line, so echoing one verbatim would draw
                                // every line of typing over the last.
                                if b == b'\r' {
                                    self.console.term.feed_bytes(b"\r\n");
                                } else {
                                    self.console.term.feed(b);
                                }
                            }
                        }
                    }
                    Source::Capture => {
                        let actions = self.console.typed(&typed);
                        self.perform(actions);
                    }
                }
            }
        }
    }

    /// Start or stop monitoring on the selected device.
    fn set_listening(&mut self, on: bool) {
        self.audio_error = None;
        if !on {
            // Dropping the stream stops it; clearing the sink discards audio
            // queued but never played.
            self.monitor = None;
            self.sink.set_enabled(false);
            return;
        }
        let device = self.devices.get(self.chosen_device).map(String::as_str);
        // Enable before opening, so the callback finds samples waiting rather
        // than starting on an empty buffer.
        self.sink.set_enabled(true);
        match line::listen(self.sink.clone(), device, self.sample_rate) {
            Ok(monitor) => self.monitor = Some(monitor),
            Err(e) => {
                self.sink.set_enabled(false);
                self.audio_error = Some(e);
            }
        }
    }

    /// Modulations the modem will accept, in the order the box shows them.
    const CARRIERS: [(&'static str, &'static str); 3] = [
        ("B103", "Bell 103 - 300 bit/s"),
        ("V22B", "V.22bis - 1200 or 2400"),
        ("V32", "V.32 - 4800 or 9600"),
    ];

    /// Boards to start from, because a text box on its own is a box nobody
    /// can type an answer into.
    ///
    /// These rot. Boards move, change port and close, and none of that is
    /// worth pinning a build to -- which is why the box beside them is
    /// editable and is the real interface. The first is Synchronet's own
    /// board, which is as close to a reference target as this has: it is run
    /// by the author of the software a great many of the surviving boards run
    /// on, and it answers with a great deal of ANSI.
    const BOARDS: [&'static str; 5] = [
        "vert.synchro.net",
        "blackflag.acid.org",
        "xibalba.l33t.codes:44510",
        "bbs.fozztexx.com",
        "heatwavebbs.com",
    ];

    /// Choosing a board, and connecting to it.
    fn net_controls(&mut self, ui: &mut egui::Ui) {
        let Source::Telnet(session) = &self.source else { return };
        let session = Arc::clone(session);
        let state = session.state();
        let dim = Color32::from_rgb(140, 150, 165);

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("host").monospace().color(dim));
            let editable = !state.connected;
            let entry = ui.add_enabled(
                editable,
                egui::TextEdit::singleline(&mut self.host)
                    .desired_width(230.0)
                    .hint_text("host or host:port"),
            );
            // Enter connects, because a box you have just typed an address
            // into and then have to go and find a button for is a box that
            // gets typed into twice.
            let entered = editable
                && entry.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));

            egui::ComboBox::from_id_salt("boards")
                .selected_text("...")
                .width(34.0)
                .show_ui(ui, |ui| {
                    for board in Self::BOARDS {
                        if ui.selectable_label(self.host == board, board).clicked() {
                            self.host = board.to_owned();
                        }
                    }
                });

            if state.connected {
                if ui.button("Disconnect").clicked() {
                    session.disconnect();
                }
            } else if (ui.button("Connect").clicked() || entered)
                && !self.host.trim().is_empty()
            {
                // A board draws its opening screen over whatever was there,
                // so start it on a clean one rather than on the last one.
                self.console.term.reset();
                self.console.term.clear_scrollback();
                session.connect(&self.host);
            }

            ui.separator();
            if state.connected {
                ui.label(
                    RichText::new(format!("connected to {}", state.peer))
                        .monospace()
                        .color(Color32::from_rgb(90, 220, 130)),
                );
                // The two options that decide whether anything looks right.
                // Without eight-bit data the art loses its top bits and every
                // box is drawn out of question marks; without the far end
                // echoing, nothing typed appears at all.
                let flag = |on: bool, yes: &str, no: &str| {
                    if on {
                        RichText::new(yes.to_owned()).monospace().color(dim)
                    } else {
                        RichText::new(no.to_owned())
                            .monospace()
                            .color(Color32::from_rgb(240, 180, 90))
                    }
                };
                ui.label(flag(state.binary, "8-bit", "7-bit!"));
                ui.label(flag(state.echo, "remote echo", "local echo"));
            } else if let Some(e) = &state.error {
                ui.label(RichText::new(e).monospace().color(Color32::from_rgb(240, 120, 120)));
            } else {
                ui.label(RichText::new("not connected").monospace().color(dim));
            }

            ui.separator();
            // The whole reason this mode exists: what arrived, beside what it
            // drew. Off by default because an opening screen is thousands of
            // bytes and would bury every notice in the transcript.
            let mut logging = session.logging();
            if ui
                .checkbox(&mut logging, "log bytes")
                .on_hover_text("put everything the board sends in the transcript as well")
                .changed()
            {
                session.set_logging(logging);
            }
            ui.add(egui::Slider::new(&mut self.font_size, 9.0..=22.0).text("font"));
        });
    }

    /// Choosing the line, and driving the call on it.
    ///
    /// The buttons do nothing the keyboard could not: each one types the
    /// command it is named after. That is not a shortcut taken, it is the only
    /// honest way to build them — the modem has one interface, and a button
    /// that reached past it into the state machine would be able to ask for
    /// things a terminal could not, and would drift from what the terminal
    /// sees the moment either changed.
    fn line_controls(&mut self, ui: &mut egui::Ui) {
        let Source::Live(session) = &self.source else { return };
        let session = Arc::clone(session);
        let state = session.state();

        // Follow the line rather than the boxes. A line opened from the
        // command line was never chosen here, and a box showing something
        // other than what is open is a box that will reopen the wrong device
        // the moment anything else on this row is touched.
        if state.open {
            if let Some(i) = self.line_inputs.iter().position(|n| *n == state.input) {
                self.chosen_input = i;
            }
            if let Some(i) = self.line_outputs.iter().position(|n| *n == state.output) {
                self.chosen_output = i;
            }
        }

        ui.horizontal_wrapped(|ui| {
            let dim = Color32::from_rgb(140, 150, 165);
            ui.label(RichText::new("line").monospace().color(dim));

            let before = (self.chosen_input, self.chosen_output);
            egui::ComboBox::from_id_salt("line-input")
                .width(230.0)
                .selected_text(
                    self.line_inputs
                        .get(self.chosen_input)
                        .map(String::as_str)
                        .unwrap_or("no input devices"),
                )
                .show_ui(ui, |ui| {
                    for (i, name) in self.line_inputs.iter().enumerate() {
                        ui.selectable_value(&mut self.chosen_input, i, name);
                    }
                });
            egui::ComboBox::from_id_salt("line-output")
                .width(230.0)
                .selected_text(
                    self.line_outputs
                        .get(self.chosen_output)
                        .map(String::as_str)
                        .unwrap_or("no output devices"),
                )
                .show_ui(ui, |ui| {
                    for (i, name) in self.line_outputs.iter().enumerate() {
                        ui.selectable_value(&mut self.chosen_output, i, name);
                    }
                });

            let picked = (self.chosen_input, self.chosen_output);
            let have_both =
                !self.line_inputs.is_empty() && !self.line_outputs.is_empty();
            // Changing a device while the line is open moves the call onto the
            // new one, which is what picking it means.
            if picked != before && state.open && have_both {
                session.open(
                    &self.line_inputs[self.chosen_input],
                    &self.line_outputs[self.chosen_output],
                );
            }

            if state.open {
                if ui.button("Close").on_hover_text("Put the line down").clicked() {
                    session.close();
                }
                // Both directions, kept apart. What makes a call worth
                // keeping is usually not obvious until it has gone wrong.
                let recording = session.recording();
                let label = match state.recording {
                    Some(secs) => format!("Stop  {secs:.0} s"),
                    None => "Record".to_owned(),
                };
                if ui
                    .selectable_label(recording, label)
                    .on_hover_text(
                        "Keep the call as a stereo file: what arrived on one                          channel, what was sent on the other, so it can be run                          through a receiver again afterwards",
                    )
                    .clicked()
                {
                    session.set_recording(!recording);
                }
            } else if ui
                .add_enabled(have_both, egui::Button::new("Open"))
                .on_hover_text("Open these two devices as one two-wire line")
                .clicked()
            {
                session.open(
                    &self.line_inputs[self.chosen_input],
                    &self.line_outputs[self.chosen_output],
                );
            }

            if state.open {
                ui.label(
                    RichText::new(format!("{} / {} Hz", state.input_rate, state.output_rate))
                        .monospace()
                        .color(dim),
                );
                if state.underruns > 0 {
                    ui.label(
                        RichText::new(format!("{} gaps sent", state.underruns))
                            .monospace()
                            .color(Color32::from_rgb(235, 100, 90)),
                    )
                    .on_hover_text(
                        "Times the line had nothing to send and sent silence.                          The far end hears a dropout",
                    );
                }
                if state.framing_errors > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} bad frames ({:.0}/s)",
                            state.framing_errors, state.framing_errors_per_second
                        ))
                        .monospace()
                        .color(Color32::from_rgb(230, 180, 90)),
                    )
                    .on_hover_text(
                        "Characters whose stop bit was in the wrong place. A few                          a second is noise on the line; dozens at once with quiet                          in between is a network dropping packets, which only                          error control hides",
                    );
                }
                if state.dropped > 0 {
                    // Not a warning to be dismissed. Timing recovery cannot
                    // know a sample went missing and reads the gap as the
                    // clock having moved.
                    ui.label(
                        RichText::new(format!("{} samples lost", state.dropped))
                            .monospace()
                            .color(Color32::from_rgb(235, 100, 90)),
                    );
                }
            }
            if let Some(err) = &state.error {
                ui.label(RichText::new(err).color(Color32::from_rgb(235, 100, 90)));
            }
            if let Some(path) = &state.recorded_to {
                ui.label(
                    RichText::new(path)
                        .monospace()
                        .color(Color32::from_rgb(120, 200, 150)),
                );
            }

            // Transmit level. On a real line this is not decoration: too low
            // and the far end cannot hear the modem over what the network
            // adds, too high and something in between clips or pulls its gain
            // control down over the whole call. In decibels because that is
            // how line levels are talked about everywhere else.
            let mut db = 20.0 * session.drive().max(1.0e-4).log10();
            if ui
                .add(
                    egui::Slider::new(&mut db, -30.0..=0.0)
                        .text("drive")
                        .suffix(" dB"),
                )
                .on_hover_text("How hard to drive the line, relative to what the modem hands over")
                .changed()
            {
                session.set_drive(10.0f32.powf(db / 20.0));
            }
            if state.open {
                // The number the slider is for. Above about a decibel down
                // the peaks are into the top of the scale and anything
                // digital between here and the far end will flatten them.
                let peak_db = 20.0 * state.tx_peak.max(1.0e-4).log10();
                let hot = state.tx_peak > 0.89;
                ui.label(
                    RichText::new(format!("peak {peak_db:>5.1} dBFS"))
                        .monospace()
                        .color(if hot {
                            Color32::from_rgb(235, 100, 90)
                        } else {
                            Color32::from_rgb(140, 150, 165)
                        }),
                )
                .on_hover_text(
                    "Loudest sample going out. Frequency shift keying sits at its \
                     peak permanently; a shaped constellation goes nearly three \
                     times above its own average, so the same drive is not the \
                     same peak",
                );
            }
        });

        ui.horizontal_wrapped(|ui| {
            let dim = Color32::from_rgb(140, 150, 165);
            ui.label(RichText::new("call").monospace().color(dim));

            // Watched by what was clicked rather than by what the value is
            // afterwards, so that the advanced window can set the same field
            // without this row deciding a command needs sending.
            let mut picked = None;
            egui::ComboBox::from_id_salt("carrier")
                .width(180.0)
                .selected_text(Self::CARRIERS[self.carrier].1)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in Self::CARRIERS.iter().enumerate() {
                        if ui.selectable_label(self.carrier == i, *label).clicked() {
                            picked = Some(i);
                        }
                    }
                });
            if let Some(i) = picked {
                self.carrier = i;
                self.modulation.fit(i);
                // Both ends have to agree: a modem listening for one of these
                // hears nothing whatever of the others. Bare, so the rates go
                // back to their defaults -- the advanced window is where a
                // range is chosen on purpose.
                session.type_bytes(
                    format!("AT+MS={}\r", Self::CARRIERS[i].0).as_bytes(),
                );
            }
            // V.250 6.4.1 makes this one setting, and it is the one that
            // belongs on the face of the window rather than behind a button:
            // with it on, the box to the left is where the call starts rather
            // than where it ends up.
            if ui
                .selectable_label(self.modulation.automode, "V.8")
                .on_hover_text(
                    "Negotiate the modulation with the far end before starting \
                     it. Both modems then enter the same one instead of each \
                     guessing, which is the one thing no modem start-up can \
                     arrange for itself. Off means the box to the left and \
                     nothing else",
                )
                .clicked()
            {
                // Not `fit`. This toggle is about whether to negotiate and
                // about nothing else: fitting would pin the range to the
                // modulation named beside it, and a range that names V.22bis's
                // rates is a range V.8 can never negotiate its way out of.
                self.modulation.automode = !self.modulation.automode;
                let command =
                    self.modulation.command(Self::CARRIERS[self.carrier].0);
                session.type_bytes(format!("{command}\r").as_bytes());
            }
            if ui
                .selectable_label(self.advanced, "Advanced")
                .on_hover_text("the rest of AT+MS: the range of line rates")
                .clicked()
            {
                self.advanced = !self.advanced;
                self.modulation.fit(self.carrier);
            }

            let online = self.frame.state == telemetry::CallState::Connected;
            let on_hook = self.frame.state == telemetry::CallState::Idle;

            if ui
                .add_enabled(on_hook, egui::Button::new("Originate"))
                .on_hover_text(
                    "ATD - be the calling modem. The softphone places the call; \
                     this only decides which end of it this is",
                )
                .clicked()
            {
                session.type_bytes(b"ATD\r");
            }
            if ui
                .add_enabled(on_hook, egui::Button::new("Answer"))
                .on_hover_text("ATA - be the answering modem, and go first")
                .clicked()
            {
                session.type_bytes(b"ATA\r");
            }
            // Two steps out of data state, and the button says which one it is
            // on. A modem in data state is not listening for commands at all:
            // the escape has to come first, and it wants a second of quiet
            // either side, so this is deliberately two clicks and not one.
            if online {
                if ui
                    .button("Escape")
                    .on_hover_text(
                        "+++ - back to command state without dropping the call. \
                         Wants a second of quiet either side, so give it a moment",
                    )
                    .clicked()
                {
                    session.type_bytes(b"+++");
                }
            } else if ui
                .add_enabled(!on_hook, egui::Button::new("Hang up"))
                .on_hover_text("ATH - put the line down")
                .clicked()
            {
                session.type_bytes(b"ATH\r");
            }
            ui.label(
                RichText::new(self.frame.state.label())
                    .monospace()
                    .color(if online {
                        Color32::from_rgb(90, 220, 130)
                    } else {
                        dim
                    }),
            );
            // Which start-up, and where inside it. A call that will not come
            // up is always stuck somewhere particular, and "negotiating" on
            // its own says nothing whatever about where.
            if !on_hook && self.frame.line_phase != "-" {
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        self.frame.modulation, self.frame.line_phase
                    ))
                    .monospace()
                    .color(Color32::from_rgb(120, 210, 255)),
                );
            }
        });

        self.advanced_modulation(ui, &session);
    }

    /// The rest of `AT+MS`, in a window rather than typed.
    ///
    /// Everything here composes one command and sends it. That is the same
    /// rule the buttons on the row above follow, and for the same reason: the
    /// modem has one interface, and a control that reached past it into the
    /// state machine could ask for things a terminal could not and would drift
    /// from what the terminal sees the moment either changed. The command being
    /// composed is on the face of the window, so nothing here is hidden.
    fn advanced_modulation(&mut self, ui: &mut egui::Ui, session: &Arc<live::Session>) {
        let dim = Color32::from_rgb(140, 150, 165);
        // Copied out because the window's own close button wants `&mut bool`
        // and so does everything inside it.
        let mut open = self.advanced;
        egui::Window::new("AT+MS - modulation")
            .open(&mut open)
            .resizable(false)
            .default_width(460.0)
            .show(ui.ctx(), |ui| {
                egui::Grid::new("ms")
                    .num_columns(2)
                    .spacing([14.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("modulation").monospace().color(dim));
                        ui.vertical(|ui| {
                            for (i, (name, label)) in Self::CARRIERS.iter().enumerate() {
                                if ui
                                    .radio(self.carrier == i, format!("{label}  ({name})"))
                                    .clicked()
                                {
                                    self.carrier = i;
                                    self.modulation.fit(i);
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(RichText::new("negotiate").monospace().color(dim));
                        ui.checkbox(
                            &mut self.modulation.automode,
                            "ask the far end first, and use what both have (V.8)",
                        )
                        .on_hover_text(
                            "V.250 6.4.1: automode enables or disables automatic modulation negotiation, e.g. ITU-T Rec. V.8. With it on, the two modems exchange call menus over V.21 and both enter the same modulation instead of each guessing. Off means the one chosen above and nothing else",
                        );
                        ui.end_row();

                        ui.label(RichText::new("line rate").monospace().color(dim));
                        ui.horizontal(|ui| {
                            // Only the rates this modulation has. They are not
                            // free numbers -- asking Bell 103 for 2400 is not a
                            // slow connection, it is an error, and V.250 5.4.2
                            // says a modem should refuse it.
                            let rates = Modulation::rates(self.carrier);
                            let before =
                                (self.modulation.min_rate, self.modulation.max_rate);
                            ui.label(RichText::new("from").color(dim));
                            rate_box(ui, "ms-min", &mut self.modulation.min_rate, rates);
                            ui.label(RichText::new("to").color(dim));
                            rate_box(ui, "ms-max", &mut self.modulation.max_rate, rates);
                            // Keep the pair the right way round by moving
                            // whichever one was not just touched.
                            if self.modulation.min_rate > self.modulation.max_rate {
                                if self.modulation.min_rate != before.0 {
                                    self.modulation.max_rate = self.modulation.min_rate;
                                } else {
                                    self.modulation.min_rate = self.modulation.max_rate;
                                }
                            }
                            if rates.len() == 1 {
                                ui.label(
                                    RichText::new("the only rate it has")
                                        .small()
                                        .color(dim),
                                );
                            }
                        });
                        ui.end_row();
                    });

                let rates = Modulation::rates(self.carrier);
                let top = rates[rates.len() - 1];
                ui.add_space(4.0);
                if self.modulation.max_rate < top {
                    ui.label(
                        RichText::new(format!(
                            "Held to {}. On a line that cannot carry {top}, that is not                              the slower connection -- it is the one that works.",
                            self.modulation.max_rate
                        ))
                        .small()
                        .color(Color32::from_rgb(240, 200, 120)),
                    );
                } else {
                    ui.label(
                        RichText::new(
                            "A ceiling is worth setting on purpose. Sixteen points at                              2400 need about 20 dB of signal to noise; four at 1200                              need about 13.",
                        )
                        .small()
                        .color(dim),
                    );
                }

                ui.separator();
                let command = self.modulation.command(Self::CARRIERS[self.carrier].0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&command)
                            .monospace()
                            .color(Color32::from_rgb(220, 225, 235)),
                    );
                    if ui
                        .button("Send")
                        .on_hover_text("Takes effect on the next call, not this one")
                        .clicked()
                    {
                        session.type_bytes(format!("{command}\r").as_bytes());
                    }
                    if ui
                        .button("Ask")
                        .on_hover_text("AT+MS? - what the modem currently has")
                        .clicked()
                    {
                        session.type_bytes(b"AT+MS?\r");
                    }
                });
            });
        self.advanced = open;
    }

    fn audio_controls(&mut self, ui: &mut egui::Ui) {
        let listening = self.monitor.is_some();
        if ui
            .selectable_label(listening, if listening { "Listening" } else { "Listen" })
            .on_hover_text("Play the line audio through an output device")
            .clicked()
        {
            self.set_listening(!listening);
        }

        let previous = self.chosen_device;
        egui::ComboBox::from_id_salt("output-device")
            .width(220.0)
            .selected_text(
                self.devices
                    .get(self.chosen_device)
                    .map(String::as_str)
                    .unwrap_or("no output devices"),
            )
            .show_ui(ui, |ui| {
                for (i, name) in self.devices.iter().enumerate() {
                    ui.selectable_value(&mut self.chosen_device, i, name);
                }
            });
        // Switching device while listening reopens the stream on the new one.
        if self.chosen_device != previous && self.monitor.is_some() {
            self.set_listening(false);
            self.set_listening(true);
        }

        if let Some(monitor) = &self.monitor {
            ui.label(
                RichText::new(format!("{} Hz", monitor.sample_rate))
                    .monospace()
                    .color(Color32::from_rgb(140, 150, 165)),
            );
        }

        // Nothing to do with the level on the line: this is how loud it is in
        // the room, and a handshake at full scale through headphones is
        // genuinely unpleasant.
        let mut volume = self.sink.volume();
        if ui
            .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("volume"))
            .on_hover_text("How loud the monitor plays. The line is not affected")
            .changed()
        {
            self.sink.set_volume(volume);
        }
        if let Some(err) = &self.audio_error {
            ui.label(RichText::new(err).color(Color32::from_rgb(235, 100, 90)));
        }
    }

    /// Pull whatever the engine has published since the last repaint.
    fn poll(&mut self) {
        if self.rx.read(&mut self.frame) && self.frame.seq != self.last_seq {
            self.last_seq = self.frame.seq;
            self.waterfall
                .push_row(&self.frame.spectrum_db, self.frame.hz_per_bin);
        }
        // Only the final line can still be growing, so re-read from there
        // rather than treating everything already copied as settled.
        let tail = self.rx.log_after(self.frozen_seq);
        self.log.retain(|e| e.seq <= self.frozen_seq);
        self.log.extend(tail);
        self.frozen_seq = match self.log.last() {
            Some(e) if e.complete => e.seq,
            _ if self.log.len() >= 2 => self.log[self.log.len() - 2].seq,
            _ => self.frozen_seq,
        };

        let data = self.rx.take_line_data();
        if self.source.is_line() {
            // Everything the modem says, whether that is an OK of its own or
            // a byte off the line. It keeps the command and online states and
            // runs its own escape timer, so this side only follows along far
            // enough to label which one it is in.
            if !data.is_empty() {
                self.console.feed_screen(&data);
            }
            // Some of what arrives is a question rather than something to
            // draw, and a board that asks one and hears nothing concludes it
            // is talking to a teletype. Only while there is a call, though:
            // in command state this would go to the AT interpreter, which
            // would rightly make nothing of it.
            if self.frame.state == telemetry::CallState::Connected {
                let reply = self.console.term.take_reply();
                if !reply.is_empty() {
                    self.source.send(&reply);
                }
            }
            self.console
                .follow(self.frame.state == telemetry::CallState::Connected);
            self.last_repaint = std::time::Instant::now();
            return;
        }

        // Everything the far end sent goes to the terminal verbatim.
        if !data.is_empty() {
            self.console.line_rx(&data);
        }

        // A capture has no far end to echo what is typed at it, so it is
        // echoed locally. Draining it also stops the queue growing without
        // bound.
        let outbound = self.console.take_tx();
        if !outbound.is_empty() {
            self.console.term.feed_bytes(&outbound);
        }

        // The escape sequence is timed, so the guard needs real elapsed time.
        let dt = self.last_repaint.elapsed().as_millis().min(1000) as u32;
        self.last_repaint = std::time::Instant::now();
        if self.console.idle(dt) {
            self.console.notice("[escaped to command state; ATO to resume]");
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        // The line and the call come first: on a live window they are the
        // controls that matter and the rest is instrumentation.
        self.line_controls(ui);
        if self.source.is_telnet() {
            // Nothing below is about a socket. There is no capture to pause,
            // no audio to monitor, and no spectrum to set a floor on.
            self.net_controls(ui);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            // A live line cannot be paused, restarted or slowed down. It is
            // happening, at the rate the sound card is happening at, and a
            // control that pretended otherwise would be lying about it.
            if self.source.is_live() {
                ui.label(
                    RichText::new("live line")
                        .monospace()
                        .color(Color32::from_rgb(90, 220, 130)),
                );
            } else {
                let running = self.control.running.load(Ordering::Relaxed);
                if ui.button(if running { "Pause" } else { "Play" }).clicked() {
                    self.control.running.store(!running, Ordering::Relaxed);
                }
                if ui.button("Restart").clicked() {
                    self.control.restart.store(true, Ordering::Relaxed);
                    self.control.running.store(true, Ordering::Relaxed);
                    self.log.clear();
                }

                ui.separator();
                let mut speed =
                    self.control.speed_pct.load(Ordering::Relaxed) as f32 / 100.0;
                if ui
                    .add(
                        egui::Slider::new(&mut speed, 0.1..=4.0)
                            .logarithmic(true)
                            .text("speed")
                            .suffix("x"),
                    )
                    .changed()
                {
                    self.control
                        .speed_pct
                        .store((speed * 100.0) as u32, Ordering::Relaxed);
                }
            }

            ui.separator();
            self.audio_controls(ui);

            ui.separator();
            ui.add(
                egui::Slider::new(&mut self.waterfall.floor_db, -140.0..=-40.0)
                    .text("floor")
                    .suffix(" dB"),
            );
            ui.add(
                egui::Slider::new(&mut self.waterfall.ceiling_db, -60.0..=0.0)
                    .text("ceiling")
                    .suffix(" dB"),
            );
        });
    }

    fn status(&self, ui: &mut egui::Ui) {
        let f = &self.frame;
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        egui::Grid::new("status")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                let mut row = |k: &str, v: String, colour: Color32| {
                    ui.label(RichText::new(k).monospace().color(dim));
                    ui.label(RichText::new(v).monospace().color(colour));
                    ui.end_row();
                };
                row("state", f.state.label().into(), bright);
                row("modulation", f.modulation.into(), bright);
                row("phase", f.line_phase.into(), dim);
                row(
                    "rate",
                    f.bit_rate.map(|r| format!("{r} bps")).unwrap_or_else(|| "-".into()),
                    bright,
                );
                row(
                    "carrier",
                    if f.carrier { "detected" } else { "none" }.into(),
                    if f.carrier { Color32::from_rgb(90, 220, 130) } else { dim },
                );
                row(
                    "quality",
                    f.symbol_quality().map(|q| q.to_string()).unwrap_or_else(|| "-".into()),
                    bright,
                );
                row("rx bytes", f.rx_bytes.to_string(), bright);
                row("tx bytes", f.tx_bytes.to_string(), bright);
                row("dropped", self.rx.dropped_frames().to_string(), dim);
                if self.monitor.is_some() {
                    row(
                        "audio u/o",
                        format!("{} / {}", self.sink.underruns(), self.sink.overruns()),
                        dim,
                    );
                }
            });
    }

    fn transcript(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("transcript").strong());
            ui.checkbox(&mut self.follow_log, "follow");
            if ui.small_button("clear").clicked() {
                self.log.clear();
                self.frozen_seq = self.rx.log_len() as u64;
            }
        });
        egui::ScrollArea::vertical()
            .stick_to_bottom(self.follow_log)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in &self.log {
                    let (tag, colour) = match entry.direction {
                        Direction::FromLine => ("RX", Color32::from_rgb(120, 210, 255)),
                        Direction::ToLine => ("TX", Color32::from_rgb(250, 200, 120)),
                        Direction::ToDce => ("DTE", Color32::from_rgb(180, 230, 150)),
                        Direction::ToDte => ("DCE", Color32::from_rgb(150, 200, 240)),
                        Direction::Note => ("--", Color32::from_rgb(140, 145, 160)),
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{:7.2}", entry.at.as_secs_f32()))
                                .font(FontId::monospace(11.0))
                                .color(Color32::from_rgb(110, 115, 130)),
                        );
                        ui.label(
                            RichText::new(format!("{tag:>3}"))
                                .font(FontId::monospace(11.0))
                                .color(colour),
                        );
                        ui.label(
                            RichText::new(&entry.text)
                                .font(FontId::monospace(12.0))
                                .color(Color32::from_rgb(215, 220, 230)),
                        );
                    });
                }
            });
    }

    /// The terminal and the transcript, and the strip that switches them.
    fn lower(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Terminal, "terminal");
            ui.selectable_value(&mut self.tab, Tab::Transcript, "transcript");
            ui.separator();
            // A socket has no command state to be in, so saying which one it
            // was in would be answering a question nobody asked.
            if self.source.is_telnet() {
                let on = self.frame.state == telemetry::CallState::Connected;
                ui.label(
                    RichText::new(if on { "online" } else { "offline" })
                        .monospace()
                        .color(if on {
                            Color32::from_rgb(90, 220, 130)
                        } else {
                            Color32::from_rgb(150, 160, 175)
                        }),
                );
            } else {
                match self.console.mode {
                    Mode::Command => ui.label(
                        RichText::new("command state")
                            .monospace()
                            .color(Color32::from_rgb(150, 160, 175)),
                    ),
                    Mode::Online => ui.label(
                        RichText::new("online")
                            .monospace()
                            .color(Color32::from_rgb(90, 220, 130)),
                    ),
                };
            }
            // In telnet mode this sits up with the host box instead, where
            // there is room for it.
            if self.tab == Tab::Terminal && !self.source.is_telnet() {
                ui.separator();
                ui.add(egui::Slider::new(&mut self.font_size, 9.0..=22.0).text("font"));
            }
        });
        ui.separator();
        match self.tab {
            Tab::Terminal => {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.terminal_pane(ui));
            }
            Tab::Transcript => self.transcript(ui),
        }
    }

    /// Label for the symbol scope, as the modem itself reports it.
    fn symbol_label(&self) -> String {
        self.frame.symbol_label.to_string()
    }
}

impl eframe::App for ScopeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        scopes::request_animation(ui.ctx());

        egui::Panel::top("controls").show(ui, |ui| {
            ui.add_space(4.0);
            self.controls(ui);
            ui.add_space(4.0);
        });

        // A socket has no signal path, so there is nothing for the scopes to
        // show and no honest way to fill them. The terminal takes the whole
        // window instead, which is what this mode is for looking at.
        if self.source.is_telnet() {
            egui::CentralPanel::default().show(ui, |ui| self.lower(ui));
            return;
        }

        egui::Panel::left("panel")
            .resizable(false)
            .exact_size(PANEL_W)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("front panel").strong());
                scopes::faceplate(ui, &self.frame.leds);

                ui.add_space(8.0);
                ui.label(RichText::new("symbols").strong());
                let label = self.symbol_label();
                scopes::symbol_scope(
                    ui,
                    &self.frame.symbols,
                    &self.frame.constellation,
                    self.frame.tones,
                    &label,
                    self.frame.symbol_quality(),
                    PANEL_W - 20.0,
                );

                ui.add_space(8.0);
                ui.label(RichText::new("receive level").strong());
                scopes::level_meter(ui, self.frame.rx_level_db);

                ui.add_space(10.0);
                self.status(ui);
            });

        egui::Panel::bottom("lower")
            .resizable(true)
            .default_size(420.0)
            .show(ui, |ui| self.lower(ui));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.label(RichText::new("waterfall  (0 - 4000 Hz)").strong());
            let available = (ui.available_height() - 150.0).max(140.0);
            self.waterfall.paint(ui, available * 0.60, self.frame.modulation);
            ui.add_space(6.0);
            ui.label(RichText::new("spectrum").strong());
            scopes::spectrum(
                ui,
                &self.frame.spectrum_db,
                self.frame.hz_per_bin,
                (available * 0.40).max(90.0),
                self.waterfall.floor_db,
                self.waterfall.ceiling_db,
                self.frame.modulation,
            );
            ui.add_space(6.0);
            ui.label(RichText::new("discriminator  (answer band)").strong());
            scopes::discriminator(ui, &self.frame.baseband, 84.0);
        });
    }
}

/// One line-rate box, offering only the rates the modulation has.
fn rate_box(ui: &mut egui::Ui, id: &str, value: &mut u32, rates: &[u32]) {
    egui::ComboBox::from_id_salt(id)
        .width(78.0)
        .selected_text(format!("{value}"))
        .show_ui(ui, |ui| {
            for &rate in rates {
                ui.selectable_value(value, rate, format!("{rate}"));
            }
        });
}

/// Compile-time reminder that the engine and UI agree on the FFT size.
const _: () = assert!(FFT_SIZE / 2 == SPECTRUM_BINS);

#[cfg(test)]
mod modulation_tests {
    use super::{Modulation, ScopeApp};

    /// The rate lists are indexed by the same number the carrier box is, so
    /// the two orders have to stay together. Nothing else enforces it.
    #[test]
    fn the_rate_lists_belong_to_the_carriers_they_are_indexed_by() {
        assert_eq!(ScopeApp::CARRIERS[0].0, "B103");
        assert_eq!(Modulation::rates(0), &[300]);
        assert_eq!(ScopeApp::CARRIERS[1].0, "V22B");
        assert_eq!(Modulation::rates(1), &[1200, 2400]);
        assert_eq!(ScopeApp::CARRIERS[2].0, "V32");
        assert_eq!(Modulation::rates(2), &[4800, 9600]);
        assert_eq!(ScopeApp::CARRIERS.len(), 3);
    }

    #[test]
    fn it_starts_where_the_recommendation_says() {
        // V.250 6.4.1: automode on, and no range asked for. The same defaults
        // the AT interpreter itself starts with, so an untouched window
        // composes the command that changes nothing.
        let m = Modulation::default();
        assert!(m.automode);
        assert_eq!((m.min_rate, m.max_rate), (0, 0), "a limit nobody asked for");
    }

    #[test]
    fn changing_modulation_moves_the_rates_into_what_it_can_do() {
        // The whole point of the window over typing the command: a rate the
        // chosen modulation has never heard of should not be composable, let
        // alone sendable.
        let mut m = Modulation::default();
        m.fit(1);
        assert_eq!((m.min_rate, m.max_rate), (1200, 2400), "V.22bis");
        m.fit(0);
        assert_eq!((m.min_rate, m.max_rate), (300, 300), "Bell 103 has one rate");
        m.fit(2);
        assert_eq!((m.min_rate, m.max_rate), (4800, 9600), "V.32");
    }

    #[test]
    fn a_ceiling_survives_a_modulation_that_still_has_it() {
        // Someone who held V.22bis to 1200 and looked at another modulation
        // and came back should find their ceiling still there.
        let mut m = Modulation { automode: true, min_rate: 1200, max_rate: 1200 };
        m.fit(1);
        assert_eq!((m.min_rate, m.max_rate), (1200, 1200));
    }

    #[test]
    fn a_minimum_above_the_maximum_is_never_composed() {
        // V.250 5.4.2 makes it an error, so the window should not be able to
        // build one to be refused.
        let mut m = Modulation { automode: false, min_rate: 9600, max_rate: 1200 };
        m.fit(2);
        assert!(m.min_rate <= m.max_rate, "{m:?}");
        let mut m = Modulation { automode: false, min_rate: 2400, max_rate: 1200 };
        m.fit(1);
        assert!(m.min_rate <= m.max_rate, "{m:?}");
    }

    #[test]
    fn the_command_is_the_one_the_interpreter_parses() {
        // Carrier, automode, minimum, maximum -- V.250 6.4.1, in that order.
        let m = Modulation { automode: true, min_rate: 1200, max_rate: 1200 };
        assert_eq!(m.command("V22B"), "AT+MS=V22B,1,1200,1200");
        let m = Modulation { automode: false, min_rate: 4800, max_rate: 9600 };
        assert_eq!(m.command("V32"), "AT+MS=V32,0,4800,9600");
        // With no range chosen, none is sent: an omitted rate is unspecified,
        // and sending the modulation's own range instead would be a ceiling
        // nobody asked for and one a negotiation could not get past.
        let m = Modulation::default();
        assert_eq!(m.command("V22B"), "AT+MS=V22B,1");
    }
}
