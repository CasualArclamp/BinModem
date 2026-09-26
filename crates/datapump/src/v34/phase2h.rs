//! Phase 2 of the half-duplex start-up (12.2): probing one way.
//!
//! Half-duplex V.34 carries a fax (T.30 Annex F). The page goes one way, from
//! the modem clause 12 calls the source to the one it calls the recipient, and
//! either of them may be the modem that dialled. So this phase 2 has two axes
//! where duplex's has one: call or answer modem, which sets the carrier and
//! the tone as it does in 11.2, and source or recipient, which sets what the
//! modem does with them. That makes four stage chains -- the call modem as
//! source with the answer modem as recipient (12.2.1), and the answer modem as
//! source with the call modem as recipient (12.2.2) -- but the two source
//! chains are one procedure on different tones, and so are the two recipient
//! chains, which is how they are built here.
//!
//! What happens (Figures 23 and 24):
//!
//! 1. Both ends send an INFO0 and follow it with their tone, as in duplex
//!    (12.2.1.1.1, 12.2.1.2.1).
//! 2. The recipient, hearing the source's tone and having sent its own for
//!    50 ms, reverses its tone, keeps it 10 ms more and falls silent
//!    (12.2.1.2.3). The source answers with a reversal of its own 40 ms later,
//!    keeps its tone 10 ms, then sends L1 for 160 ms and L2 after it
//!    (12.2.1.1.3). There is no second pair of reversals and no round trip is
//!    measured: only the source probes.
//! 3. The recipient reads at most 500 ms of L2 and sends its tone; the source
//!    hears it over the echo of its own L2, stops probing and sends its tone
//!    back (12.2.1.2.5, 12.2.1.1.4).
//! 4. The recipient, hearing that, keeps its tone 25 ms more and sends INFOh:
//!    the symbol rate, carrier, pre-emphasis and power the source is to send
//!    the page with, and how much TRN to train on (12.2.1.2.6). INFOh stands
//!    in for both of duplex's INFO1. Phase 3 follows (12.3).
//!
//! Every wait has a way out (12.2.1.3, 12.2.1.4, 12.2.2.3, 12.2.2.4), and
//! the waits are fixed times rather than round trips: 2000 ms, and 2700 ms
//! for the source's wait on its probe. The building blocks are duplex's,
//! from [`super::phase2`]: the DPSK modulator with its tones and reversals,
//! the reversal detector, the presence detector that hears a tone over the
//! echo of this end's own L2, and the probe generator and analyser.

use dsp::{OnePole, ReversalDetector, ToneDetector};

use super::dpsk;
use super::info::{Info, Info0, InfoH, Probed, SymbolRate};
use super::phase2::{
    self, AFTER_REVERSAL, AUDIBLE, GIVE_UP, L2_SETTLE, Presence, RETRAIN_SILENCE, REVERSAL_BANDWIDTH, Speaking,
    TONE_HELD, TURN,
};
pub use super::phase2::{Role, Status};
use super::probe::{self, Analyzer, Reading};
use super::signals::Size;

/// Which way the page goes: the second axis of half-duplex's phase 2, beside
/// [`Role`].
///
/// Clause 12 has "unidirectional transmission of primary channel data from
/// source to recipient modem", and either modem may be either. T.30 Annex F
/// has the calling fax send the page as a rule, and the answering one when
/// it is polled for a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Sends the page: probes the line with L1 and L2, and is told by INFOh
    /// what to send it with (12.2.1.1, 12.2.2.2).
    Source,
    /// Receives the page: reads the probe and chooses, in INFOh (12.2.1.2,
    /// 12.2.2.1).
    Recipient,
}

/// The recipient's own tone before its reversal: "After Tone B is detected
/// and Tone A has been transmitted for at least 50 ms" (12.2.1.2.3), and the
/// same of tone B in 12.2.2.1.3. Figure 23 draws the 50 ms from the tone's
/// start, Figure 24 from the far tone's detection; the text asks for both.
const TONE_BEFORE_REVERSAL: f64 = 0.050;

/// How far behind its bits the DPSK modulator's pulse puts a signal on the
/// line: six symbols at 600 baud. The tone after an INFO0 is on the line that
/// long after the INFO0 has drained from the queue, and a reversal is on the
/// very sample, so the 50 ms are counted from the later of the two.
const PULSE_SPAN: f64 = 0.010;

/// How much L2 the recipient reads, from L2's start: "may then receive L2 for
/// a period of time not to exceed 500 ms" (12.2.1.2.5, 12.2.2.1.5).
///
/// More than duplex reads, and less than it may. Duplex stops at 300 ms so
/// that its tone reaches a far end whose own wait for it is 600 ms less a
/// round trip; here the source waits 2700 ms for it (12.2.1.3.3), so the only
/// limit is the recipient's own, and Figures 23 and 24 put the tone within
/// 670 ms of the source's reversal, which is 10 ms of tone, 160 of L1 and the
/// 500. Four hundred leaves 100 ms of that, and gives the analyser nineteen
/// windows rather than fourteen for the one reading half-duplex gets: there
/// is no INFO1c from the other end to set beside it.
const L2_READ: f64 = 0.400;

/// "the answer modem continues transmitting Tone A for 25 ms, then sends
/// INFOh" (12.2.1.2.6), and the call modem the same of tone B (12.2.2.1.6).
const TONE_BEFORE_INFOH: f64 = 0.025;

/// The source's wait for the recipient's tone after its own reversal: "If, in
/// 12.2.1.1.4, Tone A is not detected within 2700 ms from transmission of the
/// Tone B phase reversal" (12.2.1.3.3; 12.2.2.4.3 for the answer modem).
const PROBE_WAIT: f64 = 2.7;

/// Every other wait of 12.2's recoveries: 2000 ms, from the source's tone to
/// INFOh (12.2.1.3.4, 12.2.2.4.4), from the recipient's reversal to the
/// source's (12.2.1.4.2, 12.2.2.3.2), and from the recipient's tone after the
/// probe to the source's (12.2.1.4.3, 12.2.2.3.3).
const WAIT: f64 = 2.0;

/// What the recipient asks the source's TRN to be: 29 steps of 35 ms, which is
/// 1015 ms.
///
/// About the second duplex sends in its phase 3, and the 1.1 to 1.9 s the two
/// real modems it was checked against sent; and four times the least the
/// receiver wants, the 512 symbols its slip retry searches, which at 2400 Bd
/// is seven steps. TRN is sent once a call, and a second of it costs nothing
/// a fax notices.
const TRN_STEPS: u8 = 29;

/// How long the far carrier's amplitude has to go without a dip before it is
/// a tone rather than a sequence: longer than any run of unchanging phase an
/// INFO0 has in it -- eighteen equal bits, which none of the fixed bits and
/// no likely capabilities and CRC make -- and well short of the 50 ms a
/// recipient's tone runs before its reversal.
const STEADY: f64 = 0.030;

/// How far a tone's amplitude may fall, against its own slow envelope, and
/// still be steady. A phase reversal takes it through nothing.
const DIP: f64 = 0.5;

/// How near the end of a steady stretch has to be to a reversal for the
/// reversal to be the end of it: the detector's own few milliseconds of
/// dipping, and the reversal detector's backdating.
const ENDED_NEAR: f64 = 0.008;

/// The far carrier with nothing on it -- the far end's tone, as against its
/// INFO0 on the same carrier -- told by its amplitude through a detector
/// wide enough to follow a symbol. Every one bit of an INFO sequence is a
/// phase reversal, and a reversal takes the amplitude through nothing on its
/// way round; a tone's amplitude never dips.
///
/// [`Presence`] cannot tell the two apart. Its detector is ten hertz wide, so
/// as to hear a tone under the echo of this end's own L2, and at that width a
/// sequence's reversals average into a smaller amplitude that looks steady
/// enough: the far end's INFO0 read as its tone held for 20 ms, and had this
/// end sending INFO0 again for a tone that came "before" an INFO0 that was
/// still arriving. So `Presence` is the judge where L2 is on the line, and
/// this is the judge where a sequence might be.
///
/// It is also the judge a modem in data or control-channel mode wants for a
/// far end that begins a primary channel retrain (12.7.1.2, 12.7.2.2): the
/// control channel is 600 baud on the same two carriers, and its symbols dip
/// the amplitude as a sequence's do, so a carrier held steady for 50 ms is
/// the tone and nothing else is. [`Steady::held`] counts it.
#[derive(Debug, Clone)]
pub(crate) struct Steady {
    fast: ToneDetector,
    /// The amplitude, slowly: what a dip is a dip below.
    envelope: OnePole,
    /// Samples fed.
    now: u64,
    /// Samples the amplitude has gone without a dip.
    held: u64,
    /// [`STEADY`], in samples.
    steady: u64,
    /// When the last stretch of [`STEADY`] or more ended, in a dip.
    ended: Option<u64>,
}

