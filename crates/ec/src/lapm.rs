//! The LAPM state machine (V.42 8.3 to 8.9).
//!
//! Establishment, release, and numbered information transfer with go-back-N
//! recovery. Time is supplied by the caller through [`Lapm::tick`] rather than
//! read from a clock, so the whole protocol is testable at whatever speed a
//! test wants.
//!
//! Not yet covered: break transfer (V.42 8.13), the multi-frame form of
//! selective reject (8.4.5.2) and the optional T402/T403 timers. XID is
//! covered, but not here: what it negotiates spans all three layers --
//! compression above, frame check sequence below -- so it is driven by the
//! stack that owns them.

use std::collections::{BTreeMap, VecDeque};

use crate::frame::{Frame, Kind, MODULUS, Role};

/// Maximum octets in an information field. V.42 9.2.3 sets the default at 128
/// and allows XID to negotiate it per direction.
pub const DEFAULT_N401: usize = 128;

/// Outstanding I frames allowed. V.42 9.2.4 sets the default at 15.
pub const DEFAULT_K: u8 = 15;

/// Acknowledgement timer in milliseconds, when the rate is not known.
///
/// V.42 9.2.1 defines what T401 is for but gives no default value at all, only
/// pointing at Appendix IV for the factors involved. Three seconds is
/// comfortable at 300 bps, where a full 128-octet frame alone takes over four
/// seconds to send, and is the sort of value real modems used at low speed.
///
/// It is far too long at any rate this modem actually runs V.42 at, which is
/// why [`t401_for`] exists.
pub const DEFAULT_T401_MS: u32 = 3000;

/// The acknowledgement timer for a given line rate (V.42 Appendix IV).
///
/// The appendix does not give a value, it gives a sum: T401 must be at least
/// "Ta + Tb + Tc + Td + Te + Tf" -- the propagation each way, the processing at
/// each end, the time to finish whatever frame was already going out, and the
/// time to send the acknowledgement. Only two of those are large, and both are
/// the line rate: a full information frame in progress, and the supervisory
/// frame that answers it.
///
/// Three seconds regardless of rate is what this was, and it is nearly seven
/// times the transmission terms at 2400 bit/s. That does not sound like much
/// until the retransmission limit multiplies it: a far end that never answers
/// a SABME costs three seconds an attempt, and on a real call that meant nine
/// seconds during which the terminal had been told nothing at all, on a
/// connection that was up and working at 2400.
///
/// The propagation allowance is generous on purpose. A call carried over VoIP
/// crosses a jitter buffer in each direction and half a second between them is
/// not unusual.
pub fn t401_for(bits_per_second: u32) -> u32 {
    /// Ta + Te, the two propagation delays.
    const PROPAGATION_MS: u32 = 500;
    /// Tc: the longest frame that could already be going out, in bits -- the
    /// information field plus address, control and check sequence.
    const FRAME_BITS: u32 = (DEFAULT_N401 as u32 + 6) * 8;
    /// Td: the supervisory frame that acknowledges it.
    const ACK_BITS: u32 = 6 * 8;

    let rate = bits_per_second.max(1);
    PROPAGATION_MS + (FRAME_BITS + ACK_BITS) * 1000 / rate
}

/// Retransmission limit.
///
/// V.42 9.2.2 specifies no default, requiring only a minimum of 1. Appendix
/// III.2 recommends "a relatively large value (e.g. 16)", and says why: by the
/// time establishment is being attempted the detection phase has already
/// finished, so each end knows the other does LAPM, and giving up early on a
/// modem known to be there throws away a connection that noise alone was
/// spoiling. The appendix's own caveat -- keep it small if the detection phase
/// was omitted, to fall back quickly to a modem that does not do this at all
/// -- does not apply here, because this modem never omits it.
pub const DEFAULT_N400: u32 = 16;

/// Retransmissions to allow when the detection phase did not confirm anything.
///
/// The other half of Appendix III.2: a large N400 is right when detection has
/// established that the far end does LAPM, and wrong when it has not, because
/// then every retry is time spent talking at a modem in a language it may not
/// speak. The appendix asks for a small value where the detection phase is
/// omitted, "to accommodate fallback operation with non-error-correcting DCEs
/// and with DCEs which support only the alternative protocol". A far end that
/// named LAPM in V.8 is good evidence and not that confirmation.
pub const UNCONFIRMED_N400: u32 = 3;

/// Tunable protocol parameters (V.42 clause 9.2).
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub n401: usize,
    pub k: u8,
    pub t401_ms: u32,
    pub n400: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            n401: DEFAULT_N401,
            k: DEFAULT_K,
            t401_ms: DEFAULT_T401_MS,
            n400: DEFAULT_N400,
        }
    }
}

/// Connection state (V.42 8.3, 8.7, 8.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    /// SABME sent, waiting for UA or DM.
    AwaitingEstablishment,
    /// DISC sent, waiting for UA or DM.
    AwaitingRelease,
    Connected,
}

