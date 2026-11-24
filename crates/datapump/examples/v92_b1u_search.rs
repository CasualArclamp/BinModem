//! Enumerate the upstream bit layer's conventions against a known sequence.
//!
//! 8.7.1 makes B1u "48 data frames of scrambled binary ones" before any data,
//! so once the levels have been decided there is exactly one stretch of the
//! upstream whose contents are known without the far end's help. That is the
//! only judge of the bit layer that does not go through our own encoder, and
//! the whole point of this program: a decoder that shares a convention error
//! with its encoder still round-trips, so the self-consistent tests in
//! `v92::modulus` cannot see one.
//!
//! Everything the bit layer does sits downstream of the twelve indices the
//! equaliser and the Viterbi decided, so a capture only has to be decided once:
//! `V92_ETA_TRACE` writes those indices out, and this program reads them back
//! and tries every combination of
//!
//!   * d(-1), the differential memory B1u starts with (zero per 8.7.1, or one);
//!   * whether step 4 reflects on d(f - 1) at all (6.4.1's printed reading, or
//!     the V.90-shaped one where there is no sign step to undo);
//!   * the order of the mixed radix -- interval 0 the least significant, which
//!     is 6.4.1's step 5 read in order, or the most significant;
//!   * the scrambler: which polynomial, whether the bits are multiplied back or
//!     divided, and whether it runs at all (6.4.1: "K scrambled bits" go into
//!     the modulus encoder, so the receiver must undo the scrambler *after* the
//!     mixed radix);
//!   * b0 first in time at the bottom of R (6.4.1 step 1) or at the top;
//!   * whether the scrambler's memory carries across frames or is cleared at
//!     each one.
//!
//! and scores each on the 48 frames of B1u, which should come out all ones. The
//! window slides over the whole stream, so the *frame* origin does not have to
//! be assumed. The *symbol* origin does: one symbol of it is a different
//! trellis frame grouping, which no rearrangement of an already decided trace
//! can reproduce, so that one is `V92_B1U_SKIP`'s business.
//!
//! ```text
//! cargo run --release --example v92_b1u_search -- ETA_TRACE [--frames N]
//! ```

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;

use datapump::v32::{Mode, Scrambler};
use datapump::v92::upstream::UpEncoder;
use datapump::v92::{Filters, Parameters, Trellis, CONSTELLATION_FRAME};
use datapump::v92::precoder::index_to_ki;

/// The twelve data frame intervals (6.4.1, Figure 1).
const UP_INTERVALS: usize = 12;

/// The trellis frame's four symbols (Figure 1), three to a data frame.
const TRELLIS_FRAME: usize = 4;

/// 8.7.1: B1u is "48 data frames of scrambled binary ones".
const B1U_FRAMES: usize = 48;

// ---------------------------------------------------------------------------
// The capture
// ---------------------------------------------------------------------------

/// What `V92_ETA_TRACE` wrote: K, the twelve moduli, and one decided trellis
/// frame's four indices a line.
struct Trace {
    k: u32,
    moduli: [u8; UP_INTERVALS],
    stages: Vec<(u64, [i32; TRELLIS_FRAME])>,
}

fn read_trace(path: &str) -> Result<Trace, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines.next().ok_or("the trace is empty")?;
    let words: Vec<&str> = header.split_whitespace().collect();
    if words.first() != Some(&"k") || words.get(2) != Some(&"moduli") {
        return Err(format!("not an index trace: {header}").into());
    }
    let k: u32 = words[1].parse()?;
    let moduli: Vec<u8> = words[3..].iter().map(|word| word.parse()).collect::<Result<_, _>>()?;
    let moduli: [u8; UP_INTERVALS] = moduli.as_slice().try_into().map_err(|_| "not twelve moduli")?;
    let mut stages = Vec::new();
    for line in lines {
        let numbers: Vec<i64> = line.split_whitespace().map(|word| word.parse()).collect::<Result<_, _>>()?;
        if numbers.len() != 1 + TRELLIS_FRAME {
            return Err(format!("not an index line: {line}").into());
        }
        stages.push((
            numbers[0] as u64,
            [numbers[1] as i32, numbers[2] as i32, numbers[3] as i32, numbers[4] as i32],
        ));
    }
    Ok(Trace { k, moduli, stages })
}

