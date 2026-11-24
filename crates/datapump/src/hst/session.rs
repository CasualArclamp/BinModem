//! One end of an HST call: the two channels, HDLC, MNP and the handover.
//!
//! What it follows from the Courier's firmware:
//!
//! - The answering end holds the forward channel at the start and the caller
//!   the back channel, and the caller is the MNP originator: it sends the
//!   link request, on the back channel.
//! - The link request carries USR's parameter 0xC0. Once both ends have
//!   agreed, the forward channel is retrained at the fastest rate both allow
//!   (the firmware's connect at 8FBB:1C70 does the same: idle, then the
//!   agreed rate, then the forward transmitter).
//! - The end on the back channel asks for the forward channel with an
//!   attention of code 0x1R, R the rate code. The holder acknowledges, and
//!   once that acknowledgement has gone it drops the forward carrier and
//!   moves to the back channel; the requester, on the acknowledgement,
//!   drops the back channel and trains the forward one (firmware events 0x73
//!   and 0x76).
//!
//! What is this implementation's own: when to ask (a backlog over a
//! threshold, with a minimum time between handovers), starting the forward
//! channel at 7200 before the rates are agreed, and a watchdog that settles
//! the ends back into opposite roles if a handover is half lost.

use std::collections::VecDeque;

use ec::hdlc::{Decoder, Encoder, Fcs};
use ec::mnp::{self, Event as MnpEvent, HstParam, Link, Lr, Role, Timing};

use super::{BackRate, BackRx, BackTx, ForwardRx, ForwardTx, Rate, rate_code, rate_from_code};

/// Which end of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Answered: holds the forward channel first; MNP responder.
    Answerer,
    /// Called: holds the back channel first; MNP originator.
    Caller,
}

/// Which channel this end transmits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holding {
    Forward,
    Back,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Forward rates this end allows; the fastest both allow is used.
    pub rates: &'static [Rate],
    pub back: BackRate,
    /// Octets waiting to go before the back-channel end asks for the forward
    /// channel.
    pub turnaround_backlog: usize,
    /// Least time between handovers, in milliseconds.
    pub min_hold_ms: u32,
    /// Let the receiving end move the forward rate down and up with the
    /// line, as the Courier's online fallback does.
    pub fallback: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rates: &[Rate::R7200, Rate::R9600, Rate::R12000, Rate::R14400],
            back: BackRate::B450,
            turnaround_backlog: 256,
            min_hold_ms: 3000,
            fallback: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// MNP is up; the forward channel will run at this rate.
    Connected { rate: Rate, back: BackRate },
    /// This end now transmits on this channel.
    Holding(Holding),
    /// The forward channel is retraining at this rate.
    Rate(Rate),
    Disconnected,
}

/// The rate the forward channel trains at before the rates are agreed.
const OPENING_RATE: Rate = Rate::R7200;
/// No carrier from the far end for this long, outside a handover, and the
/// roles are put back: the answerer takes the forward channel, the caller
/// the back.
const WATCHDOG_MS: u32 = 5000;
/// No carrier from the far end for this long, once the link is up, and the
/// call is over: long enough for the watchdog to put the roles back and the
/// far end to train again, which a far end that is still there always does.
const CARRIER_LOST_MS: u32 = 12_000;
/// After the last frame before a handover has left the transmitter, how long
/// to keep the carrier up so the far receiver's filters and trellis decoder
/// give it all up.
const DRAIN_MS: u32 = 120;
/// After a training, how long the forward receiver's figures are left to
/// settle before the fallback reads them.
const SETTLE_MS: u32 = 1000;
/// How long a rate change asked for goes unacknowledged before the asker
/// takes it as done.
const ASSUME_MS: u32 = 2500;

/// The signal to noise each forward rate works at, in dB: the figures the
/// V.17 tests run each rate at, 3 dB above where an ideal decoder reaches an
/// error rate of 1e-5.
fn working_snr(rate: Rate) -> f64 {
    match rate {
        Rate::R14400 => 27.0,
        Rate::R12000 => 24.0,
        Rate::R9600 => 21.0,
        Rate::R7200 => 18.0,
    }
}

