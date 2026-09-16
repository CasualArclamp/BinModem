//! The analogue modem from phase 3 on (9.3.2, 9.4.2): V.34 going up, PCM
//! coming down.
//!
//! ```text
//! analogue S S' PP TRN Ja ...          (quiet)       S ... S S'  (quiet)   S S' CPt ... CP CP' E B1 data
//! digital                 Sd S'd TRN1d Jd ... Jd J'd DIL ... ... DIL Ri ... R'i TRN2d MP MP' Ed B1d data
//! ```
//!
//! Everything upstream is V.34's, at the symbol rate, carrier and
//! pre-emphasis phase 2 settled: S, S-bar and PP as phase 3 sends them, TRN
//! at four points, Ja in J's modulation (8.3.1), CP, SCR and E as MP goes
//! (8.5.2), and then V.34's data mode as the digital modem's MP asks. What
//! comes down is read by [`super::pcm`], and what it means is worked out
//! here: Jd and J'd from the signs, the route from the DIL, R from its sign
//! pattern, and TRN2d, MP, Ed, B1d and data from whole data frames.

use std::collections::VecDeque;

use dsp::Complex;

use crate::v32::{Mode, Scrambler};
use crate::v34::constellation::Point;
use crate::v34::data::{Encoder as UpstreamEncoder, Params};
use crate::v34::frame::Framing;
use crate::v34::info::{Info0d, Info1aPcm, Info1c};
use crate::v34::mp::{Finder, Found, Mp, Trellis};
use crate::v34::qam::{Band, Transmitter};
use crate::v34::receiver;
use crate::v34::signals::{self, Sender, Size};
use crate::v34::trellis::Code;

use super::INTERVALS;
use super::dil::{self, Analysis, Choice, Route};
use super::encoder::{Decoder, Frame, Mapping};
use super::pcm::{self, Heard, Slicer};
use super::sequences::{self, Cp, Descriptor, JD_BITS, JD_PRIME_BITS, Jd};
use super::ucode::{self, Law};

/// TRN in phase 3: "at least 512T" (9.3.2.3), and a far receiver trains
/// better on more, as V.34's own phase 3 has found.
const PHASE3_TRN: f64 = 1.0;

/// "70 +- 5 ms" of silence after INFO1a (9.3.2.1).
const SILENCE_BEFORE_S: f64 = 0.070;

/// Frames of R in a row before it is believed.
const R_HEARD: usize = 8;

/// B1d: "48 data frames" (8.6.1).
const B1D_FRAMES: usize = 48;

/// How phases 3 and 4 are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// In data mode: downstream and upstream rates in bit/s.
    Connected { downstream: u32, upstream: u32 },
    Failed(&'static str),
}

/// What phase 2 settled, as the analogue modem needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub law: Law,
    pub uinfo: u8,
    pub server: Info0d,
    /// This end's transmitter, as INFO1a chose it and INFO1d set it up.
    pub upstream: Band,
    pub pre_emphasis: u8,
    pub power_reduction: u8,
    pub round_trip: f64,
    /// Whether both ends have the 1664-point constellation the upstream's
    /// top rates need.
    pub wide: bool,
}

impl Settings {
    /// From the digital modem's INFO0d and INFO1d, and this end's INFO1a.
    pub fn new(server: &Info0d, info1d: &Info1c, asked: &Info1aPcm, round_trip: f64, ours_wide: bool) -> Self {
        let probed = info1d.probed[asked.upstream.index() as usize];
        Self {
            law: if server.a_law { Law::A } else { Law::Mu },
            uinfo: asked.uinfo,
            server: *server,
            upstream: Band::new(asked.upstream, probed.high_carrier),
            pre_emphasis: probed.pre_emphasis,
            power_reduction: info1d.min_power_reduction,
            round_trip,
            wide: ours_wide && server.v34.constellation_1664,
        }
    }
}

/// What goes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Up {
    Silence,
    S,
    SBar,
    Pp,
    Trn,
    Ja,
    Cp,
    E,
    Data,
}

