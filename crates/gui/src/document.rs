//! A PDF's pages, drawn straight onto a fax's paper.
//!
//! A picture is fitted to the paper because it has no size of its own, only
//! pixels. A PDF page has a size -- A4 is 210 by 297 mm, whatever is on it --
//! so it is drawn at that size, onto the pel grid itself: 8.04 pels to the
//! millimetre across and 7.7 lines down, which is fine resolution. Standard is
//! every two of those lines taken together, exactly, since its lines are
//! twice as far apart. Nothing is drawn twice and nothing is resampled, so a
//! hairline rule or a full stop lands on the pels it covers and not smeared
//! across the ones either side.

use fax::page::{Halftone, PELS_PER_MM, Page, Resolution, WIDTH, WIDTH_MM};
use fax::pdf::POINTS_PER_MM;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::{RenderCache, RenderSettings};
use std::sync::Arc;

/// Millimetres down an A4 sheet, the longest page this end sends: T.4 allows
/// longer, but a picture is capped at A4 as well, and a machine that prints
/// on A4 cuts anything past it onto a second sheet.
const A4_MM: f64 = 297.0;

/// One page of a document on fine resolution's grid: `WIDTH` pels across,
/// 7.7 lines to the millimetre down, as much ink as each pel has in it.
///
/// Bytes rather than the `f32` a picture's grey is: a full sheet is four
/// million pels.
struct Sheet {
    lines: usize,
    ink: Vec<u8>,
}

impl std::fmt::Debug for Sheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sheet({} lines)", self.lines)
    }
}

impl Sheet {
    /// The sheet at a resolution, in ink.
    fn page(&self, resolution: Resolution, halftone: Halftone) -> Page {
        let grey: Vec<f32> = match resolution {
            Resolution::Fine => self.ink.iter().map(|v| f32::from(*v) / 255.0).collect(),
            // Each standard line is two fine ones, and a fine line past the
            // last is paper.
            Resolution::Standard => {
                let lines = self.lines.div_ceil(2).min(Resolution::Standard.lines());
                let mut grey = Vec::with_capacity(lines * WIDTH);
                for y in 0..lines {
                    let upper = &self.ink[2 * y * WIDTH..(2 * y + 1) * WIDTH];
                    let lower = self.ink.get((2 * y + 1) * WIDTH..(2 * y + 2) * WIDTH);
                    for x in 0..WIDTH {
                        let sum = u16::from(upper[x]) + lower.map_or(0, |l| u16::from(l[x]));
                        grey.push(f32::from(sum) / 510.0);
                    }
                }
                grey
            }
        };
        fax::page::ink(grey, resolution, halftone)
    }
}

/// The most pages of a document that go in one fax.
///
/// A page is a byte a pel, four million of them at fine resolution, and the
/// pages are held twice while a call takes its copy. Fifty is past anything a
/// person sends by fax and short of a gigabyte; a document longer than that is
/// a document to split.
pub const MOST_PAGES: usize = 50;

/// Whether a file is a PDF, by its first bytes rather than its name.
pub fn is_pdf(bytes: &[u8]) -> bool {
    // 7.5.2: the header is the first line, though readers have long allowed
    // some junk before it.
    bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-")
}

