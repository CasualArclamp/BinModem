//! The primary channel of half-duplex V.34 (clause 12): the page, one way,
//! from the source to the recipient.
//!
//! Everything the channel carries is duplex V.34's, sent one way only. The
//! recipient chose the symbol rate, carrier, pre-emphasis and power reduction
//! in INFOh (Table 22), and the MPh exchange on the control channel settled
//! the data rate, the trellis code, the shaping and the non-linear encoding
//! (12.4). With those, every burst of the primary channel is 70 ms of
//! silence, S for 128T, S-bar for 16T and PP, and then either TRN -- phase 3,
//! where the recipient trains its equaliser (12.3) -- or B1, the page's bits
//! and 35 ms of scrambled ones to turn off (12.5). Between bursts the line is
//! the control channel's.
//!
//! The source is `qam::Transmitter` fed by a sequencer of `signals` and
//! `data::Encoder`. The recipient is `receiver::Receiver`, trained on
//! `Reference::PpThenTrnAt` in phase 3 and on `Reference::Pp` before every
//! page, `data::Decoder` from B1's first symbol, `data::Acquirer` after a
//! slip, and a carrier detector of 6.6.2's kind for the page's end -- which
//! nothing in the bits marks: T.30 sends its RCP and stops.

use std::collections::VecDeque;

use dsp::Complex;

use super::data::{Acquired, Acquirer, Decoder, Encoder, Params};
use super::frame::Framing;
use super::info::SymbolRate;
use super::mp::Coefficient;
use super::qam::{Band, Transmitter};
use super::receiver::{self, Heard, Receiver, Reference};
use super::signals::{self, Reader, Sender, Size};
use super::trellis::Code;
use crate::v32::Mode;

/// "Silence for 70 +/- 5 ms" before S, in phase 3 and before every page
/// (12.3.1.1, 12.5.1).
const SILENCE_SECONDS: f64 = 0.070;

/// The step INFOh counts TRN in (Table 22, bits 15 to 21), and the length of
/// the scrambled ones that turn a page off (12.5.3.1). A whole number of
/// symbols at every symbol rate: 84 at 2400, 120 at 3429.
const STEP_SECONDS: f64 = 0.035;

/// 2D symbols in a mapping frame: four 4D symbols (8.1).
const MAPPING_FRAME: usize = 8;

/// 12.3.3: "signal S is not detected within 2000 ms".
const S_WITHIN_SECONDS: f64 = 2.0;

/// Least share of TRN's descrambled bits that have to be ones for TRN to
/// count as "satisfactorily received" (12.3.3). Phase 3's training is not
/// kept -- every page trains again on its own PP -- so what TRN tells the
/// recipient is whether the line, at the rate INFOh asked for, carries the
/// constellation at all; one symbol in a hundred wrong at sixteen points is a
/// line below 20 dB, which no page rate worth choosing survives.
const TRN_ONES: f64 = 0.99;

/// Data mode's cost per 4D symbol past which the decoder has lost its place
/// in the frames, and how long it has to stay there before the frames are
/// searched for again: as training.rs judges it.
const STRAYED_COST: f64 = 1.0;
const STRAYED_SYMBOLS: usize = 1000;

/// Circuit 109's thresholds, against the page's own level (6.6.2): off 12 dB
/// under it, on again 10 dB under -- 6.6.2's 2 dB of hysteresis -- and off
/// only after 20 ms under.
const OFF_DB: f64 = -12.0;
const ON_DB: f64 = -10.0;
const OFF_SECONDS: f64 = 0.020;

/// How quickly the level is followed, and how slowly the page's own level
/// that the thresholds are set against. A millisecond sees the carrier's end
/// within three of the last symbol, so that with the hold circuit 109 goes off
/// inside 6.6.2's 20 to 25 ms; a tenth of a second follows a line whose gain
/// drifts and not a dip.
const LEVEL_SECONDS: f64 = 0.001;
const REFERENCE_SECONDS: f64 = 0.1;

/// What INFOh chose for the primary channel (Table 22): the band the source
/// is to send on, its pre-emphasis and power reduction, and phase 3's TRN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    pub band: Band,
    /// Pre-emphasis index, 0 to 10 (bits 23 to 26).
    pub pre_emphasis: u8,
    /// Power reduction in decibels, 0 to 7 (bits 12 to 14).
    pub reduction: u8,
    /// TRN's constellation, four or sixteen points (bit 30).
    pub trn_size: Size,
    /// TRN's length in steps of 35 ms, 0 to 127 (bits 15 to 21).
    pub trn_steps: u8,
}

impl Channel {
    /// Symbols in 35 ms at the band's rate.
    fn step(&self) -> usize {
        (STEP_SECONDS * self.band.baud()).round() as usize
    }

    /// Symbols of TRN phase 3 sends (12.3.1.2).
    pub fn trn_symbols(&self) -> usize {
        usize::from(self.trn_steps) * self.step()
    }

    /// Symbols of silence before S.
    fn silence_symbols(&self) -> usize {
        (SILENCE_SECONDS * self.band.baud()).round() as usize
    }

    /// Symbols of scrambled ones that turn a page off: 35 ms, made up to
    /// whole mapping frames, since a mapping frame is what the encoder takes
    /// its bits in and the ones begin with the first frame after the page's
    /// last bit. 12.5.3.1 says 35 ms and no more; a data frame would be 40 ms
    /// at four of the six symbol rates (8.1), and what the recipient needs of
    /// the ones is its decoder's 40 4D symbols of traceback carried past the
    /// last bit, which 35 ms covers at every rate.
    pub fn turn_off_symbols(&self) -> usize {
        self.step().div_ceil(MAPPING_FRAME) * MAPPING_FRAME
    }
}

/// Data mode as the MPh exchange settled it (12.4.1.3, Tables 23 and 24):
/// the rate, and what the recipient's MPh asked of the source's encoder. The
/// precoding coefficients are carried for completeness: the recipient's
/// equaliser is a full linear one, so its MPh asks for none, and they are
/// zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataMode {
    /// Primary channel bit/s, a multiple of 2400 (bits 20 to 23, and the
    /// rates enabled in bits 35 to 49).
    pub rate: u32,
    /// The trellis code (bits 29 and 30).
    pub code: Code,
    /// Non-linear encoding (bit 31).
    pub nonlinear: bool,
    /// Expanded rather than minimum shaping (bit 32).
    pub expanded: bool,
    /// h(1), h(2) and h(3) (Type 1, bits 52 to 152), as MP carries them.
    pub precoding: [Coefficient; 3],
}

impl DataMode {
    /// The encoder's and the decoder's parameters at symbol rate `rate`, for
    /// a source whose scrambler is `source`'s (clause 7); None if Table 8 has
    /// no such data rate at that symbol rate. There is no auxiliary channel:
    /// MPh's bit 28 is reserved.
    pub fn params(&self, rate: SymbolRate, source: Mode) -> Option<Params> {
        Some(Params {
            framing: Framing::new(rate, self.rate, false, self.expanded)?,
            code: self.code,
            nonlinear: self.nonlinear,
            precoding: self.precoding,
            mode: source,
        })
    }
}

/// What the source is sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sending {
    /// Nothing: between bursts the line is the control channel's.
    Idle,
    /// The 70 ms before S.
    Silence,
    S,
    SBar,
    Pp,
    /// Phase 3's TRN (12.3.1.2).
    Trn,
    /// A page's B1: one data frame of scrambled ones (10.1.3.1).
    B1,
    /// The page's bits, circuit 106 on (12.5.1).
    Data,
    /// The 35 ms of scrambled ones that end a page (12.5.3.1).
    TurningOff,
    /// Nothing more to send, and the pulse of the last symbols still going
    /// out.
    Flushing,
}

/// A point of the four- or sixteen-point constellation at unit mean power.
fn grid(point: super::constellation::Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(receiver::unit(size))
}

/// The source's symbols, one at a time, as the transmitter's pulse asks for
/// them.
#[derive(Debug, Clone)]
struct Symbols {
    channel: Channel,
    mode: Mode,
    sender: Sender,
    segment: Sending,
    /// Symbols of the segment given so far.
    count: usize,
    /// A page's encoder; none in phase 3.
    encoder: Option<Encoder>,
    /// The page's bits waiting to go.
    bits: VecDeque<bool>,
    /// Told the page is over: circuit 105 off, and the turn-off once the bits
    /// have gone.
    ending: bool,
    /// Symbols of nothing that carry the last symbol's pulse out.
    flush: usize,
}

