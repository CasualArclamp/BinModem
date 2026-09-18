//! The SUV/CP/E acknowledge-and-repeat exchange of Phase 4 (9.6.1.1, 9.6.2.1
//! and Figures 12-14), which rate renegotiation (9.8) and fast parameter
//! exchange (9.9) enter with a context of their own.
//!
//! Both modems reach the end of training holding one thing the other has to
//! have: the analogue modem's CPu says what it will send, the digital modem's
//! CPd says how the upstream is to be encoded. Neither can go on until its own
//! has been heard. V.92 settles that with a handshake small enough to fit in
//! one paragraph on each side, and 9.6.1.1 and 9.6.2.1 are word-for-word
//! mirrors of one another: send short SUV sequences continuously; send exactly
//! **one** CP once the peer's first SUV has arrived; set an acknowledge bit in
//! everything sent after the peer's CP arrives; repeat the CP only if nothing
//! acknowledged has come back by "100 ms plus a round-trip delay from the end
//! of its CPd" (9.6.1.1.3); and stop, with E, once an acknowledged sequence
//! has been both sent and received.
//!
//! So this is one machine, not two (AD-9). It is driven by [`PeerSuv`],
//! [`PeerCp`] and [`SequenceKind`], the plain flag structs the parent module
//! declares, which is why it can be side-agnostic: it never sees a bit layout,
//! a symbol rate or a constellation, and `v92/sequences.rs` need not exist for
//! it to be finished and tested. The owner -- `v92::analogue` (V92-39),
//! `v92::digital` (V92-40) and the renegotiation and parameter-exchange
//! packages after them -- builds the wire sequences and tells the machine what
//! crossed the line.
//!
//! ```text
//! Figure 12/V.92, as the machine plays it:
//!   digital  SUVd SUVd CPd       SUVd' SUVd' Ed
//!   analogue SUVu SUVu CPu  SUVu SUVu' SUVu' E2u
//! ```
//!
//! Three things the clause leaves to the implementer are fixed here, each as a
//! named constant with its alternative reading: which instant of a received
//! sequence the repeat window is measured to ([`REPEAT_WINDOW_AT_RECEPTION_END`]),
//! what a silence request means outside rate renegotiation
//! ([`SILENCE_ONLY_IN_RENEGOTIATION`]), and whether an analogue modem's "wait
//! for my CPu" is complied with ([`HONOUR_WAIT_FOR_CP`]).
//!
//! What is deliberately *not* here is the silent period of 9.8. This machine
//! reports a peer's request and grant and offers [`Exchange::silence_ended`]
//! to clear the acknowledge state when the line comes back, because bit 33
//! means something different on either side of a silence (P4D Q10); the branch
//! itself -- SUVd' until SUVu', Ed, silence, Rt, R-bar-t -- belongs to V92-51,
//! which owns the clause.

use super::{PeerCp, PeerSuv, SequenceKind};

// ---------------------------------------------------------------------------
// The readings this module owns (plan section 4)
// ---------------------------------------------------------------------------

/// The grace before a CP may be repeated, on top of one round-trip delay:
/// "the entire CPu or SUVu sequence that is received after 100 ms plus a
/// round-trip delay from the end of its CPd" (9.6.1.1.3, and 9.6.2.1.3 the
/// other way round). Figure 14 labels the same span "RTD + 100 ms".
///
/// In seconds, because a round-trip delay is carried in seconds throughout
/// this crate (`v90::analogue::Settings::round_trip`) and the two sides of the
/// exchange count samples at different rates.
pub const CP_REPEAT_GRACE: f64 = 0.100;

/// Which instant of a received sequence the repeat window is measured to.
///
/// 9.6.1.1.3 says "up to and including the entire CPu or SUVu sequence that is
/// received after 100 ms plus a round-trip delay", and gives no instant for
/// "received". `true` here is the reading that "the entire sequence" is what
/// has to be received, so the `at` of [`Exchange::peer_suv`] and
/// [`Exchange::peer_cp`] is the sample at which **reception completed**, and
/// the sequence that straddles the deadline is included in the check (P4P
/// Q12). The alternative is that reception has to have *begun* after the
/// deadline, which would drop the straddling sequence and start repeats one
/// sequence earlier; flipping this constant means passing each sequence's
/// first sample as `at` instead, and nothing in this module changes.
pub const REPEAT_WINDOW_AT_RECEPTION_END: bool = true;

/// Whether a silence request means anything outside rate renegotiation.
///
/// SUVu bit 32 and SUVd bit 32 both read "Set to 1 indicates that a silent
/// period is requested. This may be used during rate renegotiation" (Tables 27
/// and 31), and 9.8 is the only clause that says what a silent period *is*.
/// `true` therefore sends 0 in the [`Context::Training`] and
/// [`Context::FastExchange`] contexts and swallows a received 1 without
/// reporting it, which is what P4P Q15 infers and what 9.9.2.1.2 says outright
/// for the parameter exchange ("SUVu with bit 32 clear"). The alternative is
/// to report it anyway and leave the owner to ignore it, which only moves the
/// decision somewhere that has no clause to make it with.
pub const SILENCE_ONLY_IN_RENEGOTIATION: bool = true;

/// Whether leaving a silent period clears the acknowledge state.
///
/// Table 31 bit 33 means "received CPu from the analogue modem", but 9.8.1.1.3
/// sets it in the silence handshake, where no CP has been exchanged at all, so
/// inside the silence branch it means "your SUV with bit 32 was received"
/// (P4D Q10, RRF 3.5). `true` therefore clears everything the exchange knows
/// when [`Exchange::silence_ended`] is called, so that the plain SUVd and SUVu
/// that follow Rt and R-bar-t start a fresh exchange and bit 33 gets its
/// Table 31 meaning back -- which is what Figures 16 to 18 draw: SUV, then CP,
/// then SUV'. The alternative is to keep the state across the silence, which
/// would send bit 33 set before the new CP had been seen.
pub const SILENCE_CLEARS_ACK_STATE: bool = true;

/// Whether "wait for my CPu before sending CPd" is complied with.
///
/// Table 27 bit 26: "Set to 1 indicates that the analogue modem wishes the
/// digital modem to wait for a CPu before sending a CPd. The digital modem is
/// not required to comply with this request." `true` complies, but only up to
/// [`Exchange::window`] -- one repeat window -- after the CP first became due,
/// which is what makes complying free: the wait can never outlast the time the
/// peer would have had to wait for a repeat anyway. The alternative is to
/// ignore the bit, which the table allows outright and which Figure 13 shows
/// the digital modem *not* doing.
pub const HONOUR_WAIT_FOR_CP: bool = true;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Which modulation the sequences of an exchange are carried by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    /// The TRN2 modulation of 8.7.6 and 8.8.6 -- the 4- or 8-point upstream
    /// signal, or V.90's TRN2d downstream.
    Trn2,
    /// "The preceding data mode modulation" (8.7.5, 8.8.5), which only a fast
    /// parameter exchange uses.
    Data,
}

