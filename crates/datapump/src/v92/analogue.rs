//! The V.92 analogue modem from Phase 3 on (9.5.2, 9.6.2): PCM going up as
//! well as coming down.
//!
//! V92-32 built this end as far as Jd, V92-36 as far as E1u, and V92-39 took it
//! through Phase 4 into data mode. Phase 3 is the Ru/MD/TRN1u/Ja opening, the
//! silence in which the digital modem's TRN1d trains this end's receiver, Jd,
//! Su, Jp, the second TRN1u while the DIL arrives, CPt and E1u. Phase 4 is
//! TRN2u, the SUVu/CPu exchange, E2u and B1u going up while TRN2d, SUVd, CPd,
//! Ed and B1d come down, and then data both ways.
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
//! Phase 4 (9.6.2.1) is two halves that only meet in the acknowledge bits:
//!
//! ```text
//! analogue sends   TRN2u >= 12000T or until an SUVd | SUVu ... | CPu | SUVu ...
//!                  SUVu' ... | E2u 12T (13T) | B1u 576T | data
//! digital sends    TRN2d >= 2040T | SUVd ... | CPd | SUVd' ... | Ed 12T | B1d 288T | data
//! ```
//!
//! * **Up**, TRN2u, SUVu, CPu and E2u are one four- or eight-point carrier
//!   whose scrambler is reset once, at TRN2u's first symbol, and whose sign
//!   differential runs on from E1u's last sign (8.7.2-8.7.6). What goes on it
//!   next is [`crate::v92::exchange::Exchange`]'s to say. B1u and the data
//!   after it are the 6.4 chain built from the digital modem's CPd, started
//!   from zero on the symbol after E2u (8.7.1).
//! * **Down**, TRN2d, SUVd, CPd and Ed are V.90's 5.4 encoder on the
//!   constellations of the CPt this end sent (V.90 8.6.5 with 8.8.2, 8.8.3 and
//!   8.8.5), and B1d and the data after it the same encoder on the CPu's
//!   (V.90 8.6.1). They are read six symbols at a time with V.90's own
//!   decoder.
//!
//! Four things are deliberately not this module's:
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
//!   reading `v90::dil` does for V.90; the values are kept in [`Modem::dil`]
//!   exactly as the equaliser produced them. An owner that reads them sets
//!   [`Settings::choice_follows_dil`], and TRN1u then runs on past the DIL
//!   until [`Modem::choose`] hands over the CPt and the CPu it chose
//!   (9.5.2.1.11 gives it 5000 ms). Without that the CPt is the one the
//!   modem was built with, and the CPu offers the CPt's own constellations
//!   at the fastest rate they carry.
//! * **The retrain itself.** A deadline missed, a CPd that cannot be obeyed and
//!   the digital modem's Tone B all end in [`Status::Retrain`] with the line
//!   silent, circuit 106 OFF and circuit 104 clamped, which is where 9.7.2.1
//!   and 9.7.2.2 both begin. The 70 ms of silence, Tone A and the Phase 2 that
//!   follow are V.34's signals at the line rate, and this module works a
//!   symbol at a time; its owner sends them.
//! * **Clearing the call down.** A CPd of drn 0 is obeyed (Table 30, 9.11).
//!   Sending a CPu of drn 0 is the DTE's decision and nothing here takes it:
//!   an exchange this end cannot complete ends in a retrain (9.6.2.2), which
//!   leaves the owner the choice.
//!
//! The one thing here that is not a sequence is the sample phase. Jp's epsilon
//! and the half-symbol shift both exist because the digital modem cannot move
//! its A/D and this end can move its transmitter, and nothing in this module
//! generates samples, so both are recorded rather than applied.

use std::collections::VecDeque;

use crate::v32::{Mode, Scrambler};
use crate::v34::phase2::Role;
use crate::v34::training::RetrainWatch;
use crate::v90::INTERVALS;
use crate::v90::encoder::{Decoder as CodeDecoder, Frame as CodeFrame, Mapping as CodeMapping};
use crate::v90::sequences::{Cp, DOWNSTREAM_RATES, Descriptor, JD_BITS, Jd, Layout};
use crate::v90::ucode::{self, Law};
use crate::v92::exchange::{Context, Exchange};
use crate::v92::receiver::{Heard, Receiver, Symbol};
use crate::v92::sequences::{Cpd, DownFamily, DownFinder, J, JP_PRIME_BITS, Jp, Suvu, trn2u_unit};
use crate::v92::up_signals::{Trn1u, Trn2uSender, Trn2uSize, TwoPointReader, TwoPointSender, ru, su};
use crate::v92::upstream::UpEncoder;
use crate::v92::{
    FilterLimits, Parameters, PeerCp, PeerSuv, SequenceKind, UP_INTERVALS, ja_mask_has, md_symbols, up_bits,
};

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
// Phase 4's lengths (8.7, 8.8, 9.6.2.1)
// ---------------------------------------------------------------------------

/// "it has transmitted TRN2u for at least 12000T or received an SUVd sequence"
/// (9.6.2.1.1). Twelve thousand is a thousand twelve-symbol frames, which
/// keeps 8.7.6's "TRN2u shall be an integer multiple of 12 symbols in length".
const TRN2U_MINIMUM: u64 = 12_000;

/// E2u is "a data frame" (8.7.2), and an upstream data frame "is 12 symbols
/// long" (8.7.1). CPd bit 29 adds one symbol to it.
const E2U_SYMBOLS: usize = UP_INTERVALS;

/// "Ed consists of 2 data frames of scrambled binary zeros" (8.8.2), and a
/// downstream data frame is six symbols.
const ED_FRAMES: usize = 2;

/// "B1d consists of 48 data frames of scrambled ones" (V.90 8.6.1, which 8.8.1
/// adopts whole).
const B1D_FRAMES: usize = 48;

/// How many of B1d's forty-eight frames have to read as the ones 8.6.1/V.90
/// says they are for B1d to count as received (9.6.2.1.6).
///
/// The Recommendation gives no figure: it has circuit 104 unclamped "after
/// receiving B1d" and says nothing of a B1d that arrives damaged. Three
/// quarters lets a burst through without calling a line that decodes to
/// noise a data mode. It is not what places B1d's first frame: that is
/// [`B1D_LOCK_FRAMES`]. Fewer than this is B1d not received, and B1d is sent
/// once, so the answer is a retrain (9.6.2.2) [`B1D_GRACE`] later and not a
/// wait for 9.6.2.2.1's watchdog.
const B1D_CLEAN: usize = 36;

/// Whole frames of ones, read from zeroed memories, that say B1d began on the
/// first of them.
///
/// "The symbols in the first data frame of B1d shall have the magnitudes
/// resulting from mapping the first D scrambled ones after initializing the
/// scrambler to zero" (V.90 8.6.1): so a reader with a zeroed descrambler and
/// a zeroed differential decoder reads D ones from B1d's first frame and from
/// no other place. Started a frame early or late its descrambler is twenty-
/// three bits behind the scrambler's state, and the frame comes out as
/// something else. Two frames are at least forty-two bits (D is 21 at the
/// slowest rate of Table 2/V.90), which nothing else on the line makes by
/// chance.
const B1D_LOCK_FRAMES: usize = 2;

/// How long after a B1d that did not read as one the retrain is asked for.
///
/// What took B1d's place may be the digital modem's Tone B, and 9.7.2.2
/// makes "more than 50 ms" of it the detection: its forty-eight frames are
/// 36 ms, so the count can fail before the tone has stood long enough to be
/// known for what it is. An eighth of a second lets the better-informed
/// reason arrive first; the retrain it asks for is the same either way.
const B1D_GRACE: f64 = 0.125;

/// Frames after an Ed that was read by its zeros (8.8.2) in which B1d's own
/// first frame is still looked for, before the frame after Ed is taken to be
/// it whatever it read as.
///
/// With a far end that keeps 8.8.2 the two agree and the question does not
/// arise. It arises when B1d's first frames are damaged, where the place Ed
/// gave is the only one there is; and when Ed was longer than its two frames
/// or had a frame of fill in front of it, where B1d's own start is found a
/// frame or two later and wins.
const B1D_HUNT_FRAMES: usize = 6;

/// Descrambled ones, in whole data frames one after another, that say where
/// TRN2d's six-symbol frames fall.
///
/// TRN2d is "scrambled binary ones" through the encoder of 5.4 (V.90 8.6.5),
/// whose modulus encoder spreads K bits over the frame's six symbols, so six
/// symbols read across a frame boundary make K bits of nothing in particular.
/// Ninety-six ones is four frames of a CPt at 32 000 bit/s, and no wrong
/// alignment produces that many by chance.
const LOCK_BITS: usize = 96;

/// Which way up the equaliser hands over what the digital modem sent.
///
/// The fit that trained it took TRN1d against a reference in the upstream
/// convention, "a scrambler output of 0 represents a positive voltage"
/// (8.5.7), which is the one `v92::receiver` is written in. Downstream is the
/// other way round, "a sign of 0 represents a negative voltage" (8.6.2), so
/// the fit came out inverted and a positive codeword arrives as a negative
/// value. Phase 3 never noticed, because every bit it reads is differential.
/// The 5.4 decoder's first symbol after a reset is not: its differential
/// memory starts at zero (V.90 8.6.1, 8.6.5), so there the sign itself counts.
const DOWN_SIGN: f64 = -1.0;
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

/// "If the analogue modem does not receive B1d within 20 s plus 6 round-trip
/// delays from the end of sending INFO1a, the analogue modem shall initiate a
/// retrain" (9.6.2.2.1): the timer that covers all of Phase 3 and Phase 4.
const START_UP: f64 = 20.0;
const START_UP_RTD: f64 = 6.0;

/// The U_INFO [`Settings::new`] assumes, which is the one this crate's digital
/// modem assumes when it is built by default, so that two ends nobody told
/// otherwise agree. A call that went through Phase 2 has the one its own
/// INFO1a carried (Table 18).
pub const UINFO: u8 = 79;

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
    ///
    /// One thing is asked of it. This end has to answer Tone B in Phase 3
    /// (9.5.2.2), so a DIL must not be one: a sign pattern that repeats every
    /// twenty symbols is a square wave at 400 Hz, whose third harmonic is
    /// 1200 Hz, and a DIL-segment of it longer than 50 ms would be taken for
    /// the tone. The patterns "are restarted at the beginning of each
    /// DIL-segment" (V.90 8.4.1), so segments shorter than that are safe
    /// whatever their pattern.
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
    /// The data mode constellations and shaping the CPu is to ask for (8.7.3,
    /// Table 23), when they are known before the call: the `data` half of a
    /// `v90::dil::choose`. Its rate is kept if Jd enabled it and its
    /// constellations carry it; otherwise the CPu asks for the fastest rate
    /// that both allow.
    ///
    /// `None` offers the CPt's own constellations for data mode, which is all
    /// an end that asked for no DIL has measured. A choice made from this
    /// call's own DIL arrives later, through [`Modem::choose`].
    pub cpu: Option<Cp>,
    /// Whether the CPt and the CPu are to come from this call's DIL
    /// (9.5.2.1.11: CPt "indicates to the digital modem that the analogue
    /// modem has received enough of the DIL sequence").
    ///
    /// Set, the TRN1u that covers the DIL runs on, in whole twelve-symbol
    /// frames, until the owner has read [`Modem::dil`] and called
    /// [`Modem::choose`]; the 5000 ms of 9.5.2.1.11 is what bounds the wait.
    /// Clear, CPt follows the DIL at once and is [`Self::cpt`]. With no DIL
    /// asked for there is nothing to wait for and this changes nothing.
    pub choice_follows_dil: bool,
    /// The round trip, from Phase 2's ranging: the deadlines that carry an RTD
    /// term use it.
    pub round_trip: f64,
    /// Seconds between the end of sending INFO1a and this modem being built,
    /// which 9.6.2.2.1's watchdog is counted from. Zero for an owner that
    /// builds it as INFO1a ends.
    pub since_info1a: f64,
    /// The far codec's law, which is what turns a Ucode into a level
    /// (Table 1/V.90) and what CPu bit 35 reports.
    pub law: Law,
    /// The U_INFO this end's INFO1a named (Table 18): TRN1d, Ri and the DIL's
    /// reference are sent at it, so it is the level the equaliser was trained
    /// to call one.
    pub uinfo: u8,
    /// What INFO1a bits 12:17 said this end's precoder and prefilter can hold
    /// (Table 18). A CPd asking for more is one this end cannot obey.
    pub limits: FilterLimits,
}

impl Settings {
    /// Settings for a call that asks for `descriptor` and offers
    /// `upstream_rates`, with `cpt` naming the downstream rate this end wants
    /// and no MD, on a mu-law network with [`UINFO`] and INFO1a's smallest
    /// filter limits.
    #[must_use]
    pub fn new(descriptor: Descriptor, upstream_rates: u32, cpt: Cp) -> Self {
        Self {
            descriptor,
            upstream_rates,
            md_length: 0,
            cpt,
            cpu: None,
            choice_follows_dil: false,
            round_trip: 0.0,
            since_info1a: 0.0,
            law: Law::Mu,
            uinfo: UINFO,
            limits: FilterLimits::from_info1a(0, 0, 0),
        }
    }
}

/// The CPt an end that asked for no DIL sends: 48 000 bit/s (drn 16) on
/// Ucodes 16 to 47 in every interval.
///
/// With no DIL nothing has been measured, so this is a guess, and a careful
/// one: Ucodes below 16 are a few LSBs of the codec apart and a resampler's
/// noise alone confuses them, and 32 points carry drn 16's six bits a symbol
/// with room to spare (Table 14/V.90).
#[must_use]
pub fn readable_cpt() -> Cp {
    let mask = (16..=47u8).fold(0u128, |mask, u| mask | 1 << u);
    Cp { drn: 16, constellations: vec![mask], ..Cp::default() }
}

/// How the start-up is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// E1u has gone out: Phase 3 is over and TRN2u is on the line (9.6.2.1.1).
    Phase4,
    /// Data mode both ways: B1u has been sent, so circuit 106 follows 105
    /// (9.6.2.1.5), and B1d has been received, so circuit 104 is unclamped and
    /// 109 is ON (9.6.2.1.6). The rates are in bit/s.
    Connected { downstream: u32, upstream: u32 },
    /// The digital modem's CPd said drn 0 (Table 30, 9.11).
    ClearedDown,
    /// This end has to retrain, and why: circuit 106 is OFF, circuit 104 is
    /// clamped and the line is silent, which is where 9.7.2.1 (this end's own
    /// reasons) and 9.7.2.2 (the digital modem's Tone B) both begin. Circuit
    /// 109 is as it was: neither clause moves it. The owner sends the rest.
    Retrain(&'static str),
    /// A Phase 3 deadline went by. A live modem retrains here as well
    /// (9.5.2.2); it is kept apart because nothing of Phase 4 had started.
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
    /// TRN2u and everything that rides its modulation: SUVu, CPu and E2u
    /// (8.7.2-8.7.6).
    Trn2u,
    /// B1u, 48 data frames of scrambled ones through the 6.4 chain (8.7.1).
    B1u,
    /// Data (6.4).
    Data,
}

/// The upstream, one symbol at a time.
///
/// It holds the two modulators Phase 3 uses -- the TRN1u (and MD) scrambler, and
/// the two-point differential carrier that Ja, CPt and E1u ride -- because
/// nothing between them resets anything: the GPA scrambler runs on from TRN1u
/// through Ja, and the differential encoder's memory is seeded from TRN1u's
/// final symbol (8.5.1, 8.5.4, 8.5.7). Phase 4 adds the two it uses: TRN2u's
/// carrier, and the data mode encoder.
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
    /// TRN2u's carrier, from its first symbol to E2u's last. One instance,
    /// because nothing between TRN2u, SUVu, CPu and E2u resets the scrambler
    /// or the sign differential (8.7.2, 8.7.3, 8.7.5): an empty queue is
    /// TRN2u's own scrambled ones.
    trn2u: Option<Trn2uSender>,
    /// B1u's symbols still to go, already through the 6.4 chain.
    b1u: VecDeque<f64>,
    /// The data mode encoder, built from the CPd (6.4). Its memories run on
    /// from B1u into the data.
    encoder: Option<UpEncoder>,
    /// What circuit 103 has handed over and the encoder has not taken yet.
    outbound: VecDeque<bool>,
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
            trn2u: None,
            b1u: VecDeque::new(),
            encoder: None,
            outbound: VecDeque::new(),
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
            Up::Trn2u => self.trn2u.as_mut().map_or(0.0, Trn2uSender::next_symbol),
            // 6.4.2: "Finally the output v(n) is multiplied by a gain, G",
            // which is `out`. It is in the LU units TRN1u and TRN2u are in
            // because 8.8.3 has the digital modem choose G "assuming that,
            // when the prefilter output multiplied by G has a mean-square
            // value of 1, the analogue modem will transmit at the desired
            // power" (3.8).
            Up::B1u => self.b1u.pop_front().unwrap_or(0.0),
            Up::Data => {
                let outbound = &mut self.outbound;
                // Circuit 106 follows 105 from here (9.6.2.1.5). With nothing
                // to send, circuit 103 rests at binary one, and the scrambler
                // makes a signal of it.
                self.encoder.as_mut().map_or(0.0, |encoder| encoder.next_symbol(|| outbound.pop_front().unwrap_or(true)).out)
            }
        }
    }

    /// Whether the two-point carrier has nothing left in flight.
    fn drained(&self) -> bool {
        self.sender.is_empty()
    }
}

// ---------------------------------------------------------------------------
// What comes down in Phase 4 and after it
// ---------------------------------------------------------------------------

