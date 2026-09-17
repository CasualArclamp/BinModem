//! Digital impairment learning (8.4.1, 9.3.2.9): what the route does to each
//! codeword, and the constellations that follow from it.
//!
//! Between the digital modem and the codec there may be more than wire. A T1
//! carrying signalling in its bits overwrites the least significant bit of
//! every sixth octet -- robbed-bit signalling -- so in one data frame interval
//! half the codewords arrive as their neighbours. A digital pad scales every
//! level by a table. A gateway between the two companding laws moves every
//! codeword to the nearest one of the other law. None of it is announced, and
//! all of it is exact: the same codeword always arrives as the same level.
//!
//! So the analogue modem asks for every codeword, several times in every
//! interval, and writes down what arrived. The DIL descriptor says which
//! codewords and how (8.3.1); the analysis is left to the analogue modem, and
//! so is what it asks for.

use super::INTERVALS;
use super::modulus::Moduli;
use super::sequences::{Cp, Descriptor, Mask};
use super::sign::Redundancy;
use super::ucode::{self, Law, UCODES};

/// Symbols a segment: H of 5 is six data frames, one of references and five
/// of the codeword being learned -- five readings of it in every interval.
const H: u8 = 5;

/// Pattern length, which is the segment's.
const PATTERN: usize = (H as usize + 1) * INTERVALS;

/// Codewords between one DIL segment and the next.
const ORDER_STEP: usize = 3;

/// The loudest codeword the DIL asks for, and so the loudest any
/// constellation uses, as a fraction of full scale.
///
/// A codeword is a sample, but what reaches the analogue modem is the
/// waveform the far codec draws through its samples, and that goes a good
/// deal higher than the loudest of them. On a live call through a softphone,
/// everything to about a third of full scale arrived exactly, and everything
/// much above it was held down by something with a gain control, which went
/// on reading the codewords after it low for a third of a second. Table 15's
/// powers put nothing up there that a constellation needs.
pub const LOUDEST: f64 = 0.3;

/// Spacing between neighbouring levels, in the noise's standard deviations,
/// that a constellation is built to: five either side of the decision
/// boundary, which a Gaussian error passes once in three and a half million
/// symbols.
pub const SPACING: f64 = 10.0;

/// The DIL this modem asks for: every codeword up to [`LOUDEST`] but UINFO,
/// each in a segment of six frames -- the first all references at UINFO, the
/// other five the codeword itself -- with signs from a fixed balanced
/// pattern.
///
/// Every codeword because the route is unknown, and UINFO read from the
/// references instead; six frames because five readings an interval is
/// enough, pooled over the codewords, to see a robbed bit or a pad. A pass is
/// 98 segments under μ-law, 3528 symbols, 0.44 s.
///
/// Three times up the codewords, three at a time, rather than once one at a
/// time. 8.4.1 restarts the sign pattern in every segment, so every segment
/// has the same signs, and a softphone whose jitter buffer shortens its delay
/// cuts where the audio repeats: on a live call, ten milliseconds a pass of
/// the DIL. Reading the DIL again after a cut is a matter of telling a
/// segment from its neighbour, which one codeword apart are all but alike,
/// and three apart are not. Nor are neighbours so far apart that one's
/// errors land in the other's reading: a DIL in any order reads each
/// codeword with an error that grows with what surrounds it, and in a random
/// order that is a loud neighbour as often as not.
pub fn design(law: Law, uinfo: u8) -> Descriptor {
    // The sign pattern: scrambled ones, so positive and negative in about
    // equal numbers and no line in the spectrum for an echo canceller in the
    // network to take for a tone.
    let mut scrambler = crate::v32::Scrambler::new(crate::v32::Mode::Answer);
    let signs: Vec<bool> = (0..PATTERN).map(|_| scrambler.scramble(true)).collect();
    let training: Vec<bool> = (0..PATTERN).map(|n| n >= INTERVALS).collect();
    let ucodes = (0..ORDER_STEP)
        .flat_map(|first| (first..UCODES).step_by(ORDER_STEP))
        .map(|u| u as u8)
        .filter(|&u| u != uinfo && ucode::level(law, u) <= LOUDEST)
        .collect();
    Descriptor { signs, training, h: [H; 8], refs: [uinfo; 8], ucodes }
}

