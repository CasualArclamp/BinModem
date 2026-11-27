//! A page with tone in it: greyscale or colour, as T.4 Annex E sends one.
//!
//! [`Page`](crate::page::Page) is one bit a pel. This is eight bits a
//! component, and one or three components: L* alone for a greyscale page, and
//! L*, a* and b* for a colour one. The samples stay in the codes T.42 gives
//! them, because that is what arrived and what a PDF can carry as it is. The
//! change to sRGB is here too, for a screen or a PNG.

/// How the 8-bit codes map onto CIELAB: an offset and a range for each of
/// L*, a* and b* (T.4 E.6.6).
///
/// `N = (255 / Q) x n* + P`, so `n* = (N - P) x Q / 255`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gamut {
    /// (P, Q) for L*, a* and b*.
    pub ranges: [(i16, i16); 3],
}

impl Gamut {
    /// The default gamut range (T.42, and T.4 E.6.4): L* = [0, 100],
    /// a* = [-85, 85], b* = [-75, 125], so that L = L* x 255/100,
    /// a = a* x 255/170 + 128 and b = b* x 255/200 + 96. These are the six
    /// integers of the example in T.4 E.6.6.
    pub const DEFAULT: Self = Self {
        ranges: [(0, 100), (128, 170), (96, 200)],
    };

    /// The CIELAB value of a code, for component 0, 1 or 2.
    pub fn value(&self, component: usize, code: u8) -> f32 {
        let (offset, range) = self.ranges[component];
        (f32::from(code) - f32::from(offset)) * f32::from(range) / 255.0
    }

    /// The code of a CIELAB value: rounded, and "truncated to 0 or 255 as
    /// appropriate" (T.4 E.6.4).
    pub fn code(&self, component: usize, value: f32) -> u8 {
        let (offset, range) = self.ranges[component];
        (value * 255.0 / f32::from(range) + f32::from(offset)).round().clamp(0.0, 255.0) as u8
    }
}

impl Default for Gamut {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A greyscale or colour page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// 1 for L* alone, 3 for L*, a* and b*.
    pub components: usize,
    /// Row-major, `components` codes a pel: `width * height * components`.
    pub samples: Vec<u8>,
    /// Pels to 25.4 mm, the same across and down (T.4 E.6.5).
    pub pels_per_inch: u16,
    pub gamut: Gamut,
    /// The JPEG data stream as it arrived, from SOI to EOI, if this came off
    /// a line. See [`crate::jpeg::for_a_file`] before it goes in a file.
    pub jpeg: Vec<u8>,
}

impl Picture {
    /// Whether it has a* and b* as well as L*.
    pub fn is_colour(&self) -> bool {
        self.components == 3
    }

    /// The CIELAB codes of one pel: a greyscale pel has the codes for
    /// a* = b* = 0.
    pub fn lab(&self, x: usize, y: usize) -> [u8; 3] {
        let at = (y * self.width + x) * self.components;
        if self.components == 3 {
            [self.samples[at], self.samples[at + 1], self.samples[at + 2]]
        } else {
            [self.samples[at], self.gamut.code(1, 0.0), self.gamut.code(2, 0.0)]
        }
    }

    /// Rows `from..to` as sRGB, three octets a pel.
    pub fn rgb_rows(&self, from: usize, to: usize) -> Vec<u8> {
        let to = to.min(self.height);
        let from = from.min(to);
        let converter = Converter::new(self.gamut);
        let mut out = Vec::with_capacity((to - from) * self.width * 3);
        let row = self.width * self.components;
        for y in from..to {
            let line = &self.samples[y * row..(y + 1) * row];
            if self.components == 3 {
                for pel in line.as_chunks::<3>().0 {
                    out.extend_from_slice(&converter.rgb([pel[0], pel[1], pel[2]]));
                }
            } else {
                for &l in line {
                    out.extend_from_slice(&converter.grey(l));
                }
            }
        }
        out
    }

