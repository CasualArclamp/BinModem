//! Phase 2 of the start-up (11.2): probing and ranging.
//!
//! What happens, from the end that dialled (Figure 16):
//!
//! 1. Both ends send an INFO0 and follow it with a tone, the call modem
//!    tone B at 1200 Hz and the answer modem tone A at 2400 (11.2.1.1.1,
//!    11.2.1.2.1).
//! 2. The answer modem, hearing tone B, reverses tone A. The call modem
//!    answers with a reversal of tone B exactly 40 ms after hearing that one,
//!    and the answer modem answers that with another, again 40 ms on. Each
//!    end now has a reversal of its own and the far end's answer to it, and
//!    the time between them less the 40 ms is the round trip (11.2.1.1.4,
//!    11.2.1.2.4).
//! 3. The answer modem sends L1 and L2 and the call modem measures the line
//!    with them; then the other way round, after one more pair of reversals
//!    (11.2.1.1.5 to 11.2.1.1.7, 11.2.1.2.5 to 11.2.1.2.8).
//! 4. The call modem sends INFO1c, what it found; the answer modem answers
//!    with INFO1a, what the two of them will use (11.2.1.1.8, 11.2.1.2.9).
//!
//! Each end listens on the other's frequency throughout -- tone A and INFO
//! from the answer modem are on 2400 Hz, tone B and INFO from the call modem
//! on 1200 -- and the probing signal leaves out 1200 and 2400 Hz, so that
//! each end can hear the other's tone over the echo of its own L2.

use dsp::{ReversalDetector, ToneDetector};

use super::dpsk::{self, Side};
use super::info::{Info, Info0, Info1a, Info1c, Probed, SymbolRate};
use super::probe::{self, Analyzer, Reading};

/// Which end of the call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Call,
    Answer,
}

impl Role {
    fn side(self) -> Side {
        match self {
            Self::Call => Side::Call,
            Self::Answer => Side::Answer,
        }
    }

    fn far(self) -> Side {
        match self {
            Self::Call => Side::Answer,
            Self::Answer => Side::Call,
        }
    }
}

/// How phase 2 is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// INFO1a is across, and phase 3 is next.
    Done,
    /// It went wrong, and why. Where the recommendation would retrain, this
    /// stops: a retrain starts phase 2 again from its top, and nothing after
    /// phase 2 exists here to make going round again worth it.
    Failed(&'static str),
}

/// "40 ± 1 ms" from a far reversal arriving to the answering one leaving
/// (11.2.1.1.3, 11.2.1.1.6 and 11.2.1.2.5).
const TURN: f64 = 0.040;

/// A tone goes on "for another 10 ms after the phase reversal".
const AFTER_REVERSAL: f64 = 0.010;

/// The answer modem's tone A before its reversals: "at least 50 ms" in
/// 11.2.1.2.3 and exactly 50 in 11.2.1.2.6.
const TONE_A_FIRST: f64 = 0.050;

/// How long L2 is read for: "a period of time not to exceed 500 ms".
const L2_READ: f64 = 0.500;

/// What is let go past before reading L2, so the windows see the line settled
/// on it rather than the step from L1.
const L2_SETTLE: f64 = 0.020;

/// 11.2.2.1.3 and 11.2.2.2.2: a reversal expected back within 2000 ms.
const REVERSAL_WAIT: f64 = 2.0;

/// Amplitude a tone has to reach before it counts as there, as V.32's start-up
/// uses.
const AUDIBLE: f64 = 0.008;

/// Bandwidth of the reversal detector, which V.32's start-up has measured the
/// latency of against real calls.
const REVERSAL_BANDWIDTH: f64 = 60.0;

/// How much of what is 150 Hz either side a tone has to reach to count as the
/// far end's tone rather than this end's own L2 leaking into its detector.
///
/// L2 has tones 150 Hz either side of both 1200 and 2400 Hz, and the echo of
/// this end's own L2 can be far louder than the far end's tone -- on a line
/// that loses 30 dB and reflects 15, by fifteen decibels. So the far tone is
/// not asked to stand above its neighbours, only above what they leak. The
/// detectors are 10 Hz wide, which lets a tone 150 Hz away through at about
/// one per cent; a twentieth is five times that.
const OVER_LEAKAGE: f64 = 0.05;

/// Width of the presence detectors.
const PRESENCE_BANDWIDTH: f64 = 10.0;

/// How long a tone has to be there before it is believed.
const TONE_HELD: f64 = 0.020;

