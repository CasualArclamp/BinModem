//! What the far end is sending, told by how it repeats.
//!
//! Every signal a control channel starts or stops with is a fixed pattern
//! that repeats: a tone every symbol, AC and Sh every two, PPh every eight.
//! Data does not repeat at all, being scrambled. So the watch sets each
//! half-symbol sample of the matched filter's output against the one 1, 2, 4
//! and 8 symbols before it, and what those four correlations come to says
//! which signal it is -- before anything is trained, whatever the carrier's
//! phase, and wherever in the symbol the samples fall.
//!
//! That last because the samples are two a symbol. A symbol stream's
//! correlation at a whole number of symbols, averaged over two samples half a
//! symbol apart, is what it is averaged over every instant: the pulse is
//! band-limited to less than the symbol rate, so the product of two copies of
//! the signal has no component at twice the symbol rate, and its component at
//! the symbol rate cancels between the two halves. What is left is the
//! symbols' own periodic autocorrelation, seen through the pulse: at lag L,
//! the sum over the pattern's eight spectral lines of each line's power times
//! the pulse's folded power response there, turned by L eighths of a turn per
//! line (`signature`).
//!
//! Two stages. A signal that repeats every two symbols -- tone, AC, Sh --
//! correlates fully at lags 2, 4 and 8, and lag 1 says which: the same
//! (tone), the opposite (AC), or a quarter turn each way in turn (Sh, which
//! the pulse makes a third). A signal that only repeats every eight -- PPh --
//! correlates fully at lag 8 and not at 2, and lags 1, 2 and 4 tell 10-2's
//! two readings apart: the perfect sequence's are nearly nought, the printed
//! square wave's lag 4 is minus one. The carrier's offset turns each
//! correlation by the lag times its turn a symbol, which the correlation that
//! decided the stage measures and the others are turned back by.
//!
//! And one thing more: Sh turning into S-bar-h, the half turn 12.6's
//! resynchronisation hangs on. Over two symbols, the lag-2 correlation flips
//! from one to minus one as the reversal passes.

use std::collections::VecDeque;

use dsp::Complex;

use super::{PPH_PERIOD, Reading, ac, sh};
use crate::v34::dpsk::ROLLOFF;

/// What the far end is sending, as the receiver hears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hearing {
    /// No carrier in the far end's band.
    Nothing,
    /// A carrier with nothing on it: tone A or tone B, which 12.4.3.1 and
    /// 12.7 have a modem answer.
    Tone,
    /// AC (10.2.4.1): the far end wants the control channel retrained.
    Ac,
    /// Sh, or S-bar-h after it (10.2.3.3), which repeat alike.
    Sh,
    /// Something that repeats every eight symbols as PPh does, in one
    /// reading or the other. Not yet PPh: the receiver has to find all 32 of
    /// its symbols to say so.
    Pph(Reading),
    /// Something that does not repeat: ALT, MPh, E, data -- or a page on the
    /// primary channel, or noise loud enough to be taken for a carrier.
    Modulated,
}

/// A change the watch has seen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Seen {
    /// The far carrier came or went.
    Carrier(bool),
    /// A new signal, which began at half `since`. `turn` is the carrier's
    /// turn a symbol against ours, in radians, as the signal showed it.
    Began { hearing: Hearing, since: u64, turn: f64 },
    /// Sh turned half way round into S-bar-h at about half `at`.
    Reversal { at: u64, turn: f64 },
}

/// Lags correlated at, in symbols.
const LAGS: [usize; 4] = [1, 2, 4, 8];

/// Symbols each correlation is summed over. Sixteen: PPh's lag-8 products
/// are all PPh from its symbol 8 to its 31st, and a window of sixteen of
/// them is still full of PPh for the last eight.
const WINDOW: usize = 16;

/// Half symbols a class has to hold for before it is believed: four
/// symbols.
const CONFIRM: usize = 8;

/// How much of the signal's power a correlation has to account for for the
/// signal to repeat at its lag. Noise ten decibels down leaves a repeating
/// signal 0.91; data scrambled at random over sixteen symbols comes this
/// close about once in a hundred windows, and to a class's signature as well
/// far less often -- and a PPh taken wrongly is caught when the receiver
/// looks for all of it.
const PERIODIC: f64 = 0.6;

