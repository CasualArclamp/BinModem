//! An image, scaled and thresholded into a page a fax can carry.
//!
//! A fax page is not a picture, it is 1728 marks across a line and some
//! number of lines down the paper, each mark either ink or paper. Everything
//! here is about getting from something with grey in it to that, losing as
//! little as possible on the way.

use crate::t4;

/// Pels across a scan line: 1728, which is 8.05 per millimetre across the
/// 215 mm of an A4 sheet (T.4 clause 2, and Table 1's standard).
pub const WIDTH: usize = 1728;

/// Millimetres of paper a scan line covers: the 215 mm of A4 (T.4 clause 2).
pub const WIDTH_MM: f64 = 215.0;

/// Pels per millimetre across the paper: 8.04, and the same at every
/// resolution T.4 defines here. Only the vertical changes.
pub const PELS_PER_MM: f64 = WIDTH as f64 / WIDTH_MM;

/// How finely the paper is scanned down the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    /// 3.85 lines per millimetre, which every fax machine has (T.4 2.1).
    #[default]
    Standard,
    /// 7.7 lines per millimetre: twice down the page, the same across it.
    Fine,
}

impl Resolution {
    /// Lines per millimetre.
    pub fn lines_per_mm(self) -> f64 {
        match self {
            Self::Standard => 3.85,
            Self::Fine => 7.7,
        }
    }

    /// Lines in a full A4 sheet, 297 mm of it.
    ///
    /// 1143 and 2287, which are the numbers Table 1/T.4 gives rather than
    /// what the multiplication gives to the nearest whole line: the second is
    /// not twice the first, because the standard one was rounded first.
    pub fn lines(self) -> usize {
        match self {
            Self::Standard => 1143,
            Self::Fine => 2287,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard, 3.85 lines/mm",
            Self::Fine => "fine, 7.7 lines/mm",
        }
    }
}

/// A page ready to be coded and sent.
#[derive(Debug, Clone)]
pub struct Page {
    /// One row per scan line, one `bool` per pel, true for ink.
    pub lines: Vec<Vec<bool>>,
    pub resolution: Resolution,
}

impl Page {
    pub fn width(&self) -> usize {
        WIDTH
    }

    pub fn height(&self) -> usize {
        self.lines.len()
    }

    /// What fraction of the page is ink.
    ///
    /// Worth knowing before sending one: a page that is nearly all ink codes
    /// to more bits than the same page scanned as paper, and run-length
    /// coding is the reason a fax of a letter takes half a minute and a fax
    /// of a photograph takes five.
    pub fn coverage(&self) -> f64 {
        let ink: usize = self.lines.iter().flatten().filter(|p| **p).count();
        let total = self.lines.len() * WIDTH;
        if total == 0 { 0.0 } else { ink as f64 / total as f64 }
    }

    /// The page as Modified Huffman bits (T.4 4.1).
    pub fn encode(&self) -> t4::Bits {
        t4::encode(&self.lines)
    }

    /// How long the coded page takes at a given line rate, in seconds.
    ///
    /// The coded bits alone. What a real transaction adds to it is the
    /// minimum scan line time the far end asks for in its DIS, which is fill
    /// and not page, and the phase B and D exchanges either side.
    pub fn seconds_at(&self, bits_per_second: u32) -> f64 {
        if bits_per_second == 0 {
            return f64::INFINITY;
        }
        self.encode().len() as f64 / f64::from(bits_per_second)
    }
}

/// Grey, 0.0 for paper and 1.0 for ink, in a rectangle.
///
/// The interface between whatever decoded the file and this: an image crate
/// belongs to the program that reads files, not to the fax.
#[derive(Debug, Clone)]
pub struct Grey {
    pub width: usize,
    pub height: usize,
    /// Row-major, `width * height` of them.
    pub ink: Vec<f32>,
}

