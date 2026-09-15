//! Data mode (clauses 7 to 9): bits to points, and points back to bits.
//!
//! Going out, a mapping frame's bits are scrambled and split (9.3): K of them
//! choose eight rings through the shell mapper (9.4), and each of the frame's
//! four 4D symbols takes three more -- I1, I2 and I3 -- and 2q that pick a
//! point within each of its two 2D symbols' rings. I2 and I3 turn both points
//! by a differentially encoded number of quarters (9.5). I1 turns the second
//! point half a revolution more, and the trellis encoder's bit U0 a quarter
//! more again (9.6.1). The precoder moves each point by a multiple of 2 or 4
//! so that what arrives through the far end's channel is back on the grid
//! (9.6.2), and the non-linear encoder bends the outer points out (9.7).
//!
//! Table 11 fixes the order of all that inside a 4D symbol, because each step
//! needs something the one before it made: the precoder's correction for the
//! second point is known before the trellis bit is, and the trellis bit
//! before the second point can be turned.
//!
//! Coming in, a Viterbi decoder picks the most likely sequence of channel
//! output points under the trellis code, and everything above is undone in
//! reverse. The trellis code's bit is read from the labels alone: the first
//! point's low label bits are its quarter turn Z, and the parity of the two
//! points' low bits is Y0 with the superframe's bit inversion on it (see
//! `trellis`).

use std::collections::VecDeque;

use dsp::Complex;

use super::constellation::{Point, QUARTER, clockwise, counterclockwise, quarter};
use super::frame::Framing;
use super::mp::Coefficient;
use super::shell::Shell;
use super::trellis::{self, Code};
use crate::v32::{Mode, Scrambler};

/// Theta of 9.7 when non-linear encoding is asked for.
pub const THETA: f64 = 0.3125;

/// 4D symbols the Viterbi decoder looks back over before deciding.
const DEPTH: usize = 40;

/// Everything one direction's data mode is set by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub framing: Framing,
    pub code: Code,
    pub nonlinear: bool,
    /// h(1), h(2), h(3), as MP carries them: 14 bits after the point.
    pub precoding: [Coefficient; 3],
    /// The transmitting end's scrambler.
    pub mode: Mode,
}

impl Params {
    fn theta(&self) -> f64 {
        if self.nonlinear { THETA } else { 0.0 }
    }
}

/// The average energy of the points the mapper makes, in grid units squared,
/// and of the same after the non-linear encoder: over the rings as often as
/// the shell mapper uses them, and the points within each ring evenly.
///
/// Which is what keeps data mode at the power of TRN, as 10.1.3 asks: "the
/// average signal power transmitted in Phases 3 and 4 is maintained in segment
/// B1 and the subsequent data mode". A low mapping frame gives the shell mapper
/// a zero for its top bit (9.3.1), and so only the cheaper half of the
/// combinations -- at 33 600 nine frames in fifteen are low, and counting them
/// as high left data mode 7.5% short of TRN's power.
fn energies(params: &Params, shell: &Shell) -> (f64, f64) {
    let f = &params.framing;
    let shares = if f.k > 0 {
        let high = f.r as f64 / f.p as f64;
        shell
            .ring_shares(f.k)
            .iter()
            .zip(shell.ring_shares(f.k - 1))
            .map(|(h, l)| high * h + (1.0 - high) * l)
            .collect()
    } else {
        vec![1.0]
    };
    let per_ring = 1usize << f.q;
    let ring_energy = |ring: usize, bend: &dyn Fn(f64) -> f64| {
        (0..per_ring)
            .map(|n| {
                let (x, y) = quarter((ring * per_ring + n).min(QUARTER - 1));
                bend(f64::from(x * x + y * y))
            })
            .sum::<f64>()
            / per_ring as f64
    };
    let plain: f64 = shares.iter().enumerate().map(|(r, s)| s * ring_energy(r, &|e| e)).sum();
    let theta = params.theta();
    let bent: f64 = shares
        .iter()
        .enumerate()
        .map(|(r, s)| {
            s * ring_energy(r, &|e| {
                let phi = projection(theta * e / plain);
                phi * phi * e
            })
        })
        .sum();
    (plain, bent)
}

/// Phi of 9-34 for zeta.
fn projection(zeta: f64) -> f64 {
    1.0 + zeta / 6.0 + zeta * zeta / 120.0
}

/// The nearest multiple of `step` to `value`, halfway going to the smaller
/// magnitude (9.6.2 items 2 and 3).
fn round_to(value: i64, step: i64) -> i64 {
    let whole = value.abs() / step;
    let rest = value.abs() % step;
    let n = if 2 * rest > step { whole + 1 } else { whole };
    value.signum() * n * step
}

/// Units the precoder works in: 1/128 of a grid unit, so every value 9.6.2
/// makes is a whole number.
const FINE: i64 = 128;

/// The precoder of 9.6.2, in exact arithmetic.
#[derive(Debug, Clone)]
struct Precoder {
    /// h(p) in units of 2^-14.
    h: [(i64, i64); 3],
    /// 2w in fine units.
    step: i64,
    /// x(n - 1), x(n - 2), x(n - 3) in fine units.
    x: [(i64, i64); 3],
}

impl Precoder {
    fn new(params: &Params) -> Self {
        Self {
            h: params.precoding.map(|(re, im)| (i64::from(re), i64::from(im))),
            step: 2 * params.framing.precoder_scale() * FINE,
            x: [(0, 0); 3],
        }
    }

    /// p(n) and c(n) for the next symbol, in fine units.
    fn predict(&self) -> ((i64, i64), (i64, i64)) {
        let mut q = (0i64, 0i64);
        for (x, h) in self.x.iter().zip(&self.h) {
            q.0 += x.0 * h.0 - x.1 * h.1;
            q.1 += x.0 * h.1 + x.1 * h.0;
        }
        // q is in units of 2^-21; p to the nearest 2^-7.
        let p = (round_to(q.0, 1 << 14) >> 14, round_to(q.1, 1 << 14) >> 14);
        let c = (round_to(p.0, self.step), round_to(p.1, self.step));
        (p, c)
    }

