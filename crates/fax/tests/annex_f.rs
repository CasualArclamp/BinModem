//! T.30 Annex F from end to end: two calls over a stand-in for V.34's
//! half-duplex modem.
//!
//! The stand-in is bits rather than signals, but it keeps the shape of the
//! real thing. A control channel both ways at once at 1200 bit/s, carrying
//! whatever each end gives it -- flags, frames, ones -- and nothing at all
//! from an end that has fallen silent. A primary channel one way, at the rate
//! an MPh exchange would have settled. The turnarounds between them taking
//! what V.34's own take (12.5, 12.6/V.34): 4T of ones and 70 ms of silence
//! into the primary channel's S, S-bar, PP and B1, and 35 ms of ones and 70 ms
//! out of it into the control channel's resynchronisation, which waits on the
//! far end's answer and so on the line's round trip. And a line with a delay
//! each way, up to the second and a half of round trip Rory's VoIP line has.
//!
//! What each end puts on the line is written down as it goes -- frames, runs
//! of ones, silence, pages -- so that the Figures of F.5 can be checked
//! against it.

use std::collections::VecDeque;

use fax::call::{Call, Line, Phase};
use fax::coding::Coding;
use fax::ecm::{self, PostMessage};
use fax::frames::{self, Message, Reader, Sender};
use fax::page::{Page, Resolution, WIDTH};
use fax::t30::{self, Command, Frame, Modulation};

const FS: f64 = 8000.0;
const DT: f64 = 1.0 / FS;

/// The control channel's rate: 1200 bit/s, which is what MPh gives it unless
/// both ends ask for 2400 (12.4/V.34, F.3.1.4).
const CONTROL_RATE: f64 = 1200.0;
/// Bits a modem holds between taking them from the procedure and sending
/// them: 27 ms at 1200 bit/s.
const DEPTH: usize = 32;
/// The control channel's turn-off, 4T of scrambled ones at 600 baud (12.6.3):
/// two bits a symbol.
const TURN_OFF_ONES: usize = 8;
/// The silence either side of the primary channel (12.5.1, 12.6.1).
const GAP: f64 = 0.070;
/// S 128T, S-bar 16T and PP 288T at 3200 baud, and B1's data frame.
const PRIMARY_TRAINING: f64 = 0.170;
/// The primary channel's turn-off, 35 ms of scrambled ones (12.5.3.1).
const PRIMARY_OFF: f64 = 0.035;
/// How much of the source's Sh the recipient hears before it answers.
const HEAR_SH: f64 = 0.040;
/// The recipient's answer: Sh, S-bar-h, ALT and E.
const ANSWER: f64 = 0.100;
/// Sh and S-bar-h, 24T and 8T: what the source hears of that answer before
/// it goes on.
const SH: f64 = 0.053;
/// The source's E, and ALT before it.
const E: f64 = 0.030;
/// What a start-up adds to a resynchronisation: PPh, ALT and two MPh (12.4).
const START_UP_EXTRA: f64 = 0.250;
/// Phases 2 to 4 of V.34's start-up, before the control channel carries
/// anything of T.30's.
const START_UP: f64 = 1.0;
/// How long the far end's control carrier has to have gone before the modem
/// says it has fallen silent.
const SILENCE: f64 = 0.020;

/// One sample of what an end puts on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Silent,
    /// The control channel's carrier, and the bit it carries this sample, if
    /// any.
    Control(Option<bool>),
    /// The control channel's carrier in a start-up or resynchronisation.
    ControlTraining,
    /// The primary channel's carrier in its training.
    PrimaryTraining,
    /// Primary channel data: `n` bits, the first in bit 0.
    Primary { bits: u8, n: u8 },
}

/// What an end put on the line, as the Figures of F.5 draw it.
#[derive(Debug, Clone, PartialEq)]
enum Sent {
    /// A frame on the control channel, and whether the line lost it.
    Frame(Message, bool),
    /// Ones on the control channel, this many in a row.
    Ones(usize),
    /// The end fell silent, ready for the page.
    Silent,
    /// A page burst on the primary channel: the numbers of its frames, and how
    /// many RCPs followed them.
    Page(Vec<u8>, usize),
}

/// Either end of a call, as the stand-in modem sees it: the procedure's side
/// of the join.
trait End {
    fn line(&self) -> Line;
    fn next_control_bit(&mut self) -> Option<bool>;
    fn control_bit(&mut self, bit: bool);
    fn set_far_silent(&mut self, silent: bool);
    fn next_fast_bit(&mut self) -> Option<bool>;
    fn fast_bits(&mut self, bits: &[bool]);
    fn set_fast_carrier(&mut self, up: bool);
    fn renegotiate(&self) -> bool;
    fn set_primary_rate(&mut self, bits_per_second: u32);
    fn control_restarted(&mut self);
    fn tick(&mut self, idle: bool);
    fn over(&self) -> bool;
}

impl End for Call {
    fn line(&self) -> Line {
        Call::line(self)
    }
    fn next_control_bit(&mut self) -> Option<bool> {
        Call::next_control_bit(self)
    }
    fn control_bit(&mut self, bit: bool) {
        Call::control_bit(self, bit);
    }
    fn set_far_silent(&mut self, silent: bool) {
        Call::set_far_silent(self, silent);
    }
    fn next_fast_bit(&mut self) -> Option<bool> {
        Call::next_fast_bit(self)
    }
    fn fast_bits(&mut self, bits: &[bool]) {
        Call::fast_bits(self, bits);
    }
    fn set_fast_carrier(&mut self, up: bool) {
        Call::set_fast_carrier(self, up);
    }
    fn renegotiate(&self) -> bool {
        Call::renegotiate(self)
    }
    fn set_primary_rate(&mut self, bits_per_second: u32) {
        Call::set_primary_rate(self, bits_per_second);
    }
    fn control_restarted(&mut self) {
        Call::control_restarted(self);
    }
    fn tick(&mut self, idle: bool) {
        Call::tick(self, idle);
    }
    fn over(&self) -> bool {
        self.phase().is_over()
    }
}

