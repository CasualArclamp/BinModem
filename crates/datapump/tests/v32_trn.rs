//! TRN, segment 3 of the conditioning signal (V.32 5.2.3), checked against
//! the page, against our own transmitter, and against a real modem.
//!
//! TRN is the one stretch of a V.32 start-up a receiver can know symbol for
//! symbol before it arrives: binary ones through the sender's scrambler,
//! started from all zeros, A or C by the first bit of each pair for 256
//! symbols and then Table 5 by both. It is what the far equaliser trains on,
//! and that is exactly why a mistake in it cannot be seen between two ends of
//! our own. Both would make the same mistake, the one sending it and the one
//! comparing against it, and train on each other perfectly. Our transmitter
//! did make one -- Table 5 read in counting order, C sent for D and D for C
//! from symbol 256 on -- and nothing of ours could have noticed.
//!
//! A real modem can. `tests/vectors/v32bis-14400.wav` is a Conexant softmodem
//! calling another, both directions on one tap, and each end's first
//! conditioning signal is on the line alone. So this file brings both TRNs to
//! points with a front end of its own -- a mixer, `dsp::fir_lowpass` as an
//! interpolator, samples half a symbol apart, S found by correlation, the
//! turn and the sender's clock estimated -- and solves for an equaliser by
//! least squares against [`TrnSequence`], once with Table 5 as printed and
//! once with C and D exchanged. The printed table fits the answering modem at
//! 30.5 dB and the calling one at 29.2; the exchanged one fits either at a
//! little over 1 dB, which is what half the symbols a quarter turn out comes
//! to.
//!
//! What the recording turned out to hold, measured here:
//!
//! - **The answering modem** sends S, S-bar and TRN exactly as Figure 4
//!   draws them. S-bar begins at 3.846 s, TRN symbol 0 comes 16 symbols after
//!   it (5.2.2's time reference, on a real modem), and TRN runs for 2560
//!   symbols, to 4.920 s, when R1 begins. Its carrier is 1800 Hz to within a
//!   hundredth of a hertz and its symbol clock is the recording's own.
//! - **The calling modem's** S runs straight into TRN symbol 193 at 7.423 s:
//!   there is no S-bar, and symbols 0 to 192 are not there. After the join
//!   the carrier reads 0.19 Hz high and the symbol clock 104 parts per
//!   million fast, which are the same proportion, while S before it reads
//!   1800 Hz; and the carrier's phase steps by about 17 degrees across it. A
//!   modem's oscillator does none of that and a cut in the recording would,
//!   so this is most likely the recording. TRN carries on to symbol 4800,
//!   about 9.34 s.
//! - The windows these were first attributed to, 4.25-7.00 s and 7.75-10.50
//!   s, are each end's whole transmission after S: TRN and then R1 or R2. The
//!   fits here use symbols 256 to 2303 of each TRN, which both contain.

use datapump::v32::{self, Mode, STATE_C, STATE_D, Signal, Transmitter, TrnSequence};
use dsp::{Complex, fir_lowpass, least_squares};
use std::ops::Range;

const VECTOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/vectors/v32bis-14400.wav"
);

const FS: f64 = 16_000.0;

/// Line samples in half a symbol, the equaliser's spacing.
const HALF: f64 = FS / (2.0 * v32::BAUD);

/// The four states, from Figure 1/V.32 as rendered: A (-3, -1), B (1, -3),
/// C (3, 1), D (-1, 3). Written out here rather than taken from the crate, so
/// that the check does not lean on the thing it checks.
const STATES: [(f64, f64); 4] = [(-3.0, -1.0), (1.0, -3.0), (3.0, 1.0), (-1.0, 3.0)];

