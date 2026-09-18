//! The clause 6.4 chain below the modulus encoder: the constellations and
//! equivalence classes of 6.4.2, the precoder and the prefilter, the inverse
//! map of 6.4.3, and the V.34 convolutional codes clocked once every four
//! symbols (6.4.4).
//!
//! The modulus encoder hands down twelve numbers per data frame and not one of
//! them names a level. Ki names an *equivalence class*: every index congruent
//! to Ki modulo Mi, spread right across the constellation. The transmitter
//! picks a member, and that freedom is the whole point of the structure. The
//! digital modem has sent down a precoder whose feedback section inverts the
//! line's spectral nulls; left to itself such a filter runs away, and 6.4.2
//! has no modulo operation anywhere in it to fold the excursion back. What
//! folds it back is the choice: at every symbol the transmitter spends the
//! choice on the class member that keeps |x(n)| smallest, which is exactly
//! what the digital modem assumed when it designed the coefficients -- "the
//! digital modem should design the precoder coefficients under the assumption
//! that the analogue modem minimizes the power at the precoder output on a
//! symbol-by-symbol basis" (the NOTE to 8.8.3).
//!
//! Equalising on the transmit side rather than the receive side is what the
//! whole arrangement is for. Everything in front of the central office's A/D
//! is equalised before the quantiser, so the codec sees u(n) itself and the
//! digital modem's decisions become decisions about which codeword arrived,
//! with no receive filter left to colour the quantisation noise.
//!
//! The fourth symbol of every trellis frame gives up half of its freedom. Its
//! class steps by 2 x Mi instead of Mi, and which of the two halves it lands in
//! carries the parity the convolutional encoder asked for, so that
//! eta0 + eta1 + eta2 + eta3 is Y0 modulo 2. The redundancy of a V.34 trellis
//! code therefore lives in the parity of four indices and not in a modulo
//! encoder: 6.4.4 cites only 9.6.3.2/V.34, so V.34's own modulo encoder C0 and
//! its superframe inversion V0 are not brought in, and there is no upstream
//! superframe for the second of those to key off in any case.
//!
//! ```text
//!   Ki --> [precoder: pick u(n) from E(Ki)] --x(n)--> [prefilter] --> G x v(n)
//!               ^                       |
//!               | Y0                    | y(n) = eta
//!        [conv. encoder] <--Y1:Y4-- [inverse map]
//! ```
//!
//! One chain serves both modems (AD-6). The analogue modem transmits with it;
//! the digital modem designs against it, verifies a candidate CPd by running it
//! before it sends it, and predicts B1u with it. That is why nothing here has
//! ever heard of Table 30: it is built from [`Parameters`] and from nothing
//! else.

use super::{CONSTELLATION_FRAME, Filters, Parameters, TRELLIS_FRAME, Trellis, UP_INTERVALS};
use crate::v34::trellis::{self, Code};

/// How a tie in the point selection is broken: `true` takes the index of
/// smaller magnitude, `false` the one of larger magnitude.
///
/// 6.4.2 says only that "the precoder selects a point u(n) from the
/// equivalence class E(Ki)" and gives no rule whatever. The rule implemented
/// is the one the NOTE to 8.8.3 assumes -- minimise the power at the precoder
/// output symbol by symbol -- and that settles every choice except an exact
/// tie, which falls out whenever -c(n) sits midway between two class members,
/// and always on the first symbol of a run, where c(n) is zero and the two
/// members a(-1) and a(0) are equidistant.
///
/// Nothing on the wire depends on which way a tie goes: the digital modem
/// decodes the index it actually receives, and 6.4.1's differential sign step
/// makes even a wholesale inversion of every index decode the same. The
/// setting is named so that a capture which shows a far end's transmitter
/// preferring the larger magnitude can be matched with one edit.
pub const TIE_TO_SMALLER_INDEX: bool = true;

/// The largest |G x v(n)| this module hands on, in the units 8.8.3 fixes,
/// where "the prefilter output multiplied by G has a mean-square value of 1"
/// and TRN1u's +/-LU is therefore +/-1 as well (3.8).
///
/// Derived, not printed: clause 6 bounds nothing. Pitfall P-12 is why there is
/// a bound at all -- the precoder is not a modulo precoder, so a coefficient
/// set that is wrong, or an interval whose class has a single member, lets
/// x(n) grow without limit, and what grows must not reach the line as noise.
/// Eight is 18 dB above the mean square the design works to, which a healthy
/// chain never comes near: the class choice holds |x(n)| inside half the widest
/// class spacing, and G is picked so that G x v(n) has a mean square of 1. So
/// the clamp only ever catches a runaway. If a capture ever shows a conforming
/// far end swinging further than this, this is the one number to raise.
///
/// It is a bound in *those* units and in no others. A chain run with a trial
/// gain -- which is how the digital modem finds G in the first place -- is not
/// in them, and anything measuring power before G is settled reads
/// [`Symbol::v`] and multiplies by its own gain rather than reading
/// [`Symbol::out`].
///
/// The filter state is never clamped, only [`Symbol::out`]. Clamping the state
/// would desynchronise the analogue modem from the digital modem's model of
/// it, and keeping those two identical is the one thing this chain exists for.
pub const OUTPUT_LIMIT: f64 = 8.0;

// ---------------------------------------------------------------------------
// Constellations (6.4.2)
// ---------------------------------------------------------------------------

/// The N = 2 x LC levels of one constellation set, indexed as 6.4.2 indexes
/// them.
///
/// CPd carries only the LC positive magnitudes, smallest first. 6.4.2 mirrors
/// them: "Let the N constellation points be denoted by a(eta),
/// -N/2 <= eta < N/2, where the indices are in the same order as the levels...
/// Thus negative points have negative indices, and positive points have
/// non-negative indices." So a(eta) = P[eta] for eta >= 0 and a(eta) =
/// -P[-eta-1] for eta < 0, which makes a(-1) the smallest negative level and
/// a(eta) rise with eta right across the range.
///
/// The mirroring is an interpretation, not a printed rule (P4D DC-5): the
/// Recommendation prints the positive half and the ordering, and this is the
/// only reading of the two together that fills the negative indices.
#[derive(Debug, Clone, PartialEq)]
pub struct Constellation {
    /// P[0] < P[1] < ... < P[LC-1], the levels the wire carried.
    positives: Vec<f64>,
}

