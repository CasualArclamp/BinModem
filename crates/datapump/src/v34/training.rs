//! Phases 3 and 4 of the start-up (11.3 and 11.4): each end trains its
//! receiver on the other's signal, the two ask each other with J for the
//! constellation to train with next, train again, and settle what data mode
//! will be in MP sequences.
//!
//! From the answer modem, with the call modem's side beneath it (Figures 19
//! and 20):
//!
//! ```text
//! answer  INFO1a, 70 ms, S S' PP TRN J J J ...            S S' TRN     MP MP' E
//! call                          S S' PP TRN J J J ... J J' TRN   MP MP' E
//! ```
//!
//! S' is S-bar. Every change is set off by hearing the other end: the call
//! modem starts its S on hearing the answer modem's J, the answer modem falls
//! silent on hearing the call modem's S-bar, starts phase 4 on hearing its J,
//! and the call modem ends its J with J' on hearing the answer modem's S-bar
//! again. So each step waits a round trip, and the timers of 11.3.2 and 11.4.2
//! all count one or two of them.
//!
//! MD, the manufacturer-defined signal a modem may train its echo canceller
//! with, is never sent from here -- INFO1 says so -- but a far end that sends
//! one is waited out.

use std::collections::VecDeque;

use dsp::Complex;

use super::constellation::Point;
use super::info::{Info0, Info1a, Info1c};
use super::mp::{Finder, Found, Mp, Trellis};
use super::phase2::Role;
use super::probe;
use super::qam::{Band, Transmitter};
use super::receiver::{self, Heard, Receiver, Reference};
use super::signals::{self, J_FOUR, J_PRIME, J_SIXTEEN, Reader, Sender, Size};
use crate::v32::Mode;

/// How phases 3 and 4 are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// Both ends have sent E: data mode is next.
    Done,
    Failed(&'static str),
}

/// What phase 2 settled, as far as phases 3 and 4 need it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub role: Role,
    /// This end's transmitter.
    pub transmit: Band,
    pub pre_emphasis: u8,
    pub power_reduction: u8,
    /// The far end's transmitter, which this end's receiver listens to.
    pub receive: Band,
    /// The far end's MD, in 35 ms steps.
    pub far_md: u8,
    /// Seconds.
    pub round_trip: f64,
    /// Whether the far end's INFO0 set the CME bit, which stretches phase 4's
    /// wait for E to thirty seconds.
    pub far_cme: bool,
    /// Whether both ends have the 1664-point constellation rates above 28 800
    /// need.
    pub wide: bool,
}

impl Settings {
    /// From the INFO sequences: INFO1c is what the call modem found of the
    /// answer modem's transmitter, INFO1a what the answer modem found of the
    /// call modem's and the symbol rates both ways.
    pub fn new(role: Role, far: &Info0, info1c: &Info1c, info1a: &Info1a, round_trip: f64, wide: bool) -> Self {
        let towards_call = info1c.probed[info1a.answer_to_call.index() as usize];
        let towards_answer = info1a.probed;
        let call_band = Band::new(info1a.call_to_answer, towards_answer.high_carrier);
        let answer_band = Band::new(info1a.answer_to_call, towards_call.high_carrier);
        match role {
            Role::Call => Self {
                role,
                transmit: call_band,
                pre_emphasis: towards_answer.pre_emphasis,
                power_reduction: info1a.min_power_reduction,
                receive: answer_band,
                far_md: info1a.md_length,
                round_trip,
                far_cme: far.cme,
                wide,
            },
            Role::Answer => Self {
                role,
                transmit: answer_band,
                pre_emphasis: towards_call.pre_emphasis,
                power_reduction: info1c.min_power_reduction,
                receive: call_band,
                far_md: info1c.md_length,
                round_trip,
                far_cme: far.cme,
                wide,
            },
        }
    }
}

/// "70 ± 5 ms" of silence between INFO1a and the answer modem's S
/// (11.3.1.2.1).
const SILENCE_BEFORE_S: f64 = 0.070;

/// How long this end sends TRN for in phase 3: "at least 512T", and not
/// more than a round trip and two seconds with MD. The two real modems this
/// was checked against sent 1.1 and 1.9 s; a far receiver that trains slowly
/// is better served by more than the least.
const PHASE3_TRN: f64 = 1.0;

/// The least TRN there is in either phase.
const LEAST_TRN: usize = 512;

/// Phase 4's TRN from the call modem: "may continue sending TRN for up to
/// 2000 ms" (11.4.1.1.2); the answer modem's may run a round trip longer.
const MOST_TRN: f64 = 2.0;

/// TRN symbols of the far end's phase 4 heard before this end is trained
/// enough to send MP, over and above what training itself took.
const HEARD_TRN: usize = 64;

