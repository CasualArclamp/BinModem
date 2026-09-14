//! The fax window: a picture, a page, and what the machine at the far end is.
//!
//! Everything the page needs is done here and now, before any line is
//! involved: a fax is a page long before it is a signal, and the part that
//! decides whether the far end can read it is the part that turns a
//! photograph into ink and paper.

use eframe::egui;
use egui::{Color32, RichText};
use fax::coding::Coding;
use fax::page::{Grey, Halftone, Page, Resolution};
use fax::t30;

use crate::live::Arriving;

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
    /// How long the page codes to in each coding -- MH, MR and MMR -- worked
    /// out once when it is made.
    ///
    /// Coding a full page is two to four milliseconds, and the panel wants
    /// the figures once for itself and again for each rate it could go out
    /// at. All of those in every frame of a window that is open for the whole
    /// of a call is tens of milliseconds a frame spent recomputing numbers
    /// that cannot have changed.
    coded_bits: [usize; 3],
    /// A small copy of the page for the window to draw.
    preview: Option<egui::TextureHandle>,
    pub resolution: Resolution,
    pub halftone: Halftone,
    /// Which modulations this end is willing to use.
    pub v27ter: bool,
    pub v29: bool,
    pub v17: bool,
    /// Whether this end offers error correction mode.
    pub error_correction: bool,
    /// What the far end said, once it has said anything.
    pub far: Option<t30::Capabilities>,
    pub far_identity: String,
    /// The far end's NSF, if it sent one.
    pub far_non_standard: Option<Vec<u8>>,
    /// Where the call has got to, straight off the modem.
    pub phase: Option<&'static str>,
    /// How far through the page, at what rate, and how many lines have
    /// arrived. Straight off the modem as well.
    pub progress: Option<f64>,
    pub rate: u32,
    pub lines: usize,
    /// Whether the call in progress is using error correction mode, and the
    /// coding the page is in.
    pub correcting: bool,
    pub coding: &'static str,
    pub sending: bool,
    /// Whether the modem is a fax rather than a modem just now.
    pub fax_class: bool,
    /// The number to dial, and what this end calls itself.
    pub number: String,
    pub identification: String,
    /// A page that arrived.
    incoming: Option<Page>,
    /// The page arriving, or the last one that did, drawn as it came in.
    scan: Option<Scan>,
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

/// Pels of the page to one texel of the picture of it arriving, across it.
const SCAN_ACROSS: usize = 3;

/// Texels across that picture: a third of 1728, which is twice the preview of
/// the page going out -- this one is for watching, and that one for checking.
const SCAN_WIDTH: usize = fax::page::WIDTH / SCAN_ACROSS;

/// How tall the picture of a page arriving gets before it scrolls, in points.
const SCAN_HEIGHT: f32 = 440.0;

/// What the picture's texture holds below the last row drawn.
const UNSCANNED: Color32 = Color32::from_rgb(24, 28, 36);

/// How long after the last line the page still counts as arriving, in seconds.
///
/// Longer than any gap inside a page: the longest is error correction's
/// turnaround between one block and the next, a partial page signal and its
/// answer and a training sequence, which is two or three seconds at most.
const STILL_ARRIVING: f64 = 4.0;

/// What a far end's NSF comes to, as rows for its table.
///
/// How long it is, the T.35 country code it starts with, and its first few
/// octets. The rest is the maker's own and T.30 says nothing more of it
/// (5.3.6.2.7), so neither does this. The country code goes down the line
/// most significant bit first -- the other way round from every other octet
/// in a frame -- so it is turned round to be read, and T.30 warns that some
/// machines send it the wrong way round regardless.
fn non_standard_rows(fif: &[u8]) -> Vec<(&'static str, String)> {
    let Some(country) = fif.first() else {
        return vec![("non-standard", "an empty NSF".to_owned())];
    };
    let start: Vec<String> = fif.iter().take(8).map(|o| format!("{o:02x}")).collect();
    vec![
        (
            "non-standard",
            format!("{} octets, country code {:02x}", fif.len(), country.reverse_bits()),
        ),
        (
            "  starting",
            format!("{}{}", start.join(" "), if fif.len() > 8 { " ..." } else { "" }),
        ),
    ]
}

