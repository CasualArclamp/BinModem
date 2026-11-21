//! MNP error control, as USRobotics' HST carries it.
//!
//! MNP classes 2 to 4 are a link protocol of numbered data frames, windowed
//! acknowledgements and an attention mechanism, framed here (as on HST) by
//! HDLC: flags, zero insertion and a 16-bit FCS, which [`crate::hdlc`]
//! already does. What this module does is everything inside a frame.
//!
//! Where the layout comes from:
//!
//! - **Standard headers**, used for link setup: a length indicator (LI) octet,
//!   the type, and parameters as type, length, value. The link request is
//!   the template in the Courier's firmware (0x856E7), byte for byte, with
//!   USR's parameter 0xC0 appended; the disconnect is the firmware's
//!   `04 02 01 01 reason`. The acknowledgement and attention frames follow
//!   MNP's published parameter form.
//! - **Optimized headers**, once both ends have agreed parameter 8: no LI,
//!   the type in the first octet, then fixed fields. The Courier's receive
//!   path (8FBB:1892) and its frame builders (0x862B8, 0x861E1, 0x85F8C,
//!   0x91DC2) give these: `04 seq` + data, `05 seq credit`, `06 seq code`,
//!   `07 seq`, `02 reason`.
//! - **USR parameter 0xC0**, two octets in the link request: HST flags
//!   (bit 0 HST, bit 3 the 300 bit/s back channel) and a mask of the forward
//!   rates acceptable, bit n for rate code n (bits a symbol, 2 to 7). The
//!   firmware builds it at 0x85A82 and reads it at 0x85422; the responder
//!   keeps only the rates both ends allow.
//!
//! The Courier's other USR frame types (an acknowledgement variant, data
//! with a piggybacked acknowledgement, and short data frames) are not used
//! here; this end neither sends nor needs them.
//!
//! Sans I/O: frames in through [`Link::receive`], frames out through
//! [`Link::poll_transmit`], time through [`Link::tick`].

use std::collections::VecDeque;

/// Frame types, MNP's numbering.
pub mod kind {
    pub const LR: u8 = 1;
    pub const LD: u8 = 2;
    pub const LT: u8 = 4;
    pub const LA: u8 = 5;
    pub const LN: u8 = 6;
    pub const LNA: u8 = 7;
}

/// USR's HST parameter (0xC0) in a link request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HstParam {
    pub flags: u8,
    /// Bit n set: forward rate code n (n bits a symbol, 2400n bit/s) allowed.
    pub rates: u8,
}

impl HstParam {
    pub const HST: u8 = 0x01;
    pub const BACK_300: u8 = 0x08;

    /// The fastest rate code both masks allow, if any.
    pub fn best_rate(self) -> Option<u8> {
        (2..=7).rev().find(|&n| self.rates & (1 << n) != 0)
    }
}

/// A link request's parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lr {
    /// Service class: 2 or 3 in practice; 3 is the synchronous class HST runs.
    pub class: u8,
    /// Frames that may be outstanding.
    pub window: u8,
    /// Largest data field of a data frame, in octets.
    pub max_data: u16,
    /// Parameter 8, data phase optimization: nonzero to use optimized headers.
    pub optimize: u8,
    pub hst: Option<HstParam>,
}

/// One MNP frame, without its HDLC flags and FCS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Lr(Lr),
    Ld { reason: u8 },
    Lt { seq: u8, data: Vec<u8> },
    La { seq: u8, credit: u8 },
    Ln { seq: u8, code: u8 },
    Lna { seq: u8 },
}

/// Which header layout frames use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Header {
    #[default]
    Standard,
    Optimized,
}

/// The firmware's constant parameter 1, as the template has it.
const LR_PARAM1: [u8; 6] = [0x01, 0x00, 0x00, 0x00, 0x00, 0xFF];

