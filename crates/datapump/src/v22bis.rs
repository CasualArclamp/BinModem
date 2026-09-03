//! ITU-T V.22bis — 2400 bit/s duplex, 600 baud, 16-QAM.
//!
//! Full duplex by frequency division (V.22bis 2.1): the calling modem occupies
//! the low channel on a 1200 Hz carrier and the answering modem the high channel
//! on 2400 Hz. Because the two directions sit in different bands, no echo
//! canceller is needed — that requirement only arrives with V.32.
//!
//! Data is carried in quadbits. The first two bits are a *change* of phase
//! quadrant relative to the previous symbol (V.22bis Table 1), which makes the
//! link immune to a constant carrier phase offset that is a multiple of 90
//! degrees. The last two bits pick one of four points inside the new quadrant
//! (Figure 2).

use dsp::{ComplexFir, Equalizer, Gardner, Nco, bandpass, rrc_at, rrc_taps};
use dsp::filter::{Cascade, OnePole};

/// Modulation rate (V.22bis 2.5.1).
pub const BAUD: f64 = 600.0;
/// Low channel carrier, used by the calling modem (V.22bis 2.1).
pub const CARRIER_LOW: f64 = 1200.0;
/// High channel carrier, used by the answering modem.
pub const CARRIER_HIGH: f64 = 2400.0;
/// Root-raised-cosine roll-off (V.22bis 2.4).
pub const ROLLOFF: f64 = 0.75;
/// Pulse span in symbols.
const SPAN: usize = 8;

/// Which channel this modem transmits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Calling modem: transmits low, receives high.
    Calling,
    /// Answering modem: transmits high, receives low.
    Answering,
}

impl Channel {
    pub fn transmit_carrier(self) -> f64 {
        match self {
            Self::Calling => CARRIER_LOW,
            Self::Answering => CARRIER_HIGH,
        }
    }

    pub fn receive_carrier(self) -> f64 {
        match self {
            Self::Calling => CARRIER_HIGH,
            Self::Answering => CARRIER_LOW,
        }
    }
}

/// Quadrant change for the first two bits of a quadbit (V.22bis Table 1),
/// indexed by those bits as `Q1 << 1 | Q2`. Values are quadrants anticlockwise.
const QUADRANT_CHANGE: [u8; 4] = [1, 0, 2, 3];

/// The inverse: quadrant change back to the pair of bits that caused it.
const CHANGE_TO_BITS: [u8; 4] = [0b01, 0b00, 0b10, 0b11];

/// First-quadrant points selected by the last two bits, `Q3 << 1 | Q4`
/// (V.22bis Figure 2).
///
/// `01` sits at (3,1), whose magnitude is the root of ten. That is exactly the
/// root-mean-square magnitude of the whole sixteen-point constellation, which is
/// why V.22bis 2.5.2.2 nominates it as the point used at 1200 bit/s: the slower
/// signal then has the same average power as the faster one.
const QUADRANT_POINTS: [(f64, f64); 4] = [(1.0, 1.0), (3.0, 1.0), (1.0, 3.0), (3.0, 3.0)];

/// Root-mean-square magnitude of the constellation, used to scale the
/// transmitted signal to unit power.
pub const CONSTELLATION_RMS: f64 = 3.162_277_660_168_379_5; // sqrt(10)

/// Mean power of the sixteen points, which is the square of the above.
pub const CONSTELLATION_MEAN_POWER: f64 = 10.0;

/// Ceiling on receiver gain, so silence cannot be amplified into nonsense.
const MAX_GAIN: f64 = 400.0;

/// Symbol power below which the signal is treated as absent, relative to what
/// gain control is aiming at.
const SQUELCH: f64 = 1.0e-7;

/// Rotate a first-quadrant point into `quadrant` (0 to 3, anticlockwise).
fn rotate(point: (f64, f64), quadrant: u8) -> (f64, f64) {
    match quadrant & 3 {
        0 => point,
        1 => (-point.1, point.0),
        2 => (-point.0, -point.1),
        _ => (point.1, -point.0),
    }
}

/// Which quadrant a point lies in.
fn quadrant_of(point: (f64, f64)) -> u8 {
    match (point.0 >= 0.0, point.1 >= 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    }
}