/// Slack on a far end's reply in the recovery timers, for this end's own
/// detection and transmit delays.
const SLACK: f64 = 0.3;

/// Whole MP' sequences sent before E. The recommendation asks only that the
/// one going out be finished, but on a VoIP call one of a jitter buffer's
/// twenty-millisecond slips can swallow a sequence whole -- three of sixteen
/// points' MP' fit in one -- and E after a single MP' is then E after none.
const MP_PRIME_REPEATS: usize = 8;

/// The round trips over the recommendation's own that this end waits for E
/// before giving up. The far end that answered the first call to reach phase 4
/// sent TRN for two and a half seconds of it, and its E came with less than a
/// second to spare; waiting longer costs nothing but the wait.
const E_PATIENCE: f64 = 1.0;

/// What this end sends, and how it moves from one signal to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    Silence,
    S,
    SBar,
    Pp,
    Trn,
    J,
    JPrime,
    Mp,
    E,
}

/// Symbols for the transmitter, one at a time.
#[derive(Debug, Clone)]
struct Source {
    sender: Sender,
    segment: Segment,
    /// Symbols of the current segment given.
    count: usize,
    /// Bits of a J, J', MP or E still to go.
    queue: VecDeque<bool>,
    /// What follows S-bar: PP in phase 3 and TRN in phase 4.
    after_s_bar: Segment,
    /// The constellation of TRN, MP and E, which the far end's J chose. J
    /// and J' are four points always, and so is phase 3's TRN.
    size: Size,
    /// The segment to change to at the first place the current one can end.
    pending: Option<Segment>,
    /// What this end's J asks for.
    ask: Size,
    mp: Mp,
    /// Whether the MP going out now has the acknowledge bit.
    sending_acknowledged: bool,
    /// Whole MP' sequences sent.
    acknowledged: usize,
    /// Symbols of silence given since the last signal, and how many there
    /// have to be before a change out of it is taken.
    silent: usize,
    hold: usize,
}

fn grid(point: Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(receiver::unit(size))
}

impl Source {
    fn new(mode: Mode, ask: Size) -> Self {
        Self {
            sender: Sender::new(mode),
            segment: Segment::Silence,
            count: 0,
            queue: VecDeque::new(),
            after_s_bar: Segment::Pp,
            size: Size::Four,
            pending: None,
            ask,
            mp: Mp::default(),
            sending_acknowledged: false,
            acknowledged: 0,
            silent: 0,
            hold: 0,
        }
    }

    fn start(&mut self, segment: Segment) {
        self.segment = segment;
        self.count = 0;
        self.silent = 0;
        self.queue.clear();
        match segment {
            // "The scrambler is initialized to zero prior to transmission of
            // the TRN signal" (10.1.3.8).
            Segment::Trn => self.sender.restart(),
            Segment::JPrime => self.queue.extend(J_PRIME),
            Segment::E => self.queue.extend(std::iter::repeat_n(true, signals::E_BITS)),
            _ => {}
        }
    }

    /// Move to `segment` at the next place the current one can end: straight
    /// away from silence or TRN, and at the end of a whole sequence from J or
    /// MP.
    fn change(&mut self, segment: Segment) {
        self.pending = Some(segment);
    }

    /// Bits for one symbol of a differential sequence at `size`.
    fn differential(&mut self, size: Size) -> Complex {
        let bits: Vec<bool> = (0..size.bits()).map(|_| self.queue.pop_front().unwrap_or(true)).collect();
        self.count += 1;
        grid(self.sender.differential(&bits), size)
    }

    fn next(&mut self) -> Complex {
        loop {
            match self.segment {
                Segment::Silence => {
                    if self.silent >= self.hold
                        && let Some(next) = self.pending.take()
                    {
                        self.hold = 0;
                        self.start(next);
                        continue;
                    }
                    self.silent += 1;
                    return Complex::ZERO;
                }
                Segment::S => {
                    if self.count == signals::S_SYMBOLS {
                        self.start(Segment::SBar);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s(self.count - 1), Size::Four);
                }
                Segment::SBar => {
                    if self.count == signals::S_BAR_SYMBOLS {
                        let next = self.after_s_bar;
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s_bar(self.count - 1), Size::Four);
                }
                Segment::Pp => {
                    if self.count == signals::PP_SYMBOLS {
                        self.start(Segment::Trn);
                        continue;
                    }
                    self.count += 1;
                    return signals::pp(self.count - 1).into();
                }
                Segment::Trn => {
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    let size = if self.after_s_bar == Segment::Pp { Size::Four } else { self.size };
                    return grid(self.sender.trn(size), size);
                }
                Segment::J | Segment::Mp => {
                    if self.queue.is_empty() {
                        if self.segment == Segment::Mp && self.count > 0 && self.sending_acknowledged {
                            self.acknowledged += 1;
                        }
                        if let Some(next) = self.pending.take() {
                            self.start(next);
                            continue;
                        }
                        if self.segment == Segment::J {
                            self.queue.extend(self.ask.j());
                        } else {
                            self.queue.extend(self.mp.to_bits());
                            self.sending_acknowledged = self.mp.acknowledge;
                            self.count = self.count.max(1);
                        }
                    }
                    let size = if self.segment == Segment::J { Size::Four } else { self.size };
                    return self.differential(size);
                }
                Segment::JPrime => {
                    if self.queue.is_empty() {
                        self.start(Segment::Trn);
                        continue;
                    }
                    return self.differential(Size::Four);
                }
                Segment::E => {
                    if self.queue.is_empty() {
                        self.start(Segment::Silence);
                        continue;
                    }
                    let size = self.size;
                    return self.differential(size);
                }
            }
        }
    }
}