impl Frame {
    pub fn encode(&self, header: Header) -> Vec<u8> {
        use kind::*;
        match (self, header) {
            (Frame::Lr(lr), _) => {
                let mut b = vec![0, LR, 0x02, 0x01, 6];
                b.extend(LR_PARAM1);
                b.extend([0x02, 1, lr.class, 0x03, 1, lr.window, 0x04, 2]);
                b.extend(lr.max_data.to_le_bytes());
                if lr.optimize != 0 {
                    b.extend([0x08, 1, lr.optimize]);
                }
                if let Some(h) = lr.hst {
                    b.extend([0xC0, 2, h.flags, h.rates]);
                }
                b[0] = (b.len() - 1) as u8;
                b
            }
            (Frame::Ld { reason }, Header::Standard) => vec![4, LD, 0x01, 1, *reason],
            (Frame::Ld { reason }, Header::Optimized) => vec![LD, *reason],
            (Frame::Lt { seq, data }, Header::Standard) => {
                let mut b = vec![2, LT, *seq];
                b.extend(data);
                b
            }
            (Frame::Lt { seq, data }, Header::Optimized) => {
                let mut b = vec![LT, *seq];
                b.extend(data);
                b
            }
            (Frame::La { seq, credit }, Header::Standard) => vec![7, LA, 0x01, 1, *seq, 0x02, 1, *credit],
            (Frame::La { seq, credit }, Header::Optimized) => vec![LA, *seq, *credit],
            (Frame::Ln { seq, code }, Header::Standard) => vec![7, LN, 0x01, 1, *seq, 0x02, 1, *code],
            (Frame::Ln { seq, code }, Header::Optimized) => vec![LN, *seq, *code],
            (Frame::Lna { seq }, Header::Standard) => vec![4, LNA, 0x01, 1, *seq],
            (Frame::Lna { seq }, Header::Optimized) => vec![LNA, *seq],
        }
    }

    pub fn decode(b: &[u8], header: Header) -> Option<Frame> {
        match header {
            Header::Standard => Self::decode_standard(b),
            Header::Optimized => Self::decode_optimized(b),
        }
    }

    fn decode_standard(b: &[u8]) -> Option<Frame> {
        use kind::*;
        let li = usize::from(*b.first()?);
        if li < 1 || b.len() < li + 1 {
            return None;
        }
        let head = &b[1..=li];
        let ty = head[0];
        if ty == LT {
            return Some(Frame::Lt { seq: *head.get(1)?, data: b[li + 1..].to_vec() });
        }
        // Everything else is the type then parameters; the link request has
        // a constant octet between.
        let params = if ty == LR { head.get(2..)? } else { &head[1..] };
        let mut p = Params::default();
        let mut i = 0;
        while i + 2 <= params.len() {
            let (t, n) = (params[i], usize::from(params[i + 1]));
            let v = params.get(i + 2..i + 2 + n)?;
            p.set(t, v);
            i += 2 + n;
        }
        match ty {
            LR => Some(Frame::Lr(Lr {
                class: p.get8(2)?,
                window: p.get8(3)?,
                max_data: p.get16(4).unwrap_or(64),
                optimize: p.get8(8).unwrap_or(0),
                hst: p.c0.map(|(flags, rates)| HstParam { flags, rates }),
            })),
            LD => Some(Frame::Ld { reason: p.get8(1).unwrap_or(0) }),
            LA => Some(Frame::La { seq: p.get8(1)?, credit: p.get8(2)? }),
            LN => Some(Frame::Ln { seq: p.get8(1)?, code: p.get8(2)? }),
            LNA => Some(Frame::Lna { seq: p.get8(1)? }),
            _ => None,
        }
    }

