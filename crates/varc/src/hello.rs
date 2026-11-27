//! The handshake: what each end is, said before anything else, in a way any
//! line will carry.
//!
//! V.ARC's modulations are built on the sound card's own rate, and two ends
//! need not have the same card. One hears at 192 kHz and sends at 48; the
//! other has one device both ways; one of them has been told the line is a
//! single pair of wires and the other has not. Nothing in a wideband signal
//! can settle that, because until it is settled neither end knows what
//! signal to listen for.
//!
//! So a call begins with a transaction in the middle of the voice band. Its
//! shape is that of V.8 bis: a capabilities list, a mode select, and an
//! acknowledgement.
//!
//! - **300 bits a second, differential binary phase shift keying.** The
//!   caller sends on a 1200 Hz carrier and the answerer on 2400 Hz. A nought
//!   turns the carrier over and a one leaves it. A root raised cosine with
//!   an excess bandwidth of a half shapes each symbol, so each direction
//!   keeps to 450 Hz. An end on a two-wire line hears the far end through
//!   its own signal with no echo canceller.
//! - **HDLC frames**, as V.8 bis has them: flags between frames, a nought
//!   stuffed after five ones, and a 16-bit frame check sequence.
//! - **Three messages.** The caller sends CL: the rates it hears and sends
//!   at, its channels, the modulation it was asked for, its wires, its line
//!   and the fit its band passes. The answerer has both ends' capabilities
//!   then. It works out the mode ([`agree`]) and sends MS: its own
//!   capabilities and the mode it selected. The caller checks the selection
//!   against its own answer and sends ACK. Each end repeats its message
//!   until the reply arrives.
//!
//! The answerer starts the mode when it has the ACK. The caller starts it
//! after it has sent the ACK three times.

use std::collections::VecDeque;
use std::sync::OnceLock;

use crate::link::{Receiver, Sender};

/// Symbols a second, and bits a second.
pub const BAUD: f64 = 300.0;

/// The carriers: the caller's, then the answerer's.
pub const CALLER_HZ: f64 = 1200.0;
pub const ANSWERER_HZ: f64 = 2400.0;

/// How loud: the amplitude of the carrier under a symbol at its middle.
/// The signal is -15 dBFS RMS.
const LEVEL: f64 = 0.25;

/// The excess bandwidth of the pulse, and how many symbols it lasts each
/// side of its middle.
const ROLLOFF: f64 = 0.5;
const HALF: usize = 5;

/// Points of the pulse that are kept for each symbol.
const FINE: usize = 64;

/// How far the symbol clock moves for a whole error, in symbols.
const CLOCK_GAIN: f64 = 0.04;

/// The revision of the messages, in the high half of the first octet.
const REVISION: u8 = 1;

/// The message types, in the low half of the first octet. MS, CL, ACK(1)
/// and NAK(1) have the codes of V.8 bis (Table 5).
const MS: u8 = 0b0001;
const CL: u8 = 0b0010;
const ACK: u8 = 0b0100;
const NAK: u8 = 0b1000;
/// Not a V.8 bis message: see [`Watch`].
const SLOWED: u8 = 0b1110;

/// The selection in an MS when the two ends have no mode in common.
const NO_MODE: u8 = 0xff;

/// Flags between one message and the next.
const BETWEEN: usize = 2;

/// Times the caller sends its ACK.
const ACKS: usize = 3;

/// How long the answerer waits for an ACK after the caller's last message,
/// in seconds. A caller that sent its ACK and went ahead is silent on its
/// carrier from then on.
const PATIENCE: f64 = 4.0;

/// How long an end on a narrow line waits to measure the far end's probe
/// before it gives up and says its message without, in seconds.
const UNPROBED: f64 = 6.0;

/// How long the far end's carrier is heard with no probe under it before
/// the far end is taken to send none, in seconds.
const BARE: f64 = 0.8;

/// The modulations a call may be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Dmt,
    Qam,
    Pcm,
}

/// What the line of an end is made of.
///
/// A sound card carries a waveform. A SIP call carries numbers, and between
/// two ends that have the same kind of call, with nothing that recodes them
/// on the way, the numbers arrive as they were sent. The two ends can then
/// try to use the numbers as the data. They try only when both say the same
/// kind: an end on a sound card behind a gateway hears a waveform whatever
/// the far end has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Line {
    /// A sound card, or anything else that carries a waveform.
    #[default]
    Sound,
    /// A call of sixteen-bit linear samples.
    Samples,
    /// A call in G.711, A-law.
    ALaw,
    /// A call in G.711, mu-law.
    MuLaw,
}

/// What an end is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// The rates this end's sound devices run at, hearing and sending.
    pub hears_hz: u32,
    pub sends_hz: u32,
    pub channels: u8,
    pub wanted: Wanted,
    /// This end was told the line is one pair of wires.
    pub two_wire: bool,
    /// The baud this end wants to be sent to at, where somebody has chosen
    /// one for a single-carrier mode; nought for whatever the line bears.
    pub asks_baud: u32,
    /// What the line of this end is made of.
    pub line: Line,
    /// The widest fit of a single carrier that the band this end hears
    /// will pass, as the probe measured it: one more than its place in
    /// [`FITS`], and nought for none, which is the carrier as it is built.
    pub fit: u8,
}

/// The octets of a set of capabilities in a message.
const CAPS: usize = 12;

impl Caps {
    fn encode(&self) -> [u8; CAPS] {
        let mut out = [0u8; CAPS];
        let line = match self.line {
            Line::Sound => 0,
            Line::Samples => 1,
            Line::ALaw => 2,
            Line::MuLaw => 3,
        };
        out[0] = u8::from(self.two_wire) | line << 2 | (self.fit & 15) << 4;
        out[1] = wanted_code(self.wanted);
        out[2] = self.channels;
        out[3..6].copy_from_slice(&self.hears_hz.to_le_bytes()[..3]);
        out[6..9].copy_from_slice(&self.sends_hz.to_le_bytes()[..3]);
        out[9..12].copy_from_slice(&self.asks_baud.to_le_bytes()[..3]);
        out
    }

    fn decode(body: &[u8]) -> Option<Self> {
        if body.len() != CAPS || body[0] & 2 != 0 {
            return None;
        }
        let three = |at: usize| u32::from_le_bytes([body[at], body[at + 1], body[at + 2], 0]);
        Some(Self {
            two_wire: body[0] & 1 != 0,
            line: match body[0] >> 2 & 3 {
                1 => Line::Samples,
                2 => Line::ALaw,
                3 => Line::MuLaw,
                _ => Line::Sound,
            },
            fit: body[0] >> 4,
            wanted: match body[1] {
                0 => Wanted::Dmt,
                1 => Wanted::Qam,
                2 => Wanted::Pcm,
                _ => return None,
            },
            channels: body[2],
            hears_hz: three(3),
            sends_hz: three(6),
            asks_baud: three(9),
        })
    }
}

fn wanted_code(wanted: Wanted) -> u8 {
    match wanted {
        Wanted::Dmt => 0,
        Wanted::Qam => 1,
        Wanted::Pcm => 2,
    }
}

/// A message of the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Message {
    /// The caller's capabilities.
    Cl(Caps),
    /// The answerer's capabilities, and the mode it selected: see
    /// [`selection`].
    Ms(Caps, u8),
    Ack,
    Nak,
    /// Later in a call: the far end listens for the modulation slowed by
    /// two to this power. See [`Watch`].
    Slowed(u8),
}

impl Message {
    /// The octets of the message, with no frame check sequence.
    fn encode(&self) -> Vec<u8> {
        let first = |kind: u8| kind | REVISION << 4;
        match self {
            Self::Cl(caps) => [&[first(CL)][..], &caps.encode()].concat(),
            Self::Ms(caps, selected) => [&[first(MS)][..], &caps.encode(), &[*selected]].concat(),
            Self::Ack => vec![first(ACK)],
            Self::Nak => vec![first(NAK)],
            Self::Slowed(by) => vec![first(SLOWED), *by],
        }
    }

    fn decode(octets: &[u8]) -> Option<Self> {
        let (&first, body) = octets.split_first()?;
        if first >> 4 != REVISION {
            return None;
        }
        match (first & 15, body.len()) {
            (CL, CAPS) => Caps::decode(body).map(Self::Cl),
            (MS, len) if len == CAPS + 1 => Caps::decode(&body[..CAPS]).map(|caps| Self::Ms(caps, body[CAPS])),
            (ACK, 0) => Some(Self::Ack),
            (NAK, 0) => Some(Self::Nak),
            (SLOWED, 1) => Some(Self::Slowed(body[0])),
            _ => None,
        }
    }

    /// The bits of the message as a frame: a flag, the octets and their
    /// frame check sequence with a nought after each five ones, and a flag.
    fn frame(&self) -> Vec<bool> {
        let mut octets = self.encode();
        octets.extend(crc16(&octets).to_le_bytes());
        let mut bits = flags(1);
        let mut ones = 0;
        for octet in octets {
            for i in 0..8 {
                let bit = octet >> i & 1 != 0;
                bits.push(bit);
                ones = if bit { ones + 1 } else { 0 };
                if ones == 5 {
                    bits.push(false);
                    ones = 0;
                }
            }
        }
        bits.extend(flags(1));
        bits
    }
}