/// What the far end's symbols come to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Event {
    J(Size),
    JPrime,
    Mp(Mp),
    E,
}

/// Reads the far end's symbols into bits and picks out J, J', MP and E.
#[derive(Debug, Clone)]
struct Listening {
    reader: Reader,
    size: Size,
    /// Reading TRN, which is not differentially encoded; after the first
    /// symbol that is not scrambled ones, everything is.
    trn: bool,
    /// Symbols let go while the descrambler fills with what it is reading.
    grace: usize,
    /// TRN symbols that descrambled to ones.
    trn_symbols: usize,
    /// The last 32 differentially decoded bits.
    bits: VecDeque<bool>,
    finder: Finder,
    j: Option<Size>,
    mp_found: bool,
    j_prime: bool,
}

impl Listening {
    fn new(far: Mode) -> Self {
        Self {
            reader: Reader::new(far),
            size: Size::Four,
            trn: false,
            grace: 0,
            trn_symbols: 0,
            bits: VecDeque::new(),
            finder: Finder::new(),
            j: None,
            mp_found: false,
            j_prime: false,
        }
    }

    /// TRN at `size` from here, from a scrambler this end's descrambler has
    /// not followed.
    fn begin_trn(&mut self, size: Size) {
        self.size = size;
        self.trn = true;
        self.grace = 24 / size.bits() + 1;
        self.trn_symbols = 0;
        self.bits.clear();
    }

    fn symbol(&mut self, point: Point) -> Vec<Event> {
        let mut events = Vec::new();
        if self.trn {
            let before = self.reader.clone();
            let bits = self.reader.trn(point, self.size);
            if self.grace > 0 {
                self.grace -= 1;
                return events;
            }
            if bits.iter().all(|b| *b) {
                self.trn_symbols += 1;
                return events;
            }
            self.reader = before;
            self.trn = false;
        }
        for bit in self.reader.differential(point, self.size) {
            self.bits.push_back(bit);
            if self.bits.len() > 32 {
                self.bits.pop_front();
            }
            match self.finder.feed(bit) {
                Some(Found::Mp(mp)) => {
                    self.mp_found = true;
                    events.push(Event::Mp(mp));
                }
                // Twenty ones could be anything before an MP has been read.
                Some(Found::E) if self.mp_found => events.push(Event::E),
                _ => {}
            }
        }
        if self.size == Size::Four && self.bits.len() == 32 {
            let (older, newer): (Vec<bool>, Vec<bool>) = (self.bits.range(..16).copied().collect(), self.bits.range(16..).copied().collect());
            if self.j.is_none() {
                for j in [Size::Four, Size::Sixteen] {
                    if older == j.j() && newer == j.j() {
                        self.j = Some(j);
                        events.push(Event::J(j));
                    }
                }
            } else if !self.j_prime && newer == J_PRIME && (older == J_FOUR || older == J_SIXTEEN) {
                self.j_prime = true;
                events.push(Event::JPrime);
            }
        }
        events
    }
}

/// Where phases 3 and 4 have got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    // The call modem.
    CallAwaitS,
    CallTraining,
    CallAwaitJ,
    CallSendTraining,
    CallAwaitS4,
    CallTraining4,
    CallMp,
    // The answer modem.
    AnswerSendTraining,
    AnswerAwaitS,
    AnswerTraining,
    AnswerAwaitJ,
    AnswerPhase4,
    AnswerMp,
    Finished,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::CallAwaitS | Self::AnswerAwaitS => "V.34 phase 3: listening for S",
            Self::CallTraining | Self::AnswerTraining => "V.34 phase 3: training",
            Self::CallAwaitJ | Self::AnswerAwaitJ => "V.34 phase 3: listening for J",
            Self::CallSendTraining | Self::AnswerSendTraining => "V.34 phase 3: sending PP and TRN",
            Self::CallAwaitS4 => "V.34 phase 4: listening for S",
            Self::CallTraining4 | Self::AnswerPhase4 => "V.34 phase 4: training",
            Self::CallMp | Self::AnswerMp => "V.34 phase 4: MP",
            Self::Finished => "V.34 phase 4 done",
        }
    }
}

