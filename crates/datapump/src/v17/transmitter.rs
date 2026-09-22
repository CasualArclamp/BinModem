//! The V.17 transmitter: a burst of clause 5's training, the page, and
//! Table 7's turn-off.
//!
//! The same pulse and the same line level as V.32's transmitter, because the
//! symbols are V.32bis's: a quarter-roll-off root raised cosine, which puts
//! 600 and 3000 Hz 3 dB down whatever else it does and so meets 2.4 by
//! construction, and a root mean square of 0.707 on the line for the four
//! training states, like every other transmitter in this modem.

use std::collections::VecDeque;

use dsp::{Nco, rrc_at};

use super::super::v32::trellis::Encoder;
use super::super::v32::{CONSTELLATION_RMS, ROLLOFF, Scrambler};
use super::{BAUD, BRIDGE_PATTERN, CARRIER, Conditioning, Rate, State, Training, bridge_turn, from_state, scrambler, train};

/// Symbols each side of centre in the shaping filter, as V.32's.
const SPAN: usize = 6;

/// Where a burst has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Silent,
    /// Talker-echo protection (5.3): the unmodulated carrier, then nothing.
    EchoCarrier(u64),
    EchoQuiet(u64),
    /// Segment 1, counting down.
    Alternations(u64),
    /// Segment 2.
    Conditioning(u64),
    /// Segment 3, long trains only.
    Bridge(u64),
    /// Segment 4.
    Ones(u64),
    Data,
    /// Table 7: whatever is still queued, then segment A's scrambled ones,
    /// then segment B's silence.
    TurnOff(u64),
    Quiet(u64),
}

/// V.17 transmitter.
#[derive(Debug)]
pub struct Transmitter {
    fs: f64,
    rate: Rate,
    training: Training,
    echo_protection: bool,
    nco: Nco,
    stage: Stage,
    /// Segment 2's generator, and then the scrambler it leaves, which the
    /// bridge, segment 4 and the data all carry on with.
    conditioning: Conditioning,
    scrambler: Scrambler,
    encoder: Encoder,
    /// The state last sent in segments 1 to 3, which the bridge turns from.
    state: State,
    /// The state segment 4's differential coding starts from (5.1.4), and
    /// whether its first group has gone.
    from: State,
    started: bool,
    /// Bits of the bridge sent.
    bridge_bit: usize,
    history: Vec<(f64, f64)>,
    phase: f64,
    pending: VecDeque<bool>,
    /// The symbol most recently put on the line, in `trellis` units.
    sent: (f64, f64),
}