/// The bits of `count` flags.
fn flags(count: usize) -> Vec<bool> {
    (0..8 * count).map(|i| !matches!(i % 8, 0 | 7)).collect()
}

/// CRC-16/CCITT, as X.25 and V.8 bis have it.
fn crc16(octets: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &octet in octets {
        crc ^= u16::from(octet);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    !crc
}

/// What the two ends will do, worked out the same at both from the same
/// two sets of capabilities.
#[derive(Debug, Clone, PartialEq)]
pub struct Agreed {
    pub wanted: Wanted,
    /// The line is run as one pair of wires, with echo cancelling.
    pub two_wire: bool,
    /// The rate the modulation runs at in each direction, as this end sees
    /// them: what it sends at and what it hears at. Each is the slower of
    /// the two devices on that direction; the end with the faster device
    /// converts.
    pub send_hz: f64,
    pub hear_hz: f64,
    /// How many of this end's samples there are to one of the modulation's,
    /// sending and hearing.
    pub send_times: usize,
    pub hear_times: usize,
    /// The baud each end asked to be sent to at, nought where it left it to
    /// the line: what this end is to send at, and what it asked for itself.
    pub send_baud: u32,
    pub hear_baud: u32,
    /// The fit of a single carrier each direction's band will pass, as the
    /// end that hears it measured: what this end is to send with, and what
    /// it asked for itself. One more than a place in [`FITS`]; nought for
    /// the carrier as it is built.
    pub send_fit: u8,
    pub hear_fit: u8,
    /// Both ends are on the same kind of SIP call, at one rate, so the
    /// numbers the call carries can be tried as the data.
    pub direct: bool,
    /// Both ends are on a call in G.711, in either law, and asked for the
    /// baseband levels with no baud chosen: the levels are the codec's
    /// own, one in each sample. See [`crate::locked`].
    pub locked: bool,
    /// Anything asked for that could not be had, for whoever is watching.
    pub notes: Vec<String>,
}

/// The mode an MS selects, in one octet: the modulation, the wires, and
/// whether the numbers of the call are tried as the data. Both ends get the
/// same octet from [`agree`], whichever of them asks.
fn selection(agreed: &Result<Agreed, String>) -> u8 {
    match agreed {
        Ok(agreed) => {
            wanted_code(agreed.wanted) | u8::from(agreed.two_wire) << 2 | u8::from(agreed.direct) << 3 | u8::from(agreed.locked) << 4
        }
        Err(_) => NO_MODE,
    }
}

/// Work out what a call between `mine` and `theirs` will be.
pub fn agree(mine: &Caps, theirs: &Caps) -> Result<Agreed, String> {
    let mut notes = Vec::new();
    let pair = |fast_or_slow: u32, other: u32, way: &str| -> Result<(f64, usize), String> {
        let (low, high) = (fast_or_slow.min(other), fast_or_slow.max(other));
        if low == 0 || high % low != 0 || high / low > 16 {
            return Err(format!(
                "the two ends' sound cards cannot be matched {way}: {fast_or_slow} Hz at this end and {other} Hz at the far end, and one has to be a whole number of times the other"
            ));
        }
        Ok((f64::from(low), (fast_or_slow / low) as usize))
    };
    let (send_hz, send_times) = pair(mine.sends_hz, theirs.hears_hz, "on the way out")?;
    let (hear_hz, hear_times) = pair(mine.hears_hz, theirs.sends_hz, "on the way back")?;
    if send_times > 1 || hear_times > 1 || mine.sends_hz != theirs.hears_hz || mine.hears_hz != theirs.sends_hz {
        notes.push(format!(
            "the ends' sound cards differ: {send_hz:.0} Hz on the way out and {hear_hz:.0} Hz on the way back"
        ));
    }

    // Every call on a sound card looks for an echo: each end has the line
    // to itself for a moment, and an end that hears itself keeps a
    // canceller. Nobody has to say what the line is. It was a choice, and a
    // line with a wire each way that was told it was one pair lost half its
    // rate to a canceller with nothing to cancel; now that end drops it.
    // The bit in the message that asked for this is not looked at.
    //
    // Only if both ends can cancel an echo, which wants one sound card:
    // hearing and sending at one rate.
    let able = mine.hears_hz == mine.sends_hz && theirs.hears_hz == theirs.sends_hz;
    // A call made of packets has a path each way and no echo to cancel.
    let packets = mine.line != Line::Sound && theirs.line != Line::Sound;
    if !packets && !able {
        notes.push("an end hears and sends on different sound cards, which cannot cancel an echo: no echo is looked for".to_owned());
    }

    let wanted = if mine.wanted != theirs.wanted {
        notes.push("the ends asked for different modulations: running the multicarrier one".to_owned());
        Wanted::Dmt
    } else {
        mine.wanted
    };
    // A fit is for a single carrier on a narrow line that both ends run at
    // one rate. A sound card has room for the carrier as it is built.
    let fits = wanted == Wanted::Qam && send_times == 1 && hear_times == 1 && send_hz <= NARROW && hear_hz <= NARROW;
    let fit = |asked: u8| if fits && usize::from(asked) <= FITS.len() { asked } else { 0 };
    Ok(Agreed {
        wanted,
        send_fit: fit(theirs.fit),
        hear_fit: fit(mine.fit),
        two_wire: able && !packets,
        direct: mine.line != Line::Sound
            && mine.line == theirs.line
            && send_times == 1
            && hear_times == 1
            && send_hz == hear_hz,
        locked: wanted == Wanted::Pcm
            && matches!(mine.line, Line::ALaw | Line::MuLaw)
            && matches!(theirs.line, Line::ALaw | Line::MuLaw)
            && send_times == 1
            && hear_times == 1
            && send_hz == 8000.0
            && hear_hz == 8000.0
            && mine.asks_baud == 0
            && theirs.asks_baud == 0,
        send_hz,
        hear_hz,
        send_times,
        hear_times,
        send_baud: theirs.asks_baud,
        hear_baud: mine.asks_baud,
        notes,
    })
}

/// The fastest a line runs for it to be probed, in samples a second.
///
/// A single carrier is built for a third of the line's rate in baud and
/// four-fifths of its band. On a sound card that is what the card passes
/// cleanly. On a call at 8 or 16 kHz it leaves a fifth of a band that was
/// narrow to begin with, and the call passes nearly all of it.
pub const NARROW: f64 = 16_000.0;

/// The ways a single carrier is fitted to a narrow line, the fastest first:
/// the baud and the carrier as shares of the line's sample rate, and the
/// excess bandwidth. At 8 kHz they are 3700, 3429 and 3000 baud, where the
/// carrier as it is built is 2667.
///
/// | Baud at 8 kHz | Carrier | Band |
/// |---|---|---|
/// | 3700 | 2000 Hz | 58 to 3942 Hz |
/// | 3429 | 2000 Hz | 114 to 3886 Hz |
/// | 3000 | 1900 Hz | 100 to 3700 Hz |
pub const FITS: [(f64, f64, f64); 3] = [(0.4625, 0.25, 0.05), (3.0 / 7.0, 0.25, 0.1), (0.375, 0.2375, 0.2)];

/// The probe's tones, as shares of the sample rate: two in the middle of
/// the band, which any line passes and the rest are measured against; two
/// at the bottom; three at the top. At 8 kHz: 1800 and 3000 Hz, 100 and
/// 200 Hz, 3600, 3800 and 3900 Hz.
///
/// The two in the middle are clear of the handshake's two carriers, at
/// 8 kHz and at 16 kHz: each is 375 Hz or more from the edge of a carrier's
/// band.
const PROBE: [f64; 7] = [0.225, 0.375, 0.0125, 0.025, 0.45, 0.475, 0.4875];

/// The level of each of the probe's tones.
const PROBE_LEVEL: f64 = 0.04;

/// How far below the middle of the band a tone at its edge may arrive and
/// the edge still be counted as band: 6 dB, as a ratio of powers.
const PASSES: f64 = 0.25;

/// The probe's ear: how loud each of the far end's probe tones arrives.
///
/// The handshake's carriers cross any line. What a single carrier needs to
/// know is how much more of the band the line passes than those do, and
/// nothing in the handshake says. So each end sends, under its handshake, a
/// few faint steady tones at the edges of the band, and each end measures
/// the other's. That is V.34's line probe (10.1.2.3) in miniature.
struct Probe {
    /// Samples in a tenth of a second, over which every tone is a whole
    /// number of cycles; and how many have been heard of this one.
    block: usize,
    heard: usize,
    turn: [f64; 7],
    step: [f64; 7],
    sums: [(f64, f64); 7],
    /// The power of each tone over the last three tenths.
    last: [[f64; 7]; 3],
    blocks: usize,
    /// The fit the line will pass, once it has been steady long enough to
    /// say.
    fit: Option<u8>,
}

impl Probe {
    fn new(fs: f64) -> Self {
        let mut step = [0.0; 7];
        for (step, share) in step.iter_mut().zip(PROBE) {
            *step = std::f64::consts::TAU * share;
        }
        Self {
            block: (fs / 10.0).round() as usize,
            heard: 0,
            turn: [0.0; 7],
            step,
            sums: [(0.0, 0.0); 7],
            last: [[0.0; 7]; 3],
            blocks: 0,
            fit: None,
        }
    }

    fn hear(&mut self, x: f64) {
        for i in 0..7 {
            self.sums[i].0 += x * self.turn[i].cos();
            self.sums[i].1 += x * self.turn[i].sin();
            self.turn[i] = (self.turn[i] + self.step[i]) % std::f64::consts::TAU;
        }
        self.heard += 1;
        if self.heard < self.block {
            return;
        }
        self.heard = 0;
        let scale = 2.0 / self.block as f64;
        let mut now = [0.0; 7];
        for (power, sum) in now.iter_mut().zip(&mut self.sums) {
            *power = (sum.0 * sum.0 + sum.1 * sum.1) * scale * scale;
            *sum = (0.0, 0.0);
        }
        self.last.rotate_left(1);
        self.last[2] = now;
        self.blocks += 1;
        // The middle of the band, three tenths running, there and steady:
        // the far end's probe, whole, and not the start of it.
        let middle = |block: &[f64; 7]| (block[0] + block[1]) / 2.0;
        let (least, most) = self.last.iter().map(middle).fold((f64::MAX, 0.0f64), |(a, b), m| (a.min(m), b.max(m)));
        if self.blocks < 3 || least < 1e-9 || most > 2.0 * least {
            return;
        }
        let mean = |tone: usize| self.last.iter().map(|block| block[tone]).sum::<f64>() / 3.0;
        let reference = (mean(0) + mean(1)) / 2.0;
        // The two in the middle should arrive alike. Where one is far
        // above the other, what is being heard is not a probe.
        if mean(0) > 8.0 * mean(1) || mean(1) > 8.0 * mean(0) {
            return;
        }
        let passes = |tone: usize| mean(tone) >= PASSES * reference;
        self.fit = Some(if passes(2) && passes(6) {
            1
        } else if passes(3) && passes(5) {
            2
        } else if passes(3) && passes(4) {
            3
        } else {
            0
        });
    }
}

/// The pulse of one symbol: a root raised cosine, `x` symbols from its
/// middle. The last two symbols each side are tapered to nothing.
fn pulse(x: f64) -> f64 {
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let (a, pi) = (ROLLOFF, std::f64::consts::PI);
        let half = HALF as f64;
        (0..=2 * HALF * FINE)
            .map(|i| {
                let t = i as f64 / FINE as f64 - half;
                let rrc = if t.abs() < 1e-9 {
                    1.0 - a + 4.0 * a / pi
                } else if (t.abs() - 1.0 / (4.0 * a)).abs() < 1e-9 {
                    a / 2f64.sqrt() * ((1.0 + 2.0 / pi) * (pi / (4.0 * a)).sin() + (1.0 - 2.0 / pi) * (pi / (4.0 * a)).cos())
                } else {
                    ((pi * t * (1.0 - a)).sin() + 4.0 * a * t * (pi * t * (1.0 + a)).cos()) / (pi * t * (1.0 - (4.0 * a * t).powi(2)))
                };
                let taper = if t.abs() <= half - 2.0 { 1.0 } else { 0.5 + 0.5 * (pi * (t.abs() - (half - 2.0)) / 2.0).cos() };
                rrc * taper
            })
            .collect()
    });
    let at = (x + HALF as f64) * FINE as f64;
    if at < 0.0 || at >= (table.len() - 1) as f64 {
        return 0.0;
    }
    let (whole, part) = (at as usize, at.fract());
    table[whole] + (table[whole + 1] - table[whole]) * part
}

