//! The V.32 start-up procedure (clause 5.4).
//!
//! Two modems that have never met have to agree a data rate, train an
//! equaliser at each end and an echo canceller at each end, and measure how
//! long the line takes to carry a signal there and back. None of that can use
//! a demodulator, because the demodulator is one of the things being set up,
//! so the whole exchange is conducted in fixed patterns of constellation
//! states that can be recognised as waveforms.
//!
//! The round trip is measured directly, and rather elegantly. Each modem
//! reverses the phase of what it is sending at a moment of its own choosing,
//! and starts a clock; the far end, on hearing that reversal, reverses its own
//! transmission exactly 64 symbols later; the first modem stops its clock when
//! that comes back. What is left after taking off the 64 is the time the line
//! adds, which is what the echo canceller needs to know how far back to look.

use super::{Mode, Receiver, Signal, Transmitter};
use dsp::{ReversalDetector, ToneDetector};

/// Half the symbol rate: where an alternating pattern puts its sidebands.
const OFFSET: f64 = super::BAUD / 2.0;

/// Level below which the line is carrying nothing.
const QUIET: f64 = 0.02;

/// How far a spectral line must stand above the average level of the whole
/// signal before it counts as standing there.
///
/// Measured against each signal in turn, as a ratio to the mean of the
/// rectified waveform: a bare carrier reads 1.60, the sidebands of an
/// alternation 1.28, and the conditioning signal 1.06 at the carrier with 0.68
/// at its sidebands. Everything scrambled reads 0.33 or less at every line,
/// which is the detector collecting its own bandwidth's worth of a signal
/// spread across the band rather than finding anything there. The two
/// thresholds sit in the gaps.
const STANDING: f64 = 0.7;
const STANDING_SIDEBAND: f64 = 0.5;

/// What the line is carrying, as far as the start-up needs to know.
///
/// The distinctions are all between waveforms rather than between messages,
/// which is what makes them available before anything has been demodulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// Not enough signal to say anything.
    Nothing,
    /// The V.25 answering tone at 2100 Hz (5.1).
    AnswerTone,
    /// A repeated state: the bare carrier. AA or CC in Figure 4.
    Carrier,
    /// An alternation between opposite states: sidebands at 600 and 3000 with
    /// the carrier suppressed. AC or CA.
    Alternation,
    /// An alternation between states a quarter turn apart: the same sidebands
    /// but with the carrier still standing. The conditioning signal of 5.2.
    Conditioning,
    /// Energy across the band and no line standing out: TRN, a rate signal, or
    /// data. All three are scrambled, which is what makes them look alike here
    /// and why telling them apart is the demodulator's job rather than this.
    Spread,
}

/// Recognises the start-up signals from the shape of the spectrum.
#[derive(Debug)]
pub struct Listener {
    answer: ToneDetector,
    carrier: ToneDetector,
    low: ToneDetector,
    high: ToneDetector,
    /// Total power on the line, to tell a spread signal from silence.
    power: dsp::filter::OnePole,
}

impl Listener {
    pub fn new(fs: f64) -> Self {
        // Narrow enough to separate lines 1200 Hz apart with room to spare,
        // wide enough to answer within a few tens of symbols.
        const BANDWIDTH: f64 = 60.0;
        Self {
            answer: ToneDetector::new(super::ANSWER_TONE, BANDWIDTH, fs),
            carrier: ToneDetector::new(super::CARRIER, BANDWIDTH, fs),
            low: ToneDetector::new(super::CARRIER - OFFSET, BANDWIDTH, fs),
            high: ToneDetector::new(super::CARRIER + OFFSET, BANDWIDTH, fs),
            power: dsp::filter::OnePole::new(0.020, fs),
        }
    }

    pub fn feed(&mut self, x: f64) {
        self.answer.feed(x);
        self.carrier.feed(x);
        self.low.feed(x);
        self.high.feed(x);
        self.power.process(x.abs());
    }

    /// Amplitude of the bare carrier, which 5.4.2 watches for a drop in.
    pub fn carrier_amplitude(&self) -> f64 {
        self.carrier.amplitude()
    }

