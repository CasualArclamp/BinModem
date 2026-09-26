//! T.30 Annex F: a fax call on V.34's half-duplex modem.
//!
//! Once V.8 has agreed V.34 half-duplex a call has two channels, and uses
//! them in turn. The control channel is full duplex, 1200 bit/s both ways at
//! once, and every T.30 frame goes over it; both ends keep it busy with flags
//! whenever they have nothing to say (F.3.1.2, F.3.1.4). The primary channel
//! goes one way, source to recipient, at whatever rate the modem's own MPh
//! exchange settled, and carries the page as T.4 Annex A's frames and RCP and
//! nothing else (F.3.1.3). Error correction mode is not a choice (F.3).
//!
//! What is left of clause 5 is the frames. The DIS and the DCS as before, but
//! the DCS answered with CFR at once -- there is no training check and no FTT
//! (F.3.2.1) -- and then Annex A's partial pages, PPS and PPR, MCF, EOR and
//! ERR, all as they were. What is new is how the line turns round for a page,
//! which is most of this module: the recipient sends flags until it hears
//! forty ones in a row, and then goes silent (F.3.2.2, F.3.4.4); the source
//! sends those ones until it hears the recipient fall silent, or its flags
//! stop, and at least forty have gone (F.3.2.3, F.3.4.5). Everything under
//! that -- the control channel's turn-off, the 70 ms either side, the primary
//! channel's resynchronisation, and the way back -- is the modem's.

use super::{
    Call, EcmCommand, FAST_CARRIER_GONE, FAST_CARRIER_SETTLED, Line, Phase, Role, T1_SECONDS,
    T2_SECONDS, T5_SECONDS,
};
use crate::frames::{Reader, Sender};
use crate::t30;

/// "A string of at least 40 consecutive 1s" (F.3.2.2, F.3.2.3).
///
/// Unmistakable either way round: HDLC stuffs a zero after any five ones
/// inside a frame and a flag has six, so nothing but a source turning the line
/// round puts forty in a row on the control channel.
pub const ONES: usize = 40;

/// How long the recipient's flags have to have stopped before the source takes
/// it that they have: F.3.2.3's "absence of flags".
///
/// A flag goes by every 6.7 ms at 1200 bit/s, so this is fifteen of them
/// missing -- more than a packet network spoils concealing a lost packet --
/// and nothing beside the second and a half of ones a long line takes before
/// the flags stop at all. Taking it too soon costs only the rule's margin:
/// forty ones have gone by then, and the recipient falls silent on hearing
/// them whether this end waited or not.
pub const FLAGS_GONE: f64 = 0.1;

/// How long a frame from the far end may take to finish arriving once its
/// start has been heard, during which no timer of this end's runs out.
///
/// The channel carries both ends at once, so nothing is talked over; but a
/// response whose address and control field are on the line is not a
/// response that failed to come, and a command sent again under it costs a
/// round trip for nothing -- the response is ignored, since the command is
/// going out again, and the far end has to answer the command again. The
/// longest T.30 frame is a PPR, 38 octets from opening flag to closing flag
/// with its 256-bit map, a quarter of a second at 1200 bit/s; this is that
/// with a margin. A frame that never ends -- its check failed, or the far
/// end cut it off -- holds a timer this long and no longer.
pub const FRAME_SECONDS: f64 = 0.35;

/// The primary channel's slowest and fastest data rates: "2400 bit/s to
/// 33 600 bit/s in multiples of 2400 bit/s" (5.1/V.34).
pub const PRIMARY_SLOWEST: u32 = 2400;
pub const PRIMARY_FASTEST: u32 = 33_600;

/// The flag, 0111 1110.
const FLAG: u8 = 0x7E;

/// The far end's control channel, bit by bit: what Annex F needs noticed in it
/// besides its frames.
#[derive(Debug)]
pub(super) struct FarEnd {
    /// The last eight bits heard, the newest in the top bit.
    last: u8,
    /// Ones heard in a row, for F.3.2.2's forty.
    pub(super) ones: usize,
    /// Seconds since the last flag, for F.3.2.3's "absence of flags".
    pub(super) since_flag: f64,
    /// Seconds since the last start of a frame, for [`mid_frame`]: never, to
    /// begin with.
    ///
    /// [`mid_frame`]: Self::mid_frame
    pub(super) since_start: f64,
    /// What has followed the last flag, with its stuffed zeros taken out,
    /// while it may yet be the start of a frame: the bits so far, how many,
    /// and the ones in a row among them.
    head: u16,
    got: u32,
    run: u32,
    looking: bool,
}

impl Default for FarEnd {
    fn default() -> Self {
        Self {
            last: 0,
            ones: 0,
            since_flag: 0.0,
            since_start: f64::INFINITY,
            head: 0,
            got: 0,
            run: 0,
            looking: false,
        }
    }
}

