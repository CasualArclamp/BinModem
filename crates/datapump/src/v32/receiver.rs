//! The V.32 and V.32bis receiver, on the shared QAM core.
//!
//! Everything that brings the far end's symbols back -- the fixed mixer, the
//! stored samples, the equaliser at two samples a symbol, the three loops and
//! their one gate, losing the signal and finding it again -- is `dsp::qam`,
//! copied from V.34's receiver, which is live-proven. What is left here is
//! what is V.32's own (core.md 7.7): which constellation a symbol is decided
//! against, what TRN is, when S is due, how bits come out of the points, and
//! what the rest of the modem is told.
//!
//! The receiver this replaced acquired everything by decisions. A carrier
//! loop driven by the nearest point cannot pull in more than ten or fifteen
//! degrees of a dense constellation, and a delay of N samples turns an
//! 1800 Hz carrier by 40.5 degrees times N; that was the arrival-phase ladder
//! the old sweeps measured (contract.md 0, 7 A and B). A sample dropped or
//! repeated in the middle of a call was the same jump, and nothing held,
//! rewound or read again, so it was a retrain and a lower rate.
//!
//! So acquisition happens where the Recommendation puts it (design.md 3).
//! S says how fast the carrier turns and how fast the far clock runs; the
//! change from S to S-bar says where TRN begins (5.2.2); and TRN, known
//! symbol for symbol before it arrives (5.2.3), is solved against outright by
//! least squares for the equaliser, the gain, the carrier's absolute phase
//! and its turn. The loops only ever track after that, and the dense
//! constellations are only ever tracked, never acquired.
//!
//! The start-up says when S is due, by cues at its state entries
//! ([`Receiver::idle`], [`Receiver::hunt`]), and the receiver does the rest on
//! its own: trains when S turns into S-bar, trains anyway where S ended if
//! S-bar never came, hunts again when a training fits nothing, and falls back
//! on the taps an earlier training left when a retrain's fits nothing.
//!
//! A bare receiver with no start-up in front of it -- the loopback tests, the
//! lock sweep -- has no S or TRN to go on, and starts blind instead
//! (design.md 3.8): the equaliser as the pulse's matched filter and the
//! resync search as the acquisition. No call uses it: the start-up's first cue
//! replaces it.

use std::f64::consts::TAU;
use std::sync::OnceLock;

use dsp::Complex;
use dsp::qam::{Band, Constellation, Core, Heard, Options, Point, Slicer, Stage, Training, Via, Window};

use super::trellis::{self, Coded, Decoder};
use super::{
    BAUD, CARRIER, CHANGE_TO_DIBIT, CONSTELLATION_RMS, Coding, Mode, ROLLOFF, STATES, Scrambler, TrnSequence,
    WITHIN_4800, bits_per_symbol, coding_for, point_spacing_at, signal_point,
};

/// Level at which the far carrier is declared present, and the lower level at
/// which it is declared gone: a 20 ms envelope of the in-band amplitude, five
/// decibels of hysteresis, as V.22bis 6.5.2 asks for.
///
/// The levels the receiver before this one used, in line terms: 1e-3 and
/// 5.62e-4 of amplitude after a mixer of unit gain. The core's mixer doubles
/// the signal, so that its output is the far end's baseband at its own size,
/// and the levels double with it. A hang-up drops the carrier about 100 ms
/// later at ordinary levels, which is when the modem crate reports NO CARRIER
/// (contract.md 1.1).
const CARRIER_ON: f64 = 2.0e-3;
const CARRIER_OFF: f64 = 1.124e-3;

/// Symbols decided as the nearest point, whatever the coding, after anything
/// that leaves the trellis decoder's paths meaningless for a while: a change
/// of rate or coding, a training, a blind start, a slip found again.
///
/// The best path's guess is only as good as the paths are, and paths made
/// from symbols before a jump, or from none at all, are no guide to the
/// symbol after it. Sixteen symbols is several times what an eight-state
/// code needs to sort its paths out, and short against the carrier loop's
/// fifty (design.md 2.6).
const FRESH: u32 = 16;

/// Symbols of TRN, counted from symbol 0, that the equaliser is solved over:
/// V.34's first try, and its retry further in and searched wide for a slip
/// (design.md 3.5). The retry is done by symbol 1170, inside the shortest TRN
/// 5.2.3 allows.
const FIRST: Window = Window { align: (16, 256), solve: (16, 512), search: 8 };
const RETRY: Window = Window { align: (640, 1152), solve: (640, 1152), search: 200 };

