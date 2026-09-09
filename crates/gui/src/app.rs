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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Modulation {
    /// Whether the modem may choose a different modulation than the one asked
    /// for. The Recommendation defaults this on.
    automode: bool,
    min_rate: u32,
    max_rate: u32,
}

/// The `AT+ES` and `AT+DS` subparameters the protection window is composing,
/// and the two reporting parameters that say what came of them.
///
/// Everything here is settled between the two modems and none of it is visible
/// from the terminal, which sees a CONNECT and a rate. The window is the only
/// place a person can say what they want of it before the call rather than
/// find out afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Protection {
    /// `+ES` `<orig_rqst>`, V.250 Table 20.
    request: u8,
    /// `+ES` `<orig_fbk>`: 0 carries on without, 2 hangs up.
    fallback: u8,
    /// `+ER`: report which error control was negotiated.
    report_error_control: bool,
    /// `+DS` `<direction>`: 3 both ways or 0 not at all.
    compress: bool,
    /// `+DS` `<compression_negotiation>`: 1 hangs up if the far end will not.
    compress_required: bool,
    /// `+DS` `<max_dict>`, V.42bis P1.
    max_dict: u16,
    /// `+DS` `<max_string>`, V.42bis P2.
    max_string: u8,
    /// `+DR`: report which compression was negotiated.
    report_compression: bool,
}

impl Default for Protection {
    fn default() -> Self {
        // The modem's own defaults, so the window opens agreeing with it.
        Self {
            request: 3,
            fallback: 0,
            report_error_control: false,
            compress: true,
            compress_required: false,
            max_dict: 2048,
            max_string: 250,
            report_compression: false,
        }
    }
}

impl Protection {
    /// Dictionary sizes worth offering.
    ///
    /// V.42bis Appendix II.1: "if values of N2 in the range 2^n + 1 to
    /// approximately 1.3 x 2^n are selected, no performance improvement will
    /// be gained over the selection of the value 2^n". So the powers of two,
    /// and nothing between them.
    const DICTIONARIES: [u16; 7] = [512, 1024, 2048, 4096, 8192, 16384, 32768];
    /// String lengths, from V.42bis 6.4's floor to its ceiling.
    const STRINGS: [u8; 6] = [6, 16, 32, 64, 128, 250];

    /// Whether error control is being asked for at all.
    fn wants_error_control(self) -> bool {
        self.request >= 2
    }