/// One end's voice in the handshake: bits to a shaped carrier.
struct Voice {
    /// The carrier: how far round it is, and how far it goes a sample.
    turn: f64,
    step: f64,
    /// Symbols a sample, and how far into the symbol this sample is.
    pace: f64,
    into: f64,
    /// The last symbols, the newest first: one, minus one, or nought for
    /// silence. And how many of them are not silence.
    past: VecDeque<f64>,
    live: usize,
    /// Which way up the carrier is.
    sign: f64,
    /// The bits still to send.
    bits: VecDeque<bool>,
}

impl Voice {
    fn new(fs: f64, carrier: f64) -> Self {
        Self {
            turn: 0.0,
            step: std::f64::consts::TAU * carrier / fs,
            pace: BAUD / fs,
            into: 1.0,
            past: VecDeque::from(vec![0.0; 2 * HALF]),
            live: 0,
            sign: 1.0,
            bits: VecDeque::new(),
        }
    }

    /// Whether there are no bits left and the last symbol has died away.
    fn silent(&self) -> bool {
        self.bits.is_empty() && self.live == 0
    }

    /// The next sample.
    fn say(&mut self) -> f64 {
        if self.silent() {
            return 0.0;
        }
        self.into += self.pace;
        if self.into >= 1.0 {
            self.into -= 1.0;
            // A nought turns the carrier over. A one leaves it.
            let symbol = match self.bits.pop_front() {
                Some(bit) => {
                    if !bit {
                        self.sign = -self.sign;
                    }
                    self.sign
                }
                None => 0.0,
            };
            if self.past.pop_back().is_some_and(|old| old != 0.0) {
                self.live -= 1;
            }
            self.past.push_front(symbol);
            if symbol != 0.0 {
                self.live += 1;
            }
        }
        let mut level = 0.0;
        for (k, symbol) in self.past.iter().enumerate() {
            if *symbol != 0.0 {
                level += symbol * pulse(self.into + k as f64 - HALF as f64);
            }
        }
        self.turn = (self.turn + self.step) % std::f64::consts::TAU;
        LEVEL * level * self.turn.sin()
    }
}

/// One carrier's receiver on one channel: the bits that arrive, and the
/// messages they make.
///
/// The line is turned down by the carrier and brought to about 10 kHz. Then
/// the pulse's own filter is worked out three times a symbol: on the
/// symbol, and a quarter of a symbol before and after. The two at the sides
/// keep the clock: the energy is greatest on the symbol. A symbol against
/// the one before it is the bit.
struct Ear {
    /// The carrier, as a point that goes round: where it is and how far it
    /// goes a sample.
    carrier: (f64, f64),
    step: (f64, f64),
    turned: u32,
    /// Samples of the line to one kept, and the sum of those so far.
    every: usize,
    sum: (f64, f64),
    summed: usize,
    /// The kept samples, how many there have been, and how many there are
    /// to a symbol.
    kept: Vec<(f64, f64)>,
    count: u64,
    per_symbol: f64,
    /// The place of the next symbol's middle among the kept samples.
    next: f64,
    /// The last symbol.
    last: (f64, f64),
    /// The frame being read: ones in a row, and the bits so far.
    ones: u8,
    frame: Vec<bool>,
    /// Flags in a row with nothing wrong between them.
    flags: u32,
    /// The last bit as it arrived, between minus one and one.
    eye: f64,
}

impl Ear {
    fn new(fs: f64, carrier: f64) -> Self {
        let every = ((fs / 9600.0) as usize).max(1);
        let per_symbol = fs / every as f64 / BAUD;
        let angle = std::f64::consts::TAU * carrier / fs;
        let reach = HALF as f64 * per_symbol;
        Self {
            carrier: (1.0, 0.0),
            step: (angle.cos(), angle.sin()),
            turned: 0,
            every,
            sum: (0.0, 0.0),
            summed: 0,
            kept: vec![(0.0, 0.0); (2.0 * reach + per_symbol / 2.0).ceil() as usize + 8],
            count: 0,
            per_symbol,
            next: reach + per_symbol,
            last: (0.0, 0.0),
            ones: 0,
            frame: Vec::new(),
            flags: 0,
            eye: 0.0,
        }
    }

    /// The pulse's filter with its middle at `at` among the kept samples.
    fn through(&self, at: f64) -> (f64, f64) {
        let reach = HALF as f64 * self.per_symbol;
        let (from, to) = ((at - reach).ceil().max(0.0) as u64, (at + reach).floor() as u64);
        let mut out = (0.0, 0.0);
        for place in from..=to.min(self.count.saturating_sub(1)) {
            let weight = pulse((place as f64 - at) / self.per_symbol);
            let sample = self.kept[(place % self.kept.len() as u64) as usize];
            out.0 += weight * sample.0;
            out.1 += weight * sample.1;
        }
        out
    }

    /// A sample; a message, if this sample finished one.
    fn hear(&mut self, x: f64) -> Option<Message> {
        self.sum.0 += x * self.carrier.0;
        self.sum.1 -= x * self.carrier.1;
        self.carrier = (
            self.carrier.0 * self.step.0 - self.carrier.1 * self.step.1,
            self.carrier.1 * self.step.0 + self.carrier.0 * self.step.1,
        );
        self.turned += 1;
        if self.turned == 4096 {
            // Keep the point on the circle.
            self.turned = 0;
            let size = self.carrier.0.hypot(self.carrier.1);
            self.carrier = (self.carrier.0 / size, self.carrier.1 / size);
        }
        self.summed += 1;
        if self.summed < self.every {
            return None;
        }
        let len = self.kept.len() as u64;
        self.kept[(self.count % len) as usize] = self.sum;
        self.count += 1;
        self.sum = (0.0, 0.0);
        self.summed = 0;

        let reach = HALF as f64 * self.per_symbol;
        let quarter = self.per_symbol / 4.0;
        let mut found = None;
        while (self.count - 1) as f64 >= self.next + quarter + reach {
            let (early, on, late) = (self.through(self.next - quarter), self.through(self.next), self.through(self.next + quarter));
            let power = |z: (f64, f64)| z.0 * z.0 + z.1 * z.1;
            // More energy after the symbol than before it: the clock is
            // early, and the next symbol is looked at later.
            let error = (power(late) - power(early)) / (power(late) + power(early) + 1e-30);
            self.next += self.per_symbol * (1.0 + CLOCK_GAIN * error);
            // The symbol against the one before: the same way up is a one.
            let same = on.0 * self.last.0 + on.1 * self.last.1;
            self.eye = same / ((power(on) * power(self.last)).sqrt() + 1e-30);
            self.last = on;
            if let Some(message) = self.bit(same >= 0.0) {
                found = Some(message);
            }
        }
        found
    }

