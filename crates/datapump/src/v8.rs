//! V.8's signals on a line: the V.21 channels that carry them, and clause 8.
//!
//! The messages themselves are the `v8` crate's, and know nothing of samples.
//! This is what puts them on a wire and what decides when: 300 bit/s over
//! V.21, the calling modem in the low channel and the answering modem in the
//! high one, with the timings of clause 8 around them.
//!
//! What it is for is the thing every capture of a failed call has shown. A
//! modem start-up assumes both ends already know which Recommendation is being
//! followed; nothing in V.32 or V.22bis says so, and two modems that guessed
//! differently transmit past each other until one gives up. V.8 asks first.

use dsp::filter::OnePole;
use dsp::Nco;
use v8::{Access, CallFunction, Decoder, Heard, Menu, Modulation, Modulations, Pcm, PcmRole, Protocol, Signal};

use crate::bell103::{Bell103Rx, Bell103Tx};
use crate::framing::AsyncBits;

/// V.8's signals are all at 300 bit/s (3.1, 3.4, 3.5, 3.6).
pub const BAUD: f64 = 300.0;

/// V.21 channel 1: `FA = 1180 Hz and Fz = 980 Hz`, which is space and mark.
pub const LOW: (f64, f64) = (1180.0, 980.0);

/// V.21 channel 2: `FA = 1850 Hz and Fz = 1650 Hz`.
pub const HIGH: (f64, f64) = (1850.0, 1650.0);

/// Which end of the call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Placed the call. Sends CI, CM and CJ in the low channel; hears JM in
    /// the high one.
    Calling,
    /// Took the call. Sends ANSam and JM; hears CM in the low channel.
    Answering,
}

impl Role {
    fn transmit_tones(self) -> (f64, f64) {
        match self {
            Self::Calling => LOW,
            Self::Answering => HIGH,
        }
    }

    fn receive_tones(self) -> (f64, f64) {
        match self {
            Self::Calling => HIGH,
            Self::Answering => LOW,
        }
    }
}

/// The timings of clause 8, in seconds.
pub mod timing {
    /// 8.1.1: "after transmitting no signal for 1 s, the DCE shall initiate
    /// transmission of CI, CT or CNG, or continue transmission of no signal".
    pub const CALL_QUIET: f64 = 1.0;

    /// 8.2: "for a period of at least 0.2 s after connection to line, the
    /// answer DCE shall transmit no signal".
    pub const ANSWER_QUIET: f64 = 0.2;

    /// 8.1.1: the silence between hearing ANSam and sending CM.
    ///
    /// "The minimum value for Te shall be 0.5 s. However, if it is desired to
    /// allow for network echo canceller disabling in the manner defined in
    /// ITU-T V.25, Te shall be set to a value >= 1 s." Taken at a second,
    /// because a call carried over a packet network has met more echo
    /// cancellers than a call over copper ever did.
    pub const TE: f64 = 1.0;

    /// 8.1.2 and 8.2.3: the gap between the end of V.8 and the beginning of
    /// the modulation it chose. "No signal for a period of 75 +/- 5 ms".
    pub const HANDOVER: f64 = 0.075;

    /// 8.2.2: "if not terminated by the receipt of CM or a suitable sigC,
    /// ANSam shall be transmitted for a period of 5 +/- 1 s".
    pub const ANSAM: f64 = 5.0;

    /// How long the calling modem hears ANSam without a break before it
    /// believes it. Not a figure from the Recommendation, which asks only that
    /// ANSam "has been detected" (7.2) and leaves the detector to the modem.
    ///
    /// Long enough to outlast a transient. Note 1 to 7.2 warns of "transient
    /// variations in the received answer-tone amplitude and phase that may be
    /// generated occasionally by network equipment", and to a detector that
    /// reads the modulation off the envelope a transient is a step in the
    /// envelope. A step has some of every frequency in it, 15 Hz included.
    /// The most one step, or one dropout, can put into the detector's 0.4 s
    /// correlator is a depth of 4/(2 pi 15 Hz 0.4 s), about 0.11, which
    /// decays back under the 0.08 that reads as modulated in about a tenth of
    /// a second. The worst case measured on a plain tone, a dropout half a
    /// 15 Hz cycle long so that its two edges add, read as ANSam for 0.15 s.
    /// To last out this hold a transient would have to reach
    /// 0.08 e^(0.25/0.4), about 0.15, which no single one can. It is also
    /// nearly four cycles of the modulation itself.
    ///
    /// Short enough to fit between the reversals and inside ANSam. The
    /// detector does not lose the tone at a phase reversal, but a hold shorter
    /// than the 425 ms 7.2 allows between two of them would be found in the
    /// gap even if it did. And 8.2.2 keeps ANSam up for as little as 4 s:
    /// the quarter of a second the detector takes to settle, this, Te, two CM
    /// sequences of about 0.3 s each, and the second and a half a packet
    /// network takes to carry the tone here and the CM back come to about
    /// 3.7 s of it.
    pub const ANSAM_HELD: f64 = 0.25;

    /// How long a calling modem waits to hear anything at all before giving
    /// up. Not a figure from the Recommendation, which leaves this to the
    /// modem: a number has to come from somewhere and this one is the wait a
    /// person will sit through.
    pub const PATIENCE: f64 = 60.0;
}

/// 8.1.2 and 8.2.2: a menu is believed once it has arrived twice the same.
///
/// "After a minimum of 2 identical JM sequences have been received, the call
/// DCE shall complete the current octet ... and then signal CJ shall be
/// transmitted", and "upon receiving a minimum of 2 identical CM sequences,
/// the DCE shall transmit JM". A minimum of two, and not two in a row: a
/// sequence that arrived with octets missing is one that was not received.
const IDENTICAL: u32 = 2;

/// How many torn sequences in a row are skipped before they count anyway.
///
/// Four of them is a little over a second at 300 bit/s, by which time the line
/// has said plainly that it is going to tear everything and no amount of
/// waiting will produce a whole sequence. `live-1789647424` tore six of the
/// far end's eight JMs but not four running, so a line like that one still
/// settles on the whole sequences it did deliver.
const TORN_IN_A_ROW: u32 = 4;

