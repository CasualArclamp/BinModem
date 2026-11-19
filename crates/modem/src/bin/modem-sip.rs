//! A modem on a SIP call, with nobody at the keyboard and no window.
//!
//! The window can place a call over SIP, but a window is no use to a script.
//! This is the same loop with the window taken off: an account out of a file,
//! lines typed at the modem as if a terminal had typed them, and everything
//! the modem and the call say printed with the time on it. It exists to run
//! calls against another softmodem on the same machine -- D-Modem's `dmodemd`
//! answering on a port of its own, say -- where the far end is a different
//! reading of the same Recommendations and the transcript is the result.
//!
//! ```text
//! modem-sip --accounts sip.txt --account dmodem \
//!     --type "AT+MS=V34" --type "ATD sip:modem@127.0.0.1:5070" \
//!     --send "hello" --seconds 90
//! ```
//!
//! What the terminal would have shown goes to stdout; the notes about the
//! call go to stderr. It exits 0 if the call reached data mode and 1 if not.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use modem::{Modem, State};

/// The rate the modem runs at on a SIP line, which is the window's.
const FS: f64 = 16_000.0;

/// How hard to drive the line unless told: the window's own default, twenty
/// decibels down from what the modem hands over.
///
/// The modem's transmitters leave at a root mean square of 0.707 and the
/// shaped constellations peak close to 2, and G.711 has nothing above one.
/// Written out as it stood, V.34's training went down the line with one
/// sample in six flat against the top of the scale: two of these calling each
/// other on one machine heard each other at about twenty decibels, settled on
/// 12000 bit/s, and lost MP' or E to the clipping more often than not, which
/// the terminal was told as NO CARRIER.
const DEFAULT_DRIVE: f32 = 0.1;