    pub fn level(&self) -> f64 {
        self.power.value()
    }

    pub fn classify(&self) -> Heard {
        let level = self.power.value();
        if level < QUIET {
            return Heard::Nothing;
        }
        // Everything is judged against the level of the whole signal rather
        // than against an absolute, so that a quiet line and a loud one are
        // read alike and no threshold has to be told what the line is scaled
        // to.
        let answer = self.answer.amplitude() / level;
        let carrier = self.carrier.amplitude() / level;
        let sidebands = self.low.amplitude().min(self.high.amplitude()) / level;

        // The answering tone is nowhere near the others, so it is decided on
        // its own and first.
        if answer > STANDING {
            return Heard::AnswerTone;
        }
        // A pattern repeating every two symbols puts a line at each sideband
        // whether or not its two states are opposite; what separates the cases
        // is the carrier. An opposite pair averages to nothing and leaves
        // none; an unequal pair leaves it standing above them.
        match (
            carrier > STANDING,
            sidebands > STANDING_SIDEBAND,
        ) {
            (true, true) => Heard::Conditioning,
            (true, false) => Heard::Carrier,
            (false, true) => Heard::Alternation,
            (false, false) => Heard::Spread,
        }
    }
}

/// Which end of the call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Calling,
    Answering,
}

impl Role {
    pub fn mode(self) -> Mode {
        match self {
            Self::Calling => Mode::Call,
            Self::Answering => Mode::Answer,
        }
    }
}

/// Make a transmitter and receiver for one end of a V.32 call.
pub fn endpoints(role: Role, fs: f64) -> (Transmitter, Receiver) {
    let mode = role.mode();
    (Transmitter::new(mode, fs), Receiver::new(mode, fs))
}

/// Signal that a modem sends to say what it can do (Table 6).
///
/// Only the bits this implementation has any use for are set. B0 to B3 are
/// zero and B7, B11 and B15 are one, which is what a receiver synchronises on
/// and what separates a rate signal from the E that ends it.
pub fn rate_signal(bits_4800: bool, bits_9600: bool) -> u16 {
    let mut s = 0u16;
    let mut set = |bit: u32| s |= 1 << (15 - bit);
    set(7);
    set(11);
    set(15);
    if bits_4800 {
        set(5);
    }
    if bits_9600 {
        set(6);
    }
    // B9-14 are 0 0 1 0 0 0 for the absence of special modes, and B11 above is
    // the one of those that is set.
    s
}

/// The E sequence that ends a rate exchange (Table 7).
///
/// The same as a rate signal except that B0 to B3 are ones, which is the only
/// thing distinguishing the two.
pub fn end_signal(rate: u16) -> u16 {
    rate | 0xf000
}

/// True if `s` has the synchronising bits a rate signal must have (5.3.1).
pub fn is_rate_signal(s: u16) -> bool {
    s & 0xf000 == 0 && s & (1 << 8) != 0 && s & (1 << 4) != 0 && s & 1 != 0
}

/// True if `s` is an E sequence rather than a rate signal.
pub fn is_end_signal(s: u16) -> bool {
    s & 0xf000 == 0xf000 && s & (1 << 8) != 0 && s & (1 << 4) != 0 && s & 1 != 0
}

/// The highest data rate a rate signal offers, in bits per second.
///
/// Zero calls for the connection to be cleared down, which Table 6 spells as
/// B4 to B6 all zero.
pub fn offered_rate(s: u16) -> u32 {
    let bit = |b: u32| s & (1 << (15 - b)) != 0;
    if bit(6) {
        9600
    } else if bit(5) {
        4800
    } else if bit(4) {
        2400
    } else {
        0
    }
}

/// Finds the repeated 16-bit sequences a rate signal is made of (5.3.1).
///
/// The stream carries no framing, so the sequence has to be found in it: the
/// requirement is two consecutive identical sixteens whose synchronising bits
/// are in the right places, which is enough that data is very unlikely to
/// imitate one by accident.
#[derive(Debug, Default)]
pub struct RateDetector {
    /// The last thirty-two bits seen, newest at the bottom.
    window: u32,
    filled: u32,
}

