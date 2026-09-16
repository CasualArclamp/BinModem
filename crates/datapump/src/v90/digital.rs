//! The digital modem from phase 3 on (9.3.1, 9.4.1): what a V.90 server
//! does, one codeword at a time.
//!
//! Here so that the analogue modem has something to talk to that is not
//! itself, and so that every sequence the Recommendation has the digital
//! modem send is sent by something written from the text. It runs at the
//! network's own rate: one sample in and one level out every 125
//! microseconds, the level always a codeword from phase 3 on.
//!
//! ```text
//! digital  (silent) ........... Sd S'd TRN1d Jd ... Jd J'd DIL ... DIL Ri ... R'i TRN2d MP MP' Ed B1d data
//! analogue S S' PP TRN Ja ...                 S ... S S' (DIL)  S S' CPt ...      CP CP' E B1 data
//! ```
//!
//! And from data mode, a rate renegotiation (9.6), begun by either end:
//!
//! ```text
//! digital  data  Rd ... R'd TRN2d MP ... MP' Ed B1d data
//! analogue data  ... S S'   CP ...  CP' E  B1 data
//! ```

use std::collections::VecDeque;

use crate::v32::{Mode, Scrambler};
use crate::v34::data::{Decoder as UpstreamDecoder, Params};
use crate::v34::frame::Framing;
use crate::v34::info::{Info1aPcm, Info1c};
use crate::v34::mp::{Mp, Trellis};
use crate::v34::phase2::Role;
use crate::v34::training::{RetrainWatch, SWatch, Watched};
use crate::v34::qam::Band;
use crate::v34::receiver::{self, Heard, Receiver, Reference};
use crate::v34::signals::{self, Reader, Size};
use crate::v34::trellis::Code;

use super::INTERVALS;
use super::encoder::{Encoder, Mapping};
use super::sequences::{self, Cp, CpFinder, Descriptor, DescriptorFinder, JD_PRIME_BITS, Jd};
use super::ucode::{self, Law};

/// The network's rate.
pub const FS: f64 = 8000.0;

/// TRN1d: "a minimum of 2040T" (9.3.1.4).
const TRN1D_SYMBOLS: usize = 2400;

/// Ri before CPt can have been answered: "a minimum of 192T" (9.4.1.1).
const RI_SYMBOLS: usize = 192;

/// R-bar: "4 repetitions of the 6-symbol sequence" (8.6.4).
const R_BAR_FRAMES: usize = 4;

/// TRN2d: "a minimum of 2040T" (9.4.1.2), in whole frames.
const TRN2D_FRAMES: usize = 340;

/// B1d: "48 data frames" (8.6.1). Ed: "2 data frames" (8.6.2).
const B1D_FRAMES: usize = 48;
const ED_FRAMES: usize = 2;

/// Rd in a rate renegotiation: "384T" (9.6.1.1.1).
const RD_SYMBOLS: usize = 384;

/// A renegotiation's E: "within 5000 ms plus 2 round-trip delays after
/// transmitting the Rd-to-R-bar-d transition" (9.6.1).
const RENEGOTIATION_E: f64 = 5.0;

/// Whole MPs asking for nothing sent before a cleardown is over.
const CLEARDOWN_MPS: usize = 2;

/// Sd and S-bar-d, in frames (8.4.4).
const SD_FRAMES: usize = 64;
const SD_BAR_FRAMES: usize = 8;

/// How phases 3 and 4 are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// In data mode: downstream and upstream rates in bit/s.
    Connected { downstream: u32, upstream: u32 },
    /// One end or the other asked for a rate of nothing (9.7).
    ClearedDown,
    Failed(&'static str),
}

/// What phase 2 settled, as the digital modem needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub law: Law,
    pub uinfo: u8,
    /// The analogue modem's transmitter.
    pub upstream: Band,
    /// The analogue modem's MD, in 35 ms steps.
    pub far_md: u8,
    pub round_trip: f64,
    /// What this end's Jd says.
    pub jd: Jd,
    /// Whether both ends have the 1664-point constellation the upstream's
    /// top rates need.
    pub wide: bool,
}

