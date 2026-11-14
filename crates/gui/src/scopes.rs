//! Scope painting: waterfall, spectrum, eye and the LED faceplate.

use eframe::egui::{
    Align2, Color32, ColorImage, Context, FontId, Painter, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2, pos2, vec2,
};

/// Upper edge of the display in Hz. The voiceband ends well below Nyquist, so
/// showing 0-4000 Hz wastes no space and keeps the tone pairs large.
pub const DISPLAY_HZ: f64 = 4000.0;

const BACKDROP: Color32 = Color32::from_rgb(12, 14, 18);
const GRID: Color32 = Color32::from_rgb(40, 46, 56);
const TRACE: Color32 = Color32::from_rgb(120, 220, 160);
const LABEL: Color32 = Color32::from_rgb(150, 160, 175);

/// Blue for whatever the calling modem puts on the line, orange for the
/// answering modem, so the two directions can be told apart at a glance.
const CALLING: Color32 = Color32::from_rgb(90, 150, 240);
const ANSWERING: Color32 = Color32::from_rgb(240, 170, 90);
/// A line both directions share, which V.32 onwards is the whole point of.
const SHARED: Color32 = Color32::from_rgb(160, 140, 230);

const BELL103: &[(f64, &str, Color32)] = &[
    (1070.0, "1070 O-space", CALLING),
    (1270.0, "1270 O-mark", CALLING),
    (2025.0, "2025 A-space", ANSWERING),
    (2225.0, "2225 A-mark", ANSWERING),
];

/// V.21 6: channel 1 from the calling modem and channel 2 from the answering
/// one, each a mark below its space.
const V21: &[(f64, &str, Color32)] = &[
    (980.0, "980 C-mark", CALLING),
    (1180.0, "1180 C-space", CALLING),
    (1650.0, "1650 A-mark", ANSWERING),
    (1850.0, "1850 A-space", ANSWERING),
];

const V22BIS: &[(f64, &str, Color32)] = &[
    (1200.0, "1200 calling", CALLING),
    (2400.0, "2400 answering", ANSWERING),
];

const V32: &[(f64, &str, Color32)] = &[
    // Both directions on the one carrier, which is why V.32 needs an echo
    // canceller where V.22bis needs only a filter.
    (1800.0, "1800 carrier", SHARED),
    // Where the start-up puts its sidebands when it alternates states: the
    // carrier suppressed and these two left standing (5.4).
    (600.0, "600 sideband", SHARED),
    (3000.0, "3000 sideband", SHARED),
];

/// Where to draw tone markers for the modulation actually in use.
///
/// Drawn from the modulation the modem reports rather than fixed, because a
/// marker in the wrong place is worse than none: it invites the eye to read
/// energy that is somewhere else as being where the label says.
pub fn markers(modulation: &str) -> &'static [(f64, &'static str, Color32)] {
    match modulation {
        "Bell 103" => BELL103,
        "V.21" => V21,
        "V.22bis" => V22BIS,
        "V.32" | "V.32bis" => V32,
        _ => &[],
    }
}

