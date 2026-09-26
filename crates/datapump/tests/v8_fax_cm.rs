//! A V.34 fax's call menu, off a real call, and the answer to it.
//!
//! `tests/vectors/fax-v34-cm.wav` is four seconds of what arrived when a fax
//! called this modem on 2026-09-26: V.21's low channel, the same four octets
//! over and over. This end had answered as T.30 has a fax without V.34 answer
//! -- the plain 2100 Hz tone, then a DIS -- and the caller sent a V.8 call
//! menu anyway and never stopped, waiting for a joint menu that nothing here
//! knew how to send. 7.2/V.8 forbids a CM without ANSam, so the caller took
//! the plain tone for ANSam; what it wants back is a JM all the same.
//!
//! The recording is the far end only, taken from what the window played to
//! the speakers, so it has been through the softphone and a lossy codec on
//! the way. That it reads at all is part of the point.

use datapump::bell103::{Bell103Rx, Bell103Tx};
use datapump::framing::AsyncBits;
use datapump::v8::{HIGH, LOW, Modem, Status};
use v8::{CallFunction, Decoder, Heard, Menu, Modulation, Modulations, Protocol};

const VECTOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/fax-v34-cm.wav");

/// What this end offers: the three fax modulations it has pumps for.
fn ours() -> Modulations {
    Modulations::of(&[Modulation::V17, Modulation::V29HalfDuplex, Modulation::V27ter])
}

fn recording() -> (f64, Vec<f32>) {
    let wav = line::wav::read(VECTOR).expect("could not read the vector");
    (f64::from(wav.sample_rate), wav.channel(0))
}

#[test]
fn a_real_super_g3_call_menu_is_read_and_answered() {
    let (fs, samples) = recording();
    let mut modem = Modem::overhearing(CallFunction::TransmitFax, ours(), fs);
    let mut rx = Bell103Rx::with_tones(HIGH.0, HIGH.1, fs);
    let mut decoder = Decoder::new();
    let mut joint = None;
    let mut answered_at = None;
    for (i, &s) in samples.iter().enumerate() {
        let out = modem.step(f64::from(s));
        if answered_at.is_none() && modem.has_the_line() {
            answered_at = Some(i as f64 / fs);
        }
        if let Some(octet) = rx.feed(out)
            && let Some(Heard::Cm(m) | Heard::Jm(m)) = decoder.feed(octet)
        {
            joint.get_or_insert(m);
        }
    }

    // E0 81 85 D4: the menu synchronisation; "transmit facsimile from call
    // terminal" (Table 4/T.30); a modulation octet with V.34 half-duplex; an
    // extension octet with V.17, V.29 half-duplex and V.27 ter.
    let far = modem.far_menu().expect("no call menu was read off the recording");
    assert_eq!(
        far,
        Menu {
            function: CallFunction::TransmitFax,
            modulations: Modulations::of(&[
                Modulation::V34HalfDuplex,
                Modulation::V17,
                Modulation::V29HalfDuplex,
                Modulation::V27ter,
            ]),
            protocol: Protocol::Unstated,
            access: None,
            pcm: None,
        }
    );
    // Each menu is a sixth of a second, and two identical ones are wanted;
    // a line this rough should not need many more than that.
    let at = answered_at.expect("the call menu went unanswered");
    assert!(at < 1.5, "took {at:.2} s of call menus to answer one");

    let joint = joint.expect("nothing readable went out on the high channel");
    assert_eq!(joint.function, CallFunction::TransmitFax, "{joint:?}");
    assert_eq!(joint.modulations, ours(), "{joint:?}");
    assert_eq!(modem.status(), Status::Negotiating, "the recording has no CJ in it");
    assert!(modem.has_the_line());
}

/// After the recording, a CJ: the exchange ends on V.17, which is where
/// T.30 6.1.6 sends a call whose ends do not share V.34.
#[test]
fn the_real_caller_and_this_end_settle_on_v17() {
    let (fs, samples) = recording();
    let mut modem = Modem::overhearing(CallFunction::TransmitFax, ours(), fs);
    for &s in &samples {
        modem.step(f64::from(s));
    }
    let framing = AsyncBits::new(8);
    let mut cj = Bell103Tx::with_tones(LOW.0, LOW.1, fs);
    cj.set_transmitting(true);
    for octet in v8::CJ {
        cj.push_bits(&framing.encode(octet));
    }
    while cj.pending_bits() > 0 {
        modem.step(cj.next_sample());
    }
    for _ in 0..(fs * 0.2) as usize {
        modem.step(0.0);
    }
    assert_eq!(modem.status(), Status::Agreed(Modulation::V17));
}
