//! The handshake: what each end is, said before anything else, in a way any
//! line will carry.
//!
//! V.ARC's modulations are built on the sound card's own rate, and two ends
//! need not have the same card. One hears at 192 kHz and sends at 48; the
//! other has one device both ways; one of them has been told the line is a
//! single pair of wires and the other has not. Nothing in a wideband signal
//! can settle that, because until it is settled neither end knows what
//! signal to listen for. Both simply started, and a pair that did not match
//! heard nothing, for a minute, and said `NO CARRIER`.
//!
//! So a call begins with each end saying what it is, slowly, in the middle of
//! the voice band:
//!
//! - **200 bits a second, two tones a direction.** The caller sends 1000 Hz
//!   for a one and 1400 Hz for a nought; the answerer 2000 Hz and 2400 Hz.
//!   All four are multiples of the bit rate, so over one bit each is exactly
//!   invisible to a detector listening for another: an end on a two-wire
//!   line hears the far end's tones through its own, with no echo canceller.
//!   They fit a telephone channel with room to spare, and at this rate the
//!   sample rate of the card generating or hearing them does not matter.
//! - **Asynchronous characters**, a start bit, eight bits and a stop bit, so
//!   there is no clock to recover and no training.
//! - **One short message, over and over**: what rates this end hears and
//!   sends at, how many channels, which modulation it was asked for, whether
//!   it was told the line is two-wire, and whether it has yet heard the far
//!   end's message. With a check on the end.
//!
//! When an end has the far end's message, and has seen in it that the far
//! end has this one's, it sends twice more and stops. Both then work out the
//! same answer from the same two messages ([`agree`]) and start the
//! modulation that answer names.

use crate::link::{Receiver, Sender};

/// Bits a second.
pub const BAUD: f64 = 200.0;

/// The tones: a one and a nought from the caller, then from the answerer.
const CALLER: (f64, f64) = (1000.0, 1400.0);
const ANSWERER: (f64, f64) = (2000.0, 2400.0);

/// How loud, as a peak against full scale: -15 dBFS RMS.
const LEVEL: f64 = 0.25;

/// What starts a message.
const HEAD: [u8; 2] = *b"VA";

/// The message's length on the line: the head, the body and a check.
const FRAME: usize = HEAD.len() + BODY + 2;
const BODY: usize = 16;

/// Bits of idle between one message and the next.
const GAP: usize = 16;

/// Messages sent after the far end is known to have heard, before stopping.
const PARTING: u32 = 1;

/// How long to go on once the far end's message is in hand without seeing
/// that it has heard this end, in seconds. A far end that heard and went
/// ahead is silent in these tones from then on.
const PATIENCE: f64 = 4.0;

/// The modulations a call may be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Dmt,
    Qam,
    Pcm,
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
    /// This end has the far end's message.
    pub heard: bool,
    /// The baud this end wants to be sent to at, where somebody has chosen
    /// one for a single-carrier mode; nought for whatever the line bears.
    pub asks_baud: u32,
}

impl Caps {
    fn encode(&self) -> [u8; FRAME] {
        let mut out = [0u8; FRAME];
        out[..2].copy_from_slice(&HEAD);
        out[2] = 1;
        out[3] = u8::from(self.two_wire) | u8::from(self.heard) << 1;
        out[4] = match self.wanted {
            Wanted::Dmt => 0,
            Wanted::Qam => 1,
            Wanted::Pcm => 2,
        };
        out[5] = self.channels;
        out[6..10].copy_from_slice(&self.hears_hz.to_le_bytes());
        out[10..14].copy_from_slice(&self.sends_hz.to_le_bytes());
        out[14..18].copy_from_slice(&self.asks_baud.to_le_bytes());
        let check = crc16(&out[..FRAME - 2]);
        out[FRAME - 2..].copy_from_slice(&check.to_le_bytes());
        out
    }

