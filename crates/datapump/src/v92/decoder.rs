//! The digital modem's upstream decoder (6.4 read backwards): equalised levels
//! in, scrambled bits out, with the V.34 trellis the 4D symbols ride on and
//! the twelve-interval modulus decoder behind it.
//!
//! V92-20. What the digital modem has to undo, in the order the analogue modem
//! did it:
//!
//! ```text
//! levels -> cheapest point per residue class -> Viterbi over four-symbol frames
//!        -> etas -> Ki (6.4.2 backwards) -> modulus decoder (6.4.1) -> GPA
//! ```
//!
//! The trellis frame is four symbols (Figure 1) and the code is V.34's (6.4.4),
//! so this runs [`crate::v34::trellis::Code`] exactly as V.34 does and changes
//! only how often it is clocked.
//!
//! Two things make this decoder smaller than V.34's, and both are properties of
//! the upstream rather than shortcuts. There is no superframe and no modulo
//! encoding -- 6.4.4 cites 9.6.3.2 alone -- so the only structure over four
//! symbols is the code. And the constellation sets are the *chosen* ones,
//! uploaded in CPd, so a point's level is known exactly instead of being
//! derived from a shell mapping.
//!
//! The fourth index is not free: 6.4.2's class at k = 3 steps by 2 x Mi, and
//! [`Chain`](super::precoder::Chain) takes its parity bit from the three
//! indices before it and Y0. That is why the per-frame table below is built
//! over residue classes rather than over points.

use std::collections::VecDeque;

use crate::v32::Scrambler;
use crate::v34::trellis::{self, Code};

use super::modulus::{self, Moduli12};
use super::precoder::{Constellation, code_for, index_to_ki, label_of};
use super::{Parameters, TRELLIS_FRAME, UP_INTERVALS, UP_RATES, up_bits};

/// Trellis frames held before the oldest is decided on. Forty frames is a
/// hundred and sixty symbols, twenty milliseconds: long enough for every code
/// here to have settled, short enough that the tail a cleardown cuts off is
/// nothing.
pub const SURVIVOR_DEPTH: usize = 40;

/// Where to write one decided trellis frame's four indices, when
/// `V92_ETA_TRACE` names a file: a header line naming K and the twelve moduli,
/// then `stage eta0 eta1 eta2 eta3` per decided frame.
///
/// Everything the bit layer does -- the mixed radix, the differential sign
/// step and the scrambler -- happens downstream of these indices, so writing
/// them out once is what lets a suspected convention be settled by enumerating
/// it offline over the same capture instead of one bench run per guess. The
/// indices are what the equaliser and the Viterbi decided, and do not depend on
/// any of the conventions under test.
fn eta_trace_file() -> Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>> {
    use std::sync::{Arc, Mutex, OnceLock};
    static FILE: OnceLock<Option<Arc<Mutex<std::fs::File>>>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = std::path::PathBuf::from(std::env::var_os("V92_ETA_TRACE")?);
        let file = std::fs::File::create(&path).ok()?;
        Some(Arc::new(Mutex::new(file)))
    })
    .clone()
}

/// One decided frame's indices, to `V92_ETA_TRACE` if it is set.
fn trace_etas(decoder: &Decoder, stage: u64, etas: [i32; TRELLIS_FRAME]) {
    let Some(file) = eta_trace_file() else { return };
    let Ok(mut file) = file.lock() else { return };
    use std::io::Write;
    // The header is written by whoever gets the empty file first, so that the
    // reader does not have to be told K and the moduli separately.
    if file.metadata().map(|meta| meta.len()).unwrap_or(1) == 0 {
        let moduli: Vec<String> = decoder.moduli.values().iter().map(u8::to_string).collect();
        let _ = writeln!(file, "k {} moduli {}", decoder.bits_per_frame, moduli.join(" "));
    }
    let _ = writeln!(file, "{stage} {} {} {} {}", etas[0], etas[1], etas[2], etas[3]);
}

/// The cheapest index of one symbol in each residue class modulo 4, which is
/// all a 4D subset label depends on ([`label_of`] works on 2 x eta + 1).
#[derive(Debug, Clone, Copy)]
struct Bests {
    cost: [f64; 4],
    eta: [i32; 4],
}

impl Default for Bests {
    fn default() -> Self {
        Self { cost: [f64::MAX; 4], eta: [0; 4] }
    }
}