    fn push(&mut self, x: (i64, i64)) {
        self.x = [x, self.x[0], self.x[1]];
    }
}

/// V0 for 4D symbol `m` since the start of B1, which is sent as the last data
/// frame of a superframe (10.1.3.1).
fn inversion_at(framing: &Framing, m: u64) -> bool {
    let half = 2 * framing.p as u64;
    if !m.is_multiple_of(half) {
        return false;
    }
    let halves = 2 * framing.j as u64;
    trellis::inversion(framing.j, ((m / half + halves - 2) % halves) as usize)
}

/// A 4D symbol's bits: I1, I2, I3 and the two 2D symbols' q bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Group {
    i1: bool,
    i2: bool,
    i3: bool,
    q: [usize; 2],
}

/// Read `count` bits into a number, first bit least significant.
fn number(bits: &[bool]) -> usize {
    bits.iter().enumerate().fold(0, |n, (i, &b)| n | usize::from(b) << i)
}

/// A mapping frame's bits split as 9.3 says: the shell mapper's K, and four
/// groups.
fn parse(framing: &Framing, high: bool, bits: &[bool]) -> (Vec<bool>, [Group; 4]) {
    let mut groups = [Group::default(); 4];
    if framing.b <= 12 {
        // 9.3.2: 8, 9, 11 or 12 bits, I3 of the later groups left out.
        let with_i3 = match bits.len() {
            12 => 4,
            11 => 3,
            9 => 1,
            _ => 0,
        };
        let mut at = 0;
        for (j, group) in groups.iter_mut().enumerate() {
            group.i1 = bits[at];
            group.i2 = bits[at + 1];
            at += 2;
            if j < with_i3 {
                group.i3 = bits[at];
                at += 1;
            }
        }
        return (Vec::new(), groups);
    }
    let k = framing.k;
    let mut shell_bits: Vec<bool> = if high { bits[..k].to_vec() } else { bits[..k - 1].to_vec() };
    if !high {
        shell_bits.push(false);
    }
    let mut at = if high { k } else { k - 1 };
    let q = framing.q;
    for group in &mut groups {
        group.i1 = bits[at];
        group.i2 = bits[at + 1];
        group.i3 = bits[at + 2];
        group.q = [number(&bits[at + 3..at + 3 + q]), number(&bits[at + 3 + q..at + 3 + 2 * q])];
        at += 3 + 2 * q;
    }
    (shell_bits, groups)
}

/// The same the other way: a mapping frame's bits from its shell bits and
/// groups.
fn unparse(framing: &Framing, high: bool, shell_bits: &[bool], groups: &[Group; 4]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(framing.b);
    if framing.b <= 12 {
        let count = if high { framing.b } else { framing.b - 1 };
        let with_i3 = match count {
            12 => 4,
            11 => 3,
            9 => 1,
            _ => 0,
        };
        for (j, group) in groups.iter().enumerate() {
            bits.push(group.i1);
            bits.push(group.i2);
            if j < with_i3 {
                bits.push(group.i3);
            }
        }
        return bits;
    }
    let k = framing.k;
    bits.extend_from_slice(&shell_bits[..if high { k } else { k - 1 }]);
    for group in groups {
        bits.push(group.i1);
        bits.push(group.i2);
        bits.push(group.i3);
        for side in group.q {
            bits.extend((0..framing.q).map(|i| side >> i & 1 == 1));
        }
    }
    bits
}

/// Data mode going out.
#[derive(Debug, Clone)]
pub struct Encoder {
    params: Params,
    shell: Shell,
    scrambler: Scrambler,
    state: u8,
    z: u32,
    precoder: Precoder,
    /// Mapping frames begun since B1.
    frame: u64,
    ready: VecDeque<Complex>,
    /// E|u|^2, and what a point is multiplied by to leave at unit mean power.
    energy: f64,
    scale: f64,
}