impl RateDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer one received bit. Yields a sequence once two identical ones have
    /// arrived back to back.
    pub fn feed(&mut self, bit: bool) -> Option<u16> {
        self.window = (self.window << 1) | u32::from(bit);
        self.filled = (self.filled + 1).min(32);
        if self.filled < 32 {
            return None;
        }
        let first = (self.window >> 16) as u16;
        let second = self.window as u16;
        if first != second {
            return None;
        }
        (is_rate_signal(first) || is_end_signal(first)).then_some(first)
    }

    pub fn reset(&mut self) {
        self.window = 0;
        self.filled = 0;
    }
}

/// How far the start-up has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Negotiating,
    /// Agreed, at this many bits per second.
    Connected(u32),
    /// The far end called for the connection to be cleared, or nothing
    /// recognisable arrived in time.
    Failed,
}

/// Durations from clause 5, in symbol intervals.
mod timing {
    /// The answering tone, V.25 2.2: 3.3 s, which at 2400 baud is this many.
    pub const ANSWER_TONE: u64 = 7920;
    /// How long the calling modem must hear the answering tone before joining
    /// in (5.4.1). Note 1 there lets it start on the tones alone, which is
    /// what makes this a minimum rather than a wait.
    pub const HEARD_ANSWER_TONE: u64 = 2400;
    /// The response delay both ends owe each other, 5.4.1 and 5.4.2: "64 plus
    /// or minus 2 symbol periods".
    pub const RESPONSE: u64 = 64;
    /// Alternating states before the answering modem may move on (5.4.2).
    pub const MIN_ALTERNATION: u64 = 128;
    /// The incoming carrier must be heard this long first (5.4.2).
    pub const HEARD_CARRIER: u64 = 64;
    /// Silence after the amplitude drop (5.4.2).
    pub const GAP: u64 = 16;
    /// Segment 1 of the conditioning signal (5.2.1).
    pub const SEGMENT_S: u64 = 256;
    /// Segment 2 (5.2.2).
    pub const SEGMENT_S_BAR: u64 = 16;
    /// Segment 3, at its shortest (5.2.3 gives 1280 to 8192).
    pub const SEGMENT_TRN: u64 = 1280;
    /// Scrambled ones before data may flow (5.4.1 e, 5.4.2).
    pub const SETTLE: u64 = 128;
    /// Nothing recognisable for this long and the attempt is abandoned. The
    /// recommendation sets no overall limit; a modem that waits for ever is no
    /// use to whatever is waiting on it.
    pub const PATIENCE: u64 = 2400 * 60;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Calling: silent, waiting for something to answer (5.4.1).
    Listening,
    /// Calling: repeating state A, waiting for a first reversal in the tones.
    Aa,
    /// Calling: the 64 symbols owed between hearing a reversal and answering.
    AaToCc,
    /// Calling: repeating state C, waiting for the second reversal.
    Cc,
    /// Calling: silent, waiting for the conditioning signal and then R1.
    AwaitingR1,
    /// Calling: the extra S of 5.4.1, sent for the measured round trip.
    PreRoll,

    /// Answering: sending the V.25 answering tone (5.1).
    AnswerTone,
    /// Answering: alternating A and C, waiting to hear the calling modem.
    Ac,
    /// Answering: alternating the other way round, waiting for the reversal.
    Ca,
    /// Answering: the 64 symbols owed before turning back again.
    CaToAc,
    /// Answering: alternating again, waiting for the far end to stop.
    AcAgain,
    /// Answering: the 16 symbols of silence after the drop.
    Gap,
    /// Answering: silent again after R1, waiting out the measured round trip.
    AfterR1,
    /// Answering: waiting for the far end's conditioning signal, then R2.
    AwaitingR2,

    /// Both: the conditioning signal, in its three segments.
    SendS,
    SendSBar,
    SendTrn,
    /// Both: the rate signal, until the far end answers with its own.
    SendRate,
    /// Both: the single E that ends the exchange.
    SendEnd,
    /// Both: scrambled ones while the far end settles.
    Settling,
    Connected(u32),
    Failed,
}

