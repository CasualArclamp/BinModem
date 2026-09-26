//! The control channel's receiver: the far end's band picked out from under
//! this end's own signal, what is in it told, the far end's symbols brought
//! back after PPh or Sh, and their bits handed up.
//!
//! In order, a sample at a time:
//!
//! 1. A channel-select band-pass. On a two-wire line this end's own signal
//!    comes back into its receiver, and can be twenty decibels and more above
//!    the far end's. The two directions are 1200 Hz apart and each is 1050 Hz
//!    wide, so 150 Hz separates them; and the answer modem's 1800 Hz guard
//!    tone is 600 Hz from either carrier, 75 Hz past the edge of either
//!    signal -- this end's own guard tone at the answer modem, the far end's
//!    at the call modem. `dsp::qam::Core`'s own 64-tap front end is
//!    nowhere near that sharp (`code-v34.md` Q6), so ahead of it goes a
//!    linear-phase band-pass as `dpsk.rs` and `v22bis.rs` have, a Kaiser
//!    low-pass moved to the far carrier: flat to 480 Hz either side, 65 dB
//!    down from 590. The far end's signal past 480 Hz is 16 dB down on its
//!    own and gone by 525; the equaliser makes up the little the filter
//!    takes.
//! 2. The core, fed every sample: a fixed mixer, stored samples, an
//!    equaliser at two samples a symbol solved outright on a known sequence,
//!    carrier and timing loops, loss and resync (`dsp::qam`).
//! 3. Beside it, a matched filter of its own on the same samples, read two
//!    to a symbol on a grid that never moves, into the watch
//!    ([`super::watch`]), which says what the far end is sending.
//! 4. When the watch hears PPh, or Sh turning into S-bar-h, the same matched
//!    filter finds all 32 symbols of it by correlation -- where it starts,
//!    the carrier's phase and its turn, the line's gain -- and then decides
//!    the symbols after it, ALT and whatever follows ALT at 1200 bit/s, on
//!    those references. Their bits are handed up as they come. When 80
//!    symbols are in (or E has ended, if the data after it is at 2400
//!    bit/s), the core is trained by least squares on all of them: the 32
//!    known and the rest as decided.
//! 5. From there the core makes the symbols, and they are decided, the
//!    differential code undone and the bits descrambled (`signals::Reader`).
//!    Twenty descrambled ones in a row are E (the rule of `mp::Finder`), and
//!    after E the bits are the user's, at the data rate.
//!
//! Why not train the core on PPh alone, as the Recommendation has it: 32
//! symbols against an equaliser of 31 taps. A least-squares solve with as
//! many unknowns as rows fits the noise as well as the line, and says the
//! line is far better than it is -- and the core's gate and loss watch are
//! both judged against what training said. The symbols after PPh are ALT at
//! 1200 bit/s for at least 16T (12.4.1.1, 12.4.2.3), then MPh, and no data
//! can begin before an MPh has crossed each way and E after it, 102 symbols
//! after PPh began; four diagonal points decided on PPh's own references are
//! as good as known. After Sh and S-bar-h there may be as little as 16T of
//! ALT and 10 of E before data, at 2400 bit/s if that was the rate before,
//! so there the window ends with E.

use std::collections::VecDeque;
use std::f64::consts::TAU;
use std::sync::OnceLock;

use dsp::qam::{Band, Constellation, Core, Heard as CoreHeard, Options, Slicer, Training, Window};
use dsp::{Complex, fir_lowpass_kaiser, rrc_at};

use super::watch::{Hearing, Seen, Watch};
use super::{BAUD, E_BITS, PPH_SYMBOLS, Rate, Reading, SH_SYMBOLS, far, samples_per_symbol, scrambler_of, sh, sh_bar, unit_point};
use crate::v34::constellation::{Point, clockwise, quarter};
use crate::v34::dpsk::{ROLLOFF, Side};
use crate::v34::signals::{Reader, Size};

/// The channel-select filter's low-pass prototype: flat to here, and this far
/// down from there (see the module's comment).
const SELECT_PASS: f64 = 480.0;
const SELECT_STOP: f64 = 590.0;
const SELECT_DB: f64 = 65.0;

/// Symbols either side of the centre the matched filter reaches, and the
/// places between two samples its table is made for.
const MATCHED_SPAN: usize = 4;
const MATCHED_PHASES: usize = 64;

/// How much of the mixed-down signal is kept for finding a sequence in and
/// deciding what followed it: the furthest back that is read is 46 symbols
/// before PPh was heard, and the newest 80 symbols after it began.
const KEPT_SECONDS: f64 = 0.4;

/// Symbols known and decided that the core is trained on: PPh or Sh and
/// S-bar-h, and what followed them.
const KNOWN: usize = PPH_SYMBOLS;
const WINDOW_END: usize = 80;

