//! The half-duplex V.34 modem (clause 12): one end of a Super G3 fax call,
//! from the end of phase 2 to the last page and back.
//!
//! Half-duplex V.34 has two channels, used in turn. The primary channel
//! carries the page one way, from the source to the recipient, at up to
//! 33 600 bit/s ([`super::primary`]). The control channel carries everything
//! else both ways at once, at 600 baud ([`super::control`]): the MPh
//! sequences that settle the page's rate, and then T.30's frames. Clause 12
//! says how a modem goes from one to the other and back, and this is that
//! procedure, strung over the pieces the other modules built:
//!
//! - phase 2 (12.2), [`super::phase2h`]'s, from the 75 ms of silence that
//!   end V.8 to INFOh, and again for a primary channel retrain (12.7);
//! - phase 3 (12.3): the source sends S, S-bar, PP and TRN, the recipient
//!   trains on them, and a recipient that heard nothing goes back to its
//!   phase 2 tone (12.3.3), which the source answers (12.4.3.1);
//! - the control channel start-up (12.4): 70 ms of silence, PPh, ALT, MPh
//!   until the far end's MPh has come, then E, and the rates from the two
//!   MPh;
//! - the page (12.5): the control channel turned off with 4T of scrambled
//!   ones (12.6.3), then the burst, then 35 ms of scrambled ones;
//! - back to the control channel (12.6): Sh and S-bar-h, ALT and E when
//!   nothing is to change, or PPh and the MPh exchange again when either
//!   end wants a new rate (Figures 25 to 27);
//! - the retrains: the control channel's on AC (12.8), from either end, and
//!   the primary channel's through phase 2's tones (12.7);
//! - and every three-second wait of 12.4 and 12.6, each of which ends in a
//!   control channel retrain.
//!
//! One [`Modem`] is one end. Its customer is the join to T.30 Annex F
//! (`docs/design/superg3/plan.md` 10.1 and 10.2, `wp-h2.md`): control bits
//! in and out while on the control channel, page bits in (source) or out
//! (recipient) while on the primary channel, [`Modem::to_primary`] when
//! T.30's ones are over and [`Modem::to_control`] when the page is, and a
//! few facts back -- [`Event::ControlUp`] whenever the control channel comes
//! back, [`Modem::far_silent`] for the source's turn, and the rates.
//!
//! What the text leaves open is settled as `plan.md` 8 says: the source
//! sends MPh Type 0 with bits 29 to 32 zero and the recipient Type 0 as well
//! (its equaliser is linear and asks for no precoding, `wp-e.md`); a
//! resynchronisation carries the rates over; AC that is never answered
//! falls to the same three seconds as everything else, a few times over,
//! and then the call is given up; and the two E sequences of a
//! resynchronisation go side by side, as Figure 27's corrected inset draws
//! them.

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use super::control::{self, Heard, Hearing, Kind, Phase, Rate, Reading, Segment, Sent};
use super::dpsk::{self, Side};
use super::frame::Framing;
use super::info::{Info, Info0, InfoH, SymbolRate};
use super::mp::{self, Coefficient, ControlRate, Mph, MphFinder, MphFound, Trellis};
pub use super::phase2::Role;
use super::phase2h::{self, Part};
use super::primary::{self, Channel, DataMode, Sending};
use super::probe;
use super::qam::Band;
use super::signals::Size;
use super::trellis::Code;
use crate::v32::Mode;

/// "within three seconds": every wait of 12.4.3, 12.4.4, 12.6.1.5, 12.6.1.6,
/// 12.6.2.4 and 12.6.2.5, at the end of which a control channel retrain is
/// begun (12.8.1).
const THREE_SECONDS: f64 = 3.0;

/// 12.8.1 gives no time for an AC that is never answered (`spec-v34-hdx.md`
/// F-V34 #12). It gets the same three seconds as everything else, this many
/// times over, and then the call is given up: a far end that has gone should
/// not keep this end busy for longer than a T.30 timer would.
const AC_ROUNDS: u32 = 3;

/// Times phase 3 may fail and be gone back to (12.3.3) before the call is
/// given up.
const PHASE3_TRIES: u32 = 3;

/// 12.2.1.2.6, which 12.3.3 goes back to: "continues transmitting Tone A for
/// 25 ms, then sends INFOh".
const TONE_BEFORE_INFOH: f64 = 0.025;

/// The recipient's wait for the source's tone in 12.3.3's recovery before
/// INFOh goes anyway: 12.2.1.4.3's 2000 ms, the recovery of the step 12.3.3
/// goes back to.
const TONE_WAIT: f64 = 2.0;

/// The source's wait for INFOh after answering the recipient's tone
/// (12.4.3.1). 12.2.1.3.4 has a source wait 2000 ms and then wait on; this
/// gives it that and a second exchange of tones, and then gives the call up.
const INFOH_WAIT: f64 = 5.0;

/// "After detecting Tone A for more than 50 ms" (12.7.1.2; 12.7.2.2 of
/// tone B): the far end wants the primary channel retrained.
const RETRAIN_TONE_SECONDS: f64 = 0.050;

/// How far the far end's control carrier has to fall, against its level as
/// this end began a primary channel retrain, to count as gone: 12 dB, which
/// the control receiver's 20 ms envelope takes 28 ms to fall by once the
/// far end stops -- leaving 40 ms of its 70 ms of silence before its tone --
/// and which a jitter buffer's 20 ms hole never takes it to. The receiver's
/// own carrier-off judgement is V.32's 59 dB, on a line losing 15 dB a
/// hundred milliseconds down the same envelope: longer than the far end's
/// silence, so its tone came with the carrier still on, and phase 2 stayed
/// deaf through the tone and its reversal.
const FAR_GONE: f64 = 0.25;

/// How long the far end's control carrier has to have been gone before the
/// far end counts as silent (F.3.2.3/T.30, the source's turn to the page):
/// longer than the 20 ms hole a VoIP jitter buffer leaves, and short beside
/// the 70 ms of silence the source itself puts before its S.
const FAR_SILENT_SECONDS: f64 = 0.050;

/// Where the modem is, for the join (`plan.md` 10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Phase 3 and the first control channel start-up.
    Starting,
    /// On the control channel: bits both ways.
    Control,
    /// Turning the control channel off for the page (12.6.3).
    ToPrimary,
    /// On the primary channel: the page going, or being listened for.
    Primary,
    /// The page over, and the control channel coming back (12.6, or 12.4
    /// again for a new rate).
    ToControl,
    /// A control channel retrain (12.8), begun here or by the far end.
    Retraining,
    /// Given up, and why: [`Modem::failure`].
    Failed,
}

/// What the modem has to tell the join.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Phase 2 is over: INFOh has gone or come, and [`Modem::infoh`] and
    /// [`Modem::far_capabilities`] say what it settled.
    Phase2Over,
    /// Phase 3 is over (12.3.2.3); or it failed and the tone exchange that
    /// repeats it has begun (12.3.3).
    Phase3Over { well: bool },
    /// The control channel is up -- E has come from the far end and this
    /// end's is on its way -- after a start-up, a resynchronisation or a
    /// retrain. T.30's `control_restarted()` goes here, before any bits are
    /// taken: everything either side of it is no frame (`wp-h2.md`).
    ControlUp,
    /// The page has begun arriving (recipient): circuit 109 on, B1 read with
    /// this many bits wrong.
    PageStarted { b1_errors: usize },
    /// The page's carrier has gone (recipient): circuit 109 off, and the last
    /// of the page's bits are in [`Modem::take_page_bits`].
    PageEnded,
    /// A control channel retrain has begun (12.8), from this end or the
    /// other; [`Event::ControlUp`] follows when it is done.
    Retraining,
    /// The modem has given up, for this reason.
    Failed(&'static str),
}

