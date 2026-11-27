//! Make a colour fax page and write it out every way it can go: the JPEG
//! data stream of T.4 Annex E as it would go down the line, and a PDF with
//! that stream in it as a CIELAB picture.
//!
//! `cargo run -p fax@5.6.0 --example colour_page -- out-dir`
//!
//! For looking at what the colour path makes with something that is not
//! ours: any JPEG viewer shows the stream with its colours wrong, since it
//! takes CIELAB for YCbCr, and any PDF viewer shows the PDF with them right.

use fax::picture::{Picture, lab_of_rgb};
use fax::{jpeg, pdf};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_owned());
    let dir = std::path::Path::new(&dir);
    let (width, height) = (fax::page::WIDTH, 600);
    let mut samples = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let ramp = (x * 255 / (width - 1)) as u8;
            // Eight bars of colour, a ramp of grey under them, and rules.
            let rgb = if y % 100 < 3 {
                [0, 0, 0]
            } else if y < 300 {
                match x * 8 / width {
                    0 => [255, 255, 255],
                    1 => [230, 220, 60],
                    2 => [60, 190, 200],
                    3 => [70, 170, 80],
                    4 => [190, 70, 170],
                    5 => [200, 50, 45],
                    6 => [45, 70, 170],
                    _ => [20, 20, 20],
                }
            } else if y < 450 {
                [ramp, ramp, ramp]
            } else {
                [ramp, 128, 255 - ramp]
            };
            samples.extend_from_slice(&lab_of_rgb(rgb));
        }
    }
    let mut picture = Picture { width, height, components: 3, samples, pels_per_inch: 200, ..Picture::default() };
    picture.jpeg = jpeg::encode(&picture, jpeg::Options::default());
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        match std::fs::write(&path, bytes) {
            Ok(()) => println!("{} ({} octets)", path.display(), bytes.len()),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
    };
    write("colour-page.jpg", &picture.jpeg);
    write("colour-page.pdf", &pdf::document_of(&[pdf::Leaf::Tone(&picture)]));
    let grey = Picture {
        components: 1,
        samples: picture.samples.iter().step_by(3).copied().collect(),
        jpeg: Vec::new(),
        ..picture.clone()
    };
    write("grey-page.pdf", &pdf::document_of(&[pdf::Leaf::Tone(&grey)]));
    // And as plain sRGB, to hold the others against: a PPM, which needs no
    // coder to write.
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    ppm.extend_from_slice(&picture.rgb());
    write("colour-page.ppm", &ppm);
}