/// The self-synchronising scrambler of V.22bis 5.1.
///
/// A single polynomial, 1 + x^-14 + x^-17, serves both directions. V.22 and
/// V.22bis differ from V.26ter and V.32 here, which use a different polynomial
/// at each end.
#[derive(Debug, Clone, Default)]
pub struct Scrambler {
    register: u32,
    /// Consecutive ones seen at the output.
    ones: u32,
}

impl Scrambler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scramble one bit. `Ds = Di + Ds(n-14) + Ds(n-17)`, modulo 2.
    pub fn scramble(&mut self, bit: bool) -> bool {
        // V.22bis 5.1: sixty-four consecutive ones at the output invert the
        // next input, which stops a lock-up being read as a remote loop request.
        let input = bit ^ (self.ones >= 64);
        if self.ones >= 64 {
            self.ones = 0;
        }
        let feedback = ((self.register >> 13) ^ (self.register >> 16)) & 1;
        let out = input ^ (feedback != 0);
        self.register = (self.register << 1) | u32::from(out);
        self.count(out);
        out
    }

    /// Descramble one bit. `Do = Ds + Ds(n-14) + Ds(n-17)`, modulo 2.
    pub fn descramble(&mut self, bit: bool) -> bool {
        let feedback = ((self.register >> 13) ^ (self.register >> 16)) & 1;
        let out = bit ^ (feedback != 0);
        self.register = (self.register << 1) | u32::from(bit);

        // The detector watches the scrambled stream, which is identical at both
        // ends. The counter must be reset *before* the current bit is counted,
        // exactly as the scrambler does: resetting afterwards discards this
        // bit's contribution at one end but not the other, and the two counters
        // then drift apart and invert at different places.
        let inverted = self.ones >= 64;
        if inverted {
            self.ones = 0;
        }
        self.count(bit);
        if inverted { !out } else { out }
    }

    fn count(&mut self, bit: bool) {
        if bit {
            self.ones += 1;
        } else {
            self.ones = 0;
        }
    }

    pub fn reset(&mut self) {
        self.register = 0;
        self.ones = 0;
    }
}

/// V.22bis transmitter.
///
/// The pulse is evaluated at arbitrary offsets rather than read from a tap
/// table, because the sample rate need not be a whole multiple of 600 baud.
#[derive(Debug)]
pub struct Transmitter {
    fs: f64,
    carrier: f64,
    nco: Nco,
    scrambler: Scrambler,
    quadrant: u8,
    /// Symbols still contributing to the pulse, oldest first.
    history: Vec<(f64, f64)>,
    /// Position within the current symbol period, in symbols.
    phase: f64,
    pending: Vec<bool>,
}

impl Transmitter {
    pub fn new(channel: Channel, fs: f64) -> Self {
        let carrier = channel.transmit_carrier();
        Self {
            fs,
            carrier,
            nco: Nco::new(carrier, fs),
            scrambler: Scrambler::new(),
            // V.22bis encodes a change of quadrant, so any starting quadrant
            // works as long as the receiver also tracks changes.
            quadrant: 0,
            history: vec![(0.0, 0.0); 2 * SPAN + 1],
            phase: 0.0,
            pending: Vec::new(),
        }
    }

    pub fn carrier(&self) -> f64 {
        self.carrier
    }

