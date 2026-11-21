//! The HST front panel in four states of a call, side by side, for looking
//! at without a call: `cargo run -p gui --example hst_panel`, and with
//! `-- 9600` for the first model's panel.

#![allow(dead_code)]

#[path = "../src/scopes.rs"]
mod scopes;

use eframe::egui;
use telemetry::{HstFace, Leds};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native("HST panel", options, Box::new(|_| Ok(Box::new(Preview))))
}

struct Preview;

fn states(classic: bool) -> Vec<(&'static str, Leds, Option<HstFace>)> {
    let lit = Leds { mr: true, tr: true, ..Leds::default() };
    let face = if classic {
        HstFace { forward: 9600, back: 300, started: true, answering: true, ..HstFace::default() }
    } else {
        HstFace { forward: 14_400, back: 450, started: true, answering: true, ..HstFace::default() }
    };
    vec![
        ("on hook, HST selected", lit, None),
        (
            "answering: the tone",
            Leds { oh: true, aa: true, ..lit },
            Some(HstFace { started: false, phase: "answer tone", ..face }),
        ),
        (
            "answered, sending a screenful",
            Leds { oh: true, cd: true, sd: true, ec: true, hs: true, ..lit },
            Some(HstFace { linked: true, sending_fast: true, phase: "data", ..face }),
        ),
        (
            "channels reversed: the far end uploads, a frame resent",
            Leds { oh: true, cd: true, rd: true, ec: true, hs: true, ..lit },
            Some(HstFace { linked: true, sending_fast: false, retransmitting: true, phase: "data", ..face }),
        ),
        (
            "retraining the fast channel",
            Leds { oh: true, cd: true, ec: true, hs: true, ..lit },
            Some(HstFace { linked: true, sending_fast: true, training: true, phase: "data", ..face }),
        ),
    ]
}

impl eframe::App for Preview {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(28, 30, 36)).inner_margin(12.0))
            .show(root, |ui| {
                let classic = std::env::args().any(|a| a == "9600");
                for (caption, leds, face) in states(classic) {
                    ui.label(egui::RichText::new(caption).monospace().small().color(egui::Color32::from_rgb(150, 155, 165)));
                    ui.allocate_ui(egui::vec2(560.0, 94.0), |ui| {
                        scopes::hst_faceplate(ui, &leds, face.as_ref(), 94.0, classic);
                    });
                    ui.add_space(4.0);
                }
            });
    }
}
