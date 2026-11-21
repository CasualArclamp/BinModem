//! HST's back channel: the slow direction.
//!
//! From the Courier's DSP code (06/09/95 build):
//!
//! - Two variants, picked by one flag bit: increment 0x6AAA on a 72-tap
//!   polyphase filter stepped 6 a pass, or 0x638E stepped 4. At the DSP's
//!   7200 Hz that is a symbol tick of 300 or 200 a second on a carrier of
//!   375 or 350 Hz. Every other tick carries a zero, so the real symbol rate
//!   is 150 or 100 baud, and three bits a symbol makes 450 or 300 bit/s:
//!   the rates USR's manuals give.
//! - Differential eight-phase: three scrambled bits pick a phase step from the
//!   constant 0x43705261 read a nibble at a time from the low end, so tribit
//!   0 steps 1, tribit 1 steps 6, and so on; the step is added to the phase
//!   modulo eight, and the points sit at 22.5 + 45k degrees.
//! - Scrambler: new bits enter at bit 17 and the feedback is the register
//!   xor itself three places down, which is 1 + x^-14 + x^-17.
//!
//! Bits go least significant first, as everything else on the Courier does,
//! so the first of a symbol's three bits is the tribit's low bit. That and
//! everything about the pulse and preamble below are this module's choices:
//! the Courier's 72-tap shaping and its 17-tick start are not reproduced.

use std::collections::VecDeque;
use std::f64::consts::{FRAC_PI_4, FRAC_PI_8};

use dsp::{Cascade, ComplexFir, Gardner, Nco, OnePole, bandpass, butter_lowpass, rrc_at, rrc_taps};

/// Phase step, in eighths of a turn, for each scrambled tribit: the nibbles
/// of the Courier's constant 0x43705261, low nibble first.
pub const PHASE_STEP: [u8; 8] = [1, 6, 2, 5, 0, 7, 3, 4];

/// The tribit for each phase step, [`PHASE_STEP`] inverted.
fn tribit_for_step(step: u8) -> u8 {
    PHASE_STEP.iter().position(|&s| s == step).map_or(0, |p| p as u8)
}

/// Back-channel speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackRate {
    /// 450 bit/s: 150 baud on 375 Hz.
    #[default]
    B450,
    /// 300 bit/s: 100 baud on 350 Hz.
    B300,
}

impl BackRate {
    pub fn baud(self) -> f64 {
        match self {
            BackRate::B450 => 150.0,
            BackRate::B300 => 100.0,
        }
    }

    pub fn carrier(self) -> f64 {
        match self {
            BackRate::B450 => 375.0,
            BackRate::B300 => 350.0,
        }
    }

    pub fn bits_per_second(self) -> u32 {
        (self.baud() * 3.0) as u32
    }
}

/// The back channel's scrambler, 1 + x^-14 + x^-17, self-synchronising.
#[derive(Debug, Clone, Default)]
pub struct Scrambler {
    register: u32,
}

impl Scrambler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scramble(&mut self, bit: bool) -> bool {
        let feedback = ((self.register >> 13) ^ (self.register >> 16)) & 1 != 0;
        let out = bit ^ feedback;
        self.register = (self.register << 1) | u32::from(out);
        out
    }

    pub fn descramble(&mut self, bit: bool) -> bool {
        let feedback = ((self.register >> 13) ^ (self.register >> 16)) & 1 != 0;
        self.register = (self.register << 1) | u32::from(bit);
        bit ^ feedback
    }
}

/// Root-raised-cosine roll-off of this module's pulse.
const ROLLOFF: f64 = 0.35;
/// Symbols either side of the centre the pulse is evaluated over.
const SPAN: usize = 4;
/// Scrambled ones sent before anything queued, for the far receiver's timing
/// loop and descrambler: 0.4 s at 450 bit/s.
pub const PREAMBLE_SYMBOLS: u32 = 60;

/// The point for a phase in eighths of a turn: 22.5 + 45k degrees.
fn point(phase: u8) -> (f64, f64) {
    let a = FRAC_PI_8 + FRAC_PI_4 * f64::from(phase & 7);
    (a.cos(), a.sin())
}

/// Back-channel transmitter.
#[derive(Debug)]
pub struct BackTx {
    fs: f64,
    rate: BackRate,
    nco: Nco,
    on: bool,
    /// Fraction of a symbol into the current one.
    clock: f64,
    history: Vec<(f64, f64)>,
    phase: u8,
    scrambler: Scrambler,
    pending: VecDeque<bool>,
    preamble: u32,
    gain: f64,
}

impl BackTx {
    pub fn new(fs: f64) -> Self {
        let rate = BackRate::default();
        Self {
            fs,
            rate,
            nco: Nco::new(rate.carrier(), fs),
            on: false,
            clock: 0.0,
            history: vec![(0.0, 0.0); 2 * SPAN + 1],
            phase: 0,
            scrambler: Scrambler::new(),
            pending: VecDeque::new(),
            preamble: 0,
            gain: 1.0,
        }
    }

