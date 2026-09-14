//! One end of a live V.34 call's phases 3 and 4, read off its capture.
//!
//! Ignored, because captures are not in the repository. A capture keeps what
//! arrived in channel 0 and what was sent in channel 1, so either end can be
//! read: point `V34_CAPTURE` at the file, `V34_CHANNEL` at the channel,
//! `V34_SENDER` at who sent it (`call` or `answer`), and `V34_FROM` at a
//! second or so before that end's phase 3 S.
//!
//! ```text
//! V34_CAPTURE=dist/captures/live-1789426740.wav V34_CHANNEL=0 V34_SENDER=answer \
//!     V34_FROM=13.5 cargo test -p datapump --test v34_capture -- --ignored --nocapture
//! ```
//!
//! It prints what a receiver makes of everything the end sent: S-bar, how
//! well PP and TRN trained, J, J', phase 4's S-bar and TRN, each MP with its
//! acknowledge bit, E, and the bits after E.

use std::collections::VecDeque;

use datapump::v32::Mode;
use datapump::v34::info::SymbolRate;
use datapump::v34::mp::{Finder, Found};
use datapump::v34::qam::Band;
use datapump::v34::receiver::{Heard, Receiver, Reference};
use datapump::v34::signals::{J_FOUR, J_PRIME, J_SIXTEEN, Reader, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Phase3Hunt,
    Phase3,
    Phase4Hunt,
    Phase4,
}

