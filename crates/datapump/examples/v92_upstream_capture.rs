//! Read what a live V.92 call sent up, the way the server would have had it.
//!
//! A capture's channel 1 is the analogue modem's own output at the modem's
//! rate, before `sip::Media::transmit` turns it into PCMU: a 16 kHz to 8 kHz
//! `dsp::Resampler` and a G.711 encoder. This puts channel 1 through the same
//! two and reads the sequences in a window, so that "the server never read our
//! CPt / SUVu / CPu" can be told apart: either the upstream we sent is
//! unreadable, or it was readable and the server or the route lost it.
//!
//! ```text
//! cargo run --release -p datapump --example v92_upstream_capture -- WAV FROM TO [--cpt|--epsilon] [--lead N]
//! ```
//!
//! `--epsilon` measures Su's phase against the 8 kHz instants a tenth of a
//! second at a time, as the digital modem does for Jp's epsilon (9.5.1.1.6).
//! `--cpt` reads CPt's one bit a symbol (8.5.1); without it the window is read
//! as TRN2u's four points, which is what SUVu and CPu ride (8.7.6). `--lead`
//! puts N samples of silence in front of channel 1, which moves the
//! resampler's 8 kHz instants by N 16 kHz samples: the capture does not say
//! where the live resampler's instants fell, so both are worth trying.

use std::env;
use std::fs;

use datapump::v90::ucode::{self, Law};
use datapump::v92::sequences::{CpFamily, UpFinder, trn2u_unit};
use datapump::v92::up_signals::{Trn2uReader, Trn2uSize, TwoPointReader};
use dsp::Resampler;

/// The bits twelve CPt symbols carry: one a symbol (8.5.1).
const CPT_UNIT: usize = 12;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let usage = "usage: v92_upstream_capture WAV FROM TO [--cpt|--epsilon] [--lead N]";
    let path = args.next().ok_or(usage)?;
    let from: f64 = args.next().ok_or(usage)?.parse()?;
    let to: f64 = args.next().ok_or(usage)?.parse()?;
    let (mut cpt, mut lead, mut epsilon) = (false, 0usize, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cpt" => cpt = true,
            "--epsilon" => epsilon = true,
            "--lead" => lead = args.next().ok_or(usage)?.parse()?,
            other => return Err(format!("{other}? {usage}").into()),
        }
    }

    let (rate, channels, samples) = wav(&fs::read(&path)?)?;
    if channels < 2 {
        return Err("no channel 1 in this capture".into());
    }
    // Channel 1 through the conversion `sip::Media::transmit` makes.
    let mut down = Resampler::new(f64::from(rate), 8000.0);
    let mut line = Vec::new();
    let mut out = Vec::new();
    let sent = samples.chunks(channels).map(|frame| f64::from(frame[1]));
    for x in std::iter::repeat_n(0.0, lead).chain(sent) {
        out.clear();
        down.process(x, &mut out);
        for &level in &out {
            let (u, negative) = ucode::nearest(Law::Mu, level.round() as i32);
            line.push(ucode::level(Law::Mu, u) * 32768.0 * if negative { -1.0 } else { 1.0 });
        }
    }
    // The resampler's delay, so that the window is where the transcript says.
    let delay = 16.0 / 8000.0 * f64::from(rate) / 16_000.0 + lead as f64 / f64::from(rate);
    let first = (((from + delay) * 8000.0) as usize).min(line.len());
    let last = (((to + delay) * 8000.0) as usize).min(line.len());
    let window = &line[first..last];
    let rms = (window.iter().map(|x| x * x).sum::<f64>() / window.len().max(1) as f64).sqrt();
    println!(
        "{path}: {from:.3}..{to:.3} s, {} symbols, rms {rms:.0}, lead {lead}",
        window.len()
    );
    if rms <= 0.0 {
        return Ok(());
    }
    if epsilon {
        // Su's phase against the 8 kHz instants, as 9.5.1.1.6 has the digital
        // modem measure it, a tenth of a second at a time.
        for (k, run) in window.chunks(798).enumerate() {
            if let Some(lag) = datapump::v92::epsilon::lag(run) {
                println!(
                    "  {:7.3} s  Su lag {lag:.3} symbol",
                    from + k as f64 * 798.0 / 8000.0
                );
            }
        }
        return Ok(());
    }

    let mut finder = UpFinder::new(if cpt { CPT_UNIT } else { trn2u_unit(false) });
    let mut two = TwoPointReader::differential();
    let mut four = Trn2uReader::new(Trn2uSize::Four);
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut shown = 0;
    for (n, &x) in window.iter().enumerate() {
        // TRN2u and its sequences go out at LU's power, and CPt at LU.
        let level = x / rms;
        let bits: Vec<bool> = if cpt {
            two.feed(level).into_iter().collect()
        } else {
            four.feed(level).into_iter().collect()
        };
        for bit in bits {
            let Some(family) = finder.feed(bit) else {
                continue;
            };
            let name = match &family {
                CpFamily::Cpt(cp) => format!("CPt drn {}", cp.drn),
                CpFamily::Cpu(cp) => format!("CPu drn {} ack {}", cp.drn, cp.acknowledge),
                CpFamily::Cpus(c) => format!("CPus drn {} ack {}", c.drn, c.ack),
                CpFamily::Suvu(s) => format!("SUVu ack {}", s.ack),
            };
            if shown < 6 {
                println!("  {:9.4} s  {name}", from + n as f64 / 8000.0);
                shown += 1;
            }
            *counts.entry(name).or_default() += 1;
        }
    }
    println!("  read: {counts:?}");
    Ok(())
}

/// A PCM WAV's rate, channel count and interleaved 16-bit samples.
fn wav(bytes: &[u8]) -> Result<(u32, usize, Vec<i16>), Box<dyn std::error::Error>> {
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV".into());
    }
    let (mut at, mut format) = (12, None);
    while at + 8 <= bytes.len() {
        let size = word(at + 4) as usize;
        let body = at + 8;
        match &bytes[at..at + 4] {
            b"fmt " => {
                let channels = usize::from(u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]));
                format = Some((word(body + 4), channels));
            }
            b"data" => {
                let (rate, channels) = format.ok_or("data before fmt")?;
                let end = (body + size).min(bytes.len());
                let samples = bytes[body..end]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| i16::from_le_bytes(*p))
                    .collect();
                return Ok((rate, channels, samples));
            }
            _ => {}
        }
        at = body + size + (size & 1);
    }
    Err("no data chunk".into())
}