    fn decode_optimized(b: &[u8]) -> Option<Frame> {
        use kind::*;
        let ty = *b.first()?;
        match ty {
            LT => Some(Frame::Lt { seq: *b.get(1)?, data: b[2..].to_vec() }),
            LA => Some(Frame::La { seq: *b.get(1)?, credit: *b.get(2)? }),
            LN => Some(Frame::Ln { seq: *b.get(1)?, code: *b.get(2)? }),
            LNA => Some(Frame::Lna { seq: *b.get(1)? }),
            LD => Some(Frame::Ld { reason: b.get(1).copied().unwrap_or(0) }),
            // A link request always goes in the standard layout.
            _ if b.len() > 1 && b[1] == LR => Self::decode_standard(b),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Params {
    small: [Option<u8>; 16],
    wide: [Option<u16>; 16],
    c0: Option<(u8, u8)>,
}

impl Params {
    fn set(&mut self, t: u8, v: &[u8]) {
        match (t, v.len()) {
            (0xC0, 2) => self.c0 = Some((v[0], v[1])),
            (t @ 0..=15, 1) => self.small[usize::from(t)] = Some(v[0]),
            (t @ 0..=15, 2) => self.wide[usize::from(t)] = Some(u16::from_le_bytes([v[0], v[1]])),
            _ => {}
        }
    }
    fn get8(&self, t: usize) -> Option<u8> {
        self.small[t]
    }
    fn get16(&self, t: usize) -> Option<u16> {
        self.wide[t]
    }
}

/// Which end of the link this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Sends the first link request.
    Originator,
    /// Answers one.
    Responder,
}

/// What both ends agreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agreed {
    pub window: u8,
    pub max_data: u16,
    pub header: Header,
    pub hst: Option<HstParam>,
}

impl Agreed {
    fn between(ours: &Lr, theirs: &Lr) -> Self {
        let hst = match (ours.hst, theirs.hst) {
            (Some(a), Some(b)) if a.flags & b.flags & HstParam::HST != 0 => {
                Some(HstParam { flags: a.flags & b.flags | (a.flags | b.flags) & HstParam::BACK_300, rates: a.rates & b.rates & 0xFC })
            }
            _ => None,
        };
        Agreed {
            window: ours.window.min(theirs.window).max(1),
            max_data: ours.max_data.min(theirs.max_data).max(1),
            header: if ours.optimize != 0 && theirs.optimize != 0 { Header::Optimized } else { Header::Standard },
            hst,
        }
    }

    fn as_lr(&self, class: u8) -> Lr {
        Lr {
            class,
            window: self.window,
            max_data: self.max_data,
            optimize: u8::from(self.header == Header::Optimized) * 3,
            hst: self.hst,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    /// Link request out (originator) or awaited (responder).
    Connecting,
    /// Responder: agreed and answered, waiting for the confirming acknowledgement.
    Confirming,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connected(Agreed),
    /// The far end sent an attention with this code.
    Attention(u8),
    /// The far end acknowledged our attention with this code.
    AttentionAcked(u8),
    Disconnected { reason: u8 },
}

/// Disconnect reasons this end gives.
pub mod reason {
    pub const USER: u8 = 0xFF;
    pub const RETRIES: u8 = 0x04;
    pub const NO_LR: u8 = 0x01;
}

/// Timing and retry settings.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// No progress on outstanding frames for this long and they go again.
    pub retransmit_ms: u32,
    /// Retransmissions without progress before giving up.
    pub retries: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self { retransmit_ms: 2000, retries: 12 }
    }
}

/// One end of an MNP link.
#[derive(Debug)]
pub struct Link {
    role: Role,
    ours: Lr,
    agreed: Option<Agreed>,
    state: State,
    timing: Timing,
    /// Header layout frames go out in, and are read in.
    tx_header: Header,
    rx_header: Header,
    control: VecDeque<Frame>,
    events: VecDeque<Event>,
    // Sending.
    outgoing: VecDeque<u8>,
    next_seq: u8,
    /// Sent and not yet acknowledged, oldest first, with whether each is
    /// waiting to go (again).
    unacked: VecDeque<(u8, Vec<u8>, bool)>,
    credit: u8,
    timer_ms: u32,
    tries: u32,
    // Receiving.
    expected: u8,
    incoming: Vec<u8>,
    ack_due: bool,
    // Attention.
    ln_seq: u8,
    ln_out: Option<(u8, u8)>,
    ln_timer_ms: u32,
    ln_seen: Option<u8>,
    retransmitted: u64,
}

impl Link {
    pub fn new(role: Role, ours: Lr) -> Self {
        Self {
            role,
            ours,
            agreed: None,
            state: State::Idle,
            timing: Timing::default(),
            tx_header: Header::Standard,
            rx_header: Header::Standard,
            control: VecDeque::new(),
            events: VecDeque::new(),
            outgoing: VecDeque::new(),
            next_seq: 1,
            unacked: VecDeque::new(),
            credit: 0,
            timer_ms: 0,
            tries: 0,
            expected: 1,
            incoming: Vec::new(),
            ack_due: false,
            ln_seq: 0,
            ln_out: None,
            ln_timer_ms: 0,
            ln_seen: None,
            retransmitted: 0,
        }
    }

