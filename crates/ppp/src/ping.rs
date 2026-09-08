//! Sending echoes and keeping track of which ones came back.
//!
//! RFC 792 gives the two fields that make this possible and says what they are
//! for: the identifier and sequence number "may be used by the echo sender to
//! aid in matching the replies with the requests". Everything here is that
//! sentence -- a request goes out with a sequence number and the time it left,
//! and a reply carrying the same number is the answer to it.
//!
//! It is separate from [`crate::link`] because it is the first thing on top of
//! IP rather than part of it, and because a round-trip time is worth measuring
//! somewhere it can be tested without a modem in the way.

use crate::ip::Arrived;
use crate::link::Link;

/// How long to wait for an answer before calling it lost.
///
/// A VoIP trunk in the path costs the better part of a second each way before
/// the modem has done anything, and the modem's own error control will resend
/// a damaged frame on top of that. Five seconds is the point past which an
/// answer is not late, it is missing.
pub const DEFAULT_TIMEOUT_MS: u32 = 5_000;

/// How often to send, when sending repeatedly.
pub const DEFAULT_INTERVAL_MS: u32 = 1_000;

/// What happened to one echo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// It went out.
    Sent(u16),
    /// And came back, after this long.
    Reply { sequence: u16, round_trip_ms: u64 },
    /// Or did not.
    Lost(u16),
    /// Something else's echo arrived and was answered. Not this end's ping,
    /// but the clearest possible sign the link carries IP.
    Answered,
}

/// What the echoes have added up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub sent: u32,
    pub received: u32,
    pub lost: u32,
    /// The last round trip, and the extremes of all of them.
    pub last_ms: u64,
    pub best_ms: u64,
    pub worst_ms: u64,
    total_ms: u64,
}

impl Stats {
    /// The mean round trip, over the echoes that came back.
    pub fn average_ms(&self) -> Option<f64> {
        (self.received > 0).then(|| self.total_ms as f64 / f64::from(self.received))
    }

    /// The proportion lost, of those that have been settled one way or the
    /// other. An echo still in flight is neither, and counting it as lost
    /// would make every reading start at a hundred per cent.
    pub fn loss(&self) -> f64 {
        let settled = self.received + self.lost;
        if settled == 0 {
            return 0.0;
        }
        f64::from(self.lost) / f64::from(settled)
    }

    fn answered(&mut self, round_trip_ms: u64) {
        self.received += 1;
        self.last_ms = round_trip_ms;
        self.total_ms += round_trip_ms;
        self.best_ms = if self.best_ms == 0 {
            round_trip_ms
        } else {
            self.best_ms.min(round_trip_ms)
        };
        self.worst_ms = self.worst_ms.max(round_trip_ms);
    }
}

/// One echo that has gone out and not come back.
#[derive(Debug, Clone, Copy)]
struct Outstanding {
    sequence: u16,
    sent_at: u64,
}

/// Sends echoes and matches the answers to them.
#[derive(Debug)]
pub struct Pinger {
    /// 792's Identifier: what tells this end's echoes from anybody else's.
    id: u16,
    next_sequence: u16,
    outstanding: Vec<Outstanding>,
    /// Link time in milliseconds, which is the only clock this has. Measuring
    /// against it rather than against the wall means a round trip is what the
    /// link took, not what the machine was doing at the time.
    clock: u64,
    since_sent: u32,
    running: bool,
    /// Set when an echo is due but the link was not up to send it on, so the
    /// first one goes the moment it can rather than an interval later.
    waiting: bool,
    pub every_ms: u32,
    pub timeout_ms: u32,
    /// What to put in it. The far end sends it straight back, so anything that
    /// arrives changed is the line's doing and worth seeing.
    pub payload: Vec<u8>,
    pub stats: Stats,
    events: Vec<Event>,
}

impl Pinger {
    pub fn new(id: u16) -> Self {
        Self {
            id,
            next_sequence: 1,
            outstanding: Vec::new(),
            clock: 0,
            since_sent: 0,
            running: false,
            waiting: false,
            every_ms: DEFAULT_INTERVAL_MS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            payload: b"BinModem".to_vec(),
            stats: Stats::default(),
            events: Vec::new(),
        }
    }

    /// Start sending: one straight away and then one every `every_ms`.
    pub fn start(&mut self) {
        self.running = true;
        self.waiting = true;
    }

