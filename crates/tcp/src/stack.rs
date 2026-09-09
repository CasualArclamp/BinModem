//! Many connections on one address: finding the one a segment belongs to, and
//! making a new one when a segment does not belong to any.
//!
//! RFC 9293 3.9.2 draws the line this sits on -- "the TCP/lower-level
//! interface" -- and everything below it is somebody else's. What arrives here
//! is a payload and the two addresses out of the datagram that carried it;
//! what leaves is a payload and the address to carry it to. Whether that
//! happens over PPP on a modem or over anything else is not decided here.

use std::collections::HashMap;

use crate::connection::{Connection, Endpoint, Report, State};
use crate::segment::Segment;
use crate::seq::Seq;

/// One connection's name within a stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(pub u32);

/// A segment ready for the layer below: where it goes and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub to: [u8; 4],
    pub payload: Vec<u8>,
}

/// Something the stack has to tell the program above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub handle: Handle,
    pub report: Report,
}

/// IANA's dynamic range, which is where a port nobody asked for comes from.
const FIRST_EPHEMERAL: u16 = 49_152;
const LAST_EPHEMERAL: u16 = 65_535;

/// How many connections may exist at once.
///
/// A browser opens six per site and a modem carries one page at a time, so
/// this is generous. It exists so that a far end sending nothing but SYNs
/// cannot make this end allocate for ever.
const MOST_CONNECTIONS: usize = 64;

/// Every connection on one address.
#[derive(Debug)]
pub struct Stack {
    address: [u8; 4],
    connections: HashMap<Handle, Connection>,
    /// Ports somebody is waiting on. A SYN to one of these makes a connection;
    /// a SYN anywhere else is refused.
    listening: Vec<u16>,
    next_handle: u32,
    next_port: u16,
    clock: u64,
    /// RFC 9293 3.4.1's ISN generator is "a clock... incremented roughly every
    /// 4 microseconds", offset per connection so that two on the same address
    /// do not start at the same place. RFC 6528 wants that offset to be a
    /// keyed hash and this is not one; there is one hop under this and nobody
    /// on it to guess.
    seed: u32,
    out: Vec<Outgoing>,
    events: Vec<Event>,
    /// Connections that arrived and are waiting to be taken.
    arrived: Vec<Handle>,
}

impl Stack {
    pub fn new(address: [u8; 4], seed: u32) -> Self {
        Self {
            address,
            connections: HashMap::new(),
            listening: Vec::new(),
            next_handle: 1,
            next_port: FIRST_EPHEMERAL,
            clock: 0,
            seed,
            out: Vec::new(),
            events: Vec::new(),
            arrived: Vec::new(),
        }
    }

    pub fn address(&self) -> [u8; 4] {
        self.address
    }

    /// Change the address this stack answers to.
    ///
    /// IPCP hands one over after the link is up, which is later than a stack
    /// would ordinarily like to know. Nothing is open at that point, so this
    /// refuses once anything is.
    pub fn set_address(&mut self, address: [u8; 4]) -> bool {
        if !self.connections.is_empty() {
            return false;
        }
        self.address = address;
        true
    }

    /// 3.10.1's passive OPEN, for a whole port rather than one connection.
    pub fn listen(&mut self, port: u16) {
        if !self.listening.contains(&port) {
            self.listening.push(port);
        }
    }

    pub fn stop_listening(&mut self, port: u16) {
        self.listening.retain(|p| *p != port);
    }

    /// The active OPEN. Gives back the handle to talk about it by.
    pub fn connect(&mut self, to: Endpoint) -> Option<Handle> {
        if self.connections.len() >= MOST_CONNECTIONS {
            return None;
        }
        let port = self.free_port()?;
        let local = Endpoint::new(self.address, port);
        let connection = Connection::connect(local, to, self.initial_sequence());
        Some(self.keep(connection))
    }

