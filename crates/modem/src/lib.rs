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
use datapump::AsyncBits;
use datapump::bell103;
use datapump::v22bis;
use datapump::v32;
use datapump::v8 as v8line;
use v8::{CallFunction, Modulation, Modulations};
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
    /// The terminal typed something while the call was being placed, which
    /// V.250 5.6.1 makes an instruction to give up on it.
    Aborted,
    /// The far end went away.
    CarrierLost,
    /// The handshake never completed.
    NoAnswer,
}

/// The line side, whichever modulation is in use.
///
/// The two are shaped alike on purpose: a sample in, a sample out, a status,
/// and bits either way. What sits above them has no business knowing which is
/// which, and the only place that decides is `+MS`.
#[derive(Debug)]
enum Pump {
    /// V.22bis: two directions in two halves of the band, 1200 or 2400 bit/s.
    V22bis(Box<v22bis::handshake::Modem>),
    /// V.32: both directions in the whole band at once, 4800 or 9600 bit/s,
    /// with the echo canceller that makes that possible.
    V32(Box<v32::startup::Modem>),
    /// Bell 103: 300 bit/s, two tones a direction, and nothing else at all.
    Bell103(Box<bell103::Modem>),
}

impl Pump {
    fn step(&mut self, line: f64) -> f64 {
        match self {
            Self::V22bis(m) => m.step(line),
            Self::V32(m) => m.step(line),
            Self::Bell103(m) => m.step(line),
        }
    }

    /// Whether the handshake is still going, has finished, or has given up.
    fn status(&self) -> Progress {
        match self {
            Self::V22bis(m) => match m.status() {
                v22bis::handshake::Status::Negotiating => Progress::Negotiating,
                v22bis::handshake::Status::Connected(r) => {
                    Progress::Connected(r.bits_per_second())
                }
                v22bis::handshake::Status::Failed => Progress::Failed,
            },
            Self::V32(m) => match m.status() {
                v32::startup::Status::Negotiating => Progress::Negotiating,
                v32::startup::Status::Connected(rate) => Progress::Connected(rate),
                v32::startup::Status::Failed => Progress::Failed,
            },
            Self::Bell103(m) => match m.status() {
                bell103::Status::Negotiating => Progress::Negotiating,
                bell103::Status::Connected(rate) => Progress::Connected(rate),
                bell103::Status::Failed => Progress::Failed,
            },
        }
    }

    fn carrier(&self) -> bool {
        match self {
            Self::V22bis(m) => m.carrier(),
            // V.32's start-up measures the line rather than watching a
            // carrier detector, but once connected the receiver has one and it
            // is the only thing that will notice the far end hanging up.
            // Answering `true` here meant a V.32 call never ended: the near end
            // went back to command state and the far end sat in data for ever,
            // waiting for a carrier that had gone before it started waiting.
            Self::V32(m) => m.carrier(),
            Self::Bell103(m) => m.carrier(),
        }
    }

    fn take_bits(&mut self) -> Vec<bool> {
        match self {
            Self::V22bis(m) => m.take_bits(),
            Self::V32(m) => m.take_bits(),
            Self::Bell103(m) => m.take_bits(),
        }
    }

    fn send_bits(&mut self, bits: &[bool]) {
        match self {
            Self::V22bis(m) => m.send_bits(bits),
            Self::V32(m) => m.send_bits(bits),
            Self::Bell103(m) => m.send_bits(bits),
        }
    }

    fn pending_bits(&self) -> usize {
        match self {
            Self::V22bis(m) => m.pending_bits(),
            Self::V32(m) => m.pending_bits(),
            Self::Bell103(m) => m.pending_bits(),
        }
    }

    /// The point the receiver last decided on, where the modulation has one.
    ///
    /// Frequency shift keying does not: what it decides is which of two tones
    /// arrived, and a scope for that is an eye rather than a constellation.
    fn constellation_point(&self) -> Option<(f64, f64)> {
        match self {
            Self::V22bis(m) => Some(m.constellation_point()),
            Self::V32(m) => Some(m.constellation_point()),
            Self::Bell103(_) => None,
        }
    }