/// The one try made when S ends without S-bar (design.md 3.2).
///
/// Something between the two hid the reversal, and it can be most things: a
/// jitter buffer dropping a packet across the join takes S-bar and the first
/// 32 symbols of TRN with it, and one that fills a packet with comfort noise
/// there leaves S-bar where the hunt, starting over, cannot see it. So which
/// symbol of TRN arrived as S ended is anything from 67 symbols before it, a
/// packet of noise and S-bar still to come, to 263 after it -- which also
/// covers the one real modem's TRN in the tree, whose recording runs S
/// straight into TRN symbol 193 (`tests/v32_trn.rs`). The window is past all
/// of them, symbols 272 on, so that whatever did arrive, only TRN is in it.
///
/// And short. What S said of the far clock is what the rows are read by, and
/// where S and TRN were not one stretch of signal it may not hold: in that
/// recording TRN runs 104 ppm faster than the S before it, which solved over
/// 768 symbols fitted 22.3 dB and over 384 fits 26.3. A mistaken clock costs
/// the square of the window's length; the rows' own noise, a few tenths of a
/// decibel for halving it.
const LAPSED: Window = Window { align: (272, 528), solve: (272, 656), search: 330 };

/// Half symbols from where S ended to TRN symbol 0, at the middle of the
/// search [`LAPSED`] makes: TRN symbol 98 arriving as S ends.
const LAPSED_OFFSET: i64 = 32 - 2 * 98;

/// Signal to noise, in decibels, below which a training's fit is taken to be
/// the wrong alignment and the next try is made (`v34/receiver.rs:90`).
const ACCEPT_DB: f64 = 12.0;

/// Symbols of the far end's TRN known ahead: the shortest TRN there is, which
/// every window above falls inside.
const TRN_KNOWN: usize = 1280;

/// V.32's constellations at unit power, each a table for the core to decide
/// against, labelled so that what is decided can be read back as bits.
#[derive(Debug)]
struct Tables {
    /// A, B, C and D, labelled by the state's index (Figure 1): the start-up
    /// and 4800.
    four: Slicer,
    /// 9600 without the trellis code (2.4.1.1), labelled four to a quadrant:
    /// the quadrant's state times four and the point within it.
    sixteen: Slicer,
    /// The trellis codings, labelled by their code (Figures 2-1 to 2-4/V.32bis),
    /// in order of the bits a symbol carries: 7200, 9600, 12 000, 14 400.
    coded: [Slicer; 4],
}

/// The tables, made once: a constellation works out its lattice and what
/// garbage reads against it when it is made, and every receiver shares them.
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let unit = |(x, y): (f64, f64)| Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS);
        let four = Constellation::new(STATES.iter().copied().map(unit).collect());
        let sixteen = Constellation::new(
            (0..16).map(|label| unit(signal_point(label / 4, label % 4))).collect(),
        );
        let coded = [trellis::AT_7200, trellis::AT_9600, trellis::AT_12000, trellis::AT_14400]
            .map(|coded| Slicer::table(Constellation::new((0..coded.size()).map(|code| unit(coded.point(code))).collect())));
        Tables { four: Slicer::table(four), sixteen: Slicer::table(sixteen), coded }
    })
}

/// The table for a rate and coding.
fn slicer_for(rate: u32, coding: Coding) -> Slicer {
    let tables = tables();
    match coding_for(rate, coding) {
        Some(coded) => tables.coded[coded.bits - 3].clone(),
        None if bits_per_symbol(rate) == 4 => tables.sixteen.clone(),
        None => tables.four.clone(),
    }
}

/// V.32 and V.32bis receiver, at every rate.
#[derive(Debug)]
pub struct Receiver {
    core: Core,
    /// The far end's TRN at unit power: the far end's own polynomial, started
    /// from zero, ones in (5.2.3).
    trn: Vec<Complex>,
    descrambler: Scrambler,
    bits: Vec<bool>,
    /// The state of the last symbol decided, which Table 1 is undone against.
    quadrant: Option<u8>,
    /// Bits each arriving symbol carries: two at 4800 and up to six at
    /// 14 400.
    carried: u32,
    /// The rate the arriving data is coded at.
    rate: u32,
    /// Which of the two 9600 modulations is in use.
    coding: Coding,
    /// The trellis coding the rate and the choice come to, when they come to
    /// one at all.
    coded: Option<Coded>,
    /// The Viterbi decoder, used only when `coded` is set.
    trellis: Decoder,
    /// Symbols still to be decided as the nearest point: see [`FRESH`].
    fresh: u32,
    /// Slips the core had found at the last symbol.
    slips: u32,
    /// The last symbol made, at unit power.
    last: Complex,
    carrier: bool,
    /// Held from outside: symbols made, nothing learned.
    held: bool,
    /// The far carrier's offset as the last S had it, and how this receiver
    /// last came to be tracking.
    s_offset_hz: Option<f64>,
    via: Option<Via>,
    /// The training S-bar or a lapse of S calls for, held back while the hunt
    /// goes on listening: what to train on, and the half-symbol sample by
    /// which its first window is all in.
    pending: Option<(Training, u64)>,
}

