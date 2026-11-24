//! Phases 3 and 4 and data mode of a V.34 call, overheard on a recording
//! with an end a channel.
//!
//! Each end's transmission is alone on its channel, so each is read with
//! the receiver the other end read it with: S and S-bar, PP and TRN, J, the
//! second TRN, MP and E, and then data. What a tap has that neither end had
//! is both MPs. An MP is what its sender wants to *receive*, so the data
//! each modem sends is laid out by the other's MP, and the rate by both.
//!
//! The two channels are fed in step, as they are for V.90, so that what one
//! end has said is there when the other's signal turns on it.

use std::collections::VecDeque;

use datapump::v32::Mode;
use datapump::v34::constellation::Point;
use datapump::v34::data::{Acquired, Acquirer, Decoder, Params};
use datapump::v34::frame::Framing;
use datapump::v34::info::SymbolRate;
use datapump::v34::mp::{Finder, Found, Mp, Trellis};
use datapump::v34::qam::Band;
use datapump::v34::receiver::{Heard, Receiver, Reference};
use datapump::v34::signals::{self, J_PRIME, Reader, Size};
use datapump::v34::training::{SWatch, Watched, negotiate};
use datapump::v34::trellis::Code;
use dsp::Complex;

use crate::model::{End, Event, Layer};
use crate::phase2::Settled;

/// Symbols held for one event before the older ones are let go.
const HELD: usize = 200_000;

/// What each end has said that the other's signal depends on. Index 0 is
/// the call modem, 1 the answer modem.
#[derive(Debug, Default)]
struct Shared {
    /// The constellation each end's J asked the other to train it with.
    j: [Option<Size>; 2],
    mp: [Option<Mp>; 2],
    /// The precoding coefficients each end last sent. An MP may come
    /// without them, and then the ones before it stand (11.4.1.1.2).
    precoding: [[(i16, i16); 3]; 2],
}

/// One direction's data, as bits, with the time of every so many of them.
#[derive(Debug, Clone, Default)]
pub struct Stream {
    pub from: Option<End>,
    pub bits: Vec<bool>,
    /// (bits so far, seconds), in order.
    pub marks: Vec<(usize, f64)>,
    /// The rate data mode was last at.
    pub bps: Option<u32>,
}