/// How the stand-in's modem is doing, and what it does next.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// V.34's start-up, before the control channel carries anything.
    StartUp(f64),
    /// The control channel, both ways.
    Control,
    /// Off the control channel: the bits still held, then 4T of ones, then
    /// the primary channel (the source) or silence (the recipient).
    Leaving { ones: usize, primary: bool },
    PrimaryGap(f64),
    PrimaryTraining(f64),
    /// The source, sending the page.
    Primary,
    /// The source, turning the primary channel off and coming back.
    PrimaryOff(f64),
    ControlGap(f64),
    /// The source's Sh, S-bar-h and ALT (or PPh, ALT and MPh) until it has
    /// heard enough of the recipient's answer, and then its E.
    Resync { heard: f64, e: f64 },
    /// The recipient, silent: listening for the primary channel, and for the
    /// source coming back to the control channel after it.
    Listen { heard: f64 },
    /// The recipient's answer to the source's resynchronisation.
    Answering(f64),
    Hung,
}

/// Follows a page burst as it goes, to say which frame is going.
#[derive(Debug, Default)]
struct Follow {
    last: u8,
    run: u32,
    looking: bool,
    octet: u8,
    got: u32,
    head: Vec<u8>,
}

impl Follow {
    /// One bit of a burst; the FCF of the frame in hand, and its number for an
    /// FCD, as soon as they have gone.
    fn bit(&mut self, bit: bool) -> Option<(u8, Option<u8>)> {
        self.last = self.last >> 1 | u8::from(bit) << 7;
        if self.last == 0x7E {
            self.looking = true;
            (self.run, self.octet, self.got) = (0, 0, 0);
            self.head.clear();
            return None;
        }
        if !self.looking {
            return None;
        }
        if self.run == 5 {
            self.run = 0;
            if bit {
                self.looking = false;
            }
            return None;
        }
        self.run = if bit { self.run + 1 } else { 0 };
        self.octet |= u8::from(bit) << self.got;
        self.got += 1;
        if self.got < 8 {
            return None;
        }
        self.head.push(self.octet);
        (self.octet, self.got) = (0, 0);
        match *self.head.as_slice() {
            [0xFF, 0x03, ecm::RCP] => {
                self.looking = false;
                Some((ecm::RCP, None))
            }
            [0xFF, 0x03, ecm::FCD, number] => {
                self.looking = false;
                Some((ecm::FCD, Some(number)))
            }
            _ if self.head.len() >= 4 => {
                self.looking = false;
                None
            }
            _ => None,
        }
    }
}

/// What the line does to a call: how long it is, and what it loses.
#[derive(Debug, Clone)]
struct Setup {
    round_trip: f64,
    /// The primary rate the start-up's MPh exchange settled.
    rate: u32,
    /// Whether the modems say when the far end falls silent, or leave the
    /// procedure to notice its flags stop.
    report_silence: bool,
    /// Control frames the line loses: the `n`th of this kind, counting from
    /// nought, from whichever end sends them.
    lose: Vec<(Frame, usize)>,
    /// Page frames the line spoils: by burst, counting from nought, and frame
    /// number.
    spoil: fn(usize, u8) -> bool,
    seconds: f64,
}

fn never(_: usize, _: u8) -> bool {
    false
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            round_trip: 0.05,
            rate: 28_800,
            report_silence: true,
            lose: Vec::new(),
            spoil: never,
            seconds: 120.0,
        }
    }
}

/// The stand-in for V.34's half-duplex modem at one end.
#[derive(Debug)]
struct Modem {
    mode: Mode,
    /// What the procedure wanted last sample, so that a change shows.
    was: Line,
    clock: f64,
    fast: f64,
    held: VecDeque<bool>,
    rate: u32,
    renegotiating: bool,
    renegotiations: usize,
    /// The procedure has run out of page bits.
    done: bool,
    /// How long since the far end's control carrier, and whether the
    /// procedure has been told the primary carrier is up.
    quiet: f64,
    carrier: bool,
    setup: Setup,
    /// What this end put on the line.
    sent: Vec<(f64, Sent)>,
    tap: Reader,
    ones: usize,
    counts: Vec<(Frame, usize)>,
    follow: Follow,
    page: Vec<u8>,
    rcps: usize,
    bursts: usize,
    spoil_next: bool,
}

impl Modem {
    fn new(setup: &Setup) -> Self {
        Self {
            mode: Mode::StartUp(START_UP),
            was: Line::Quiet,
            clock: 0.0,
            fast: 0.0,
            held: VecDeque::new(),
            rate: setup.rate,
            renegotiating: false,
            renegotiations: 0,
            done: false,
            quiet: 0.0,
            carrier: false,
            setup: setup.clone(),
            sent: Vec::new(),
            tap: Reader::new(),
            ones: 0,
            counts: Vec::new(),
            follow: Follow::default(),
            page: Vec::new(),
            rcps: 0,
            bursts: 0,
            spoil_next: false,
        }
    }

    /// One sample: what the far end sent arrives, and what this end sends goes.
    fn step(&mut self, end: &mut dyn End, arriving: Signal, t: f64) -> Signal {
        let want = end.line();
        if want != self.was {
            self.follow_the_procedure(end, want, t);
            self.was = want;
        }
        self.hear(end, arriving);
        let out = self.speak(end, t);
        let idle = self.held.is_empty() && (self.mode != Mode::Primary || self.done);
        end.tick(idle);
        if end.over() && self.mode != Mode::Hung {
            self.end_ones(t);
            self.mode = Mode::Hung;
        }
        out
    }

