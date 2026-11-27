//! Pages written out as a PDF: one sheet of paper each, the size the page was
//! scanned at, with the page on it as a T.6 picture.
//!
//! PDF has carried fax pages from the start. Its CCITTFaxDecode filter takes
//! T.4 and T.6 coding as they are (ISO 32000-1 7.4.6, Table 11), so a page
//! goes in coded the way it would go down a line under error correction, as
//! MMR -- no other compression, and nothing decoded or re-coded on the way.
//!
//! Each page is its own sheet, sized in points from the pels: 8.04 across to
//! the millimetre and 3.85 or 7.7 down it. The picture is stretched onto that
//! sheet, which is what makes a standard-resolution page come out the right
//! shape rather than squashed to half its height -- the same thing the PNG
//! does by drawing every line twice, without drawing any line twice.
//!
//! A page that came with tone, greyscale or colour, goes in the same way: as
//! the JPEG data stream it arrived in. PDF's DCTDecode filter takes baseline
//! JPEG as it is (ISO 32000-1 7.4.8), and its Lab colour space is CIELAB
//! with a white point given (8.6.5.4), which is what T.42 codes. So nothing
//! is decoded, converted or coded again there either.

use crate::mmr;
use crate::page::{PELS_PER_MM, Resolution};
use crate::picture::{Picture, WHITE_D50};

/// Points to the millimetre: PDF measures in 1/72 of an inch.
pub const POINTS_PER_MM: f64 = 72.0 / 25.4;

/// A page for [`document`]: its scan lines, true for ink, and how finely it
/// was scanned down the paper.
pub type Sheet<'a> = (&'a [Vec<bool>], Resolution);

/// A page of either kind.
#[derive(Debug, Clone, Copy)]
pub enum Leaf<'a> {
    /// Ink and paper: scan lines, true for ink, and how finely they were
    /// scanned down the paper.
    Ink(&'a [Vec<bool>], Resolution),
    /// Greyscale or colour, as T.4 Annex E sends it.
    Tone(&'a Picture),
}

impl Leaf<'_> {
    fn is_empty(&self) -> bool {
        match self {
            Self::Ink(lines, _) => lines.is_empty(),
            Self::Tone(picture) => picture.width == 0 || picture.height == 0,
        }
    }

    /// Pels across and down, and the sheet in millimetres.
    fn size(&self) -> (usize, usize, f64, f64) {
        match self {
            Self::Ink(lines, resolution) => {
                let (columns, rows) = (lines[0].len(), lines.len());
                (columns, rows, columns as f64 / PELS_PER_MM, rows as f64 / resolution.lines_per_mm())
            }
            Self::Tone(picture) => {
                let (wide, tall) = picture.size_mm();
                (picture.width, picture.height, wide, tall)
            }
        }
    }
}

