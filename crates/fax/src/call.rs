//! A fax call, from either end.
//!
//! T.30 divides a call into five phases: A is getting the two machines to
//! agree they are faxes, B is finding out what they can do and settling on
//! it, C is the page, D is what to do about it, E is hanging up. This is all
//! five, from the end that dialled and from the end that answered.
//!
//! Nothing here knows what a signal is. It says what the line should be
//! doing -- a tone, the 300 bit/s control channel, or the high-speed carrier
//! at an agreed rate -- and takes back whatever bits arrived. The reason for
//! the split is that a fax call changes modulation eight or ten times before
//! a page has moved, and the rule for when it changes is procedure rather
//! than signal processing.
//!
//! The awkward part of that procedure, and the part every implementation gets
//! wrong first, is that the two ends take turns. There is no moment when both
//! are talking, and every turnaround has a settling time either side of it
//! that is longer than it looks like it should be.

use crate::ecm;
use crate::frames::{Message, Reader, Sender};
use crate::page::{Page, Resolution};
use crate::t30::{self, Capabilities, Command, Frame, Modulation};
use crate::t4;

/// Which end of the call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The end that dialled, which sends the page.
    Caller,
    /// The end that answered, which receives it.
    Answerer,
}

/// What carries a page: a modulation, and one of its rates.
///
/// Both, because neither is enough alone. 9600 is V.29 or V.17 and 7200 is
/// either of those too, and they are not remotely the same signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Speed {
    pub modulation: Modulation,
    pub bits_per_second: u32,
}

/// What the procedure wants on the line at this instant.
///
/// A modulation and a rate rather than a data pump, because which pump
/// carries them is not this crate's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// Nothing at all. Every turnaround has one of these in it.
    Quiet,
    /// The calling tone, in its own on-and-off rhythm (5.1.1).
    CallingTone,
    /// The called tone, steadily (5.1.2).
    CalledTone,
    /// Frames going out on V.21 channel 2.
    Control,
    /// Listening on V.21 channel 2.
    Listen,
    /// A high-speed burst going out.
    Fast(Speed),
    /// Listening for a high-speed burst.
    FastListen(Speed),
}

/// How far the call has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    // Phase A and B, from the end that dialled.
    Calling,
    Listening,
    Commanding,
    Training,
    AwaitingConfirm,
    // Phase C and D, the same end.
    Sending,
    EndingPage,
    AwaitingReceipt,
    // Phase A and B, from the end that answered.
    Answering,
    Identifying,
    AwaitingCommand,
    CheckingTraining,
    Confirming,
    // Phase C and D, the same end.
    Receiving,
    AwaitingPostMessage,
    Acknowledging,
    AwaitingDisconnect,
    // Phase E, and the two ends it can come to.
    Ending,
    Done,
    Failed,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Calling => "calling",
            Self::Listening => "listening",
            Self::Commanding => "sending the command",
            Self::Training => "sending the training check",
            Self::AwaitingConfirm => "waiting to be let go",
            Self::Sending => "sending the page",
            Self::EndingPage => "end of page",
            Self::AwaitingReceipt => "waiting for the receipt",
            Self::Answering => "answering",
            Self::Identifying => "saying what we are",
            Self::AwaitingCommand => "waiting for the command",
            Self::CheckingTraining => "checking the training",
            Self::Confirming => "confirming",
            Self::Receiving => "receiving the page",
            Self::AwaitingPostMessage => "waiting for the end of the page",
            Self::Acknowledging => "acknowledging",
            Self::AwaitingDisconnect => "waiting for the far end to hang up",
            Self::Ending => "hanging up",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// Whether there is nothing left for the call to do.
    pub fn is_over(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// T1: how long to keep trying in phase B before giving up (5.3.3.1).
///
/// "35 s +/- 5 s", and it is generous on purpose: a machine at the other end
/// may be picking up paper, and the whole of phase B can pass before it says
/// anything at all.
pub const T1_SECONDS: f64 = 35.0;

/// T2: how long to wait for a command once the two ends are talking (5.4.2.2).
pub const T2_SECONDS: f64 = 6.0;

/// T4: the gap between one attempt at a command and the next (5.4.2.4).
pub const T4_SECONDS: f64 = 3.0;

/// The settling time either side of a change of modulation.
///
/// NOTE 3 and NOTE 4 under 5.1: "should be followed by a delay of 75 +/- 20
/// ms before the signalling, utilizing a different modulation system,
/// commences". Both directions, and both matter -- a machine that starts its
/// training the instant its own closing flag has gone trains the far end
/// while the far end is still shutting down its own receiver.
pub const TURNAROUND_SECONDS: f64 = 0.075;

/// The called tone, 2100 Hz, held for this long (5.1.2).
pub const CED_SECONDS: f64 = 3.0;

/// TCF: "A series of 0 for 1.5 s +/- 10%" (6.2.6).
pub const TCF_SECONDS: f64 = 1.5;

/// How long the high-speed carrier has to be there before it counts as a
/// burst having started.
///
/// The shortest burst any of this sends is the short turn-on sequence, fifty
/// milliseconds at 4800, and the one it actually sends is fourteen times
/// that. What is being ruled out is shorter still: the level meter ringing as
/// the last V.21 burst dies away puts ten milliseconds of carrier on the line
/// at exactly the moment this end starts listening for one, and a burst that
/// arrived and ended is what the procedure reads that as.
const FAST_CARRIER_SETTLED: f64 = 0.040;

/// How long the high-speed carrier has to have been gone before the burst is
/// over.
///
/// Not the instant it goes, because a real transmitter takes it away on
/// purpose in the middle of a burst. V.27 ter's protection against talker echo
/// is a fifth of a second of plain carrier and then "20 ms to 25 ms" of
/// nothing before the training starts, and a public fax service sends exactly
/// that. Read as the end of the burst, it had this end judge a training check
/// made of nothing but the plain carrier -- and refuse it, every time, while
/// the check itself was still arriving and would have read as 7195 zeros out
/// of 7200.
///
/// A fifth of a second bridges that gap many times over, and the short
/// dropouts a packet network leaves in the middle of a page with it. Against
/// the three seconds T.30 gives a receiver to answer, it is nothing.
const FAST_CARRIER_GONE: f64 = 0.200;

/// How long a timer that has run out may be held open while the far end is
/// audibly talking on the control channel.
///
/// A timeout in a waiting phase means sending something: a DIS again, a DCS
/// again, a failure to train. Sending it while the far end's flags are on the
/// line talks over the very answer being waited for, and a half-duplex modem
/// cannot hear while it talks, so the answer is lost for certain. A recorded
/// call did exactly that -- repeated its DIS over the far end's TSI and DCS,
/// and never heard either.
///
/// Held, but not for ever: a carrier detector stuck on by a noisy line would
/// otherwise wait out the whole call. Six seconds is T2, and longer than the
/// longest burst of frames anything sends in phase B.
const HOLD_FOR_THE_FAR_END: f64 = 6.0;

/// How long a page carrier may stay up with nothing readable in it before the
/// receiving end stops waiting for it to end.
///
/// T5 of A.5.4.1, sixty seconds: the longest T.30 has anybody wait on a far end
/// that is still there. A burst this end cannot read is still a burst, and the
/// command that follows it cannot be heard until it is over -- giving up on it
/// after T2's six seconds went back to listening for a partial page signal
/// while the far end still had most of a retransmission to send, and then gave
/// up on that too.
const T5_SECONDS: f64 = 60.0;

/// How many of a training check's bits have to be zeros for the rate to be
/// accepted, and how many in a row mark where it starts.
///
/// T.30 does not give a number. It says only that the check exists "to verify
/// training and to give a first indication of the acceptability of the
/// channel", and leaves the judgement to the receiver.
///
/// A fraction rather than one unbroken run, because one unbroken run makes a
/// single bit error in a second and a half of line reject a rate that would
/// have carried the page perfectly well. What is in front of the zeros is the
/// tail of a training sequence through a descrambler, which is never zeros
/// and has to be skipped: thirty-two zeros in a row is where the check
/// starts, and nothing but the check produces thirty-two.
const TCF_ZEROS_WANTED: f64 = 0.95;
const TCF_STARTS_AFTER: usize = 32;

/// The modulations this modem can carry a page with.
///
/// V.17 is not among them yet. What goes in a DIS is this same fact in the
/// form Table 2 wants it.
pub const OUR_MODULATIONS: [Modulation; 2] = [Modulation::V27ter, Modulation::V29];

/// Which post-message command goes out next under error correction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EcmCommand {
    /// A partial page has ended (A.4.3 1).
    Pps,
    /// Frames asked for four times are given up on (A.4.3 2).
    Eor,
    /// The same frames again, at a slower rate (A.4.1 1).
    Ctc,
    /// Is the receiver ready yet (A.4.3 3).
    Rr,
}

/// A fax call.
#[derive(Debug)]
pub struct Call {
    role: Role,
    phase: Phase,
    reader: Reader,
    sender: Sender,
    /// Seconds since the call began, which is what T1 counts.
    elapsed: f64,
    step: f64,
    /// Seconds left before whatever is being waited for is given up on.
    timer: f64,
    /// A settling gap to sit out before the next thing happens.
    pause: f64,
    /// What to do once the pause is over.
    after_pause: Option<Phase>,