/// Where to look for PPh's first symbol, in symbols from where the watch
/// believed it: from 46 symbols before to 12 before. The watch believes PPh
/// between its 18th symbol, after silence, and about its 36th, after data at
/// a poor signal to noise.
const PPH_FROM: f64 = -46.0;
const PPH_TO: f64 = -12.0;

/// Where to look for Sh's first symbol, in symbols from where the watch
/// heard it reverse: S-bar-h begins a symbol or two before the reversal is
/// heard, and Sh 24 symbols before that.
const SH_FROM: f64 = -30.0;
const SH_TO: f64 = -20.0;

/// Steps a symbol the search for a sequence's start is made in, before the
/// best is refined.
const STEPS: usize = 8;

/// How well a found sequence has to correlate with what it should be, as a
/// share of the most it could: 0.18 is what scrambled data comes to on
/// average against 32 symbols of anything.
const MATCHED: f64 = 0.7;

/// The phase loop's share of each decided symbol's phase error, while the
/// symbols after a sequence are decided on its references.
const DECIDING_GAIN: f64 = 0.05;

/// Half symbols either side of where the sequence was found that the core's
/// training looks for it.
const TRAINING_SEARCH: i64 = 2;

/// Signal to noise, in decibels, below which the core takes a training to
/// have fitted nothing and looks for the signal with the taps an earlier one
/// left, if there was one.
const ACCEPT_DB: f64 = 12.0;

/// How long without a carrier, or with the core lost, before the far end is
/// taken to have stopped. A tenth of a second of no carrier is past any
/// jitter buffer's hole; a quarter of a second lost is past the resync that
/// follows a slip, which the core makes 64 symbols after it -- and every bit
/// decided in the meantime is still handed up, since a slip of whole symbols,
/// as a VoIP line's are, leaves the timing and the carrier where they were.
const OFF_SECONDS: f64 = 0.1;
const LOST_SECONDS: f64 = 0.25;

/// Line samples a half-symbol sample of the core's is made after the one it
/// is centred on: its interpolating filter reaches 32 samples ahead
/// (`v27ter/receiver.rs`).
const HALF_LAG: f64 = 31.5;

/// What the receiver is doing with the far end's symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing trained on: listening for PPh, or Sh and S-bar-h.
    Hunting,
    /// Trained, and reading ALT, MPh and E at 1200 bit/s: the bits go to
    /// [`Receiver::take_sync_bits`].
    Sync,
    /// E has come: the bits are the user's, at the data rate, and go to
    /// [`Receiver::take_bits`].
    Data,
}

/// What a training started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    Pph(Reading),
    /// Sh and S-bar-h.
    Sh,
}

/// What the receiver has to report. Every `at` is a sample of this
/// receiver's input, counted from nought as [`Receiver::now`] counts them,
/// and says where on the line the thing was, not when it was worked out --
/// exactly for PPh, the reversal and E, which are found symbol by symbol;
/// within the 20 ms the envelope takes for the carrier; and for a tone, AC
/// and Sh, where they had repeated for long enough to be told apart: about
/// 14 symbols into them after silence and 18 after data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Heard {
    /// The far carrier came, or went: its envelope in this end's band
    /// crossed a level 54 dB under the nominal (5 dB lower going).
    Carrier { on: bool, at: u64 },
    /// An unmodulated carrier began: tone A or B (12.4.3.1, 12.7).
    Tone { at: u64 },
    /// AC began (10.2.4.1). 12.8.2 answers it once it has lasted 100 ms,
    /// which [`Receiver::hearing_since`] measures.
    Ac { at: u64 },
    /// Sh began.
    Sh { at: u64 },
    /// Sh turned into S-bar-h: `at` is where S-bar-h began. Its 8 symbols
    /// end [`super::SH_BAR_SYMBOLS`] symbols later.
    Reversal { at: u64 },
    /// All of PPh, found: where its first symbol began and its last ended,
    /// and in which of 10-2's readings.
    Pph { reading: Reading, began: u64, ended: u64 },
    /// The equaliser is trained, and the core has taken over.
    Trained { on: Reference, snr_db: f64 },
    /// Nothing fitted; the receiver is hunting again.
    Untrained { on: Reference },
    /// E ended at `at`: what follows is data.
    E { at: u64 },
    /// The far end stopped -- its carrier went, the core lost it for good,
    /// or it began a tone, AC or Sh -- and the receiver is hunting again.
    Lost { at: u64 },
}

/// The four- and sixteen-point constellations, each with the grid point of
/// every label, for the core to decide against and the reader to read.
#[derive(Debug)]
struct Tables {
    four: Slicer,
    four_points: Vec<Point>,
    sixteen: Slicer,
    sixteen_points: Vec<Point>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let four_points: Vec<Point> = (0..4).map(|turn| clockwise(quarter(0), turn)).collect();
        let sixteen_points: Vec<Point> =
            (0..4).flat_map(|label| (0..4).map(move |turn| clockwise(quarter(label), turn))).collect();
        let table = |points: &[Point], size: Size| {
            Slicer::table(Constellation::new(points.iter().map(|&p| unit_point(p, size)).collect()))
        };
        Tables {
            four: table(&four_points, Size::Four),
            sixteen: table(&sixteen_points, Size::Sixteen),
            four_points,
            sixteen_points,
        }
    })
}