/// Map a normalised magnitude to a waterfall colour.
///
/// Black through blue, cyan, green, yellow to red: the palette every ham
/// waterfall uses, chosen because weak signals stay visible against the noise
/// floor while strong ones saturate distinctly.
fn heat(v: f32) -> Color32 {
    let v = v.clamp(0.0, 1.0);
    let (r, g, b) = if v < 0.25 {
        let t = v / 0.25;
        (0.0, 0.0, 0.35 + 0.65 * t)
    } else if v < 0.45 {
        let t = (v - 0.25) / 0.20;
        (0.0, t, 1.0)
    } else if v < 0.65 {
        let t = (v - 0.45) / 0.20;
        (0.0, 1.0, 1.0 - t)
    } else if v < 0.85 {
        let t = (v - 0.65) / 0.20;
        (t, 1.0, 0.0)
    } else {
        let t = (v - 0.85) / 0.15;
        (1.0, 1.0 - t, 0.0)
    };
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// A scrolling spectrogram. Newest row at the top.
pub struct Waterfall {
    width: usize,
    height: usize,
    image: ColorImage,
    texture: Option<TextureHandle>,
    /// dB window mapped across the colour ramp.
    pub floor_db: f32,
    pub ceiling_db: f32,
}

impl Waterfall {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            image: ColorImage::filled([width, height], BACKDROP),
            texture: None,
            floor_db: -90.0,
            ceiling_db: -20.0,
        }
    }

    /// Push one spectrum row, resampling the bins across the display width.
    pub fn push_row(&mut self, bins: &[f32], hz_per_bin: f64) {
        let px = &mut self.image.pixels;
        let w = self.width;
        // Scroll everything down by one row, then write the new row at the top.
        px.copy_within(0..(self.height - 1) * w, w);

        let span = self.ceiling_db - self.floor_db;
        for (x, cell) in px.iter_mut().take(w).enumerate() {
            let hz = DISPLAY_HZ * x as f64 / w as f64;
            let bin = (hz / hz_per_bin).round() as usize;
            let db = bins.get(bin).copied().unwrap_or(-120.0);
            *cell = heat((db - self.floor_db) / span);
        }
    }

    /// Paint it, with what it is a picture of written on it: the frequency
    /// along the top, how long ago each row was down the left, and what the
    /// colours stand for down the right. `row_secs` is how long a row lasts,
    /// which only whoever pushes the rows can know.
    pub fn paint(&mut self, ui: &mut Ui, height: f32, modulation: &str, row_secs: f32) {
        let texture = self.texture.get_or_insert_with(|| {
            ui.ctx()
                .load_texture("waterfall", self.image.clone(), TextureOptions::LINEAR)
        });
        texture.set(self.image.clone(), TextureOptions::LINEAR);

        let (response, painter) = allocate(ui, height);
        let rect = response.rect;
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        paint_tone_markers(&painter, rect, false, modulation);

        // Frequency along the top, every 500 Hz.
        let mut hz = 500.0;
        while hz < DISPLAY_HZ {
            let x = rect.left() + rect.width() * (hz / DISPLAY_HZ) as f32;
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.top() + 4.0)],
                Stroke::new(1.0, LABEL),
            );
            tag(&painter, pos2(x, rect.top() + 5.0), Align2::CENTER_TOP, format!("{hz:.0}"), LABEL);
            hz += 500.0;
        }

        // How long ago, down the left: a round step that gives a handful of
        // labels whatever the depth comes to.
        let depth = self.height as f32 * row_secs;
        if depth > 0.5 {
            let step = [1.0f32, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0]
                .into_iter()
                .find(|s| depth / s <= 6.0)
                .unwrap_or(120.0);
            let mut ago = step;
            while ago < depth {
                let y = rect.top() + rect.height() * ago / depth;
                painter.line_segment(
                    [pos2(rect.left(), y), pos2(rect.left() + 5.0, y)],
                    Stroke::new(1.0, LABEL),
                );
                tag(&painter, pos2(rect.left() + 7.0, y), Align2::LEFT_CENTER, format!("-{ago:.0} s"), LABEL);
                ago += step;
            }
        }

        // The colour ramp and the levels at its two ends.
        let bar = Rect::from_min_max(
            pos2(rect.right() - 12.0, rect.top() + 22.0),
            pos2(rect.right() - 4.0, rect.bottom() - 22.0),
        );
        if bar.height() > 40.0 {
            let steps = bar.height() as usize;
            for k in 0..steps {
                let y = bar.top() + k as f32;
                painter.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    Stroke::new(1.0, heat(1.0 - k as f32 / steps as f32)),
                );
            }
            painter.rect_stroke(bar, 0.0, Stroke::new(1.0, GRID), eframe::egui::StrokeKind::Outside);
            tag(&painter, pos2(rect.right() - 3.0, bar.top() - 3.0), Align2::RIGHT_BOTTOM, format!("{:.0} dB", self.ceiling_db), LABEL);
            tag(&painter, pos2(rect.right() - 3.0, bar.bottom() + 3.0), Align2::RIGHT_TOP, format!("{:.0} dB", self.floor_db), LABEL);
        }

        // Under the pointer: which frequency, and how long ago.
        if let Some(at) = response.hover_pos() {
            let hz = DISPLAY_HZ * ((at.x - rect.left()) / rect.width()) as f64;
            let ago = depth * (at.y - rect.top()) / rect.height();
            painter.line_segment(
                [pos2(at.x, rect.top()), pos2(at.x, rect.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(90)),
            );
            painter.line_segment(
                [pos2(rect.left(), at.y), pos2(rect.right(), at.y)],
                Stroke::new(1.0, Color32::from_white_alpha(90)),
            );
            let side = if at.x > rect.center().x { Align2::RIGHT_BOTTOM } else { Align2::LEFT_BOTTOM };
            let dx = if at.x > rect.center().x { -8.0 } else { 8.0 };
            tag(&painter, pos2(at.x + dx, at.y - 6.0), side, format!("{hz:.0} Hz  {ago:.1} s ago"), Color32::WHITE);
        }
        frame_border(&painter, rect);
    }
}