impl Settings {
    /// From INFO1d as this end sent it and the analogue modem's V.90 INFO1a.
    pub fn new(law: Law, info1d: &Info1c, asked: &Info1aPcm, round_trip: f64, wide: bool) -> Self {
        let probed = info1d.probed[asked.upstream.index() as usize];
        Self {
            law,
            uinfo: asked.uinfo,
            upstream: Band::new(asked.upstream, probed.high_carrier),
            far_md: asked.md_length,
            round_trip,
            // Every rate, CP on four points, and the one look-ahead a digital
            // modem must have (5.4.5.5: "ld of 0 and 1 are mandatory").
            jd: Jd { rates: Jd::ALL_RATES, sixteen_in_training: false, sixteen_in_renegotiation: false, lookahead: 1 },
            wide,
        }
    }
}

/// What goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Out {
    Silence,
    Sd,
    SdBar,
    Trn1d,
    Jd,
    JdPrime,
    Dil,
    Ri,
    RiBar,
    Rd,
    RdBar,
    Trn2d,
    Mp,
    Ed,
    B1d,
    Data,
}

/// The levels the digital modem sends, one symbol at a time.
#[derive(Debug, Clone)]
struct Source {
    law: Law,
    uinfo: u8,
    out: Out,
    /// Symbols since Sd began: where in its data frame each one falls.
    symbol: u64,
    /// Symbols, or for signals made of data frames frames, of the current
    /// signal sent.
    count: usize,
    pending: Option<Out>,
    after_jd_prime: Out,
    scrambler: Scrambler,
    /// Jd's differential encoder.
    sign: bool,
    jd: Vec<bool>,
    bits: VecDeque<bool>,
    dil: Vec<(u8, bool)>,
    /// Where each DIL segment ends, in symbols of a pass.
    dil_ends: Vec<usize>,
    frame: VecDeque<f64>,
    encoder: Option<Encoder>,
    training: Option<Mapping>,
    data_mode: Option<Mapping>,
    mp: Option<Mp>,
    /// Whether MP' is what goes out from the next sequence, and whether the
    /// one going out is; and how many MP' have gone whole.
    mp_ack: bool,
    sending_acknowledged: bool,
    acknowledged: usize,
    /// Whole MPs of either kind sent since MP began.
    mps_sent: usize,
    /// Rd's codeword in each interval.
    r_codes: [u8; INTERVALS],
    data: VecDeque<bool>,
}

impl Source {
    fn new(law: Law, uinfo: u8, jd: Jd) -> Self {
        Self {
            law,
            uinfo,
            out: Out::Silence,
            symbol: 0,
            count: 0,
            pending: None,
            after_jd_prime: Out::Ri,
            scrambler: Scrambler::new(Mode::Call),
            sign: false,
            jd: jd.to_bits(),
            bits: VecDeque::new(),
            dil: Vec::new(),
            dil_ends: Vec::new(),
            frame: VecDeque::new(),
            encoder: None,
            training: None,
            data_mode: None,
            mp: None,
            mp_ack: false,
            sending_acknowledged: false,
            acknowledged: 0,
            mps_sent: 0,
            r_codes: [0; INTERVALS],
            data: VecDeque::new(),
        }
    }

    fn level(&self, ucode: u8, positive: bool) -> f64 {
        ucode::level(self.law, ucode) * if positive { 1.0 } else { -1.0 }
    }

    fn start(&mut self, out: Out) {
        self.out = out;
        self.count = 0;
        self.bits.clear();
        self.frame.clear();
        match out {
            // "The first symbol of Sd is defined to be transmitted in data
            // frame interval 0" (8.4.4).
            Out::Sd => self.symbol = 0,
            // "The scrambler is initialized to zero prior to the transmission
            // of TRN1d" (8.4.5).
            Out::Trn1d => self.scrambler.reset(),
            Out::JdPrime => self.bits.extend([false; JD_PRIME_BITS]),
            Out::Trn2d => {
                // "The scrambler, differential encoder and spectral shape
                // filter memory shall be initialized to zero prior to
                // transmitting TRN2d" (8.6.5), and the same for B1d (8.6.1).
                self.scrambler.reset();
                self.encoder = self.training.clone().map(|m| Encoder::new(m, self.law));
            }
            Out::B1d => {
                self.scrambler.reset();
                self.encoder = self.data_mode.clone().map(|m| Encoder::new(m, self.law));
            }
            _ => {}
        }
    }

    fn at_frame_boundary(&self) -> bool {
        self.symbol.is_multiple_of(INTERVALS as u64)
    }

    /// Change to `out` at the next place the current signal can end.
    fn change(&mut self, out: Out) {
        self.pending = Some(out);
    }

    fn next(&mut self) -> f64 {
        let level = self.next_level();
        if self.out != Out::Silence {
            self.symbol += 1;
        }
        level
    }

