//! Two connections on one line, with the line behaving badly.
//!
//! Everything in `connection.rs` is written against a document and can be read
//! against it. What cannot be read is whether the whole of it works when the
//! line loses a segment, delivers two in the wrong order, or holds one for a
//! second and then hands it over -- which is what the tests here do, because a
//! stack that has only ever been run over a perfect channel has not been run.
//!
//! Segments cross as octets, through `to_bytes` and `parse`, so the checksum
//! and the pseudo-header are in the path as well.

use tcp::connection::{Connection, Endpoint, Report, State};
use tcp::segment::Segment;

const A: [u8; 4] = [10, 0, 0, 1];
const B: [u8; 4] = [10, 0, 0, 2];

/// How long a step of the line is. Ten milliseconds is fine enough for a
/// timer measured in seconds and coarse enough that a whole transfer runs in
/// a few thousand of them.
const STEP_MS: u32 = 10;

/// What the line does to what is put on it.
#[derive(Debug, Clone, Copy)]
struct Line {
    /// One way, in milliseconds. A modem call over a VoIP trunk is most of a
    /// second; this is enough to make the round trip real without making
    /// every test slow.
    delay_ms: u32,
    /// Drop one segment in this many. Zero drops none.
    lose_one_in: u32,
    /// Deliver one segment in this many after the one behind it.
    swap_one_in: u32,
}

impl Default for Line {
    fn default() -> Self {
        Self { delay_ms: 60, lose_one_in: 0, swap_one_in: 0 }
    }
}

/// Two ends and the line between them.
struct Pair {
    a: Connection,
    b: Connection,
    line: Line,
    /// Segments on their way: when they arrive, which end for, and the octets.
    flying: Vec<(u32, bool, Vec<u8>)>,
    clock: u32,
    crossed: u32,
    lost: u32,
    from_a: Vec<u8>,
    from_b: Vec<u8>,
    held_back: Option<(bool, Vec<u8>)>,
}

impl Pair {
    fn new(line: Line) -> Self {
        let mut a = Connection::connect(Endpoint::new(A, 40_000), Endpoint::new(B, 80), 1_000);
        let mut b = Connection::listen(Endpoint::new(B, 80));
        b.set_initial_sequence(500_000);
        // A modest size, so a few kilobytes is several segments and the
        // windowing is exercised rather than skipped.
        a.set_receive_mss(512);
        b.set_receive_mss(512);
        Self {
            a,
            b,
            line,
            flying: Vec::new(),
            clock: 0,
            crossed: 0,
            lost: 0,
            from_a: Vec::new(),
            from_b: Vec::new(),
            held_back: None,
        }
    }

    /// Run the line for `ms`, stopping early when `done` is satisfied.
    fn run(&mut self, ms: u32, done: impl Fn(&Pair) -> bool) -> bool {
        for _ in 0..(ms / STEP_MS) {
            self.step();
            if done(self) {
                return true;
            }
        }
        done(self)
    }

    fn step(&mut self) {
        self.clock += STEP_MS;

        for (to_b, segment) in self.outbound() {
            let bytes = if to_b {
                segment.to_bytes(A, B)
            } else {
                segment.to_bytes(B, A)
            };
            self.put_on_the_line(to_b, bytes);
        }

        let now = self.clock;
        let arriving: Vec<(bool, Vec<u8>)> = {
            let (arrived, waiting): (Vec<_>, Vec<_>) =
                std::mem::take(&mut self.flying).into_iter().partition(|(at, _, _)| *at <= now);
            self.flying = waiting;
            arrived.into_iter().map(|(_, to_b, bytes)| (to_b, bytes)).collect()
        };
        for (to_b, bytes) in arriving {
            if to_b {
                if let Some(segment) = Segment::parse(A, B, &bytes) {
                    self.b.receive(A, &segment);
                }
                self.from_b.extend(self.b.take_received());
            } else {
                if let Some(segment) = Segment::parse(B, A, &bytes) {
                    self.a.receive(B, &segment);
                }
                self.from_a.extend(self.a.take_received());
            }
        }

        self.a.tick(STEP_MS);
        self.b.tick(STEP_MS);
        self.from_a.extend(self.a.take_received());
        self.from_b.extend(self.b.take_received());
    }

    fn outbound(&mut self) -> Vec<(bool, Segment)> {
        let mut out: Vec<(bool, Segment)> =
            self.a.take_segments().into_iter().map(|s| (true, s)).collect();
        out.extend(self.b.take_segments().into_iter().map(|s| (false, s)));
        out
    }

