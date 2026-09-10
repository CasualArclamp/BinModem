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
}

/// The preview is drawn at a size a window can hold, not at 1728 across.
const PREVIEW_WIDTH: usize = 288;

impl Fax {
    pub fn new() -> Self {
        Self {
            resolution: Resolution::Standard,
            halftone: Halftone::Threshold,
            v27ter: true,
            v29: true,
            v17: true,
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

    /// Draw the window. Returns true if the user asked to start a fax.
    pub fn show(&mut self, ui: &mut egui::Ui, online: bool) -> bool {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        let mut open = self.open;
        let mut start = false;
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
                    ui.checkbox(&mut self.v29, "V.29").on_hover_text("7200 and 9600");
                    ui.checkbox(&mut self.v17, "V.17")
                        .on_hover_text("7200 to 14 400, trellis coded");
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
                ui.add_enabled_ui(online && self.page.is_some(), |ui| {
                    if ui
                        .button("Start fax")
                        .on_hover_text(
                            "ATD and then T.30: the answering machine's \
                             identification and capabilities first, then the page",
                        )
                        .clicked()
                    {
                        start = true;
                    }
                });
                if !online {
                    ui.label(
                        RichText::new("There is no call. A fax needs one.")
                            .small()
                            .color(dim),
                    );
                }

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
}
