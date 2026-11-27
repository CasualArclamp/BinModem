//! The Fax tab: a picture, a page, what the machine at the far end is, and
//! how each call went.
//!
//! Everything the page needs is done here and now, before any line is
//! involved: a fax is a page long before it is a signal, and the part that
//! decides whether the far end can read it is the part that turns a
//! photograph into ink and paper.

use eframe::egui;
use egui::{Color32, RichText};
use fax::coding::Coding;
use fax::page::{Grey, Halftone, Page, Resolution};
use fax::picture::{Converter, Picture};
use fax::call::Exchanged;
use fax::t30::{self, Frame};
use std::sync::Arc;

use crate::document;
use crate::live::Arriving;

/// What the window is holding.
///
/// Not `Debug`: a texture handle is not, and a page is a megabyte of booleans
/// nobody wants printed.
#[derive(Default)]
pub struct Fax {
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
    /// The same pages with tone, if they are to go so: one for each page, or
    /// none at all.
    toned: Vec<Toned>,
    /// The page of those in the preview, decoded from its stream, and which
    /// it is: what the far end will see, the coding's own marks included.
    tone_shown: Option<(usize, Picture)>,
    /// What the pages go as where the far end takes it, and how hard a page
    /// with tone is squeezed.
    pub tone: SendTone,
    pub quality: Quality,
    /// How long the pages code to in each coding -- MH, MR, MMR and JBIG --
    /// all of them together, worked out once when they are made.
    ///
    /// Coding a full page is two to four milliseconds, and the panel wants
    /// the figures once for itself and again for each rate it could go out
    /// at. All of those in every frame of a window that is open for the whole
    /// of a call is tens of milliseconds a frame spent recomputing numbers
    /// that cannot have changed.
    coded_bits: [usize; 4],
    /// The same, a page at a time, for the time left on the page going.
    coded_pages: Vec<[usize; 4]>,
    /// Where each line of a page ends in its bits, in the coding a call is
    /// sending it in, worked out the first time a call sends that page that
    /// way.
    line_ends: Vec<Option<LineMap>>,
    /// Which of the pages the preview is of.
    showing: usize,
    /// A small copy of that page for the window to draw.
    preview: Option<egui::TextureHandle>,
    /// Pels to a texel across the preview, as it was made.
    preview_scale: usize,
    /// How big the page is drawn, the same for the page going and the page
    /// arriving.
    zoom: Zoom,
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
    /// Whether this end offers JPEG, for greyscale and colour pages, which
    /// goes only with error correction too.
    pub colour: bool,
    /// What the far end said, once it has said anything.
    pub far: Option<t30::Capabilities>,
    /// The field it was read from, as it arrived: a DIS when this end is
    /// sending, and the caller's DCS when this end is receiving.
    far_field: Option<Vec<u8>>,
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
    /// What this end calls itself. The number to dial is the dialler's.
    pub identification: String,
    /// The calls since the window opened, the last the one in progress if
    /// one is.
    pub calls: Vec<CallRecord>,
    /// A page of an earlier call opened from the list: which call, which of
    /// its pages, and its picture.
    opened: Option<(usize, usize, Scan)>,
    /// Which page the big picture shows.
    pub view: View,
    /// Lines arriving: when the page's count was taken, the count, and lines
    /// a second since.
    pace: Option<(f64, usize, f64)>,
    /// Who the call in progress is to, until the far end says who it is.
    pub dialled: String,
    /// What this call's page is going, or went, in -- kept from the moment a
    /// page starts moving until the call is over, so that it does not blink
    /// out in the turnaround between one page and the next.
    carried: Option<Carried>,
    /// A page that arrived: the one `scan` is a picture of.
    incoming: Option<Page>,
    /// That page as a picture, if it came with tone.
    incoming_picture: Option<Arc<Picture>>,
    /// The page arriving, or the last one that did, drawn as it came in.
    scan: Option<Scan>,
    /// The pages of the same call that arrived before that one, with their
    /// numbers in it.
    earlier: Vec<(usize, Page)>,
    /// And the pictures of those that came with tone, by the same numbers.
    earlier_pictures: Vec<(usize, Arc<Picture>)>,
    /// One of those being looked at instead, by its place in `earlier`, and
    /// its picture.
    looking: Option<(usize, Scan)>,
    /// What was said about saving the last page.
    pub saved: Option<String>,
}

/// A page's line ends in one coding: being worked out on a thread of its
/// own -- a dense page at fine is most of a second of coding and decoding,
/// which the window would otherwise spend frozen as the page starts -- or
/// ready, with the page's bits in all.
enum LineMap {
    Working(&'static str, std::sync::mpsc::Receiver<(Vec<usize>, usize)>),
    Ready(&'static str, Vec<usize>, usize),
}

/// What the pages go as, where the far end can take it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SendTone {
    /// Ink and paper, which every fax machine reads.
    #[default]
    Ink,
    /// Lightness alone, in JPEG (T.30 Table 2 bit 68).
    Greyscale,
    /// Lightness and colour, in JPEG (bits 68 and 69).
    Colour,
}

impl SendTone {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ink => "ink",
            Self::Greyscale => "greyscale",
            Self::Colour => "colour",
        }
    }
}

/// How hard a page with tone is squeezed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    Small,
    #[default]
    Normal,
    Best,
}

impl Quality {
    pub fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Normal => "normal",
            Self::Best => "best",
        }
    }

    /// The quality the JPEG encoder is given, 1 to 100: how the tables of
    /// T.81 Annex K are scaled.
    pub fn value(self) -> u8 {
        match self {
            Self::Small => 40,
            Self::Normal => 75,
            Self::Best => 90,
        }
    }
}

/// A page with tone, coded and ready to go.
struct Toned {
    /// The page as the JPEG stream it goes as. Only the stream and what
    /// describes it: the samples are left out, since a sheet of them is
    /// twelve megabytes and a document may have fifty.
    picture: Picture,
    /// The same page as lightness alone, for a far end that reads greyscale
    /// and not colour, if `picture` is in colour.
    lightness: Option<Picture>,
    /// How many bits of the stream have gone when each line is whole.
    ends: Vec<usize>,
}

/// What the pages to send are made from.
enum Source {
    /// A picture: grey with square pixels, fitted to the paper as one page,
    /// and the same picture in its colours.
    Picture(Grey, Arc<image::RgbImage>),
    /// A PDF, kept as the file it came in: its pages are drawn onto the paper
    /// again for another resolution, which is quicker than keeping them drawn.
    Document(Arc<Vec<u8>>),
}

/// What a page is carried in: its coding, the modulation under it and its
/// rates, and whether error correction is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
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

/// Texels across the picture of a page arriving: one for every pel, so the
/// page is drawn a screen pixel a pel with nothing smoothed or averaged.
const SCAN_WIDTH: usize = fax::page::WIDTH;

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
/// The ground of the badge on a page that came with tone, greyscale or
/// colour: not on the scale from green to red, because JPEG is not a smaller
/// or a larger coding of the same page but a coding of another kind of page.
const TONE: Color32 = Color32::from_rgb(190, 150, 235);

/// A page coding's colour, by how small it makes a page: T.85's JBIG, then
/// T.6's MMR, then T.4's MR and MH.
fn coding_colour(coding: &str) -> Color32 {
    match coding {
        "JPEG" => TONE,
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

/// What the badge on a page with tone says when pointed at.
const TONE_TIP: &str = "This page came as JPEG (T.4 Annex E): greyscale or colour in CIELAB, \
                        eight bits a component. It is shown as sRGB";

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

/// Screen pixels down for each line at `resolution`: as many as make the
/// pels square, at least one -- two at standard, one at fine and above.
fn pixels_per_line(resolution: Resolution) -> usize {
    (fax::page::PELS_PER_MM / resolution.lines_per_mm()).round().max(1.0) as usize
}

/// How big to draw a picture `width` texels across and `rows` down, in
/// points, each texel `m` screen pixels square: never a fraction of a pixel,
/// so nothing is smoothed.
fn picture_size(ppp: f32, width: usize, rows: usize, m: usize) -> egui::Vec2 {
    egui::vec2((width * m) as f32 / ppp, (rows * m) as f32 / ppp)
}

/// Row `row` of a page drawn `n` pels to a texel, onto `out`.
///
/// The page is taken with each line repeated [`pixels_per_line`] times, so
/// its pels are square, and each texel is the ink of the `n` by `n` block
/// under it: black or white at 1:1, and grey where a block is part ink when
/// shrunk, so a thin line fades rather than vanishing. A block past the
/// last line counts only what is there.
fn block_row(lines: &[Vec<bool>], ppl: usize, n: usize, row: usize, out: &mut Vec<Color32>) {
    let width = SCAN_WIDTH.div_ceil(n);
    let tall = lines.len() * ppl;
    let (y0, y1) = (row * n, ((row + 1) * n).min(tall));
    for tx in 0..width {
        let (x0, x1) = (tx * n, ((tx + 1) * n).min(SCAN_WIDTH));
        let mut ink = 0;
        for sy in y0..y1 {
            let line = &lines[sy / ppl];
            for x in x0..x1 {
                ink += usize::from(line.get(x).is_some_and(|pel| *pel));
            }
        }
        let count = ((y1 - y0) * (x1 - x0)).max(1);
        let v = (255 - ink * 255 / count) as u8;
        out.push(Color32::from_rgb(v, v, v));
    }
}

/// What a page with tone is called on its badge.
fn tone_said(picture: &Picture) -> &'static str {
    if picture.is_colour() { "JPEG colour" } else { "JPEG grey" }
}

/// Row `row` of a page with tone drawn `n` pels to a texel, onto `out`: as
/// [`block_row`], with each texel the mean of the CIELAB codes under it,
/// turned to sRGB. Past the edge of a picture narrower than the paper is
/// paper.
fn tone_row(picture: &Picture, converter: &Converter, ppl: usize, n: usize, row: usize, out: &mut Vec<Color32>) {
    let width = SCAN_WIDTH.div_ceil(n);
    let tall = picture.height * ppl;
    let (y0, y1) = (row * n, ((row + 1) * n).min(tall));
    let paper = picture.gamut.code(0, 100.0);
    let across = picture.width * picture.components;
    for tx in 0..width {
        let (x0, x1) = (tx * n, ((tx + 1) * n).min(SCAN_WIDTH));
        let mut sum = [0usize; 3];
        for sy in y0..y1 {
            let line = &picture.samples[(sy / ppl) * across..(sy / ppl + 1) * across];
            for x in x0..x1 {
                if x >= picture.width {
                    sum[0] += usize::from(paper);
                    sum[1] += 128;
                    sum[2] += 128;
                    continue;
                }
                let at = x * picture.components;
                for (c, total) in sum.iter_mut().enumerate().take(picture.components) {
                    *total += usize::from(line[at + c]);
                }
            }
        }
        let count = ((y1 - y0) * (x1 - x0)).max(1);
        let mean = |total: usize| ((total + count / 2) / count) as u8;
        let [r, g, b] = if picture.is_colour() && x0 < picture.width {
            converter.rgb([mean(sum[0]), mean(sum[1]), mean(sum[2])])
        } else {
            converter.grey(mean(sum[0]))
        };
        out.push(Color32::from_rgb(r, g, b));
    }
}

/// How much of a page the panel shows: whole screen pixels to a texel, or
/// whole texels to a screen pixel, and nothing in between.
///
/// Until the wheel is turned over the page it fits the page across the
/// panel, shrinking it by the least whole number that does; the wheel then
/// steps through 1:7 .. 1:2, 1:1, 2:1 .. 8:1 about the pointer, and a
/// double click goes back to fitting.
#[derive(Default)]
struct Zoom {
    /// The level the wheel chose, if it has: below zero shrinks by one
    /// more than its size, zero is 1:1, above it magnifies by one more.
    level: Option<i32>,
    /// Wheel turned but not yet a whole step.
    wheel: f32,
    /// Where the page was shown last frame and how far it was scrolled.
    inner: Option<egui::Rect>,
    offset: egui::Vec2,
}

/// Points of a touchpad's scrolling to a step of zoom, about a notch of a
/// mouse wheel.
const ZOOM_STEP: f32 = 50.0;

impl Zoom {
    /// The level that fits the page across `room` screen pixels: 1:1 when
    /// it does, never magnified.
    fn fit(room: f32) -> i32 {
        1 - (SCAN_WIDTH as f32 / room.max(1.0)).ceil().max(1.0) as i32
    }

    /// Pels to a texel, and screen pixels to a texel, at `level`.
    fn factors(level: i32) -> (usize, usize) {
        if level < 0 { ((1 - level) as usize, 1) } else { (1, (level + 1) as usize) }
    }

    fn scale(level: i32) -> f32 {
        let (n, m) = Self::factors(level);
        m as f32 / n as f32
    }

    /// Take the wheel and a double click over the page, before it is
    /// drawn: the pels to a texel, screen pixels to a texel, the level, and
    /// where to scroll to keep the spot under the pointer still if the zoom
    /// changed.
    fn begin(&mut self, ui: &mut egui::Ui, room: f32) -> (usize, usize, i32, Option<egui::Vec2>) {
        let fit = Self::fit(room);
        let old = self.level.unwrap_or(fit);
        let mut new = old;
        let mut anchor = None;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        match (self.inner, pointer) {
            (Some(inner), Some(p)) if inner.contains(p) => {
                // Steps from the wheel itself, a notch a step, and the
                // scrolling it would have done taken away from the page.
                self.wheel += ui.input_mut(|i| {
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                    i.raw
                        .events
                        .iter()
                        .map(|e| match e {
                            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => {
                                delta.y / ZOOM_STEP
                            }
                            egui::Event::MouseWheel { delta, .. } => delta.y,
                            _ => 0.0,
                        })
                        .sum::<f32>()
                });
                while self.wheel >= 1.0 {
                    new += 1;
                    self.wheel -= 1.0;
                }
                while self.wheel <= -1.0 {
                    new -= 1;
                    self.wheel += 1.0;
                }
                new = new.clamp(-6, 7);
                if ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
                    self.level = None;
                    new = fit;
                } else if new != old {
                    self.level = Some(new);
                }
                if new != old {
                    let at = p - inner.min;
                    let r = Self::scale(new) / Self::scale(old);
                    anchor = Some(((self.offset + at) * r - at).max(egui::Vec2::ZERO));
                }
            }
            _ => self.wheel = 0.0,
        }
        let (n, m) = Self::factors(new);
        (n, m, new, anchor)
    }

    /// Where the page was shown, for the next frame's wheel, and the zoom
    /// in the corner.
    fn end(&mut self, ui: &mut egui::Ui, inner: egui::Rect, offset: egui::Vec2, level: i32) {
        self.inner = Some(inner);
        self.offset = offset;
        let (n, m) = Self::factors(level);
        let text = format!(
            "{m}:{n}{} · wheel to zoom, double click to fit",
            if self.level.is_none() { " fit" } else { "" }
        );
        let font = egui::FontId::monospace(11.0);
        let galley = ui.painter().layout_no_wrap(text, font, Color32::from_gray(200));
        let at = egui::pos2(inner.right() - galley.size().x - 18.0, inner.top() + 4.0);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(at, galley.size()).expand(3.0),
            3.0,
            Color32::from_rgba_unmultiplied(13, 17, 23, 210),
        );
        ui.painter().galley(at, galley, Color32::from_gray(200));
    }
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
    /// The same lines as a picture, if the page has tone, and what turns its
    /// codes into the colours of a screen. A page with one is drawn from it,
    /// and `lines` is then its lightness as ink, kept for what wants that.
    tone: Option<(Picture, Converter)>,
    /// The rows finished so far, SCAN_WIDTH texels to a row.
    pixels: Vec<Color32>,
    rows: usize,
    /// When the last line came in, by the window's clock.
    last_line: f64,
    /// The rows the texture has room for, and how many it has been given.
    texture: Option<egui::TextureHandle>,
    capacity: usize,
    uploaded: usize,
    /// Pels to a texel, across and down the squared-up page.
    scale: usize,
}