    pub fn set_timing(&mut self, timing: Timing) {
        self.timing = timing;
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Data frames sent again since the link started.
    pub fn retransmitted(&self) -> u64 {
        self.retransmitted
    }

    pub fn agreed(&self) -> Option<Agreed> {
        self.agreed
    }

    pub fn is_connected(&self) -> bool {
        self.state == State::Connected
    }

    /// Start: the originator sends its link request; the responder listens.
    pub fn connect(&mut self) {
        self.state = State::Connecting;
        self.timer_ms = 0;
        self.tries = 0;
        if self.role == Role::Originator {
            self.control.push_back(Frame::Lr(self.ours));
        }
    }

    pub fn disconnect(&mut self, why: u8) {
        if matches!(self.state, State::Connected | State::Confirming) {
            self.control.push_back(Frame::Ld { reason: why });
        }
        self.finish(why);
    }

    fn finish(&mut self, why: u8) {
        if self.state != State::Disconnected {
            self.state = State::Disconnected;
            self.events.push_back(Event::Disconnected { reason: why });
        }
    }

    /// Queue data to send.
    pub fn send_data(&mut self, data: &[u8]) {
        self.outgoing.extend(data);
    }

    /// Octets waiting to go, including those sent and not yet acknowledged.
    pub fn backlog(&self) -> usize {
        self.outgoing.len() + self.unacked.iter().map(|(_, d, _)| d.len()).sum::<usize>()
    }

    /// Data received in order since the last call.
    pub fn read(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.incoming)
    }

    /// Send an attention with `code`. One at a time: a second before the
    /// first is acknowledged replaces it.
    pub fn attention(&mut self, code: u8) {
        self.ln_seq = self.ln_seq.wrapping_add(1);
        self.ln_out = Some((self.ln_seq, code));
        self.ln_timer_ms = 0;
        self.control.push_back(Frame::Ln { seq: self.ln_seq, code });
    }

    /// The code of an attention sent and not yet acknowledged.
    pub fn attention_pending(&self) -> Option<u8> {
        self.ln_out.map(|(_, c)| c)
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// The next control frame to send (a request, acknowledgement,
    /// attention or disconnect), without starting any data: for an end that
    /// is about to hand its channel over.
    pub fn poll_control(&mut self) -> Option<Vec<u8>> {
        if self.control.is_empty() && !(self.ack_due && self.state == State::Connected) {
            return None;
        }
        if self.control.is_empty() {
            self.ack_due = false;
            let credit = self.agreed.map_or(1, |a| a.window);
            return Some(Frame::La { seq: self.expected.wrapping_sub(1), credit }.encode(self.tx_header));
        }
        self.poll_transmit()
    }

    /// The next frame to send, encoded, if any; call when the line can take
    /// one.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        if let Some(f) = self.control.pop_front() {
            // The originator's confirming acknowledgement is the last
            // standard frame it sends.
            let bytes = f.encode(self.tx_header);
            if self.role == Role::Originator && matches!(f, Frame::La { seq: 0, .. })
                && let Some(a) = self.agreed {
                    self.tx_header = a.header;
                }
            return Some(bytes);
        }
        if self.state != State::Connected {
            return None;
        }
        if self.ack_due {
            self.ack_due = false;
            let credit = self.agreed.map_or(1, |a| a.window);
            return Some(Frame::La { seq: self.expected.wrapping_sub(1), credit }.encode(self.tx_header));
        }
        // A retransmission due.
        if let Some(i) = self.unacked.iter().position(|(_, _, due)| *due) {
            let (seq, data, due) = &mut self.unacked[i];
            *due = false;
            self.retransmitted += 1;
            return Some(Frame::Lt { seq: *seq, data: data.clone() }.encode(self.tx_header));
        }
        let max = usize::from(self.agreed.map_or(64, |a| a.max_data));
        if !self.outgoing.is_empty() && self.unacked.len() < usize::from(self.credit) {
            let n = self.outgoing.len().min(max);
            let data: Vec<u8> = self.outgoing.drain(..n).collect();
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            if self.unacked.is_empty() {
                self.timer_ms = 0;
            }
            self.unacked.push_back((seq, data.clone(), false));
            return Some(Frame::Lt { seq, data }.encode(self.tx_header));
        }
        None
    }