    /// Output level relative to a unit-power constellation.
    pub fn set_gain(&mut self, gain: f64) {
        self.gain = gain;
    }

    /// Raise the carrier: the preamble, then whatever is queued.
    pub fn start(&mut self, rate: BackRate) {
        self.rate = rate;
        self.nco = Nco::new(rate.carrier(), self.fs);
        self.on = true;
        self.clock = 0.0;
        self.history.fill((0.0, 0.0));
        self.phase = 0;
        self.scrambler = Scrambler::new();
        self.preamble = PREAMBLE_SYMBOLS;
    }

    pub fn stop(&mut self) {
        self.on = false;
        self.pending.clear();
    }

    pub fn rate(&self) -> BackRate {
        self.rate
    }

    pub fn is_transmitting(&self) -> bool {
        self.on
    }

    /// Whether the preamble is over and queued bits are going out.
    pub fn trained(&self) -> bool {
        self.on && self.preamble == 0
    }

    /// Queue bits, first to go first. With nothing queued it sends ones.
    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend(bits.iter().copied());
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }

    fn next_symbol(&mut self) -> (f64, f64) {
        let mut tribit = 0u8;
        for i in 0..3 {
            let bit = if self.preamble > 0 { true } else { self.pending.pop_front().unwrap_or(true) };
            tribit |= u8::from(self.scrambler.scramble(bit)) << i;
        }
        self.preamble = self.preamble.saturating_sub(1);
        self.phase = (self.phase + PHASE_STEP[usize::from(tribit)]) & 7;
        point(self.phase)
    }

    pub fn next_sample(&mut self) -> f64 {
        if !self.on {
            self.nco.step();
            return 0.0;
        }
        self.clock += self.rate.baud() / self.fs;
        while self.clock >= 1.0 {
            self.clock -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol();
            self.history.push(symbol);
        }
        let centre = SPAN as f64;
        let mut bb = (0.0, 0.0);
        for (i, &(re, im)) in self.history.iter().enumerate() {
            let tap = rrc_at(self.clock + centre - i as f64, ROLLOFF);
            bb.0 += re * tap;
            bb.1 += im * tap;
        }
        let (cos, sin) = self.nco.step();
        self.gain * (bb.0 * cos - bb.1 * sin)
    }
}

/// Band the back-channel receiver lets in from the line, in hertz. Wide of
/// either variant on purpose: a tighter guard here cost more in phase
/// distortion than it bought, since the matched filter after the
/// down-conversion is what does the selecting.
const RX_BAND: (f64, f64) = (150.0, 650.0);
/// Mean symbol power, relative to the receiver's own reference, above which
/// the carrier counts as present, and below which it counts as gone.
const CARRIER_ON: f64 = 1e-4;
const CARRIER_OFF: f64 = 3e-5;

/// Rate the receiver works at once the band is down at baseband, roughly:
/// the line rate divided by a whole number to land near this.
const WORK_RATE: f64 = 1600.0;

/// Back-channel receiver.
#[derive(Debug)]
pub struct BackRx {
    fs: f64,
    rate: BackRate,
    guard: Cascade,
    nco: Nco,
    lp_i: Cascade,
    lp_q: Cascade,
    /// Line samples to each baseband one, and how far into the current run.
    decimate: usize,
    skip: usize,
    matched: ComplexFir,
    gardner: Gardner,
    countdown: f64,
    previous: (f64, f64),
    last_symbol: Option<(f64, f64)>,
    descrambler: Scrambler,
    power: OnePole,
    carrier: bool,
    bits: Vec<bool>,
    /// Mean squared distance of each differential decision from its ideal
    /// angle, in radians squared: a quality figure.
    error: OnePole,
}

impl BackRx {
    pub fn new(fs: f64) -> Self {
        Self::with_rate(fs, BackRate::default())
    }

