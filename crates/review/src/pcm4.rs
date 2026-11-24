//! Phase 4 and data mode of a V.90 call as the digital modem sends them:
//! Ri and its turn, TRN2d, MP, Ed, B1d and data.
//!
//! This is the analogue modem's side of phase 4 with the modem taken out.
//! The analogue modem reads these frames against the constellations it
//! chose itself and the levels its own DIL showed it. A tap chose nothing,
//! so the constellations are the ones it read in the analogue modem's CPt
//! and CP on the other channel, and the levels are the ones the DIL had on
//! this one, by the same receiver that is now reading the frames.

use datapump::v32::{Mode, Scrambler};
use datapump::v34::mp::{Finder, Found, Mp};
use datapump::v90::INTERVALS;
use datapump::v90::dil::Route;
use datapump::v90::encoder::{Decoder, Frame, Mapping};
use datapump::v90::pcm::{Receiver, Slicer, Symbol};
use datapump::v90::sequences::{self, Cp};

use crate::model::{End, Event, Layer};
use crate::pcm::Shared;
use crate::v34::{Stream, mp_fields};

/// Frames of R in a row before it is R, and of R out of place before the
/// frames are taken to have moved.
const R_HEARD: usize = 8;
const R_MOVED: usize = 6;
/// B1d is 48 data frames (8.6.1).
const B1D_FRAMES: usize = 48;
/// A data frame, in seconds.
const FRAME: f64 = INTERVALS as f64 / 8000.0;
/// Symbols held for one event.
const HELD: usize = 200_000;

/// Watches for R and its turn to R-bar (8.6.4): "+ + + - - -" at one level,
/// frame after frame, and then the same moved by three.
#[derive(Debug, Clone, Default)]
struct RWatch {
    frame: [f64; INTERVALS],
    run: usize,
    moved: usize,
    heard: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RSeen {
    Nothing,
    /// R has turned into R-bar: TRN2d begins at this symbol.
    Turned(u64),
    /// R is arriving this many symbols late: the frames are somewhere else.
    Moved(usize),
}

impl RWatch {
    fn feed(&mut self, symbol: &Symbol, level: f64) -> RSeen {
        let i = symbol.interval();
        self.frame[i] = symbol.value;
        if i != INTERVALS - 1 {
            return RSeen::Nothing;
        }
        let late = (0..INTERVALS).find(|&m| {
            (0..INTERVALS).all(|j| {
                let k = (j + INTERVALS - m) % INTERVALS;
                let v = self.frame[j];
                (v >= 0.0) == (k < 3) && (0.5 * level..1.5 * level).contains(&v.abs())
            })
        });
        match late {
            Some(0) => {
                self.run = if self.moved == 0 { self.run + 1 } else { 1 };
                self.moved = 0;
                if self.run >= R_HEARD {
                    self.heard = true;
                }
                RSeen::Nothing
            }
            Some(3) if self.heard => {
                // R-bar's first frame. TRN2d starts after the other three.
                let frame_start = symbol.index + 1 - INTERVALS as u64;
                RSeen::Turned(frame_start + 4 * INTERVALS as u64)
            }
            Some(m) if m != 3 => {
                self.run = if self.moved == m { self.run + 1 } else { 1 };
                self.moved = m;
                if self.run < R_MOVED {
                    return RSeen::Nothing;
                }
                self.run = 0;
                self.moved = 0;
                RSeen::Moved(m)
            }
            _ => {
                if !self.heard {
                    self.run = 0;
                }
                RSeen::Nothing
            }
        }
    }
}

/// Signed levels of each interval's constellation, as the route delivers
/// them: (level, Ucode, positive).
type Levels = [Vec<(f64, u8, bool)>; INTERVALS];

fn levels_for(cp: &Cp, route: &Route) -> Levels {
    std::array::from_fn(|i| {
        cp.points(i)
            .into_iter()
            .flat_map(|u| {
                let level = route.levels[i][usize::from(u)];
                [(level, u, true), (-level, u, false)]
            })
            .collect()
    })
}

fn slicer_for(levels: &Levels) -> Slicer {
    Slicer::Levels(Box::new(std::array::from_fn(|i| {
        let mut signed: Vec<f64> = levels[i].iter().map(|l| l.0).collect();
        signed.sort_by(|a, b| b.total_cmp(a));
        signed
    })))
}

fn nearest(levels: &[(f64, u8, bool)], value: f64) -> (u8, bool) {
    levels
        .iter()
        .min_by(|a, b| (a.0 - value).abs().total_cmp(&(b.0 - value).abs()))
        .map_or((0, false), |l| (l.1, l.2))
}

/// The frames from TRN2d on.
struct Frames {
    from: u64,
    started: f64,
    decoder: Decoder,
    levels: Levels,
    frame: [(u8, bool); INTERVALS],
    descrambler: Scrambler,
    finder: Finder,
    mp: Option<Mp>,
    mp_run: Option<(Mp, usize)>,
    /// Frames read since TRN2d began.
    read: usize,
    trn_over: bool,
    zero_frames: usize,
    ed: bool,
    b1d_left: usize,
    b1d_from: f64,
    /// Data mode's event, once it has begun.
    data: Option<usize>,
    last_frame: Option<Frame>,
    /// Frames lately that this mapping could not have made.
    bad: usize,
}

/// The digital modem's transmission after the DIL.
pub(crate) struct Fourth {
    who: End,
    level: f64,
    route: Route,
    r: RWatch,
    ri_from: Option<f64>,
    began: f64,
    frames: Option<Frames>,
    /// Why the frames could not be read, if they could not.
    failed: Option<String>,
    over: bool,
    seen: Vec<f32>,
    pub(crate) events: Vec<Event>,
    pub(crate) stream: Stream,
}

impl Fourth {
    pub(crate) fn new(who: End, level: f64, route: Route, now: f64) -> Self {
        Self {
            who,
            level,
            route,
            r: RWatch::default(),
            ri_from: None,
            began: now,
            frames: None,
            failed: None,
            over: false,
            seen: Vec::new(),
            events: Vec::new(),
            stream: Stream {
                from: Some(who),
                ..Stream::default()
            },
        }
    }