/// How big to draw `rows` rows of a page arriving, across `width` points.
///
/// The width is the width whatever the rows, and the rows are as tall as the
/// width makes them: the page's own shape, scaled to fit across and growing
/// only downwards.
fn picture_size(width: f32, rows: usize) -> egui::Vec2 {
    egui::vec2(width, rows as f32 * width / SCAN_WIDTH as f32)
}

/// A page arriving, drawn as it comes in.
///
/// The way slow-scan television draws: a row at a time from the top, so what
/// is on the paper can be seen long before the paper is finished. A row is one
/// texel for every three pels across and as many lines down as make the texel
/// square on the paper, worked out once every line under it is in and never
/// again -- so each new row costs the texture a strip of 576 texels rather
/// than the whole page every time a line comes off the decoder.
struct Scan {
    /// Which page this is, as the line numbered it.
    page: u64,
    resolution: Resolution,
    lines: Vec<Vec<bool>>,
    /// The rows finished so far, SCAN_WIDTH texels to a row.
    pixels: Vec<Color32>,
    rows: usize,
    /// When the last line came in, by the window's clock.
    last_line: f64,
    /// The rows the texture has room for, and how many it has been given.
    texture: Option<egui::TextureHandle>,
    capacity: usize,
    uploaded: usize,
}

impl Scan {
    fn new(page: u64, resolution: Resolution) -> Self {
        Self {
            page,
            resolution,
            lines: Vec::new(),
            pixels: Vec::new(),
            rows: 0,
            last_line: f64::NEG_INFINITY,
            texture: None,
            capacity: 0,
            uploaded: 0,
        }
    }

    /// Lines of the page under one row of the picture.
    ///
    /// Three pels across is 3/8.04 mm of paper, and that much paper down the
    /// page is 1.44 lines at standard and 2.87 at fine. Drawn that way a page
    /// comes out the shape of the sheet at either resolution, rather than a
    /// standard one half the height of the same page sent fine.
    fn lines_per_row(&self) -> f64 {
        SCAN_ACROSS as f64 * self.resolution.lines_per_mm() / fax::page::PELS_PER_MM
    }

    /// The lines under row `row`: at least one, since a row covers more than
    /// a line at both resolutions.
    fn span(&self, row: usize) -> std::ops::Range<usize> {
        let k = self.lines_per_row();
        (row as f64 * k).round() as usize..((row + 1) as f64 * k).round() as usize
    }

    /// Lines starting at line `from`, and every row they finish.
    ///
    /// Lines already here are skipped, which is what a batch that overlaps the
    /// page it continues needs. A batch that starts past the end leaves a gap
    /// that nothing can fill, and is left out.
    fn extend(&mut self, from: usize, lines: Vec<Vec<bool>>) {
        let have = self.lines.len();
        if from > have {
            return;
        }
        self.lines.extend(lines.into_iter().skip(have - from));
        while self.span(self.rows).end <= self.lines.len() {
            self.finish_row();
        }
    }

    /// Average the pels under the next row into it.
    fn finish_row(&mut self) {
        let span = self.span(self.rows);
        let under = &self.lines[span];
        for tx in 0..SCAN_WIDTH {
            let mut ink = 0;
            for line in under {
                for x in tx * SCAN_ACROSS..(tx + 1) * SCAN_ACROSS {
                    ink += usize::from(line.get(x).is_some_and(|pel| *pel));
                }
            }
            let v = (255 - ink * 255 / (under.len() * SCAN_ACROSS)) as u8;
            self.pixels.push(Color32::from_rgb(v, v, v));
        }
        self.rows += 1;
    }

    /// Rows that can be drawn: all of them, unless the page has run longer
    /// than the largest texture the graphics card takes.
    fn shown(&self) -> usize {
        self.rows.min(self.capacity)
    }

