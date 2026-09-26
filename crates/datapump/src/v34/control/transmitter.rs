//! The control channel's transmitter: a queue of signals, as line samples
//! (10.2.4).
//!
//! A procedure says what to send, in order -- "silence 70 ms, PPh, then ALT
//! until told otherwise, then these MPh bits, then E, then data" -- and this
//! sends it at 600 baud. Some signals have a length of their own: silence,
//! PPh, Sh and S-bar-h, E, the 4T of ones, bits sent once. The rest go on
//! until something is queued behind them: ALT once it has had its least, AC,
//! an MPh repeated whole, and data. So a procedure queues what comes next at
//! the moment it knows what that is, and the signal before it gives way at
//! the next symbol it may: "finish the current MPh, send one E" (12.4.1.3)
//! is queueing E behind a repeated MPh.
//!
//! The modulator is `dpsk.rs`'s made complex: the same carriers, levels,
//! guard tone and Figure 13 pulse, with a history of complex points in place
//! of a real one. Its clock is `qam.rs`'s, exact at any whole-numbered sample
//! rate: 80/3 samples a symbol at 16 kHz.

use std::collections::VecDeque;
use std::f64::consts::TAU;

use dsp::{Complex, rrc_at};

use super::{E_SYMBOLS, PPH_SYMBOLS, Rate, Reading, ac, samples_per_symbol, scrambler_of, sh, sh_bar, unit_point};
use crate::v34::dpsk::{GUARD_TONE, ROLLOFF, Side};
use crate::v34::signals::{Sender, Size};

/// Symbols either side of the centre that the pulse is carried for.
///
/// Two more than `dpsk.rs`'s six, tapered to nothing past the last, so that
/// what this end puts into the far end's band -- 675 Hz from its own carrier
/// and more, where its own receiver listens on a two-wire line -- is as
/// little as the pulse allows (see the test that measures it).
const SPAN: usize = 8;

/// How long the guard tone takes to come up and to die away, as a raised
/// cosine.
///
/// 10.2.4 says nothing of how the guard tone starts, and `dpsk.rs` switches
/// it. Switched, its first cycle spreads across the whole of the call
/// modem's band 600 Hz below it -- where the answer modem's own receiver
/// listens, with this end's signal coming back into it on a two-wire line.
/// The call modem's PPh arrives while the answer modem is still silent, and
/// the answer modem starts its own PPh in the middle of the window its
/// receiver trains on: switched, the guard tone cost that training 14 dB.
const GUARD_RAMP_SECONDS: f64 = 0.008;

/// The symbols about the centre whose sounding holds the guard tone up: from
/// three still to come, so that its ramp is nearly done as the first symbol
/// peaks, to one gone, so that it dies away with the last. Not the pulse's
/// whole reach, which would carry the tone 13 ms into clause 12's silences
/// at each end.
const GUARD_AHEAD: usize = 3;
const GUARD_BEHIND: usize = 1;

/// One thing to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Nothing, for this many symbols. The guard tone stops too: clause 12's
    /// silences are silence.
    Silence(usize),
    /// PPh, once: 32 symbols (10.2.4.5).
    Pph(Reading),
    /// ALT (10.2.4.2): scrambled alternations of 0 and 1, from a scrambler
    /// and a differential encoder started at zero, beginning with 0. At least
    /// this many symbols, and then on until something is queued behind it.
    Alt { at_least: usize },
    /// AC (10.2.4.1), until something is queued behind it.
    Ac,
    /// Sh for this many symbols (10.2.3.3): 24 in clause 12.
    Sh(usize),
    /// S-bar-h for this many symbols: 8 in clause 12.
    ShBar(usize),
    /// These bits once, at 1200 bit/s, carrying on the scrambler and the
    /// differential encoder from whatever went before; padded with ones to a
    /// whole symbol.
    Bits(Vec<bool>),
    /// These bits over and over at 1200 bit/s, a whole repetition at a time,
    /// until something is queued behind them: MPh (12.4.1.2 to 12.4.1.3).
    /// Clause 12's MPh are 88 or 188 bits, whole symbols both.
    Repeat(Vec<bool>),
    /// E: twenty scrambled ones at 1200 bit/s (10.2.4.3).
    E,
    /// Scrambled ones at the data rate for this many symbols: the 4T a
    /// control channel turns off with (12.6.3).
    Ones(usize),
    /// The user's data at the data rate, from [`Transmitter::send_bits`],
    /// until something is queued behind it and every bit queued before that
    /// has gone. Ones when there are no bits: an idle line is marking.
    Data,
}

/// What a segment is, without what it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Silence,
    Pph,
    Alt,
    Ac,
    Sh,
    ShBar,
    Bits,
    Repeat,
    E,
    Ones,
    Data,
}

