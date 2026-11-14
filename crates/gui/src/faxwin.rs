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
use std::sync::Arc;

use crate::document;
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
    /// The picture or the PDF, kept so that the pages can be made again when
    /// the resolution or the halftone changes without going back to the file.
    source: Option<Source>,
    /// What the file is, for the panel: a picture and its size, or a
    /// document and its pages.
    source_said: (&'static str, String),
    /// The pages to send, in order: a picture's one, or a document's.
    pages: Vec<Page>,
    /// How long the pages code to in each coding -- MH, MR, MMR and JBIG --
    /// all of them together, worked out once when they are made.
    ///
    /// Coding a full page is two to four milliseconds, and the panel wants
    /// the figures once for itself and again for each rate it could go out
    /// at. All of those in every frame of a window that is open for the whole
    /// of a call is tens of milliseconds a frame spent recomputing numbers
    /// that cannot have changed.
    coded_bits: [usize; 4],
    /// Which of the pages the preview is of.
    showing: usize,
    /// A small copy of that page for the window to draw.
    preview: Option<egui::TextureHandle>,
    pub resolution: Resolution,
    pub halftone: Halftone,
    /// Which modulations this end is willing to use.
    pub v27ter: bool,
    pub v29: bool,
    pub v17: bool,
    /// Whether this end offers V.34 half-duplex -- Super G3, the page at up
    /// to 33 600 -- which puts V.8 in front of the call.
    pub v34: bool,
    /// Whether this end offers error correction mode.
    pub error_correction: bool,
    /// Whether this end offers JBIG, which goes only with error correction.
    pub jbig: bool,
    /// What the far end said, once it has said anything.
    pub far: Option<t30::Capabilities>,
    pub far_identity: String,
    /// The far end's NSF, if it sent one.
    pub far_non_standard: Option<Vec<u8>>,
    /// Where the call has got to, straight off the modem.
    pub phase: Option<&'static str>,
    /// How far through the page, at what rate -- and, on V.34, at what
    /// symbol rate under it -- and how many lines have arrived. Straight off
    /// the modem as well.
    pub progress: Option<f64>,
    pub rate: u32,
    pub symbol_rate: u32,
    pub lines: usize,
    /// Which page of the call is going or arriving, and how many it has.
    pub sheet: usize,
    pub sheets: usize,
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
    /// What this call's page is going, or went, in -- kept from the moment a
    /// page starts moving until the call is over, so that it does not blink
    /// out in the turnaround between one page and the next.
    carried: Option<Carried>,
    /// A page that arrived: the one `scan` is a picture of.
    incoming: Option<Page>,
    /// The page arriving, or the last one that did, drawn as it came in.
    scan: Option<Scan>,
    /// The pages of the same call that arrived before that one, with their
    /// numbers in it.
    earlier: Vec<(usize, Page)>,
    /// One of those being looked at instead, by its place in `earlier`, and
    /// its picture.
    looking: Option<(usize, Scan)>,
    /// What was said about saving the last page.
    pub saved: Option<String>,
}

/// What the pages to send are made from.
enum Source {
    /// A picture: grey with square pixels, fitted to the paper as one page.
    Picture(Grey),
    /// A PDF, kept as the file it came in: its pages are drawn onto the paper
    /// again for another resolution, which is quicker than keeping them drawn.
    Document(Arc<Vec<u8>>),
}

/// What a page is carried in: its coding, the modulation under it and its
/// rates, and whether error correction is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Carried {
    coding: &'static str,
    modulation: &'static str,
    rate: u32,
    symbol_rate: u32,
    correcting: bool,
}

/// What the window is asking the modem to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// Dial, and send whatever page is loaded.
    Dial(String),
    /// Go off hook and take whatever arrives.
    Answer,
    /// Put the call down, in the middle of a fax or before it answers.
    HangUp,
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

/// Green: the best a fax can do -- JBIG, V.34, error correction.
const BEST: Color32 = Color32::from_rgb(90, 220, 130);
/// Yellow, orange and a redder orange for the rungs below it.
const GOOD: Color32 = Color32::from_rgb(235, 205, 90);
const FAIR: Color32 = Color32::from_rgb(240, 160, 80);
const POOR: Color32 = Color32::from_rgb(225, 120, 90);
/// The ink on a badge's coloured ground.
const BADGE_INK: Color32 = Color32::from_rgb(18, 20, 24);

/// A page coding's colour, by how small it makes a page: T.85's JBIG, then
/// T.6's MMR, then T.4's MR and MH.
fn coding_colour(coding: &str) -> Color32 {
    match coding {
        "JBIG" => BEST,
        "MMR" => GOOD,
        "MR" => FAIR,
        _ => POOR,
    }
}

/// A page carrier's colour, by how fast it goes: V.34's half-duplex mode
/// (Super G3, to 33 600), then V.17, V.29 and V.27 ter.
fn modulation_colour(modulation: &str) -> Color32 {
    match modulation {
        "V.34" => BEST,
        "V.17" => GOOD,
        "V.29" => FAIR,
        _ => POOR,
    }
}

