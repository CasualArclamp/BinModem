//! The fax window: a picture, a page, and what the machine at the far end is.
//!
//! Everything the page needs is done here and now, before any line is
//! involved: a fax is a page long before it is a signal, and the part that
//! decides whether the far end can read it is the part that turns a
//! photograph into ink and paper.

use eframe::egui;
use egui::{Color32, RichText};
use fax::page::{Grey, Halftone, Page, Resolution};
use fax::t30;

/// What the window is holding.
///
/// Not `Debug`: a texture handle is not, and a page is a megabyte of booleans
/// nobody wants printed.
#[derive(Default)]
pub struct Fax {
    pub open: bool,
    /// Where the picture came from, and what went wrong if it did not.
    pub path: String,
    pub trouble: Option<String>,
    /// The picture, kept so that the page can be made again when the
    /// resolution or the halftone changes without going back to the file.
    source: Option<Grey>,
    source_size: (usize, usize),
    page: Option<Page>,
    /// A small copy of the page for the window to draw.
    preview: Option<egui::TextureHandle>,
    pub resolution: Resolution,
    pub halftone: Halftone,
    /// Which modulations this end is willing to use.
    pub v27ter: bool,
    pub v29: bool,
    pub v17: bool,
    /// What the far end said, once it has said anything.
    pub far: Option<t30::Capabilities>,
    pub far_identity: String,
    /// Where the call has got to, straight off the modem.
    pub phase: Option<&'static str>,
    /// How far through the page, at what rate, and how many lines have
    /// arrived. Straight off the modem as well.
    pub progress: Option<f64>,
    pub rate: u32,
    pub lines: usize,
    pub sending: bool,
    /// Whether the modem is a fax rather than a modem just now.
    pub fax_class: bool,
    /// The number to dial, and what this end calls itself.
    pub number: String,
    pub identification: String,
    /// A page that arrived, and a small copy of it to draw.
    incoming: Option<Page>,
    incoming_preview: Option<egui::TextureHandle>,
    /// What was said about saving the last page.
    pub saved: Option<String>,
}

/// What the window is asking the modem to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// Dial, and send whatever page is loaded.
    Dial(String),
    /// Go off hook and take whatever arrives.
    Answer,
}

/// The preview is drawn at a size a window can hold, not at 1728 across.
const PREVIEW_WIDTH: usize = 288;

impl Fax {
    pub fn new() -> Self {
        Self {
            resolution: Resolution::Standard,
            halftone: Halftone::Threshold,
            v27ter: true,
            // Neither is written yet, so neither is offered. A DIS that lists
            // a modulation this end cannot raise is an invitation to a call
            // that dies at the training check.
            v29: false,
            v17: false,
            ..Self::default()
        }
    }

    /// The modulations the checkboxes come to.
    pub fn ours(&self) -> Vec<t30::Modulation> {
        let mut out = Vec::new();
        if self.v27ter {
            out.push(t30::Modulation::V27ter);
        }
        if self.v29 {
            out.push(t30::Modulation::V29);
        }
        if self.v17 {
            out.push(t30::Modulation::V17);
        }
        out
    }

    /// Read a picture off the disc and keep it as grey.
    ///
    /// Everything becomes luminance and then ink: a fax has one bit per pel
    /// and no notion of colour at all, so the sooner the colour goes the
    /// fewer places there are to get it wrong.
    pub fn load(&mut self, path: &std::path::Path) {
        self.trouble = None;
        self.page = None;
        self.preview = None;
        let decoded = match image::open(path) {
            Ok(image) => image,
            Err(e) => {
                self.trouble = Some(format!("{e}"));
                self.source = None;
                return;
            }
        };
        let grey = decoded.to_luma8();
        let (w, h) = (grey.width() as usize, grey.height() as usize);
        self.source_size = (w, h);
        self.source = Some(Grey {
            width: w,
            height: h,
            // Ink, so a white page is nothing and a black one is everything.
            // The image is the other way round, which is the only reason this
            // subtraction exists.
            ink: grey.pixels().map(|p| 1.0 - f32::from(p.0[0]) / 255.0).collect(),
        });
        self.path = path.display().to_string();
        self.render();
    }

    /// Make the page from the picture already loaded.
    pub fn render(&mut self) {
        let Some(source) = self.source.as_ref() else { return };
        self.page = Some(fax::page::render(source, self.resolution, self.halftone));
        self.preview = None;
    }

