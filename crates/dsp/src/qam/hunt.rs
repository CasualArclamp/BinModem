//! Looking for S and its change to S-bar in half-symbol samples.
//!
//! Copied from `v34/receiver.rs:276-364`, whose comment says why it works.
//! V.34's S and V.32's are the same signal: two points a quarter turn apart,
//! alternating, and S-bar is S turned half a revolution (V.32 5.2.1, 5.2.2;
//! core.md 2.1). A half-symbol sample set against the one two symbols before
//! it is the same all through S, whatever the timing and the carrier's phase,
//! so S is found before anything has been trained; and once S is sure its
//! four samples are learned as a template, and S-bar is the template turned
//! round.
//!
//! Two things are added.
//!
//! The discriminator. V.32's preamble tones AA, CC, AC and CA also repeat
//! every two symbols, and V.34's hunt took AA to CC and AC to CA for S and
//! S-bar (core.md E6). They differ from S in where their power is. S, A and B
//! in turn, has a line at the carrier and one at each band edge, 1200 Hz
//! either side at 2400 baud: DC, and plus and minus a quarter of the
//! half-symbol rate. AA is a bare carrier, all at DC; AC is the carrier
//! suppressed, all at the edges. So a half is S-like only if V.34's test
//! holds and DC's share of the three lines is between a quarter and 0.92
//! (design.md 3.4). V.32 2.2 allows the edges 2 to 7 dB down, which puts S's
//! share at 1 / (1 + 2 x 10^(-L/10)), 0.44 to 0.71; the far end's pulse is not
//! known, so the test is a ratio and not anything about its shape.
//!
//! The carrier's frequency. S is periodic, so its offset from our carrier is
//! there for the taking before any training (design.md 3.3). Roughly, from
//! the turn of the same two-symbol correlation the hunt already makes, which
//! is unambiguous to a quarter of the symbol rate; finely, from the turn
//! between the first and second halves of the S heard since arming, about a
//! hundred symbols apart, unwrapped by the rough figure.
//!
//! And the far clock's drift, from the same two halves. A least-squares
//! solve fits one equaliser across its whole window, and V.34's trained
//! 7.5 dB worse at 200 ppm than at none, because a tenth of a symbol of
//! timing went by in the window; the taps it made to fit that drift go on
//! amplifying noise out of band long after the timing loop has taken the
//! drift. S's band-edge lines turn against each other by half a turn for
//! every symbol the timing moves, whatever the carrier does, so S says how
//! fast the timing moves before TRN begins, and training can read its rows
//! on a grid that moves with it.

use std::collections::VecDeque;

use super::AUDIBLE;
use crate::Complex;

/// Halves of S in a row before its template is trusted: twenty symbols
/// (`v34/receiver.rs:301`).
const HELD: usize = 40;

/// Halves the discriminator weighs: four periods of S.
const WEIGHED: usize = 16;

/// DC's share of S's three lines, least and most (design.md 3.4).
const SHARE: (f64, f64) = (0.25, 0.92);

/// Halves of S added together for the fine frequency, and the most of them
/// used: sixteen symbols a chunk, and 256 symbols in two blocks of 128.
const CHUNK: usize = 32;
const CHUNKS_USED: usize = 16;

/// The four quarter turns a half-symbol sample steps through at a quarter of
/// the half-symbol rate, which is where S's band-edge lines fall.
const QUARTERS: [Complex; 4] = [Complex::ONE, Complex::I, Complex::new(-1.0, 0.0), Complex::new(0.0, -1.0)];

/// What a hunt came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Hunted {
    S,
    /// S-bar begins at half `at`, give or take one, after an S whose carrier
    /// turns by `turn` radians a symbol against ours, and whose symbols are
    /// `drift` longer than the half-symbol samples' spacing makes them.
    Reversal { at: u64, turn: f64, drift: f64 },
    /// S stopped at half `at` without turning into S-bar, having turned by
    /// `turn` and drifted by `drift` as a reversal's are measured.
    Lapsed { at: u64, turn: f64, drift: f64 },
}

