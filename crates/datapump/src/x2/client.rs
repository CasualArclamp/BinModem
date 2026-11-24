//! A simulated x2 client, from the end of phase 2: what the tests call the
//! server with.
//!
//! Written from the same facts as the server (libv90modem's
//! `docs/history/x2-facts.md`), so it shows that the pieces fit together and
//! not that they are x2: only a real client, or the original firmware in an
//! emulator, can show that. It runs at the network's rate on a digital line,
//! so the octets it decodes are exactly the ones sent, and it finds the
//! server's frames by counting from the preamble rather than from the
//! alignment pattern.

use std::collections::VecDeque;

use dsp::Complex;

use super::bank::{Bank, FRAME};
use super::map::Mapper;
use super::record;
use crate::v32::{Mode, Scrambler};
use crate::v34::constellation::Point;
use crate::v34::data::{Encoder, Params};
use crate::v34::frame::Framing;
use crate::v34::dpsk;
use crate::v34::info::{SymbolRate, X2Marker};
use crate::v34::phase2::{self, Role};
use crate::v34::qam::{Band, Transmitter};
use crate::v34::receiver;
use crate::v34::signals::{self, Sender, Size};
use crate::v34::trellis::Code;
use crate::v90::ucode::{self, Law};

pub const FS: f64 = 8000.0;

/// The client's phase 3 TRN, in symbols; a Courier's is some 6400, which the
/// server's receiver does not need.
const PHASE3_TRN: usize = 2048;
/// Its phase 4 TRN at four points, then at sixteen.
const PHASE4_TRN4: usize = 256;
const PHASE4_TRN16: usize = 512;
/// A rate change's TRN, on four points (x2-facts, "Rate change").
const CHANGE_TRN: usize = 512;
/// How long the client sends J over the server's preamble: about 150 ms.
const J_OVER_PREAMBLE: usize = 1200;
/// Upstream data rates the client offers: to 28 800.
const UPSTREAM_MASK: u16 = 0x0ffe;

/// The server's segment lengths, in octets, from the preamble's first.
const PRE: usize = 363;
const TRN: usize = 20004;
const JW: usize = 1200;
const ONES: usize = 6000;
const ALIGN: usize = 1152;
/// The start segment's frames in the data levels: 680 at the start-up.
const START: usize = 680;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Up {
    Silence,
    S,
    SBar,
    Pp,
    Trn(usize, Size),
    J,
    Records(Size),
    E,
    Data,
}

/// What goes upstream, a symbol at a time.
#[derive(Debug, Clone)]
struct Upstream {
    sender: Sender,
    up: Up,
    count: usize,
    queue: VecDeque<bool>,
    after: VecDeque<Up>,
    size: Size,
    encoder: Option<Encoder>,
    data: VecDeque<bool>,
    index: u8,
    acked: bool,
    mp_sent_acked: bool,
}

impl Upstream {
    fn start(&mut self, up: Up) {
        let was_trn = matches!(self.up, Up::Trn(..));
        self.up = up;
        self.count = 0;
        self.queue.clear();
        match up {
            Up::Trn(..) if !was_trn => self.sender.restart(),
            Up::Records(size) => {
                self.size = size;
                self.mp_sent_acked = false;
            }
            Up::E => self.queue.extend([true; signals::E_BITS]),
            Up::Data => {
                let framing = Framing::new(SymbolRate::S3200, upstream_rate(), false, true).unwrap();
                let params = Params { framing, code: Code::States64, nonlinear: true, precoding: [(0, 0); 3], mode: Mode::Answer };
                self.encoder = Some(Encoder::new(params));
            }
            _ => {}
        }
    }

    fn then(&mut self, ups: &[Up]) {
        self.after = ups.iter().copied().collect();
        let first = self.after.pop_front().unwrap_or(Up::Silence);
        self.start(first);
    }

    fn next_up(&mut self) {
        let next = self.after.pop_front().unwrap_or(Up::Silence);
        self.start(next);
    }

    fn differential(&mut self, size: Size) -> Complex {
        let bits: Vec<bool> = (0..size.bits()).map(|_| self.queue.pop_front().unwrap_or(true)).collect();
        grid(self.sender.differential(&bits), size)
    }

