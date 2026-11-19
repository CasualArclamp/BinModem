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

use varc::dmt::{self, Plan};
use varc::link::{Receiver, Sender};
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
        }
    }
}

type End = Station<Box<dyn Sender>, Box<dyn Receiver>>;

/// One V.ARC call.
pub(crate) struct Call {
    pub mode: VarcMode,
    /// The known points this end sends with and listens for, a set a
    /// channel, counted from these.
    sends: u8,
    hears: u8,
    station: Option<End>,
    fs: f64,
    samples: u64,
    /// Samples since any channel last heard the far end.
    deaf: u64,
    /// When the first channel came up loaded, in samples.
    first_loaded: Option<u64>,
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
        Self { mode, sends, hears, station: None, fs: 0.0, samples: 0, deaf: 0, first_loaded: None }
    }

    fn build(&self, fs: f64, channels: usize) -> Result<End, String> {
        let mut pairs: Vec<(Box<dyn Sender>, Box<dyn Receiver>)> = Vec::new();
        for c in 0..channels as u8 {
            let (sends, hears) = (self.sends + c, self.hears + c);
            match self.mode {
                VarcMode::Dmt => {
                    let plan = Plan::for_rate(fs);
                    pairs.push((
                        Box::new(dmt::Tx::new(plan.clone().coloured(sends))),
                        Box::new(dmt::Rx::new(plan.coloured(hears))),
                    ));
                }
                VarcMode::Qam | VarcMode::Pcm => {
                    let shape = if self.mode == VarcMode::Qam { Shape::qam(fs) } else { Shape::pcm(fs) };
                    let shape = shape.ok_or_else(|| {
                        format!("{} wants a sound card running at a multiple of 48 kHz, not {fs} Hz", self.mode.name())
                    })?;
                    pairs.push((
                        Box::new(sc::Tx::new(shape.clone().coloured(sends))),
                        Box::new(sc::Rx::new(shape.coloured(hears))),
                    ));
                }
            }
        }
        Ok(Station::new(fs, pairs))
    }

    /// One frame of the card's samples in, one out.
    pub fn step(&mut self, fs: f64, input: &[f64], output: &mut [f64]) -> Result<(), String> {
        if self.station.is_none() || self.fs != fs {
            self.station = Some(self.build(fs, input.len())?);
            self.fs = fs;
        }
        let station = self.station.as_mut().expect("just built");
        station.step(input, output);
        self.samples += 1;
        if station.channels.iter().any(|c| c.rx.health().locked) {
            self.deaf = 0;
        } else {
            self.deaf += 1;
        }
        if self.first_loaded.is_none() && station.channels.iter().any(|c| c.loaded()) {
            self.first_loaded = Some(self.samples);
        }
        Ok(())
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
        let Some(station) = self.station.as_ref() else { return "V.ARC: waiting for the sound card" };
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

    /// The signal to noise ratio on each channel, in decibels.
    pub fn snr_db(&self) -> Vec<f64> {
        self.station.as_ref().map_or_else(Vec::new, |s| s.channels.iter().map(|c| c.rx.health().snr_db).collect())
    }
}