impl Trace {
    /// The capture as data frames of twelve indices.
    ///
    /// Figure 1 puts three trellis frames in a data frame and 8.7.1 begins one
    /// at the first symbol of B1u, so a data frame may only start at a stage
    /// whose index is a multiple of three -- which is the same rule
    /// `v92::decoder` applies when it decides what to keep.
    fn frames(&self) -> Vec<[u8; UP_INTERVALS]> {
        let mut frames = Vec::new();
        let mut pending: Vec<u8> = Vec::with_capacity(UP_INTERVALS);
        for &(index, etas) in &self.stages {
            if pending.is_empty() && !index.is_multiple_of(3) {
                continue;
            }
            for (k, &eta) in etas.iter().enumerate() {
                let interval = pending.len();
                pending.push(index_to_ki(eta, self.moduli[interval], k));
                if pending.len() == UP_INTERVALS {
                    let labels: [u8; UP_INTERVALS] =
                        std::mem::take(&mut pending).try_into().expect("twelve intervals");
                    frames.push(labels);
                }
            }
        }
        frames
    }
}

/// The capture read straight off the equaliser's own levels, with no trellis.
///
/// 6.4.2 makes the precoder pick one member of each equivalence class, so with
/// a prefilter that passes x through (V92_PRE=none) every transmitted level is
/// a constellation point and the nearest one is the index that was sent. What
/// this bypasses is the Viterbi, whose decisions are the other candidate for
/// where a decided index goes wrong.
fn levels_frames(path: &str, trace: &Trace, gain: f64) -> Result<Vec<[u8; UP_INTERVALS]>, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut levels = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let _at = words.next();
        let level: f64 = words.next().ok_or("a level line has no level")?.parse()?;
        levels.push(level);
    }
    let positive: Vec<f64> = (0..64).map(|i| gain * f64::from(2 * i + 1)).collect();
    let nearest = |level: f64| {
        let mut best = (f64::MAX, 0i32);
        for eta in -8i32..8 {
            let point = if eta >= 0 { positive[eta as usize] } else { -positive[(-eta - 1) as usize] };
            let cost = (level - point) * (level - point);
            if cost < best.0 {
                best = (cost, eta);
            }
        }
        best.1
    };
    let mut frames = Vec::new();
    let mut labels = [0u8; UP_INTERVALS];
    for (n, &level) in levels.iter().enumerate() {
        let interval = n % UP_INTERVALS;
        labels[interval] = index_to_ki(nearest(level), trace.moduli[interval], interval % TRELLIS_FRAME);
        if interval == UP_INTERVALS - 1 {
            frames.push(labels);
            labels = [0u8; UP_INTERVALS];
        }
    }
    Ok(frames)
}

