//! What a received point is decided against, and what its errors mean.
//!
//! V.34's receiver knew two kinds of constellation: its own four and sixteen
//! points, and data mode's grid of odd integers (`v34/receiver.rs:141-216`).
//! V.32 has more, and they do not fit either. Its four synchronising states
//! are turned 26.57 degrees from the axes, and its 32- and 128-point crosses
//! sit on a lattice turned 45 degrees to V.34's grid (spec.md 2.1, core.md
//! 7.1). So a constellation here is a table of points, from whoever knows
//! them, and the grid stays as it was for V.34.
//!
//! What the loss and resync arithmetic needs from a constellation is not its
//! shape but how dense it is. On four or sixteen points a lost signal's error
//! is most of the way to the next point; on thirty-two and more every sample
//! lands within half a step of some point whatever it is, and garbage reads
//! only about a sixth of a step squared (`v34/receiver.rs:173-179`). A
//! sixteen-point rule on a 128-point cross would never see a loss at all
//! (core.md N5), so the density, not the kind of slicer, picks the rule.

use std::sync::Arc;

use crate::Complex;

/// How a constellation's errors behave once the signal is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// Sixteen points or fewer: V.34's four- and sixteen-point rules.
    Sparse,
    /// More than sixteen: V.34's data-mode grid rules.
    Dense,
}

/// A constellation's points, indexed by label, with what the receiver needs
/// to know about them worked out once.
///
/// The points are expected at unit mean power: the gain control holds the
/// equaliser's output to whatever mean power the table has, and every
/// threshold is in the table's own units.
#[derive(Debug, Clone, PartialEq)]
pub struct Constellation {
    points: Vec<Complex>,
    d2min: f64,
    power: f64,
    /// The variance of the points' power: nought for four points, 0.32 for
    /// sixteen (design.md P2).
    spread: f64,
    garbage: f64,
    /// The angle the points' mean fourth power lies at, which a resync's
    /// fourth-power phase is measured from.
    fourth: f64,
    /// |E p^4| / E|p|^4: how strong that line is, 1 for four points.
    fourth_line: f64,
    density: Density,
    lattice: Option<Lattice>,
}

/// A constellation that is part of a square lattice, turned however it is,
/// with each lattice site's possible nearest points listed in advance.
///
/// Rounding a point's lattice coordinates finds the site it is nearest. If
/// that site is one of the constellation's points, that point is the nearest
/// of them all: the lattice's cells are squares and the site's own cell
/// holds the point. If it is not -- beyond the edge, or a cross's missing
/// corner -- the answer is one of a short list worked out for that site: every
/// point that is nearer the cell at its closest than the best point is at its
/// farthest. Outside the listed sites the search is exhaustive, which only
/// garbage ever reaches.
#[derive(Debug, Clone, PartialEq)]
struct Lattice {
    origin: Complex,
    /// One over a step along the first axis: multiplying by it turns a
    /// point's offset from the origin into lattice coordinates.
    inverse: Complex,
    low: (i64, i64),
    width: usize,
    height: usize,
    /// For each site, where its candidates start and how many there are.
    sites: Vec<(u32, u16)>,
    candidates: Vec<u16>,
}

impl Constellation {
    /// A table of `points`, the label of each being its index.
    ///
    /// Panics on fewer than two points or on two points in the same place,
    /// neither of which is a constellation.
    pub fn new(points: Vec<Complex>) -> Self {
        let n = points.len();
        assert!((2..=usize::from(u16::MAX)).contains(&n), "a constellation of {n} points");
        let mut d2min = f64::INFINITY;
        let mut pair = (0, 1);
        for i in 0..n {
            for j in i + 1..n {
                let d = (points[i] - points[j]).norm_sqr();
                if d < d2min {
                    d2min = d;
                    pair = (i, j);
                }
            }
        }
        assert!(d2min > 0.0, "two points of a constellation in the same place");
        let power = points.iter().map(|p| p.norm_sqr()).sum::<f64>() / n as f64;
        let spread = points.iter().map(|p| (p.norm_sqr() - power).powi(2)).sum::<f64>() / n as f64;
        let m4 = points.iter().fold(Complex::ZERO, |sum, p| sum + *p * *p * *p * *p);
        let fourth_line = m4.abs() / points.iter().map(|p| p.norm_sqr().powi(2)).sum::<f64>();
        // A square constellation's fourth power points exactly the opposite
        // way to the real axis. Rounding in the table must not move that off
        // pi, where V.34's resync puts it (`v34/receiver.rs:1175`).
        let mut fourth = m4.arg();
        if (fourth.abs() - std::f64::consts::PI).abs() < 1e-9 {
            fourth = std::f64::consts::PI;
        }
        let density = if n <= 16 { Density::Sparse } else { Density::Dense };
        let lattice = Lattice::find(&points, points[pair.1] - points[pair.0]);
        let mut table = Self { points, d2min, power, spread, garbage: 0.0, fourth, fourth_line, density, lattice };
        table.garbage = table.measure_garbage();
        table
    }

