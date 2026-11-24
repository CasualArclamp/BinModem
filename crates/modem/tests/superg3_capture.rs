//! Read a recorded Super G3 call, both directions, with the receivers a call
//! would use.
//!
//! ```text
//! SUPERG3_CAPTURE=F:/dialupmodem2/dist/captures/live-1790500484.wav \
//!     cargo test -p modem --release --test superg3_capture -- --ignored --nocapture
//! ```
//!
//! Ignored, because it needs a recording. A capture has two channels, what
//! arrived and what was sent, on one timeline. Each is read as the other end
//! would read it: phase 2's DPSK for the INFO sequences, then the V.34 control
//! channel's receiver for PPh (and which of 10-2's readings it was), ALT, the
//! MPh sequences and E, and the T.30 frames after it. What one end said and
//! the other never answered is where a call stopped.
//!
//! `SUPERG3_SENT_IS_CALL=0` if this end answered the call: the sent channel is
//! then the answer modem's.

use datapump::v34::control::{self, Heard};
use datapump::v34::dpsk::{self, Side};
use datapump::v34::info::Info;
use datapump::v34::mp::{MphFinder, MphFound};
use fax::frames;

fn describe(heard: &Heard, fs: f64) -> String {
    let t = |at: u64| at as f64 / fs;
    match *heard {
        Heard::Carrier { on, at } => format!("{:7.3}s carrier {}", t(at), if on { "on" } else { "off" }),
        Heard::Tone { at } => format!("{:7.3}s tone", t(at)),
        Heard::Ac { at } => format!("{:7.3}s AC", t(at)),
        Heard::Sh { at } => format!("{:7.3}s Sh", t(at)),
        Heard::Reversal { at } => format!("{:7.3}s Sh -> S-bar-h", t(at)),
        Heard::Pph { reading, began, ended } => {
            format!("{:7.3}s PPh, {reading:?}, to {:.3}s", t(began), t(ended))
        }
        Heard::Trained { on, snr_db } => format!("        trained on {on:?}, {snr_db:.1} dB"),
        Heard::Untrained { on } => format!("        untrained on {on:?}"),
        Heard::E { at } => format!("{:7.3}s E", t(at)),
        Heard::Lost { at } => format!("{:7.3}s lost", t(at)),
    }
}

/// Everything one direction of the call said.
fn read_direction(samples: &[f32], fs: f64, sender: Side) {
    let listener = match sender {
        Side::Call => Side::Answer,
        Side::Answer => Side::Call,
    };
    let mut infos = dpsk::Receiver::half_duplex(sender, fs);
    let mut control = control::Receiver::new(listener, fs);
    let mut mph = MphFinder::new();
    let mut hdlc = frames::Reader::new();
    let mut data_bits = 0usize;
    for (i, &s) in samples.iter().enumerate() {
        let x = f64::from(s);
        let now = i as f64 / fs;
        if let Some(info) = infos.feed(x) {
            match info {
                Info::Info0(i0) => println!("{now:7.3}s INFO0 {i0:?}"),
                Info::InfoH(h) => println!("{now:7.3}s INFOh {h:?}"),
                other => println!("{now:7.3}s {other:?}"),
            }
        }
        control.feed(x);
        while let Some(heard) = control.heard() {
            println!("{}", describe(&heard, fs));
        }
        for bit in control.take_sync_bits() {
            match mph.feed(bit) {
                Some(MphFound::Mph(m)) => println!("{now:7.3}s MPh {m:?}"),
                Some(MphFound::E) => println!("{now:7.3}s (E in the sync bits)"),
                None => {}
            }
        }
        for bit in control.take_bits() {
            data_bits += 1;
            if let Some(message) = hdlc.feed(bit) {
                println!(
                    "{now:7.3}s frame {:?}{} {} {:02x?}",
                    message.frame,
                    if message.last { "" } else { " (more)" },
                    if message.from_caller { "X=1" } else { "X=0" },
                    message.fif
                );
            }
        }
    }
    println!(
        "  -- {data_bits} data bits, {} pieces between flags that were not whole frames, PPh reading {:?}, trained {:.1} dB, {} slips",
        hdlc.bad,
        control.reference(),
        control.trained_snr_db(),
        control.slips()
    );
}

