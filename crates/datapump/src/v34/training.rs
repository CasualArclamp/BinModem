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
//!
//! Data mode can go back to MP without going back to the start (11.6 and
//! 11.7): either end sends S, S-bar and, for a new rate, TRN, then both swap
//! MP sequences at four points and send E and B1 again. A rate renegotiation
//! comes out at new rates, and a cleardown -- MP asking for nothing either
//! way -- ends the call.
//!
//! ```text
//! initiating  data S S' TRN MP MP MP MP' MP' E B1 data
//! responding  data             S S' TRN MP' MP' E B1 data
//! ```

use std::collections::VecDeque;

use dsp::Complex;

use super::constellation::Point;
use super::data::{Acquired, Acquirer, Decoder, Encoder, Params};
use super::frame::Framing;
use super::info::{Info0, Info1a, Info1c};
use super::mp::{Finder, Found, Mp, Trellis};
use super::phase2::Role;
use super::probe;
use super::qam::{Band, Transmitter};
use super::receiver::{self, Heard, Receiver, Reference};
use super::signals::{self, J_FOUR, J_PRIME, J_SIXTEEN, Reader, Sender, Size};
use super::trellis::Code;
use crate::v32::Mode;

/// How phases 3 and 4 are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// Both ends have sent E, but no data mode could be set up between the
    /// two MPs -- no rate both ends enable.
    Done,
    /// In data mode: B1 has arrived, at these rates in bit/s.
    Connected { transmit: u32, receive: u32 },
    /// Data mode was up and the two ends are back at MP: a rate renegotiation
    /// or a cleardown, from either end.
    Retraining,
    /// A cleardown is over, and with it the call.
    ClearedDown,
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

/// Symbols of S in a row before data mode believes the far end has stopped
/// sending data for a rate renegotiation or a cleardown.
const S_HEARD: usize = 24;

/// TRN this end sends in a rate renegotiation before its MP. Its receiver is
/// trained already and so is the far end's, so this is only a little: the
/// modem the first live renegotiation came from sent about a hundred and
/// forty symbols of it.
const RENEGOTIATION_TRN: usize = 256;

/// What reading the far end's data costs the trellis a 4D symbol, averaged, in
/// grid units squared, past which the decoder has lost its place: data mode
/// reads a few tenths on the lines V.34 has run over, and a decoder reading
/// from the wrong place well over one.
const STRAYED_COST: f64 = 1.0;

/// 2D symbols of that before looking for the place again: a third of a
/// second or so.
const STRAYED_SYMBOLS: usize = 1000;

/// The far end's 2D symbols, while this end listens for its E, whose average
/// distance from sixteen points is past this -- or which the receiver cannot
/// lock on to as sixteen points at all -- for longer than a slip takes to find
/// again, with a signal there, for a data signal rather than MP.
const UNLIKE_ERROR: f64 = 0.02;
const UNLIKE_SYMBOLS: usize = 150;

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
    /// B1, and data after it.
    Data,
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
    /// Data mode's encoder, made ready before E goes so that B1 follows it
    /// with nothing between.
    encoder: Option<Encoder>,
    /// Data waiting to go.
    data: VecDeque<bool>,
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
            encoder: None,
            data: VecDeque::new(),
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
                        // 11.4.1.1.4 and 11.4.1.2.4: "After sending an E
                        // sequence, the ... modem shall send B1".
                        let next = if self.encoder.is_some() { Segment::Data } else { Segment::Silence };
                        self.start(next);
                        continue;
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Segment::Data => {
                    // Data stops where it stands for S (11.6.1.1.1, 11.6.1.2.2).
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Segment::Silence);
                        continue;
                    };
                    // B1 is one data frame of scrambled ones (10.1.3.1): the
                    // encoder's first data frame takes ones whatever is
                    // waiting.
                    let b1 = encoder.mapping_frames() < encoder.params().framing.p as u64;
                    let data = &mut self.data;
                    self.count += 1;
                    return encoder.next_symbol(&mut || if b1 { true } else { data.pop_front().unwrap_or(true) });
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
    /// For a test: an E that never arrives, as a slip can make it.
    #[cfg(test)]
    deaf_to_e: bool,
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
            #[cfg(test)]
            deaf_to_e: false,
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
                #[cfg(test)]
                Some(Found::E) if self.deaf_to_e => {}
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

