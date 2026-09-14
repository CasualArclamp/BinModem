//! The modulation the INFO sequences ride on (10.1.2.3.1): binary DPSK at
//! 600 bit/s, on 1200 Hz from the call modem and 2400 Hz from the answer
//! modem.
//!
//! "The transmit point is rotated 180 degrees from the previous point if the
//! transmit bit is a 1, and ... 0 degrees ... if the transmit bit is a 0." A
//! carrier with no reversals in it is a string of zeros, which is also what
//! tones A and B are between their phase reversals, so the same modulator
//! carries a sequence straight on into the tone that follows it.
//!
//! The two directions are 1200 Hz apart, and that is the whole of what keeps
//! them apart in phase 2: both modems talk at once, and until phase 3 there is
//! no echo canceller trained to take one off the other. A channel filter does
//! it, as it does for V.22bis.

use std::collections::VecDeque;

use dsp::{ComplexFir, Nco, fir_lowpass, rrc_at, rrc_taps};

use super::info::{self, Info, Info0, Info1a, Info1c};

/// "600 bit/s ± 0.01%", one bit a symbol.
pub const BAUD: f64 = 600.0;

/// Excess bandwidth of the pulse, from Figure 13.
///
/// The recommendation gives a template rather than a filter: flat to within
/// 0.75 dB out to 125 Hz either side of the carrier, 2 to 4 dB down at 300,
/// 5 to 9 at 400, and at least 20 down past 550. A root-raised-cosine at 600
/// baud with three-quarters roll-off lands inside every one of those: -0.1 dB
/// at 125, -3 at 300, -7.5 at 400, -11.7 at 450, and nothing past 525. Half
/// is too narrow at 300 Hz and a full roll-off too wide at 450.
pub const ROLLOFF: f64 = 0.75;

/// Symbols each side of centre in the shaping filter.
const SPAN: usize = 6;

/// The 1800 Hz guard tone the answer modem sends under its INFO sequences
/// and tone A.
pub const GUARD_TONE: f64 = 1800.0;

/// Which modem is sending, which is what decides the carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Call,
    Answer,
}

impl Side {
    /// "INFO sequences are transmitted by the answer modem with a carrier
    /// frequency of 2400 Hz ... by the call modem with a carrier frequency of
    /// 1200 Hz", and tones A and B are those same two frequencies.
    pub fn carrier(self) -> f64 {
        match self {
            Self::Call => 1200.0,
            Self::Answer => 2400.0,
        }
    }

    /// The carrier's amplitude, against a nominal transmit level of one.
    ///
    /// The answer modem sends its carrier "at 1 dB below the nominal transmit
    /// power, plus a 1800 Hz ... guard tone 7 dB below", which between them
    /// come back to within a whisker of the nominal. The call modem sends its
    /// carrier at the nominal power and no guard tone at all.
    fn carrier_amplitude(self) -> f64 {
        match self {
            Self::Call => 1.0,
            Self::Answer => 10f64.powf(-1.0 / 20.0),
        }
    }

    fn guard_amplitude(self) -> f64 {
        match self {
            Self::Call => 0.0,
            Self::Answer => 10f64.powf(-7.0 / 20.0),
        }
    }

    /// The lengths of the sequences this side sends: INFO0 from either, and
    /// then INFO1c from the call modem or INFO1a from the answer modem.
    fn lengths(self) -> [usize; 2] {
        match self {
            Self::Call => [info::INFO0_BITS, info::INFO1C_BITS],
            Self::Answer => [info::INFO0_BITS, info::INFO1A_BITS],
        }
    }
}

/// INFO sequences and tones, as line samples.
#[derive(Debug, Clone)]
pub struct Transmitter {
    side: Side,
    fs: f64,
    carrier: Nco,
    guard: Nco,
    /// How far through the current symbol, from 0 to 1.
    clock: f64,
    /// The symbols the pulse is spread across, oldest first.
    history: Vec<f64>,
    pending: VecDeque<bool>,
    /// The point the carrier is on: one or minus one.
    point: f64,
    /// Whether the carrier is on. Off, the modulator sends nothing, and the
    /// next sequence starts with a point of its own.
    on: bool,
}

