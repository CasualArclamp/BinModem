//! The V.92 analogue modem from Phase 3 on (9.5.2, 9.6.2): PCM going up as
//! well as coming down.
//!
//! V92-32 built this end as far as Jd and V92-36 as far as E1u; V92-39 takes it
//! through Phase 4 and data mode. What is here is Phase 3: the Ru/MD/TRN1u/Ja
//! opening, the silence in which the digital modem's TRN1d trains this end's
//! receiver, Jd, Su, Jp, the second TRN1u while the DIL arrives, CPt and E1u.
//!
//! Phase 3 is less a conversation than a timetable both ends keep without
//! asking, and every signal in it is one of two things: a **timing mark** that
//! says where the other end's clock is, or a **burst of bits** in the two-point
//! modulation TRN1u establishes. Ru and its bar are the first kind -- the
//! digital modem finds the reversal and starts its own TRN1u 24T later -- and Ja
//! is the second, carrying the DIL descriptor and the upstream rate mask.
//!
//! ```text
//! analogue sends   silent 70 ms | Ru 384T | Rbar-u 24T | [MD Ru 384T | Rbar-u 24T]
//!                  TRN1u >= 2040T | Ja (24 ones, descriptor repeated)
//!                  --- silence ---                         (TRN1d arrives here)
//!                  Su 144T | Sbar-u 24.5T | Su ...         (Jd arrives here)
//!                  Sbar-u 24T + epsilon                    (on Jp)
//!                  TRN1u >= 2040T over the DIL
//!                  CPt repeated until Ri | E1u
//! digital sends    --- silence --- | Sd 384T | Sbar-d 48T | TRN1d >= 2040T | Jd ...
//!                  Jp ... | Jp' 12T | DIL ... | Ri 384T | Rbar-i 24T | TRN2d
//! ```
//!
//! Three things are deliberately not this module's:
//!
//! * **The fraction of a symbol.** S-bar-u is 24.5T the first time and 24T plus
//!   epsilon the second (9.5.2.1.7-9.5.2.1.8), and 8.5.6 wants Su and S-bar-u to
//!   be whole numbers of twelve symbols. So the half and the fraction are not
//!   extra symbols: they are a lasting delay of every later upstream symbol, and
//!   [`crate::v92::transmit`] applies them at the D/A. This module records them
//!   in [`Modem::delays`], in the order 9.5.2.1 gives, and works in symbols
//!   throughout.
//! * **The DIL's contents.** The descriptor this end put in Ja says how long the
//!   DIL is, and while it arrives this end sends TRN1u. What the symbols *mean*
//!   -- which codewords the network robs, how far apart the levels are -- is the
//!   reading `v90::dil` does for V.90 and V92-39 will do here; the values are
//!   kept in [`Modem::dil`] exactly as the equaliser produced them.
//! * **Phase 4.** E1u is the last thing Phase 3 sends; TRN2u, SUVu, CPu, E2u and
//!   B1u are V92-39's, and [`Status::Phase3Done`] is where this end waits for
//!   them.
//!
//! The one thing here that is not a sequence is the sample phase. Jp's epsilon
//! and the half-symbol shift both exist because the digital modem cannot move
//! its A/D and this end can move its transmitter, and nothing in this module
//! generates samples, so both are recorded rather than applied.

use std::collections::VecDeque;

use crate::v32::Mode;
use crate::v90::sequences::{Cp, Descriptor, JD_BITS, Jd, Layout};
use crate::v92::receiver::{Heard, Receiver, Symbol};
use crate::v92::sequences::{J, JP_PRIME_BITS, Jp};
use crate::v92::up_signals::{Trn1u, TwoPointReader, TwoPointSender, ru, su};
use crate::v92::{UP_INTERVALS, md_symbols};

/// This end's symbol rate, which is the network's: "The upstream symbol rate
/// shall be 8000 symbol/s derived from the digital network" (6.2). Phase 3 is
/// outside the clause, but the rate is the same one the digital modem runs at.
pub const FS: f64 = 8000.0;

// ---------------------------------------------------------------------------
// The signals' lengths (9.5.2.1)
// ---------------------------------------------------------------------------

/// "Before transmitting signal Ru, the analogue modem shall transmit silence for
/// a period of 70 +- 5 ms" (9.5.2.1.1).
const LEAD_IN: f64 = 0.070;

/// Ru is 384T and R-bar-u 24T (9.5.2.1.1): 64 and 4 periods of the six-symbol
/// sequence 8.5.5 repeats.
const RU_SYMBOLS: u64 = 384;
const RU_BAR_SYMBOLS: u64 = 24;

/// "The analogue modem shall then transmit TRN1u for at least 2040T"
/// (9.5.2.1.2), and 8.5.7 wants every TRN1u segment to be a whole number of
/// twelve symbols -- which 2040 is, 170 frames of them.
const TRN1U_MINIMUM: u64 = 2040;

/// Su is 144T (9.5.2.1.6); S-bar-u is 24T plus a fraction (9.5.2.1.7-8).
const SU_SYMBOLS: u64 = 144;
const SU_BAR_SYMBOLS: u64 = 24;