/// The first 288 symbols of each end's TRN, from spec.md 4.3: the rule of
/// 5.2.3 run from zero, with Table 5 from symbol 256.
const CALL_288: &str = concat!(
    "CCCCCCCCCAAACCCCCCAAAAACCCCAAACCAAACAAAAAAAAACAACCCCCCACCCACCCCA",
    "CCAACAACACCCAACCCCCCAAAAAACCAAACCCCAACAAAAACAACCCCCAACACAACCAACC",
    "AACAAAACACCCCAACACACAAACCCCAAACCACACCAACAACAACAACCCACCCCAAAAACAA",
    "CAAACCACCCCAAAAACCACACACCAACCCAAAAAACACCAACCAACCCCAAACACAACAACAC",
    "ACCBCAADBBCDCCBACDCACCCCADCBABDB",
);
const ANSWER_288: &str = concat!(
    "CCCAACCCAACCACCAACAACCAAACACAACCAAAACACCAACCCCCACACACCAACAAACCAC",
    "CCCACCACCCAACAAACACAAAACAACACCACAAAAACCACAAACAACCCCACACACACAACCC",
    "CCACAACACACCCCCCACAACACAAAAACACACCACACCCCCACACACACAAAAAAACACAACA",
    "CCAAAACCACCAACAAACCCCAACCACCACAACAACAACCCAACAAAAAAACACCAACCACACC",
    "CBABAACDDCBBCCADBCABABCACCCACCDD",
);

fn letter(state: usize) -> char {
    char::from(b"ABCD"[state])
}

#[test]
fn the_first_288_symbols_are_the_ones_the_recommendation_gives() {
    for (mode, want) in [(Mode::Call, CALL_288), (Mode::Answer, ANSWER_288)] {
        let mut trn = TrnSequence::new(mode);
        let got: String = (0..288).map(|_| letter(trn.next())).collect();
        assert_eq!(got, want, "{mode:?}");
    }
    // And the fifteen 5.2.3 itself prints under its dibits, in time order,
    // for the calling modem's polynomial and then the answering modem's.
    let mut call = TrnSequence::new(Mode::Call);
    let mut answer = TrnSequence::new(Mode::Answer);
    let call: String = (0..15).map(|_| letter(call.next())).collect();
    let answer: String = (0..15).map(|_| letter(answer.next())).collect();
    assert_eq!(call, "CCCCCCCCCAAACCC");
    assert_eq!(answer, "CCCAACCCAACCACC");
}

/// Send `symbols` of one signal and note the state of each.
///
/// At 2400 samples a second the transmitter makes exactly one symbol per
/// sample, so the state it reports after each sample is that sample's symbol.
fn send(tx: &mut Transmitter, signal: Signal, symbols: usize) -> Vec<usize> {
    tx.set_signal(signal);
    (0..symbols)
        .map(|_| {
            tx.next_sample();
            tx.state()
        })
        .collect()
}

#[test]
fn the_transmitter_sends_trn_as_the_sequence_gives_it() {
    for mode in [Mode::Call, Mode::Answer] {
        let mut want = TrnSequence::new(mode);
        let want: Vec<usize> = (0..8192).map(|_| want.next()).collect();

        let mut tx = Transmitter::new(mode, v32::BAUD);
        send(&mut tx, Signal::ConditioningS, 256);
        send(&mut tx, Signal::ConditioningSbar, 16);
        // The longest TRN 5.2.3 allows.
        let first = send(&mut tx, Signal::Trn, 8192);
        assert_eq!(first, want, "{mode:?}, the first TRN");

        // A rate signal runs the scrambler on from wherever TRN left it, and
        // the next conditioning signal's TRN has to start from zero again.
        send(&mut tx, Signal::Rate(0b0000_1111_1111_1001), 64);
        send(&mut tx, Signal::ConditioningS, 256);
        send(&mut tx, Signal::ConditioningSbar, 16);
        let second = send(&mut tx, Signal::Trn, 1280);
        assert_eq!(second, want[..1280], "{mode:?}, a second TRN");
    }
}

// ---------------------------------------------------------------------------
// The front end.

