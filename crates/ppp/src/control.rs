//! The option negotiation automaton, and the packets it negotiates with
//! (RFC 1661 sections 4 and 5).
//!
//! Not LCP. RFC 1661 defines this once and every control protocol runs it:
//! LCP settles what the link can do, IPCP settles addresses, and both are the
//! same ten states and sixteen events with different options inside the
//! packets. So it is written once here and the protocols on top supply only
//! what their options mean.
//!
//! The state table below is the document's, in the document's own notation, so
//! that it can be read against page 12 line by line rather than trusted. That
//! is deliberate: a transcription error in a table this size is invisible in
//! ordinary code and produces a link that almost works.

/// 5: what kind of packet this is.
///
/// "When a packet is received with an unknown Code field, a Code-Reject packet
/// is transmitted", which is why this keeps the number it did not recognise
/// rather than refusing to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    ConfigureRequest,
    ConfigureAck,
    ConfigureNak,
    ConfigureReject,
    TerminateRequest,
    TerminateAck,
    CodeReject,
    ProtocolReject,
    EchoRequest,
    EchoReply,
    DiscardRequest,
    /// Something this implementation has no name for, kept so it can be
    /// rejected by number.
    Unknown(u8),
}

impl Code {
    pub fn from_u8(code: u8) -> Self {
        match code {
            1 => Self::ConfigureRequest,
            2 => Self::ConfigureAck,
            3 => Self::ConfigureNak,
            4 => Self::ConfigureReject,
            5 => Self::TerminateRequest,
            6 => Self::TerminateAck,
            7 => Self::CodeReject,
            8 => Self::ProtocolReject,
            9 => Self::EchoRequest,
            10 => Self::EchoReply,
            11 => Self::DiscardRequest,
            other => Self::Unknown(other),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::ConfigureRequest => 1,
            Self::ConfigureAck => 2,
            Self::ConfigureNak => 3,
            Self::ConfigureReject => 4,
            Self::TerminateRequest => 5,
            Self::TerminateAck => 6,
            Self::CodeReject => 7,
            Self::ProtocolReject => 8,
            Self::EchoRequest => 9,
            Self::EchoReply => 10,
            Self::DiscardRequest => 11,
            Self::Unknown(other) => other,
        }
    }
}

/// One control protocol packet: 5's Code, Identifier, Length and Data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub code: Code,
    /// 5: "aids in matching requests and replies".
    pub id: u8,
    pub data: Vec<u8>,
}

impl Message {
    /// Read one from the information field of a PPP frame.
    ///
    /// Returns nothing for the two cases 5 calls silently discardable: a
    /// packet too short to hold a header, and a Length field that does not
    /// describe the packet it is in. "Octets outside the range of the Length
    /// field are treated as padding and are ignored on reception", so a longer
    /// buffer is not an error -- something below may pad.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
        if length < 4 || length > bytes.len() {
            return None;
        }
        Some(Self {
            code: Code::from_u8(bytes[0]),
            id: bytes[1],
            data: bytes[4..length].to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let length = (self.data.len() + 4) as u16;
        let mut out = Vec::with_capacity(length as usize);
        out.push(self.code.to_u8());
        out.push(self.id);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&self.data);
        out
    }
}

/// One configuration option: 5.1's Type, Length and Data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOption {
    pub kind: u8,
    pub value: Vec<u8>,
}

impl ConfigOption {
    /// Read a whole list of them, as a Configure-Request carries.
    ///
    /// All or nothing. An option whose length runs past the end of the packet
    /// makes the rest unreadable -- there is no way to find where the next one
    /// starts -- so the list is refused rather than half-believed.
    pub fn parse_all(mut bytes: &[u8]) -> Option<Vec<Self>> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            if bytes.len() < 2 {
                return None;
            }
            let length = usize::from(bytes[1]);
            // 5.1: the Length includes the Type and Length octets, so two is
            // the shortest an option can be and anything less is malformed.
            if length < 2 || length > bytes.len() {
                return None;
            }
            out.push(Self { kind: bytes[0], value: bytes[2..length].to_vec() });
            bytes = &bytes[length..];
        }
        Some(out)
    }

    pub fn write_all(options: &[Self], out: &mut Vec<u8>) {
        for option in options {
            out.push(option.kind);
            out.push((option.value.len() + 2) as u8);
            out.extend_from_slice(&option.value);
        }
    }
}