    /// Queue bits for transmission, most significant first within each byte.
    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend_from_slice(bits);
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            for i in (0..8).rev() {
                self.pending.push(b & (1 << i) != 0);
            }
        }
    }

    /// Map the next quadbit to a constellation point (V.22bis 2.5.2.1).
    fn next_symbol(&mut self) -> (f64, f64) {
        let mut quad = [false; 4];
        for slot in &mut quad {
            let bit = if self.pending.is_empty() {
                // Idle fills with ones, as the handshake does.
                true
            } else {
                self.pending.remove(0)
            };
            *slot = self.scrambler.scramble(bit);
        }
        let change = QUADRANT_CHANGE[usize::from(quad[0]) << 1 | usize::from(quad[1])];
        self.quadrant = (self.quadrant + change) & 3;
        let point = QUADRANT_POINTS[usize::from(quad[2]) << 1 | usize::from(quad[3])];
        rotate(point, self.quadrant)
    }

    /// Produce one line sample.
    pub fn next_sample(&mut self) -> f64 {
        // Advance the symbol clock, pulling a new symbol when it wraps.
        self.phase += BAUD / self.fs;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol();
            self.history.push(symbol);
        }

        // Sum the shaped contributions of every symbol still in range.
        //
        // Output runs SPAN symbols behind the newest symbol so the pulse can be
        // centred on data that has already arrived. The offset must *grow* with
        // the phase within a symbol: when the phase wraps and the history
        // shifts, the two changes cancel and each symbol's offset advances
        // smoothly. Subtracting the phase instead makes it jump by two symbol
        // periods at every boundary, which puts a step in the waveform and
        // splatters energy right across the neighbouring channel.
        let centre = SPAN as f64;
        let mut baseband = (0.0, 0.0);
        for (i, &(re, im)) in self.history.iter().enumerate() {
            let offset = self.phase + centre - i as f64;
            let tap = rrc_at(offset, ROLLOFF);
            baseband.0 += re * tap;
            baseband.1 += im * tap;
        }

        // Up-convert: the real part of the baseband times the carrier phasor.
        let (cos, sin) = self.nco.step();
        (baseband.0 * cos - baseband.1 * sin) / CONSTELLATION_RMS
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }
}

/// V.22bis receiver.
#[derive(Debug)]
pub struct Receiver {
    band: Cascade,
    nco: Nco,
    matched: ComplexFir,
    gardner: Gardner,
    /// Samples until the next timing instant.
    countdown: f64,
    /// Previous matched-filter output, for interpolating between samples.
    previous_filtered: (f64, f64),
    /// Carrier phase correction, in turns.
    phase: f64,
    frequency: f64,
    agc: OnePole,
    equalizer: Equalizer,
    /// Symbols seen, so adaptation can wait for the other loops.
    symbols: u64,
    quadrant: Option<u8>,
    descrambler: Scrambler,
    bits: Vec<bool>,
    last_symbol: (f64, f64),
    last_error: f64,
    level: OnePole,
}

impl Receiver {
    pub fn new(channel: Channel, fs: f64) -> Self {
        let carrier = channel.receive_carrier();
        let sps = fs / BAUD;
        // The signal occupies the carrier plus half the symbol rate scaled by
        // the roll-off, so a little over 500 Hz either side.
        // Wide and gentle. A steeper filter tightened around the channel was
        // tried and made things worse in both directions: its group delay
        // distorts the pulse more than the adjacent channel it removes costs.
        // Selectivity has to come from the matched filter, or from cancelling
        // our own transmitter, rather than from brute filtering here.
        let half = BAUD * (1.0 + ROLLOFF) / 2.0 + 260.0;
        Self {
            band: bandpass(2, (carrier - half).max(120.0), carrier + half, fs),
            nco: Nco::new(carrier, fs),
            matched: ComplexFir::new(rrc_taps(sps, ROLLOFF, SPAN)),
            // A gentle timing loop: there is nothing to chase in a matched
            // pair of clocks, and a slow loop rides out amplitude noise.
            gardner: Gardner::new(sps, 0.005),
            countdown: sps / 2.0,
            previous_filtered: (0.0, 0.0),
            phase: 0.0,
            frequency: 0.0,
            // Started at the target so the first symbols do not see a
            // division by nearly zero.
            agc: OnePole::starting_at(CONSTELLATION_MEAN_POWER, 0.050, fs / sps),
            // Fed unit-power symbols, so the textbook constant-modulus target
            // applies unchanged. Its gradient goes as the cube of the
            // magnitude, so handing it the raw scale where mean power is ten
            // would make every update a thousand times too large.
            equalizer: Equalizer::new(21, 1.32),
            symbols: 0,
            quadrant: None,
            descrambler: Scrambler::new(),
            bits: Vec::new(),
            last_symbol: (0.0, 0.0),
            last_error: 0.0,
            level: OnePole::new(0.020, fs),
        }
    }

