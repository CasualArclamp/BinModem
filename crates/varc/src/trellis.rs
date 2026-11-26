//! A trellis code and a shell mapper for the single carrier.
//!
//! [`crate::sc`] put a whole number of bits on every symbol, on a square or
//! a rectangle of points, each point for itself. That wants about eleven
//! decibels between the points and the noise, and it throws away whatever
//! part of a bit the line could have carried. This is what a V.34 does
//! about both, in the simplest form that does it:
//!
//! - **A trellis code.** The grid has twice the points the data needs, cut
//!   into eight subsets (Ungerboeck, "Channel coding with multilevel/phase
//!   signals", 1982). A convolutional code of rate two-thirds says which
//!   subset each symbol's point is in, so that only certain runs of
//!   subsets are ever sent, and the receiver looks for the run nearest to
//!   what arrived and not for the point nearest to each symbol. Points
//!   within a subset are 9 dB further apart than neighbours are, so what
//!   picks the point inside the subset needs no code of its own.
//! - **A shell mapper** (V.34 9.4). The points of each subset are put in
//!   rings by their energy. A frame of eight symbols has a number, and the
//!   number picks the eight rings: the numbers there are go to the frames
//!   of least total energy. So the outer rings are used less than the inner
//!   ones, which is most of a decibel, and a frame may carry any whole
//!   number of bits, which is an eighth of a bit to the symbol.
//!
//! The code here is not V.34's. V.34's is four-dimensional and turns with
//! the constellation, because a V.34 has to survive a quarter turn. A V.ARC
//! receiver has a sync word to say which way up it is, so this is the plain
//! two-dimensional code, with sixty-four states.
//!
//! A block of symbols is coded for itself: the code starts from nothing
//! and its last four symbols steer it back to nothing, so the receiver
//! knows where every block's run begins and ends.

use dsp::Complex;

use crate::map::{BitReader, BitWriter};

/// Delay elements in the encoder: sixty-four states.
const NU: u32 = 6;

/// How many states the code has.
pub const STATES: usize = 1 << NU;

/// The code's parity-check polynomials, in octal as Ungerboeck's tables
/// have them. Found by a search of every code of this size for the greatest
/// free distance and then the fewest paths at it: the squared distance is
/// 7 where neighbouring points are 1 apart, against 2 for a grid with half
/// the points and no code, which is 5.4 dB.
const H0: u32 = 0o105;
const H1: u32 = 0o032;
const H2: u32 = 0o060;

/// Symbols at the end of a block that steer the code back to nothing. They
/// carry none of the two bits a symbol that the code reads.
pub const TAIL: usize = 4;

/// Symbols in a shell mapper's frame.
pub const FRAME: usize = 8;

/// The most data bits a frame may carry: twelve and a half a symbol.
pub const MOST: u16 = 100;

/// The fewest: three a symbol, which is two for the code and one beside.
pub const LEAST: u16 = 24;

/// One step of the encoder: the state it is in and the two bits it reads,
/// to the bit it adds and the state it goes to. The bit it adds is the
/// state's own, so every way out of a state is in the same half of the
/// grid.
fn step(state: u8, read: u8) -> (u8, u8) {
    let (z1, z2) = (u32::from(read) & 1, u32::from(read) >> 1 & 1);
    let z0 = u32::from(state) & 1;
    let mut next = u32::from(state) >> 1;
    for k in 1..=NU {
        let bit = (H0 >> k & 1 & z0) ^ (H1 >> k & 1 & z1) ^ (H2 >> k & 1 & z2);
        next ^= bit << (k - 1);
    }
    (z0 as u8, next as u8)
}

/// Which of the eight subsets a point is in, by its place on the grid of
/// odd whole numbers: the bit the code adds, then the two it reads.
///
/// The first bit halves the grid into the two colours of a chessboard, the
/// second halves each of those into squares twice the size, and the third
/// halves each of those into chessboards again. Points that share all
/// three are four grid steps apart along the diagonals.
fn subset(x: i32, y: i32) -> usize {
    let (a, b) = ((x - 1) >> 1, (y - 1) >> 1);
    let (alpha, beta) = (a & 1, b & 1);
    let gamma = ((a >> 1) + (b >> 1)) & 1;
    ((alpha ^ beta) | alpha << 1 | gamma << 2) as usize
}