    /// The turnarounds the procedure asks for (section 10.1 of the plan).
    fn follow_the_procedure(&mut self, end: &mut dyn End, want: Line, t: f64) {
        match (self.mode, want) {
            // Circuit 105 off: onto the primary channel, or silent for it.
            (Mode::Control, Line::V34Primary | Line::V34PrimaryListen) => {
                self.end_ones(t);
                self.mode = Mode::Leaving {
                    ones: TURN_OFF_ONES,
                    primary: want == Line::V34Primary,
                };
            }
            // The page is over: the primary channel's turn-off, and back to
            // the control channel by its start-up if a new rate is wanted.
            (Mode::Primary, Line::V34Control | Line::V34Listen) => {
                self.renegotiating = end.renegotiate();
                self.sent.push((t, Sent::Page(std::mem::take(&mut self.page), self.rcps)));
                self.mode = Mode::PrimaryOff(PRIMARY_OFF);
            }
            _ => {}
        }
    }

    fn hear(&mut self, end: &mut dyn End, arriving: Signal) {
        self.quiet = match arriving {
            Signal::Control(_) | Signal::ControlTraining => 0.0,
            _ => self.quiet + DT,
        };
        if self.setup.report_silence {
            end.set_far_silent(self.quiet > SILENCE);
        }
        match self.mode {
            Mode::Control => {
                if let Signal::Control(Some(bit)) = arriving {
                    end.control_bit(bit);
                }
            }
            Mode::Listen { heard } => {
                let carrier = matches!(arriving, Signal::PrimaryTraining | Signal::Primary { .. });
                if carrier != self.carrier {
                    end.set_fast_carrier(carrier);
                    self.carrier = carrier;
                }
                if let Signal::Primary { bits, n } = arriving {
                    let bits: Vec<bool> = (0..n).map(|i| bits >> i & 1 == 1).collect();
                    end.fast_bits(&bits);
                }
                let heard = if arriving == Signal::ControlTraining { heard + DT } else { 0.0 };
                self.mode = if heard >= HEAR_SH {
                    Mode::Answering(ANSWER)
                } else {
                    Mode::Listen { heard }
                };
            }
            Mode::Resync { heard, e } if heard < SH => {
                if matches!(arriving, Signal::ControlTraining | Signal::Control(_)) {
                    self.mode = Mode::Resync { heard: heard + DT, e };
                }
            }
            _ => {}
        }
    }

    fn speak(&mut self, end: &mut dyn End, t: f64) -> Signal {
        match self.mode {
            Mode::StartUp(left) => {
                if left > DT {
                    self.mode = Mode::StartUp(left - DT);
                } else {
                    self.back_on_control(end);
                }
                Signal::Silent
            }
            Mode::Control => {
                if !self.control_bit_due() {
                    return Signal::Control(None);
                }
                while self.held.len() < DEPTH {
                    match end.next_control_bit() {
                        Some(bit) => self.take(bit, t),
                        None => break,
                    }
                }
                Signal::Control(self.held.pop_front())
            }
            Mode::Leaving { ones, primary } => {
                if !self.control_bit_due() {
                    return Signal::Control(None);
                }
                if let Some(bit) = self.held.pop_front() {
                    return Signal::Control(Some(bit));
                }
                if ones > 0 {
                    self.mode = Mode::Leaving { ones: ones - 1, primary };
                    return Signal::Control(Some(true));
                }
                self.mode = if primary {
                    Mode::PrimaryGap(GAP)
                } else {
                    self.sent.push((t, Sent::Silent));
                    Mode::Listen { heard: 0.0 }
                };
                Signal::Silent
            }
            Mode::PrimaryGap(left) => {
                self.mode = if left > DT {
                    Mode::PrimaryGap(left - DT)
                } else {
                    Mode::PrimaryTraining(PRIMARY_TRAINING)
                };
                Signal::Silent
            }
            Mode::PrimaryTraining(left) => {
                if left > DT {
                    self.mode = Mode::PrimaryTraining(left - DT);
                } else {
                    self.mode = Mode::Primary;
                    self.done = false;
                    self.follow = Follow::default();
                    self.rcps = 0;
                }
                Signal::PrimaryTraining
            }
            Mode::Primary => {
                let n = self.primary_bits_due();
                let mut bits = 0u8;
                for i in 0..n {
                    let bit = match end.next_fast_bit() {
                        Some(bit) => self.page_bit(bit),
                        // Nothing more to send, and 105 still on: marks.
                        None => {
                            self.done = true;
                            true
                        }
                    };
                    bits |= u8::from(bit) << i;
                }
                Signal::Primary { bits, n }
            }
            Mode::PrimaryOff(left) => {
                self.mode = if left > DT {
                    Mode::PrimaryOff(left - DT)
                } else {
                    self.bursts += 1;
                    Mode::ControlGap(GAP)
                };
                let n = self.primary_bits_due();
                Signal::Primary { bits: 0xFF, n }
            }
            Mode::ControlGap(left) => {
                self.mode = if left > DT {
                    Mode::ControlGap(left - DT)
                } else {
                    Mode::Resync { heard: 0.0, e: 0.0 }
                };
                Signal::Silent
            }
            Mode::Resync { heard, e } => {
                if heard >= SH {
                    let wait = E + if self.renegotiating { START_UP_EXTRA } else { 0.0 };
                    if e + DT >= wait {
                        if self.renegotiating {
                            // What MPh settles is the modem's business; this
                            // one takes a rung down, as a line that would not
                            // carry the rate would have it do.
                            self.rate = (self.rate - 2400).max(2400);
                            self.renegotiations += 1;
                            end.set_primary_rate(self.rate);
                        }
                        self.renegotiating = false;
                        self.back_on_control(end);
                    } else {
                        self.mode = Mode::Resync { heard, e: e + DT };
                    }
                }
                Signal::ControlTraining
            }
            Mode::Listen { .. } | Mode::Hung => Signal::Silent,
            Mode::Answering(left) => {
                if left > DT {
                    self.mode = Mode::Answering(left - DT);
                } else {
                    self.back_on_control(end);
                }
                Signal::ControlTraining
            }
        }
    }

