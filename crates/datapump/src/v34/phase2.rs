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
//!
//! V.92's full phase 2 is this phase 2. "The operating procedures for full
//! Phase 2 and the associated recovery procedures are identical to those for
//! Phase 2 of ITU-T V.90" (9.3), so nothing here moves in time; what V.92
//! changes is what the sequences say.
//!
//! 1. Each INFO0 carries two bits that V.90 reserved: "V.92 capability" and
//!    "requests short Phase 2 to be used". INFO0d and INFO0a put them the
//!    other way round from each other -- 27 and 26 against 26 and 27 -- which
//!    is the one trap in the clause (Tables 15 and 16/V.92).
//! 2. When both modems have shown V.92, the digital modem's INFO1d is Table
//!    17, where bit 70 stops being the 3429 high-carrier flag and becomes
//!    "the channel supports PCM upstream".
//! 3. The analogue modem may then answer with Table 18, which asks for eight
//!    thousand symbols a second in *both* directions: PCM upstream. It "shall
//!    not use this sequence if bit 70 of INFO1d is clear" (8.4.1).
//!
//! Everything else -- the INFO0 recovery, the reversals, the probe, the
//! timeouts -- is untouched, and a pair of modems that offer no V.92 puts
//! exactly the bits on the line that it put there before.

use dsp::{ReversalDetector, ToneDetector};

use super::dpsk::{self, Side};
use super::info::{Info, Info0, Info0d, Info1a, Info1aPcm, Info1aPcmUp, Info1c, PcmFlags, Probed, SymbolRate};
use crate::v90;
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

/// What this end has of V.92 to offer, and what it wants out of it.
///
/// The two INFO0 flags are capabilities, sent before either end knows anything
/// about the other, so a V.92 modem always sets its own and reads the far
/// end's afterwards. The other two are choices this end makes about the call:
/// whether PCM upstream is wanted at all, and -- if it is -- what shape of
/// precoder and prefilter the digital modem may design against (Table 18 bits
/// 12:17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct V92Wish {
    /// "V.92 capability: 1": INFO0d bit 27, INFO0a bit 26 (Tables 15 and 16).
    pub capable: bool,
    /// "Set to 1 requests short Phase 2 to be used": INFO0d bit 26, INFO0a
    /// bit 27.
    ///
    /// An analogue modem "shall only indicate the desire to use a short Phase
    /// 2 if it intends to connect in either PCM upstream or V.90 data mode"
    /// (9.4), so this is not a capability but a promise about INFO1a. Nothing
    /// in this module acts on the agreement yet -- short phase 2's own stages
    /// are a later package -- and [`Modem::short_phase2_agreed`] is where the
    /// four bits of 9.4 are read.
    pub short_phase2: bool,
    /// Whether PCM upstream is wanted: `+PIG=0` at the analogue modem, and the
    /// digital modem's own verdict on the channel, which is what its INFO1d
    /// bit 70 carries.
    pub pcm_upstream: bool,
    /// What the analogue modem can run, for Table 18 bits 12:17. The digital
    /// modem has no use for it.
    pub up_caps: UpCaps,
}

impl V92Wish {
    /// The two INFO0 bits, whichever table is about to lay them out.
    fn flags(self) -> PcmFlags {
        PcmFlags { v92: self.capable, short_phase2: self.short_phase2 }
    }
}

/// Table 18/V.92 bits 12:17: the precoder and prefilter the analogue modem is
/// able to run, which is what bounds the digital modem's design of them.
///
/// The default is the least any V.92 analogue modem may promise -- p1 and z2
/// only, 192 coefficients in all and 128 in one section -- because 0 is the
/// first row of each of the three fields and every one of them is a floor
/// rather than a reserved value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpCaps {
    /// Bits 12:13: "0 = p1(i) and z2(i) are supported; 1 = z1(i), p1(i),
    /// z2(i); 2 = p1(i), p2(i), z2(i); 3 = z1(i), p1(i), p2(i), z2(i)".
    pub sections: u8,
    /// Bits 14:15: L_tot "in multiples of 64 starting at 192".
    pub ltot_code: u8,
    /// Bits 16:17: L_max "in multiples of 64 starting at 128".
    pub lmax_code: u8,
}

/// V.90's part in phase 2 (9.2/V.90), where there is one.
///
/// V.90 runs V.34's phase 2 with its own INFO sequences, and gives the two
/// sides by what the modems are rather than by who dialled: "INFO sequences
/// are transmitted by the analogue modem with a carrier frequency of 2400 Hz
/// ... by the digital modem with a carrier frequency of 1200 Hz" (8.2.3.1).
/// So the analogue modem is V.34's answer modem here even though it placed
/// the call, and the digital modem V.34's call modem.
///
/// Each role comes twice: once as V.90 built it, and once carrying a
/// [`V92Wish`]. The V.90 pair are not a special case of the V.92 pair with an
/// empty wish -- they are the same thing, and the INFO0 they send is identical
/// bit for bit -- but they are kept as their own constructors so that a V.90
/// call cannot acquire a V.92 flag by accident, and so that the two places
/// that build them need no change at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pcm {
    /// The analogue modem: it asks for V.90 in INFO1a when the far end's
    /// INFO0 was an INFO0d.
    Analogue,
    /// The digital modem, whose INFO0 is this INFO0d. Its bits 12 to 28 are
    /// filled in from this end's own capabilities.
    Digital(Info0d),
    /// The analogue modem with V.92 to offer.
    AnalogueV92(V92Wish),
    /// The digital modem with V.92 to offer.
    DigitalV92(Info0d, V92Wish),
}

impl Pcm {
    /// The V.34 side this takes in phase 2.
    pub fn role(self) -> Role {
        match self {
            Self::Analogue | Self::AnalogueV92(_) => Role::Answer,
            Self::Digital(_) | Self::DigitalV92(..) => Role::Call,
        }
    }