    /// The command lines this composes, in the order they should be sent.
    ///
    /// `+ES` first, because turning error control off is also turning
    /// compression off -- V.42bis rides on LAPM and there is nowhere else for
    /// it to be -- and a terminal reading these back should see them settle in
    /// an order that makes sense.
    fn commands(self) -> Vec<String> {
        vec![
            format!("AT+ES={},{}", self.request, self.fallback),
            format!(
                "AT+DS={},{},{},{}",
                if self.compress { 3 } else { 0 },
                u8::from(self.compress_required),
                self.max_dict,
                self.max_string
            ),
            format!(
                "AT+ER={};+DR={}",
                u8::from(self.report_error_control),
                u8::from(self.report_compression)
            ),
        ]
    }
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
            2 => &[4800, 9600],
            // V.32bis 2.3: the two V.32 rates and the three it adds, all at
            // the same 2400 baud.
            _ => &[4800, 7200, 9600, 12_000, 14_400],
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

/// Width of the panel holding the far end's account of itself.
const DISTANT_W: f32 = 250.0;

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
    /// The same, for `AT+ES` and `AT+DS`.
    protection: Protection,
    protection_open: bool,
    /// The PPP window.
    network_open: bool,
    /// Whether the window has asked for a stream of echoes rather than one.
    ping_repeatedly: bool,
    /// And whether it has asked for web traffic to be carried.
    carry_web: bool,
    /// The file transfer window, and the two paths it works with.
    transfer_open: bool,
    send_path: String,
    receive_dir: String,
    /// Whether the line was open on the last frame, for noticing when it opens.
    line_was_open: bool,
    /// Where a telnet connection is aimed.
    host: String,
    tab: Tab,
    font_size: f32,
    last_repaint: std::time::Instant,
    /// What was written out last, so that a frame which changed nothing does
    /// not rewrite the file sixty times a second.
    remembered: String,
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
        let chosen_in = live::named(&inputs, live::LINE_IN).unwrap_or(0);
        let chosen_out = live::named(&outputs, live::LINE_OUT).unwrap_or(0);
        let mut app = Self {
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
            protection: Protection::default(),
            protection_open: false,
            network_open: false,
            ping_repeatedly: false,
            carry_web: false,
            transfer_open: false,
            line_was_open: false,
            send_path: String::new(),
            receive_dir: "downloads".to_owned(),
            tab: Tab::Terminal,
            font_size: 14.0,
            last_repaint: std::time::Instant::now(),
            remembered: String::new(),
        };
        // Before anything is drawn, so the first frame shows what the modem
        // will actually be set to rather than the defaults it never used.
        app.recall();
        app
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
            // Keep the keys a terminal needs instead of letting them move the
            // focus. egui spends the arrows and Tab on walking between widgets
            // and Escape on giving focus up, which are the right defaults for
            // a form and wrong for this: a board's menus are driven with the
            // arrows, and Escape is how half of them are left. Pressing either
            // put the cursor somewhere else in the window and sent nothing
            // down the line, so an arrow-key menu could not be used at all.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            let typed = console::keys_to_bytes(ui);
            if !typed.is_empty() {
                match &self.source {
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
    const CARRIERS: [(&'static str, &'static str); 4] = [
        ("B103", "Bell 103 - 300 bit/s"),
        ("V22B", "V.22bis - 1200 or 2400"),
        ("V32", "V.32 - 4800 or 9600"),
        ("V32B", "V.32bis - 4800 to 14400"),
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

        // A line that has just opened gets told everything, once. Otherwise
        // what the window shows and what the modem is running are two
        // different things that happen to have started the same, and they
        // drift the moment anything is typed at the terminal.
        if state.open && !self.line_was_open {
            self.assert_settings(&session);
        }
        self.line_was_open = state.open;

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

                // The comparison, which is the thing the slider is really for.
                // A transmit level is neither right nor wrong on its own; it
                // is right or wrong against what is arriving.
                if state.rx_rms > 1.0e-4 && state.tx_rms > 1.0e-4 {
                    let over = 20.0 * (state.tx_rms / state.rx_rms).log10();
                    let loud = over > 6.0;
                    ui.label(
                        RichText::new(format!("{over:+5.1} dB vs far end"))
                            .monospace()
                            .color(if loud {
                                Color32::from_rgb(235, 100, 90)
                            } else {
                                Color32::from_rgb(140, 150, 165)
                            }),
                    )
                    .on_hover_text(
                        "How much louder this modem is than the one it is talking \
                         to. Well above zero and something in the path is being \
                         driven past what it can carry cleanly -- which does not \
                         sound like silence at the far end, it sounds like a far \
                         end that answers the robust parts of a handshake and none \
                         of the delicate ones. V.21 at 300 bit/s survives almost \
                         anything; a sixteen-point constellation does not",
                    );
                }
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
                // Every subparameter, not just the carrier. A bare +MS leaves
                // the rest to V.250 6.4.1's defaults -- which include automode
                // on -- so choosing a modulation here used to switch V.8 back
                // on behind the button beside it, while the button went on
                // saying it was off. Picking V.22bis and being handed V.32 is
                // not a surprise anybody should have to work out from a
                // recording afterwards.
                session.type_bytes(
                    format!("{}\r", self.modulation.command(Self::CARRIERS[i].0))
                        .as_bytes(),
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
            if ui
                .selectable_label(self.network_open, "Network")
                .on_hover_text(
                    "PPP over the call: give the two ends addresses and ping \
                     between them",
                )
                .clicked()
            {
                self.network_open = !self.network_open;
            }
            if ui
                .selectable_label(self.transfer_open, "Files")
                .on_hover_text(
                    "ZMODEM: send a file to the far end, or take one it offers",
                )
                .clicked()
            {
                self.transfer_open = !self.transfer_open;
            }
            if ui
                .selectable_label(self.protection_open, "Error control")
                .on_hover_text(
                    "AT+ES and AT+DS: V.42 error control and V.42bis compression, \
                     and whether the modem should report what it negotiated",
                )
                .clicked()
            {
                self.protection_open = !self.protection_open;
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
        self.advanced_protection(ui, &session);
        self.transfer_window(ui, &session);
        self.network_window(ui, &session);
    }

    /// PPP over the call, and a ping over that.
    ///
    /// The point of the window is the two numbers at the bottom: an address
    /// this end was given rather than configured, and a round trip measured
    /// over a modem. Between them they say the call is carrying IP, which is
    /// not something any amount of staring at a constellation will tell you.
    fn network_window(&mut self, ui: &mut egui::Ui, session: &Arc<live::Session>) {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        let good = Color32::from_rgb(90, 220, 130);
        let mut open = self.network_open;
        let link = session.network();
        egui::Window::new("PPP - network")
            .open(&mut open)
            .resizable(false)
            .default_width(420.0)
            .show(ui.ctx(), |ui| {
                let online = self.frame.state == telemetry::CallState::Connected;
                ui.horizontal(|ui| {
                    if link.is_none() {
                        if ui
                            .add_enabled(online, egui::Button::new("Bring PPP up"))
                            .on_hover_text(
                                "RFC 1661: the terminal stops being a terminal and \
                                 the call starts carrying frames",
                            )
                            .clicked()
                        {
                            session.start_network();
                        }
                    } else if ui.button("Put it down").clicked() {
                        self.ping_repeatedly = false;
                        self.carry_web = false;
                        session.stop_network();
                    }
                    if let Some(view) = &link {
                        ui.label(
                            RichText::new(&view.phase)
                                .monospace()
                                .color(if view.up { good } else { dim }),
                        );
                        ui.label(
                            RichText::new(if view.serving {
                                "handing out an address"
                            } else {
                                "asking for one"
                            })
                            .small()
                            .color(dim),
                        );
                    }
                });

                if !online && link.is_none() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("There is no call. PPP needs one under it.")
                            .small()
                            .color(dim),
                    );
                }

                let Some(view) = link else { return };
                ui.separator();
                egui::Grid::new("ppp addresses")
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("this end").monospace().color(dim));
                        ui.label(RichText::new(&view.local).monospace().color(bright));
                        ui.end_row();
                        ui.label(RichText::new("far end").monospace().color(dim));
                        ui.label(RichText::new(&view.remote).monospace().color(bright));
                        ui.end_row();
                        ui.label(RichText::new("frames").monospace().color(dim));
                        ui.label(
                            RichText::new(format!(
                                "{} octets out, {} in",
                                view.tx_bytes, view.rx_bytes
                            ))
                            .monospace()
                            .small()
                            .color(dim),
                        );
                        ui.end_row();
                    });

                ui.separator();
                ui.add_enabled_ui(view.up, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button("Ping")
                            .on_hover_text("RFC 792: one echo request to the far end")
                            .clicked()
                        {
                            session.ping_once();
                        }
                        if ui
                            .checkbox(&mut self.ping_repeatedly, "one a second")
                            .changed()
                        {
                            session.ping_repeatedly(self.ping_repeatedly);
                        }
                        if view.in_flight > 0 {
                            ui.label(
                                RichText::new(format!("{} in flight", view.in_flight))
                                    .small()
                                    .color(dim),
                            );
                        }
                    });
                });

                ui.separator();
                ui.add_enabled_ui(view.up, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut self.carry_web, "carry web traffic")
                            .on_hover_text(
                                "SOCKS 5 over the link: the end that answered \
                                 the call opens the connections, the end that \
                                 dialled points a browser at a local port",
                            )
                            .changed()
                        {
                            session.carry_web(self.carry_web);
                        }
                        if let Some(p) = &view.proxy {
                            if let Some(why) = &p.trouble {
                                ui.label(
                                    RichText::new(why)
                                        .small()
                                        .color(Color32::from_rgb(235, 100, 90)),
                                );
                            } else if !p.at.is_empty() {
                                ui.label(
                                    RichText::new(if p.serving {
                                        format!("offering the internet at {}", p.at)
                                    } else {
                                        format!("socks5://{}", p.at)
                                    })
                                    .monospace()
                                    .small()
                                    .color(bright),
                                );
                            }
                        }
                    });
                    if let Some(p) = &view.proxy
                        && p.open > 0
                    {
                        ui.label(
                            RichText::new(format!("{} connections being carried", p.open))
                                .small()
                                .color(dim),
                        );
                    }
                });

                let s = view.stats;
                if s.sent == 0 {
                    return;
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} sent, {} back, {} lost ({:.0}%)",
                        s.sent,
                        s.received,
                        s.lost,
                        s.loss() * 100.0
                    ))
                    .monospace()
                    .small()
                    .color(if s.lost > 0 {
                        Color32::from_rgb(240, 200, 120)
                    } else {
                        dim
                    }),
                );
                if let Some(average) = s.average_ms() {
                    ui.label(
                        RichText::new(format!(
                            "round trip {} ms, best {}, worst {}, mean {average:.0}",
                            s.last_ms, s.best_ms, s.worst_ms
                        ))
                        .monospace()
                        .color(bright),
                    );
                }
            });
        self.network_open = open;
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
                            "A ceiling is worth setting on purpose. Every rate here                              is 2400 baud and they differ only in how crowded the                              constellation is: four points at 4800, a hundred and                              twenty-eight at 14 400, and about 20 dB more signal to                              noise wanted across that span.",
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

    /// Every setting the window holds, as command lines.
    ///
    /// Put back what the last run was set to.
    fn recall(&mut self) {
        let (carrier, modulation, protection) =
            from_remembered(&crate::remembered::Remembered::load());
        self.carrier = carrier;
        self.modulation = modulation;
        self.protection = protection;
        self.remembered = self.settings().join("
");
    }

    /// Write the settings out if they have moved since they were last written.
    ///
    /// Compared as the commands they compose rather than field by field, which
    /// is the comparison that matters: two states that assert identically are
    /// the same state as far as the modem is concerned.
    fn remember(&mut self) {
        let now = self.settings().join("
");
        if now != self.remembered {
            self.remembered = now;
            to_remember(self.carrier, self.modulation, self.protection).save();
        }
    }

    /// `&F` first, and then all of it. The window's controls are the ones a
    /// person has actually looked at, so they are what the modem should be
    /// running -- and anything not represented here should be a default rather
    /// than whatever the last call left behind.
    fn settings(&self) -> Vec<String> {
        let mut out = vec!["AT&F".to_owned()];
        out.push(self.modulation.command(Self::CARRIERS[self.carrier].0));
        out.extend(self.protection.commands());
        out
    }

    /// Put the modem into the state the window is showing.
    fn assert_settings(&self, session: &Arc<live::Session>) {
        for command in self.settings() {
            session.type_bytes(format!("{command}\r").as_bytes());
        }
    }

    /// Sending a file, or taking one.
    fn transfer_window(&mut self, ui: &mut egui::Ui, session: &Arc<live::Session>) {
        let dim = Color32::from_rgb(140, 150, 165);
        let mut open = self.transfer_open;
        let running = session.transfer();
        egui::Window::new("ZMODEM - files")
            .open(&mut open)
            .resizable(false)
            .default_width(520.0)
            .show(ui.ctx(), |ui| {
                let busy = running.as_ref().is_some_and(|t| !t.finished);
                let online = self.frame.state == telemetry::CallState::Connected;

                ui.add_enabled_ui(online && !busy, |ui| {
                    egui::Grid::new("xfer")
                        .num_columns(3)
                        .spacing([10.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("send").monospace().color(dim));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.send_path)
                                    .desired_width(300.0)
                                    .hint_text("path to a file"),
                            );
                            if ui.button("Send").clicked() && !self.send_path.trim().is_empty() {
                                session.send_file(self.send_path.trim().into());
                            }
                            ui.end_row();

                            ui.label(RichText::new("receive").monospace().color(dim));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.receive_dir)
                                    .desired_width(300.0)
                                    .hint_text("directory to keep files in"),
                            );
                            if ui
                                .button("Receive")
                                .on_hover_text(
                                    "Wait for the far end to start sending. Tell the \
                                     board to send first: this end answers, it does \
                                     not ask",
                                )
                                .clicked()
                            {
                                session.receive_into(self.receive_dir.trim().into());
                            }
                            ui.end_row();
                        });
                });

                if !online {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("There is no call. A transfer needs one.")
                            .small()
                            .color(dim),
                    );
                }

                let Some(t) = running else { return };
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if t.sending { "sending" } else { "receiving" })
                            .monospace()
                            .color(dim),
                    );
                    ui.label(
                        RichText::new(if t.name.is_empty() { "-" } else { &t.name })
                            .monospace()
                            .color(Color32::from_rgb(220, 225, 235)),
                    );
                });
                ui.add_space(4.0);

                // A total is what the far end said, and 13 calls it "an
                // estimate only" -- so a bar is drawn where there is one and a
                // running count where there is not, rather than a bar that
                // pretends to know.
                match t.total.filter(|n| *n > 0) {
                    Some(total) => {
                        let part = (t.position as f32 / total as f32).clamp(0.0, 1.0);
                        ui.add(
                            egui::ProgressBar::new(part)
                                .desired_width(480.0)
                                .text(format!(
                                    "{} of {} bytes  ({:.0}%)",
                                    t.position,
                                    total,
                                    part * 100.0
                                )),
                        );
                    }
                    None => {
                        ui.label(
                            RichText::new(format!("{} bytes", t.position))
                                .monospace()
                                .color(dim),
                        );
                    }
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let stat = |ui: &mut egui::Ui, text: String, warn: bool| {
                        ui.label(RichText::new(text).monospace().small().color(if warn {
                            Color32::from_rgb(240, 200, 120)
                        } else {
                            dim
                        }));
                    };
                    stat(ui, format!("{:.0} bytes/s", t.rate), false);
                    ui.separator();
                    // What an error costs, which is the number worth watching:
                    // 8.2 recovers by sending the sender back, so a rewind is
                    // ground covered twice.
                    stat(ui, format!("{} rewinds", t.rewinds), t.rewinds > 0);
                    if t.sending {
                        ui.separator();
                        stat(ui, format!("{} bytes resent", t.resent), t.resent > 0);
                    } else {
                        ui.separator();
                        stat(ui, format!("{} damaged", t.damaged), t.damaged > 0);
                    }
                });

                if !t.outcome.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(&t.outcome)
                            .monospace()
                            .color(if t.finished && t.outcome == "done" {
                                Color32::from_rgb(90, 220, 130)
                            } else if t.finished {
                                Color32::from_rgb(235, 100, 90)
                            } else {
                                dim
                            }),
                    );
                }
                if let Some(where_to) = &t.written_to {
                    ui.label(RichText::new(where_to).monospace().small().color(dim));
                }
                if !t.finished {
                    ui.add_space(4.0);
                    if ui
                        .button("Cancel")
                        .on_hover_text("Eight CAN characters, which is how ZMODEM stops")
                        .clicked()
                    {
                        session.cancel_transfer();
                    }
                }
            });
        self.transfer_open = open;
    }

    /// `AT+ES` and `AT+DS`, and the two parameters that report on them.
    fn advanced_protection(&mut self, ui: &mut egui::Ui, session: &Arc<live::Session>) {
        let dim = Color32::from_rgb(140, 150, 165);
        let mut open = self.protection_open;
        egui::Window::new("AT+ES / AT+DS - error control")
            .open(&mut open)
            .resizable(false)
            .default_width(520.0)
            .show(ui.ctx(), |ui| {
                let p = &mut self.protection;
                egui::Grid::new("es")
                    .num_columns(2)
                    .spacing([14.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("error control").monospace().color(dim));
                        ui.vertical(|ui| {
                            // V.250 Table 20. Values 0 and 1 differ in what the
                            // DTE interface does, which is nothing here: both
                            // mean a connection with no protocol on it.
                            ui.radio_value(&mut p.request, 3, "V.42, asking the far end first")
                                .on_hover_text(
                                    "Initiate V.42 with Detection Phase. The two modems \
                                     exchange the patterns of V.42 7.2.1 to find out \
                                     whether the other does error control at all, which \
                                     costs up to three quarters of a second and is what \
                                     makes a far end without it work rather than fail",
                                );
                            ui.radio_value(&mut p.request, 2, "V.42, without asking")
                                .on_hover_text(
                                    "Initiate V.42 without Detection Phase. For a far end \
                                     already known to do V.42 -- and what V.92 requires \
                                     once V.8 has settled it. Against a far end that does \
                                     not, the fallback below is what happens instead",
                                );
                            ui.radio_value(&mut p.request, 0, "none")
                                .on_hover_text(
                                    "Direct mode: the line carries start-stop characters \
                                     and nothing checks them. Which is how every modem \
                                     worked before 1989, and how Bell 103 still works here",
                                );
                        });
                        ui.end_row();

                        ui.label(RichText::new("if there is none").monospace().color(dim));
                        ui.add_enabled_ui(p.wants_error_control(), |ui| {
                            ui.vertical(|ui| {
                                ui.radio_value(&mut p.fallback, 0, "carry on without it")
                                    .on_hover_text(
                                        "Error control optional. A far end without V.42 is \
                                         a perfectly ordinary far end -- V.42 7.2.1 exists \
                                         to find that out rather than to fail on it",
                                    );
                                ui.radio_value(&mut p.fallback, 2, "hang up")
                                    .on_hover_text(
                                        "Error control required; if not established, \
                                         disconnect. For a call whose whole point is that \
                                         what arrives is what was sent",
                                    );
                            });
                        });
                        ui.end_row();

                        ui.label(RichText::new("compression").monospace().color(dim));
                        ui.add_enabled_ui(p.wants_error_control(), |ui| {
                            ui.vertical(|ui| {
                                ui.checkbox(&mut p.compress, "V.42bis, both directions")
                                    .on_hover_text(
                                        "V.42bis rides on LAPM and there is nowhere else \
                                         for it to be, so error control off is compression \
                                         off. Negotiated as a pair: one direction only is \
                                         a promise this modem cannot keep",
                                    );
                                ui.add_enabled_ui(p.compress, |ui| {
                                    ui.checkbox(
                                        &mut p.compress_required,
                                        "hang up if the far end will not",
                                    )
                                    .on_hover_text(
                                        "V.250 Table 27: disconnect if V.42bis is not \
                                         negotiated by the remote DCE as specified",
                                    );
                                });
                            });
                        });
                        ui.end_row();

                        ui.label(RichText::new("dictionary").monospace().color(dim));
                        ui.add_enabled_ui(p.wants_error_control() && p.compress, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("codewords").color(dim));
                                combo(ui, "ds-dict", &mut p.max_dict, &Protection::DICTIONARIES);
                                ui.label(RichText::new("longest string").color(dim));
                                combo(ui, "ds-str", &mut p.max_string, &Protection::STRINGS);
                            });
                        });
                        ui.end_row();

                        ui.label(RichText::new("tell me").monospace().color(dim));
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut p.report_error_control,
                                "which error control was agreed  (+ER)",
                            )
                            .on_hover_text(
                                "Prints +ER: LAPM or +ER: NONE to the terminal just \
                                 before the CONNECT. V.250 6.5.5 puts it there on \
                                 purpose: the modem has settled which protocol it will \
                                 use by then, and CONNECT is the last thing said, so \
                                 what is above it describes the call about to start. \
                                 The window's own log line says more than this; it is \
                                 for a terminal program reading the modem, or for a \
                                 transcript that has to hold the answer",
                            );
                            ui.checkbox(
                                &mut p.report_compression,
                                "which compression was agreed  (+DR)",
                            )
                            .on_hover_text(
                                "Prints +DR: V42B or +DR: NONE, between the error \
                                 control report and the CONNECT (V.250 6.6.3)",
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(4.0);
                let p = self.protection;
                if p.report_error_control || p.report_compression {
                    // What the terminal will actually see, since the point of
                    // the setting is a line of text and nothing else.
                    let mut shown = String::new();
                    if p.report_error_control {
                        shown.push_str(if p.wants_error_control() {
                            "+ER: LAPM   "
                        } else {
                            "+ER: NONE   "
                        });
                    }
                    if p.report_compression {
                        shown.push_str(
                            if p.wants_error_control() && p.compress {
                                "+DR: V42B   "
                            } else {
                                "+DR: NONE   "
                            },
                        );
                    }
                    shown.push_str("CONNECT 9600");
                    ui.label(
                        RichText::new(format!("the terminal will see:  {shown}"))
                            .small()
                            .monospace()
                            .color(dim),
                    );
                }
                // The one thing worth saying out loud, because the numbers
                // look like they are being given away and they are not.
                let note = if !p.wants_error_control() {
                    "Nothing checks what arrives. Every byte the line damages is a byte \
                     the terminal reads."
                } else if p.max_dict <= 512 {
                    "512 codewords is V.42bis's floor. The lower of the two ends is what \
                     runs, so this decides it for both."
                } else {
                    "The lower of the two ends is what runs, so asking for more than the \
                     far end has costs nothing and asking for less decides it for both."
                };
                ui.label(RichText::new(note).small().color(if p.wants_error_control() {
                    dim
                } else {
                    Color32::from_rgb(240, 200, 120)
                }));

                ui.separator();
                for command in p.commands() {
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
                    });
                }
                ui.horizontal(|ui| {
                    if ui
                        .button("Send all")
                        .on_hover_text("All three, in order")
                        .clicked()
                    {
                        for command in p.commands() {
                            session.type_bytes(format!("{command}\r").as_bytes());
                        }
                    }
                    if ui
                        .button("Reset and send everything")
                        .on_hover_text(
                            "AT&F and then every setting this window holds, \
                             modulation included. What the line gets when it \
                             opens, and the way back to a known state",
                        )
                        .clicked()
                    {
                        self.assert_settings(session);
                    }
                    if ui
                        .button("Ask")
                        .on_hover_text("What the modem currently has")
                        .clicked()
                    {
                        session.type_bytes(b"AT+ES?;+DS?;+ER?;+DR?\r");
                    }
                });
            });
        self.protection_open = open;
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

        self.call_settings(ui);
    }

    /// Every ceiling this modem will offer, as rate, modulation and label.
    ///
    /// One list rather than a modulation and a rate to be chosen separately,
    /// because nobody wants "V.22bis" and "2400" as two decisions -- they want
    /// 2400, and the modulation that reaches it follows from that.
    /// Every ceiling worth one press, and which carrier each belongs to.
    ///
    /// Everything from 4800 up is V.32bis, including the two rates plain V.32
    /// also has: V.32bis does them too, and offering the older carrier here
    /// would only take 7200 away. Choosing V.32 on purpose -- which is worth
    /// doing against a far end that claims V.32bis and cannot hold it -- is in
    /// the Advanced window, where a deliberate choice belongs.
    const CEILINGS: [(u32, usize, &'static str); 8] = [
        (300, 0, "300"),
        (1200, 1, "1200"),
        (2400, 1, "2400"),
        (4800, 3, "4800"),
        (7200, 3, "7200"),
        (9600, 3, "9600"),
        (12_000, 3, "12000"),
        (14_400, 3, "14400"),
    ];

    /// How fast at most, and the three things that are simply on or off.
    ///
    /// V.250 makes this three commands across two clauses and a person does
    /// not think of it that way. What they want to say is how fast at most and
    /// whether to use the things that make a call reliable, and every one of
    /// those is one click here.
    ///
    /// Nothing new is settable that was not settable before -- the modulation
    /// box and the two windows write the same subparameters and are still
    /// there for the rest of them. What is new is that the common answer does
    /// not need a window opened to give it.
    fn call_settings(&mut self, ui: &mut egui::Ui) {
        let Source::Live(session) = &self.source else { return };
        let session = Arc::clone(session);
        let dim = Color32::from_rgb(140, 150, 165);

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("up to").monospace().color(dim));

            let mut picked = None;
            for (rate, carrier, label) in Self::CEILINGS {
                let chosen =
                    self.carrier == carrier && self.modulation.max_rate == rate;
                if ui
                    .radio(chosen, label)
                    .on_hover_text(Self::CARRIERS[carrier].1)
                    .clicked()
                {
                    picked = Some((rate, carrier));
                }
            }
            if let Some((rate, carrier)) = picked {
                self.carrier = carrier;
                self.modulation.fit(carrier);
                // A ceiling and no floor. V.250 6.4.1 has an unspecified rate
                // "determined by the modulation means selected", which is what
                // is wanted underneath: as fast as this, and as slow as it
                // takes.
                self.modulation.min_rate = 0;
                self.modulation.max_rate = rate;
                session.type_bytes(
                    format!(
                        "{}\r",
                        self.modulation.command(Self::CARRIERS[carrier].0)
                    )
                    .as_bytes(),
                );
            }

            ui.separator();

            // V.250 6.4.1: with automode on, the box above is where the call
            // starts rather than where it ends up.
            let mut automode = self.modulation.automode;
            if ui
                .checkbox(&mut automode, "V.8")
                .on_hover_text(
                    "Negotiate the modulation with the far end (AT+MS                      <automode>). Off means the one chosen above and nothing                      else.",
                )
                .clicked()
            {
                self.modulation.automode = automode;
                session.type_bytes(
                    format!(
                        "{}\r",
                        self.modulation.command(Self::CARRIERS[self.carrier].0)
                    )
                    .as_bytes(),
                );
            }

            let mut protect = self.protection.wants_error_control();
            if ui
                .checkbox(&mut protect, "V.42")
                .on_hover_text(
                    "Error control: what arrives is what was sent, or the                      call ends (AT+ES).",
                )
                .clicked()
            {
                // 3 is the Recommendation's own default and the one the
                // window's first radio button offers: ask the far end first.
                self.protection.request = if protect { 3 } else { 0 };
                for line in self.protection.commands() {
                    session.type_bytes(format!("{line}\r").as_bytes());
                }
            }

            // V.42bis rides on LAPM and has nowhere else to be, so without
            // error control there is nothing for this to be checked against.
            let mut compress = self.protection.compress && protect;
            let response = ui
                .add_enabled(protect, egui::Checkbox::new(&mut compress, "V.42bis"))
                .on_hover_text(
                    "Compression, which needs error control underneath it                      (AT+DS).",
                )
                .on_disabled_hover_text(
                    "V.42bis rides on LAPM and there is nowhere else for it                      to be, so error control off is compression off.",
                );
            if response.clicked() {
                self.protection.compress = compress;
                for line in self.protection.commands() {
                    session.type_bytes(format!("{line}\r").as_bytes());
                }
            }

            ui.separator();
            ui.label(
                RichText::new(self.modulation.command(Self::CARRIERS[self.carrier].0))
                    .monospace()
                    .color(dim),
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
                // How far the receiver is missing by, against the distance
                // between the points it is choosing between. It is the number
                // 7's retrain decides on, and the only one that means the same
                // thing at 4800 and at 14 400: a tenth is a clean lock,
                // a quarter is where this modem gives up on the rate, and a
                // half is a coin flip.
                if let Some(miss) = f.reception {
                    row(
                        "reading",
                        format!("{miss:.2} of the gap"),
                        if miss < 0.25 {
                            Color32::from_rgb(90, 220, 130)
                        } else {
                            Color32::from_rgb(230, 140, 90)
                        },
                    );
                } else {
                    row(
                        "quality",
                        f.symbol_quality()
                            .map(|q| q.to_string())
                            .unwrap_or_else(|| "-".into()),
                        bright,
                    );
                }
                // What the echo canceller is doing, which on a two-wire pair
                // decides everything and is otherwise invisible: a
                // constellation full of noise looks the same whether the noise
                // is the line or this modem listening to itself.
                if let Some(db) = f.echo_loss_db {
                    row(
                        "echo out",
                        format!("{db:.1} dB"),
                        if db >= 6.0 {
                            Color32::from_rgb(90, 220, 130)
                        } else {
                            Color32::from_rgb(230, 140, 90)
                        },
                    );
                    row(
                        "echo at",
                        match f.echo_at {
                            Some((delay, strength)) => format!(
                                "{:.0} ms, {:.2}",
                                delay as f64 * 1000.0 / f.sample_rate,
                                strength
                            ),
                            None => "not found".into(),
                        },
                        if f.echo_at.is_some() { dim } else { Color32::from_rgb(230, 140, 90) },
                    );
                }
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
            if self.tab == Tab::Terminal {
                ui.separator();
                // A board draws with ANSI and then stops talking, and what it
                // leaves behind is the terminal it was halfway through setting
                // up: a colour, a scrolling region, the cursor somewhere, the
                // mouse reporting to nobody. All of that is this end's state
                // and none of it survives a reset, so there is no reason to
                // drop the call to get a readable screen back.
                if ui
                    .small_button("reset")
                    .on_hover_text(
                        "Put the screen back to a plain terminal -- colours, \
                         cursor, wrapping and mouse reporting all as they \
                         started. Local, so it can be done mid-call and the \
                         far end will not know",
                    )
                    .clicked()
                {
                    self.console.term.reset();
                }
            }
            // In telnet mode this sits up with the host box instead, where
            // there is room for it.
            if self.tab == Tab::Terminal && !self.source.is_telnet() {
                ui.add(egui::Slider::new(&mut self.font_size, 9.0..=22.0).text("font"));
            }
        });
        ui.separator();
        match self.tab {
            Tab::Terminal => {
                // A socket has no far end that describes itself, so there is
                // nothing to put here and the terminal takes the room.
                if !self.source.is_telnet() {
                    egui::Panel::right("distant")
                        .resizable(false)
                        .exact_size(DISTANT_W)
                        .show(ui, |ui| self.distant(ui));
                }
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.terminal_pane(ui));
            }
            Tab::Transcript => self.transcript(ui),
        }
    }

    /// What the far end has said about itself.
    ///
    /// Beside the terminal because it is about the same call and there is room
    /// there. Everything in it was said by the other modem: which modulations
    /// it has, whether it named LAPM in V.8, what it answered in the detection
    /// phase, and what it proposed in XID. None of that reaches the terminal
    /// and all of it is the answer to why a call went the way it did.
    fn distant(&self, ui: &mut egui::Ui) {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        ui.add_space(6.0);
        ui.label(RichText::new("distant").strong());
        ui.add_space(4.0);
        if self.frame.distant.is_empty() {
            ui.label(
                RichText::new(
                    "Nothing said yet. A far end describes itself in the V.8 menu,                      in the detection phase, and in XID -- and a call that gets                      none of the way through says nothing at all.",
                )
                .small()
                .color(dim),
            );
            return;
        }
        egui::Grid::new("distant-rows")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                for (key, value) in &self.frame.distant {
                    ui.label(RichText::new(*key).monospace().small().color(dim));
                    // Wrapped, because a list of modulations is longer than the
                    // panel and truncating it would hide the interesting end.
                    ui.add(
                        egui::Label::new(
                            RichText::new(value).monospace().small().color(bright),
                        )
                        .wrap(),
                    );
                    ui.end_row();
                }
            });
    }

    /// Label for the symbol scope, as the modem itself reports it.
    fn symbol_label(&self) -> String {
        self.frame.symbol_label.to_string()
    }
}