/// Why the connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// This end asked to disconnect.
    Local,
    /// The peer sent DISC.
    Peer,
    /// N400 attempts produced no response (V.42 8.3.2.2, 8.7.3).
    NoResponse,
    /// The peer refused or is disconnected.
    Refused,
}

/// What the state machine reports upward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The error-corrected connection is up.
    Connected,
    /// The connection ended.
    Released(Cause),
    /// Data recovered from the peer, in order.
    Data(Vec<u8>),
    /// The peer re-established mid-connection, so unacknowledged data was lost
    /// (V.42 8.2.4.3: previously-transmitted I frames are discarded).
    Reset,
}

/// One LAPM entity.
#[derive(Debug)]
pub struct Lapm {
    role: Role,
    dlci: u8,
    params: Params,
    state: State,

    /// Send state variable (V.42 8.2.3.2.2).
    vs: u8,
    /// Acknowledge state variable (V.42 8.2.3.2.3).
    va: u8,
    /// Receive state variable (V.42 8.2.3.2.5).
    vr: u8,

    /// Data from the local DTE waiting to go out.
    pending: VecDeque<Vec<u8>>,
    /// I frames sent but not yet acknowledged, oldest first.
    unacked: VecDeque<(u8, Vec<u8>)>,
    /// Frames queued for transmission.
    out: VecDeque<(Frame, Kind)>,
    /// Events for the control function.
    events: VecDeque<Event>,

    /// The peer told us to stop sending (RNR).
    peer_busy: bool,
    /// A REJ has been sent and not yet resolved, so do not send another
    /// (V.42 8.4.4: only one reject exception at a time).
    reject_sent: bool,
    /// Whether the single-selective-reject procedure was agreed (8.4.5.1).
    srej: bool,
    /// I frames that arrived ahead of V(R), held until the gap closes.
    ///
    /// The whole of what selective reject buys. Go-back-N throws these away
    /// and asks for all of them again; 8.2.4.8.1 says "I frames that may have
    /// been transmitted following the I frame indicated by the SREJ frame
    /// shall not be retransmitted", which is only possible if the receiver
    /// kept them.
    held: BTreeMap<u8, Vec<u8>>,
    /// An in-sequence I frame arrived and has not yet been acknowledged.
    ack_pending: bool,
    /// The timer-recovery condition (V.42 8.5.3): T401 expired with frames
    /// outstanding, so the peer has been polled and its reply decides what
    /// needs sending again.
    timer_recovery: bool,

    timer: Option<u32>,
    retries: u32,
}

impl Lapm {
    pub fn new(role: Role, dlci: u8, params: Params) -> Self {
        Self {
            role,
            dlci,
            params,
            state: State::Disconnected,
            vs: 0,
            va: 0,
            vr: 0,
            pending: VecDeque::new(),
            unacked: VecDeque::new(),
            out: VecDeque::new(),
            events: VecDeque::new(),
            peer_busy: false,
            reject_sent: false,
            srej: false,
            held: BTreeMap::new(),
            ack_pending: false,
            timer_recovery: false,
            timer: None,
            retries: 0,
        }
    }

    /// Change the retransmission limit before anything has been established.
    ///
    /// How much patience is warranted is not known when the entity is built:
    /// it depends on how the detection phase came out, which happens later.
    pub fn set_retransmissions(&mut self, n400: u32) {
        self.params.n400 = n400;
    }

    /// Use the selective retransmission procedure (V.42 8.4.5.1).
    ///
    /// Optional, and only after both ends have said so in XID: 8.4.5.1 has an
    /// end that did not agree treat an SREJ as an unrecognized control field,
    /// which under 8.5.5 ends the connection.
    pub fn set_selective_reject(&mut self, on: bool) {
        self.srej = on;
    }

