//! A jitter buffer's concealment landing inside the far end's first S.
//!
//! The receiver finds TRN by S turning into S-bar, or by S stopping. A 20 ms
//! hole in S -- silence, comfort noise, or a fragment of the last packet
//! repeated -- stops S just as its end would, and its phase jumps can read as
//! S turning into S-bar. A receiver that trains on the first of those at once
//! spends the next third of a second deaf while the real S-bar and TRN go by,
//! and the call then waits a minute on a rate signal it can never read. That
//! was found in review, at every one of these moments, and none of the other
//! tests puts a concealment in S.
//!
//! The moments are the ones review found failing: late in the answering end's
//! S as the calling end hears it, and late in the calling end's first S as the
//! answering end hears it.

use std::collections::VecDeque;

use datapump::v32::startup::{Modem, Rates, Role, Status, rate_signal};

const FS: f64 = 16_000.0;

/// What fills the hole.
#[derive(Clone, Copy, Debug)]
enum Fill {
    Silence,
    /// Noise at the level of the packet before.
    ComfortNoise,
    /// The last 50 samples of the packet before, again and again, faded in
    /// and out: a concealment that repeats a pitch period.
    Repeat,
}

/// Seconds to connect when 20 ms of `fill` is put into what one end hears at
/// `at` seconds, with `delay` samples each way; `None` if not within 30 s.
fn connects_with_hole(at: f64, into_answering: bool, delay: usize, fill: Fill) -> Option<f64> {
    const HOLE: usize = 320;
    let offer = rate_signal(Rates::between(4800, 14_400));
    let mut calling = Modem::new(Role::Calling, offer, FS);
    let mut answering = Modem::new(Role::Answering, offer, FS);
    let mut to_calling: VecDeque<f64> = VecDeque::from(vec![0.0; delay]);
    let mut to_answering: VecDeque<f64> = VecDeque::from(vec![0.0; delay]);
    let start = (at * FS) as usize;
    let mut recent: VecDeque<f64> = VecDeque::from(vec![0.0; HOLE]);
    let mut late: VecDeque<f64> = VecDeque::new();
    let mut seed = 0x9876_5431u64;
    let mut gauss = move || {
        let mut uniform = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (a, b) = (uniform(), uniform());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    };
    let (mut from_calling, mut from_answering) = (0.0, 0.0);
    for i in 0..(30.0 * FS) as usize {
        to_answering.push_back(from_calling);
        to_calling.push_back(from_answering);
        let mut heard_calling = to_calling.pop_front().unwrap_or(0.0);
        let mut heard_answering = to_answering.pop_front().unwrap_or(0.0);
        let x = if into_answering { &mut heard_answering } else { &mut heard_calling };
        if i == start {
            let rms = (recent.iter().map(|v| v * v).sum::<f64>() / HOLE as f64).sqrt();
            let last: Vec<f64> = recent.iter().copied().collect();
            for k in 0..HOLE {
                let edge = k.min(HOLE - 1 - k);
                let fade = if edge < 40 { edge as f64 / 40.0 } else { 1.0 };
                late.push_back(match fill {
                    Fill::Silence => 0.0,
                    Fill::ComfortNoise => rms * gauss(),
                    Fill::Repeat => last[HOLE - 50 + k % 50] * fade,
                });
            }
        }
        if i >= start {
            late.push_back(*x);
            *x = late.pop_front().unwrap_or(0.0);
        }
        recent.pop_front();
        recent.push_back(*x);
        from_calling = calling.step(heard_calling);
        from_answering = answering.step(heard_answering);
        let _ = (calling.take_bits(), answering.take_bits());
        if matches!(calling.status(), Status::Connected(_)) && matches!(answering.status(), Status::Connected(_)) {
            return Some(i as f64 / FS);
        }
    }
    None
}

#[test]
fn a_hole_in_the_far_end_s_does_not_stop_the_call() {
    let cases = [
        (3.56, false, 1, Fill::Silence),
        (3.56, false, 1, Fill::Repeat),
        (4.25, true, 1, Fill::Silence),
        (4.25, true, 1, Fill::Repeat),
        (4.30, true, 1, Fill::ComfortNoise),
        // 0.7 s each way, as over Rory's VoIP line.
        (7.06, false, 11_200, Fill::Repeat),
        (10.95, true, 11_200, Fill::ComfortNoise),
    ];
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = cases
            .iter()
            .map(|&(at, into_answering, delay, fill)| {
                s.spawn(move || (at, into_answering, delay, fill, connects_with_hole(at, into_answering, delay, fill)))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("a call panicked")).collect()
    });
    let failed: Vec<_> = results.iter().filter(|r| r.4.is_none()).collect();
    assert!(failed.is_empty(), "never connected: {failed:?}");
}