/// What phase 2 settled, for a modem started from phase 3: INFOh as it went
/// or came, and the two INFO0 (`wp-d.md`, "For G").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    pub infoh: InfoH,
    /// This end's INFO0.
    pub ours: Info0,
    /// The far end's.
    pub far: Info0,
}

/// What this end is waiting for from the far end, with its own signals
/// queued or going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Awaiting {
    /// PPh, in the start-up straight after phase 3 (12.4.1.1, 12.4.2.1),
    /// where a tone instead is 12.3.3's recovery (12.4.3.1).
    FirstPph,
    /// PPh, in a start-up the source began after a page for a new rate
    /// (12.6.1.1 -> 12.4.1.1), or asked of the recipient by one (12.6.2.1).
    Pph,
    /// PPh or Sh followed by S-bar-h, after a page: the source with its own
    /// Sh, S-bar-h and ALT going (12.6.1.2), the recipient silent (12.6.2.1).
    ShOrPph,
    /// The source's PPh, the recipient having answered Sh with PPh and ALT
    /// because it wants a change (12.6.2.3).
    ChangePph,
    /// The responder's PPh, this end's AC going (12.8.1).
    Ac,
    /// The initiator's PPh, this end's PPh and ALT sent in answer to its AC
    /// (12.8.2).
    Responding,
}

/// Where the modem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Phase 2 (12.2): the first time, or again for a primary channel
    /// retrain (12.7). Nothing else of the modem runs meanwhile.
    Phase2,
    /// Phase 3 (12.3): the source sending it, the recipient training on it.
    Phase3,
    /// The tone exchange that repeats phase 3: the recipient's tone, the
    /// source's, 25 ms more and INFOh again (12.3.3, 12.4.3.1).
    Recovering { tone_heard: Option<u64>, infoh_sent: bool },
    Awaiting(Awaiting),
    /// MPh going, over and over; the far end's awaited (12.4.1.2, 12.4.2.3).
    Mph,
    /// E queued behind whatever is going; the far end's awaited.
    AwaitE,
    /// On the control channel, data both ways.
    Control,
    /// 4T of scrambled ones queued (12.6.3); the page follows.
    TurningOff,
    /// The page (12.5): going, or listened for.
    Page,
    Failed,
}

/// How the control channel start-up in hand began: what the join is told
/// the modem is doing while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    First,
    /// After a page, resynchronising or restarting.
    Page,
    /// A retrain on AC.
    Retrain,
}

/// One end of a half-duplex V.34 call.
#[derive(Debug)]
pub struct Modem {
    role: Role,
    /// Whether this end sends the page.
    source_end: bool,
    fs: f64,
    /// Samples stepped.
    now: u64,
    stage: Stage,
    turn: Turn,
    /// When the wait in hand runs out, in samples.
    deadline: Option<u64>,

    /// Phase 2, while it runs, and kept for its readings after.
    phase2: Option<phase2h::Modem>,
    /// Phase 2 is over, and [`Modem::infoh`] means something.
    phase2_done: bool,
    /// A primary channel retrain this end began with the far end still on
    /// the control channel: its carrier's level then, and phase 2 is fed
    /// silence until the level has fallen [`FAR_GONE`] below it.
    deaf_above: Option<f64>,

    control: control::Modem,
    source: Option<primary::Source>,
    recipient: Option<primary::Recipient>,
    /// Phase 2's tone and INFOh, for 12.3.3 and 12.4.3.1.
    tone: dpsk::Transmitter,
    /// The source's ear for INFOh while it answers the recipient's tone.
    infoh_rx: Option<dpsk::Receiver>,
    /// The tone is to start once this end's control signals have died away.
    tone_pending: bool,

    infoh: InfoH,
    channel: Channel,
    ours: Info0,
    far: Info0,
    /// Which PPh this end sends (`plan.md` 8.1).
    reading: Reading,

    /// This end's MPh as last sent, and the far end's as last received.
    mph: Mph,
    far_mph: Option<Mph>,
    finder: MphFinder,
    /// The precoding coefficients the far recipient's Type 1 MPh gave this
    /// start-up, zero until one does (10.2.4.4).
    precoding: [Coefficient; 3],
    /// Data mode as the last MPh exchange settled it.
    data: Option<DataMode>,
    /// The control channel's rates, transmit then receive, once settled.
    rates: Option<(ControlRate, ControlRate)>,
    /// What this end asks the far end to send control data at (bit 27).
    control_rate: ControlRate,
    /// The most this end offers in MPh, as a multiple of 2400.
    cap: u8,
    /// A start-up rather than a resynchronisation is wanted when the page
    /// is over, for a new rate.
    renegotiate: bool,
    /// The source has been told the page is over and is turning off.
    page_ending: bool,
    /// The far end's E has come in this start-up or resynchronisation.
    far_e: bool,
    /// Since when the far end's control carrier has been gone.
    far_off_since: Option<u64>,
    /// The AC being heard has been answered.
    ac_answered: bool,
    /// What the primary receiver last trained to, in decibels.
    trained_snr: Option<f64>,
    recoveries: u32,
    /// Control channel retrains begun, from either end.
    retrains: u32,
    /// Rounds of AC sent without an answer.
    ac_rounds: u32,
    events: VecDeque<Event>,
    failure: Option<&'static str>,
}

impl Modem {
    /// One end from the 75 ms of silence that end V.8 (12.1): phase 2 goes
    /// first, INFO0 at once, and phase 3 follows on what it settles. `role`
    /// is which modem dialled, which fixes the carriers and the scramblers;
    /// `source` whether this end sends the page.
    pub fn new(role: Role, source: bool, fs: f64) -> Self {
        let phase2 = phase2h::Modem::new(role, part_of(source), fs);
        let placeholder = InfoH {
            power_reduction: 0,
            trn_length: 0,
            high_carrier: false,
            pre_emphasis: 0,
            symbol_rate: SymbolRate::S2400,
            trn_size: Size::Four,
        };
        let setup = Setup { infoh: placeholder, ours: phase2.capabilities(), far: Info0::default() };
        let mut modem = Self::build(role, source, fs, setup);
        modem.phase2 = Some(phase2);
        modem.phase2_done = false;
        modem.stage = Stage::Phase2;
        modem
    }

    /// One end from phase 3 on, phase 2 having settled `setup` elsewhere:
    /// the source begins its 70 ms of silence and S at once (12.3.1.1), the
    /// recipient listens for them (12.3.2.1).
    pub fn after_phase2(role: Role, source: bool, fs: f64, setup: Setup) -> Self {
        let mut modem = Self::build(role, source, fs, setup);
        modem.begin_phase3();
        modem
    }

