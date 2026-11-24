//! Super G3 from end to end: two of our fax calls, both offering V.34, over
//! a line with delay, loss, noise, echo and a VoIP jitter buffer's slips.
//!
//! Everything a call has in it -- T.30's tones and V.8 (clause 6), V.34's
//! phase 2 and phase 3, the control channel start-up, phase B's frames on the
//! control channel, the turn to the primary channel, the page at up to 33 600
//! bit/s in T.4 Annex A's frames, the way back for the receipt, and the
//! disconnect (T.30 Annex F on V.34 clause 12) -- driven a sample at a time
//! through `modem::FaxCall`, which is what the window drives. The ends are
//! ours at both sides: no Super G3 machine has been on the bench yet, and
//! `docs/design/superg3/wp-h3.md` says what to ask for when one is.

use std::collections::VecDeque;

use fax::call::Phase;
use fax::coding::Coding;
use fax::page::{Page, Resolution};
use fax::t30::{Frame, Modulation};
use modem::FaxCall;

const FS: f64 = 16_000.0;

const WITH_V17: [Modulation; 3] = [Modulation::V27ter, Modulation::V29, Modulation::V17];

/// A page of type-like strokes, a rule, and a band of ordered-dither grey,
/// shifted by `seed` so that one page is not another.
fn a_page(lines: usize, resolution: Resolution, seed: usize) -> Page {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        let x = x + 37 * seed;
                        if y % 50 == 25 {
                            return (100..1600).contains(&x);
                        }
                        if y >= lines / 2 {
                            return BAYER[y % 4][x % 4] < (x * 16 / width) as u8;
                        }
                        let within = y % 12;
                        within < 9 && ((x + y / 12 * 7) % 23 < 3 || (within == 4 && x % 23 < 15))
                    })
                    .collect()
            })
            .collect(),
        resolution,
    }
}

/// How the line is.
#[derive(Debug, Clone, Copy)]
struct Line {
    /// Seconds each way.
    one_way: f64,
    /// Signal to noise at each receiver, in decibels; infinite for none.
    snr_db: f64,
    /// Each end's own signal back into its receiver, in decibels down;
    /// infinite for none.
    echo_db: f64,
    /// A jitter buffer's 20 ms slip every this many seconds, alternately
    /// towards one end and the other, twice dropped and then twice made up;
    /// nought for none.
    slip_every: f64,
    /// A slip in the middle of each page's first burst as well, towards the
    /// answering end: dropped in odd pages, made up in even ones. Where the
    /// slips of `slip_every` fall is wherever the call happens to be, and a
    /// page going one way leaves half of them landing on silence.
    slip_in_pages: bool,
}

impl Line {
    /// A short, clean line.
    fn clean() -> Self {
        Self { one_way: 0.030, snr_db: f64::INFINITY, echo_db: f64::INFINITY, slip_every: 0.0, slip_in_pages: false }
    }

    fn noisy(snr_db: f64) -> Self {
        Self { snr_db, ..Self::clean() }
    }

    /// This rig's VoIP line: three quarters of a second each way, this end's
    /// own signal 10 dB down in its own receiver, and a good deal of noise.
    fn voip() -> Self {
        Self { one_way: 0.750, snr_db: 40.0, echo_db: 10.0, ..Self::clean() }
    }
}

/// The two directions of the line, with everything `Line` says done to them.
struct Wire {
    line: Line,
    to_answerer: VecDeque<f64>,
    to_caller: VecDeque<f64>,
    loss: f64,
    noise: f64,
    echo: f64,
    seed: u32,
    last_from_caller: f64,
    last_from_answerer: f64,
    /// Samples so far, `slip_every`'s slips due so far, and slips made.
    n: usize,
    periodic: usize,
    slips: usize,
}