    /// Discriminator output for the modulations whose scope is that eye, where
    /// `+1` is a mark and `-1` a space.
    fn discriminator(&self) -> Option<f64> {
        match self {
            Self::Bell103(m) => Some(m.level()),
            _ => None,
        }
    }

    /// Characters the line lost, where the pump is the one that frames them.
    fn line_framing_errors(&self) -> Option<u64> {
        match self {
            Self::Bell103(m) => Some(m.framing_errors()),
            // The synchronous pumps hand up a bit stream and have no idea
            // where a character begins, so the framing is done above them and
            // the count belongs there.
            _ => None,
        }
    }

    /// The discriminator reading at the centre of each recovered bit.
    fn take_symbol(&mut self) -> Option<f64> {
        match self {
            Self::Bell103(m) => m.take_symbol(),
            _ => None,
        }
    }

    /// Mean distance from the decisions being made, which is how well the
    /// receiver is doing.
    fn residual_error(&self) -> Option<f64> {
        match self {
            Self::V22bis(m) => Some(m.residual_error()),
            Self::V32(m) => Some(m.residual_error()),
            Self::Bell103(_) => None,
        }
    }

    /// How many states the modulation has, for a scope to size itself by.
    fn states(&self) -> usize {
        match self {
            Self::V22bis(m) => match m.status() {
                v22bis::handshake::Status::Connected(v22bis::Rate::Bps2400) => 16,
                _ => 4,
            },
            // Four during the whole start-up and at 4800; sixteen once the
            // rate exchange has settled on 9600 (2.4.1.1).
            Self::V32(m) => match m.status() {
                v32::startup::Status::Connected(9600) => 16,
                _ => 4,
            },
            Self::Bell103(_) => 2,
        }
    }

    /// Short name for the signal shape, as a faceplate would print it.
    fn shape(&self) -> &'static str {
        match self {
            Self::V22bis(_) | Self::V32(_) => match self.states() {
                16 => "16QAM",
                _ => "4PSK",
            },
            Self::Bell103(_) => "2FSK",
        }
    }

    /// The name of the modulation itself.
    fn standard(&self) -> &'static str {
        match self {
            Self::V22bis(_) => "V.22bis",
            Self::V32(_) => "V.32",
            Self::Bell103(_) => "Bell 103",
        }
    }

    /// Which step of the handshake the line is on, for anything that wants to
    /// show progress or work out where one stalled.
    fn phase(&self) -> &'static str {
        match self {
            // V.22bis negotiates by timing rather than by a sequence of named
            // steps, so there is nothing finer to report than whether it is
            // still going.
            Self::V22bis(m) => match m.status() {
                v22bis::handshake::Status::Negotiating => "negotiating",
                v22bis::handshake::Status::Connected(_) => "connected",
                v22bis::handshake::Status::Failed => "failed",
            },
            Self::V32(m) => m.phase(),
            Self::Bell103(m) => m.line_phase(),
        }
    }
}

/// How far a handshake has got, in terms neither modulation owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    Negotiating,
    Connected(u32),
    Failed,
}

/// How long after dialling a character is taken as an instruction to stop.
///
/// V.250 5.6.1: "characters transmitted during the first 125 milliseconds
/// after transmission of the termination character shall be ignored (to allow
/// for the DTE to append additional control characters such as line feed after
/// the command line termination character)".
const ABORT_GUARD_MS: u32 = 125;