impl Tables {
    fn slicer(&self, size: Size) -> Slicer {
        match size {
            Size::Four => self.four.clone(),
            Size::Sixteen => self.sixteen.clone(),
        }
    }

    fn point(&self, size: Size, label: usize) -> Point {
        match size {
            Size::Four => self.four_points[label],
            Size::Sixteen => self.sixteen_points[label],
        }
    }
}

/// A real finite impulse response, the newest samples kept twice over so
/// that the ones it reaches always lie in one run.
#[derive(Debug, Clone)]
struct Select {
    taps: Vec<f64>,
    buffer: Vec<f64>,
    at: usize,
}

impl Select {
    /// The band-pass about `carrier`.
    fn new(carrier: f64, fs: f64) -> Self {
        let prototype = fir_lowpass_kaiser(SELECT_PASS, SELECT_STOP, SELECT_DB, fs);
        let middle = (prototype.len() / 2) as f64;
        // Moved up to the carrier about its own middle, which keeps it
        // symmetric and so of linear phase.
        let taps: Vec<f64> =
            prototype.iter().enumerate().map(|(i, h)| 2.0 * h * (TAU * carrier * (i as f64 - middle) / fs).cos()).collect();
        let n = taps.len();
        Self { taps, buffer: vec![0.0; 2 * n], at: 0 }
    }

    /// Samples the output lags the input by.
    fn delay(&self) -> f64 {
        (self.taps.len() / 2) as f64
    }

    fn process(&mut self, x: f64) -> f64 {
        let n = self.taps.len();
        self.buffer[self.at] = x;
        self.buffer[self.at + n] = x;
        self.at = (self.at + 1) % n;
        // Oldest first; the taps are symmetric, so no reversing them.
        let window = &self.buffer[self.at..self.at + n];
        // Four sums side by side, which the compiler can keep in step.
        let mut lanes = [0.0; 4];
        for (chunk, taps) in window.as_chunks::<4>().0.iter().zip(self.taps.as_chunks::<4>().0) {
            for ((lane, x), h) in lanes.iter_mut().zip(chunk).zip(taps) {
                *lane += x * h;
            }
        }
        let rest = n % 4;
        let tail: f64 = window[n - rest..].iter().zip(&self.taps[n - rest..]).map(|(x, h)| x * h).sum();
        lanes.iter().sum::<f64>() + tail
    }
}

/// The matched filter, as a table of the pulse at [`MATCHED_PHASES`] places
/// between two samples, so that its output can be had at any instant.
#[derive(Debug, Clone)]
struct Matched {
    /// Samples it reaches either side.
    reach: i64,
    width: usize,
    table: Vec<f64>,
}

impl Matched {
    fn new(sps: f64) -> Self {
        let edge = MATCHED_SPAN as f64 + 1.0;
        let reach = (edge * sps).ceil() as i64;
        let width = (2 * reach + 2) as usize;
        let pulse = |u: f64| {
            if u.abs() >= edge { 0.0 } else { rrc_at(u, ROLLOFF) * (0.5 + 0.5 * (std::f64::consts::PI * u / edge).cos()) }
        };
        // Scaled so that a symbol sent through the far end's pulse comes out
        // at its own size: the pulse against itself sums to one.
        let gain: f64 = (-reach..=reach).map(|n| pulse(n as f64 / sps) * rrc_at(n as f64 / sps, ROLLOFF)).sum();
        let mut table = vec![0.0; MATCHED_PHASES * width];
        for phase in 0..MATCHED_PHASES {
            let mu = phase as f64 / MATCHED_PHASES as f64;
            for i in 0..width {
                // Tap i meets the sample `reach - i` before the instant.
                let tau = mu + reach as f64 - i as f64;
                table[phase * width + i] = pulse(tau / sps) / gain;
            }
        }
        Self { reach, width, table }
    }
}

/// A sequence found: where its first symbol is centred, in samples of the
/// select filter's output, and the references it gives.
#[derive(Debug, Clone, Copy)]
struct Found {
    reference: Reference,
    start: f64,
    /// The carrier's phase at the sequence's middle symbol, its turn a
    /// symbol, and the line's gain.
    phase: f64,
    turn: f64,
    gain: f64,
}

/// The symbols after a sequence, being decided on its references.
#[derive(Debug, Clone)]
struct Deciding {
    found: Found,
    /// Symbols known and decided, at unit power, from the sequence's first.
    targets: Vec<Complex>,
    /// The phase loop's correction so far.
    correction: f64,
    end: usize,
}