/// No part of phase 2 takes this long, round trips and all.
const GIVE_UP: f64 = 20.0;

/// Where phase 2 has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    // The call modem.
    CallInfo0,
    CallFirstReversal,
    CallRanging,
    CallReadProbe,
    CallAwaitTone,
    CallSendProbe,
    CallInfo1,
    // The answer modem.
    AnswerInfo0,
    AnswerAwaitTone,
    AnswerRanging,
    AnswerSendProbe,
    AnswerProbeReversal,
    AnswerReadProbe,
    AnswerInfo1,
    // Both.
    Finished,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::CallInfo0 | Self::AnswerInfo0 => "V.34 INFO0",
            Self::CallFirstReversal | Self::AnswerAwaitTone => "V.34 tones",
            Self::CallRanging | Self::AnswerRanging => "V.34 ranging",
            Self::CallReadProbe | Self::AnswerReadProbe => "V.34 reading the probe",
            Self::CallAwaitTone | Self::AnswerProbeReversal => "V.34 tones again",
            Self::CallSendProbe | Self::AnswerSendProbe => "V.34 sending the probe",
            Self::CallInfo1 | Self::AnswerInfo1 => "V.34 INFO1",
            Self::Finished => "V.34 phase 2 done",
        }
    }
}

/// What is on the line from this end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speaking {
    Silent,
    /// An INFO sequence or a tone, through the DPSK modulator.
    Carrier,
    /// L1 until the sample given, and L2 after it.
    Probe { l1_until: u64 },
}

/// A tone that is there and standing clear of what is either side of it.
#[derive(Debug, Clone)]
struct Presence {
    tone: ToneDetector,
    below: ToneDetector,
    above: ToneDetector,
    held: u64,
}

impl Presence {
    fn new(freq: f64, fs: f64) -> Self {
        Self {
            tone: ToneDetector::new(freq, PRESENCE_BANDWIDTH, fs),
            below: ToneDetector::new(freq - 150.0, PRESENCE_BANDWIDTH, fs),
            above: ToneDetector::new(freq + 150.0, PRESENCE_BANDWIDTH, fs),
            held: 0,
        }
    }

    /// Whether what is either side of the tone is louder than the tone, which
    /// is what a probing signal looks like from here: L2 has tones 150 Hz
    /// either side of 1200 and 2400 Hz and nothing at either.
    fn probing(&self) -> bool {
        self.below.amplitude().max(self.above.amplitude()) > self.tone.amplitude()
    }

    fn feed(&mut self, x: f64) {
        self.tone.feed(x);
        self.below.feed(x);
        self.above.feed(x);
        let there = self.tone.amplitude() > AUDIBLE
            && self.tone.amplitude() > OVER_LEAKAGE * self.below.amplitude().max(self.above.amplitude());
        self.held = if there { self.held + 1 } else { 0 };
    }
}

/// Phase 2, one end of it.
#[derive(Debug, Clone)]
pub struct Modem {
    role: Role,
    fs: f64,
    /// Samples since phase 2 began.
    now: u64,
    stage: Stage,
    status: Status,
    /// When the stage began, and a deadline for it where it has one.
    since: u64,
    deadline: Option<u64>,

    tx: dpsk::Transmitter,
    probe: probe::Generator,
    speaking: Speaking,
    /// A reversal of this end's tone, due at this sample.
    reverse_at: Option<u64>,
    /// When this end's last reversal went out.
    reversed_at: Option<u64>,
    /// This end's tone stops at this sample.
    silence_at: Option<u64>,
    /// L1 starts at this sample.
    probe_at: Option<u64>,

    rx: dpsk::Receiver,
    reversals: ReversalDetector,
    presence: Presence,
    /// Reversals detected before this sample are not tone reversals but the
    /// tail of an INFO sequence, which is DPSK and full of them.
    ignore_reversals_until: u64,
    analyzer: Analyzer,
    /// The stretch of L2 being read.
    read_from: u64,
    read_until: u64,

    ours: Info0,
    far: Option<Info0>,
    /// When the far end's INFO0 was last received, for noticing it come again.
    far_info0_count: u32,
    first_reversal_at: Option<u64>,
    round_trip: Option<u64>,
    reading: Option<Reading>,
    info1c: Option<Info1c>,
    info1a: Option<Info1a>,
    /// Times a timeout of 11.2.2 was taken instead of the signal it waited for.
    recoveries: u32,
}