/// One modem.
#[derive(Debug)]
pub struct Modem {
    at: Interpreter,
    escape: EscapeDetector,
    state: State,
    fs: f64,
    /// The line side, once off hook.
    pump: Option<Pump>,
    /// The rate the handshake settled on.
    rate: u32,
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
    /// Milliseconds since the call was placed, for the guard in V.250 5.6.1.
    since_dial_ms: u32,
    /// The V.8 negotiation, while one is running.
    ///
    /// It comes before the data pump and instead of it. Every modem
    /// Recommendation's start-up assumes both ends already agree which one is
    /// being followed, and nothing in any of them says so; V.8 is the
    /// conversation that settles it, and it has to finish before there is a
    /// pump to build.
    negotiation: Option<v8line::Modem>,
    /// Whether V.8 settled on LAPM before the data carriers went up.
    declared_lapm: bool,
    /// The rate of a connection the terminal has not been told about yet.
    ///
    /// V.250 6.5.5 puts the error control report "before the final result
    /// code", so the CONNECT cannot go out until the negotiation that report
    /// describes has finished. Nothing is lost by the wait: anything typed
    /// into the gap is queued, and the far end is not listening for it yet
    /// either.
    announce: Option<u32>,
}

impl Modem {
    pub fn new(fs: f64) -> Self {
        Self {
            at: Interpreter::new(),
            escape: EscapeDetector::new(),
            state: State::Command,
            fs,
            pump: None,
            rate: 0,
            ec: None,
            want_error_control: true,
            role: Role::Calling,
            outbound: Vec::new(),
            async_bits: AsyncBits::new(8),
            elapsed_samples: 0.0,
            since_dial_ms: 0,
            negotiation: None,
            declared_lapm: false,
            announce: None,
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
        (self.rate > 0).then_some(self.rate)
    }

    /// The modulation in use, by the name `+MS` knows it as.
    pub fn modulation(&self) -> &str {
        &self.at.modulation.carrier
    }

    /// Which step of the handshake the line is on.
    pub fn line_phase(&self) -> &'static str {
        if let Some(negotiation) = self.negotiation.as_ref() {
            return negotiation.phase();
        }
        self.pump.as_ref().map_or("on hook", Pump::phase)
    }