    fn event(&self, start: f64, end: f64, layer: Layer, tag: &str) -> Event {
        Event::new(start, end.max(start), self.who, layer, tag)
    }

    pub(crate) fn symbol(
        &mut self,
        now: f64,
        symbol: &Symbol,
        rx: &mut Receiver,
        shared: &mut Shared,
    ) {
        if self.over {
            return;
        }
        if self.seen.len() < HELD {
            self.seen.push(symbol.value as f32);
        }
        if self.frames.is_none() {
            if self.failed.is_some() {
                return;
            }
            let was_heard = self.r.heard;
            match self.r.feed(symbol, self.level) {
                RSeen::Moved(late) => {
                    let offset = (rx.frame_offset() + (INTERVALS - late) as u64) % INTERVALS as u64;
                    rx.set_frame_offset(offset);
                }
                RSeen::Turned(from) => self.turned(now, from, shared),
                RSeen::Nothing => {
                    if self.r.heard && !was_heard {
                        rx.set_slicer(Slicer::Binary(self.level));
                        let from = now - R_HEARD as f64 * FRAME;
                        self.ri_from = Some(from);
                        // What came before R is the tail of the DIL.
                        let keep = (R_HEARD * INTERVALS).min(self.seen.len());
                        self.seen.drain(..self.seen.len() - keep);
                    }
                }
            }
            return;
        }
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        if symbol.index + 1 == frames.from {
            // TRN2d's first symbol is next: decide against CPt's levels.
            rx.set_slicer(slicer_for(&frames.levels));
            frames.started = now;
            self.seen.clear();
            return;
        }
        if symbol.index < frames.from {
            return;
        }
        let i = symbol.interval();
        frames.frame[i] = nearest(&frames.levels[i], symbol.value);
        if i != INTERVALS - 1 {
            return;
        }
        let frame = Frame {
            ucodes: std::array::from_fn(|k| frames.frame[k].0),
            positive: std::array::from_fn(|k| frames.frame[k].1),
        };
        frames.read += 1;
        let possible = frames.decoder.could_have_sent(&frame);
        frames.bad = if possible {
            frames.bad.saturating_sub(1)
        } else {
            frames.bad + 2
        };
        // A scrambler does not give the same frame twice running; a line
        // that has stopped changing gives nothing else.
        let repeated = frames.last_frame.replace(frame) == Some(frame);
        let bits: Vec<bool> = frames
            .decoder
            .frame(frame)
            .into_iter()
            .map(|b| frames.descrambler.descramble(b))
            .collect();

        if let Some(index) = frames.data {
            if self.stream.bits.len() / 256 != (self.stream.bits.len() + bits.len()) / 256
                || self.stream.marks.is_empty()
            {
                self.stream.marks.push((self.stream.bits.len(), now));
            }
            self.stream.bits.extend(bits);
            self.events[index].end = now;
            if frames.bad > 120 {
                self.end_data(
                    now,
                    "the frames stopped being ones this constellation could make",
                );
            }
            return;
        }
        if frames.ed {
            // B1d: 48 frames of scrambled ones, and then data.
            if repeated {
                return;
            }
            frames.b1d_left -= 1;
            if frames.b1d_left > 0 {
                return;
            }
            let from = frames.b1d_from;
            let rate = shared
                .cp
                .as_ref()
                .and_then(|cp| sequences::data_rate(cp.drn));
            frames.bad = 0;
            let levels = std::mem::take(&mut self.seen);
            self.events.push(
                self.event(from, now, Layer::Phase4, "B1d")
                    .summary("B1d: 48 frames of ones at data mode's constellation".to_owned())
                    .pcm(&levels),
            );
            let mut data = self.event(now, now, Layer::Data, "data");
            data = match rate {
                Some(rate) => {
                    self.stream.bps = Some(rate);
                    data.summary(format!("data mode, PCM at {rate} bit/s"))
                        .field("rate", format!("{rate} bit/s"))
                }
                None => data.summary("data mode, PCM".to_owned()),
            };
            if let Some(cp) = &shared.cp {
                let sizes: Vec<String> = (0..INTERVALS)
                    .map(|i| cp.points(i).len().to_string())
                    .collect();
                data = data
                    .field("points in each interval", sizes.join(" "))
                    .field("sign redundancy Sr", format!("{:?}", cp.redundancy))
                    .field("bits a frame", cp.frame_bits().to_string());
            }
            self.events.push(data);
            if let Some(frames) = self.frames.as_mut() {
                frames.data = Some(self.events.len() - 1);
            }
            shared.down_data = Some(now);
            return;
        }
        if !frames.trn_over {
            // TRN2d is ones, once the descrambler has filled.
            if frames.read <= 2 || bits.iter().all(|b| *b) {
                return;
            }
            frames.trn_over = true;
            let from = frames.started;
            let levels = std::mem::take(&mut self.seen);
            self.events.push(
                self.event(from, now - FRAME, Layer::Phase4, "TRN2d")
                    .summary(format!(
                        "TRN2d, {:.0} ms: ones at the training constellation CPt asked for",
                        1000.0 * (now - from)
                    ))
                    .pcm(&levels),
            );
        }
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        // Ed is two frames of scrambled zeros (8.6.2). It comes after the
        // digital modem has acknowledged a CP with an MP', so not before
        // either has been read; and a frame the constellation could not
        // have made is a frame read wrong, whatever it decodes to.
        let zeros = bits.iter().all(|b| !*b);
        let due = frames.mp.is_some_and(|mp| mp.acknowledge) && shared.cp.is_some();
        frames.zero_frames = if zeros && !repeated && possible && due {
            frames.zero_frames + 1
        } else {
            0
        };
        if frames.zero_frames == 2 {
            frames.ed = true;
            frames.b1d_left = B1D_FRAMES;
            frames.b1d_from = now;
            frames.mp_run = None;
            // B1d and data are at data mode's constellation, the coding
            // started afresh.
            let data = shared
                .cp
                .as_ref()
                .and_then(|cp| Mapping::from_cp(cp).map(|m| (m, cp.clone())));
            self.seen.clear();
            self.events.push(
                Event::new(now - 2.0 * FRAME, now, self.who, Layer::Phase4, "Ed")
                    .summary("Ed: two frames of zeros, and B1d follows".to_owned()),
            );
            match data {
                Some((mapping, cp)) => {
                    frames.decoder = Decoder::new(mapping);
                    frames.levels = levels_for(&cp, &self.route);
                    rx.set_slicer(slicer_for(&frames.levels));
                }
                None => {
                    self.failed = Some(
                        "no CP was read from the analogue modem to read data mode by".to_owned(),
                    );
                    self.frames = None;
                }
            }
            return;
        }
        for bit in bits {
            if let Some(Found::Mp(mp)) = frames.finder.feed(bit) {
                frames.mp = Some(mp);
                shared.mp = Some(mp);
                if let Some((was, index)) = &frames.mp_run
                    && *was == mp
                {
                    let event = &mut self.events[*index];
                    event.end = now;
                    if let Some((_, value)) =
                        event.fields.iter_mut().find(|(name, _)| name == "sent")
                    {
                        let n: usize = value
                            .split(' ')
                            .next()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1);
                        *value = format!("{} times", n + 1);
                    }
                    continue;
                }
                let tag = if mp.acknowledge { "MP'" } else { "MP" };
                let levels = std::mem::take(&mut self.seen);
                let event = Event::new(now - 0.03, now, self.who, Layer::Phase4, tag)
                    .summary(format!(
                        "{tag}: at most {} bit/s upstream",
                        u32::from(mp.answer_to_call) * 2400
                    ))
                    .fields(mp_fields(&mp))
                    .field("sent", "1 times")
                    .pcm(&levels);
                self.events.push(event);
                frames.mp_run = Some((mp, self.events.len() - 1));
            }
        }
    }