impl Scan {
    /// The picture of a page that is already whole, in its tone if it came
    /// with any.
    fn of_tone(sheet: usize, page: &Page, picture: Option<&Picture>) -> Self {
        let mut scan = Self::new(u64::MAX, sheet, page.resolution);
        match picture {
            // As many lines of each as there are of both: they are the same
            // page, and one that was cut short is cut short in both.
            Some(picture) => {
                let lines = page.lines.len().min(picture.height);
                let mut part = picture.clone();
                part.jpeg = Vec::new();
                part.height = lines;
                part.samples.truncate(lines * picture.width * picture.components);
                scan.extend_in_tone(0, page.lines[..lines].to_vec(), Some(part));
            }
            None => scan.extend(0, page.lines.clone()),
        }
        scan
    }

    fn new(page: u64, sheet: usize, resolution: Resolution) -> Self {
        Self {
            page,
            sheet,
            resolution,
            lines: Vec::new(),
            tone: None,
            pixels: Vec::new(),
            rows: 0,
            last_line: f64::NEG_INFINITY,
            texture: None,
            capacity: 0,
            uploaded: 0,
            scale: 1,
        }
    }

    /// Texels across the picture.
    fn width(&self) -> usize {
        SCAN_WIDTH.div_ceil(self.scale)
    }

    /// Draw `n` pels to a texel from now on: every row again from the lines
    /// kept, and a texture to match.
    fn set_scale(&mut self, n: usize) {
        let n = n.max(1);
        if n == self.scale {
            return;
        }
        self.scale = n;
        self.redraw();
        self.finish_rows();
    }

    fn finish_rows(&mut self) {
        let ppl = pixels_per_line(self.resolution);
        while (self.rows + 1) * self.scale <= self.lines.len() * ppl {
            self.finish_row();
        }
    }

    /// Lines of the page under one row of the picture.
    #[cfg(test)]
    fn lines_per_row(&self) -> f64 {
        self.scale as f64 / pixels_per_line(self.resolution) as f64
    }

    /// The lines under row `row`, whole or in part.
    #[cfg(test)]
    fn span(&self, row: usize) -> std::ops::Range<usize> {
        let ppl = pixels_per_line(self.resolution);
        row * self.scale / ppl..((row + 1) * self.scale).div_ceil(ppl)
    }

    /// Lines starting at line `from`, and every row they finish.
    ///
    /// Lines already here are skipped, which is what a batch that overlaps the
    /// page it continues needs. A batch that starts past the end leaves a gap
    /// that nothing can fill, and is left out.
    fn extend(&mut self, from: usize, lines: Vec<Vec<bool>>) {
        self.extend_in_tone(from, lines, None);
    }

    /// Lines starting at line `from`, and the same lines as a picture if the
    /// page has tone.
    ///
    /// A page is drawn in tone from its first line or not at all: a picture
    /// that starts part way down a page drawn as ink so far is left out, and
    /// so is one that stops matching the picture there is.
    fn extend_in_tone(&mut self, from: usize, lines: Vec<Vec<bool>>, tone: Option<Picture>) {
        let have = self.lines.len();
        if from > have {
            return;
        }
        let skip = have - from;
        let fresh = lines.len().saturating_sub(skip);
        match (self.tone.as_mut(), tone) {
            (None, Some(mut picture)) if have == 0 && picture.height == lines.len() => {
                picture.jpeg = Vec::new();
                let converter = Converter::new(picture.gamut);
                self.tone = Some((picture, converter));
            }
            (Some((picture, _)), Some(more))
                if more.width == picture.width
                    && more.components == picture.components
                    && more.height == lines.len() =>
            {
                let across = more.width * more.components;
                picture.samples.extend_from_slice(&more.samples[skip.min(more.height) * across..]);
                picture.height += fresh;
            }
            (Some(_), _) if fresh > 0 => {
                // Lines with no picture to go with them: the page goes on
                // as ink, which is at least all of it.
                self.tone = None;
                self.redraw();
            }
            _ => {}
        }
        self.lines.extend(lines.into_iter().skip(skip));
        self.finish_rows();
    }

    /// Every row again, from the lines kept.
    fn redraw(&mut self) {
        self.pixels.clear();
        self.rows = 0;
        self.texture = None;
        self.capacity = 0;
        self.uploaded = 0;
    }

    /// The page as a picture, if it has tone, as far as it has come.
    fn picture(&self) -> Option<&Picture> {
        self.tone.as_ref().map(|(picture, _)| picture)
    }

    /// The next row, from the lines under it.
    fn finish_row(&mut self) {
        let ppl = pixels_per_line(self.resolution);
        match self.tone.as_ref() {
            Some((picture, converter)) => {
                tone_row(picture, converter, ppl, self.scale, self.rows, &mut self.pixels);
            }
            None => block_row(&self.lines, ppl, self.scale, self.rows, &mut self.pixels),
        }
        self.rows += 1;
    }