    fn decode(frame: &[u8]) -> Option<Self> {
        if frame.len() != FRAME || frame[..2] != HEAD || frame[2] != 1 {
            return None;
        }
        if crc16(&frame[..FRAME - 2]) != u16::from_le_bytes([frame[FRAME - 2], frame[FRAME - 1]]) {
            return None;
        }
        Some(Self {
            two_wire: frame[3] & 1 != 0,
            heard: frame[3] & 2 != 0,
            wanted: match frame[4] {
                0 => Wanted::Dmt,
                1 => Wanted::Qam,
                2 => Wanted::Pcm,
                _ => return None,
            },
            channels: frame[5],
            hears_hz: u32::from_le_bytes(frame[6..10].try_into().ok()?),
            sends_hz: u32::from_le_bytes(frame[10..14].try_into().ok()?),
            asks_baud: u32::from_le_bytes(frame[14..18].try_into().ok()?),
        })
    }
}

/// What an end says later in a call, in the same tones, when it cannot
/// hear the far end's modulation: how many times slowed it is now listening
/// for, as a power of two.
///
/// The handshake's tones get through lines the modulation does not, which is
/// the whole reason they are what they are. A line that carries one way and
/// not the other leaves one end loaded and waiting and the other hearing
/// nothing it can use, and until this there was nothing either could do
/// about it. The end that hears nothing cannot tell the far end so in the
/// modulation, since it does not know that its own is heard; it can in
/// these.
const NOTICE_HEAD: [u8; 2] = *b"VN";
const NOTICE: usize = NOTICE_HEAD.len() + 2 + 2;

fn notice(slowed: u8) -> [u8; NOTICE] {
    let mut out = [0u8; NOTICE];
    out[..2].copy_from_slice(&NOTICE_HEAD);
    out[2] = 1;
    out[3] = slowed;
    let check = crc16(&out[..NOTICE - 2]);
    out[NOTICE - 2..].copy_from_slice(&check.to_le_bytes());
    out
}

fn read_notice(frame: &[u8]) -> Option<u8> {
    if frame.len() != NOTICE || frame[..2] != NOTICE_HEAD || frame[2] != 1 {
        return None;
    }
    (crc16(&frame[..NOTICE - 2]) == u16::from_le_bytes([frame[NOTICE - 2], frame[NOTICE - 1]]))
        .then_some(frame[3])
}

/// What an ear made of the characters it has read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heard {
    Caps(Caps),
    /// The far end is listening for the modulation slowed by two to this.
    Slowed(u8),
}