impl Wire {
    fn new(line: Line) -> Self {
        let delay = ((line.one_way * FS) as usize).max(1);
        // Fifteen decibels of loss, as the modem's own tests have it; the
        // noise is set against the signal as it arrives.
        let loss = 10f64.powf(-15.0 / 20.0);
        Self {
            line,
            to_answerer: std::iter::repeat_n(0.0, delay).collect(),
            to_caller: std::iter::repeat_n(0.0, delay).collect(),
            loss,
            noise: if line.snr_db.is_finite() { 10f64.powf(-line.snr_db / 20.0) * 0.707 * loss } else { 0.0 },
            echo: if line.echo_db.is_finite() { 10f64.powf(-line.echo_db / 20.0) } else { 0.0 },
            seed: 0x2545_f491,
            last_from_caller: 0.0,
            last_from_answerer: 0.0,
            n: 0,
            periodic: 0,
            slips: 0,
        }
    }

    /// Uniform noise of unit variance.
    fn rand(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (f64::from(self.seed) / f64::from(u32::MAX) - 0.5) * 3.464
    }

    /// What each end hears this sample.
    fn hear(&mut self) -> (f64, f64) {
        let by_caller = self.to_caller.pop_front().unwrap_or(0.0) * self.loss
            + self.echo * self.last_from_caller
            + self.noise * self.rand();
        let by_answerer = self.to_answerer.pop_front().unwrap_or(0.0) * self.loss
            + self.echo * self.last_from_answerer
            + self.noise * self.rand();
        (by_caller, by_answerer)
    }

    /// What each end said this sample.
    fn say(&mut self, from_caller: f64, from_answerer: f64) {
        self.last_from_caller = from_caller;
        self.last_from_answerer = from_answerer;
        self.to_answerer.push_back(from_caller);
        self.to_caller.push_back(from_answerer);
        self.n += 1;
        if self.line.slip_every > 0.0 && self.n.is_multiple_of((self.line.slip_every * FS) as usize) {
            self.periodic += 1;
            let k = self.periodic;
            self.slip_towards(k % 2 == 1, k % 4 == 1 || k % 4 == 2);
        }
    }

    /// A VoIP jitter buffer's slip in one direction: the next twenty
    /// milliseconds never arrive (`drop`), or arrive twice. Counted only if
    /// made: a buffer cannot drop more than it holds.
    fn slip_towards(&mut self, answerer: bool, drop: bool) -> bool {
        let n = (0.020 * FS) as usize;
        let queue = if answerer { &mut self.to_answerer } else { &mut self.to_caller };
        if queue.len() <= n {
            return false;
        }
        if drop {
            queue.drain(..n);
        } else {
            let again: Vec<f64> = queue.iter().take(n).copied().collect();
            for x in again.into_iter().rev() {
                queue.push_front(x);
            }
        }
        self.slips += 1;
        true
    }
}

/// What a call came to.
#[derive(Default)]
struct Outcome {
    /// The pages the answering end received, with their numbers in the call.
    pages: Vec<(usize, Page)>,
    /// Every frame each end heard.
    heard_by_answerer: Vec<Frame>,
    heard_by_caller: Vec<Frame>,
    /// The primary rate each page went at, as the caller had it while
    /// sending.
    rates: Vec<u32>,
    /// Each change of what the answering end's scope was told it was
    /// drawing, and how many points it was given while the page arrived.
    shapes: Vec<&'static str>,
    points: usize,
    /// The names the answering end gave its phase, each change.
    phases: Vec<&'static str>,
    /// When each end was over, in seconds of line.
    caller_over: Option<f64>,
    answerer_over: Option<f64>,
    /// Slips the wire made, and when and in what the calling end was for
    /// each; how many of them were `slip_in_pages`'s.
    slips: usize,
    slipped: Vec<(f64, &'static str)>,
    page_slips: usize,
}

impl Outcome {
    fn seconds(&self) -> f64 {
        self.caller_over.unwrap_or(f64::NAN).max(self.answerer_over.unwrap_or(f64::NAN))
    }
}

/// By hand, so that a failure prints the pages' numbers and sizes rather
/// than their megabytes of pels.
impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pages: Vec<(usize, usize)> = self.pages.iter().map(|(n, p)| (*n, p.lines.len())).collect();
        f.debug_struct("Outcome")
            .field("pages (number, lines)", &pages)
            .field("heard_by_answerer", &self.heard_by_answerer)
            .field("heard_by_caller", &self.heard_by_caller)
            .field("rates", &self.rates)
            .field("shapes", &self.shapes)
            .field("points", &self.points)
            .field("phases", &self.phases)
            .field("caller_over", &self.caller_over)
            .field("answerer_over", &self.answerer_over)
            .field("slips", &self.slips)
            .field("slipped", &self.slipped)
            .field("page_slips", &self.page_slips)
            .finish()
    }
}