/// Watches the far end's equalised symbols for S, and then for S turning into
/// S-bar: how a rate renegotiation or a cleardown begins in data mode.
///
/// S is point 0 and point 0 turned a quarter, alternately, so each symbol is a
/// quarter turn from the one before and the same as the one before that --
/// which scrambled data keeps up for a symbol or two and never for twenty-four,
/// even at 4800 where data is four points too. S-bar is S turned half way, so
/// where one becomes the other, two symbols in a row are opposite the ones two
/// before them.
#[derive(Debug, Clone, Default)]
struct SWatch {
    last: VecDeque<Complex>,
    run: usize,
    heard: bool,
    flips: usize,
    /// Symbols since S was heard.
    since: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watched {
    Nothing,
    S,
    SBar,
}

impl SWatch {
    fn feed(&mut self, y: Complex) -> Watched {
        self.last.push_back(y);
        if self.last.len() > 3 {
            self.last.pop_front();
        }
        if self.last.len() < 3 {
            return Watched::Nothing;
        }
        let (two_back, one_back) = (self.last[0], self.last[1]);
        // Four points at unit power sit at magnitude one, a squared distance
        // of two from their neighbours.
        let near = |a: Complex, b: Complex| (a - b).norm_sqr() < 0.1;
        let point = (0.6..1.5).contains(&y.norm_sqr());
        let quarter = near(y, one_back * Complex::I) || near(y, -(one_back * Complex::I));
        if !self.heard {
            self.run = if point && quarter && near(y, two_back) { self.run + 1 } else { 0 };
            if self.run >= S_HEARD {
                self.heard = true;
                return Watched::S;
            }
            return Watched::Nothing;
        }
        self.since += 1;
        self.flips = if point && quarter && near(y, -two_back) { self.flips + 1 } else { 0 };
        if self.flips == 2 {
            return Watched::SBar;
        }
        Watched::Nothing
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
    // Both.
    Data,
    /// Back at MP from data mode, as either end (11.6, 11.7).
    Renegotiation,
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
            Self::Data => "V.34 data",
            Self::Renegotiation => "V.34 rate renegotiation",
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

    /// Data mode's decoder, once the far end's E has come.
    decoder: Option<Decoder>,
    /// Bits of the far end's B1 still to come, and how many of them were not
    /// the ones B1 is.
    b1_left: usize,
    b1_errors: usize,
    /// Data received.
    received: Vec<bool>,

    /// The far end's S, in data mode and in a renegotiation this end began.
    s_watch: SWatch,
    /// Whether the far end's S-bar has been heard in this renegotiation, and
    /// so its MP is being listened for.
    far_s_bar: bool,
    /// Whether this end began the renegotiation, and whether it is a
    /// cleardown -- one this end began, or one the far end's MP asks for.
    initiated: bool,
    clearing: bool,
    /// Renegotiations since the call began, either end's.
    renegotiations: u32,
    /// The most this end's MP offers to receive, if less than it could.
    receive_cap: Option<u8>,
    /// The precoding coefficients this end's transmitter uses: zero until a
    /// Type 1 MP says otherwise, and kept through a Type 0 one.
    precoding: [super::mp::Coefficient; 3],

    /// Looking for where the far end's data frames are, and the receiver's
    /// slips as last seen, since one is a reason to look.
    acquirer: Option<Acquirer>,
    slips_seen: u32,
    /// 2D symbols the decoder has cost what a lost place does, in a row.
    strayed: usize,
    /// While listening for E: how far from sixteen points the far end's
    /// symbols are, averaged, for how many symbols in a row too far, and for
    /// how many they have been any way off at all -- which is how far back
    /// the loops were last taught by symbols that were what they seemed.
    unlike: f64,
    unlike_run: usize,
    off_run: usize,
    /// Times the frames were found again.
    found_again: u32,
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
            decoder: None,
            b1_left: 0,
            b1_errors: 0,
            received: Vec::new(),
            s_watch: SWatch::default(),
            far_s_bar: false,
            initiated: false,
            clearing: false,
            renegotiations: 0,
            receive_cap: None,
            precoding: [(0, 0); 3],
            acquirer: None,
            slips_seen: 0,
            strayed: 0,
            unlike: 0.0,
            unlike_run: 0,
            off_run: 0,
            found_again: 0,
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
            Status::ClearedDown => "V.34 cleared down",
            _ if self.stage == Stage::Renegotiation && self.clearing => "V.34 cleardown",
            _ => self.stage.name(),
        }
    }

    /// Rate renegotiations and cleardowns since the call began, from either
    /// end.
    pub fn renegotiations(&self) -> u32 {
        self.renegotiations
    }

    /// Times data mode's frames were found again from the data itself: after
    /// a slip, a decoder that had lost its place, or an E that never came.
    pub fn found_again(&self) -> u32 {
        self.found_again
    }

    /// Start a rate renegotiation from data mode (11.6.1.1), offering to
    /// receive no faster than `receive`, a multiple of 2400. False, and
    /// nothing done, outside data mode.
    pub fn renegotiate(&mut self, receive: u8) -> bool {
        if self.stage != Stage::Data {
            return false;
        }
        self.receive_cap = Some(receive);
        self.begin_renegotiation(true);
        true
    }

    /// End the call from data mode the way 11.7.1 does: S, S-bar, and MP
    /// asking for nothing either way until both ends are sending MP'. False,
    /// and nothing done, outside data mode.
    pub fn clear_down(&mut self) -> bool {
        if self.stage != Stage::Data {
            return false;
        }
        self.clearing = true;
        self.begin_renegotiation(true);
        // "transmit signal S-bar for 16T, and send MP sequences requesting
        // zeroes" -- no TRN.
        let ours = self.make_mp();
        self.ours = Some(ours);
        self.source.mp = ours;
        self.source.after_s_bar = Segment::Mp;
        true
    }

