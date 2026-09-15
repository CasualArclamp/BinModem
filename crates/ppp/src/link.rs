//! One end of a PPP link, from octets on a modem to IP datagrams.
//!
//! RFC 1661 3.2 draws the phases this walks through: the link is dead until
//! something below carries octets, then LCP settles what the two ends can do,
//! then -- if either end asked for it -- authentication, then the network
//! protocols, one of which is IP.
//!
//! Authentication is RFC 1334's PAP or RFC 1994's CHAP, in [`crate::auth`],
//! and it runs in whichever directions were asked for: a provider asks the
//! caller who it is, and this end can be either. Two of these that nobody has
//! given an account to still go straight from LCP to addresses.

use crate::auth::{self, Account, Authenticator, Outcome, Prover};
use crate::control::{Code, Limits, Message, State};
use crate::frame::{Deframer, Framer, Packet};
use crate::ip;
use crate::ipcp::Ipcp;
use crate::lcp::{Auth, Lcp};
use crate::session::{Report, Session};
use crate::vj;

/// 3.2's phase diagram, as far as this goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 3.3: nothing below is carrying anything.
    Dead,
    /// 3.4: LCP is negotiating, or -- once it is up and anyone who had to has
    /// said who they are -- IPCP is.
    Establish,
    /// 3.5: one end is proving who it is to the other.
    Authenticate,
    /// 3.6: IP can flow.
    Network,
    /// 3.7: going away.
    Terminate,
}

/// Who this end is, and whom it lets in.
#[derive(Debug, Clone, Default)]
pub struct Authentication {
    /// What this end says when a far end asks who it is. Nothing set and a
    /// far end that asks is told an empty name, and refuses it, which is a
    /// failure with a reason rather than a link that hangs.
    pub account: Option<Account>,
    /// Whether this end insists a caller say who it is, and the accounts it
    /// will take. None asks nobody anything.
    pub callers: Option<Vec<Account>>,
    /// What this end calls itself, in a CHAP challenge.
    pub name: String,
    /// For CHAP's challenges, which must differ from call to call.
    pub seed: u64,
}

/// How many times round [`Link::round`] before giving up on it settling.
///
/// LCP coming up starts authentication or IPCP, each of which has something
/// to send, and the end of authentication starts IPCP. That is three rounds
/// and a fourth to find there is no more; the rest is slack.
const ROUNDS: usize = 6;

/// One end of the link.
#[derive(Debug)]
pub struct Link {
    framer: Framer,
    deframer: Deframer,
    lcp: Session<Lcp>,
    ipcp: Session<Ipcp>,
    authentication: Authentication,
    /// This end proving who it is, while it is.
    prover: Option<Prover>,
    /// And this end checking the far end.
    authenticator: Option<Authenticator>,
    /// The name the far end proved, once it has.
    who: Option<String>,
    phase: Phase,
    line: Vec<u8>,
    arrived: Vec<ip::Arrived>,
    /// Datagrams carrying something other than an echo, for whatever is above
    /// this to make sense of.
    carried: Vec<ip::Carried>,
    /// RFC 1144, once IPCP has agreed to it. One for each direction, because
    /// RFC 1332 4 negotiates each direction on its own: a link may compress
    /// one way and not the other, and between different implementations it
    /// often does.
    compressor: Option<vj::Compressor>,
    decompressor: Option<vj::Decompressor>,
    /// 791's Identification field, which only has to differ between datagrams
    /// that are alive at once.
    next_id: u16,
    /// Why the link is not up, once there is a reason worth telling a person.
    trouble: Option<String>,
    /// Whether this end asked for the link to close, so a close is not
    /// reported as the far end's doing.
    closing: bool,
    opened: bool,
    /// Whether LCP has ever come up on this link.
    was_up: bool,
}

impl Link {
    /// `local` is what this end will call itself and `remote` what it will
    /// offer the other; zeroes for either mean it is asking rather than
    /// telling (RFC 1332 3.3).
    pub fn new(local: [u8; 4], remote: [u8; 4]) -> Self {
        Self::with_authentication(local, remote, Authentication::default())
    }

