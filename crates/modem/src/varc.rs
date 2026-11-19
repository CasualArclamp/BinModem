//! A V.ARC call, as the modem sees one.
//!
//! Every other call this modem makes starts with a negotiation, because the
//! far end could be anything. A V.ARC call has nothing to negotiate: the
//! far end is another of these on the other end of a lead, or it is nothing.
//! `ATD` and `ATA` still mean what they mean, since the two ends have to
//! differ in one respect (see [`varc::dmt::Plan::coloured`]) and "the one
//! that dialled" is as good a way to tell them apart as any.
//!
//! The awkward part is the line. The rest of the modem runs at one rate on
//! one channel, and a sound card is converted to suit. V.ARC is built on the
//! card as it is, both channels at its own rate, so a V.ARC call is stepped
//! through its own door, [`crate::Modem::varc_step`], by whatever owns the
//! sound card, with a frame of the card's own samples at a time. The modem
//! says when it wants that, with [`crate::Modem::varc_wanted`].
//!
//! The other line it runs on is a SIP call that agreed on linear samples,
//! where there is no sound card at all and every sample arrives as the
//! sixteen bits that were sent. There the call starts with no modulation
//! whatever ([`varc::exact`]), and if a few seconds show that the path is
//! not exact after all it carries on as the multicarrier mode at the
//! call's 16 kHz, which wants only that the path be clean.

use varc::dmt::{self, Plan};
use varc::exact;
use varc::link::{Receiver, Report, Sender};
use varc::sc::{self, Shape};
use varc::station::Station;

use crate::Role;

/// Which of V.ARC's modulations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarcMode {
    /// Multicarrier, loaded a carrier at a time.
    Dmt,
    /// One carrier at 16 000 baud.
    Qam,
    /// Levels at baseband, 16 000 baud.
    Pcm,
    /// No modulation: the samples of a linear call are the data. Not one
    /// `+MS` can ask for; it is what `VARC` becomes on a line that carries
    /// samples exactly.
    Linear,
}

impl VarcMode {
    /// The mode `+MS` names, if it names one of these.
    pub fn from_carrier(carrier: &str) -> Option<Self> {
        match carrier {
            "VARC" => Some(Self::Dmt),
            "VARCQ" => Some(Self::Qam),
            "VARCP" => Some(Self::Pcm),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dmt => "V.ARC/DMT",
            Self::Qam => "V.ARC/QAM",
            Self::Pcm => "V.ARC/PCM",
            Self::Linear => "V.ARC/L16",
        }
    }
}

type End = Station<Box<dyn Sender>, Box<dyn Receiver>>;

/// How often a call's line is looked over: every five milliseconds.
const LOOKS_A_SECOND: f64 = 200.0;

/// How long a line that should carry samples exactly is given to show that
/// it does, in seconds: this long with nothing heard at all, or the longer
/// with something heard and the two ends still not agreed.
const EXACT_UNHEARD: f64 = 4.0;
const EXACT_UNAGREED: f64 = 10.0;

/// One V.ARC call.
pub(crate) struct Call {
    pub mode: VarcMode,
    /// The known points this end sends with and listens for, a set a
    /// channel, counted from these.
    sends: u8,
    hears: u8,
    station: Option<End>,
    fs: f64,
    /// The rate it sends at, which is the other sound device's.
    fs_out: f64,
    /// The trim, over all channels, that whoever is watching was last told.
    trim_said: f64,
    samples: u64,
    /// Samples since any channel last heard the far end.
    deaf: u64,
    /// When the first channel came up loaded, in samples.
    first_loaded: Option<u64>,
    /// Samples until the line is next looked over.
    until_look: u32,
    /// The line carries samples exactly: a linear SIP call, not a card.
    exact_line: bool,
    /// And was found not to after all, so that it is not tried again.
    inexact: bool,
    /// Something worth telling whoever is watching, once.
    note: Option<String>,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call").field("mode", &self.mode).field("fs", &self.fs).finish_non_exhaustive()
    }
}

impl Call {
    /// A call as the end that dialled or the end that answered. `looped` is
    /// a lead that comes straight back: this end hears itself and nothing
    /// else, so it listens for what it sends.
    pub fn new(mode: VarcMode, role: Role, looped: bool) -> Self {
        let (sends, hears) = match (looped, role) {
            (true, _) => (0, 0),
            (false, Role::Calling) => (0, 8),
            (false, Role::Answering) => (8, 0),
        };
        Self {
            mode,
            sends,
            hears,
            station: None,
            fs: 0.0,
            fs_out: 0.0,
            trim_said: 0.0,
            samples: 0,
            deaf: 0,
            first_loaded: None,
            until_look: 0,
            exact_line: false,
            inexact: false,
            note: None,
        }
    }

