//! A modem: the whole of one, from `ATD` to samples on the line.
//!
//! Everything below this has been built and tested on its own. A data pump
//! turns bits into a waveform and back; a handshake gets two of them to agree;
//! V.42 makes the resulting bit stream reliable; an AT interpreter turns what
//! a terminal types into requests. None of that is a modem until something
//! decides when each of them applies, which is what this does.
//!
//! The states are V.250's, and there are only four that matter. In *command*
//! state the terminal is talking to the modem and what it types is parsed. In
//! *handshaking* the line is up but the two ends have not agreed anything yet,
//! and the terminal is told nothing until they do. In *data* state everything
//! the terminal types goes down the line and everything arriving comes back
//! up, and nothing is parsed at all. *Online command* state is the odd one:
//! the connection is still there but the terminal has escaped back to talking
//! to the modem, which is how a caller hangs up without dropping carrier
//! first.
//!
//! The escape is the well-known three plusses, and the reason it needs a
//! second of quiet either side is that otherwise a file containing them would
//! drop the call carrying it.

use at::escape::EscapeDetector;
use at::result::ResultCode;
use at::{Action, Interpreter};
use datapump::v22bis;
use datapump::AsyncBits;
use ec::stack::Phase;
use ec::xid::Compression;
use ec::{Params, Role as EcRole, Stack};

/// Which end of the call this modem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Calling,
    Answering,
}

/// What the modem is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// On hook. The line is not connected and nothing is transmitted.
    Command,
    /// Off hook, negotiating. V.250 has the terminal hear nothing until this
    /// ends, one way or the other.
    Handshaking,
    /// Connected, and everything the terminal types goes down the line.
    Data,
    /// Connected, but the terminal has escaped back to talking to the modem.
    OnlineCommand,
}

/// Why a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The terminal asked, with `ATH`.
    LocalRequest,
    /// The far end went away.
    CarrierLost,
    /// The handshake never completed.
    NoAnswer,
}

/// One modem.
#[derive(Debug)]
pub struct Modem {
    at: Interpreter,
    escape: EscapeDetector,
    state: State,
    fs: f64,
    /// The line side, once off hook.
    pump: Option<v22bis::handshake::Modem>,
    /// Error control over it, once connected.
    ec: Option<Stack>,
    /// Whether error control is wanted at all. Without it the connection is
    /// still perfectly usable and simply has no protection.
    want_error_control: bool,
    role: Role,
    /// Bytes waiting to go down the line, held while the link comes up.
    outbound: Vec<u8>,
    /// Start-stop framing for a connection without error control, where
    /// nothing else says where one character ends and the next begins.
    async_bits: AsyncBits,
    /// Milliseconds since the last tick, accumulated from samples.
    elapsed_samples: f64,
}

impl Modem {
    pub fn new(fs: f64) -> Self {
        Self {
            at: Interpreter::new(),
            escape: EscapeDetector::new(),
            state: State::Command,
            fs,
            pump: None,
            ec: None,
            want_error_control: true,
            role: Role::Calling,
            outbound: Vec::new(),
            async_bits: AsyncBits::new(8),
            elapsed_samples: 0.0,
        }
    }