/// How far from a class's signature the correlations may be, as a distance
/// between their normalised values.
const NEAR: f64 = 0.3;

/// The lag-2 correlation over two symbols below which Sh has turned into
/// S-bar-h.
const REVERSED: f64 = -0.3;

/// The far carrier's envelope, over 20 ms, at which it is taken to have come
/// and to have gone: V.32's levels (`v32/receiver.rs`), five decibels of
/// hysteresis, in the units of a baseband doubled back to the far end's own
/// size.
const CARRIER_ON: f64 = 2.0e-3;
const CARRIER_OFF: f64 = 1.124e-3;
const ENVELOPE_SECONDS: f64 = 0.020;

/// One lag's correlation, summed over the last [`WINDOW`] symbols.
#[derive(Debug, Clone)]
struct Lag {
    /// The lag, in half symbols.
    halves: usize,
    products: VecDeque<(Complex, f64)>,
    sum: Complex,
    power: f64,
    length: usize,
}

impl Lag {
    fn new(symbols: usize, window: usize) -> Self {
        Self {
            halves: 2 * symbols,
            products: VecDeque::with_capacity(2 * window + 1),
            sum: Complex::ZERO,
            power: 0.0,
            length: 2 * window,
        }
    }

    /// Take the newest half, given the halves before it, newest last.
    fn push(&mut self, recent: &VecDeque<Complex>) {
        let n = recent.len();
        if n <= self.halves {
            return;
        }
        let (now, then) = (recent[n - 1], recent[n - 1 - self.halves]);
        let entry = (now * then.conj(), 0.5 * (now.norm_sqr() + then.norm_sqr()));
        self.sum += entry.0;
        self.power += entry.1;
        self.products.push_back(entry);
        if self.products.len() > self.length
            && let Some((product, power)) = self.products.pop_front()
        {
            self.sum -= product;
            self.power -= power;
        }
    }

    /// Sum again from what is kept, against rounding creeping in.
    fn refresh(&mut self) {
        self.sum = self.products.iter().fold(Complex::ZERO, |s, p| s + p.0);
        self.power = self.products.iter().map(|p| p.1).sum();
    }

    /// The correlation as a share of the power, once the window is full.
    fn value(&self) -> Option<Complex> {
        (self.products.len() == self.length && self.power > 0.0).then(|| self.sum.scale(1.0 / self.power))
    }
}

/// The classes a repeating signal can be, and where their correlations lie.
#[derive(Debug, Clone)]
struct Signatures {
    /// Lag 1 against lag 2, for the signals that repeat every two symbols.
    every_two: [(Hearing, f64); 3],
    /// Lags 1, 2 and 4 against lag 8, for PPh's two readings.
    every_eight: [(Hearing, [f64; 3]); 2],
}

impl Signatures {
    fn new() -> Self {
        let period = |f: &dyn Fn(usize) -> Complex| -> [Complex; PPH_PERIOD] { std::array::from_fn(f) };
        let tone = signature(&period(&|_| sh(0)));
        let alternating = signature(&period(&ac));
        let sh = signature(&period(&sh));
        let with_i = signature(&period(&|i| Reading::WithI.point(i)));
        let printed = signature(&period(&|i| Reading::AsPrinted.point(i)));
        Self {
            every_two: [(Hearing::Tone, tone[0] / tone[1]), (Hearing::Ac, alternating[0] / alternating[1]), (Hearing::Sh, sh[0] / sh[1])],
            every_eight: [(Hearing::Pph(Reading::WithI), with_i), (Hearing::Pph(Reading::AsPrinted), printed)],
        }
    }
}