/// CRC-16/CCITT, as X.25 has it.
fn crc16(octets: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for &octet in octets {
        crc ^= u16::from(octet);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// What the two ends will do, worked out the same at both from the same
/// two messages.
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
    /// Anything asked for that could not be had, for whoever is watching.
    pub notes: Vec<String>,
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
    if send_times > 1
        || hear_times > 1
        || mine.sends_hz != theirs.hears_hz
        || mine.hears_hz != theirs.sends_hz
    {
        notes.push(format!(
            "the ends' sound cards differ: {send_hz:.0} Hz on the way out and {hear_hz:.0} Hz on the way back"
        ));
    }

    // Two-wire if either end was told so, and only if both can cancel an
    // echo, which wants one sound card: hearing and sending at one rate.
    let asked = mine.two_wire || theirs.two_wire;
    let able = mine.hears_hz == mine.sends_hz && theirs.hears_hz == theirs.sends_hz;
    if asked && !able {
        notes.push(
            "two-wire was asked for, and an end hears and sends on different sound cards, which cannot cancel an echo: running as a wire each way"
                .to_owned(),
        );
    }

    let wanted = if mine.wanted != theirs.wanted {
        notes.push(
            "the ends asked for different modulations: running the multicarrier one".to_owned(),
        );
        Wanted::Dmt
    } else {
        mine.wanted
    };
    Ok(Agreed {
        wanted,
        two_wire: asked && able,
        send_hz,
        hear_hz,
        send_times,
        hear_times,
        send_baud: theirs.asks_baud,
        hear_baud: mine.asks_baud,
        notes,
    })
}

/// One tone pair's detector on one channel: is the bit now arriving a one
/// or a nought, and the characters that makes.
///
/// Each tone is measured over the last bit through a raised-cosine window,
/// not a square one. Over a square window the four tones are invisible to
/// each other only while each holds still, and an end on one pair of wires
/// hears its own tones, far louder than the far end's, changing every bit:
/// each change splashed across the far end's tones and drowned them. The
/// raised cosine falls away fast enough that it does not.
///
/// A raised-cosine window is the square window's answer at the tone, less a
/// quarter of its answers one bit rate either side, each turned to where the
/// window began. So five running sums serve both tones: they are 400 Hz
/// apart, which is two bit rates.
struct Ear {
    /// The line times each of five tones 200 Hz apart, over the last bit,
    /// and the running sums: real and imaginary for each.
    products: Vec<[f64; 10]>,
    sums: [f64; 10],
    at: usize,
    /// How far round each of the five is, and how far each goes a sample.
    turn: [f64; 5],
    step: [f64; 5],
    /// How far round one bit rate is, counted from where the window began.
    began: f64,
    began_step: f64,
    /// The character being read: samples until the next look, bits so far.
    was_one: bool,
    wait: f64,
    bit: i8,
    octet: u8,
    per_bit: f64,
    /// The last characters read, for finding a message in.
    read: Vec<u8>,
    /// Which tone is arriving, between minus one and one.
    eye: f64,
}

impl Ear {
    fn new(fs: f64, tones: (f64, f64)) -> Self {
        let per_bit = fs / BAUD;
        let len = per_bit.round() as usize;
        let tau = std::f64::consts::TAU;
        let mut step = [0.0; 5];
        for (i, step) in step.iter_mut().enumerate() {
            *step = tau * (tones.0 - BAUD + BAUD * i as f64) / fs;
        }
        debug_assert!((tones.1 - tones.0 - 2.0 * BAUD).abs() < 1e-9);
        Self {
            products: vec![[0.0; 10]; len],
            sums: [0.0; 10],
            at: 0,
            turn: [0.0; 5],
            step,
            // The window that ends on the first sample began a bit before.
            began: (-(tau * BAUD / fs) * (len as f64 - 1.0)).rem_euclid(tau),
            began_step: tau * BAUD / fs,
            was_one: true,
            wait: 0.0,
            bit: -1,
            octet: 0,
            per_bit,
            read: Vec::with_capacity(2 * FRAME),
            eye: 0.0,
        }
    }

    /// The raised-cosine answer at the middle one of three neighbouring
    /// sums, as a power.
    fn through(&self, middle: usize) -> f64 {
        let at = |i: usize| (self.sums[2 * i], self.sums[2 * i + 1]);
        let (cos, sin) = (self.began.cos(), self.began.sin());
        let (below, here, above) = (at(middle - 1), at(middle), at(middle + 1));
        // The one below turned back by where the window began, the one
        // above turned on by it.
        let re = 0.5 * here.0
            - 0.25 * (below.0 * cos + below.1 * sin)
            - 0.25 * (above.0 * cos - above.1 * sin);
        let im = 0.5 * here.1
            - 0.25 * (below.1 * cos - below.0 * sin)
            - 0.25 * (above.1 * cos + above.0 * sin);
        re * re + im * im
    }

    /// A sample; a message, if this sample finished one.
    fn hear(&mut self, x: f64) -> Option<Heard> {
        let mut now = [0.0; 10];
        for i in 0..5 {
            now[2 * i] = x * self.turn[i].cos();
            now[2 * i + 1] = -x * self.turn[i].sin();
            self.turn[i] = (self.turn[i] + self.step[i]) % std::f64::consts::TAU;
        }
        let old = std::mem::replace(&mut self.products[self.at], now);
        self.at = (self.at + 1) % self.products.len();
        for i in 0..10 {
            self.sums[i] += now[i] - old[i];
        }
        let (one, nought) = (self.through(1), self.through(3));
        // Which tone, and by how much: one for all of it on the tone for a
        // one, minus one for all of it on the other. What an eye is drawn
        // from.
        self.eye = (one - nought) / (one + nought + 1e-30);
        self.began = (self.began + self.began_step) % std::f64::consts::TAU;
        let is_one = one >= nought;

        // The sums cover the bit just gone, so a change seen now happened
        // half a bit ago and the middle of the new bit is half a bit on.
        let mut found = None;
        if self.bit < 0 {
            if self.was_one && !is_one {
                self.bit = 0;
                self.wait = self.per_bit / 2.0;
            }
        } else {
            self.wait -= 1.0;
            if self.wait <= 0.0 {
                self.wait += self.per_bit;
                match self.bit {
                    // The start bit, looked at whole: still a nought?
                    0 if is_one => self.bit = -1,
                    0 => {
                        self.bit = 1;
                        self.octet = 0;
                    }
                    1..=8 => {
                        self.octet |= u8::from(is_one) << (self.bit - 1);
                        self.bit += 1;
                    }
                    _ => {
                        self.bit = -1;
                        // A stop bit that is not a one is not a character.
                        if is_one {
                            found = self.character();
                        }
                    }
                }
            }
        }
        self.was_one = is_one;
        found
    }

    fn character(&mut self) -> Option<Heard> {
        if self.read.len() == 2 * FRAME {
            self.read.remove(0);
        }
        self.read.push(self.octet);
        let last = |len: usize| {
            self.read
                .len()
                .checked_sub(len)
                .map(|from| &self.read[from..])
        };
        last(FRAME)
            .and_then(Caps::decode)
            .map(Heard::Caps)
            .or_else(|| last(NOTICE).and_then(read_notice).map(Heard::Slowed))
    }
}

/// One end's part of the handshake.
pub struct Hello {
    mine: Caps,
    theirs: Option<Caps>,
    fs_out: f64,
    ears: Vec<Ear>,
    /// The tones this end sends, how far round the one being sent is, and
    /// what is being sent: the bits of a message, then idle.
    tones: (f64, f64),
    turn: f64,
    bits: Vec<bool>,
    at: usize,
    left_in_bit: f64,
    /// Messages sent whole since the far end's said it had heard.
    parting: u32,
    /// Samples heard since the far end's message arrived.
    since: f64,
    fs_in: f64,
    done: bool,
}

impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hello")
            .field("mine", &self.mine)
            .field("theirs", &self.theirs)
            .finish_non_exhaustive()
    }
}