/// The half symbol 9.5.2.1.7 asks for at the S-bar-u that ends Su's 144T.
///
/// It is a delay of the transmitter and not a longer pattern: "S-bar-u shall be
/// 24.5T", and 8.5.6 requires whole twelve-symbol lengths, so the 0.5 belongs to
/// `v92::transmit::PcmTransmitter::delay`.
const HALF_SYMBOL: f64 = 0.5;

/// TRN1d begins 48T after the Sd-to-S-bar-d reversal: S-bar-d is 48T long
/// (V.90 8.4.4), and the reversal is where it begins.
const TRN1D_START: u64 = 48;

/// Symbols of TRN1d gone by before the equaliser is fitted, and how far either
/// side of the expected start the fit searches. Both are the digital modem's own
/// numbers for its TRN1u (`v92::digital`), because the two fits are the same job
/// on the same sequence.
const TRAIN_AFTER: u64 = 1300;
const TRAIN_SEARCH: u64 = 64;

/// Jp's epsilon is a "16-bit unsigned integer covering the range [0, 1) symbol"
/// (Table 22 bits 18:33), so one symbol is 2^16 of them.
const EPSILON_STEPS: f64 = 65536.0;

// ---------------------------------------------------------------------------
// Deadlines (9.5.2.2, and the start-up timer of 9.6.2.2.1)
// ---------------------------------------------------------------------------

/// AR1: "If the Sd-to-S-bar-d reversal is not detected within 1500 ms of the
/// start of Ja, the analogue modem shall retrain". No round-trip term.
const SD_WITHIN: f64 = 1.5;

/// AR2: "If Jd is not received within 4500 ms of the end of Ja".
const JD_WITHIN: f64 = 4.5;

/// A6: Su goes out "no later than 5000 ms after the silence of A3 began", so Jd
/// has to have been read by then.
const SU_WITHIN: f64 = 5.0;

/// A11: CPt "shall start within 5000 ms of sending the S-bar-u of A8".
const CPT_WITHIN: f64 = 5.0;

/// "If B1d is not received within 20 s + 6 x RTD of the end of INFO1a, retrain"
/// (9.6.2.2.1): the timer that covers all of Phase 3 and Phase 4.
const START_UP: f64 = 20.0;
const START_UP_RTD: f64 = 6.0;

/// Whether the N = 0 branch keeps 9.5.2.1.10's order: TRN1u until Ri, then CPt,
/// and E1u on R-bar-i.
///
/// That order needs a far end which sends Ri **before** it has seen a CPt, which
/// is what 9.5.1.1.13's SCR path does: with no DIL asked for, the digital modem
/// puts SCR in its place and then Ri. It is true here because that is what a
/// real digital modem does -- an Agere SV92PP waiting for Ri with SCR on the
/// line is what showed the difference, and the digital modem in this crate
/// sends SCR for the same reason. False takes A11's order for N = 0 as well,
/// which is only the right reading for a far end that stays silent until a CPt
/// arrives.
pub const ZERO_DIL_A10_ORDER: bool = true;

/// Whether V92_TRACE in the environment is set, so that a start-up that will not
/// find its place is legible.
fn tracing() -> bool {
    use std::sync::OnceLock;
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("V92_TRACE").is_some())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// What Phase 2 settled, as Phase 3 needs it.
///
/// Everything here is this end's own choice, not the far end's: the descriptor
/// and the rate mask Ja carries, the MD this end will send, and the CPt it will
/// offer once the DIL has arrived. The far end's say arrives later, in Jd and
/// Jp, and is kept by the [`Modem`] itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The DIL descriptor Ja asks the digital modem for (Table 20, 8.5.4).
    /// [`Descriptor::none`] asks for no DIL, and 9.5.2.1.10 then replaces the
    /// DIL with SCR.
    pub descriptor: Descriptor,
    /// The upstream rate mask that rides with it (Table 20 bit 188+P): the
    /// nineteen rates of 6.1 this end's PCM transmitter can send.
    pub upstream_rates: u32,
    /// This end's MD, in 276-symbol steps (Table 18 bits 18:24). It goes out
    /// between the two Ru reversals, and the digital modem waits exactly this
    /// long before it hunts Ru again (9.5.1.1.1).
    pub md_length: u8,
    /// The CPt this end will send: the downstream rates it can receive.
    pub cpt: Cp,
    /// The round trip, from Phase 2's ranging: the deadlines that carry an RTD
    /// term use it.
    pub round_trip: f64,
}

impl Settings {
    /// Settings for a call that asks for `descriptor` and offers
    /// `upstream_rates`, with `cpt` naming the downstream rate this end wants
    /// and no MD.
    #[must_use]
    pub fn new(descriptor: Descriptor, upstream_rates: u32, cpt: Cp) -> Self {
        Self { descriptor, upstream_rates, md_length: 0, cpt, round_trip: 0.0 }
    }
}

/// How Phase 3 is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// E1u has gone out: Phase 3 is over and Phase 4 (V92-39) takes the line.
    Phase3Done,
    /// A deadline went by, or the far end stopped making sense. A live modem
    /// retrains here (9.5.2.2); the retrain itself is not built.
    Failed(&'static str),
}

// ---------------------------------------------------------------------------
// What goes up
// ---------------------------------------------------------------------------