    fn back_on_control(&mut self, end: &mut dyn End) {
        self.mode = Mode::Control;
        self.carrier = false;
        end.set_fast_carrier(false);
        end.control_restarted();
    }

    fn control_bit_due(&mut self) -> bool {
        self.clock += CONTROL_RATE / FS;
        if self.clock >= 1.0 {
            self.clock -= 1.0;
            true
        } else {
            false
        }
    }

    fn primary_bits_due(&mut self) -> u8 {
        self.fast += f64::from(self.rate) / FS;
        let n = self.fast.floor();
        self.fast -= n;
        n as u8
    }

    /// A control bit taken from the procedure: written down, and spoiled if it
    /// finishes a frame the line is to lose.
    fn take(&mut self, bit: bool, t: f64) {
        self.held.push_back(bit);
        if bit {
            self.ones += 1;
        } else {
            self.end_ones(t);
        }
        if let Some(message) = self.tap.feed(bit) {
            let n = self.counts.iter().filter(|(f, _)| *f == message.frame).count();
            self.counts.push((message.frame, n));
            let lost = self.setup.lose.contains(&(message.frame, n));
            if lost {
                // Inside its frame check, which the closing flag follows.
                let at = self.held.len().checked_sub(12).expect("the frame had gone");
                self.held[at] = !self.held[at];
            }
            self.sent.push((t, Sent::Frame(message, lost)));
        }
    }

    /// A run of ones over: written down if it was the source's.
    fn end_ones(&mut self, t: f64) {
        if self.ones >= fax::call::ONES {
            self.sent.push((t, Sent::Ones(self.ones)));
        }
        self.ones = 0;
    }

    /// A page bit taken from the procedure: written down, and spoiled if it is
    /// in a frame the line is to spoil.
    fn page_bit(&mut self, bit: bool) -> bool {
        let on_the_line = if self.spoil_next {
            self.spoil_next = false;
            !bit
        } else {
            bit
        };
        match self.follow.bit(bit) {
            Some((ecm::FCD, Some(number))) => {
                self.page.push(number);
                self.spoil_next = (self.setup.spoil)(self.bursts, number);
            }
            Some((ecm::RCP, _)) => self.rcps += 1,
            _ => {}
        }
        on_the_line
    }
}

/// What a call came to.
#[derive(Debug)]
struct Record {
    caller: Vec<(f64, Sent)>,
    answerer: Vec<(f64, Sent)>,
    renegotiations: usize,
    rate: u32,
    seconds: f64,
}

/// Run a call to the end, or to `setup.seconds`.
fn run(caller: &mut dyn End, answerer: &mut dyn End, setup: &Setup) -> Record {
    let delay = ((setup.round_trip / 2.0 * FS).round() as usize).max(1);
    let mut to_caller: VecDeque<Signal> = std::iter::repeat_n(Signal::Silent, delay).collect();
    let mut to_answerer = to_caller.clone();
    let mut a = Modem::new(setup);
    let mut b = Modem::new(setup);
    caller.set_primary_rate(setup.rate);
    answerer.set_primary_rate(setup.rate);
    let mut t = 0.0;
    for _ in 0..(setup.seconds * FS) as usize {
        let into_a = to_caller.pop_front().unwrap_or(Signal::Silent);
        let into_b = to_answerer.pop_front().unwrap_or(Signal::Silent);
        let out_a = a.step(caller, into_a, t);
        let out_b = b.step(answerer, into_b, t);
        to_caller.push_back(out_b);
        to_answerer.push_back(out_a);
        // The far end's modem learns a new rate from the same MPh exchange.
        if a.rate != b.rate {
            let rate = a.rate.min(b.rate);
            (a.rate, b.rate) = (rate, rate);
            caller.set_primary_rate(rate);
            answerer.set_primary_rate(rate);
        }
        t += DT;
        if caller.over() && answerer.over() {
            break;
        }
    }
    Record {
        caller: a.sent,
        answerer: b.sent,
        renegotiations: a.renegotiations + b.renegotiations,
        rate: a.rate,
        seconds: t,
    }
}

/// What an end sent, as the Figures name it.
fn names(sent: &[(f64, Sent)]) -> Vec<String> {
    sent.iter()
        .map(|(_, s)| match s {
            Sent::Frame(m, lost) => {
                let mut name = m.frame.name().to_owned();
                if matches!(m.frame, Frame::Pps | Frame::Eor)
                    && let Some(command) = m.fif.first().and_then(|&c| PostMessage::from_code(c))
                {
                    name += match command {
                        PostMessage::Null => "-NULL",
                        PostMessage::Eom => "-EOM",
                        PostMessage::Mps => "-MPS",
                        PostMessage::Eop => "-EOP",
                    };
                }
                if *lost {
                    name += " (lost)";
                }
                name
            }
            Sent::Ones(_) => "ones".to_owned(),
            Sent::Silent => "silent".to_owned(),
            Sent::Page(..) => "page".to_owned(),
        })
        .collect()
}