    pub fn tick(&mut self, dt_ms: u32) {
        match self.state {
            State::Connecting | State::Confirming => {
                self.timer_ms += dt_ms;
                if self.timer_ms >= self.timing.retransmit_ms {
                    self.timer_ms = 0;
                    self.tries += 1;
                    if self.tries > self.timing.retries {
                        self.finish(reason::NO_LR);
                        return;
                    }
                    match (self.role, self.state) {
                        (Role::Originator, State::Connecting) => self.control.push_back(Frame::Lr(self.ours)),
                        (Role::Responder, State::Confirming) => {
                            if let Some(a) = self.agreed {
                                self.control.push_back(Frame::Lr(a.as_lr(self.ours.class)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            State::Connected => {
                if !self.unacked.is_empty() {
                    self.timer_ms += dt_ms;
                    if self.timer_ms >= self.timing.retransmit_ms {
                        self.timer_ms = 0;
                        self.tries += 1;
                        if self.tries > self.timing.retries {
                            self.disconnect(reason::RETRIES);
                            return;
                        }
                        for f in &mut self.unacked {
                            f.2 = true;
                        }
                        // Ask where the far end has got to.
                        self.ack_due = true;
                    }
                }
                if let Some((seq, code)) = self.ln_out {
                    self.ln_timer_ms += dt_ms;
                    if self.ln_timer_ms >= self.timing.retransmit_ms {
                        self.ln_timer_ms = 0;
                        self.control.push_back(Frame::Ln { seq, code });
                    }
                }
            }
            _ => {}
        }
    }

    /// A frame from the far end, flags and FCS already stripped and checked.
    pub fn receive(&mut self, bytes: &[u8]) {
        // A link request is always standard, whatever has been agreed.
        let frame = Frame::decode(bytes, self.rx_header).or_else(|| match Frame::decode(bytes, Header::Standard) {
            Some(f @ Frame::Lr(_)) => Some(f),
            _ => None,
        });
        let Some(frame) = frame else { return };
        match frame {
            Frame::Lr(theirs) => self.on_lr(theirs),
            Frame::Ld { reason } => self.finish(reason),
            Frame::La { seq, credit } => self.on_la(seq, credit),
            Frame::Lt { seq, data } => self.on_lt(seq, data),
            Frame::Ln { seq, code } => {
                if self.state != State::Connected {
                    return;
                }
                if self.ln_seen != Some(seq) {
                    self.ln_seen = Some(seq);
                    self.events.push_back(Event::Attention(code));
                }
                self.control.push_back(Frame::Lna { seq });
            }
            Frame::Lna { seq } => {
                if let Some((s, code)) = self.ln_out
                    && s == seq {
                        self.ln_out = None;
                        self.events.push_back(Event::AttentionAcked(code));
                    }
            }
        }
    }

    fn on_lr(&mut self, theirs: Lr) {
        match (self.role, self.state) {
            (Role::Responder, State::Connecting | State::Confirming) => {
                let a = Agreed::between(&self.ours, &theirs);
                self.agreed = Some(a);
                self.state = State::Confirming;
                self.timer_ms = 0;
                self.control.push_back(Frame::Lr(a.as_lr(self.ours.class)));
            }
            (Role::Originator, State::Connecting) => {
                // The responder's request carries what it agreed; take the
                // same view of it from our side.
                let a = Agreed::between(&self.ours, &theirs);
                self.connected(a);
                // The responder sends in the agreed layout from the moment
                // our confirmation reaches it.
                self.rx_header = a.header;
                self.control.push_back(Frame::La { seq: 0, credit: a.window });
            }
            (Role::Originator, State::Connected) => {
                // Our confirmation was lost: confirm again, in the layout the
                // responder is still reading.
                if let Some(a) = self.agreed {
                    self.control.push_back(Frame::La { seq: 0, credit: a.window });
                    self.tx_header = Header::Standard;
                }
            }
            _ => {}
        }
    }

    fn connected(&mut self, a: Agreed) {
        self.agreed = Some(a);
        self.state = State::Connected;
        self.credit = a.window;
        self.timer_ms = 0;
        self.tries = 0;
        self.events.push_back(Event::Connected(a));
    }

    fn on_la(&mut self, seq: u8, credit: u8) {
        if self.role == Role::Responder && self.state == State::Confirming {
            if let Some(a) = self.agreed {
                self.connected(a);
                self.rx_header = a.header;
                self.tx_header = a.header;
            }
            return;
        }
        if self.state != State::Connected {
            return;
        }
        // Drop everything up to and including `seq`.
        let mut progressed = false;
        while let Some((s, _, _)) = self.unacked.front() {
            let behind = seq.wrapping_sub(*s);
            if behind < 128 {
                self.unacked.pop_front();
                progressed = true;
            } else {
                break;
            }
        }
        self.credit = credit.max(1);
        if progressed {
            self.timer_ms = 0;
            self.tries = 0;
        }
    }

    fn on_lt(&mut self, seq: u8, data: Vec<u8>) {
        if self.state == State::Confirming {
            // Data before the confirmation means the confirmation was lost
            // but the originator is connected: so are we.
            if let Some(a) = self.agreed {
                self.connected(a);
                self.rx_header = a.header;
                self.tx_header = a.header;
                // This frame was read as standard; it was not. Drop it and
                // let the acknowledgement bring it back.
                self.ack_due = true;
                return;
            }
        }
        if self.state != State::Connected {
            return;
        }
        if seq == self.expected {
            self.incoming.extend(data);
            self.expected = self.expected.wrapping_add(1);
        }
        self.ack_due = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lr(hst: Option<HstParam>) -> Lr {
        Lr { class: 3, window: 8, max_data: 64, optimize: 3, hst }
    }

    #[test]
    fn the_link_request_is_the_couriers_template() {
        let b = Frame::Lr(Lr { class: 2, window: 8, max_data: 64, optimize: 0, hst: None }).encode(Header::Standard);
        let template = [0x14, 1, 2, 1, 6, 1, 0, 0, 0, 0, 0xFF, 2, 1, 3, 3, 1, 8, 4, 2, 0x40, 0];
        // The template's class octet is 3; ours says 2 here on purpose, to
        // see it land in the right place.
        let mut want = template;
        want[13] = 2;
        assert_eq!(b, want);
    }

    #[test]
    fn every_frame_survives_both_layouts() {
        let frames = vec![
            Frame::Lt { seq: 7, data: b"hello".to_vec() },
            Frame::La { seq: 9, credit: 8 },
            Frame::Ln { seq: 2, code: 0x14 },
            Frame::Lna { seq: 2 },
            Frame::Ld { reason: 0xFF },
        ];
        for h in [Header::Standard, Header::Optimized] {
            for f in &frames {
                assert_eq!(Frame::decode(&f.encode(h), h).as_ref(), Some(f), "{f:?} in {h:?}");
            }
        }
        let r = Frame::Lr(lr(Some(HstParam { flags: 1, rates: 0x3C })));
        assert_eq!(Frame::decode(&r.encode(Header::Standard), Header::Standard), Some(r.clone()));
        assert_eq!(Frame::decode(&r.encode(Header::Standard), Header::Optimized), Some(r));
    }

    /// Two links joined directly, with frames dropped where `drop` says.
    fn run(a: &mut Link, b: &mut Link, steps: usize, drop: &mut dyn FnMut(usize) -> bool) {
        let mut n = 0;
        for _ in 0..steps {
            for (from, to) in [(0, 1), (1, 0)] {
                let (x, y) = if from == 0 { (&mut *a, &mut *b) } else { (&mut *b, &mut *a) };
                if let Some(f) = x.poll_transmit() {
                    n += 1;
                    if !drop(n) {
                        y.receive(&f);
                    }
                }
                let _ = to;
            }
            a.tick(50);
            b.tick(50);
        }
    }

    fn pair() -> (Link, Link) {
        let mut a = Link::new(Role::Originator, lr(Some(HstParam { flags: 1, rates: 0x7C })));
        let mut b = Link::new(Role::Responder, lr(Some(HstParam { flags: 1 | 8, rates: 0x1C })));
        a.connect();
        b.connect();
        (a, b)
    }

    #[test]
    fn two_ends_agree_hst_and_the_rates_both_allow() {
        let (mut a, mut b) = pair();
        run(&mut a, &mut b, 10, &mut |_| false);
        assert!(a.is_connected() && b.is_connected());
        let agreed = a.agreed().unwrap();
        assert_eq!(agreed, b.agreed().unwrap());
        assert_eq!(agreed.header, Header::Optimized);
        let hst = agreed.hst.unwrap();
        assert_eq!(hst.rates, 0x1C);
        assert_eq!(hst.best_rate(), Some(4));
        assert!(hst.flags & HstParam::BACK_300 != 0);
    }

    #[test]
    fn data_crosses_both_ways_in_order() {
        let (mut a, mut b) = pair();
        let one: Vec<u8> = (0..3000u32).map(|i| (i * 7 + 3) as u8).collect();
        let two: Vec<u8> = (0..700u32).map(|i| (i * 13 + 1) as u8).collect();
        a.send_data(&one);
        b.send_data(&two);
        run(&mut a, &mut b, 400, &mut |_| false);
        let (got_b, got_a) = (b.read(), a.read());
        assert!(got_b == one, "b got {} of {} octets", got_b.len(), one.len());
        assert!(got_a == two, "a got {} of {} octets", got_a.len(), two.len());
    }

    #[test]
    fn lost_frames_come_back() {
        let (mut a, mut b) = pair();
        let one: Vec<u8> = (0..4000u32).map(|i| (i * 31 + 5) as u8).collect();
        a.send_data(&one);
        // Lose one frame in nine, after the link is up.
        run(&mut a, &mut b, 3000, &mut |n| n > 6 && n % 9 == 0);
        let got = b.read();
        assert!(got == one, "got {} of {} octets", got.len(), one.len());
        assert_eq!(a.backlog(), 0);
        assert!(a.retransmitted() > 0);
    }

    #[test]
    fn attentions_are_delivered_once_and_acknowledged() {
        let (mut a, mut b) = pair();
        run(&mut a, &mut b, 10, &mut |_| false);
        while a.poll_event().is_some() {}
        while b.poll_event().is_some() {}
        a.attention(0x14);
        // Lose the first try at the attention.
        let mut first = true;
        run(&mut a, &mut b, 100, &mut |_| std::mem::take(&mut first));
        let b_events: Vec<Event> = std::iter::from_fn(|| b.poll_event()).collect();
        let a_events: Vec<Event> = std::iter::from_fn(|| a.poll_event()).collect();
        assert_eq!(b_events, vec![Event::Attention(0x14)]);
        assert_eq!(a_events, vec![Event::AttentionAcked(0x14)]);
        assert_eq!(a.attention_pending(), None);
    }

    #[test]
    fn sequence_numbers_wrap_without_loss() {
        let (mut a, mut b) = pair();
        let one: Vec<u8> = (0..40_000u32).map(|i| (i * 17 + 9) as u8).collect();
        a.send_data(&one);
        run(&mut a, &mut b, 20_000, &mut |n| n > 6 && n % 23 == 0);
        let got = b.read();
        assert!(got == one, "got {} of {} octets", got.len(), one.len());
    }

    #[test]
    fn a_lost_link_request_or_confirmation_is_recovered() {
        // Frames 1, 2, 3 are the originator's request, the responder's reply
        // and the confirmation; lose each in turn.
        for lost in 1..=3 {
            let (mut a, mut b) = pair();
            a.send_data(b"after the handshake");
            run(&mut a, &mut b, 300, &mut |n| n == lost);
            assert!(a.is_connected() && b.is_connected(), "losing frame {lost}");
            assert_eq!(b.read(), b"after the handshake", "losing frame {lost}");
        }
    }

    #[test]
    fn a_disconnect_ends_both() {
        let (mut a, mut b) = pair();
        run(&mut a, &mut b, 10, &mut |_| false);
        a.disconnect(reason::USER);
        run(&mut a, &mut b, 5, &mut |_| false);
        assert_eq!(a.state(), State::Disconnected);
        assert_eq!(b.state(), State::Disconnected);
    }
}