impl Grey {
    /// Sample with the box filter that scaling down wants.
    ///
    /// Every source pel inside the destination pel is averaged, rather than
    /// one of them being picked. On a scan of text the difference is the
    /// difference between grey strokes that dither into something readable
    /// and strokes that vanish between the sample points.
    fn box_sample(&self, x0: f64, x1: f64, y0: f64, y1: f64) -> f32 {
        let xa = (x0.floor().max(0.0) as usize).min(self.width.saturating_sub(1));
        let xb = (x1.ceil() as usize).clamp(xa + 1, self.width);
        let ya = (y0.floor().max(0.0) as usize).min(self.height.saturating_sub(1));
        let yb = (y1.ceil() as usize).clamp(ya + 1, self.height);
        let mut sum = 0.0f64;
        let mut n = 0usize;
        for y in ya..yb {
            for x in xa..xb {
                sum += f64::from(self.ink[y * self.width + x]);
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
    }

    /// Scale onto the paper, keeping the shape of the thing in the picture.
    ///
    /// Which is not the same as keeping the ratio of the pel counts, because
    /// a fax pel is not square. Across the paper there are 8.04 to the
    /// millimetre; down it there are 3.85, or 7.7 if the two ends agreed to
    /// it. So a standard-resolution page is 1728 by 1143 and yet is taller
    /// than it is wide, and a photograph fitted by pel counts alone comes out
    /// squashed to half its height.
    ///
    /// The fitting is therefore done in millimetres of paper and converted to
    /// pels at the end.
    fn fit(&self, width: usize, height: usize, lines_per_mm: f64) -> Vec<f32> {
        let mut out = vec![0.0f32; width * height];
        if self.width == 0 || self.height == 0 {
            return out;
        }
        let paper_h_mm = height as f64 / lines_per_mm;
        // The source has square pixels, so its shape is its pixel counts.
        let scale = (WIDTH_MM / self.width as f64)
            .min(paper_h_mm / self.height as f64);
        let drawn_w = (((self.width as f64 * scale) * PELS_PER_MM).round() as usize)
            .clamp(1, width);
        let drawn_h = (((self.height as f64 * scale) * lines_per_mm).round() as usize)
            .clamp(1, height);
        let left = (width - drawn_w) / 2;
        // Against the top rather than centred. A fax comes out of a machine
        // top first, and a page of text that starts a third of the way down
        // looks like a fault.
        let top = 0;
        for y in 0..drawn_h {
            let sy0 = y as f64 * self.height as f64 / drawn_h as f64;
            let sy1 = (y + 1) as f64 * self.height as f64 / drawn_h as f64;
            for x in 0..drawn_w {
                let sx0 = x as f64 * self.width as f64 / drawn_w as f64;
                let sx1 = (x + 1) as f64 * self.width as f64 / drawn_w as f64;
                out[(top + y) * width + left + x] = self.box_sample(sx0, sx1, sy0, sy1);
            }
        }
        out
    }
}

/// How grey becomes ink or paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Halftone {
    /// Anything past halfway is ink.
    ///
    /// Right for text and line art, where the greys are only the edges of
    /// strokes, and wrong for anything with a tone in it: a photograph
    /// thresholded is a silhouette.
    #[default]
    Threshold,
    /// Floyd-Steinberg: the error at each pel is pushed into the neighbours
    /// that have not been decided yet.
    ///
    /// A fax has one bit and no more, so a photograph can only be sent as a
    /// pattern of dots that averages to the right tone. This is that, and it
    /// is what makes a photograph over a fax legible at all.
    Diffuse,
}

impl Halftone {
    pub fn name(self) -> &'static str {
        match self {
            Self::Threshold => "threshold - text and line art",
            Self::Diffuse => "diffuse - photographs",
        }
    }
}