    /// Rows in a whole A4 sheet at this page's resolution.
    fn sheet_rows(&self) -> usize {
        (self.resolution.lines() * pixels_per_line(self.resolution)).div_ceil(self.scale)
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
        let width = self.width();
        let outgrown = self.rows > self.capacity && self.capacity < largest;
        if self.texture.is_none() || outgrown {
            let sheet = self.sheet_rows();
            self.capacity = sheet.max(self.rows + self.rows / 2).min(largest);
            let upto = self.shown();
            let mut pixels = self.pixels[..upto * width].to_vec();
            pixels.resize(width * self.capacity, UNSCANNED);
            self.texture = Some(ctx.load_texture(
                "fax-arriving",
                egui::ColorImage::new([width, self.capacity], pixels),
                egui::TextureOptions::NEAREST,
            ));
            self.uploaded = upto;
        }
        let upto = self.shown();
        if self.uploaded < upto
            && let Some(texture) = self.texture.as_mut()
        {
            let strip = egui::ColorImage::new(
                [width, upto - self.uploaded],
                self.pixels[self.uploaded * width..upto * width].to_vec(),
            );
            texture.set_partial([0, self.uploaded], strip, egui::TextureOptions::NEAREST);
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

/// Write a page with tone out as a picture: its CIELAB codes as sRGB, a pel
/// to a pixel, since at 200 pels to 25.4 mm both ways its pels are square.
fn write_tone(picture: &Picture, path: &std::path::Path) -> Result<(), String> {
    let (width, height) = (picture.width as u32, picture.height as u32);
    let rgb = picture.rgb();
    let unshaped = || "the page did not come out rectangular".to_owned();
    if picture.is_colour() {
        let image = image::RgbImage::from_raw(width, height, rgb).ok_or_else(unshaped)?;
        image.save(path).map_err(|e| format!("{e}"))
    } else {
        let grey = rgb.as_chunks::<3>().0.iter().map(|pel| pel[0]).collect();
        let image = image::GrayImage::from_raw(width, height, grey).ok_or_else(unshaped)?;
        image.save(path).map_err(|e| format!("{e}"))
    }
}

/// A page to save: its number in its call, its lines of ink, how finely they
/// were scanned, and the page as a picture if it came with tone.
type Kept<'a> = (usize, &'a [Vec<bool>], Resolution, Option<&'a Picture>);

/// Write a page out as a picture, in its tone if it has any.
fn write_kept((_, lines, resolution, tone): &Kept<'_>, path: &std::path::Path) -> Result<(), String> {
    match tone {
        Some(picture) => write_tone(picture, path),
        None => write_page(lines, *resolution, path),
    }
}

/// Write pages out as one PDF, each on a sheet the size it was scanned at: a
/// page of ink in T.6, and a page with tone as the JPEG it arrived in.
fn write_pdf(pages: &[Kept<'_>], path: &std::path::Path) -> Result<(), String> {
    let leaves: Vec<fax::pdf::Leaf<'_>> = pages
        .iter()
        .map(|(_, lines, resolution, tone)| match tone {
            Some(picture) => fax::pdf::Leaf::Tone(picture),
            None => fax::pdf::Leaf::Ink(lines, *resolution),
        })
        .collect();
    std::fs::write(path, fax::pdf::document_of(&leaves)).map_err(|e| format!("{e}"))
}

/// Whether a name chosen to save to asks for a PDF.
fn names_pdf(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Which page the big picture shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// Whatever the call is doing: the page going out while one goes, the
    /// page arriving while one arrives.
    #[default]
    Follow,
    Sending,
    Received,
}

/// One fax call, from its first phase to the account of how it went.
#[derive(Debug, Clone)]
pub struct CallRecord {
    /// This end dialled and sent, rather than answered and received.
    pub sending: bool,
    /// By the window's clock.
    pub started: f64,
    pub ended: Option<f64>,
    /// What the far end called itself, or the number dialled until it says.
    pub far: String,
    /// Every T.30 frame of the call, either end's, in order.
    pub frames: Vec<Exchanged>,
    /// The last phase the call was in before it was over: where it ended,
    /// if it ended badly.
    pub last_phase: Option<&'static str>,
    pub trouble: Option<String>,
    /// The highest page number the call reached.
    pub sheets: usize,
    pub carried: Option<Carried>,
    /// Put down with Stop rather than ended by the procedure.
    pub stopped_here: bool,
    /// The pages that arrived, with their numbers in the call.
    pub received: Vec<(usize, Page)>,
    /// The pictures of those that came with tone, by the same numbers.
    pub pictures: Vec<(usize, Arc<Picture>)>,
}

/// How a call came out, as far as T.30 says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Still going.
    Going,
    /// The last page answered with MCF, and the call ended as one should.
    Confirmed,
    /// The last page answered with RTP (kept, but train again) or RTN (not
    /// kept: train again and send it again).
    RetrainAsked(Frame),
    /// It ended somewhere else: the T.30 phase it was in, and why.
    Failed { phase: char, why: String },
}

impl CallRecord {
    fn new(sending: bool, started: f64, far: &str) -> Self {
        Self {
            sending,
            started,
            ended: None,
            far: far.to_owned(),
            frames: Vec::new(),
            last_phase: None,
            trouble: None,
            sheets: 0,
            carried: None,
            stopped_here: false,
            received: Vec::new(),
            pictures: Vec::new(),
        }
    }

    /// A frame the receiving end sends in answer to a page.
    fn is_response(frame: Frame) -> bool {
        matches!(frame, Frame::Mcf | Frame::Rtp | Frame::Rtn | Frame::Ppr)
    }

    /// A frame the sending end sends after a page.
    fn is_post_message(frame: Frame) -> bool {
        matches!(frame, Frame::Mps | Frame::Eop | Frame::Eom | Frame::Pps)
    }

    /// The last answer to a page, from whichever end received.
    fn last_response(&self) -> Option<Frame> {
        self.frames
            .iter()
            .rev()
            .find(|f| f.ours != self.sending && Self::is_response(f.frame))
            .map(|f| f.frame)
    }

    /// Pages the receiving end said it had: MCF or RTP straight after an MPS,
    /// EOP or EOM, counted once however many times the command went. Under
    /// error correction a page is many blocks and many MCFs, so there it is
    /// the page the call reached, less one if the last was not confirmed.
    pub fn confirmed_pages(&self) -> usize {
        if self.frames.iter().any(|f| f.frame == Frame::Pps) {
            let last_good = self.last_response() == Some(Frame::Mcf);
            return if last_good { self.sheets } else { self.sheets.saturating_sub(1) };
        }
        let mut asked = false;
        let mut count = 0;
        for f in &self.frames {
            if f.ours == self.sending && matches!(f.frame, Frame::Mps | Frame::Eop | Frame::Eom) {
                asked = true;
            } else if asked && f.ours != self.sending && Self::is_response(f.frame) {
                count += usize::from(matches!(f.frame, Frame::Mcf | Frame::Rtp));
                asked = false;
            }
        }
        count
    }

    pub fn verdict(&self) -> Verdict {
        if self.ended.is_none() {
            return Verdict::Going;
        }
        let last_post = self
            .frames
            .iter()
            .rev()
            .find(|f| f.ours == self.sending && Self::is_post_message(f.frame))
            .map(|f| f.frame);
        let hung_up = self.frames.iter().any(|f| f.frame == Frame::Dcn);
        // The last page: EOP, or under error correction the PPS before a
        // disconnect, which is all that can be seen of its EOP from here.
        // Or a receiver that had confirmed the last page and was waiting for
        // the far end to hang up, which T.30 reaches only after the last:
        // a far end that puts the call down as it sends DCN cuts the DCN off.
        let waiting_to_hang_up = !self.sending
            && self.last_phase == Some(fax::call::Phase::AwaitingDisconnect.name());
        let last_page = waiting_to_hang_up
            || match last_post {
                Some(Frame::Eop) => true,
                Some(Frame::Pps) => hung_up,
                _ => false,
            };
        let phase = self
            .last_phase
            .and_then(fax::call::Phase::named)
            .map_or('A', fax::call::Phase::letter);
        let why = || {
            self.trouble.clone().unwrap_or_else(|| {
                if self.stopped_here {
                    "stopped at this end".to_owned()
                } else if self.frames.is_empty() {
                    "no fax machine answered".to_owned()
                } else {
                    "the line went down".to_owned()
                }
            })
        };
        match self.last_response() {
            Some(Frame::Mcf) if last_page && self.trouble.is_none() => Verdict::Confirmed,
            Some(frame @ (Frame::Rtp | Frame::Rtn)) if last_page || self.trouble.is_none() => {
                Verdict::RetrainAsked(frame)
            }
            Some(Frame::Mcf) => Verdict::Failed {
                phase,
                why: format!("{}, after page {} was confirmed", why(), self.confirmed_pages()),
            },
            _ => Verdict::Failed { phase, why: why() },
        }
    }

    /// The verdict in a line, and the colour to draw it in.
    pub fn headline(&self) -> (String, Color32) {
        let pages = |n: usize| if n == 1 { "1 page".to_owned() } else { format!("{n} pages") };
        match self.verdict() {
            Verdict::Going => (
                if self.sending { "sending" } else { "receiving" }.to_owned(),
                GOOD,
            ),
            Verdict::Confirmed if self.sending => (
                format!("Confirmed: the far end took {} (MCF)", pages(self.confirmed_pages())),
                BEST,
            ),
            Verdict::Confirmed => (
                format!("Received {}, confirmed (MCF)", pages(self.received.len().max(self.confirmed_pages()))),
                BEST,
            ),
            Verdict::RetrainAsked(Frame::Rtp) => (
                "Page kept, but the far end asked to train again (RTP)".to_owned(),
                GOOD,
            ),
            Verdict::RetrainAsked(_) => (
                "Page refused: the far end asked to train again and resend (RTN)".to_owned(),
                POOR,
            ),
            Verdict::Failed { phase, why } => (
                format!("Disconnected in phase {phase} ({}): {why}", phase_said(phase)),
                POOR,
            ),
        }
    }

    /// The rest, in a line: pages each way, the carrier and coding, error
    /// correction, and how long.
    pub fn details(&self, now: f64) -> String {
        let mut parts = vec![
            format!("{} sent", if self.sending { self.confirmed_pages() } else { 0 }),
            format!("{} received", self.received.len()),
        ];
        if let Some(c) = self.carried {
            parts.push(match c.symbol_rate {
                0 => format!("{} {}", c.modulation, c.rate),
                baud => format!("{} {} on {baud} baud", c.modulation, c.rate),
            });
            parts.push(c.coding.to_owned());
            parts.push(if c.correcting { "ECM" } else { "no ECM" }.to_owned());
        }
        parts.push(clock(self.ended.unwrap_or(now) - self.started));
        parts.join(" · ")
    }

    /// The result in a word or two, for the session list.
    pub fn short(&self) -> (&'static str, Color32) {
        match self.verdict() {
            Verdict::Going => ("...", GOOD),
            Verdict::Confirmed => ("MCF", BEST),
            Verdict::RetrainAsked(Frame::Rtp) => ("RTP", GOOD),
            Verdict::RetrainAsked(_) => ("RTN", POOR),
            Verdict::Failed { phase: 'A', .. } => ("no fax", POOR),
            Verdict::Failed { phase: 'B', .. } => ("failed B", POOR),
            Verdict::Failed { phase: 'C', .. } => ("failed C", POOR),
            Verdict::Failed { phase: 'D', .. } => ("failed D", POOR),
            Verdict::Failed { .. } => ("failed E", POOR),
        }
    }
}

/// What T.30's phase letters stand for.
fn phase_said(phase: char) -> &'static str {
    match phase {
        'A' => "call set-up",
        'B' => "before the page",
        'C' => "during the page",
        'D' => "after the page",
        _ => "hanging up",
    }
}

/// Seconds as minutes and seconds.
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// How long ago, for the session list: the window has no calendar, only
/// the seconds since it opened, and "4 min ago" says what is wanted.
fn ago(seconds: f64) -> String {
    match seconds.max(0.0) as u64 {
        0..60 => "just now".to_owned(),
        s @ 60..3600 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
}

/// Which of the four codings a name is, as `coded_bits` orders them.
fn coding_index(coding: &str) -> Option<usize> {
    ["MH", "MR", "MMR", "JBIG"].iter().position(|c| *c == coding)
}

/// How far through a fax the sending end is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendProgress {
    pub page_bits: usize,
    pub done_bits: usize,
    /// Seconds left on this page, and on the pages after it, at the rate.
    pub page_left: f64,
    pub rest_left: f64,
}

/// `bits` is what each page codes to, `sheet` the page going (from one) and
/// `fraction` how far through it.
pub fn send_progress(bits: &[usize], sheet: usize, fraction: f64, rate: u32) -> Option<SendProgress> {
    let page_bits = *bits.get(sheet.checked_sub(1)?)?;
    if rate == 0 {
        return None;
    }
    let done_bits = (fraction.clamp(0.0, 1.0) * page_bits as f64).round() as usize;
    let rate = f64::from(rate);
    Some(SendProgress {
        page_bits,
        done_bits,
        page_left: (page_bits - done_bits) as f64 / rate,
        rest_left: bits[sheet..].iter().sum::<usize>() as f64 / rate,
    })
}

/// One row of the table of how long the fax takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateRow {
    pub rate: u32,
    pub modulation: &'static str,
    /// Bits in the smallest coding this end will send at that rate, and in
    /// MH, which every machine reads.
    pub least: usize,
    pub most: usize,
}

/// Fastest first, and at the same rate the better modulation first.
fn modulation_rank(modulation: &str) -> u8 {
    match modulation {
        "V.34" => 0,
        "V.17" => 1,
        "V.29" => 2,
        _ => 3,
    }
}

/// Every rate this end will send at, fastest first. V.34 carries the
/// packed coding, since it always has error correction (T.30 F.3); the rest
/// carry `best`.
pub fn rate_rows(v34: bool, ours: &[t30::Modulation], packed: usize, best: usize, mh: usize) -> Vec<RateRow> {
    let mut rows = Vec::new();
    if v34 {
        rows.push(RateRow { rate: 33_600, modulation: "V.34", least: packed, most: mh });
    }
    for m in ours {
        for rate in m.rates() {
            rows.push(RateRow { rate: *rate, modulation: m.name(), least: best, most: mh });
        }
    }
    rows.sort_by(|a, b| {
        b.rate
            .cmp(&a.rate)
            .then(modulation_rank(a.modulation).cmp(&modulation_rank(b.modulation)))
    });
    rows
}

/// Whether a row is what the call agreed on. Spaces aside, since V.27 ter
/// is written both ways.
pub fn agreed(row: &RateRow, carried: Option<Carried>) -> bool {
    carried.is_some_and(|c| {
        let same = |a: &str, b: &str| a.replace(' ', "") == b.replace(' ', "");
        same(row.modulation, c.modulation) && (row.rate == c.rate || row.modulation == "V.34")
    })
}

/// Write pages out: one PDF if the name ends in `.pdf`, and otherwise a PNG
/// each, `fax.png` becoming `fax-1.png` and so on by the page's number.
/// Returns what to say about it.
fn save_pages(pages: &[Kept<'_>], path: &std::path::Path) -> String {
    if names_pdf(path) {
        return match write_pdf(pages, path) {
            Ok(()) if pages.len() == 1 => format!("saved to {}", path.display()),
            Ok(()) => format!("{} pages saved to {}", pages.len(), path.display()),
            Err(e) => e,
        };
    }
    if let [page] = pages {
        return match write_kept(page, path) {
            Ok(()) => format!("saved to {}", path.display()),
            Err(e) => e,
        };
    }
    let stem = path.file_stem().map_or_else(|| "fax".into(), |s| s.to_string_lossy());
    let named = |sheet: usize| path.with_file_name(format!("{stem}-{sheet}.png"));
    let mut written = 0;
    let mut trouble = None;
    for page in pages {
        match write_kept(page, &named(page.0)) {
            Ok(()) => written += 1,
            Err(e) => trouble = Some(e),
        }
    }
    trouble.unwrap_or_else(|| {
        format!("{written} pages saved as {}", path.with_file_name(format!("{stem}-N.png")).display())
    })
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
            // Offered: bits 68 and 69 in the DIS, so that a machine with a
            // colour scanner may send what it scanned.
            colour: true,
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
        self.toned.clear();
        self.tone_shown = None;
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
            self.source = Some(Source::Picture(
                Grey {
                    width: w,
                    height: h,
                    // Ink, so a white page is nothing and a black one is
                    // everything. The image is the other way round, which is
                    // the only reason this subtraction exists.
                    ink: grey.pixels().map(|p| 1.0 - f32::from(p.0[0]) / 255.0).collect(),
                },
                Arc::new(decoded.to_rgb8()),
            ));
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
            Source::Picture(grey, _) => vec![fax::page::render(grey, self.resolution, self.halftone)],
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
        self.coded_pages.clear();
        self.line_ends = (0..pages.len()).map(|_| None).collect();
        for page in &pages {
            let codings = [Coding::ModifiedHuffman, Coding::ModifiedRead, Coding::Mmr, Coding::Jbig];
            let mut each = [0; 4];
            for ((total, one), coding) in self.coded_bits.iter_mut().zip(each.iter_mut()).zip(codings) {
                *one = coding.encode(&page.lines, page.resolution, 0).len();
                *total += *one;
            }
            self.coded_pages.push(each);
        }
        self.showing = self.showing.min(pages.len().saturating_sub(1));
        self.pages = pages;
        self.preview = None;
        self.tone_shown = None;
        self.toned = self.tone_pages();
    }

    /// The pages again with tone, coded, if that is how they are to go: one
    /// for each page, or none if they go as ink or cannot all be made.
    ///
    /// On the grid of fine resolution, which is also the 200 pels to 25.4 mm
    /// of a colour page (T.4 E.6.5, Note), whatever the pages of ink are at.
    fn tone_pages(&self) -> Vec<Toned> {
        if self.tone == SendTone::Ink {
            return Vec::new();
        }
        let width = fax::page::WIDTH;
        let drawn: Vec<Vec<u8>> = match self.source.as_ref() {
            None => Vec::new(),
            Some(Source::Picture(_, rgb)) => {
                // Each of red, green and blue laid on the paper as the grey
                // is, as ink, so that what the picture does not cover is
                // paper in all three.
                let (w, h) = (rgb.width() as usize, rgb.height() as usize);
                let laid = [0, 1, 2].map(|c| {
                    let channel =
                        Grey { width: w, height: h, ink: rgb.pixels().map(|p| 1.0 - f32::from(p.0[c]) / 255.0).collect() };
                    fax::page::lay(&channel, Resolution::Fine)
                });
                let pels = laid[0].len();
                vec![(0..pels).flat_map(|i| [0, 1, 2].map(|c| ((1.0 - laid[c][i]) * 255.0).round() as u8)).collect()]
            }
            Some(Source::Document(bytes)) => document::read_in_colour(bytes)
                .map(|pages| pages.into_iter().map(|page| page.rgb).collect())
                .unwrap_or_default(),
        };
        if drawn.len() != self.pages.len() {
            return Vec::new();
        }
        let options = fax::jpeg::Options { quality: self.quality.value(), ..fax::jpeg::Options::default() };
        // The stream, and of the picture only what describes it.
        let carrier = |of: &Picture, jpeg: Vec<u8>| Picture { samples: Vec::new(), jpeg, ..of.clone() };
        drawn
            .iter()
            .map(|rgb| {
                let colour = Picture::of_rgb(width, rgb);
                let grey = colour.lightness();
                let main = if self.tone == SendTone::Colour { &colour } else { &grey };
                let (stream, ends) = fax::jpeg::encode_with_lines(main, options);
                Toned {
                    picture: carrier(main, stream),
                    lightness: (self.tone == SendTone::Colour)
                        .then(|| carrier(&grey, fax::jpeg::encode(&grey, options))),
                    ends: ends.into_iter().map(|octets| octets * 8).collect(),
                }
            })
            .collect()
    }

    /// The pages as pictures with tone and as lightness alone, for the call
    /// to take with the pages of ink: empty unless they are to go with tone.
    pub fn pictures(&self) -> crate::live::Pictures {
        (
            self.toned.iter().map(|t| Some(t.picture.clone())).collect(),
            match self.tone {
                SendTone::Colour => self.toned.iter().map(|t| t.lightness.clone()).collect(),
                _ => Vec::new(),
            },
        )
    }

    /// Bits in the pages' JPEG streams, a page at a time.
    fn tone_bits(&self) -> Vec<usize> {
        self.toned.iter().map(|t| t.picture.jpeg.len() * 8).collect()
    }

    /// The page with tone as it will go, `n` pels to a texel: at most
    /// `largest` rows, and at least one.
    fn tone_thumbnail(picture: &Picture, largest: usize, n: usize) -> egui::ColorImage {
        let n = n.max(1);
        let width = SCAN_WIDTH.div_ceil(n);
        let rows = picture.height.div_ceil(n).clamp(1, largest.max(1));
        let converter = Converter::new(picture.gamut);
        let mut pixels = Vec::with_capacity(width * rows);
        if picture.height > 0 {
            for row in 0..rows {
                tone_row(picture, &converter, 1, n, row, &mut pixels);
            }
        }
        pixels.resize(width * rows, Color32::WHITE);
        egui::ColorImage { size: [width, rows], pixels, source_size: egui::Vec2::new(width as f32, rows as f32) }
    }

    /// The page as it will go, `n` pels to a texel on the squared-up page
    /// -- black or white at 1:1, nothing smoothed. At most `largest` rows,
    /// the most the graphics card takes, and at least one, blank, for a page
    /// with no lines.
    fn thumbnail(page: &Page, largest: usize, n: usize) -> egui::ColorImage {
        let n = n.max(1);
        let width = SCAN_WIDTH.div_ceil(n);
        let ppl = pixels_per_line(page.resolution);
        let rows = (page.height() * ppl).div_ceil(n).clamp(1, largest.max(1));
        let mut pixels = Vec::with_capacity(width * rows);
        if !page.lines.is_empty() {
            for row in 0..rows {
                block_row(&page.lines, ppl, n, row, &mut pixels);
            }
        }
        pixels.resize(width * rows, Color32::WHITE);
        egui::ColorImage {
            size: [width, rows],
            pixels,
            source_size: egui::Vec2::new(width as f32, rows as f32),
        }
    }

    fn texture(&mut self, ctx: &egui::Context, n: usize) -> Option<egui::TextureHandle> {
        if self.preview.is_none() || self.preview_scale != n {
            let page = self.pages.get(self.showing)?;
            let largest = ctx.input(|i| i.max_texture_side);
            // A page that goes with tone is shown as its stream decodes.
            if let Some(toned) = self.toned.get(self.showing)
                && self.tone_shown.as_ref().is_none_or(|(at, _)| *at != self.showing)
            {
                let mut decoder = fax::jpeg::Decoder::new(SCAN_WIDTH);
                decoder.feed(&toned.picture.jpeg);
                self.tone_shown = decoder.picture().map(|picture| (self.showing, picture));
            }
            let image = match self.tone_shown.as_ref().filter(|(at, _)| *at == self.showing && !self.toned.is_empty()) {
                Some((_, picture)) => Self::tone_thumbnail(picture, largest, n),
                None => Self::thumbnail(page, largest, n),
            };
            self.preview =
                Some(ctx.load_texture("fax-page", image, egui::TextureOptions::NEAREST));
            self.preview_scale = n;
        }
        self.preview.clone()
    }

    /// Take what the modem knows about the call in progress, at `now` by the
    /// window's clock.
    ///
    /// A call starts when the modem has a fax call in a phase that is not
    /// over, and ends when the phase is over or the modem is on hook again.
    pub fn observe(&mut self, frame: &telemetry::Frame, now: f64) {
        let over = |p: &str| {
            matches!(
                fax::call::Phase::named(p),
                Some(fax::call::Phase::Done | fax::call::Phase::Failed)
            )
        };
        let going = frame.state != telemetry::CallState::Idle && frame.fax_phase.is_some_and(|p| !over(p));
        let live = self.calls.last().is_some_and(|c| c.ended.is_none());
        if going && !live {
            self.carried = None;
            self.far = None;
            self.far_field = None;
            self.far_identity.clear();
            self.far_non_standard = None;
            self.trouble = None;
            self.pace = None;
            self.view = View::Follow;
            self.opened = None;
            self.calls.push(CallRecord::new(frame.fax_sending, now, &self.dialled));
        }
        if let Some(call) = self.calls.last_mut().filter(|c| c.ended.is_none()) {
            call.sending = frame.fax_sending;
            if let Some(p) = frame.fax_phase.filter(|p| !over(p) && *p != "hanging up") {
                call.last_phase = Some(p);
            }
            call.sheets = call.sheets.max(frame.fax_sheet);
            if !frame.fax_identity.trim().is_empty() {
                frame.fax_identity.trim().clone_into(&mut call.far);
            }
            if let Some(trouble) = frame.fax_trouble.as_deref() {
                call.trouble = Some(trouble.to_owned());
            }
            if !going {
                call.ended = Some(now);
            }
        }
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
        //
        // And only while the page's own modulation is the one in use. Between
        // pages, and after the last, the modem is back on V.21 for the
        // frames, with the page's rate and progress still held -- which used
        // to put "V.21 14400" on the badge and the outcome.
        let modulation = match (frame.modulation, self.carried) {
            ("V.21", Some(c)) => Some(c.modulation),
            ("V.21", None) => None,
            (m, _) => Some(m),
        };
        if frame.fax_progress.is_some()
            && frame.fax_rate > 0
            && let Some(modulation) = modulation
        {
            self.carried = Some(Carried {
                coding: frame.fax_coding,
                modulation,
                rate: frame.fax_rate,
                symbol_rate: frame.fax_symbol_rate,
                correcting: frame.fax_error_correction,
            });
            if let Some(call) = self.calls.last_mut().filter(|c| c.ended.is_none()) {
                call.carried = self.carried;
            }
        }
        if !frame.fax_identity.is_empty() {
            self.far_identity = frame.fax_identity.clone();
        }
        if let Some(fif) = frame.fax_non_standard.as_deref() {
            self.far_non_standard = Some(fif.to_vec());
        }
        if let Some(fif) = frame.fax_capabilities.as_deref() {
            self.far_field = Some(fif.to_vec());
            self.far = Some(t30::capabilities(fif));
        }
        if let Some(trouble) = frame.fax_trouble.as_deref() {
            self.trouble = Some(trouble.to_owned());
        }
    }

    /// T.30 frames either end sent. They belong to the last call, which may
    /// have been seen to end a moment before its last frames arrived.
    pub fn exchanged(&mut self, frames: Vec<Exchanged>) {
        if let Some(call) = self.calls.last_mut() {
            call.frames.extend(frames);
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
    #[cfg(test)]
    pub fn arrived(&mut self, sheet: usize, page: Page) {
        self.arrived_in_tone(sheet, page, None);
    }

    /// A page has arrived, and with it the page as a picture if it came with
    /// tone: greyscale or colour.
    pub fn arrived_in_tone(&mut self, sheet: usize, page: Page, picture: Option<Arc<Picture>>) {
        let same = self.scan.as_ref().is_some_and(|scan| scan.sheet == sheet);
        let drawn = same
            && self.scan.as_ref().is_some_and(|scan| {
                scan.lines.len() == page.lines.len()
                    && scan.resolution == page.resolution
                    && scan.tone.is_some() == picture.is_some()
            });
        if !same {
            self.next_sheet(sheet);
        }
        if !drawn {
            // Keep the number of the page being drawn, so the lines of it
            // still on their way are known for what they are.
            let mut scan = Scan::of_tone(sheet, &page, picture.as_deref());
            scan.page = self.scan.as_ref().map_or(u64::MAX, |scan| scan.page);
            self.scan = Some(scan);
        }
        if let Some(call) = self.calls.last_mut() {
            match call.received.iter_mut().find(|(s, _)| *s == sheet) {
                Some(kept) => kept.1 = page.clone(),
                None => call.received.push((sheet, page.clone())),
            }
            call.pictures.retain(|(s, _)| *s != sheet);
            if let Some(picture) = picture.as_ref() {
                call.pictures.push((sheet, picture.clone()));
            }
        }
        self.incoming = Some(page);
        self.incoming_picture = picture;
        self.saved = None;
    }

    /// Another page of a call is starting. The one before it goes with the
    /// call's others -- or, if this is the first page of a call, the last
    /// call's pages go.
    fn next_sheet(&mut self, sheet: usize) {
        if let Some(page) = self.incoming.take() {
            let before = self.scan.as_ref().map_or(0, |scan| scan.sheet);
            self.earlier.push((before, page));
            if let Some(picture) = self.incoming_picture.take() {
                self.earlier_pictures.push((before, picture));
            }
        }
        self.incoming_picture = None;
        if sheet <= 1 {
            self.earlier.clear();
            self.earlier_pictures.clear();
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
        self.looking = self.earlier.get(to).map(|(sheet, page)| {
            let picture = self.earlier_pictures.iter().find(|(s, _)| s == sheet).map(|(_, p)| p.as_ref());
            (to, Scan::of_tone(*sheet, page, picture))
        });
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
            scan.extend_in_tone(arriving.from, arriving.lines, arriving.tone);
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
    ///
    /// A page that came with tone, greyscale or colour, saves in it: the PNG
    /// as sRGB, and the PDF with the JPEG stream as it arrived.
    pub fn save(&mut self, path: &std::path::Path) {
        let Some((sheet, lines, resolution)) = self.in_view() else { return };
        let kept: Kept<'_> = (sheet, lines, resolution, self.picture_in_view());
        let written = if names_pdf(path) { write_pdf(&[kept], path) } else { write_kept(&kept, path) };
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

    /// The page in view as a picture, if it came with tone: the whole of one
    /// that has arrived, with its stream, and as much as has come of one
    /// still arriving, without.
    fn picture_in_view(&self) -> Option<&Picture> {
        if let Some((i, _)) = &self.looking {
            let (sheet, _) = self.earlier.get(*i)?;
            return self.earlier_pictures.iter().find(|(s, _)| s == sheet).map(|(_, p)| p.as_ref());
        }
        match self.incoming.as_ref() {
            Some(_) => self.incoming_picture.as_deref(),
            None => self.scan.as_ref().and_then(Scan::picture),
        }
    }

    /// What the page in view is called on its badge, if it came with tone.
    fn tone_in_view(&self) -> Option<&'static str> {
        if let Some((call, at, _)) = &self.opened {
            let call = self.calls.get(*call)?;
            let (sheet, _) = call.received.get(*at)?;
            return call.pictures.iter().find(|(s, _)| s == sheet).map(|(_, p)| tone_said(p));
        }
        self.picture_in_view().map(tone_said)
    }

    /// Every page of the call: in one PDF if the name ends in `.pdf`, a page
    /// to a sheet, and otherwise each to its own picture -- `fax.png` becomes
    /// `fax-1.png`, `fax-2.png` and so on, by the page's number in the call.
    pub fn save_all(&mut self, path: &std::path::Path) {
        let stem = path.file_stem().map_or_else(|| "fax".into(), |s| s.to_string_lossy());
        let named = |sheet: usize| path.with_file_name(format!("{stem}-{sheet}.png"));
        let mut pages: Vec<Kept<'_>> = self
            .earlier
            .iter()
            .map(|(sheet, page)| {
                let picture = self.earlier_pictures.iter().find(|(s, _)| s == sheet).map(|(_, p)| p.as_ref());
                (*sheet, page.lines.as_slice(), page.resolution, picture)
            })
            .collect();
        if let Some(scan) = self.scan.as_ref() {
            match self.incoming.as_ref() {
                Some(page) => {
                    pages.push((scan.sheet, &page.lines, scan.resolution, self.incoming_picture.as_deref()));
                }
                None if !scan.lines.is_empty() => {
                    pages.push((scan.sheet, &scan.lines, scan.resolution, scan.picture()));
                }
                None => {}
            }
        }
        if names_pdf(path) {
            self.saved = Some(match write_pdf(&pages, path) {
                Ok(()) => format!("{} pages saved to {}", pages.len(), path.display()),
                Err(e) => e,
            });
            return;
        }
        let mut written = 0;
        let mut trouble = None;
        for page in &pages {
            match write_kept(page, &named(page.0)) {
                Ok(()) => written += 1,
                Err(e) => trouble = Some(e),
            }
        }
        self.saved = Some(match trouble {
            Some(e) => e,
            None => format!("{written} pages saved as {}", named(0).with_file_name(format!("{stem}-N.png")).display()),
        });
    }

    /// The call in progress, if one is.
    fn live(&self) -> Option<&CallRecord> {
        self.calls.last().filter(|c| c.ended.is_none())
    }

    /// Which picture the page panel shows, once Follow is decided.
    fn shown_view(&self) -> View {
        match self.view {
            View::Follow => match self.live() {
                Some(call) if call.sending => View::Sending,
                Some(_) => View::Received,
                None if self.opened.is_some() || self.scan.is_some() => View::Received,
                None if !self.pages.is_empty() => View::Sending,
                None => View::Received,
            },
            view => view,
        }
    }

    /// Put down the call, and say it was this end that did.
    pub fn stopped(&mut self) {
        if let Some(call) = self.calls.last_mut().filter(|c| c.ended.is_none()) {
            call.stopped_here = true;
        }
    }

    /// The far end, where the Scopes tab has V.8's: who it says it is, what
    /// it says it can do, and what the two ends have in common.
    pub fn far_panel(&self, ui: &mut egui::Ui) {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        ui.add_space(6.0);
        ui.label(RichText::new("the fax machine at the far end").strong());
        let Some(caps) = self.far.as_ref() else {
            ui.label(
                RichText::new(
                    "Nothing yet. It says what it is in a DIS, which arrives a few \
                     seconds into the call.",
                )
                .small()
                .color(dim),
            );
            return;
        };
        egui::Grid::new("fax-far")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                    ui.label(RichText::new(k).monospace().size(12.0).color(dim));
                    ui.label(RichText::new(v).monospace().size(12.0).color(bright));
                    ui.end_row();
                };
                if !self.far_identity.is_empty() {
                    row(ui, "identity", &self.far_identity);
                }
                if let Some(fif) = self.far_non_standard.as_deref() {
                    for (k, v) in non_standard_rows(fif) {
                        row(ui, k, &v);
                    }
                }
                // Receiving, the field is the caller's DCS, not a DIS: its rate
                // bits name the one modulation chosen for this call, which a
                // DIS reading turns into "none this knows" and "nothing in
                // common". On a V.34 call T.30 sends them as zero (Table 2,
                // Note 33), and the rate is the modem's, from its MPh
                // exchange. Either way they are said once, as this call.
                for (k, v) in caps.rows() {
                    if !self.sending && matches!(k, "modulations" | "fastest") {
                        continue;
                    }
                    match (k, v.split_once("JBIG")) {
                        // The best coding there is, in its green, where the
                        // far end says it has it.
                        ("coding", Some((before, after))) => {
                            ui.label(RichText::new(k).monospace().size(12.0).color(dim));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(RichText::new(before).monospace().size(12.0).color(bright));
                                ui.label(RichText::new("JBIG").monospace().size(12.0).strong().color(BEST));
                                ui.label(RichText::new(after).monospace().size(12.0).color(bright));
                            });
                            ui.end_row();
                        }
                        _ => row(ui, k, &v),
                    }
                }
                ui.label(
                    RichText::new(if self.sending { "both ends" } else { "this call" })
                        .monospace()
                        .size(12.0)
                        .color(dim),
                );
                let shared = self.shared(caps);
                ui.label(
                    RichText::new(shared.clone().unwrap_or_else(|| "nothing in common".to_owned()))
                        .monospace()
                        .size(12.0)
                        .color(if shared.is_some() { BEST } else { Color32::from_rgb(235, 100, 90) }),
                );
                ui.end_row();
            });
    }

