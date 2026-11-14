//! V.8 bis on the line: the tones, the frames, and the exchange of them.
//!
//! [`v8::bis`] says what the signals and messages are. This makes them,
//! hears them, and runs one station's side of a transaction (clause 9):
//! Figure 11 for the station that started it and Figure 12 for the one that
//! answers, which are the same machine looked at from each end.
//!
//! A transaction is short and takes turns. One station sends a signal -- two
//! tones for 400 ms and one for 100 -- and the other answers with a signal
//! or a message; messages go back and forth until one of them is an MS and
//! the answer to it is an ACK. Then the line belongs to whatever the MS
//! named, and the station that *received* the MS is its answering modem,
//! "irrespective of which station originated the call" (9.9).
//!
//! The two stations never use the same frequencies: the initiating one's
//! signals are on 1375 and 2002 Hz and its messages on V.21's low channel,
//! the responding one's on 1529 and 2225 Hz and the high channel (Table 1,
//! 7.2.1). So a station on a two-wire line is not confused by its own echo,
//! and this listens the whole time it is sending.

use std::collections::VecDeque;
use std::f64::consts::TAU;

use ec::hdlc::{Decoder, Encoder, Fcs, FrameError, fcs_ok};
use v8::bis::{self, Data, Kind, Message, Modulation, Signal};

pub use v8::bis::Station as Side;

use crate::v21;

/// How loudly a signal or a message goes out, as a fraction of full scale.
///
/// The same as V.8's answer tone and its messages, for the same reason: the
/// far end sets its gain on whatever comes first.
const LEVEL: f64 = 0.35;
/// MRe and CRe go out quieter: "12 to 15 dB below" the nominal level
/// (7.1.4), because a person may be the one who picked the telephone up.
const QUIET: f64 = LEVEL * 0.2239; // -13 dB

/// How long two tones of a pair have to stand before they are a first
/// segment. V.92 has a caller act on "the initial 50 ms of CRe" (9.2.1.2);
/// a little more than that keeps speech from passing for one.
const PAIR_HELD: f64 = 0.060;
/// How long the single tone has to stand to be a second segment.
const TONE_HELD: f64 = 0.030;
/// How long after the pair stops a second segment may still begin.
const TONE_WINDOW: f64 = 0.150;
/// The quiet a station leaves before it answers: the rest of the far end's
/// second segment, which is recognised before it is over, and a little air.
const TURNAROUND: f64 = 0.090;
/// The quiet before answering a message, which is recognised at its last
/// flag: there is nothing left of it to wait out.
const AFTER_MESSAGE: f64 = 0.020;
/// "remain silent for a minimum of 400 ms" before MRe or CRe (10.2.2).
const BEFORE_OPENING: f64 = 0.400;
/// "more than 5 s" in any state but the two that are rest, and the station
/// goes back to the start (9.8).
const STATE_LIMIT: f64 = 5.0;
/// "no V.8 bis signals ... within 3 s" of MRe or CRe: send it again, or give
/// the line up (10.2.2).
const OPENING_PATIENCE: f64 = 3.0;

/// One tone, measured: a correlator with a one-pole low-pass behind it.
#[derive(Debug, Clone)]
struct Probe {
    hz: f64,
    step: f64,
    phase: f64,
    re: f64,
    im: f64,
    alpha: f64,
}

impl Probe {
    fn new(hz: f64, fs: f64) -> Self {
        // Eight milliseconds: a tenth of the shortest thing there is to hear,
        // and narrow enough that the two pairs, 154 Hz apart at the nearest,
        // are told apart by a factor of seven.
        Self { hz, step: TAU * hz / fs, phase: 0.0, re: 0.0, im: 0.0, alpha: 1.0 - (-1.0 / (0.008 * fs)).exp() }
    }

    fn feed(&mut self, x: f64) {
        self.phase += self.step;
        if self.phase > TAU {
            self.phase -= TAU;
        }
        self.re += self.alpha * (x * self.phase.cos() - self.re);
        self.im += self.alpha * (-x * self.phase.sin() - self.im);
    }

    /// The amplitude of a sine at this frequency that would read as this.
    fn amplitude(&self) -> f64 {
        2.0 * self.re.hypot(self.im)
    }
}

/// Hears signals: a pair of tones, and then the single tone that names it.
#[derive(Debug, Clone)]
struct Ear {
    fs: f64,
    /// The four first-segment tones: the initiating pair, then the
    /// responding one.
    pairs: [Probe; 4],
    /// The six second-segment tones.
    tones: Vec<Probe>,
    /// Mean square of the line, at the probes' own time constant.
    power: f64,
    alpha: f64,
    /// Samples the pair now standing has stood, and whose it is.
    pair: Option<(Side, usize)>,
    /// A first segment that has ended: whose, and how long ago.
    armed: Option<(Side, usize)>,
    /// The single tone now standing after it: which, and for how long.
    tone: Option<(usize, usize)>,
}