impl Constellation {
    /// One set from CPd's ascending "linear value" words.
    ///
    /// The scale of those words is not this module's business: G normalises
    /// the power, so the chain works in whatever units the design chose.
    pub fn new(points: &[u16]) -> Self {
        Self { positives: points.iter().map(|&p| f64::from(p)).collect() }
    }

    /// LC, "the number of positive points in the constellation set" (Table 30).
    pub fn positive_points(&self) -> usize {
        self.positives.len()
    }

    /// N = 2 x LC, "one of 2*LC1 through 2*LC6 in Table 30" (6.4.2).
    pub fn levels(&self) -> usize {
        2 * self.positives.len()
    }

    /// -N/2, the lowest index the set holds.
    pub fn lowest(&self) -> i32 {
        -(self.positives.len() as i32)
    }

    /// N/2 - 1, the highest index the set holds. 6.4.2's range is
    /// -N/2 <= eta < N/2, so the top index is one below N/2.
    pub fn highest(&self) -> i32 {
        self.positives.len() as i32 - 1
    }

    /// Whether `eta` is one of this set's indices.
    pub fn holds(&self, eta: i32) -> bool {
        (self.lowest()..=self.highest()).contains(&eta)
    }

    /// a(eta), or `None` for an index the set does not hold.
    pub fn level(&self, eta: i32) -> Option<f64> {
        self.holds(eta).then(|| self.at(eta))
    }

    /// a(eta) for an index already known to be in range.
    fn at(&self, eta: i32) -> f64 {
        if eta >= 0 { self.positives[eta as usize] } else { -self.positives[(-eta - 1) as usize] }
    }

    /// The index whose level is nearest `target`, ignoring equivalence classes.
    ///
    /// Levels rise with the index, so this is a binary search: partition the
    /// positive magnitudes and compare the two either side. A negative target
    /// is mirrored rather than searched, because a(-eta-1) = -a(eta) reverses
    /// the order and negates the levels, leaving every distance as it was.
    fn nearest(&self, target: f64) -> i32 {
        if self.positives.is_empty() {
            // A set with no points has no index. `choose` never gets here,
            // because an empty set has an empty span first.
            return 0;
        }
        if target < 0.0 {
            return -1 - self.nearest(-target);
        }
        let top = self.positives.len() - 1;
        let above = self.positives.partition_point(|&p| p < target);
        if above == 0 {
            // Every level is at or above the target, so the smallest positive
            // one wins: a(-1) is further away than a(0) for any target >= 0.
            return 0;
        }
        if above > top {
            return top as i32;
        }
        let below = above - 1;
        if target - self.positives[below] <= self.positives[above] - target {
            below as i32
        } else {
            above as i32
        }
    }

    /// The member of `class` inside this set whose level is nearest `target`,
    /// or `None` if the class has no member here at all.
    ///
    /// `target` is -c(n): choosing the member nearest it is choosing the member
    /// that minimises |x(n)| = |a(eta) + c(n)|, which is the rule the NOTE to
    /// 8.8.3 assumes. Ties go by [`TIE_TO_SMALLER_INDEX`].
    pub fn choose(&self, class: Class, target: f64) -> Option<i32> {
        let (first, last) = class.span(self.lowest(), self.highest())?;
        // The class member nearest in *index* to the level-nearest index is
        // the one nearest in level as well, because the levels rise with the
        // index; its two neighbours cover the clamping at either end.
        let middle = class.step_at_or_below(self.nearest(target)).clamp(first, last);
        let mut best: Option<(f64, i32)> = None;
        for step in [middle - 1, middle, middle + 1] {
            if step < first || step > last {
                continue;
            }
            let eta = class.index(step);
            let distance = (self.at(eta) - target).abs();
            let better = match best {
                None => true,
                Some((far, _)) if distance < far => true,
                Some((far, _)) if distance > far => false,
                Some((_, chosen)) => (eta.abs() < chosen.abs()) == TIE_TO_SMALLER_INDEX,
            };
            if better {
                best = Some((distance, eta));
            }
        }
        best.map(|(_, eta)| eta)
    }
}

// ---------------------------------------------------------------------------
// Equivalence classes (6.4.2)
// ---------------------------------------------------------------------------

/// One equivalence class E(Ki): the indices base + z x step, z any integer.
///
/// "E(Ki) = {a(eta_k) | eta_k = Ki + z_k Mi, z_k an integer} for k = 0, 1, 2;
/// {a(eta_k) | eta_k = 2Ki + 2 z_k Mi + (eta_0 + eta_1 + eta_2 + Y0) mod 2,
/// z_k an integer} for k = 3" (6.4.2).
///
/// The fourth position is the trellis code's: its class steps twice as far, so
/// half as many members are on offer, and the half it sits in is fixed by the
/// parity of the three indices already chosen in this trellis frame together
/// with Y0. That is pitfall P-4 -- the indices *actually chosen*, and a
/// non-negative modulo 2 of a sum that can be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Class {
    base: i64,
    step: i64,
}

impl Class {
    /// E(Ki) for a data frame interval whose trellis position is `k = i mod 4`.
    ///
    /// `parity` is the (eta_0 + eta_1 + eta_2 + Y0) mod 2 term and is read only
    /// at k = 3, where 6.4.2 puts it.
    pub fn for_interval(ki: u8, modulus: u8, k: usize, parity: bool) -> Self {
        let ki = i64::from(ki);
        let modulus = i64::from(modulus.max(1));
        if k == TRELLIS_FRAME - 1 {
            Self { base: 2 * ki + i64::from(parity), step: 2 * modulus }
        } else {
            Self { base: ki, step: modulus }
        }
    }

    /// How far apart the members are: Mi at k = 0, 1, 2 and 2 x Mi at k = 3.
    pub fn spacing(self) -> i64 {
        self.step
    }

    /// The member `step` steps along from the class's base index. A step far
    /// enough out to leave an index behind is clamped, because no constellation
    /// reaches anywhere near i32's ends and a wrapped index would be a member
    /// of some other class.
    pub fn index(self, step: i64) -> i32 {
        self.base
            .saturating_add(step.saturating_mul(self.step))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }

    /// Whether `eta` belongs to the class.
    pub fn holds(self, eta: i32) -> bool {
        (i64::from(eta) - self.base).rem_euclid(self.step) == 0
    }