/// What the route did, as far as the DIL showed it.
#[derive(Debug, Clone)]
pub struct Route {
    /// The level each codeword arrived at, in each data frame interval, with
    /// its sign taken off: `levels[interval][ucode]`.
    pub levels: [[f64; UCODES]; INTERVALS],
    /// The spread of the readings of each codeword, pooled across intervals.
    pub spread: [f64; UCODES],
    /// How many readings went into each.
    pub readings: [[u32; UCODES]; INTERVALS],
}

/// Reads a DIL as it arrives.
#[derive(Debug, Clone)]
pub struct Analysis {
    sum: [[f64; UCODES]; INTERVALS],
    squares: [[f64; UCODES]; INTERVALS],
    count: [[u32; UCODES]; INTERVALS],
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}

impl Analysis {
    pub fn new() -> Self {
        Self { sum: [[0.0; UCODES]; INTERVALS], squares: [[0.0; UCODES]; INTERVALS], count: [[0; UCODES]; INTERVALS] }
    }

    /// One symbol: the codeword and sign that were sent, the interval they
    /// were sent in, and what the equaliser made of them.
    pub fn feed(&mut self, ucode: u8, positive: bool, interval: usize, value: f64) {
        let level = if positive { value } else { -value };
        let (i, u) = (interval % INTERVALS, usize::from(ucode) % UCODES);
        self.sum[i][u] += level;
        self.squares[i][u] += level * level;
        self.count[i][u] += 1;
    }

    /// What it all comes to.
    pub fn route(&self) -> Route {
        let mut levels = [[0.0; UCODES]; INTERVALS];
        let mut spread = [0.0; UCODES];
        for u in 0..UCODES {
            let (mut variance, mut degrees) = (0.0, 0u32);
            for (i, row) in levels.iter_mut().enumerate() {
                let n = self.count[i][u];
                if n == 0 {
                    continue;
                }
                let mean = self.sum[i][u] / f64::from(n);
                row[u] = mean;
                variance += self.squares[i][u] - f64::from(n) * mean * mean;
                degrees += n.saturating_sub(1);
            }
            spread[u] = if degrees > 0 { (variance.max(0.0) / f64::from(degrees)).sqrt() } else { f64::INFINITY };
        }
        Route { levels, spread, readings: self.count }
    }
}

impl Route {
    /// A route that does nothing: every codeword arrives as itself, with
    /// `noise` of spread.
    pub fn clean(law: Law, noise: f64) -> Self {
        let levels = std::array::from_fn(|_| std::array::from_fn(|u| ucode::level(law, u as u8)));
        Self { levels, spread: [noise; UCODES], readings: [[1; UCODES]; INTERVALS] }
    }

    /// The typical spread, the median across codewords that were read.
    pub fn noise(&self) -> f64 {
        let mut s: Vec<f64> = self.spread.iter().copied().filter(|x| x.is_finite()).collect();
        if s.is_empty() {
            return f64::INFINITY;
        }
        s.sort_by(f64::total_cmp);
        s[s.len() / 2]
    }

    /// The spread data mode can expect in a signal whose RMS level, as sent,
    /// is `rms`.
    ///
    /// Not the median across codewords. The spread grows with level -- a DIL
    /// segment of the loudest codewords reads several times as untidy as one
    /// of the quietest, from everything in the receiver whose error is a share
    /// of the signal -- and data mode's symbols sit in a signal at the power
    /// Table 15 allows, loud or quiet as each one is. A constellation spaced
    /// by the median made an error every second or so. So the spread's square
    /// is fitted as a floor plus a share of each segment's power, across every
    /// codeword read, and taken at data mode's power.
    pub fn noise_at(&self, law: Law, rms: f64) -> f64 {
        let points: Vec<(f64, f64)> = (0..UCODES)
            .filter(|&u| self.spread[u].is_finite())
            .map(|u| (ucode::level(law, u as u8).powi(2), self.spread[u].powi(2)))
            .collect();
        if points.len() < 2 {
            return self.noise();
        }
        let n = points.len() as f64;
        let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
        let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let slope = if sxx > 0.0 { (sxy / sxx).max(0.0) } else { 0.0 };
        let floor = (my - slope * mx).max(0.0);
        // Never tidier than the quiet codewords read.
        (floor + slope * rms * rms).sqrt().max(self.noise())
    }