impl Ear {
    fn new(fs: f64) -> Self {
        let (i1, i2) = Side::Initiating.pair();
        let (r1, r2) = Side::Responding.pair();
        Self {
            fs,
            pairs: [Probe::new(i1, fs), Probe::new(i2, fs), Probe::new(r1, fs), Probe::new(r2, fs)],
            tones: Signal::ALL.iter().map(|s| Probe::new(s.segment_2(), fs)).collect(),
            power: 0.0,
            alpha: 1.0 - (-1.0 / (0.008 * fs)).exp(),
            pair: None,
            armed: None,
            tone: None,
        }
    }

    /// Whose first segment is on the line now, once it has stood long enough
    /// to be one.
    fn pair_standing(&self) -> Option<Side> {
        self.pair.filter(|(_, n)| *n as f64 >= PAIR_HELD * self.fs).map(|(side, _)| side)
    }

    fn feed(&mut self, x: f64) -> Option<(Signal, Side)> {
        for probe in self.pairs.iter_mut().chain(self.tones.iter_mut()) {
            probe.feed(x);
        }
        self.power += self.alpha * (x * x - self.power);
        let rms = self.power.sqrt();
        if rms < 1e-4 {
            self.pair = None;
            self.tone = None;
            self.age();
            return None;
        }

        // Two equal tones of amplitude A have a root mean square of A, and
        // each probe reads A: so each of a pair is most of the line's power,
        // and neither is much louder than the other.
        let standing = [Side::Initiating, Side::Responding].into_iter().enumerate().find(|(k, _)| {
            let (a, b) = (self.pairs[2 * k].amplitude(), self.pairs[2 * k + 1].amplitude());
            a > 0.55 * rms && b > 0.55 * rms && a < 2.5 * b && b < 2.5 * a
        });
        match (standing, self.pair) {
            (Some((_, side)), Some((was, n))) if was == side => self.pair = Some((side, n + 1)),
            (Some((_, side)), _) => self.pair = Some((side, 1)),
            (None, Some((side, n))) => {
                if n as f64 >= PAIR_HELD * self.fs {
                    self.armed = Some((side, 0));
                    self.tone = None;
                }
                self.pair = None;
            }
            (None, None) => {}
        }

        let (side, _) = self.armed?;
        self.age();
        // A single tone of amplitude A has a root mean square of A over root
        // two, so it reads 1.41 of it: nearly all of the line.
        let loud = self
            .tones
            .iter()
            .enumerate()
            .filter(|(_, p)| p.amplitude() > 1.15 * rms)
            .max_by(|a, b| a.1.amplitude().total_cmp(&b.1.amplitude()))
            .map(|(k, _)| k);
        self.tone = match (loud, self.tone) {
            (Some(k), Some((was, n))) if was == k => Some((k, n + 1)),
            (Some(k), _) => Some((k, 1)),
            (None, _) => None,
        };
        let (k, n) = self.tone?;
        if (n as f64) < TONE_HELD * self.fs {
            return None;
        }
        self.armed = None;
        self.tone = None;
        // A tone that is not this side's to send after its pair is nobody's
        // signal: the pair was speech, or the tone was.
        Signal::from_segment_2(self.tones[k].hz, side).map(|signal| (signal, side))
    }

    fn age(&mut self) {
        if let Some((side, n)) = self.armed {
            self.armed = ((n as f64) < TONE_WINDOW * self.fs).then_some((side, n + 1));
        }
    }
}

/// One thing to put on the line.
#[derive(Debug, Clone)]
enum Piece {
    Silence(usize),
    /// Two tones at once, each at half of `level`.
    Pair { hz: (f64, f64), samples: usize, level: f64 },
    Tone { hz: f64, samples: usize, level: f64 },
    /// A stream of bits on this station's V.21 channel.
    Bits(Vec<bool>),
}

/// Puts signals and messages on the line, one after another.
#[derive(Debug)]
struct Mouth {
    fs: f64,
    side: Side,
    fsk: v21::Sender,
    queue: VecDeque<Piece>,
    /// Samples into the piece at the head of the queue.
    at: usize,
    sending_bits: bool,
    /// Samples since the last bit of a message was taken up by the
    /// modulator, which has then to be given its own time on the line.
    tail: usize,
}

impl Mouth {
    fn new(side: Side, fs: f64) -> Self {
        // 7.2.1: the initiating station's messages on the low channel.
        let channel = match side {
            Side::Initiating => v21::CHANNEL_1,
            Side::Responding => v21::CHANNEL_2,
        };
        Self { fs, side, fsk: v21::Sender::on(channel, fs), queue: VecDeque::new(), at: 0, sending_bits: false, tail: 0 }
    }

    fn busy(&self) -> bool {
        !self.queue.is_empty()
    }

    fn silence(&mut self, seconds: f64) {
        self.queue.push_back(Piece::Silence((seconds * self.fs) as usize));
    }