/// Which procedure has entered the 9.6 exchange.
///
/// 9.8.1.1.2, 9.8.2.1.3, 9.9.1.1.2 and 9.9.2.1.2 all end by saying to continue
/// at 9.6.1.1.2 or 9.6.2.1.2, so the same machine runs three times over a call
/// (P4P 2.3). What differs is around it, not in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Phase 4, final training, entered from 9.6.1.1.1 or 9.6.2.1.1.
    Training,
    /// Rate renegotiation, 9.8. The only context in which a silent period is
    /// defined.
    Renegotiation,
    /// Fast parameter exchange, 9.9.
    FastExchange,
}

impl Context {
    /// What carries SUV, CP and E here: "SUVu is scrambled and transmitted
    /// using the corresponding TRN2u modulation during Training and Rate
    /// Renegotiation. During Fast Parameter Exchange it is transmitted using
    /// preceding data mode modulation" (8.7.5, and 8.8.5 downstream).
    pub fn modulation(self) -> Modulation {
        match self {
            Self::Training | Self::Renegotiation => Modulation::Trn2,
            Self::FastExchange => Modulation::Data,
        }
    }

    /// Whether CPd bit 29, the one-symbol extension of E2u, may be set.
    ///
    /// Only in initial training: the bit moves the upstream data-frame phase
    /// by 1T at the point where B1u begins interval 0, and a renegotiation or
    /// a parameter exchange has to keep the frame alignment it already has
    /// (P4D Q5, 8.7.2).
    pub fn may_extend_e2u(self) -> bool {
        matches!(self, Self::Training)
    }

    /// Whether FB1u precedes B1u: "After sending the E2u sequence, the
    /// analogue modem shall send either B1u, or, for Fast Parameter Exchange,
    /// FB1u followed by B1u" (9.6.2.1.5, with 9.6.1.1.6 for the receiver).
    /// The digital modem sends no FB1d either way.
    pub fn fb1_before_b1(self) -> bool {
        matches!(self, Self::FastExchange)
    }

    /// Whether a silent period is defined here at all. Only 9.8 defines one,
    /// and 9.9.2.1.2 requires bit 32 clear in a parameter exchange; see
    /// [`SILENCE_ONLY_IN_RENEGOTIATION`].
    pub fn defines_silence(self) -> bool {
        !SILENCE_ONLY_IN_RENEGOTIATION || matches!(self, Self::Renegotiation)
    }
}

// ---------------------------------------------------------------------------
// The exchange
// ---------------------------------------------------------------------------

/// The side-agnostic SUV/CP/E machine of 9.6.1.1 and 9.6.2.1.
///
/// The owner drives it at sequence boundaries. [`Exchange::next_sequence`]
/// says what should go next, [`Exchange::sequence_started`] gives back the
/// acknowledge bit to put in it, and [`Exchange::sequence_ended`] closes it
/// off; [`Exchange::peer_suv`], [`Exchange::peer_cp`] and
/// [`Exchange::peer_e`] report what has been received, whenever it arrives.
///
/// `next_sequence` is advice, not an order. An owner that is not yet able to
/// send its CP -- the digital modem is still designing CPd out of TRN2u while
/// its SUVd repeat -- may send an SUV instead, and the obligation stands until
/// a CP has actually been started. Figure 13 draws exactly that.
#[derive(Debug, Clone)]
pub struct Exchange {
    context: Context,
    /// 100 ms + RTD in samples of the owner's own clock.
    window: u64,
    /// We have transmitted a CP or SUV with the acknowledge bit set
    /// (9.6.1.1.4's "has sent ... with the acknowledgement bit set").
    sent_ack: bool,
    /// A CP -- long or short -- has been received from the peer.
    got_peer_cp: bool,
    /// A CP or SUV with the acknowledge bit set, or an E, has been received.
    peer_acked: bool,
    /// An SUV has been received from the peer, which is what releases our one
    /// CP (9.6.1.1.2, 9.6.2.1.2).
    peer_suv: bool,
    /// We have started at least one SUV of our own, which 9.8.2.1.3 requires
    /// before the CP ("after having transmitted an SUVu and received an
    /// SUVd").
    sent_suv: bool,
    /// The sample the first CP of this exchange finished at, which the repeat
    /// window is measured from.
    my_cp_end: Option<u64>,
    /// The repeat rule has fired: CPs go out until the exchange ends.
    repeat_cp: bool,
    /// Whether the window check has already been settled, one way or the
    /// other.
    settled: bool,
    /// Our one CP is still owed.
    need_single_cp: bool,
    /// The peer's last SUV asked us to wait for its CP (Table 27 bit 26).
    wait_for_cp: bool,
    /// When that wait runs out.
    hold_until: Option<u64>,
    /// The kind in flight and the acknowledge bit it went out with.
    sending: Option<(SequenceKind, bool)>,
    /// The peer's last SUV asked for or granted a silent period.
    peer_silence: bool,
    /// The peer has asked for one at some point in this exchange.
    peer_asked_silence: bool,
    /// What our own SUV should carry in bit 32.
    silence: bool,
    /// Our E has been sent.
    finished: bool,
}

impl Exchange {
    /// A fresh exchange in `context`, with `round_trip` in seconds and `fs`
    /// the owner's own sample rate -- 8000 for the digital modem, the line
    /// rate for the analogue one.
    pub fn new(context: Context, round_trip: f64, fs: f64) -> Self {
        Self {
            context,
            window: ((CP_REPEAT_GRACE + round_trip) * fs).round().max(0.0) as u64,
            sent_ack: false,
            got_peer_cp: false,
            peer_acked: false,
            peer_suv: false,
            sent_suv: false,
            my_cp_end: None,
            repeat_cp: false,
            settled: false,
            need_single_cp: true,
            wait_for_cp: false,
            hold_until: None,
            sending: None,
            peer_silence: false,
            peer_asked_silence: false,
            silence: false,
            finished: false,
        }
    }

    /// Which procedure this exchange belongs to.
    pub fn context(&self) -> Context {
        self.context
    }

    /// 100 ms + RTD, in samples: the repeat window of 9.6.1.1.3, and the bound
    /// on complying with a "wait for my CPu".
    pub fn window(&self) -> u64 {
        self.window
    }

    // -- what the peer sent --------------------------------------------------