    /// The page shrunk to something a window can show.
    ///
    /// Averaged rather than sampled, so that a dithered photograph looks like
    /// the tones it stands for instead of like a moire pattern -- which is
    /// what a fax looks like to the eye at arm's length, and the only honest
    /// way to show what is about to be sent.
    fn thumbnail(page: &Page) -> egui::ColorImage {
        // One preview pixel per block of the page, square in page pels, so
        // the thumbnail has the same shape as the page and not the paper.
        // Which is the right way round here: this is a picture of the data.
        let across = (fax::page::WIDTH / PREVIEW_WIDTH).max(1);
        let rows = (page.height() / across).max(1);
        let mut pixels = Vec::with_capacity(PREVIEW_WIDTH * rows);
        for ry in 0..rows {
            for rx in 0..PREVIEW_WIDTH {
                let mut ink = 0usize;
                let mut n = 0usize;
                for y in ry * across..((ry + 1) * across).min(page.height()) {
                    for x in rx * across..((rx + 1) * across).min(fax::page::WIDTH) {
                        ink += usize::from(page.lines[y][x]);
                        n += 1;
                    }
                }
                let v = (255 - ink * 255 / n.max(1)) as u8;
                pixels.push(Color32::from_rgb(v, v, v));
            }
        }
        egui::ColorImage {
            size: [PREVIEW_WIDTH, rows],
            pixels,
            source_size: egui::Vec2::new(PREVIEW_WIDTH as f32, rows as f32),
        }
    }