impl Hello {
    /// The end that called, or the end that answered, with what it is.
    pub fn new(calling: bool, mine: Caps) -> Self {
        let (tones, listens) = if calling {
            (CALLER, ANSWERER)
        } else {
            (ANSWERER, CALLER)
        };
        let fs_in = f64::from(mine.hears_hz);
        let mut hello = Self {
            ears: (0..mine.channels.max(1))
                .map(|_| Ear::new(fs_in, listens))
                .collect(),
            mine,
            theirs: None,
            fs_out: f64::from(mine.sends_hz),
            tones,
            turn: 0.0,
            bits: Vec::new(),
            at: 0,
            left_in_bit: 0.0,
            parting: 0,
            since: 0.0,
            fs_in,
            done: false,
        };
        hello.message();
        hello
    }

    /// Which of the far end's two tones is arriving on the first channel:
    /// one for the tone that is a one, minus one for the other, and between
    /// them while it changes or when neither is there. For drawing.
    pub fn eye(&self) -> f64 {
        self.ears.first().map_or(0.0, |ear| ear.eye)
    }

    /// The far end's message, once it has arrived.
    pub fn theirs(&self) -> Option<&Caps> {
        self.theirs.as_ref()
    }

    /// Whether this end has said all it has to and may start the call.
    pub fn done(&self) -> bool {
        self.done
    }

    /// What the two ends will do, once this end has the far end's message.
    pub fn agreed(&self) -> Option<Result<Agreed, String>> {
        self.theirs.as_ref().map(|theirs| agree(&self.mine, theirs))
    }