impl Steady {
    /// A judge of the carrier at `freq`: 1200 Hz for the call modem's tone B
    /// and sequences, 2400 for the answer modem's tone A and sequences.
    pub(crate) fn new(freq: f64, fs: f64) -> Self {
        Self {
            fast: ToneDetector::new(freq, REVERSAL_BANDWIDTH, fs),
            envelope: OnePole::new(0.020, fs),
            now: 0,
            held: 0,
            steady: (STEADY * fs).round() as u64,
            ended: None,
        }
    }

    pub(crate) fn feed(&mut self, x: f64) {
        self.now += 1;
        self.fast.feed(x);
        let amplitude = self.fast.amplitude();
        let envelope = self.envelope.process(amplitude);
        if amplitude > AUDIBLE && amplitude > DIP * envelope {
            self.held += 1;
        } else {
            if self.held >= self.steady {
                self.ended = Some(self.now);
            }
            self.held = 0;
        }
    }

    /// Whether the far carrier is a tone now: steady for [`STEADY`].
    pub(crate) fn is_tone(&self) -> bool {
        self.held() >= self.steady
    }

    /// How long the far carrier has been steady, in samples, and nought
    /// while anything is on it or it is away.
    pub(crate) fn held(&self) -> u64 {
        self.held
    }

    /// Whether the far carrier was a tone up to `at`, and dipped there: the
    /// tone reversed then, rather than a sequence happening to hold its
    /// phase for a while and then change it.
    fn was_tone_until(&self, at: u64, near: u64) -> bool {
        self.ended.is_some_and(|ended| ended.abs_diff(at) <= near)
    }
}

/// Where phase 2 has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    // The source (12.2.1.1 as the call modem, 12.2.2.2 as the answer modem).
    /// INFO0 and then the tone; the far INFO0 awaited.
    SourceInfo0,
    /// The tone; the recipient's tone and then its reversal awaited.
    SourceAwaitReversal,
    /// The reversal 40 ms on, 10 ms of tone, L1, L2; the recipient's tone
    /// awaited over the echo.
    SourceProbing,
    /// The tone; INFOh awaited.
    SourceAwaitInfoH,
    // The recipient (12.2.1.2 as the answer modem, 12.2.2.1 as the call modem).
    /// INFO0 and then the tone; the far INFO0 awaited.
    RecipientInfo0,
    /// The tone; the source's tone awaited, and 50 ms of this end's own, for
    /// the reversal.
    RecipientAwaitTone,
    /// Silent after the reversal; the source's reversal awaited.
    RecipientAwaitReversal,
    /// L1 going past and L2 being read.
    RecipientReadProbe,
    /// The tone; the source's tone awaited, then 25 ms more and INFOh.
    RecipientAwaitTone2,
    /// INFOh going out.
    RecipientInfoH,
    // Both.
    Finished,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::SourceInfo0 | Self::RecipientInfo0 => "V.34 INFO0",
            Self::SourceAwaitReversal | Self::RecipientAwaitTone | Self::RecipientAwaitReversal => "V.34 tones",
            Self::SourceProbing => "V.34 sending the probe",
            Self::RecipientReadProbe => "V.34 reading the probe",
            Self::SourceAwaitInfoH | Self::RecipientAwaitTone2 | Self::RecipientInfoH => "V.34 INFOh",
            Self::Finished => "V.34 phase 2 done",
        }
    }
}

/// Half-duplex phase 2, one end of it.
#[derive(Debug, Clone)]
pub struct Modem {
    role: Role,
    part: Part,
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
    /// INFOh goes at this sample, the 25 ms of tone before it being up.
    infoh_at: Option<u64>,
    /// Since when this end's carrier has had nothing on it but the tone: the
    /// INFO0 ahead of it drained from the queue then. None while a sequence
    /// is going out, or nothing is.
    tone_since: Option<u64>,

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
    /// The far carrier as a tone rather than a sequence.
    steady: Steady,
    /// The far end's tone has been away since this end began waiting for it
    /// to come again.
    far_tone_gone: bool,
    /// L1 and L2 have gone out at least once, so that an INFOh can be about
    /// them.
    probed: bool,
    reading: Option<Reading>,
    infoh: Option<InfoH>,
    /// When INFOh began going out.
    infoh_sent_at: Option<u64>,
    /// INFOh went without the source's tone having been heard.
    blind: bool,
    /// Times a wait of 12.2's recoveries ran out and the way out was taken.
    recoveries: u32,
    /// INFO0 sequences sent beyond the first (12.2.1.3.1 and its three
    /// siblings).
    info0_repeats: u32,
}

impl Modem {
    /// Every field at rest: silent, at the finished stage, nothing heard. The
    /// ways in fill in the rest.
    fn blank(role: Role, part: Part, fs: f64) -> Self {
        let far_tone = role.far().carrier();
        Self {
            role,
            part,
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
            infoh_at: None,
            tone_since: None,
            // INFO0 from either modem, and INFOh from the recipient (10.2.2).
            rx: dpsk::Receiver::half_duplex(role.far(), fs),
            reversals: ReversalDetector::new(far_tone, REVERSAL_BANDWIDTH, AUDIBLE, fs),
            presence: Presence::new(far_tone, fs),
            ignore_reversals_until: 0,
            analyzer: Analyzer::new(fs),
            read_from: u64::MAX,
            read_until: u64::MAX,
            ours: phase2::Modem::capabilities(),
            far: None,
            steady: Steady::new(far_tone, fs),
            far_tone_gone: false,
            probed: false,
            reading: None,
            infoh: None,
            infoh_sent_at: None,
            blind: false,
            recoveries: 0,
            info0_repeats: 0,
        }
    }

    /// Phase 2 from its start: the 75 ms of silence that end phase 1 have
    /// already gone.
    pub fn new(role: Role, part: Part, fs: f64) -> Self {
        Self::with_capabilities(role, part, fs, phase2::Modem::capabilities())
    }

    /// Phase 2 from its start, with `ours` for this end's INFO0.
    fn with_capabilities(role: Role, part: Part, fs: f64, ours: Info0) -> Self {
        let mut modem = Self::blank(role, part, fs);
        modem.ours = ours;
        modem.stage = match part {
            Part::Source => Stage::SourceInfo0,
            Part::Recipient => Stage::RecipientInfo0,
        };
        modem.speaking = Speaking::Carrier;
        // 12.2.1.1.1 and its three siblings: INFO0 "with bit 28 set to 0,
        // followed by" this end's tone -- which the modulator carries on into
        // by itself.
        let bits = modem.ours.to_bits();
        modem.tx.send(&bits);
        modem
    }

    /// Phase 2 as a primary channel retrain (12.7): the capabilities were
    /// exchanged the first time and are not again, so this end goes straight
    /// to 70 ms of silence, its tone, and the tone exchange of 12.2.1.1.3,
    /// 12.2.1.2.3, 12.2.2.1.3 or 12.2.2.2.3. `ours` and `far` are the two
    /// INFO0 of the first time round, kept so INFOh can be chosen from them.
    ///
    /// 12.7.1.1 and 12.7.2.1 have the modem that starts a retrain send its
    /// tone and listen for the far end's; 12.7.1.2 and 12.7.2.2 have the one
    /// answering, which has heard the far tone for 50 ms already, send its
    /// tone. From the tone on the two are the same, so both enter the same
    /// way, and which this end is belongs to whatever owns it.
    pub fn retrain(role: Role, part: Part, fs: f64, ours: Info0, far: Info0) -> Self {
        let mut modem = Self::blank(role, part, fs);
        modem.ours = ours;
        modem.far = Some(far);
        // "transmit silence for 70 ± 5 ms", then the tone.
        modem.tone_at = Some(modem.ms(RETRAIN_SILENCE));
        // The step from the data or the control channel into this is not a
        // reversal, however it reads.
        modem.ignore_reversals_until = modem.ms(RETRAIN_SILENCE + 0.050);
        modem.stage = match part {
            Part::Source => Stage::SourceAwaitReversal,
            Part::Recipient => Stage::RecipientAwaitTone,
        };
        modem
    }

    /// A retrain of this phase 2 (12.7): the same end, the same far end.
    pub fn again(&self) -> Self {
        Self::retrain(self.role, self.part, self.fs, self.ours, self.far.unwrap_or_default())
    }