    /// The texture, given whatever rows have been finished since last time.
    ///
    /// Made with room for a whole sheet, so that an ordinary page only ever
    /// adds strips to it, and made again half as big again if a page runs
    /// longer -- T.30 allows unlimited length.
    fn texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if self.rows == 0 {
            return None;
        }
        let largest = ctx.input(|i| i.max_texture_side);
        let outgrown = self.rows > self.capacity && self.capacity < largest;
        if self.texture.is_none() || outgrown {
            let sheet = (self.resolution.lines() as f64 / self.lines_per_row()).ceil() as usize;
            self.capacity = sheet.max(self.rows + self.rows / 2).min(largest);
            let upto = self.shown();
            let mut pixels = self.pixels[..upto * SCAN_WIDTH].to_vec();
            pixels.resize(SCAN_WIDTH * self.capacity, UNSCANNED);
            self.texture = Some(ctx.load_texture(
                "fax-arriving",
                egui::ColorImage::new([SCAN_WIDTH, self.capacity], pixels),
                egui::TextureOptions::LINEAR,
            ));
            self.uploaded = upto;
        }
        let upto = self.shown();
        if self.uploaded < upto
            && let Some(texture) = self.texture.as_mut()
        {
            let strip = egui::ColorImage::new(
                [SCAN_WIDTH, upto - self.uploaded],
                self.pixels[self.uploaded * SCAN_WIDTH..upto * SCAN_WIDTH].to_vec(),
            );
            texture.set_partial([0, self.uploaded], strip, egui::TextureOptions::LINEAR);
            self.uploaded = upto;
        }
        self.texture.clone()
    }
}