fn grid(point: Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(receiver::unit(size))
}

/// The upstream, one symbol at a time.
#[derive(Debug, Clone)]
struct Source {
    sender: Sender,
    up: Up,
    count: usize,
    /// How long S runs: to a count, or until changed.
    s_length: Option<usize>,
    after_s_bar: Up,
    trn_length: usize,
    pending: Option<Up>,
    queue: VecDeque<bool>,
    ja: Vec<bool>,
    cp: Vec<bool>,
    cp_is_ack: bool,
    next_cp: Option<(Vec<bool>, bool)>,
    /// Whole CP sequences sent, and whole CP' sequences.
    cps: usize,
    acknowledged: usize,
    restarted: bool,
    size: Size,
    encoder: Option<UpstreamEncoder>,
    data: VecDeque<bool>,
    hold: usize,
    silent: usize,
}

impl Source {
    fn new(trn_length: usize) -> Self {
        Self {
            // The analogue modem scrambles with GPA (8.3).
            sender: Sender::new(Mode::Answer),
            up: Up::Silence,
            count: 0,
            s_length: Some(signals::S_SYMBOLS),
            after_s_bar: Up::Pp,
            trn_length,
            pending: None,
            queue: VecDeque::new(),
            ja: Vec::new(),
            cp: Vec::new(),
            cp_is_ack: false,
            next_cp: None,
            cps: 0,
            acknowledged: 0,
            restarted: false,
            size: Size::Four,
            encoder: None,
            data: VecDeque::new(),
            hold: 0,
            silent: 0,
        }
    }

    fn start(&mut self, up: Up) {
        self.up = up;
        self.count = 0;
        self.silent = 0;
        self.queue.clear();
        match up {
            Up::Trn => self.sender.restart(),
            // "The scrambler and differential encoder are initialized to zero
            // prior to the transmission of the first CPt sequence" (8.5.2).
            Up::Cp if !self.restarted => {
                self.restarted = true;
                self.sender.restart();
            }
            Up::E => self.queue.extend(std::iter::repeat_n(true, signals::E_BITS)),
            _ => {}
        }
    }

    fn change(&mut self, up: Up) {
        self.pending = Some(up);
    }

    fn differential(&mut self, size: Size) -> Complex {
        let bits: Vec<bool> = (0..size.bits()).map(|_| self.queue.pop_front().unwrap_or(true)).collect();
        self.count += 1;
        grid(self.sender.differential(&bits), size)
    }