/// Every setting worth carrying from one run to the next, as name and value.
///
/// The modulation is stored by its AT name rather than by its place in the
/// list, so that adding one to the list does not silently change what an older
/// file means.
fn to_remember(
    carrier: usize,
    modulation: Modulation,
    protection: Protection,
) -> crate::remembered::Remembered {
    let mut r = crate::remembered::Remembered::default();
    r.set("carrier", ScopeApp::CARRIERS[carrier].0);
    r.set("automode", modulation.automode);
    r.set("min_rate", modulation.min_rate);
    r.set("max_rate", modulation.max_rate);
    r.set("es_request", protection.request);
    r.set("es_fallback", protection.fallback);
    r.set("report_error_control", protection.report_error_control);
    r.set("compress", protection.compress);
    r.set("compress_required", protection.compress_required);
    r.set("max_dict", protection.max_dict);
    r.set("max_string", protection.max_string);
    r.set("report_compression", protection.report_compression);
    r
}

/// The same, backwards, starting from the defaults.
///
/// Anything missing or unreadable keeps its default, so a file written by an
/// older version -- or no file at all -- leaves the window exactly where it
/// would have been.
fn from_remembered(
    r: &crate::remembered::Remembered,
) -> (usize, Modulation, Protection) {
    let mut carrier = 1;
    let mut m = Modulation::default();
    let mut p = Protection::default();
    if let Some(name) = r.text("carrier")
        && let Some(i) = ScopeApp::CARRIERS.iter().position(|c| c.0 == name)
    {
        carrier = i;
    }
    if let Some(v) = r.get("automode") {
        m.automode = v;
    }
    if let Some(v) = r.get("min_rate") {
        m.min_rate = v;
    }
    if let Some(v) = r.get("max_rate") {
        m.max_rate = v;
    }
    // The rate boxes offer only the rates the chosen modulation has, so a pair
    // carried over from a different one has to be brought back into range
    // rather than asserted as it stands.
    //
    // Zero is left alone. V.250 6.4.1: unspecified rates "are determined by
    // the modulation means selected", so zero is the absence of a limit rather
    // than a limit that happens to be out of range, and fitting it would turn
    // "whatever this modulation can do" into a ceiling nobody asked for.
    if m.min_rate != 0 || m.max_rate != 0 {
        m.fit(carrier);
    }
    if let Some(v) = r.get("es_request") {
        p.request = v;
    }
    if let Some(v) = r.get("es_fallback") {
        p.fallback = v;
    }
    if let Some(v) = r.get("report_error_control") {
        p.report_error_control = v;
    }
    if let Some(v) = r.get("compress") {
        p.compress = v;
    }
    if let Some(v) = r.get("compress_required") {
        p.compress_required = v;
    }
    if let Some(v) = r.get("max_dict") {
        p.max_dict = v;
    }
    if let Some(v) = r.get("max_string") {
        p.max_string = v;
    }
    if let Some(v) = r.get("report_compression") {
        p.report_compression = v;
    }
    (carrier, m, p)
}

