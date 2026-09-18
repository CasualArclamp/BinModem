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
                Info::Info1aPcmUp(_) => "INFO1a (V.92 PCM upstream)",
                Info::Info1aV34Up(_) => "INFO1a (V.92 Table 19)",
                Info::Mh(_) => "MH",
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

/// The analogue modem's DIL descriptor, as it went up in Ja: the upstream
/// read with V.34's receiver, since Ja is V.34's modulation (8.3.1).
fn ja() -> Option<datapump::v90::sequences::Descriptor> {
    use datapump::v32::Mode;
    use datapump::v34::qam::Band;
    use datapump::v34::receiver::{Heard, Receiver, Reference};
    use datapump::v34::signals::{Reader, Size};
    use datapump::v90::sequences::Descriptor;

    let (fs, samples) = samples();
    // INFO1d put the upstream on the low carrier at 3200.
    let mut rx = Receiver::new(datapump::v34::qam::Band::new(SymbolRate::S3200, false), fs);
    let _: Band = rx.band();
    rx.hunt();
    let mut reader = Reader::new(Mode::Answer);
    let mut bits: Vec<bool> = Vec::new();
    let (mut in_trn, mut ones) = (true, 0);
    let first = (10.0 * fs) as usize;
    for &x in &samples[first..(12.6 * fs) as usize] {
        rx.feed(f64::from(x));
        while let Some(heard) = rx.heard() {
            match heard {
                // The second S-bar, after MD: the first is at 9.4 s.
                Heard::Reversal { at } => rx.train(Reference::PpThenTrn, Mode::Answer, at),
                Heard::Symbol(symbol) => {
                    if in_trn {
                        let got = reader.trn(symbol.decided, Size::Four);
                        if got.iter().all(|b| *b) || ones < 46 {
                            ones += 2;
                            continue;
                        }
                        in_trn = false;
                    }
                    bits.extend(reader.differential(symbol.decided, Size::Four));
                }
                _ => {}
            }
        }
    }
    (0..bits.len().saturating_sub(18)).find_map(|s| {
        let fresh = s == 0 || !bits[s - 1];
        (fresh && bits[s..s + 17].iter().all(|b| *b) && !bits[s + 17]).then(|| Descriptor::from_bits(&bits[s..])).flatten()
    })
}

#[test]
fn the_analogue_modem_s_ja_asks_for_a_dil_of_147_segments() {
    let d = ja().expect("no DIL descriptor checked");
    assert_eq!(d.ucodes.len(), 147);
    assert_eq!(d.signs.len(), 126);
    assert_eq!(d.training.len(), 126);
    assert_eq!(d.h, [20, 20, 20, 20, 20, 20, 11, 11]);
    assert_eq!(d.refs, [78; 8], "every reference is UINFO");
    // Ucodes 0 to 117 in order, with UINFO after every fourth.
    let training: Vec<u8> = d.ucodes.iter().copied().enumerate().filter(|(i, _)| i % 5 != 4).map(|(_, u)| u).collect();
    assert_eq!(training, (0..118).collect::<Vec<u8>>());
    assert!(d.ucodes.iter().skip(4).step_by(5).all(|&u| u == 78));
    assert_eq!(d.len(), 17_334);
}

/// The server's phase 3, heard the way the analogue modem hears it.
struct Downstream {
    trained_db: f64,
    inverted: bool,
    /// The last Jd, and the symbol after it.
    jd: Option<(u64, datapump::v90::sequences::Jd)>,
    /// Where the DIL began, once J'd has been read.
    dil_from: Option<u64>,
    /// Symbols from TRN1d on.
    symbols: Vec<datapump::v90::pcm::Symbol>,
}