/// B1u as 8.7.1 says the analogue modem sends it: 48 data frames of ones
/// through GPA and 6.4, from memories that were all zero at the first symbol.
///
/// This is the *forward* reading of the same question the enumeration above
/// asks backwards, and it is the stronger of the two: 8.7.1 zeroes every
/// memory before B1u, so the expected level of every symbol of B1u is known
/// with nothing left to guess, and the capture can be compared against it
/// symbol by symbol. A convention error in the bit layer, the trellis or the
/// scrambler shows up as a level that is not where this says it should be.
fn b1u_levels(trace: &Trace, gain: f64, frames: usize, trellis: Trellis) -> Vec<u8> {
    let parameters = Parameters {
        drn: (0..=19u8).find(|&drn| datapump::v92::up_bits(drn) == trace.k).unwrap_or(1),
        trellis,
        extend_e2u: false,
        gain,
        moduli: trace.moduli,
        filters: Filters { z2: vec![1.0], ..Filters::default() },
        sets: vec![(0..8).map(|index| (2 * index + 1) as u16).collect()],
        indices: [0; CONSTELLATION_FRAME],
    };
    let Ok(mut encoder) = UpEncoder::new(&parameters) else { return Vec::new() };
    encoder.reset_all();
    let mut levels = Vec::with_capacity(frames * UP_INTERVALS);
    for n in 0..(frames * UP_INTERVALS) {
        let symbol = encoder.next_symbol(|| true);
        // What the digital modem has to recover: not the point, which the
        // analogue modem chose from the class, but the class itself (6.4.2).
        let interval = n % UP_INTERVALS;
        levels.push(index_to_ki(symbol.eta, trace.moduli[interval], interval % TRELLIS_FRAME));
    }
    levels
}

/// How well a forward B1u matches the capture at one symbol offset.
///
/// The equaliser's output is on a different scale from the encoder's `u`, so
/// the fit is the fraction of symbols whose *decided point* is the one B1u
/// puts there, with the capture's scale taken as the CPd gain -- the same
/// division `v92::decoder` does.
fn match_b1u(levels: &[u8], captured: &[f64], at: usize, gain: f64, moduli: &[u8; UP_INTERVALS]) -> (usize, usize) {
    let point = |eta: i32| {
        let magnitude = f64::from(2 * eta.abs() + 1);
        if eta >= 0 { magnitude } else { -magnitude }
    };
    let mut agree = 0usize;
    let mut seen = 0usize;
    for (offset, &ki) in levels.iter().enumerate() {
        let Some(&level) = captured.get(at + offset) else { break };
        seen += 1;
        // The capture on the encoder's scale: G x v(n), divided back by G.
        let target = level / gain;
        let mut best = (f64::MAX, 0i32);
        for candidate in -8i32..8 {
            let cost = (target - point(candidate)) * (target - point(candidate));
            if cost < best.0 {
                best = (cost, candidate);
            }
        }
        let interval = offset % UP_INTERVALS;
        let observed = index_to_ki(best.1, moduli[interval], interval % TRELLIS_FRAME);
        if observed == ki {
            agree += 1;
        }
    }
    (agree, seen)
}

// ---------------------------------------------------------------------------
// The conventions
// ---------------------------------------------------------------------------

/// Whether step 4's reflection is undone at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reflect {
    /// 6.4.1 step 4 as printed: "R0 = R if d(f - 1) = 0; R0 = M - 1 - R if
    /// d(f - 1) = 1", undone against the decoder's own d(f - 1).
    PreviousD,
    /// No sign step: the frame is R0. What a 6.4.1 without V.90's steps 2 to 4
    /// would look like, and the reading a transmitter that only did step 1 and
    /// step 5 would need.
    None,
}

/// Which end of the mixed radix interval 0 is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accumulate {
    /// "Divide R0 by M0 ... the quotient becomes R1 for use in the calculation
    /// for the next data frame interval" -- interval 0 is the least
    /// significant, so it comes back up last.
    IntervalZeroLeast,
    /// The same sum taken the other way up: interval 0 the most significant.
    IntervalZeroMost,
}

/// What is done to the frame's bits, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scrambling {
    /// 6.4.1's "K scrambled bits" (6.3: the analogue modem's polynomial,
    /// GP_A = 1 + x^-5 + x^-23, clause 7/V.34): the transmitter scrambled
    /// before the modulus encoder, so the receiver multiplies back after the
    /// mixed radix.
    Descramble(Mode),
    /// The polynomial run the other way, which is what a receiver that had the
    /// two ends of 6.4.1 the wrong way round would do.
    Scramble(Mode),
    /// Neither.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Convention {
    d_init: bool,
    reflect: Reflect,
    accumulate: Accumulate,
    scrambling: Scrambling,
    /// 6.4.1 step 1's "b0 is first in time" as the least significant bit, or
    /// the other reading, b0 at the top of R.
    b0_at_top: bool,
    /// Whether the scrambler's memory runs on across frames (8.7.1 zeroes it
    /// once, before B1u) or is cleared at each frame.
    reset_each_frame: bool,
}