    /// A bit; a message, if this bit was the end of its closing flag.
    fn bit(&mut self, bit: bool) -> Option<Message> {
        if bit {
            self.ones += 1;
            if self.ones > 6 {
                // Seven ones: an abort, or no signal.
                self.ones = 7;
                self.frame.clear();
                self.flags = 0;
            } else {
                self.frame.push(true);
            }
            return None;
        }
        let ones = std::mem::take(&mut self.ones);
        match ones {
            // A stuffed nought.
            5 => None,
            // A flag. Its first nought and its six ones are on the end of
            // the frame.
            6 => {
                let bits = self.frame.len().saturating_sub(7);
                let found = (bits >= 24 && bits.is_multiple_of(8))
                    .then(|| {
                        let octets: Vec<u8> = self.frame[..bits]
                            .chunks(8)
                            .map(|octet| octet.iter().enumerate().fold(0, |sum, (i, &bit)| sum | u8::from(bit) << i))
                            .collect();
                        let (body, check) = octets.split_at(octets.len() - 2);
                        (crc16(body) == u16::from_le_bytes([check[0], check[1]])).then(|| Message::decode(body)).flatten()
                    })
                    .flatten();
                self.flags = if bits == 0 || found.is_some() { self.flags + 1 } else { 0 };
                self.frame.clear();
                found
            }
            _ => {
                if self.frame.len() < 8 * 64 {
                    self.frame.push(false);
                } else {
                    self.frame.clear();
                    self.flags = 0;
                }
                None
            }
        }
    }
}

/// Where an end is in the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Flags only. The caller measures the line. The answerer waits for the
    /// CL, or has it and measures the line.
    Flags,
    /// The caller sends its CL until the MS arrives.
    Cl,
    /// The answerer sends its MS until the ACK arrives.
    Ms,
    /// The caller has the MS and sends its ACK, or its NAK.
    Reply,
    /// The last symbols die away.
    Closing,
}

/// One end's part of the handshake.
pub struct Hello {
    calling: bool,
    mine: Caps,
    theirs: Option<Caps>,
    fs_in: f64,
    fs_out: f64,
    ears: Vec<Ear>,
    voice: Voice,
    stage: Stage,
    /// The far end refused the selection, or selected a mode this end did
    /// not work out for itself.
    refused: bool,
    /// Samples heard since the far end's last message.
    since: f64,
    done: bool,
    /// The probe's tones this end sends under its own, on a narrow line,
    /// and its ear for the far end's.
    probe_turn: [f64; 7],
    probe: Option<Probe>,
    /// Whether this end has measured the far end's probe, or has given up
    /// on there being one, and so knows what to say in its message.
    ready: bool,
    /// Samples heard, and how many had been when the far end's carrier was
    /// first there.
    heard_for: f64,
    carrier_at: Option<f64>,
}

impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hello")
            .field("mine", &self.mine)
            .field("theirs", &self.theirs)
            .field("stage", &self.stage)
            .finish_non_exhaustive()
    }
}

impl Hello {
    /// The end that called, or the end that answered, with what it is.
    pub fn new(calling: bool, mine: Caps) -> Self {
        let (carrier, listens) = if calling { (CALLER_HZ, ANSWERER_HZ) } else { (ANSWERER_HZ, CALLER_HZ) };
        let (fs_in, fs_out) = (f64::from(mine.hears_hz), f64::from(mine.sends_hz));
        let mut voice = Voice::new(fs_out, carrier);
        // Noughts first: a symbol that turns over each time is what the far
        // end's clock finds soonest.
        voice.bits.extend(std::iter::repeat_n(false, 16));
        Self {
            calling,
            ears: (0..mine.channels.max(1)).map(|_| Ear::new(fs_in, listens)).collect(),
            mine,
            theirs: None,
            fs_in,
            fs_out,
            voice,
            stage: Stage::Flags,
            refused: false,
            since: 0.0,
            done: false,
            probe_turn: [0.0; 7],
            probe: (fs_in <= NARROW).then(|| Probe::new(fs_in)),
            ready: fs_in > NARROW,
            heard_for: 0.0,
            carrier_at: None,
        }
    }

    /// The last bit of the far end's on the first channel as it arrived:
    /// one for a carrier that stayed as it was, minus one for a carrier
    /// that turned over, and between them when there is no carrier. For
    /// drawing.
    pub fn eye(&self) -> f64 {
        self.ears.first().map_or(0.0, |ear| ear.eye)
    }

    /// The far end's capabilities, once they have arrived.
    pub fn theirs(&self) -> Option<&Caps> {
        self.theirs.as_ref()
    }

    /// Whether this end has said all it has to and may start the call.
    pub fn done(&self) -> bool {
        self.done
    }

    /// Where the transaction is, for whoever is watching.
    pub fn stage(&self) -> &'static str {
        match (self.stage, self.calling) {
            (Stage::Flags, true) => "V.ARC: measuring the line before the CL",
            (Stage::Flags, false) if self.theirs.is_some() => "V.ARC: CL received, measuring the line before the MS",
            (Stage::Flags, false) => "V.ARC: waiting for the CL",
            (Stage::Cl, _) => "V.ARC: CL sent, waiting for the MS",
            (Stage::Ms, _) => "V.ARC: CL received, MS sent, waiting for the ACK",
            (_, _) if self.refused => "V.ARC: the mode was refused",
            (_, true) => "V.ARC: MS received, ACK sent",
            (_, false) => "V.ARC: ACK received",
        }
    }

    /// What the two ends will do, once this end has the far end's
    /// capabilities.
    pub fn agreed(&self) -> Option<Result<Agreed, String>> {
        let agreed = agree(&self.mine, self.theirs.as_ref()?);
        Some(match agreed {
            Ok(_) if self.refused => Err("the two ends did not select the same mode: they are not the same version".to_owned()),
            other => other,
        })
    }

    /// A frame as it arrived, a sample a channel.
    pub fn hear(&mut self, input: &[f64]) {
        self.heard_for += 1.0;
        self.since += 1.0;
        let mut arrived = None;
        for (ear, &x) in self.ears.iter_mut().zip(input) {
            if let Some(message) = ear.hear(x) {
                arrived = Some(message);
            }
        }
        if self.carrier_at.is_none() && (arrived.is_some() || self.ears.iter().any(|ear| ear.flags >= 3)) {
            self.carrier_at = Some(self.heard_for);
        }
        if let Some(probe) = self.probe.as_mut().filter(|_| !self.ready) {
            probe.hear(input.iter().sum::<f64>() / input.len().max(1) as f64);
            // The far end's probe measured; or its carrier heard a while
            // with no probe under it, which is a far end that sends none;
            // or nothing in a long while.
            let bare = self.carrier_at.is_some_and(|at| self.heard_for - at > BARE * self.fs_in);
            if let Some(fit) = probe.fit.or((bare || self.heard_for > UNPROBED * self.fs_in).then_some(0)) {
                self.mine.fit = fit;
                self.ready = true;
            }
        }
        if self.done || self.stage == Stage::Closing {
            return;
        }
        match (self.calling, arrived) {
            (false, Some(Message::Cl(caps))) if matches!(self.stage, Stage::Flags | Stage::Ms) => {
                self.theirs = Some(caps);
                self.since = 0.0;
            }
            (false, Some(Message::Ack | Message::Nak)) if self.stage == Stage::Ms => {
                self.refused = arrived == Some(Message::Nak);
                self.close();
            }
            (true, Some(Message::Ms(caps, selected))) if self.stage == Stage::Cl => {
                self.theirs = Some(caps);
                self.refused = selected != selection(&agree(&self.mine, &caps));
                // The message being sent is cut off: the reply is due.
                self.voice.bits.clear();
                self.stage = Stage::Reply;
            }
            _ => {}
        }
        if self.ready && self.stage == Stage::Flags && (self.calling || self.theirs.is_some()) {
            self.stage = if self.calling { Stage::Cl } else { Stage::Ms };
        }
        // A caller that has sent its ACK and gone ahead is silent on its
        // carrier. Its ACK was lost, and it is not waited for.
        if self.stage == Stage::Ms && self.since > PATIENCE * self.fs_in {
            self.close();
        }
    }

    /// Stop: nothing more is sent but the end of the last symbols.
    fn close(&mut self) {
        self.voice.bits.clear();
        self.stage = Stage::Closing;
    }

    /// The next sample to send, the same on every channel.
    pub fn say(&mut self) -> f64 {
        if self.done {
            return 0.0;
        }
        if self.voice.bits.is_empty() {
            match self.stage {
                Stage::Flags => self.voice.bits.extend(flags(1)),
                Stage::Cl => {
                    self.voice.bits.extend(Message::Cl(self.mine).frame());
                    self.voice.bits.extend(flags(BETWEEN));
                }
                Stage::Ms => {
                    let selected = self.theirs.map_or(NO_MODE, |theirs| selection(&agree(&self.mine, &theirs)));
                    self.voice.bits.extend(Message::Ms(self.mine, selected).frame());
                    self.voice.bits.extend(flags(BETWEEN));
                }
                Stage::Reply => {
                    let reply = if self.refused { Message::Nak } else { Message::Ack };
                    self.voice.bits.extend(flags(1));
                    for _ in 0..ACKS {
                        self.voice.bits.extend(reply.frame());
                    }
                    self.stage = Stage::Closing;
                }
                Stage::Closing => {}
            }
        }
        let said = self.voice.say();
        if self.stage == Stage::Closing && self.voice.silent() {
            self.done = true;
            return 0.0;
        }
        // The probe's tones, under whatever else is said, on a narrow line.
        let mut probe = 0.0;
        if self.fs_out <= NARROW {
            for (turn, share) in self.probe_turn.iter_mut().zip(PROBE) {
                *turn = (*turn + std::f64::consts::TAU * share) % std::f64::consts::TAU;
                probe += PROBE_LEVEL * turn.sin();
            }
        }
        said + probe
    }
}

