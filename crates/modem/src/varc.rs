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
use varc::echo::Canceller;
use varc::hello::{Agreed, Caps, Faster, Hello, Slower, Wanted};
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
            // The multicarrier mode on one pair of wires is the same
            // modulation behind an echo canceller.
            "VARC" | "VARC2W" => Some(Self::Dmt),
            "VARCQ" | "VARCQ2W" => Some(Self::Qam),
            "VARCP" | "VARCP2W" => Some(Self::Pcm),
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

/// How long each end of a two-wire call has the line to itself, to learn
/// its own echo with nobody talking over it, in seconds. The canceller needs
/// most of a second to find how far round the line its echo is and a
/// fraction of one to fit it.
const ALONE: f64 = 2.0;

/// Where a two-wire call has got to in learning its echoes. After the
/// handshake the caller sends alone, then the answerer, then both.
///
/// Neither end has a clock the other can see, so each step is taken on what
/// is heard: the line going quiet, the far end's signal starting, the far
/// end's signal stopping. Every wait has a limit, past which the end simply
/// goes on, and a call whose turns went wrong is a call that learns its
/// echoes with the far end talking, as it did before there were turns.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Turn {
    /// Silent, waiting for the handshake's tones to have stopped.
    Quiet { waited: f64, still: f64 },
    /// Sending alone, for this many more samples.
    Mine { left: f64 },
    /// Silent while the far end sends alone. `armed` is whether the line has
    /// been heard quiet since this end stopped: until it has, what is on
    /// the line is the tail of this end's own echo, which is not the far
    /// end starting. `heard` is how loud the far end was when it started,
    /// once it has; `gone` how long the line has been quiet; `left` what
    /// remains of the far end's turn as this end counts it.
    Theirs { armed: bool, waited: f64, heard: Option<f64>, gone: f64, left: f64 },
    /// Both ends sending: the call itself.
    Both,
}

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
    /// The line is one pair of wires, both ends sending into it: a
    /// canceller a channel takes this end's own signal out of what it
    /// hears. Empty on a line with a wire each way.
    two_wire: bool,
    echo: Vec<Canceller>,
    cleaned: Vec<f64>,
    /// How far down the echo was when whoever is watching was last told.
    echo_said: f64,
    /// Times the cancellers had found the echo somewhere new, when last
    /// looked.
    echo_moved: u64,
    /// The end that called, and a lead that comes straight back.
    calling: bool,
    looped: bool,
    /// Whose turn it is on a two-wire call, how loud the line is now and
    /// was during the handshake, and whether this end is keeping silent.
    turn: Turn,
    loud: f64,
    spoke: f64,
    muted: bool,
    /// How loud the line is with nobody on it, as heard between the
    /// handshake and the first turn.
    floor: f64,
    /// The handshake's tones as they are heard, for a symbol scope: which
    /// tone, eight times a bit.
    symbols: Vec<f64>,
    /// The handshake, while it is going on, and what came of it: what the
    /// far end said it was, and what the two will do.
    hello: Option<Hello>,
    agreed: Option<Agreed>,
    far: Option<Caps>,
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
    notes: Vec<String>,
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
            two_wire: false,
            echo: Vec::new(),
            cleaned: Vec::new(),
            echo_said: 0.0,
            echo_moved: 0,
            calling: role == Role::Calling,
            looped,
            turn: Turn::Both,
            loud: 0.0,
            spoke: 0.0,
            muted: false,
            floor: 0.0,
            symbols: Vec::new(),
            hello: None,
            agreed: None,
            far: None,
            samples: 0,
            deaf: 0,
            first_loaded: None,
            until_look: 0,
            exact_line: false,
            inexact: false,
            notes: Vec::new(),
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
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// One sample of a two-wire call's turns: see [`Turn`].
    fn take_turns(&mut self, fs: f64, input: &[f64]) {
        let power: f64 = input.iter().map(|x| x * x).sum();
        // Two milliseconds' worth: quick enough to time a turn by.
        self.loud += (power - self.loud) / (0.002 * fs);
        // Quiet, for knowing the handshake is over, is a hundredth of what
        // the handshake sounded like. After that the line has been heard
        // with nobody on it, and a signal is ten times that: the far end
        // taking its turn may be far fainter than this end's own tones
        // were, which is most of what the handshake sounded like here.
        let quiet = (0.01 * self.spoke).max(1e-9);
        let signal = (10.0 * self.floor).max(1e-8);
        let settle = |echo: &mut Vec<Canceller>| -> Option<f64> {
            // Only what was learned well is kept from being unlearned.
            let down = echo.iter().map(Canceller::down_db).fold(f64::INFINITY, f64::min);
            let found = echo.iter().all(|c| c.delay().is_some());
            (found && down >= 20.0).then(|| {
                echo.iter_mut().for_each(Canceller::settle);
                down
            })
        };
        self.turn = match self.turn {
            Turn::Quiet { waited, still } => {
                let still = if self.loud < quiet { still + 1.0 } else { 0.0 };
                if still > 0.15 * fs || waited > 1.5 * fs {
                    self.floor = self.loud;
                    if self.calling {
                        self.muted = false;
                        Turn::Mine { left: ALONE * fs }
                    } else {
                        Turn::Theirs { armed: true, waited: 0.0, heard: None, gone: 0.0, left: 0.0 }
                    }
                } else {
                    Turn::Quiet { waited: waited + 1.0, still }
                }
            }
            Turn::Mine { left } if left > 0.0 => Turn::Mine { left: left - 1.0 },
            Turn::Mine { .. } => {
                if let Some(down) = settle(&mut self.echo) {
                    self.notes.push(format!("V.ARC: this end's echo learned with the line to itself: {down:.0} dB down"));
                }
                if self.calling {
                    // The answerer's turn now, once this end's own echo
                    // has died away.
                    self.muted = true;
                    Turn::Theirs { armed: false, waited: 0.0, heard: None, gone: 0.0, left: 0.0 }
                } else {
                    // The answerer goes straight on; the caller joins it.
                    Turn::Both
                }
            }
            Turn::Theirs { armed: false, waited, gone, .. } => {
                // Ten milliseconds of quiet, or half a second in any case:
                // no echo this canceller can deal with is longer.
                let gone = if self.loud < signal { gone + 1.0 } else { 0.0 };
                let armed = gone > 0.01 * fs || waited > 0.5 * fs;
                Turn::Theirs { armed, waited: if armed { 0.0 } else { waited + 1.0 }, heard: None, gone: if armed { 0.0 } else { gone }, left: 0.0 }
            }
            Turn::Theirs { armed: true, waited, heard: None, .. } => {
                if self.loud > signal {
                    Turn::Theirs { armed: true, waited: 0.0, heard: Some(self.loud), gone: 0.0, left: ALONE * fs }
                } else if waited > 4.0 * fs {
                    // Nobody took a turn. On with it.
                    self.muted = false;
                    Turn::Both
                } else {
                    Turn::Theirs { armed: true, waited: waited + 1.0, heard: None, gone: 0.0, left: 0.0 }
                }
            }
            Turn::Theirs { armed: true, waited, heard: Some(level), gone, left } => {
                if self.calling {
                    // The caller cannot hear the answerer stop, since it
                    // does not: it counts the answerer's turn from when it
                    // began and joins in as it ends.
                    if left <= 0.0 {
                        self.muted = false;
                        Turn::Both
                    } else {
                        Turn::Theirs { armed: true, waited, heard: Some(level), gone, left: left - 1.0 }
                    }
                } else {
                    // The answerer waits for the caller to fall silent:
                    // a hundredth of the loudest it has been, or back to
                    // what the empty line sounded like.
                    let level = level.max(self.loud);
                    let gone = if self.loud < (0.01 * level).max(signal) { gone + 1.0 } else { 0.0 };
                    if gone > 0.1 * fs || waited > (ALONE + 2.0) * fs {
                        self.muted = false;
                        Turn::Mine { left: ALONE * fs }
                    } else {
                        Turn::Theirs { armed: true, waited: waited + 1.0, heard: Some(level), gone, left }
                    }
                }
            }
            Turn::Both => Turn::Both,
        };
    }

    /// Whether this call starts by saying what each end is. Not on a lead
    /// that comes straight back, where there is nobody else; and not on a
    /// line of exact samples, where the far end is known to be the same as
    /// this one and the samples are not sound.
    fn greets(&self) -> bool {
        !self.looped && !(self.exact_line && !self.inexact)
    }

    /// The ends as they would be with nothing to agree: both the same.
    fn alone(fs: f64, fs_out: f64, wanted: Wanted, two_wire: bool) -> Agreed {
        Agreed { wanted, two_wire, send_hz: fs_out, hear_hz: fs, send_times: 1, hear_times: 1, notes: Vec::new() }
    }

    fn wanted(&self) -> Wanted {
        match self.mode {
            VarcMode::Qam => Wanted::Qam,
            VarcMode::Pcm => Wanted::Pcm,
            _ => Wanted::Dmt,
        }
    }

    fn build(&mut self, fs: f64, agreed: &Agreed, channels: usize) -> Result<End, String> {
        // On a line of exact samples the multicarrier mode gives way to no
        // modulation at all, until that has been tried and failed.
        self.mode = match agreed.wanted {
            Wanted::Qam => VarcMode::Qam,
            Wanted::Pcm => VarcMode::Pcm,
            Wanted::Dmt if self.exact_line && !self.inexact => VarcMode::Linear,
            Wanted::Dmt => VarcMode::Dmt,
        };
        // The modulation runs at the slower card's rate each way; this end
        // makes up the difference where its own card is the faster.
        let (fs, fs_out, device) = (agreed.hear_hz, agreed.send_hz, fs);
        let fit = |tx: Box<dyn Sender>, rx: Box<dyn Receiver>| -> (Box<dyn Sender>, Box<dyn Receiver>) {
            let tx: Box<dyn Sender> = if agreed.send_times > 1 { Box::new(Faster::new(tx, agreed.send_times)) } else { tx };
            let rx: Box<dyn Receiver> = if agreed.hear_times > 1 { Box::new(Slower::new(rx, agreed.hear_times)) } else { rx };
            (tx, rx)
        };
        let mut pairs: Vec<(Box<dyn Sender>, Box<dyn Receiver>)> = Vec::new();
        for c in 0..channels as u8 {
            let (sends, hears) = (self.sends + c, self.hears + c);
            match self.mode {
                VarcMode::Linear => {
                    pairs.push(fit(Box::new(exact::Tx::new(fs_out)), Box::new(exact::Rx::new(fs))));
                }
                VarcMode::Dmt => {
                    pairs.push(fit(
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
                    pairs.push(fit(
                        Box::new(sc::Tx::new(shape(fs_out)?.coloured(sends))),
                        Box::new(sc::Rx::new(shape(fs)?.coloured(hears))),
                    ));
                }
            }
        }
        Ok(Station::new(device, pairs))
    }

    /// One frame to send, when hearing and sending are stepped apart.
    /// Silence until something has been heard to say what the rates are.
    pub fn speak(&mut self, output: &mut [f64]) {
        match (self.hello.as_mut(), self.station.as_mut()) {
            // Saying what this end is: the same on every channel.
            (Some(hello), _) => output.fill(hello.say()),
            (None, Some(station)) => {
                station.speak(output);
                // Not this end's turn: the modulation runs on, unheard.
                if self.muted {
                    output.fill(0.0);
                }
            }
            (None, None) => output.fill(0.0),
        }
        for (canceller, &sample) in self.echo.iter_mut().zip(output.iter()) {
            canceller.sent(sample);
        }
    }

    /// Say that the line is one pair of wires, or is not. Before the call's
    /// first sample.
    pub fn two_wire(&mut self, two_wire: bool) {
        self.two_wire = two_wire;
    }

    /// How far down each channel's canceller has taken this end's echo, in
    /// decibels, and how far round the line it found it, in samples. Empty
    /// on a line with a wire each way.
    pub fn echo(&self) -> Vec<(f64, Option<usize>)> {
        self.echo.iter().map(|c| (c.down_db(), c.delay())).collect()
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
        if self.fs != fs || self.fs_out != fs_out {
            // A line at other rates is another line: start from nothing.
            self.station = None;
            self.hello = None;
            self.agreed = None;
            self.fs = fs;
            self.fs_out = fs_out;
        }
        // First, each end says what it is, and the two work out what they
        // will do. Until that is done there is no modulation to run.
        if self.station.is_none() && self.agreed.is_none() && self.greets() {
            let (calling, wanted, two_wire) = (self.calling, self.wanted(), self.two_wire);
            let hello = self.hello.get_or_insert_with(|| {
                Hello::new(
                    calling,
                    Caps {
                        hears_hz: fs as u32,
                        sends_hz: fs_out as u32,
                        channels: input.len() as u8,
                        wanted,
                        two_wire,
                        heard: false,
                    },
                )
            });
            hello.hear(input);
            if self.samples.is_multiple_of((fs / (8.0 * varc::hello::BAUD)).max(1.0) as u64) && self.symbols.len() < 4096 {
                self.symbols.push(hello.eye());
            }
            // How loud the line is while the two ends are talking, to know
            // quiet by afterwards.
            let power: f64 = input.iter().map(|x| x * x).sum();
            self.spoke += (power - self.spoke) / (0.05 * fs);
            self.samples += 1;
            if !hello.done() {
                return Ok(self.samples.is_multiple_of((fs / LOOKS_A_SECOND).max(1.0) as u64));
            }
            let theirs = hello.theirs().copied();
            self.far = theirs;
            let agreed = hello.agreed().unwrap_or_else(|| Err("the far end said nothing".to_owned()))?;
            self.hello = None;
            let mut said = Vec::new();
            if let Some(theirs) = theirs {
                said.push(format!(
                    "V.ARC: the far end hears at {} Hz and sends at {} Hz, on {} channel{}",
                    theirs.hears_hz,
                    theirs.sends_hz,
                    theirs.channels,
                    if theirs.channels == 1 { "" } else { "s" }
                ));
            }
            said.push(format!(
                "V.ARC: agreed on {}, {:.0} Hz out and {:.0} Hz back",
                if agreed.two_wire { "one pair of wires" } else { "a wire each way" },
                agreed.send_hz,
                agreed.hear_hz
            ));
            said.extend(agreed.notes.iter().map(|note| format!("V.ARC: {note}")));
            self.notes.push(said.join("\n"));
            self.agreed = Some(agreed);
        }
        if self.station.is_none() {
            let agreed = match self.agreed.clone() {
                Some(agreed) => agreed,
                None => {
                    // Nobody to agree with: this end as it was asked to be.
                    if self.two_wire && fs != fs_out {
                        return Err(format!(
                            "V.ARC two-wire needs its input and output on one sound card at one rate: this line hears at {fs} Hz and sends at {fs_out} Hz. A line with a wire each way is plain V.ARC"
                        ));
                    }
                    Self::alone(fs, fs_out, self.wanted(), self.two_wire)
                }
            };
            let mut station = self.build(fs, &agreed, input.len())?;
            if agreed.two_wire {
                // Two ends on one pair of wires are two signals in one
                // converter, and two at the level one is made for do not
                // fit: their peaks together pass full scale. So each starts
                // 6 dB down. Found the hard way: each end learned its echo
                // alone, at full level and cleanly, and the moment the two
                // spoke together the line clipped.
                for channel in &mut station.channels {
                    channel.trim_db = 6.0;
                    channel.tx.trim(6.0);
                }
                self.trim_said = 6.0 * station.channels.len() as f64;
            }
            self.station = Some(station);
            self.until_look = 0;
            // The canceller lays what was sent against what was heard,
            // sample for sample, at the card's own rate.
            self.echo = if agreed.two_wire { (0..input.len()).map(|_| Canceller::new(fs)).collect() } else { Vec::new() };
            // On one pair of wires each end first has the line to itself.
            // Only where there was a handshake: without one there is no far
            // end known to be keeping the same turns.
            let turns = agreed.two_wire && self.far.is_some();
            self.turn = if turns { Turn::Quiet { waited: 0.0, still: 0.0 } } else { Turn::Both };
            self.muted = turns;
            self.loud = self.spoke;
            self.agreed = Some(agreed);
        }
        if self.turn != Turn::Both {
            self.take_turns(fs, input);
        }
        let station = self.station.as_mut().expect("just built");
        if self.echo.is_empty() {
            station.hear(input);
        } else {
            self.cleaned.clear();
            for ((canceller, &x), channel) in self.echo.iter_mut().zip(input).zip(station.channels.iter_mut()) {
                self.cleaned.push(canceller.clean(x));
                // The receiver's own stage is told what the echo is of,
                // and to start again if the echo has moved.
                channel.rx.echo_under(canceller.under_now());
            }
            if self.echo.iter().map(|c| c.moved).sum::<u64>() != self.echo_moved {
                self.echo_moved = self.echo.iter().map(|c| c.moved).sum();
                for channel in &mut station.channels {
                    channel.rx.echo_again();
                }
            }
            station.line(input);
            station.hear(&self.cleaned);
        }
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
        // Once a receiver's own echo stage is working, the canceller in
        // front of it holds still for it; and lets go if the receiver loses
        // the far end, since that stage learns from the far end's symbols.
        //
        // Only once the receiver is already reading well, though. That stage
        // can only finish a job that is mostly done: it learns from symbols
        // it can read, and at 10 dB it reads too few of them rightly to
        // learn much. Below 30 dB the canceller in front is the one that
        // will get there, however slowly, and it is left to.
        for (canceller, channel) in self.echo.iter_mut().zip(&station.channels) {
            let health = channel.rx.health();
            let enough = if canceller.held() { 24.0 } else { 30.0 };
            canceller.hold(health.steady && health.snr_db >= enough && channel.rx.echo_left_db().is_some());
        }
        // The echo, as it goes down: said each time it is 10 dB further.
        if let Some(canceller) = self.echo.first() {
            let down = canceller.down_db();
            if canceller.delay().is_some() && down >= self.echo_said + 10.0 {
                self.echo_said = (down / 10.0).floor() * 10.0;
                self.notes.push(format!(
                    "V.ARC: this end's echo is {:.1} ms round the line and {down:.0} dB down",
                    1000.0 * canceller.delay().unwrap_or(0) as f64 / fs
                ));
            }
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
            self.notes.push(if lower {
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
                self.notes.push(
                    "V.ARC: the line does not carry samples exactly; carrying on as the multicarrier mode".to_owned(),
                );
                self.agreed = None;
                self.station = None;
                self.deaf = 0;
                return Ok(true);
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
        if let Some(hello) = self.hello.as_ref() {
            return if hello.theirs().is_some() { "V.ARC: agreeing with the far end" } else { "V.ARC: calling the far end" };
        }
        match self.turn {
            Turn::Quiet { .. } => return "V.ARC: waiting for the line to go quiet",
            Turn::Mine { .. } => return "V.ARC: learning this end's echo, alone on the line",
            Turn::Theirs { .. } => return "V.ARC: silent while the far end learns its echo",
            Turn::Both => {}
        }
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

    /// The band as the first channel that hears anything has it: each
    /// carrier's frequency, its signal to noise in decibels, and the bits
    /// the far end is putting on it.
    pub fn carriers(&self) -> Vec<(f64, f64, u8)> {
        let Some(station) = self.station.as_ref() else { return Vec::new() };
        station
            .channels
            .iter()
            .find(|c| c.rx.health().locked)
            .map(|c| c.rx.spectrum().iter().map(|bin| (bin.hz, bin.snr_db, bin.bits)).collect())
            .unwrap_or_default()
    }

    /// The first channel's echo, as its canceller has learned it: the
    /// line's answer to one sample sent. Empty on a line with a wire each
    /// way, and until the echo has been found.
    pub fn echo_response(&self) -> Vec<f64> {
        self.echo.first().map(Canceller::response).unwrap_or_default()
    }

    /// What the far end said it was in the handshake, and what was agreed,
    /// a row a fact: for showing beside what a V.8 menu would have said.
    pub fn distant(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        let said = |caps: &Caps| {
            format!(
                "hears at {} Hz, sends at {} Hz, {} channel{}",
                caps.hears_hz,
                caps.sends_hz,
                caps.channels,
                if caps.channels == 1 { "" } else { "s" }
            )
        };
        let asked = |caps: &Caps| {
            let mode = match caps.wanted {
                Wanted::Dmt => "multicarrier",
                Wanted::Qam => "QAM at 16 000 baud",
                Wanted::Pcm => "levels at 16 000 baud",
            };
            format!("{mode}, {}", if caps.two_wire { "one pair of wires" } else { "a wire each way" })
        };
        // While it is still going on, what has arrived so far.
        let far = self.hello.as_ref().and_then(|hello| hello.theirs().copied()).or(self.far);
        match far {
            Some(caps) => {
                rows.push(("V.ARC hello", said(&caps)));
                rows.push(("asked for", asked(&caps)));
                if self.hello.is_some() {
                    rows.push(("far end", if caps.heard { "has heard this end" } else { "has not heard this end yet" }.to_owned()));
                }
            }
            None if self.hello.is_some() => {
                let (ours, theirs) = if self.calling { ("1000 and 1400", "2000 and 2400") } else { ("2000 and 2400", "1000 and 1400") };
                rows.push(("V.ARC hello", format!("calling on {ours} Hz, listening on {theirs} Hz: nothing heard yet")));
            }
            None => {}
        }
        if let (Some(agreed), true) = (self.agreed.as_ref(), self.far.is_some()) {
            rows.push((
                "agreed",
                format!(
                    "{}, {:.0} Hz out and {:.0} Hz back",
                    if agreed.two_wire { "one pair of wires" } else { "a wire each way" },
                    agreed.send_hz,
                    agreed.hear_hz
                ),
            ));
            for note in &agreed.notes {
                rows.push(("note", note.clone()));
            }
        }
        rows
    }

    /// Whether the call is still in its handshake.
    pub fn greeting(&self) -> bool {
        self.hello.is_some()
    }

    /// The handshake's tones as heard since this was last asked: which of
    /// the far end's two is arriving, between minus one and one.
    pub fn take_symbols(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.symbols)
    }

    /// The signal to noise ratio on each channel, in decibels.
    pub fn snr_db(&self) -> Vec<f64> {
        self.station.as_ref().map_or_else(Vec::new, |s| s.channels.iter().map(|c| c.rx.health().snr_db).collect())
    }
}