    fn build(role: Role, source: bool, fs: f64, setup: Setup) -> Self {
        let side = side_of(role);
        Self {
            role,
            source_end: source,
            fs,
            now: 0,
            stage: Stage::Phase3,
            turn: Turn::First,
            deadline: None,
            phase2: None,
            phase2_done: true,
            deaf_above: None,
            control: control::Modem::new(side, fs),
            source: None,
            recipient: None,
            tone: dpsk::Transmitter::new(side, fs),
            infoh_rx: None,
            tone_pending: false,
            infoh: setup.infoh,
            channel: channel_of(&setup.infoh),
            ours: setup.ours,
            far: setup.far,
            reading: Reading::default(),
            mph: Mph::default(),
            far_mph: None,
            finder: MphFinder::new(),
            precoding: [(0, 0); 3],
            data: None,
            rates: None,
            control_rate: ControlRate::Bps1200,
            cap: 14,
            renegotiate: false,
            page_ending: false,
            far_e: false,
            far_off_since: None,
            ac_answered: false,
            trained_snr: None,
            recoveries: 0,
            retrains: 0,
            ac_rounds: 0,
            events: VecDeque::new(),
            failure: None,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Whether this end sends the page.
    pub fn is_source(&self) -> bool {
        self.source_end
    }

    pub fn state(&self) -> State {
        match self.stage {
            Stage::Control => State::Control,
            Stage::TurningOff => State::ToPrimary,
            Stage::Page => State::Primary,
            Stage::Failed => State::Failed,
            // A retrain, of either channel, is one to the join from its
            // first silence to the channel coming back: a 12.7 retrain's
            // phase 3 and start-up included.
            _ if self.turn == Turn::Retrain => State::Retraining,
            Stage::Phase2 | Stage::Phase3 | Stage::Recovering { .. } | Stage::Awaiting(Awaiting::FirstPph) => State::Starting,
            Stage::Awaiting(_) | Stage::Mph | Stage::AwaitE => match self.turn {
                Turn::First => State::Starting,
                Turn::Page => State::ToControl,
                Turn::Retrain => State::Retraining,
            },
        }
    }

    /// Why the modem gave up, once it has.
    pub fn failure(&self) -> Option<&'static str> {
        self.failure
    }

    /// A name for the window.
    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::Phase2 => self.phase2.as_ref().map_or("V.34 phase 2", phase2h::Modem::phase),
            Stage::Phase3 => "V.34 phase 3",
            Stage::Recovering { .. } => "V.34 phase 3 again",
            Stage::Awaiting(Awaiting::Ac | Awaiting::Responding) => "V.34 control retrain",
            Stage::Awaiting(Awaiting::ShOrPph) => "V.34 control resync",
            Stage::Awaiting(_) | Stage::Mph | Stage::AwaitE => match self.turn {
                Turn::Retrain => "V.34 control retrain",
                _ => "V.34 control start-up",
            },
            Stage::Control => "V.34 control channel",
            Stage::TurningOff => "V.34 turning to the page",
            Stage::Page if self.source_end => "V.34 page",
            Stage::Page => "V.34 page awaited",
            Stage::Failed => "V.34 failed",
        }
    }

    /// The next thing to tell the join, if there is one.
    pub fn event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// What INFOh chose for the primary channel, once phase 2 is over.
    pub fn channel(&self) -> Option<Channel> {
        self.phase2_done.then_some(self.channel)
    }

    /// INFOh as it went or came, once phase 2 is over.
    pub fn infoh(&self) -> Option<InfoH> {
        self.phase2_done.then_some(self.infoh)
    }

    /// This end's INFO0.
    pub fn capabilities(&self) -> Info0 {
        self.ours
    }

    /// The far end's INFO0, once phase 2 has it.
    pub fn far_capabilities(&self) -> Option<Info0> {
        self.phase2_done.then_some(self.far)
    }

    /// Phase 2, for the window's readings of it: what it probed, how often
    /// it recovered. None for a modem started from phase 3 that has not
    /// retrained through it.
    pub fn phase2(&self) -> Option<&phase2h::Modem> {
        self.phase2.as_ref()
    }

    /// The primary channel's rate in bit/s, as the last MPh exchange settled
    /// it (12.4.1.3, 12.4.2.4).
    pub fn primary_rate(&self) -> Option<u32> {
        self.data.map(|d| d.rate)
    }

    /// Data mode as the last MPh exchange settled it.
    pub fn data_mode(&self) -> Option<DataMode> {
        self.data
    }

    /// The control channel's rates in bit/s, what this end transmits at and
    /// what it receives at, once settled.
    pub fn control_rates(&self) -> Option<(u32, u32)> {
        self.rates.map(|(t, r)| (bits_per_second(t), bits_per_second(r)))
    }

    /// This end's MPh as last sent, and the far end's as last received.
    pub fn mph(&self) -> (Mph, Option<Mph>) {
        (self.mph, self.far_mph)
    }

    /// The control channel modem, for the window's readings.
    pub fn control(&self) -> &control::Modem {
        &self.control
    }

    /// The primary channel's source, at the end that sends the page.
    pub fn primary_source(&self) -> Option<&primary::Source> {
        self.source.as_ref()
    }

    /// The primary channel's recipient, at the end that receives it.
    pub fn primary_recipient(&self) -> Option<&primary::Recipient> {
        self.recipient.as_ref()
    }

    /// What the primary receiver last trained to, in decibels.
    pub fn primary_snr_db(&self) -> Option<f64> {
        self.trained_snr
    }

    /// Times phase 3 failed and was gone back to (12.3.3, 12.4.3.1).
    pub fn recoveries(&self) -> u32 {
        self.recoveries
    }

    /// Control channel retrains (12.8) begun, from either end.
    pub fn retrains(&self) -> u32 {
        self.retrains
    }

    /// Offer no more than `bits_per_second` on the primary channel in the
    /// next MPh: the join's or the window's cap. Rounded down to a multiple
    /// of 2400, and never below it.
    pub fn limit_rate(&mut self, bits_per_second: u32) {
        self.cap = ((bits_per_second / 2400) as u8).clamp(1, 14);
    }

    /// Ask the far end, in the next MPh, to send control channel data at
    /// this rate (bit 27). Both ends transmit at the lower of the two asked
    /// for, since asymmetric rates are left for further study
    /// (F.3.1.4/T.30).
    pub fn ask_control_rate(&mut self, rate: ControlRate) {
        self.control_rate = rate;
    }

    /// Send PPh in this reading of 10-2 from now on (`plan.md` 8.1).
    pub fn send_pph_as(&mut self, reading: Reading) {
        self.reading = reading;
    }

    /// Whether the far end has fallen silent on the control channel: its
    /// carrier gone for [`FAR_SILENT_SECONDS`] while this end is on the
    /// channel. How the source knows the recipient is ready for the page
    /// (F.3.2.3, F.3.4.5/T.30). False from the moment the channel comes
    /// back, and while this end is turning round.
    pub fn far_silent(&self) -> bool {
        self.stage == Stage::Control
            && self.far_off_since.is_some_and(|since| (self.now - since) as f64 >= FAR_SILENT_SECONDS * self.fs)
    }

    /// Control channel bits to send, first in time first, taken only while
    /// the channel is up: false, and none taken, otherwise. They go behind
    /// whatever is queued, this end's E included.
    pub fn send_control_bits(&mut self, bits: &[bool]) -> bool {
        if self.stage != Stage::Control {
            return false;
        }
        self.control.transmitter.send_bits(bits);
        true
    }

    /// Control channel bits taken and not yet sent.
    pub fn pending_control_bits(&self) -> usize {
        self.control.transmitter.pending_bits()
    }

    /// The far end's control channel bits since this was last asked,
    /// descrambled: those after its E, and only while the channel is up.
    /// Off it there is nothing the far end means: the recipient's
    /// receiver, left reading after the source turned off, would hand up
    /// the page's energy as bits for the quarter second its core takes to
    /// give up.
    pub fn take_control_bits(&mut self) -> Vec<bool> {
        let bits = self.control.receiver.take_bits();
        if self.stage == Stage::Control { bits } else { Vec::new() }
    }

    /// Whether the far end's control carrier is on the line.
    pub fn control_carrier(&self) -> bool {
        self.control.receiver.carrier()
    }

    /// Page bits to send, at the source while the page is going: false, and
    /// none taken, otherwise. Ones go while there are none.
    pub fn send_page_bits(&mut self, bits: &[bool]) -> bool {
        if self.stage != Stage::Page || self.page_ending {
            return false;
        }
        match self.source.as_mut() {
            Some(source) => {
                source.push_bits(bits);
                true
            }
            None => false,
        }
    }

    /// Page bits taken and not yet sent.
    pub fn pending_page_bits(&self) -> usize {
        self.source.as_ref().map_or(0, primary::Source::pending_bits)
    }

    /// The page's bits so far, at the recipient: those after B1, in order.
    pub fn take_page_bits(&mut self) -> Vec<bool> {
        self.recipient.as_mut().map(primary::Recipient::take_bits).unwrap_or_default()
    }

    /// Circuit 109 at the recipient: whether the page's carrier is there.
    pub fn page_carrier(&self) -> bool {
        self.recipient.as_ref().is_some_and(primary::Recipient::carrier)
    }

    /// Leave the control channel for the page: circuit 105 dropping. The
    /// source sends 4T of scrambled ones and then the burst (12.6.3.1,
    /// 12.5.1); the recipient the 4T and then silence, listening (12.6.3.2,
    /// 12.5.2). What is queued for the control channel goes first. False,
    /// and nothing done, off the control channel.
    pub fn to_primary(&mut self) -> bool {
        if self.stage != Stage::Control || self.data.is_none() {
            return false;
        }
        self.control.transmitter.queue(Segment::Ones(control::TURN_OFF_SYMBOLS));
        // Nothing more is coming that means anything: the recipient's
        // receiver would otherwise go on reading the source's ones, and
        // then the page's energy, as data. Hunting is what 12.5.2 and
        // 12.6.2.1 want of it next.
        self.control.receiver.stop();
        self.stage = Stage::TurningOff;
        self.deadline = None;
        true
    }

    /// The page is over: circuit 105 dropping again. At the source, what is
    /// queued goes, then the turn-off (12.5.3.1), then 12.6.1 -- or 12.4.1.1
    /// when `renegotiate` asks for a new rate, this end then offering one
    /// step below the rate in use (F.3.4.5/T.30 Note 1). At the recipient
    /// the turn is the far end's signals' to make (12.5.2, 12.6.2), and this
    /// records the wish for a change (12.6.2.3) -- and, where no page is
    /// arriving, stops listening for one (12.5.3.2). False if there is no
    /// page to end.
    pub fn to_control(&mut self, renegotiate: bool) -> bool {
        // The recipient's page ends with the far carrier, and its wish for a
        // change holds good until the source's Sh is answered.
        let turning = !self.source_end && self.stage == Stage::Awaiting(Awaiting::ShOrPph);
        if self.stage != Stage::Page && !turning {
            return false;
        }
        if renegotiate {
            let current = self.data.map_or(14, |d| (d.rate / 2400) as u8);
            self.cap = self.cap.min(current.saturating_sub(1)).max(1);
            self.renegotiate = true;
        }
        if let Some(source) = self.source.as_mut() {
            if !self.page_ending {
                source.end_page();
                self.page_ending = true;
            }
        } else if let Some(recipient) = self.recipient.as_mut()
            && !recipient.carrier()
        {
            // 12.5.3.2: "detects the OFF to ON transition of Circuit 105 ...
            // turn OFF Circuit 109 and clamp Circuit 104, then proceed
            // according to 12.6.2".
            recipient.stop();
            self.await_after_page();
        }
        true
    }

    /// Retrain the control channel (12.8.1): AC until the far end's PPh,
    /// then PPh, ALT and the MPh exchange again. For the join when a
    /// recipient's T.30 has something to say on a channel that is down
    /// (`wp-h2.md`), or anyone who finds the channel bad. False while the
    /// source is on the page, in phase 3, or once the modem has given up.
    pub fn retrain_control(&mut self) -> bool {
        match self.stage {
            Stage::Control | Stage::Mph | Stage::AwaitE | Stage::TurningOff => {}
            Stage::Awaiting(Awaiting::Ac) => return true,
            Stage::Awaiting(_) => {}
            Stage::Page if !self.source_end => {}
            _ => return false,
        }
        self.begin_retrain();
        true
    }

    /// Retrain the primary channel (12.7.1.1, 12.7.2.1): circuit 106 off,
    /// 70 ms of silence, this end's phase 2 tone, and the tone exchange,
    /// phase 3 and the control channel start-up again, with no INFO0. From
    /// the control channel only; T.30 leaves its use in phase C for further
    /// study (F.3.3). False elsewhere.
    pub fn retrain_primary(&mut self) -> bool {
        if self.stage != Stage::Control {
            return false;
        }
        self.begin_phase2_again(true);
        true
    }

    /// Carry the modem one sample further: hear `input`, and say what goes
    /// on the line -- the control channel's sample and the primary
    /// channel's summed, each nought when idle, and phase 2's tone in the
    /// recoveries.
    pub fn step(&mut self, input: f64) -> f64 {
        self.now += 1;
        if self.stage == Stage::Phase2 {
            // Phase 2 has the line to itself: nothing else listens or
            // speaks until INFOh has gone or come. Except that in a retrain
            // this end began, the far end is still on the control channel
            // for a round trip and more, and its 600 baud symbols are on the
            // very carrier phase 2 listens to for the far tone: at four
            // points three transitions in four turn the phase by a quarter
            // or not at all, which phase 2's tone judge does not see as a
            // dip, and the one in four that turns it half way is a tone's
            // reversal to it. 12.7.1.1 and 12.7.2.1 have the initiator
            // "condition its receiver to detect" the far tone after its own
            // silence; here phase 2 hears silence until the far carrier has
            // gone, the control receiver's envelope of it saying when, and
            // the far end's own 70 ms of silence then come before its tone.
            if let Some(loud) = self.deaf_above {
                self.control.receiver.feed(input);
                while self.control.receiver.heard().is_some() {}
                self.control.receiver.take_sync_bits();
                self.control.receiver.take_bits();
                if self.control.receiver.level() <= loud * FAR_GONE {
                    self.deaf_above = None;
                }
            }
            let heard = if self.deaf_above.is_some() { 0.0 } else { input };
            let Some(phase2) = self.phase2.as_mut() else { return 0.0 };
            let out = phase2.step(heard);
            match phase2.status() {
                phase2h::Status::Running => {}
                phase2h::Status::Done => self.finish_phase2(),
                phase2h::Status::Failed(why) => self.fail(why),
            }
            return out;
        }
        let mut out = self.control.step(input);
        if let Some(recipient) = self.recipient.as_mut() {
            recipient.feed(input);
        }
        if let Some(mut rx) = self.infoh_rx.take() {
            let info = rx.feed(input);
            self.infoh_rx = Some(rx);
            if let Some(info) = info {
                self.on_info(info);
            }
        }
        if self.stage != Stage::Failed {
            self.hear();
            self.sent();
            self.primary_events();
            self.sync_bits();
            self.poll();
            if self.deadline.is_some_and(|d| self.now >= d) {
                self.on_deadline();
            }
        }
        if let Some(source) = self.source.as_mut() {
            out += source.next_sample();
        }
        out + self.tone.next_sample()
    }

    fn seconds(&self, seconds: f64) -> u64 {
        (seconds * self.fs) as u64
    }

    fn wait(&mut self, seconds: f64) {
        self.deadline = Some(self.now + self.seconds(seconds));
    }

    /// Everything the control receiver heard.
    fn hear(&mut self) {
        while let Some(heard) = self.control.receiver.heard() {
            match heard {
                Heard::Pph { .. } => self.on_pph(),
                Heard::Reversal { .. } => self.on_reversal(),
                Heard::Tone { .. } => self.on_tone(),
                Heard::E { .. } => self.on_far_e(),
                _ => {}
            }
        }
    }

    /// The far end's PPh: the start of a control channel start-up, from
    /// wherever this end is.
    fn on_pph(&mut self) {
        match self.stage {
            // 12.4.1.1 and 12.4.1.2: this end's PPh and ALT are going; MPh
            // within 120T.
            Stage::Awaiting(Awaiting::FirstPph | Awaiting::Pph) if self.source_end => self.send_mph(),
            // 12.4.2.1 to 12.4.2.3: PPh back, ALT for 16T, then MPh.
            Stage::Awaiting(Awaiting::FirstPph | Awaiting::Pph) => {
                self.answer_pph();
                self.send_mph();
            }
            // 12.6.1.3 at the source, 12.6.2.1 at the recipient: the other
            // end wants a change.
            Stage::Awaiting(Awaiting::ShOrPph) => {
                self.answer_pph();
                self.send_mph();
            }
            // 12.6.2.3 -> 12.4.2.3, and 12.8.2's responder: this end's PPh
            // and ALT went already.
            Stage::Awaiting(Awaiting::ChangePph | Awaiting::Responding) => self.send_mph(),
            // 12.8.1: the responder's PPh ends this end's AC.
            Stage::Awaiting(Awaiting::Ac) => {
                self.control.transmitter.clear();
                self.answer_pph();
                self.send_mph();
            }
            // A start-up asked for with no AC and no page between: not a
            // procedure of clause 12's, but a far end that sends PPh wants
            // the MPh exchange, and answering it costs nothing.
            Stage::Control => {
                self.turn = Turn::Retrain;
                self.answer_pph();
                self.send_mph();
            }
            _ => {}
        }
    }

    /// Sh followed by S-bar-h from the far end.
    fn on_reversal(&mut self) {
        match self.stage {
            // 12.6.1.4: E now, at the rates of before.
            Stage::Awaiting(Awaiting::ShOrPph) if self.source_end => self.queue_e(false),
            // 12.6.2.2 and 12.6.2.3. Sh in a page that never trained, or
            // still being listened for, is the page over (wp-e.md item 5);
            // in AC, the source came back from its page before hearing it;
            // in data or a resynchronisation already answered, a source
            // that did not hear this end's Sh -- answered again, since the
            // source's own E is only ever sent after this end's reversal.
            Stage::Awaiting(Awaiting::ShOrPph | Awaiting::Ac) | Stage::Control if !self.source_end => self.answer_sh(),
            Stage::AwaitE if !self.source_end && self.turn == Turn::Page => self.answer_sh(),
            Stage::Page if !self.source_end && !self.page_carrier() => {
                if let Some(recipient) = self.recipient.as_mut() {
                    recipient.stop();
                }
                self.answer_sh();
            }
            _ => {}
        }
    }

    /// An unmodulated carrier from the far end: the recipient's phase 2 tone,
    /// when the source is waiting for PPh after phase 3 (12.4.3.1).
    fn on_tone(&mut self) {
        match self.stage {
            Stage::Awaiting(Awaiting::FirstPph) if self.source_end => self.recover_source(),
            Stage::Recovering { tone_heard: None, infoh_sent } if !self.source_end => {
                self.stage = Stage::Recovering { tone_heard: Some(self.now), infoh_sent };
            }
            _ => {}
        }
    }

    /// The far end's E: the control channel is up once this end's is queued.
    fn on_far_e(&mut self) {
        self.far_e = true;
        if self.stage == Stage::AwaitE {
            self.up();
        }
    }

    /// Everything the control transmitter has put on the line.
    fn sent(&mut self) {
        while let Some(sent) = self.control.transmitter.sent() {
            let Sent::Ended { kind, .. } = sent else { continue };
            match (kind, self.stage) {
                (Kind::Ones, Stage::TurningOff) => self.begin_page(),
                // 12.6.1.5, 12.6.1.6, 12.6.2.5: three seconds "after sending
                // Sh followed by S-bar-h".
                (Kind::ShBar, Stage::Awaiting(Awaiting::ShOrPph) | Stage::AwaitE) => self.wait(THREE_SECONDS),
                // 12.4.3.2 and 12.4.4.2: three seconds after sending PPh.
                (Kind::Pph, Stage::Awaiting(Awaiting::FirstPph | Awaiting::Pph | Awaiting::Responding)) => {
                    self.wait(THREE_SECONDS);
                }
                _ => {}
            }
        }
    }

    /// Everything the primary receiver has to report.
    fn primary_events(&mut self) {
        let Some(recipient) = self.recipient.as_mut() else { return };
        let mut events = Vec::new();
        while let Some(event) = recipient.event() {
            events.push(event);
        }
        for event in events {
            match event {
                primary::Event::Trained { snr_db } => self.trained_snr = Some(snr_db),
                primary::Event::Phase3Over { well } => {
                    self.events.push_back(Event::Phase3Over { well });
                    if well {
                        // 12.4.2.1, and 12.4.4.1's three seconds "after
                        // receipt of the end of signal TRN".
                        self.turn = self.turn_after_phase3();
                        self.stage = Stage::Awaiting(Awaiting::FirstPph);
                        self.wait(THREE_SECONDS);
                    } else {
                        self.recover_recipient();
                    }
                }
                primary::Event::PageStarted { b1_errors } => self.events.push_back(Event::PageStarted { b1_errors }),
                primary::Event::PageEnded => {
                    self.events.push_back(Event::PageEnded);
                    if self.stage == Stage::Page {
                        self.await_after_page();
                    }
                }
            }
        }
    }

    /// The recipient, its page over: listen for PPh or Sh (12.6.2.1), with
    /// 12.6.2.4's three seconds "after the receipt of the end of primary
    /// channel data".
    fn await_after_page(&mut self) {
        if self.control.receiver.phase() == Phase::Data {
            self.control.receiver.stop();
        }
        self.turn = Turn::Page;
        self.far_e = false;
        self.stage = Stage::Awaiting(Awaiting::ShOrPph);
        self.wait(THREE_SECONDS);
    }

    /// The far end's ALT, MPh and E bits, looked through for its MPh.
    fn sync_bits(&mut self) {
        let bits = self.control.receiver.take_sync_bits();
        for bit in bits {
            if let Some(MphFound::Mph(far)) = self.finder.feed(bit) {
                self.far_mph = Some(far);
                if let Some(precoding) = far.precoding {
                    self.precoding = precoding;
                }
                // 12.4.1.3, 12.4.2.4: "received at least one MPh sequence and
                // the modem is sending MPh sequences ... complete the current
                // MPh and send a single 20-bit E sequence".
                if self.stage == Stage::Mph && self.settle() {
                    self.queue_e(true);
                }
            }
        }
    }

    /// What is watched for rather than reported.
    fn poll(&mut self) {
        self.far_off_since = if self.control.receiver.carrier() { None } else { self.far_off_since.or(Some(self.now)) };

        // 12.8.2: "After detecting signal AC for more than 100 ms", PPh and
        // ALT; and 12.8.1's collision rule, an initiator that hears AC
        // becoming the responder.
        let ac = self.control.receiver.hearing() == Hearing::Ac;
        if !ac {
            self.ac_answered = false;
        } else if !self.ac_answered && self.control.receiver.hearing_for() > control::AC_SECONDS {
            let respond = match self.stage {
                Stage::Control | Stage::Mph | Stage::AwaitE => true,
                Stage::Awaiting(_) => true,
                Stage::Page => !self.source_end,
                _ => false,
            };
            if respond {
                self.ac_answered = true;
                self.respond_to_ac();
            }
        }

        // 12.7.1.2, 12.7.2.2: the far end's phase 2 tone "for more than
        // 50 ms" while this end is on the control channel is a primary
        // channel retrain to answer. Where PPh is awaited after phase 3 it is
        // 12.3.3's recovery instead (`on_tone`).
        if self.control.receiver.hearing() == Hearing::Tone
            && self.control.receiver.hearing_for() > RETRAIN_TONE_SECONDS
            && matches!(
                self.stage,
                Stage::Control | Stage::Mph | Stage::AwaitE | Stage::Awaiting(Awaiting::ShOrPph | Awaiting::Pph | Awaiting::ChangePph)
            )
        {
            self.begin_phase2_again(false);
            return;
        }

        match self.stage {
            Stage::Phase3 if self.source_end => {
                // 12.3.1.3: TRN over, the control channel follows -- 70 ms of
                // silence and PPh (12.4.1.1).
                if self.source.as_ref().is_some_and(|s| s.sending() == Sending::Idle) {
                    self.begin_startup(self.turn_after_phase3());
                    // A tone already there is 12.3.3's recovery under way.
                    if self.control.receiver.hearing() == Hearing::Tone {
                        self.recover_source();
                    }
                }
            }
            Stage::Page if self.source_end && self.page_ending => {
                if self.source.as_ref().is_some_and(|s| s.sending() == Sending::Idle) {
                    self.page_ending = false;
                    // 12.6.1.1: a new rate wanted, PPh (12.4.1.1); otherwise
                    // Sh and S-bar-h.
                    if self.renegotiate { self.begin_startup(Turn::Page) } else { self.begin_resync() }
                }
            }
            Stage::Recovering { tone_heard, infoh_sent } => {
                if self.source_end {
                    if self.tone_pending && !self.control.transmitter.is_sending() {
                        self.tone_pending = false;
                        self.tone.send(&[]);
                    }
                } else if infoh_sent {
                    if !self.tone.is_sending() {
                        // 12.3.2.1: "After sending INFOh, the recipient modem
                        // transmits silence and conditions its receiver to
                        // detect S".
                        if let Some(recipient) = self.recipient.as_mut() {
                            recipient.expect_phase3();
                        }
                        self.stage = Stage::Phase3;
                        self.deadline = None;
                    }
                } else if tone_heard.is_some_and(|at| self.now >= at + self.seconds(TONE_BEFORE_INFOH)) {
                    self.send_infoh();
                }
            }
            _ => {}
        }
    }

    /// A wait has run out.
    fn on_deadline(&mut self) {
        self.deadline = None;
        match self.stage {
            Stage::Awaiting(Awaiting::Ac) => {
                self.ac_rounds += 1;
                if self.ac_rounds >= AC_ROUNDS {
                    self.fail("the far end did not answer AC");
                } else {
                    self.wait(THREE_SECONDS);
                }
            }
            // 12.2.1.4.3, 12.2.2.3.3: the source's tone not heard, INFOh
            // goes anyway.
            Stage::Recovering { infoh_sent: false, .. } if !self.source_end => self.send_infoh(),
            Stage::Recovering { .. } if self.source_end => self.fail("no INFOh after the recipient's tone"),
            Stage::Recovering { .. } | Stage::Phase2 | Stage::Phase3 | Stage::Control | Stage::TurningOff | Stage::Page | Stage::Failed => {}
            // 12.4.3.2 to 12.4.3.4, 12.4.4.1 to 12.4.4.3, 12.6.1.5, 12.6.1.6,
            // 12.6.2.4, 12.6.2.5: "initiate a control channel retrain as
            // defined in 12.8.1".
            Stage::Awaiting(_) | Stage::Mph | Stage::AwaitE => self.begin_retrain(),
        }
    }

    /// What the start-up after phase 3 is to the join: the first, or the
    /// tail of a primary channel retrain (12.7) begun on the control channel.
    fn turn_after_phase3(&self) -> Turn {
        if self.turn == Turn::Retrain { Turn::Retrain } else { Turn::First }
    }

    /// The source's control channel start-up (12.4.1.1): 70 ms of silence,
    /// PPh, ALT for at least 16T; the far PPh awaited. Straight after phase
    /// 3 -- the first time, or after a 12.7 retrain -- the recipient's tone
    /// may come instead of PPh (12.4.3.1), so that wait has its own name.
    fn begin_startup(&mut self, turn: Turn) {
        self.turn = turn;
        self.control.transmitter.queue(Segment::Silence(control::SILENCE_SYMBOLS));
        self.queue_pph();
        self.control.transmitter.queue(Segment::Alt { at_least: control::ALT_LEAST });
        self.stage = Stage::Awaiting(if turn == Turn::Page { Awaiting::Pph } else { Awaiting::FirstPph });
        self.deadline = None;
    }

    /// The source's resynchronisation (12.6.1.1, 12.6.1.2): 70 ms of silence,
    /// Sh for 24T, S-bar-h for 8T, then ALT while PPh or Sh is listened for.
    fn begin_resync(&mut self) {
        self.turn = Turn::Page;
        self.far_e = false;
        let tx = &mut self.control.transmitter;
        tx.queue(Segment::Silence(control::SILENCE_SYMBOLS));
        tx.queue(Segment::Sh(control::SH_SYMBOLS));
        tx.queue(Segment::ShBar(control::SH_BAR_SYMBOLS));
        tx.queue(Segment::Alt { at_least: control::ALT_LEAST });
        self.stage = Stage::Awaiting(Awaiting::ShOrPph);
        self.deadline = None;
    }

    /// PPh queued: a start-up begins here, so what the last one settled of
    /// the far MPh and the precoder is forgotten -- "set to 0 before the
    /// first MPh sequence is received during a control channel start-up"
    /// (10.2.4.4).
    fn queue_pph(&mut self) {
        self.control.transmitter.queue(Segment::Pph(self.reading));
        self.far_mph = None;
        self.precoding = [(0, 0); 3];
        self.finder = MphFinder::new();
        self.far_e = false;
    }

    /// PPh in answer to the far end's, then ALT (12.4.2.1, 12.4.2.2,
    /// 12.6.1.3, 12.6.2.1, 12.8.1).
    fn answer_pph(&mut self) {
        self.queue_pph();
        self.control.transmitter.queue(Segment::Alt { at_least: control::ALT_LEAST });
    }

    /// MPh, over and over, behind whatever is going (12.4.1.2, 12.4.2.3);
    /// E straight after the first if the far end's MPh is in already.
    fn send_mph(&mut self) {
        self.mph = self.make_mph();
        self.control.transmitter.queue(Segment::Repeat(self.mph.to_bits()));
        self.stage = Stage::Mph;
        // 12.4.3.3, 12.4.4.2: three seconds for the far MPh.
        self.wait(THREE_SECONDS);
        if self.far_mph.is_some() && self.settle() {
            self.queue_e(true);
        }
    }

    /// The recipient's answer to Sh and S-bar-h: the same back, ALT for 16T
    /// and E (12.6.2.2) -- or PPh and ALT when a change is wanted (12.6.2.3).
    fn answer_sh(&mut self) {
        let tx = &mut self.control.transmitter;
        tx.clear();
        self.turn = Turn::Page;
        if self.renegotiate {
            self.answer_pph();
            self.stage = Stage::Awaiting(Awaiting::ChangePph);
            self.wait(THREE_SECONDS);
            return;
        }
        let tx = &mut self.control.transmitter;
        tx.queue(Segment::Sh(control::SH_SYMBOLS));
        tx.queue(Segment::ShBar(control::SH_BAR_SYMBOLS));
        tx.queue(Segment::Alt { at_least: control::ALT_LEAST });
        self.queue_e(false);
    }

    /// E, and data behind it. `from_mph` starts 12.4.3.4's and 12.4.4.3's
    /// three seconds; a resynchronisation's run from S-bar-h already.
    fn queue_e(&mut self, from_mph: bool) {
        self.control.transmitter.queue(Segment::E);
        self.control.transmitter.queue(Segment::Data);
        self.stage = Stage::AwaitE;
        if from_mph {
            self.wait(THREE_SECONDS);
        }
        if self.far_e {
            self.up();
        }
    }

    /// The control channel is up.
    fn up(&mut self) {
        self.stage = Stage::Control;
        self.deadline = None;
        self.far_e = false;
        self.renegotiate = false;
        self.ac_rounds = 0;
        self.far_off_since = None;
        // Nothing read before this moment is the far end's data.
        self.control.receiver.take_bits();
        self.events.push_back(Event::ControlUp);
    }

    /// The 4T of ones are out: the page (12.5.1), or the wait for it
    /// (12.5.2).
    fn begin_page(&mut self) {
        let Some(data) = self.data else { return self.fail("no data mode for the page") };
        let begun = match (self.source.as_mut(), self.recipient.as_mut()) {
            (Some(source), _) => source.page(data),
            (_, Some(recipient)) => recipient.expect_page(data),
            _ => false,
        };
        if !begun {
            return self.fail("Table 8 has no such data rate at this symbol rate");
        }
        self.page_ending = false;
        self.stage = Stage::Page;
        self.deadline = None;
    }

    /// A control channel retrain, from this end (12.8.1): AC until the far
    /// end's PPh.
    fn begin_retrain(&mut self) {
        if let Some(recipient) = self.recipient.as_mut() {
            recipient.stop();
        }
        self.control.transmitter.clear();
        self.control.transmitter.queue(Segment::Ac);
        self.turn = Turn::Retrain;
        self.far_mph = None;
        self.far_e = false;
        self.page_ending = false;
        self.stage = Stage::Awaiting(Awaiting::Ac);
        self.wait(THREE_SECONDS);
        self.retrains += 1;
        self.events.push_back(Event::Retraining);
    }

    /// The far end's AC, heard for 100 ms: PPh and ALT (12.8.2), the
    /// initiator's PPh awaited.
    fn respond_to_ac(&mut self) {
        if let Some(recipient) = self.recipient.as_mut() {
            recipient.stop();
        }
        // An initiator that hears AC becomes the responder (12.8.1): the
        // same retrain, told of once.
        let already = self.stage == Stage::Awaiting(Awaiting::Ac);
        self.control.transmitter.clear();
        self.turn = Turn::Retrain;
        self.page_ending = false;
        self.answer_pph();
        self.stage = Stage::Awaiting(Awaiting::Responding);
        self.wait(THREE_SECONDS);
        if !already {
            self.retrains += 1;
            self.events.push_back(Event::Retraining);
        }
    }

    /// The recipient's way back from a phase 3 that failed (12.3.3): its
    /// tone, the source's awaited.
    fn recover_recipient(&mut self) {
        self.recoveries += 1;
        if self.recoveries > PHASE3_TRIES {
            return self.fail("phase 3 failed too often");
        }
        self.tone.send(&[]);
        self.stage = Stage::Recovering { tone_heard: None, infoh_sent: false };
        self.wait(TONE_WAIT);
    }

    /// INFOh again, 25 ms into the tone after the source's is heard, or
    /// blind (12.2.1.2.6, 12.2.1.4.3).
    fn send_infoh(&mut self) {
        let Stage::Recovering { tone_heard, .. } = self.stage else { return };
        self.tone.send(&self.infoh.to_bits());
        self.tone.silence();
        self.stage = Stage::Recovering { tone_heard, infoh_sent: true };
        self.deadline = None;
    }

    /// The source's answer to the recipient's tone where PPh was expected
    /// (12.4.3.1): its own tone, and INFOh listened for.
    fn recover_source(&mut self) {
        self.recoveries += 1;
        if self.recoveries > PHASE3_TRIES {
            return self.fail("phase 3 failed too often");
        }
        // Whatever is going -- ALT runs until something is queued behind
        // it -- ends cleanly through the pulse, and the tone follows.
        self.control.transmitter.clear();
        self.control.transmitter.queue(Segment::Silence(1));
        self.tone_pending = true;
        self.infoh_rx = Some(dpsk::Receiver::half_duplex(side_of(other(self.role)), self.fs));
        self.stage = Stage::Recovering { tone_heard: None, infoh_sent: false };
        self.wait(INFOH_WAIT);
    }

    /// INFOh from the recipient, at a source answering its tone: phase 3
    /// again (12.4.3.1 -> 12.3.1), on whatever the new INFOh asks.
    fn on_info(&mut self, info: Info) {
        let Info::InfoH(infoh) = info else { return };
        if !matches!(self.stage, Stage::Recovering { .. }) || !self.source_end {
            return;
        }
        self.tone.stop();
        self.tone_pending = false;
        self.infoh_rx = None;
        if infoh != self.infoh {
            self.infoh = infoh;
            self.channel = channel_of(&infoh);
            self.source = Some(primary::Source::new(self.channel, mode_of(self.role), self.fs));
        }
        if let Some(source) = self.source.as_mut() {
            source.phase3();
        }
        self.stage = Stage::Phase3;
        self.deadline = None;
    }

    /// Phase 2 is done: INFOh, the far INFO0, and phase 3 on them.
    fn finish_phase2(&mut self) {
        let Some(phase2) = self.phase2.as_ref() else { return };
        let Some(infoh) = phase2.infoh() else { return self.fail("phase 2 done without INFOh") };
        self.infoh = infoh;
        self.channel = channel_of(&infoh);
        self.ours = phase2.capabilities();
        self.far = phase2.far_capabilities().unwrap_or_default();
        self.phase2_done = true;
        self.events.push_back(Event::Phase2Over);
        self.begin_phase3();
    }

    /// Phase 3 (12.3): the source's burst begins with its 70 ms of silence,
    /// the recipient listens for S.
    fn begin_phase3(&mut self) {
        if self.source_end {
            let mut source = primary::Source::new(self.channel, mode_of(self.role), self.fs);
            source.phase3();
            self.source = Some(source);
        } else {
            let mut recipient = primary::Recipient::new(self.channel, mode_of(other(self.role)), self.fs);
            recipient.expect_phase3();
            self.recipient = Some(recipient);
        }
        self.stage = Stage::Phase3;
        self.deadline = None;
    }

    /// A primary channel retrain (12.7), begun here or answered: everything
    /// else falls silent, and phase 2 runs again from its tones. When
    /// `initiating`, the far end has yet to hear of it, and its control
    /// carrier is kept out of phase 2's ears until it goes (see `step`).
    fn begin_phase2_again(&mut self, initiating: bool) {
        self.control.transmitter.stop();
        self.control.receiver.stop();
        self.deaf_above = (initiating && self.control.receiver.carrier()).then(|| self.control.receiver.level());
        if let Some(recipient) = self.recipient.as_mut() {
            recipient.stop();
        }
        self.tone.stop();
        self.infoh_rx = None;
        self.tone_pending = false;
        self.phase2 = Some(phase2h::Modem::retrain(self.role, part_of(self.source_end), self.fs, self.ours, self.far));
        self.turn = Turn::Retrain;
        self.stage = Stage::Phase2;
        self.deadline = None;
        self.far_e = false;
        self.page_ending = false;
        self.retrains += 1;
        self.events.push_back(Event::Retraining);
    }

    /// This end's MPh (Tables 23 and 24, and `wp-ab.md` "For G").
    ///
    /// The source offers every rate Table 8 has at the symbol rate, up to
    /// the cap; the recipient no more than its receiver could take by the
    /// signal to noise it trained to, with the allowance duplex's MP and
    /// phase 2's projections make (`training.rs`). Rates above 28 800 only
    /// where the far end's INFO0 has the 1664-point constellation (NOTE 1).
    /// Sixteen states, no non-linear encoding and minimum shaping asked of
    /// the source, since the recipient's slicer is linear (`wp-e.md`); Type
    /// 0 from both ends, the source's bits 29 to 32 zero (NOTE 2); no
    /// asymmetric control rates (F.3.1.4/T.30).
    fn make_mph(&self) -> Mph {
        let rate = self.channel.band.rate;
        let wide = if self.far.constellation_1664 { 14 } else { 12 };
        let by_line = match (self.source_end, self.trained_snr) {
            (false, Some(snr_db)) => rate_by_snr(snr_db, self.channel.band.baud()),
            _ => 14,
        };
        let rates = (1..=14u8)
            .filter(|&n| Framing::new(rate, u32::from(n) * 2400, false, false).is_some())
            .fold(0u16, |mask, n| mask | 1 << (n - 1));
        // Never below the least rate the symbol rate has: 2400 bit/s is
        // 2400 Bd's alone (Table 8), and a cap or a poor line asking for it
        // elsewhere would leave the two MPh with no rate in common.
        let least = rates.trailing_zeros() as u8 + 1;
        let max_rate = probe::ceiling(rate).min(wide).min(by_line).min(self.cap).max(least);
        Mph {
            max_rate,
            control_rate: self.control_rate,
            trellis: Trellis::States16,
            non_linear: false,
            expanded_shaping: false,
            rates,
            asymmetric_control: false,
            precoding: None,
        }
    }

    /// Both MPh in hand: the primary channel's rate and data mode, and the
    /// control channel's rates (12.4.1.3, 12.4.1.4, 12.4.2.4, 12.4.2.5).
    /// False, and the modem failed, if the two share no rate.
    fn settle(&mut self) -> bool {
        let Some(far) = self.far_mph else { return false };
        let ours = self.mph;
        let (source, recipient) = if self.source_end { (&ours, &far) } else { (&far, &ours) };
        let Some(n) = mp::primary_rate(source, recipient) else {
            self.fail("the two MPh share no primary rate");
            return false;
        };
        self.data = Some(DataMode {
            rate: u32::from(n) * 2400,
            code: code_of(recipient.trellis),
            nonlinear: recipient.non_linear,
            expanded: recipient.expanded_shaping,
            // The recipient's own MPh asks for none.
            precoding: if self.source_end { self.precoding } else { [(0, 0); 3] },
        });
        let (transmit, receive) = mp::control_rates(&ours, &far);
        self.control.transmitter.set_rate(rate_of(transmit));
        self.control.receiver.set_rate(rate_of(receive));
        self.rates = Some((transmit, receive));
        true
    }

    fn fail(&mut self, why: &'static str) {
        self.stage = Stage::Failed;
        self.failure = Some(why);
        self.deadline = None;
        self.control.transmitter.stop();
        self.tone.stop();
        self.infoh_rx = None;
        if let Some(source) = self.source.as_mut()
            && source.sending() == Sending::Data
        {
            source.end_page();
        }
        if let Some(recipient) = self.recipient.as_mut() {
            recipient.stop();
        }
        self.events.push_back(Event::Failed(why));
    }
}