    /// The whole picture as sRGB, three octets a pel.
    pub fn rgb(&self) -> Vec<u8> {
        self.rgb_rows(0, self.height)
    }

    /// A picture from sRGB, three octets a pel and `width` pels across, in
    /// the default gamut and at the basic resolution of 200 pels to 25.4 mm
    /// (T.30 Table 2 Note 25).
    pub fn of_rgb(width: usize, rgb: &[u8]) -> Self {
        let (pels, _) = rgb.as_chunks::<3>();
        // A page is mostly runs of one colour, paper above all, and the
        // conversion is a cube root and a power for each of three: the last
        // answer is kept for a pel that is the same as the one before it.
        let mut last = ([255u8; 3], lab_of_rgb([255; 3]));
        let mut samples = Vec::with_capacity(pels.len() * 3);
        for pel in pels {
            if *pel != last.0 {
                last = (*pel, lab_of_rgb(*pel));
            }
            samples.extend_from_slice(&last.1);
        }
        Self {
            width,
            height: pels.len().checked_div(width).unwrap_or(0),
            components: 3,
            samples,
            pels_per_inch: 200,
            ..Self::default()
        }
    }

    /// The picture as its lightness alone: L* and nothing else, which is
    /// what a greyscale page is (T.4 E.6.2.2). Without the stream, which was
    /// of all three.
    pub fn lightness(&self) -> Self {
        Self {
            components: 1,
            samples: self.samples.iter().step_by(self.components.max(1)).copied().collect(),
            jpeg: Vec::new(),
            ..self.clone()
        }
    }

    /// Millimetres of paper across and down.
    pub fn size_mm(&self) -> (f64, f64) {
        let per_mm = f64::from(self.pels_per_inch.max(1)) / 25.4;
        (self.width as f64 / per_mm, self.height as f64 / per_mm)
    }
}

/// The D50 white point, with Y = 1 (T.4 E.6.4: X0 = 96.422, Y0 = 100.000,
/// Z0 = 82.521).
pub const WHITE_D50: [f32; 3] = [0.964_22, 1.0, 0.825_21];

/// XYZ under D50 to linear sRGB, which is under D65: the sRGB matrix of
/// IEC 61966-2-1 after a Bradford chromatic adaptation from D50 to D65.
/// The combined matrix is the one Bruce Lindbloom publishes for sRGB with a
/// D50 reference white (brucelindbloom.com, "RGB/XYZ Matrices").
const XYZ_D50_TO_SRGB: [[f32; 3]; 3] = [
    [3.133_856, -1.616_866_7, -0.490_614_6],
    [-0.978_768_4, 1.916_141_5, 0.033_454_0],
    [0.071_945_3, -0.228_991_4, 1.405_242_7],
];

/// The inverse of CIELAB's f: t cubed above 6/29, and the line below it
/// (CIE 15).
fn inverse(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA { t * t * t } else { 3.0 * DELTA * DELTA * (t - 4.0 / 29.0) }
}

/// CIELAB codes to sRGB, with the tables that make a page of it quick.
#[derive(Debug, Clone)]
pub struct Converter {
    /// f(Y/Yn) for each L code, and the a* and b* terms for theirs.
    fy: [f32; 256],
    fa: [f32; 256],
    fb: [f32; 256],
    /// Linear light, 0 to 1 in 4096 steps, as an sRGB octet.
    curve: Vec<u8>,
    /// Each L code alone, as a grey.
    greys: [u8; 256],
}