    /// Feed one line sample. Recovered bits accumulate; drain with `take_bits`.
    pub fn feed(&mut self, sample: f64) {
        let x = self.band.process(sample);
        self.level.process(x.abs());

        // Down-convert to baseband by the conjugate carrier, then filter with
        // the matched root-raised-cosine and nothing else.
        //
        // No extra low-pass: a matched pair of root-raised-cosine filters is
        // free of intersymbol interference only if nothing further shapes the
        // pulse, and a Butterworth in the same path destroys exactly that
        // property. The matched filter already rejects both the image at twice
        // the carrier and the neighbouring channel.
        let (cos, sin) = self.nco.step();
        let filtered = self.matched.process((x * cos, x * -sin));

        let previous = std::mem::replace(&mut self.previous_filtered, filtered);
        let before = self.countdown;
        self.countdown -= 1.0;
        if self.countdown > 0.0 {
            return;
        }
        // The wanted instant almost never lands on a sample: 16 kHz against
        // 600 baud is 26.67 samples per symbol. Taking the nearest sample would
        // mistime every symbol by up to half a sample, which shows up as
        // occasional symbol errors rather than an obvious failure. Interpolate
        // to where the instant actually falls, `before` samples past the
        // previous one.
        let mu = before.clamp(0.0, 1.0);
        let at = (
            previous.0 + mu * (filtered.0 - previous.0),
            previous.1 + mu * (filtered.1 - previous.1),
        );
        self.countdown += self.gardner.interval();
        let Some(symbol) = self.gardner.feed(at) else { return };
        self.on_symbol(symbol);
    }

    fn on_symbol(&mut self, symbol: (f64, f64)) {
        // Automatic gain control on mean power, not mean magnitude: for this
        // constellation those differ by five per cent, since the mean of the
        // sixteen magnitudes is 2.995 while their root-mean-square is 3.162.
        // Matching power to power keeps the decision boundaries where the
        // slicer expects them.
        let power = symbol.0 * symbol.0 + symbol.1 * symbol.1;
        let mean_power = self.agc.process(power);
        // Bound the gain. Between calls a capture contains answer tones,
        // silence before the carrier and silence after the hangup, and during
        // those the mean power falls towards zero. An unbounded gain then sends
        // the symbol to infinity, which the equaliser turns into NaN within a
        // few symbols and never recovers from.
        let gain = (CONSTELLATION_MEAN_POWER / mean_power.max(1e-9))
            .sqrt()
            .clamp(0.0, MAX_GAIN);

        // Rotate by the tracked carrier phase.
        let turn = self.phase * std::f64::consts::TAU;
        let (c, s) = (turn.cos(), turn.sin());
        let point = (
            (symbol.0 * c - symbol.1 * s) * gain,
            (symbol.0 * s + symbol.1 * c) * gain,
        );
        // The carrier loop works on the unequalised symbol. Putting it after
        // the equaliser would add that filter's delay inside the loop, and a
        // loop with ten symbols of delay in it will not stay stable.
        let coarse = nearest_point(point);
        // Decision-directed phase error: the angle between what arrived and
        // what it should have been.
        let error = (point.1 * coarse.0 - point.0 * coarse.1)
            / (coarse.0 * coarse.0 + coarse.1 * coarse.1 + 1e-9);
        self.last_error = error;
        // A second-order loop, so a residual frequency offset is also removed.
        // V.22bis 2.6 requires tolerating up to seven hertz.
        self.frequency += -1.5e-5 * error;
        self.frequency = self.frequency.clamp(-0.02, 0.02);
        self.phase += -0.008 * error + self.frequency;
        self.phase -= self.phase.floor();

        // Equalise, then slice. A real line smears the constellation far
        // beyond what a sixteen-point decision can survive.
        let normalized = (point.0 / CONSTELLATION_RMS, point.1 / CONSTELLATION_RMS);
        let equalized = self.equalizer.equalize(normalized);
        let scaled = (
            equalized.0 * CONSTELLATION_RMS,
            equalized.1 * CONSTELLATION_RMS,
        );
        let decision = nearest_point(scaled);
        // Hold the equaliser still until gain control and the carrier loop have
        // settled. Adapting against the acquisition transient teaches it
        // nonsense that it then has to unlearn.
        self.symbols += 1;
        // Nothing worth learning from silence or from a steady answer tone,
        // and plenty to unlearn afterwards.
        if self.symbols > 64 && mean_power > SQUELCH {
            self.equalizer.adapt(
                equalized,
                (
                    decision.0 / CONSTELLATION_RMS,
                    decision.1 / CONSTELLATION_RMS,
                ),
            );
        }
        self.last_symbol = scaled;

        let quadrant = quadrant_of(decision);
        let Some(previous) = self.quadrant.replace(quadrant) else {
            // The first symbol only establishes a reference; a change needs two.
            return;
        };
        let change = (quadrant + 4 - previous) & 3;
        let leading = CHANGE_TO_BITS[change as usize];
        let trailing = point_bits(decision, quadrant);

        for bit in [
            leading & 0b10 != 0,
            leading & 0b01 != 0,
            trailing & 0b10 != 0,
            trailing & 0b01 != 0,
        ] {
            let out = self.descrambler.descramble(bit);
            self.bits.push(out);
        }
    }

