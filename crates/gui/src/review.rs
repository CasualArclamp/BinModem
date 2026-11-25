//! The Review tab: a recording of somebody else's call, laid out to be read.
//!
//! Time runs left to right and frequency up, a waterfall for each end where
//! the recording has an end a channel. What `review` found is drawn on it as
//! boxes, each over the stretch of time and the band it names, with a few
//! letters beside it and no more: the waterfall is for seeing where things
//! are and how they follow one another. Everything that was read out of a
//! signal is in the panel beside it, for whichever box was last clicked.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, pos2, vec2,
};
use line::{AudioSink, Monitor};
use review::{End, Event, Layer, Plot, Report, Tap, Waterfall};

use crate::scopes::{self, PcmView, heat};

const DIM: Color32 = Color32::from_rgb(140, 150, 165);
const BRIGHT: Color32 = Color32::from_rgb(220, 225, 235);
const GRID: Color32 = Color32::from_rgb(58, 62, 72);
const BACK: Color32 = Color32::from_rgb(9, 11, 14);
/// The calling modem's colour, and the answering modem's.
const CALLER: Color32 = Color32::from_rgb(255, 120, 105);
const ANSWERER: Color32 = Color32::from_rgb(105, 180, 255);
const EITHER: Color32 = Color32::from_rgb(215, 215, 200);
const PICKED: Color32 = Color32::from_rgb(255, 240, 150);

/// Width of the panel the details are in.
pub const DETAIL_W: f32 = 330.0;
/// Columns of waterfall to a texture: well inside what any card will take.
const TILE: usize = 2048;
/// The level the bottom of the colour ramp stands for, and the top, in dBFS.
const FLOOR_DB: f32 = -88.0;
const CEILING_DB: f32 = -10.0;

fn colour(end: End) -> Color32 {
    match end {
        End::Caller => CALLER,
        End::Answerer => ANSWERER,
        End::Line => EITHER,
    }
}

/// The part of the call a layer belongs to, for the strip along the top.
fn part(layer: Layer) -> (usize, &'static str) {
    match layer {
        Layer::Tone | Layer::V8bis | Layer::V8 => (0, "phase 1  set-up"),
        Layer::Startup => (2, "training and rates"),
        Layer::Phase2 => (1, "phase 2  probing"),
        Layer::Phase3 => (2, "phase 3  training"),
        Layer::Phase4 => (3, "phase 4  parameters"),
        Layer::Data | Layer::Link | Layer::Payload => (4, "data"),
        Layer::Fax => (4, "fax"),
    }
}

/// One channel's waterfall as textures.
struct Tiles {
    /// Each with the column it starts at and how many it holds.
    tiles: Vec<(TextureHandle, usize, usize)>,
    columns_per_sec: f64,
    hz_top: f64,
}

impl Tiles {
    fn new(ctx: &egui::Context, name: &str, waterfall: &Waterfall) -> Self {
        let columns = waterfall.columns();
        let bins = waterfall.bins;
        let span = CEILING_DB - FLOOR_DB;
        let ramp: Vec<Color32> = (0..=255u32)
            .map(|cell| {
                let db = cell as f32 / 255.0 * -(Waterfall::FLOOR_DB as f32) + Waterfall::FLOOR_DB as f32;
                heat((db - FLOOR_DB) / span)
            })
            .collect();
        let mut tiles = Vec::new();
        let mut first = 0;
        while first < columns {
            let count = TILE.min(columns - first);
            let mut image = ColorImage::filled([count, bins], BACK);
            for x in 0..count {
                let column = waterfall.column(first + x);
                for (bin, &cell) in column.iter().enumerate() {
                    // Low frequencies at the bottom.
                    image.pixels[(bins - 1 - bin) * count + x] = ramp[cell as usize];
                }
            }
            let texture = ctx.load_texture(format!("review-{name}-{first}"), image, TextureOptions::LINEAR);
            tiles.push((texture, first, count));
            first += count;
        }
        Self { tiles, columns_per_sec: waterfall.columns_per_sec, hz_top: bins as f64 * waterfall.hz_per_bin }
    }
}