/// What a receiver makes of one symbol for one subset: the nearest of the
/// subset's points, by its place among them, and how far off it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct Near {
    pub rank: u16,
    pub distance: f32,
}

/// A grid for one number of bits to the frame, with its rings and the
/// tables its shell mapper counts by.
#[derive(Debug, Clone)]
pub struct Grid {
    /// Data bits a frame of eight symbols carries.
    frame_bits: u16,
    /// Of those, the bits that pick the eight rings.
    shell_bits: u32,
    /// Bits that pick a point within its ring, each symbol.
    inner_bits: u32,
    rings: usize,
    /// The points of each subset, innermost first, as odd whole numbers.
    points: [Vec<(i16, i16)>; 8],
    /// Where each place on the grid comes in its subset's points, over a
    /// square of `2 * half` places a side; `u16::MAX` for a place that is
    /// not a point.
    rank: Vec<u16>,
    half: i32,
    /// What to multiply the odd whole numbers by for unit power.
    scale: f64,
    /// The furthest any point is from the middle, along either axis, at
    /// unit power.
    reach: f64,
    /// Ways to make each total from two rings, from four, and from eight;
    /// and frames with a total below each.
    g2: Vec<u64>,
    g4: Vec<u64>,
    g8: Vec<u64>,
    below: Vec<u64>,
    /// For each state, what to read over the last symbols of a block to
    /// leave the code in the state it started in.
    home: Vec<[u8; TAIL]>,
}

fn convolve(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = vec![0u64; a.len() + b.len() - 1];
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            out[i + j] += x * y;
        }
    }
    out
}

