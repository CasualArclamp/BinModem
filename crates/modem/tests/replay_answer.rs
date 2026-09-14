//! A recorded fax call, answered again by the code that answers them now.
//!
//! ```text
//! FAX_CAPTURE=F:/dialupmodem2/dist/captures/live-1789349396.wav \
//!     cargo test -p modem --test replay_answer -- --ignored --nocapture
//! ```
//!
//! Only the far end's half of the recording goes in, so this end's replies
//! reach nobody and the far end carries on as it did at the time. That is
//! enough for the first exchange: the far end's command, its training and its
//! training check arrive exactly as they arrived, and what matters is whether
//! this end would now accept them.

use fax::call::Phase;
use modem::FaxCall;

#[test]
#[ignore = "needs a recording"]
fn the_far_ends_training_check_is_accepted() {
    let path = std::env::var("FAX_CAPTURE").expect("set FAX_CAPTURE");
    let from: f64 = std::env::var("FAX_FROM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.3);
    let wav = line::wav::read(&path).expect("could not read the recording");
    let fs = f64::from(wav.sample_rate);
    let far = wav.channel(0);

    let mut call = FaxCall::answer(fs, "61388880000");
    // Through the called tone and our own identification, into waiting for a
    // command, with nothing on the line.
    let mut waited = 0usize;
    while call.phase() != Phase::AwaitingCommand {
        call.step(0.0);
        waited += 1;
        assert!(waited < (fs * 10.0) as usize, "never got to waiting");
    }

    let mut phases = vec![(0.0, call.phase())];
    for (i, &s) in far.iter().enumerate().skip((from * fs) as usize) {
        call.step(f64::from(s));
        if phases.last().map(|p| p.1) != Some(call.phase()) {
            phases.push((i as f64 / fs, call.phase()));
        }
        if call.phase() == Phase::Receiving {
            break;
        }
    }
    for (at, phase) in &phases {
        println!("  {at:6.2}s  {}", phase.name());
    }
    assert!(
        phases.iter().any(|p| p.1 == Phase::Receiving),
        "the training check was not accepted"
    );
}