    /// A whole SUV has been received, its CRC good, at sample `at`
    /// ([`REPEAT_WINDOW_AT_RECEPTION_END`]). A sequence with a bad CRC is not
    /// reported at all: 9.6 has no rule for one beyond the repeat mechanism
    /// itself.
    pub fn peer_suv(&mut self, suv: PeerSuv, at: u64) {
        self.peer_suv = true;
        self.wait_for_cp = HONOUR_WAIT_FOR_CP && suv.wait_for_cp;
        if self.context.defines_silence() {
            self.peer_silence = suv.silence;
            self.peer_asked_silence |= suv.silence;
        }
        self.heard(suv.ack, at);
    }

    /// A whole CP has been received at sample `at`. A CPus is a CP: 9.6.1.1.2
    /// names only CPu, but the short form carries the same acknowledge bit and
    /// the same drn, and the renegotiation and parameter-exchange clauses send
    /// it in place of CPu (P4P 6.1 D4-2).
    pub fn peer_cp(&mut self, cp: PeerCp, at: u64) {
        self.got_peer_cp = true;
        self.heard(cp.ack, at);
    }

    /// The peer's E has been received at sample `at`. 9.6.1.1.4 lists E2u
    /// beside an acknowledged sequence, because a modem only sends E once it
    /// has acknowledged one.
    pub fn peer_e(&mut self, at: u64) {
        self.heard(true, at);
    }

    /// One received CP or SUV, against the repeat rule of 9.6.1.1.3.
    fn heard(&mut self, ack: bool, at: u64) {
        if ack {
            self.acknowledged();
        } else if !self.settled {
            // "up to and including the entire CPu or SUVu sequence that is
            // received after 100 ms plus a round-trip delay from the end of
            // its CPd": the first sequence to complete past the deadline is
            // the last one that can carry the acknowledgement, so once it has
            // gone by unacknowledged the repeats start.
            if let Some(end) = self.my_cp_end
                && at > end.saturating_add(self.window)
            {
                self.repeat_cp = true;
                self.settled = true;
            }
        }
    }

    /// Something acknowledged has arrived, so the reason for repeating is
    /// gone. 9.6.1.1.3 only says when repeats *start*; stopping them once an
    /// acknowledgement finally turns up is inferred, and costs nothing,
    /// because 9.6.1.1.4 ends the exchange a sequence later anyway.
    fn acknowledged(&mut self) {
        self.peer_acked = true;
        self.repeat_cp = false;
        self.settled = true;
    }

    // -- what we send --------------------------------------------------------

    /// What should go next, asked at a sequence boundary, `now` being that
    /// boundary.
    ///
    /// The order is the order of the clauses: E ends the exchange
    /// (9.6.1.1.4), otherwise a CP goes if one is owed or is being repeated
    /// (9.6.1.1.2, 9.6.1.1.3), otherwise an SUV. A CP is the only thing ever
    /// held back; an SUV, an E and a B1 never are.
    pub fn next_sequence(&mut self, now: u64) -> SequenceKind {
        if self.finished {
            return SequenceKind::B1;
        }
        if self.sent_ack && self.peer_acked {
            return SequenceKind::E;
        }
        if self.cp_due(now) {
            return SequenceKind::Cp;
        }
        SequenceKind::Suv
    }

    /// Whether a CP may go at `now`, arming the wait-for-CP bound the first
    /// time one is held back.
    fn cp_due(&mut self, now: u64) -> bool {
        if self.repeat_cp {
            return true;
        }
        if !self.need_single_cp || !self.peer_suv || !self.sent_suv {
            return false;
        }
        if self.got_peer_cp || !self.wait_for_cp {
            return true;
        }
        match self.hold_until {
            Some(until) => now > until,
            None => {
                self.hold_until = Some(now.saturating_add(self.window));
                false
            }
        }
    }

    /// `kind` is going out now. The returned acknowledge bit is fixed for the
    /// whole sequence: "subsequent CPd and SUVd sequences" (9.6.1.1.2) change
    /// at a boundary and nowhere else, which is also what makes a group of
    /// CPd and CPd' "all contain identical information" apart from that bit
    /// (8.8.3, 8.8.5, 8.7.3, 8.7.5).
    ///
    /// TRN2, E and B1 carry no acknowledge bit and always give back `false`.
    pub fn sequence_started(&mut self, kind: SequenceKind) -> bool {
        let ack = carries_ack(kind) && self.got_peer_cp;
        if is_cp(kind) {
            self.need_single_cp = false;
        }
        if matches!(kind, SequenceKind::Suv) {
            self.sent_suv = true;
        }
        self.sending = Some((kind, ack));
        ack
    }

    /// `kind` finished at sample `at`.
    ///
    /// The acknowledge bit counts as *sent* here rather than at the start,
    /// because 9.6.1.1.4 asks for a sequence the modem "has sent"; and the
    /// repeat window of 9.6.1.1.3 runs "from the end of its CPd", which is
    /// this instant, and from the end of the first CP only -- a repeat does
    /// not restart it, or Figure 14's three CPu' could not follow one another.
    pub fn sequence_ended(&mut self, kind: SequenceKind, at: u64) {
        if let Some((_, ack)) = self.sending.take() {
            self.sent_ack |= ack;
        }
        if is_cp(kind) && self.my_cp_end.is_none() {
            self.my_cp_end = Some(at);
        }
        if matches!(kind, SequenceKind::E) {
            self.finished = true;
        }
    }

    /// The sequence in flight and the acknowledge bit it went out with.
    pub fn sending(&self) -> Option<(SequenceKind, bool)> {
        self.sending
    }

    // -- the silent period of 9.8 -------------------------------------------

    /// Ask for a silent period in the SUVs we send (bit 32).
    ///
    /// Refused outside [`Context::Renegotiation`], where the bit stays 0: see
    /// [`SILENCE_ONLY_IN_RENEGOTIATION`].
    pub fn request_silence(&mut self, wanted: bool) {
        self.silence = wanted && self.context.defines_silence();
    }

    /// What bit 32 of our next SUV should carry.
    pub fn silence_requested(&self) -> bool {
        self.silence
    }

    /// Bit 32 of the peer's last SUV. 9.8.1.1.4 waits for "an SUVu with bit 32
    /// clear", so this follows the latest sequence rather than latching.
    /// Always `false` outside a renegotiation.
    pub fn peer_silence(&self) -> bool {
        self.peer_silence
    }

    /// Whether the peer has asked for a silent period at any point in this
    /// exchange, which is the test 9.8.1.1.2 and 9.8.2.1.3 make ("if bit 32 is
    /// set in either ... go to"). Always `false` outside a renegotiation.
    pub fn peer_asked_silence(&self) -> bool {
        self.peer_asked_silence
    }