    /// The recipient's way back from a phase 3 that failed (12.3.3): "if
    /// signal S is not detected within 2000 ms or TRN is not satisfactorily
    /// received", the answer modem "shall condition its receiver to detect
    /// Tone B and shall transmit Tone A and proceed in accordance with
    /// 12.2.1.2.6", and the call modem the same of the other tones and
    /// 12.2.2.1.6. So: this end's tone at once, the source's awaited, 25 ms
    /// more and INFOh again -- the same INFOh, since the line was read once
    /// and what was chosen from the reading stands. The source, still in
    /// phase 2 for want of the first INFOh, is waiting for exactly this
    /// (12.2.1.3.4, 12.2.2.4.4).
    ///
    /// For a recipient; a source has no such way back into phase 2.
    pub fn infoh_again(&self) -> Self {
        debug_assert_eq!(self.part, Part::Recipient, "only a recipient sends INFOh");
        let mut modem = Self::blank(self.role, self.part, self.fs);
        modem.ours = self.ours;
        modem.far = self.far;
        modem.reading = self.reading.clone();
        modem.start_tone();
        modem.enter(Stage::RecipientAwaitTone2);
        modem.deadline = Some(modem.ms(WAIT));
        modem
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn part(&self) -> Part {
        self.part
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

    /// This end's INFO0, as it went.
    pub fn capabilities(&self) -> Info0 {
        self.ours
    }

    /// The far end's INFO0, once it has arrived.
    pub fn far_capabilities(&self) -> Option<Info0> {
        self.far
    }

    /// INFOh as it was sent or received: what the source sends the page with
    /// from phase 3 on, and how much TRN it trains the recipient on. Some
    /// once phase 2 is done.
    pub fn infoh(&self) -> Option<InfoH> {
        self.infoh
    }

    /// Whether a recipient's INFOh went without the source's tone having been
    /// heard (12.2.1.4.3, 12.2.2.3.3): the source may not be listening for it.
    pub fn blind(&self) -> bool {
        self.blind
    }

    /// What a recipient made of the source's L2. A source reads nothing.
    pub fn reading(&self) -> Option<&Reading> {
        self.reading.as_ref()
    }

    /// How many times a wait of 12.2's recoveries ran out and its way out
    /// was taken. Phase 2 survives them, and a clean line should need none.
    pub fn recoveries(&self) -> u32 {
        self.recoveries
    }

    /// INFO0 sequences sent beyond the first, because the far end's tone came
    /// before its INFO0 or its INFO0 came again (12.2.1.3.1, 12.2.1.4.1,
    /// 12.2.2.3.1, 12.2.2.4.1).
    pub fn info0_repeats(&self) -> u32 {
        self.info0_repeats
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
        self.steady.feed(line);
        // A probing signal -- the source's, arriving, or the echo of this
        // end's own -- leaves the reversal detector sure the tone after it is
        // far off frequency (see `ReversalDetector::restart`).
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
                // 12.2 has waits with no end -- a source whose reversal is
                // never answered sends its tone for ever (12.2.1.3.2) -- and
                // this is where they end.
                self.fail("phase 2 went on for twenty seconds");
            } else {
                self.timers();
                self.note_tone();
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
            self.since = self.now;
        }
        if self.reverse_at == Some(self.now) {
            self.reverse_at = None;
            self.tx.reverse();
            self.reversed_at = Some(self.now);
            // The tone "is transmitted for another 10 ms after the phase
            // reversal" (12.2.1.1.3, 12.2.1.2.3, 12.2.2.1.3, 12.2.2.2.3).
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
        if self.infoh_at == Some(self.now) {
            self.infoh_at = None;
            self.send_infoh();
        }
    }

    /// Note since when this end's carrier has carried nothing but the tone.
    /// An INFO0 and the tone after it share the carrier (10.1.2.3.1), and the
    /// recipient's 50 ms of tone before its reversal count from when the
    /// INFO0 -- the first or a repeat -- is out of the way.
    fn note_tone(&mut self) {
        if matches!(self.speaking, Speaking::Carrier) && self.tx.pending() == 0 {
            self.tone_since.get_or_insert(self.now);
        } else {
            self.tone_since = None;
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

    /// Whether the far end's INFO0 is still being listened for: until the
    /// far end's reversal has been heard, or this end's has been answered.
    /// 12.2.1.3.1 and its siblings reach that far ("in 12.2.1.1.2 or
    /// 12.2.1.1.3"), and the recipient's wait for the source's reversal is
    /// taken in as well, for the reason given where it is handled. One heard
    /// after that is a damaged INFOh that read as an INFO0, which one wrong
    /// symbol in the right place makes of it.
    fn awaiting_info0(&self) -> bool {
        matches!(
            self.stage,
            Stage::SourceInfo0
                | Stage::SourceAwaitReversal
                | Stage::RecipientInfo0
                | Stage::RecipientAwaitTone
                | Stage::RecipientAwaitReversal
        )
    }

    fn heard(&mut self, info: Info) {
        // Every sequence ends in fill ones, which are reversals, and the
        // detector reports them late.
        self.ignore_reversals_until = self.now + self.ms(0.040);
        match info {
            Info::Info0(far) if self.awaiting_info0() => {
                let repeated = self.far.is_some();
                self.far = Some(far);
                // The NOTE to each recovery clause: bit 28 of this end's INFO0
                // is set to 1 "after correctly receiving" the far end's.
                self.ours.acknowledge = true;
                // "or repeated INFO0a is received, the modem will repeatedly
                // send INFO0c" (12.2.1.3.1, and the three like it). One for
                // each, and only while the far end's bit 28 says it has not
                // got ours: it repeats in answer to our repeats as well, and
                // across a long line the two would go on answering each
                // other's answers.
                if repeated && !far.acknowledge && self.tx.pending() == 0 {
                    self.repeat_info0();
                    // Repeated after this end's reversal: the source is still
                    // at 12.2.1.1.1, sending INFO0c for want of INFO0a, and
                    // listens for no reversal until it has one (12.2.1.1.2),
                    // so the one this end made is lost on it. 12.2.1.4.1 names
                    // 12.2.1.2.2 and 12.2.1.2.3 and this is the same want a
                    // little later; back to 12.2.1.2.3 with it, since a source
                    // that reads this INFO0a goes on to listen for a reversal
                    // and is owed a fresh one. Left to 12.2.1.4.2's 2000 ms
                    // this end would hear tone B, send its tone and reverse
                    // again 50 ms on, before the source's next INFO0c could
                    // arrive to say it was still short, and the two would go
                    // round like that until phase 2 gave up.
                    if self.stage == Stage::RecipientAwaitReversal {
                        self.speaking = Speaking::Carrier;
                        self.reversed_at = None;
                        self.enter(Stage::RecipientAwaitTone);
                    }
                }
            }
            // The source, once it has probed, takes INFOh in any stage: the
            // recipient sends it only after reading the probe, and a source
            // that has lost its place -- waiting out 2700 ms for a tone it
            // did not hear (12.2.1.3.3), say -- is better told than left to
            // wait for a reversal that is not coming. Before probing there is
            // nothing an INFOh could be about, and a damaged INFO0 can read as
            // one.
            Info::InfoH(infoh) if self.part == Part::Source && self.probed => {
                self.infoh = Some(infoh);
                // 12.2.1.1.4, 12.2.2.2.4: "After receiving INFOh, the modem
                // shall proceed according to 12.3.1", which begins with 70 ms
                // of silence.
                self.tx.stop();
                self.speaking = Speaking::Silent;
                self.status = Status::Done;
                self.enter(Stage::Finished);
            }
            _ => {}
        }
    }

    /// INFO0 once more, on the carrier as it stands, with bit 28 as it now is.
    fn repeat_info0(&mut self) {
        self.info0_repeats += 1;
        let bits = self.ours.to_bits();
        self.tx.send(&bits);
    }

    /// A reversal of the far carrier at `at`, which is a reversal of the far
    /// tone only if the far carrier was a tone up to then: an INFO0 sent
    /// again mid-tone is full of turns of the phase, and one of them can hold
    /// long enough for the detector -- and is what the far end's tone
    /// reversal is heard as by a 12.2 that has already had its INFO0.
    fn reversal(&mut self, at: u64) {
        let tone = self.steady.was_tone_until(at, self.ms(ENDED_NEAR));
        match self.stage {
            // 12.2.1.1.3, 12.2.2.2.3: the recipient's reversal, answered
            // "40 ± 10 ms" later with this end's; then 10 ms more of the tone,
            // L1 for 160 ms and L2. Of the tone, which 12.2.1.1.2 and
            // 12.2.1.3.3 have this end hear first.
            Stage::SourceAwaitReversal if tone => {
                let reverse_at = (at + self.ms(TURN)).max(self.now + 1);
                self.reverse_at = Some(reverse_at);
                self.probe_at = Some(reverse_at + self.ms(AFTER_REVERSAL));
                self.probed = true;
                self.enter(Stage::SourceProbing);
                // 12.2.1.3.3, 12.2.2.4.3: the recipient's tone "within 2700 ms
                // from transmission of the ... phase reversal".
                self.deadline = Some(reverse_at + self.ms(PROBE_WAIT));
            }
            // 12.2.1.2.4, 12.2.2.1.4: the source's reversal. L1 follows it by
            // 10 ms and runs 160 ms, and L2 is read after that.
            Stage::RecipientAwaitReversal if tone => {
                let l2 = at + self.ms(AFTER_REVERSAL + probe::L1_SECONDS);
                self.read_from = l2 + self.ms(L2_SETTLE);
                self.read_until = l2 + self.ms(L2_READ);
                self.analyzer.reset();
                self.enter(Stage::RecipientReadProbe);
            }
            _ => {}
        }
    }

    fn stage_step(&mut self) {
        let now = self.now;
        let held = self.ms(TONE_HELD);
        match self.stage {
            Stage::SourceInfo0 | Stage::RecipientInfo0 => {
                // 12.2.1.3.1 and its siblings: the far tone "detected before
                // correctly receiving" the far INFO0, so this end's is sent
                // again, and again, until the far end's arrives. The tone,
                // not the far INFO0 still arriving on the same carrier.
                if self.far.is_none() && self.steady.is_tone() && self.tx.pending() == 0 {
                    self.repeat_info0();
                }
                // 12.2.1.1.2 and its siblings: the far INFO0 in, and this
                // end's out, so on to the tones.
                if self.far.is_some() && self.tx.pending() == 0 {
                    self.enter(match self.part {
                        Part::Source => Stage::SourceAwaitReversal,
                        Part::Recipient => Stage::RecipientAwaitTone,
                    });
                }
            }
            Stage::SourceAwaitReversal => {
                // 12.2.1.1.2: "detect Tone A and the subsequent Tone A phase
                // reversal", which `reversal` does at once. Without a
                // reversal the tone goes on, with no time limit of the
                // clause's own (12.2.1.3.2, 12.2.2.4.2).
            }
            Stage::SourceProbing => {
                let Speaking::Probe { l1_until } = self.speaking else { return };
                if now > l1_until && self.presence.stood(held) {
                    // 12.2.1.1.4, 12.2.2.2.4: the recipient's tone, heard over
                    // the echo of L2. This end's tone, and INFOh awaited
                    // "within 2000 ms from the transmission of Tone B"
                    // (12.2.1.3.4).
                    self.start_tone();
                    self.enter(Stage::SourceAwaitInfoH);
                    self.deadline = Some(now + self.ms(WAIT));
                } else if self.deadline.is_some_and(|d| now > d) {
                    // 12.2.1.3.3, 12.2.2.4.3: no tone within 2700 ms of the
                    // reversal. The tone, and the far tone and its reversal
                    // awaited again, which probes again.
                    self.recoveries += 1;
                    self.start_tone();
                    self.enter(Stage::SourceAwaitReversal);
                }
            }
            Stage::SourceAwaitInfoH => {
                if self.deadline.is_some_and(|d| now > d) {
                    // 12.2.1.3.4, 12.2.2.4.4: no INFOh within 2000 ms. "The
                    // call modem shall continue to send Tone B and condition
                    // its receiver to detect Tone A", so the tone stays and
                    // the far tone is waited for afresh: the recipient's went
                    // away after its INFOh (12.3.2.1), and comes back with the
                    // next one (12.3.3).
                    self.recoveries += 1;
                    self.deadline = None;
                    self.far_tone_gone = false;
                }
                if self.deadline.is_none() {
                    if self.presence.held == 0 {
                        self.far_tone_gone = true;
                    }
                    if self.far_tone_gone && self.steady.is_tone() {
                        // "Upon detection of Tone A, the call modem proceeds
                        // in accordance with 12.2.1.1.4."
                        self.deadline = Some(now + self.ms(WAIT));
                    }
                }
            }
            Stage::RecipientAwaitTone => {
                // 12.2.1.4.2 and 12.2.2.3.2 send this end back here silent:
                // "condition its receiver to detect Tone B. Upon detecting
                // Tone B, the answer modem transmits Tone A". Not while a
                // retrain's tone is already due after its silence.
                if matches!(self.speaking, Speaking::Silent) && self.tone_at.is_none() && self.steady.is_tone() {
                    self.start_tone();
                    return;
                }
                // 12.2.1.2.3, 12.2.2.1.3: the source's tone heard and this
                // end's own on the line 50 ms. The reversal, 10 ms more of
                // the tone, silence, and the source's reversal awaited "within
                // 2000 ms from the transmission of the Tone A phase reversal"
                // (12.2.1.4.2).
                let own = self.tone_since.is_some_and(|t| now - t >= self.ms(TONE_BEFORE_REVERSAL + PULSE_SPAN));
                if own && self.steady.is_tone() {
                    self.reverse_at = Some(now + 1);
                    self.enter(Stage::RecipientAwaitReversal);
                    self.deadline = Some(now + 1 + self.ms(WAIT));
                    // The reversal is this end's, and the source's answer to
                    // it cannot be back before a round trip and 40 ms.
                    self.ignore_reversals_until = now + self.ms(TURN);
                }
            }
            Stage::RecipientAwaitReversal => {
                if self.deadline.is_some_and(|d| now > d) {
                    // 12.2.1.4.2, 12.2.2.3.2: no reversal from the source
                    // within 2000 ms of this end's. The source's tone awaited,
                    // then this end's tone, and 12.2.1.2.3 again.
                    self.recoveries += 1;
                    self.reversed_at = None;
                    self.enter(Stage::RecipientAwaitTone);
                }
            }
            Stage::RecipientReadProbe => {
                if now >= self.read_until {
                    self.reading = self.analyzer.reading();
                    // 12.2.1.2.5, 12.2.2.1.5: "then transmits Tone A and
                    // conditions its receiver to detect Tone B", and INFOh
                    // goes anyway if tone B is "not detected within 2000 ms
                    // from beginning of transmission of Tone A" (12.2.1.4.3).
                    self.start_tone();
                    self.enter(Stage::RecipientAwaitTone2);
                    self.deadline = Some(now + self.ms(WAIT));
                }
            }
            Stage::RecipientAwaitTone2 => {
                if self.infoh_at.is_some() {
                    return;
                }
                if self.steady.is_tone() {
                    // 12.2.1.2.6, 12.2.2.1.6: the source's tone, after its L2;
                    // 25 ms more of this end's, then INFOh.
                    self.infoh_at = Some(now + self.ms(TONE_BEFORE_INFOH));
                } else if self.deadline.is_some_and(|d| now > d) {
                    // 12.2.1.4.3, 12.2.2.3.3: "sends INFOh, and then proceeds
                    // to Phase 3", tone or no tone.
                    self.recoveries += 1;
                    self.blind = true;
                    self.send_infoh();
                }
            }
            Stage::RecipientInfoH => {
                // 12.3.2.1: "After sending INFOh, the recipient modem transmits
                // silence". Done once the last of it is off the line.
                if !self.tx.is_sending() {
                    self.status = Status::Done;
                    self.enter(Stage::Finished);
                }
            }
            Stage::Finished => {}
        }
    }

    /// INFOh out, straight on from the tone as one group with it, and silence
    /// after it.
    fn send_infoh(&mut self) {
        let infoh = self.settle();
        let bits = infoh.to_bits();
        // Kept as it went, which is to the fields' own resolution.
        self.infoh = InfoH::from_bits(&bits);
        self.infoh_sent_at = Some(self.now);
        self.tx.send(&bits);
        self.tx.silence();
        self.speaking = Speaking::Carrier;
        self.enter(Stage::RecipientInfoH);
    }

    /// INFOh: what the source is to send the page with, from what this end
    /// read of its L2 (Table 22).
    ///
    /// The symbol rate, carrier and pre-emphasis are chosen as duplex's
    /// INFO1a chooses them for the direction it is free in: for each symbol
    /// rate the reading projects a data rate on the better of the carriers
    /// the source can transmit on, and the symbol rate with the highest
    /// projection wins, the lower symbol rate on a tie, since that is the one
    /// with the margin. There is no asymmetry to keep within, the page going
    /// one way. The projections are bounded by what the source's INFO0 says
    /// its transmitter can do and by what this end's own INFO0 says it can --
    /// which here is the same of both directions -- as INFO1a's are bounded
    /// by both ends' INFO0.
    ///
    /// No power reduction is asked for, as none is in INFO1c or INFO1a; and
    /// none may be asked of a source whose INFO0 bit 20 says its transmitter
    /// cannot reduce.
    ///
    /// TRN is [`TRN_STEPS`] of 35 ms, about a second, on sixteen points where
    /// the reading projects more than four bits a symbol at the chosen rate
    /// and four points otherwise. Sixteen points train the equaliser on
    /// something nearer the constellations the page will use, and let the
    /// receiver go on refining on its own decisions of TRN (12.3.2.2); but
    /// those decisions are only worth having where the line can carry
    /// sixteen points, and four bits a symbol through the projection's six
    /// decibel gap is 18 dB of signal to noise, which is about what deciding
    /// sixteen points takes. Below that the page goes at 14 400 or less, and
    /// four points train a receiver well enough for that.
    fn settle(&self) -> InfoH {
        let far = self.far.unwrap_or_default();
        let wide = far.constellation_1664 && self.ours.constellation_1664;
        // The source's transmitter and this end's receiver as one set of
        // capabilities (Table 14, bits 12 to 19), so that the reading is
        // asked about nothing outside both. Bit 19 is the source's alone: it
        // is about transmitting at 3429.
        let both = Info0 {
            rate_2743: far.rate_2743 && self.ours.rate_2743,
            rate_2800: far.rate_2800 && self.ours.rate_2800,
            rate_3429: far.rate_3429 && self.ours.rate_3429,
            low_carrier_3000: far.low_carrier_3000 && self.ours.low_carrier_3000,
            high_carrier_3000: far.high_carrier_3000 && self.ours.high_carrier_3000,
            low_carrier_3200: far.low_carrier_3200 && self.ours.low_carrier_3200,
            high_carrier_3200: far.high_carrier_3200 && self.ours.high_carrier_3200,
            ..far
        };
        let projected =
            |rate: SymbolRate| self.reading.as_ref().map_or_else(Probed::default, |r| r.probed(rate, &both, wide));
        let (symbol_rate, probed) = SymbolRate::ALL
            .iter()
            .map(|&rate| (rate, projected(rate)))
            .filter(|(_, p)| p.max_rate > 0)
            .max_by(|a, b| a.1.max_rate.cmp(&b.1.max_rate).then(b.0.index().cmp(&a.0.index())))
            // Nothing projected -- no reading, or none the source can send
            // at -- and INFOh must still say something: the symbol rate
            // every V.34 modem has, the least of them, and the plainest
            // training.
            .unwrap_or((SymbolRate::S2400, Probed::default()));
        let bits_per_symbol = f64::from(probed.max_rate) * 2400.0 / probe::symbols_per_second(symbol_rate);
        InfoH {
            power_reduction: 0,
            trn_length: TRN_STEPS,
            high_carrier: probed.high_carrier,
            pre_emphasis: probed.pre_emphasis,
            symbol_rate,
            trn_size: if bits_per_symbol > 4.0 { Size::Sixteen } else { Size::Four },
        }
    }
}

#[cfg(test)]
mod tests {
    use dsp::{Cascade, OnePole, butter_lowpass};

    use super::super::phase2::tests::Line;
    use super::*;

    const FS: f64 = 16_000.0;

    /// Something the line does once to one direction, at a moment on the
    /// sending end's clock.
    #[derive(Debug, Clone, Copy)]
    enum Fault {
        /// A stretch played twice: a jitter buffer's concealment, which on
        /// this rig's VoIP line is about 20 ms every few seconds. Twenty
        /// milliseconds is 24 cycles of tone B, 48 of tone A and three
        /// repetitions of L2, so the tones and the probe do not notice one;
        /// an INFO sequence caught by one is lost. The line's delay has to
        /// hold the stretch.
        Insert { at: f64, seconds: f64, towards: Role },
        /// A stretch of silence: a hole.
        Mute { at: f64, seconds: f64, towards: Role },
    }

    /// The line's frequency response.
    #[derive(Debug, Clone, Copy)]
    enum Shape {
        Flat,
        /// Falling away towards the top of the band: one pole with its
        /// corner at 1500 Hz, which is 5.5 dB down at 2400 Hz and 8 at 3500.
        Tilted,
        /// Cut off above 2300 Hz, eighth order.
        Narrow,
    }

    enum Filter {
        Flat,
        Pole(OnePole),
        Cascade(Cascade),
    }

    impl Filter {
        fn new(shape: Shape) -> Self {
            match shape {
                Shape::Flat => Self::Flat,
                Shape::Tilted => Self::Pole(OnePole::new(1.0 / (std::f64::consts::TAU * 1500.0), FS)),
                Shape::Narrow => Self::Cascade(butter_lowpass(8, 2300.0, FS)),
            }
        }

        fn process(&mut self, x: f64) -> f64 {
            match self {
                Self::Flat => x,
                Self::Pole(pole) => pole.process(x),
                Self::Cascade(cascade) => cascade.process(x),
            }
        }
    }

    /// What phase 3 would do at a recipient whose INFOh got no answer: with
    /// no S within 2000 ms of it, back to the tone and INFOh (12.3.3). There
    /// is no phase 3 here, so the harness stands in for it.
    fn stand_in_for_phase_3(recipient: &mut Modem, source: &Modem, done_at: &mut Option<usize>, i: usize) {
        if recipient.part() != Part::Recipient {
            return;
        }
        if recipient.status() == Status::Done && source.status() == Status::Running {
            let at = *done_at.get_or_insert(i);
            if i - at >= (2.0 * FS) as usize {
                *recipient = recipient.infoh_again();
                *done_at = None;
            }
        }
    }

    /// Run the two ends against each other over `line`, shaped and faulted,
    /// for at most `seconds`.
    fn run(
        line: &mut Line,
        seconds: f64,
        mut caller: Modem,
        mut answerer: Modem,
        shape: Shape,
        faults: &[Fault],
    ) -> (Modem, Modem) {
        let (mut shape_to_answer, mut shape_to_call) = (Filter::new(shape), Filter::new(shape));
        let (mut mute_to_answer, mut mute_to_call) = (0usize, 0usize);
        let (mut from_call, mut from_answer) = (0.0, 0.0);
        let (mut caller_done_at, mut answerer_done_at) = (None, None);
        for i in 0..(seconds * FS) as usize {
            for fault in faults {
                match *fault {
                    Fault::Insert { at, seconds, towards } if (at * FS) as usize == i => {
                        let queue = match towards {
                            Role::Answer => &mut line.to_answer,
                            Role::Call => &mut line.to_call,
                        };
                        // The next stretch to be heard, heard twice.
                        let n = ((seconds * FS) as usize).min(queue.len());
                        let again: Vec<f64> = queue.iter().take(n).copied().collect();
                        let mut all: Vec<f64> = queue.drain(..).collect();
                        all.splice(n..n, again);
                        queue.extend(all);
                    }
                    Fault::Mute { at, seconds, towards } if (at * FS) as usize == i => match towards {
                        Role::Answer => mute_to_answer = (seconds * FS) as usize,
                        Role::Call => mute_to_call = (seconds * FS) as usize,
                    },
                    _ => {}
                }
            }
            let at_call = line.to_call.pop_front().unwrap() + line.echo * from_call + line.noise();
            let at_answer = line.to_answer.pop_front().unwrap() + line.echo * from_answer + line.noise();
            from_call = caller.step(at_call);
            from_answer = answerer.step(at_answer);
            let mut to_answer = shape_to_answer.process(from_call * line.loss);
            let mut to_call = shape_to_call.process(from_answer * line.loss);
            if mute_to_answer > 0 {
                mute_to_answer -= 1;
                to_answer = 0.0;
            }
            if mute_to_call > 0 {
                mute_to_call -= 1;
                to_call = 0.0;
            }
            line.to_answer.push_back(to_answer);
            line.to_call.push_back(to_call);
            stand_in_for_phase_3(&mut caller, &answerer, &mut caller_done_at, i);
            stand_in_for_phase_3(&mut answerer, &caller, &mut answerer_done_at, i);
            if caller.status() != Status::Running && answerer.status() != Status::Running {
                break;
            }
        }
        (caller, answerer)
    }

    /// The two ends, by which sends the page: the call modem as source with
    /// the answer modem as recipient (12.2.1), or the other way about
    /// (12.2.2).
    fn pair(source: Role) -> (Modem, Modem) {
        let part = |role| if role == source { Part::Source } else { Part::Recipient };
        (Modem::new(Role::Call, part(Role::Call), FS), Modem::new(Role::Answer, part(Role::Answer), FS))
    }

    /// The source and the recipient of a pair, in that order.
    fn ends<'a>(caller: &'a Modem, answerer: &'a Modem) -> (&'a Modem, &'a Modem) {
        if caller.part() == Part::Source { (caller, answerer) } else { (answerer, caller) }
    }

    /// Both ends done, with the one INFOh between them.
    fn through(source: &Modem, recipient: &Modem) -> InfoH {
        assert_eq!(source.status(), Status::Done, "source ({:?}) stuck at {}", source.role(), source.phase());
        assert_eq!(
            recipient.status(),
            Status::Done,
            "recipient ({:?}) stuck at {}",
            recipient.role(),
            recipient.phase()
        );
        let infoh = recipient.infoh().expect("the recipient sent no INFOh");
        assert_eq!(source.infoh(), Some(infoh), "the source has a different INFOh");
        infoh
    }

    /// A short clean line.
    fn short() -> Line {
        Line::new(0.010, 10.0, f64::INFINITY, 70.0)
    }

    /// This rig's VoIP line: 750 ms each way, 20 dB down, with an echo of
    /// each end's own signal 15 dB down.
    fn voip() -> Line {
        Line::new(0.750, 20.0, 15.0, 60.0)
    }

    /// Both chains over a short clean line: nothing recovered from, and the
    /// INFOh a clean line earns.
    #[test]
    fn all_four_chains_get_through_on_a_short_line() {
        for source in [Role::Call, Role::Answer] {
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut short(), 12.0, caller, answerer, Shape::Flat, &[]);
            let (s, r) = ends(&c, &a);
            let infoh = through(s, r);
            for m in [s, r] {
                assert_eq!(m.recoveries(), 0, "{:?} {:?} took a way out", m.role(), m.part());
                assert_eq!(m.info0_repeats(), 0, "{:?} {:?} repeated INFO0", m.role(), m.part());
                assert!(m.far_capabilities().is_some_and(|far| !far.acknowledge));
            }
            assert!(!r.blind());
            assert!(r.reading().is_some() && s.reading().is_none());
            assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{infoh:?}");
            assert_eq!(infoh.trn_size, Size::Sixteen, "{infoh:?}");
            assert_eq!(infoh.pre_emphasis, 0, "{infoh:?}");
            assert_eq!(infoh.power_reduction, 0, "{infoh:?}");
            assert_eq!(infoh.trn_length, TRN_STEPS);
            assert!(infoh.trn_symbols() >= 512, "{infoh:?}");
            // Done inside a second and a half: INFO0, the tones, 570 ms of
            // probe, the tones again, INFOh.
            assert!(s.now < (1.5 * FS) as u64, "the source took {:.2} s", s.now as f64 / FS);
        }
    }

    #[test]
    fn the_voip_line_is_survived_both_ways() {
        for source in [Role::Call, Role::Answer] {
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut voip(), 20.0, caller, answerer, Shape::Flat, &[]);
            let (s, r) = ends(&c, &a);
            let infoh = through(s, r);
            assert_eq!(s.recoveries() + r.recoveries(), 0, "a way out was taken");
            assert_eq!(s.info0_repeats() + r.info0_repeats(), 0, "an INFO0 was repeated");
            assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{infoh:?}");
        }
    }

    /// Both chains over the VoIP line with noise on it: 30 dB of signal to
    /// noise across the band, and then 6, which the detectors of tones,
    /// reversals and sequences have to hear through with nothing repeated
    /// and no way out taken, and which the reading then answers to. The
    /// analyser's windows lift a tone some 10 dB clear of the broadband
    /// figure, so 30 dB still earns 3429 Bd and sixteen points, and 6 dB
    /// four points at whichever symbol rate projects 9600 or 12 000.
    #[test]
    fn a_noisy_voip_line_is_survived_both_ways() {
        for (noise_db, trn_size) in [(50.0, Size::Sixteen), (26.0, Size::Four)] {
            for source in [Role::Call, Role::Answer] {
                let (caller, answerer) = pair(source);
                let (c, a) = run(&mut Line::new(0.750, 20.0, 15.0, noise_db), 20.0, caller, answerer, Shape::Flat, &[]);
                let (s, r) = ends(&c, &a);
                let infoh = through(s, r);
                let case = format!("{source:?} as source with noise at -{noise_db} dB");
                assert_eq!(s.recoveries() + r.recoveries(), 0, "{case}: a way out was taken");
                assert_eq!(s.info0_repeats() + r.info0_repeats(), 0, "{case}: an INFO0 was repeated");
                assert_eq!(infoh.trn_size, trn_size, "{case}: {infoh:?}");
                if trn_size == Size::Sixteen {
                    assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{case}: {infoh:?}");
                }
            }
        }
    }

    /// The VoIP line with a jitter buffer's inserts every 700 ms, in both
    /// directions, placed three ways so that some land on the INFO0 and the
    /// INFOh, which are then lost and asked for again.
    #[test]
    fn a_line_that_slips_is_survived() {
        for source in [Role::Call, Role::Answer] {
            for first in [0.78, 0.45, 0.20] {
                let faults: Vec<Fault> = (0..30)
                    .flat_map(|k| {
                        let at = first + 0.7 * f64::from(k);
                        [Role::Call, Role::Answer].map(|towards| Fault::Insert { at, seconds: 0.020, towards })
                    })
                    .collect();
                let (caller, answerer) = pair(source);
                let (c, a) = run(&mut voip(), 20.0, caller, answerer, Shape::Flat, &faults);
                let (s, r) = ends(&c, &a);
                let infoh = through(s, r);
                assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "inserts from {first}: {infoh:?}");
            }
        }
    }