impl Receiver {
    /// `mode` is this modem's own end; the descrambler and TRN are the far
    /// end's polynomial, since that is what will arrive.
    ///
    /// Starts blind, at whatever rate and coding are set before the signal
    /// arrives: a start-up's first cue puts that aside.
    pub fn new(mode: Mode, fs: f64) -> Self {
        let four = tables().four.clone();
        let mut core = Core::new(Band::new(fs, BAUD, CARRIER), Options::fixed(), four.clone());
        core.acquire_blind(four, ROLLOFF);
        let mut sequence = TrnSequence::new(mode.peer());
        let trn = (0..TRN_KNOWN)
            .map(|_| {
                let (x, y) = STATES[sequence.next()];
                Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS)
            })
            .collect();
        Self {
            core,
            trn,
            descrambler: Scrambler::new(mode.peer()),
            bits: Vec::new(),
            quadrant: None,
            carried: 2,
            rate: 4800,
            coding: Coding::Uncoded,
            coded: None,
            trellis: Decoder::new(trellis::AT_9600),
            fresh: FRESH,
            slips: 0,
            last: Complex::ZERO,
            carrier: false,
            held: false,
            s_offset_hz: None,
            via: None,
            pending: None,
        }
    }

    /// Stop listening for the far end, but go on keeping what arrives.
    ///
    /// For the stretches of the start-up where there is nothing of the far
    /// end's to learn from: its tones, the silences, and this end's own
    /// conditioning signal, which is all that is on the line then.
    pub fn idle(&mut self) {
        self.core.idle();
        self.pending = None;
    }

    /// Listen for the far end's S and the change to S-bar, and train on the
    /// TRN that follows.
    ///
    /// Only when S is due: V.34's hunt, which this is, took AA turning into CC
    /// and AC into CA for S turning into S-bar (core.md E6), and although the
    /// core now tells those apart by where their power is, the start-up knows
    /// better still when S can arrive.
    pub fn hunt(&mut self) {
        self.core.hunt();
        self.pending = None;
    }

    /// Change the rate the arriving data is coded at.
    ///
    /// A different moment from the transmitter's, and 5.4 is explicit about
    /// which: "When the modem detects an incoming 16-bit E sequence ... it
    /// shall condition itself to receive data at the rate and with the coding
    /// indicated by the E sequence." The far end changes as it finishes
    /// sending that E, so the two land on the same place in the stream.
    ///
    /// Everything the receiver has learned carries across: the taps, the
    /// carrier, the timing, the gain. Only what the symbols are decided
    /// against changes (design.md 3.6).
    pub fn set_data_rate(&mut self, bits_per_second: u32) {
        self.rate = bits_per_second;
        self.carried = bits_per_symbol(bits_per_second);
        self.follow();
    }

    /// Choose between the two modulations 9600 bit/s has (2.4.1).
    pub fn set_coding(&mut self, coding: Coding) {
        self.coding = coding;
        self.follow();
    }

    /// Work out the coding from the rate and the choice, whichever was set
    /// last, and everything that depends on it.
    fn follow(&mut self) {
        let coded = coding_for(self.rate, self.coding);
        if coded.map(|c| c.bits) != self.coded.map(|c| c.bits) {
            match coded {
                Some(coded) => self.trellis.set_coding(coded),
                None => self.trellis.reset(),
            }
            self.fresh = FRESH;
        }
        self.coded = coded;
        let slicer = slicer_for(self.rate, self.coding);
        let same = match (&slicer, self.core.slicer()) {
            (Slicer::Table(a), Slicer::Table(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        };
        if same {
            return;
        }
        if self.core.stage() == Stage::Blind {
            // Nothing found yet, so there is nothing to keep: look for the
            // constellation the data will be on.
            self.core.acquire_blind(slicer, ROLLOFF);
        } else {
            // A late switch costs nothing. The start-up switches at its next
            // symbol after reading E, so a B1 symbol or so may be decided on
            // the old constellation; its large error is refused by the gate,
            // and B1 carries no data (design.md 3.6).
            self.core.set_slicer(slicer);
        }
        self.fresh = FRESH;
    }

    pub fn feed(&mut self, sample: f64) {
        self.core.feed(sample);
        let envelope = self.core.envelope();
        self.carrier = if self.carrier { envelope > CARRIER_OFF } else { envelope > CARRIER_ON };
        self.listen();
        // A training nothing has overtaken: the window it would have waited
        // for is in, so train on it now, from the samples the core kept.
        if self.pending.as_ref().is_some_and(|(_, due)| self.core.halves() >= *due)
            && let Some((training, _)) = self.pending.take()
        {
            self.core.train(training);
        }
        // Every symbol whose samples are in, now. After a training that is a
        // few hundred at once, from the end of the window it solved over.
        while let Some(point) = self.core.next() {
            let target = self.decide(&point);
            let Some(symbol) = self.core.settle(target) else { break };
            self.last = symbol.point;
            if self.core.slips() != self.slips {
                // Found again after a slip: the paths the trellis decoder
                // holds straddle the jump.
                self.slips = self.core.slips();
                self.fresh = FRESH;
            }
            self.fresh = self.fresh.saturating_sub(1);
        }
    }

    /// Act on what the core has heard.
    fn listen(&mut self) {
        while let Some(heard) = self.core.heard() {
            match heard {
                // S again: what stopped it, or turned it, was a hole in S and
                // not its end, and S-bar is still to come.
                Heard::S => self.pending = None,
                Heard::Reversal { at, turn, drift } => {
                    // 5.2.2: "The transition from segment 1 to segment 2
                    // provides a well-defined event in the signal that may be
                    // used for generating a time reference in the receiver."
                    // TRN symbol 0 is the seventeenth symbol after it, whose
                    // centre is 32 half symbols on from where S-bar began.
                    //
                    // Held back like a lapse, below, and for the same reason:
                    // a concealment that repeats a fragment of S has phase
                    // jumps in it that read as S turning into S-bar. If S goes
                    // on afterwards, the hunt hears it and this is dropped.
                    self.s_offset_hz = Some(turn * BAUD / TAU);
                    let training = self.training(at + 32, FIRST, Some(RETRY), Some((turn, drift)));
                    self.pending = Some((training, due(at + 32, FIRST)));
                    self.core.hunt();
                }
                // Not trained on straight away. A jitter buffer's 20 ms of
                // silence, comfort noise or a repeated packet in the middle
                // of S ends it here just as S-bar would, and a training begun
                // on that spends the next third of a second deaf while the
                // real S-bar and TRN go by -- the call then waits on a rate
                // signal it can never read. So the hunt goes on, for exactly
                // as long as this training would have been collecting its
                // window anyway: S heard again, or S-bar, overtakes it, and
                // if neither comes it runs from the kept samples, late by
                // nothing. The window's far edge is 1990 halves back at most,
                // well inside the 4096 the core keeps.
                Heard::Lapsed { at } => {
                    let measured = self.core.s_measured();
                    self.s_offset_hz = measured.map(|(turn, _)| turn * BAUD / TAU);
                    let start = at.saturating_add_signed(LAPSED_OFFSET);
                    self.pending = Some((self.training(start, LAPSED, None, measured), due(start, LAPSED)));
                    self.core.hunt();
                }
                Heard::Trained { via, .. } => {
                    self.via = Some(via);
                    // TRN is not differentially coded, so its last symbol is
                    // just a state; the rate signal's first is read against
                    // it, as 5.3 has the far end encode it.
                    self.quadrant = None;
                    self.fresh = FRESH;
                    self.slips = self.core.slips();
                }
                // Nothing fitted where the sequence was said to be. The far
                // end may be starting again; listen for the next S.
                Heard::Untrained => self.core.hunt(),
            }
        }
    }

    /// Training on the far end's TRN, symbol 0 centred on half `start`.
    fn training(&self, start: u64, first: Window, retry: Option<Window>, measured: Option<(f64, f64)>) -> Training {
        Training {
            targets: self.trn.clone(),
            start,
            first,
            retry,
            turn: measured.map(|(turn, _)| turn),
            drift: measured.map(|(_, drift)| drift),
            accept_db: ACCEPT_DB,
            slicer: tables().four.clone(),
            fallback: true,
        }
    }

    /// Read one symbol's bits, and say what the loops should take it to be.
    fn decide(&mut self, point: &Point) -> Complex {
        let Some(coded) = self.coded else {
            // The uncoded constellations' nearest point is the decision
            // itself, and has no delay.
            self.uncoded_bits(point.label.unwrap_or(0));
            return point.nearest;
        };
        // 2.4.1.2: the decision is a whole sequence rather than a point, so
        // the bits come from the decoder, some symbols later.
        let at = point.z.scale(CONSTELLATION_RMS);
        if let Some(group) = self.trellis.decode((at.re, at.im)) {
            for &bit in &group[..coded.bits] {
                let out = self.descrambler.descramble(bit);
                self.bits.push(out);
            }
        }
        if self.fresh > 0 {
            return point.nearest;
        }
        match self.trellis.tentative() {
            Some(code) => {
                let (x, y) = coded.point(code);
                Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS)
            }
            None => point.nearest,
        }
    }

    /// Table 1 undone, and Table 3 at 9600: the quadrant carries the first
    /// two bits of each group, by how far it turned since the symbol before,
    /// and the point within it the other two.
    fn uncoded_bits(&mut self, label: usize) {
        let (state, within) = if self.carried == 4 { (label / 4, label % 4) } else { (label, WITHIN_4800) };
        let quadrant = state as u8;
        let Some(previous) = self.quadrant.replace(quadrant) else {
            return;
        };
        let change = (quadrant + 4 - previous) & 3;
        let dibit = CHANGE_TO_DIBIT[change as usize];
        let group = [dibit & 0b10 != 0, dibit & 0b01 != 0, within & 0b10 != 0, within & 0b01 != 0];
        for &bit in group.iter().take(self.carried as usize) {
            let out = self.descrambler.descramble(bit);
            self.bits.push(out);
        }
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// Take whole octets, most significant bit first, leaving any remainder.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        let whole = self.bits.len() / 8;
        let bits: Vec<bool> = self.bits.drain(..whole * 8).collect();
        bits.as_chunks::<8>()
            .0
            .iter()
            .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)))
            .collect()
    }

    /// The last symbol made, equalised, at the constellation's own unit power.
    ///
    /// It changes once for every symbol made, and not while none are, which is
    /// how a scope that reads it every sample knows a new one has come.
    pub fn constellation_point(&self) -> (f64, f64) {
        (self.last.re, self.last.im)
    }

    /// Mean distance of the symbols from the nearest point, in the units
    /// [`point_spacing`](Self::point_spacing) is measured in, over about a
    /// hundred symbols.
    ///
    /// Every symbol made counts, whether the gate let it teach the loops or
    /// not, and it goes on counting while the signal is lost. So a signal
    /// lost for a second reads as unsatisfactory reception, and 7's retrain
    /// follows without any other watchdog (design.md 4.6).
    pub fn residual_error(&self) -> f64 {
        self.core.residual_error()
    }

    /// How far apart the closest two points of the constellation in use are,
    /// in the units [`residual_error`](Self::residual_error) is measured in.
    ///
    /// The error on its own says nothing. Half this distance is the decision
    /// boundary, so the same error is a comfortably locked receiver at 4800
    /// and a receiver reading noise at 14 400, where the points are a sixth as
    /// far apart. Anything that wants to judge reception has to divide by this
    /// first.
    pub fn point_spacing(&self) -> f64 {
        point_spacing_at(self.rate, self.coding)
    }

    /// Whether the far end's carrier is there: the in-band amplitude of what
    /// arrives, over 20 ms, with five decibels of hysteresis.
    pub fn carrier(&self) -> bool {
        self.carrier
    }

    /// Whether the loops may learn from what is arriving.
    ///
    /// Held, the receiver goes on making symbols and handing up bits, the
    /// carrier's phase goes on turning, and nothing learns or is judged lost.
    /// The start-up no longer needs this: it says when the far end is worth
    /// listening to with [`idle`](Self::idle) and [`hunt`](Self::hunt).
    pub fn set_adapting(&mut self, adapting: bool) {
        self.held = !adapting;
        self.core.hold(self.held);
    }

    /// What the receiver is doing: idle, hunting for S, training, tracking,
    /// or looking blind.
    pub fn stage(&self) -> Stage {
        self.core.stage()
    }

    /// Whether symbols are being made and followed.
    pub fn is_tracking(&self) -> bool {
        self.core.is_tracking()
    }

    /// Signal to noise of every symbol against its nearest point, in
    /// decibels.
    pub fn snr_db(&self) -> f64 {
        self.core.snr_db()
    }

    /// What the last training, fallback or blind start came to, in decibels.
    pub fn trained_snr_db(&self) -> f64 {
        self.core.trained_snr_db()
    }

    /// How this receiver last came to be tracking.
    pub fn trained_via(&self) -> Option<Via> {
        self.via
    }

    /// The far carrier's offset from 1800 Hz as the last S heard said it.
    pub fn s_offset_hz(&self) -> Option<f64> {
        self.s_offset_hz
    }

    /// Slips found and followed: a sample dropped or repeated, a hole, a jump.
    pub fn slips(&self) -> u32 {
        self.core.slips()
    }

    /// Whether the signal has jumped and not been found again yet.
    pub fn is_lost(&self) -> bool {
        self.core.is_lost()
    }

    /// Symbols since the signal was lost; nought while it is not.
    pub fn lost_for(&self) -> usize {
        self.core.lost_for()
    }

    /// The far clock's rate against this end's, as the timing loop has it, in
    /// parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.core.drift_ppm()
    }

    /// The far carrier's offset as the carrier loop has it, in hertz.
    pub fn offset_hz(&self) -> f64 {
        self.core.offset_hz()
    }

    /// The carrier phase being taken out, in degrees.
    pub fn rotation_degrees(&self) -> f64 {
        self.core.rotation().to_degrees()
    }

    /// The gain control's gain, in decibels.
    pub fn gain_db(&self) -> f64 {
        self.core.gain_db()
    }
}