    /// Back from data mode to S, S-bar and MP (11.6.1.1.1, 11.6.1.2.2).
    ///
    /// The responding end starts on hearing S rather than on S turning into
    /// S-bar, which is the recommendation's cue, and so is some thirty
    /// milliseconds sooner: the first live renegotiation was begun by a far end
    /// that gave up on an answer a round trip and a quarter of a second after
    /// its S-bar, and over VoIP every millisecond of that went on the line.
    fn begin_renegotiation(&mut self, initiating: bool) {
        self.renegotiations += 1;
        self.initiated = initiating;
        self.status = Status::Retraining;
        self.acquirer = None;
        self.b1_left = 0;
        self.listening = Listening::new(self.far_mode);
        self.far_s_bar = false;
        if initiating {
            // The far end goes on sending data until it hears this end's S,
            // and that data is still data: 104 is clamped on hearing the far
            // end's S (11.6.1.1.2), not before.
            self.s_watch = SWatch::default();
        } else {
            self.clamp();
        }
        self.far_mp = None;
        self.far_acknowledged = false;
        self.far_e = false;
        self.sent_e = false;
        self.ours = None;
        // "The TRN signal and the MP and E sequences are all sent using a
        // 4-point constellation during rate renegotiation."
        self.source.size = Size::Four;
        self.source.after_s_bar = Segment::Trn;
        self.source.acknowledged = 0;
        self.source.sending_acknowledged = false;
        self.source.encoder = None;
        self.source.change(Segment::S);
        // 11.6.2: E within 2500 ms and two round trips of this end's S-bar if
        // it began, three if it answered; CME makes it thirty seconds.
        let trips = if initiating { 2.0 } else { 3.0 };
        let wait = if self.settings.far_cme { 30.0 } else { 2.5 + (trips + E_PATIENCE) * self.rtd() + SLACK + 0.05 };
        self.deadline = Some((self.samples(wait), "no E in the rate renegotiation"));
        self.enter(Stage::Renegotiation);
    }

    /// The far end's S: clamp 104 and decide against four points.
    fn clamp(&mut self) {
        self.decoder = None;
        self.rx.set_size(Size::Four);
    }

    /// Look for where the far end's data frames are, from its data.
    fn search(&mut self) {
        let Some(params) = self.receive_params() else { return };
        if self.decoder.is_none() && self.acquirer.is_none() {
            // From listening for E: the far end's data has been decided as
            // sixteen points since the error first rose, and every loop
            // taught by the decisions.
            self.rx.rewind(self.off_run as u64 + 32);
        }
        let acquirer = Acquirer::new(params);
        self.rx.set_grid(acquirer.grid_scale(), acquirer.extent());
        self.decoder = None;
        self.acquirer = Some(acquirer);
        self.strayed = 0;
        self.unlike = 0.0;
        self.unlike_run = 0;
        self.off_run = 0;
    }

    /// The far end's data, decoded, to where it goes: B1's ones counted, and
    /// everything after them received.
    fn take_decoded(&mut self) {
        let Some(decoder) = self.decoder.as_mut() else { return };
        for bit in decoder.take_bits() {
            if self.b1_left > 0 {
                self.b1_left -= 1;
                self.b1_errors += usize::from(!bit);
            } else {
                self.received.push(bit);
            }
        }
    }

    /// While MP' comes and E is waited for: a far end whose symbols have
    /// stopped being sixteen points, with the receiver locked on to them, has
    /// gone into data mode, and its E was lost -- the second live call to
    /// reach MP' had a VoIP slip land on it.
    fn watch_for_lost_e(&mut self, error: f64) {
        let waiting = matches!(self.stage, Stage::CallMp | Stage::AnswerMp | Stage::Renegotiation)
            && self.far_acknowledged
            && !self.far_e
            && (self.stage != Stage::Renegotiation || self.far_s_bar);
        if !waiting {
            return;
        }
        self.unlike += 0.05 * (error - self.unlike);
        let unlike = self.unlike > UNLIKE_ERROR || self.rx.is_lost();
        self.unlike_run = if unlike && self.rx.level() > 1e-4 { self.unlike_run + 1 } else { 0 };
        // A tenth of the way there is off already.
        self.off_run = if self.unlike > 0.1 * UNLIKE_ERROR || self.rx.is_lost() { self.off_run + 1 } else { 0 };
        if self.unlike_run > UNLIKE_SYMBOLS {
            self.search();
        }
    }