/// What is on the screen of a recording, and where the reader is in it.
pub struct ReviewPane {
    report: Option<Report>,
    path: Option<PathBuf>,
    tiles: Vec<Tiles>,
    loading: Option<(PathBuf, Receiver<Result<Report, String>>)>,
    error: Option<String>,
    /// The left edge of the view in seconds, and how many seconds it is wide.
    view: (f64, f64),
    selected: Option<usize>,
    /// How PCM symbols are drawn: the ladder by default, which is what a
    /// DIL is.
    pcm_view: PcmView,
    /// Whether the symbols are also in a window of their own, at a size
    /// where a few hundred levels can be told apart.
    symbols_open: bool,
    /// Where listening starts from, in seconds: where the picture was last
    /// clicked, or where playing last got to.
    playhead: f64,
    playing: Option<Playing>,
    /// Which channels are heard: None for both.
    heard: Option<usize>,
    volume: f32,
    sink: Option<Arc<AudioSink>>,
    monitor: Option<Monitor>,
    audio_error: Option<String>,
}

/// A stretch of the recording being played.
struct Playing {
    /// First and last sample of it.
    from: usize,
    to: usize,
    /// How many have been handed to the sound card.
    pushed: usize,
    started: Instant,
}

/// How far ahead of the sound the queue is kept, in seconds.
const LEAD: f64 = 0.15;

impl Default for ReviewPane {
    fn default() -> Self {
        Self {
            report: None,
            path: None,
            tiles: Vec::new(),
            loading: None,
            error: None,
            view: (0.0, 30.0),
            selected: None,
            pcm_view: PcmView::Ladder,
            symbols_open: false,
            playhead: 0.0,
            playing: None,
            heard: None,
            volume: 0.25,
            sink: None,
            monitor: None,
            audio_error: None,
        }
    }
}

impl ReviewPane {
    /// Start reading a recording. The reading is done off the window's
    /// thread: a long one takes a few seconds.
    pub fn open(&mut self, path: &Path) {
        let (tx, rx) = channel();
        let owned = path.to_path_buf();
        let for_thread = owned.clone();
        std::thread::spawn(move || {
            let name =
                for_thread.file_name().map_or_else(|| "recording".to_owned(), |n| n.to_string_lossy().into_owned());
            let result = line::wav::read(&for_thread)
                .map(|wav| review::analyse(&wav, &name))
                .map_err(|error| format!("{}: {error}", for_thread.display()));
            let _ = tx.send(result);
        });
        self.loading = Some((owned, rx));
        self.error = None;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some((path, rx)) = &self.loading else { return };
        match rx.try_recv() {
            Ok(Ok(report)) => {
                self.tiles = report
                    .channels
                    .iter()
                    .enumerate()
                    .map(|(index, channel)| Tiles::new(ctx, &index.to_string(), &channel.waterfall))
                    .collect();
                self.view = (0.0, report.seconds.max(1.0));
                self.selected = None;
                self.playing = None;
                self.playhead = 0.0;
                // The sound card is opened for this recording's rate.
                self.monitor = None;
                self.sink = None;
                // For a picture of the window with nobody at it: the view
                // and the signal picked, as "start,seconds,tag".
                if let Ok(wanted) = std::env::var("BINMODEM_REVIEW_VIEW") {
                    let mut parts = wanted.splitn(3, ',');
                    let start = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
                    let span = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
                    if let (Some(start), Some(span)) = (start, span) {
                        self.view = (start, span);
                    }
                    if let Some(tag) = parts.next() {
                        self.selected = report.events.iter().position(|e| e.tag == tag.trim());
                    }
                }
                self.path = Some(path.clone());
                self.report = Some(report);
                self.loading = None;
            }
            Ok(Err(error)) => {
                self.error = Some(error);
                self.loading = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.error = Some("the reading of the recording stopped without a result".to_owned());
                self.loading = None;
            }
        }
    }

    /// Play from `start` to `end`, in seconds.
    fn play(&mut self, start: f64, end: f64) {
        let Some(report) = &self.report else { return };
        let fs = f64::from(report.sample_rate);
        let length = report.channels.first().map_or(0, |c| c.samples.len());
        let from = ((start.max(0.0) * fs) as usize).min(length);
        let to = ((end * fs) as usize).clamp(from, length);
        if to == from {
            return;
        }
        self.audio_error = None;
        let sink = self.sink.get_or_insert_with(|| Arc::new(AudioSink::new((fs * 0.5) as usize))).clone();
        // Off and on again empties whatever the last stretch left queued.
        sink.set_enabled(false);
        sink.set_enabled(true);
        sink.set_volume(self.volume);
        if self.monitor.is_none() {
            match line::listen(sink, None, fs) {
                Ok(monitor) => self.monitor = Some(monitor),
                Err(error) => {
                    self.audio_error = Some(error);
                    return;
                }
            }
        }
        self.playing = Some(Playing { from, to, pushed: from, started: Instant::now() });
    }

