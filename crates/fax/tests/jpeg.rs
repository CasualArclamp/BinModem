//! The JPEG of T.4 Annex E, against a JPEG implementation that is not ours.
//!
//! Two things are checked each way round. Streams made by the `image` crate's
//! encoder go through our decoder, and must come out as the components that
//! crate's own decoder gets from them. Streams made by our encoder go through
//! that decoder, and must come out as the components we put in. A JPEG stream
//! says nothing of what its components mean, so YCbCr from one side and
//! CIELAB from the other are the same octets to both: the comparison is of
//! components, and the colour arithmetic on each side is undone or left out.

use fax::jpeg::{self, Decoder, Options};
use fax::picture::{Gamut, Picture, lab_of_rgb};
use image::codecs::jpeg::{JpegDecoder, JpegEncoder};
use image::{ExtendedColorType, ImageDecoder};

/// A picture with edges, a ramp and some colour: something for every part of
/// the coding to do.
fn rgb_test_card(width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let ramp = (x * 255 / width.max(2).saturating_sub(1)) as u8;
            let band = y * 4 / height.max(1);
            let pel = match band {
                0 => [ramp, ramp, ramp],
                1 => [ramp, 255 - ramp, 128],
                2 if (x / 16 + y / 16) % 2 == 0 => [230, 40, 40],
                2 => [250, 250, 245],
                _ => [40, (y * 255 / height) as u8, 200 - ramp / 2],
            };
            out.extend_from_slice(&pel);
        }
    }
    out
}

/// The same card as CIELAB codes.
fn lab_test_card(width: usize, height: usize) -> Picture {
    let samples = rgb_test_card(width, height)
        .as_chunks::<3>().0.iter()
        .flat_map(|p| lab_of_rgb([p[0], p[1], p[2]]))
        .collect();
    Picture { width, height, components: 3, samples, pels_per_inch: 200, gamut: Gamut::DEFAULT, jpeg: Vec::new() }
}

/// ITU-R BT.601 as JFIF uses it, which is what the `image` crate's codec does
/// to and from RGB.
fn ycbcr_of_rgb(rgb: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (f32::from(rgb[0]), f32::from(rgb[1]), f32::from(rgb[2]));
    [
        0.299 * r + 0.587 * g + 0.114 * b,
        -0.168_736 * r - 0.331_264 * g + 0.5 * b + 128.0,
        0.5 * r - 0.418_688 * g - 0.081_312 * b + 128.0,
    ]
}

fn rgb_of_ycbcr(c: [u8; 3]) -> [f32; 3] {
    let (y, cb, cr) = (f32::from(c[0]), f32::from(c[1]) - 128.0, f32::from(c[2]) - 128.0);
    [y + 1.402 * cr, y - 0.344_136 * cb - 0.714_136 * cr, y + 1.772 * cb]
}

/// Their decoder's reading of a stream, as RGB or as grey.
fn their_decoding(stream: &[u8]) -> (usize, usize, Vec<u8>) {
    let decoder = JpegDecoder::new(std::io::Cursor::new(stream)).expect("their decoder refused the stream");
    let (width, height) = decoder.dimensions();
    let mut out = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut out).expect("their decoder could not read the stream");
    (width as usize, height as usize, out)
}

fn ours(stream: &[u8]) -> Picture {
    let mut decoder = Decoder::new(fax::page::WIDTH);
    decoder.feed(stream);
    assert!(decoder.is_done(), "no EOI");
    assert_eq!(decoder.damaged(), 0);
    decoder.picture().expect("no picture")
}

fn mean_and_worst(a: impl Iterator<Item = f32>, b: impl Iterator<Item = f32>) -> (f32, f32) {
    let (mut sum, mut worst, mut n) = (0.0f32, 0.0f32, 0u32);
    for (x, y) in a.zip(b) {
        let d = (x - y).abs();
        sum += d;
        worst = worst.max(d);
        n += 1;
    }
    (sum / n.max(1) as f32, worst)
}