    /// The far end's S turned into S-bar: its TRN or MP is next.
    fn heard_far_s_bar(&mut self) {
        self.far_s_bar = true;
        self.listening = Listening::new(self.far_mode);
        self.listening.begin_trn(Size::Four);
        // Past the rest of S-bar, which the watch heard two symbols of.
        self.listening.grace += signals::S_BAR_SYMBOLS - 2;
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

    /// Points the far end's signal is being decided against: four or sixteen
    /// in training, and data mode's L once B1 has begun.
    pub fn constellation_size(&self) -> usize {
        match (self.decoder.as_ref(), self.rx.size()) {
            (Some(decoder), _) => decoder.params().framing.l,
            (None, Size::Four) => 4,
            (None, Size::Sixteen) => 16,
        }
    }

    /// The largest coordinate those points reach, at the unit mean power
    /// [`Self::constellation_point`] reports them in: 1/sqrt(2) for four,
    /// 3/sqrt(10) for sixteen, and about one and a half for data mode's
    /// shaped hundreds.
    pub fn constellation_peak(&self) -> f64 {
        match (self.decoder.as_ref(), self.rx.size()) {
            (Some(decoder), _) => decoder.peak(),
            (None, Size::Four) => std::f64::consts::FRAC_1_SQRT_2,
            (None, Size::Sixteen) => 3.0 / 10f64.sqrt(),
        }
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
        let live = |status: Status| matches!(status, Status::Running | Status::Connected { .. } | Status::Retraining);
        while let Some(heard) = self.rx.heard() {
            if live(self.status) {
                self.heard(heard);
            }
        }
        if live(self.status) {
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
                if matches!(self.stage, Stage::Data | Stage::Renegotiation) && !self.far_s_bar {
                    match self.s_watch.feed(symbol.point) {
                        Watched::S if self.stage == Stage::Data => self.begin_renegotiation(false),
                        // The far end answering a renegotiation this end began.
                        Watched::S => self.clamp(),
                        Watched::SBar => self.heard_far_s_bar(),
                        // S-bar missed, to a slip or to noise: TRN is surely
                        // under way by now.
                        Watched::Nothing if self.s_watch.heard && self.s_watch.since > signals::S_SYMBOLS + 32 => self.heard_far_s_bar(),
                        Watched::Nothing => {}
                    }
                    if self.stage == Stage::Renegotiation && !self.far_s_bar && self.decoder.is_none() {
                        return;
                    }
                }
                // A slip loses or repeats symbols, and with them the place in
                // the frames.
                if self.rx.slips() != self.slips_seen {
                    self.slips_seen = self.rx.slips();
                    if self.decoder.is_some() || self.acquirer.is_some() {
                        self.search();
                    }
                }
                if let Some(acquirer) = self.acquirer.as_mut() {
                    match acquirer.feed(symbol.point) {
                        Acquired::Searching => {}
                        Acquired::Found(decoder) => {
                            self.acquirer = None;
                            self.found_again += 1;
                            // An E that never came was sent all the same: its
                            // data is here. Where its B1 was is not known, and
                            // B1 is ones, which the far end's idle is too.
                            self.far_e = true;
                            self.b1_left = 0;
                            self.decoder = Some(*decoder);
                            self.take_decoded();
                        }
                        Acquired::Nothing if self.decoder.is_none() && self.stage != Stage::Data => {
                            // Not data mode after all: listen for E again.
                            self.acquirer = None;
                            // Sixteen points in phase 4, as this end's J asked;
                            // four in a renegotiation.
                            let size = if self.stage == Stage::Renegotiation { Size::Four } else { self.source.ask };
                            self.rx.set_size(size);
                        }
                        Acquired::Nothing => self.search(),
                    }
                    return;
                }
                if let Some(decoder) = self.decoder.as_mut() {
                    decoder.feed(symbol.point);
                    let strayed = decoder.path_cost() > STRAYED_COST;
                    self.take_decoded();
                    self.strayed = if strayed { self.strayed + 1 } else { 0 };
                    if self.strayed > STRAYED_SYMBOLS {
                        self.search();
                    }
                    return;
                }
                self.watch_for_lost_e(symbol.error);
                if self.acquirer.is_some() {
                    return;
                }
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
                if let Some(coefficients) = mp.precoding {
                    self.precoding = coefficients;
                }
                if self.stage == Stage::Renegotiation && mp.call_to_answer == 0 && mp.answer_to_call == 0 {
                    self.clearing = true;
                }
                self.far_mp = Some(mp);
                if mp.acknowledge {
                    self.far_acknowledged = true;
                }
            }
            (_, Event::E) => {
                self.far_e = true;
                // "After receiving a 20-bit E sequence, the modem shall
                // condition its receiver to receive B1" (11.4.1.1.5): the next
                // symbol is B1's first.
                if let Some(params) = self.receive_params() {
                    let decoder = Decoder::new(params);
                    self.rx.set_grid(decoder.grid_scale(), decoder.extent());
                    self.b1_left = params.framing.n;
                    self.decoder = Some(decoder);
                }
            }
            _ => {}
        }
    }

    /// Data mode as this end sends it: at the rate the two MPs came to, with
    /// the trellis code, shaping, non-linear encoding and precoding the far
    /// end's MP asked for.
    fn transmit_params(&self) -> Option<Params> {
        let far = self.far_mp?;
        let (transmit, _) = self.rates()?;
        Some(Params {
            framing: Framing::new(self.settings.transmit.rate, u32::from(transmit) * 2400, false, far.expanded_shaping)?,
            code: code_of(far.trellis),
            nonlinear: far.non_linear,
            // "Prior to receiving the first MP sequence in Phase 4, the
            // precoding coefficients are initialized to 0. If a Type 0 sequence
            // is received, the precoding coefficients are unaffected."
            precoding: self.precoding,
            mode: own_mode(self.settings.role),
        })
    }

    /// Data mode as the far end sends it: as this end's own MP asked.
    fn receive_params(&self) -> Option<Params> {
        let ours = self.ours?;
        let (_, receive) = self.rates()?;
        Some(Params {
            framing: Framing::new(self.settings.receive.rate, u32::from(receive) * 2400, false, ours.expanded_shaping)?,
            code: code_of(ours.trellis),
            nonlinear: ours.non_linear,
            precoding: ours.precoding.unwrap_or([(0, 0); 3]),
            mode: self.far_mode,
        })
    }

