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
use v8::{CallFunction, Decoder, Heard, Menu, Modulation, Modulations, Signal};

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

    /// How long a calling modem waits to hear anything at all before giving
    /// up. Not a figure from the Recommendation, which leaves this to the
    /// modem: a number has to come from somewhere and this one is the wait a
    /// person will sit through.
    pub const PATIENCE: f64 = 60.0;
}

/// 7.3 and 7.4: a menu is believed once it has arrived twice the same.
const IDENTICAL: u32 = 2;

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
    /// The last menu heard, and how many times running it has arrived the same.
    last: Option<Menu>,
    repeats: u32,
    /// What was agreed, once it has been.
    chosen: Option<Modulation>,
    /// Octets of the sequence being sent, and where in it we are.
    outgoing: Vec<u8>,
    /// Zero octets of CJ seen so far (8.2.3 wants all three).
    cj: usize,
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
            menu: Menu { function, modulations: ours },
            tx,
            rx: Bell103Rx::with_tones(rx_space, rx_mark, fs),
            bits: AsyncBits::new(8),
            decoder: Decoder::new(),
            answer: v8::AnswerTone::new(fs),
            tone: Nco::new(v8::ansam::ANSWER_TONE, fs),
            modulation: Nco::new(v8::ansam::MODULATION_RATE, fs),
            reversals: 0.0,
            level: OnePole::new(0.100, fs),
            state: State::Quiet,
            elapsed: 0.0,
            total: 0.0,
            fs,
            last: None,
            repeats: 0,
            chosen: None,
            outgoing: Vec::new(),
            cj: 0,
        }
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

    /// One sample in, one sample out.
    pub fn step(&mut self, line: f64) -> f64 {
        let dt = 1.0 / self.fs;
        self.elapsed += dt;
        self.total += dt;
        self.level.process(line.abs());

        self.answer.feed(line);
        if let Some(octet) = self.rx.feed(line) {
            self.heard(octet);
        }

        self.advance();
        self.transmit()
    }

    /// An octet came off the line.
    fn heard(&mut self, octet: u8) {
        let Some(heard) = self.decoder.feed(octet) else { return };
        match heard {
            // The same octets whichever way they came; which channel carried
            // them is what says whether this is a call menu or a joint one,
            // and that is settled by which end we are.
            Heard::Cm(menu) => {
                if self.last == Some(menu) {
                    self.repeats += 1;
                } else {
                    self.last = Some(menu);
                    self.repeats = 1;
                }
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

    /// Queue a sequence for transmission, and start the carrier.
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
                // on without V.8 rather than failing.
                if self.answer.is_ansam() {
                    self.enter(State::Waiting);
                } else if self.answer.is_plain()
                    && self.elapsed > timing::TE
                {
                    // Held for a while before believing it: ANSam is a
                    // modulated tone, and the modulation takes a moment to
                    // measure. Deciding on the first instant of a tone would
                    // call every ANSam a plain one.
                    self.enter(State::Done(Status::NoNegotiation));
                }
            }

            State::Waiting => {
                // 8.1.1: silence for Te, "prior to transmitting signal CM".
                if self.elapsed >= timing::TE {
                    let cm = v8::sequence(Signal::Cm, &self.menu);
                    self.send(cm);
                    self.enter(State::SendingCm);
                }
            }

            State::SendingCm => {
                // 8.1.2: "after a minimum of 2 identical JM sequences have
                // been received... signal CJ shall be transmitted."
                if let Some(jm) = self.settled() {
                    self.chosen = jm.chosen();
                    // "The call DCE shall complete the current octet and
                    // associated start and stop bits and then signal CJ shall
                    // be transmitted" -- so the queue is drained, not dropped.
                    self.outgoing.clear();
                    self.send(v8::CJ.to_vec());
                    self.enter(State::SendingCj);
                } else if self.outgoing.is_empty() && self.tx.pending_bits() == 0 {
                    // 7.3: "a repetitive sequence". Say it again.
                    let cm = v8::sequence(Signal::Cm, &self.menu);
                    self.send(cm);
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
                    let jm = cm.joint(self.menu.modulations);
                    self.chosen = jm.chosen();
                    self.last = None;
                    self.repeats = 0;
                    let octets = v8::sequence(Signal::Jm, &jm);
                    self.send(octets);
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
                    self.send(jm);
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
        let mut modulations = Modulations::NONE;
        if let Some(m) = self.chosen {
            modulations.insert(m);
        }
        v8::sequence(
            Signal::Jm,
            &Menu { function: self.menu.function, modulations },
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