    /// What the far end said, as it says it.
    pub identity: String,
    pub capabilities: Option<Capabilities>,
    /// The capability field exactly as it arrived, so anything wanting to
    /// read it differently still can.
    pub capability_field: Option<Vec<u8>>,
    /// Every frame either end sent, for the log.
    heard: Vec<Message>,
    /// This end's own identification, sent as a TSI or a CSI.
    identification: String,

    /// The rate the page is being carried at, once it is settled.
    modulation: Modulation,
    rate: u32,
    /// Speeds still worth trying, fastest first, should the far end refuse
    /// this one.
    fallback: Vec<Speed>,
    /// The modulations this end is willing to use, which is what goes in its
    /// DIS and what it will choose from when it sends.
    offer: Vec<Modulation>,
    /// The minimum scan line time the receiving end asked for, as the three
    /// bits of Table 2 and as milliseconds.
    scan_line_field: u8,
    scan_line_ms: f64,
    resolution: Resolution,
    /// How the page is coded: chosen by the end that sends it, from what the
    /// end that receives it said it could read.
    scheme: t4::Scheme,
    /// Whether this end offers error correction mode, and whether this call is
    /// using it.
    error_correction_offered: bool,
    ecm: bool,
    /// Error correction mode, sending: the page cut into frames, the block of
    /// them in hand, the frames of it asked for again and how many times, and
    /// the page's number in the call.
    ecm_frames: Vec<Vec<u8>>,
    ecm_block: usize,
    ecm_resend: Vec<usize>,
    ecm_pprs: u32,
    ecm_page: u8,
    ecm_command: EcmCommand,
    /// Error correction mode, receiving: the frames arriving, the page so far,
    /// how many frames the block in hand has, the last block confirmed, what
    /// to answer with, and where to go once the answer has gone.
    collector: ecm::Collector,
    ecm_octets: Vec<u8>,
    /// How much of the page, counted in octets from its start, has gone into
    /// the decoder: confirmed blocks and then the unbroken run of the block in
    /// hand.
    ecm_fed: usize,
    ecm_expected: usize,
    ecm_confirmed: Option<(u8, u8)>,
    ecm_response: Option<Message>,
    after_answering: Phase,

    /// The page this end is sending, if it has one.
    page: Option<Page>,
    /// That page coded, and how far through it the line has got.
    fast_out: Vec<bool>,
    fast_at: usize,
    /// Bits arriving on the high-speed carrier, while they are still being
    /// judged rather than decoded.
    fast_in: Vec<bool>,
    /// Whether the far end's high-speed carrier is up, and whether it has
    /// been up at all since this end started listening for it. A burst ends
    /// when the carrier goes away, and the carrier being away before it ever
    /// arrived is not the same thing.
    fast_carrier: bool,
    fast_seen: bool,
    /// How long it has been up for, and gone for, without a break.
    fast_up: f64,
    fast_down: f64,
    /// Whether the far end's control channel is on the line, and how long a
    /// timeout has been held open because of it.
    control_carrier: bool,
    held: f64,
    /// The page arriving, if one is.
    decoder: t4::Decoder,
    /// The page that arrived.
    pub received: Option<Page>,
    /// Whether the last thing judged was good, which decides what follows the
    /// frame now going out.
    accepted: bool,
    /// Attempts made at the command in hand. 5.4.2 allows three of anything
    /// before the call is a lost cause, and a first attempt that goes
    /// unanswered is the ordinary way a fax call starts on a bad line.
    attempts: u8,
    /// Why the call ended, when it ended badly.
    pub trouble: Option<String>,
}

impl Call {
    /// The end that dialled. `page` is what it is calling to send, if
    /// anything: a call with no page still identifies itself, learns what the
    /// far end is, and hangs up politely.
    pub fn originate(fs: f64, identification: &str, page: Option<Page>) -> Self {
        Self::new(Role::Caller, fs, identification, page)
    }

    /// The end that answered.
    pub fn answer(fs: f64, identification: &str) -> Self {
        Self::new(Role::Answerer, fs, identification, None)
    }