    /// The same, with an account to prove who this end is and, if
    /// `callers` is set, a demand that the far end prove who it is.
    pub fn with_authentication(local: [u8; 4], remote: [u8; 4], authentication: Authentication) -> Self {
        // A modem call has a round trip measured in whole seconds once a VoIP
        // trunk is in it, so the Restart timer is the long end of what 4.6
        // suggests rather than the short.
        let limits = Limits { restart_ms: 3000, ..Limits::default() };
        let wanted = crate::lcp::Wanted::default();
        let lcp = match &authentication.callers {
            Some(accounts) => Lcp::demanding(wanted, auth::methods_for(accounts)),
            None => Lcp::new(wanted),
        };
        Self {
            framer: Framer::new(),
            deframer: Deframer::new(),
            lcp: Session::new(lcp, limits),
            ipcp: Session::new(Ipcp::new(local, remote), limits),
            authentication,
            prover: None,
            authenticator: None,
            who: None,
            phase: Phase::Dead,
            line: Vec::new(),
            arrived: Vec::new(),
            carried: Vec::new(),
            compressor: None,
            decompressor: None,
            next_id: 1,
            trouble: None,
            closing: false,
            opened: false,
            was_up: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether IP can be carried right now.
    pub fn up(&self) -> bool {
        self.phase == Phase::Network
    }

    /// The addresses the two ends settled on.
    pub fn addresses(&self) -> ([u8; 4], [u8; 4]) {
        (self.ipcp.protocol.local(), self.ipcp.protocol.remote())
    }

    /// Do not ask the far end for header compression.
    ///
    /// Nothing needs this on a real call. It is here because a test that wants
    /// to watch what a datagram does has to be able to stop the layer below
    /// rewriting it.
    pub fn without_header_compression(mut self) -> Self {
        self.ipcp.protocol = self.ipcp.protocol.without_header_compression();
        self
    }

    /// What header compression was agreed, each way round.
    pub fn header_compression(&self) -> crate::ipcp::Compression {
        self.ipcp.protocol.compression
    }

    /// Why the link is down or going, if there is a reason to give.
    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }

    /// The name the far end proved to this end, if it was asked for one.
    pub fn who(&self) -> Option<&str> {
        self.who.as_deref()
    }

    /// How the far end was asked to prove who it is, if it was.
    pub fn checked_with(&self) -> Option<Auth> {
        self.authenticator.as_ref().map(Authenticator::method)
    }

    /// And how this end was, if the far end asked.
    pub fn proved_with(&self) -> Option<Auth> {
        self.prover.as_ref().map(Prover::method)
    }

    /// Whether the link has been opened and has since given up: LCP is back
    /// where nothing more will happen unless the far end starts again.
    pub fn ended(&self) -> bool {
        self.opened
            && self.phase == Phase::Dead
            && matches!(self.lcp.state(), State::Closed | State::Stopped | State::Initial | State::Starting)
    }

    /// The modem has connected: there is something under this now.
    pub fn open(&mut self) {
        self.opened = true;
        self.phase = Phase::Establish;
        self.lcp.open();
        self.lcp.up();
        self.pump();
    }

    /// And has hung up.
    pub fn close(&mut self) {
        self.closing = true;
        // 3.7 closes the network protocols before the link under them.
        self.ipcp.close();
        self.lcp.close();
        self.phase = Phase::Terminate;
        self.pump();
    }

    /// Octets from the modem.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            // A frame that did not survive the line is counted where it was
            // counted, and told to the decompressor: RFC 1144 4.1 has it
            // rebuild each header from the one before, so after a gap it must
            // throw packets away until one names its connection again.
            // Otherwise the changes in the next packet are applied to
            // whichever conversation went last, and the TCP checksum has one
            // chance in 65536 of not noticing. Every protocol here has its own
            // timer and will ask again.
            let read = self.deframer.feed(byte);
            if read.is_err()
                && let Some(d) = self.decompressor.as_mut()
            {
                d.error();
            }
            if let Ok(Some(packet)) = read {
                self.deliver(packet);
                // Round by round rather than once at the end: what one frame
                // agreed to governs how the next one is read, and the next one
                // may be in this same buffer. A Configure-Ack and the first
                // frame sent under what it agreed arrive together often
                // enough that reading them under the same settings deadlocks
                // the link.
                self.pump();
            }
        }
        self.pump();
    }

    /// Time passing, for the Restart timers.
    pub fn tick(&mut self, ms: u32) {
        self.lcp.tick(ms);
        if let Some(prover) = self.prover.as_mut() {
            prover.tick(ms);
        }
        if let Some(authenticator) = self.authenticator.as_mut() {
            authenticator.tick(ms);
        }
        if self.phase == Phase::Network || self.ipcp.state() != State::Initial {
            self.ipcp.tick(ms);
        }
        self.pump();
    }

    /// Octets for the modem.
    pub fn take_line(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.line)
    }

    /// Echoes that have arrived.
    pub fn take_arrived(&mut self) -> Vec<ip::Arrived> {
        std::mem::take(&mut self.arrived)
    }

    /// And everything else that has: whatever is above this deals with it.
    pub fn take_carried(&mut self) -> Vec<ip::Carried> {
        std::mem::take(&mut self.carried)
    }

    /// Put a payload of some other protocol on the link, addressed from this
    /// end to the other.
    ///
    /// Does nothing before the network phase, for the reason 3.6 gives: there
    /// is nowhere to send it and no address to send it from.
    pub fn send_payload(&mut self, protocol: u8, payload: &[u8]) -> bool {
        if !self.up() {
            return false;
        }
        let (local, remote) = self.addresses();
        let datagram = ip::build(local, remote, protocol, payload, self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.send_datagram(datagram);
        true
    }

    /// Send one echo request to the far end.
    ///
    /// Does nothing before the network phase, because there is nowhere to send
    /// it and no address to send it from.
    pub fn ping(&mut self, id: u16, sequence: u16, payload: &[u8]) -> bool {
        if !self.up() {
            return false;
        }
        let (local, remote) = self.addresses();
        let echo = ip::Echo {
            reply: false,
            id,
            sequence,
            payload: payload.to_vec(),
        };
        let datagram = ip::datagram(local, remote, &echo, self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.send_datagram(datagram);
        true
    }

    /// Put one datagram on the link, through the compressor if there is one.
    ///
    /// Everything that sends IP goes through here, including a ping: RFC 1144
    /// 3.2.3 sends anything that is not compressible TCP as it stands and
    /// leaves the compressor's state alone, so there is nothing to decide
    /// above this and nothing gained by deciding it.
    fn send_datagram(&mut self, datagram: Vec<u8>) {
        match self.compressor.as_mut() {
            Some(c) => {
                let (kind, packet) = c.compress(&datagram);
                self.send(kind.protocol(), packet);
            }
            None => self.send(crate::protocol::IP, datagram),
        }
    }

    fn deliver(&mut self, packet: Packet) {
        match packet.protocol {
            crate::protocol::LCP => {
                if let Some(message) = Message::parse(&packet.payload) {
                    if message.code == Code::ProtocolReject {
                        self.protocol_rejected(&message.data);
                    }
                    self.lcp.receive(message);
                }
            }
            crate::protocol::IPCP => {
                // 3.5: during authentication "all other packets received
                // during this phase MUST be silently discarded", and IPCP has
                // not been opened before then, so its session drops them
                // itself.
                if let Some(message) = Message::parse(&packet.payload) {
                    self.ipcp.receive(message);
                }
            }
            crate::protocol::PAP | crate::protocol::CHAP => {
                // Each packet is for whichever end of the exchange it is
                // addressed to. The codes say which: a request or a response
                // is for the end checking, and everything else for the end
                // being checked. A packet for an exchange that is not running
                // is dropped, as 1334 2.2.1 and 1994 4.1 both say.
                let code = packet.payload.first().copied().unwrap_or(0);
                let for_checker = match packet.protocol {
                    crate::protocol::PAP => code == auth::pap::AUTHENTICATE_REQUEST,
                    _ => code == auth::chap::RESPONSE,
                };
                if for_checker {
                    if let Some(a) = self.authenticator.as_mut()
                        && auth::protocol(a.method()) == packet.protocol
                    {
                        a.receive(&packet.payload);
                    }
                } else if let Some(p) = self.prover.as_mut()
                    && auth::protocol(p.method()) == packet.protocol
                {
                    p.receive(&packet.payload);
                }
            }
            crate::protocol::IP
            | crate::protocol::COMPRESSED_TCP
            | crate::protocol::UNCOMPRESSED_TCP => {
                // 3.6: "IP packets received before this phase is reached
                // SHOULD be silently discarded", and one arriving after it
                // that is not an echo is not this layer's business either.
                if !self.up() {
                    return;
                }
                let Some(kind) = vj::Kind::from_protocol(packet.protocol) else {
                    return;
                };
                let datagram = match (kind, self.decompressor.as_mut()) {
                    (vj::Kind::Ip, _) => packet.payload,
                    // A far end sending these without having been told this
                    // end can read them. Nothing can be done with it, and
                    // 5.7's Protocol-Reject is for a protocol number this end
                    // does not run at all rather than one it did not agree to,
                    // so it is dropped.
                    (_, None) => return,
                    (kind, Some(d)) => match d.decompress(kind, &packet.payload) {
                        Some(datagram) => datagram,
                        None => return,
                    },
                };
                let Some(carried) = ip::read(&datagram) else {
                    return;
                };
                if carried.protocol != ip::PROTOCOL_ICMP {
                    // Somebody else's business. TCP is the only thing that
                    // asks for it so far.
                    self.carried.push(carried);
                    return;
                }
                if let Some(echo) = ip::Echo::parse(&carried.payload) {
                    let arrived = ip::Arrived {
                        from: carried.from,
                        to: carried.to,
                        echo,
                    };
                    if !arrived.echo.reply {
                        // RFC 792 makes answering an echo the receiver's job,
                        // and doing it here rather than above keeps a ping
                        // working before there is anything above.
                        let reply = arrived.echo.to_reply();
                        let datagram =
                            ip::datagram(arrived.to, arrived.from, &reply, self.next_id);
                        self.next_id = self.next_id.wrapping_add(1);
                        self.send_datagram(datagram);
                    }
                    self.arrived.push(arrived);
                }
            }
            // 5.7: "Upon reception of a packet with an unknown Protocol field,
            // the implementation MUST transmit a Protocol-Reject." A far end
            // running pppd offers compression and IPv6 as a matter of course,
            // and without this it asks for them every three seconds for half a
            // minute.
            other => self.lcp.reject_protocol(other, &packet.payload),
        }
    }

    /// The far end has refused one of this end's protocols.
    fn protocol_rejected(&mut self, data: &[u8]) {
        let [hi, lo, ..] = *data else { return };
        match u16::from_be_bytes([hi, lo]) {
            crate::protocol::IPCP | crate::protocol::IP => {
                self.ipcp.refused();
                self.note("the far end does not carry IP");
            }
            // An authentication protocol refused is authentication failed:
            // the exchange cannot finish, and 3.5 has nowhere else to go.
            crate::protocol::PAP | crate::protocol::CHAP => {
                self.note("the far end refused the authentication protocol it had agreed to");
                self.close_link();
            }
            _ => {}
        }
    }

    /// Keep the first reason: it is the cause, and what follows is the effect.
    fn note(&mut self, why: &str) {
        if self.trouble.is_none() {
            self.trouble = Some(why.to_owned());
        }
    }

    /// End the link for a reason of this end's own, as 3.5 has an
    /// authenticator do when a caller fails: "the authenticator SHOULD proceed
    /// instead to the Link Termination phase."
    fn close_link(&mut self) {
        self.prover = None;
        self.authenticator = None;
        self.ipcp.close();
        self.lcp.close();
        self.phase = Phase::Terminate;
    }

    /// LCP is up: 3.5 if anyone asked for it, 3.6 if not.
    fn begin_authentication(&mut self) {
        // What the far end demands of this end is in what this end agreed to.
        let theirs = self.lcp.protocol.agreed.auth;
        // What this end demands of the far end is what it asked for -- never
        // what came back in the acknowledgement, which a far end could have
        // trimmed. A demand is not something the other side gets a say in.
        let ours = self.authentication.callers.as_ref().and(self.lcp.protocol.wanted.auth);
        if self.authentication.callers.is_some() && ours.is_none() {
            self.note("this end would have asked the caller who it is, and did not");
            self.close_link();
            return;
        }
        if let Some(method) = theirs {
            let account = self.authentication.account.clone().unwrap_or_default();
            self.prover = Some(Prover::new(method, account));
        }
        if let (Some(method), Some(accounts)) = (ours, self.authentication.callers.clone()) {
            let name = if self.authentication.name.is_empty() { "binmodem" } else { &self.authentication.name };
            self.authenticator = Some(Authenticator::new(method, accounts, name, self.authentication.seed));
        }
        if self.prover.is_some() || self.authenticator.is_some() {
            self.phase = Phase::Authenticate;
        } else {
            self.begin_network();
        }
    }

    fn begin_network(&mut self) {
        self.phase = Phase::Establish;
        self.ipcp.open();
        self.ipcp.up();
    }

    /// Where authentication has got to, and what follows from it.
    fn follow_authentication(&mut self) {
        if self.phase != Phase::Authenticate {
            return;
        }
        let proving = self.prover.as_ref().map(|p| p.outcome().clone());
        let checking = self.authenticator.as_ref().map(|a| a.outcome().clone());
        for outcome in [&proving, &checking].into_iter().flatten() {
            if let Outcome::Failed(why) = outcome {
                let why = why.clone();
                self.note(&why);
                // One more round first, so the refusal itself goes out ahead
                // of the Terminate-Request: a caller told why is better off
                // than one that is just hung up on.
                self.flush_authentication();
                self.close_link();
                return;
            }
        }
        let done = |o: &Option<Outcome>| o.as_ref().is_none_or(|o| *o == Outcome::Passed);
        if done(&proving) && done(&checking) {
            if let Some(a) = &self.authenticator {
                self.who = a.who().map(str::to_owned);
            }
            // Both are kept, not dropped: 1334 and 1994 both require a
            // repeated request or response after success to be answered the
            // same way, and CHAP's challenger may ask again at any time.
            self.begin_network();
        }
    }

    fn flush_authentication(&mut self) {
        let mut out = Vec::new();
        if let Some(p) = self.prover.as_mut() {
            let protocol = auth::protocol(p.method());
            out.extend(p.take_output().into_iter().map(|b| (protocol, b)));
        }
        if let Some(a) = self.authenticator.as_mut() {
            let protocol = auth::protocol(a.method());
            out.extend(a.take_output().into_iter().map(|b| (protocol, b)));
        }
        for (protocol, bytes) in out {
            self.send(protocol, bytes);
        }
    }

    /// Move whatever the sessions have produced onto the line, and follow the
    /// phase they put the link in.
    ///
    /// Output before reports, and that order is the whole of it. What a
    /// session produced, it produced under the settings that were in force
    /// when it produced it: the Configure-Ack that brings this end up is the
    /// last frame the far end will read while it is still down, and framing it
    /// under what it agreed to would put octets on the line the far end is
    /// still entitled to strip.
    ///
    /// It goes round more than once because a report starts the next protocol,
    /// which has something to say immediately -- and that, correctly, is sent
    /// under the new settings.
    fn pump(&mut self) {
        for _ in 0..ROUNDS {
            if !self.round() {
                break;
            }
        }
    }

    /// One round of it. Says whether anything happened.
    fn round(&mut self) -> bool {
        let lcp: Vec<_> = self.lcp.take_output();
        let ipcp: Vec<_> = self.ipcp.take_output();
        let mut anything = !lcp.is_empty() || !ipcp.is_empty();
        for message in lcp {
            self.send(crate::protocol::LCP, message.to_bytes());
        }
        let before = self.line.len();
        self.flush_authentication();
        anything |= self.line.len() != before;
        for message in ipcp {
            self.send(crate::protocol::IPCP, message.to_bytes());
        }

        let phase = self.phase;
        for report in self.lcp.take_reports() {
            anything = true;
            match report {
                Report::Up => {
                    self.was_up = true;
                    // 3.4: what LCP agreed takes effect now, and the framer is
                    // where most of it lands.
                    let agreed = self.lcp.protocol.agreed;
                    self.framer.set_accm(agreed.accm);
                    self.framer.set_compression(agreed.acfc, agreed.pfc);
                    self.deframer.set_accm(self.lcp.protocol.wanted.accm);
                    self.begin_authentication();
                }
                Report::Down | Report::Finished => {
                    // 3.4: "All Configuration Options are assumed to be at
                    // default values unless altered by the configuration
                    // exchange", and a link that has gone down has no
                    // exchange in force.
                    self.framer = Framer::new();
                    self.deframer.set_accm(crate::frame::DEFAULT_ACCM);
                    self.compressor = None;
                    self.decompressor = None;
                    self.ipcp.down();
                    self.prover = None;
                    self.authenticator = None;
                    if report == Report::Finished && !self.closing {
                        let why = if self.was_up {
                            "the far end ended the link".to_owned()
                        } else if let Some(value) = &self.lcp.protocol.unknown_auth {
                            format!(
                                "the far end wants {} to say who is calling, which this end does not do",
                                crate::lcp::describe_auth(value)
                            )
                        } else {
                            "the far end never agreed to a link".to_owned()
                        };
                        self.note(&why);
                    }
                    self.phase = Phase::Dead;
                }
                Report::Started => {}
            }
        }
        if let Some(why) = self.lcp.protocol.refused.take() {
            anything = true;
            self.note(&why);
            self.close_link();
        }
        self.follow_authentication();
        for report in self.ipcp.take_reports() {
            anything = true;
            match report {
                Report::Up => {
                    self.phase = Phase::Network;
                    // Built here rather than at negotiation, so a slot table
                    // never outlives the agreement that sized it.
                    let agreed = self.ipcp.protocol.compression;
                    self.compressor = agreed.sending.map(vj::Compressor::new);
                    self.decompressor = agreed.receiving.map(vj::Decompressor::new);
                }
                Report::Down | Report::Finished => {
                    self.compressor = None;
                    self.decompressor = None;
                    if self.phase == Phase::Network {
                        self.phase = Phase::Establish;
                    }
                }
                Report::Started => {}
            }
        }
        anything || self.phase != phase
    }

    fn send(&mut self, protocol: u16, payload: Vec<u8>) {
        // 5: "Regardless of which Configuration Options are enabled, all LCP
        // Link Configuration, Link Termination, and Code-Reject packets (codes
        // 1 through 7) are always sent as if no Configuration Options were
        // negotiated." Which is what lets a far end that has already gone back
        // to its defaults read a Terminate-Request, or a new Configure-Request
        // for a link it thinks is open, at all.
        let unconfigured = protocol == crate::protocol::LCP && matches!(payload.first(), Some(1..=7));
        let packet = Packet { protocol, payload };
        if unconfigured {
            Framer::new().frame(&packet, &mut self.line);
        } else {
            self.framer.frame(&packet, &mut self.line);
        }
    }
}
