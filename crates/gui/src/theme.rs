//! Dark and light: which one the window uses, and the text colours for it.
//!
//! The window's colours were chosen for dark panels. On a light panel, the
//! same light greys and pale colours are hard to read (issue 100). `ink`
//! gives each of them a dark partner with the same hue. The instrument plates,
//! the scopes and the terminal draw their own dark backgrounds and keep their
//! colours in both themes.

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32};

/// Text and marks on a dark panel.
pub const DIM: Color32 = Color32::from_rgb(140, 150, 165);
pub const BRIGHT: Color32 = Color32::from_rgb(220, 225, 235);
pub const GOOD: Color32 = Color32::from_rgb(90, 220, 130);
pub const BAD: Color32 = Color32::from_rgb(235, 100, 90);
pub const WARN: Color32 = Color32::from_rgb(240, 180, 90);

/// What the operator chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// The same as Windows.
    #[default]
    System,
    Dark,
    Light,
}

impl Choice {
    /// The next one for the button: system, dark, light, and round again.
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::System,
        }
    }

    /// The name in settings.txt.
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "system" => Some(Self::System),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    /// The button's symbol.
    pub fn icon(self) -> &'static str {
        match self {
            Self::System => "◑",
            Self::Dark => "🌙",
            Self::Light => "☀",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
        }
    }
}

static LIGHT: AtomicBool = AtomicBool::new(false);

/// Read the theme egui draws this frame with. Call at the start of each
/// frame, before anything asks for a colour.
pub fn follow(ui: &egui::Ui) {
    LIGHT.store(!ui.visuals().dark_mode, Ordering::Relaxed);
}

/// Whether this frame is drawn on light panels.
pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

fn linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn gamma(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// Relative luminance, as WCAG counts it.
pub fn luminance(c: Color32) -> f64 {
    0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
}

/// A colour chosen for a dark panel, made right for a light one: the same
/// hue, darkened in linear light. The brighter it was, the darker it becomes,
/// so what stood out still stands out. The darkest result is near black, and
/// the lightest still has a contrast of about 5:1 on a white panel.
pub fn on_light(c: Color32) -> Color32 {
    let y = luminance(c);
    if y <= 0.0 {
        return c;
    }
    let target = (0.20 - 0.24 * y).clamp(0.015, 0.16);
    if y <= target {
        return c;
    }
    let k = target / y;
    Color32::from_rgba_unmultiplied(
        gamma(linear(c.r()) * k),
        gamma(linear(c.g()) * k),
        gamma(linear(c.b()) * k),
        c.a(),
    )
}

/// A text colour chosen for a dark panel, right for this frame's panels.
pub fn ink(c: Color32) -> Color32 {
    if light() { on_light(c) } else { c }
}

pub fn dim() -> Color32 {
    ink(DIM)
}

pub fn bright() -> Color32 {
    ink(BRIGHT)
}

pub fn good() -> Color32 {
    ink(GOOD)
}

pub fn bad() -> Color32 {
    ink(BAD)
}

pub fn warn() -> Color32 {
    ink(WARN)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Every text colour reads on egui's light panel, at WCAG AA or better.
    #[test]
    fn the_light_partners_read_on_a_light_panel() {
        let panel = egui::Visuals::light().panel_fill;
        for c in [DIM, BRIGHT, GOOD, BAD, WARN, Color32::from_rgb(110, 115, 130), Color32::from_rgb(240, 200, 120)] {
            let ratio = contrast(on_light(c), panel);
            assert!(ratio >= 4.5, "{c:?} -> {:?}: {ratio:.1}:1", on_light(c));
        }
    }

    /// Bright text stays darker than dim text on a light panel, as it is
    /// lighter on a dark one.
    #[test]
    fn what_stood_out_still_stands_out() {
        assert!(luminance(on_light(BRIGHT)) < luminance(on_light(DIM)));
    }

    #[test]
    fn the_hue_is_kept() {
        let green = on_light(GOOD);
        assert!(green.g() > green.r() && green.g() > green.b(), "{green:?}");
        let red = on_light(BAD);
        assert!(red.r() > red.g() && red.r() > red.b(), "{red:?}");
    }

    #[test]
    fn the_choice_goes_round_and_reads_back() {
        let mut c = Choice::System;
        for _ in 0..3 {
            assert_eq!(Choice::parse(c.name()), Some(c));
            c = c.next();
        }
        assert_eq!(c, Choice::System);
        assert_eq!(Choice::parse("purple"), None);
    }
}
