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