    fn next(&mut self) -> Complex {
        loop {
            match self.up {
                Up::Silence => {
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
                Up::S => {
                    let done = match self.s_length {
                        Some(n) => self.count >= n,
                        // Until something else is asked for, and then at the
                        // end of a pair, so S-bar follows in step.
                        None => self.pending.is_some() && self.count.is_multiple_of(2),
                    };
                    if done {
                        let next = if self.s_length.is_some() { Up::SBar } else { self.pending.take().unwrap_or(Up::SBar) };
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s(self.count - 1), Size::Four);
                }
                Up::SBar => {
                    if self.count == signals::S_BAR_SYMBOLS {
                        let next = self.after_s_bar;
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s_bar(self.count - 1), Size::Four);
                }
                Up::Pp => {
                    if self.count == signals::PP_SYMBOLS {
                        self.start(Up::Trn);
                        continue;
                    }
                    self.count += 1;
                    return signals::pp(self.count - 1).into();
                }
                Up::Trn => {
                    if self.count >= self.trn_length {
                        self.start(Up::Ja);
                        continue;
                    }
                    self.count += 1;
                    return grid(self.sender.trn(Size::Four), Size::Four);
                }
                Up::Ja => {
                    // "Transmission of sequence Ja may be terminated without
                    // completing the final DIL descriptor" (8.3.1).
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    if self.queue.is_empty() {
                        self.queue.extend(self.ja.iter().copied());
                    }
                    return self.differential(Size::Four);
                }
                Up::Cp => {
                    if self.queue.is_empty() {
                        if self.count > 0 {
                            self.cps += 1;
                            if self.cp_is_ack {
                                self.acknowledged += 1;
                            }
                        }
                        if let Some(next) = self.pending.take() {
                            self.start(next);
                            continue;
                        }
                        if let Some((cp, ack)) = self.next_cp.take() {
                            self.cp = cp;
                            self.cp_is_ack = ack;
                        }
                        self.queue.extend(self.cp.iter().copied());
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::E => {
                    if self.queue.is_empty() {
                        let next = if self.encoder.is_some() { Up::Data } else { Up::Silence };
                        self.start(next);
                        continue;
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::Data => {
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Up::Silence);
                        continue;
                    };
                    // B1 is one data frame of scrambled ones (8.5.1, and
                    // 10.1.3.1/V.34).
                    let b1 = encoder.mapping_frames() < encoder.params().framing.p as u64;
                    let data = &mut self.data;
                    self.count += 1;
                    return encoder.next_symbol(&mut || if b1 { true } else { data.pop_front().unwrap_or(true) });
                }
            }
        }
    }
}

/// Reads Jd and J'd off the signs (8.4.2, 8.4.3).
#[derive(Debug, Clone)]
struct JdReader {
    descrambler: Scrambler,
    differential: bool,
    previous: bool,
    bits: VecDeque<bool>,
    /// The last Jd read whole, and the symbol after it.
    last: Option<(u64, Jd)>,
}

impl JdReader {
    fn new() -> Self {
        Self { descrambler: Scrambler::new(Mode::Call), differential: false, previous: false, bits: VecDeque::new(), last: None }
    }

    /// One symbol's sign. True when this symbol ended a J'd.
    fn feed(&mut self, index: u64, positive: bool) -> bool {
        let before = self.descrambler.clone();
        let mut bit = self.descrambler.descramble(if self.differential { positive ^ self.previous } else { positive });
        if !self.differential && !bit {
            // TRN1d descrambles to ones: the first zero is Jd, which is
            // differential from here -- and was for the symbol that showed
            // it.
            self.differential = true;
            self.descrambler = before;
            bit = self.descrambler.descramble(positive ^ self.previous);
        }
        self.previous = positive;
        self.bits.push_back(bit);
        if self.bits.len() > JD_BITS {
            self.bits.pop_front();
        }
        if self.bits.len() == JD_BITS
            && let Some(jd) = Jd::from_bits(self.bits.make_contiguous())
        {
            self.last = Some((index + 1, jd));
        }
        // "12 binary zeroes" where the next Jd's sync would start.
        match self.last {
            Some((end, _)) if index + 1 == end + JD_PRIME_BITS as u64 => {
                self.bits.iter().rev().take(JD_PRIME_BITS).all(|b| !*b)
            }
            _ => false,
        }
    }
}

/// Watches for R and its turn to R-bar (8.6.4).
#[derive(Debug, Clone, Default)]
struct RWatch {
    frame: [f64; INTERVALS],
    /// Frames of R in a row, and which way round.
    run: usize,
    turned: bool,
    heard: bool,
}

impl RWatch {
    /// One symbol. The index of the frame after R-bar ends -- TRN2d's first
    /// -- once the turn is seen.
    fn feed(&mut self, symbol: &pcm::Symbol, level: f64) -> Option<u64> {
        let i = symbol.interval();
        self.frame[i] = symbol.value;
        if i != INTERVALS - 1 {
            return None;
        }
        let near = self.frame.iter().all(|v| (0.5 * level..1.5 * level).contains(&v.abs()));
        let signs: Vec<bool> = self.frame.iter().map(|v| *v >= 0.0).collect();
        let r = [true, true, true, false, false, false];
        let r_bar = [false, false, false, true, true, true];
        // "Neither R nor R-bar are differentially encoded ... the receiver
        // [has] to be able to detect these sequences regardless of their
        // polarity."
        let pattern = if signs == r {
            Some(false)
        } else if signs == r_bar {
            Some(true)
        } else {
            None
        };
        match (near, pattern) {
            (true, Some(way)) if !self.heard || way == self.turned => {
                if self.run > 0 && way != self.turned {
                    self.run = 0;
                }
                self.turned = way;
                self.run += 1;
                if self.run >= R_HEARD {
                    self.heard = true;
                }
                None
            }
            (true, Some(_)) => {
                // R turned: the first of R-bar's four frames. TRN2d starts
                // after the other three.
                let frame_start = symbol.index + 1 - INTERVALS as u64;
                Some(frame_start + 4 * INTERVALS as u64)
            }
            _ => {
                if !self.heard {
                    self.run = 0;
                }
                None
            }
        }
    }
}

/// Signed levels of each interval's constellation, as the route delivers
/// them: (level, Ucode, positive).
type Levels = [Vec<(f64, u8, bool)>; INTERVALS];

fn levels_for(cp: &Cp, route: &Route) -> Levels {
    std::array::from_fn(|i| {
        cp.points(i)
            .into_iter()
            .flat_map(|u| {
                let level = route.levels[i][usize::from(u)];
                [(level, u, true), (-level, u, false)]
            })
            .collect()
    })
}

fn slicer_for(levels: &Levels) -> Slicer {
    Slicer::Levels(Box::new(std::array::from_fn(|i| levels[i].iter().map(|l| l.0).collect())))
}

/// Downstream data frames: TRN2d, MP, Ed, B1d and data.
#[derive(Debug, Clone)]
struct Frames {
    from: u64,
    decoder: Decoder,
    levels: Levels,
    frame: [(u8, bool); INTERVALS],
    descrambler: Scrambler,
    finder: Finder,
    mp: Option<Mp>,
    far_acknowledged: bool,
    zero_frames: usize,
    ed: bool,
    b1d_left: usize,
    data: bool,
}

/// Where the analogue modem has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    SendTraining,
    AwaitSd,
    Training,
    AwaitJd,
    AwaitJdPrime,
    Dil,
    Phase4,
    Data,
    Finished,
}

/// The analogue modem, phase 3 on.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    fs: f64,
    now: u64,
    stage: Stage,
    status: Status,
    deadline: Option<(u64, &'static str)>,
    tx: Transmitter,
    source: Source,
    rx: pcm::Receiver,
    descriptor: Descriptor,
    jd: JdReader,
    far_jd: Option<Jd>,
    dil_from: u64,
    dil: Vec<(u8, bool)>,
    analysis: Analysis,
    route: Option<Route>,
    choice: Option<Choice>,
    r_watch: RWatch,
    trn2d_from: Option<u64>,
    frames: Option<Frames>,
    received: Vec<bool>,
    upstream_rate: u32,
    downstream_rate: u32,
}

impl Modem {
    /// Phase 3 from its start: the moment INFO1a has gone.
    pub fn new(settings: Settings, fs: f64) -> Self {
        let trn_length = (PHASE3_TRN * settings.upstream.baud()) as usize;
        let mut source = Source::new(trn_length);
        let silence = (SILENCE_BEFORE_S * settings.upstream.baud()).round() as usize;
        source.hold = silence.saturating_sub(Transmitter::lookahead());
        source.after_s_bar = Up::Pp;
        source.change(Up::S);
        let descriptor = dil::design(settings.uinfo);
        source.ja = descriptor.to_bits();
        let mut rx = pcm::Receiver::new(settings.law, fs);
        rx.hunt(settings.uinfo);
        let mut modem = Self {
            settings,
            fs,
            now: 0,
            stage: Stage::SendTraining,
            status: Status::Running,
            deadline: None,
            tx: Transmitter::new(settings.upstream, settings.pre_emphasis, settings.power_reduction, fs),
            source,
            rx,
            dil: descriptor.symbols().collect(),
            descriptor,
            jd: JdReader::new(),
            far_jd: None,
            dil_from: 0,
            analysis: Analysis::new(),
            route: None,
            choice: None,
            r_watch: RWatch::default(),
            trn2d_from: None,
            frames: None,
            received: Vec::new(),
            upstream_rate: 0,
            downstream_rate: 0,
        };
        // 9.4.2: B1d "within 15 s plus 5 round-trip delays after sending
        // INFO1a".
        modem.deadline = Some((modem.samples(15.0 + 5.0 * settings.round_trip), "no B1d from the digital modem"));
        modem
    }

    fn samples(&self, seconds: f64) -> u64 {
        self.now + (seconds * self.fs).round() as u64
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::SendTraining | Stage::AwaitSd | Stage::Training => "V.90 phase 3: training",
            Stage::AwaitJd | Stage::AwaitJdPrime => "V.90 phase 3: Jd",
            Stage::Dil => "V.90 phase 3: DIL",
            Stage::Phase4 => "V.90 phase 4",
            Stage::Data => "V.90 data",
            Stage::Finished => "V.90 finished",
        }
    }

    /// The downstream receiver, for looking at.
    pub fn receiver(&self) -> &pcm::Receiver {
        &self.rx
    }

    /// The DIL this end asked for.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// The digital modem's Jd.
    pub fn far_jd(&self) -> Option<Jd> {
        self.far_jd
    }

    /// What the DIL showed of the route.
    pub fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// What this end asked for.
    pub fn choice(&self) -> Option<&Choice> {
        self.choice.as_ref()
    }

    /// The digital modem's MP.
    pub fn far_mp(&self) -> Option<Mp> {
        self.frames.as_ref().and_then(|f| f.mp)
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.source.data.extend(bits.iter().copied());
    }

    /// Data waiting to go, less what the next mapping frame takes at once.
    pub fn pending_bits(&self) -> usize {
        let frame = self.source.encoder.as_ref().map_or(0, |e| e.params().framing.b);
        self.source.data.len().saturating_sub(frame)
    }

    fn fail(&mut self, why: &'static str) {
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
        self.source.pending = None;
        self.source.start(Up::Silence);
        self.rx.idle();
    }

    /// One line sample in, one out.
    pub fn step(&mut self, line: f64) -> f64 {
        self.now += 1;
        self.rx.feed(line);
        while let Some(heard) = self.rx.heard() {
            if self.stage != Stage::Finished {
                self.heard(heard);
            }
        }
        if let Some((at, why)) = self.deadline
            && self.now > at
            && self.status == Status::Running
        {
            self.fail(why);
        }
        self.stage_step();
        let source = &mut self.source;
        self.tx.next_sample(|| source.next())
    }

    fn stage_step(&mut self) {
        match self.stage {
            Stage::SendTraining if self.source.up == Up::Ja => {
                self.stage = Stage::AwaitSd;
                // 9.3.2.4: S-bar-d within 1500 ms of the start of Ja.
                self.deadline = Some((self.samples(1.5 + self.settings.round_trip), "no Sd from the digital modem"));
            }
            Stage::Phase4 | Stage::Data => {
                // 9.4.2.4: a CP' sent, and MP' or Ed heard: E once the
                // current CP' is whole.
                let heard_back = self.frames.as_ref().is_some_and(|f| f.far_acknowledged || f.ed);
                if self.source.up == Up::Cp && self.source.acknowledged >= 1 && heard_back && self.source.pending.is_none() {
                    self.prepare_upstream();
                    self.source.change(Up::E);
                }
                let receiving = self.frames.as_ref().is_some_and(|f| f.data);
                if self.status == Status::Running && receiving && self.source.up == Up::Data {
                    self.status = Status::Connected { downstream: self.downstream_rate, upstream: self.upstream_rate };
                    self.deadline = None;
                }
            }
            _ => {}
        }
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Reversal { .. } => {
                if self.stage == Stage::AwaitSd {
                    // 9.3.2.4: "terminate Ja and transmit silence".
                    self.source.change(Up::Silence);
                    self.stage = Stage::Training;
                    self.deadline = Some((self.samples(4.5 + self.settings.round_trip), "no Jd from the digital modem"));
                }
            }
            Heard::Trained { .. } => {
                if self.stage == Stage::Training {
                    self.stage = Stage::AwaitJd;
                }
            }
            Heard::Untrained => self.fail("the digital modem's TRN1d did not train this end"),
            Heard::Symbol(symbol) => self.symbol(symbol),
        }
    }