/// When an end first sent a frame of this kind, and the frame.
fn first(sent: &[(f64, Sent)], frame: Frame) -> (f64, Message) {
    sent.iter()
        .find_map(|(t, s)| match s {
            Sent::Frame(m, _) if m.frame == frame => Some((*t, m.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {} in {:?}", frame.name(), names(sent)))
}

fn pages(sent: &[(f64, Sent)]) -> Vec<(Vec<u8>, usize)> {
    sent.iter()
        .filter_map(|(_, s)| match s {
            Sent::Page(frames, rcps) => Some((frames.clone(), *rcps)),
            _ => None,
        })
        .collect()
}

/// A page with something recognisable on it, and `mark` to tell it apart.
fn a_page(rows: usize, resolution: Resolution, mark: usize) -> Page {
    Page {
        lines: (0..rows)
            .map(|y| {
                (0..WIDTH)
                    .map(|x| {
                        ((x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30)
                            || (x / 100 == mark && y % 3 == 0)
                    })
                    .collect()
            })
            .collect(),
        resolution,
    }
}

/// A page busy enough to come to a good many frames in any coding: what a
/// test needs one of them missing from.
fn a_busy_page(rows: usize) -> Page {
    Page {
        lines: (0..rows)
            .map(|y| {
                (0..WIDTH)
                    .map(|x| {
                        let h = (x as u32).wrapping_mul(2_654_435_761) ^ (y as u32).wrapping_mul(40_503);
                        h.wrapping_mul(2_246_822_519) >> 29 == 0
                    })
                    .collect()
            })
            .collect(),
        resolution: Resolution::Standard,
    }
}

fn caller(pages: Vec<Page>) -> Call {
    let mut call = Call::originate_pages(FS, "61399990000", pages);
    call.start_annex_f();
    call
}

fn answerer(jbig: bool) -> Call {
    let mut call = Call::answer(FS, "61388880000");
    call.set_jbig(jbig);
    call.start_annex_f();
    call
}

fn received(call: &mut Call) -> Vec<(usize, Page)> {
    std::iter::from_fn(|| call.take_received()).collect()
}

fn assert_done(caller: &Call, answerer: &Call, record: &Record) {
    for (end, call) in [("caller", caller), ("answerer", answerer)] {
        assert_eq!(
            call.phase(),
            Phase::Done,
            "the {end} ended at {} ({:?}) after {:.1} s: caller {:?}, answerer {:?}",
            call.phase().name(),
            call.trouble,
            record.seconds,
            names(&record.caller),
            names(&record.answerer),
        );
    }
}

/// Figure F.5-1 from the end of the modem's start-up, and Figure F.5-3.
///
/// The answering end's CSI and DIS; the TSI and DCS, answered with CFR and
/// no training check; the ones, the silence, and the page; and at its end the
/// post-message command on the control channel, the MCF, and the disconnect.
#[test]
fn a_page_goes_over_v34_half_duplex_as_figures_f5_1_and_f5_3_draw_it() {
    let page = a_page(120, Resolution::Standard, 3);
    let mut caller = caller(vec![page.clone()]);
    let mut answerer = answerer(true);
    let record = run(&mut caller, &mut answerer, &Setup::default());
    assert_done(&caller, &answerer, &record);
    assert_eq!(caller.trouble, None);
    assert_eq!(answerer.trouble, None);
    assert_eq!(names(&record.caller), ["TSI", "DCS", "ones", "page", "PPS-EOP", "DCN"]);
    assert_eq!(names(&record.answerer), ["CSI", "DIS", "CFR", "silent", "MCF"]);

    let got = received(&mut answerer);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.lines, page.lines, "the page came out different");
    assert_eq!(answerer.coding(), Coding::Jbig);

    // The capabilities and the command as Table 2 has them under V.34: the
    // DIS says V.8 and error correction; the DCS names no rate (Note 33).
    let (_, dis) = first(&record.answerer, Frame::Dis);
    assert!(t30::bit(&dis.fif, 6) && t30::bit(&dis.fif, 27), "{:02x?}", dis.fif);
    let (_, dcs) = first(&record.caller, Frame::Dcs);
    assert_eq!(t30::field_of(&dcs.fif, 11, 14), 0, "{:02x?}", dcs.fif);
    assert!(t30::bit(&dcs.fif, 27) && t30::commands_jbig(&dcs.fif));
    // A page, and its three RCPs, on the primary channel.
    let bursts = pages(&record.caller);
    assert_eq!(bursts.len(), 1);
    assert_eq!(bursts[0].1, ecm::RCP_FRAMES);
    assert!(caller.error_correction() && answerer.error_correction());
}

/// Figure F.5-2: between pages, PPS-MPS on the control channel, MCF, and the
/// ones and silence again before the next page.
#[test]
fn pages_follow_one_another_as_figure_f5_2_draws_it() {
    for (jbig, resolution) in [
        (true, Resolution::Standard),
        (true, Resolution::Fine),
        (false, Resolution::Standard),
        (false, Resolution::Fine),
    ] {
        let sent: Vec<Page> = (0..3).map(|n| a_page(60 + 20 * n, resolution, n + 2)).collect();
        let mut caller = caller(sent.clone());
        let mut answerer = answerer(jbig);
        let record = run(&mut caller, &mut answerer, &Setup::default());
        assert_done(&caller, &answerer, &record);
        let want = if jbig { Coding::Jbig } else { Coding::Mmr };
        assert_eq!(caller.coding(), want);
        assert_eq!(answerer.coding(), want);
        assert_eq!(
            names(&record.caller),
            [
                "TSI", "DCS", "ones", "page", "PPS-MPS", "ones", "page", "PPS-MPS", "ones", "page",
                "PPS-EOP", "DCN"
            ],
            "{want:?}, {resolution:?}"
        );
        assert_eq!(
            names(&record.answerer),
            ["CSI", "DIS", "CFR", "silent", "MCF", "silent", "MCF", "silent", "MCF"],
            "{want:?}, {resolution:?}"
        );
        let got = received(&mut answerer);
        let numbers: Vec<usize> = got.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, [1, 2, 3]);
        for ((n, page), want) in got.iter().zip(&sent) {
            assert_eq!(page.resolution, resolution, "page {n}");
            assert_eq!(page.lines, want.lines, "page {n} came out different in {:?}", caller.coding());
        }
    }
}

/// A standard page and a fine one in the same call go at the resolution the
/// call settled, as they do under clause 5: the standard one twice over.
#[test]
fn a_standard_page_after_a_fine_one_goes_fine() {
    let fine = a_page(40, Resolution::Fine, 1);
    let standard = a_page(30, Resolution::Standard, 5);
    let mut caller = caller(vec![fine.clone(), standard.clone()]);
    let mut answerer = answerer(true);
    let record = run(&mut caller, &mut answerer, &Setup::default());
    assert_done(&caller, &answerer, &record);
    let got = received(&mut answerer);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].1.lines, fine.lines);
    let doubled: Vec<Vec<bool>> = standard.lines.iter().flat_map(|l| [l.clone(), l.clone()]).collect();
    assert_eq!(got[1].1.lines, doubled);
    assert_eq!(got[1].1.resolution, Resolution::Fine);
}

/// Rory's VoIP line: a second and a half of round trip. Nothing about the
/// procedure changes, but everything that waits on the far end waits that
/// much longer -- the source's ones above all, which run until the
/// recipient's silence has come back down the line.
#[test]
fn a_round_trip_of_a_second_and_a_half_is_only_slower() {
    let sent: Vec<Page> = (0..2).map(|n| a_page(80, Resolution::Fine, n + 4)).collect();
    let mut caller = caller(sent.clone());
    let mut answerer = answerer(true);
    let setup = Setup {
        round_trip: 1.5,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    assert_eq!(caller.trouble, None);
    let got = received(&mut answerer);
    assert_eq!(got.len(), 2);
    for ((_, page), want) in got.iter().zip(&sent) {
        assert_eq!(page.lines, want.lines);
    }
    assert_eq!(
        names(&record.caller),
        ["TSI", "DCS", "ones", "page", "PPS-MPS", "ones", "page", "PPS-EOP", "DCN"]
    );
    // Every run of ones lasted the round trip, and forty more.
    for (_, s) in &record.caller {
        if let Sent::Ones(n) = s {
            assert!(*n as f64 > 1.5 * CONTROL_RATE, "{n} ones");
        }
    }
}

/// Figures F.5-1 and F.5-2 again, with the modem never saying the far end has
/// fallen silent: the flags stopping is enough (F.3.2.3's "or absence of
/// flags").
#[test]
fn the_far_ends_flags_stopping_is_as_good_as_its_silence() {
    let sent = vec![a_page(50, Resolution::Standard, 1), a_page(50, Resolution::Standard, 6)];
    let mut caller = caller(sent.clone());
    let mut answerer = answerer(false);
    let setup = Setup {
        report_silence: false,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    let got = received(&mut answerer);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].1.lines, sent[1].lines);
}

/// Frames lost on the primary channel cost a PPR and a retransmission of just
/// those frames, and not the page: Figure F.5-6's PPS and PPR, without its
/// change of rate.
#[test]
fn frames_the_primary_channel_spoils_cost_a_ppr_and_not_the_page() {
    fn spoil(burst: usize, frame: u8) -> bool {
        burst == 0 && (frame == 1 || frame == 3)
    }
    let page = a_busy_page(60);
    let mut caller = caller(vec![page.clone()]);
    let mut answerer = answerer(false);
    let setup = Setup {
        spoil,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    assert_eq!(
        names(&record.caller),
        ["TSI", "DCS", "ones", "page", "PPS-EOP", "ones", "page", "PPS-EOP", "DCN"]
    );
    assert_eq!(names(&record.answerer), ["CSI", "DIS", "CFR", "silent", "PPR", "silent", "MCF"]);
    let bursts = pages(&record.caller);
    assert!(bursts[0].0.len() > 4, "only {} frames: {:?}", bursts[0].0.len(), bursts[0].0);
    assert_eq!(bursts[1].0, [1, 3], "the retransmission was not the frames asked for");
    let got = received(&mut answerer);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.lines, page.lines, "the page was not put right");
    assert_eq!(record.renegotiations, 0);
}

/// Four PPRs in a row for one block. No CTC under V.34 (F.3.4.5 Note 1): the
/// frames go again, and the control channel comes back after them by its
/// start-up, with a new rate from the MPh exchange; this time they arrive.
#[test]
fn four_pprs_in_a_row_bring_a_new_rate_and_not_a_ctc() {
    fn spoil(burst: usize, frame: u8) -> bool {
        burst < 4 && frame == 2
    }
    let page = a_busy_page(40);
    let mut caller = caller(vec![page.clone()]);
    let mut answerer = answerer(false);
    let setup = Setup {
        spoil,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    let said = names(&record.answerer);
    assert_eq!(said.iter().filter(|s| *s == "PPR").count(), 4, "{said:?}");
    assert_eq!(said.last().map(String::as_str), Some("MCF"));
    let commands = names(&record.caller);
    assert!(!commands.iter().any(|c| c == "CTC" || c.starts_with("EOR")), "{commands:?}");
    assert_eq!(pages(&record.caller).len(), 5, "{commands:?}");
    assert_eq!(record.renegotiations, 1, "no new rate was asked for");
    assert_eq!(record.rate, 26_400);
    assert_eq!(caller.rate(), 26_400);
    assert_eq!(received(&mut answerer)[0].1.lines, page.lines);
}

/// And a block that never gets through: four PPRs, a new rate, four more, and
/// then the end of retransmission -- EOR, answered ERR -- and the disconnect.
#[test]
fn a_block_that_never_gets_through_ends_in_eor() {
    fn spoil(_: usize, frame: u8) -> bool {
        frame == 2
    }
    let mut caller = caller(vec![a_busy_page(40)]);
    let mut answerer = answerer(false);
    let setup = Setup {
        spoil,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    let commands = names(&record.caller);
    let said = names(&record.answerer);
    assert_eq!(said.iter().filter(|s| *s == "PPR").count(), 8, "{said:?}");
    assert_eq!(said.last().map(String::as_str), Some("ERR"), "{said:?}");
    assert_eq!(&commands[commands.len() - 2..], ["EOR-EOP", "DCN"], "{commands:?}");
    assert_eq!(pages(&record.caller).len(), 8);
    assert_eq!(record.renegotiations, 1, "a second new rate for the same block");
}

/// Figure F.5-7: the MCF is lost, T4 runs out, the PPS goes again on the
/// control channel and is answered again -- without the page, which arrived.
#[test]
fn a_lost_mcf_is_recovered_by_t4_as_figure_f5_7_draws_it() {
    let sent = vec![a_page(50, Resolution::Standard, 2), a_page(50, Resolution::Standard, 7)];
    let mut caller = caller(sent.clone());
    let mut answerer = answerer(true);
    let setup = Setup {
        lose: vec![(Frame::Mcf, 0)],
        round_trip: 0.6,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    assert_eq!(
        names(&record.caller),
        ["TSI", "DCS", "ones", "page", "PPS-MPS", "PPS-MPS", "ones", "page", "PPS-EOP", "DCN"]
    );
    assert_eq!(
        names(&record.answerer),
        ["CSI", "DIS", "CFR", "silent", "MCF (lost)", "MCF", "silent", "MCF"]
    );
    // "T4 elapsed" between the two.
    let pps: Vec<f64> = record
        .caller
        .iter()
        .filter_map(|(t, s)| matches!(s, Sent::Frame(m, _) if m.frame == Frame::Pps).then_some(*t))
        .collect();
    assert!(pps[1] - pps[0] > fax::call::T4_SECONDS, "{pps:?}");
    let got = received(&mut answerer);
    assert_eq!(got.len(), 2, "a page twice, or not at all");
    assert_eq!(got[1].1.lines, sent[1].lines);
}

/// A lost CFR: T4 again, and the command again, which is answered again.
#[test]
fn a_lost_cfr_is_recovered_by_t4() {
    let page = a_page(50, Resolution::Fine, 2);
    let mut caller = caller(vec![page.clone()]);
    let mut answerer = answerer(true);
    let setup = Setup {
        lose: vec![(Frame::Cfr, 0)],
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    assert_eq!(
        names(&record.caller),
        ["TSI", "DCS", "TSI", "DCS", "ones", "page", "PPS-EOP", "DCN"]
    );
    assert_eq!(
        names(&record.answerer),
        ["CSI", "DIS", "CFR (lost)", "CFR", "silent", "MCF"]
    );
    assert_eq!(received(&mut answerer)[0].1.lines, page.lines);
}

/// A lost PPR: the recipient has answered and is flagging, waiting for the
/// source's ones (F.3.4.4), and hears the PPS again instead when the source's
/// T4 runs out. It answers again from the frames it has, and the
/// retransmission that follows is still only the frames it asked for.
#[test]
fn a_lost_ppr_is_recovered_by_t4_and_still_costs_only_the_frames_asked_for() {
    fn spoil(burst: usize, frame: u8) -> bool {
        burst == 0 && frame == 2
    }
    let page = a_busy_page(60);
    let mut caller = caller(vec![page.clone()]);
    let mut answerer = answerer(false);
    let setup = Setup {
        lose: vec![(Frame::Ppr, 0)],
        spoil,
        ..Setup::default()
    };
    let record = run(&mut caller, &mut answerer, &setup);
    assert_done(&caller, &answerer, &record);
    assert_eq!(
        names(&record.caller),
        ["TSI", "DCS", "ones", "page", "PPS-EOP", "PPS-EOP", "ones", "page", "PPS-EOP", "DCN"]
    );
    assert_eq!(
        names(&record.answerer),
        ["CSI", "DIS", "CFR", "silent", "PPR (lost)", "PPR", "silent", "MCF"]
    );
    let bursts = pages(&record.caller);
    assert_eq!(bursts.len(), 2);
    assert_eq!(bursts[1].0, [2], "the retransmission was not the frame asked for");
    assert_eq!(received(&mut answerer)[0].1.lines, page.lines, "the page was not put right");
}

/// A far end that says only what it is told to, in order: for the parts of
/// Annex F this crate's own source never does.
#[derive(Debug)]
struct Script {
    steps: VecDeque<Step>,
    begun: bool,
    sender: Sender,
    reader: Reader,
    ones: usize,
    far_silent: bool,
    done: bool,
}

#[derive(Debug)]
enum Step {
    /// Frames on the control channel.
    Say(Vec<Message>),
    /// Flags until a frame of this kind arrives.
    Await(Frame),
    /// Ones until the far end falls silent (F.3.2.3).
    Ones,
    /// A page burst on the primary channel.
    Page(VecDeque<bool>),
}

impl Script {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: steps.into(),
            begun: false,
            sender: Sender::new(),
            reader: Reader::new(),
            ones: 0,
            far_silent: false,
            done: false,
        }
    }

    fn next_step(&mut self) {
        self.steps.pop_front();
        self.begun = false;
    }
}

impl End for Script {
    fn line(&self) -> Line {
        match self.steps.front() {
            Some(Step::Say(_)) => Line::V34Control,
            Some(Step::Await(_)) => Line::V34Listen,
            Some(Step::Ones) => Line::V34Ones,
            Some(Step::Page(_)) => Line::V34Primary,
            None if self.done => Line::Quiet,
            None => Line::V34Control,
        }
    }
    fn next_control_bit(&mut self) -> Option<bool> {
        let quiet = self.sender.is_empty() && !self.sender.mid_flag();
        match self.steps.front() {
            None if quiet => None,
            Some(Step::Ones) if quiet => {
                self.ones += 1;
                Some(true)
            }
            _ => Some(self.sender.next_bit_or_flag()),
        }
    }
    fn control_bit(&mut self, bit: bool) {
        if let Some(message) = self.reader.feed(bit)
            && let Some(Step::Await(frame)) = self.steps.front()
            && *frame == message.frame
        {
            self.next_step();
        }
    }
    fn set_far_silent(&mut self, silent: bool) {
        self.far_silent = silent;
    }
    fn next_fast_bit(&mut self) -> Option<bool> {
        match self.steps.front_mut() {
            Some(Step::Page(bits)) => bits.pop_front(),
            _ => None,
        }
    }
    fn fast_bits(&mut self, _: &[bool]) {}
    fn set_fast_carrier(&mut self, _: bool) {}
    fn renegotiate(&self) -> bool {
        false
    }
    fn set_primary_rate(&mut self, _: u32) {}
    fn control_restarted(&mut self) {
        self.reader = Reader::new();
    }
    fn tick(&mut self, idle: bool) {
        if !self.begun {
            self.begun = true;
            if let Some(Step::Say(messages)) = self.steps.front() {
                self.sender.send_flagged(messages, frames::V34_FLAGS);
            }
            if matches!(self.steps.front(), Some(Step::Ones)) {
                self.ones = 0;
            }
        }
        match self.steps.front() {
            Some(Step::Say(_)) if self.sender.is_empty() => self.next_step(),
            Some(Step::Ones) if self.ones >= fax::call::ONES && self.far_silent => self.next_step(),
            Some(Step::Page(bits)) if bits.is_empty() && idle => self.next_step(),
            None if idle && self.sender.is_empty() => self.done = true,
            _ => {}
        }
    }
    fn over(&self) -> bool {
        self.done
    }
}

/// A page as a burst on the primary channel: A.3.1's synchronisation, the
/// frames, and three RCPs.
fn burst(lines: &[Vec<bool>], resolution: Resolution, rate: u32) -> (VecDeque<bool>, usize) {
    let octets = ecm::pack(&Coding::Mmr.encode(lines, resolution, 0));
    let data = ecm::frames(&octets, ecm::FRAME_OCTETS);
    let numbered: Vec<(u8, &[u8])> = data.iter().enumerate().map(|(i, d)| (i as u8, d.as_slice())).collect();
    (ecm::partial_page(&numbered, rate).into(), data.len())
}

fn dcs(fine: bool) -> Message {
    Message::new(Frame::Dcs, true).with_fif(&t30::v34_command(Command {
        modulation: Modulation::V29,
        bits_per_second: 9600,
        fine,
        scan_line_field: 0b111,
        coding: Coding::Mmr,
        optional_l0: false,
        error_correction: true,
    }))
}

/// Figure F.5-4: a mode change without a change of rate. The source ends a
/// page with PPS-EOM; the recipient answers MCF, waits out T2 -- which its
/// flags and the source's do not put back (F.3.2.3 Note 2) -- and sends its
/// DIS again; a new DCS, here for a fine page after a standard one, is
/// answered CFR, and the line turns round for it.
///
/// This crate's own source never needs a mode change -- it sends every page
/// at the terms it chose first -- so the source here is a script of what the
/// Figure draws, and it is the recipient that is being checked.
#[test]
fn a_change_of_mode_goes_as_figure_f5_4_draws_it() {
    const RATE: u32 = 28_800;
    let first_page = a_page(40, Resolution::Standard, 3);
    let second_page = a_page(80, Resolution::Fine, 9);
    let (one, frames_one) = burst(&first_page.lines, Resolution::Standard, RATE);
    let (two, frames_two) = burst(&second_page.lines, Resolution::Fine, RATE);
    let tsi = Message::new(Frame::Tsi, true)
        .and_more()
        .with_fif(&t30::identification_field("61377770000"));
    let pps = |command: PostMessage, page: u8, frames: usize| {
        Message::new(Frame::Pps, true).with_fif(&ecm::pps_field(command, page, 0, frames))
    };
    let mut source = Script::new(vec![
        Step::Await(Frame::Dis),
        Step::Say(vec![tsi.clone(), dcs(false)]),
        Step::Await(Frame::Cfr),
        Step::Ones,
        Step::Page(one),
        Step::Say(vec![pps(PostMessage::Eom, 0, frames_one)]),
        Step::Await(Frame::Mcf),
        Step::Await(Frame::Dis),
        Step::Say(vec![tsi, dcs(true)]),
        Step::Await(Frame::Cfr),
        Step::Ones,
        Step::Page(two),
        Step::Say(vec![pps(PostMessage::Eop, 1, frames_two)]),
        Step::Await(Frame::Mcf),
        Step::Say(vec![Message::new(Frame::Dcn, true)]),
    ]);
    let mut answerer = answerer(false);
    let setup = Setup {
        rate: RATE,
        ..Setup::default()
    };
    let record = run(&mut source, &mut answerer, &setup);
    assert!(source.done, "the script stopped at {:?}", source.steps.front());
    assert_eq!(answerer.phase(), Phase::Done, "{:?}", answerer.trouble);
    assert_eq!(answerer.trouble, None);
    assert_eq!(
        names(&record.caller),
        ["TSI", "DCS", "ones", "page", "PPS-EOM", "TSI", "DCS", "ones", "page", "PPS-EOP", "DCN"]
    );
    assert_eq!(
        names(&record.answerer),
        ["CSI", "DIS", "CFR", "silent", "MCF", "CSI", "DIS", "CFR", "silent", "MCF"]
    );
    // "T2 elapsed" from the end of the MCF to the DIS, and the flags on both
    // sides of the line meanwhile did not put it back.
    let said: Vec<(f64, Frame)> = record
        .answerer
        .iter()
        .filter_map(|(t, s)| match s {
            Sent::Frame(m, _) => Some((*t, m.frame)),
            _ => None,
        })
        .collect();
    let mcf = said.iter().find(|(_, f)| *f == Frame::Mcf).unwrap().0;
    let dis = said.iter().filter(|(_, f)| *f == Frame::Dis).nth(1).unwrap().0;
    assert!(
        dis - mcf > fax::call::T2_SECONDS,
        "the DIS came {:.2} s after the MCF",
        dis - mcf
    );
    let got = received(&mut answerer);
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].1.resolution, &got[0].1.lines), (Resolution::Standard, &first_page.lines));
    assert_eq!((got[1].1.resolution, &got[1].1.lines), (Resolution::Fine, &second_page.lines));
}