#[test]
fn a_colour_stream_from_their_encoder_decodes_to_the_components_they_put_in() {
    // Sizes that are and are not whole MCUs.
    for (width, height) in [(64, 48), (61, 37), (200, 123)] {
        let rgb = rgb_test_card(width, height);
        let mut stream = Vec::new();
        JpegEncoder::new_with_quality(&mut stream, 95)
            .encode(&rgb, width as u32, height as u32, ExtendedColorType::Rgb8)
            .unwrap();
        let picture = ours(&stream);
        assert_eq!((picture.width, picture.height, picture.components), (width, height, 3));
        // Our components are their YCbCr. Against the source, within what
        // the quantisation and the chroma subsampling cost...
        let (mean, _) = mean_and_worst(
            picture.samples.iter().map(|s| f32::from(*s)),
            rgb.as_chunks::<3>().0.iter().flat_map(|p| ycbcr_of_rgb([p[0], p[1], p[2]])),
        );
        assert!(mean < 4.0, "{width} x {height}: components differ from the source by {mean} on average");
        // ...and against their own decoder's RGB, which had the same
        // coefficients to work from. Their chroma is interpolated and ours
        // is repeated, so edges of colour differ and flat areas do not.
        let (_, _, theirs) = their_decoding(&stream);
        let (mean, _) = mean_and_worst(
            picture.samples.as_chunks::<3>().0.iter().flat_map(|c| rgb_of_ycbcr([c[0], c[1], c[2]])),
            theirs.iter().map(|s| f32::from(*s)),
        );
        assert!(mean < 3.0, "{width} x {height}: differs from their decoding by {mean} on average");
    }
}

#[test]
fn a_grey_stream_from_their_encoder_decodes_to_what_their_decoder_gets() {
    let (width, height) = (150, 77);
    let grey: Vec<u8> = (0..width * height).map(|i| ((i % width) * 255 / width) as u8 ^ ((i / width) as u8 & 0x20)).collect();
    let mut stream = Vec::new();
    JpegEncoder::new_with_quality(&mut stream, 90)
        .encode(&grey, width as u32, height as u32, ExtendedColorType::L8)
        .unwrap();
    let picture = ours(&stream);
    assert_eq!((picture.width, picture.height, picture.components), (width, height, 1));
    let (_, _, theirs) = their_decoding(&stream);
    // One component and no colour arithmetic: the two decoders differ only
    // by how each rounds its inverse transform.
    let (mean, worst) = mean_and_worst(picture.samples.iter().map(|s| f32::from(*s)), theirs.iter().map(|s| f32::from(*s)));
    assert!(worst <= 2.0 && mean < 0.5, "differs from their decoding by {mean} on average, {worst} at worst");
}

#[test]
fn their_decoder_reads_our_streams_as_the_components_we_put_in() {
    for options in [
        Options::default(),
        Options { subsample: false, ..Options::default() },
        Options { restart_interval: 5, ..Options::default() },
        Options { quality: 30, ..Options::default() },
    ] {
        let picture = lab_test_card(120, 90);
        let stream = jpeg::encode(&picture, options);
        let (width, height, theirs) = their_decoding(&stream);
        assert_eq!((width, height), (120, 90), "{options:?}");
        // They took our L*, a*, b* for Y, Cb, Cr and made RGB of them: put
        // that back and it is our components again.
        // RGB cannot hold every YCbCr triple, and CIELAB codes read as YCbCr
        // go outside it in places, where their decoder clips: those pels are
        // left out of the comparison.
        let kept: Vec<(&[u8; 3], &[u8; 3])> = theirs
            .as_chunks::<3>().0.iter()
            .zip(picture.samples.as_chunks::<3>().0)
            .filter(|(rgb, _)| rgb.iter().all(|v| (1..255).contains(v)))
            .collect();
        assert!(kept.len() > 120 * 90 / 4, "{options:?}: only {} pels were not clipped", kept.len());
        let (mean, _) = mean_and_worst(
            kept.iter().flat_map(|(p, _)| ycbcr_of_rgb([p[0], p[1], p[2]])),
            kept.iter().flat_map(|(_, lab)| lab.iter().map(|s| f32::from(*s))),
        );
        assert!(mean < 5.0, "{options:?}: their reading differs by {mean} on average");
        // Ours has no such trouble.
        let back = ours(&stream);
        let (mean, _) = mean_and_worst(back.samples.iter().map(|s| f32::from(*s)), picture.samples.iter().map(|s| f32::from(*s)));
        assert!(mean < 4.0, "{options:?}: our reading differs by {mean} on average");
    }
}

