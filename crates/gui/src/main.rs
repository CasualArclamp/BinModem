//! Scope for the modem, over a capture or over a real line.
//!
//! Two things can be on the screen. A capture is a recording of a call
//! somebody else placed: it is replayed, watched, and cannot be typed at. A
//! live line is a modem of our own, and the terminal in the window is its DTE
//! — `AT` commands are answered, `ATD` dials, and everything the scopes show
//! is the call actually happening.
//!
//! ```text
//!   modem-scope                                  the Bell 103 golden vector
//!   modem-scope <path.wav>                       any capture
//!   modem-scope --devices                        what audio this machine has
//!   modem-scope --live --in <dev> --out <dev>    a modem on that line
//! ```
//!
//! Both device names are required for `--live` and neither defaults. The
//! default output on a desktop machine is whatever the speakers are plugged
//! into, and a handshake played through speakers is no use to anyone.

mod app;
mod console;
mod engine;
mod live;
mod scopes;

use std::path::PathBuf;
use std::sync::Arc;

use app::Source;
use engine::{Control, SCOPE_LEN, SPECTRUM_BINS};
use line::AudioSink;

/// The rate a live modem runs at, matching [`live`].
const LIVE_FS: f64 = 16_000.0;

fn default_vector() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/bell103-300.wav")
}

fn list_devices() {
    println!("input devices (--in):");
    for name in line::input_devices() {
        println!("  {name}");
    }
    println!("\noutput devices (--out):");
    for name in line::output_devices() {
        println!("  {name}");
    }
}

struct Args {
    path: Option<PathBuf>,
    live: bool,
    input: Option<String>,
    output: Option<String>,
}

fn parse() -> Result<Option<Args>, String> {
    let mut args = Args { path: None, live: false, input: None, output: None };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut rest = raw.iter();
    while let Some(arg) = rest.next() {
        let mut value = |name: &str| {
            rest.next().cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--live" => args.live = true,
            "--in" => args.input = Some(value("--in")?),
            "--out" => args.output = Some(value("--out")?),
            "--devices" | "--list-devices" => {
                list_devices();
                return Ok(None);
            }
            "--help" | "-h" => {
                println!(
                    "modem-scope [path.wav]                        replay a capture\n\
                     modem-scope --devices                         list audio devices\n\
                     modem-scope --live --in <dev> --out <dev>     a modem on a real line"
                );
                return Ok(None);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown argument {other}"));
            }
            other => args.path = Some(PathBuf::from(other)),
        }
    }
    if args.live && (args.input.is_none() || args.output.is_none()) {
        return Err("--live needs both --in and --out; see --devices".into());
    }
    Ok(Some(args))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(args) = parse()? else { return Ok(()) };

    let control = Arc::new(Control::default());
    let (sample_rate, title) = if args.live {
        (LIVE_FS, "dialupmodem2 - live")
    } else {
        let path = args.path.clone().unwrap_or_else(default_vector);
        let wav = line::wav::read(&path)?;
        (f64::from(wav.sample_rate), "dialupmodem2 - scope")
    };

    let (tx, rx) = telemetry::channel(SCOPE_LEN, SPECTRUM_BINS, sample_rate);
    // A quarter second of monitor buffer: enough to ride out scheduling jitter
    // without adding latency you can hear against the scopes.
    let sink = Arc::new(AudioSink::new((sample_rate * 0.25) as usize));

    let (engine, source) = if args.live {
        let keyboard = Arc::new(live::Keyboard::default());
        let handle = live::spawn(
            args.input.unwrap(),
            args.output.unwrap(),
            tx,
            control.clone(),
            keyboard.clone(),
            sink.clone(),
        )?;
        (handle, Source::Live(keyboard))
    } else {
        let path = args.path.unwrap_or_else(default_vector);
        (
            engine::spawn(&path, tx, control.clone(), sink.clone())?,
            Source::Capture,
        )
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 860.0])
            .with_min_inner_size([900.0, 640.0])
            .with_title(title),
        ..Default::default()
    };

    let ui_control = control.clone();
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            Ok(Box::new(app::ScopeApp::new(
                rx,
                ui_control,
                sink,
                sample_rate,
                source,
            )))
        }),
    )?;

    control.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = engine.join();
    Ok(())
}
