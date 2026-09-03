//! Live scope for the modem.
//!
//! Usage: `modem-scope [path-to-wav]`, defaulting to the Bell 103 golden vector.

mod app;
mod console;
mod engine;
mod scopes;

use std::path::PathBuf;
use std::sync::Arc;

use engine::{Control, SCOPE_LEN, SPECTRUM_BINS};
use line::AudioSink;

fn default_vector() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/bell103-300.wav")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(default_vector);
    let wav = line::wav::read(&path)?;
    let sample_rate = wav.sample_rate as f64;
    drop(wav);

    let (tx, rx) = telemetry::channel(SCOPE_LEN, SPECTRUM_BINS, sample_rate);
    let control = Arc::new(Control::default());
    // A quarter second of monitor buffer: enough to ride out scheduling jitter
    // without adding latency you can hear against the scopes.
    let sink = Arc::new(AudioSink::new((sample_rate * 0.25) as usize));
    let engine = engine::spawn(&path, tx, control.clone(), sink.clone())?;

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 860.0])
            .with_min_inner_size([900.0, 640.0])
            .with_title("dialupmodem2 - scope"),
        ..Default::default()
    };

    let ui_control = control.clone();
    eframe::run_native(
        "dialupmodem2 - scope",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            Ok(Box::new(app::ScopeApp::new(rx, ui_control, sink, sample_rate)))
        }),
    )?;

    control.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = engine.join();
    Ok(())
}