    fn mp(&self) -> Vec<bool> {
        let w0 = (u16::from(self.index) << 2) | if self.acked { 0x8000 } else { 0 };
        let mut bits = record::build(&[w0, UPSTREAM_MASK, 0x0000, 0x0500]);
        bits.extend([false, false]);
        bits
    }

    fn symbol(&mut self) -> Complex {
        loop {
            match self.up {
                Up::Silence => return Complex::ZERO,
                Up::S => {
                    if self.count == signals::S_SYMBOLS {
                        self.next_up();
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s(self.count - 1), Size::Four);
                }
                Up::SBar => {
                    if self.count == signals::S_BAR_SYMBOLS {
                        self.next_up();
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s_bar(self.count - 1), Size::Four);
                }
                Up::Pp => {
                    if self.count == signals::PP_SYMBOLS {
                        self.next_up();
                        continue;
                    }
                    self.count += 1;
                    return signals::pp(self.count - 1).into();
                }
                Up::Trn(n, size) => {
                    if self.count == n {
                        self.next_up();
                        continue;
                    }
                    self.count += 1;
                    return grid(self.sender.trn(size), size);
                }
                Up::J => {
                    if self.queue.is_empty() {
                        self.queue.extend(signals::J_FOUR);
                    }
                    return self.differential(Size::Four);
                }
                Up::Records(size) => {
                    if self.queue.is_empty() {
                        if self.mp_sent_acked {
                            self.start(Up::E);
                            continue;
                        }
                        // MP until the server's record is in, then MP' once.
                        self.mp_sent_acked = self.acked;
                        let mp = self.mp();
                        self.queue.extend(mp);
                    }
                    return self.differential(size);
                }
                Up::E => {
                    if self.queue.is_empty() {
                        self.start(Up::Data);
                        continue;
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::Data => {
                    let encoder = self.encoder.as_mut().unwrap();
                    // B1: one data frame of scrambled ones.
                    let b1 = encoder.mapping_frames() < encoder.params().framing.p as u64;
                    let data = &mut self.data;
                    return encoder.next_symbol(&mut || if b1 { true } else { data.pop_front().unwrap_or(true) });
                }
            }
        }
    }
}

/// How the client reads the downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Down {
    /// Silence, until the preamble.
    Quiet,
    /// The script's fixed segments, counted from the preamble's first octet.
    Script(usize),
    /// Frames in levels A: sync, and the records.
    LevelsA,
    /// Frames in the data levels: the start's, then data.
    Data(usize),
    /// A rate change, from the marker's first frame.
    Change(usize),
}

/// Phase 2 as an x2 client has it: V.34's call modem up to the server's L1,
/// then its marker in place of L1, L2 and INFO1c.
#[derive(Debug, Clone)]
struct Front {
    v34: phase2::Modem,
    marker: dpsk::Transmitter,
    now: u64,
    /// When the server's L1 began, as this end heard it.
    probe_at: Option<u64>,
    sent: bool,
    quiet: u64,
}

/// The server's L1 lasts 160 ms; the marker follows 10 to 100 ms after it.
const MARKER_AFTER_PROBE: f64 = 0.190;

/// The simulated client.
#[derive(Debug, Clone)]
pub struct Client {
    front: Option<Front>,
    tx: Transmitter,
    upstream: Upstream,
    gain: f64,
    down: Down,
    descrambler: Scrambler,
    mapper: Mapper,
    bank: &'static Bank,
    octets: [u8; FRAME],
    at: usize,
    window: VecDeque<bool>,
    received: Vec<bool>,
    /// Server records the client has read.
    pub records_heard: usize,
    /// Frames the client could not read in the levels it expected.
    pub bad_frames: usize,
}

impl Client {
    /// After its marker: the high carrier at 3200 baud, as a Courier's.
    pub fn new() -> Self {
        let band = Band::new(SymbolRate::S3200, true);
        let mut upstream = Upstream {
            sender: Sender::new(Mode::Answer),
            up: Up::Silence,
            count: 0,
            queue: VecDeque::new(),
            after: VecDeque::new(),
            size: Size::Four,
            encoder: None,
            data: VecDeque::new(),
            index: 1,
            acked: false,
            mp_sent_acked: false,
        };
        upstream.then(&[Up::S, Up::SBar, Up::Pp, Up::Trn(PHASE3_TRN, Size::Four), Up::J]);
        Self {
            front: None,
            tx: Transmitter::new(band, 0, 0, FS),
            upstream,
            gain: 0.25,
            down: Down::Quiet,
            descrambler: Scrambler::new(Mode::Call),
            mapper: Mapper::levels_a(),
            bank: Bank::get(1, 0).unwrap(),
            octets: [0; FRAME],
            at: 0,
            window: VecDeque::new(),
            received: Vec::new(),
            records_heard: 0,
            bad_frames: 0,
        }
    }