/// The downstream, one six-symbol data frame at a time: the slicer, V.90 5.4's
/// decoder and the descrambler, for one set of constellations.
///
/// A new one is built wherever the digital modem starts its coding again --
/// TRN2d and B1d both begin with "the scrambler, differential encoder and
/// spectral shape filter memory ... initialized to zero" (V.90 8.6.5, 8.6.1)
/// -- so there is no reset.
#[derive(Debug, Clone)]
struct FrameReader {
    /// Each interval's constellation as its Ucodes and their levels, in
    /// fractions of full scale (Table 1/V.90).
    points: [Vec<(u8, f64)>; INTERVALS],
    decoder: CodeDecoder,
    /// GPC, 1 + x^-18 + x^-23: the digital modem's polynomial. Clause 5 makes
    /// its scrambler "the same as those given in clause 5/V.90", and
    /// 5.3/V.90 names "the generating polynomial, GPC, in equation
    /// 7-1/V.34".
    descrambler: Scrambler,
    ucodes: [u8; INTERVALS],
    positive: [bool; INTERVALS],
    /// How far into the frame the next symbol is.
    at: usize,
    /// Symbols to let go by before the first frame begins.
    skip: usize,
}

impl FrameReader {
    /// A reader for what a CPt or a CPu asks the digital modem to send, whose
    /// first frame begins `skip` symbols from now. `None` if the rate is more
    /// than the constellations carry (5.4.3/V.90).
    fn new(cp: &Cp, law: Law, skip: usize) -> Option<Self> {
        let mapping = CodeMapping::from_cp(cp)?;
        Some(Self {
            points: std::array::from_fn(|i| cp.points(i).into_iter().map(|u| (u, ucode::level(law, u))).collect()),
            decoder: CodeDecoder::new(mapping),
            descrambler: Scrambler::new(Mode::Call),
            ucodes: [0; INTERVALS],
            positive: [false; INTERVALS],
            at: 0,
            skip,
        })
    }

    /// One symbol, as a signed fraction of full scale; and at the frame's
    /// sixth, the D descrambled bits the frame carried.
    fn feed(&mut self, level: f64) -> Option<Vec<bool>> {
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        // 5.4.4/V.90 backwards: the point of this interval's constellation
        // nearest the magnitude that arrived.
        let magnitude = level.abs();
        let nearest = self.points[self.at]
            .iter()
            .min_by(|a, b| (a.1 - magnitude).abs().total_cmp(&(b.1 - magnitude).abs()));
        self.ucodes[self.at] = nearest.map_or(0, |point| point.0);
        self.positive[self.at] = level >= 0.0;
        self.at += 1;
        if self.at < INTERVALS {
            return None;
        }
        self.at = 0;
        let frame = CodeFrame { ucodes: self.ucodes, positive: self.positive };
        let bits = self.decoder.frame(frame);
        Some(bits.into_iter().map(|bit| self.descrambler.descramble(bit)).collect())
    }
}

/// A reader started from zero on a frame boundary, on the chance that B1d
/// begins there (V.90 8.6.1).
#[derive(Debug, Clone)]
struct Probe {
    reader: FrameReader,
    /// Which frame since TRN2d was found this one's first is.
    born: usize,
    /// Whole frames read so far, and how many of them were all ones.
    frames: usize,
    clean: usize,
    /// Whether this is the frame after an Ed that was read by its zeros, which
    /// is kept whatever it reads as: it is the only place there is for a B1d
    /// whose first frames are damaged ([`B1D_HUNT_FRAMES`]).
    after_ed: bool,
}

impl Probe {
    /// Whether every frame so far has been ones, from the first.
    fn unbroken(&self) -> bool {
        self.clean == self.frames
    }
}

/// TRN2d has been found, and what follows it is being read.
#[derive(Debug, Clone)]
struct Training {
    /// SUVd, CPd and Ed ride TRN2d's modulation and its scrambler (8.8.2,
    /// 8.8.3, 8.8.5), so one reader carries on through all of them.
    reader: FrameReader,
    finder: DownFinder,
    /// Whether a CPd has been read. Ed follows an acknowledged sequence in
    /// each direction (9.6.1.1.4), and this end acknowledges nothing before
    /// it has a CPd (9.6.2.1.2), so before one there is no B1d to look for.
    cpd_read: bool,
    /// Whether the frame before this one ended a sequence, or was a frame of
    /// zeros that followed one.
    boundary: bool,
    /// Frames of zeros counted from that boundary.
    zeros: usize,
    /// Whole frames read since TRN2d was found.
    frames: usize,
    /// The frame after Ed, once Ed has been read by its zeros.
    after_ed: Option<usize>,
    /// The places B1d might have begun.
    probes: Vec<Probe>,
}

/// Where the downstream reader has got to.
#[derive(Debug, Clone)]
enum DownState {
    /// R-bar-i has been heard and TRN2d is due: six readers, one for each
    /// place a six-symbol frame could begin, each with the descrambled ones
    /// it has read in whole frames without a break.
    Hunting(Vec<(FrameReader, usize)>),
    /// TRN2d found: SUVd, CPd, Ed, and the first frames of B1d.
    Training(Box<Training>),
    /// B1d's first frame has been placed: the rest of it, on the CPu's
    /// constellations (9.6.2.1.6).
    B1d { reader: Box<FrameReader>, frames: usize, clean: usize },
    /// B1d has gone by: data.
    Data(Box<FrameReader>),
    /// What the state is while it is being worked on, and never otherwise.
    Lost,
}

/// What the downstream reader has to report.
#[derive(Debug, Clone)]
enum DownEvent {
    /// TRN2d's frames have been found.
    Found,
    /// A CRC-good SUVd or CPd.
    Sequence(Box<DownFamily>),
    /// Ed, read as its two frames of zeros where a sequence ended (8.8.2).
    Ed,
    /// B1d's first frame has been placed. `ed_read` is whether Ed had been
    /// reported before it, and `late` how many frames after that Ed's two it
    /// began.
    B1dBegun { ed_read: bool, late: usize },
    /// B1d's forty-eight frames have gone by, this many of them scrambled
    /// ones.
    B1d { clean: usize },
    /// One data frame's bits.
    Data(Vec<bool>),
}

/// The downstream from TRN2d on (9.6.2.1.1, 9.6.2.1.6).
#[derive(Debug, Clone)]
struct Downstream {
    /// A reader for B1d with every memory zero, on the constellations of the
    /// CPu this end sends: what each [`Probe`] starts as.
    fresh: FrameReader,
    state: DownState,
}

impl Downstream {
    /// A reader looking for TRN2d on the constellations of `cpt`, which will
    /// read B1d and the data on those of `cpu`. `None` if either names a
    /// rate its own constellations do not carry (5.4.3/V.90), which leaves
    /// Phase 4 with nothing that can be read.
    fn hunting(cpt: &Cp, cpu: &Cp, law: Law) -> Option<Self> {
        // TRN2d begins on a frame boundary 24T after the Ri-to-R-bar-i
        // reversal (V.90 8.6.4: R-bar-i is four repetitions of six symbols),
        // but the reversal is only placed to within a period, which is one
        // whole frame: so every one of the six alignments is tried.
        let candidates: Option<Vec<_>> =
            (0..INTERVALS).map(|skip| FrameReader::new(cpt, law, skip).map(|reader| (reader, 0))).collect();
        Some(Self { fresh: FrameReader::new(cpu, law, 0)?, state: DownState::Hunting(candidates?) })
    }

    /// Whether TRN2d's frames have been found.
    fn found(&self) -> bool {
        !matches!(self.state, DownState::Hunting(_))
    }

    /// One symbol of what follows TRN2d, until B1d's first frame is placed.
    ///
    /// Two things are read side by side, because 9.6.2.1.6's "after receiving
    /// Ed, the analogue modem shall condition its receiver to receive B1d"
    /// leaves no symbol between them and Ed is two frames long:
    ///
    /// * **Ed**, on the CPt's constellations, as 8.8.2's "2 data frames of
    ///   scrambled binary zeros" in the place where a sequence ended. Only
    ///   there, because a CPd holds runs of zeros longer than a frame -- a
    ///   filter section of no coefficients is a word of them. That makes it
    ///   depend on the sequence before Ed having been read, and the digital
    ///   modem sends exactly one more after the acknowledgement (9.6.1.1.4).
    /// * **B1d**, on the CPu's, from every frame boundary: V.90 8.6.1 zeroes
    ///   the scrambler and the differential encoder before its first frame,
    ///   so a reader that starts from zero reads D ones there and nowhere
    ///   else ([`B1D_LOCK_FRAMES`]). This needs nothing of Ed, so one damaged
    ///   sequence does not cost the call; and it is B1d's own evidence of
    ///   where B1d begins, so an Ed of the wrong length does not move it.
    fn feed_training(&self, mut training: Box<Training>, level: f64) -> (DownState, Option<DownEvent>) {
        let frame = training.reader.feed(level);
        // Every probe began on a frame boundary of this reader's, so they all
        // finish a frame on the symbol it does.
        for probe in &mut training.probes {
            if let Some(bits) = probe.reader.feed(level) {
                probe.frames += 1;
                probe.clean += usize::from(bits.iter().all(|&bit| bit));
            }
        }
        let Some(bits) = frame else {
            return (DownState::Training(training), None);
        };
        training.frames += 1;
        training.probes.retain(|probe| probe.unbroken() || probe.after_ed);
        // B1d's own first frame, if one of them has found it; failing that,
        // the frame after Ed, once it has had its chance.
        let begun = training
            .probes
            .iter()
            .position(|probe| probe.unbroken() && probe.frames >= B1D_LOCK_FRAMES)
            .or_else(|| training.probes.iter().position(|probe| probe.after_ed && probe.frames >= B1D_HUNT_FRAMES));
        if let Some(k) = begun {
            let probe = training.probes.swap_remove(k);
            let late = training.after_ed.map_or(0, |after_ed| probe.born.saturating_sub(after_ed));
            let state = DownState::B1d { reader: Box::new(probe.reader), frames: probe.frames, clean: probe.clean };
            return (state, Some(DownEvent::B1dBegun { ed_read: training.after_ed.is_some(), late }));
        }
        let mut event = None;
        // Whether the frame that starts on the next symbol is the one after
        // an Ed read by its zeros.
        let mut after_ed = false;
        if training.after_ed.is_none() {
            let mut found = None;
            for &bit in &bits {
                if let Some(family) = training.finder.feed(bit) {
                    found = Some(family);
                }
            }
            if let Some(family) = found {
                // The finder answers at the sequence's own fill bit (Tables
                // 30 and 31), and the fill after it runs "to the next
                // multiple of 6 symbols": so the sequence ends with the frame
                // that fill bit is in, which is this one.
                training.cpd_read |= matches!(family, DownFamily::Cpd(_));
                training.boundary = true;
                training.zeros = 0;
                event = Some(DownEvent::Sequence(Box::new(family)));
            } else if training.boundary && bits.iter().all(|&bit| !bit) {
                training.zeros += 1;
                if training.zeros >= ED_FRAMES && training.cpd_read {
                    training.after_ed = Some(training.frames);
                    after_ed = true;
                    event = Some(DownEvent::Ed);
                }
            } else {
                training.boundary = false;
                training.zeros = 0;
            }
        }
        if training.cpd_read {
            let born = training.frames;
            training.probes.push(Probe { reader: self.fresh.clone(), born, frames: 0, clean: 0, after_ed });
        }
        (DownState::Training(training), event)
    }

    /// One symbol, as a signed fraction of full scale.
    fn feed(&mut self, level: f64) -> Option<DownEvent> {
        let (state, event) = match std::mem::replace(&mut self.state, DownState::Lost) {
            DownState::Hunting(mut candidates) => {
                let mut found = None;
                for (k, (reader, ones)) in candidates.iter_mut().enumerate() {
                    if let Some(bits) = reader.feed(level) {
                        *ones = if bits.iter().all(|&bit| bit) { *ones + bits.len() } else { 0 };
                        if *ones >= LOCK_BITS {
                            found = Some(k);
                        }
                    }
                }
                match found {
                    Some(k) => {
                        let (reader, _) = candidates.swap_remove(k);
                        let training = Training {
                            reader,
                            // The seventeen ones a frame sync is made of are
                            // TRN2d's own, however long it has been running:
                            // the finder takes the last seventeen of any run.
                            finder: DownFinder::new(),
                            cpd_read: false,
                            boundary: false,
                            zeros: 0,
                            frames: 0,
                            after_ed: None,
                            probes: Vec::new(),
                        };
                        (DownState::Training(Box::new(training)), Some(DownEvent::Found))
                    }
                    None => (DownState::Hunting(candidates), None),
                }
            }
            DownState::Training(training) => self.feed_training(training, level),
            DownState::B1d { mut reader, frames, clean } => match reader.feed(level) {
                None => (DownState::B1d { reader, frames, clean }, None),
                Some(bits) => {
                    let clean = clean + usize::from(bits.iter().all(|&bit| bit));
                    if frames + 1 < B1D_FRAMES {
                        (DownState::B1d { reader, frames: frames + 1, clean }, None)
                    } else {
                        // The scrambler and the differential encoder run on
                        // from B1d into the data (V.90 8.6.1), so the reader
                        // does too.
                        (DownState::Data(reader), Some(DownEvent::B1d { clean }))
                    }
                }
            },
            DownState::Data(mut reader) => {
                let bits = reader.feed(level);
                (DownState::Data(reader), bits.map(DownEvent::Data))
            }
            DownState::Lost => (DownState::Lost, None),
        };
        self.state = state;
        event
    }
}

// ---------------------------------------------------------------------------
// Where the state machine has got to
// ---------------------------------------------------------------------------

/// Where the start-up has got to.
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
    /// Phase 4, from TRN2u's first symbol to B1u's last (9.6.2.1.1-9.6.2.1.5).
    Phase4,
    /// B1u has gone out and what follows it is data (9.6.2.1.5).
    Data,
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
    /// "The digital modem shall use the polynomial, GPC, in equation
    /// 7-1/V.34 when generating signals Jd, Jp, Jp', SCR and TRN1d" (8.6).
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
    /// Whether [`Modem::choose`] has handed over the CPt and the CPu
    /// ([`Settings::choice_follows_dil`]).
    chosen: bool,
    /// Whether Ri has been heard with no DIL asked for, so that CPt is due
    /// at the next twelve-symbol boundary of the TRN1u on the line
    /// (9.5.2.1.9, 9.5.2.1.10).
    ri_heard: bool,
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
    /// The level U_INFO stands for, as a fraction of full scale: the
    /// equaliser was trained to call it one, so this turns its output back
    /// into the scale Table 1/V.90's Ucodes are on.
    unit: f64,
    /// The downstream from TRN2d on, once R-bar-i has been heard.
    down: Option<Downstream>,
    /// 9.6.2.1.2-9.6.2.1.4: the SUV/CP/E exchange, which decides what goes
    /// out on TRN2u's carrier next and when the handshake is done. It is here
    /// from the start so that nothing received before this end's own SUVu is
    /// lost.
    exchange: Exchange,
    /// The sequence on TRN2u's carrier now, or `None` while TRN2u itself is.
    in_flight: Option<SequenceKind>,
    /// Whether a CRC-good SUVd has arrived, which ends TRN2u early
    /// (9.6.2.1.1).
    suvd_heard: bool,
    /// The CPu this end sends: one for the whole exchange, because a group of
    /// CPu and CPu' "shall all contain identical information" but for the
    /// acknowledge bit (8.7.3).
    cpu: Option<Cp>,
    /// The digital modem's CPd as it now stands, each later one merged over
    /// the ones before it (8.8.3), and the parameters it comes to.
    cpd: Option<Cpd>,
    parameters: Option<Parameters>,
    /// The rate the CPu's constellations carry, in bit/s.
    downstream: u32,
    /// Tone B, 1200 Hz, which is the digital modem retraining (9.6.2.2).
    retrain_watch: RetrainWatch,
    /// Circuit 106: ON once B1u has been sent (9.6.2.1.5), OFF again for a
    /// retrain (9.7.2.1, 9.7.2.2).
    circuit106: bool,
    /// Whether circuit 104 has been let go. It is unclamped once B1d has been
    /// received (9.6.2.1.6) and clamped to binary one again for a retrain
    /// (9.7.2.1, 9.7.2.2).
    circuit104: bool,
    /// Circuit 109: ON once B1d has been received (9.6.2.1.6). It is not
    /// 104's shadow: 9.7.2.1 and 9.7.2.2 clamp 104 and say nothing of 109,
    /// so a retrain leaves it as it was.
    circuit109: bool,
    /// When a B1d that did not read as B1d went by, which is answered with a
    /// retrain a little later ([`B1D_GRACE`]).
    b1d_failed: Option<u64>,
    /// Data bits received since circuit 104 was unclamped, for whoever asks.
    recovered: Vec<bool>,
    /// The downstream as the equaliser gave it, for a scope.
    scope: crate::v92::ScopePair,
    status: Status,
    notes: Vec<String>,
}

