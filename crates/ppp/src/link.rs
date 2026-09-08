//! One end of a PPP link, from octets on a modem to IP datagrams.
//!
//! RFC 1661 3.2 draws the phases this walks through: the link is dead until
//! something below carries octets, then LCP settles what the two ends can do,
//! then -- if anyone asked for it -- authentication, then the network
//! protocols, one of which is IP.
//!
//! Authentication is the gap. Neither end here demands it, so two of these
//! reach the network phase directly; a far end that does demand it will get as
//! far as agreeing to LCP and no further, which is at least a failure with a
//! name on it.

use crate::control::Limits;
use crate::frame::{Deframer, Framer, Packet};
use crate::ip;
use crate::ipcp::Ipcp;
use crate::lcp::Lcp;
use crate::session::{Report, Session};

/// 3.2's phase diagram, as far as this goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 3.3: nothing below is carrying anything.
    Dead,
    /// 3.4: LCP is negotiating.
    Establish,
    /// 3.6: IP can flow.
    Network,
    /// 3.7: going away.
    Terminate,
}

/// One end of the link.
#[derive(Debug)]
pub struct Link {
    framer: Framer,
    deframer: Deframer,
    lcp: Session<Lcp>,
    ipcp: Session<Ipcp>,
    phase: Phase,
    line: Vec<u8>,
    arrived: Vec<ip::Arrived>,
    /// 791's Identification field, which only has to differ between datagrams
    /// that are alive at once.
    next_id: u16,
}

impl Link {
    /// `local` is what this end will call itself and `remote` what it will
    /// offer the other; zeroes for either mean it is asking rather than
    /// telling (RFC 1332 3.3).
    pub fn new(local: [u8; 4], remote: [u8; 4]) -> Self {
        // A modem call has a round trip measured in whole seconds once a VoIP
        // trunk is in it, so the Restart timer is the long end of what 4.6
        // suggests rather than the short.
        let limits = Limits { restart_ms: 3000, ..Limits::default() };
        Self {
            framer: Framer::new(),
            deframer: Deframer::new(),
            lcp: Session::new(Lcp::new(crate::lcp::Wanted::default()), limits),
            ipcp: Session::new(Ipcp::new(local, remote), limits),
            phase: Phase::Dead,
            line: Vec::new(),
            arrived: Vec::new(),
            next_id: 1,
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

    /// The modem has connected: there is something under this now.
    pub fn open(&mut self) {
        self.phase = Phase::Establish;
        self.lcp.open();
        self.lcp.up();
        self.pump();
    }

    /// And has hung up.
    pub fn close(&mut self) {
        self.lcp.close();
        self.ipcp.close();
        self.phase = Phase::Terminate;
        self.pump();
    }

    /// Octets from the modem.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            // A frame that did not survive the line is counted where it was
            // counted and otherwise ignored: every protocol here has its own
            // timer and will ask again.
            if let Ok(Some(packet)) = self.deframer.feed(byte) {
                self.deliver(packet);
            }
        }
        self.pump();
    }

    /// Time passing, for the Restart timers.
    pub fn tick(&mut self, ms: u32) {
        self.lcp.tick(ms);
        if self.phase == Phase::Network || self.ipcp.state() != crate::control::State::Initial {
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
        self.send(crate::protocol::IP, datagram);
        true
    }

    fn deliver(&mut self, packet: Packet) {
        match packet.protocol {
            crate::protocol::LCP => {
                if let Some(message) = crate::control::Message::parse(&packet.payload) {
                    self.lcp.receive(message);
                }
            }
            crate::protocol::IPCP => {
                if let Some(message) = crate::control::Message::parse(&packet.payload) {
                    self.ipcp.receive(message);
                }
            }
            crate::protocol::IP => {
                // 3.6: "IP packets received before this phase is reached
                // SHOULD be silently discarded", and one arriving after it
                // that is not an echo is not this layer's business either.
                if !self.up() {
                    return;
                }
                if let Some(arrived) = ip::parse(&packet.payload) {
                    if !arrived.echo.reply {
                        // RFC 792 makes answering an echo the receiver's job,
                        // and doing it here rather than above keeps a ping
                        // working before there is anything above.
                        let reply = arrived.echo.to_reply();
                        let datagram =
                            ip::datagram(arrived.to, arrived.from, &reply, self.next_id);
                        self.next_id = self.next_id.wrapping_add(1);
                        self.send(crate::protocol::IP, datagram);
                    }
                    self.arrived.push(arrived);
                }
            }
            // Anything else. 5.7 asks for a Protocol-Reject, which is worth
            // having once there is something that would send one.
            _ => {}
        }
    }

    /// Move whatever the two sessions have produced onto the line, and follow
    /// the phase they put the link in.
    fn pump(&mut self) {
        for report in self.lcp.take_reports() {
            match report {
                Report::Up => {
                    // 3.4: what LCP agreed takes effect now, and the framer is
                    // where most of it lands.
                    let agreed = self.lcp.protocol.agreed;
                    self.framer.set_accm(agreed.accm);
                    self.framer.set_compression(agreed.acfc, agreed.pfc);
                    self.deframer.set_accm(self.lcp.protocol.wanted.accm);
                    // 3.5 would put authentication here. Nothing does yet, so
                    // 3.6 follows directly.
                    self.ipcp.open();
                    self.ipcp.up();
                }
                Report::Down | Report::Finished => {
                    self.ipcp.down();
                    self.phase = Phase::Dead;
                }
                Report::Started => {}
            }
        }
        for report in self.ipcp.take_reports() {
            match report {
                Report::Up => self.phase = Phase::Network,
                Report::Down | Report::Finished => {
                    if self.phase == Phase::Network {
                        self.phase = Phase::Establish;
                    }
                }
                Report::Started => {}
            }
        }
        let lcp: Vec<_> = self.lcp.take_output();
        for message in lcp {
            self.send(crate::protocol::LCP, message.to_bytes());
        }
        let ipcp: Vec<_> = self.ipcp.take_output();
        for message in ipcp {
            self.send(crate::protocol::IPCP, message.to_bytes());
        }
    }

    fn send(&mut self, protocol: u16, payload: Vec<u8>) {
        self.framer.frame(&Packet { protocol, payload }, &mut self.line);
    }
}