/// Run two ends against each other over `line` until both are over or
/// `seconds` have passed, with `during` given both ends and the time every
/// sample for whatever a test wants done to them on the way.
fn run(
    caller: &mut FaxCall,
    answerer: &mut FaxCall,
    line: Line,
    seconds: f64,
    during: &mut dyn FnMut(&mut FaxCall, &mut FaxCall, f64),
) -> Outcome {
    let trace = std::env::var("FAX_TRACE").is_ok();
    let mut wire = Wire::new(line);
    let mut outcome = Outcome::default();
    let (mut was_a, mut was_b) = ("", "");
    let mut sheet_sent = 0;
    let mut sheet_slipped = 0;
    for i in 0..(seconds * FS) as usize {
        let t = i as f64 / FS;
        during(caller, answerer, t);
        let (by_caller, by_answerer) = wire.hear();
        let a = caller.step(by_caller);
        let b = answerer.step(by_answerer);
        let slips = wire.slips;
        wire.say(a, b);
        // Half way through the first burst of each page, where there is page
        // on the line to spoil.
        if line.slip_in_pages
            && caller.phase() == Phase::Sending
            && caller.sheet() > sheet_slipped
            && caller.progress().is_some_and(|p| p >= 0.5)
        {
            sheet_slipped = caller.sheet();
            if wire.slip_towards(true, sheet_slipped % 2 == 1) {
                outcome.page_slips += 1;
            }
        }
        if wire.slips > slips {
            outcome.slipped.push((t, caller.phase_name()));
        }

        outcome.heard_by_answerer.extend(answerer.take_heard().into_iter().map(|m| m.frame));
        outcome.heard_by_caller.extend(caller.take_heard().into_iter().map(|m| m.frame));
        outcome.pages.extend(answerer.take_received());
        if caller.phase() == Phase::Sending
            && caller.sheet() != sheet_sent
            && let Some(rate) = caller.primary_rate()
        {
            sheet_sent = caller.sheet();
            outcome.rates.push(rate);
        }
        let shape = answerer.shape();
        if outcome.shapes.last() != Some(&shape) {
            outcome.shapes.push(shape);
        }
        if answerer.phase() == Phase::Receiving && answerer.constellation_point().is_some() {
            outcome.points += 1;
        }
        let phase = answerer.phase_name();
        if outcome.phases.last() != Some(&phase) {
            outcome.phases.push(phase);
        }
        if trace && (caller.phase_name() != was_a || answerer.phase_name() != was_b) {
            eprintln!("{t:7.2}s caller {:<40} answerer {}", caller.phase_name(), answerer.phase_name());
            was_a = caller.phase_name();
            was_b = answerer.phase_name();
        }
        if outcome.caller_over.is_none() && caller.phase().is_over() {
            outcome.caller_over = Some(t);
        }
        if outcome.answerer_over.is_none() && answerer.phase().is_over() {
            outcome.answerer_over = Some(t);
        }
        if caller.phase().is_over() && answerer.phase().is_over() {
            break;
        }
    }
    outcome.slips = wire.slips;
    outcome
}

fn nothing(_: &mut FaxCall, _: &mut FaxCall, _: f64) {}

/// Two ends, both offering V.34 and every modulation, the caller with
/// `pages` to send.
fn pair(pages: Vec<Page>) -> (FaxCall, FaxCall) {
    let caller = FaxCall::originate_pages(FS, "61399990000", pages).offering(&WITH_V17).with_v34(true);
    let answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17).with_v34(true);
    (caller, answerer)
}