/// A sequence the watch heard, to be looked for once all of it could be in.
#[derive(Debug, Clone, Copy)]
struct Search {
    /// Where to look for its first symbol, in samples of the select filter's
    /// output.
    from: f64,
    to: f64,
    /// When the matched filter's output has to have reached, and the
    /// carrier's turn a symbol as the watch heard it.
    due: f64,
    turn: f64,
}

/// What the receiver is doing with a sequence it has found.
#[derive(Debug, Clone)]
enum Work {
    Idle,
    Deciding(Box<Deciding>),
    /// The core has been given a training and has yet to say how it went.
    Training(Reference),
}

/// The control channel's receiver, for what the far end sends.
#[derive(Debug)]
pub struct Receiver {
    side: Side,
    fs: f64,
    select: Select,
    core: Core,
    /// The core's recent half-symbol samples and where each is centred, in
    /// samples of the select filter's output.
    halves: VecDeque<(u64, f64)>,
    noted: u64,
    /// The mixer's phase, as a fraction of a turn, and its step.
    mixer: f64,
    mixer_step: f64,
    /// Mixed-down samples, and the index of the first.
    baseband: VecDeque<Complex>,
    first: u64,
    kept: usize,
    /// Samples taken.
    taken: u64,
    matched: Matched,
    sps: f64,
    /// Where the watch's half-symbol grid starts, and its next point.
    grid_start: f64,
    grid_next: f64,
    watch: Watch,
    /// PPh, and Sh with S-bar-h, heard and to be looked for. Kept apart from
    /// the work in hand, which only a sequence found replaces: something
    /// that repeats like PPh for a moment in the middle of ALT is looked for
    /// and not found.
    pph: Option<Search>,
    sh: Option<Search>,
    work: Work,
    phase: Phase,
    rate: Rate,
    /// What the symbols are decided against now.
    size: Size,
    reader: Reader,
    /// Descrambled ones in a row, for E.
    ones: usize,
    e_seen: bool,
    sync_bits: Vec<bool>,
    data_bits: Vec<bool>,
    heard: VecDeque<Heard>,
    reference: Option<Reference>,
    /// Since when the carrier has been gone, and the core lost.
    off_since: Option<u64>,
    lost_since: Option<u64>,
    last: Complex,
}

impl Receiver {
    /// The receiver of the call or the answer modem -- `side` is this end --
    /// for what the other end sends, at `fs` samples a second.
    pub fn new(side: Side, fs: f64) -> Self {
        let carrier = far(side).carrier();
        let (p, q) = samples_per_symbol(fs);
        let sps = p as f64 / q as f64;
        let matched = Matched::new(sps);
        let grid_start = matched.reach as f64 + 2.0;
        Self {
            side,
            fs,
            select: Select::new(carrier, fs),
            core: new_core(fs, carrier),
            halves: VecDeque::new(),
            noted: 0,
            mixer: 0.0,
            mixer_step: carrier / fs,
            baseband: VecDeque::new(),
            first: 0,
            kept: (KEPT_SECONDS * fs) as usize,
            taken: 0,
            matched,
            sps,
            grid_start,
            grid_next: grid_start,
            watch: Watch::new(2.0 * BAUD),
            pph: None,
            sh: None,
            work: Work::Idle,
            phase: Phase::Hunting,
            rate: Rate::R1200,
            size: Size::Four,
            reader: Reader::new(scrambler_of(far(side))),
            ones: 0,
            e_seen: false,
            sync_bits: Vec::new(),
            data_bits: Vec::new(),
            heard: VecDeque::new(),
            reference: None,
            off_since: None,
            lost_since: None,
            last: Complex::ZERO,
        }
    }

    /// This end: the receiver listens to the other.
    pub fn side(&self) -> Side {
        self.side
    }

    /// Samples taken: the clock every [`Heard`] is counted on.
    pub fn now(&self) -> u64 {
        self.taken
    }

    /// The next thing heard, if there is one.
    pub fn heard(&mut self) -> Option<Heard> {
        self.heard.pop_front()
    }

    /// What the far end is sending now.
    pub fn hearing(&self) -> Hearing {
        self.watch.hearing()
    }

    /// The sample of this receiver's input where what the far end is sending
    /// now began, give or take a few symbols.
    pub fn hearing_since(&self) -> u64 {
        self.line_time(self.grid_time(self.watch.since()))
    }

    /// For how long, in seconds, the far end has been sending what it is
    /// sending now.
    pub fn hearing_for(&self) -> f64 {
        self.taken.saturating_sub(self.hearing_since()) as f64 / self.fs
    }

    /// Whether the far end's carrier is on the line.
    pub fn carrier(&self) -> bool {
        self.watch.carrier()
    }