impl Bests {
    fn new(set: &Constellation, level: f64) -> Self {
        let mut bests = Self::default();
        for eta in set.lowest()..=set.highest() {
            let Some(point) = set.level(eta) else { continue };
            let residue = eta.rem_euclid(4) as usize;
            let cost = (level - point) * (level - point);
            if cost < bests.cost[residue] {
                bests.cost[residue] = cost;
                bests.eta[residue] = eta;
            }
        }
        bests
    }
}

/// One trellis frame in the Viterbi's window: its branch metrics and the four
/// indices each of them means.
#[derive(Debug, Clone, Copy)]
struct Stage {
    /// `table[out * 16 + y]`, the cost of the best four indices for that (Y0,
    /// y), or infinite when no indices make it.
    table: [f64; 32],
    /// Which residue class each of the four symbols was taken from, per
    /// (out, y).
    residues: [[(i32, i32, i32, i32); 16]; 2],
    /// The per-symbol tables it was built from, so that a decision can name
    /// the indices rather than the classes.
    bests: [Bests; TRELLIS_FRAME],
    /// Which trellis frame this is, counted from the first symbol of B1u.
    /// Three of them make a data frame (Figure 1), so only every third may
    /// start one -- a decision that begins part way through a data frame has
    /// nothing to give.
    index: u64,
}

/// The upstream decoder: one equalised level a symbol in, bits out every
/// twelve.
#[derive(Debug, Clone)]
pub struct Decoder {
    moduli: Moduli12,
    code: Code,
    params: Parameters,
    descrambler: Scrambler,
    modulus: modulus::Decoder,
    bits_per_frame: u32,
    /// The symbol count since the first symbol of B1u (8.7.1).
    n: u64,
    frame: [Bests; TRELLIS_FRAME],
    filled: usize,
    metrics: Vec<f64>,
    stages: VecDeque<Stage>,
    /// `back[i][state]` is where the state at frame i + 1 came from, and
    /// whether Y0 went out inverted to get there.
    back: VecDeque<Vec<(u8, bool)>>,
    /// Ki decided but not yet a whole data frame's worth.
    labels: Vec<u8>,
    bits: Vec<bool>,
    decided: u64,
}

impl Decoder {
    /// A decoder for a far end sending these parameters, starting at the first
    /// symbol of B1u: every memory zero and n = 0, exactly as the encoder
    /// starts (8.7.1).
    pub fn new(parameters: &Parameters) -> Self {
        let code = code_for(parameters.trellis);
        let mut metrics = vec![f64::MAX; code.states()];
        metrics[0] = 0.0;
        Self {
            moduli: Moduli12::new(parameters.moduli),
            code,
            params: parameters.clone(),
            descrambler: Scrambler::new(crate::v32::Mode::Answer),
            modulus: modulus::Decoder::new(),
            bits_per_frame: up_bits(parameters.drn),
            n: 0,
            frame: [Bests::default(); TRELLIS_FRAME],
            filled: 0,
            metrics,
            stages: VecDeque::new(),
            back: VecDeque::new(),
            labels: Vec::with_capacity(UP_INTERVALS),
            bits: Vec::new(),
            decided: 0,
        }
    }

    /// One equalised symbol, on the scale the parameters' levels are in.
    pub fn feed(&mut self, level: f64) {
        // The wire carries G x v (8.8.3), while the constellation sets are the
        // pre-gain magnitudes a(eta), so the equalised level is divided back by
        // G before it is sliced against them.
        let level = level / self.params.gain;
        let interval = super::interval(self.n);
        let set: &[u16] = self.params.set_for(interval).unwrap_or(&[]);
        self.frame[self.filled] = Bests::new(&Constellation::new(set), level);
        self.filled += 1;
        self.n += 1;
        if self.filled == TRELLIS_FRAME {
            self.filled = 0;
            self.fill();
        }
    }

    /// Bits decoded so far, taken. Empty until the window has filled.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// Symbols fed since the first symbol of B1u.
    pub fn symbols(&self) -> u64 {
        self.n
    }

    /// Trellis frames decided on so far.
    pub fn decided(&self) -> u64 {
        self.decided
    }

    /// The rate this decoder was built for, in bit/s.
    pub fn rate(&self) -> u32 {
        self.bits_per_frame * 8000 / UP_INTERVALS as u32
    }

    /// The rung of 6.1's ladder this decoder was built for.
    pub fn drn(&self) -> u8 {
        (1..=UP_RATES as u8).find(|&drn| up_bits(drn) == self.bits_per_frame).unwrap_or(0)
    }

