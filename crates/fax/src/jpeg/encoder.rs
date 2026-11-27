//! Writing a T.4 Annex E page: the least that is one.
//!
//! The stream of T.4 E.6.2.1, marker for marker: SOI, the `G3FAX` APP1, the
//! Huffman tables of T.81 Annex K, its two quantisation tables scaled, a
//! baseline frame, one interleaved scan and EOI. Nothing is fitted to the
//! picture. It is here so that a page can be sent in a test and across a
//! loopback, and so that the decoder has something made to the letter of the
//! annex to read.

use super::{
    G3FAX, G3FAX_VERSION, HuffmanSpec, QUANT_CHROMINANCE, QUANT_LUMINANCE, ZIGZAG, ac_chrominance,
    ac_luminance, dc_chrominance, dc_luminance, dct, marker,
};
use crate::picture::Picture;

/// How a picture is coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// 1 to 100: how the tables of T.81 Annex K are scaled, by the rule the
    /// Independent JPEG Group's software made usual. 50 is the tables as
    /// printed.
    pub quality: u8,
    /// MCUs to a restart interval, or zero for no restart markers (DRI).
    pub restart_interval: u16,
    /// Write Y = 0 in the frame header and the number of lines in a DNL
    /// marker after the scan, as a machine that scans while it sends does
    /// (T.4 E.6.2).
    pub lines_at_the_end: bool,
    /// One a* and one b* sample to four of L*: the default, which T.30 calls
    /// 4:1:1 (Table 2 Note 38). Off is no subsampling, DCS bit 73.
    pub subsample: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { quality: 75, restart_interval: 0, lines_at_the_end: false, subsample: true }
    }
}

/// A quantisation table of Annex K scaled for a quality, in row-major order.
fn scaled(table: &[u8; 64], quality: u8) -> [u8; 64] {
    // The scaling of the Independent JPEG Group's `jpeg_quality_scaling`:
    // 5000/Q percent below 50 and 200 - 2Q percent from there up.
    let quality = u32::from(quality.clamp(1, 100));
    let percent = if quality < 50 { 5000 / quality } else { 200 - 2 * quality };
    table.map(|q| ((u32::from(q) * percent + 50) / 100).clamp(1, 255) as u8)
}

/// One component's samples at the sampling it is coded at.
struct Plane {
    width: usize,
    height: usize,
    samples: Vec<u8>,
}

impl Plane {
    /// An 8 x 8 block, with the edge of the picture repeated past it
    /// (A.2.4 leaves what completes a block to the encoder).
    fn block(&self, left: usize, top: usize) -> [u8; 64] {
        std::array::from_fn(|i| {
            let x = (left + i % 8).min(self.width - 1);
            let y = (top + i / 8).min(self.height - 1);
            self.samples[y * self.width + x]
        })
    }
}

/// The entropy-coded segment being written (F.1.2.3).
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u32,
    held: u8,
}

impl Bits {
    fn put(&mut self, code: u16, length: u8) {
        self.acc = self.acc << length | u32::from(code);
        self.held += length;
        while self.held >= 8 {
            self.held -= 8;
            let octet = (self.acc >> self.held) as u8;
            self.out.push(octet);
            if octet == 0xFF {
                // B.1.1.5: a zero octet after every X'FF' of data.
                self.out.push(0);
            }
        }
        self.acc &= (1 << self.held) - 1;
    }

    /// Fill the last octet with ones (F.1.2.3).
    fn pad(&mut self) {
        if self.held > 0 {
            let fill = 8 - self.held;
            self.put((1u16 << fill) - 1, fill);
        }
    }
}

/// The size category of a value and the bits that follow its code
/// (F.1.2.1.1, Table F.1; F.1.2.2.1).
fn category(value: i32) -> (u8, u16) {
    let size = (32 - value.unsigned_abs().leading_zeros()) as u8;
    let bits = if value < 0 { value - 1 } else { value };
    (size, (bits & ((1 << size) - 1)) as u16)
}

fn segment(out: &mut Vec<u8>, code: u8, body: &[u8]) {
    out.extend_from_slice(&[0xFF, code]);
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
}

