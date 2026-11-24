//! An emulated U.S. Robotics Courier, running USR's own firmware in
//! courier-emu (github.com/cryan209/courier-emu), calls this modem answering
//! as a V.90 server: the local test peer for x2.
//!
//! courier-emu and its ROM images stay outside this repository; this is only
//! the far end of its line link. Start the Courier listening on a TCP line
//! (see `tools/x2/courier-peer.md`), then:
//!
//! ```text
//! cargo run --release -p modem --example courier_peer -- 127.0.0.1:47123 [--pad 0|3|6] [--no-x2] [--seconds N]
//! ```
//!
//! The line carries the Courier's codec samples, 8000 a second, in frames
//! with its hook and call state (courier-emu's `line.py`): a nine-byte
//! little-endian header (u32 stamp, u8 off hook, u8 ringing, u8 call state,
//! u16 sample count), then the samples as i16. The Courier sends a frame
//! first; each end then waits for the other's. This end is the exchange as
//! well as the server: the Courier's samples go to mu-law at the levels of
//! its codec and line interface (-5.86 dB), this modem's codewords come back
//! through an optional digital pad (decoded, scaled in Q15, the nearest
//! octet again) and are decoded to the Courier's codec (-5.54 dB), the
//! figures libv90modem's bridge used for the same emulator.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use dsp::Resampler;
use modem::{Modem, State};
use sip::g711::{ulaw_decode, ulaw_encode};

const CALL_RINGING: u8 = 2;
const CALL_ANSWERED: u8 = 3;
const CALL_CLEARED: u8 = 4;

/// The Courier's codec to mu-law, and back (Q15).
const TO_ULAW: i32 = 16690;
const TO_COURIER: i32 = 17316;