impl Modem {
    /// Every field at rest: silent, at the finished stage, nothing heard. The
    /// two ways in fill in the rest.
    fn blank(role: Role, fs: f64) -> Self {
        let far_tone = role.far().carrier();
        Self {
            role,
            fs,
            now: 0,
            stage: Stage::Finished,
            status: Status::Running,
            since: 0,
            deadline: None,
            tx: dpsk::Transmitter::new(role.side(), fs),
            probe: probe::Generator::new(fs),
            speaking: Speaking::Silent,
            reverse_at: None,
            reversed_at: None,
            silence_at: None,
            probe_at: None,
            rx: dpsk::Receiver::new(role.far(), fs),
            reversals: ReversalDetector::new(far_tone, REVERSAL_BANDWIDTH, AUDIBLE, fs),
            presence: Presence::new(far_tone, fs),
            ignore_reversals_until: 0,
            analyzer: Analyzer::new(fs),
            read_from: u64::MAX,
            read_until: u64::MAX,
            ours: Self::capabilities(),
            far: None,
            far_info0_count: 0,
            first_reversal_at: None,
            round_trip: None,
            reading: None,
            info1c: None,
            info1a: None,
            recoveries: 0,
        }
    }

    /// Phase 2 from its start: the 75 ms of silence that end phase 1 have
    /// already gone.
    pub fn new(role: Role, fs: f64) -> Self {
        let mut modem = Self::blank(role, fs);
        modem.stage = match role {
            Role::Call => Stage::CallInfo0,
            Role::Answer => Stage::AnswerInfo0,
        };
        modem.speaking = Speaking::Carrier;
        // 11.2.1.1.1 and 11.2.1.2.1: INFO0 "with bit 28 set to 0, followed by"
        // this end's tone -- which the modulator carries on into by itself.
        let bits = modem.ours.to_bits();
        modem.tx.send(&bits);
        modem
    }

    /// Phase 2 as a retrain (11.5): the capabilities were settled the first
    /// time and are not exchanged again, so INFO0 is skipped and this end goes
    /// straight to its tone and the reversal handshake. `far` is what the far
    /// end's INFO0 said the first time round, kept so INFO1 has it.
    ///
    /// 11.5.1.2 has the responding call modem "transmit Tone B ... and proceed
    /// in accordance with 11.2.1.1.3", which is the reversal this end waits for
    /// in [`Stage::CallFirstReversal`]. 11.5.2.2 has the responding answer
    /// modem "transmit Tone A and proceed in accordance with 11.2.1.2.3", which
    /// is [`Stage::AnswerAwaitTone`]. An end initiating (11.5.1.1, 11.5.2.1)
    /// starts its tone first and waits for the far end's; from the reversal on
    /// the two are the same, so both enter the same way.
    pub fn retrain(role: Role, fs: f64, far: Info0) -> Self {
        let mut modem = Self::blank(role, fs);
        modem.far = Some(far);
        modem.far_info0_count = 1;
        modem.ours.acknowledge = true;
        modem.start_tone();
        // The step from data or a renegotiation tone into this one is not a
        // reversal, however it reads.
        modem.ignore_reversals_until = modem.ms(0.050);
        modem.stage = match role {
            Role::Call => Stage::CallFirstReversal,
            Role::Answer => Stage::AnswerAwaitTone,
        };
        modem
    }