    /// Take the bits recovered so far.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// Take whole octets, most significant bit first, leaving any remainder.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        let whole = self.bits.len() / 8;
        let bits: Vec<bool> = self.bits.drain(..whole * 8).collect();
        bits.as_chunks::<8>().0.iter()
            .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)))
            .collect()
    }

    /// The most recent equalised symbol, for a constellation display.
    pub fn constellation_point(&self) -> (f64, f64) {
        (
            self.last_symbol.0 / CONSTELLATION_RMS,
            self.last_symbol.1 / CONSTELLATION_RMS,
        )
    }

    /// Residual carrier phase error, as a measure of lock quality.
    pub fn phase_error(&self) -> f64 {
        self.last_error
    }

    /// Mean distance between equalised symbols and their decisions. Small means
    /// a clean, well-equalised constellation.
    pub fn residual_error(&self) -> f64 {
        self.equalizer.error()
    }

    /// True while the equaliser is still adapting blind.
    pub fn equalizer_blind(&self) -> bool {
        self.equalizer.is_blind()
    }

    pub fn level(&self) -> f64 {
        self.level.value()
    }

    /// Carrier loop state, for diagnostics.
    #[doc(hidden)]
    pub fn loop_state(&self) -> (f64, f64, f64) {
        (self.phase, self.frequency, self.gardner.error())
    }
}

/// The constellation point nearest `p`.
fn nearest_point(p: (f64, f64)) -> (f64, f64) {
    // The sixteen points are the odd coordinates from -3 to 3, so rounding to
    // the nearest odd value in each axis finds the closest without a search.
    let snap = |v: f64| {
        let odd = ((v - 1.0) / 2.0).round() * 2.0 + 1.0;
        odd.clamp(-3.0, 3.0)
    };
    (snap(p.0), snap(p.1))
}