    fn stop(&mut self) {
        self.playing = None;
        if let Some(sink) = &self.sink {
            sink.set_enabled(false);
        }
    }

    /// Hand the sound card what it is due, and move the playhead.
    fn pump(&mut self, ctx: &egui::Context) {
        let (Some(report), Some(playing), Some(sink)) = (&self.report, &mut self.playing, &self.sink) else { return };
        let fs = f64::from(report.sample_rate);
        let elapsed = playing.started.elapsed().as_secs_f64();
        let wanted = (playing.from + ((elapsed + LEAD) * fs) as usize).min(playing.to);
        if wanted > playing.pushed {
            let block: Vec<f32> = (playing.pushed..wanted)
                .map(|n| match self.heard {
                    Some(channel) => report.channels.get(channel).and_then(|c| c.samples.get(n)).copied().unwrap_or(0.0),
                    None => {
                        let sum: f32 = report.channels.iter().map(|c| c.samples.get(n).copied().unwrap_or(0.0)).sum();
                        sum / report.channels.len().max(1) as f32
                    }
                })
                .collect();
            sink.set_volume(self.volume);
            sink.push(&block);
            playing.pushed = wanted;
        }
        let at = playing.from + (elapsed * fs) as usize;
        self.playhead = at.min(playing.to) as f64 / fs;
        if at >= playing.to + (LEAD * fs) as usize {
            self.playing = None;
        } else {
            // Keep the playhead on the picture.
            if self.playhead > self.view.0 + self.view.1 || self.playhead < self.view.0 {
                self.view.0 = (self.playhead - 0.05 * self.view.1).max(0.0);
            }
            ctx.request_repaint();
        }
    }

