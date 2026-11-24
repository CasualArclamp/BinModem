//! The x2 server from the end of phase 2 on: at the network's 8000 Hz, one
//! sample in and one octet's level out.
//!
//! ```text
//! server  silence ................ lead pre TRN Jwords ones align sync ... record ... start data
//! client  S S' PP TRN J ... J (quiet)              S S' TRN(4,16) MP ... MP' E B1 data
//! ```
//!
//! The client's phase 3 is V.34's S, S-bar, PP, TRN on four points and J, at
//! the symbol rate and carrier of its marker, scrambled with GPA, the
//! answering modem's polynomial (x2-facts, "Phase 3 script"). The server is
//! silent until the client's J and then runs its script ([`Script`]). The
//! client's phase 4 is S, S-bar, TRN at four and then sixteen points, and its
//! MP on sixteen points until our record is in, then MP' and E (twenty ones)
//! and V.34's B1; its upstream is V.34 data at that symbol rate with
//! expanded shaping, 64 states and non-linear encoding ("Upstream").
//!
//! In data mode the client may change rate: S, S-bar, a short TRN and an MP
//! on four points asking for the index it measured; the server answers it
//! ([`Script::rate_change`]), and MP', E and B1 follow on four points. A
//! client that sends tone A in data mode has asked for a retrain, which x2
//! cannot serve: the call ends ("Unsettled", the live call of 2026-11-21).

use std::collections::VecDeque;

use super::bank::Bank;
use super::record::{self, Mp};
use super::script::Script;
use crate::v32::Mode;
use crate::v34::data::{Decoder as UpstreamDecoder, Params};
use crate::v34::frame::Framing;
use crate::v34::info::{SymbolRate, X2Marker};
use crate::v34::phase2::Role;
use crate::v34::qam::Band;
use crate::v34::receiver::{self, Heard, Receiver, Reference};
use crate::v34::signals::{self, Reader, Size};
use crate::v34::training::{RetrainWatch, SWatch, Watched};
use crate::v34::trellis::Code;
use crate::v90::ucode::{self, Law};

/// The network's rate.
pub const FS: f64 = 8000.0;

/// The start-up's time limit, from the marker: the Courier's data begins some
/// 4 s after its J, which comes some 2 s after the marker.
const STARTUP: f64 = 20.0;
/// A rate change's, from the client's S (libv90modem's 6 s).
const RATE_CHANGE: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// In data mode: the payload rates, downstream and upstream, in bit/s.
    Connected { downstream: u32, upstream: u32 },
    Failed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The client's S and its turn to S-bar.
    AwaitS,
    Training,
    /// TRN, then J.
    ReadJ,
    /// Quiet, then its phase 4's S.
    AwaitPhase4,
    /// MP, MP' and E.
    Phase4,
    Data,
    Finished,
}

/// What the server knows of the call from phase 2.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub marker: X2Marker,
    pub round_trip: f64,
}

impl Settings {
    fn band(&self) -> Band {
        Band::new(self.marker.symbol_rate().unwrap_or(SymbolRate::S3200), self.marker.high_carrier)
    }
}

/// The x2 server, phase 3 on.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    now: u64,
    stage: Stage,
    status: Status,
    deadline: Option<u64>,
    script: Script,
    rx: Receiver,
    reader: Reader,
    size: Size,
    trn_symbols: usize,
    in_trn: bool,
    bits16: VecDeque<bool>,
    mps: record::Reader,
    mp: Option<Mp>,
    ones: usize,
    decoder: Option<UpstreamDecoder>,
    b1_left: usize,
    received: Vec<bool>,
    to_send: VecDeque<bool>,
    upstream_rate: u32,
    rate_changes: u32,
    changing: bool,
    /// Whether this start-up's or rate change's MP has been answered.
    answered: bool,
    s_watch: SWatch,
    retrain_watch: RetrainWatch,
    trace: bool,
    watch: UpstreamWatch,
}

/// For the trace: what the upstream's bits look like, a second at a time.
#[derive(Debug, Clone, Default)]
struct UpstreamWatch {
    shift: u8,
    bits: u32,
    ones: u32,
    flags: u32,
    since: u64,
}

impl UpstreamWatch {
    fn bit(&mut self, bit: bool, now: u64) {
        self.shift = (self.shift >> 1) | if bit { 0x80 } else { 0 };
        self.bits += 1;
        self.ones += u32::from(bit);
        self.flags += u32::from(self.shift == 0x7e);
        if now >= self.since + FS as u64 {
            eprintln!(
                "[x2 {:8.3}] upstream: {} bits, {} ones, {} flags",
                now as f64 / FS, self.bits, self.ones, self.flags
            );
            *self = Self { since: now, shift: self.shift, ..Self::default() };
        }
    }
}