/// A label with a dark plate behind it, so it reads over a waterfall or a
/// trace whatever colour is underneath.
fn tag(painter: &Painter, at: Pos2, anchor: Align2, text: String, colour: Color32) {
    let galley = painter.layout_no_wrap(text, FontId::monospace(10.0), colour);
    let rect = anchor.anchor_size(at, galley.size());
    painter.rect_filled(rect.expand2(vec2(3.0, 1.0)), 0.0, Color32::from_black_alpha(190));
    painter.galley(rect.min, galley, colour);
}

/// What a spectrum says of itself, measured from its bins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    /// The strongest bin: where it is and how strong.
    pub peak_hz: f64,
    pub peak_db: f32,
    /// What the quiet tenth of the band sits at.
    pub floor_db: f32,
    /// The lowest and highest frequencies with energy within 20 dB of the
    /// peak: the band the signal occupies.
    pub low_hz: f64,
    pub high_hz: f64,
}

/// Measure a spectrum, or None if nothing stands clear of the noise.
///
/// Above 100 Hz only: under that is mains hum and the DC a sound card leaves,
/// and neither is anything a modem sent.
pub fn measure(bins: &[f32], hz_per_bin: f64) -> Option<Measured> {
    if hz_per_bin <= 0.0 {
        return None;
    }
    let first = (100.0 / hz_per_bin).ceil() as usize;
    let last = ((DISPLAY_HZ / hz_per_bin) as usize).min(bins.len());
    if last < first + 8 {
        return None;
    }
    let band = &bins[first..last];
    // Rust note: `total_cmp` orders floats including NaN, which `max_by`
    // needs since f32 has no total order of its own.
    let (at, &peak_db) = band.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let mut sorted = band.to_vec();
    sorted.sort_by(f32::total_cmp);
    let floor_db = sorted[sorted.len() / 10];
    if peak_db - floor_db < 12.0 {
        return None;
    }
    let edge = (peak_db - 20.0).max(floor_db + 6.0);
    let low = band.iter().position(|&v| v >= edge)?;
    let high = band.iter().rposition(|&v| v >= edge)?;
    let hz = |k: usize| (first + k) as f64 * hz_per_bin;
    Some(Measured { peak_hz: hz(at), peak_db, floor_db, low_hz: hz(low), high_hz: hz(high) })
}

/// The line samples' own figures: the largest, the root mean square, and how
/// many sit at full scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wave {
    pub peak_db: f32,
    pub rms_db: f32,
    pub clipped: usize,
}

pub fn wave(samples: &[f32]) -> Option<Wave> {
    if samples.is_empty() {
        return None;
    }
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let mean = samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32;
    let db = |v: f32| 20.0 * (v + 1e-9).log10();
    Some(Wave {
        peak_db: db(peak),
        rms_db: db(mean.sqrt()),
        clipped: samples.iter().filter(|v| v.abs() >= 0.999).count(),
    })
}