#[test]
fn their_decoder_reads_our_grey_stream() {
    let (width, height) = (96, 64);
    let samples: Vec<u8> = (0..width * height).map(|i| ((i % width) * 2 + (i / width)) as u8).collect();
    let picture = Picture { width, height, components: 1, samples, pels_per_inch: 200, ..Picture::default() };
    let stream = jpeg::encode(&picture, Options { quality: 90, ..Options::default() });
    let (w, h, theirs) = their_decoding(&stream);
    assert_eq!((w, h, theirs.len()), (width, height, width * height));
    let (mean, worst) = mean_and_worst(theirs.iter().map(|s| f32::from(*s)), picture.samples.iter().map(|s| f32::from(*s)));
    assert!(mean < 1.5 && worst < 12.0, "their reading differs by {mean} on average, {worst} at worst");
}

#[test]
fn the_stream_starts_as_t4_annex_e_says() {
    // T.4 E.6.5: "X'FFD8', X'FFE1', X'000C', X'47', X'33', X'46', X'41',
    // X'58', X'00', X'07CA', X'00C8'".
    let stream = jpeg::encode(&lab_test_card(16, 16), Options::default());
    assert_eq!(
        stream[..16],
        [0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x0C, 0x47, 0x33, 0x46, 0x41, 0x58, 0x00, 0x07, 0xCA, 0x00, 0xC8]
    );
    assert_eq!(stream[stream.len() - 2..], [0xFF, 0xD9]);
}

#[test]
fn a_page_whose_length_comes_at_the_end_is_as_long_as_the_dnl_says() {
    // T.4 E.6.2: Y = 0 in the frame header, and DNL after the scan.
    for (height, restart_interval) in [(40, 0), (37, 0), (37, 3), (16, 1)] {
        let picture = lab_test_card(72, height);
        let options = Options { lines_at_the_end: true, restart_interval, ..Options::default() };
        let stream = jpeg::encode(&picture, options);
        let known = jpeg::encode(&picture, Options { lines_at_the_end: false, ..options });
        let back = ours(&stream);
        assert_eq!(back.height, height, "restart {restart_interval}");
        assert_eq!(back.samples, ours(&known).samples, "the same page either way");
        // And for a file, Y is filled in and the DNL is gone, so that
        // anything reads it: the stream that said its length at the start.
        let filed = jpeg::for_a_file(&back.jpeg, back.height);
        let (_, h, _) = their_decoding(&filed);
        assert_eq!(h, height);
        assert_eq!(filed, known);
        assert_eq!(jpeg::for_a_file(&known, height), known, "a stream that says its length is left alone");
    }
}