    /// The step numbers of the first and last members inside
    /// `lowest..=highest`, or `None` when the class has no member there --
    /// which is the infeasible parameter set of 6.4.2, reported rather than
    /// panicked on.
    fn span(self, lowest: i32, highest: i32) -> Option<(i64, i64)> {
        let first = ceiling(i64::from(lowest) - self.base, self.step);
        let last = (i64::from(highest) - self.base).div_euclid(self.step);
        (first <= last).then_some((first, last))
    }

    /// The step number of the last member at or below `eta`.
    fn step_at_or_below(self, eta: i32) -> i64 {
        (i64::from(eta) - self.base).div_euclid(self.step)
    }
}

/// `a / b` rounded towards positive infinity, for a positive `b`.
fn ceiling(a: i64, b: i64) -> i64 {
    -((-a).div_euclid(b))
}

/// The Ki an index came from: 6.4.2's class map read backwards, which is what
/// the digital modem's decoder needs once the Viterbi has settled on an index.
///
/// For k = 0, 1, 2 the class is eta = Ki + z Mi, so Ki is eta modulo Mi taken
/// non-negative. For k = 3 the class is eta = 2Ki + 2z Mi + p, so the parity
/// bit p comes off the bottom of eta first and Ki is ((eta - p) / 2) modulo Mi.
pub fn index_to_ki(eta: i32, modulus: u8, k: usize) -> u8 {
    let modulus = i32::from(modulus.max(1));
    if k == TRELLIS_FRAME - 1 {
        let parity = eta.rem_euclid(2);
        (((eta - parity) / 2).rem_euclid(modulus)) as u8
    } else {
        eta.rem_euclid(modulus) as u8
    }
}

// ---------------------------------------------------------------------------
// The inverse map and the convolutional codes (6.4.3, 6.4.4)
// ---------------------------------------------------------------------------

/// The V.34 code CPd bits 27:28 asked for: "0 = 16 state, 1 = 32 state,
/// 2 = 64 state" (Table 30), which are the codes of 9.6.3.2/V.34 "except that
/// the 2T delays are replaced by 4T delays" (6.4.4).
///
/// Replacing the delays is not a change to the code at all, only to how often
/// it is clocked: V.34's 4D interval was two symbols and V.92's trellis frame
/// is four, so `v34::trellis::Code` is used exactly as V.34 uses it and
/// [`Chain`] clocks it once every four symbols.
pub fn code_for(trellis: Trellis) -> Code {
    match trellis {
        Trellis::Sixteen => Code::States16,
        Trellis::ThirtyTwo => Code::States32,
        Trellis::SixtyFour => Code::States64,
    }
}

/// The 3-bit subset label of a pair of indices: Figure 9/V.34 on the
/// odd-integer coordinates, which 6.4.3 "calculated as 2 x y(k) + 1".
pub fn label_of(first: i32, second: i32) -> u8 {
    trellis::label((2 * first + 1, 2 * second + 1))
}

/// The inverse map of 6.4.3: the four indices of a trellis frame to
/// [Y4 Y3 Y2 Y1] as bits 3 to 0.
///
/// "For each trellis frame the inverse map takes the two pairs (y(0),y(1)) and
/// (y(2),y(3)) and produces Y1, Y2, Y3 and Y4. It is identical to the
/// symbol-to-bit converter described in 9.6.3.1/V.34" -- so the two pairs are
/// V.34's y(2m) and y(2m+1), their labels come from Figure 9/V.34 and the four
/// bits from Table 13/V.34, both of which `v34::trellis` already carries and
/// tests against the rendered pages.
pub fn inverse_map(etas: [i32; TRELLIS_FRAME]) -> u8 {
    trellis::convert(label_of(etas[0], etas[1]), label_of(etas[2], etas[3]))
}

// ---------------------------------------------------------------------------
// The filters (6.4.2)
// ---------------------------------------------------------------------------

/// A tapped delay line. `back(kappa)` is the value kappa symbols ago.
#[derive(Debug, Clone, Default)]
struct Delay {
    line: Vec<f64>,
    /// Where the next value goes, so `back(1)` is the one behind it.
    head: usize,
}

impl Delay {
    fn new(taps: usize) -> Self {
        Self { line: vec![0.0; taps], head: 0 }
    }

    fn back(&self, kappa: usize) -> f64 {
        if self.line.is_empty() {
            return 0.0;
        }
        debug_assert!(kappa <= self.line.len(), "the delay line is shorter than the tap asking of it");
        self.line[(self.head + self.line.len() - kappa) % self.line.len()]
    }

    fn push(&mut self, value: f64) {
        if self.line.is_empty() {
            return;
        }
        self.line[self.head] = value;
        self.head = (self.head + 1) % self.line.len();
    }

    fn reset(&mut self) {
        self.line.fill(0.0);
        self.head = 0;
    }
}

/// The precoder filter of 6.4.2:
/// x(n) = u(n) + sum over kappa = 1..LZ1 of u(n-kappa) z1(kappa)
/// + sum over kappa = 1..LP1 of x(n-kappa) p1(kappa).
///
/// It is split into [`Precoder::tail`] and [`Precoder::accept`] rather than
/// offered as one step, because the point selection sits between the two: the
/// tail is c(n), the part of x(n) the current symbol has no say in, and the
/// whole choice is which level to add to it.
#[derive(Debug, Clone, Default)]
pub struct Precoder {
    /// z1(1) .. z1(LZ1); `z1[0]` is z1(1). The feed-forward section starts at
    /// kappa = 1, because u(n) itself enters with weight 1 (pitfall P-5).
    z1: Vec<f64>,
    /// p1(1) .. p1(LP1); `p1[0]` is p1(1).
    p1: Vec<f64>,
    u_back: Delay,
    x_back: Delay,
}

impl Precoder {
    /// The precoder these coefficients describe, every memory zero.
    pub fn new(filters: &Filters) -> Self {
        Self {
            z1: filters.z1.clone(),
            p1: filters.p1.clone(),
            u_back: Delay::new(filters.z1.len()),
            x_back: Delay::new(filters.p1.len()),
        }
    }

    /// c(n): everything in x(n) that the symbol about to be chosen does not
    /// contribute. Choosing the level nearest -c(n) is choosing the smallest
    /// |x(n)|.
    pub fn tail(&self) -> f64 {
        let feed_forward: f64 =
            self.z1.iter().enumerate().map(|(index, &z)| z * self.u_back.back(index + 1)).sum();
        let feedback: f64 = self.p1.iter().enumerate().map(|(index, &p)| p * self.x_back.back(index + 1)).sum();
        feed_forward + feedback
    }