    /// Data received, taken.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    /// Data to send.
    pub fn send_bits(&mut self, bits: &[bool]) {
        self.source.data.extend(bits.iter().copied());
    }

    /// Data waiting to go, less what the next mapping frame will take at
    /// once. A mapping frame's bits all go into the encoder together -- 79 of
    /// them at 33 600 -- and whatever is feeding this has to have that many
    /// ready or the frame is made up with idle ones, which in the middle of an
    /// HDLC frame is an abort.
    pub fn pending_bits(&self) -> usize {
        let frame = self.source.encoder.as_ref().map_or(0, |e| e.params().framing.b);
        self.source.data.len().saturating_sub(frame)
    }

    /// Bits of the far end's B1 that were not ones.
    pub fn b1_errors(&self) -> usize {
        self.b1_errors
    }

    /// What the far end's data costs the trellis decoder a 4D symbol, in grid
    /// units squared: about the noise when all is well.
    pub fn path_cost(&self) -> Option<f64> {
        self.decoder.as_ref().map(Decoder::path_cost)
    }

    /// Whether the far end's signal is there to be demodulated.
    pub fn carrier(&self) -> bool {
        matches!(self.status, Status::Connected { .. } | Status::Retraining) && self.rx.level() > 1e-4
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
        let receive = self.receive_cap.map_or(receive, |cap| receive.min(cap.max(1)));
        let transmit = probe::ceiling(s.transmit.rate);
        let (call_to_answer, answer_to_call) = match s.role {
            _ if self.clearing && self.initiated => (0, 0),
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
            Stage::CallMp | Stage::AnswerMp => self.exchange_mp(),
            Stage::Renegotiation => {
                if self.ours.is_none() && self.source.segment == Segment::Trn && self.source.count >= RENEGOTIATION_TRN {
                    let ours = self.make_mp();
                    self.ours = Some(ours);
                    self.source.mp = ours;
                    self.source.change(Segment::Mp);
                }
                if self.ours.is_some() {
                    self.exchange_mp();
                }
            }
            Stage::CallAwaitS
            | Stage::CallTraining
            | Stage::CallAwaitJ
            | Stage::CallAwaitS4
            | Stage::AnswerAwaitS
            | Stage::AnswerTraining
            | Stage::AnswerAwaitJ
            | Stage::Data
            | Stage::Finished => {}
        }
    }

    /// MP, MP', E and B1: the end of phase 4, and of a renegotiation.
    fn exchange_mp(&mut self) {
        if self.far_mp.is_some()
            && !self.source.mp.acknowledge
            && let Some(ours) = self.ours
        {
            // "complete sending the current MP sequence and then send
            // MP' sequences" -- which the next repetition is.
            self.source.mp = ours.acknowledged();
        }
        if self.clearing && self.cleared() {
            self.status = Status::ClearedDown;
            self.deadline = None;
            self.source.pending = None;
            self.source.start(Segment::Silence);
            self.rx.idle();
            self.enter(Stage::Finished);
            return;
        }
        if !self.clearing && !self.sent_e && self.source.acknowledged >= MP_PRIME_REPEATS && (self.far_acknowledged || self.far_e) {
            self.source.encoder = self.transmit_params().map(Encoder::new);
            self.source.change(Segment::E);
            self.sent_e = true;
        }
        if self.sent_e && self.far_e {
            if self.decoder.is_some() && self.source.encoder.is_some() {
                // 11.4.1.1.5: "After receiving B1, the modem shall
                // unclamp Circuit 104, turn on Circuit 109, and begin
                // demodulating data."
                if self.b1_left == 0 && self.source.segment == Segment::Data {
                    if let Some((transmit, receive)) = self.rates() {
                        self.status = Status::Connected {
                            transmit: u32::from(transmit) * 2400,
                            receive: u32::from(receive) * 2400,
                        };
                    }
                    self.deadline = None;
                    // Listening for the next renegotiation's S.
                    self.s_watch = SWatch::default();
                    self.far_s_bar = false;
                    self.receive_cap = None;
                    self.enter(Stage::Data);
                }
            } else {
                // No data mode between these MPs. Done once E is not
                // just asked for but on the line: the pulse carries
                // symbols a way ahead of the sample going out.
                let flushed = self.source.segment == Segment::Silence && self.source.silent > 2 * Transmitter::lookahead();
                if flushed {
                    self.status = Status::Done;
                    self.deadline = None;
                    self.enter(Stage::Finished);
                }
            }
        }
    }

    /// Whether a cleardown has gone as far as 11.7 takes it: the responding
    /// end once it has the initiating end's MP and has sent an MP' back, the
    /// initiating end once it is both sending and receiving MP'.
    fn cleared(&self) -> bool {
        let sent = self.source.acknowledged >= 1;
        if self.initiated { sent && self.far_acknowledged } else { sent && self.far_mp.is_some() }
    }
}

/// The trellis code an MP asks for.
fn code_of(trellis: Trellis) -> Code {
    match trellis {
        Trellis::States16 => Code::States16,
        Trellis::States32 => Code::States32,
        Trellis::States64 => Code::States64,
    }
}