    pub fn points(&self) -> &[Complex] {
        &self.points
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The least squared distance between two of the points.
    pub fn d2min(&self) -> f64 {
        self.d2min
    }

    /// The points' mean power: 1 for a table at unit power.
    pub fn power(&self) -> f64 {
        self.power
    }

    /// The mean squared error of a signal of the same power that is not this
    /// constellation at all: complex Gaussian noise, decided against it.
    pub fn garbage(&self) -> f64 {
        self.garbage
    }

    pub fn density(&self) -> Density {
        self.density
    }

    /// How strong the points' fourth-power line is against their fourth
    /// power: 1 for four points, about a seventh for V.32's 128-point cross.
    pub fn fourth_line(&self) -> f64 {
        self.fourth_line
    }

    /// Whether the fast search found a lattice under the points.
    pub fn on_lattice(&self) -> bool {
        self.lattice.is_some()
    }

    /// The label of the point nearest `z`.
    pub fn nearest(&self, z: Complex) -> usize {
        self.lattice.as_ref().and_then(|l| l.nearest(&self.points, z)).unwrap_or_else(|| self.nearest_exhaustive(z))
    }

    /// The label of the point nearest `z`, by trying every one: what the fast
    /// search is checked against. The first of equals wins.
    pub fn nearest_exhaustive(&self, z: Complex) -> usize {
        let mut best = (f64::INFINITY, 0);
        for (i, p) in self.points.iter().enumerate() {
            let d = (z - *p).norm_sqr();
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }

    /// Gaussian noise of the table's power decided against it, from a fixed
    /// seed so that every table of the same points reads the same.
    fn measure_garbage(&self) -> f64 {
        const DRAWS: usize = 8192;
        let mut seed = 0x9e37_79b9_u32;
        let mut uniform = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (f64::from(seed) + 0.5) / (f64::from(u32::MAX) + 1.0)
        };
        let sigma = (self.power / 2.0).sqrt();
        let mut sum = 0.0;
        for _ in 0..DRAWS {
            let (a, b) = (uniform(), uniform());
            let r = sigma * (-2.0 * a.ln()).sqrt();
            let z = Complex::from_polar(r, std::f64::consts::TAU * b);
            sum += (z - self.points[self.nearest(z)]).norm_sqr();
        }
        sum / DRAWS as f64
    }
}

impl Lattice {
    /// The lattice with one step `step` that every point lies on, if there is
    /// one, and the map of its sites.
    fn find(points: &[Complex], step: Complex) -> Option<Self> {
        let origin = points[0];
        let inverse = Complex::ONE / step;
        let mut coordinates = Vec::with_capacity(points.len());
        for p in points {
            let w = (*p - origin) * inverse;
            let (u, v) = (w.re.round(), w.im.round());
            if (w.re - u).abs() > 1e-6 || (w.im - v).abs() > 1e-6 {
                return None;
            }
            coordinates.push((u as i64, v as i64));
        }
        let (u_low, u_high) = coordinates.iter().fold((i64::MAX, i64::MIN), |(a, b), c| (a.min(c.0), b.max(c.0)));
        let (v_low, v_high) = coordinates.iter().fold((i64::MAX, i64::MIN), |(a, b), c| (a.min(c.1), b.max(c.1)));
        // A whole constellation's width beyond each edge, and two sites more:
        // far enough out that only garbage and the grossest gain errors fall
        // off the map.
        let margin = (u_high - u_low).max(v_high - v_low) + 2;
        let low = (u_low - margin, v_low - margin);
        let width = (u_high - u_low + 1 + 2 * margin) as usize;
        let height = (v_high - v_low + 1 + 2 * margin) as usize;
        let mut member = vec![None; width * height];
        for (label, &(u, v)) in coordinates.iter().enumerate() {
            member[(v - low.1) as usize * width + (u - low.0) as usize] = Some(label as u16);
        }
        let mut sites = Vec::with_capacity(width * height);
        let mut candidates = Vec::new();
        for row in 0..height {
            for column in 0..width {
                let start = candidates.len() as u32;
                if let Some(label) = member[row * width + column] {
                    candidates.push(label);
                } else {
                    // The site's cell is the unit square about it, in lattice
                    // units, which are the same in both directions. Whatever
                    // point is nearest some part of the cell is no farther
                    // from the cell's nearest edge than the best point is from
                    // its farthest corner.
                    let (u, v) = (low.0 + column as i64, low.1 + row as i64);
                    let offsets: Vec<(f64, f64)> = coordinates
                        .iter()
                        .map(|&(a, b)| (((a - u) as f64).abs(), ((b - v) as f64).abs()))
                        .collect();
                    let farthest = offsets.iter().map(|&(x, y)| (x + 0.5).powi(2) + (y + 0.5).powi(2)).fold(f64::INFINITY, f64::min);
                    for (label, &(x, y)) in offsets.iter().enumerate() {
                        let nearest = (x - 0.5).max(0.0).powi(2) + (y - 0.5).max(0.0).powi(2);
                        if nearest <= farthest + 1e-9 {
                            candidates.push(label as u16);
                        }
                    }
                }
                sites.push((start, (candidates.len() as u32 - start) as u16));
            }
        }
        Some(Self { origin, inverse, low, width, height, sites, candidates })
    }