/// 4.2: where the automaton is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Initial = 0,
    Starting = 1,
    Closed = 2,
    Stopped = 3,
    Closing = 4,
    Stopping = 5,
    ReqSent = 6,
    AckRcvd = 7,
    AckSent = 8,
    Opened = 9,
}

impl State {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Initial,
            1 => Self::Starting,
            2 => Self::Closed,
            3 => Self::Stopped,
            4 => Self::Closing,
            5 => Self::Stopping,
            6 => Self::ReqSent,
            7 => Self::AckRcvd,
            8 => Self::AckSent,
            _ => Self::Opened,
        }
    }
}

/// 4.3: what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The lower layer is ready to carry frames.
    Up,
    /// And is not any more.
    Down,
    /// This end wants the link.
    Open,
    /// And does not.
    Close,
    /// The restart timer expired with attempts left.
    TimeoutWithRestarts,
    /// And without.
    TimeoutNoRestarts,
    /// A Configure-Request this end can agree to in full.
    ReceiveConfigureRequestGood,
    /// One it cannot.
    ReceiveConfigureRequestBad,
    ReceiveConfigureAck,
    /// A Configure-Nak or a Configure-Reject, which 4.3 treats alike here.
    ReceiveConfigureNak,
    ReceiveTerminateRequest,
    ReceiveTerminateAck,
    /// A code this implementation does not know.
    ReceiveUnknownCode,
    /// A Code-Reject or Protocol-Reject for something not essential.
    ReceiveRejectRecoverable,
    /// Or for something that is.
    ReceiveRejectFatal,
    /// An Echo-Request, Echo-Reply or Discard-Request.
    ReceiveEcho,
}

impl Event {
    fn row(self) -> usize {
        match self {
            Self::Up => 0,
            Self::Down => 1,
            Self::Open => 2,
            Self::Close => 3,
            Self::TimeoutWithRestarts => 4,
            Self::TimeoutNoRestarts => 5,
            Self::ReceiveConfigureRequestGood => 6,
            Self::ReceiveConfigureRequestBad => 7,
            Self::ReceiveConfigureAck => 8,
            Self::ReceiveConfigureNak => 9,
            Self::ReceiveTerminateRequest => 10,
            Self::ReceiveTerminateAck => 11,
            Self::ReceiveUnknownCode => 12,
            Self::ReceiveRejectRecoverable => 13,
            Self::ReceiveRejectFatal => 14,
            Self::ReceiveEcho => 15,
        }
    }
}

/// 4.4: what to do about it.
///
/// Named as the document names them, because the table is written in those
/// names and a reader checking one against the other should not have to
/// translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// tlu: This-Layer-Up. The link is usable.
    ThisLayerUp,
    /// tld: This-Layer-Down.
    ThisLayerDown,
    /// tls: This-Layer-Started.
    ThisLayerStarted,
    /// tlf: This-Layer-Finished.
    ThisLayerFinished,
    /// irc: Initialize-Restart-Count.
    InitializeRestartCount,
    /// zrc: Zero-Restart-Count.
    ZeroRestartCount,
    /// scr: Send-Configure-Request.
    SendConfigureRequest,
    /// sca: Send-Configure-Ack.
    SendConfigureAck,
    /// scn: Send-Configure-Nak or Send-Configure-Reject.
    SendConfigureNak,
    /// str: Send-Terminate-Request.
    SendTerminateRequest,
    /// sta: Send-Terminate-Ack.
    SendTerminateAck,
    /// scj: Send-Code-Reject.
    SendCodeReject,
    /// ser: Send-Echo-Reply.
    SendEchoReply,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "tlu" => Self::ThisLayerUp,
            "tld" => Self::ThisLayerDown,
            "tls" => Self::ThisLayerStarted,
            "tlf" => Self::ThisLayerFinished,
            "irc" => Self::InitializeRestartCount,
            "zrc" => Self::ZeroRestartCount,
            "scr" => Self::SendConfigureRequest,
            "sca" => Self::SendConfigureAck,
            "scn" => Self::SendConfigureNak,
            "str" => Self::SendTerminateRequest,
            "sta" => Self::SendTerminateAck,
            "scj" => Self::SendCodeReject,
            "ser" => Self::SendEchoReply,
            _ => return None,
        })
    }
}

