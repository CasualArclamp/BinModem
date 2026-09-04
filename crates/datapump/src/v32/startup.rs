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
use dsp::{EchoCanceller, EchoFinder, ReversalDetector, ToneDetector};
// Part of this module's surface: `Modem::reflection` hands one back, and what
// is above a data pump should not have to reach past it to name the type.
pub use dsp::Reflection;

/// Half the symbol rate: where an alternating pattern puts its sidebands.
const OFFSET: f64 = super::BAUD / 2.0;

/// Level below which the line is carrying nothing.
///
/// A modem has to work across the range of levels the network delivers, which
/// is some 34 dB between a short local call and a long one. This sits below
/// the bottom of that: a signal arriving 20 dB down, which is ordinary once it
/// has crossed a network, reads about twelve times this.
const QUIET: f64 = 0.005;

/// Amplitude a tone must reach before its phase is worth watching, on the same
/// footing as [`QUIET`].
const AUDIBLE: f64 = 0.008;

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

    /// Amplitude of the bare carrier, which 5.4.2 watches for and then
    /// watches for a drop in.
    pub fn carrier_amplitude(&self) -> f64 {
        self.carrier.amplitude()
    }

    /// Amplitude of the weaker of the two sidebands, which is what 5.4.1 has
    /// the calling modem listen for as "600 Hz and 3000 Hz".
    pub fn sideband_amplitude(&self) -> f64 {
        self.low.amplitude().min(self.high.amplitude())
    }

    /// Amplitude of the answering tone (5.1).
    pub fn answer_amplitude(&self) -> f64 {
        self.answer.amplitude()
    }

    pub fn level(&self) -> f64 {
        self.power.value()
    }

    /// What the line is carrying, judged by which lines stand above the level
    /// of the whole signal.
    ///
    /// Only usable when the modem's own echo is either absent or cancelled.
    /// Everything here is a ratio to the total, and an uncancelled echo is
    /// part of that total: a modem sending a bare carrier and hearing it come
    /// back off the hybrid will find the carrier standing proud of everything
    /// else and conclude the far end is sending one.
    ///
    /// The start-up avoids depending on it until then, and can, because the
    /// signals of its half-duplex opening are in different places: a modem
    /// repeating a state puts everything at 1800 Hz and listens at 600 and
    /// 3000, and the modem alternating states does the exact reverse. Each is
    /// deaf to its own echo by construction rather than by cancelling it,
    /// which is what lets the exchange happen before anything is trained.
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

/// Finds the 16-bit sequences a rate exchange is made of (5.3).
///
/// The stream carries no framing, so the boundary has to be found in it.
/// 5.3.1 gives the rule: two consecutive identical sixteens with their
/// synchronising bits in the right places, which data is very unlikely to
/// imitate by accident. That fixes the boundary as well as identifying the
/// signal, and everything after can be read off it directly.
///
/// Reading it off matters, because the sequence that ends the exchange is sent
/// exactly once. 5.3.2 has a modem "first complete the transmission of the
/// current 16-bit rate sequence, and then transmit one 16-bit sequence E", so
/// a detector that insisted on seeing every sequence twice would see every
/// rate signal and never the thing that ends them. It would also be looking
/// for a repetition that cannot occur, since what follows E is data.
#[derive(Debug, Default)]
pub struct RateDetector {
    /// The last thirty-two bits seen, newest at the bottom.
    window: u32,
    filled: u32,
    /// Whether the 16-bit boundary has been found.
    locked: bool,
    /// Bits since that boundary.
    since: u32,
}