    /// What this end offers of V.92. The V.90 constructors offer nothing, so
    /// every flag reads false and every test below them takes the V.90 path.
    pub fn wish(self) -> V92Wish {
        match self {
            Self::AnalogueV92(wish) | Self::DigitalV92(_, wish) => wish,
            Self::Analogue | Self::Digital(_) => V92Wish::default(),
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

/// How long L2 is read for: "a period of time not to exceed 500 ms", and
/// well short of that.
///
/// The limit is a comparison with the far end's clock, not a target. The end
/// sending L2 gives the other's tone "600 ms plus a round trip delay from the
/// beginning of L2" (11.2.2.2.3; 650 for the call modem's wait, 11.2.2.1.5),
/// and the round trips cancel: a tone started after 500 ms of L2 reaches the
/// far end with 100 ms left for its detector to hear it over the echo of its
/// own L2. A real modem called on 2026-09-17, with a line that echoed us back
/// only 25 dB down, used all of it: in each of three attempts its tone A came
/// 92 to 103 ms after ours arrived, a few milliseconds either side of that
/// deadline. Each time it then ignored our reversal and probe, and retrained
/// 2 s and two round trips later. The modems that get through answer 20 to
/// 50 ms after our tone arrives, and dialup.world reads only about 250 ms of
/// our L2. Three hundred is fourteen of the analyzer's windows, which its
/// median still reads a jitter-buffer slip or two past.
const L2_READ: f64 = 0.300;

/// Silence before the tone that starts or answers a retrain: "70 ± 5 ms"
/// (11.5.1.1, 11.5.1.2, 11.5.2.1, 11.5.2.2).
const RETRAIN_SILENCE: f64 = 0.070;

/// Allowed on top of the recommendation's waits for the far end's tone after
/// this end's L2 (11.2.2.1.5, 11.2.2.2.3). The far end may read the whole
/// 500 ms, which leaves 150 and 100 ms of those waits, and a jitter-buffer
/// slip can take twenty of them. Waiting longer costs nothing: the far end's
/// own wait for what follows has seconds in it.
const TONE_AFTER_PROBE_SLACK: f64 = 0.300;

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
    /// This end's tone starts at this sample, after a retrain's silence.
    tone_at: Option<u64>,

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
    /// Set when the failure is one to retrain from.
    retrain_instead: bool,
    /// V.90's part, if this is V.90's phase 2.
    pcm: Option<Pcm>,
    /// Whether an analogue modem that meets a digital one asks for V.34
    /// anyway, having found the line will not carry PCM.
    pcm_declined: bool,
    /// The far end's INFO0d, if it sent one.
    far_info0d: Option<Info0d>,
    /// An INFO1a asking for V.90, sent or received.
    info1a_pcm: Option<Info1aPcm>,
    /// An INFO1a asking for PCM upstream (Table 18/V.92), sent or received.
    info1a_pcm_up: Option<Info1aPcmUp>,
    /// Whether an analogue modem that could ask for PCM upstream asks for
    /// V.90 data mode instead, having found the line will not carry it.
    pcm_up_declined: bool,
    /// The far end's two V.92 bits, read as its own INFO0 table lays them out.
    ///
    /// Only ever written in a PCM role. V.34's Table 14 has the transmit clock
    /// source in those two bits, and a V.34 modem that names one means a
    /// clock by it (P2P P1, P2S N-3).
    far_flags: PcmFlags,
    /// Set on a retrain, where 9.7 always re-enters full phase 2 whatever the
    /// INFO0 exchange once agreed.
    full_phase2_only: bool,
    /// Bit 70 as this end's INFO1d sent it, meaning "the channel supports PCM
    /// upstream" rather than a carrier flag. False until an INFO1d has gone.
    sent_pcm_upstream: bool,
    /// Why the last INFO1a was counted as not received, if one was.
    refused_info1a: Option<&'static str>,
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
            tone_at: None,
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
            retrain_instead: false,
            pcm: None,
            pcm_declined: false,
            far_info0d: None,
            info1a_pcm: None,
            info1a_pcm_up: None,
            pcm_up_declined: false,
            far_flags: PcmFlags::default(),
            full_phase2_only: false,
            sent_pcm_upstream: false,
            refused_info1a: None,
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
        let bits = modem.info0_bits();
        modem.tx.send(&bits);
        modem
    }

    /// V.90's phase 2 from its start (9.2.1.1.1, 9.2.2.1.1).
    pub fn v90(pcm: Pcm, fs: f64) -> Self {
        let mut modem = Self::blank(pcm.role(), fs);
        modem.pcm = Some(pcm);
        modem.stage = match pcm.role() {
            Role::Call => Stage::CallInfo0,
            Role::Answer => Stage::AnswerInfo0,
        };
        modem.speaking = Speaking::Carrier;
        let bits = modem.info0_bits();
        modem.tx.send(&bits);
        modem
    }

    /// V.90's phase 2 as a retrain: "Any subsequent retrains shall use Phase
    /// 2 of V.90 regardless of the analogue modem's choice of operating mode"
    /// (9.2.1.1.8, 9.2.2.1.9).
    pub fn v90_retrain(pcm: Pcm, fs: f64, far: Info0, far_info0d: Option<Info0d>) -> Self {
        let mut modem = Self::retrain(pcm.role(), fs, far);
        modem.pcm = Some(pcm);
        modem.far_info0d = far_info0d;
        modem
    }

    /// A retrain of this phase 2 (11.5): the same end, the same far end, and
    /// V.90's part kept if there is one.
    ///
    /// The V.92 flags are kept too. A retrain never repeats INFO0, so the one
    /// exchange there was is the only place either end's flags could have come
    /// from -- and 9.3 wants them still in force: "If both the digital and
    /// analogue modems indicate V.92 capability, any subsequent retrains shall
    /// use Phase 2 of ITU-T V.92." What a retrain does *not* keep is the short
    /// phase 2 agreement: every re-entry 9.7 defines lands in the full
    /// procedure, at the reversal the two ends were already expecting.
    pub fn again(&self) -> Self {
        let far = self.far.unwrap_or_default();
        let mut modem = match self.pcm {
            Some(pcm) => Self::v90_retrain(pcm, self.fs, far, self.far_info0d),
            None => Self::retrain(self.role, self.fs, far),
        };
        modem.pcm_declined = self.pcm_declined;
        modem.pcm_up_declined = self.pcm_up_declined;
        modem.far_flags = self.far_flags;
        modem.full_phase2_only = true;
        modem
    }

    /// Ask for V.34 in INFO1a from now on, even of a V.90 digital modem
    /// (9.2.2.1.9 leaves the choice to the analogue modem).
    pub fn decline_pcm(&mut self) {
        self.pcm_declined = true;
    }

    /// Ask for V.90 data mode rather than PCM upstream in INFO1a from now on,
    /// even of a V.92 digital modem that has offered it.
    ///
    /// The rung above [`Self::decline_pcm`] on the fallback ladder: 9.3 makes
    /// Table 18 a "may" for the analogue modem, so a line that will not carry
    /// PCM upstream is left by asking for Table 10 instead, without giving up
    /// PCM downstream as well.
    pub fn decline_pcm_upstream(&mut self) {
        self.pcm_up_declined = true;
    }

    /// This end's INFO0 as it goes out: INFO0d from a digital modem, and the
    /// two V.92 flags at whichever bits this layout puts them.
    ///
    /// The V.90 roles take the first two arms, which are the code that was
    /// here before V.92 existed, so a V.90 pair sends the sequence it always
    /// sent -- including whatever bits 26:27 a caller's own INFO0d carried.
    fn info0_bits(&self) -> Vec<bool> {
        match self.pcm {
            Some(Pcm::Digital(d)) => Info0d { v34: self.ours, ..d }.to_bits(),
            Some(Pcm::Analogue) | None => self.ours.to_bits(),
            // Table 15/V.92: bit 26 is the short phase 2 request and bit 27
            // the V.92 capability. `Info0d::to_bits` drops V.34's clock from
            // the seventeen bits it shares with INFO0a and writes these two
            // over the zeros V.90 reserved there.
            Some(Pcm::DigitalV92(d, wish)) => {
                let mut info0d = Info0d { v34: self.ours, ..d };
                info0d.set_pcm_flags(wish.flags());
                info0d.to_bits()
            }
            // Table 16/V.92, which has them the other way round: bit 26 the
            // capability, bit 27 the request.
            Some(Pcm::AnalogueV92(wish)) => {
                let mut ours = self.ours;
                ours.set_pcm_flags(wish.flags());
                ours.to_bits()
            }
        }
    }

    /// What this end offers of V.92, which is nothing outside a PCM role.
    fn wish(&self) -> V92Wish {
        self.pcm.map(Pcm::wish).unwrap_or_default()
    }

    /// Whether both modems have shown V.92 capability: "bit 27 of INFO0d and
    /// bit 26 of INFO0a" (9.3).
    ///
    /// Everything V.92 adds to phase 2 hangs off this -- Table 17's meaning
    /// for INFO1d bit 70, Table 18's availability, the retrain rule and the
    /// ODP/ADP bypass -- and it is false for every V.90 and V.34 call, because
    /// no wish has been offered and the far bits were never read as flags.
    pub fn both_v92(&self) -> bool {
        self.wish().capable && self.far_flags.v92
    }

    /// The far end's two V.92 bits as its own INFO0 laid them out. All false
    /// outside a PCM role, where those bits are a transmit clock.
    pub fn far_pcm_flags(&self) -> PcmFlags {
        self.far_flags
    }

    /// Whether all four bits of 9.4 agree, so that short phase 2 is the one to
    /// run: both modems V.92 (INFO0d bit 27, INFO0a bit 26) and both asking
    /// for it (INFO0d bit 26, INFO0a bit 27).
    ///
    /// False after a retrain, whatever the flags say, because every retrain
    /// re-enters full phase 2 (9.7).
    pub fn short_phase2_agreed(&self) -> bool {
        !self.full_phase2_only && self.both_v92() && self.wish().short_phase2 && self.far_flags.short_phase2
    }

    /// Whether V.42's ODP/ADP exchange is to be skipped (9.3.1): "If both
    /// modems indicate V.92 capability as well as indicating LAPM protocol in
    /// ITU-T V.8 or ITU-T V.8 *bis*".
    ///
    /// Phase 2 knows one half of that and V.8 knows the other, so the caller
    /// passes in what the menus said. V.8 7.3 warns that some equipment shows
    /// LAPM in prot0 and still wants the exchange, so whatever bypasses should
    /// still answer an ODP that arrives anyway.
    pub fn lapm_bypass_allowed(&self, both_lapm: bool) -> bool {
        self.both_v92() && both_lapm
    }

    /// V.90's part, if this is V.90's phase 2.
    pub fn pcm(&self) -> Option<Pcm> {
        self.pcm
    }

    /// The far end's INFO0d, if it sent one: a V.90 digital modem.
    pub fn far_info0d(&self) -> Option<Info0d> {
        self.far_info0d
    }

    /// The INFO1a asking for V.90, if one went or came.
    pub fn info1a_pcm(&self) -> Option<Info1aPcm> {
        self.info1a_pcm
    }

    /// The INFO1a asking for PCM upstream (Table 18/V.92), if one went or
    /// came. At most one of this and [`Self::info1a_pcm`] is ever set.
    pub fn info1a_pcm_up(&self) -> Option<Info1aPcmUp> {
        self.info1a_pcm_up
    }

    /// Why the last INFO1a was counted as not received, if one was.
    ///
    /// A sequence whose CRC checked but whose layout this phase 2 may not act
    /// on is treated as though it had not arrived (P2P P15), which leaves the
    /// recovery of 9.2.1.2.6 to run: the reason is here for a log, not for a
    /// decision.
    pub fn refused_info1a(&self) -> Option<&'static str> {
        self.refused_info1a
    }