/// A period-8 pattern's correlations at lags 1, 2 and 4 against lag 0 (and
/// so against lag 8), as the watch sees them through the pulse twice over --
/// the far end's root raised cosine and the matched filter's.
///
/// Line q of the pattern's eight, at q/8 of the symbol rate, has power
/// |S_q|^2, and comes through the pulse at that frequency and at its alias a
/// symbol rate below, each weighted by the raised cosine's square. The
/// weights are 1 at the carrier and a half at half the symbol rate, where
/// either alias is at the raised cosine's midpoint.
fn signature(pattern: &[Complex; PPH_PERIOD]) -> [f64; 3] {
    let raised = |f: f64| {
        let f = f.abs();
        let (low, high) = ((1.0 - ROLLOFF) / 2.0, (1.0 + ROLLOFF) / 2.0);
        if f <= low {
            1.0
        } else if f >= high {
            0.0
        } else {
            0.5 * (1.0 + (std::f64::consts::PI / ROLLOFF * (f - low)).cos())
        }
    };
    let n = PPH_PERIOD as f64;
    let lines: Vec<(f64, f64)> = (0..PPH_PERIOD)
        .map(|q| {
            let s = pattern.iter().enumerate().fold(Complex::ZERO, |sum, (k, x)| {
                sum + *x * Complex::from_polar(1.0, -std::f64::consts::TAU * (q * k) as f64 / n)
            });
            let f = q as f64 / n;
            (f, s.norm_sqr() * (raised(f).powi(2) + raised(f - 1.0).powi(2)))
        })
        .collect();
    let total: f64 = lines.iter().map(|l| l.1).sum();
    [1usize, 2, 4].map(|lag| {
        lines.iter().map(|&(f, p)| p * (std::f64::consts::TAU * f * lag as f64).cos()).sum::<f64>() / total
    })
}

/// The far end's signal, classed.
#[derive(Debug, Clone)]
pub(super) struct Watch {
    /// The newest halves: enough for the longest lag.
    recent: VecDeque<Complex>,
    lags: [Lag; 4],
    /// Lag 2 over two symbols, for Sh's reversal.
    short: Lag,
    signatures: Signatures,
    /// The class the newest halves suggest, for how many halves, and from
    /// which.
    candidate: Hearing,
    held: usize,
    /// What is believed, and since which half.
    hearing: Hearing,
    since: u64,
    /// The carrier's turn a symbol, as the repeating signal in hand has it.
    turn: f64,
    /// Whether the Sh in hand may yet reverse.
    armed: bool,
    envelope: f64,
    keep: f64,
    carrier: bool,
    /// Halves taken.
    count: u64,
    seen: VecDeque<Seen>,
}

impl Watch {
    /// A watch on halves taken at `rate` a second.
    pub(super) fn new(rate: f64) -> Self {
        Self {
            recent: VecDeque::with_capacity(2 * 8 + 2),
            lags: LAGS.map(|lag| Lag::new(lag, WINDOW)),
            short: Lag::new(2, 2),
            signatures: Signatures::new(),
            candidate: Hearing::Nothing,
            held: 0,
            hearing: Hearing::Nothing,
            since: 0,
            turn: 0.0,
            armed: false,
            envelope: 0.0,
            keep: (-1.0 / (ENVELOPE_SECONDS * rate)).exp(),
            carrier: false,
            count: 0,
            seen: VecDeque::new(),
        }
    }

    pub(super) fn hearing(&self) -> Hearing {
        self.hearing
    }

    /// The half at which what is heard began.
    pub(super) fn since(&self) -> u64 {
        self.since
    }

    pub(super) fn carrier(&self) -> bool {
        self.carrier
    }

    /// The far carrier's envelope.
    pub(super) fn level(&self) -> f64 {
        self.envelope
    }

    pub(super) fn seen(&mut self) -> Option<Seen> {
        self.seen.pop_front()
    }

    /// Take the next half-symbol sample of the matched filter's output.
    pub(super) fn push(&mut self, half: Complex) {
        let index = self.count;
        self.count += 1;
        self.envelope = self.keep * self.envelope + (1.0 - self.keep) * half.abs();
        let carrier = if self.carrier { self.envelope > CARRIER_OFF } else { self.envelope > CARRIER_ON };
        if carrier != self.carrier {
            self.carrier = carrier;
            self.seen.push_back(Seen::Carrier(carrier));
        }
        self.recent.push_back(half);
        if self.recent.len() > 2 * 8 + 1 {
            self.recent.pop_front();
        }
        for lag in &mut self.lags {
            lag.push(&self.recent);
        }
        self.short.push(&self.recent);
        if index % 4096 == 4095 {
            for lag in &mut self.lags {
                lag.refresh();
            }
        }
        let (class, turn) = self.classify();
        if class == self.candidate {
            self.held += 1;
        } else {
            self.candidate = class;
            self.held = 1;
        }
        if self.held == CONFIRM && class != self.hearing {
            self.hearing = class;
            self.since = index + 1 - CONFIRM as u64;
            if let Some(turn) = turn {
                self.turn = turn;
            }
            self.armed = class == Hearing::Sh;
            self.seen.push_back(Seen::Began { hearing: class, since: self.since, turn: self.turn });
        } else if class == self.hearing
            && let Some(turn) = turn
        {
            self.turn = turn;
        }
        if self.armed && self.hearing == Hearing::Sh {
            self.reversal(index);
        }
    }

