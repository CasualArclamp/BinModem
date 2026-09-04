//! The V.22bis handshake (clause 6.3).
//!
//! Two modems that have never met agree on a signalling rate by taking turns
//! with the four signals of [`Signal`], to a schedule measured in
//! milliseconds. Neither is told what the other can do; each infers it from
//! what it hears. The exchange is arranged so that a V.22 modem, which knows
//! nothing of 2400 bit/s, still reaches a working connection at 1200: the
//! signal that offers the faster rate is one it never sends and never listens
//! for, so its silence on the matter is itself the answer.
//!
//! Driven a sample at a time, like everything else here. Each step reads what
//! the receiver is hearing and sets what the transmitter is sending.

use super::{Channel, Pattern, Rate, Receiver, Signal, Transmitter};

/// Which end of the call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Placed the call: transmits low, receives high, and begins silent.
    Calling,
    /// Took the call: transmits high, receives low, and speaks first.
    Answering,
}

/// How far the handshake has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Still negotiating.
    Negotiating,
    /// Agreed, and data may flow at this rate.
    Connected(Rate),
    /// Nothing recognisable arrived in time.
    Failed,
}

/// The timings of 6.3.1.1, in seconds.
///
/// Each is the middle of the range the recommendation gives, since a modem has
/// to land inside the range rather than on an edge of it.
mod timing {
    /// Answering tone, V.25 2.2 note 1: 3.3 s plus or minus 0.7.
    pub const ANSWER_TONE: f64 = 3.3;
    /// Unscrambled binary 1 heard before the calling modem responds.
    pub const HEARD_UNSCRAMBLED: f64 = 0.155;
    /// Silence the calling modem keeps afterwards.
    pub const PAUSE: f64 = 0.456;
    /// The double dibit that offers 2400.
    pub const DOUBLE_DIBIT: f64 = 0.100;
    /// Scrambled binary 1 heard before a 1200 connection is agreed.
    pub const HEARD_SCRAMBLED: f64 = 0.270;
    /// After the rate is agreed, when to start sending at 2400.
    pub const TO_2400: f64 = 0.600;
    /// And how long to send it before data may follow.
    pub const SETTLE_2400: f64 = 0.200;
    /// Nothing recognisable for this long and the attempt is abandoned. The
    /// recommendation sets no such limit; a modem that waits for ever is no
    /// use to the thing waiting on it.
    pub const PATIENCE: f64 = 60.0;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Answering: sending the V.25 answering tone.
    AnswerTone,
    /// Answering: sending unscrambled binary 1 and waiting to hear back
    /// (6.3.1.1.2 a).
    OfferingOnes,
    /// Calling: silent, waiting to hear unscrambled binary 1 (6.3.1.1.1 a).
    Listening,
    /// Calling: silent for the 456 ms that follows (6.3.1.1.1 b).
    Pausing,
    /// Sending the double dibit that offers 2400 (6.3.1.1.1 b, 6.3.1.1.2 b).
    OfferingDoubleDibit,
    /// Sending scrambled binary 1 at 1200, which either settles the connection
    /// there or is overtaken by the far end offering 2400.
    Scrambled1200,
    /// The rate is agreed at 2400 and the clock is running towards it.
    Rising2400,
    /// Sending scrambled binary 1 at 2400 and letting the far end settle.
    Settling2400,
    Connected(Rate),
    Failed,
}

/// Runs one end of the handshake.
#[derive(Debug)]
pub struct Handshake {
    role: Role,
    state: State,
    /// Seconds in the current state.
    elapsed: f64,
    /// Seconds since the handshake began, for the overall limit.
    total: f64,
    /// One sample, in seconds.
    step: f64,
    /// Seconds the pattern being waited for has been present without a break.
    held: f64,
    /// Whether the far end has offered 2400. It may do so while this end is
    /// still sending its own offer, so the fact has to be remembered rather
    /// than merely acted on.
    offered_2400: bool,
}