#[test]
#[ignore = "needs a recording: SUPERG3_CAPTURE=path"]
fn read_a_super_g3_capture() {
    let path = std::env::var("SUPERG3_CAPTURE").expect("SUPERG3_CAPTURE=path to a capture");
    let wav = line::wav::read(&path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    let sent_is_call = std::env::var("SUPERG3_SENT_IS_CALL").map_or(true, |v| v != "0");
    let (sent, arrived) = if sent_is_call { (Side::Call, Side::Answer) } else { (Side::Answer, Side::Call) };
    println!("== what arrived: the {arrived:?} modem");
    read_direction(&wav.channel(0), fs, arrived);
    println!("== what was sent: the {sent:?} modem");
    read_direction(&wav.channel(1), fs, sent);
}

/// Every frame between two flags, good or bad: HDLC by hand, so that a frame
/// that fails its check is still seen, and what it began with.
#[derive(Default)]
struct Deframer {
    ones: u32,
    bits: Vec<bool>,
}

impl Deframer {
    /// A bit in; the octets of a frame, with whether its check passed, when a
    /// flag ends one.
    fn feed(&mut self, bit: bool) -> Option<(Vec<u8>, bool)> {
        if bit {
            self.ones += 1;
            if self.ones >= 7 {
                // Seven ones: an abort, or the line idling. Nothing is a frame.
                self.bits.clear();
                return None;
            }
            self.bits.push(true);
            return None;
        }
        let ones = std::mem::take(&mut self.ones);
        if ones == 5 {
            // A zero stuffed after five ones.
            return None;
        }
        if ones == 6 {
            // A flag: 0 and six ones went in, and this zero closes it.
            let end = self.bits.len().saturating_sub(7);
            let data: Vec<bool> = self.bits.drain(..).take(end).collect();
            if data.len() >= 32 && data.len().is_multiple_of(8) {
                let octets: Vec<u8> = data
                    .chunks(8)
                    .map(|c| c.iter().enumerate().fold(0u8, |o, (i, &b)| o | (u8::from(b) << i)))
                    .collect();
                let mut crc = 0xffffu16;
                for &o in &octets {
                    crc ^= u16::from(o);
                    for _ in 0..8 {
                        crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
                    }
                }
                return Some((octets, crc == 0xf0b8));
            }
            return None;
        }
        self.bits.push(false);
        None
    }
}

/// Every frame of one direction's control channel data, bad ones included.
fn raw_frames(samples: &[f32], fs: f64, sender: Side) {
    let listener = match sender {
        Side::Call => Side::Answer,
        Side::Answer => Side::Call,
    };
    let mut control = control::Receiver::new(listener, fs);
    let mut deframer = Deframer::default();
    for (i, &s) in samples.iter().enumerate() {
        control.feed(f64::from(s));
        while control.heard().is_some() {}
        control.take_sync_bits();
        for bit in control.take_bits() {
            if let Some((octets, good)) = deframer.feed(bit) {
                let body = &octets[..octets.len() - 2];
                let name = frames::Message::parse(body)
                    .map_or_else(|| "?".to_owned(), |m| format!("{:?}", m.frame));
                println!(
                    "{:7.3}s {} {:>4} octets  {:<5} {:02x?}",
                    i as f64 / fs,
                    if good { "good" } else { "BAD " },
                    octets.len(),
                    name,
                    &body[..body.len().min(6)]
                );
            }
        }
    }
}

#[test]
#[ignore = "needs a recording: SUPERG3_CAPTURE=path"]
fn every_frame_of_a_super_g3_capture() {
    let path = std::env::var("SUPERG3_CAPTURE").expect("SUPERG3_CAPTURE=path to a capture");
    let wav = line::wav::read(&path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    let sent_is_call = std::env::var("SUPERG3_SENT_IS_CALL").map_or(true, |v| v != "0");
    let (sent, arrived) = if sent_is_call { (Side::Call, Side::Answer) } else { (Side::Answer, Side::Call) };
    println!("== what arrived: the {arrived:?} modem");
    raw_frames(&wav.channel(0), fs, arrived);
    println!("== what was sent: the {sent:?} modem");
    raw_frames(&wav.channel(1), fs, sent);
}