    /// From the start of phase 2: V.34's call modem until the server's L1,
    /// then the marker (`4d`), then phase 3.
    pub fn from_phase2() -> Self {
        let mut c = Self::new();
        c.upstream.after.push_front(c.upstream.up);
        c.upstream.up = Up::Silence;
        c.front = Some(Front {
            v34: phase2::Modem::new(Role::Call, FS),
            marker: dpsk::Transmitter::new(dpsk::Side::Call, FS),
            now: 0,
            probe_at: None,
            sent: false,
            quiet: 0,
        });
        c
    }

    /// Whether phase 2 is over for it.
    pub fn past_phase2(&self) -> bool {
        self.front.is_none()
    }

    fn front_step(&mut self, input: f64) -> Option<f64> {
        let f = self.front.as_mut()?;
        f.now += 1;
        let v34 = f.v34.step(input);
        if f.probe_at.is_none() && f.v34.phase() == "V.34 reading the probe" {
            f.probe_at = Some(f.now);
        }
        if !f.sent && f.probe_at.is_some_and(|at| f.now >= at + (MARKER_AFTER_PROBE * FS) as u64) {
            let mut bits = vec![true, true];
            bits.extend(X2Marker::from_body(0x4d).to_bits());
            f.marker.send(&bits);
            f.marker.silence();
            f.sent = true;
        }
        if !f.sent {
            // Quiet once the server's L1 is in: no L1, L2 or tone of its own.
            return Some(if f.probe_at.is_some() { 0.0 } else { v34 });
        }
        if f.marker.is_sending() {
            return Some(f.marker.next_sample());
        }
        f.quiet += 1;
        if f.quiet < (0.075 * FS) as u64 {
            return Some(0.0);
        }
        self.front = None;
        self.upstream.next_up();
        None
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.upstream.data.extend(bits.iter().copied());
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    /// Whether data is moving both ways.
    pub fn in_data(&self) -> bool {
        self.upstream.up == Up::Data && matches!(self.down, Down::Data(n) if n > START)
    }

    /// The bank the downstream is read in.
    pub fn bank(&self) -> &'static Bank {
        self.bank
    }

    /// Ask for rate `index`: S, S-bar, TRN and an MP on four points.
    pub fn change_rate(&mut self, index: u8) {
        let u = &mut self.upstream;
        u.index = index;
        u.acked = false;
        u.encoder = None;
        u.then(&[Up::S, Up::SBar, Up::Trn(CHANGE_TRN, Size::Four), Up::Records(Size::Four)]);
    }

    /// One network sample in (the server's level), one out.
    pub fn step(&mut self, input: f64) -> f64 {
        if let Some(out) = self.front_step(input) {
            return out;
        }
        let (u, negative) = ucode::nearest(Law::Mu, (input * 32768.0).round() as i32);
        self.octet(ucode::octet(Law::Mu, u, negative));
        let upstream = &mut self.upstream;
        self.tx.next_sample(|| upstream.symbol()) * self.gain
    }

    fn octet(&mut self, o: u8) {
        match self.down {
            Down::Quiet => {
                // The lead is 7f too; the preamble's first is ab.
                if o != 0x7f && o != 0xff {
                    self.down = Down::Script(0);
                    self.octet(o);
                }
            }
            Down::Script(n) => self.script_octet(n, o),
            _ => {
                self.octets[self.at] = o;
                self.at += 1;
                if self.at == FRAME {
                    self.at = 0;
                    let f = self.octets;
                    self.frame(f);
                }
            }
        }
    }

