//! A V.32 or V.32bis call, overheard on a recording with an end a channel.
//!
//! V.32 gives both directions the whole band and one carrier, so on a tap of
//! the pair nothing can be read once both ends are sending. With an end a
//! channel, and the other end's echo taken out of it, each modem is alone
//! on its own: its conditioning signal and training segment, its rate
//! signals, E, and then data, which is read at the rate and coding E said.

use datapump::v32::startup::{
    RateDetector, agreed_coding, describe_sequence, is_end_signal, is_rate_signal, offered_rate,
};
use datapump::v32::{Coding, Mode, Receiver};

use crate::model::{End, Event, Layer};
use crate::v34::Stream;

/// Symbols held for one event.
const HELD: usize = 200_000;

/// One modem's transmission.
struct Direction {
    fs: f64,
    who: End,
    /// Whether this is the answering modem, whose rate signals are R1 and
    /// R3; the calling modem's is R2.
    answering: bool,
    rx: Receiver,
    rates: RateDetector,
    power: f64,
    alpha: f64,
    loud: f64,
    quiet: usize,
    /// When the signal now on the line came up.
    since: Option<f64>,
    trained: bool,
    /// Training segments so far, and rate signals.
    rounds: usize,
    /// The rate signal being repeated: what it is, and its event.
    run: Option<(u16, usize)>,
    /// The last rate signal this end sent, for the other to agree a coding
    /// against.
    said: Option<u16>,
    data: Option<usize>,
    seen: Vec<(f32, f32)>,
    events: Vec<Event>,
    stream: Stream,
}

impl Direction {
    fn new(answering: bool, who: End, fs: f64) -> Self {
        // A receiver as the other end has it: the calling end's is the one
        // that hears the answering modem.
        let mut rx = Receiver::new(if answering { Mode::Call } else { Mode::Answer }, fs);
        rx.hunt();
        Self {
            fs,
            who,
            answering,
            rx,
            rates: RateDetector::new(),
            power: 0.0,
            alpha: 1.0 - (-1.0 / (0.005 * fs)).exp(),
            loud: 0.0,
            quiet: 0,
            since: None,
            trained: false,
            rounds: 0,
            run: None,
            said: None,
            data: None,
            seen: Vec::new(),
            events: Vec::new(),
            stream: Stream {
                from: Some(who),
                ..Stream::default()
            },
        }
    }

    fn event(&self, start: f64, end: f64, layer: Layer, tag: &str) -> Event {
        // 1800 Hz, 2400 symbols a second.
        Event::new(start, end.max(start), self.who, layer, tag).band(600.0, 3000.0)
    }

    fn feed(&mut self, n: usize, x: f64, theirs: Option<u16>) {
        let now = n as f64 / self.fs;
        self.power += self.alpha * (x * x - self.power);
        if self.power.sqrt() > 0.003 {
            if self.since.is_none() {
                self.since = Some(now);
            }
        } else {
            self.since = None;
        }
        self.rx.feed(x);
        let bits = self.rx.take_bits();
        if !bits.is_empty() && self.rx.is_tracking() && self.seen.len() < HELD {
            let (i, q) = self.rx.constellation_point();
            self.seen.push((i as f32, q as f32));
        }
        if !self.trained && self.rx.trained_via().is_some() {
            self.trained = true;
            self.rounds += 1;
            self.loud = self.power;
            self.rates = RateDetector::new();
            self.run = None;
            let from = self.since.unwrap_or(now).min(now);
            let points = std::mem::take(&mut self.seen);
            let snr = self.rx.trained_snr_db();
            // A training with no signal before it is the receiver finding
            // its feet again on what it was already following.
            if now - from > 0.05 {
                self.events.push(
                    self.event(from, now, Layer::Startup, "TRN")
                        .summary(format!(
                            "S, S-bar and TRN, {:.0} ms: trained a receiver to {snr:.0} dB",
                            1000.0 * (now - from)
                        ))
                        .field("carrier", "1800 Hz, 2400 symbols/s")
                        .field("signal to noise after training", format!("{snr:.1} dB"))
                        .qam(&points, 4),
                );
            }
        }
        if let Some(index) = self.data {
            // Data mode: everything is data, at the rate E gave.
            for bit in bits {
                if self.stream.bits.len().is_multiple_of(256) {
                    self.stream.marks.push((self.stream.bits.len(), now));
                }
                self.stream.bits.push(bit);
            }
            self.events[index].end = now;
        } else if self.trained {
            for bit in bits {
                let Some(sequence) = self.rates.feed(bit) else {
                    continue;
                };
                if is_end_signal(sequence) {
                    self.heard_end(now, sequence, theirs);
                    break;
                }
                if is_rate_signal(sequence) {
                    self.heard_rate(now, sequence);
                }
            }
        }
        // A modem that has said its rates goes quiet while the other trains,
        // and comes back with a conditioning signal of its own; and data
        // ends when the signal does.
        let silent = self.loud > 0.0 && self.power < 0.02 * self.loud;
        self.quiet = if silent { self.quiet + 1 } else { 0 };
        if self.quiet as f64 > 0.02 * self.fs && self.trained {
            if let Some(index) = self.data.take() {
                self.finish_data(index, now - 0.02, "the signal stopped");
            }
            self.trained = false;
            self.run = None;
            self.seen.clear();
            self.rx.hunt();
        }
    }

