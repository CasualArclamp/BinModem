//! Bell 103 — 300 bps full-duplex FSK.
//!
//! Full duplex is achieved by frequency division, so the two directions occupy
//! separate bands and no echo canceller is required:
//!
//! | direction | space | mark |
//! |---|---|---|
//! | originating | 1070 Hz | 1270 Hz |
//! | answering | 2025 Hz | 2225 Hz |
//!
//! Bell 103 is the North American 300 bps standard; ITU-T V.21 is the
//! equivalent elsewhere and differs only in tone assignment (980/1180 and
//! 1650/1850), so the same receiver and transmitter serve both once given the
//! other pair of tones.
//!
//! There is almost nothing else to it, and that is why it is worth having.
//! Separate bands mean no echo canceller. A discriminator means no carrier
//! recovery. Start-stop framing that re-acquires on every character means no
//! timing loop to hold still. What is left is two tones and a moment's
//! patience, which is all it ever took to reach a bulletin board.

use dsp::FskDetector;

use crate::framing::{AsyncBits, AsyncFramer};

/// Which end of the call this modem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Originate,
    Answer,
}

impl Role {
    /// The band this role transmits in, as `(space, mark)`.
    pub fn transmit_tones(self) -> (f64, f64) {
        match self {
            Role::Originate => (1070.0, 1270.0),
            Role::Answer => (2025.0, 2225.0),
        }
    }

    /// The band this role listens to: whatever the far end transmits.
    pub fn receive_tones(self) -> (f64, f64) {
        match self {
            Role::Originate => Role::Answer.transmit_tones(),
            Role::Answer => Role::Originate.transmit_tones(),
        }
    }
}

pub const BAUD: f64 = 300.0;

/// A streaming Bell 103 receiver: line samples in, characters out.
#[derive(Debug)]
pub struct Bell103Rx {
    detector: FskDetector,
    framer: AsyncFramer,
    last_level: f64,
    symbol: Option<f64>,
}

impl Bell103Rx {
    /// Build a receiver listening to the band the far end of `role` transmits in.
    pub fn new(role: Role, fs: f64) -> Self {
        let (space, mark) = role.receive_tones();
        Self::with_tones(space, mark, fs)
    }

    /// Build a receiver for an explicit tone pair, for V.21 or for tapping a
    /// specific direction out of a 2-wire capture.
    pub fn with_tones(space: f64, mark: f64, fs: f64) -> Self {
        Self {
            detector: FskDetector::new(space, mark, BAUD, fs),
            framer: AsyncFramer::new(BAUD, fs, 8),
            last_level: 0.0,
            symbol: None,
        }
    }

    /// Feed one line sample; yields a character when a frame completes.
    #[inline]
    pub fn feed(&mut self, sample: f64) -> Option<u8> {
        let level = self.detector.feed(sample);
        self.last_level = level;
        let carrier = self.detector.carrier();
        let out = self.framer.feed(level, carrier);
        self.symbol = self.framer.take_sampled();
        out
    }

    /// Discriminator level of the bit sampled on the last `feed`, if any.
    ///
    /// One value per recovered bit, taken at the bit centre. This is what the
    /// symbol scope plots: distance from zero is the slicer's decision margin.
    pub fn take_symbol(&mut self) -> Option<f64> {
        self.symbol.take()
    }

    pub fn carrier(&self) -> bool {
        self.detector.carrier()
    }

    /// Most recent discriminator output: `+1` is a mark, `-1` a space.
    ///
    /// This is what an eye diagram is drawn from, and the only meaningful scope
    /// for FSK — there is no constellation to plot.
    pub fn level(&self) -> f64 {
        self.last_level
    }

    /// Received signal envelope in this band, for a level meter.
    pub fn amplitude(&self) -> f64 {
        self.detector.level()
    }

    pub fn framing_errors(&self) -> u64 {
        self.framer.framing_errors
    }
}


/// A streaming Bell 103 transmitter: bits in, line samples out.
///
/// Continuous-phase frequency shift keying. The phase carries across every
/// change of tone rather than restarting on it, and that is the whole of the
/// design: a phase discontinuity is a step, a step has energy at every
/// frequency, and the band next door belongs to the other direction. Two
/// oscillators switched between would splatter straight into it.
///
/// The line idles at mark, which is why an idle 300 bit/s modem is a steady
/// whistle rather than silence. It is also what makes async framing work at
/// all: a start bit is recognised as a departure from a mark that was already
/// there.
#[derive(Debug)]
pub struct Bell103Tx {
    space: f64,
    mark: f64,
    fs: f64,
    /// Carrier phase in turns, kept continuous across every shift.
    phase: f64,
    /// Samples left in the bit currently on the line.
    countdown: f64,
    sps: f64,
    pending: std::collections::VecDeque<bool>,
    /// The bit being sent. Idle is mark, so an empty queue still carries one.
    current: bool,
    transmitting: bool,
}

