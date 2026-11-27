//! How steadily data crosses a slow line with a long round trip.
//!
//! Two TCP stacks, each on a PPP link, joined by a line that carries a set
//! number of bits a second and delays them. One end sends without pause and
//! the other reads. The test looks at what arrives in each tenth of a
//! second: a good pump fills the line all the time.

use ppp::link::Link;
use tcp::Endpoint;
use tcp::stack::{Handle, Stack};

const A: [u8; 4] = [10, 0, 0, 1];
const B: [u8; 4] = [10, 0, 0, 2];
const STEP_MS: u32 = 10;

/// One direction of the line: a modem queue that drains at the line rate,
/// then a delay.
struct Wire {
    octets_per_step: f64,
    credit: f64,
    queue: std::collections::VecDeque<u8>,
    delay: std::collections::VecDeque<Vec<u8>>,
}

impl Wire {
    fn new(bit_rate: u32, delay_ms: u32) -> Self {
        Self {
            octets_per_step: f64::from(bit_rate) / 8.0 * f64::from(STEP_MS) / 1000.0,
            credit: 0.0,
            queue: Default::default(),
            delay: (0..delay_ms / STEP_MS).map(|_| Vec::new()).collect(),
        }
    }

    /// One step: what leaves the far end of the wire now.
    fn step(&mut self) -> Vec<u8> {
        self.credit += self.octets_per_step;
        let n = (self.credit as usize).min(self.queue.len());
        self.credit -= n as f64;
        if self.queue.is_empty() {
            self.credit = self.credit.min(1.0);
        }
        self.delay.push_back(self.queue.drain(..n).collect());
        self.delay.pop_front().unwrap_or_default()
    }
}

struct End {
    stack: Stack,
    link: Link,
    /// The bit rate this end sends at, and the one it hears at.
    rate: u32,
    hears: u32,
    /// Whether the line paces the stack, as the window has it.
    paced: bool,
    /// Whether the stack sizes its window for the line.
    tuned: bool,
}

impl End {
    fn new(local: [u8; 4], remote: [u8; 4], rate: u32, hears: u32, new: bool, seed: u32) -> Self {
        let mut link = Link::new(local, remote);
        link.open();
        Self { stack: Stack::new(local, seed), link, rate, hears, paced: new, tuned: new }
    }

    /// One step, the way the window drives a link.
    fn step(&mut self, heard: &[u8], wire: &mut Wire) {
        if !heard.is_empty() {
            self.link.feed(heard);
        }
        self.link.tick(STEP_MS);
        for d in self.link.take_carried() {
            if d.protocol == ppp::ip::PROTOCOL_TCP {
                self.stack.deliver(d.from, d.to, &d.payload);
            }
        }
        if self.tuned {
            self.stack.set_receive_rate(self.hears);
        }
        if self.paced {
            self.stack.pace(self.link.room_for(self.rate));
        }
        self.stack.tick(STEP_MS);
        for out in self.stack.take_outgoing() {
            self.link.send_to(out.to, ppp::ip::PROTOCOL_TCP, &out.payload);
        }
        let ahead = (self.rate as usize / 32).max(256);
        let room = ahead.saturating_sub(wire.queue.len());
        wire.queue.extend(self.link.take_line_upto(room));
    }
}

struct Outcome {
    /// Octets a second over the steady part.
    rate: f64,
    /// Share of the steady time with less than four fifths of a line, taken
    /// three seconds at a time: a slow line delivers a whole segment at once
    /// and then nothing for a third of a second.
    thin: f64,
    strip: String,
    resent: u32,
}