impl Encoder {
    /// From the start of B1: scrambler, trellis encoder, differential encoder
    /// and precoder all at zero (10.1.3.1).
    pub fn new(params: Params) -> Self {
        let shell = Shell::new(params.framing.m);
        let (energy, bent) = energies(&params, &shell);
        Self {
            shell,
            scrambler: Scrambler::new(params.mode),
            state: 0,
            z: 0,
            precoder: Precoder::new(&params),
            frame: 0,
            ready: VecDeque::new(),
            energy,
            scale: 1.0 / bent.sqrt(),
            params,
        }
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Mapping frames begun since B1.
    pub fn mapping_frames(&self) -> u64 {
        self.frame
    }

    /// The next 2D symbol at unit mean power, taking data bits from `source`
    /// whenever a mapping frame begins.
    pub fn next_symbol(&mut self, source: &mut dyn FnMut() -> bool) -> Complex {
        if self.ready.is_empty() {
            self.encode_frame(source);
        }
        self.ready.pop_front().expect("a frame makes eight symbols")
    }

    fn encode_frame(&mut self, source: &mut dyn FnMut() -> bool) {
        let framing = self.params.framing;
        let within = (self.frame % framing.p as u64) as usize;
        let high = framing.high(within);
        let count = framing.bits_in(within);
        let bits: Vec<bool> = (0..count).map(|_| self.scrambler.scramble(source())).collect();
        let (shell_bits, groups) = parse(&framing, high, &bits);
        let rings = if framing.k > 0 { self.shell.map(number(&shell_bits) as u64) } else { [0; 8] };
        let theta = self.params.theta();
        for (j, group) in groups.iter().enumerate() {
            let m = self.frame * 4 + j as u64;
            // 9.5
            self.z = (self.z + u32::from(group.i2) + 2 * u32::from(group.i3)) % 4;
            let point = |side: usize| quarter((group.q[side] + (rings[2 * j + side] << framing.q)).min(QUARTER - 1));
            let (v0, v1) = (point(0), point(1));
            // Table 11, steps 1 to 9.
            let u0 = clockwise(v0, self.z);
            let (p0, c0) = self.precoder.predict();
            let y0 = (i64::from(u0.0) * FINE + c0.0, i64::from(u0.1) * FINE + c0.1);
            let x0 = (y0.0 - p0.0, y0.1 - p0.1);
            self.precoder.push(x0);
            let (p1, c1) = self.precoder.predict();
            let carry = trellis::modulo((c0.0 / FINE, c0.1 / FINE), (c1.0 / FINE, c1.1 / FINE));
            let u_bit = self.params.code.output(self.state) ^ carry ^ inversion_at(&framing, m);
            let turn = (self.z + 2 * u32::from(group.i1) + u32::from(u_bit)) % 4;
            let u1 = clockwise(v1, turn);
            let y1 = (i64::from(u1.0) * FINE + c1.0, i64::from(u1.1) * FINE + c1.1);
            let x1 = (y1.0 - p1.0, y1.1 - p1.1);
            self.precoder.push(x1);
            let grid = |y: (i64, i64)| ((y.0 / FINE) as i32, (y.1 / FINE) as i32);
            let inputs = trellis::convert(trellis::label(grid(y0)), trellis::label(grid(y1))) & self.params.code.inputs();
            self.state = self.params.code.next(self.state, inputs);
            // 9.7, and out at unit power.
            for x in [x0, x1] {
                let point = Complex::new(x.0 as f64 / FINE as f64, x.1 as f64 / FINE as f64);
                let phi = projection(theta * point.norm_sqr() / self.energy);
                self.ready.push_back(point.scale(phi * self.scale));
            }
        }
        self.frame += 1;
    }

    /// What a unit-power symbol is multiplied by to be back in grid units.
    pub fn grid_scale(&self) -> f64 {
        1.0 / self.scale
    }
}

/// The largest coordinate of any point of a framing's constellation, with
/// room for the precoder's corrections of up to 2w either way.
pub fn extent(framing: &Framing) -> i32 {
    let largest = (0..(framing.l / 4).min(QUARTER))
        .map(|n| {
            let (x, y) = quarter(n);
            x.abs().max(y.abs())
        })
        .max()
        .unwrap_or(1);
    largest + 2 * framing.precoder_scale() as i32
}

/// The largest coordinate any point of a direction's constellation reaches,
/// non-linear encoding's stretch of the outer points included, in grid units.
pub fn peak(params: &Params) -> f64 {
    let shell = Shell::new(params.framing.m);
    let (energy, _) = energies(params, &shell);
    let theta = params.theta();
    (0..(params.framing.l / 4).min(QUARTER))
        .map(|n| {
            let (x, y) = quarter(n);
            let phi = projection(theta * f64::from(x * x + y * y) / energy);
            f64::from(x.abs().max(y.abs())) * phi
        })
        .fold(1.0, f64::max)
}

/// The label of each point of the quarter superconstellation, by position.
fn quarter_label(point: Point) -> Option<usize> {
    static LABELS: std::sync::OnceLock<std::collections::HashMap<Point, usize>> = std::sync::OnceLock::new();
    LABELS.get_or_init(|| (0..QUARTER).map(|n| (quarter(n), n)).collect()).get(&point).copied()
}

/// For each label, the nearest odd-grid point carrying it and how far away.
fn nearest_by_label(r: Complex) -> [(Point, f64); 8] {
    let mut best = [((0, 0), f64::MAX); 8];
    let odd = |v: f64| 2 * ((v - 1.0) / 2.0).round() as i32 + 1;
    let (cx, cy) = (odd(r.re), odd(r.im));
    for dx in (-4..=4).step_by(2) {
        for dy in (-4..=4).step_by(2) {
            let p = (cx + dx, cy + dy);
            let d = (f64::from(p.0) - r.re).powi(2) + (f64::from(p.1) - r.im).powi(2);
            let l = trellis::label(p) as usize;
            if d < best[l].1 {
                best[l] = (p, d);
            }
        }
    }
    best
}

/// The label pairs of each 4D subset, by Y0 and the code's input bits: the
/// index is 16 Y0 + Y.
fn subsets(code: Code) -> Vec<Vec<(u8, u8)>> {
    let values = 1usize << 4;
    let mut subsets = vec![Vec::new(); 2 * values];
    for first in 0..8u8 {
        for second in 0..8u8 {
            let y = trellis::convert(first, second) & code.inputs();
            let y0 = usize::from((first ^ second) & 1);
            subsets[y0 * values + y as usize].push((first, second));
        }
    }
    subsets
}

/// A received 2D symbol at unit power, in grid units, with 9.7's stretch taken
/// back out: by fixed-point iteration, since a point bent out by phi of its
/// own energy is brought back by phi of the unbent one's.
fn to_grid(params: &Params, energy: f64, scale: f64, symbol: Complex) -> Complex {
    let point = symbol.scale(scale);
    if !params.nonlinear {
        return point;
    }
    let theta = params.theta();
    let mut x = point;
    for _ in 0..3 {
        x = point.scale(1.0 / projection(theta * x.norm_sqr() / energy));
    }
    x
}

/// For two received 2D symbols, the nearest pair of points in each 4D subset
/// and how far away, by 16 Y0 + Y.
fn branches(subsets: &[Vec<(u8, u8)>], near: &[[(Point, f64); 8]; 2]) -> Vec<(f64, (u8, u8))> {
    subsets
        .iter()
        .map(|pairs| {
            pairs.iter().fold((f64::MAX, (0, 0)), |best, &(a, b)| {
                let d = near[0][a as usize].1 + near[1][b as usize].1;
                if d < best.0 { (d, (a, b)) } else { best }
            })
        })
        .collect()
}

/// One 4D symbol's worth of the decoder's memory.
#[derive(Debug, Clone)]
struct Stage {
    /// For each state reached: where it came from, and the labels of the two
    /// points on that branch.
    back: Vec<(u8, u8, u8)>,
    /// The nearest point of each label to each of the two points received.
    near: [[(Point, f64); 8]; 2],
}

/// Data mode coming in.
#[derive(Debug, Clone)]
pub struct Decoder {
    params: Params,
    shell: Shell,
    descrambler: Scrambler,
    /// Label pairs in each 4D subset, by Y0 with the inversion and the code's
    /// inputs.
    subsets: Vec<Vec<(u8, u8)>>,
    metrics: Vec<f64>,
    stages: VecDeque<Stage>,
    /// 4D symbols received, and decided.
    received: u64,
    decided: u64,
    half: Option<Complex>,
    /// What undoing the transmitter needs as it goes.
    precoder: Precoder,
    z: u32,
    groups: [Group; 4],
    labels: [usize; 8],
    energy: f64,
    scale: f64,
    /// The largest coordinate a point reaches, at unit mean power.
    peak: f64,
    bits: Vec<bool>,
    /// Points the decisions put outside the constellation.
    outside: u64,
    /// The best path's cost per 4D symbol, averaged, in grid units squared.
    cost: f64,
    /// The first 4D symbol of the first whole mapping frame read, where a
    /// decoder picked up part way through a frame starts giving bits.
    whole_from: u64,
}

impl Decoder {
    /// From the start of B1, for a far end sending with `params`.
    pub fn new(params: Params) -> Self {
        let shell = Shell::new(params.framing.m);
        let (energy, bent) = energies(&params, &shell);
        let code = params.code;
        let mut metrics = vec![f64::MAX; code.states()];
        metrics[0] = 0.0;
        Self {
            shell,
            descrambler: Scrambler::new(params.mode),
            subsets: subsets(code),
            metrics,
            stages: VecDeque::new(),
            received: 0,
            decided: 0,
            half: None,
            precoder: Precoder::new(&params),
            z: 0,
            groups: [Group::default(); 4],
            labels: [0; 8],
            energy,
            scale: bent.sqrt(),
            peak: peak(&params) / bent.sqrt(),
            bits: Vec::new(),
            outside: 0,
            cost: 0.0,
            whole_from: 0,
            params,
        }
    }