    fn next_level(&mut self) -> f64 {
        let w = 16 + self.uinfo;
        loop {
            match self.out {
                Out::Silence => {
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    return 0.0;
                }
                Out::Sd | Out::SdBar => {
                    let frames = if self.out == Out::Sd { SD_FRAMES } else { SD_BAR_FRAMES };
                    if self.count == frames * INTERVALS {
                        self.start(if self.out == Out::Sd { Out::SdBar } else { Out::Trn1d });
                        continue;
                    }
                    // {+W, +0, +W, -W, -0, -W} (8.4.4), turned over for
                    // S-bar-d.
                    let pattern = [(w, true), (0, true), (w, true), (w, false), (0, false), (w, false)];
                    let (u, positive) = pattern[self.count % INTERVALS];
                    self.count += 1;
                    return self.level(u, positive ^ (self.out == Out::SdBar));
                }
                Out::Trn1d => {
                    if self.count >= TRN1D_SYMBOLS && self.at_frame_boundary() {
                        // 8.4.2: "The differential encoder shall be
                        // initialized with the final symbol of the transmitted
                        // TRN1d" -- which `sign` already is.
                        self.start(Out::Jd);
                        continue;
                    }
                    self.count += 1;
                    self.sign = self.scrambler.scramble(true);
                    return self.level(self.uinfo, self.sign);
                }
                Out::Jd | Out::JdPrime => {
                    if self.bits.is_empty() {
                        if self.out == Out::JdPrime {
                            let next = self.after_jd_prime;
                            self.start(next);
                            continue;
                        }
                        if let Some(next) = self.pending.take() {
                            self.start(next);
                            continue;
                        }
                        self.bits.extend(self.jd.clone());
                    }
                    let bit = self.bits.pop_front().unwrap_or(false);
                    self.sign ^= self.scrambler.scramble(bit);
                    return self.level(self.uinfo, self.sign);
                }
                Out::Dil => {
                    if self.dil.is_empty() {
                        self.start(Out::Ri);
                        continue;
                    }
                    let at = self.count % self.dil.len();
                    // "The sequence shall be terminated on a DIL-segment
                    // boundary."
                    if (at == 0 || self.dil_ends.contains(&at))
                        && let Some(next) = self.pending.take()
                    {
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    let (u, positive) = self.dil[at];
                    return self.level(u, positive);
                }
                Out::Ri | Out::RiBar | Out::Rd | Out::RdBar => {
                    let bar = matches!(self.out, Out::RiBar | Out::RdBar);
                    if self.at_frame_boundary() {
                        if bar && self.count == R_BAR_FRAMES * INTERVALS {
                            self.start(Out::Trn2d);
                            continue;
                        }
                        if self.out == Out::Rd && self.count >= RD_SYMBOLS {
                            self.start(Out::RdBar);
                            continue;
                        }
                        if self.out == Out::Ri
                            && self.count >= RI_SYMBOLS
                            && let Some(next) = self.pending.take()
                        {
                            self.start(next);
                            continue;
                        }
                    }
                    // "+ + + - - -", and the other way round for R-bar. Ri is
                    // UINFO throughout; Rd is "the highest power PCM codeword
                    // from the data mode constellation of each data frame
                    // interval" (8.6.4).
                    let interval = (self.symbol % INTERVALS as u64) as usize;
                    let ucode = if matches!(self.out, Out::Ri | Out::RiBar) { self.uinfo } else { self.r_codes[interval] };
                    self.count += 1;
                    return self.level(ucode, (interval < 3) ^ bar);
                }
                Out::Trn2d | Out::Mp | Out::Ed | Out::B1d | Out::Data => {
                    if self.frame.is_empty() {
                        if self.frame_boundary_change() {
                            continue;
                        }
                        self.make_frame();
                    }
                    return self.frame.pop_front().unwrap_or(0.0);
                }
            }
        }
    }

    /// Where a signal made of data frames moves on, at a frame boundary.
    /// True if it did.
    fn frame_boundary_change(&mut self) -> bool {
        let next = match self.out {
            Out::Trn2d if self.count >= TRN2D_FRAMES => Some(Out::Mp),
            Out::Mp if self.bits.is_empty() => {
                if self.count > 0 {
                    self.mps_sent += 1;
                    if self.sending_acknowledged {
                        self.acknowledged += 1;
                    }
                }
                let next = self.pending.take();
                if next.is_none() {
                    // A whole MP, or MP' once the far end's CP has come.
                    let mp = self.mp.unwrap_or_default();
                    let mp = if self.mp_ack { mp.acknowledged() } else { mp };
                    let frame_bits = self.encoder.as_ref().map_or(1, Encoder::frame_bits);
                    self.sending_acknowledged = self.mp_ack;
                    self.bits.extend(sequences::mp_bits(&mp, frame_bits));
                }
                next
            }
            Out::Ed if self.count == ED_FRAMES => Some(Out::B1d),
            Out::B1d if self.count == B1D_FRAMES => Some(Out::Data),
            Out::Data => self.pending.take(),
            _ => None,
        };
        match next {
            Some(out) => {
                self.start(out);
                true
            }
            None => false,
        }
    }

    /// One data frame of the current signal.
    fn make_frame(&mut self) {
        let Some(encoder) = self.encoder.as_mut() else {
            self.frame.extend([0.0; INTERVALS]);
            return;
        };
        let d = encoder.frame_bits();
        let source: Vec<bool> = match self.out {
            Out::Trn2d | Out::B1d => vec![true; d],
            Out::Ed => vec![false; d],
            Out::Mp => (0..d).map(|_| self.bits.pop_front().unwrap_or(false)).collect(),
            _ => (0..d).map(|_| self.data.pop_front().unwrap_or(true)).collect(),
        };
        let scrambler = &mut self.scrambler;
        let mut given = source.into_iter();
        let frame = encoder.next_frame(|| scrambler.scramble(given.next().unwrap_or(true)));
        self.count += 1;
        let law = self.law;
        self.frame.extend(frame.amplitudes(law).iter().map(|&a| f64::from(a) / 32768.0));
    }
}

/// Where the digital modem has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    AwaitS,
    Training,
    ReadJa,
    SendJd,
    AwaitFirstReversal,
    AwaitSecondReversal,
    Phase4Cpt,
    Phase4Cp,
    Data,
    Finished,
}