/// One signal of the upstream timetable, as the source is asked for the next
/// symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Up {
    /// Nothing: the 70 ms before Ru, and the silence that lets TRN1d train.
    Silence,
    /// Ru, 384T (8.5.5).
    Ru,
    /// R-bar-u, 24T.
    RuBar,
    /// MD, in 276-symbol steps (9.5.2.1.1 with 8.5.3).
    Md,
    /// TRN1u, at least 2040T (8.5.7). Sent twice: before Ja and over the DIL.
    Trn1u,
    /// Ja: the twenty-four ones, then the descriptor repeated (8.5.4).
    Ja,
    /// Su, 144T and then running on (8.5.6).
    Su,
    /// S-bar-u: 24.5T after Su, and 24T plus epsilon after Jp.
    SuBar,
    /// CPt repeated until Ri (8.5.1).
    Cpt,
    /// E1u, one data frame of scrambled zeros (8.5.2).
    E1u,
}

/// The upstream, one symbol at a time.
///
/// It holds the two modulators Phase 3 uses -- the TRN1u (and MD) scrambler, and
/// the two-point differential carrier that Ja, CPt and E1u ride -- because
/// nothing between them resets anything: the GPA scrambler runs on from TRN1u
/// through Ja, and the differential encoder's memory is seeded from TRN1u's
/// final symbol (8.5.1, 8.5.4, 8.5.7).
#[derive(Debug, Clone)]
struct Source {
    up: Up,
    /// Symbols produced since `up` began.
    count: u64,
    trn: Trn1u,
    sender: TwoPointSender,
    /// Set when the sequence in flight is not to be continued: Ja is cut at a
    /// twelve-bit boundary when Sd turns over, and CPt ends where it is when Ri
    /// arrives.
    stop: bool,
    ja_bits: Vec<bool>,
    cpt_bits: Vec<bool>,
}

impl Source {
    fn new(settings: &Settings) -> Self {
        Self {
            up: Up::Silence,
            count: 0,
            trn: Trn1u::new(),
            sender: TwoPointSender::after(Trn1u::new()),
            stop: false,
            ja_bits: settings.descriptor.to_bits_in(Some(settings.upstream_rates)),
            // One bit a symbol (8.5.1), so twelve symbols are twelve bits: a
            // CPt repetition is a whole number of them, which is what keeps
            // the upstream data frame grid Ja established.
            cpt_bits: settings.cpt.to_bits_in(Layout::V92, UP_INTERVALS),
        }
    }

    fn start(&mut self, up: Up) {
        self.up = up;
        self.count = 0;
        self.stop = false;
    }

    /// Take over the two-point carrier from the TRN1u that has just been sent.
    ///
    /// "The differentially encoded symbol shall be initialized with the final
    /// symbol of the preceding TRN1u" (8.5.1), and the scrambler is not reset:
    /// the far end's descrambler is locked to it already.
    fn begin_sequence(&mut self) {
        let trn = self.trn.clone();
        self.sender = TwoPointSender::after(trn);
        self.stop = false;
    }

    /// One symbol out, in LU units.
    fn next(&mut self) -> f64 {
        let n = self.count;
        self.count += 1;
        match self.up {
            Up::Silence => 0.0,
            Up::Ru => ru(n, false),
            Up::RuBar => ru(n, true),
            // MD's content is "manufacturer defined" (8.5.3) and its only other
            // requirement is its length: a scrambled burst is what an echo
            // canceller can be trained on, and it is the one signal both ends
            // already have.
            Up::Md | Up::Trn1u => self.trn.next_symbol(),
            Up::Su => su(n, false),
            Up::SuBar => su(n, true),
            Up::Ja => {
                if let Some(level) = self.sender.next_symbol() {
                    return level;
                }
                if self.stop {
                    return 0.0;
                }
                // "24 binary ones followed by repetitions of the DIL
                // descriptor" (8.5.4): the ones are queued once, by `begin_ja`.
                self.sender.push(&self.ja_bits);
                self.sender.next_symbol().unwrap_or(0.0)
            }
            Up::Cpt => {
                if let Some(level) = self.sender.next_symbol() {
                    return level;
                }
                if self.stop {
                    return 0.0;
                }
                // "CPt, CPt, ..." (8.5.1): the twenty-four ones are queued once,
                // by `begin_cpt`, and every repetition after them is a whole
                // twelve-symbol frame.
                self.sender.push(&self.cpt_bits);
                self.sender.next_symbol().unwrap_or(0.0)
            }
            Up::E1u => self.sender.next_symbol().unwrap_or(0.0),
        }
    }