/// A word on a coloured ground, big enough to read from across the room.
fn badge(ui: &mut egui::Ui, text: &str, ground: Color32, tip: &str) {
    ui.label(
        RichText::new(format!(" {text} "))
            .monospace()
            .strong()
            .size(15.0)
            .color(BADGE_INK)
            .background_color(ground),
    )
    .on_hover_text(tip);
}

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
    /// Which page this is, as the line numbered it, and which page of its
    /// call.
    page: u64,
    sheet: usize,
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
    /// The picture of a page that is already whole.
    fn of(sheet: usize, page: &Page) -> Self {
        let mut scan = Self::new(u64::MAX, sheet, page.resolution);
        scan.extend(0, page.lines.clone());
        scan
    }

    fn new(page: u64, sheet: usize, resolution: Resolution) -> Self {
        Self {
            page,
            sheet,
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

/// Write a page out as a picture, each line as many pixels tall as makes the
/// pels square.
fn write_page(lines: &[Vec<bool>], resolution: Resolution, path: &std::path::Path) -> Result<(), String> {
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
    let image = image::GrayImage::from_raw(width as u32, height as u32, pixels)
        .ok_or_else(|| "the page did not come out rectangular".to_owned())?;
    image.save(path).map_err(|e| format!("{e}"))
}

/// Write pages out as one PDF, each on a sheet the size it was scanned at.
fn write_pdf(pages: &[fax::pdf::Sheet<'_>], path: &std::path::Path) -> Result<(), String> {
    std::fs::write(path, fax::pdf::document(pages)).map_err(|e| format!("{e}"))
}

/// Whether a name chosen to save to asks for a PDF.
fn names_pdf(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

impl Fax {
    pub fn new() -> Self {
        Self {
            resolution: Resolution::Standard,
            halftone: Halftone::Threshold,
            v27ter: true,
            v29: true,
            // Offered, so a machine that has it trains at 14 400 first. T.30's
            // ladder takes a failed training check down through 12 000, 9600
            // and 7200 before it gives up on the modulation.
            v17: true,
            // Offered too: a Super G3 machine hears ANSam and sends its call
            // menu, and the page goes at up to 33 600; a plain one hears the
            // called tone and the call is clause 5's.
            v34: true,
            error_correction: true,
            jbig: true,
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

    /// Read a picture or a PDF off the disc.
    ///
    /// A picture is kept as grey. Everything becomes luminance and then ink:
    /// a fax has one bit per pel and no notion of colour at all, so the
    /// sooner the colour goes the fewer places there are to get it wrong. A
    /// PDF is kept as the file, and its pages drawn onto the paper from it.
    pub fn load(&mut self, path: &std::path::Path) {
        self.trouble = None;
        self.source = None;
        self.pages.clear();
        self.showing = 0;
        self.preview = None;
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.trouble = Some(format!("{e}"));
                return;
            }
        };
        if document::is_pdf(&bytes) {
            self.source = Some(Source::Document(Arc::new(bytes)));
        } else {
            let decoded = match image::load_from_memory(&bytes) {
                Ok(image) => image,
                Err(e) => {
                    self.trouble = Some(format!("{e}"));
                    return;
                }
            };
            let grey = decoded.to_luma8();
            let (w, h) = (grey.width() as usize, grey.height() as usize);
            self.source_said = ("picture", format!("{w} by {h}"));
            self.source = Some(Source::Picture(Grey {
                width: w,
                height: h,
                // Ink, so a white page is nothing and a black one is
                // everything. The image is the other way round, which is the
                // only reason this subtraction exists.
                ink: grey.pixels().map(|p| 1.0 - f32::from(p.0[0]) / 255.0).collect(),
            }));
        }
        self.path = path.display().to_string();
        self.render();
        if self.pages.is_empty() {
            self.source = None;
        }
    }

    /// Make the pages from the picture or the PDF already loaded.
    pub fn render(&mut self) {
        let Some(source) = self.source.as_ref() else { return };
        let pages = match source {
            Source::Picture(grey) => vec![fax::page::render(grey, self.resolution, self.halftone)],
            Source::Document(bytes) => match document::read(bytes, self.resolution, self.halftone) {
                Ok((pages, all)) => {
                    self.source_said = (
                        "document",
                        match all {
                            1 => "1 page".to_owned(),
                            _ if all > pages.len() => {
                                format!("the first {} of its {all} pages", pages.len())
                            }
                            _ => format!("{all} pages"),
                        },
                    );
                    pages
                }
                Err(e) => {
                    self.trouble = Some(e);
                    Vec::new()
                }
            },
        };
        self.coded_bits = [0; 4];
        for page in &pages {
            let codings = [Coding::ModifiedHuffman, Coding::ModifiedRead, Coding::Mmr, Coding::Jbig];
            for (total, coding) in self.coded_bits.iter_mut().zip(codings) {
                *total += coding.encode(&page.lines, page.resolution, 0).len();
            }
        }
        self.showing = self.showing.min(pages.len().saturating_sub(1));
        self.pages = pages;
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
                let down = ry * across..((ry + 1) * across).min(page.height());
                let along = rx * across..((rx + 1) * across).min(fax::page::WIDTH);
                for line in &page.lines[down] {
                    for &pel in &line[along.clone()] {
                        ink += usize::from(pel);
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
            let page = self.pages.get(self.showing)?;
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
        self.symbol_rate = frame.fax_symbol_rate;
        self.lines = frame.fax_lines;
        self.sheet = frame.fax_sheet;
        self.sheets = frame.fax_sheets;
        self.correcting = frame.fax_error_correction;
        self.coding = frame.fax_coding;
        self.sending = frame.fax_sending;
        self.fax_class = frame.fax_class;
        // Only once a page is moving: until the command has settled it, the
        // coding the call reports is the MH every call starts from.
        if frame.fax_progress.is_some() && frame.fax_rate > 0 {
            self.carried = Some(Carried {
                coding: frame.fax_coding,
                modulation: frame.modulation,
                rate: frame.fax_rate,
                symbol_rate: frame.fax_symbol_rate,
                correcting: frame.fax_error_correction,
            });
        }
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

    /// The pages that are ready to send, in order.
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// A page has arrived.
    ///
    /// Usually it has been drawn already, line by line on the way in. If the
    /// window did not get every line of it -- or got the page before the last
    /// few, which the two threads are free to do -- the picture is made again
    /// from the page itself.
    pub fn arrived(&mut self, sheet: usize, page: Page) {
        let same = self.scan.as_ref().is_some_and(|scan| scan.sheet == sheet);
        let drawn = same
            && self.scan.as_ref().is_some_and(|scan| {
                scan.lines.len() == page.lines.len() && scan.resolution == page.resolution
            });
        if !same {
            self.next_sheet(sheet);
        }
        if !drawn {
            // Keep the number of the page being drawn, so the lines of it
            // still on their way are known for what they are.
            let mut scan = Scan::of(sheet, &page);
            scan.page = self.scan.as_ref().map_or(u64::MAX, |scan| scan.page);
            self.scan = Some(scan);
        }
        self.incoming = Some(page);
        self.saved = None;
    }

    /// Another page of a call is starting. The one before it goes with the
    /// call's others -- or, if this is the first page of a call, the last
    /// call's pages go.
    fn next_sheet(&mut self, sheet: usize) {
        if let Some(page) = self.incoming.take() {
            let before = self.scan.as_ref().map_or(0, |scan| scan.sheet);
            self.earlier.push((before, page));
        }
        if sheet <= 1 {
            self.earlier.clear();
        }
        self.looking = None;
        self.saved = None;
    }

    /// Look at the page `step` places away from the one in view, among the
    /// call's pages.
    fn look(&mut self, step: isize) {
        let at = self.looking.as_ref().map_or(self.earlier.len(), |(i, _)| *i);
        let last = self.earlier.len() + usize::from(self.scan.is_some());
        let Some(to) = at.checked_add_signed(step).filter(|to| *to < last) else {
            return;
        };
        self.looking = self
            .earlier
            .get(to)
            .map(|(sheet, page)| (to, Scan::of(*sheet, page)));
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
            self.next_sheet(arriving.sheet);
            self.scan = Some(Scan::new(arriving.page, arriving.sheet, arriving.resolution));
        }
        if let Some(scan) = self.scan.as_mut() {
            scan.page = arriving.page;
            scan.sheet = arriving.sheet;
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
    /// A name ending in `.pdf` saves a PDF instead, whose sheet is the size
    /// the page was scanned at, so the lines need no drawing twice.
    ///
    /// A page still arriving saves as far as it has got, which is also what
    /// is left of a call that ended part way down the page.
    pub fn save(&mut self, path: &std::path::Path) {
        let Some((_, lines, resolution)) = self.in_view() else { return };
        let written = if names_pdf(path) {
            write_pdf(&[(lines, resolution)], path)
        } else {
            write_page(lines, resolution, path)
        };
        self.saved = Some(match written {
            Ok(()) => format!("saved to {}", path.display()),
            Err(e) => e,
        });
    }

    /// The page in view: an earlier one being looked at, or the last to
    /// arrive, or as much of the one arriving as has.
    fn in_view(&self) -> Option<(usize, &[Vec<bool>], Resolution)> {
        if let Some((i, _)) = &self.looking {
            let (sheet, page) = self.earlier.get(*i)?;
            return Some((*sheet, &page.lines, page.resolution));
        }
        let scan = self.scan.as_ref();
        let sheet = scan.map_or(0, |scan| scan.sheet);
        match (self.incoming.as_ref(), scan) {
            (Some(page), _) => Some((sheet, &page.lines, page.resolution)),
            (None, Some(scan)) if !scan.lines.is_empty() => {
                Some((sheet, &scan.lines, scan.resolution))
            }
            _ => None,
        }
    }

    /// Every page of the call: in one PDF if the name ends in `.pdf`, a page
    /// to a sheet, and otherwise each to its own picture -- `fax.png` becomes
    /// `fax-1.png`, `fax-2.png` and so on, by the page's number in the call.
    pub fn save_all(&mut self, path: &std::path::Path) {
        let stem = path.file_stem().map_or_else(|| "fax".into(), |s| s.to_string_lossy());
        let named = |sheet: usize| path.with_file_name(format!("{stem}-{sheet}.png"));
        let mut pages: Vec<(usize, &[Vec<bool>], Resolution)> = self
            .earlier
            .iter()
            .map(|(sheet, page)| (*sheet, page.lines.as_slice(), page.resolution))
            .collect();
        if let Some(scan) = self.scan.as_ref() {
            match self.incoming.as_ref() {
                Some(page) => pages.push((scan.sheet, &page.lines, page.resolution)),
                None if !scan.lines.is_empty() => pages.push((scan.sheet, &scan.lines, scan.resolution)),
                None => {}
            }
        }
        if names_pdf(path) {
            let sheets: Vec<fax::pdf::Sheet<'_>> =
                pages.iter().map(|(_, lines, resolution)| (*lines, *resolution)).collect();
            self.saved = Some(match write_pdf(&sheets, path) {
                Ok(()) => format!("{} pages saved to {}", sheets.len(), path.display()),
                Err(e) => e,
            });
            return;
        }
        let mut written = 0;
        let mut trouble = None;
        for (sheet, lines, resolution) in pages {
            match write_page(lines, resolution, &named(sheet)) {
                Ok(()) => written += 1,
                Err(e) => trouble = Some(e),
            }
        }
        self.saved = Some(match trouble {
            Some(e) => e,
            None => format!("{written} pages saved as {}", named(0).with_file_name(format!("{stem}-N.png")).display()),
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
                    if ui
                        .button("Browse")
                        .on_hover_text(
                            "A picture, sent as one page, or a PDF, sent a page to a sheet \
                             at the size it is printed -- up to 50 of its pages",
                        )
                        .clicked()
                        && let Some(chosen) = rfd::FileDialog::new()
                            .add_filter("pictures and PDFs", &["png", "jpg", "jpeg", "bmp", "gif", "pdf"])
                            .add_filter("PDFs", &["pdf"])
                            .add_filter("pictures", &["png", "jpg", "jpeg", "bmp", "gif"])
                            .pick_file()
                    {
                        self.load(&chosen);
                    }
                    let shown = if self.path.is_empty() {
                        "no picture or PDF".to_owned()
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
                    ui.checkbox(&mut self.v17, "V.17").on_hover_text(
                        "7200 to 14 400, trellis coded. Untick it to hold a                          call to V.29 and V.27ter",
                    );
                    ui.checkbox(&mut self.v34, "V.34").on_hover_text(
                        "Super G3: V.8 in front of the call, and the page over V.34's half-duplex mode at up to 33 600, with the T.30 frames on its 1200 bit/s control channel. Used only when the far end has it; answering, it means ANSam in place of the called tone. Untick it for the plain tones and V.17 down",
                    );
                    ui.checkbox(&mut self.error_correction, "ECM").on_hover_text(
                        "Error correction mode, T.30 Annex A: the page goes in numbered frames, and any the far end cannot read are sent again instead of printed as streaks. Used only when the far end offers it too",
                    );
                    ui.add_enabled(self.error_correction, egui::Checkbox::new(&mut self.jbig, "JBIG"))
                        .on_hover_text(
                            "T.85's JBIG: every pel predicted from the ones around it and arithmetic coded, a good deal smaller than MMR on a picture. Only with ECM, and only when the far end offers it too; unticked, a page goes in MMR",
                        );
                });
                if again {
                    self.render();
                }

                ui.separator();
                let texture = self.texture(ui.ctx());
                let count = self.pages.len();
                let heading = match self.pages.as_slice() {
                    [] => "page to send".to_owned(),
                    [page] => format!(
                        "page to send: {} lines, {}",
                        page.height(),
                        page.resolution.name()
                    ),
                    _ => format!("{count} pages to send, {}", self.resolution.name()),
                };
                let mut step = 0isize;
                egui::CollapsingHeader::new(RichText::new(heading).color(dim))
                    .id_salt("fax-send")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
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
                                if count > 1 {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(self.showing > 0, egui::Button::new("<"))
                                            .on_hover_text("the page before")
                                            .clicked()
                                        {
                                            step = -1;
                                        }
                                        ui.label(
                                            RichText::new(format!("page {} of {count}", self.showing + 1))
                                                .monospace()
                                                .color(bright),
                                        );
                                        if ui
                                            .add_enabled(self.showing + 1 < count, egui::Button::new(">"))
                                            .on_hover_text("the page after")
                                            .clicked()
                                        {
                                            step = 1;
                                        }
                                    });
                                }
                            });
                            ui.vertical(|ui| {
                                if let Some(page) = self.pages.get(self.showing) {
                                    let [mh, mr, mmr, jbig] = self.coded_bits;
                                    let (what, said) = &self.source_said;
                                    let rows = [
                                        (*what, said.clone()),
                                        (
                                            "page",
                                            format!("{} by {} pels", page.width(), page.height()),
                                        ),
                                        ("ink", format!("{:.1}% of the paper", page.coverage() * 100.0)),
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
                                            // Each coding in its colour, JBIG in the
                                            // green of the best there is.
                                            ui.label(RichText::new("coded").monospace().color(dim));
                                            ui.horizontal(|ui| {
                                                for (name, bits) in [("MH", mh), ("MR", mr), ("MMR", mmr), ("JBIG", jbig)] {
                                                    ui.label(
                                                        RichText::new(format!("{name} {}k", bits / 1000))
                                                            .monospace()
                                                            .color(coding_colour(name)),
                                                    );
                                                }
                                                // The figures and the times under
                                                // them are for the whole fax.
                                                let whole = match count {
                                                    1 => "bits".to_owned(),
                                                    _ => format!("bits, all {count} pages"),
                                                };
                                                ui.label(RichText::new(whole).monospace().color(dim));
                                            });
                                            ui.end_row();
                                            // What it costs at each rate this end is
                                            // willing to use, which is the number
                                            // anybody actually wants from this window:
                                            // from the smallest coding this end will
                                            // send -- JBIG or MMR under error
                                            // correction, MR without -- to MH, which
                                            // every machine reads. V.34 first, which
                                            // has error correction always (T.30 F.3).
                                            let packed = if self.jbig { jbig.min(mmr) } else { mmr };
                                            let best = if self.error_correction { packed } else { mr };
                                            let mut times: Vec<(u32, usize, &str)> = Vec::new();
                                            if self.v34 {
                                                times.push((33_600, packed, "V.34"));
                                            }
                                            for m in self.ours() {
                                                for rate in m.rates() {
                                                    times.push((*rate, best, m.name()));
                                                }
                                            }
                                            for (rate, least, name) in times {
                                                ui.label(
                                                    RichText::new(format!("at {rate}"))
                                                        .monospace()
                                                        .color(dim),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{:.0} to {:.0} s",
                                                            least as f64 / f64::from(rate),
                                                            mh as f64 / f64::from(rate),
                                                        ))
                                                        .monospace()
                                                        .color(bright),
                                                    );
                                                    ui.label(
                                                        RichText::new(name)
                                                            .monospace()
                                                            .color(modulation_colour(name)),
                                                    );
                                                });
                                                ui.end_row();
                                            }
                                        });
                                }
                            });
                        });
                    });
                if step != 0 {
                    self.showing = self.showing.saturating_add_signed(step).min(count.saturating_sub(1));
                    self.preview = None;
                }

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
                if let Some(stop) = self.call_progress(ui, on_hook, dim, bright) {
                    start = Some(stop);
                }

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
                                // On a V.34 call the answering end's field is
                                // the caller's DCS, whose rate bits T.30 has
                                // sent as zero (Table 2, Note 33): read as a
                                // DIS they say V.27 ter at 4800, which is not
                                // what the far end can do. The rate is the
                                // modem's, from its MPh exchange.
                                let v34 = self.symbol_rate > 0;
                                for (k, v) in caps.rows() {
                                    if v34 && !self.sending && matches!(k, "modulations" | "fastest") {
                                        continue;
                                    }
                                    ui.label(RichText::new(k).monospace().color(dim));
                                    match (k, v.split_once("JBIG")) {
                                        // The best coding there is, in its green,
                                        // where the far end says it has it.
                                        ("coding", Some((before, after))) => {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 0.0;
                                                ui.label(RichText::new(before).monospace().color(bright));
                                                ui.label(RichText::new("JBIG").monospace().strong().color(BEST));
                                                ui.label(RichText::new(after).monospace().color(bright));
                                            });
                                        }
                                        _ => {
                                            ui.label(RichText::new(v).monospace().color(bright));
                                        }
                                    }
                                    ui.end_row();
                                }
                                ui.label(RichText::new("both ends").monospace().color(dim));
                                let shared = if v34 {
                                    Some(format!(
                                        "V.34 at {} bit/s on {} baud",
                                        self.rate, self.symbol_rate
                                    ))
                                } else {
                                    caps.best_shared(&self.ours())
                                        .map(|(m, rate)| format!("{} at {rate} bit/s", m.name()))
                                };
                                ui.label(
                                    RichText::new(
                                        shared.clone().unwrap_or_else(|| "nothing in common".to_owned()),
                                    )
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
    ) -> Option<Start> {
        if on_hook {
            self.carried = None;
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
            return None;
        }
        let what = match self.phase {
            Some(p) => p.to_owned(),
            None => "a call, but not a fax".to_owned(),
        };
        let side = if self.sending { "sending" } else { "receiving" };
        let mut stop = None;
        ui.horizontal(|ui| {
            if ui
                .button("Stop")
                .on_hover_text(
                    "put the call down now. A fax has no terminal to type ATH \
                     at, so this is the way out of one that has stalled",
                )
                .clicked()
            {
                stop = Some(Start::HangUp);
            }
            ui.label(RichText::new(format!("{side}: {what}")).small().color(bright));
            if self.sending && self.sheets > 1 {
                ui.label(
                    RichText::new(format!("page {} of {}", self.sheet, self.sheets))
                        .small()
                        .color(dim),
                );
            }
            if !self.sending && self.sheet > 1 {
                ui.label(RichText::new(format!("page {}", self.sheet)).small().color(dim));
            }
            if !self.sending && self.lines > 0 {
                ui.label(
                    RichText::new(format!("{} lines", self.lines))
                        .small()
                        .color(dim),
                );
            }
        });
        // What the page is going in, where it can be seen from across the
        // room: the coding, the modulation and its rates, and error
        // correction, each in green when it is the best a fax has. V.34
        // picks its symbol rate as well as its bit rate, and the two together
        // say how crowded the constellation is.
        if let Some(c) = self.carried {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{side} in")).color(bright));
                badge(
                    ui,
                    c.coding,
                    coding_colour(c.coding),
                    "The page's coding, best first: JBIG (T.85), MMR (T.6), MR and \
                     MH (T.4). JBIG and MMR go only with error correction, and only \
                     when both ends have them",
                );
                ui.label(RichText::new("over").color(bright));
                let carrier = if c.symbol_rate > 0 {
                    format!("{} {} bit/s on {} baud", c.modulation, c.rate, c.symbol_rate)
                } else {
                    format!("{} {} bit/s", c.modulation, c.rate)
                };
                badge(
                    ui,
                    &carrier,
                    modulation_colour(c.modulation),
                    "What carries the page, fastest first: V.34 half-duplex (Super \
                     G3) to 33 600, V.17 to 14 400, V.29 to 9600, V.27 ter to 4800",
                );
                badge(
                    ui,
                    if c.correcting { "ECM" } else { "no ECM" },
                    if c.correcting { BEST } else { POOR },
                    "Error correction mode: frames the far end could not read are \
                     asked for and sent again, rather than printed as streaks",
                );
                if c.coding == "JBIG" && c.modulation == "V.34" && c.correcting {
                    ui.label(RichText::new("best mode").strong().color(BEST)).on_hover_text(
                        "JBIG over V.34 with error correction: the smallest page \
                         at the fastest rate a fax has",
                    );
                }
            });
        }
        if let Some(fraction) = self.progress {
            ui.add(
                egui::ProgressBar::new(fraction as f32)
                    .desired_height(8.0)
                    .show_percentage(),
            );
        }
        stop
    }

    /// The page arriving, drawn as it comes, or the page that last arrived.
    fn receive_row(&mut self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        ui.separator();
        let now = ui.input(|i| i.time);
        let arriving = self.looking.is_none()
            && self
                .scan
                .as_ref()
                .is_some_and(|scan| self.incoming.is_none() && now - scan.last_line < STILL_ARRIVING);
        let pages = self.earlier.len() + usize::from(self.scan.is_some());
        let at = self.looking.as_ref().map_or(self.earlier.len(), |(i, _)| *i);
        // "Page" alone for a call of one page, and its number for any other.
        let named = |sheet: usize| {
            if pages > 1 || sheet > 1 {
                format!("page {sheet}")
            } else {
                "page".to_owned()
            }
        };
        let heading = match (&self.looking, self.scan.as_ref(), self.incoming.is_some(), arriving) {
            (Some((_, scan)), _, _, _) => {
                format!("{} received: {} lines", named(scan.sheet), scan.lines.len())
            }
            (None, None, _, _) => "page received".to_owned(),
            (None, Some(scan), _, true) => {
                format!("{} arriving: {} lines", named(scan.sheet), scan.lines.len())
            }
            (None, Some(scan), true, false) => {
                format!("{} received: {} lines", named(scan.sheet), scan.lines.len())
            }
            // A call that ended part way down a page, or one whose page has
            // stopped and not yet been handed over.
            (None, Some(scan), false, false) => {
                format!("{}, as far as it came: {} lines", named(scan.sheet), scan.lines.len())
            }
        };
        let (mut save, mut save_all, mut step) = (false, false, 0isize);
        egui::CollapsingHeader::new(RichText::new(heading).color(dim))
            .id_salt("fax-receive")
            .default_open(true)
            .show(ui, |ui| {
                if pages > 1 {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(at > 0, egui::Button::new("<"))
                            .on_hover_text("the page before")
                            .clicked()
                        {
                            step = -1;
                        }
                        ui.label(
                            RichText::new(format!("{} of {pages} pages", at + 1))
                                .monospace()
                                .color(bright),
                        );
                        if ui
                            .add_enabled(at + 1 < pages, egui::Button::new(">"))
                            .on_hover_text("the page after, and the last is the one arriving")
                            .clicked()
                        {
                            step = 1;
                        }
                    });
                }
                let (scan, page) = match self.looking.as_mut() {
                    Some((i, scan)) => (Some(scan), self.earlier.get(*i).map(|(_, page)| page)),
                    None => (self.scan.as_mut(), self.incoming.as_ref()),
                };
                let Some(scan) = scan else {
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
                    if let Some(page) = page {
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
                        .button("Save")
                        .on_hover_text(
                            "This page, as a PDF or a PNG, whichever the name \
                             ends in -- as far as it has got, if it is still \
                             arriving or the call ended part way down it",
                        )
                        .clicked();
                    if pages > 1 {
                        save_all = ui
                            .button(format!("Save all {pages}"))
                            .on_hover_text(
                                "Every page of the call: in one PDF, a page to a \
                                 sheet, or each to its own PNG with the page's \
                                 number after the name chosen",
                            )
                            .clicked();
                    }
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
        if step != 0 {
            self.look(step);
        }
        if save
            && let Some(chosen) = rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .add_filter("PNG picture", &["png"])
                .set_file_name(match self.in_view() {
                    Some((sheet, _, _)) if pages > 1 => format!("fax-{sheet}.pdf"),
                    _ => "fax.pdf".to_owned(),
                })
                .save_file()
        {
            self.save(&chosen);
        }
        if save_all
            && let Some(chosen) = rfd::FileDialog::new()
                .add_filter("PDF, every page in one", &["pdf"])
                .add_filter("PNG pictures, a page each", &["png"])
                .set_file_name("fax.pdf")
                .save_file()
        {
            self.save_all(&chosen);
        }
    }

    /// What to type at the modem to start one.
    pub fn commands(start: &Start) -> String {
        // The class first: it decides what the dial does, and a modem told to
        // dial before it is told what it is places a data call.
        match start {
            Start::Dial(number) => format!("AT+FCLASS=1\rATD{number}\r"),
            Start::Answer => "AT+FCLASS=1\rATA\r".to_owned(),
            // Handled by the window, not the modem's command line.
            Start::HangUp => String::new(),
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
            let mut scan = Scan::new(0, 1, resolution);
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
            let mut scan = Scan::new(0, 1, resolution);
            let mut before: Vec<Color32> = Vec::new();
            for (i, line) in page.lines.iter().enumerate() {
                scan.extend(i, vec![line.clone()]);
                assert!(scan.span(scan.rows).end > scan.lines.len(), "a finished row was left");
                assert_eq!(&scan.pixels[..before.len()], &before[..], "a finished row changed");
                before.clone_from(&scan.pixels);
            }
            let mut whole = Scan::new(0, 1, resolution);
            whole.extend(0, page.lines.clone());
            assert_eq!(scan.pixels, whole.pixels, "a line at a time is not the same picture");
        }
    }

    #[test]
    fn ink_draws_dark_and_paper_light() {
        let mut scan = Scan::new(0, 1, Resolution::Standard);
        scan.extend(0, lines_of(20, true));
        scan.extend(20, lines_of(20, false));
        assert_eq!(scan.pixels.first(), Some(&Color32::from_rgb(0, 0, 0)));
        assert_eq!(scan.pixels.last(), Some(&Color32::from_rgb(255, 255, 255)));
    }

    #[test]
    fn lines_that_come_twice_are_drawn_once_and_lines_past_a_gap_not_at_all() {
        let mut scan = Scan::new(0, 1, Resolution::Standard);
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
            sheet: 1,
            resolution: Resolution::Standard,
            from,
            lines: page.lines[from..to].to_vec(),
        };
        let mut fax = Fax::new();
        assert!(fax.arriving(batch(0, 30), 1.0), "the top of a page is a new page");
        fax.arrived(1, page.clone());
        assert!(!fax.arriving(batch(30, 40), 1.1), "its own last lines are not");
        assert_eq!(fax.scan.as_ref().map(|s| &s.lines), Some(&page.lines));
        assert!(fax.incoming.is_some(), "the page that arrived was forgotten");

        // A page handed over whole before any of its lines were.
        let mut fax = Fax::new();
        fax.arrived(1, page.clone());
        assert!(!fax.arriving(batch(35, 40), 2.0));
        assert_eq!(fax.scan.as_ref().map(|s| &s.lines), Some(&page.lines));

        // And the next page is new, and the last one goes.
        let next = Arriving {
            page: 4,
            sheet: 1,
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
                sheet: 1,
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
        let mut scan = Scan::new(0, 1, Resolution::Standard);
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
                    sheet: 1,
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
            fax.arrived(1, page);
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

    #[test]
    fn the_pages_of_a_call_are_kept_and_can_be_looked_at_and_saved() {
        let pages: Vec<Page> = (0..3).map(|n| page_of(10 + n)).collect();
        let mut fax = Fax::new();
        for (n, page) in pages.iter().enumerate() {
            let sheet = n + 1;
            let arriving = Arriving {
                page: 10 + n as u64,
                sheet,
                resolution: Resolution::Standard,
                from: 0,
                lines: page.lines[..4].to_vec(),
            };
            assert!(fax.arriving(arriving, n as f64), "page {sheet} is not a new page");
            fax.arrived(sheet, page.clone());
        }
        let kept: Vec<(usize, usize)> = fax.earlier.iter().map(|(n, p)| (*n, p.lines.len())).collect();
        assert_eq!(kept, [(1, 10), (2, 11)]);
        assert_eq!(fax.in_view().map(|(n, l, _)| (n, l.len())), Some((3, 12)));

        fax.look(-1);
        assert_eq!(fax.in_view().map(|(n, l, _)| (n, l.len())), Some((2, 11)));
        fax.look(-1);
        fax.look(-1);
        assert_eq!(fax.in_view().map(|(n, _, _)| n), Some(1), "went before the first");
        fax.look(1);
        fax.look(1);
        assert_eq!(fax.in_view().map(|(n, _, _)| n), Some(3));
        assert!(fax.looking.is_none(), "the last page is the one arriving");
        fax.look(1);
        assert_eq!(fax.in_view().map(|(n, _, _)| n), Some(3), "went past the last");

        let dir = std::env::temp_dir().join("binmodem-faxwin-pages");
        let _ = std::fs::create_dir_all(&dir);
        fax.save_all(&dir.join("call.png"));
        for (n, page) in pages.iter().enumerate() {
            let path = dir.join(format!("call-{}.png", n + 1));
            let image = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(image.height() as usize, page.lines.len() * 2);
            let _ = std::fs::remove_file(&path);
        }
        assert!(fax.saved.as_deref().is_some_and(|s| s.starts_with("3 pages")), "{:?}", fax.saved);

        // A new call's first page, and the last call's pages go.
        let arriving = Arriving {
            page: 20,
            sheet: 1,
            resolution: Resolution::Standard,
            from: 0,
            lines: pages[0].lines[..2].to_vec(),
        };
        assert!(fax.arriving(arriving, 9.0));
        assert!(fax.earlier.is_empty(), "the last call's pages are still there");
    }

    /// The badges' colours rank the codings and the carriers, the best of
    /// each in green: JBIG and V.34.
    #[test]
    fn the_best_coding_and_carrier_are_green_and_the_rest_are_not() {
        assert_eq!(coding_colour("JBIG"), BEST);
        for worse in ["MMR", "MR", "MH"] {
            assert_ne!(coding_colour(worse), BEST, "{worse}");
        }
        assert_eq!(modulation_colour("V.34"), BEST);
        for slower in ["V.17", "V.29", "V.27ter", "V.21"] {
            assert_ne!(modulation_colour(slower), BEST, "{slower}");
        }
    }

    /// What the page goes in is taken only once a page is moving -- before
    /// the command settles it every call reports MH -- and is kept through
    /// the turnaround after it, when the page's progress has gone.
    #[test]
    fn what_a_page_goes_in_is_kept_from_the_page_and_not_before() {
        let mut fax = Fax::new();
        let mut frame = telemetry::Frame::new(16, 16, 16_000.0);
        frame.fax_coding = "MH";
        frame.modulation = "V.34";
        frame.fax_rate = 33_600;
        frame.fax_symbol_rate = 3429;
        frame.fax_error_correction = true;
        fax.observe(&frame);
        assert_eq!(fax.carried, None, "taken before a page was moving");

        frame.fax_coding = "JBIG";
        frame.fax_progress = Some(0.25);
        fax.observe(&frame);
        let page = Carried {
            coding: "JBIG",
            modulation: "V.34",
            rate: 33_600,
            symbol_rate: 3429,
            correcting: true,
        };
        assert_eq!(fax.carried, Some(page));

        frame.fax_progress = None;
        fax.observe(&frame);
        assert_eq!(fax.carried, Some(page), "lost in the turnaround after the page");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("binmodem-faxwin-pdf");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn a_pdf_goes_a_page_to_a_sheet_and_a_picture_as_one() {
        let pages: Vec<Page> = (0..3).map(|n| page_of(300 + 100 * n)).collect();
        let sheets: Vec<fax::pdf::Sheet<'_>> =
            pages.iter().map(|p| (p.lines.as_slice(), p.resolution)).collect();
        let path = scratch("three.pdf");
        std::fs::write(&path, fax::pdf::document(&sheets)).unwrap();
        let mut fax = Fax::new();
        fax.load(&path);
        assert_eq!(fax.trouble, None);
        assert_eq!(fax.source_said, ("document", "3 pages".to_owned()));
        let heights: Vec<usize> = fax.pages().iter().map(Page::height).collect();
        assert_eq!(heights, [300, 400, 500], "the pages are not the lengths they were");

        // Fine: every page drawn again, with twice the lines.
        fax.showing = 2;
        fax.resolution = Resolution::Fine;
        fax.render();
        let heights: Vec<usize> = fax.pages().iter().map(Page::height).collect();
        assert_eq!(heights, [600, 800, 1000]);
        assert_eq!(fax.showing, 2, "the preview went back to the first page");
        let codings = [Coding::ModifiedHuffman, Coding::ModifiedRead, Coding::Mmr, Coding::Jbig];
        for (total, coding) in fax.coded_bits.into_iter().zip(codings) {
            let each: usize = fax.pages().iter().map(|p| coding.encode(&p.lines, p.resolution, 0).len()).sum();
            assert_eq!(total, each, "{coding:?} is not the whole fax");
        }

        // A picture is one page, whatever came before it.
        let picture = scratch("square.png");
        image::GrayImage::from_pixel(40, 40, image::Luma([0])).save(&picture).unwrap();
        fax.load(&picture);
        assert_eq!(fax.pages().len(), 1);
        assert_eq!(fax.showing, 0);
        assert_eq!(fax.source_said, ("picture", "40 by 40".to_owned()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&picture);
    }

    #[test]
    fn a_received_page_saved_as_a_pdf_loads_back_as_the_same_page() {
        // Forwarding a fax: what arrived, saved, and put in to go out again.
        let page = page_of(700);
        let mut fax = Fax::new();
        fax.arrived(1, page.clone());
        let path = scratch("forward.pdf");
        fax.save(&path);
        assert!(fax.saved.as_deref().is_some_and(|s| s.starts_with("saved to")), "{:?}", fax.saved);
        fax.load(&path);
        assert_eq!(fax.trouble, None);
        assert_eq!(fax.pages().len(), 1);
        let back = &fax.pages()[0];
        let wrong = back.lines.iter().flatten().zip(page.lines.iter().flatten()).filter(|(a, b)| a != b).count();
        assert_eq!(back.height(), page.height());
        assert_eq!(wrong, 0, "{wrong} pels came back different");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_page_of_a_call_saves_into_one_pdf() {
        let pages: Vec<Page> = (0..3).map(|n| page_of(10 + n)).collect();
        let mut fax = Fax::new();
        for (n, page) in pages.iter().enumerate() {
            fax.arrived(n + 1, page.clone());
        }
        let path = scratch("call.pdf");
        fax.save_all(&path);
        assert!(fax.saved.as_deref().is_some_and(|s| s.starts_with("3 pages saved to")), "{:?}", fax.saved);
        let pdf = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert_eq!(text.matches("/Type /Page ").count(), 3);
        assert!(!scratch("call-1.png").exists(), "it wrote pictures as well");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_document_longer_than_a_fax_goes_as_far_as_it_can() {
        let lines = lines_of(4, true);
        let sheets: Vec<fax::pdf::Sheet<'_>> =
            (0..document::MOST_PAGES + 7).map(|_| (lines.as_slice(), Resolution::Standard)).collect();
        let path = scratch("long.pdf");
        std::fs::write(&path, fax::pdf::document(&sheets)).unwrap();
        let mut fax = Fax::new();
        fax.load(&path);
        assert_eq!(fax.pages().len(), document::MOST_PAGES);
        assert_eq!(fax.source_said.1, format!("the first 50 of its {} pages", document::MOST_PAGES + 7));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_that_is_neither_says_so_and_leaves_nothing_to_send() {
        let path = scratch("broken.pdf");
        std::fs::write(&path, b"%PDF-1.4\nnot really").unwrap();
        let mut fax = Fax::new();
        fax.load(&path);
        assert!(fax.trouble.is_some());
        assert!(fax.pages().is_empty());
        assert!(fax.source.is_none());
        let _ = std::fs::remove_file(&path);
    }
}