    fn signal(&mut self, signal: Signal) {
        let level = if matches!(signal, Signal::MRe | Signal::CRe) { QUIET } else { LEVEL };
        self.queue.push_back(Piece::Pair { hz: self.side.pair(), samples: (bis::SEGMENT_1 * self.fs) as usize, level });
        self.queue.push_back(Piece::Tone { hz: signal.segment_2(), samples: (bis::SEGMENT_2 * self.fs) as usize, level });
    }

    /// Queue a message: its preamble of mark unless an escape signal's
    /// second segment has just been that already (7.2.4), two flags, the
    /// information field and its FCS, and a flag (7.2.3).
    fn message(&mut self, message: &Message, after_escape: bool) {
        let mut bits = Vec::new();
        if !after_escape {
            bits.resize((bis::PREAMBLE * v21::BAUD).round() as usize, true);
        }
        let mut encoder = Encoder::new(Fcs::Bits16);
        encoder.idle(1);
        encoder.frame(&message.encode());
        while let Some(bit) = encoder.next_bit() {
            bits.push(bit);
        }
        self.queue.push_back(Piece::Bits(bits));
    }

    fn next_sample(&mut self) -> f64 {
        loop {
            let Some(piece) = self.queue.front() else { return 0.0 };
            let t = self.at as f64 / self.fs;
            let (out, done) = match piece {
                Piece::Silence(n) => (0.0, self.at + 1 >= *n),
                Piece::Pair { hz, samples, level } => {
                    (0.5 * level * ((TAU * hz.0 * t).sin() + (TAU * hz.1 * t).sin()), self.at + 1 >= *samples)
                }
                Piece::Tone { hz, samples, level } => (level * (TAU * hz * t).sin(), self.at + 1 >= *samples),
                Piece::Bits(bits) => {
                    if !self.sending_bits {
                        self.fsk.set_transmitting(true);
                        self.fsk.push_bits(bits);
                        self.sending_bits = true;
                    }
                    if self.fsk.pending_bits() == 0 {
                        // The modulator has taken the last bit up. It goes
                        // on sending it for a bit time, and only then is the
                        // carrier dropped: stopping when the queue empties
                        // cut the closing flag's last zero off, and a flag
                        // short of a bit is seven ones, which is an abort.
                        self.tail += 1;
                        if self.tail as f64 > self.fs / v21::BAUD {
                            self.fsk.set_transmitting(false);
                            self.sending_bits = false;
                            self.tail = 0;
                            self.queue.pop_front();
                            self.at = 0;
                            continue;
                        }
                    }
                    (LEVEL * self.fsk.next_sample(), false)
                }
            };
            self.at += 1;
            if done {
                self.queue.pop_front();
                self.at = 0;
            }
            return out;
        }
    }
}

/// What a station does on hearing a mode request (Figure 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnMr {
    /// Choose the mode itself and say so: transaction 1.
    Ms,
    /// Hand the choice back, with MRd: transaction 7. Only to MRe.
    Mrd,
    /// Ask for the far end's capabilities first, with CRd: transaction 10.
    /// Only to MRe.
    Crd,
}

/// What a station does on hearing a capabilities request (Figure 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnCr {
    /// Send its list and let the far end choose: transaction 2.
    Cl,
    /// Send its list and ask for the far end's, to choose itself:
    /// transaction 3.
    Clr,
    /// Ask for the far end's list with CRd, to choose itself: transaction
    /// 12. Only to CRe.
    Crd,
}

/// How a transaction ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The MS that was agreed.
    pub ms: Message,
    /// Whether this station received it, and so is the answering modem of
    /// what follows (9.9).
    pub answer: bool,
}