/// Both ends done, every page whole and in order, and the call V.34's.
fn check(caller: &FaxCall, answerer: &FaxCall, sent: &[Page], got: &Outcome, case: &str) {
    assert_eq!(caller.phase(), Phase::Done, "{case}: the caller ended at {} ({:?}); {got:?}", caller.phase_name(), caller.trouble());
    assert_eq!(answerer.phase(), Phase::Done, "{case}: the answerer ended at {} ({:?}); {got:?}", answerer.phase_name(), answerer.trouble());
    assert!(caller.v34_agreed() && answerer.v34_agreed(), "{case}: not a V.34 call");
    assert_eq!((caller.standard(), answerer.standard()), ("V.34", "V.34"), "{case}");
    assert!(caller.error_correction() && answerer.error_correction(), "{case}: F.3 makes error correction mandatory");
    let numbers: Vec<usize> = got.pages.iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, (1..=sent.len()).collect::<Vec<_>>(), "{case}: pages {numbers:?}; heard {:?}", got.heard_by_answerer);
    for ((n, page), want) in got.pages.iter().zip(sent) {
        assert_eq!(page.resolution, want.resolution, "{case}: page {n} arrived at the wrong resolution");
        assert!(page.lines == want.lines, "{case}: page {n} came out different");
    }
    assert_eq!(answerer.pages_received(), sent.len(), "{case}");
    assert_eq!(got.rates.len(), sent.len(), "{case}: a rate for each page: {:?}", got.rates);
    assert!(got.heard_by_answerer.contains(&Frame::Pps), "{case}: no partial page signal: {:?}", got.heard_by_answerer);
    assert!(!got.heard_by_answerer.contains(&Frame::Eop), "{case}: a bare EOP under error correction");
    // No training check and no FTT (F.3.2.1): the DCS is answered with CFR.
    assert!(!got.heard_by_caller.contains(&Frame::Ftt), "{case}: an FTT in an Annex F call");
    assert!(got.heard_by_caller.contains(&Frame::Cfr), "{case}: no CFR: {:?}", got.heard_by_caller);
}

#[test]
fn one_page_crosses_at_33_600_on_a_clean_line() {
    let pages = vec![a_page(300, Resolution::Standard, 0)];
    let (mut caller, mut answerer) = pair(pages.clone());
    let got = run(&mut caller, &mut answerer, Line::clean(), 60.0, &mut nothing);
    check(&caller, &answerer, &pages, &got, "one page");
    assert_eq!(got.rates, [33_600], "{got:?}");
    assert_eq!(caller.symbol_rate(), Some(3429));
    assert_eq!(answerer.symbol_rate(), Some(3429));
    assert_eq!(answerer.primary_rate(), Some(33_600));
    assert_eq!(caller.control_rates(), Some((1200, 1200)));
    assert_eq!(caller.coding(), Coding::Jbig);
    assert_eq!(answerer.coding(), Coding::Jbig);
    assert_eq!(caller.v34_retrains(), (0, 0), "retrains or phase 3 again on a clean line");
    let snr = answerer.v34_snr_db().expect("the page trained to nothing");
    assert!(snr > 35.0, "trained to {snr:.1} dB on a clean line");
    // The scope had both channels: the control channel's four points and the
    // page's 1408, with a page's worth of points to draw.
    assert!(got.shapes.contains(&"4PSK") && got.shapes.contains(&"1408TCM"), "{:?}", got.shapes);
    assert!(got.points > 1000, "{} points while the page arrived", got.points);
    // And named every step: V.8, phase 2, phase 3, the start-up, T.30.
    for wanted in ["V.8: sending ANSam", "V.34 INFO0", "V.34 phase 3", "V.34 control start-up", "receiving the page"] {
        assert!(got.phases.contains(&wanted), "{wanted:?} not among {:?}", got.phases);
    }
    // A page of this size at 33 600 is a second; the call is V.8, phase 2,
    // phase 3 and phase B either side of it.
    assert!(got.seconds() < 15.0, "the call took {:.1} s", got.seconds());
    eprintln!("one page: {:.1} s of line, page trained to {snr:.1} dB", got.seconds());
}