impl Fax {
    pub fn new() -> Self {
        Self {
            resolution: Resolution::Standard,
            halftone: Halftone::Threshold,
            v27ter: true,
            v29: true,
            // Not written yet, so not offered. A DIS that lists a modulation
            // this end cannot raise is an invitation to a call that dies at the
            // training check.
            v17: false,
            error_correction: true,
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
        let page = fax::page::render(source, self.resolution, self.halftone);
        self.coded_bits = [Coding::ModifiedHuffman, Coding::ModifiedRead, Coding::Mmr]
            .map(|coding| coding.encode(&page.lines, page.resolution, 0).len());
        self.page = Some(page);
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
        self.correcting = frame.fax_error_correction;
        self.coding = frame.fax_coding;
        self.sending = frame.fax_sending;
        self.fax_class = frame.fax_class;
        if !frame.fax_identity.is_empty() {
            self.far_identity = frame.fax_identity.clone();
        }
        if let Some(fif) = frame.fax_non_standard.as_deref() {
            self.far_non_standard = Some(fif.to_vec());
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
    ///
    /// Usually it has been drawn already, line by line on the way in. If the
    /// window did not get every line of it -- or got the page before the last
    /// few, which the two threads are free to do -- the picture is made again
    /// from the page itself.
    pub fn arrived(&mut self, page: Page) {
        let drawn = self.scan.as_ref().is_some_and(|scan| {
            scan.lines.len() == page.lines.len() && scan.resolution == page.resolution
        });
        if !drawn {
            // Keep the number of the page being drawn, so the lines of it
            // still on their way are known for what they are.
            let number = self.scan.as_ref().map_or(u64::MAX, |scan| scan.page);
            let mut scan = Scan::new(number, page.resolution);
            scan.extend(0, page.lines.clone());
            self.scan = Some(scan);
        }
        self.incoming = Some(page);
        self.saved = None;
    }

    /// More lines of a page arriving. True if they start a page not seen
    /// before.
    pub fn arriving(&mut self, arriving: Arriving, now: f64) -> bool {
        // A new number starting at the top is a new page. A new number part
        // way down is the page this window already has: it made the picture
        // from the finished page before it was told which number that was.
        let fresh = self
            .scan
            .as_ref()
            .is_none_or(|scan| scan.page != arriving.page && arriving.from == 0);
        if fresh {
            self.scan = Some(Scan::new(arriving.page, arriving.resolution));
            self.incoming = None;
            self.saved = None;
        }
        if let Some(scan) = self.scan.as_mut() {
            scan.page = arriving.page;
            scan.extend(arriving.from, arriving.lines);
            scan.last_line = now;
        }
        fresh
    }

    /// Write a received page out as a picture.
    ///
    /// Each scan line is drawn as many pixels tall as it takes to make the
    /// pels square, because they are not: a fax is 8.05 pels per millimetre
    /// across and 3.85 or 7.7 lines per millimetre down it. Saved at the pel
    /// grid it came in on, a standard-resolution page is half the height it
    /// should be and everything on it looks squashed.
    ///
    /// A page still arriving saves as far as it has got, which is also what
    /// is left of a call that ended part way down the page.
    pub fn save(&mut self, path: &std::path::Path) {
        let (lines, resolution) = match (self.incoming.as_ref(), self.scan.as_ref()) {
            (Some(page), _) => (&page.lines, page.resolution),
            (None, Some(scan)) if !scan.lines.is_empty() => (&scan.lines, scan.resolution),
            _ => return,
        };
        let tall = (fax::page::PELS_PER_MM / resolution.lines_per_mm())
            .round()
            .max(1.0) as usize;
        let width = fax::page::WIDTH;
        let height = lines.len() * tall;
        let mut pixels = Vec::with_capacity(width * height);
        for line in lines {
            for _ in 0..tall {
                pixels.extend(line.iter().map(|ink| if *ink { 0u8 } else { 255u8 }));
            }
        }
        let said = match image::GrayImage::from_raw(width as u32, height as u32, pixels) {
            None => "the page did not come out rectangular".to_owned(),
            Some(image) => match image.save(path) {
                Ok(()) => format!("saved to {}", path.display()),
                Err(e) => format!("{e}"),
            },
        };
        self.saved = Some(said);
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
                        .on_hover_text(
                            "2400 and 4800, and the one every fax has. Left \
                             unticked with nothing else ticked, it is offered \
                             anyway",
                        );
                    ui.checkbox(&mut self.v29, "V.29").on_hover_text(
                        "7200 and 9600. Untick it to hold a call to V.27ter, \
                         which is slower and more forgiving of a bad line",
                    );
                    ui.add_enabled_ui(false, |ui| {
                        ui.checkbox(&mut self.v17, "V.17").on_hover_text(
                            "7200 to 14 400, trellis coded. Not written yet",
                        );
                    });
                    ui.checkbox(&mut self.error_correction, "ECM").on_hover_text(
                        "Error correction mode, T.30 Annex A: the page goes in                          numbered frames, and any the far end cannot read are                          sent again instead of printed as streaks. Used only                          when the far end offers it too",
                    );
                });
                if again {
                    self.render();
                }

                ui.separator();
                let texture = self.texture(ui.ctx());
                let heading = match self.page.as_ref() {
                    Some(page) => format!(
                        "page to send: {} lines, {}",
                        page.height(),
                        page.resolution.name()
                    ),
                    None => "page to send".to_owned(),
                };
                egui::CollapsingHeader::new(RichText::new(heading).color(dim))
                    .id_salt("fax-send")
                    .default_open(true)
                    .show(ui, |ui| {
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
                                    let [mh, mr, mmr] = self.coded_bits;
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
                                        (
                                            "coded",
                                            format!(
                                                "MH {}k, MR {}k, MMR {}k bits",
                                                mh / 1000,
                                                mr / 1000,
                                                mmr / 1000
                                            ),
                                        ),
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
                                            // anybody actually wants from this window:
                                            // from MMR, if the far end has error
                                            // correction, to MH, which every machine
                                            // reads.
                                            for m in self.ours() {
                                                for rate in m.rates() {
                                                    ui.label(
                                                        RichText::new(format!("at {rate}"))
                                                            .monospace()
                                                            .color(dim),
                                                    );
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{:.0} to {:.0} s  {}",
                                                            mmr as f64 / f64::from(*rate),
                                                            mh as f64 / f64::from(*rate),
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

                self.receive_row(ui, dim, bright);

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
                                if let Some(fif) = self.far_non_standard.as_deref() {
                                    for (k, v) in non_standard_rows(fif) {
                                        ui.label(RichText::new(k).monospace().color(dim));
                                        ui.label(RichText::new(v).monospace().color(bright));
                                        ui.end_row();
                                    }
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
                    RichText::new(format!("at {} bit/s in {}", self.rate, self.coding))
                        .small()
                        .color(dim),
                );
            }
            if self.correcting {
                ui.label(RichText::new("with error correction").small().color(dim));
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

    /// The page arriving, drawn as it comes, or the page that last arrived.
    fn receive_row(&mut self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        ui.separator();
        let now = ui.input(|i| i.time);
        let arriving = self
            .scan
            .as_ref()
            .is_some_and(|scan| self.incoming.is_none() && now - scan.last_line < STILL_ARRIVING);
        let heading = match (self.scan.as_ref(), self.incoming.is_some(), arriving) {
            (None, _, _) => "page received".to_owned(),
            (Some(scan), _, true) => format!("page arriving: {} lines", scan.lines.len()),
            (Some(scan), true, false) => format!("page received: {} lines", scan.lines.len()),
            // A call that ended part way down a page, or one whose page has
            // stopped and not yet been handed over.
            (Some(scan), false, false) => {
                format!("page, as far as it came: {} lines", scan.lines.len())
            }
        };
        let mut save = false;
        egui::CollapsingHeader::new(RichText::new(heading).color(dim))
            .id_salt("fax-receive")
            .default_open(true)
            .show(ui, |ui| {
                let Some(scan) = self.scan.as_mut() else {
                    ui.label(
                        RichText::new(
                            "Nothing yet. Press Wait for a fax, and a page \
                             that arrives is drawn here a line at a time as it \
                             comes in.",
                        )
                        .small()
                        .color(dim),
                    );
                    return;
                };
                if let Some(texture) = scan.texture(ui.ctx()) {
                    let shown = scan.shown();
                    let bottom = shown as f32 / scan.capacity as f32;
                    egui::ScrollArea::vertical()
                        .id_salt("fax-receive-scroll")
                        .max_height(SCAN_HEIGHT)
                        // Following the newest line while the view is at the
                        // bottom, and staying put once scrolled up to look.
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            // The whole width of the panel from the first row,
                            // and only ever taller. Painted into a rectangle of
                            // its own rather than through an image widget: that
                            // keeps the texture's proportions, and the texture
                            // is a whole sheet tall, so the top ten rows of a
                            // page came out a few pels wide and widened as the
                            // rest arrived.
                            let size = picture_size(ui.available_width(), shown);
                            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                            ui.painter().image(
                                texture.id(),
                                rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, bottom)),
                                Color32::WHITE,
                            );
                            if arriving {
                                // Where the page has got to.
                                ui.painter().hline(
                                    rect.x_range(),
                                    rect.bottom() - 1.0,
                                    egui::Stroke::new(2.0, Color32::from_rgb(90, 220, 130)),
                                );
                            }
                        });
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(scan.resolution.name())
                            .monospace()
                            .color(bright),
                    );
                    if let Some(page) = self.incoming.as_ref() {
                        ui.label(
                            RichText::new(format!(
                                "{:.1}% of the paper is ink",
                                page.coverage() * 100.0
                            ))
                            .monospace()
                            .color(dim),
                        );
                    }
                    save = ui
                        .button("Save as PNG")
                        .on_hover_text(
                            "The page as far as it has got, if it is still \
                             arriving or the call ended part way down it",
                        )
                        .clicked();
                });
                if let Some(saved) = &self.saved {
                    ui.label(RichText::new(saved).small().color(dim));
                }
            });
        if arriving {
            // The green line goes when the page stops, and nothing else may
            // be about to ask for a frame when it does.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
        if save
            && let Some(chosen) = rfd::FileDialog::new()
                .add_filter("pictures", &["png"])
                .set_file_name("fax.png")
                .save_file()
        {
            self.save(&chosen);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn page_of(lines: usize) -> Page {
        Page {
            lines: (0..lines)
                .map(|y| (0..fax::page::WIDTH).map(|x| (x + y) % 7 == 0).collect())
                .collect(),
            resolution: Resolution::Standard,
        }
    }

    #[test]
    fn an_nsf_shows_its_length_and_its_country_code_turned_round() {
        // The one off the recording, as far as it read.
        let fif = [0x00, 0x00, 0x51, 0x00, 0x00, 0x10, 0xb1, 0x2a, 0x12, 0xa2];
        let rows = non_standard_rows(&fif);
        assert_eq!(rows[0].1, "10 octets, country code 00");
        assert_eq!(rows[1].1, "00 00 51 00 00 10 b1 2a ...");
        // Most significant bit first on the line: 0x01 as read is 0x80.
        assert_eq!(non_standard_rows(&[0x01, 0x02]), vec![
            ("non-standard", "2 octets, country code 80".to_owned()),
            ("  starting", "01 02".to_owned()),
        ]);
        assert_eq!(non_standard_rows(&[]).len(), 1);
    }

    fn lines_of(count: usize, ink: bool) -> Vec<Vec<bool>> {
        vec![vec![ink; fax::page::WIDTH]; count]
    }

    #[test]
    fn a_page_arriving_is_drawn_the_shape_of_the_sheet() {
        // A whole A4 sheet at either resolution is the same picture, 297 mm
        // down and 215 across -- not a standard page half the height of the
        // same page sent fine.
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let mut scan = Scan::new(0, resolution);
            scan.extend(0, lines_of(resolution.lines(), false));
            let tall = scan.rows as f64 / SCAN_WIDTH as f64;
            assert!(
                (tall - 297.0 / 215.0).abs() < 0.01,
                "{resolution:?}: {} rows for {SCAN_WIDTH} across",
                scan.rows
            );
            assert_eq!(scan.pixels.len(), scan.rows * SCAN_WIDTH);
        }
    }

    #[test]
    fn a_row_is_drawn_once_its_lines_are_in_and_never_again() {
        // Which is what lets the texture take a strip at a time: a row that
        // could still change would have to be sent again.
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let page = page_of(300);
            let mut scan = Scan::new(0, resolution);
            let mut before: Vec<Color32> = Vec::new();
            for (i, line) in page.lines.iter().enumerate() {
                scan.extend(i, vec![line.clone()]);
                assert!(scan.span(scan.rows).end > scan.lines.len(), "a finished row was left");
                assert_eq!(&scan.pixels[..before.len()], &before[..], "a finished row changed");
                before.clone_from(&scan.pixels);
            }
            let mut whole = Scan::new(0, resolution);
            whole.extend(0, page.lines.clone());
            assert_eq!(scan.pixels, whole.pixels, "a line at a time is not the same picture");
        }
    }

    #[test]
    fn ink_draws_dark_and_paper_light() {
        let mut scan = Scan::new(0, Resolution::Standard);
        scan.extend(0, lines_of(20, true));
        scan.extend(20, lines_of(20, false));
        assert_eq!(scan.pixels.first(), Some(&Color32::from_rgb(0, 0, 0)));
        assert_eq!(scan.pixels.last(), Some(&Color32::from_rgb(255, 255, 255)));
    }

    #[test]
    fn lines_that_come_twice_are_drawn_once_and_lines_past_a_gap_not_at_all() {
        let mut scan = Scan::new(0, Resolution::Standard);
        scan.extend(0, lines_of(10, false));
        scan.extend(5, lines_of(10, false));
        assert_eq!(scan.lines.len(), 15);
        scan.extend(20, lines_of(5, false));
        assert_eq!(scan.lines.len(), 15, "lines after a gap went in");
    }

    #[test]
    fn a_page_handed_over_before_its_last_lines_is_not_drawn_twice() {
        // The line thread hands over lines and then the page, and the window
        // takes them in whatever order it happens to look.
        let page = page_of(40);
        let batch = |from: usize, to: usize| Arriving {
            page: 3,
            resolution: Resolution::Standard,
            from,
            lines: page.lines[from..to].to_vec(),
        };
        let mut fax = Fax::new();
        assert!(fax.arriving(batch(0, 30), 1.0), "the top of a page is a new page");
        fax.arrived(page.clone());
        assert!(!fax.arriving(batch(30, 40), 1.1), "its own last lines are not");
        assert_eq!(fax.scan.as_ref().map(|s| &s.lines), Some(&page.lines));
        assert!(fax.incoming.is_some(), "the page that arrived was forgotten");

        // A page handed over whole before any of its lines were.
        let mut fax = Fax::new();
        fax.arrived(page.clone());
        assert!(!fax.arriving(batch(35, 40), 2.0));
        assert_eq!(fax.scan.as_ref().map(|s| &s.lines), Some(&page.lines));

        // And the next page is new, and the last one goes.
        let next = Arriving {
            page: 4,
            resolution: Resolution::Fine,
            from: 0,
            lines: page.lines[..5].to_vec(),
        };
        assert!(fax.arriving(next, 3.0));
        assert!(fax.incoming.is_none(), "the last page is still there");
        assert_eq!(fax.scan.as_ref().map(|s| s.lines.len()), Some(5));
    }

    #[test]
    fn a_page_still_arriving_saves_as_far_as_it_has_got() {
        let dir = std::env::temp_dir().join("binmodem-faxwin-test");
        let _ = std::fs::create_dir_all(&dir);
        let mut fax = Fax::new();
        fax.arriving(
            Arriving {
                page: 0,
                resolution: Resolution::Standard,
                from: 0,
                lines: page_of(25).lines,
            },
            0.0,
        );
        let path = dir.join("partial.png");
        fax.save(&path);
        let saved = fax.saved.clone().unwrap_or_default();
        assert!(saved.starts_with("saved to"), "{saved}");
        let image = image::open(&path).expect("it did not write a picture");
        assert_eq!(image.height(), 50, "25 standard lines, two pixels each");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_texture_has_room_for_a_sheet_and_grows_for_a_longer_page() {
        let ctx = egui::Context::default();
        let largest = ctx.input(|i| i.max_texture_side);
        let mut scan = Scan::new(0, Resolution::Standard);
        assert!(scan.texture(&ctx).is_none(), "a texture for nothing");
        scan.extend(0, lines_of(100, false));
        let texture = scan.texture(&ctx).expect("no texture");
        assert_eq!(texture.size()[0], SCAN_WIDTH);
        assert!(texture.size()[1] >= 795, "{} rows is not a sheet", texture.size()[1]);
        assert_eq!(scan.uploaded, scan.rows);

        // Half as long again as a sheet.
        scan.extend(100, lines_of(1700, false));
        let texture = scan.texture(&ctx).expect("no texture");
        assert!(texture.size()[1] >= scan.rows, "the page ran off the texture");
        assert_eq!(scan.uploaded, scan.rows);

        // And longer than the card takes: the texture stops at its largest
        // and the picture at the texture.
        let enough = (largest as f64 * scan.lines_per_row()) as usize + 10;
        scan.extend(1800, lines_of(enough, false));
        let texture = scan.texture(&ctx).expect("no texture");
        assert_eq!(texture.size()[1], largest);
        assert_eq!(scan.shown(), largest);
        assert_eq!(scan.uploaded, largest);
    }

    /// Every rectangle painted with `texture`, however deep in the shapes and
    /// whichever way it was painted: a mesh from a painter, or a rectangle
    /// filled with the texture, which is how an image widget does it.
    fn painted_with(shapes: &[egui::Shape], texture: egui::TextureId, out: &mut Vec<egui::Rect>) {
        for shape in shapes {
            match shape {
                egui::Shape::Rect(filled)
                    if filled.brush.as_ref().is_some_and(|b| b.fill_texture_id == texture) =>
                {
                    out.push(filled.rect);
                }
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => {
                    let mut rect = egui::Rect::NOTHING;
                    for vertex in &mesh.vertices {
                        rect.extend_with(vertex.pos);
                    }
                    out.push(rect);
                }
                egui::Shape::Vec(inner) => painted_with(inner, texture, out),
                _ => {}
            }
        }
    }

    #[test]
    fn a_page_arriving_is_drawn_the_width_of_the_panel_from_its_first_row() {
        // It was not. The top of a page came out a sliver a few pels wide in
        // the middle of the panel and widened as the page came in, because the
        // picture was made to keep the proportions of a texture a whole sheet
        // tall. Drawn here the way the window draws it, in a real frame.
        let ctx = egui::Context::default();
        let page = page_of(1143);
        let mut fax = Fax::new();
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        let mut had = 0;
        for lines in [3, 12, 200, 1143] {
            fax.arriving(
                Arriving {
                    page: 0,
                    resolution: Resolution::Standard,
                    from: had,
                    lines: page.lines[had..lines].to_vec(),
                },
                0.0,
            );
            had = lines;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 900.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                fax.receive_row(ui, Color32::GRAY, Color32::WHITE);
            });
            // What a renderer would have uploaded. There is none here.
            output.textures_delta.clear();
            let texture = fax
                .scan
                .as_ref()
                .and_then(|scan| scan.texture.as_ref())
                .expect("no texture")
                .id();
            let shapes: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
            let mut drawn = Vec::new();
            painted_with(&shapes, texture, &mut drawn);
            let rect = *drawn.first().unwrap_or_else(|| panic!("{lines} lines: the page was not drawn"));
            widths.push(rect.width());
            heights.push(rect.height());
        }
        assert!(widths[0] > 500.0, "the first rows are {} across", widths[0]);
        assert!(
            widths.iter().all(|w| (w - widths[0]).abs() < 0.5),
            "the page changed width as it arrived: {widths:?}"
        );
        assert!(heights.windows(2).all(|h| h[0] < h[1]), "not growing downwards: {heights:?}");
        // And the page's own shape at the end: 795 rows of 576 across.
        let whole = heights[3] / widths[3];
        assert!((whole - 795.0 / 576.0).abs() < 0.01, "a sheet {whole} times as tall as it is wide");
    }