/// The scrambler of this end's own transmitter.
fn own_mode(role: Role) -> Mode {
    match role {
        Role::Call => Mode::Call,
        Role::Answer => Mode::Answer,
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
        let (caller, answerer, _, _) = call_with_data(one_way, noise_db, ppm, seconds, &[], &[]);
        (caller, answerer)
    }

    /// Two ends joined by a line: a delay each way, a loss, noise, and the
    /// answer end's clock `ppm` off the call end's.
    struct Link {
        caller: Modem,
        answerer: Modem,
        to_answer: VecDeque<f64>,
        to_call: VecDeque<f64>,
        loss: f64,
        noise: f64,
        seed: u32,
        up: dsp::Resampler,
        down: dsp::Resampler,
        into_answer: VecDeque<f64>,
        out_of_answer: VecDeque<f64>,
        buffer: Vec<f64>,
        /// Samples of the call end's clock so far.
        n: usize,
        /// Data each end received.
        at_call: Vec<bool>,
        at_answer: Vec<bool>,
    }

    impl Link {
        fn new(one_way: f64, noise_db: f64, ppm: f64) -> Self {
            let delay = ((one_way * FS) as usize).max(1);
            let loss = 10f64.powf(-15.0 / 20.0);
            Self {
                caller: Modem::new(settings(Role::Call, 2.0 * one_way), FS),
                answerer: Modem::new(settings(Role::Answer, 2.0 * one_way), FS),
                to_answer: std::iter::repeat_n(0.0, delay).collect(),
                to_call: std::iter::repeat_n(0.0, delay).collect(),
                loss,
                noise: 10f64.powf(-noise_db / 20.0) * 0.707 * loss,
                seed: 0x1234_5678,
                // The answer end's samples are taken `ppm` apart from the
                // call end's by resampling both ways.
                up: dsp::Resampler::new(FS, FS * (1.0 + ppm * 1e-6)),
                down: dsp::Resampler::new(FS * (1.0 + ppm * 1e-6), FS),
                into_answer: VecDeque::new(),
                out_of_answer: VecDeque::new(),
                buffer: Vec::new(),
                n: 0,
                at_call: Vec::new(),
                at_answer: Vec::new(),
            }
        }

        fn rand(&mut self) -> f64 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            (f64::from(self.seed) / f64::from(u32::MAX) - 0.5) * 3.464
        }

        /// One sample of the call end's clock.
        fn step(&mut self) {
            let heard_by_call = self.to_call.pop_front().unwrap() * self.loss + self.noise * self.rand();
            let from_call = self.caller.step(heard_by_call);
            self.to_answer.push_back(from_call);
            self.buffer.clear();
            self.up.process(self.to_answer.pop_front().unwrap(), &mut self.buffer);
            self.into_answer.extend(self.buffer.iter().copied());
            while let Some(x) = self.into_answer.pop_front() {
                let noise = self.noise * self.rand();
                let from_answer = self.answerer.step(x * self.loss + noise);
                self.buffer.clear();
                self.down.process(from_answer, &mut self.buffer);
                self.out_of_answer.extend(self.buffer.iter().copied());
            }
            self.to_call.push_back(self.out_of_answer.pop_front().unwrap_or(0.0));
            self.at_answer.extend(self.answerer.take_bits());
            self.at_call.extend(self.caller.take_bits());
            self.n += 1;
        }

        /// Steps until `done` says so or `seconds` have gone, whichever is
        /// first; true if `done` did.
        fn run_until(&mut self, seconds: f64, mut done: impl FnMut(&Self) -> bool) -> bool {
            let end = self.n + (seconds * FS) as usize;
            while self.n < end {
                self.step();
                if done(self) {
                    return true;
                }
            }
            false
        }

        fn end(&mut self, role: Role) -> &mut Modem {
            match role {
                Role::Call => &mut self.caller,
                Role::Answer => &mut self.answerer,
            }
        }

        fn both_connected(&self) -> bool {
            let up = |m: &Modem| matches!(m.status(), Status::Connected { .. });
            up(&self.caller) && up(&self.answerer)
        }

        /// A VoIP jitter buffer's slip on the way to `towards`: the next
        /// twenty milliseconds never arrive, or arrive twice.
        fn slip(&mut self, towards: Role, dropped: bool) {
            let n = (0.020 * FS) as usize;
            let queue = match towards {
                Role::Call => &mut self.to_call,
                Role::Answer => &mut self.to_answer,
            };
            if dropped {
                queue.drain(..n);
            } else {
                let again: Vec<f64> = queue.iter().take(n).copied().collect();
                for x in again.into_iter().rev() {
                    queue.push_front(x);
                }
            }
        }
    }

    /// The same, and once both ends are in data mode `from_call` and
    /// `from_answer` sent for half a second; what each end received after its
    /// B1 comes back too.
    fn call_with_data(
        one_way: f64,
        noise_db: f64,
        ppm: f64,
        seconds: f64,
        call_data: &[bool],
        answer_data: &[bool],
    ) -> (Modem, Modem, Vec<bool>, Vec<bool>) {
        let mut link = Link::new(one_way, noise_db, ppm);
        let ended = |m: &Modem| matches!(m.status(), Status::Failed(_) | Status::Done);
        let connected = link.run_until(seconds, |l| l.both_connected() || ended(&l.caller) || ended(&l.answerer));
        if connected && link.both_connected() {
            link.caller.send_bits(call_data);
            link.answerer.send_bits(answer_data);
            // Half a second and the round trip: long enough for the data to
            // cross.
            link.run_until(0.5 + 2.0 * one_way, |_| false);
        }
        (link.caller, link.answerer, link.at_call, link.at_answer)
    }

    fn check_connected(caller: &Modem, answerer: &Modem) {
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
        assert!(matches!(caller.status(), Status::Connected { .. }), "call modem stuck at {}", caller.phase());
        assert!(matches!(answerer.status(), Status::Connected { .. }), "answer modem stuck at {}", answerer.phase());
        // Each end's B1 arrived as the ones it is.
        assert_eq!((caller.b1_errors(), answerer.b1_errors()), (0, 0));
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
        check_connected(&caller, &answerer);
        // A clean line: 33 600 from call to answer is the call modem's own
        // ceiling at 3200 symbols a second, 31 200.
        let (call_tx, call_rx) = caller.rates().unwrap();
        assert_eq!(call_tx, 13, "call to answer at 3200 symbols a second");
        assert_eq!(call_rx, 14, "answer to call at 3429");
    }

    #[test]
    fn a_voip_round_trip_and_a_clock_114_ppm_out_are_survived() {
        let (caller, answerer) = call(0.580, 45.0, 114.0, 25.0);
        check_connected(&caller, &answerer);
    }

    #[test]
    fn data_crosses_both_ways_once_connected() {
        // A pattern each way that no idle line of ones could be mistaken for.
        let from_call: Vec<bool> = (0..6000).map(|i| (i * 37 + 11) % 7 < 3).collect();
        let from_answer: Vec<bool> = (0..6000).map(|i| (i * 13 + 5) % 5 < 2).collect();
        let (caller, answerer, at_call, at_answer) = call_with_data(0.030, 45.0, 60.0, 14.0, &from_call, &from_answer);
        check_connected(&caller, &answerer);
        let contains = |haystack: &[bool], needle: &[bool]| haystack.windows(needle.len()).any(|w| w == needle);
        assert!(contains(&at_answer, &from_call), "the answer end did not receive the call end's data ({} bits)", at_answer.len());
        assert!(contains(&at_call, &from_answer), "the call end did not receive the answer end's data ({} bits)", at_call.len());
    }

    /// A pattern no idle line of ones could be mistaken for.
    fn pattern(length: usize, step: usize) -> Vec<bool> {
        (0..length).map(|i| (i * step + 11) % 7 < 3).collect()
    }

    fn contains(haystack: &[bool], needle: &[bool]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn either_end_renegotiates_the_rate_and_data_crosses_at_the_new_one() {
        for initiator in [Role::Call, Role::Answer] {
            let mut link = Link::new(0.030, 45.0, 60.0);
            assert!(link.run_until(20.0, Link::both_connected), "never connected: {}", link.caller.phase());
            let before = link.caller.rates().unwrap();
            // The initiating end offers to receive no faster than 19 200.
            assert!(link.end(initiator).renegotiate(8));
            assert_eq!(link.end(initiator).status(), Status::Retraining);
            // The far end hears S, both are back at MP, and both come out.
            let answered = link.run_until(1.0, |l| l.caller.status() == Status::Retraining && l.answerer.status() == Status::Retraining);
            assert!(answered, "{initiator:?} began, and the other end never answered");
            assert!(link.run_until(6.0, Link::both_connected), "{initiator:?}: stuck at {} and {}", link.caller.phase(), link.answerer.phase());
            let (call_tx, call_rx) = link.caller.rates().unwrap();
            let (answer_tx, answer_rx) = link.answerer.rates().unwrap();
            assert_eq!((call_tx, call_rx), (answer_rx, answer_tx), "{initiator:?}");
            let slowed = match initiator {
                Role::Call => call_rx,
                Role::Answer => answer_rx,
            };
            assert_eq!(slowed, 8, "{initiator:?}: was {before:?}");
            // The other way is as fast as it was.
            let kept = match initiator {
                Role::Call => call_tx,
                Role::Answer => answer_tx,
            };
            assert_eq!(kept, if initiator == Role::Call { before.0 } else { before.1 }, "{initiator:?}");
            for m in [&link.caller, &link.answerer] {
                assert_eq!(m.renegotiations(), 1);
                assert_eq!(m.b1_errors(), 0, "{:?}", m.settings().role);
            }
            // And data goes both ways at the new rates.
            let (from_call, from_answer) = (pattern(4000, 37), pattern(4000, 13));
            link.at_call.clear();
            link.at_answer.clear();
            link.caller.send_bits(&from_call);
            link.answerer.send_bits(&from_answer);
            link.run_until(0.8, |_| false);
            assert!(contains(&link.at_answer, &from_call), "{initiator:?}: call to answer lost");
            assert!(contains(&link.at_call, &from_answer), "{initiator:?}: answer to call lost");
            // Again from the other end: the end that answered the first one
            // is listening for S as it was before it.
            let other = if initiator == Role::Call { Role::Answer } else { Role::Call };
            assert!(link.end(other).renegotiate(6));
            assert!(link.run_until(1.0, |l| l.caller.status() == Status::Retraining && l.answerer.status() == Status::Retraining), "{other:?} began a second, and it was not heard");
            assert!(link.run_until(6.0, Link::both_connected), "second: stuck at {} and {}", link.caller.phase(), link.answerer.phase());
            for m in [&link.caller, &link.answerer] {
                assert_eq!(m.renegotiations(), 2);
            }
            let received = |m: &Modem| m.rates().map(|r| r.1);
            assert_eq!(received(link.end(other)), Some(6), "{other:?}");
        }
    }

    #[test]
    fn a_renegotiation_survives_a_voip_round_trip() {
        let mut link = Link::new(0.570, 45.0, 100.0);
        assert!(link.run_until(30.0, Link::both_connected), "never connected: {}", link.caller.phase());
        assert!(link.answerer.renegotiate(10));
        assert!(link.run_until(1.0, |l| l.caller.status() == Status::Retraining), "the call end never heard S");
        assert!(link.run_until(10.0, Link::both_connected), "stuck at {} and {}", link.caller.phase(), link.answerer.phase());
        assert_eq!(link.answerer.rates().map(|r| r.1), Some(10));
    }

    #[test]
    fn a_slip_in_data_mode_costs_only_the_data_in_flight() {
        for (towards, dropped) in [(Role::Call, true), (Role::Answer, false), (Role::Answer, true)] {
            let mut link = Link::new(0.040, 45.0, 60.0);
            assert!(link.run_until(20.0, Link::both_connected), "never connected");
            link.run_until(0.5, |_| false);
            link.slip(towards, dropped);
            let found = |l: &Link| match towards {
                Role::Call => l.caller.found_again(),
                Role::Answer => l.answerer.found_again(),
            };
            assert!(link.run_until(2.0, |l| found(l) == 1), "{towards:?} dropped {dropped}: the frames were not found again");
            assert!(link.both_connected(), "{towards:?} dropped {dropped}: {} and {}", link.caller.phase(), link.answerer.phase());
            let (from_call, from_answer) = (pattern(4000, 37), pattern(4000, 13));
            link.at_call.clear();
            link.at_answer.clear();
            link.caller.send_bits(&from_call);
            link.answerer.send_bits(&from_answer);
            link.run_until(1.0, |_| false);
            assert!(contains(&link.at_answer, &from_call), "{towards:?} dropped {dropped}: call to answer lost after the slip");
            assert!(contains(&link.at_call, &from_answer), "{towards:?} dropped {dropped}: answer to call lost after the slip");
        }
    }

    #[test]
    fn an_e_that_never_arrives_is_found_from_the_data_after_it() {
        let mut link = Link::new(0.030, 45.0, 0.0);
        link.caller.listening.deaf_to_e = true;
        assert!(link.run_until(25.0, Link::both_connected), "stuck at {} and {}", link.caller.phase(), link.answerer.phase());
        assert_eq!(link.caller.found_again(), 1);
        assert_eq!(link.answerer.found_again(), 0);
        let (from_call, from_answer) = (pattern(4000, 37), pattern(4000, 13));
        link.at_call.clear();
        link.at_answer.clear();
        link.caller.send_bits(&from_call);
        link.answerer.send_bits(&from_answer);
        link.run_until(1.0, |_| false);
        assert!(contains(&link.at_answer, &from_call), "call to answer lost");
        assert!(contains(&link.at_call, &from_answer), "answer to call lost");
    }

    #[test]
    fn a_cleardown_from_either_end_ends_the_call_at_both() {
        for initiator in [Role::Call, Role::Answer] {
            let mut link = Link::new(0.030, 45.0, 0.0);
            assert!(link.run_until(20.0, Link::both_connected), "never connected");
            assert!(link.end(initiator).clear_down());
            let over = |l: &Link| l.caller.status() == Status::ClearedDown && l.answerer.status() == Status::ClearedDown;
            assert!(link.run_until(4.0, over), "{initiator:?}: at {:?} and {:?}", link.caller.status(), link.answerer.status());
            assert_eq!(link.caller.phase(), "V.34 cleared down");
        }
    }

    #[test]
    fn s_is_heard_in_data_and_its_turn_to_s_bar_is_found() {
        let unit = |p: Point| Complex::new(f64::from(p.0), f64::from(p.1)).scale(receiver::unit(Size::Four));
        // Scrambled four-point data -- data mode at 4800 -- is never S.
        let mut watch = SWatch::default();
        let mut seed = 99u32;
        for _ in 0..200_000 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let quarters = seed >> 16 & 3;
            let y = unit(signals::s(0)) * [Complex::ONE, Complex::I, -Complex::ONE, -Complex::I][quarters as usize];
            assert_eq!(watch.feed(y), Watched::Nothing);
        }
        // S turned any way at all, then S-bar.
        for turn in [Complex::ONE, Complex::I, -Complex::ONE, -Complex::I] {
            let mut watch = SWatch::default();
            let mut heard = None;
            for n in 0..signals::S_SYMBOLS {
                if watch.feed(unit(signals::s(n)) * turn) == Watched::S {
                    heard.get_or_insert(n);
                }
            }
            assert_eq!(heard, Some(S_HEARD + 1), "S turned {turn:?}");
            let turned = (0..signals::S_BAR_SYMBOLS).position(|n| watch.feed(unit(signals::s_bar(n)) * turn) == Watched::SBar);
            assert_eq!(turned, Some(1), "S-bar turned {turn:?}");
        }
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
