//! Bell 212A — 1200 bit/s full duplex, with Bell 103 underneath it.
//!
//! The 1200 bit/s half of a 212A is V.22's modulation, or rather V.22 is the
//! 212A's: 600 baud, four-phase differential PSK, the calling end on a
//! 1200 Hz carrier and the answering end on 2400 Hz, the same dibit-to-phase
//! changes and the same self-synchronising scrambler. So the transmitter,
//! receiver and scrambler here are the V.22bis ones held at 1200.
//!
//! What differs is how a call comes up, and that is because a 212A has to
//! work with the Bell 103 that was already in every office:
//!
//! - The answering modem waits out a two-second billing delay and then sends
//!   2225 Hz. That is not a V.25 answer tone, it is Bell 103's answering mark,
//!   so a 103 that dialled in hears exactly the carrier it expects.
//! - There is no guard tone and no unscrambled-ones signal from the answering
//!   end. The calling 212A hears 2225 Hz for 155 ms, waits 456 ms, and sends
//!   scrambled ones at 1200 in the low channel. A calling 103 would have sent
//!   its 1270 Hz mark instead.
//! - The answering 212A decides by what comes back: 270 ms of scrambled ones
//!   and it drops the tone and answers with scrambled ones at 1200 in the high
//!   channel; a steady 1270 Hz mark and it is talking to a 103, so it stays on
//!   2225 Hz and becomes one.
//! - The calling 212A, having offered 1200, waits for scrambled ones back. If
//!   it goes on hearing nothing but a steady 2225 Hz mark the far end is a
//!   103 that has taken its offer as a carrier, and it falls back to 1270 Hz.
//!
//! Timings: 155 ms, 456 ms, 270 ms and 765 ms are the figures V.22 6.3.1.2
//! inherited from the 212A, and are what 212A-compatible chip data sheets
//! quote. How long the calling end listens to a steady mark before deciding
//! the far end is a 103 is not a published figure; the 1.2 s here is long
//! enough for a 212A answerer to have heard 270 ms of the offer and turned
//! round with time to spare.

use crate::bell103::{self, Bell103Rx, Bell103Tx, Role};
use crate::framing::AsyncBits;
use crate::v22bis::{Channel, Pattern, Rate, Receiver, Signal, Transmitter};

/// The answering 212A's answer tone, which is also Bell 103's answering mark.
pub const ANSWER_TONE: f64 = 2225.0;

/// How far a 212A call has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Negotiating,
    /// Connected, at 1200 or, after falling back, at 300.
    Connected(u32),
    Failed,
}

mod timing {
    /// Off hook and silent before the answer tone: Bell's billing delay.
    pub const BILLING_DELAY: f64 = 2.0;
    /// 2225 Hz heard this long before the calling end believes it.
    pub const HEARD_TONE: f64 = 0.155;
    /// Then silent this long before offering 1200.
    pub const PAUSE: f64 = 0.456;
    /// Scrambled ones heard this long before they count.
    pub const HEARD_SCRAMBLED: f64 = 0.270;
    /// A steady 1270 Hz mark heard this long and the caller is a 103.
    pub const HEARD_103: f64 = 0.300;
    /// Answering: scrambled ones sent this long before data may flow.
    /// Calling: this long after hearing them.
    pub const SETTLE: f64 = 0.765;
    /// The calling end, having offered 1200, hears only a steady 2225 Hz
    /// mark for this long and takes the far end for a 103.
    pub const FALL_BACK: f64 = 1.2;
    /// Nothing recognisable for this long and the attempt is abandoned.
    pub const PATIENCE: f64 = 60.0;
}

/// Discriminator level above which the other band reads as its idle mark.
///
/// Set well above what scrambled DPSK in the same band averages to -- a
/// 1200 Hz carrier reads about +0.3 in the 1070/1270 discriminator -- so that
/// an offer of 1200 is not mistaken for a 103's mark.
const MARK_LEVEL: f64 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Answering: off hook, silent, the billing delay.
    Billing,
    /// Answering: 2225 Hz, listening for which kind of modem called.
    AnswerTone,
    /// Calling: silent, listening for 2225 Hz.
    Listening,
    /// Calling: heard it, and pausing before the offer.
    Pausing,
    /// Sending scrambled ones at 1200 and listening for them back.
    Offering,
    /// Scrambled ones going both ways; waiting out the settle time.
    Settling,
    Connected,
    /// Fallen back to Bell 103, whose own state takes over.
    Bell103,
    Failed,
}