    fn put_on_the_line(&mut self, to_b: bool, bytes: Vec<u8>) {
        self.crossed += 1;
        if self.line.lose_one_in > 0 && self.crossed.is_multiple_of(self.line.lose_one_in) {
            self.lost += 1;
            return;
        }
        if self.line.swap_one_in > 0 && self.crossed.is_multiple_of(self.line.swap_one_in) {
            // Kept back until the next one has been put on, which is the
            // cheapest way to deliver two out of order.
            if let Some((was_to_b, held)) = self.held_back.take() {
                self.arrive(was_to_b, held);
            }
            self.held_back = Some((to_b, bytes));
            return;
        }
        self.arrive(to_b, bytes);
        if let Some((was_to_b, held)) = self.held_back.take() {
            self.arrive(was_to_b, held);
        }
    }

    fn arrive(&mut self, to_b: bool, bytes: Vec<u8>) {
        self.flying.push((self.clock + self.line.delay_ms, to_b, bytes));
    }

    fn established(&self) -> bool {
        self.a.state == State::Established && self.b.state == State::Established
    }

    fn connect(&mut self) {
        assert!(
            self.run(10_000, |p| p.established()),
            "never established: a is {} and b is {}",
            self.a.state.name(),
            self.b.state.name()
        );
    }
}

#[test]
fn a_connection_is_made_in_three_segments() {
    let mut pair = Pair::new(Line { delay_ms: 60, ..Line::default() });
    pair.connect();
    assert_eq!(pair.crossed, 3, "the handshake took {} segments", pair.crossed);
    assert_eq!(pair.a.remote, Endpoint::new(B, 80));
    // The listener learnt who was calling from the datagram that carried the
    // SYN, which is the only place that is written down.
    assert_eq!(pair.b.remote, Endpoint::new(A, 40_000));
}

#[test]
fn data_crosses_both_ways() {
    let mut pair = Pair::new(Line::default());
    pair.connect();

    assert_eq!(pair.a.send(b"GET / HTTP/1.0\r\n\r\n"), 18);
    assert!(pair.run(5_000, |p| p.from_b.len() >= 18), "the request never arrived");
    assert_eq!(pair.from_b, b"GET / HTTP/1.0\r\n\r\n");

    let answer = b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nhi";
    assert_eq!(pair.b.send(answer), answer.len());
    assert!(pair.run(5_000, |p| p.from_a.len() >= answer.len()), "no answer");
    assert_eq!(pair.from_a, answer);
}

/// Enough to need many segments, a window that fills, and Nagle to decide
/// against sending several times.
fn a_page() -> Vec<u8> {
    let mut page = Vec::new();
    for i in 0..1500u32 {
        page.extend_from_slice(format!("line {i} of a page that came over a modem\r\n").as_bytes());
    }
    page
}

#[test]
fn a_whole_page_crosses_intact() {
    let page = a_page();
    let mut pair = Pair::new(Line::default());
    pair.connect();

    let mut at = 0;
    for _ in 0..20_000 {
        if at < page.len() {
            at += pair.a.send(&page[at..]);
        }
        pair.step();
        if pair.from_b.len() >= page.len() {
            break;
        }
    }
    assert_eq!(pair.from_b.len(), page.len(), "only part of it arrived");
    assert_eq!(pair.from_b, page, "it arrived changed");
}

/// The same, over a line that loses one segment in seven.
#[test]
fn a_page_crosses_a_line_that_loses_segments() {
    let page = a_page();
    let mut pair = Pair::new(Line { lose_one_in: 7, ..Line::default() });
    pair.connect();

    let mut at = 0;
    for _ in 0..60_000 {
        if at < page.len() {
            at += pair.a.send(&page[at..]);
        }
        pair.step();
        if pair.from_b.len() >= page.len() {
            break;
        }
    }
    assert!(pair.lost > 20, "the line did not lose anything: {}", pair.lost);
    assert_eq!(pair.from_b.len(), page.len(), "only part of it arrived");
    assert_eq!(pair.from_b, page, "it arrived changed");
}

/// And over one that delivers every fifth segment behind the one after it.
#[test]
fn a_page_crosses_a_line_that_reorders_segments() {
    let page = a_page();
    let mut pair = Pair::new(Line { swap_one_in: 5, ..Line::default() });
    pair.connect();

    let mut at = 0;
    for _ in 0..60_000 {
        if at < page.len() {
            at += pair.a.send(&page[at..]);
        }
        pair.step();
        if pair.from_b.len() >= page.len() {
            break;
        }
    }
    assert_eq!(pair.from_b.len(), page.len(), "only part of it arrived");
    assert_eq!(pair.from_b, page, "it arrived out of order");
}

