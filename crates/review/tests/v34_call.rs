//! A call these modems made, read by a third party that took no part in it.
//!
//! Two modems call each other on V.34 and each end's transmission is kept on
//! its own channel, as a capture of the two halves of a call has it. The
//! reader gets the recording and nothing else: not what was agreed, not what
//! was typed. Everything it reports it has to have got off the line.

use line::wav::Wav;
use modem::{Modem, State};
use review::{End, Layer};

const FS: f64 = 16_000.0;

fn type_at(modem: &mut Modem, line: &str) {
    for b in line.bytes() {
        modem.feed_dte(b);
    }
    modem.feed_dte(b'\r');
}

/// A V.34 call of `seconds`, the caller on the first channel, with a line
/// typed at each end once it is up.
fn call(seconds: f64) -> Wav {
    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        type_at(m, "AT+MS=V34");
        // Compression off, so that what was typed is in the frames as typed.
        type_at(m, "AT+DS=0;+DS44=0");
        m.take_dte();
    }
    type_at(&mut host, "ATA");
    type_at(&mut caller, "ATD5551234");
    let mut samples = Vec::new();
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let (mut up_at, mut spoken) = (None, false);
    for n in 0..(seconds * FS) as usize {
        let now = n as f64 / FS;
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);
        // A quarter of the modem's own scale, whose peaks are past full scale.
        samples.push(0.25 * from_caller as f32);
        samples.push(0.25 * from_host as f32);
        caller.take_dte();
        host.take_dte();
        if up_at.is_none() && caller.state() == State::Data && host.state() == State::Data {
            up_at = Some(now);
        }
        if let Some(at) = up_at
            && !spoken
            && now > at + 1.0
        {
            spoken = true;
            for b in b"Hello from the calling modem.\r\n" {
                caller.feed_dte(*b);
            }
            for b in b"And hello back from the answering one.\r\n" {
                host.feed_dte(*b);
            }
        }
    }
    assert!(up_at.is_some(), "the two modems never connected");
    // As a file would have it: sixteen bits a sample.
    let samples = samples.into_iter().map(|x| (x * 32768.0).round().clamp(-32768.0, 32767.0) / 32768.0).collect();
    Wav { sample_rate: FS as u32, channels: 2, samples }
}

#[test]
fn a_v34_call_is_read_down_to_what_was_typed() {
    let report = review::analyse(&call(13.0), "a V.34 call");
    let tags = |from: End, layer: Layer| -> Vec<&str> {
        report.events.iter().filter(|e| e.from == from && e.layer == layer).map(|e| e.tag.as_str()).collect()
    };
    assert_eq!(report.tap, review::Tap::Split { caller: 0 }, "the caller is on the first channel");

    // Set-up and phase 2, each end's own.
    assert!(report.find("ANSam").is_some(), "no answer tone: {}", report.text(false));
    assert!(tags(End::Caller, Layer::V8).contains(&"CM"));
    assert!(tags(End::Answerer, Layer::V8).contains(&"JM"));
    assert!(tags(End::Caller, Layer::Phase2).contains(&"INFO1c"));
    assert!(tags(End::Answerer, Layer::Phase2).contains(&"INFO1a"));

    // Phases 3 and 4, in the order each end sends them.
    for end in [End::Caller, End::Answerer] {
        let third = tags(end, Layer::Phase3);
        assert!(third.starts_with(&["S", "S-bar", "PP", "TRN", "J"]), "{end:?} phase 3 was {third:?}");
        let fourth = tags(end, Layer::Phase4);
        assert!(fourth.contains(&"TRN") && fourth.contains(&"MP'") && fourth.ends_with(&["E"]), "{end:?} phase 4 was {fourth:?}");
    }
    assert!(tags(End::Caller, Layer::Phase3).contains(&"J'"), "only the call modem sends J'");

    // Data mode at the rate the two modems themselves reported.
    for end in [End::Caller, End::Answerer] {
        let data = report.events.iter().find(|e| e.from == end && e.tag == "data").expect("no data mode");
        assert!(data.summary.contains("33600"), "{end:?}: {}", data.summary);
        assert!(!data.symbols.is_empty(), "no symbols kept for the plot");
    }

    // The link, and what crossed it.
    assert!(tags(End::Caller, Layer::Link).contains(&"SABME"));
    assert!(tags(End::Answerer, Layer::Link).contains(&"UA"));
    let said = |from: End| -> Vec<&str> {
        report.events.iter().filter(|e| e.from == from && e.tag == "text").map(|e| e.summary.as_str()).collect()
    };
    assert_eq!(said(End::Caller), ["Hello from the calling modem.\\r\\n"]);
    assert_eq!(said(End::Answerer), ["And hello back from the answering one.\\r\\n"]);
}
