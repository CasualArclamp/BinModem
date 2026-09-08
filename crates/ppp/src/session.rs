//! Running the automaton: the part that turns a state table into packets.
//!
//! [`super::control`] says what to do and this does it. Both are shared by
//! every control protocol, so a [`Session`] is told what it is negotiating by
//! a [`Protocol`] and does not otherwise care.

use crate::control::{
    Action, Code, ConfigOption, Event, Limits, Message, State, transition,
};

/// What a control protocol has to be able to say for itself.
pub trait Protocol {
    /// The PPP protocol number its packets travel under.
    fn number(&self) -> u16;
    /// The options to put in a Configure-Request.
    fn request(&self) -> Vec<ConfigOption>;
    /// What to answer the peer's request with, recording what is agreed if the
    /// answer is an acknowledgement.
    fn review(&mut self, options: &[ConfigOption]) -> crate::lcp::Review;
    /// The peer has agreed to what this end asked for.
    fn acked(&mut self, options: &[ConfigOption]);
    /// The peer wants different values. 5.3: what comes back is what it would
    /// accept, so the next request should carry those.
    fn naked(&mut self, options: &[ConfigOption]);
    /// The peer will not discuss these at all. 5.4: stop asking.
    fn rejected(&mut self, options: &[ConfigOption]);
}

/// What a session has to tell the layer above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// 4.4's This-Layer-Up: the negotiation succeeded and this protocol is
    /// usable.
    Up,
    /// This-Layer-Down: it is not, any more.
    Down,
    /// This-Layer-Started: something below should be brought up.
    Started,
    /// This-Layer-Finished: and taken down.
    Finished,
}

/// One control protocol's negotiation, in progress.
#[derive(Debug)]
pub struct Session<P: Protocol> {
    pub protocol: P,
    state: State,
    limits: Limits,
    /// 4.6's Restart counter, counting down.
    restarts: u32,
    /// Milliseconds until the Restart timer expires, if it is running.
    timer: Option<u32>,
    /// 5: the Identifier of the request this end has outstanding.
    id: u8,
    next_id: u8,
    /// The request being answered, kept from classifying it to acting on it.
    answering: Option<(u8, Vec<ConfigOption>)>,
    /// Whether that answer is 5.4's Configure-Reject rather than 5.3's
    /// Configure-Nak. The automaton has one event for both -- "this end
    /// cannot agree" -- and only the packet knows which.
    nak_is_reject: bool,
    /// A code this end did not recognise, for the rejection that owes it.
    unknown: Option<Message>,
    /// An Echo-Request waiting for its reply.
    echo: Option<Message>,
    out: Vec<Message>,
    reports: Vec<Report>,
}

