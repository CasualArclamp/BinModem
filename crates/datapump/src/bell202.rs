//! Bell 202 — 1200 bit/s FSK, half duplex.
//!
//! One band for both directions: mark at 1200 Hz, space at 2200 Hz, 1200
//! baud. That is a modulation index of 0.83 and needs the whole voice band,
//! so the two ends cannot talk at once and take turns instead. It is the
//! modem caller ID still uses, and what most 1200 bit/s FSK on a phone line
//! or a radio is.
//!
//! Taking turns is the whole of the protocol:
//!
//! - An end with something to send waits until the line is quiet, raises its
//!   carrier at mark, and holds it for the RTS-to-CTS delay so that the far
//!   receiver's carrier detector and framer have an idle line to find the
//!   first start bit on.
//! - When it runs out it holds mark a moment longer, in case there is more,
//!   then ends with the soft carrier turn-off: 900 Hz for 10 ms, phase
//!   continuous. The far end reads that as "carrier going, ignore what comes
//!   next" and squelches its receiver through the ring-down that would
//!   otherwise come out as a stray character.
//! - The answering end waits a little longer for quiet than the calling end
//!   does, so that two ends that find something to say at the same moment
//!   do not both key up.
//!
//! The call itself is set up as a Bell 103 or 212A is: the answering end
//! waits out the billing delay and sends 2225 Hz; the calling end hears it,
//! waits for it to end, and both are then connected and silent. Neither can
//! tell the other has hung up from the carrier -- an idle half-duplex line
//! has none -- so `carrier()` stays up for the whole of a connected call.
//!
//! The 5 bit/s reverse channel (387 Hz) is not implemented.

use dsp::FskDetector;

use crate::bell103::Role;
use crate::framing::{AsyncBits, AsyncFramer};
use crate::v21::Tone;

pub const BAUD: f64 = 1200.0;
pub const MARK: f64 = 1200.0;
pub const SPACE: f64 = 2200.0;
/// Soft carrier turn-off.
pub const SCT: f64 = 900.0;
/// The answering end's answer tone.
pub const ANSWER_TONE: f64 = 2225.0;

mod timing {
    /// Off hook and silent before the answer tone.
    pub const BILLING_DELAY: f64 = 2.0;
    /// How long the answer tone lasts.
    pub const ANSWER_TONE: f64 = 2.0;
    /// The answer tone heard this long before the calling end believes it.
    pub const HEARD_TONE: f64 = 0.3;
    /// And gone this long before the call is up.
    pub const TONE_GONE: f64 = 0.05;
    /// Carrier up at mark before the first data bit: the RTS-to-CTS delay.
    pub const RTS_CTS: f64 = 0.033;
    /// Mark held after the last bit before turning the carrier off.
    pub const HANG: f64 = 0.020;
    /// The soft carrier turn-off tone.
    pub const SCT: f64 = 0.010;
    /// Quiet needed before keying: the calling end, and the answering end,
    /// which defers.
    pub const QUIET_CALLING: f64 = 0.015;
    pub const QUIET_ANSWERING: f64 = 0.060;
    /// Received 900 Hz this long and it is a turn-off, not a mark.
    pub const HEARD_SCT: f64 = 0.004;
    /// Nothing recognisable for this long and the attempt is abandoned.
    pub const PATIENCE: f64 = 60.0;
}

/// Discriminator reading for 900 Hz: well past mark, which reads +1.
const SCT_LEVEL: f64 = 1.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Billing,
    AnswerTone,
    Listening,
    ToneEnding,
    Connected,
    Failed,
}

/// Where the transmitter is in a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    Quiet,
    Preamble,
    Sending,
    Hang,
    TurnOff,
}

/// Continuous-phase FSK transmitter with the 900 Hz turn-off.
#[derive(Debug)]
struct Tx {
    fs: f64,
    phase: f64,
    countdown: f64,
    sps: f64,
    pending: std::collections::VecDeque<bool>,
    current: bool,
}

impl Tx {
    fn new(fs: f64) -> Self {
        Self {
            fs,
            phase: 0.0,
            countdown: 0.0,
            sps: fs / BAUD,
            pending: Default::default(),
            current: true,
        }
    }

    /// One sample of `hz`, carrying the phase on.
    fn tone(&mut self, hz: f64) -> f64 {
        self.phase += hz / self.fs;
        self.phase -= self.phase.floor();
        (self.phase * std::f64::consts::TAU).cos()
    }