    /// A decoder picked up part way through data mode, whose next 2D symbol
    /// begins 4D symbol `m` as B1 counts them -- from the start of B1, modulo
    /// a superframe. The trellis state is not known, so every one is as
    /// likely; the bits of the mapping frame `m` falls in, if it does not
    /// start one, are not given.
    pub fn resume(params: Params, m: u64) -> Self {
        let mut decoder = Self::new(params);
        decoder.metrics = vec![0.0; params.code.states()];
        decoder.received = m;
        decoder.decided = m;
        decoder.whole_from = m.next_multiple_of(4);
        decoder
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// What a unit-power symbol is multiplied by to be in grid units.
    pub fn grid_scale(&self) -> f64 {
        self.scale
    }

    /// The largest coordinate a point of the constellation reaches, at the
    /// unit mean power symbols are fed in at.
    pub fn peak(&self) -> f64 {
        self.peak
    }

    /// How far out the constellation's points reach, in grid units.
    pub fn extent(&self) -> i32 {
        extent(&self.params.framing)
    }

    /// What the best path through the trellis has cost a 4D symbol lately, in
    /// grid units squared: the noise, when the code is the one being sent.
    pub fn path_cost(&self) -> f64 {
        self.cost
    }

    /// Decided points that fell outside the constellation.
    pub fn outside(&self) -> u64 {
        self.outside
    }

    /// Bits decoded so far, and taken.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// One received 2D symbol at unit mean power.
    pub fn feed(&mut self, symbol: Complex) {
        let point = to_grid(&self.params, self.energy, self.scale, symbol);
        match self.half.take() {
            None => self.half = Some(point),
            Some(first) => self.viterbi(first, point),
        }
    }

    fn viterbi(&mut self, r0: Complex, r1: Complex) {
        let code = self.params.code;
        let near = [nearest_by_label(r0), nearest_by_label(r1)];
        let inversion = inversion_at(&self.params.framing, self.received);
        // The best pair in each subset.
        let values = 16usize;
        let best = branches(&self.subsets, &near);
        let n = code.states();
        let mut next = vec![f64::MAX; n];
        let mut back = vec![(0u8, 0u8, 0u8); n];
        for state in 0..n {
            if self.metrics[state] == f64::MAX {
                continue;
            }
            let y0 = usize::from(code.output(state as u8) ^ inversion);
            for y in (0..16u8).filter(|y| y & !code.inputs() == 0) {
                let (d, pair) = best[y0 * values + y as usize];
                if d == f64::MAX {
                    continue;
                }
                let to = code.next(state as u8, y) as usize;
                let total = self.metrics[state] + d;
                if total < next[to] {
                    next[to] = total;
                    back[to] = (state as u8, pair.0, pair.1);
                }
            }
        }
        let floor = next.iter().copied().fold(f64::MAX, f64::min);
        self.cost += 0.02 * (floor - self.cost);
        for m in &mut next {
            if *m != f64::MAX {
                *m -= floor;
            }
        }
        self.metrics = next;
        self.stages.push_back(Stage { back, near });
        self.received += 1;
        if self.stages.len() > DEPTH {
            self.decide_oldest();
        }
    }

    /// Trace the best path back to the oldest 4D symbol held, and decide it.
    fn decide_oldest(&mut self) {
        let mut state = self
            .metrics
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(s, _)| s)
            .unwrap_or(0);
        let mut labels = (0u8, 0u8);
        for stage in self.stages.iter().rev() {
            let (from, a, b) = stage.back[state];
            labels = (a, b);
            state = from as usize;
        }
        let oldest = self.stages.pop_front().expect("more than DEPTH are held");
        // `state` is where the oldest branch left from.
        let y = [oldest.near[0][labels.0 as usize].0, oldest.near[1][labels.1 as usize].0];
        self.undo(y, state as u8);
    }