    fn script_octet(&mut self, n: usize, o: u8) {
        self.down = Down::Script(n + 1);
        if n == J_OVER_PREAMBLE && self.upstream.up == Up::J {
            // Quiet until the server's J words.
            self.upstream.start(Up::Silence);
        }
        if (PRE..PRE + TRN + JW + ONES).contains(&n) {
            // One scrambled bit an octet: negative for a 1.
            self.descrambler.descramble(o & 0x80 == 0);
        }
        if n + 1 == PRE + TRN + JW {
            // Phase 4, on the J words.
            self.upstream.then(&[
                Up::S,
                Up::SBar,
                Up::Trn(PHASE4_TRN4, Size::Four),
                Up::Trn(PHASE4_TRN16, Size::Sixteen),
                Up::Records(Size::Sixteen),
            ]);
        }
        if n + 1 == PRE + TRN + JW + ONES + ALIGN {
            self.down = Down::LevelsA;
            self.at = 0;
        }
    }

    fn frame(&mut self, f: [u8; FRAME]) {
        match self.down {
            Down::LevelsA => {
                if let Some(bits) = self.mapper.clone().unframe(&f) {
                    self.mapper.unframe(&f);
                    self.read_records(bits);
                } else if self.upstream.acked {
                    // The start's second frame: the data levels from here.
                    self.mapper.switch_to(self.bank);
                    self.data_frame(0, f);
                } else {
                    self.bad_frames += 1;
                }
            }
            Down::Data(n) => {
                if self.is_marker(&f) {
                    self.down = Down::Change(1);
                    return;
                }
                self.data_frame(n, f);
            }
            Down::Change(k) => self.change_frame(k, f),
            _ => {}
        }
    }

    fn is_marker(&self, f: &[u8; FRAME]) -> bool {
        (0..FRAME).all(|i| f[i] | 0x80 == self.bank.list(i).recv[0] && (f[i] & 0x80 != 0) == (i < 3))
    }

    fn read_records(&mut self, bits: Vec<bool>) {
        for b in bits {
            let b = self.descrambler.descramble(b);
            self.window.push_back(b);
            if self.window.len() > record::bits(3) {
                self.window.pop_front();
            }
            if self.window.len() == record::bits(3) {
                let w: Vec<bool> = self.window.iter().copied().collect();
                if record::parse(&w, 3).is_some() {
                    self.records_heard += 1;
                    self.window.clear();
                    self.upstream.acked = true;
                }
            }
        }
    }

    fn data_frame(&mut self, n: usize, f: [u8; FRAME]) {
        self.down = Down::Data(n + 1);
        let Some(bits) = self.mapper.unframe(&f) else {
            self.bad_frames += 1;
            return;
        };
        let bits: Vec<bool> = bits.into_iter().map(|b| self.descrambler.descramble(b)).collect();
        if n >= START {
            self.received.extend(bits);
        }
    }

    /// A rate change, from the marker: 9 frames of it, 172 of records and one
    /// of ones in index 1's levels, 681 of ones in the new, then data.
    fn change_frame(&mut self, k: usize, f: [u8; FRAME]) {
        self.down = Down::Change(k + 1);
        if k < 9 {
            if k == 8 {
                self.descrambler.reset();
                self.mapper = Mapper::data(Bank::get(1, 0).unwrap());
                self.bank = Bank::get(self.upstream.index, 0).unwrap();
            }
            return;
        }
        let j = k - 9;
        if j == 173 {
            // The first of 681 frames of ones in the new levels: the next is
            // the first of the 680 the start-up has too.
            self.mapper.switch_to(self.bank);
            self.data_frame(0, f);
            self.down = Down::Data(0);
            return;
        }
        let Some(bits) = self.mapper.unframe(&f) else {
            self.bad_frames += 1;
            return;
        };
        if j < 172 {
            self.read_records(bits);
        } else {
            for b in bits {
                self.descrambler.descramble(b);
            }
        }
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

fn upstream_rate() -> u32 {
    (1..=12u32).rev().find(|n| UPSTREAM_MASK >> (n - 1) & 1 != 0).unwrap() * 2400
}

fn grid(point: Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(receiver::unit(size))
}