impl Selection {
    /// The modulation the MS names, if it selected a data call.
    pub fn modulation(&self) -> Option<Modulation> {
        self.ms.data_mode().and_then(|d| d.selected())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running,
    Selected(Selection),
    /// It came to nothing, and why: the line goes on without V.8 bis.
    Failed(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// Listening for a signal or a message that starts a transaction.
    Initial,
    /// An answering station's silence before its MRe or CRe.
    Opening,
    /// Sent MRe or MRd as the initiating station: MS, CRd or MRd comes next.
    SentMr,
    /// Sent CRe, or CRd: a CL or a CLR comes next, or a CRd back.
    SentCr,
    /// Sent CL, or MRd in answer to MRe: an MS comes next, or a CRd.
    SentCl,
    /// Sent CLR: a CL comes next and an MS straight after it.
    SentClr,
    /// Sent MS: ACK or NAK comes next.
    SentMs(Message),
    /// Accepted an MS; the ACK, if one was asked for, is going out.
    Accepting(Message),
    Done,
}

/// One station of a V.8 bis transaction.
#[derive(Debug)]
pub struct Station {
    fs: f64,
    side: Side,
    /// Whether this is the start of a call that was answered automatically,
    /// where a message in answer to a signal has ESr in front of it (9.4).
    establishment: bool,
    capabilities: Message,
    on_mr: OnMr,
    on_cr: OnCr,
    /// The signal an answering station opens with, and how many times it has.
    opening: Option<Signal>,
    openings: u32,
    ear: Ear,
    rx: v21::Receiver,
    decoder: Decoder,
    mouth: Mouth,
    state: State,
    /// Samples in this state, and since the call began.
    in_state: usize,
    far: Option<Message>,
    status: Status,
    notes: Vec<String>,
    /// Whether an escape signal has been heard and its message has not.
    escaped: bool,
}

impl Station {
    fn new(side: Side, capabilities: Message, fs: f64) -> Self {
        // Each station listens on the other's channel.
        let hears = match side {
            Side::Initiating => v21::CHANNEL_2,
            Side::Responding => v21::CHANNEL_1,
        };
        Self {
            fs,
            side,
            establishment: true,
            capabilities,
            on_mr: OnMr::Ms,
            on_cr: OnCr::Crd,
            opening: None,
            openings: 0,
            ear: Ear::new(fs),
            rx: v21::Receiver::on(hears, fs),
            decoder: Decoder::new(Fcs::Bits16),
            mouth: Mouth::new(side, fs),
            state: State::Initial,
            in_state: 0,
            far: None,
            status: Status::Running,
            notes: Vec::new(),
            escaped: false,
        }
    }

    /// The station that has just answered a call by itself (10.2.2): the
    /// initiating one, opening with `signal`, which is MRe to have the caller
    /// choose the mode or CRe to be told what the caller can do.
    pub fn answering(capabilities: Message, signal: Signal, fs: f64) -> Self {
        debug_assert!(matches!(signal, Signal::MRe | Signal::CRe), "an answering station opens with MRe or CRe");
        let mut station = Self::new(Side::Initiating, capabilities, fs);
        station.opening = Some(signal);
        station.state = State::Opening;
        station
    }

    /// The station that placed the call (10.2.1): the responding one, which
    /// waits to hear what the answering station opens with.
    pub fn calling(capabilities: Message, fs: f64) -> Self {
        Self::new(Side::Responding, capabilities, fs)
    }

    /// What to answer a mode request and a capabilities request with.
    pub fn replying(mut self, on_mr: OnMr, on_cr: OnCr) -> Self {
        self.on_mr = on_mr;
        self.on_cr = on_cr;
        self
    }

    pub fn status(&self) -> Status {
        // Selected only once this end's last word is on the line: what
        // follows starts "immediately after the transmission of ACK(1)" (9.6)
        // and not on top of it.
        if matches!(self.status, Status::Selected(_)) && self.mouth.busy() {
            return Status::Running;
        }
        self.status.clone()
    }

    /// What the far end said it can do, once it has sent a list.
    pub fn far_capabilities(&self) -> Option<&Message> {
        self.far.as_ref()
    }

    /// Whose first segment is on the line just now. V.92's quick connect
    /// acts on this before the second segment says which signal it is.
    pub fn pair_heard(&self) -> Option<Side> {
        self.ear.pair_standing()
    }

    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Initial => "V.8 bis: listening",
            State::Opening => "V.8 bis: the silence before the request",
            State::SentMr => "V.8 bis: mode request sent",
            State::SentCr => "V.8 bis: capabilities request sent",
            State::SentCl => "V.8 bis: capabilities sent",
            State::SentClr => "V.8 bis: capabilities sent and asked for",
            State::SentMs(_) => "V.8 bis: mode select sent",
            State::Accepting(_) => "V.8 bis: mode select accepted",
            State::Done => "V.8 bis: done",
        }
    }

    /// What was heard and sent, in order, for a transcript.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// One sample in, one sample out.
    pub fn step(&mut self, sample: f64) -> f64 {
        if self.status == Status::Running {
            self.in_state += 1;
            if let Some((signal, _)) = self.ear.feed(sample).filter(|(_, side)| *side != self.side) {
                self.heard_signal(signal);
            }
            if let Some(bit) = self.rx.feed(sample)
                && let Some(frame) = self.decoder.feed(bit)
            {
                match information(frame, &self.decoder).and_then(|octets| Message::decode(&octets)) {
                    Some(message) => self.heard_message(message),
                    None => self.heard_rubbish(),
                }
            }
            self.keep_time();
        }
        self.mouth.next_sample()
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.in_state = 0;
    }

    fn note(&mut self, text: String) {
        self.notes.push(text);
    }

    fn fail(&mut self, why: &'static str) {
        self.note(format!("V.8 bis: {why}"));
        self.status = Status::Failed(why);
        self.enter(State::Done);
    }

    fn send_signal(&mut self, signal: Signal) {
        self.note(format!("sent {}", signal.name()));
        self.mouth.signal(signal);
    }

    /// Send `message` in answer to a signal: with ESr in front at call
    /// establishment (9.4), whose second segment is the message's preamble.
    fn answer_signal(&mut self, message: &Message) {
        self.mouth.silence(TURNAROUND);
        let escape = self.establishment && self.side == Side::Responding;
        if escape {
            self.send_signal(Signal::ESr);
        }
        self.send_message(message, escape);
    }

    fn send_message(&mut self, message: &Message, after_escape: bool) {
        self.note(format!("sent {}", describe(message)));
        self.mouth.message(message, after_escape);
    }

