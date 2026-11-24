//! What a review of a recording is made of.
//!
//! A recording is read once, start to finish, and comes out as a list of
//! things that were on the line: each with the time it started and ended,
//! which end sent it, a few letters to write on the waterfall, and whatever
//! could be read out of it. Nothing here knows how any of it was found.

/// Which end of the call something came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum End {
    /// The modem that placed the call.
    Caller,
    /// The modem that answered it.
    Answerer,
    /// Heard on a tap that carries both and not attributable to either.
    Line,
}

impl End {
    pub fn label(self) -> &'static str {
        match self {
            Self::Caller => "caller",
            Self::Answerer => "answerer",
            Self::Line => "line",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Caller => Self::Answerer,
            Self::Answerer => Self::Caller,
            Self::Line => Self::Line,
        }
    }
}

/// What part of the call a thing belongs to, coarsest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    /// Tones of the network and of call set-up: dial tone, ANS, CNG.
    Tone,
    /// V.8 bis, before the answer tone.
    V8bis,
    /// V.8: CI, CM, JM, CJ.
    V8,
    /// A start-up of the older modulations: V.21 to V.32bis.
    Startup,
    /// Phase 2 of V.34 and its descendants: INFO, the tones, the probing.
    Phase2,
    /// Phase 3: equaliser and echo canceller training.
    Phase3,
    /// Phase 4: final training and the exchange of parameters.
    Phase4,
    /// Data mode itself.
    Data,
    /// What the error control said: V.42 detection, XID, LAPM frames.
    Link,
    /// What was carried: PPP and above, or plain characters.
    Payload,
    /// Fax: T.30 frames and pages.
    Fax,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tone => "tone",
            Self::V8bis => "V.8 bis",
            Self::V8 => "V.8",
            Self::Startup => "start-up",
            Self::Phase2 => "phase 2",
            Self::Phase3 => "phase 3",
            Self::Phase4 => "phase 4",
            Self::Data => "data",
            Self::Link => "V.42",
            Self::Payload => "payload",
            Self::Fax => "fax",
        }
    }
}

/// What the symbols of an event are, for whoever draws them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Plot {
    /// No symbols were kept.
    #[default]
    None,
    /// Points of a constellation, in phase and quadrature, of this many
    /// points; the constellation in use has unit mean power.
    Qam(usize),
    /// PCM: levels and no plane. Each symbol is paired with the one before
    /// it, as (before, level), on the scale where full scale is one.
    Pcm,
    /// Frequency shift keying: one reading a bit, as (0, margin), where one
    /// and minus one are the two tones and zero is the threshold.
    Fsk,
    /// A tone, as where its phasor stood every few milliseconds, the
    /// loudest at a radius of one: a steady tone is a dot or a ring, a
    /// phase reversal is a jump across the middle.
    Phasor,
}

/// The most symbols an event keeps for drawing.
pub const SYMBOLS_KEPT: usize = 20_000;

/// One thing that was on the line.
#[derive(Debug, Clone)]
pub struct Event {
    /// Seconds from the start of the recording.
    pub start: f64,
    pub end: f64,
    pub from: End,
    pub layer: Layer,
    /// A few letters for the waterfall: "CM", "INFO0c", "DIL".
    pub tag: String,
    /// One line for a list.
    pub summary: String,
    /// Everything read out of it, name and value, in the order to show it.
    pub fields: Vec<(String, String)>,
    /// The octets or bits as recovered, where there were any.
    pub octets: Vec<u8>,
    /// The band it occupied in hertz, for the box drawn round it; None where
    /// it filled the channel.
    pub band: Option<(f64, f64)>,
    /// How loud it was, as an amplitude on the scale of the samples; zero
    /// where nobody measured.
    pub level: f64,
    /// The symbols a receiver made of it, where one followed it: at most
    /// [`SYMBOLS_KEPT`], evenly taken from however many there were.
    pub symbols: Vec<(f32, f32)>,
    pub plot: Plot,
}

impl Event {
    pub fn new(start: f64, end: f64, from: End, layer: Layer, tag: impl Into<String>) -> Self {
        Self {
            start,
            end: end.max(start),
            from,
            layer,
            tag: tag.into(),
            summary: String::new(),
            fields: Vec::new(),
            octets: Vec::new(),
            band: None,
            level: 0.0,
            symbols: Vec::new(),
            plot: Plot::None,
        }
    }

    pub fn summary(mut self, text: impl Into<String>) -> Self {
        self.summary = text.into();
        self
    }