/// The line as an oscilloscope would show it: the last twenty milliseconds,
/// started on a rising zero crossing so a steady tone stands still, and
/// scaled to its own peak -- with the scale written on it, since a trace
/// that fills the box at -40 dBFS and at -3 looks the same.
pub fn oscilloscope(ui: &mut Ui, samples: &[f32], sample_rate: f64, size: Vec2) {
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, BACKDROP);
    let mid = rect.center().y;
    painter.line_segment([pos2(rect.left(), mid), pos2(rect.right(), mid)], Stroke::new(1.0, GRID));
    for quarter in 1..4 {
        let x = rect.left() + rect.width() * quarter as f32 / 4.0;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, GRID));
    }

    let span = ((sample_rate * 0.020) as usize).clamp(2, samples.len().max(2));
    if samples.len() >= span {
        // The latest crossing that still leaves a whole span after it.
        let latest = samples.len() - span;
        let start = (1..=latest)
            .rev()
            .find(|&i| samples[i - 1] < 0.0 && samples[i] >= 0.0)
            .unwrap_or(latest);
        let shown = &samples[start..start + span];
        let peak = shown.iter().fold(1e-4f32, |m, v| m.max(v.abs()));
        let half = rect.height() * 0.5 - 16.0;
        let points: Vec<Pos2> = shown
            .iter()
            .enumerate()
            .map(|(i, v)| {
                pos2(
                    rect.left() + rect.width() * i as f32 / (span - 1) as f32,
                    mid - half * v / peak,
                )
            })
            .collect();
        for pair in points.windows(2) {
            painter.line_segment([pair[0], pair[1]], Stroke::new(1.2, TRACE));
        }
        painter.text(
            pos2(rect.left() + 5.0, rect.bottom() - 3.0),
            Align2::LEFT_BOTTOM,
            format!("full height \u{b1}{peak:.3} of full scale"),
            FontId::monospace(10.0),
            LABEL,
        );
    }
    painter.text(
        pos2(rect.left() + 5.0, rect.top() + 3.0),
        Align2::LEFT_TOP,
        "line  20 ms  5 ms a division",
        FontId::monospace(10.0),
        LABEL,
    );
    frame_border(&painter, rect);
}

fn allocate(ui: &mut Ui, height: f32) -> (eframe::egui::Response, Painter) {
    let size = vec2(ui.available_width(), height);
    ui.allocate_painter(size, Sense::hover())
}

fn frame_border(painter: &Painter, rect: Rect) {
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, GRID),
        eframe::egui::StrokeKind::Inside,
    );
}

/// Vertical lines at the tones the modulation in use lives on, so the eye can
/// find them instantly.
///
/// Bell 103's tones sit 200 Hz apart, which is only a few pixels wide, so the
/// labels are staggered vertically. Drawn on one line they overlap into an
/// unreadable smear.
fn paint_tone_markers(painter: &Painter, rect: Rect, with_text: bool, modulation: &str) {
    for (i, (hz, name, colour)) in markers(modulation).iter().enumerate() {
        let x = rect.left() + rect.width() * (*hz / DISPLAY_HZ) as f32;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, colour.gamma_multiply(0.55)),
        );
        if with_text {
            let row = (i % 2) as f32;
            painter.text(
                pos2(x + 3.0, rect.top() + 2.0 + row * 11.0),
                Align2::LEFT_TOP,
                name,
                FontId::monospace(9.0),
                *colour,
            );
        }
    }
}

/// What the spectrum scope draws.
pub struct Trace<'a> {
    pub bins: &'a [f32],
    /// The highest each bin has been lately, falling slowly: what a tone
    /// that came and went looked like while it was there.
    pub hold: &'a [f32],
    pub hz_per_bin: f64,
    pub floor: f32,
    pub ceiling: f32,
    pub modulation: &'a str,
    pub measured: Option<Measured>,
}