    /// The far carrier's envelope, in the units of its own symbols: one for a
    /// far end at the nominal level on a line with no loss.
    pub fn level(&self) -> f64 {
        self.watch.level()
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether the far end's symbols are being read: from the moment PPh or
    /// Sh is found until the far end stops.
    pub fn is_locked(&self) -> bool {
        match self.work {
            Work::Deciding(_) | Work::Training(_) => true,
            _ => self.phase != Phase::Hunting && self.core.is_tracking() && !self.core.is_lost(),
        }
    }

    /// Whether E has come since the last PPh or Sh was found.
    pub fn e_seen(&self) -> bool {
        self.e_seen
    }

    /// The rate the far end's data comes at after E: what this end asked for
    /// in its own MPh (12.4.1.4, 12.4.2.5). ALT, MPh and E are at 1200 bit/s
    /// whatever this is. Kept across resynchronisations, which "send control
    /// data at the previous control channel rate" (12.6.1.4).
    pub fn set_rate(&mut self, rate: Rate) {
        self.rate = rate;
        if self.phase == Phase::Data && self.size != rate.size() {
            self.size = rate.size();
            self.core.set_slicer(tables().slicer(self.size));
        }
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// What the last training started from, and so which reading of PPh
    /// the far end sends, if it has sent one.
    pub fn reference(&self) -> Option<Reference> {
        self.reference
    }

    /// The bits of ALT, MPh and E, descrambled, since this was last asked.
    pub fn take_sync_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.sync_bits)
    }