    #[test]
    fn a_thumbnail_has_as_many_pixels_as_it_says_it_has() {
        // The image and the size it declares are handed over separately, and
        // the two disagreeing is a panic inside the drawing rather than a
        // wrong picture. A page that arrives is whatever length the far end
        // sent, including lengths nothing here would ever produce.
        for lines in [0, 1, 5, 6, 7, 11, 191, 1143, 2287] {
            let image = Fax::thumbnail(&page_of(lines));
            assert_eq!(
                image.pixels.len(),
                image.size[0] * image.size[1],
                "a page of {lines} lines"
            );
            assert!(image.size[1] > 0, "a page of {lines} lines has no rows");
        }
    }

    #[test]
    fn a_page_that_arrived_saves_at_the_shape_it_was_sent() {
        // A fax pel is not square: 8.05 across the millimetre and 3.85 or 7.7
        // down it. Written out on the grid it arrived on, a standard page is
        // half the height it should be.
        let dir = std::env::temp_dir().join("binmodem-faxwin-test");
        let _ = std::fs::create_dir_all(&dir);
        for (resolution, tall) in [(Resolution::Standard, 2), (Resolution::Fine, 1)] {
            let mut fax = Fax::new();
            let mut page = page_of(20);
            page.resolution = resolution;
            fax.arrived(page);
            let path = dir.join(format!("{}.png", resolution.name().replace(['.', ',', ' ', '/'], "-")));
            fax.save(&path);
            let saved = fax.saved.clone().unwrap_or_default();
            assert!(saved.starts_with("saved to"), "{saved}");
            let image = image::open(&path).expect("it did not write a picture");
            assert_eq!(image.width() as usize, fax::page::WIDTH);
            assert_eq!(image.height() as usize, 20 * tall);
            let _ = std::fs::remove_file(&path);
        }
    }
}