/// An end's ear for the handshake's carrier for the rest of the call, and
/// its voice on its own: for saying, and hearing said, that the modulation
/// is not getting through and should be slowed.
///
/// The handshake's signal gets through lines the modulation does not. A
/// line that carries one way and not the other leaves one end loaded and
/// waiting and the other hearing nothing it can use. The end that hears
/// nothing cannot tell the far end so in the modulation, since it does not
/// know that its own is heard. It can in this.
pub struct Watch {
    ears: Vec<Ear>,
    voice: Voice,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch").field("saying", &self.saying()).finish_non_exhaustive()
    }
}

impl Watch {
    /// For the end that called or the end that answered, hearing on
    /// `channels` channels at `fs_in` and sending at `fs_out`.
    pub fn new(calling: bool, fs_in: f64, fs_out: f64, channels: usize) -> Self {
        let (carrier, listens) = if calling { (CALLER_HZ, ANSWERER_HZ) } else { (ANSWERER_HZ, CALLER_HZ) };
        Self { ears: (0..channels.max(1)).map(|_| Ear::new(fs_in, listens)).collect(), voice: Voice::new(fs_out, carrier) }
    }

    /// A frame as it arrived. What the far end says it is listening for,
    /// if this frame finished its saying so.
    pub fn hear(&mut self, input: &[f64]) -> Option<u8> {
        let mut asked = None;
        for (ear, &x) in self.ears.iter_mut().zip(input) {
            if let Some(Message::Slowed(by)) = ear.hear(x) {
                asked = Some(by);
            }
        }
        asked
    }

    /// Tell the far end this end is listening for the modulation slowed by
    /// two to the `slowed`: three times over, most of a second, during
    /// which [`Watch::say`] has the line.
    pub fn tell(&mut self, slowed: u8) {
        self.voice.bits.clear();
        // Noughts and flags first, for the far end's clock.
        self.voice.bits.extend(std::iter::repeat_n(false, 16));
        self.voice.bits.extend(flags(2));
        for _ in 0..3 {
            self.voice.bits.extend(Message::Slowed(slowed).frame());
            self.voice.bits.extend(flags(1));
        }
    }

    /// Whether there is something still being said.
    pub fn saying(&self) -> bool {
        !self.voice.silent()
    }

    /// The next sample of what is being said; silence when nothing is.
    pub fn say(&mut self) -> f64 {
        self.voice.say()
    }
}

/// A sender whose modulation runs slower than the sound card it sends on:
/// each of its samples becomes `times` of the card's.
#[allow(missing_debug_implementations)]
pub struct Faster {
    inner: Box<dyn Sender>,
    times: usize,
    taps: Vec<f64>,
    /// The modulation's last samples, newest first.
    past: std::collections::VecDeque<f64>,
    phase: usize,
    /// How many times faster than it believes the modulation is being run;
    /// less than one for slower. See [`Faster::paced`].
    pace: f64,
}

/// A receiver whose modulation runs slower than the sound card it hears on:
/// `times` of the card's samples become one of its own.
#[allow(missing_debug_implementations)]
pub struct Slower {
    inner: Box<dyn Receiver>,
    times: usize,
    taps: Vec<f64>,
    /// The card's last samples, newest first: the line's, and this end's
    /// own signal as a canceller in front has placed it.
    past: std::collections::VecDeque<f64>,
    own: std::collections::VecDeque<f64>,
    own_given: bool,
    count: usize,
    /// How many times faster than it believes the modulation is being run.
    pace: f64,
}

/// A sender whose modulation is run at twice the speed it was made for, by
/// sending every other one of its samples.
///
/// Only for a modulation that keeps below half its own band, which the
/// baseband levels do: 9.6 kHz of 24. Halving its samples doubles every
/// frequency in it, to 19.2 kHz of 24, and doubles its baud. One that
/// already fills its band, as the carrier at 10.5 kHz does, would fold over
/// on itself.
#[allow(missing_debug_implementations)]
pub struct Twice {
    inner: Box<dyn Sender>,
    taps: Vec<f64>,
    past: std::collections::VecDeque<f64>,
}

/// The receiver to go with [`Twice`]: each sample heard becomes two of the
/// modulation's.
#[allow(missing_debug_implementations)]
pub struct Half {
    inner: Box<dyn Receiver>,
    taps: Vec<f64>,
    past: std::collections::VecDeque<f64>,
}

impl Twice {
    pub fn new(inner: Box<dyn Sender>) -> Self {
        let taps = between(2);
        Self { past: std::collections::VecDeque::from(vec![0.0; taps.len()]), inner, taps }
    }
}

impl Sender for Twice {
    fn wants(&self) -> Option<crate::frame::Layout> {
        self.inner.wants()
    }
    fn give(&mut self, line: Vec<u8>) {
        self.inner.give(line)
    }
    fn next_sample(&mut self) -> f64 {
        for _ in 0..2 {
            self.past.pop_back();
            let sample = self.inner.next_sample();
            self.past.push_front(sample);
        }
        self.taps.iter().zip(&self.past).map(|(t, x)| t * x).sum()
    }
    fn adopt(&mut self, id: u8, table: crate::link::Table) {
        self.inner.adopt(id, table)
    }
    fn id(&self) -> u8 {
        self.inner.id()
    }
    fn reset(&mut self) {
        self.inner.reset()
    }
    fn rates(&self) -> (f64, f64) {
        let (line, frames) = self.inner.rates();
        (2.0 * line, 2.0 * frames)
    }
    fn clipped(&self) -> u64 {
        self.inner.clipped()
    }
    fn trim(&mut self, db: f64) {
        self.inner.trim(db)
    }
}

impl Half {
    pub fn new(inner: Box<dyn Receiver>) -> Self {
        let taps = between(2);
        Self { past: std::collections::VecDeque::from(vec![0.0; taps.len() / 2 + 1]), inner, taps }
    }
}

impl Receiver for Half {
    fn feed(&mut self, sample: f64) {
        self.past.pop_back();
        self.past.push_front(sample);
        for phase in 0..2 {
            let filled: f64 = self.taps.iter().skip(phase).step_by(2).zip(&self.past).map(|(t, x)| t * x).sum();
            self.inner.feed(2.0 * filled);
        }
    }
    fn take(&mut self) -> Option<crate::link::Arrived> {
        self.inner.take()
    }
    fn proposal(&mut self) -> Option<crate::link::Table> {
        self.inner.proposal()
    }
    fn expect(&mut self, id: u8, table: crate::link::Table) {
        self.inner.expect(id, table)
    }
    fn id(&self) -> Option<u8> {
        self.inner.id()
    }
    fn back_off(&mut self, db: f64) {
        self.inner.back_off(db)
    }
    fn remeasure(&mut self, db: f64) {
        self.inner.remeasure(db)
    }
    fn health(&self) -> crate::link::Health {
        self.inner.health()
    }
    fn rates(&self) -> (f64, f64) {
        let (line, frames) = self.inner.rates();
        (2.0 * line, 2.0 * frames)
    }
    fn spectrum(&self) -> Vec<crate::link::Bin> {
        let mut bins = self.inner.spectrum();
        for bin in &mut bins {
            bin.hz *= 2.0;
        }
        bins
    }
    fn points(&mut self) -> Vec<(f64, f64)> {
        self.inner.points()
    }
    fn shown_bits(&self) -> u8 {
        self.inner.shown_bits()
    }
    fn code(&self) -> Option<(usize, usize)> {
        self.inner.code()
    }
    fn fix(&mut self, bits: Option<u8>, code: Option<f64>) {
        self.inner.fix(bits, code)
    }
}