#[test]
fn three_fine_pages_cross_in_mmr() {
    // JBIG off at the answering end, so the pages go in MMR (T.6), each a
    // different one at fine resolution.
    let pages: Vec<Page> = (0..3).map(|n| a_page(80 + 20 * n, Resolution::Fine, n + 1)).collect();
    let (mut caller, answerer) = pair(pages.clone());
    let mut answerer = answerer.with_jbig(false);
    let got = run(&mut caller, &mut answerer, Line::clean(), 90.0, &mut nothing);
    check(&caller, &answerer, &pages, &got, "three fine pages");
    assert_eq!(got.rates, [33_600; 3], "{got:?}");
    assert_eq!(caller.coding(), Coding::Mmr);
    assert_eq!(answerer.coding(), Coding::Mmr);
    assert_eq!((caller.sheet(), caller.sheets()), (3, 3));
    let pps = got.heard_by_answerer.iter().filter(|f| **f == Frame::Pps).count();
    assert!(pps >= 3, "{:?}", got.heard_by_answerer);
    eprintln!("three fine pages in MMR: {:.1} s of line", got.seconds());
}

#[test]
fn a_page_crosses_a_noisy_line_at_the_rate_the_line_allows() {
    // Twenty-eight decibels of signal to noise at each receiver: the MPh
    // exchange settles under 33 600, by what the page's training measured,
    // and the page arrives whole under error correction.
    let pages = vec![a_page(200, Resolution::Standard, 4)];
    let (mut caller, mut answerer) = pair(pages.clone());
    let got = run(&mut caller, &mut answerer, Line::noisy(28.0), 90.0, &mut nothing);
    check(&caller, &answerer, &pages, &got, "28 dB");
    let rate = got.rates[0];
    assert!((9600..33_600).contains(&rate), "{rate} bit/s at 28 dB; {got:?}");
    assert_eq!(answerer.primary_rate(), Some(rate));
    eprintln!("28 dB: {rate} bit/s on {:?} baud, trained to {:.1} dB, {:.1} s of line", caller.symbol_rate(), answerer.v34_snr_db().unwrap_or(f64::NAN), got.seconds());
}

#[test]
fn a_page_crosses_this_rigs_voip_line() {
    // Three quarters of a second each way, this end's own signal 10 dB down
    // in its receiver: every wait of V.8, phase 2, clause 12 and T.30 holds
    // through a round trip of a second and a half, and the page arrives.
    let pages = vec![a_page(200, Resolution::Standard, 5)];
    let (mut caller, mut answerer) = pair(pages.clone());
    let got = run(&mut caller, &mut answerer, Line::voip(), 90.0, &mut nothing);
    check(&caller, &answerer, &pages, &got, "VoIP line");
    assert!(got.rates[0] >= 28_800, "{got:?}");
    assert_eq!(caller.v34_retrains().0, 0, "a control channel retrain over the delay");
    eprintln!("VoIP line: {} bit/s, {:.1} s of line", got.rates[0], got.seconds());
}

#[test]
fn two_pages_cross_a_line_that_slips_twenty_milliseconds_every_two_seconds() {
    // A VoIP jitter buffer's 20 ms, dropped or made up, every two seconds
    // from the first, alternately towards each end -- on V.8's menus, phase
    // 2, the control channel -- and once more half way through each page,
    // towards the answering end: dropped from the first, made up in the
    // second. Whatever a slip lands on is followed, or sent again by the
    // procedure that owns it: a page's spoilt frames are asked for again
    // (T.4 Annex A's PPR), and both pages arrive whole. A tenth of a second
    // each way, so that the jitter buffer has the 20 ms to drop.
    let pages = vec![a_page(200, Resolution::Standard, 6), a_page(120, Resolution::Standard, 7)];
    let (mut caller, mut answerer) = pair(pages.clone());
    let line = Line { one_way: 0.100, slip_every: 2.0, slip_in_pages: true, ..Line::clean() };
    let got = run(&mut caller, &mut answerer, line, 120.0, &mut nothing);
    assert_eq!(got.page_slips, 2, "{got:?}");
    assert!(got.slips >= 6, "only {} slips: {got:?}", got.slips);
    check(&caller, &answerer, &pages, &got, "slips");
    assert!(got.heard_by_caller.contains(&Frame::Ppr), "the slips in the pages spoilt nothing: {got:?}");
    eprintln!(
        "slips: {} of them, {:.1} s of line, frames heard {:?} and {:?}, slipped {:?}",
        got.slips,
        got.seconds(),
        got.heard_by_answerer,
        got.heard_by_caller,
        got.slipped
    );
}