impl Converter {
    pub fn new(gamut: Gamut) -> Self {
        let mut fy = [0.0; 256];
        let mut fa = [0.0; 256];
        let mut fb = [0.0; 256];
        for code in 0..=255u8 {
            let at = usize::from(code);
            // CIE 15: L* = 116 f(Y/Yn) - 16, a* = 500 (f(X/Xn) - f(Y/Yn)),
            // b* = 200 (f(Y/Yn) - f(Z/Zn)).
            fy[at] = (gamut.value(0, code).clamp(0.0, 100.0) + 16.0) / 116.0;
            fa[at] = gamut.value(1, code) / 500.0;
            fb[at] = gamut.value(2, code) / 200.0;
        }
        let curve: Vec<u8> = (0..4096)
            .map(|i| {
                let linear = i as f32 / 4095.0;
                // IEC 61966-2-1: the sRGB transfer function.
                let v = if linear <= 0.003_130_8 {
                    12.92 * linear
                } else {
                    1.055 * linear.powf(1.0 / 2.4) - 0.055
                };
                (v * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect();
        // A grey has a* = b* = 0, so X/Xn = Y/Yn = Z/Zn: the white of D50
        // goes to the white of D65, and the grey is the luminance alone.
        let mut greys = [0u8; 256];
        for (grey, f) in greys.iter_mut().zip(&fy) {
            *grey = curve[(inverse(*f).clamp(0.0, 1.0) * 4095.0).round() as usize];
        }
        Self { fy, fa, fb, curve, greys }
    }

    /// One pel of L*, a*, b* codes as sRGB.
    pub fn rgb(&self, lab: [u8; 3]) -> [u8; 3] {
        let fy = self.fy[usize::from(lab[0])];
        let x = WHITE_D50[0] * inverse(fy + self.fa[usize::from(lab[1])]);
        let y = WHITE_D50[1] * inverse(fy);
        let z = WHITE_D50[2] * inverse(fy - self.fb[usize::from(lab[2])]);
        let mut out = [0u8; 3];
        for (channel, row) in out.iter_mut().zip(&XYZ_D50_TO_SRGB) {
            let linear = (row[0] * x + row[1] * y + row[2] * z).clamp(0.0, 1.0);
            *channel = self.curve[(linear * 4095.0).round() as usize];
        }
        out
    }

    /// One pel of L* alone as sRGB.
    pub fn grey(&self, l: u8) -> [u8; 3] {
        [self.greys[usize::from(l)]; 3]
    }
}

/// sRGB to CIELAB codes in the default gamut: the reverse of [`Converter`],
/// for making a picture to send or to test with.
pub fn lab_of_rgb(rgb: [u8; 3]) -> [u8; 3] {
    fn linear(v: u8) -> f32 {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    }
    fn f(t: f32) -> f32 {
        const DELTA: f32 = 6.0 / 29.0;
        if t > DELTA * DELTA * DELTA { t.cbrt() } else { t / (3.0 * DELTA * DELTA) + 4.0 / 29.0 }
    }
    // Linear sRGB to XYZ under D50: the inverse of `XYZ_D50_TO_SRGB`, from
    // the same source.
    const SRGB_TO_XYZ_D50: [[f32; 3]; 3] = [
        [0.436_074_7, 0.385_064_9, 0.143_080_4],
        [0.222_504_5, 0.716_878_6, 0.060_616_9],
        [0.013_932_2, 0.097_104_5, 0.714_173_3],
    ];
    let (r, g, b) = (linear(rgb[0]), linear(rgb[1]), linear(rgb[2]));
    let mut xyz = [0.0f32; 3];
    for (value, row) in xyz.iter_mut().zip(&SRGB_TO_XYZ_D50) {
        *value = row[0] * r + row[1] * g + row[2] * b;
    }
    let (fx, fy, fz) = (f(xyz[0] / WHITE_D50[0]), f(xyz[1] / WHITE_D50[1]), f(xyz[2] / WHITE_D50[2]));
    let gamut = Gamut::DEFAULT;
    [
        gamut.code(0, 116.0 * fy - 16.0),
        gamut.code(1, 500.0 * (fx - fy)),
        gamut.code(2, 200.0 * (fy - fz)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_gamut_is_the_codes_t4_gives() {
        // T.4 E.6.4: L = L* x 255/100, a = a* x 255/170 + 128,
        // b = b* x 255/200 + 96.
        let gamut = Gamut::DEFAULT;
        assert_eq!(gamut.code(0, 100.0), 255);
        assert_eq!(gamut.code(0, 0.0), 0);
        assert_eq!(gamut.code(1, 0.0), 128);
        assert_eq!(gamut.code(2, 0.0), 96);
        assert_eq!(gamut.code(1, -85.0), 1); // 0.5, to the nearest integer.
        assert_eq!(gamut.code(1, 200.0), 255, "outside the range is truncated");
        assert!((gamut.value(2, 255) - 124.7).abs() < 0.1);
    }

    #[test]
    fn white_is_white_and_black_is_black_and_grey_has_no_colour() {
        let converter = Converter::new(Gamut::DEFAULT);
        assert_eq!(converter.rgb([255, 128, 96]), [255, 255, 255]);
        assert_eq!(converter.rgb([0, 128, 96]), [0, 0, 0]);
        // L* = 50 is the sRGB grey 119.
        let mid = converter.grey(128);
        assert!(mid[0] == mid[1] && mid[1] == mid[2]);
        assert!((i32::from(mid[0]) - 119).abs() <= 1, "{mid:?}");
        for l in 0..=255 {
            let [r, g, b] = converter.rgb([l, 128, 96]);
            assert!(r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1, "L code {l}: {r} {g} {b}");
        }
    }

    #[test]
    fn rgb_goes_to_lab_and_comes_back() {
        let converter = Converter::new(Gamut::DEFAULT);
        // The primaries have b* or a* past the default gamut and are
        // truncated, so the test colours are less saturated ones.
        for rgb in [[200, 60, 50], [40, 120, 60], [50, 80, 160], [240, 220, 40], [128, 128, 128], [255, 255, 255], [12, 12, 12]] {
            let back = converter.rgb(lab_of_rgb(rgb));
            for c in 0..3 {
                assert!(back[c].abs_diff(rgb[c]) <= 4, "{rgb:?} came back {back:?}");
            }
        }
        // Known value: sRGB red is L* 54.3, a* 80.8, b* 69.9 under D50.
        let red = lab_of_rgb([255, 0, 0]);
        let gamut = Gamut::DEFAULT;
        assert!((gamut.value(0, red[0]) - 54.3).abs() < 0.6, "{red:?}");
        assert!((gamut.value(1, red[1]) - 80.8).abs() < 0.8, "{red:?}");
        assert!((gamut.value(2, red[2]) - 69.9).abs() < 0.8, "{red:?}");
    }

    #[test]
    fn a_picture_is_made_from_rgb_and_its_lightness_taken() {
        let picture = Picture::of_rgb(2, &[255, 255, 255, 0, 0, 0, 200, 60, 50, 128, 128, 128]);
        assert_eq!((picture.width, picture.height, picture.components), (2, 2, 3));
        assert_eq!(picture.lab(0, 0), [255, 128, 96]);
        assert_eq!(picture.lab(1, 0), [0, 128, 96]);
        assert_eq!(picture.pels_per_inch, 200);
        let grey = picture.lightness();
        assert_eq!((grey.width, grey.height, grey.components), (2, 2, 1));
        assert_eq!(grey.samples, [255, 0, picture.samples[6], picture.samples[9]]);
    }

    #[test]
    fn a_grey_picture_reads_as_lab_with_no_colour() {
        let picture = Picture {
            width: 2,
            height: 1,
            components: 1,
            samples: vec![0, 255],
            pels_per_inch: 200,
            ..Picture::default()
        };
        assert_eq!(picture.lab(1, 0), [255, 128, 96]);
        assert_eq!(picture.rgb(), [0, 0, 0, 255, 255, 255]);
        assert!(!picture.is_colour());
    }
}