/// Runs one end of the V.32 start-up.
///
/// Stepped one sample at a time, like everything else here. It owns the
/// listening: the sample handed in goes to its own tone detectors and to the
/// receiver, so the caller should not also feed the receiver.
#[derive(Debug)]
pub struct Startup {
    role: Role,
    state: State,
    listener: Listener,
    /// Reversals in the bare carrier, which the answering modem watches.
    carrier_reversals: ReversalDetector,
    /// Reversals in each sideband, which the calling modem watches. Both
    /// belong to the one signal and turn over together, so a reversal seen on
    /// either counts once and the other is ignored for a while afterwards.
    low_reversals: ReversalDetector,
    high_reversals: ReversalDetector,
    reversal_quiet: u64,
    rates: RateDetector,
    /// Samples until the next symbol boundary.
    countdown: f64,
    sps: f64,
    /// Symbols in the current state, and since the start.
    symbols: u64,
    total: u64,
    /// Symbols since the round-trip timer was started, and its final value.
    timer: Option<u64>,
    round_trip: u64,
    /// Consecutive symbols the condition being waited for has held.
    held: u64,
    /// Amplitude of the incoming carrier while it was up, for spotting a drop.
    carrier_peak: f64,
    /// Held until the next symbol boundary, where the machine can act on it.
    pending_reversal: bool,
    pending_sequence: Option<u16>,
    /// What this modem offers, and what has been settled on.
    offer: u16,
    agreed: u32,
}

impl Startup {
    /// `offer` is the rate signal this modem sends, from [`rate_signal`].
    pub fn new(role: Role, offer: u16, fs: f64) -> Self {
        let sps = fs / super::BAUD;
        Self {
            role,
            state: match role {
                Role::Calling => State::Listening,
                Role::Answering => State::AnswerTone,
            },
            listener: Listener::new(fs),
            carrier_reversals: ReversalDetector::new(super::CARRIER, 60.0, 0.05, fs),
            low_reversals: ReversalDetector::new(super::CARRIER - OFFSET, 60.0, 0.05, fs),
            high_reversals: ReversalDetector::new(super::CARRIER + OFFSET, 60.0, 0.05, fs),
            reversal_quiet: 0,
            rates: RateDetector::new(),
            countdown: sps,
            sps,
            symbols: 0,
            total: 0,
            timer: None,
            round_trip: 0,
            held: 0,
            carrier_peak: 0.0,
            pending_reversal: false,
            pending_sequence: None,
            offer,
            agreed: 0,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn status(&self) -> Status {
        match self.state {
            State::Connected(rate) => Status::Connected(rate),
            State::Failed => Status::Failed,
            _ => Status::Negotiating,
        }
    }

    /// The round trip, in symbol intervals, once it has been measured.
    ///
    /// This is what an echo canceller needs in order to know how far back the
    /// line's reflection of our own signal can be.
    pub fn round_trip(&self) -> u64 {
        self.round_trip
    }

    /// Which step of the procedure this end is on, for diagnostics and for
    /// anything that wants to show progress.
    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Listening => "listening",
            State::Aa => "AA",
            State::AaToCc => "AA to CC",
            State::Cc => "CC",
            State::AwaitingR1 => "awaiting R1",
            State::PreRoll => "S pre-roll",
            State::AnswerTone => "answer tone",
            State::Ac => "AC",
            State::Ca => "CA",
            State::CaToAc => "CA to AC",
            State::AcAgain => "AC again",
            State::Gap => "gap",
            State::AfterR1 => "after R1",
            State::AwaitingR2 => "awaiting R2",
            State::SendS => "S",
            State::SendSBar => "S bar",
            State::SendTrn => "TRN",
            State::SendRate => "rate signal",
            State::SendEnd => "E",
            State::Settling => "settling",
            State::Connected(_) => "connected",
            State::Failed => "failed",
        }
    }

    /// What the line is carrying at the moment.
    pub fn heard(&self) -> Heard {
        self.listener.classify()
    }