    /// Whether the two-point carrier has nothing left in flight.
    fn drained(&self) -> bool {
        self.sender.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Where the state machine has got to
// ---------------------------------------------------------------------------

/// Where Phase 3 has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// A1-A2: silence, Ru, R-bar-u, [MD, Ru, R-bar-u], TRN1u, then Ja.
    Opening,
    /// A3: Ja is on the line and Sd's reversal is awaited.
    Ja,
    /// A4-A5: silent, fitting the equaliser on TRN1d.
    Training,
    /// A5: trained, and Jd is being read out of what follows.
    Jd,
    /// A6-A7: Su, S-bar-u, Su running on, waiting for Jp.
    Su,
    /// A8: S-bar-u for 24T plus epsilon, while Jp' is awaited.
    AfterJp,
    /// A9-A11: TRN1u while the DIL arrives, then CPt.
    Dil,
    /// A10-A11: CPt repeated until Ri.
    Cpt,
    /// The twelve zeros of E1u.
    E1u,
    /// Phase 3 is over: see V92-39 for TRN2u and Phase 4.
    Done,
}

// ---------------------------------------------------------------------------
// The modem
// ---------------------------------------------------------------------------

/// The V.92 analogue modem, from the end of Phase 2.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    /// What the digital modem is saying, sliced as two points.
    rx: Receiver,
    /// Jd, Jp and Jp' come off one differential stream, descrambled with GPC:
    /// they are the digital modem's sequences and it sends them (6.3).
    reader: TwoPointReader,
    /// The last seventy-two decoded bits, because Jd and Jp are framed
    /// sequences that arrive with no marker in front of them.
    window: VecDeque<bool>,
    /// Consecutive zero bits, which is how Jp' is seen (8.6.4).
    zeroes: usize,
    /// Whether Jp' has been read, after which the DIL's symbols are counted.
    jp_prime: bool,
    dil_left: usize,
    dil: Vec<f64>,
    source: Source,
    stage: Stage,
    /// Where TRN1d began, in the receiver's own symbol count.
    trn_from: Option<u64>,
    /// How many Ru segments the opening has sent, 0 to 2.
    rus: u8,
    far_jd: Option<Jd>,
    jp: Option<Jp>,
    /// Circuit 107, raised when a CRC-valid Jp with bit 47 set arrives
    /// (9.5.2.1.8).
    circuit107: bool,
    /// The sub-symbol delays S-bar-u carries, in the order they are to be
    /// applied: half a symbol, then epsilon (`v92::transmit`).
    delays: Vec<f64>,
    /// Whether the Su on the line is A6's first one, which is 144T long.
    ///
    /// A7's Su -- the one after the 24.5T S-bar-u -- is not: it "runs on until
    /// Jp", and the digital modem is allowed to take its time working out
    /// epsilon before it sends one. Cutting that run at 144T as well would put
    /// a S-bar-u nobody asked for into the middle of the measurement the digital
    /// modem is still making, and a second half symbol into the transmitter.
    su_short: bool,
    now: u64,
    lead_in: u64,
    ja_began: Option<u64>,
    sd_deadline: Option<u64>,
    jd_deadline: Option<u64>,
    su_by: Option<u64>,
    cpt_by: Option<u64>,
    start_up_by: u64,
    status: Status,
    notes: Vec<String>,
}