/// Phases 3 and 4, one end of them.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    fs: f64,
    now: u64,
    stage: Stage,
    status: Status,
    /// A sample to give up at, and what to say.
    deadline: Option<(u64, &'static str)>,
    /// Where it had got to when it gave up.
    stopped_at: &'static str,
    tx: Transmitter,
    source: Source,
    rx: Receiver,
    listening: Listening,
    far_mode: Mode,
    /// Waiting out the far end's MD until this sample.
    md_until: Option<u64>,
    md_waited: bool,
    /// Symbols of phase 3's TRN to send.
    trn_symbols: usize,

    far_asked: Option<Size>,
    phase3_snr: Option<f64>,
    phase4_snr: Option<f64>,
    ours: Option<Mp>,
    far_mp: Option<Mp>,
    far_acknowledged: bool,
    far_e: bool,
    sent_e: bool,
}

impl Modem {
    /// Phase 3 from its start: for the answer modem the moment INFO1a has
    /// gone, for the call modem the moment it has arrived.
    pub fn new(settings: Settings, fs: f64) -> Self {
        let (own, far) = match settings.role {
            Role::Call => (Mode::Call, Mode::Answer),
            Role::Answer => (Mode::Answer, Mode::Call),
        };
        // Sixteen points for the far end's phase 4, as both real modems
        // asked of theirs.
        let ask = Size::Sixteen;
        let mut modem = Self {
            settings,
            fs,
            now: 0,
            stage: match settings.role {
                Role::Call => Stage::CallAwaitS,
                Role::Answer => Stage::AnswerSendTraining,
            },
            status: Status::Running,
            deadline: None,
            stopped_at: "",
            tx: Transmitter::new(settings.transmit, settings.pre_emphasis, settings.power_reduction, fs),
            source: Source::new(own, ask),
            rx: Receiver::new(settings.receive, fs),
            listening: Listening::new(far),
            far_mode: far,
            md_until: None,
            md_waited: false,
            trn_symbols: (PHASE3_TRN * settings.transmit.baud()) as usize,
            far_asked: None,
            phase3_snr: None,
            phase4_snr: None,
            ours: None,
            far_mp: None,
            far_acknowledged: false,
            far_e: false,
            sent_e: false,
        };
        match settings.role {
            Role::Call => {
                modem.rx.hunt();
                // 11.3.2.1.1: J within 2800 ms and two round trips of INFO1c.
                modem.deadline = Some((modem.samples(2.8 + 2.0 * settings.round_trip + SLACK), "no J from the answer modem"));
            }
            Role::Answer => {
                // Silence, then S. The pulse reaches this many symbols ahead
                // of the line, and they count towards the silence.
                let silence = (SILENCE_BEFORE_S * settings.transmit.baud()).round() as usize;
                modem.source.hold = silence.saturating_sub(Transmitter::lookahead());
                modem.source.after_s_bar = Segment::Pp;
                modem.source.change(Segment::S);
            }
        }
        modem
    }

    fn samples(&self, seconds: f64) -> u64 {
        self.now + (seconds * self.fs).round() as u64
    }

    fn rtd(&self) -> f64 {
        self.settings.round_trip
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn phase(&self) -> &'static str {
        match self.status {
            Status::Failed(_) => self.stopped_at,
            _ => self.stage.name(),
        }
    }

    /// The constellation the far end's J asked this end to train it with.
    pub fn far_asked(&self) -> Option<Size> {
        self.far_asked
    }

    /// What this end's J asked for.
    pub fn asked(&self) -> Size {
        self.source.ask
    }

    /// Signal to noise this end's receiver trained to in each phase.
    pub fn phase3_snr(&self) -> Option<f64> {
        self.phase3_snr
    }

    pub fn phase4_snr(&self) -> Option<f64> {
        self.phase4_snr
    }

    /// The decisions' signal to noise now.
    pub fn snr(&self) -> f64 {
        self.rx.snr_db()
    }