/// One end of a Bell 212A call.
#[derive(Debug)]
pub struct Modem {
    role: Role,
    state: State,
    step: f64,
    elapsed: f64,
    total: f64,
    held: f64,
    marked: f64,
    fs: f64,
    tx: Transmitter,
    rx: Receiver,
    /// The answering end's 2225 Hz: Bell 103's mark, sent as the answer tone.
    tone: Bell103Tx,
    /// A discriminator on the far end's Bell 103 band: the answer tone, heard
    /// from the calling end; a 103's 1270 Hz mark, heard from the answering.
    ear: Bell103Rx,
    /// The Bell 103 this became, if it fell back.
    fallback: Option<Box<bell103::Modem>>,
    /// 2225 Hz and 2100 Hz, narrow: what tells a Bell answer from V.25's
    /// or V.8's answer tone, which the FSK ear alone cannot. 2100 Hz sits
    /// inside the 103 band, and a caller that took ANSam for 2225 Hz
    /// "connected" to a V.92 answerer still offering V.32.
    bell_tone: dsp::ToneDetector,
    v25_tone: dsp::ToneDetector,
    /// Characters out of and into start-stop framing at 1200, for `send` and
    /// `take_bytes`; what sits above a pump uses the bits.
    framing: AsyncBits,
    received: Vec<bool>,
}

impl Modem {
    pub fn new(role: Role, fs: f64) -> Self {
        let channel = match role {
            Role::Originate => Channel::Calling,
            Role::Answer => Channel::Answering,
        };
        let (space, mark) = role.receive_tones();
        let mut tone = Bell103Tx::new(role, fs);
        tone.set_transmitting(false);
        let mut tx = Transmitter::at_rate(channel, Rate::Bps1200, fs);
        tx.set_signal(Signal::Silent);
        let mut rx = Receiver::new(channel, fs);
        rx.set_rate(Rate::Bps1200);
        Self {
            role,
            state: match role {
                Role::Originate => State::Listening,
                Role::Answer => State::Billing,
            },
            step: 1.0 / fs,
            elapsed: 0.0,
            total: 0.0,
            held: 0.0,
            marked: 0.0,
            fs,
            tx,
            rx,
            tone,
            ear: Bell103Rx::with_tones(space, mark, fs),
            fallback: None,
            bell_tone: dsp::ToneDetector::new(ANSWER_TONE, 40.0, fs),
            v25_tone: dsp::ToneDetector::new(2100.0, 40.0, fs),
            framing: AsyncBits::new(8),
            received: Vec::new(),
        }
    }

    /// An answering modem with the billing delay already behind it: on a
    /// call that has been answered for seconds, it whistles 2225 Hz at once.
    pub fn past_billing(mut self) -> Self {
        if self.state == State::Billing {
            self.tone.set_transmitting(true);
            self.state = State::AnswerTone;
        }
        self
    }

    /// Take one sample from the line and give back the one to put on it.
    pub fn step(&mut self, line: f64) -> f64 {
        if let Some(m) = self.fallback.as_mut() {
            return m.step(line);
        }
        self.rx.feed(line);
        self.ear.feed(line);
        self.bell_tone.feed(line);
        self.v25_tone.feed(line);
        self.advance();
        if let Some(m) = self.fallback.as_mut() {
            // The 103 starts on the next sample; this one is its first.
            return m.step(line);
        }
        let dpsk = self.tx.next_sample();
        let tone = self.tone.next_sample();
        dpsk + tone
    }

    fn advance(&mut self) {
        self.elapsed += self.step;
        self.total += self.step;
        let pattern = self.rx.pattern();
        let scrambled = pattern == Pattern::ScrambledOnes;
        let far_mark = self.ear.carrier() && self.ear.level() > MARK_LEVEL;

        match self.state {
            State::Billing => {
                if self.elapsed >= timing::BILLING_DELAY {
                    self.tone.set_transmitting(true);
                    self.enter(State::AnswerTone);
                }
            }
            State::AnswerTone => {
                if scrambled {
                    self.marked = 0.0;
                    if self.hold(true) >= timing::HEARD_SCRAMBLED {
                        // A 212A: drop the tone, answer in kind.
                        self.tone.set_transmitting(false);
                        self.tx.set_signal(Signal::ScrambledOnes);
                        self.enter(State::Settling);
                    }
                } else {
                    self.held = 0.0;
                    if self.mark(far_mark) >= timing::HEARD_103 {
                        self.fall_back();
                    }
                }
            }
            State::Listening => {
                let bell = far_mark && self.bell_tone.amplitude() > 2.0 * self.v25_tone.amplitude();
                if self.hold(bell) >= timing::HEARD_TONE {
                    self.enter(State::Pausing);
                }
            }
            State::Pausing => {
                if self.elapsed >= timing::PAUSE {
                    self.tx.set_signal(Signal::ScrambledOnes);
                    self.enter(State::Offering);
                }
            }
            State::Offering => {
                if scrambled {
                    self.marked = 0.0;
                    if self.hold(true) >= timing::HEARD_SCRAMBLED {
                        self.enter(State::Settling);
                    }
                } else {
                    self.held = 0.0;
                    if self.mark(far_mark) >= timing::FALL_BACK {
                        self.fall_back();
                    }
                }
            }
            State::Settling => {
                if self.elapsed >= timing::SETTLE {
                    // What the handshake heard is not data.
                    self.rx.take_bits();
                    self.enter(State::Connected);
                }
            }
            State::Connected | State::Bell103 | State::Failed => return,
        }
        if self.total >= timing::PATIENCE {
            self.state = State::Failed;
            self.tx.set_signal(Signal::Silent);
            self.tone.set_transmitting(false);
        }
    }

