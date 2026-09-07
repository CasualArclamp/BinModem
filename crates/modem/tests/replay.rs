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
            match decoder.feed(bit) {
                Some(Ok(body)) => {
                    good += 1;
                    let what = match Frame::decode(&body, EcRole::Originator) {
                        Ok((addr, frame)) => format!("{:?} {:?}", addr.kind, frame),
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
         {flags} were malformed\n"
    );
}