impl Grid {
    /// The grid for `frame_bits` data bits to a frame of eight symbols,
    /// between [`LEAST`] and [`MOST`].
    pub fn new(frame_bits: u16) -> Self {
        let frame_bits = frame_bits.clamp(LEAST, MOST);
        // Two bits a symbol are the code's. Of the rest, three a symbol or
        // so pick the rings, since it is the rings that shape the signal
        // and eight or more of them do nearly all the shaping there is to
        // do; what is left over picks a point inside its ring.
        let rest = u32::from(frame_bits) - 2 * FRAME as u32;
        let inner_bits = (rest / FRAME as u32).saturating_sub(3);
        let shell_bits = rest - FRAME as u32 * inner_bits;
        // A fifth more rings than the bits need, so that the mapper has
        // frames to leave out, and the ones it leaves out are the loud
        // ones. V.34 does the same and calls the grid expanded.
        let rings = if shell_bits == 0 { 1 } else { (1.2 * 2f64.powf(f64::from(shell_bits) / FRAME as f64)).ceil() as usize };
        let each = rings << inner_bits;

        // Every point near enough the middle to be wanted, by subset and
        // then by energy.
        let half = ((4.0 * each as f64 * 8.0 / std::f64::consts::PI).sqrt().ceil() as i32 / 2 + 6) & !1;
        let mut points: [Vec<(i16, i16)>; 8] = Default::default();
        for a in -half..half {
            for b in -half..half {
                let (x, y) = (2 * a + 1, 2 * b + 1);
                points[subset(x, y)].push((x as i16, y as i16));
            }
        }
        for list in &mut points {
            list.sort_by_key(|&(x, y)| (i32::from(x) * i32::from(x) + i32::from(y) * i32::from(y), x, y));
            list.truncate(each);
        }
        let side = (2 * half) as usize;
        let mut rank = vec![u16::MAX; side * side];
        let mut furthest = 0i32;
        for list in &points {
            for (place, &(x, y)) in list.iter().enumerate() {
                let (a, b) = ((i32::from(x) - 1) >> 1, (i32::from(y) - 1) >> 1);
                rank[(a + half) as usize * side + (b + half) as usize] = place as u16;
                furthest = furthest.max(i32::from(x).abs()).max(i32::from(y).abs());
            }
        }

        let g1 = vec![1u64; rings];
        let g2 = convolve(&g1, &g1);
        let g4 = convolve(&g2, &g2);
        let g8 = convolve(&g4, &g4);
        let mut below = Vec::with_capacity(g8.len() + 1);
        let mut sum = 0u64;
        for count in &g8 {
            below.push(sum);
            sum += count;
        }
        below.push(sum);

        // How often each ring is used: in every frame whose total is below
        // the last total the bits reach, and in that share of the frames at
        // the last total.
        let g7 = convolve(&g4, &convolve(&g2, &g1));
        let frames = 1u64 << shell_bits;
        let last = below.iter().rposition(|&b| b < frames).unwrap_or(0).min(g8.len() - 1);
        let part = (frames - below[last]) as f64 / g8[last] as f64;
        let mut used = vec![0.0; rings];
        for (ring, used) in used.iter_mut().enumerate() {
            for total in ring..=last {
                let ways = g7.get(total - ring).copied().unwrap_or(0) as f64;
                *used += if total == last { part * ways } else { ways };
            }
        }
        let all: f64 = used.iter().sum();
        let mut power = 0.0;
        for (ring, used) in used.iter().enumerate() {
            let mut energy = 0.0;
            for list in &points {
                for &(x, y) in &list[ring << inner_bits..(ring + 1) << inner_bits] {
                    energy += f64::from(x) * f64::from(x) + f64::from(y) * f64::from(y);
                }
            }
            power += used / all * energy / (8 << inner_bits) as f64;
        }
        let scale = 1.0 / power.sqrt();

        // What steers each state home: every run of four symbols tried.
        // Three do not reach every state.
        let home = (0..STATES as u8)
            .map(|from| {
                (0..1u16 << (2 * TAIL))
                    .map(|run| std::array::from_fn(|i| (run >> (2 * i) & 3) as u8))
                    .find(|run: &[u8; TAIL]| run.iter().fold(from, |state, &read| step(state, read).1) == 0)
                    .expect("four symbols bring any state home")
            })
            .collect();

        Self { frame_bits, shell_bits, inner_bits, rings, points, rank, half, scale, reach: f64::from(furthest) * scale, g2, g4, g8, below, home }
    }

    /// Data bits a frame carries.
    pub fn frame_bits(&self) -> u16 {
        self.frame_bits
    }

    /// Points in the grid: twice what the data needs, and a fifth more.
    pub fn size(&self) -> usize {
        8 * (self.rings << self.inner_bits)
    }

    /// How far the outermost point is from the middle along an axis, with
    /// the grid at unit power: what a diagram of it is scaled by.
    pub fn reach(&self) -> f64 {
        self.reach
    }

    /// Data bits a block of `symbols` symbols carries: its frames, less
    /// what the last symbols give up to steer the code home.
    pub fn block_bits(frame_bits: u16, symbols: usize) -> usize {
        (symbols / FRAME * usize::from(frame_bits)).saturating_sub(2 * TAIL)
    }

    /// The point at `rank` of `subset`, at unit power.
    fn point(&self, subset: usize, rank: usize) -> Complex {
        let (x, y) = self.points[subset][rank];
        Complex::new(f64::from(x) * self.scale, f64::from(y) * self.scale)
    }

    /// The eight rings for a frame's number: the frames of least total
    /// come first, and among those of one total they are counted off by
    /// halves, as V.34 9.4 has it.
    fn rings_for(&self, mut number: u64) -> [usize; FRAME] {
        let total = self.below.iter().rposition(|&b| b <= number).unwrap_or(0).min(self.g8.len() - 1);
        number -= self.below[total];
        let mut out = [0; FRAME];
        self.halve(&self.g4, total, number, 4, &mut out);
        out
    }