impl RateDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer one received bit. Yields each complete sequence once the boundary
    /// between them is known.
    pub fn feed(&mut self, bit: bool) -> Option<u16> {
        self.window = (self.window << 1) | u32::from(bit);
        self.filled = (self.filled + 1).min(32);
        if self.filled < 32 {
            return None;
        }
        let group = self.window as u16;
        let previous = (self.window >> 16) as u16;

        // 5.3.1: two identical sixteens with the synchronising bits in place.
        // Checked at every position rather than only until a boundary is first
        // found, because a boundary found in noise will never match the real
        // thing, and a detector that could not change its mind stayed wrong
        // for the rest of the call.
        if group == previous && is_rate_signal(group) {
            self.locked = true;
            self.since = 0;
            return Some(group);
        }

        // The sequence that ends the exchange is sent exactly once (5.3.2), so
        // it cannot be asked to repeat. Accepting a lone group is only safe
        // once the boundary is known: the synchronising bits are seven of
        // sixteen, so one group in a hundred and twenty-eight of anything at
        // all matches them, and a detector that accepted lone groups at an
        // unknown boundary finds a rate signal in scrambled data within a
        // second. That is exactly what happened, and what it cost was a modem
        // deciding it had heard R1 partway through the far end's training
        // segment and answering over the top of it.
        if self.locked {
            self.since += 1;
            if self.since >= 16 {
                self.since = 0;
                if is_end_signal(group) {
                    return Some(group);
                }
            }
        }
        None
    }

    pub fn reset(&mut self) {
        self.window = 0;
        self.filled = 0;
        self.locked = false;
        self.since = 0;
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
    /// Shortest a rate signal may be sent for.
    ///
    /// 5.3.1 identifies one by two identical sixteen-bit sequences, so four of
    /// them is twice what a far end needs to see and still only thirteen
    /// milliseconds.
    pub const MIN_RATE_SIGNAL: u64 = 32;
    /// Silence after the amplitude drop (5.4.2).
    pub const GAP: u64 = 16;
    /// Segment 1 of the conditioning signal (5.2.1).
    pub const SEGMENT_S: u64 = 256;
    /// Segment 2 (5.2.2).
    pub const SEGMENT_S_BAR: u64 = 16;
    /// Segment 3, at its shortest (5.2.3 gives 1280 to 8192).
    pub const SEGMENT_TRN: u64 = 1280;
    /// Segment 3 on a line long enough to reflect as well as attenuate.
    ///
    /// Note 3 to 5.4.2 says the training segment "is suitable for training the
    /// echo canceller in the transmitting modem", and allows a longer sequence
    /// still if one is wanted. One is wanted here. A network reflection has to
    /// be found before the taps that cancel it can be placed, and then those
    /// taps have to converge, and both have to happen inside the one stretch
    /// of the start-up the far end is silent for. The shortest segment allowed
    /// is half a second, which is enough for one of those jobs.
    pub const SEGMENT_TRN_LONG: u64 = 4096;
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
    carrier_quiet: u64,
    sideband_quiet: u64,
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
    /// Whether a training segment has been sent yet. Only the first is a
    /// window the echo canceller can learn anything from.
    trained: bool,
    /// Held until the next symbol boundary, where the machine can act on them.
    pending_carrier_reversal: bool,
    pending_sideband_reversal: bool,
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
            carrier_reversals: ReversalDetector::new(super::CARRIER, 60.0, AUDIBLE, fs),
            low_reversals: ReversalDetector::new(super::CARRIER - OFFSET, 60.0, AUDIBLE, fs),
            high_reversals: ReversalDetector::new(super::CARRIER + OFFSET, 60.0, AUDIBLE, fs),
            carrier_quiet: 0,
            sideband_quiet: 0,
            rates: RateDetector::new(),
            countdown: sps,
            sps,
            symbols: 0,
            total: 0,
            timer: None,
            round_trip: 0,
            held: 0,
            carrier_peak: 0.0,
            trained: false,
            pending_carrier_reversal: false,
            pending_sideband_reversal: false,
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

    /// Whether this end is sending the training segment, which is the one
    /// stretch of the start-up the far end is required to be silent through
    /// and therefore the only time an echo canceller can learn anything.
    ///
    /// Note 3 to 5.4.2 says as much: the TRN segment "is suitable for training
    /// the echo canceller in the transmitting modem", and allows a separate
    /// sequence before the conditioning signal if a longer one is wanted.
    ///
    /// Only the first one, though. The answering modem sends a conditioning
    /// signal twice, and the second time the calling modem is still sending
    /// R2 over the top of it: 5.4.1 has that continue "until an incoming rate
    /// signal R3 is detected", which cannot arrive until the conditioning
    /// signal it follows is over. Adapting through that has the canceller try
    /// to explain the far end as an echo of us and throw away everything it
    /// learned in the first segment, when the line really was quiet. Left in,
    /// it cost the answering modem its receiver: a residual error of 0.55
    /// against the 0.07 the other end managed on the same call.
    pub fn training_echo(&self) -> bool {
        matches!(self.state, State::SendTrn) && !self.trained
    }

    /// Whether the far end is required to be silent just now.
    ///
    /// True through the whole of this modem's own first conditioning sequence
    /// and not merely its training segment. Everything on the line then is our
    /// own echo, so a receiver that goes on adapting through it is adapting to
    /// the wrong signal, and an equaliser and a carrier loop that have settled
    /// on a modem's own transmission have settled somewhere it will not easily
    /// leave.
    ///
    /// That is not hypothetical either. The calling modem trains its receiver
    /// on the answering modem's first conditioning signal and had it locked, a
    /// residual error of 0.013; it then began its own conditioning sequence,
    /// spent it converging onto its own echo instead, and sat at 0.5 for the
    /// rest of the call, unable to read the R3 it was waiting for. The
    /// answering modem, which is silent while it listens, never had the
    /// problem, which is what made it look like an echo canceller fault.
    ///
    /// Deliberately narrower than [`training_echo`](Self::training_echo),
    /// which is TRN alone. Both are windows where the line carries nothing but
    /// us, but the echo canceller wants only the part of it with no pattern:
    /// S and S-bar repeat every two symbols, and a filter learned from a
    /// periodic reference is one of the many that explain that period and
    /// almost certainly not the one the line is.
    pub fn far_end_quiet(&self) -> bool {
        if matches!(self.state, State::Connected(_)) {
            // The listener below is only fed while the start-up is running, so
            // its answer goes stale the moment this connects. Data state has
            // its own reasons to keep adapting and none to stop.
            return false;
        }
        if matches!(
            self.state,
            State::PreRoll | State::SendS | State::SendSBar | State::SendTrn
        ) && !self.trained
        {
            return true;
        }
        // Or the line simply has nothing on it, which the start-up leaves it
        // with more than once: between one modem finishing a sequence and the
        // other reacting there is a round trip of silence, and an adaptive
        // receiver let loose on silence does not stay where it was put. The
        // calling modem lost a residual error of 0.013 to 0.46 in the 68 ms
        // between starting R2 and the answer arriving.
        self.listener.classify() == Heard::Nothing
    }

    /// How long the training segment is being sent for, in symbols.
    ///
    /// 5.2.3 allows anything from 1280 to 8192, and which end of that to use
    /// is decided by the round trip already measured: a line short enough that
    /// everything reflected comes back inside the near taps has only one job
    /// to do here, and a longer one has two.
    pub fn training_symbols(&self) -> u64 {
        let near = (ECHO_SPAN_MS * super::BAUD / 1000.0) as u64;
        if self.round_trip > near {
            timing::SEGMENT_TRN_LONG
        } else {
            timing::SEGMENT_TRN
        }
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
        //
        // The carrier and the sidebands are kept apart rather than run
        // together, because which of them a modem should be listening to
        // depends on which end of the call it is. A calling modem repeats one
        // state, which puts everything at 1800 Hz and nothing at the
        // sidebands; an answering modem alternates, which does the exact
        // reverse. Each therefore listens where its own signal is not, and is
        // deaf to its own reflection by construction — the same trick
        // `Listener::classify` relies on, and it has to be the same here.
        //
        // Watching both at once looks harmless and is not. A modem hears its
        // own hybrid at once and the far end after the length of the line, so
        // whichever of the two came back first stopped the clock, and it was
        // always the hybrid. Both ends duly measured a round trip of zero on a
        // line hundreds of miles long, and did it in a way nothing caught,
        // because a test line with no echo on it has nothing else to hear.
        let at_carrier = self.carrier_reversals.feed(line);
        // Both sidebands belong to the one signal and turn together.
        let at_low = self.low_reversals.feed(line);
        let at_high = self.high_reversals.feed(line);
        let carrier = self.gate(at_carrier, true);
        let sidebands = self.gate(at_low || at_high, false);

        // Bits arriving feed the rate detector while there is still a rate to
        // agree; afterwards they are the caller's, as data.
        self.pending_carrier_reversal |= carrier;
        self.pending_sideband_reversal |= sidebands;
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
        let carrier = std::mem::take(&mut self.pending_carrier_reversal);
        let sidebands = std::mem::take(&mut self.pending_sideband_reversal);
        let sequence = self.pending_sequence.take();
        self.symbols += 1;
        self.total += 1;
        if let Some(t) = self.timer.as_mut() {
            *t += 1;
        }
        self.advance(carrier, sidebands, sequence, tx);
        if !matches!(self.state, State::Connected(_) | State::Failed)
            && self.total >= timing::PATIENCE
        {
            self.state = State::Failed;
        }
        self.status()
    }

    /// Report a reversal at most once, and not again for a while.
    ///
    /// A phase reversal is an event in a signal that goes on either side of
    /// it, and a detector watching for one has no way to tell a second event
    /// from its own recovery from the first.
    fn gate(&mut self, fired: bool, is_carrier: bool) -> bool {
        let quiet = if is_carrier {
            &mut self.carrier_quiet
        } else {
            &mut self.sideband_quiet
        };
        if *quiet > 0 {
            *quiet -= 1;
            false
        } else if fired {
            *quiet = (self.sps * 16.0) as u64;
            true
        } else {
            false
        }
    }

    /// One symbol of the state machine.
    ///
    /// The two reversals are separate arguments rather than one, so that a
    /// state has to say which signal it is listening to. The far end's is the
    /// only right answer, and it is a different one at each end of the call.
    fn advance(
        &mut self,
        carrier_reversal: bool,
        sideband_reversal: bool,
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
                //
                // The tones are looked for where they are rather than by
                // classifying the whole line, which is what the rest of this
                // phase does too and for the reason given on `classify`.
                // The sidebands have to stand above the answering tone as well
                // as above the floor. A one-pole detector 900 Hz from a tone
                // still passes a fortieth of it, and a fortieth of the
                // answering tone is well clear of any absolute threshold worth
                // having: without this the calling modem hears the answer as
                // its own cue and starts transmitting over it immediately.
                let sidebands = self.listener.sideband_amplitude();
                let tones =
                    sidebands > AUDIBLE && sidebands > self.listener.answer_amplitude();
                let heard_enough = self.hold(heard == Heard::AnswerTone)
                    >= timing::HEARD_ANSWER_TONE;
                if tones || heard_enough {
                    self.enter(State::Aa);
                }
            }
            State::Aa => {
                tx.set_signal(Signal::StateA);
                // The far end is alternating, so its reversal is in the
                // sidebands. This end is repeating a state, which puts nothing
                // there at all.
                if sideband_reversal {
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
                if sideband_reversal {
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
                // 5.4.2: an even number of symbols, at least 128, and "an
                // incoming tone has been detected at 1800 Hz for 64 symbol
                // periods". Looked for at 1800 Hz exactly, where this modem's
                // own alternation puts nothing at all, so its echo of itself
                // cannot be mistaken for the far end.
                self.note_carrier();
                let long_enough =
                    self.symbols >= timing::MIN_ALTERNATION && self.symbols.is_multiple_of(2);
                let tone = self.listener.carrier_amplitude() > AUDIBLE;
                if long_enough && self.hold(tone) >= timing::HEARD_CARRIER {
                    self.timer = Some(0);
                    tx.set_signal(Signal::AlternateCA);
                    self.enter(State::Ca);
                }
            }
            State::Ca => {
                self.note_carrier();
                // The far end is repeating a state, so its reversal is in the
                // bare carrier at 1800 Hz. This end is alternating, which
                // suppresses the carrier and leaves that place empty.
                if carrier_reversal {
                    self.round_trip = self.measured();
                    self.enter(State::CaToAc);
                }
            }
            State::CaToAc => {
                self.note_carrier();
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
                if self.symbols >= self.training_symbols() {
                    self.trained = true;
                    self.rates.reset();
                    tx.set_signal(Signal::Rate(self.offer));
                    self.enter(State::SendRate);
                }
            }
            State::SendRate => {
                // A rate signal has to be sent long enough to be recognised.
                // 5.3.1 asks for two identical sixteens, so anything shorter
                // than a few of them cannot be detected however good the line
                // is: the answering modem was leaving this state six
                // milliseconds after entering it, having sent twenty-eight
                // bits of a thing that takes thirty-two to identify, and the
                // calling modem never saw an R3 at all.
                if self.symbols < timing::MIN_RATE_SIGNAL {
                    return;
                }
                match self.role {
                    // 5.4.2: the first time through, the answering modem is
                    // sending R1 and is waiting for the calling modem's
                    // conditioning signal, not for a rate. The second time it
                    // is sending R3 and waits to be closed out with an E.
                    // Having agreed a rate already is what tells them apart.
                    Role::Answering if self.agreed == 0 => {
                        if self.hold(heard == Heard::Conditioning) >= timing::HEARD_CARRIER {
                            tx.set_signal(Signal::Silent);
                            self.enter(State::AfterR1);
                        }
                    }
                    Role::Answering => {
                        if let Some(s) = sequence.filter(|&s| is_end_signal(s)) {
                            if self.agreed == 0 {
                                self.agreed = offered_rate(s);
                            }
                            tx.set_signal(Signal::Rate(end_signal(self.offer)));
                            self.enter(State::SendEnd);
                        }
                    }
                    // 5.4.1: "Transmission of R2 shall continue until an
                    // incoming rate signal R3 is detected."
                    Role::Calling => {
                        let Some(s) = sequence else { return };
                        if !is_rate_signal(s) && !is_end_signal(s) {
                            return;
                        }
                        let theirs = offered_rate(s);
                        if theirs == 0 {
                            // Table 6: no rate at all is a call to clear down.
                            self.state = State::Failed;
                            return;
                        }
                        // Each step down the chain is the lesser of what the
                        // two ends can do: R2 excludes anything R1 did not
                        // offer, and R3 anything R2 did not.
                        let mine = offered_rate(self.offer);
                        self.agreed = if self.agreed == 0 {
                            theirs.min(mine)
                        } else {
                            self.agreed.min(theirs)
                        };
                        tx.set_signal(Signal::Rate(end_signal(self.offer)));
                        self.enter(State::SendEnd);
                    }
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

    /// Remember how loud the calling modem's carrier has been, so that its
    /// going away can be recognised as a drop rather than against a threshold
    /// that would have to be told what the line is scaled to.
    fn note_carrier(&mut self) {
        self.carrier_peak = self.carrier_peak.max(self.listener.carrier_amplitude());
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

/// A complete V.32 modem: one end of a call, on a two-wire line.
///
/// Ties together the four things that have to run at once and cannot be run
/// separately. The transmitter and receiver share a band, so the receiver
/// hears the transmitter; the echo canceller removes that, but only once it
/// has been trained, and the only time it can be trained is while the far end
/// is required to be silent; and knowing when that is means following the
/// start-up. Each of those is testable on its own and none of them is much use
/// on its own.
#[derive(Debug)]
pub struct Modem {
    tx: Transmitter,
    rx: Receiver,
    startup: Startup,
    echo: EchoCanceller,
    /// Looks for the network's reflection while the line is quiet enough to
    /// find it. Dropped once it has answered.
    finder: Option<EchoFinder>,
    /// Samples spent looking, and how many to spend.
    searched: usize,
    search_for: usize,
    /// What the search turned up, kept for diagnostics: the number is the
    /// difference between a canceller that works on a long line and one that
    /// does not, and there is no way to see it from outside.
    reflection: Option<Reflection>,
    fs: f64,
    /// The return loss as training ended, which is the last moment it means
    /// anything: with both ends talking the meter compares everything heard
    /// against everything left, and the far end is in both.
    trained_loss: f64,
    was_training: bool,
}

/// How far back the first run of taps looks, in milliseconds.
///
/// The reflection off a hybrid, which is immediate, and a little of what the
/// network adds behind it.
const ECHO_SPAN_MS: f64 = 8.0;

/// How much line the second run of taps covers, in milliseconds.
///
/// A network reflection is one path among many rather than one impedance step,
/// so it arrives smeared rather than as a copy. This is what that smearing is
/// allowed to be; anything longer is taps modelling nothing.
const FAR_SPAN_MS: f64 = 4.0;

/// Weakest reflection worth a second run of taps.
///
/// Under this it is not clear there is a reflection at all. The search takes
/// the largest of some hundreds of candidates, and the largest of hundreds of
/// numbers that should all be zero is not zero; a bar has to sit above what
/// that alone produces. Above it, the reflection is within about 8 dB of the
/// far modem, which is close enough to be worth removing.
const FAINTEST: f64 = 0.15;

impl Modem {
    /// `offer` is the rate signal this modem sends, from [`rate_signal`].
    pub fn new(role: Role, offer: u16, fs: f64) -> Self {
        let (tx, rx) = endpoints(role, fs);
        Self {
            tx,
            rx,
            startup: Startup::new(role, offer, fs),
            echo: EchoCanceller::new((ECHO_SPAN_MS * fs / 1000.0) as usize, 0.5),
            finder: None,
            searched: 0,
            search_for: 0,
            reflection: None,
            fs,
            trained_loss: 0.0,
            was_training: false,
        }
    }

    /// Take one sample from the line and give back the one to put on it.
    pub fn step(&mut self, line: f64) -> f64 {
        // The canceller is told what went out and what came back, and returns
        // what is left. Its own history remembers how long ago each sample
        // was sent, so the caller need not.
        let sent = self.tx.last_sample();

        // The training segment is the only stretch of the start-up where the
        // line carries our own signal and nothing else, so it is the only
        // chance to find out where the line puts it back as well as what shape
        // it comes back in. The first half goes on finding it and the second
        // on cancelling it.
        if self.startup.training_echo() && !self.was_training {
            self.begin_search();
        }
        if let Some(finder) = self.finder.as_mut() {
            // What arrived, rather than what the canceller left of it: the
            // near echo it removes is nowhere near the delays being searched,
            // and this way the search does not depend on how the near taps are
            // getting on.
            finder.feed(sent, line);
            self.searched += 1;
            if self.searched >= self.search_for {
                self.place_far_taps();
            }
        }

        let cleaned = self.echo.process(sent, line);

        // Adapt only while this end is transmitting its training segment,
        // which is the one stretch the far end is required to be quiet for.
        // Adapting through the far end would have the canceller try to explain
        // it as an echo of us, which it is not, and unlearn what it knows.
        let training = self.startup.training_echo();
        // The equaliser is held still for the same reason the canceller is let
        // loose, and over a longer stretch: what is on the line through the
        // whole of our own conditioning sequence is this modem's own echo, and
        // there is nothing in it for a receiver to learn.
        self.rx.set_adapting(!self.startup.far_end_quiet());
        if self.was_training && !training {
            self.trained_loss = self.echo.echo_return_loss();
        }
        self.was_training = training;
        self.echo.set_adapting(training);

        if matches!(self.startup.status(), Status::Connected(_)) {
            self.rx.feed(cleaned);
        } else {
            self.startup.step(cleaned, &mut self.tx, &mut self.rx);
        }
        self.tx.next_sample()
    }

    /// Start looking for a network reflection, if there is anywhere for one to
    /// be that the near taps do not already cover.
    ///
    /// The round trip measured during the start-up is what bounds the search.
    /// Nothing can come back later than that, so the delays past it need not
    /// be considered, and on a short line there is nothing to consider at all.
    ///
    /// The bound is the measurement plus the width of the taps being placed,
    /// and it needs the slack to be in that direction rather than the other:
    /// searching too far costs arithmetic during a stretch where there is time
    /// for it, and searching too close in misses the reflection entirely. What
    /// slack there is turns out to be spare, because the measurement errs
    /// long — 81 symbols against a true 80 on a clean line, 106 against 96
    /// on one where the returning signal is weak enough to slow the detector
    /// down, which is the error the subtraction models least well.
    fn begin_search(&mut self) {
        let first = self.echo.span();
        let last = self.round_trip_samples() + self.far_taps();
        if last <= first {
            return;
        }
        self.finder = Some(EchoFinder::new(first, last));
        self.searched = 0;
        self.search_for =
            (self.startup.training_symbols() as f64 / 2.0 * self.fs / super::BAUD) as usize;
    }

    /// Put the second run of taps where the search says the reflection is.
    fn place_far_taps(&mut self) {
        let Some(finder) = self.finder.take() else {
            return;
        };
        let Some(found) = finder.best().filter(|f| f.strength >= FAINTEST) else {
            return;
        };
        // Centred on the reflection rather than starting at it: what comes
        // back is the signal through whatever the line did to it, and that
        // spreads either side of where the bulk of it lands.
        let taps = self.far_taps();
        let offset = found.delay.saturating_sub(taps / 2).max(self.echo.span());
        self.echo.watch_far_echo(offset, taps);
        self.reflection = Some(found);
    }

    fn far_taps(&self) -> usize {
        (FAR_SPAN_MS * self.fs / 1000.0) as usize
    }

    fn round_trip_samples(&self) -> usize {
        (self.startup.round_trip() as f64 * self.fs / super::BAUD) as usize
    }

    /// The network reflection the canceller went looking for, if it found one.
    pub fn reflection(&self) -> Option<Reflection> {
        self.reflection
    }

    pub fn status(&self) -> Status {
        self.startup.status()
    }

    pub fn phase(&self) -> &'static str {
        self.startup.phase()
    }

    /// The round trip the start-up measured, in symbol intervals.
    pub fn round_trip(&self) -> u64 {
        self.startup.round_trip()
    }

    /// How much of its own echo the modem was removing when it finished
    /// training, in decibels.
    ///
    /// Taken then because that is the last moment the figure means anything.
    /// The meter is the ratio of everything heard to everything left, and once
    /// the far end is talking it is in both, so the number falls towards the
    /// ratio of echo to far signal however well the cancelling is going.
    pub fn echo_return_loss(&self) -> f64 {
        self.trained_loss
    }

    /// Queue data for transmission. Only meaningful once connected.
    pub fn send(&mut self, bytes: &[u8]) {
        self.tx.push_bytes(bytes);
    }

    pub fn take_bytes(&mut self) -> Vec<u8> {
        self.rx.take_bytes()
    }

    /// Bits recovered from the line.
    ///
    /// What sits above a data pump under V.42 wants bits rather than bytes:
    /// the frames say where the octet boundaries are and the pump has no
    /// business guessing at them.
    pub fn take_bits(&mut self) -> Vec<bool> {
        self.rx.take_bits()
    }

    /// Queue bits for transmission.
    pub fn send_bits(&mut self, bits: &[bool]) {
        self.tx.push_bits(bits);
    }

    /// How many are still waiting to go out, so that whatever is feeding this
    /// knows when to hand over more.
    pub fn pending_bits(&self) -> usize {
        self.tx.pending_bits()
    }

    pub fn constellation_point(&self) -> (f64, f64) {
        self.rx.constellation_point()
    }

    /// Mean distance of the received symbols from the decisions made about
    /// them, which is how well the receiver is doing.
    pub fn residual_error(&self) -> f64 {
        self.rx.residual_error()
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