impl Convention {
    fn combinations() -> Vec<Self> {
        let mut all = Vec::new();
        for d_init in [false, true] {
            for reflect in [Reflect::PreviousD, Reflect::None] {
                for accumulate in [Accumulate::IntervalZeroLeast, Accumulate::IntervalZeroMost] {
                    for scrambling in [
                        Scrambling::Descramble(Mode::Answer),
                        Scrambling::Descramble(Mode::Call),
                        Scrambling::Scramble(Mode::Answer),
                        Scrambling::Scramble(Mode::Call),
                        Scrambling::None,
                    ] {
                        for b0_at_top in [false, true] {
                            for reset_each_frame in [false, true] {
                                all.push(Self {
                                    d_init,
                                    reflect,
                                    accumulate,
                                    scrambling,
                                    b0_at_top,
                                    reset_each_frame,
                                });
                            }
                        }
                    }
                }
            }
        }
        all
    }

    fn label(&self) -> String {
        let reflect = match self.reflect {
            Reflect::PreviousD => "d(f-1)",
            Reflect::None => "none",
        };
        let accumulate = match self.accumulate {
            Accumulate::IntervalZeroLeast => "0=least",
            Accumulate::IntervalZeroMost => "0=most",
        };
        let scrambling = match self.scrambling {
            Scrambling::Descramble(Mode::Answer) => "descramble/answer",
            Scrambling::Descramble(Mode::Call) => "descramble/call",
            Scrambling::Scramble(Mode::Answer) => "scramble/answer",
            Scrambling::Scramble(Mode::Call) => "scramble/call",
            Scrambling::None => "plain",
        };
        let b0 = if self.b0_at_top { "b0 top" } else { "b0 bottom" };
        let reset = if self.reset_each_frame { "scrambler per frame" } else { "scrambler continuous" };
        format!(
            "d(-1)={} reflect {reflect} accumulate {accumulate} {scrambling} {b0} {reset}",
            u8::from(self.d_init)
        )
    }
}

// ---------------------------------------------------------------------------
// One run of the bit layer
// ---------------------------------------------------------------------------

/// 6.4.1 backwards, with the conventions under test.
///
/// Returns the frame's K bits, b0 first in time (6.4.1 step 1).
fn decode_frame(
    labels: &[u8; UP_INTERVALS],
    trace: &Trace,
    convention: Convention,
    d_prev: &mut bool,
) -> Option<Vec<bool>> {
    let moduli = &trace.moduli;
    let k = trace.k;
    let product: u128 = moduli.iter().map(|&m| u128::from(m)).product();
    if product == 0 || k >= 128 || product < 1u128 << k {
        return None;
    }
    // Step 5 backwards: the mixed radix read from interval 11 down to 0, which
    // is R0 = K0 + M0 (K1 + M1 (... + M10 K11)). Turning the twelve round is
    // the same sum with interval 0 at the top.
    let order: Vec<usize> = match convention.accumulate {
        Accumulate::IntervalZeroLeast => (0..UP_INTERVALS).rev().collect(),
        Accumulate::IntervalZeroMost => (0..UP_INTERVALS).collect(),
    };
    let mut r0: u128 = 0;
    for interval in order {
        let modulus = moduli[interval];
        if labels[interval] >= modulus {
            return None;
        }
        r0 = r0 * u128::from(modulus) + u128::from(labels[interval]);
    }
    let m = product;
    // Step 4 backwards, or not at all.
    let r = match convention.reflect {
        Reflect::PreviousD => {
            if *d_prev {
                m.wrapping_sub(1).wrapping_sub(r0)
            } else {
                r0
            }
        }
        Reflect::None => r0,
    };
    // Steps 2 and 3 again, on R, to carry d(f - 1) forward. A convention that
    // puts more in R than the rate carries is masked rather than refused: this
    // is a search over readings of the clause, not a hunt for the one reading
    // that happens to keep every frame inside its rate.
    let s = 2 * r > m.wrapping_sub(1);
    *d_prev ^= s;
    let value = r & ((1u128 << k) - 1);
    Some((0..k).map(|i| (value >> i & 1) == 1).collect::<Vec<bool>>())
}

