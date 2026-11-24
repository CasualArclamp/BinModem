//! Datagrams waiting for the line, taken a flow at a time.
//!
//! A modem is the slowest hop there is, so whatever is queued for it is
//! queued for seconds. Measured on a 56k call with one 5 MB download running,
//! a page that needed three small packets took 5.7 to 8.6 seconds instead of
//! 1.6: everything the download had made was handed to the modem first, in
//! one line, and the small packets waited behind all of it.
//!
//! So datagrams wait here, one queue to a flow -- a TCP or UDP conversation,
//! told apart by its addresses and ports -- and leave one at a time from each
//! flow in turn. A flow keeps its own order, which TCP and RFC 1144's
//! compression both need; flows overtake each other, which is the point. A
//! new page waits behind at most one packet of the download, not all of it.
//!
//! Nothing is compressed or framed here. Van Jacobson compression leaves out
//! the connection number when a packet belongs to the same connection as the
//! one before it on the line, so it has to be done in the order the packets
//! actually go, which is the order they leave this queue.

use std::collections::{HashMap, VecDeque};

/// A conversation: protocol, both addresses and, for TCP and UDP, both ports.
type Flow = (u8, [u8; 4], [u8; 4], u16, u16);

#[derive(Debug, Default)]
pub struct FairQueue {
    flows: HashMap<Flow, VecDeque<Vec<u8>>>,
    /// Flows with something waiting, in the order they are next served.
    turn: VecDeque<Flow>,
    octets: usize,
}

impl FairQueue {
    pub fn push(&mut self, datagram: Vec<u8>) {
        let flow = flow_of(&datagram);
        self.octets += datagram.len();
        let queue = self.flows.entry(flow).or_default();
        if queue.is_empty() {
            self.turn.push_back(flow);
        }
        queue.push_back(datagram);
    }

    /// The next datagram: the front of the flow whose turn it is.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let flow = self.turn.pop_front()?;
        let queue = self.flows.get_mut(&flow)?;
        let datagram = queue.pop_front()?;
        if queue.is_empty() {
            self.flows.remove(&flow);
        } else {
            self.turn.push_back(flow);
        }
        self.octets -= datagram.len();
        Some(datagram)
    }

    /// Octets waiting.
    pub fn octets(&self) -> usize {
        self.octets
    }

    pub fn is_empty(&self) -> bool {
        self.turn.is_empty()
    }

    pub fn clear(&mut self) {
        self.flows.clear();
        self.turn.clear();
        self.octets = 0;
    }
}

/// The flow an IPv4 datagram belongs to. Anything too short or not IPv4 is
/// one flow of its own.
fn flow_of(datagram: &[u8]) -> Flow {
    if datagram.len() < 20 || datagram[0] >> 4 != 4 {
        return (0, [0; 4], [0; 4], 0, 0);
    }
    let header = usize::from(datagram[0] & 0x0f) * 4;
    let protocol = datagram[9];
    let source = [datagram[12], datagram[13], datagram[14], datagram[15]];
    let destination = [datagram[16], datagram[17], datagram[18], datagram[19]];
    let ports = match protocol {
        6 | 17 if datagram.len() >= header + 4 => (
            u16::from_be_bytes([datagram[header], datagram[header + 1]]),
            u16::from_be_bytes([datagram[header + 2], datagram[header + 3]]),
        ),
        _ => (0, 0),
    };
    (protocol, source, destination, ports.0, ports.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TCP datagram from port `from`, `len` octets long, its last octet a
    /// mark to tell it apart.
    fn tcp(from: u16, len: usize, mark: u8) -> Vec<u8> {
        let mut d = vec![0u8; len.max(40)];
        d[0] = 0x45;
        d[9] = 6;
        d[12..16].copy_from_slice(&[10, 0, 0, 1]);
        d[16..20].copy_from_slice(&[10, 0, 0, 2]);
        d[20..22].copy_from_slice(&from.to_be_bytes());
        d[22..24].copy_from_slice(&80u16.to_be_bytes());
        *d.last_mut().unwrap() = mark;
        d
    }

    #[test]
    fn a_small_flow_is_not_stuck_behind_a_large_one() {
        let mut q = FairQueue::default();
        for i in 0..20 {
            q.push(tcp(1000, 1500, i));
        }
        q.push(tcp(2000, 40, 100));
        // The download's first packet, then the page's: not twenty later.
        assert_eq!(q.pop().unwrap().last(), Some(&0));
        assert_eq!(q.pop().unwrap().last(), Some(&100));
        assert_eq!(q.pop().unwrap().last(), Some(&1));
    }

    #[test]
    fn a_flow_keeps_its_own_order_and_the_count_adds_up() {
        let mut q = FairQueue::default();
        q.push(tcp(1000, 100, 1));
        q.push(tcp(2000, 60, 7));
        q.push(tcp(1000, 100, 2));
        q.push(tcp(1000, 100, 3));
        assert_eq!(q.octets(), 360);
        let order: Vec<u8> = std::iter::from_fn(|| q.pop()).map(|d| *d.last().unwrap()).collect();
        assert_eq!(order, [1, 7, 2, 3]);
        assert!(q.is_empty());
        assert_eq!(q.octets(), 0);
    }
}