    /// Interval `i`'s constellation: codewords whose levels stand `spacing`
    /// spreads apart from their neighbours and from their own opposites.
    ///
    /// Built from the top down, because the loud end is where the codewords
    /// are sparse and every one counts. A codeword the route sent to the
    /// same level as one already taken -- a robbed bit's neighbour, say --
    /// is too close to it and left out.
    pub fn constellation(&self, i: usize, spacing: f64, law: Law) -> Vec<u8> {
        let mut order: Vec<u8> = (0..UCODES as u8).filter(|&u| self.readings[i][usize::from(u)] > 0).collect();
        order.sort_by(|&a, &b| self.levels[i][usize::from(b)].total_cmp(&self.levels[i][usize::from(a)]));
        // How far a codeword arrived from where it was sent.
        let moved = |u: u8| (self.levels[i][usize::from(u)] - ucode::level(law, u)).abs();
        let mut chosen: Vec<u8> = Vec::new();
        for u in order {
            let level = self.levels[i][usize::from(u)];
            let spread = self.spread[usize::from(u)];
            // Far enough from its own opposite, across zero.
            if 2.0 * level < spacing * spread {
                break;
            }
            match chosen.last().copied() {
                Some(last) if self.levels[i][usize::from(last)] - level < 0.5 * spacing * (spread + self.spread[usize::from(last)]) => {
                    // Two codewords the route put in the same place: keep the
                    // one that arrived as itself.
                    if moved(u) < moved(last) {
                        chosen.pop();
                        chosen.push(u);
                    }
                }
                _ => chosen.push(u),
            }
        }
        chosen.sort_unstable();
        chosen
    }
}

/// 8.5.2's average power of a constellation set carrying K bits, in Table
/// 1's units squared: every level weighted by how many of the 2^K messages
/// the modulus encoder sends to it.
pub fn average_power(law: Law, sets: &[Vec<u8>; INTERVALS], k: u32) -> f64 {
    let moduli: Moduli = std::array::from_fn(|i| sets[i].len() as u16);
    if k >= 64 || moduli.contains(&0) {
        return f64::INFINITY;
    }
    let total = 1u128 << k;
    // The modulus encoder's quotients and remainders for R0 = 2^K - 1.
    let mut r = total - 1;
    let mut a: u128 = 1;
    let mut power = 0.0;
    for (i, set) in sets.iter().enumerate() {
        let m = u128::from(moduli[i]);
        let ki = r % m;
        let next = r / m;
        // Labels run loudest first (5.4.4).
        let mut points = set.clone();
        points.sort_unstable_by(|x, y| y.cmp(x));
        for (j, &u) in points.iter().enumerate() {
            let j = j as u128;
            let n = if j < ki {
                a * (next + 1)
            } else if j == ki {
                total - a * (r - next)
            } else {
                a * next
            };
            let linear = f64::from(ucode::linear(law, u));
            power += linear * linear * n as f64;
        }
        r = next;
        a *= m;
    }
    power / (INTERVALS as f64 * total as f64)
}

/// What the analogue modem asks for: a data mode constellation set and the
/// training one for phase 4, both inside the digital modem's power.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub data: Cp,
    pub training: Cp,
}

/// Interval `i`'s codewords from the quietest up, `spacing` apart in level
/// and each at least half that from its own opposite across zero.
///
/// Two codewords the route put within `spacing` of each other cannot both be
/// used; of the two, the one that arrived as itself is kept.
///
/// A codeword whose own readings spread over half a spacing is not a level
/// at all -- the loudest ones, where a softphone's sample rate conversion
/// has run out of headroom, come back anywhere below where they were sent --
/// and is left out.
fn ladder(route: &Route, law: Law, i: usize, spacing: f64) -> Vec<u8> {
    let level = |u: u8| route.levels[i][usize::from(u)];
    let mut order: Vec<u8> = (0..UCODES as u8)
        .filter(|&u| route.readings[i][usize::from(u)] > 0 && 2.0 * route.spread[usize::from(u)] <= spacing)
        .collect();
    order.sort_by(|&a, &b| level(a).total_cmp(&level(b)));
    let moved = |u: u8| (level(u) - ucode::level(law, u)).abs();
    // Codewords the route put in the same place -- within a few noises, as a
    // robbed bit's neighbours are -- are one level: keep whichever of them
    // arrived as itself.
    let mut levels: Vec<u8> = Vec::new();
    // Where the current group starts: a group is measured from its bottom,
    // or a representative that moves up as it is replaced would carry the
    // group up with it.
    let mut bottom = f64::NEG_INFINITY;
    for u in order {
        match levels.last_mut() {
            Some(same) if level(u) - bottom < 0.3 * spacing => {
                if moved(u) < moved(*same) {
                    *same = u;
                }
            }
            _ => {
                bottom = level(u);
                levels.push(u);
            }
        }
    }
    let mut chosen: Vec<u8> = Vec::new();
    for u in levels {
        if 2.0 * level(u) < spacing {
            continue;
        }
        if chosen.last().is_none_or(|&last| level(u) - level(last) >= spacing) {
            chosen.push(u);
        }
    }
    chosen
}