impl Symbols {
    fn new(channel: Channel, mode: Mode) -> Self {
        Self {
            channel,
            mode,
            sender: Sender::new(mode),
            segment: Sending::Idle,
            count: 0,
            encoder: None,
            bits: VecDeque::new(),
            ending: false,
            flush: 2 * Transmitter::lookahead() + 2,
        }
    }

    fn start(&mut self, segment: Sending) {
        self.segment = segment;
        self.count = 0;
        if segment == Sending::Trn {
            // "The scrambler is initialized to zero prior to transmission of
            // the TRN signal" (10.1.3.8).
            self.sender.restart();
        }
    }

    /// Begin a burst: phase 3's without an encoder, a page's with one.
    fn begin(&mut self, encoder: Option<Encoder>) {
        self.encoder = encoder;
        self.bits.clear();
        self.ending = false;
        self.start(Sending::Silence);
    }

    fn next(&mut self) -> Complex {
        loop {
            let count = self.count;
            match self.segment {
                Sending::Idle => return Complex::ZERO,
                Sending::Silence => {
                    if count == self.channel.silence_symbols() {
                        self.start(Sending::S);
                        continue;
                    }
                    self.count += 1;
                    return Complex::ZERO;
                }
                Sending::S => {
                    if count == signals::S_SYMBOLS {
                        self.start(Sending::SBar);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s(count), Size::Four);
                }
                Sending::SBar => {
                    if count == signals::S_BAR_SYMBOLS {
                        self.start(Sending::Pp);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s_bar(count), Size::Four);
                }
                Sending::Pp => {
                    if count == signals::PP_SYMBOLS {
                        // "After transmitting signal PP, the source modem
                        // shall transmit signal TRN" (12.3.1.2); before a
                        // page, "PP followed by sequence B1" (12.5.1).
                        let next = if self.encoder.is_some() {
                            Sending::B1
                        } else if self.channel.trn_symbols() > 0 {
                            Sending::Trn
                        } else {
                            Sending::Flushing
                        };
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return signals::pp(count).into();
                }
                Sending::Trn => {
                    if count == self.channel.trn_symbols() {
                        self.start(Sending::Flushing);
                        continue;
                    }
                    self.count += 1;
                    let size = self.channel.trn_size;
                    return grid(self.sender.trn(size), size);
                }
                Sending::B1 => {
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Sending::Flushing);
                        continue;
                    };
                    if count == encoder.params().framing.symbols_per_data_frame() {
                        self.start(Sending::Data);
                        continue;
                    }
                    // "One data frame of scrambled binary ones" (10.1.3.1):
                    // the encoder's first frame takes ones whatever is
                    // waiting, and its scrambler, trellis and precoder began
                    // at zero with it.
                    self.count += 1;
                    return encoder.next_symbol(&mut || true);
                }
                Sending::Data => {
                    // The turn-off begins at a mapping frame, the unit the
                    // encoder takes bits in, once the page's last bit has been
                    // taken into one.
                    if count.is_multiple_of(MAPPING_FRAME) && self.ending && self.bits.is_empty() {
                        self.start(Sending::TurningOff);
                        continue;
                    }
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Sending::Flushing);
                        continue;
                    };
                    self.count += 1;
                    // Nothing to send yet is ones, HDLC's idle, as it is in
                    // duplex data mode.
                    let bits = &mut self.bits;
                    return encoder.next_symbol(&mut || bits.pop_front().unwrap_or(true));
                }
                Sending::TurningOff => {
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Sending::Flushing);
                        continue;
                    };
                    if count == self.channel.turn_off_symbols() {
                        self.start(Sending::Flushing);
                        continue;
                    }
                    self.count += 1;
                    return encoder.next_symbol(&mut || true);
                }
                Sending::Flushing => {
                    if count == self.flush {
                        self.encoder = None;
                        self.start(Sending::Idle);
                        continue;
                    }
                    self.count += 1;
                    return Complex::ZERO;
                }
            }
        }
    }
}

/// The source end of the primary channel: phase 3 (12.3.1) and each page's
/// burst (12.5.1, 12.5.3.1), as line samples.
///
/// One transmitter for the whole call, built from INFOh's choices, and
/// silent -- giving zero samples -- between bursts, so that whoever sums
/// this with the control channel's samples can do so all the time.
#[derive(Debug, Clone)]
pub struct Source {
    tx: Transmitter,
    symbols: Symbols,
}

impl Source {
    /// A source sending on `channel`, whose own scrambler is `mode`'s.
    pub fn new(channel: Channel, mode: Mode, fs: f64) -> Self {
        Self {
            tx: Transmitter::new(channel.band, channel.pre_emphasis, channel.reduction, fs),
            symbols: Symbols::new(channel, mode),
        }
    }

    pub fn channel(&self) -> Channel {
        self.symbols.channel
    }

    pub fn sending(&self) -> Sending {
        self.symbols.segment
    }

    /// Whether anything is on the line, or about to be.
    pub fn is_sending(&self) -> bool {
        self.symbols.segment != Sending::Idle
    }

    /// Begin phase 3, INFOh having arrived (12.3.1): the 70 ms of silence,
    /// S, S-bar, PP and TRN as INFOh asked. Cuts short whatever was going.
    pub fn phase3(&mut self) {
        self.symbols.begin(None);
    }

    /// Begin a page's burst (12.5.1): the 70 ms of silence, S, S-bar, PP, B1
    /// and then the bits pushed here, ones while there are none. False, and
    /// nothing begun, if the symbol rate has no such data rate in Table 8.
    /// Cuts short whatever was going.
    pub fn page(&mut self, data: DataMode) -> bool {
        let Some(params) = data.params(self.symbols.channel.band.rate, self.symbols.mode) else { return false };
        self.symbols.begin(Some(Encoder::new(params)));
        true
    }

    /// Bits for the page, taken as the encoder needs them.
    pub fn push_bits(&mut self, bits: &[bool]) {
        self.symbols.bits.extend(bits);
    }

    /// Bits pushed and not yet taken.
    pub fn pending_bits(&self) -> usize {
        self.symbols.bits.len()
    }

    /// The page is over: circuit 105 off (12.5.3.1). What is pushed still
    /// goes, then 35 ms of scrambled ones, then nothing.
    pub fn end_page(&mut self) {
        self.symbols.ending = true;
    }

    /// Symbols asked of the sequencer so far, the pulse's lookahead included.
    pub fn symbols(&self) -> u64 {
        self.tx.symbols()
    }

    pub fn next_sample(&mut self) -> f64 {
        let symbols = &mut self.symbols;
        self.tx.next_sample(|| symbols.next())
    }
}

/// What the recipient reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// S, S-bar and PP heard and trained on -- phase 3's (12.3.2.2) or a
    /// page's (12.5.2) -- to this signal to noise.
    Trained { snr_db: f64 },
    /// Phase 3 is over: TRN has run for the length INFOh asked (12.3.2.3), or
    /// phase 3 failed. `well` is false when 12.3.3's recovery is due -- no S
    /// within 2000 ms, PP that did not train, or TRN that did not descramble
    /// to ones -- and the recipient is to go back to its phase 2 tone.
    Phase3Over { well: bool },
    /// B1 has been read (12.5.2): circuit 109 on, and the bits from here on
    /// are the page's.
    PageStarted { b1_errors: usize },
    /// The source's carrier has gone (12.5.3.2, 6.6.2): circuit 109 off, and
    /// the last of the page's bits are here.
    PageEnded,
}

/// What the recipient is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    /// Listening for S and S-bar.
    Hunting,
    /// Solving for the equaliser over PP.
    Training,
    /// Following phase 3's TRN.
    Trn,
    /// Counting a page's B1.
    B1,
    /// Receiving a page.
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    /// Phase 3: hunting for S, with 12.3.3's 2000 ms up at `deadline`
    /// samples.
    Hunting3 { deadline: u64 },
    Training3 { deadline: u64 },
    /// Phase 3's TRN, over at half-symbol sample `end`.
    Trn { end: u64 },
    HuntingPage,
    TrainingPage,
    /// B1, and the page after it.
    Page,
}