/// Send for `seconds` at `down` bit/s one way, `up` the other, each way
/// delayed `delay_ms`.
///
/// `sender` and `reader` say whether each end is this program as it is now.
/// An end that is not is a plain TCP: RFC 5681's window and a fixed buffer,
/// which is also what a provider's end looks like from here.
fn run(down: u32, up: u32, delay_ms: u32, seconds: u32, sender: bool, reader: bool) -> Outcome {
    let mut a = End::new(A, B, down, up, sender, 1);
    let mut b = End::new(B, A, up, down, reader, 2);
    let mut a_to_b = Wire::new(down, delay_ms);
    let mut b_to_a = Wire::new(up, delay_ms);
    let (mut at_a, mut at_b) = (Vec::new(), Vec::new());

    let mut sender: Option<Handle> = None;
    let mut reader: Option<Handle> = None;
    let mut bins: Vec<usize> = Vec::new();
    let mut seed = 0x1234_5678u32;
    let mut started = None;
    let total_steps = (seconds + 30) * 1000 / STEP_MS;
    for step in 0..total_steps {
        a.step(&at_a, &mut a_to_b);
        b.step(&at_b, &mut b_to_a);
        at_b = a_to_b.step();
        at_a = b_to_a.step();

        if a.link.up() && b.link.up() && reader.is_none() && sender.is_none() {
            a.stack.size_for_link(a.link.mru().0, a.link.mru().1);
            b.stack.size_for_link(b.link.mru().0, b.link.mru().1);
            a.stack.listen(80);
            reader = b.stack.connect(Endpoint::new(A, 80));
        }
        if sender.is_none() {
            sender = a.stack.take_arrived().pop();
        }
        // The sender always has more to say. Random, as a page that is
        // already compressed is.
        if let Some(c) = sender.and_then(|h| a.stack.get_mut(h)) {
            let mut block = [0u8; 2048];
            for o in &mut block {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *o = (seed >> 24) as u8;
            }
            let _ = c.send(&block);
        }
        if let Some(c) = reader.and_then(|h| b.stack.get_mut(h)) {
            let got = c.take_received().len();
            if got > 0 && started.is_none() {
                started = Some(step);
            }
            if let Some(from) = started {
                let bin = ((step - from) / (100 / STEP_MS)) as usize;
                if bins.len() <= bin {
                    bins.resize(bin + 1, 0);
                }
                bins[bin] += got;
            }
        }
        if started.is_some_and(|from| step - from >= seconds * 1000 / STEP_MS) {
            break;
        }
    }
    assert!(started.is_some(), "nothing arrived");
    bins.truncate((seconds * 10) as usize);

    let full = f64::from(down) / 8.0 / 10.0;
    let strip: String = bins
        .iter()
        .map(|&n| match n as f64 / full {
            x if x >= 0.5 => '#',
            x if x > 0.0 => '.',
            _ => ' ',
        })
        .collect();
    // The first five seconds are the start: slow start and the first
    // measurement of the round trip.
    let steady = &bins[50.min(bins.len())..];
    let rate = steady.iter().sum::<usize>() as f64 / (steady.len() as f64 / 10.0);
    let spans: Vec<usize> = steady.as_chunks::<30>().0.iter().map(|s| s.iter().sum()).collect();
    let thin = spans.iter().filter(|&&n| (n as f64) < 0.8 * 30.0 * full).count() as f64 / spans.len() as f64;
    let resent = sender.and_then(|h| a.stack.get(h)).map_or(0, |c| c.status().resent);
    Outcome { rate, thin, strip, resent }
}

fn show(name: &str, down: u32, o: &Outcome) {
    println!(
        "  {name}: {:.0} of {:.0} octets/s ({:.0} %), thin {:.0} % of the time, resent {}",
        o.rate,
        f64::from(down) / 8.0,
        100.0 * o.rate / (f64::from(down) / 8.0),
        100.0 * o.thin,
        o.resent
    );
    for line in o.strip.as_bytes().chunks(100) {
        println!("    |{}|", String::from_utf8_lossy(line));
    }
}

/// The line stays full, whatever its rate and however long its round trip.
#[test]
fn the_line_stays_full_across_a_long_round_trip() {
    // Name, bit/s down, bit/s up, delay each way in ms.
    let cases = [
        ("V.34 33600, 0.3 s each way", 33_600, 33_600, 300),
        ("V.90 53333/33600, 0.4 s each way", 53_333, 33_600, 400),
        ("V.90 53333/33600, 0.8 s each way", 53_333, 33_600, 800),
        ("V.ARC 200k, 0.4 s each way", 200_000, 200_000, 400),
        ("V.ARC 744k, 0.4 s each way", 744_000, 744_000, 400),
        ("V.ARC 744k, 1.0 s each way", 744_000, 744_000, 1000),
    ];
    let mut bad = Vec::new();
    for (name, down, up, delay) in cases {
        let plain = run(down, up, delay, 30, false, false);
        show(&format!("{name}, two plain ends"), down, &plain);
        // Only the reading end is this program: a call to a provider.
        let reading = run(down, up, delay, 30, false, true);
        show(&format!("{name}, a plain sender"), down, &reading);
        let o = run(down, up, delay, 30, true, true);
        show(name, down, &o);
        // A plain sender starts slowly and V.ARC has no plain senders, so
        // this is asked of the dial-up rates only.
        if down < 100_000 && reading.rate < 0.9 * f64::from(down) / 8.0 {
            bad.push(name);
        }
        // PPP framing and the two headers take a few percent.
        if o.rate < 0.9 * f64::from(down) / 8.0 || o.thin > 0.02 {
            bad.push(name);
        }
    }
    assert!(bad.is_empty(), "the line ran thin on: {bad:?}");
}