    /// What the two ends have settled on, for the far-end panel: the best
    /// both have when this end is sending and has the far end's DIS, and
    /// what the caller's DCS chose when receiving.
    fn shared(&self, caps: &t30::Capabilities) -> Option<String> {
        if self.symbol_rate > 0 {
            return Some(format!("V.34 at {} bit/s on {} baud", self.rate, self.symbol_rate));
        }
        if self.sending {
            return caps.best_shared(&self.ours()).map(|(m, rate)| format!("{} at {rate} bit/s", m.name()));
        }
        self.far_field
            .as_deref()
            .and_then(t30::command_rate)
            .map(|(m, rate)| format!("{} at {rate} bit/s", m.name()))
            .or_else(|| self.carried.map(|c| format!("{} at {} bit/s", c.modulation, c.rate)))
    }

    /// The page, large: the one going out, or the one arriving drawn a line
    /// at a time, or a page of an earlier call opened from the list.
    pub fn page_panel(&mut self, ui: &mut egui::Ui) {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        let shown = self.shown_view();
        ui.horizontal(|ui| {
            for (view, name) in [(View::Follow, "follow the call"), (View::Sending, "to send"), (View::Received, "received")] {
                if ui.selectable_label(self.view == view, name).clicked() {
                    self.view = view;
                }
            }
            ui.separator();
            if let Some(trouble) = &self.trouble {
                ui.label(RichText::new(trouble).small().color(Color32::from_rgb(235, 100, 90)));
            }
        });
        match shown {
            View::Sending => self.sending_page(ui, dim, bright),
            _ => self.received_page(ui, dim, bright),
        }
    }