    /// Time passing: the opening, its repeat, and 9.8's five seconds.
    fn keep_time(&mut self) {
        let seconds = self.in_state as f64 / self.fs;
        match self.state {
            State::Opening if seconds >= BEFORE_OPENING => {
                let signal = self.opening.expect("only an answering station opens");
                self.openings += 1;
                self.send_signal(signal);
                self.enter(if signal == Signal::MRe { State::SentMr } else { State::SentCr });
            }
            // "the answering station may retransmit MRe or CRe or clear down
            // the line" (10.2.2): once more, and then the line is given to
            // whatever comes after V.8 bis.
            State::SentMr | State::SentCr if self.opening.is_some() && seconds >= OPENING_PATIENCE && !self.mouth.busy() => {
                if self.openings >= 2 {
                    self.fail("nothing at the far end answered");
                } else {
                    self.enter(State::Opening);
                    self.in_state = (BEFORE_OPENING * self.fs) as usize;
                }
            }
            State::Initial | State::Opening | State::Done => {}
            _ if seconds >= STATE_LIMIT => self.fail("the far end stopped part of the way through"),
            _ => {}
        }
    }

    fn heard_signal(&mut self, signal: Signal) {
        self.note(format!("heard {}", signal.name()));
        match (&self.state, signal) {
            // An escape says a message is coming and nothing more.
            (_, Signal::ESi | Signal::ESr) => self.escaped = true,
            (State::Initial, Signal::MRe | Signal::MRd) => match self.on_mr {
                OnMr::Mrd if signal == Signal::MRe => {
                    self.mouth.silence(TURNAROUND);
                    self.send_signal(Signal::MRd);
                    // Figure 12: "Sent CL or MR", which an MS or a CRd follows.
                    self.enter(State::SentCl);
                }
                OnMr::Crd if signal == Signal::MRe => {
                    self.mouth.silence(TURNAROUND);
                    self.send_signal(Signal::CRd);
                    self.enter(State::SentCr);
                }
                _ => self.choose(None, true),
            },
            (State::Initial, Signal::CRe | Signal::CRd) => match self.on_cr {
                OnCr::Crd if signal == Signal::CRe => {
                    self.mouth.silence(TURNAROUND);
                    self.send_signal(Signal::CRd);
                    self.enter(State::SentCr);
                }
                OnCr::Clr => {
                    let list = self.list(Kind::Clr);
                    self.answer_signal(&list);
                    self.enter(State::SentClr);
                }
                _ => {
                    let list = self.list(Kind::Cl);
                    self.answer_signal(&list);
                    self.enter(State::SentCl);
                }
            },
            // Figure 11: the far end handed the choice back.
            (State::SentMr, Signal::MRd) => self.choose(None, true),
            // Figures 11 and 12: the far end wants this end's list.
            (State::SentMr | State::SentCr | State::SentCl, Signal::CRd) => {
                let list = self.list(Kind::Cl);
                self.mouth.silence(TURNAROUND);
                self.send_message(&list, false);
                self.enter(State::SentCl);
            }
            _ => {}
        }
    }

    fn heard_message(&mut self, message: Message) {
        self.note(format!("heard {}", describe(&message)));
        self.escaped = false;
        match (self.state.clone(), message.kind) {
            (State::Initial | State::SentMr | State::SentCl | State::SentClr, Kind::Ms) => self.judge(message),
            (State::Initial | State::SentCr, Kind::Cl) => {
                self.far = Some(message);
                self.choose(self.far.clone(), false);
            }
            // The first half of CL-MS: the MS is on its way behind it.
            (State::SentClr, Kind::Cl) => self.far = Some(message),
            (State::Initial | State::SentCr, Kind::Clr) => {
                // CL-MS: this end's list and its choice, one behind the
                // other with no silence between (9.1, Note 1).
                self.far = Some(message);
                let list = self.list(Kind::Cl);
                self.mouth.silence(AFTER_MESSAGE);
                self.send_message(&list, false);
                self.choose(self.far.clone(), false);
            }
            (State::SentMs(ms), Kind::Ack1) => self.selected(ms, false),
            (State::SentMs(_), kind) if kind.is_nak() => self.fail("the far end refused the mode"),
            _ => {}
        }
    }

    /// 9.8: "If an invalid frame is received in any state, the receiving
    /// station shall send a NAK(1) and immediately return to the Initial
    /// V.8 bis State." Only where a frame was due, though: the tones of a
    /// signal are not a frame gone wrong.
    fn heard_rubbish(&mut self) {
        if matches!(self.state, State::Initial | State::Opening | State::Done) && !self.escaped {
            return;
        }
        self.note("heard a frame that did not check".to_owned());
        self.mouth.silence(AFTER_MESSAGE);
        self.send_message(&Message::bare(Kind::Nak1), false);
        self.escaped = false;
        self.enter(State::Initial);
    }

    /// This end's capabilities as a list of `kind`.
    fn list(&self, kind: Kind) -> Message {
        Message { kind, ..self.capabilities.clone() }
    }