/// The pages, in order, as the bytes of a PDF file.
///
/// A page with no lines has nothing to draw and no size to draw it at, and is
/// left out.
pub fn document(pages: &[Sheet<'_>]) -> Vec<u8> {
    let leaves: Vec<Leaf<'_>> = pages.iter().map(|(lines, resolution)| Leaf::Ink(lines, *resolution)).collect();
    document_of(&leaves)
}

/// What a tone picture's image dictionary says of its colours, and the
/// stream that goes with it.
fn tone_picture(picture: &Picture) -> (String, Vec<u8>) {
    // The stream as it arrived, with its length filled in if a DNL marker
    // carried it; or one made here, for a picture that has none.
    let stream = if picture.jpeg.is_empty() {
        crate::jpeg::encode(picture, crate::jpeg::Options::default())
    } else {
        crate::jpeg::for_a_file(&picture.jpeg, picture.height)
    };
    // A code N stands for (N - P) x Q / 255 (T.4 E.6.6), so codes 0 and 255
    // stand for the two ends of a Decode pair (ISO 32000-1 8.9.5.2).
    let ends = |component: usize| (picture.gamut.value(component, 0), picture.gamut.value(component, 255));
    let white = format!("/WhitePoint [{} {} {}]", WHITE_D50[0], WHITE_D50[1], WHITE_D50[2]);
    let space = if picture.is_colour() {
        let (a, b) = (ends(1), ends(2));
        // Range is what the space allows, and whole numbers that hold the
        // ends are enough for it.
        let range = format!("/Range [{} {} {} {}]", a.0.floor(), a.1.ceil(), b.0.floor(), b.1.ceil());
        let l = ends(0);
        format!(
            "/ColorSpace [/Lab << {white} {range} >>] /Decode [{} {} {} {} {} {}] ",
            l.0, l.1, a.0, a.1, b.0, b.1
        )
    } else {
        // Lab has three components and a greyscale page has one, so each L*
        // code is looked up as a colour with no a* or b* (8.6.6.3). The
        // range is symmetric so that 128 stands for zero.
        let table: String = (0..=255u8)
            .map(|code| {
                let l = (picture.gamut.value(0, code).clamp(0.0, 100.0) * 2.55).round() as u8;
                format!("{l:02X}8080")
            })
            .collect();
        format!("/ColorSpace [/Indexed [/Lab << {white} /Range [-128 127 -128 127] >>] 255 <{table}>] ")
    };
    // ColorTransform 0: the three components are CIELAB as they stand. Left
    // to its default, the filter takes any three components for YCbCr and
    // turns them to RGB before the colour space ever sees them (7.4.8, Table
    // 13), and white paper comes out orange. A stream off a fax has no Adobe
    // marker to say otherwise, so the dictionary must.
    (format!("{space}/BitsPerComponent 8 /Filter /DCTDecode /DecodeParms << /ColorTransform 0 >> "), stream)
}

/// The pages, of ink or of tone, in order, as the bytes of a PDF file.
pub fn document_of(pages: &[Leaf<'_>]) -> Vec<u8> {
    let pages: Vec<&Leaf<'_>> = pages.iter().filter(|leaf| !leaf.is_empty()).collect();
    let mut out = Writer::default();
    // The header, and a comment of bytes past 127 on the line after it, which
    // is what 7.5.2 recommends so that anything moving the file knows it is
    // binary.
    out.bytes(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    // 1 is the catalogue, 2 the page tree, 3 what made the file; each page is
    // three more after that -- the page, what is drawn on it, the picture.
    let page_object = |n: usize| 4 + 3 * n;
    out.object(1, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..pages.len()).map(|n| format!("{} 0 R", page_object(n))).collect();
    out.object(
        2,
        format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).as_bytes(),
    );
    out.object(
        3,
        format!("<< /Producer (BinModem {}) >>", env!("CARGO_PKG_VERSION")).as_bytes(),
    );

    for (n, leaf) in pages.iter().enumerate() {
        let (page, drawn, picture) = (page_object(n), page_object(n) + 1, page_object(n) + 2);
        let (columns, rows, wide, tall) = leaf.size();
        let (wide, tall) = (points(wide), points(tall));
        out.object(
            page,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {wide} {tall}] \
                 /Resources << /XObject << /Page {picture} 0 R >> >> /Contents {drawn} 0 R >>"
            )
            .as_bytes(),
        );
        // The picture is a unit square until it is drawn: scaled to the sheet
        // it fills the sheet (8.9).
        out.stream(drawn, "", format!("q {wide} 0 0 {tall} 0 0 cm /Page Do Q").as_bytes());
        let head = format!("/Type /XObject /Subtype /Image /Width {columns} /Height {rows} ");
        match leaf {
            Leaf::Ink(lines, _) => {
                // K -1 is T.6 (Table 11). BlackIs1 false, which is also the
                // default, is T.4's sense of the bits: a 0 in the picture is
                // black, which is what DeviceGray makes of it too. The
                // coding ends in EOFB, which EndOfBlock -- true unless it
                // says otherwise -- expects to find.
                let coded = mmr::encode(lines);
                out.stream(
                    picture,
                    &format!(
                        "{head}/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode \
                         /DecodeParms << /K -1 /Columns {columns} /Rows {rows} /BlackIs1 false >> "
                    ),
                    coded.octets(),
                );
            }
            Leaf::Tone(tone) => {
                let (colours, stream) = tone_picture(tone);
                out.stream(picture, &format!("{head}{colours}"), &stream);
            }
        }
    }
    out.finish(page_object(pages.len()), 1, 3)
}

/// Millimetres in points, to the hundredth: finer than a pel, and short.
fn points(mm: f64) -> String {
    let p = (mm * POINTS_PER_MM * 100.0).round() / 100.0;
    format!("{p}")
}

