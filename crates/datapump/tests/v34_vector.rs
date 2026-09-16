//! Phase 2 of a real V.34 call, read off a recording.
//!
//! `tests/vectors/v34-33600.wav` is a Conexant softmodem's call at 33 600,
//! both directions summed on one tap as on a two-wire line. Phase 2 is the one
//! part of V.34 that can be read out of a recording like that without an echo
//! canceller: the call modem's INFO sequences are on 1200 Hz and the answer
//! modem's on 2400, so a receiver on each carrier hears one side only.
//!
//! Every sequence here checks its CRC, which is what settles the layout of the
//! tables, the order of the CRC and the sense of the DPSK against a modem
//! nobody here wrote.

use datapump::v34::dpsk::{Receiver, Side};
use datapump::v34::info::{Info, SymbolRate};

const VECTOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v34-33600.wav");

fn sequences() -> Vec<(f64, Side, Info)> {
    let wav = line::wav::read(VECTOR).expect("could not read the vector");
    let fs = f64::from(wav.sample_rate);
    let samples = wav.channel(0);
    let mut out = Vec::new();
    for side in [Side::Call, Side::Answer] {
        let mut rx = Receiver::new(side, fs);
        for (i, &s) in samples.iter().enumerate() {
            if let Some(info) = rx.feed(f64::from(s)) {
                out.push((i as f64 / fs, side, info));
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

#[test]
fn the_info_sequences_of_a_real_call_check() {
    // Three of the four, and exactly those three: nothing anywhere else in
    // eighteen seconds of V.8, probing, training and data passes for one.
    //
    // The fourth, INFO0c, is on the recording but cannot be read off it. The
    // call modem's carrier comes up at 3.73 s and runs steady as tone B from
    // 3.81, so INFO0c is the 82 ms between -- and the answer modem's JM runs
    // until 3.80, still finishing its octets after CJ. On a summed tap the two
    // are about as loud as each other, and JM's 1650 Hz mark is 450 Hz off
    // INFO0c's carrier, inside its band where no filter can reach it. The
    // answer modem heard INFO0c through a hybrid, with its own JM taken off
    // it, and the call went ahead without a repeat. INFO0c has the layout
    // INFO0a has, and INFO0a checks.
    let found = sequences();
    for (at, side, info) in &found {
        println!("{at:6.3}s {side:?}: {info:#?}");
    }
    let kinds: Vec<(Side, &str)> = found
        .iter()
        .map(|(_, side, info)| {
            let kind = match info {
                Info::Info0(_) => "INFO0",
                Info::Info1c(_) => "INFO1c",
                Info::Info1a(_) => "INFO1a",
                Info::Info0d(_) => "INFO0d",
                Info::Info1aPcm(_) => "INFO1a (V.90)",
            };
            (*side, kind)
        })
        .collect();
    assert_eq!(
        kinds,
        vec![(Side::Answer, "INFO0"), (Side::Call, "INFO1c"), (Side::Answer, "INFO1a")]
    );
}

#[test]
fn the_real_capabilities_and_results_are_what_a_33600_modem_says() {
    let found = sequences();
    let info0a = found.iter().find_map(|(_, _, i)| if let Info::Info0(x) = i { Some(*x) } else { None }).unwrap();
    // A modem that did 33 600 on this call: every symbol rate, both carriers
    // at 3000 and 3200, and the 1664-point constellation that 33 600 needs.
    assert!(info0a.rate_2743 && info0a.rate_2800 && info0a.rate_3429 && info0a.transmit_3429);
    assert!(info0a.constellation_1664);
    let info1c = found.iter().find_map(|(_, _, i)| if let Info::Info1c(x) = i { Some(*x) } else { None }).unwrap();
    // Probing results that climb with the symbol rate, as they should on a
    // clean line: each step up in symbol rate a step up in projected rate.
    let rates: Vec<u8> = info1c.probed.iter().map(|p| p.max_rate).collect();
    assert!(rates.windows(2).all(|w| w[0] < w[1]), "{rates:?}");
    assert!(info1c.probed.iter().all(|p| p.pre_emphasis <= 10));
}

#[test]
fn the_real_call_settled_on_what_a_33600_call_needs() {
    // A call at 33 600 bit/s is 14 times 2400 in at least one direction, and
    // only the two fastest symbol rates carry that many bits.
    let found = sequences();
    let Some((_, _, Info::Info1a(settled))) = found.iter().find(|(_, _, i)| matches!(i, Info::Info1a(_))) else {
        panic!("no INFO1a");
    };
    assert!(
        matches!(settled.answer_to_call, SymbolRate::S3200 | SymbolRate::S3429)
            || matches!(settled.call_to_answer, SymbolRate::S3200 | SymbolRate::S3429),
        "{settled:?}"
    );
    // And the two INFO0 sequences are in front of the two INFO1 sequences, as
    // Figure 16 draws them.
    let first = |which: fn(&Info) -> bool| found.iter().find(|(_, _, i)| which(i)).map(|(t, _, _)| *t);
    let info0 = first(|i| matches!(i, Info::Info0(_))).unwrap();
    let info1c = first(|i| matches!(i, Info::Info1c(_))).unwrap();
    let info1a = first(|i| matches!(i, Info::Info1a(_))).unwrap();
    assert!(info0 < info1c && info1c < info1a, "{info0} {info1c} {info1a}");
}

/// What a call modem's receiver makes of the answer modem's phase 3 on the
/// recording: where S-bar was, how well PP and TRN trained it, and the J the
/// answer modem sent.
struct Phase3 {
    reversal_at: f64,
    trained_db: f64,
    trn_symbols: usize,
    j: Option<(f64, [bool; 16])>,
}

fn answer_phase3(from: f64, to: f64) -> Phase3 {
    answer_phase3_in(VECTOR, 0, from, to)
}

fn answer_phase3_in(path: &str, channel: usize, from: f64, to: f64) -> Phase3 {
    use datapump::v32::Mode;
    use datapump::v34::qam::Band;
    use datapump::v34::receiver::{Heard, Receiver, Reference};
    use datapump::v34::signals::{J_FOUR, J_SIXTEEN, Reader, Size};

    let wav = line::wav::read(path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    let samples = wav.channel(channel);
    // INFO1a said 3429 symbols a second both ways, and at 3429 both carriers
    // are 1959 Hz.
    let band = Band::new(SymbolRate::S3429, false);
    let mut rx = Receiver::new(band, fs);
    rx.hunt();
    let mut reader = Reader::new(Mode::Answer);
    let mut bits: Vec<bool> = Vec::new();
    let mut in_trn = true;
    let mut found = Phase3 { reversal_at: 0.0, trained_db: 0.0, trn_symbols: 0, j: None };
    let first = (from * fs) as usize;
    for (i, &x) in samples[first..(to * fs) as usize].iter().enumerate() {
        rx.feed(f64::from(x));
        while let Some(heard) = rx.heard() {
            let now = (first + i) as f64 / fs;
            match heard {
                Heard::S => {}
                Heard::Reversal { at } => {
                    found.reversal_at = now;
                    rx.train(Reference::PpThenTrn, Mode::Answer, at);
                }
                Heard::Trained { snr_db } => found.trained_db = snr_db,
                Heard::Untrained => panic!("PP did not train at {now:.3} s"),
                Heard::Symbol(symbol) => {
                    if in_trn {
                        let got = reader.trn(symbol.decided, Size::Four);
                        if got.iter().all(|b| *b) || bits.len() < 46 {
                            bits.extend(got);
                            found.trn_symbols += 1;
                            continue;
                        }
                        in_trn = false;
                    }
                    bits.extend(reader.differential(symbol.decided, Size::Four));
                    let tail = &bits[bits.len().saturating_sub(48)..];
                    for j in [J_FOUR, J_SIXTEEN] {
                        if found.j.is_none() && tail.len() == 48 && tail.chunks(16).all(|c| c == j) {
                            found.j = Some((now, j));
                        }
                    }
                }
            }
        }
    }
    found
}

#[test]
fn a_real_answer_modems_phase_3_trains_this_receiver_and_asks_for_16_points() {
    // INFO1a ends at 6.00 s. Seventy milliseconds of silence, S and S-bar,
    // PP, and TRN from 6.19 s -- 1.87 s of it on this call -- and then J,
    // until the call modem's own S comes in on top at 8.45 s.
    let phase3 = answer_phase3(5.99, 8.45);
    assert!((6.09..6.12).contains(&phase3.reversal_at), "S-bar at {:.3} s", phase3.reversal_at);
    assert!(phase3.trained_db > 15.0, "trained to {:.1} dB", phase3.trained_db);
    // Every TRN symbol after training descrambles to ones, which checks the
    // scrambler, the four-point mapping and the bit order against the modem.
    assert!(phase3.trn_symbols > 5000, "{} symbols of TRN read", phase3.trn_symbols);
    let (at, j) = phase3.j.expect("no J");
    assert_eq!(j, datapump::v34::signals::J_SIXTEEN, "J asked for four points");
    assert!((8.0..8.2).contains(&at), "J read at {at:.3} s");
}

/// The same for a live capture, which is not kept in the repository: name it
/// in `V34_CAPTURE`, with `V34_FROM` and `V34_TO` in seconds around the far
/// end's phase 3. Captures keep what arrived in channel 0.
#[test]
#[ignore]
fn a_captured_answer_modems_phase_3() {
    let Ok(path) = std::env::var("V34_CAPTURE") else { return };
    let seconds = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok());
    let phase3 = answer_phase3_in(&path, 0, seconds("V34_FROM").unwrap_or(0.0), seconds("V34_TO").unwrap_or(30.0));
    println!(
        "S-bar at {:.4} s, trained to {:.1} dB, {} TRN symbols read, J {:?}",
        phase3.reversal_at,
        phase3.trained_db,
        phase3.trn_symbols,
        phase3.j.map(|(at, j)| (at, j == datapump::v34::signals::J_SIXTEEN))
    );
}