#[derive(Debug, Clone, Default)]
pub(super) struct Hunt {
    /// The last four samples: two symbols.
    recent: VecDeque<Complex>,
    /// The last eight correlations with two symbols before, and powers.
    correlations: VecDeque<(Complex, f64)>,
    held: usize,
    /// S's four half-symbol samples, averaged, once it is sure.
    template: [Complex; 4],
    armed: bool,
    /// The last four samples against the template, and the template's power.
    matches: VecDeque<(f64, f64)>,
    lapsed: usize,

    discriminate: bool,
    /// The last few halves, each with its index, for the discriminator.
    weighed: VecDeque<(Complex, u64)>,
    /// The two-symbol correlations of all the S heard so far, summed.
    coarse: Complex,
    /// S since arming, in chunks of each of its four samples summed.
    chunk: [Complex; 4],
    in_chunk: usize,
    chunks: Vec<[Complex; 4]>,
}

impl Hunt {
    pub(super) fn new(discriminate: bool) -> Self {
        Self { discriminate, ..Self::default() }
    }

    pub(super) fn feed(&mut self, half: Complex, index: u64) -> Option<Hunted> {
        let phase = (index % 4) as usize;
        if self.discriminate {
            self.weighed.push_back((half, index));
            if self.weighed.len() > WEIGHED {
                self.weighed.pop_front();
            }
        }
        if self.armed {
            // As `v34/receiver.rs:305-330`.
            let t = self.template[phase];
            self.matches.push_back(((half * t.conj()).re, t.norm_sqr()));
            if self.matches.len() > 4 {
                self.matches.pop_front();
            }
            let (along, power) = self.matches.iter().fold((0.0, 0.0), |(a, p), &(x, y)| (a + x, p + y));
            let ratio = along / power.max(1e-12);
            if self.matches.len() == 4 && ratio < -0.5 {
                // The first of the four that turned it.
                let (turn, drift) = self.estimate();
                return Some(Hunted::Reversal { at: index.saturating_sub(3), turn, drift });
            }
            if ratio > 0.5 {
                self.template[phase] = self.template[phase].scale(0.9) + half.scale(0.1);
                self.lapsed = 0;
                if let Some(before) = self.recent.front() {
                    self.coarse += half * before.conj();
                }
                self.chunk[phase] += half;
                self.in_chunk += 1;
                if self.in_chunk == CHUNK {
                    self.chunks.push(std::mem::take(&mut self.chunk));
                    self.in_chunk = 0;
                }
            } else {
                self.lapsed += 1;
                if self.lapsed > 24 {
                    let at = index.saturating_sub(24);
                    // What S said of the carrier and the clock holds whether
                    // or not S-bar came after it, and a driver that trains
                    // anyway wants it.
                    let (turn, drift) = self.estimate();
                    *self = Self::new(self.discriminate);
                    return Some(Hunted::Lapsed { at, turn, drift });
                }
            }
            self.recent.push_back(half);
            if self.recent.len() > 4 {
                self.recent.pop_front();
            }
            return None;
        }
        // As `v34/receiver.rs:332-362`.
        if self.recent.len() == 4 {
            let before = self.recent[0];
            let c = half * before.conj();
            let p = (half.norm_sqr() + before.norm_sqr()) / 2.0;
            self.correlations.push_back((c, p));
            if self.correlations.len() > 8 {
                self.correlations.pop_front();
            }
            let (sum_c, sum_p) =
                self.correlations.iter().fold((Complex::ZERO, 0.0), |(sc, sp), &(c, p)| (sc + c, sp + p));
            let mut s_like = sum_c.re > 0.7 * sum_p && sum_p / self.correlations.len() as f64 > AUDIBLE;
            if s_like && self.discriminate {
                s_like = self.share().is_some_and(|share| (SHARE.0..=SHARE.1).contains(&share));
            }
            if s_like {
                self.held += 1;
                // Averaged over the last stretch of S, weighted to the newest.
                self.template[phase] = self.template[phase].scale(0.8) + half.scale(0.2);
                self.coarse += c;
            } else {
                self.held = 0;
                self.template = [Complex::ZERO; 4];
                self.coarse = Complex::ZERO;
            }
            if self.held >= HELD {
                self.armed = true;
                self.lapsed = 0;
                self.matches.clear();
                self.chunk = [Complex::ZERO; 4];
                self.in_chunk = 0;
                self.chunks.clear();
                self.recent.pop_front();
                self.recent.push_back(half);
                return Some(Hunted::S);
            }
            self.recent.pop_front();
        }
        self.recent.push_back(half);
        None
    }