impl Transmitter {
    pub fn new(side: Side, fs: f64) -> Self {
        Self {
            side,
            fs,
            carrier: Nco::new(side.carrier(), fs),
            guard: Nco::new(GUARD_TONE, fs),
            clock: 0.0,
            history: vec![0.0; 2 * SPAN + 1],
            pending: VecDeque::new(),
            point: 1.0,
            on: false,
        }
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// Queue a sequence, or bits of one.
    ///
    /// "Each INFO sequence is preceded by a point at an arbitrary carrier
    /// phase. When multiple INFO sequences are transmitted as a group, only
    /// the first sequence is preceded by" one: a sequence queued while the
    /// carrier is already up follows straight on from it.
    pub fn send(&mut self, bits: &[bool]) {
        self.on = true;
        self.pending.extend(bits.iter().copied());
    }

    /// Stop once whatever is queued has gone: the carrier falls to nothing
    /// through the pulse rather than being cut.
    pub fn silence(&mut self) {
        self.on = false;
    }

    /// Bits still to go.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Turn the carrier half way round, on the very next sample.
    ///
    /// Tones A and B mark time with their phase reversals (10.1.2.1 and
    /// 10.1.2.2), and a round trip is measured between two of them, so a
    /// reversal belongs at a sample and not at the next symbol boundary a
    /// millisecond and a half away. Every symbol the pulse is still spread
    /// across is turned round with it, which turns the whole of the output
    /// round at once: a carrier with nothing on it is the same point over and
    /// over, and those points sum to exactly the carrier.
    pub fn reverse(&mut self) {
        self.point = -self.point;
        for symbol in &mut self.history {
            *symbol = -*symbol;
        }
    }

    /// Stop dead: nothing queued, nothing still dying away, and the next
    /// sequence starting from a point of its own.
    ///
    /// For the handover to a signal that is not this one -- L1 follows tone A
    /// and tone B directly, and a tail of the tone underneath the probing
    /// would be read as part of the line.
    pub fn stop(&mut self) {
        self.pending.clear();
        self.history.fill(0.0);
        self.on = false;
    }

    /// Whether the carrier is on or still dying away.
    pub fn is_sending(&self) -> bool {
        self.on || !self.pending.is_empty() || self.history.iter().any(|s| *s != 0.0)
    }

    fn next_symbol(&mut self) -> f64 {
        if let Some(bit) = self.pending.pop_front() {
            if self.history.iter().all(|s| *s == 0.0) {
                // From silence, the first symbol is the arbitrary point, and
                // the bit waits for the next.
                self.pending.push_front(bit);
                return self.point;
            }
            if bit {
                self.point = -self.point;
            }
            return self.point;
        }
        if self.on { self.point } else { 0.0 }
    }

    pub fn next_sample(&mut self) -> f64 {
        self.clock += BAUD / self.fs;
        while self.clock >= 1.0 {
            self.clock -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol();
            self.history.push(symbol);
        }
        // Output runs SPAN symbols behind the newest, so the pulse is centred
        // on symbols that have already been chosen. The offset grows with the
        // clock, so that when the clock wraps and the history shifts the two
        // cancel and no symbol's pulse jumps (as in V.22bis).
        let centre = SPAN as f64;
        let mut baseband = 0.0;
        for (i, &symbol) in self.history.iter().enumerate() {
            if symbol != 0.0 {
                baseband += symbol * rrc_at(self.clock + centre - i as f64, ROLLOFF);
            }
        }
        let (cos, _) = self.carrier.step();
        let (guard, _) = self.guard.step();
        let sounding = self.history.iter().any(|s| *s != 0.0);
        let guard = if sounding { guard * self.side.guard_amplitude() } else { 0.0 };
        baseband * cos * self.side.carrier_amplitude() + guard
    }
}

/// How many sampling instants a symbol is tried at.
///
/// Eight, so no guess is more than a sixteenth of a symbol from the right one,
/// where a raised cosine with this much roll-off has closed to within a few
/// per cent of its opening.
const PHASES: usize = 8;

/// One way of sampling the stream: an instant within the symbol, and the bits
/// that instant has decided.
#[derive(Debug, Clone)]
struct Branch {
    /// The next instant to sample at, in samples since the receiver started.
    next: f64,
    previous_symbol: (f64, f64),
    /// The most recent bits, as many as the longest sequence this side sends.
    bits: VecDeque<bool>,
}

/// INFO sequences out of line samples.
///
/// Without a timing loop. An INFO sequence has no preamble -- one point at an
/// arbitrary phase, four fill ones and the frame sync, and then the
/// information -- so a loop has a dozen symbols to find the instant in before
/// the bits it has got wrong are bits of the sync. A loop that crosses half a
/// symbol in a few tens of symbols loses the first sequence of every call it
/// starts badly placed for, and it did.
///
/// What saves having one is 10.1.2.3.1's clock: "600 bit/s ± 0.01%". Across
/// the longest sequence, 109 bits, that is a hundredth of a symbol of drift,
/// so the right instant for its first bit is the right instant for its last.
/// So every instant is tried at once, eight to a symbol, each deciding its own
/// bits, and the CRC says which of them was right.
#[derive(Debug, Clone)]
pub struct Receiver {
    side: Side,
    nco: Nco,
    select: ComplexFir,
    matched: ComplexFir,
    sps: f64,
    /// Samples fed, and the matched filter's output at the last of them.
    now: f64,
    previous_filtered: (f64, f64),
    branches: Vec<Branch>,
    /// Smoothed magnitude of the selected channel.
    level: f64,
    level_step: f64,
}

impl Receiver {
    /// A receiver for what `side` sends.
    pub fn new(side: Side, fs: f64) -> Self {
        let sps = fs / BAUD;
        Self {
            side,
            nco: Nco::new(side.carrier(), fs),
            // Linear phase, for the reason V.22bis's is. The other direction
            // is 1200 Hz away and the answer modem's own guard tone 600 Hz
            // away; at baseband both sit past the matched filter's 525 Hz
            // edge, and this is what holds them there. Scaled with the sample
            // rate so its transition band is the same width at every rate.
            select: ComplexFir::new(fir_lowpass(560.0, (fs / 40.0) as usize | 1, fs)),
            matched: ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN)),
            sps,
            now: 0.0,
            previous_filtered: (0.0, 0.0),
            branches: (0..PHASES)
                .map(|k| Branch {
                    next: 1.0 + sps * k as f64 / PHASES as f64,
                    previous_symbol: (0.0, 0.0),
                    bits: VecDeque::with_capacity(info::INFO1C_BITS),
                })
                .collect(),
            level: 0.0,
            level_step: 1.0 - (-1.0 / (0.020 * fs)).exp(),
        }
    }

    /// How strong the carrier in this receiver's band is, as a magnitude.
    pub fn level(&self) -> f64 {
        self.level
    }

    /// Feed one line sample. Yields a sequence the moment its CRC checks.
    pub fn feed(&mut self, sample: f64) -> Option<Info> {
        let (cos, sin) = self.nco.step();
        let selected = self.select.process((sample * cos, sample * -sin));
        let magnitude = (selected.0 * selected.0 + selected.1 * selected.1).sqrt();
        self.level += self.level_step * (magnitude - self.level);
        let filtered = self.matched.process(selected);
        let previous = std::mem::replace(&mut self.previous_filtered, filtered);
        self.now += 1.0;
        let mut found = None;
        for branch in &mut self.branches {
            while branch.next <= self.now {
                // Interpolated to where the instant falls between the last
                // two samples: 16 kHz against 600 baud is 26.67 to a symbol.
                let mu = 1.0 - (self.now - branch.next);
                let at = (
                    previous.0 + mu * (filtered.0 - previous.0),
                    previous.1 + mu * (filtered.1 - previous.1),
                );
                branch.next += self.sps;
                if found.is_none() {
                    found = decide(branch, at, self.side);
                }
            }
        }
        if found.is_some() {
            // Every bit of it has been used, by every branch that was reading
            // it, and none of them may start another.
            for branch in &mut self.branches {
                branch.bits.clear();
            }
        }
        found
    }
}