/// Taps each side of the interpolator's centre, in line samples.
const REACH: usize = 48;

/// Phases of the interpolator between one line sample and the next.
const PHASES: usize = 64;

/// Taps of the equaliser, half a symbol apart: V.34's 31, fifteen and a half
/// symbols, which is more than the channel and the pulse need between them.
const TAPS: usize = 31;

/// The symbols of TRN every fit is measured over: all of them past the change
/// to Table 5 up to 2303, which both real modems' TRN contains.
const FIT: Range<usize> = 256..2304;

/// The symbols the unequalised search for TRN correlates over.
const FIND: Range<usize> = 256..768;

/// As little of a receiver as it takes to bring a TRN back to points.
struct FrontEnd {
    /// The line mixed down by the nominal carrier, 2 x[n] e^(-j 2 pi 1800 n / fs),
    /// so that the far end's signal sits either side of zero.
    mixed: Vec<Complex>,
    /// A low-pass at 1600 Hz, designed by `dsp::fir_lowpass` at `PHASES`
    /// times the line's rate, so that reading it at a fraction of a sample is
    /// picking the right taps: the low-pass and the interpolator are one
    /// filter. It takes out the image the mixer leaves at twice the carrier.
    taps: Vec<f64>,
}

impl FrontEnd {
    fn new(line: &[f64]) -> Self {
        let step = std::f64::consts::TAU * v32::CARRIER / FS;
        let mixed = line
            .iter()
            .enumerate()
            .map(|(n, &x)| Complex::from_polar(2.0 * x, -step * n as f64))
            .collect();
        let taps = fir_lowpass(1600.0, 2 * REACH * PHASES + 1, FS * PHASES as f64);
        Self { mixed, taps }
    }

    /// The low-passed baseband at `t`, in line samples.
    fn at(&self, t: f64) -> Complex {
        let centre = (self.taps.len() / 2) as f64;
        let first = (t - REACH as f64).ceil().max(0.0) as usize;
        let last = ((t + REACH as f64).floor() as usize).min(self.mixed.len() - 1);
        let mut sum = Complex::ZERO;
        for n in first..=last {
            let k = (centre + (n as f64 - t) * PHASES as f64).round() as usize;
            if let Some(&h) = self.taps.get(k) {
                sum += self.mixed[n] * h;
            }
        }
        // Each phase of a filter designed for unit gain at the higher rate
        // sums to one part in `PHASES`.
        sum * PHASES as f64
    }

    /// `count` samples half a symbol apart, from a sender whose clock runs
    /// `ppm` fast, with sample `anchor` falling at `at` line samples.
    fn halves(&self, at: f64, anchor: usize, count: usize, ppm: f64) -> Vec<Complex> {
        let spacing = HALF / (1.0 + ppm * 1e-6);
        (0..count)
            .map(|k| self.at(at + (k as f64 - anchor as f64) * spacing))
            .collect()
    }
}

/// Take a steady turn out: `omega` radians a half symbol.
fn derotate(y: &[Complex], omega: f64) -> Vec<Complex> {
    y.iter()
        .enumerate()
        .map(|(k, &v)| v * Complex::from_polar(1.0, -omega * k as f64))
        .collect()
}

/// Where S was, in half-symbols.
struct S {
    /// The first of its own.
    began: usize,
    /// The first past it.
    stops: usize,
    /// Where S-bar begins, if it came next: the first of the four
    /// half-symbols that the reversal turns negative.
    reversal: Option<usize>,
}

