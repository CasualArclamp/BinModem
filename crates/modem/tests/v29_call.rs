//! Whole fax calls at V.29, between two `FaxCall`s, through the modem crate.
//!
//! The receiver's own proofs are `crates/datapump/tests/v29_receiver.rs`;
//! these are what they come to in a call. A page over a line with hiss on it
//! used to lose its top rate at 53 dB of signal to noise -- better than any
//! real telephone circuit -- because the V.29 receiver's carrier detector
//! latched on to the hiss before the training check arrived
//! (`docs/design/slow-modes/fax-qam.md` 3.1). And a call that cannot train at
//! one rate has to find the next one down, which needs every receiver in the
//! ladder to hear its burst after the one before it was spoiled.
//!
//! `cargo test -p modem --release --test v29_call -- --nocapture` prints the
//! rate each call settled at.

use fax::call::Phase;
use fax::page::{Page, Resolution};
use fax::t30::Modulation;
use modem::FaxCall;

const FS: f64 = 16_000.0;

/// Something like a page of text, `lines` long: bands of short runs with
/// white between them, as `fullpage.rs` draws it.
fn a_page(lines: usize) -> Page {
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        let band = (y / 24) % 3 != 2;
                        band && (x / 9 + y / 3) % 5 < 2 && (40..width - 40).contains(&x)
                    })
                    .collect()
            })
            .collect(),
        resolution: Resolution::Standard,
    }
}

/// Seeded white noise, xorshift64* with Box-Muller.
struct Hiss {
    state: u64,
    sigma: f64,
}

impl Hiss {
    fn new(sigma: f64, seed: u64) -> Self {
        Self { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, sigma }
    }

    fn unit(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn next(&mut self) -> f64 {
        if self.sigma == 0.0 {
            return 0.0;
        }
        let u1 = self.unit().max(1e-18);
        let u2 = self.unit();
        self.sigma * (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// White noise of Es/N0 `snr_db` against a page carrier at the level every
/// transmitter here leaves at, a root mean square of 0.707, at 2400 baud.
fn sigma_at(snr_db: f64) -> f64 {
    ((FS / 2.0) / 2400.0 * 0.5 * 10f64.powf(-snr_db / 10.0)).sqrt()
}

/// How a call went.
struct Outcome {
    page: Option<Page>,
    /// The rate the caller last sent at, and every high-speed rate it
    /// trained at, in order.
    rate: u32,
    tried: Vec<(Modulation, u32)>,
}

/// A call from one `FaxCall` to another, hiss of `sigma` on both directions,
/// and the caller's first `spoiled` training checks drowned in noise at 6 dB
/// on their way.
fn call(page: &Page, sigma: f64, spoiled: usize, seed: u64) -> Outcome {
    let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
    let mut answerer = FaxCall::answer(FS, "61388880000");
    let mut up = Hiss::new(sigma, seed);
    let mut down = Hiss::new(sigma, seed ^ 0x5eed);
    let mut drown = Hiss::new(sigma_at(6.0), seed ^ 0xd0);
    let (mut to_caller, mut to_answerer) = (0.0, 0.0);
    let mut tried = Vec::new();
    let mut was_training = false;
    // `FAX_TRACE=1` prints every change of phase at both ends, as
    // `faxcall.rs`'s own calls do.
    let trace = std::env::var("FAX_TRACE").is_ok();
    let mut was = (caller.phase(), answerer.phase());
    for i in 0..(FS * 90.0) as usize {
        let a = caller.step(to_caller);
        let b = answerer.step(to_answerer);
        if trace && (caller.phase(), answerer.phase()) != was {
            was = (caller.phase(), answerer.phase());
            eprintln!("{:6.2}s caller {:<32} answerer {}", i as f64 / FS, was.0.name(), was.1.name());
        }
        let training = caller.phase() == Phase::Training;
        if training && !was_training {
            let speed = caller.speed();
            tried.push((speed.modulation, speed.bits_per_second));
        }
        was_training = training;
        let drowned = if training && tried.len() <= spoiled { drown.next() } else { 0.0 };
        to_answerer = a + up.next() + drowned;
        to_caller = b + down.next();
        if caller.phase().is_over() && answerer.phase().is_over() {
            break;
        }
    }
    Outcome { page: answerer.received().cloned(), rate: caller.rate(), tried }
}

/// A page at 9600 over a clean line, over hiss just past the level that used
/// to cost the top rate (fax-qam.md 3.1: 2.6e-3), over ten times that, and
/// over hiss at 25 dB of signal to noise.
#[test]
fn a_page_crosses_at_9600_over_a_line_with_hiss_on_it() {
    let page = a_page(40);
    let mut wrong = Vec::new();
    for (k, sigma) in [0.0, 3.0e-3, 3.0e-2, sigma_at(25.0)].into_iter().enumerate() {
        let outcome = call(&page, sigma, 0, 0x2900 + k as u64);
        let arrived = outcome.page.as_ref().is_some_and(|p| p.lines == page.lines);
        let state = if arrived { "intact" } else { "LOST" };
        println!("hiss {sigma:.1e}: page {state}, at {}, trained at {:?}", outcome.rate, outcome.tried);
        if !arrived || outcome.rate != 9600 {
            wrong.push((sigma, arrived, outcome.rate));
        }
    }
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// A training check at 9600 that arrives drowned: the call drops to 7200 and
/// the page crosses there.
#[test]
fn a_spoiled_check_at_9600_falls_back_to_7200() {
    let page = a_page(40);
    let outcome = call(&page, 1.0e-3, 1, 0x2972);
    let arrived = outcome.page.as_ref().is_some_and(|p| p.lines == page.lines);
    println!("page {}, at {}, trained at {:?}", if arrived { "intact" } else { "LOST" }, outcome.rate, outcome.tried);
    assert!(arrived, "the page did not arrive");
    assert_eq!(outcome.rate, 7200);
    assert_eq!(outcome.tried, [(Modulation::V29, 9600), (Modulation::V29, 7200)]);
}

/// Two drowned in a row: 9600, then 7200, then the page at 4800, which T.30
/// takes to V.27 ter.
#[test]
fn two_spoiled_checks_fall_back_through_7200_to_4800() {
    let page = a_page(40);
    let outcome = call(&page, 1.0e-3, 2, 0x2948);
    let arrived = outcome.page.as_ref().is_some_and(|p| p.lines == page.lines);
    println!("page {}, at {}, trained at {:?}", if arrived { "intact" } else { "LOST" }, outcome.rate, outcome.tried);
    assert!(arrived, "the page did not arrive");
    assert_eq!(outcome.rate, 4800);
    assert_eq!(
        outcome.tried,
        [(Modulation::V29, 9600), (Modulation::V29, 7200), (Modulation::V27ter, 4800)]
    );
}