    /// The INFOh a line earns. The clean line duplex projects at 33 600 both
    /// ways gets 3429 Bd and sixteen-point TRN; with the top of its band
    /// falling away it gets pre-emphasis; cut off above 2300 Hz it gets a
    /// lower symbol rate; and too noisy for four bits a symbol it gets
    /// four-point TRN. A source whose INFO0 has no 3429 and no high carrier
    /// at 3200 is asked for neither.
    #[test]
    fn infoh_answers_to_the_line_and_to_the_source() {
        let settle = |caller: Modem, answerer: Modem, shape: Shape, noise_db: f64| {
            let (c, a) = run(&mut Line::new(0.020, 6.0, f64::INFINITY, noise_db), 12.0, caller, answerer, shape, &[]);
            let (s, r) = ends(&c, &a);
            through(s, r)
        };
        let clean = {
            let (c, a) = pair(Role::Call);
            settle(c, a, Shape::Flat, 55.0)
        };
        assert_eq!((clean.symbol_rate, clean.pre_emphasis, clean.trn_size), (SymbolRate::S3429, 0, Size::Sixteen), "{clean:?}");
        let tilted = {
            let (c, a) = pair(Role::Call);
            settle(c, a, Shape::Tilted, 55.0)
        };
        assert!(tilted.pre_emphasis >= 2, "{tilted:?}");
        let noisy = {
            let (c, a) = pair(Role::Call);
            settle(c, a, Shape::Flat, 35.0)
        };
        assert_eq!(noisy.symbol_rate, SymbolRate::S3429, "{noisy:?}");
        let narrow = {
            let (c, a) = pair(Role::Call);
            settle(c, a, Shape::Narrow, 35.0)
        };
        assert!(narrow.symbol_rate.index() < SymbolRate::S3429.index(), "{narrow:?}");
        let poor = {
            let (c, a) = pair(Role::Call);
            settle(c, a, Shape::Flat, 20.0)
        };
        assert_eq!(poor.trn_size, Size::Four, "{poor:?}");
        let lesser = {
            let ours = Info0 {
                rate_3429: false,
                transmit_3429: false,
                high_carrier_3200: false,
                ..phase2::Modem::capabilities()
            };
            let source = Modem::with_capabilities(Role::Answer, Part::Source, FS, ours);
            settle(Modem::new(Role::Call, Part::Recipient, FS), source, Shape::Flat, 55.0)
        };
        assert_eq!((lesser.symbol_rate, lesser.high_carrier), (SymbolRate::S3200, false), "{lesser:?}");
        // A source whose INFO0 bit 20 says it could send quieter is not asked
        // to: no power reduction is ever asked for, as none is in INFO1a.
        let willing = {
            let ours = Info0 { can_reduce_power: true, ..phase2::Modem::capabilities() };
            let source = Modem::with_capabilities(Role::Call, Part::Source, FS, ours);
            settle(source, Modem::new(Role::Answer, Part::Recipient, FS), Shape::Flat, 55.0)
        };
        assert_eq!((willing.power_reduction, willing.symbol_rate), (0, SymbolRate::S3429), "{willing:?}");
    }