    /// What this end says it can do.
    ///
    /// Everything V.34 has, since everything V.34 has is what is being
    /// built: phase 2 asks for the far end's honest projections, and a modem
    /// that said less would only be told less.
    fn capabilities() -> Info0 {
        Info0 {
            rate_2743: true,
            rate_2800: true,
            rate_3429: true,
            low_carrier_3000: true,
            high_carrier_3000: true,
            low_carrier_3200: true,
            high_carrier_3200: true,
            transmit_3429: true,
            can_reduce_power: false,
            asymmetry: 5,
            cme: false,
            constellation_1664: true,
            clock: 0,
            acknowledge: false,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn phase(&self) -> &'static str {
        match self.status {
            Status::Failed(_) => "V.34 phase 2 failed",
            _ => self.stage.name(),
        }
    }

    /// The far end's INFO0, once it has arrived.
    pub fn far_capabilities(&self) -> Option<Info0> {
        self.far
    }

    /// The round trip, in seconds, once it has been measured.
    pub fn round_trip(&self) -> Option<f64> {
        self.round_trip.map(|n| n as f64 / self.fs)
    }

    /// What this end made of the far end's L2.
    pub fn reading(&self) -> Option<&Reading> {
        self.reading.as_ref()
    }

    /// How many times a timeout of 11.2.2 was taken instead of the signal
    /// it was waiting for. Phase 2 survives them, and a clean line should
    /// need none.
    pub fn recoveries(&self) -> u32 {
        self.recoveries
    }

    pub fn info1c(&self) -> Option<Info1c> {
        self.info1c
    }

    pub fn info1a(&self) -> Option<Info1a> {
        self.info1a
    }

    fn ms(&self, seconds: f64) -> u64 {
        (seconds * self.fs).round() as u64
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.since = self.now;
        self.deadline = None;
    }

    fn fail(&mut self, why: &'static str) {
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
        self.tx.stop();
        self.speaking = Speaking::Silent;
    }

    /// Carry phase 2 one sample further: hear `line`, and say what goes on it.
    pub fn step(&mut self, line: f64) -> f64 {
        self.now += 1;
        let info = self.rx.feed(line);
        self.presence.feed(line);
        // A probing signal -- the far end's, or the echo of this end's own --
        // leaves the reversal detector sure the tone after it is far off
        // frequency, and it refuses that tone's reversal. Tone A reverses 50 ms
        // after the far end's L2 ends (11.2.1.2.6), so the detector starts
        // again for as long as probing is what it hears.
        if self.presence.probing() {
            self.reversals.restart();
        }
        let reversed = self.reversals.feed(line) && self.now >= self.ignore_reversals_until;
        // Where the reversal was on the line, as against where it was noticed.
        let reversal = reversed.then(|| self.now.saturating_sub(u64::from(self.reversals.latency())));
        if (self.read_from..self.read_until).contains(&self.now) {
            self.analyzer.feed(line);
        }

        if self.status == Status::Running {
            if self.now > self.ms(GIVE_UP) {
                self.fail("phase 2 went on for twenty seconds");
            } else {
                self.timers();
                if let Some(info) = info {
                    self.heard(info);
                }
                if let Some(at) = reversal {
                    self.reversal(at);
                }
                self.stage_step();
            }
        }
        self.speak()
    }

    /// Everything scheduled for this sample.
    fn timers(&mut self) {
        if self.reverse_at == Some(self.now) {
            self.reverse_at = None;
            self.tx.reverse();
            self.reversed_at = Some(self.now);
            self.silence_at = Some(self.now + self.ms(AFTER_REVERSAL));
        }
        if self.silence_at == Some(self.now) {
            self.silence_at = None;
            self.tx.stop();
            self.speaking = Speaking::Silent;
            if let Some(at) = self.probe_at.take() {
                // The tone's ten milliseconds are up and L1 follows at once.
                debug_assert!(at <= self.now + 1);
                self.speaking = Speaking::Probe { l1_until: self.now + self.ms(probe::L1_SECONDS) };
            }
        }
    }

    fn speak(&mut self) -> f64 {
        match self.speaking {
            Speaking::Silent => 0.0,
            Speaking::Carrier => self.tx.next_sample(),
            Speaking::Probe { l1_until } => self.probe.next_sample(self.now < l1_until),
        }
    }

    fn start_tone(&mut self) {
        self.tx.send(&[]);
        self.speaking = Speaking::Carrier;
    }

    fn heard(&mut self, info: Info) {
        match (self.role, info) {
            (_, Info::Info0(far)) => {
                self.far = Some(far);
                self.far_info0_count += 1;
                // Its tail is fill bits, which are reversals.
                self.ignore_reversals_until = self.now + self.ms(0.040);
                // "The call modem shall set bit 28 of sequence INFO0c to 1
                // after correctly receiving INFO0a", and the answer modem the
                // same of INFO0a.
                self.ours.acknowledge = true;
                if self.far_info0_count > 1 && matches!(self.stage, Stage::CallInfo0 | Stage::AnswerInfo0) {
                    // 11.2.2.1.1 and 11.2.2.2.1: the far end is repeating its
                    // INFO0, so it did not get ours. Once more, now saying
                    // that its own arrived.
                    let bits = self.ours.to_bits();
                    self.tx.send(&bits);
                }
            }
            (Role::Answer, Info::Info1c(info1c)) if self.stage == Stage::AnswerInfo1 => {
                self.info1c = Some(info1c);
                let bits = self.settle(&info1c).to_bits();
                // Kept as it went, which is to the field's own resolution.
                self.info1a = Info1a::from_bits(&bits);
                // Straight on from tone A, as one group with it.
                self.tx.send(&bits);
                self.tx.silence();
                self.speaking = Speaking::Carrier;
            }
            (Role::Call, Info::Info1a(info1a)) if self.stage == Stage::CallInfo1 => {
                self.info1a = Some(info1a);
                self.status = Status::Done;
                self.enter(Stage::Finished);
            }
            _ => {}
        }
    }

    fn reversal(&mut self, at: u64) {
        match self.stage {
            // 11.2.1.1.3: the answer modem's first reversal, answered 40 ms
            // later.
            Stage::CallFirstReversal => {
                self.first_reversal_at = Some(at);
                self.reverse_at = Some((at + self.ms(TURN)).max(self.now + 1));
                self.enter(Stage::CallRanging);
                self.deadline = Some(at + self.ms(REVERSAL_WAIT));
            }
            // 11.2.1.1.4: its second, and the round trip.
            Stage::CallRanging if self.reversed_at.is_some_and(|ours| at > ours) => {
                let ours = self.reversed_at.expect("checked");
                self.round_trip = Some((at - ours).saturating_sub(self.ms(TURN)));
                // L1 begins 10 ms after that reversal and runs 160 ms, and L2
                // is read for 500 after it.
                let l2 = at + self.ms(AFTER_REVERSAL + probe::L1_SECONDS);
                self.read_from = l2 + self.ms(L2_SETTLE);
                self.read_until = l2 + self.ms(L2_READ);
                self.analyzer.reset();
                self.enter(Stage::CallReadProbe);
            }
            // 11.2.1.1.6: the answer modem's reversal after its L2, answered
            // 40 ms later with tone B's, then L1 and L2.
            Stage::CallAwaitTone if self.presence.held > 0 || self.now - self.since > self.ms(0.1) => {
                self.reverse_at = Some((at + self.ms(TURN)).max(self.now + 1));
                self.probe_at = self.reverse_at.map(|r| r + self.ms(AFTER_REVERSAL));
                self.enter(Stage::CallSendProbe);
            }
            // 11.2.1.2.4: the call modem's answer to ours, and the round trip.
            Stage::AnswerRanging if self.reversed_at.is_some_and(|ours| at > ours) => {
                let ours = self.reversed_at.expect("checked");
                self.round_trip = Some((at - ours).saturating_sub(self.ms(TURN)));
                // 11.2.1.2.5: tone A's reversal 40 ms on, then L1 and L2.
                self.reverse_at = Some((at + self.ms(TURN)).max(self.now + 1));
                self.probe_at = self.reverse_at.map(|r| r + self.ms(AFTER_REVERSAL));
                self.enter(Stage::AnswerSendProbe);
            }
            // 11.2.1.2.7: the call modem's reversal before its L1 and L2.
            Stage::AnswerProbeReversal => {
                let l2 = at + self.ms(AFTER_REVERSAL + probe::L1_SECONDS);
                self.read_from = l2 + self.ms(L2_SETTLE);
                self.read_until = l2 + self.ms(L2_READ);
                self.analyzer.reset();
                self.enter(Stage::AnswerReadProbe);
            }
            _ => {}
        }
    }

    fn rtd(&self) -> u64 {
        self.round_trip.unwrap_or(0)
    }

    fn stage_step(&mut self) {
        let now = self.now;
        match self.stage {
            Stage::CallInfo0 => {
                // 11.2.1.1.2: INFO0a received, and INFO0c gone, so on to the
                // reversals.
                if self.far.is_some() && self.tx.pending() == 0 {
                    self.enter(Stage::CallFirstReversal);
                }
            }
            Stage::CallFirstReversal => {
                // 11.2.2.1.2: tone B until the reversal comes, however long.
            }
            Stage::CallRanging => {
                if self.deadline.is_some_and(|d| now > d) {
                    // 11.2.2.1.3: silence, and tone B again once tone A is
                    // heard, back to waiting for its reversal.
                    self.recoveries += 1;
                    self.first_reversal_at = None;
                    self.reversed_at = None;
                    self.enter(Stage::CallFirstReversal);
                }
            }
            Stage::CallReadProbe => {
                if now >= self.read_until {
                    self.reading = self.analyzer.reading();
                    // 11.2.1.1.5: "The call modem shall then transmit Tone B".
                    self.start_tone();
                    self.enter(Stage::CallAwaitTone);
                    self.deadline = Some(now + self.ms(0.900) + self.rtd());
                    self.ignore_reversals_until = now + self.ms(0.050);
                }
            }
            Stage::CallAwaitTone => {
                if self.deadline.is_some_and(|d| now > d) && self.reverse_at.is_none() {
                    // 11.2.2.1.4: no reversal, so wait 40 ms and go on as
                    // though there had been one.
                    self.recoveries += 1;
                    self.reverse_at = Some(now + self.ms(TURN));
                    self.probe_at = self.reverse_at.map(|r| r + self.ms(AFTER_REVERSAL));
                    self.enter(Stage::CallSendProbe);
                }
            }
            Stage::CallSendProbe => {
                let Speaking::Probe { l1_until } = self.speaking else { return };
                if self.deadline.is_none() {
                    // Measured from the beginning of L2.
                    self.deadline = Some(l1_until + self.ms(0.650) + self.rtd());
                }
                // 11.2.1.1.7: tone A, heard over the echo of L2, and INFO1c.
                if now > l1_until && self.presence.held >= self.ms(TONE_HELD) {
                    let reading = self.reading.clone();
                    let far = self.far.unwrap_or_default();
                    let wide = far.constellation_1664 && self.ours.constellation_1664;
                    let mut probed = [Probed::default(); 6];
                    if let Some(reading) = reading.as_ref() {
                        for (slot, rate) in probed.iter_mut().zip(SymbolRate::ALL) {
                            *slot = reading.probed(rate, &far, wide);
                        }
                    }
                    let info1c = Info1c {
                        min_power_reduction: 0,
                        additional_power_reduction: 0,
                        md_length: 0,
                        probed,
                        frequency_offset: reading.and_then(|r| r.frequency_offset),
                    };
                    let bits = info1c.to_bits();
                    self.info1c = Info1c::from_bits(&bits);
                    self.speaking = Speaking::Carrier;
                    self.tx.stop();
                    self.tx.send(&bits);
                    // 11.2.1.1.8: "After sending INFO1c, the call modem shall
                    // transmit silence".
                    self.tx.silence();
                    self.enter(Stage::CallInfo1);
                } else if self.deadline.is_some_and(|d| now > d) {
                    self.fail("no tone A after this end's probing");
                }
            }
            Stage::CallInfo1 => {
                if self.deadline.is_none() && self.tx.pending() == 0 {
                    // 11.2.2.1.6: INFO1a within 700 ms and a round trip.
                    self.deadline = Some(now + self.ms(0.700) + self.rtd() + self.ms(0.3));
                }
                if self.deadline.is_some_and(|d| now > d) {
                    self.fail("no INFO1a");
                }
            }

            Stage::AnswerInfo0 => {
                if self.far.is_some() && self.tx.pending() == 0 {
                    self.enter(Stage::AnswerAwaitTone);
                }
            }
            Stage::AnswerAwaitTone => {
                // 11.2.1.2.3: tone B heard, and tone A on for 50 ms.
                if self.presence.held >= self.ms(TONE_HELD) && now - self.since >= self.ms(TONE_A_FIRST) {
                    self.reverse_at = Some(now + 1);
                    self.enter(Stage::AnswerRanging);
                    self.deadline = Some(now + self.ms(REVERSAL_WAIT));
                    // The reversal is this end's, and the call modem's answer
                    // cannot be back before a round trip and 40 ms.
                    self.ignore_reversals_until = now + self.ms(TURN);
                }
            }
            Stage::AnswerRanging => {
                // The reversal just scheduled should not also stop the tone.
                if self.reversed_at.is_some() && self.silence_at.is_some() && self.probe_at.is_none() {
                    self.silence_at = None;
                }
                if self.deadline.is_some_and(|d| now > d) {
                    // 11.2.2.2.2: listen for tone B again and reverse again.
                    self.recoveries += 1;
                    self.reversed_at = None;
                    self.enter(Stage::AnswerAwaitTone);
                }
            }
            Stage::AnswerSendProbe => {
                let Speaking::Probe { l1_until } = self.speaking else { return };
                if self.deadline.is_none() {
                    self.deadline = Some(l1_until + self.ms(0.600) + self.rtd());
                }
                // 11.2.1.2.6: tone B over the echo of L2, then tone A for 50 ms,
                // its reversal, 10 ms more and silence.
                if now > l1_until && self.presence.held >= self.ms(TONE_HELD) {
                    self.tx.stop();
                    self.start_tone();
                    self.reverse_at = Some(now + self.ms(TONE_A_FIRST));
                    self.enter(Stage::AnswerProbeReversal);
                    self.ignore_reversals_until = now + self.ms(TONE_A_FIRST + AFTER_REVERSAL);
                } else if self.deadline.is_some_and(|d| now > d) {
                    // 11.2.2.2.3: tone A, and back to 11.2.1.2.3.
                    self.recoveries += 1;
                    self.tx.stop();
                    self.start_tone();
                    self.reversed_at = None;
                    self.enter(Stage::AnswerAwaitTone);
                }
            }
            Stage::AnswerProbeReversal => {}
            Stage::AnswerReadProbe => {
                if now >= self.read_until {
                    self.reading = self.analyzer.reading();
                    // 11.2.1.2.8: tone A, and listen for INFO1c.
                    self.start_tone();
                    self.enter(Stage::AnswerInfo1);
                    self.deadline = Some(now + self.ms(2.0) + 2 * self.rtd());
                }
            }
            Stage::AnswerInfo1 => {
                if let Some(info1a) = self.info1a {
                    let _ = info1a;
                    if !self.tx.is_sending() {
                        self.status = Status::Done;
                        self.enter(Stage::Finished);
                    }
                } else if self.deadline.is_some_and(|d| now > d) {
                    // 11.2.2.2.4.
                    self.fail("no INFO1c");
                }
            }
            Stage::Finished => {}
        }
    }

    /// INFO1a: the symbol rates both ways, and what the call modem's
    /// transmitter is to use (11.2.1.2.9, Table 16).
    fn settle(&self, info1c: &Info1c) -> Info1a {
        let far = self.far.unwrap_or_default();
        let wide = far.constellation_1664 && self.ours.constellation_1664;
        // Towards the call modem: the fastest the call modem projected, the
        // lower symbol rate on a tie, since that is the one with the margin.
        let best = |rates: &[(SymbolRate, u8)]| {
            rates
                .iter()
                .copied()
                .filter(|(_, r)| *r > 0)
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.index().cmp(&a.0.index())))
        };
        let towards_call: Vec<(SymbolRate, u8)> =
            SymbolRate::ALL.iter().map(|&r| (r, info1c.probed[r.index() as usize].max_rate)).collect();
        let answer_to_call = best(&towards_call).map_or(SymbolRate::S2400, |b| b.0);
        // Towards this end: what this end read of the call modem's L2, within
        // the asymmetry both ends allow.
        let steps = u32::from(far.asymmetry.min(self.ours.asymmetry));
        let towards_answer: Vec<(SymbolRate, u8, Probed)> = SymbolRate::ALL
            .iter()
            .filter(|r| r.index().abs_diff(answer_to_call.index()) <= steps)
            .map(|&r| {
                let probed = self.reading.as_ref().map_or_else(Probed::default, |x| x.probed(r, &far, wide));
                (r, probed.max_rate, probed)
            })
            .collect();
        let (call_to_answer, probed) = best(&towards_answer.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>())
            .and_then(|(r, _)| towards_answer.iter().find(|x| x.0 == r).map(|x| (x.0, x.2)))
            .unwrap_or((answer_to_call, Probed::default()));
        Info1a {
            min_power_reduction: 0,
            additional_power_reduction: 0,
            md_length: 0,
            probed,
            answer_to_call,
            call_to_answer,
            frequency_offset: self.reading.as_ref().and_then(|r| r.frequency_offset),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    /// A line between two modems: a delay each way, a loss, noise, and a
    /// little of each end's own signal coming back to it.
    struct Line {
        to_answer: std::collections::VecDeque<f64>,
        to_call: std::collections::VecDeque<f64>,
        loss: f64,
        echo: f64,
        seed: u32,
        noise: f64,
    }

    impl Line {
        fn new(one_way: f64, loss_db: f64, echo_db: f64, noise_db: f64) -> Self {
            let delay = (one_way * FS) as usize;
            Self {
                to_answer: std::iter::repeat_n(0.0, delay.max(1)).collect(),
                to_call: std::iter::repeat_n(0.0, delay.max(1)).collect(),
                loss: 10f64.powf(-loss_db / 20.0),
                echo: if echo_db.is_finite() { 10f64.powf(-echo_db / 20.0) } else { 0.0 },
                seed: 0x9e37_79b9,
                noise: 10f64.powf(-noise_db / 20.0),
            }
        }

        fn noise(&mut self) -> f64 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            (f64::from(self.seed) / f64::from(u32::MAX) - 0.5) * 2.0 * self.noise * 1.2247
        }
    }