    /// The page to send, as big as the panel allows, with what has gone
    /// shaded off while it goes.
    fn sending_page(&mut self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        // Following the call: the page in view is the page going.
        let going = self.live().is_some_and(|c| c.sending) && self.sheet > 0;
        if going && self.sheet - 1 != self.showing && self.sheet <= self.pages.len() {
            self.showing = self.sheet - 1;
            self.preview = None;
        }
        let count = self.pages.len();
        let mut step = 0isize;
        ui.horizontal(|ui| {
            if count > 1 {
                if ui.add_enabled(self.showing > 0, egui::Button::new("<")).clicked() {
                    step = -1;
                }
                ui.label(RichText::new(format!("page {} of {count}", self.showing + 1)).monospace().color(bright));
                if ui.add_enabled(self.showing + 1 < count, egui::Button::new(">")).clicked() {
                    step = 1;
                }
            }
            if let Some(page) = self.pages.get(self.showing) {
                ui.label(
                    RichText::new(format!(
                        "{} · {} by {} pels · {:.1}% ink",
                        page.resolution.name(),
                        page.width(),
                        page.height(),
                        page.coverage() * 100.0
                    ))
                    .monospace()
                    .color(dim),
                );
            }
            if let Some(toned) = self.toned.get(self.showing) {
                ui.label(
                    RichText::new(format!(
                        "{} {:.0} kB where the far end reads it",
                        tone_said(&toned.picture),
                        toned.picture.jpeg.len() as f64 / 1000.0
                    ))
                    .monospace()
                    .color(TONE),
                )
                .on_hover_text(
                    "The page is shown as it goes in JPEG, at 200 pels to 25.4 mm. A far end \
                     that does not read JPEG gets the page of ink instead",
                );
            }
        });
        if step != 0 {
            self.showing = self.showing.saturating_add_signed(step).min(count.saturating_sub(1));
            self.preview = None;
        }
        let ppp = ui.ctx().pixels_per_point();
        let (n, m, level, anchor) = self.zoom.begin(ui, (ui.available_width() - 14.0) * ppp);
        let Some(texture) = self.texture(ui.ctx(), n) else {
            ui.label(
                RichText::new("Nothing to send yet. Browse for a picture or a PDF on the right.")
                    .small()
                    .color(dim),
            );
            return;
        };
        // Whole screen pixels to a texel or whole texels to a pixel, never
        // smoothed: a page bigger than the room scrolls.
        let size = picture_size(ppp, texture.size()[0], texture.size()[1], m);
        let progress = (going && self.sheet - 1 == self.showing)
            .then_some(self.progress)
            .flatten()
            .map(|fraction| {
                // Down to the line the bits have reached, not as far down as
                // the fraction of the bits: a blank stretch is a few bits a
                // line and a dense one hundreds, so the two part company on
                // any page with both.
                match self.line_reached(self.showing, fraction) {
                    Some((line, lines)) => line / lines.max(1) as f64,
                    None => fraction,
                }
            });
        let mut area = egui::ScrollArea::both().id_salt("fax-send-scroll").auto_shrink([false, false]);
        if let Some(offset) = anchor {
            area = area.scroll_offset(offset);
        }
        let out = area.show(ui, |ui| {
                let (area, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width().max(size.x), size.y),
                    egui::Sense::hover(),
                );
                let left = ((area.center().x - size.x / 2.0).max(area.left()) * ppp).round() / ppp;
                let top = (area.top() * ppp).round() / ppp;
                let rect = egui::Rect::from_min_size(egui::pos2(left, top), size);
                ui.painter().image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                // What is still to go, shaded, and a green line where the
                // page has got to.
                if let Some(down) = progress {
                    let y = rect.top() + rect.height() * down.clamp(0.0, 1.0) as f32;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(egui::pos2(rect.left(), y), rect.max),
                        0.0,
                        Color32::from_rgba_unmultiplied(24, 28, 36, 200),
                    );
                    ui.painter().hline(rect.x_range(), y, egui::Stroke::new(2.0, BEST));
                }
            });
        self.zoom.end(ui, out.inner_rect, out.state.offset, level);
    }

    /// The page arriving, drawn as it comes, or the last one that did, or a
    /// page opened from the list.
    fn received_page(&mut self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        let now = ui.input(|i| i.time);
        if let Some((call, at, _)) = &self.opened {
            let (call, at) = (*call, *at);
            let pages = self.calls.get(call).map_or(0, |c| c.received.len());
            let mut step = 0isize;
            let mut close = false;
            ui.horizontal(|ui| {
                let far = self.calls.get(call).map_or("", |c| c.far.as_str());
                ui.label(RichText::new(format!("from an earlier call, {far}")).color(dim));
                if pages > 1 {
                    if ui.add_enabled(at > 0, egui::Button::new("<")).clicked() {
                        step = -1;
                    }
                    ui.label(RichText::new(format!("page {} of {pages}", at + 1)).monospace().color(bright));
                    if ui.add_enabled(at + 1 < pages, egui::Button::new(">")).clicked() {
                        step = 1;
                    }
                }
                if let Some(said) = self.tone_in_view() {
                    badge(ui, said, TONE, TONE_TIP);
                }
                close = ui.button("Back to the call").clicked();
            });
            if close {
                self.opened = None;
                return;
            }
            if step != 0 {
                self.open(call, at.saturating_add_signed(step));
            }
            if let Some((_, _, scan)) = self.opened.as_mut() {
                draw_scan(ui, scan, false, &mut self.zoom);
            }
            return;
        }
        let arriving = self.looking.is_none()
            && self
                .scan
                .as_ref()
                .is_some_and(|scan| self.incoming.is_none() && now - scan.last_line < STILL_ARRIVING);
        let pages = self.earlier.len() + usize::from(self.scan.is_some());
        let at = self.looking.as_ref().map_or(self.earlier.len(), |(i, _)| *i);
        let (mut save, mut save_all, mut step) = (false, false, 0isize);
        ui.horizontal(|ui| {
            let heading = match (self.looking.as_ref().map(|(_, s)| s).or(self.scan.as_ref()), arriving) {
                (None, _) => "page received".to_owned(),
                (Some(scan), true) => format!("page {} arriving: {} lines", scan.sheet.max(1), scan.lines.len()),
                (Some(scan), false) => format!("page {} received: {} lines", scan.sheet.max(1), scan.lines.len()),
            };
            ui.label(RichText::new(heading).color(dim));
            if let Some(said) = self.tone_in_view() {
                badge(ui, said, TONE, TONE_TIP);
            }
            if pages > 1 {
                if ui.add_enabled(at > 0, egui::Button::new("<")).clicked() {
                    step = -1;
                }
                ui.label(RichText::new(format!("{} of {pages}", at + 1)).monospace().color(bright));
                if ui.add_enabled(at + 1 < pages, egui::Button::new(">")).clicked() {
                    step = 1;
                }
            }
            if self.in_view().is_some() {
                save = ui
                    .button("Save")
                    .on_hover_text(
                        "This page, as a PDF or a PNG, whichever the name ends in -- as far as \
                         it has got, if it is still arriving or the call ended part way down it. \
                         A greyscale or colour page saves in its tone, and its PDF holds the \
                         JPEG as it arrived",
                    )
                    .clicked();
            }
            if pages > 1 {
                save_all = ui
                    .button(format!("Save all {pages}"))
                    .on_hover_text(
                        "Every page of the call: in one PDF, a page to a sheet, or each to \
                         its own PNG with the page's number after the name chosen",
                    )
                    .clicked();
            }
            if let Some(saved) = &self.saved {
                ui.label(RichText::new(saved).small().color(dim));
            }
        });
        let scan = match self.looking.as_mut() {
            Some((_, scan)) => Some(scan),
            None => self.scan.as_mut(),
        };
        match scan {
            Some(scan) => draw_scan(ui, scan, arriving, &mut self.zoom),
            None => {
                ui.label(
                    RichText::new(
                        "Nothing yet. Press Wait for a fax, or answer a call as a fax, and a \
                         page that arrives is drawn here a line at a time as it comes in.",
                    )
                    .small()
                    .color(dim),
                );
            }
        }
        if arriving {
            // The green line goes when the page stops, and nothing else may
            // be about to ask for a frame when it does.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
        if step != 0 {
            self.look(step);
        }
        if save {
            let default_name = match self.in_view() {
                Some((sheet, _, _)) if pages > 1 => format!("fax-{sheet}.pdf"),
                _ => "fax.pdf".to_owned(),
            };
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            let picked = rfd::FileDialog::new()
                .add_filter("PDF", &["pdf"])
                .add_filter("PNG picture", &["png"])
                .set_file_name(&default_name)
                .save_file();
            #[cfg(all(target_os = "windows", target_arch = "x86"))]
            let picked = crate::xp_compat::pick_save_file(
                "Save fax",
                Some(&default_name),
                &[("PDF", "*.pdf"), ("PNG picture", "*.png")],
                None,
            );
            if let Some(chosen) = picked {
                self.save(&chosen);
            }
        }
        if save_all {
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            let picked = rfd::FileDialog::new()
                .add_filter("PDF, every page in one", &["pdf"])
                .add_filter("PNG pictures, a page each", &["png"])
                .set_file_name("fax.pdf")
                .save_file();
            #[cfg(all(target_os = "windows", target_arch = "x86"))]
            let picked = crate::xp_compat::pick_save_file(
                "Save all fax pages",
                Some("fax.pdf"),
                &[
                    ("PDF, every page in one", "*.pdf"),
                    ("PNG pictures, a page each", "*.png"),
                ],
                None,
            );
            if let Some(chosen) = picked {
                self.save_all(&chosen);
            }
        }
    }

    /// Open page `at` of an earlier call's received pages.
    fn open(&mut self, call: usize, at: usize) {
        let Some(record) = self.calls.get(call) else {
            return;
        };
        let Some((sheet, page)) = record.received.get(at) else {
            return;
        };
        let picture = record.pictures.iter().find(|(s, _)| s == sheet).map(|(_, p)| p.as_ref());
        self.opened = Some((call, at, Scan::of_tone(*sheet, page, picture)));
        self.view = View::Received;
    }

    /// Everything to the right of the page: the number and the keys, how far
    /// the call has got, how the last one went, the document, the rates and
    /// the calls so far. Returns what was asked of the modem.
    pub fn side_panel(&mut self, ui: &mut egui::Ui, on_hook: bool, number: &mut String) -> Option<Start> {
        let dim = Color32::from_rgb(140, 150, 165);
        let bright = Color32::from_rgb(220, 225, 235);
        let now = ui.input(|i| i.time);
        let mut start = None;

        egui::Grid::new("fax-dial").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("fax number").color(dim)).on_hover_text(
                "The dialler's number: the same one the Dial keypad edits, and what Send fax dials",
            );
            ui.add(egui::TextEdit::singleline(number).desired_width(180.0).hint_text("from the dialler"));
            ui.end_row();
            ui.label(RichText::new("this end (TSI/CSI)").color(dim)).on_hover_text(
                "What this end calls itself: sent as a TSI when sending and a CSI when \
                 receiving, and shown at the far end. Digits, spaces and a plus; blank is \
                 allowed, and plenty of machines send nothing",
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.identification)
                    .desired_width(180.0)
                    .hint_text("e.g. +61 3 9999 0000"),
            );
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.add_enabled_ui(on_hook, |ui| {
                if ui
                    .button("Send fax")
                    .on_hover_text(
                        "AT+FCLASS=1 and then ATD to the number above. The calling tone goes \
                         out, whatever answers says what it is, and then the pages follow",
                    )
                    .clicked()
                {
                    start = Some(Start::Dial(number.trim().to_owned()));
                }
                if ui
                    .button("Wait for a fax")
                    .on_hover_text(
                        "AT+FCLASS=1 and then ATA. This end answers with the 2100 Hz tone, \
                         says what it can receive, and keeps whatever arrives",
                    )
                    .clicked()
                {
                    start = Some(Start::Answer);
                }
            });
            if ui
                .add_enabled(!on_hook, egui::Button::new("Stop"))
                .on_hover_text(
                    "Put the call down now. A fax has no terminal to type ATH at, so this is \
                     the way out of one that has stalled",
                )
                .clicked()
            {
                start = Some(Start::HangUp);
            }
        });

        ui.separator();
        if on_hook {
            if self.fax_class {
                ui.label(
                    RichText::new("The modem is in fax class. The Modem key at the top right makes it a modem again.")
                        .small()
                        .color(dim),
                );
            }
        } else {
            self.call_progress(ui, now, dim, bright);
        }
        if let Some(call) = self.calls.last().filter(|c| c.ended.is_some()) {
            let (headline, colour) = call.headline();
            egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.0, colour)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("last call").small().color(dim));
                ui.label(RichText::new(headline).strong().color(colour));
                ui.label(RichText::new(call.details(now)).monospace().size(12.0).color(bright));
                if !call.far.is_empty() {
                    ui.label(RichText::new(format!("far end {}", call.far)).monospace().size(12.0).color(dim));
                }
            });
        }

        ui.separator();
        self.document(ui, dim, bright);

        ui.separator();
        self.rate_table(ui, dim, bright);

        ui.separator();
        self.session_list(ui, now, dim, bright);
        start
    }

    /// How far through the call: the phase, the page, the bits or lines,
    /// and the time left at the agreed rate.
    fn call_progress(&mut self, ui: &mut egui::Ui, now: f64, dim: Color32, bright: Color32) {
        let what = self.phase.unwrap_or("a call, but not a fax");
        let phase = self.phase.and_then(fax::call::Phase::named).map(fax::call::Phase::letter);
        let side = if self.sending { "sending" } else { "receiving" };
        ui.label(
            RichText::new(match phase {
                Some(p) => format!("{side} · phase {p}: {what}"),
                None => format!("{side}: {what}"),
            })
            .strong()
            .color(bright),
        );
        if let Some(c) = self.live().and_then(|call| call.carried) {
            ui.horizontal_wrapped(|ui| {
                badge(
                    ui,
                    c.coding,
                    coding_colour(c.coding),
                    "The page's coding, best first: JBIG (T.85), MMR (T.6), MR and MH (T.4). \
                     JBIG and MMR go only with error correction, and only when both ends have them",
                );
                let carrier = if c.symbol_rate > 0 {
                    format!("{} {} on {} Bd", c.modulation, c.rate, c.symbol_rate)
                } else {
                    format!("{} {}", c.modulation, c.rate)
                };
                badge(
                    ui,
                    &carrier,
                    modulation_colour(c.modulation),
                    "What carries the page, fastest first: V.34 half-duplex (Super G3) to \
                     33 600, V.17 to 14 400, V.29 to 9600, V.27 ter to 4800",
                );
                badge(
                    ui,
                    if c.correcting { "ECM" } else { "no ECM" },
                    if c.correcting { BEST } else { POOR },
                    "Error correction mode: frames the far end could not read are asked for \
                     and sent again, rather than printed as streaks",
                );
            });
        }
        let Some(fraction) = self.progress else { return };
        let mut said = Vec::new();
        if self.sending {
            let jpeg = self.live().and_then(|c| c.carried).map_or(self.coding, |c| c.coding) == "JPEG";
            let bits: Vec<usize> = match self.coding_now() {
                Some(i) => self.coded_pages.iter().map(|p| p[i]).collect(),
                None if jpeg => self.tone_bits(),
                None => Vec::new(),
            };
            let pages = self.sheets.max(self.sheet).max(1);
            said.push(format!("page {} of {pages}", self.sheet.max(1)));
            if let Some(p) = send_progress(&bits, self.sheet, fraction, self.rate) {
                said.push(format!("{:.1} of {:.1} kB", p.done_bits as f64 / 8000.0, p.page_bits as f64 / 8000.0));
                if let Some((line, lines)) = self.line_reached(self.sheet - 1, fraction) {
                    said.push(format!("line {} of {lines}", line.floor() as usize));
                }
                said.push(format!("{} left on this page", clock(p.page_left)));
                if p.rest_left > 0.0 {
                    said.push(format!("about {} for the rest", clock(p.rest_left)));
                }
            } else {
                said.push(format!("{:.0}%", fraction * 100.0));
            }
        } else {
            said.push(format!("page {}", self.sheet.max(1)));
            // How fast lines are arriving, measured over the page so far.
            match self.pace {
                Some((_, l0, _)) if self.lines < l0 => self.pace = Some((now, self.lines, 0.0)),
                Some((t0, l0, _)) if now - t0 > 2.0 => {
                    let per_second = (self.lines - l0) as f64 / (now - t0);
                    self.pace = Some((t0, l0, per_second));
                }
                None => self.pace = Some((now, self.lines, 0.0)),
                _ => {}
            }
            let expected = self.scan.as_ref().map_or(Resolution::Standard, |s| s.resolution).lines();
            said.push(format!("{} of about {expected} lines", self.lines));
            if let Some((_, _, per_second)) = self.pace
                && per_second > 0.0
                && self.lines < expected
            {
                said.push(format!("about {} left", clock((expected - self.lines) as f64 / per_second)));
            }
        }
        ui.add(egui::ProgressBar::new(fraction as f32).desired_height(16.0).show_percentage());
        ui.label(RichText::new(said.join(" · ")).monospace().size(12.0).color(dim));
    }

    /// How far down page `page` the call has got once `fraction` of its bits
    /// have gone, in lines, and how many lines it has. None before the
    /// coding is known.
    fn line_reached(&mut self, page: usize, fraction: f64) -> Option<(f64, usize)> {
        let coding = self.live().and_then(|c| c.carried).map_or(self.coding, |c| c.coding);
        if coding == "JPEG" {
            // The encoder said where each line ends when it made the stream.
            let ends = &self.toned.get(page)?.ends;
            let sent = (fraction.clamp(0.0, 1.0) * *ends.last()? as f64).round() as usize;
            return Some((fax::coding::line_at(ends, sent), ends.len()));
        }
        let which = match coding {
            "MH" => Coding::ModifiedHuffman,
            "MR" => Coding::ModifiedRead,
            "MMR" => Coding::Mmr,
            "JBIG" => Coding::Jbig,
            _ => return None,
        };
        let lines = self.pages.get(page)?.height();
        let slot = self.line_ends.get_mut(page)?;
        let current = match slot.as_ref() {
            Some(LineMap::Working(c, _) | LineMap::Ready(c, _, _)) => *c == coding,
            None => false,
        };
        if !current {
            let p = self.pages[page].clone();
            let (tell, told) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let bits = which.encode(&p.lines, p.resolution, 0);
                let _ = tell.send((fax::coding::line_ends(which, &bits), bits.len()));
            });
            *slot = Some(LineMap::Working(coding, told));
        }
        if let Some(LineMap::Working(c, told)) = slot.as_ref()
            && let Ok((ends, total)) = told.try_recv()
        {
            *slot = Some(LineMap::Ready(c, ends, total));
        }
        let Some(LineMap::Ready(_, ends, total)) = slot.as_ref() else { return None };
        let sent = (fraction.clamp(0.0, 1.0) * *total as f64).round() as usize;
        Some((fax::coding::line_at(ends, sent), lines))
    }

    /// Which of the four codings the call is sending in.
    fn coding_now(&self) -> Option<usize> {
        coding_index(self.live().and_then(|c| c.carried).map_or(self.coding, |c| c.coding))
    }

    /// The document to send and how it is made into pages, folded away
    /// during a call.
    fn document(&mut self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        let heading = match self.pages.len() {
            0 => "document: none".to_owned(),
            1 => "document: 1 page".to_owned(),
            n => format!("document: {n} pages"),
        };
        egui::CollapsingHeader::new(RichText::new(heading).color(bright))
            .id_salt("fax-document")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("Browse")
                        .on_hover_text(
                            "A picture, sent as one page, or a PDF, sent a page to a sheet at \
                             the size it is printed -- up to 50 of its pages",
                        )
                        .clicked()
                    {
                        #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
                        let picked = rfd::FileDialog::new()
                            .add_filter("pictures and PDFs", &["png", "jpg", "jpeg", "bmp", "gif", "pdf"])
                            .add_filter("PDFs", &["pdf"])
                            .add_filter("pictures", &["png", "jpg", "jpeg", "bmp", "gif"])
                            .pick_file();
                        #[cfg(all(target_os = "windows", target_arch = "x86"))]
                        let picked = crate::xp_compat::pick_open_file(
                            "Choose picture or PDF",
                            &[
                                ("pictures and PDFs", "*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.pdf"),
                                ("PDFs", "*.pdf"),
                                ("pictures", "*.png;*.jpg;*.jpeg;*.bmp;*.gif"),
                            ],
                            None,
                        );
                        if let Some(chosen) = picked {
                            self.load(&chosen);
                            self.view = View::Sending;
                        }
                    }
                    let shown = std::path::Path::new(&self.path)
                        .file_name()
                        .map_or_else(|| "no picture or PDF".to_owned(), |n| n.to_string_lossy().into_owned());
                    ui.add(egui::Label::new(RichText::new(shown).monospace().color(dim)).truncate())
                        .on_hover_text(&self.path);
                });
                let mut again = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("paper").monospace().color(dim));
                    for r in [Resolution::Standard, Resolution::Fine] {
                        if ui.selectable_label(self.resolution == r, r.name()).clicked() {
                            self.resolution = r;
                            again = true;
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("tone ").monospace().color(dim));
                    for tone in [SendTone::Ink, SendTone::Greyscale, SendTone::Colour] {
                        if ui
                            .selectable_label(self.tone == tone, tone.name())
                            .on_hover_text(
                                "What the pages go as. Greyscale and colour go in JPEG (T.30 \
                                 Annex E), under ECM, and only to a far end that says it reads \
                                 them; any other gets the pages of ink",
                            )
                            .clicked()
                        {
                            self.tone = tone;
                            again = true;
                        }
                    }
                });
                if self.tone != SendTone::Ink {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("quality").monospace().color(dim));
                        for quality in [Quality::Small, Quality::Normal, Quality::Best] {
                            if ui
                                .selectable_label(self.quality == quality, quality.name())
                                .on_hover_text("How hard the JPEG is squeezed: a smaller page goes sooner")
                                .clicked()
                            {
                                self.quality = quality;
                                again = true;
                            }
                        }
                        let bits: usize = self.tone_bits().iter().sum();
                        // At the fastest rate this end offers.
                        let fastest = rate_rows(self.v34, &self.ours(), 0, 0, 0).first().map(|row| row.rate);
                        if let Some(rate) = fastest.filter(|_| bits > 0) {
                            ui.label(
                                RichText::new(format!(
                                    "JPEG {}k · {} at {rate}",
                                    bits / 1000,
                                    clock(bits as f64 / f64::from(rate))
                                ))
                                .monospace()
                                .color(TONE),
                            );
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("ink  ").monospace().color(dim));
                    for h in [Halftone::Threshold, Halftone::Diffuse] {
                        if ui.selectable_label(self.halftone == h, h.name()).clicked() {
                            self.halftone = h;
                            again = true;
                        }
                    }
                    if self.tone != SendTone::Ink {
                        ui.label(RichText::new("if the far end has no JPEG").small().color(dim));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("offer").monospace().color(dim));
                    ui.checkbox(&mut self.v27ter, "V.27ter").on_hover_text(
                        "2400 and 4800, and the one every fax has. Left unticked with nothing \
                         else ticked, it is offered anyway",
                    );
                    ui.checkbox(&mut self.v29, "V.29").on_hover_text(
                        "7200 and 9600. Untick it to hold a call to V.27ter, which is slower \
                         and more forgiving of a bad line",
                    );
                    ui.checkbox(&mut self.v17, "V.17").on_hover_text(
                        "7200 to 14 400, trellis coded. Untick it to hold a call to V.29 and V.27ter",
                    );
                    ui.checkbox(&mut self.v34, "V.34").on_hover_text(
                        "Super G3: V.8 in front of the call, and the page over V.34's \
                         half-duplex mode at up to 33 600. Used only when the far end has it; \
                         answering, it means ANSam in place of the called tone",
                    );
                    ui.checkbox(&mut self.error_correction, "ECM").on_hover_text(
                        "Error correction mode, T.30 Annex A: the page goes in numbered frames, \
                         and any the far end cannot read are sent again instead of printed as \
                         streaks. Used only when the far end offers it too",
                    );
                    ui.add_enabled(self.error_correction, egui::Checkbox::new(&mut self.jbig, "JBIG"))
                        .on_hover_text(
                            "T.85's JBIG: a good deal smaller than MMR on a picture. Only with \
                             ECM, and only when the far end offers it too",
                        );
                    ui.add_enabled(self.error_correction, egui::Checkbox::new(&mut self.colour, "Colour"))
                        .on_hover_text(
                            "Greyscale and colour pages, as JPEG (T.30 Annex E): answering, this \
                             end says it reads them, and a machine with a colour scanner may \
                             send one. Sending, pages with a tone chosen above go in it where \
                             the far end reads it. Only with ECM",
                        );
                });
                if again {
                    self.render();
                }
                if !self.pages.is_empty() {
                    let [mh, mr, mmr, jbig] = self.coded_bits;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("coded").monospace().color(dim));
                        for (name, bits) in [("MH", mh), ("MR", mr), ("MMR", mmr), ("JBIG", jbig)] {
                            ui.label(
                                RichText::new(format!("{name} {}k", bits / 1000))
                                    .monospace()
                                    .color(coding_colour(name)),
                            );
                        }
                    });
                }
            });
    }

    /// How long the document takes at every rate this end offers, fastest
    /// first, with the rate the call agreed on lit once there is one.
    fn rate_table(&self, ui: &mut egui::Ui, dim: Color32, bright: Color32) {
        if self.pages.is_empty() {
            return;
        }
        let [mh, mr, mmr, jbig] = self.coded_bits;
        let packed = if self.jbig { jbig.min(mmr) } else { mmr };
        let best = if self.error_correction { packed } else { mr };
        let carried = self.live().and_then(|c| c.carried).or(self.carried);
        ui.label(RichText::new("how long the document takes").color(bright)).on_hover_text(
            "From the smallest coding this end will send at each rate -- JBIG or MMR under \
             error correction, MR without -- to MH, which every machine reads. Fastest first",
        );
        egui::Grid::new("fax-rates").num_columns(3).spacing([10.0, 2.0]).show(ui, |ui| {
            for row in rate_rows(self.v34, &self.ours(), packed, best, mh) {
                let lit = agreed(&row, carried);
                let rate = if lit && row.modulation == "V.34" {
                    carried.map_or(row.rate, |c| c.rate)
                } else {
                    row.rate
                };
                let text = |s: String, colour: Color32| {
                    let t = RichText::new(s).monospace().size(12.0).color(colour);
                    if lit { t.strong().background_color(Color32::from_rgb(30, 70, 45)) } else { t }
                };
                ui.label(text(format!("{rate}"), if lit { BEST } else { dim }));
                ui.label(text(row.modulation.to_owned(), modulation_colour(row.modulation)));
                ui.label(text(
                    format!(
                        "{} to {}{}",
                        clock(row.least as f64 / f64::from(rate)),
                        clock(row.most as f64 / f64::from(rate)),
                        if lit { "  agreed" } else { "" }
                    ),
                    if lit { BEST } else { bright },
                ));
                ui.end_row();
            }
        });
    }

    /// The calls since the window opened, newest first.
    fn session_list(&mut self, ui: &mut egui::Ui, now: f64, dim: Color32, bright: Color32) {
        ui.label(RichText::new("calls this session").color(bright));
        if self.calls.is_empty() {
            ui.label(RichText::new("None yet.").small().color(dim));
            return;
        }
        let (mut open, mut save) = (None, None);
        egui::Grid::new("fax-calls").num_columns(5).spacing([8.0, 3.0]).striped(true).show(ui, |ui| {
            for (i, call) in self.calls.iter().enumerate().rev().take(30) {
                ui.label(RichText::new(ago(now - call.ended.unwrap_or(now))).size(12.0).color(dim));
                let far = if call.far.is_empty() { "?" } else { call.far.as_str() };
                ui.label(
                    RichText::new(format!("{} {far}", if call.sending { "to" } else { "from" }))
                        .monospace()
                        .size(12.0)
                        .color(bright),
                )
                .on_hover_text(call.details(now));
                let (short, colour) = call.short();
                ui.label(RichText::new(short).monospace().size(12.0).strong().color(colour))
                    .on_hover_text(call.headline().0);
                let pages = if call.sending { call.confirmed_pages() } else { call.received.len() };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{pages} p")).monospace().size(12.0).color(dim));
                    // A page with tone among them: the same badge as on the
                    // page, small.
                    let colour = call.pictures.iter().any(|(_, p)| p.is_colour());
                    if !call.pictures.is_empty() {
                        ui.label(
                            RichText::new(if colour { " colour " } else { " grey " })
                                .monospace()
                                .size(11.0)
                                .color(BADGE_INK)
                                .background_color(TONE),
                        )
                        .on_hover_text(TONE_TIP);
                    }
                });
                ui.horizontal(|ui| {
                    if !call.received.is_empty() {
                        if ui.small_button("open").clicked() {
                            open = Some(i);
                        }
                        if ui.small_button("save").on_hover_text("As one PDF, or a PNG a page").clicked() {
                            save = Some(i);
                        }
                    }
                });
                ui.end_row();
            }
        });
        if let Some(i) = open {
            self.open(i, 0);
        }
        if let Some(i) = save {
            #[cfg(not(all(target_os = "windows", target_arch = "x86")))]
            let picked = rfd::FileDialog::new()
                .add_filter("PDF, every page in one", &["pdf"])
                .add_filter("PNG pictures, a page each", &["png"])
                .set_file_name("fax.pdf")
                .save_file();
            #[cfg(all(target_os = "windows", target_arch = "x86"))]
            let picked = crate::xp_compat::pick_save_file(
                "Save received fax",
                Some("fax.pdf"),
                &[
                    ("PDF, every page in one", "*.pdf"),
                    ("PNG pictures, a page each", "*.png"),
                ],
                None,
            );
            if let Some(chosen) = picked
                && let Some(call) = self.calls.get(i)
            {
                let pages: Vec<Kept<'_>> = call
                    .received
                    .iter()
                    .map(|(sheet, page)| {
                        let picture = call.pictures.iter().find(|(s, _)| s == sheet).map(|(_, p)| p.as_ref());
                        (*sheet, page.lines.as_slice(), page.resolution, picture)
                    })
                    .collect();
                self.saved = Some(save_pages(&pages, &chosen));
            }
        }
        if let Some(saved) = &self.saved {
            ui.label(RichText::new(saved).small().color(dim));
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

/// A page arriving, or arrived, drawn the whole width of the panel and
/// scrolling once it is taller than the room, following the newest line
/// while the view is at the bottom.
fn draw_scan(ui: &mut egui::Ui, scan: &mut Scan, arriving: bool, zoom: &mut Zoom) {
    // Whole screen pixels to a texel or whole texels to a pixel, never
    // smoothed: fitted across the panel until the wheel says otherwise, and
    // scrolling, following the newest line down.
    let ppp = ui.ctx().pixels_per_point();
    let room = ui.available_size();
    let (n, m, level, anchor) = zoom.begin(ui, (room.x - 14.0) * ppp);
    scan.set_scale(n);
    let Some(texture) = scan.texture(ui.ctx()) else { return };
    let shown = scan.shown();
    let bottom = shown as f32 / scan.capacity as f32;
    let sheet = scan.sheet_rows();
    let width = scan.width();
    let mut area = egui::ScrollArea::both()
        .id_salt("fax-receive-scroll")
        .max_height(room.y)
        .auto_shrink([false, false])
        .stick_to_bottom(true);
    if let Some(offset) = anchor {
        area = area.scroll_offset(offset);
    }
    let out = area.show(ui, |ui| {
        // Painted into a rectangle of its own rather than through an image
        // widget: the texture is a whole sheet tall, and an image widget
        // keeps its proportions.
        let full = picture_size(ppp, width, shown.max(sheet), m);
        let (area, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(full.x), full.y),
            egui::Sense::hover(),
        );
        // On the pixel grid, so each texel lands on whole pixels.
        let left = ((area.center().x - full.x / 2.0).max(area.left()) * ppp).round() / ppp;
        let top = (area.top() * ppp).round() / ppp;
        let paper = egui::Rect::from_min_size(egui::pos2(left, top), full);
        // The rest of the sheet, still to come.
        ui.painter().rect_filled(paper, 0.0, UNSCANNED);
        let rect = egui::Rect::from_min_size(paper.min, picture_size(ppp, width, shown, m));
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, bottom)),
            Color32::WHITE,
        );
        if arriving {
            ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, egui::Stroke::new(2.0, BEST));
        }
    });
    zoom.end(ui, out.inner_rect, out.state.offset, level);
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
        // A whole A4 sheet at either resolution is the same picture, near
        // enough 297 mm down and 215 across -- not a standard page half the
        // height of the same page sent fine. Near enough: drawn a whole
        // pixel to a pel, 7.7 lines to the millimetre come out as tall as
        // 8.04 pels across, 4% short.
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let mut scan = Scan::new(0, 1, resolution);
            scan.extend(0, lines_of(resolution.lines(), false));
            let size = picture_size(1.0, scan.width(), scan.rows, 1);
            let tall = (size.y / size.x) as f64;
            assert!(
                (tall - 297.0 / 215.0).abs() < 0.07,
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
            tone: None,
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
            tone: None,
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
                tone: None,
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
        // Shrunk by three, so a sheet fits the test's small texture limit.
        let mut scan = Scan::new(0, 1, Resolution::Standard);
        scan.set_scale(3);
        assert!(scan.texture(&ctx).is_none(), "a texture for nothing");
        scan.extend(0, lines_of(100, false));
        let texture = scan.texture(&ctx).expect("no texture");
        assert_eq!(texture.size()[0], SCAN_WIDTH / 3);
        assert!(texture.size()[1] >= 762, "{} rows is not a sheet", texture.size()[1]);
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
                    tone: None,
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
                fax.received_page(ui, Color32::GRAY, Color32::WHITE);
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
        // 1728 pels do not fit 640 points, so a third: 576 across.
        assert!((widths[0] - 576.0).abs() < 0.5, "the first rows are {} across", widths[0]);
        assert!(
            widths.iter().all(|w| (w - widths[0]).abs() < 0.5),
            "the page changed width as it arrived: {widths:?}"
        );
        assert!(heights.windows(2).all(|h| h[0] < h[1]), "not growing downwards: {heights:?}");
        // And whole pixels at the end: 1143 standard lines, two pixels
        // each, shrunk by three.
        assert!((heights[3] - 762.0).abs() < 0.5, "a sheet {} tall", heights[3]);
    }

    #[test]
    fn a_thumbnail_has_as_many_pixels_as_it_says_it_has() {
        // The image and the size it declares are handed over separately, and
        // the two disagreeing is a panic inside the drawing rather than a
        // wrong picture. A page that arrives is whatever length the far end
        // sent, including lengths nothing here would ever produce.
        for lines in [0, 1, 5, 6, 7, 11, 191, 1143, 2287] {
            let image = Fax::thumbnail(&page_of(lines), 16384, 3);
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
                tone: None,
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
            tone: None,
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
        fax.observe(&frame, 0.0);
        assert_eq!(fax.carried, None, "taken before a page was moving");

        frame.fax_coding = "JBIG";
        frame.fax_progress = Some(0.25);
        fax.observe(&frame, 0.0);
        let page = Carried {
            coding: "JBIG",
            modulation: "V.34",
            rate: 33_600,
            symbol_rate: 3429,
            correcting: true,
        };
        assert_eq!(fax.carried, Some(page));

        frame.fax_progress = None;
        fax.observe(&frame, 0.0);
        assert_eq!(fax.carried, Some(page), "lost in the turnaround after the page");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("binmodem-faxwin-pdf");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// What a stream decodes to.
    fn decode(stream: &[u8]) -> Picture {
        let mut decoder = fax::jpeg::Decoder::new(fax::page::WIDTH);
        decoder.feed(stream);
        assert!(decoder.is_done() && decoder.damaged() == 0);
        decoder.picture().expect("the stream does not decode")
    }

    #[test]
    fn a_picture_goes_in_the_tone_asked_for_and_as_ink_as_well() {
        // Red on the left and blue on the right, twice as wide as it is tall.
        let path = scratch("two-colours.png");
        image::RgbImage::from_fn(200, 100, |x, _| image::Rgb(if x < 100 { [200, 60, 50] } else { [50, 80, 160] }))
            .save(&path)
            .unwrap();
        let mut fax = Fax::new();
        fax.load(&path);
        assert_eq!(fax.pages().len(), 1);
        // Ink unless asked: nothing with tone goes to the call.
        let (pictures, lightness) = fax.pictures();
        assert!(pictures.is_empty() && lightness.is_empty());

        fax.tone = SendTone::Colour;
        fax.render();
        let (pictures, lightness) = fax.pictures();
        assert_eq!((pictures.len(), lightness.len()), (1, 1));
        let sent = pictures[0].as_ref().unwrap();
        // The width of the paper, and as long as the picture is on it at
        // 7.7 lines to the millimetre: half of 215 mm.
        assert_eq!((sent.width, sent.components, sent.pels_per_inch), (fax::page::WIDTH, 3, 200));
        assert!(sent.height.abs_diff(828) <= 2, "{} lines", sent.height);
        assert!(sent.samples.is_empty(), "the call was given the samples as well as the stream");
        let back = decode(&sent.jpeg);
        assert_eq!((back.width, back.height, back.components), (sent.width, sent.height, 3));
        let rgb = back.rgb();
        let at = |x: usize, y: usize| [rgb[(y * back.width + x) * 3], rgb[(y * back.width + x) * 3 + 1], rgb[(y * back.width + x) * 3 + 2]];
        for (x, want) in [(300, [200, 60, 50]), (1400, [50, 80, 160])] {
            let got = at(x, 400);
            assert!((0..3).all(|c| got[c].abs_diff(want[c]) <= 12), "at {x}: {got:?}");
        }
        // The same page as lightness alone, for a far end without colour.
        let alone = lightness[0].as_ref().unwrap();
        assert_eq!((alone.components, alone.height), (1, sent.height));
        assert_eq!(decode(&alone.jpeg).components, 1);
        // And the page of ink is still there, for a far end with neither.
        assert_eq!(fax.pages().len(), 1);

        // Greyscale: lightness is the page, and there is nothing beside it.
        fax.tone = SendTone::Greyscale;
        fax.render();
        let (pictures, lightness) = fax.pictures();
        assert_eq!(pictures[0].as_ref().map(|p| p.components), Some(1));
        assert!(lightness.is_empty());

        // A smaller page for a lower quality.
        let size = |fax: &mut Fax, quality: Quality| {
            fax.quality = quality;
            fax.render();
            fax.tone_bits()[0]
        };
        let (small, normal, best) =
            (size(&mut fax, Quality::Small), size(&mut fax, Quality::Normal), size(&mut fax, Quality::Best));
        assert!(small < normal && normal < best, "{small}, {normal}, {best}");

        // Back to ink, and they are gone.
        fax.tone = SendTone::Ink;
        fax.render();
        assert!(fax.pictures().0.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_document_goes_a_picture_to_a_page_and_the_line_sent_follows_the_stream() {
        // Three pages from the window's own PDF of a received colour call:
        // each is a sheet with a colour picture on it.
        let pictures: Vec<Picture> = [40, 60, 80].into_iter().map(colour_picture).collect();
        let leaves: Vec<fax::pdf::Leaf<'_>> = pictures.iter().map(fax::pdf::Leaf::Tone).collect();
        let path = scratch("colour-three.pdf");
        std::fs::write(&path, fax::pdf::document_of(&leaves)).unwrap();
        let mut fax = Fax::new();
        fax.tone = SendTone::Colour;
        fax.resolution = Resolution::Fine;
        fax.load(&path);
        assert_eq!(fax.trouble, None);
        assert_eq!(fax.pages().len(), 3);
        let (sent, lightness) = fax.pictures();
        assert_eq!((sent.len(), lightness.len()), (3, 3));
        for (n, (picture, page)) in sent.iter().zip(fax.pages()).enumerate() {
            let picture = picture.as_ref().unwrap();
            // As long as its page of ink at fine resolution: the same grid.
            assert_eq!(picture.height, page.height(), "page {}", n + 1);
            assert_eq!(picture.components, 3);
        }
        // The picture in the PDF came through the PDF renderer in its
        // colours: red on the left of the first page.
        let back = decode(&sent[0].as_ref().unwrap().jpeg);
        let rgb = back.rgb();
        let pel = &rgb[(20 * back.width + 100) * 3..(20 * back.width + 100) * 3 + 3];
        assert!(pel[0].abs_diff(200) <= 16 && pel[1].abs_diff(60) <= 16 && pel[2].abs_diff(50) <= 16, "{pel:?}");

        // Sending in JPEG, the line reached comes from the stream's own
        // line ends, with no waiting, and runs from the top to the bottom.
        fax.coding = "JPEG";
        let lines = sent[1].as_ref().unwrap().height;
        assert_eq!(fax.line_reached(1, 0.0).map(|(line, of)| (line < 1.0, of)), Some((true, lines)));
        let (middle, _) = fax.line_reached(1, 0.5).unwrap();
        assert!(middle > 0.0 && middle < lines as f64);
        assert_eq!(fax.line_reached(1, 1.0), Some((lines as f64, lines)));
        assert_eq!(fax.tone_bits().len(), 3);
        let _ = std::fs::remove_file(&path);
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

    /// The frames of the first V.17 call to a real machine, read back out of
    /// its recording (dist/captures/live-1795341989.wav) with the replay
    /// test: the far end's CSI, DIS and CFR, this end's TSI, DCS and EOP,
    /// the far end's MCF at 52.2 s, and this end's DCN.
    fn telstra() -> Vec<Exchanged> {
        let far = |frame| Exchanged { frame, ours: false };
        let near = |frame| Exchanged { frame, ours: true };
        vec![
            far(Frame::Csi),
            far(Frame::Dis),
            near(Frame::Tsi),
            near(Frame::Dcs),
            far(Frame::Cfr),
            near(Frame::Eop),
            far(Frame::Mcf),
            near(Frame::Dcn),
        ]
    }

    fn v17() -> Carried {
        Carried { coding: "MH", modulation: "V.17", rate: 14_400, symbol_rate: 0, correcting: false }
    }

    fn sent(frames: Vec<Exchanged>) -> CallRecord {
        let mut call = CallRecord::new(true, 10.0, "1300 368 909");
        call.frames = frames;
        call.sheets = 1;
        call.carried = Some(v17());
        call.last_phase = Some(fax::call::Phase::AwaitingReceipt.name());
        call.ended = Some(56.0);
        call
    }

    #[test]
    fn the_first_v17_call_to_a_real_machine_was_confirmed() {
        let call = sent(telstra());
        assert_eq!(call.verdict(), Verdict::Confirmed);
        assert_eq!(call.confirmed_pages(), 1);
        let (headline, colour) = call.headline();
        assert_eq!(headline, "Confirmed: the far end took 1 page (MCF)");
        assert_eq!(colour, BEST);
        assert_eq!(call.details(0.0), "1 sent · 0 received · V.17 14400 · MH · no ECM · 0:46");
        assert_eq!(call.short().0, "MCF");
    }

    #[test]
    fn a_call_still_going_has_no_verdict() {
        let mut call = sent(telstra());
        call.ended = None;
        assert_eq!(call.verdict(), Verdict::Going);
    }

    #[test]
    fn a_page_answered_with_rtn_or_rtp_asks_for_a_retrain() {
        for (answer, word) in [(Frame::Rtn, "RTN"), (Frame::Rtp, "RTP")] {
            let mut frames = telstra();
            frames[6].frame = answer;
            let call = sent(frames);
            assert_eq!(call.verdict(), Verdict::RetrainAsked(answer));
            assert!(call.headline().0.contains(word), "{}", call.headline().0);
            assert_eq!(call.confirmed_pages(), usize::from(answer == Frame::Rtp));
        }
    }

    #[test]
    fn a_call_that_ends_before_the_page_says_which_phase_and_why() {
        let mut call = sent(telstra()[..4].to_vec());
        call.last_phase = Some(fax::call::Phase::AwaitingConfirm.name());
        call.trouble = Some("no answer to the DCS".to_owned());
        assert_eq!(call.verdict(), Verdict::Failed { phase: 'B', why: "no answer to the DCS".to_owned() });
        assert!(call.headline().0.starts_with("Disconnected in phase B"), "{}", call.headline().0);
        assert_eq!(call.short().0, "failed B");

        let mut stopped = sent(telstra()[..6].to_vec());
        stopped.last_phase = Some(fax::call::Phase::AwaitingReceipt.name());
        stopped.stopped_here = true;
        assert_eq!(stopped.verdict(), Verdict::Failed { phase: 'D', why: "stopped at this end".to_owned() });

        let mut nobody = sent(Vec::new());
        nobody.last_phase = Some(fax::call::Phase::Calling.name());
        assert_eq!(nobody.verdict(), Verdict::Failed { phase: 'A', why: "no fax machine answered".to_owned() });
    }

    #[test]
    fn a_first_page_confirmed_and_then_a_hang_up_is_not_a_confirmed_call() {
        let near = |frame| Exchanged { frame, ours: true };
        let far = |frame| Exchanged { frame, ours: false };
        let mut frames = telstra()[..5].to_vec();
        frames.extend([near(Frame::Mps), far(Frame::Mcf), far(Frame::Dcn)]);
        let mut call = sent(frames);
        call.trouble = Some("the far end hung up while this end was sending the page".to_owned());
        match call.verdict() {
            Verdict::Failed { why, .. } => assert!(why.ends_with("after page 1 was confirmed"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_command_sent_three_times_and_answered_once_is_one_page() {
        let near = |frame| Exchanged { frame, ours: true };
        let far = |frame| Exchanged { frame, ours: false };
        let mut frames = telstra()[..5].to_vec();
        frames.extend([near(Frame::Eop), near(Frame::Eop), near(Frame::Eop), far(Frame::Mcf), near(Frame::Dcn)]);
        assert_eq!(sent(frames).confirmed_pages(), 1);
    }

    #[test]
    fn a_fax_received_and_confirmed_by_this_end_is_confirmed() {
        let near = |frame| Exchanged { frame, ours: true };
        let far = |frame| Exchanged { frame, ours: false };
        let mut call = CallRecord::new(false, 0.0, "");
        call.frames = vec![
            near(Frame::Csi),
            near(Frame::Dis),
            far(Frame::Dcs),
            near(Frame::Cfr),
            far(Frame::Mps),
            near(Frame::Mcf),
            far(Frame::Eop),
            near(Frame::Mcf),
            far(Frame::Dcn),
        ];
        call.received = vec![(1, page_of(10)), (2, page_of(10))];
        call.sheets = 2;
        call.ended = Some(80.0);
        assert_eq!(call.verdict(), Verdict::Confirmed);
        assert_eq!(call.confirmed_pages(), 2);
        assert_eq!(call.headline().0, "Received 2 pages, confirmed (MCF)");
    }

    #[test]
    fn the_rate_table_goes_fastest_first() {
        let ours = [t30::Modulation::V27ter, t30::Modulation::V29, t30::Modulation::V17];
        let rows = rate_rows(true, &ours, 10, 20, 30);
        let order: Vec<(u32, &str)> = rows.iter().map(|r| (r.rate, r.modulation)).collect();
        assert_eq!(order, vec![
            (33_600, "V.34"),
            (14_400, "V.17"),
            (12_000, "V.17"),
            (9600, "V.17"),
            (9600, "V.29"),
            (7200, "V.17"),
            (7200, "V.29"),
            (4800, "V.27ter"),
            (2400, "V.27ter"),
        ]);
        assert_eq!((rows[0].least, rows[1].least, rows[1].most), (10, 20, 30));
    }

    #[test]
    fn the_rate_the_call_agreed_is_the_one_lit() {
        let ours = [t30::Modulation::V27ter, t30::Modulation::V29, t30::Modulation::V17];
        let rows = rate_rows(true, &ours, 10, 20, 30);
        let lit: Vec<(u32, &str)> =
            rows.iter().filter(|r| agreed(r, Some(v17()))).map(|r| (r.rate, r.modulation)).collect();
        assert_eq!(lit, vec![(14_400, "V.17")]);
        assert!(rows.iter().all(|r| !agreed(r, None)), "lit before anything was agreed");
        // V.34 at whatever rate it settled on, which is seldom 33 600.
        let v34 = Carried { coding: "JBIG", modulation: "V.34", rate: 26_400, symbol_rate: 3200, correcting: true };
        let lit: Vec<&str> = rows.iter().filter(|r| agreed(r, Some(v34))).map(|r| r.modulation).collect();
        assert_eq!(lit, vec!["V.34"]);
        // V.27 ter written with its space still matches.
        let v27 = Carried { coding: "MH", modulation: "V.27 ter", rate: 4800, symbol_rate: 0, correcting: false };
        assert!(rows.iter().any(|r| agreed(r, Some(v27)) && r.rate == 4800));
    }

    #[test]
    fn the_time_left_is_this_page_and_the_ones_after_it_at_the_rate() {
        let bits = [144_000, 72_000, 28_800];
        let p = send_progress(&bits, 2, 0.5, 14_400).unwrap();
        assert_eq!((p.page_bits, p.done_bits), (72_000, 36_000));
        assert!((p.page_left - 2.5).abs() < 1e-9, "{}", p.page_left);
        assert!((p.rest_left - 2.0).abs() < 1e-9, "{}", p.rest_left);
        assert_eq!(send_progress(&bits, 0, 0.5, 14_400), None, "no page yet");
        assert_eq!(send_progress(&bits, 4, 0.5, 14_400), None, "past the last page");
        assert_eq!(send_progress(&bits, 1, 0.5, 0), None, "no rate yet");
    }

    #[test]
    fn a_call_is_a_record_from_its_first_phase_to_on_hook_and_keeps_its_frames() {
        let mut fax = Fax::new();
        fax.dialled = "1300368909".to_owned();
        let mut frame = telemetry::Frame::new(16, 16, 16_000.0);
        fax.observe(&frame, 0.0);
        assert!(fax.calls.is_empty(), "a call out of nothing");

        frame.state = telemetry::CallState::Negotiating;
        frame.fax_phase = Some(fax::call::Phase::Calling.name());
        frame.fax_sending = true;
        fax.observe(&frame, 1.0);
        assert_eq!(fax.calls.len(), 1);
        assert_eq!(fax.calls[0].far, "1300368909", "the number dialled until the far end says");
        frame.fax_identity = "     1300  368 909".to_owned();
        frame.fax_phase = Some(fax::call::Phase::AwaitingReceipt.name());
        frame.fax_sheet = 1;
        fax.observe(&frame, 40.0);
        fax.exchanged(telstra()[..6].to_vec());
        assert_eq!(fax.calls[0].far, "1300  368 909");
        assert_eq!(fax.calls[0].verdict(), Verdict::Going);

        frame.fax_phase = Some(fax::call::Phase::Done.name());
        fax.observe(&frame, 47.0);
        // The last frames arrive just after the end was seen.
        fax.exchanged(telstra()[6..].to_vec());
        assert_eq!(fax.calls[0].ended, Some(47.0));
        assert_eq!(fax.calls[0].verdict(), Verdict::Confirmed);

        // Still "done" and on hook: no second call.
        frame.state = telemetry::CallState::Idle;
        fax.observe(&frame, 48.0);
        assert_eq!(fax.calls.len(), 1);
        // The next one is another record.
        frame.state = telemetry::CallState::Negotiating;
        frame.fax_phase = Some(fax::call::Phase::Answering.name());
        frame.fax_sending = false;
        fax.observe(&frame, 60.0);
        assert_eq!(fax.calls.len(), 2);
        assert!(!fax.calls[1].sending);
    }

    #[test]
    fn a_page_that_arrives_is_kept_with_its_call() {
        let mut fax = Fax::new();
        let mut frame = telemetry::Frame::new(16, 16, 16_000.0);
        frame.state = telemetry::CallState::Negotiating;
        frame.fax_phase = Some(fax::call::Phase::Receiving.name());
        fax.observe(&frame, 0.0);
        fax.arrived(1, page_of(30));
        fax.arrived(2, page_of(40));
        fax.arrived(2, page_of(41));
        let kept: Vec<(usize, usize)> = fax.calls[0].received.iter().map(|(s, p)| (*s, p.height())).collect();
        assert_eq!(kept, vec![(1, 30), (2, 41)], "a page handed over again replaces itself");
        fax.open(0, 1);
        assert_eq!(fax.opened.as_ref().map(|(c, p, s)| (*c, *p, s.lines.len())), Some((0, 1, 41)));
        assert_eq!(fax.shown_view(), View::Received);
    }

    /// A colour picture the width of the paper: bands of colour down a page
    /// of `rows` lines, with the stream it would have arrived in.
    fn colour_picture(rows: usize) -> Picture {
        let width = fax::page::WIDTH;
        let mut samples = Vec::with_capacity(width * rows * 3);
        for _ in 0..rows {
            for x in 0..width {
                let rgb = match x * 3 / width {
                    0 => [200, 60, 50],
                    1 => [250, 250, 245],
                    _ => [50, 80, 160],
                };
                samples.extend_from_slice(&fax::picture::lab_of_rgb(rgb));
            }
        }
        let mut picture =
            Picture { width, height: rows, components: 3, samples, pels_per_inch: 200, ..Picture::default() };
        picture.jpeg = fax::jpeg::encode(&picture, fax::jpeg::Options::default());
        picture
    }

    /// What the line thread makes of a picture: the page of ink that comes
    /// with it, and the picture as the decoder gives it.
    fn decoded(picture: &Picture) -> (Page, Picture) {
        let mut decoder = fax::jpeg::Decoder::new(fax::page::WIDTH);
        decoder.feed(&picture.jpeg);
        let page = Page { lines: decoder.lines().to_vec(), resolution: Resolution::Fine };
        (page, decoder.picture().expect("the stream does not decode"))
    }

    fn close_to(got: Color32, want: [u8; 3]) -> bool {
        got.r().abs_diff(want[0]) <= 12 && got.g().abs_diff(want[1]) <= 12 && got.b().abs_diff(want[2]) <= 12
    }

    #[test]
    fn a_colour_page_is_drawn_in_its_colours_as_it_arrives() {
        let (page, picture) = decoded(&colour_picture(64));
        let part = |from: usize, to: usize| {
            let across = picture.width * 3;
            Picture {
                height: to - from,
                samples: picture.samples[from * across..to * across].to_vec(),
                jpeg: Vec::new(),
                ..picture.clone()
            }
        };
        let batch = |from: usize, to: usize| Arriving {
            page: 7,
            sheet: 1,
            resolution: Resolution::Fine,
            from,
            lines: page.lines[from..to].to_vec(),
            tone: Some(part(from, to)),
        };
        let mut fax = Fax::new();
        assert!(fax.arriving(batch(0, 16), 0.0));
        // Lines that overlap what is there are taken once, as ink's are.
        assert!(!fax.arriving(batch(8, 48), 0.1));
        let scan = fax.scan.as_mut().unwrap();
        scan.set_scale(4);
        assert_eq!(scan.lines.len(), 48);
        assert_eq!(scan.picture().map(|p| p.height), Some(48));
        assert_eq!(scan.rows, 12);
        // Red on the left, paper in the middle, blue on the right.
        let width = scan.width();
        for row in [0, 11] {
            let at = |tx: usize| scan.pixels[row * width + tx];
            assert!(close_to(at(20), [200, 60, 50]), "row {row}: {:?}", at(20));
            assert!(close_to(at(width / 2), [250, 250, 245]), "row {row}: {:?}", at(width / 2));
            assert!(close_to(at(width - 20), [50, 80, 160]), "row {row}: {:?}", at(width - 20));
        }
        assert_eq!(fax.tone_in_view(), Some("JPEG colour"));

        // The page itself, when it comes, is the one already drawn.
        fax.arriving(batch(48, 64), 0.2);
        let drawn = fax.scan.as_ref().unwrap().rows;
        fax.arrived_in_tone(1, page.clone(), Some(Arc::new(picture.clone())));
        assert_eq!(fax.scan.as_ref().unwrap().rows, drawn);
        assert_eq!(fax.scan.as_ref().unwrap().page, 7, "the page was drawn again from scratch");
        assert_eq!(fax.picture_in_view().map(|p| p.jpeg.len()), Some(picture.jpeg.len()));

        // A page of ink after it has no badge, and the colour one is kept.
        fax.arrived(2, page_of(10));
        assert_eq!(fax.tone_in_view(), None);
        fax.look(-1);
        assert_eq!(fax.tone_in_view(), Some("JPEG colour"));
        let (_, looked) = fax.looking.as_ref().unwrap();
        assert!(close_to(looked.pixels[20], [200, 60, 50]), "{:?}", looked.pixels[20]);
    }

    #[test]
    fn a_colour_page_saves_in_colour_and_its_pdf_holds_the_stream_that_arrived() {
        let sent = colour_picture(40);
        let (page, picture) = decoded(&sent);
        let mut fax = Fax::new();
        fax.arrived_in_tone(1, page, Some(Arc::new(picture.clone())));

        let png = scratch("colour.png");
        fax.save(&png);
        assert!(fax.saved.as_deref().is_some_and(|s| s.starts_with("saved to")), "{:?}", fax.saved);
        let image = image::open(&png).unwrap().to_rgb8();
        // A pel to a pixel: 200 pels to 25.4 mm both ways is square.
        assert_eq!((image.width() as usize, image.height() as usize), (fax::page::WIDTH, 40));
        let at = |x: u32| image.get_pixel(x, 20).0;
        assert!(at(100)[0].abs_diff(200) <= 12 && at(100)[2].abs_diff(50) <= 12, "{:?}", at(100));
        assert!(at(1600)[2].abs_diff(160) <= 12 && at(1600)[0].abs_diff(50) <= 12, "{:?}", at(1600));
        let _ = std::fs::remove_file(&png);

        let pdf = scratch("colour.pdf");
        fax.save(&pdf);
        let bytes = std::fs::read(&pdf).unwrap();
        let _ = std::fs::remove_file(&pdf);
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"/Filter /DCTDecode") && has(b"/ColorSpace [/Lab"), "not a JPEG in CIELAB");
        assert!(has(&sent.jpeg), "the stream in the file is not the one that arrived");

        // A greyscale page is a grey picture.
        let mut grey = Picture {
            components: 1,
            samples: sent.samples.iter().step_by(3).copied().collect(),
            jpeg: Vec::new(),
            ..sent
        };
        grey.jpeg = fax::jpeg::encode(&grey, fax::jpeg::Options::default());
        let (page, picture) = decoded(&grey);
        fax.arrived_in_tone(1, page, Some(Arc::new(picture)));
        assert_eq!(fax.tone_in_view(), Some("JPEG grey"));
        fax.save(&png);
        assert_eq!(image::open(&png).unwrap().color(), image::ColorType::L8);
        let _ = std::fs::remove_file(&png);
    }

    #[test]
    fn a_call_keeps_its_colour_pages_and_saves_them_with_its_others() {
        let (page, picture) = decoded(&colour_picture(24));
        let mut fax = Fax::new();
        fax.calls.push(CallRecord::new(false, 0.0, "61399990000"));
        fax.arrived(1, page_of(30));
        fax.arrived_in_tone(2, page, Some(Arc::new(picture.clone())));
        assert_eq!(fax.calls[0].received.len(), 2);
        assert_eq!(fax.calls[0].pictures.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [2]);

        // Both pages in one PDF: the first in T.6 and the second as JPEG.
        let pdf = scratch("mixed.pdf");
        fax.save_all(&pdf);
        let bytes = std::fs::read(&pdf).unwrap();
        let _ = std::fs::remove_file(&pdf);
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"/Count 2") && has(b"/Filter /CCITTFaxDecode") && has(b"/Filter /DCTDecode"));

        // Opened from the list of calls, it is drawn in its colours again.
        fax.open(0, 1);
        assert_eq!(fax.tone_in_view(), Some("JPEG colour"));
        let (_, _, scan) = fax.opened.as_ref().unwrap();
        assert!(close_to(scan.pixels[20], [200, 60, 50]), "{:?}", scan.pixels[20]);
        fax.open(0, 0);
        assert_eq!(fax.tone_in_view(), None);
    }

    #[test]
    fn a_call_record_saves_every_page_as_one_pdf_or_a_png_each() {
        let pages = [page_of(20), page_of(30)];
        let refs: Vec<Kept<'_>> =
            pages.iter().enumerate().map(|(i, p)| (i + 1, p.lines.as_slice(), p.resolution, None)).collect();
        let pdf = scratch("record.pdf");
        assert!(save_pages(&refs, &pdf).starts_with("2 pages saved"));
        assert!(std::fs::metadata(&pdf).is_ok());
        let png = scratch("record.png");
        assert!(save_pages(&refs, &png).starts_with("2 pages saved as"));
        for n in [1, 2] {
            let each = png.with_file_name(format!("record-{n}.png"));
            assert!(std::fs::metadata(&each).is_ok(), "{}", each.display());
            let _ = std::fs::remove_file(each);
        }
        let _ = std::fs::remove_file(pdf);
    }

    #[test]
    fn times_read_as_minutes_and_seconds_and_ages_as_words() {
        assert_eq!(clock(46.0), "0:46");
        assert_eq!(clock(130.4), "2:10");
        assert_eq!(clock(-3.0), "0:00");
        assert_eq!(ago(5.0), "just now");
        assert_eq!(ago(250.0), "4 min ago");
        assert_eq!(ago(7300.0), "2 h ago");
    }

    #[test]
    fn the_page_modulation_is_kept_when_the_modem_goes_back_to_v21_for_the_frames() {
        let mut fax = Fax::new();
        let mut frame = telemetry::Frame::new(16, 16, 16_000.0);
        frame.state = telemetry::CallState::Negotiating;
        frame.fax_phase = Some(fax::call::Phase::Receiving.name());
        frame.fax_coding = "JBIG";
        frame.fax_rate = 14_400;
        frame.fax_error_correction = true;
        frame.fax_progress = Some(0.5);
        frame.modulation = "V.21";
        fax.observe(&frame, 0.0);
        assert_eq!(fax.carried, None, "V.21 taken for the page's modulation");
        frame.modulation = "V.17";
        fax.observe(&frame, 1.0);
        frame.modulation = "V.21";
        frame.fax_progress = Some(1.0);
        fax.observe(&frame, 2.0);
        let carried = fax.carried.expect("nothing carried");
        assert_eq!((carried.modulation, carried.rate), ("V.17", 14_400));
        assert_eq!(fax.calls[0].carried.map(|c| c.modulation), Some("V.17"));
    }

    #[test]
    fn receiving_the_far_end_is_what_its_dcs_chose_not_nothing_in_common() {
        let mut fax = Fax::new();
        let mut frame = telemetry::Frame::new(16, 16, 16_000.0);
        frame.state = telemetry::CallState::Negotiating;
        frame.fax_phase = Some(fax::call::Phase::CheckingTraining.name());
        frame.fax_sending = false;
        // A DCS for V.17 at 14 400: bits 11 to 14 are 0001 (Table 2).
        let mut dcs = vec![0u8; 3];
        dcs[1] = 0b0010_0000;
        assert_eq!(t30::command_rate(&dcs), Some((t30::Modulation::V17, 14_400)), "the field in this test");
        frame.fax_capabilities = Some(dcs);
        fax.observe(&frame, 0.0);
        let caps = fax.far.clone().expect("no far end");
        assert_eq!(fax.shared(&caps).as_deref(), Some("V.17 at 14400 bit/s"));
    }

    #[test]
    fn the_line_sent_follows_the_lines_and_not_the_share_of_the_bits() {
        // A page white for its top half and noise below: half its bits gone
        // is well into the bottom half, not the middle of the page.
        let mut seed = 7u32;
        let lines: Vec<Vec<bool>> = (0..300)
            .map(|y| {
                (0..fax::page::WIDTH)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        y >= 150 && seed >> 31 == 1
                    })
                    .collect()
            })
            .collect();
        let mut fax = Fax::new();
        fax.pages = vec![Page { lines, resolution: Resolution::Standard }];
        fax.line_ends = vec![None];
        for coding in ["MH", "MR", "MMR", "JBIG"] {
            fax.coding = coding;
            // Worked out on a thread: none until it is done.
            let mut reached = None;
            // Generous: a debug build coding JBIG under a full test run is slow.
            for _ in 0..12_000 {
                reached = fax.line_reached(0, 0.5);
                if reached.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let (half, of) = reached.expect("no line");
            assert_eq!(of, 300);
            assert!(half > 200.0, "{coding}: half the bits is line {half}");
            let (all, _) = fax.line_reached(0, 1.0).unwrap();
            assert!((all - 300.0).abs() < 1.0, "{coding}: every bit is line {all}");
        }
        fax.coding = "-";
        assert_eq!(fax.line_reached(0, 0.5), None, "a coding not known yet");
    }

    #[test]
    fn a_page_to_send_is_drawn_the_shape_of_its_paper_at_either_resolution() {
        // The same sheet at standard and at fine: half the lines, the same
        // paper, the same picture -- a pixel a pel, standard lines doubled.
        let size = |resolution: Resolution, n| {
            let page = Page { lines: lines_of(resolution.lines(), false), resolution };
            let image = Fax::thumbnail(&page, 16384, n);
            picture_size(1.0, image.size[0], image.size[1], 1)
        };
        for n in [1, 2, 3] {
            let (a, b) = (size(Resolution::Standard, n), size(Resolution::Fine, n));
            assert!((a.x - b.x).abs() < 0.01 && (a.y / b.y - 1.0).abs() < 0.01, "{a:?} against {b:?}");
            assert!((a.y / a.x - 297.0 / 215.0).abs() < 0.07, "not A4: {a:?}");
        }
        let page = Page { lines: lines_of(5, true), resolution: Resolution::Fine };
        assert_eq!(Fax::thumbnail(&page, 16384, 1).size, [1728, 5]);
        assert_eq!(Fax::thumbnail(&page, 16384, 2).size, [864, 3]);
        // Shrunk, a block all ink is still black, and black and white at 1:1.
        assert!(Fax::thumbnail(&page, 16384, 2).pixels.iter().all(|p| *p == Color32::BLACK));
    }

    #[test]
    fn an_ecm_receive_whose_dcn_was_cut_off_is_still_confirmed() {
        // Seen filming: the sender hangs up the SIP call as it sends DCN, so
        // the receiver never hears it -- but it had already confirmed the
        // last page and was waiting for the far end to hang up.
        let near = |frame| Exchanged { frame, ours: true };
        let far = |frame| Exchanged { frame, ours: false };
        let mut call = CallRecord::new(false, 0.0, "");
        call.frames = vec![
            near(Frame::Csi),
            near(Frame::Dis),
            far(Frame::Dcs),
            near(Frame::Cfr),
            far(Frame::Pps),
            near(Frame::Mcf),
        ];
        call.received = vec![(1, page_of(10))];
        call.sheets = 1;
        call.last_phase = Some(fax::call::Phase::AwaitingDisconnect.name());
        call.ended = Some(40.0);
        assert_eq!(call.verdict(), Verdict::Confirmed);
        // Still receiving the page when the line went: not confirmed.
        call.last_phase = Some(fax::call::Phase::Receiving.name());
        assert!(matches!(call.verdict(), Verdict::Failed { phase: 'C', .. }), "{:?}", call.verdict());
    }
}
