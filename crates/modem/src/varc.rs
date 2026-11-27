//! A V.ARC call, as the modem sees one.
//!
//! A V.ARC call starts as every other call this modem makes does, with V.8,
//! because the far end could be anything. Each end says in its V.8 menu
//! that it has V.ARC, and only when both have said so does the call come
//! here. What is here starts with a negotiation of its own, the handshake
//! of [`varc::hello`], which settles what V.8 has no words for: rates,
//! channels, wires, and what the line is made of. Then the mode runs.
//! `ATD` and `ATA` mean what they mean, and the two ends have to differ in
//! one respect anyway (see [`varc::dmt::Plan::coloured`]).
//!
//! The awkward part is the line. The rest of the modem runs at one rate on
//! one channel, and a sound card is converted to suit. V.ARC is built on the
//! card as it is, both channels at its own rate, so a V.ARC call is stepped
//! through its own door, [`crate::Modem::varc_step`], by whatever owns the
//! sound card, with a frame of the card's own samples at a time. The modem
//! says when it wants that, with [`crate::Modem::varc_wanted`].
//!
//! The other line it runs on is a SIP call, in any coding, where there is
//! no sound card at all. Every mode runs on one: the modulations at the
//! rate of the call, which for G.711 is 8 kHz, through whatever the coding
//! does to a waveform. And where both ends say they are on the same kind of
//! call, the multicarrier mode first tries no modulation whatever, the
//! numbers of the call being the data ([`varc::exact`], [`varc::octets`]).
//! If a few seconds show that the path does not carry them as they were
//! sent, it carries on as the multicarrier modulation.

use varc::dmt::{self, Plan};
use varc::echo::Canceller;
use varc::fit::{Fitted, Unfitted};
use varc::hello::{Agreed, Caps, FITS, Faster, Half, Hello, Line, NARROW, Slower, Twice, Wanted, Watch};
use datapump::v90::ucode::{self, Law};
use varc::exact;
use varc::octets;
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
    /// No modulation on a call in G.711: its codewords are the data. What
    /// `VARC` becomes on a line that carries codewords exactly.
    G711,
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
            Self::G711 => "V.ARC/G.711",
        }
    }
}

type End = Station<Box<dyn Sender>, Box<dyn Receiver>>;

/// The most a direction is slowed: by two to this, which is eight times.
/// At 48 kHz that takes the single carrier from 16 000 baud between 1.5 and
/// 19.5 kHz down to 2000 baud between 190 Hz and 2.4 kHz. The baseband
/// levels start twice as fast and have a step more, down to the same 2000.
const SLOWEST: u8 = 3;

/// How a direction of a single-carrier call is run is one number. Up to
/// [`SLOWEST`] and one more it is how slowed the carrier is, as a power of
/// two. From this on it is a fit of the carrier to a narrow line, by its
/// place in [`FITS`]: faster than the carrier as it is built, and the
/// fastest first.
const FIT: u8 = 16;

/// Which fit a speed is, if it is one.
fn fit_of(speed: u8) -> Option<usize> {
    speed.checked_sub(FIT).map(usize::from).filter(|fit| *fit < FITS.len())
}

/// How many times slowed a speed is.
fn times(speed: u8) -> u32 {
    if speed >= FIT { 1 } else { 1 << speed }
}

/// The rate the single-carrier modes are built for. They are then run at
/// whatever rate the line has, sample for sample: see [`Call::sender`].
const BUILT_FOR: f64 = 48_000.0;

/// How long a receiver that has never heard the far end waits before it asks
/// for the modulation slower, in seconds, by how slowed it is already. A
/// slowed modulation takes as many times longer to be found.
fn patience(slowed: u8) -> f64 {
    6.0 + 3.0 * f64::from(times(slowed))
}

/// How long each end of a call on a sound card has the line to itself, to learn
/// its own echo with nobody talking over it, in seconds. The canceller needs
/// most of a second to find how far round the line its echo is and a
/// fraction of one to fit it.
const ALONE: f64 = 2.0;