    fn turned(&mut self, now: f64, from: u64, shared: &Shared) {
        let ri_from = self.ri_from.unwrap_or(now - R_HEARD as f64 * FRAME);
        let turn = now - FRAME;
        let levels = std::mem::take(&mut self.seen);
        self.events.push(
            self.event(ri_from, turn, Layer::Phase4, "Ri")
                .summary(format!(
                    "Ri for {:.0} ms: + + + - - - at the reference level",
                    1000.0 * (turn - ri_from)
                ))
                .pcm(&levels),
        );
        self.events.push(
            self.event(turn, turn + 4.0 * FRAME, Layer::Phase4, "R-bar-i")
                .summary("R-bar-i: Ri turned over, 24 symbols".to_owned()),
        );
        let training = shared
            .cpt
            .as_ref()
            .and_then(|cpt| Mapping::from_cp(cpt).map(|m| (m, cpt.clone())));
        match training {
            Some((mapping, cpt)) => {
                self.frames = Some(Frames {
                    from,
                    started: now,
                    decoder: Decoder::new(mapping),
                    levels: levels_for(&cpt, &self.route),
                    frame: [(0, false); INTERVALS],
                    descrambler: Scrambler::new(Mode::Call),
                    finder: Finder::new(),
                    mp: None,
                    mp_run: None,
                    read: 0,
                    trn_over: false,
                    zero_frames: 0,
                    ed: false,
                    b1d_left: 0,
                    b1d_from: now,
                    data: None,
                    last_frame: None,
                    bad: 0,
                });
            }
            None => {
                self.failed =
                    Some("no CPt was read from the analogue modem to read phase 4 by".to_owned())
            }
        }
    }

