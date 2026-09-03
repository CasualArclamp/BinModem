//! The scope window.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use eframe::egui::{self, Color32, FontId, RichText};
use line::{AudioSink, Monitor};
use telemetry::{Direction, Frame, LogEntry, Subscriber};

use crate::engine::{Control, FFT_SIZE, SCOPE_LEN, SPECTRUM_BINS};
use crate::scopes::{self, Waterfall};

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
    follow_log: bool,
    // Monitoring. The cpal stream is not Send on Windows, so it has to live on
    // the thread that created it: this one.
    sink: Arc<AudioSink>,
    monitor: Option<Monitor>,
    devices: Vec<String>,
    chosen_device: usize,
    audio_error: Option<String>,
    sample_rate: f64,
}

impl ScopeApp {
    pub fn new(
        rx: Subscriber,
        control: Arc<Control>,
        sink: Arc<AudioSink>,
        sample_rate: f64,
    ) -> Self {
        Self {
            rx,
            control,
            frame: Frame::new(SCOPE_LEN, SPECTRUM_BINS, sample_rate),
            waterfall: Waterfall::new(WATERFALL_W, WATERFALL_H),
            log: Vec::new(),
            last_seq: 0,
            follow_log: true,
            sink,
            monitor: None,
            devices: line::output_devices(),
            chosen_device: 0,
            audio_error: None,
            sample_rate,
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
        let new = self.rx.log_since(self.log.len());
        self.log.extend(new);
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
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
            let mut speed = self.control.speed_pct.load(Ordering::Relaxed) as f32 / 100.0;
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

    /// Label for the symbol scope, matching the modulation in use.
    fn symbol_label(&self) -> String {
        if self.frame.constellation.is_empty() {
            format!("{}FSK", self.frame.tones)
        } else {
            format!("{}PSK", self.frame.tones)
        }
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

        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(180.0)
            .show(ui, |ui| self.transcript(ui));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.label(RichText::new("waterfall  (0 - 4000 Hz)").strong());
            let available = (ui.available_height() - 150.0).max(140.0);
            self.waterfall.paint(ui, available * 0.60);
            ui.add_space(6.0);
            ui.label(RichText::new("spectrum").strong());
            scopes::spectrum(
                ui,
                &self.frame.spectrum_db,
                self.frame.hz_per_bin,
                (available * 0.40).max(90.0),
                self.waterfall.floor_db,
                self.waterfall.ceiling_db,
            );
            ui.add_space(6.0);
            ui.label(RichText::new("discriminator  (answer band)").strong());
            scopes::discriminator(ui, &self.frame.baseband, 84.0);
        });
    }
}

/// Compile-time reminder that the engine and UI agree on the FFT size.
const _: () = assert!(FFT_SIZE / 2 == SPECTRUM_BINS);