    /// Run the two ends of phase 2 against each other.
    fn call(line: &mut Line, seconds: f64) -> (Modem, Modem) {
        let mut caller = Modem::new(Role::Call, FS);
        let mut answerer = Modem::new(Role::Answer, FS);
        let (mut from_call, mut from_answer) = (0.0, 0.0);
        for _ in 0..(seconds * FS) as usize {
            let at_call = line.to_call.pop_front().unwrap() + line.echo * from_call + line.noise();
            let at_answer = line.to_answer.pop_front().unwrap() + line.echo * from_answer + line.noise();
            from_call = caller.step(at_call);
            from_answer = answerer.step(at_answer);
            line.to_answer.push_back(from_call * line.loss);
            line.to_call.push_back(from_answer * line.loss);
            if caller.status() != Status::Running && answerer.status() != Status::Running {
                break;
            }
        }
        (caller, answerer)
    }

    #[test]
    fn two_ends_get_through_phase_2_on_a_short_line() {
        let mut line = Line::new(0.010, 10.0, f64::INFINITY, 70.0);
        let (caller, answerer) = call(&mut line, 12.0);
        assert_eq!(caller.status(), Status::Done, "call modem stuck at {}", caller.phase());
        assert_eq!(answerer.status(), Status::Done, "answer modem stuck at {}", answerer.phase());
        assert_eq!(caller.info1a(), answerer.info1a());
        assert_eq!(caller.info1c(), answerer.info1c());
        for (who, m) in [("call", &caller), ("answer", &answerer)] {
            let rtd = m.round_trip().expect("no round trip");
            assert!((rtd - 0.020).abs() < 0.002, "{who} measured {rtd}");
            assert_eq!(m.recoveries(), 0, "{who} timed out rather than hearing a reversal");
        }
    }