/// The pages of a PDF -- the first [`MOST_PAGES`] of them -- drawn onto the
/// paper and inked at a resolution, and how many pages the document has.
///
/// Drawn again whenever they are wanted at another resolution rather than
/// kept drawn: a page takes about 15 ms, and keeping them costs as much
/// memory again as the pages themselves.
pub fn read(
    bytes: &Arc<Vec<u8>>,
    resolution: Resolution,
    halftone: Halftone,
) -> Result<(Vec<Page>, usize), String> {
    let bytes = Arc::clone(bytes);
    // A renderer is a large thing for a document to find its way into a
    // corner of, and this one runs on the window's thread: a document it
    // cannot draw is a message, not the window gone.
    let drawn = std::panic::catch_unwind(move || {
        let pdf = Pdf::new(bytes).map_err(|e| match e {
            LoadPdfError::Decryption(_) => "the PDF is locked with a password".to_owned(),
            LoadPdfError::Invalid => "that is not a PDF this can read".to_owned(),
        })?;
        let cache = RenderCache::new();
        let settings = InterpreterSettings::default();
        let all = pdf.pages();
        let pages: Vec<Page> = all
            .iter()
            .take(MOST_PAGES)
            .map(|page| draw(page, &cache, &settings).page(resolution, halftone))
            .collect();
        if pages.is_empty() {
            return Err("the PDF has no pages".to_owned());
        }
        Ok((pages, all.len()))
    });
    drawn.unwrap_or_else(|_| Err("the PDF could not be drawn".to_owned()))
}