/// One branch's decision on a symbol, and the sequence it completes if any.
fn decide(branch: &mut Branch, symbol: (f64, f64), side: Side) -> Option<Info> {
    // The differential decision: a point turned half way round from the last
    // one is a 1. Nothing about the carrier's absolute phase matters, which is
    // why there is no carrier loop here at all -- a few hertz of offset turns
    // the point a few degrees a symbol, and a decision that only asks "same or
    // opposite" does not notice.
    let (re, im) = branch.previous_symbol;
    let turned = symbol.0 * re + symbol.1 * im;
    branch.previous_symbol = symbol;
    if branch.bits.len() == info::INFO1C_BITS {
        branch.bits.pop_front();
    }
    branch.bits.push_back(turned < 0.0);

    let bits = branch.bits.make_contiguous();
    for length in side.lengths() {
        // Checked the moment the CRC is in: the trailing fill says nothing,
        // and whatever follows a sequence may not be ones at all.
        let without_fill = length - info::FILL.len();
        if bits.len() < without_fill {
            continue;
        }
        let candidate = &bits[bits.len() - without_fill..];
        let found = match (side, length) {
            (_, info::INFO0_BITS) => Info0::from_bits(candidate).map(Info::Info0),
            (Side::Call, _) => Info1c::from_bits(candidate).map(Info::Info1c),
            (Side::Answer, _) => Info1a::from_bits(candidate).map(Info::Info1a),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::info::{Probed, SymbolRate};
    use super::*;

    const FS: f64 = 16_000.0;

    fn capabilities() -> Info0 {
        Info0 {
            rate_2743: true,
            rate_2800: true,
            rate_3429: true,
            low_carrier_3000: true,
            high_carrier_3000: true,
            low_carrier_3200: true,
            high_carrier_3200: true,
            transmit_3429: true,
            can_reduce_power: true,
            asymmetry: 5,
            constellation_1664: true,
            ..Info0::default()
        }
    }

    fn results() -> Info1a {
        Info1a {
            min_power_reduction: 2,
            additional_power_reduction: 3,
            md_length: 0,
            probed: Probed { high_carrier: true, pre_emphasis: 4, max_rate: 14 },
            answer_to_call: SymbolRate::S3429,
            call_to_answer: SymbolRate::S3200,
            frequency_offset: Some(0.5),
        }
    }

    /// Run a transmitter into a receiver through `line`, and collect what the
    /// receiver found.
    fn through(side: Side, sequences: &[Vec<bool>], mut line: impl FnMut(usize, f64) -> f64) -> Vec<Info> {
        let mut tx = Transmitter::new(side, FS);
        let mut rx = Receiver::new(side, FS);
        for bits in sequences {
            tx.send(bits);
        }
        tx.silence();
        let mut found = Vec::new();
        let mut i = 0;
        // Some silence in front, so the receiver starts somewhere arbitrary.
        for _ in 0..777 {
            if let Some(info) = rx.feed(line(i, 0.0)) {
                found.push(info);
            }
            i += 1;
        }
        while tx.is_sending() {
            let sample = tx.next_sample();
            if let Some(info) = rx.feed(line(i, sample)) {
                found.push(info);
            }
            i += 1;
        }
        for _ in 0..(FS as usize) / 4 {
            if let Some(info) = rx.feed(line(i, 0.0)) {
                found.push(info);
            }
            i += 1;
        }
        found
    }

    #[test]
    fn both_sides_carry_both_their_sequences() {
        let info1c = Info1c { md_length: 12, ..Info1c::default() };
        let found = through(Side::Call, &[capabilities().to_bits(), info1c.to_bits()], |_, x| x);
        assert_eq!(found, vec![Info::Info0(capabilities()), Info::Info1c(info1c)]);
        let found = through(Side::Answer, &[capabilities().to_bits(), results().to_bits()], |_, x| x);
        assert_eq!(found, vec![Info::Info0(capabilities()), Info::Info1a(results())]);
    }

    #[test]
    fn a_sequence_carries_on_into_the_tone_after_it() {
        // INFO0 followed by tone B: the carrier stays up with no reversals,
        // and the receiver finds the sequence and nothing more.
        let mut tx = Transmitter::new(Side::Call, FS);
        let mut rx = Receiver::new(Side::Call, FS);
        tx.send(&capabilities().to_bits());
        let mut found = Vec::new();
        for _ in 0..(FS as usize) {
            if let Some(info) = rx.feed(tx.next_sample()) {
                found.push(info);
            }
        }
        assert_eq!(found, vec![Info::Info0(capabilities())]);
        assert!(tx.is_sending(), "the tone stopped");
    }

    #[test]
    fn a_reversal_turns_the_carrier_round_on_the_sample() {
        let mut tx = Transmitter::new(Side::Call, FS);
        let mut plain = Transmitter::new(Side::Call, FS);
        tx.send(&[false; 40]);
        plain.send(&[false; 40]);
        for _ in 0..4000 {
            tx.next_sample();
            plain.next_sample();
        }
        tx.reverse();
        for _ in 0..200 {
            let (a, b) = (tx.next_sample(), plain.next_sample());
            assert!((a + b).abs() < 1e-9, "{a} against {b}");
        }
        tx.stop();
        assert!(!tx.is_sending());
        assert_eq!(tx.next_sample(), 0.0);
    }

    #[test]
    fn both_directions_at_once_on_one_pair_are_each_heard() {
        // Phase 2 is duplex: the call modem's INFO0c and the answer modem's
        // INFO0a are on the line together, as they are on a two-wire
        // recording, and each receiver has to hear only its own.
        let mut call = Transmitter::new(Side::Call, FS);
        let mut answer = Transmitter::new(Side::Answer, FS);
        let mut hears_call = Receiver::new(Side::Call, FS);
        let mut hears_answer = Receiver::new(Side::Answer, FS);
        let info1c = Info1c { min_power_reduction: 5, ..Info1c::default() };
        call.send(&capabilities().to_bits());
        call.send(&info1c.to_bits());
        call.silence();
        // The answer modem a third of a symbol and some later, and louder.
        let mut answer_bits = capabilities().to_bits();
        answer_bits.extend(results().to_bits());
        let (mut from_call, mut from_answer) = (Vec::new(), Vec::new());
        for i in 0..(FS as usize) {
            if i == 131 {
                answer.send(&answer_bits);
                answer.silence();
            }
            let line = 0.5 * call.next_sample() + 1.4 * answer.next_sample();
            from_call.extend(hears_call.feed(line));
            from_answer.extend(hears_answer.feed(line));
        }
        assert_eq!(from_call, vec![Info::Info0(capabilities()), Info::Info1c(info1c)]);
        assert_eq!(from_answer, vec![Info::Info0(capabilities()), Info::Info1a(results())]);
    }

    #[test]
    fn a_quiet_noisy_line_still_carries_them() {
        // Forty decibels down with noise twenty decibels under the signal.
        let mut seed = 12345u32;
        let found = through(Side::Answer, &[capabilities().to_bits(), results().to_bits()], |_, x| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 2.0 * 0.1 * 0.707 * 1.73;
            (x + noise) * 0.01
        });
        assert_eq!(found, vec![Info::Info0(capabilities()), Info::Info1a(results())]);
    }

    #[test]
    fn the_spectrum_sits_inside_figure_13() {
        // Random bits for a long time, averaged into a power spectrum around
        // the call modem's carrier, which has no guard tone to get in the way.
        let mut tx = Transmitter::new(Side::Call, FS);
        let mut seed = 99u32;
        let n = 4096;
        let frames = 60;
        let mut power = vec![0.0f64; n / 2];
        let fft = dsp::Fft::new(n);
        // Settle first.
        let mut bits = Vec::new();
        for _ in 0..(frames * n) / 26 + 64 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            bits.push(seed & 1 == 1);
        }
        tx.send(&bits);
        for _ in 0..2000 {
            tx.next_sample();
        }
        let window: Vec<f64> = (0..n)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())
            .collect();
        for _ in 0..frames {
            let mut re: Vec<f64> = (0..n).map(|i| tx.next_sample() * window[i]).collect();
            let mut im = vec![0.0; n];
            fft.process(&mut re, &mut im);
            for k in 0..n / 2 {
                power[k] += re[k] * re[k] + im[k] * im[k];
            }
        }
        let hz = FS / n as f64;
        let at = |offset: f64| {
            // Averaged over a few bins, for a steadier reading.
            let centre = ((1200.0 + offset) / hz).round() as usize;
            (centre - 2..=centre + 2).map(|k| power[k]).sum::<f64>() / 5.0
        };
        let reference = at(0.0);
        let db = |offset: f64| 10.0 * (at(offset) / reference).log10();
        for side in [-1.0, 1.0] {
            let within = |offset: f64, low: f64, high: f64| {
                let v = db(side * offset);
                assert!((low..=high).contains(&v), "{} Hz: {v:.2} dB, not {low} to {high}", side * offset);
            };
            within(125.0, -0.75, 0.75);
            within(300.0, -4.0, -2.0);
            within(400.0, -9.0, -5.0);
            // Between the two templates' straight lines at 450 Hz.
            within(450.0, -9.0 - 11.0 * 50.0 / 75.0, -9.0);
            let far = db(side * 560.0);
            assert!(far < -20.0, "{} Hz: {far:.2} dB", side * 560.0);
        }
    }

    #[test]
    fn the_answer_side_leaves_at_the_nominal_level() {
        // 1 dB under for the carrier and 7 under for the guard tone come back
        // to within a quarter of a decibel of the nominal between them.
        for side in [Side::Call, Side::Answer] {
            let mut tx = Transmitter::new(side, FS);
            tx.send(&vec![false; 400]);
            let samples: Vec<f64> = (0..(FS as usize / 2)).map(|_| tx.next_sample()).collect();
            let steady = &samples[4000..];
            let rms = (steady.iter().map(|x| x * x).sum::<f64>() / steady.len() as f64).sqrt();
            let db = 20.0 * (rms / std::f64::consts::FRAC_1_SQRT_2).log10();
            assert!(db.abs() < 0.25, "{side:?}: {db:.2} dB from nominal");
        }
    }
}