/// Instantaneous spectrum, with what can be read off it written on it: the
/// peak, the noise floor, the band the signal occupies, and the frequency
/// and level under the pointer.
pub fn spectrum(ui: &mut Ui, height: f32, trace: Trace<'_>) {
    let Trace { bins, hold, hz_per_bin, floor, ceiling, modulation, measured } = trace;
    let (response, painter) = allocate(ui, height);
    let whole = response.rect;
    painter.rect_filled(whole, 0.0, BACKDROP);
    // The frequency scale has a strip of its own under the plot, so the
    // numbers are not drawn through the noise floor.
    let rect = Rect::from_min_max(whole.min, pos2(whole.right(), whole.bottom() - 14.0));

    let span = ceiling - floor;
    let y_of = |db: f32| rect.bottom() - rect.height() * ((db - floor) / span).clamp(0.0, 1.0);
    let x_of = |hz: f64| rect.left() + rect.width() * (hz / DISPLAY_HZ) as f32;

    // Horizontal grid every 20 dB, labelled.
    let mut db = (ceiling / 20.0).floor() * 20.0;
    while db >= floor {
        let y = y_of(db);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, GRID));
        painter.text(
            pos2(rect.left() + 3.0, y - 1.0),
            Align2::LEFT_BOTTOM,
            format!("{db:.0}"),
            FontId::monospace(9.0),
            LABEL,
        );
        db -= 20.0;
    }
    painter.text(
        pos2(rect.left() + 3.0, rect.bottom() - 2.0),
        Align2::LEFT_BOTTOM,
        "dBFS",
        FontId::monospace(9.0),
        LABEL,
    );
    // Frequency grid and ticks every 500 Hz.
    let mut hz = 0.0;
    while hz <= DISPLAY_HZ {
        let x = x_of(hz);
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, GRID));
        let (anchor, text) = if hz == 0.0 {
            (Align2::LEFT_BOTTOM, "0 Hz".to_owned())
        } else if hz >= DISPLAY_HZ {
            (Align2::RIGHT_BOTTOM, format!("{hz:.0}"))
        } else {
            (Align2::CENTER_BOTTOM, format!("{hz:.0}"))
        };
        painter.text(pos2(x, whole.bottom() - 1.0), anchor, text, FontId::monospace(9.0), LABEL);
        hz += 500.0;
    }
    paint_tone_markers(&painter, rect, true, modulation);

    let w = rect.width() as usize;
    let line = |source: &[f32], stroke: Stroke| {
        let points: Vec<Pos2> = (0..w)
            .map(|x| {
                let hz = DISPLAY_HZ * x as f64 / w as f64;
                let bin = (hz / hz_per_bin).round() as usize;
                pos2(rect.left() + x as f32, y_of(source.get(bin).copied().unwrap_or(-120.0)))
            })
            .collect();
        for pair in points.windows(2) {
            painter.line_segment([pair[0], pair[1]], stroke);
        }
    };
    line(hold, Stroke::new(1.0, TRACE.gamma_multiply(0.35)));
    line(bins, Stroke::new(1.2, TRACE));

    if let Some(m) = measured {
        // The noise floor, as a line with its level on it.
        let y = y_of(m.floor_db);
        let faint = Color32::from_rgb(200, 150, 90);
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, faint.gamma_multiply(0.5)),
        );
        tag(&painter, pos2(rect.right() - 4.0, y - 2.0), Align2::RIGHT_BOTTOM, format!("floor {:.0} dB", m.floor_db), faint);

        // The band the signal occupies, bracketed.
        let (a, b) = (x_of(m.low_hz), x_of(m.high_hz));
        let top = rect.top() + 34.0;
        if b - a > 24.0 {
            let stroke = Stroke::new(1.0, Color32::from_rgb(130, 190, 240));
            painter.line_segment([pos2(a, top), pos2(b, top)], stroke);
            painter.line_segment([pos2(a, top - 4.0), pos2(a, top + 4.0)], stroke);
            painter.line_segment([pos2(b, top - 4.0), pos2(b, top + 4.0)], stroke);
            tag(
                &painter,
                pos2((a + b) * 0.5, top + 3.0),
                Align2::CENTER_TOP,
                format!("{:.0} - {:.0} Hz, {:.0} wide", m.low_hz, m.high_hz, m.high_hz - m.low_hz),
                Color32::from_rgb(130, 190, 240),
            );
        }

        // The peak, named where it stands.
        let at = pos2(x_of(m.peak_hz), y_of(m.peak_db));
        painter.circle_stroke(at, 3.5, Stroke::new(1.2, Color32::WHITE));
        let (anchor, dx) = if at.x > rect.center().x { (Align2::RIGHT_CENTER, -8.0) } else { (Align2::LEFT_CENTER, 8.0) };
        tag(
            &painter,
            pos2(at.x + dx, (at.y + 12.0).max(rect.top() + 62.0)),
            anchor,
            format!("peak {:.0} Hz  {:.1} dB  {:.0} over the floor", m.peak_hz, m.peak_db, m.peak_db - m.floor_db),
            Color32::WHITE,
        );
    }

    // Under the pointer: the frequency, and the level of the bin there.
    if let Some(at) = response.hover_pos().filter(|p| rect.contains(*p)) {
        let hz = DISPLAY_HZ * ((at.x - rect.left()) / rect.width()) as f64;
        let level = bins.get((hz / hz_per_bin).round() as usize).copied().unwrap_or(-120.0);
        painter.line_segment(
            [pos2(at.x, rect.top()), pos2(at.x, rect.bottom())],
            Stroke::new(1.0, Color32::from_white_alpha(90)),
        );
        let (anchor, dx) = if at.x > rect.center().x { (Align2::RIGHT_BOTTOM, -8.0) } else { (Align2::LEFT_BOTTOM, 8.0) };
        tag(&painter, pos2(at.x + dx, at.y - 6.0), anchor, format!("{hz:.0} Hz  {level:.1} dBFS"), Color32::WHITE);
    }
    frame_border(&painter, whole);
}