/// Circuit 109 for the primary channel (6.6.2, 12.5.3.2): whether the
/// source's carrier is there.
///
/// 6.6.2 puts the thresholds in dBm at the line: on above -43, off below
/// -48, and off 20 to 25 ms after the level falls. Nothing here knows the
/// line's level in dBm -- a sound card into a VoIP call has whatever gain it
/// has -- so the thresholds are set against the signal itself: the page's own
/// level, learned as it is read, with off 12 dB under it and on again 10 dB
/// under. The lines V.34 runs over here leave their noise 30 dB and more
/// under the signal, and a VoIP call's level does not fade; what falls 12 dB
/// for 20 ms is a carrier that has stopped. The receiver's own `level` is a
/// slow average, a quarter of a second behind, and no use for this.
///
/// The level is the samples' power, smoothed over a millisecond. The
/// transmit pulse dies away within a couple of symbols of the last one, so
/// the fall is seen inside three milliseconds, and the hold makes up the rest
/// of 6.6.2's 20 to 25.
#[derive(Debug, Clone)]
struct Detector {
    level: f64,
    reference: f64,
    quick: f64,
    slow: f64,
    /// Samples the level has been under the off threshold, and how many turn
    /// 109 off.
    under: usize,
    hold: usize,
    on: bool,
    /// Whether a page is being read, and so the level judged.
    armed: bool,
}

impl Detector {
    fn new(fs: f64) -> Self {
        Self {
            level: 0.0,
            reference: 0.0,
            quick: 1.0 / (LEVEL_SECONDS * fs),
            slow: 1.0 / (REFERENCE_SECONDS * fs),
            under: 0,
            hold: (OFF_SECONDS * fs).round() as usize,
            on: false,
            armed: false,
        }
    }

    fn feed(&mut self, sample: f64) {
        self.level += self.quick * (sample * sample - self.level);
        if !self.armed {
            return;
        }
        let off = self.reference * 10f64.powf(OFF_DB / 10.0);
        let on = self.reference * 10f64.powf(ON_DB / 10.0);
        if self.level > on {
            // The page's own level, followed while it is plainly there and
            // never dragged down by a dip or by the end.
            self.reference += self.slow * (self.level - self.reference);
            self.under = 0;
            self.on = true;
        } else if self.level < off {
            self.under += 1;
            if self.under >= self.hold {
                self.on = false;
            }
        } else {
            // Between the two thresholds 6.6.2 leaves 109 as it is.
            self.under = 0;
        }
    }

    /// A page has trained: judge its carrier from here, against the level it
    /// has now.
    fn arm(&mut self) {
        self.reference = self.level;
        self.under = 0;
        self.on = true;
        self.armed = true;
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.on = false;
    }

    /// Whether a page's carrier, once there, has gone.
    fn gone(&self) -> bool {
        self.armed && !self.on
    }
}

/// The recipient end of the primary channel: phase 3 (12.3.2, 12.3.3) and
/// each page (12.5.2, 12.5.3.2).
#[derive(Debug, Clone)]
pub struct Recipient {
    rx: Receiver,
    channel: Channel,
    /// The source's scrambler.
    far: Mode,
    fs: f64,
    stage: Stage,
    events: VecDeque<Event>,
    /// Samples taken.
    now: u64,
    /// Phase 3's TRN: where it ends, in half-symbol samples, once S-bar has
    /// placed it; the reader that descrambles it; symbols let go while the
    /// descrambler fills; and its bits, and how many of them were ones.
    trn_end: u64,
    reader: Reader,
    grace: usize,
    trn_bits: usize,
    trn_ones: usize,
    /// The page: its data mode, its decoder or the search for its frames
    /// after a slip, B1's bits still to count and how many were wrong, and
    /// the page's bits not yet taken.
    params: Option<Params>,
    decoder: Option<Decoder>,
    acquirer: Option<Acquirer>,
    b1_left: usize,
    b1_errors: usize,
    bits: Vec<bool>,
    /// The receiver's slips as last seen, since one is a reason to search.
    slips_seen: u32,
    /// Symbols in a row the decoder's cost has said it is lost.
    strayed: usize,
    detector: Detector,
}

impl Recipient {
    /// A recipient of `channel` from a source whose scrambler is `far`'s.
    pub fn new(channel: Channel, far: Mode, fs: f64) -> Self {
        Self {
            rx: Receiver::new(channel.band, fs),
            channel,
            far,
            fs,
            stage: Stage::Idle,
            events: VecDeque::new(),
            now: 0,
            trn_end: 0,
            reader: Reader::new(far),
            grace: 0,
            trn_bits: 0,
            trn_ones: 0,
            params: None,
            decoder: None,
            acquirer: None,
            b1_left: 0,
            b1_errors: 0,
            bits: Vec::new(),
            slips_seen: 0,
            strayed: 0,
            detector: Detector::new(fs),
        }
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    pub fn state(&self) -> State {
        match self.stage {
            Stage::Idle => State::Idle,
            Stage::Hunting3 { .. } | Stage::HuntingPage => State::Hunting,
            Stage::Training3 { .. } | Stage::TrainingPage => State::Training,
            Stage::Trn { .. } => State::Trn,
            Stage::Page if self.b1_left > 0 => State::B1,
            Stage::Page => State::Data,
        }
    }

    /// INFOh has gone: listen for phase 3 (12.3.2.1). Phase 3 is over, one
    /// way or the other, with `Event::Phase3Over`.
    pub fn expect_phase3(&mut self) {
        self.leave();
        self.rx.hunt();
        self.stage = Stage::Hunting3 { deadline: self.now + (S_WITHIN_SECONDS * self.fs) as u64 };
    }

    /// This end has gone quiet on the control channel: listen for a page in
    /// `data` mode (12.5.2). False, and nothing begun, if the symbol rate has
    /// no such data rate in Table 8. The page comes as `Event::Trained`,
    /// `Event::PageStarted`, bits through `take_bits`, and `Event::PageEnded`.
    pub fn expect_page(&mut self, data: DataMode) -> bool {
        let Some(params) = data.params(self.channel.band.rate, self.far) else { return false };
        self.leave();
        self.params = Some(params);
        self.rx.hunt();
        self.stage = Stage::HuntingPage;
        true
    }

    /// Stop listening to the primary channel: this end's own circuit 105 has
    /// come on (12.5.3.2), or the call is over.
    pub fn stop(&mut self) {
        self.leave();
        self.rx.idle();
        self.stage = Stage::Idle;
    }

    /// Leave whatever was being followed, keeping the bits.
    fn leave(&mut self) {
        self.decoder = None;
        self.acquirer = None;
        self.params = None;
        self.b1_left = 0;
        self.b1_errors = 0;
        self.strayed = 0;
        self.slips_seen = self.rx.slips();
        self.detector.disarm();
    }

    /// The next thing to report, if there is one.
    pub fn event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// The page's bits decoded so far, taken: those after B1, in order.
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    /// Circuit 109: whether a page's carrier is being received.
    pub fn carrier(&self) -> bool {
        self.detector.armed && self.detector.on
    }

    /// The received level, in decibels against a unit-power signal at the
    /// line's nominal level.
    pub fn level_db(&self) -> f64 {
        10.0 * (self.detector.level / 0.5).max(1e-12).log10()
    }

    /// Signal to noise of the receiver's decisions, in decibels.
    pub fn snr_db(&self) -> f64 {
        self.rx.snr_db()
    }

    /// What the last training left, in decibels.
    pub fn trained_snr_db(&self) -> f64 {
        self.rx.trained_snr_db()
    }

    /// Slips the receiver has found and followed.
    pub fn slips(&self) -> u32 {
        self.rx.slips()
    }

    /// B1's bits that were not ones, in the page being read or last read.
    pub fn b1_errors(&self) -> usize {
        self.b1_errors
    }

    /// The share of phase 3's TRN bits that descrambled to ones, once the
    /// descrambler had filled; one when TRN had no length to judge.
    pub fn trn_share(&self) -> f64 {
        if self.trn_bits == 0 { 1.0 } else { self.trn_ones as f64 / self.trn_bits as f64 }
    }

    /// The last symbol equalised, while trained.
    pub fn last_point(&self) -> Option<Complex> {
        self.rx.last_point()
    }

    /// The far clock's rate against this end's, in parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.rx.drift_ppm()
    }