/// The whole capture through the bit layer, as a bit string.
fn decode_all(frames: &[[u8; UP_INTERVALS]], trace: &Trace, convention: Convention) -> Vec<bool> {
    let mut d_prev = convention.d_init;
    let mut scrambler = match convention.scrambling {
        Scrambling::Descramble(mode) | Scrambling::Scramble(mode) => Some(Scrambler::new(mode)),
        Scrambling::None => None,
    };
    let mut bits = Vec::with_capacity(frames.len() * trace.k as usize);
    for labels in frames {
        let Some(mut frame) = decode_frame(labels, trace, convention, &mut d_prev) else {
            continue;
        };
        if convention.b0_at_top {
            frame.reverse();
        }
        if convention.reset_each_frame
            && let Some(scrambler) = scrambler.as_mut()
        {
            scrambler.reset();
        }
        for bit in frame {
            bits.push(match (&mut scrambler, convention.scrambling) {
                (Some(scrambler), Scrambling::Descramble(_)) => scrambler.descramble(bit),
                (Some(scrambler), Scrambling::Scramble(_)) => scrambler.scramble(bit),
                _ => bit,
            });
        }
    }
    bits
}

/// What a window of the bit string looks like.
struct Window {
    ones: usize,
    at: usize,
    /// The longest run of ones in the whole stream, which is where a B1u of a
    /// length other than 8.7.1's would show.
    run: usize,
}