    fn symbol(&mut self, symbol: pcm::Symbol) {
        match self.stage {
            Stage::AwaitJd | Stage::AwaitJdPrime => {
                let jd_prime = self.jd.feed(symbol.index, symbol.positive());
                if self.stage == Stage::AwaitJd
                    && let Some((_, jd)) = self.jd.last
                {
                    // 9.3.2.7: S, and listen for J'd.
                    self.far_jd = Some(jd);
                    self.source.size = if jd.sixteen_in_training { Size::Sixteen } else { Size::Four };
                    self.source.s_length = None;
                    self.source.change(Up::S);
                    self.stage = Stage::AwaitJdPrime;
                    self.deadline = None;
                }
                if jd_prime && self.stage == Stage::AwaitJdPrime {
                    // 9.3.2.8: S-bar for 16T, and the DIL straight after J'd.
                    self.source.after_s_bar = Up::Silence;
                    self.source.change(Up::SBar);
                    self.stage = Stage::Dil;
                    self.dil_from = symbol.index + 1;
                    let law = self.settings.law;
                    let levels: Vec<f64> = self
                        .dil
                        .iter()
                        .map(|&(u, positive)| ucode::level(law, u) * if positive { 1.0 } else { -1.0 })
                        .collect();
                    self.rx.expect(levels);
                }
            }
            Stage::Dil => {
                let at = (symbol.index - self.dil_from) as usize;
                if let Some(&(u, positive)) = self.dil.get(at) {
                    self.analysis.feed(u, positive, symbol.interval(), symbol.value);
                }
                if at + 1 == self.dil.len() {
                    self.finish_dil();
                }
            }
            Stage::Phase4 | Stage::Data => self.phase4_symbol(symbol),
            _ => {}
        }
    }