    /// Send the MS: the fastest thing both ends have where the far end's
    /// list is known, and the fastest this end has where it is not.
    fn choose(&mut self, far: Option<Message>, answering_signal: bool) {
        let ours = self.capabilities.data_mode().unwrap_or_default();
        let modulation = match far.as_ref().and_then(Message::data_mode) {
            Some(theirs) => ours.best_with(&theirs),
            None if far.is_some() => None,
            None => ours.best(),
        };
        let Some(modulation) = modulation else {
            self.fail("the two ends have no data mode in common");
            return;
        };
        let ms = Message::select(&self.capabilities, modulation);
        if answering_signal {
            self.answer_signal(&ms);
        } else {
            self.mouth.silence(AFTER_MESSAGE);
            self.send_message(&ms, false);
        }
        self.enter(State::SentMs(ms));
    }

    /// An MS arrived: take it or refuse it (9.5, 9.7).
    fn judge(&mut self, ms: Message) {
        let ours = self.capabilities.data_mode().unwrap_or_default();
        let can = ms.data_mode().is_some_and(|asked| ours.can_do(&asked));
        self.mouth.silence(AFTER_MESSAGE);
        if !can {
            // "does not support or has disabled the requested mode".
            self.send_message(&Message::bare(Kind::Nak3), false);
            self.fail("the far end asked for a mode this end does not have");
            return;
        }
        // 9.7: ACK(1) where the MS asked for one, and otherwise the answer
        // tone alone is the answer.
        if ms.identify().ack1 {
            self.send_message(&Message::bare(Kind::Ack1), false);
        }
        self.enter(State::Accepting(ms.clone()));
        self.selected(ms, true);
    }

    fn selected(&mut self, ms: Message, answer: bool) {
        self.status = Status::Selected(Selection { ms, answer });
        self.enter(State::Done);
    }
}

/// The information field of a frame the decoder has finished, if it is a
/// valid one by 7.2.9.
///
/// The decoder is V.42's, and V.42's shortest frame is an address, a control
/// octet and the FCS: four octets. V.8 bis's is three -- "fewer than 3 octets
/// between the flags" is what makes a frame invalid -- and ACK and NAK are
/// exactly that, one octet and its check. So a frame the decoder calls too
/// short is looked at again here.
fn information(frame: Result<Vec<u8>, FrameError>, decoder: &Decoder) -> Option<Vec<u8>> {
    match frame {
        Ok(octets) => Some(octets),
        Err(FrameError::TooShort) => {
            let whole = decoder.discarded();
            (whole.len() == 3 && fcs_ok(whole, Fcs::Bits16)).then(|| whole[..1].to_vec())
        }
        Err(_) => None,
    }
}

/// A message in a few words, for a transcript.
fn describe(message: &Message) -> String {
    let mut text = message.kind.name().to_owned();
    if let Some(data) = message.data_mode() {
        text.push_str(&format!(" ({})", list_of(&data)));
    }
    text
}