    /// The next message's bits: each character with its start and stop, and
    /// a little idle after.
    fn message(&mut self) {
        let caps = Caps {
            heard: self.theirs.is_some(),
            ..self.mine
        };
        self.bits.clear();
        for octet in caps.encode() {
            self.bits.push(false);
            self.bits.extend((0..8).map(|i| octet >> i & 1 != 0));
            self.bits.push(true);
        }
        self.bits.extend(std::iter::repeat_n(true, GAP));
        self.at = 0;
    }

    /// A frame as it arrived, a sample a channel.
    pub fn hear(&mut self, input: &[f64]) {
        for (ear, &x) in self.ears.iter_mut().zip(input) {
            if let Some(Heard::Caps(caps)) = ear.hear(x) {
                // The latest is the one that counts: it may now say the far
                // end has heard.
                self.theirs = Some(caps);
            }
        }
        if self.theirs.is_some() {
            self.since += 1.0;
            if self.since > PATIENCE * self.fs_in {
                self.done = true;
            }
        }
    }

    /// The next sample to send, the same on every channel.
    pub fn say(&mut self) -> f64 {
        if self.done {
            return 0.0;
        }
        if self.left_in_bit <= 0.0 {
            self.left_in_bit += self.fs_out / BAUD;
            self.at += 1;
            if self.at >= self.bits.len() {
                // A message has gone whole. Was it one the far end could
                // act on, sent after it said it had heard?
                if self.theirs.is_some_and(|t| t.heard) {
                    self.parting += 1;
                    if self.parting > PARTING {
                        self.done = true;
                        return 0.0;
                    }
                }
                self.message();
            }
        }
        self.left_in_bit -= 1.0;
        let hz = if self.bits[self.at] {
            self.tones.0
        } else {
            self.tones.1
        };
        self.turn = (self.turn + std::f64::consts::TAU * hz / self.fs_out) % std::f64::consts::TAU;
        LEVEL * self.turn.sin()
    }
}

/// An end's ear for the handshake's tones for the rest of the call, and its
/// voice in them: for saying, and hearing said, that the modulation is not
/// getting through and should be slowed.
pub struct Watch {
    ears: Vec<Ear>,
    tones: (f64, f64),
    fs_out: f64,
    turn: f64,
    bits: Vec<bool>,
    at: usize,
    left_in_bit: f64,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("saying", &self.saying())
            .finish_non_exhaustive()
    }
}

impl Watch {
    /// For the end that called or the end that answered, hearing on
    /// `channels` channels at `fs_in` and sending at `fs_out`.
    pub fn new(calling: bool, fs_in: f64, fs_out: f64, channels: usize) -> Self {
        let (tones, listens) = if calling {
            (CALLER, ANSWERER)
        } else {
            (ANSWERER, CALLER)
        };
        Self {
            ears: (0..channels.max(1))
                .map(|_| Ear::new(fs_in, listens))
                .collect(),
            tones,
            fs_out,
            turn: 0.0,
            bits: Vec::new(),
            at: 0,
            left_in_bit: 0.0,
        }
    }

    /// A frame as it arrived. What the far end says it is listening for,
    /// if this frame finished its saying so.
    pub fn hear(&mut self, input: &[f64]) -> Option<u8> {
        let mut asked = None;
        for (ear, &x) in self.ears.iter_mut().zip(input) {
            if let Some(Heard::Slowed(by)) = ear.hear(x) {
                asked = Some(by);
            }
        }
        asked
    }

    /// Tell the far end this end is listening for the modulation slowed by
    /// two to the `slowed`: three times over, a second and a bit, during
    /// which [`Watch::say`] has the line.
    pub fn tell(&mut self, slowed: u8) {
        self.bits.clear();
        // A bit of idle first, so the first start bit is an edge.
        self.bits.extend(std::iter::repeat_n(true, GAP));
        for _ in 0..3 {
            for octet in notice(slowed) {
                self.bits.push(false);
                self.bits.extend((0..8).map(|i| octet >> i & 1 != 0));
                self.bits.push(true);
            }
            self.bits.extend(std::iter::repeat_n(true, GAP));
        }
        self.at = 0;
        self.left_in_bit = self.fs_out / BAUD;
    }