/// 4.1's state transition table, exactly as it is printed.
///
/// Rows are the events in the order 4.1 lists them and columns the ten states.
/// Each cell is `action,action/state`, a bare state, or `-` for a transition
/// the document calls illegal. A trailing letter on a state is one of 4.1's
/// footnotes -- p for the passive option, r for restart, x for the crossed
/// connection -- and does not change where the automaton goes.
///
/// Kept as text on purpose. Sixteen by ten is a hundred and sixty cells, and
/// the only way to be sure of them is to be able to lay this beside page 12.
const TABLE: [[&str; 10]; 16] = [
    // Up
    ["2", "irc,scr/6", "-", "-", "-", "-", "-", "-", "-", "-"],
    // Down
    ["-", "-", "0", "tls/1", "0", "1", "1", "1", "1", "tld/1"],
    // Open
    ["tls/1", "1", "irc,scr/6", "3r", "5r", "5r", "6", "7", "8", "9r"],
    // Close
    ["0", "tlf/0", "2", "2", "4", "4", "irc,str/4", "irc,str/4", "irc,str/4", "tld,irc,str/4"],
    // TO+
    ["-", "-", "-", "-", "str/4", "str/5", "scr/6", "scr/6", "scr/8", "-"],
    // TO-
    ["-", "-", "-", "-", "tlf/2", "tlf/3", "tlf/3p", "tlf/3p", "tlf/3p", "-"],
    // RCR+
    ["-", "-", "sta/2", "irc,scr,sca/8", "4", "5", "sca/8", "sca,tlu/9", "sca/8", "tld,scr,sca/8"],
    // RCR-
    ["-", "-", "sta/2", "irc,scr,scn/6", "4", "5", "scn/6", "scn/7", "scn/6", "tld,scr,scn/6"],
    // RCA
    ["-", "-", "sta/2", "sta/3", "4", "5", "irc/7", "scr/6x", "irc,tlu/9", "tld,scr/6x"],
    // RCN
    ["-", "-", "sta/2", "sta/3", "4", "5", "irc,scr/6", "scr/6x", "irc,scr/8", "tld,scr/6x"],
    // RTR
    ["-", "-", "sta/2", "sta/3", "sta/4", "sta/5", "sta/6", "sta/6", "sta/6", "tld,zrc,sta/5"],
    // RTA
    ["-", "-", "2", "3", "tlf/2", "tlf/3", "6", "6", "8", "tld,scr/6"],
    // RUC
    ["-", "-", "scj/2", "scj/3", "scj/4", "scj/5", "scj/6", "scj/7", "scj/8", "scj/9"],
    // RXJ+
    ["-", "-", "2", "3", "4", "5", "6", "6", "8", "9"],
    // RXJ-
    ["-", "-", "tlf/2", "tlf/3", "tlf/2", "tlf/3", "tlf/3", "tlf/3", "tlf/3", "tld,irc,str/5"],
    // RXR
    ["-", "-", "2", "3", "4", "5", "6", "7", "8", "ser/9"],
];

/// What one event does: the actions to take, in order, and where to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub actions: Vec<Action>,
    pub next: State,
}

/// Read one cell of 4.1's table.
///
/// `None` for the dash, which the document calls an illegal transition: 4.1
/// says those "indicate an event that cannot occur", and an implementation
/// that reaches one has a fault of its own rather than a peer misbehaving.
pub fn transition(state: State, event: Event) -> Option<Transition> {
    let cell = TABLE[event.row()][state as usize];
    if cell == "-" {
        return None;
    }
    let (actions, target) = match cell.rsplit_once('/') {
        Some((actions, target)) => (actions, target),
        // A bare number: stay put or move, doing nothing.
        None => ("", cell),
    };
    // Strip 4.1's footnote letters, which say why rather than where.
    let digits: String = target.chars().filter(char::is_ascii_digit).collect();
    let next = State::from_index(digits.parse::<usize>().ok()?);
    let actions = actions
        .split(',')
        .filter(|a| !a.is_empty())
        .map(|a| Action::parse(a).expect("the table names an action that is not one"))
        .collect();
    Some(Transition { actions, next })
}