/// The digital modem, phase 3 on.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    now: u64,
    stage: Stage,
    status: Status,
    deadline: Option<(u64, &'static str)>,
    source: Source,
    rx: Receiver,
    reader: Reader,
    in_trn: bool,
    trn_symbols: usize,
    ja: DescriptorFinder,
    cps: CpFinder,
    ones: usize,
    md_until: Option<u64>,
    md_waited: bool,
    descriptor: Option<Descriptor>,
    cpt: Option<Cp>,
    cp: Option<Cp>,
    far_e: bool,
    decoder: Option<UpstreamDecoder>,
    b1_left: usize,
    received: Vec<bool>,
    upstream_rate: u32,
    phase3_snr: Option<f64>,
    /// The analogue modem's tone A, which starts a retrain (9.5.1.2), and
    /// whether one is wanted.
    retrain_watch: RetrainWatch,
    wants_retrain: bool,
    /// The analogue modem's S and S-bar, which begin or answer a rate
    /// renegotiation (9.6.1.2); whether S-bar has been heard in this one.
    s_watch: SWatch,
    far_s_bar: bool,
    renegotiating: bool,
    clearing: bool,
    renegotiations: u32,
    /// The upstream no faster than this, as a multiple of 2400, from a
    /// renegotiation this end began.
    upstream_cap: Option<u8>,
}

impl Modem {
    /// Phase 3 from its start: the moment INFO1a has arrived.
    pub fn new(settings: Settings) -> Self {
        let mut rx = Receiver::new(settings.upstream, FS);
        rx.hunt();
        let mut modem = Self {
            settings,
            now: 0,
            stage: Stage::AwaitS,
            status: Status::Running,
            deadline: None,
            source: Source::new(settings.law, settings.uinfo, settings.jd),
            rx,
            reader: Reader::new(Mode::Answer),
            in_trn: true,
            trn_symbols: 0,
            ja: DescriptorFinder::default(),
            cps: CpFinder::default(),
            ones: 0,
            md_until: None,
            md_waited: false,
            descriptor: None,
            cpt: None,
            cp: None,
            far_e: false,
            decoder: None,
            b1_left: 0,
            received: Vec::new(),
            upstream_rate: 0,
            phase3_snr: None,
            retrain_watch: RetrainWatch::new(Role::Answer, FS),
            wants_retrain: false,
            s_watch: SWatch::default(),
            far_s_bar: false,
            renegotiating: false,
            clearing: false,
            renegotiations: 0,
            upstream_cap: None,
        };
        // 9.4.1: B1 "within 15 s plus 5 round-trip delays after receiving
        // INFO1a".
        modem.deadline = Some((modem.samples(15.0 + 5.0 * settings.round_trip), "no B1 from the analogue modem"));
        modem
    }