    /// Everything above the trellis, undone for one decided 4D symbol.
    fn undo(&mut self, y: [Point; 2], state: u8) {
        let framing = self.params.framing;
        let m = self.decided;
        let j = (m % 4) as usize;
        // The precoder, replayed: u = y - c.
        let (p0, c0) = self.precoder.predict();
        let y0 = (i64::from(y[0].0) * FINE, i64::from(y[0].1) * FINE);
        self.precoder.push((y0.0 - p0.0, y0.1 - p0.1));
        let (p1, c1) = self.precoder.predict();
        let y1 = (i64::from(y[1].0) * FINE, i64::from(y[1].1) * FINE);
        self.precoder.push((y1.0 - p1.0, y1.1 - p1.1));
        let u0 = (((y0.0 - c0.0) / FINE) as i32, ((y0.1 - c0.1) / FINE) as i32);
        let u1 = (((y1.0 - c1.0) / FINE) as i32, ((y1.1 - c1.1) / FINE) as i32);
        let carry = trellis::modulo((c0.0 / FINE, c0.1 / FINE), (c1.0 / FINE, c1.1 / FINE));
        let u_bit = self.params.code.output(state) ^ carry ^ inversion_at(&framing, m);
        // 9.6.1 backwards: the first point's turn is Z, the second's Z + 2 I1
        // + U0.
        let z = u32::from(trellis::label(u0) & 3);
        let t = u32::from(trellis::label(u1) & 3);
        let i1 = ((t + 8 - z - u32::from(u_bit)) % 4) / 2 == 1;
        let i = (z + 4 - self.z) % 4;
        self.z = z;
        let label = |u: Point, turn: u32, outside: &mut u64| {
            quarter_label(counterclockwise(u, turn)).filter(|&l| l < framing.l / 4).unwrap_or_else(|| {
                *outside += 1;
                0
            })
        };
        let l0 = label(u0, z, &mut self.outside);
        let l1 = label(u1, t, &mut self.outside);
        let mask = (1usize << framing.q) - 1;
        self.groups[j] = Group { i1, i2: i & 1 == 1, i3: i & 2 == 2, q: [l0 & mask, l1 & mask] };
        self.labels[2 * j] = l0 >> framing.q;
        self.labels[2 * j + 1] = l1 >> framing.q;
        self.decided += 1;
        if j == 3 && m - 3 >= self.whole_from {
            let frame = m / 4;
            let within = (frame % framing.p as u64) as usize;
            let high = framing.high(within);
            let shell_bits: Vec<bool> = if framing.k > 0 {
                let rings = self.labels.map(|r| r.min(framing.m - 1));
                let r0 = self.shell.unmap(rings);
                (0..framing.k).map(|i| r0 >> i & 1 == 1).collect()
            } else {
                Vec::new()
            };
            let bits = unparse(&framing, high, &shell_bits, &self.groups);
            for bit in bits {
                let out = self.descrambler.descramble(bit);
                self.bits.push(out);
            }
        }
    }
}

/// Most a right place to read from costs the trellis a 4D symbol, on average,
/// in grid units squared: a quarter of the least distance between two 4D
/// subsets. Data mode reads a few tenths on the lines V.34 has run over, and
/// a wrong place well over one.
const SEARCH_COST: f64 = 1.0;

/// Least the best place to read from has to cost less than the next best, in
/// grid units squared: one and a half inversions' worth.
const SEARCH_MARGIN: f64 = 6.0;

/// Searches that find nothing before the signal is taken not to be data mode
/// as expected.
const SEARCHES: usize = 4;

/// What a search for data mode's frames has come to.
#[derive(Debug)]
pub enum Acquired {
    Searching,
    /// A decoder at the place found, with every symbol the search read in it
    /// already.
    Found(Box<Decoder>),
    /// Long enough to say it is not data mode as these parameters make it.
    Nothing,
}

/// One place data mode's frames could start, and what reading from there has
/// cost.
#[derive(Debug, Clone)]
struct Trial {
    /// Which 2D symbol read begins a 4D symbol, 0 or 1.
    pairing: usize,
    /// Which 4D symbol read, counted modulo a half data frame, begins one.
    phase: usize,
    metrics: Vec<f64>,
    cost: f64,
}

/// Finds where data mode's frames are, in symbols picked up part way through:
/// after a slip has lost or repeated some, or when the far end's E never
/// arrived.
///
/// Nothing in the signal marks a frame except the superframe's bit inversions
/// (9.6.3, Table 12): at the start of each half data frame, the trellis
/// encoder's Y0 goes out inverted or not, in a pattern of 2J that repeats
/// once a superframe. A decoder reading from the wrong place pays for every
/// inversion it does not expect -- and for every 4D symbol, if it pairs the 2D
/// symbols wrongly. So a trial decoder for each pairing and each 4D symbol a
/// half frame could start at, each free to take either Y0 at the start of
/// every half frame, tells the right pairing and the half frame boundaries by
/// what its best path costs over a superframe. Which Y0 the best of them took
/// at each of those places then spells out the pattern, and where in it the
/// search began: the pattern's zeros are 4, 9, 2 and 1 apart, so no turn of it
/// but one fits.
#[derive(Debug, Clone)]
pub struct Acquirer {
    params: Params,
    subsets: Vec<Vec<(u8, u8)>>,
    energy: f64,
    scale: f64,
    /// The symbols read since this search began, as fed, and in grid units.
    symbols: Vec<Complex>,
    points: Vec<Complex>,
    trials: Vec<Trial>,
    /// 4D symbols each pairing's trials have read.
    read: [usize; 2],
    failed: usize,
}

impl Acquirer {
    pub fn new(params: Params) -> Self {
        let shell = Shell::new(params.framing.m);
        let (energy, bent) = energies(&params, &shell);
        let mut acquirer = Self {
            params,
            subsets: subsets(params.code),
            energy,
            scale: bent.sqrt(),
            symbols: Vec::new(),
            points: Vec::new(),
            trials: Vec::new(),
            read: [0, 0],
            failed: 0,
        };
        acquirer.restart();
        acquirer
    }

    /// What a unit-power symbol is multiplied by to be in grid units.
    pub fn grid_scale(&self) -> f64 {
        self.scale
    }

    /// How far out the constellation's points reach, in grid units.
    pub fn extent(&self) -> i32 {
        extent(&self.params.framing)
    }

    /// 4D symbols a search reads: a superframe, over which the pattern of
    /// inversions goes round once.
    fn length(&self) -> usize {
        4 * self.params.framing.p * self.params.framing.j
    }

    fn half_frame(&self) -> usize {
        2 * self.params.framing.p
    }

    /// Start again from the next symbol.
    fn restart(&mut self) {
        let states = self.params.code.states();
        self.symbols.clear();
        self.points.clear();
        self.read = [0, 0];
        self.trials = (0..2)
            .flat_map(|pairing| (0..self.half_frame()).map(move |phase| (pairing, phase)))
            .map(|(pairing, phase)| Trial { pairing, phase, metrics: vec![0.0; states], cost: 0.0 })
            .collect();
    }