/// A PDF being written: the bytes, and where each object in them starts,
/// which the cross-reference table at the end has to give to the byte.
#[derive(Debug, Default)]
struct Writer {
    out: Vec<u8>,
    /// By object number, from 1.
    offsets: Vec<usize>,
}

impl Writer {
    fn bytes(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    fn start(&mut self, number: usize) {
        if self.offsets.len() < number {
            self.offsets.resize(number, 0);
        }
        self.offsets[number - 1] = self.out.len();
        self.bytes(format!("{number} 0 obj\n").as_bytes());
    }

    fn object(&mut self, number: usize, body: &[u8]) {
        self.start(number);
        self.bytes(body);
        self.bytes(b"\nendobj\n");
    }

    /// A stream: `entries` go in its dictionary beside its length. The end of
    /// line before `endstream` is not part of the data, and not counted in
    /// it (7.3.8.1).
    fn stream(&mut self, number: usize, entries: &str, data: &[u8]) {
        self.start(number);
        self.bytes(format!("<< {entries}/Length {} >>\nstream\n", data.len()).as_bytes());
        self.bytes(data);
        self.bytes(b"\nendstream\nendobj\n");
    }

    /// The cross-reference table and the trailer (7.5.4, 7.5.5). `objects` is
    /// one past the last object number.
    fn finish(mut self, objects: usize, root: usize, info: usize) -> Vec<u8> {
        let table = self.out.len();
        self.bytes(format!("xref\n0 {objects}\n").as_bytes());
        // Every entry exactly 20 bytes, its end of line two of them.
        self.bytes(b"0000000000 65535 f\r\n");
        for n in 1..objects {
            let at = self.offsets.get(n - 1).copied().unwrap_or(0);
            self.bytes(format!("{at:010} 00000 n\r\n").as_bytes());
        }
        self.bytes(
            format!(
                "trailer\n<< /Size {objects} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{table}\n%%EOF\n"
            )
            .as_bytes(),
        );
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::WIDTH;

    /// A page with something on every line that differs from the line above,
    /// so that a decoder that slips shows it.
    fn page_of(rows: usize, seed: usize) -> Vec<Vec<bool>> {
        (0..rows)
            .map(|y| (0..WIDTH).map(|x| (x * 7 + y * 13 + seed) % 29 < 9 || x / 64 == y % 27).collect())
            .collect()
    }

    fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        hay[from..].windows(needle.len()).position(|w| w == needle).map(|i| i + from)
    }

    /// The text of the dictionary of object `n`, and where it starts, found
    /// through the cross-reference table -- which is checked on the way.
    fn object(pdf: &[u8], n: usize) -> (usize, String) {
        let tail = pdf.len() - 40;
        let mark = find(pdf, b"startxref\n", tail).unwrap() + 10;
        let digits = find(pdf, b"\n", mark).unwrap();
        let at: usize = std::str::from_utf8(&pdf[mark..digits]).unwrap().parse().unwrap();
        assert!(pdf[at..].starts_with(b"xref\n0 "), "startxref does not point at the table");
        let entries = find(pdf, b"65535 f\r\n", at).unwrap() + 9;
        let entry = &pdf[entries + (n - 1) * 20..entries + n * 20];
        assert_eq!(&entry[10..], b" 00000 n\r\n");
        let offset: usize = std::str::from_utf8(&entry[..10]).unwrap().parse().unwrap();
        let head = format!("{n} 0 obj\n");
        assert!(pdf[offset..].starts_with(head.as_bytes()), "object {n} is not where the table says");
        let body = offset + head.len();
        let end = find(pdf, b">>\n", body).unwrap() + 2;
        (body, String::from_utf8(pdf[body..end].to_vec()).unwrap())
    }

    /// The sheet's width and height, in points.
    fn media_box(dict: &str) -> (f64, f64) {
        let at = dict.find("/MediaBox [").unwrap() + 11;
        let corners: Vec<f64> = dict[at..dict[at..].find(']').unwrap() + at]
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(corners[..2], [0.0, 0.0]);
        (corners[2], corners[3])
    }

    fn number(dict: &str, key: &str) -> f64 {
        let at = dict.find(key).unwrap_or_else(|| panic!("no {key} in {dict}")) + key.len();
        dict[at..].split_whitespace().next().unwrap().trim_end_matches(']').parse().unwrap()
    }

    #[test]
    fn every_page_is_its_own_sheet_the_size_it_was_scanned_at() {
        let standard = page_of(1143, 0);
        let fine = page_of(1000, 5);
        let pdf = document(&[
            (&standard, Resolution::Standard),
            (&[], Resolution::Standard),
            (&fine, Resolution::Fine),
        ]);
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let (_, tree) = object(&pdf, 2);
        assert!(tree.contains("/Kids [4 0 R 7 0 R] /Count 2"), "{tree}");

        for (n, (lines, resolution)) in [(&standard, Resolution::Standard), (&fine, Resolution::Fine)]
            .into_iter()
            .enumerate()
        {
            let (_, page) = object(&pdf, 4 + 3 * n);
            // 1728 pels over 215 mm across, and the lines at the resolution's
            // pitch down, in points.
            let (wide, tall) = media_box(&page);
            assert!((wide - 215.0 * POINTS_PER_MM).abs() < 0.01, "{wide} points across");
            let want = lines.len() as f64 / resolution.lines_per_mm() * POINTS_PER_MM;
            assert!((tall - want).abs() < 0.01, "{tall} points down, not {want}");
            // The picture drawn over the whole of it.
            let (body, drawn) = object(&pdf, 5 + 3 * n);
            let length = number(&drawn, "/Length ") as usize;
            let at = find(&pdf, b"stream\n", body).unwrap() + 7;
            let content = String::from_utf8_lossy(&pdf[at..at + length]).into_owned();
            let sheet = format!("{} 0 0 {}", points(215.0), points(lines.len() as f64 / resolution.lines_per_mm()));
            assert_eq!(content, format!("q {sheet} 0 0 cm /Page Do Q"));
        }
        // A full standard page is A4's 297 mm.
        let (_, first) = object(&pdf, 4);
        let (_, tall) = media_box(&first);
        assert!((tall / POINTS_PER_MM - 297.0).abs() < 0.5, "{} mm", tall / POINTS_PER_MM);
    }

    fn tone(components: usize, rows: usize) -> Picture {
        let width = WIDTH;
        Picture {
            width,
            height: rows,
            components,
            samples: (0..width * rows * components).map(|i| (i / 7 % 256) as u8).collect(),
            pels_per_inch: 200,
            ..Picture::default()
        }
    }

    #[test]
    fn a_colour_page_goes_in_as_the_jpeg_it_arrived_in_and_says_it_is_lab() {
        // As off a line from a machine that scans while it sends: Y = 0 in
        // the frame header and the length in a DNL marker.
        let mut picture = tone(3, 40);
        let options = crate::jpeg::Options { lines_at_the_end: true, ..crate::jpeg::Options::default() };
        picture.jpeg = crate::jpeg::encode(&picture, options);
        let ink = page_of(30, 0);
        let pdf = document_of(&[Leaf::Ink(&ink, Resolution::Fine), Leaf::Tone(&picture)]);
        let (_, tree) = object(&pdf, 2);
        assert!(tree.contains("/Count 2"), "{tree}");
        // The sheet is the picture's size at 200 pels to 25.4 mm.
        let (wide, tall) = media_box(&object(&pdf, 7).1);
        assert!((wide - 1728.0 / 200.0 * 72.0).abs() < 0.01, "{wide} points across");
        assert!((tall - 40.0 / 200.0 * 72.0).abs() < 0.01, "{tall} points down");

        let (body, dict) = object(&pdf, 9);
        assert!(dict.contains("/Filter /DCTDecode"), "{dict}");
        // The components are not YCbCr and the filter must not treat them so:
        // without this a viewer shows white paper as orange.
        assert!(dict.contains("/DecodeParms << /ColorTransform 0 >>"), "{dict}");
        assert!(dict.contains("/BitsPerComponent 8"), "{dict}");
        assert_eq!(number(&dict, "/Width "), 1728.0);
        assert_eq!(number(&dict, "/Height "), 40.0);
        // CIELAB under D50 (T.4 E.6.4), and T.42's codes: a = a* x 255/170 +
        // 128 puts a* = 0 at code 128, so code 0 is -85.33 and 255 is 84.67.
        assert!(dict.contains("/ColorSpace [/Lab << /WhitePoint [0.96422 1 0.82521] /Range [-86 85 -76 125] >>]"), "{dict}");
        let at = dict.find("/Decode [").unwrap() + 9;
        let decode: Vec<f64> =
            dict[at..dict[at..].find(']').unwrap() + at].split_whitespace().map(|n| n.parse().unwrap()).collect();
        for (got, want) in decode.iter().zip([0.0, 100.0, -85.333, 84.667, -75.294, 124.706]) {
            assert!((got - want).abs() < 0.001, "{decode:?}");
        }
        // The stream is the one that arrived, but for its length: Y filled
        // in and the DNL gone, which is the stream that said so at the start.
        let length = number(&dict, "/Length ") as usize;
        let data = find(&pdf, b"stream\n", body).unwrap() + 7;
        let known = crate::jpeg::encode(&picture, crate::jpeg::Options::default());
        assert_eq!(&pdf[data..data + length], known.as_slice());
        assert!(pdf[data + length..].starts_with(b"\nendstream"), "the length is wrong");
        // And the ink page beside it is still T.6.
        assert!(object(&pdf, 6).1.contains("/Filter /CCITTFaxDecode"));
    }

    #[test]
    fn a_greyscale_page_is_lightness_looked_up_as_lab() {
        // No stream with it: one is made.
        let picture = tone(1, 16);
        let pdf = document_of(&[Leaf::Tone(&picture)]);
        let (body, dict) = object(&pdf, 6);
        assert!(dict.contains("/ColorSpace [/Indexed [/Lab << /WhitePoint [0.96422 1 0.82521] /Range [-128 127 -128 127] >>] 255 <008080"), "{dict}");
        // 256 entries of three octets, the last L* = 100 with no colour.
        let table = &dict[dict.find('<').map(|_| dict.find(" 255 <").unwrap() + 6).unwrap()..];
        let table = &table[..table.find('>').unwrap()];
        assert_eq!(table.len(), 256 * 6);
        assert!(table.ends_with("FF8080"), "{}", &table[table.len() - 12..]);
        // The table does the decoding, so there is no Decode array.
        assert!(!dict.contains("/Decode ["), "{dict}");
        let length = number(&dict, "/Length ") as usize;
        let data = find(&pdf, b"stream\n", body).unwrap() + 7;
        let mut decoder = crate::jpeg::Decoder::new(WIDTH);
        decoder.feed(&pdf[data..data + length]);
        let back = decoder.picture().expect("the stream in the file does not decode");
        assert_eq!((back.width, back.height, back.components), (WIDTH, 16, 1));
    }

    #[test]
    fn the_picture_on_each_sheet_is_the_page_in_t6() {
        let pages = [page_of(1143, 0), page_of(400, 11)];
        let pdf = document(&[(&pages[0], Resolution::Standard), (&pages[1], Resolution::Fine)]);
        for (n, lines) in pages.iter().enumerate() {
            let (body, dict) = object(&pdf, 6 + 3 * n);
            assert!(dict.contains("/Filter /CCITTFaxDecode"), "{dict}");
            assert!(dict.contains("/K -1"), "{dict}");
            assert_eq!(number(&dict, "/Columns "), WIDTH as f64);
            assert_eq!(number(&dict, "/Rows "), lines.len() as f64);
            assert_eq!(number(&dict, "/Height "), lines.len() as f64);
            let length = number(&dict, "/Length ") as usize;
            let data = find(&pdf, b"stream\n", body).unwrap() + 7;
            assert!(pdf[data + length..].starts_with(b"\nendstream"), "the length is wrong");
            // Our own T.6 decoder reads it back as the page that went in.
            let mut decoder = mmr::Decoder::new(WIDTH);
            let bits: Vec<bool> = pdf[data..data + length]
                .iter()
                .flat_map(|byte| (0..8).map(move |b| byte >> (7 - b) & 1 == 1))
                .collect();
            decoder.feed_bits(&bits);
            assert!(decoder.is_done(), "no EOFB at the end of page {n}");
            assert_eq!(decoder.damaged(), 0);
            assert_eq!(decoder.lines(), lines.as_slice(), "page {n} came back different");
        }
    }
}