/// Symbol scope, in the style ARDOP uses for its FSK modes.
///
/// A cross of axes with the decision threshold at the centre. Each recovered
/// symbol is drawn as a vertical line on the horizontal axis at its slicer
/// margin: out at the arm tips it was an unambiguous tone, in toward the centre
/// it was a marginal decision. Colour follows the same scale, green at the tips
/// through yellow to red at the middle, so a degrading link is visible as the
/// marks collapsing inward and reddening before any bit errors appear.
///
/// Binary FSK populates only the horizontal axis. The vertical axis is drawn
/// for the four-tone modes to use, where symbols occupy all four arms.
///
/// The same widget serves the phase and quadrature-amplitude modulations: when
/// `constellation` is non-empty it plots those points as a dot scatter against
/// the same cross, which is what every mode above 300 bps will need.
/// The scatter half of the symbol scope: the points, how many the modulation
/// has, and how far they reach.
pub struct Constellation<'a> {
    pub points: &'a [(f32, f32)],
    /// Sets how many arms are drawn: 2 for Bell 103, 16 for V.22bis.
    pub tones: usize,
    /// How far the points reach. One fits the box exactly.
    pub peak: f32,
    /// The points are PCM samples, each against the next, rather than I and
    /// Q -- which a reader will take them for unless told.
    pub pairs: bool,
}