    /// The fastest V.34 the far end's line probe says this end could
    /// receive, in bit/s: what a V.90 analogue modem gives up by choosing
    /// V.90. None before the probe has been read.
    pub fn v34_receive_rate(&self) -> Option<u32> {
        let (reading, far) = (self.reading.as_ref()?, self.far?);
        let wide = far.constellation_1664 && self.ours.constellation_1664;
        SymbolRate::ALL.iter().map(|&r| u32::from(reading.probed(r, &far, wide).max_rate) * 2400).max()
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
        // Silence first, then the tone. Going straight from data or L2 into
        // the tone left a far end one retrain handled late and another it
        // began itself.
        modem.speaking = Speaking::Silent;
        modem.tone_at = Some(modem.ms(RETRAIN_SILENCE));
        // The step from data or a renegotiation tone into this one is not a
        // reversal, however it reads.
        modem.ignore_reversals_until = modem.ms(RETRAIN_SILENCE + 0.050);
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

    /// Stop here, the way [`Self::fail`] does, for a lapse that 11.2.2 has
    /// answered with a retrain rather than the end of the call.
    fn fail_for_a_retrain(&mut self, why: &'static str) {
        self.fail(why);
        self.retrain_instead = true;
    }

    /// Whether the failure is one the recommendation recovers from by
    /// retraining (11.5.1.1, 11.5.2.1). What that retrain is belongs to
    /// whatever owns this: phase 2 knows only that it has lost its place.
    pub fn asks_for_retrain(&self) -> bool {
        self.retrain_instead
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
        if self.tone_at == Some(self.now) {
            self.tone_at = None;
            self.start_tone();
            // The stage's own clock -- tone A's 50 ms before its reversal --
            // starts with the tone.
            self.since = self.now;
        }
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
            // A V.90 digital modem's INFO0 carries INFO0a's bits, and is heard
            // as one -- by a V.34 modem too, which has no use for the rest.
            (_, Info::Info0d(far)) => {
                self.far_info0d = Some(far);
                // Table 15/V.92's order, and only in a PCM role: V.90 reserves
                // these two bits and sets both to 0, so a V.90 digital modem
                // reads as "not V.92" without knowing V.92 exists.
                if self.pcm.is_some() {
                    self.far_flags = far.pcm_flags();
                }
                self.heard(Info::Info0(far.v34));
            }
            (_, Info::Info0(far)) => {
                self.far = Some(far);
                // Table 16/V.92's order, for the INFO0a a digital modem hears.
                // Not when this arm was reached from the one above, which has
                // already read the same two bits the other way round; and not
                // in a V.34 phase 2, where they are a transmit clock source.
                if self.pcm.is_some() && self.far_info0d.is_none() {
                    self.far_flags = far.pcm_flags();
                }
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
                    let bits = self.info0_bits();
                    self.tx.send(&bits);
                }
            }
            (Role::Answer, Info::Info1c(info1c)) if self.stage == Stage::AnswerInfo1 => {
                self.info1c = Some(info1c);
                // An analogue modem that heard an INFO0d asks for V.90
                // (9.2.2.1.9); anything else is V.34's INFO1a. A V.92 pair
                // whose INFO1d allowed PCM upstream asks for that instead
                // (9.3, 8.4.1).
                let analogue = self.pcm.is_some_and(|p| p.role() == Role::Answer);
                let bits = match (analogue, self.far_info0d) {
                    (true, Some(far)) if self.table_18_allowed(&info1c) => {
                        let asked = self.settle_pcm_up(&far);
                        let bits = asked.to_bits();
                        self.info1a_pcm_up = Some(asked);
                        bits
                    }
                    (true, Some(far)) if !self.pcm_declined => {
                        let bits = self.settle_pcm(&info1c, &far).to_bits();
                        self.info1a_pcm = Info1aPcm::from_bits(&bits);
                        bits
                    }
                    _ => {
                        let bits = self.settle(&info1c).to_bits();
                        // Kept as it went, which is to the field's own
                        // resolution.
                        self.info1a = Info1a::from_bits(&bits);
                        bits
                    }
                };
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
            // 9.2.1.1.8: "the digital modem shall proceed to Phase 3 of the
            // start-up procedure if bits 37:39 of INFO1a indicate the integer
            // 6". A V.34 modem never gets one, since it never sent INFO0d.
            (Role::Call, Info::Info1aPcm(asked)) if self.stage == Stage::CallInfo1 && self.pcm.is_some() => {
                self.info1a_pcm = Some(asked);
                self.status = Status::Done;
                self.enter(Stage::Finished);
            }
            // Table 18/V.92: eight thousand symbols a second in both
            // directions. 9.3 allows it only when both modems have shown V.92,
            // and 8.4.1 only when the INFO1d this end sent had bit 70 set --
            // so a frame that arrives without both is one this end may not act
            // on, and P2P P15 counts it as not received rather than as a
            // reason to fail. The 9.2.1.2.6 recovery then runs on its own.
            (Role::Call, Info::Info1aPcmUp(asked)) if self.stage == Stage::CallInfo1 && self.pcm.is_some() => {
                if self.both_v92() && self.sent_pcm_upstream {
                    self.info1a_pcm_up = Some(asked);
                    self.status = Status::Done;
                    self.enter(Stage::Finished);
                } else if self.both_v92() {
                    self.refused_info1a = Some("a table 18 info1a after an info1d that cleared bit 70");
                } else {
                    self.refused_info1a = Some("a table 18 info1a from a far end that did not indicate v.92");
                }
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
                // is read for L2_READ after it.
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
                //
                // Silent here means 11.2.2.1.3 sent us back, and that clause
                // says how to come back from it: "the call modem shall
                // transmit silence and condition its receiver to detect Tone
                // A. After detecting Tone A, the call modem shall transmit
                // Tone B." Without the second half of that sentence this end
                // says nothing for the rest of phase 2, while the answer modem
                // waits in 11.2.1.2.3 for the tone B that would start it.
                //
                // Not while a tone is already due: a retrain opens with a
                // deliberate silence before its own tone (11.5.1.2), and tone
                // A is on the line all through it.
                if matches!(self.speaking, Speaking::Silent)
                    && self.tone_at.is_none()
                    && self.presence.held >= self.ms(TONE_HELD)
                {
                    self.start_tone();
                }
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
                    self.deadline = Some(l1_until + self.ms(0.650 + TONE_AFTER_PROBE_SLACK) + self.rtd());
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
                    let mut info1c = Info1c {
                        min_power_reduction: 0,
                        additional_power_reduction: 0,
                        md_length: 0,
                        probed,
                        frequency_offset: reading.and_then(|r| r.frequency_offset),
                    };
                    // Table 17/V.92: when both modems have shown V.92, bit 70
                    // stops being the 3429 high-carrier flag and becomes "Set
                    // to 0 indicates that the channel does not support PCM
                    // upstream". It is a permission, not an instruction: the
                    // analogue modem may still ask for V.90 or V.34 (N-6).
                    if self.both_v92() {
                        info1c.set_pcm_upstream(self.wish().pcm_upstream);
                        self.sent_pcm_upstream = info1c.pcm_upstream();
                    }
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
                    // 11.2.2.1.5: "the call modem shall initiate a retrain".
                    self.fail_for_a_retrain("no tone A after this end's probing");
                }
            }
            Stage::CallInfo1 => {
                if self.deadline.is_none() && self.tx.pending() == 0 {
                    // 11.2.2.1.6: INFO1a within 700 ms and a round trip.
                    self.deadline = Some(now + self.ms(0.700) + self.rtd() + self.ms(0.3));
                }
                if self.deadline.is_some_and(|d| now > d) {
                    // 11.2.2.1.6: listen for tone A or INFOMARKSa, and on
                    // INFOMARKSa "either initiate a retrain according to
                    // 11.5.1.1 or send INFO1c". The far end has gone on to
                    // phase 3 by now; after its own wait for S it sends
                    // INFOMARKSa and listens for tone B (11.3.2.2.1). So the
                    // retrain is what both recovery paths lead to.
                    self.fail_for_a_retrain("no INFO1a");
                }
            }

            Stage::AnswerInfo0 => {
                if self.far.is_some() && self.tx.pending() == 0 {
                    self.enter(Stage::AnswerAwaitTone);
                }
            }
            Stage::AnswerAwaitTone => {
                // 11.2.1.2.3: tone B heard, and tone A on for 50 ms.
                if self.tone_at.is_none()
                    && self.presence.held >= self.ms(TONE_HELD)
                    && now - self.since >= self.ms(TONE_A_FIRST)
                {
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
                    self.deadline = Some(l1_until + self.ms(0.600 + TONE_AFTER_PROBE_SLACK) + self.rtd());
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
                if self.info1a.is_some() || self.info1a_pcm.is_some() || self.info1a_pcm_up.is_some() {
                    if !self.tx.is_sending() {
                        self.status = Status::Done;
                        self.enter(Stage::Finished);
                    }
                } else if self.deadline.is_some_and(|d| now > d) {
                    // 11.2.2.2.4: "initiate a retrain according to 11.5.2.1
                    // or send INFOMARKSa".
                    self.fail_for_a_retrain("no INFO1c");
                }
            }
            Stage::Finished => {}
        }
    }