/// What the procedure has decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Still going.
    Negotiating,
    /// Both ends have a modulation in common and the line is clear for it.
    Agreed(Modulation),
    /// The far end sent the plain answering tone of V.25. It does not do V.8,
    /// and 8.1.1 sends the call on to the modulation's own procedure rather
    /// than negotiating: "if ANS (rather than ANSam) is detected, the DCE
    /// shall proceed in accordance with Annex A/V.32 bis, ITU-T T.30, or other
    /// appropriate Recommendations."
    NoNegotiation,
    /// Nothing in common, or nothing heard at all.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// The silence both ends open with.
    Quiet,
    /// The calling modem, listening for an answering tone.
    Listening,
    /// The calling modem's Te: silence between hearing ANSam and answering it.
    Waiting,
    /// Sending CM, over and over, until JM comes back.
    SendingCm,
    /// Sending CJ, which is three zero octets.
    SendingCj,
    /// The answering modem's ANSam.
    Ansam,
    /// Sending JM until CJ arrives.
    SendingJm,
    /// The 75 ms of nothing before the chosen modulation starts.
    Handover,
    Done(Status),
}

/// One end of a V.8 negotiation.
#[derive(Debug)]
pub struct Modem {
    role: Role,
    /// What this end can do, and what the call is for.
    menu: Menu,
    tx: Bell103Tx,
    rx: Bell103Rx,
    bits: AsyncBits,
    decoder: Decoder,
    /// The calling modem's ear for the answering tone.
    answer: v8::AnswerTone,
    /// Seconds it has said ANSam, and the plain tone of V.25, without a
    /// break.
    ansam_held: f64,
    plain_held: f64,
    /// The answering modem's voice for it.
    tone: Nco,
    modulation: Nco,
    reversals: f64,
    /// Smoothed level of the line, for noticing the far end has stopped.
    level: OnePole,
    state: State,
    /// Seconds in the current state.
    elapsed: f64,
    /// Seconds since the whole thing began.
    total: f64,
    fs: f64,
    /// The last menu heard, the octets it was read from, and how many times
    /// that same sequence has arrived.
    last: Option<Menu>,
    last_octets: Vec<u8>,
    repeats: u32,
    /// Framing errors the receiver has counted, whether one of them fell
    /// inside the sequence being gathered now, and how many sequences running
    /// have been thrown away for it.
    framing_errors: u64,
    torn: bool,
    torn_in_a_row: u32,
    /// What was agreed, once it has been.
    chosen: Option<Modulation>,
    /// The error control both ends named, if they named any (Table 6).
    agreed: Protocol,
    /// The menu the far end sent, which is what it has said about itself.
    ///
    /// From the calling end this is the joint menu of 7.4 and so already the
    /// intersection: what the far end has *and* this end offered. From the
    /// answering end it is the call menu, which is everything the far end has.
    far_menu: Option<Menu>,
    /// Octets of the sequence being sent, and where in it we are.
    outgoing: Vec<u8>,
    /// Zero octets of CJ seen so far (8.2.3 wants all three).
    cj: usize,
    /// The JM this end answered with, to repeat as it was.
    sent_jm: Option<Menu>,
}

impl Modem {
    /// A modem that can do `ours`, for a call of the given function.
    pub fn new(role: Role, function: CallFunction, ours: Modulations, fs: f64) -> Self {
        let (tx_space, tx_mark) = role.transmit_tones();
        let (rx_space, rx_mark) = role.receive_tones();
        let mut tx = Bell103Tx::with_tones(tx_space, tx_mark, fs);
        tx.set_transmitting(false);
        Self {
            role,
            menu: Menu { function, modulations: ours, protocol: Protocol::Unstated, access: None, pcm: None },
            tx,
            rx: Bell103Rx::with_tones(rx_space, rx_mark, fs),
            bits: AsyncBits::new(8),
            decoder: Decoder::new(),
            answer: v8::AnswerTone::new(fs),
            ansam_held: 0.0,
            plain_held: 0.0,
            tone: Nco::new(v8::ansam::ANSWER_TONE, fs),
            modulation: Nco::new(v8::ansam::MODULATION_RATE, fs),
            reversals: 0.0,
            level: OnePole::new(0.100, fs),
            state: State::Quiet,
            elapsed: 0.0,
            total: 0.0,
            fs,
            last: None,
            last_octets: Vec::new(),
            repeats: 0,
            framing_errors: 0,
            torn: false,
            torn_in_a_row: 0,
            chosen: None,
            agreed: Protocol::Unstated,
            far_menu: None,
            outgoing: Vec::new(),
            cj: 0,
            sent_jm: None,
        }
    }

    /// Ask for LAPM in the protocol category (Table 6).
    ///
    /// 7.3: the category "may be included in order to negotiate LAPM without
    /// requiring the ODP/ADP exchange". What this modem does with the answer
    /// is not to skip that exchange -- 7.3 warns in the same breath that "some
    /// existing implementations of V.8 may indicate LAPM in prot0, but still
    /// require the ODP/ADP exchange", and V.42 Appendix VI.2 says many
    /// answering modems run it regardless in order to catch protocols V.8
    /// cannot name. It is worth having anyway: a far end that has said it does
    /// LAPM has said so whether or not its ADP survives the line.
    pub fn offering_lapm(mut self) -> Self {
        self.menu.protocol = Protocol::Lapm;
        self
    }

    /// Offer to be half of a V.90 pair (Table 5).
    ///
    /// 7.3: a call menu carrying the PCM category also carries the PSTN
    /// access category -- this modem is on an analogue line as far as it
    /// knows, which is the claim that commits to least -- and V.34, which
    /// V.90 falls back to.
    pub fn offering_pcm(self, pcm: Pcm) -> Self {
        self.offering_pcm_on(pcm, Access::default())
    }

    /// The same, saying what kind of line this end is on: a V.90 server is
    /// "on a digital network connection".
    pub fn offering_pcm_on(mut self, pcm: Pcm, access: Access) -> Self {
        self.menu.pcm = Some(pcm);
        self.menu.access = Some(access);
        self.menu.modulations.insert(Modulation::V34Duplex);
        self
    }

    /// Which half of a V.90 pair this end is to be, once the menus are
    /// settled: none unless both offered a PCM category and the two make a
    /// pair (9.1.1/V.90).
    pub fn pcm_role(&self) -> Option<PcmRole> {
        let ours = self.menu.pcm?;
        let far = self.far_menu?.pcm?;
        // From the calling end the far menu is the joint one, which carries
        // the PCM category back only for a pair; V.34 has to have been agreed
        // too, since V.90 is V.34 in its other direction.
        if self.chosen != Some(Modulation::V34Duplex) {
            return None;
        }
        Pcm::pair(ours, far, self.role == Role::Calling)
    }

