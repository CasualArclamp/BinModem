//! DialSnoop: a recording of a dial-up call, read and laid out.
//!
//! The same reader the modem's window has on its Review tab, in a program
//! that is nothing else. It has no line and no modem: it opens a WAV of a
//! call somebody else made and shows what was on it, from the answer tone to
//! what was typed.
//!
//!   dialsnoop                    open with nothing loaded
//!   dialsnoop <recording.wav>    and read this one
//!
//! Two channels with an end on each can be followed all the way; one channel
//! with both ends on it as far as the two share a band.

// No console window behind the program's own when it is opened from Explorer;
// `parent_console` gives back the output a shell expects.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(target_os = "windows", target_arch = "x86"))]
mod xp_compat;

mod crashlog;
mod parent_console;
mod review;
// The symbol scope is the modem window's, and so is the rest of that file,
// which draws things only a modem has.
#[allow(dead_code)]
mod scopes;
#[allow(dead_code)]
mod theme;
mod window;

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};

const BUILD: &str = env!("CARGO_PKG_VERSION");

struct Snoop {
    pane: review::ReviewPane,
}

impl eframe::App for Snoop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("name").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("DIALSNOOP").monospace().strong().color(crate::theme::ink(Color32::from_rgb(220, 225, 235))));
                ui.label(
                    RichText::new(format!("a dial-up call, read off a recording   {BUILD}"))
                        .small()
                        .color(crate::theme::ink(Color32::from_rgb(140, 150, 165))),
                );
            });
            ui.add_space(3.0);
        });
        egui::CentralPanel::default().show(ui, |ui| self.pane.ui(ui));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    crashlog::install();
    parent_console::attach();

    let mut recording: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "dialsnoop                    open with nothing loaded\n\
                     dialsnoop <recording.wav>    read a recording of a call: what was said, and when\n\
                     \n\
                     Two channels with an end on each are followed into data mode and through it.\n\
                     One channel with both ends on it is followed until the two share a band."
                );
                return Ok(());
            }
            "--version" => {
                println!("dialsnoop {BUILD}");
                return Ok(());
            }
            other if other.starts_with('-') => return Err(format!("unknown argument {other}").into()),
            other => recording = Some(PathBuf::from(other)),
        }
    }

    let options = || eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("DialSnoop"),
        ..Default::default()
    };
    window::run(
        "DialSnoop",
        options,
        move |cc| {
            // DialSnoop has no theme key: it stays dark, also on a light
            // Windows theme (issue 100).
            cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
            let mut pane = review::ReviewPane::default();
            if let Some(path) = &recording {
                pane.open(path);
            }
            Ok(Box::new(Snoop { pane }))
        },
    )?;
    Ok(())
}