impl Bell103Tx {
    /// Build a transmitter for the band `role` sends in.
    pub fn new(role: Role, fs: f64) -> Self {
        let (space, mark) = role.transmit_tones();
        Self::with_tones(space, mark, fs)
    }

    pub fn with_tones(space: f64, mark: f64, fs: f64) -> Self {
        Self {
            space,
            mark,
            fs,
            phase: 0.0,
            countdown: fs / BAUD,
            sps: fs / BAUD,
            pending: std::collections::VecDeque::new(),
            current: true,
            transmitting: false,
        }
    }

    /// Whether to put anything on the line at all.
    ///
    /// A modem is silent until it is its turn: the calling end of a 300 bit/s
    /// call says nothing until it has heard the answering end, and a carrier
    /// raised early is one the far end has to decide about while it is still
    /// deciding about everything else.
    pub fn set_transmitting(&mut self, on: bool) {
        self.transmitting = on;
    }

    pub fn is_transmitting(&self) -> bool {
        self.transmitting
    }

    pub fn push_bits(&mut self, bits: &[bool]) {
        self.pending.extend(bits.iter().copied());
    }

    pub fn pending_bits(&self) -> usize {
        self.pending.len()
    }

    pub fn next_sample(&mut self) -> f64 {
        if !self.transmitting {
            return 0.0;
        }
        self.countdown -= 1.0;
        if self.countdown <= 0.0 {
            self.countdown += self.sps;
            self.current = self.pending.pop_front().unwrap_or(true);
        }
        let hz = if self.current { self.mark } else { self.space };
        self.phase += hz / self.fs;
        self.phase -= self.phase.floor();
        (self.phase * std::f64::consts::TAU).cos()
    }
}

/// How far a 300 bit/s call has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Negotiating,
    Connected(u32),
    /// Nothing answered.
    Failed,
}

/// Timings for bringing a 300 bit/s call up.
mod timing {
    /// Quiet after the answering modem picks up, before it raises its carrier.
    ///
    /// There is no handshake to speak of at 300 bit/s -- the answering modem's
    /// mark tone *is* the answerback -- but a moment of quiet first is what
    /// every modem does and what the network expects.
    pub const ANSWER_DELAY: f64 = 1.0;
    /// How long the far carrier must hold before this end answers it.
    pub const CARRIER_HELD: f64 = 0.45;
    /// How long the far end must sit at idle mark before data may flow.
    ///
    /// This is what waits out a V.25 answer tone without having to recognise
    /// one. 2100 Hz falls inside the answering band and trips its carrier
    /// detector, but it sits below the band centre and so reads as a space,
    /// not a mark. A modem that waited only for carrier would start talking
    /// over three seconds of answer tone.
    pub const MARK_HELD: f64 = 0.2;
    /// Nothing recognisable for this long and the attempt is abandoned.
    pub const PATIENCE: f64 = 60.0;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Answering: off hook and quiet, letting the line settle.
    Waiting,
    /// Originating: listening for the answering modem, saying nothing.
    Listening,
    /// Carrier up, waiting for the far end to settle to idle mark.
    Marking,
    Connected,
    Failed,
}

/// A complete Bell 103 modem: one end of a 300 bit/s call.
///
/// There is very little to it, which is the point of starting here. The two
/// directions are in different bands, so no echo canceller; the tones are
/// detected by a discriminator, so no carrier recovery; the framing
/// re-acquires on every start bit, so no timing loop to hold. What is left is
/// the call setup, and at 300 bit/s that is: one end whistles, the other
/// whistles back.
#[derive(Debug)]
pub struct Modem {
    tx: Bell103Tx,
    rx: Bell103Rx,
    phase: Phase,
    fs: f64,
    /// Samples in the current phase, and since the call began.
    elapsed: f64,
    total: f64,
    /// Samples the condition being waited for has held.
    held: f64,
    /// Recovered characters, held until something asks.
    received: Vec<u8>,
    /// For turning those back into the bits they arrived as.
    framing: AsyncBits,
}