/// A low-pass for going between a rate and `times` that rate: flat to 0.86
/// of the slower rate's half, which is as far up as the modulation puts
/// anything, and gone by the half itself.
fn between(times: usize) -> Vec<f64> {
    let len = 64 * times + 1;
    let mid = (len / 2) as f64;
    let cutoff = 0.93 * 0.5 / times as f64;
    let beta = 7.0;
    let norm = crate::interp::bessel_i0(beta);
    let mut taps: Vec<f64> = (0..len)
        .map(|i| {
            let x = i as f64 - mid;
            let sinc = if x == 0.0 { 2.0 * cutoff } else { (std::f64::consts::TAU * cutoff * x).sin() / (std::f64::consts::PI * x) };
            let r = x / mid;
            sinc * crate::interp::bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / norm
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

impl Faster {
    pub fn new(inner: Box<dyn Sender>, times: usize) -> Self {
        Self::paced(inner, times, 1.0)
    }

    /// For a modulation slowed `slow` times: see [`Faster::paced`].
    pub fn slowed(inner: Box<dyn Sender>, times: usize, slow: usize) -> Self {
        Self::paced(inner, times, 1.0 / slow as f64)
    }

    /// The same, for a modulation that is being run slower than it was
    /// made for: built for one rate, and given `times` of the card's samples
    /// to each of its own where its own rate would have had fewer. Every
    /// frequency in it comes down by `slow` and every symbol lasts `slow`
    /// times as long, which is how a line that will not pass the band the
    /// modulation wants is given one it will. The modulation knows nothing
    /// of it; only the rates it reports have to be put right.
    ///
    /// More generally, a modulation built for one rate may be run at any
    /// other: `pace` is how many times faster than the rate it believes in
    /// its samples really go by. A single carrier built for 48 kHz and
    /// played sample for sample on a card at 192 kHz is the same carrier at
    /// four times the baud across four times the band, with `times` one and
    /// `pace` four.
    pub fn paced(inner: Box<dyn Sender>, times: usize, pace: f64) -> Self {
        let taps = between(times);
        Self {
            past: std::collections::VecDeque::from(vec![0.0; taps.len() / times + 1]),
            inner,
            times,
            taps,
            phase: 0,
            pace,
        }
    }
}

impl Sender for Faster {
    fn wants(&self) -> Option<crate::frame::Layout> {
        self.inner.wants()
    }
    fn give(&mut self, line: Vec<u8>) {
        self.inner.give(line)
    }
    fn next_sample(&mut self) -> f64 {
        if self.times == 1 {
            // Sample for sample: there is nothing to fill in.
            return self.inner.next_sample();
        }
        if self.phase == 0 {
            self.past.pop_back();
            let sample = self.inner.next_sample();
            self.past.push_front(sample);
        }
        // Every `times`th tap, starting at this phase, against the
        // modulation's samples; and `times` over, since all but one in
        // `times` of the faster rate's samples are the filter's doing.
        let sum: f64 = self.taps.iter().skip(self.phase).step_by(self.times).zip(&self.past).map(|(t, x)| t * x).sum();
        self.phase = (self.phase + 1) % self.times;
        sum * self.times as f64
    }
    fn adopt(&mut self, id: u8, table: crate::link::Table) {
        self.inner.adopt(id, table)
    }
    fn id(&self) -> u8 {
        self.inner.id()
    }
    fn reset(&mut self) {
        self.inner.reset()
    }
    fn rates(&self) -> (f64, f64) {
        let (line, frames) = self.inner.rates();
        (line * self.pace, frames * self.pace)
    }
    fn clipped(&self) -> u64 {
        self.inner.clipped()
    }
    fn trim(&mut self, db: f64) {
        self.inner.trim(db)
    }
}

impl Slower {
    pub fn new(inner: Box<dyn Receiver>, times: usize) -> Self {
        Self::paced(inner, times, 1.0)
    }

    /// The receiving half of [`Faster::slowed`].
    pub fn slowed(inner: Box<dyn Receiver>, times: usize, slow: usize) -> Self {
        Self::paced(inner, times, 1.0 / slow as f64)
    }

    /// The receiving half of [`Faster::paced`].
    pub fn paced(inner: Box<dyn Receiver>, times: usize, pace: f64) -> Self {
        let taps = between(times);
        Self {
            past: std::collections::VecDeque::from(vec![0.0; taps.len()]),
            own: std::collections::VecDeque::from(vec![0.0; taps.len()]),
            own_given: false,
            inner,
            times,
            taps,
            count: 0,
            pace,
        }
    }
}

impl Receiver for Slower {
    fn feed(&mut self, sample: f64) {
        if self.times == 1 {
            // Sample for sample: there is nothing to take out.
            if self.own_given {
                self.inner.echo_under(self.own[0]);
            }
            return self.inner.feed(sample);
        }
        self.past.pop_back();
        self.past.push_front(sample);
        self.count += 1;
        if self.count < self.times {
            return;
        }
        self.count = 0;
        if self.own_given {
            let own: f64 = self.taps.iter().zip(&self.own).map(|(t, x)| t * x).sum();
            self.inner.echo_under(own);
        }
        let line: f64 = self.taps.iter().zip(&self.past).map(|(t, x)| t * x).sum();
        self.inner.feed(line);
    }
    fn echo_under(&mut self, sample: f64) {
        self.own_given = true;
        self.own.pop_back();
        self.own.push_front(sample);
    }
    fn echo_again(&mut self) {
        self.inner.echo_again()
    }
    fn echo_left_db(&self) -> Option<f64> {
        self.inner.echo_left_db()
    }
    fn take(&mut self) -> Option<crate::link::Arrived> {
        self.inner.take()
    }
    fn proposal(&mut self) -> Option<crate::link::Table> {
        self.inner.proposal()
    }
    fn expect(&mut self, id: u8, table: crate::link::Table) {
        self.inner.expect(id, table)
    }
    fn id(&self) -> Option<u8> {
        self.inner.id()
    }
    fn back_off(&mut self, db: f64) {
        self.inner.back_off(db)
    }
    fn remeasure(&mut self, db: f64) {
        self.inner.remeasure(db)
    }
    fn health(&self) -> crate::link::Health {
        self.inner.health()
    }
    fn rates(&self) -> (f64, f64) {
        let (line, frames) = self.inner.rates();
        (line * self.pace, frames * self.pace)
    }
    fn spectrum(&self) -> Vec<crate::link::Bin> {
        let mut bins = self.inner.spectrum();
        for bin in &mut bins {
            bin.hz *= self.pace;
        }
        bins
    }
    fn points(&mut self) -> Vec<(f64, f64)> {
        self.inner.points()
    }
    fn shown_bits(&self) -> u8 {
        self.inner.shown_bits()
    }
    fn code(&self) -> Option<(usize, usize)> {
        self.inner.code()
    }
    fn fix(&mut self, bits: Option<u8>, code: Option<f64>) {
        self.inner.fix(bits, code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(hears: u32, sends: u32) -> Caps {
        Caps { hears_hz: hears, sends_hz: sends, channels: 2, wanted: Wanted::Dmt, two_wire: false, asks_baud: 0, line: Line::Sound, fit: 0 }
    }

    #[test]
    fn a_message_survives_being_written_down() {
        let said = Caps { two_wire: true, wanted: Wanted::Pcm, asks_baud: 8000, line: Line::MuLaw, fit: 3, ..caps(192_000, 48_000) };
        assert_eq!(Caps::decode(&said.encode()), Some(said));
        for message in [Message::Cl(said), Message::Ms(said, 0x0a), Message::Ack, Message::Nak, Message::Slowed(3)] {
            assert_eq!(Message::decode(&message.encode()), Some(message));
        }
        // The types are those of V.8 bis.
        assert_eq!(Message::Ms(said, 0).encode()[0] & 15, 1);
        assert_eq!(Message::Cl(said).encode()[0] & 15, 2);
        assert_eq!(Message::Ack.encode()[0] & 15, 4);
    }

    #[test]
    fn a_frame_is_read_from_its_bits_and_a_spoiled_one_is_not() {
        let said = Caps { asks_baud: 0xff_ffff, ..caps(0xff_ffff, 0x7e_7e7e) };
        let message = Message::Ms(said, NO_MODE);
        let read = |bits: &[bool]| {
            let mut ear = Ear::new(8_000.0, CALLER_HZ);
            bits.iter().filter_map(|&bit| ear.bit(bit)).collect::<Vec<_>>()
        };
        let mut bits = flags(2);
        bits.extend(message.frame());
        bits.extend(Message::Ack.frame());
        // No run of ones in a frame is as long as a flag's.
        let inside = &message.frame()[8..message.frame().len() - 8];
        assert!(!inside.windows(6).any(|run| run.iter().all(|&bit| bit)));
        assert_eq!(read(&bits), vec![message, Message::Ack]);
        bits[40] ^= true;
        assert_eq!(read(&bits), vec![Message::Ack]);
    }

    /// Two ends say hello over a line: `gain` on the way, `noise` on top,
    /// and each end's own signal mixed into what it hears if the line is one
    /// pair of wires.
    fn meet(a: Caps, b: Caps, own: f64, noise: f64) -> (Hello, Hello, f64) {
        // The line is as fast as the faster card on it, and each end's
        // samples are held or skipped to fit. Crude, and enough for this.
        let line = f64::from(a.sends_hz.max(a.hears_hz).max(b.sends_hz).max(b.hears_hz));
        let (mut caller, mut host) = (Hello::new(true, a), Hello::new(false, b));
        let mut seed = 9u64;
        let mut hiss = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            noise * (((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5) * 3.46
        };
        let (mut from_caller, mut from_host) = (0.0, 0.0);
        let mut owed = [0.0f64; 4];
        let rates = [f64::from(a.sends_hz), f64::from(a.hears_hz), f64::from(b.sends_hz), f64::from(b.hears_hz)];
        let mut tick = 0u64;
        while !(caller.done() && host.done()) && tick < (20.0 * line) as u64 {
            tick += 1;
            for (owed, rate) in owed.iter_mut().zip(rates) {
                *owed += rate / line;
            }
            if owed[0] >= 1.0 {
                owed[0] -= 1.0;
                from_caller = caller.say();
            }
            if owed[2] >= 1.0 {
                owed[2] -= 1.0;
                from_host = host.say();
            }
            if owed[1] >= 1.0 {
                owed[1] -= 1.0;
                let x = 0.5 * from_host + own * from_caller + hiss();
                caller.hear(&[x, x]);
            }
            if owed[3] >= 1.0 {
                owed[3] -= 1.0;
                let x = 0.5 * from_caller + own * from_host + hiss();
                host.hear(&[x, x]);
            }
        }
        (caller, host, tick as f64 / line)
    }

    /// A line that passes `low` to `high` hertz and nothing else: a
    /// windowed sinc.
    struct Band {
        taps: Vec<f64>,
        past: std::collections::VecDeque<f64>,
    }

    impl Band {
        fn new(fs: f64, low: f64, high: f64) -> Self {
            let half = 160i64;
            let sinc = |cut: f64, t: f64| if t == 0.0 { 2.0 * cut / fs } else { (std::f64::consts::TAU * cut * t / fs).sin() / (std::f64::consts::PI * t) };
            let taps: Vec<f64> = (-half..=half)
                .map(|i| {
                    let t = i as f64;
                    (sinc(high, t) - sinc(low, t)) * (0.5 + 0.5 * (std::f64::consts::PI * t / (half + 1) as f64).cos())
                })
                .collect();
            Self { past: std::collections::VecDeque::from(vec![0.0; taps.len()]), taps }
        }

        fn pass(&mut self, x: f64) -> f64 {
            self.past.pop_back();
            self.past.push_front(x);
            self.taps.iter().zip(&self.past).map(|(t, p)| t * p).sum()
        }
    }

    /// Two ends at 8 kHz say hello through a line that passes `band`, the
    /// host starting `late` seconds after the caller: what each made of
    /// the band it hears, and how long it took.
    fn probed(band: Option<(f64, f64)>, late: f64, wanted: Wanted) -> (Agreed, Agreed, f64) {
        let fs = 8_000.0;
        let mine = Caps { wanted, channels: 1, line: Line::MuLaw, ..caps(8_000, 8_000) };
        let (mut caller, mut host) = (Hello::new(true, mine), Hello::new(false, mine));
        let mut lines = band.map(|(low, high)| (Band::new(fs, low, high), Band::new(fs, low, high)));
        let (mut from_caller, mut from_host) = (0.0, 0.0);
        let mut tick = 0usize;
        while !(caller.done() && host.done()) && tick < (20.0 * fs) as usize {
            let (to_host, to_caller) = match lines.as_mut() {
                Some((out, back)) => (out.pass(from_caller), back.pass(from_host)),
                None => (from_caller, from_host),
            };
            from_caller = caller.say();
            caller.hear(&[to_caller]);
            if tick as f64 >= late * fs {
                from_host = host.say();
                host.hear(&[to_host]);
            }
            tick += 1;
        }
        assert!(caller.done() && host.done(), "{band:?}: not done");
        (caller.agreed().unwrap().unwrap(), host.agreed().unwrap().unwrap(), tick as f64 / fs)
    }

    #[test]
    fn the_probe_finds_how_much_of_the_band_a_narrow_line_passes() {
        // A call that passes everything, as G.711 between two of these
        // does: the widest fit, each way.
        let (caller, host, took) = probed(None, 0.0, Wanted::Qam);
        assert_eq!((caller.hear_fit, caller.send_fit, host.hear_fit, host.send_fit), (1, 1, 1, 1));
        // The probe, the CL, the MS and the ACK: under three seconds.
        assert!(took < 3.0, "{took} s");
        // An end that starts most of a second after the other.
        let (caller, host, _) = probed(None, 0.8, Wanted::Qam);
        assert_eq!((caller.hear_fit, host.hear_fit), (1, 1));
        // What a telephone network passes: none of them fits, and the
        // carrier runs as it is built.
        let (caller, host, _) = probed(Some((300.0, 3400.0)), 0.0, Wanted::Qam);
        assert_eq!((caller.hear_fit, caller.send_fit, host.hear_fit, host.send_fit), (0, 0, 0, 0));
        // A line that stops between the two.
        let (caller, host, _) = probed(Some((50.0, 3850.0)), 0.3, Wanted::Qam);
        assert_eq!((caller.hear_fit, host.hear_fit), (2, 2));
        let (caller, host, _) = probed(Some((150.0, 3700.0)), 0.0, Wanted::Qam);
        assert_eq!((caller.hear_fit, host.hear_fit), (3, 3));
        // Only a single carrier is fitted; the others take no notice.
        let (caller, host, _) = probed(None, 0.0, Wanted::Dmt);
        assert_eq!((caller.hear_fit, caller.send_fit, host.hear_fit, host.send_fit), (0, 0, 0, 0));
    }

    #[test]
    fn a_far_end_that_sends_no_probe_is_not_waited_for() {
        // A sound card at 48 kHz calling an end at 8 kHz: nothing to fit,
        // and the handshake takes what it took.
        let narrow = Caps { wanted: Wanted::Qam, ..caps(8_000, 8_000) };
        let wide = Caps { wanted: Wanted::Qam, ..caps(48_000, 48_000) };
        for (a, b) in [(narrow, wide), (wide, narrow)] {
            let (caller, host, took) = meet(a, b, 0.0, 0.0);
            assert!(caller.done() && host.done());
            assert!(took < 4.0, "{took} s");
            let agreed = caller.agreed().unwrap().unwrap();
            assert_eq!((agreed.hear_fit, agreed.send_fit), (0, 0));
        }
    }

    #[test]
    fn two_ends_learn_what_each_other_are_and_agree() {
        // The rig that started this: each end hears on one device and sends
        // on another, at different rates, and both were told two-wire.
        let a = Caps { two_wire: true, ..caps(48_000, 192_000) };
        let b = Caps { two_wire: true, ..caps(192_000, 48_000) };
        let (caller, host, took) = meet(a, b, 0.0, 0.0);
        assert!(caller.done() && host.done(), "after {took:.1} s");
        // A CL, an MS and three ACKs, with no probe to wait for.
        assert!(took < 2.0, "took {took:.1} s");
        assert_eq!(caller.theirs().map(|c| (c.hears_hz, c.sends_hz)), Some((192_000, 48_000)));
        assert_eq!(host.theirs().map(|c| (c.hears_hz, c.sends_hz)), Some((48_000, 192_000)));
        assert_eq!((caller.stage(), host.stage()), ("V.ARC: MS received, ACK sent", "V.ARC: ACK received"));
        let (ours, theirs) = (caller.agreed().unwrap().unwrap(), host.agreed().unwrap().unwrap());
        // No echo is looked for, and each direction is at its own rate.
        assert!(!ours.two_wire && !theirs.two_wire);
        assert_eq!((ours.send_hz, ours.hear_hz), (192_000.0, 48_000.0));
        assert_eq!((theirs.send_hz, theirs.hear_hz), (48_000.0, 192_000.0));
        assert_eq!((ours.send_times, ours.hear_times), (1, 1));
        assert!(ours.notes.iter().any(|n| n.contains("cannot cancel an echo")), "{:?}", ours.notes);
    }

    /// Two ends at 48 kHz, a sample at a time, with a line each way that
    /// `carries` says is whole or cut: how long each took to be done.
    fn in_step(mut carries: impl FnMut(&Hello, &Hello) -> (bool, bool), seconds: f64) -> (Hello, Hello, Option<f64>, Option<f64>) {
        let fs = 48_000.0;
        let (mut caller, mut host) = (Hello::new(true, caps(48_000, 48_000)), Hello::new(false, caps(48_000, 48_000)));
        let (mut caller_done, mut host_done) = (None, None);
        for tick in 0..(seconds * fs) as usize {
            let (out, back) = carries(&caller, &host);
            let (from_caller, from_host) = (caller.say(), host.say());
            caller.hear(&[if back { from_host } else { 0.0 }; 2]);
            host.hear(&[if out { from_caller } else { 0.0 }; 2]);
            if caller.done() && caller_done.is_none() {
                caller_done = Some(tick as f64 / fs);
            }
            if host.done() && host_done.is_none() {
                host_done = Some(tick as f64 / fs);
            }
        }
        (caller, host, caller_done, host_done)
    }

    #[test]
    fn each_message_is_repeated_until_its_reply_arrives() {
        // Nothing gets back to the caller for two seconds: it sends its CL
        // all that time, the answerer its MS, and nobody is done.
        let mut ticks = 0;
        let (caller, host, caller_done, host_done) = in_step(
            |caller, host| {
                ticks += 1;
                if ticks == 2 * 48_000 {
                    assert_eq!(caller.stage(), "V.ARC: CL sent, waiting for the MS");
                    assert_eq!(host.stage(), "V.ARC: CL received, MS sent, waiting for the ACK");
                    assert!(caller.theirs().is_none() && host.theirs().is_some());
                }
                (true, ticks > 2 * 48_000)
            },
            4.0,
        );
        assert!(caller.done() && host.done());
        // The answerer is done when it has the ACK, which is before the
        // caller has sent its last.
        assert!(caller_done.unwrap() > 2.0 && host_done.unwrap() > 2.0);
        assert!(host_done.unwrap() < caller_done.unwrap() + 0.1);
    }

    #[test]
    fn an_answerer_whose_ack_is_lost_goes_on_when_the_caller_is_silent() {
        // The line to the answerer is cut when the caller has the MS.
        let (caller, host, caller_done, host_done) = in_step(|caller, _| (caller.theirs().is_none(), true), 8.0);
        assert!(caller.done() && host.done());
        assert!(host_done.unwrap() > caller_done.unwrap() + PATIENCE - 1.0, "{caller_done:?} {host_done:?}");
        assert_eq!(caller.agreed().unwrap().unwrap().wanted, host.agreed().unwrap().unwrap().wanted);
    }

    #[test]
    fn a_selection_this_end_did_not_work_out_is_refused() {
        // An answerer of some other version, which selects what this
        // caller does not: the caller says NAK, and neither end starts.
        let fs = 48_000.0;
        let mut caller = Hello::new(true, caps(48_000, 48_000));
        let mut far = Voice::new(fs, ANSWERER_HZ);
        let mut ear = Ear::new(fs, CALLER_HZ);
        let mut replies = Vec::new();
        for _ in 0..(3.0 * fs) as usize {
            if far.bits.is_empty() {
                far.bits.extend(Message::Ms(caps(48_000, 48_000), 2).frame());
            }
            caller.hear(&[far.say(); 2]);
            replies.extend(ear.hear(caller.say()));
        }
        assert!(caller.done());
        assert!(replies.contains(&Message::Nak) && !replies.contains(&Message::Ack), "{replies:?}");
        assert!(caller.agreed().unwrap().is_err());
    }

    #[test]
    fn the_numbers_of_a_call_are_tried_only_between_two_ends_on_the_same_kind_of_call() {
        let on = |line: Line| Caps { line, ..caps(8_000, 8_000) };
        for line in [Line::Samples, Line::ALaw, Line::MuLaw] {
            assert!(agree(&on(line), &on(line)).unwrap().direct, "{line:?}");
            // What the line is made of crosses in the message.
            assert_eq!(Caps::decode(&on(line).encode()), Some(on(line)));
        }
        // A sound card each end, or one behind a gateway, or a gateway
        // that changes one law into the other: a waveform.
        assert!(!agree(&on(Line::Sound), &on(Line::Sound)).unwrap().direct);
        assert!(!agree(&on(Line::MuLaw), &on(Line::Sound)).unwrap().direct);
        assert!(!agree(&on(Line::ALaw), &on(Line::MuLaw)).unwrap().direct);
        // And not across a change of rate.
        let fast = Caps { line: Line::Samples, ..caps(16_000, 16_000) };
        assert!(!agree(&fast, &on(Line::Samples)).unwrap().direct);
        // The codec's own levels are for the baseband mode on G.711, in
        // one law or two, where nobody chose a baud.
        let levels = |line: Line| Caps { wanted: Wanted::Pcm, ..on(line) };
        assert!(agree(&levels(Line::MuLaw), &levels(Line::ALaw)).unwrap().locked);
        assert!(!agree(&levels(Line::MuLaw), &levels(Line::Sound)).unwrap().locked);
        assert!(!agree(&levels(Line::Samples), &levels(Line::Samples)).unwrap().locked);
        assert!(!agree(&on(Line::MuLaw), &on(Line::MuLaw)).unwrap().locked);
        let chosen = Caps { asks_baud: 4000, ..levels(Line::MuLaw) };
        assert!(!agree(&chosen, &levels(Line::MuLaw)).unwrap().locked);
    }

    #[test]
    fn a_call_of_packets_is_never_run_as_one_pair_of_wires() {
        let on = |line: Line| Caps { line, two_wire: true, ..caps(8_000, 8_000) };
        let agreed = agree(&on(Line::MuLaw), &on(Line::MuLaw)).unwrap();
        assert!(!agreed.two_wire);
        assert!(agreed.notes.is_empty(), "{:?}", agreed.notes);
        // One end on a sound card can still have an echo to cancel.
        assert!(agree(&on(Line::Sound), &on(Line::MuLaw)).unwrap().two_wire);
    }

    #[test]
    fn it_gets_through_on_one_pair_of_wires_and_through_noise() {
        // Each end's own signal four times as loud as the far end's, and
        // noise as loud as the far end's signal.
        let a = Caps { two_wire: true, ..caps(48_000, 48_000) };
        let (caller, host, took) = meet(a, caps(48_000, 48_000), 2.0, 0.09);
        assert!(caller.done() && host.done(), "after {took:.1} s");
        assert!(took < 3.0, "took {took:.1} s");
        let ours = caller.agreed().unwrap().unwrap();
        assert!(ours.two_wire, "{:?}", ours.notes);
        assert_eq!(host.agreed().unwrap().unwrap().two_wire, ours.two_wire);
    }

    #[test]
    fn a_fast_card_and_a_slow_one_meet_at_the_slow_ones_rate() {
        let (caller, host, _) = meet(caps(192_000, 192_000), caps(48_000, 48_000), 0.0, 0.0);
        let (fast, slow) = (caller.agreed().unwrap().unwrap(), host.agreed().unwrap().unwrap());
        assert_eq!((fast.send_hz, fast.hear_hz, fast.send_times, fast.hear_times), (48_000.0, 48_000.0, 4, 4));
        assert_eq!((slow.send_hz, slow.hear_hz, slow.send_times, slow.hear_times), (48_000.0, 48_000.0, 1, 1));
        // And cards that no whole number relates are told so: the answerer
        // selects no mode, and both ends know why.
        let odd = agree(&caps(44_100, 44_100), &caps(48_000, 48_000));
        assert!(odd.is_err_and(|why| why.contains("44100") && why.contains("48000")));
        let (caller, host, _) = meet(caps(44_100, 44_100), caps(48_000, 48_000), 0.0, 0.0);
        assert!(caller.done() && host.done());
        assert!(caller.agreed().unwrap().is_err_and(|why| why.contains("44100")));
        assert!(host.agreed().unwrap().is_err_and(|why| why.contains("44100")));
    }

    #[test]
    fn the_signal_keeps_to_its_band() {
        // A second of each carrier with messages on it, and how much of
        // its power is more than 300 Hz from the carrier: under a
        // thousandth.
        let fs = 48_000.0;
        for carrier in [CALLER_HZ, ANSWERER_HZ] {
            let mut voice = Voice::new(fs, carrier);
            let said: Vec<f64> = (0..fs as usize)
                .map(|_| {
                    if voice.bits.is_empty() {
                        voice.bits.extend(Message::Cl(caps(48_000, 192_000)).frame());
                    }
                    voice.say()
                })
                .collect();
            let total: f64 = said.iter().map(|x| x * x).sum::<f64>() / fs;
            // -15 dBFS RMS, to within a decibel.
            assert!((10.0 * total.log10() + 15.0).abs() < 1.0, "{} dB", 10.0 * total.log10());
            let mut outside = 0.0;
            for hz in (0..4000).step_by(1) {
                if (f64::from(hz) - carrier).abs() <= 300.0 {
                    continue;
                }
                let (mut re, mut im) = (0.0, 0.0);
                for (n, x) in said.iter().enumerate() {
                    let turn = std::f64::consts::TAU * f64::from(hz) * n as f64 / fs;
                    re += x * turn.cos();
                    im += x * turn.sin();
                }
                outside += 2.0 * (re * re + im * im) / (fs * fs);
            }
            assert!(outside < 1e-3 * total, "{carrier} Hz: {:.1} dB", 10.0 * (outside / total).log10());
        }
    }

    #[test]
    fn an_end_that_cannot_hear_says_so_through_the_other_ends_signal() {
        // The far end is sending its modulation, which to this signal is
        // noise right across the band and as loud as it is.
        let (mut deaf, mut far) = (Watch::new(true, 48_000.0, 48_000.0, 1), Watch::new(false, 48_000.0, 48_000.0, 1));
        deaf.tell(2);
        let mut seed = 5u64;
        let mut heard = Vec::new();
        let mut said_for = 0;
        for _ in 0..(3.0 * 48_000.0) as usize {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let hiss = 0.18 * (((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5) * 3.46;
            said_for += usize::from(deaf.saying());
            let said = deaf.say();
            heard.extend(far.hear(&[0.3 * said + hiss]));
            // And nothing the deaf end hears is a notice to it.
            assert_eq!(deaf.hear(&[hiss + said]), None);
        }
        assert!(!deaf.saying());
        assert!(said_for < 48_000, "{said_for} samples");
        assert!(!heard.is_empty() && heard.iter().all(|&by| by == 2), "{heard:?}");
    }

    #[test]
    fn nobody_there_is_never_done() {
        let mut hello = Hello::new(true, caps(48_000, 48_000));
        for _ in 0..(10.0 * 48_000.0) as usize {
            let own = hello.say();
            // Its own signal comes straight back and nothing else does.
            hello.hear(&[own, own]);
        }
        assert!(!hello.done());
        assert!(hello.theirs().is_none());
    }
}