    /// Whether there is something still being said.
    pub fn saying(&self) -> bool {
        self.at < self.bits.len()
    }

    /// The next sample of what is being said; silence when nothing is.
    pub fn say(&mut self) -> f64 {
        if !self.saying() {
            return 0.0;
        }
        let hz = if self.bits[self.at] {
            self.tones.0
        } else {
            self.tones.1
        };
        self.turn = (self.turn + std::f64::consts::TAU * hz / self.fs_out) % std::f64::consts::TAU;
        self.left_in_bit -= 1.0;
        if self.left_in_bit <= 0.0 {
            self.left_in_bit += self.fs_out / BAUD;
            self.at += 1;
        }
        LEVEL * self.turn.sin()
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
        Self {
            past: std::collections::VecDeque::from(vec![0.0; taps.len()]),
            inner,
            taps,
        }
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
        Self {
            past: std::collections::VecDeque::from(vec![0.0; taps.len() / 2 + 1]),
            inner,
            taps,
        }
    }
}

impl Receiver for Half {
    fn feed(&mut self, sample: f64) {
        self.past.pop_back();
        self.past.push_front(sample);
        for phase in 0..2 {
            let filled: f64 = self
                .taps
                .iter()
                .skip(phase)
                .step_by(2)
                .zip(&self.past)
                .map(|(t, x)| t * x)
                .sum();
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
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (std::f64::consts::TAU * cutoff * x).sin() / (std::f64::consts::PI * x)
            };
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
        let sum: f64 = self
            .taps
            .iter()
            .skip(self.phase)
            .step_by(self.times)
            .zip(&self.past)
            .map(|(t, x)| t * x)
            .sum();
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
        Caps {
            hears_hz: hears,
            sends_hz: sends,
            channels: 2,
            wanted: Wanted::Dmt,
            two_wire: false,
            heard: false,
            asks_baud: 0,
        }
    }

    #[test]
    fn a_message_survives_being_written_down() {
        let said = Caps {
            two_wire: true,
            heard: true,
            wanted: Wanted::Pcm,
            asks_baud: 8000,
            ..caps(192_000, 48_000)
        };
        assert_eq!(Caps::decode(&said.encode()), Some(said));
        let mut spoiled = said.encode();
        spoiled[7] ^= 1;
        assert_eq!(Caps::decode(&spoiled), None);
    }

