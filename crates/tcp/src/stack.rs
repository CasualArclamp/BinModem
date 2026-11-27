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
    /// What every new connection asks for, and the most the link below can
    /// carry in one segment.
    receive_mss: u16,
    send_limit: u16,
    /// Whether the line below says when data goes (see [`Stack::pace`]).
    paced: bool,
    /// The bit rate the line delivers to this end, once somebody has said.
    receive_rate: Option<u32>,
    /// The connection the line served last, so the next round starts after
    /// it.
    served: Handle,
    /// The longest round trip a connection here has measured lately, in
    /// milliseconds, for the next connection to start from. Zero is none.
    round_trip_ms: u32,
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
            receive_mss: 1460,
            send_limit: u16::MAX,
            paced: false,
            receive_rate: None,
            served: Handle(0),
            round_trip_ms: 0,
        }
    }

    /// Let the line decide when new data goes: `room` octets of it now.
    ///
    /// Call this every round with what the queue for the line has room for.
    /// From the first call on, no connection sends new data at any other
    /// time, so the queue holds what the line needs and no more, and the
    /// line never waits a round trip for an acknowledgement before it gets
    /// the next segment (see [`Connection::grant`]).
    ///
    /// The room goes round the connections a segment at a time, so one
    /// download does not take all of it.
    pub fn pace(&mut self, room: usize) {
        self.paced = true;
        let mut order: Vec<Handle> = self.connections.keys().copied().collect();
        order.sort();
        let start = order.partition_point(|h| *h <= self.served);
        order.rotate_left(start);

        let mut room = room.min(u32::MAX as usize) as u32;
        // A connection that sent nothing has nothing more to send this
        // round.
        let mut hungry = order;
        for handle in &hungry {
            if let Some(c) = self.connections.get_mut(handle)
                && !c.paced()
            {
                c.grant(0);
            }
        }
        while room > 0 && !hungry.is_empty() {
            let mut still = Vec::with_capacity(hungry.len());
            for handle in hungry {
                let Some(c) = self.connections.get_mut(&handle) else { continue };
                if room == 0 || !c.wants_the_line() {
                    continue;
                }
                // One segment a turn.
                let used = c.grant(1);
                if used > 0 {
                    room = room.saturating_sub(used);
                    self.served = handle;
                    still.push(handle);
                }
            }
            hungry = still;
        }
        self.collect();
    }

    /// Say what bit rate the line delivers to this end, so every connection
    /// offers a window that holds a round trip of it (see
    /// [`Connection::tune_receive`]).
    pub fn set_receive_rate(&mut self, bits_per_second: u32) {
        self.receive_rate = Some(bits_per_second);
    }

    /// Size new connections for the link under them.
    ///
    /// `largest_in` is the biggest datagram this end said it can receive and
    /// `largest_out` the biggest the far end of the link will take, both as
    /// PPP's MRU counts them. RFC 9293 3.7.1 takes the fixed IP and TCP
    /// headers off each: the MSS option "should be equal to the effective MTU
    /// minus the fixed IP and TCP headers", and the effective send MSS is the
    /// smaller of what the far end offers and what the link permits.
    pub fn size_for_link(&mut self, largest_in: u16, largest_out: u16) {
        let headers = (crate::connection::HEADER_LEN + 20) as u16;
        self.receive_mss = largest_in.saturating_sub(headers).max(88);
        self.send_limit = largest_out.saturating_sub(headers).max(88);
    }

    /// The MSS new connections ask for, and the most they will send in one.
    pub fn sizes(&self) -> (u16, u16) {
        (self.receive_mss, self.send_limit)
    }

    /// Every connection, for anything that wants to look at them.
    pub fn connections(&self) -> impl Iterator<Item = (Handle, &Connection)> {
        self.connections.iter().map(|(h, c)| (*h, c))
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
        let mut connection =
            Connection::connect_sized(local, to, self.initial_sequence(), self.receive_mss, self.send_limit);
        connection.expect_round_trip(self.round_trip_ms);
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
            if let Some(rate) = self.receive_rate {
                connection.tune_receive(rate);
            }
            connection.tick(ms);
        }
        // Every connection crosses the same line, so the longest of them is
        // what the next one should expect.
        if let Some(longest) = self.connections.values().map(Connection::round_trip_ms).filter(|ms| *ms > 0).max() {
            self.round_trip_ms = longest;
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
            connection.set_receive_mss(self.receive_mss);
            connection.set_send_limit(self.send_limit);
            connection.receive(from, &segment);
            connection.expect_round_trip(self.round_trip_ms);
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

    fn keep(&mut self, mut connection: Connection) -> Handle {
        if self.paced {
            // Nothing goes until the line says so.
            connection.grant(0);
        }
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

    /// Two connected stacks, the host listening on 80, and the handle of
    /// each end of `n` connections.
    fn connected(n: usize) -> (Stack, Stack, Vec<Handle>, Vec<Handle>) {
        let mut caller = Stack::new(A, 1);
        let mut host = Stack::new(B, 2);
        host.listen(80);
        let calling: Vec<Handle> = (0..n).map(|_| caller.connect(Endpoint::new(B, 80)).unwrap()).collect();
        run(&mut caller, &mut host, 500);
        let mut hosting = host.take_arrived();
        hosting.sort();
        assert_eq!(hosting.len(), n);
        (caller, host, calling, hosting)
    }

    /// Both ends offer a window scale, so a window can be more than sixteen
    /// bits once the line is known to be fast.
    #[test]
    fn a_fast_line_gets_a_window_past_sixteen_bits() {
        let (mut caller, mut host, calling, hosting) = connected(1);
        // 744 kbit/s: ninety-three thousand octets a second.
        caller.set_receive_rate(744_000);
        run(&mut caller, &mut host, 500);
        let offered = caller.get(calling[0]).unwrap().status().rcv_wnd;
        assert!(offered > 100_000, "the window stayed at {offered}");

        // The host sends until the window stops it, with nothing read.
        let data = vec![7u8; 4096];
        for _ in 0..200 {
            host.get_mut(hosting[0]).unwrap().send(&data);
            run(&mut caller, &mut host, 20);
        }
        let held = caller.get(calling[0]).unwrap().available();
        assert!(held > 100_000, "only {held} octets crossed");
        assert!(held <= offered as usize, "{held} octets crossed a window of {offered}");
    }

    /// A slow line keeps a small window: what a sender may put in a
    /// provider's queue is a few seconds of line, not a megabyte.
    #[test]
    fn a_slow_line_keeps_a_small_window() {
        let (mut caller, mut host, calling, _) = connected(1);
        caller.set_receive_rate(33_600);
        run(&mut caller, &mut host, 500);
        let offered = caller.get(calling[0]).unwrap().status().rcv_wnd;
        assert!((16_384..=20_000).contains(&offered), "the window is {offered}");
    }

    /// A paced stack sends new data when the line has room and at no other
    /// time, and shares the room between its connections.
    #[test]
    fn the_line_says_when_data_goes_and_connections_share_it() {
        let (mut caller, mut host, calling, hosting) = connected(2);
        host.pace(0);
        let data = vec![7u8; 8000];
        for h in &hosting {
            assert_eq!(host.get_mut(*h).unwrap().send(&data), data.len());
        }
        run(&mut caller, &mut host, 500);
        for h in &calling {
            assert_eq!(caller.get(*h).unwrap().available(), 0, "data went with no room on the line");
        }

        // Room for two segments: one from each, not two from one.
        host.pace(2 * 1500);
        run(&mut caller, &mut host, 500);
        let got: Vec<usize> = calling.iter().map(|h| caller.get(*h).unwrap().available()).collect();
        assert_eq!(got, [1460, 1460]);

        // And with room every round, all of it crosses.
        for _ in 0..50 {
            host.pace(3000);
            run(&mut caller, &mut host, 20);
        }
        for h in &calling {
            assert_eq!(caller.get(*h).unwrap().available(), data.len());
        }
    }

    /// The second connection on a line starts from the round trip the first
    /// one measured, so a line slower than a second does not send every SYN
    /// twice.
    #[test]
    fn a_new_connection_expects_the_round_trip_the_line_has_shown() {
        let mut caller = Stack::new(A, 1);
        let mut host = Stack::new(B, 2);
        host.listen(80);
        // 800 ms each way, kept by hand.
        let mut flying: std::collections::VecDeque<(u32, bool, Outgoing)> = Default::default();
        let mut step = |caller: &mut Stack, host: &mut Stack, now: u32| {
            for out in caller.take_outgoing() {
                flying.push_back((now + 800, true, out));
            }
            for out in host.take_outgoing() {
                flying.push_back((now + 800, false, out));
            }
            while flying.front().is_some_and(|(at, ..)| *at <= now) {
                let (_, to_host, out) = flying.pop_front().unwrap();
                if to_host {
                    host.deliver(A, out.to, &out.payload);
                } else {
                    caller.deliver(B, out.to, &out.payload);
                }
            }
            caller.tick(10);
            host.tick(10);
        };
        let first = caller.connect(Endpoint::new(B, 80)).unwrap();
        for now in (0..3_000).step_by(10) {
            step(&mut caller, &mut host, now);
        }
        let status = caller.get(first).unwrap().status();
        assert_eq!(status.state, State::Established);
        assert!(status.resent >= 1, "a first SYN on an unknown line waits a second and no more");
        // A SYN sent twice measures nothing (Karn's rule). The request
        // after it does, and the answer does for the host.
        caller.get_mut(first).unwrap().send(b"a request");
        let answering = host.take_arrived()[0];
        host.get_mut(answering).unwrap().send(b"and an answer");
        for now in (3_000..6_000).step_by(10) {
            step(&mut caller, &mut host, now);
        }
        assert!(caller.get(first).unwrap().round_trip_ms() >= 1_600);

        let second = caller.connect(Endpoint::new(B, 80)).unwrap();
        for now in (6_000..12_000).step_by(10) {
            step(&mut caller, &mut host, now);
        }
        let status = caller.get(second).unwrap().status();
        assert_eq!(status.state, State::Established);
        assert_eq!(status.resent, 0, "the second SYN was sent twice");
        // And the host's end of it did not send its answer twice either.
        let hosts: Vec<u32> = host.connections().map(|(_, c)| c.status().resent).collect();
        assert_eq!(hosts.iter().filter(|r| **r == 0).count(), 1, "resent: {hosts:?}");
    }
}