    /// A whole pass of the DIL is in: choose, and say so (9.3.2.10).
    fn finish_dil(&mut self) {
        let route = self.analysis.route();
        let law = self.settings.law;
        let limit = super::power_limit(&self.settings.server);
        let jd = self.far_jd.unwrap_or_default();
        let Some(mut choice) = dil::choose(&route, law, limit, |drn| jd.enables(drn)) else {
            self.fail("the route cannot carry V.90's slowest rate");
            return;
        };
        let upstream_rates = if self.settings.wide { 0x1fff } else { 0x07ff };
        for cp in [&mut choice.data, &mut choice.training] {
            cp.a_law = law == Law::A;
            cp.upstream_rates = upstream_rates;
            cp.lookahead = 0;
        }
        self.downstream_rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
        // "S for 128T followed by S-bar for 16T", and phase 4: CPt.
        self.source.s_length = Some(signals::S_SYMBOLS);
        self.source.after_s_bar = Up::Cp;
        self.source.cp = choice.training.to_bits();
        self.source.cp_is_ack = false;
        self.source.change(Up::S);
        self.route = Some(route);
        self.choice = Some(choice);
        // What comes down now is more DIL, and then R: nothing to learn from
        // until R is sure.
        self.rx.set_slicer(Slicer::Free);
        self.stage = Stage::Phase4;
    }