    /// Advance one sample: listen, and decide what to send.
    pub fn step(&mut self, line: f64, tx: &mut Transmitter, rx: &mut Receiver) -> Status {
        self.listener.feed(line);
        rx.feed(line);

        // Reversals and rate sequences arrive whenever they arrive, which is
        // very rarely on one of this machine's symbol boundaries. Both are
        // therefore latched until the next one: the state machine runs a
        // symbol at a time, and anything not held for it is simply lost.
        let reversal = {
            let a = self.carrier_reversals.feed(line);
            let b = self.low_reversals.feed(line);
            let c = self.high_reversals.feed(line);
            let any = a || b || c;
            if self.reversal_quiet > 0 {
                self.reversal_quiet -= 1;
                false
            } else if any {
                // Both sidebands belong to the one signal and turn together.
                self.reversal_quiet = (self.sps * 16.0) as u64;
                true
            } else {
                false
            }
        };

        // Bits arriving feed the rate detector while there is still a rate to
        // agree; afterwards they are the caller's, as data.
        self.pending_reversal |= reversal;
        if !matches!(self.state, State::Connected(_) | State::Failed) {
            for bit in rx.take_bits() {
                if let Some(s) = self.rates.feed(bit) {
                    self.pending_sequence = Some(s);
                }
            }
        }

        self.countdown -= 1.0;
        if self.countdown > 0.0 {
            return self.status();
        }
        self.countdown += self.sps;
        let reversal = std::mem::take(&mut self.pending_reversal);
        let sequence = self.pending_sequence.take();
        self.symbols += 1;
        self.total += 1;
        if let Some(t) = self.timer.as_mut() {
            *t += 1;
        }
        self.advance(reversal, sequence, tx);
        if !matches!(self.state, State::Connected(_) | State::Failed)
            && self.total >= timing::PATIENCE
        {
            self.state = State::Failed;
        }
        self.status()
    }