    /// DC's share of the power in the last few halves' three lines: at the
    /// carrier, and a quarter of the half-symbol rate either side of it.
    fn share(&self) -> Option<f64> {
        if self.weighed.len() < WEIGHED {
            return None;
        }
        let (mut dc, mut above, mut below) = (Complex::ZERO, Complex::ZERO, Complex::ZERO);
        for &(h, n) in &self.weighed {
            let q = QUARTERS[(n % 4) as usize];
            dc += h;
            above += h * q.conj();
            below += h * q;
        }
        let (dc, edges) = (dc.norm_sqr(), above.norm_sqr() + below.norm_sqr());
        Some(dc / (dc + edges).max(1e-30))
    }

    /// The carrier's turn a symbol and the far clock's drift, from the S
    /// heard.
    ///
    /// The turn is the two-symbol correlation's turn, halved, and then the
    /// turn between the first and second halves of the S since arming,
    /// unwrapped by it. The drift is from S's two band-edge lines: a far
    /// clock running fast brings its symbols in a little earlier every
    /// symbol, which turns the upper line back and the lower one on, by half
    /// a turn for every symbol of timing, whatever the carrier does to both.
    fn estimate(&self) -> (f64, f64) {
        let coarse = self.coarse.arg() / 2.0;
        // The newest chunk is the one nearest S-bar, where the change is
        // already smeared into what is heard.
        let usable = self.chunks.len().saturating_sub(1);
        let count = usable.min(CHUNKS_USED) & !1;
        if count < 2 {
            return (coarse, 0.0);
        }
        let blocks = &self.chunks[usable - count..usable];
        let sum = |chunks: &[[Complex; 4]]| {
            chunks.iter().fold([Complex::ZERO; 4], |mut s, c| {
                for (a, b) in s.iter_mut().zip(c) {
                    *a += *b;
                }
                s
            })
        };
        let (early, late) = (sum(&blocks[..count / 2]), sum(&blocks[count / 2..]));
        let lean = early.iter().zip(&late).fold(Complex::ZERO, |s, (e, l)| s + *l * e.conj());
        // Symbols between the two blocks' middles.
        let lag = (count / 2 * CHUNK / 2) as f64;
        let expected = coarse * lag;
        let turn = (expected + wrap(lean.arg() - expected)) / lag;
        // The upper line against the lower, in each block: a half-symbol
        // sample's four phases step through a quarter turn each at the band
        // edges.
        let edges = |s: &[Complex; 4]| {
            let (upper, lower) = s.iter().zip(QUARTERS).fold((Complex::ZERO, Complex::ZERO), |(u, l), (x, q)| (u + *x * q.conj(), l + *x * q));
            upper * lower.conj()
        };
        let moved = wrap((edges(&late) * edges(&early).conj()).arg());
        // Symbols of timing the lines moved by, a symbol: how much longer the
        // far end's symbols are than the ones sampled.
        let drift = (-moved / (std::f64::consts::TAU * lag)).clamp(-DRIFT_LIMIT, DRIFT_LIMIT);
        (turn, drift)
    }
}

/// The most drift S is believed to show: the timing loop's own limit.
const DRIFT_LIMIT: f64 = 0.001;

/// An angle brought to within half a turn either way.
fn wrap(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}