    /// This frame's branch metrics, and one trellis step.
    fn fill(&mut self) {
        let mut stage = build(&self.frame, self.code);
        stage.index = self.n / TRELLIS_FRAME as u64 - 1;
        let states = self.code.states();
        let mut next = vec![f64::MAX; states];
        let mut from = vec![(0u8, false); states];
        for (state, &metric) in self.metrics.iter().enumerate() {
            if metric == f64::MAX {
                continue;
            }
            let out = usize::from(self.code.output(state as u8));
            for y in 0..16usize {
                if y & !self.code.inputs() as usize != 0 {
                    continue;
                }
                // Y0 is the state's own output and nothing else. V.34 lets a
                // half frame invert it; V.92's upstream has no superframe to be
                // half way through, so 6.4.3's parity is the only rule and a
                // flipped table entry is not a path this receiver may take.
                let cost = stage.table[out * 16 + y];
                if cost == f64::MAX {
                    continue;
                }
                let to = usize::from(self.code.next(state as u8, y as u8));
                if metric + cost < next[to] {
                    next[to] = metric + cost;
                    from[to] = (state as u8, false);
                }
            }
        }
        let floor = next.iter().copied().fold(f64::MAX, f64::min);
        if floor == f64::MAX {
            // Nothing in the constellation is anywhere near the line: hold
            // what is left of the window and start again rather than decide
            // on noise.
            self.stages.clear();
            self.back.clear();
            self.metrics.iter_mut().for_each(|metric| *metric = f64::MAX);
            self.metrics[0] = 0.0;
            return;
        }
        for metric in &mut next {
            if *metric != f64::MAX {
                *metric -= floor;
            }
        }
        self.metrics = next;
        self.stages.push_back(stage);
        self.back.push_back(from);
        while self.stages.len() > SURVIVOR_DEPTH {
            self.decide();
        }
    }

    /// Decide the oldest frame in the window, from the cheapest path through
    /// the whole window.
    fn decide(&mut self) {
        let Some(best) = self
            .metrics
            .iter()
            .enumerate()
            .filter(|(_, metric)| **metric != f64::MAX)
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(state, _)| state as u8)
        else {
            return;
        };
        // The state the oldest frame started in, by walking the window's own
        // back pointers.
        // `back[k][s]` is where the state after frame k came from, so the
        // oldest frame's decision needs the state it *started* in: every
        // pointer in the window, from the newest state back past the oldest.
        let mut state = best;
        for step in self.back.iter().rev() {
            state = step[usize::from(state)].0;
        }
        let stage = self.stages.pop_front().expect("a stage is held before one is decided");
        self.back.pop_front();
        let out = usize::from(self.code.output(state));
        let mut chosen = (f64::MAX, 0usize);
        for y in 0..16usize {
            if y & !self.code.inputs() as usize != 0 {
                continue;
            }
            let cost = stage.table[out * 16 + y];
            if cost < chosen.0 {
                chosen = (cost, y);
            }
        }
        let residues = stage.residues[out][chosen.1];
        let mut etas = [0i32; TRELLIS_FRAME];
        let classes = [residues.0, residues.1, residues.2, residues.3];
        for (k, &residue) in classes.iter().enumerate() {
            etas[k] = stage.bests[k].eta[residue.rem_euclid(4) as usize];
        }
        debug_assert_eq!(
            trellis::convert(label_of(etas[0], etas[1]), label_of(etas[2], etas[3])) & self.code.inputs(),
            chosen.1 as u8,
            "the indices decided on do not carry the inputs the path took"
        );
        self.decided += 1;
        trace_etas(self, stage.index, etas);
        // 8.7.1 and Figure 1: a data frame is three trellis frames, and the
        // first symbol of B1u begins one. A decision that lands in the middle
        // of one -- which is every decision until the window has passed -- has
        // no whole data frame's worth of indices and is dropped.
        if !self.labels.is_empty() || stage.index.is_multiple_of(3) {
            self.push_etas(etas);
        }
    }

    /// One frame's indices to Ki, and twelve symbols' worth to bits.
    fn push_etas(&mut self, etas: [i32; TRELLIS_FRAME]) {
        for (k, &eta) in etas.iter().enumerate() {
            let interval = self.labels.len();
            let modulus = self.moduli.modulus(interval);
            self.labels.push(index_to_ki(eta, modulus, k));
            if self.labels.len() == UP_INTERVALS {
                let labels: [u8; UP_INTERVALS] =
                    std::mem::take(&mut self.labels).try_into().expect("twelve intervals");
                if let Ok(frame) = self.modulus.decode(&labels, &self.moduli, self.bits_per_frame) {
                    for bit in frame.bits() {
                        let out = self.descrambler.descramble(bit);
                        self.bits.push(out);
                    }
                }
            }
        }
    }
}