impl<P: Protocol> Session<P> {
    pub fn new(protocol: P, limits: Limits) -> Self {
        Self {
            protocol,
            state: State::Initial,
            limits,
            restarts: 0,
            timer: None,
            id: 0,
            next_id: 1,
            answering: None,
            nak_is_reject: false,
            unknown: None,
            echo: None,
            out: Vec::new(),
            reports: Vec::new(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Packets to put on the line, oldest first.
    pub fn take_output(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.out)
    }

    /// What the layer above needs to know about.
    pub fn take_reports(&mut self) -> Vec<Report> {
        std::mem::take(&mut self.reports)
    }

    /// This end wants the link (4.3's Open event).
    pub fn open(&mut self) {
        self.fire(Event::Open);
    }

    /// And does not.
    pub fn close(&mut self) {
        self.fire(Event::Close);
    }

    /// The layer below can carry frames now.
    pub fn up(&mut self) {
        self.fire(Event::Up);
    }

    /// And cannot.
    pub fn down(&mut self) {
        self.fire(Event::Down);
    }

    /// Advance the Restart timer.
    pub fn tick(&mut self, ms: u32) {
        let Some(left) = self.timer else { return };
        if left > ms {
            self.timer = Some(left - ms);
            return;
        }
        self.timer = None;
        // 4.6: the counter is decremented on every transmission including the
        // first, so a zero here means the last one has already gone out.
        if self.restarts > 0 {
            self.fire(Event::TimeoutWithRestarts);
        } else {
            self.fire(Event::TimeoutNoRestarts);
        }
    }

    /// One packet arrived for this protocol.
    pub fn receive(&mut self, message: Message) {
        match message.code {
            Code::ConfigureRequest => {
                let Some(options) = ConfigOption::parse_all(&message.data) else {
                    // 5.1 gives no way to answer a request whose options
                    // cannot be read: there is nothing to echo back and
                    // nothing to name. Silence lets the peer's own restart
                    // timer try again.
                    return;
                };
                match self.protocol.review(&options) {
                    crate::lcp::Review::Ack => {
                        // 5.2: an acknowledgement is the request echoed back
                        // exactly as it arrived.
                        self.answering = Some((message.id, options));
                        self.fire(Event::ReceiveConfigureRequestGood);
                    }
                    // The automaton has one event for both -- this end cannot
                    // agree -- and which packet says so is settled here, since
                    // only here is the difference still known.
                    crate::lcp::Review::Nak(counter) => {
                        self.answering = Some((message.id, counter));
                        self.nak_is_reject = false;
                        self.fire(Event::ReceiveConfigureRequestBad);
                    }
                    crate::lcp::Review::Reject(counter) => {
                        self.answering = Some((message.id, counter));
                        self.nak_is_reject = true;
                        self.fire(Event::ReceiveConfigureRequestBad);
                    }
                }
            }
            Code::ConfigureAck => {
                // 5.2: "the Identifier field MUST match that of the last
                // transmitted Configure-Request", and an unmatched one is
                // "silently discarded without affecting the automaton".
                if message.id != self.id {
                    return;
                }
                if let Some(options) = ConfigOption::parse_all(&message.data) {
                    self.protocol.acked(&options);
                }
                self.fire(Event::ReceiveConfigureAck);
            }
            Code::ConfigureNak | Code::ConfigureReject => {
                if message.id != self.id {
                    return;
                }
                if let Some(options) = ConfigOption::parse_all(&message.data) {
                    if message.code == Code::ConfigureNak {
                        self.protocol.naked(&options);
                    } else {
                        self.protocol.rejected(&options);
                    }
                }
                self.fire(Event::ReceiveConfigureNak);
            }
            Code::TerminateRequest => {
                // 5.5: the reply carries the request's Identifier.
                self.answering = Some((message.id, Vec::new()));
                self.fire(Event::ReceiveTerminateRequest);
            }
            Code::TerminateAck => self.fire(Event::ReceiveTerminateAck),
            Code::EchoRequest => {
                self.echo = Some(message);
                self.fire(Event::ReceiveEcho);
            }
            Code::EchoReply | Code::DiscardRequest => self.fire(Event::ReceiveEcho),
            // 5.6, 5.7: a rejection of something this end could do without is
            // survivable; one of something it cannot is not. Nothing here is
            // essential to a link that has got this far, so both are taken as
            // recoverable and the link stays up.
            Code::CodeReject | Code::ProtocolReject => {
                self.fire(Event::ReceiveRejectRecoverable);
            }
            Code::Unknown(_) => {
                self.unknown = Some(message);
                self.fire(Event::ReceiveUnknownCode);
            }
        }
    }

    fn fire(&mut self, event: Event) {
        let Some(t) = transition(self.state, event) else {
            // 4.1 calls these illegal, meaning they cannot happen rather than
            // that they are forbidden. Nothing is done and nothing is broken.
            return;
        };
        for action in t.actions {
            self.perform(action);
        }
        // 4.1: "The Restart timer is stopped when transitioning from any state
        // where the timer is running to a state where the timer is not."
        let runs = transition(t.next, Event::TimeoutWithRestarts).is_some();
        if !runs {
            self.timer = None;
        }
        self.state = t.next;
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::ThisLayerUp => self.reports.push(Report::Up),
            Action::ThisLayerDown => self.reports.push(Report::Down),
            Action::ThisLayerStarted => self.reports.push(Report::Started),
            Action::ThisLayerFinished => self.reports.push(Report::Finished),
            Action::InitializeRestartCount => {
                self.restarts = self.limits.max_configure;
            }
            Action::ZeroRestartCount => self.restarts = 0,
            Action::SendConfigureRequest => {
                self.id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                let mut data = Vec::new();
                ConfigOption::write_all(&self.protocol.request(), &mut data);
                self.send(Code::ConfigureRequest, self.id, data);
                self.restart();
            }
            Action::SendConfigureAck => {
                if let Some((id, options)) = self.answering.take() {
                    let mut data = Vec::new();
                    ConfigOption::write_all(&options, &mut data);
                    self.send(Code::ConfigureAck, id, data);
                }
            }
            Action::SendConfigureNak => {
                if let Some((id, options)) = self.answering.take() {
                    let mut data = Vec::new();
                    ConfigOption::write_all(&options, &mut data);
                    let code = if self.nak_is_reject {
                        Code::ConfigureReject
                    } else {
                        Code::ConfigureNak
                    };
                    self.send(code, id, data);
                }
            }
            Action::SendTerminateRequest => {
                self.id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                self.send(Code::TerminateRequest, self.id, Vec::new());
                self.restart();
            }
            Action::SendTerminateAck => {
                let id = self.answering.take().map_or(self.id, |(id, _)| id);
                self.send(Code::TerminateAck, id, Vec::new());
            }
            Action::SendCodeReject => {
                if let Some(message) = self.unknown.take() {
                    // 5.6: the rejected packet is copied in, "beginning with
                    // the Information field", truncated to fit the MRU.
                    let mut data = message.to_bytes();
                    data.truncate(usize::from(crate::lcp::DEFAULT_MRU) - 8);
                    let id = self.next_id;
                    self.next_id = self.next_id.wrapping_add(1);
                    self.send(Code::CodeReject, id, data);
                }
            }
            Action::SendEchoReply => {
                if let Some(request) = self.echo.take() {
                    // 5.8: the Identifier is the request's, and the Magic-
                    // Number is this end's own. The four octets of magic are
                    // at the head of the data either way.
                    let mut data = request.data.clone();
                    if data.len() >= 4 {
                        data[..4].copy_from_slice(&[0, 0, 0, 0]);
                    }
                    self.send(Code::EchoReply, request.id, data);
                }
            }
        }
    }

    fn restart(&mut self) {
        self.restarts = self.restarts.saturating_sub(1);
        self.timer = Some(self.limits.restart_ms);
    }

    fn send(&mut self, code: Code, id: u8, data: Vec<u8>) {
        self.out.push(Message { code, id, data });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcp::Review;

    /// A protocol that agrees to everything and asks for one option, so the
    /// tests are about the engine rather than about LCP.
    #[derive(Debug, Default)]
    struct Agreeable {
        acked: usize,
        naked: usize,
        rejected: usize,
    }

    impl Protocol for Agreeable {
        fn number(&self) -> u16 {
            crate::protocol::LCP
        }
        fn request(&self) -> Vec<ConfigOption> {
            vec![ConfigOption { kind: 5, value: vec![1, 2, 3, 4] }]
        }
        fn review(&mut self, _: &[ConfigOption]) -> Review {
            Review::Ack
        }
        fn acked(&mut self, _: &[ConfigOption]) {
            self.acked += 1;
        }
        fn naked(&mut self, _: &[ConfigOption]) {
            self.naked += 1;
        }
        fn rejected(&mut self, _: &[ConfigOption]) {
            self.rejected += 1;
        }
    }

    fn started() -> Session<Agreeable> {
        let mut s = Session::new(Agreeable::default(), Limits::default());
        s.open();
        s.up();
        s
    }

    #[test]
    fn opening_sends_a_request() {
        let mut s = started();
        let out = s.take_output();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, Code::ConfigureRequest);
        assert_eq!(s.state(), State::ReqSent);
    }

    #[test]
    fn agreement_in_either_order_brings_the_layer_up() {
        for peer_first in [true, false] {
            let mut s = started();
            let id = s.take_output()[0].id;
            let ack = Message { code: Code::ConfigureAck, id, data: vec![5, 6, 1, 2, 3, 4] };
            let req = Message { code: Code::ConfigureRequest, id: 7, data: vec![] };
            if peer_first {
                s.receive(req);
                s.receive(ack);
            } else {
                s.receive(ack);
                s.receive(req);
            }
            assert_eq!(s.state(), State::Opened, "peer_first {peer_first}");
            assert!(s.take_reports().contains(&Report::Up));
            assert_eq!(s.protocol.acked, 1);
        }
    }

    /// 5.2: an acknowledgement of a request this end did not send is not one.
    #[test]
    fn an_ack_for_the_wrong_request_is_ignored() {
        let mut s = started();
        let id = s.take_output()[0].id;
        s.receive(Message { code: Code::ConfigureAck, id: id.wrapping_add(9), data: vec![] });
        assert_eq!(s.state(), State::ReqSent, "a stale ack moved the automaton");
        assert_eq!(s.protocol.acked, 0);
    }

    /// 4.6: the request is repeated, and eventually given up on.
    #[test]
    fn a_silent_peer_is_asked_ten_times_and_then_abandoned() {
        let limits = Limits { max_configure: 10, restart_ms: 3000, ..Limits::default() };
        let mut s = Session::new(Agreeable::default(), limits);
        s.open();
        s.up();
        let mut requests = s.take_output().len();
        for _ in 0..20 {
            s.tick(3000);
            requests += s
                .take_output()
                .iter()
                .filter(|m| m.code == Code::ConfigureRequest)
                .count();
            if s.state() == State::Stopped {
                break;
            }
        }
        assert_eq!(s.state(), State::Stopped, "never gave up");
        assert_eq!(requests, 10, "4.6 asks for the counter's worth of attempts");
        assert!(s.take_reports().contains(&Report::Finished));
    }

    /// The timer only runs where the table says it does.
    #[test]
    fn a_settled_link_has_no_timer_left_running() {
        let mut s = started();
        let id = s.take_output()[0].id;
        s.receive(Message { code: Code::ConfigureAck, id, data: vec![] });
        s.receive(Message { code: Code::ConfigureRequest, id: 1, data: vec![] });
        assert_eq!(s.state(), State::Opened);
        // Including the acknowledgement that answered the peer's request,
        // which is owed and not unprompted.
        let _ = s.take_output();
        // Any amount of time passing does nothing at all.
        for _ in 0..100 {
            s.tick(10_000);
        }
        assert_eq!(s.state(), State::Opened);
        assert!(s.take_output().is_empty(), "a settled link sent something unprompted");
    }

    /// 5.8: an Echo-Request is answered without disturbing anything.
    #[test]
    fn an_echo_is_answered_in_place() {
        let mut s = started();
        let id = s.take_output()[0].id;
        s.receive(Message { code: Code::ConfigureAck, id, data: vec![] });
        s.receive(Message { code: Code::ConfigureRequest, id: 1, data: vec![] });
        let _ = s.take_output();
        s.receive(Message {
            code: Code::EchoRequest,
            id: 0x42,
            data: vec![0xde, 0xad, 0xbe, 0xef, b'h', b'i'],
        });
        let out = s.take_output();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, Code::EchoReply);
        assert_eq!(out[0].id, 0x42);
        // The magic number is replaced by this end's; the rest comes back.
        assert_eq!(&out[0].data[4..], b"hi");
        assert_eq!(s.state(), State::Opened);
    }

    /// 5: a code with no meaning gets a Code-Reject and the link survives.
    #[test]
    fn an_unknown_code_is_rejected_by_number() {
        let mut s = started();
        let _ = s.take_output();
        s.receive(Message { code: Code::Unknown(0x5a), id: 3, data: vec![9, 9] });
        let out = s.take_output();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, Code::CodeReject);
        assert_eq!(out[0].data[0], 0x5a, "the rejected packet is copied in");
        assert_eq!(s.state(), State::ReqSent, "the link was dropped over it");
    }

    /// 5.5: a peer that wants to hang up is acknowledged.
    #[test]
    fn a_terminate_request_is_acknowledged_with_its_own_identifier() {
        let mut s = started();
        let _ = s.take_output();
        s.receive(Message { code: Code::TerminateRequest, id: 0x1f, data: vec![] });
        let out = s.take_output();
        assert_eq!(out[0].code, Code::TerminateAck);
        assert_eq!(out[0].id, 0x1f);
    }
}