    /// The round trip the handshake measured, where it measures one.
    pub fn round_trip_symbols(&self) -> Option<u64> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => Some(m.round_trip()),
            _ => None,
        }
    }

    /// How much of its own echo the line is removing, in decibels.
    pub fn echo_return_loss(&self) -> Option<f64> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => Some(m.echo_return_loss()),
            _ => None,
        }
    }

    /// The point the receiver last decided on, for a constellation scope.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        self.pump.as_ref().and_then(Pump::constellation_point)
    }

    /// Discriminator output, for the modulations whose scope is an eye.
    pub fn discriminator(&self) -> Option<f64> {
        self.pump.as_ref().and_then(Pump::discriminator)
    }

    /// One discriminator reading per recovered bit, taken at the bit centre.
    pub fn take_symbol(&mut self) -> Option<f64> {
        self.pump.as_mut().and_then(Pump::take_symbol)
    }

    /// How far the received points are sitting from the decisions made about
    /// them, which is the one number that says whether a call is healthy.
    pub fn residual_error(&self) -> Option<f64> {
        self.pump.as_ref().and_then(Pump::residual_error)
    }

    /// How many states the modulation in use has.
    pub fn states(&self) -> usize {
        self.pump.as_ref().map_or(2, Pump::states)
    }

    /// Short name for the signal shape: "16QAM", "2FSK" and so on.
    pub fn shape(&self) -> &'static str {
        self.pump.as_ref().map_or("-", Pump::shape)
    }

    /// The modulation in use, or the one the next call will use.
    pub fn standard(&self) -> &'static str {
        if self.negotiation.is_some() {
            return "V.8";
        }
        match self.pump.as_ref() {
            Some(p) => p.standard(),
            None => match self.at.modulation.carrier.as_str() {
                "V32" => "V.32",
                "B103" => "Bell 103",
                _ => "V.22bis",
            },
        }
    }

    /// Whether the line is off hook, which is to say there is a call on it.
    pub fn off_hook(&self) -> bool {
        // A negotiation is a call too. It is the first thing on the line after
        // the far end picks up, and a front panel that showed the lamp out
        // until a pump existed would show it out for the loudest three seconds
        // of the call.
        self.pump.is_some() || self.negotiation.is_some()
    }

    /// Whether the far end's carrier is present.
    pub fn carrier(&self) -> bool {
        self.pump.as_ref().is_some_and(Pump::carrier)
    }

    /// Whether this end placed the call or took it.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Where the line puts our own signal back, if the handshake went looking.
    ///
    /// Worth reporting on a real line, because it is the one number that says
    /// whether the echo canceller is pointed at anything: taps placed where
    /// the reflection is not are taps modelling nothing.
    pub fn reflection(&self) -> Option<datapump::v32::startup::Reflection> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => m.reflection(),
            _ => None,
        }
    }

    /// Characters whose stop bit was not where it should have been.
    ///
    /// Only meaningful on a connection with no error control, which is the
    /// only kind that puts start-stop framing on the line. It is the cheapest
    /// measure of how a link is really doing, and more than that it says what
    /// *kind* of trouble it is in: errors from noise arrive evenly, a few a
    /// second, for as long as the noise lasts, while errors from a network
    /// that lost a packet arrive dozens at a time with nothing in between. The
    /// two want completely different answers and look identical in the text.
    pub fn framing_errors(&self) -> u64 {
        // Wherever the framing actually happens. Bell 103 finds characters on
        // the line itself and hands them up already framed, so asking the
        // layer above would always answer zero -- it is being handed a round
        // trip through bytes we recovered ourselves, which cannot fail.
        self.pump
            .as_ref()
            .and_then(Pump::line_framing_errors)
            .unwrap_or_else(|| self.async_bits.framing_errors())
    }

    /// Whether V.8 named LAPM before the data carriers went up.
    ///
    /// Not the same question as [`Modem::error_controlled`], which is about
    /// what is running now. This is about what both ends said they would do,
    /// at 300 bit/s, in the protocol category of V.8 Table 6.
    pub fn error_control_negotiated(&self) -> bool {
        self.declared_lapm
    }

    /// Whether error control is running on the current call.
    pub fn error_controlled(&self) -> bool {
        self.ec.as_ref().is_some_and(Stack::is_connected)
    }

    /// Frames that arrived and did not survive the line.
    ///
    /// The difference between a link that is working and one that is only
    /// apparently working. LAPM retransmits, so a call can be delivering every
    /// byte correctly and still be losing most of what is sent -- and the
    /// terminal, which sees only the bytes, cannot tell.
    pub fn damaged_frames(&self) -> u64 {
        self.ec.as_ref().map_or(0, Stack::damaged_frames)
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
                // V.250 5.6.1, and the abortability clause of the D command:
                // a single character from the terminal while a call is being
                // placed is an instruction to give up on it, and the modem
                // "disconnects from the line in an orderly manner".
                //
                // Not for the first eighth of a second, though. The character
                // that ended the command line is very often followed by a line
                // feed, and a terminal that appended one would otherwise be
                // hanging up on itself the instant it dialled.
                if self.since_dial_ms >= ABORT_GUARD_MS {
                    self.end_call(Ended::Aborted);
                }
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

        if self.negotiation.is_some() {
            return self.negotiate(line);
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
        if self.state == State::Handshaking {
            self.since_dial_ms = self.since_dial_ms.saturating_add(ms);
        }
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
        // The pump connecting is not the handshake ending. The detection phase
        // and the XID exchange run on top of it, and V.250 6.5.5 has the
        // terminal told what was negotiated before it is told it has connected
        // -- so until the CONNECT goes out this is still handshaking. Which is
        // also the honest answer to what a character typed into that gap
        // means: 5.6.1's instruction to give up on the call, because from the
        // terminal's side there is not yet a call.
        if self.announce.is_some() {
            self.carry_data();
            return;
        }
        let Some(pump) = self.pump.as_ref() else { return };
        match pump.status() {
            Progress::Negotiating => {}
            Progress::Connected(rate) => {
                self.rate = rate;
                // Bell 103 is asynchronous all the way down: its line format
                // *is* start-stop framing, and its receiver finds the frames
                // by re-synchronising on each start bit rather than by holding
                // a bit clock. V.42 wants a synchronous bit pipe underneath it
                // and would hand this one HDLC, which the framer would take
                // apart into characters that were never there. So error
                // control is off at 300 bit/s -- which is also how anyone ever
                // dialled a board at 300 bit/s.
                let framed = matches!(pump, Pump::Bell103(_));
                if self.want_error_control && !framed {
                    let role = match self.role {
                        Role::Calling => EcRole::Originator,
                        Role::Answering => EcRole::Answerer,
                    };
                    let mut stack = Stack::new(role, Params::default());
                    if self.declared_lapm {
                        stack = stack.declared_lapm();
                    }
                    // Offer compression in both directions and let the far end
                    // decide. What runs is the intersection, so offering more
                    // than the far end can do costs nothing.
                    if self.at.compression {
                        stack.offer_compression(Compression::Both);
                    }
                    self.ec = Some(stack);
                }
                // Held rather than sent. What goes out first is the report
                // of what was negotiated, and that is not known yet.
                self.announce = Some(rate);
                self.announce_connect();
            }
            Progress::Failed => self.end_call(Ended::NoAnswer),
        }
    }

    /// Tell the terminal the call is up, once there is nothing left to say
    /// about it.
    ///
    /// V.250 6.5.5: the `+ER` report is issued "at the point during error
    /// control negotiation (handshaking) at which the DCE has determined which
    /// error control protocol will be used (if any), before the final result
    /// code (e.g., CONNECT) is transmitted", and 6.6.3 puts `+DR` between the
    /// two. So the order is fixed and the CONNECT is last, which means it
    /// cannot go out while the answer is still being worked out.
    fn announce_connect(&mut self) {
        let Some(rate) = self.announce else { return };
        // No stack at all is an answer: this is a call without error control,
        // and there is nothing to wait for.
        if self.ec.as_ref().is_some_and(|e| !e.settled()) {
            return;
        }
        self.announce = None;
        self.state = State::Data;
        self.escape.reset();

        // Table 24/V.250. `ALT` is for the alternative protocol of Annex A,
        // which this modem does not do, so the report is between two.
        if self.at.config.report_error_control {
            let kind = if self.error_controlled() { "LAPM" } else { "NONE" };
            self.at.emit(ResultCode::Extended(format!("+ER: {kind}")));
        }
        // Table 29/V.250. V.42bis is negotiated as a pair here -- both
        // directions or neither -- so the one-directional reports cannot
        // arise.
        if self.at.config.report_compression {
            let kind = if self.compressing() { "V42B" } else { "NONE" };
            self.at.emit(ResultCode::Extended(format!("+DR: {kind}")));
        }
        // V.250 6.2.7: with X at 1 or above the CONNECT carries the rate,
        // which is the only way a terminal finds out what it got rather than
        // what it asked for.
        let code = if self.at.config.x == 0 {
            ResultCode::Connect
        } else {
            ResultCode::ConnectText(format!("{rate}"))
        };
        self.at.emit(code);
    }

    fn carry_data(&mut self) {
        let Some(pump) = self.pump.as_ref() else { return };
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
        self.announce_connect();

        let Some(pump) = self.pump.as_mut() else { return };
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
                Action::SelectModulation(_) | Action::SelectCompression(_) => {
                    // Both take effect on the next call, so there is nothing
                    // to do now beyond acknowledging: the interpreter has
                    // already recorded what was asked for.
                    self.at.emit(ResultCode::Ok);
                }
                Action::SelectErrorControl(e) => {
                    self.want_error_control = e.wanted();
                    self.at.emit(ResultCode::Ok);
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
        self.since_dial_ms = 0;
        self.rate = 0;
        self.ec = None;
        self.declared_lapm = false;
        self.announce = None;
        self.outbound.clear();
        self.async_bits.reset();
        self.state = State::Handshaking;

        // V.250 6.4.1's automode: the modem "may fall back to another
        // modulation on its own". V.8 is how two modems do that on purpose
        // rather than by each guessing and hoping, so that is what automode
        // means here. With it off, the modulation named is the modulation
        // used and there is nothing to negotiate.
        let offered = self.offered();
        if self.at.modulation.automode && !offered.is_empty() {
            let role = match role {
                Role::Calling => v8line::Role::Calling,
                Role::Answering => v8line::Role::Answering,
            };
            self.pump = None;
            let mut negotiation =
                v8line::Modem::new(role, CallFunction::Data, offered, self.fs);
            // V.8 Table 6 has an octet for error control, and 7.3 says it is
            // there "in order to negotiate LAPM without requiring the ODP/ADP
            // exchange". Asking costs one octet in a sequence already being
            // sent, and what comes back is a second opinion on the question
            // the detection phase is about to ask over a much worse channel.
            if self.want_error_control {
                negotiation = negotiation.offering_lapm();
            }
            self.negotiation = Some(negotiation);
            return;
        }
        self.start_pump(None);
    }

    /// What to put in a call menu.
    ///
    /// Only what this modem can actually demodulate, and only inside the range
    /// `+MS` asked for -- offering a modulation and then failing to hold it is
    /// worse than never offering it, and the whole value of the exchange is
    /// that what comes back can be believed.
    ///
    /// Bell 103 is not in the list and cannot be: Table 4 of V.8 is a table of
    /// V-series modulations and Bell 103 is not one of them. A modem told to
    /// use it is therefore told something V.8 has no way to express, and the
    /// honest answer is not to negotiate at all.
    fn offered(&self) -> Modulations {
        let (lowest, highest) = self.rate_range();
        let settings = &self.at.modulation;
        let Some(preferred) = (match settings.carrier.as_str() {
            "V32" => Some(Modulation::V32bis),
            "B103" => None,
            _ => Some(Modulation::V22bis),
        }) else {
            return Modulations::NONE;
        };
        let mut offered = Modulations::NONE;
        offered.insert(preferred);
        // Everything else this modem has, within the rates asked for. V.32
        // starts at 4800 and V.22bis spans 1200 to 2400, so a ceiling of 1200
        // rules the first out entirely rather than merely discouraging it.
        if highest >= 4800 {
            offered.insert(Modulation::V32bis);
        }
        if highest >= 1200 && lowest <= 2400 {
            offered.insert(Modulation::V22bis);
        }
        offered
    }

    /// The rate range `+MS` asked for, with V.250's "unspecified" resolved.
    ///
    /// 6.4.1 makes zero mean no limit rather than a rate of nothing, so every
    /// comparison wants it turned into the limit it stands for first.
    fn rate_range(&self) -> (u32, u32) {
        let settings = &self.at.modulation;
        let highest = if settings.max_rate == 0 { u32::MAX } else { settings.max_rate };
        (settings.min_rate, highest)
    }

    /// Carry the negotiation one sample further, and build the pump when it
    /// has decided.
    fn negotiate(&mut self, line: f64) -> f64 {
        let negotiation = self.negotiation.as_mut().expect("checked by the caller");
        let out = negotiation.step(line);
        match negotiation.status() {
            v8line::Status::Negotiating => {}
            v8line::Status::Agreed(modulation) => {
                self.declared_lapm = negotiation.lapm();
                self.negotiation = None;
                self.start_pump(Some(modulation));
            }
            // 8.1.1: a far end that sent the plain answering tone of V.25 does
            // not speak V.8, and the call goes on "in accordance with Annex
            // A/V.32 bis, ITU-T T.30, or other appropriate Recommendations" --
            // which here means the modulation +MS named, exactly as before any
            // of this existed.
            v8line::Status::NoNegotiation => {
                self.negotiation = None;
                self.start_pump(None);
            }
            // Nothing in common, or nothing heard. Both ends know, which is
            // the difference between this and a minute of silence.
            v8line::Status::Failed => {
                self.negotiation = None;
                self.end_call(Ended::NoAnswer);
            }
        }
        out
    }

    /// Build the data pump and let its own start-up begin.
    ///
    /// `chosen` is what V.8 agreed, where it ran. Without one the modulation
    /// is whichever `+MS` named.
    fn start_pump(&mut self, chosen: Option<Modulation>) {
        let role = self.role;
        let carrier = match chosen {
            Some(Modulation::V32bis) => "V32".to_owned(),
            Some(Modulation::V22bis) => "V22B".to_owned(),
            // Nothing else is ever offered, so nothing else can come back.
            _ => self.at.modulation.carrier.clone(),
        };
        self.pump = Some(match carrier.as_str() {
            "V32" => {
                let hs_role = match role {
                    Role::Calling => v32::startup::Role::Calling,
                    Role::Answering => v32::startup::Role::Answering,
                };
                // Offer what this receiver can actually demodulate, and no
                // more: offering a rate and then failing to read it is worse
                // than never offering it. Both of these are read here --
                // 4800 by 2.4.2 and 9600 by the nonredundant coding of
                // 2.4.1.1, which is the alternative every V.32 modem is
                // required to be able to fall back on. Trellis coding is not,
                // so B8 of the rate signal stays clear.
                //
                // And only within the range +MS allows. <max_rate> is "the
                // highest value at which the DCE may establish a connection",
                // which is not advice: a modem that offers 9600 to a terminal
                // that asked for at most 4800 will get 9600, because the far
                // end has no way to know it was not meant.
                let (lowest, highest) = self.rate_range();
                let offer = v32::startup::rate_signal(
                    highest >= 4800 && lowest <= 4800,
                    highest >= 9600,
                );
                Pump::V32(Box::new(v32::startup::Modem::new(hs_role, offer, self.fs)))
            }
            "B103" => {
                let hs_role = match role {
                    Role::Calling => bell103::Role::Originate,
                    Role::Answering => bell103::Role::Answer,
                };
                Pump::Bell103(Box::new(bell103::Modem::new(hs_role, self.fs)))
            }
            _ => {
                let hs_role = match role {
                    Role::Calling => v22bis::handshake::Role::Calling,
                    Role::Answering => v22bis::handshake::Role::Answering,
                };
                // +MS carries a maximum rate and it is not decoration. The
                // sixteen points of 2400 need about 20 dB of signal to noise
                // to be told apart and the four of 1200 need about 13, so on
                // a line that cannot give the first, 2400 is not the faster
                // connection but the one that carries nothing.
                let ceiling = if self.rate_range().1 >= 2400 {
                    v22bis::Rate::Bps2400
                } else {
                    v22bis::Rate::Bps1200
                };
                Pump::V22bis(Box::new(v22bis::handshake::Modem::at_most(
                    hs_role, ceiling, self.fs,
                )))
            }
        });
        self.state = State::Handshaking;
    }

    fn end_call(&mut self, why: Ended) {
        self.pump = None;
        self.negotiation = None;
        self.rate = 0;
        self.ec = None;
        // A call that ends before its CONNECT went out never connected, and
        // the terminal is about to be told why instead.
        self.announce = None;
        self.outbound.clear();
        self.escape.reset();
        self.state = State::Command;
        self.at.emit(match why {
            Ended::LocalRequest => ResultCode::Ok,
            // 6.3.1: what a dial that did not get there reports.
            Ended::Aborted => ResultCode::NoCarrier,
            Ended::CarrierLost => ResultCode::NoCarrier,
            Ended::NoAnswer => ResultCode::NoAnswer,
        });
    }
}

/// Scale a recording so that it fits, and report what it was scaled by.
///
/// A modem's output is not bounded by one. The pulse shaping sums the tails of
/// several symbols, so the peak runs well above the average, and two modems on
/// one pair sum again on top of that: a V.22bis call between two of these
/// reaches about one and a half. Written to a sixteen-bit file as it stands,
/// every one of those peaks comes back clipped, and every measurement made
/// from the file afterwards is of something else.
///
/// The level is not information. A real line delivers whatever it delivers,
/// which is why a receiver has gain control at all, so scaling a recording to
/// fit loses nothing that was in it.
pub fn fit_to_scale(samples: &mut [f32], target: f32) -> f32 {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak <= 0.0 {
        return 1.0;
    }
    let gain = target / peak;
    if gain >= 1.0 {
        return 1.0;
    }
    for s in samples.iter_mut() {
        *s *= gain;
    }
    gain
}