impl Modem {
    /// From the end of Phase 2: silent for 70 ms, then Ru.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        let mut rx = Receiver::new();
        // The digital modem's TRN1d is generated with GPC, not the GPA an
        // analogue modem's own TRN1u uses (6.3, 9.5.1.1.2).
        rx.set_reference_mode(Mode::Call);
        // A3 asks for Sd and its reversal, and Sd is Su's shape the other way
        // up (V.90 8.4.4): between the 70 ms of silence and the DIL nothing
        // else of that shape is due.
        rx.expect_su();
        let start_up_by = ((START_UP + START_UP_RTD * settings.round_trip) * FS) as u64;
        Self {
            source: Source::new(&settings),
            settings,
            rx,
            reader: TwoPointReader::differential_with(Mode::Call),
            window: VecDeque::new(),
            zeroes: 0,
            jp_prime: false,
            dil_left: 0,
            dil: Vec::new(),
            stage: Stage::Opening,
            trn_from: None,
            rus: 0,
            far_jd: None,
            jp: None,
            circuit107: false,
            delays: Vec::new(),
            su_short: false,
            now: 0,
            lead_in: (LEAD_IN * FS) as u64,
            ja_began: None,
            sd_deadline: None,
            jd_deadline: None,
            su_by: None,
            cpt_by: None,
            start_up_by,
            status: Status::Running,
            notes: Vec::new(),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// What is on the line, for the transcript.
    #[must_use]
    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::Opening => "V.92 phase 3: Ru, TRN1u and Ja",
            Stage::Ja => "V.92 phase 3: Ja",
            Stage::Training => "V.92 phase 3: TRN1d",
            Stage::Jd => "V.92 phase 3: Jd",
            Stage::Su => "V.92 phase 3: Su",
            Stage::AfterJp => "V.92 phase 3: S-bar-u",
            Stage::Dil => "V.92 phase 3: TRN1u over the DIL",
            Stage::Cpt => "V.92 phase 3: CPt",
            Stage::E1u => "V.92 phase 3: E1u",
            Stage::Done => "V.92 phase 3: done",
        }
    }

    /// The digital modem's Jd, once read (8.6.2).
    pub fn far_jd(&self) -> Option<&Jd> {
        self.far_jd.as_ref()
    }

    /// Its Jp, once read (8.6.3).
    pub fn jp(&self) -> Option<&Jp> {
        self.jp.as_ref()
    }

    /// The fraction of a symbol Jp asked for, or None before Jp.
    ///
    /// "16-bit unsigned integer covering the range [0, 1) symbol" (Table 22
    /// bits 18:33), so the fraction is the field over 65 536.
    #[must_use]
    pub fn epsilon(&self) -> Option<f64> {
        self.jp.map(|jp| f64::from(jp.epsilon) / EPSILON_STEPS)
    }

    /// Whether circuit 107 has been raised (9.5.2.1.8).
    #[must_use]
    pub fn circuit107(&self) -> bool {
        self.circuit107
    }

    /// The sub-symbol delays the transmitter is to apply, in order: half a
    /// symbol at 9.5.2.1.7's S-bar-u, then epsilon at 9.5.2.1.8's.
    #[must_use]
    pub fn delays(&self) -> &[f64] {
        &self.delays
    }

    /// The DIL's symbols as the equaliser produced them, for V92-39 to read.
    #[must_use]
    pub fn dil(&self) -> &[f64] {
        &self.dil
    }

    /// How far into Phase 3 this modem has got, in symbols.
    #[must_use]
    pub fn elapsed(&self) -> u64 {
        self.now
    }

    /// Lines for the transcript, taken by whoever shows one.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// One line sample in, one symbol out.
    pub fn step(&mut self, from_digital: f64) -> f64 {
        self.now += 1;
        self.rx.feed(from_digital);
        while let Some(heard) = self.rx.heard() {
            self.heard(heard);
        }
        if tracing() && self.now.is_multiple_of(2000) {
            eprintln!(
                "V92ATRACE: {} symbols, {}, up {:?}, trained {}, snr {:.1}",
                self.now,
                self.phase(),
                self.source.up,
                self.rx.is_trained(),
                self.rx.snr_db()
            );
        }
        self.train();
        self.advance();
        self.timers();
        self.source.next()
    }

    // -----------------------------------------------------------------------
    // The timetable: what goes out next
    // -----------------------------------------------------------------------

    /// Where each signal ends, and what follows it (9.5.2.1's A1-A11).
    fn advance(&mut self) {
        match self.source.up {
            Up::Silence => {
                // A1: 70 ms of silence opens Phase 3.
                if self.stage == Stage::Opening && self.source.count >= self.lead_in {
                    self.begin_ru();
                }
            }
            Up::Ru => {
                if self.source.count >= RU_SYMBOLS {
                    self.source.start(Up::RuBar);
                }
            }
            Up::RuBar => {
                if self.source.count >= RU_BAR_SYMBOLS {
                    // A1: with an MD to send, an Ru/R-bar-u block follows it;
                    // without one, TRN1u comes straight after the first block.
                    if self.settings.md_length > 0 && self.rus == 1 {
                        self.source.start(Up::Md);
                        let symbols = md_symbols(self.settings.md_length);
                        self.notes.push(format!("MD of {symbols} symbols, then Ru again"));
                    } else {
                        self.begin_trn1u();
                    }
                }
            }
            Up::Md => {
                if self.source.count >= md_symbols(self.settings.md_length) as u64 {
                    self.begin_ru();
                }
            }
            Up::Trn1u => {
                if self.stage == Stage::Opening {
                    // A2: TRN1u, then Ja.
                    if self.source.count >= TRN1U_MINIMUM {
                        self.begin_ja();
                    }
                } else if self.source.count >= self.trn1u_over_dil() && self.dil_complete() {
                    // A9-A11: the TRN1u that covers the DIL runs until the DIL
                    // has arrived and 2040T have gone by; both bounds are
                    // multiples of twelve, so the segment is one too (8.5.7).
                    // With no DIL asked for it does not end here: A10 has it
                    // run until Ri, and Ri is what starts the CPt.
                    if !self.zero_dil_a10() {
                        self.begin_cpt();
                    }
                }
            }
            Up::Ja => {
                // A3: the reversal cut Ja at the last twelve-bit boundary it
                // reached, and this end is then silent until A6.
                if self.source.stop && self.source.drained() {
                    self.source.start(Up::Silence);
                }
            }
            Up::Su => {
                if self.su_short && self.source.count >= SU_SYMBOLS {
                    // A7: 24.5T of S-bar-u. The half symbol is the
                    // transmitter's, and this is where it is asked for.
                    self.source.start(Up::SuBar);
                    self.delays.push(HALF_SYMBOL);
                    self.su_short = false;
                }
            }
            Up::SuBar => {
                if self.source.count >= SU_BAR_SYMBOLS {
                    if self.stage == Stage::Su {
                        // A7: Su again, and it runs on until Jp.
                        self.source.start(Up::Su);
                    } else {
                        // A8: this was the bar after Jp, and TRN1u follows it.
                        self.begin_trn1u_over_dil();
                    }
                }
            }
            Up::Cpt => {
                if self.source.stop && self.source.drained() {
                    // A10-A11: finish the current CPt, then E1u -- through the
                    // same carrier, so the scrambler runs on into it.
                    self.source.start(Up::E1u);
                    self.source.sender.e1u();
                    self.stage = Stage::E1u;
                    self.notes.push("CPt ends, E1u".into());
                }
            }
            Up::E1u => {
                if self.source.drained() {
                    // Phase 3 is over; TRN2u is V92-39's.
                    self.source.start(Up::Silence);
                    self.stage = Stage::Done;
                    self.status = Status::Phase3Done;
                    self.notes.push("E1u sent, phase 3 done".into());
                }
            }
        }
    }

    fn begin_ru(&mut self) {
        self.source.start(Up::Ru);
        self.rus += 1;
    }

    fn begin_trn1u(&mut self) {
        // "The scrambler shall be initialized to zero prior to the transmission
        // of TRN1u" (8.5.7), for each segment.
        self.source.trn = Trn1u::new();
        self.source.start(Up::Trn1u);
    }

    /// A3: Ja, with the twenty-four ones in front of the first repetition.
    fn begin_ja(&mut self) {
        self.source.begin_sequence();
        self.source.sender.preamble();
        self.source.start(Up::Ja);
        self.stage = Stage::Ja;
        self.ja_began = Some(self.now);
        self.sd_deadline = Some(self.now + (SD_WITHIN * FS) as u64);
        self.notes.push("Ja".into());
    }

    /// A9: the second TRN1u, whose first symbol is upstream interval 0.
    fn begin_trn1u_over_dil(&mut self) {
        self.begin_trn1u();
        self.stage = Stage::Dil;
        // A11: Ri, and then R-bar-i, are Ru's shape at UINFO (8.6.5 with
        // 8.5.5) and nothing else of that shape comes before them.
        self.rx.expect_ru();
        self.notes.push("TRN1u over the DIL, from upstream interval 0".into());
    }

    /// A10-A11: CPt, after the twenty-four ones (8.5.1).
    fn begin_cpt(&mut self) {
        self.source.begin_sequence();
        self.source.sender.preamble();
        self.source.start(Up::Cpt);
        self.stage = Stage::Cpt;
        self.cpt_by = None;
        self.notes.push(format!("CPt for drn {}", self.settings.cpt.drn));
    }

    /// How long the TRN1u over the DIL has to be: at least 2040T, and at least
    /// as long as the DIL, rounded up to whole twelve-symbol frames (8.5.7,
    /// 9.5.2.1.9).
    fn trn1u_over_dil(&self) -> u64 {
        let dil = self.settings.descriptor.len() as u64;
        let frames = dil.div_ceil(UP_INTERVALS as u64) * UP_INTERVALS as u64;
        frames.max(TRN1U_MINIMUM)
    }

    /// Whether Jp' has been read and a whole pass of the DIL has arrived after
    /// it.
    fn dil_complete(&self) -> bool {
        self.jp_prime && self.dil_left == 0
    }

    // -----------------------------------------------------------------------
    // What the digital modem says
    // -----------------------------------------------------------------------

    fn heard(&mut self, heard: Heard) {
        if tracing()
            && !matches!(heard, Heard::Symbol(_))
        {
            eprintln!("V92ATRACE: {heard:?} at {} in {}", self.now, self.phase());
        }
        match heard {
            // Sd is Su's shape the other way up (V.90 8.4.4), so the hunt
            // reports Sd as Su and its reversal as SuReversal.
            Heard::Su { at } => {
                if self.stage == Stage::Ja {
                    self.notes.push(format!("Sd at {at}"));
                }
            }
            Heard::SuReversal { at } => {
                if self.stage == Stage::Ja {
                    self.sd_reversal(at);
                }
            }
            // Ri and R-bar-i are Ru and R-bar-u's shape at UINFO (8.6.5 with
            // 8.5.5), so the same hunt reports them as Ru: "+ + + - - -" is
            // three-and-three whichever direction it goes in.
            Heard::Ru { at } => self.ri(at),
            Heard::RuReversal { at } => self.ri_bar(at),
            Heard::Symbol(symbol) => self.symbol(symbol),
            Heard::Trained { .. } | Heard::Lost | Heard::Found => {}
        }
    }

    /// A3: Sd has turned over, so Ja ends at the last twelve-bit boundary it
    /// reached and this end goes quiet for the digital modem's TRN1d.
    fn sd_reversal(&mut self, at: u64) {
        self.source.stop = true;
        self.stage = Stage::Training;
        self.sd_deadline = None;
        // A4: "the Sd to S-bar-d reversal marks the start of TRN1d", and
        // S-bar-d is 48T long (V.90 8.4.4).
        self.trn_from = Some(at + TRN1D_START);
        self.jd_deadline = Some(self.now + (JD_WITHIN * FS) as u64);
        // A6: Jd has to be read within 5 s of the silence, so that Su can go
        // out by then.
        self.su_by = Some(self.now + ((SU_WITHIN + self.settings.round_trip) * FS) as u64);
        self.notes.push(format!("Sd reversal at {at}, silent for TRN1d"));
    }

    /// A4-A5: the equaliser is fitted on the first 2040T of TRN1d, and Jd is
    /// read after it.
    fn train(&mut self) {
        if self.stage != Stage::Training {
            return;
        }
        let Some(from) = self.trn_from else { return };
        if self.rx.symbols() < from + TRAIN_AFTER {
            return;
        }
        // The fit is indexed against the receiver's history, which is a window
        // that slid past the reversal long ago: `from` is an absolute symbol
        // count and has to be brought back to where the window starts.
        let base = self.rx.symbols() - self.rx.history_len() as u64;
        let at = from.saturating_sub(base);
        if self.rx.train_near(at, TRAIN_SEARCH) {
            self.reader = TwoPointReader::differential_with(Mode::Call);
            self.window.clear();
            self.stage = Stage::Jd;
            self.notes.push(format!("TRN1d trained at {:.1} dB", self.rx.snr_db()));
        } else {
            // A hundred and thirty least squares over forty-eight taps is not
            // something to ask every sample: move the anchor on and try again.
            self.trn_from = Some(from + TRAIN_AFTER / 2);
        }
    }

    fn symbol(&mut self, symbol: Symbol) {
        match self.stage {
            // Jd and Jp are both seventy-two bits behind the same frame, told
            // apart by bit 47 (8.6.2-8.6.3), so they are read out of one sliding
            // window of the decoded stream.
            Stage::Jd | Stage::Su => {
                let Some(bit) = self.reader.feed(symbol.value) else { return };
                self.window.push_back(bit);
                while self.window.len() > JD_BITS {
                    self.window.pop_front();
                }
                if self.window.len() == JD_BITS
                    && let Some(j) = J::from_bits(&self.window.iter().copied().collect::<Vec<_>>())
                {
                    self.heard_frame(j);
                }
            }
            // A8-A9: Jp' is twelve binary zeroes (8.6.4), and the DIL follows
            // it.
            Stage::AfterJp | Stage::Dil => {
                if self.jp_prime {
                    self.dil.push(symbol.value);
                    self.dil_left = self.dil_left.saturating_sub(1);
                    return;
                }
                let Some(bit) = self.reader.feed(symbol.value) else { return };
                self.zeroes = if bit { 0 } else { self.zeroes + 1 };
                if self.zeroes == JP_PRIME_BITS {
                    self.jp_prime = true;
                    self.dil_left = self.settings.descriptor.len();
                    self.notes.push(format!("Jp' read, DIL of {} symbols", self.dil_left));
                }
            }
            _ => {}
        }
    }

    fn heard_frame(&mut self, j: J) {
        match (self.stage, j) {
            (Stage::Jd, J::Jd(jd)) if self.far_jd.is_none() => {
                self.far_jd = Some(jd);
                self.su_by = None;
                self.jd_deadline = None;
                // A6: Su at once, for 144T. This end may wait up to 5 s for a
                // better moment and has no reason to.
                self.source.start(Up::Su);
                self.su_short = true;
                self.stage = Stage::Su;
                self.notes.push(format!("Jd read, lookahead {}", jd.lookahead));
            }
            (Stage::Su, J::Jp(jp)) if self.jp.is_none() => {
                self.jp = Some(jp);
                // A8: circuit 107 comes on when a CRC-valid Jp with bit 47 set
                // arrives.
                self.circuit107 = true;
                self.stage = Stage::AfterJp;
                self.source.start(Up::SuBar);
                // "S-bar-u for 24T plus epsilon" is a lasting delay of the
                // transmitter, not extra symbols (8.5.6).
                self.delays.push(f64::from(jp.epsilon) / EPSILON_STEPS);
                self.cpt_by = Some(self.now + ((CPT_WITHIN + self.settings.round_trip) * FS) as u64);
                self.notes.push(format!("Jp read, epsilon {} and 107 on", jp.epsilon));
            }
            _ => {}
        }
    }

    /// Ri: the digital modem has read a CPt, so the CPt series ends (A11).
    fn ri(&mut self, at: u64) {
        match self.stage {
            Stage::Cpt if !self.zero_dil_a10() => {
                self.source.stop = true;
                self.notes.push(format!("Ri at {at}, CPt ends"));
            }
            // A10's order: Ri is what *starts* CPt, and R-bar-i ends it.
            Stage::Dil if self.zero_dil_a10() => {
                self.begin_cpt();
                self.notes.push(format!("Ri at {at}, CPt"));
            }
            _ => {}
        }
    }

    /// R-bar-i: with N = 0 the CPt series ends here (A10).
    fn ri_bar(&mut self, at: u64) {
        if self.stage == Stage::Cpt && self.zero_dil_a10() {
            self.source.stop = true;
            self.notes.push(format!("R-bar-i at {at}, CPt ends"));
        }
    }

    /// Whether this call is in A10's order: no DIL asked for, and that order
    /// taken (see [`ZERO_DIL_A10_ORDER`]).
    fn zero_dil_a10(&self) -> bool {
        ZERO_DIL_A10_ORDER && self.settings.descriptor.is_empty()
    }

    fn timers(&mut self) {
        if matches!(self.status, Status::Failed(_) | Status::Phase3Done) {
            return;
        }
        if let Some(at) = self.sd_deadline
            && self.now > at
        {
            self.sd_deadline = None;
            // AR1, which carries no round-trip term.
            self.fail("no Sd reversal within 1500 ms of Ja");
            return;
        }
        if let Some(at) = self.jd_deadline
            && self.now > at
            && self.far_jd.is_none()
        {
            self.jd_deadline = None;
            self.fail("no Jd within 4500 ms of Ja");
            return;
        }
        if let Some(at) = self.su_by
            && self.now > at
            && self.far_jd.is_none()
        {
            self.su_by = None;
            self.fail("no Jd within 5000 ms of the silence after Ja");
            return;
        }
        if let Some(at) = self.cpt_by
            && self.now > at
            && self.stage != Stage::Cpt
        {
            self.cpt_by = None;
            self.fail("no CPt within 5000 ms of the S-bar-u after Jp");
            return;
        }
        if self.now > self.start_up_by {
            self.fail("no B1d within 20 s and six round trips");
        }
    }

    /// A deadline went by. 9.5.2.2 says this end retrains here, which is the
    /// retrain ladder's business; until then the failure is named and the line
    /// goes quiet, which is where a retrain begins anyway (9.7.2.1: silence for
    /// 70 ms, then tone A).
    fn fail(&mut self, why: &'static str) {
        self.notes.push(format!("V.92 analogue: {why}"));
        self.status = Status::Failed(why);
        self.source.start(Up::Silence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::UP_RATES;

    fn settings(descriptor: Descriptor) -> Settings {
        Settings::new(descriptor, (1 << UP_RATES) - 1, Cp { drn: 16, constellations: vec![(1u128 << 40) - 1], ..Cp::default() })
    }

    /// A1: the opening is 70 ms of silence, 384T of Ru, 24T of its bar, and then
    /// TRN1u -- which 8.5.7 wants in whole twelve-symbol frames.
    #[test]
    fn the_opening_is_the_printed_lengths() {
        assert_eq!((LEAD_IN * FS) as u64, 560);
        assert_eq!((RU_SYMBOLS, RU_BAR_SYMBOLS), (384, 24));
        assert_eq!(TRN1U_MINIMUM % UP_INTERVALS as u64, 0, "TRN1u is whole frames");
        // 276-symbol MD steps are 23 frames, so an MD block keeps the frame grid
        // too.
        assert_eq!(md_symbols(3) % UP_INTERVALS, 0);
    }

    /// A9: the TRN1u over the DIL is at least 2040T, at least as long as the
    /// DIL, and a whole number of twelve-symbol frames.
    #[test]
    fn the_trn1u_over_the_dil_is_whole_frames_and_long_enough() {
        let mut modem = Modem::new(settings(Descriptor::none()));
        assert_eq!(modem.trn1u_over_dil(), TRN1U_MINIMUM, "no DIL asks for the minimum");

        // A DIL of six symbols still costs 2040T, and one longer than that is
        // rounded up to the next frame.
        let mut descriptor = Descriptor::none();
        descriptor.ucodes = vec![0x10];
        modem.settings.descriptor = descriptor.clone();
        assert_eq!(descriptor.len(), 6);
        assert_eq!(modem.trn1u_over_dil(), TRN1U_MINIMUM);

        // Twenty segments training chord 1 with H1 = 3 are 4 x 6 = 24 symbols
        // each, so 480 in all -- longer than 2040T? No: 480 is shorter, and the
        // minimum still wins. H1 = 0xff makes each segment 256 x 6 = 1536, and
        // then the DIL is the longer of the two.
        let mut descriptor = Descriptor { h: [0xff; 8], ..Descriptor::none() };
        descriptor.ucodes = vec![0x10; 20];
        modem.settings.descriptor = descriptor.clone();
        assert_eq!(descriptor.len(), 20 * 1536);
        assert_eq!(modem.trn1u_over_dil(), 30_720);
        assert!(modem.trn1u_over_dil() >= descriptor.len() as u64);
        assert_eq!(modem.trn1u_over_dil() % UP_INTERVALS as u64, 0);

        let mut descriptor = Descriptor { h: [3; 8], ..Descriptor::none() };
        descriptor.ucodes = vec![0x10; 20];
        modem.settings.descriptor = descriptor.clone();
        assert_eq!(descriptor.len(), 20 * 24);
        assert_eq!(modem.trn1u_over_dil(), TRN1U_MINIMUM, "a short DIL still costs 2040T");
    }

    /// Table 22 bits 18:33: epsilon is a "16-bit unsigned integer covering the
    /// range [0, 1) symbol", so the fraction is the field over 65 536, and
    /// nothing here turns it into extra symbols.
    #[test]
    fn epsilon_is_a_fraction_of_a_symbol() {
        let mut modem = Modem::new(settings(Descriptor::none()));
        assert_eq!(modem.epsilon(), None);
        modem.jp = Some(Jp { epsilon: 0, ..Jp::default() });
        assert_eq!(modem.epsilon(), Some(0.0));
        modem.jp = Some(Jp { epsilon: 1 << 15, ..Jp::default() });
        assert_eq!(modem.epsilon(), Some(0.5));
        modem.jp = Some(Jp { epsilon: u16::MAX, ..Jp::default() });
        assert!(modem.epsilon().unwrap() < 1.0, "[0, 1) symbol");
        // 24T plus the fraction is still 24 symbols of pattern (8.5.6).
        assert_eq!(SU_BAR_SYMBOLS, 24);
        assert_eq!(SU_SYMBOLS % UP_INTERVALS as u64, 0);
    }

    /// D2 clocked from the wrong anchor is a fit against the wrong signal, so the
    /// first TRN1u is at least as long as the fit needs (8.5.7's 2040T against
    /// `v92::receiver::TRAIN_WINDOW` and the search the fit does either side of
    /// its anchor).
    #[test]
    fn the_first_trn1u_is_long_enough_for_the_fit() {
        assert!(TRN1U_MINIMUM >= TRAIN_AFTER + TRAIN_SEARCH, "{} symbols", TRN1U_MINIMUM);
        // The retry moves the anchor on by half of TRAIN_AFTER, as the digital
        // modem's own retry does, and there is still TRN1u left after it.
        assert!(TRN1U_MINIMUM >= TRAIN_AFTER + TRAIN_AFTER / 2 + TRAIN_SEARCH);
    }
}