    pub fn feed(&mut self, sample: f64) {
        self.now += 1;
        self.detector.feed(sample);
        self.rx.feed(sample);
        while let Some(heard) = self.rx.heard() {
            self.heard(heard);
        }
        match self.stage {
            Stage::Hunting3 { deadline } if self.now >= deadline => self.finish_phase3(false),
            Stage::Trn { end } if self.rx.halves() >= end => {
                // 12.3.2.3: TRN is over by the count, and the control channel
                // is next.
                let well = self.trn_share() >= TRN_ONES;
                self.finish_phase3(well);
            }
            Stage::Page if self.detector.gone() => {
                // 12.5.3.2: "If the received signal level falls below the
                // turn-off threshold as defined in 6.6.2, then the modem shall
                // turn OFF Circuit 109 and clamp Circuit 104."
                self.leave();
                self.rx.idle();
                self.stage = Stage::Idle;
                self.events.push_back(Event::PageEnded);
            }
            _ => {}
        }
    }

    fn heard(&mut self, heard: Heard) {
        match (self.stage, heard) {
            (Stage::Hunting3 { deadline }, Heard::Reversal { at }) => {
                // 12.3.2.2: "the modem conditions its receiver to begin
                // training its main channel equalizer using signal PP", and
                // TRN follows at INFOh's size for INFOh's length.
                self.rx.train(Reference::PpThenTrnAt(self.channel.trn_size), self.far, at);
                let after_s_bar = signals::S_BAR_SYMBOLS + signals::PP_SYMBOLS + self.channel.trn_symbols();
                self.trn_end = at + 2 * after_s_bar as u64;
                self.stage = Stage::Training3 { deadline };
            }
            (Stage::HuntingPage, Heard::Reversal { at }) => {
                // 12.5.2: "resynchronize its receiver using signal PP". The
                // data grid may be set any time before training is done, and
                // B1's first symbol is decided against it.
                self.rx.train(Reference::Pp, self.far, at);
                if let Some(params) = self.params {
                    let decoder = Decoder::new(params);
                    self.rx.set_grid(decoder.grid_scale(), decoder.extent());
                    self.decoder = Some(decoder);
                }
                self.stage = Stage::TrainingPage;
            }
            (Stage::Training3 { .. }, Heard::Trained { snr_db }) => {
                self.events.push_back(Event::Trained { snr_db });
                // TRN from its first symbol -- or from its 512th, after a
                // slip in PP sent training to its second try -- descrambled
                // with the source's polynomial. The descrambler fills on 23
                // bits.
                self.reader = Reader::new(self.far);
                self.grace = 24 / self.channel.trn_size.bits() + 1;
                self.trn_bits = 0;
                self.trn_ones = 0;
                self.stage = Stage::Trn { end: self.trn_end };
                if self.rx.halves() >= self.trn_end {
                    // TRN of no length at all: over before training was.
                    self.finish_phase3(true);
                }
            }
            (Stage::TrainingPage, Heard::Trained { snr_db }) => {
                self.events.push_back(Event::Trained { snr_db });
                if let Some(params) = self.params {
                    self.b1_left = params.framing.n;
                }
                self.b1_errors = 0;
                self.strayed = 0;
                self.slips_seen = self.rx.slips();
                self.detector.arm();
                self.stage = Stage::Page;
            }
            (Stage::Training3 { deadline }, Heard::Untrained) => {
                // Not phase 3's S after all, or its PP spoilt. Listen on while
                // 12.3.3's 2000 ms last; the receiver went idle on this.
                if self.now >= deadline {
                    self.finish_phase3(false);
                } else {
                    self.rx.hunt();
                    self.stage = Stage::Hunting3 { deadline };
                }
            }
            (Stage::TrainingPage, Heard::Untrained) => {
                // Not a page's S-bar, or a page's spoilt: the receiver is
                // hunting again already, through what it kept from just after
                // that S-bar, so calling `hunt` would lose an S it has heard.
                self.decoder = None;
                self.stage = Stage::HuntingPage;
            }
            (Stage::Trn { .. }, Heard::Symbol(symbol)) => {
                let bits = self.reader.trn(symbol.decided, self.channel.trn_size);
                if self.grace > 0 {
                    self.grace -= 1;
                } else {
                    self.trn_bits += bits.len();
                    self.trn_ones += bits.iter().filter(|b| **b).count();
                }
            }
            (Stage::Page, Heard::Symbol(symbol)) => self.page_symbol(symbol),
            _ => {}
        }
    }

    /// Phase 3 is over, well or not, and the primary channel is quiet until
    /// a page is expected.
    fn finish_phase3(&mut self, well: bool) {
        self.rx.idle();
        self.stage = Stage::Idle;
        self.events.push_back(Event::Phase3Over { well });
    }