    fn fall_back(&mut self) {
        self.fallback = Some(Box::new(bell103::Modem::marking(self.role, self.fs)));
        self.tx.set_signal(Signal::Silent);
        self.tone.set_transmitting(false);
        self.enter(State::Bell103);
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.elapsed = 0.0;
        self.held = 0.0;
        self.marked = 0.0;
    }

    /// Seconds scrambled ones (or, listening, the answer tone) have held.
    fn hold(&mut self, present: bool) -> f64 {
        self.held = if present { self.held + self.step } else { 0.0 };
        self.held
    }

    /// Seconds the far end has sat at a steady Bell 103 mark, counted apart
    /// from scrambled ones so that neither interrupts the other's count.
    fn mark(&mut self, present: bool) -> f64 {
        self.marked = if present {
            self.marked + self.step
        } else {
            0.0
        };
        self.marked
    }

    pub fn status(&self) -> Status {
        if let Some(m) = self.fallback.as_ref() {
            return match m.status() {
                bell103::Status::Negotiating => Status::Negotiating,
                bell103::Status::Connected(r) => Status::Connected(r),
                bell103::Status::Failed => Status::Failed,
            };
        }
        match self.state {
            State::Connected => Status::Connected(1200),
            State::Failed => Status::Failed,
            _ => Status::Negotiating,
        }
    }

    /// Whether this call fell back to Bell 103.
    pub fn is_bell103(&self) -> bool {
        self.fallback.is_some()
    }

    /// "Bell 212A", or "Bell 103" once fallen back.
    pub fn standard(&self) -> &'static str {
        if self.fallback.is_some() {
            "Bell 103"
        } else {
            "Bell 212A"
        }
    }

    /// Which step of the call setup this end is on.
    pub fn phase(&self) -> &'static str {
        if let Some(m) = self.fallback.as_ref() {
            return m.line_phase();
        }
        match self.state {
            State::Billing => "billing delay",
            State::AnswerTone => "answer tone",
            State::Listening => "listening",
            State::Pausing => "pausing",
            State::Offering => "offering 1200",
            State::Settling => "scrambled ones",
            State::Connected => "connected",
            State::Bell103 => "Bell 103",
            State::Failed => "failed",
        }
    }

    pub fn carrier(&self) -> bool {
        match self.fallback.as_ref() {
            Some(m) => m.carrier(),
            None => self.rx.carrier(),
        }
    }

    /// Bits recovered from the line: synchronous descrambled bits at 1200,
    /// start-stop framed characters at 300 (see `bell103::Modem::take_bits`).
    pub fn take_bits(&mut self) -> Vec<bool> {
        match self.fallback.as_mut() {
            Some(m) => m.take_bits(),
            None => self.rx.take_bits(),
        }
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        match self.fallback.as_mut() {
            Some(m) => m.send_bits(bits),
            None => self.tx.push_bits(bits),
        }
    }

    pub fn pending_bits(&self) -> usize {
        match self.fallback.as_ref() {
            Some(m) => m.pending_bits(),
            None => self.tx.pending_bits(),
        }
    }

    /// Characters, in start-stop framing over whichever modulation is up.
    pub fn send(&mut self, bytes: &[u8]) {
        let bits: Vec<bool> = bytes.iter().flat_map(|&b| self.framing.encode(b)).collect();
        self.send_bits(&bits);
    }

    /// Characters recovered from the line, once connected.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        if let Some(m) = self.fallback.as_mut() {
            return m.take_bytes();
        }
        if self.state != State::Connected {
            self.rx.take_bits();
            return Vec::new();
        }
        self.received.extend(self.rx.take_bits());
        let bits = std::mem::take(&mut self.received);
        bits.into_iter()
            .filter_map(|b| self.framing.feed(b))
            .collect()
    }

    /// The decision point, at 1200; nothing once fallen back to FSK.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        self.fallback
            .is_none()
            .then(|| self.rx.constellation_point())
    }

    pub fn residual_error(&self) -> Option<f64> {
        self.fallback.is_none().then(|| self.rx.residual_error())
    }

    /// Discriminator output, once fallen back to FSK.
    pub fn level(&self) -> Option<f64> {
        self.fallback.as_ref().map(|m| m.level())
    }

    pub fn framing_errors(&self) -> Option<u64> {
        self.fallback.as_ref().map(|m| m.framing_errors())
    }

    pub fn take_symbol(&mut self) -> Option<f64> {
        self.fallback.as_mut().and_then(|m| m.take_symbol())
    }
}