    /// One sample of data, mark when nothing is queued.
    fn data(&mut self) -> f64 {
        self.countdown -= 1.0;
        if self.countdown <= 0.0 {
            self.countdown += self.sps;
            self.current = self.pending.pop_front().unwrap_or(true);
        }
        self.tone(if self.current { MARK } else { SPACE })
    }

    /// Whether the bit on the line is the last of what was queued.
    fn drained(&self) -> bool {
        self.pending.is_empty() && self.current
    }
}

/// One end of a Bell 202 call.
#[derive(Debug)]
pub struct Modem {
    role: Role,
    state: State,
    turn: Turn,
    step: f64,
    elapsed: f64,
    total: f64,
    held: f64,
    /// Seconds in the current turn state.
    turn_elapsed: f64,
    /// Seconds the line has been quiet.
    quiet: f64,
    /// Seconds of 900 Hz heard in a row, and whether it has been believed.
    sct_heard: f64,
    squelched: bool,
    tx: Tx,
    tone: Tone,
    detector: FskDetector,
    framer: AsyncFramer,
    /// For the answer tone: a discriminator either side of 2225 Hz.
    ear: FskDetector,
    received: Vec<u8>,
    framing: AsyncBits,
    last_level: f64,
    symbol: Option<f64>,
}

impl Modem {
    pub fn new(role: Role, fs: f64) -> Self {
        Self {
            role,
            state: match role {
                Role::Originate => State::Listening,
                Role::Answer => State::Billing,
            },
            turn: Turn::Quiet,
            step: 1.0 / fs,
            elapsed: 0.0,
            total: 0.0,
            held: 0.0,
            turn_elapsed: 0.0,
            quiet: 0.0,
            sct_heard: 0.0,
            squelched: false,
            tx: Tx::new(fs),
            tone: Tone::new(ANSWER_TONE, fs),
            detector: FskDetector::new(SPACE, MARK, BAUD, fs),
            framer: AsyncFramer::new(BAUD, fs, 8),
            ear: FskDetector::new(ANSWER_TONE - 100.0, ANSWER_TONE + 100.0, 300.0, fs),
            received: Vec::new(),
            framing: AsyncBits::new(8),
            last_level: 0.0,
            symbol: None,
        }
    }

    /// Take one sample from the line and give back the one to put on it.
    pub fn step(&mut self, line: f64) -> f64 {
        self.listen(line);
        self.advance();
        self.speak()
    }

    fn listen(&mut self, line: f64) {
        let level = self.detector.feed(line);
        self.ear.feed(line);
        self.last_level = level;
        let carrier = self.detector.carrier();
        if self.turn != Turn::Quiet || self.state != State::Connected {
            // Our own carrier, or the handshake: nothing to read.
            self.framer.feed(1.0, false);
            self.framer.take_sampled();
            self.squelched = false;
            self.sct_heard = 0.0;
            return;
        }
        // A turn-off: squelch until the carrier has gone.
        self.sct_heard = if carrier && level > SCT_LEVEL {
            self.sct_heard + self.step
        } else {
            0.0
        };
        if self.sct_heard >= timing::HEARD_SCT {
            self.squelched = true;
        }
        if !carrier {
            self.squelched = false;
        }
        let live = carrier && !self.squelched;
        // 900 Hz is mark-side, so the framer sees an idle line through it.
        if let Some(byte) = self.framer.feed(level.min(1.0), live) {
            self.received.push(byte);
        }
        self.symbol = self.framer.take_sampled();
    }

    fn advance(&mut self) {
        self.elapsed += self.step;
        self.total += self.step;
        let tone = self.ear.carrier() && self.ear.level().abs() < 0.5;
        match self.state {
            State::Billing => {
                if self.elapsed >= timing::BILLING_DELAY {
                    self.enter(State::AnswerTone);
                }
            }
            State::AnswerTone => {
                if self.elapsed >= timing::ANSWER_TONE {
                    self.enter(State::Connected);
                }
            }
            State::Listening => {
                if self.hold(tone) >= timing::HEARD_TONE {
                    self.enter(State::ToneEnding);
                }
            }
            State::ToneEnding => {
                if self.hold(!self.ear.carrier()) >= timing::TONE_GONE {
                    self.enter(State::Connected);
                }
            }
            State::Connected => self.take_turns(),
            State::Failed => {}
        }
        if self.state != State::Connected
            && self.state != State::Failed
            && self.total >= timing::PATIENCE
        {
            self.state = State::Failed;
        }
    }