pub fn symbol_scope(
    ui: &mut Ui,
    symbols: &[f32],
    dots: Constellation<'_>,
    label: &str,
    quality: Option<u32>,
    height: f32,
) -> eframe::egui::Response {
    let Constellation { points: constellation, tones, peak, pairs } = dots;
    let size = vec2(ui.available_width(), height);
    let (response, painter) = ui.allocate_painter(size, Sense::click());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, Color32::BLACK);

    let centre = rect.center();
    // Keep the plot square so the two axes share a scale.
    let radius = (rect.width().min(rect.height()) * 0.5) - 12.0;
    let axis = Color32::from_rgb(70, 130, 200);

    let quadrature = tones > 2 || !constellation.is_empty();
    painter.line_segment(
        [pos2(centre.x - radius, centre.y), pos2(centre.x + radius, centre.y)],
        Stroke::new(1.5, axis),
    );
    if quadrature {
        painter.line_segment(
            [pos2(centre.x, centre.y - radius), pos2(centre.x, centre.y + radius)],
            Stroke::new(1.5, axis),
        );
    }
    // Tick at each arm tip: where an ideal symbol should land.
    for dx in [-1.0f32, 1.0] {
        let x = centre.x + dx * radius;
        painter.line_segment(
            [pos2(x, centre.y - 5.0), pos2(x, centre.y + 5.0)],
            Stroke::new(1.0, axis.gamma_multiply(0.8)),
        );
    }

    // Newer symbols are drawn more strongly, so the display shows the present
    // rather than a smear of everything ever received.
    let n = symbols.len().max(1);
    for (i, &v) in symbols.iter().enumerate() {
        let margin = v.abs().min(1.0);
        let x = centre.x + v.clamp(-1.2, 1.2) * radius;
        let fade = 0.30 + 0.70 * (i as f32 / n as f32);
        let half = 7.0 + 5.0 * margin;
        painter.line_segment(
            [pos2(x, centre.y - half), pos2(x, centre.y + half)],
            Stroke::new(3.0, margin_colour(margin).gamma_multiply(fade)),
        );
    }

    // Phase and QAM modulations plot as a dot scatter instead. Points are
    // scaled so a unit-magnitude symbol sits at the arm tip, matching the FSK
    // convention that the tips are where an ideal symbol belongs -- and then
    // shrunk to fit if the constellation reaches further than that, which
    // V.32's thirty-two points do. Eight of them have a coordinate a quarter
    // beyond the box and were being drawn off the edge of it, so a
    // thirty-two-point constellation showed twenty-four dots.
    let fit = 1.0 / peak.max(1.0);
    let m = constellation.len().max(1);
    let at = |re: f32, im: f32| {
        pos2(
            centre.x + (re * fit).clamp(-1.4, 1.4) * radius,
            centre.y - (im * fit).clamp(-1.4, 1.4) * radius,
        )
    };
    // Every constellation is drawn the same way, whatever the modulation:
    // one colour, faint enough that the symbols landing on a point build up
    // into it, as one mesh rather than thousands of shapes.
    //
    // Colouring each symbol by its distance from the centre, which the
    // smaller constellations used to do, paints the outer ring green and the
    // inner one red on any modulation whose points are not all the same
    // distance out -- which says something about the constellation and
    // nothing about the line. What a reader wants from this scope is the
    // shape of the clusters, and that is what building up shows.
    //
    // The smaller constellations keep 512 symbols against V.34's thousands,
    // so their squares are larger and less faint; otherwise sixteen clusters
    // of thirty-two symbols would be almost invisible beside a V.34 cloud.
    let mut mesh = eframe::egui::Mesh::default();
    let crowded = m > 200;
    let side = (radius / 180.0).clamp(1.0, 2.5) * if crowded { 1.0 } else { 1.7 };
    let colour = Color32::from_rgba_unmultiplied(120, 220, 160, if crowded { 110 } else { 150 });
    for &(re, im) in constellation {
        mesh.add_colored_rect(Rect::from_center_size(at(re, im), vec2(side, side)), colour);
    }
    painter.add(eframe::egui::Shape::mesh(mesh));

    // Always say something. A silent, empty scope gives no way to tell a modem
    // that is not decoding from a display that is not being fed.
    if pairs {
        // Said on the axes themselves, always: this is not I against Q.
        let faint = Color32::from_rgb(110, 125, 145);
        painter.text(pos2(centre.x + radius - 2.0, centre.y + 4.0), Align2::RIGHT_TOP, "sample n", FontId::monospace(10.0), faint);
        painter.text(pos2(centre.x + 4.0, centre.y - radius + 2.0), Align2::LEFT_TOP, "sample n+1", FontId::monospace(10.0), faint);
    }
    let (text, colour) = match quality {
        _ if pairs && !constellation.is_empty() => (
            format!("{label} {tones} points, last {} samples", constellation.len()),
            Color32::from_rgb(150, 160, 175),
        ),
        Some(q) => (format!("{label} Quality: {q}"), margin_colour(q as f32 / 100.0)),
        None if tones > 128 && !constellation.is_empty() => (
            format!("{label}  {tones} points, last {} symbols", constellation.len()),
            Color32::from_rgb(150, 160, 175),
        ),
        None if !constellation.is_empty() => (
            format!("{label}  {} points", constellation.len()),
            Color32::from_rgb(150, 160, 175),
        ),
        None => (format!("{label}  no symbols"), Color32::from_rgb(120, 100, 100)),
    };
    painter.text(
        pos2(rect.left() + 6.0, rect.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        text,
        FontId::monospace(11.0),
        colour,
    );
    frame_border(&painter, rect);
    response
}

/// Green at a full decision margin, through yellow, to red at the threshold.
fn margin_colour(margin: f32) -> Color32 {
    let m = margin.clamp(0.0, 1.0);
    if m > 0.5 {
        let t = (m - 0.5) / 0.5;
        Color32::from_rgb(
            (255.0 * (1.0 - t) + 60.0 * t) as u8,
            (215.0 * (1.0 - t) + 230.0 * t) as u8,
            (60.0 * (1.0 - t) + 90.0 * t) as u8,
        )
    } else {
        let t = m / 0.5;
        Color32::from_rgb(
            (235.0 * (1.0 - t) + 255.0 * t) as u8,
            (60.0 * (1.0 - t) + 215.0 * t) as u8,
            (55.0 * (1.0 - t) + 60.0 * t) as u8,
        )
    }
}

/// One faceplate lamp.
fn lamp(painter: &Painter, centre: Pos2, on: bool, label: &str, hue: Color32) {
    let colour = if on { hue } else { hue.gamma_multiply(0.16) };
    painter.circle_filled(centre, 8.0, colour);
    if on {
        // A soft halo, so a lit lamp reads at a glance.
        painter.circle_filled(centre, 12.0, colour.gamma_multiply(0.25));
    }
    painter.circle_stroke(centre, 8.0, Stroke::new(1.0, GRID));
    painter.text(
        pos2(centre.x, centre.y + 13.0),
        Align2::CENTER_TOP,
        label,
        FontId::monospace(11.0),
        if on { LABEL } else { LABEL.gamma_multiply(0.5) },
    );
}