impl FarEnd {
    /// Whether a frame of the far end's is part way here: its start heard
    /// within [`FRAME_SECONDS`], and its end not yet.
    pub(super) fn mid_frame(&self) -> bool {
        self.since_start < FRAME_SECONDS
    }

    /// Hear one bit, and say whether it has just completed the address and
    /// control field of a T.30 frame (5.3.6.1): the start of a frame, for T2
    /// (F.3.2.3 Note 2).
    ///
    /// Two octets in rather than one, because noise is full of flags -- one
    /// bit in 256 starts one -- and of whatever octet follows them, while an
    /// address of 0xFF and a control field of 0x03 or 0x13 after a flag is
    /// sixteen bits noise gets right once in 65 536 tries.
    pub(super) fn hear(&mut self, bit: bool) -> bool {
        self.last = self.last >> 1 | u8::from(bit) << 7;
        self.ones = if bit { self.ones + 1 } else { 0 };
        if self.last == FLAG {
            self.since_flag = 0.0;
            self.looking = true;
            (self.head, self.got, self.run) = (0, 0, 0);
            return false;
        }
        if !self.looking {
            return false;
        }
        if self.run == 5 {
            self.run = 0;
            if bit {
                // A sixth one: a flag on its way, or an abort. Not a frame.
                self.looking = false;
            }
            // Otherwise the zero HDLC put in after five ones, which is not
            // the frame's.
            return false;
        }
        self.run = if bit { self.run + 1 } else { 0 };
        self.head |= u16::from(bit) << self.got;
        self.got += 1;
        if self.got < 16 {
            return false;
        }
        self.looking = false;
        let [address, control] = self.head.to_le_bytes();
        let started =
            address == t30::ADDRESS && matches!(control, t30::CONTROL_MORE | t30::CONTROL_FINAL);
        if started {
            self.since_start = 0.0;
        }
        started
    }
}

impl Call {
    /// V.8 has agreed V.34 half-duplex, so from here the call is T.30 Annex
    /// F's (6.1.5, and Figure 11's "Duplex? No").
    ///
    /// For the join to call once, as soon as V.8 has settled, whichever end
    /// this is. Both ends then begin phase B as though the control channel
    /// were already up -- the end that answered with its CSI and DIS, the end
    /// that dialled waiting for them -- and what they queue waits in
    /// [`next_control_bit`](Self::next_control_bit) until the modem has
    /// finished its start-up (phases 2 to 4 of 12/V.34) and takes it. Whatever
    /// clause 5 had going out is dropped. T1 starts again from here, which
    /// F.3.2.3 Note 1 allows "in order to conform with operation of Annex D",
    /// and without which the start-up would come out of it.
    ///
    /// Error correction mode is on from here whatever
    /// [`set_error_correction`](Self::set_error_correction) said: it "is
    /// mandatory for all facsimile messages using the V.34 modulation system"
    /// (F.3). The primary rate is taken to be 33 600 until
    /// [`set_primary_rate`](Self::set_primary_rate) says what the MPh exchange
    /// made it.
    pub fn start_annex_f(&mut self) {
        self.v34 = true;
        self.ecm = true;
        self.rate = PRIMARY_FASTEST;
        self.sender = Sender::new();
        self.reader = Reader::new();
        self.far = FarEnd::default();
        self.far_silent = false;
        self.burst.clear();
        self.burst_started = false;
        self.new_rate = false;
        self.renegotiated = false;
        self.pause = 0.0;
        self.after_pause = None;
        self.attempts = 0;
        self.held = 0.0;
        self.phase_b_since = self.elapsed;
        match self.role {
            Role::Caller => self.enter(Phase::Listening),
            Role::Answerer => self.enter(Phase::Identifying),
        }
    }

    /// Whether this call is in T.30 Annex F, on V.34's half-duplex modem.
    pub fn annex_f(&self) -> bool {
        self.v34
    }

    /// Whether the modem hears the far end fallen silent on the control
    /// channel: its carrier gone, after the turn-off of 12.6.3/V.34.
    ///
    /// Which is how the source knows the recipient is ready for the page
    /// (F.3.2.3, F.3.4.5). The procedure watches the far end's flags itself as
    /// well, and their stopping will do if this is never said.
    ///
    /// Said as a level or as an edge, either will do: the procedure forgets
    /// it as the line begins to turn round and whenever the control channel
    /// comes back, so a carrier detector that dropped out for a moment before
    /// the response is never taken for the recipient falling silent after it.
    pub fn set_far_silent(&mut self, silent: bool) {
        self.far_silent = silent;
    }

    /// The primary channel's data rate, as the modem's MPh exchange settled it
    /// (12.4/V.34), whenever it does: for the 200 ms of flags that open every
    /// page (A.3.1/T.4), and for anyone wanting to know. Only once the call is
    /// in Annex F; before that, and in clause 5, the rate is T.30's own.
    pub fn set_primary_rate(&mut self, bits_per_second: u32) {
        if self.v34 {
            self.rate = bits_per_second;
        }
    }