/// How much more a constellation set can say than the bits it carries: a
/// quarter.
///
/// Room that is never used, and there for a reason. A frame read with its
/// intervals in the wrong places -- which is what a softphone's jitter buffer
/// does to every frame after a slip -- comes out of the modulus decoder as a
/// number the encoder could never have made about a fifth of the time, and a
/// frame read in its right place never does. That is how the analogue modem
/// knows it has lost its place, and finds it again.
pub const ROOM: (u128, u128) = (5, 4);

/// Whether these moduli carry `k` bits with [`ROOM`] to spare.
pub fn fits_with_room(moduli: Moduli, k: u32) -> bool {
    let product: u128 = moduli.iter().map(|&m| u128::from(m)).product();
    k < 120 && product * ROOM.1 >= ROOM.0 << k
}

/// Sets carrying `k` bits at `spacing`, as quiet as they can be: each
/// interval takes the fewest of its ladder that the bits need, and where one
/// interval's ladder is short -- a robbed bit halves it -- the others take
/// more, the cheapest next rung first. None if the ladders cannot carry `k`
/// or the result is over `limit`.
fn quietest(route: &Route, law: Law, k: u32, spacing: f64, limit: f64) -> Option<[Vec<u8>; INTERVALS]> {
    let ladders: [Vec<u8>; INTERVALS] = std::array::from_fn(|i| ladder(route, law, i, spacing));
    let even = (2f64.powf(f64::from(k) / INTERVALS as f64) - 1e-9).ceil() as usize;
    let mut sizes: [usize; INTERVALS] = std::array::from_fn(|i| even.min(ladders[i].len()));
    let fits = |sizes: &[usize; INTERVALS]| {
        let moduli: Moduli = std::array::from_fn(|i| sizes[i] as u16);
        fits_with_room(moduli, k)
    };
    while !fits(&sizes) {
        let cheapest = (0..INTERVALS)
            .filter(|&i| sizes[i] < ladders[i].len())
            .min_by(|&a, &b| {
                let next = |i: usize| route.levels[i][usize::from(ladders[i][sizes[i]])];
                next(a).total_cmp(&next(b))
            })?;
        sizes[cheapest] += 1;
    }
    let sets: [Vec<u8>; INTERVALS] = std::array::from_fn(|i| ladders[i][..sizes[i]].to_vec());
    (average_power(law, &sets, k) <= limit).then_some(sets)
}

/// The sets carrying `k` bits with the most room between levels the power
/// allows, if that room is at least `least` of the noise.
fn widest(route: &Route, law: Law, k: u32, least: f64, limit: f64) -> Option<[Vec<u8>; INTERVALS]> {
    let noise = route.noise_at(law, limit.sqrt() / 32768.0);
    let build = |factor: f64| quietest(route, law, k, factor * noise, limit);
    let mut best = build(least)?;
    let (mut low, mut high) = (least, 64.0 * least);
    for _ in 0..24 {
        let middle = 0.5 * (low + high);
        match build(middle) {
            Some(sets) => {
                best = sets;
                low = middle;
            }
            None => high = middle,
        }
    }
    Some(best)
}