    fn take_turns(&mut self) {
        self.turn_elapsed += self.step;
        let far = self.detector.carrier();
        match self.turn {
            Turn::Quiet => {
                // Quiet is counted only while there is something waiting to
                // go: a line that has been idle for a minute says nothing
                // about whether the far end found something to say in the
                // same millisecond as this one, and the answering end's
                // longer wait is only worth anything if both start counting
                // together.
                let waiting = !self.tx.pending.is_empty();
                self.quiet = if far || !waiting {
                    0.0
                } else {
                    self.quiet + self.step
                };
                let wait = match self.role {
                    Role::Originate => timing::QUIET_CALLING,
                    Role::Answer => timing::QUIET_ANSWERING,
                };
                if waiting && self.quiet >= wait {
                    self.tx.current = true;
                    self.tx.countdown = 0.0;
                    self.set_turn(Turn::Preamble);
                }
            }
            Turn::Preamble => {
                if self.turn_elapsed >= timing::RTS_CTS {
                    self.set_turn(Turn::Sending);
                }
            }
            Turn::Sending => {
                if self.tx.drained() {
                    self.set_turn(Turn::Hang);
                }
            }
            Turn::Hang => {
                if !self.tx.drained() {
                    self.set_turn(Turn::Sending);
                } else if self.turn_elapsed >= timing::HANG {
                    self.set_turn(Turn::TurnOff);
                }
            }
            Turn::TurnOff => {
                if self.turn_elapsed >= timing::SCT {
                    self.quiet = 0.0;
                    self.set_turn(Turn::Quiet);
                }
            }
        }
    }

    fn speak(&mut self) -> f64 {
        match self.state {
            State::AnswerTone => self.tone.next_sample(),
            State::Connected => match self.turn {
                Turn::Quiet => 0.0,
                Turn::Preamble => self.tx.tone(MARK),
                Turn::Sending | Turn::Hang => self.tx.data(),
                Turn::TurnOff => self.tx.tone(SCT),
            },
            _ => 0.0,
        }
    }

    fn set_turn(&mut self, turn: Turn) {
        self.turn = turn;
        self.turn_elapsed = 0.0;
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.elapsed = 0.0;
        self.held = 0.0;
    }

    fn hold(&mut self, present: bool) -> f64 {
        self.held = if present { self.held + self.step } else { 0.0 };
        self.held
    }

    pub fn status(&self) -> Status {
        match self.state {
            State::Connected => Status::Connected(BAUD as u32),
            State::Failed => Status::Failed,
            _ => Status::Negotiating,
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Billing => "billing delay",
            State::AnswerTone => "answer tone",
            State::Listening => "listening",
            State::ToneEnding => "tone ending",
            State::Connected => match self.turn {
                Turn::Quiet if self.detector.carrier() => "receiving",
                Turn::Quiet => "idle",
                Turn::Preamble => "carrier up",
                Turn::Sending => "sending",
                Turn::Hang => "hang",
                Turn::TurnOff => "turn-off",
            },
            State::Failed => "failed",
        }
    }

    /// Whether this end has the line.
    pub fn transmitting(&self) -> bool {
        self.state == State::Connected && self.turn != Turn::Quiet
    }

    /// Up for the whole of a connected call: an idle half-duplex line has no
    /// carrier on it, so its absence says nothing about the far end.
    pub fn carrier(&self) -> bool {
        self.state == State::Connected || self.detector.carrier()
    }

    /// Whether the far end's carrier is on the line right now.
    pub fn far_carrier(&self) -> bool {
        self.detector.carrier()
    }

    pub fn take_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.received)
    }

    /// The characters, back in start-stop bits (see `bell103::Modem::take_bits`).
    pub fn take_bits(&mut self) -> Vec<bool> {
        let bytes = std::mem::take(&mut self.received);
        bytes.iter().flat_map(|&b| self.framing.encode(b)).collect()
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.tx.pending.extend(bits.iter().copied());
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let bits: Vec<bool> = bytes.iter().flat_map(|&b| self.framing.encode(b)).collect();
        self.send_bits(&bits);
    }

    pub fn pending_bits(&self) -> usize {
        self.tx.pending.len()
    }

    pub fn framing_errors(&self) -> u64 {
        self.framer.framing_errors
    }

    /// Discriminator output: `+1` mark, `-1` space.
    pub fn level(&self) -> f64 {
        self.last_level
    }

    pub fn take_symbol(&mut self) -> Option<f64> {
        self.symbol.take()
    }
}

/// How far a Bell 202 call has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Negotiating,
    Connected(u32),
    Failed,
}