fn downstream() -> Downstream {
    use datapump::v32::{Mode, Scrambler};
    use datapump::v90::pcm::{Heard, Receiver};
    use datapump::v90::sequences::{JD_BITS, JD_PRIME_BITS, Jd};
    use datapump::v90::ucode::{self, Law};

    let (fs, samples) = samples();
    let dil: Vec<f64> = ja()
        .expect("no DIL descriptor")
        .symbols()
        .map(|(u, positive)| ucode::level(Law::Mu, u) * if positive { 1.0 } else { -1.0 })
        .collect();
    let mut rx = Receiver::new(Law::Mu, fs);
    rx.hunt(78);
    let mut out = Downstream { trained_db: 0.0, inverted: false, jd: None, dil_from: None, symbols: Vec::new() };
    let mut descrambler = Scrambler::new(Mode::Call);
    let (mut differential, mut previous) = (false, false);
    let mut bits: Vec<bool> = Vec::new();
    let first = (12.0 * fs) as usize;
    for &x in &samples[first..(19.0 * fs) as usize] {
        rx.feed(f64::from(x));
        while let Some(heard) = rx.heard() {
            match heard {
                Heard::Trained { snr_db, inverted } => {
                    out.trained_db = snr_db;
                    out.inverted = inverted;
                }
                Heard::Symbol(s) => {
                    out.symbols.push(s);
                    if out.dil_from.is_some() {
                        continue;
                    }
                    let sign = s.positive();
                    let before = descrambler.clone();
                    let mut bit = descrambler.descramble(if differential { sign ^ previous } else { sign });
                    if !differential && !bit {
                        // TRN1d is over: Jd is differential, from this
                        // symbol on.
                        differential = true;
                        descrambler = before;
                        bit = descrambler.descramble(sign ^ previous);
                    }
                    previous = sign;
                    bits.push(bit);
                    if bits.len() >= JD_BITS
                        && let Some(jd) = Jd::from_bits(&bits[bits.len() - JD_BITS..])
                    {
                        out.jd = Some((s.index + 1, jd));
                    }
                    // J'd: twelve zeros where the next Jd's sync would be,
                    // and the DIL straight after (9.3.1.6).
                    if let Some((end, _)) = out.jd
                        && s.index + 1 == end + JD_PRIME_BITS as u64
                        && bits[bits.len() - JD_PRIME_BITS..].iter().all(|b| !*b)
                    {
                        out.dil_from = Some(s.index + 1);
                        rx.expect(dil.iter().copied());
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[test]
fn the_server_s_trn1d_trains_the_downstream_receiver_and_jd_checks() {
    let heard = downstream();
    // The line is noisy: 19 dB is what a least-squares fit over the whole of
    // TRN1d gets.
    assert!(heard.trained_db > 14.0, "trained to {:.1} dB", heard.trained_db);
    assert!(heard.inverted, "this line turns the signal over");
    let (end, jd) = heard.jd.expect("no Jd checked");
    assert_eq!(jd.rates, datapump::v90::sequences::Jd::ALL_RATES);
    assert!(!jd.sixteen_in_training);
    assert_eq!(jd.lookahead, 1);
    // Jd ends on a frame boundary, after four seconds of TRN1d -- 9.3.1.4's
    // "within 4000 ms" of starting it, give or take the line -- and nineteen
    // repetitions of itself.
    assert_eq!(end % 6, 0);
    assert_eq!(end, 33_816);
    assert_eq!(heard.dil_from, Some(end + 12), "no J'd after the last Jd");
}

#[test]
fn the_dil_that_arrives_is_the_dil_the_analogue_modem_asked_for() {
    use datapump::v90::ucode::{self, Law};
    let heard = downstream();
    let descriptor = ja().expect("no DIL descriptor");
    let start = heard.dil_from.expect("no J'd");
    let wanted: Vec<f64> = descriptor
        .symbols()
        .map(|(u, positive)| ucode::level(Law::Mu, u) * if positive { 1.0 } else { -1.0 })
        .collect();
    let got: Vec<f64> = heard.symbols.iter().filter(|s| s.index >= start).map(|s| s.value).take(wanted.len()).collect();
    assert_eq!(got.len(), wanted.len(), "the recording ends inside the DIL");
    let signal: f64 = wanted.iter().map(|w| w * w).sum();
    let error: f64 = got.iter().zip(&wanted).map(|(g, w)| (g - w).powi(2)).sum();
    let snr = 10.0 * (signal / error).log10();
    // As well as TRN1d fits, which it would not if a pattern, a length or a
    // reference were read wrong.
    assert!(snr > 14.0, "the DIL fits to {snr:.1} dB");
    // And shifted by a symbol either way it does not fit at all.
    for shift in [1usize, 6] {
        let error: f64 = got[shift..].iter().zip(&wanted).map(|(g, w)| (g - w).powi(2)).sum();
        assert!(10.0 * (signal / error).log10() < 3.0, "the DIL fits {shift} symbols off too");
    }
}