/// The half-symbol sample by which a training from `start` has all of its
/// first window in: the core's own reckoning, with a little over its
/// equaliser's reach to spare.
fn due(start: u64, window: Window) -> u64 {
    let end = window.solve.1.max(window.align.1) as u64;
    start + window.search.max(0) as u64 + 2 * end + 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_is_at_unit_power_and_sliced_as_by_search() {
        // The core finds the nearest point by lattice rounding, which is only
        // worth having if it is the nearest point: V.32's four are turned
        // 26.57 degrees from the axes and its crosses sit on a lattice turned
        // 45, and a fast search that got either wrong would decide some
        // points wrong on a perfect line.
        let tables = tables();
        let every = [&tables.four, &tables.sixteen, &tables.coded[0], &tables.coded[1], &tables.coded[2], &tables.coded[3]];
        let mut seed = 0x2545_f491u32;
        let mut uniform = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f64::from(seed) / f64::from(u32::MAX) - 0.5
        };
        for slicer in every {
            let Slicer::Table(table) = slicer else { panic!("V.32's constellations are tables") };
            assert!((table.power() - 1.0).abs() < 1e-12, "{} points at power {}", table.len(), table.power());
            assert!(table.on_lattice(), "{} points found no lattice", table.len());
            for _ in 0..50_000 {
                let z = Complex::new(uniform(), uniform()).scale(4.0);
                assert_eq!(table.nearest(z), table.nearest_exhaustive(z), "{} points at {z:?}", table.len());
            }
        }
    }

    #[test]
    fn each_label_reads_back_as_the_point_it_names() {
        // The four are labelled by state, the sixteen by quadrant and place in
        // it, the coded by code: what the bits are read from.
        let tables = tables();
        let Slicer::Table(four) = &tables.four else { unreachable!() };
        for (state, &(x, y)) in STATES.iter().enumerate() {
            assert_eq!(four.nearest(Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS)), state);
        }
        let Slicer::Table(sixteen) = &tables.sixteen else { unreachable!() };
        for state in 0..4 {
            for within in 0..4 {
                let (x, y) = signal_point(state, within);
                assert_eq!(sixteen.nearest(Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS)), 4 * state + within);
            }
        }
        for (k, coded) in [trellis::AT_7200, trellis::AT_9600, trellis::AT_12000, trellis::AT_14400].into_iter().enumerate() {
            let Slicer::Table(table) = &tables.coded[k] else { unreachable!() };
            for code in 0..coded.size() {
                let (x, y) = coded.point(code);
                assert_eq!(table.nearest(Complex::new(x, y).scale(1.0 / CONSTELLATION_RMS)), code);
            }
        }
    }
}