/// Where S stops, found by correlation.
///
/// S repeats every two symbols, so each half-symbol sample is the one four
/// before it over again, whatever the channel, the timing or the carrier's
/// phase. S-bar is S turned half a revolution, so across the change the
/// product with the one four back goes negative for two symbols and then
/// positive again for the rest of S-bar; anything that follows S without an
/// S-bar leaves the product positive no longer.
///
/// A stretch only counts as S after 64 symbols of it, which nothing in TRN
/// manages, and only if the signal is still there after it: an alternation
/// falling silent, as AC does before S, is not a reversal.
fn where_s_stops(y: &[Complex]) -> Option<S> {
    let mut product = vec![0.0; y.len() + 1];
    let mut power = vec![0.0; y.len() + 1];
    for k in 0..y.len() {
        let (p, e) = match k.checked_sub(4) {
            Some(back) => (
                (y[k] * y[back].conj()).re,
                0.5 * (y[k].norm_sqr() + y[back].norm_sqr()),
            ),
            None => (0.0, 0.0),
        };
        product[k + 1] = product[k] + p;
        power[k + 1] = power[k] + e;
    }
    let alike = |a: usize, b: usize| (product[b] - product[a]) / (power[b] - power[a] + 1e-30);
    let loud = |a: usize, b: usize| (power[b] - power[a]) / (b - a) as f64;

    let mut began = None;
    for k in 4..y.len().saturating_sub(40) {
        if alike(k, k + 4) >= 0.7 {
            began.get_or_insert(k);
            continue;
        }
        if let Some(start) = began.take()
            && k - start >= 128
            && loud(k, k + 32) > 0.5 * loud(k - 64, k)
        {
            // S-bar's first four half-symbols are where the product over four
            // is most negative, a little after it first dips: the pulses
            // either side of the change reach across it.
            let reversal = (alike(k + 6, k + 30) >= 0.7).then(|| {
                (k..k + 12)
                    .min_by(|&a, &b| alike(a, a + 4).total_cmp(&alike(b, b + 4)))
                    .expect("a reversal has a middle")
            });
            return Some(S {
                began: start,
                stops: k,
                reversal,
            });
        }
    }
    None
}

/// The carrier's offset from nominal, from S: radians a half symbol.
///
/// S is the same every two symbols, so a sample against the one a whole
/// number of periods before it differs only by how far the carrier has
/// turned in between. The longer the lag the finer the measurement; half the
/// stretch leaves the other half to average over.
fn turn_from_s(s: &[Complex]) -> f64 {
    let s = &s[s.len().saturating_sub(400)..];
    let lag = 4 * (s.len() / 8);
    let mut sum = Complex::ZERO;
    for k in lag..s.len() {
        sum += s[k] * s[k - lag].conj();
    }
    sum.arg() / lag as f64
}

/// TRN's targets at unit power, with C and D exchanged from symbol 256 when
/// `exchanged` is set: the mapping our transmitter used to send.
fn targets(mode: Mode, exchanged: bool, count: usize) -> Vec<Complex> {
    let mut trn = TrnSequence::new(mode);
    (0..count)
        .map(|n| {
            let state = match trn.next() {
                STATE_C if exchanged && n >= 256 => STATE_D,
                STATE_D if exchanged && n >= 256 => STATE_C,
                state => state,
            };
            let (re, im) = STATES[state];
            Complex::new(re, im) / 10f64.sqrt()
        })
        .collect()
}

/// The samples the equaliser sees for symbol `n` when symbol 0 is centred on
/// sample `c0`.
fn row(y: &[Complex], c0: usize, n: usize) -> Option<&[Complex]> {
    let centre = c0 + 2 * n;
    y.get(centre.checked_sub(TAPS / 2)?..centre + TAPS / 2 + 1)
}

/// What the equaliser `w` makes of one row.
fn equalise(row: &[Complex], w: &[Complex]) -> Complex {
    row.iter()
        .zip(w)
        .fold(Complex::ZERO, |acc, (&x, &k)| acc + x * k)
}