    fn phase4_symbol(&mut self, symbol: pcm::Symbol) {
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return };
        if self.trn2d_from.is_none() {
            let level = ucode::level(self.settings.law, self.settings.uinfo);
            let was_heard = self.r_watch.heard;
            if let Some(from) = self.r_watch.feed(&symbol, level) {
                // 9.4.2.2: the current CPt whole, then CP.
                self.trn2d_from = Some(from);
                self.source.next_cp = Some((choice.data.to_bits(), false));
                let Some(training) = Mapping::from_cp(&choice.training) else {
                    self.fail("this end's CPt is not a mapping");
                    return;
                };
                let levels = levels_for(&choice.training, route);
                self.frames = Some(Frames {
                    from,
                    decoder: Decoder::new(training),
                    levels,
                    frame: [(0, false); INTERVALS],
                    descrambler: Scrambler::new(Mode::Call),
                    finder: Finder::new(),
                    mp: None,
                    far_acknowledged: false,
                    zero_frames: 0,
                    ed: false,
                    b1d_left: 0,
                    data: false,
                });
            } else if self.r_watch.heard && !was_heard {
                self.rx.set_slicer(Slicer::Binary(level));
            }
            return;
        }
        let Some(frames) = self.frames.as_mut() else { return };
        if symbol.index + 1 == frames.from {
            // TRN2d's first symbol is next: decide against CPt's levels.
            self.rx.set_slicer(slicer_for(&frames.levels));
            return;
        }
        if symbol.index < frames.from {
            return;
        }
        let i = symbol.interval();
        let nearest = frames.levels[i]
            .iter()
            .min_by(|a, b| (a.0 - symbol.value).abs().total_cmp(&(b.0 - symbol.value).abs()))
            .map_or((0, false), |l| (l.1, l.2));
        frames.frame[i] = nearest;
        if i != INTERVALS - 1 {
            return;
        }
        let frame = Frame {
            ucodes: std::array::from_fn(|k| frames.frame[k].0),
            positive: std::array::from_fn(|k| frames.frame[k].1),
        };
        let bits: Vec<bool> = frames.decoder.frame(frame).into_iter().map(|b| frames.descrambler.descramble(b)).collect();
        if frames.data {
            self.received.extend(bits);
            return;
        }
        if frames.ed {
            // B1d: 48 frames of scrambled ones.
            frames.b1d_left -= 1;
            if frames.b1d_left == 0 {
                frames.data = true;
                self.stage = Stage::Data;
            }
            return;
        }
        let zeros = bits.iter().all(|b| !*b);
        frames.zero_frames = if zeros && frames.mp.is_some() { frames.zero_frames + 1 } else { 0 };
        if frames.zero_frames == 2 {
            // Ed: B1d next, at data mode's constellation, with the coding
            // started afresh (8.6.1).
            frames.ed = true;
            frames.b1d_left = B1D_FRAMES;
            let Some(data) = Mapping::from_cp(&choice.data) else { return };
            frames.decoder = Decoder::new(data);
            frames.levels = levels_for(&choice.data, route);
            let slicer = slicer_for(&frames.levels);
            self.rx.set_slicer(slicer);
            return;
        }
        for bit in bits {
            if let Some(Found::Mp(mp)) = frames.finder.feed(bit) {
                if mp.acknowledge {
                    frames.far_acknowledged = true;
                }
                if frames.mp.is_none() {
                    // 9.4.2.3: "complete sending the current CP sequence,
                    // and then send CP' sequences".
                    self.source.next_cp = Some((choice.data.acknowledged().to_bits(), true));
                }
                frames.mp = Some(mp);
            }
        }
    }

    /// Data mode's upstream, as the digital modem's MP asks for it.
    fn prepare_upstream(&mut self) {
        let (Some(mp), Some(choice)) = (self.far_mp(), self.choice.as_ref()) else { return };
        let rate = super::digital::upstream_rate(&choice.data, &mp);
        self.upstream_rate = u32::from(rate) * 2400;
        let Some(framing) = Framing::new(self.settings.upstream.rate, self.upstream_rate, false, mp.expanded_shaping) else {
            self.fail("no upstream rate both ends allow");
            return;
        };
        let params = Params {
            framing,
            code: match mp.trellis {
                Trellis::States16 => Code::States16,
                Trellis::States32 => Code::States32,
                Trellis::States64 => Code::States64,
            },
            nonlinear: mp.non_linear,
            precoding: mp.precoding.unwrap_or([(0, 0); 3]),
            mode: Mode::Answer,
        };
        self.source.encoder = Some(UpstreamEncoder::new(params));
    }
}