/// Turn an image into a page.
///
/// A fax page is as long as the picture on it, not a fixed sheet: T.4 sends
/// scan lines until the page ends and puts no length in the header, so a short
/// image makes a short page rather than a full A4 with a field of white below
/// it. The picture is laid across the full 215 mm width and the page is however
/// many lines tall that makes it, capped at the A4 sheet -- a picture taller
/// than A4 is scaled down to fit it, as before.
pub fn render(image: &Grey, resolution: Resolution, halftone: Halftone) -> Page {
    let max_lines = resolution.lines();
    let height = if image.width == 0 || image.height == 0 {
        max_lines
    } else {
        // The picture's height in scan lines when it is drawn the full width
        // of the paper: its aspect ratio times the width, in millimetres, at
        // this resolution's lines per millimetre.
        let lines = (image.height as f64 / image.width as f64) * WIDTH_MM * resolution.lines_per_mm();
        (lines.round() as usize).clamp(1, max_lines)
    };
    let mut grey = image.fit(WIDTH, height, resolution.lines_per_mm());
    let mut lines = Vec::with_capacity(height);
    match halftone {
        Halftone::Threshold => {
            for y in 0..height {
                let row = &grey[y * WIDTH..(y + 1) * WIDTH];
                lines.push(row.iter().map(|v| *v >= 0.5).collect());
            }
        }
        Halftone::Diffuse => {
            for y in 0..height {
                let mut row = vec![false; WIDTH];
                for x in 0..WIDTH {
                    let want = grey[y * WIDTH + x];
                    let ink = want >= 0.5;
                    row[x] = ink;
                    let error = want - if ink { 1.0 } else { 0.0 };
                    // 7/16 right, and 3/16, 5/16, 1/16 along the row below.
                    let mut spread = |dx: isize, dy: usize, share: f32| {
                        let nx = x as isize + dx;
                        let ny = y + dy;
                        if nx >= 0 && (nx as usize) < WIDTH && ny < height {
                            grey[ny * WIDTH + nx as usize] += error * share;
                        }
                    };
                    spread(1, 0, 7.0 / 16.0);
                    spread(-1, 1, 3.0 / 16.0);
                    spread(0, 1, 5.0 / 16.0);
                    spread(1, 1, 1.0 / 16.0);
                }
                lines.push(row);
            }
        }
    }
    Page { lines, resolution }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, ink: f32) -> Grey {
        Grey { width, height, ink: vec![ink; width * height] }
    }

    #[test]
    fn a_page_is_as_long_as_the_picture_not_the_sheet() {
        // A square drawn the full 215 mm across is 215 mm tall, well short of
        // A4's 297: the page is that long and no longer, with no field of
        // white below it.
        for resolution in [Resolution::Standard, Resolution::Fine] {
            let square = render(&solid(100, 100, 0.0), resolution, Halftone::Threshold);
            assert_eq!(square.width(), 1728);
            let mm = square.height() as f64 / resolution.lines_per_mm();
            assert!((mm - 215.0).abs() < 3.0, "{resolution:?}: {mm} mm tall");
        }
        // A picture taller than the sheet is capped at it and scaled to fit,
        // as a real machine's page is.
        let tall = render(&solid(100, 400, 0.0), Resolution::Standard, Halftone::Threshold);
        assert_eq!(tall.height(), 1143);
        let fine = render(&solid(100, 400, 0.0), Resolution::Fine, Halftone::Threshold);
        assert_eq!(fine.height(), 2287);
    }

    #[test]
    fn white_paper_stays_white_and_black_paper_stays_black() {
        let white = render(&solid(64, 64, 0.0), Resolution::Standard, Halftone::Threshold);
        assert_eq!(white.coverage(), 0.0);
        // Black now fills its page: the page is the square's own shape, so the
        // ink reaches every edge with no white margin below it.
        let black = render(&solid(64, 64, 1.0), Resolution::Standard, Halftone::Threshold);
        assert!(black.coverage() > 0.99, "a black square left the page part white: {}", black.coverage());
    }

    fn inked_lines(page: &Page) -> Vec<usize> {
        page.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.iter().any(|p| *p))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn a_square_comes_out_square_on_the_paper() {
        // 215 mm across the page, so a square is 215 mm down it too, which at
        // 3.85 lines to the millimetre is 828 lines and not 1728. Fitting by
        // pel counts alone gives the second, and a picture squashed to half
        // its height.
        for (resolution, want) in
            [(Resolution::Standard, 828.0), (Resolution::Fine, 1656.0)]
        {
            let page = render(&solid(200, 200, 1.0), resolution, Halftone::Threshold);
            let inked = inked_lines(&page);
            let tall = inked.len() as f64;
            assert!(
                (tall - want).abs() < 4.0,
                "{}: a square came out {tall} lines tall, wanted {want}",
                resolution.name()
            );
            assert_eq!(inked.first(), Some(&0), "and it starts at the top");
        }
    }

    #[test]
    fn a_tall_picture_is_limited_by_the_length_of_the_paper() {
        // Taller than A4's 215 by 297, so it is the height that runs out.
        let page = render(&solid(100, 400, 1.0), Resolution::Standard, Halftone::Threshold);
        let inked = inked_lines(&page);
        assert_eq!(inked.len(), page.height(), "it should fill the paper");
        let across = page.lines[0].iter().filter(|p| **p).count();
        assert!(
            across < WIDTH,
            "and not fill the width: {across} of {WIDTH}"
        );
    }

    /// Square pixels in the shape of A4, so the picture fills the paper.
    fn a4(ink: f32) -> Grey {
        solid(430, 594, ink)
    }

    #[test]
    fn half_grey_is_a_silhouette_thresholded_and_a_pattern_diffused() {
        let grey = a4(0.5);
        let hard = render(&grey, Resolution::Standard, Halftone::Threshold);
        // Exactly half is ink under the rule "past halfway", so the whole
        // thing goes one way. That is the failure the other mode exists for.
        // Not quite every pel, because a picture in the shape of A4 lands a
        // rounded number of lines short of the bottom of it.
        assert!(hard.coverage() > 0.99, "{}", hard.coverage());

        let soft = render(&grey, Resolution::Standard, Halftone::Diffuse);
        let ink = soft.coverage();
        assert!(
            (0.45..0.55).contains(&ink),
            "diffusing half grey gave {ink} of a page"
        );
    }

    #[test]
    fn a_blank_page_codes_to_almost_nothing() {
        // An A4-shaped blank page, so it is the full sheet: a square one would
        // be shorter now, which the length test above covers.
        let page = render(&solid(215, 297, 0.0), Resolution::Standard, Halftone::Threshold);
        assert_eq!(page.height(), 1143);
        let bits = page.encode();
        // Every line is EOL, 1728 white, 0 white: 29 bits, plus the RTC.
        assert_eq!(bits.len(), 1143 * 29 + 6 * 12);
        // Which at the slowest fax rate there is comes to a few seconds.
        let seconds = page.seconds_at(2400);
        assert!(seconds > 13.0 && seconds < 15.0, "{seconds} s at 2400");
    }

    #[test]
    fn a_dithered_photograph_costs_far_more_than_a_page_of_text() {
        // The whole reason a fax of a photograph is slow: run-length coding
        // has nothing to work with once every other pel changes colour.
        let text = render(&a4(0.0), Resolution::Standard, Halftone::Threshold);
        let photo = render(&a4(0.5), Resolution::Standard, Halftone::Diffuse);
        assert!(
            photo.encode().len() > 50 * text.encode().len(),
            "photograph {} bits against text {} bits",
            photo.encode().len(),
            text.encode().len()
        );
    }

    #[test]
    fn every_line_is_the_full_width() {
        let page = render(&solid(37, 91, 0.7), Resolution::Fine, Halftone::Diffuse);
        assert!(page.lines.iter().all(|l| l.len() == WIDTH));
    }
}