    /// Whether this INFO1d and this call between them let the analogue modem
    /// ask for PCM upstream with a Table 18 INFO1a.
    ///
    /// Four things, and all of them (9.3, 8.4.1):
    ///
    /// - both modems have shown V.92, or the digital modem's INFO1d is Table 9
    ///   and its bit 70 means a carrier rather than a channel;
    /// - that bit 70 is set -- "The analogue modem shall not use this sequence
    ///   if bit 70 of INFO1d is clear";
    /// - PCM upstream is wanted here at all, which `+PIG=1` turns off;
    /// - and neither rung of the fallback ladder has been taken.
    fn table_18_allowed(&self, info1d: &Info1c) -> bool {
        self.both_v92()
            && info1d.pcm_upstream()
            && self.wish().pcm_upstream
            && !self.pcm_up_declined
            && !self.pcm_declined
    }

    /// V.92's INFO1a (Table 18/V.92): PCM upstream, the precoder and prefilter
    /// the digital modem may design, and the codeword it is to train with.
    ///
    /// The symbol rates are not chosen here, because there is nothing to
    /// choose: Table 18 names the integer 6 in both fields, which is eight
    /// thousand a second each way. MD is length 0, so the digital modem starts
    /// on TRN1u as soon as it hears the Ru reversal.
    fn settle_pcm_up(&self, far: &Info0d) -> Info1aPcmUp {
        let caps = self.wish().up_caps;
        Info1aPcmUp {
            sections: caps.sections,
            ltot_code: caps.ltot_code,
            lmax_code: caps.lmax_code,
            md_length: 0,
            uinfo: v90::training_codeword_v92(far),
        }
    }

