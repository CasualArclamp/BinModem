//! Replay a recorded call through the whole modem, not just the data pump.
//!
//! Ignored, because it needs a capture and captures are not in the repository:
//! they are large, and the ones worth having come off somebody's telephone
//! line. Point it at one with
//!
//! ```text
//! MODEM_CAPTURE=dist/captures/live-1788741621.wav cargo test -p modem \
//!     --test replay -- --ignored --nocapture
//! ```
//!
//! The data pump has its own replay, which answers whether the receiver could
//! read the far end. This answers the other question, and the one a call that
//! went wrong actually raises: what the modem above it decided, and when. The
//! two ends of a handshake and everything on top of them -- V.8, the start-up,
//! the detection phase, XID, LAPM -- each of which can be the one that gave
//! up, and none of which is visible from a waveform.
//!
//! Only the first channel is fed in. The modem is being asked to do exactly
//! what it did on the day, with the far end's own signal.

use modem::Modem;

const FS: f64 = 16_000.0;

fn capture() -> Option<Vec<f32>> {
    let path = std::env::var("MODEM_CAPTURE").ok()?;
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "the modem is built for 16 kHz");
    println!(
        "\n{path}: {:.1} s, {} channels at {} Hz",
        wav.duration_secs(),
        wav.channels,
        wav.sample_rate
    );
    Some(wav.channel(0))
}