/// Where a call on a sound card has got to in learning its echoes. After
/// the handshake the caller sends alone, then the answerer, then both. An
/// end that hears nothing come back in its turn has no echo, and when the
/// turns are over it starts again with no canceller.
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
    /// On one pair of wires, both ends sending into it: a canceller a
    /// channel takes this end's own signal out of what it hears. Empty once
    /// this end has had the line to itself and heard no echo.
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
    /// The loudest the line was in the second half of this end's turn,
    /// when only this end sent: its echo, or nothing.
    echoed: f64,
    /// This end's turn found no echo. When the turns are over it starts
    /// again as an end on a wire each way.
    unechoed: bool,
    /// For the rest of the call: an ear for the handshake's tones, and a
    /// voice in them, for an end that cannot hear the modulation to say so.
    watch: Option<Watch>,
    /// How slowed this end's receivers are listening for the modulation,
    /// and its transmitters sending it: by two to this.
    hears_slowed: u8,
    sends_slowed: u8,
    /// Samples the receivers have gone without hearing the far end, while
    /// they never have; and whether they ever have.
    unheard: f64,
    heard_once: bool,
    /// Codewords read when the receivers now listening began.
    read_before: u64,
    /// What somebody has chosen for a single-carrier call and not left to
    /// the modem: the baud, the code as a share of each word, and the bits
    /// a symbol.
    asks_baud: u32,
    asks: (Option<u8>, Option<f64>),
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
    /// The line carries G.711 codewords exactly, in this law: a SIP call
    /// between two of these with nothing recoding it on the way.
    g711: Option<Law>,
    /// And was found not to after all, so that it is not tried again.
    inexact: bool,
    /// When the station now running was built, in samples.
    built_at: u64,
    /// Whether the fits the handshake found have been taken up.
    refitted: bool,
    /// What the stream may be compressed with: `varc::squeeze`'s bits.
    squeeze: u8,
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
            echoed: 0.0,
            unechoed: false,
            watch: None,
            hears_slowed: 0,
            sends_slowed: 0,
            unheard: 0.0,
            heard_once: false,
            read_before: 0,
            asks_baud: 0,
            asks: (None, None),
            symbols: Vec::new(),
            hello: None,
            agreed: None,
            far: None,
            samples: 0,
            deaf: 0,
            first_loaded: None,
            until_look: 0,
            exact_line: false,
            g711: None,
            inexact: false,
            built_at: 0,
            refitted: false,
            squeeze: 0,
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

    /// Say that the line carries G.711 codewords, in the law given, or
    /// that it does not. Before the first frame.
    pub fn codeword_line(&mut self, law: Option<Law>) {
        if self.g711 != law {
            self.g711 = law;
            self.station = None;
        }
    }

    /// What the stream may be compressed with, if the far end has it too.
    pub fn compress(&mut self, can: u8) {
        self.squeeze = can;
        if let Some(station) = self.station.as_mut() {
            station.compress(can);
        }
    }

    /// What each direction's stream is compressed with, sending and then
    /// receiving, and the octets before and after each way.
    pub fn compression(&self) -> Option<((u8, u8), [u64; 4])> {
        let squeeze = &self.station.as_ref()?.arq.squeeze;
        Some((squeeze.methods(), [squeeze.plain_out, squeeze.coded_out, squeeze.plain_in, squeeze.coded_in]))
    }

    /// Whether the far end's compressed stream stopped making sense.
    pub fn compression_broke(&self) -> bool {
        self.station.as_ref().is_some_and(|s| s.arq.squeeze.broken())
    }

    /// What this end's line is made of, for the handshake to say.
    fn line(&self) -> Line {
        match self.g711 {
            Some(Law::A) => Line::ALaw,
            Some(Law::Mu) => Line::MuLaw,
            None if self.exact_line => Line::Samples,
            None => Line::Sound,
        }
    }

    /// The level every codeword of the line's law is, and its silence.
    fn codewords(&self) -> (octets::Levels, u8) {
        let law = self.g711.unwrap_or_default();
        let mut levels = [0.0; 256];
        for (octet, level) in levels.iter_mut().enumerate() {
            let (ucode, negative) = ucode::from_octet(law, octet as u8);
            *level = ucode::level(law, ucode) * if negative { -1.0 } else { 1.0 };
        }
        // What a packet that never came is filled with: nought, encoded.
        (levels, ucode::octet(law, ucode::nearest(law, 0).0, false))
    }

    /// What there is to tell about the call since this was last asked.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// One sample of watching for a direction that does not carry the
    /// modulation, and of doing something about it.
    ///
    /// A receiver that has never heard the far end, for as long as
    /// [`patience`] allows, starts listening for the modulation at half the
    /// speed and says so in the handshake's tones, which get through lines
    /// the modulation does not. An end that hears that said sends at
    /// whatever speed it was told. Each direction is slowed for itself: a
    /// line that is a cable one way and a loudspeaker and a microphone the
    /// other runs at full speed one way and a quarter of it the other.
    fn slow_down(&mut self, fs: f64, fs_out: f64, input: &[f64]) -> Result<(), String> {
        let Some(agreed) = self.agreed.clone() else { return Ok(()) };
        let calling = self.calling;
        let watch = self.watch.get_or_insert_with(|| Watch::new(calling, fs, fs_out, input.len()));
        if let Some(asked) = watch.hear(input) {
            let asked = if fit_of(asked).is_some() { asked } else { asked.min(self.slowest()) };
            if asked != self.sends_slowed {
                self.sends_slowed = asked;
                for c in 0..input.len() as u8 {
                    let tx = self.sender(&agreed, c, asked)?;
                    if let Some(channel) = self.station.as_mut().and_then(|s| s.channels.get_mut(usize::from(c))) {
                        channel.send_with(tx);
                    }
                }
                self.notes.push(match fit_of(asked) {
                    Some(_) => format!(
                        "V.ARC: the far end cannot hear this end; sending at {:.0} baud",
                        self.baud(agreed.send_hz, asked)
                    ),
                    None => format!("V.ARC: the far end cannot hear this end; sending {} times slower", 1u32 << asked),
                });
            }
        }
        let Some(station) = self.station.as_ref() else { return Ok(()) };
        // Heard means read: codewords arriving that the code could mend.
        // Not found, since a receiver can find a signal through a room and
        // read none of it; and not a figure in decibels, since with a code
        // that is mostly checks 8 dB is a line and with the lightest it is
        // not. Four words since this receiver began is a line.
        let read: u64 = station.channels.iter().map(|c| c.tally.words - c.tally.failed).sum();
        if read >= self.read_before + 4 {
            self.heard_once = true;
            self.unheard = 0.0;
        } else if !self.heard_once {
            self.unheard += 1.0;
        }
        if self.unheard > patience(self.hears_slowed) * fs {
            self.unheard = 0.0;
            // At the slowest there is nothing slower to ask for, but the
            // far end may not have heard the asking: say it again.
            // From a fit, the next narrower fit, and after the last of them
            // the carrier as it is built; from there, by halves.
            let slowed = match fit_of(self.hears_slowed) {
                Some(fit) if fit + 1 < FITS.len() => self.hears_slowed + 1,
                Some(_) => 0,
                None => (self.hears_slowed + 1).min(self.slowest()),
            };
            if slowed != self.hears_slowed {
                self.hears_slowed = slowed;
                self.read_before = read;
                for c in 0..input.len() as u8 {
                    let rx = self.receiver(&agreed, c, slowed)?;
                    if let Some(channel) = self.station.as_mut().and_then(|s| s.channels.get_mut(usize::from(c))) {
                        channel.listen_with(rx);
                    }
                }
                self.notes.push(if slowed < FIT && slowed > 0 {
                    format!("V.ARC: nothing heard of the far end; asking it to send {} times slower", 1u32 << slowed)
                } else {
                    format!(
                        "V.ARC: nothing heard of the far end; asking it to send at {:.0} baud",
                        self.baud(agreed.hear_hz, slowed)
                    )
                });
            }
            if let Some(watch) = self.watch.as_mut() {
                watch.tell(slowed);
            }
        }
        Ok(())
    }

    /// How slowed each direction is, as a number of times: what this end
    /// hears, then what it sends.
    pub fn slowed(&self) -> (u32, u32) {
        (times(self.hears_slowed), times(self.sends_slowed))
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
                // The caller starts its turn after 0.15 s of quiet. The
                // answerer must be listening by then, so it asks for less
                // quiet. It also waits longer for it: the two ends do not
                // finish the handshake at the same moment, and a caller
                // that finished 1 s later is still sending its tones.
                let (enough, most) = if self.calling { (0.15, 1.5) } else { (0.05, 4.0) };
                if still > enough * fs || waited > most * fs {
                    // A line that was never quiet says nothing of its floor.
                    self.floor = self.loud.min(quiet);
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
            Turn::Mine { left } if left > 0.0 => {
                if left < 0.5 * ALONE * fs {
                    self.echoed = self.echoed.max(self.loud);
                }
                Turn::Mine { left: left - 1.0 }
            }
            Turn::Mine { .. } => {
                // No echo: nothing came back, or what came back is not
                // this end's own signal. The canceller looks for its own
                // signal at every delay, and finds none in a far end that
                // did not keep to its turn. Or what came back is a
                // hundredth of what the handshake sounded like and the
                // canceller could not learn it in its turn.
                let found = self.echo.iter().all(|c| c.delay().is_some());
                let learned = found && self.echo.iter().all(|c| c.down_db() >= 20.0);
                if self.echoed < signal || !found || (!learned && self.echoed < 0.01 * self.spoke) {
                    // This end sent alone and nothing came back: the line
                    // has a wire each way. A canceller with no echo to learn
                    // learns the far end as if it were one, and that is
                    // noise. So it goes.
                    self.echo.clear();
                    self.unechoed = true;
                    self.notes.push("V.ARC: no echo of this end on the line: a wire each way, and no canceller".to_owned());
                } else if let Some(down) = settle(&mut self.echo) {
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
                } else if waited > 4.0 * fs && !self.calling {
                    // The caller's turn was not heard. This end takes its
                    // own turn all the same: a canceller that has had no
                    // turn does not know if there is an echo.
                    self.muted = false;
                    Turn::Mine { left: ALONE * fs }
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

    /// Whether this call starts by saying what each end is: every call
    /// does, but one on a lead that comes straight back, where there is
    /// nobody else.
    fn greets(&self) -> bool {
        !self.looped
    }

    /// The ends as they would be with nothing to agree: both the same.
    fn alone(fs: f64, fs_out: f64, wanted: Wanted, two_wire: bool, direct: bool) -> Agreed {
        Agreed {
            wanted,
            two_wire,
            direct,
            send_hz: fs_out,
            hear_hz: fs,
            send_times: 1,
            hear_times: 1,
            send_baud: 0,
            hear_baud: 0,
            send_fit: 0,
            hear_fit: 0,
            notes: Vec::new(),
        }
    }

    fn wanted(&self) -> Wanted {
        match self.mode {
            VarcMode::Qam => Wanted::Qam,
            VarcMode::Pcm => Wanted::Pcm,
            _ => Wanted::Dmt,
        }
    }

    /// A transmitter for channel `c`, for the modulation slowed by two to
    /// the `slowed`.
    ///
    /// The modulation is built for one rate and run at another by how many
    /// of the card's samples it is given to each of its own. Every frequency
    /// in it moves with the rate and every symbol lasts as long as its
    /// samples do, which is how a line that will not pass the band it wants
    /// is given one it will.
    ///
    /// The single-carrier modes are built for 48 kHz, where the carrier at
    /// 16 000 baud covers 1.5 to 19.5 kHz, and are run sample for sample at
    /// whatever rate the line has: a third of the rate in baud, four-fifths
    /// of the band, whatever the card. They were 16 000 baud at any rate,
    /// which on a card at 192 kHz was a tenth of what it passes. The
    /// baseband levels keep below half their band, so they are run twice as
    /// fast again: two-thirds of the rate in baud.
    fn sender(&self, agreed: &Agreed, c: u8, slowed: u8) -> Result<Box<dyn Sender>, String> {
        let (fs, colour) = (agreed.send_hz, self.sends + c);
        let slow = times(slowed) as usize;
        Ok(match self.mode {
            VarcMode::Linear => Box::new(exact::Tx::new(fs)),
            VarcMode::G711 => Box::new(octets::Tx::new(fs, &self.codewords().0)),
            VarcMode::Dmt => {
                let tx: Box<dyn Sender> = Box::new(dmt::Tx::new(Plan::for_rate(fs).coloured(colour)));
                if agreed.send_times * slow > 1 { Box::new(Faster::slowed(tx, agreed.send_times * slow, slow)) } else { tx }
            }
            // Fitted to a narrow line: run at three times the baud that
            // was chosen, with its carrier in the middle of the line's
            // band, and brought to the line's rate. See `varc::fit`.
            VarcMode::Qam if fit_of(slowed).is_some() => {
                let (baud, carrier, rolloff) = FITS[fit_of(slowed).unwrap_or(0)];
                let shape = Shape::fitted(sc::BAUD * carrier / baud, rolloff).coloured(colour);
                Box::new(Fitted::new(Box::new(sc::Tx::new(shape)), BUILT_FOR, 3.0 * baud * fs, fs))
            }
            VarcMode::Qam => {
                let tx = Box::new(sc::Tx::new(self.shape().coloured(colour)));
                Box::new(Faster::paced(tx, agreed.send_times * slow, fs / BUILT_FOR / slow as f64))
            }
            VarcMode::Pcm if slowed == 0 => {
                let tx = Box::new(Twice::new(Box::new(sc::Tx::new(self.shape().coloured(colour)))));
                Box::new(Faster::paced(tx, agreed.send_times, fs / BUILT_FOR))
            }
            VarcMode::Pcm => {
                let tx = Box::new(sc::Tx::new(self.shape().coloured(colour)));
                Box::new(Faster::paced(tx, agreed.send_times * slow / 2, 2.0 * fs / BUILT_FOR / slow as f64))
            }
        })
    }

    /// The receiver to go with [`Self::sender`] at the far end.
    fn receiver(&self, agreed: &Agreed, c: u8, slowed: u8) -> Result<Box<dyn Receiver>, String> {
        let (fs, colour) = (agreed.hear_hz, self.hears + c);
        let slow = times(slowed) as usize;
        let mut rx: Box<dyn Receiver> = match self.mode {
            VarcMode::Linear => Box::new(exact::Rx::new(fs)),
            VarcMode::G711 => {
                let (levels, quiet) = self.codewords();
                Box::new(octets::Rx::new(fs, &levels, quiet))
            }
            VarcMode::Dmt => {
                let rx: Box<dyn Receiver> = Box::new(dmt::Rx::new(Plan::for_rate(fs).coloured(colour)));
                if agreed.hear_times * slow > 1 { Box::new(Slower::slowed(rx, agreed.hear_times * slow, slow)) } else { rx }
            }
            VarcMode::Qam if fit_of(slowed).is_some() => {
                let (baud, carrier, rolloff) = FITS[fit_of(slowed).unwrap_or(0)];
                let shape = Shape::fitted(sc::BAUD * carrier / baud, rolloff).coloured(colour);
                Box::new(Unfitted::new(Box::new(sc::Rx::new(shape)), BUILT_FOR, 3.0 * baud * fs, fs))
            }
            VarcMode::Qam => {
                let rx = Box::new(sc::Rx::new(self.shape().coloured(colour)));
                Box::new(Slower::paced(rx, agreed.hear_times * slow, fs / BUILT_FOR / slow as f64))
            }
            VarcMode::Pcm if slowed == 0 => {
                let rx = Box::new(Half::new(Box::new(sc::Rx::new(self.shape().coloured(colour)))));
                Box::new(Slower::paced(rx, agreed.hear_times, fs / BUILT_FOR))
            }
            VarcMode::Pcm => {
                let rx = Box::new(sc::Rx::new(self.shape().coloured(colour)));
                Box::new(Slower::paced(rx, agreed.hear_times * slow / 2, 2.0 * fs / BUILT_FOR / slow as f64))
            }
        };
        // What somebody chose, where they chose.
        rx.fix(self.asks.0, self.asks.1);
        Ok(rx)
    }

    fn shape(&self) -> Shape {
        let shape = if self.mode == VarcMode::Qam { Shape::qam(BUILT_FOR) } else { Shape::pcm(BUILT_FOR) };
        shape.expect("the rate the single-carrier modes are built for")
    }

    /// The most a direction of this call is slowed.
    fn slowest(&self) -> u8 {
        if self.mode == VarcMode::Pcm { SLOWEST + 1 } else { SLOWEST }
    }

    /// The baud a single-carrier direction runs at on a line of `hz`,
    /// slowed by two to the `slowed`; nought for the other modes.
    fn baud(&self, hz: f64, slowed: u8) -> f64 {
        if let (VarcMode::Qam, Some(fit)) = (self.mode, fit_of(slowed)) {
            return FITS[fit].0 * hz;
        }
        let fastest = match self.mode {
            VarcMode::Qam => hz / 3.0,
            VarcMode::Pcm => 2.0 * hz / 3.0,
            _ => return 0.0,
        };
        fastest / f64::from(times(slowed))
    }

    /// How slowed a direction on a line of `hz` has to be for the baud that
    /// was asked for: the nearest there is, the fits among them where the
    /// line may be fitted. `None` where none was.
    fn slowed_for(&self, hz: f64, baud: u32, fitted: bool) -> Option<u8> {
        if baud == 0 || self.baud(hz, 0) == 0.0 {
            return None;
        }
        let fits = if fitted && self.mode == VarcMode::Qam { FITS.len() as u8 } else { 0 };
        (0..=self.slowest()).chain(FIT..FIT + fits).min_by(|a, b| {
            let miss = |s: u8| (self.baud(hz, s).ln() - f64::from(baud).ln()).abs();
            miss(*a).total_cmp(&miss(*b))
        })
    }

    fn build(&mut self, fs: f64, agreed: &Agreed, channels: usize) -> Result<End, String> {
        // Where both ends are on the same kind of SIP call, the
        // multicarrier mode gives way to no modulation at all, the samples
        // or the codewords of the call being the data, until that has been
        // tried and failed. The single-carrier modes were asked for by
        // name and run as themselves on any call.
        self.mode = match agreed.wanted {
            Wanted::Qam => VarcMode::Qam,
            Wanted::Pcm => VarcMode::Pcm,
            Wanted::Dmt if agreed.direct && !self.inexact && self.g711.is_some() => VarcMode::G711,
            Wanted::Dmt if agreed.direct && !self.inexact && self.exact_line => VarcMode::Linear,
            Wanted::Dmt => VarcMode::Dmt,
        };
        self.built_at = self.samples;
        // A baud that was asked for, by either end for what it hears, is
        // where that direction starts; and where it stays, since somebody
        // asked for it.
        // A single carrier on a narrow line starts at the widest fit the
        // probe found the band would pass. It is not held there: an end
        // that hears nothing of it asks for the next.
        let narrow = self.mode == VarcMode::Qam
            && agreed.send_times == 1
            && agreed.hear_times == 1
            && agreed.send_hz <= NARROW
            && agreed.hear_hz <= NARROW;
        if self.mode == VarcMode::Qam && self.station.is_none() && !self.refitted {
            self.sends_slowed = if agreed.send_fit > 0 { FIT + agreed.send_fit - 1 } else { 0 };
            self.hears_slowed = if agreed.hear_fit > 0 { FIT + agreed.hear_fit - 1 } else { 0 };
            self.refitted = true;
        }
        if let Some(slowed) = self.slowed_for(agreed.send_hz, agreed.send_baud, narrow) {
            self.sends_slowed = slowed;
        }
        if let Some(slowed) = self.slowed_for(agreed.hear_hz, agreed.hear_baud, narrow) {
            self.hears_slowed = slowed;
            self.heard_once = true;
        }
        let mut pairs: Vec<(Box<dyn Sender>, Box<dyn Receiver>)> = Vec::new();
        for c in 0..channels as u8 {
            pairs.push((self.sender(agreed, c, self.sends_slowed)?, self.receiver(agreed, c, self.hears_slowed)?));
        }
        let mut station = Station::new(fs, pairs);
        station.level_is_fixed(matches!(self.mode, VarcMode::Linear | VarcMode::G711));
        station.compress(self.squeeze);
        Ok(station)
    }

    /// One frame to send, when hearing and sending are stepped apart.
    /// Silence until something has been heard to say what the rates are.
    pub fn speak(&mut self, output: &mut [f64]) {
        // One signal, the same on every channel the card has: see `hear`.
        let mut one = [0.0];
        self.speak_one(&mut one);
        output.fill(one[0]);
    }

    fn speak_one(&mut self, output: &mut [f64]) {
        match (self.hello.as_mut(), self.station.as_mut()) {
            // Saying what this end is: the same on every channel.
            (Some(hello), _) => output.fill(hello.say()),
            // Telling the far end this end cannot hear it: the modulation
            // waits where it is until that has been said.
            (None, Some(_)) if self.watch.as_ref().is_some_and(Watch::saying) => {
                let said = self.watch.as_mut().map_or(0.0, Watch::say);
                output.fill(said);
            }
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

    /// What a single-carrier call is to ask the far end for by hand: the
    /// baud, the percentage of each codeword that is data, and the bits a
    /// symbol. Nought for any leaves that one to the modem. Before the
    /// call's first sample.
    pub fn ask_for(&mut self, baud: u32, code: u8, bits: u8) {
        self.asks_baud = baud;
        self.asks = ((bits > 0).then_some(bits), (code > 0).then(|| f64::from(code) / 100.0));
    }

    /// The baud each direction of a single-carrier call is running at: what
    /// this end hears, then what it sends. Noughts for the other modes and
    /// before the ends have agreed.
    pub fn bauds(&self) -> (u32, u32) {
        match self.agreed.as_ref() {
            Some(agreed) if self.station.is_some() => (
                self.baud(agreed.hear_hz, self.hears_slowed) as u32,
                self.baud(agreed.send_hz, self.sends_slowed) as u32,
            ),
            _ => (0, 0),
        }
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
    ///
    /// The call is one signal however many channels the card has: what is
    /// sent is the same on all of them, and what is heard is all of them
    /// added together. It was a signal a channel, each with its own data,
    /// which is twice the rate down a stereo lead and a call that half
    /// works down anything else: a stereo cable one way and one microphone
    /// back left one channel of two on its starting table for good.
    pub fn hear(&mut self, fs: f64, fs_out: f64, input: &[f64]) -> Result<bool, String> {
        let one = [input.iter().sum::<f64>() / input.len().max(1) as f64];
        self.hear_one(fs, fs_out, &one)
    }

    fn hear_one(&mut self, fs: f64, fs_out: f64, input: &[f64]) -> Result<bool, String> {
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
            let (calling, wanted, asks_baud) = (self.calling, self.wanted(), self.asks_baud);
            let line = self.line();
            let hello = self.hello.get_or_insert_with(|| {
                Hello::new(
                    calling,
                    Caps {
                        hears_hz: fs as u32,
                        sends_hz: fs_out as u32,
                        channels: input.len() as u8,
                        wanted,
                        two_wire: false,
                        asks_baud,
                        line,
                        fit: 0,
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
                "V.ARC: agreed on {:.0} Hz out and {:.0} Hz back{}",
                agreed.send_hz,
                agreed.hear_hz,
                if agreed.two_wire { "; each end first looks for its own echo" } else { "" }
            ));
            said.extend(agreed.notes.iter().map(|note| format!("V.ARC: {note}")));
            self.notes.push(said.join("\n"));
            self.agreed = Some(agreed);
        }
        if self.station.is_none() {
            let agreed = match self.agreed.clone() {
                Some(agreed) => agreed,
                None => {
                    // Nobody to agree with: a lead that comes straight
                    // back. What it hears is itself, and that is no echo to
                    // take out.
                    Self::alone(fs, fs_out, self.wanted(), false, self.line() != Line::Sound)
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
        } else if self.greets() && !matches!(self.mode, VarcMode::Linear | VarcMode::G711) {
            // Not while the numbers of the call are the data: they are not
            // sound, and the modes that use them say for themselves when
            // they do not arrive.
            self.slow_down(fs, fs_out, input)?;
        }
        if self.unechoed && self.turn == Turn::Both {
            // The turns are over and this end has no echo. It starts again
            // as an end on a wire each way: at its full level, and with a
            // receiver that did not hear the turns.
            self.unechoed = false;
            if let Some(agreed) = self.agreed.as_mut() {
                agreed.two_wire = false;
            }
            self.station = None;
            self.watch = None;
            self.deaf = 0;
            self.unheard = 0.0;
            self.heard_once = false;
            self.read_before = 0;
            self.trim_said = 0.0;
            return Ok(true);
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
        // A line that was to carry its numbers exactly and has not: neither
        // end can read the other, and both find that out at about the same
        // time, so both give up on it at about the same time and start
        // again as the modulation, which only wants the line to be clean.
        // What the two ends agreed in the handshake stands.
        if matches!(self.mode, VarcMode::Linear | VarcMode::G711) && self.first_loaded.is_none() {
            let waited = (self.samples - self.built_at) as f64 / fs;
            let heard = station.channels.iter().any(|c| c.rx.health().locked);
            if (!heard && waited > EXACT_UNHEARD) || waited > EXACT_UNAGREED {
                self.inexact = true;
                self.notes.push(if self.mode == VarcMode::G711 {
                    "V.ARC: the call does not carry G.711 codewords as they were sent; carrying on as the multicarrier mode".to_owned()
                } else {
                    "V.ARC: the line does not carry samples exactly; carrying on as the multicarrier mode".to_owned()
                });
                self.station = None;
                self.watch = None;
                self.deaf = 0;
                self.unheard = 0.0;
                self.heard_once = false;
                self.read_before = 0;
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
        // Each way for itself: a channel may be loaded one way only.
        for channel in &station.channels {
            if channel.hears_loaded() {
                rx += channel.rx.rates().1;
            }
            if channel.sends_loaded() {
                tx += channel.tx.rates().1;
            }
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
            return hello.stage();
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
        // The channel carrying most, so that a channel still on its
        // starting table is not what is shown for one that is loaded.
        let mut most = -1.0;
        for channel in &mut station.channels {
            let points = channel.rx.points();
            let carries = channel.rx.rates().1;
            if !points.is_empty() && carries > most {
                most = carries;
                taken = (points, channel.rx.shown_bits());
            }
        }
        taken
    }

    /// The Reed-Solomon code on what is arriving, (n, k), on the first
    /// channel that is hearing a table it knows.
    pub fn code(&self) -> Option<(usize, usize)> {
        // Of the channel carrying most, as the symbol diagram is.
        let channels = &self.station.as_ref()?.channels;
        let best = channels.iter().max_by(|a, b| a.rx.rates().1.total_cmp(&b.rx.rates().1))?;
        best.rx.code().or_else(|| channels.iter().find_map(|c| c.rx.code()))
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
            match caps.wanted {
                Wanted::Dmt => "multicarrier",
                Wanted::Qam => "QAM",
                Wanted::Pcm => "baseband levels",
            }
            .to_owned()
        };
        // While it is still going on, what has arrived so far.
        let far = self.hello.as_ref().and_then(|hello| hello.theirs().copied()).or(self.far);
        match far {
            Some(caps) => {
                rows.push(("V.ARC hello", said(&caps)));
                rows.push(("asked for", asked(&caps)));
                if let Some(hello) = self.hello.as_ref() {
                    rows.push(("transaction", hello.stage().trim_start_matches("V.ARC: ").to_owned()));
                }
            }
            None if self.hello.is_some() => {
                let (ours, theirs) = if self.calling { (1200, 2400) } else { (2400, 1200) };
                rows.push(("V.ARC hello", format!("carrier on {ours} Hz, listening on {theirs} Hz: nothing heard yet")));
                if let Some(hello) = self.hello.as_ref() {
                    rows.push(("transaction", hello.stage().trim_start_matches("V.ARC: ").to_owned()));
                }
            }
            None => {}
        }
        if let (Some(agreed), true) = (self.agreed.as_ref(), self.far.is_some()) {
            rows.push((
                "agreed",
                format!(
                    "{:.0} Hz out and {:.0} Hz back, {}",
                    agreed.send_hz,
                    agreed.hear_hz,
                    if agreed.two_wire { "with an echo canceller" } else { "no echo canceller" }
                ),
            ));
            for note in &agreed.notes {
                rows.push(("note", note.clone()));
            }
            if times(self.hears_slowed) > 1 || times(self.sends_slowed) > 1 {
                let times = |by: u8| if times(by) == 1 { "at full speed".to_owned() } else { format!("{} times slower", times(by)) };
                rows.push((
                    "slowed",
                    format!("hearing {}, sending {}", times(self.hears_slowed), times(self.sends_slowed)),
                ));
            }
            let (hears, sends) = self.bauds();
            if hears > 0 {
                rows.push(("baud", format!("{hears} arriving, {sends} going")));
            }
        }
        rows
    }

    /// Whether the call is still in its handshake.
    pub fn greeting(&self) -> bool {
        self.hello.is_some()
    }

    /// The handshake's bits as heard since this was last asked: one for a
    /// carrier that stayed as it was, minus one for one that turned over.
    pub fn take_symbols(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.symbols)
    }

    /// Signal to noise at the slicer on the channel carrying most, in
    /// decibels, once the line has been measured.
    pub fn best_snr_db(&self) -> Option<f64> {
        let channels = &self.station.as_ref()?.channels;
        let best = channels.iter().max_by(|a, b| a.rx.rates().1.total_cmp(&b.rx.rates().1))?;
        let health = best.rx.health();
        health.steady.then_some(health.snr_db)
    }

    /// The signal to noise ratio on each channel, in decibels.
    pub fn snr_db(&self) -> Vec<f64> {
        self.station.as_ref().map_or_else(Vec::new, |s| s.channels.iter().map(|c| c.rx.health().snr_db).collect())
    }
}