#[test]
fn a_rate_change_is_made_at_the_start_of_the_control_channel_after_a_page() {
    // F.3.4.1 and F.3.4.2/T.30: a data rate change is a control channel
    // start-up in place of the resynchronisation. Asked for at the source
    // while the first page goes, it sends PPh after the page (12.6.1.1/V.34)
    // and the MPh exchange settles the new rate; asked for at the recipient,
    // it answers the source's Sh with PPh instead (12.6.2.3, Figure 26). The
    // second page goes at the new rate either way.
    for (at_source, cap, want) in [(true, 24_000, 24_000), (false, 21_600, 21_600)] {
        let pages = vec![a_page(120, Resolution::Standard, 8), a_page(120, Resolution::Standard, 9)];
        let (mut caller, mut answerer) = pair(pages.clone());
        let mut asked = false;
        let mut during = |caller: &mut FaxCall, answerer: &mut FaxCall, _t: f64| {
            if !asked && caller.phase() == Phase::Sending && caller.sheet() == 1 {
                asked = true;
                if at_source {
                    caller.limit_v34_rate(cap);
                } else {
                    answerer.limit_v34_rate(cap);
                }
            }
        };
        let got = run(&mut caller, &mut answerer, Line::clean(), 90.0, &mut during);
        let case = format!("cap {cap} at the {}", if at_source { "source" } else { "recipient" });
        check(&caller, &answerer, &pages, &got, &case);
        assert_eq!(got.rates, [33_600, want], "{case}: {got:?}");
        assert_eq!(answerer.primary_rate(), Some(want), "{case}");
        assert_eq!(caller.v34_retrains(), (0, 0), "{case}: a retrain rather than a start-up");
        eprintln!("{case}: rates {:?}, {:.1} s of line", got.rates, got.seconds());
    }
}

#[test]
fn a_call_with_no_page_says_goodbye_over_the_control_channel() {
    // The caller has nothing to send, and does what `fax::call` does on any
    // call without a page: identifies itself and commands, so as to learn
    // what the far end is, and hangs up when the CFR lets it go. Every frame
    // of that is on the control channel, and the primary channel is never
    // turned to -- the answerer is still waiting for the forty ones when the
    // DCN arrives.
    let (mut caller, mut answerer) = pair(Vec::new());
    let got = run(&mut caller, &mut answerer, Line::clean(), 60.0, &mut nothing);
    assert_eq!(caller.phase(), Phase::Done, "{:?}", caller.trouble());
    assert_eq!(answerer.phase(), Phase::Done, "{:?}", answerer.trouble());
    assert!(caller.v34_agreed() && answerer.v34_agreed());
    assert!(got.pages.is_empty());
    assert_eq!(got.heard_by_answerer, [Frame::Tsi, Frame::Dcs, Frame::Dcn], "{got:?}");
    assert!(got.heard_by_caller.ends_with(&[Frame::Dis, Frame::Cfr]), "{got:?}");
    assert!(got.rates.is_empty() && got.points == 0, "the primary channel was used: {got:?}");
    assert!(!got.shapes.iter().any(|s| s.ends_with("TCM")), "{got:?}");
    assert!(got.seconds() < 15.0, "the call took {:.1} s", got.seconds());
}

