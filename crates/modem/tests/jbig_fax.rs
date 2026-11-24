//! JBIG in whole fax calls, through the modem crate.
//!
//! Two of our ends offer each other T.85's JBIG in their DIS (Table 2/T.30
//! bits 78 and 79), and under error correction mode -- the only way T.4 4.4
//! lets it go -- the page goes in it: the BIE in frames, each octet least
//! significant bit first, decoded as the frames come in. Either end turning it
//! off leaves MMR, and turning error correction off leaves Modified READ.

use fax::call::Phase;
use fax::coding::Coding;
use fax::page::{Page, Resolution};
use fax::t30::{Frame, Modulation};
use modem::FaxCall;

const FS: f64 = 16_000.0;

const WITH_V17: [Modulation; 3] = [Modulation::V27ter, Modulation::V29, Modulation::V17];

/// A page of type-like strokes, a rule, and a band of ordered-dither grey:
/// something JBIG's templates and its moving AT pixel both have work in.
fn a_page(lines: usize, resolution: Resolution) -> Page {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines)
            .map(|y| {
                (0..width)
                    .map(|x| {
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

/// What a call came to.
#[derive(Debug, Default)]
struct Outcome {
    page: Option<Page>,
    /// Every frame the answering end heard.
    heard: Vec<Frame>,
    /// The answering end's line count each time it changed while the page
    /// was still arriving.
    seen: Vec<usize>,
}

/// Two fax calls against each other, with `noise` applied to what reaches the
/// answering end, sample by sample, given the caller's phase and how far
/// through the page it is.
fn call(
    caller: &mut FaxCall,
    answerer: &mut FaxCall,
    seconds: f64,
    noise: &mut dyn FnMut(f64, Phase, Option<f64>) -> f64,
) -> Outcome {
    let (mut to_caller, mut to_answerer) = (0.0, 0.0);
    let mut outcome = Outcome::default();
    for _ in 0..(seconds * FS) as usize {
        let a = caller.step(to_caller);
        let b = answerer.step(to_answerer);
        to_caller = b;
        to_answerer = noise(a, caller.phase(), caller.progress());
        outcome.heard.extend(answerer.take_heard().into_iter().map(|m| m.frame));
        if answerer.phase() == Phase::Receiving && outcome.page.is_none() {
            let lines = answerer.lines().len();
            if outcome.seen.last() != Some(&lines) {
                outcome.seen.push(lines);
            }
        }
        if outcome.page.is_none() {
            outcome.page = answerer.take_received().map(|(_, page)| page);
        }
        if caller.phase().is_over() && answerer.phase().is_over() {
            break;
        }
    }
    outcome
}

fn clean(sample: f64, _: Phase, _: Option<f64>) -> f64 {
    sample
}

#[test]
fn two_of_these_send_a_page_in_jbig_at_either_resolution() {
    for resolution in [Resolution::Standard, Resolution::Fine] {
        let page = a_page(300, resolution);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone())).offering(&WITH_V17);
        let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17);
        let got = call(&mut caller, &mut answerer, 60.0, &mut clean);
        assert_eq!(caller.phase(), Phase::Done, "{resolution:?}: the caller ended at {} ({:?})", caller.phase().name(), caller.trouble());
        assert_eq!(answerer.phase(), Phase::Done, "{resolution:?}: the answerer ended at {} ({:?})", answerer.phase().name(), answerer.trouble());
        assert!(caller.error_correction() && answerer.error_correction());
        assert_eq!(caller.coding(), Coding::Jbig, "{resolution:?}: sent in {:?}", caller.coding());
        assert_eq!(answerer.coding(), Coding::Jbig, "{resolution:?}: read as {:?}", answerer.coding());
        let arrived = got.page.expect("no page arrived");
        assert_eq!(arrived.resolution, resolution);
        assert!(arrived.lines == page.lines, "{resolution:?}: the page came out different");
        // Drawn as it came, a frame's worth at a time, not all at the end.
        let partway = got.seen.iter().filter(|&&n| n > 0 && n < page.lines.len()).count();
        assert!(partway >= 3, "{resolution:?}: seen at {partway} heights on the way ({:?})", got.seen);
    }
}

#[test]
fn jbig_goes_only_where_both_ends_offer_it_and_error_correction_is_on() {
    // Off at either end, the page goes in MMR as it did before JBIG; with no
    // error correction, in Modified READ, since T.4 4.3 and 4.4 allow neither
    // of the others without it.
    for (at_caller, at_answerer, error_correction, want) in [
        (false, true, true, Coding::Mmr),
        (true, false, true, Coding::Mmr),
        (true, true, false, Coding::ModifiedRead),
    ] {
        let page = a_page(40, Resolution::Standard);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone())).with_jbig(at_caller);
        let mut answerer = FaxCall::answer(FS, "61388880000")
            .with_jbig(at_answerer)
            .with_error_correction(error_correction);
        let got = call(&mut caller, &mut answerer, 60.0, &mut clean);
        let case = format!("caller {at_caller}, answerer {at_answerer}, error correction {error_correction}");
        assert_eq!(caller.coding(), want, "{case}");
        assert_eq!(answerer.coding(), want, "{case}");
        assert!(got.page.expect("no page arrived").lines == page.lines, "{case}");
    }
}

#[test]
fn a_burst_of_noise_costs_jbig_frames_and_not_the_page() {
    // One wrong bit would put every pel after it wrong, which is why T.85
    // makes error-free transmission mandatory: the frames the burst lands on
    // are asked for and sent again, and the page arrives exactly.
    let page = a_page(600, Resolution::Standard);
    let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
    let mut answerer = FaxCall::answer(FS, "61388880000");
    let mut seed = 0x1234_5678u32;
    let mut started: Option<usize> = None;
    let mut sample = 0usize;
    let mut burst = |a: f64, phase: Phase, progress: Option<f64>| {
        sample += 1;
        if started.is_none() && phase == Phase::Sending && progress.is_some_and(|p| p > 0.3) {
            started = Some(sample);
        }
        // A tenth of a second, the first time through only.
        if started.is_some_and(|s| sample - s < (FS * 0.1) as usize) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            a + (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 2.0
        } else {
            a
        }
    };
    let got = call(&mut caller, &mut answerer, 90.0, &mut burst);
    assert_eq!(caller.coding(), Coding::Jbig);
    let pps = got.heard.iter().filter(|f| **f == Frame::Pps).count();
    assert!(pps >= 2, "the damaged frames were never sent again: {:?}", got.heard);
    let arrived = got.page.unwrap_or_else(|| panic!("no page ({:?})", answerer.trouble()));
    assert!(arrived.lines == page.lines, "the page was not put right");
}