impl Transmitter {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            rate: Rate::default(),
            training: Training::default(),
            echo_protection: false,
            nco: Nco::new(CARRIER, fs),
            stage: Stage::Silent,
            conditioning: Conditioning::new(),
            scrambler: scrambler(),
            encoder: Encoder::new(),
            state: State::A,
            from: State::A,
            started: false,
            bridge_bit: 0,
            history: vec![(0.0, 0.0); 2 * SPAN + 1],
            phase: 0.0,
            pending: VecDeque::new(),
            sent: (0.0, 0.0),
        }
    }

    /// Put 5.3's protection against talker echo in front of every burst:
    /// 192.5 ms of unmodulated carrier and 22.5 ms of nothing, which 5.3
    /// counts "as part of the training sequences". Off unless asked for.
    pub fn set_echo_protection(&mut self, on: bool) {
        self.echo_protection = on;
    }

    /// Raise the carrier and begin a burst at `rate`, with the training T.30
    /// calls for: [`Training::Long`] for a training check, a
    /// [`Training::Resync`] for a page that follows one.
    pub fn start(&mut self, rate: Rate, training: Training) {
        self.rate = rate;
        self.training = training;
        self.conditioning = Conditioning::new();
        self.scrambler = scrambler();
        // 5.1.4: "The convolutional encoder initial state shall be
        // initialized to zero."
        self.encoder = Encoder::new();
        self.state = State::A;
        self.started = false;
        self.bridge_bit = 0;
        self.stage = if self.echo_protection {
            Stage::EchoCarrier(train::ECHO_CARRIER)
        } else {
            Stage::Alternations(train::ALTERNATIONS)
        };
        self.phase = 0.0;
        self.history.fill((0.0, 0.0));
        self.pending.clear();
    }

    /// Finish the burst: whatever is queued, then Table 7's turn-off.
    ///
    /// A second call is not a second turn-off. Whoever is above cannot see
    /// symbol boundaries and asks on every sample until the carrier goes.
    pub fn stop(&mut self) {
        if !matches!(self.stage, Stage::Silent | Stage::TurnOff(_) | Stage::Quiet(_)) {
            self.stage = Stage::TurnOff(train::TURN_OFF_ONES);
        }
    }

    /// Drop the carrier now.
    pub fn abort(&mut self) {
        self.stage = Stage::Silent;
        self.pending.clear();
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    pub fn training(&self) -> Training {
        self.training
    }

    /// Whether anything is going out: the turn-off's silence counts, since
    /// Table 7 makes it part of the burst and a new one may not start in it
    /// (its Note).
    pub fn is_transmitting(&self) -> bool {
        self.stage != Stage::Silent
    }

    /// Whether the training is over and data is going out: 5.1.4's circuit
    /// 106, which turns on at the end of segment 4.
    pub fn trained(&self) -> bool {
        matches!(self.stage, Stage::Data | Stage::TurnOff(_) | Stage::Quiet(_))
    }

    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend(bits.iter().copied());
    }

    /// Push octets most significant bit first.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            for i in (0..8).rev() {
                self.pending.push_back(byte >> i & 1 != 0);
            }
        }
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }

    /// The point most recently sent, at the unit power a receiver reports
    /// points at, or `None` while nothing is going out.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        if !self.is_transmitting() || self.sent == (0.0, 0.0) {
            return None;
        }
        Some((self.sent.0 / CONSTELLATION_RMS, self.sent.1 / CONSTELLATION_RMS))
    }

    /// One symbol at the channel rate, 2.3: a group of scrambled bits, the
    /// first two differentially coded by Table 1 and all of it through the
    /// convolutional encoder onto the rate's constellation. `ones` takes
    /// binary ones rather than the queue: segment 4, and the turn-off.
    fn coded(&mut self, ones: bool) -> (f64, f64) {
        let coded = self.rate.coded();
        let mut group = [false; 6];
        for slot in group.iter_mut().take(coded.bits) {
            let bit = if ones { true } else { self.pending.pop_front().unwrap_or(true) };
            *slot = self.scrambler.scramble(bit);
        }
        if !self.started {
            // 5.1.4: the differential encoder starts from the first symbol
            // of segment 3 on a long train, the last of segment 2 on a
            // resync.
            from_state(&mut group, self.from);
            self.started = true;
        }
        let code = self.encoder.encode(&coded, &group);
        let (x, y) = coded.point(code);
        let lift = self.rate.lift();
        (x * lift, y * lift)
    }

    /// The next symbol, or `None` for one of no energy.
    fn next_symbol(&mut self) -> Option<(f64, f64)> {
        let state = match self.stage {
            Stage::Silent => return None,
            Stage::EchoCarrier(left) => {
                self.stage = if left > 1 { Stage::EchoCarrier(left - 1) } else { Stage::EchoQuiet(train::ECHO_QUIET) };
                // Unmodulated: the one state over and over is the carrier
                // alone.
                State::A
            }
            Stage::EchoQuiet(left) => {
                self.stage = if left > 1 { Stage::EchoQuiet(left - 1) } else { Stage::Alternations(train::ALTERNATIONS) };
                return None;
            }
            Stage::Alternations(left) => {
                self.stage = if left > 1 {
                    Stage::Alternations(left - 1)
                } else {
                    Stage::Conditioning(self.training.equalizer())
                };
                // Counting down from 256, so the first is even: A first.
                if (train::ALTERNATIONS - left).is_multiple_of(2) { State::A } else { State::B }
            }
            Stage::Conditioning(left) => {
                let state = self.conditioning.next_state();
                if left > 1 {
                    self.stage = Stage::Conditioning(left - 1);
                } else {
                    self.scrambler = std::mem::take(&mut self.conditioning).into_scrambler();
                    self.stage = match self.training {
                        Training::Long => Stage::Bridge(train::BRIDGE),
                        Training::Resync => {
                            self.from = state;
                            Stage::Ones(train::SCRAMBLED_ONES)
                        }
                    };
                }
                state
            }
            Stage::Bridge(left) => {
                // 5.1.3: Table 5's sixteen bits eight times, scrambled, two to
                // a symbol, each dibit a change of state by Table 6 from the
                // last state of segment 2.
                let mut dibit = 0u8;
                for _ in 0..2 {
                    let bit = BRIDGE_PATTERN[self.bridge_bit % BRIDGE_PATTERN.len()];
                    self.bridge_bit += 1;
                    dibit = dibit << 1 | u8::from(self.scrambler.scramble(bit));
                }
                let state = self.state.turned(bridge_turn(dibit));
                if left == train::BRIDGE {
                    // 5.1.4: "For the long train sequence, the differential
                    // encoder shall be initialized using the first symbol of
                    // segment 3." The first and not the last, as printed; a
                    // receiver joins the coding a few symbols in whichever it
                    // is, since segment 4 carries nothing but ones.
                    self.from = state;
                }
                self.stage = if left > 1 { Stage::Bridge(left - 1) } else { Stage::Ones(train::SCRAMBLED_ONES) };
                state
            }
            Stage::Ones(left) => {
                self.stage = if left > 1 { Stage::Ones(left - 1) } else { Stage::Data };
                return Some(self.coded(true));
            }
            Stage::Data => return Some(self.coded(false)),
            Stage::TurnOff(left) => {
                if !self.pending.is_empty() {
                    return Some(self.coded(false));
                }
                self.stage = if left > 1 { Stage::TurnOff(left - 1) } else { Stage::Quiet(train::TURN_OFF_QUIET) };
                return Some(self.coded(true));
            }
            Stage::Quiet(left) => {
                self.stage = if left > 1 { Stage::Quiet(left - 1) } else { Stage::Silent };
                return None;
            }
        };
        self.state = state;
        Some(state.point())
    }

    pub fn next_sample(&mut self) -> f64 {
        if self.stage == Stage::Silent {
            self.nco.step();
            self.sent = (0.0, 0.0);
            return 0.0;
        }
        self.phase += BAUD / self.fs;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.history.remove(0);
            let symbol = self.next_symbol().unwrap_or((0.0, 0.0));
            self.history.push(symbol);
            self.sent = symbol;
        }
        let centre = SPAN as f64;
        let mut baseband = (0.0, 0.0);
        for (i, &(re, im)) in self.history.iter().enumerate() {
            let tap = rrc_at(self.phase + centre - i as f64, ROLLOFF);
            baseband.0 += re * tap;
            baseband.1 += im * tap;
        }
        let (cos, sin) = self.nco.step();
        (baseband.0 * cos - baseband.1 * sin) / CONSTELLATION_RMS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    /// The states a transmitter sends for its first `n` symbols, read off
    /// the symbol it makes rather than the line.
    fn symbols(tx: &mut Transmitter, n: usize) -> Vec<(f64, f64)> {
        (0..n).map(|_| tx.next_symbol().unwrap_or((0.0, 0.0))).collect()
    }

    fn state_of(p: (f64, f64)) -> Option<State> {
        State::ALL.into_iter().find(|s| s.point() == p)
    }

    #[test]
    fn segment_two_opens_as_table_4_prints_it() {
        let mut tx = Transmitter::new(FS);
        tx.start(Rate::R14400, Training::Long);
        let sent = symbols(&mut tx, 256 + 16);
        let opening: Vec<Option<State>> = sent[256..].iter().map(|p| state_of(*p)).collect();
        use State::{B, C, D};
        let want = [C, D, C, D, C, D, C, D, C, D, C, D, B, D, B, D].map(Some);
        assert_eq!(opening, want);
    }

    #[test]
    fn segment_one_alternates_a_and_b_from_a() {
        let mut tx = Transmitter::new(FS);
        tx.start(Rate::R9600, Training::Resync);
        let sent = symbols(&mut tx, 256);
        for (i, p) in sent.iter().enumerate() {
            let want = if i % 2 == 0 { State::A } else { State::B };
            assert_eq!(state_of(*p), Some(want), "symbol {i}");
        }
    }

    #[test]
    fn each_training_has_the_segments_table_3_prints() {
        // Where each segment ends is where what goes out changes kind: A and
        // B alternating, any of A to D, and then points of the channel's own
        // constellation, which at 14 400 are never A to D.
        for (training, conditioning, bridge) in [(Training::Long, 2976, 64), (Training::Resync, 38, 0)] {
            let mut tx = Transmitter::new(FS);
            tx.start(Rate::R14400, training);
            let total = 256 + conditioning + bridge + 48;
            let sent = symbols(&mut tx, total + 1);
            let four = sent.iter().take_while(|p| state_of(**p).is_some()).count();
            assert_eq!(four, 256 + conditioning + bridge, "{training:?}");
            // Trained exactly at the end of segment 4.
            let mut tx = Transmitter::new(FS);
            tx.start(Rate::R14400, training);
            symbols(&mut tx, total - 1);
            assert!(!tx.trained(), "{training:?} trained a symbol early");
            symbols(&mut tx, 1);
            assert!(tx.trained(), "{training:?} not trained after {total}");
        }
    }

    #[test]
    fn the_bridge_is_table_5_through_the_scrambler_and_table_6() {
        // Undo it: the change of state from each symbol to the next is
        // Table 6's dibit, and descrambling those gives Table 5 eight times.
        let mut tx = Transmitter::new(FS);
        tx.start(Rate::R7200, Training::Long);
        let sent = symbols(&mut tx, 256 + 2976 + 64);
        let states: Vec<State> = sent[256 + 2975..].iter().map(|p| state_of(*p).expect("a state")).collect();
        let mut descrambler = crate::v32::Scrambler::new(crate::v32::Mode::Call);
        // The descrambler needs 23 bits of history: segment 2's own.
        let mut conditioning = Conditioning::new();
        for _ in 0..2976 {
            let d = conditioning.next_dibit();
            descrambler.descramble(d & 2 != 0);
            descrambler.descramble(d & 1 != 0);
        }
        let mut bits = Vec::new();
        for pair in states.windows(2) {
            let turn = (pair[1] as u8 + 4 - pair[0] as u8) % 4;
            let dibit = (0..4u8).find(|d| bridge_turn(*d) == turn).expect("Table 6 has every turn");
            bits.push(descrambler.descramble(dibit & 2 != 0));
            bits.push(descrambler.descramble(dibit & 1 != 0));
        }
        let want: Vec<bool> = BRIDGE_PATTERN.iter().copied().cycle().take(128).collect();
        assert_eq!(bits, want);
    }

    #[test]
    fn the_training_goes_out_at_the_level_of_every_other_transmitter() {
        let mut tx = Transmitter::new(FS);
        tx.start(Rate::R14400, Training::Long);
        let samples: Vec<f64> = (0..(FS * 1.2) as usize).map(|_| tx.next_sample()).collect();
        let rms = (samples[4000..].iter().map(|x| x * x).sum::<f64>() / (samples.len() - 4000) as f64).sqrt();
        assert!((rms - 0.707).abs() < 0.03, "segment 2 at {rms:.4} rms");
    }

    #[test]
    fn the_turn_off_is_table_7s() {
        let mut tx = Transmitter::new(FS);
        tx.start(Rate::R9600, Training::Resync);
        while !tx.trained() {
            tx.next_symbol();
        }
        tx.push_bytes(&[0x00; 3]);
        tx.stop();
        // 24 bits at four a symbol, then 32 of ones and 48 of nothing.
        let mut energy = 0;
        let mut quiet = 0;
        while tx.is_transmitting() {
            match tx.next_symbol() {
                Some(_) => energy += 1,
                None => quiet += 1,
            }
        }
        assert_eq!((energy, quiet), (6 + 32, 48));
    }

    #[test]
    fn echo_protection_is_a_plain_carrier_and_a_gap_in_front() {
        let mut tx = Transmitter::new(FS);
        tx.set_echo_protection(true);
        tx.start(Rate::R14400, Training::Long);
        let sent: Vec<Option<(f64, f64)>> = (0..462 + 54 + 2).map(|_| tx.next_symbol()).collect();
        assert!(sent[..462].iter().all(|p| *p == Some(State::A.point())));
        assert!(sent[462..516].iter().all(Option::is_none));
        assert_eq!(sent[516], Some(State::A.point()));
        assert_eq!(sent[517], Some(State::B.point()));
    }
}