#[test]
fn a_page_arrives_by_the_row_and_comes_to_the_same_page() {
    let picture = lab_test_card(160, 120);
    for options in [Options::default(), Options { lines_at_the_end: true, restart_interval: 4, ..Options::default() }] {
        let stream = jpeg::encode(&picture, options);
        let whole = ours(&stream);
        // Bits off the line, each octet least significant bit first (T.4
        // E.6.9), 37 octets at a time, and zeros after EOI (E.6.1).
        let mut decoder = Decoder::new(160);
        let mut heights = Vec::new();
        let mut padded = stream.clone();
        padded.extend_from_slice(&[0; 9]);
        for chunk in padded.chunks(37) {
            let bits: Vec<bool> = chunk.iter().flat_map(|o| (0..8).map(move |i| o >> i & 1 == 1)).collect();
            decoder.feed_bits(&bits);
            if heights.last() != Some(&decoder.lines().len()) {
                heights.push(decoder.lines().len());
            }
            // What has come so far is already the top of the page. Until a
            // DNL says where the page ends, the last MCU row counts whole.
            let part = decoder.part(0, decoder.height()).map(|p| p.samples).unwrap_or_default();
            let shared = part.len().min(whole.samples.len());
            assert_eq!(part[..shared], whole.samples[..shared]);
        }
        assert!(decoder.is_done());
        assert!(heights.len() > 4, "{options:?}: seen at {heights:?}");
        assert!(heights.windows(2).all(|w| w[0] < w[1]));
        let got = decoder.picture().unwrap();
        assert_eq!(got.samples, whole.samples);
        assert_eq!(got.jpeg, stream, "the stream is kept as it came, without the padding");
        assert_eq!(decoder.lines().len(), 120);
        assert!(decoder.lines().iter().all(|l| l.len() == 160));
    }
}

#[test]
fn the_one_bit_preview_is_dark_where_the_page_is() {
    let (width, height) = (64, 32);
    let samples = (0..width * height).map(|i| if i % width < 32 { 0 } else { 255 }).collect();
    let picture = Picture { width, height, components: 1, samples, pels_per_inch: 200, ..Picture::default() };
    // A preview wider than the picture is paper past its edge.
    let mut decoder = Decoder::new(80);
    decoder.feed(&jpeg::encode(&picture, Options { quality: 95, ..Options::default() }));
    for line in decoder.lines() {
        assert_eq!(line.len(), 80);
        assert!(line[..30].iter().all(|p| *p), "black is ink");
        assert!(line[34..].iter().all(|p| !*p), "white is paper");
    }
}

#[test]
fn the_gamut_and_the_resolution_come_from_the_app1_markers() {
    let mut picture = lab_test_card(16, 16);
    picture.gamut = Gamut { ranges: [(0, 100), (128, 255), (128, 255)] };
    picture.pels_per_inch = 300;
    let back = ours(&jpeg::encode(&picture, Options::default()));
    assert_eq!(back.gamut, picture.gamut);
    assert_eq!(back.pels_per_inch, 300);
    // And without a G3FAX1 marker, the default range of T.4 E.6.4.
    assert_eq!(ours(&jpeg::encode(&lab_test_card(16, 16), Options::default())).gamut, Gamut::DEFAULT);
}

#[test]
fn a_stream_that_is_not_a_fax_is_refused_and_not_decoded_wrong() {
    let stream = jpeg::encode(&lab_test_card(32, 32), Options::default());
    // A progressive frame: SOF2 in place of SOF0.
    let mut progressive = stream.clone();
    let sof = progressive.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    progressive[sof + 1] = 0xC2;
    let mut decoder = Decoder::new(32);
    decoder.feed(&progressive);
    assert!(decoder.is_done() && decoder.damaged() == 1 && decoder.picture().is_none());
    // Twelve bits a sample, which was not offered.
    let mut deep = stream.clone();
    deep[sof + 4] = 12;
    let mut decoder = Decoder::new(32);
    decoder.feed(&deep);
    assert!(decoder.is_done() && decoder.damaged() == 1);
    // Cut short in the scan: what came is kept, and nothing is invented.
    let mut decoder = Decoder::new(32);
    decoder.feed(&stream[..stream.len() - 40]);
    assert!(!decoder.is_done());
    assert!(decoder.height() < 32);
    // Noise in the scan must not panic, whatever it decodes to.
    let mut seed = 99u32;
    for _ in 0..200 {
        let mut noisy = stream.clone();
        for _ in 0..3 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let at = (seed >> 8) as usize % noisy.len();
            noisy[at] ^= 1 << (seed & 7);
        }
        let mut decoder = Decoder::new(32);
        decoder.feed(&noisy);
        let _ = decoder.picture();
    }
}