fn list_of(data: &Data) -> String {
    let named = [
        (data.v92_digital, "V.92 digital"),
        (data.v92_analogue, "V.92 analogue"),
        (data.v90_digital, "V.90 digital"),
        (data.v90_analogue, "V.90 analogue"),
        (data.v34, "V.34"),
        (data.v32bis, "V.32bis"),
        (data.v32, "V.32"),
        (data.v22bis, "V.22bis"),
        (data.v22, "V.22"),
        (data.v21, "V.21"),
        (data.v42, "V.42"),
    ];
    named.iter().filter(|(has, _)| *has).map(|(_, name)| *name).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use v8::bis::{Identification, Network};

    const FS: f64 = 16_000.0;

    fn caller() -> Message {
        Message::data(
            Kind::Cl,
            Identification { v8: true, ack1: true, ..Identification::default() },
            Data { v42: true, v34: true, v32bis: true, v22bis: true, v90_analogue: true, v92_analogue: true, ..Data::default() },
        )
    }

    fn server() -> Message {
        Message::data(
            Kind::Cl,
            Identification { v8: true, ack1: true, network: Some(Network { digital: true, ..Network::default() }), ..Identification::default() },
            Data { v42: true, v34: true, v32bis: true, v90_digital: true, v92_digital: true, ..Data::default() },
        )
    }

    /// Two stations on a line with `delay` seconds each way, run until both
    /// are finished or `seconds` have gone. What each heard and sent.
    fn run(mut a: Station, mut b: Station, delay: f64, seconds: f64) -> (Station, Station, Vec<String>, Vec<String>) {
        let n = ((delay * FS) as usize).max(1);
        let (mut to_b, mut to_a) = (VecDeque::from(vec![0.0; n]), VecDeque::from(vec![0.0; n]));
        let (mut said_a, mut said_b) = (Vec::new(), Vec::new());
        for _ in 0..(seconds * FS) as usize {
            let (heard_a, heard_b) = (to_a.pop_front().unwrap(), to_b.pop_front().unwrap());
            to_b.push_back(a.step(heard_a));
            to_a.push_back(b.step(heard_b));
            said_a.extend(a.take_notes());
            said_b.extend(b.take_notes());
            if a.status() != Status::Running && b.status() != Status::Running {
                break;
            }
        }
        (a, b, said_a, said_b)
    }

    fn selection(station: &Station) -> Selection {
        match station.status() {
            Status::Selected(selection) => selection,
            other => panic!("not selected: {other:?}"),
        }
    }

    /// Table 1: each signal is heard as itself, from the station that sent
    /// it, and as nothing else.
    #[test]
    fn every_signal_is_heard_as_itself() {
        for (signal, side) in [
            (Signal::MRe, Side::Initiating),
            (Signal::MRd, Side::Initiating),
            (Signal::MRd, Side::Responding),
            (Signal::CRe, Side::Initiating),
            (Signal::CRd, Side::Initiating),
            (Signal::CRd, Side::Responding),
            (Signal::ESi, Side::Initiating),
            (Signal::ESr, Side::Responding),
        ] {
            let mut mouth = Mouth::new(side, FS);
            mouth.silence(0.05);
            mouth.signal(signal);
            mouth.silence(0.3);
            let mut ear = Ear::new(FS);
            let mut heard = Vec::new();
            while mouth.busy() {
                heard.extend(ear.feed(mouth.next_sample()));
            }
            assert_eq!(heard, [(signal, side)], "{} from {side:?}", signal.name());
        }
    }

    /// A message crosses the line in its frame: a hundred milliseconds of
    /// mark, two flags, the field, the FCS and a flag (7.2.3, 7.2.4).
    #[test]
    fn a_message_crosses_in_its_frame() {
        let mut mouth = Mouth::new(Side::Responding, FS);
        mouth.message(&caller(), false);
        mouth.silence(0.1);
        let mut rx = v21::Receiver::on(v21::CHANNEL_2, FS);
        let mut decoder = Decoder::new(Fcs::Bits16);
        let mut frames = Vec::new();
        let mut samples = 0usize;
        while mouth.busy() {
            samples += 1;
            if let Some(bit) = rx.feed(mouth.next_sample())
                && let Some(frame) = decoder.feed(bit)
            {
                frames.push(frame.expect("the frame checks"));
            }
        }
        assert_eq!(frames, [caller().encode()]);
        // 30 bits of mark, 3 flags, 11 octets and 2 of FCS at 300 bit/s, and
        // whatever was stuffed: a little under half a second.
        let seconds = samples as f64 / FS - 0.1;
        assert!((0.44..0.52).contains(&seconds), "{seconds:.3} s on the line");
    }

    /// Transaction 12 of Table 7, which is the one a caller takes control
    /// with: CRe, CRd, CL, MS, ACK. The answering station ends up the
    /// answering modem, and a V.92 server and a V.92 caller choose V.92.
    #[test]
    fn a_caller_that_answers_cre_with_crd_chooses_the_mode() {
        let a = Station::answering(server(), Signal::CRe, FS);
        let b = Station::calling(caller(), FS).replying(OnMr::Ms, OnCr::Crd);
        let (a, b, said_a, said_b) = run(a, b, 0.02, 8.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        let kinds = |said: &[String]| said.iter().map(|s| s.split(" (").next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(kinds(&said_a), ["sent CRe", "heard CRd", "sent CL", "heard MS", "sent ACK(1)"]);
        assert_eq!(kinds(&said_b), ["heard CRe", "sent CRd", "heard CL", "sent MS", "heard ACK(1)"]);
        let (at_a, at_b) = (selection(&a), selection(&b));
        assert!(at_a.answer && !at_b.answer, "the station that received MS answers (9.9)");
        assert_eq!(at_a.modulation(), Some(Modulation::V92));
        assert_eq!(at_a.ms, at_b.ms);
        assert!(at_a.ms.data_mode().unwrap().v92_analogue, "the caller's half is the one named");
        assert_eq!(b.far_capabilities(), Some(&server()));
    }

    /// Transaction 2: CRe, then ESr and CL, MS, ACK. Here the answering
    /// station chooses, and the *calling* station becomes the answer modem.
    #[test]
    fn a_caller_that_answers_cre_with_its_list_is_chosen_for() {
        let a = Station::answering(server(), Signal::CRe, FS);
        let b = Station::calling(caller(), FS).replying(OnMr::Ms, OnCr::Cl);
        let (a, b, said_a, said_b) = run(a, b, 0.02, 8.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        let kinds = |said: &[String]| said.iter().map(|s| s.split(" (").next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(kinds(&said_a), ["sent CRe", "heard ESr", "heard CL", "sent MS", "heard ACK(1)"]);
        assert_eq!(kinds(&said_b), ["heard CRe", "sent ESr", "sent CL", "heard MS", "sent ACK(1)"]);
        assert!(selection(&b).answer && !selection(&a).answer);
        assert_eq!(selection(&a).modulation(), Some(Modulation::V92));
        assert!(selection(&a).ms.data_mode().unwrap().v92_digital, "the server's half is the one named");
    }

    /// Transaction 3: CRe, ESr and CLR, CL-MS, ACK.
    #[test]
    fn a_caller_that_answers_cre_with_clr_gets_a_list_and_a_choice_together() {
        let a = Station::answering(server(), Signal::CRe, FS);
        let b = Station::calling(caller(), FS).replying(OnMr::Ms, OnCr::Clr);
        let (_, b, said_a, said_b) = run(a, b, 0.02, 8.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        let kinds = |said: &[String]| said.iter().map(|s| s.split(" (").next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(kinds(&said_a), ["sent CRe", "heard ESr", "heard CLR", "sent CL", "sent MS", "heard ACK(1)"]);
        assert_eq!(kinds(&said_b), ["heard CRe", "sent ESr", "sent CLR", "heard CL", "heard MS", "sent ACK(1)"]);
        assert!(selection(&b).answer);
        assert_eq!(b.far_capabilities(), Some(&server()));
    }

    /// Transaction 1: MRe, then ESr and MS, ACK. The caller chooses with
    /// nothing of the far end's to go on.
    #[test]
    fn a_mode_request_is_answered_with_a_mode() {
        let a = Station::answering(server(), Signal::MRe, FS);
        let b = Station::calling(caller(), FS).replying(OnMr::Ms, OnCr::Cl);
        let (a, b, said_a, said_b) = run(a, b, 0.02, 8.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        let kinds = |said: &[String]| said.iter().map(|s| s.split(" (").next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(kinds(&said_a), ["sent MRe", "heard ESr", "heard MS", "sent ACK(1)"]);
        assert_eq!(kinds(&said_b), ["heard MRe", "sent ESr", "sent MS", "heard ACK(1)"]);
        assert!(selection(&a).answer);
        assert_eq!(selection(&b).modulation(), Some(Modulation::V92));
    }

    /// Transaction 7: MRe, MRd, MS, ACK: the caller hands the choice back.
    #[test]
    fn a_mode_request_handed_back_is_answered_by_the_station_that_asked() {
        let a = Station::answering(server(), Signal::MRe, FS);
        let b = Station::calling(caller(), FS).replying(OnMr::Mrd, OnCr::Cl);
        let (_, b, said_a, said_b) = run(a, b, 0.02, 8.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        let kinds = |said: &[String]| said.iter().map(|s| s.split(" (").next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(kinds(&said_a), ["sent MRe", "heard MRd", "sent MS", "heard ACK(1)"]);
        assert_eq!(kinds(&said_b), ["heard MRe", "sent MRd", "heard MS", "sent ACK(1)"]);
        assert!(selection(&b).answer);
    }

    /// 9.5: an MS for a mode the station does not have is refused with
    /// NAK(3), and both ends give V.8 bis up.
    #[test]
    fn a_mode_this_end_does_not_have_is_refused() {
        // A far end that can only be an analogue V.90 modem asks another
        // analogue modem for V.90.
        let analogue_only = Message::data(
            Kind::Cl,
            Identification { ack1: true, ..Identification::default() },
            Data { v90_analogue: true, ..Data::default() },
        );
        let a = Station::answering(analogue_only.clone(), Signal::MRe, FS);
        let b = Station::calling(analogue_only, FS).replying(OnMr::Ms, OnCr::Cl);
        let (a, b, said_a, said_b) = run(a, b, 0.02, 8.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        assert!(said_a.iter().any(|s| s == "sent NAK(3)"), "{said_a:?}");
        assert!(matches!(a.status(), Status::Failed(_)));
        assert!(matches!(b.status(), Status::Failed(_)));
    }

    /// 10.2.2: an answering station that hears nothing sends its request
    /// once more and then lets the line go, so that an answer tone can
    /// follow for a caller with no V.8 bis.
    #[test]
    fn a_silent_far_end_is_given_up_on() {
        let mut a = Station::answering(server(), Signal::CRe, FS);
        let mut sent = Vec::new();
        let mut seconds = 0.0;
        while a.status() == Status::Running && seconds < 20.0 {
            a.step(0.0);
            sent.extend(a.take_notes());
            seconds += 1.0 / FS;
        }
        assert_eq!(a.status(), Status::Failed("nothing at the far end answered"));
        assert_eq!(sent.iter().filter(|s| *s == "sent CRe").count(), 2, "{sent:?}");
        assert!((6.0..8.5).contains(&seconds), "gave up after {seconds:.1} s");
    }

    /// A round trip of a second, which is what a call over a trunk has, and
    /// the transaction still comes out: nothing in it is on a short clock.
    #[test]
    fn a_long_line_does_not_stop_it() {
        let a = Station::answering(server(), Signal::CRe, FS);
        let b = Station::calling(caller(), FS);
        let (a, b, said_a, said_b) = run(a, b, 0.5, 15.0);
        println!("answering: {said_a:#?}\ncalling: {said_b:#?}");
        assert_eq!(selection(&a).modulation(), Some(Modulation::V92));
        assert_eq!(selection(&a).ms, selection(&b).ms);
    }
}