    pub fn status(&self) -> Status {
        match self.state {
            State::Done(s) => s,
            _ => Status::Negotiating,
        }
    }

    /// What the procedure is doing, for a scope to show.
    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Quiet => "quiet",
            State::Listening => "listening for an answer",
            State::Waiting => "Te",
            State::SendingCm => "CM",
            State::SendingCj => "CJ",
            State::Ansam => "ANSam",
            State::SendingJm => "JM",
            State::Handover => "handover",
            State::Done(_) => "done",
        }
    }

    /// The modulation both ends settled on.
    pub fn chosen(&self) -> Option<Modulation> {
        self.chosen
    }

    /// Whether both ends named LAPM in the protocol category.
    pub fn lapm(&self) -> bool {
        self.agreed == Protocol::Lapm
    }

    /// The menu the far end sent, once one has arrived.
    pub fn far_menu(&self) -> Option<Menu> {
        self.far_menu
    }

    /// One sample in, one sample out.
    pub fn step(&mut self, line: f64) -> f64 {
        let dt = 1.0 / self.fs;
        self.elapsed += dt;
        self.total += dt;
        self.level.process(line.abs());

        self.answer.feed(line);
        // What the detector says at one instant is not a decision. Its
        // readings are averages, and an average can be anything for a moment
        // while what it is averaging changes under it: the first moments of a
        // plain tone, the step as a call is picked up and the line goes from
        // noise to digital silence, the last moments of a tone. Every one of
        // those has read as ANSam, or as the plain tone, for a few tens of
        // milliseconds on a real call.
        self.ansam_held = if self.answer.is_ansam() { self.ansam_held + dt } else { 0.0 };
        self.plain_held = if self.answer.is_plain() { self.plain_held + dt } else { 0.0 };
        let octet = self.rx.feed(line);
        // A character whose stop bit was not a mark. Clause 5 runs a sequence
        // octet against octet with nothing between them, so a framing error
        // means the framer spent the rest of that character, and usually the
        // next one or two, hunting for a clean start bit -- and the sequence
        // being gathered now has a hole in it. A packet network's jitter
        // buffer deletes or inserts a couple of bit times every few seconds,
        // which at 300 bit/s is exactly this.
        if self.rx.framing_errors() != self.framing_errors {
            self.framing_errors = self.rx.framing_errors();
            self.torn = true;
        }
        if let Some(octet) = octet {
            self.heard(octet);
        }

        self.advance();
        self.transmit()
    }

    /// An octet came off the line.
    fn heard(&mut self, octet: u8) {
        // The synchronisation octet that ends one sequence begins the next
        // (clause 5), so this is the boundary: what the flag holds belongs to
        // the sequence ending here, and the one starting here begins clean.
        //
        // Taken here rather than where a menu is reported, because not every
        // sequence is reported -- a body that will not parse as a menu, a run
        // of zeros read as CJ, an octet lost out of the synchronisation
        // itself. The flag used to survive all of those and condemn the next
        // sequence for a tear that was not in it.
        let torn = octet == v8::SYNC_MENU && std::mem::take(&mut self.torn);
        let Some(heard) = self.decoder.feed(octet) else { return };
        match heard {
            // The same octets whichever way they came; which channel carried
            // them is what says whether this is a call menu or a joint one,
            // and that is settled by which end we are.
            Heard::Cm(menu) => {
                if torn {
                    self.torn_in_a_row += 1;
                }
                // Octets are missing, so these are not the octets the far end
                // sent and this sequence has not been received. Skipped rather
                // than counted against the run: 8.1.2 and 8.2.2 ask for "a
                // minimum of 2 identical" and not for two in a row, and a
                // sequence nobody received contradicts nothing.
                //
                // Only while there is some prospect of a whole one. A line
                // that tears every sequence would otherwise leave the run at
                // nothing for ever: the answerer's ANSam would run out with no
                // JM sent (8.2.2), the caller would never send CJ (8.1.2), and
                // the call would leave V.8 altogether with no modulation and
                // no protocol agreed -- which is worse than any menu it could
                // have mis-read. So after [`TORN_IN_A_ROW`] of them the
                // requirement that is left is the one the clauses actually
                // state: two sequences identical octet for octet. A tear
                // deletes bit times wherever the line happened to slip, so two
                // of them coming out the same is a far longer shot than the
                // damage this skipping was put in for.
                if torn && self.torn_in_a_row <= TORN_IN_A_ROW {
                    return;
                }
                if !torn {
                    self.torn_in_a_row = 0;
                }
                // Identical means the octets, not the menu they parse to.
                // Clause 6 has a receiver ignore every code and octet reserved
                // for future definition, so an octet damaged into an unknown
                // tag disappears without changing the menu: on a real call two
                // JMs that had each lost their last three octets, one ending
                // `13 a9` and the next `13 10`, compared equal. This end sent
                // CJ on them 0.6 s before the first undamaged JM arrived, and
                // so never read the protocol octet that said LAPM.
                if self.last_octets == self.decoder.sequence() {
                    self.repeats += 1;
                } else {
                    self.last_octets = self.decoder.sequence().to_vec();
                    self.repeats = 1;
                }
                self.last = Some(menu);
            }
            // 8.2.3: JM stops when "all 3 octets of CJ have been received".
            Heard::Cj => self.cj = v8::CJ.len(),
            Heard::Ci(_) => {}
            Heard::Jm(_) => {}
        }
    }

    /// Whether the far end has said the same thing twice, as 7.4 and 8.1.2
    /// both require before it is acted on.
    fn settled(&self) -> Option<Menu> {
        (self.repeats >= IDENTICAL).then_some(self.last).flatten()
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.elapsed = 0.0;
    }

    /// Queue one whole sequence, preamble and all.
    ///
    /// 7.3 and 7.4: "a CM sequence starts with 10 ONEs followed by 10
    /// synchronization bits", and the same for JM. The ONEs are not a
    /// formality and not decoration. They are the idle condition of the line
    /// held for ten bit times, and they are the only thing standing between a
    /// carrier appearing and a start bit arriving.
    ///
    /// Without them the far end's frequency shift keyer is handed a carrier
    /// and the first start bit in the same instant, with its carrier detector
    /// and its gain control both still settling. It misses the octet -- and
    /// the octet it misses is the synchronisation, without which there is no
    /// finding the message. A real answering modem duly sent ANSam for its
    /// four and a half seconds, heard nothing it recognised, and fell back to
    /// V.22bis exactly as 8.2.2 tells it to.
    fn send_sequence(&mut self, octets: Vec<u8>) {
        self.tx.set_transmitting(true);
        self.tx.push_bits(&[true; v8::PREAMBLE_ONES]);
        self.outgoing = octets;
    }

    /// Queue octets with no preamble, for the signals that have none.
    ///
    /// CJ is three octets of zeros on a carrier that is already up and already
    /// being read (3.5). It needs no run-in, and one would only delay it.
    fn send(&mut self, octets: Vec<u8>) {
        self.outgoing = octets;
        self.tx.set_transmitting(true);
    }

    fn advance(&mut self) {
        // Nobody waits for ever. 8 gives no figure for this, so it is ours.
        if self.total > timing::PATIENCE && !matches!(self.state, State::Done(_)) {
            self.enter(State::Done(Status::Failed));
            return;
        }
        match self.state {
            State::Quiet => {
                let quiet = match self.role {
                    Role::Calling => timing::CALL_QUIET,
                    Role::Answering => timing::ANSWER_QUIET,
                };
                if self.elapsed >= quiet {
                    self.enter(match self.role {
                        Role::Calling => State::Listening,
                        // 8.2.2: "if the answer DCE supports CM/JM exchanges,
                        // ANSam shall be transmitted".
                        Role::Answering => State::Ansam,
                    });
                }
            }

            State::Listening => {
                // 8.1.1. ANSam means the far end will negotiate; the plain
                // answering tone of V.25 means it will not, and the call goes
                // on without V.8 rather than failing. Each has to be heard
                // without a break before it is believed: a CM sent to a modem
                // that does not do V.8 is what 7.2 forbids, and giving up on a
                // modem that does throws away everything V.8 could have found.
                if self.ansam_held >= timing::ANSAM_HELD {
                    self.enter(State::Waiting);
                } else if self.plain_held >= timing::TE {
                    // Held for longer: ANSam is a modulated tone, and the
                    // modulation takes a moment to measure. Deciding on the
                    // first instant of a tone would call every ANSam a plain
                    // one.
                    self.enter(State::Done(Status::NoNegotiation));
                }
            }

            State::Waiting => {
                // 8.1.1: silence for Te, "prior to transmitting signal CM".
                if self.elapsed >= timing::TE {
                    let cm = v8::sequence(Signal::Cm, &self.menu);
                    self.send_sequence(cm);
                    self.enter(State::SendingCm);
                }
            }

            State::SendingCm => {
                // 8.1.2: "after a minimum of 2 identical JM sequences have
                // been received... signal CJ shall be transmitted."
                if let Some(jm) = self.settled() {
                    self.far_menu = Some(jm);
                    self.chosen = jm.chosen();
                    // A JM naming LAPM is an answer to the CM that asked, so
                    // by 7.4 it is already the intersection of the two.
                    self.agreed = jm.protocol;
                    // "The call DCE shall complete the current octet and
                    // associated start and stop bits and then signal CJ shall
                    // be transmitted" -- so the queue is drained, not dropped.
                    self.outgoing.clear();
                    self.send(v8::CJ.to_vec());
                    self.enter(State::SendingCj);
                } else if self.outgoing.is_empty() && self.tx.pending_bits() == 0 {
                    // 7.3: "a repetitive sequence". Say it again.
                    let cm = v8::sequence(Signal::Cm, &self.menu);
                    self.send_sequence(cm);
                }
            }

            State::SendingCj => {
                if self.outgoing.is_empty() && self.tx.pending_bits() == 0 {
                    self.tx.set_transmitting(false);
                    self.enter(State::Handover);
                }
            }

            State::Ansam => {
                // 8.2.2: "upon receiving a minimum of 2 identical CM
                // sequences, the DCE shall transmit JM".
                if let Some(cm) = self.settled() {
                    self.far_menu = Some(cm);
                    let jm = match (self.menu.pcm, self.menu.access) {
                        (Some(pcm), access) => {
                            cm.joint_pcm(self.menu.modulations, self.menu.protocol, pcm, access.unwrap_or_default())
                        }
                        _ => cm.joint(self.menu.modulations, self.menu.protocol),
                    };
                    self.sent_jm = Some(jm);
                    self.chosen = jm.chosen();
                    self.agreed = jm.protocol;
                    self.last = None;
                    self.last_octets.clear();
                    self.repeats = 0;
                    let octets = v8::sequence(Signal::Jm, &jm);
                    self.send_sequence(octets);
                    self.enter(State::SendingJm);
                } else if self.elapsed >= timing::ANSAM {
                    // "If neither CM nor a suitable sigC is detected during
                    // ANSam transmission" the call goes on without V.8.
                    self.enter(State::Done(Status::NoNegotiation));
                }
            }

            State::SendingJm => {
                // 8.2.3: "JM transmission shall continue until signal CJ is
                // detected and all 3 octets of CJ have been received", and may
                // be "terminated without any requirement to complete a current
                // JM sequence".
                if self.cj >= v8::CJ.len() {
                    self.outgoing.clear();
                    self.tx.set_transmitting(false);
                    self.enter(State::Handover);
                } else if self.outgoing.is_empty() && self.tx.pending_bits() == 0 {
                    let jm = self.last_jm();
                    self.send_sequence(jm);
                }
            }

            State::Handover => {
                if self.elapsed >= timing::HANDOVER {
                    self.enter(State::Done(match self.chosen {
                        Some(m) => Status::Agreed(m),
                        // 8.1.2 and 8.2.3 both allow disconnecting when the
                        // joint menu is all zeros. There is nothing to fall
                        // back to: the two modems have nothing in common and
                        // have now said so to each other.
                        None => Status::Failed,
                    }));
                }
            }

            State::Done(_) => {}
        }
    }

    /// The JM to repeat, rebuilt from what was agreed.
    fn last_jm(&self) -> Vec<u8> {
        if let Some(jm) = self.sent_jm {
            return v8::sequence(Signal::Jm, &jm);
        }
        let mut modulations = Modulations::NONE;
        if let Some(m) = self.chosen {
            modulations.insert(m);
        }
        v8::sequence(
            Signal::Jm,
            &Menu {
                function: self.menu.function,
                modulations,
                protocol: self.agreed,
                access: None,
                pcm: None,
            },
        )
    }

    fn transmit(&mut self) -> f64 {
        // The answering tone is not framed octets and does not go through the
        // frequency shift keyer at all.
        if self.state == State::Ansam {
            return self.ansam();
        }
        if self.tx.is_transmitting()
            && self.tx.pending_bits() == 0
            && !self.outgoing.is_empty()
        {
            let octet = self.outgoing.remove(0);
            let bits = self.bits.encode(octet);
            self.tx.push_bits(&bits);
        }
        self.tx.next_sample()
    }

    /// One sample of ANSam (7.2).
    fn ansam(&mut self) -> f64 {
        self.reversals += 1.0 / self.fs;
        // "Phase reversals at an interval of 450 +/- 25 ms."
        let flips = (self.reversals / 0.450) as u64;
        let sign = if flips.is_multiple_of(2) { 1.0 } else { -1.0 };
        let (m, _) = self.modulation.step();
        // "The modulated envelope shall range in amplitude between 0.8 and 1.2
        // times its average amplitude."
        let envelope = 1.0 + v8::ansam::NOMINAL_DEPTH * m;
        let (c, _) = self.tone.step();
        ANSAM_LEVEL * sign * envelope * c
    }
}