    pub fn field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((name.into(), value.into()));
        self
    }

    pub fn fields(mut self, more: impl IntoIterator<Item = (String, String)>) -> Self {
        self.fields.extend(more);
        self
    }

    pub fn octets(mut self, octets: &[u8]) -> Self {
        self.octets = octets.to_vec();
        self
    }

    pub fn band(mut self, low: f64, high: f64) -> Self {
        self.band = Some((low.min(high), low.max(high)));
        self
    }

    /// Constellation points, of a constellation of `points` points.
    pub fn qam(mut self, symbols: &[(f32, f32)], points: usize) -> Self {
        self.symbols = thinned(symbols);
        self.plot = if self.symbols.is_empty() { Plot::None } else { Plot::Qam(points) };
        self
    }

    /// PCM levels, in the order they came.
    pub fn pcm(mut self, levels: &[f32]) -> Self {
        let pairs: Vec<(f32, f32)> = levels.windows(2).map(|w| (w[0], w[1])).collect();
        self.symbols = thinned(&pairs);
        self.plot = if self.symbols.is_empty() { Plot::None } else { Plot::Pcm };
        self
    }

    /// The discriminator's reading at the middle of each bit.
    pub fn fsk(mut self, margins: &[f32]) -> Self {
        let pairs: Vec<(f32, f32)> = margins.iter().map(|m| (0.0, *m)).collect();
        self.symbols = thinned(&pairs);
        self.plot = if self.symbols.is_empty() { Plot::None } else { Plot::Fsk };
        self
    }

    /// A tone's phasor, sampled; scaled here so the loudest is at one.
    pub fn phasor(mut self, points: &[(f32, f32)]) -> Self {
        let peak = points.iter().map(|(re, im)| re.hypot(*im)).fold(0.0f32, f32::max);
        if peak > 0.0 {
            let scaled: Vec<(f32, f32)> = points.iter().map(|(re, im)| (re / peak, im / peak)).collect();
            self.symbols = thinned(&scaled);
            self.plot = Plot::Phasor;
        }
        self
    }

    pub fn level(mut self, amplitude: f64) -> Self {
        self.level = amplitude;
        self
    }

    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

/// At most [`SYMBOLS_KEPT`] of them, taken evenly.
fn thinned(symbols: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if symbols.len() <= SYMBOLS_KEPT {
        return symbols.to_vec();
    }
    (0..SYMBOLS_KEPT).map(|i| symbols[i * symbols.len() / SYMBOLS_KEPT]).collect()
}

/// How a recording's channels relate to the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// One channel with both directions added together: a two-wire tap, or a
    /// microphone at a modem's speaker.
    Summed,
    /// One direction a channel. `caller` is the channel the calling modem's
    /// transmission is on.
    Split { caller: usize },
}

/// One channel's picture: power against time and frequency.
#[derive(Debug, Clone)]
pub struct Waterfall {
    /// Columns a second.
    pub columns_per_sec: f64,
    /// Rows a column, from 0 Hz up.
    pub bins: usize,
    /// Hertz a row.
    pub hz_per_bin: f64,
    /// Column after column, each `bins` long; 0 is `FLOOR_DB` and 255 is 0 dBFS.
    pub cells: Vec<u8>,
}

impl Waterfall {
    /// What a cell of zero stands for.
    pub const FLOOR_DB: f64 = -96.0;

    pub fn columns(&self) -> usize {
        self.cells.len().checked_div(self.bins).unwrap_or(0)
    }

    pub fn column(&self, index: usize) -> &[u8] {
        &self.cells[index * self.bins..(index + 1) * self.bins]
    }
}

/// A level against time, for the strip under a waterfall.
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub points_per_sec: f64,
    /// dBFS (of a full-scale sine), one a point.
    pub db: Vec<f32>,
}

/// One channel of the recording.
#[derive(Debug, Clone)]
pub struct Channel {
    /// Whose transmission it is, or `Line` for a summed tap.
    pub from: End,
    pub waterfall: Waterfall,
    pub level: Track,
    /// The channel itself, for playing.
    pub samples: Vec<f32>,
}

/// Everything found in one recording.
#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    pub sample_rate: u32,
    pub seconds: f64,
    pub tap: Tap,
    pub channels: Vec<Channel>,
    /// In order of starting time.
    pub events: Vec<Event>,
    /// The call in a dozen lines: what it was, what it agreed, how fast.
    pub verdict: Vec<(String, String)>,
    /// What could not be read and why, for the reader to weigh the rest by.
    pub caveats: Vec<String>,
}

impl Report {
    /// The verdict line of that name, if the call got that far.
    pub fn verdict(&self, name: &str) -> Option<&str> {
        self.verdict.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// The first event with that tag.
    pub fn find(&self, tag: &str) -> Option<&Event> {
        self.events.iter().find(|e| e.tag == tag)
    }

    /// As text, a line an event: what the command line prints.
    pub fn text(&self, with_fields: bool) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let tap = match self.tap {
            Tap::Summed => "both ends on one channel".to_owned(),
            Tap::Split { caller } => format!("an end a channel, caller on {}", caller + 1),
        };
        let _ = writeln!(out, "{}: {:.1} s at {} Hz, {tap}", self.name, self.seconds, self.sample_rate);
        for (name, value) in &self.verdict {
            let _ = writeln!(out, "  {name:<22} {value}");
        }
        for caveat in &self.caveats {
            let _ = writeln!(out, "  note: {caveat}");
        }
        let _ = writeln!(out);
        for e in &self.events {
            let _ = writeln!(
                out,
                "{:8.3} {:7.3}  {:<8} {:<8} {:<10} {}",
                e.start,
                e.duration(),
                e.from.label(),
                e.layer.label(),
                e.tag,
                e.summary
            );
            if with_fields {
                for (name, value) in &e.fields {
                    let _ = writeln!(out, "{:>38}{name}: {value}", "");
                }
            }
        }
        out
    }
}