    fn new(role: Role, fs: f64, identification: &str, page: Option<Page>) -> Self {
        Self {
            role,
            phase: match role {
                Role::Caller => Phase::Calling,
                Role::Answerer => Phase::Answering,
            },
            reader: Reader::new(),
            sender: Sender::new(),
            elapsed: 0.0,
            step: 1.0 / fs,
            timer: T1_SECONDS,
            pause: 0.0,
            after_pause: None,
            identity: String::new(),
            capabilities: None,
            capability_field: None,
            heard: Vec::new(),
            identification: identification.to_owned(),
            modulation: Modulation::V27ter,
            rate: 4800,
            fallback: Vec::new(),
            offer: OUR_MODULATIONS.to_vec(),
            scan_line_field: 0b111,
            scan_line_ms: 0.0,
            resolution: page.as_ref().map_or(Resolution::Standard, |p| p.resolution),
            scheme: t4::Scheme::OneDimensional,
            error_correction_offered: true,
            ecm: false,
            ecm_frames: Vec::new(),
            ecm_block: 0,
            ecm_resend: Vec::new(),
            ecm_pprs: 0,
            ecm_page: 0,
            ecm_command: EcmCommand::Pps,
            collector: ecm::Collector::new(),
            ecm_octets: Vec::new(),
            ecm_fed: 0,
            ecm_expected: 0,
            ecm_confirmed: None,
            ecm_response: None,
            after_answering: Phase::AwaitingDisconnect,
            page,
            fast_out: Vec::new(),
            fast_at: 0,
            fast_in: Vec::new(),
            fast_carrier: false,
            fast_seen: false,
            fast_up: 0.0,
            fast_down: 0.0,
            control_carrier: false,
            held: 0.0,
            decoder: t4::Decoder::new(),
            received: None,
            accepted: false,
            attempts: 0,
            trouble: None,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn seconds(&self) -> f64 {
        self.elapsed
    }

    /// The rate the page is being carried at.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn modulation(&self) -> Modulation {
        self.modulation
    }

    /// The modulation and rate the page is being carried at.
    pub fn speed(&self) -> Speed {
        Speed {
            modulation: self.modulation,
            bits_per_second: self.rate,
        }
    }

    /// Offer error correction mode, or not.
    ///
    /// Offered by default. A far end that has it and is told this end does
    /// too will use it, and a page that goes in frames is a page that arrives
    /// whole or says exactly which parts did not.
    pub fn set_error_correction(&mut self, on: bool) {
        self.error_correction_offered = on;
    }

    /// Whether this call is in error correction mode.
    pub fn error_correction(&self) -> bool {
        self.ecm
    }

    /// Use only these modulations, of the ones this end has.
    ///
    /// Anything not built is dropped, and an empty offer is V.27 ter: T.30
    /// makes it the one every machine must have, so a call that offered
    /// nothing at all would be a call nobody could answer.
    pub fn set_offer(&mut self, offer: &[Modulation]) {
        let mut kept: Vec<Modulation> = offer
            .iter()
            .copied()
            .filter(|m| OUR_MODULATIONS.contains(m))
            .collect();
        if kept.is_empty() {
            kept.push(Modulation::V27ter);
        }
        self.offer = kept;
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    /// Frames seen since this was last asked, for a log.
    pub fn take_heard(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.heard)
    }

    /// How far through the page the call is, as a fraction.
    ///
    /// Bits for the end that is sending, because that is what it knows;
    /// lines for the end that is receiving, because it does not know how many
    /// are coming until they stop.
    pub fn progress(&self) -> Option<f64> {
        match self.role {
            Role::Caller => {
                if self.fast_out.is_empty() {
                    return None;
                }
                let burst = self.fast_at as f64 / self.fast_out.len() as f64;
                if self.ecm && !self.ecm_frames.is_empty() {
                    // Blocks done, and how far through the one on the line.
                    return Some((self.ecm_block as f64 + burst) / self.ecm_blocks() as f64);
                }
                Some(burst)
            }
            Role::Answerer => {
                let lines = self.decoder.lines().len();
                if lines == 0 {
                    return None;
                }
                Some((lines as f64 / self.resolution.lines() as f64).min(1.0))
            }
        }
    }

    /// Lines of the page that have arrived.
    pub fn lines_received(&self) -> usize {
        self.decoder.lines().len()
    }

    /// The lines themselves, as far as the page has been decoded: the whole
    /// page once it is over, and the top of it while it is arriving.
    pub fn lines(&self) -> &[Vec<bool>] {
        self.decoder.lines()
    }

    /// What the line should be doing at this instant.
    pub fn line(&self) -> Line {
        if self.pause > 0.0 {
            return Line::Quiet;
        }
        match self.phase {
            Phase::Calling => Line::CallingTone,
            Phase::Answering => Line::CalledTone,
            Phase::Listening
            | Phase::AwaitingConfirm
            | Phase::AwaitingReceipt
            | Phase::AwaitingCommand
            | Phase::AwaitingPostMessage
            | Phase::AwaitingDisconnect => Line::Listen,
            Phase::Commanding
            | Phase::Identifying
            | Phase::Confirming
            | Phase::Acknowledging
            | Phase::EndingPage
            | Phase::Ending => Line::Control,
            Phase::Training | Phase::Sending => Line::Fast(self.speed()),
            Phase::CheckingTraining | Phase::Receiving => Line::FastListen(self.speed()),
            Phase::Done | Phase::Failed => Line::Quiet,
        }
    }

    /// Whether the calling tone should be sounding just now.
    ///
    /// 5.1.1 has it on for half a second in every three and a half, and only
    /// until something answers. It is a courtesy rather than a requirement:
    /// it tells a person who picked up that a fax is waiting, and it tells an
    /// answering machine which of the two it is talking to.
    pub fn calling_tone_on(&self) -> bool {
        if self.line() != Line::CallingTone {
            return false;
        }
        let period = crate::CNG_ON + crate::CNG_OFF;
        self.elapsed % period < crate::CNG_ON
    }

    /// The next bit for the control channel, if any.
    pub fn next_control_bit(&mut self) -> Option<bool> {
        self.sender.next_bit()
    }

    /// The next bit for the high-speed carrier, if any.
    pub fn next_fast_bit(&mut self) -> Option<bool> {
        let bit = *self.fast_out.get(self.fast_at)?;
        self.fast_at += 1;
        Some(bit)
    }

    /// A bit recovered from the control channel.
    pub fn control_bit(&mut self, bit: bool) {
        if let Some(message) = self.reader.feed(bit) {
            self.received(message);
        }
    }

    /// Bits recovered from the high-speed carrier.
    ///
    /// Anything arriving puts the clock back to the start. A page takes a
    /// minute at 4800 and four at 2400, and the six seconds T2 allows for a
    /// command is nothing like long enough to wait for one: what the timer
    /// has to mean here is how long the line may go quiet, not how long the
    /// page may take.
    pub fn fast_bits(&mut self, bits: &[bool]) {
        match self.phase {
            Phase::CheckingTraining => {
                self.fast_in.extend_from_slice(bits);
                self.timer = T2_SECONDS;
            }
            Phase::Receiving if self.ecm => {
                // A frame rather than a bit, for the same reason as a line.
                let before = self.collector.count();
                self.collector.feed_bits(bits);
                if self.collector.count() != before {
                    self.timer = T2_SECONDS;
                    self.follow_ecm_page();
                }
            }
            Phase::Receiving => {
                // A line rather than a bit, because noise is bits too. A
                // carrier detector that has latched onto the hiss on a line
                // hands up a bit every symbol for ever, and a clock put back
                // by every one of them is a clock that never runs out.
                let before = self.decoder.lines().len();
                self.decoder.feed_bits(bits);
                if self.decoder.lines().len() != before {
                    self.timer = T2_SECONDS;
                }
            }
            _ => {}
        }
    }

    /// Whether the far end's control channel is on the line.
    pub fn set_control_carrier(&mut self, up: bool) {
        self.control_carrier = up;
    }

    /// Whether the far end's high-speed carrier is on the line.
    pub fn set_fast_carrier(&mut self, up: bool) {
        self.fast_carrier = up;
        if !up {
            self.fast_up = 0.0;
        }
    }

    /// One sample of time passing.
    ///
    /// `idle` says whether everything handed over has actually left the line.
    /// It is not the same question as whether this has any bits left: a
    /// transmitter is fed ahead of the line, so the last frame of a burst is
    /// still being modulated long after its last bit was handed over. Reading
    /// the two as one hung the call up before its own disconnect had reached
    /// the far end, which is exactly the rudeness a disconnect exists to
    /// avoid.
    pub fn tick(&mut self, idle: bool) {
        self.elapsed += self.step;
        if self.phase.is_over() {
            return;
        }
        if self.pause > 0.0 {
            self.pause -= self.step;
            if self.pause <= 0.0
                && let Some(next) = self.after_pause.take()
            {
                self.enter(next);
            }
            return;
        }
        self.timer -= self.step;
        match self.phase {
            Phase::Calling | Phase::Listening => {
                if self.elapsed > T1_SECONDS {
                    self.give_up("nothing that sounded like a fax answered");
                }
            }
            Phase::Answering => {
                if self.elapsed > CED_SECONDS {
                    self.pause_then(Phase::Identifying);
                }
            }
            Phase::Commanding
            | Phase::Identifying
            | Phase::Confirming
            | Phase::Acknowledging
            | Phase::EndingPage => {
                if self.sender.is_empty() && idle {
                    self.control_burst_ended();
                }
            }
            Phase::Ending => {
                if self.sender.is_empty() && idle {
                    self.phase = Phase::Done;
                }
            }
            Phase::Training | Phase::Sending => {
                if self.fast_at >= self.fast_out.len() && idle {
                    self.fast_burst_ended();
                }
            }
            Phase::CheckingTraining | Phase::Receiving => {
                if self.fast_carrier {
                    self.fast_up += self.step;
                    self.fast_down = 0.0;
                    if self.fast_up > FAST_CARRIER_SETTLED {
                        self.fast_seen = true;
                    }
                } else {
                    self.fast_down += self.step;
                }
                // A high-speed burst has no closing flag. What ends it is the
                // carrier going away, and for a page the return to control
                // T.4 puts at the end of it as well.
                let finished = if self.ecm {
                    self.collector.ended()
                } else {
                    self.decoder.is_done()
                };
                if self.phase == Phase::Receiving && finished {
                    self.page_ended();
                } else if self.fast_seen && self.fast_down > FAST_CARRIER_GONE {
                    self.fast_burst_heard();
                } else if self.phase == Phase::CheckingTraining && self.check_is_long_enough()
                {
                    // A training check is a second and a half and no longer.
                    // Twice that much has arrived, so whatever is on the line
                    // is not going to stop being on it, and there is already
                    // more than enough to judge.
                    self.fast_burst_heard();
                } else if self.timer <= 0.0 {
                    if self.phase == Phase::Receiving
                        && self.fast_carrier
                        && self.held < T5_SECONDS
                    {
                        self.held += self.step;
                    } else {
                        self.held = 0.0;
                        self.timed_out();
                    }
                }
            }
            Phase::AwaitingConfirm
            | Phase::AwaitingReceipt
            | Phase::AwaitingCommand
            | Phase::AwaitingPostMessage
            | Phase::AwaitingDisconnect => {
                if self.timer <= 0.0 {
                    if self.control_carrier && self.held < HOLD_FOR_THE_FAR_END {
                        self.held += self.step;
                    } else {
                        self.held = 0.0;
                        self.timed_out();
                    }
                } else {
                    self.held = 0.0;
                }
            }
            Phase::Done | Phase::Failed => {}
        }
    }

    // ---- entering a phase -------------------------------------------------

    /// Sit out the settling time, then take up `next`.
    fn pause_then(&mut self, next: Phase) {
        self.pause = TURNAROUND_SECONDS;
        self.after_pause = Some(next);
        self.phase = next;
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.timer = match phase {
            Phase::AwaitingConfirm | Phase::AwaitingReceipt => T4_SECONDS,
            Phase::AwaitingCommand | Phase::AwaitingPostMessage => T2_SECONDS,
            Phase::AwaitingDisconnect => T4_SECONDS,
            Phase::CheckingTraining | Phase::Receiving => T2_SECONDS,
            _ => T1_SECONDS,
        };
        match phase {
            Phase::Commanding => self.send_command(),
            Phase::Identifying => self.send_identity(),
            Phase::Training => self.send_training_check(),
            Phase::Sending => self.send_page(),
            Phase::EndingPage => self.send_end_of_page(),
            Phase::Confirming => self.send_confirmation(),
            Phase::Acknowledging => self.send_acknowledgement(),
            Phase::Ending => self.send_disconnect(),
            Phase::CheckingTraining => {
                self.fast_in.clear();
                self.fast_seen = false;
                self.fast_up = 0.0;
                self.fast_down = 0.0;
            }
            Phase::Receiving => {
                if self.ecm {
                    // A retransmission fills in the same block, so what has
                    // arrived already stays.
                    self.collector.next_partial_page();
                    if self.ecm_octets.is_empty() && self.collector.count() == 0 {
                        // Nothing of this page is here yet, so it is a new
                        // one and the last page's lines can go.
                        self.decoder.reset_to(self.scheme);
                        self.ecm_fed = 0;
                    }
                } else {
                    self.decoder.reset_to(self.scheme);
                }
                self.fast_seen = false;
                self.fast_up = 0.0;
                self.fast_down = 0.0;
            }
            _ => {}
        }
    }

    fn give_up(&mut self, why: &str) {
        if self.trouble.is_none() {
            self.trouble = Some(why.to_owned());
        }
        self.phase = Phase::Failed;
    }

    /// Give up, but tell the far end first.
    ///
    /// 5.3.7 has a disconnect for exactly this. A machine that simply stops
    /// answering leaves the far end holding the line for its whole T1 and
    /// then reporting a failure to whoever is standing at it, which is a
    /// worse outcome than the one being reported.
    fn bow_out(&mut self, why: &str) {
        if self.trouble.is_none() {
            self.trouble = Some(why.to_owned());
        }
        self.pause_then(Phase::Ending);
    }

    // ---- what the far end said --------------------------------------------

    fn received(&mut self, message: Message) {
        // Everything that arrives on the control channel resets the clock:
        // the far end is there and is talking, which is what the timers are
        // really asking about.
        self.timer = T2_SECONDS;
        match message.frame {
            Frame::Csi | Frame::Tsi => {
                self.identity = t30::identification(&message.fif);
                if self.phase == Phase::Calling {
                    self.phase = Phase::Listening;
                }
            }
            Frame::Nsf => {
                if self.phase == Phase::Calling {
                    self.phase = Phase::Listening;
                }
            }
            Frame::Dis => {
                let caps = t30::capabilities(&message.fif);
                self.scan_line_ms = caps.scan_line_ms;
                self.scan_line_field = t30::field_of(&message.fif, 21, 23);
                self.capabilities = Some(caps);
                self.capability_field = Some(message.fif.clone());
                // Only if there is a rate to command. A far end offering
                // nothing this end can raise has already been told so, and
                // sending it a DCS naming a modulation neither of us agreed
                // on would be worse than saying nothing.
                if self.role == Role::Caller && self.choose_rate() {
                    self.pause_then(Phase::Commanding);
                }
            }
            Frame::Dcs => {
                if self.role == Role::Answerer {
                    self.capability_field = Some(message.fif.clone());
                    if let Some((modulation, rate)) = t30::command_rate(&message.fif) {
                        self.modulation = modulation;
                        self.rate = rate;
                    }
                    self.resolution = if t30::bit(&message.fif, 15) {
                        Resolution::Fine
                    } else {
                        Resolution::Standard
                    };
                    self.scheme = if t30::bit(&message.fif, 16) {
                        t4::Scheme::TwoDimensional
                    } else {
                        t4::Scheme::OneDimensional
                    };
                    self.ecm = t30::bit(&message.fif, 27);
                    self.collector = ecm::Collector::new();
                    self.ecm_octets.clear();
                    self.ecm_confirmed = None;
                    self.pause_then(Phase::CheckingTraining);
                }
            }
            Frame::Cfr => {
                if self.phase == Phase::AwaitingConfirm {
                    if self.page.is_none() {
                        // Let go to send a page there is not one of. Say so
                        // rather than holding the line.
                        self.pause_then(Phase::Ending);
                    } else {
                        self.pause_then(Phase::Sending);
                    }
                }
            }
            Frame::Ftt => {
                if self.phase == Phase::AwaitingConfirm {
                    self.step_down();
                }
            }
            Frame::Mcf | Frame::Rtp => {
                if self.phase == Phase::AwaitingReceipt {
                    if self.ecm {
                        self.next_block();
                    } else {
                        self.pause_then(Phase::Ending);
                    }
                }
            }
            Frame::Err => {
                if self.phase == Phase::AwaitingReceipt && self.ecm {
                    self.next_block();
                }
            }
            Frame::Ppr => {
                if self.phase == Phase::AwaitingReceipt && self.ecm {
                    self.frames_wanted_again(&message.fif);
                }
            }
            Frame::Ctr => {
                if self.phase == Phase::AwaitingReceipt && self.ecm {
                    self.ecm_command = EcmCommand::Pps;
                    self.attempts = 0;
                    self.pause_then(Phase::Sending);
                }
            }
            Frame::Rnr => {
                // A.5.4.5: "the transmitter immediately sends an RR command
                // until an MCF ... is received correctly".
                if self.phase == Phase::AwaitingReceipt && self.ecm {
                    self.ecm_command = EcmCommand::Rr;
                    self.attempts = 0;
                    self.pause_then(Phase::EndingPage);
                }
            }
            // The far end listens for these on the control channel even while
            // it is waiting for the page carrier: a sender whose partial page
            // signal went unanswered sends it again, and it has to be heard.
            Frame::Pps => {
                if self.ecm && matches!(self.phase, Phase::Receiving | Phase::AwaitingPostMessage) {
                    self.partial_page_signal(&message.fif);
                }
            }
            Frame::Eor => {
                if self.ecm && matches!(self.phase, Phase::Receiving | Phase::AwaitingPostMessage) {
                    self.end_of_retransmission(&message.fif);
                }
            }
            Frame::Ctc => {
                if self.ecm && matches!(self.phase, Phase::Receiving | Phase::AwaitingPostMessage) {
                    // A.4.1: the FIF is bits 1 to 16 of a DCS, and "the
                    // receiving terminal uses only bits 11-14".
                    if let Some((modulation, rate)) = t30::command_rate(&message.fif) {
                        self.modulation = modulation;
                        self.rate = rate;
                    }
                    self.respond(Message::new(Frame::Ctr, false), Phase::Receiving);
                }
            }
            Frame::Rtn => {
                if self.phase == Phase::AwaitingReceipt {
                    self.trouble = Some("the far end could not read the page".to_owned());
                    self.pause_then(Phase::Ending);
                }
            }
            Frame::Eop | Frame::Mps | Frame::Eom => {
                if self.phase == Phase::AwaitingPostMessage {
                    self.finish_page();
                    self.pause_then(Phase::Acknowledging);
                }
            }
            // 5.3.7: either end may disconnect at any point, and a disconnect
            // needs no answer.
            Frame::Dcn => self.phase = Phase::Done,
            _ => {
                if self.phase == Phase::Calling {
                    self.phase = Phase::Listening;
                }
            }
        }
        self.heard.push(message);
    }

    /// How many goes at one command before it is a lost cause (5.4.2).
    const ATTEMPTS: u8 = 3;

    fn timed_out(&mut self) {
        match self.phase {
            Phase::AwaitingConfirm => {
                // The same rate again before a slower one: a command that was
                // not heard is not a line that cannot carry the rate.
                if self.attempts < Self::ATTEMPTS {
                    self.pause_then(Phase::Commanding);
                } else {
                    self.attempts = 0;
                    self.step_down();
                }
            }
            Phase::AwaitingCommand => {
                if self.elapsed > T1_SECONDS {
                    self.give_up("the far end never said what it wanted");
                } else {
                    // 5.4.2: say it again. A DIS that was not heard is the
                    // commonest way a fax call stalls, and the answer is to
                    // repeat it until T1 runs out.
                    self.pause_then(Phase::Identifying);
                }
            }
            Phase::AwaitingReceipt => {
                if self.attempts < Self::ATTEMPTS {
                    self.pause_then(Phase::EndingPage);
                } else {
                    self.bow_out("no receipt for the page");
                }
            }
            Phase::AwaitingPostMessage => {
                self.bow_out("the page stopped and nothing said why");
            }
            Phase::AwaitingDisconnect => self.phase = Phase::Done,
            Phase::CheckingTraining => {
                // Nothing came up on the high-speed carrier at all. Say so
                // with a failure to train rather than sit here: the far end
                // drops a rate and tries again, which may well be the answer.
                self.accepted = false;
                self.pause_then(Phase::Confirming);
            }
            Phase::Receiving => {
                let nothing = if self.ecm {
                    self.collector.count() == 0 && self.ecm_octets.is_empty()
                } else {
                    self.decoder.lines().is_empty()
                };
                if nothing {
                    self.bow_out("the page never arrived");
                } else {
                    self.page_ended();
                }
            }
            _ => {}
        }
    }

    // ---- the caller's side ------------------------------------------------

    /// Every speed both ends have, fastest first.
    ///
    /// One ladder across modulations rather than one per modulation, because
    /// a line that will not carry V.29 at 7200 may well carry V.27 ter at 4800,
    /// and stopping at the bottom of V.29 would give up on a call that had two
    /// more rungs in it.
    pub fn ladder(&self) -> Vec<Speed> {
        let Some(caps) = self.capabilities.as_ref() else {
            return Vec::new();
        };
        let mut speeds: Vec<Speed> = caps
            .modulations
            .iter()
            .filter(|m| self.offer.contains(m))
            .flat_map(|&modulation| {
                let ceiling = caps.ceiling(modulation);
                modulation
                    .rates()
                    .iter()
                    .filter(move |r| **r <= ceiling)
                    .map(move |&bits_per_second| Speed { modulation, bits_per_second })
            })
            .collect();
        speeds.sort_by_key(|s| std::cmp::Reverse(s.bits_per_second));
        speeds
    }

    /// Pick the fastest speed both ends have (5.3.6.2.2).
    fn choose_rate(&mut self) -> bool {
        let mut ladder = self.ladder();
        if ladder.is_empty() {
            self.bow_out("nothing in common with the far end");
            return false;
        }
        let first = ladder.remove(0);
        self.modulation = first.modulation;
        self.rate = first.bits_per_second;
        self.fallback = ladder;
        self.attempts = 0;
        // The page's own resolution if the far end can print it, and standard
        // if not. And the two-dimensional coding whenever the far end reads
        // it, since on anything with lines in it the page comes out smaller.
        if let Some(caps) = self.capabilities.as_ref() {
            let wanted = self.page.as_ref().map_or(Resolution::Standard, |p| p.resolution);
            self.resolution = if wanted == Resolution::Fine && caps.fine_resolution {
                Resolution::Fine
            } else {
                Resolution::Standard
            };
            self.scheme = if caps.two_dimensional {
                t4::Scheme::TwoDimensional
            } else {
                t4::Scheme::OneDimensional
            };
            self.ecm = self.error_correction_offered && caps.error_correction;
        }
        true
    }

    /// Try the next speed down after a failure to train (6.2.7).
    fn step_down(&mut self) {
        if self.fallback.is_empty() {
            self.bow_out("the line would not carry a page at any rate");
            return;
        }
        let next = self.fallback.remove(0);
        self.modulation = next.modulation;
        self.rate = next.bits_per_second;
        self.attempts = 0;
        self.pause_then(Phase::Commanding);
    }

    fn send_command(&mut self) {
        self.attempts += 1;
        let tsi = Message::new(Frame::Tsi, true)
            .and_more()
            .with_fif(&t30::identification_field(&self.identification));
        let dcs = Message::new(Frame::Dcs, true).with_fif(&t30::command(Command {
            modulation: self.modulation,
            bits_per_second: self.rate,
            fine: self.resolution == Resolution::Fine,
            scan_line_field: self.scan_line_field,
            two_dimensional: self.scheme == t4::Scheme::TwoDimensional,
            error_correction: self.ecm,
        }));
        self.sender.send(&[tsi, dcs]);
    }

    fn send_training_check(&mut self) {
        // 6.2.6: zeros for a second and a half, through the training that
        // comes in front of them.
        let bits = (TCF_SECONDS * f64::from(self.rate)) as usize;
        self.fast_out = vec![false; bits];
        self.fast_at = 0;
    }

    fn fast_out_page(&self) -> Vec<bool> {
        let Some(page) = self.page.as_ref() else {
            return Vec::new();
        };
        // The minimum scan line time is not a property of the picture but of
        // the paper at the far end, and it arrived in the DIS. Under error
        // correction mode there is none: Note 8, "0 ms".
        let min_bits = if self.ecm {
            0
        } else {
            (self.scan_line_ms / 1000.0 * f64::from(self.rate)).ceil() as usize
        };
        // A fine page to a machine that only prints standard loses every
        // other line: 7.7 lines to the millimetre is exactly twice 3.85, so
        // that is the same page at the resolution it can take.
        let halved: Vec<Vec<bool>>;
        let lines = if page.resolution == Resolution::Fine && self.resolution == Resolution::Standard {
            halved = page.lines.iter().step_by(2).cloned().collect();
            &halved
        } else {
            &page.lines
        };
        match self.scheme {
            t4::Scheme::OneDimensional => t4::encode_padded(lines, min_bits).to_bits(),
            t4::Scheme::TwoDimensional => {
                crate::mr::encode(lines, crate::mr::k_for(self.resolution), min_bits).to_bits()
            }
        }
    }

    fn send_page(&mut self) {
        if !self.ecm {
            self.fast_out = self.fast_out_page();
            self.fast_at = 0;
            return;
        }
        if self.ecm_frames.is_empty() {
            self.ecm_frames = ecm::frames(&ecm::pack(&self.fast_out_page()), ecm::FRAME_OCTETS);
            self.ecm_block = 0;
        }
        let bits = {
            let block = self.ecm_block_frames();
            let numbers: Vec<usize> = if self.ecm_resend.is_empty() {
                (0..block.len()).collect()
            } else {
                self.ecm_resend.clone()
            };
            let frames: Vec<(u8, &[u8])> = numbers
                .iter()
                .filter_map(|&n| block.get(n).map(|data| (n as u8, data.as_slice())))
                .collect();
            ecm::partial_page(&frames, self.rate)
        };
        self.fast_out = bits;
        self.fast_at = 0;
    }

    /// The frames of the block in hand.
    fn ecm_block_frames(&self) -> &[Vec<u8>] {
        let start = (self.ecm_block * ecm::BLOCK_FRAMES).min(self.ecm_frames.len());
        let end = (start + ecm::BLOCK_FRAMES).min(self.ecm_frames.len());
        &self.ecm_frames[start..end]
    }

    fn ecm_blocks(&self) -> usize {
        self.ecm_frames.len().div_ceil(ecm::BLOCK_FRAMES).max(1)
    }

    fn send_end_of_page(&mut self) {
        self.attempts += 1;
        if !self.ecm {
            // One page and no more, so end of procedure rather than
            // multi-page signal (6.2.9).
            self.sender.send(&[Message::new(Frame::Eop, true)]);
            return;
        }
        let frames = self.ecm_block_frames().len();
        let last = self.ecm_block + 1 >= self.ecm_blocks();
        let command = if last {
            ecm::PostMessage::Eop
        } else {
            ecm::PostMessage::Null
        };
        let message = match self.ecm_command {
            EcmCommand::Pps => Message::new(Frame::Pps, true).with_fif(&ecm::pps_field(
                command,
                self.ecm_page,
                self.ecm_block as u8,
                frames,
            )),
            EcmCommand::Eor => Message::new(Frame::Eor, true).with_fif(&[command.code()]),
            EcmCommand::Ctc => {
                let dcs = t30::command(Command {
                    modulation: self.modulation,
                    bits_per_second: self.rate,
                    fine: self.resolution == Resolution::Fine,
                    scan_line_field: self.scan_line_field,
                    two_dimensional: self.scheme == t4::Scheme::TwoDimensional,
                    error_correction: true,
                });
                Message::new(Frame::Ctc, true).with_fif(&dcs[..2])
            }
            EcmCommand::Rr => Message::new(Frame::Rr, true),
        };
        self.sender.send(&[message]);
    }

    /// The block in hand has arrived, or been given up on: on to the next,
    /// or to the end of the call.
    fn next_block(&mut self) {
        self.ecm_block += 1;
        self.ecm_resend.clear();
        self.ecm_pprs = 0;
        self.ecm_command = EcmCommand::Pps;
        self.attempts = 0;
        if self.ecm_block < self.ecm_blocks() {
            self.pause_then(Phase::Sending);
        } else {
            self.pause_then(Phase::Ending);
        }
    }

    /// A PPR: send the frames it asks for, or, the fourth time, change the
    /// terms (A.1.3).
    fn frames_wanted_again(&mut self, fif: &[u8]) {
        let wanted = ecm::read_ppr(fif, self.ecm_block_frames().len());
        if wanted.is_empty() {
            // A PPR asking for nothing is a confirmation in all but name.
            self.next_block();
            return;
        }
        self.ecm_resend = wanted;
        self.ecm_pprs += 1;
        self.attempts = 0;
        if self.ecm_pprs < ecm::PPRS_BEFORE_GIVING_WAY {
            self.pause_then(Phase::Sending);
        } else if !self.fallback.is_empty() {
            // "The modem speed may fall back or continue at the same speed in
            // accordance with the decision of the transmitting terminal." A
            // block that has failed four times at a rate is a rate to leave.
            let next = self.fallback.remove(0);
            self.modulation = next.modulation;
            self.rate = next.bits_per_second;
            self.ecm_pprs = 0;
            self.ecm_command = EcmCommand::Ctc;
            self.pause_then(Phase::EndingPage);
        } else {
            // Nowhere slower to go: give up on what is still missing, and let
            // the coding make what it can of the rest.
            self.ecm_command = EcmCommand::Eor;
            self.pause_then(Phase::EndingPage);
        }
    }

    // ---- the answerer's side ----------------------------------------------

    fn send_identity(&mut self) {
        let csi = Message::new(Frame::Csi, false)
            .and_more()
            .with_fif(&t30::identification_field(&self.identification));
        let dis = Message::new(Frame::Dis, false).with_fif(&t30::our_capabilities(&self.offer, self.error_correction_offered));
        self.sender.send(&[csi, dis]);
    }

    fn send_confirmation(&mut self) {
        let frame = if self.accepted { Frame::Cfr } else { Frame::Ftt };
        self.sender.send(&[Message::new(frame, false)]);
    }

    /// Confirm the page, or ask for it again (6.2.7 and 6.3.2).
    ///
    /// T.30 leaves the threshold to the receiver, as it does with the
    /// training check: it says only that a machine decides whether the
    /// signal it received is acceptable. A twentieth of the lines spoiled is
    /// the ordinary limit, and it is generous -- a page that far gone is
    /// still readable, and asking for it again costs another minute.
    fn send_acknowledgement(&mut self) {
        if let Some(message) = self.ecm_response.take() {
            self.sender.send(&[message]);
            return;
        }
        let lines = self.decoder.lines().len();
        let good = lines > 0 && self.decoder.damaged() * 20 <= lines;
        let frame = if good { Frame::Mcf } else { Frame::Rtn };
        if !good {
            self.trouble = Some(format!(
                "{} of {lines} lines came out wrong",
                self.decoder.damaged()
            ));
        }
        self.sender.send(&[Message::new(frame, false)]);
    }

    /// Judge a training check (6.2.6).
    fn fast_burst_heard(&mut self) {
        if self.phase != Phase::CheckingTraining {
            return;
        }
        self.accepted = self.training_check_passes();
        self.pause_then(Phase::Confirming);
    }

    /// Whether more than a training check's worth has arrived.
    fn check_is_long_enough(&self) -> bool {
        let seconds = self.fast_in.len() as f64 / f64::from(self.rate);
        seconds > 2.0 * (1.0 + TCF_SECONDS)
    }

    fn training_check_passes(&self) -> bool {
        // Where the zeros begin, which is where the training stops.
        let mut run = 0usize;
        let mut start = None;
        for (i, &bit) in self.fast_in.iter().enumerate() {
            run = if bit { 0 } else { run + 1 };
            if run == TCF_STARTS_AFTER {
                start = Some(i + 1 - TCF_STARTS_AFTER);
                break;
            }
        }
        let Some(start) = start else { return false };
        // A second and a half of it from there, or whatever arrived.
        let want = (TCF_SECONDS * f64::from(self.rate)) as usize;
        let end = (start + want).min(self.fast_in.len());
        let window = &self.fast_in[start..end];
        // Half the expected length has to be there at all: a burst that was
        // cut short is not a channel that will carry a page.
        if window.len() * 2 < want {
            return false;
        }
        let zeros = window.iter().filter(|b| !**b).count();
        zeros as f64 >= TCF_ZEROS_WANTED * window.len() as f64
    }

    fn page_ended(&mut self) {
        // Under error correction mode a burst is a partial page, and what it
        // was part of is only known once the post-message command says.
        if !self.ecm {
            self.finish_page();
        }
        self.enter(Phase::AwaitingPostMessage);
    }

    /// Answer under error correction mode, then go on to `next`.
    fn respond(&mut self, message: Message, next: Phase) {
        self.ecm_response = Some(message);
        self.after_answering = next;
        self.pause_then(Phase::Acknowledging);
    }

    /// Where a post-message command leaves the call once it has been answered.
    fn after(command: ecm::PostMessage) -> Phase {
        match command {
            ecm::PostMessage::Null | ecm::PostMessage::Mps => Phase::Receiving,
            ecm::PostMessage::Eop => Phase::AwaitingDisconnect,
            ecm::PostMessage::Eom => Phase::AwaitingCommand,
        }
    }

    /// A PPS: confirm the block if it is whole, or ask for what is missing.
    fn partial_page_signal(&mut self, fif: &[u8]) {
        let Some((command, page, block, frames)) = ecm::read_pps(fif) else {
            return;
        };
        self.ecm_expected = frames;
        if self.collector.count() == 0 && self.ecm_confirmed == Some((page, block)) {
            // This block was confirmed and the confirmation was lost, so the
            // same PPS has come again. The block is already in the page; say
            // so again rather than asking for it all over.
            self.respond(Message::new(Frame::Mcf, false), Self::after(command));
            return;
        }
        if self.collector.complete(frames) {
            let data = self.collector.take_block(frames);
            self.ecm_octets.extend_from_slice(&data);
            self.ecm_confirmed = Some((page, block));
            self.block_accepted(command, Frame::Mcf);
        } else {
            let fif = ecm::ppr_field(frames, |i| self.collector.has(i));
            self.respond(Message::new(Frame::Ppr, false).with_fif(&fif), Phase::Receiving);
        }
    }

    /// An EOR: the sender has given up on what is still missing.
    fn end_of_retransmission(&mut self, fif: &[u8]) {
        let command = fif
            .first()
            .and_then(|&code| ecm::PostMessage::from_code(code))
            .unwrap_or(ecm::PostMessage::Null);
        let data = self.collector.take_block(self.ecm_expected.max(1));
        self.ecm_octets.extend_from_slice(&data);
        self.block_accepted(command, Frame::Err);
    }

    /// A block is done with: finish the page if the command says it is over,
    /// and answer.
    fn block_accepted(&mut self, command: ecm::PostMessage, answer: Frame) {
        self.follow_ecm_page();
        if command != ecm::PostMessage::Null {
            self.finish_ecm_page();
            self.ecm_octets.clear();
        }
        self.respond(Message::new(answer, false), Self::after(command));
    }

    /// Decode as much of a page arriving in frames as has arrived in order.
    ///
    /// The blocks already confirmed, and then the run of the block in hand
    /// that has arrived unbroken from its first frame -- so that the page can
    /// be watched arriving, as it can without error correction, rather than
    /// appearing whole at the end. Nothing after a missing frame goes in until
    /// that frame does: the coding is a stream, and what follows a gap in it
    /// is not the page that follows the gap.
    fn follow_ecm_page(&mut self) {
        let mut at = 0;
        let confirmed = std::iter::once(self.ecm_octets.as_slice());
        for chunk in confirmed.chain(self.collector.leading()) {
            let end = at + chunk.len();
            if end > self.ecm_fed {
                // Everything before `at` is in already, so this is where the
                // decoder has got to.
                self.decoder.feed_bits(&ecm::unpack(&chunk[self.ecm_fed - at..]));
                self.ecm_fed = end;
            }
            at = end;
        }
    }

    /// Finish a page that arrived in frames.
    ///
    /// Everything confirmed has gone into the decoder by now, bar what the
    /// last block brought, and a block the sender gave up on goes in with its
    /// missing frames left out, which is what T.4's coding resynchronizes on.
    fn finish_ecm_page(&mut self) {
        self.follow_ecm_page();
        if self.received.is_none() && !self.decoder.lines().is_empty() {
            self.received = Some(self.decoder.page(self.resolution));
        }
    }

    fn finish_page(&mut self) {
        if self.received.is_none() && !self.decoder.lines().is_empty() {
            self.received = Some(self.decoder.page(self.resolution));
        }
    }

    // ---- both -------------------------------------------------------------

    fn send_disconnect(&mut self) {
        self.sender
            .send(&[Message::new(Frame::Dcn, self.role == Role::Caller)]);
    }

    /// A burst of frames has finished leaving the line.
    fn control_burst_ended(&mut self) {
        match self.phase {
            Phase::Commanding => self.pause_then(Phase::Training),
            Phase::Identifying => self.enter(Phase::AwaitingCommand),
            Phase::Confirming => {
                if self.accepted {
                    self.pause_then(Phase::Receiving);
                } else {
                    // A failure to train sends the far end back to its DCS.
                    self.enter(Phase::AwaitingCommand);
                }
            }
            Phase::Acknowledging => {
                if self.ecm {
                    match self.after_answering {
                        // The sender turns its carrier round and trains; the
                        // settling gap is this end's half of that.
                        Phase::Receiving => self.pause_then(Phase::Receiving),
                        next => self.enter(next),
                    }
                } else {
                    self.enter(Phase::AwaitingDisconnect);
                }
            }
            Phase::EndingPage => self.enter(Phase::AwaitingReceipt),
            _ => {}
        }
    }

    /// A high-speed burst has finished leaving the line.
    fn fast_burst_ended(&mut self) {
        self.fast_out.clear();
        self.fast_at = 0;
        match self.phase {
            Phase::Training => self.pause_then(Phase::AwaitingConfirm),
            Phase::Sending => self.pause_then(Phase::EndingPage),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames;
    use crate::t30::Frame;

    const FS: f64 = 16_000.0;
    /// A real machine's capability frame, off a recording of a public fax
    /// number answering.
    const DIS: [u8; 4] = [0x00, 0x6e, 0xf8, 0x00];
    const CSI: &[u8; 20] = b"       909 863  0031";

    /// Run a call, feeding it whatever the far end is made to say.
    ///
    /// The far end only ever talks on the control channel here, so anything
    /// past phase B stalls, which is the point of most of these.
    fn run(far: &[Message], seconds: f64) -> Call {
        run_until(far, seconds, |c| c.phase().is_over())
    }

    /// The same, stopping the moment `done` is happy.
    ///
    /// Worth having because the far end here never answers a training check,
    /// so a call left running long enough always ends up stepping down a rate
    /// and then giving up -- which is right, and is not what most of these
    /// are asking about.
    fn run_until(far: &[Message], seconds: f64, done: impl Fn(&Call) -> bool) -> Call {
        run_call(Call::originate(FS, "61400000000", None), far, seconds, done)
    }

    /// The same, with a call already made.
    fn run_call(
        mut call: Call,
        far: &[Message],
        seconds: f64,
        done: impl Fn(&Call) -> bool,
    ) -> Call {
        let mut tx = frames::Sender::new();
        tx.send(far);
        for _ in 0..(seconds * FS) as usize {
            // The far end speaks at 300 bit/s; one bit every so many samples.
            let listening = matches!(call.line(), Line::Listen | Line::CallingTone);
            if listening
                && (call.seconds() * 300.0).fract() < 300.0 / FS
                && let Some(bit) = tx.next_bit()
            {
                call.control_bit(bit);
            }
            call.tick(true);
            // And drain whatever this end is sending, as a line would.
            while call.next_control_bit().is_some() {}
            while call.next_fast_bit().is_some() {}
            if done(&call) {
                break;
            }
        }
        call
    }


    /// A training check as it really arrives: training first, then zeros,
    /// with `errors` of them flipped, then the tail of the turn-off.
    fn a_training_check(rate: u32, errors: usize) -> Vec<bool> {
        let mut bits: Vec<bool> = (0..3400)
            .map(|i: u32| !i.wrapping_mul(2_654_435_761).is_multiple_of(3))
            .collect();
        let zeros = (TCF_SECONDS * f64::from(rate)) as usize;
        let start = bits.len();
        bits.extend(std::iter::repeat_n(false, zeros));
        for k in 0..errors {
            // Spread them out, so no two land next to each other.
            bits[start + (k + 1) * zeros / (errors + 1)] = true;
        }
        bits.extend(std::iter::repeat_n(true, 30));
        bits
    }

    fn judge(rate: u32, bits: Vec<bool>) -> bool {
        let mut call = Call::answer(FS, "1");
        call.rate = rate;
        call.fast_in = bits;
        call.training_check_passes()
    }

    #[test]
    fn a_clean_training_check_is_accepted() {
        assert!(judge(4800, a_training_check(4800, 0)));
        assert!(judge(2400, a_training_check(2400, 0)));
    }

    #[test]
    fn a_training_check_with_a_few_errors_in_it_is_still_good_enough() {
        // One bit in a second and a half is a line that will carry a page.
        // Requiring one unbroken run of zeros threw the rate away for it.
        assert!(judge(4800, a_training_check(4800, 1)));
        assert!(judge(4800, a_training_check(4800, 20)));
    }

    #[test]
    fn a_training_check_that_is_mostly_wrong_is_refused() {
        let mut bits = a_training_check(4800, 0);
        let zeros = (TCF_SECONDS * 4800.0) as usize;
        let from = bits.len() - zeros;
        for (i, bit) in bits[from..].iter_mut().enumerate() {
            *bit = !i.is_multiple_of(5);
        }
        assert!(!judge(4800, bits));
    }

    #[test]
    fn nothing_at_all_on_the_fast_carrier_is_refused() {
        assert!(!judge(4800, Vec::new()));
        assert!(!judge(4800, vec![true; 5000]), "ones are not a check");
    }

    #[test]
    fn a_check_that_was_cut_short_is_refused() {
        let mut bits = a_training_check(4800, 0);
        bits.truncate(3400 + 2000);
        assert!(!judge(4800, bits), "a third of a check is not a check");
    }

    #[test]
    fn a_page_may_take_longer_than_a_command_timeout() {
        // A page is a minute at 4800 and four at 2400. T2 is six seconds, and
        // it has to mean how long the line may go quiet rather than how long
        // the page may take.
        let mut coded = t4::Bits::new();
        for y in 0..300 {
            let line: Vec<bool> = (0..crate::page::WIDTH)
                .map(|x| (x / 50 + y / 4).is_multiple_of(2))
                .collect();
            t4::write_line(&mut coded, &line);
        }
        let bits = coded.to_bits();

        let mut call = Call::answer(FS, "1");
        call.enter(Phase::Receiving);
        call.set_fast_carrier(true);
        let long = T2_SECONDS * 3.0;
        let samples = (FS * long) as usize;
        let every = samples / bits.len();
        for i in 0..samples {
            if i.is_multiple_of(every)
                && let Some(&bit) = bits.get(i / every)
            {
                call.fast_bits(&[bit]);
            }
            call.tick(true);
        }
        assert!(call.lines_received() > 0, "no line arrived at all");
        assert_eq!(
            call.phase(),
            Phase::Receiving,
            "it gave up on a page that was still arriving"
        );
    }

    #[test]
    fn a_page_that_stops_arriving_does_not_wait_for_ever() {
        // And noise is not a page. What arrives when a carrier detector has
        // latched onto the hiss on a line is a bit every symbol for ever,
        // which is bits without lines. A carrier that is still there is waited
        // on, because a burst this end cannot read is still a burst -- but for
        // T5 and no longer.
        let mut call = Call::answer(FS, "1");
        call.enter(Phase::Receiving);
        call.set_fast_carrier(true);
        for i in 0..20_000 {
            call.fast_bits(&[!(i as u32).wrapping_mul(2_654_435_761).is_multiple_of(3)]);
        }
        for _ in 0..(FS * (T2_SECONDS + 1.0)) as usize {
            call.tick(true);
        }
        assert_eq!(
            call.phase(),
            Phase::Receiving,
            "gave up on a carrier that was still on the line"
        );
        for _ in 0..(FS * T5_SECONDS) as usize {
            call.tick(true);
        }
        assert_ne!(call.phase(), Phase::Receiving, "it waited for ever");
    }

    #[test]
    fn a_page_carrier_that_goes_away_is_not_waited_on() {
        // The carrier gone and nothing readable arrived: six seconds, not
        // sixty.
        let mut call = Call::answer(FS, "1");
        call.enter(Phase::Receiving);
        call.set_fast_carrier(false);
        for _ in 0..(FS * (T2_SECONDS + 1.0)) as usize {
            call.tick(true);
        }
        assert_ne!(call.phase(), Phase::Receiving, "it waited on silence");
    }

    #[test]
    fn a_command_is_tried_again_before_the_rate_is_dropped() {
        // 5.4.2 allows three goes. A DCS that was not heard is not a line
        // that cannot carry the rate, and dropping a rung on the first
        // silence makes the page take longer for no reason.
        let call = run_until(
            &[Message::new(Frame::Dis, false).with_fif(&DIS)],
            30.0,
            |c| c.rate() == 7200,
        );
        assert_eq!(call.rate(), 7200, "it never dropped a rate at all");
        assert_eq!(call.modulation(), Modulation::V29, "it skipped a rung");
        let commands = call
            .heard
            .iter()
            .filter(|m| m.frame == Frame::Dis)
            .count();
        assert_eq!(commands, 1, "the far end only ever sent one DIS");
        assert!(
            call.seconds() > 3.0 * T4_SECONDS,
            "it dropped the rate after {:.1} s, too soon for three tries",
            call.seconds()
        );
    }

    #[test]
    fn giving_up_says_goodbye_first() {
        // A machine that simply stops answering leaves the far end holding
        // the line for its whole T1 and then reporting a failure.
        let mut fif = vec![0u8; 3];
        t30::set_bit(&mut fif, 10, true);
        // Bits 11 to 14 as 1000: V.29 alone, and this end told to use only
        // V.27 ter, so there is nothing both of them have.
        t30::set_field(&mut fif, 11, 14, 0b1000);
        let mut ours = Call::originate(FS, "1", None);
        ours.set_offer(&[Modulation::V27ter]);
        let call = run_call(
            ours,
            &[Message::new(Frame::Dis, false).with_fif(&fif)],
            20.0,
            |c| c.phase().is_over(),
        );
        assert_eq!(call.phase(), Phase::Done, "it did not hang up politely");
        assert!(call.trouble.is_some(), "it gave up without saying why");
    }


    #[test]
    fn a_timeout_waits_for_the_far_end_to_stop_talking() {
        // The far end's flags are on the line when the six seconds run out.
        // Repeating the DIS now would talk over its command.
        let mut call = Call::answer(FS, "1");
        call.phase = Phase::AwaitingCommand;
        call.timer = 0.01;
        call.set_control_carrier(true);
        for _ in 0..(FS * 2.0) as usize {
            call.tick(true);
        }
        assert_eq!(
            call.phase(),
            Phase::AwaitingCommand,
            "it talked over the far end"
        );
        call.set_control_carrier(false);
        for _ in 0..(FS * 0.2) as usize {
            call.tick(true);
        }
        assert_ne!(
            call.phase(),
            Phase::AwaitingCommand,
            "it went on waiting after the far end stopped"
        );
    }

    #[test]
    fn a_carrier_that_never_goes_away_does_not_hold_the_call_for_ever() {
        let mut call = Call::answer(FS, "1");
        call.phase = Phase::AwaitingCommand;
        call.timer = 0.01;
        call.set_control_carrier(true);
        for _ in 0..(FS * (HOLD_FOR_THE_FAR_END + 1.0)) as usize {
            call.tick(true);
        }
        assert_ne!(call.phase(), Phase::AwaitingCommand, "it waited for ever");
    }


    fn fine_page(rows: usize) -> Page {
        Page {
            lines: (0..rows)
                .map(|y| (0..crate::page::WIDTH).map(|x| (x + y) % 50 < 5).collect())
                .collect(),
            resolution: Resolution::Fine,
        }
    }

    /// The command this end sends, for a page, to a far end that sent `dis`.
    fn command_for(page: Option<Page>, dis: &[u8]) -> Vec<u8> {
        let mut call = Call::originate(FS, "1", page);
        call.capabilities = Some(t30::capabilities(dis));
        assert!(call.choose_rate());
        call.enter(Phase::Commanding);
        let bits: Vec<bool> = std::iter::from_fn(|| call.next_control_bit()).collect();
        let mut reader = Reader::new();
        let sent: Vec<Message> = bits.iter().filter_map(|b| reader.feed(*b)).collect();
        sent.into_iter()
            .find(|m| m.frame == Frame::Dcs)
            .expect("no DCS")
            .fif
    }

    #[test]
    fn a_fine_page_is_commanded_as_fine() {
        // It was not: the end that dialled never took its resolution from the
        // page, so every page went out saying standard, and a fine page arrived
        // at twice its height.
        let dcs = command_for(Some(fine_page(10)), &t30::our_capabilities(&OUR_MODULATIONS, true));
        assert!(t30::bit(&dcs, 15), "a fine page was commanded as standard");
    }

    #[test]
    fn a_fine_page_to_a_standard_machine_is_sent_standard_and_halved() {
        let mut fif = t30::our_capabilities(&OUR_MODULATIONS, true);
        t30::set_bit(&mut fif, 15, false);
        let page = fine_page(10);
        let dcs = command_for(Some(page.clone()), &fif);
        assert!(!t30::bit(&dcs, 15), "commanded fine to a machine without it");

        let mut call = Call::originate(FS, "1", Some(page));
        call.capabilities = Some(t30::capabilities(&fif));
        assert!(call.choose_rate());
        let mut decoder = t4::Decoder::with_scheme(crate::page::WIDTH, call.scheme);
        decoder.feed_bits(&call.fast_out_page());
        assert_eq!(decoder.lines().len(), 5, "ten fine lines are five standard ones");
    }

    #[test]
    fn two_dimensional_coding_is_used_exactly_when_the_far_end_reads_it() {
        let page = fine_page(10);
        let ours = t30::our_capabilities(&OUR_MODULATIONS, true);
        assert!(t30::bit(&command_for(Some(page.clone()), &ours), 16));
        // The real machine's DIS, which does not offer it.
        assert!(!t30::bit(&command_for(Some(page), &DIS), 16));
    }

    #[test]
    fn the_calling_tone_is_on_for_half_a_second_in_every_three_and_a_half() {
        let mut call = Call::originate(FS, "1", None);
        let mut on = 0usize;
        let total = (FS * (crate::CNG_ON + crate::CNG_OFF)) as usize;
        for _ in 0..total {
            if call.calling_tone_on() {
                on += 1;
            }
            call.tick(true);
        }
        let fraction = on as f64 / total as f64;
        let want = crate::CNG_ON / (crate::CNG_ON + crate::CNG_OFF);
        assert!(
            (fraction - want).abs() < 0.02,
            "the tone was on {fraction:.3} of the time, wanted {want:.3}"
        );
    }

    #[test]
    fn a_machine_that_says_what_it_is_gets_heard() {
        let call = run_until(
            &[
                Message::new(Frame::Csi, false).and_more().with_fif(CSI),
                Message::new(Frame::Dis, false).with_fif(&DIS),
            ],
            10.0,
            |c| c.phase() == Phase::Training,
        );
        assert_eq!(call.identity, "1300  368 909");
        let caps = call.capabilities.clone().expect("it said what it can do");
        assert_eq!(
            caps.modulations,
            vec![
                Modulation::V27ter,
                Modulation::V29,
                Modulation::V17
            ]
        );
        assert!(caps.receives);
        // V.17 is not built, so the fastest thing in common is V.29 at 9600.
        assert_eq!(call.rate(), 9600);
        assert_eq!(call.modulation(), Modulation::V29);
    }

    #[test]
    fn a_machine_offering_only_the_fall_back_gets_the_fall_back() {
        // Bits 11 to 14 as 0000: V.27 ter fall-back mode, 2400 and no more.
        let mut fif = vec![0u8; 3];
        t30::set_bit(&mut fif, 10, true);
        t30::set_field(&mut fif, 11, 14, 0b0000);
        let call = run_until(
            &[Message::new(Frame::Dis, false).with_fif(&fif)],
            10.0,
            |c| c.phase() == Phase::Training,
        );
        assert_eq!(call.rate(), 2400);
    }

    #[test]
    fn nothing_at_all_gives_up_after_t1_and_not_before() {
        let call = run(&[], T1_SECONDS - 2.0);
        assert_eq!(call.phase(), Phase::Calling, "gave up early");
        let call = run(&[], T1_SECONDS + 2.0);
        assert_eq!(call.phase(), Phase::Failed);
        assert!(call.trouble.is_some(), "it failed without saying why");
    }

    #[test]
    fn a_far_end_that_hangs_up_ends_the_call() {
        let call = run(&[Message::new(Frame::Dcn, false)], 10.0);
        assert_eq!(call.phase(), Phase::Done);
        assert!(call.capabilities.is_none());
    }

    #[test]
    fn the_command_names_one_rate_and_the_identification_goes_out_backwards() {
        let mut call = Call::originate(FS, "61400000000", None);
        call.capabilities = Some(t30::capabilities(&DIS));
        call.choose_rate();
        call.enter(Phase::Commanding);
        let mut bits = Vec::new();
        while let Some(b) = call.next_control_bit() {
            bits.push(b);
        }
        let mut reader = Reader::new();
        let sent: Vec<Message> = bits.iter().filter_map(|b| reader.feed(*b)).collect();
        assert_eq!(sent.len(), 2, "a TSI and a DCS, in that order");
        assert_eq!(sent[0].frame, Frame::Tsi);
        assert!(sent[0].from_caller, "we are the end that dialled");
        assert_eq!(t30::identification(&sent[0].fif), "61400000000");
        assert_eq!(sent[1].frame, Frame::Dcs);
        assert_eq!(
            t30::command_rate(&sent[1].fif),
            Some((Modulation::V29, 9600))
        );
    }

    #[test]
    fn what_we_offer_reads_back_as_what_we_can_do() {
        // The DIS this modem sends, read with the same reader that reads
        // everybody else's. A capability frame that cannot be read by its own
        // parser is one no far end will read either.
        let caps = t30::capabilities(&t30::our_capabilities(&OUR_MODULATIONS, true));
        assert!(caps.receives);
        assert!(!caps.can_be_polled, "there is nothing here to fetch");
        assert_eq!(caps.modulations, vec![Modulation::V27ter, Modulation::V29]);
        assert!(caps.fine_resolution);
        assert!(caps.two_dimensional, "T.4 4.2 is offered as well as 4.1");
        assert!(caps.error_correction, "T.30 Annex A is offered");
        assert_eq!(caps.widths_mm, vec![215]);
        assert_eq!(caps.length, "unlimited");
        assert_eq!(caps.scan_line_ms, 0.0);
        assert_eq!(caps.octets, 4, "bit 27 is in the fourth octet");

        // And without it, the three octets of a DIS with no extension.
        let plain = t30::our_capabilities(&OUR_MODULATIONS, false);
        assert_eq!(plain.len(), 3);
        assert!(!t30::capabilities(&plain).error_correction);
        assert!(!t30::bit(&plain, 24), "an extension bit with nothing after it");
    }

    #[test]
    fn a_narrower_offer_reads_back_as_narrower() {
        for (offer, want) in [
            (vec![Modulation::V27ter], vec![Modulation::V27ter]),
            (vec![Modulation::V29], vec![Modulation::V29]),
            (vec![Modulation::V29, Modulation::V27ter], vec![Modulation::V27ter, Modulation::V29]),
            // Nothing at all is V.27 ter, which every machine must have.
            (vec![], vec![Modulation::V27ter]),
        ] {
            let caps = t30::capabilities(&t30::our_capabilities(&offer, true));
            assert_eq!(caps.modulations, want, "offering {offer:?}");
        }
    }

    #[test]
    fn the_ladder_runs_down_through_v29_and_on_into_v27ter() {
        // The real machine's DIS offers all three. Without V.17 here, the
        // ladder is V.29's two rates and then V.27 ter's two, in that order:
        // a line that will not carry 7200 may well carry 4800, and a call
        // that stopped at the bottom of one modulation would give up with two
        // rungs left.
        let mut call = Call::originate(FS, "1", None);
        call.capabilities = Some(t30::capabilities(&DIS));
        let ladder: Vec<(Modulation, u32)> = call
            .ladder()
            .iter()
            .map(|s| (s.modulation, s.bits_per_second))
            .collect();
        assert_eq!(
            ladder,
            vec![
                (Modulation::V29, 9600),
                (Modulation::V29, 7200),
                (Modulation::V27ter, 4800),
                (Modulation::V27ter, 2400),
            ]
        );
    }

    #[test]
    fn what_this_end_will_not_use_is_not_on_the_ladder() {
        let mut call = Call::originate(FS, "1", None);
        call.set_offer(&[Modulation::V27ter]);
        call.capabilities = Some(t30::capabilities(&DIS));
        assert!(call.ladder().iter().all(|s| s.modulation == Modulation::V27ter));
        // And V.17, which the far end offers and this end has not got, is
        // never offered no matter what is asked for.
        call.set_offer(&[Modulation::V17]);
        assert_eq!(call.offer, vec![Modulation::V27ter]);
    }

    #[test]
    fn every_failure_to_train_takes_one_rung_down() {
        let mut call = Call::originate(FS, "1", None);
        call.capabilities = Some(t30::capabilities(&DIS));
        assert!(call.choose_rate());
        let mut seen = vec![(call.modulation(), call.rate())];
        for _ in 0..3 {
            call.step_down();
            seen.push((call.modulation(), call.rate()));
        }
        assert_eq!(
            seen,
            vec![
                (Modulation::V29, 9600),
                (Modulation::V29, 7200),
                (Modulation::V27ter, 4800),
                (Modulation::V27ter, 2400),
            ]
        );
        call.step_down();
        assert!(call.trouble.is_some(), "ran off the bottom without saying so");
    }

    #[test]
    fn the_answering_end_holds_the_called_tone_and_then_says_what_it_is() {
        let mut call = Call::answer(FS, "61399990000");
        assert_eq!(call.line(), Line::CalledTone);
        for _ in 0..(FS * (CED_SECONDS - 0.2)) as usize {
            call.tick(true);
        }
        assert_eq!(call.line(), Line::CalledTone, "the tone stopped early");
        for _ in 0..(FS * 0.4) as usize {
            call.tick(true);
        }
        // The settling gap first, and then the frames.
        assert_eq!(call.phase(), Phase::Identifying);
        for _ in 0..(FS * TURNAROUND_SECONDS * 1.2) as usize {
            call.tick(true);
        }
        assert_eq!(call.line(), Line::Control);
        let mut bits = Vec::new();
        while let Some(b) = call.next_control_bit() {
            bits.push(b);
        }
        let mut reader = Reader::new();
        let sent: Vec<Message> = bits.iter().filter_map(|b| reader.feed(*b)).collect();
        let names: Vec<Frame> = sent.iter().map(|m| m.frame).collect();
        assert_eq!(names, vec![Frame::Csi, Frame::Dis]);
        assert_eq!(t30::identification(&sent[0].fif), "61399990000");
    }

    #[test]
    fn every_change_of_modulation_has_a_settling_gap_in_front_of_it() {
        // NOTE 3 under 5.1. The gap is the whole reason a fax call takes as
        // long as it does, and leaving it out is invisible in a loopback and
        // fatal on a line with an echo suppressor in it.
        let mut call = Call::originate(FS, "1", None);
        call.capabilities = Some(t30::capabilities(&DIS));
        call.choose_rate();
        call.pause_then(Phase::Commanding);
        let mut quiet = 0usize;
        while call.line() == Line::Quiet {
            call.tick(true);
            quiet += 1;
        }
        let seconds = quiet as f64 / FS;
        assert!(
            (seconds - TURNAROUND_SECONDS).abs() < 0.005,
            "the gap was {:.0} ms, and 5.1 asks for {:.0}",
            seconds * 1000.0,
            TURNAROUND_SECONDS * 1000.0
        );
    }
}