/// How loudly the answering tone goes out, as a fraction of full scale.
///
/// 7.2 defers the figure to V.2, which is about power delivered to a line
/// rather than about numbers in a computer. This leaves the same headroom the
/// data pumps are given.
const ANSAM_LEVEL: f64 = 0.35;

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    fn all() -> Modulations {
        Modulations::of(&[Modulation::V32bis, Modulation::V22bis, Modulation::V21])
    }

    /// Run the two ends against each other over a clean line.
    fn negotiate(
        ours: Modulations,
        theirs: Modulations,
        seconds: f64,
    ) -> (Status, Status) {
        let mut calling = Modem::new(Role::Calling, CallFunction::Data, ours, FS);
        let mut answering =
            Modem::new(Role::Answering, CallFunction::Data, theirs, FS);
        let (mut to_calling, mut to_answering) = (0.0, 0.0);
        for _ in 0..(seconds * FS) as usize {
            let from_calling = calling.step(to_calling);
            let from_answering = answering.step(to_answering);
            to_calling = from_answering;
            to_answering = from_calling;
        }
        (calling.status(), answering.status())
    }



    /// Run the two ends against each other, each saying whether it does LAPM.
    fn negotiate_protocol(ours: bool, theirs: bool, seconds: f64) -> (bool, bool) {
        let mut calling = Modem::new(Role::Calling, CallFunction::Data, all(), FS);
        if ours {
            calling = calling.offering_lapm();
        }
        let mut answering = Modem::new(Role::Answering, CallFunction::Data, all(), FS);
        if theirs {
            answering = answering.offering_lapm();
        }
        let (mut to_calling, mut to_answering) = (0.0, 0.0);
        for _ in 0..(seconds * FS) as usize {
            let from_calling = calling.step(to_calling);
            let from_answering = answering.step(to_answering);
            to_calling = from_answering;
            to_answering = from_calling;
        }
        (calling.lapm(), answering.lapm())
    }

    /// The JM the far end sent all through the call in `live-1789647424.wav`:
    /// data call function, two modulation octets and an empty extension, LAPM,
    /// PSTN access, and PCM availability with nothing set.
    const WHOLE_MENU: &[u8] = &[0xc1, 0x45, 0x13, 0x10, 0x2a, 0x0d, 0x07];

    /// What the framer recovered of it wherever a slip had eaten the tail.
    const CUT_SHORT: &[u8] = &[0xc1, 0x45, 0x13, 0x10];

    /// Play a series of menus at an answering modem and give back the one it
    /// acted on.
    ///
    /// Each is clause 5's sequence: ten ONEs, Table 1's synchronisation, and
    /// the body. Where `torn`, a character whose stop bit is a space follows
    /// the body -- which is what a jitter buffer that has deleted two or three
    /// bit times leaves behind at 300 bit/s. The octets after the hole never
    /// arrive, and the fault is the one thing the framer records.
    ///
    /// The last menu given is never acted on: nothing ends a sequence but the
    /// synchronisation of the next one, because a menu carries no length.
    fn menus_at_an_answerer(sequences: &[(&[u8], bool)]) -> (Option<Menu>, u64) {
        let mut modem = Modem::new(Role::Answering, CallFunction::Data, all(), FS);
        let framing = AsyncBits::new(8);
        let mut tx = Bell103Tx::with_tones(LOW.0, LOW.1, FS);
        tx.set_transmitting(true);
        for (body, torn) in sequences {
            let mut bits = vec![true; v8::PREAMBLE_ONES];
            for octet in std::iter::once(&v8::SYNC_MENU).chain(*body) {
                bits.extend(framing.encode(*octet));
            }
            if *torn {
                bits.extend([false; 10]);
            }
            tx.push_bits(&bits);
        }
        // 8.2: "for a period of at least 0.2 s after connection to line, the
        // answer DCE shall transmit no signal", and it is listening for a menu
        // only once its ANSam has begun.
        for _ in 0..=(timing::ANSWER_QUIET * FS) as usize {
            modem.step(0.0);
        }
        while tx.pending_bits() > 0 {
            modem.step(tx.next_sample());
        }
        (modem.far_menu(), modem.rx.framing_errors())
    }

    /// Two menus that parse alike are not two identical sequences.
    ///
    /// 8.2.2 acts on "a minimum of 2 identical CM sequences", and clause 5
    /// says a sequence is its octets. Comparing the parsed menus instead takes
    /// two different sequences for one, because clause 6 has a receiver ignore
    /// every code reserved for future definition: `a9` has a tag Table 2 does
    /// not give and vanishes, and `10` is an extension octet with no
    /// modulation bit set. Both leave the same menu.
    #[test]
    fn a_menu_is_believed_on_its_octets_and_not_on_what_they_parse_to() {
        let (heard, torn) = menus_at_an_answerer(&[
            (&[0xc1, 0x45, 0x13, 0xa9], false),
            (CUT_SHORT, false),
            (WHOLE_MENU, false),
            (WHOLE_MENU, false),
            (WHOLE_MENU, false),
        ]);
        assert_eq!(torn, 0, "the line was clean and the framer disagrees");
        let heard = heard.expect("no menu was acted on");
        assert_eq!(heard.protocol, Protocol::Lapm, "{heard:?}");
        assert!(heard.access.is_some(), "{heard:?}");
    }

    /// A menu the line tore is a menu that was not received.
    ///
    /// From `live-1789647424.wav`: six of the far end's eight JM sequences
    /// lost octets to a jitter buffer, and this end sent CJ on a pair of them
    /// 0.6 s before the first whole one arrived -- so the call went on without
    /// ever reading the octet that said LAPM, and the report of it said the
    /// protocol was not stated. 8.1.2 and 8.2.2 ask for "a minimum of 2
    /// identical", not for two in a row, so a sequence with a hole in it is
    /// left out of the count rather than breaking it.
    #[test]
    fn a_menu_the_line_tore_is_not_one_that_was_received() {
        let (heard, torn) = menus_at_an_answerer(&[
            (CUT_SHORT, true),
            (CUT_SHORT, true),
            (WHOLE_MENU, false),
            (WHOLE_MENU, false),
            (WHOLE_MENU, false),
        ]);
        assert!(torn >= 2, "the framer counted {torn} torn characters");
        let heard = heard.expect("no menu was acted on");
        assert_eq!(heard.protocol, Protocol::Lapm, "{heard:?}");
        assert!(heard.access.is_some(), "{heard:?}");
    }

    /// A tear belongs to the sequence it fell in and to no other.
    ///
    /// The flag was cleared only where a menu was reported, and a sequence the
    /// line tore to nothing reports no menu -- so the tear outlived it and
    /// threw away the next sequence too, whole and clean though that one was.
    /// Here the first sequence loses every octet it had, and the two good ones
    /// behind it are what 8.2.2's "minimum of 2 identical" is counted from.
    #[test]
    fn a_tear_belongs_to_the_sequence_it_fell_in() {
        const NOTHING_LEFT: &[u8] = &[];
        let (heard, torn) = menus_at_an_answerer(&[
            (NOTHING_LEFT, true),
            (WHOLE_MENU, false),
            (WHOLE_MENU, false),
            (WHOLE_MENU, false),
        ]);
        assert_eq!(torn, 1, "the framer counted {torn} torn characters");
        let heard = heard.expect("the tear was still held against a whole sequence");
        assert_eq!(heard.protocol, Protocol::Lapm, "{heard:?}");
        assert!(heard.access.is_some(), "{heard:?}");
    }

    /// A line that tears every sequence still gets a menu read off it.
    ///
    /// Skipping a torn sequence has to be bounded, because there is a line on
    /// which every attempt is torn and no number of them adds up to 8.2.2's
    /// "minimum of 2 identical CM sequences". Skipping for ever there means
    /// the answerer's ANSam runs out with no JM sent, the caller never sends
    /// CJ, and the call leaves V.8 with neither a modulation nor a protocol
    /// agreed -- worse than the mis-read menu the skipping was put in for. The
    /// requirement that is left is the one the clause states: two sequences
    /// the same, octet for octet.
    #[test]
    fn a_line_that_tears_every_sequence_still_settles_a_menu() {
        let torn_all = [(WHOLE_MENU, true); 8];
        let (heard, torn) = menus_at_an_answerer(&torn_all);
        assert!(torn >= 7, "the framer counted {torn} torn characters");
        let heard = heard.expect("no menu was acted on, so the call left V.8");
        assert_eq!(heard.protocol, Protocol::Lapm, "{heard:?}");
        assert!(heard.access.is_some(), "{heard:?}");
    }

    #[test]
    fn two_modems_settle_error_control_before_the_line_is_trained() {
        // 7.3: the protocol category "may be included in order to negotiate
        // LAPM without requiring the ODP/ADP exchange". Both ends come out of
        // V.8 knowing, which is a second and quite independent way of learning
        // what the detection phase is there to find out.
        assert_eq!(negotiate_protocol(true, true, 8.0), (true, true));
    }

    #[test]
    fn one_end_asking_for_lapm_settles_nothing() {
        // 7.4 completes the negotiation only when the JM answers a CM that
        // asked. An answering modem that does LAPM has nothing to answer if
        // the call never raised it, and a calling modem that asks a far end
        // which does not do LAPM gets no octet back.
        assert_eq!(negotiate_protocol(true, false, 8.0), (false, false));
        assert_eq!(negotiate_protocol(false, true, 8.0), (false, false));
        assert_eq!(negotiate_protocol(false, false, 8.0), (false, false));
    }

    #[test]
    fn a_sequence_runs_in_on_the_mark_tone_before_it_starts() {
        // 7.3: "a CM sequence starts with 10 ONEs followed by 10
        // synchronization bits". The ONEs are the idle line held for ten bit
        // times, and they are the only thing between a carrier appearing and a
        // start bit arriving. Without them a far end is handed both at once,
        // with its carrier detector and its gain control still settling; the
        // octet it misses is the synchronisation, and after that there is no
        // message to find.
        //
        // A real answering modem, sent a CM with no run-in, held its ANSam for
        // the full four and a half seconds of 8.2.2, heard nothing it
        // recognised, and fell back to V.22bis. On a clean simulated line the
        // negotiation completed anyway, which is why nothing here noticed.
        //
        // So this measures the tone rather than counting bit times: a ONE is
        // the mark, and in V.21's low channel the mark is 980 Hz.
        let mut calling = Modem::new(Role::Calling, CallFunction::Data, all(), FS);
        let mut answering =
            Modem::new(Role::Answering, CallFunction::Data, all(), FS);
        let (mut to_calling, mut to_answering) = (0.0, 0.0);
        let mut run_in: Vec<f64> = Vec::new();
        let wanted = (FS / BAUD * v8::PREAMBLE_ONES as f64) as usize;

        for _ in 0..(9.0 * FS) as usize {
            let from_calling = calling.step(to_calling);
            let from_answering = answering.step(to_answering);
            to_calling = from_answering;
            to_answering = from_calling;
            if !run_in.is_empty() || from_calling.abs() > 1.0e-9 {
                run_in.push(from_calling);
                if run_in.len() >= wanted {
                    break;
                }
            }
        }
        assert_eq!(run_in.len(), wanted, "the calling modem never transmitted");

        // Which of the two tones the run-in is made of.
        let energy = |f: f64| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &x) in run_in.iter().enumerate() {
                let w = std::f64::consts::TAU * f * i as f64 / FS;
                re += x * w.cos();
                im -= x * w.sin();
            }
            re.hypot(im) / run_in.len() as f64
        };
        let (space, mark) = LOW;
        assert!(
            energy(mark) > 4.0 * energy(space),
            "the run-in is not ten bit times of mark: {:.4} at {mark} Hz \
             against {:.4} at {space} Hz",
            energy(mark),
            energy(space)
        );
    }

    #[test]
    fn the_two_channels_are_the_ones_v21_defines() {
        // V.21: "channel No. 1 (FA = 1180 Hz and Fz = 980 Hz); channel No. 2
        // (FA = 1850 Hz and Fz = 1650 Hz)". The calling modem sends in the
        // low channel (3.1, 3.4, 3.5) and the answering modem in the high one
        // (3.6), so each hears the other's.
        assert_eq!(Role::Calling.transmit_tones(), LOW);
        assert_eq!(Role::Calling.receive_tones(), HIGH);
        assert_eq!(Role::Answering.transmit_tones(), HIGH);
        assert_eq!(Role::Answering.receive_tones(), LOW);
    }

    #[test]
    fn two_modems_agree_on_the_fastest_thing_they_share() {
        let (calling, answering) = negotiate(all(), all(), 12.0);
        assert_eq!(calling, Status::Agreed(Modulation::V32bis));
        assert_eq!(answering, Status::Agreed(Modulation::V32bis));
    }

    #[test]
    fn the_answer_is_what_both_ends_have_and_not_what_one_wants() {
        // The whole point. A calling modem that can do V.32bis and an
        // answering modem that cannot must come out at V.22bis, and both must
        // come out at the same place -- which is the part no modem start-up
        // can arrange for itself.
        let ours = Modulations::of(&[Modulation::V32bis, Modulation::V22bis]);
        let theirs = Modulations::of(&[Modulation::V22bis, Modulation::V21]);
        let (calling, answering) = negotiate(ours, theirs, 12.0);
        assert_eq!(calling, Status::Agreed(Modulation::V22bis));
        assert_eq!(answering, Status::Agreed(Modulation::V22bis));
    }

    #[test]
    fn nothing_in_common_is_found_out_rather_than_waited_through() {
        // Without V.8 this is the case that wastes a minute: both ends start
        // their own start-up and neither hears anything it recognises. With
        // it, they say so and stop.
        let (calling, answering) = negotiate(
            Modulations::of(&[Modulation::V32bis]),
            Modulations::of(&[Modulation::V21]),
            14.0,
        );
        assert_eq!(calling, Status::Failed);
        assert_eq!(answering, Status::Failed);
    }

    #[test]
    fn a_far_end_that_only_sends_the_plain_tone_is_not_negotiated_with() {
        // 8.1.1: "if ANS (rather than ANSam) is detected, the DCE shall
        // proceed in accordance with Annex A/V.32 bis, ITU-T T.30, or other
        // appropriate Recommendations". Not a failure -- an older modem, whose
        // call goes on the old way.
        let mut calling = Modem::new(Role::Calling, CallFunction::Data, all(), FS);
        let mut phase = 0.0f64;
        let mut out = 0.0f64;
        for _ in 0..(FS * 6.0) as usize {
            phase += std::f64::consts::TAU * 2100.0 / FS;
            let sample = calling.step(0.35 * phase.sin());
            out = out.max(sample.abs());
        }
        assert_eq!(calling.status(), Status::NoNegotiation);
        assert!(out < 1.0e-6, "answered a modem that cannot hear it");
    }

    /// One sample of an answering tone `t` seconds into it: ANSam at 7.2's
    /// depth, or the plain tone of V.25.
    fn answering_tone(ansam: bool, reversal_s: f64, level: f64, t: f64) -> f64 {
        let flips = if reversal_s > 0.0 { (t / reversal_s) as u64 } else { 0 };
        let sign = if flips.is_multiple_of(2) { 1.0 } else { -1.0 };
        let depth = if ansam { v8::ansam::NOMINAL_DEPTH } else { 0.0 };
        let envelope = 1.0 + depth * (std::f64::consts::TAU * v8::ansam::MODULATION_RATE * t).sin();
        level * envelope * sign * (std::f64::consts::TAU * v8::ansam::ANSWER_TONE * t).sin()
    }

    /// The gain of a tone that stops at `stop`, cut off or faded out over
    /// `fade` seconds.
    fn ending(t: f64, stop: f64, fade: f64) -> f64 {
        if t < stop {
            1.0
        } else if fade > 0.0 {
            (1.0 - (t - stop) / fade).max(0.0)
        } else {
            0.0
        }
    }

    /// Flat, deterministic noise between -1 and 1.
    fn noise() -> impl FnMut() -> f64 {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    /// What a calling modem made of a line: when it stopped listening and
    /// what it went on to, and the loudest thing it sent.
    struct Verdict {
        left: Option<(f64, State)>,
        loudest: f64,
    }

    /// Play `line`, a function of the time since the call was connected, at a
    /// calling modem.
    fn heard_by_a_caller(seconds: f64, mut line: impl FnMut(f64) -> f64) -> Verdict {
        let mut calling = Modem::new(Role::Calling, CallFunction::Data, all(), FS);
        let mut heard = Verdict { left: None, loudest: 0.0 };
        for i in 0..(seconds * FS) as usize {
            let t = i as f64 / FS;
            heard.loudest = heard.loudest.max(calling.step(line(t)).abs());
            if heard.left.is_none() && !matches!(calling.state, State::Quiet | State::Listening) {
                heard.left = Some((t, calling.state));
            }
        }
        heard
    }

    #[test]
    fn a_noisy_line_that_falls_silent_has_not_answered() {
        // `live-1790039606`: line noise until the far end picked up, and then
        // the exact zeros of a digital network with nothing on it yet. The
        // detector's averages decayed at their own rates, passed for a plain
        // answering tone for 40 ms, and a single instant of that was taken
        // for a far end that does not do V.8 -- two seconds before the far
        // end sent its tone.
        let mut hiss = noise();
        let heard = heard_by_a_caller(6.0, |t| if t < 3.0 { 0.05 * hiss() } else { 0.0 });
        assert!(heard.left.is_none(), "decided on silence: {:?}", heard.left);
        assert!(heard.loudest < 1.0e-6, "transmitted into a silent line");
    }

    #[test]
    fn the_end_of_a_plain_tone_is_taken_for_nothing() {
        // A calling modem that begins listening late in a plain answering
        // tone -- a handshake started again, a line connected late -- hears
        // too little of it to call it plain, and then hears it stop. The step
        // down to nothing read as modulation, and a single instant of it sent
        // CM to a modem that had just said it has never heard of V.8; where
        // it did not, the tone outlived itself on the silence and was called
        // plain, on nothing at all, once the modem had listened for Te.
        for fade in [0.0, 0.050] {
            for reversal in [0.0, 0.450] {
                let heard = heard_by_a_caller(5.0, |t| {
                    ending(t, 1.1, fade) * answering_tone(false, reversal, 0.3, t)
                });
                assert!(
                    heard.left.is_none(),
                    "a plain tone faded over {fade} s ended in {:?}",
                    heard.left
                );
                assert!(heard.loudest < 1.0e-6, "sent CM to a modem that cannot hear it");
            }
        }
    }

    #[test]
    fn a_plain_tone_is_believed_only_once_it_has_been_held() {
        // `live-1789614742`: a plain answering tone whose first 16 ms read as
        // modulated, because the envelope stepping up from nothing is a step
        // and a step has 15 Hz in it. Deciding on the first instant, the modem
        // would have answered it with CM; and where the first instant happens
        // to read as plain instead, it has decided on a tone that could as
        // well have been the first instant of ANSam. Which of the two it reads
        // as depends on what else is on the line while the detector settles,
        // so the tone is played over a telephone line's noise and started at
        // eight points across a cycle of the modulation.
        for k in 0..8 {
            let start = 2.0 + f64::from(k) / 8.0 / v8::ansam::MODULATION_RATE;
            for reversal in [0.0, 0.450] {
                let mut hiss = noise();
                let heard = heard_by_a_caller(5.0, |t| {
                    let tone =
                        if t < start { 0.0 } else { answering_tone(false, reversal, 0.3, t - start) };
                    tone + 0.07 * hiss()
                });
                let (when, state) = heard.left.expect("never decided");
                assert_eq!(
                    state,
                    State::Done(Status::NoNegotiation),
                    "a plain tone from {start:.4} s, at {when:.3} s"
                );
                assert!(
                    (start + timing::TE..start + timing::TE + 0.5).contains(&when),
                    "a plain tone from {start:.4} s decided at {when:.3} s"
                );
                assert!(heard.loudest < 1.0e-6, "sent CM to a modem that cannot hear it");
            }
        }
    }

    #[test]
    fn a_plain_tone_with_a_gap_in_it_is_decided_a_second_after_the_gap() {
        // `live-1790039606` again: 135 ms of exact zeros in the far end's
        // answering tone, a second after it began, which is a packet network
        // and not the far end. A tone that has stopped has stopped. Held
        // without a break means held since it came back.
        let (start, gap) = (2.0, (2.95, 3.085));
        for reversal in [0.0, 0.450] {
            let heard = heard_by_a_caller(6.0, |t| {
                if t < start || (gap.0..gap.1).contains(&t) {
                    0.0
                } else {
                    answering_tone(false, reversal, 0.3, t - start)
                }
            });
            let (when, state) = heard.left.expect("never decided");
            assert_eq!(state, State::Done(Status::NoNegotiation), "at {when:.3} s");
            assert!(
                (gap.1 + timing::TE..gap.1 + timing::TE + 0.1).contains(&when),
                "decided at {when:.3} s"
            );
            assert!(heard.loudest < 1.0e-6, "sent CM to a modem that cannot hear it");
        }
    }

    #[test]
    fn ansam_is_still_found_at_every_level_a_network_delivers() {
        // Everything above makes the calling modem slower to believe what it
        // hears. None of it can be allowed to cost the real thing: ANSam with
        // its reversals and without, from a short call and a long one, is
        // believed a hold after the detector settles, and answered a Te after
        // that.
        let start = 2.0;
        for db in [0.0, -10.0, -20.0, -30.0] {
            let level = 0.3 * 10.0f64.powf(db / 20.0);
            for reversal in [0.0, 0.450] {
                let heard = heard_by_a_caller(4.0, |t| {
                    if t < start { 0.0 } else { answering_tone(true, reversal, level, t - start) }
                });
                let (when, state) = heard.left.expect("ANSam never believed");
                assert_eq!(state, State::Waiting, "ANSam at {db} dB, at {when:.3} s");
                assert!(
                    (start + timing::ANSAM_HELD..start + timing::ANSAM_HELD + 0.4).contains(&when),
                    "ANSam at {db} dB believed at {when:.3} s"
                );
                assert!(heard.loudest > 0.1, "never sent CM to ANSam at {db} dB");
            }
        }
    }

    #[test]
    fn a_calling_modem_says_nothing_until_it_is_answered() {
        // 8.1.1 opens with a second of silence, and 7.2 forbids a CM before
        // ANSam has been detected. A modem that talked into the answering tone
        // would be doing the very thing this exists to stop.
        let mut calling = Modem::new(Role::Calling, CallFunction::Data, all(), FS);
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 3.0) as usize {
            loudest = loudest.max(calling.step(0.0).abs());
        }
        assert!(loudest < 1.0e-6, "transmitted into a silent line");
    }

    #[test]
    fn an_answering_modem_waits_before_it_speaks() {
        // 8.2: "for a period of at least 0.2 s after connection to line, the
        // answer DCE shall transmit no signal".
        let mut answering =
            Modem::new(Role::Answering, CallFunction::Data, all(), FS);
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 0.19) as usize {
            loudest = loudest.max(answering.step(0.0).abs());
        }
        assert!(loudest < 1.0e-6, "spoke before the line had settled");
        for _ in 0..(FS * 0.3) as usize {
            loudest = loudest.max(answering.step(0.0).abs());
        }
        assert!(loudest > 0.1, "never sent an answering tone");
    }

    #[test]
    fn what_the_answering_modem_sends_is_ansam_and_not_ans() {
        // The tone has to carry its modulation, or every calling modem will
        // read it as V.25's and refuse to negotiate -- which is the failure
        // this end would then be causing rather than suffering.
        let mut answering =
            Modem::new(Role::Answering, CallFunction::Data, all(), FS);
        let mut ear = v8::AnswerTone::new(FS);
        for _ in 0..(FS * 4.0) as usize {
            ear.feed(answering.step(0.0));
        }
        assert!(ear.present(), "no answering tone at all");
        assert!(
            ear.is_ansam(),
            "sent a plain answering tone, depth {:.3}",
            ear.depth()
        );
    }
}