    /// V.90's INFO1a (Table 10/V.90): the upstream symbol rate, and the
    /// codeword the digital modem is to train with.
    fn settle_pcm(&self, info1d: &Info1c, far: &Info0d) -> Info1aPcm {
        // "An integer between 3 and 5 ... consistent with information in
        // INFO1d": the fastest the digital modem projected, the lower symbol
        // rate on a tie; 3429 only where both ends allow it upstream.
        let allowed = |r: SymbolRate| match r {
            SymbolRate::S3000 | SymbolRate::S3200 => true,
            SymbolRate::S3429 => far.upstream_3429 && self.ours.transmit_3429,
            _ => false,
        };
        let upstream = SymbolRate::ALL
            .iter()
            .copied()
            .filter(|&r| allowed(r) && info1d.probed[r.index() as usize].max_rate > 0)
            .max_by(|a, b| {
                let rate = |r: &SymbolRate| info1d.probed[r.index() as usize].max_rate;
                rate(a).cmp(&rate(b)).then(b.index().cmp(&a.index()))
            })
            // 6.2: 3200 is the one every analogue modem has.
            .unwrap_or(SymbolRate::S3200);
        Info1aPcm {
            md_length: 0,
            uinfo: v90::training_codeword(far),
            upstream,
            frequency_offset: self.reading.as_ref().and_then(|r| r.frequency_offset),
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
        run(line, seconds, Modem::new(Role::Call, FS), Modem::new(Role::Answer, FS))
    }

    /// The same for any two ends: the first takes V.34's call side.
    fn run(line: &mut Line, seconds: f64, mut caller: Modem, mut answerer: Modem) -> (Modem, Modem) {
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

    fn server() -> Info0d {
        Info0d { nominal_power: 4, max_power: 23, power_at_codec: true, ..Info0d::default() }
    }

    /// 9.2: a V.90 pair gets through V.34's phase 2 with V.90's sequences in
    /// it, and the analogue modem asks for V.90 and a codeword to train on.
    #[test]
    fn a_v90_pair_settles_on_v90() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = run(&mut line, 12.0, Modem::v90(Pcm::Digital(server()), FS), Modem::v90(Pcm::Analogue, FS));
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        // Each end heard what the other is.
        assert_eq!(analogue.far_info0d().map(|d| d.max_power), Some(23));
        assert!(digital.far_info0d().is_none());
        let asked = analogue.info1a_pcm().expect("the analogue modem sent no V.90 INFO1a");
        assert_eq!(digital.info1a_pcm(), Some(asked));
        assert!(digital.info1a().is_none() && analogue.info1a().is_none());
        assert_eq!(asked.uinfo, 79);
        assert!(matches!(asked.upstream, SymbolRate::S3000 | SymbolRate::S3200));
        // And the digital modem probed the upstream, as INFO1d says.
        assert!(digital.info1c().is_some_and(|i| i.probed[4].max_rate > 0));
    }

    /// A V.34 modem calling a V.90 digital modem hears an INFO0d as an
    /// INFO0, and gets V.34.
    #[test]
    fn a_v34_modem_calling_a_v90_server_gets_v34() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, v34) = run(&mut line, 12.0, Modem::v90(Pcm::Digital(server()), FS), Modem::new(Role::Answer, FS));
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(v34.status(), Status::Done, "V.34: {}", v34.phase());
        assert!(v34.info1a().is_some());
        assert!(digital.info1a().is_some(), "the server did not take V.34's INFO1a");
        assert!(digital.info1a_pcm().is_none());
    }

    /// When each end starts its tone after reading the other's L2, against
    /// the far end's deadline for hearing it (11.2.2.2.3 and 11.2.2.1.5), in
    /// seconds to spare once the tone has crossed the line.
    fn time_to_spare(one_way: f64) -> (f64, f64) {
        let mut line = Line::new(one_way, 20.0, 15.0, 60.0);
        let (mut caller, mut answerer) = (Modem::new(Role::Call, FS), Modem::new(Role::Answer, FS));
        let (mut from_call, mut from_answer) = (0.0, 0.0);
        // Where each end's L2 began, and where the other's tone began after it.
        let (mut answer_l2, mut call_l2, mut tone_b, mut tone_a) = (None, None, None, None);
        for _ in 0..(20.0 * FS) as usize {
            let at_call = line.to_call.pop_front().unwrap() + line.echo * from_call + line.noise();
            let at_answer = line.to_answer.pop_front().unwrap() + line.echo * from_answer + line.noise();
            let (call_was, answer_was) = (caller.stage, answerer.stage);
            from_call = caller.step(at_call);
            from_answer = answerer.step(at_answer);
            line.to_answer.push_back(from_call * line.loss);
            line.to_call.push_back(from_answer * line.loss);
            if let (Stage::AnswerSendProbe, Speaking::Probe { l1_until }) = (answerer.stage, answerer.speaking) {
                answer_l2.get_or_insert(l1_until);
            }
            if let (Stage::CallSendProbe, Speaking::Probe { l1_until }) = (caller.stage, caller.speaking) {
                call_l2.get_or_insert(l1_until);
            }
            if call_was == Stage::CallReadProbe && caller.stage == Stage::CallAwaitTone {
                tone_b.get_or_insert(caller.now);
            }
            if answer_was == Stage::AnswerReadProbe && answerer.stage == Stage::AnswerInfo1 {
                tone_a.get_or_insert(answerer.now);
            }
            if caller.status() != Status::Running && answerer.status() != Status::Running {
                break;
            }
        }
        assert_eq!(caller.status(), Status::Done, "call modem stuck at {}", caller.phase());
        let crossing = (one_way * FS) as u64;
        let spare = |l2: Option<u64>, tone: Option<u64>, wait: f64, far: &Modem| {
            let (l2, tone) = (l2.expect("no L2"), tone.expect("no tone"));
            let deadline = l2 + far.ms(wait) + far.rtd();
            (deadline as f64 - (tone + crossing) as f64) / FS
        };
        (
            spare(answer_l2, tone_b, 0.600, &answerer),
            spare(call_l2, tone_a, 0.650, &caller),
        )
    }

    #[test]
    fn each_end_leaves_the_other_time_to_hear_its_tone() {
        // Reading the whole 500 ms the recommendation allows left a real
        // modem's detector 100 ms, and it missed its deadline by one. The
        // round trips cancel, so the margin is the same on every line.
        for one_way in [0.010, 0.650, 0.750] {
            let (for_answer, for_call) = time_to_spare(one_way);
            assert!(for_answer > 0.25, "{one_way} s each way: the answer modem has {for_answer:.3} s to hear tone B");
            assert!(for_call > 0.30, "{one_way} s each way: the call modem has {for_call:.3} s to hear tone A");
        }
    }

    /// 11.2.2.1.3: a call modem sent back to the beginning speaks again.
    ///
    /// "The call modem shall transmit silence and condition its receiver to
    /// detect Tone A. After detecting Tone A, the call modem shall transmit
    /// Tone B." Only the first half of that sentence was here, so a call modem
    /// whose ranging reversal went unanswered fell silent for the rest of
    /// phase 2 -- and the answer modem, waiting in 11.2.1.2.3 for the tone B
    /// that would set it going, waited with it. Both ends listening and
    /// neither speaking until phase 2 gave up twenty seconds later, which on a
    /// V.90 retrain took the call down with it.
    #[test]
    fn a_call_modem_whose_ranging_went_unanswered_sends_tone_b_again() {
        let mut m = Modem::new(Role::Call, FS);
        // Ranging, with this end's reversal made and nothing answering it. The
        // ten milliseconds after that reversal have already left the line
        // silent (11.2.1.1.3).
        m.stage = Stage::CallRanging;
        m.since = m.now;
        m.deadline = Some(m.ms(REVERSAL_WAIT));
        m.reversed_at = Some(m.now);
        m.tx.stop();
        m.speaking = Speaking::Silent;

        // Tone A all the while, which is what an answer modem waiting in
        // 11.2.1.2.3 is sending.
        let mut tone_a = dsp::Nco::new(Side::Answer.carrier(), FS);
        let out: Vec<f64> = (0..((REVERSAL_WAIT + 1.0) * FS) as usize)
            .map(|_| m.step(0.9 * tone_a.step().1))
            .collect();
        assert_eq!(m.stage, Stage::CallFirstReversal, "11.2.2.1.3 never ran");
        let after = &out[((REVERSAL_WAIT + 0.5) * FS) as usize..];
        let loudest = after.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(loudest > 0.1, "the call modem is still silent half a second later: {loudest}");
    }

    #[test]
    fn a_retrain_begins_with_70_ms_of_silence_and_then_the_tone() {
        // 11.5.1.1 and 11.5.2.2 alike, from either end.
        for role in [Role::Call, Role::Answer] {
            let mut m = Modem::retrain(role, FS, Info0::default());
            let out: Vec<f64> = (0..(0.2 * FS) as usize).map(|_| m.step(0.0)).collect();
            let first = out.iter().position(|s| s.abs() > 1e-9).expect("no tone at all");
            let ms = first as f64 / FS * 1000.0;
            assert!((65.0..=75.0).contains(&ms), "{role:?}: the tone began after {ms} ms");
            let loudest = out[first + 320..].iter().fold(0.0f64, |m, s| m.max(s.abs()));
            assert!(loudest > 0.1, "{role:?}: the tone is at {loudest}");
        }
    }

    /// And a V.90 analogue modem that meets a V.34 modem's INFO0 asks for V.34.
    #[test]
    fn a_v90_analogue_modem_meeting_v34_asks_for_v34() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (v34, analogue) = run(&mut line, 12.0, Modem::new(Role::Call, FS), Modem::v90(Pcm::Analogue, FS));
        assert_eq!(v34.status(), Status::Done);
        assert_eq!(analogue.status(), Status::Done);
        assert!(analogue.info1a().is_some() && analogue.info1a_pcm().is_none());
        assert!(v34.info1a().is_some());
    }

    /// All four sections, 384 coefficients in all and 320 in one: the top row
    /// of each of Table 18's three capability fields.
    fn caps() -> UpCaps {
        UpCaps { sections: 3, ltot_code: 3, lmax_code: 3 }
    }

    /// A V.92 analogue modem. It does not ask for a short phase 2: the four
    /// bits of 9.4 are read here, but short phase 2's own stages are not in
    /// this package, so a modem that asked for one and got it would be waiting
    /// for a reversal the far end had already made.
    fn analogue_v92(pcm_upstream: bool) -> Pcm {
        Pcm::AnalogueV92(V92Wish { capable: true, short_phase2: false, pcm_upstream, up_caps: caps() })
    }

    /// A V.92 digital modem, whose `pcm_upstream` is its verdict on the
    /// channel and so what its INFO1d bit 70 will carry.
    fn digital_v92(pcm_upstream: bool) -> Pcm {
        Pcm::DigitalV92(server(), V92Wish { capable: true, pcm_upstream, ..V92Wish::default() })
    }

    /// Two PCM modems against each other, the digital one on V.34's call side.
    fn pcm_call(line: &mut Line, digital: Pcm, analogue: Pcm) -> (Modem, Modem) {
        run(line, 12.0, Modem::v90(digital, FS), Modem::v90(analogue, FS))
    }

    /// 9.3: both modems indicate V.92, so INFO1d is Table 17 with bit 70 set
    /// and the analogue modem answers with Table 18 -- "the integer 6" in both
    /// symbol rate fields, which is eight thousand a second each way.
    #[test]
    fn a_v92_pair_settles_on_pcm_upstream() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, digital_v92(true), analogue_v92(true));
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        // Each end read the other's capability bit, at its own table's place.
        assert!(digital.both_v92() && analogue.both_v92());
        assert!(!digital.short_phase2_agreed() && !analogue.short_phase2_agreed());
        // Bit 70 said the channel carries PCM upstream, and it was the
        // digital modem's verdict rather than the probe's carrier flag.
        assert!(analogue.info1c().is_some_and(|d| d.pcm_upstream()), "INFO1d bit 70 was clear");
        let asked = analogue.info1a_pcm_up().expect("the analogue modem sent no Table 18 INFO1a");
        assert_eq!(digital.info1a_pcm_up(), Some(asked));
        assert_eq!(digital.refused_info1a(), None);
        // And no other INFO1a layout went either way.
        assert!(analogue.info1a().is_none() && analogue.info1a_pcm().is_none());
        assert!(digital.info1a().is_none() && digital.info1a_pcm().is_none());
        // What Table 18 carries: this end's filter capabilities, an MD of
        // nothing, and a codeword the digital modem can build Sd from.
        assert_eq!((asked.sections, asked.ltot_code, asked.lmax_code), (3, 3, 3));
        assert_eq!(asked.md_length, 0);
        assert_eq!(asked.uinfo, 79, "the same ceiling V.90 works to, for the same INFO0d");
        assert!(asked.uinfo_is_sendable());
    }

    /// 9.3: "If either modem does not indicate V.92 capability, then the
    /// digital modem and analogue modem shall use the information bits
    /// defined in 8.2.3.2 of ITU-T V.90" -- so a V.92 analogue modem that
    /// meets a V.90 server gets V.90's Table 10, and bit 70 of the INFO1d it
    /// reads is a carrier flag it must not take for a permission.
    #[test]
    fn a_v92_analogue_modem_meets_a_v90_digital_modem_with_table_10() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, Pcm::Digital(server()), analogue_v92(true));
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        assert!(!analogue.both_v92() && !digital.both_v92());
        assert_eq!(analogue.far_pcm_flags(), PcmFlags::default());
        assert!(analogue.info1a_pcm_up().is_none(), "Table 18 went to a modem that never offered V.92");
        let asked = analogue.info1a_pcm().expect("no Table 10 INFO1a");
        assert_eq!(digital.info1a_pcm(), Some(asked));
    }

    /// The mirror image: a V.92 server and a V.90 analogue modem, which sends
    /// zeros in INFO0a bits 26:27 because V.90 reserves them.
    #[test]
    fn a_v90_analogue_modem_meets_a_v92_digital_modem_with_table_10() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, digital_v92(true), Pcm::Analogue);
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        assert!(!digital.both_v92());
        assert_eq!(digital.far_pcm_flags(), PcmFlags::default());
        assert!(digital.info1a_pcm().is_some() && digital.info1a_pcm_up().is_none());
        // The INFO1d it sent kept bit 70 as V.90's 3429 high-carrier flag: the
        // verdict is written over it only when both ends are V.92.
        assert!(!digital.sent_pcm_upstream);
    }

    /// 8.4.1: "The analogue modem shall not use this sequence if bit 70 of
    /// INFO1d is clear." Both modems are V.92 here, but the digital one says
    /// the channel will not carry PCM upstream, so Table 10 is what goes.
    #[test]
    fn bit_70_clear_means_no_table_18() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, digital_v92(false), analogue_v92(true));
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        assert!(digital.both_v92() && analogue.both_v92(), "the flags crossed all the same");
        assert!(!analogue.info1c().expect("no INFO1d").pcm_upstream(), "bit 70 was set");
        assert!(analogue.info1a_pcm_up().is_none(), "Table 18 went with bit 70 clear");
        assert!(analogue.info1a_pcm().is_some() && digital.info1a_pcm().is_some());
    }

    /// `+PIG=1`, "PCM upstream ignore": the channel would carry it and the far
    /// end offers it, but this end does not want it. 9.3 makes Table 18 a
    /// "may", so V.90 data mode is the answer.
    #[test]
    fn pig_off_means_no_table_18() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, digital_v92(true), analogue_v92(false));
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        assert!(analogue.info1c().is_some_and(|d| d.pcm_upstream()), "bit 70 should still be set");
        assert!(analogue.info1a_pcm_up().is_none());
        assert!(digital.info1a_pcm().is_some(), "the server took Table 10");
    }

    /// The fallback ladder's upper rung: PCM upstream would not hold, so the
    /// next INFO1a asks for V.90 data mode instead -- and PCM downstream is
    /// kept, which is what makes this a rung of its own rather than
    /// `decline_pcm`.
    #[test]
    fn declining_pcm_upstream_gives_table_10_next_time() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, mut analogue) = pcm_call(&mut line, digital_v92(true), analogue_v92(true));
        assert!(analogue.info1a_pcm_up().is_some(), "no Table 18 the first time round");

        analogue.decline_pcm_upstream();
        let mut again = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = run(&mut again, 12.0, digital.again(), analogue.again());
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        assert!(analogue.info1a_pcm_up().is_none(), "Table 18 again after declining it");
        assert!(analogue.info1a_pcm().is_some() && digital.info1a_pcm().is_some());
        // The offer is still on the table; only this end's answer changed.
        assert!(analogue.both_v92());
        assert!(analogue.info1c().is_some_and(|d| d.pcm_upstream()));
    }

    /// 9.3: "any subsequent retrains shall use Phase 2 of ITU-T V.92", and
    /// 11.5 does not repeat INFO0 -- so the flags of the one exchange there
    /// was have to survive, or the retrain would drop to V.90's layouts and
    /// the two ends would disagree about what bit 70 means. 9.7's four
    /// re-entry points are all in the full procedure, so a pair that had
    /// agreed a short phase 2 stops agreeing one.
    #[test]
    fn a_retrain_between_v92_modems_keeps_the_flags_and_runs_full_phase_2() {
        let short = |pcm: Pcm| match pcm {
            Pcm::AnalogueV92(w) => Pcm::AnalogueV92(V92Wish { short_phase2: true, ..w }),
            Pcm::DigitalV92(d, w) => Pcm::DigitalV92(d, V92Wish { short_phase2: true, ..w }),
            other => other,
        };
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) =
            pcm_call(&mut line, short(digital_v92(true)), short(analogue_v92(true)));
        assert!(digital.short_phase2_agreed() && analogue.short_phase2_agreed(), "all four bits are set");

        let (before_d, before_a) = (digital.far_pcm_flags(), analogue.far_pcm_flags());
        let mut again = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = run(&mut again, 12.0, digital.again(), analogue.again());
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(analogue.status(), Status::Done, "analogue: {}", analogue.phase());
        // No INFO0 crossed this time, and the flags are the ones that did.
        assert_eq!(digital.far_info0_count, 1);
        assert_eq!((digital.far_pcm_flags(), analogue.far_pcm_flags()), (before_d, before_a));
        assert!(digital.both_v92() && analogue.both_v92());
        assert!(!digital.short_phase2_agreed() && !analogue.short_phase2_agreed(), "9.7 re-enters full phase 2");
        // Which is to say the V.92 layouts are still in use: a full phase 2
        // with its probing, an INFO1d with bit 70, and Table 18 again.
        assert!(analogue.reading().is_some(), "a full phase 2 probes");
        assert!(analogue.info1a_pcm_up().is_some() && digital.info1a_pcm_up().is_some());
    }

    /// P2P P1 and P2S N-3: V.34's Table 14 has the transmit clock source in
    /// bits 26:27, so those bits are read as V.92's flags only in a PCM phase
    /// 2, never in V.34's own.
    ///
    /// The other direction cannot be defended from inside phase 2. A V.92
    /// digital modem reading a V.34 modem's INFO0a has nothing in the frame to
    /// tell "V.92 capability" from "synchronized to the receive timing", and
    /// 9.3 offers no second signal. What that costs is bounded and is pinned
    /// below: the digital modem believes the bit, writes its PCM-upstream
    /// verdict into INFO1d bit 70 where V.90 would have put the 3429 carrier
    /// flag, and the call still lands on V.34, because a V.34 modem answers
    /// with Table 11 whatever bit 70 said.
    #[test]
    fn a_v34_info0_with_a_clock_of_1_is_not_taken_for_v92() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let mut v34 = Modem::new(Role::Answer, FS);
        // "Transmit clock source: synchronized to the receive timing", which
        // sits in the two bits V.92's INFO0a uses.
        v34.ours.clock = 1;
        // The constructor queued an INFO0 with the clock that was there; this
        // test means the other one.
        v34.tx.stop();
        let bits = v34.info0_bits();
        assert!(bits[26] && !bits[27], "the clock should still go out as a clock");
        v34.tx.send(&bits);
        let (digital, v34) = run(&mut line, 12.0, Modem::v90(digital_v92(true), FS), v34);
        assert_eq!(digital.status(), Status::Done, "digital: {}", digital.phase());
        assert_eq!(v34.status(), Status::Done, "V.34: {}", v34.phase());
        // The V.34 end reads no flags at all: it has no PCM role.
        assert!(!v34.both_v92());
        assert_eq!(v34.far_pcm_flags(), PcmFlags::default());
        // And it could not: an INFO0d's bits 26:27 are parked above V.34's own
        // two on the way in, so they are not a clock source there either.
        assert_eq!(v34.far_capabilities().map(|f| f.clock & 3), Some(0), "an INFO0d read as a clock");
        // The digital end cannot tell, and does believe it -- and nothing
        // comes of it, because the far end asked for V.34.
        assert!(digital.both_v92(), "no frame bit could say otherwise");
        assert!(digital.info1a().is_some(), "the server did not take V.34's INFO1a");
        assert!(digital.info1a_pcm_up().is_none() && digital.info1a_pcm().is_none());
    }

    /// 9.3.1: "If both modems indicate V.92 capability as well as indicating
    /// LAPM protocol in ITU-T V.8 or ITU-T V.8 bis, then the V.42 ODP/ADP
    /// exchange shall be bypassed." Phase 2 knows the first half; the second
    /// is the V.8 menus, and both are needed.
    #[test]
    fn the_lapm_bypass_needs_both_v92_and_both_prot0() {
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, digital_v92(true), analogue_v92(true));
        for (who, m) in [("digital", &digital), ("analogue", &analogue)] {
            assert!(m.lapm_bypass_allowed(true), "{who} would not bypass with both halves");
            assert!(!m.lapm_bypass_allowed(false), "{who} bypassed without LAPM in prot0");
        }
        // A V.90 pair never bypasses, however the menus read.
        let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
        let (digital, analogue) = pcm_call(&mut line, Pcm::Digital(server()), Pcm::Analogue);
        assert!(!digital.lapm_bypass_allowed(true) && !analogue.lapm_bypass_allowed(true));
    }

    /// P2P P15: a Table 18 INFO1a is one the digital modem may act on only if
    /// both modems indicated V.92 (9.3) and the INFO1d it sent had bit 70 set
    /// (8.4.1). A frame whose CRC checked but whose layout fails either test
    /// is counted as not received, which leaves the recovery of 9.2.1.2.6 to
    /// run rather than ending the call on a frame the far end may yet repeat.
    #[test]
    fn a_table_18_info1a_this_end_did_not_invite_is_counted_as_not_received() {
        let asked = Info1aPcmUp { sections: 3, ltot_code: 0, lmax_code: 0, md_length: 0, uinfo: 90 };

        // A V.92 pair whose INFO1d said the channel will not carry PCM
        // upstream, and an analogue modem that asks for it regardless.
        let mut m = Modem::v90(digital_v92(false), FS);
        m.far_flags = PcmFlags { v92: true, short_phase2: false };
        m.stage = Stage::CallInfo1;
        m.heard(Info::Info1aPcmUp(asked));
        assert_eq!(m.status(), Status::Running, "the frame was acted on");
        assert!(m.info1a_pcm_up().is_none());
        assert_eq!(m.refused_info1a(), Some("a table 18 info1a after an info1d that cleared bit 70"));

        // And the same frame from a far end that never indicated V.92, with
        // bit 70 set, so that only the capability test can refuse it.
        let mut m = Modem::v90(digital_v92(true), FS);
        m.stage = Stage::CallInfo1;
        m.sent_pcm_upstream = true;
        m.heard(Info::Info1aPcmUp(asked));
        assert_eq!(m.status(), Status::Running);
        assert!(m.info1a_pcm_up().is_none());
        assert_eq!(m.refused_info1a(), Some("a table 18 info1a from a far end that did not indicate v.92"));

        // With both, it is the INFO1a this phase 2 was waiting for.
        let mut m = Modem::v90(digital_v92(true), FS);
        m.far_flags = PcmFlags { v92: true, short_phase2: false };
        m.stage = Stage::CallInfo1;
        m.sent_pcm_upstream = true;
        m.heard(Info::Info1aPcmUp(asked));
        assert_eq!(m.status(), Status::Done);
        assert_eq!(m.info1a_pcm_up(), Some(asked));
        assert_eq!(m.refused_info1a(), None);
    }

    /// Tables 15 and 16/V.92 put the same two flags the other way round, which
    /// is the one trap in the clause: INFO0d bit 26 asks for a short phase 2
    /// and bit 27 says V.92, and INFO0a has them swapped.
    #[test]
    fn the_info0_flags_go_out_where_each_table_puts_them() {
        use crate::v34::info::{INFO0_BITS, INFO0D_BITS};
        let asking = V92Wish { capable: true, short_phase2: true, pcm_upstream: true, up_caps: caps() };
        let capable = V92Wish { short_phase2: false, ..asking };

        let bits = Modem::v90(Pcm::DigitalV92(server(), capable), FS).info0_bits();
        assert_eq!(bits.len(), INFO0D_BITS);
        assert!(!bits[26] && bits[27], "INFO0d: 26 is the request, 27 the capability");
        let bits = Modem::v90(Pcm::DigitalV92(server(), asking), FS).info0_bits();
        assert!(bits[26] && bits[27]);
        assert_eq!(Info0d::from_bits(&bits).map(|d| d.pcm_flags()), Some(asking.flags()));

        let bits = Modem::v90(Pcm::AnalogueV92(capable), FS).info0_bits();
        assert_eq!(bits.len(), INFO0_BITS);
        assert!(bits[26] && !bits[27], "INFO0a: 26 is the capability, 27 the request");
        let bits = Modem::v90(Pcm::AnalogueV92(asking), FS).info0_bits();
        assert!(bits[26] && bits[27]);
        assert_eq!(Info0::from_bits(&bits).map(|a| a.pcm_flags()), Some(asking.flags()));
    }

    /// The V.90 constructors mean "not V.92", and mean it on the wire: a pair
    /// that offers nothing puts the same bits out as it did before V.92 was
    /// written, which is what keeps every V.90 call unchanged.
    #[test]
    fn the_v90_constructors_still_send_what_they_always_sent() {
        let nothing = V92Wish::default();
        for (v90, v92) in [
            (Pcm::Analogue, Pcm::AnalogueV92(nothing)),
            (Pcm::Digital(server()), Pcm::DigitalV92(server(), nothing)),
        ] {
            let was = Modem::v90(v90, FS).info0_bits();
            assert_eq!(was, Modem::v90(v92, FS).info0_bits(), "{v90:?}");
            assert!(!was[26] && !was[27], "{v90:?}: V.90 sets both of these to 0");
            assert_eq!(v90.wish(), nothing);
            assert_eq!(v90.role(), v92.role());
        }
    }
}
