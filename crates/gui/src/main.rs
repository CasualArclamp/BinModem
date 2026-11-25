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
//!   binmodem --com <port> [--telnet-in <port>] a modem on a COM port for DOSBox
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

// No console window behind the program's own when it is opened from Explorer;
// `parent_console` gives back the output a shell expects.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(target_os = "windows", target_arch = "x86"))]
mod xp_compat;

mod answer;
mod app;
mod bbs;
mod callprompt;
mod comport;
mod crashlog;
mod dialin;
mod document;
mod faxwin;
mod console;
mod contacts;
mod engine;
mod live;
mod net;
mod network;
mod parent_console;
mod remembered;
mod review;
mod scopes;
mod speed;
mod tap;
mod window;

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

/// The SIP accounts the file holds, and where the file is.
///
/// Printed with the path, because the answer to "there are no accounts" is
/// always a path and never a list.
fn list_accounts() {
    match sip::Account::default_path() {
        Some(path) => {
            let _ = sip::Account::write_example(&path);
            println!("accounts (--sip), from {}:", path.display());
        }
        None => println!("accounts (--sip):"),
    }
    match sip::Account::load_default() {
        Ok(accounts) if accounts.is_empty() => {
            println!("  none yet; the file above has a commented example in it");
        }
        Ok(accounts) => {
            for account in accounts {
                println!(
                    "  {}  {} through {}{}",
                    account.name,
                    account.uri(),
                    account.next_hop(),
                    if account.register { "" } else { " (no registration)" }
                );
            }
        }
        Err(e) => println!("  the account file will not read: {e}"),
    }
}

struct Args {
    path: Option<PathBuf>,
    /// Replay the capture carried inside the program.
    golden: bool,
    live: bool,
    input: Option<String>,
    output: Option<String>,
    /// A SIP account to open the line on instead of a pair of audio devices.
    sip: Option<String>,
    /// Whether to open a terminal onto a socket instead of a modem, and where
    /// to point it. `Some(None)` is the mode with the host chosen in the
    /// window, which is the usual way in.
    telnet: Option<Option<String>>,
    /// Lines to type at the modem once its line is open, in order, and
    /// whether to open on the scopes: for a call with nobody at the keyboard.
    typed: Vec<String>,
    scopes: bool,
    /// Or on the Fax tab, which only a live line has.
    fax: bool,
    /// Answer a ringing SIP call as this, after it has rung a moment.
    answer_as: Option<String>,
    /// Whether the Setup row stays showing once a line is open: for a
    /// picture of a window that shows what line it is on.
    setup: bool,
    /// Record the line from the start, as the window's Record key would: kept
    /// in `captures` when the window closes or the key is pressed.
    record: bool,
    /// Open on the Review tab; with a recording to read there, if one was
    /// named.
    review: Option<Option<PathBuf>>,
}

fn parse() -> Result<Option<Args>, String> {
    let mut args = Args {
        path: None,
        golden: false,
        live: false,
        input: None,
        output: None,
        sip: None,
        telnet: None,
        typed: Vec::new(),
        scopes: false,
        fax: false,
        answer_as: None,
        setup: false,
        record: false,
        review: None,
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
            // A line with no sound card in it at all. Implies --live,
            // because there is nothing else it could mean.
            "--sip" => {
                args.sip = Some(value("--sip")?);
                args.live = true;
            }
            "--accounts" => {
                list_accounts();
                return Ok(None);
            }
            "--type" => args.typed.push(value("--type")?),
            "--scopes" => args.scopes = true,
            "--fax" => args.fax = true,
            "--answer-as" => args.answer_as = Some(value("--answer-as")?),
            "--setup" => args.setup = true,
            "--record" => args.record = true,
            // The recording is optional: without one the tab opens empty,
            // and one is opened or dropped there.
            "--review" => {
                let path = rest.clone().next().filter(|p| !p.starts_with('-'));
                if path.is_some() {
                    rest.next();
                }
                args.review = Some(path.map(PathBuf::from));
                args.live = true;
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
                     binmodem ... --type <line> [--scopes]      typed at the modem once the line is open\n\
                     binmodem ... --fax                         open on the Fax tab, on a live line\n\
                     binmodem ... --answer-as fax|modem         press that answer when a SIP call rings\n\
                     binmodem ... --record                      record the line from the start, into captures\n\
                     binmodem ... --setup                       leave the Setup row showing with a line open\n\
                     binmodem --review [path.wav]               read a recording of a call: what was said, and when\n\
                     binmodem [path.wav]                        replay a capture\n\
                     binmodem --capture                         replay the Bell 103 golden vector\n\
                     binmodem --devices                         list audio devices\n\
                     binmodem --sip <account>                   a call over SIP, with no sound card\n\
                     binmodem --accounts                        list the SIP accounts there are\n\
                     binmodem --telnet [host]                   a board over a socket, no modem\n\
                     binmodem --answer --in <dev> --out <dev>   a board to dial, on the same cable\n\
                     binmodem --com <port> [--telnet-in <port>] a modem on a COM port for DOSBox"
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
    // Before anything at all, so that whatever goes wrong next leaves a line
    // behind saying where. Nothing else in here writes anything down.
    crashlog::install();
    parent_console::attach();

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

    // So is the modem on a COM port, which has no window either.
    if raw.first().is_some_and(|a| a == "--com") {
        let code = comport::run(raw.clone());
        std::process::exit(i32::from(code != std::process::ExitCode::SUCCESS));
    }

    let Some(mut args) = parse()? else { return Ok(()) };
    // Taken now, because `args` is picked apart below.
    let (typed, scopes, fax, setup) = (std::mem::take(&mut args.typed), args.scopes, args.fax, args.setup);
    let answer_as = args.answer_as.take();
    let review = args.review.take();

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
        session.set_recording(args.record);
        // Named devices open straight away; without them the window opens with
        // the modem on the desk and no line in it, and the line panel is where
        // one gets chosen.
        // Named devices are asked for here; the line thread opens the two
        // cables by itself when they are not, because finding them means
        // enumerating audio devices and the thread that opens one is the right
        // place to go looking.
        if let Some(account) = &args.sip {
            session.open_sip(account);
        } else if let (Some(input), Some(output)) = (&args.input, &args.output) {
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

    let options = || eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 860.0])
            .with_min_inner_size([900.0, 640.0])
            .with_title(title),
        ..Default::default()
    };

    let ui_control = control.clone();
    window::run(
        title,
        options,
        move |cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            let mut app = app::ScopeApp::new(rx, ui_control, sink, sample_rate, source);
            app.script(typed, scopes);
            app.open_fax_tab(fax);
            if let Some(how) = answer_as.as_deref() {
                app.answer_as(how)?;
            }
            app.keep_setup_showing(setup);
            if let Some(recording) = &review {
                app.open_review(recording.as_deref());
            }
            Ok(Box::new(app))
        },
    )?;

    control.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = engine.join();
    Ok(())
}