    /// The silent period is over and the exchange begins again, which is where
    /// the plain SUVd and SUVu of Figures 16 to 18 come from.
    ///
    /// See [`SILENCE_CLEARS_ACK_STATE`]: bit 33 meant "your SUV was received"
    /// inside the silence and means "your CP was received" outside it, so
    /// everything the exchange knows about acknowledgement is dropped, the one
    /// CP is owed again, and the window is unarmed. The context and the
    /// measured window survive.
    pub fn silence_ended(&mut self) {
        self.peer_silence = false;
        self.peer_asked_silence = false;
        self.silence = false;
        if !SILENCE_CLEARS_ACK_STATE {
            return;
        }
        self.sent_ack = false;
        self.got_peer_cp = false;
        self.peer_acked = false;
        self.peer_suv = false;
        self.sent_suv = false;
        self.my_cp_end = None;
        self.repeat_cp = false;
        self.settled = false;
        self.need_single_cp = true;
        self.wait_for_cp = false;
        self.hold_until = None;
    }

    // -- what the owner can look at -----------------------------------------

    /// We have sent a CP or SUV with the acknowledge bit set.
    pub fn sent_ack(&self) -> bool {
        self.sent_ack
    }

    /// A CP has been received from the peer, so everything we send from the
    /// next boundary on carries the acknowledge bit.
    pub fn got_peer_cp(&self) -> bool {
        self.got_peer_cp
    }

    /// Something acknowledged, or the peer's E, has been received.
    pub fn peer_acked(&self) -> bool {
        self.peer_acked
    }

    /// The repeat rule of 9.6.1.1.3 has fired.
    pub fn repeating(&self) -> bool {
        self.repeat_cp
    }

    /// When our first CP of this exchange finished, if it has.
    pub fn my_cp_end(&self) -> Option<u64> {
        self.my_cp_end
    }

    /// Our E has been sent, so what follows is B1 -- FB1 first in a parameter
    /// exchange ([`Context::fb1_before_b1`]).
    pub fn finished(&self) -> bool {
        self.finished
    }
}

/// Whether a sequence has an acknowledge bit at all. Bit 33 is it in CPu, CPt,
/// CPus, CPd, SUVu and SUVd; TRN2, E and B1 are not framed sequences.
fn carries_ack(kind: SequenceKind) -> bool {
    matches!(kind, SequenceKind::Suv | SequenceKind::Cp | SequenceKind::Cpus)
}