    /// The user's bits, descrambled, since this was last asked.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.data_bits)
    }

    /// Stop reading the far end's symbols and hunt for PPh or Sh again,
    /// keeping what the equaliser has learnt for a training that fits
    /// nothing to fall back on.
    pub fn stop(&mut self) {
        self.work = Work::Idle;
        self.phase = Phase::Hunting;
        self.core.idle();
        self.lost_since = None;
    }

    /// Hunt afresh, forgetting everything learnt: the next PPh or Sh trains
    /// the equaliser from nothing, with no earlier taps to fall back on.
    pub fn reset(&mut self) {
        self.stop();
        self.core = new_core(self.fs, far(self.side).carrier());
        self.halves.clear();
        self.noted = 0;
    }

    /// Signal to noise of the symbols against the nearest point, in decibels.
    pub fn snr_db(&self) -> f64 {
        self.core.snr_db()
    }

    /// What the last training came to, in decibels.
    pub fn trained_snr_db(&self) -> f64 {
        self.core.trained_snr_db()
    }

    /// The last symbol, at the constellation's own unit power.
    pub fn constellation_point(&self) -> (f64, f64) {
        self.last.into()
    }

    /// Slips found and followed.
    pub fn slips(&self) -> u32 {
        self.core.slips()
    }

    /// The far clock against this end's, in parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.core.drift_ppm()
    }

    /// The far carrier's offset from where it should be, in hertz.
    pub fn offset_hz(&self) -> f64 {
        self.core.offset_hz()
    }

    /// Take one sample of the line.
    pub fn feed(&mut self, sample: f64) {
        let selected = self.select.process(sample);
        let index = self.taken;
        self.taken += 1;
        self.core.feed(selected);
        let angle = TAU * self.mixer;
        self.mixer = (self.mixer + self.mixer_step).fract();
        self.baseband.push_back(Complex::new(angle.cos(), -angle.sin()).scale(2.0 * selected));
        if self.baseband.len() > self.kept {
            self.baseband.pop_front();
            self.first += 1;
        }
        while let Some(half) = self.matched_at(self.grid_next) {
            self.grid_next += self.sps / 2.0;
            self.watch.push(half);
            while let Some(seen) = self.watch.seen() {
                self.on_seen(seen);
            }
        }
        self.work();
        self.track();
        self.note_halves(index);
        self.supervise();
    }

    /// The matched filter's output at `time`, in samples of the select
    /// filter's output, if every sample it reaches is kept.
    fn matched_at(&self, time: f64) -> Option<Complex> {
        let floor = time.floor();
        let mut phase = ((time - floor) * MATCHED_PHASES as f64).round() as usize;
        let mut base = floor as i64;
        if phase == MATCHED_PHASES {
            phase = 0;
            base += 1;
        }
        let from = base - self.matched.reach;
        let to = from + self.matched.width as i64;
        if from < self.first as i64 || to > (self.first as usize + self.baseband.len()) as i64 {
            return None;
        }
        let offset = (from - self.first as i64) as usize;
        let row = &self.matched.table[phase * self.matched.width..(phase + 1) * self.matched.width];
        let mut sum = Complex::ZERO;
        for (i, tap) in row.iter().enumerate() {
            sum += self.baseband[offset + i] * *tap;
        }
        Some(sum)
    }

    /// Where the watch's half `index` is, in samples of the select filter's
    /// output.
    fn grid_time(&self, index: u64) -> f64 {
        self.grid_start + index as f64 * self.sps / 2.0
    }

    /// A time in samples of the select filter's output as a sample of this
    /// receiver's input: the filter delays everything by half its length.
    fn line_time(&self, time: f64) -> u64 {
        (time - self.select.delay()).max(0.0).round() as u64
    }

    fn on_seen(&mut self, seen: Seen) {
        match seen {
            Seen::Carrier(on) => {
                let at = self.line_time(self.grid_time(self.watch_now()));
                self.heard.push_back(Heard::Carrier { on, at });
            }
            Seen::Began { hearing, since, turn } => {
                let at = self.line_time(self.grid_time(since));
                match hearing {
                    Hearing::Tone | Hearing::Ac | Hearing::Sh => {
                        self.heard.push_back(match hearing {
                            Hearing::Tone => Heard::Tone { at },
                            Hearing::Ac => Heard::Ac { at },
                            _ => Heard::Sh { at },
                        });
                        // Whatever the far end was sending, it has stopped:
                        // these are what a modem sends to start again.
                        self.lose();
                    }
                    Hearing::Pph(_) => {
                        // Look for all of it once it could all be here.
                        let heard = self.grid_time(self.watch_now());
                        let (from, to) = (heard + PPH_FROM * self.sps, heard + PPH_TO * self.sps);
                        let due = to + PPH_SYMBOLS as f64 * self.sps;
                        self.pph.get_or_insert(Search { from, to, due, turn });
                    }
                    Hearing::Nothing | Hearing::Modulated => {}
                }
            }
            Seen::Reversal { at, turn } => {
                let reversal = self.grid_time(at);
                let (from, to) = (reversal + SH_FROM * self.sps, reversal + SH_TO * self.sps);
                let due = to + KNOWN as f64 * self.sps;
                self.sh = Some(Search { from, to, due, turn });
            }
        }
    }

    /// The watch's newest half.
    fn watch_now(&self) -> u64 {
        ((self.grid_next - self.grid_start) / (self.sps / 2.0)).round() as u64 - 1
    }

    /// Whatever the receiver is working on beside the core: sequences heard
    /// looked for once all of them could be in, and the symbols after one
    /// found decided.
    fn work(&mut self) {
        if let Some(search) = self.pph
            && self.matched_at(search.due).is_some()
        {
            self.pph = None;
            let readings = [Reading::WithI, Reading::AsPrinted].map(Reference::Pph);
            if let Some(found) = self.find(search, &readings)
                && let Reference::Pph(reading) = found.reference
            {
                let began = self.line_time(found.start - 0.5 * self.sps);
                let ended = self.line_time(found.start + (PPH_SYMBOLS as f64 - 0.5) * self.sps);
                self.heard.push_back(Heard::Pph { reading, began, ended });
                self.begin(found);
            }
        }
        if let Some(search) = self.sh
            && self.matched_at(search.due).is_some()
        {
            self.sh = None;
            if let Some(found) = self.find(search, &[Reference::Sh]) {
                let at = self.line_time(found.start + (SH_SYMBOLS as f64 - 0.5) * self.sps);
                self.heard.push_back(Heard::Reversal { at });
                self.begin(found);
            }
        }
        if matches!(self.work, Work::Deciding(_))
            && let Work::Deciding(mut deciding) = std::mem::replace(&mut self.work, Work::Idle)
        {
            if self.decide(&mut deciding) {
                self.train(*deciding);
            } else {
                self.work = Work::Deciding(deciding);
            }
        }
    }

    /// The best place for the first symbol of any of `references` that
    /// `search` allows, and what it says of the carrier and the line, if the
    /// sequence is really there.
    fn find(&self, search: Search, references: &[Reference]) -> Option<Found> {
        let Search { from, to, turn, .. } = search;
        let step = self.sps / STEPS as f64;
        // The matched filter's output an eighth of a symbol apart, from the
        // earliest start to the latest start's last symbol.
        let earliest = (self.first as f64 + self.matched.reach as f64 + 1.0).max(from);
        if to < earliest {
            return None;
        }
        let count = ((to - earliest) / step).floor() as usize + 1 + STEPS * (KNOWN - 1);
        let outputs: Vec<Complex> = (0..count).map_while(|j| self.matched_at(earliest + j as f64 * step)).collect();
        if outputs.len() < STEPS * (KNOWN - 1) + 1 {
            return None;
        }
        let middle = (KNOWN as f64 - 1.0) / 2.0;
        let mut best: Option<(f64, usize, Reference)> = None;
        let mut scores = Vec::new();
        for &reference in references {
            let known = known(reference);
            let turned: Vec<Complex> =
                (0..KNOWN).map(|k| known[k].conj() * Complex::from_polar(1.0, -turn * (k as f64 - middle))).collect();
            let row: Vec<f64> = (0..outputs.len() - STEPS * (KNOWN - 1))
                .map(|j| (0..KNOWN).fold(Complex::ZERO, |sum, k| sum + outputs[j + STEPS * k] * turned[k]).norm_sqr())
                .collect();
            for (j, &score) in row.iter().enumerate() {
                if best.is_none_or(|b| score > b.0) {
                    best = Some((score, j, reference));
                }
            }
            scores.push((reference, row));
        }
        let (_, j, reference) = best?;
        let row = &scores.iter().find(|s| s.0 == reference)?.1;
        // Between the steps, by the parabola through the best and its
        // neighbours.
        let mut offset = 0.0;
        if j > 0 && j + 1 < row.len() {
            let (early, peak, late) = (row[j - 1], row[j], row[j + 1]);
            let curve = early - 2.0 * peak + late;
            if curve < 0.0 {
                offset = (0.5 * (early - late) / curve).clamp(-0.5, 0.5);
            }
        }
        let start = earliest + (j as f64 + offset) * step;
        let symbols: Vec<Complex> = (0..KNOWN).map(|k| self.matched_at(start + k as f64 * self.sps)).collect::<Option<_>>()?;
        let known = known(reference);
        // The turn, left over from the watch's: from the first half of the
        // sequence against the second.
        let lean = |range: std::ops::Range<usize>, turn: f64| {
            range.fold(Complex::ZERO, |sum, k| {
                sum + symbols[k] * known[k].conj() * Complex::from_polar(1.0, -turn * (k as f64 - middle))
            })
        };
        let half = KNOWN / 2;
        let turn = turn + (lean(half..KNOWN, turn) * lean(0..half, turn).conj()).arg() / half as f64;
        let whole = lean(0..KNOWN, turn);
        let energy: f64 = symbols.iter().map(|s| s.norm_sqr()).sum();
        let power: f64 = known.iter().map(|k| k.norm_sqr()).sum();
        if energy <= 0.0 || whole.abs() / (energy * power).sqrt() < MATCHED {
            return None;
        }
        Some(Found { reference, start, phase: whole.arg(), turn, gain: whole.abs() / power })
    }

    /// A sequence found: the receiver reads what follows it from here.
    fn begin(&mut self, found: Found) {
        // Whatever the core was following has been replaced.
        self.core.idle();
        self.reference = Some(found.reference);
        self.phase = Phase::Sync;
        self.size = Size::Four;
        // ALT starts the far scrambler at zero and, as this modem sends it,
        // the differential encoder at Z = 0 (10.2.4.2; `plan.md` 8.4).
        self.reader = Reader::new(scrambler_of(far(self.side)));
        self.ones = 0;
        self.e_seen = false;
        self.lost_since = None;
        let targets = known(found.reference).to_vec();
        self.work = Work::Deciding(Box::new(Deciding { found, targets, correction: 0.0, end: WINDOW_END }));
    }

    /// Decide every symbol after the sequence that has come, on its
    /// references. True once the window is full.
    fn decide(&mut self, deciding: &mut Deciding) -> bool {
        let found = deciding.found;
        let middle = (KNOWN as f64 - 1.0) / 2.0;
        while deciding.targets.len() < deciding.end {
            let k = deciding.targets.len();
            let Some(y) = self.matched_at(found.start + k as f64 * self.sps) else { return false };
            let phase = found.phase + found.turn * (k as f64 - middle) + deciding.correction;
            let z = y * Complex::from_polar(1.0 / found.gain, -phase);
            let point = (if z.re < 0.0 { -1 } else { 1 }, if z.im < 0.0 { -1 } else { 1 });
            let target = unit_point(point, Size::Four);
            deciding.correction += DECIDING_GAIN * (z * target.conj()).arg();
            deciding.targets.push(target);
            self.last = z;
            let e = self.absorb(point, Size::Four, found.start + (k as f64 + 0.5) * self.sps);
            if e && self.rate == Rate::R2400 {
                // Sixteen points next: the window ends with E.
                deciding.end = deciding.targets.len();
            }
        }
        true
    }

    /// Train the core on everything known and decided.
    fn train(&mut self, deciding: Deciding) {
        let Deciding { found, targets, .. } = deciding;
        let Some(start) = self.half_at(found.start) else {
            self.heard.push_back(Heard::Untrained { on: found.reference });
            self.stop();
            return;
        };
        let n = targets.len();
        self.core.train(Training {
            targets,
            start,
            first: Window { align: (0, n), solve: (0, n), search: TRAINING_SEARCH },
            retry: None,
            turn: Some(found.turn),
            drift: None,
            accept_db: ACCEPT_DB,
            slicer: tables().slicer(self.size),
            fallback: true,
        });
        self.work = Work::Training(found.reference);
    }

    /// One symbol's grid point, as bits: differential code undone,
    /// descrambled, and handed up; E looked for. `end` is where the symbol
    /// ends, in samples of the select filter's output. True if it ended E.
    fn absorb(&mut self, point: Point, size: Size, end: f64) -> bool {
        let bits = self.reader.differential(point, size);
        let mut e = false;
        for bit in bits {
            match self.phase {
                Phase::Sync => {
                    self.sync_bits.push(bit);
                    // mp::Finder's rule: twenty ones in a row. ALT
                    // descrambles to alternations and an MPh's longest run
                    // is its seventeen-one sync, so nothing before E comes
                    // near; and a symbol's second bit is taken with its
                    // first, so a run begun on ALT's last 1 still ends where
                    // E does.
                    self.ones = if bit { self.ones + 1 } else { 0 };
                    if self.ones == E_BITS {
                        e = true;
                    }
                }
                Phase::Data => {
                    if self.watch.carrier() {
                        self.data_bits.push(bit);
                    }
                }
                Phase::Hunting => {}
            }
        }
        if e {
            self.phase = Phase::Data;
            self.e_seen = true;
            let at = self.line_time(end);
            self.heard.push_back(Heard::E { at });
            if self.size != self.rate.size() {
                self.size = self.rate.size();
                self.core.set_slicer(tables().slicer(self.size));
            }
        }
        e
    }

    /// Everything the core has to say, and every symbol it has made.
    fn track(&mut self) {
        while let Some(heard) = self.core.heard() {
            match heard {
                CoreHeard::Trained { snr_db, .. } => {
                    if let Work::Training(on) = self.work {
                        self.work = Work::Idle;
                        self.heard.push_back(Heard::Trained { on, snr_db });
                    }
                }
                CoreHeard::Untrained => {
                    if let Work::Training(on) = self.work {
                        self.heard.push_back(Heard::Untrained { on });
                        self.stop();
                    }
                }
                _ => {}
            }
        }
        if self.phase == Phase::Hunting || !matches!(self.work, Work::Idle) {
            return;
        }
        while let Some(point) = self.core.next() {
            let Some(symbol) = self.core.settle(point.nearest) else { break };
            self.last = symbol.point;
            let grid = tables().point(self.size, symbol.label.unwrap_or(0));
            // The symbol just settled is centred two halves before the next.
            let end = self.half_time(self.core.next_half().saturating_sub(1)).unwrap_or(self.taken as f64);
            self.absorb(grid, self.size, end);
        }
    }

    /// Keep where the core's newest halves are centred, so that a place the
    /// matched filter found can be named as one of them.
    fn note_halves(&mut self, index: u64) {
        let made = self.core.halves();
        if made < self.noted {
            // A resync read the newest again on a moved grid.
            while self.halves.back().is_some_and(|&(h, _)| h >= made) {
                self.halves.pop_back();
            }
            self.noted = made;
        }
        for h in self.noted..made {
            self.halves.push_back((h, index as f64 - HALF_LAG));
        }
        self.noted = made;
        while self.halves.len() > 8192 {
            self.halves.pop_front();
        }
    }

    /// The core's half centred nearest `time`, if it has one within half a
    /// half of it, and a sample for where each half was noted.
    fn half_at(&self, time: f64) -> Option<u64> {
        let after = self.halves.partition_point(|&(_, t)| t < time);
        let near = |i: usize| self.halves.get(i).map(|&(h, t)| ((t - time).abs(), h));
        let nearest = match (after.checked_sub(1).and_then(near), near(after)) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (a, b) => a.or(b),
        }?;
        (nearest.0 <= self.sps / 4.0 + 1.0).then_some(nearest.1)
    }

    /// Where the core's half `half` is centred.
    fn half_time(&self, half: u64) -> Option<f64> {
        let first = self.halves.front()?.0;
        self.halves.get(half.checked_sub(first)? as usize).map(|&(_, t)| t)
    }

    /// Drop the far end once it has plainly gone.
    fn supervise(&mut self) {
        let now = self.taken;
        self.off_since = if self.watch.carrier() { None } else { self.off_since.or(Some(now)) };
        let lost = matches!(self.work, Work::Idle) && self.core.is_lost();
        self.lost_since = if lost { self.lost_since.or(Some(now)) } else { None };
        if self.phase == Phase::Hunting && matches!(self.work, Work::Idle) {
            return;
        }
        let too_long = |since: Option<u64>, seconds: f64| since.is_some_and(|s| (now - s) as f64 > seconds * self.fs);
        if too_long(self.off_since, OFF_SECONDS) || too_long(self.lost_since, LOST_SECONDS) {
            self.lose();
        }
    }

    /// The far end has stopped: say so, and hunt again.
    fn lose(&mut self) {
        if self.phase == Phase::Hunting && matches!(self.work, Work::Idle) {
            return;
        }
        self.stop();
        let at = self.line_time(self.grid_time(self.watch_now()));
        self.heard.push_back(Heard::Lost { at });
    }
}

/// The 32 symbols a training starts from, at unit power.
fn known(reference: Reference) -> [Complex; KNOWN] {
    match reference {
        Reference::Pph(reading) => std::array::from_fn(|i| reading.point(i)),
        Reference::Sh => std::array::from_fn(|i| if i < SH_SYMBOLS { sh(i) } else { sh_bar(i - SH_SYMBOLS) }),
    }
}

/// A core for the far carrier at 600 baud, with every fix the core has.
fn new_core(fs: f64, carrier: f64) -> Core {
    Core::new(Band::new(fs, BAUD, carrier), Options::fixed(), tables().slicer(Size::Four))
}