    /// One received 2D symbol at unit mean power.
    pub fn feed(&mut self, symbol: Complex) -> Acquired {
        self.symbols.push(symbol);
        self.points.push(to_grid(&self.params, self.energy, self.scale, symbol));
        let (length, half_frame, code) = (self.length(), self.half_frame(), self.params.code);
        for pairing in 0..2 {
            let n = self.read[pairing];
            let at = pairing + 2 * n;
            if n == length || at + 1 >= self.points.len() {
                continue;
            }
            let near = [nearest_by_label(self.points[at]), nearest_by_label(self.points[at + 1])];
            let table: Vec<f64> = branches(&self.subsets, &near).iter().map(|b| b.0).collect();
            for trial in self.trials.iter_mut().filter(|t| t.pairing == pairing) {
                let free = n % half_frame == trial.phase;
                trial.cost += step(&mut trial.metrics, &table, code, free, None);
            }
            self.read[pairing] += 1;
        }
        if self.read != [length, length] {
            return Acquired::Searching;
        }
        if let Some(decoder) = self.decide() {
            return Acquired::Found(Box::new(decoder));
        }
        self.failed += 1;
        if self.failed == SEARCHES {
            return Acquired::Nothing;
        }
        self.restart();
        Acquired::Searching
    }

    /// The decoder the search came to, if it came to one.
    fn decide(&self) -> Option<Decoder> {
        let length = self.length();
        let mut order: Vec<&Trial> = self.trials.iter().collect();
        order.sort_by(|a, b| a.cost.total_cmp(&b.cost));
        let (best, runner) = (order[0], order[1]);
        if best.cost / length as f64 > SEARCH_COST || runner.cost - best.cost < SEARCH_MARGIN {
            return None;
        }
        let framing = self.params.framing;
        let halves = 2 * framing.j;
        let offset = self.pattern_at(best)?;
        // 4D symbol `phase` begins the half frame the pattern calls `offset`,
        // which B1 counting would reach at (offset + 2) half frames: B1 is the
        // last data frame of a superframe, halves 2J - 2 and 2J - 1.
        let there = ((offset + 2) % halves * self.half_frame()) as u64;
        let superframe = length as u64;
        let m = (there + superframe - best.phase as u64) % superframe;
        let mut decoder = Decoder::resume(self.params, m);
        for &symbol in &self.symbols[best.pairing..] {
            decoder.feed(symbol);
        }
        Some(decoder)
    }

    /// Where in the pattern of inversions a trial's first free place is: read
    /// off its best path, and fitted to the pattern with no more than one of
    /// the places misread.
    fn pattern_at(&self, trial: &Trial) -> Option<usize> {
        let (length, half_frame, code) = (self.length(), self.half_frame(), self.params.code);
        let mut metrics = vec![0.0; code.states()];
        let mut back = Vec::with_capacity(length);
        for n in 0..length {
            let at = trial.pairing + 2 * n;
            let near = [nearest_by_label(self.points[at]), nearest_by_label(self.points[at + 1])];
            let table: Vec<f64> = branches(&self.subsets, &near).iter().map(|b| b.0).collect();
            let mut stage = vec![(0u8, false); code.states()];
            step(&mut metrics, &table, code, n % half_frame == trial.phase, Some(&mut stage));
            back.push(stage);
        }
        let mut state = metrics.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(s, _)| s);
        let mut inverted = vec![false; length];
        for n in (0..length).rev() {
            let (from, flipped) = back[n][state];
            inverted[n] = flipped;
            state = usize::from(from);
        }
        let seen: Vec<bool> = (trial.phase..length).step_by(half_frame).map(|n| inverted[n]).collect();
        let halves = 2 * self.params.framing.j;
        let misread = |offset: usize| {
            seen.iter().enumerate().filter(|&(i, &v)| trellis::inversion(self.params.framing.j, (offset + i) % halves) != v).count()
        };
        let mut fits: Vec<(usize, usize)> = (0..halves).map(|offset| (misread(offset), offset)).collect();
        fits.sort_unstable();
        (fits[0].0 <= 1 && fits[1].0 >= fits[0].0 + 2).then_some(fits[0].1)
    }
}