fn main() -> ExitCode {
    let mut file: Option<PathBuf> = None;
    let mut name: Option<String> = None;
    let mut typed: Vec<String> = Vec::new();
    let mut send: Option<String> = None;
    let mut seconds = 120.0f64;
    let mut hold = 10.0f64;
    let mut fax: Option<Fax> = None;
    let mut record: Option<PathBuf> = None;
    let mut answer = false;
    let mut memos: Option<PathBuf> = None;
    let mut drive = DEFAULT_DRIVE;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--accounts" => file = Some(PathBuf::from(value())),
            "--account" => name = Some(value()),
            "--type" => typed.push(value()),
            "--send" => send = Some(value()),
            "--seconds" => seconds = value().parse().unwrap_or(seconds),
            "--hold" => hold = value().parse().unwrap_or(hold),
            // A fax rather than a data call: a test page, sent when the
            // script's ATD dials. g3 keeps to V.17 and below; v34 offers
            // Super G3 as well.
            "--fax" => {
                fax = Some(match value().as_str() {
                    "g3" => Fax { v34: false, pages: 1 },
                    "v34" => Fax { v34: true, pages: 1 },
                    other => {
                        eprintln!("--fax is g3 or v34, not {other}");
                        return ExitCode::FAILURE;
                    }
                });
            }
            "--record" => record = Some(PathBuf::from(value())),
            // The other way round: wait for a call and take it with ATA.
            "--answer" => answer = true,
            "--memos" => memos = Some(PathBuf::from(value())),
            "--drive" => {
                let text = value();
                match text.parse::<f32>() {
                    Ok(d) if d > 0.0 && d <= 1.0 => drive = d,
                    _ => {
                        eprintln!("--drive is a multiple above 0 and up to 1, not {text}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--pages" => {
                if let Some(f) = fax.as_mut() {
                    f.pages = value().parse().unwrap_or(1);
                }
            }
            "--help" | "-h" => {
                println!(
                    "modem-sip [--accounts <file>] --account <name> --type <line>... \n\
                     \x20         [--send <text>] [--hold <s>] [--seconds <s>]\n\
                     \x20         [--fax g3|v34 [--pages <n>]] [--record <wav>] [--answer]\n\
                     \x20         [--memos <file>] [--drive <multiple>]\n\
                     \n\
                     Opens the SIP account, types each line at the modem a second apart,\n\
                     and prints what comes back. Once connected it sends --send, stays up\n\
                     for --hold seconds and hangs up. --seconds bounds the whole run.\n\
                     Without --accounts the accounts are the window's own file.\n\
                     --memos keeps what V.92 calls teach in a file, by the string dialled,\n\
                     so that the next call to the same place starts up the short way.\n\
                     --drive is how hard to drive the line, as a multiple of what the\n\
                     modem hands over: 0.1 unless told, which is the window's own.\n\
                     With --fax the modem is a fax machine with a test page to send:\n\
                     type AT+FCLASS=1 and the ATD, and it exits 0 if a page went."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let accounts = match &file {
        Some(path) => sip::Account::load(path),
        None => sip::Account::load_default(),
    };
    let accounts = match accounts {
        Ok(a) => a,
        Err(e) => {
            eprintln!("the account file will not read: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(account) = accounts
        .into_iter()
        .find(|a| name.as_deref().is_none_or(|n| a.name == n))
    else {
        eprintln!("no such account; --account names a [section] of the file");
        return ExitCode::FAILURE;
    };
    let line = match sip::Line::open(account, FS) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not open the line: {e}");
            return ExitCode::FAILURE;
        }
    };

    run(&line, typed, send, hold, seconds, fax, record, answer, memos, drive)
}

/// A fax to send instead of a data call.
struct Fax {
    v34: bool,
    pages: usize,
}

/// A page with something on it to recognise at the far end: a frame, a
/// diagonal, a ladder of bars that get coarser, and a block that counts the
/// sheet in binary. Nothing here needs a file.
fn test_page(sheet: usize) -> fax::page::Page {
    const WIDTH: usize = 1728;
    const HEIGHT: usize = 600;
    let lines = (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| {
                    let frame = !(8..WIDTH - 8).contains(&x) || !(8..HEIGHT - 8).contains(&y);
                    let diagonal = (x as i64 - (y as i64) * 2).rem_euclid(432) < 6;
                    let bars = (100..220).contains(&y) && {
                        let period = 4usize << (x / 288);
                        x % period < period / 2
                    };
                    let count = (300..380).contains(&y)
                        && (200..840).contains(&x)
                        && ((sheet + 1) >> ((x - 200) / 80)) & 1 == 1
                        && (x - 200) % 80 < 60;
                    frame || diagonal || bars || count
                })
                .collect()
        })
        .collect();
    fax::page::Page { lines, resolution: fax::page::Resolution::Standard }
}

// One argument for each thing the command line can ask for.
#[allow(clippy::too_many_arguments)]
fn run(
    line: &sip::Line,
    typed: Vec<String>,
    send: Option<String>,
    hold: f64,
    seconds: f64,
    fax: Option<Fax>,
    record: Option<PathBuf>,
    answer: bool,
    memos: Option<PathBuf>,
    drive: f32,
) -> ExitCode {
    let started = Instant::now();
    let note = |text: &str| eprintln!("[{:7.2}] {text}", started.elapsed().as_secs_f64());

    let mut modem = Modem::new(FS);
    // V.92's recognised connections, by dial string: a second run with the
    // same file and the same ATD starts up the short way.
    if let Some(path) = memos {
        modem.remember_connections(Some(modem::memos::Memos::open(path)));
    }
    let faxing = fax.is_some();
    if let Some(fax) = &fax {
        modem.fax_identification = "BINMODEM".to_owned();
        modem.fax_v34 = fax.v34;
        modem.fax_pages = (0..fax.pages.max(1)).map(test_page).collect();
    }
    let mut fax_said = String::new();
    let mut fax_last = "";
    let mut fax_awaited_receipt = false;
    let mut typed = typed.into_iter();
    let mut next_line = Instant::now() + Duration::from_millis(500);

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut from_network: Vec<f32> = Vec::with_capacity(2048);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut was = State::Command;
    let mut phase = "";
    let mut call_active = false;
    let mut had_the_call = false;
    let mut asked_to_hang_up = false;
    let mut connected_at: Option<Instant> = None;
    let mut reached_data = false;
    let mut over_at: Option<Instant> = None;
    let mut received = 0usize;
    // What arrived and what was sent, a pair to a sample, as the window keeps it.
    let mut recording: Vec<f32> = Vec::new();

    loop {
        // The lines somebody would have typed, a second apart so that each
        // has its answer before the next begins.
        if Instant::now() >= next_line
            && let Some(text) = typed.next()
        {
            note(&format!("typed: {text}"));
            for byte in text.bytes().chain(std::iter::once(b'\r')) {
                modem.feed_dte(byte);
            }
            next_line = Instant::now() + Duration::from_secs(1);
        }

        // The call, which has to be placed before there is a line at all.
        for said in line.poll() {
            note(&said);
            if answer && said.contains("ATA to answer") {
                note("typed: ATA");
                for byte in *b"ATA" {
                    modem.feed_dte(byte);
                }
                modem.feed_dte(13);
            }
        }
        if let Some(number) = modem.take_dial_request() {
            line.dial(&number);
        }
        if modem.take_answer_request() {
            line.answer();
        }
        let was_active = call_active;
        call_active = line.up() || sip::ua::state::is_a_call(&line.status().call);
        let busy = modem.state() != State::Command;
        if busy && call_active {
            had_the_call = true;
        }
        if was_active && !call_active && busy {
            note("the call is over; telling the modem");
            modem.carrier_lost();
        } else if call_active && !busy && had_the_call && !asked_to_hang_up {
            note("the modem has hung up; ending the call");
            line.hang_up();
            asked_to_hang_up = true;
        }
        if !call_active {
            if had_the_call && over_at.is_none() {
                over_at = Some(Instant::now());
            }
            had_the_call = false;
            asked_to_hang_up = false;
        }

        // The line is the clock, as it is with a sound card.
        from_line.clear();
        from_network.clear();
        line.receive_both(&mut from_line, &mut from_network);
        if from_line.is_empty() {
            std::thread::sleep(Duration::from_millis(2));
        } else {
            // A V.ARC call wants a call that carries linear samples. On
            // one, the network's own samples are the modem's line and what
            // it says goes back untouched; on any other it is an ordinary
            // call after all.
            if modem.varc_wanted().is_some() && !line.linear() {
                modem.varc_fall_back();
            }
            let levels: Vec<f64> = from_network.iter().map(|l| f64::from(*l)).collect();
            to_line.clear();
            if modem.varc_wanted().is_some() {
                modem.varc_exact_line(true);
                let mut exact: Vec<f32> = Vec::with_capacity(levels.len());
                for &level in &levels {
                    let mut out = [0.0];
                    modem.varc_step(sip::media::LINEAR_RATE, &[level], &mut out);
                    exact.push(out[0] as f32);
                }
                to_line.extend_from_slice(&exact);
                line.transmit_exact(&exact);
                for text in modem.take_line_notes() {
                    note(&text);
                }
            } else {
                // A digital modem reads the network's own levels (it ignores
                // them otherwise).
                modem.give_network_levels(&levels);
                for &s in &from_line {
                    let said = modem.step(f64::from(s)) as f32;
                    // Levels that are exact are codewords or V.92's LU, and
                    // no drive of any other size will do: see
                    // `Modem::exact_levels`.
                    let drive = if modem.exact_levels() { 1.0 } else { drive };
                    to_line.push(said * drive);
                }
                // A digital modem's codewords go as they are; everything else
                // is a waveform and goes through the rate conversion.
                match modem.take_network_levels() {
                    Some(levels) => {
                        let levels: Vec<f32> = levels.iter().map(|l| *l as f32).collect();
                        line.transmit_exact(&levels);
                    }
                    None => line.transmit(&to_line),
                }
            }
            if record.is_some() {
                for (heard, sent) in from_line.iter().zip(to_line.iter()) {
                    recording.push(*heard);
                    recording.push(*sent);
                }
            }
        }

        let out = modem.take_dte();
        if !out.is_empty() {
            received += out.len();
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&out);
            let _ = stdout.flush();
        }

        for said in modem.take_line_notes() {
            note(&format!("  line: {said}"));
        }
        if modem.line_phase() != phase {
            phase = modem.line_phase();
            note(&format!("phase: {phase}"));
        }
        // A fax call has phases of its own, and they are the transcript.
        if let Some(call) = modem.fax_call() {
            let now = format!(
                "fax: {} | sheet {}/{}, {} bit/s{}{}{}",
                call.phase_name(),
                call.sheet(),
                call.sheets(),
                call.rate(),
                call.symbol_rate().map(|b| format!(" on {b} baud")).unwrap_or_default(),
                if call.error_correction() { ", ECM" } else { "" },
                call.trouble().map(|t| format!(" -- {t}")).unwrap_or_default(),
            );
            fax_awaited_receipt |= call.phase() == fax::call::Phase::AwaitingReceipt;
            fax_last = call.phase_name();
            if now != fax_said {
                note(&now);
                fax_said = now;
            }
        }
        if modem.state() != was {
            was = modem.state();
            let detail = match modem.rate() {
                Some(r) => format!(
                    ", {r} bit/s, {}{}{}{}",
                    if modem.varc_wanted().is_some() {
                        "its own retransmission"
                    } else if modem.error_controlled() {
                        "V.42"
                    } else {
                        "no error control"
                    },
                    if modem.compressing() { " with compression" } else { "" },
                    if modem.quick_connected().is_some() { ", short phase 1" } else { "" },
                    if modem.short_phase2() { ", short phase 2" } else { "" }
                ),
                None => String::new(),
            };
            note(&format!("modem: {was:?}{detail}"));
            if was == State::Data && connected_at.is_none() {
                connected_at = Some(Instant::now());
                reached_data = true;
                if let Some(text) = &send {
                    for byte in text.bytes().chain(*b"\r\n") {
                        modem.feed_dte(byte);
                    }
                }
            }
        }

        // Up for long enough: hang up the way ATH would.
        if connected_at.is_some_and(|at| at.elapsed().as_secs_f64() >= hold)
            && modem.state() != State::Command
        {
            note("held long enough; hanging up");
            modem.hang_up();
            connected_at = None;
        }
        // A moment after the call is over, so the BYE and its answer cross.
        if over_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(1)) {
            break;
        }
        if started.elapsed().as_secs_f64() >= seconds {
            note("out of time");
            line.hang_up();
            std::thread::sleep(Duration::from_millis(300));
            break;
        }
    }

    if let Some(path) = &record {
        match line::wav::write_channels(path, &recording, 2, FS as u32) {
            Ok(()) => note(&format!(
                "kept {:.1} s as {}",
                recording.len() as f64 / 2.0 / FS,
                path.display()
            )),
            Err(e) => note(&format!("could not write the recording: {e}")),
        }
    }
    let stats = line.stats();
    note(&format!("{received} octets came to the terminal; line: {stats:?}"));
    if faxing {
        // The last thing the call was doing says how it ended; the far end's
        // own log is the proof that the page arrived.
        note(&format!(
            "fax: ended in \"{fax_last}\"; a page was {}sent to its end",
            if fax_awaited_receipt { "" } else { "not " }
        ));
        return if fax_awaited_receipt { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    if reached_data { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