    /// Whether the procedure wants a new primary rate at the next start of the
    /// control channel.
    ///
    /// F.3.4.1: after a page "the source terminal shall ... initiate either the
    /// control channel resynchronization procedure or, if a data rate change is
    /// desired, the control channel start-up procedure" -- 12.6/V.34, or 12.4
    /// with its MPh exchange. The join asks the modem for the second while this
    /// says so as it turns the line round from [`Line::V34Primary`].
    ///
    /// Only ever the source's, after a block has been asked for a fourth time
    /// (see [`fourth_ppr`](Self::fourth_ppr)); it holds from then until the
    /// control channel is back after the next page, which is the turnaround it
    /// is for. What rate comes of it is the modem's, and comes back through
    /// [`set_primary_rate`](Self::set_primary_rate).
    pub fn renegotiate(&self) -> bool {
        self.v34 && self.new_rate
    }

    /// The modem has just brought the control channel back: after a start-up,
    /// a resynchronisation or a retrain (12.4, 12.6, 12.8/V.34).
    ///
    /// Anything part way through a frame either side of that is not a frame.
    /// What had arrived of one is forgotten, and a burst of this end's that
    /// the restart cut into goes again from its start, so that F.3.1.4's two
    /// flags are in front of its first frame as after every other restart. A
    /// burst still waiting has them already, and nothing is done to it.
    pub fn control_restarted(&mut self) {
        if !self.v34 {
            return;
        }
        self.reader = Reader::new();
        self.far = FarEnd::default();
        // Up at both ends, so not silent at either.
        self.far_silent = false;
        if self.burst_started && !self.sender.is_empty() {
            let burst = std::mem::take(&mut self.burst);
            self.sender = Sender::new();
            self.queue(&burst);
        } else {
            self.sender.drop_flag();
        }
    }

    /// What the line should be doing, in an Annex F call.
    pub(super) fn annex_f_line(&self) -> Line {
        match self.phase {
            Phase::Done | Phase::Failed => Line::Quiet,
            Phase::Sending => Line::V34Primary,
            Phase::Receiving => Line::V34PrimaryListen,
            Phase::TurningAround if self.role == Role::Caller => Line::V34Ones,
            Phase::Commanding
            | Phase::Identifying
            | Phase::Confirming
            | Phase::Acknowledging
            | Phase::EndingPage
            | Phase::Ending => Line::V34Control,
            _ => Line::V34Listen,
        }
    }

    /// The next bit for the control channel, in an Annex F call.
    pub(super) fn next_annex_f_bit(&mut self) -> Option<bool> {
        let quiet = self.sender.is_empty() && !self.sender.mid_flag();
        match self.phase {
            Phase::Done | Phase::Failed => None,
            // The disconnect has gone and nothing follows it: F.3.4.5 Note 2
            // lets a terminal "disconnect the line immediately after sending
            // DCN without sending consecutive 1s", Figure F.5-3's Note.
            Phase::Ending if quiet => None,
            Phase::TurningAround if self.role == Role::Caller && quiet => {
                self.ones_sent += 1;
                Some(true)
            }
            _ => {
                if !self.sender.is_empty() && !self.sender.mid_flag() {
                    self.burst_started = true;
                }
                Some(self.sender.next_bit_or_flag())
            }
        }
    }

    /// A bit off the control channel, before the frame reader has it.
    pub(super) fn hear_annex_f(&mut self, bit: bool) {
        // F.3.2.3 Note 2: "T2 timer shall be reset at the start of each new
        // frame instead of the detection of flags". Flags never stop on this
        // channel, so a clock they put back would never run out -- and no
        // Figure could show "T2 elapsed" between an MCF and a DIS (F.5-4).
        let started = self.far.hear(bit);
        if started
            && matches!(
                self.phase,
                Phase::AwaitingCommand | Phase::AwaitingPostMessage | Phase::TurningAround
            )
        {
            self.timer = T2_SECONDS;
        }
    }

    /// Both ends' counts start again as the line begins to turn round.
    ///
    /// And the far end is not silent yet, whatever the modem said of it
    /// before the response that has just let the page go: it was sending
    /// that response, and a carrier detector that dropped out for a moment is
    /// not a recipient ready for the page.
    pub(super) fn begin_turning_around(&mut self) {
        self.ones_sent = 0;
        self.far.ones = 0;
        self.far.since_flag = 0.0;
        self.far_silent = false;
    }