fn scan(bits: &[bool], length: usize) -> Window {
    let mut running = Vec::with_capacity(bits.len());
    let mut total = 0usize;
    for &bit in bits {
        total += usize::from(bit);
        running.push(total);
    }
    let total_at = |end: usize| if end == 0 { 0 } else { running[end - 1] };
    let mut best = Window { ones: 0, at: 0, run: 0 };
    let mut run = 0usize;
    for (i, &bit) in bits.iter().enumerate() {
        if bit {
            run += 1;
            best.run = best.run.max(run);
        } else {
            run = 0;
        }
        if i + 1 >= length {
            let count = total_at(i + 1) - total_at(i + 1 - length);
            if count > best.ones {
                best.ones = count;
                best.at = i + 1 - length;
            }
        }
    }
    best
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut path = None;
    let mut count = B1U_FRAMES;
    let mut dump: Option<String> = None;
    let mut only: Option<String> = None;
    let mut levels: Option<String> = None;
    let mut gain = 0.108_465_229f64;
    // How far into the capture a window may start. B1u is at the beginning of
    // data mode; the far end's own silence at the end of the call holds levels
    // above the top of the constellation, which slice to all ones and would
    // otherwise win every window.
    let mut within: Option<usize> = None;
    let mut forward = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => count = args.next().ok_or("--frames wants a count")?.parse()?,
            "--dump" => dump = Some(args.next().ok_or("--dump wants a path")?),
            "--only" => only = Some(args.next().ok_or("--only wants a label fragment")?),
            "--levels" => levels = Some(args.next().ok_or("--levels wants a path")?),
            "--gain" => gain = args.next().ok_or("--gain wants a number")?.parse()?,
            "--forward" => forward = true,
            "--within" => within = Some(args.next().ok_or("--within wants a frame count")?.parse()?),
            other => path = Some(other.to_owned()),
        }
    }
    let path = path.ok_or("usage: v92_b1u_search ETA_TRACE [--frames N]")?;
    let trace = read_trace(&path)?;
    let frames = match levels.as_deref() {
        Some(path) => levels_frames(path, &trace, gain)?,
        None => trace.frames(),
    };
    println!(
        "\n{path}: {} decided trellis frames, {} data frames of k = {}, moduli {:?}{}",
        trace.stages.len(),
        frames.len(),
        trace.k,
        trace.moduli,
        if levels.is_some() { " (indices straight off the levels, no Viterbi)" } else { "" }
    );
    if frames.len() < count {
        return Err("not enough whole data frames in the trace".into());
    }

    // The forward reading: B1u's own levels against the capture.
    if forward {
        let Some(path) = levels.as_deref() else {
            return Err("--forward wants --levels".into());
        };
        let captured: Vec<f64> = fs::read_to_string(path)?
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1).and_then(|word| word.parse::<f64>().ok()))
            .collect();
        let codes = [Trellis::Sixteen, Trellis::ThirtyTwo, Trellis::SixtyFour];
        for &trellis in &codes {
        let b1u = b1u_levels(&trace, gain, 64, trellis);
        println!("\n  {trellis:?}: B1u from memories at zero, {} symbols, compared by Ki", b1u.len());
        println!("\n  {:>5}  {:>13}  agreement with the capture", "at", "agree");
        let mut rows = Vec::new();
        let last = captured.len().saturating_sub(b1u.len());
        for at in 0..last {
            let (agree, seen) = match_b1u(&b1u, &captured, at, gain, &trace.moduli);
            if seen == 0 {
                break;
            }
            rows.push((agree, seen, at));
        }
        rows.sort_by_key(|a| std::cmp::Reverse(a.0));
        for &(agree, seen, at) in rows.iter().take(3) {
            println!("  {at:>6}  {agree:>4}/{seen:<4}   {:.1}%", 100.0 * agree as f64 / seen as f64);
        }
        }
        std::io::stdout().flush()?;
        return Ok(());
    }

    // What the indices look like on their own, before any convention. If the
    // physical layer is deciding the constellation properly these are close to
    // uniform over each modulus; a single value twice as common as the rest is
    // a slicer pinned somewhere it should not be.
    let mut histograms: Vec<BTreeMap<u8, usize>> =
        (0..UP_INTERVALS).map(|_| BTreeMap::new()).collect();
    for labels in &frames {
        for (interval, &label) in labels.iter().enumerate() {
            *histograms[interval].entry(label).or_default() += 1;
        }
    }
    println!("  indices by interval:");
    for (interval, histogram) in histograms.iter().enumerate() {
        let mut shown: Vec<String> = histogram.iter().map(|(k, n)| format!("{k}:{n}")).collect();
        if shown.len() > 10 {
            shown.truncate(10);
            shown.push("...".to_owned());
        }
        println!("    {interval:>2}: {}", shown.join(" "));
    }

    // Every convention, over the whole capture.
    let window = count * trace.k as usize;
    println!("\n  the best {count} data frames ({window} bits) anywhere in the stream:");
    let mut scored: Vec<(usize, usize, usize, Convention, Vec<bool>)> = Vec::new();
    for convention in Convention::combinations() {
        let mut bits = decode_all(&frames, &trace, convention);
        if let Some(limit) = within {
            bits.truncate((limit * trace.k as usize).min(bits.len()));
        }
        if bits.len() < window {
            continue;
        }
        let found = scan(&bits, window);
        scored.push((found.ones, found.run, found.at, convention, bits));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    println!("\n  {:>7}  {:>6}  {:>6}  {:>4}  conventions", "ones", "ones%", "at bit", "run");
    for (ones, run, at, convention, _) in scored.iter().take(12) {
        println!(
            "  {ones:>7}  {:>6.1}  {at:>6}  {run:>4}  {}",
            100.0 * *ones as f64 / window as f64,
            convention.label()
        );
    }

    // A caller that names one convention wants that one analysed, whatever
    // else does better, so that two captures can be compared like for like.
    let chosen = match only.as_deref() {
        Some(fragment) => scored.iter().find(|(_, _, _, convention, _)| convention.label().contains(fragment)),
        None => None,
    };
    let Some((ones, run, at, convention, bits)) = chosen.or_else(|| scored.first()) else {
        return Err("nothing decoded".into());
    };
    let (ones, run, at) = (*ones, *run, *at);
    if ones == window {
        println!(
            "\n  B1u is all ones from bit {at} with {}: {window} of {window}.",
            convention.label()
        );
        std::io::stdout().flush()?;
        return Ok(());
    }
    println!(
        "\n  no combination reads B1u as all ones. The best {window}-bit window holds {ones} ones \
         ({:.1}%) from bit {at} with {}, and the longest run of ones anywhere is {run}.",
        100.0 * ones as f64 / window as f64,
        convention.label()
    );

    // Where the ones are, frame by frame, from the winning window: a stretch of
    // B1u that is short of ones shows which symbols went wrong rather than only
    // how many.
    let per_frame = trace.k as usize;
    let first = at / per_frame;
    println!("\n  ones per data frame from frame {first}:");
    for frame in first..(first + count + 8).min(frames.len()) {
        let from = frame * per_frame;
        let to = (from + per_frame).min(bits.len());
        if to - from != per_frame {
            break;
        }
        let ones = bits[from..to].iter().filter(|bit| **bit).count();
        let text: String = bits[from..to].iter().map(|bit| if *bit { '1' } else { '0' }).collect();
        println!("    {frame:>5}: {ones:>2}/{per_frame} {text}");
    }
    let mut octets = BTreeMap::<u8, usize>::new();
    for chunk in bits[at..(at + window).min(bits.len())].chunks(8) {
        if chunk.len() == 8 {
            let mut octet = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                if bit {
                    octet |= 1 << i;
                }
            }
            *octets.entry(octet).or_default() += 1;
        }
    }
    let mut common: Vec<(u8, usize)> = octets.into_iter().collect();
    common.sort_by_key(|a| std::cmp::Reverse(a.1));
    let shown: Vec<String> = common.iter().take(8).map(|(octet, n)| format!("{octet:02x}:{n}")).collect();
    println!("  most common octets in the window: {}", shown.join(" "));

    // Every frame's ones, all the way down the capture: a stretch that is all
    // ones and *is* B1u is where this ends, and a profile that is 50% at the
    // start and 85% later is telling a different story.
    let quarter = (frames.len() / 40).max(1);
    println!("\n  ones per data frame, averaged over {quarter}-frame blocks:");
    let mut block = Vec::new();
    for frame in 0..frames.len() {
        let from = frame * per_frame;
        let to = (from + per_frame).min(bits.len());
        if to - from != per_frame {
            break;
        }
        block.push(bits[from..to].iter().filter(|bit| **bit).count());
        if block.len() == quarter || frame + 1 == frames.len() {
            let total: usize = block.iter().sum();
            println!(
                "    frames {frame:>6}: {:>5.1}% ones",
                100.0 * total as f64 / (block.len() * per_frame) as f64
            );
            block.clear();
        }
    }
    if let Some(path) = dump {
        let text: String = bits
            .chunks(per_frame)
            .map(|frame| frame.iter().map(|bit| if *bit { '1' } else { '0' }).collect::<String>())
            .collect::<Vec<String>>()
            .join("\n");
        fs::write(&path, text + "\n")?;
        println!("  bits written to {path}");
    }
    std::io::stdout().flush()?;
    Ok(())
}