    /// A page's symbol: B1's, or the page's, or a search's.
    fn page_symbol(&mut self, symbol: receiver::Symbol) {
        // A slip loses or repeats symbols, and with them the place in the
        // frames.
        if self.rx.slips() != self.slips_seen {
            self.slips_seen = self.rx.slips();
            self.search();
        }
        if let Some(acquirer) = self.acquirer.as_mut() {
            match acquirer.feed(symbol.point) {
                Acquired::Searching => {}
                Acquired::Found(decoder) => {
                    self.acquirer = None;
                    self.decoder = Some(*decoder);
                    if self.b1_left > 0 {
                        // The slip took B1 with it. B1 is ones, as is
                        // anything the source has to fill with, so nothing of
                        // the page is in what was lost.
                        self.b1_left = 0;
                        self.events.push_back(Event::PageStarted { b1_errors: self.b1_errors });
                    }
                    self.take_decoded();
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
        }
    }

    /// Look for where the page's data frames are, in the symbols as they
    /// come, as training.rs does after a slip.
    fn search(&mut self) {
        let Some(params) = self.params else { return };
        let acquirer = Acquirer::new(params);
        self.rx.set_grid(acquirer.grid_scale(), acquirer.extent());
        self.decoder = None;
        self.acquirer = Some(acquirer);
        self.strayed = 0;
    }

    /// The decoder's bits to where they go: B1's ones counted, and the page's
    /// kept for taking.
    fn take_decoded(&mut self) {
        let Some(decoder) = self.decoder.as_mut() else { return };
        for bit in decoder.take_bits() {
            if self.b1_left > 0 {
                self.b1_left -= 1;
                self.b1_errors += usize::from(!bit);
                if self.b1_left == 0 {
                    // 12.5.2: "After receiving sequence B1, the modem shall
                    // unclamp Circuit 104, turn on Circuit 109, and begin
                    // receiving user data."
                    self.events.push_back(Event::PageStarted { b1_errors: self.b1_errors });
                }
            } else {
                self.bits.push(bit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::dpsk;

    const FS: f64 = 16_000.0;

    fn channel(rate: SymbolRate, high: bool, trn_size: Size, trn_steps: u8) -> Channel {
        Channel { band: Band::new(rate, high), pre_emphasis: 0, reduction: 0, trn_size, trn_steps }
    }

    fn data_mode(rate: u32, code: Code) -> DataMode {
        DataMode { rate, code, nonlinear: false, expanded: false, precoding: [(0, 0); 3] }
    }

    fn random_bits(count: usize, mut seed: u32) -> Vec<bool> {
        (0..count)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed & 1 == 1
            })
            .collect()
    }

    /// Samples a symbol at `band`'s rate.
    fn samples_per_symbol(band: Band) -> f64 {
        FS / band.baud()
    }

    /// Run the source until it is idle again, and then for a hundred samples
    /// more: its samples, and the sample at which each change of what it was
    /// sending was asked of it -- the first segment from the first symbol the
    /// pulse asked for, which comes a symbol after the call.
    fn burst(source: &mut Source) -> (Vec<f64>, Vec<(usize, Sending)>) {
        let mut out = Vec::new();
        let mut last = source.sending();
        let mut changes = Vec::new();
        let before = source.symbols();
        loop {
            out.push(source.next_sample());
            if changes.is_empty() && source.symbols() > before {
                changes.push((out.len(), last));
            }
            let now = source.sending();
            if now != last {
                changes.push((out.len(), now));
                last = now;
            }
            if now == Sending::Idle {
                out.extend((0..100).map(|_| source.next_sample()));
                return (out, changes);
            }
        }
    }

    /// How long each segment of a burst lasted, in symbols.
    fn segments(changes: &[(usize, Sending)], band: Band) -> Vec<(Sending, f64)> {
        changes
            .windows(2)
            .map(|w| (w[0].1, (w[1].0 - w[0].0) as f64 / samples_per_symbol(band)))
            .collect()
    }

    #[test]
    fn phase_3_is_70_ms_s_s_bar_pp_and_trn_as_infoh_asked() {
        // 12.3.1.1 and 12.3.1.2 at every symbol rate, both TRN sizes, and
        // lengths from none to the 127 steps INFOh can ask for.
        for (n, rate) in SymbolRate::ALL.into_iter().enumerate() {
            for (size, steps) in [(Size::Four, 0u8), (Size::Sixteen, 1), (Size::Four, 7), (Size::Sixteen, 127)] {
                let channel = channel(rate, n % 2 == 1, size, steps);
                let mut source = Source::new(channel, Mode::Call, FS);
                assert!(!source.is_sending());
                source.phase3();
                let (samples, changes) = burst(&mut source);
                let what = format!("{rate:?} {size:?} x {steps}");
                let got = segments(&changes, channel.band);
                let mut want = vec![
                    (Sending::Silence, 0.070 * channel.band.baud()),
                    (Sending::S, 128.0),
                    (Sending::SBar, 16.0),
                    (Sending::Pp, 288.0),
                ];
                if steps > 0 {
                    want.push((Sending::Trn, f64::from(steps) * 0.035 * channel.band.baud()));
                }
                assert_eq!(got.len(), want.len() + 1, "{what}: {got:?}");
                for ((segment, symbols), (wanted, expected)) in got.iter().zip(&want) {
                    assert_eq!(segment, wanted, "{what}");
                    assert!((symbols - expected).abs() < 1.01, "{what}: {segment:?} lasted {symbols:.2} symbols, not {expected:.2}");
                }
                assert_eq!(got.last().map(|s| s.0), Some(Sending::Flushing), "{what}");
                // Nothing on the line for the first 70 ms, and nothing left
                // once the source says it is idle.
                let silent = (0.068 * FS) as usize;
                assert!(samples[..silent].iter().all(|x| x.abs() < 1e-9), "{what}: something in the silence");
                let tail = &samples[samples.len() - 100..];
                assert!(tail.iter().all(|x| x.abs() < 1e-12), "{what}: the line is not quiet once the source is idle");
                let peak = samples.iter().fold(0.0f64, |m, x| m.max(x.abs()));
                assert!(peak > 0.5, "{what}: peak {peak}");
            }
        }
    }

    #[test]
    fn a_page_ends_with_35_ms_of_ones_in_whole_mapping_frames_then_nothing() {
        // 12.5.3.1, at every symbol rate: after the page's last bit is taken
        // into a mapping frame, the ones last 35 ms made up to whole frames,
        // and then the pulse is carried out and the line is quiet.
        for rate in SymbolRate::ALL {
            let channel = channel(rate, false, Size::Four, 4);
            let step = (0.035 * channel.band.baud()).round() as usize;
            assert_eq!(channel.turn_off_symbols(), step.div_ceil(8) * 8, "{rate:?}");
            let data = data_mode(if rate == SymbolRate::S2400 { 19_200 } else { 24_000 }, Code::States16);
            let mut source = Source::new(channel, Mode::Answer, FS);
            assert!(source.page(data));
            let bits = random_bits(6000, 3);
            source.push_bits(&bits);
            source.end_page();
            let (samples, changes) = burst(&mut source);
            let got = segments(&changes, channel.band);
            let names: Vec<Sending> = got.iter().map(|s| s.0).collect();
            assert_eq!(
                names,
                [Sending::Silence, Sending::S, Sending::SBar, Sending::Pp, Sending::B1, Sending::Data, Sending::TurningOff, Sending::Flushing],
                "{rate:?}"
            );
            let framing = data.params(rate, Mode::Answer).unwrap().framing;
            let (b1, page, ones) = (got[4].1, got[5].1, got[6].1);
            assert!((b1 - framing.symbols_per_data_frame() as f64).abs() < 1.01, "{rate:?}: B1 lasted {b1:.1} symbols");
            // The page's bits, in whole mapping frames of b or b - 1 bits.
            let frames = (page / 8.0).round();
            assert!((page - 8.0 * frames).abs() < 1.01, "{rate:?}: data lasted {page:.1} symbols");
            let carried: usize = (0..frames as usize).map(|i| framing.bits_in(i)).sum();
            assert!(carried >= bits.len() && carried < bits.len() + framing.b, "{rate:?}: {carried} bits in {frames} frames for {}", bits.len());
            let expect = channel.turn_off_symbols() as f64;
            assert!((ones - expect).abs() < 1.01, "{rate:?}: ones lasted {ones:.1} symbols, not {expect}");
            assert!(ones >= 0.035 * channel.band.baud() - 0.01, "{rate:?}: less than 35 ms of ones");
            assert!(ones < 0.035 * channel.band.baud() + 8.0, "{rate:?}: more than a mapping frame over 35 ms");
            assert_eq!(source.pending_bits(), 0);
            let tail = &samples[samples.len() - 100..];
            assert!(tail.iter().all(|x| x.abs() < 1e-12), "{rate:?}: the line is not quiet once the source is idle");
        }
    }

    #[test]
    fn a_data_mode_is_table_8s_row_or_nothing() {
        let mode = data_mode(33_600, Code::States64);
        assert!(mode.params(SymbolRate::S3200, Mode::Call).is_none());
        let params = mode.params(SymbolRate::S3429, Mode::Call).expect("33 600 at 3429");
        assert_eq!((params.framing.n, params.framing.b, params.code, params.mode), (1176, 79, Code::States64, Mode::Call));
        let mut source = Source::new(channel(SymbolRate::S3200, true, Size::Four, 2), Mode::Call, FS);
        assert!(!source.page(mode));
        assert!(!source.is_sending());
        assert!(source.page(data_mode(31_200, Code::States16)));
        assert!(source.is_sending());
    }

    /// A clock `ppm` parts per million slow, and the line's loss, noise and
    /// delay of `delay` seconds.
    fn line(samples: &[f64], ppm: f64, loss_db: f64, noise_db: f64, delay: f64) -> Vec<f64> {
        let mut resampler = dsp::Resampler::new(FS, FS * (1.0 + ppm * 1e-6));
        let mut out = vec![0.0; (delay * FS) as usize];
        for &x in samples {
            resampler.process(x, &mut out);
        }
        let gain = 10f64.powf(-loss_db / 20.0);
        let noise = 10f64.powf(-noise_db / 20.0) * 0.707;
        let mut seed = 0x2545_f491_u32;
        out.iter()
            .map(|x| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                x * gain + (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 3.464 * noise * gain
            })
            .collect()
    }

    /// The control channel between bursts, as far as the primary channel's
    /// receiver is concerned: `seconds` of something else at 600 baud from
    /// the source, on 1200 Hz from a call modem and on 2400 Hz with the guard
    /// tone from an answer modem (10.2.4).
    fn control(mode: Mode, seconds: f64) -> Vec<f64> {
        let side = match mode {
            Mode::Call => dpsk::Side::Call,
            Mode::Answer => dpsk::Side::Answer,
        };
        let mut tx = dpsk::Transmitter::new(side, FS);
        tx.send(&random_bits((seconds * dpsk::BAUD) as usize, 0x0bad_cafe));
        tx.silence();
        let mut out = Vec::new();
        while tx.is_sending() {
            out.push(tx.next_sample());
        }
        out
    }

    /// A call from the source's side: phase 3, then each of `pages` after
    /// its gap of seconds -- the 70 ms of silence the source keeps before it
    /// comes back on the control channel (12.4.1.1, 12.6.1.1), the control
    /// channel for half the gap, and silence for the rest. The samples, and
    /// for each page the sample at which the source began carrying its last
    /// symbol out.
    fn call(channel: Channel, mode: Mode, data: DataMode, pages: &[(f64, Vec<bool>)]) -> (Vec<f64>, Vec<usize>) {
        let mut source = Source::new(channel, mode, FS);
        source.phase3();
        let (mut out, _) = burst(&mut source);
        let mut ends = Vec::new();
        for (gap, bits) in pages {
            out.extend(std::iter::repeat_n(0.0, (0.070 * FS) as usize));
            out.extend(control(mode, gap / 2.0));
            out.extend(std::iter::repeat_n(0.0, (gap / 2.0 * FS) as usize));
            assert!(source.page(data), "{:?} has no {} bit/s", channel.band.rate, data.rate);
            source.push_bits(bits);
            source.end_page();
            let (samples, changes) = burst(&mut source);
            let flushing = changes.iter().find(|(_, s)| *s == Sending::Flushing).expect("a turn-off").0;
            ends.push(out.len() + flushing);
            out.extend(samples);
        }
        out.extend(std::iter::repeat_n(0.0, FS as usize / 2));
        (out, ends)
    }

    /// What a recipient made of a call.
    #[derive(Debug, Default)]
    struct Received {
        /// Every event, and the sample it came at.
        events: Vec<(usize, Event)>,
        /// Each page's bits, from B1's end.
        pages: Vec<Vec<bool>>,
        trn_share: f64,
        /// Slips the receiver found and followed.
        slips: u32,
    }

    impl Received {
        fn at(&self, wanted: impl Fn(&Event) -> bool) -> Vec<usize> {
            self.events.iter().filter(|(_, e)| wanted(e)).map(|(i, _)| *i).collect()
        }

        fn phase3_well(&self) -> Option<bool> {
            self.events.iter().find_map(|(_, e)| match e {
                Event::Phase3Over { well } => Some(*well),
                _ => None,
            })
        }

        fn page_starts(&self) -> Vec<(usize, usize)> {
            self.events
                .iter()
                .filter_map(|(i, e)| match e {
                    Event::PageStarted { b1_errors } => Some((*i, *b1_errors)),
                    _ => None,
                })
                .collect()
        }

        fn page_ends(&self) -> Vec<usize> {
            self.at(|e| matches!(e, Event::PageEnded))
        }
    }

    /// Hear `samples` as a recipient: phase 3 first, and then a page in
    /// `data` mode expected as soon as phase 3 or the page before is over.
    fn listen(samples: &[f64], channel: Channel, far: Mode, data: Option<DataMode>) -> Received {
        let mut rx = Recipient::new(channel, far, FS);
        rx.expect_phase3();
        let mut received = Received::default();
        for (i, &x) in samples.iter().enumerate() {
            rx.feed(x);
            while let Some(event) = rx.event() {
                received.events.push((i, event));
                match event {
                    Event::Phase3Over { .. } | Event::PageEnded => {
                        received.trn_share = rx.trn_share();
                        if let Some(data) = data {
                            assert!(rx.expect_page(data));
                        }
                    }
                    Event::PageStarted { .. } => received.pages.push(Vec::new()),
                    Event::Trained { .. } => {}
                }
            }
            if let Some(page) = received.pages.last_mut() {
                page.extend(rx.take_bits());
            }
        }
        received.slips = rx.slips();
        received
    }

    /// How many of `sent` are not at the start of `got`, the missing counted
    /// too.
    fn wrong_bits(sent: &[bool], got: &[bool]) -> usize {
        sent.iter().zip(got).filter(|(a, b)| a != b).count() + sent.len().saturating_sub(got.len())
    }

    /// The fastest, a middling and the slowest data rate at each symbol rate
    /// (Table 8), and a trellis code for each.
    fn rates(rate: SymbolRate) -> [(u32, Code); 3] {
        let (low, high) = match rate {
            SymbolRate::S2400 => (2400, 21_600),
            SymbolRate::S2743 | SymbolRate::S2800 => (4800, 26_400),
            SymbolRate::S3000 => (4800, 28_800),
            SymbolRate::S3200 => (4800, 31_200),
            SymbolRate::S3429 => (4800, 33_600),
        };
        let mid = (low + high) / 2 / 2400 * 2400;
        [(high, Code::States64), (mid, Code::States32), (low, Code::States16)]
    }

    /// A line good enough for a data rate: about what V.34 needs at it, and
    /// a few decibels over.
    fn noise_for(rate: u32) -> f64 {
        26.0 + rate as f64 / 1750.0
    }

    #[test]
    fn phase_3_and_pages_arrive_whole_at_every_symbol_rate_and_a_spread_of_data_rates() {
        // Phase 3 and three pages, each after seconds of the control channel
        // and silence, over a line with delay, noise, and the far clock tens
        // of ppm off: TRN judged well, every page trained on its PP, B1
        // counted without an error, every bit of every page right, and the
        // page's end seen within 6.6.2's 25 ms of the carrier going.
        for (n, rate) in SymbolRate::ALL.into_iter().enumerate() {
            for (m, (bits_per_second, code)) in rates(rate).into_iter().enumerate() {
                let channel = channel(rate, (n + m) % 2 == 1, if m == 1 { Size::Sixteen } else { Size::Four }, 3 + m as u8);
                let mode = if (n + m) % 3 == 0 { Mode::Answer } else { Mode::Call };
                // Expanded shaping in some; never non-linear encoding, which
                // the recipient's MPh does not ask for: the receiver's slicer
                // is linear, and a constellation whose outer points 9.7 has
                // bent outward reads as lost from B1 on.
                let data = DataMode { expanded: m == 2, ..data_mode(bits_per_second, code) };
                let what = format!("{rate:?} high {} at {bits_per_second} {code:?} from {mode:?}", channel.band.high_carrier);
                let pages: Vec<(f64, Vec<bool>)> = [(1.4, 0.6), (2.2, 0.4), (1.0, 0.5)]
                    .into_iter()
                    .enumerate()
                    .map(|(k, (gap, seconds))| (gap, random_bits((seconds * bits_per_second as f64) as usize, 11 + k as u32 + n as u32 * 7)))
                    .collect();
                let (sent, ends) = call(channel, mode, data, &pages);
                let (ppm, delay) = ([37.0, -60.0, 22.0][m], 0.030);
                let heard = listen(&line(&sent, ppm, 12.0, noise_for(bits_per_second), delay), channel, mode, Some(data));
                assert_eq!(heard.phase3_well(), Some(true), "{what}: phase 3 {:?}", heard.events.first());
                let starts = heard.page_starts();
                assert_eq!(starts.len(), pages.len(), "{what}: pages started {starts:?}");
                assert_eq!(heard.page_ends().len(), pages.len(), "{what}: pages ended");
                for (k, (page, (_, bits))) in heard.pages.iter().zip(&pages).enumerate() {
                    assert_eq!(starts[k].1, 0, "{what} page {k}: B1 errors");
                    let wrong = wrong_bits(bits, page);
                    assert_eq!(wrong, 0, "{what} page {k}: {wrong} bits wrong of {} ({} decoded)", bits.len(), page.len());
                    // What comes after the page is the turn-off's ones, and
                    // little else before the end is seen.
                    let extra = page.len() - bits.len();
                    assert!(extra < (0.100 * bits_per_second as f64) as usize, "{what} page {k}: {extra} bits after the page");
                    assert!(page[bits.len()..bits.len() + extra * 3 / 4].iter().all(|b| *b), "{what} page {k}: the turn-off is not ones");
                    // The end, against when the last symbol was centred on
                    // the line: the source began carrying it out at `ends`,
                    // and the pulse puts it on the line its lookahead later.
                    let last = ends[k] as f64 * (1.0 + ppm * 1e-6) + Transmitter::lookahead() as f64 * samples_per_symbol(channel.band) + delay * FS;
                    let latency = (heard.page_ends()[k] as f64 - last) / FS * 1000.0;
                    assert!((15.0..=27.0).contains(&latency), "{what} page {k}: the end seen {latency:.1} ms after the last symbol");
                }
            }
        }
    }

    /// A jitter buffer's slip at sample `at`: twenty milliseconds made up --
    /// the twenty before, faded across both joins as concealment does -- or
    /// twenty dropped.
    fn slip(samples: &mut Vec<f64>, at: usize, inserted: bool) {
        let n = (0.020 * FS) as usize;
        if inserted {
            let fade = 40;
            let mut made: Vec<f64> = samples[at - n..at].to_vec();
            for (i, x) in made.iter_mut().enumerate() {
                let edge = i.min(n - 1 - i);
                if edge < fade {
                    *x *= edge as f64 / fade as f64;
                }
            }
            samples.splice(at..at, made);
        } else {
            samples.drain(at..at + n);
        }
    }

    /// Where `needle` first occurs in `haystack`.
    fn find(haystack: &[bool], needle: &[bool]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// One page from a source that has done phase 3 and a spell of the
    /// control channel: the samples, and the samples at which the source was
    /// asked for the page's first data symbol and began carrying its last
    /// symbol out.
    fn one_page(channel: Channel, mode: Mode, data: DataMode, bits: &[bool]) -> (Vec<f64>, usize, usize) {
        let mut source = Source::new(channel, mode, FS);
        source.phase3();
        let (mut out, _) = burst(&mut source);
        out.extend(std::iter::repeat_n(0.0, (0.070 * FS) as usize));
        out.extend(control(mode, 0.5));
        out.extend(std::iter::repeat_n(0.0, FS as usize / 2));
        assert!(source.page(data));
        source.push_bits(bits);
        source.end_page();
        let (samples, changes) = burst(&mut source);
        let at = |wanted: Sending| out.len() + changes.iter().find(|(_, s)| *s == wanted).expect("the segment").0;
        let (start, end) = (at(Sending::Data), at(Sending::Flushing));
        out.extend(samples);
        out.extend(std::iter::repeat_n(0.0, FS as usize / 2));
        (out, start, end)
    }

    #[test]
    fn a_slip_inside_a_page_costs_only_the_bits_in_flight() {
        // Twenty milliseconds made up or dropped by a jitter buffer a second
        // and a half into a page: the receiver finds the jump and the acquirer
        // the frames after it, and everything before the slip, and from a
        // second after it to the end, is right bit for bit.
        for (rate, bits_per_second, inserted, ppm) in [
            (SymbolRate::S3429, 28_800, true, 40.0),
            (SymbolRate::S3429, 28_800, false, -30.0),
            (SymbolRate::S2743, 21_600, false, 55.0),
            (SymbolRate::S2800, 14_400, true, 0.0),
            (SymbolRate::S3200, 24_000, false, -45.0),
        ] {
            let channel = channel(rate, false, Size::Four, 3);
            let data = data_mode(bits_per_second, Code::States64);
            let bits = random_bits(3 * bits_per_second as usize, 21);
            let (mut sent, start, _) = one_page(channel, Mode::Call, data, &bits);
            let at = 1.5;
            slip(&mut sent, start + (at * FS) as usize, inserted);
            let heard = listen(&line(&sent, ppm, 15.0, noise_for(bits_per_second) - 2.0, 0.020), channel, Mode::Call, Some(data));
            let what = format!("{rate:?} at {bits_per_second}, inserted {inserted}");
            assert_eq!(heard.page_starts().len(), 1, "{what}: pages");
            assert_eq!(heard.page_ends().len(), 1, "{what}: ends");
            assert_eq!(heard.slips, 1, "{what}: slips followed");
            let got = &heard.pages[0];
            // Up to the slip, less a frame and the decoder's traceback.
            let before = ((at - 0.05) * bits_per_second as f64) as usize;
            assert_eq!(wrong_bits(&bits[..before], got), 0, "{what}: bits wrong before the slip");
            // From a second after it: found again in what was decoded, and
            // right from there to the end.
            let from = ((at + 1.0) * bits_per_second as f64) as usize;
            let tail = &bits[from..];
            let found = find(&got[before..], &tail[..64]).unwrap_or_else(|| panic!("{what}: the page after the slip is not there")) + before;
            assert_eq!(wrong_bits(tail, &got[found..]), 0, "{what}: bits wrong after the slip");
            // What went: the slip, the receiver's finding it, and the
            // search's superframe -- under a second's worth.
            assert!(found <= from, "{what}: {} bits more decoded than sent", found - from);
            assert!(from - found < bits_per_second as usize, "{what}: {} bits lost to the slip", from - found);
        }
    }

    #[test]
    fn a_page_the_size_of_an_ecm_block_arrives_whole() {
        // 256 frames of 256 octets with their flags, addresses, controls,
        // frame numbers and checks, and three RCPs (T.4 Annex A): about
        // 70 000 octets, 16.7 s at 33 600 bit/s. Every bit, at the fastest
        // rate with expanded shaping and at 28 800 on 3000 baud, the clocks
        // 45 and 55 ppm apart.
        for (rate, bits_per_second, code, expanded, ppm) in
            [(SymbolRate::S3429, 33_600, Code::States64, true, 45.0), (SymbolRate::S3000, 28_800, Code::States32, false, -55.0)]
        {
            let channel = channel(rate, true, Size::Sixteen, 8);
            let data = DataMode { expanded, ..data_mode(bits_per_second, code) };
            let bits = random_bits(70_000 * 8, 31);
            let (sent, _) = call(channel, Mode::Call, data, &[(1.0, bits.clone())]);
            let heard = listen(&line(&sent, ppm, 10.0, noise_for(bits_per_second), 0.050), channel, Mode::Call, Some(data));
            let what = format!("{rate:?} at {bits_per_second}");
            assert_eq!(heard.phase3_well(), Some(true), "{what}");
            let starts = heard.page_starts();
            assert_eq!(starts.len(), 1, "{what}: pages started {starts:?}");
            assert_eq!(starts[0].1, 0, "{what}: B1 errors");
            assert_eq!(heard.page_ends().len(), 1, "{what}: pages ended");
            assert_eq!(heard.slips, 0, "{what}: slips");
            let wrong = wrong_bits(&bits, &heard.pages[0]);
            assert_eq!(wrong, 0, "{what}: {wrong} bits wrong of {} ({} decoded)", bits.len(), heard.pages[0].len());
        }
    }

    /// `samples` faded by `depth_db` from `from` for `seconds`, eased in and
    /// out over 10 ms.
    fn fade(samples: &mut [f64], from: usize, seconds: f64, depth_db: f64) {
        let ramp = (0.010 * FS) as usize;
        let length = (seconds * FS) as usize;
        let floor = 10f64.powf(-depth_db / 20.0);
        for i in 0..length {
            let edge = i.min(length - 1 - i);
            let eased = if edge < ramp { 0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / ramp as f64).cos() } else { 1.0 };
            samples[from + i] *= 1.0 - eased * (1.0 - floor);
        }
    }

    #[test]
    fn the_end_of_a_page_is_seen_inside_25_ms_and_a_fade_is_not_an_end() {
        // 6.6.2: circuit 109 off 20 to 25 ms after the level falls, measured
        // here from the last symbol's centre on the line; and not off for a
        // carrier that dips 6 or 10 dB for 300 ms, the deeper of them 2 dB
        // above the threshold.
        for (rate, bits_per_second) in [(SymbolRate::S2400, 16_800), (SymbolRate::S3000, 24_000), (SymbolRate::S3429, 31_200)] {
            let channel = channel(rate, rate == SymbolRate::S3000, Size::Four, 2);
            let data = data_mode(bits_per_second, Code::States16);
            let bits = random_bits((1.2 * bits_per_second as f64) as usize, 41);
            let (sent, start, end) = one_page(channel, Mode::Answer, data, &bits);
            for depth_db in [0.0, 6.0, 10.0] {
                let (ppm, delay) = (30.0, 0.025);
                let mut samples = line(&sent, ppm, 12.0, 40.0, delay);
                let on_line = |at: usize| (at as f64 * (1.0 + ppm * 1e-6) + delay * FS) as usize;
                if depth_db > 0.0 {
                    fade(&mut samples, on_line(start) + (0.5 * FS) as usize, 0.3, depth_db);
                }
                let heard = listen(&samples, channel, Mode::Answer, Some(data));
                let what = format!("{rate:?} at {bits_per_second}, fade {depth_db} dB");
                assert_eq!(heard.page_starts().len(), 1, "{what}: pages started");
                let ends = heard.page_ends();
                assert_eq!(ends.len(), 1, "{what}: the page ended {} times", ends.len());
                let last = on_line(end) as f64 + Transmitter::lookahead() as f64 * samples_per_symbol(channel.band);
                let latency = (ends[0] as f64 - last) / FS * 1000.0;
                assert!((18.0..=25.5).contains(&latency), "{what}: the end seen {latency:.1} ms after the last symbol");
                if depth_db == 0.0 {
                    assert_eq!(wrong_bits(&bits, &heard.pages[0]), 0, "{what}");
                }
            }
        }
    }

    #[test]
    fn trn_of_sixteen_points_for_127_steps_and_of_no_length_at_all_train() {
        // INFOh's TRN at sixteen points for the 4.445 s of 127 steps: read,
        // all ones, and phase 3 over at the count. And TRN of no length, at
        // either size: trained on PP, and over at once.
        for (rate, size, steps) in [
            (SymbolRate::S3429, Size::Sixteen, 127),
            (SymbolRate::S2400, Size::Sixteen, 127),
            (SymbolRate::S3429, Size::Four, 0),
            (SymbolRate::S2400, Size::Sixteen, 0),
        ] {
            let channel = channel(rate, rate == SymbolRate::S3429, size, steps);
            let mut source = Source::new(channel, Mode::Answer, FS);
            source.phase3();
            let (mut sent, changes) = burst(&mut source);
            let trn_end = changes.iter().find(|(_, s)| *s == Sending::Flushing).expect("the flush").0;
            sent.extend(std::iter::repeat_n(0.0, FS as usize / 4));
            let (ppm, delay) = (-80.0, 0.010);
            let heard = listen(&line(&sent, ppm, 10.0, 42.0, delay), channel, Mode::Answer, None);
            let what = format!("{rate:?} {size:?} x {steps}");
            let trained = heard
                .events
                .iter()
                .find_map(|(_, e)| match e {
                    Event::Trained { snr_db } => Some(*snr_db),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{what}: never trained"));
            assert!(trained > 35.0, "{what}: trained to {trained:.1} dB");
            assert_eq!(heard.phase3_well(), Some(true), "{what}: {:?}", heard.events);
            let over = heard.at(|e| matches!(e, Event::Phase3Over { .. }))[0];
            let expected = trn_end as f64 * (1.0 + ppm * 1e-6) + delay * FS;
            let late = (over as f64 - expected) / FS * 1000.0;
            assert!((-10.0..=40.0).contains(&late), "{what}: phase 3 over {late:.1} ms after TRN's end");
            if steps > 0 {
                assert!((heard.trn_share - 1.0).abs() < 1e-9, "{what}: TRN ones {:.4}", heard.trn_share);
            }
        }
    }

    #[test]
    fn phase_3_is_not_well_without_s_in_2000_ms_or_with_a_trn_that_is_not_trn() {
        let rate = SymbolRate::S3200;
        let channel = channel(rate, false, Size::Sixteen, 6);
        // Nothing but noise: 12.3.3's 2000 ms, and then the recovery.
        let silence = vec![0.0; (2.5 * FS) as usize];
        let heard = listen(&line(&silence, 0.0, 10.0, 40.0, 0.0), channel, Mode::Call, None);
        assert!(heard.events.iter().all(|(_, e)| !matches!(e, Event::Trained { .. })), "trained on noise");
        let over = heard.at(|e| matches!(e, Event::Phase3Over { well: false }));
        assert_eq!(over.len(), 1, "{:?}", heard.events);
        let at = over[0] as f64 / FS;
        assert!((1.99..=2.01).contains(&at), "phase 3 given up at {at:.3} s");
        // S, S-bar and PP as they should be, and random sixteen-point symbols
        // for TRN's length: PP trains, and TRN does not descramble to ones.
        let mut symbols: Vec<Complex> = vec![Complex::ZERO; channel.silence_symbols()];
        symbols.extend((0..signals::S_SYMBOLS).map(|n| grid(signals::s(n), Size::Four)));
        symbols.extend((0..signals::S_BAR_SYMBOLS).map(|n| grid(signals::s_bar(n), Size::Four)));
        symbols.extend((0..signals::PP_SYMBOLS).map(|n| Complex::from(signals::pp(n))));
        let random = random_bits(4 * channel.trn_symbols(), 77);
        symbols.extend(random.chunks(4).map(|b| {
            let axis = |outer: bool, negative: bool| if outer { 3 } else { 1 } * if negative { -1 } else { 1 };
            grid((axis(b[0], b[1]), axis(b[2], b[3])), Size::Sixteen)
        }));
        let mut tx = Transmitter::new(channel.band, 0, 0, FS);
        let total = symbols.len() as u64;
        let mut symbols = VecDeque::from(symbols);
        let mut sent = Vec::new();
        while tx.symbols() < total + 2 * Transmitter::lookahead() as u64 {
            sent.push(tx.next_sample(|| symbols.pop_front().unwrap_or(Complex::ZERO)));
        }
        sent.extend(std::iter::repeat_n(0.0, FS as usize / 4));
        let heard = listen(&line(&sent, 20.0, 10.0, 40.0, 0.0), channel, Mode::Call, None);
        assert!(heard.events.iter().any(|(_, e)| matches!(e, Event::Trained { .. })), "PP did not train");
        assert_eq!(heard.phase3_well(), Some(false), "{:?}", heard.events);
        assert!(heard.trn_share < 0.9, "TRN ones {:.3}", heard.trn_share);
        // And TRN at four points where sixteen were asked, as an INFOh misread
        // would have it: the point's own bits come out as noise.
        let mut source = Source::new(Channel { trn_size: Size::Four, ..channel }, Mode::Call, FS);
        source.phase3();
        let (mut sent, _) = burst(&mut source);
        sent.extend(std::iter::repeat_n(0.0, FS as usize / 4));
        let heard = listen(&line(&sent, 0.0, 10.0, 40.0, 0.0), channel, Mode::Call, None);
        assert_eq!(heard.phase3_well(), Some(false), "four-point TRN read as sixteen: {:?}", heard.events);
        assert!(heard.trn_share < 0.9, "TRN ones {:.3}", heard.trn_share);
    }

    #[test]
    fn a_slip_of_whole_symbols_and_cycles_at_2400_or_3000_baud_leaves_no_jump_to_find() {
        // Twenty milliseconds is 48 symbols at 2400 baud and 60 at 3000, and
        // 32, 36 or 40 whole cycles of their carriers: twenty milliseconds
        // dropped, or made up with only concealment's fades to show for it,
        // leaves no jump in the timing or the carrier's phase, and the
        // receiver's loops see nothing. Nor does the decoder's cost say much:
        // the trellis carries on, and only Table 12's inversions, now half a
        // data frame out, cost it anything. At 2400 baud the mapping frames
        // still line up, so the bits after the slip come out right except
        // where the inversions disagree; at 3000 they are half a frame out
        // and the rest of the burst is wrong. T.30's ECM sends those frames
        // again; the cure is a decoder that counts inversions against Table
        // 12 (data.rs), not anything here. What holds: everything before the
        // slip is right, the receiver did not think it slipped, and the end
        // is still seen.
        for (rate, bits_per_second, high, inserted) in [(SymbolRate::S2400, 19_200, false, false), (SymbolRate::S3000, 21_600, true, false), (SymbolRate::S3000, 14_400, false, true)] {
            let channel = channel(rate, high, Size::Four, 3);
            let data = data_mode(bits_per_second, Code::States16);
            let bits = random_bits(2 * bits_per_second as usize, 23);
            let (mut sent, start, _) = one_page(channel, Mode::Call, data, &bits);
            slip(&mut sent, start + FS as usize, inserted);
            let heard = listen(&line(&sent, 30.0, 15.0, noise_for(bits_per_second), 0.020), channel, Mode::Call, Some(data));
            let what = format!("{rate:?} at {bits_per_second}, inserted {inserted}");
            assert_eq!(heard.page_starts().len(), 1, "{what}: pages");
            assert_eq!(heard.page_ends().len(), 1, "{what}: ends");
            assert_eq!(heard.slips, 0, "{what}: the receiver found a jump after all");
            let before = ((1.0 - 0.05) * bits_per_second as f64) as usize;
            assert_eq!(wrong_bits(&bits[..before], &heard.pages[0]), 0, "{what}: bits wrong before the slip");
        }
    }
}