/// One trellis step over every state for a trial: `free` lets Y0 go either way,
/// as it may where a half frame starts. The cheapest path's cost is taken out
/// of every metric and returned; `back`, if given, keeps where each state came
/// from and whether Y0 went out inverted to get there.
fn step(metrics: &mut Vec<f64>, table: &[f64], code: Code, free: bool, mut back: Option<&mut Vec<(u8, bool)>>) -> f64 {
    let mut next = vec![f64::MAX; code.states()];
    for (state, &from) in metrics.iter().enumerate() {
        if from == f64::MAX {
            continue;
        }
        let out = usize::from(code.output(state as u8));
        for y in (0..16u8).filter(|y| y & !code.inputs() == 0) {
            let plain = table[out * 16 + y as usize];
            let turned = table[(1 - out) * 16 + y as usize];
            let (d, flipped) = if free && turned < plain { (turned, true) } else { (plain, false) };
            if d == f64::MAX {
                continue;
            }
            let to = code.next(state as u8, y) as usize;
            if from + d < next[to] {
                next[to] = from + d;
                if let Some(back) = back.as_deref_mut() {
                    back[to] = (state as u8, flipped);
                }
            }
        }
    }
    let floor = next.iter().copied().fold(f64::MAX, f64::min);
    for m in &mut next {
        if *m != f64::MAX {
            *m -= floor;
        }
    }
    *metrics = next;
    floor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::info::SymbolRate;

    fn params(rate: SymbolRate, primary: u32, code: Code, expanded: bool, nonlinear: bool) -> Params {
        Params {
            framing: Framing::new(rate, primary, false, expanded).unwrap(),
            code,
            nonlinear,
            precoding: [(0, 0); 3],
            mode: Mode::Answer,
        }
    }

    /// Bits through an encoder and straight into a decoder, with noise at
    /// `snr_db`, and the bits back in the order they went in.
    fn loopback(p: Params, frames: usize, snr_db: f64) -> (Vec<bool>, Vec<bool>) {
        let mut encoder = Encoder::new(p);
        let mut decoder = Decoder::new(p);
        let mut seed = 0x9e37_79b9_u32;
        let mut sent = Vec::new();
        let mut noise_seed = 0x2545_f491_u32;
        let sigma = 10f64.powf(-snr_db / 20.0) / 2f64.sqrt();
        let mut gauss = move || {
            // Two uniforms to one Gaussian, well enough for a test.
            let mut u = || {
                noise_seed ^= noise_seed << 13;
                noise_seed ^= noise_seed >> 17;
                noise_seed ^= noise_seed << 5;
                f64::from(noise_seed) / f64::from(u32::MAX)
            };
            let (a, b) = (u().max(1e-12), u());
            (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        };
        for _ in 0..frames * 8 {
            let symbol = encoder.next_symbol(&mut || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let bit = seed & 1 == 1;
                sent.push(bit);
                bit
            });
            decoder.feed(symbol + Complex::new(gauss() * sigma, gauss() * sigma));
        }
        (sent, decoder.take_bits())
    }

    #[test]
    fn a_frames_bits_come_back_from_its_split() {
        for (rate, primary) in [(SymbolRate::S3429, 4800), (SymbolRate::S3429, 7200), (SymbolRate::S2400, 2400), (SymbolRate::S3429, 33_600), (SymbolRate::S3000, 4800)] {
            let f = Framing::new(rate, primary, false, false).unwrap();
            // A framing whose every frame is high has no low frame to split.
            let lows = f.r < f.p;
            for high in [true, false].into_iter().filter(|&h| h || lows) {
                let count = if high { f.b } else { f.b - 1 };
                let bits: Vec<bool> = (0..count).map(|i| (i * 7 + 3) % 5 < 2).collect();
                let (shell_bits, groups) = parse(&f, high, &bits);
                assert_eq!(unparse(&f, high, &shell_bits, &groups), bits, "{rate:?} at {primary}, high {high}");
            }
        }
    }

    #[test]
    fn the_precoder_rounds_halfway_to_the_smaller_magnitude() {
        assert_eq!(round_to(3, 2), 2);
        assert_eq!(round_to(-3, 2), -2);
        assert_eq!(round_to(5, 4), 4);
        assert_eq!(round_to(7, 4), 8);
        assert_eq!(round_to(-7, 4), -8);
        assert_eq!(round_to(6, 4), 4);
    }

    #[test]
    fn every_rate_and_code_decodes_clean() {
        for (rate, primary) in [
            (SymbolRate::S3429, 4800),
            (SymbolRate::S3429, 14_400),
            (SymbolRate::S3429, 31_200),
            (SymbolRate::S3429, 33_600),
            (SymbolRate::S3200, 28_800),
            (SymbolRate::S2400, 9600),
        ] {
            for code in [Code::States16, Code::States32, Code::States64] {
                for expanded in [false, true] {
                    let p = params(rate, primary, code, expanded, false);
                    let (sent, got) = loopback(p, 60, 80.0);
                    assert!(got.len() > 100, "{rate:?} at {primary} {code:?}: {} bits", got.len());
                    // The descrambler needs 23 bits to fall into step.
                    let wrong = sent.iter().zip(&got).skip(23).filter(|(a, b)| a != b).count();
                    assert_eq!(wrong, 0, "{rate:?} at {primary} {code:?} expanded {expanded}");
                }
            }
        }
    }

    /// Symbols from an encoder of random bits, with noise at `snr_db`, and
    /// the bits.
    fn signal(p: Params, symbols: usize, snr_db: f64) -> (Vec<Complex>, Vec<bool>) {
        let mut encoder = Encoder::new(p);
        let mut seed = 0x1234_5679_u32;
        let mut noise = 0x2545_f491_u32;
        let sigma = 10f64.powf(-snr_db / 20.0) / 2f64.sqrt();
        let mut uniform = move || {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            f64::from(noise) / f64::from(u32::MAX)
        };
        let mut gauss = move || {
            let (a, b) = (uniform().max(1e-12), uniform());
            (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        };
        let mut sent = Vec::new();
        let out = (0..symbols)
            .map(|_| {
                let symbol = encoder.next_symbol(&mut || {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let bit = seed & 1 == 1;
                    sent.push(bit);
                    bit
                });
                symbol + Complex::new(gauss() * sigma, gauss() * sigma)
            })
            .collect();
        (out, sent)
    }

    #[test]
    fn data_mode_picked_up_anywhere_finds_its_frames() {
        // Every trellis code, the rates with J of 7 and 8, minimum and
        // expanded shaping, and places to start that are none of them the
        // start of anything.
        for (rate, primary, code, expanded) in [
            (SymbolRate::S3429, 33_600, Code::States16, false),
            (SymbolRate::S3429, 31_200, Code::States64, true),
            (SymbolRate::S3200, 28_800, Code::States32, false),
            (SymbolRate::S3429, 4800, Code::States16, false),
            (SymbolRate::S2400, 9600, Code::States16, true),
        ] {
            let p = params(rate, primary, code, expanded, false);
            let superframe = 8 * p.framing.p * p.framing.j;
            let (symbols, sent) = signal(p, 6 * superframe, 38.0);
            for start in [1, 2 * superframe + 7, 3 * superframe - 333] {
                let mut acquirer = Acquirer::new(p);
                let mut decoder = None;
                let mut fed = start;
                while decoder.is_none() && fed < symbols.len() {
                    match acquirer.feed(symbols[fed]) {
                        Acquired::Searching => {}
                        Acquired::Found(found) => decoder = Some(found),
                        Acquired::Nothing => panic!("{rate:?} {primary} {code:?}: nothing from {start}"),
                    }
                    fed += 1;
                }
                let mut decoder = decoder.unwrap_or_else(|| panic!("{rate:?} {primary} {code:?}: never found from {start}"));
                assert!(fed - start <= 2 * superframe + 2, "{rate:?} {primary}: {} symbols to find", fed - start);
                for &symbol in &symbols[fed..] {
                    decoder.feed(symbol);
                }
                // A good long run of what went in, back out of it.
                let got = decoder.take_bits();
                let middle = &sent[sent.len() * 2 / 3..sent.len() * 2 / 3 + 2000];
                assert!(got.windows(middle.len()).any(|w| w == middle), "{rate:?} {primary} {code:?} from {start}: {} bits and not those", got.len());
            }
        }
    }

    #[test]
    fn noise_that_is_not_data_mode_is_not_found_as_it() {
        let p = params(SymbolRate::S3429, 31_200, Code::States16, false, false);
        let mut acquirer = Acquirer::new(p);
        let mut seed = 7u32;
        let mut uniform = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f64::from(seed) / f64::from(u32::MAX) - 0.5
        };
        let mut outcome = Acquired::Searching;
        for _ in 0..(8 * p.framing.p * p.framing.j) * SEARCHES + 8 {
            outcome = acquirer.feed(Complex::new(uniform() * 3.0, uniform() * 3.0));
            if !matches!(outcome, Acquired::Searching) {
                break;
            }
        }
        assert!(matches!(outcome, Acquired::Nothing), "{outcome:?}");
    }

    #[test]
    fn non_linear_encoding_decodes_clean() {
        let p = params(SymbolRate::S3429, 33_600, Code::States64, true, true);
        let (sent, got) = loopback(p, 60, 80.0);
        let wrong = sent.iter().zip(&got).skip(23).filter(|(a, b)| a != b).count();
        assert_eq!(wrong, 0);
    }

    #[test]
    fn a_unit_power_signal_leaves_at_unit_power() {
        // Every rate at 3429 symbols a second, both shapings, and non-linear
        // encoding at the rates that stretch the most: high and low mapping
        // frames alike, since TRN's power is the one to keep (10.1.3).
        let mut cases: Vec<(u32, bool, bool)> = (2..=14).flat_map(|r| [(r * 2400, false, false), (r * 2400, true, false)]).collect();
        cases.extend([(33_600, true, true), (31_200, false, true), (28_800, true, true)]);
        for (primary, expanded, nonlinear) in cases {
            let p = params(SymbolRate::S3429, primary, Code::States64, expanded, nonlinear);
            let mut encoder = Encoder::new(p);
            let mut seed = 77u32;
            let n = 120_000;
            let power = (0..n)
                .map(|_| {
                    encoder
                        .next_symbol(&mut || {
                            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                            seed >> 16 & 1 == 1
                        })
                        .norm_sqr()
                })
                .sum::<f64>()
                / n as f64;
            assert!((power - 1.0).abs() < 0.012, "{primary} expanded {expanded} nonlinear {nonlinear}: {power}");
        }
    }

    #[test]
    fn the_peak_is_as_far_as_any_symbol_goes() {
        let mut peaks = Vec::new();
        for nonlinear in [false, true] {
            let p = params(SymbolRate::S3429, 33_600, Code::States16, true, nonlinear);
            let mut encoder = Encoder::new(p);
            let peak = Decoder::new(p).peak();
            let mut seed = 5u32;
            let furthest = (0..60_000)
                .map(|_| {
                    let s = encoder.next_symbol(&mut || {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        seed >> 16 & 1 == 1
                    });
                    s.re.abs().max(s.im.abs())
                })
                .fold(0.0, f64::max);
            assert!(furthest <= peak * 1.000_001, "nonlinear {nonlinear}: a symbol at {furthest} past {peak}");
            assert!(furthest > 0.9 * peak, "nonlinear {nonlinear}: nothing near {peak}, {furthest} at most");
            peaks.push(peak);
        }
        // Stretched outwards, and a shaped constellation reaches about one
        // and a half at unit power.
        assert!(peaks[1] > peaks[0] && (1.3..2.0).contains(&peaks[0]), "{peaks:?}");
    }

    #[test]
    fn noise_is_decoded_through_down_to_what_each_rate_needs() {
        // 33 600 packs 1408 points into the power 4800 gives four, and wants
        // about 35 dB of signal to noise to come through clean; 14 400 wants
        // less than 29. Both hold over 300 mapping frames here, and 33 600
        // does not at 29 dB -- which is the cliff a line trained at 33 dB
        // stands at the edge of.
        for (primary, snr, clean) in [(33_600, 36.0, true), (14_400, 29.0, true), (33_600, 29.0, false)] {
            let p = params(SymbolRate::S3429, primary, Code::States16, false, false);
            let (sent, got) = loopback(p, 300, snr);
            let wrong = sent.iter().zip(&got).skip(23).filter(|(a, b)| a != b).count();
            assert_eq!(wrong == 0, clean, "{primary} at {snr} dB: {wrong} of {} wrong", got.len());
        }
    }

    #[test]
    fn precoding_is_undone_through_the_channel_it_was_made_for() {
        // A channel of 1 + h(1) z^-1 + h(2) z^-2 + h(3) z^-3 on x gives back
        // y, less the rounding of p: which is what a precoder is for.
        let mut p = params(SymbolRate::S3429, 31_200, Code::States16, false, false);
        p.precoding = [(4096, -2048), (-1024, 512), (256, 0)];
        let mut encoder = Encoder::new(p);
        let mut decoder = Decoder::new(p);
        let h: Vec<Complex> = p.precoding.iter().map(|&(re, im)| Complex::new(f64::from(re), f64::from(im)).scale(1.0 / 16384.0)).collect();
        let mut past = VecDeque::from(vec![Complex::ZERO; 3]);
        let mut seed = 5u32;
        let mut sent = Vec::new();
        let unit = encoder.grid_scale();
        for _ in 0..80 * 8 {
            let x = encoder.next_symbol(&mut || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                sent.push(seed & 1 == 1);
                seed & 1 == 1
            });
            let grid = x.scale(unit);
            let mut out = grid;
            for (xp, hp) in past.iter().zip(&h) {
                out += *xp * *hp;
            }
            past.pop_back();
            past.push_front(grid);
            decoder.feed(out.scale(1.0 / decoder.grid_scale()));
        }
        let got = decoder.take_bits();
        let wrong = sent.iter().zip(&got).skip(23).filter(|(a, b)| a != b).count();
        assert!(got.len() > 1000);
        assert_eq!(wrong, 0);
    }
}
