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
//!
//! What the choice at the end of the DIL comes to is shown whatever became of
//! it: the rate unshaped and the rate with the spectral shaping asked for, and
//! what they went on. And every line the transcript would have had of phase
//! 4 -- each CP sent and MP found, E, Ed and B1d -- at the time it came.

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
    // The V.90 modem as it was a sample ago, near the end of the DIL: one
    // whose choice ends in a fallback is gone the moment it has chosen.
    let mut before: Option<datapump::v90::analogue::Modem> = None;
    for (i, &s) in arrived.iter().enumerate().skip(start) {
        if let Some(v) = modem.v90()
            && v.route().is_none()
        {
            let (read, of, _) = v.dil_progress();
            before = (read > 0 && read + 64 >= of).then(|| v.clone());
        }
        modem.step(f64::from(s));
        // What phase 4 did, as the transcript tells it.
        for text in modem.take_notes() {
            println!("{:8.3}  {text}", i as f64 / FS);
        }
        if let Some(mut v) = before.take_if(|_| modem.v90().is_none_or(|v| v.route().is_some())) {
            let took = std::time::Instant::now();
            v.step(f64::from(s));
            println!("{:8.3}  the choice took {:.1} ms", i as f64 / FS, took.elapsed().as_secs_f64() * 1e3);
            chosen(&v, i as f64 / FS);
        }
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

/// What the DIL and TRN1d came to: what was chosen, shaped and not, and what
/// the choice went on.
fn chosen(v: &datapump::v90::analogue::Modem, at: f64) {
    use datapump::v90::{dil, sequences, shaping};
    let Some(route) = v.route() else { return };
    let law = v.settings().law;
    let limit = datapump::v90::power_limit(&v.settings().server);
    let jd = v.far_jd().unwrap_or_default();
    let leftover = v.receiver().residue().leftover();
    let rate = |c: &dil::Choice| sequences::data_rate(c.data.drn);
    let unshaped = dil::choose(route, law, limit, |drn| jd.enables(drn));
    let asked = shaping::choose(route, law, limit, |drn| jd.enables(drn), jd.lookahead, leftover.as_ref());
    println!(
        "{at:8.3}  unshaped {:?}; chosen {:?} with {:?}, leaving {:.2} of the error; V.34 would carry {}",
        unshaped.as_ref().and_then(rate),
        asked.as_ref().map(shaping::Asked::rate),
        asked.as_ref().map(|a| a.shaping),
        asked.as_ref().map_or(1.0, |a| a.left),
        v.settings().v34_receive
    );
    match leftover {
        Some(l) => {
            let carried: f64 = l.response.iter().map(|c| c * c).sum::<f64>() - l.misread;
            println!(
                "          TRN1d and Jd left {:.1} dB carried and {:.1} dB of noise, under the signal",
                10.0 * carried.max(1e-12).log10(),
                10.0 * (l.noise / l.power).max(1e-12).log10()
            );
        }
        None => println!("          too little of TRN1d and Jd read to say what shaping would do"),
    }
    for line in dil::explain(route, law, limit) {
        println!("          {line}");
    }
    // How the spread goes with the level: a floor is noise, a share of the
    // level is something the signal brings with it.
    let spreads: Vec<String> = (8..100u8)
        .step_by(10)
        .map(|u| format!("{u}: {:.1e} at {:.3}", route.spread[usize::from(u)], datapump::v90::ucode::level(law, u)))
        .collect();
    println!("          spread by codeword {}", spreads.join(", "));
}