impl Modem {
    pub fn new(settings: Settings) -> Self {
        let mut rx = Receiver::new(settings.band(), FS);
        rx.hunt();
        Self {
            settings,
            now: 0,
            stage: Stage::AwaitS,
            status: Status::Running,
            deadline: Some((STARTUP * FS) as u64),
            script: Script::new(),
            rx,
            // The client scrambles with GPA, the answering modem's.
            reader: Reader::new(Mode::Answer),
            size: Size::Four,
            trn_symbols: 0,
            in_trn: true,
            bits16: VecDeque::with_capacity(32),
            mps: record::Reader::new(),
            mp: None,
            ones: 0,
            decoder: None,
            b1_left: 0,
            received: Vec::new(),
            to_send: VecDeque::new(),
            upstream_rate: 0,
            rate_changes: 0,
            changing: false,
            answered: false,
            s_watch: SWatch::default(),
            retrain_watch: RetrainWatch::new(Role::Answer, FS),
            trace: std::env::var_os("X2_TRACE").is_some(),
            watch: UpstreamWatch::default(),
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::AwaitS | Stage::Training | Stage::ReadJ => "x2 phase 3: training",
            Stage::AwaitPhase4 => "x2 phase 3: the server's training",
            Stage::Phase4 if self.changing => "x2 rate change",
            Stage::Phase4 => "x2 phase 4",
            Stage::Data => "x2 data",
            Stage::Finished => "x2 finished",
        }
    }

    /// The client's last MP.
    pub fn mp(&self) -> Option<Mp> {
        self.mp
    }

    pub fn rate_changes(&self) -> u32 {
        self.rate_changes
    }

    pub fn last_point(&self) -> Option<(f64, f64)> {
        self.rx.last_point().map(Into::into)
    }

    /// The largest coordinate the upstream's points reach, as
    /// [`Self::last_point`] gives them.
    pub fn upstream_peak(&self) -> f64 {
        match self.decoder.as_ref() {
            Some(decoder) => decoder.peak(),
            None if self.size == Size::Sixteen => 3.0 / 10f64.sqrt(),
            None => std::f64::consts::FRAC_1_SQRT_2,
        }
    }

    /// Points the upstream is read against.
    pub fn upstream_points(&self) -> usize {
        match self.decoder.as_ref() {
            Some(decoder) => decoder.params().framing.l,
            None if self.size == Size::Sixteen => 16,
            None => 4,
        }
    }

    pub fn snr_db(&self) -> f64 {
        self.rx.snr_db()
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.to_send.extend(bits.iter().copied());
    }

    pub fn pending_bits(&self) -> usize {
        self.to_send.len()
    }

    /// One network sample in, one level out (a codeword's value over 32768).
    pub fn step(&mut self, input: f64) -> f64 {
        self.now += 1;
        self.rx.feed(input);
        if self.stage == Stage::Data && self.retrain_watch.feed(input, FS) {
            self.fail("the client asked for a retrain, which x2 cannot serve");
        }
        while let Some(heard) = self.rx.heard() {
            if self.stage != Stage::Finished {
                self.heard(heard);
            }
        }
        if self.deadline.is_some_and(|at| self.now > at) && self.stage != Stage::Finished {
            self.fail(if self.changing { "the client's rate change did not finish" } else { "the x2 start-up did not finish" });
        }
        if self.stage == Stage::Data
            && self.status == Status::Running
            && self.decoder.is_some()
            && self.b1_left == 0
            && self.script.in_data()
        {
            self.status = Status::Connected { downstream: self.script.bank().rate(), upstream: self.upstream_rate };
            self.deadline = None;
        }
        let to_send = &mut self.to_send;
        // Scrambled ones when nothing is waiting: the line's idle.
        let octet = self.script.next(&mut || to_send.pop_front().unwrap_or(true));
        let (u, negative) = ucode::from_octet(Law::Mu, octet);
        f64::from(ucode::amplitude(Law::Mu, u, negative)) / 32768.0
    }

