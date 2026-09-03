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

/// Bell 103 tone markers, drawn over the waterfall and spectrum.
pub const MARKERS: &[(f64, &str, Color32)] = &[
    (1070.0, "1070 O-space", Color32::from_rgb(90, 150, 240)),
    (1270.0, "1270 O-mark", Color32::from_rgb(90, 150, 240)),
    (2025.0, "2025 A-space", Color32::from_rgb(240, 170, 90)),
    (2225.0, "2225 A-mark", Color32::from_rgb(240, 170, 90)),
];

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

    pub fn paint(&mut self, ui: &mut Ui, height: f32) {
        let texture = self.texture.get_or_insert_with(|| {
            ui.ctx()
                .load_texture("waterfall", self.image.clone(), TextureOptions::LINEAR)
        });
        texture.set(self.image.clone(), TextureOptions::LINEAR);

        let (rect, painter) = allocate(ui, height);
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        paint_tone_markers(&painter, rect, false);
        frame_border(&painter, rect);
    }
}

fn allocate(ui: &mut Ui, height: f32) -> (Rect, Painter) {
    let size = vec2(ui.available_width(), height);
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    (response.rect, painter)
}

fn frame_border(painter: &Painter, rect: Rect) {
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, GRID),
        eframe::egui::StrokeKind::Inside,
    );
}

/// Vertical lines at the Bell 103 tones, so the eye can find them instantly.
///
/// The tones of a pair sit 200 Hz apart, which is only a few pixels wide, so
/// the labels are staggered vertically. Drawn on one line they overlap into an
/// unreadable smear.
fn paint_tone_markers(painter: &Painter, rect: Rect, with_text: bool) {
    for (i, (hz, name, colour)) in MARKERS.iter().enumerate() {
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

/// Instantaneous spectrum, drawn as a filled trace.
pub fn spectrum(ui: &mut Ui, bins: &[f32], hz_per_bin: f64, height: f32, floor: f32, ceiling: f32) {
    let (rect, painter) = allocate(ui, height);
    painter.rect_filled(rect, 0.0, BACKDROP);

    // Horizontal grid every 20 dB, labelled.
    let span = ceiling - floor;
    let mut db = ceiling;
    while db >= floor {
        let y = rect.top() + rect.height() * (ceiling - db) / span;
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, GRID));
        painter.text(
            pos2(rect.left() + 3.0, y),
            Align2::LEFT_CENTER,
            format!("{db:.0}"),
            FontId::monospace(9.0),
            LABEL,
        );
        db -= 20.0;
    }
    paint_tone_markers(&painter, rect, true);

    let w = rect.width() as usize;
    let mut points = Vec::with_capacity(w);
    for x in 0..w {
        let hz = DISPLAY_HZ * x as f64 / w as f64;
        let bin = (hz / hz_per_bin).round() as usize;
        let v = bins.get(bin).copied().unwrap_or(-120.0);
        let t = ((v - floor) / span).clamp(0.0, 1.0);
        points.push(pos2(rect.left() + x as f32, rect.bottom() - rect.height() * t));
    }
    for pair in points.windows(2) {
        painter.line_segment([pair[0], pair[1]], Stroke::new(1.2, TRACE));
    }

    // Frequency ticks every 500 Hz.
    let mut hz = 0.0;
    while hz <= DISPLAY_HZ {
        let x = rect.left() + rect.width() * (hz / DISPLAY_HZ) as f32;
        painter.text(
            pos2(x, rect.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            format!("{hz:.0}"),
            FontId::monospace(9.0),
            LABEL,
        );
        hz += 500.0;
    }
    frame_border(&painter, rect);
}

/// Discriminator trace with mark and space decision levels.
///
/// This is the FSK equivalent of a constellation: above the centre line is a
/// mark, below is a space, and the vertical spread shows the margin the slicer
/// has to work with.
pub fn discriminator(ui: &mut Ui, samples: &[f32], height: f32) {
    let (rect, painter) = allocate(ui, height);
    painter.rect_filled(rect, 0.0, BACKDROP);

    let level_y = |v: f32| rect.center().y - rect.height() * 0.5 * v.clamp(-1.5, 1.5) / 1.5;

    // Mark, space and decision threshold.
    for (v, colour, label) in [
        (1.0f32, Color32::from_rgb(90, 200, 120), "mark"),
        (0.0, GRID, "slice"),
        (-1.0, Color32::from_rgb(220, 120, 110), "space"),
    ] {
        let y = level_y(v);
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, colour.gamma_multiply(0.7)),
        );
        painter.text(
            pos2(rect.right() - 3.0, y),
            Align2::RIGHT_BOTTOM,
            label,
            FontId::monospace(9.0),
            colour,
        );
    }

    if samples.len() >= 2 {
        let step = rect.width() / (samples.len() - 1) as f32;
        let mut prev = pos2(rect.left(), level_y(samples[0]));
        for (i, &v) in samples.iter().enumerate().skip(1) {
            let p = pos2(rect.left() + i as f32 * step, level_y(v));
            painter.line_segment([prev, p], Stroke::new(1.0, TRACE));
            prev = p;
        }
    }
    frame_border(&painter, rect);
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
pub fn symbol_scope(
    ui: &mut Ui,
    symbols: &[f32],
    constellation: &[(f32, f32)],
    tones: usize,
    label: &str,
    quality: Option<u32>,
    height: f32,
) {
    let size = vec2(ui.available_width(), height);
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
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
    // convention that the tips are where an ideal symbol belongs.
    let m = constellation.len().max(1);
    for (i, &(re, im)) in constellation.iter().enumerate() {
        let p = pos2(
            centre.x + re.clamp(-1.4, 1.4) * radius,
            centre.y - im.clamp(-1.4, 1.4) * radius,
        );
        let fade = 0.30 + 0.70 * (i as f32 / m as f32);
        let magnitude = (re * re + im * im).sqrt().min(1.0);
        painter.circle_filled(p, 1.8, margin_colour(magnitude).gamma_multiply(fade));
    }

    if let Some(q) = quality {
        painter.text(
            pos2(rect.left() + 6.0, rect.bottom() - 4.0),
            Align2::LEFT_BOTTOM,
            format!("{label} Quality: {q}"),
            FontId::monospace(11.0),
            margin_colour(q as f32 / 100.0),
        );
    }
    frame_border(&painter, rect);
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
    painter.circle_filled(centre, 6.0, colour);
    if on {
        // A soft halo, so a lit lamp reads at a glance.
        painter.circle_filled(centre, 9.0, colour.gamma_multiply(0.25));
    }
    painter.circle_stroke(centre, 6.0, Stroke::new(1.0, GRID));
    painter.text(
        pos2(centre.x, centre.y + 11.0),
        Align2::CENTER_TOP,
        label,
        FontId::monospace(9.0),
        if on { LABEL } else { LABEL.gamma_multiply(0.5) },
    );
}

/// The LED faceplate, in the order a real modem carried the lamps.
pub fn faceplate(ui: &mut Ui, leds: &telemetry::Leds) {
    let size = vec2(ui.available_width(), 44.0);
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
        lamp(&painter, pos2(x, rect.top() + 12.0), *on, label, *hue);
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

#[allow(dead_code)]
fn unused(_: Vec2) {}