#[test]
fn a_far_end_that_vanishes_after_the_page_fails_the_call_with_the_modems_reason() {
    // The answering end goes off the line as the page goes. The caller's
    // modem sends its page, then Sh and S-bar-h that nothing answers, waits
    // its three seconds (12.6.1.5/V.34), retrains on AC three times over
    // (12.8.1, plan 8.4), and gives up; the call is failed with that reason,
    // rather than after T.30's own timers.
    let pages = vec![a_page(60, Resolution::Standard, 10)];
    let (mut caller, mut answerer) = pair(pages);
    let mut gone_at = None;
    let mut wire = Wire::new(Line::clean());
    for i in 0..(40.0 * FS) as usize {
        let t = i as f64 / FS;
        let (by_caller, by_answerer) = wire.hear();
        let a = caller.step(by_caller);
        let b = if gone_at.is_some() { 0.0 } else { answerer.step(by_answerer) };
        wire.say(a, b);
        if gone_at.is_none() && caller.phase() == Phase::Sending {
            gone_at = Some(t);
        }
        if caller.phase().is_over() {
            let gone = gone_at.expect("over before the page");
            assert_eq!(caller.phase(), Phase::Failed);
            let trouble = caller.trouble().expect("no reason given");
            assert!(trouble.starts_with("V.34:") && trouble.contains("AC"), "{trouble:?}");
            // Three seconds after S-bar-h, three rounds of three of AC.
            assert!((11.0..16.0).contains(&(t - gone)), "failed {:.1} s after the far end went", t - gone);
            eprintln!("the far end gone: failed {:.1} s later, {trouble:?}", t - gone);
            return;
        }
    }
    panic!("the caller never gave up: {} ({:?})", caller.phase_name(), caller.trouble());
}

#[test]
fn a_recipient_whose_page_never_comes_brings_the_channel_back_for_its_dcn_or_fails() {
    // The calling end goes off the line once the recipient has fallen silent
    // for the page. T2 runs out on the recipient with the channel down; its
    // DCN needs a control channel, so the join asks the modem for 12.8's
    // retrain, which nothing answers, and the call fails with the modem's
    // reason inside a few seconds of AC rather than a minute of T.30's.
    let pages = vec![a_page(60, Resolution::Standard, 11)];
    let (mut caller, mut answerer) = pair(pages);
    let mut gone_at = None;
    let mut wire = Wire::new(Line::clean());
    for i in 0..(60.0 * FS) as usize {
        let t = i as f64 / FS;
        let (by_caller, by_answerer) = wire.hear();
        let a = if gone_at.is_some() { 0.0 } else { caller.step(by_caller) };
        let b = answerer.step(by_answerer);
        wire.say(a, b);
        if gone_at.is_none() && answerer.phase() == Phase::Receiving {
            gone_at = Some(t);
        }
        if answerer.phase().is_over() {
            let gone = gone_at.expect("over before the page was awaited");
            assert_eq!(answerer.phase(), Phase::Failed);
            let trouble = answerer.trouble().expect("no reason given");
            assert!(trouble.starts_with("V.34:") && trouble.contains("AC"), "{trouble:?}");
            // T2's six seconds, then three rounds of three seconds of AC.
            assert!((14.0..20.0).contains(&(t - gone)), "failed {:.1} s after the far end went", t - gone);
            eprintln!("the source gone: the recipient failed {:.1} s later, {trouble:?}", t - gone);
            return;
        }
    }
    panic!("the recipient never gave up: {} ({:?})", answerer.phase_name(), answerer.trouble());
}

/// A V.34 end against a plain one, either way round: the plain answerer's
/// called tone drops the caller's V.8, and the plain caller hears the V.34
/// answerer's ANSam as the called tone and waits for its DIS; both calls are
/// clause 5's, at V.17's 14 400.
#[test]
fn a_v34_end_and_a_plain_end_complete_on_clause_5_either_way_round() {
    for (caller_v34, answerer_v34) in [(true, false), (false, true)] {
        let page = a_page(60, Resolution::Standard, 12);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone())).offering(&WITH_V17).with_v34(caller_v34);
        let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17).with_v34(answerer_v34);
        let got = run(&mut caller, &mut answerer, Line::clean(), 60.0, &mut nothing);
        let case = format!("caller V.34 {caller_v34}, answerer V.34 {answerer_v34}");
        assert_eq!(caller.phase(), Phase::Done, "{case}: {:?}", caller.trouble());
        assert_eq!(answerer.phase(), Phase::Done, "{case}: {:?}", answerer.trouble());
        assert!(!caller.v34_agreed() && !answerer.v34_agreed(), "{case}: V.34 agreed with a plain end");
        assert_eq!(caller.standard(), "V.21", "{case}");
        assert_eq!(got.pages.len(), 1, "{case}");
        assert!(got.pages[0].1.lines == page.lines, "{case}: the page came out different");
        assert_eq!(caller.speed().bits_per_second, 14_400, "{case}: {:?}", caller.speed());
        assert_eq!(caller.symbol_rate(), None, "{case}");
    }
}