impl Modem {
    pub fn new(role: Role, fs: f64) -> Self {
        Self {
            tx: Bell103Tx::new(role, fs),
            rx: Bell103Rx::new(role, fs),
            phase: match role {
                Role::Originate => Phase::Listening,
                Role::Answer => Phase::Waiting,
            },
            fs,
            elapsed: 0.0,
            total: 0.0,
            held: 0.0,
            received: Vec::new(),
            framing: AsyncBits::new(8),
        }
    }

    /// Take one sample from the line and give back the one to put on it.
    pub fn step(&mut self, line: f64) -> f64 {
        if let Some(byte) = self.rx.feed(line)
            && self.phase == Phase::Connected
        {
            self.received.push(byte);
        }
        self.advance();
        self.tx.next_sample()
    }

    fn advance(&mut self) {
        let step = 1.0 / self.fs;
        self.elapsed += step;
        self.total += step;
        let carrier = self.rx.carrier();
        // The far end is idling when its carrier is up and its discriminator
        // is sitting at mark.
        let idle = carrier && self.rx.level() > 0.5;

        match self.phase {
            Phase::Waiting => {
                if self.elapsed >= timing::ANSWER_DELAY {
                    self.tx.set_transmitting(true);
                    self.enter(Phase::Marking);
                }
            }
            Phase::Listening => {
                if self.hold(carrier, step) >= timing::CARRIER_HELD {
                    self.tx.set_transmitting(true);
                    self.enter(Phase::Marking);
                }
            }
            Phase::Marking => {
                if self.hold(idle, step) >= timing::MARK_HELD {
                    self.enter(Phase::Connected);
                }
            }
            Phase::Connected | Phase::Failed => return,
        }
        if self.total >= timing::PATIENCE {
            self.phase = Phase::Failed;
        }
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.elapsed = 0.0;
        self.held = 0.0;
    }

    fn hold(&mut self, present: bool, step: f64) -> f64 {
        if present {
            self.held += step;
        } else {
            self.held = 0.0;
        }
        self.held
    }

    pub fn status(&self) -> Status {
        match self.phase {
            Phase::Connected => Status::Connected(BAUD as u32),
            Phase::Failed => Status::Failed,
            _ => Status::Negotiating,
        }
    }

    /// Which step of the call setup this end is on.
    pub fn line_phase(&self) -> &'static str {
        match self.phase {
            Phase::Waiting => "waiting",
            Phase::Listening => "listening",
            Phase::Marking => "mark",
            Phase::Connected => "connected",
            Phase::Failed => "failed",
        }
    }

    pub fn carrier(&self) -> bool {
        self.rx.carrier()
    }

    /// Characters recovered from the line.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.received)
    }

    /// The same characters, back in the start-stop bits they arrived as.
    ///
    /// What sits above a data pump elsewhere in this workspace wants bits,
    /// because a synchronous pump has no idea where a character begins and no
    /// business guessing. This one does know: at 300 bit/s the line format
    /// *is* start-stop framing, and the receiver has already found the frames
    /// by re-synchronising on each start bit, which is far steadier than any
    /// continuously tracked bit clock. So the framing is undone and redone
    /// rather than bypassed. It is exact -- a valid frame and its character
    /// determine each other -- and it keeps one interface for every pump.
    pub fn take_bits(&mut self) -> Vec<bool> {
        let bytes = std::mem::take(&mut self.received);
        bytes
            .iter()
            .flat_map(|&b| self.framing.encode(b))
            .collect()
    }

    /// Queue bits for transmission.
    pub fn send_bits(&mut self, bits: &[bool]) {
        self.tx.push_bits(bits);
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let bits: Vec<bool> = bytes.iter().flat_map(|&b| self.framing.encode(b)).collect();
        self.tx.push_bits(&bits);
    }

    pub fn pending_bits(&self) -> usize {
        self.tx.pending_bits()
    }

    /// Discriminator output, which is the only scope FSK has.
    pub fn level(&self) -> f64 {
        self.rx.level()
    }

    /// The discriminator reading at the centre of each recovered bit.
    ///
    /// One value per bit rather than per sample, which is what an eye is drawn
    /// from: distance from zero is the slicer's margin on that decision.
    pub fn take_symbol(&mut self) -> Option<f64> {
        self.rx.take_symbol()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_listen_to_the_opposite_band() {
        assert_eq!(Role::Originate.receive_tones(), (2025.0, 2225.0));
        assert_eq!(Role::Answer.receive_tones(), (1070.0, 1270.0));
    }
}