/// Choose CP and CPt for a route.
///
/// Data mode first: the fastest rate the digital modem's Jd enables whose
/// constellations can stand [`SPACING`] noises apart inside Table 15's power,
/// and then as far apart as that power allows -- the margin a rate can have
/// is margin it should have. Phase 4's the same way from Table 17's rates, at
/// twice the spacing, and never so much quieter than data mode's that data
/// mode is more than "3 dB above" it (8.5.2).
///
/// None if the route cannot carry V.90's slowest rate.
pub fn choose(route: &Route, law: Law, limit: u32, enabled: impl Fn(u8) -> bool) -> Option<Choice> {
    let limit = f64::from(limit).powi(2);
    let s = Redundancy::None.data_bits() as u32;
    let (k_low, k_high) = (super::D_RANGE.0 - s, super::largest_k(s));
    let (k, sets) = (k_low..=k_high)
        .rev()
        .filter(|&k| enabled((k + s - 20) as u8))
        .find_map(|k| widest(route, law, k, SPACING, limit).map(|sets| (k, sets)))?;
    let data_power = average_power(law, &sets, k);
    let data = cp_for(&sets, (k + s - 20) as u8, true);

    // Table 17: K from 6 to 24 with S at 6.
    let (k, training_sets) = (6..=24u32)
        .rev()
        .find_map(|k| widest(route, law, k, 2.0 * SPACING, limit).filter(|sets| 2.0 * average_power(law, sets, k) >= data_power).map(|sets| (k, sets)))
        .or_else(|| (6..=24u32).rev().find_map(|k| widest(route, law, k, 2.0 * SPACING, limit).map(|sets| (k, sets))))?;
    let training = cp_for(&training_sets, (k + s - 8) as u8, false);
    Some(Choice { data, training })
}

/// What choosing made of a route, a line to a rate, for when it chose
/// nothing or not much.
pub fn explain(route: &Route, law: Law, limit: u32) -> Vec<String> {
    let noise = route.noise_at(law, f64::from(limit) / 32768.0);
    let limit = f64::from(limit).powi(2);
    let mut out = vec![format!("noise {noise:.2e}, ceiling {:.0}", limit.sqrt())];
    for i in 0..INTERVALS {
        let l = ladder(route, law, i, SPACING * noise);
        out.push(format!("interval {i}: {} rungs at {:.0} apart, from {:?}", l.len(), SPACING * noise * 32768.0, &l[..l.len().min(8)]));
    }
    for k in [15u32, 20, 25, 30, 36] {
        match quietest(route, law, k, SPACING * noise, f64::INFINITY) {
            Some(sets) => out.push(format!("K {k}: power {:.0}, sizes {:?}", average_power(law, &sets, k).sqrt(), sets.iter().map(Vec::len).collect::<Vec<_>>())),
            None => out.push(format!("K {k}: the ladders cannot carry it")),
        }
    }
    out
}