impl Segment {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Silence(_) => Kind::Silence,
            Self::Pph(_) => Kind::Pph,
            Self::Alt { .. } => Kind::Alt,
            Self::Ac => Kind::Ac,
            Self::Sh(_) => Kind::Sh,
            Self::ShBar(_) => Kind::ShBar,
            Self::Bits(_) => Kind::Bits,
            Self::Repeat(_) => Kind::Repeat,
            Self::E => Kind::E,
            Self::Ones(_) => Kind::Ones,
            Self::Data => Kind::Data,
        }
    }
}

/// A segment reaching the line, or leaving it: `at` is the sample, counted
/// as [`Transmitter::now`] counts them, at which its first symbol begins or
/// its last one ends -- half a symbol either side of the symbol's centre.
///
/// A segment that came to no symbols at all -- ALT with nothing asked of it
/// and something queued already -- says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Began { kind: Kind, at: u64 },
    Ended { kind: Kind, at: u64 },
}

/// One symbol in the pulse's reach, and what it opens or closes.
#[derive(Debug, Clone, Copy)]
struct Slot {
    symbol: Complex,
    kind: Option<Kind>,
    began: bool,
    ended: bool,
}

impl Slot {
    const SILENT: Self = Self { symbol: Complex::ZERO, kind: None, began: false, ended: false };
}

/// The segment being sent.
#[derive(Debug, Clone)]
struct Current {
    segment: Segment,
    /// Symbols of it sent so far.
    done: usize,
    /// Bits of it sent so far, for the segments that are given bits.
    position: usize,
}

/// Control-channel signals, as line samples.
#[derive(Debug, Clone)]
pub struct Transmitter {
    side: Side,
    /// Samples a symbol, as p/q, and where the next sample falls past the
    /// centre symbol, in 1/p of a symbol.
    p: u64,
    q: u64,
    frac: u64,
    /// The pulse at each of the p places a sample can fall, 2 SPAN + 1 taps
    /// each.
    table: Vec<f64>,
    /// The symbols the pulse reaches, oldest first, the centre one in the
    /// middle.
    history: VecDeque<Slot>,
    /// Symbols in the history that are not silence.
    sounding: usize,
    /// Carrier and guard tone phases, as fractions of a turn, and their steps
    /// a sample.
    carrier: f64,
    carrier_step: f64,
    guard: f64,
    guard_step: f64,
    carrier_amplitude: f64,
    guard_amplitude: f64,
    /// How far up its ramp the guard tone is, in samples, and the ramp's
    /// length.
    guard_up: usize,
    guard_ramp: usize,
    queue: VecDeque<Segment>,
    current: Option<Current>,
    /// The scrambler and the differential encoder, carried from ALT through
    /// MPh and E into data.
    sender: Sender,
    rate: Rate,
    bits: VecDeque<bool>,
    sent: VecDeque<Sent>,
    now: u64,
    symbols: u64,
}