    fn samples(&self, seconds: f64) -> u64 {
        self.now + (seconds * FS).round() as u64
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::AwaitS | Stage::Training | Stage::ReadJa => "V.90 phase 3: training",
            Stage::SendJd => "V.90 phase 3: Jd",
            Stage::AwaitFirstReversal | Stage::AwaitSecondReversal => "V.90 phase 3: DIL",
            Stage::Phase4Cp if self.renegotiating => "V.90 rate renegotiation",
            Stage::Phase4Cpt | Stage::Phase4Cp => "V.90 phase 4",
            Stage::Data => "V.90 data",
            Stage::Finished => "V.90 finished",
        }
    }

    /// The DIL the analogue modem asked for.
    pub fn descriptor(&self) -> Option<&Descriptor> {
        self.descriptor.as_ref()
    }

    pub fn cp(&self) -> Option<&Cp> {
        self.cp.as_ref()
    }

    pub fn cpt(&self) -> Option<&Cp> {
        self.cpt.as_ref()
    }

    pub fn phase3_snr(&self) -> Option<f64> {
        self.phase3_snr
    }

    /// The analogue modem's last upstream symbol, once phase 3 has trained
    /// the receiver.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        self.rx.last_point().map(Into::into)
    }

    /// Points the upstream is being decided against: four or sixteen in
    /// training, and data mode's L once B1 has begun.
    pub fn upstream_points(&self) -> usize {
        match self.decoder.as_ref() {
            Some(decoder) => decoder.params().framing.l,
            None if self.rx.size() == Size::Sixteen => 16,
            None => 4,
        }
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    /// Whether V.90's phase 2 should be run again: read once, and cleared.
    pub fn take_retrain(&mut self) -> bool {
        std::mem::take(&mut self.wants_retrain)
    }

    /// Start a retrain (9.5.1.1).
    pub fn start_retrain(&mut self) {
        self.wants_retrain = true;
    }

    /// Rate renegotiations and cleardowns since the call began, from either
    /// end.
    pub fn renegotiations(&self) -> u32 {
        self.renegotiations
    }

    /// Start a rate renegotiation from data mode (9.6.1.1), asking the
    /// analogue modem to send no faster than `upstream`, a multiple of 2400.
    /// False, and nothing done, outside data mode.
    pub fn renegotiate(&mut self, upstream: u8) -> bool {
        if !self.in_data_mode() {
            return false;
        }
        self.upstream_cap = Some(upstream);
        self.begin_renegotiation(true);
        true
    }

    /// End the call from data mode (9.7): a renegotiation whose MP asks for
    /// nothing. False, and nothing done, outside data mode.
    pub fn clear_down(&mut self) -> bool {
        if !self.in_data_mode() {
            return false;
        }
        self.clearing = true;
        self.begin_renegotiation(true);
        true
    }

    fn in_data_mode(&self) -> bool {
        matches!(self.status, Status::Connected { .. }) && !self.renegotiating
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.source.data.extend(bits.iter().copied());
    }

    pub fn pending_bits(&self) -> usize {
        self.source.data.len()
    }

    fn fail(&mut self, why: &'static str) {
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
        self.source.start(Out::Silence);
        self.source.pending = None;
    }

    /// One network sample in, one out.
    pub fn step(&mut self, input: f64) -> f64 {
        self.now += 1;
        self.rx.feed(input);
        // 9.3.1, 9.4.1 and 9.6.1: tone A is the analogue modem retraining.
        if self.stage != Stage::Finished && self.retrain_watch.feed(input, FS) {
            self.wants_retrain = true;
        }
        while let Some(heard) = self.rx.heard() {
            if self.stage != Stage::Finished {
                self.heard(heard);
            }
        }
        if let Some(until) = self.md_until
            && self.now >= until
        {
            self.md_until = None;
            self.rx.hunt();
        }
        if let Some((at, _)) = self.deadline
            && self.now > at
            && self.status == Status::Running
        {
            // 9.4.1 and 9.6.1: a start-up or a renegotiation that goes
            // nowhere is a retrain.
            self.deadline = None;
            self.wants_retrain = true;
        }
        self.stage_step();
        self.source.next()
    }

    /// From data mode to Rd (9.6.1.1.1, 9.6.1.2.2), and phase 4 after it.
    ///
    /// The answering end starts on S rather than on S turning into S-bar,
    /// as V.34's does: sooner, over a line where every millisecond of a
    /// round trip is already on the far end's clock.
    fn begin_renegotiation(&mut self, initiating: bool) {
        let (Some(cp), Some(cpt)) = (self.cp.take(), self.cpt.as_ref()) else { return };
        let Some(training) = Mapping::for_renegotiation(cpt, &cp) else {
            self.fail("the renegotiation has no mapping to train on");
            return;
        };
        self.renegotiations += 1;
        self.renegotiating = true;
        self.status = Status::Running;
        self.source.r_codes = std::array::from_fn(|i| cp.points(i).last().copied().unwrap_or(0));
        self.source.training = Some(training);
        self.source.mp = Some(self.make_mp());
        self.source.mp_ack = false;
        self.source.sending_acknowledged = false;
        self.source.acknowledged = 0;
        self.source.mps_sent = 0;
        self.source.change(Out::Rd);
        self.far_e = false;
        self.far_s_bar = false;
        self.cps = CpFinder::default();
        self.ones = 0;
        if initiating {
            // The analogue modem's data is data until its S (9.6.1.2.1).
            self.s_watch = SWatch::default();
        } else {
            self.clamp();
        }
        let rd = (RD_SYMBOLS + (R_BAR_FRAMES + 1) * INTERVALS) as f64 / FS;
        let wait = RENEGOTIATION_E + 2.0 * self.settings.round_trip + rd;
        self.deadline = Some((self.samples(wait), "no E in the rate renegotiation"));
        self.stage = Stage::Phase4Cp;
    }

    /// The analogue modem's S: circuit 104 clamped, and CP to be read.
    fn clamp(&mut self) {
        self.decoder = None;
        self.b1_left = 0;
        self.rx.set_size(self.renegotiation_size());
    }

    fn heard_far_s_bar(&mut self) {
        self.far_s_bar = true;
        self.cps = CpFinder::default();
        self.ones = 0;
    }

    fn renegotiation_size(&self) -> Size {
        if self.settings.jd.sixteen_in_renegotiation { Size::Sixteen } else { Size::Four }
    }

    /// One end has asked for nothing: the call is over (9.7).
    fn cleared_down(&mut self) {
        self.status = Status::ClearedDown;
        self.stage = Stage::Finished;
        self.renegotiating = false;
        self.source.start(Out::Silence);
        self.source.pending = None;
    }

    fn stage_step(&mut self) {
        match self.stage {
            Stage::Phase4Cp if self.clearing => {
                if self.source.out == Out::Mp && self.source.mps_sent >= CLEARDOWN_MPS {
                    self.cleared_down();
                }
            }
            Stage::Phase4Cp => {
                // 9.4.1.4: an MP' sent, and CP' or E heard.
                let heard_back = self.cp.as_ref().is_some_and(|cp| cp.acknowledge) || self.far_e;
                if self.source.out == Out::Mp && self.source.acknowledged >= 1 && heard_back && self.source.pending.is_none() {
                    self.source.change(Out::Ed);
                }
            }
            Stage::Data => {
                if self.status == Status::Running
                    && !self.renegotiating
                    && self.source.out == Out::Data
                    && self.b1_left == 0
                    && self.decoder.is_some()
                {
                    let downstream = self.cp.as_ref().and_then(|cp| sequences::data_rate(cp.drn)).unwrap_or(0);
                    self.status = Status::Connected { downstream, upstream: self.upstream_rate };
                    self.deadline = None;
                }
                if self.source.out == Out::Mp && self.source.acknowledged >= 1 && self.source.pending.is_none() {
                    self.source.change(Out::Ed);
                }
            }
            _ => {}
        }
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::S => {
                if self.stage == Stage::SendJd {
                    // 9.3.1.5: "complete the current Jd sequence and then
                    // transmit J'd", and the DIL after it.
                    let dil = self.descriptor.as_ref().is_some_and(|d| !d.is_empty());
                    self.source.after_jd_prime = if dil { Out::Dil } else { Out::Ri };
                    self.source.change(Out::JdPrime);
                    self.stage = Stage::AwaitFirstReversal;
                }
            }
            Heard::Reversal { at } => self.reversal(at),
            Heard::Trained { snr_db } => {
                if self.stage == Stage::Training {
                    self.phase3_snr = Some(snr_db);
                    self.stage = Stage::ReadJa;
                    self.in_trn = true;
                    self.trn_symbols = 0;
                }
            }
            Heard::Untrained => self.fail("the analogue modem's training sequence did not train this end"),
            Heard::Symbol(symbol) => self.symbol(symbol),
        }
    }

    fn reversal(&mut self, at: u64) {
        match self.stage {
            Stage::AwaitS => {
                if self.settings.far_md > 0 && !self.md_waited {
                    // 9.3.1.1: wait out MD, then S and S-bar again.
                    self.md_waited = true;
                    self.md_until = Some(self.samples(0.035 * f64::from(self.settings.far_md)));
                    self.rx.idle();
                    return;
                }
                self.rx.train(Reference::PpThenTrn, Mode::Answer, at);
                self.stage = Stage::Training;
            }
            Stage::AwaitFirstReversal => {
                if self.source.after_jd_prime == Out::Dil {
                    // The S-bar that answers J'd. The one that ends the DIL is
                    // still to come (9.3.1.6).
                    self.stage = Stage::AwaitSecondReversal;
                    self.rx.hunt();
                } else {
                    self.begin_phase4(at);
                }
            }
            Stage::AwaitSecondReversal => {
                // 9.3.1.6: "complete sending the current segment of the DIL
                // and proceed to Phase 4".
                self.source.change(Out::Ri);
                self.begin_phase4(at);
            }
            _ => {}
        }
    }

    /// Phase 4: the analogue modem's CPt follows its S-bar straight away,
    /// and is read with the equaliser phase 3 left.
    fn begin_phase4(&mut self, s_bar: u64) {
        self.stage = Stage::Phase4Cpt;
        self.rx.resume(s_bar + 2 * signals::S_BAR_SYMBOLS as u64);
        self.rx.set_size(self.cp_size());
        self.cps = CpFinder::default();
        self.ones = 0;
    }

    fn cp_size(&self) -> Size {
        if self.settings.jd.sixteen_in_training { Size::Sixteen } else { Size::Four }
    }

    fn symbol(&mut self, symbol: receiver::Symbol) {
        match self.stage {
            Stage::ReadJa => {
                if self.in_trn {
                    let before = self.reader.clone();
                    let bits = self.reader.trn(symbol.decided, Size::Four);
                    self.trn_symbols += 1;
                    // The descrambler fills on TRN's first symbols.
                    if bits.iter().all(|b| *b) || self.trn_symbols < 24 {
                        return;
                    }
                    self.reader = before;
                    self.in_trn = false;
                }
                for bit in self.reader.differential(symbol.decided, Size::Four) {
                    if let Some(descriptor) = self.ja.feed(bit) {
                        self.heard_ja(descriptor);
                        return;
                    }
                }
            }
            Stage::Phase4Cpt | Stage::Phase4Cp | Stage::Data => {
                let watching = self.stage == Stage::Data || self.renegotiating;
                if watching && !self.far_s_bar {
                    match self.s_watch.feed(symbol.point) {
                        Watched::S if self.renegotiating => self.clamp(),
                        Watched::S => self.begin_renegotiation(false),
                        Watched::SBar => self.heard_far_s_bar(),
                        // S-bar missed: CP is surely under way by now.
                        Watched::Nothing if self.s_watch.heard && self.s_watch.since > signals::S_SYMBOLS + 32 => {
                            self.heard_far_s_bar();
                        }
                        Watched::Nothing => {}
                    }
                    if self.renegotiating && !self.far_s_bar && self.decoder.is_none() {
                        return;
                    }
                }
                if let Some(decoder) = self.decoder.as_mut() {
                    decoder.feed(symbol.point);
                    for bit in decoder.take_bits() {
                        if self.b1_left > 0 {
                            self.b1_left -= 1;
                        } else {
                            self.received.push(bit);
                        }
                    }
                    return;
                }
                let size = if self.renegotiating { self.renegotiation_size() } else { self.cp_size() };
                for bit in self.reader.differential(symbol.decided, size) {
                    self.ones = if bit { self.ones + 1 } else { 0 };
                    // "20-bit E": twenty ones, which nothing in CP runs to.
                    if self.stage == Stage::Phase4Cp && self.ones >= signals::E_BITS && self.cp.is_some() {
                        self.heard_e();
                        return;
                    }
                    if let Some(cp) = self.cps.feed(bit) {
                        self.heard_cp(cp);
                    }
                }
            }
            _ => {}
        }
    }

    fn heard_ja(&mut self, descriptor: Descriptor) {
        // 9.3.1.3: "may wait for up to 500 ms and shall then transmit signal
        // Sd". Not waiting.
        self.source.dil = descriptor.symbols().collect();
        let mut end = 0;
        self.source.dil_ends = descriptor
            .ucodes
            .iter()
            .map(|&u| {
                end += descriptor.segment_length(u);
                end
            })
            .collect();
        self.descriptor = Some(descriptor);
        self.source.change(Out::Sd);
        self.stage = Stage::SendJd;
        // The analogue modem goes quiet on hearing S-bar-d. Its S after Jd is
        // what is listened for now.
        self.rx.hunt();
    }

    fn heard_cp(&mut self, cp: Cp) {
        if !cp.data_mode {
            if self.stage == Stage::Phase4Cpt {
                // 9.4.1.2: "send signal R-bar-i for 24T followed by TRN2d".
                let Some(training) = Mapping::from_cp(&cp) else {
                    self.fail("the analogue modem's CPt is not one this end can send");
                    return;
                };
                self.source.training = Some(training);
                self.source.mp = Some(self.make_mp());
                self.source.change(Out::RiBar);
                self.cpt = Some(cp);
                self.stage = Stage::Phase4Cp;
            }
            return;
        }
        if cp.drn == 0 {
            self.cleared_down();
            return;
        }
        // A renegotiation's CP may ask for another rate, and B1d goes out at
        // whatever the last one asked for.
        let Some(data_mode) = Mapping::from_cp(&cp) else {
            self.fail("the analogue modem's CP is not one this end can send");
            return;
        };
        self.source.data_mode = Some(data_mode);
        // 9.4.1.3: "After receiving the analogue modem's CP sequence, the
        // digital modem shall complete sending the current MP sequence, and
        // then send MP' sequences."
        self.source.mp_ack = true;
        self.cp = Some(cp);
    }

    fn heard_e(&mut self) {
        self.far_e = true;
        let (Some(ours), Some(cp)) = (self.source.mp, self.cp.as_ref()) else { return };
        let rate = upstream_rate(cp, &ours);
        self.upstream_rate = u32::from(rate) * 2400;
        let Some(framing) = Framing::new(self.settings.upstream.rate, self.upstream_rate, false, ours.expanded_shaping) else {
            self.fail("no upstream rate both ends allow");
            return;
        };
        // 9.4.1.6: B1 next, then data.
        let params = Params { framing, code: Code::States16, nonlinear: ours.non_linear, precoding: [(0, 0); 3], mode: Mode::Answer };
        let decoder = UpstreamDecoder::new(params);
        self.rx.set_grid(decoder.grid_scale(), decoder.extent());
        self.b1_left = framing.n;
        self.decoder = Some(decoder);
        self.stage = Stage::Data;
        // Listening for the next renegotiation's S.
        self.renegotiating = false;
        self.s_watch = SWatch::default();
        self.far_s_bar = false;
        self.deadline = None;
    }

    /// This end's MP: what the analogue modem's transmitter is to do.
    fn make_mp(&self) -> Mp {
        let rate = self.settings.upstream.rate;
        let snr = 10f64.powf(self.phase3_snr.unwrap_or(20.0).min(60.0) / 10.0);
        let bits = (1.0 + snr / 10f64.powf(0.6)).log2();
        let most = crate::v34::probe::ceiling(rate);
        let most = if self.settings.wide { most } else { most.min(12) };
        let upstream = ((bits * self.settings.upstream.baud() / 2400.0).floor() as u8).clamp(2, most);
        let upstream = self.upstream_cap.map_or(upstream, |cap| upstream.min(cap.max(2)));
        Mp {
            call_to_answer: 0,
            // 9.7: "drn = 0 indicates cleardown".
            answer_to_call: if self.clearing { 0 } else { upstream },
            auxiliary: false,
            trellis: Trellis::States16,
            non_linear: false,
            expanded_shaping: false,
            acknowledge: false,
            rates: Mp::rates_up_to(14) & !1,
            asymmetric: false,
            precoding: None,
        }
    }
}

/// The upstream rate, as a multiple of 2400: "the maximum rate enabled in
/// both modems that is less than or equal to the maximum analogue to digital
/// modem data signalling rate specified in the MP sequence" (9.4.2.4).
///
/// CP's mask has 4800 in its bit 0; MP's, read as V.34 reads it, has 2400
/// there and 4800 in bit 1.
pub fn upstream_rate(cp: &Cp, mp: &Mp) -> u8 {
    let enabled = (u32::from(cp.upstream_rates) << 1) & u32::from(mp.rates);
    (2..=mp.answer_to_call.min(14)).rev().find(|r| enabled >> (r - 1) & 1 == 1).unwrap_or(0)
}