    fn texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if self.preview.is_none() {
            let page = self.page.as_ref()?;
            let image = Self::thumbnail(page);
            self.preview =
                Some(ctx.load_texture("fax-page", image, egui::TextureOptions::LINEAR));
        }
        self.preview.clone()
    }

    /// Take what the modem knows about the call in progress.
    pub fn observe(&mut self, frame: &telemetry::Frame) {
        self.phase = frame.fax_phase;
        self.progress = frame.fax_progress;
        self.rate = frame.fax_rate;
        self.lines = frame.fax_lines;
        self.sending = frame.fax_sending;
        self.fax_class = frame.fax_class;
        if !frame.fax_identity.is_empty() {
            self.far_identity = frame.fax_identity.clone();
        }
        if let Some(fif) = frame.fax_capabilities.as_deref() {
            self.far = Some(t30::capabilities(fif));
        }
        if let Some(trouble) = frame.fax_trouble.as_deref() {
            self.trouble = Some(trouble.to_owned());
        }
    }

    /// The page that is ready to send, if one is.
    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref()
    }

    /// A page has arrived.
    pub fn arrived(&mut self, page: Page) {
        self.incoming = Some(page);
        self.incoming_preview = None;
        self.saved = None;
    }

    /// Write a received page out as a picture.
    ///
    /// Each scan line is drawn as many pixels tall as it takes to make the
    /// pels square, because they are not: a fax is 8.05 pels per millimetre
    /// across and 3.85 or 7.7 lines per millimetre down it. Saved at the pel
    /// grid it came in on, a standard-resolution page is half the height it
    /// should be and everything on it looks squashed.
    pub fn save(&mut self, path: &std::path::Path) {
        let Some(page) = self.incoming.as_ref() else { return };
        let tall = (fax::page::PELS_PER_MM / page.resolution.lines_per_mm())
            .round()
            .max(1.0) as usize;
        let width = fax::page::WIDTH;
        let height = page.lines.len() * tall;
        let mut pixels = Vec::with_capacity(width * height);
        for line in &page.lines {
            for _ in 0..tall {
                pixels.extend(line.iter().map(|ink| if *ink { 0u8 } else { 255u8 }));
            }
        }
        let image: image::GrayImage =
            match image::ImageBuffer::from_raw(width as u32, height as u32, pixels) {
                Some(image) => image,
                None => {
                    self.saved = Some("the page did not come out rectangular".to_owned());
                    return;
                }
            };
        self.saved = Some(match image.save(path) {
            Ok(()) => format!("saved to {}", path.display()),
            Err(e) => format!("{e}"),
        });
    }

    /// Draw the window. Returns what the user asked the modem to do.
    pub fn show(&mut self, ui: &mut egui::Ui, on_hook: bool) -> Option<Start> {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        let mut open = self.open;
        let mut start = None;
        egui::Window::new("T.30 - fax")
            .open(&mut open)
            .resizable(false)
            .default_width(620.0)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Browse").clicked()
                        && let Some(chosen) = rfd::FileDialog::new()
                            .add_filter("pictures", &["png", "jpg", "jpeg", "bmp", "gif"])
                            .pick_file()
                    {
                        self.load(&chosen);
                    }
                    let shown = if self.path.is_empty() {
                        "no picture".to_owned()
                    } else {
                        self.path.clone()
                    };
                    ui.label(RichText::new(shown).monospace().color(dim));
                });
                if let Some(trouble) = &self.trouble {
                    ui.label(
                        RichText::new(trouble).color(Color32::from_rgb(235, 100, 90)),
                    );
                }

                ui.separator();
                let mut again = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("paper").monospace().color(dim));
                    for r in [Resolution::Standard, Resolution::Fine] {
                        if ui
                            .selectable_label(self.resolution == r, r.name())
                            .clicked()
                        {
                            self.resolution = r;
                            again = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ink   ").monospace().color(dim));
                    for h in [Halftone::Threshold, Halftone::Diffuse] {
                        if ui.selectable_label(self.halftone == h, h.name()).clicked() {
                            self.halftone = h;
                            again = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("offer ").monospace().color(dim));
                    ui.checkbox(&mut self.v27ter, "V.27ter")
                        .on_hover_text("2400 and 4800, and the one every fax has");
                    ui.add_enabled_ui(false, |ui| {
                        ui.checkbox(&mut self.v29, "V.29")
                            .on_hover_text("7200 and 9600. Not written yet");
                        ui.checkbox(&mut self.v17, "V.17")
                            .on_hover_text(
                                "7200 to 14 400, trellis coded. Not written yet",
                            );
                    });
                });
                if again {
                    self.render();
                }

                ui.separator();
                let texture = self.texture(ui.ctx());
                ui.horizontal_top(|ui| {
                    if let Some(texture) = texture {
                        let size = texture.size_vec2();
                        let height = (size.y * 260.0 / size.x).min(340.0);
                        ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(egui::vec2(260.0, height)),
                        );
                    } else {
                        ui.label(
                            RichText::new("Nothing to send yet.")
                                .small()
                                .color(dim),
                        );
                    }
                    ui.vertical(|ui| {
                        if let Some(page) = self.page.as_ref() {
                            let bits = page.encode().len();
                            let rows = [
                                (
                                    "picture",
                                    format!(
                                        "{} by {}",
                                        self.source_size.0, self.source_size.1
                                    ),
                                ),
                                (
                                    "page",
                                    format!("{} by {} pels", page.width(), page.height()),
                                ),
                                ("ink", format!("{:.1}% of the paper", page.coverage() * 100.0)),
                                ("coded", format!("{} bits, MH", bits)),
                            ];
                            egui::Grid::new("fax-page")
                                .num_columns(2)
                                .spacing([10.0, 4.0])
                                .show(ui, |ui| {
                                    for (k, v) in rows {
                                        ui.label(RichText::new(k).monospace().color(dim));
                                        ui.label(
                                            RichText::new(v).monospace().color(bright),
                                        );
                                        ui.end_row();
                                    }
                                    // What it costs at each rate this end is
                                    // willing to use, which is the number
                                    // anybody actually wants from this window.
                                    for m in self.ours() {
                                        for rate in m.rates() {
                                            ui.label(
                                                RichText::new(format!("at {rate}"))
                                                    .monospace()
                                                    .color(dim),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{:.0} s  {}",
                                                    page.seconds_at(*rate),
                                                    m.name()
                                                ))
                                                .monospace()
                                                .color(bright),
                                            );
                                            ui.end_row();
                                        }
                                    }
                                });
                        }
                    });
                });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("dial ").monospace().color(dim));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.number)
                            .desired_width(150.0)
                            .hint_text("a fax number"),
                    );
                    ui.label(RichText::new("as").monospace().color(dim));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.identification)
                            .desired_width(140.0)
                            .hint_text("this end's number"),
                    )
                    .on_hover_text(
                        "Sent as a TSI or a CSI. Digits, spaces and a plus, \
                         and blank is allowed: plenty of machines send nothing",
                    );
                    ui.add_enabled_ui(on_hook, |ui| {
                        if ui
                            .button("Send fax")
                            .on_hover_text(
                                "AT+FCLASS=1 and then ATD. The calling tone \
                                 goes out, whatever answers says what it is, \
                                 and then the page follows",
                            )
                            .clicked()
                        {
                            start = Some(Start::Dial(self.number.trim().to_owned()));
                        }
                        if ui
                            .button("Wait for a fax")
                            .on_hover_text(
                                "AT+FCLASS=1 and then ATA. This end answers \
                                 with the 2100 Hz tone, says what it can \
                                 receive, and keeps whatever arrives",
                            )
                            .clicked()
                        {
                            start = Some(Start::Answer);
                        }
                    });
                });
                self.call_progress(ui, on_hook, dim, bright);

                self.incoming_page(ui, dim, bright);

                ui.separator();
                ui.label(RichText::new("the machine at the far end").color(dim));
                match self.far.as_ref() {
                    None => {
                        ui.label(
                            RichText::new(
                                "Nothing yet. It says what it is in a DIS, which \
                                 arrives a few seconds into the call.",
                            )
                            .small()
                            .color(dim),
                        );
                    }
                    Some(caps) => {
                        egui::Grid::new("fax-far")
                            .num_columns(2)
                            .spacing([10.0, 4.0])
                            .show(ui, |ui| {
                                if !self.far_identity.is_empty() {
                                    ui.label(
                                        RichText::new("identity").monospace().color(dim),
                                    );
                                    ui.label(
                                        RichText::new(&self.far_identity)
                                            .monospace()
                                            .color(bright),
                                    );
                                    ui.end_row();
                                }
                                for (k, v) in caps.rows() {
                                    ui.label(RichText::new(k).monospace().color(dim));
                                    ui.label(RichText::new(v).monospace().color(bright));
                                    ui.end_row();
                                }
                                ui.label(RichText::new("both ends").monospace().color(dim));
                                let shared = caps.best_shared(&self.ours());
                                ui.label(
                                    RichText::new(match shared {
                                        Some((m, rate)) => {
                                            format!("{} at {rate} bit/s", m.name())
                                        }
                                        None => "nothing in common".to_owned(),
                                    })
                                    .monospace()
                                    .color(match shared {
                                        Some(_) => Color32::from_rgb(90, 220, 130),
                                        None => Color32::from_rgb(235, 100, 90),
                                    }),
                                );
                                ui.end_row();
                            });
                    }
                }
            });
        self.open = open;
        start
    }

    /// Where the call has got to, and how much of the page has moved.
    fn call_progress(
        &mut self,
        ui: &mut egui::Ui,
        on_hook: bool,
        dim: Color32,
        bright: Color32,
    ) {
        if on_hook {
            // A modem stays whatever class it was last told, which is also a
            // good way to dial a bulletin board and greet it with a calling
            // tone. Worth saying, since nothing else on the panel does.
            if self.fax_class {
                ui.label(
                    RichText::new(
                        "The modem is in fax class. AT+FCLASS=0 makes it a \
                         modem again.",
                    )
                    .small()
                    .color(dim),
                );
            }
            return;
        }
        let what = match self.phase {
            Some(p) => p.to_owned(),
            None => "a call, but not a fax".to_owned(),
        };
        let side = if self.sending { "sending" } else { "receiving" };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{side}: {what}")).small().color(bright));
            if self.rate > 0 {
                ui.label(
                    RichText::new(format!("at {} bit/s", self.rate))
                        .small()
                        .color(dim),
                );
            }
            if !self.sending && self.lines > 0 {
                ui.label(
                    RichText::new(format!("{} lines", self.lines))
                        .small()
                        .color(dim),
                );
            }
        });
        if let Some(fraction) = self.progress {
            ui.add(
                egui::ProgressBar::new(fraction as f32)
                    .desired_height(8.0)
                    .show_percentage(),
            );
        }
    }

    /// The page that arrived, and what to do with it.
    fn incoming_page(&mut self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        let Some(page) = self.incoming.as_ref() else { return };
        let lines = page.lines.len();
        let coverage = page.coverage();
        let resolution = page.resolution.name();
        if self.incoming_preview.is_none() {
            let image = Self::thumbnail(page);
            self.incoming_preview = Some(ui.ctx().load_texture(
                "fax-incoming",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
        ui.separator();
        ui.label(RichText::new("the page that arrived").color(dim));
        ui.horizontal_top(|ui| {
            if let Some(texture) = self.incoming_preview.clone() {
                let size = texture.size_vec2();
                let height = (size.y * 200.0 / size.x).min(300.0);
                ui.add(
                    egui::Image::new(&texture)
                        .fit_to_exact_size(egui::vec2(200.0, height)),
                );
            }
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!("{lines} lines, {resolution}"))
                        .monospace()
                        .color(bright),
                );
                ui.label(
                    RichText::new(format!(
                        "{:.1}% of the paper is ink",
                        coverage * 100.0
                    ))
                    .monospace()
                    .color(dim),
                );
                if ui.button("Save as PNG").clicked()
                    && let Some(chosen) = rfd::FileDialog::new()
                        .add_filter("pictures", &["png"])
                        .set_file_name("fax.png")
                        .save_file()
                {
                    self.save(&chosen);
                }
                if let Some(saved) = &self.saved {
                    ui.label(RichText::new(saved).small().color(dim));
                }
            });
        });
    }

    /// What to type at the modem to start one.
    pub fn commands(start: &Start) -> String {
        // The class first: it decides what the dial does, and a modem told to
        // dial before it is told what it is places a data call.
        match start {
            Start::Dial(number) => format!("AT+FCLASS=1\rATD{number}\r"),
            Start::Answer => "AT+FCLASS=1\rATA\r".to_owned(),
        }
    }
}