    /// The far end's last symbol, equalised, at unit mean power.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        self.rx.last_point().map(Into::into)
    }

    /// The far clock against this end's, as the receiver's timing loop has
    /// it, in parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.rx.drift_ppm()
    }

    /// Jumps in the far end's signal the receiver found and followed: a VoIP
    /// jitter buffer's slips.
    pub fn slips(&self) -> u32 {
        self.rx.slips()
    }

    /// The MP this end sends, once it has been made.
    pub fn our_mp(&self) -> Option<Mp> {
        self.ours
    }

    pub fn far_mp(&self) -> Option<Mp> {
        self.far_mp
    }

    /// The data rates each way, as multiples of 2400 bit/s -- this end's
    /// transmitter's and its receiver's -- once both MPs are known.
    pub fn rates(&self) -> Option<(u8, u8)> {
        let far = self.far_mp?;
        let ours = self.our_mp()?;
        let (call, answer) = match self.settings.role {
            Role::Call => (ours, far),
            Role::Answer => (far, ours),
        };
        let (towards_answer, towards_call) = negotiate(&call, &answer);
        Some(match self.settings.role {
            Role::Call => (towards_answer, towards_call),
            Role::Answer => (towards_call, towards_answer),
        })
    }

    fn fail(&mut self, why: &'static str) {
        self.stopped_at = self.stage.name();
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
        self.source.pending = None;
        self.source.start(Segment::Silence);
        self.rx.idle();
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// Carry phases 3 and 4 one sample further: hear `line`, and say what goes
    /// on it.
    pub fn step(&mut self, line: f64) -> f64 {
        self.now += 1;
        self.rx.feed(line);
        while let Some(heard) = self.rx.heard() {
            if self.status == Status::Running {
                self.heard(heard);
            }
        }
        if self.status == Status::Running {
            if let Some((at, why)) = self.deadline
                && self.now > at
            {
                self.fail(why);
            } else {
                self.stage_step();
            }
        }
        let source = &mut self.source;
        self.tx.next_sample(|| source.next())
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Reversal { at } => self.reversal(at),
            Heard::Trained { snr_db } => {
                match self.stage {
                    Stage::CallTraining => {
                        self.phase3_snr = Some(snr_db);
                        self.enter(Stage::CallAwaitJ);
                    }
                    Stage::AnswerTraining => {
                        self.phase3_snr = Some(snr_db);
                        self.enter(Stage::AnswerAwaitJ);
                        // 11.3.2.2.2: J within 2600 ms and two round trips of
                        // the end of this end's J.
                        self.deadline = Some((self.samples(2.6 + 2.0 * self.rtd() + SLACK), "no J from the call modem"));
                    }
                    Stage::CallTraining4 => self.phase4_snr = Some(snr_db),
                    _ => {}
                }
                let size = self.rx.size();
                self.listening.begin_trn(size);
            }
            Heard::Untrained => self.fail("the far end's training sequence did not train this end"),
            Heard::Symbol(symbol) => {
                for event in self.listening.symbol(symbol.decided) {
                    self.event(event);
                }
            }
        }
    }

    /// The far end's S turning into S-bar.
    fn reversal(&mut self, at: u64) {
        match self.stage {
            Stage::CallAwaitS | Stage::AnswerAwaitS => {
                if self.stage == Stage::AnswerAwaitS {
                    // 11.3.1.2.4: silence, once the current J is whole.
                    self.source.change(Segment::Silence);
                    self.deadline = None;
                }
                if self.settings.far_md > 0 && !self.md_waited {
                    // MD, then S and S-bar again.
                    self.md_waited = true;
                    self.md_until = Some(self.samples(0.035 * f64::from(self.settings.far_md)));
                    self.rx.idle();
                    return;
                }
                self.rx.train(Reference::PpThenTrn, self.far_mode, at);
                self.enter(if self.stage == Stage::CallAwaitS { Stage::CallTraining } else { Stage::AnswerTraining });
            }
            Stage::CallAwaitS4 => {
                // 11.4.1.1.1: the answer modem's S-bar. Stop J with a J', and
                // TRN after it; train on the answer modem's TRN.
                let asked = self.source.ask;
                self.rx.train(Reference::Trn(asked), self.far_mode, at);
                self.source.size = self.far_asked.unwrap_or(Size::Four);
                self.source.after_s_bar = Segment::Trn;
                self.source.change(Segment::JPrime);
                self.enter(Stage::CallTraining4);
                // 11.4.2.1.2: E within 2500 ms and two round trips of J'.
                let wait = if self.settings.far_cme { 30.0 } else { 2.5 + (2.0 + E_PATIENCE) * self.rtd() + SLACK };
                self.deadline = Some((self.samples(wait), "no E from the answer modem"));
            }
            _ => {}
        }
    }

    fn event(&mut self, event: Event) {
        match (self.stage, event) {
            (Stage::CallAwaitJ, Event::J(size)) => {
                // 11.3.1.1.3: "may wait for up to 500 ms" -- and does not, since
                // the answer modem's own wait for S-bar is only 600 ms and a
                // round trip from the start of its J.
                self.far_asked = Some(size);
                self.source.size = size;
                self.source.after_s_bar = Segment::Pp;
                self.source.change(Segment::S);
                self.rx.hunt();
                self.deadline = None;
                self.enter(Stage::CallSendTraining);
            }
            (Stage::AnswerAwaitJ, Event::J(size)) => {
                // 11.3.1.2.6 and 11.4.1.2.1: S, S-bar and TRN at the size asked.
                self.far_asked = Some(size);
                self.source.size = size;
                self.source.after_s_bar = Segment::Trn;
                self.source.change(Segment::S);
                self.enter(Stage::AnswerPhase4);
                // 11.4.2.2.2: E within 2500 ms and three round trips of S-bar.
                let wait = if self.settings.far_cme { 30.0 } else { 2.5 + (3.0 + E_PATIENCE) * self.rtd() + SLACK + 0.05 };
                self.deadline = Some((self.samples(wait), "no E from the call modem"));
            }
            (Stage::AnswerPhase4, Event::JPrime) => {
                // The call modem's TRN, at the size this end asked for.
                let asked = self.source.ask;
                self.rx.set_size(asked);
                self.listening.begin_trn(asked);
            }
            (_, Event::Mp(mp)) => {
                if self.far_mp.is_none() {
                    self.phase4_snr.get_or_insert(self.rx.snr_db());
                }
                self.far_mp = Some(mp);
                if mp.acknowledge {
                    self.far_acknowledged = true;
                }
            }
            (_, Event::E) => self.far_e = true,
            _ => {}
        }
    }

    /// The MP this end sends: what it can take and give.
    fn make_mp(&self) -> Mp {
        let s = &self.settings;
        let wide_cap = |rate: u8| if s.wide { rate } else { rate.min(12) };
        // What this end's receiver could take, by the signal to noise it
        // trained to, with the same allowance phase 2's projections make.
        let snr = 10f64.powf(self.rx.snr_db().min(60.0) / 10.0);
        let bits = (1.0 + snr / 10f64.powf(0.6)).log2();
        let receive = ((bits * s.receive.baud() / 2400.0).floor() as u8).clamp(1, probe::ceiling(s.receive.rate));
        let transmit = probe::ceiling(s.transmit.rate);
        let (call_to_answer, answer_to_call) = match s.role {
            Role::Call => (transmit, receive),
            Role::Answer => (receive, transmit),
        };
        Mp {
            call_to_answer: wide_cap(call_to_answer),
            answer_to_call: wide_cap(answer_to_call),
            auxiliary: false,
            trellis: Trellis::States16,
            non_linear: false,
            expanded_shaping: false,
            acknowledge: false,
            rates: Mp::rates_up_to(14),
            asymmetric: true,
            precoding: None,
        }
    }

    fn stage_step(&mut self) {
        if let Some(until) = self.md_until
            && self.now >= until
        {
            self.md_until = None;
            self.rx.hunt();
        }
        let baud = self.settings.transmit.baud();
        match self.stage {
            Stage::AnswerSendTraining => {
                if self.source.segment == Segment::Trn && self.source.count >= self.trn_symbols {
                    self.source.change(Segment::J);
                }
                if self.source.segment == Segment::J {
                    // 11.3.2.2.1: S-bar within 600 ms and a round trip of J.
                    self.rx.hunt();
                    self.deadline = Some((self.samples(0.6 + self.rtd() + SLACK), "no S from the call modem"));
                    self.enter(Stage::AnswerAwaitS);
                }
            }
            Stage::CallSendTraining => {
                if self.source.segment == Segment::Trn && self.source.count >= self.trn_symbols {
                    self.source.change(Segment::J);
                }
                if self.source.segment == Segment::J {
                    // 11.4.2.1.1: the answer modem's S-bar within 600 ms and a
                    // round trip of this end's J.
                    self.deadline = Some((self.samples(0.6 + self.rtd() + SLACK), "no S from the answer modem in phase 4"));
                    self.enter(Stage::CallAwaitS4);
                }
            }
            Stage::CallTraining4 => {
                // 11.4.1.1.2: TRN for 512T at least, then MP once trained, and
                // not past two seconds of TRN whatever.
                let sent = if self.source.segment == Segment::Trn { self.source.count } else { 0 };
                let trained = self.rx.is_trained() && self.listening.trn_symbols >= HEARD_TRN;
                if sent >= LEAST_TRN && (trained || sent as f64 >= MOST_TRN * baud) {
                    let ours = self.make_mp();
                    self.ours = Some(ours);
                    self.source.mp = ours;
                    self.source.change(Segment::Mp);
                    self.enter(Stage::CallMp);
                }
            }
            Stage::AnswerPhase4 => {
                // 11.4.1.2.2: MP after 512T of the call modem's TRN, and not
                // past two seconds and a round trip of this end's.
                let sent = if self.source.segment == Segment::Trn { self.source.count } else { 0 };
                let heard = self.listening.j_prime && self.listening.trn_symbols + self.listening.grace >= LEAST_TRN;
                if sent >= LEAST_TRN && (heard || sent as f64 >= (MOST_TRN + self.rtd()) * baud) {
                    let ours = self.make_mp();
                    self.ours = Some(ours);
                    self.source.mp = ours;
                    self.source.change(Segment::Mp);
                    self.enter(Stage::AnswerMp);
                }
            }
            Stage::CallMp | Stage::AnswerMp => {
                if self.far_mp.is_some()
                    && !self.source.mp.acknowledge
                    && let Some(ours) = self.ours
                {
                    // "complete sending the current MP sequence and then send
                    // MP' sequences" -- which the next repetition is.
                    self.source.mp = ours.acknowledged();
                }
                if !self.sent_e && self.source.acknowledged >= MP_PRIME_REPEATS && (self.far_acknowledged || self.far_e) {
                    self.source.change(Segment::E);
                    self.sent_e = true;
                }
                // Done once E is not just asked for but on the line: the pulse
                // carries symbols a way ahead of the sample going out, and a
                // modem that stopped as soon as E had been asked for would
                // hang up with the whole of a sixteen-point E still inside it.
                let flushed = self.source.segment == Segment::Silence && self.source.silent > 2 * Transmitter::lookahead();
                if self.sent_e && self.far_e && flushed {
                    self.status = Status::Done;
                    self.deadline = None;
                    self.enter(Stage::Finished);
                }
            }
            Stage::CallAwaitS
            | Stage::CallTraining
            | Stage::CallAwaitJ
            | Stage::CallAwaitS4
            | Stage::AnswerAwaitS
            | Stage::AnswerTraining
            | Stage::AnswerAwaitJ
            | Stage::Finished => {}
        }
    }
}