    /// Whether selective retransmission is in use.
    pub fn selective_reject(&self) -> bool {
        self.srej
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn is_connected(&self) -> bool {
        self.state == State::Connected
    }

    /// Frames to hand to the HDLC layer.
    pub fn poll_transmit(&mut self) -> Option<(Frame, Kind)> {
        self.pump();
        self.out.pop_front()
    }

    /// Events for the control function.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Ask for an error-corrected connection (V.42 8.3).
    ///
    /// Calling this while already connected re-establishes, which V.42 8.6
    /// provides for: the sequence variables are reset at both ends and
    /// unacknowledged frames are discarded. Only an establishment already in
    /// progress is ignored, since a second SABME would achieve nothing.
    pub fn connect(&mut self) {
        if self.state == State::AwaitingEstablishment {
            return;
        }
        self.reset_variables();
        self.state = State::AwaitingEstablishment;
        self.retries = 0;
        // The P bit is set so the peer's UA can be matched to this request.
        self.send(Frame::Sabme { poll: true }, Kind::Command);
        self.start_timer();
    }

    /// Ask to release the connection (V.42 8.7.2).
    pub fn disconnect(&mut self) {
        if self.state == State::Disconnected {
            return;
        }
        self.pending.clear();
        self.unacked.clear();
        self.state = State::AwaitingRelease;
        self.retries = 0;
        // V.42 8.7.2 note: DISC always carries P set to 1, so a DM response
        // cannot be misread as unsolicited.
        self.send(Frame::Disc { poll: true }, Kind::Command);
        self.start_timer();
    }

    /// Bytes accepted from above and not yet put in a frame.
    ///
    /// What anyone upstream needs to know before handing over more. There is
    /// no back pressure in `send_data` -- it takes whatever it is given -- so
    /// the only thing stopping a caller filling memory with a file is a caller
    /// that asks first.
    pub fn queued(&self) -> usize {
        self.pending.iter().map(Vec::len).sum()
    }

    /// Queue data for the peer. Split to fit N401 (V.42 9.2.3).
    pub fn send_data(&mut self, data: &[u8]) {
        for chunk in data.chunks(self.params.n401) {
            self.pending.push_back(chunk.to_vec());
        }
    }

    /// Advance timers by `dt_ms` (V.42 8.3.2.2, 8.7.3, 8.4.8).
    pub fn tick(&mut self, dt_ms: u32) {
        let Some(remaining) = self.timer else { return };
        if remaining > dt_ms {
            self.timer = Some(remaining - dt_ms);
            return;
        }
        self.timer = None;
        self.retries += 1;
        if self.retries >= self.params.n400 {
            self.fail();
            return;
        }
        match self.state {
            State::AwaitingEstablishment => {
                self.send(Frame::Sabme { poll: true }, Kind::Command);
                self.start_timer();
            }
            State::AwaitingRelease => {
                self.send(Frame::Disc { poll: true }, Kind::Command);
                self.start_timer();
            }
            State::Connected => {
                // V.42 8.5.3: enter the timer-recovery condition and poll,
                // rather than blindly resending. The acknowledgement may simply
                // have been lost, and the reply says what is really missing.
                //
                // This is the only way a lost *final* frame is recovered: with
                // nothing following it, the peer never sees anything out of
                // sequence and so never sends a reject.
                self.timer_recovery = true;
                self.send(Frame::Rr { nr: self.vr, pf: true }, Kind::Command);
                self.ack_pending = false;
                self.start_timer();
            }
            State::Disconnected => {}
        }
    }

    /// Handle a frame from the peer.
    pub fn receive(&mut self, frame: Frame, kind: Kind) {
        match &frame {
            // Set-mode and release commands are handled in every state
            // (V.42 8.8, 8.9).
            Frame::Sabme { poll } => self.on_sabme(*poll),
            Frame::Disc { poll } => self.on_disc(*poll),
            Frame::Ua { final_bit } => self.on_ua(*final_bit),
            Frame::Dm { final_bit } => self.on_dm(*final_bit),
            _ if self.state != State::Connected => {
                // V.42 8.8: everything else is discarded while disconnected.
            }
            Frame::I { ns, nr, poll, info } => self.on_i(*ns, *nr, *poll, info.clone()),
            Frame::Rr { nr, pf } => self.on_rr(*nr, *pf, kind),
            Frame::Rnr { nr, pf } => self.on_rnr(*nr, *pf, kind),
            Frame::Rej { nr, pf } => self.on_rej(*nr, *pf, kind),
            Frame::Srej { nr } => self.on_srej(*nr),
            // SREJ, UI, XID, TEST and FRMR are not implemented yet; ignoring
            // them is safe because the peer's timers will recover.
            _ => {}
        }
    }

    // -- unnumbered handling ------------------------------------------------

    fn on_sabme(&mut self, poll: bool) {
        let was_connected = self.state == State::Connected;
        // V.42 8.2.4.3: acceptance resets V(S), V(A) and V(R), and previously
        // transmitted unacknowledged I frames are discarded.
        self.reset_variables();
        self.send(Frame::Ua { final_bit: poll }, Kind::Response);
        self.state = State::Connected;
        self.stop_timer();
        if was_connected {
            self.events.push_back(Event::Reset);
        } else {
            self.events.push_back(Event::Connected);
        }
    }

    fn on_disc(&mut self, poll: bool) {
        if self.state == State::Disconnected {
            // V.42 8.8: answer with DM rather than UA.
            self.send(Frame::Dm { final_bit: poll }, Kind::Response);
            return;
        }
        self.send(Frame::Ua { final_bit: poll }, Kind::Response);
        self.enter_disconnected(Cause::Peer);
    }

    fn on_ua(&mut self, final_bit: bool) {
        match self.state {
            State::AwaitingEstablishment if final_bit => {
                self.reset_variables();
                self.state = State::Connected;
                self.stop_timer();
                self.events.push_back(Event::Connected);
            }
            State::AwaitingRelease if final_bit => self.enter_disconnected(Cause::Local),
            _ => {}
        }
    }

    fn on_dm(&mut self, final_bit: bool) {
        match self.state {
            // V.42 8.9.3: a DM with F clear colliding with a set-mode command
            // is ignored.
            State::AwaitingEstablishment if final_bit => {
                self.enter_disconnected(Cause::Refused)
            }
            State::AwaitingRelease if final_bit => self.enter_disconnected(Cause::Local),
            State::Connected => self.enter_disconnected(Cause::Refused),
            _ => {}
        }
    }

    // -- numbered handling --------------------------------------------------

    fn on_i(&mut self, ns: u8, nr: u8, poll: bool, info: Vec<u8>) {
        self.acknowledge(nr);
        if ns == self.vr {
            self.deliver(info);
        } else if self.srej {
            // Out of sequence, and the frames after the missing one are worth
            // keeping: only the one that was lost has to be asked for
            // (8.2.4.8.1). Bounded by the window, so that a sequence number
            // from nowhere cannot make this grow.
            let top = (self.vr + u8::min(self.params.k, MODULUS - 1)) % MODULUS;
            if in_window(self.vr, ns, top) {
                self.held.insert(ns, info);
            }
            if !self.reject_sent {
                self.reject_sent = true;
                // 8.2.4.8.1: the P/F bit of an SREJ is always 0, and its N(R)
                // asks for one frame rather than acknowledging any.
                self.send(Frame::Srej { nr: self.vr }, Kind::Command);
            }
        } else if !self.reject_sent {
            // Out of sequence: ask for everything from V(R) again
            // (V.42 8.4.4). Only one outstanding reject at a time.
            self.reject_sent = true;
            self.send(Frame::Rej { nr: self.vr, pf: false }, Kind::Command);
        }
        if poll {
            // V.42 8.4.2.1: a command with P set demands an immediate response.
            self.send(Frame::Rr { nr: self.vr, pf: true }, Kind::Response);
            self.ack_pending = false;
        }
    }

    /// Take an in-sequence frame, and everything its arrival unblocks.
    ///
    /// The SREJ exception clears here too: 8.2.4.8.1 clears it "upon receipt
    /// of the I frame with an N(S) equal to the N(R) of the SREJ frame", and
    /// that N(R) was V(R), which is the frame being taken.
    fn deliver(&mut self, info: Vec<u8>) {
        self.vr = (self.vr + 1) % MODULUS;
        self.reject_sent = false;
        self.ack_pending = true;
        if !info.is_empty() {
            self.events.push_back(Event::Data(info));
        }
        while let Some(next) = self.held.remove(&self.vr) {
            self.vr = (self.vr + 1) % MODULUS;
            if !next.is_empty() {
                self.events.push_back(Event::Data(next));
            }
        }
    }

    /// V.42 8.4.5.1: send again the one I frame the peer asked for.
    fn on_srej(&mut self, nr: u8) {
        self.peer_busy = false;
        // "The N(R) of the SREJ frame does not indicate acknowledgement of any
        // I frames" (8.2.4.8.1), so V(A) does not move -- which is the whole
        // difference between this and a reject.
        let Some((ns, info)) = self.unacked.iter().find(|(ns, _)| *ns == nr).cloned() else {
            return;
        };
        self.out.push_back((
            Frame::I { ns, nr: self.vr, poll: false, info },
            Kind::Command,
        ));
        self.ack_pending = false;
        self.start_timer();
    }

    fn on_rr(&mut self, nr: u8, pf: bool, kind: Kind) {
        self.peer_busy = false;
        self.acknowledge(nr);
        self.resolve_timer_recovery(pf, kind);
        self.answer_poll(pf, kind);
    }

    fn on_rnr(&mut self, nr: u8, pf: bool, kind: Kind) {
        self.acknowledge(nr);
        self.peer_busy = true;
        self.resolve_timer_recovery(pf, kind);
        self.answer_poll(pf, kind);
    }

    /// Act on the reply to a timer-recovery poll (V.42 8.5.3).
    ///
    /// The peer's N(R) has already been applied, so anything still
    /// unacknowledged never arrived and is sent again.
    fn resolve_timer_recovery(&mut self, pf: bool, kind: Kind) {
        if !self.timer_recovery || !pf || kind != Kind::Response {
            return;
        }
        self.timer_recovery = false;
        self.retries = 0;
        if self.va != self.vs {
            self.retransmit_from(self.va);
        }
    }

    fn on_rej(&mut self, nr: u8, pf: bool, kind: Kind) {
        self.peer_busy = false;
        self.acknowledge(nr);
        // Go back to N(R): everything still unacknowledged is sent again.
        self.retransmit_from(nr);
        self.answer_poll(pf, kind);
    }

    /// A command with P set requires a response with F set (V.42 8.4.2.1).
    fn answer_poll(&mut self, pf: bool, kind: Kind) {
        if pf && kind == Kind::Command {
            self.send(Frame::Rr { nr: self.vr, pf: true }, Kind::Response);
            self.ack_pending = false;
        }
    }

    /// Retire acknowledged frames. N(R) is valid when V(A) <= N(R) <= V(S)
    /// modulo 128 (V.42 8.2.3.2.3).
    fn acknowledge(&mut self, nr: u8) {
        if !in_window(self.va, nr, self.vs) {
            return;
        }
        while self.va != nr {
            self.unacked.pop_front();
            self.va = (self.va + 1) % MODULUS;
        }
        if self.va == self.vs {
            // Everything is acknowledged, so nothing is outstanding to time.
            self.stop_timer();
            self.retries = 0;
        } else {
            self.start_timer();
        }
    }

    fn retransmit_from(&mut self, nr: u8) {
        if !in_window(self.va, nr, self.vs) {
            return;
        }
        // Re-send from the buffer without disturbing V(S): the frames keep the
        // sequence numbers they were first given.
        let frames: Vec<(u8, Vec<u8>)> = self
            .unacked
            .iter()
            .filter(|(ns, _)| in_window(nr, *ns, self.vs))
            .cloned()
            .collect();
        for (ns, info) in frames {
            self.out.push_back((
                Frame::I { ns, nr: self.vr, poll: false, info },
                Kind::Command,
            ));
        }
        self.ack_pending = false;
        self.start_timer();
    }

    /// Send whatever the window allows, then any outstanding acknowledgement.
    fn pump(&mut self) {
        if self.state != State::Connected {
            return;
        }
        while !self.peer_busy
            && !self.pending.is_empty()
            && outstanding(self.va, self.vs) < self.params.k
        {
            let info = self.pending.pop_front().expect("checked non-empty");
            let ns = self.vs;
            self.unacked.push_back((ns, info.clone()));
            self.vs = (self.vs + 1) % MODULUS;
            self.out.push_back((
                // N(R) rides along, which acknowledges received frames for free.
                Frame::I { ns, nr: self.vr, poll: false, info },
                Kind::Command,
            ));
            self.ack_pending = false;
            self.start_timer();
        }
        if self.ack_pending {
            self.ack_pending = false;
            self.send(Frame::Rr { nr: self.vr, pf: false }, Kind::Response);
        }
    }

    // -- housekeeping -------------------------------------------------------

    fn send(&mut self, frame: Frame, kind: Kind) {
        self.out.push_back((frame, kind));
    }

    fn reset_variables(&mut self) {
        self.vs = 0;
        self.va = 0;
        self.vr = 0;
        self.held.clear();
        self.unacked.clear();
        self.peer_busy = false;
        self.reject_sent = false;
        self.ack_pending = false;
        self.timer_recovery = false;
    }

    fn enter_disconnected(&mut self, cause: Cause) {
        self.state = State::Disconnected;
        self.stop_timer();
        self.pending.clear();
        self.unacked.clear();
        self.events.push_back(Event::Released(cause));
    }

    fn fail(&mut self) {
        self.enter_disconnected(Cause::NoResponse);
    }

    fn start_timer(&mut self) {
        self.timer = Some(self.params.t401_ms);
    }

    fn stop_timer(&mut self) {
        self.timer = None;
    }

    /// Address this entity uses when encoding.
    pub fn role(&self) -> Role {
        self.role
    }

    pub fn dlci(&self) -> u8 {
        self.dlci
    }
}

/// Number of outstanding I frames, modulo 128.
fn outstanding(va: u8, vs: u8) -> u8 {
    vs.wrapping_sub(va) % MODULUS
}

/// True when `n` lies in `[low, high]` going forward modulo 128.
fn in_window(low: u8, n: u8, high: u8) -> bool {
    let span = high.wrapping_sub(low) % MODULUS;
    let offset = n.wrapping_sub(low) % MODULUS;
    offset <= span
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::DLCI_DATA;

    fn pair() -> (Lapm, Lapm) {
        (
            Lapm::new(Role::Originator, DLCI_DATA, Params::default()),
            Lapm::new(Role::Answerer, DLCI_DATA, Params::default()),
        )
    }

    /// Move every queued frame from `from` to `to`, returning how many crossed.
    fn deliver(from: &mut Lapm, to: &mut Lapm) -> usize {
        let mut n = 0;
        while let Some((frame, kind)) = from.poll_transmit() {
            to.receive(frame, kind);
            n += 1;
        }
        n
    }

    /// Exchange until both ends stop talking.
    fn settle(a: &mut Lapm, b: &mut Lapm) {
        for _ in 0..64 {
            let moved = deliver(a, b) + deliver(b, a);
            if moved == 0 {
                return;
            }
        }
        panic!("the two ends never stopped exchanging frames");
    }

    fn events(l: &mut Lapm) -> Vec<Event> {
        let mut v = Vec::new();
        while let Some(e) = l.poll_event() {
            v.push(e);
        }
        v
    }

    fn data_from(l: &mut Lapm) -> Vec<u8> {
        events(l)
            .into_iter()
            .filter_map(|e| match e {
                Event::Data(d) => Some(d),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Move queued frames across, dropping the I frame numbered `lose`.
    ///
    /// One frame lost out of the middle of a full window, which is the case
    /// the two procedures answer differently.
    fn deliver_losing(from: &mut Lapm, to: &mut Lapm, lose: Option<u8>) -> Vec<u8> {
        let mut sent = Vec::new();
        while let Some((frame, kind)) = from.poll_transmit() {
            if let Frame::I { ns, .. } = &frame {
                sent.push(*ns);
                if Some(*ns) == lose {
                    continue;
                }
            }
            to.receive(frame, kind);
        }
        sent
    }

    /// Establish a connection, optionally with selective reject agreed.
    fn connected(srej: bool) -> (Lapm, Lapm) {
        let (mut a, mut b) = pair();
        a.set_selective_reject(srej);
        b.set_selective_reject(srej);
        a.connect();
        settle(&mut a, &mut b);
        assert!(a.is_connected() && b.is_connected());
        (a, b)
    }

    /// Send five frames with the third lost, and report what was sent again.
    fn recover(srej: bool) -> (Vec<u8>, Vec<u8>) {
        let (mut a, mut b) = connected(srej);
        for i in 0..5u8 {
            a.send_data(&[b'a' + i]);
        }
        // The third frame never arrives.
        deliver_losing(&mut a, &mut b, Some(2));
        // Whatever b makes of that goes back, and whatever a sends in reply is
        // the measurement: with go-back-N it is the lost frame and everything
        // after, with selective reject it is the lost frame.
        deliver(&mut b, &mut a);
        let again = deliver_losing(&mut a, &mut b, None);
        settle(&mut a, &mut b);
        (again, data_from(&mut b))
    }

    #[test]
    fn a_reject_asks_for_the_lost_frame_and_everything_after_it() {
        // V.42 8.4.4, which is the procedure without the optional one.
        let (again, data) = recover(false);
        assert_eq!(again, vec![2, 3, 4], "go-back-N resends from the gap");
        assert_eq!(data, b"abcde", "and everything still arrives in order");
    }

    #[test]
    fn a_selective_reject_asks_for_the_lost_frame_and_no_others() {
        // V.42 8.2.4.8.1: "I frames that may have been transmitted following
        // the I frame indicated by the SREJ frame shall not be retransmitted
        // as the result of receiving an SREJ frame." Which is only possible
        // because the receiver kept them, and delivered them in order once the
        // gap closed.
        let (again, data) = recover(true);
        assert_eq!(again, vec![2], "only the frame that was lost");
        assert_eq!(data, b"abcde", "and everything still arrives in order");
    }

    #[test]
    fn a_selective_reject_does_not_acknowledge_anything() {
        // 8.2.4.8.1: "the N(R) of the SREJ frame does not indicate
        // acknowledgement of any I frames". An end that treated it as one
        // would drop the frames before the gap from its retransmission buffer
        // and have nothing to send if they were asked for again.
        let (mut a, mut b) = connected(true);
        for i in 0..4u8 {
            a.send_data(&[b'a' + i]);
        }
        deliver_losing(&mut a, &mut b, Some(1));
        deliver(&mut b, &mut a);
        // Frames 1, 2 and 3 are still outstanding: 1 because it was lost, and
        // 2 and 3 because an SREJ acknowledges nothing.
        assert_eq!(a.va, 1, "V(A) moved on a frame that was never acknowledged");
    }

    #[test]
    fn frames_held_behind_a_gap_are_not_delivered_early() {
        // Out of order on the line is in order at the DTE, or the whole thing
        // is pointless.
        let (mut a, mut b) = connected(true);
        for i in 0..4u8 {
            a.send_data(&[b'a' + i]);
        }
        deliver_losing(&mut a, &mut b, Some(0));
        assert_eq!(data_from(&mut b), b"", "nothing can be delivered yet");
        deliver(&mut b, &mut a);
        deliver_losing(&mut a, &mut b, None);
        assert_eq!(data_from(&mut b), b"abcd", "and then all of it, in order");
    }

    #[test]
    fn window_arithmetic_wraps_at_the_modulus() {
        assert!(in_window(0, 0, 0));
        assert!(in_window(0, 3, 5));
        assert!(!in_window(0, 6, 5));
        // Wrapping across 127 to 0.
        assert!(in_window(126, 1, 3));
        assert!(!in_window(126, 4, 3));
        assert_eq!(outstanding(126, 2), 4);
    }

    #[test]
    fn sabme_and_ua_establish_a_connection() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);
        assert!(a.is_connected());
        assert!(b.is_connected());
        assert!(events(&mut a).contains(&Event::Connected));
        assert!(events(&mut b).contains(&Event::Connected));
    }

    #[test]
    fn data_crosses_in_order() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);
        events(&mut b);

        a.send_data(b"the quick brown fox");
        settle(&mut a, &mut b);
        assert_eq!(data_from(&mut b), b"the quick brown fox");
    }