/// The LED faceplate, in the order a real modem carried the lamps.
pub fn faceplate(ui: &mut Ui, leds: &telemetry::Leds) {
    let size = vec2(ui.available_width(), 52.0);
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, BACKDROP);

    let green = Color32::from_rgb(80, 230, 120);
    let amber = Color32::from_rgb(245, 180, 70);
    let red = Color32::from_rgb(235, 100, 90);

    let lamps: [(&str, bool, Color32); 9] = [
        ("MR", leds.mr, green),
        ("TR", leds.tr, green),
        ("SD", leds.sd, amber),
        ("RD", leds.rd, amber),
        ("CD", leds.cd, green),
        ("OH", leds.oh, red),
        ("AA", leds.aa, amber),
        ("HS", leds.hs, green),
        ("EC", leds.ec, green),
    ];

    let step = rect.width() / lamps.len() as f32;
    for (i, (label, on, hue)) in lamps.iter().enumerate() {
        let x = rect.left() + step * (i as f32 + 0.5);
        lamp(&painter, pos2(x, rect.top() + 16.0), *on, label, *hue);
    }
    frame_border(&painter, rect);
}

/// A horizontal level meter in dBFS.
pub fn level_meter(ui: &mut Ui, db: f32) {
    let size = vec2(ui.available_width(), 14.0);
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, BACKDROP);

    let (floor, ceiling) = (-60.0f32, 0.0f32);
    let t = ((db - floor) / (ceiling - floor)).clamp(0.0, 1.0);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * t, rect.height()));
    // Green up to -12 dBFS, amber to -3, red above: a modem receiving above
    // -10 dBFS is almost certainly clipping somewhere upstream.
    let colour = if db > -3.0 {
        Color32::from_rgb(235, 90, 80)
    } else if db > -12.0 {
        Color32::from_rgb(240, 180, 70)
    } else {
        Color32::from_rgb(80, 210, 120)
    };
    painter.rect_filled(filled, 0.0, colour);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{db:.1} dBFS"),
        FontId::monospace(10.0),
        Color32::from_rgb(230, 235, 240),
    );
    frame_border(&painter, rect);
}

/// Repaint continuously while a call is running, so the waterfall scrolls.
pub fn request_animation(ctx: &Context) {
    ctx.request_repaint_after(std::time::Duration::from_millis(16));
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_is_measured_where_it_stands() {
        // 512 bins of 7.8125 Hz: noise at -90, one tone at 2225 Hz and -30.
        let hz_per_bin: f64 = 4000.0 / 512.0;
        let mut bins = vec![-90.0f32; 512];
        let tone = (2225.0f64 / hz_per_bin).round() as usize;
        bins[tone] = -30.0;
        bins[tone - 1] = -42.0;
        bins[tone + 1] = -42.0;
        let m = measure(&bins, hz_per_bin).expect("a tone 60 dB over the noise");
        assert!((m.peak_hz - 2225.0).abs() < hz_per_bin, "peak at {}", m.peak_hz);
        assert_eq!(m.peak_db, -30.0);
        assert_eq!(m.floor_db, -90.0);
        assert!(m.low_hz < m.peak_hz && m.peak_hz < m.high_hz);
        assert!(m.high_hz - m.low_hz < 4.0 * hz_per_bin, "{} wide", m.high_hz - m.low_hz);
    }

    #[test]
    fn silence_measures_as_nothing() {
        assert_eq!(measure(&[-120.0; 512], 4000.0 / 512.0), None);
        assert_eq!(measure(&[], 7.8), None);
    }

    #[test]
    fn a_full_scale_sine_has_three_decibels_of_crest() {
        let samples: Vec<f32> = (0..800).map(|i| (i as f32 * 0.25).sin()).collect();
        let w = wave(&samples).unwrap();
        assert!(w.peak_db.abs() < 0.1, "peak {}", w.peak_db);
        assert!((w.peak_db - w.rms_db - 3.01).abs() < 0.1, "crest {}", w.peak_db - w.rms_db);
        assert_eq!(wave(&[]), None);
    }
}