/// The same for the back channel, whose filters are slower.
const BACK_DRAIN_MS: u32 = 250;
/// How long the requester stays quiet between dropping the back channel and
/// starting the forward training, so the far end is off the forward channel
/// and listening before segment 1 begins: the far end's drain, and margin.
const TAKE_DELAY_MS: u32 = 400;
/// Bits kept queued in a transmitter.
const QUEUE_BITS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// The far end asked for the forward channel; give it up once the
    /// acknowledgement has gone, and then this many milliseconds more for
    /// the far receiver's pipeline to empty.
    GiveUp(Rate, Option<u32>),
    /// Our request was acknowledged: the back channel is down, and the
    /// forward channel starts after this many milliseconds more.
    Take(Rate, u32),
    /// The far end asked for a new forward rate: retrain at it once the
    /// acknowledgement has gone, and the far pipeline has emptied.
    Retrain(Rate, Option<u32>),
    /// The back channel is to change speed once what is queued has gone,
    /// and the far receiver's filters have given it up.
    Slow(BackRate, Option<u32>),
}

/// One end of an HST call.
#[derive(Debug)]
pub struct Session {
    fs: f64,
    side: Side,
    cfg: Config,
    link: Link,
    holding: Holding,
    rate: Rate,
    back: BackRate,
    ftx: ForwardTx,
    frx: ForwardRx,
    btx: BackTx,
    /// Both back-channel speeds are listened for at once, each through its
    /// own frame decoder, so a change of speed needs no coordination on the
    /// receiving side.
    brx: [BackRx; 2],
    bdec: [Decoder; 2],
    enc: Encoder,
    dec: Decoder,
    pending: Pending,
    events: VecDeque<Event>,
    samples_per_tick: usize,
    samples: usize,
    since_switch_ms: u32,
    quiet_ms: u32,
    /// Since the far end's carrier was last heard, not reset by the
    /// watchdog; and whether the call has been given up for that.
    deaf_ms: u32,
    lost: bool,
    /// The rates both ends allow, as a mask of rate codes.
    agreed_rates: u8,
    /// Since the forward receiver last went into data, in milliseconds, and
    /// since a rate change was asked for.
    in_data_ms: u32,
    asked_ms: u32,
}

fn rate_mask(rates: &[Rate]) -> u8 {
    rates.iter().fold(0, |m, &r| m | 1 << rate_code(r))
}

impl Session {
    pub fn new(fs: f64, side: Side, cfg: Config) -> Self {
        let ours = Lr {
            class: 3,
            window: 8,
            max_data: 64,
            optimize: 3,
            hst: Some(HstParam {
                flags: HstParam::HST | if cfg.back == BackRate::B300 { HstParam::BACK_300 } else { 0 },
                rates: rate_mask(cfg.rates),
            }),
        };
        let role = if side == Side::Caller { Role::Originator } else { Role::Responder };
        let mut link = Link::new(role, ours);
        // Long enough to cover a forward training and a handover.
        link.set_timing(Timing { retransmit_ms: 3000, retries: 15 });
        link.connect();
        let mut s = Self {
            fs,
            side,
            cfg,
            link,
            holding: Holding::Back,
            rate: OPENING_RATE,
            // The link request goes at 450 whatever is wanted; 300 is agreed
            // in it.
            back: BackRate::B450,
            ftx: ForwardTx::new(fs),
            frx: ForwardRx::new(fs),
            btx: BackTx::new(fs),
            brx: [BackRx::with_rate(fs, BackRate::B450), BackRx::with_rate(fs, BackRate::B300)],
            bdec: [Decoder::new(Fcs::Bits16), Decoder::new(Fcs::Bits16)],
            enc: Encoder::new(Fcs::Bits16),
            dec: Decoder::new(Fcs::Bits16),
            pending: Pending::None,
            events: VecDeque::new(),
            samples_per_tick: (fs / 100.0).round() as usize,
            samples: 0,
            since_switch_ms: 0,
            quiet_ms: 0,
            deaf_ms: 0,
            lost: false,
            agreed_rates: rate_mask(cfg.rates),
            in_data_ms: 0,
            asked_ms: 0,
        };
        let first = if side == Side::Answerer { Holding::Forward } else { Holding::Back };
        s.take(first);
        s
    }

    pub fn side(&self) -> Side {
        self.side
    }