    fn fail(&mut self, why: &'static str) {
        if self.trace {
            eprintln!("[x2 {:8.3}] failed: {why}", self.now as f64 / FS);
        }
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Reversal { at } => match self.stage {
                Stage::AwaitS => {
                    self.rx.train(Reference::PpThenTrn, Mode::Answer, at);
                    self.stage = Stage::Training;
                }
                Stage::AwaitPhase4 => {
                    // Phase 4's TRN follows its S-bar, and the equaliser phase
                    // 3 left reads it.
                    self.rx.resume(at + 2 * signals::S_BAR_SYMBOLS as u64);
                    self.begin_records(Size::Sixteen);
                    self.stage = Stage::Phase4;
                }
                _ => {}
            },
            Heard::Trained { snr_db } if self.stage == Stage::Training => {
                if self.trace {
                    eprintln!("[x2 {:8.3}] trained, {snr_db:.1} dB", self.now as f64 / FS);
                }
                self.stage = Stage::ReadJ;
            }
            Heard::Untrained => self.fail("the client's training sequence did not train this end"),
            Heard::Symbol(symbol) => self.symbol(symbol),
            _ => {}
        }
    }

    /// Reading for records from here, at `size`.
    fn begin_records(&mut self, size: Size) {
        self.size = size;
        self.rx.set_size(size);
        self.mps = record::Reader::new();
        self.ones = 0;
    }

    fn symbol(&mut self, symbol: receiver::Symbol) {
        match self.stage {
            Stage::ReadJ => {
                if self.in_trn {
                    let before = self.reader.clone();
                    let bits = self.reader.trn(symbol.decided, Size::Four);
                    self.trn_symbols += 1;
                    if bits.iter().all(|b| *b) || self.trn_symbols < 24 {
                        return;
                    }
                    self.reader = before;
                    self.in_trn = false;
                }
                for bit in self.reader.differential(symbol.decided, Size::Four) {
                    if self.bits16.len() == 32 {
                        self.bits16.pop_front();
                    }
                    self.bits16.push_back(bit);
                    if self.bits16.len() == 32 {
                        let (older, newer): (Vec<bool>, Vec<bool>) =
                            (self.bits16.range(..16).copied().collect(), self.bits16.range(16..).copied().collect());
                        if older == newer && (newer == signals::J_FOUR || newer == signals::J_SIXTEEN) {
                            if self.trace {
                                eprintln!("[x2 {:8.3}] the client's J", self.now as f64 / FS);
                            }
                            self.script.start();
                            self.stage = Stage::AwaitPhase4;
                            self.rx.hunt();
                            return;
                        }
                    }
                }
            }
            Stage::Phase4 | Stage::Data => {
                if self.stage == Stage::Data && !self.changing {
                    match self.s_watch.feed(symbol.point) {
                        Watched::S => {
                            // A rate change: the data stops, and the MP comes
                            // on four points after S-bar and a short TRN.
                            if self.trace {
                                eprintln!("[x2 {:8.3}] the client's S: a rate change", self.now as f64 / FS);
                            }
                            self.changing = true;
                            self.decoder = None;
                            self.b1_left = 0;
                            self.begin_records(Size::Four);
                            self.stage = Stage::Phase4;
                            self.status = Status::Running;
                            self.deadline = Some(self.now + (RATE_CHANGE * FS) as u64);
                            return;
                        }
                        Watched::SBar | Watched::Nothing => {}
                    }
                }
                if let Some(decoder) = self.decoder.as_mut() {
                    decoder.feed(symbol.point);
                    for bit in decoder.take_bits() {
                        if self.b1_left > 0 {
                            self.b1_left -= 1;
                        } else {
                            self.received.push(bit);
                            if self.trace {
                                self.watch.bit(bit, self.now);
                            }
                        }
                    }
                    return;
                }
                for bit in self.reader.differential(symbol.decided, self.size) {
                    self.ones = if bit { self.ones + 1 } else { 0 };
                    // E: twenty ones, where the next MP's seventeen would be.
                    if self.ones >= signals::E_BITS && self.answered {
                        self.heard_e();
                        return;
                    }
                    if let Some(mp) = self.mps.push(bit) {
                        self.heard_mp(mp);
                    }
                }
            }
            _ => {}
        }
    }

    fn heard_mp(&mut self, mp: Mp) {
        if self.trace {
            eprintln!(
                "[x2 {:8.3}] MP {:04x} {:04x} {:04x} {:04x}: index {}, pad {}, ack {}",
                self.now as f64 / FS, mp.0[0], mp.0[1], mp.0[2], mp.0[3], mp.rate(), mp.pad(), mp.ack()
            );
        }
        self.mp = Some(mp);
        // The first MP of a start-up or a rate change is answered; the rest
        // are its repeats and its MP'.
        if self.answered || mp.ack() {
            return;
        }
        if mp.md() != 5 {
            self.fail("the client's MP asks for a number of signs x2 does not have");
            return;
        }
        let Some(bank) = Bank::get(mp.rate(), mp.pad()) else {
            self.fail("the client's MP asks for a rate or pad there is no bank for");
            return;
        };
        self.answered = true;
        if self.changing {
            self.script.rate_change(bank, mp.pad());
        } else {
            self.script.mp(bank);
        }
    }

    fn heard_e(&mut self) {
        let Some(mp) = self.mp else { return };
        let Some(rate) = mp.upstream_rate() else {
            self.fail("the client's MP allows no upstream rate");
            return;
        };
        let band = self.settings.band();
        let Some(framing) = Framing::new(band.rate, rate, false, true) else {
            self.fail("the client's upstream rate has no framing at its symbol rate");
            return;
        };
        if self.trace {
            eprintln!("[x2 {:8.3}] E: upstream {rate}", self.now as f64 / FS);
        }
        let params = Params { framing, code: Code::States64, nonlinear: true, precoding: [(0, 0); 3], mode: Mode::Answer };
        let decoder = UpstreamDecoder::new(params);
        self.rx.set_bent_grid(decoder.grid_scale(), decoder.extent(), decoder.warp());
        self.b1_left = framing.n;
        self.decoder = Some(decoder);
        self.upstream_rate = rate;
        self.script.e();
        if self.changing {
            self.rate_changes += 1;
        }
        self.changing = false;
        self.answered = false;
        self.s_watch = SWatch::default();
        self.stage = Stage::Data;
    }
}