/// The 32 costs of one frame: the best four indices for every (Y0, y).
///
/// The search is over residue classes -- four per symbol -- because a 4D subset
/// label depends only on 2 x eta mod 8, and because the fourth index's parity
/// is fixed by the first three and Y0 (6.4.2's class at k = 3).
fn build(bests: &[Bests; TRELLIS_FRAME], code: Code) -> Stage {
    let mut stage = Stage { table: [f64::MAX; 32], residues: [[(0, 0, 0, 0); 16]; 2], bests: *bests, index: 0 };
    for out in 0..2usize {
        for y in 0..16usize {
            if y & !code.inputs() as usize != 0 {
                continue;
            }
            let mut cost = f64::MAX;
            let mut chosen = (0i32, 0i32, 0i32, 0i32);
            for r0 in 0..4usize {
                if bests[0].cost[r0] == f64::MAX {
                    continue;
                }
                for r1 in 0..4usize {
                    if bests[1].cost[r1] == f64::MAX {
                        continue;
                    }
                    let first = label_of(r0 as i32, r1 as i32);
                    for r2 in 0..4usize {
                        if bests[2].cost[r2] == f64::MAX {
                            continue;
                        }
                        let parity = (r0 + r1 + r2 + out) % 2;
                        for r3 in (parity..4).step_by(2) {
                            if bests[3].cost[r3] == f64::MAX {
                                continue;
                            }
                            let second = label_of(r2 as i32, r3 as i32);
                            // The code reads only the bits it has: 16 states
                            // take Y1 and Y2, 32 add Y4 and 64 all four
                            // (9.6.3.2), so a frame whose converter spells a
                            // larger number is the same branch to a shorter
                            // code.
                            if trellis::convert(first, second) & code.inputs() != y as u8 {
                                continue;
                            }
                            let total =
                                bests[0].cost[r0] + bests[1].cost[r1] + bests[2].cost[r2] + bests[3].cost[r3];
                            if total < cost {
                                cost = total;
                                chosen = (r0 as i32, r1 as i32, r2 as i32, r3 as i32);
                            }
                        }
                    }
                }
            }
            stage.table[out * 16 + y] = cost;
            stage.residues[out][y] = chosen;
        }
    }
    stage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::CONSTELLATION_FRAME;
    use crate::v92::precoder::Chain;
    use crate::v92::upstream::UpEncoder;
    use crate::v92::{Filters, Trellis};

    /// Parameters with one constellation set of `levels` positive points
    /// spaced by two, every interval on the modulus that holds its share of
    /// the frame, and a prefilter that passes x straight through.
    pub(super) fn parameters(drn: u8, trellis: Trellis) -> Parameters {
        Parameters {
            drn,
            trellis,
            extend_e2u: false,
            gain: 1.0,
            moduli: [1u8 << (up_bits(drn) / UP_INTERVALS as u32); UP_INTERVALS],
            filters: Filters { z2: vec![1.0], ..Filters::default() },
            sets: vec![(0..64).map(|index| (2 * index + 1) as u16).collect()],
            indices: [0; CONSTELLATION_FRAME],
        }
    }

    /// A deterministic bit source.
    pub(super) struct Random(pub u64);

    impl Random {
        pub(super) fn new(seed: u64) -> Self {
            Self(seed | 1)
        }

        pub(super) fn next(&mut self) -> bool {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 63 == 0
        }
    }

    /// 6.4.1 to 6.4.4 over a channel with nothing in it: the encoder's levels
    /// straight into the decoder, and every bit back, at all three codes and
    /// at both ends of the ladder. This is the whole claim of the package.
    #[test]
    fn every_bit_comes_back_over_a_perfect_channel() {
        for trellis in [Trellis::Sixteen, Trellis::ThirtyTwo, Trellis::SixtyFour] {
            for drn in [1u8, 19] {
                let parameters = parameters(drn, trellis);
                let mut encoder = UpEncoder::new(&parameters).expect("feasible");
                let mut decoder = Decoder::new(&parameters);
                let mut random = Random::new(0x5eed);
                let mut sent = Vec::new();
                let mut received = Vec::new();
                for _ in 0..(SURVIVOR_DEPTH * 6 * UP_INTERVALS) {
                    let symbol = encoder.next_symbol(|| {
                        let bit = random.next();
                        sent.push(bit);
                        bit
                    });
                    // "Over a perfect channel": the level the chain emitted,
                    // with nothing between it and the receiver.
                    decoder.feed(symbol.u);
                    received.extend(decoder.take_bits());
                }
                assert_eq!(decoder.drn(), drn, "{trellis:?}");
                assert_eq!(decoder.rate(), crate::v92::up_rate(drn), "{trellis:?}");
                assert!(!received.is_empty(), "{trellis:?} drn {drn}: nothing decoded");
                // The decoder's output starts SURVIVOR_DEPTH frames late, so
                // the two streams are compared from where they line up.
                let (skipped, errors) = align(&sent, &received);
                assert!(skipped.is_some(), "{trellis:?} drn {drn}: the streams do not line up");
                assert_eq!(errors, 0, "{trellis:?} drn {drn}: {errors} bits wrong");
            }
        }
    }

    /// Where the decoded stream starts relative to the sent one, and how many
    /// bits differ from there to the end of the shorter.
    fn align(sent: &[bool], received: &[bool]) -> (Option<usize>, usize) {
        let shortest = sent.len().min(received.len());
        let at = (0..shortest.min(300)).find(|&offset| {
            let length = shortest - offset;
            length > 64 && sent[offset..offset + length].iter().zip(&received[..length]).all(|(a, b)| a == b)
        });
        if at.is_none() {
            let mut best = (usize::MAX, 0usize);
            for offset in 0..shortest.min(400) {
                let length = shortest - offset;
                let errors = sent[offset..offset + length].iter().zip(&received[..length]).filter(|(a, b)| a != b).count();
                if errors < best.0 { best = (errors, offset); }
            }
            eprintln!("align: shortest {shortest} best {best:?} (errors, offset)");
        }
        match at {
            Some(offset) => {
                let length = shortest - offset;
                let errors = sent[offset..].iter().zip(&received[..length]).filter(|(a, b)| a != b).count();
                (Some(offset), errors)
            }
            None => (None, sent.iter().zip(received).filter(|(a, b)| a != b).count()),
        }
    }

    /// 6.4.1 and Figure 1: twelve symbols are one data frame of K bits, so the
    /// decoder hands bits over in whole frames and only once the window is
    /// past.
    #[test]
    fn bits_come_out_a_whole_data_frame_at_a_time_and_late() {
        let parameters = parameters(13, Trellis::Sixteen);
        let mut encoder = UpEncoder::new(&parameters).expect("feasible");
        let mut decoder = Decoder::new(&parameters);
        let bits = parameters.bits() as usize;
        let mut random = Random::new(9);
        let mut decoded = 0usize;
        let mut early = 0usize;
        for symbol in 0..(SURVIVOR_DEPTH * UP_INTERVALS) {
            let level = encoder.next_symbol(|| random.next()).u;
            decoder.feed(level);
            let got = decoder.take_bits().len();
            if got > 0 && early == 0 {
                early = symbol;
            }
            decoded += got;
        }
        assert_eq!(decoded % bits, 0, "{decoded} bits are not whole data frames");
        assert!(early >= SURVIVOR_DEPTH * TRELLIS_FRAME, "bits came out before the window was past");
        // 480 symbols are 120 trellis frames, forty of which the window holds,
        // so thirty data frames at most and a few short of that in practice.
        assert!(decoded >= bits * 20, "{decoded} bits from {} symbols", decoder.symbols());
        let _: fn(&Parameters) -> Result<Chain, &'static str> = Chain::new;
    }

    /// A line that is not carrying the constellation is the *receiver's*
    /// business (V92-22, which has the presence and error-rate watches a
    /// receiver has), and not this decoder's: every level it is given has a
    /// nearest point in every class, so a decoder on its own always answers.
    /// What it must not do is run off the end of its window or lose its place.
    #[test]
    fn a_dead_line_still_leaves_the_window_whole() {
        let parameters = parameters(13, Trellis::Sixteen);
        let mut decoder = Decoder::new(&parameters);
        for _ in 0..(SURVIVOR_DEPTH * 4 * UP_INTERVALS) {
            decoder.feed(0.0);
        }
        assert_eq!(decoder.symbols(), (SURVIVOR_DEPTH * 4 * UP_INTERVALS) as u64);
        assert!(decoder.stages.len() <= SURVIVOR_DEPTH);
        assert!(decoder.back.len() <= SURVIVOR_DEPTH);
        let _ = decoder.take_bits();
    }
}

