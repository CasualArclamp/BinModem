//! Whole fax calls at V.27 ter, between two `FaxCall` ends, over a line with
//! hiss on it the whole way through.
//!
//! The rest of the tree's fax calls run over a silent line, or one whose
//! noise is uniform and loud enough that the page carrier was always found
//! one rung down (fax-qam.md 3.1). A real line is never silent between the
//! bursts, and the burst has to be found in whatever the line carries while
//! the procedure waits for it. So here the hiss is Gaussian, seeded, at a
//! stated signal to noise, and on the line from the first sample; and in one
//! call a jitter buffer plays 20 ms of made-up audio into the middle of the
//! page burst as well.
//!
//! Signal to noise is Es/N0 at 1600 baud against the page carrier's power,
//! as `datapump/tests/v27ter_receiver.rs` has it.

use fax::call::Phase;
use fax::page::{Page, Resolution};
use fax::t30::Modulation;
use modem::FaxCall;

const FS: f64 = 16_000.0;

/// A page with something recognisable on it.
fn a_page(lines: usize) -> Page {
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines)
            .map(|y| (0..width).map(|x| (x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30).collect())
            .collect(),
        resolution: Resolution::Standard,
    }
}

#[derive(Debug, Clone)]
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

/// One direction of the line: hiss, and a jitter buffer that plays the last
/// 20 ms again, faded in and out, once the far end has been sending its page
/// for `slip_after` seconds.
struct Direction {
    noise: Random,
    sigma: f64,
    slip_after: Option<f64>,
    sending: f64,
    recent: Vec<f64>,
    replay: Vec<f64>,
    delayed: std::collections::VecDeque<f64>,
}

impl Direction {
    fn new(seed: u64, snr_db: f64, slip_after: Option<f64>) -> Self {
        // The page carrier's power is a half; Es/N0 at 1600 baud.
        let variance = (FS / 2.0) / 1600.0 * 0.5 * 10f64.powf(-snr_db / 10.0);
        Self {
            noise: Random::new(seed),
            sigma: variance.sqrt(),
            slip_after,
            sending: 0.0,
            recent: Vec::new(),
            replay: Vec::new(),
            delayed: std::collections::VecDeque::new(),
        }
    }

    fn carry(&mut self, x: f64, page_going: bool) -> f64 {
        let n = (0.020 * FS) as usize;
        self.recent.push(x);
        if self.recent.len() > n {
            self.recent.remove(0);
        }
        if page_going {
            self.sending += 1.0 / FS;
        }
        if let Some(after) = self.slip_after
            && self.sending > after
        {
            // The buffer runs dry and conceals: the 20 ms just gone, again,
            // and everything after it that much later.
            self.slip_after = None;
            let fade = 40;
            self.replay = self
                .recent
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let edge = i.min(n - 1 - i);
                    if edge < fade { v * edge as f64 / fade as f64 } else { *v }
                })
                .collect();
            self.replay.reverse();
        }
        let out = if let Some(made_up) = self.replay.pop() {
            self.delayed.push_back(x);
            made_up
        } else if let Some(late) = self.delayed.pop_front() {
            self.delayed.push_back(x);
            late
        } else {
            x
        };
        out + self.sigma * self.noise.gaussian()
    }
}

/// A call between `caller` and `answerer` down a line with hiss at `snr_db`
/// each way, the page burst slipped 20 ms `slip_after` seconds in.
///
/// `FAX_TRACE=1` prints every change of phase at both ends, and every
/// control frame each end heard, as `faxcall.rs`'s own tests do.
fn call(mut caller: FaxCall, mut answerer: FaxCall, snr_db: f64, slip_after: Option<f64>, seed: u64) -> (FaxCall, FaxCall) {
    let mut to_answerer = Direction::new(seed, snr_db, slip_after);
    let mut to_caller = Direction::new(seed + 1, snr_db, None);
    let (mut from_caller, mut from_answerer) = (0.0, 0.0);
    let trace = std::env::var("FAX_TRACE").is_ok();
    let (mut was_a, mut was_b) = (caller.phase(), answerer.phase());
    for i in 0..(FS * 90.0) as usize {
        let a = caller.step(from_answerer);
        let b = answerer.step(from_caller);
        from_caller = to_answerer.carry(a, caller.phase() == Phase::Sending);
        from_answerer = to_caller.carry(b, false);
        if trace {
            let at = i as f64 / FS;
            for m in caller.take_heard() {
                eprintln!("{at:6.2}s caller heard {:?} {:02x?}", m.frame, m.fif);
            }
            for m in answerer.take_heard() {
                eprintln!("{at:6.2}s answerer heard {:?} {:02x?}", m.frame, m.fif);
            }
            if caller.phase() != was_a || answerer.phase() != was_b {
                eprintln!("{at:6.2}s caller {:<32} answerer {}", caller.phase().name(), answerer.phase().name());
                (was_a, was_b) = (caller.phase(), answerer.phase());
            }
        }
        if caller.phase().is_over() && answerer.phase().is_over() {
            break;
        }
    }
    (caller, answerer)
}