/// What this end's receiver could take, as a multiple of 2400, by the
/// signal to noise it trained to: `training.rs`'s rule for duplex's MP, the
/// same 6 dB allowance as phase 2's projections.
fn rate_by_snr(snr_db: f64, baud: f64) -> u8 {
    let snr = 10f64.powf(snr_db.min(60.0) / 10.0);
    let bits = (1.0 + snr / 10f64.powf(0.6)).log2();
    ((bits * baud / 2400.0).floor() as u8).clamp(1, 14)
}

/// The primary channel INFOh chose (Table 22).
fn channel_of(infoh: &InfoH) -> Channel {
    Channel {
        band: Band::new(infoh.symbol_rate, infoh.high_carrier),
        pre_emphasis: infoh.pre_emphasis,
        reduction: infoh.power_reduction,
        trn_size: infoh.trn_size,
        trn_steps: infoh.trn_length,
    }
}

/// Phase 2's name for which end sends the page.
fn part_of(source: bool) -> Part {
    if source { Part::Source } else { Part::Recipient }
}

/// The other end of the call.
fn other(role: Role) -> Role {
    match role {
        Role::Call => Role::Answer,
        Role::Answer => Role::Call,
    }
}

fn side_of(role: Role) -> Side {
    match role {
        Role::Call => Side::Call,
        Role::Answer => Side::Answer,
    }
}

/// The scrambler a role sends with: GPC for the call modem, GPA for the
/// answer modem (clause 7).
fn mode_of(role: Role) -> Mode {
    match role {
        Role::Call => Mode::Call,
        Role::Answer => Mode::Answer,
    }
}

fn rate_of(rate: ControlRate) -> Rate {
    match rate {
        ControlRate::Bps1200 => Rate::R1200,
        ControlRate::Bps2400 => Rate::R2400,
    }
}

fn bits_per_second(rate: ControlRate) -> u32 {
    rate_of(rate).bits_per_second()
}

fn code_of(trellis: Trellis) -> Code {
    match trellis {
        Trellis::States16 => Code::States16,
        Trellis::States32 => Code::States32,
        Trellis::States64 => Code::States64,
    }
}