impl Modem {
    /// From the end of Phase 2: silent for 70 ms, then Ru.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        let mut rx = Receiver::new();
        // The digital modem's TRN1d is generated with GPC (8.6), not the GPA
        // an analogue modem's own TRN1u uses (6.3, 8.5.7).
        rx.set_reference_mode(Mode::Call);
        // A3 asks for Sd and its reversal, and Sd is Su's shape the other way
        // up (V.90 8.4.4): between the 70 ms of silence and the DIL nothing
        // else of that shape is due.
        rx.expect_su();
        // 9.6.2.2.1 counts from the end of sending INFO1a, which may be a
        // little before this modem was built.
        let start_up = START_UP + START_UP_RTD * settings.round_trip - settings.since_info1a;
        let start_up_by = (start_up.max(0.0) * FS) as u64;
        let mut exchange = Exchange::new(Context::Training, settings.round_trip, FS);
        // 9.6.2.1.4: "complete sending the current CPu sequence and transmit
        // E2u". Nothing is held on the line in between.
        exchange.hold_before_e(0);
        Self {
            unit: ucode::level(settings.law, settings.uinfo),
            down: None,
            exchange,
            in_flight: None,
            suvd_heard: false,
            cpu: None,
            cpd: None,
            parameters: None,
            downstream: 0,
            // Tone B, 1200 Hz, is the call modem's tone in V.34 and the digital
            // modem's here (9.7.1.1), so that is the role the watch is told
            // the far end has.
            retrain_watch: RetrainWatch::new(Role::Call, FS),
            circuit106: false,
            circuit104: false,
            circuit109: false,
            b1d_failed: None,
            recovered: Vec::new(),
            scope: crate::v92::ScopePair::default(),
            source: Source::new(&settings),
            settings,
            rx,
            reader: TwoPointReader::differential_with(Mode::Call),
            window: VecDeque::new(),
            zeroes: 0,
            jp_prime: false,
            dil_left: 0,
            dil: Vec::new(),
            chosen: false,
            ri_heard: false,
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
        // Once the line has been given up nothing of Phase 3 or Phase 4 is on
        // it, whatever stage the start-up had reached.
        match self.status {
            Status::Retrain(_) => return "V.92: silent, for a retrain",
            Status::ClearedDown => return "V.92: cleared down",
            Status::Failed(_) => return "V.92 phase 3: failed",
            Status::Running | Status::Phase4 | Status::Connected { .. } => {}
        }
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
            Stage::Phase4 => match (self.source.up, self.in_flight) {
                (Up::B1u, _) => "V.92 phase 4: B1u",
                (_, Some(SequenceKind::Cp)) => "V.92 phase 4: CPu",
                (_, Some(SequenceKind::E)) => "V.92 phase 4: E2u",
                (_, Some(_)) => "V.92 phase 4: SUVu",
                (_, None) => "V.92 phase 4: TRN2u",
            },
            Stage::Data => "V.92 data",
        }
    }

    /// The CPu this end sends, once Phase 4 has it (8.7.3).
    pub fn cpu(&self) -> Option<&Cp> {
        self.cpu.as_ref()
    }

    /// The digital modem's CPd as it stands, later ones merged over earlier
    /// (8.8.3), once one has been read.
    pub fn cpd(&self) -> Option<&Cpd> {
        self.cpd.as_ref()
    }

    /// What that CPd tells this end's transmitter to do (6.4).
    pub fn parameters(&self) -> Option<&Parameters> {
        self.parameters.as_ref()
    }

    /// Whether circuit 104 has been let go. Before B1d has been received it
    /// is clamped to binary one and nothing received reaches the DTE
    /// (9.6.2.1.6), and a retrain clamps it again (9.7.2.1, 9.7.2.2).
    #[must_use]
    pub fn circuit104(&self) -> bool {
        self.circuit104
    }

    /// Whether circuit 106 is ON, which it is once B1u has been sent
    /// (9.6.2.1.5): only then is anything from [`Self::send_bits`] taken.
    #[must_use]
    pub fn circuit106(&self) -> bool {
        self.circuit106
    }

    /// Whether circuit 109 is ON, which it is once B1d has been received
    /// (9.6.2.1.6). A retrain does not turn it OFF: 9.7.2.1 and 9.7.2.2 name
    /// circuits 106 and 104 only. A cleardown does, because the call and its
    /// line signal are over (9.11 names no circuit; this is the reading).
    #[must_use]
    pub fn circuit109(&self) -> bool {
        self.circuit109
    }

    /// Data for the far end, from circuit 103. It waits here until B1u has
    /// gone out, because until then circuit 106 is OFF (9.6.2.1.5).
    pub fn send_bits(&mut self, bits: &[bool]) {
        self.source.outbound.extend(bits.iter().copied());
    }

    /// The last two downstream symbols, each against the one after it, for
    /// a scope (see [`crate::v92::ScopePair`]).
    #[must_use]
    pub fn pair(&self) -> Option<(f64, f64)> {
        self.scope.pair()
    }

    /// How many points the downstream is being read against, both signs
    /// counted: two through Phase 3, the CPt's constellation through Phase 4
    /// (V.90 8.6.5) and the CPu's in data mode (V.90 8.6.1).
    #[must_use]
    pub fn points(&self) -> usize {
        let of = |cp: &Cp| 2 * cp.points(0).len();
        match self.status {
            Status::Connected { .. } => self.cpu().map_or(2, of),
            Status::Phase4 => of(&self.settings.cpt),
            _ => 2,
        }
    }

    /// How many of those bits the encoder has not taken yet.
    #[must_use]
    pub fn pending_bits(&self) -> usize {
        self.source.outbound.len()
    }

    /// How many bits the encoder takes in one go: K, a whole data frame's, at
    /// the frame's first symbol (6.4.1). A caller that keeps fewer queued has
    /// the rest of the frame made up with binary ones. Zero before data mode.
    #[must_use]
    pub fn fill_bits(&self) -> usize {
        match (self.stage, self.source.encoder.as_ref()) {
            (Stage::Data, Some(encoder)) if !self.stopped() => encoder.frame_bits() as usize,
            _ => 0,
        }
    }

    /// The data received since the last call. Nothing arrives before B1d has
    /// gone by: circuit 104 is clamped until then (9.6.2.1.6).
    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.recovered)
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

    /// The DIL's symbols as the equaliser produced them, for whoever reads
    /// the route out of them (see [`Self::choose`]).
    #[must_use]
    pub fn dil(&self) -> &[f64] {
        &self.dil
    }

    /// Whether a whole pass of the DIL that Ja asked for has arrived, which
    /// is when [`Self::dil`] is worth reading. Never true with no DIL asked
    /// for.
    #[must_use]
    pub fn dil_received(&self) -> bool {
        !self.settings.descriptor.is_empty() && self.dil_complete()
    }

    /// Hand over the CPt and the CPu chosen from this call's DIL: the
    /// `training` and `data` halves of a `v90::dil::choose`.
    ///
    /// 9.5.2.1.11 has CPt say "that the analogue modem has received enough of
    /// the DIL sequence", so it is the first thing the choice is needed for,
    /// and the CPu is the same reading's data mode half (8.7.3). They are
    /// taken until CPt's first symbol has gone out. After that the series on
    /// the line is that CPt and has to stay it: the digital modem builds
    /// TRN2d from whichever repetition it read (V.90 8.6.5).
    ///
    /// False, and nothing changed, if CPt has begun, or if `training` names
    /// a rate its own constellations do not carry (5.4.3/V.90): TRN2d could
    /// not then be read.
    pub fn choose(&mut self, training: Cp, data: Cp) -> bool {
        let before_cpt = matches!(
            self.stage,
            Stage::Opening | Stage::Ja | Stage::Training | Stage::Jd | Stage::Su | Stage::AfterJp | Stage::Dil
        );
        if !before_cpt || self.stopped() || CodeMapping::from_cp(&training).is_none() {
            return false;
        }
        // One bit a symbol (8.5.1), filled to whole twelve-symbol frames.
        self.source.cpt_bits = training.to_bits_in(Layout::V92, UP_INTERVALS);
        self.settings.cpt = training;
        self.settings.cpu = Some(data);
        self.chosen = true;
        true
    }

    /// How long this modem has been running, in symbols.
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
        self.watch_for_tone_b(from_digital);
        self.train();
        self.advance();
        self.timers();
        self.source.next()
    }

    /// Whether this modem has given the line up: a retrain is wanted, the
    /// call has been cleared down, or Phase 3 failed.
    fn stopped(&self) -> bool {
        matches!(self.status, Status::Failed(_) | Status::Retrain(_) | Status::ClearedDown)
    }

    /// 9.5.2.2: "If Tone B is detected during Phase 3, the analogue modem
    /// shall respond to retrain according to 9.7.2.2", 9.6.2.2 says the same
    /// of Phase 4, and 9.7.2.2 makes "more than 50 ms" of it the detection.
    /// Data mode is watched the same way: the digital modem's retrain begins
    /// with the same tone whenever it begins (9.7.1.1).
    ///
    /// Of what the digital modem sends in Phase 3, Sd and Ri are six-symbol
    /// patterns, 1333 Hz, which is one of the two neighbours the watch holds
    /// the tone against; and TRN1d, Jd, Jp and SCR are scrambled and fill
    /// the band. The DIL is whatever this end's own descriptor made it, and
    /// [`Settings::descriptor`] says what it must not be.
    fn watch_for_tone_b(&mut self, sample: f64) {
        if self.stopped() {
            return;
        }
        if self.retrain_watch.feed(sample, FS) {
            self.retrain("tone B: the digital modem is retraining");
        }
    }

    // -----------------------------------------------------------------------
    // The timetable: what goes out next
    // -----------------------------------------------------------------------

    /// Where each signal ends, and what follows it (9.5.2.1's A1-A11).
    fn advance(&mut self) {
        // A line that has been given up stays silent: 9.7.2.1's 70 ms is the
        // owner's to time, and the opening's own 70 ms of silence must not be
        // taken for it and followed by Ru.
        if self.stopped() {
            return;
        }
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
                } else if self.source.count.is_multiple_of(UP_INTERVALS as u64) {
                    // A9-A11. "The length for this segment of TRN1u shall be
                    // in multiple of 12 symbols" (9.5.2.1.9, and 8.5.7 of
                    // every segment), and the digital modem keeps "data frame
                    // interval alignment from the first symbol of the second
                    // TRN1u signal" (8.5.7), which Phase 4 is built on: so
                    // whatever ends this segment, it ends on a twelfth
                    // symbol. What ends it arrives when it arrives -- the
                    // DIL's last symbol, the owner's choice, Ri -- and is
                    // acted on here, at the next boundary.
                    let due = if self.zero_dil_a10() {
                        // A10: no DIL was asked for, and Ri is what starts
                        // the CPt (9.5.2.1.10). No length is asked of this
                        // segment but the twelves.
                        self.ri_heard
                    } else {
                        // A11: "at least 2040T of TRN1u followed by CPt"
                        // (9.5.2.1.11), once the DIL has arrived and, if the
                        // CPt is to come from it, has been read.
                        self.source.count >= self.trn1u_over_dil()
                            && self.dil_complete()
                            && (self.chosen || !self.settings.choice_follows_dil)
                    };
                    if due {
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
                    // Phase 3 is over, and TRN2u's first symbol is the one
                    // this step sends: there is no gap after E1u.
                    self.begin_phase4();
                }
            }
            Up::Trn2u => self.phase4_out(),
            Up::B1u => {
                if self.source.b1u.is_empty() {
                    self.begin_data();
                }
            }
            Up::Data => {}
        }
    }

    // -----------------------------------------------------------------------
    // Phase 4, going up (9.6.2.1.1-9.6.2.1.5)
    // -----------------------------------------------------------------------

    /// 9.6.2.1.1: TRN2u, on the symbol after E1u's twelfth.
    ///
    /// "The scrambler shall be reset at the beginning of TRN2u" and "the
    /// differential encoder memory shall be initialized with the last
    /// transmitted sign bit of the preceding E1u sequence" (8.7.6). The size
    /// of the constellation is Jp bit 48's to say (Table 22); bit 49 is for a
    /// rate renegotiation.
    fn begin_phase4(&mut self) {
        let Some(cpu) = self.settled_cpu() else { return };
        let size = Trn2uSize::from_jp_bit(self.jp.is_some_and(|jp| jp.eight_in_training));
        // The bit that chose E1u's last symbol, where one is negative
        // (8.5.7), which is what Tables 28 and 29 call a sign bit of one too.
        let sign = self.source.sender.last_sign();
        self.source.trn2u = Some(Trn2uSender::seeded(size, sign));
        self.source.start(Up::Trn2u);
        self.stage = Stage::Phase4;
        self.status = Status::Phase4;
        self.in_flight = None;
        self.notes.push("E1u sent, phase 3 done".into());
        self.notes.push(format!(
            "TRN2u on {} points; the CPu will ask for drn {}, {} bit/s",
            size.points(),
            cpu.drn,
            self.downstream
        ));
    }

    /// The CPu this end sends (8.7.3, Table 23), settled once.
    ///
    /// Its constellations and shaping are [`Settings::cpu`]'s, or the CPt's
    /// own where nothing better has been measured. Its rate has to be one
    /// "supported and enabled in the transmitter of the digital modem" (Jd,
    /// Table 21) that the constellations carry (5.4.3/V.90: 2^K is no more
    /// than the product of their sizes) and that Table 2/V.90 defines for
    /// this S, so the rate asked for is kept if it is all three and otherwise
    /// replaced by the fastest that is. Its look-ahead is no more than Jd bits
    /// 49:50 allow.
    ///
    /// `None` if no rate is all three. Table 23's only word for that is drn
    /// 0, "indicates cleardown", and ending the call is not a thing to do
    /// unasked (9.11 leaves when to the modem that "sends a rate sequence",
    /// and has no rule that makes one mandatory): the caller retrains
    /// instead, which 9.5.2.2 and 9.6.2.2 allow "at any time" and which
    /// leaves the owner free to ask for another DIL, fall back or hang up.
    ///
    /// Bits 36:48 are V.90's upstream rate mask, and in V.92 they are
    /// "reserved for the ITU ... set to 0 by the analogue modem" (Table 23):
    /// the PCM upstream's mask went up in Ja (Table 20). The mask is cleared
    /// here so that what [`Modem::cpu`] reports is what the line carries.
    fn data_cp(&mut self) -> Option<Cp> {
        if let Some(cpu) = &self.cpu {
            return Some(cpu.clone());
        }
        let jd = self.far_jd;
        let enabled = |drn: u8| jd.is_none_or(|jd| jd.enables(drn));
        let carries = |cp: &Cp| CodeMapping::from_cp(cp).is_some_and(|mapping| mapping.valid());
        let asked = self.settings.cpu.is_some();
        let mut cpu = self.settings.cpu.clone().unwrap_or_else(|| self.settings.cpt.clone());
        cpu.data_mode = true; // Table 23 bits 19:20: "CPu denoted by 1"
        cpu.acknowledge = false;
        cpu.a_law = matches!(self.settings.law, Law::A); // bit 35
        cpu.upstream_rates = 0; // bits 36:48
        if let Some(jd) = jd {
            cpu.lookahead = cpu.lookahead.min(jd.lookahead); // bits 49:50
        }
        if !(asked && enabled(cpu.drn) && carries(&cpu)) {
            cpu.drn = (1..=DOWNSTREAM_RATES as u8)
                .rev()
                .find(|&drn| enabled(drn) && carries(&Cp { drn, ..cpu.clone() }))?;
        }
        self.downstream = CodeMapping::from_cp(&cpu).map_or(0, |mapping| mapping.rate());
        self.cpu = Some(cpu.clone());
        Some(cpu)
    }

    /// The CPu, or a retrain if there is none to send ([`Self::data_cp`]).
    fn settled_cpu(&mut self) -> Option<Cp> {
        let cpu = self.data_cp();
        if cpu.is_none() {
            self.retrain("no downstream rate that Jd enabled fits the constellations");
        }
        cpu
    }

    /// 9.6.2.1.1-9.6.2.1.4: what goes on TRN2u's carrier next, asked where
    /// the sequence before it ends.
    ///
    /// Every sequence here is a whole number of twelve symbols, and so is
    /// TRN2u (8.7.6), so a boundary is where the carrier's queue has run out
    /// and, while TRN2u alone is on the line, a twelfth symbol.
    fn phase4_out(&mut self) {
        if self.source.trn2u.as_ref().is_none_or(|sender| sender.queued() > 0) {
            return;
        }
        let now = self.now;
        match self.in_flight.take() {
            Some(kind) => {
                self.exchange.sequence_ended(kind, now);
                // 9.6.2.1.5: "After sending the E2u sequence, the analogue
                // modem shall send ... B1u", on the very next symbol.
                if kind == SequenceKind::E {
                    self.begin_b1u();
                    return;
                }
            }
            None => {
                // 9.6.2.1.1: "When the analogue modem is ready to receive a
                // CPd sequence and it has transmitted TRN2u for at least
                // 12000T or received an SUVd sequence, it shall transmit SUVu
                // sequences." Ready is having found TRN2d's frames, which an
                // SUVd that has been read implies.
                let sent = self.source.count;
                if !sent.is_multiple_of(UP_INTERVALS as u64) {
                    return;
                }
                let found = self.down.as_ref().is_some_and(Downstream::found);
                if !(self.suvd_heard || (found && sent >= TRN2U_MINIMUM)) {
                    return;
                }
                self.notes.push(format!("TRN2u ends after {sent} symbols, SUVu"));
            }
        }
        let kind = self.exchange.next_sequence(now);
        // The acknowledge bit is fixed for the whole sequence: "subsequent CPu
        // and SUVu sequences" carry it once a CPd has arrived (9.6.2.1.2).
        let ack = self.exchange.sequence_started(kind);
        let Some(sender) = self.source.trn2u.as_mut() else { return };
        let size = sender.size();
        // "Fill bits: 0s to extend the ... sequence length to the next
        // multiple of 12 symbols" (Tables 23 and 27), in the bits twelve
        // symbols of this constellation carry.
        let unit = trn2u_unit(size == Trn2uSize::Eight);
        let (kind, bits) = match (kind, self.cpu.as_ref()) {
            (SequenceKind::Cp, Some(cpu)) => {
                self.notes.push(format!("CPu for drn {}, ack {ack}", cpu.drn));
                (kind, Cp { acknowledge: ack, ..cpu.clone() }.to_bits_in(Layout::V92, unit))
            }
            (SequenceKind::E, _) => {
                // 8.7.2: "a data frame of scrambled, differentially encoded
                // zeroes ... transmitted using the corresponding TRN2u
                // modulation", and "extended by a single symbol if bit 29 of
                // CPd is set". The extra symbol is more of the same zeros.
                let extended = self.parameters.as_ref().is_some_and(|parameters| parameters.extend_e2u);
                let symbols = E2U_SYMBOLS + usize::from(extended);
                self.notes.push(format!("E2u, {symbols} symbols"));
                (kind, vec![false; symbols * size.bits()])
            }
            _ => {
                // Table 27 in initial training: bit 26 clear, because this
                // end's CPu is ready and asking the digital modem to wait for
                // it would only lengthen the start-up; bits 27:31 are 16, "no
                // measurement has been taken", because nothing has yet been
                // sent through the prefilter; bit 32 clear, because only a
                // rate renegotiation defines a silent period (9.8).
                let suvu = Suvu { wait_for_cpu: false, level: None, silence: false, ack };
                (SequenceKind::Suv, suvu.to_bits(unit))
            }
        };
        sender.push(&bits);
        self.in_flight = Some(kind);
    }

    /// 9.6.2.1.5 with 8.7.1: B1u, "48 data frames of scrambled binary ones
    /// transmitted using the data mode constellation parameters from the
    /// preceding CPd".
    ///
    /// "The first symbol of B1u shall begin data frame interval 0", it is n =
    /// 0 of 6.4.2's filters, and "the scrambler, modulus encoder,
    /// convolutional encoder, precoder and prefilter memories are initialized
    /// to zero" before it: a new encoder is all of that, and B1u's first
    /// symbol is the one after E2u's last, whether E2u was twelve symbols or
    /// thirteen.
    fn begin_b1u(&mut self) {
        let Some(parameters) = self.parameters.clone() else {
            // E2u only goes out after a CPd has been acknowledged, so this is
            // not reachable; a retrain is the honest answer if it ever is.
            self.retrain("E2u went out with no CPd to send B1u by");
            return;
        };
        match UpEncoder::new(&parameters) {
            Ok(mut encoder) => {
                // 6.4.2: the line carries v(n) "multiplied by a gain, G".
                self.source.b1u = encoder.b1u().iter().map(|symbol| symbol.out).collect();
                self.source.encoder = Some(encoder);
                self.source.start(Up::B1u);
                self.notes.push(format!("B1u at {} bit/s", parameters.rate()));
            }
            Err(why) => self.retrain(why),
        }
    }

    /// 9.6.2.1.5: "The modem shall then enable circuit 106 to respond to the
    /// condition of circuit 105 and begin data transmission using the
    /// modulation procedures of 6.4."
    fn begin_data(&mut self) {
        self.source.start(Up::Data);
        self.stage = Stage::Data;
        self.circuit106 = true;
        self.notes.push("B1u sent, 106 on".into());
        self.connect();
    }

    /// Data mode is both directions: 106 after this end's B1u, and 104 and
    /// 109 after the digital modem's B1d, which are two separate events
    /// (9.6.2.1.5, 9.6.2.1.6).
    fn connect(&mut self) {
        if !(self.circuit106 && self.circuit104 && self.circuit109) || self.stopped() {
            return;
        }
        let upstream = self.parameters.as_ref().map_or(0, Parameters::rate);
        self.status = Status::Connected { downstream: self.downstream, upstream };
        self.notes.push(format!("V.92 data: {} down, {upstream} up", self.downstream));
    }

    // -----------------------------------------------------------------------
    // Phase 4, coming down (9.6.2.1.1, 9.6.2.1.6)
    // -----------------------------------------------------------------------

    /// TRN2d follows R-bar-i -- the digital modem is to "transmit R-bar-i and
    /// then proceed to Phase 4" (9.5.1.1.12, 9.5.1.1.13), where it "shall
    /// transmit TRN2d for at least 2040T" (9.6.1.1.1) -- so the reversal is
    /// where the downstream reader starts looking for it.
    ///
    /// And the only place. Before the reversal the line carries Ri, one level
    /// in every symbol, which through the CPt's mapping is the same D bits
    /// frame after frame: whether that descrambles to ones is a property of
    /// the CPt and U_INFO and not of chance, so it is nothing a run of ones
    /// should be looked for in. And with no DIL asked for a missed R-bar-i
    /// leaves CPt with nothing to end it (9.5.2.1.10), which no search for
    /// TRN2d mends. A reversal that was not heard is what 9.6.2.2.1's timer
    /// is for.
    fn begin_downstream(&mut self) {
        if self.down.is_some() {
            return;
        }
        let Some(cpu) = self.settled_cpu() else { return };
        // V.90 8.6.5: "The constellation set to be used is passed in CPt",
        // which is the one this end sent.
        self.down = Downstream::hunting(&self.settings.cpt, &cpu, self.settings.law);
        if self.down.is_none() {
            // Nothing of Phase 4 can be read, so nothing of it can be
            // answered: 9.5.2.2 and 9.6.2.2 allow the retrain at any time.
            self.retrain("the CPt's rate is more than its constellations carry");
            return;
        }
        // Everything Phase 3 read was two points, and the receiver decided
        // them. From here each symbol is one of its own interval's
        // constellation, which only the frame reader can say, so the
        // receiver equalises and decides nothing.
        self.rx.hand_over_unsliced();
    }

    fn heard_down(&mut self, event: DownEvent) {
        let now = self.now;
        match event {
            DownEvent::Found => self.notes.push("TRN2d found, reading for SUVd".into()),
            DownEvent::Sequence(family) => match *family {
                DownFamily::Suvd(suvd) => {
                    if !self.suvd_heard {
                        self.notes.push(format!("SUVd, ack {}", suvd.ack));
                    }
                    self.suvd_heard = true;
                    // Table 31: bits 19:31 are not interpreted, and bit 32's
                    // silent period is a rate renegotiation's, which the
                    // exchange knows this is not.
                    let suv = PeerSuv { ack: suvd.ack, silence: suvd.silence, wait_for_cp: false };
                    self.exchange.peer_suv(suv, now);
                }
                DownFamily::Cpd(cpd) => self.heard_cpd(&cpd),
            },
            DownEvent::Ed => {
                // 9.6.2.1.4 counts Ed beside an acknowledged sequence.
                self.exchange.peer_e(now);
                self.notes.push("Ed, reading for B1d".into());
            }
            DownEvent::B1dBegun { ed_read, late } => {
                if !ed_read {
                    // Ed is the two frames in front of B1d whether or not
                    // they were read as it (8.8.2, 9.6.1.1.5), and 9.6.2.1.4
                    // counts it beside an acknowledged sequence.
                    self.exchange.peer_e(now);
                    self.notes.push("B1d found; the Ed before it was not read".into());
                } else if late > 0 {
                    self.notes.push(format!("B1d began {late} frames after Ed's two"));
                }
            }
            DownEvent::B1d { clean } => {
                self.notes.push(format!("B1d, {clean} of {B1D_FRAMES} frames scrambled ones"));
                if clean < B1D_CLEAN {
                    // Not B1d, so circuit 104 stays clamped, and B1d is not
                    // sent twice: `timers` asks for the retrain.
                    self.b1d_failed = Some(now);
                    return;
                }
                // 9.6.2.1.6: "After receiving B1d, the analogue modem shall
                // unclamp circuit 104, turn on circuit 109, and begin
                // demodulating data."
                self.circuit104 = true;
                self.circuit109 = true;
                self.connect();
            }
            DownEvent::Data(bits) => {
                if self.circuit104 {
                    self.recovered.extend(bits);
                }
            }
        }
    }

    /// A CRC-good CPd (8.8.3, Table 30).
    fn heard_cpd(&mut self, cpd: &Cpd) {
        // "drn = 0 shall indicate cleardown" (Table 30 bits 22:26; 9.11 puts
        // the field in an SUVd, which has none).
        if cpd.drn == 0 {
            self.clear_down("the digital modem's CPd said drn 0");
            return;
        }
        // 8.8.3 removes a part's bits when the part is absent and does not
        // say what its absence means. The reading taken is "as the CPd before
        // said", so the first CPd of a training, which has no CPd before it,
        // has to carry all three; one that does not is answered with a
        // retrain (9.6.2.2) rather than with a guess at the filters.
        let merged = match &self.cpd {
            Some(previous) => cpd.merged_over(previous),
            None if cpd.complete() => cpd.clone(),
            None => {
                self.retrain("the first CPd is missing a part");
                return;
            }
        };
        // The rate has to be one Ja offered (Table 20), the filters ones
        // INFO1a said this end can hold (Table 18), and the whole of it
        // something the 6.4 chain can be built from. A CPd that is none of
        // those cannot be obeyed, and acknowledging it would promise a B1u
        // this end cannot send.
        if !ja_mask_has(self.settings.upstream_rates, merged.drn) {
            self.retrain("the CPd asks for an upstream rate Ja did not offer");
            return;
        }
        if let Err(why) = merged.check(&self.settings.limits, up_bits(merged.drn)) {
            self.retrain(why);
            return;
        }
        let parameters = match Parameters::try_from(&merged) {
            Ok(parameters) => parameters,
            Err(why) => {
                self.retrain(why);
                return;
            }
        };
        if let Err(why) = UpEncoder::new(&parameters) {
            self.retrain(why);
            return;
        }
        if self.cpd.as_ref().is_none_or(|previous| previous.ack != cpd.ack) {
            self.notes.push(format!("CPd for drn {}, {} bit/s, ack {}", merged.drn, parameters.rate(), cpd.ack));
        }
        self.cpd = Some(merged);
        self.parameters = Some(parameters);
        self.exchange.peer_cp(PeerCp { ack: cpd.ack }, self.now);
    }

    // -----------------------------------------------------------------------
    // Giving the line up (9.6.2.2, 9.7.2, 9.11)
    // -----------------------------------------------------------------------

    /// 9.7.2.1 and 9.7.2.2 begin the same way: "turn OFF circuit 106, clamp
    /// circuit 104 to binary one and transmit silence". That much is done
    /// here; the 70 ms, Tone A and Phase 2 are the owner's, who is told why.
    fn retrain(&mut self, why: &'static str) {
        if self.stopped() {
            return;
        }
        self.notes.push(format!("V.92 analogue: retrain, {why}"));
        self.status = Status::Retrain(why);
        self.stop();
    }

    /// 9.11: the call is over.
    fn clear_down(&mut self, why: &'static str) {
        if self.stopped() {
            return;
        }
        self.notes.push(format!("V.92 analogue: cleardown, {why}"));
        self.status = Status::ClearedDown;
        self.stop();
        // The call is over and so is its line signal. 9.11 names no circuit;
        // 109 OFF is what a DTE is owed when there is no carrier left.
        self.circuit109 = false;
    }

    /// Silence on the line, circuit 106 OFF and circuit 104 clamped (9.7.2.1,
    /// 9.7.2.2). Circuit 109 is the caller's to decide: a retrain leaves it.
    fn stop(&mut self) {
        self.source.start(Up::Silence);
        self.in_flight = None;
        self.down = None;
        self.circuit106 = false;
        self.circuit104 = false;
        self.b1d_failed = None;
        // Circuit 104 is clamped to binary one from here, so what was
        // received and not yet taken does not reach the DTE. That is all the
        // clamp can do: Tone B is only known for what it is after "more than
        // 50 ms" (9.7.2.2), and what the data decoder made of those 50 ms
        // went to a DTE that took its bits as they came. Telling that from
        // data is the error control's business above this.
        self.recovered.clear();
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
        if self.stopped() {
            return;
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
        self.scope.feed(symbol.value);
        // From R-bar-i on, what arrives is the 5.4 encoder's and is read a
        // data frame at a time. The equaliser calls U_INFO's level one, and
        // hands it over the other way up ([`DOWN_SIGN`]).
        let level = DOWN_SIGN * symbol.value * self.unit;
        if let Some(down) = self.down.as_mut()
            && let Some(event) = down.feed(level)
        {
            self.heard_down(event);
        }
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
            // A10's order: Ri is what *starts* CPt, and R-bar-i ends it. It
            // starts where the TRN1u on the line reaches a twelfth symbol
            // (9.5.2.1.9), which `advance` sees to.
            Stage::Dil if self.zero_dil_a10() && !self.ri_heard => {
                self.ri_heard = true;
                self.notes.push(format!("Ri at {at}, CPt at the next twelfth symbol"));
            }
            _ => {}
        }
    }

    /// R-bar-i: with N = 0 the CPt series ends here (A10), and in either
    /// order TRN2d is what follows it.
    fn ri_bar(&mut self, at: u64) {
        if self.stage == Stage::Cpt && self.zero_dil_a10() {
            self.source.stop = true;
            self.notes.push(format!("R-bar-i at {at}, CPt ends"));
        }
        // With a DIL the digital modem sends R-bar-i on reading E1u, so it
        // arrives with TRN2u already on the line; with none it arrives while
        // CPt is still going out. Either way Phase 4's downstream starts 24T
        // after it (V.90 8.6.4), and before a CPt has been sent there is
        // nothing it could be read with.
        if matches!(self.stage, Stage::Cpt | Stage::E1u | Stage::Phase4) {
            self.begin_downstream();
        }
    }

    /// Whether this call is in A10's order: no DIL asked for, and that order
    /// taken (see [`ZERO_DIL_A10_ORDER`]).
    fn zero_dil_a10(&self) -> bool {
        ZERO_DIL_A10_ORDER && self.settings.descriptor.is_empty()
    }

    fn timers(&mut self) {
        if self.stopped() {
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
        // A B1d that did not read as one will not be sent again, and 9.6.2.2
        // allows the retrain at any time ([`B1D_GRACE`]).
        if let Some(at) = self.b1d_failed
            && self.now > at + (B1D_GRACE * FS) as u64
        {
            self.retrain("what followed Ed was not B1d");
            return;
        }
        // 9.6.2.2.1: no B1d "within 20 s plus 6 round-trip delays from the end
        // of sending INFO1a" is a retrain according to 9.7.2.1. It runs
        // through Phase 3 and Phase 4 alike, and only B1d stops it: circuit
        // 104 is let go when B1d has been received and not before.
        if !self.circuit104 && self.now > self.start_up_by {
            self.retrain("no B1d within 20 s and six round trips");
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

    /// 9.5.2.1.9: "The length for this segment of TRN1u shall be in multiple
    /// of 12 symbols". With no DIL asked for, what ends it is Ri (9.5.2.1.10),
    /// and Ri is heard on whichever symbol it is heard on: CPt waits for the
    /// twelfth.
    #[test]
    fn ri_ends_the_second_trn1u_on_a_twelfth_symbol() {
        for heard_after in 1..=24u64 {
            let mut modem = Modem::new(settings(Descriptor::none()));
            modem.begin_trn1u_over_dil();
            let mut sent = 0u64;
            while modem.source.up == Up::Trn1u {
                assert!(sent < 48, "Ri after {heard_after} symbols never started CPt");
                if sent == heard_after {
                    modem.ri(0);
                    assert_eq!(modem.source.up, Up::Trn1u, "Ri alone does not cut TRN1u");
                }
                modem.advance();
                if modem.source.up == Up::Trn1u {
                    modem.source.next();
                    sent += 1;
                }
            }
            assert_eq!(modem.source.up, Up::Cpt);
            assert_eq!(sent, heard_after.div_ceil(12) * 12, "Ri after {heard_after} symbols of TRN1u");
        }
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

/// Phase 4 and data mode, against the digital modem's own downstream played
/// from a script.
///
/// The far end here is [`crate::v92::digital::Source`], which is what the
/// digital modem sends every one of its signals with, told what to send by
/// the test rather than by the digital modem's state machine: so each test
/// chooses exactly which SUVd, CPd and Ed arrive and when, and reads what the
/// analogue modem puts on the line the way 8.7 says a digital modem would.
#[cfg(test)]
mod phase4_tests {
    use super::*;
    use crate::v90::encoder::Encoder as CodeEncoder;
    use crate::v90::sequences::CpFinder;
    use crate::v92::decoder::Decoder as UpDecoder;
    use crate::v92::digital::{self, Out};
    use crate::v92::sequences::{CpFamily, Suvd, UpFinder};
    use crate::v92::up_signals::Trn2uReader;
    use crate::v92::upchoice::gain_for;
    use crate::v92::{Filters, JA_MASK_ALL, Trellis};

    /// Symbols a test waits for something that ought to happen in a second or
    /// two.
    const SOON: u64 = 30_000;

    /// A CPt for 32 000 bit/s on thirty-two points: Ucodes 16 to 47, none of
    /// them the zero level, whose sign no receiver can read.
    fn cpt() -> Cp {
        let mask = (16..=47u8).fold(0u128, |mask, u| mask | 1 << u);
        Cp { drn: 16, constellations: vec![mask], ..Cp::default() }
    }

    fn settings() -> Settings {
        Settings::new(Descriptor::none(), JA_MASK_ALL, cpt())
    }

    fn far_settings() -> digital::Settings {
        digital::Settings {
            law: Law::Mu,
            uinfo: UINFO,
            far_md: 0,
            round_trip: 0.0,
            wide: true,
            limits: FilterLimits::from_info1a(0, 0, 0),
            habits: digital::Habits::PROMPT,
        }
    }

    /// Modulation parameters for a line that does nothing: a prefilter of one
    /// tap, no precoder, one constellation of odd levels in every interval,
    /// and G chosen so that the mean square of G x v is one (8.8.3).
    ///
    /// `drn` 1 is 24 000 bit/s, K = 36, three bits an interval; `drn` 7 is
    /// 32 000 bit/s, K = 48, four bits an interval (6.1, 6.4.1).
    fn parameters(drn: u8, extend_e2u: bool) -> Parameters {
        let width = up_bits(drn) / UP_INTERVALS as u32;
        assert_eq!(width * UP_INTERVALS as u32, up_bits(drn), "a rate whose K divides by twelve");
        let modulus = 1u8 << width;
        // 6.4.2: at k = 3 a class steps by 2 x Mi, so the set needs LC >= Mi.
        let levels = usize::from(modulus);
        Parameters {
            drn,
            trellis: Trellis::Sixteen,
            extend_e2u,
            gain: gain_for(levels),
            moduli: [modulus; UP_INTERVALS],
            filters: Filters { z1: Vec::new(), p1: Vec::new(), z2: vec![1.0], p2: Vec::new() },
            sets: vec![(0..levels as u16).map(|k| 2 * k + 1).collect()],
            indices: [0; 6],
        }
    }

    /// The same parameters as a CPd's fields can say them: G in unsigned
    /// Q0.16 and the coefficients in Q0.15 (Table 30).
    fn as_sent(parameters: &Parameters) -> Parameters {
        let bits = Cpd::from(parameters).to_bits(INTERVALS);
        let cpd = Cpd::from_bits(&bits).expect("a CPd reads back");
        Parameters::try_from(&cpd).expect("a complete CPd")
    }

    /// A deterministic bit pattern, for the data.
    fn pattern(length: usize, seed: u64) -> Vec<bool> {
        let mut state = seed | 1;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state >> 63 == 0
            })
            .collect()
    }

    /// What the far end is asked to send next.
    enum Item {
        /// SUVd, CPd or Ed, as bits.
        Sequence(Out, Vec<bool>),
        /// The same, with one symbol of its first data frame turned over on
        /// the line: a sequence whose frame sync does not survive.
        Spoiled(Out, Vec<bool>),
        /// B1d and the data after it, as octets.
        Codes(Vec<u8>),
    }

    /// Which symbol of a spoiled sequence is turned over, counted from its
    /// first: inside the seventeen ones of its frame sync whatever D is.
    const SPOILED_SYMBOL: usize = 3;

    /// The digital modem, as far as the analogue modem can tell.
    struct Far {
        source: digital::Source,
        queue: VecDeque<Item>,
        /// Whether the source has been handed something it has not started.
        armed: bool,
        /// Whether what it was handed is to be spoiled, and the symbols still
        /// to go before the one that is turned over.
        spoil_next: bool,
        spoil_in: Option<usize>,
        /// What a sequence is filled to, in bits: D, the bits of a data frame
        /// on TRN2d's modulation, which is Tables 30 and 31's "next multiple
        /// of 6 symbols".
        unit: usize,
        /// The analogue modem's TRN2u, SUVu, CPu and E2u, read as 8.7.6 says.
        reader: Trn2uReader,
        finder: UpFinder,
        /// Bits to a symbol of that: two on four points, three on eight
        /// (Tables 28 and 29).
        width: usize,
        /// Every descrambled bit of it. The reader takes TRN2u's first symbol
        /// as its reference, so bit `b` came from symbol `1 + b / width`.
        bits: Vec<bool>,
        /// The sequences found, with the index of the bit that completed each.
        heard: Vec<(usize, CpFamily)>,
    }

    impl Far {
        /// A digital modem that has read this analogue modem's Ja and CPt,
        /// and whose Jp asks for TRN2u on eight points if `eight` (Table 22
        /// bit 48).
        fn new(settings: &Settings, eight: bool) -> Self {
            let mut source = digital::Source::new(&far_settings());
            assert!(source.set_cpt(&settings.cpt), "a CPt its own constellations carry");
            source.set_dil(&settings.descriptor);
            source.set_jp(&Jp { eight_in_training: eight, ..Jp::default() });
            let unit = source.fill_unit();
            Self {
                source,
                queue: VecDeque::new(),
                armed: false,
                spoil_next: false,
                spoil_in: None,
                unit,
                reader: Trn2uReader::new(Trn2uSize::from_jp_bit(eight)),
                finder: UpFinder::new(trn2u_unit(eight)),
                width: Trn2uSize::from_jp_bit(eight).bits(),
                bits: Vec::new(),
                heard: Vec::new(),
            }
        }

        /// One codeword out. A queued item is handed to the source as soon as
        /// the one before it has started, so that each goes out once and in
        /// order; with nothing queued the source repeats its last sequence,
        /// which is 9.6.1.1.1's "SUVd repeatedly".
        fn next(&mut self) -> f64 {
            let was = self.source.out();
            let mut level = self.source.next();
            let ended = self.source.take_finished().is_some();
            if self.armed && (ended || (was == Out::Trn2d && self.source.out() != Out::Trn2d)) {
                // What was handed over has started, and `level` is its first
                // symbol.
                self.armed = false;
                if std::mem::take(&mut self.spoil_next) {
                    self.spoil_in = Some(SPOILED_SYMBOL);
                }
            }
            match self.spoil_in {
                Some(0) => {
                    level = -level;
                    self.spoil_in = None;
                }
                Some(left) => self.spoil_in = Some(left - 1),
                None => {}
            }
            // Nothing of Phase 4 is asked for before TRN2d is on the line: the
            // source holds one request at a time, and until then that one is
            // R-bar-i's.
            if !self.armed
                && self.source.in_phase4()
                && let Some(item) = self.queue.pop_front()
            {
                match item {
                    Item::Sequence(out, bits) => self.source.send(out, bits),
                    Item::Spoiled(out, bits) => {
                        self.source.send(out, bits);
                        self.spoil_next = true;
                    }
                    Item::Codes(octets) => self.source.send_codes(octets),
                }
                self.armed = true;
            }
            level
        }

        fn suvd(&mut self, ack: bool) {
            self.queue.push_back(Item::Sequence(Out::Suvd, Suvd { silence: false, ack }.to_bits(self.unit)));
        }

        /// An SUVd whose frame sync is damaged on the line.
        fn spoiled_suvd(&mut self, ack: bool) {
            self.queue.push_back(Item::Spoiled(Out::Suvd, Suvd { silence: false, ack }.to_bits(self.unit)));
        }

        fn cpd(&mut self, cpd: &Cpd) {
            self.queue.push_back(Item::Sequence(Out::Cpd, cpd.to_bits(self.unit)));
        }

        /// Ed: "2 data frames of scrambled binary zeros" (8.8.2).
        fn ed(&mut self) {
            self.queue.push_back(Item::Sequence(Out::Ed, self.source.ed_bits()));
        }

        /// Ed, then B1d and `data` on the constellations `cpu` asked for, then
        /// the idle ones of a data mode with nothing to send (V.90 8.6.1).
        fn ed_b1d_and_data(&mut self, cpu: &Cp, data: &[bool]) {
            self.ed();
            self.b1d_and_data(cpu, data);
        }

        /// B1d and what follows it, behind whatever is queued already.
        fn b1d_and_data(&mut self, cpu: &Cp, data: &[bool]) {
            let mapping = CodeMapping::from_cp(cpu).expect("a CPu its own constellations carry");
            let mut encoder = CodeEncoder::new(mapping, Law::Mu);
            let mut scrambler = Scrambler::new(Mode::Call);
            let mut data = data.iter().copied();
            let mut octets = Vec::new();
            for n in 0..B1D_FRAMES + 3000 {
                let frame = encoder.next_frame(|| {
                    let bit = if n < B1D_FRAMES { true } else { data.next().unwrap_or(true) };
                    scrambler.scramble(bit)
                });
                octets.extend(frame.octets(Law::Mu));
            }
            self.queue.push_back(Item::Codes(octets));
        }

        /// One upstream symbol of TRN2u's modulation.
        fn hear(&mut self, level: f64) {
            for bit in self.reader.feed(level) {
                self.bits.push(bit);
                if let Some(family) = self.finder.feed(bit) {
                    self.heard.push((self.bits.len() - 1, family));
                }
            }
        }

        fn suvus(&self) -> Vec<(usize, Suvu)> {
            self.heard
                .iter()
                .filter_map(|(at, family)| match family {
                    CpFamily::Suvu(suvu) => Some((*at, *suvu)),
                    _ => None,
                })
                .collect()
        }

        fn cpus(&self) -> Vec<(usize, Cp)> {
            self.heard
                .iter()
                .filter_map(|(at, family)| match family {
                    CpFamily::Cpu(cp) => Some((*at, cp.clone())),
                    _ => None,
                })
                .collect()
        }
    }

    /// The symbol a bit of the four-point upstream stream came from (see
    /// [`Far::bits`]).
    fn symbol_of(bit: usize) -> usize {
        1 + bit / 2
    }

    /// Bits from an SUVu's first to the fill bit the finder answers at
    /// (Table 27: bit 51).
    const SUVU_TO_FILL: usize = 51;

    /// An SUVu: fifty-two bits filled to seventy-two, which is thirty-six
    /// symbols on four points and twenty-four on eight (Table 27 with Tables
    /// 28 and 29; both are whole twelves).
    const SUVU_BITS: usize = 72;

    /// The two ends and the wire between them, which is perfect: this is a
    /// test of what the analogue modem does, and a line that did nothing to
    /// the signal is the one on which every symbol can be checked exactly.
    struct Call {
        modem: Modem,
        far: Far,
        /// How far the Phase 3 script has got, and a count it keeps.
        act: u8,
        seen: u32,
        /// The last symbol sent before TRN2u, which is E1u's last.
        before: f64,
        /// Everything sent from TRN2u's first symbol on.
        up: Vec<f64>,
        /// Where in `up` the modem said B1u began.
        b1u_from: Option<usize>,
        /// When set, Tone B is on the line in place of the far end's signal,
        /// and this many samples of it have gone.
        tone: Option<u64>,
        /// When set, nothing of Phase 4 arrives: the line is dead from
        /// TRN2d's first symbol.
        mute: bool,
        /// Symbols of B1d, counted from its first, that arrive as nothing:
        /// a dropout.
        dropout: std::ops::Range<usize>,
        /// Symbols of B1d and the data behind it that the far end has sent.
        b1d_sent: usize,
        /// Symbols of the TRN1u that covers the DIL (9.5.2.1.9), and symbols
        /// from its first to TRN2u's first.
        trn1u: u64,
        grid: u64,
        /// The CPt series, read off the line the way a digital modem reads
        /// it: one bit a symbol, differential, GPA (8.5.1).
        cpt_reader: TwoPointReader,
        cpt_finder: CpFinder,
        cpts: Vec<Cp>,
        /// The CPt and CPu an owner chose from the DIL, and how many symbols
        /// after the DIL's last it takes to choose them.
        choice: Option<(Cp, Cp, u32)>,
        notes: Vec<String>,
    }

    impl Call {
        fn new(settings: Settings) -> Self {
            Self::with_jp_bit_48(settings, false)
        }

        fn with_jp_bit_48(settings: Settings, eight: bool) -> Self {
            let far = Far::new(&settings, eight);
            Self::against(settings, far)
        }

        fn against(settings: Settings, far: Far) -> Self {
            Self {
                modem: Modem::new(settings),
                far,
                act: 0,
                seen: 0,
                before: 0.0,
                up: Vec::new(),
                b1u_from: None,
                tone: None,
                mute: false,
                dropout: 0..0,
                b1d_sent: 0,
                trn1u: 0,
                grid: 0,
                cpt_reader: TwoPointReader::differential_with(Mode::Answer),
                cpt_finder: CpFinder::v92(UP_INTERVALS),
                cpts: Vec::new(),
                choice: None,
                notes: Vec::new(),
            }
        }

        /// The digital modem's half of Phase 3 (9.5.1.1), each signal sent
        /// when the analogue modem has got to the place that calls for it.
        fn phase3(&mut self) {
            let phase = self.modem.phase();
            match self.act {
                // 9.5.1.1.3: Sd on reading Ja.
                0 if phase == "V.92 phase 3: Ja" => {
                    self.far.source.change(Out::Sd);
                    self.act = 1;
                }
                // 9.5.1.1.8: Jp once Su has been measured, which a real one
                // takes a few hundred symbols over.
                1 if phase == "V.92 phase 3: Su" => {
                    self.seen += 1;
                    if self.seen == 400 {
                        self.far.source.change(Out::Jp);
                        self.act = 2;
                        self.seen = 0;
                    }
                }
                // 9.5.1.1.9: Jp' on the S-bar-u that answers Jp. No DIL was
                // asked for, so SCR and then Ri follow it (9.5.1.1.13).
                2 if phase == "V.92 phase 3: S-bar-u" => {
                    self.far.source.change(Out::JpPrime);
                    self.act = 3;
                }
                // Once a CPt has been read, which takes one repetition of it
                // and here is given two: R-bar-i if no DIL was asked for and
                // Ri has gone out already (9.5.1.1.13), and otherwise Ri at
                // the end of the DIL segment in flight (9.5.1.1.12).
                3 if phase == "V.92 phase 3: CPt" => {
                    self.seen += 1;
                    if self.seen == 700 {
                        let no_dil = self.modem.settings().descriptor.is_empty();
                        self.far.source.change(if no_dil { Out::RiBar } else { Out::Ri });
                        self.act = if no_dil { 5 } else { 4 };
                    }
                }
                // With a DIL, R-bar-i answers E1u (9.5.1.1.12), which has gone
                // by when TRN2u is on the line.
                4 if self.modem.status() == Status::Phase4 => {
                    self.far.source.change(Out::RiBar);
                    self.act = 5;
                }
                _ => {}
            }
        }

        fn tick(&mut self) {
            self.phase3();
            // An owner that reads the DIL takes a while over it, and then
            // hands over what it chose.
            if self.modem.dil_received()
                && let Some((training, data, left)) = self.choice.take()
            {
                if left == 0 {
                    assert!(self.modem.choose(training, data), "a choice made before CPt is taken");
                } else {
                    self.choice = Some((training, data, left - 1));
                }
            }
            let mut down = self.far.next();
            if self.far.source.out() == Out::B1d {
                if self.dropout.contains(&self.b1d_sent) {
                    down = 0.0;
                }
                self.b1d_sent += 1;
            }
            if self.mute && self.far.source.in_phase4() {
                down = 0.0;
            }
            if let Some(n) = self.tone.as_mut() {
                // Tone B is 1200 Hz (9.7.1.1), a tenth of full scale here.
                down = 0.1 * (std::f64::consts::TAU * 1200.0 * *n as f64 / FS).sin();
                *n += 1;
            }
            let up = self.modem.step(down);
            // `phase` names the signal the symbol just sent belongs to.
            let phase = self.modem.phase();
            let over_dil = phase == "V.92 phase 3: TRN1u over the DIL";
            self.trn1u += u64::from(over_dil);
            if self.modem.status() == Status::Running && (over_dil || self.grid > 0) {
                self.grid += 1;
                if let Some(bit) = self.cpt_reader.feed(up)
                    && let Some(cp) = self.cpt_finder.feed(bit)
                {
                    self.cpts.push(cp);
                }
            }
            if self.up.is_empty() && self.modem.status() == Status::Running {
                self.before = up;
            } else {
                if self.b1u_from.is_none() && self.modem.phase() == "V.92 phase 4: B1u" {
                    self.b1u_from = Some(self.up.len());
                }
                self.up.push(up);
                if self.b1u_from.is_none() {
                    self.far.hear(up);
                }
            }
            self.notes.append(&mut self.modem.take_notes());
        }

        fn run(&mut self, ticks: u64) {
            for _ in 0..ticks {
                self.tick();
            }
        }

        fn run_until(&mut self, done: impl Fn(&Self) -> bool, ticks: u64) -> bool {
            for _ in 0..ticks {
                if done(self) {
                    return true;
                }
                self.tick();
            }
            done(self)
        }

        /// Phase 3, to TRN2u's first symbol.
        fn reach_phase4(&mut self) {
            let reached = self.run_until(|call| call.modem.status() == Status::Phase4, 60_000);
            assert!(reached, "phase 3 stopped at {}: {:#?}", self.modem.phase(), self.notes);
        }

        /// The digital modem's half of the exchange, as 9.6.1.1.1-9.6.1.1.5
        /// have it: SUVd, one CPd after the first SUVu, the acknowledge bit
        /// once the CPu has been read, and Ed, B1d and data once an
        /// acknowledged SUVu has arrived.
        fn exchange(&mut self, parameters: &Parameters, data: &[bool]) {
            let cpu = self.acknowledged(parameters);
            self.far.ed_b1d_and_data(&cpu, data);
        }

        /// The same as far as the acknowledgements: SUVd, the CPd, SUVd' once
        /// the CPu has been read, and an SUVu' back. Returns that CPu.
        fn acknowledged(&mut self, parameters: &Parameters) -> Cp {
            self.far.suvd(false);
            assert!(
                self.run_until(|call| !call.far.suvus().is_empty(), SOON),
                "no SUVu: {} {:#?}",
                self.modem.phase(),
                self.notes
            );
            self.far.cpd(&Cpd::from(parameters));
            self.far.suvd(false);
            assert!(self.run_until(|call| !call.far.cpus().is_empty(), SOON), "no CPu: {:#?}", self.notes);
            self.far.suvd(true);
            assert!(
                self.run_until(|call| call.far.suvus().iter().any(|(_, suvu)| suvu.ack), SOON),
                "no SUVu': {:#?}",
                self.notes
            );
            self.far.cpus()[0].1.clone()
        }

        /// Data mode is reached, and `down` arrives behind B1d from its first
        /// bit (V.90 8.6.1).
        fn connects_and_reads(&mut self, down: &[bool]) {
            let connected = self.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON);
            assert!(connected, "{:?} at {}: {:#?}", self.modem.status(), self.modem.phase(), self.notes);
            let mut taken = Vec::new();
            for _ in 0..down.len() + 600 {
                self.tick();
                taken.extend(self.modem.take_bits());
            }
            assert!(taken.len() >= down.len(), "{} bits came down: {:#?}", taken.len(), self.notes);
            assert!(taken[..down.len()] == down[..], "the data after B1d, bit for bit: {:#?}", self.notes);
        }

        fn noted(&self, text: &str) -> bool {
            self.notes.iter().any(|note| note == text)
        }

        /// Where on the line E2u and B1u are, from the line alone: E2u is the
        /// zeros that follow the last SUVu', and B1u the symbol after them.
        /// Returns B1u's first symbol as an index into `up`.
        fn b1u_on_the_line(&self, e2u_symbols: usize) -> usize {
            let (at, last) = *self.far.suvus().last().expect("an SUVu went out");
            assert!(last.ack, "the sequence before E2u carries the acknowledge bit (9.6.2.1.4)");
            let e2u = at - SUVU_TO_FILL + SUVU_BITS;
            let width = self.far.width;
            let zeros = &self.far.bits[e2u..e2u + width * e2u_symbols];
            assert!(zeros.iter().all(|&bit| !bit), "E2u is scrambled zeros (8.7.2): {zeros:?}");
            1 + e2u / width + e2u_symbols
        }
    }

    /// 9.6.2.1 from end to end: the whole of Phase 4 against a digital modem
    /// that keeps its side of 9.6.1.1, and then data both ways.
    ///
    /// What the Recommendation requires and this checks: the CPu names a rate
    /// (Table 23); circuit 106 comes on after B1u and circuits 104 and 109
    /// after B1d (9.6.2.1.5, 9.6.2.1.6); nothing received before B1d has gone
    /// by reaches the DTE, and what comes after it does, from its first bit
    /// (V.90 8.6.1); and what the DTE sends goes out behind B1u's 48 frames
    /// of ones, through the chain the CPd described (8.7.1, 6.4).
    #[test]
    fn the_analogue_modem_connects_against_a_scripted_phase_4() {
        let wanted = parameters(7, false);
        let down = pattern(3000, 0xd0d0);
        let up = pattern(2000, 0x0b0b);
        let mut call = Call::new(settings());
        call.reach_phase4();
        // The DTE may hand data over whenever it likes; with circuit 106 OFF
        // it waits.
        call.modem.send_bits(&up);
        assert_eq!(call.modem.fill_bits(), 0, "nothing is taken before data mode");
        call.exchange(&wanted, &down);
        let connected = call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON);
        assert!(connected, "{:?} at {}: {:#?}", call.modem.status(), call.modem.phase(), call.notes);

        // Table 23: the CPu asked for a rate, and the status reports it; the
        // upstream rate is the CPd's (Table 30).
        let cpu = call.far.cpus()[0].1.clone();
        assert!(cpu.data_mode, "type 1 is a CPu");
        let wire = as_sent(&wanted);
        assert_eq!(call.modem.parameters(), Some(&wire));
        assert_eq!(
            call.modem.status(),
            Status::Connected { downstream: (u32::from(cpu.drn) + 20) * 8000 / 6, upstream: 32_000 }
        );
        assert_eq!(call.modem.phase(), "V.92 data");
        assert!(call.modem.circuit104() && call.modem.circuit106() && call.modem.circuit109());
        // B1d is 48 frames of ones from zeroed memories (V.90 8.6.1), and it
        // only reads as that if the reader started on its first symbol the
        // right way up.
        assert!(
            call.notes.iter().any(|note| note == "B1d, 48 of 48 frames scrambled ones"),
            "{:#?}",
            call.notes
        );

        // Downstream: the data that follows B1d, from its first bit.
        let mut taken = Vec::new();
        for _ in 0..6000 {
            call.tick();
            taken.extend(call.modem.take_bits());
        }
        assert!(taken.len() >= down.len(), "{} bits came down", taken.len());
        assert_eq!(taken[..down.len()], down[..], "the data after B1d, bit for bit");
        let stray = taken[down.len()..].iter().position(|&bit| !bit);
        assert_eq!(stray, None, "and then an idle line's ones, of {} bits", taken.len() - down.len());

        // Upstream: B1u's ones and then the DTE's bits, read by a decoder
        // built from the same CPd and started on B1u's first symbol.
        let from = call.b1u_on_the_line(E2U_SYMBOLS);
        assert_eq!(Some(from), call.b1u_from, "B1u begins on the symbol after E2u (8.7.1)");
        let mut decoder = UpDecoder::new(&wire);
        let mut back = Vec::new();
        for &level in &call.up[from..] {
            decoder.feed(level);
            back.extend(decoder.take_bits());
        }
        let ones = 48 * up_bits(7) as usize;
        assert!(back.len() >= ones + up.len(), "{} bits went up", back.len());
        assert!(back[..ones].iter().all(|&bit| bit), "B1u is 48 data frames of ones (8.7.1)");
        assert_eq!(back[ones..ones + up.len()], up[..], "and the data follows it at once (9.6.2.1.5)");
        assert_eq!(call.modem.fill_bits(), up_bits(7) as usize, "K bits a data frame (6.4.1)");
    }

    /// 9.5.2.1.11 and 9.5.1.1.12: with a DIL, this end sends E1u on Ri, and
    /// R-bar-i and TRN2d only arrive once TRN2u is already on the line. The
    /// call comes up the same way.
    #[test]
    fn the_call_connects_when_a_dil_was_asked_for() {
        // Four segments of twenty-four symbols, each a reference at U_INFO
        // and a training symbol (V.90 8.4.1).
        let descriptor = Descriptor {
            signs: vec![true, false],
            training: vec![false, true],
            h: [3; 8],
            refs: [UINFO; 8],
            ucodes: vec![0x30, 0x38, 0x40, 0x48],
        };
        let down = pattern(1200, 0x1d1d);
        let mut call = Call::new(Settings { descriptor, ..settings() });
        call.reach_phase4();
        assert!(call.notes.iter().any(|note| note.starts_with("Ri at")), "CPt ended on Ri: {:#?}", call.notes);
        assert_eq!(call.far.source.out(), Out::Ri, "and R-bar-i has not been sent yet");
        call.exchange(&parameters(1, false), &down);
        let connected = call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON);
        assert!(connected, "{:?} at {}: {:#?}", call.modem.status(), call.modem.phase(), call.notes);
        let mut taken = Vec::new();
        for _ in 0..2000 {
            call.tick();
            taken.extend(call.modem.take_bits());
        }
        assert!(taken.len() >= down.len() && taken[..down.len()] == down[..], "the data after B1d, bit for bit");
    }

    /// Table 22 bit 48: "size of constellation used to transmit CPu, E2u, SUVu
    /// and TRN2u during training sequences: 0 = 4-point, 1 = 8-point". Eight
    /// points are Table 29's, three bits a symbol, so an SUVu's seventy-two
    /// bits are twenty-four symbols and E2u's twelve symbols thirty-six bits.
    #[test]
    fn jp_bit_48_asks_for_eight_point_trn2u() {
        let wanted = parameters(1, false);
        let mut call = Call::with_jp_bit_48(settings(), true);
        call.reach_phase4();
        assert_eq!(call.modem.jp().map(|jp| jp.eight_in_training), Some(true));
        call.exchange(&wanted, &[]);
        assert!(call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON), "{:#?}", call.notes);
        // Table 29: the odd magnitudes one to seven over the root of
        // twenty-one, and all four of them in use.
        let from = call.b1u_from.expect("B1u went out");
        let mut seen = [false; 4];
        for &sent in &call.up[..from] {
            let step = (sent.abs() * 21f64.sqrt() - 1.0) / 2.0;
            assert!((step - step.round()).abs() < 1e-9 && (0.0..=3.0).contains(&step.round()), "{sent} is not Table 29's");
            seen[step.round() as usize] = true;
        }
        assert_eq!(seen, [true; 4]);
        // Every sequence was read at three bits a symbol and filled to twelve
        // symbols of them (Tables 23 and 27).
        let suvus = call.far.suvus();
        assert!(suvus.len() >= 2 && call.far.cpus().len() == 1);
        // The reader takes TRN2u's first symbol as its reference, so the bit
        // an SUVu starts on is three short of a whole number of frames.
        for (at, _) in &suvus {
            assert_eq!((at - SUVU_TO_FILL + 3) % 36, 0, "an SUVu begins on a twelve-symbol boundary");
        }
        // 8.7.2: E2u is a data frame on "the corresponding TRN2u modulation",
        // so twelve symbols of eight points, thirty-six zero bits; and B1u
        // begins on the symbol after them (8.7.1), where a decoder that
        // starts its interval 0 reads its 48 frames of ones.
        call.run(600);
        let b1u = call.b1u_on_the_line(E2U_SYMBOLS);
        assert_eq!(Some(b1u), call.b1u_from, "B1u begins on the symbol after E2u's twelfth");
        assert_eq!(b1u % UP_INTERVALS, 0, "which is on the grid TRN2u kept");
        let mut decoder = UpDecoder::new(&as_sent(&wanted));
        let mut back = Vec::new();
        for &level in &call.up[b1u..] {
            decoder.feed(level);
            back.extend(decoder.take_bits());
        }
        let ones = 48 * up_bits(1) as usize;
        assert!(back.len() >= ones && back[..ones].iter().all(|&bit| bit), "B1u's 48 frames read as ones");
    }

    /// 8.7.6: TRN2u begins on the symbol after E1u's last, its scrambler is
    /// reset there, and its sign differential starts from E1u's last sign.
    #[test]
    fn trn2u_follows_e1u_with_a_reset_scrambler_and_e1u_s_sign() {
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.run(600);
        assert_ne!(call.before, 0.0, "E1u's last symbol is a symbol, not a gap");
        // "A scrambler output of 1 represents a negative voltage" (8.5.7).
        let mut model = Trn2uSender::seeded(Trn2uSize::Four, call.before < 0.0);
        for (n, &sent) in call.up.iter().take(600).enumerate() {
            let wanted = model.next_symbol();
            assert!((sent - wanted).abs() < 1e-12, "symbol {n}: sent {sent}, TRN2u is {wanted}");
        }
        // Table 28: four points, at one and three over the root of five.
        let inner = 1.0 / 5f64.sqrt();
        assert!(call.up.iter().all(|x| (x.abs() - inner).abs() < 1e-9 || (x.abs() - 3.0 * inner).abs() < 1e-9));
        assert_eq!(call.modem.phase(), "V.92 phase 4: TRN2u");
    }

    /// 9.6.2.1.1: SUVu starts once this end is ready for a CPd "and it has
    /// transmitted TRN2u for at least 12000T or received an SUVd sequence",
    /// and 8.7.6 has TRN2u end on a twelve-symbol boundary.
    #[test]
    fn trn2u_runs_for_twelve_thousand_symbols_or_until_an_suvd_arrives() {
        // No SUVd at all: the far end sends TRN2d and nothing else.
        let mut quiet = Call::new(settings());
        quiet.reach_phase4();
        assert!(quiet.run_until(|call| !call.far.suvus().is_empty(), SOON), "{:#?}", quiet.notes);
        let (at, first) = quiet.far.suvus()[0];
        let began = symbol_of(at - SUVU_TO_FILL);
        assert_eq!(began as u64, TRN2U_MINIMUM, "TRN2u ran {began} symbols");
        assert!(!first.ack, "no CPd has been received");
        // And until then it was TRN2u: scrambled ones, once the far end's
        // descrambler has filled.
        assert!(quiet.far.bits[64..at - SUVU_TO_FILL].iter().all(|&bit| bit), "TRN2u is scrambled ones (8.7.6)");
        // Table 27 in initial training: no wait asked for, no level measured,
        // no silence asked for.
        assert_eq!(first, Suvu { wait_for_cpu: false, level: None, silence: false, ack: false });

        // With an SUVd, it ends at the first frame boundary after the SUVd
        // has been read.
        let mut prompt = Call::new(settings());
        prompt.reach_phase4();
        prompt.far.suvd(false);
        assert!(prompt.run_until(|call| !call.far.suvus().is_empty(), SOON), "{:#?}", prompt.notes);
        let (at, _) = prompt.far.suvus()[0];
        let began = symbol_of(at - SUVU_TO_FILL);
        assert!((began as u64) < TRN2U_MINIMUM, "an SUVd ends TRN2u early: it ran {began} symbols");
        assert_eq!(began % UP_INTERVALS, 0, "and on a twelve-symbol boundary (8.7.6)");
        let heard = prompt.notes.iter().position(|note| note.starts_with("SUVd")).expect("the SUVd was read");
        let ended = prompt.notes.iter().position(|note| note.starts_with("TRN2u ends")).expect("TRN2u ended");
        assert!(heard < ended, "TRN2u ended before the SUVd arrived: {:#?}", prompt.notes);
    }

    /// 9.6.2.1.2 and 9.6.2.1.3: "a single CPu sequence followed by more SUVu
    /// sequences" after an SUVd, and repeated CPu only if nothing received up
    /// to "the entire CPd or SUVd sequence that is received after 100 ms plus
    /// a round-trip delay from the end of its CPu" carried the acknowledge
    /// bit. Figure 14 draws the repeats back to back.
    #[test]
    fn one_cpu_goes_after_the_first_suvd_and_is_repeated_only_when_unacknowledged() {
        // Acknowledged at once: one CPu and no more, from the first SUVd to
        // data mode.
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.exchange(&parameters(1, false), &[]);
        assert!(call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON));
        assert_eq!(call.far.cpus().len(), 1, "a single CPu");
        // It came after an SUVu, and SUVu followed it.
        let cpu_at = call.far.cpus()[0].0;
        let suvus = call.far.suvus();
        assert!(suvus.first().is_some_and(|(at, _)| *at < cpu_at), "SUVu first (9.6.2.1.1)");
        assert!(suvus.iter().any(|(at, _)| *at > cpu_at), "and more SUVu after it (9.6.2.1.2)");
        // The acknowledge bit follows the CPd: clear in the CPu, which went
        // before any CPd had been read, and set in the SUVu that came after.
        assert!(!call.far.cpus()[0].1.acknowledge);
        assert!(suvus.last().is_some_and(|(_, suvu)| suvu.ack));

        // Never acknowledged: the far end goes on sending SUVd with the bit
        // clear, as one that has not read the CPu would.
        let mut deaf = Call::new(settings());
        deaf.reach_phase4();
        deaf.far.suvd(false);
        assert!(deaf.run_until(|call| call.far.cpus().len() >= 4, SOON), "{:#?}", deaf.notes);
        let cpus = deaf.far.cpus();
        // A CPu is one length, and the finder answers at its last bit, so the
        // repeats are back to back if they are exactly that far apart.
        let length = cpus[0].1.to_bits_in(Layout::V92, trn2u_unit(false)).len();
        assert_eq!(cpus[2].0 - cpus[1].0, length, "repeated CPu, back to back (Figure 14)");
        assert_eq!(cpus[3].0 - cpus[2].0, length);
        // The first repeat waited out 100 ms, which is 800 symbols at no
        // round trip, from the end of the first CPu: two bits a symbol.
        let waited = (cpus[1].0 - length - cpus[0].0) / 2;
        assert!(waited >= 800, "the first repeat came {waited} symbols after the CPu");
        // And not much longer: the SUVd that straddles the deadline, and the
        // SUVu in flight when it has been read.
        assert!(waited <= 800 + 18 + 36 + 72, "it came {waited} symbols after");
        // Between the CPu and its first repeat it was SUVu, none of them
        // acknowledging anything.
        let between: Vec<_> = deaf.far.suvus().into_iter().filter(|(at, _)| *at > cpus[0].0 && *at < cpus[1].0).collect();
        assert!(between.len() >= 20 && between.iter().all(|(_, suvu)| !suvu.ack), "{} SUVu between", between.len());
        // A group of CPu is the same CPu (8.7.3).
        assert!(cpus.iter().all(|(_, cpu)| *cpu == cpus[0].1));
    }

    /// 9.6.2.1.4: once an acknowledged sequence has been both sent and
    /// received, this end is to "complete sending the current CPu sequence
    /// and transmit E2u": nothing is held on the line in between.
    #[test]
    fn e2u_follows_the_first_acknowledged_suvu() {
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.far.suvd(false);
        assert!(call.run_until(|call| !call.far.suvus().is_empty(), SOON));
        call.far.cpd(&Cpd::from(&parameters(1, false)));
        call.far.suvd(false);
        assert!(call.run_until(|call| !call.far.cpus().is_empty(), SOON));
        // From here the far end acknowledges, and never sends Ed: the SUVd'
        // alone is what 9.6.2.1.4 asks for.
        call.far.suvd(true);
        assert!(call.run_until(|call| call.b1u_from.is_some(), SOON), "{:#?}", call.notes);
        let acked = call.far.suvus().into_iter().filter(|(_, suvu)| suvu.ack).count();
        // The acknowledge bit can first go out in the SUVu that starts after
        // the CPd was read, and the far end's own may arrive during it or
        // during the one after.
        assert!((1..=3).contains(&acked), "{acked} SUVu' went out before E2u");
        assert_eq!(Some(call.b1u_on_the_line(E2U_SYMBOLS)), call.b1u_from);

        // 9.6.2.1.5 and 9.6.2.1.6 are two events. B1u's last symbol turns
        // circuit 106 ON whatever has come down; circuit 104 stays clamped
        // and 109 OFF until B1d has, and with neither Ed nor B1d on the line
        // that is where they stay.
        assert!(call.run_until(|call| call.modem.circuit106(), SOON), "{:#?}", call.notes);
        assert_eq!(call.up.len() - call.b1u_from.unwrap(), 48 * UP_INTERVALS + 1, "106 comes ON as B1u ends");
        call.run(2000);
        assert!(call.modem.circuit106() && !call.modem.circuit104() && !call.modem.circuit109());
        assert_eq!(call.modem.status(), Status::Phase4, "one direction is not a connection");
        assert_eq!(call.modem.fill_bits(), up_bits(1) as usize, "but the DTE's bits are taken (9.6.2.1.5)");
        assert!(call.modem.take_bits().is_empty(), "and none are handed to it");
        // Then Ed and B1d, and the other three follow.
        let cpu = call.far.cpus()[0].1.clone();
        call.far.ed_b1d_and_data(&cpu, &[]);
        assert!(call.run_until(|call| call.modem.circuit104(), SOON), "{:#?}", call.notes);
        assert!(call.modem.circuit109());
        assert!(matches!(call.modem.status(), Status::Connected { .. }));
    }

    /// 8.7.2: "E2u shall be extended by a single symbol if bit 29 of CPd is
    /// set", and 8.7.1 still has B1u's first symbol begin data frame interval
    /// 0 with every memory zero.
    #[test]
    fn an_extension_asked_for_in_cpd_bit_29_is_sent_and_b1u_still_starts_interval_zero() {
        let wanted = parameters(1, true);
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.exchange(&wanted, &[]);
        assert!(call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON));
        call.run(600);
        // Thirteen symbols of zeros, and B1u on the fourteenth.
        let from = call.b1u_on_the_line(E2U_SYMBOLS + 1);
        assert_eq!(Some(from), call.b1u_from, "B1u begins after thirteen symbols of E2u");
        assert_eq!(from % UP_INTERVALS, 1, "which is one symbol off the grid TRN2u kept");
        // A decoder that starts its interval 0 there reads B1u's ones.
        let wire = as_sent(&wanted);
        let mut decoder = UpDecoder::new(&wire);
        let mut back = Vec::new();
        for &level in &call.up[from..] {
            decoder.feed(level);
            back.extend(decoder.take_bits());
        }
        let ones = 48 * up_bits(1) as usize;
        assert!(back.len() >= ones, "{} bits", back.len());
        assert!(back[..ones].iter().all(|&bit| bit), "B1u's 48 frames read as ones from interval 0");
    }

    /// 8.7.1: B1u is "48 data frames of scrambled binary ones transmitted
    /// using the data mode constellation parameters from the preceding CPd",
    /// from n = 0 with "the scrambler, modulus encoder, convolutional encoder,
    /// precoder and prefilter memories ... initialized to zero".
    #[test]
    fn b1u_is_the_frames_the_cpd_parameters_predict() {
        let wanted = parameters(7, false);
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.exchange(&wanted, &[]);
        assert!(call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON));
        call.run(200);
        let from = call.b1u_on_the_line(E2U_SYMBOLS);
        // The model: the 6.4 chain built afresh from what the CPd's fields
        // said, fed 48 frames of ones.
        let wire = as_sent(&wanted);
        let mut model = UpEncoder::new(&wire).expect("parameters the chain can be built from");
        let frames = model.b1u();
        assert_eq!(frames.len(), 48 * UP_INTERVALS, "48 data frames of twelve symbols");
        for (n, symbol) in frames.iter().enumerate() {
            let sent = call.up[from + n];
            // 6.4.2: the line carries v(n) "multiplied by a gain, G".
            assert!((sent - symbol.out).abs() < 1e-12, "B1u symbol {n}: sent {sent}, the CPd predicts {}", symbol.out);
        }
        // And the data mode that follows carries on from B1u's memories: an
        // idle DTE is more ones through the same encoder.
        for n in 0..96 {
            let wanted = model.next_symbol(|| true).out;
            let sent = call.up[from + frames.len() + n];
            assert!((sent - wanted).abs() < 1e-12, "symbol {n} after B1u: sent {sent}, wanted {wanted}");
        }
    }

    /// Table 23 bits 21:25: the rate has to be one Jd enabled (Table 21), and
    /// one the constellations carry (5.4.3/V.90). Bits 49:50 have to be
    /// inside Jd's look-ahead.
    #[test]
    fn the_cpu_asks_for_a_downstream_rate_the_jd_enabled() {
        // Thirty-two points in each interval carry thirty bits, which with
        // six sign bits is D = 36: drn 16, 48 000 bit/s.
        let mut modem = Modem::new(settings());
        modem.far_jd = Some(Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() });
        let cpu = modem.data_cp().expect("a rate fits");
        assert!(cpu.data_mode);
        assert_eq!(cpu.drn, 16, "the fastest the constellations carry");
        assert_eq!(modem.downstream, 48_000);

        // A Jd that stops at 40 000 (drn 10) is obeyed.
        let mut modem = Modem::new(settings());
        modem.far_jd = Some(Jd { rates: (1 << 10) - 1, lookahead: 1, ..Jd::default() });
        assert_eq!(modem.data_cp().map(|cpu| cpu.drn), Some(10), "the fastest the Jd enabled");

        // A rate asked for is kept when Jd enabled it, and replaced when not.
        let asked = Cp { drn: 12, data_mode: true, lookahead: 3, ..cpt() };
        let mut modem = Modem::new(Settings { cpu: Some(asked.clone()), ..settings() });
        modem.far_jd = Some(Jd { rates: Jd::ALL_RATES, lookahead: 2, ..Jd::default() });
        let cpu = modem.data_cp().expect("a rate fits");
        assert_eq!(cpu.drn, 12);
        assert_eq!(cpu.lookahead, 2, "no more look-ahead than Jd bits 49:50 allow");
        let mut modem = Modem::new(Settings { cpu: Some(asked), ..settings() });
        modem.far_jd = Some(Jd { rates: 0b111, lookahead: 1, ..Jd::default() });
        assert_eq!(modem.data_cp().map(|cpu| cpu.drn), Some(3), "drn 12 was not enabled; 30 666 was");

        // And a Jd that enables nothing the constellations carry leaves no
        // CPu to send: drn 0 would be a cleardown (Table 23), and that is
        // not this modem's to decide.
        let mut modem = Modem::new(settings());
        modem.far_jd = Some(Jd { rates: 1 << 21, lookahead: 1, ..Jd::default() });
        assert_eq!(modem.data_cp(), None);
        assert_eq!(modem.cpu(), None);

        // On the line: the CPu a digital modem reads is that CPu, with the
        // V.92 header -- type 1 -- and no acknowledge before a CPd. The CPt
        // it is made from is given a V.90 upstream rate mask here, to show
        // that none of it reaches the line.
        let mut call = Call::new(Settings { cpt: Cp { upstream_rates: 0x1abc, ..cpt() }, ..settings() });
        call.reach_phase4();
        call.far.suvd(false);
        assert!(call.run_until(|call| !call.far.cpus().is_empty(), SOON));
        let (at, sent) = call.far.cpus()[0].clone();
        assert_eq!(Some(&sent), call.modem.cpu());
        assert_eq!(sent.drn, 16, "the script's Jd enables every rate");
        assert_eq!(sent.constellations, cpt().constellations);
        // Table 23 bits 36:48: "Reserved for the ITU: These bits are set to 0
        // by the analogue modem". The rate mask went up in Ja (Table 20).
        // Read where they are on the line, because a V.92 reader does not
        // interpret them and so cannot say what they were.
        let length = sent.to_bits_in(Layout::V92, trn2u_unit(false)).len();
        let first = at + 1 - length;
        let on_the_line = &call.far.bits[first..=at];
        assert!(on_the_line[..17].iter().all(|&bit| bit) && !on_the_line[17], "the CPu's frame sync and start bit");
        assert!(on_the_line[19] && !on_the_line[20], "type 1, a CPu (bits 19:20)");
        assert!(on_the_line[36..=48].iter().all(|&bit| !bit), "bits 36:48 went out as zeros");
        assert_eq!(call.modem.settings().cpt.upstream_rates, 0x1abc, "though the CPt it came from had some set");
    }

    /// Table 30 bits 22:26: "drn = 0 shall indicate cleardown" (and 9.11).
    #[test]
    fn a_cpd_with_drn_0_clears_the_call_down() {
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.far.suvd(false);
        assert!(call.run_until(|call| !call.far.suvus().is_empty(), SOON));
        call.far.cpd(&Cpd::cleardown(false));
        call.far.suvd(false);
        let cleared = call.run_until(|call| call.modem.status() == Status::ClearedDown, SOON);
        assert!(cleared, "{:?}: {:#?}", call.modem.status(), call.notes);
        // The call is over: nothing more goes out.
        let from = call.up.len();
        call.run(400);
        assert!(call.up[from + 1..].iter().all(|&x| x == 0.0), "the line is silent after a cleardown");
        assert!(!call.modem.circuit106() && !call.modem.circuit109());
    }

    /// 8.8.3 removes an absent part's bits and gives it no meaning; the
    /// reading here is "as before", so a *first* CPd with a part missing has
    /// nothing to fall back on and is answered with a retrain (9.6.2.2).
    #[test]
    fn a_first_cpd_missing_a_part_asks_for_a_retrain() {
        for part in 0..3 {
            let mut cpd = Cpd::from(&parameters(1, false));
            match part {
                0 => cpd.moduli = None,
                1 => cpd.filters = None,
                _ => cpd.sets = None,
            }
            let mut call = Call::new(settings());
            call.reach_phase4();
            call.far.suvd(false);
            assert!(call.run_until(|call| !call.far.suvus().is_empty(), SOON));
            call.far.cpd(&cpd);
            call.far.suvd(false);
            let asked = call.run_until(|call| matches!(call.modem.status(), Status::Retrain(_)), SOON);
            assert!(asked, "part {part}: {:?}: {:#?}", call.modem.status(), call.notes);
            assert_eq!(call.modem.status(), Status::Retrain("the first CPd is missing a part"));
            // 9.7.2.1: circuit 106 OFF, circuit 104 clamped, silence.
            assert!(!call.modem.circuit106() && !call.modem.circuit104());
            let from = call.up.len();
            call.run(560);
            assert!(call.up[from + 1..].iter().all(|&x| x == 0.0), "silence for the 70 ms and after");
        }

        // A later CPd may leave parts out: they stay as the first one said.
        let wanted = parameters(1, false);
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.far.suvd(false);
        assert!(call.run_until(|call| !call.far.suvus().is_empty(), SOON));
        call.far.cpd(&Cpd::from(&wanted));
        let mut bare = Cpd::from(&wanted);
        (bare.moduli, bare.filters, bare.sets, bare.ack) = (None, None, None, true);
        call.far.cpd(&bare);
        call.far.suvd(true);
        assert!(call.run_until(|call| call.b1u_from.is_some(), SOON), "{:?}: {:#?}", call.modem.status(), call.notes);
        assert_eq!(call.modem.parameters(), Some(&as_sent(&wanted)), "the parts the second CPd left out stand");
    }

    /// 9.6.2.2.1: no B1d "within 20 s plus 6 round-trip delays from the end
    /// of sending INFO1a" is a retrain, and Phase 4 does not stop the clock.
    #[test]
    fn no_b1d_within_20_s_and_six_round_trips_asks_for_a_retrain() {
        // A tenth of a second of round trip, and INFO1a ended half a second
        // before this modem was built: 20 + 0.6 - 0.5 seconds are left.
        let mut call = Call::new(Settings { round_trip: 0.1, since_info1a: 0.5, ..settings() });
        call.reach_phase4();
        // The far end sends TRN2d and SUVd and never a CPd.
        call.far.suvd(false);
        let asked = call.run_until(|call| call.modem.status() != Status::Phase4, 200_000);
        assert!(asked, "still {} after {} symbols", call.modem.phase(), call.modem.elapsed());
        assert_eq!(call.modem.status(), Status::Retrain("no B1d within 20 s and six round trips"));
        let due = (20.1 * FS) as u64;
        assert!((due..=due + 2).contains(&call.modem.elapsed()), "at {} symbols, due at {due}", call.modem.elapsed());
        assert!(!call.far.suvus().is_empty(), "it was in the exchange when the clock ran out");
        assert!(!call.modem.circuit106() && !call.modem.circuit104());
        let from = call.up.len();
        call.run(560);
        assert!(call.up[from + 1..].iter().all(|&x| x == 0.0), "silence (9.7.2.1)");
    }

    /// Put Tone B on the line and check what 9.7.2.2 requires of the modem.
    ///
    /// The DTE here takes its bits as they come, a tick at a time. What it is
    /// handed while the tone has not yet stood for "more than 50 ms" is what
    /// the data decoder made of the tone, and no clamp can take that back;
    /// what is required, and checked, is that nothing arrives once circuit
    /// 104 has been clamped.
    fn tone_b_is_answered(call: &mut Call, during: &str) {
        let carrier = call.modem.circuit109();
        call.tone = Some(0);
        let before = call.modem.elapsed();
        let mut asked = false;
        for _ in 0..1600 {
            call.tick();
            call.modem.take_bits();
            if matches!(call.modem.status(), Status::Retrain(_)) {
                asked = true;
                break;
            }
        }
        assert!(asked, "{during}: no retrain, {:?} at {}", call.modem.status(), call.modem.phase());
        assert_eq!(
            call.modem.status(),
            Status::Retrain("tone B: the digital modem is retraining"),
            "{during}: the reason says the far end asked"
        );
        // "After detecting Tone B for more than 50 ms" (9.7.2.2): not before
        // 400 symbols of it, and not long after.
        let took = call.modem.elapsed() - before;
        assert!((400..=800).contains(&took), "{during}: answered after {took} symbols of tone B");
        // "turn OFF circuit 106, clamp circuit 104 to binary one and transmit
        // silence". Circuit 109 is not in the sentence, and stays where it
        // was.
        assert!(!call.modem.circuit106(), "{during}: 106 is OFF");
        assert!(!call.modem.circuit104(), "{during}: 104 is clamped");
        assert_eq!(call.modem.circuit109(), carrier, "{during}: 109 is as it was");
        assert_eq!(call.modem.fill_bits(), 0, "{during}: nothing is taken from the DTE");
        assert_eq!(call.modem.phase(), "V.92: silent, for a retrain");
        let from = call.up.len();
        for _ in 0..600 {
            call.tick();
            assert!(call.modem.take_bits().is_empty(), "{during}: nothing reaches the DTE once 104 is clamped");
        }
        assert!(call.up[from..].iter().all(|&x| x == 0.0), "{during}: the line is silent");
    }

    /// 9.5.2.2: "If Tone B is detected during Phase 3, the analogue modem
    /// shall respond to retrain according to 9.7.2.2" -- while TRN1d is
    /// training this end, and while this end's own TRN1u is on the line.
    #[test]
    fn tone_b_during_phase_3_asks_for_a_retrain() {
        for during in ["V.92 phase 3: TRN1d", "V.92 phase 3: TRN1u over the DIL"] {
            let mut call = Call::new(settings());
            let reached = call.run_until(|call| call.modem.phase() == during, 60_000);
            assert!(reached, "never reached {during}: {:#?}", call.notes);
            call.run(200);
            assert_eq!(call.modem.status(), Status::Running, "{during} is phase 3");
            tone_b_is_answered(&mut call, during);
        }
    }

    /// And nothing of an ordinary Phase 3 is taken for Tone B. Sd, TRN1d, Jd,
    /// Jp, SCR and Ri are held against the watch by every test here that
    /// reaches Phase 4; this is the DIL that comes nearest the tone.
    #[test]
    fn a_dil_in_short_segments_is_not_taken_for_tone_b() {
        // A sign pattern with a period of twenty symbols is a square wave at
        // 400 Hz: a line at 1200 Hz, its third harmonic, and nothing at 1050
        // or 1350. But "the patterns are restarted at the beginning of each
        // DIL-segment" (V.90 8.4.1), and a segment of ninety-six symbols is
        // 12 ms and not a whole number of periods, so the line turns at
        // every boundary and never stands for the 50 ms of 9.7.2.2.
        let mut signs = vec![true; 10];
        signs.extend([false; 10]);
        let descriptor = Descriptor {
            signs,
            training: vec![true],
            h: [15; 8],
            refs: [UINFO; 8],
            ucodes: vec![0x20, 0x48, 0x28, 0x50, 0x30, 0x58, 0x38, 0x60, 0x40, 0x68],
        };
        let mut call = Call::new(Settings { descriptor, ..settings() });
        call.reach_phase4();
        assert_eq!(call.modem.status(), Status::Phase4);
    }

    /// 9.6.2.2: "If Tone B is detected during Phase 4, the analogue modem
    /// shall respond to retrain according to 9.7.2.2" -- during TRN2u, during
    /// the exchange and during B1d alike.
    #[test]
    fn tone_b_during_phase_4_asks_for_a_retrain() {
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.run(3000);
        assert_eq!(call.modem.phase(), "V.92 phase 4: TRN2u");
        tone_b_is_answered(&mut call, "TRN2u");

        let mut call = Call::new(settings());
        call.reach_phase4();
        call.far.suvd(false);
        assert!(call.run_until(|call| call.far.suvus().len() >= 3, SOON));
        tone_b_is_answered(&mut call, "the exchange");

        let mut call = Call::new(settings());
        call.reach_phase4();
        call.exchange(&parameters(1, false), &[]);
        assert!(call.run_until(|call| call.notes.iter().any(|note| note.starts_with("Ed")), SOON));
        call.run(60);
        assert!(!call.modem.circuit109(), "B1d is still arriving");
        tone_b_is_answered(&mut call, "B1d");
    }

    /// The same in data mode, with data crossing: the digital modem's retrain
    /// begins with Tone B whenever it begins (9.7.1.1), and this end answers
    /// it as 9.7.2.2 says.
    #[test]
    fn tone_b_in_data_mode_asks_for_a_retrain() {
        let down = pattern(20_000, 0xfeed);
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.exchange(&parameters(7, false), &down);
        assert!(call.run_until(|call| matches!(call.modem.status(), Status::Connected { .. }), SOON));
        call.modem.send_bits(&pattern(4000, 0xcafe));
        call.run(1500);
        let taken = call.modem.take_bits();
        assert!(taken.len() > 1000 && taken[..] == down[..taken.len()], "data was crossing");
        assert!(matches!(call.modem.status(), Status::Connected { .. }), "and no retrain without a tone");
        assert!(call.modem.circuit109(), "109 is ON in data mode, and the retrain is to leave it");
        tone_b_is_answered(&mut call, "data mode");
    }

    /// A DIL of four segments, `h + 1` six-symbol repetitions each: a
    /// reference at U_INFO and a training symbol (V.90 8.4.1).
    fn dil(h: u8) -> Descriptor {
        Descriptor {
            signs: vec![true, false],
            training: vec![false, true],
            h: [h; 8],
            refs: [UINFO; 8],
            ucodes: vec![0x30, 0x38, 0x40, 0x48],
        }
    }

    /// 9.5.2.1.9: "The length for this segment of TRN1u shall be in multiple
    /// of 12 symbols", and 8.5.7: "The digital modem shall keep data frame
    /// interval alignment from the first symbol of the second TRN1u signal."
    /// Everything from there to B1u is whole twelves -- the twenty-four ones,
    /// each CPt, E1u, TRN2u, SUVu, CPu, E2u -- so that 8.7.1's "the first
    /// symbol of B1u shall begin data frame interval 0" is the interval 0
    /// the digital modem has been counting to.
    ///
    /// What ends that TRN1u does not arrive on a twelfth symbol: Ri when no
    /// DIL was asked for (9.5.2.1.10), and the DIL's own last symbol when one
    /// longer than 2040T was (9.5.2.1.11).
    #[test]
    fn the_second_trn1u_is_whole_twelves_and_so_is_everything_up_to_b1u() {
        let long = dil(127);
        assert!(long.len() > TRN1U_MINIMUM as usize, "a DIL of {} symbols outlasts 2040T", long.len());
        for descriptor in [Descriptor::none(), long] {
            let asked = descriptor.len() as u64;
            let mut call = Call::new(Settings { descriptor, ..settings() });
            call.reach_phase4();
            assert!(call.trn1u >= asked.max(12), "TRN1u covered the DIL: {} symbols", call.trn1u);
            if asked > 0 {
                assert!(call.trn1u >= TRN1U_MINIMUM, "at least 2040T with a DIL (9.5.2.1.9)");
            }
            assert_eq!(call.trn1u % 12, 0, "a DIL of {asked}: TRN1u ran {} symbols", call.trn1u);
            assert_eq!(call.grid % 12, 0, "a DIL of {asked}: {} symbols from TRN1u's first to TRN2u's", call.grid);
            // And on through Phase 4: `up` begins at TRN2u's first symbol.
            call.exchange(&parameters(1, false), &[]);
            assert!(call.run_until(|call| call.b1u_from.is_some(), SOON), "{:#?}", call.notes);
            assert_eq!(call.b1u_from.unwrap() % 12, 0, "a DIL of {asked}: B1u's first symbol is interval 0");
        }
    }

    /// Table 23: the only thing a CPu can say with no rate to ask for is drn
    /// 0, "indicates cleardown". That ends the call, which nothing asked this
    /// modem to do; 9.5.2.2 and 9.6.2.2 let it retrain "at any time", and
    /// that is what it does, with no CPu sent at all.
    #[test]
    fn no_downstream_rate_is_a_retrain_and_not_a_cleardown() {
        // Four points an interval carry twelve bits a frame, eighteen with
        // the six signs: enough for a CPt at D = 16 (Table 17/V.90) and for
        // no data mode rate, the slowest of which is D = 21 (Table 2/V.90).
        let mask = [20u8, 28, 36, 44].iter().fold(0u128, |mask, &u| mask | 1 << u);
        let sparse = Cp { drn: 8, constellations: vec![mask], ..Cp::default() };
        let mut call = Call::new(Settings::new(Descriptor::none(), JA_MASK_ALL, sparse));
        let stopped = call.run_until(|call| call.modem.status() != Status::Running, 60_000);
        assert!(stopped, "phase 3 stopped at {}: {:#?}", call.modem.phase(), call.notes);
        assert_eq!(
            call.modem.status(),
            Status::Retrain("no downstream rate that Jd enabled fits the constellations")
        );
        assert_eq!(call.modem.cpu(), None);
        let from = call.up.len();
        call.run(3000);
        assert!(call.up[from..].iter().all(|&x| x == 0.0), "silence (9.7.2.1)");
        assert!(call.far.cpus().is_empty() && call.far.suvus().is_empty(), "and nothing of an exchange");
    }

    /// 9.6.2.1.6 has B1d follow Ed, and 9.6.1.1.4 has the digital modem send
    /// exactly one more sequence between the acknowledgement and Ed. If that
    /// one arrives damaged, Ed cannot be told from the zeros a CPd is full
    /// of -- but B1d can still be told from everything, by starting from
    /// zero as V.90 8.6.1 says it does.
    #[test]
    fn b1d_is_found_behind_a_sequence_that_arrived_damaged() {
        let down = pattern(1500, 0x5eed);
        let mut call = Call::new(settings());
        call.reach_phase4();
        let cpu = call.acknowledged(&parameters(1, false));
        call.far.spoiled_suvd(true);
        call.far.ed_b1d_and_data(&cpu, &down);
        call.connects_and_reads(&down);
        assert!(call.noted("B1d found; the Ed before it was not read"), "{:#?}", call.notes);
        assert!(!call.noted("Ed, reading for B1d"), "Ed was read after all: {:#?}", call.notes);
        assert!(call.noted("B1d, 48 of 48 frames scrambled ones"), "{:#?}", call.notes);
    }

    /// B1d begins where B1d begins. 8.8.2 makes Ed two data frames, and
    /// Tables 30 and 31 fill a sequence "to the next multiple of 6 symbols";
    /// a far end that sends Ed twice over, or a frame of fill too many, moves
    /// the place Ed points at by whole frames, and the forty-eight frames of
    /// ones would hide that and hand the difference to the DTE as data.
    #[test]
    fn b1d_is_placed_by_its_own_first_frame_and_not_by_the_length_of_ed() {
        // Ed twice: four frames of zeros.
        let down = pattern(1500, 0xed2);
        let mut call = Call::new(settings());
        call.reach_phase4();
        let cpu = call.acknowledged(&parameters(1, false));
        call.far.ed();
        call.far.ed_b1d_and_data(&cpu, &down);
        call.connects_and_reads(&down);
        assert!(call.noted("B1d began 2 frames after Ed's two"), "{:#?}", call.notes);
        assert!(call.noted("B1d, 48 of 48 frames scrambled ones"), "{:#?}", call.notes);

        // A frame of fill too many. With D = 26 an SUVd's fifty-two bits are
        // two frames exactly; filled to six *bits* first and then to a frame
        // they are three, the third all zeros, and Ed's first frame then
        // looks like its second.
        let mut call = Call::new(Settings::new(Descriptor::none(), JA_MASK_ALL, Cp { drn: 18, ..cpt() }));
        assert_eq!(call.far.unit, 26, "D of a CPt with drn 18 (Table 23)");
        call.far.unit = INTERVALS;
        call.reach_phase4();
        let cpu = call.acknowledged(&parameters(1, false));
        call.far.ed_b1d_and_data(&cpu, &down);
        call.connects_and_reads(&down);
        assert!(call.noted("B1d began 1 frames after Ed's two"), "{:#?}", call.notes);
        assert!(call.noted("B1d, 48 of 48 frames scrambled ones"), "{:#?}", call.notes);

        // And the same D with the fill Table 31 asks for: Ed and B1d agree.
        let mut call = Call::new(Settings::new(Descriptor::none(), JA_MASK_ALL, Cp { drn: 18, ..cpt() }));
        call.reach_phase4();
        call.exchange(&parameters(1, false), &down);
        call.connects_and_reads(&down);
        assert!(call.noted("Ed, reading for B1d"), "{:#?}", call.notes);
        assert!(!call.notes.iter().any(|note| note.starts_with("B1d began")), "{:#?}", call.notes);
    }

    /// A B1d whose first frames are lost has no first frame to be found by.
    /// The frame after Ed is then the only place there is, and three quarters
    /// of the forty-eight frames reading as ones is what makes it B1d.
    #[test]
    fn a_b1d_whose_first_frames_are_lost_is_placed_by_ed() {
        let down = pattern(1500, 0xd0);
        let mut call = Call::new(settings());
        call.reach_phase4();
        // Three data frames of nothing.
        call.dropout = 0..3 * INTERVALS;
        call.exchange(&parameters(1, false), &down);
        call.connects_and_reads(&down);
        assert!(call.noted("Ed, reading for B1d"), "{:#?}", call.notes);
        let counted = call.notes.iter().find(|note| note.starts_with("B1d, ")).expect("B1d was counted");
        assert!(counted != "B1d, 48 of 48 frames scrambled ones", "the dropout cost nothing: {counted}");
    }

    /// 9.6.2.1.6 unclamps circuit 104 "after receiving B1d". What follows Ed
    /// and is not B1d leaves it clamped, and since B1d is sent once the
    /// answer is a retrain (9.6.2.2) well inside the twenty seconds.
    #[test]
    fn what_follows_ed_and_is_not_b1d_asks_for_a_retrain() {
        let mut call = Call::new(settings());
        call.reach_phase4();
        // The line goes dead as B1d begins and stays dead.
        call.dropout = 0..usize::MAX;
        call.exchange(&parameters(1, false), &[]);
        assert!(call.run_until(|call| call.noted("Ed, reading for B1d"), SOON), "{:#?}", call.notes);
        let ed = call.modem.elapsed();
        let asked = call.run_until(|call| matches!(call.modem.status(), Status::Retrain(_)), SOON);
        assert!(asked, "{:?}: {:#?}", call.modem.status(), call.notes);
        assert_eq!(call.modem.status(), Status::Retrain("what followed Ed was not B1d"));
        // Forty-eight frames to count, and an eighth of a second in case it
        // was Tone B that took their place.
        let took = call.modem.elapsed() - ed;
        assert!((1288..=1300).contains(&took), "asked {took} symbols after Ed");
        assert!(!call.modem.circuit104() && !call.modem.circuit109() && !call.modem.circuit106());
        assert!(call.modem.take_bits().is_empty(), "nothing was handed to the DTE");
    }

    /// A CPd this end cannot obey is not acknowledged: acknowledging it
    /// promises a B1u made with it (9.6.2.1.2, 9.6.2.1.5). The rate has to be
    /// one Ja offered (Table 20), the filters ones INFO1a said this end can
    /// hold (Table 18 bits 12:17), and the moduli have to carry the rate's K
    /// bits (6.4.1). 9.6.2.2 is the retrain.
    #[test]
    fn a_cpd_that_cannot_be_obeyed_asks_for_a_retrain() {
        let good = parameters(7, false);
        // INFO1a's code 0 offers neither optional section, so a precoder
        // feed-forward section is one this end never said it had.
        let unoffered = Parameters {
            filters: Filters { z1: vec![0.25], ..good.filters.clone() },
            ..good.clone()
        };
        // Twelve intervals of modulus 2 carry twelve bits; drn 7 is K = 48.
        let small = Parameters { moduli: [2; UP_INTERVALS], ..good.clone() };
        let cases = [
            // Every rate but drn 7's 32 000.
            (JA_MASK_ALL & !(1 << 6), &good, "the CPd asks for an upstream rate Ja did not offer"),
            (JA_MASK_ALL, &unoffered, "the precoder has a feed-forward section this end did not offer"),
            (JA_MASK_ALL, &small, "the data frame carries more bits than the moduli can hold"),
        ];
        for (mask, asked, why) in cases {
            let mut call = Call::new(Settings::new(Descriptor::none(), mask, cpt()));
            call.reach_phase4();
            call.far.suvd(false);
            assert!(call.run_until(|call| !call.far.suvus().is_empty(), SOON));
            call.far.cpd(&Cpd::from(asked));
            call.far.suvd(false);
            let stopped = call.run_until(|call| matches!(call.modem.status(), Status::Retrain(_)), SOON);
            assert!(stopped, "{why}: {:?}: {:#?}", call.modem.status(), call.notes);
            assert_eq!(call.modem.status(), Status::Retrain(why));
            assert_eq!(call.modem.parameters(), None, "{why}: nothing of it was kept");
            assert!(call.far.suvus().iter().all(|(_, suvu)| !suvu.ack), "{why}: and it was never acknowledged");
            assert!(call.far.cpus().iter().all(|(_, cpu)| !cpu.acknowledge));
        }
    }

    /// 8.8.3: a part that is absent has its bits "removed from the CPd
    /// sequence", and the reading here is that it stands as the CPd before
    /// said. The other half of that is that a part which *is* there replaces
    /// the one before: a second CPd with a new rate and new moduli, and no
    /// filters or constellations, is those moduli on the first one's filters
    /// and sets, and B1u is sent with exactly that.
    #[test]
    fn a_later_cpd_replaces_the_parts_it_carries_and_keeps_the_rest() {
        let first = parameters(7, false);
        // K = 36 on moduli of 8, which the first CPd's sixteen-level sets
        // still hold a class for (6.4.2).
        let merged = Parameters { drn: 1, moduli: [8; UP_INTERVALS], ..first.clone() };
        let mut second = Cpd::from(&merged);
        // It comes after the CPu has been read, so it is a CPd' (9.6.1.1.2).
        (second.filters, second.sets, second.ack) = (None, None, true);
        let mut call = Call::new(settings());
        call.reach_phase4();
        call.far.suvd(false);
        assert!(call.run_until(|call| !call.far.suvus().is_empty(), SOON));
        call.far.cpd(&Cpd::from(&first));
        call.far.suvd(false);
        assert!(call.run_until(|call| call.modem.parameters().is_some(), SOON), "{:#?}", call.notes);
        assert_eq!(call.modem.parameters(), Some(&as_sent(&first)));
        call.far.cpd(&second);
        call.far.suvd(true);
        assert!(call.run_until(|call| call.modem.circuit106(), SOON), "{:?}: {:#?}", call.modem.status(), call.notes);
        let wire = as_sent(&merged);
        assert_eq!(call.modem.parameters(), Some(&wire), "the new rate and moduli on the old filters and sets");
        assert_eq!(call.modem.cpd().map(|cpd| (cpd.drn, cpd.moduli)), Some((1, Some([8; UP_INTERVALS]))));
        // And that is what B1u was sent with.
        call.run(600);
        let from = call.b1u_on_the_line(E2U_SYMBOLS);
        let mut decoder = UpDecoder::new(&wire);
        let mut back = Vec::new();
        for &level in &call.up[from..] {
            decoder.feed(level);
            back.extend(decoder.take_bits());
        }
        let ones = 48 * up_bits(1) as usize;
        assert!(back.len() >= ones && back[..ones].iter().all(|&bit| bit), "B1u reads as ones at the second CPd's rate");
    }

    /// 9.6.2.1.1: SUVu waits for "ready to receive a CPd sequence" as well as
    /// for the 12000T, and an end that has not found TRN2d's frames could not
    /// read a CPd if one came. So with nothing coming down, TRN2u goes on.
    #[test]
    fn trn2u_goes_on_past_12000t_while_trn2d_has_not_been_found() {
        // With a DIL, R-bar-i and TRN2d come after E1u (9.5.1.1.12), so the
        // line can go dead between them.
        let mut call = Call::new(Settings { descriptor: dil(3), ..settings() });
        call.mute = true;
        call.reach_phase4();
        call.run(TRN2U_MINIMUM + 2000);
        assert!(call.modem.down.as_ref().is_some_and(|down| !down.found()), "R-bar-i was heard and TRN2d was not");
        assert_eq!(call.modem.phase(), "V.92 phase 4: TRN2u");
        assert!(call.far.suvus().is_empty(), "no SUVu from an end that could not read a CPd");
        assert!(call.far.bits[64..].iter().all(|&bit| bit), "TRN2u, scrambled ones, all the way (8.7.6)");
        assert_eq!(call.modem.status(), Status::Phase4, "until 9.6.2.2.1's timer says otherwise");
    }

    /// 9.6.2.2.1's timer counts "from the end of sending INFO1a", so it is
    /// running all through Phase 3; and what it starts is 9.7.2.1, which is
    /// silence. The 70 ms of silence that opens Phase 3 is not that silence,
    /// and must not be followed by Ru again.
    #[test]
    fn the_start_up_timer_runs_in_phase_3_and_leaves_the_line_silent() {
        // All but a tenth of a second of the twenty had gone when this modem
        // was built: the timer runs out 800 symbols in, during Ru.
        let mut modem = Modem::new(Settings { since_info1a: 19.9, ..settings() });
        let sent: Vec<f64> = (0..6000).map(|_| modem.step(0.0)).collect();
        assert_eq!(modem.status(), Status::Retrain("no B1d within 20 s and six round trips"));
        assert!(sent[..560].iter().all(|&x| x == 0.0), "70 ms of silence (9.5.2.1.1)");
        assert!(sent[560..800].iter().all(|&x| x != 0.0), "then Ru");
        assert!(sent[801..].iter().all(|&x| x == 0.0), "and nothing after the timer: no second Ru, no TRN1u");
        assert_eq!(modem.phase(), "V.92: silent, for a retrain");
        assert!(!modem.circuit106() && !modem.circuit104());
    }

    /// The CPt says "that the analogue modem has received enough of the DIL
    /// sequence" (9.5.2.1.11), so an owner that chooses its constellations
    /// from the DIL has to be able to choose them after it: TRN1u runs on, in
    /// whole twelves, until it has, and the CPt and the CPu that then go out
    /// are the ones it chose.
    #[test]
    fn the_cpt_and_the_cpu_can_be_chosen_once_the_dil_has_arrived() {
        // What the owner will choose: a CPt at D = 20, and sixty-four points
        // an interval for data, which with the six signs is D = 42, drn 22.
        let training = Cp { drn: 12, ..cpt() };
        let points = (16..=79u8).fold(0u128, |mask, u| mask | 1 << u);
        let data = Cp { drn: 22, data_mode: true, constellations: vec![points], ..Cp::default() };
        // What the modem is built with is a CPt it will never send.
        let settings = Settings {
            descriptor: dil(3),
            choice_follows_dil: true,
            ..Settings::new(Descriptor::none(), JA_MASK_ALL, Cp { drn: 8, ..cpt() })
        };
        // The digital modem reads the CPt that is sent.
        let far = Far::new(&Settings { cpt: training.clone(), ..settings.clone() }, false);
        let mut call = Call::against(settings, far);
        // Reading a DIL takes a while: three thousand symbols of it here.
        call.choice = Some((training.clone(), data.clone(), 3000));
        assert!(!call.modem.dil_received());
        call.reach_phase4();
        assert!(call.trn1u > 3000, "TRN1u waited for the choice: {} symbols", call.trn1u);
        assert_eq!(call.trn1u % 12, 0, "in whole twelves (9.5.2.1.9)");
        let sent = call.cpts.first().expect("a CPt was read off the line");
        assert_eq!((sent.drn, &sent.constellations), (training.drn, &training.constellations));
        assert!(call.cpts.iter().all(|cpt| cpt == sent), "and every repetition of it was that one");
        assert!(!call.modem.choose(training, data.clone()), "a choice after CPt has begun is refused");

        let down = pattern(2000, 0xd11);
        call.exchange(&parameters(1, false), &down);
        call.connects_and_reads(&down);
        let cpu = call.far.cpus()[0].1.clone();
        assert_eq!((cpu.drn, &cpu.constellations), (22, &data.constellations), "the CPu is the data half of the choice");
        assert_eq!(call.modem.status(), Status::Connected { downstream: 56_000, upstream: 24_000 });
    }
}