    /// One symbol of the state machine.
    fn advance(
        &mut self,
        reversal: bool,
        sequence: Option<u16>,
        tx: &mut Transmitter,
    ) {
        let heard = self.listener.classify();
        match self.state {
            // ---- calling modem -------------------------------------------
            State::Listening => {
                // 5.4.1: silent until the answering modem is heard. Note 1
                // there allows starting on the alternating tones alone, since
                // the answering tone may have been truncated or suppressed.
                tx.set_signal(Signal::Silent);
                // Either the answering tone heard for a second, or the
                // alternating tones on their own: note 1 to 5.4.2 allows the
                // second, since the answering tone may have been truncated or
                // never sent at all on a national connection.
                let heard_enough = self.hold(heard == Heard::AnswerTone)
                    >= timing::HEARD_ANSWER_TONE;
                if heard == Heard::Alternation || heard_enough {
                    self.enter(State::Aa);
                }
            }
            State::Aa => {
                tx.set_signal(Signal::StateA);
                if reversal {
                    // The far end has turned its alternation over. Start the
                    // clock and owe it a reversal of our own in 64 symbols.
                    self.timer = Some(0);
                    self.enter(State::AaToCc);
                }
            }
            State::AaToCc => {
                if self.symbols >= timing::RESPONSE {
                    tx.set_signal(Signal::StateC);
                    self.enter(State::Cc);
                }
            }
            State::Cc => {
                if reversal {
                    // Our reversal has come back, so stop the clock.
                    self.round_trip = self.measured();
                    tx.set_signal(Signal::Silent);
                    self.enter(State::AwaitingR1);
                }
            }
            State::AwaitingR1 => {
                tx.set_signal(Signal::Silent);
                if let Some(s) = sequence.filter(|&s| is_rate_signal(s)) {
                    self.agreed = offered_rate(s).min(offered_rate(self.offer));
                    if self.agreed == 0 {
                        self.state = State::Failed;
                        return;
                    }
                    tx.set_signal(Signal::ConditioningS);
                    self.enter(State::PreRoll);
                }
            }
            State::PreRoll => {
                // 5.4.1: an S sequence for the period already measured, which
                // lines this modem's conditioning signal up with the far end's
                // idea of when it should arrive.
                if self.symbols >= self.round_trip {
                    self.enter(State::SendS);
                }
            }

            // ---- answering modem -----------------------------------------
            State::AnswerTone => {
                tx.set_signal(Signal::AnswerTone);
                if self.symbols >= timing::ANSWER_TONE {
                    tx.set_signal(Signal::AlternateAC);
                    self.enter(State::Ac);
                }
            }
            State::Ac => {
                // 5.4.2: an even number of symbols, at least 128, and the
                // calling modem's carrier heard for 64.
                if heard == Heard::Carrier {
                    self.carrier_peak = self.carrier_peak.max(self.listener.carrier_amplitude());
                }
                let long_enough = self.symbols >= timing::MIN_ALTERNATION && self.symbols.is_multiple_of(2);
                if long_enough && self.hold(heard == Heard::Carrier) >= timing::HEARD_CARRIER {
                    self.timer = Some(0);
                    tx.set_signal(Signal::AlternateCA);
                    self.enter(State::Ca);
                }
            }
            State::Ca => {
                self.carrier_peak = self.carrier_peak.max(self.listener.carrier_amplitude());
                if reversal {
                    self.round_trip = self.measured();
                    self.enter(State::CaToAc);
                }
            }
            State::CaToAc => {
                self.carrier_peak = self.carrier_peak.max(self.listener.carrier_amplitude());
                if self.symbols >= timing::RESPONSE {
                    tx.set_signal(Signal::AlternateAC);
                    self.enter(State::AcAgain);
                }
            }
            State::AcAgain => {
                // 5.4.2: wait for the incoming tone to drop away, which is the
                // calling modem ceasing to transmit once it has its own
                // measurement.
                let dropped = self.listener.carrier_amplitude() < self.carrier_peak / 4.0;
                if self.hold(dropped) >= timing::GAP {
                    tx.set_signal(Signal::Silent);
                    self.enter(State::Gap);
                }
            }
            State::Gap => {
                if self.symbols >= timing::GAP {
                    self.enter(State::SendS);
                }
            }
            State::AfterR1 => {
                // 5.4.2: having sent R1 and heard the far end's conditioning
                // signal, wait out the measured round trip before believing
                // what arrives next.
                tx.set_signal(Signal::Silent);
                if self.symbols >= self.round_trip {
                    self.rates.reset();
                    self.enter(State::AwaitingR2);
                }
            }
            State::AwaitingR2 => {
                tx.set_signal(Signal::Silent);
                if let Some(s) = sequence.filter(|&s| is_rate_signal(s)) {
                    self.agreed = offered_rate(s).min(offered_rate(self.offer));
                    if self.agreed == 0 {
                        self.state = State::Failed;
                        return;
                    }
                    tx.set_signal(Signal::ConditioningS);
                    self.enter(State::SendS);
                }
            }

            // ---- both ends -----------------------------------------------
            State::SendS => {
                tx.set_signal(Signal::ConditioningS);
                if self.symbols >= timing::SEGMENT_S {
                    tx.set_signal(Signal::ConditioningSbar);
                    self.enter(State::SendSBar);
                }
            }
            State::SendSBar => {
                if self.symbols >= timing::SEGMENT_S_BAR {
                    tx.set_signal(Signal::Trn);
                    self.enter(State::SendTrn);
                }
            }
            State::SendTrn => {
                if self.symbols >= timing::SEGMENT_TRN {
                    self.rates.reset();
                    tx.set_signal(Signal::Rate(self.offer));
                    self.enter(State::SendRate);
                }
            }
            State::SendRate => {
                // The answering modem comes through here twice, and what it
                // is waiting for differs. The first time it is sending R1,
                // and 5.4.2 has it stop when the calling modem's conditioning
                // signal arrives, not when a rate does; the second time it is
                // sending R3 and waits to be closed out with an E. Having
                // agreed a rate already is what tells the two apart.
                if self.role == Role::Answering
                    && self.agreed == 0
                    && self.hold(heard == Heard::Conditioning) >= timing::HEARD_CARRIER
                {
                    tx.set_signal(Signal::Silent);
                    self.enter(State::AfterR1);
                    return;
                }
                match sequence {
                    Some(s) if is_end_signal(s) => {
                        // The far end has closed the exchange out. Answer in
                        // kind and settle at what it named.
                        if self.agreed == 0 {
                            self.agreed = offered_rate(s);
                        }
                        tx.set_signal(Signal::Rate(end_signal(self.offer)));
                        self.enter(State::SendEnd);
                    }
                    Some(s) if is_rate_signal(s) => {
                        let theirs = offered_rate(s);
                        if theirs == 0 {
                            // Table 6: no rate at all is a call to clear down.
                            self.state = State::Failed;
                            return;
                        }
                        // 5.4.1: R2 excludes anything R1 did not offer, and
                        // R3 anything R2 did not, so each step down the chain
                        // is the lesser of what the two ends can do.
                        let mine = offered_rate(self.offer);
                        self.agreed = if self.agreed == 0 {
                            theirs.min(mine)
                        } else {
                            self.agreed.min(theirs)
                        };
                        tx.set_signal(Signal::Rate(end_signal(self.offer)));
                        self.enter(State::SendEnd);
                    }
                    _ => {}
                }
            }
            State::SendEnd => {
                // 5.3.2: one complete sixteen-bit sequence, which is eight
                // symbols at two bits each.
                if self.symbols >= 8 {
                    tx.set_signal(Signal::ScrambledOnes);
                    self.enter(State::Settling);
                }
            }
            State::Settling => {
                if let Some(s) = sequence.filter(|&s| is_end_signal(s) && self.agreed == 0) {
                    self.agreed = offered_rate(s);
                }
                if self.symbols >= timing::SETTLE {
                    // Only 4800 is demodulated here, so anything else agreed
                    // would be a rate this modem cannot actually receive.
                    let rate = if self.agreed == 0 { 4800 } else { self.agreed };
                    self.state = State::Connected(rate);
                }
            }
            State::Connected(_) | State::Failed => {}
        }
    }