    /// Connections that came in and have not been taken yet.
    pub fn take_arrived(&mut self) -> Vec<Handle> {
        std::mem::take(&mut self.arrived)
    }

    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.out)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn get(&self, handle: Handle) -> Option<&Connection> {
        self.connections.get(&handle)
    }

    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut Connection> {
        self.connections.get_mut(&handle)
    }

    pub fn open_connections(&self) -> usize {
        self.connections.len()
    }

    /// Everything a connection wants to say, and what became of it.
    pub fn tick(&mut self, ms: u32) {
        self.clock += u64::from(ms);
        for connection in self.connections.values_mut() {
            connection.tick(ms);
        }
        self.collect();
    }

    /// A TCP payload arrived in a datagram from `from`, addressed to `to`.
    pub fn deliver(&mut self, from: [u8; 4], to: [u8; 4], payload: &[u8]) {
        if to != self.address {
            // 3.9.2.3: a datagram that was not for this end is not this end's
            // business, and answering it would tell somebody it exists.
            return;
        }
        let Some(segment) = Segment::parse(from, to, payload) else {
            // A bad checksum. 3.1 MUST-3 has it checked and 1122 3.2.1.6 has a
            // segment that fails simply discarded -- not reset, because the
            // header naming the connection is itself in doubt.
            return;
        };
        let remote = Endpoint::new(from, segment.source_port);
        let local = Endpoint::new(to, segment.destination_port);

        if let Some(handle) = self.find(local, remote) {
            if let Some(connection) = self.connections.get_mut(&handle) {
                connection.receive(from, &segment);
            }
            self.collect();
            return;
        }

        // Nothing has this four-tuple. A SYN to a port somebody is waiting on
        // makes a connection; anything else gets 3.10.7.1's treatment.
        if segment.syn()
            && !segment.rst()
            && self.listening.contains(&segment.destination_port)
            && self.connections.len() < MOST_CONNECTIONS
        {
            let mut connection = Connection::listen(local);
            connection.set_initial_sequence(self.initial_sequence());
            connection.receive(from, &segment);
            let handle = self.keep(connection);
            self.arrived.push(handle);
            self.collect();
            return;
        }
        self.refuse(from, &segment);
    }

    /// 3.10.7.1's CLOSED state, for a segment no connection owns.
    fn refuse(&mut self, from: [u8; 4], segment: &Segment) {
        if segment.rst() {
            // "An incoming segment containing a RST is discarded", which is
            // also what stops two closed ends resetting each other for ever.
            return;
        }
        let mut reset = Segment {
            source_port: segment.destination_port,
            destination_port: segment.source_port,
            ..Segment::default()
        };
        if segment.ack() {
            reset.sequence = segment.acknowledgment;
            reset.flags = crate::segment::flag::RST;
        } else {
            reset.acknowledgment = (Seq(segment.sequence) + segment.length()).0;
            reset.flags = crate::segment::flag::RST | crate::segment::flag::ACK;
        }
        self.out.push(Outgoing {
            to: from,
            payload: reset.to_bytes(self.address, from),
        });
    }

    /// Take everything the connections have produced, and forget the ones that
    /// are over.
    fn collect(&mut self) {
        let mut done = Vec::new();
        for (handle, connection) in &mut self.connections {
            let to = connection.remote.address;
            let from = connection.local.address;
            for segment in connection.take_segments() {
                self.out.push(Outgoing {
                    to,
                    payload: segment.to_bytes(from, to),
                });
            }
            for report in connection.take_reports() {
                self.events.push(Event { handle: *handle, report });
            }
            if connection.state == State::Closed && connection.available() == 0 {
                done.push(*handle);
            }
        }
        for handle in done {
            self.connections.remove(&handle);
        }
    }

    fn find(&self, local: Endpoint, remote: Endpoint) -> Option<Handle> {
        self.connections
            .iter()
            .find(|(_, c)| c.local == local && c.remote == remote)
            .map(|(handle, _)| *handle)
    }

    fn keep(&mut self, connection: Connection) -> Handle {
        let handle = Handle(self.next_handle);
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        self.connections.insert(handle, connection);
        self.collect();
        handle
    }

    fn free_port(&mut self) -> Option<u16> {
        let span = u32::from(LAST_EPHEMERAL - FIRST_EPHEMERAL) + 1;
        for _ in 0..span {
            let port = self.next_port;
            self.next_port = if port == LAST_EPHEMERAL {
                FIRST_EPHEMERAL
            } else {
                port + 1
            };
            let taken = self.connections.values().any(|c| c.local.port == port);
            if !taken && !self.listening.contains(&port) {
                return Some(port);
            }
        }
        None
    }

    /// 3.4.1: "a clock... incremented roughly every 4 microseconds", so a
    /// millisecond is two hundred and fifty of them.
    fn initial_sequence(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.clock as u32).wrapping_mul(250).wrapping_add(self.seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 4] = [10, 0, 0, 1];
    const B: [u8; 4] = [10, 0, 0, 2];

    /// Run two stacks against each other for `ms`, in ten millisecond steps.
    fn run(a: &mut Stack, b: &mut Stack, ms: u32) {
        for _ in 0..(ms / 10) {
            for out in a.take_outgoing() {
                b.deliver(A, out.to, &out.payload);
            }
            for out in b.take_outgoing() {
                a.deliver(B, out.to, &out.payload);
            }
            a.tick(10);
            b.tick(10);
        }
    }

    #[test]
    fn a_connection_finds_its_way_to_a_listening_port() {
        let mut caller = Stack::new(A, 1);
        let mut host = Stack::new(B, 2);
        host.listen(1080);

        let handle = caller.connect(Endpoint::new(B, 1080)).expect("no port");
        run(&mut caller, &mut host, 2_000);

        let arrived = host.take_arrived();
        assert_eq!(arrived.len(), 1, "the far end saw nothing arrive");
        assert_eq!(caller.get(handle).unwrap().state, State::Established);
        assert_eq!(host.get(arrived[0]).unwrap().state, State::Established);
        // The caller's port came out of the dynamic range.
        assert!(caller.get(handle).unwrap().local.port >= FIRST_EPHEMERAL);
    }

    #[test]
    fn a_connection_to_a_port_nobody_wants_is_refused() {
        let mut caller = Stack::new(A, 1);
        let mut host = Stack::new(B, 2);
        // Listening on one port says nothing about any other.
        host.listen(1080);

        let handle = caller.connect(Endpoint::new(B, 25)).expect("no port");
        run(&mut caller, &mut host, 2_000);
        assert!(host.take_arrived().is_empty());
        assert!(
            caller.get(handle).is_none(),
            "it is still trying: {:?}",
            caller.get(handle).map(|c| c.state)
        );
        assert!(
            caller
                .take_events()
                .iter()
                .any(|e| e.report == Report::Refused),
            "nobody was told it was refused"
        );
    }

    /// Several at once, each finding its own way back.
    #[test]
    fn many_connections_keep_their_own_data() {
        let mut caller = Stack::new(A, 7);
        let mut host = Stack::new(B, 9);
        host.listen(1080);

        let handles: Vec<Handle> = (0..4)
            .map(|_| caller.connect(Endpoint::new(B, 1080)).expect("no port"))
            .collect();
        run(&mut caller, &mut host, 2_000);
        let theirs = host.take_arrived();
        assert_eq!(theirs.len(), 4);

        for (i, handle) in handles.iter().enumerate() {
            let message = format!("this is connection {i}");
            caller.get_mut(*handle).unwrap().send(message.as_bytes());
        }
        run(&mut caller, &mut host, 2_000);

        // Every one of them arrived somewhere, and each somewhere got exactly
        // one of them.
        let mut seen: Vec<String> = theirs
            .iter()
            .map(|h| String::from_utf8(host.get_mut(*h).unwrap().take_received()).unwrap())
            .collect();
        seen.sort();
        let mut want: Vec<String> = (0..4).map(|i| format!("this is connection {i}")).collect();
        want.sort();
        assert_eq!(seen, want);
    }

    /// Two connections in a row do not begin at the same sequence number,
    /// which 3.4.1 is entirely about.
    #[test]
    fn two_connections_do_not_start_in_the_same_place() {
        let mut stack = Stack::new(A, 1);
        let first = stack.connect(Endpoint::new(B, 80)).unwrap();
        let second = stack.connect(Endpoint::new(B, 80)).unwrap();
        let a = stack.get(first).unwrap().status().snd_nxt;
        let b = stack.get(second).unwrap().status().snd_nxt;
        assert_ne!(a, b);
        // And they are on different ports, or they would be the same
        // connection.
        assert_ne!(
            stack.get(first).unwrap().local.port,
            stack.get(second).unwrap().local.port
        );
    }

    /// A segment addressed to somebody else is not answered, because
    /// answering it would say this address is here.
    #[test]
    fn a_segment_for_another_address_is_ignored() {
        let mut stack = Stack::new(A, 1);
        stack.listen(80);
        let syn = Segment {
            source_port: 5000,
            destination_port: 80,
            flags: crate::segment::flag::SYN,
            ..Segment::default()
        };
        let elsewhere = [10, 0, 0, 99];
        stack.deliver(B, elsewhere, &syn.to_bytes(B, elsewhere));
        assert!(stack.take_outgoing().is_empty(), "it answered");
        assert_eq!(stack.open_connections(), 0);
    }

    /// And one whose checksum does not hold up is discarded rather than
    /// answered: the header naming the connection is itself in doubt.
    #[test]
    fn a_damaged_segment_is_discarded_without_a_reset() {
        let mut stack = Stack::new(A, 1);
        stack.listen(80);
        let syn = Segment {
            source_port: 5000,
            destination_port: 80,
            flags: crate::segment::flag::SYN,
            ..Segment::default()
        };
        let mut bytes = syn.to_bytes(B, A);
        bytes[0] ^= 0x40;
        stack.deliver(B, A, &bytes);
        assert!(stack.take_outgoing().is_empty(), "it answered a bad checksum");
        assert_eq!(stack.open_connections(), 0);
    }

    /// The address arrives from IPCP after the link is up, which is later than
    /// a stack would like -- but before anything is open, which is what
    /// matters.
    #[test]
    fn the_address_can_be_set_before_anything_is_open_and_not_after() {
        let mut stack = Stack::new([0, 0, 0, 0], 1);
        assert!(stack.set_address(A));
        assert_eq!(stack.address(), A);
        stack.connect(Endpoint::new(B, 80)).unwrap();
        assert!(!stack.set_address(B), "it moved with a connection open");
        assert_eq!(stack.address(), A);
    }
}