    /// One sample of time passing, in an Annex F call.
    pub(super) fn tick_annex_f(&mut self, idle: bool) {
        self.timer -= self.step;
        self.far.since_flag += self.step;
        self.far.since_start += self.step;
        match self.phase {
            Phase::Calling | Phase::Listening => {
                if self.elapsed - self.phase_b_since > T1_SECONDS {
                    self.bow_out("the far end never said what it can do");
                }
            }
            // A burst is over once its last bit is in the modem's hands. The
            // flags after it are the channel's rather than the burst's, and a
            // modem that always has flags to send is never idle.
            Phase::Commanding
            | Phase::Identifying
            | Phase::Confirming
            | Phase::Acknowledging
            | Phase::EndingPage => {
                if self.sender.is_empty() {
                    self.control_burst_ended();
                }
            }
            // Except the last: the disconnect has to have left the line before
            // the line goes.
            Phase::Ending => {
                if self.sender.is_empty() && idle {
                    self.phase = Phase::Done;
                }
            }
            Phase::TurningAround => self.turning_around(),
            Phase::Sending => {
                if self.fast_at >= self.fast_out.len() && idle {
                    self.fast_burst_ended();
                }
            }
            Phase::Receiving => self.hearing_the_page(),
            // No holding a timeout open while the far end talks, as clause 5
            // must on a line only one end can use at once: this channel
            // carries both, and the far end's flags are always on it. Only
            // while a frame of its is part way here, which is the response on
            // its way (FRAME_SECONDS).
            Phase::AwaitingConfirm
            | Phase::AwaitingReceipt
            | Phase::AwaitingCommand
            | Phase::AwaitingPostMessage
            | Phase::AwaitingDisconnect => {
                if self.timer <= 0.0 && !self.far.mid_frame() {
                    self.timed_out();
                }
            }
            // Clause 5's alone, and never here.
            Phase::Answering | Phase::Training | Phase::CheckingTraining => {}
            Phase::Done | Phase::Failed => {}
        }
    }

    /// The line turning round for a page, from either end.
    fn turning_around(&mut self) {
        match self.role {
            // F.3.2.3 and F.3.4.5: "consecutive 1s until silence (or absence
            // of flags) is detected from the recipient terminal and at least
            // 40 1s have been sent". Then the page, whose 70 ms of silence and
            // resynchronisation are the modem's.
            Role::Caller => {
                let quiet = self.far_silent || self.far.since_flag > FLAGS_GONE;
                if self.ones_sent >= ONES && quiet {
                    self.enter(Phase::Sending);
                } else if self.timer <= 0.0 {
                    self.bow_out("the far end never fell silent for the page");
                }
            }
            // F.3.2.2 and F.3.4.4: "flags until a string of at least 40
            // consecutive 1s is detected and then ... silence".
            Role::Answerer => {
                if self.far.ones >= ONES {
                    self.enter(Phase::Receiving);
                } else if self.timer <= 0.0 {
                    self.bow_out("the page never came");
                }
            }
        }
    }

    /// The recipient silent, and the page arriving on the primary channel.
    fn hearing_the_page(&mut self) {
        if self.fast_carrier {
            self.fast_up += self.step;
            self.fast_down = 0.0;
            if self.fast_up > FAST_CARRIER_SETTLED {
                self.fast_seen = true;
            }
        } else {
            self.fast_down += self.step;
        }
        // Over at the first RCP -- one "decoded correctly" is enough to start
        // on the post-message command (5.3.2.1 Note) -- or when the primary
        // channel has gone without one, since the post-message command will
        // say what came and the modem is off back to the control channel for
        // it either way (F.3.4.2).
        if self.collector.ended() || (self.fast_seen && self.fast_down > FAST_CARRIER_GONE) {
            self.page_ended();
        } else if self.timer <= 0.0 {
            if self.fast_carrier && self.held < T5_SECONDS {
                self.held += self.step;
            } else {
                self.held = 0.0;
                self.timed_out();
            }
        }
    }