    fn end_data(&mut self, now: f64, why: &str) {
        self.over = true;
        self.stream.marks.push((self.stream.bits.len(), now));
        let Some(index) = self.frames.as_ref().and_then(|f| f.data) else {
            return;
        };
        let levels = std::mem::take(&mut self.seen);
        let event = &mut self.events[index];
        event.end = now.max(event.start);
        event.fields.push(("ended".to_owned(), why.to_owned()));
        let done = event.clone().pcm(&levels);
        self.events[index] = done;
    }

    /// What is left when the recording ends.
    pub(crate) fn finish(&mut self, now: f64) {
        if self.over {
            return;
        }
        if self.frames.as_ref().is_some_and(|f| f.data.is_some()) {
            self.end_data(now, "the recording, or this stretch of it, ended");
            return;
        }
        // Whatever was not read is still there to be looked at.
        let levels = std::mem::take(&mut self.seen);
        let (from, what) = match (&self.frames, &self.failed) {
            (Some(frames), _) => (
                frames.started,
                "phase 4 from the digital modem, which did not reach data mode on the recording"
                    .to_owned(),
            ),
            (None, Some(why)) => (
                self.began,
                format!("phase 4 from the digital modem, symbols only: {why}"),
            ),
            (None, None) => (self.began, "after the DIL: no Ri was found".to_owned()),
        };
        if !levels.is_empty() {
            self.events.push(
                self.event(from, now, Layer::Phase4, "phase 4")
                    .summary(what)
                    .pcm(&levels),
            );
        }
    }
}