/// Least squares for the equaliser over `window`, and how close it came to
/// the targets there, in dB.
fn solve(
    y: &[Complex],
    c0: usize,
    t: &[Complex],
    window: Range<usize>,
) -> Option<(Vec<Complex>, f64)> {
    let rows: Vec<&[Complex]> = window
        .clone()
        .map(|n| row(y, c0, n))
        .collect::<Option<_>>()?;
    let energy: f64 = rows
        .iter()
        .flat_map(|r| r.iter())
        .map(|x| x.norm_sqr())
        .sum();
    // A light ridge. Half of what an equaliser at twice the symbol rate sees
    // is the band beyond the signal, where nothing holds the weights.
    let w = least_squares(&rows, &t[window.clone()], 1e-4 * energy / TAPS as f64)?;
    let (mut signal, mut error) = (0.0, 0.0);
    for (r, &want) in rows.iter().zip(&t[window]) {
        signal += want.norm_sqr();
        error += (equalise(r, &w) - want).norm_sqr();
    }
    Some((w, 10.0 * (signal / error).log10()))
}

/// How far the solved equaliser's output keeps turning against the targets
/// over `window`: radians a symbol, from the phase of successive blocks.
fn residual_turn(
    y: &[Complex],
    c0: usize,
    w: &[Complex],
    t: &[Complex],
    window: Range<usize>,
) -> f64 {
    let block = 128;
    let mut points = Vec::new();
    let mut unwrapped = 0.0;
    let mut last = None;
    for start in window.clone().step_by(block) {
        let end = (start + block).min(window.end);
        let mut sum = Complex::ZERO;
        for (n, want) in t.iter().enumerate().take(end).skip(start) {
            if let Some(r) = row(y, c0, n) {
                sum += equalise(r, w) * want.conj();
            }
        }
        let phase = sum.arg();
        if let Some(before) = last {
            let mut step: f64 = phase - before;
            step -= std::f64::consts::TAU * (step / std::f64::consts::TAU).round();
            unwrapped += step;
        }
        last = Some(phase);
        points.push((start as f64 + block as f64 / 2.0, unwrapped));
    }
    let n = points.len() as f64;
    let (mx, my) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), &(x, y)| (a + x / n, b + y / n));
    let (sxy, sxx) = points.iter().fold((0.0, 0.0), |(a, b), &(x, y)| {
        (a + (x - mx) * (y - my), b + (x - mx) * (x - mx))
    });
    sxy / sxx
}

/// What the front end made of one end's TRN.
struct Heard {
    /// Where S stopped, in seconds.
    s_stops: f64,
    /// Half-symbols from where S-bar begins to the centre of TRN symbol 0, if
    /// there was an S-bar.
    after_reversal: Option<i64>,
    /// Which symbol of TRN was arriving as S stopped, counting from where
    /// symbol 0 is or would have been. Negative when TRN began after S.
    symbol_as_s_stops: f64,
    /// The carrier's offset, and how fast the sender's clock runs.
    turn_hz: f64,
    ppm: f64,
    /// The least-squares fit over [`FIT`], in dB.
    fit: f64,
}

impl std::fmt::Display for Heard {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "S stops at {:.4} s, ", self.s_stops)?;
        match self.after_reversal {
            Some(halves) => write!(f, "TRN symbol 0 {halves} half-symbols after the reversal")?,
            None => write!(
                f,
                "no S-bar, TRN symbol {:.1} as S stops",
                self.symbol_as_s_stops
            )?,
        }
        write!(
            f,
            "; turn {:+.3} Hz, clock {:+.0} ppm; fit {:.2} dB",
            self.turn_hz, self.ppm, self.fit
        )
    }
}