    /// The nearest point by the map, or None off the map.
    fn nearest(&self, points: &[Complex], z: Complex) -> Option<usize> {
        let w = (z - self.origin) * self.inverse;
        let (u, v) = (w.re.round(), w.im.round());
        if !u.is_finite() || !v.is_finite() {
            return None;
        }
        let (column, row) = (u as i64 - self.low.0, v as i64 - self.low.1);
        if column < 0 || row < 0 || column as usize >= self.width || row as usize >= self.height {
            return None;
        }
        let (start, count) = self.sites[row as usize * self.width + column as usize];
        let listed = &self.candidates[start as usize..start as usize + usize::from(count)];
        if let [only] = listed {
            return Some(usize::from(*only));
        }
        let mut best = (f64::INFINITY, 0);
        for &label in listed {
            let d = (z - points[usize::from(label)]).norm_sqr();
            if d < best.0 {
                best = (d, usize::from(label));
            }
        }
        Some(best.1)
    }
}

/// What the decisions that keep the loops going are made against.
#[derive(Debug, Clone, PartialEq)]
pub enum Slicer {
    /// A table of points: V.32's constellations, or V.34's four and sixteen.
    Table(Arc<Constellation>),
    /// Every odd grid point out to `limit`, a unit-power symbol being `scale`
    /// grid units: V.34's data mode, whose trellis decoder makes the real
    /// decisions some symbols later than the loops can wait
    /// (`v34/receiver.rs:146-149`).
    Grid { scale: f64, limit: i32 },
}

impl Slicer {
    pub fn table(constellation: Constellation) -> Self {
        Self::Table(Arc::new(constellation))
    }

    /// The nearest point to `z`, with its label if the slicer is a table.
    pub fn decide(&self, z: Complex) -> (Option<usize>, Complex) {
        match self {
            Self::Table(table) => {
                let label = table.nearest(z);
                (Some(label), table.points[label])
            }
            // As `v34/receiver.rs:157-161`.
            Self::Grid { scale, limit } => {
                let (scale, limit) = (*scale, *limit);
                let odd = |v: f64| (2 * ((v * scale - 1.0) / 2.0).round() as i32 + 1).clamp(-limit, limit);
                let point = (odd(z.re), odd(z.im));
                (None, Complex::new(f64::from(point.0), f64::from(point.1)).scale(1.0 / scale))
            }
        }
    }

    /// The least squared distance between two of its points.
    pub fn min_distance_squared(&self) -> f64 {
        match self {
            Self::Table(table) => table.d2min,
            Self::Grid { scale, .. } => (2.0 / scale).powi(2),
        }
    }

    pub fn density(&self) -> Density {
        match self {
            Self::Table(table) => table.density,
            Self::Grid { .. } => Density::Dense,
        }
    }

    /// The mean power the gain control holds the output to.
    pub fn mean_power(&self) -> f64 {
        match self {
            Self::Table(table) => table.power,
            Self::Grid { .. } => 1.0,
        }
    }