    /// Two ends say hello over a line: `gain` on the way, `noise` on top,
    /// and each end's own signal mixed into what it hears if the line is one
    /// pair of wires.
    fn meet(a: Caps, b: Caps, own: f64, noise: f64) -> (Hello, Hello, f64) {
        // The line is as fast as the faster card on it, and each end's
        // samples are held or skipped to fit. Crude, and enough for tones.
        let line = f64::from(a.sends_hz.max(a.hears_hz).max(b.sends_hz).max(b.hears_hz));
        let (mut caller, mut host) = (Hello::new(true, a), Hello::new(false, b));
        let mut seed = 9u64;
        let mut hiss = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            noise * (((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5) * 3.46
        };
        let (mut from_caller, mut from_host) = (0.0, 0.0);
        let mut owed = [0.0f64; 4];
        let rates = [
            f64::from(a.sends_hz),
            f64::from(a.hears_hz),
            f64::from(b.sends_hz),
            f64::from(b.hears_hz),
        ];
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

    #[test]
    fn two_ends_learn_what_each_other_are_and_agree() {
        // The rig that started this: each end hears on one device and sends
        // on another, at different rates, and both were told two-wire.
        let a = Caps {
            two_wire: true,
            ..caps(48_000, 192_000)
        };
        let b = Caps {
            two_wire: true,
            ..caps(192_000, 48_000)
        };
        let (caller, host, took) = meet(a, b, 0.0, 0.0);
        assert!(caller.done() && host.done(), "after {took:.1} s");
        assert!(took < 6.0, "took {took:.1} s");
        assert_eq!(
            caller.theirs().map(|c| (c.hears_hz, c.sends_hz)),
            Some((192_000, 48_000))
        );
        let (ours, theirs) = (
            caller.agreed().unwrap().unwrap(),
            host.agreed().unwrap().unwrap(),
        );
        // A wire each way after all, each direction at its own rate.
        assert!(!ours.two_wire && !theirs.two_wire);
        assert_eq!((ours.send_hz, ours.hear_hz), (192_000.0, 48_000.0));
        assert_eq!((theirs.send_hz, theirs.hear_hz), (48_000.0, 192_000.0));
        assert_eq!((ours.send_times, ours.hear_times), (1, 1));
        assert!(
            ours.notes
                .iter()
                .any(|n| n.contains("two-wire was asked for")),
            "{:?}",
            ours.notes
        );
    }

    #[test]
    fn it_gets_through_on_one_pair_of_wires_and_through_noise() {
        // Each end's own tones four times as loud as the far end's, and
        // noise as loud as the far end's tones.
        let a = Caps {
            two_wire: true,
            ..caps(48_000, 48_000)
        };
        let (caller, host, took) = meet(a, caps(48_000, 48_000), 2.0, 0.09);
        assert!(caller.done() && host.done(), "after {took:.1} s");
        let ours = caller.agreed().unwrap().unwrap();
        assert!(ours.two_wire, "{:?}", ours.notes);
        assert_eq!(host.agreed().unwrap().unwrap().two_wire, ours.two_wire);
    }

    #[test]
    fn a_fast_card_and_a_slow_one_meet_at_the_slow_ones_rate() {
        let (caller, host, _) = meet(caps(192_000, 192_000), caps(48_000, 48_000), 0.0, 0.0);
        let (fast, slow) = (
            caller.agreed().unwrap().unwrap(),
            host.agreed().unwrap().unwrap(),
        );
        assert_eq!(
            (fast.send_hz, fast.hear_hz, fast.send_times, fast.hear_times),
            (48_000.0, 48_000.0, 4, 4)
        );
        assert_eq!(
            (slow.send_hz, slow.hear_hz, slow.send_times, slow.hear_times),
            (48_000.0, 48_000.0, 1, 1)
        );
        // And cards that no whole number relates are told so.
        let odd = agree(&caps(44_100, 44_100), &caps(48_000, 48_000));
        assert!(odd.is_err_and(|why| why.contains("44100") && why.contains("48000")));
    }

    #[test]
    fn an_end_that_cannot_hear_says_so_through_the_other_ends_signal() {
        // The far end is sending its modulation, which to these tones is
        // noise right across the band and as loud as they are.
        let (mut deaf, mut far) = (
            Watch::new(true, 48_000.0, 48_000.0, 1),
            Watch::new(false, 48_000.0, 48_000.0, 1),
        );
        deaf.tell(2);
        let mut seed = 5u64;
        let mut heard = Vec::new();
        for _ in 0..(3.0 * 48_000.0) as usize {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let hiss = 0.18 * (((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5) * 3.46;
            let said = deaf.say();
            heard.extend(far.hear(&[0.3 * said + hiss]));
            // And nothing the deaf end hears is a notice to it.
            assert_eq!(deaf.hear(&[hiss + said]), None);
        }
        assert!(!deaf.saying());
        assert!(
            !heard.is_empty() && heard.iter().all(|&by| by == 2),
            "{heard:?}"
        );
    }

    #[test]
    fn nobody_there_is_never_done() {
        let mut hello = Hello::new(true, caps(48_000, 48_000));
        for _ in 0..(10.0 * 48_000.0) as usize {
            let own = hello.say();
            // Its own tones come straight back and nothing else does.
            hello.hear(&[own, own]);
        }
        assert!(!hello.done());
        assert!(hello.theirs().is_none());
    }
}