    #[test]
    fn data_flows_in_both_directions() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);
        events(&mut a);
        events(&mut b);

        a.send_data(b"from a");
        b.send_data(b"from b");
        settle(&mut a, &mut b);
        assert_eq!(data_from(&mut b), b"from a");
        assert_eq!(data_from(&mut a), b"from b");
    }

    #[test]
    fn long_data_is_split_to_n401_and_reassembles() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);
        events(&mut b);

        let payload: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        a.send_data(&payload);
        settle(&mut a, &mut b);
        assert_eq!(data_from(&mut b), payload);
    }

    #[test]
    fn the_window_limits_frames_in_flight() {
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, Params { k: 3, n401: 1, ..Default::default() });
        let mut b = Lapm::new(Role::Answerer, DLCI_DATA, Params { k: 3, n401: 1, ..Default::default() });
        a.connect();
        settle(&mut a, &mut b);

        a.send_data(b"abcdefgh");
        // Collect without delivering, so nothing is acknowledged.
        let mut sent = 0;
        while let Some((frame, _)) = a.poll_transmit() {
            if matches!(frame, Frame::I { .. }) {
                sent += 1;
            }
        }
        assert_eq!(sent, 3, "k is 3, so only three frames may be outstanding");
    }

    #[test]
    fn a_lost_frame_is_recovered_by_reject() {
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, Params { n401: 1, ..Default::default() });
        let mut b = Lapm::new(Role::Answerer, DLCI_DATA, Params { n401: 1, ..Default::default() });
        a.connect();
        settle(&mut a, &mut b);
        events(&mut b);

        a.send_data(b"abcde");
        // Drop the second I frame on the way across.
        let mut index = 0;
        while let Some((frame, kind)) = a.poll_transmit() {
            let is_i = matches!(frame, Frame::I { .. });
            if is_i {
                index += 1;
                if index == 2 {
                    continue; // lost in transit
                }
            }
            b.receive(frame, kind);
        }
        settle(&mut a, &mut b);
        assert_eq!(
            data_from(&mut b),
            b"abcde",
            "go-back-N should have replaced the lost frame"
        );
    }

    #[test]
    fn receive_not_ready_stops_the_sender() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);

        a.receive(Frame::Rnr { nr: 0, pf: false }, Kind::Command);
        a.send_data(b"blocked");
        let mut sent_i = 0;
        while let Some((frame, _)) = a.poll_transmit() {
            if matches!(frame, Frame::I { .. }) {
                sent_i += 1;
            }
        }
        assert_eq!(sent_i, 0, "RNR should hold everything back");

        // RR releases it again.
        a.receive(Frame::Rr { nr: 0, pf: false }, Kind::Command);
        settle(&mut a, &mut b);
        assert!(!data_from(&mut b).is_empty());
    }

    #[test]
    fn disconnect_is_confirmed_by_the_peer() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);
        events(&mut a);
        events(&mut b);

        a.disconnect();
        settle(&mut a, &mut b);
        assert_eq!(a.state(), State::Disconnected);
        assert_eq!(b.state(), State::Disconnected);
        assert!(events(&mut a).contains(&Event::Released(Cause::Local)));
        assert!(events(&mut b).contains(&Event::Released(Cause::Peer)));
    }

    #[test]
    fn disc_while_disconnected_is_answered_with_dm() {
        // V.42 8.8.
        let mut a = Lapm::new(Role::Answerer, DLCI_DATA, Params::default());
        a.receive(Frame::Disc { poll: true }, Kind::Command);
        let (frame, kind) = a.poll_transmit().expect("should have answered");
        assert_eq!(frame, Frame::Dm { final_bit: true });
        assert_eq!(kind, Kind::Response);
    }

    #[test]
    fn establishment_retries_then_gives_up_after_n400() {
        let params = Params { n400: 3, t401_ms: 1000, ..Default::default() };
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, params);
        a.connect();
        // The first SABME plus two retries, then failure on the third expiry.
        let mut sabmes = 0;
        while let Some((frame, _)) = a.poll_transmit() {
            if matches!(frame, Frame::Sabme { .. }) {
                sabmes += 1;
            }
        }
        for _ in 0..3 {
            a.tick(1000);
            while let Some((frame, _)) = a.poll_transmit() {
                if matches!(frame, Frame::Sabme { .. }) {
                    sabmes += 1;
                }
            }
        }
        assert_eq!(sabmes, 3, "one initial SABME and N400-1 retries");
        assert_eq!(a.state(), State::Disconnected);
        assert!(events(&mut a).contains(&Event::Released(Cause::NoResponse)));
    }

    #[test]
    fn the_timer_does_not_fire_early() {
        let params = Params { t401_ms: 1000, ..Default::default() };
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, params);
        a.connect();
        let _ = a.poll_transmit();
        a.tick(400);
        a.tick(400);
        assert!(a.poll_transmit().is_none(), "fired before T401 elapsed");
        a.tick(400);
        assert!(a.poll_transmit().is_some(), "should have retried by now");
    }

    #[test]
    fn a_refused_connection_is_reported() {
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, Params::default());
        a.connect();
        a.receive(Frame::Dm { final_bit: true }, Kind::Response);
        assert_eq!(a.state(), State::Disconnected);
        assert!(events(&mut a).contains(&Event::Released(Cause::Refused)));
    }

    #[test]
    fn an_unsolicited_dm_during_establishment_is_ignored() {
        // V.42 8.9.3: a DM with F clear colliding with SABME is ignored.
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, Params::default());
        a.connect();
        a.receive(Frame::Dm { final_bit: false }, Kind::Response);
        assert_eq!(a.state(), State::AwaitingEstablishment);
    }

    #[test]
    fn re_establishment_resets_the_sequence_numbers() {
        // V.42 8.2.4.3: accepting SABME sets V(S), V(A) and V(R) to zero and
        // discards unacknowledged frames.
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);
        events(&mut a);
        events(&mut b);

        a.send_data(b"some data first");
        settle(&mut a, &mut b);
        events(&mut b);

        b.connect(); // peer re-establishes mid-connection
        settle(&mut a, &mut b);
        assert!(events(&mut a).contains(&Event::Reset));

        // Numbering starts again, and traffic still works.
        a.send_data(b"after reset");
        settle(&mut a, &mut b);
        assert_eq!(data_from(&mut b), b"after reset");
    }

    #[test]
    fn a_poll_is_answered_with_a_final() {
        // V.42 8.4.2.1.
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b);

        b.receive(Frame::Rr { nr: 0, pf: true }, Kind::Command);
        let replies: Vec<_> = std::iter::from_fn(|| b.poll_transmit()).collect();
        assert!(
            replies
                .iter()
                .any(|(f, k)| matches!(f, Frame::Rr { pf: true, .. }) && *k == Kind::Response),
            "expected an RR response with F set, got {replies:?}"
        );
    }

    #[test]
    fn a_lost_final_frame_is_recovered_by_the_timer() {
        // Nothing follows the last frame, so the peer never sees anything out
        // of sequence and never sends a reject. Only T401 recovery gets it back
        // (V.42 8.5.3). This is the case a reject-only implementation loses.
        let params = Params { t401_ms: 1000, ..Default::default() };
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, params);
        let mut b = Lapm::new(Role::Answerer, DLCI_DATA, params);
        a.connect();
        settle(&mut a, &mut b);
        events(&mut b);

        a.send_data(b"the last frame goes missing");
        // Drop everything a sends: the single I frame never arrives.
        while a.poll_transmit().is_some() {}
        assert!(data_from(&mut b).is_empty());

        // T401 expires, a polls, b answers with what it actually has.
        a.tick(1000);
        settle(&mut a, &mut b);
        assert_eq!(
            data_from(&mut b),
            b"the last frame goes missing",
            "timer recovery should have replaced the lost frame"
        );
    }

    #[test]
    fn timer_recovery_does_not_resend_what_was_already_acknowledged() {
        let params = Params { t401_ms: 1000, n401: 4, ..Default::default() };
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, params);
        let mut b = Lapm::new(Role::Answerer, DLCI_DATA, params);
        a.connect();
        settle(&mut a, &mut b);
        events(&mut b);

        a.send_data(b"abcdefghijkl");
        settle(&mut a, &mut b);
        let first = data_from(&mut b);
        assert_eq!(first, b"abcdefghijkl");

        // A spurious expiry with nothing outstanding must not duplicate data.
        a.tick(1000);
        settle(&mut a, &mut b);
        assert!(
            data_from(&mut b).is_empty(),
            "acknowledged data was sent a second time"
        );
    }

    #[test]
    fn sequence_numbers_wrap_past_the_modulus() {
        let params = Params { n401: 1, ..Default::default() };
        let mut a = Lapm::new(Role::Originator, DLCI_DATA, params);
        let mut b = Lapm::new(Role::Answerer, DLCI_DATA, params);
        a.connect();
        settle(&mut a, &mut b);
        events(&mut b);

        // Well past 128 single-octet frames, so N(S) wraps more than once.
        let payload: Vec<u8> = (0..300).map(|i| (i % 251) as u8).collect();
        a.send_data(&payload);
        settle(&mut a, &mut b);
        assert_eq!(data_from(&mut b), payload, "wrapping lost or reordered data");
    }
}
