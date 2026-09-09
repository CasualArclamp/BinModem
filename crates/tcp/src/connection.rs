//! One connection: the TCB of RFC 9293 3.3.1 and the event processing of 3.10.
//!
//! The variables keep the document's own names, lower-cased -- `snd_una` for
//! SND.UNA and so on -- because every rule about them is written in those
//! terms and a reader with the document open should not have to translate.
//!
//! What is here and what is not:
//!
//! * The eleven states and 3.10.7's processing of an arriving segment, in the
//!   order the document does it: sequence, RST, SYN, ACK, text, FIN.
//! * Retransmission, with RFC 6298's estimator and Karn's rule about which
//!   samples may be used.
//! * The Nagle algorithm (3.7.4), delayed acknowledgements (3.8.6.3), zero
//!   window probing (3.8.6.1) and the receiver's half of silly window
//!   avoidance (3.8.6.2.2) -- all four of which matter far more on a modem
//!   than on anything this was last written for. An acknowledgement is forty
//!   octets, which at 2400 bit/s is a sixth of a second of line.
//! * Segments that arrive out of order are held rather than dropped
//!   (SHLD-31), which turns one lost segment into one retransmission instead
//!   of a stall.
//!
//! * RFC 5681: slow start, congestion avoidance, fast retransmit and fast
//!   recovery. There is one hop under this and no congestion to speak of, so
//!   the argument for leaving it out looked good until the tests were written.
//!   Fast retransmit is not really congestion control at all -- it is the
//!   difference between repairing a lost segment on the third duplicate
//!   acknowledgement and repairing it a minute later when a timer that has
//!   doubled six times finally goes off. Without it, one loss in seven turned
//!   a page into a stall.
//!
//! Not here: window scaling and timestamps (RFC 7323), selective
//! acknowledgement, and urgent data -- none of which is any use at these
//! speeds. Urgent data is read on arrival (RCV.UP is kept) and never
//! generated: RFC 6093 found the mechanism too inconsistently implemented to
//! rely on.

use std::collections::VecDeque;

use crate::segment::{self, DEFAULT_MSS, Segment, flag};
use crate::seq::Seq;

/// 3.3.2's states. CLOSED is "fictional... it represents the state when there
/// is no TCB", which here is a connection that may still be read for what
/// became of it and then dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

impl State {
    /// Whether both ends have agreed sequence numbers, which is what makes a
    /// RST or a SYN mean something different (3.10.7.4).
    pub fn synchronized(self) -> bool {
        !matches!(self, State::Closed | State::Listen | State::SynSent)
    }

    /// Whether data may still be sent from here.
    pub fn can_send(self) -> bool {
        matches!(self, State::Established | State::CloseWait)
    }

    /// The name the document uses, for a log.
    pub fn name(self) -> &'static str {
        match self {
            State::Closed => "CLOSED",
            State::Listen => "LISTEN",
            State::SynSent => "SYN-SENT",
            State::SynReceived => "SYN-RECEIVED",
            State::Established => "ESTABLISHED",
            State::FinWait1 => "FIN-WAIT-1",
            State::FinWait2 => "FIN-WAIT-2",
            State::CloseWait => "CLOSE-WAIT",
            State::Closing => "CLOSING",
            State::LastAck => "LAST-ACK",
            State::TimeWait => "TIME-WAIT",
        }
    }
}

/// What the connection has to tell the program above it: 3.9.1.8's
/// asynchronous reports, and the two ordinary ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// The three-way handshake finished. Data may be sent.
    Established,
    /// Something arrived to be read.
    Data,
    /// A FIN came in: "connection closing". Nothing more will arrive, but
    /// this end may still send.
    Closing,
    /// A RST came in, or one was warranted: "connection reset".
    Reset,
    /// A SYN was answered with a RST: "connection refused".
    Refused,
    /// It is over and the block may go.
    Closed,
}

/// One end of a connection, named the way a socket is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

impl Endpoint {
    pub fn new(address: [u8; 4], port: u16) -> Self {
        Self { address, port }
    }
}

/// How much this end will hold for a reader that has not read yet.
///
/// Eight kilobytes is about three seconds of a 14 400 bit/s line, which is as
/// far ahead as there is any point running when the thing underneath is a
/// telephone call.
pub const RECEIVE_BUFFER: usize = 8192;

/// RFC 6298 (2.1): "the sender SHOULD set RTO <- 1 second" until there is a
/// measurement, and (2.4) nothing computed later may be rounded below it.
const INITIAL_RTO_MS: u32 = 1_000;
const MIN_RTO_MS: u32 = 1_000;
/// (2.5): "A maximum value MAY be placed on RTO provided it is at least 60
/// seconds."
const MAX_RTO_MS: u32 = 60_000;
/// (2.2): "RTO <- SRTT + max (G, K*RTTVAR) where K = 4."
const K: u32 = 4;

/// 3.8.6.3: "the delay MUST be less than 0.5 seconds" (MUST-40). Well under
/// it, because a modem's round trip is long enough already.
const DELAYED_ACK_MS: u32 = 200;

/// 3.10.8's TIME-WAIT timeout is twice the maximum segment lifetime. The
/// document takes MSL to be two minutes; on a link with one hop and no
/// network to wander in, a segment that is thirty seconds late is lost.
const TIME_WAIT_MS: u32 = 30_000;

/// How many times to resend before giving up: 3.8.3's R2, which "MUST be
/// greater than 100 seconds" of trying (MUST-20). Twelve doublings from a
/// second, capped at a minute, is far past that.
const MAX_RETRANSMITS: u32 = 12;

/// After this many backoffs the estimator is thrown away and started again.
///
/// RFC 6298 5: "a TCP implementation MAY clear SRTT and RTTVAR after backing
/// off the timer multiple times as it is likely that the current SRTT and
/// RTTVAR are bogus in this situation."
const BACKOFFS_BEFORE_FORGETTING: u32 = 3;

/// RFC 5681 3.2: "the arrival of 3 duplicate ACKs... as an indication that a
/// segment has been lost".
const DUPLICATES_BEFORE_RESENDING: u32 = 3;

/// The initial slow start threshold. RFC 5681 3.1: "SHOULD be set arbitrarily
/// high (e.g., to the size of the largest possible advertised window)", which
/// without window scaling is what sixteen bits will hold.
const INITIAL_SSTHRESH: u32 = u16::MAX as u32;

