//! A real V.90 call, read off a recording.
//!
//! `tests/vectors/v90-56k.wav` is a Conexant V.92 softmodem dialling a 56k
//! server, both directions summed on one tap. The analogue modem is the one on
//! the tap, so everything it sent is loud and everything the server sent has
//! crossed the line first.
//!
//! These read what can be read without separating the two directions: V.8's
//! menus, which are in different V.21 channels, and phase 2's INFO sequences,
//! which V.90 puts on the two carriers V.34 does. The server is the digital
//! modem and takes the 1200 Hz side, whichever end dialled.

use datapump::Bell103Rx;
use datapump::v34::dpsk::{Receiver, Side};
use datapump::v34::info::{Info, SymbolRate};
use v8::{Access, CallFunction, Decoder, Heard, Menu, Modulation, Pcm, PcmRole, Protocol};

const VECTOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v90-56k.wav");

fn samples() -> (f64, Vec<f32>) {
    let wav = line::wav::read(VECTOR).expect("could not read the vector");
    (f64::from(wav.sample_rate), wav.channel(0))
}

/// Every menu heard in one V.21 channel.
fn menus(tones: (f64, f64)) -> Vec<Menu> {
    let (fs, samples) = samples();
    let mut rx = Bell103Rx::with_tones(tones.0, tones.1, fs);
    let mut decoder = Decoder::new();
    let mut out = Vec::new();
    for &s in &samples {
        if let Some(octet) = rx.feed(f64::from(s))
            && let Some(Heard::Cm(menu)) = decoder.feed(octet)
        {
            out.push(menu);
        }
    }
    out
}

fn sequences() -> Vec<(f64, Side, Info)> {
    let (fs, samples) = samples();
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
fn the_call_menu_offers_an_analogue_v90_modem_and_the_answer_a_digital_one() {
    let cm = menus(datapump::v8::LOW);
    let jm = menus(datapump::v8::HIGH);
    assert!(cm.len() >= 2, "{} call menus", cm.len());
    assert!(jm.len() >= 2, "{} joint menus", jm.len());
    let (cm, jm) = (cm[1], jm[1]);
    assert_eq!(cm.function, CallFunction::Data);
    assert_eq!(cm.pcm, Some(Pcm::ANALOGUE));
    assert_eq!(cm.access, Some(Access::default()));
    assert_eq!(cm.protocol, Protocol::Lapm);
    assert!(cm.modulations.contains(Modulation::V34Duplex));

    assert_eq!(jm.pcm, Some(Pcm { analogue: false, digital: true, v91: false }));
    assert_eq!(jm.access, Some(Access { digital: true, ..Access::default() }));
    assert_eq!(Pcm::pair(Pcm::ANALOGUE, jm.pcm.unwrap(), true), Some(PcmRole::Analogue));
}

#[test]
fn the_digital_modem_s_info0d_says_what_the_network_is() {
    let found = sequences();
    for (at, side, info) in &found {
        println!("{at:6.3}s {side:?}: {info:?}");
    }
    let info0d = found
        .iter()
        .find_map(|(_, _, i)| if let Info::Info0d(x) = i { Some(*x) } else { None })
        .expect("no INFO0d");
    // A North American server: mu-law, at the codec, with a -12 dBm0
    // ceiling -- which is what Table 15/V.90 turns into a limit on the
    // constellations the analogue modem may ask for.
    assert!(!info0d.a_law);
    assert!(info0d.power_at_codec);
    assert_eq!(info0d.max_dbm0(), -12.0);
    assert_eq!(info0d.nominal_dbm0(), -10.0);
    assert!(info0d.v34.constellation_1664 && info0d.v34.rate_3429);
    assert!(!info0d.upstream_3429);
}

#[test]
fn the_analogue_modem_asks_for_v90_and_names_its_training_codeword() {
    let found = sequences();
    let kinds: Vec<(Side, &str)> = found
        .iter()
        .map(|(_, side, info)| {
            let kind = match info {
                Info::Info0(_) => "INFO0",
                Info::Info0d(_) => "INFO0d",
                Info::Info1c(_) => "INFO1d",
                Info::Info1a(_) => "INFO1a (V.34)",
                Info::Info1aPcm(_) => "INFO1a (V.90)",
            };
            (*side, kind)
        })
        .collect();
    // INFO0a is on the recording but not readable off it: the server's JM is
    // still finishing when it goes, and its 1850 Hz mark is in INFO0a's band.
    assert_eq!(
        kinds,
        vec![(Side::Call, "INFO0d"), (Side::Call, "INFO1d"), (Side::Answer, "INFO1a (V.90)")]
    );
    let Some((_, _, Info::Info1aPcm(asked))) = found.iter().find(|(_, _, i)| matches!(i, Info::Info1aPcm(_))) else {
        panic!("no V.90 INFO1a");
    };
    // "UINFO shall be greater than 66."
    assert_eq!(asked.uinfo, 78);
    assert_eq!(asked.upstream, SymbolRate::S3200);
    assert_eq!(asked.md_length, 20, "700 ms of MD, which is on the recording");

    // And INFO1d, as the server probed the upstream: every symbol rate usable,
    // climbing with the rate, on the low carrier.
    let info1d = found
        .iter()
        .find_map(|(_, _, i)| if let Info::Info1c(x) = i { Some(*x) } else { None })
        .unwrap();
    let rates: Vec<u8> = info1d.probed.iter().map(|p| p.max_rate).collect();
    assert_eq!(rates, vec![5, 6, 6, 7, 8, 9]);
    assert!(!info1d.probed[4].high_carrier, "3200 goes up on the low carrier");
}