impl Transmitter {
    /// The transmitter of the call or the answer modem, at `fs` samples a
    /// second.
    pub fn new(side: Side, fs: f64) -> Self {
        let (p, q) = samples_per_symbol(fs);
        let width = 2 * SPAN + 1;
        let mut table = vec![0.0; p as usize * width];
        let edge = SPAN as f64 + 1.0;
        for frac in 0..p as usize {
            for i in 0..width {
                let t = frac as f64 / p as f64 + SPAN as f64 - i as f64;
                // A Hann taper to nothing past the last symbol carried, as
                // `qam.rs` has it, so that the cut does not ring into the far
                // end's band.
                let taper = 0.5 + 0.5 * (std::f64::consts::PI * t / edge).cos();
                table[frac * width + i] = rrc_at(t, ROLLOFF) * taper;
            }
        }
        // Unit energy: averaged over where a sample can fall, the squares of
        // the taps it uses come to one symbol's worth, so a unit-power symbol
        // leaves at a root-mean-square of 0.707, the nominal level.
        let energy = table.iter().map(|t| t * t).sum::<f64>() / p as f64;
        for tap in &mut table {
            *tap /= energy.sqrt();
        }
        // "The answer modem shall transmit ... at 1 dB below the nominal
        // transmit power level, plus a 1800 ± 0.01% Hz guard tone at a level
        // 7 dB below the nominal transmit power level. The call modem shall
        // transmit with a 1200 ± 0.01% Hz carrier at the nominal transmit
        // power level" (10.2.4), as INFO does.
        let (carrier_amplitude, guard_amplitude) = match side {
            Side::Call => (1.0, 0.0),
            Side::Answer => (10f64.powf(-1.0 / 20.0), 10f64.powf(-7.0 / 20.0)),
        };
        Self {
            side,
            p,
            q,
            frac: 0,
            table,
            history: std::iter::repeat_n(Slot::SILENT, width).collect(),
            sounding: 0,
            carrier: 0.0,
            carrier_step: side.carrier() / fs,
            guard: 0.0,
            guard_step: GUARD_TONE / fs,
            carrier_amplitude,
            guard_amplitude,
            guard_up: 0,
            guard_ramp: ((GUARD_RAMP_SECONDS * fs) as usize).max(1),
            queue: VecDeque::new(),
            current: None,
            sender: Sender::new(scrambler_of(side)),
            rate: Rate::R1200,
            bits: VecDeque::new(),
            sent: VecDeque::new(),
            now: 0,
            symbols: 0,
        }
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// Send `segment` after everything queued so far.
    pub fn queue(&mut self, segment: Segment) {
        self.queue.push_back(segment);
    }

    /// Drop everything queued behind the segment being sent. It carries on
    /// as it would have with nothing behind it.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Stop dead: nothing queued, nothing being sent, nothing still dying
    /// away. The user's bits stay queued for the next [`Segment::Data`].
    pub fn stop(&mut self) {
        self.queue.clear();
        self.current = None;
        self.history.iter_mut().for_each(|slot| *slot = Slot::SILENT);
        self.sounding = 0;
        self.guard_up = 0;
    }

    /// Queue the user's bits, first in time first, for [`Segment::Data`].
    ///
    /// Keep ahead of the line: a symbol that finds fewer bits than it carries
    /// sends what there is padded with ones, so bits given in dribs within a
    /// frame would have ones put between them.
    pub fn send_bits(&mut self, bits: &[bool]) {
        self.bits.extend(bits.iter().copied());
    }

    /// The user's bits not sent yet.
    pub fn pending_bits(&self) -> usize {
        self.bits.len()
    }

    /// The rate [`Segment::Data`] and [`Segment::Ones`] go at, from the next
    /// symbol. Everything else goes at 1200 bit/s whatever this is (10.2.4).
    pub fn set_rate(&mut self, rate: Rate) {
        self.rate = rate;
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// The segment symbols are being made for: the pulse puts them on the
    /// line [`SPAN`] symbols later.
    pub fn sending(&self) -> Option<Kind> {
        self.current.as_ref().map(|c| c.segment.kind())
    }

    /// The segment whose symbol is on the line now.
    pub fn on_air(&self) -> Option<Kind> {
        self.history[SPAN].kind
    }

    /// Segments queued behind the one being sent.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether anything is being sent, queued or still dying away.
    pub fn is_sending(&self) -> bool {
        self.sounding > 0 || self.guard_up > 0 || self.current.is_some() || !self.queue.is_empty()
    }

    /// The next segment to reach or leave the line.
    pub fn sent(&mut self) -> Option<Sent> {
        self.sent.pop_front()
    }

    /// Samples made so far: the clock [`Sent`] is counted on.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Symbols made so far, silence included.
    pub fn symbols(&self) -> u64 {
        self.symbols
    }

    /// Symbols made that have not reached the line yet.
    pub fn lookahead() -> usize {
        SPAN
    }

    /// The next line sample.
    pub fn next_sample(&mut self) -> f64 {
        let mut out = 0.0;
        if self.sounding > 0 {
            let width = 2 * SPAN + 1;
            let row = &self.table[self.frac as usize * width..(self.frac as usize + 1) * width];
            let mut baseband = Complex::ZERO;
            for (tap, slot) in row.iter().zip(&self.history) {
                baseband += slot.symbol * *tap;
            }
            let angle = TAU * self.carrier;
            out = (baseband.re * angle.cos() - baseband.im * angle.sin()) * self.carrier_amplitude;
        }
        // Under everything the answer modem sends, from its first symbol to
        // its last, as `dpsk.rs` has it; but brought up and down gently.
        if self.guard_amplitude > 0.0 {
            let near = self.history.range(SPAN - GUARD_BEHIND..=SPAN + GUARD_AHEAD).any(|s| s.symbol != Complex::ZERO);
            self.guard_up = if near { (self.guard_up + 1).min(self.guard_ramp) } else { self.guard_up.saturating_sub(1) };
        }
        if self.guard_up > 0 {
            let level = 0.5 - 0.5 * (std::f64::consts::PI * self.guard_up as f64 / self.guard_ramp as f64).cos();
            out += level * self.guard_amplitude * (TAU * self.guard).cos();
        }
        self.carrier = (self.carrier + self.carrier_step).fract();
        self.guard = (self.guard + self.guard_step).fract();
        self.now += 1;
        self.frac += self.q;
        while self.frac >= self.p {
            self.frac -= self.p;
            self.shift();
        }
        out
    }

    /// One symbol on: the oldest goes, a new one comes, and the one now in
    /// the middle, whose pulse peaks at the next sample, says what it opens
    /// or closes.
    fn shift(&mut self) {
        if let Some(old) = self.history.pop_front()
            && old.symbol != Complex::ZERO
        {
            self.sounding -= 1;
        }
        let slot = self.pull();
        if slot.symbol != Complex::ZERO {
            self.sounding += 1;
        }
        self.history.push_back(slot);
        self.symbols += 1;
        let centre = self.history[SPAN];
        let half = (self.p / (2 * self.q)).max(1);
        if let Some(kind) = centre.kind {
            if centre.began {
                self.sent.push_back(Sent::Began { kind, at: self.now.saturating_sub(half) });
            }
            if centre.ended {
                self.sent.push_back(Sent::Ended { kind, at: self.now + half });
            }
        }
    }

    /// The next symbol from the queue.
    fn pull(&mut self) -> Slot {
        loop {
            let mut current = match self.current.take() {
                Some(current) => current,
                None => match self.queue.pop_front() {
                    Some(segment) => self.start(segment),
                    None => return Slot::SILENT,
                },
            };
            if self.over(&current) {
                // The symbol that went last is its last, and says so when it
                // reaches the line.
                if current.done > 0
                    && let Some(last) = self.history.back_mut()
                {
                    last.ended = true;
                }
                continue;
            }
            let symbol = self.produce(&mut current);
            current.done += 1;
            let slot = Slot { symbol, kind: Some(current.segment.kind()), began: current.done == 1, ended: false };
            self.current = Some(current);
            return slot;
        }
    }

    fn start(&mut self, segment: Segment) -> Current {
        if matches!(segment, Segment::Alt { .. }) {
            // "The initial state of the scrambler shall be all zeroes"
            // (10.2.4.2), and the differential encoder starts ALT at Z = 0:
            // the clause does not say, and a receiver that expects otherwise
            // loses one symbol of ALT (`plan.md` 8.4).
            self.sender.restart();
        }
        Current { segment, done: 0, position: 0 }
    }

    /// Whether `current` has finished, or may give way to what is queued.
    fn over(&self, current: &Current) -> bool {
        let next = !self.queue.is_empty();
        match &current.segment {
            Segment::Silence(n) | Segment::Sh(n) | Segment::ShBar(n) | Segment::Ones(n) => current.done >= *n,
            Segment::Pph(_) => current.done >= PPH_SYMBOLS,
            Segment::E => current.done >= E_SYMBOLS,
            Segment::Alt { at_least } => current.done >= *at_least && next,
            Segment::Ac => next,
            Segment::Bits(bits) => current.position >= bits.len(),
            Segment::Repeat(bits) => bits.is_empty() || (next && current.done > 0 && current.position.is_multiple_of(bits.len())),
            Segment::Data => next && self.bits.is_empty(),
        }
    }

    /// The point `current`'s next symbol is.
    fn produce(&mut self, current: &mut Current) -> Complex {
        let n = current.done;
        match &current.segment {
            Segment::Silence(_) => Complex::ZERO,
            Segment::Pph(reading) => reading.point(n),
            Segment::Ac => ac(n),
            Segment::Sh(_) => sh(n),
            Segment::ShBar(_) => sh_bar(n),
            // "Alternations of binary 0 and 1", a 0 first: every symbol's two
            // bits are 0 then 1.
            Segment::Alt { .. } => self.differential(&[false, true], Size::Four),
            Segment::E => self.differential(&[true, true], Size::Four),
            Segment::Bits(bits) => {
                let pair: Vec<bool> = (0..2).map(|i| bits.get(current.position + i).copied().unwrap_or(true)).collect();
                current.position += 2;
                self.differential(&pair, Size::Four)
            }
            Segment::Repeat(bits) => {
                let pair: Vec<bool> = (0..2).map(|i| bits[(current.position + i) % bits.len()]).collect();
                current.position += 2;
                self.differential(&pair, Size::Four)
            }
            Segment::Ones(_) => {
                let ones = vec![true; self.rate.bits()];
                self.differential(&ones, self.rate.size())
            }
            Segment::Data => {
                let bits: Vec<bool> = (0..self.rate.bits()).map(|_| self.bits.pop_front().unwrap_or(true)).collect();
                self.differential(&bits, self.rate.size())
            }
        }
    }

    /// 10.2.4's mapping: I1 and I2 turn point `2 Q2 + Q1` of Figure 5
    /// clockwise by Z = Z-1 + 2 I2 + I1, all of it scrambled first --
    /// `signals::Sender::differential` exactly.
    fn differential(&mut self, bits: &[bool], size: Size) -> Complex {
        unit_point(self.sender.differential(bits), size)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SH_BAR_SYMBOLS, SH_SYMBOLS, pph, pph_as_printed};
    use super::*;
    use crate::v32::Mode;
    use crate::v34::constellation::clockwise;
    use crate::v34::signals::Reader;

    const FS: f64 = 16_000.0;

    /// The symbols a transmitter makes, one a call, with what each belongs
    /// to.
    fn symbols(tx: &mut Transmitter, count: usize) -> Vec<(Complex, Option<Kind>)> {
        (0..count)
            .map(|_| {
                let slot = tx.pull();
                tx.history.pop_front();
                tx.history.push_back(slot);
                (slot.symbol, slot.kind)
            })
            .collect()
    }

    fn near(a: Complex, b: Complex) -> bool {
        (a - b).abs() < 1e-12
    }

    /// Integer grid points back from unit-power points of the four-point set.
    fn grid(z: Complex) -> (i32, i32) {
        let s = std::f64::consts::SQRT_2;
        ((z.re * s).round() as i32, (z.im * s).round() as i32)
    }

    #[test]
    fn pph_is_32_symbols_of_the_chosen_reading() {
        for reading in [Reading::WithI, Reading::AsPrinted] {
            let mut tx = Transmitter::new(Side::Call, FS);
            tx.queue(Segment::Pph(reading));
            let got = symbols(&mut tx, PPH_SYMBOLS + 2);
            for (i, (z, kind)) in got.iter().take(PPH_SYMBOLS).enumerate() {
                assert!(near(*z, reading.point(i)), "{reading:?} symbol {i}");
                assert_eq!(*kind, Some(Kind::Pph));
            }
            assert!(got[PPH_SYMBOLS..].iter().all(|(z, k)| *z == Complex::ZERO && k.is_none()));
        }
        // PPh(0) is sent first and is point 0's direction.
        assert!(near(pph(0), Complex::new(1.0, 1.0).scale(std::f64::consts::FRAC_1_SQRT_2)));
        assert!(near(pph_as_printed(0), pph(0)));
    }

    #[test]
    fn sh_s_bar_h_and_ac_are_the_points_10_2_3_3_and_10_2_4_1_name() {
        let mut tx = Transmitter::new(Side::Answer, FS);
        tx.queue(Segment::Sh(SH_SYMBOLS));
        tx.queue(Segment::ShBar(SH_BAR_SYMBOLS));
        tx.queue(Segment::Ac);
        let got: Vec<(i32, i32)> = symbols(&mut tx, 40).into_iter().map(|(z, _)| grid(z)).collect();
        // Sh: point 0, then it turned counterclockwise by 90 degrees, ending
        // on the turned one.
        assert_eq!(got[0], (1, 1));
        assert_eq!(got[1], (-1, 1));
        assert_eq!(got[SH_SYMBOLS - 1], (-1, 1), "Sh ends on point 0 turned counterclockwise 90");
        // S-bar-h: 180, then 270 counterclockwise, beginning with 180.
        assert_eq!(got[SH_SYMBOLS], (-1, -1), "S-bar-h begins on point 0 turned 180");
        assert_eq!(got[SH_SYMBOLS + 1], (1, -1));
        assert_eq!(got[SH_SYMBOLS + SH_BAR_SYMBOLS - 1], (1, -1));
        // AC: point 0 and point 0 turned 180, alternately.
        let ac_from = SH_SYMBOLS + SH_BAR_SYMBOLS;
        for (n, point) in got[ac_from..].iter().enumerate() {
            assert_eq!(*point, if n % 2 == 0 { (1, 1) } else { (-1, -1) });
        }
    }

    #[test]
    fn alt_descrambles_to_alternations_from_a_zero_and_e_to_twenty_ones() {
        for side in [Side::Call, Side::Answer] {
            let mut tx = Transmitter::new(side, FS);
            let mode = match side {
                Side::Call => Mode::Call,
                Side::Answer => Mode::Answer,
            };
            // Something first, so that ALT's restart is seen to matter.
            tx.queue(Segment::Bits(vec![true, false, false, true, true, true]));
            tx.queue(Segment::Alt { at_least: 30 });
            tx.queue(Segment::E);
            let got = symbols(&mut tx, 3 + 30 + E_SYMBOLS);
            // A receiver started where ALT starts reads every bit of it.
            let mut reader = Reader::new(mode);
            let bits: Vec<bool> = got[3..].iter().flat_map(|(z, _)| reader.differential(grid(*z), Size::Four)).collect();
            let alt = &bits[..60];
            for (i, b) in alt.iter().enumerate() {
                assert_eq!(*b, i % 2 == 1, "{side:?} ALT bit {i}");
            }
            assert!(bits[60..].iter().all(|b| *b), "{side:?} E");
            assert_eq!(bits.len() - 60, 20);
            // From a scrambler at zero and Z = 0, the first dibit 0 then 1
            // turns point 0 clockwise by two quarters.
            assert_eq!(grid(got[3].0), clockwise((1, 1), 2));
        }
    }

    #[test]
    fn open_ended_signals_give_way_when_something_is_queued() {
        let mut tx = Transmitter::new(Side::Call, FS);
        // Queued at once: ALT still gets its least.
        tx.queue(Segment::Alt { at_least: 16 });
        tx.queue(Segment::E);
        let kinds: Vec<Option<Kind>> = symbols(&mut tx, 30).into_iter().map(|(_, k)| k).collect();
        assert!(kinds[..16].iter().all(|k| *k == Some(Kind::Alt)));
        assert!(kinds[16..26].iter().all(|k| *k == Some(Kind::E)));
        assert!(kinds[26..].iter().all(Option::is_none), "nothing after E: silence");

        // A repeated MPh finishes the repetition it is in.
        let mut tx = Transmitter::new(Side::Call, FS);
        let mph: Vec<bool> = (0..88).map(|i| i % 3 == 0).collect();
        tx.queue(Segment::Repeat(mph));
        let first = symbols(&mut tx, 50);
        assert!(first.iter().all(|(_, k)| *k == Some(Kind::Repeat)));
        tx.queue(Segment::E);
        let rest: Vec<Option<Kind>> = symbols(&mut tx, 40).into_iter().map(|(_, k)| k).collect();
        // 50 symbols in, the second repetition is 6 symbols old: 38 to go.
        assert!(rest[..38].iter().all(|k| *k == Some(Kind::Repeat)), "{rest:?}");
        assert_eq!(rest[38], Some(Kind::E));

        // Data drains what was queued for it before giving way.
        let mut tx = Transmitter::new(Side::Call, FS);
        tx.send_bits(&[false; 11]);
        tx.queue(Segment::Data);
        tx.queue(Segment::Ones(4));
        let kinds: Vec<Option<Kind>> = symbols(&mut tx, 12).into_iter().map(|(_, k)| k).collect();
        assert!(kinds[..6].iter().all(|k| *k == Some(Kind::Data)), "{kinds:?}");
        assert!(kinds[6..10].iter().all(|k| *k == Some(Kind::Ones)));
        assert_eq!(tx.pending_bits(), 0);
    }

    #[test]
    fn data_comes_back_through_a_reader_at_both_rates() {
        for rate in [Rate::R1200, Rate::R2400] {
            let mut tx = Transmitter::new(Side::Answer, FS);
            tx.set_rate(rate);
            let data: Vec<bool> = (0..400).map(|i| (i * 37 + 5) % 11 < 5).collect();
            tx.queue(Segment::Alt { at_least: 20 });
            tx.queue(Segment::E);
            tx.queue(Segment::Data);
            tx.send_bits(&data);
            let got = symbols(&mut tx, 20 + E_SYMBOLS + 400 / rate.bits());
            let mut reader = Reader::new(Mode::Answer);
            let scale = crate::v34::receiver::unit(rate.size());
            let mut bits = Vec::new();
            for (i, (z, _)) in got.iter().enumerate() {
                let size = if i < 20 + E_SYMBOLS { Size::Four } else { rate.size() };
                let s = crate::v34::receiver::unit(size);
                let point = ((z.re / s).round() as i32, (z.im / s).round() as i32);
                bits.extend(reader.differential(point, size));
            }
            assert_eq!(&bits[bits.len() - 400..], &data[..], "{rate:?}");
            if rate == Rate::R2400 {
                // Sixteen points at unit mean power.
                let tail = &got[20 + E_SYMBOLS..];
                let power = tail.iter().map(|(z, _)| z.norm_sqr()).sum::<f64>() / tail.len() as f64;
                assert!((power - 1.0).abs() < 0.2, "{power}");
                assert!(tail.iter().any(|(z, _)| (z.re.abs() - 3.0 * scale).abs() < 1e-9));
            }
        }
    }

    #[test]
    fn segments_say_when_they_reach_and_leave_the_line() {
        let mut tx = Transmitter::new(Side::Call, FS);
        tx.queue(Segment::Silence(42));
        tx.queue(Segment::Pph(Reading::WithI));
        tx.queue(Segment::Alt { at_least: 16 });
        tx.queue(Segment::E);
        let mut sent = Vec::new();
        for _ in 0..(FS as usize) / 5 {
            tx.next_sample();
            sent.extend(std::iter::from_fn(|| tx.sent()));
        }
        let symbol = FS / 600.0;
        let at = |s: &Sent| match s {
            Sent::Began { at, .. } | Sent::Ended { at, .. } => *at as f64,
        };
        let kinds: Vec<Sent> = sent.clone();
        assert_eq!(kinds.len(), 8, "{kinds:?}");
        assert!(matches!(kinds[0], Sent::Began { kind: Kind::Silence, .. }), "{kinds:?}");
        assert!(matches!(kinds[2], Sent::Began { kind: Kind::Pph, .. }));
        assert!(matches!(kinds[3], Sent::Ended { kind: Kind::Pph, .. }));
        assert!(matches!(kinds[7], Sent::Ended { kind: Kind::E, .. }));
        // PPh is 32 symbols long, and ALT 16, on the line.
        assert!((at(&kinds[3]) - at(&kinds[2]) - 32.0 * symbol).abs() <= 2.0);
        assert!((at(&kinds[5]) - at(&kinds[4]) - 16.0 * symbol).abs() <= 2.0);
        // Silence reaches the line after the pulse's reach, and PPh 70 ms
        // after that.
        assert!((at(&kinds[2]) - at(&kinds[0]) - 0.070 * FS).abs() <= 2.0);
        assert!(!tx.is_sending());
    }

    #[test]
    fn the_answer_modem_is_silent_through_a_silence_guard_tone_and_all() {
        // PPh, clause 12's 70 ms, PPh: between the first's end and the
        // second's beginning the guard tone has to come down and go back up,
        // and leave most of the 70 ms to silence.
        let mut tx = Transmitter::new(Side::Answer, FS);
        tx.queue(Segment::Pph(Reading::WithI));
        tx.queue(Segment::Silence(42));
        tx.queue(Segment::Pph(Reading::WithI));
        let mut samples = Vec::new();
        let mut sent = Vec::new();
        while tx.is_sending() {
            samples.push(tx.next_sample());
            sent.extend(std::iter::from_fn(|| tx.sent()));
        }
        let ended = sent.iter().find(|s| matches!(s, Sent::Ended { kind: Kind::Pph, .. })).copied();
        let Some(Sent::Ended { at: ended, .. }) = ended else { panic!("{sent:?}") };
        let Some(Sent::Began { at: began, .. }) = sent.iter().rev().find(|s| matches!(s, Sent::Began { kind: Kind::Pph, .. })).copied()
        else {
            panic!("{sent:?}")
        };
        assert!((began - ended) as f64 >= 0.070 * FS - 2.0, "{sent:?}");
        // Quiet to 60 dB under the nominal level from 12 ms after the one to
        // 12 ms before the other: 46 ms of the 70.
        let margin = (0.012 * FS) as u64;
        let quiet = &samples[(ended + margin) as usize..(began - margin) as usize];
        let peak = quiet.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        assert!(peak < 1e-3, "{peak:.2e} in the silence");
        // And the tone is up by the time the second PPh's first symbol peaks.
        let at = (began + (FS / 1200.0) as u64) as usize;
        let around = &samples[at - 40..at + 40];
        assert!(around.iter().fold(0.0f64, |m, x| m.max(x.abs())) > 0.3);
    }

    /// Averaged power spectrum of `n`-point frames of a transmitter sending
    /// random data at `rate`.
    fn spectrum(side: Side, rate: Rate, n: usize, frames: usize) -> Vec<f64> {
        let mut tx = Transmitter::new(side, FS);
        tx.set_rate(rate);
        let mut seed = 99u32;
        let bits: Vec<bool> = (0..(frames * n) / 6 + 400)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed & 1 == 1
            })
            .collect();
        tx.queue(Segment::Data);
        tx.send_bits(&bits);
        for _ in 0..2000 {
            tx.next_sample();
        }
        let fft = dsp::Fft::new(n);
        let window: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()).collect();
        let mut power = vec![0.0f64; n / 2];
        for _ in 0..frames {
            let mut re: Vec<f64> = (0..n).map(|i| tx.next_sample() * window[i]).collect();
            let mut im = vec![0.0; n];
            fft.process(&mut re, &mut im);
            for k in 0..n / 2 {
                power[k] += re[k] * re[k] + im[k] * im[k];
            }
        }
        power
    }

    #[test]
    fn the_spectrum_sits_inside_figure_13_at_both_rates_and_on_both_carriers() {
        let n = 4096;
        let hz = FS / n as f64;
        for side in [Side::Call, Side::Answer] {
            for rate in [Rate::R1200, Rate::R2400] {
                let power = spectrum(side, rate, n, 60);
                let carrier = side.carrier();
                let at = |offset: f64| {
                    let centre = ((carrier + offset) / hz).round() as usize;
                    (centre - 2..=centre + 2).map(|k| power[k]).sum::<f64>() / 5.0
                };
                let reference = at(0.0);
                let db = |offset: f64| 10.0 * (at(offset) / reference).log10();
                for sign in [-1.0, 1.0] {
                    let within = |offset: f64, low: f64, high: f64| {
                        let v = db(sign * offset);
                        assert!(
                            (low..=high).contains(&v),
                            "{side:?} {rate:?} {} Hz: {v:.2} dB, not {low} to {high}",
                            sign * offset
                        );
                    };
                    // Figure 13, read off the rendered page (PDF page 33).
                    within(125.0, -0.75, 0.75);
                    within(300.0, -4.0, -2.0);
                    within(400.0, -9.0, -5.0);
                    within(450.0, -9.0 - 11.0 * 50.0 / 75.0, -9.0);
                    let far = db(sign * 560.0);
                    assert!(far < -20.0, "{side:?} {rate:?} {} Hz: {far:.2} dB", sign * 560.0);
                }
            }
        }
    }

    #[test]
    fn the_guard_tone_is_six_decibels_under_the_answer_carrier() {
        // 7 dB under the nominal level against the carrier's 1 dB under it:
        // measured as the power in the guard tone's bins against the power
        // of everything within 600 Hz of the carrier.
        let n = 4096;
        let hz = FS / n as f64;
        let power = spectrum(Side::Answer, Rate::R2400, n, 40);
        let band = |from: f64, to: f64| ((from / hz).round() as usize..=(to / hz).round() as usize).map(|k| power[k]).sum::<f64>();
        let guard = band(1800.0 - 12.0, 1800.0 + 12.0);
        let carrier = band(2400.0 - 600.0 + 20.0, 2400.0 + 600.0);
        let db = 10.0 * (guard / carrier).log10();
        assert!((db + 6.0).abs() < 0.3, "guard tone {db:.2} dB against the carrier");
        // And the call modem has none.
        let power = spectrum(Side::Call, Rate::R1200, n, 10);
        let bins = (1800.0 / hz).round() as usize;
        let total: f64 = power.iter().sum();
        assert!(power[bins - 3..=bins + 3].iter().sum::<f64>() / total < 1e-6);
    }

    #[test]
    fn each_side_leaves_at_the_nominal_level() {
        // The call modem's carrier at the nominal level, and the answer
        // modem's 1 dB under with a guard tone 7 dB under, which between them
        // come back to within a whisker of it -- on every signal, whatever
        // its constellation.
        let segments = [Segment::Pph(Reading::WithI), Segment::Ac, Segment::Sh(1000), Segment::Alt { at_least: 1000 }, Segment::Data];
        for side in [Side::Call, Side::Answer] {
            for segment in &segments {
                for rate in [Rate::R1200, Rate::R2400] {
                    let mut tx = Transmitter::new(side, FS);
                    tx.set_rate(rate);
                    // PPh repeated, to have enough of it.
                    for _ in 0..40 {
                        tx.queue(segment.clone());
                    }
                    let samples: Vec<f64> = (0..(FS as usize)).map(|_| tx.next_sample()).collect();
                    let steady = &samples[4000..12000];
                    let rms = (steady.iter().map(|x| x * x).sum::<f64>() / steady.len() as f64).sqrt();
                    let db = 20.0 * (rms / std::f64::consts::FRAC_1_SQRT_2).log10();
                    assert!(db.abs() < 0.3, "{side:?} {segment:?} {rate:?}: {db:.2} dB from nominal");
                }
            }
        }
    }

    #[test]
    fn little_of_this_end_lands_in_the_far_end_band() {
        // What a two-wire line brings back into this end's own receiver is
        // this end's signal in the other direction's band: 675 Hz and more
        // from its own carrier. The pulse is spread over sixteen symbols and
        // tapered so that there is next to nothing there.
        let n = 4096;
        let hz = FS / n as f64;
        for (side, far) in [(Side::Call, 2400.0), (Side::Answer, 1200.0)] {
            let power = spectrum(side, Rate::R2400, n, 40);
            let carrier = side.carrier();
            let bin = |f: f64| (f / hz).round() as usize;
            let own = power[bin(carrier - 75.0)..=bin(carrier + 75.0)].iter().sum::<f64>() / (bin(carrier + 75.0) - bin(carrier - 75.0) + 1) as f64;
            let (from, to) = (far - 525.0, far + 525.0);
            let worst = power[bin(from)..=bin(to)]
                .iter()
                .enumerate()
                .filter(|(k, _)| side == Side::Call || ((bin(from) + k) as f64 * hz - 1800.0).abs() > 30.0)
                .map(|(_, p)| *p)
                .fold(0.0, f64::max);
            let db = 10.0 * (worst / own).log10();
            assert!(db < -60.0, "{side:?}: {db:.1} dB in the far band");
        }
    }
}