    /// Clock the chosen level and the x(n) it produced into the memories.
    pub fn accept(&mut self, u: f64, x: f64) {
        self.u_back.push(u);
        self.x_back.push(x);
    }

    /// "The precoder ... memories are initialized to zero prior to
    /// transmitting B1u" (8.7.1).
    pub fn reset(&mut self) {
        self.u_back.reset();
        self.x_back.reset();
    }
}

/// The prefilter of 6.4.2:
/// v(n) = sum over kappa = 0..LZ2-1 of x(n-kappa) z2(kappa)
/// + sum over kappa = 1..LP2 of v(n-kappa) p2(kappa).
///
/// Its feed-forward section starts at kappa = 0 and runs to LZ2 - 1, so z2(0)
/// multiplies the current x(n), while the precoder's starts at kappa = 1. The
/// two indexings differ by one and the coefficient counts follow them, which is
/// pitfall P-5 and the reason the two sections are not one piece of code.
#[derive(Debug, Clone, Default)]
pub struct Prefilter {
    /// z2(0) .. z2(LZ2-1); `z2[0]` is z2(0), the tap on the current symbol.
    z2: Vec<f64>,
    /// p2(1) .. p2(LP2); `p2[0]` is p2(1).
    p2: Vec<f64>,
    x_back: Delay,
    v_back: Delay,
}

impl Prefilter {
    /// The prefilter these coefficients describe, every memory zero.
    pub fn new(filters: &Filters) -> Self {
        Self {
            z2: filters.z2.clone(),
            p2: filters.p2.clone(),
            // z2 reaches back to LZ2 - 1, because z2(0) is the current sample.
            x_back: Delay::new(filters.z2.len().saturating_sub(1)),
            v_back: Delay::new(filters.p2.len()),
        }
    }

    /// v(n) from x(n), clocking the memories on.
    pub fn step(&mut self, x: f64) -> f64 {
        let feed_forward: f64 = self
            .z2
            .iter()
            .enumerate()
            .map(|(kappa, &z)| z * if kappa == 0 { x } else { self.x_back.back(kappa) })
            .sum();
        let feedback: f64 = self.p2.iter().enumerate().map(|(index, &p)| p * self.v_back.back(index + 1)).sum();
        let v = feed_forward + feedback;
        self.x_back.push(x);
        self.v_back.push(v);
        v
    }

    /// "The ... prefilter memories are initialized to zero prior to
    /// transmitting B1u" (8.7.1).
    pub fn reset(&mut self) {
        self.x_back.reset();
        self.v_back.reset();
    }
}

// ---------------------------------------------------------------------------
// The chain
// ---------------------------------------------------------------------------

/// What one symbol through the chain produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Symbol {
    /// n, counted from the first symbol of B1u, which "corresponds to n = 0 in
    /// the prefilter and precoder filter output equations in 6.4.2" (8.7.1).
    pub n: u64,
    /// y(n): "the index of the constellation point u(n)" (6.4.2), the number
    /// the digital modem's Viterbi decoder has to recover.
    pub eta: i32,
    /// u(n), the level itself.
    pub u: f64,
    /// x(n), the precoder output. Never saturated (P-12).
    pub x: f64,
    /// v(n), the prefilter output. Never saturated either.
    pub v: f64,
    /// G x v(n), bounded by [`OUTPUT_LIMIT`]. In the units of 8.8.3 and 3.8,
    /// where a mean square of 1 is the desired transmit power, so the line
    /// sample is LU times this.
    pub out: f64,
    /// The Y0 the convolutional encoder held over this symbol's trellis frame.
    /// It is the same for all four symbols of the frame, and the parity of
    /// their four indices equals it.
    pub y0: bool,
}

/// The whole of 6.4.2 to 6.4.4, from the moduli down to G x v(n).
///
/// Built from [`Parameters`] and never from a `Cpd` (AD-3), so it exists long
/// before the wire format does and the design can run it to find G.
#[derive(Debug, Clone)]
pub struct Chain {
    moduli: [u8; UP_INTERVALS],
    /// One entry per set CPd carried, in the order CPd carried them.
    sets: Vec<Constellation>,
    /// Which of those each constellation frame index j = 0..5 uses.
    indices: [u8; CONSTELLATION_FRAME],
    gain: f64,
    code: Code,
    precoder: Precoder,
    prefilter: Prefilter,
    /// The convolutional encoder's state. "There is an inherent delay of one
    /// 4D interval", so this holds the inputs of every frame before the
    /// current one.
    state: u8,
    /// Y0 for the trellis frame in progress, read once at k = 0.
    y0: bool,
    /// The indices chosen so far in the trellis frame in progress.
    etas: [i32; TRELLIS_FRAME],
    n: u64,
}