#[test]
#[ignore = "needs a capture; see the module comment"]
fn a_captured_end_of_phases_3_and_4() {
    let Ok(path) = std::env::var("V34_CAPTURE") else {
        println!("set V34_CAPTURE to a recording to run this");
        return;
    };
    let number = |name: &str, default: f64| std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(default);
    let channel = number("V34_CHANNEL", 0.0) as usize;
    let from = number("V34_FROM", 0.0);
    let to = number("V34_TO", 1e9);
    let sender = match std::env::var("V34_SENDER").as_deref() {
        Ok("call") => Mode::Call,
        _ => Mode::Answer,
    };
    // Phase 4 is at sixteen points unless told otherwise.
    let phase4_size = if number("V34_PHASE4_POINTS", 16.0) as u32 == 4 { Size::Four } else { Size::Sixteen };
    let rate = match number("V34_BAUD", 3429.0) as u32 {
        2400 => SymbolRate::S2400,
        2743 => SymbolRate::S2743,
        2800 => SymbolRate::S2800,
        3000 => SymbolRate::S3000,
        3200 => SymbolRate::S3200,
        _ => SymbolRate::S3429,
    };
    let band = Band::new(rate, number("V34_HIGH", 0.0) != 0.0);

    let wav = line::wav::read(&path).expect("could not read the capture");
    let fs = f64::from(wav.sample_rate);
    let samples = wav.channel(channel);
    let first = (from * fs) as usize;
    let last = ((to * fs) as usize).min(samples.len());
    println!("{path} channel {channel}, {sender:?} modem's signal, {:.1} to {:.1} s", from, last as f64 / fs);

    let mut rx = Receiver::new(band, fs);
    rx.hunt();
    let mut stage = Stage::Phase3Hunt;
    let mut reader = Reader::new(sender);
    let mut finder = Finder::new();
    let mut size = Size::Four;
    let mut trn = false;
    let mut grace = 0usize;
    let mut trn_symbols = 0usize;
    let mut bits: VecDeque<bool> = VecDeque::new();
    let mut j_seen = false;
    let mut errors: VecDeque<f64> = VecDeque::new();
    let mut symbols = 0usize;
    let mut after_e: Option<Vec<bool>> = None;
    let mut last_report = 0.0;
    let mut mp_count = 0usize;
    let (mut slips, mut lost) = (0, false);
    // Equalised points between two times, for looking at.
    let (dump_from, dump_to) = (number("V34_DUMP_FROM", 0.0), number("V34_DUMP_TO", 0.0));
    let mut dump = std::env::var("V34_DUMP").ok().map(|path| std::fs::File::create(path).expect("could not make the dump"));

    for (i, &x) in samples[first..last].iter().enumerate() {
        let now = (first + i) as f64 / fs;
        rx.feed(f64::from(x));
        while let Some(heard) = rx.heard() {
            match heard {
                Heard::Reversal { at } => {
                    println!("{now:8.3} S-bar");
                    let reference = if stage == Stage::Phase3Hunt { Reference::PpThenTrn } else { Reference::Trn(phase4_size) };
                    rx.train(reference, sender, at);
                }
                Heard::Trained { snr_db } => {
                    stage = if stage == Stage::Phase3Hunt { Stage::Phase3 } else { Stage::Phase4 };
                    size = rx.size();
                    println!("{now:8.3} trained on {:?} to {snr_db:.1} dB", if stage == Stage::Phase3 { "PP and TRN" } else { "phase 4 TRN" });
                    trn = true;
                    grace = 24 / size.bits() + 1;
                    trn_symbols = 0;
                    bits.clear();
                }
                Heard::Untrained => {
                    println!("{now:8.3} did not train; hunting again");
                    rx.hunt();
                }
                Heard::Symbol(symbol) => {
                    symbols += 1;
                    if rx.slips() != slips {
                        slips = rx.slips();
                        println!("{now:8.3} found the signal again after a slip ({slips} so far)");
                    }
                    if rx.is_lost() != lost {
                        lost = rx.is_lost();
                        if lost {
                            println!("{now:8.3} lost the signal");
                        }
                    }
                    if let Some(dump) = dump.as_mut()
                        && (dump_from..dump_to).contains(&now)
                    {
                        use std::io::Write;
                        writeln!(dump, "{now:.5},{:.4},{:.4},{:.5}", symbol.point.re, symbol.point.im, symbol.error).unwrap();
                    }
                    errors.push_back(symbol.error);
                    if errors.len() > 64 {
                        errors.pop_front();
                    }
                    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
                    if now - last_report > 0.25 {
                        last_report = now;
                        println!("{now:8.3}   {:.1} dB, {} points, drift {:+.0} ppm", -10.0 * mean.log10(), if size == Size::Four { 4 } else { 16 }, rx.drift_ppm());
                    }
                    // The end fell silent: phase 3's J is over, and phase 4's
                    // S is next.
                    if stage == Stage::Phase3 && j_seen && errors.len() == 64 && mean > 0.3 {
                        println!("{now:8.3} gone quiet after {symbols} symbols; hunting for phase 4's S");
                        stage = Stage::Phase4Hunt;
                        rx.hunt();
                        errors.clear();
                        continue;
                    }
                    if trn {
                        let before = reader.clone();
                        let got = reader.trn(symbol.decided, size);
                        if grace > 0 {
                            grace -= 1;
                            continue;
                        }
                        if got.iter().all(|b| *b) {
                            trn_symbols += 1;
                            continue;
                        }
                        println!("{now:8.3} TRN over after {trn_symbols} symbols of ones");
                        reader = before;
                        trn = false;
                    }
                    for bit in reader.differential(symbol.decided, size) {
                        if let Some(tail) = after_e.as_mut()
                            && tail.len() < 400
                        {
                            tail.push(bit);
                            if tail.len() == 400 {
                                let text: String = tail.iter().map(|b| if *b { '1' } else { '0' }).collect();
                                println!("{now:8.3} 400 bits after E:");
                                for chunk in text.as_bytes().chunks(80) {
                                    println!("           {}", std::str::from_utf8(chunk).unwrap());
                                }
                            }
                        }
                        bits.push_back(bit);
                        if bits.len() > 32 {
                            bits.pop_front();
                        }
                        match finder.feed(bit) {
                            Some(Found::Mp(mp)) => {
                                mp_count += 1;
                                if mp_count <= 3 || mp.acknowledge {
                                    println!("{now:8.3} MP{} #{mp_count}: {mp:?}", if mp.acknowledge { "'" } else { "" });
                                }
                            }
                            Some(Found::E) if mp_count > 0 && after_e.is_none() => {
                                println!("{now:8.3} E");
                                after_e = Some(Vec::new());
                            }
                            _ => {}
                        }
                    }
                    if size == Size::Four && bits.len() == 32 {
                        let older: Vec<bool> = bits.range(..16).copied().collect();
                        let newer: Vec<bool> = bits.range(16..).copied().collect();
                        if !j_seen {
                            for (name, j) in [("four", J_FOUR), ("sixteen", J_SIXTEEN)] {
                                if older == j && newer == j {
                                    println!("{now:8.3} J asking for {name} points");
                                    j_seen = true;
                                }
                            }
                        } else if newer == J_PRIME && (older == J_FOUR || older == J_SIXTEEN) {
                            println!("{now:8.3} J'; TRN at {phase4_size:?} next");
                            size = phase4_size;
                            rx.set_size(size);
                            trn = true;
                            grace = 24 / size.bits() + 1;
                            trn_symbols = 0;
                            bits.clear();
                            stage = Stage::Phase4;
                        }
                    }
                }
            }
        }
    }
    println!("{mp_count} MP sequences in all");
}