    fn heard_rate(&mut self, now: f64, sequence: u16) {
        self.said = Some(sequence);
        if let Some((was, index)) = self.run
            && was == sequence
        {
            self.events[index].end = now;
            return;
        }
        // R1 and R3 are the answering modem's, R2 the calling modem's; a
        // retrain goes round them again.
        let tag = match (self.answering, self.rounds) {
            (true, 1) => "R1",
            (true, _) => "R3",
            (false, _) => "R2",
        };
        let points = std::mem::take(&mut self.seen);
        let event = self
            .event(now - 16.0 / 4800.0, now, Layer::Startup, tag)
            .summary(format!("{tag}: {}", describe_sequence(sequence)))
            .field("rates and coding", describe_sequence(sequence))
            .field("the sixteen bits", format!("{sequence:016b}"))
            .qam(&points, 4);
        self.events.push(event);
        self.run = Some((sequence, self.events.len() - 1));
    }

    fn heard_end(&mut self, now: f64, sequence: u16, theirs: Option<u16>) {
        let rate = offered_rate(sequence);
        // The coding is what both ends' rate signals allow; where the other
        // end's was not read, this end's own is the best guide there is.
        let coding = agreed_coding(sequence, theirs.unwrap_or(sequence), rate);
        self.rx.set_data_rate(rate);
        self.rx.set_coding(coding);
        self.run = None;
        self.seen.clear();
        self.events.push(
            self.event(now - 16.0 / 4800.0, now, Layer::Startup, "E")
                .summary(format!("E: {}", describe_sequence(sequence)))
                .field("the sixteen bits", format!("{sequence:016b}")),
        );
        self.stream.bps = Some(rate);
        let trellis = coding == Coding::Trellis;
        self.events.push(
            self.event(now, now, Layer::Data, "data")
                .summary(format!(
                    "data mode at {rate} bit/s{}",
                    if trellis { ", trellis coded" } else { "" }
                ))
                .field("rate", format!("{rate} bit/s"))
                .field("coding", if trellis { "trellis" } else { "none" })
                .field(
                    "points in the constellation",
                    datapump::v32::constellation_size(rate, coding).to_string(),
                ),
        );
        self.data = Some(self.events.len() - 1);
    }

    fn finish_data(&mut self, index: usize, now: f64, why: &str) {
        let points = std::mem::take(&mut self.seen);
        let count = self.stream.bps.map_or(32, |rate| {
            datapump::v32::constellation_size(rate, Coding::Trellis)
        });
        let event = &mut self.events[index];
        event.end = now.max(event.start);
        event.fields.push(("ended".to_owned(), why.to_owned()));
        let done = event.clone().qam(&points, count);
        self.events[index] = done;
        self.stream.marks.push((self.stream.bits.len(), now));
    }
}

/// Follow a V.32 or V.32bis call from `from` seconds. `call` and `answer`
/// are the two modems' own transmissions.
pub fn follow(
    call: (&[f32], End),
    answer: (&[f32], End),
    fs: f64,
    from: f64,
) -> Option<(Vec<Event>, [Stream; 2])> {
    let mut caller = Direction::new(false, call.1, fs);
    let mut answerer = Direction::new(true, answer.1, fs);
    let first = (from.max(0.0) * fs) as usize;
    let last = call.0.len().min(answer.0.len());
    for n in first..last {
        let (theirs, ours) = (answerer.said, caller.said);
        caller.feed(n, call.0[n] as f64, theirs);
        answerer.feed(n, answer.0[n] as f64, ours);
    }
    let end = last as f64 / fs;
    for direction in [&mut caller, &mut answerer] {
        if let Some(index) = direction.data.take() {
            direction.finish_data(index, end, "the recording ended");
        }
    }
    if caller.events.is_empty() && answerer.events.is_empty() {
        return None;
    }
    let mut events = caller.events;
    events.extend(answerer.events);
    Some((events, [caller.stream, answerer.stream]))
}