    /// The whole tab.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        self.pump(ui.ctx());
        // Space starts and stops, as it does everywhere sound is played.
        if self.report.is_some() && ui.input(|i| i.key_pressed(egui::Key::Space)) {
            if self.playing.is_some() {
                self.stop();
            } else {
                self.play(self.playhead, f64::MAX);
            }
        }
        // A recording dropped on the window is a recording to read.
        let dropped = ui.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty())
        });
        if let Some(path) = dropped {
            self.open(&path);
        }
        self.symbols_window(ui.ctx());
        egui::Panel::right("review-detail").resizable(false).exact_size(DETAIL_W).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("review-detail-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(DETAIL_W - 16.0);
                if let Some((start, end)) = self.detail(ui) {
                    self.play(start, end);
                }
            });
        });
        self.bar(ui);
        ui.add_space(2.0);
        if self.report.is_some() {
            self.picture(ui);
        } else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("no recording open").color(DIM));
                ui.label(
                    RichText::new(
                        "Open a WAV of a call, or drop one on the window. Two channels with an end on each can be followed furthest; one channel with both ends on it as far as the two share a band.",
                    )
                    .small()
                    .color(DIM),
                );
            });
        }
    }

    fn bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open recording").clicked() {
                #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
                let picked = rfd::FileDialog::new()
                    .add_filter("recordings", &["wav"])
                    .pick_file();
                #[cfg(all(target_os = "windows", target_arch = "x86"))]
                let picked = crate::xp_compat::pick_open_file(
                    "Open recording",
                    &[("recordings (*.wav)", "*.wav")],
                    None,
                );
                if let Some(chosen) = picked {
                    self.open(&chosen);
                }
            }
            if let Some((path, _)) = &self.loading {
                ui.spinner();
                ui.label(RichText::new(format!("reading {}", path.display())).small().color(DIM));
                return;
            }
            if let Some(error) = &self.error {
                ui.label(RichText::new(error).small().color(Color32::from_rgb(235, 100, 90)));
            }
            let Some(report) = &self.report else { return };
            ui.label(RichText::new(&report.name).monospace().color(BRIGHT));
            let tap = match report.tap {
                Tap::Summed => "both ends on one channel".to_owned(),
                Tap::Split { .. } => "an end a channel".to_owned(),
            };
            ui.label(
                RichText::new(format!(
                    "{:.1} s  {} Hz  {tap}  {} signals",
                    report.seconds,
                    report.sample_rate,
                    report.events.len()
                ))
                .small()
                .color(DIM),
            );
            let seconds = report.seconds.max(1.0);
            let two = report.channels.len() > 1;
            let ends: Vec<(usize, &'static str)> =
                report.channels.iter().enumerate().map(|(i, c)| (i, c.from.label())).collect();
            let playing = self.playing.is_some();
            let mut toggle = false;
            if ui
                .button(if playing { "Stop" } else { "Play" })
                .on_hover_text("Play the recording from the white line, which a click on the picture moves. Space does the same")
                .clicked()
            {
                toggle = true;
            }
            if two {
                egui::ComboBox::from_id_salt("review-heard")
                    .width(84.0)
                    .selected_text(match self.heard {
                        None => "both ends",
                        Some(channel) => ends.get(channel).map_or("one end", |e| e.1),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.heard, None, "both ends");
                        for (channel, name) in &ends {
                            ui.selectable_value(&mut self.heard, Some(*channel), *name);
                        }
                    });
            }
            ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false)).on_hover_text("How loud");
            if let Some(error) = &self.audio_error {
                ui.label(RichText::new(error).small().color(Color32::from_rgb(235, 100, 90)));
            }
            if toggle {
                if playing {
                    self.stop();
                } else {
                    self.play(self.playhead, f64::MAX);
                }
                return;
            }
            let Some(report) = &self.report else { return };
            let _ = report;
            if ui
                .small_button("whole call")
                .on_hover_text(
                    "Show the whole recording, as a double click on the picture does. The wheel zooms about the pointer, a drag moves along, and the left and right arrows step from one signal to the next",
                )
                .clicked()
            {
                self.view = (0.0, seconds);
            }
        });
    }

    /// The waterfalls, the boxes on them, and the axes.
    fn picture(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.report else { return };
        // What is left of the panel this is in and no more: a row above
        // that ran wide would otherwise have this drawn under the
        // details beside it.
        let room = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let (response, painter) = ui.allocate_painter(room.size(), Sense::click_and_drag());
        let whole = response.rect;
        let seconds = report.seconds.max(0.001);

        // Room on the left for the frequencies, along the top for the parts
        // of the call, along the bottom for the time.
        let left = whole.left() + 38.0;
        let ribbon = Rect::from_min_max(pos2(left, whole.top()), pos2(whole.right(), whole.top() + 18.0));
        let axis_top = whole.bottom() - 18.0;
        let lanes_rect = Rect::from_min_max(pos2(left, ribbon.bottom() + 2.0), pos2(whole.right(), axis_top));
        let lanes = report.channels.len().max(1);
        let gap = 6.0;
        let lane_h = (lanes_rect.height() - gap * (lanes as f32 - 1.0)) / lanes as f32;
        let lane_rect = |index: usize| {
            let top = lanes_rect.top() + index as f32 * (lane_h + gap);
            Rect::from_min_max(pos2(left, top), pos2(whole.right(), top + lane_h))
        };

        // The pointer first, so that what is drawn is drawn where the view
        // has just been moved to.
        let width = (whole.right() - left).max(1.0) as f64;
        let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| whole.contains(*p));
        if let Some(p) = pointer {
            let wheel = ui.input_mut(|i| {
                let y = i.smooth_scroll_delta.y;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                y
            });
            if wheel != 0.0 {
                let at = self.view.0 + (f64::from(p.x - left) / width).clamp(0.0, 1.0) * self.view.1;
                let scale = (-f64::from(wheel) * 0.0035).exp();
                let span = (self.view.1 * scale).clamp(0.02, seconds);
                self.view.0 = at - (at - self.view.0) * span / self.view.1;
                self.view.1 = span;
            }
        }
        if response.dragged() {
            self.view.0 -= f64::from(response.drag_delta().x) / width * self.view.1;
        }
        if response.double_clicked() {
            self.view = (0.0, seconds);
        }
        self.view.1 = self.view.1.clamp(0.02, seconds);
        self.view.0 = self.view.0.clamp(0.0, (seconds - self.view.1).max(0.0));
        let (t0, span) = self.view;
        let x_of = |t: f64| left + ((t - t0) / span * width) as f32;
        let t_of = |x: f32| t0 + f64::from(x - left) / width * span;

        painter.rect_filled(whole, 0.0, BACK);

        // Which events go in which lane.
        let lane_of = |event: &Event| match report.tap {
            Tap::Summed => 0,
            Tap::Split { .. } => report.channels.iter().position(|c| c.from == event.from).unwrap_or(0),
        };

        let mut hit: Option<(usize, f32)> = None;
        for (index, channel) in report.channels.iter().enumerate() {
            let rect = lane_rect(index);
            let clipped = painter.with_clip_rect(rect);
            if let Some(tiles) = self.tiles.get(index) {
                for (texture, first, count) in &tiles.tiles {
                    let a = x_of(*first as f64 / tiles.columns_per_sec);
                    let b = x_of((*first + *count) as f64 / tiles.columns_per_sec);
                    if b < rect.left() || a > rect.right() {
                        continue;
                    }
                    clipped.image(
                        texture.id(),
                        Rect::from_min_max(pos2(a, rect.top()), pos2(b, rect.bottom())),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
            let hz_top = self.tiles.get(index).map_or(4000.0, |t| t.hz_top);
            let y_of = |hz: f64| rect.bottom() - (hz / hz_top).clamp(0.0, 1.0) as f32 * rect.height();

            // The frequencies, down the left.
            for khz in 1..(hz_top / 1000.0).ceil() as usize {
                let y = y_of(khz as f64 * 1000.0);
                painter.line_segment([pos2(left - 4.0, y), pos2(left, y)], Stroke::new(1.0, GRID));
                painter.text(pos2(left - 6.0, y), Align2::RIGHT_CENTER, format!("{khz}k"), FontId::monospace(10.0), DIM);
            }
            let who = match channel.from {
                End::Caller => "caller",
                End::Answerer => "answerer",
                End::Line => "line",
            };
            painter.text(
                pos2(whole.left() + 2.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                who,
                FontId::monospace(10.0),
                colour(channel.from),
            );

            // The boxes. Labels are placed as they come and one that would
            // sit on another is moved up a line, or left off: the box is
            // still there to be clicked.
            let mut placed: Vec<Rect> = Vec::new();
            for (number, event) in report.events.iter().enumerate() {
                if lane_of(event) != index || event.end < t0 || event.start > t0 + span {
                    continue;
                }
                let (a, b) = (x_of(event.start), x_of(event.end));
                let (low, high) = event.band.unwrap_or((0.0, hz_top));
                let frame = Rect::from_min_max(pos2(a, y_of(high)), pos2(b.max(a + 1.5), y_of(low)));
                let picked = self.selected == Some(number);
                let tint = if picked { PICKED } else { colour(event.from) };
                if picked {
                    clipped.rect_filled(frame, 0.0, PICKED.gamma_multiply(0.12));
                }
                if frame.width() < 3.0 {
                    clipped.line_segment(
                        [frame.center_top(), frame.center_bottom()],
                        Stroke::new(if picked { 2.0 } else { 1.0 }, tint),
                    );
                } else {
                    clipped.rect_stroke(frame, 0.0, Stroke::new(if picked { 2.0 } else { 1.0 }, tint), StrokeKind::Inside);
                }
                if let Some(p) = pointer
                    && frame.expand(2.0).contains(p)
                    && hit.is_none_or(|(_, area)| frame.area() < area)
                {
                    hit = Some((number, frame.area()));
                }

                let font = FontId::monospace(11.0);
                let text = clipped.layout_no_wrap(event.tag.clone(), font, tint);
                let size = text.size();
                // Above the box where there is room, inside its top where
                // there is not.
                let mut at = pos2(frame.left() + 1.0, frame.top() - size.y - 1.0);
                if at.y < rect.top() + 12.0 {
                    at.y = frame.top() + 1.0;
                }
                // A label that starts off the left of the view is brought
                // in to its edge, as long as its box is still on show.
                at.x = at.x.max(rect.left() + 2.0).min(frame.right().max(rect.left() + 2.0));
                let first = Rect::from_min_size(at, size).expand2(vec2(2.0, 0.0));
                // Where it would go, then up a line at a time while there is
                // lane above, then down inside the box.
                let step = size.y + 1.0;
                let free = |label: &Rect, placed: &[Rect]| {
                    label.top() >= rect.top() && label.bottom() <= rect.bottom() && !placed.iter().any(|o| o.intersects(*label))
                };
                let label = (0..4)
                    .map(|k| first.translate(vec2(0.0, -step * k as f32)))
                    .chain((1..9).map(|k| first.translate(vec2(0.0, step * k as f32))))
                    .find(|candidate| free(candidate, &placed));
                if let Some(label) = label
                    && label.right() > rect.left()
                {
                    clipped.rect_filled(label, 2.0, Color32::from_black_alpha(190));
                    clipped.galley(label.min + vec2(2.0, 0.0), text, tint);
                    placed.push(label);
                    if let Some(p) = pointer
                        && label.contains(p)
                    {
                        hit = Some((number, 0.0));
                    }
                }
            }
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, GRID), StrokeKind::Outside);
        }

        // The parts of the call along the top, each from its first signal to
        // the next part's.
        // A part begins where the first signal of a later part is; and
        // phase 2 again after data mode is a retrain, where they start over.
        let mut parts: Vec<(usize, &'static str, f64)> = Vec::new();
        let mut order_of: Vec<&Event> = report.events.iter().collect();
        order_of.sort_by(|a, b| a.start.total_cmp(&b.start));
        for event in order_of {
            let (order, name) = part(event.layer);
            let now = parts.last().map(|p| p.0);
            let retrain = now == Some(4) && event.layer == Layer::Phase2;
            if now.is_none_or(|n| order > n) || retrain {
                parts.push((order, if retrain { "retrain  phase 2" } else { name }, event.start));
            }
        }
        let ribbon_clip = painter.with_clip_rect(ribbon);
        for (i, (_, name, from)) in parts.iter().enumerate() {
            let until = parts.get(i + 1).map_or(seconds, |next| next.2);
            let (a, b) = (x_of(*from), x_of(until));
            if b < ribbon.left() || a > ribbon.right() {
                continue;
            }
            let bar = Rect::from_min_max(pos2(a, ribbon.top()), pos2(b, ribbon.bottom()));
            ribbon_clip.rect_filled(bar.shrink2(vec2(1.0, 1.0)), 2.0, Color32::from_rgb(30, 34, 42));
            ribbon_clip.with_clip_rect(bar.intersect(ribbon)).text(
                pos2(a.max(ribbon.left()) + 5.0, ribbon.center().y),
                Align2::LEFT_CENTER,
                *name,
                FontId::monospace(10.0),
                BRIGHT,
            );
        }

        time_axis(&painter, left, whole.right(), axis_top, t0, span);

        // Where listening is, or would start.
        let head = x_of(self.playhead);
        if head >= left && head <= whole.right() {
            painter.line_segment(
                [pos2(head, ribbon.bottom()), pos2(head, axis_top)],
                Stroke::new(if self.playing.is_some() { 2.0 } else { 1.0 }, Color32::WHITE),
            );
        }
        if response.clicked()
            && let Some(p) = pointer.filter(|p| p.x >= left)
            && self.playing.is_none()
        {
            self.playhead = t_of(p.x).clamp(0.0, seconds);
        }

        // Where the pointer is, in seconds.
        if let Some(p) = pointer.filter(|p| p.x >= left) {
            painter.line_segment(
                [pos2(p.x, lanes_rect.top()), pos2(p.x, axis_top)],
                Stroke::new(1.0, Color32::from_white_alpha(70)),
            );
            painter.text(
                pos2(p.x + 4.0, axis_top - 2.0),
                Align2::LEFT_BOTTOM,
                format!("{:.3} s", t_of(p.x)),
                FontId::monospace(10.0),
                BRIGHT,
            );
        }

        if let Some((number, _)) = hit {
            let event = &report.events[number];
            response.clone().on_hover_text_at_pointer(format!("{}  {}", event.tag, event.summary));
            if response.clicked() {
                self.selected = Some(number);
            }
        } else if response.clicked() {
            self.selected = None;
        }

        // Left and right step through the signals in the order they came.
        let step = ui.input(|i| {
            i32::from(i.key_pressed(egui::Key::ArrowRight)) - i32::from(i.key_pressed(egui::Key::ArrowLeft))
        });
        if step != 0 && !report.events.is_empty() {
            let last = report.events.len() as i32 - 1;
            let next = match self.selected {
                Some(now) => (now as i32 + step).clamp(0, last),
                None if step > 0 => 0,
                None => last,
            } as usize;
            self.selected = Some(next);
            let event = &report.events[next];
            if event.start < self.view.0 || event.end > self.view.0 + self.view.1 {
                self.view.0 = (event.start - self.view.1 * 0.3).clamp(0.0, (seconds - self.view.1).max(0.0));
            }
        }
    }

    /// The symbols of an event, drawn `height` high. True if it was clicked.
    fn symbols(ui: &mut egui::Ui, event: &Event, view: PcmView, height: f32) -> bool {
        let (tones, pcm, label) = match event.plot {
            Plot::None => return false,
            Plot::Qam(points) => (points, None, format!("{}  {} symbols", event.tag, event.symbols.len())),
            Plot::Pcm => (2, Some(view), format!("{}  {} symbols  PCM", event.tag, event.symbols.len())),
            Plot::Phasor => (4, None, format!("{}  the tone's phasor", event.tag)),
            Plot::Fsk => {
                // One line a bit, along the axis the two tones are the ends of.
                let margins: Vec<f32> = event.symbols.iter().map(|(_, m)| *m).collect();
                let label = format!("{}  {} bits  space left, mark right", event.tag, margins.len());
                return scopes::symbol_scope(
                    ui,
                    &margins,
                    scopes::Constellation { points: &[], tones: 2, peak: 1.0, pcm: None },
                    &label,
                    None,
                    height.min(150.0),
                )
                .clicked();
            }
        };
        if pcm.is_some() {
            // PCM is drawn to its own levels, whatever they are: a call's
            // are a fraction of full scale, and at full scale's size they
            // are a stripe across the middle. To all but the few largest,
            // so that one click on the line does not shrink the rest.
            let mut sizes: Vec<f32> = event.symbols.iter().map(|(_, level)| level.abs()).collect();
            sizes.sort_by(f32::total_cmp);
            let most = sizes.get(sizes.len().saturating_sub(1) * 998 / 1000).copied().unwrap_or(0.0);
            let scale = 1.0 / (1.04 * most).max(1e-6);
            let fitted: Vec<(f32, f32)> = event
                .symbols
                .iter()
                .map(|(before, level)| ((before * scale).clamp(-1.0, 1.0), (level * scale).clamp(-1.0, 1.0)))
                .collect();
            let label = format!("{label}  full height {:.0} of 32768", 32768.0 / scale);
            return scopes::symbol_scope(
                ui,
                &[],
                scopes::Constellation { points: &fitted, tones, peak: 1.0, pcm },
                &label,
                None,
                height,
            )
            .clicked();
        }
        // A constellation of unit mean power reaches a little past one.
        let peak = event.symbols.iter().map(|(a, b)| a.abs().max(b.abs())).fold(0.0f32, f32::max).clamp(1.0, 2.0);
        scopes::symbol_scope(
            ui,
            &[],
            scopes::Constellation { points: &event.symbols, tones, peak, pcm },
            &label,
            None,
            height,
        )
        .clicked()
    }

    /// The symbols of the picked event again, as large as the window they
    /// are in.
    pub fn symbols_window(&mut self, ctx: &egui::Context) {
        if !self.symbols_open {
            return;
        }
        let Some(event) = self.selected.and_then(|n| self.report.as_ref()?.events.get(n)) else { return };
        if event.plot == Plot::None {
            return;
        }
        let mut open = true;
        let mut view = self.pcm_view;
        egui::Window::new(format!("symbols: {}", event.tag))
            .id(egui::Id::new("review-symbols"))
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 780.0])
            .show(ctx, |ui| {
                if event.plot == Plot::Pcm {
                    ui.horizontal(|ui| {
                        for choice in PcmView::ALL {
                            ui.selectable_value(&mut view, choice, choice.name());
                        }
                    });
                }
                let side = ui.available_width().min(ui.available_height()).max(240.0);
                Self::symbols(ui, event, view, side);
            });
        self.pcm_view = view;
        self.symbols_open = open;
    }

    /// The panel: the call in a few lines, or the signal that was clicked.
    /// Gives back a stretch of the recording, in seconds, if the reader has
    /// asked to hear one.
    fn detail(&mut self, ui: &mut egui::Ui) -> Option<(f64, f64)> {
        ui.add_space(6.0);
        let mut wanted = None;
        let Some(report) = &self.report else {
            ui.label(RichText::new("the call").strong());
            ui.add_space(4.0);
            ui.label(RichText::new("Nothing read yet.").small().color(DIM));
            return None;
        };
        let rows = |ui: &mut egui::Ui, id: &str, rows: &[(String, String)]| {
            egui::Grid::new(id).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                for (key, value) in rows {
                    ui.label(RichText::new(key).monospace().small().color(DIM));
                    ui.add(egui::Label::new(RichText::new(value).monospace().small().color(BRIGHT)).wrap());
                    ui.end_row();
                }
            });
        };
        let Some(event) = self.selected.and_then(|n| report.events.get(n)) else {
            ui.label(RichText::new("the call").strong());
            ui.add_space(4.0);
            if report.verdict.is_empty() {
                ui.label(RichText::new("Nothing recognised in this recording.").small().color(DIM));
            }
            rows(ui, "review-verdict", &report.verdict);
            for caveat in &report.caveats {
                ui.add_space(6.0);
                ui.label(RichText::new(caveat).small().color(Color32::from_rgb(240, 180, 90)));
            }
            ui.add_space(10.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Click a box on the waterfall for everything read out of it. The wheel zooms, a drag moves along, the arrow keys step through the signals.",
                    )
                    .small()
                    .color(DIM),
                )
                .wrap(),
            );
            return None;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&event.tag).strong().monospace().color(colour(event.from)));
            ui.label(RichText::new(event.layer.label()).small().color(DIM));
            if ui
                .small_button("play this")
                .on_hover_text("Hear this signal alone, from a moment before it to a moment after")
                .clicked()
            {
                // A reversal or an S-bar is too short to hear as anything:
                // a fifth of a second at the least.
                let pad = (0.2 - event.duration()).max(0.0) / 2.0 + 0.03;
                wanted = Some((event.start - pad, event.end + pad));
            }
        });
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(&event.summary).small().color(BRIGHT)).wrap());
        ui.add_space(6.0);
        let mut head = vec![
            ("from".to_owned(), event.from.label().to_owned()),
            ("starts".to_owned(), format!("{:.3} s", event.start)),
            ("lasts".to_owned(), format!("{:.0} ms", 1000.0 * event.duration())),
        ];
        if let Some((low, high)) = event.band {
            head.push(("band".to_owned(), format!("{low:.0} to {high:.0} Hz")));
        }
        rows(ui, "review-head", &head);
        if event.plot != Plot::None {
            ui.add_space(6.0);
            if event.plot == Plot::Pcm {
                ui.horizontal(|ui| {
                    for choice in PcmView::ALL {
                        ui.selectable_value(&mut self.pcm_view, choice, RichText::new(choice.name()).small());
                    }
                });
            }
            if Self::symbols(ui, event, self.pcm_view, DETAIL_W - 24.0) {
                self.symbols_open = true;
            }
            ui.label(RichText::new("click the plot for a larger one").small().color(DIM));
        }
        if !event.fields.is_empty() {
            ui.add_space(6.0);
            ui.separator();
            rows(ui, "review-fields", &event.fields);
        }
        if !event.octets.is_empty() {
            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new(format!("{} octets", event.octets.len())).small().color(DIM));
            let dump: Vec<String> = event
                .octets
                .chunks(12)
                .map(|row| row.iter().map(|o| format!("{o:02X}")).collect::<Vec<_>>().join(" "))
                .collect();
            ui.label(RichText::new(dump.join("\n")).monospace().small().color(BRIGHT));
        }
        wanted
    }
}

/// Seconds along the bottom, at a round step that leaves the labels apart.
fn time_axis(painter: &Painter, left: f32, right: f32, top: f32, t0: f64, span: f64) {
    let width = f64::from(right - left).max(1.0);
    let wanted = span * 90.0 / width;
    let decade = 10f64.powf(wanted.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * decade).find(|s| *s >= wanted).unwrap_or(decade * 10.0);
    let places = if step >= 1.0 { 0 } else { (-step.log10()).ceil() as usize };
    let mut t = (t0 / step).ceil() * step;
    while t <= t0 + span {
        let x = left + ((t - t0) / span * width) as f32;
        painter.line_segment([pos2(x, top), pos2(x, top + 4.0)], Stroke::new(1.0, GRID));
        painter.text(
            Pos2::new(x, top + 5.0),
            Align2::CENTER_TOP,
            format!("{t:.places$} s"),
            FontId::monospace(10.0),
            DIM,
        );
        t += step;
    }
}