    /// Say that the line carries samples exactly, or does not. Before the
    /// first frame, since it decides what the call starts as.
    pub fn exact_line(&mut self, exact: bool) {
        if self.exact_line != exact {
            self.exact_line = exact;
            self.station = None;
        }
    }

    /// What there is to tell about the call since this was last asked.
    pub fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }

    fn build(&mut self, fs: f64, fs_out: f64, channels: usize) -> Result<End, String> {
        // On a line of exact samples the multicarrier mode gives way to no
        // modulation at all, until that has been tried and failed.
        if matches!(self.mode, VarcMode::Dmt | VarcMode::Linear) {
            self.mode = if self.exact_line && !self.inexact { VarcMode::Linear } else { VarcMode::Dmt };
        }
        let mut pairs: Vec<(Box<dyn Sender>, Box<dyn Receiver>)> = Vec::new();
        for c in 0..channels as u8 {
            let (sends, hears) = (self.sends + c, self.hears + c);
            match self.mode {
                VarcMode::Linear => {
                    pairs.push((Box::new(exact::Tx::new(fs_out)), Box::new(exact::Rx::new(fs))));
                }
                VarcMode::Dmt => {
                    pairs.push((
                        Box::new(dmt::Tx::new(Plan::for_rate(fs_out).coloured(sends))),
                        Box::new(dmt::Rx::new(Plan::for_rate(fs).coloured(hears))),
                    ));
                }
                VarcMode::Qam | VarcMode::Pcm => {
                    let name = self.mode.name();
                    let qam = self.mode == VarcMode::Qam;
                    let shape = |fs: f64| {
                        let shape = if qam { Shape::qam(fs) } else { Shape::pcm(fs) };
                        shape.ok_or_else(|| {
                            format!("{name} wants a sound card running at a multiple of 48 kHz, not {fs} Hz")
                        })
                    };
                    pairs.push((
                        Box::new(sc::Tx::new(shape(fs_out)?.coloured(sends))),
                        Box::new(sc::Rx::new(shape(fs)?.coloured(hears))),
                    ));
                }
            }
        }
        Ok(Station::new(fs, pairs))
    }

    /// One frame to send, when hearing and sending are stepped apart.
    /// Silence until something has been heard to say what the rates are.
    pub fn speak(&mut self, output: &mut [f64]) {
        match self.station.as_mut() {
            Some(station) => station.speak(output),
            None => output.fill(0.0),
        }
    }

    /// One frame heard, at `fs`, by an end that sends at `fs_out`: the two
    /// need not be the same, since each direction is a line of its own and
    /// the far end hears this one at whatever rate this one sends. The call's
    /// clock is the samples heard. `Ok(true)` every few
    /// milliseconds, when the line has been looked over and whoever is
    /// running the call should look at it too: not every sample, because
    /// asking each channel how it was getting on at every sample of a
    /// 192 kHz card cost more than the modulation did.
    pub fn hear(&mut self, fs: f64, fs_out: f64, input: &[f64]) -> Result<bool, String> {
        if self.station.is_none() || self.fs != fs || self.fs_out != fs_out {
            self.station = Some(self.build(fs, fs_out, input.len())?);
            self.fs = fs;
            self.fs_out = fs_out;
            self.until_look = 0;
        }
        let station = self.station.as_mut().expect("just built");
        station.hear(input);
        self.samples += 1;
        if self.until_look > 0 {
            self.until_look -= 1;
            return Ok(false);
        }
        let every = (fs / LOOKS_A_SECOND) as u32;
        self.until_look = every.saturating_sub(1);
        if station.channels.iter().any(|c| c.rx.health().locked) {
            self.deaf = 0;
        } else {
            self.deaf += u64::from(every.max(1));
        }
        // A change of level is worth saying: it starts the line again.
        let trim: f64 = station.channels.iter().map(|c| c.trim_db).sum();
        if trim != self.trim_said {
            let lower = trim > self.trim_said;
            self.trim_said = trim;
            let levels: Vec<String> = station.channels.iter().map(|c| format!("{:.1} dB", -c.trim_db)).collect();
            let why = station
                .channels
                .iter()
                .find_map(|c| c.there)
                .map(|r| format!("peaks at {:.1} dBFS, {} samples at full scale", r.peak_db, r.clipped))
                .unwrap_or_default();
            self.note = Some(if lower {
                format!("V.ARC: the far end hears this end too loud ({why}); now sending at {}", levels.join(", "))
            } else {
                format!("V.ARC: the far end has room again ({why}); now sending at {}", levels.join(", "))
            });
        }
        if self.first_loaded.is_none() && station.channels.iter().any(|c| c.loaded()) {
            self.first_loaded = Some(self.samples);
        }
        // A line that was to carry samples exactly and has not: neither end
        // can read the other, and both find that out at about the same
        // time, so both give up on it at about the same time and start
        // again as something that only wants the line to be clean.
        if self.mode == VarcMode::Linear && self.first_loaded.is_none() {
            let waited = self.samples as f64 / fs;
            let heard = station.channels.iter().any(|c| c.rx.health().locked);
            if (!heard && waited > EXACT_UNHEARD) || waited > EXACT_UNAGREED {
                self.inexact = true;
                self.note = Some(
                    "V.ARC: the line does not carry samples exactly; carrying on as the multicarrier mode".to_owned(),
                );
                self.station = Some(self.build(fs, fs_out, input.len())?);
                self.deaf = 0;
            }
        }
        Ok(true)
    }

    pub fn seconds(&self) -> f64 {
        if self.fs > 0.0 { self.samples as f64 / self.fs } else { 0.0 }
    }

    /// Seconds since any channel heard the far end.
    pub fn deaf_seconds(&self) -> f64 {
        if self.fs > 0.0 { self.deaf as f64 / self.fs } else { 0.0 }
    }

    pub fn hearing(&self) -> bool {
        self.station.as_ref().is_some_and(|s| s.channels.iter().any(|c| c.rx.health().locked))
    }

    /// The rates arriving and going, in bits a second of data, over the
    /// channels that are loaded both ways.
    pub fn rates(&self) -> (u32, u32) {
        let Some(station) = self.station.as_ref() else { return (0, 0) };
        let (mut rx, mut tx) = (0.0, 0.0);
        for channel in station.channels.iter().filter(|c| c.loaded()) {
            rx += channel.rx.rates().1;
            tx += channel.tx.rates().1;
        }
        (rx as u32, tx as u32)
    }

    /// Whether the call is ready to be called a connection: every channel
    /// loaded, or some loaded and the rest given long enough to show they
    /// are not coming. A lead with one channel wired is a line all the same.
    pub fn ready(&self) -> bool {
        let Some(station) = self.station.as_ref() else { return false };
        let loaded = station.channels.iter().filter(|c| c.loaded()).count();
        let waited = self.first_loaded.is_some_and(|at| self.samples - at > (6.0 * self.fs) as u64);
        loaded > 0 && (loaded == station.channels.len() || waited)
    }

    pub fn channels(&self) -> (usize, usize) {
        match self.station.as_ref() {
            Some(s) => (s.channels.iter().filter(|c| c.loaded()).count(), s.channels.len()),
            None => (0, 0),
        }
    }

    pub fn write(&mut self, octets: &[u8]) {
        if let Some(station) = self.station.as_mut() {
            station.write(octets);
        }
    }

    pub fn read(&mut self) -> Vec<u8> {
        self.station.as_mut().map(Station::read).unwrap_or_default()
    }

    /// Octets written and not yet acknowledged.
    pub fn pending(&self) -> usize {
        self.station.as_ref().map_or(0, |s| s.arq.pending())
    }

    pub fn phase(&self) -> &'static str {
        let Some(station) = self.station.as_ref() else { return "V.ARC: waiting for the line" };
        if station.channels.iter().any(|c| c.loaded()) {
            "V.ARC: loaded"
        } else if station.channels.iter().any(|c| c.rx.health().steady) {
            "V.ARC: exchanging tables"
        } else if station.channels.iter().any(|c| c.rx.health().locked) {
            "V.ARC: measuring the line"
        } else {
            "V.ARC: listening"
        }
    }

    /// Points for a symbol diagram: those of the first channel that has
    /// any, scaled so the grid's outermost point is at one, and the bits a
    /// symbol of that grid. The other channels' are thrown away, so that
    /// what is drawn is one grid and not two laid over each other.
    pub fn take_points(&mut self) -> (Vec<(f64, f64)>, u8) {
        let mut taken = (Vec::new(), 0);
        let Some(station) = self.station.as_mut() else { return taken };
        for channel in &mut station.channels {
            let points = channel.rx.points();
            if taken.0.is_empty() && !points.is_empty() {
                taken = (points, channel.rx.shown_bits());
            }
        }
        taken
    }

    /// The Reed-Solomon code on what is arriving, (n, k), on the first
    /// channel that is hearing a table it knows.
    pub fn code(&self) -> Option<(usize, usize)> {
        self.station.as_ref()?.channels.iter().find_map(|c| c.rx.code())
    }

    /// What the far end says it hears of this end, on the first channel it
    /// has said anything about, and how far this end has turned itself
    /// down on that channel because of it.
    pub fn far_end(&self) -> Option<(Report, f64)> {
        self.station.as_ref()?.channels.iter().find_map(|c| c.there.map(|report| (report, c.trim_db)))
    }

    /// The signal to noise ratio on each channel, in decibels.
    pub fn snr_db(&self) -> Vec<f64> {
        self.station.as_ref().map_or_else(Vec::new, |s| s.channels.iter().map(|c| c.rx.health().snr_db).collect())
    }
}