/// One page onto the paper: at its own size if it fits across the 215 mm and
/// down an A4 sheet, and shrunk until it does if not.
///
/// A page too wide for the line and wider than it is tall -- A4 or Letter on
/// its side -- is turned a quarter turn anticlockwise first, as a printer set
/// to landscape turns it, so that it fills the sheet at its own size instead
/// of shrinking to a strip across the top of it. A page that fits across is
/// left as it is, however short: a fax page is as long as what is on it, and
/// a received one saved as a PDF is often wider than it is tall.
fn draw<'a>(
    page: &'a hayro::hayro_syntax::page::Page<'a>,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
) -> Sheet {
    let (wide_pt, tall_pt) = page.render_dimensions();
    // A millimetre over, so that Letter's 215.9 is not too wide.
    let turned = wide_pt > tall_pt && f64::from(wide_pt) / POINTS_PER_MM > WIDTH_MM + 1.0;
    let (across_pt, down_pt) = if turned { (tall_pt, wide_pt) } else { (wide_pt, tall_pt) };
    let across_mm = f64::from(across_pt) / POINTS_PER_MM;
    let down_mm = f64::from(down_pt) / POINTS_PER_MM;
    let shrink = (WIDTH_MM / across_mm).min(A4_MM / down_mm).min(1.0);
    let pels = (PELS_PER_MM * shrink / POINTS_PER_MM) as f32;
    let lines = (Resolution::Fine.lines_per_mm() * shrink / POINTS_PER_MM) as f32;
    let (x_scale, y_scale) = if turned { (lines, pels) } else { (pels, lines) };
    // Rounded rather than cut short, which is what the renderer would do:
    // A4 is 2286.9 fine lines, and Table 1/T.4's 2287.
    let pixels = |points: f32, scale: f32| Some((points * scale).round().clamp(1.0, f32::from(u16::MAX)) as u16);
    let pixmap = hayro::render(
        page,
        cache,
        settings,
        &RenderSettings {
            x_scale,
            y_scale,
            width: pixels(wide_pt, x_scale),
            height: pixels(tall_pt, y_scale),
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    let (w, h) = (usize::from(pixmap.width()), usize::from(pixmap.height()));
    // Luminance as the image crate reckons it for a picture, turned into
    // ink. The background is opaque white, so the alpha is always whole and
    // the premultiplied colour is the colour.
    let ink: Vec<u8> = pixmap
        .data()
        .iter()
        .map(|p| {
            let luma = (2126 * u32::from(p.r) + 7152 * u32::from(p.g) + 722 * u32::from(p.b)) / 10_000;
            255 - luma as u8
        })
        .collect();
    // Across and down the paper, from the pixmap's across and down: a
    // quarter turn anticlockwise puts the pixmap's right-hand column along
    // the top of the sheet.
    let (across, down) = if turned { (h, w) } else { (w, h) };
    let at = |x: usize, y: usize| if turned { ink[x * w + (w - 1 - y)] } else { ink[y * w + x] };
    let across = across.min(WIDTH);
    let down = down.min(Resolution::Fine.lines()).max(1);
    let left = (WIDTH - across) / 2;
    let mut sheet = vec![0u8; WIDTH * down];
    for y in 0..down.min(if turned { w } else { h }) {
        for x in 0..across {
            sheet[y * WIDTH + left + x] = at(x, y);
        }
    }
    Sheet { lines: down, ink: sheet }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF of one page with rectangles on it, written out by hand: what
    /// the renderer is given is known to the point.
    fn pdf_with(wide: f64, tall: f64, drawing: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {wide} {tall}] /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{drawing}\nendstream", drawing.len()),
        ];
        for (n, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", n + 1).as_bytes());
        }
        let table = out.len();
        out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f\r\n");
        for at in offsets {
            out.extend_from_slice(format!("{at:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes());
        out
    }

    const A4: (f64, f64) = (595.28, 841.89);

    fn pages_of(pdf: Vec<u8>, resolution: Resolution) -> Vec<Page> {
        read(&Arc::new(pdf), resolution, Halftone::Threshold).expect("it did not read").0
    }

    fn mm(points: f64) -> f64 {
        points / POINTS_PER_MM
    }

    /// Where the ink is: the first and last line with any, and the first and
    /// last pel of any line.
    fn inked(page: &Page) -> Option<(usize, usize, usize, usize)> {
        let rows: Vec<usize> = (0..page.height()).filter(|y| page.lines[*y].contains(&true)).collect();
        let cols: Vec<usize> = (0..WIDTH).filter(|x| page.lines.iter().any(|l| l[*x])).collect();
        Some((*rows.first()?, *rows.last()?, *cols.first()?, *cols.last()?))
    }

    #[test]
    fn an_a4_page_is_a4_on_the_paper_at_either_resolution() {
        // A 10 mm square 20 mm in from the left and 30 mm down from the top.
        let side = 10.0 * POINTS_PER_MM;
        let (x, y) = (20.0 * POINTS_PER_MM, A4.1 - 30.0 * POINTS_PER_MM - side);
        let pdf = pdf_with(A4.0, A4.1, &format!("0 g {x} {y} {side} {side} re f"));
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let pages = pages_of(pdf.clone(), resolution);
            assert_eq!(pages.len(), 1);
            let page = &pages[0];
            assert_eq!(page.height(), resolution.lines(), "{resolution:?}");
            let (top, bottom, left, right) = inked(page).expect("no ink");
            let lines_mm = resolution.lines_per_mm();
            // The 210 mm sheet is centred on the 215 mm line, 2.5 mm in.
            let across = |pel: usize| pel as f64 / PELS_PER_MM - 2.5;
            let down = |line: usize| line as f64 / lines_mm;
            let near = |got: f64, want: f64| (got - want).abs() < 0.3;
            assert!(near(across(left), 20.0), "{resolution:?}: left edge at {} mm", across(left));
            assert!(near(across(right + 1), 30.0), "{resolution:?}: right edge at {} mm", across(right + 1));
            assert!(near(down(top), 30.0), "{resolution:?}: top edge at {} mm", down(top));
            assert!(near(down(bottom + 1), 40.0), "{resolution:?}: bottom edge at {} mm", down(bottom + 1));
        }
    }

    #[test]
    fn a_hairline_lands_on_the_pels_it_covers() {
        // Half a point, 0.18 mm: one or two pels across at 8.04 to the
        // millimetre, and it must be there at both resolutions.
        let pdf = pdf_with(A4.0, A4.1, "0 G 0.5 w 100 100 m 100 700 l S 100 400 m 500 400 l S");
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let page = &pages_of(pdf.clone(), resolution)[0];
            let lines_mm = resolution.lines_per_mm();
            let row = ((A4.1 - 400.0) / POINTS_PER_MM * lines_mm) as usize;
            let across = page.lines[row - 1..=row + 1].iter().filter(|l| l[1000]).count();
            assert!(across >= 1, "{resolution:?}: the rule across the page is gone");
            let col = ((100.0 / POINTS_PER_MM + 2.5) * PELS_PER_MM) as usize;
            let down = page.lines[row + 50][col - 1..=col + 1].iter().filter(|p| **p).count();
            assert!(down >= 1, "{resolution:?}: the rule down the page is gone");
        }
    }

    #[test]
    fn a_landscape_page_is_turned_to_fill_the_sheet() {
        // A4 on its side, with a mark in its top left corner. A quarter turn
        // anticlockwise puts that corner at the bottom left of the sheet.
        let side = 20.0 * POINTS_PER_MM;
        let pdf = pdf_with(A4.1, A4.0, &format!("0 g 0 {} {side} {side} re f", A4.0 - side));
        let page = &pages_of(pdf, Resolution::Fine)[0];
        assert_eq!(page.height(), Resolution::Fine.lines(), "not the full sheet");
        let (top, bottom, left, right) = inked(page).expect("no ink");
        let bottom_mm = (bottom + 1) as f64 / 7.7;
        assert!((bottom_mm - mm(A4.1)).abs() < 0.5, "the mark ends {bottom_mm} mm down");
        assert!(((bottom + 1 - top) as f64 / 7.7 - 20.0).abs() < 0.5);
        assert!(((left as f64 / PELS_PER_MM) - 2.5).abs() < 0.3, "the mark starts {left} pels in");
        assert!((((right + 1 - left) as f64 / PELS_PER_MM) - 20.0).abs() < 0.3);
    }

    #[test]
    fn a_short_page_that_fits_across_is_not_turned() {
        // A5 on its side, 210 by 148 mm: it fits the line as it is, and goes
        // at its own size, a short page with its mark still top left.
        let (wide, tall) = (595.28, 419.53);
        let side = 20.0 * POINTS_PER_MM;
        let pdf = pdf_with(wide, tall, &format!("0 g 0 {} {side} {side} re f", tall - side));
        let page = &pages_of(pdf, Resolution::Fine)[0];
        assert!((page.height() as f64 / 7.7 - mm(tall)).abs() < 0.5, "{} lines", page.height());
        let (top, bottom, left, _) = inked(page).expect("no ink");
        assert_eq!(top, 0);
        assert!(((bottom + 1) as f64 / 7.7 - 20.0).abs() < 0.5);
        assert!(((left as f64 / PELS_PER_MM) - 2.5).abs() < 0.3);
    }

    #[test]
    fn a_page_too_big_for_the_paper_is_shrunk_to_fit_it() {
        // A3, filled: it comes out the width of the line and A4 long.
        let pdf = pdf_with(841.89, 1190.55, "0 g 0 0 841.89 1190.55 re f");
        let page = &pages_of(pdf, Resolution::Standard)[0];
        assert_eq!(page.height(), Resolution::Standard.lines());
        assert!(page.coverage() > 0.97, "{}", page.coverage());
    }

    #[test]
    fn what_is_not_a_pdf_says_so() {
        assert!(read(&Arc::new(b"%PDF-1.4 and nothing else".to_vec()), Resolution::Standard, Halftone::Threshold).is_err());
        assert!(is_pdf(&pdf_with(A4.0, A4.1, "")));
        assert!(!is_pdf(b"\x89PNG\r\n\x1a\n"));
    }

    /// How long a real document takes, for anybody who wonders.
    ///
    /// `DOCUMENT=path cargo test -p gui --release document::tests::timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn timing() {
        let Ok(path) = std::env::var("DOCUMENT") else { return };
        let bytes = Arc::new(std::fs::read(&path).expect("no such file"));
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let start = std::time::Instant::now();
            let (pages, all) = read(&bytes, resolution, Halftone::Threshold).expect("it did not read");
            println!("{} of {all} pages at {resolution:?} in {:?}", pages.len(), start.elapsed());
        }
    }
}
