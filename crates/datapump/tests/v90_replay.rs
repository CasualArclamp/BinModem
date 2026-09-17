//! Replay a recorded V.90 call's downstream through the analogue modem, and
//! say what each stage of the start-up made of it.
//!
//! Ignored, because it needs a capture, and captures are not in the
//! repository. The modem's own replay (`crates/modem/tests/replay.rs`) shows
//! when V.8 ended and V.90's phase 2 began; give that as a sample number:
//!
//! ```text
//! V90_CAPTURE=dist/captures/live-1789599183.wav V90_START=88704 \
//!     cargo test -p datapump --release --test v90_replay -- --ignored --nocapture
//! ```
//!
//! Only the first channel is fed in, from that sample on: the modem does what
//! it did on the day, with the far end's own signal. What it does after the
//! first thing it would have done differently -- a DIL finished sooner, say --
//! the recording cannot answer.

use datapump::v90::startup::Analogue;

const FS: f64 = 16_000.0;

#[test]
#[ignore = "needs a capture; see the module comment"]
fn probe_replay_v90() {
    let (Ok(path), Ok(start)) = (std::env::var("V90_CAPTURE"), std::env::var("V90_START")) else {
        println!("set V90_CAPTURE and V90_START to run this");
        return;
    };
    let start: usize = start.parse().expect("V90_START is a sample number");
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(f64::from(wav.sample_rate), FS, "the modem is built for 16 kHz");
    let arrived = wav.channel(0);
    let mut modem = Analogue::new(FS);
    let mut last = None;
    for (i, &s) in arrived.iter().enumerate().skip(start) {
        modem.step(f64::from(s));
        let v90 = modem.v90();
        let now = (
            modem.phase(),
            v90.map(|v| v.dil_progress().0 / 500),
            v90.map(|v| v.receiver().is_lost()),
            v90.map(|v| v.dil_moved() + v.frames_moved()),
        );
        if last != Some(now) {
            let detail = v90
                .map(|v| {
                    let rx = v.receiver();
                    let (read, of, searching) = v.dil_progress();
                    format!(
                        "snr {:5.1} dB, trained {:4.1}, drift {:6.1} ppm, frame offset {}, Jd {}, DIL {read}/{of}{}{}, moved {}, lost {}",
                        rx.snr_db(),
                        rx.trained_snr_db(),
                        rx.drift_ppm(),
                        rx.frame_offset(),
                        if v.far_jd().is_some() { "read" } else { "-" },
                        if searching { " (searching)" } else { "" },
                        if v.dil_found_late() { " (found without J'd)" } else { "" },
                        v.dil_moved() + v.frames_moved(),
                        rx.is_lost(),
                    )
                })
                .unwrap_or_default();
            println!("{:8.3}  {:<28} {detail}{}", i as f64 / FS, now.0, modem.round_trip().map(|r| format!(" round trip {r:.3} s")).unwrap_or_default());
            last = Some(now);
        }
    }
    println!("ended {:?}, last failure {:?}", modem.status(), modem.last_failure());
}
