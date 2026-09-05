//! The scope window.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, FontId, RichText};
use line::{AudioSink, Monitor};
use telemetry::{Direction, Frame, LogEntry, Subscriber};

use crate::console::{self, Console, Mode};
use crate::engine::{Control, FFT_SIZE, SCOPE_LEN, SPECTRUM_BINS};
use crate::live;
use crate::scopes::{self, Waterfall};

/// Where what is on the scope comes from.
pub enum Source {
    /// A recording of a call someone else placed. It can be watched, paused
    /// and slowed down, and nothing typed at it can have any effect.
    Capture,
    /// A modem of our own on a real line. The terminal below is its DTE: what
    /// is typed goes to the modem, and the modem answers for itself.
    Live(Arc<live::Session>),
}

impl Source {
    fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
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
        let pick = |names: &[String], want: &str| {
            names.iter().position(|n| n.contains(want)).unwrap_or(0)
        };
        let chosen_in = pick(&inputs, "CABLE Output");
        let chosen_out = pick(&outputs, "CABLE Input");
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
            console: if source.is_live() { Console::live() } else { Console::new() },
            source,
            line_inputs: inputs,
            line_outputs: outputs,
            // A virtual cable is almost always the right answer, so it starts
            // selected where there is one.
            chosen_input: chosen_in,
            chosen_output: chosen_out,
            carrier: 1,
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
        let response = console::view(ui, &self.console.term, self.font_size);
        if response.clicked() {
            response.request_focus();
        }
        if response.has_focus() {
            let typed = console::keys_to_bytes(ui);
            if !typed.is_empty() {
                match &self.source {
                    // Straight to the modem, which has an AT interpreter of
                    // its own and will echo, answer, and decide for itself
                    // what is a command and what is data. Nothing is parsed
                    // on this side of the line.
                    Source::Live(session) => session.type_bytes(&typed),
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

            let before = self.carrier;
            egui::ComboBox::from_id_salt("carrier")
                .width(180.0)
                .selected_text(Self::CARRIERS[self.carrier].1)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in Self::CARRIERS.iter().enumerate() {
                        ui.selectable_value(&mut self.carrier, i, *label);
                    }
                });
            if self.carrier != before {
                // Both ends have to agree: a modem listening for one of these
                // hears nothing whatever of the others.
                session.type_bytes(
                    format!("AT+MS={}\r", Self::CARRIERS[self.carrier].0).as_bytes(),
                );
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
        });
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
        if self.source.is_live() {
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
            if self.frame.state == telemetry::CallState::Connected
                && let Source::Live(session) = &self.source
            {
                let reply = self.console.term.take_reply();
                if !reply.is_empty() {
                    session.type_bytes(&reply);
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
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Terminal, "terminal");
                    ui.selectable_value(&mut self.tab, Tab::Transcript, "transcript");
                    ui.separator();
                    match self.console.mode {
                        Mode::Command => ui.label(
                            RichText::new("command state").monospace().color(
                                Color32::from_rgb(150, 160, 175),
                            ),
                        ),
                        Mode::Online => ui.label(
                            RichText::new("online").monospace().color(
                                Color32::from_rgb(90, 220, 130),
                            ),
                        ),
                    };
                    if self.tab == Tab::Terminal {
                        ui.separator();
                        ui.add(
                            egui::Slider::new(&mut self.font_size, 9.0..=22.0).text("font"),
                        );
                    }
                });
                ui.separator();
                match self.tab {
                    Tab::Terminal => {
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                            self.terminal_pane(ui)
                        });
                    }
                    Tab::Transcript => self.transcript(ui),
                }
            });

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

/// Compile-time reminder that the engine and UI agree on the FFT size.
const _: () = assert!(FFT_SIZE / 2 == SPECTRUM_BINS);