    /// Split `total` between the two halves of a run of `2 * half` rings,
    /// the `number`th way there is, and go on down to pairs. `ways` is how
    /// many ways there are to make each total from `half` rings.
    fn halve(&self, ways: &[u64], total: usize, mut number: u64, half: usize, out: &mut [usize]) {
        let count = |part: usize| ways.get(part).copied().unwrap_or(0);
        let mut first = 0;
        loop {
            let here = count(first) * count(total - first);
            if number < here {
                break;
            }
            number -= here;
            first += 1;
        }
        let (low, high) = (number % count(first), number / count(first));
        let (left, right) = out.split_at_mut(half);
        if half == 2 {
            self.pair(first, low, left);
            self.pair(total - first, high, right);
        } else {
            self.halve(&self.g2, first, low, half / 2, left);
            self.halve(&self.g2, total - first, high, half / 2, right);
        }
    }

    /// The `number`th pair of rings that add up to `total`.
    fn pair(&self, total: usize, number: u64, out: &mut [usize]) {
        let least = total.saturating_sub(self.rings - 1);
        out[0] = least + number as usize;
        out[1] = total - out[0];
    }

    /// The number of a frame's eight rings: [`Self::rings_for`] backwards.
    fn number_of(&self, rings: &[usize; FRAME]) -> u64 {
        let pair = |a: usize, b: usize| (a + b, (a - (a + b).saturating_sub(self.rings - 1)) as u64);
        let join = |ways: &[u64], (first, low): (usize, u64), (second, high): (usize, u64)| {
            let count = |part: usize| ways.get(part).copied().unwrap_or(0);
            let total = first + second;
            let before: u64 = (0..first).map(|part| count(part) * count(total - part)).sum();
            (total, before + low + high * count(first))
        };
        let first = join(&self.g2, pair(rings[0], rings[1]), pair(rings[2], rings[3]));
        let second = join(&self.g2, pair(rings[4], rings[5]), pair(rings[6], rings[7]));
        let (total, number) = join(&self.g4, first, second);
        self.below[total] + number
    }

    /// The symbols of one block, from the bits it carries.
    pub fn encode(&self, reader: &mut BitReader, symbols: usize) -> Vec<Complex> {
        let mut out = Vec::with_capacity(symbols);
        let mut state = 0u8;
        let frames = symbols / FRAME;
        for frame in 0..frames {
            let rings = self.rings_for(u64::from(reader.take(self.shell_bits)));
            for (i, &ring) in rings.iter().enumerate() {
                let at = frame * FRAME + i;
                let read = if at + TAIL >= frames * FRAME {
                    // The way home, worked out where it starts.
                    let left = frames * FRAME - at;
                    if left == TAIL {
                        self.tail_from(state, &mut out, reader, &rings[i..]);
                        break;
                    }
                    0
                } else {
                    reader.take(2) as u8
                };
                let (added, next) = step(state, read);
                state = next;
                let inner = reader.take(self.inner_bits) as usize;
                out.push(self.point(usize::from(added | read << 1), ring << self.inner_bits | inner));
            }
        }
        out
    }

    /// The last symbols of a block: what the code reads is whatever takes
    /// it home, and the rest of each symbol is data as before.
    fn tail_from(&self, mut state: u8, out: &mut Vec<Complex>, reader: &mut BitReader, rings: &[usize]) {
        let way = self.home[usize::from(state)];
        for (&read, &ring) in way.iter().zip(rings) {
            let (added, next) = step(state, read);
            state = next;
            let inner = reader.take(self.inner_bits) as usize;
            out.push(self.point(usize::from(added | read << 1), ring << self.inner_bits | inner));
        }
    }