/// The data rates both ways, as multiples of 2400 -- call to answer, then
/// answer to call -- from the call modem's MP and the answer modem's
/// (11.4.1.1.3, 11.4.1.2.3).
///
/// Each is the fastest rate both ends enable that is no faster than either
/// end's limit for that direction; unless either end wants symmetric rates,
/// in which case both are the fastest no faster than any of the four limits.
pub fn negotiate(call: &Mp, answer: &Mp) -> (u8, u8) {
    let enabled = call.rates & answer.rates;
    let fastest = |limit: u8| (1..=limit.min(14)).rev().find(|r| enabled >> (r - 1) & 1 == 1).unwrap_or(0);
    if call.asymmetric && answer.asymmetric {
        (
            fastest(call.call_to_answer.min(answer.call_to_answer)),
            fastest(call.answer_to_call.min(answer.answer_to_call)),
        )
    } else {
        let limit = call.call_to_answer.min(call.answer_to_call).min(answer.call_to_answer).min(answer.answer_to_call);
        let rate = fastest(limit);
        (rate, rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::info::{Probed, SymbolRate};

    const FS: f64 = 16_000.0;

    fn settings(role: Role, round_trip: f64) -> Settings {
        let far = Info0 { constellation_1664: true, ..Info0::default() };
        let probed = Probed { high_carrier: false, pre_emphasis: 2, max_rate: 14 };
        let info1c = Info1c { probed: [probed; 6], ..Info1c::default() };
        let info1a = Info1a {
            min_power_reduction: 0,
            additional_power_reduction: 0,
            md_length: 0,
            probed: Probed { high_carrier: false, pre_emphasis: 1, max_rate: 14 },
            answer_to_call: SymbolRate::S3429,
            call_to_answer: SymbolRate::S3200,
            frequency_offset: None,
        };
        Settings::new(role, &far, &info1c, &info1a, round_trip, true)
    }

    /// Two ends of phases 3 and 4 on a line with a delay each way, a loss,
    /// noise, and the answer end's clock off the call end's.
    fn call(one_way: f64, noise_db: f64, ppm: f64, seconds: f64) -> (Modem, Modem) {
        let delay = (one_way * FS) as usize;
        let mut caller = Modem::new(settings(Role::Call, 2.0 * one_way), FS);
        let mut answerer = Modem::new(settings(Role::Answer, 2.0 * one_way), FS);
        let mut to_answer: VecDeque<f64> = std::iter::repeat_n(0.0, delay.max(1)).collect();
        let mut to_call: VecDeque<f64> = std::iter::repeat_n(0.0, delay.max(1)).collect();
        let loss = 10f64.powf(-15.0 / 20.0);
        let noise = 10f64.powf(-noise_db / 20.0) * 0.707 * loss;
        let mut seed = 0x1234_5678_u32;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 3.464
        };
        // The answer end runs on its own clock: its samples are taken
        // `ppm` apart from the call end's by resampling both ways.
        let mut up = dsp::Resampler::new(FS, FS * (1.0 + ppm * 1e-6));
        let mut down = dsp::Resampler::new(FS * (1.0 + ppm * 1e-6), FS);
        let (mut into_answer, mut out_of_answer): (VecDeque<f64>, VecDeque<f64>) = (VecDeque::new(), VecDeque::new());
        let mut buffer = Vec::new();
        for _ in 0..(seconds * FS) as usize {
            let heard_by_call = to_call.pop_front().unwrap() * loss + noise * rand();
            let from_call = caller.step(heard_by_call);
            to_answer.push_back(from_call);
            // Through the answer end's clock.
            buffer.clear();
            up.process(to_answer.pop_front().unwrap(), &mut buffer);
            into_answer.extend(buffer.iter().copied());
            while let Some(x) = into_answer.pop_front() {
                let from_answer = answerer.step(x * loss + noise * rand());
                buffer.clear();
                down.process(from_answer, &mut buffer);
                out_of_answer.extend(buffer.iter().copied());
            }
            to_call.push_back(out_of_answer.pop_front().unwrap_or(0.0));
            if caller.status() != Status::Running && answerer.status() != Status::Running {
                break;
            }
        }
        (caller, answerer)
    }

    fn check_done(caller: &Modem, answerer: &Modem) {
        for m in [caller, answerer] {
            println!(
                "{:?}: {} at {:.2} s, phase 3 {:?} dB, phase 4 {:?} dB, now {:.1} dB, drift {:.1} ppm, rates {:?}, far {:?}",
                m.settings().role,
                m.phase(),
                m.now as f64 / FS,
                m.phase3_snr(),
                m.phase4_snr(),
                m.snr(),
                m.drift_ppm(),
                m.rates(),
                m.far_mp()
            );
        }
        assert_eq!(caller.status(), Status::Done, "call modem stuck at {}", caller.phase());
        assert_eq!(answerer.status(), Status::Done, "answer modem stuck at {}", answerer.phase());
        // Each end asked for sixteen points and was given them.
        assert_eq!(caller.far_asked(), Some(Size::Sixteen));
        assert_eq!(answerer.far_asked(), Some(Size::Sixteen));
        // Each end read the other's MP, and ended on its MP'.
        let (call_mp, answer_mp) = (caller.our_mp().unwrap(), answerer.our_mp().unwrap());
        assert_eq!(caller.far_mp(), Some(answer_mp.acknowledged()));
        assert_eq!(answerer.far_mp(), Some(call_mp.acknowledged()));
        // And the two ends agree on the rates.
        let (call_tx, call_rx) = caller.rates().unwrap();
        let (answer_tx, answer_rx) = answerer.rates().unwrap();
        assert_eq!((call_tx, call_rx), (answer_rx, answer_tx));
        for m in [caller, answerer] {
            assert!(m.phase3_snr().unwrap() > 25.0, "{:?} phase 3 at {:?}", m.settings().role, m.phase3_snr());
        }
    }

    #[test]
    fn two_ends_train_and_exchange_mp_on_a_short_line() {
        let (caller, answerer) = call(0.010, 45.0, 0.0, 12.0);
        check_done(&caller, &answerer);
        // A clean line: 33 600 from call to answer is the call modem's own
        // ceiling at 3200 symbols a second, 31 200.
        let (call_tx, call_rx) = caller.rates().unwrap();
        assert_eq!(call_tx, 13, "call to answer at 3200 symbols a second");
        assert_eq!(call_rx, 14, "answer to call at 3429");
    }

    #[test]
    fn a_voip_round_trip_and_a_clock_114_ppm_out_are_survived() {
        let (caller, answerer) = call(0.580, 45.0, 114.0, 25.0);
        check_done(&caller, &answerer);
    }

    #[test]
    fn a_far_end_that_never_speaks_is_given_up_on() {
        let mut caller = Modem::new(settings(Role::Call, 1.0), FS);
        for _ in 0..(6.0 * FS) as usize {
            caller.step(0.0);
        }
        assert_eq!(caller.status(), Status::Failed("no J from the answer modem"));
        let mut answerer = Modem::new(settings(Role::Answer, 1.0), FS);
        for _ in 0..(4.0 * FS) as usize {
            answerer.step(0.0);
        }
        assert_eq!(answerer.status(), Status::Failed("no S from the call modem"));
    }

    #[test]
    fn rates_follow_both_ends_limits() {
        let call = Mp { call_to_answer: 14, answer_to_call: 10, rates: Mp::rates_up_to(14), asymmetric: true, ..Mp::default() };
        let answer = Mp { call_to_answer: 12, answer_to_call: 14, rates: Mp::rates_up_to(14) & !(1 << 11), asymmetric: true, ..Mp::default() };
        // 12 is not enabled at the answer end, so call to answer steps down to
        // 11; answer to call is the call end's limit of 10.
        assert_eq!(negotiate(&call, &answer), (11, 10));
        let symmetric = Mp { asymmetric: false, ..answer };
        assert_eq!(negotiate(&call, &symmetric), (10, 10));
    }
}