    /// What the clock says the line adds, once everything else is taken off.
    ///
    /// Four things sit between the two events the clock is started and stopped
    /// by, and only one of them is the line.
    ///
    /// The two ends do not measure the same interval, which is the part most
    /// easily got wrong. The calling modem starts its clock on *detecting* the
    /// far end's reversal and stops it on detecting the answer, so both 64
    /// symbol waits fall inside: its own and the far end's. The answering
    /// modem starts its clock on *sending* its own reversal, so only the far
    /// end's wait is inside. Subtracting 64 at both would leave the calling
    /// modem reading a round trip 64 symbols too long.
    ///
    /// The rest is the machinery at each end. The shaper holds a pulse back
    /// while it is being formed, and the reversal detector cannot report
    /// anything until its average has followed the signal round; both happen
    /// twice, once going and once coming back, and on a short line they come
    /// to more than the line does.
    fn measured(&mut self) -> u64 {
        let response = match self.role {
            Role::Calling => 2 * timing::RESPONSE,
            Role::Answering => timing::RESPONSE,
        };
        let latency = 2.0 * f64::from(self.low_reversals.latency()) / self.sps;
        let overhead = response + latency.round() as u64 + 2 * super::SHAPING_DELAY;
        self.timer.take().unwrap_or(0).saturating_sub(overhead)
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.symbols = 0;
        self.held = 0;
    }