/// The modem's own rate, as modem-sip runs it: the line is taken up to it and
/// back down, and a digital modem's codewords go round that as they are.
const FS: f64 = 16_000.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut addr = "127.0.0.1:47123".to_owned();
    let mut pad = 0u32;
    let mut x2 = true;
    let mut seconds = 200.0;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pad" => pad = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--no-x2" => x2 = false,
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            other => addr = other.to_owned(),
        }
    }
    let pad_q15: Option<i32> = match pad {
        3 => Some(23198),
        6 => Some(16423),
        _ => None,
    };

    let mut stream = connect(&addr);
    stream.set_nodelay(true).ok();
    let mut modem = Modem::new(FS);
    let mut up = Resampler::new(8000.0, FS);
    let mut down = Resampler::new(FS, 8000.0);
    let (mut wide, mut narrow) = (Vec::new(), Vec::new());
    for line in ["AT+MS=V90", if x2 { "ATS70=1" } else { "ATS70=0" }] {
        for b in line.bytes().chain(*b"\r") {
            modem.feed_dte(b);
        }
    }
    // COURIER_PEER_RECORD=FILE: what went to the Courier and what came from
    // it, interleaved i16 at 8000 Hz.
    let mut record = std::env::var_os("COURIER_PEER_RECORD").map(|p| std::fs::File::create(p).expect("record file"));
    let started = Instant::now();
    let mut frames: u64 = 0;
    let mut answered = false;
    let mut state = 0u8;
    let mut said = Vec::new();
    let mut last = String::new();
    let mut sent_text = 0;
    let mut connect_at: Option<f64> = None;
    loop {
        let Some((courier_off_hook, ringing, call, samples)) = receive(&mut stream) else {
            println!("[{:7.2}] line closed", frames as f64 * 0.02);
            break;
        };
        let now = frames as f64 * samples.len().max(1) as f64 / 8000.0;
        state = state.max(call);
        if !answered && (ringing != 0 || call == CALL_RINGING) {
            println!("[{now:7.2}] the Courier rings us: ATA");
            for b in b"ATA\r" {
                modem.feed_dte(*b);
            }
            answered = true;
            state = CALL_ANSWERED;
        }
        // The Courier's codec to mu-law, as the exchange's codec would.
        let octets: Vec<u8> = samples
            .iter()
            .map(|&s| ulaw_encode(((i32::from(s) * TO_ULAW + 16384) >> 15).clamp(-32768, 32767) as i16))
            .collect();
        let levels: Vec<f64> = octets.iter().map(|&o| f64::from(ulaw_decode(o)) / 32768.0).collect();
        modem.set_network_a_law(false);
        modem.give_network_levels(&levels);
        narrow.clear();
        for &l in &levels {
            wide.clear();
            up.process(l, &mut wide);
            for &w in &wide {
                let said = modem.step(w);
                down.process(said, &mut narrow);
            }
        }
        // Exactly one out for each in, whatever the conversions did.
        narrow.resize(levels.len(), 0.0);
        let ours = modem.take_network_levels().filter(|l| l.len() == levels.len()).unwrap_or_else(|| narrow.clone());
        let back: Vec<i16> = ours
            .iter()
            .map(|&l| {
                let mut o = ulaw_encode((l * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
                if let Some(q) = pad_q15 {
                    let v = i32::from(ulaw_decode(o));
                    o = ulaw_encode(((v * q + 16384) >> 15) as i16);
                }
                ((i32::from(ulaw_decode(o)) * TO_COURIER + 16384) >> 15) as i16
            })
            .collect();
        if let Some(f) = record.as_mut() {
            let mut bytes = Vec::with_capacity(4 * back.len());
            for (sent, heard) in back.iter().zip(&samples) {
                bytes.extend_from_slice(&sent.to_le_bytes());
                bytes.extend_from_slice(&heard.to_le_bytes());
            }
            f.write_all(&bytes).ok();
        }
        frames += 1;
        let stamp = (frames * back.len() as u64 * 4_348_000 / 8000) as u32;
        let off_hook = u8::from(answered && state != CALL_CLEARED);
        if send(&mut stream, stamp, off_hook, state, &back).is_err() {
            println!("[{now:7.2}] line closed");
            break;
        }

        let dte = modem.take_dte();
        if !dte.is_empty() {
            said.extend_from_slice(&dte);
            for line in String::from_utf8_lossy(&dte).split(['\r', '\n']).filter(|l| !l.trim().is_empty()) {
                println!("[{now:7.2}] ours: {}", line.trim());
            }
        }
        for note in modem.take_line_notes() {
            println!("[{now:7.2}] note: {note}");
        }
        let status = format!(
            "{:?} {} {:?}/{:?} {} / {}",
            modem.state(),
            modem.standard(),
            modem.rate(),
            modem.transmit_rate(),
            modem.line_phase(),
            modem.error_control_phase()
        );
        if status != last {
            println!("[{now:7.2}] {status}");
            last = status;
        }
        if modem.state() == State::Data && connect_at.is_none() {
            connect_at = Some(now);
        }
        // Once in data, a numbered line to the Courier every second.
        if let Some(at) = connect_at
            && modem.state() == State::Data
            && now - at > 2.0 + sent_text as f64
        {
            let text = format!("line {sent_text:03} from BinModem's x2 server, the quick brown fox\r\n");
            for b in text.bytes() {
                modem.feed_dte(b);
            }
            sent_text += 1;
        }
        if courier_off_hook == 0 && answered && state >= CALL_ANSWERED && connect_at.is_some() {
            println!("[{now:7.2}] the Courier is on hook");
        }
        if now > seconds || started.elapsed() > Duration::from_secs_f64(seconds * 4.0) {
            break;
        }
    }
    println!("ours at the end: {} {:?}/{:?}, {} lines sent", modem.standard(), modem.rate(), modem.transmit_rate(), sent_text);
}

fn connect(addr: &str) -> TcpStream {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        match TcpStream::connect(addr) {
            Ok(s) => return s,
            Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => panic!("no Courier at {addr}: {e}"),
        }
    }
}

fn receive(stream: &mut TcpStream) -> Option<(u8, u8, u8, Vec<i16>)> {
    let mut header = [0u8; 9];
    stream.read_exact(&mut header).ok()?;
    let count = u16::from_le_bytes([header[7], header[8]]) as usize;
    let mut body = vec![0u8; 2 * count];
    stream.read_exact(&mut body).ok()?;
    let samples = body.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect();
    Some((header[4], header[5], header[6], samples))
}

fn send(stream: &mut TcpStream, stamp: u32, off_hook: u8, state: u8, samples: &[i16]) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(9 + 2 * samples.len());
    frame.extend_from_slice(&stamp.to_le_bytes());
    frame.push(off_hook);
    frame.push(0);
    frame.push(state);
    frame.extend_from_slice(&(samples.len() as u16).to_le_bytes());
    for s in samples {
        frame.extend_from_slice(&s.to_le_bytes());
    }
    stream.write_all(&frame)
}