impl eframe::App for ScopeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        // Anything the last frame changed, kept for the next run. Cheap when
        // nothing moved, which is almost every frame.
        self.remember();
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
                    scopes::Constellation {
                        points: &self.frame.constellation,
                        tones: self.frame.tones,
                        peak: self.frame.constellation_peak,
                    },
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
/// A box offering one of a fixed set of values.
fn combo<T>(ui: &mut egui::Ui, id: &str, value: &mut T, options: &[T])
where
    T: Copy + PartialEq + std::fmt::Display,
{
    egui::ComboBox::from_id_salt(id)
        .width(88.0)
        .selected_text(format!("{value}"))
        .show_ui(ui, |ui| {
            for option in options {
                ui.selectable_value(value, *option, format!("{option}"));
            }
        });
}

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
        assert_eq!(ScopeApp::CARRIERS[3].0, "V32B");
        assert_eq!(Modulation::rates(3), &[4800, 7200, 9600, 12_000, 14_400]);
        assert_eq!(ScopeApp::CARRIERS.len(), 4);
    }

    /// The two V.32 carriers are two ceilings on one modulation, and the
    /// faster one has every rate the slower one has.
    #[test]
    fn v32bis_can_do_everything_v32_can() {
        for rate in Modulation::rates(2) {
            assert!(
                Modulation::rates(3).contains(rate),
                "V.32bis cannot do {rate}, which V.32 can"
            );
        }
        // And the strip of ceilings only ever names a carrier that has the
        // rate it is offering.
        for (rate, carrier, _) in ScopeApp::CEILINGS {
            assert!(
                Modulation::rates(carrier).contains(&rate),
                "the ceiling {rate} names a carrier that has no such rate"
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What one machine with both cables installed actually reports.
    ///
    /// Kept verbatim because the trap is in the detail: VB-Audio installs a
    /// sixteen-channel endpoint beside each ordinary one, and it sorts first.
    fn outputs() -> Vec<String> {
        [
            "XG2703-GS (NVIDIA High Definition Audio)",
            "CABLE-A In 16ch (VB-Audio Virtual Cable A)",
            "CABLE-A Input (VB-Audio Virtual Cable A)",
            "CABLE-B Input (VB-Audio Virtual Cable B)",
            "Speakers (Realtek(R) Audio)",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    }

    /// Run a command line through a real interpreter and report what it made
    /// of it.
    fn interpreted(line: &str) -> (at::Interpreter, String) {
        let mut it = at::Interpreter::new();
        it.config.echo = false;
        for b in line.bytes() {
            it.feed(b);
        }
        it.feed(b'\r');
        let out = String::from_utf8(it.take_output()).unwrap();
        (it, out)
    }

    #[test]
    fn what_the_last_run_was_set_to_comes_back() {
        // The window's controls are asserted onto the modem when a line opens,
        // so a window that starts at its defaults puts the modem back to them.
        // A person who left it on V.32 with V.8 off and no compression came
        // back to V.22bis with V.8 on and compression, having been told
        // nothing.
        let carrier = 2; // V.32
        let modulation = Modulation { automode: false, min_rate: 4800, max_rate: 4800 };
        let protection = Protection {
            request: 2,
            fallback: 2,
            compress: false,
            compress_required: false,
            max_dict: 1024,
            max_string: 32,
            report_error_control: true,
            report_compression: true,
        };

        let (c, m, p) = from_remembered(&to_remember(carrier, modulation, protection));
        assert_eq!(c, carrier, "the modulation came back as something else");
        assert_eq!(m, modulation);
        assert_eq!(p, protection);
        // And what the modem is told is the same either way round, which is
        // the only comparison that matters.
        assert_eq!(
            ScopeApp::CARRIERS[c].0, "V32",
            "stored by name, so the list may be added to"
        );
    }

    /// Nothing remembered leaves the window exactly where it starts.
    #[test]
    fn a_first_run_keeps_every_default() {
        let (c, m, p) = from_remembered(&crate::remembered::Remembered::default());
        assert_eq!(c, 1, "V.22bis, as it was");
        assert_eq!(m, Modulation::default());
        assert_eq!(p, Protection::default());
    }

    /// A rate that the remembered modulation does not have is brought back
    /// into range rather than asserted.
    #[test]
    fn a_rate_from_another_modulation_is_fitted_to_this_one() {
        let mut r = to_remember(
            2,
            Modulation { automode: false, min_rate: 9600, max_rate: 9600 },
            Protection::default(),
        );
        // The same file, with the modulation changed under it to one that has
        // no 9600 -- which is what editing the box by hand would do.
        r.set("carrier", "V22B");
        let (c, m, _) = from_remembered(&r);
        assert_eq!(ScopeApp::CARRIERS[c].0, "V22B");
        assert!(
            Modulation::rates(c).contains(&m.max_rate),
            "V.22bis was left asking for {}",
            m.max_rate
        );
    }

    #[test]
    fn the_startup_settings_leave_the_modem_where_the_window_says() {
        // What the line gets the moment it opens. The window's controls are
        // the ones a person has looked at, so they are what the modem should
        // be running -- and before this it was whatever the modem happened to
        // default to, which drifted from the window the moment anything was
        // typed at the terminal.
        let app_carrier = 2; // V.32, so it is not the modem's own default
        let modulation = Modulation { automode: false, min_rate: 4800, max_rate: 4800 };
        let protection = Protection {
            request: 2,
            fallback: 2,
            compress: true,
            compress_required: false,
            max_dict: 1024,
            max_string: 32,
            report_error_control: true,
            report_compression: true,
        };

        let mut commands = vec!["AT&F".to_owned()];
        commands.push(modulation.command(ScopeApp::CARRIERS[app_carrier].0));
        commands.extend(protection.commands());

        let mut it = at::Interpreter::new();
        it.config.echo = false;
        for command in &commands {
            for b in command.bytes() {
                it.feed(b);
            }
            it.feed(b'\r');
            let out = String::from_utf8(it.take_output()).unwrap();
            assert!(!out.contains("ERROR"), "{command:?} was refused");
        }

        assert_eq!(it.modulation.carrier, "V32");
        assert!(!it.modulation.automode);
        assert_eq!(it.modulation.max_rate, 4800);
        assert_eq!(it.error_control.request, 2);
        assert_eq!(it.error_control.fallback, 2);
        assert_eq!(it.compression.max_dict, 1024);
        assert!(it.config.report_error_control && it.config.report_compression);
    }

    #[test]
    fn the_reset_comes_first_or_it_undoes_the_rest() {
        // &F restores the factory configuration, which now includes +MS, +ES
        // and +DS. Sent after them it would put every one of them back.
        let commands = {
            let mut v = vec!["AT&F".to_owned()];
            v.push(Modulation::default().command("V22B"));
            v.extend(Protection::default().commands());
            v
        };
        assert_eq!(commands[0], "AT&F");
        assert!(commands[1..].iter().all(|c| c != "AT&F"));
    }

    #[test]
    fn every_command_the_window_composes_is_one_the_modem_accepts() {
        // The window builds AT lines by hand and the modem parses them by
        // hand, and nothing else makes the two agree. A window offering a
        // setting the interpreter refuses is worse than one without it.
        for p in [
            Protection::default(),
            Protection { request: 2, fallback: 2, ..Protection::default() },
            Protection { request: 0, ..Protection::default() },
            Protection {
                compress: false,
                report_error_control: true,
                report_compression: true,
                ..Protection::default()
            },
            Protection { max_dict: 512, max_string: 6, ..Protection::default() },
            Protection { max_dict: 32768, max_string: 250, ..Protection::default() },
        ] {
            for command in p.commands() {
                let (_, out) = interpreted(&command);
                assert!(!out.contains("ERROR"), "{command:?} was refused");
            }
        }
    }

    #[test]
    fn what_the_window_sends_is_what_the_modem_then_has() {
        // And the values survive the round trip, which is the part a typo in
        // the format string would not.
        let p = Protection {
            request: 2,
            fallback: 2,
            compress: true,
            compress_required: true,
            max_dict: 4096,
            max_string: 32,
            report_error_control: true,
            report_compression: true,
        };
        let mut it = at::Interpreter::new();
        it.config.echo = false;
        for command in p.commands() {
            for b in command.bytes() {
                it.feed(b);
            }
            it.feed(b'\r');
            it.take_output();
        }
        assert_eq!(it.error_control.request, 2);
        assert_eq!(it.error_control.fallback, 2);
        assert_eq!(it.compression.direction, 3);
        assert!(it.compression.required);
        assert_eq!(it.compression.max_dict, 4096);
        assert_eq!(it.compression.max_string, 32);
        assert!(it.config.report_error_control);
        assert!(it.config.report_compression);
    }

    #[test]
    fn turning_error_control_off_turns_compression_off_with_it() {
        // V.42bis rides on LAPM and there is nowhere else for it to be. The
        // window disables the compression controls, and the command it sends
        // has to say the same thing -- so the order matters: +ES first.
        let p = Protection { request: 0, ..Protection::default() };
        let commands = p.commands();
        assert!(commands[0].starts_with("AT+ES=0"), "{:?}", commands[0]);
        let (it, _) = interpreted(&commands[0]);
        assert!(!it.error_control.wanted());
    }

    #[test]
    fn the_line_is_found_by_name_and_not_by_position() {
        let outs = outputs();
        let i = live::named(&outs, live::LINE_OUT).expect("the B cable is in that list");
        assert!(outs[i].starts_with("CABLE-B Input"), "found {:?}", outs[i]);
    }

    #[test]
    fn a_sixteen_channel_endpoint_is_not_mistaken_for_the_cable() {
        // VB-Audio installs a sixteen-channel endpoint beside each ordinary
        // one and it sorts first. It is the same cable with sixteen channels
        // on it, and a modem opened there puts its carrier down one of them.
        //
        // On a machine with one cable, where the fallback is what matches, the
        // two names differ by three characters: "CABLE Input" against "CABLE
        // In 16ch". The pattern has to be the whole of "Input" for that to be
        // a difference at all.
        let single: Vec<String> = [
            "CABLE In 16ch (VB-Audio Virtual Cable)",
            "CABLE Input (VB-Audio Virtual Cable)",
            "Speakers (Realtek(R) Audio)",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let i = live::named(&single, live::LINE_OUT).expect("one cable is still a cable");
        assert!(!single[i].contains("16ch"), "matched {:?}", single[i]);
        assert!(single[i].starts_with("CABLE Input"));
    }

    #[test]
    fn two_cables_are_preferred_to_one() {
        // A machine with both has them for this: one carries what the softphone
        // plays and the other what this modem says, so neither modem hears
        // itself. A single cable is a two-wire line with both modems across it,
        // which is a fine model of a telephone pair and useless for reaching
        // anything outside the machine.
        let mut both = outputs();
        both.push("CABLE Input (VB-Audio Virtual Cable)".to_owned());
        let i = live::named(&both, live::LINE_OUT).expect("B is in there");
        assert!(both[i].starts_with("CABLE-B Input"), "matched {:?}", both[i]);
    }

    #[test]
    fn a_machine_without_the_cables_is_not_guessed_at() {
        // The whole reason this returns an Option. Falling back to whichever
        // device is first would open the line on the speakers, and a handshake
        // played through speakers is no use to anyone.
        let plain: Vec<String> = ["Speakers (Realtek(R) Audio)", "Microphone (Logi C615)"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(live::named(&plain, live::LINE_IN), None);
        assert_eq!(live::named(&plain, live::LINE_OUT), None);
    }
}
