//! Scope for the modem, over a capture or over a real line.
//!
//! Two things can be on the screen. A capture is a recording of a call
//! somebody else placed: it is replayed, watched, and cannot be typed at. A
//! live line is a modem of our own, and the terminal in the window is its DTE
//! — `AT` commands are answered, `ATD` dials, and everything the scopes show
//! is the call actually happening.
//!
//! ```text
//!   binmodem                                  a modem, line chosen in the window
//!   binmodem --in <dev> --out <dev>           and opened straight away
//!   binmodem <path.wav>                       replay a capture
//!   binmodem --capture                        replay the Bell 103 golden vector
//!   binmodem --devices                        what audio this machine has
//!   binmodem --telnet [host]                  a board over a socket, no modem
//!   binmodem --answer --in <dev> --out <dev>  a board to dial, on the same cable
//! ```
//!
//! A modem is what this is for, so a modem is what it opens with. `--live` is
//! still accepted and still means what it says; it is simply no longer the
//! thing that has to be typed to get the program's own subject on the screen.
//!
//! The last is for working on the terminal rather than on the modem. A board
//! sends the same ANSI down a socket as down a call, so the screen is the same
//! screen -- but over a socket every byte arrives, which means anything that
//! draws wrongly is the terminal's fault and not the line's. That is not a
//! distinction a capture can make.
//!
//! Neither device defaults, and `--live` on its own opens with no line rather
//! than guessing at one. The default output on a desktop machine is whatever
//! the speakers are plugged into, and a handshake played through speakers is
//! no use to anyone.

mod answer;
mod app;
mod console;
mod engine;
mod live;
mod net;
mod scopes;

use std::path::PathBuf;
use std::sync::Arc;

use app::Source;
use engine::{Control, SCOPE_LEN, SPECTRUM_BINS};
use line::AudioSink;

/// The rate a live modem runs at, matching [`live`].
const LIVE_FS: f64 = 16_000.0;

/// What to open when nothing was named.
///
/// Not a path. It used to be one, built from the directory this was compiled
/// in, which meant the program worked on exactly one computer and reported a
/// missing file on every other. The capture is carried inside the program now
/// and this is the name it answers to.
fn default_vector() -> PathBuf {
    PathBuf::from(engine::GOLDEN_NAME)
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
    /// Replay the capture carried inside the program.
    golden: bool,
    live: bool,
    input: Option<String>,
    output: Option<String>,
    /// Whether to open a terminal onto a socket instead of a modem, and where
    /// to point it. `Some(None)` is the mode with the host chosen in the
    /// window, which is the usual way in.
    telnet: Option<Option<String>>,
}

fn parse() -> Result<Option<Args>, String> {
    let mut args = Args {
        path: None,
        golden: false,
        live: false,
        input: None,
        output: None,
        telnet: None,
    };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut rest = raw.iter();
    while let Some(arg) = rest.next() {
        let mut value = |name: &str| {
            rest.next().cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--live" => args.live = true,
            "--capture" | "--golden" => args.golden = true,
            // The host is optional: without one the window opens with the box
            // empty and nothing connected, exactly as --live does with no
            // devices named.
            "--telnet" => {
                let host = rest.clone().next().filter(|h| !h.starts_with('-'));
                if host.is_some() {
                    rest.next();
                }
                args.telnet = Some(host.cloned());
            }
            "--in" => args.input = Some(value("--in")?),
            "--out" => args.output = Some(value("--out")?),
            "--devices" | "--list-devices" => {
                list_devices();
                return Ok(None);
            }
            "--help" | "-h" => {
                println!(
                    "binmodem                                   a modem, line chosen in the window\n\
                     binmodem --in <dev> --out <dev>            and opened straight away\n\
                     binmodem [path.wav]                        replay a capture\n\
                     binmodem --capture                         replay the Bell 103 golden vector\n\
                     binmodem --devices                         list audio devices\n\
                     binmodem --telnet [host]                   a board over a socket, no modem\n\
                     binmodem --answer --in <dev> --out <dev>   a board to dial, on the same cable"
                );
                return Ok(None);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown argument {other}"));
            }
            other => args.path = Some(PathBuf::from(other)),
        }
    }
    // --in and --out are optional now: without them the window opens with no
    // line and the devices are chosen there. Naming one and not the other is
    // still a mistake worth catching.
    if args.input.is_some() != args.output.is_some() {
        return Err("--in and --out go together; see --devices".into());
    }
    // One or the other. A window can show a modem on a line or a terminal on a
    // socket, and asking for both is asking which of two things the terminal
    // in it is wired to.
    if args.live && args.telnet.is_some() {
        return Err("--live and --telnet are different windows; pick one".into());
    }
    // A modem is the subject, so a modem is what this opens with. Nothing else
    // asked for means the line, not a recording of somebody else's line --
    // which is what somebody who has just double-clicked the file is after,
    // and what every other mode here is a deliberate departure from.
    args.live = args.live || (args.path.is_none() && !args.golden && args.telnet.is_none());
    if args.live && (args.path.is_some() || args.golden) {
        return Err("--live and a capture are different windows; pick one".into());
    }
    Ok(Some(args))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The answering modem is a different program in the same file. It owns the
    // process rather than sharing it -- there is no window, and what it prints
    // is the whole of its output -- so it is dispatched before anything else
    // is set up.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().is_some_and(|a| a == "--answer") {
        let code = answer::run(raw[1..].to_vec());
        // ExitCode cannot be returned from here, and its value cannot be read
        // out of it either, so the two outcomes are told apart by identity.
        std::process::exit(i32::from(code != std::process::ExitCode::SUCCESS));
    }

    let Some(args) = parse()? else { return Ok(()) };

    let control = Arc::new(Control::default());
    let (sample_rate, title) = if args.live {
        (LIVE_FS, "BinModem - live")
    } else if args.telnet.is_some() {
        // Nothing here is sampled. The rate only has to be something the
        // telemetry channel can be sized against.
        (LIVE_FS, "BinModem - telnet")
    } else {
        let path = args.path.clone().unwrap_or_else(default_vector);
        let wav = engine::capture(&path)?;
        (f64::from(wav.sample_rate), "BinModem - scope")
    };

    let (tx, rx) = telemetry::channel(SCOPE_LEN, SPECTRUM_BINS, sample_rate);
    // A quarter second of monitor buffer: enough to ride out scheduling jitter
    // without adding latency you can hear against the scopes.
    let sink = Arc::new(AudioSink::new((sample_rate * 0.25) as usize));

    let (engine, source) = if let Some(host) = args.telnet {
        let session = Arc::new(net::Session::default());
        if let Some(host) = host {
            session.connect(&host);
        }
        let handle = net::spawn(tx, control.clone(), session.clone());
        (handle, Source::Telnet(session))
    } else if args.live {
        let session = Arc::new(live::Session::default());
        // Named devices open straight away; without them the window opens with
        // the modem on the desk and no line in it, and the line panel is where
        // one gets chosen.
        // Named devices are asked for here; the line thread opens the two
        // cables by itself when they are not, because finding them means
        // enumerating audio devices and the thread that opens one is the right
        // place to go looking.
        if let (Some(input), Some(output)) = (&args.input, &args.output) {
            session.open(input, output);
        }
        let handle = live::spawn(tx, control.clone(), session.clone(), sink.clone());
        (handle, Source::Live(session))
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