/// A picture as the JPEG data stream of T.4 Annex E.
///
/// The picture's samples are taken as they are: L*, or L*, a* and b*, in
/// T.42's codes. A picture with no pels gives an empty stream.
pub fn encode(picture: &Picture, options: Options) -> Vec<u8> {
    encode_with_lines(picture, options).0
}

/// A picture as the JPEG data stream of T.4 Annex E, and where each line of
/// it ends in that stream: `ends[n]` is how many octets a receiver has taken
/// in when line `n` can be decoded. Every line of an MCU row ends where the
/// row does, since a row is decoded whole.
pub fn encode_with_lines(picture: &Picture, options: Options) -> (Vec<u8>, Vec<usize>) {
    let (width, height) = (picture.width, picture.height);
    if width == 0 || height == 0 || !matches!(picture.components, 1 | 3) {
        return (Vec::new(), Vec::new());
    }
    let colour = picture.components == 3;
    let factor = if colour && options.subsample { 2 } else { 1 };

    // The planes: L* whole, and a* and b* as the mean of the four values at
    // the lightness positions where they are subsampled (T.4 E.6.3).
    let mut planes = Vec::with_capacity(picture.components);
    for c in 0..picture.components {
        let f = if c == 0 { 1 } else { factor };
        let (w, h) = (width.div_ceil(f), height.div_ceil(f));
        let mut samples = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (mut sum, mut n) = (0u32, 0u32);
                for yy in (y * f..(y + 1) * f).take_while(|yy| *yy < height) {
                    for xx in (x * f..(x + 1) * f).take_while(|xx| *xx < width) {
                        sum += u32::from(picture.samples[(yy * width + xx) * picture.components + c]);
                        n += 1;
                    }
                }
                samples.push(((sum + n / 2) / n) as u8);
            }
        }
        planes.push(Plane { width: w, height: h, samples });
    }

    let quant = [scaled(&QUANT_LUMINANCE, options.quality), scaled(&QUANT_CHROMINANCE, options.quality)];
    let specs: [HuffmanSpec; 4] = [dc_luminance(), ac_luminance(), dc_chrominance(), ac_chrominance()];

    let mut out = vec![0xFF, marker::SOI];
    // E.6.5: "G3FAX", X'00', the version and the resolution.
    let mut app = G3FAX.to_vec();
    app.push(0);
    app.extend_from_slice(&G3FAX_VERSION.to_be_bytes());
    app.extend_from_slice(&picture.pels_per_inch.to_be_bytes());
    segment(&mut out, marker::APP1, &app);
    // E.6.6: the gamut range, only where it is not the default.
    if picture.gamut != crate::picture::Gamut::DEFAULT {
        let mut app = G3FAX.to_vec();
        app.push(1);
        for (offset, range) in picture.gamut.ranges {
            app.extend_from_slice(&offset.to_be_bytes());
            app.extend_from_slice(&range.to_be_bytes());
        }
        segment(&mut out, marker::APP1, &app);
    }
    // DHT (B.2.4.2): class and destination, the counts, the values. Tables
    // 0 for L* and 1 for a* and b* (T.4 E.6.2.1).
    let tables = if colour { 4 } else { 2 };
    let mut dht = Vec::new();
    for (i, spec) in specs.iter().enumerate().take(tables) {
        dht.push(((i % 2) as u8) << 4 | (i / 2) as u8);
        dht.extend_from_slice(&spec.bits);
        dht.extend_from_slice(&spec.values);
    }
    segment(&mut out, marker::DHT, &dht);
    // DQT (B.2.4.1): the elements in zig-zag order.
    let mut dqt = Vec::new();
    for (i, table) in quant.iter().enumerate().take(if colour { 2 } else { 1 }) {
        dqt.push(i as u8);
        dqt.extend(ZIGZAG.iter().map(|&at| table[usize::from(at)]));
    }
    segment(&mut out, marker::DQT, &dqt);
    if options.restart_interval > 0 {
        segment(&mut out, marker::DRI, &options.restart_interval.to_be_bytes());
    }
    // SOF0 (B.2.2): P = 8, Y, X, and the components 0, 1 and 2.
    let said = if options.lines_at_the_end { 0 } else { height as u16 };
    let mut sof = vec![8];
    sof.extend_from_slice(&said.to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(picture.components as u8);
    for c in 0..picture.components {
        let sampling = if c == 0 && colour { factor as u8 } else { 1 };
        sof.extend_from_slice(&[c as u8, sampling << 4 | sampling, u8::from(c > 0)]);
    }
    segment(&mut out, marker::SOF0, &sof);
    // SOS (B.2.3): Ss = 0, Se = 63, Ah = Al = 0.
    let mut sos = vec![picture.components as u8];
    for c in 0..picture.components {
        let table = u8::from(c > 0);
        sos.extend_from_slice(&[c as u8, table << 4 | table]);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    segment(&mut out, marker::SOS, &sos);

    // The scan (A.2.3): an MCU is `factor x factor` blocks of L* and one
    // each of a* and b*.
    let codes: Vec<[(u16, u8); 256]> = specs.iter().map(HuffmanSpec::codes).collect();
    let mcu = 8 * factor;
    let (across, down) = (width.div_ceil(mcu), height.div_ceil(mcu));
    let mut bits = Bits::default();
    let mut predictions = [0i32; 3];
    let mut coefficients = [0.0f32; 64];
    let mut count = 0usize;
    let mut ends = Vec::with_capacity(height);
    for my in 0..down {
        // The row before this one is whole once the octet its last bits
        // are in has gone, which it has not yet: one more, and one more
        // again in case that octet is X'FF' and has a zero stuffed after it.
        if my > 0 {
            let lines = (my * mcu).min(height) - ends.len();
            ends.extend(std::iter::repeat_n(out.len() + bits.out.len() + 2, lines));
        }
        for mx in 0..across {
            let interval = usize::from(options.restart_interval);
            if interval > 0 && count > 0 && count.is_multiple_of(interval) {
                // E.1.4: pad, RSTm with m counting 0 to 7 round, and the
                // predictions back to zero.
                bits.pad();
                bits.out.extend_from_slice(&[0xFF, marker::RST0 + ((count / interval - 1) % 8) as u8]);
                predictions = [0; 3];
            }
            count += 1;
            for (c, plane) in planes.iter().enumerate() {
                let f = if c == 0 { factor } else { 1 };
                let table = usize::from(c > 0);
                for part in 0..f * f {
                    let samples = plane.block((mx * f + part % f) * 8, (my * f + part / f) * 8);
                    dct::forward(&samples, &mut coefficients);
                    let level = |k: usize| {
                        let at = usize::from(ZIGZAG[k]);
                        (coefficients[at] / f32::from(quant[table][at])).round() as i32
                    };
                    // F.1.2.1: the difference of the DC term.
                    let dc = level(0);
                    let (size, extra) = category(dc - predictions[c]);
                    predictions[c] = dc;
                    let (code, length) = codes[2 * table][usize::from(size)];
                    bits.put(code, length);
                    bits.put(extra, size);
                    // F.1.2.2: run and size, ZRL for sixteen zeros, EOB.
                    let mut run = 0u8;
                    for k in 1..64 {
                        // An AC term of eight-bit samples has at most ten
                        // bits (Table F.2).
                        let value = level(k).clamp(-1023, 1023);
                        if value == 0 {
                            run += 1;
                            continue;
                        }
                        while run > 15 {
                            let (code, length) = codes[2 * table + 1][0xF0];
                            bits.put(code, length);
                            run -= 16;
                        }
                        let (size, extra) = category(value);
                        let (code, length) = codes[2 * table + 1][usize::from(run << 4 | size)];
                        bits.put(code, length);
                        bits.put(extra, size);
                        run = 0;
                    }
                    if run > 0 {
                        let (code, length) = codes[2 * table + 1][0x00];
                        bits.put(code, length);
                    }
                }
            }
        }
    }
    bits.pad();
    out.append(&mut bits.out);
    if options.lines_at_the_end {
        // DNL (B.2.5).
        segment(&mut out, marker::DNL, &(height as u16).to_be_bytes());
    }
    out.extend_from_slice(&[0xFF, marker::EOI]);
    // The last row is whole, and its length known, at the end of the stream.
    ends.resize(height, out.len());
    (out, ends)
}