/// Recover the last two bits of a quadbit from a decided point.
fn point_bits(point: (f64, f64), quadrant: u8) -> u8 {
    // Rotate back to the first quadrant, then match against the four points.
    let base = rotate(point, (4 - quadrant) & 3);
    let mut best = 0usize;
    let mut best_distance = f64::MAX;
    for (i, &(x, y)) in QUADRANT_POINTS.iter().enumerate() {
        let d = (base.0 - x).powi(2) + (base.1 - y).powi(2);
        if d < best_distance {
            best_distance = d;
            best = i;
        }
    }
    best as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channels_face_each_other() {
        assert_eq!(Channel::Calling.transmit_carrier(), CARRIER_LOW);
        assert_eq!(Channel::Calling.receive_carrier(), CARRIER_HIGH);
        assert_eq!(Channel::Answering.transmit_carrier(), CARRIER_HIGH);
        assert_eq!(Channel::Answering.receive_carrier(), CARRIER_LOW);
    }

    #[test]
    fn the_quadrant_changes_match_table_1() {
        // 00 turns by one quadrant, 01 stays, 10 turns by two, 11 by three.
        assert_eq!(QUADRANT_CHANGE[0b00], 1);
        assert_eq!(QUADRANT_CHANGE[0b01], 0);
        assert_eq!(QUADRANT_CHANGE[0b10], 2);
        assert_eq!(QUADRANT_CHANGE[0b11], 3);
        // And the inverse agrees.
        for (bits, &change) in QUADRANT_CHANGE.iter().enumerate() {
            assert_eq!(CHANGE_TO_BITS[change as usize], bits as u8);
        }
    }

    #[test]
    fn the_constellation_matches_figure_2() {
        assert_eq!(QUADRANT_POINTS[0b00], (1.0, 1.0));
        assert_eq!(QUADRANT_POINTS[0b01], (3.0, 1.0));
        assert_eq!(QUADRANT_POINTS[0b10], (1.0, 3.0));
        assert_eq!(QUADRANT_POINTS[0b11], (3.0, 3.0));
    }

    #[test]
    fn the_v22_compatibility_point_carries_the_average_power() {
        // V.22bis 2.5.2.2 nominates 01 for 1200 bit/s. Its magnitude is the
        // root-mean-square of the whole constellation, so the slower signal has
        // the same average power as the faster one.
        let (x, y) = QUADRANT_POINTS[0b01];
        let magnitude = (x * x + y * y).sqrt();
        let mut sum = 0.0;
        for q in 0..4u8 {
            for p in QUADRANT_POINTS {
                let r = rotate(p, q);
                sum += r.0 * r.0 + r.1 * r.1;
            }
        }
        let rms = (sum / 16.0).sqrt();
        assert!((magnitude - rms).abs() < 1e-12, "{magnitude} against {rms}");
        assert!((rms - CONSTELLATION_RMS).abs() < 1e-12);
    }

    #[test]
    fn rotation_walks_the_quadrants() {
        let p = (1.0, 3.0);
        assert_eq!(rotate(p, 0), (1.0, 3.0));
        assert_eq!(rotate(p, 1), (-3.0, 1.0));
        assert_eq!(rotate(p, 2), (-1.0, -3.0));
        assert_eq!(rotate(p, 3), (3.0, -1.0));
        assert_eq!(quadrant_of(rotate(p, 1)), 1);
        assert_eq!(quadrant_of(rotate(p, 3)), 3);
    }

    #[test]
    fn every_constellation_point_decodes_to_its_own_bits() {
        for quadrant in 0..4u8 {
            for (bits, &base) in QUADRANT_POINTS.iter().enumerate() {
                let point = rotate(base, quadrant);
                assert_eq!(quadrant_of(point), quadrant);
                assert_eq!(point_bits(point, quadrant), bits as u8);
            }
        }
    }

    #[test]
    fn slicing_snaps_to_the_nearest_point() {
        assert_eq!(nearest_point((0.9, 1.1)), (1.0, 1.0));
        assert_eq!(nearest_point((2.7, -3.4)), (3.0, -3.0));
        assert_eq!(nearest_point((-1.2, 2.6)), (-1.0, 3.0));
        // Beyond the constellation, clamp rather than run away.
        assert_eq!(nearest_point((9.0, -9.0)), (3.0, -3.0));
    }

    #[test]
    fn the_scrambler_is_self_inverse() {
        let mut tx = Scrambler::new();
        let mut rx = Scrambler::new();
        let input: Vec<bool> = (0..2000).map(|i| (i * 37 + 11) % 5 < 2).collect();
        let recovered: Vec<bool> = input
            .iter()
            .map(|&b| rx.descramble(tx.scramble(b)))
            .collect();
        assert_eq!(recovered, input);
    }

    #[test]
    fn the_scrambler_breaks_up_a_constant_input() {
        // Its purpose: a run of identical bits must not become a line spectrum.
        let mut s = Scrambler::new();
        let out: Vec<bool> = (0..4000).map(|_| s.scramble(true)).collect();
        let ones = out.iter().filter(|b| **b).count();
        let ratio = ones as f64 / out.len() as f64;
        assert!(
            (0.4..0.6).contains(&ratio),
            "all-ones input produced {ratio} ones, which is not scrambled"
        );
    }

    #[test]
    fn the_scrambler_recovers_from_a_lock_up() {
        // V.22bis 5.1: sixty-four ones at the output invert the next input, so
        // the pathological all-ones state cannot persist.
        let mut s = Scrambler::new();
        s.register = 0;
        let mut longest = 0;
        let mut run = 0;
        for _ in 0..20_000 {
            if s.scramble(false) {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        assert!(longest <= 64, "a run of {longest} ones escaped the detector");
    }
}
