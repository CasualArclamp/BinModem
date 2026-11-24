//! BinModem as a V.90 digital modem, on a pipe instead of a sound card.
//!
//! This is the answering half of a V.90 call driven from the outside: PCM in
//! on stdin and PCM out on stdout, one block at a time, so that a modem built
//! from the Recommendation can be placed on a line somebody else owns. It is
//! the same modem as `--answer` in the window -- V.8's answer, the V.90
//! digital start-up, V.42 -- with the audio device replaced by two pipes.
//!
//! The format is what the sound card would carry: s16 little-endian, mono, at
//! `--fs`. Nothing else goes to stdout. What the DTE would see goes to stderr,
//! and to `--log` when one is given.
//!
//! ```text
//! v90-bridge --fs 9600 --log /tmp/binmodem.log < line.pcm > line.pcm
//! ```

use std::io::{Read, Write};
use std::process::ExitCode;

use modem::{Modem, State};

/// Where what the DTE would see is written: stderr for a person watching,
/// a file for whatever reads it afterwards, and both when there is a file.
struct Log {
    file: Box<dyn Write>,
}

impl Log {
    fn say(&mut self, text: &str) {
        eprintln!("{text}");
        let _ = writeln!(self.file, "{text}");
        let _ = self.file.flush();
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut fs = 9600.0f64;
    let mut log_path: Option<String> = None;
    let mut greeting = "BinModem V.90 digital server\r\n".to_owned();
    let mut greeting_after = 1.0f64;
    let mut hang_after = 40.0f64;
    let mut modulation = "V90".to_owned();
    let mut no_answer = false;
    let mut call = false;
    let mut dte_out: Option<String> = None;

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--fs" => fs = value().parse().unwrap_or(fs),
            "--log" => log_path = Some(value()),
            "--greeting" => greeting = value(),
            "--greeting-after" => greeting_after = value().parse().unwrap_or(greeting_after),
            "--hang-after" => hang_after = value().parse().unwrap_or(hang_after),
            "--mod" => modulation = value().to_ascii_uppercase(),
            "--no-answer" => no_answer = true,
            "--dte-out" => dte_out = Some(value()),
            "--call" => call = true,
            "--help" | "-h" => {
                println!(
                    "v90-bridge [--fs 9600] [--mod V90] [--log <path>] \
                     [--greeting <text>] [--greeting-after <s>] [--hang-after <s>]\n\
                     \n\
                     A V.90 digital answering modem on stdin/stdout: s16 \
                     little-endian mono at --fs, one block in and one out. \
                     Everything the DTE would see goes to stderr and --log."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let file: Box<dyn Write> = match log_path {
        Some(path) => match std::fs::File::create(&path) {
            Ok(f) => Box::new(std::io::BufWriter::new(f)),
            Err(e) => {
                eprintln!("could not open {path}: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => Box::new(std::io::sink()),
    };
    let mut log = Log { file };

    // The data the modem hands its DTE, written as it arrives. A log line has
    // to render it printable and can lose bytes; a caller comparing what it
    // sent with what arrived needs the bytes themselves.
    let mut received: Option<std::fs::File> = match &dte_out {
        Some(path) => match std::fs::File::create(path) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("could not open {path}: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let mut modem = Modem::new(fs);
    log.say(&format!("BinModem V.90 digital modem at {fs} Hz"));
    // BINMODEM_MS overrides the whole +MS argument list, which is how a far
    // end that cannot do V.8 is met: `V92,0` is automode off, so the pump is
    // built from that name alone and no V.8 runs at either end.
    let ms = std::env::var("BINMODEM_MS").unwrap_or_else(|_| modulation.clone());
    for byte in format!("AT+MS={ms}\r").bytes() {
        modem.feed_dte(byte);
    }
    drain(&mut modem, &mut log);
    if call {
        for byte in b"ATD5551234\r" {
            modem.feed_dte(*byte);
        }
        drain(&mut modem, &mut log);
    } else if !no_answer {
        for byte in b"ATA\r" {
            modem.feed_dte(*byte);
        }
        drain(&mut modem, &mut log);
    }

    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut in_bytes = vec![0u8; 8192];
    let mut out_bytes: Vec<u8> = Vec::with_capacity(8192);
    let mut samples = 0u64;
    let mut elapsed = 0.0f64;
    let mut last_state = (modem.state(), String::new(), String::new());
    let mut sent_greeting = false;
    let mut hung_up = false;
    let trace = std::env::var_os("MODEM_TRACE").is_some();
    let mut seen_dte: Vec<u8> = Vec::new();
    let mut line: Vec<f64> = Vec::with_capacity(4096);

    loop {
        let want = match stdin.read(&mut in_bytes) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                log.say(&format!("stdin: {e}"));
                return ExitCode::FAILURE;
            }
        };
        let n = want / 2;
        line.clear();
        for i in 0..n {
            let v = i16::from_le_bytes([in_bytes[2 * i], in_bytes[2 * i + 1]]);
            line.push(f64::from(v) / 32768.0);
        }

        out_bytes.clear();
        for &x in &line {
            let y = modem.step(x);
            let v = (y * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            out_bytes.extend_from_slice(&v.to_le_bytes());
        }
        if let Err(e) = stdout.write_all(&out_bytes).and_then(|()| stdout.flush()) {
            log.say(&format!("stdout: {e}"));
            return ExitCode::FAILURE;
        }

        samples += n as u64;
        elapsed = samples as f64 / fs;

        let data = drain(&mut modem, &mut log);
        if let Some(file) = received.as_mut()
            && modem.state() == State::Data
        {
            let _ = file.write_all(&data);
        }
        seen_dte.extend(data);
        for note in modem.take_line_notes() {
            log.say(&format!("{elapsed:8.3}  note: {note}"));
        }
        if trace {
            for crossed in modem.take_frame_log() {
                let hex: String = crossed.body.iter().map(|b| format!("{b:02x}")).collect();
                log.say(&format!(
                    "{elapsed:8.3}  frame {} {} {hex}",
                    if crossed.outbound { "out" } else { "in " },
                    if crossed.intact { "ok  " } else { "bad " },
                ));
            }
        }

        let state = modem.state();
        let phase = match modem.phase2_stage() {
            Some(stage) => format!("{} ({stage})", modem.line_phase()),
            None => modem.line_phase().to_owned(),
        };
        let standard = modem.standard().to_owned();
        if (state, &phase, &standard) != (last_state.0, &last_state.1, &last_state.2) {
            let rate = modem.rate().map_or_else(|| "-".to_owned(), |r| r.to_string());
            let up = modem
                .transmit_rate()
                .map_or_else(|| "-".to_owned(), |r| r.to_string());
            log.say(&format!(
                "{elapsed:8.3}  {state:?} {standard} {phase}  down {rate} / up {up}"
            ));
            last_state = (state, phase, standard);
        }

        if state == State::Data && !sent_greeting && elapsed >= greeting_after {
            sent_greeting = true;
            for b in greeting.bytes() {
                modem.feed_dte(b);
            }
            log.say(&format!("{elapsed:8.3}  sent the greeting to the far end"));
        }
        if state == State::Data && elapsed >= hang_after && !hung_up {
            hung_up = true;
            modem.hang_up();
            log.say(&format!("{elapsed:8.3}  hanging up"));
        }
    }

    let text = String::from_utf8_lossy(&seen_dte);
    log.say(&format!("call over after {elapsed:.3} s"));
    log.say(&format!(
        "the DTE side received {} bytes: {:?}",
        seen_dte.len(),
        text
    ));
    ExitCode::SUCCESS
}

/// Take what the modem has said to its DTE since last time, put it in the log,
/// and hand it back so that a bridge can keep the data part.
fn drain(modem: &mut Modem, log: &mut Log) -> Vec<u8> {
    let out = modem.take_dte();
    if !out.is_empty() {
        log.say(&format!("DTE: {:?}", String::from_utf8_lossy(&out)));
    }
    out
}