    /// A.1.3's fourth PPR for one block, under Annex F.
    ///
    /// A.1.3 offers two ways on from there: EOR, giving up on what is still
    /// missing, or CTC, correcting on at a slower speed. F.3.4.5 Note 1 takes
    /// CTC away -- "CTR/CTC frames shall not be used in V.34 ECM protocol and
    /// EOR/ERR or DCN signals are used to transit" -- and says where a slower
    /// speed comes from instead: "Data rate change is possible at every start
    /// of the control channel according to the procedures in F.3.4.1 and
    /// F.3.4.2", the modem's own MPh exchange.
    ///
    /// So the first time a block has its fourth PPR, the frames go again as
    /// asked, a new rate is asked for at the start of the control channel that
    /// follows them (F.3.4.1), and the count starts again: four more tries,
    /// all but the first at whatever rate the modem settles. The second time,
    /// or already at 2400, the end of retransmission, which A.1.3 then leaves
    /// as the only way on. DCN is the other thing the Note names, and is what
    /// a far end that will not answer the EOR gets, as it would any command.
    pub(super) fn fourth_ppr(&mut self) {
        if !self.renegotiated && self.rate > PRIMARY_SLOWEST {
            self.renegotiated = true;
            self.new_rate = true;
            self.ecm_pprs = 0;
            self.on_to_the_page();
        } else {
            self.ecm_command = EcmCommand::Eor;
            self.pause_then(Phase::EndingPage);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::coding::Coding;
    use crate::frames::{Message, V34_FLAGS};
    use crate::page::{Page, Resolution};
    use crate::t30::{Command, Frame, Modulation};

    const FS: f64 = 8000.0;

    fn answering() -> Call {
        let mut call = Call::answer(FS, "61388880000");
        call.start_annex_f();
        call
    }

    fn calling(page: Option<Page>) -> Call {
        let mut call = Call::originate(FS, "61399990000", page);
        call.start_annex_f();
        call
    }

    fn a_page() -> Page {
        Page {
            lines: (0..20)
                .map(|y| (0..crate::page::WIDTH).map(|x| (x + y) % 40 < 7).collect())
                .collect(),
            resolution: Resolution::Standard,
        }
    }

    /// A burst of frames as the far end's procedure would give them to its
    /// modem.
    fn burst(messages: &[Message]) -> VecDeque<bool> {
        let mut tx = Sender::new();
        tx.send_flagged(messages, V34_FLAGS);
        std::iter::from_fn(|| tx.next_bit()).collect()
    }

    fn a_dcs() -> Message {
        Message::new(Frame::Dcs, true).with_fif(&t30::v34_command(Command {
            modulation: Modulation::V29,
            bits_per_second: 9600,
            fine: false,
            scan_line_field: 0b111,
            coding: Coding::Mmr,
            optional_l0: false,
            error_correction: true,
        }))
    }

    /// Time passing with the control channel up at 1200 bit/s both ways: what
    /// the call sends is taken, and it is given `far` -- and then flags while
    /// `flags` says the far end has nothing else to say, or nothing if not.
    fn talk(call: &mut Call, far: &mut VecDeque<bool>, seconds: f64, flags: bool) -> Vec<bool> {
        let mut out = Vec::new();
        let mut idle = Sender::new();
        let mut clock = 0.0;
        for _ in 0..(seconds * FS) as usize {
            clock += 1200.0 / FS;
            if clock >= 1.0 {
                clock -= 1.0;
                if let Some(bit) = call.next_control_bit() {
                    out.push(bit);
                }
                let bit = match far.pop_front() {
                    Some(bit) => Some(bit),
                    None if flags => Some(idle.next_bit_or_flag()),
                    None => None,
                };
                if let Some(bit) = bit {
                    call.control_bit(bit);
                }
            }
            call.tick(true);
        }
        // A real far end never stops part way through a flag, and the six
        // ones in the middle of one would count towards the next forty.
        while idle.mid_flag() {
            call.control_bit(idle.next_bit_or_flag());
        }
        out
    }

    /// Give the call a burst of frames, and stop the moment it has acted on
    /// them, before it has sent anything of its own.
    fn tell(call: &mut Call, messages: &[Message]) {
        let before = call.phase();
        for bit in burst(messages) {
            call.control_bit(bit);
            call.tick(true);
            if call.phase() != before {
                return;
            }
        }
    }

    fn frames_in(bits: &[bool]) -> Vec<Message> {
        let mut reader = Reader::new();
        bits.iter().filter_map(|&bit| reader.feed(bit)).collect()
    }

    fn count(bits: &[bool], frame: Frame) -> usize {
        frames_in(bits).iter().filter(|m| m.frame == frame).count()
    }

    fn octets(bits: &[bool]) -> Vec<u8> {
        bits.chunks(8)
            .map(|c| c.iter().enumerate().fold(0u8, |o, (i, &b)| o | u8::from(b) << i))
            .collect()
    }

    /// Whether some bits are flags and nothing else, wherever the first
    /// begins.
    fn all_flags(bits: &[bool]) -> bool {
        (0..8).any(|k| {
            let whole = (bits.len() - k) / 8 * 8;
            whole > 0 && octets(&bits[k..k + whole]).iter().all(|&o| o == FLAG)
        })
    }

    #[test]
    fn the_answering_end_begins_phase_b_on_the_control_channel_and_then_flags() {
        let mut call = answering();
        assert_eq!(call.phase(), Phase::Identifying);
        assert_eq!(call.line(), Line::V34Control);
        let bits = talk(&mut call, &mut VecDeque::new(), 1.0, true);
        let sent = frames_in(&bits);
        let names: Vec<Frame> = sent.iter().map(|m| m.frame).collect();
        assert_eq!(names, [Frame::Csi, Frame::Dis]);
        // F.3.1.4: at least two flags ahead of the first frame.
        assert!(octets(&bits[..8 * V34_FLAGS]).iter().all(|&o| o == FLAG));
        let dis = &sent[1].fif;
        assert!(t30::bit(dis, 27) && t30::bit(dis, 6), "{dis:02x?}");
        // And then nothing but flags, waiting for the command.
        assert_eq!(call.phase(), Phase::AwaitingCommand);
        assert_eq!(call.line(), Line::V34Listen);
        let more = talk(&mut call, &mut VecDeque::new(), 0.5, true);
        assert!(more.len() > 500);
        assert!(all_flags(&more), "not flags");
    }

    #[test]
    fn error_correction_is_used_even_where_it_was_not_offered() {
        let mut call = Call::answer(FS, "1");
        call.set_error_correction(false);
        call.start_annex_f();
        assert!(call.error_correction());
        let bits = talk(&mut call, &mut VecDeque::new(), 1.0, true);
        let dis = frames_in(&bits).into_iter().find(|m| m.frame == Frame::Dis).expect("no DIS");
        assert!(t30::capabilities(&dis.fif).error_correction);
    }

    #[test]
    fn a_dcs_is_answered_with_cfr_at_once_and_names_no_rate() {
        let mut call = answering();
        talk(&mut call, &mut VecDeque::new(), 1.0, true);
        call.set_primary_rate(31_200);
        let bits = talk(&mut call, &mut burst(&[a_dcs()]), 0.5, true);
        let sent: Vec<Frame> = frames_in(&bits).iter().map(|m| m.frame).collect();
        assert_eq!(sent, [Frame::Cfr], "no training check, and CFR straight back");
        assert_eq!(call.phase(), Phase::TurningAround);
        assert_eq!(call.line(), Line::V34Listen, "flags until the ones");
        assert_eq!(call.rate(), 31_200, "the DCS's bits 11 to 14 set a rate");
        assert_eq!(call.coding(), Coding::Mmr);
    }

    #[test]
    fn the_recipient_falls_silent_on_the_fortieth_one_and_not_before() {
        let mut call = answering();
        talk(&mut call, &mut VecDeque::new(), 1.0, true);
        talk(&mut call, &mut burst(&[a_dcs()]), 0.5, true);
        assert_eq!(call.phase(), Phase::TurningAround);
        // Thirty-nine, a zero, thirty-nine: not forty in a row.
        let mut ones: VecDeque<bool> = std::iter::repeat_n(true, ONES - 1).collect();
        ones.push_back(false);
        ones.extend(std::iter::repeat_n(true, ONES - 1));
        let seconds = ones.len() as f64 / 1200.0 + 0.01;
        talk(&mut call, &mut ones, seconds, false);
        assert_eq!(call.phase(), Phase::TurningAround);
        assert_eq!(call.line(), Line::V34Listen);
        // And one more.
        talk(&mut call, &mut VecDeque::from([true]), 0.01, false);
        assert_eq!(call.phase(), Phase::Receiving);
        assert_eq!(call.line(), Line::V34PrimaryListen);
    }

    /// The source the moment a CFR has let it go, with a page or without one.
    fn let_go(page: bool) -> Call {
        let mut call = calling(page.then(a_page));
        let dis =
            Message::new(Frame::Dis, false).with_fif(&t30::v34_capabilities(&[Modulation::V29], true));
        talk(&mut call, &mut burst(&[dis]), 1.0, true);
        assert_eq!(call.phase(), Phase::AwaitingConfirm);
        tell(&mut call, &[Message::new(Frame::Cfr, false)]);
        // The flag this end was part way through when the CFR came goes out
        // whole first, as it would on the line; the ones begin after it.
        while call.sender.mid_flag() {
            call.next_control_bit();
        }
        call
    }

    #[test]
    fn the_source_sends_forty_ones_however_soon_the_far_end_is_silent() {
        let mut call = let_go(true);
        assert_eq!(call.phase(), Phase::TurningAround);
        assert_eq!(call.line(), Line::V34Ones);
        call.set_far_silent(true);
        let mut ones = 0;
        while call.phase() == Phase::TurningAround {
            assert_eq!(call.next_control_bit(), Some(true));
            ones += 1;
            call.tick(true);
            assert!(ones <= ONES, "more than forty to a far end already silent");
        }
        assert_eq!(ones, ONES);
        assert_eq!(call.phase(), Phase::Sending);
        assert_eq!(call.line(), Line::V34Primary);
    }

    #[test]
    fn the_source_sends_ones_until_the_far_ends_flags_stop() {
        let mut call = let_go(true);
        let ones = talk(&mut call, &mut VecDeque::new(), 2.0, true);
        assert!(ones.len() > 2000 && ones.iter().all(|&b| b), "{} bits", ones.len());
        assert_eq!(call.phase(), Phase::TurningAround, "the far end is still flagging");
        talk(&mut call, &mut VecDeque::new(), FLAGS_GONE - 0.02, false);
        assert_eq!(call.phase(), Phase::TurningAround, "too soon");
        talk(&mut call, &mut VecDeque::new(), 0.04, false);
        assert_eq!(call.phase(), Phase::Sending);
    }

    #[test]
    fn a_silence_from_before_the_line_began_turning_round_does_not_count() {
        let mut call = calling(Some(a_page()));
        let dis =
            Message::new(Frame::Dis, false).with_fif(&t30::v34_capabilities(&[Modulation::V29], true));
        talk(&mut call, &mut burst(&[dis]), 1.0, true);
        assert_eq!(call.phase(), Phase::AwaitingConfirm);
        // The modem's carrier detector dropped out for a moment before the
        // CFR came, and nothing has been said of it since.
        call.set_far_silent(true);
        tell(&mut call, &[Message::new(Frame::Cfr, false)]);
        assert_eq!(call.phase(), Phase::TurningAround);
        // The far end is still flagging, so the ones go on.
        talk(&mut call, &mut VecDeque::new(), 1.0, true);
        assert_eq!(call.phase(), Phase::TurningAround, "a stale silence let the page go");
        call.set_far_silent(true);
        talk(&mut call, &mut VecDeque::new(), 0.01, true);
        assert_eq!(call.phase(), Phase::Sending);
    }

    #[test]
    fn a_response_arriving_as_t4_runs_out_is_waited_for() {
        let mut call = calling(Some(a_page()));
        let dis =
            Message::new(Frame::Dis, false).with_fif(&t30::v34_capabilities(&[Modulation::V29], true));
        talk(&mut call, &mut burst(&[dis]), 1.0, true);
        assert_eq!(call.phase(), Phase::AwaitingConfirm);
        // T4 all but out as the CFR begins: its address and control field are
        // on the line before the timer runs out -- four flags and then the
        // frame's own, some 45 ms at 1200 bit/s -- and its closing flag
        // after, at some 75 ms. Without the wait the command would go again
        // and the CFR, arriving while it went, would be ignored.
        call.timer = 0.06;
        let bits = talk(&mut call, &mut burst(&[Message::new(Frame::Cfr, false)]), 0.5, true);
        assert_eq!(call.phase(), Phase::TurningAround, "T4 ran out under the response");
        assert_eq!(count(&bits, Frame::Dcs), 0, "the command went again over its own answer");
        let dis = Message::new(Frame::Dis, false)
            .with_fif(&t30::v34_capabilities(&[Modulation::V29], true));
        // And the start of a frame that never ends -- the address and control
        // field, three bits more, and then an abort -- holds the timer for
        // FRAME_SECONDS and no longer: the command goes again after it.
        let mut call = calling(Some(a_page()));
        talk(&mut call, &mut burst(&[dis]), 1.0, true);
        assert_eq!(call.phase(), Phase::AwaitingConfirm);
        call.timer = 0.06;
        let cfr = burst(&[Message::new(Frame::Cfr, false)]);
        let mut start: VecDeque<bool> = cfr.into_iter().take(8 * (V34_FLAGS + 1) + 20).collect();
        start.extend(std::iter::repeat_n(true, 8));
        let bits = talk(&mut call, &mut start, FRAME_SECONDS, true);
        assert_eq!(count(&bits, Frame::Dcs), 0, "T4 ran out under the start of a frame");
        let bits = talk(&mut call, &mut VecDeque::new(), 0.6, true);
        assert_eq!(count(&bits, Frame::Dcs), 1, "held past the longest frame");
    }

    #[test]
    fn a_source_with_no_page_says_goodbye_rather_than_turning_round() {
        let mut call = let_go(false);
        assert_eq!(call.phase(), Phase::Ending);
        let bits = talk(&mut call, &mut VecDeque::new(), 0.5, true);
        let sent: Vec<Frame> = frames_in(&bits).iter().map(|m| m.frame).collect();
        assert_eq!(sent, [Frame::Dcn]);
        assert_eq!(call.phase(), Phase::Done);
        assert_eq!(call.next_control_bit(), None, "anything after the disconnect");
    }

    #[test]
    fn the_page_opens_with_two_hundred_milliseconds_of_flags_at_the_primary_rate() {
        let mut call = let_go(true);
        call.set_primary_rate(33_600);
        call.set_far_silent(true);
        while call.phase() == Phase::TurningAround {
            call.next_control_bit();
            call.tick(true);
        }
        let bits: Vec<bool> = std::iter::from_fn(|| call.next_fast_bit()).collect();
        let flags = octets(&bits).iter().take_while(|&&o| o == FLAG).count();
        // A.3.1/T.4: "nominal 200 ms, tolerance +100 ms" -- 840 flags at this
        // rate -- and then the first frame's own opening flag.
        assert_eq!(flags, 841, "{flags} flags at 33 600 bit/s");
    }

    #[test]
    fn a_dis_again_is_the_command_again_and_an_ftt_is_not_heard() {
        let mut call = calling(Some(a_page()));
        let dis =
            Message::new(Frame::Dis, false).with_fif(&t30::v34_capabilities(&[Modulation::V29], false));
        let first = talk(&mut call, &mut burst(std::slice::from_ref(&dis)), 1.0, true);
        assert_eq!(call.phase(), Phase::AwaitingConfirm);
        talk(&mut call, &mut burst(&[Message::new(Frame::Ftt, false)]), 0.5, true);
        assert_eq!(call.phase(), Phase::AwaitingConfirm, "F.3.2.1: there is no FTT");
        let again = talk(&mut call, &mut burst(&[dis]), 1.0, true);
        let dcs = |bits: &[bool]| {
            frames_in(bits).into_iter().find(|m| m.frame == Frame::Dcs).expect("no DCS")
        };
        assert_eq!(dcs(&again), dcs(&first), "not the same command again");
        assert_eq!(t30::field_of(&dcs(&first).fif, 11, 14), 0, "Note 33");
    }

    #[test]
    fn flags_do_not_put_t2_back_and_the_start_of_a_frame_does() {
        // F.3.2.3 Note 2. The far end's flags go on for ever on this channel,
        // and T2 runs out through them: the DIS goes again.
        let mut call = answering();
        talk(&mut call, &mut VecDeque::new(), 1.0, true);
        assert_eq!(call.phase(), Phase::AwaitingCommand);
        let bits = talk(&mut call, &mut VecDeque::new(), T2_SECONDS + 0.5, true);
        assert_eq!(count(&bits, Frame::Dis), 1, "the flags put T2 back");

        // The start of a frame -- an address and a control field after a flag
        // -- and then nothing of it: T2 is put back all the same, and the DIS
        // does not go again when it would have.
        let mut call = answering();
        talk(&mut call, &mut VecDeque::new(), 1.0, true);
        let mut bits = talk(&mut call, &mut VecDeque::new(), T2_SECONDS - 1.0, true);
        let dcs = burst(&[Message::new(Frame::Dcs, true).with_fif(&[0; 40])]);
        // The four flags ahead of the burst and the frame's own opening flag,
        // then the address and control field -- eighteen bits on the line,
        // with the zeros HDLC stuffs into them -- and two bits of the FCF.
        let mut start: VecDeque<bool> = dcs.into_iter().take(8 * (V34_FLAGS + 1) + 20).collect();
        start.extend(std::iter::repeat_n(true, 8));
        bits.extend(talk(&mut call, &mut start, 0.1, true));
        bits.extend(talk(&mut call, &mut VecDeque::new(), 2.0, true));
        assert_eq!(call.phase(), Phase::AwaitingCommand);
        assert_eq!(count(&bits, Frame::Dis), 0, "T2 ran out counting from before the frame began");
    }

    #[test]
    fn nothing_holds_a_timeout_open_on_a_channel_that_carries_both_ends() {
        let mut call = answering();
        talk(&mut call, &mut VecDeque::new(), 1.0, true);
        call.set_control_carrier(true);
        let bits = talk(&mut call, &mut VecDeque::new(), T2_SECONDS + 0.5, true);
        assert_eq!(count(&bits, Frame::Dis), 1, "the far end's carrier held T2 open");
    }

    #[test]
    fn a_burst_the_control_channel_restarted_under_goes_again_whole() {
        let mut call = answering();
        // Part of the CSI goes, and is lost to a retrain.
        for _ in 0..120 {
            call.next_control_bit();
        }
        call.control_restarted();
        let bits = talk(&mut call, &mut VecDeque::new(), 1.0, true);
        let sent: Vec<Frame> = frames_in(&bits).iter().map(|m| m.frame).collect();
        assert_eq!(sent, [Frame::Csi, Frame::Dis]);
        assert!(octets(&bits[..8 * V34_FLAGS]).iter().all(|&o| o == FLAG), "no flags first");
        // And a burst not yet begun is left as it is.
        let mut call = answering();
        call.control_restarted();
        let bits = talk(&mut call, &mut VecDeque::new(), 1.0, true);
        assert_eq!(frames_in(&bits).len(), 2);
    }

    #[test]
    fn the_primary_rate_is_the_modems_and_only_in_annex_f() {
        let mut call = Call::originate(FS, "1", None);
        call.set_primary_rate(28_800);
        assert_eq!(call.rate(), 4800, "set outside Annex F");
        call.start_annex_f();
        assert!(call.annex_f());
        assert_eq!(call.rate(), PRIMARY_FASTEST);
        call.set_primary_rate(21_600);
        assert_eq!(call.rate(), 21_600);
        assert!(!call.renegotiate());
    }

    #[test]
    fn noise_after_a_flag_is_not_the_start_of_a_frame() {
        let mut far = FarEnd::default();
        let mut starts = 0;
        let mut x = 0x2545_f491u32;
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if far.hear(x & 1 == 1) {
                starts += 1;
            }
        }
        // A flag in noise every 256 bits or so, and an address and control
        // field after one once in 65 536 of those.
        assert!(starts <= 2, "{starts} starts of frames in noise");
        let mut far = FarEnd::default();
        let frame = burst(&[Message::new(Frame::Mcf, false)]);
        assert_eq!(frame.into_iter().filter(|&b| far.hear(b)).count(), 1);
    }
}
