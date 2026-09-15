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
//!
//! Data mode after E is read at `V34_DATA_RATE` (31200 unless told), with
//! what the receiving end's MP asked for: `V34_CODE` 16, 32 or 64 states,
//! `V34_NONLINEAR` and `V34_EXPANDED` 1 for either. `V34_DUMP` writes every
//! equalised point between `V34_DUMP_FROM` and `V34_DUMP_TO` seconds as
//! `time,re,im,error,data` -- `data` 1 once data mode's grid is in use, when
//! re and im are in its grid units -- for `tools/plot_constellation.py`.
//!
//! When data mode turns into four points again, the end has gone back to S,
//! TRN and MP for a rate renegotiation, and its new MP is read the same way.
//! `V34_RENEGOTIATION=1` starts there, for a stretch that begins with one.
//!
//! The second test, `a_captured_call_through_the_start_up`, runs the far end's
//! channel through the start-up the modem itself runs, from where V.8 handed
//! over (`V34_SKIP`): what the modem's own receiver made of the data, and when
//! it changed stage.

use std::collections::VecDeque;

use datapump::v32::Mode;
use datapump::v34::data::{Decoder, Params};
use datapump::v34::frame::Framing;
use datapump::v34::trellis::Code;
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
    // A rate renegotiation (11.6) is S, S-bar, TRN and MP at four points,
    // the way phase 4 is: `V34_RENEGOTIATION=1` starts there.
    let mut stage = if number("V34_RENEGOTIATION", 0.0) != 0.0 { Stage::Phase4Hunt } else { Stage::Phase3Hunt };
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
    // Data mode after E, as the receiving end's MP asked for it: the rate the
    // two MPs came to, the trellis code, non-linear encoding and shaping. No
    // precoding, which a Type 0 MP or zero coefficients both come to.
    let data_rate = number("V34_DATA_RATE", 31_200.0) as u32;
    let code = match number("V34_CODE", 16.0) as u32 {
        32 => Code::States32,
        64 => Code::States64,
        _ => Code::States16,
    };
    let nonlinear = number("V34_NONLINEAR", 0.0) != 0.0;
    let expanded = number("V34_EXPANDED", 0.0) != 0.0;
    let mut data: Option<Decoder> = None;
    let mut data_bits: Vec<bool> = Vec::new();
    let mut e_seen = false;
    // How far data mode's points are from four points: near nothing when the
    // far end has gone back to S, TRN and MP for a rate renegotiation.
    let mut four_fit: VecDeque<f64> = VecDeque::new();
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
                    if e_seen && data.is_none() {
                        let params = Params {
                            framing: Framing::new(rate, data_rate, false, expanded).expect("a rate Table 8 has"),
                            code,
                            nonlinear,
                            precoding: [(0, 0); 3],
                            mode: sender,
                        };
                        let decoder = Decoder::new(params);
                        rx.set_grid(decoder.grid_scale(), decoder.extent());
                        println!("{now:8.3} B1 and data at {data_rate} from here, grid scale {:.2}", decoder.grid_scale());
                        data = Some(decoder);
                    }
                    if let Some(dump) = dump.as_mut()
                        && (dump_from..dump_to).contains(&now)
                    {
                        use std::io::Write;
                        let (scale, grid) = data.as_ref().map_or((1.0, 0), |d| (d.grid_scale(), 1));
                        writeln!(dump, "{now:.5},{:.4},{:.4},{:.5},{grid}", symbol.point.re * scale, symbol.point.im * scale, symbol.error).unwrap();
                    }
                    if data.is_some() {
                        let corner = std::f64::consts::FRAC_1_SQRT_2;
                        let off = (symbol.point.re.abs() - corner).powi(2) + (symbol.point.im.abs() - corner).powi(2);
                        four_fit.push_back(off);
                        if four_fit.len() > 48 {
                            four_fit.pop_front();
                        }
                        if four_fit.len() == 48 && four_fit.iter().sum::<f64>() / 48.0 < 0.02 {
                            println!("{now:8.3} four points again, after {} data bits: S, TRN and MP of a renegotiation", data_bits.len());
                            data = None;
                            four_fit.clear();
                            size = Size::Four;
                            rx.set_size(size);
                            reader = Reader::new(sender);
                            finder = Finder::new();
                            trn = true;
                            // Past the rest of S and S-bar before TRN.
                            grace = 64;
                            trn_symbols = 0;
                            bits.clear();
                            mp_count = 0;
                            after_e = None;
                            e_seen = false;
                            stage = Stage::Phase4;
                            continue;
                        }
                    }
                    if let Some(decoder) = data.as_mut() {
                        decoder.feed(symbol.point);
                        data_bits.extend(decoder.take_bits());
                        if now - last_report > 0.02 && now < 30.3 || now - last_report > 0.25 {
                            last_report = now;
                            let recent = &data_bits[data_bits.len().saturating_sub(500)..];
                            println!(
                                "{now:8.3}   data: {:.1} dB on the grid, path cost {:.2}, {} bits, last 500 {:.2} ones, drift {:+.0} ppm",
                                rx.snr_db(),
                                decoder.path_cost(),
                                data_bits.len(),
                                recent.iter().filter(|b| **b).count() as f64 / recent.len().max(1) as f64,
                                rx.drift_ppm()
                            );
                        }
                        continue;
                    }
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
                                e_seen = true;
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
    if !data_bits.is_empty() {
        let text: String = data_bits.iter().take(2400).map(|b| if *b { '1' } else { '0' }).collect();
        println!("the first data bits:");
        for chunk in text.as_bytes().chunks(100) {
            println!("  {}", std::str::from_utf8(chunk).unwrap());
        }
        let ones = data_bits.iter().filter(|b| **b).count();
        let flags = data_bits.windows(8).filter(|w| *w == [false, true, true, true, true, true, true, false]).count();
        println!("{} data bits, {} ones, {} HDLC flags", data_bits.len(), ones, flags);
        // What V.42 made of it: frames whose FCS checks, and the ones that
        // did not.
        let mut hdlc = ec::hdlc::Decoder::new(ec::hdlc::Fcs::Bits16);
        hdlc.accept_either();
        let (mut good, mut bad) = (0, 0);
        for &bit in &data_bits {
            match hdlc.feed(bit) {
                Some(Ok(frame)) => {
                    good += 1;
                    if good <= 6 {
                        println!("  frame of {} octets: {:02x?}", frame.len(), &frame[..frame.len().min(24)]);
                    }
                }
                Some(Err(e)) => {
                    bad += 1;
                    if bad <= 3 {
                        println!("  bad frame: {e:?}");
                    }
                }
                None => {}
            }
        }
        println!("{good} frames checked, {bad} did not");
        if let Ok(path) = std::env::var("V34_BITS") {
            let text: String = data_bits.iter().map(|b| if *b { '1' } else { '0' }).collect();
            std::fs::write(path, text).unwrap();
        }
    }
}