    /// Symbols the condition being waited for has held unbroken.
    fn hold(&mut self, present: bool) -> u64 {
        if present {
            self.held += 1;
        } else {
            self.held = 0;
        }
        self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    /// What a listener makes of half a second of one signal.
    fn hear(signal: Signal, mode: Mode) -> Heard {
        let mut tx = Transmitter::new(mode, FS);
        tx.set_signal(signal);
        let mut listener = Listener::new(FS);
        for _ in 0..(FS as usize / 2) {
            listener.feed(tx.next_sample());
        }
        listener.classify()
    }

    #[test]
    #[ignore]
    fn lines_of_each_signal() {
        for (name, signal, mode) in [
            ("Silent", Signal::Silent, Mode::Call),
            ("AnswerTone", Signal::AnswerTone, Mode::Answer),
            ("StateA", Signal::StateA, Mode::Call),
            ("AlternateAC", Signal::AlternateAC, Mode::Answer),
            ("ConditioningS", Signal::ConditioningS, Mode::Call),
            ("Trn", Signal::Trn, Mode::Call),
            ("ScrambledOnes", Signal::ScrambledOnes, Mode::Call),
            ("Rate", Signal::Rate(0x0699), Mode::Call),
        ] {
            let mut tx = Transmitter::new(mode, FS);
            tx.set_signal(signal);
            let mut l = Listener::new(FS);
            for _ in 0..(FS as usize / 2) {
                l.feed(tx.next_sample());
            }
            let level = l.level();
            println!(
                "{name:>14}: level {level:.3}  answer {:.3}  carrier {:.3}                   low {:.3}  high {:.3}   ratios {:.2} {:.2} {:.2}",
                l.answer.amplitude(), l.carrier.amplitude(),
                l.low.amplitude(), l.high.amplitude(),
                l.answer.amplitude() / level, l.carrier.amplitude() / level,
                l.low.amplitude().min(l.high.amplitude()) / level,
            );
        }
    }

    #[test]
    fn each_start_up_signal_is_recognised_for_what_it_is() {
        assert_eq!(hear(Signal::Silent, Mode::Call), Heard::Nothing);
        assert_eq!(hear(Signal::AnswerTone, Mode::Answer), Heard::AnswerTone);
        assert_eq!(hear(Signal::StateA, Mode::Call), Heard::Carrier);
        assert_eq!(hear(Signal::StateC, Mode::Call), Heard::Carrier);
        assert_eq!(hear(Signal::AlternateAC, Mode::Answer), Heard::Alternation);
        assert_eq!(hear(Signal::AlternateCA, Mode::Answer), Heard::Alternation);
        assert_eq!(
            hear(Signal::ConditioningS, Mode::Call),
            Heard::Conditioning
        );
        assert_eq!(
            hear(Signal::ConditioningSbar, Mode::Call),
            Heard::Conditioning
        );
        assert_eq!(hear(Signal::Trn, Mode::Call), Heard::Spread);
        assert_eq!(hear(Signal::ScrambledOnes, Mode::Call), Heard::Spread);
        assert_eq!(hear(Signal::Rate(0x0699), Mode::Call), Heard::Spread);
    }

    #[test]
    fn an_alternation_is_not_confused_with_the_conditioning_signal() {
        // The two put their sidebands in the same place and differ only in
        // whether the carrier survives, which is the one thing the start-up
        // turns on: hearing 5.2's conditioning signal when 5.4's alternation
        // was sent would have a modem skip the whole delay measurement.
        assert_ne!(
            hear(Signal::AlternateAC, Mode::Answer),
            hear(Signal::ConditioningS, Mode::Answer)
        );
    }

    #[test]
    fn a_rate_signal_says_what_it_offers() {
        let r = rate_signal(true, false);
        assert!(is_rate_signal(r), "{r:016b} lacks its synchronising bits");
        assert!(!is_end_signal(r));
        assert_eq!(offered_rate(r), 4800);

        let both = rate_signal(true, true);
        assert_eq!(offered_rate(both), 9600, "the highest offered wins");

        let e = end_signal(r);
        assert!(is_end_signal(e), "{e:016b} is not recognised as E");
        assert!(!is_rate_signal(e), "E was taken for a rate signal");
        assert_eq!(offered_rate(e), 4800, "E carries the rate it settles on");
    }

    #[test]
    fn no_rate_at_all_calls_for_a_cleardown() {
        // Table 6: B4 to B6 all zero. A modem that reads that as some default
        // rate would keep talking to one that has given up.
        let none = rate_signal(false, false);
        assert!(is_rate_signal(none));
        assert_eq!(offered_rate(none), 0);
    }
}