/// Bring the TRN of the end at `mode`, somewhere between `from` and `to`
/// seconds of `line`, back to points against Table 5, or against the
/// exchanged mapping, and say how well it fits.
///
/// Each mapping gets every chance: its own search for where TRN is, its own
/// clock, its own turn and its own equaliser.
fn hear(fe: &FrontEnd, mode: Mode, from: f64, to: f64, exchanged: bool) -> Heard {
    let start = from * FS;
    let count = ((to - from) * FS / HALF) as usize;
    let nominal = fe.halves(start, 0, count, 0.0);

    // S, and the turn from it.
    let s = where_s_stops(&nominal).expect("no S found");
    let stops = s.stops;
    let mut omega = turn_from_s(&nominal[s.began..stops]);

    // Where TRN is: the known sequence against the unequalised samples, at
    // every alignment from S running into TRN 320 symbols in to TRN starting
    // 24 symbols after S. Only the main tap of the channel has to be there
    // for the right one to stand out.
    let t = targets(mode, exchanged, FIT.end + 16);
    let y = derotate(&nominal, omega);
    let mut c0 = 0;
    let mut best = 0.0;
    for c in stops.saturating_sub(640).max(TAPS)..stops + 48 {
        let (mut sum, mut energy) = (Complex::ZERO, 0.0);
        for n in FIND {
            let Some(&v) = y.get(c + 2 * n) else { break };
            sum += v * t[n].conj();
            energy += v.norm_sqr();
        }
        let score = sum.norm_sqr() / (energy + 1e-30);
        if score > best {
            best = score;
            c0 = c;
        }
    }

    // The turn again, from TRN itself: what the solved equaliser's output
    // still does against the targets, block by block. S cannot be trusted
    // with it on its own. The calling modem's S in the recording reads 1800
    // Hz and its TRN a fifth of a hertz more, which is part of why the join
    // between them looks like a cut.
    let refine = |y: &[Complex], mut omega: f64| {
        for _ in 0..3 {
            let turned = derotate(y, omega);
            let Some((w, _)) = solve(&turned, c0, &t, FIT) else {
                break;
            };
            omega += residual_turn(&turned, c0, &w, &t, FIT) / 2.0;
        }
        omega
    };
    omega = refine(&nominal, omega);

    // The sender's clock: resampled about the middle of the fit, so that the
    // alignment just found stays put, and kept at the best fit. A coarse pass
    // and then a fine one, and the turn again at the clock found.
    let anchor = c0 + FIT.start + FIT.end;
    let at = start + anchor as f64 * HALF;
    let mut ppm = 0.0;
    for (span, step) in [(200, 20), (18, 2)] {
        let centre = ppm;
        let mut best = f64::NEG_INFINITY;
        for k in (-span..=span).step_by(step) {
            let candidate = centre + f64::from(k);
            let y = derotate(&fe.halves(at, anchor, count, candidate), omega);
            let db = solve(&y, c0, &t, FIT).map_or(f64::NEG_INFINITY, |(_, db)| db);
            if db > best {
                best = db;
                ppm = candidate;
            }
        }
    }
    let resampled = fe.halves(at, anchor, count, ppm);
    omega = refine(&resampled, omega);

    // And the alignment checked either side.
    let y = derotate(&resampled, omega);
    let fit = (c0 - 2..=c0 + 2)
        .filter_map(|c| solve(&y, c, &t, FIT).map(|(_, db)| db))
        .fold(f64::NEG_INFINITY, f64::max);

    // Where TRN is against S is read from the correlation's peak, which is
    // where the channel's main tap puts each symbol. The least-squares
    // alignment is free to wander a half-symbol or two from it, since
    // thirty-one taps can put their centre wherever they like.
    Heard {
        s_stops: from + stops as f64 * HALF / FS,
        after_reversal: s.reversal.map(|r| c0 as i64 - r as i64),
        symbol_as_s_stops: (stops as f64 - c0 as f64) / 2.0,
        turn_hz: omega * 2.0 * v32::BAUD / std::f64::consts::TAU,
        ppm,
        fit,
    }
}

/// Both mappings for one end, printed.
fn both(fe: &FrontEnd, mode: Mode, from: f64, to: f64, name: &str) -> (Heard, Heard) {
    let printed = hear(fe, mode, from, to, false);
    let exchanged = hear(fe, mode, from, to, true);
    println!("{name} {mode:?}, Table 5:           {printed}");
    println!("{name} {mode:?}, C and D exchanged: {exchanged}");
    (printed, exchanged)
}

