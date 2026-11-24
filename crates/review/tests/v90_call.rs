//! A V.90 call, read by a third party that took no part in it.
//!
//! One of these modems dials a digital modem across a telephone network,
//! and each end is kept on its own channel at the network's rate, as a
//! capture of the two RTP streams of such a call has it. The reader gets
//! the recording and nothing else.

#[path = "../examples/shared/pcm_pair.rs"]
mod pcm_pair;

use line::wav::Wav;
use review::{End, Layer};

#[test]
fn a_v90_call_is_read_down_to_what_was_typed() {
    let call = pcm_pair::record(13.0, true);
    let (down, up) = call.rates.expect("the two modems never connected");
    // As a file would have it: sixteen bits a sample.
    let samples = call
        .samples
        .iter()
        .map(|x| (x * 32768.0).round().clamp(-32768.0, 32767.0) / 32768.0)
        .collect();
    let report = review::analyse(
        &Wav {
            sample_rate: 8000,
            channels: 2,
            samples,
        },
        "a V.90 call",
    );
    let tags = |from: End, layer: Layer| -> Vec<&str> {
        report
            .events
            .iter()
            .filter(|e| e.from == from && e.layer == layer)
            .map(|e| e.tag.as_str())
            .collect()
    };
    let text = report.text(false);

    // Phase 2: the digital modem takes the call modem's part though it
    // answered.
    assert!(
        tags(End::Answerer, Layer::Phase2).contains(&"INFO0d"),
        "{text}"
    );
    assert!(
        tags(End::Caller, Layer::Phase2).contains(&"INFO1a"),
        "{text}"
    );

    // Phase 3, each end's.
    let third = tags(End::Caller, Layer::Phase3);
    assert!(
        third.starts_with(&["S", "S-bar", "PP", "TRN", "Ja"]),
        "the analogue modem's phase 3 was {third:?}"
    );
    assert_eq!(
        tags(End::Answerer, Layer::Phase3),
        ["Sd", "S-bar-d", "TRN1d", "Jd", "J'd", "DIL"]
    );

    // Phase 4: what the analogue modem asked for, and the digital modem
    // doing it.
    let fourth = tags(End::Caller, Layer::Phase4);
    assert!(
        fourth.contains(&"CPt") && fourth.contains(&"CP") && fourth.ends_with(&["E"]),
        "{fourth:?}"
    );
    let fourth = tags(End::Answerer, Layer::Phase4);
    assert!(
        fourth.starts_with(&["Ri", "R-bar-i", "TRN2d", "MP"]),
        "the digital modem's phase 4 was {fourth:?}"
    );
    assert!(
        fourth.ends_with(&["Ed", "B1d"]),
        "the digital modem's phase 4 was {fourth:?}"
    );

    // Data mode at the rates the analogue modem itself reported.
    let data = |from: End| {
        report
            .events
            .iter()
            .find(|e| e.from == from && e.tag == "data")
            .expect("no data mode")
    };
    assert!(
        data(End::Answerer).summary.contains(&down.to_string()),
        "{} is not {down}",
        data(End::Answerer).summary
    );
    assert!(
        data(End::Caller).summary.contains(&up.to_string()),
        "{} is not {up}",
        data(End::Caller).summary
    );

    // And what crossed it, which is what each end itself received.
    let said = |from: End| -> Vec<&str> {
        report
            .events
            .iter()
            .filter(|e| e.from == from && e.tag == "text")
            .map(|e| e.summary.as_str())
            .collect()
    };
    assert_eq!(said(End::Caller), ["Hello from the analogue modem.\\r\\n"]);
    assert_eq!(
        said(End::Answerer),
        ["And hello back from the server.\\r\\n"]
    );
    assert_eq!(call.server_received, b"Hello from the analogue modem.\r\n");
    assert!(String::from_utf8_lossy(&call.caller_saw).contains("And hello back from the server."));
}