    pub fn with_rate(fs: f64, rate: BackRate) -> Self {
        let decimate = ((fs / WORK_RATE).round() as usize).max(1);
        let work = fs / decimate as f64;
        let sps = work / rate.baud();
        Self {
            fs,
            rate,
            guard: bandpass(6, RX_BAND.0, RX_BAND.1, fs),
            nco: Nco::new(rate.carrier(), fs),
            // Ahead of the decimation: wide of the channel, which the matched
            // filter after it shapes, and well short of the new Nyquist.
            lp_i: butter_lowpass(4, 3.0 * rate.baud(), fs),
            lp_q: butter_lowpass(4, 3.0 * rate.baud(), fs),
            decimate,
            skip: 0,
            matched: ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN)),
            gardner: Gardner::new(sps, 0.02 * sps),
            countdown: sps / 2.0,
            previous: (0.0, 0.0),
            last_symbol: None,
            descrambler: Scrambler::new(),
            power: OnePole::new(0.050, rate.baud()),
            carrier: false,
            bits: Vec::new(),
            error: OnePole::starting_at(0.5, 0.2, rate.baud()),
        }
    }

    pub fn rate(&self) -> BackRate {
        self.rate
    }

    /// Change speed and forget the line.
    pub fn set_rate(&mut self, rate: BackRate) {
        *self = Self::with_rate(self.fs, rate);
    }

    pub fn restart(&mut self) {
        *self = Self::with_rate(self.fs, self.rate);
    }

    pub fn feed(&mut self, sample: f64) {
        let x = self.guard.process(sample);
        let (cos, sin) = self.nco.step();
        let bb = (self.lp_i.process(2.0 * x * cos), self.lp_q.process(-2.0 * x * sin));
        self.skip += 1;
        if self.skip < self.decimate {
            return;
        }
        self.skip = 0;
        let filtered = self.matched.process(bb);
        self.countdown -= 1.0;
        if self.countdown <= 0.0 {
            // Linear interpolation back to where the instant fell.
            let frac = 1.0 + self.countdown;
            let at = (
                self.previous.0 + (filtered.0 - self.previous.0) * frac,
                self.previous.1 + (filtered.1 - self.previous.1) * frac,
            );
            if let Some(symbol) = self.gardner.feed(at) {
                self.symbol(symbol);
            }
            self.countdown += self.gardner.interval();
        }
        self.previous = filtered;
    }

    fn symbol(&mut self, z: (f64, f64)) {
        let p = self.power.process(z.0 * z.0 + z.1 * z.1);
        self.carrier = if self.carrier { p > CARRIER_OFF } else { p > CARRIER_ON };
        let Some(prev) = self.last_symbol.replace(z) else { return };
        if !self.carrier {
            return;
        }
        // z * conj(prev): the turn since the last symbol.
        let d = (z.0 * prev.0 + z.1 * prev.1, z.1 * prev.0 - z.0 * prev.1);
        let angle = d.1.atan2(d.0);
        let eighths = (angle / FRAC_PI_4).round();
        let miss = angle - eighths * FRAC_PI_4;
        self.error.process(miss * miss);
        let step = (eighths as i64).rem_euclid(8) as u8;
        let tribit = tribit_for_step(step);
        for i in 0..3 {
            let bit = self.descrambler.descramble(tribit >> i & 1 != 0);
            self.bits.push(bit);
        }
    }

    /// Bits decoded since the last call, first received first.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    pub fn carrier(&self) -> bool {
        self.carrier
    }

    /// Rough signal to noise from the spread of the phase decisions, in dB.
    pub fn snr_db(&self) -> f64 {
        // For phase noise of variance v on a unit circle, SNR ~ 1 / (2v).
        -10.0 * (2.0 * self.error.value().max(1e-9)).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_step_table_is_a_permutation() {
        let mut seen = [false; 8];
        for &s in &PHASE_STEP {
            seen[usize::from(s)] = true;
        }
        assert!(seen.iter().all(|&s| s));
        for t in 0..8u8 {
            assert_eq!(tribit_for_step(PHASE_STEP[usize::from(t)]), t);
        }
    }

    #[test]
    fn the_scrambler_undoes_itself() {
        let mut s = Scrambler::new();
        let mut d = Scrambler::new();
        let bits: Vec<bool> = (0..500u32).map(|i| (i * 7919 + i / 3) % 5 < 2).collect();
        let out: Vec<bool> = bits.iter().map(|&b| d.descramble(s.scramble(b))).collect();
        assert_eq!(out, bits);
    }

    fn loopback(fs: f64, rate: BackRate) -> (Vec<bool>, Vec<bool>) {
        let mut tx = BackTx::new(fs);
        let mut rx = BackRx::with_rate(fs, rate);
        tx.start(rate);
        let data: Vec<bool> = (0..900u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) & 1 == 1).collect();
        tx.push_bits(&data);
        let seconds = f64::from(PREAMBLE_SYMBOLS) / rate.baud() + data.len() as f64 / f64::from(rate.bits_per_second()) + 0.5;
        let mut got = Vec::new();
        for _ in 0..(seconds * fs) as usize {
            rx.feed(0.3 * tx.next_sample());
            got.extend(rx.take_bits());
        }
        (data, got)
    }

    /// Where `want` first appears in `got`, as a whole.
    fn find(got: &[bool], want: &[bool]) -> Option<usize> {
        got.windows(want.len()).position(|w| w == want)
    }

    #[test]
    fn it_carries_data_to_itself_at_both_speeds_and_rates() {
        for fs in [8000.0, 16_000.0] {
            for rate in [BackRate::B450, BackRate::B300] {
                let (data, got) = loopback(fs, rate);
                assert!(
                    find(&got, &data).is_some(),
                    "{rate:?} at {fs}: sent {} bits, none of the run came back whole",
                    data.len()
                );
            }
        }
    }
}