    #[test]
    fn the_round_trip_of_a_voip_line_is_measured_and_survived() {
        // 750 ms each way, which is what the rig this is built on measures,
        // with an echo of each end's own signal 15 dB down and the line 20 dB.
        let mut line = Line::new(0.750, 20.0, 15.0, 60.0);
        let (caller, answerer) = call(&mut line, 20.0);
        assert_eq!(caller.status(), Status::Done, "call modem stuck at {}", caller.phase());
        assert_eq!(answerer.status(), Status::Done, "answer modem stuck at {}", answerer.phase());
        for (who, m) in [("call", &caller), ("answer", &answerer)] {
            let rtd = m.round_trip().expect("no round trip");
            assert!((rtd - 1.5).abs() < 0.002, "{who} measured {rtd}");
            assert_eq!(m.recoveries(), 0, "{who} timed out rather than hearing a reversal");
        }
    }

    #[test]
    fn a_clean_line_is_projected_at_33600_both_ways() {
        let mut line = Line::new(0.020, 6.0, f64::INFINITY, 55.0);
        let (caller, answerer) = call(&mut line, 12.0);
        let info1a = caller.info1a().expect("no INFO1a");
        assert_eq!(answerer.status(), Status::Done);
        assert_eq!(info1a.answer_to_call, SymbolRate::S3429, "{info1a:?}");
        assert_eq!(info1a.call_to_answer, SymbolRate::S3429, "{info1a:?}");
        assert_eq!(info1a.probed.max_rate, 14, "{info1a:?}");
        let info1c = caller.info1c().unwrap();
        assert_eq!(info1c.probed[5].max_rate, 14, "{info1c:?}");
    }

    #[test]
    fn a_noisy_line_is_projected_slower() {
        let mut line = Line::new(0.020, 10.0, f64::INFINITY, 32.0);
        let (caller, answerer) = call(&mut line, 12.0);
        assert_eq!(caller.status(), Status::Done, "call modem stuck at {}", caller.phase());
        assert_eq!(answerer.status(), Status::Done, "answer modem stuck at {}", answerer.phase());
        let info1a = caller.info1a().unwrap();
        assert!(info1a.probed.max_rate < 12, "{info1a:?}");
        assert!(info1a.probed.max_rate > 2, "{info1a:?}");
    }

    #[test]
    fn a_far_end_that_never_answers_is_given_up_on() {
        let mut caller = Modem::new(Role::Call, FS);
        for _ in 0..(21.0 * FS) as usize {
            caller.step(0.0);
        }
        assert!(matches!(caller.status(), Status::Failed(_)), "{:?}", caller.status());
    }
}