    /// 12.7: a retrain runs the tone exchange again, from 70 ms of silence
    /// and the tone, with no INFO0, and settles the same INFOh again.
    #[test]
    fn a_retrain_runs_the_tone_exchange_without_info0() {
        for source in [Role::Call, Role::Answer] {
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut Line::new(0.030, 10.0, 20.0, 50.0), 12.0, caller, answerer, Shape::Flat, &[]);
            let first = {
                let (s, r) = ends(&c, &a);
                through(s, r)
            };
            // 12.7.1.1, 12.7.2.1: "transmit silence for 70 ± 5 ms", then the
            // tone, from either end.
            for mut m in [c.again(), a.again()] {
                let out: Vec<f64> = (0..(0.2 * FS) as usize).map(|_| m.step(0.0)).collect();
                let at = out.iter().position(|s| s.abs() > 1e-9).expect("no tone at all") as f64 / FS * 1000.0;
                assert!((65.0..=75.0).contains(&at), "{:?}: the tone began after {at} ms", m.role());
            }
            let (c2, a2) = run(&mut Line::new(0.030, 10.0, 20.0, 50.0), 12.0, c.again(), a.again(), Shape::Flat, &[]);
            let (s, r) = ends(&c2, &a2);
            assert_eq!(through(s, r), first);
            assert_eq!(s.info0_repeats() + r.info0_repeats(), 0, "an INFO0 went in a retrain");
            assert_eq!(s.recoveries() + r.recoveries(), 0, "a way out was taken");
            assert_eq!(s.far_capabilities(), c.far_capabilities().filter(|_| s.role() == Role::Call).or(a.far_capabilities()));
        }
    }

    /// 12.7 as it happens: one end begins a retrain while the other is still
    /// talking on the control channel. The other hears the tone for 50 ms
    /// (12.7.1.2, 12.7.2.2) -- through a [`Steady`] of its own, as whatever
    /// owns it will -- and only then falls silent for 70 ms and answers with
    /// its own tone. Until then it talks, stood in for by the DPSK modulator
    /// sending random bits: 600 baud on the same carrier, with the same
    /// turns of the phase in it as the control channel's symbols, none of
    /// which may pass for the tone or its reversal. Either part begins it,
    /// on either chain, and the exchange ends in the INFOh of the first time.
    #[test]
    fn a_retrain_begun_at_one_end_is_answered_by_the_other() {
        for source in [Role::Call, Role::Answer] {
            for beginner in [Part::Source, Part::Recipient] {
                let (c, a) = clean_run(source);
                let (s, r) = ends(&c, &a);
                let first = through(s, r);
                let (begins, answers) = if beginner == Part::Source { (s, r) } else { (r, s) };
                let case = format!("{source:?} as source, the {beginner:?} beginning");
                let mut line = Line::new(0.030, 10.0, 20.0, 50.0);
                let mut begun = begins.again();
                let mut answered: Option<Modem> = None;
                let mut chatter = dpsk::Transmitter::new(answers.role().side(), FS);
                let mut seed = 0x2545_f491u32;
                let mut watch = Steady::new(begins.role().side().carrier(), FS);
                // When the answering end heard 50 ms of the tone, when each
                // end's tone then began, on the shared clock.
                let (mut heard, mut begun_tone, mut answered_tone) = (None, None, None);
                let (mut from_call, mut from_answer) = (0.0, 0.0);
                for i in 0..(12.0 * FS) as usize {
                    let at_call = line.to_call.pop_front().unwrap() + line.echo * from_call + line.noise();
                    let at_answer = line.to_answer.pop_front().unwrap() + line.echo * from_answer + line.noise();
                    let begins_call = begins.role() == Role::Call;
                    let (at_begun, at_answering) = if begins_call { (at_call, at_answer) } else { (at_answer, at_call) };
                    let out_begun = begun.step(at_begun);
                    if out_begun.abs() > 1e-9 {
                        begun_tone.get_or_insert(i);
                    }
                    watch.feed(at_answering);
                    if answered.is_none() && watch.held() >= (0.050 * FS) as u64 {
                        heard = Some(i);
                        chatter.stop();
                        answered = Some(answers.again());
                    }
                    let out_answering = match answered.as_mut() {
                        Some(m) => {
                            let out = m.step(at_answering);
                            if out.abs() > 1e-9 {
                                answered_tone.get_or_insert(i);
                            }
                            out
                        }
                        None => {
                            if chatter.pending() < 64 {
                                let bits: Vec<bool> = (0..256)
                                    .map(|_| {
                                        seed ^= seed << 13;
                                        seed ^= seed >> 17;
                                        seed ^= seed << 5;
                                        seed & 1 == 1
                                    })
                                    .collect();
                                chatter.send(&bits);
                            }
                            chatter.next_sample()
                        }
                    };
                    (from_call, from_answer) = if begins_call { (out_begun, out_answering) } else { (out_answering, out_begun) };
                    line.to_answer.push_back(from_call * line.loss);
                    line.to_call.push_back(from_answer * line.loss);
                    if begun.status() != Status::Running && answered.as_ref().is_some_and(|m| m.status() != Status::Running) {
                        break;
                    }
                }
                let answered = answered.unwrap_or_else(|| panic!("{case}: the tone was never heard for 50 ms"));
                let (s, r) = if beginner == Part::Source { (&begun, &answered) } else { (&answered, &begun) };
                assert_eq!(through(s, r), first, "{case}");
                assert_eq!(s.recoveries() + r.recoveries(), 0, "{case}: a way out was taken");
                assert_eq!(s.info0_repeats() + r.info0_repeats(), 0, "{case}: an INFO0 went in a retrain");
                let ms = |n: usize| n as f64 / FS * 1000.0;
                let (heard, begun_tone, answered_tone) = (heard.unwrap(), begun_tone.unwrap(), answered_tone.unwrap());
                assert!((65.0..=75.0).contains(&ms(begun_tone)), "{case}: the beginner's tone came after {} ms", ms(begun_tone));
                // The tone crossed the line and was heard 50 ms, and not much
                // more: the answering end's own detector took what it took.
                let listened = ms(heard - begun_tone) - 30.0;
                assert!((50.0..=70.0).contains(&listened), "{case}: the tone was listened to for {listened:.1} ms before it was answered");
                let silent = ms(answered_tone - heard);
                assert!((65.0..=75.0).contains(&silent), "{case}: the answering end was silent for {silent:.1} ms");
            }
        }
    }

    /// 12.2.1.3.1, 12.2.1.4.1, 12.2.2.3.1, 12.2.2.4.1: an INFO0 lost in a
    /// hole, from either end of either chain. The end that missed it hears
    /// the other's tone with no INFO0 before it, and sends its own INFO0
    /// again; the other, getting a second INFO0 that does not acknowledge its
    /// own, sends its own again with bit 28 set; and the exchange goes on
    /// from there. On the short line one repeat is in flight at a time; on
    /// the VoIP line a round trip holds twenty of them, every one of which
    /// is answered before either end reverses.
    #[test]
    fn a_lost_info0_is_sent_again_and_acknowledged() {
        for (line, name) in [(short as fn() -> Line, "short"), (voip, "VoIP")] {
            for source in [Role::Call, Role::Answer] {
                for lost in [Role::Call, Role::Answer] {
                    let towards = if lost == Role::Call { Role::Answer } else { Role::Call };
                    let hole = Fault::Mute { at: 0.030, seconds: 0.030, towards };
                    let (caller, answerer) = pair(source);
                    let (c, a) = run(&mut line(), 20.0, caller, answerer, Shape::Flat, &[hole]);
                    let (s, r) = ends(&c, &a);
                    let infoh = through(s, r);
                    let case = format!("{name} line, {source:?} source, {lost:?} INFO0 lost");
                    assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{case}: {infoh:?}");
                    let (missed, sent) = if lost == Role::Call { (&a, &c) } else { (&c, &a) };
                    assert!(missed.info0_repeats() >= 1, "{case}: the end that missed it did not repeat its own");
                    assert!(sent.info0_repeats() >= 1, "{case}: the end whose INFO0 was lost did not send it again");
                    assert!(missed.far_capabilities().unwrap().acknowledge, "{case}: the INFO0 that got through did not acknowledge");
                    assert_eq!(s.recoveries() + r.recoveries(), 0, "{case}: a wait ran out");
                }
            }
        }
    }

    /// A clean run of a chain, for when things happened in it: the call
    /// modem and the answer modem, as `run` gives them back.
    fn clean_run(source: Role) -> (Modem, Modem) {
        let (caller, answerer) = pair(source);
        run(&mut short(), 12.0, caller, answerer, Shape::Flat, &[])
    }

    /// When, on the ends' shared clock, the source and the recipient of a
    /// chain reversed their tones over a clean line, in seconds.
    fn reversals_on_a_clean_line(source: Role) -> (f64, f64) {
        let (c, a) = clean_run(source);
        let (s, r) = ends(&c, &a);
        (s.reversed_at.unwrap() as f64 / FS, r.reversed_at.unwrap() as f64 / FS)
    }

    /// The direction from the source of a chain to its recipient.
    fn towards_recipient(source: Role) -> Role {
        if source == Role::Call { Role::Answer } else { Role::Call }
    }

    /// 12.2.1.4.2 and 12.2.1.3.3 (12.2.2.3.2 and 12.2.2.4.3 the other way):
    /// the source's reversal falls in a hole. The recipient, hearing no
    /// reversal within 2000 ms of its own, listens for the source's tone;
    /// the source, hearing no tone within 2700 ms of its reversal, stops L2
    /// and sends its tone; the recipient sends its own, reverses again, and
    /// the probe is sent and read again.
    #[test]
    fn a_source_reversal_lost_in_a_hole_is_probed_again() {
        for source in [Role::Call, Role::Answer] {
            let (source_reversal, _) = reversals_on_a_clean_line(source);
            let hole = Fault::Mute { at: source_reversal - 0.050, seconds: 0.200, towards: towards_recipient(source) };
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut short(), 12.0, caller, answerer, Shape::Flat, &[hole]);
            let (s, r) = ends(&c, &a);
            let infoh = through(s, r);
            assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{source:?} as source: {infoh:?}");
            assert_eq!(s.recoveries(), 1, "{source:?} as source: the source did not take 12.2.1.3.3's way out");
            assert_eq!(r.recoveries(), 1, "{source:?} as source: the recipient did not take 12.2.1.4.2's way out");
            assert!(!r.blind());
        }
    }

    /// 12.2.1.3.2 and 12.2.1.4.2 (12.2.2.4.2 and 12.2.2.3.2 the other way):
    /// the recipient's reversal falls in a hole. The source keeps its tone
    /// and waits for another, having no time limit of its own; the
    /// recipient, hearing no reversal back within 2000 ms, hears the tone
    /// that is still there, sends its own again and reverses again.
    #[test]
    fn a_recipient_reversal_lost_in_a_hole_is_made_again() {
        for source in [Role::Call, Role::Answer] {
            let (_, recipient_reversal) = reversals_on_a_clean_line(source);
            let hole = Fault::Mute { at: recipient_reversal - 0.050, seconds: 0.150, towards: source };
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut short(), 12.0, caller, answerer, Shape::Flat, &[hole]);
            let (s, r) = ends(&c, &a);
            through(s, r);
            assert_eq!(s.recoveries(), 0, "{source:?} as source: the source has no way out of 12.2.1.1.3 to take");
            assert_eq!(r.recoveries(), 1, "{source:?} as source: the recipient did not take 12.2.1.4.2's way out");
        }
    }

    /// 12.2.1.3.4 (12.2.2.4.4) and 12.3.3: INFOh falls in a hole. The source,
    /// with no INFOh within 2000 ms of its tone, keeps the tone and waits
    /// for the recipient's tone to go and come again; the recipient, with no
    /// S within 2000 ms of its INFOh, comes back with its tone, hears the
    /// source's, and sends INFOh again.
    #[test]
    fn an_infoh_lost_in_a_hole_is_sent_again() {
        for source in [Role::Call, Role::Answer] {
            let sent = {
                let (c, a) = clean_run(source);
                ends(&c, &a).1.infoh_sent_at.expect("no INFOh in a clean run") as f64 / FS
            };
            let hole = Fault::Mute { at: sent - 0.010, seconds: 0.120, towards: source };
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut short(), 12.0, caller, answerer, Shape::Flat, &[hole]);
            let (s, r) = ends(&c, &a);
            let infoh = through(s, r);
            assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{source:?} as source: {infoh:?}");
            assert_eq!(s.recoveries(), 1, "{source:?} as source: the source did not take 12.2.1.3.4's way out");
            assert!(!r.blind());
            // The second INFOh came from a recipient that came back to its
            // tone, two seconds after the first.
            assert!(r.now < (0.5 * FS) as u64, "{source:?} as source: the recipient is the one that started phase 2");
            assert!(s.now > (3.0 * FS) as u64, "{source:?} as source: the source was done at {:.2} s", s.now as f64 / FS);
        }
    }

    /// 12.2.1.4.3 (12.2.2.3.3): the source's tone after the probe never
    /// reaches the recipient, which sends INFOh anyway 2000 ms after starting
    /// its own tone; and the source, which heard the recipient's tone and
    /// has waited out its 2000 ms for INFOh, takes it when it comes.
    #[test]
    fn a_recipient_that_hears_no_tone_sends_infoh_blind() {
        for source in [Role::Call, Role::Answer] {
            let (source_reversal, _) = reversals_on_a_clean_line(source);
            // From after the recipient has read its L2 until past its wait.
            let hole = Fault::Mute { at: source_reversal + 0.620, seconds: 2.600, towards: towards_recipient(source) };
            let (caller, answerer) = pair(source);
            let (c, a) = run(&mut short(), 12.0, caller, answerer, Shape::Flat, &[hole]);
            let (s, r) = ends(&c, &a);
            let infoh = through(s, r);
            assert_eq!(infoh.symbol_rate, SymbolRate::S3429, "{source:?} as source: {infoh:?}");
            assert!(r.blind(), "{source:?} as source: the recipient waited for a tone it could not hear");
            assert_eq!(r.recoveries(), 1);
        }
    }

    /// The timings of Figures 23 and 24, on a short line, for either chain:
    /// the source's reversal 40 ± 10 ms after the recipient's reaches it; the
    /// recipient's tone on the line 50 ms before its reversal; L1 10 ms after
    /// the source's reversal; the recipient's tone within 670 ms of the
    /// source's reversal reaching it; and INFOh 25 ms after the source's tone
    /// is heard.
    #[test]
    fn the_timings_of_figures_23_and_24_are_kept() {
        for source in [Role::Call, Role::Answer] {
            let one_way = 0.010;
            let mut line = Line::new(one_way, 10.0, f64::INFINITY, 70.0);
            let (mut caller, mut answerer) = pair(source);
            // When the recipient's tone was first on the line, when it began
            // its tone after the probe, when the source began its tone after
            // the probe, and when L1 began.
            let (mut recipient_tone, mut recipient_tone_again, mut source_tone, mut l1) = (None, None, None, None);
            for _ in 0..(12.0 * FS) as usize {
                let at_call = line.to_call.pop_front().unwrap() + line.noise();
                let at_answer = line.to_answer.pop_front().unwrap() + line.noise();
                let from_call = caller.step(at_call);
                let from_answer = answerer.step(at_answer);
                line.to_answer.push_back(from_call * line.loss);
                line.to_call.push_back(from_answer * line.loss);
                let (s, r) = ends(&caller, &answerer);
                if let (Stage::RecipientAwaitTone, Some(t)) = (r.stage, r.tone_since) {
                    recipient_tone.get_or_insert(t);
                }
                if r.stage == Stage::RecipientAwaitTone2 {
                    recipient_tone_again.get_or_insert(r.now);
                }
                if s.stage == Stage::SourceAwaitInfoH {
                    source_tone.get_or_insert(s.now);
                }
                if let (Stage::SourceProbing, Speaking::Probe { l1_until }) = (s.stage, s.speaking) {
                    l1.get_or_insert(l1_until - s.ms(probe::L1_SECONDS));
                }
                if caller.status() != Status::Running && answerer.status() != Status::Running {
                    break;
                }
            }
            let (s, r) = ends(&caller, &answerer);
            through(s, r);
            let ms = |n: u64| n as f64 / FS * 1000.0;
            let delay = (one_way * FS) as u64;
            let (rr, sr) = (r.reversed_at.unwrap(), s.reversed_at.unwrap());
            let turn = ms(sr - (rr + delay));
            assert!((30.0..=50.0).contains(&turn), "{source:?} as source: the source reversed {turn:.1} ms after the recipient's reversal arrived");
            let own = ms(rr - recipient_tone.unwrap()) - PULSE_SPAN * 1000.0;
            assert!(own >= 50.0, "{source:?} as source: {own:.1} ms of the recipient's tone before its reversal");
            assert_eq!(l1.unwrap() - sr, s.ms(AFTER_REVERSAL), "{source:?} as source: L1 did not follow the reversal by 10 ms");
            let answered = ms(recipient_tone_again.unwrap() - (sr + delay));
            assert!(answered <= 670.0, "{source:?} as source: the recipient's tone came {answered:.1} ms after the source's reversal arrived");
            let before_infoh = ms(r.infoh_sent_at.unwrap() - (source_tone.unwrap() + delay));
            assert!((25.0..=95.0).contains(&before_infoh), "{source:?} as source: INFOh came {before_infoh:.1} ms after the source's tone arrived");
        }
    }

    #[test]
    fn a_far_end_that_never_answers_is_given_up_on() {
        for part in [Part::Source, Part::Recipient] {
            let mut m = Modem::new(Role::Call, part, FS);
            for _ in 0..(21.0 * FS) as usize {
                m.step(0.0);
            }
            assert!(matches!(m.status(), Status::Failed(_)), "{part:?}: {:?}", m.status());
        }
    }
}