    /// The nearest point of each subset to what arrived, and which of the
    /// eight is nearest of all, with its point.
    pub fn nearest(&self, z: Complex) -> ([Near; 8], Complex) {
        let (u, v) = (z.re / self.scale, z.im / self.scale);
        let mut out = [Near::default(); 8];
        let mut best = (f64::MAX, Complex::ZERO);
        for (which, near) in out.iter_mut().enumerate() {
            let (alpha, beta, gamma) = ((which >> 1 & 1) as i32, ((which >> 1 ^ which) & 1) as i32, (which >> 2 & 1) as i32);
            // The subset is a chessboard of squares four steps a side:
            // the nearest square, and if it is the wrong colour, the one
            // next to it on the side the point leans to.
            let (ta, tb) = ((u - f64::from(2 * alpha + 1)) / 4.0, (v - f64::from(2 * beta + 1)) / 4.0);
            let (mut ra, mut rb) = (ta.round() as i32, tb.round() as i32);
            if (ra + rb - gamma) & 1 != 0 {
                if (ta - f64::from(ra)).abs() > (tb - f64::from(rb)).abs() {
                    ra += if ta > f64::from(ra) { 1 } else { -1 };
                } else {
                    rb += if tb > f64::from(rb) { 1 } else { -1 };
                }
            }
            let (x, y) = (4 * ra + 2 * alpha + 1, 4 * rb + 2 * beta + 1);
            let found = self.rank_at(x, y).map(|rank| (rank, x, y)).or_else(|| self.inside(which, u, v));
            let Some((rank, x, y)) = found else {
                *near = Near { rank: 0, distance: f32::MAX };
                continue;
            };
            let (du, dv) = (u - f64::from(x), v - f64::from(y));
            let distance = (du * du + dv * dv) * self.scale * self.scale;
            *near = Near { rank, distance: distance as f32 };
            if distance < best.0 {
                best = (distance, Complex::new(f64::from(x) * self.scale, f64::from(y) * self.scale));
            }
        }
        (out, best.1)
    }

    fn rank_at(&self, x: i32, y: i32) -> Option<u16> {
        let (a, b) = ((x - 1) >> 1, (y - 1) >> 1);
        let side = 2 * self.half;
        if a < -self.half || a >= self.half || b < -self.half || b >= self.half {
            return None;
        }
        let rank = self.rank[((a + self.half) * side + (b + self.half)) as usize];
        (rank != u16::MAX).then_some(rank)
    }

    /// The nearest point of a subset to somewhere outside the grid: every
    /// point of the subset looked at. Rare, and only at the grid's edge.
    fn inside(&self, which: usize, u: f64, v: f64) -> Option<(u16, i32, i32)> {
        self.points[which]
            .iter()
            .enumerate()
            .map(|(rank, &(x, y))| {
                let (du, dv) = (u - f64::from(x), v - f64::from(y));
                (du * du + dv * dv, rank as u16, i32::from(x), i32::from(y))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, rank, x, y)| (rank, x, y))
    }

    /// The bits of one block, from what the receiver made of each of its
    /// symbols, and the mean squared distance between what arrived and
    /// what was decided on: the noise, as the code sees it.
    ///
    /// Viterbi's algorithm: the run of subsets the code allows that is
    /// nearest to what arrived, from the state the code starts every block
    /// in to that state again.
    pub fn decode(&self, heard: &[[Near; 8]], writer: &mut BitWriter) -> f64 {
        let symbols = heard.len() / FRAME * FRAME;
        if symbols == 0 {
            return 0.0;
        }
        let mut cost = [f32::MAX; STATES];
        cost[0] = 0.0;
        // For each symbol and state: where the best way in came from, and
        // the two bits that were read on it.
        let mut back = vec![[0u8; STATES]; symbols];
        for (near, back) in heard[..symbols].iter().zip(back.iter_mut()) {
            let mut next = [f32::MAX; STATES];
            for state in 0..STATES as u8 {
                let so_far = cost[usize::from(state)];
                if so_far == f32::MAX {
                    continue;
                }
                for read in 0..4u8 {
                    let (added, to) = step(state, read);
                    let total = so_far + near[usize::from(added | read << 1)].distance;
                    if total < next[usize::from(to)] {
                        next[usize::from(to)] = total;
                        back[usize::from(to)] = state << 2 | read;
                    }
                }
            }
            cost = next;
        }
        let mut state = 0u8;
        let mut reads = vec![0u8; symbols];
        let mut ranks = vec![0u16; symbols];
        let mut off = 0.0;
        for at in (0..symbols).rev() {
            let step_back = back[at][usize::from(state)];
            let (from, read) = (step_back >> 2, step_back & 3);
            let near = heard[at][usize::from((from & 1) | read << 1)];
            reads[at] = read;
            ranks[at] = near.rank;
            off += f64::from(near.distance);
            state = from;
        }
        let mask = (1u16 << self.inner_bits) - 1;
        for frame in 0..symbols / FRAME {
            let at = frame * FRAME;
            let rings: [usize; FRAME] = std::array::from_fn(|i| usize::from(ranks[at + i] >> self.inner_bits).min(self.rings - 1));
            writer.put(self.number_of(&rings) as u32, self.shell_bits);
            for i in at..at + FRAME {
                if i + TAIL < symbols {
                    writer.put(u32::from(reads[i]), 2);
                }
                writer.put(u32::from(ranks[i] & mask), self.inner_bits);
            }
        }
        off / symbols as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Noise(u64);

    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }

        fn gaussian(&mut self) -> f64 {
            (-2.0 * self.uniform().ln()).sqrt() * (std::f64::consts::TAU * self.uniform()).cos()
        }
    }

    #[test]
    fn the_codes_free_distance_is_what_the_search_found() {
        // The least a run of subsets the code allows can differ from the
        // run of all noughts, by Dijkstra's algorithm over the states. A
        // step that differs in the first bit is 1 off, in the second and
        // not the first 2, in the third alone 4.
        let off = |label: u8| if label & 1 != 0 { 1 } else if label & 2 != 0 { 2 } else if label & 4 != 0 { 4 } else { 0 };
        let mut best = [u32::MAX; STATES];
        let mut open: Vec<(u32, u8)> = (1..4u8).map(|read| (off(step(0, read).0 | read << 1), step(0, read).1)).collect();
        let mut free = u32::MAX;
        while let Some(at) = (0..open.len()).min_by_key(|&i| open[i].0) {
            let (distance, state) = open.swap_remove(at);
            if state == 0 {
                free = distance;
                break;
            }
            if best[usize::from(state)] <= distance {
                continue;
            }
            best[usize::from(state)] = distance;
            for read in 0..4u8 {
                let (added, next) = step(state, read);
                open.push((distance + off(added | read << 1), next));
            }
        }
        assert_eq!(free, 7);
    }

    #[test]
    fn points_of_one_subset_are_four_steps_apart_along_the_diagonals() {
        let grid = Grid::new(80);
        for list in &grid.points {
            let least = list
                .iter()
                .flat_map(|a| list.iter().filter(move |b| *b != a).map(move |b| (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)))
                .min()
                .unwrap();
            // Odd whole numbers are two apart, so neighbours are 4 off in
            // these units and a subset's points 32.
            assert_eq!(least, 32);
        }
        // And neighbours on the grid are never in one subset, nor in the
        // same half: the first bit is a chessboard.
        assert_ne!(subset(1, 1) & 1, subset(3, 1) & 1);
        assert_ne!(subset(1, 1) & 1, subset(1, -1) & 1);
        assert_eq!(subset(1, 1) & 1, subset(3, 3) & 1);
    }

    #[test]
    fn every_frame_number_has_its_own_rings_and_the_quiet_ones_come_first() {
        for frame_bits in [24, 27, 29, 32] {
            let grid = Grid::new(frame_bits);
            let mut seen = std::collections::HashSet::new();
            let mut last = 0;
            for number in 0..1u64 << grid.shell_bits {
                let rings = grid.rings_for(number);
                assert!(rings.iter().all(|&ring| ring < grid.rings), "{frame_bits}: {rings:?}");
                assert!(seen.insert(rings), "{frame_bits}: {number} repeats {rings:?}");
                assert_eq!(grid.number_of(&rings), number, "{frame_bits}: {rings:?}");
                let total: usize = rings.iter().sum();
                assert!(total >= last, "{frame_bits}: {number} is quieter than the one before");
                last = total;
            }
        }
        // And the larger ones, which there are too many frames of to try
        // them all.
        let mut noise = Noise(3);
        for frame_bits in [36, 40, 47, 56, 71, 84, 92, MOST] {
            let grid = Grid::new(frame_bits);
            for _ in 0..20_000 {
                let number = (noise.uniform() * (1u64 << grid.shell_bits) as f64) as u64;
                let rings = grid.rings_for(number);
                assert!(rings.iter().all(|&ring| ring < grid.rings));
                assert_eq!(grid.number_of(&rings), number, "{frame_bits}");
            }
        }
    }

    /// A block through the code and back, with noise at `snr_db` on the
    /// way: how many of its octets came back wrong.
    fn through(grid: &Grid, symbols: usize, snr_db: f64, noise: &mut Noise) -> (usize, usize, f64) {
        let octets = Grid::block_bits(grid.frame_bits(), symbols) / 8;
        let sent: Vec<u8> = (0..octets).map(|_| (noise.uniform() * 256.0) as u8).collect();
        let points = grid.encode(&mut BitReader::new(sent.clone()), symbols);
        assert_eq!(points.len(), symbols);
        let power: f64 = points.iter().map(|p| p.norm_sqr()).sum::<f64>() / symbols as f64;
        let sigma = (10f64.powf(-snr_db / 10.0) / 2.0).sqrt();
        let heard: Vec<[Near; 8]> =
            points.iter().map(|&p| grid.nearest(p + Complex::new(sigma * noise.gaussian(), sigma * noise.gaussian())).0).collect();
        let mut writer = BitWriter::default();
        grid.decode(&heard, &mut writer);
        let got = writer.finish(octets);
        (sent.iter().zip(&got).filter(|(a, b)| a != b).count(), octets, power)
    }

    #[test]
    fn a_block_comes_back_as_it_went_and_at_unit_power() {
        let mut noise = Noise(11);
        for frame_bits in [LEAST, 31, 40, 57, 64, 77, 83, 88, 96, MOST] {
            let grid = Grid::new(frame_bits);
            let (mut wrong, mut power, mut blocks) = (0, 0.0, 0.0);
            for symbols in [248, 496, 3968] {
                let (bad, _, p) = through(&grid, symbols, 200.0, &mut noise);
                wrong += bad;
                power += p;
                blocks += 1.0;
            }
            assert_eq!(wrong, 0, "{frame_bits} bits a frame");
            let power: f64 = power / blocks;
            assert!((10.0 * power.log10()).abs() < 0.15, "{frame_bits} bits a frame: power {power}");
        }
    }

    #[test]
    fn the_code_reads_through_noise_that_a_bare_grid_does_not() {
        // Ten and a half bits a symbol at 37 dB. A square grid of that
        // many points and no code wants about 42 dB for one symbol in a
        // hundred thousand wrong.
        let mut noise = Noise(5);
        let grid = Grid::new(84);
        let (mut wrong, mut all) = (0, 0);
        for _ in 0..40 {
            let (bad, octets, _) = through(&grid, 248, 37.0, &mut noise);
            wrong += bad;
            all += octets;
        }
        assert!(wrong * 20_000 < all, "{wrong} octets wrong of {all}");
    }

    /// Octets wrong at each ratio, for choosing what to ask of a line.
    #[test]
    #[ignore = "a measurement, not a test"]
    fn what_the_code_wants_of_a_line() {
        let mut noise = Noise(17);
        for frame_bits in [40u16, 72, 80, 84, 88] {
            let grid = Grid::new(frame_bits);
            let bits = f64::from(frame_bits) / 8.0;
            // Shannon: the ratio at which this many bits is all there is.
            let limit = 10.0 * (2f64.powf(bits) - 1.0).log10();
            let mut line = format!("{bits:5.2} bits a symbol, {} points, limit {limit:.1} dB:", grid.size());
            for over in [1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0] {
                let (mut wrong, mut all, mut blocks_bad) = (0, 0, 0);
                for _ in 0..1000 {
                    let (bad, octets, _) = through(&grid, 248, limit + over, &mut noise);
                    wrong += bad;
                    all += octets;
                    blocks_bad += usize::from(bad > 0);
                }
                line += &format!("  +{over}: {:.1e} ({blocks_bad})", wrong as f64 / all as f64);
            }
            println!("{line}");
        }
    }
}