/// Whether a sequence discharges the one-CP obligation. CPus is the short CPu
/// (Table 24) and counts.
fn is_cp(kind: SequenceKind) -> bool {
    matches!(kind, SequenceKind::Cp | SequenceKind::Cpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8000 samples per second on both sides, so one sample is one symbol at
    /// the upstream and downstream symbol rate and the scripted traces below
    /// can be read in symbols.
    const FS: f64 = 8000.0;

    /// Which end a scripted side is, for the labels the figures print.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Side {
        Digital,
        Analogue,
    }

    impl Side {
        fn label(self, kind: SequenceKind, ack: bool) -> String {
            let stem = match (self, kind) {
                (Self::Digital, SequenceKind::Trn2) => "TRN2d",
                (Self::Digital, SequenceKind::Suv) => "SUVd",
                (Self::Digital, SequenceKind::Cp | SequenceKind::Cpus) => "CPd",
                (Self::Digital, SequenceKind::E) => "Ed",
                (Self::Digital, SequenceKind::B1) => "B1d",
                (Self::Analogue, SequenceKind::Trn2) => "TRN2u",
                (Self::Analogue, SequenceKind::Suv) => "SUVu",
                (Self::Analogue, SequenceKind::Cp) => "CPu",
                (Self::Analogue, SequenceKind::Cpus) => "CPus",
                (Self::Analogue, SequenceKind::E) => "E2u",
                (Self::Analogue, SequenceKind::B1) => "B1u",
            };
            if ack { format!("{stem}'") } else { stem.to_string() }
        }
    }

    /// One scripted end: an exchange, the lengths of the sequences it sends,
    /// and what it has put on the line.
    #[derive(Debug)]
    struct Scripted {
        side: Side,
        exchange: Exchange,
        suv: u64,
        cp: u64,
        e: u64,
        /// The sample the sequence in flight ends at.
        end: u64,
        sending: Option<(SequenceKind, bool)>,
        /// Sequences whose CP is withheld until this sample, because the
        /// digital modem is still designing it out of TRN2u.
        cp_ready: u64,
        /// The next CP this side sends is dropped by the line.
        lose_cp: bool,
        /// Bits 26 and 32 of the SUVs this side sends.
        flags: PeerSuv,
        trace: Vec<String>,
        /// Where each sequence of the trace started.
        starts: Vec<u64>,
    }

    impl Scripted {
        fn new(side: Side, context: Context, round_trip: f64, suv: u64, cp: u64, start: u64) -> Self {
            Self {
                side,
                exchange: Exchange::new(context, round_trip, FS),
                suv,
                cp,
                e: 12,
                end: start,
                sending: None,
                cp_ready: 0,
                lose_cp: false,
                flags: PeerSuv::default(),
                trace: Vec::new(),
                starts: Vec::new(),
            }
        }

        fn length(&self, kind: SequenceKind) -> u64 {
            match kind {
                SequenceKind::Suv => self.suv,
                SequenceKind::Cp | SequenceKind::Cpus => self.cp,
                _ => self.e,
            }
        }
    }

    /// A sequence crossing the line: when its reception completes, who hears
    /// it, and what it was.
    #[derive(Debug, Clone, Copy)]
    struct Crossing {
        at: u64,
        to: usize,
        kind: SequenceKind,
        ack: bool,
        suv: PeerSuv,
    }

    /// Two scripted ends and a one-way delay, stepped event by event: a
    /// delivery lands before the boundary it falls on, because a sequence that
    /// finishes arriving exactly at a boundary has been received by then.
    #[derive(Debug)]
    struct Line {
        ends: [Scripted; 2],
        delay: u64,
        crossings: Vec<Crossing>,
    }

    impl Line {
        fn new(digital: Scripted, analogue: Scripted, delay: u64) -> Self {
            Self { ends: [digital, analogue], delay, crossings: Vec::new() }
        }

        fn run(&mut self, until: u64) {
            loop {
                let mut next = None;
                for end in &self.ends {
                    if !end.exchange.finished() {
                        next = Some(next.map_or(end.end, |t: u64| t.min(end.end)));
                    }
                }
                for crossing in &self.crossings {
                    next = Some(next.map_or(crossing.at, |t: u64| t.min(crossing.at)));
                }
                let Some(t) = next else { break };
                if t > until {
                    break;
                }
                let now = t;
                for crossing in self.crossings.clone() {
                    if crossing.at != now {
                        continue;
                    }
                    let heard = &mut self.ends[crossing.to].exchange;
                    match crossing.kind {
                        SequenceKind::Suv => {
                            heard.peer_suv(PeerSuv { ack: crossing.ack, ..crossing.suv }, now);
                        }
                        SequenceKind::Cp | SequenceKind::Cpus => {
                            heard.peer_cp(PeerCp { ack: crossing.ack }, now);
                        }
                        _ => heard.peer_e(now),
                    }
                }
                self.crossings.retain(|crossing| crossing.at != now);
                for i in 0..2 {
                    if self.ends[i].exchange.finished() || self.ends[i].end != now {
                        continue;
                    }
                    if let Some((kind, ack)) = self.ends[i].sending.take() {
                        self.ends[i].exchange.sequence_ended(kind, now);
                        self.ends[i].trace.push(self.ends[i].side.label(kind, ack));
                        let lost = is_cp(kind) && self.ends[i].lose_cp;
                        if lost {
                            self.ends[i].lose_cp = false;
                        } else {
                            self.crossings.push(Crossing {
                                at: now + self.delay,
                                to: 1 - i,
                                kind,
                                ack,
                                suv: self.ends[i].flags,
                            });
                        }
                        if self.ends[i].exchange.finished() {
                            continue;
                        }
                    }
                    let mut kind = self.ends[i].exchange.next_sequence(now);
                    if is_cp(kind) && now < self.ends[i].cp_ready {
                        kind = SequenceKind::Suv;
                    }
                    let ack = self.ends[i].exchange.sequence_started(kind);
                    self.ends[i].sending = Some((kind, ack));
                    self.ends[i].starts.push(now);
                    self.ends[i].end = now + self.ends[i].length(kind);
                }
            }
        }

        fn trace(&self, side: usize) -> Vec<&str> {
            self.ends[side].trace.iter().map(String::as_str).collect()
        }
    }

    /// The trace with each run of identical labels collapsed to a count, for
    /// the figures whose boxes stand for a span the Recommendation draws far
    /// shorter than 100 ms.
    fn runs(trace: &[&str]) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for label in trace {
            match out.last_mut() {
                Some((last, count)) if last == label => *count += 1,
                _ => out.push(((*label).to_string(), 1)),
            }
        }
        out
    }

    /// Figure 12/V.92, "final training where the two CP sequences occur at
    /// about the same time": the digital modem sends SUVd SUVd CPd SUVd'
    /// SUVd' Ed against the analogue modem's SUVu SUVu CPu SUVu SUVu' SUVu'
    /// E2u.
    ///
    /// The lengths are the printed multiples -- 6 symbols downstream, 12
    /// upstream (Tables 30, 31, 23, 27) -- with an SUVd of 36 and an SUVu of
    /// 24 symbols, the CPs at 96, and 0.75 ms each way, which is what puts the
    /// crossings where the figure draws them.
    #[test]
    fn figure_12_crossing_cps_end_in_ed_and_e2u() {
        let rtd = 12.0 / FS;
        let digital = Scripted::new(Side::Digital, Context::Training, rtd, 36, 96, 0);
        let analogue = Scripted::new(Side::Analogue, Context::Training, rtd, 24, 96, 12);
        let mut line = Line::new(digital, analogue, 6);
        line.run(4000);
        assert_eq!(line.trace(0), ["SUVd", "SUVd", "CPd", "SUVd'", "SUVd'", "Ed"]);
        assert_eq!(
            line.trace(1),
            ["SUVu", "SUVu", "CPu", "SUVu", "SUVu'", "SUVu'", "E2u"]
        );
    }

    /// Figure 13/V.92, "final training where CPu is sent earlier than CPd":
    /// the analogue modem's SUVu carries Table 27 bit 26, so the digital
    /// modem holds its CPd back, the CPu arrives first, and the single CPd is
    /// already CPd'.
    ///
    /// The figure draws one SUVd' between the CPu's arrival and CPd'. That is
    /// the design still running -- the digital modem builds CPd out of TRN2u
    /// while its SUVd repeat -- and it is scripted here as `cp_ready`, the
    /// same drawing slack P4P reads into Figure 14's late switch to SUVu'.
    #[test]
    fn figure_13_a_cpu_heard_first_makes_the_single_cpd_a_cpd_prime() {
        let rtd = 12.0 / FS;
        let mut digital = Scripted::new(Side::Digital, Context::Training, rtd, 24, 48, 0);
        digital.cp_ready = 120;
        let mut analogue = Scripted::new(Side::Analogue, Context::Training, rtd, 24, 36, 0);
        // The analogue modem's SUVu asks the digital modem to wait for its CPu.
        analogue.flags.wait_for_cp = true;
        let mut line = Line::new(digital, analogue, 6);
        line.run(4000);
        assert_eq!(
            line.trace(0),
            ["SUVd", "SUVd", "SUVd", "SUVd", "SUVd'", "CPd'", "SUVd'", "SUVd'", "Ed"]
        );
        assert_eq!(
            line.trace(1),
            ["SUVu", "SUVu", "CPu", "SUVu", "SUVu", "SUVu", "SUVu", "SUVu'", "E2u"]
        );
    }

    /// Figure 14/V.92, "final training where the first CPu is not received by
    /// the digital modem": the analogue modem sees no acknowledgement in any
    /// SUVd up to the one completing after its CPu's end plus 100 ms and a
    /// round trip, and repeats CPu' until the exchange ends.
    ///
    /// The shape is the figure's exactly -- SUVu SUVu CPu, four SUVu, SUVu',
    /// three CPu', E2u, against SUVd x5, CPd, SUVd, SUVd', Ed -- but the
    /// figure is not to scale. It draws "RTD + 100 ms" across four SUVu; with
    /// SUVu 12 symbols long that span is 6 ms, not 106, so the runs of
    /// unacknowledged sequences come out 68 and 76 rather than 1 and 4. The
    /// instant the repeat starts is asserted against the clause below.
    #[test]
    fn figure_14_a_lost_cpu_is_repeated_after_100_ms_and_a_round_trip() {
        let rtd = 48.0 / FS;
        let digital = Scripted::new(Side::Digital, Context::Training, rtd, 12, 36, 0);
        let mut analogue = Scripted::new(Side::Analogue, Context::Training, rtd, 12, 36, 18);
        analogue.lose_cp = true;
        let mut line = Line::new(digital, analogue, 24);
        line.run(20_000);
        let digital_runs = runs(&line.trace(0));
        let analogue_runs = runs(&line.trace(1));
        assert_eq!(
            digital_runs,
            [
                ("SUVd".to_string(), 5),
                ("CPd".to_string(), 1),
                ("SUVd".to_string(), 76),
                ("SUVd'".to_string(), 1),
                ("Ed".to_string(), 1),
            ]
        );
        assert_eq!(
            analogue_runs,
            [
                ("SUVu".to_string(), 2),
                ("CPu".to_string(), 1),
                ("SUVu".to_string(), 4),
                ("SUVu'".to_string(), 68),
                ("CPu'".to_string(), 3),
                ("E2u".to_string(), 1),
            ]
        );
        // The repeat cannot begin before the end of the lost CPu plus 100 ms
        // plus the round trip (9.6.2.1.3), and does begin at the first
        // boundary after it.
        let window = line.ends[1].exchange.window();
        assert_eq!(window, ((0.100 + rtd) * FS).round() as u64);
        let cp_end = line.ends[1].starts[2] + 36;
        let repeat = line.ends[1]
            .trace
            .iter()
            .position(|label| label == "CPu'")
            .expect("the analogue modem repeated its CPu");
        let repeat_start = line.ends[1].starts[repeat];
        assert!(repeat_start > cp_end + window, "{repeat_start} was not past {}", cp_end + window);
        // And not much past it: one SUVd has to finish arriving after the
        // deadline before the rule fires, and the repeat then waits for our
        // own next boundary, so 12 + 12 samples is the whole of the slack.
        let late = repeat_start - (cp_end + window);
        assert!(late <= 24, "the repeat waited {late} samples past the deadline");
    }

    /// 9.6.1.1.3's "up to and including the entire CPu or SUVu sequence that
    /// is received after 100 ms plus a round-trip delay": the sequence that
    /// straddles the deadline is part of the check, so an acknowledgement in
    /// it stops the repeat that would otherwise start
    /// ([`REPEAT_WINDOW_AT_RECEPTION_END`], P4P Q12).
    #[test]
    fn the_sequence_that_completes_after_the_deadline_still_counts() {
        const { assert!(REPEAT_WINDOW_AT_RECEPTION_END) };
        let mut exchange = Exchange::new(Context::Training, 0.0, FS);
        let window = exchange.window();
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(exchange.next_sequence(24), SequenceKind::Cp);
        exchange.sequence_started(SequenceKind::Cp);
        exchange.sequence_ended(SequenceKind::Cp, 120);
        // Unacknowledged, but still inside the window: nothing yet.
        exchange.peer_suv(PeerSuv::default(), 120 + window);
        assert!(!exchange.repeating());
        // The one that crosses the deadline carries the acknowledgement, so
        // the repeat never starts.
        exchange.peer_suv(PeerSuv { ack: true, ..PeerSuv::default() }, 120 + window + 1);
        assert!(!exchange.repeating());
        assert!(exchange.peer_acked());
        assert_eq!(exchange.next_sequence(200 + window), SequenceKind::Suv);
    }

    /// The same window with the straddling sequence unacknowledged: 9.6.2.1.3
    /// then asks for repeated CPu, and they go on until the exchange ends.
    #[test]
    fn an_unacknowledged_sequence_past_the_deadline_starts_the_repeats() {
        let mut exchange = Exchange::new(Context::Training, 0.0, FS);
        let window = exchange.window();
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        exchange.sequence_started(SequenceKind::Cp);
        exchange.sequence_ended(SequenceKind::Cp, 120);
        exchange.peer_suv(PeerSuv::default(), 120 + window);
        assert_eq!(exchange.next_sequence(120 + window), SequenceKind::Suv);
        exchange.peer_suv(PeerSuv::default(), 120 + window + 1);
        assert!(exchange.repeating());
        assert_eq!(exchange.next_sequence(200 + window), SequenceKind::Cp);
        // A repeat does not restart the window: the end recorded is still the
        // first CP's, which is what lets Figure 14's three CPu' run together.
        exchange.sequence_started(SequenceKind::Cp);
        exchange.sequence_ended(SequenceKind::Cp, 300 + window);
        assert_eq!(exchange.my_cp_end(), Some(120));
        assert_eq!(exchange.next_sequence(300 + window), SequenceKind::Cp);
    }

    /// 9.6.1.1.2 asks for "a single CPd", and 9.6.1.1.3 only adds repeats when
    /// nothing acknowledged comes back. With acknowledgements arriving, the
    /// one CP is the only one.
    #[test]
    fn no_second_cp_is_sent_while_acks_arrive() {
        let mut exchange = Exchange::new(Context::Training, 0.0, FS);
        let window = exchange.window();
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        exchange.sequence_started(SequenceKind::Cp);
        exchange.sequence_ended(SequenceKind::Cp, 120);
        exchange.peer_suv(PeerSuv { ack: true, ..PeerSuv::default() }, 150);
        for step in 0..8 {
            let now = 150 + step * window;
            exchange.peer_suv(PeerSuv { ack: true, ..PeerSuv::default() }, now);
            assert!(!exchange.repeating(), "a repeat started at {now}");
        }
        // Nothing of ours has carried the acknowledge bit yet -- no CP has
        // arrived -- so 9.6.1.1.4 is not met and SUVs go on.
        assert_eq!(exchange.next_sequence(150 + 8 * window), SequenceKind::Suv);
    }

    /// 9.6.1.1.4 lists E2u beside an acknowledged sequence: "it has received a
    /// CPu or SUVu sequence with the acknowledgement bit set or E2u".
    #[test]
    fn an_e_counts_as_an_acknowledgement() {
        let mut exchange = Exchange::new(Context::Training, 0.0, FS);
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        exchange.peer_cp(PeerCp::default(), 30);
        // Our next sequence carries the acknowledge bit, which is half of
        // 9.6.1.1.4.
        assert!(exchange.sequence_started(SequenceKind::Cp));
        exchange.sequence_ended(SequenceKind::Cp, 120);
        assert!(exchange.sent_ack());
        assert_eq!(exchange.next_sequence(120), SequenceKind::Suv);
        exchange.peer_e(130);
        assert!(exchange.peer_acked());
        assert_eq!(exchange.next_sequence(140), SequenceKind::E);
    }

    /// Table 24/V.92 is the short CPu, and it carries the same acknowledge bit
    /// at 33 and the same drn: sending one discharges the single-CP obligation
    /// of 9.6.2.1.2, and receiving one is receiving a CP (P4P 6.1 D4-2).
    #[test]
    fn a_cpus_counts_as_a_cpu() {
        let mut ours = Exchange::new(Context::Renegotiation, 0.0, FS);
        ours.peer_suv(PeerSuv::default(), 0);
        ours.sequence_started(SequenceKind::Suv);
        ours.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(ours.next_sequence(24), SequenceKind::Cp);
        ours.sequence_started(SequenceKind::Cpus);
        ours.sequence_ended(SequenceKind::Cpus, 60);
        assert_eq!(ours.my_cp_end(), Some(60));
        assert_eq!(ours.next_sequence(60), SequenceKind::Suv);

        let mut theirs = Exchange::new(Context::Renegotiation, 0.0, FS);
        theirs.peer_suv(PeerSuv::default(), 0);
        theirs.peer_cp(PeerCp::default(), 30);
        assert!(theirs.got_peer_cp());
        assert!(theirs.sequence_started(SequenceKind::Suv));
    }

    /// 9.6.1.1.2 sets the acknowledge bit in "subsequent" sequences, and
    /// 8.8.3 makes a group of CPd and CPd' "all contain identical
    /// information": a CP arriving in the middle of a sequence cannot change
    /// the bit that sequence is already carrying.
    #[test]
    fn the_ack_bit_changes_only_at_a_sequence_boundary() {
        let mut exchange = Exchange::new(Context::Training, 0.0, FS);
        exchange.peer_suv(PeerSuv::default(), 0);
        assert!(!exchange.sequence_started(SequenceKind::Suv));
        // Mid-sequence.
        exchange.peer_cp(PeerCp::default(), 12);
        assert_eq!(exchange.sending(), Some((SequenceKind::Suv, false)));
        exchange.sequence_ended(SequenceKind::Suv, 24);
        assert!(!exchange.sent_ack());
        // The next boundary, and only then.
        let kind = exchange.next_sequence(24);
        assert!(exchange.sequence_started(kind));
        assert_eq!(exchange.sending(), Some((kind, true)));
        exchange.sequence_ended(kind, 120);
        assert!(exchange.sent_ack());
    }

    /// A group of SUVs differs in the acknowledge bit and nothing else: every
    /// sequence of the run is the same kind, and the bit goes false to true
    /// once and stays (8.7.5, 8.8.5).
    #[test]
    fn a_group_of_suvs_differs_only_in_the_acknowledge_bit() {
        let mut exchange = Exchange::new(Context::Training, 0.0, FS);
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        exchange.sequence_started(SequenceKind::Cp);
        exchange.sequence_ended(SequenceKind::Cp, 120);
        let mut group = Vec::new();
        for step in 0..6u64 {
            let now = 120 + step * 24;
            if step == 3 {
                exchange.peer_cp(PeerCp::default(), now - 1);
            }
            let kind = exchange.next_sequence(now);
            let ack = exchange.sequence_started(kind);
            group.push((kind, ack));
            exchange.sequence_ended(kind, now + 24);
        }
        assert!(group.iter().all(|(kind, _)| *kind == SequenceKind::Suv));
        let acks: Vec<bool> = group.iter().map(|(_, ack)| *ack).collect();
        assert_eq!(acks, [false, false, false, true, true, true]);
    }

    /// The window is 100 ms plus a round-trip delay, and on this project's
    /// VoIP path that round trip is about 1.5 s (memory "VoIP line round
    /// trip"). A modem that used 100 ms alone would repeat its CP fourteen
    /// times before the first acknowledgement could possibly arrive.
    #[test]
    fn with_a_1_5_s_round_trip_no_early_repeat_happens() {
        let mut exchange = Exchange::new(Context::Training, 1.5, FS);
        assert_eq!(exchange.window(), 12_800);
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        exchange.sequence_started(SequenceKind::Cp);
        exchange.sequence_ended(SequenceKind::Cp, 120);
        // A whole second of unacknowledged SUVd, one every 12 samples.
        let mut at = 120;
        while at < 120 + 8000 {
            at += 12;
            exchange.peer_suv(PeerSuv::default(), at);
            assert!(!exchange.repeating(), "a repeat started at {at}");
        }
        // And on past the deadline, where it does fire.
        while at <= 120 + 12_800 {
            at += 12;
            exchange.peer_suv(PeerSuv::default(), at);
        }
        assert!(exchange.repeating());
    }

    /// 9.8 is the only clause that defines a silent period, so in training and
    /// in a parameter exchange bit 32 goes out clear and a received 1 is
    /// swallowed: the trace is the one the clear run gives, box for box
    /// ([`SILENCE_ONLY_IN_RENEGOTIATION`], P4P Q15, 9.9.2.1.2).
    #[test]
    fn a_silence_request_in_training_is_ignored() {
        for context in [Context::Training, Context::FastExchange] {
            let mut traces = Vec::new();
            for silence in [false, true] {
                let rtd = 12.0 / FS;
                let digital = Scripted::new(Side::Digital, context, rtd, 36, 96, 0);
                let mut analogue = Scripted::new(Side::Analogue, context, rtd, 24, 96, 12);
                analogue.flags.silence = silence;
                let mut line = Line::new(digital, analogue, 6);
                line.ends[0].exchange.request_silence(true);
                line.run(4000);
                assert!(!line.ends[0].exchange.peer_silence(), "{context:?} reported a request");
                assert!(!line.ends[0].exchange.peer_asked_silence());
                assert!(
                    !line.ends[0].exchange.silence_requested(),
                    "{context:?} let our own bit 32 be set"
                );
                traces.push((
                    line.trace(0).iter().map(ToString::to_string).collect::<Vec<_>>(),
                    line.trace(1).iter().map(ToString::to_string).collect::<Vec<_>>(),
                ));
            }
            assert_eq!(traces[0], traces[1], "{context:?} changed with bit 32");
        }
    }

    /// In a rate renegotiation the same bit is real: it is reported as it
    /// stands, latched for the "if bit 32 is set in either" test of 9.8.1.1.2,
    /// and our own may be set.
    #[test]
    fn a_silence_request_is_reported_in_a_renegotiation() {
        let mut exchange = Exchange::new(Context::Renegotiation, 0.0, FS);
        exchange.request_silence(true);
        assert!(exchange.silence_requested());
        exchange.peer_suv(PeerSuv { silence: true, ..PeerSuv::default() }, 100);
        assert!(exchange.peer_silence());
        assert!(exchange.peer_asked_silence());
        // 9.8.1.1.4 waits for "an SUVu with bit 32 clear", so the reported bit
        // follows the latest sequence while the latch remembers the request.
        exchange.peer_suv(PeerSuv::default(), 200);
        assert!(!exchange.peer_silence());
        assert!(exchange.peer_asked_silence());
    }

    /// Bit 33 acknowledges the SUV handshake inside the silence branch and a
    /// CP outside it, so leaving the silence starts the exchange again: the
    /// SUVd after R-bar-t is plain, and the one CP is owed once more
    /// ([`SILENCE_CLEARS_ACK_STATE`], P4D Q10, Figures 16 to 18).
    #[test]
    fn leaving_a_silent_period_clears_the_acknowledge_state() {
        let mut exchange = Exchange::new(Context::Renegotiation, 0.0, FS);
        exchange.request_silence(true);
        exchange.peer_suv(PeerSuv { ack: true, silence: true, ..PeerSuv::default() }, 0);
        exchange.peer_cp(PeerCp { ack: true }, 20);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        assert!(exchange.sent_ack() && exchange.peer_acked() && exchange.got_peer_cp());

        exchange.silence_ended();
        assert!(!exchange.sent_ack());
        assert!(!exchange.peer_acked());
        assert!(!exchange.got_peer_cp());
        assert!(!exchange.silence_requested());
        assert!(!exchange.peer_silence());
        assert_eq!(exchange.my_cp_end(), None);
        assert_eq!(exchange.window(), 800);
        // A plain SUVd, then the single CPd, exactly as after Phase 4's entry.
        assert_eq!(exchange.next_sequence(100), SequenceKind::Suv);
        assert!(!exchange.sequence_started(SequenceKind::Suv));
        exchange.sequence_ended(SequenceKind::Suv, 124);
        exchange.peer_suv(PeerSuv::default(), 130);
        assert_eq!(exchange.next_sequence(148), SequenceKind::Cp);
    }

    /// Table 27 bit 26, "the analogue modem wishes the digital modem to wait
    /// for a CPu before sending a CPd. The digital modem is not required to
    /// comply": complying holds the CP back until the peer's arrives, not
    /// complying sends it after the first peer SUV, and a peer that asks and
    /// then sends nothing cannot hold us past one repeat window
    /// ([`HONOUR_WAIT_FOR_CP`]).
    #[test]
    fn a_peer_that_asks_us_to_wait_for_its_cp_delays_our_cp() {
        let asked = PeerSuv { wait_for_cp: true, ..PeerSuv::default() };

        // Asked, and the peer's CP arrives: no CP of ours goes before it.
        let mut waiting = Exchange::new(Context::Training, 0.0, FS);
        waiting.peer_suv(asked, 0);
        waiting.sequence_started(SequenceKind::Suv);
        waiting.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(waiting.next_sequence(24), SequenceKind::Suv);
        assert_eq!(waiting.next_sequence(48), SequenceKind::Suv);
        waiting.peer_cp(PeerCp::default(), 60);
        assert_eq!(waiting.next_sequence(72), SequenceKind::Cp);

        // Not asked: the CP goes at the first boundary after the peer's SUV.
        let mut prompt = Exchange::new(Context::Training, 0.0, FS);
        prompt.peer_suv(PeerSuv::default(), 0);
        prompt.sequence_started(SequenceKind::Suv);
        prompt.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(prompt.next_sequence(24), SequenceKind::Cp);

        // Asked, and no CP ever comes: the wait is bounded by one window, so
        // complying costs nothing.
        let mut stalled = Exchange::new(Context::Training, 0.0, FS);
        let window = stalled.window();
        stalled.peer_suv(asked, 0);
        stalled.sequence_started(SequenceKind::Suv);
        stalled.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(stalled.next_sequence(24), SequenceKind::Suv);
        assert_eq!(stalled.next_sequence(24 + window), SequenceKind::Suv);
        assert_eq!(stalled.next_sequence(25 + window), SequenceKind::Cp);

        // And an SUV, an E or a B1 is never held back by the bit.
        let mut ending = Exchange::new(Context::Training, 0.0, FS);
        ending.peer_suv(PeerSuv { ack: true, ..asked }, 0);
        ending.peer_cp(PeerCp { ack: true }, 10);
        assert!(ending.sequence_started(SequenceKind::Suv));
        ending.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(ending.next_sequence(24), SequenceKind::E);
    }

    /// 9.8.2.1.3 releases the CP only "after having transmitted an SUVu and
    /// received an SUVd", so a peer SUV that arrives while TRN2 is still
    /// running does not put a CP first.
    #[test]
    fn the_cp_waits_for_one_of_our_own_suvs_as_well() {
        let mut exchange = Exchange::new(Context::Renegotiation, 0.0, FS);
        exchange.peer_suv(PeerSuv::default(), 0);
        exchange.sequence_started(SequenceKind::Trn2);
        exchange.sequence_ended(SequenceKind::Trn2, 2040);
        assert_eq!(exchange.next_sequence(2040), SequenceKind::Suv);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 2064);
        assert_eq!(exchange.next_sequence(2064), SequenceKind::Cp);
    }

    /// 9.6.1.1.4 wants both halves: an acknowledged sequence sent *and* one
    /// received. Neither alone ends the exchange.
    #[test]
    fn e_follows_only_an_acknowledged_sequence_both_ways() {
        // Received but not sent: our CP has not been acknowledged.
        let mut one_way = Exchange::new(Context::Training, 0.0, FS);
        one_way.peer_suv(PeerSuv { ack: true, ..PeerSuv::default() }, 0);
        one_way.sequence_started(SequenceKind::Suv);
        one_way.sequence_ended(SequenceKind::Suv, 24);
        assert!(one_way.peer_acked() && !one_way.sent_ack());
        assert_ne!(one_way.next_sequence(24), SequenceKind::E);

        // Sent but not received.
        let mut other_way = Exchange::new(Context::Training, 0.0, FS);
        other_way.peer_suv(PeerSuv::default(), 0);
        other_way.peer_cp(PeerCp::default(), 10);
        assert!(other_way.sequence_started(SequenceKind::Suv));
        other_way.sequence_ended(SequenceKind::Suv, 24);
        assert!(other_way.sent_ack() && !other_way.peer_acked());
        assert_ne!(other_way.next_sequence(24), SequenceKind::E);
    }

    /// After E comes B1 (9.6.1.1.5, 9.6.2.1.5), with FB1u first only in a
    /// parameter exchange (9.6.2.1.5), and CPd bit 29 offered only in initial
    /// training (P4D Q5).
    #[test]
    fn the_context_says_what_surrounds_the_exchange() {
        assert_eq!(Context::Training.modulation(), Modulation::Trn2);
        assert_eq!(Context::Renegotiation.modulation(), Modulation::Trn2);
        assert_eq!(Context::FastExchange.modulation(), Modulation::Data);
        assert!(Context::Training.may_extend_e2u());
        assert!(!Context::Renegotiation.may_extend_e2u());
        assert!(!Context::FastExchange.may_extend_e2u());
        assert!(Context::FastExchange.fb1_before_b1());
        assert!(!Context::Training.fb1_before_b1());
        assert!(Context::Renegotiation.defines_silence());
        assert!(!Context::Training.defines_silence());

        let mut exchange = Exchange::new(Context::FastExchange, 0.0, FS);
        exchange.peer_suv(PeerSuv { ack: true, ..PeerSuv::default() }, 0);
        exchange.peer_cp(PeerCp { ack: true }, 10);
        exchange.sequence_started(SequenceKind::Suv);
        exchange.sequence_ended(SequenceKind::Suv, 24);
        assert_eq!(exchange.next_sequence(24), SequenceKind::E);
        exchange.sequence_started(SequenceKind::E);
        exchange.sequence_ended(SequenceKind::E, 36);
        assert!(exchange.finished());
        assert_eq!(exchange.next_sequence(36), SequenceKind::B1);
    }
}