/// What 3.10.6's STATUS call gives back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub snd_una: u32,
    pub snd_nxt: u32,
    pub snd_wnd: u32,
    pub rcv_nxt: u32,
    pub rcv_wnd: u32,
    pub rto_ms: u32,
    pub retransmits: u32,
    pub cwnd: u32,
    pub ssthresh: u32,
    /// Octets given to this end and not yet acknowledged by the other.
    pub unacknowledged: usize,
    /// Octets that have arrived in order and not been read.
    pub unread: usize,
    /// Segments that arrived early and are waiting for the gap before them.
    pub held: usize,
}

/// One connection.
#[derive(Debug)]
pub struct Connection {
    pub state: State,
    pub local: Endpoint,
    pub remote: Endpoint,

    // Table 2: send sequence variables.
    snd_una: Seq,
    snd_nxt: Seq,
    /// SND.WND. Held in thirty-two bits, as REC-1 recommends, although only
    /// sixteen are ever on the wire without window scaling.
    snd_wnd: u32,
    snd_wl1: Seq,
    snd_wl2: Seq,
    iss: Seq,

    // Table 3: receive sequence variables.
    rcv_nxt: Seq,
    rcv_wnd: u32,
    rcv_up: Seq,
    irs: Seq,

    /// 3.7.1: what the far end said it can receive, or the default if it said
    /// nothing (MUST-15).
    send_mss: u16,
    /// And what this end asks for.
    recv_mss: u16,

    /// The retransmission queue: everything from SND.UNA that has been given
    /// to this connection and not yet acknowledged. `snd_nxt - snd_una` octets
    /// of it are in flight and the rest has not gone out.
    outgoing: VecDeque<u8>,
    /// Data in order, waiting for a reader.
    incoming: VecDeque<u8>,
    /// And data that arrived early. SHLD-31: "Segments with higher beginning
    /// sequence numbers SHOULD be held for later processing."
    held: Vec<(Seq, Vec<u8>)>,
    /// The user has closed; a FIN goes out once everything before it has.
    closing: bool,
    /// Whether the FIN has been given a place in the sequence space yet.
    fin_sent: bool,
    /// Whether this came from a passive open, which decides what a RST in
    /// SYN-RECEIVED means (3.10.7.4).
    passive: bool,

    /// RFC 5681 3.1's congestion window: "a sender-side limit on the amount
    /// of data the sender can transmit into the network before receiving an
    /// acknowledgment", where the advertised window is the receiver's limit
    /// and "the minimum of cwnd and rwnd governs data transmission".
    cwnd: u32,
    ssthresh: u32,
    /// Acknowledgements in a row that moved nothing, which is how a loss
    /// announces itself.
    duplicates: u32,
    /// Whether 3.2's fast recovery is running, during which cwnd is inflated
    /// by what has left the network and is deflated again at the end.
    recovering: bool,

    // RFC 6298's estimator.
    rto_ms: u32,
    srtt_ms: u32,
    rttvar_ms: u32,
    /// The sequence number and departure time of the segment being timed, if
    /// one is. Karn's algorithm (RFC 6298 3): a sample from a retransmitted
    /// segment is ambiguous, so the timing is abandoned when one is resent.
    timing: Option<(Seq, u64)>,
    retransmits: u32,

    /// Milliseconds since the connection began. Every deadline below is a
    /// point on this.
    clock: u64,
    retransmit_at: Option<u64>,
    time_wait_at: Option<u64>,
    ack_at: Option<u64>,
    /// Full-sized segments taken in since the last acknowledgement, for
    /// SHLD-19's "at least every second full-sized segment".
    since_ack: u32,

    out: Vec<Segment>,
    reports: Vec<Report>,
}

impl Connection {
    /// 3.10.1's passive OPEN: wait for somebody to call.
    ///
    /// The far end is unknown until one does; 3.10.7.2 fills it in then --
    /// "If the listen was not fully specified (i.e., the remote socket was not
    /// fully specified), then the unspecified fields should be filled in now."
    pub fn listen(local: Endpoint) -> Self {
        Self::new(local, Endpoint::default(), Seq(0), State::Listen, true)
    }

    /// And the active one. `isn` is the initial send sequence number, which
    /// 3.4.1 wants unpredictable and which the caller is better placed to
    /// choose than this is.
    pub fn connect(local: Endpoint, remote: Endpoint, isn: u32) -> Self {
        let mut c = Self::new(local, remote, Seq(isn), State::SynSent, false);
        // <SEQ=ISS><CTL=SYN>, and 3.7.1 SHLD-5 puts the size on it.
        c.snd_nxt = c.iss + 1;
        let mss = c.recv_mss;
        c.transmit(Segment {
            sequence: c.iss.0,
            flags: flag::SYN,
            mss: Some(mss),
            ..c.blank()
        });
        c
    }

    fn new(local: Endpoint, remote: Endpoint, iss: Seq, state: State, passive: bool) -> Self {
        Self {
            state,
            local,
            remote,
            snd_una: iss,
            snd_nxt: iss,
            snd_wnd: 0,
            snd_wl1: Seq(0),
            snd_wl2: Seq(0),
            iss,
            rcv_nxt: Seq(0),
            rcv_wnd: RECEIVE_BUFFER as u32,
            rcv_up: Seq(0),
            irs: Seq(0),
            send_mss: DEFAULT_MSS,
            cwnd: u32::from(DEFAULT_MSS),
            ssthresh: INITIAL_SSTHRESH,
            duplicates: 0,
            recovering: false,
            // What this end can take in one piece. A PPP link with a 1500
            // octet MRU leaves this after the two headers, and 3.7.1 has the
            // option carry "the effective MTU minus the fixed IP and TCP
            // headers".
            recv_mss: 1460,
            outgoing: VecDeque::new(),
            incoming: VecDeque::new(),
            held: Vec::new(),
            closing: false,
            fin_sent: false,
            passive,
            rto_ms: INITIAL_RTO_MS,
            srtt_ms: 0,
            rttvar_ms: 0,
            timing: None,
            retransmits: 0,
            clock: 0,
            retransmit_at: None,
            time_wait_at: None,
            ack_at: None,
            since_ack: 0,
            out: Vec::new(),
            reports: Vec::new(),
        }
    }

    /// Tell this end what the far end can receive in one segment, and what
    /// this end will ask for. Both are bounded by what the link below will
    /// carry in one datagram.
    pub fn set_receive_mss(&mut self, mss: u16) {
        self.recv_mss = mss.max(88);
    }