    /// What the newest halves are, and the turn they show if they repeat.
    fn classify(&self) -> (Hearing, Option<f64>) {
        if !self.carrier {
            return (Hearing::Nothing, None);
        }
        let [Some(one), Some(two), Some(four), Some(eight)] = self.lags.each_ref().map(Lag::value) else {
            return (Hearing::Modulated, None);
        };
        // Turned back by the carrier's turn over the lag, and against how
        // much of the signal repeats at all.
        let along = |c: Complex, lag: f64, turn: f64, whole: f64| (c * Complex::from_polar(1.0, -lag * turn)).re / whole;
        // What repeats every two symbols repeats every four and eight too,
        // and scrambled data seldom comes near at even one of them: a false
        // tone, AC or Sh would drop a receiver out of its data.
        if [two, four, eight].iter().all(|c| c.abs() >= PERIODIC) {
            let turn = two.arg() / 2.0;
            let first = along(one, 1.0, turn, two.abs());
            let best = self.signatures.every_two.iter().map(|&(h, s)| (h, (first - s).abs())).min_by(|a, b| a.1.total_cmp(&b.1));
            return match best {
                Some((hearing, distance)) if distance < NEAR => (hearing, Some(turn)),
                _ => (Hearing::Modulated, None),
            };
        }
        if eight.abs() >= PERIODIC {
            let turn = eight.arg() / 8.0;
            let whole = eight.abs();
            let v = [along(one, 1.0, turn, whole), along(two, 2.0, turn, whole), along(four, 4.0, turn, whole)];
            let best = self
                .signatures
                .every_eight
                .iter()
                .map(|&(h, s)| (h, s.iter().zip(&v).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt()))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            return match best {
                Some((hearing, distance)) if distance < NEAR => (hearing, Some(turn)),
                _ => (Hearing::Modulated, None),
            };
        }
        (Hearing::Modulated, None)
    }

    /// Sh turning into S-bar-h: over the last two symbols, each half set
    /// against the one two symbols before it has turned half way round.
    fn reversal(&mut self, index: u64) {
        let Some(short) = self.short.value() else { return };
        let along = (short * Complex::from_polar(1.0, -2.0 * self.turn)).re;
        if along < REVERSED {
            self.armed = false;
            // The two symbols the correlation spans straddle the reversal.
            self.seen.push_back(Seen::Reversal { at: index.saturating_sub(2), turn: self.turn });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signatures_tell_every_repeating_signal_apart() {
        let signatures = Signatures::new();
        let two: Vec<f64> = signatures.every_two.iter().map(|s| s.1).collect();
        // Tone and AC are the same and the opposite a symbol on; Sh's lines
        // at half the symbol rate come through the pulse at a quarter of the
        // power, which leaves a third.
        assert!((two[0] - 1.0).abs() < 1e-9 && (two[1] + 1.0).abs() < 1e-9, "{two:?}");
        assert!((two[2] - 1.0 / 3.0).abs() < 1e-9, "{two:?}");
        for (i, a) in two.iter().enumerate() {
            for b in &two[i + 1..] {
                assert!((a - b).abs() > 2.0 * NEAR, "{two:?}");
            }
        }
        let [(_, with_i), (_, printed)] = signatures.every_eight;
        // The perfect sequence is nearly flat through the pulse; the printed
        // square wave is the opposite of itself four symbols on.
        assert!(with_i.iter().all(|v| v.abs() < 0.2), "{with_i:?}");
        assert!((printed[2] + 1.0).abs() < 1e-9, "{printed:?}");
        let apart = with_i.iter().zip(&printed).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
        assert!(apart > 2.0 * NEAR, "{apart}");
    }
}