impl Handshake {
    pub fn new(role: Role, fs: f64) -> Self {
        Self {
            role,
            state: match role {
                Role::Calling => State::Listening,
                Role::Answering => State::AnswerTone,
            },
            elapsed: 0.0,
            total: 0.0,
            step: 1.0 / fs,
            held: 0.0,
            offered_2400: false,
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

    /// Advance one sample: read what is arriving, decide what to send.
    ///
    /// The receiver is taken by mutable reference because the rate is
    /// something the handshake *knows*: it negotiated it. Telling the receiver
    /// saves it working the same thing out from the shape of the
    /// constellation, which it can do but more slowly and less certainly.
    pub fn step(&mut self, tx: &mut Transmitter, rx: &mut Receiver) -> Status {
        self.elapsed += self.step;
        self.total += self.step;
        let heard = rx.pattern();

        match self.state {
            State::AnswerTone => {
                tx.set_signal(Signal::AnswerTone);
                if self.elapsed >= timing::ANSWER_TONE {
                    self.enter(State::OfferingOnes);
                }
            }

            State::OfferingOnes => {
                // 6.3.1.1.2 a): unscrambled binary 1 at 1200, and listen.
                tx.set_rate(Rate::Bps1200);
                tx.set_signal(Signal::UnscrambledOnes);
                if heard == Pattern::DoubleDibit {
                    // 6.3.1.1.2 b): the far end can do 2400. Answer in kind.
                    self.offered_2400 = true;
                    self.enter(State::OfferingDoubleDibit);
                } else if self.hold(heard == Pattern::ScrambledOnes) >= timing::HEARD_SCRAMBLED {
                    // Scrambled ones and never the double dibit: a V.22 modem,
                    // so the connection settles at 1200.
                    self.settle(rx, Rate::Bps1200);
                }
            }

            State::Listening => {
                // 6.3.1.1.1 a): say nothing at all until the far end does.
                tx.set_signal(Signal::Silent);
                if self.hold(heard == Pattern::UnscrambledOnes) >= timing::HEARD_UNSCRAMBLED {
                    self.enter(State::Pausing);
                }
            }

            State::Pausing => {
                // 6.3.1.1.1 b): a further 456 ms of silence before answering.
                tx.set_signal(Signal::Silent);
                if self.elapsed >= timing::PAUSE {
                    self.enter(State::OfferingDoubleDibit);
                }
            }

            State::OfferingDoubleDibit => {
                tx.set_rate(Rate::Bps1200);
                tx.set_signal(Signal::DoubleDibit);
                if heard == Pattern::DoubleDibit {
                    self.offered_2400 = true;
                }
                if self.elapsed >= timing::DOUBLE_DIBIT {
                    self.enter(State::Scrambled1200);
                }
            }

            State::Scrambled1200 => {
                tx.set_rate(Rate::Bps1200);
                tx.set_signal(Signal::ScrambledOnes);
                if heard == Pattern::DoubleDibit {
                    self.offered_2400 = true;
                }
                if self.offered_2400 {
                    self.enter(State::Rising2400);
                } else if self.hold(heard == Pattern::ScrambledOnes) >= timing::HEARD_SCRAMBLED {
                    self.settle(rx, Rate::Bps1200);
                }
            }

            State::Rising2400 => {
                // 6.3.1.1.1 d): the far end is still reading 1200 for another
                // 600 ms, so keep sending what it can read until then.
                if self.elapsed >= timing::TO_2400 {
                    tx.set_rate(Rate::Bps2400);
                    rx.set_rate(Rate::Bps2400);
                    self.enter(State::Settling2400);
                }
            }

            State::Settling2400 => {
                tx.set_signal(Signal::ScrambledOnes);
                if self.elapsed >= timing::SETTLE_2400 {
                    self.settle(rx, Rate::Bps2400);
                }
            }

            State::Connected(_) | State::Failed => {}
        }

        if !matches!(self.state, State::Connected(_) | State::Failed)
            && self.total >= timing::PATIENCE
        {
            self.state = State::Failed;
        }
        self.status()
    }

    /// Seconds the pattern currently being waited for has held unbroken.
    ///
    /// Every wait in the handshake is for a signal to be *continuously*
    /// present for a stated time, so a break starts the count again.
    fn hold(&mut self, present: bool) -> f64 {
        if present {
            self.held += self.step;
        } else {
            self.held = 0.0;
        }
        self.held
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.elapsed = 0.0;
        self.held = 0.0;
    }

    fn settle(&mut self, rx: &mut Receiver, rate: Rate) {
        rx.set_rate(rate);
        self.enter(State::Connected(rate));
    }
}

/// One end of a V.22bis call: transmitter, receiver and handshake together.
///
/// The three have to be stepped in lockstep and in the right order, and there
/// is only one right order, so putting them behind one call removes a way of
/// getting it wrong. It also gives V.22bis the same shape as V.32, which
/// matters because what sits above them should not have to care which is in
/// use.
#[derive(Debug)]
pub struct Modem {
    tx: Transmitter,
    rx: Receiver,
    hs: Handshake,
}

impl Modem {
    pub fn new(role: Role, fs: f64) -> Self {
        // A channel is named for the end that uses it and says both what that
        // end transmits in and what it listens to: the calling modem transmits
        // low and receives high, the answering modem the reverse.
        let channel = match role {
            Role::Calling => Channel::Calling,
            Role::Answering => Channel::Answering,
        };
        Self {
            tx: Transmitter::at_rate(channel, Rate::Bps1200, fs),
            rx: Receiver::new(channel, fs),
            hs: Handshake::new(role, fs),
        }
    }

    /// Take one sample from the line and give back the one to put on it.
    pub fn step(&mut self, line: f64) -> f64 {
        self.rx.feed(line);
        self.hs.step(&mut self.tx, &mut self.rx);
        self.tx.next_sample()
    }

    pub fn status(&self) -> Status {
        self.hs.status()
    }

    pub fn rate(&self) -> Rate {
        self.rx.rate()
    }

    pub fn carrier(&self) -> bool {
        self.rx.carrier()
    }

    /// Bits recovered from the line.
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

    pub fn residual_error(&self) -> f64 {
        self.rx.residual_error()
    }
}