    /// Stop sending. Echoes already in flight are still waited for, because an
    /// answer that arrives after the last request is still an answer.
    pub fn stop(&mut self) {
        self.running = false;
        self.waiting = false;
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// How many are still in flight.
    pub fn in_flight(&self) -> usize {
        self.outstanding.len()
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Send one now, whether or not it is running.
    pub fn ping_once(&mut self, link: &mut Link) -> bool {
        let sequence = self.next_sequence;
        if !link.ping(self.id, sequence, &self.payload) {
            return false;
        }
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.outstanding.push(Outstanding { sequence, sent_at: self.clock });
        self.stats.sent += 1;
        self.since_sent = 0;
        self.waiting = false;
        self.events.push(Event::Sent(sequence));
        true
    }

    /// Run for `ms` of link time: take in what has arrived, give up on
    /// anything that has waited too long, and send if one is due.
    ///
    /// Gives back whatever arrived that was not an answer to this end's own
    /// echoes, which is the far end's pings and anything else the link brought
    /// up.
    pub fn poll(&mut self, link: &mut Link, ms: u32) -> Vec<Arrived> {
        self.clock += u64::from(ms);
        self.since_sent = self.since_sent.saturating_add(ms);

        let mut others = Vec::new();
        for arrived in link.take_arrived() {
            if !self.claim(&arrived) {
                if !arrived.echo.reply {
                    // The link answered it on the way past; this is only the
                    // record that it happened.
                    self.events.push(Event::Answered);
                }
                others.push(arrived);
            }
        }

        // Before sending, so an echo cannot be declared lost in the same round
        // it was sent in, however large a tick this is called with.
        self.expire();

        if self.running && (self.waiting || self.since_sent >= self.every_ms) {
            self.waiting = true;
            self.ping_once(link);
        }
        others
    }

    /// Take one arrival if it is an answer to something this end sent.
    fn claim(&mut self, arrived: &Arrived) -> bool {
        if !arrived.echo.reply || arrived.echo.id != self.id {
            return false;
        }
        let Some(at) = self
            .outstanding
            .iter()
            .position(|o| o.sequence == arrived.echo.sequence)
        else {
            // A duplicate, or one already given up on. Not this end's any more.
            return false;
        };
        let sent_at = self.outstanding.remove(at).sent_at;
        let round_trip_ms = self.clock.saturating_sub(sent_at);
        self.stats.answered(round_trip_ms);
        self.events.push(Event::Reply { sequence: arrived.echo.sequence, round_trip_ms });
        true
    }

    fn expire(&mut self) {
        let timeout = u64::from(self.timeout_ms);
        let clock = self.clock;
        let mut lost = Vec::new();
        self.outstanding.retain(|o| {
            let overdue = clock.saturating_sub(o.sent_at) >= timeout;
            if overdue {
                lost.push(o.sequence);
            }
            !overdue
        });
        for sequence in lost {
            self.stats.lost += 1;
            self.events.push(Event::Lost(sequence));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two ends of a link, run against each other for `ms`.
    fn together(a: &mut Link, b: &mut Link, pinger: &mut Pinger, ms: u32) {
        for _ in 0..ms {
            let from_a = a.take_line();
            if !from_a.is_empty() {
                b.feed(&from_a);
            }
            let from_b = b.take_line();
            if !from_b.is_empty() {
                a.feed(&from_b);
            }
            a.tick(1);
            b.tick(1);
            pinger.poll(a, 1);
        }
    }

    fn pair() -> (Link, Link) {
        let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
        a.open();
        b.open();
        let mut idle = Pinger::new(0);
        together(&mut a, &mut b, &mut idle, 200);
        assert!(a.up() && b.up(), "the link did not come up: {:?} {:?}", a.phase(), b.phase());
        (a, b)
    }

    #[test]
    fn echoes_go_out_at_the_interval_and_come_back() {
        let (mut a, mut b) = pair();
        let mut pinger = Pinger::new(0x4269);
        pinger.every_ms = 100;
        pinger.start();
        together(&mut a, &mut b, &mut pinger, 1_000);
        pinger.stop();
        together(&mut a, &mut b, &mut pinger, 50);

        // One straight away and one every hundred milliseconds after it.
        assert_eq!(pinger.stats.sent, 10, "sent {}", pinger.stats.sent);
        assert_eq!(pinger.stats.received, pinger.stats.sent, "not all came back");
        assert_eq!(pinger.stats.lost, 0);
        assert_eq!(pinger.stats.loss(), 0.0);
    }

    #[test]
    fn a_round_trip_is_measured_in_link_time() {
        let (mut a, mut b) = pair();
        let mut pinger = Pinger::new(1);
        assert!(pinger.ping_once(&mut a));
        together(&mut a, &mut b, &mut pinger, 20);
        assert_eq!(pinger.stats.received, 1);
        // The two ends here are wired together with nothing in between, so a
        // round trip is a round of the loop each way. What matters is that it
        // is measured at all and comes out small rather than absurd.
        assert!(pinger.stats.last_ms <= 4, "took {} ms", pinger.stats.last_ms);
        assert_eq!(pinger.stats.best_ms, pinger.stats.last_ms);
        assert_eq!(pinger.stats.average_ms(), Some(pinger.stats.last_ms as f64));
    }

    /// An echo nobody answers is given up on rather than waited for for ever.
    #[test]
    fn an_unanswered_echo_is_eventually_lost() {
        let (mut a, _b) = pair();
        let mut pinger = Pinger::new(2);
        pinger.timeout_ms = 300;
        assert!(pinger.ping_once(&mut a));
        // Thrown away rather than delivered: the far end never hears it.
        let _ = a.take_line();
        for _ in 0..400 {
            a.tick(1);
            pinger.poll(&mut a, 1);
        }
        assert_eq!(pinger.stats.lost, 1);
        assert_eq!(pinger.stats.received, 0);
        assert_eq!(pinger.in_flight(), 0);
        assert!(pinger.take_events().contains(&Event::Lost(1)));
    }

    /// The far end's pings are answered and reported, but not counted as this
    /// end's own.
    #[test]
    fn the_far_ends_echoes_are_not_mistaken_for_answers() {
        let (mut a, mut b) = pair();
        let mut theirs = Pinger::new(0xbeef);
        assert!(theirs.ping_once(&mut b));
        let mut mine = Pinger::new(0x1234);
        together(&mut a, &mut b, &mut mine, 20);
        assert_eq!(mine.stats.sent, 0);
        assert_eq!(mine.stats.received, 0);
        assert!(mine.take_events().contains(&Event::Answered));
        assert_eq!(theirs.stats.sent, 1);
    }

    /// A reply for somebody else's identifier is not this end's.
    #[test]
    fn another_senders_reply_is_left_alone() {
        let (mut a, mut b) = pair();
        let mut pinger = Pinger::new(0x0001);
        assert!(pinger.ping_once(&mut a));
        // Same sequence number, different identifier.
        assert!(b.ping(0x0002, 1, b"not yours"));
        together(&mut a, &mut b, &mut pinger, 20);
        assert_eq!(pinger.stats.received, 1, "it took the wrong one");
    }

    /// Nothing goes out before the link can carry it, and nothing is counted
    /// as sent either.
    #[test]
    fn a_ping_on_a_link_that_is_not_up_is_not_counted() {
        let mut alone = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let mut pinger = Pinger::new(3);
        pinger.start();
        for _ in 0..500 {
            alone.tick(1);
            pinger.poll(&mut alone, 1);
        }
        assert_eq!(pinger.stats.sent, 0);
        assert_eq!(pinger.stats.lost, 0);
    }

    /// And once it is up, the first one does not wait for the interval.
    #[test]
    fn the_first_echo_goes_as_soon_as_there_is_somewhere_to_send_it() {
        let mut a = Link::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let mut b = Link::new([10, 0, 0, 2], [10, 0, 0, 1]);
        let mut pinger = Pinger::new(4);
        pinger.every_ms = 60_000;
        pinger.start();
        a.open();
        b.open();
        together(&mut a, &mut b, &mut pinger, 200);
        assert_eq!(pinger.stats.sent, 1, "it waited a whole minute");
        assert_eq!(pinger.stats.received, 1);
    }

    #[test]
    fn loss_over_nothing_is_nothing_rather_than_everything() {
        let stats = Stats::default();
        assert_eq!(stats.loss(), 0.0);
        assert_eq!(stats.average_ms(), None);
    }
}