impl Stream {
    /// When bit `index` went by.
    pub fn time_of(&self, index: usize) -> f64 {
        let after = self.marks.partition_point(|(bits, _)| *bits <= index);
        match (self.marks.get(after.wrapping_sub(1)), self.marks.get(after)) {
            (Some(&(b0, t0)), Some(&(b1, t1))) if b1 > b0 => t0 + (t1 - t0) * (index - b0) as f64 / (b1 - b0) as f64,
            (Some(&(_, t)), _) | (None, Some(&(_, t))) => t,
            (None, None) => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    AwaitS,
    Training3,
    /// TRN and then J.
    Phase3,
    /// The answer modem's second S, after its J.
    AwaitS4,
    Training4,
    /// TRN at J's size, MP and E.
    Phase4,
    Data,
    /// S has been heard in data mode: a rate renegotiation.
    Renegotiating,
    Over,
}

/// One modem's transmission.
struct Direction {
    fs: f64,
    who: End,
    /// 0 for the call modem, 1 for the answer modem.
    me: usize,
    mode: Mode,
    rate: SymbolRate,
    band: Band,
    rx: Receiver,
    reader: Reader,
    finder: Finder,
    stage: Stage,
    s_from: Option<f64>,
    since: Option<f64>,
    power: f64,
    alpha: f64,
    loud: f64,
    quiet: usize,
    /// Reading TRN: true until its ones give way.
    trn: bool,
    grace: usize,
    trn_from: f64,
    trained_db: f64,
    size: Size,
    tail: VecDeque<bool>,
    j: Option<Size>,
    j_from: f64,
    mp_run: Option<(Mp, usize)>,
    mp_found: bool,
    decoder: Option<Decoder>,
    acquirer: Option<Acquirer>,
    params: Option<Params>,
    slips: u32,
    b1_left: usize,
    s_watch: SWatch,
    /// Symbols the trellis has been paying dearly for, running.
    bad: usize,
    data_event: Option<usize>,
    seen: Vec<(f32, f32)>,
    events: Vec<Event>,
    stream: Stream,
    renegotiations: usize,
}

impl Direction {
    fn new(band: Band, rate: SymbolRate, mode: Mode, who: End, fs: f64) -> Self {
        let mut rx = Receiver::new(band, fs);
        rx.hunt();
        Self {
            fs,
            who,
            me: usize::from(mode == Mode::Answer),
            mode,
            rate,
            band,
            rx,
            reader: Reader::new(mode),
            finder: Finder::new(),
            stage: Stage::AwaitS,
            s_from: None,
            since: None,
            power: 0.0,
            alpha: 1.0 - (-1.0 / (0.005 * fs)).exp(),
            loud: 0.0,
            quiet: 0,
            trn: false,
            grace: 0,
            trn_from: 0.0,
            trained_db: 0.0,
            size: Size::Four,
            tail: VecDeque::with_capacity(32),
            j: None,
            j_from: 0.0,
            mp_run: None,
            mp_found: false,
            decoder: None,
            acquirer: None,
            params: None,
            slips: 0,
            b1_left: 0,
            s_watch: SWatch::default(),
            bad: 0,
            data_event: None,
            seen: Vec::new(),
            events: Vec::new(),
            stream: Stream { from: Some(who), ..Stream::default() },
            renegotiations: 0,
        }
    }

    fn event(&self, start: f64, end: f64, layer: Layer, tag: &str) -> Event {
        let (carrier, half) = (self.band.carrier(), self.band.baud() / 2.0);
        Event::new(start, end, self.who, layer, tag).band((carrier - half).max(0.0), carrier + half)
    }

    fn begin_trn(&mut self, size: Size, now: f64) {
        self.size = size;
        self.trn = true;
        // The descrambler has not followed this scrambler: its first two
        // dozen bits are its own filling.
        self.grace = 24 / size.bits() + 1;
        self.trn_from = now;
        self.tail.clear();
        self.seen.clear();
    }

    fn feed(&mut self, n: usize, x: f64, shared: &mut Shared) {
        if self.stage == Stage::Over {
            return;
        }
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
        while let Some(heard) = self.rx.heard() {
            match heard {
                Heard::S => {
                    if self.s_from.is_none() {
                        self.s_from = Some(now);
                    }
                }
                Heard::Reversal { at } => self.reversal(now, at, shared),
                Heard::Trained { snr_db } => {
                    self.trained_db = snr_db;
                    self.loud = self.power;
                    match self.stage {
                        Stage::Training3 => {
                            let pp = signals::PP_SYMBOLS as f64 / self.band.baud();
                            let from = self.trn_from;
                            self.events.push(
                                self.event(from, from + pp, Layer::Phase3, "PP")
                                    .summary("PP: the sequence the far equaliser is first fitted to".to_owned()),
                            );
                            self.begin_trn(Size::Four, from + pp);
                            self.stage = Stage::Phase3;
                        }
                        Stage::Training4 => {
                            let (size, from) = (self.size, self.trn_from);
                            self.begin_trn(size, from);
                            self.stage = Stage::Phase4;
                        }
                        _ => {}
                    }
                }
                Heard::Untrained => {
                    // An S and S-bar with nothing to train on after them.
                    self.stage = match self.stage {
                        Stage::Training3 => Stage::AwaitS,
                        Stage::Training4 => Stage::AwaitS4,
                        other => other,
                    };
                    self.rx.hunt();
                }
                Heard::Symbol(symbol) => {
                    if self.seen.len() < HELD {
                        self.seen.push((symbol.point.re as f32, symbol.point.im as f32));
                    }
                    self.symbol(now, symbol.decided, symbol.point, shared);
                }
            }
        }
        // The answer modem's J goes on until it stops; data goes on until
        // the line goes quiet.
        let silent = self.power < 0.02 * self.loud;
        self.quiet = if silent { self.quiet + 1 } else { 0 };
        if self.quiet as f64 > 0.012 * self.fs {
            match self.stage {
                Stage::Phase3 if self.j.is_some() && self.mode == Mode::Answer => {
                    self.end_j(now - 0.012);
                    self.stage = Stage::AwaitS4;
                    self.s_from = None;
                    self.rx.hunt();
                }
                Stage::Data => self.end_data(now - 0.012, "the signal stopped"),
                _ => {}
            }
        }
    }

    fn reversal(&mut self, now: f64, at: u64, shared: &mut Shared) {
        let baud = self.band.baud();
        let when = now - self.rx.halves().saturating_sub(at) as f64 / (2.0 * baud);
        let bar = signals::S_BAR_SYMBOLS as f64 / baud;
        let heard = self.s_from.take().unwrap_or(when - signals::S_SYMBOLS as f64 / baud);
        let from = heard.max(self.since.unwrap_or(heard)).min(when);
        if when - from < 0.01 {
            self.rx.hunt();
            return;
        }
        let layer = if self.stage == Stage::AwaitS4 { Layer::Phase4 } else { Layer::Phase3 };
        match self.stage {
            Stage::AwaitS => {
                self.rx.train(Reference::PpThenTrn, self.mode, at);
                self.stage = Stage::Training3;
            }
            Stage::AwaitS4 => {
                // TRN at the size the call modem's J asked for.
                self.size = shared.j[0].unwrap_or(Size::Four);
                self.rx.train(Reference::Trn(self.size), self.mode, at);
                self.stage = Stage::Training4;
            }
            _ => return,
        }
        self.trn_from = when + bar;
        self.events.push(self.event(from, when, layer, "S").summary(format!("S for {:.0} ms", 1000.0 * (when - from))));
        self.events.push(self.event(when, when + bar, layer, "S-bar").summary("S-bar: S turned over, 16 symbols".to_owned()));
    }

    fn end_j(&mut self, now: f64) {
        let Some(j) = self.j else { return };
        let points = std::mem::take(&mut self.seen);
        let asks = if j == Size::Sixteen { 16 } else { 4 };
        self.events.push(
            self.event(self.j_from, now, Layer::Phase3, "J")
                .summary(format!("J: asks to be trained with {asks} points in phase 4"))
                .field("asks for", format!("{asks}-point TRN and MP from the other end"))
                .qam(&points, 4),
        );
    }

    fn symbol(&mut self, now: f64, decided: Point, point: Complex, shared: &mut Shared) {
        match self.stage {
            Stage::Phase3 | Stage::Phase4 => self.training_symbol(now, decided, shared),
            Stage::Data => self.data_symbol(now, point, shared),
            Stage::Renegotiating => {
                let watched = self.s_watch.feed(point);
                // S-bar, or S long enough ago that TRN is surely under way.
                if watched == Watched::SBar || self.s_watch.since > signals::S_SYMBOLS + 32 {
                    let from = self.s_from.take().unwrap_or(now);
                    self.events.push(
                        self.event(from, now, Layer::Phase4, "S")
                            .summary("S in data mode: a rate renegotiation begins".to_owned()),
                    );
                    self.begin_trn(Size::Four, now);
                    self.stage = Stage::Phase4;
                }
            }
            _ => {}
        }
    }

    fn training_symbol(&mut self, now: f64, decided: Point, shared: &mut Shared) {
        if self.trn {
            let before = self.reader.clone();
            let bits = self.reader.trn(decided, self.size);
            if self.grace > 0 {
                self.grace -= 1;
                return;
            }
            if bits.iter().all(|b| *b) {
                return;
            }
            self.reader = before;
            self.trn = false;
            let points = std::mem::take(&mut self.seen);
            let (layer, size) = (if self.stage == Stage::Phase3 { Layer::Phase3 } else { Layer::Phase4 }, self.size);
            let count = if size == Size::Sixteen { 16 } else { 4 };
            let from = self.trn_from;
            self.events.push(
                self.event(from, now, layer, "TRN")
                    .summary(format!(
                        "TRN, {count}-point, {:.0} ms: trained a receiver to {:.0} dB",
                        1000.0 * (now - from),
                        self.trained_db
                    ))
                    .field("symbol rate", format!("{:.0} symbols/s", self.band.baud()))
                    .field("carrier", format!("{:.0} Hz", self.band.carrier()))
                    .field("signal to noise after training", format!("{:.1} dB", self.trained_db))
                    .qam(&points, count),
            );
            self.j_from = now;
        }
        for bit in self.reader.differential(decided, self.size) {
            if self.tail.len() == 32 {
                self.tail.pop_front();
            }
            self.tail.push_back(bit);
            if self.stage == Stage::Phase4 {
                match self.finder.feed(bit) {
                    Some(Found::Mp(mp)) => self.heard_mp(now, mp, shared),
                    Some(Found::E) if self.mp_found => {
                        self.heard_e(now, shared);
                        return;
                    }
                    _ => {}
                }
            }
        }
        if self.stage != Stage::Phase3 || self.tail.len() < 32 {
            return;
        }
        let older: Vec<bool> = self.tail.range(..16).copied().collect();
        let newer: Vec<bool> = self.tail.range(16..).copied().collect();
        if self.j.is_none() {
            for size in [Size::Four, Size::Sixteen] {
                if older == size.j() && newer == size.j() {
                    self.j = Some(size);
                    shared.j[self.me] = Some(size);
                }
            }
        }
        // The call modem ends its J with J' and goes straight on, with no S,
        // to TRN at the size the answer modem's J asked for.
        if let Some(j) = self.j
            && self.mode == Mode::Call
            && newer == J_PRIME
            && older == j.j()
        {
            let prime = 16.0 / (2.0 * self.band.baud());
            self.end_j(now - prime);
            self.events.push(
                self.event(now - prime, now, Layer::Phase3, "J'").summary("J': J is over, and phase 4 follows".to_owned()),
            );
            let size = shared.j[1].unwrap_or(Size::Four);
            self.rx.set_size(size);
            self.begin_trn(size, now);
            self.stage = Stage::Phase4;
        }
    }

    fn heard_mp(&mut self, now: f64, mp: Mp, shared: &mut Shared) {
        self.mp_found = true;
        shared.mp[self.me] = Some(mp);
        if let Some(coefficients) = mp.precoding {
            shared.precoding[self.me] = coefficients;
        }
        if let Some((was, index)) = &self.mp_run
            && *was == mp
        {
            let event = &mut self.events[*index];
            event.end = now;
            if let Some((_, value)) = event.fields.iter_mut().find(|(name, _)| name == "sent") {
                let n: usize = value.split(' ').next().and_then(|v| v.parse().ok()).unwrap_or(1);
                *value = format!("{} times", n + 1);
            }
            let more = std::mem::take(&mut self.seen);
            if event.symbols.len() + more.len() <= crate::model::SYMBOLS_KEPT {
                event.symbols.extend(more);
            }
            return;
        }
        let tag = if mp.acknowledge { "MP'" } else { "MP" };
        let bps = |n: u8| u32::from(n) * 2400;
        let rates: Vec<String> = (1..=14).filter(|r| mp.rates >> (r - 1) & 1 == 1).map(|r| (r * 2400).to_string()).collect();
        let precoding = mp.precoding.map_or_else(
            || "none sent".to_owned(),
            |h| h.iter().map(|(re, im)| format!("{:+.3}{:+.3}j", f64::from(*re) / 16384.0, f64::from(*im) / 16384.0)).collect::<Vec<_>>().join("  "),
        );
        let size = if self.size == Size::Sixteen { 16 } else { 4 };
        let points = std::mem::take(&mut self.seen);
        let event = self
            .event(now - 0.06, now, Layer::Phase4, tag)
            .summary(format!(
                "{tag}: at most {} bit/s from the caller, {} from the answerer",
                bps(mp.call_to_answer),
                bps(mp.answer_to_call)
            ))
            .field("most from the caller", format!("{} bit/s", bps(mp.call_to_answer)))
            .field("most from the answerer", format!("{} bit/s", bps(mp.answer_to_call)))
            .field("acknowledges the other's MP", if mp.acknowledge { "yes" } else { "no" })
            .field("trellis it wants to receive", match mp.trellis {
                Trellis::States16 => "16 states",
                Trellis::States32 => "32 states",
                Trellis::States64 => "64 states",
            })
            .field("non-linear encoding", if mp.non_linear { "yes" } else { "no" })
            .field("shaping", if mp.expanded_shaping { "expanded" } else { "minimum" })
            .field("precoding h(1) h(2) h(3)", precoding)
            .field("rates enabled", rates.join(", "))
            .field("different rates each way", if mp.asymmetric { "allowed" } else { "not allowed" })
            .field("auxiliary channel", if mp.auxiliary { "yes" } else { "no" })
            .field("sent", "1 times")
            .qam(&points, size);
        self.events.push(event);
        self.mp_run = Some((mp, self.events.len() - 1));
    }

    fn heard_e(&mut self, now: f64, shared: &mut Shared) {
        let e = signals::E_BITS as f64 / (self.size.bits() as f64 * self.band.baud());
        self.events
            .push(self.event(now - e, now, Layer::Phase4, "E").summary("E: twenty ones, and data mode follows".to_owned()));
        self.mp_run = None;
        self.seen.clear();
        self.stage = Stage::Data;
        self.s_watch = SWatch::default();
        self.bad = 0;
        self.decoder = None;
        self.acquirer = None;
        // The data this modem sends is laid out as the other end asked.
        let (call, answer) = (shared.mp[0], shared.mp[1]);
        let theirs = shared.mp[1 - self.me];
        let params = match (call, answer, theirs) {
            (Some(call), Some(answer), Some(theirs)) => {
                let (up, down) = negotiate(&call, &answer);
                let rate = if self.me == 0 { up } else { down };
                Framing::new(self.rate, u32::from(rate) * 2400, false, theirs.expanded_shaping).map(|framing| Params {
                    framing,
                    code: match theirs.trellis {
                        Trellis::States16 => Code::States16,
                        Trellis::States32 => Code::States32,
                        Trellis::States64 => Code::States64,
                    },
                    nonlinear: theirs.non_linear,
                    precoding: shared.precoding[1 - self.me],
                    mode: self.mode,
                })
            }
            _ => None,
        };
        self.params = params;
        let mut data = self.event(now, now, Layer::Data, "data");
        match params {
            Some(params) => {
                let decoder = Decoder::new(params);
                self.rx.set_grid(decoder.grid_scale(), decoder.extent());
                self.b1_left = params.framing.n;
                self.slips = self.rx.slips();
                self.stream.bps = Some(params.framing.primary);
                data = data
                    .summary(format!("data mode at {} bit/s", params.framing.primary))
                    .field("rate", format!("{} bit/s", params.framing.primary))
                    .field("symbol rate", format!("{:.0} symbols/s", self.band.baud()))
                    .field("points in the constellation", params.framing.l.to_string())
                    .field("trellis", format!("{} states", params.code.states()))
                    .field("non-linear encoding", if params.nonlinear { "yes" } else { "no" })
                    .field("shaping", if params.framing.expanded { "expanded" } else { "minimum" });
                self.decoder = Some(decoder);
            }
            None => {
                data = data.summary("data mode, but the other end's MP was not read: symbols only".to_owned());
            }
        }
        self.events.push(data);
        self.data_event = Some(self.events.len() - 1);
    }

    fn data_symbol(&mut self, now: f64, point: Complex, shared: &mut Shared) {
        if self.s_watch.feed(point) == Watched::S {
            // The far end has turned to S in the middle of data.
            self.renegotiations += 1;
            self.end_data(now, "S: a rate renegotiation");
            self.stage = Stage::Renegotiating;
            self.s_from = Some(now - 0.008);
            self.rx.set_size(Size::Four);
            self.reader = Reader::new(self.mode);
            self.finder = Finder::new();
            self.mp_found = false;
            shared.mp[self.me] = None;
            return;
        }
        if self.rx.slips() != self.slips {
            // A slip loses or repeats symbols, and with them the place in
            // the frames: look for it again.
            self.slips = self.rx.slips();
            if let Some(params) = self.params {
                let acquirer = Acquirer::new(params);
                self.rx.set_grid(acquirer.grid_scale(), acquirer.extent());
                self.acquirer = Some(acquirer);
                self.decoder = None;
            }
        }
        if let Some(acquirer) = self.acquirer.as_mut() {
            match acquirer.feed(point) {
                Acquired::Found(decoder) => {
                    self.acquirer = None;
                    self.b1_left = 0;
                    self.decoder = Some(*decoder);
                }
                Acquired::Nothing => self.end_data(now, "the framing was lost and not found again"),
                Acquired::Searching => {}
            }
            return;
        }
        let Some(decoder) = self.decoder.as_mut() else {
            if let Some(index) = self.data_event {
                self.events[index].end = now;
            }
            return;
        };
        decoder.feed(point);
        for bit in decoder.take_bits() {
            if self.b1_left > 0 {
                self.b1_left -= 1;
                continue;
            }
            if self.stream.bits.len().is_multiple_of(256) {
                self.stream.marks.push((self.stream.bits.len(), now));
            }
            self.stream.bits.push(bit);
        }
        // The trellis pays a little for noise and a great deal for a signal
        // that is not this code at all: a retrain's tones, or silence.
        self.bad = if decoder.path_cost() > 4.0 { self.bad + 1 } else { 0 };
        if let Some(index) = self.data_event {
            self.events[index].end = now;
        }
        if self.bad > 1500 {
            self.end_data(now - 1500.0 / self.band.baud(), "what followed was not this data mode");
        }
    }

    fn end_data(&mut self, now: f64, why: &str) {
        if let Some(index) = self.data_event.take() {
            let points = std::mem::take(&mut self.seen);
            let count = self.params.map_or(1024, |p| p.framing.l);
            let event = &mut self.events[index];
            event.end = now.max(event.start);
            event.fields.push(("ended".to_owned(), why.to_owned()));
            let done = event.clone().qam(&points, count);
            self.events[index] = done;
        }
        self.stream.marks.push((self.stream.bits.len(), now));
        self.decoder = None;
        self.acquirer = None;
        if self.stage == Stage::Data {
            self.stage = Stage::Over;
        }
    }
}

/// What following a call came to.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// The calling modem's data, and the answering modem's.
    pub streams: [Stream; 2],
    pub renegotiations: usize,
}

/// Follow a V.34 call from the end of a phase 2 until `until` seconds.
/// `call` and `answer` are the two modems' own transmissions.
pub fn follow(
    call: (&[f32], End),
    answer: (&[f32], End),
    fs: f64,
    settled: &Settled,
    until: f64,
) -> Option<(Vec<Event>, Outcome)> {
    let info1a = settled.info1a?;
    let info1c = settled.info1c?;
    let from = settled.ended?;
    // Each end's carrier is the one the other end's probing chose for it.
    let down = info1a.answer_to_call;
    let up = info1a.call_to_answer;
    let answer_band = Band::new(down, info1c.probed[down.index() as usize].high_carrier);
    let call_band = Band::new(up, info1a.probed.high_carrier);
    let mut shared = Shared::default();
    let mut caller = Direction::new(call_band, up, Mode::Call, call.1, fs);
    let mut answerer = Direction::new(answer_band, down, Mode::Answer, answer.1, fs);
    let first = ((from + 0.01) * fs) as usize;
    let last = call.0.len().min(answer.0.len()).min((until * fs) as usize);
    for n in first..last {
        caller.feed(n, call.0[n] as f64, &mut shared);
        answerer.feed(n, answer.0[n] as f64, &mut shared);
    }
    let end = last as f64 / fs;
    for direction in [&mut caller, &mut answerer] {
        if direction.stage == Stage::Data {
            direction.end_data(end, "the recording, or this stretch of it, ended");
        }
    }
    let renegotiations = caller.renegotiations.max(answerer.renegotiations);
    let mut events = caller.events;
    events.extend(answerer.events);
    Some((events, Outcome { streams: [caller.stream, answerer.stream], renegotiations }))
}