impl Chain {
    /// The chain these parameters describe, every memory zero and n = 0, which
    /// is the state 8.7.1 requires before B1u.
    ///
    /// The checks here are the chain's own and no more: every interval must
    /// have a constellation, the prefilter must have a feed-forward section or
    /// v(n) is identically zero, and every equivalence class must have a
    /// member. They deliberately stop short of [`Parameters::fits`], which is
    /// the wider check a CPd passes before it goes on the wire, because the
    /// digital modem's design runs this chain with a *trial* gain to find the G
    /// that makes the mean square of G x v(n) equal to 1 (8.8.3), and a trial
    /// gain is not one Table 30's unsigned Q0.16 field could carry.
    pub fn new(parameters: &Parameters) -> Result<Self, &'static str> {
        if parameters.filters.z2.is_empty() {
            return Err("the prefilter has no feed-forward section");
        }
        for i in 0..UP_INTERVALS {
            let Some(points) = parameters.set_for(i) else {
                return Err("a constellation index points past the sets");
            };
            if points.is_empty() {
                return Err("a constellation set is empty");
            }
            if parameters.modulus(i) == 0 {
                return Err("an interval has no modulus");
            }
            // 6.4.2 states neither rule. A class steps by Mi, or by 2 x Mi at
            // k = 3, so it has a member inside -N/2..N/2 for every Ki only if
            // N is at least that step.
            let levels = 2 * points.len();
            let step = usize::from(parameters.modulus(i)) * if i % TRELLIS_FRAME == TRELLIS_FRAME - 1 { 2 } else { 1 };
            if levels < step {
                return Err("an interval's constellation is too small for its modulus");
            }
        }
        Ok(Self {
            moduli: parameters.moduli,
            sets: parameters.sets.iter().map(|points| Constellation::new(points)).collect(),
            indices: parameters.indices,
            gain: parameters.gain,
            code: code_for(parameters.trellis),
            precoder: Precoder::new(&parameters.filters),
            prefilter: Prefilter::new(&parameters.filters),
            state: 0,
            y0: false,
            etas: [0; TRELLIS_FRAME],
            n: 0,
        })
    }

    /// One Ki in, one symbol out.
    ///
    /// The interval, the constellation frame index and the trellis position all
    /// come from n, which counts from the first symbol of B1u (Figure 1 with
    /// 8.7.1), so the caller hands over nothing but the number the modulus
    /// encoder produced.
    pub fn step(&mut self, ki: u8) -> Symbol {
        let n = self.n;
        let interval = super::interval(n);
        let k = super::trellis_index(n);
        if k == 0 {
            // "Y0(m) does not depend on the current frame's inputs": the state
            // is not clocked until the frame's four points have been chosen, so
            // reading its output here and using it at k = 3 is the same thing.
            self.y0 = self.code.output(self.state);
            self.etas = [0; TRELLIS_FRAME];
        }
        // P-4: the parity is over the indices actually chosen in this frame,
        // and the modulo 2 is non-negative because the sum can be negative.
        let parity = k == TRELLIS_FRAME - 1
            && (self.etas[0] + self.etas[1] + self.etas[2] + i32::from(self.y0)).rem_euclid(2) == 1;
        let class = Class::for_interval(ki, self.moduli[interval], k, parity);
        let set = &self.sets[usize::from(self.indices[super::constellation_index(n)])];

        let tail = self.precoder.tail();
        let eta = set
            .choose(class, -tail)
            .expect("Chain::new proved every equivalence class has a member in its constellation");
        let u = set.at(eta);
        let x = u + tail;
        self.precoder.accept(u, x);
        let v = self.prefilter.step(x);

        self.etas[k] = eta;
        if k == TRELLIS_FRAME - 1 {
            // 6.4.4: the inverse map's four bits into the code, clocked once
            // per trellis frame. The mask is what each code actually reads --
            // 16 states ignore Y3 and Y4, 32 states ignore Y3.
            let y = inverse_map(self.etas) & self.code.inputs();
            self.state = self.code.next(self.state, y);
        }
        self.n += 1;
        Symbol { n, eta, u, x, v, out: (self.gain * v).clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT), y0: self.y0 }
    }

    /// Back to the state before B1u: "the ... convolutional encoder, precoder
    /// and prefilter memories are initialized to zero" (8.7.1), and the symbol
    /// count with them, because "the first symbol of B1u shall begin data frame
    /// interval 0".
    pub fn reset(&mut self) {
        self.precoder.reset();
        self.prefilter.reset();
        self.state = 0;
        self.y0 = false;
        self.etas = [0; TRELLIS_FRAME];
        self.n = 0;
    }

    /// n: how many symbols have gone through since the last reset, which is
    /// also the n of the symbol about to be produced.
    pub fn symbols(&self) -> u64 {
        self.n
    }

    /// The data frame interval the next symbol falls in, i = n mod 12.
    pub fn interval(&self) -> usize {
        super::interval(self.n)
    }

    /// The trellis position the next symbol falls in, k = n mod 4.
    pub fn trellis_position(&self) -> usize {
        super::trellis_index(self.n)
    }

    /// The indices chosen so far in the trellis frame in progress. The array is
    /// cleared at k = 0, so positions at or after [`Chain::trellis_position`]
    /// are zero rather than the last frame's.
    pub fn frame_indices(&self) -> [i32; TRELLIS_FRAME] {
        self.etas
    }

    /// The constellation data frame interval `i` uses, by way of j = i mod 6.
    pub fn constellation(&self, i: usize) -> &Constellation {
        &self.sets[usize::from(self.indices[i % CONSTELLATION_FRAME])]
    }

    /// G, the gain at the prefilter output.
    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// The convolutional code in use, for a decoder that has to match it.
    pub fn code(&self) -> Code {
        self.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic source, so the tests do not pull in a crate for it.
    struct Random(u64);

    impl Random {
        fn new(seed: u64) -> Self {
            Self(seed | 1)
        }

        fn next(&mut self) -> u64 {
            // xorshift64*, enough for choosing Ki at random.
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, limit: u8) -> u8 {
            (self.next() % u64::from(limit.max(1))) as u8
        }
    }

    /// `count` positive levels spaced by two: 1, 3, 5, ... as a codec's own
    /// levels would be spaced, and the shape [`Constellation`] expects.
    fn ladder(count: usize) -> Vec<u16> {
        (0..count).map(|index| (2 * index + 1) as u16).collect()
    }

    /// Parameters with one constellation set, every interval on modulus `m`,
    /// and the filters given.
    fn parameters(set: Vec<u16>, m: u8, filters: Filters, gain: f64) -> Parameters {
        Parameters {
            drn: 1,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            gain,
            moduli: [m; UP_INTERVALS],
            filters,
            sets: vec![set],
            indices: [0; CONSTELLATION_FRAME],
        }
    }

    /// A prefilter that passes x straight through: z2 = [1], nothing else.
    fn flat() -> Filters {
        Filters { z2: vec![1.0], ..Filters::default() }
    }

    /// 6.4.2 with nothing in the way: x(n) = u(n), v(n) = x(n), and the output
    /// is G x v(n) with G = 1, so every symbol is the level the class chose.
    ///
    /// The set's levels are small on purpose. G = 1 only puts the chain in
    /// 8.8.3's units -- where the mean square of G x v(n) is 1 -- if the levels
    /// themselves are already in them, and [`OUTPUT_LIMIT`] is a bound in those
    /// units.
    #[test]
    fn with_no_filters_the_output_is_the_chosen_level() {
        let mut chain = Chain::new(&parameters(vec![1, 2, 3, 4], 4, flat(), 1.0)).expect("the parameters are feasible");
        let mut random = Random::new(7);
        for _ in 0..600 {
            let interval = chain.interval();
            let k = chain.trellis_position();
            let ki = random.below(4);
            let symbol = chain.step(ki);
            assert_eq!(symbol.x, symbol.u, "x(n) is u(n) when the precoder has no taps");
            assert_eq!(symbol.v, symbol.x, "v(n) is x(n) when z2 is the single tap 1");
            assert_eq!(symbol.out, symbol.u, "G = 1, so the output is the level itself");
            let set = chain.constellation(interval);
            assert_eq!(set.level(symbol.eta), Some(symbol.u), "the level is a(eta)");
            assert_eq!(index_to_ki(symbol.eta, 4, k), ki, "the index is in E(Ki)");
        }
    }

    /// Pitfall P-5: "v(n) = sum over kappa = 0..LZ2-1 of x(n-kappa) z2(kappa)"
    /// begins at kappa = 0, while "x(n) = u(n) + sum over kappa = 1..LZ1 of
    /// u(n-kappa) z1(kappa)" begins at kappa = 1 because u(n) is already there
    /// with weight 1 (6.4.2).
    #[test]
    fn the_prefilter_feed_forward_starts_at_kappa_0_and_the_precoder_s_at_1() {
        // One z2 tap uses the current x, so v(0) is not zero.
        let mut now = Chain::new(&parameters(ladder(16), 1, flat(), 1.0)).expect("feasible");
        let first = now.step(0);
        assert_eq!(first.v, first.x, "z2(0) multiplies x(n) itself");
        assert_ne!(first.v, 0.0, "the prefilter answers on its very first symbol");

        // Two z2 taps with the first zero delay by exactly one symbol.
        let delayed = Filters { z2: vec![0.0, 1.0], ..Filters::default() };
        let mut later = Chain::new(&parameters(ladder(16), 1, delayed, 1.0)).expect("feasible");
        let one = later.step(0);
        let two = later.step(0);
        assert_eq!(one.v, 0.0, "z2(1) has nothing behind it on the first symbol");
        assert_eq!(two.v, one.x, "and on the second it is x(n-1)");

        // One z1 tap is z1(1), so it contributes nothing on the first symbol.
        let feed_forward = Filters { z1: vec![0.5], z2: vec![1.0], ..Filters::default() };
        let mut precoded = Chain::new(&parameters(ladder(16), 1, feed_forward, 1.0)).expect("feasible");
        let one = precoded.step(0);
        assert_eq!(one.x, one.u, "z1 starts at kappa = 1, so u(n) stands alone at n = 0");
        let two = precoded.step(0);
        assert!(
            (two.x - (two.u + 0.5 * one.u)).abs() < 1e-12,
            "x(1) = u(1) + z1(1) u(0): {} against {}",
            two.x,
            two.u + 0.5 * one.u
        );
    }

    /// 6.4.2 with 6.4.4: the k = 3 class is 2Ki + 2z Mi + (eta0 + eta1 + eta2 +
    /// Y0) mod 2, so the four indices of a trellis frame sum to Y0 modulo 2,
    /// and Y0 is what a separate copy of the code produces from the inverse
    /// map's bits (INTRO 9.5).
    #[test]
    fn the_four_indices_of_a_trellis_frame_have_the_parity_the_encoder_asked_for() {
        for trellis in [Trellis::Sixteen, Trellis::ThirtyTwo, Trellis::SixtyFour] {
            let mut settings = parameters(ladder(16), 8, flat(), 1.0);
            settings.trellis = trellis;
            let mut chain = Chain::new(&settings).expect("feasible");
            let mut random = Random::new(11);
            // A separate encoder, clocked only by the indices the chain chose.
            let code = code_for(trellis);
            let mut state = 0u8;
            for frame in 0..5000 {
                let expected = code.output(state);
                let mut etas = [0i32; TRELLIS_FRAME];
                for eta in &mut etas {
                    let symbol = chain.step(random.below(8));
                    assert_eq!(symbol.y0, expected, "frame {frame} of the {trellis:?} code disagrees on Y0");
                    *eta = symbol.eta;
                }
                let sum = etas.iter().sum::<i32>().rem_euclid(2) == 1;
                assert_eq!(sum, expected, "frame {frame}: the indices {etas:?} do not carry Y0");
                state = code.next(state, inverse_map(etas) & code.inputs());
            }
        }
    }

    /// Figure 9/V.34 through 6.4.3's 2 x y(k) + 1: a label's low bit is the
    /// parity of the two indices, which is what lets the k = 3 parity rule
    /// stand in for V.34's modulo encoder (INTRO 6.4.3, checked over +/-130).
    #[test]
    fn a_label_s_low_bit_is_the_parity_of_the_index_sum() {
        for first in -130..=130 {
            for second in -130..=130 {
                let label = label_of(first, second);
                assert_eq!(
                    u32::from(label & 1),
                    (first + second).rem_euclid(2) as u32,
                    "the label of ({first}, {second}) is {label:03b}"
                );
            }
        }
    }

    /// Pitfall P-12: the precoder is not a modulo precoder, so a feedback
    /// section keeps it bounded only because the class choice spends its
    /// freedom on that. With a real p1 the excursion stays inside the
    /// constellation's own top level.
    #[test]
    fn the_precoder_output_stays_bounded_over_a_long_run() {
        let filters = Filters { p1: vec![0.6, -0.2, 0.1], z2: vec![1.0], ..Filters::default() };
        let mut chain = Chain::new(&parameters(ladder(16), 8, filters, 1.0)).expect("feasible");
        let mut random = Random::new(13);
        let top = 31.0; // P[15] of the test set: levels 1, 3, ... 31.
        let mut worst = 0.0f64;
        for _ in 0..100_000 {
            worst = worst.max(chain.step(random.below(8)).x.abs());
        }
        // The mechanism, and why the bound is well under the top level: the
        // chosen member is the one nearest -c(n), so |x(n)| cannot exceed half
        // the spacing between class members as long as the class has a member
        // either side of -c(n). The widest class here is the k = 3 one, 2 x Mi
        // = 16 indices apart, and the levels step by two, so half the spacing
        // is 16 -- which is what the run measures, to six figures.
        assert!(worst < 0.6 * top, "the precoder output reached {worst}, over 0.6 of the top level {top}");
        assert!(worst > 0.4 * top, "the precoder output only reached {worst}: the run is not exercising it");
    }

    /// 6.4.2 states neither rule, but a class with no member inside
    /// -N/2..N/2 cannot be transmitted: N must be at least Mi, and at k = 3,
    /// where the class steps by 2 x Mi, at least 2 x Mi. Reported, never
    /// panicked on.
    #[test]
    fn a_class_with_no_member_is_reported() {
        // N = 8 against Mi = 9: no member even at k = 0.
        let too_small = parameters(ladder(4), 9, flat(), 1.0);
        assert_eq!(
            Chain::new(&too_small).unwrap_err(),
            "an interval's constellation is too small for its modulus"
        );

        // N = 8 is enough at k = 0, 1, 2 with Mi = 8 but not at k = 3, where
        // the step is 16. Only intervals 3, 7 and 11 are short.
        let mut edge = parameters(ladder(4), 8, flat(), 1.0);
        assert_eq!(
            Chain::new(&edge).unwrap_err(),
            "an interval's constellation is too small for its modulus",
            "N = 2 x Mi is required at k = 3"
        );
        // Halving the modulus of the k = 3 intervals alone makes it feasible.
        for i in [3, 7, 11] {
            edge.moduli[i] = 4;
        }
        assert!(Chain::new(&edge).is_ok(), "N = 8 carries Mi = 8 at k < 3 and Mi = 4 at k = 3");

        // A set index past the sets, and a prefilter that would make v zero.
        let mut astray = parameters(ladder(16), 8, flat(), 1.0);
        astray.indices[2] = 3;
        assert_eq!(Chain::new(&astray).unwrap_err(), "a constellation index points past the sets");
        let silent = parameters(ladder(16), 8, Filters::default(), 1.0);
        assert_eq!(Chain::new(&silent).unwrap_err(), "the prefilter has no feed-forward section");
    }

    /// The digital modem's half of 6.4.2, all the way back: run both filters
    /// backwards from v(n) to recover x(n) and then u(n), read off the index,
    /// and take Ki from it -- eta modulo Mi at k = 0, 1, 2 and
    /// ((eta - p) / 2) modulo Mi at k = 3, where p is eta's own parity.
    ///
    /// This is the shape of the real receiver (CD 7.7 steps 3 and 4) and it
    /// catches what a forward-only test cannot: an off-by-one in either
    /// feed-forward section would leave the inverse unable to find the levels
    /// again, which is pitfall P-5's whole point.
    #[test]
    fn an_inverse_channel_gives_back_every_k_as_eta_mod_m() {
        let filters = Filters { z1: vec![0.2], p1: vec![0.5, -0.25], z2: vec![1.0, 0.3], p2: vec![0.1] };
        let moduli = [7u8, 8, 9, 6, 5, 8, 8, 4, 9, 9, 7, 3];
        let mut settings = parameters(ladder(16), 8, filters.clone(), 1.0);
        settings.moduli = moduli;
        let mut chain = Chain::new(&settings).expect("feasible");
        let mut random = Random::new(17);

        // The receiver's own memories, most recent first. It is handed v(n)
        // alone and everything else is what it has worked out for itself.
        let back = |history: &Vec<f64>, kappa: usize| history.get(kappa - 1).copied().unwrap_or(0.0);
        let keep = |history: &mut Vec<f64>, value: f64| {
            history.insert(0, value);
            history.truncate(4);
        };
        let (mut u_back, mut x_back, mut v_back) = (Vec::new(), Vec::new(), Vec::new());

        for _ in 0..20_000 {
            let interval = chain.interval();
            let k = chain.trellis_position();
            let ki = random.below(moduli[interval]);
            let symbol = chain.step(ki);

            // Undo the prefilter. z2(0) is the only tap on the current x(n),
            // so everything else moves to the other side and is divided out.
            let known: f64 = filters.z2.iter().enumerate().skip(1).map(|(kappa, &z)| z * back(&x_back, kappa)).sum::<f64>()
                + filters.p2.iter().enumerate().map(|(index, &p)| p * back(&v_back, index + 1)).sum::<f64>();
            let x = (symbol.v - known) / filters.z2[0];
            // Undo the precoder: u(n) = x(n) - c(n), with c(n) built from the
            // recovered history rather than from the transmitter's.
            let c: f64 = filters.z1.iter().enumerate().map(|(index, &z)| z * back(&u_back, index + 1)).sum::<f64>()
                + filters.p1.iter().enumerate().map(|(index, &p)| p * back(&x_back, index + 1)).sum::<f64>();
            let u = x - c;

            // Slice u back to an index, as the receiver's decisions do.
            let set = chain.constellation(interval);
            let eta = (set.lowest()..=set.highest())
                .min_by(|&a, &b| {
                    let distance = |eta: i32| (set.level(eta).expect("in range") - u).abs();
                    distance(a).total_cmp(&distance(b))
                })
                .expect("the set is not empty");
            assert_eq!(eta, symbol.eta, "the inverse channel lost the index at interval {interval}, k = {k}");
            assert_eq!(
                index_to_ki(eta, moduli[interval], k),
                ki,
                "interval {interval} at k = {k}: index {eta} does not come back as Ki = {ki}"
            );

            keep(&mut u_back, set.level(eta).expect("in range"));
            keep(&mut x_back, x);
            keep(&mut v_back, symbol.v);
        }
    }

    /// 8.7.1: "the scrambler, modulus encoder, convolutional encoder, precoder
    /// and prefilter memories are initialized to zero prior to transmitting
    /// B1u", so the first data frame of B1u is the same twelve symbols every
    /// time. The vector is pinned: a change to what the memories start at
    /// would otherwise pass unnoticed until a live call.
    #[test]
    fn the_memories_are_zero_before_b1u() {
        let filters = Filters { z1: vec![0.2], p1: vec![0.5, -0.25], z2: vec![1.0, 0.3], p2: vec![0.1] };
        let mut chain = Chain::new(&parameters(ladder(16), 4, filters, 1.0)).expect("feasible");
        // Ki = 1 in every interval stands for B1u's scrambled ones reaching the
        // modulus encoder: what matters is that the run is the same every time.
        // v(n) is pinned rather than G x v(n), because v(n) is the last filter
        // memory and nothing clamps it.
        let run = |chain: &mut Chain| (0..UP_INTERVALS).map(|_| chain.step(1).v).collect::<Vec<_>>();

        let first = run(&mut chain);
        // Worked through by hand for the first two: at n = 0 every memory is
        // zero, so c(0) = 0, the class {1, -3, -7, ...} puts a(1) = 3 nearest
        // it, x(0) = 3 and v(0) = z2(0) x(0) = 3. At n = 1, c(1) = 0.2 u(0) +
        // 0.5 x(0) = 2.1, the class member nearest -2.1 is a(-3) = -5, so
        // x(1) = -2.9 and v(1) = -2.9 + 0.3 x(0) + 0.1 v(0) = -1.7.
        let pinned = [
            3.0,
            -1.7,
            -1.24,
            -7.959,
            -5.7659,
            -5.14284,
            -0.874909,
            -6.9262409,
            -5.34184284,
            -5.189106159,
            -1.0040668659,
            -6.97925434284,
        ];
        for (got, want) in first.iter().zip(pinned.iter()) {
            assert!((got - want).abs() < 1e-9, "the first twelve prefilter outputs after a reset: {first:?}");
        }

        // Running on and resetting reproduces it exactly.
        for _ in 0..500 {
            chain.step(2);
        }
        chain.reset();
        assert_eq!(run(&mut chain), first, "a reset puts every memory back to zero");
        assert_eq!(chain.symbols(), UP_INTERVALS as u64, "and n counts from zero again");
    }

    /// P-12: "keep x and v in f64 ... and saturate the final D/A value, but
    /// never the filter state". A gain far too large clamps the output and
    /// leaves x(n) and v(n) exactly where a sane gain would have left them.
    #[test]
    fn the_state_is_never_saturated_only_the_output() {
        let filters = Filters { p1: vec![0.6, -0.2], z2: vec![1.0, 0.25], ..Filters::default() };
        let mut quiet = Chain::new(&parameters(ladder(16), 8, filters.clone(), 0.01)).expect("feasible");
        let mut loud = Chain::new(&parameters(ladder(16), 8, filters, 1000.0)).expect("feasible");
        let mut random = Random::new(19);
        let mut clamped = 0;
        for _ in 0..5000 {
            let ki = random.below(8);
            let soft = quiet.step(ki);
            let hard = loud.step(ki);
            assert_eq!(soft.eta, hard.eta, "the gain must not change which point is chosen");
            assert_eq!(soft.x, hard.x, "x(n) is the same whatever G is");
            assert_eq!(soft.v, hard.v, "and so is v(n)");
            assert_eq!(soft.out, 0.01 * soft.v, "a small gain never reaches the limit");
            assert!(hard.out.abs() <= OUTPUT_LIMIT, "the output is bounded");
            if hard.v.abs() > 0.0 {
                clamped += 1;
                assert_eq!(hard.out, (1000.0 * hard.v).clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT));
            }
        }
        assert!(clamped > 4000, "only {clamped} symbols exercised the clamp");
    }

    /// 6.4.2's two class forms, written out: the step is Mi at k = 0, 1, 2 and
    /// 2 x Mi at k = 3, and the k = 3 base carries the parity bit.
    #[test]
    fn the_fourth_symbol_s_class_steps_by_twice_the_modulus() {
        for k in 0..TRELLIS_FRAME {
            let class = Class::for_interval(3, 7, k, true);
            if k == TRELLIS_FRAME - 1 {
                assert_eq!(class.spacing(), 14, "k = 3 steps by 2 x Mi");
                assert_eq!(class.index(0), 7, "2 x Ki + parity");
                assert!(class.holds(7 - 14) && class.holds(7 + 14));
                assert!(!class.holds(8), "the parity fixes which half the index is in");
            } else {
                assert_eq!(class.spacing(), 7, "k = {k} steps by Mi");
                assert_eq!(class.index(0), 3, "Ki itself, with the parity ignored");
                assert!(class.holds(3 - 7) && class.holds(3 + 7));
            }
        }
    }

    /// [`TIE_TO_SMALLER_INDEX`]: with c(n) = 0 the two members a(-1) and a(0)
    /// are equidistant, and the rule takes the smaller magnitude.
    #[test]
    fn a_tie_goes_to_the_smaller_index() {
        let set = Constellation::new(&[1, 3, 5, 7]);
        // Mi = 1, so every index is in the class and -1 and 0 both sit at
        // distance P[0] from a target of zero.
        let every = Class::for_interval(0, 1, 0, false);
        assert_eq!(set.choose(every, 0.0), Some(if TIE_TO_SMALLER_INDEX { 0 } else { -1 }));
        // Away from the tie the nearest level wins outright.
        assert_eq!(set.choose(every, 2.9), Some(1), "a(1) = 3 is nearest 2.9");
        assert_eq!(set.choose(every, -6.0), Some(-3), "a(-3) = -5 is nearer than a(-4) = -7");
        assert_eq!(set.choose(every, 900.0), Some(3), "a target past the top clamps to it");
        assert_eq!(set.choose(every, -900.0), Some(-4), "and past the bottom to -N/2");
    }

    /// 6.4.2's indexing: "negative points have negative indices, and positive
    /// points have non-negative indices", with a(-eta-1) = -a(eta).
    #[test]
    fn the_levels_rise_with_the_index_across_the_mirror() {
        let set = Constellation::new(&[2, 6, 11]);
        assert_eq!(set.positive_points(), 3);
        assert_eq!(set.levels(), 6);
        assert_eq!(set.lowest(), -3);
        assert_eq!(set.highest(), 2);
        assert_eq!(set.level(-4), None, "there is no index below -N/2");
        assert_eq!(set.level(3), None, "nor one at N/2");
        let levels: Vec<f64> = (-3..=2).map(|eta| set.level(eta).expect("in range")).collect();
        assert_eq!(levels, vec![-11.0, -6.0, -2.0, 2.0, 6.0, 11.0]);
        assert!(levels.windows(2).all(|pair| pair[0] < pair[1]), "the levels rise with the index");
    }

    /// Table 30 bits 27:28 pick the code, and 6.4.4 clocks it once per trellis
    /// frame rather than once per 2D symbol. The three codes are V.34's own.
    #[test]
    fn the_code_is_the_one_cpd_bits_27_and_28_named() {
        assert_eq!(code_for(Trellis::Sixteen), Code::States16);
        assert_eq!(code_for(Trellis::ThirtyTwo), Code::States32);
        assert_eq!(code_for(Trellis::SixtyFour), Code::States64);
        let mut settings = parameters(ladder(16), 8, flat(), 1.0);
        settings.trellis = Trellis::SixtyFour;
        let chain = Chain::new(&settings).expect("feasible");
        assert_eq!(chain.code(), Code::States64);
        assert_eq!(chain.code().states(), 64);
    }
}