/// The two ends of a call from here with one page, at V.27 ter only.
fn ends(page: &Page) -> (FaxCall, FaxCall) {
    (
        FaxCall::originate_pages(FS, "61399990000", vec![page.clone()]).offering(&[Modulation::V27ter]),
        FaxCall::answer(FS, "61388880000"),
    )
}

#[test]
fn a_page_crosses_at_4800_over_a_line_with_hiss_on_it() {
    // The receiver this replaced found its carrier in the hiss and settled
    // the call at 2400 on both of these (fax-qam.md 3.1). The fax layer also
    // turns the line round to listen for the page some 75 ms into the far
    // end's turn-on, after the whole of segment 3: the page's burst is found
    // by its conditioning alone.
    let mut failed = Vec::new();
    for (snr_db, seed) in [(30.0, 1), (22.0, 2)] {
        let page = a_page(40);
        let (caller, answerer) = ends(&page);
        let (caller, answerer) = call(caller, answerer, snr_db, None, seed);
        let whole = answerer.received().is_some_and(|got| got.lines == page.lines);
        eprintln!("at {snr_db} dB: rate {}, page {}, ({:?})", caller.rate(), if whole { "whole" } else { "not whole" }, caller.trouble());
        if !whole || caller.rate() != 4800 {
            failed.push((snr_db, caller.rate(), whole));
        }
    }
    assert!(failed.is_empty(), "(dB, rate, page whole) that failed: {failed:?}");
}

#[test]
fn a_page_crosses_at_4800_through_a_slip_in_the_middle_of_it() {
    // Error correction mode, so that the frame the slip spoils is sent again
    // and the page still arrives whole: the receiver finds the signal again
    // after the jump rather than losing the rest of the burst.
    let page = a_page(120);
    let (caller, answerer) = ends(&page);
    let (caller, answerer) =
        call(caller.with_error_correction(true), answerer.with_error_correction(true), 30.0, Some(1.0), 3);
    let whole = answerer.received().is_some_and(|got| got.lines == page.lines);
    eprintln!("slip: rate {}, ecm {}, page {}", caller.rate(), caller.error_correction(), if whole { "whole" } else { "not whole" });
    let got = answerer.received().unwrap_or_else(|| panic!("no page arrived ({:?})", answerer.trouble()));
    assert_eq!(got.lines, page.lines, "the page came out different");
    assert_eq!(caller.rate(), 4800, "the call dropped a rate");
}

#[test]
fn a_line_too_noisy_for_4800_falls_back_to_2400() {
    // Eight phases at 1600 baud need about 17 dB; four at 1200 baud about
    // 12, and the slower symbols carry 1.25 dB more energy each. At 13 dB
    // the training check at 4800 cannot pass and the page at 2400 should not
    // fail.
    let page = a_page(20);
    let (caller, answerer) = ends(&page);
    let (caller, answerer) = call(caller, answerer, 13.0, None, 5);
    let whole = answerer.received().is_some_and(|got| got.lines == page.lines);
    eprintln!("at 13 dB: rate {}, page {} ({:?})", caller.rate(), if whole { "whole" } else { "not whole" }, caller.trouble());
    let got = answerer.received().unwrap_or_else(|| panic!("no page arrived ({:?})", answerer.trouble()));
    assert_eq!(got.lines, page.lines, "the page came out different");
    assert_eq!(caller.rate(), 2400, "the call should have settled at 2400");
}