#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_replay_call() {
    let Some(line) = capture() else {
        println!("set MODEM_CAPTURE to a recording to run this");
        return;
    };
    // The command line the window sends, so that the replay is the same modem
    // the call was placed with rather than a default one.
    let commands = std::env::var("MODEM_COMMANDS")
        .unwrap_or_else(|_| "AT+MS=V22B,1,1200,2400".to_owned());

    let mut modem = Modem::new(FS);
    for command in commands.split(';') {
        for b in command.trim().bytes() {
            modem.feed_dte(b);
        }
        modem.feed_dte(b'\r');
    }
    modem.take_dte();
    for b in b"ATD\r" {
        modem.feed_dte(*b);
    }

    let mut dte = Vec::new();
    let (mut state, mut phase, mut ec) = (modem.state(), "", "");
    let mut damaged = 0u64;
    println!("  {:>7}  {:<14} {:<26} {:<13} rate", "t", "state", "line", "V.42");
    for (i, s) in line.iter().enumerate() {
        modem.step(f64::from(*s));
        dte.extend(modem.take_dte());
        for text in modem.take_line_notes() {
            println!("  {:>7.3}  {text}", i as f64 / FS);
        }
        let now = (modem.state(), modem.line_phase(), modem.error_control_phase());
        if now != (state, phase, ec) || modem.damaged_frames() != damaged {
            damaged = modem.damaged_frames();
            let shown = format!("{:?}", now.0);
            let line = format!("{} {}", modem.standard(), now.1);
            let rate = modem.rate().map_or_else(|| "-".to_owned(), |r| r.to_string());
            let damage =
                if damaged > 0 { format!("  {damaged} damaged") } else { String::new() };
            println!(
                "  {:>7.3}  {shown:<14} {line:<26} {:<13} {rate}{damage}",
                i as f64 / FS,
                if now.2.is_empty() { "-" } else { now.2 },
            );
            (state, phase, ec) = now;
        }
    }

    // What the far end said about itself, which is the other half of any
    // question about why a call went the way it did.
    let distant = modem.distant();
    if distant.is_empty() {
        println!("
  the far end said nothing about itself");
    } else {
        println!("
  the far end:");
        for (what, value) in distant {
            println!("    {what:<22} {value}");
        }
    }

    println!("\n  the terminal was told:");
    for line in String::from_utf8_lossy(&dte).lines() {
        if !line.trim().is_empty() {
            println!("    {line}");
        }
    }
    println!(
        "\n  ended in {:?}, {} damaged frames\n",
        modem.state(),
        modem.damaged_frames()
    );
}

/// Every frame the far end sent, once the carriers were up.
///
/// The layer between the two replays. `probe_replay_call` says what this
/// modem decided; the data pump's own replay says whether the waveform could
/// be read. Neither says what the far end actually put on the line, and a link
/// that establishes and then carries nothing is a question about exactly that.
#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_replay_frames() {
    use datapump::v22bis::handshake::{Modem as Pump, Role, Status};
    use ec::frame::{Frame, Role as EcRole};
    use ec::hdlc::{Decoder, Fcs, FrameError};

    let Some(line) = capture() else {
        println!("set MODEM_CAPTURE to a recording to run this");
        return;
    };
    // The real pump is built after V.8 hands over, not at the start of the
    // recording. A receiver that spent twenty seconds training on an answer
    // tone and a V.21 menu is not the receiver the call had, and what it makes
    // of the data afterwards is its own.
    let skip: f64 = std::env::var("MODEM_SKIP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let line = &line[((skip * FS) as usize).min(line.len())..];
    if skip > 0.0 {
        println!("  starting the receiver at {skip:.3} s, where V.8 handed over");
    }
    let mut pump = Pump::new(Role::Calling, FS);
    let mut decoder = Decoder::new(Fcs::Bits16);
    // The far end may have agreed 32-bit checking, and a decoder fixed at the
    // wrong width calls every frame corrupt.
    decoder.accept_either();

    // Everything the receiver recovered, as it recovered it, for when the
    // frames do not decode and the question becomes what is actually on the
    // line. A link that establishes and then carries nothing has its answer
    // there, and no layer above this one is going to show it.
    let dump = std::env::var("MODEM_BITS").ok();
    let mut raw: Vec<bool> = Vec::new();

    let mut connected = None;
    let (mut good, mut bad, mut flags) = (0u64, 0u64, 0u64);
    let mut seen: Vec<(f64, String)> = Vec::new();
    for (i, s) in line.iter().enumerate() {
        pump.step(f64::from(*s));
        let t = i as f64 / FS + skip;
        if connected.is_none() {
            if let Status::Connected(rate) = pump.status() {
                connected = Some(t);
                println!("\n  carriers up at {t:.3} s, {} bit/s", rate.bits_per_second());
                // The training is not the connection: what the receiver made
                // of it is thousands of bits of noise, and feeding them to a
                // deframer is how a call finds frames in its own warm-up.
                pump.take_bits();
            }
            continue;
        }
        for bit in pump.take_bits() {
            if dump.is_some() {
                raw.push(bit);
            }
            match decoder.feed(bit) {
                Some(Ok(body)) => {
                    good += 1;
                    let what = match Frame::decode(&body, EcRole::Originator) {
                        // The address octet as it arrived, because the whole
                        // question of who is polling whom turns on one bit of
                        // it and a decoder is exactly what is in doubt.
                        Ok((addr, frame)) => {
                            format!("[{:02x}] {:?} {:?}", body[0], addr.kind, frame)
                        }
                        Err(e) => format!("undecodable: {e:?} {body:02x?}"),
                    };
                    if seen.len() < 40 {
                        seen.push((t, what));
                    }
                }
                Some(Err(FrameError::BadFcs)) => bad += 1,
                Some(Err(_)) => flags += 1,
                None => {}
            }
        }
    }

    if connected.is_none() {
        println!("\n  the carriers never came up\n");
        return;
    }
    println!("\n  {:>8}  frame", "t");
    for (t, what) in &seen {
        println!("  {t:>8.3}  {what}");
    }
    println!(
        "\n  {good} frames checked out, {bad} failed their check sequence, \
         {flags} were malformed"
    );
    if let Some(path) = dump {
        // Written both ways round. V.42 8.1.1 sends the low-order bit of each
        // octet first, which is not the order HDLC is drawn in, and which of
        // the two a dump turns out to be readable in says something itself.
        let pack = |msb_first: bool| -> Vec<u8> {
            raw.chunks(8)
                .map(|c| {
                    c.iter().enumerate().fold(0u8, |b, (i, &v)| {
                        let at = if msb_first { 7 - i } else { i };
                        b | (u8::from(v) << at)
                    })
                })
                .collect()
        };
        for (suffix, msb) in [("lsb", false), ("msb", true)] {
            let name = format!("{path}.{suffix}");
            match std::fs::write(&name, pack(msb)) {
                Ok(()) => println!("  {} bits written to {name}", raw.len()),
                Err(e) => println!("  could not write {name}: {e}"),
            }
        }
    }
    println!();
}

/// Replay a raw s16le capture through the whole modem, and keep what its DTE
/// was handed.
///
/// The interop rigs write the far end's own transmit stream as raw s16le at
/// 8000 Hz (`agr-tx.raw`), and the bench's own DTE log is the answer this has
/// to reproduce: a call whose bytes are wrong on the bench is a call that can
/// be run again here in a second instead of forty, with the waveform held
/// still while the decoder is changed underneath it.
///
/// ```text
/// MODEM_RAW=/tmp/agr-v90-interop/<run>/agr-tx.raw \
///     cargo test -p modem --test replay -- --ignored --nocapture probe_replay_raw
/// ```
#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_replay_raw() {
    let Ok(path) = std::env::var("MODEM_RAW") else {
        println!("set MODEM_RAW to a raw s16le capture to run this");
        return;
    };
    let fs: f64 = std::env::var("MODEM_FS").ok().and_then(|s| s.parse().ok()).unwrap_or(8000.0);
    let bytes = std::fs::read(&path).expect("could not read the capture");
    let line: Vec<f64> = bytes
        .as_chunks::<2>().0.iter()
        .map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
        .collect();
    println!("\n{path}: {:.1} s at {fs} Hz", line.len() as f64 / fs);

    let ms = std::env::var("MODEM_MS").unwrap_or_else(|_| "V92".to_owned());
    // When this end went off-hook, which the rig holds back so that the caller
    // has finished dialling (`--answer-delay`, 3.4 s by default). A replay that
    // answers at once puts its ANSam in before the far end's V.8 has begun.
    let answer_at: f64 = std::env::var("MODEM_ANSWER_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(3.4);
    let mut modem = Modem::new(fs);
    let say = |modem: &mut Modem, bytes: &[u8]| {
        for b in bytes {
            modem.feed_dte(*b);
        }
        let _ = modem.take_dte();
    };
    say(&mut modem, format!("AT+MS={ms}\r").as_bytes());

    let mut dte: Vec<u8> = Vec::new();
    let (mut state, mut phase) = (modem.state(), String::new());
    let mut shown = 0usize;
    let mut ec_said = String::new();
    let mut answered = false;
    for (i, &s) in line.iter().enumerate() {
        if !answered && i as f64 / fs >= answer_at {
            answered = true;
            say(&mut modem, b"ATA\r");
        }
        modem.step(s);
        dte.extend(modem.take_dte());
        for text in modem.take_line_notes() {
            println!("  {:>7.3}  {text}", i as f64 / fs);
        }
        let now = (modem.state(), modem.line_phase().to_owned());
        let ec_now = modem.error_control_phase();
        if ec_now != ec_said.as_str() {
            ec_said = ec_now.to_owned();
            println!(
                "  {:>7.3}  error control: phase {:?}, negotiated {}, connected {}, compression {:?}, {:?}",
                i as f64 / fs,
                ec_now,
                modem.error_control_negotiated(),
                modem.error_controlled(),
                modem.compression_name(),
                modem.state()
            );
        }
        if now != (state, phase.clone()) {
            let rate = modem.rate().map_or_else(|| "-".to_owned(), |r| r.to_string());
            let up = modem.transmit_rate().map_or_else(|| "-".to_owned(), |r| r.to_string());
            println!(
                "  {:>7.3}  {:?} {} {} down {rate} / up {up}",
                i as f64 / fs,
                now.0,
                modem.standard(),
                now.1
            );
            (state, phase) = now;
        }
        // As it arrives, in window, so that a decoder that repeats says so.
        if dte.len() >= shown + 16 {
            let tail = &dte[shown..dte.len().min(shown + 16)];
            let hex: String = tail.iter().map(|b| format!("{b:02x}")).collect();
            println!("  {:>7.3}  dte {shown}: {hex}", i as f64 / fs);
            shown = dte.len();
        }
    }
    // The end of the question, in one line: a call whose V.42 detection went
    // transparent, or whose XID never agreed a compressor, carries the DTE's
    // characters over a link with nothing protecting them -- and, when the
    // physical layer is what is wrong, carries somebody else's.
    println!(
        "\n  error control at the end: {:?}, V.44 {:?}, V.42 {:?}",
        modem.error_control_phase(),
        modem.compression_name(),
        modem.error_control_detail()
    );
    println!("  {} bytes to the DTE", dte.len());
    if let Ok(out) = std::env::var("MODEM_DTE_OUT") {
        let _ = std::fs::write(&out, &dte);
        println!("  written to {out}");
    }
    println!("  ended in {:?}, {} damaged frames\n", modem.state(), modem.damaged_frames());
}