    /// 3.10.6's STATUS call: "state, active/passive, ... send window, receive
    /// window", and what this end is waiting on.
    ///
    /// Everything in it is otherwise private, and it exists because a stalled
    /// connection is a set of numbers that do not agree with each other and no
    /// way to see them is no way to find out which.
    pub fn status(&self) -> Status {
        Status {
            state: self.state,
            snd_una: self.snd_una.0,
            snd_nxt: self.snd_nxt.0,
            snd_wnd: self.snd_wnd,
            rcv_nxt: self.rcv_nxt.0,
            rcv_wnd: self.rcv_wnd,
            rto_ms: self.rto_ms,
            retransmits: self.retransmits,
            cwnd: self.cwnd,
            ssthresh: self.ssthresh,
            unacknowledged: self.outgoing.len(),
            unread: self.incoming.len(),
            held: self.held.len(),
        }
    }

    /// Segments to put on the network.
    pub fn take_segments(&mut self) -> Vec<Segment> {
        std::mem::take(&mut self.out)
    }

    pub fn take_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }

    /// Everything that has arrived in order and not been read.
    pub fn take_received(&mut self) -> Vec<u8> {
        let out: Vec<u8> = self.incoming.drain(..).collect();
        if !out.is_empty() {
            // 3.10.7.4: "adjusts RCV.WND as appropriate to the current buffer
            // availability", which has just gone up by everything read.
            self.advertise();
        }
        out
    }

    /// How much has arrived and not been read.
    pub fn available(&self) -> usize {
        self.incoming.len()
    }

    /// How much this end is still waiting to have acknowledged.
    pub fn unsent(&self) -> usize {
        self.outgoing.len()
    }

    /// Whether the far end has said it will send no more.
    pub fn finished(&self) -> bool {
        matches!(
            self.state,
            State::CloseWait | State::Closing | State::LastAck | State::TimeWait | State::Closed
        )
    }

    /// 3.10.2's SEND. Takes what it has room for and says how much that was.
    pub fn send(&mut self, data: &[u8]) -> usize {
        if !self.state.can_send() {
            return 0;
        }
        // Twice the receive buffer, which is enough to keep a modem busy for
        // several seconds and small enough that a caller which ignores the
        // return value cannot fill memory with it.
        let room = (2 * RECEIVE_BUFFER).saturating_sub(self.outgoing.len());
        let take = data.len().min(room);
        self.outgoing.extend(&data[..take]);
        self.emit();
        take
    }

    /// 3.10.4's CLOSE: no more will be sent, but what has been sent is still
    /// seen through and what arrives is still taken (3.6.1's half-close).
    pub fn close(&mut self) {
        match self.state {
            State::Listen | State::SynSent => {
                self.state = State::Closed;
                self.reports.push(Report::Closed);
            }
            State::SynReceived | State::Established | State::CloseWait => {
                self.closing = true;
                self.emit();
            }
            // Already closing, or closed.
            _ => {}
        }
    }

    /// 3.10.5's ABORT: a RST, and nothing more.
    pub fn abort(&mut self) {
        if self.state.synchronized() {
            let snd_nxt = self.snd_nxt;
            self.transmit(Segment {
                sequence: snd_nxt.0,
                flags: flag::RST,
                ..self.blank()
            });
        }
        self.give_up(Report::Closed);
    }

    /// 3.10.8's timeouts, in milliseconds of whatever clock the caller keeps.
    pub fn tick(&mut self, ms: u32) {
        self.clock += u64::from(ms);

        if self.due(self.time_wait_at) {
            // "If the time-wait timeout expires on a connection, delete the
            // TCB, enter the CLOSED state, and return."
            self.time_wait_at = None;
            self.give_up(Report::Closed);
            return;
        }
        if self.due(self.ack_at) {
            self.ack_at = None;
            self.acknowledge();
        }
        if self.due(self.retransmit_at) {
            self.retransmit();
        }
    }

    fn due(&self, at: Option<u64>) -> bool {
        at.is_some_and(|at| self.clock >= at)
    }

    /// 3.10.7: a segment has arrived for this connection.
    ///
    /// `from` is the source address out of the datagram that carried it. A
    /// segment does not hold one -- the addresses live in the layer below and
    /// reach TCP only through the pseudo-header -- so it is passed in, and a
    /// connection that was listening learns from it who is calling.
    pub fn receive(&mut self, from: [u8; 4], segment: &Segment) {
        if self.state == State::Listen {
            self.remote.address = from;
        }
        match self.state {
            State::Closed => self.arrived_closed(segment),
            State::Listen => self.arrived_listening(segment),
            State::SynSent => self.arrived_syn_sent(segment),
            _ => self.arrived_synchronized(segment),
        }
    }

    // ---------------------------------------------------------------- 3.10.7.1

    /// "an incoming segment not containing a RST causes a RST to be sent in
    /// response. The acknowledgment and sequence field values are selected to
    /// make the reset sequence acceptable to the TCP endpoint that sent the
    /// offending segment."
    fn arrived_closed(&mut self, segment: &Segment) {
        if segment.rst() {
            return;
        }
        self.out.push(self.reset_for(segment));
    }

    fn reset_for(&self, segment: &Segment) -> Segment {
        let mut out = Segment {
            source_port: self.local.port,
            destination_port: self.remote.port,
            window: 0,
            ..Segment::default()
        };
        if segment.ack() {
            // <SEQ=SEG.ACK><CTL=RST>
            out.sequence = segment.acknowledgment;
            out.flags = flag::RST;
        } else {
            // <SEQ=0><ACK=SEG.SEQ+SEG.LEN><CTL=RST,ACK>
            out.sequence = 0;
            out.acknowledgment = (Seq(segment.sequence) + segment.length()).0;
            out.flags = flag::RST | flag::ACK;
        }
        out
    }

    // ---------------------------------------------------------------- 3.10.7.2

    fn arrived_listening(&mut self, segment: &Segment) {
        // First: "An incoming RST should be ignored."
        if segment.rst() {
            return;
        }
        // Second: "Any acknowledgment is bad if it arrives on a connection
        // still in the LISTEN state."
        if segment.ack() {
            self.out.push(Segment {
                source_port: self.local.port,
                destination_port: segment.source_port,
                sequence: segment.acknowledgment,
                flags: flag::RST,
                ..Segment::default()
            });
            return;
        }
        // Third: the SYN. "If the listen was not fully specified... then the
        // unspecified fields should be filled in now."
        if !segment.syn() {
            // Fourth: "This should not be reached."
            return;
        }
        self.remote.port = segment.source_port;
        self.irs = Seq(segment.sequence);
        self.rcv_nxt = Seq(segment.sequence) + 1;
        self.snd_una = self.iss;
        self.snd_nxt = self.iss + 1;
        self.snd_wnd = u32::from(segment.window);
        self.snd_wl1 = Seq(segment.sequence);
        self.send_mss = segment.mss.unwrap_or(DEFAULT_MSS);
        self.state = State::SynReceived;
        // <SEQ=ISS><ACK=RCV.NXT><CTL=SYN,ACK>
        let (iss, rcv_nxt, mss) = (self.iss, self.rcv_nxt, self.recv_mss);
        self.transmit(Segment {
            sequence: iss.0,
            acknowledgment: rcv_nxt.0,
            flags: flag::SYN | flag::ACK,
            mss: Some(mss),
            ..self.blank()
        });
    }

    /// Give a listening connection the sequence number it will open with.
    ///
    /// Separate from [`Connection::listen`] because a listener is made once
    /// and may accept many connections, and 3.4.1 wants each to start
    /// somewhere the last one does not predict.
    pub fn set_initial_sequence(&mut self, isn: u32) {
        if self.state == State::Listen {
            self.iss = Seq(isn);
            self.snd_una = self.iss;
            self.snd_nxt = self.iss;
        }
    }

    // ---------------------------------------------------------------- 3.10.7.3

    fn arrived_syn_sent(&mut self, segment: &Segment) {
        // First, the ACK bit.
        let mut acceptable_ack = false;
        if segment.ack() {
            let ack = Seq(segment.acknowledgment);
            // "If SEG.ACK =< ISS or SEG.ACK > SND.NXT, send a reset (unless
            // the RST bit is set, if so drop the segment and return)."
            if ack.before_or_at(self.iss) || ack.after(self.snd_nxt) {
                if !segment.rst() {
                    self.out.push(Segment {
                        sequence: segment.acknowledgment,
                        flags: flag::RST,
                        ..self.blank()
                    });
                }
                return;
            }
            acceptable_ack = true;
        }

        // Second, the RST bit. "If the ACK was acceptable, then signal to the
        // user 'error: connection reset'... Otherwise (no ACK), drop the
        // segment and return."
        if segment.rst() {
            if acceptable_ack {
                // A reset in answer to a SYN is what a closed port says, and
                // 3.9.1.1 calls it by its own name.
                self.give_up(Report::Refused);
            }
            return;
        }

        // Fourth, the SYN bit. (Third is security, which has no meaning here.)
        if !segment.syn() {
            // Fifth: "if neither of the SYN or RST bits is set, then drop the
            // segment and return."
            return;
        }
        self.irs = Seq(segment.sequence);
        self.rcv_nxt = Seq(segment.sequence) + 1;
        self.send_mss = segment.mss.unwrap_or(DEFAULT_MSS);
        if segment.ack() {
            // Only the SYN can have been acknowledged here, and a SYN takes a
            // place in the sequence space and no room in the queue.
            self.snd_una = Seq(segment.acknowledgment);
        }
        self.snd_wnd = u32::from(segment.window);
        self.snd_wl1 = Seq(segment.sequence);
        self.snd_wl2 = Seq(segment.acknowledgment);

        if self.snd_una.after(self.iss) {
            // "If SND.UNA > ISS (our SYN has been ACKed), change the
            // connection state to ESTABLISHED."
            self.state = State::Established;
            self.open_window();
            self.reports.push(Report::Established);
            self.acknowledge();
            // "If there are other controls or text in the segment, then
            // continue processing at the sixth step."
            if segment.length() > 1 || segment.fin() {
                self.take_text_and_fin(segment);
            }
            self.emit();
        } else {
            // A simultaneous open: both ends sent a SYN and neither has been
            // acknowledged yet (3.5's Figure 7).
            self.state = State::SynReceived;
            let (iss, rcv_nxt, mss) = (self.iss, self.rcv_nxt, self.recv_mss);
            self.transmit(Segment {
                sequence: iss.0,
                acknowledgment: rcv_nxt.0,
                flags: flag::SYN | flag::ACK,
                mss: Some(mss),
                ..self.blank()
            });
        }
    }

    // ---------------------------------------------------------------- 3.10.7.4

    fn arrived_synchronized(&mut self, segment: &Segment) {
        // First, the sequence number. Table 6's four cases.
        let acceptable = self.acceptable(segment);
        if !acceptable {
            // "If an incoming segment is not acceptable, an acknowledgment
            // should be sent in reply (unless the RST bit is set, if so drop
            // the segment and return)."
            if !segment.rst() {
                self.acknowledge();
            }
            return;
        }

        // Second, the RST bit.
        if segment.rst() {
            match self.state {
                State::SynReceived if self.passive => {
                    // "return this connection to LISTEN state... The user need
                    // not be informed."
                    self.relisten();
                }
                State::SynReceived => self.give_up(Report::Refused),
                State::Closing | State::LastAck | State::TimeWait => {
                    self.give_up(Report::Closed)
                }
                _ => self.give_up(Report::Reset),
            }
            return;
        }

        // Fourth, the SYN bit. (Third is security again.)
        if segment.syn() {
            if self.state == State::SynReceived && self.passive {
                self.relisten();
                return;
            }
            // RFC 5961's challenge: "if the SYN bit is set, irrespective of
            // the sequence number, TCP endpoints MUST send a 'challenge ACK'".
            // Which is what refusing to be reset by a stray SYN comes to.
            self.acknowledge();
            return;
        }

        // Fifth, the ACK field. "if the ACK bit is off, drop the segment and
        // return."
        if !segment.ack() {
            return;
        }
        if !self.take_acknowledgment(segment) {
            return;
        }

        // Sixth, the URG bit. Kept, never acted on: RFC 6093 found the
        // mechanism too inconsistently implemented to build anything over.
        if segment.has(flag::URG) {
            let up = Seq(segment.sequence) + u32::from(segment.urgent);
            if up.after(self.rcv_up) {
                self.rcv_up = up;
            }
        }

        // Seventh and eighth: the text and the FIN.
        self.take_text_and_fin(segment);
        self.emit();
    }

    /// Table 6's acceptability test, all four rows of it.
    fn acceptable(&self, segment: &Segment) -> bool {
        let seq = Seq(segment.sequence);
        let len = segment.length();
        match (len, self.rcv_wnd) {
            // "SEG.SEQ = RCV.NXT"
            (0, 0) => seq == self.rcv_nxt,
            // "RCV.NXT =< SEG.SEQ < RCV.NXT+RCV.WND"
            (0, window) => seq.within(self.rcv_nxt, window),
            // "not acceptable"
            (_, 0) => false,
            (_, window) => {
                seq.within(self.rcv_nxt, window)
                    || (seq + (len - 1)).within(self.rcv_nxt, window)
            }
        }
    }

    /// The fifth step, which is most of what a connection spends its life
    /// doing. Says whether processing should carry on.
    fn take_acknowledgment(&mut self, segment: &Segment) -> bool {
        let ack = Seq(segment.acknowledgment);

        if self.state == State::SynReceived {
            // "If SND.UNA < SEG.ACK =< SND.NXT, then enter ESTABLISHED state."
            if self.snd_una.before(ack) && ack.before_or_at(self.snd_nxt) {
                self.state = State::Established;
                self.open_window();
                self.reports.push(Report::Established);
            } else {
                // "If the segment acknowledgment is not acceptable, form a
                // reset segment <SEQ=SEG.ACK><CTL=RST> and send it."
                self.out.push(Segment {
                    sequence: segment.acknowledgment,
                    flags: flag::RST,
                    ..self.blank()
                });
                return false;
            }
        }

        // "If the ACK acks something not yet sent (SEG.ACK > SND.NXT), then
        // send an ACK, drop the segment, and return."
        if ack.after(self.snd_nxt) {
            self.acknowledge();
            return false;
        }
        if self.snd_una.before(ack) {
            self.measure(ack);
            let advanced = self.snd_una.distance_to(ack) as usize;
            self.snd_una = ack;
            self.drop_acknowledged(advanced);
            self.retransmits = 0;
            self.grow_window(advanced as u32);
            // RFC 6298 (5.3): "When an ACK is received that acknowledges new
            // data, restart the retransmission timer", and 3.10.8 has it
            // follow the front of the queue, so it stops when there is
            // nothing left in it.
            self.arm_retransmit();
        } else if ack == self.snd_una && segment.payload.is_empty() && !self.outgoing.is_empty()
        {
            self.duplicate_acknowledgment();
        }
        // "If SND.UNA =< SEG.ACK =< SND.NXT, the send window should be
        // updated... The check here prevents using old segments to update the
        // window."
        let seq = Seq(segment.sequence);
        if self.snd_wl1.before(seq) || (self.snd_wl1 == seq && self.snd_wl2.before_or_at(ack)) {
            self.snd_wnd = u32::from(segment.window);
            self.snd_wl1 = seq;
            self.snd_wl2 = ack;
        }

        // The per-state additions.
        let fin_acked = self.fin_sent && self.snd_una == self.snd_nxt;
        match self.state {
            State::FinWait1 if fin_acked => self.state = State::FinWait2,
            State::Closing if fin_acked => self.enter_time_wait(),
            State::LastAck if fin_acked => {
                // "If our FIN is now acknowledged, delete the TCB, enter the
                // CLOSED state, and return."
                self.give_up(Report::Closed);
                return false;
            }
            State::TimeWait => {
                // "The only thing that can arrive in this state is a
                // retransmission of the remote FIN. Acknowledge it, and
                // restart the 2 MSL timeout."
                self.acknowledge();
                self.enter_time_wait();
                return false;
            }
            _ => {}
        }
        true
    }

    /// The seventh and eighth steps: what the segment carried, and whether it
    /// was the last of it.
    fn take_text_and_fin(&mut self, segment: &Segment) {
        // "In the following it is assumed that the segment is the idealized
        // segment that begins at RCV.NXT and does not exceed the window. One
        // could tailor actual segments to fit this assumption by trimming off
        // any portions that lie outside the window."
        let seq = Seq(segment.sequence) + u32::from(segment.syn());
        let mut delivered = false;

        if !segment.payload.is_empty() && !self.finished() {
            if seq == self.rcv_nxt {
                delivered = self.deliver(&segment.payload);
            } else if seq.after(self.rcv_nxt) && seq.within(self.rcv_nxt, self.rcv_wnd) {
                // SHLD-31: hold it rather than throw it away. One lost segment
                // then costs one retransmission instead of everything after it.
                self.hold(seq, &segment.payload);
                // RFC 5681 4.2 asks for an immediate acknowledgement of a
                // segment above a gap, so the far end learns of the hole now.
                self.acknowledge();
                return;
            } else if seq.before(self.rcv_nxt) {
                // An overlap: only the part past RCV.NXT is new.
                let already = seq.distance_to(self.rcv_nxt) as usize;
                if already < segment.payload.len() {
                    delivered = self.deliver(&segment.payload[already..]);
                }
            }
        }

        // Eighth, the FIN. Only when it is the next thing in the sequence
        // space; one that arrived past a gap is not yet the end.
        let fin_at = seq + segment.payload.len() as u32;
        if segment.fin() && fin_at == self.rcv_nxt {
            self.rcv_nxt += 1;
            self.reports.push(Report::Closing);
            match self.state {
                State::SynReceived | State::Established => self.state = State::CloseWait,
                State::FinWait1 => {
                    // "If our FIN has been ACKed (perhaps in this segment),
                    // then enter TIME-WAIT... otherwise, enter the CLOSING
                    // state."
                    if self.fin_sent && self.snd_una == self.snd_nxt {
                        self.enter_time_wait();
                    } else {
                        self.state = State::Closing;
                    }
                }
                State::FinWait2 => self.enter_time_wait(),
                // "Remain in the ... state." TIME-WAIT restarts its timer.
                State::TimeWait => self.enter_time_wait(),
                _ => {}
            }
            // A FIN is acknowledged at once. There is nothing left to carry it.
            self.acknowledge();
            return;
        }

        if delivered {
            self.reports.push(Report::Data);
            // SHLD-19: "An ACK SHOULD be generated for at least every second
            // full-sized segment", and otherwise the delayed one covers it.
            self.since_ack += 1;
            if self.since_ack >= 2 {
                self.acknowledge();
            } else if self.ack_at.is_none() {
                self.ack_at = Some(self.clock + u64::from(DELAYED_ACK_MS));
            }
        }
    }

    /// Take data that begins exactly at RCV.NXT, and whatever was held behind
    /// it. Says whether anything went in.
    fn deliver(&mut self, data: &[u8]) -> bool {
        let room = RECEIVE_BUFFER.saturating_sub(self.incoming.len());
        let take = data.len().min(room);
        if take == 0 {
            return false;
        }
        self.incoming.extend(&data[..take]);
        self.rcv_nxt += take as u32;
        // Anything held that now begins where the stream does.
        while let Some(at) = self.held.iter().position(|(seq, bytes)| {
            seq.before_or_at(self.rcv_nxt) && (*seq + bytes.len() as u32).after(self.rcv_nxt)
        }) {
            let (seq, bytes) = self.held.remove(at);
            let already = seq.distance_to(self.rcv_nxt) as usize;
            let room = RECEIVE_BUFFER.saturating_sub(self.incoming.len());
            let take = (bytes.len() - already).min(room);
            if take == 0 {
                break;
            }
            self.incoming.extend(&bytes[already..already + take]);
            self.rcv_nxt += take as u32;
        }
        // Anything wholly behind the stream now is a duplicate.
        let rcv_nxt = self.rcv_nxt;
        self.held
            .retain(|(seq, bytes)| (*seq + bytes.len() as u32).after(rcv_nxt));
        self.advertise();
        true
    }

    /// Keep a segment that arrived early.
    fn hold(&mut self, seq: Seq, data: &[u8]) {
        // A handful is all a link with one hop under it can produce, and a
        // list that could grow without bound is a way to be run out of memory
        // by a far end that only ever sends the second segment.
        const MOST: usize = 16;
        if self.held.iter().any(|(at, _)| *at == seq) {
            return;
        }
        if self.held.len() >= MOST {
            return;
        }
        self.held.push((seq, data.to_vec()));
    }

    /// 3.8.6.2.2, the receiver's half of silly window avoidance: do not
    /// announce a window that is not worth filling, and never take back one
    /// already announced -- "The total of RCV.NXT and RCV.WND should not be
    /// reduced."
    fn advertise(&mut self) {
        let free = RECEIVE_BUFFER.saturating_sub(self.incoming.len()) as u32;
        let worth = u32::from(self.recv_mss).min(RECEIVE_BUFFER as u32 / 2);
        if free >= self.rcv_wnd || free >= worth {
            self.rcv_wnd = free;
        }
    }

    // ------------------------------------------------------------ transmission

    /// What to put on the line now: as much as the window, the size and Nagle
    /// between them allow.
    fn emit(&mut self) {
        if !self.state.synchronized() || self.state == State::TimeWait {
            return;
        }
        loop {
            // "SND.WND is an offset from SND.UNA", and RFC 5681 3.1 has
            // "the minimum of cwnd and rwnd" govern what may be sent.
            let in_flight = self.snd_una.distance_to(self.snd_nxt);
            let window = self.snd_wnd.min(self.cwnd).saturating_sub(in_flight);
            let waiting = self.outgoing.len() as u32 - in_flight.min(self.outgoing.len() as u32);
            let size = u32::from(self.send_mss).min(window).min(waiting);

            if size == 0 {
                break;
            }
            // 3.7.4: "If there is unacknowledged data... then the sending TCP
            // endpoint buffers all user data... until the outstanding data has
            // been acknowledged or until the TCP endpoint can send a full-sized
            // segment." The exception is a close: what is waiting is all there
            // will ever be, so there is nothing to coalesce it with.
            let full = size == u32::from(self.send_mss);
            let last = size == waiting && self.closing;
            if in_flight > 0 && !full && !last {
                break;
            }

            let from = in_flight as usize;
            let payload: Vec<u8> = self
                .outgoing
                .iter()
                .skip(from)
                .take(size as usize)
                .copied()
                .collect();
            let sequence = self.snd_nxt.0;
            self.snd_nxt += size;
            // 3.9.1.2's push: the far end is told to hand this up rather than
            // wait for more, which is what every last segment of a request is.
            let more_waiting = waiting > size;
            let mut flags = flag::ACK;
            if !more_waiting {
                flags |= flag::PSH;
            }
            let rcv_nxt = self.rcv_nxt;
            self.transmit(Segment {
                sequence,
                acknowledgment: rcv_nxt.0,
                flags,
                payload,
                ..self.blank()
            });
        }

        // 3.8.6.1: "the sending TCP peer must regularly retransmit to the
        // receiving TCP peer even when the window is zero" -- because what
        // reopens the window is an acknowledgement, and acknowledgements are
        // not retransmitted. Without this, one lost window update stops the
        // connection for ever. The probe itself is sent by the timer.
        if self.snd_wnd == 0
            && !self.outgoing.is_empty()
            && self.snd_una == self.snd_nxt
            && self.retransmit_at.is_none()
        {
            self.retransmit_at = Some(self.clock + u64::from(self.rto_ms));
        }

        // The FIN goes when everything before it has, and takes the last place
        // in the sequence space (3.4).
        let everything_sent = self.snd_una.distance_to(self.snd_nxt) as usize == self.outgoing.len();
        if self.closing && !self.fin_sent && everything_sent {
            self.fin_sent = true;
            let sequence = self.snd_nxt.0;
            self.snd_nxt += 1;
            let rcv_nxt = self.rcv_nxt;
            self.transmit(Segment {
                sequence,
                acknowledgment: rcv_nxt.0,
                flags: flag::FIN | flag::ACK,
                ..self.blank()
            });
            self.state = match self.state {
                State::CloseWait => State::LastAck,
                State::Established | State::SynReceived => State::FinWait1,
                other => other,
            };
        }
    }

    /// <SEQ=SND.NXT><ACK=RCV.NXT><CTL=ACK>, the segment that appears in nearly
    /// every step of 3.10.7.
    fn acknowledge(&mut self) {
        if !self.state.synchronized() {
            return;
        }
        self.ack_at = None;
        self.since_ack = 0;
        let (snd_nxt, rcv_nxt) = (self.snd_nxt, self.rcv_nxt);
        self.out.push(Segment {
            sequence: snd_nxt.0,
            acknowledgment: rcv_nxt.0,
            flags: flag::ACK,
            ..self.blank()
        });
    }

    /// Everything that goes out and occupies sequence space comes through
    /// here, so that the retransmission timer and the round-trip sample are
    /// never forgotten in one branch of 3.10.7 and remembered in another.
    fn transmit(&mut self, segment: Segment) {
        if segment.length() > 0 {
            self.arm_retransmit();
            // RFC 6298 3: one sample at a time, and only from a segment that
            // has not been sent before.
            if self.timing.is_none() {
                self.timing = Some((Seq(segment.sequence) + segment.length(), self.clock));
            }
            self.ack_at = None;
            self.since_ack = 0;
        }
        self.out.push(segment);
    }

    /// A blank segment addressed to the far end, with this end's window on it.
    fn blank(&self) -> Segment {
        Segment {
            source_port: self.local.port,
            destination_port: self.remote.port,
            window: self.rcv_wnd.min(u32::from(u16::MAX)) as u16,
            ..Segment::default()
        }
    }

    /// Drop what an acknowledgement covered.
    ///
    /// The queue begins at SND.UNA, so what SND.UNA passed is always the front
    /// of it -- but `advanced` is a distance in the sequence space and the
    /// queue holds only octets. A SYN and a FIN each take a place in that
    /// space and no room in the queue, so the clamp is not a safety net; it is
    /// where those two are accounted for.
    fn drop_acknowledged(&mut self, advanced: usize) {
        let acked = advanced.min(self.outgoing.len());
        self.outgoing.drain(..acked);
    }

    /// RFC 5681 3.1's initial window, set when the handshake finishes.
    ///
    /// "the SYN/ACK and the acknowledgment of the SYN/ACK MUST NOT increase
    /// the size of the congestion window. Further, if the SYN or SYN/ACK is
    /// lost, the initial window used by a sender after a correctly
    /// transmitted SYN MUST be one segment."
    fn open_window(&mut self) {
        let smss = u32::from(self.send_mss);
        self.cwnd = if self.retransmits > 0 {
            smss
        } else if self.send_mss > 2190 {
            2 * smss
        } else if self.send_mss > 1095 {
            3 * smss
        } else {
            4 * smss
        };
        // RFC 6298 (5.7): "If the timer expires awaiting the ACK of a SYN
        // segment and the TCP implementation is using an RTO less than 3
        // seconds, the RTO MUST be re-initialized to 3 seconds when data
        // transmission begins."
        if self.retransmits > 0 {
            self.rto_ms = self.rto_ms.max(3_000);
        }
        self.retransmits = 0;
    }

    /// An acknowledgement that moved SND.UNA: 3.1's slow start below the
    /// threshold and congestion avoidance above it.
    fn grow_window(&mut self, acked: u32) {
        self.duplicates = 0;
        if self.recovering {
            // 3.2 step 6: "When the next ACK arrives that acknowledges
            // previously unacknowledged data, a TCP MUST set cwnd to
            // ssthresh... This is termed 'deflating' the window."
            self.recovering = false;
            self.cwnd = self.ssthresh;
            return;
        }
        let smss = u32::from(self.send_mss);
        if self.cwnd < self.ssthresh {
            // "cwnd += min (N, SMSS)" (2), where N is what was acknowledged.
            self.cwnd = self.cwnd.saturating_add(acked.min(smss));
        } else {
            // "cwnd += SMSS*SMSS/cwnd" (3), once per acknowledgement.
            self.cwnd = self
                .cwnd
                .saturating_add((smss * smss / self.cwnd.max(1)).max(1));
        }
        self.cwnd = self.cwnd.min(INITIAL_SSTHRESH);
    }

    /// 3.2's fast retransmit and fast recovery.
    fn duplicate_acknowledgment(&mut self) {
        self.duplicates += 1;
        let smss = u32::from(self.send_mss);
        match self.duplicates.cmp(&DUPLICATES_BEFORE_RESENDING) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                // Steps 2 and 3: "ssthresh = max (FlightSize / 2, 2*SMSS)",
                // then "The lost segment starting at SND.UNA MUST be
                // retransmitted and cwnd set to ssthresh plus 3*SMSS. This
                // artificially 'inflates' the congestion window by the number
                // of segments (three) that have left the network and which the
                // receiver has buffered."
                let in_flight = self.snd_una.distance_to(self.snd_nxt);
                self.ssthresh = (in_flight / 2).max(2 * smss);
                self.recovering = true;
                self.resend_front();
                self.cwnd = self.ssthresh.saturating_add(3 * smss);
            }
            std::cmp::Ordering::Greater => {
                // Step 4: "For each additional duplicate ACK received (after
                // the third), cwnd MUST be incremented by SMSS."
                self.cwnd = self.cwnd.saturating_add(smss).min(INITIAL_SSTHRESH);
            }
        }
    }

    fn arm_retransmit(&mut self) {
        if self.snd_una == self.snd_nxt {
            // Nothing outstanding: 3.10.8 has the timer follow the queue.
            self.retransmit_at = None;
            return;
        }
        self.retransmit_at = Some(self.clock + u64::from(self.rto_ms));
    }

    /// RFC 6298 (2.2) and (2.3), with Karn's rule from section 3.
    fn measure(&mut self, ack: Seq) {
        let Some((at, sent)) = self.timing else { return };
        if ack.before(at) {
            return;
        }
        self.timing = None;
        // Karn's rule is already kept: a retransmission clears `timing`, so a
        // sample that survives to here is from a segment sent once. Refusing
        // it merely because something else was resent earlier is what keeps an
        // RTO that has doubled its way to a minute from ever collapsing again,
        // and RFC 6298 5 says the collapsing is the point -- "once a new RTT
        // measurement is obtained... the computations outlined in Section 2
        // are performed, including the computation of RTO, which may result in
        // 'collapsing' RTO back down after it has been subject to exponential
        // back off".
        let r = (self.clock - sent).min(u64::from(u32::MAX)) as u32;
        if self.srtt_ms == 0 {
            // "SRTT <- R, RTTVAR <- R/2"
            self.srtt_ms = r;
            self.rttvar_ms = r / 2;
        } else {
            // "RTTVAR <- (1 - beta) * RTTVAR + beta * |SRTT - R'|" then
            // "SRTT <- (1 - alpha) * SRTT + alpha * R'", in that order, with
            // beta = 1/4 and alpha = 1/8.
            let difference = self.srtt_ms.abs_diff(r);
            self.rttvar_ms = (3 * self.rttvar_ms + difference) / 4;
            self.srtt_ms = (7 * self.srtt_ms + r) / 8;
        }
        // "RTO <- SRTT + max (G, K*RTTVAR)", then (2.4)'s floor and (2.5)'s
        // ceiling. G, the clock granularity, is a millisecond here.
        self.rto_ms = self
            .srtt_ms
            .saturating_add((K * self.rttvar_ms).max(1))
            .clamp(MIN_RTO_MS, MAX_RTO_MS);
    }

    /// 3.10.8: "send the segment at the front of the retransmission queue
    /// again, reinitialize the retransmission timer".
    fn retransmit(&mut self) {
        self.retransmits += 1;
        if self.retransmits > MAX_RETRANSMITS {
            // 3.8.3: past R2 the connection is aborted and the user told.
            self.abort();
            return;
        }
        // RFC 6298 (5.5): "the host MUST set RTO <- RTO * 2".
        self.rto_ms = (self.rto_ms.saturating_mul(2)).min(MAX_RTO_MS);
        if self.retransmits >= BACKOFFS_BEFORE_FORGETTING {
            // RFC 6298 5: after several backoffs the estimate "is likely...
            // bogus", so it is thrown away and rebuilt from the next sample
            // with (2.2) rather than smoothed towards the truth from a value
            // that was never near it.
            self.srtt_ms = 0;
            self.rttvar_ms = 0;
        }
        // RFC 5681 3.1: loss found by the timer sets "ssthresh = max
        // (FlightSize / 2, 2*SMSS)" (4) and cwnd to the loss window, "which
        // equals 1 full-sized segment".
        let smss = u32::from(self.send_mss);
        let in_flight = self.snd_una.distance_to(self.snd_nxt);
        if in_flight > 0 {
            self.ssthresh = (in_flight / 2).max(2 * smss);
            self.cwnd = smss;
        }
        self.recovering = false;
        self.duplicates = 0;
        self.resend_front();
    }

    /// Put the front of the retransmission queue on the line again.
    ///
    /// The timer's business and fast retransmit's are the same segment; only
    /// what led to it differs, so what leads to it is above and this is here.
    fn resend_front(&mut self) {
        // The sample in flight is now ambiguous (Karn's algorithm, RFC 6298
        // 3), so it is abandoned rather than used.
        self.timing = None;

        let sequence = self.snd_una;
        let in_flight = self.snd_una.distance_to(self.snd_nxt);
        if in_flight == 0 {
            // Nothing is outstanding, so this is 3.8.6.1's probe: one octet
            // past a window of nothing, whose answer carries the window that
            // reopens. It goes into the sequence space like any other, so if
            // it is dropped it is resent by the ordinary path above.
            if self.snd_wnd == 0 && !self.outgoing.is_empty() {
                let (octet, rcv_nxt) = (self.outgoing[0], self.rcv_nxt);
                self.snd_nxt += 1;
                self.out.push(Segment {
                    sequence: sequence.0,
                    acknowledgment: rcv_nxt.0,
                    flags: flag::ACK,
                    payload: vec![octet],
                    ..self.blank()
                });
                self.arm_retransmit();
            } else {
                self.retransmit_at = None;
            }
            return;
        }

        // A SYN that has not been acknowledged is the whole of the queue.
        if matches!(self.state, State::SynSent | State::SynReceived) && self.outgoing.is_empty() {
            let flags = if self.state == State::SynSent {
                flag::SYN
            } else {
                flag::SYN | flag::ACK
            };
            let (iss, rcv_nxt, mss) = (self.iss, self.rcv_nxt, self.recv_mss);
            let mut again = Segment {
                sequence: iss.0,
                acknowledgment: rcv_nxt.0,
                flags,
                mss: Some(mss),
                ..self.blank()
            };
            if self.state == State::SynSent {
                again.acknowledgment = 0;
            }
            self.out.push(again);
            self.arm_retransmit();
            return;
        }

        // Or the FIN, when everything before it is acknowledged.
        if self.outgoing.is_empty() && self.fin_sent {
            let (snd_una, rcv_nxt) = (self.snd_una, self.rcv_nxt);
            self.out.push(Segment {
                sequence: snd_una.0,
                acknowledgment: rcv_nxt.0,
                flags: flag::FIN | flag::ACK,
                ..self.blank()
            });
            self.arm_retransmit();
            return;
        }

        // Otherwise the front of the data, up to one segment of it. 3.8.6.1's
        // zero window probe falls out of the same branch: when the far end has
        // announced nothing, one octet still goes, and its answer carries the
        // window that reopens.
        let size = self
            .outgoing
            .len()
            .min(usize::from(self.send_mss))
            .min(in_flight as usize)
            .max(1)
            .min(self.outgoing.len());
        let payload: Vec<u8> = self.outgoing.iter().take(size).copied().collect();
        let rcv_nxt = self.rcv_nxt;
        self.out.push(Segment {
            sequence: sequence.0,
            acknowledgment: rcv_nxt.0,
            flags: flag::ACK | flag::PSH,
            payload,
            ..self.blank()
        });
        self.arm_retransmit();
    }

    fn enter_time_wait(&mut self) {
        self.state = State::TimeWait;
        self.retransmit_at = None;
        self.ack_at = None;
        self.time_wait_at = Some(self.clock + u64::from(TIME_WAIT_MS));
    }

    fn relisten(&mut self) {
        self.state = State::Listen;
        self.remote = Endpoint::default();
        self.outgoing.clear();
        self.incoming.clear();
        self.held.clear();
        self.closing = false;
        self.fin_sent = false;
        self.retransmit_at = None;
        self.ack_at = None;
        self.snd_una = self.iss;
        self.snd_nxt = self.iss;
    }

    fn give_up(&mut self, why: Report) {
        self.state = State::Closed;
        self.outgoing.clear();
        self.held.clear();
        self.retransmit_at = None;
        self.time_wait_at = None;
        self.ack_at = None;
        self.reports.push(why);
    }
}

/// A segment's four-tuple, for finding the connection it belongs to.
pub fn addressed_to(segment: &Segment, from: [u8; 4], to: [u8; 4]) -> (Endpoint, Endpoint) {
    (
        Endpoint::new(from, segment.source_port),
        Endpoint::new(to, segment.destination_port),
    )
}

/// The size of a segment header with no options, exported so a caller can
/// work out what will fit in a datagram.
pub const HEADER_LEN: usize = segment::HEADER_LEN;