    /// The error of a signal that is not this constellation at all.
    pub fn garbage(&self) -> f64 {
        match self {
            Self::Table(table) => table.garbage,
            Self::Grid { .. } => self.min_distance_squared() / 6.0,
        }
    }

    /// Where the fourth power of a symbol in step lies.
    pub(super) fn fourth(&self) -> f64 {
        match self {
            Self::Table(table) => table.fourth,
            Self::Grid { .. } => std::f64::consts::PI,
        }
    }

    /// The gain control's share of each symbol: time constants of 32, 128
    /// and 256 symbols for four, sixteen and more points (design.md 2.2). A
    /// denser constellation's power varies more from symbol to symbol, and
    /// the estimate needs more of them to be as steady (design.md P2).
    pub(super) fn agc_rate(&self) -> f64 {
        match self {
            Self::Table(table) if table.len() <= 4 => 1.0 / 32.0,
            Self::Table(table) if table.len() <= 16 => 1.0 / 128.0,
            _ => 1.0 / 256.0,
        }
    }

    /// How far the gain control's power estimate wanders on its own, as a
    /// share of the power, three standard deviations of it: from the points'
    /// own spread of power, and from noise of power `noise` on top.
    ///
    /// The estimate is made without decisions, so every symbol's power goes
    /// into it, and the constellation's own spread is noise to it. On sixteen
    /// points at 128 symbols that is 1.8% of gain, rms, which alone holds a
    /// clean line to 35 dB, and on the 128-point cross at 256 symbols it is
    /// 44 dB. So within this much of the right power the gain is left alone,
    /// for the equaliser to settle from the decisions, and the gain control
    /// acts only on what is beyond it.
    pub(super) fn agc_zone(&self, noise: f64) -> f64 {
        let (mean, spread) = match self {
            Self::Table(table) => (table.power, table.spread),
            // The grid is V.34's, which has no gain control; a 128-point
            // cross's spread is as good a guess as any.
            Self::Grid { .. } => (1.0, 0.34),
        };
        let rate = self.agc_rate();
        let variance = spread / (mean * mean) + 2.0 * noise.max(0.0) / mean;
        3.0 * (variance * rate / (2.0 - rate)).sqrt()
    }

    /// What the carrier loop's phase error is divided by for a decision at
    /// `target`.
    ///
    /// V.34 divides by the target's power, floored at a tenth
    /// (`v34/receiver.rs:955-956`). That leaves an inner point of a 128-point
    /// cross, at 0.024, a quarter of the say of an outer one, and the loop's
    /// bandwidth depending on which points came (core.md 9, 7.9). Unweighted,
    /// every point has its own power's say, and the loop's gain is the
    /// constellation's mean power, 1. V.34's grid keeps its own form.
    pub(super) fn phase_power(&self, target: Complex, unweighted: bool) -> f64 {
        match self {
            Self::Table(_) if unweighted => 1.0,
            _ => target.norm_sqr().max(0.1),
        }
    }

    /// Mean squared error past which the signal is taken to be lost
    /// (`v34/receiver.rs:180-185`).
    pub(super) fn lost_level(&self) -> f64 {
        match self.density() {
            Density::Sparse => 0.25 * self.min_distance_squared(),
            Density::Dense => self.min_distance_squared() / 12.0,
        }
    }

    /// The recent error past which the signal is lost, given what it settles
    /// to (`v34/receiver.rs:191-196`).
    pub(super) fn lost_threshold(&self, settled: f64) -> f64 {
        match self.density() {
            Density::Sparse => (8.0 * settled).max(self.lost_level()),
            Density::Dense => (2.0 * settled).max(self.lost_level()),
        }
    }

    /// The error a resync's best reading has to come under to be believed
    /// (`v34/receiver.rs:199-206`).
    pub(super) fn found_level(&self, settled: f64) -> f64 {
        match self.density() {
            Density::Sparse => (4.0 * settled).max(0.0625 * self.min_distance_squared()),
            Density::Dense => (2.0 * settled).max(0.4 * self.min_distance_squared() / 6.0),
        }
    }

    /// Symbols a resync reads, and over which lost is judged
    /// (`v34/receiver.rs:210-215`).
    pub(super) fn window(&self) -> (usize, usize) {
        match self.density() {
            Density::Sparse => (super::RESYNC_WINDOW, 8),
            Density::Dense => (4 * super::RESYNC_WINDOW, 32),
        }
    }
}