/// A CP for these sets: one mask for each different set.
fn cp_for(sets: &[Vec<u8>; INTERVALS], drn: u8, data_mode: bool) -> Cp {
    let mut constellations: Vec<Mask> = Vec::new();
    let mut intervals = [0u8; INTERVALS];
    for (i, set) in sets.iter().enumerate() {
        let mask = set.iter().fold(0 as Mask, |m, &u| m | 1 << u);
        let index = constellations.iter().position(|&c| c == mask).unwrap_or_else(|| {
            constellations.push(mask);
            constellations.len() - 1
        });
        intervals[i] = index as u8;
    }
    Cp { data_mode, drn, intervals, constellations, ..Cp::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::encoder::Mapping;

    #[test]
    fn our_dil_asks_for_every_codeword_in_six_frames_each() {
        let d = design(Law::Mu, 79);
        // Every codeword once up to a third of full scale, bar UINFO, which
        // the references read.
        let mut ucodes = d.ucodes.clone();
        ucodes.sort_unstable();
        assert_eq!(ucodes, (0..99u8).filter(|&u| u != 79).collect::<Vec<_>>());
        assert!(ucode::level(Law::Mu, 98) <= LOUDEST && ucode::level(Law::Mu, 99) > LOUDEST);
        assert_eq!(d.len(), 98 * 36);
        let bits = d.to_bits();
        assert_eq!(Descriptor::from_bits(&bits), Some(d.clone()));
        // A frame of references and five of the codeword, in every segment.
        let symbols: Vec<(u8, bool)> = d.symbols().collect();
        let fiftieth = d.ucodes[50];
        assert!(symbols[36 * 50..36 * 50 + 6].iter().all(|&(u, _)| u == 79));
        assert!(symbols[36 * 50 + 6..36 * 51].iter().all(|&(u, _)| u == fiftieth));
        // And no two neighbours alike.
        assert!(d.ucodes.windows(2).all(|w| w[0].abs_diff(w[1]) >= 3), "{:?}", d.ucodes);
        // Signs about balanced.
        let positive = d.signs.iter().filter(|b| **b).count();
        assert!((12..=24).contains(&positive), "{positive} of 36 positive");
    }

    /// A route with a robbed bit in interval 3: every octet there arrives
    /// with its least significant bit set, which under mu-law moves every
    /// other Ucode onto its neighbour.
    fn robbed(u: u8) -> u8 {
        let octet = ucode::octet(Law::Mu, u, false) | 1;
        ucode::from_octet(Law::Mu, octet).0
    }

    fn analyse(noise: f64, rob: bool) -> Route {
        let d = design(Law::Mu, 79);
        let mut analysis = Analysis::new();
        let mut x = 0x1234_5678_9abc_def0u64;
        for (n, (u, positive)) in d.symbols().enumerate() {
            let interval = n % INTERVALS;
            let arrived = if rob && interval == 3 { robbed(u) } else { u };
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let gaussian = ((x >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 12f64.sqrt() * noise;
            let level = ucode::level(Law::Mu, arrived) * if positive { 1.0 } else { -1.0 };
            analysis.feed(u, positive, interval, level + gaussian);
        }
        analysis.route()
    }

    #[test]
    fn the_analysis_reads_the_levels_and_the_noise() {
        let route = analyse(0.002, false);
        assert!((route.noise() / 0.002 - 1.0).abs() < 0.2, "noise read as {}", route.noise());
        for i in 0..INTERVALS {
            // UINFO read off the references, and the loudest the DIL asks for.
            for u in [0u8, 40, 79, 98] {
                let want = ucode::level(Law::Mu, u);
                assert!((route.levels[i][usize::from(u)] - want).abs() < 0.003, "interval {i} code {u}");
            }
        }
    }

    #[test]
    fn a_robbed_bit_halves_one_interval_s_ladder() {
        let route = analyse(0.0003, true);
        let spacing = SPACING * route.noise();
        let clean = ladder(&route, Law::Mu, 0, spacing);
        let robbed_set = ladder(&route, Law::Mu, 3, spacing);
        let top = |set: &[u8]| set.iter().filter(|&&u| u >= 64).count();
        assert!(top(&clean) >= 30, "{clean:?}");
        assert!(top(&robbed_set) <= top(&clean) / 2 + 1, "{robbed_set:?}");
        // Below Uchord 2 a robbed bit moves a codeword by less than the noise
        // here, and nothing can tell which of a pair arrived as itself.
        for &u in robbed_set.iter().filter(|&&u| u >= 16) {
            assert_eq!(robbed(u), u, "{u} is not one the robbed bit leaves alone");
        }
        // And a route like it still gets a choice. Interval 3 need not come
        // out shorter: a constellation spaced wider than two codewords skips
        // every other one anyway, and the robbed bit then costs nothing but
        // which of each pair is used.
        let choice = choose(&route, Law::Mu, 15124, |_| true).expect("no choice on a robbed route");
        for &u in &choice.data.points(3) {
            assert_eq!(robbed(u), u);
        }
    }

    #[test]
    fn a_robbed_bit_halves_one_interval_s_constellation() {
        let route = analyse(0.0003, true);
        let clean = route.constellation(0, SPACING, Law::Mu);
        let robbed_set = route.constellation(3, SPACING, Law::Mu);
        // The loud end, where the codes are sparse enough to use them all,
        // loses half of itself.
        let top = |set: &[u8]| set.iter().filter(|&&u| u >= 64).count();
        assert!(top(&clean) >= 30, "{clean:?}");
        assert!(top(&robbed_set) <= top(&clean) / 2 + 1, "{robbed_set:?}");
        // And what is left in it arrives as itself.
        for &u in &robbed_set {
            assert_eq!(robbed(u), u, "{u} is not one the robbed bit leaves alone");
        }
    }

    /// 8.5.2's formula against a count done the long way.
    #[test]
    fn the_average_power_formula_counts_every_message() {
        let sets: [Vec<u8>; INTERVALS] =
            [vec![10, 20, 30], vec![5, 60], vec![100, 101, 102, 103, 104], vec![1], vec![7, 8], vec![90, 91, 92]];
        let k = 5;
        let formula = average_power(Law::Mu, &sets, k);
        let moduli: Moduli = std::array::from_fn(|i| sets[i].len() as u16);
        let mut brute = 0.0;
        for m in 0..1u32 << k {
            let bits: Vec<bool> = (0..k).map(|b| m >> b & 1 == 1).collect();
            let labels = crate::v90::modulus::encode(&bits, moduli);
            for i in 0..INTERVALS {
                let mut points = sets[i].clone();
                points.sort_unstable_by(|a, b| b.cmp(a));
                let linear = f64::from(ucode::linear(Law::Mu, points[usize::from(labels[i])]));
                brute += linear * linear;
            }
        }
        brute /= INTERVALS as f64 * f64::from(1u32 << k);
        assert!((formula - brute).abs() < 1e-6 * brute, "{formula} against {brute}");
    }

    /// At a given rate, the room between levels is as wide as the power
    /// lets it be, and never narrower than the noise demands.
    #[test]
    fn the_chosen_levels_stand_well_clear_of_the_noise() {
        let noise = 0.0002;
        let route = Route::clean(Law::Mu, noise);
        let choice = choose(&route, Law::Mu, 4024, |_| true).unwrap();
        for i in 0..INTERVALS {
            let mut levels: Vec<f64> = choice.data.points(i).iter().map(|&u| ucode::level(Law::Mu, u)).collect();
            levels.sort_by(f64::total_cmp);
            let closest = levels.windows(2).map(|w| w[1] - w[0]).fold(f64::INFINITY, f64::min);
            assert!(closest >= SPACING * noise, "interval {i}: {closest} apart");
            assert!(2.0 * levels[0] >= SPACING * noise, "interval {i}: {} from its opposite", 2.0 * levels[0]);
        }
    }

    #[test]
    fn a_clean_quiet_route_reaches_the_top_of_the_ladder() {
        let route = Route::clean(Law::Mu, 0.00005);
        let choice = choose(&route, Law::Mu, 15124, |_| true).expect("nothing to choose");
        let data = Mapping::from_cp(&choice.data).expect("the data CP is not a mapping");
        assert_eq!(data.rate(), 56_000);
        let training = Mapping::from_cp(&choice.training).expect("the CPt is not a mapping");
        assert!(training.frame_bits() <= 30);
        // Within the power it was given.
        assert!(average_power(Law::Mu, &std::array::from_fn(|i| choice.data.points(i)), data.k) <= 15124f64.powi(2));
    }

    #[test]
    fn a_noisier_route_and_a_lower_ceiling_come_out_slower() {
        // Quiet enough for 56 000 at full power, but only because the loud
        // codewords are there to use: with next to no noise at all, the quiet
        // ones would carry it too, and the ceiling would cost nothing.
        let quiet = choose(&Route::clean(Law::Mu, 0.0004), Law::Mu, 15124, |_| true).unwrap();
        let noisy = choose(&Route::clean(Law::Mu, 0.003), Law::Mu, 15124, |_| true).unwrap();
        let low = choose(&Route::clean(Law::Mu, 0.0004), Law::Mu, 2540, |_| true).unwrap();
        let rate = |c: &Choice| Mapping::from_cp(&c.data).unwrap().rate();
        assert!(rate(&noisy) < rate(&quiet), "{} against {}", rate(&noisy), rate(&quiet));
        assert!(rate(&low) < rate(&quiet));
        println!("quiet {} noisy {} at -16 dBm0 {}", rate(&quiet), rate(&noisy), rate(&low));
        // And only enabled rates are asked for.
        let odd = choose(&Route::clean(Law::Mu, 0.00005), Law::Mu, 15124, |drn| drn % 2 == 1).unwrap();
        assert_eq!(odd.data.drn % 2, 1);
    }

    #[test]
    fn a_route_too_noisy_for_28000_is_refused() {
        assert_eq!(choose(&Route::clean(Law::Mu, 0.05), Law::Mu, 15124, |_| true), None);
    }
}