/// The same capture through the start-up the modem runs, from where V.8
/// handed over: what the modem's own receiver made of the far end's data.
///
/// ```text
/// V34_CAPTURE=dist/captures/live-1789442205.wav V34_SKIP=6.525 \
///     cargo test -p datapump --test v34_capture start_up -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs a capture; see the module comment"]
fn a_captured_call_through_the_start_up() {
    use datapump::v34::phase2::Role;
    use datapump::v34::startup::{Modem, Status};

    let Ok(path) = std::env::var("V34_CAPTURE") else {
        println!("set V34_CAPTURE to a recording to run this");
        return;
    };
    let skip = std::env::var("V34_SKIP").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let role = match std::env::var("V34_ROLE").as_deref() {
        Ok("answer") => Role::Answer,
        _ => Role::Call,
    };
    let wav = line::wav::read(&path).expect("could not read the capture");
    let fs = f64::from(wav.sample_rate);
    let samples = wav.channel(0);
    let mut modem = Modem::new(role, fs);
    let (mut phase, mut status) = ("", Status::Running);
    let mut bits: Vec<bool> = Vec::new();
    let mut last_report = 0.0;
    for (i, &x) in samples[(skip * fs) as usize..].iter().enumerate() {
        let now = skip + i as f64 / fs;
        modem.step(f64::from(x));
        if modem.phase() != phase || modem.status() != status {
            (phase, status) = (modem.phase(), modem.status());
            println!("{now:8.3} {phase}, {status:?}");
        }
        let got = modem.take_bits();
        bits.extend(&got);
        if let Some(training) = modem.training()
            && matches!(status, Status::Connected { .. } | Status::Retraining)
            && now - last_report > 0.1
        {
            last_report = now;
            let recent = &bits[bits.len().saturating_sub(500)..];
            println!(
                "{now:8.3}   {} bits, last 500 {:.2} ones, B1 errors {}, path cost {:.2?}, {:.1} dB, slips {}, far MP {:?}",
                bits.len(),
                recent.iter().filter(|b| **b).count() as f64 / recent.len().max(1) as f64,
                training.b1_errors(),
                training.path_cost(),
                training.snr(),
                training.slips(),
                training.far_mp().map(|m| (m.call_to_answer, m.answer_to_call, m.acknowledge)),
            );
        }
    }
    if let Ok(path) = std::env::var("V34_BITS") {
        let text: String = bits.iter().map(|b| if *b { '1' } else { '0' }).collect();
        std::fs::write(path, text).unwrap();
    }
}