/// 4.6: how many times a request is repeated before giving up, and how long
/// between them.
///
/// "A suggested maximum value is 10", and three seconds is the Restart timer's
/// suggested default. Both are made to be changed: a satellite link wants
/// longer and a local one wants shorter, and a modem call at 9600 with a
/// second of round trip in it wants the longer end of both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_configure: u32,
    pub max_terminate: u32,
    pub restart_ms: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_configure: 10, max_terminate: 2, restart_ms: 3000 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [State; 10] = [
        State::Initial,
        State::Starting,
        State::Closed,
        State::Stopped,
        State::Closing,
        State::Stopping,
        State::ReqSent,
        State::AckRcvd,
        State::AckSent,
        State::Opened,
    ];
    const EVENTS: [Event; 16] = [
        Event::Up,
        Event::Down,
        Event::Open,
        Event::Close,
        Event::TimeoutWithRestarts,
        Event::TimeoutNoRestarts,
        Event::ReceiveConfigureRequestGood,
        Event::ReceiveConfigureRequestBad,
        Event::ReceiveConfigureAck,
        Event::ReceiveConfigureNak,
        Event::ReceiveTerminateRequest,
        Event::ReceiveTerminateAck,
        Event::ReceiveUnknownCode,
        Event::ReceiveRejectRecoverable,
        Event::ReceiveRejectFatal,
        Event::ReceiveEcho,
    ];

    /// Every cell of the table reads, and every action in it is one.
    #[test]
    fn all_one_hundred_and_sixty_cells_parse() {
        for state in STATES {
            for event in EVENTS {
                // The call itself panics on an action name that is not one.
                let _ = transition(state, event);
            }
        }
    }

    /// 4.1: "The states in which the Restart timer is running are identifiable
    /// by the presence of TO events."
    ///
    /// Which is a claim about the table, and one worth checking: the four
    /// states with a timeout are exactly the four that are waiting for the
    /// peer to answer something.
    #[test]
    fn the_timer_runs_exactly_where_the_timeouts_are() {
        let running: Vec<State> = STATES
            .into_iter()
            .filter(|&s| transition(s, Event::TimeoutWithRestarts).is_some())
            .collect();
        assert_eq!(
            running,
            vec![
                State::Closing,
                State::Stopping,
                State::ReqSent,
                State::AckRcvd,
                State::AckSent
            ]
        );
        // And Opened is not among them: nothing there is waiting for a reply.
        assert!(transition(State::Opened, Event::TimeoutWithRestarts).is_none());
    }

    /// The four states before the link exists cannot receive anything, because
    /// there is nothing under them to receive it on.
    #[test]
    fn nothing_arrives_before_the_lower_layer_is_up() {
        for event in EVENTS {
            let arriving = !matches!(
                event,
                Event::Up | Event::Down | Event::Open | Event::Close
            );
            if arriving {
                assert!(
                    transition(State::Initial, event).is_none(),
                    "{event:?} is legal in Initial"
                );
                assert!(
                    transition(State::Starting, event).is_none(),
                    "{event:?} is legal in Starting"
                );
            }
        }
    }

    /// The path a call that works takes, read straight off the table.
    #[test]
    fn an_ordinary_negotiation_walks_from_initial_to_opened() {
        // Something wants the link before the modem has connected.
        let mut state = State::Initial;
        let t = transition(state, Event::Open).unwrap();
        assert_eq!(t.actions, vec![Action::ThisLayerStarted]);
        state = t.next;
        assert_eq!(state, State::Starting);

        // The modem connects.
        let t = transition(state, Event::Up).unwrap();
        assert_eq!(
            t.actions,
            vec![Action::InitializeRestartCount, Action::SendConfigureRequest]
        );
        state = t.next;
        assert_eq!(state, State::ReqSent);

        // The far end asks for things this end can live with.
        let t = transition(state, Event::ReceiveConfigureRequestGood).unwrap();
        assert_eq!(t.actions, vec![Action::SendConfigureAck]);
        state = t.next;
        assert_eq!(state, State::AckSent);

        // And agrees to ours.
        let t = transition(state, Event::ReceiveConfigureAck).unwrap();
        assert_eq!(
            t.actions,
            vec![Action::InitializeRestartCount, Action::ThisLayerUp]
        );
        state = t.next;
        assert_eq!(state, State::Opened);
    }

    /// The same two events in the other order, which is just as ordinary: two
    /// ends both send first and neither is wrong.
    #[test]
    fn the_other_order_gets_there_too() {
        let mut state = State::ReqSent;
        state = transition(state, Event::ReceiveConfigureAck).unwrap().next;
        assert_eq!(state, State::AckRcvd);
        let t = transition(state, Event::ReceiveConfigureRequestGood).unwrap();
        assert_eq!(t.actions, vec![Action::SendConfigureAck, Action::ThisLayerUp]);
        assert_eq!(t.next, State::Opened);
    }

    /// A packet is its own length, and a shorter buffer is not one.
    #[test]
    fn a_message_reads_back_as_it_was_written() {
        let m = Message {
            code: Code::ConfigureRequest,
            id: 0x2b,
            data: vec![0x01, 0x04, 0x05, 0xdc],
        };
        let bytes = m.to_bytes();
        assert_eq!(bytes[..4], [1, 0x2b, 0x00, 0x08]);
        assert_eq!(Message::parse(&bytes), Some(m));
    }

    /// 5: "Octets outside the range of the Length field are treated as padding
    /// and are ignored on reception."
    #[test]
    fn padding_past_the_length_is_not_part_of_the_packet() {
        let mut bytes = Message {
            code: Code::EchoRequest,
            id: 1,
            data: vec![0xde, 0xad],
        }
        .to_bytes();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let got = Message::parse(&bytes).unwrap();
        assert_eq!(got.data, vec![0xde, 0xad]);
    }

    /// 5: "When a packet is received with an invalid Length field, the packet
    /// is silently discarded."
    #[test]
    fn a_length_that_lies_is_refused() {
        assert_eq!(Message::parse(&[1, 1, 0xff, 0xff, 0, 0]), None);
        assert_eq!(Message::parse(&[1, 1, 0, 3]), None, "shorter than a header");
        assert_eq!(Message::parse(&[1, 1, 0]), None, "not even a header");
    }

    #[test]
    fn options_read_back_as_they_were_written() {
        let options = vec![
            ConfigOption { kind: 1, value: vec![0x05, 0xdc] },
            ConfigOption { kind: 5, value: vec![0xde, 0xad, 0xbe, 0xef] },
            ConfigOption { kind: 7, value: vec![] },
        ];
        let mut bytes = Vec::new();
        ConfigOption::write_all(&options, &mut bytes);
        assert_eq!(bytes, [1, 4, 0x05, 0xdc, 5, 6, 0xde, 0xad, 0xbe, 0xef, 7, 2]);
        assert_eq!(ConfigOption::parse_all(&bytes), Some(options));
    }

    /// An option whose length runs past the end leaves no way to find the next
    /// one, so the list is refused rather than half-read.
    #[test]
    fn an_option_that_overruns_refuses_the_whole_list() {
        assert_eq!(ConfigOption::parse_all(&[1, 4, 0x05]), None);
        assert_eq!(ConfigOption::parse_all(&[1, 1]), None, "shorter than its header");
        assert_eq!(ConfigOption::parse_all(&[1]), None);
        assert_eq!(ConfigOption::parse_all(&[]), Some(vec![]));
    }

    /// 5: an unknown code is kept by number so it can be rejected as one.
    #[test]
    fn an_unknown_code_survives_the_round_trip() {
        assert_eq!(Code::from_u8(200), Code::Unknown(200));
        assert_eq!(Code::Unknown(200).to_u8(), 200);
        for n in 1..=11u8 {
            assert_eq!(Code::from_u8(n).to_u8(), n);
        }
    }
}