/// The recorded Super G3 machine's call menu, answered with V.34 on: the
/// joint menu offers V.34 half-duplex, and after CJ the answerer sends
/// INFO0a -- V.34's phase 2, not a DIS on V.21 (6.1.5/T.30).
#[test]
fn the_recorded_call_menu_is_answered_with_v34_and_info0a_follows_cj() {
    use datapump::bell103::Bell103Tx;
    use datapump::framing::AsyncBits;
    use datapump::v34::dpsk::{Receiver, Side};
    use datapump::v34::info::Info;
    use datapump::v8::LOW;

    let vector = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/fax-v34-cm.wav");
    let wav = line::wav::read(vector).expect("could not read the vector");
    let fs = f64::from(wav.sample_rate);
    let mut call = FaxCall::answer(fs, "61399990000").offering(&WITH_V17).with_v34(true);

    let mut jm_ear = datapump::bell103::Bell103Rx::with_tones(datapump::v8::HIGH.0, datapump::v8::HIGH.1, fs);
    let mut jm_decoder = v8::Decoder::new();
    let mut joint = None;
    let mut frames_ear = datapump::v21::Receiver::new(fs);
    let mut reader = fax::frames::Reader::new();
    let mut frames = Vec::new();
    let mut info_ear = Receiver::half_duplex(Side::Answer, fs);
    let mut info0 = None;
    let mut info0_at = None;
    // Read between uses of the closure, so shared rather than borrowed by it.
    let t = std::cell::Cell::new(0.0f64);
    let mut hear = |call: &mut FaxCall, input: f64| {
        let out = call.step(input);
        t.set(t.get() + 1.0 / fs);
        // The decoder names every menu a CM: which one it is, is which
        // channel it came on (`v8::Decoder::heard_as_jm`), and this is the
        // answering end's.
        if let Some(octet) = jm_ear.feed(out)
            && let Some(v8::Heard::Cm(m) | v8::Heard::Jm(m)) = jm_decoder.feed(octet)
        {
            joint.get_or_insert(m);
        }
        if let Some(bit) = frames_ear.feed(out)
            && let Some(m) = reader.feed(bit)
        {
            frames.push(m.frame);
        }
        if call.v34_agreed()
            && let Some(Info::Info0(info)) = info_ear.feed(out)
            && info0.is_none()
        {
            info0 = Some(info);
            info0_at = Some(t.get());
        }
    };

    for &s in &wav.channel(0) {
        hear(&mut call, f64::from(s));
    }
    assert!(!call.v34_agreed(), "in Annex F before CJ");
    let framing = AsyncBits::new(8);
    let mut cj = Bell103Tx::with_tones(LOW.0, LOW.1, fs);
    cj.set_transmitting(true);
    for octet in v8::CJ {
        cj.push_bits(&framing.encode(octet));
    }
    while cj.pending_bits() > 0 {
        hear(&mut call, cj.next_sample());
    }
    let cj_over = t.get();
    for _ in 0..(fs * 3.0) as usize {
        hear(&mut call, 0.0);
    }

    let joint = joint.expect("no joint menu went out");
    assert!(joint.modulations.contains(v8::Modulation::V34HalfDuplex), "{joint:?}");
    assert!(call.v34_agreed(), "not in Annex F after CJ: {}", call.phase_name());
    assert!(frames.is_empty(), "frames on V.21 in an Annex F call: {frames:?}");
    let info0 = info0.expect("no INFO0a after CJ");
    let at = info0_at.unwrap() - cj_over;
    // 75 ms after CJ (8.2.3/V.8), and the sequence itself is 49 bits at 600
    // bit/s plus the receiver's own delay.
    assert!((0.1..0.6).contains(&at), "INFO0a read {at:.3} s after CJ");
    assert!(info0.rate_3429 && info0.constellation_1664, "{info0:?}");
    assert!(call.phase_name().starts_with("V.34"), "{}", call.phase_name());
    eprintln!("INFO0a read {at:.3} s after CJ: {info0:?}");
}