    pub fn holding(&self) -> Holding {
        self.holding
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// The back channel's speed.
    pub fn back_rate(&self) -> BackRate {
        self.back
    }

    /// Whether the far end's carrier is being heard on the channel this end
    /// listens to.
    pub fn hearing(&self) -> bool {
        match self.holding {
            Holding::Forward => self.brx.iter().any(BackRx::carrier),
            Holding::Back => self.frx.carrier(),
        }
    }

    /// Whether the forward channel is training, or a handover or rate change
    /// is under way: what a Courier blinks its MR lamp for.
    pub fn training(&self) -> bool {
        self.pending != Pending::None
            || match self.holding {
                Holding::Forward => !self.ftx.trained(),
                Holding::Back => !self.frx.in_data(),
            }
    }

    pub fn is_connected(&self) -> bool {
        self.link.is_connected()
    }

    /// Octets queued to send and not yet acknowledged.
    pub fn backlog(&self) -> usize {
        self.link.backlog()
    }

    /// Data frames this end has had to send again.
    pub fn retransmitted(&self) -> u64 {
        self.link.retransmitted()
    }

    pub fn send(&mut self, data: &[u8]) {
        self.link.send_data(data);
    }

    pub fn read(&mut self) -> Vec<u8> {
        self.link.read()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    pub fn hang_up(&mut self) {
        self.link.disconnect(mnp::reason::USER);
    }

    /// Start transmitting on `which`, and listening on the other.
    fn take(&mut self, which: Holding) {
        self.holding = which;
        self.enc = Encoder::new(Fcs::Bits16);
        self.dec = Decoder::new(Fcs::Bits16);
        match which {
            Holding::Forward => {
                self.btx.stop();
                self.ftx.start(self.rate);
                for (rx, dec) in self.brx.iter_mut().zip(&mut self.bdec) {
                    rx.restart();
                    *dec = Decoder::new(Fcs::Bits16);
                }
            }
            Holding::Back => {
                self.ftx.abort();
                self.btx.start(self.back);
                self.frx.set_rate(self.rate);
            }
        }
        self.since_switch_ms = 0;
        self.quiet_ms = 0;
        self.events.push_back(Event::Holding(which));
    }

    /// Retrain the forward channel at a new rate, from whichever end.
    fn retrain(&mut self, rate: Rate) {
        if rate != self.rate {
            self.events.push_back(Event::Rate(rate));
        }
        self.rate = rate;
        match self.holding {
            Holding::Forward => {
                self.enc = Encoder::new(Fcs::Bits16);
                self.ftx.start(rate);
            }
            Holding::Back => {
                self.dec = Decoder::new(Fcs::Bits16);
                self.frx.set_rate(rate);
            }
        }
    }

    fn trained(&self) -> bool {
        match self.holding {
            Holding::Forward => self.ftx.trained(),
            Holding::Back => self.btx.trained(),
        }
    }

    fn queued_bits(&self) -> usize {
        match self.holding {
            Holding::Forward => self.ftx.pending_bits(),
            Holding::Back => self.btx.pending_bits(),
        }
    }

    fn top_up(&mut self) {
        if !self.trained() || self.queued_bits() >= QUEUE_BITS {
            return;
        }
        if self.enc.is_empty() {
            if self.pending == Pending::None {
                match self.link.poll_transmit() {
                    Some(f) => self.enc.frame(&f),
                    None => self.enc.idle(1),
                }
            } else {
                // A handover is waiting: control frames only, and once they
                // are gone let the queue run dry.
                match self.link.poll_control() {
                    Some(f) => self.enc.frame(&f),
                    None => return,
                }
            }
        }
        let bits: Vec<bool> = std::iter::from_fn(|| self.enc.next_bit()).take(QUEUE_BITS).collect();
        match self.holding {
            Holding::Forward => self.ftx.push_bits(&bits),
            Holding::Back => self.btx.push_bits(&bits),
        }
    }

    pub fn next_sample(&mut self) -> f64 {
        self.top_up();
        match self.holding {
            Holding::Forward => self.ftx.next_sample(),
            Holding::Back => self.btx.next_sample(),
        }
    }

    pub fn feed(&mut self, sample: f64) {
        self.frx.feed(sample);
        for rx in &mut self.brx {
            rx.feed(sample);
        }
        match self.holding {
            Holding::Forward => {
                for (rx, dec) in self.brx.iter_mut().zip(&mut self.bdec) {
                    for bit in rx.take_bits() {
                        if let Some(Ok(frame)) = dec.feed(bit) {
                            self.link.receive(&frame);
                        }
                    }
                }
            }
            Holding::Back => {
                let bits = self.frx.take_bits();
                for rx in &mut self.brx {
                    rx.take_bits();
                }
                if self.frx.in_data() {
                    for bit in bits {
                        if let Some(Ok(frame)) = self.dec.feed(bit) {
                            self.link.receive(&frame);
                        }
                    }
                }
            }
        }
        self.samples += 1;
        if self.samples >= self.samples_per_tick {
            self.samples = 0;
            self.tick(10);
        }
    }

    fn tick(&mut self, ms: u32) {
        self.link.tick(ms);
        self.since_switch_ms = self.since_switch_ms.saturating_add(ms);
        while let Some(e) = self.link.poll_event() {
            self.on_link(e);
        }
        // A handover waiting on its acknowledgement going out.
        if let Pending::GiveUp(rate, drain) = self.pending {
            match drain {
                None if self.enc.is_empty() && self.queued_bits() == 0 => {
                    self.pending = Pending::GiveUp(rate, Some(DRAIN_MS));
                }
                Some(left) if left <= ms => {
                    self.pending = Pending::None;
                    self.rate = rate;
                    self.take(Holding::Back);
                }
                Some(left) => self.pending = Pending::GiveUp(rate, Some(left - ms)),
                None => {}
            }
        }
        if let Pending::Retrain(rate, drain) = self.pending {
            match drain {
                None if self.enc.is_empty() && self.queued_bits() == 0 => {
                    self.pending = Pending::Retrain(rate, Some(DRAIN_MS));
                }
                Some(left) if left <= ms => {
                    self.pending = Pending::None;
                    self.retrain(rate);
                }
                Some(left) => self.pending = Pending::Retrain(rate, Some(left - ms)),
                None => {}
            }
        }
        if let Pending::Slow(back, drain) = self.pending {
            match drain {
                None if self.enc.is_empty() && self.queued_bits() == 0 => {
                    self.pending = Pending::Slow(back, Some(BACK_DRAIN_MS));
                }
                Some(left) if left <= ms => {
                    self.pending = Pending::None;
                    self.btx.start(back);
                }
                Some(left) => self.pending = Pending::Slow(back, Some(left - ms)),
                None => {}
            }
        }
        if let Pending::Take(rate, left) = self.pending {
            if left <= ms {
                self.pending = Pending::None;
                self.rate = rate;
                self.take(Holding::Forward);
            } else {
                self.pending = Pending::Take(rate, left - ms);
            }
        }
        // Ask for the forward channel when there is a backlog.
        if self.link.is_connected()
            && self.holding == Holding::Back
            && self.pending == Pending::None
            && self.link.attention_pending().is_none()
            && self.since_switch_ms >= self.cfg.min_hold_ms
            && self.link.backlog() > self.cfg.turnaround_backlog
        {
            self.link.attention(0x10 | rate_code(self.rate));
        }
        self.fallback(ms);
        // The watchdog.
        let hearing = match self.holding {
            Holding::Forward => self.brx.iter().any(BackRx::carrier),
            Holding::Back => self.frx.carrier(),
        };
        self.quiet_ms = if hearing { 0 } else { self.quiet_ms.saturating_add(ms) };
        self.deaf_ms = if hearing || !self.link.is_connected() { 0 } else { self.deaf_ms.saturating_add(ms) };
        if self.deaf_ms >= CARRIER_LOST_MS && !self.lost {
            self.lost = true;
            self.events.push_back(Event::Disconnected);
        }
        if self.quiet_ms >= WATCHDOG_MS && self.pending == Pending::None {
            let should = if self.side == Side::Answerer { Holding::Forward } else { Holding::Back };
            if self.holding != should {
                self.take(should);
            } else {
                self.quiet_ms = 0;
            }
        }
    }

    /// The receiving end's half of the online fallback: watch the forward
    /// channel's signal to noise and ask for the rate it will carry.
    fn fallback(&mut self, ms: u32) {
        if !self.cfg.fallback || self.holding != Holding::Back || !self.link.is_connected() {
            return;
        }
        if !self.frx.in_data() {
            self.in_data_ms = 0;
            return;
        }
        self.in_data_ms = self.in_data_ms.saturating_add(ms);
        // An attention for a rate that is not acknowledged in time is
        // assumed to have been acted on: the far end retrains on receiving
        // it, and its acknowledgement may then be at the rate we are not
        // yet listening for.
        if let Some(code) = self.link.attention_pending() {
            self.asked_ms = self.asked_ms.saturating_add(ms);
            if code & 0xF0 == 0x30 && self.asked_ms >= ASSUME_MS
                && let Some(rate) = rate_from_code(code & 0x0F)
                    && rate != self.rate {
                        self.retrain(rate);
                        self.in_data_ms = 0;
                    }
            return;
        }
        // Give the loops a second to settle after a training.
        if self.in_data_ms < SETTLE_MS {
            return;
        }
        let snr = self.frx.snr_db();
        let allowed = |r: Rate| self.agreed_rates & (1 << rate_code(r)) != 0;
        let ladder = [Rate::R7200, Rate::R9600, Rate::R12000, Rate::R14400];
        let here = ladder.iter().position(|&r| r == self.rate).unwrap_or(0);
        let target = if snr < working_snr(self.rate) - 2.0 {
            // Down as far as the line needs, at once.
            ladder[..here].iter().rev().copied().find(|&r| allowed(r) && working_snr(r) - 1.0 <= snr).or_else(|| ladder.first().copied())
        } else {
            // Up one step, when the line has room for it.
            ladder.get(here + 1).copied().filter(|&r| allowed(r) && snr > working_snr(r) + 1.0)
        };
        if let Some(rate) = target
            && rate != self.rate {
                self.asked_ms = 0;
                self.link.attention(0x30 | rate_code(rate));
            }
    }

    fn on_link(&mut self, e: MnpEvent) {
        match e {
            MnpEvent::Connected(agreed) => {
                if let Some(h) = agreed.hst {
                    self.agreed_rates = h.rates;
                }
                let best = agreed.hst.and_then(|h| h.best_rate()).and_then(rate_from_code).unwrap_or(OPENING_RATE);
                if let Some(h) = agreed.hst {
                    let back = if h.flags & HstParam::BACK_300 != 0 { BackRate::B300 } else { self.cfg.back };
                    if back != self.back {
                        self.back = back;
                        if self.holding == Holding::Back {
                            // After the confirmation has gone: see Pending.
                            self.pending = Pending::Slow(back, None);
                        }
                    }
                }
                if best != self.rate {
                    self.retrain(best);
                }
                self.events.push_back(Event::Connected { rate: best, back: self.back });
            }
            MnpEvent::Attention(code) if code & 0xF0 == 0x10 && self.holding == Holding::Forward => {
                let rate = rate_from_code(code & 0x0F).unwrap_or(self.rate);
                self.pending = Pending::GiveUp(rate, None);
            }
            MnpEvent::Attention(code) if code & 0xF0 == 0x30 && self.holding == Holding::Forward => {
                if let Some(rate) = rate_from_code(code & 0x0F)
                    && rate != self.rate && self.pending == Pending::None {
                        self.pending = Pending::Retrain(rate, None);
                    }
            }
            MnpEvent::AttentionAcked(code) if code & 0xF0 == 0x30 && self.holding == Holding::Back => {
                if let Some(rate) = rate_from_code(code & 0x0F)
                    && rate != self.rate {
                        self.retrain(rate);
                        self.in_data_ms = 0;
                    }
            }
            MnpEvent::AttentionAcked(code) if code & 0xF0 == 0x10 && self.holding == Holding::Back => {
                let rate = rate_from_code(code & 0x0F).unwrap_or(self.rate);
                self.btx.stop();
                self.pending = Pending::Take(rate, TAKE_DELAY_MS);
            }
            MnpEvent::Disconnected { .. } => self.events.push_back(Event::Disconnected),
            _ => {}
        }
    }

    /// Sample rate this session runs at.
    pub fn sample_rate(&self) -> f64 {
        self.fs
    }
}