/// TRN symbol 0 comes sixteen symbols after S-bar begins (5.2.2): 32
/// half-symbols, give or take how the channel and the equaliser share the
/// delay between them.
fn trn_follows_sbar_by_16_symbols(heard: &Heard) -> bool {
    heard
        .after_reversal
        .is_some_and(|halves| (30..=34).contains(&halves))
}

#[test]
fn our_own_trn_fits_table_5_through_the_same_front_end() {
    // The front end proved on a signal with nothing wrong with it, so that
    // what it says about a real modem is about the modem.
    for mode in [Mode::Call, Mode::Answer] {
        let mut tx = Transmitter::new(mode, FS);
        let mut line = Vec::new();
        for (signal, symbols) in [
            (Signal::ConditioningS, 256),
            (Signal::ConditioningSbar, 16),
            (Signal::Trn, 2560),
            (Signal::Rate(0b0000_1111_1111_1001), 256),
        ] {
            tx.set_signal(signal);
            let samples = (symbols as f64 * FS / v32::BAUD) as usize;
            line.extend((0..samples).map(|_| tx.next_sample()));
        }
        let fe = FrontEnd::new(&line);
        let (printed, exchanged) = both(&fe, mode, 0.0, line.len() as f64 / FS, "ours");
        assert!(
            trn_follows_sbar_by_16_symbols(&printed),
            "{mode:?}: our TRN is not where S-bar puts it: {printed}"
        );
        assert!(
            printed.fit >= 40.0,
            "{mode:?}: our own TRN fits Table 5 at only {:.1} dB",
            printed.fit
        );
        assert!(
            exchanged.fit <= printed.fit - 6.0,
            "{mode:?}: the exchanged mapping fits at {:.1} dB against {:.1}",
            exchanged.fit,
            printed.fit
        );
    }
}

#[test]
fn a_real_modems_trn_is_table_5_as_printed() {
    let wav = line::wav::read(VECTOR).expect("read V.32bis vector");
    assert_eq!(f64::from(wav.sample_rate), FS);
    let line: Vec<f64> = wav.mono().iter().map(|&s| f64::from(s)).collect();
    let fe = FrontEnd::new(&line);

    // The answering modem's first S, S-bar and TRN, while the calling modem
    // is silent. From just after its AC, so that the S found is the S.
    let (printed, exchanged) = both(&fe, Mode::Answer, 3.75, 5.0, "answering");
    // 5.2.2's time reference, kept by a real modem.
    assert!(
        trn_follows_sbar_by_16_symbols(&printed),
        "the answering modem's TRN is not where its S-bar puts it: {printed}"
    );
    assert!(
        printed.fit >= 25.0,
        "the answering modem's TRN fits Table 5 at {:.1} dB",
        printed.fit
    );
    assert!(
        exchanged.fit <= printed.fit - 6.0,
        "the answering modem's TRN fits the exchanged mapping at {:.1} dB against {:.1}",
        exchanged.fit,
        printed.fit
    );

    // The calling modem's, alone on the line once the answering modem has
    // stopped R1 on hearing its S. In this recording that S runs straight
    // into TRN partway through: see the module comment.
    let (printed, exchanged) = both(&fe, Mode::Call, 7.2, 8.6, "calling");
    assert!(
        printed.after_reversal.is_none() && (188.0..=196.0).contains(&printed.symbol_as_s_stops),
        "the calling modem's S no longer runs into TRN symbol 193: {printed}"
    );
    assert!(
        printed.fit >= 25.0,
        "the calling modem's TRN fits Table 5 at {:.1} dB",
        printed.fit
    );
    assert!(
        exchanged.fit <= printed.fit - 6.0,
        "the calling modem's TRN fits the exchanged mapping at {:.1} dB against {:.1}",
        exchanged.fit,
        printed.fit
    );
}