/// 3.6: one end closes, the other sees it, answers, and both let go.
#[test]
fn a_close_from_one_end_is_seen_at_the_other() {
    let mut pair = Pair::new(Line::default());
    pair.connect();

    pair.a.send(b"bye");
    pair.a.close();
    assert!(
        pair.run(10_000, |p| p.b.finished() && p.a.state == State::FinWait2),
        "the far end never heard: a is {} and b is {}",
        pair.a.state.name(),
        pair.b.state.name()
    );
    // Everything sent before the FIN arrived before it.
    assert_eq!(pair.from_b, b"bye");
    assert_eq!(pair.b.state, State::CloseWait);
    assert_eq!(pair.a.state, State::FinWait2);

    // 3.6.1: the half-closed direction still carries.
    pair.b.send(b"and to you");
    assert!(pair.run(5_000, |p| p.from_a.len() >= 10), "the other direction stopped");
    assert_eq!(pair.from_a, b"and to you");

    pair.b.close();
    assert!(
        pair.run(10_000, |p| p.a.state == State::TimeWait && p.b.state == State::Closed),
        "the close did not finish: a is {} and b is {}",
        pair.a.state.name(),
        pair.b.state.name()
    );
}

/// And TIME-WAIT lets go on its own, without anybody saying anything more.
#[test]
fn time_wait_ends_by_itself() {
    let mut pair = Pair::new(Line::default());
    pair.connect();
    pair.a.close();
    pair.run(5_000, |p| p.a.state == State::FinWait2);
    pair.b.close();
    assert!(pair.run(5_000, |p| p.a.state == State::TimeWait));

    let reports_before = pair.a.take_reports();
    assert!(!reports_before.contains(&Report::Closed));
    assert!(
        pair.run(60_000, |p| p.a.state == State::Closed),
        "TIME-WAIT never ended"
    );
    assert!(pair.a.take_reports().contains(&Report::Closed));
}

/// A SYN to a connection that is not listening is refused rather than
/// ignored, which is what makes a closed port fail quickly (3.10.7.1).
#[test]
fn a_connection_to_nothing_is_refused() {
    let mut caller = Connection::connect(Endpoint::new(A, 40_000), Endpoint::new(B, 81), 7);
    let mut nobody = Connection::listen(Endpoint::new(B, 81));
    // A listener that has been closed is 3.10.7.1's CLOSED state, which is
    // the state a port nothing is bound to is in.
    nobody.close();
    assert_eq!(nobody.state, State::Closed);

    for _ in 0..100 {
        for segment in caller.take_segments() {
            let bytes = segment.to_bytes(A, B);
            if let Some(s) = Segment::parse(A, B, &bytes) {
                nobody.receive(A, &s);
            }
        }
        for segment in nobody.take_segments() {
            let bytes = segment.to_bytes(B, A);
            if let Some(s) = Segment::parse(B, A, &bytes) {
                caller.receive(B, &s);
            }
        }
        caller.tick(STEP_MS);
        nobody.tick(STEP_MS);
        if caller.state == State::Closed {
            break;
        }
    }
    assert_eq!(caller.state, State::Closed, "it kept trying");
    assert!(
        caller.take_reports().contains(&Report::Refused),
        "it was not told the connection was refused"
    );
}

/// A line that goes away entirely is given up on rather than tried for ever
/// (3.8.3's R2).
#[test]
fn a_far_end_that_vanishes_is_given_up_on() {
    let mut pair = Pair::new(Line::default());
    pair.connect();
    pair.a.send(b"is anybody there");
    // Everything from here on falls on the floor.
    pair.line.lose_one_in = 1;

    assert!(
        pair.run(600_000, |p| p.a.state == State::Closed),
        "it was still trying after ten minutes: {}",
        pair.a.state.name()
    );
    // 3.8.3 wants at least a hundred seconds of trying before it gives up.
    assert!(pair.clock > 100_000, "it gave up after only {} ms", pair.clock);
}

/// A connection carries what is put on it in both directions at once, which
/// is what a proxy does all day.
#[test]
fn both_directions_run_at_once() {
    let mut pair = Pair::new(Line::default());
    pair.connect();
    let up: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let down: Vec<u8> = (0..20_000u32).map(|i| (i % 253) as u8).collect();

    let (mut sent_up, mut sent_down) = (0, 0);
    for _ in 0..40_000 {
        if sent_up < up.len() {
            sent_up += pair.a.send(&up[sent_up..]);
        }
        if sent_down < down.len() {
            sent_down += pair.b.send(&down[sent_down..]);
        }
        pair.step();
        if pair.from_b.len() >= up.len() && pair.from_a.len() >= down.len() {
            break;
        }
    }
    assert_eq!(pair.from_b, up, "what went up did not arrive");
    assert_eq!(pair.from_a, down, "what came down did not arrive");
}