    /// Whether to attempt V.42 on the next call.
    pub fn set_error_control(&mut self, on: bool) {
        self.want_error_control = on;
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn is_online(&self) -> bool {
        matches!(self.state, State::Data | State::OnlineCommand)
    }

    /// The rate agreed, once there is a connection.
    pub fn rate(&self) -> Option<u32> {
        self.pump.as_ref().map(|p| p.rate().bits_per_second())
    }

    /// Whether error control is running on the current call.
    pub fn error_controlled(&self) -> bool {
        self.ec.as_ref().is_some_and(Stack::is_connected)
    }

    /// Whether V.42bis was agreed, which needs both ends to have offered it.
    pub fn compressing(&self) -> bool {
        self.ec.as_ref().is_some_and(Stack::compressing)
    }

    /// Bytes for the terminal.
    pub fn take_dte(&mut self) -> Vec<u8> {
        let mut out = self.at.take_output();
        if self.state == State::Data {
            match self.ec.as_mut() {
                Some(ec) => out.extend(ec.take_received()),
                None => {
                    // Without error control there are no frames, so the
                    // characters are found by their own start and stop bits.
                    if let Some(pump) = self.pump.as_mut() {
                        for bit in pump.take_bits() {
                            if let Some(c) = self.async_bits.feed(bit) {
                                out.push(c);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// One byte typed by the terminal.
    pub fn feed_dte(&mut self, byte: u8) {
        match self.state {
            State::Command | State::OnlineCommand => {
                self.at.feed(byte);
                self.run_actions();
            }
            State::Handshaking => {
                // V.250 6.3.1: what arrives while a call is being placed is
                // not a command and is not data either. Dropping it is right;
                // holding it would deliver a burst of stale typing the moment
                // the connection came up.
            }
            State::Data => {
                // The escape detector sees everything, because the sequence
                // that returns to command state is made of ordinary data.
                self.escape.data(byte, &self.at.regs);
                self.outbound.push(byte);
            }
        }
    }

    /// Advance the line by one sample, returning the sample to transmit.
    pub fn step(&mut self, line: f64) -> f64 {
        self.elapsed_samples += 1.0;
        let ms = 1000.0 / self.fs;
        if self.elapsed_samples * ms >= 1.0 {
            let whole = (self.elapsed_samples * ms) as u32;
            self.elapsed_samples -= f64::from(whole) / ms;
            self.tick(whole);
        }

        let Some(pump) = self.pump.as_mut() else {
            return 0.0;
        };
        let out = pump.step(line);

        match self.state {
            State::Handshaking => self.advance_handshake(),
            State::Data | State::OnlineCommand => self.carry_data(),
            State::Command => {}
        }
        out
    }

    /// Time passing, which drives the escape guard and V.42's timers.
    pub fn tick(&mut self, ms: u32) {
        if let Some(ec) = self.ec.as_mut() {
            ec.tick(ms);
        }
        if self.state == State::Data && self.escape.idle(ms, &self.at.regs) {
            // V.250 6.1.4: the sequence is only an escape if it is surrounded
            // by quiet, which is what keeps a file containing three plusses
            // from dropping the call carrying it.
            self.state = State::OnlineCommand;
            self.at.emit(ResultCode::Ok);
        }
    }

    fn advance_handshake(&mut self) {
        let Some(pump) = self.pump.as_ref() else { return };
        match pump.status() {
            v22bis::handshake::Status::Negotiating => {}
            v22bis::handshake::Status::Connected(rate) => {
                self.state = State::Data;
                self.escape.reset();
                if self.want_error_control {
                    let role = match self.role {
                        Role::Calling => EcRole::Originator,
                        Role::Answering => EcRole::Answerer,
                    };
                    let mut stack = Stack::new(role, Params::default());
                    // Offer compression in both directions and let the far end
                    // decide. What runs is the intersection, so offering more
                    // than the far end can do costs nothing.
                    stack.offer_compression(Compression::Both);
                    self.ec = Some(stack);
                }
                // V.250 6.2.7: with X at 1 or above the CONNECT carries the
                // rate, which is the only way a terminal finds out what it
                // got rather than what it asked for.
                let code = if self.at.config.x == 0 {
                    ResultCode::Connect
                } else {
                    ResultCode::ConnectText(format!("{}", rate.bits_per_second()))
                };
                self.at.emit(code);
            }
            v22bis::handshake::Status::Failed => self.end_call(Ended::NoAnswer),
        }
    }

    fn carry_data(&mut self) {
        let Some(pump) = self.pump.as_mut() else { return };
        if !pump.carrier() {
            self.end_call(Ended::CarrierLost);
            return;
        }
        // A far end that does not do error control is a perfectly ordinary far
        // end, and V.42 7.2.1 exists to find that out rather than to fail on
        // it. Once the detection phase has said so there is nothing for the
        // stack to do, and the characters go down the line as they are.
        if self.ec.as_ref().is_some_and(|e| e.phase() == Phase::Transparent) {
            self.ec = None;
        }
        match self.ec.as_mut() {
            Some(ec) => {
                // Error control owns the bit stream in both directions: it
                // frames what goes out and unframes what comes back, and the
                // line is never idle because a synchronous link always carries
                // something.
                for bit in pump.take_bits() {
                    ec.feed_bit(bit);
                }
                if !self.outbound.is_empty() && ec.is_connected() {
                    let queued = std::mem::take(&mut self.outbound);
                    ec.send(&queued);
                }
                // Keep the transmitter fed. Running it dry would put the
                // pump's own idle pattern on the line in the middle of a
                // frame, which the far end would read as an abort.
                while pump.pending_bits() < 64 {
                    let bit = ec.next_bit();
                    pump.send_bits(&[bit]);
                }
            }
            None => {
                // Each character wrapped in a start and a stop bit (V.14), so
                // that the far end can find where it begins. A synchronous
                // line carries bits whether or not anything is sending, and
                // nothing else in an unprotected connection marks the
                // boundaries.
                if !self.outbound.is_empty() {
                    let queued = std::mem::take(&mut self.outbound);
                    let mut bits = Vec::new();
                    for byte in queued {
                        bits.extend(self.async_bits.encode(byte));
                    }
                    pump.send_bits(&bits);
                }
            }
        }
    }

    fn run_actions(&mut self) {
        for action in self.at.take_actions() {
            match action {
                Action::Dial(_) => self.place_call(Role::Calling),
                Action::Answer => self.place_call(Role::Answering),
                Action::HangUp => {
                    if self.pump.is_some() {
                        self.end_call(Ended::LocalRequest);
                    } else {
                        self.at.emit(ResultCode::Ok);
                    }
                }
                Action::OffHook => self.at.emit(ResultCode::Ok),
                Action::ReturnOnline => {
                    if self.state == State::OnlineCommand {
                        self.state = State::Data;
                        self.escape.reset();
                        self.at.emit(ResultCode::Connect);
                    } else {
                        // V.250 6.3.7: there is nothing to return to.
                        self.at.emit(ResultCode::Error);
                    }
                }
                Action::ResetProfile(_) | Action::FactoryDefaults(_) => {
                    if self.pump.is_some() {
                        self.end_call(Ended::LocalRequest);
                    }
                    self.at.emit(ResultCode::Ok);
                }
            }
        }
    }

    fn place_call(&mut self, role: Role) {
        self.role = role;
        let hs_role = match role {
            Role::Calling => v22bis::handshake::Role::Calling,
            Role::Answering => v22bis::handshake::Role::Answering,
        };
        self.pump = Some(v22bis::handshake::Modem::new(hs_role, self.fs));
        self.ec = None;
        self.outbound.clear();
        self.state = State::Handshaking;
    }

    fn end_call(&mut self, why: Ended) {
        self.pump = None;
        self.ec = None;
        self.outbound.clear();
        self.escape.reset();
        self.state = State::Command;
        self.at.emit(match why {
            Ended::LocalRequest => ResultCode::Ok,
            Ended::CarrierLost => ResultCode::NoCarrier,
            Ended::NoAnswer => ResultCode::NoAnswer,
        });
    }
}
