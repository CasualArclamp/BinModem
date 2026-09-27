//! A fax call on the line: the tones, V.21, the page carriers, and T.30 above
//! them.
//!
//! The join between the two halves. [`fax::call`] knows the procedure and
//! nothing about signals; [`datapump::v21`], [`datapump::v27ter`],
//! [`datapump::v29`] and [`datapump::v17`] know the signals and nothing about
//! the procedure. This
//! puts one on top of the other and gives the result a sample at a time, which
//! is the only thing a line understands.
//!
//! The whole of the join is one question asked once a sample: what should be
//! on the line just now. A fax call answers it with a different thing eight or
//! ten times before a page has moved -- a tone, then 300 bit/s, then silence,
//! then 9600, then silence, then 300 again -- and every one of those changes
//! is a carrier going up or down at both ends.
//!
//! A Super G3 call answers it differently. Once V.8 has agreed V.34
//! half-duplex (T.30 clause 6) the line belongs to one modem for the rest of
//! the call, [`datapump::v34::halfduplex`], which has two channels of its own
//! and turns between them itself; the procedure, T.30 Annex F, says only
//! which channel it wants ([`Line`]'s `V34*` words) and hands over bits. What
//! this join does there is map the one onto the other (`plan.md` 10.1), and
//! keep to the rule that the modem is never given a bit it will not send.

use datapump::v34::halfduplex::{self, Event, State};
use datapump::v34::phase2::{Role as V34Role, Status as Phase2Status};
use datapump::v34::{control, data, signals::Size};
use datapump::v8 as v8line;
use datapump::{v17, v21, v27ter, v29};
use fax::call::{Call, Line, Phase, Role, Speed};
use fax::coding::Coding;
use fax::page::{Page, Resolution};
use fax::t30::Modulation;

/// Which page carrier a speed calls for, and at which of its rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    V27ter(v27ter::Rate),
    V29(v29::Rate),
    V17(v17::Rate),
}

impl Carrier {
    /// `None` for anything this end has no pump for, which only ever happens
    /// when the far end's DCS names one: the ladder this end climbs is built
    /// from what it has.
    fn of(speed: Speed) -> Option<Self> {
        Some(match (speed.modulation, speed.bits_per_second) {
            (Modulation::V27ter, 4800) => Self::V27ter(v27ter::Rate::R4800),
            (Modulation::V27ter, 2400) => Self::V27ter(v27ter::Rate::R2400),
            (Modulation::V29, 9600) => Self::V29(v29::Rate::R9600),
            (Modulation::V29, 7200) => Self::V29(v29::Rate::R7200),
            (Modulation::V29, 4800) => Self::V29(v29::Rate::R4800),
            (Modulation::V17, rate) => Self::V17(v17::Rate::of(rate)?),
            _ => return None,
        })
    }
}

/// Every modulation this end has a pump for, and so may offer: V.27 ter, V.29
/// and V.17.
///
/// Not what a call offers unless it is asked to. [`fax::call::OUR_MODULATIONS`]
/// stays the default offer, which is what the window's boxes start from and
/// what everything before V.17 was proved against; offering this puts V.17
/// in the DIS, and two ends that both do settle on V.17 at 14 400.
pub const MODULATIONS: [Modulation; 3] = [Modulation::V27ter, Modulation::V29, Modulation::V17];

/// The same modulations as V.8 names them (Table 4/V.8), for a joint menu.
///
/// V.29 is the half-duplex bit: a fax's V.29 is the half-duplex use of it,
/// which is why V.8 gave that its own bit.
fn v8_modulations(modulations: &[Modulation]) -> v8::Modulations {
    let mut set = v8::Modulations::NONE;
    for m in modulations {
        set.insert(match m {
            Modulation::V27ter => v8::Modulation::V27ter,
            Modulation::V29 => v8::Modulation::V29HalfDuplex,
            Modulation::V17 => v8::Modulation::V17,
        });
    }
    set
}

/// A name for V.34's data-mode constellation by its points, as the V.17
/// names go: the L of Table 10/V.34 for the rate and symbol rate in use,
/// which is every value Table 10's rule gives across Table 8's rows, minimum
/// and expanded shaping both.
fn tcm_name(points: usize) -> &'static str {
    match points {
        4 => "4TCM",
        8 => "8TCM",
        12 => "12TCM",
        16 => "16TCM",
        20 => "20TCM",
        24 => "24TCM",
        28 => "28TCM",
        32 => "32TCM",
        36 => "36TCM",
        40 => "40TCM",
        44 => "44TCM",
        48 => "48TCM",
        52 => "52TCM",
        56 => "56TCM",
        68 => "68TCM",
        72 => "72TCM",
        88 => "88TCM",
        96 => "96TCM",
        104 => "104TCM",
        112 => "112TCM",
        120 => "120TCM",
        128 => "128TCM",
        144 => "144TCM",
        160 => "160TCM",
        176 => "176TCM",
        192 => "192TCM",
        208 => "208TCM",
        224 => "224TCM",
        256 => "256TCM",
        272 => "272TCM",
        320 => "320TCM",
        352 => "352TCM",
        384 => "384TCM",
        416 => "416TCM",
        448 => "448TCM",
        512 => "512TCM",
        544 => "544TCM",
        576 => "576TCM",
        640 => "640TCM",
        704 => "704TCM",
        768 => "768TCM",
        832 => "832TCM",
        896 => "896TCM",
        960 => "960TCM",
        1024 => "1024TCM",
        1152 => "1152TCM",
        1280 => "1280TCM",
        1408 => "1408TCM",
        1536 => "1536TCM",
        1664 => "1664TCM",
        _ => "TCM",
    }
}

/// How long the answering end's ANSam lasts with no call menu heard, when
/// V.34 is offered.
///
/// T.30 6.1.1: "an answering V.34 capable facsimile terminal shall transmit
/// ANSam until a valid CM response is received or until an ANSam time-out
/// (2.6 to 4.0 s) has expired" -- the window its plain called tone has
/// (4.1.1), in place of V.8's 5 +/- 1 s ([`v8line::timing::ANSAM`]), which
/// the data modems keep. The long end of it, because everything the caller
/// has to do fits inside: hear the tone and believe it, Te, two call menus --
/// and a call over a packet network puts a second and a half of round trip in
/// front of the first menu.
const ANSAM_SECONDS: f64 = 3.8;

/// How many control channel bits the join hands V.34's half-duplex modem
/// ahead of the line.
///
/// Sixteen is 13 ms at 1200 bit/s. Enough that a symbol never finds fewer
/// bits than it carries -- four at most, and the queue is filled again every
/// sample -- since a symbol short of bits is padded with ones, which inside a
/// frame is a frame spoilt (`control::Transmitter::send_bits`); and few
/// enough that what T.30 counts as sent is on the line within a flag or two,
/// which matters for the forty ones it counts before a page (F.3.2.3), and
/// that a burst cut into by a restart loses almost nothing.
const V34_CONTROL_AHEAD: usize = 16;

/// How many page bits are kept ahead of the primary channel's encoder.
///
/// The encoder takes bits a mapping frame at a time, up to 79 of them at
/// 33 600 bit/s, and pads a short frame with ones (`primary::Symbols`); this
/// is several frames at every rate, and 15 ms at the fastest, so the page
/// never runs dry inside a frame while T.30 still has bits to give. When
/// T.30 has none left the queue drains to nothing, which is the end of the
/// burst.
const V34_PAGE_AHEAD: usize = 512;

/// Where the control channel's transmitter puts a bit on the line, after
/// taking it: its pulse's lookahead and a symbol either side, in symbols at
/// 600 baud.
///
/// What [`Call::tick`]'s `idle` has to wait for at the end of an Annex F
/// call: the DCN's closing flag has been taken by the modem, and has then
/// to have left the pulse before the line is dropped under it (F.3.4.5 Note
/// 2 lets the line go straight after the DCN, not before it).
const V34_FLUSH_SYMBOLS: u64 = 10;

/// How long the source holds its turn to the page for the modem to hear the
/// recipient fall silent, once T.30 has taken the recipient's flags stopping
/// for its turn, before turning on that alone.
///
/// F.3.2.3 lets the source turn on "silence (or absence of flags)", and T.30
/// takes the second once [`fax::call::FLAGS_GONE`], a tenth of a second, has
/// passed without one. But the flags stop too for the 70 ms of silence a far
/// end keeps before its 12.7/V.34 retrain tone. T.30 then asks for the page
/// 130 to 165 ms after the far end stopped, by the control receiver's delay
/// and the noise, and the modem goes into the retrain -- after which it
/// refuses the turn -- once it has heard 50 ms of the tone, about 155 ms
/// after: a race either may win, and a page sent into a recipient in phase 2
/// if T.30 does. So the modem's own hearing of the silence,
/// [`halfduplex::Modem::far_silent`], is what turns the source (`wp-g.md`,
/// "For H3"), and this is the fallback for a far end whose silence it cannot
/// hear: a fifth of a second more, by when a retrain has shown itself.
const V34_TURN_HOLD: f64 = 0.2;

/// What carries a call once V.8 has agreed V.34 half-duplex and T.30 Annex F
/// has it: the modem of clause 12, from its phase 2 on.
///
/// One modem from the hand-over point to the last page. It runs phase 2
/// (12.2/V.34) itself from the 75 ms of silence after V.8 -- INFO0 on its
/// first sample -- and then phase 3 and the control channel start-up; and it
/// goes back through phase 2 for a primary channel retrain (12.7), so none of
/// phase 2 is the join's to run (`wp-g.md`, "For H3"). Both roles are fixed
/// by T.30: the end that dialled is the call modem and sends the page, so it
/// is phase 2's source; the end that answered is the answer modem and the
/// recipient. T.30 has no other case on an ordinary call, and polling is not
/// built.
#[derive(Debug)]
enum HalfDuplex {
    /// Phase 2 to the last page (12.2 to 12.8/V.34): the modem T.30's bits
    /// go through. Nothing of T.30's is taken until the control channel is
    /// first up; its clocks run from the hand-over (F.3.2.3 Note 1).
    Modem(Box<halfduplex::Modem>),
    /// The call is over and the line dropped, or the modem gave up: nothing
    /// is on the line, and what was learnt is in [`FaxCall::v34_facts`].
    Over,
}

/// What the half-duplex modem has settled, copied out once a sample so that
/// the window and the far-end panel can have it after the modem has gone.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct V34Facts {
    /// The primary channel's symbol rate, from INFOh.
    symbol_rate: Option<u32>,
    high_carrier: bool,
    /// The primary channel's data rate, as the last MPh exchange settled it.
    primary_rate: Option<u32>,
    /// The control channel's rates, this end's transmitter's and receiver's.
    control_rates: Option<(u32, u32)>,
    /// What the primary receiver last trained to, at the recipient.
    trained_snr_db: Option<f64>,
    retrains: u32,
    recoveries: u32,
}

/// Which of V.34's two channels the scope should be drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum V34Channel {
    /// The control channel, on four points at 1200 bit/s or sixteen at 2400
    /// (10.2.4/V.34).
    Control(Size),
    /// The primary channel in its data mode, with the parameters that fix
    /// its constellation.
    Primary(data::Params),
}

/// A fax call, from either end.
#[derive(Debug)]
pub struct FaxCall {
    call: Call,
    control_tx: v21::Sender,
    control_rx: v21::Receiver,
    v27ter_tx: v27ter::Transmitter,
    v27ter_rx: v27ter::Receiver,
    v29_tx: v29::Transmitter,
    v29_rx: v29::Receiver,
    v17_tx: v17::Transmitter,
    v17_rx: v17::Receiver,
    /// The speed of the last V.17 long train this end sent, which decides
    /// whether the next burst may have the short one.
    v17_long: Option<Speed>,
    cng: v21::Tone,
    ced: v21::Tone,
    /// What the line was doing on the last sample, so a change can be seen.
    line: Line,
    /// V.8, as much of it as this call runs.
    ///
    /// With V.34 offered, all of it, at either end (T.30 clause 6). Without,
    /// the answering end's ear for a V.8 call menu and its voice for the
    /// joint menu that answers one -- see [`v8line::Modem::overhearing`] for
    /// why a fax that sends the plain answer tone listens for a CM at all --
    /// and nothing at the calling end. Kept only until the far end has shown
    /// it is doing T.30 after all.
    v8: Option<v8line::Modem>,
    /// Whether V.34 half-duplex is offered ([`with_v34`](Self::with_v34)).
    v34_offered: bool,
    /// This end's fax modulations as a V.8 menu names them: what a joint
    /// menu is the intersection with, and what a call menu offers.
    v8_offer: v8::Modulations,
    /// Where the call stands with V.34's half-duplex modem: `Some` from the
    /// moment V.8 agreed V.34 half-duplex and the call went to Annex F.
    v34: Option<HalfDuplex>,
    /// What that modem settled, kept after it.
    v34_facts: V34Facts,
    /// Why the modem gave up, when it did: the call is failed from then.
    v34_trouble: Option<String>,
    /// The most the primary channel is to be offered, once anyone has asked
    /// ([`limit_v34_rate`](Self::limit_v34_rate)).
    v34_cap: Option<u32>,
    /// Whether the modem has accepted this end's turn to the primary channel
    /// for the page in hand, so that a turn it refused, or one a retrain
    /// undid, is asked for again.
    v34_turned: bool,
    /// Samples the source has held a turn T.30 asked for, waiting for the
    /// modem to hear the recipient silent ([`V34_TURN_HOLD`]).
    v34_held: u64,
    /// Samples for which the control channel's transmitter has had no bit of
    /// T.30's waiting, for [`Call::tick`]'s `idle`.
    v34_drained: u64,
    /// The menu the far end sent, if it sent one.
    far_menu: Option<v8::Menu>,
    /// The joint menu V.8 settled, once it has.
    joint_menu: Option<v8::Menu>,
    fs: f64,
}

impl FaxCall {
    /// The end that dialled.
    pub fn originate(fs: f64, identification: &str, page: Option<Page>) -> Self {
        Self::with(Call::originate(fs, identification, page), fs)
    }

    /// The end that dialled, with several pages to send, in order.
    pub fn originate_pages(fs: f64, identification: &str, pages: Vec<Page>) -> Self {
        Self::with(Call::originate_pages(fs, identification, pages), fs)
    }

    /// The end that answered.
    pub fn answer(fs: f64, identification: &str) -> Self {
        Self::with(Call::answer(fs, identification), fs)
    }

    fn with(mut call: Call, fs: f64) -> Self {
        call.set_available(&MODULATIONS);
        let mut this = Self {
            call,
            control_tx: v21::Sender::new(fs),
            control_rx: v21::Receiver::new(fs),
            v27ter_tx: v27ter::Transmitter::new(fs),
            v27ter_rx: v27ter::Receiver::new(fs),
            v29_tx: v29::Transmitter::new(fs),
            v29_rx: v29::Receiver::new(fs),
            v17_tx: v17::Transmitter::new(fs),
            v17_rx: v17::Receiver::new(fs),
            v17_long: None,
            cng: v21::Tone::new(v21::CNG, fs),
            ced: v21::Tone::new(v21::CED, fs),
            line: Line::Quiet,
            v8: None,
            v34_offered: false,
            v8_offer: v8_modulations(&fax::call::OUR_MODULATIONS),
            v34: None,
            v34_facts: V34Facts::default(),
            v34_trouble: None,
            v34_cap: None,
            v34_turned: false,
            v34_held: 0,
            v34_drained: 0,
            far_menu: None,
            joint_menu: None,
            fs,
        };
        this.start_v8();
        this
    }

    /// Use only these modulations: what goes in this end's DIS, and what it
    /// will choose from when it sends.
    ///
    /// And what goes in a joint menu, should a call menu arrive, or in this
    /// end's own call menu: the two have to agree, or V.8 would settle on a
    /// modulation the DIS then withholds.
    #[must_use]
    pub fn offering(mut self, modulations: &[Modulation]) -> Self {
        self.call.set_offer(modulations);
        self.v8_offer = v8_modulations(modulations);
        self.start_v8();
        self
    }

    /// Offer V.34 half-duplex, or not: T.30 clause 6 in place of clause 5's
    /// tones, at either end.
    ///
    /// Answering, that is ANSam in place of the called tone (6.1.1), and a
    /// joint menu with V.34 half-duplex in it for a caller whose call menu
    /// has it; dialling, a call menu on hearing ANSam (6.1.2). Either way the
    /// call goes to T.30 Annex F once V.8 has agreed it (6.1.5,
    /// [`Call::start_annex_f`]), carried by V.34's half-duplex modem from
    /// its phase 2 to the last page, and to clause 5 as before when it has
    /// not (6.1.6).
    ///
    /// Off unless asked, here; the modem above this join asks
    /// (`Modem::fax_v34`, the window's V.34 box). Off, the call is what it was
    /// before: the called tone and a DIS, with a call menu overheard and
    /// answered without V.34.
    #[must_use]
    pub fn with_v34(mut self, on: bool) -> Self {
        self.v34_offered = on;
        self.start_v8();
        self
    }

    /// The V.8 this call begins with, if any, from what it offers.
    ///
    /// The end that answers receives, so "transmit facsimile from call
    /// terminal" (Table 4/T.30) is the only call function it takes up, and
    /// the one the end that dials asks for.
    fn start_v8(&mut self) {
        let function = v8::CallFunction::TransmitFax;
        self.v8 = match (self.call.role(), self.v34_offered) {
            (Role::Answerer, false) => {
                Some(v8line::Modem::overhearing(function, self.v8_offer, self.fs))
            }
            (Role::Answerer, true) => Some(
                v8line::Modem::new(v8line::Role::Answering, function, self.super_g3_offer(), self.fs)
                    .with_ansam_seconds(ANSAM_SECONDS)
                    .answering_only_its_function(),
            ),
            (Role::Caller, true) => Some(v8line::Modem::new(
                v8line::Role::Calling,
                function,
                self.super_g3_offer(),
                self.fs,
            )),
            (Role::Caller, false) => None,
        };
    }

    /// This end's modulations with V.34 half-duplex, as a menu offers them.
    ///
    /// Half-duplex alone, never V.34 duplex beside it: 7.4/V.8 settles a
    /// joint menu on the lowest item number of Table 4/V.8, duplex is item 1
    /// to half-duplex's 2, and a fax that offered both would find itself in
    /// Annex C (plan 8.5).
    fn super_g3_offer(&self) -> v8::Modulations {
        let mut offer = self.v8_offer;
        offer.insert(v8::Modulation::V34HalfDuplex);
        offer
    }

    /// Put V.27 ter's protection against talker echo in front of every burst:
    /// a fifth of a second of plain carrier, then twenty milliseconds of
    /// nothing, then the training. V.17's is the same thing (5.3/V.17), and
    /// goes on with it.
    #[must_use]
    pub fn with_echo_protection(mut self, on: bool) -> Self {
        self.v27ter_tx.set_echo_protection(on);
        self.v17_tx.set_echo_protection(on);
        self
    }

    /// Offer error correction mode, or not.
    #[must_use]
    pub fn with_error_correction(mut self, on: bool) -> Self {
        self.call.set_error_correction(on);
        self
    }

    /// Whether this call is in error correction mode.
    pub fn error_correction(&self) -> bool {
        self.call.error_correction()
    }

    /// Offer JBIG, or not. Only ever used under error correction mode.
    #[must_use]
    pub fn with_jbig(mut self, on: bool) -> Self {
        self.call.set_jbig(on);
        self
    }

    /// The coding the page goes in, once a DCS has settled it.
    pub fn coding(&self) -> Coding {
        self.call.coding()
    }

    pub fn role(&self) -> Role {
        self.call.role()
    }

    /// How far the call has got: T.30's phase, or failed once V.34's modem
    /// has given up under it.
    ///
    /// The modem giving up ends the call from here rather than from inside
    /// the procedure, which has no way of being told and would otherwise sit
    /// out a timer of its own -- T1 is 35 s -- before saying something less
    /// true than the modem's reason.
    pub fn phase(&self) -> Phase {
        if self.v34_trouble.is_some() {
            return Phase::Failed;
        }
        self.call.phase()
    }

    /// What the call is doing, as a person would say it: T.30's phase, or
    /// V.8 while that has the line, or V.34's modem while it is between
    /// channels and the procedure is waiting on it.
    pub fn phase_name(&self) -> &'static str {
        if let Some(v8) = &self.v8
            && v8.has_the_line()
        {
            return match v8.phase() {
                "quiet" => "V.8: the silence before ANSam",
                "ANSam" => "V.8: sending ANSam",
                "Te" => "V.8: ANSam heard, the silence before the call menu",
                "CM" => "V.8: sending the call menu",
                "CJ" => "V.8: ending the call menu",
                "JM" => "V.8: answering the call menu",
                "handover" => "V.8: handing over",
                _ => "V.8",
            };
        }
        match self.v34.as_ref() {
            // On a channel, the procedure's word for it: "sending the page"
            // says more than "V.34 page". Between them, the modem's, which in
            // phase 2 is phase 2's own ("V.34 INFO0", "V.34 tones" ...).
            Some(HalfDuplex::Modem(modem)) => match modem.state() {
                State::Control | State::Primary => self.call.phase().name(),
                _ => modem.phase(),
            },
            Some(HalfDuplex::Over) | None => self.phase().name(),
        }
    }

    /// The V.8 menu the far end sent, if it sent one. Answering, its call
    /// menu: what the calling fax can do, V.34 included, whatever this end
    /// could do about it. Dialling, its joint menu.
    pub fn far_menu(&self) -> Option<v8::Menu> {
        self.far_menu
    }

    /// The joint menu V.8 settled, once it has: what the two ends have in
    /// common, whichever end this is (7.4/V.8).
    pub fn joint_menu(&self) -> Option<v8::Menu> {
        self.joint_menu
    }

    /// Whether V.8 agreed V.34 half-duplex, so that the call is T.30 Annex
    /// F's (6.1.5) -- from the hand-over point on, 75 ms after CJ, with the
    /// modem's own start-up (INFO0, 11.1/V.34) begun on that sample.
    pub fn v34_agreed(&self) -> bool {
        self.v34.is_some()
    }

    /// The primary channel's symbol rate in baud, once V.34's INFOh has
    /// chosen it (12.2/V.34); kept after the call.
    pub fn symbol_rate(&self) -> Option<u32> {
        self.v34_facts.symbol_rate
    }

    /// The primary channel's data rate in bit/s, as V.34's last MPh exchange
    /// settled it (12.4/V.34); kept after the call. What [`rate`](Self::rate)
    /// says too, once the procedure has been told, but from the modem.
    pub fn primary_rate(&self) -> Option<u32> {
        self.v34_facts.primary_rate
    }

    /// The control channel's rates in bit/s, this end's transmitter's and
    /// its receiver's (12.4.1.4/V.34); kept after the call.
    pub fn control_rates(&self) -> Option<(u32, u32)> {
        self.v34_facts.control_rates
    }

    /// Control channel retrains (12.8/V.34) the call has had, from either
    /// end, and times phase 3 failed and was gone back to (12.3.3/V.34).
    pub fn v34_retrains(&self) -> (u32, u32) {
        (self.v34_facts.retrains, self.v34_facts.recoveries)
    }

    /// What the primary receiver last trained to, in decibels, at the end
    /// that receives the page.
    pub fn v34_snr_db(&self) -> Option<f64> {
        self.v34_facts.trained_snr_db
    }

    /// Offer the primary channel no more than this, in bit/s, from the next
    /// MPh exchange on -- and, when a page is already going faster, ask for
    /// one at the next start of the control channel (F.3.4.1/T.30, 12.6/V.34:
    /// the source through a start-up in place of its resynchronisation, the
    /// recipient by answering Sh with PPh, 12.6.2.3).
    ///
    /// The window's cap, should it grow one, and a way of forcing a rate
    /// change in a test. Rounded down to a multiple of 2400 by the modem, and
    /// never below it.
    pub fn limit_v34_rate(&mut self, bits_per_second: u32) {
        self.v34_cap = Some(bits_per_second);
        if let Some(HalfDuplex::Modem(modem)) = self.v34.as_mut() {
            modem.limit_rate(bits_per_second);
        }
    }

    /// What the V.34 modem settled, for the far-end panel, in rows: nothing
    /// until V.8 has agreed V.34.
    pub fn v34_rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        let facts = &self.v34_facts;
        match self.v34.as_ref() {
            None => return rows,
            // The first phase 2, before INFOh has settled anything to show.
            Some(HalfDuplex::Modem(modem)) if modem.infoh().is_none() => {
                rows.push(("V.34 fax", format!("phase 2: {}", modem.phase())));
            }
            Some(HalfDuplex::Modem(_) | HalfDuplex::Over) => {}
        }
        if let Some(baud) = facts.symbol_rate {
            let carrier = if facts.high_carrier { "high" } else { "low" };
            let rate = facts.primary_rate.map_or("no rate yet".to_owned(), |r| format!("{r} bit/s"));
            rows.push(("primary channel", format!("{rate} on {baud} baud, {carrier} carrier")));
        }
        if let Some((transmit, receive)) = facts.control_rates {
            let mut what = if transmit == receive {
                format!("{transmit} bit/s both ways")
            } else {
                format!("{transmit} bit/s out, {receive} in")
            };
            if facts.retrains > 0 {
                what.push_str(&format!(", {} retrain{}", facts.retrains, if facts.retrains == 1 { "" } else { "s" }));
            }
            rows.push(("control channel", what));
        }
        if let Some(snr) = facts.trained_snr_db {
            let mut what = format!("{snr:.1} dB");
            if facts.recoveries > 0 {
                what.push_str(&format!(", phase 3 gone back to {} time{}", facts.recoveries, if facts.recoveries == 1 { "" } else { "s" }));
            }
            rows.push(("page trained to", what));
        }
        rows
    }

    pub fn seconds(&self) -> f64 {
        self.call.seconds()
    }

    pub fn identity(&self) -> &str {
        &self.call.identity
    }

    /// The far end's NSF, as it arrived.
    pub fn non_standard(&self) -> Option<&[u8]> {
        self.call.non_standard.as_deref()
    }

    pub fn capabilities(&self) -> Option<&fax::t30::Capabilities> {
        self.call.capabilities.as_ref()
    }

    /// The capability field as it arrived.
    pub fn capability_field(&self) -> Option<&[u8]> {
        self.call.capability_field.as_deref()
    }

    /// The rate the page is being carried at.
    pub fn rate(&self) -> u32 {
        self.call.rate()
    }

    /// The modulation and rate the page is being carried at.
    pub fn speed(&self) -> Speed {
        self.call.speed()
    }

    /// How far through the page the call has got.
    pub fn progress(&self) -> Option<f64> {
        self.call.progress()
    }

    /// Lines of a page that have arrived.
    pub fn lines_received(&self) -> usize {
        self.call.lines_received()
    }

    /// The lines of the page arriving, as far as it has been decoded.
    pub fn lines(&self) -> &[Vec<bool>] {
        self.call.lines()
    }

    /// The resolution the page is arriving at, as the DCS said.
    pub fn resolution(&self) -> Resolution {
        self.call.resolution()
    }

    /// The oldest page that arrived and has not been taken, once one has.
    pub fn received(&self) -> Option<&Page> {
        self.call.received()
    }

    /// The oldest page that arrived, with its number in the call, handed over
    /// and forgotten.
    ///
    /// A page is a couple of megabytes of booleans, so it is moved rather
    /// than copied and moved exactly once. Whoever takes it owns it.
    pub fn take_received(&mut self) -> Option<(usize, Page)> {
        self.call.take_received()
    }

    /// Pages finished so far in this call.
    pub fn pages_received(&self) -> usize {
        self.call.pages_received()
    }

    /// Which page of the call is going or arriving, or last arrived,
    /// counting from one.
    pub fn sheet(&self) -> usize {
        self.call.sheet()
    }

    /// How many pages the call has, as far as this end knows.
    pub fn sheets(&self) -> usize {
        self.call.sheets()
    }

    /// Why the call went badly, if it did: the modem's reason where V.34's
    /// modem gave up, the procedure's otherwise.
    pub fn trouble(&self) -> Option<&str> {
        self.v34_trouble.as_deref().or(self.call.trouble.as_deref())
    }

    pub fn take_heard(&mut self) -> Vec<fax::frames::Message> {
        self.call.take_heard()
    }

    /// Whether anything of the far end's is on the line.
    pub fn carrier(&self) -> bool {
        if let Some(HalfDuplex::Modem(modem)) = self.v34.as_ref() {
            return modem.control_carrier() || modem.page_carrier();
        }
        self.control_rx.carrier()
            || self.v27ter_rx.carrier()
            || self.v29_rx.carrier()
            || self.v17_rx.carrier()
    }

    /// The page carrier the line is on just now, if it is on one.
    ///
    /// A fax call has three receivers and only ever one of them is the one
    /// that matters. Which, decides what a scope should be drawing: the page
    /// carriers have constellations and the control channel has an eye, and
    /// those are not the same picture at all.
    fn page_carrier(&self) -> Option<Carrier> {
        match self.line {
            Line::Fast(speed) | Line::FastListen(speed) => Carrier::of(speed),
            _ => None,
        }
    }

    /// Which of V.34's channels the scope should be drawing, once the call is
    /// on the half-duplex modem: the primary channel from the moment this end
    /// turns to it until the page is over, the control channel otherwise.
    fn v34_channel(&self) -> Option<V34Channel> {
        let Some(HalfDuplex::Modem(modem)) = self.v34.as_ref() else { return None };
        // Phase 2 -- the first, or a 12.7 retrain's -- is neither channel:
        // INFO sequences in binary DPSK, and tones, with nothing to decide.
        if modem.phase2().is_some_and(|phase2| phase2.status() == Phase2Status::Running) {
            return None;
        }
        Some(match modem.state() {
            State::Primary | State::ToPrimary => {
                let data = modem.data_mode()?;
                // Which scrambler the source has does not move a point of the
                // constellation; the source is the call modem here anyway.
                V34Channel::Primary(data.params(modem.channel()?.band.rate, datapump::v32::Mode::Call)?)
            }
            State::Failed => return None,
            _ => V34Channel::Control(modem.control().receiver.rate().size()),
        })
    }

    /// The point V.34's receivers last decided on, while one of them is
    /// deciding: the primary channel's at the recipient from B1 on, the
    /// control channel's from PPh or Sh until the far end stops. The source
    /// has nothing to draw while its page goes out -- the primary channel is
    /// one way, and its transmitter keeps no reading of what it sends.
    fn v34_point(&self) -> Option<(f64, f64)> {
        let Some(HalfDuplex::Modem(modem)) = self.v34.as_ref() else { return None };
        match self.v34_channel()? {
            V34Channel::Primary(_) => {
                let recipient = modem.primary_recipient()?;
                recipient.carrier().then(|| recipient.last_point()).flatten().map(Into::into)
            }
            V34Channel::Control(_) => {
                let receiver = &modem.control().receiver;
                receiver.is_locked().then(|| receiver.constellation_point())
            }
        }
    }

    /// Signal to noise of the V.34 receiver that is deciding, in decibels,
    /// while one is.
    fn v34_snr(&self) -> Option<f64> {
        let Some(HalfDuplex::Modem(modem)) = self.v34.as_ref() else { return None };
        match self.v34_channel()? {
            V34Channel::Primary(_) => {
                let recipient = modem.primary_recipient()?;
                recipient.carrier().then(|| recipient.snr_db())
            }
            V34Channel::Control(_) => {
                let receiver = &modem.control().receiver;
                receiver.is_locked().then(|| receiver.snr_db())
            }
        }
    }

    /// The gap between neighbouring points of the V.34 constellation being
    /// decided, at the unit mean power the points are reported in.
    ///
    /// Exact for the control channel's four and sixteen points. For the
    /// primary channel's shaped hundreds it is the square constellation's
    /// figure -- sqrt(6 / (L - 1)) for L points at odd multiples of half a
    /// gap -- which is a little under the truth, since shaping uses the outer
    /// rings less and so spends less power for the same gap; near enough for
    /// a meter whose decision boundary is half.
    fn v34_point_spacing(channel: V34Channel) -> f64 {
        match channel {
            V34Channel::Control(Size::Four) => 2.0 * std::f64::consts::FRAC_1_SQRT_2,
            V34Channel::Control(Size::Sixteen) => 2.0 / 10f64.sqrt(),
            V34Channel::Primary(params) => (6.0 / (params.framing.l.max(2) - 1) as f64).sqrt(),
        }
    }

    /// The constellation on the line just now, while a page carrier is.
    ///
    /// Whichever end of it this is. Sending, it is the points going out --
    /// there is nothing arriving on a half-duplex line to draw instead, and a
    /// scope that froze on the last point it received would show a training
    /// sequence as a single dot. Listening, it is the points the receiver
    /// decided on, but only while it hears a carrier: the silence either side
    /// of a burst comes out of an equaliser as a smear at the centre that is
    /// not a picture of anything.
    ///
    /// On V.34 it is whichever channel is up: see [`Self::v34_point`].
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        if self.v34.is_some() {
            return self.v34_point();
        }
        match self.line {
            Line::Fast(speed) => match Carrier::of(speed)? {
                Carrier::V27ter(_) => self.v27ter_tx.last_point(),
                Carrier::V29(_) => self.v29_tx.last_point(),
                Carrier::V17(_) => self.v17_tx.last_point(),
            },
            Line::FastListen(speed) => match Carrier::of(speed)? {
                Carrier::V27ter(_) => self
                    .v27ter_rx
                    .carrier()
                    .then(|| self.v27ter_rx.constellation_point()),
                Carrier::V29(_) => self
                    .v29_rx
                    .carrier()
                    .then(|| self.v29_rx.constellation_point()),
                Carrier::V17(_) => self
                    .v17_rx
                    .carrier()
                    .then(|| self.v17_rx.constellation_point()),
            },
            _ => None,
        }
    }

    /// How far out that constellation reaches.
    ///
    /// One for V.27 ter, whose points are on the unit circle. V.29's outer
    /// ring on the axes is a third beyond it, and a scope drawn to the unit
    /// circle would put four of its sixteen points off the edge. V.17's
    /// crosses reach further still.
    pub fn constellation_peak(&self) -> f64 {
        if self.v34.is_some() {
            // V.34's points are reported at unit mean power, and its peaks
            // are the largest coordinate at that power, as the duplex pump
            // has them: 1/sqrt(2) for four points, 3/sqrt(10) for sixteen,
            // and data mode's own, about one and a half for the shaped
            // hundreds. `data::peak` is not that -- it is in grid units,
            // tens of them at 33 600 -- and a scope sized by it drew every
            // point of the first real Super G3 page as one dot at the centre.
            return match self.v34_channel() {
                Some(V34Channel::Control(Size::Four)) | None => std::f64::consts::FRAC_1_SQRT_2,
                Some(V34Channel::Control(Size::Sixteen)) => 3.0 / 10f64.sqrt(),
                Some(V34Channel::Primary(params)) => data::unit_peak(&params),
            };
        }
        match self.page_carrier() {
            Some(Carrier::V29(_)) => self.v29_rx.constellation_peak(),
            Some(Carrier::V17(rate)) => rate.peak(),
            _ => 1.0,
        }
    }

    /// The control channel's discriminator, while that is the one in use.
    pub fn discriminator(&self) -> Option<f64> {
        self.on_the_control_channel()
            .then(|| self.control_rx.discriminator())
    }

    /// One reading per recovered bit of the control channel.
    pub fn take_symbol(&mut self) -> Option<f64> {
        if !self.on_the_control_channel() {
            return None;
        }
        self.control_rx.take_symbol()
    }

    /// Whether V.21's channel is the one the scope should be drawing: not
    /// while a page carrier is, and never once the call is V.34's, whose
    /// control channel is QAM and has a constellation rather than an eye.
    fn on_the_control_channel(&self) -> bool {
        self.v34.is_none() && !matches!(self.line, Line::Fast(_) | Line::FastListen(_))
    }

    /// Mean distance from the decisions being made, where there are points to
    /// decide between: at unit mean power, so that a signal to noise in
    /// decibels is minus twenty times its logarithm, which is how V.34's
    /// receivers report theirs.
    pub fn residual_error(&self) -> Option<f64> {
        if self.v34.is_some() {
            return self.v34_snr().map(|snr_db| 10f64.powf(-snr_db / 20.0));
        }
        Some(match self.page_carrier()? {
            Carrier::V27ter(_) => self.v27ter_rx.residual_error(),
            Carrier::V29(_) => self.v29_rx.residual_error(),
            Carrier::V17(_) => self.v17_rx.residual_error(),
        })
    }

    /// That distance as a fraction of the gap between neighbouring points,
    /// where half is the decision boundary.
    pub fn reception(&self) -> Option<f64> {
        if self.v34.is_some() {
            let channel = self.v34_channel()?;
            return self.residual_error().map(|error| error / Self::v34_point_spacing(channel));
        }
        Some(match self.page_carrier()? {
            Carrier::V27ter(_) => {
                self.v27ter_rx.residual_error() / self.v27ter_rx.point_spacing()
            }
            Carrier::V29(_) => self.v29_rx.residual_error() / self.v29_rx.point_spacing(),
            Carrier::V17(_) => self.v17_rx.residual_error() / self.v17_rx.point_spacing(),
        })
    }

    /// How many points the scope should expect.
    pub fn states(&self) -> usize {
        if self.v34.is_some() {
            return match self.v34_channel() {
                Some(V34Channel::Control(size)) => 1 << size.bits(),
                Some(V34Channel::Primary(params)) => params.framing.l,
                // Phase 2's INFO sequences: binary DPSK on a tone.
                None => 2,
            };
        }
        match self.page_carrier() {
            None => 2,
            Some(Carrier::V27ter(rate)) => usize::from(rate.phases()),
            Some(Carrier::V29(rate)) => rate.constellation().len(),
            Some(Carrier::V17(rate)) => rate.points(),
        }
    }

    /// Short name for the signal shape, as a faceplate would print it.
    ///
    /// V.29 is amplitude and phase rather than a square grid, so its names
    /// say so: two radii on each of the eight phases is not what "16QAM" makes
    /// anybody picture. V.34's data mode is named as V.17's is, by the points
    /// on the line -- "1408TCM" at 33 600 on 3429 baud -- and its control
    /// channel by its four or sixteen.
    pub fn shape(&self) -> &'static str {
        if self.v34.is_some() {
            return match self.v34_channel() {
                Some(V34Channel::Control(Size::Four)) => "4PSK",
                Some(V34Channel::Control(Size::Sixteen)) => "16QAM",
                Some(V34Channel::Primary(params)) => tcm_name(params.framing.l),
                None => "DPSK",
            };
        }
        match self.page_carrier() {
            None => "2FSK",
            Some(Carrier::V27ter(v27ter::Rate::R4800)) => "8PSK",
            Some(Carrier::V27ter(v27ter::Rate::R2400)) => "4PSK",
            Some(Carrier::V29(v29::Rate::R9600)) => "16APM",
            Some(Carrier::V29(v29::Rate::R7200)) => "8APM",
            Some(Carrier::V29(v29::Rate::R4800)) => "4PSK",
            // Trellis-coded QAM, named by the points on the line rather than
            // by the bits: 128 carry six, one of them redundant.
            Some(Carrier::V17(v17::Rate::R14400)) => "128TCM",
            Some(Carrier::V17(v17::Rate::R12000)) => "64TCM",
            Some(Carrier::V17(v17::Rate::R9600)) => "32TCM",
            Some(Carrier::V17(v17::Rate::R7200)) => "16TCM",
        }
    }

    /// The modulation carrying the line just now.
    pub fn standard(&self) -> &'static str {
        if self.v34.is_some() {
            return "V.34";
        }
        match self.page_carrier() {
            None => "V.21",
            Some(Carrier::V27ter(_)) => "V.27ter",
            Some(Carrier::V29(_)) => "V.29",
            Some(Carrier::V17(_)) => "V.17",
        }
    }

    /// One sample in, one sample out.
    pub fn step(&mut self, input: f64) -> f64 {
        if self.v34_trouble.is_some() {
            // The modem gave up and took the line with it: nothing more is
            // sent or heard, and the call is failed (see `phase`).
            return 0.0;
        }
        if let Some(out) = self.negotiate(input) {
            return out;
        }
        if self.v34.is_some() {
            return self.annex_f_step(input);
        }
        let want = self.call.line();
        self.follow(want);
        self.listen(want, input);
        let (out, idle) = self.talk(want);
        self.call.tick(idle);
        out
    }

    /// One sample of an Annex F call: the half-duplex modem, from its phase 2
    /// on, with T.30 mapped onto it (`plan.md` 10.1).
    fn annex_f_step(&mut self, input: f64) -> f64 {
        // Over, the modem goes with the line: it stops, and nothing is left
        // dying away, which F.3.4.5 Note 2 allows once the DCN has gone.
        if self.call.line() == Line::Quiet && !matches!(self.v34, Some(HalfDuplex::Over)) {
            self.v34 = Some(HalfDuplex::Over);
            self.line = Line::Quiet;
        }
        let mut failed = None;
        let out = match self.v34.as_mut() {
            None | Some(HalfDuplex::Over) => {
                self.call.tick(true);
                0.0
            }
            Some(HalfDuplex::Modem(modem)) => {
                let out = modem.step(input);
                let source = modem.is_source();
                while let Some(event) = modem.event() {
                    match event {
                        // Before any bit is taken: what came either side of
                        // the restart is no frame, and a burst it cut into
                        // goes again whole (`wp-h2.md`). The rate is the last
                        // MPh exchange's, which the restart may have changed.
                        Event::ControlUp => {
                            self.call.control_restarted();
                            if let Some(rate) = modem.primary_rate() {
                                self.call.set_primary_rate(rate);
                            }
                            self.v34_held = 0;
                        }
                        // A retrain, of either channel, from either end, puts
                        // both back on the control channel: a turn made before
                        // it is undone, and is made again once the channel is
                        // back if T.30 still wants the page. Nothing else
                        // undoes one -- the channel coming back after a page
                        // is the page's turn over, not one to make again for a
                        // recipient whose T.30 is a moment behind its modem.
                        Event::Retraining => {
                            self.v34_turned = false;
                            self.v34_held = 0;
                        }
                        // Phase 2 failing, or anything after it: the modem's
                        // reason is the call's.
                        Event::Failed(why) => failed = Some(format!("V.34: {why}")),
                        Event::Phase2Over | Event::Phase3Over { .. } | Event::PageStarted { .. } | Event::PageEnded => {}
                    }
                }
                // What INFOh chose, once phase 2 is over: the first time, or
                // again after a primary channel retrain (12.7) or a phase 3
                // gone back to (12.3.3), either of which may choose afresh.
                if let Some(infoh) = modem.infoh() {
                    self.v34_facts.symbol_rate = Some(infoh.symbol_rate.nominal());
                    self.v34_facts.high_carrier = infoh.high_carrier;
                }
                self.v34_facts.primary_rate = modem.primary_rate();
                self.v34_facts.control_rates = modem.control_rates();
                self.v34_facts.trained_snr_db = modem.primary_snr_db();
                self.v34_facts.retrains = modem.retrains();
                self.v34_facts.recoveries = modem.recoveries();

                // What the far end has to say, and what the modem knows of
                // it: the control channel's bits whenever the channel is up
                // (the modem hands up none off it), the page's at the
                // recipient, and the far end's silence for the source's turn
                // (F.3.2.3).
                self.call.set_far_silent(modem.far_silent());
                for bit in modem.take_control_bits() {
                    self.call.control_bit(bit);
                }
                if !source {
                    let bits = modem.take_page_bits();
                    if !bits.is_empty() {
                        self.call.fast_bits(&bits);
                    }
                    self.call.set_fast_carrier(modem.page_carrier());
                }

                // The turnarounds, which are the modem's to make and the
                // procedure's to ask for (`plan.md` 10.1): V34Ones to
                // V34Primary at the source and V34Listen to V34PrimaryListen
                // at the recipient are circuit 105 dropping, 12.6.3/V.34; the
                // page's end at the source is 105 dropping again, 12.5.3.1,
                // and then 12.6.1 -- or 12.4.1.1 when a new rate is wanted.
                let want = self.call.line();
                let change_wanted = self.v34_cap.is_some_and(|cap| modem.primary_rate().is_some_and(|rate| cap < rate));
                if want != self.line {
                    match (self.line, want) {
                        // A turn asked for, made below once the modem can.
                        (_, Line::V34Primary | Line::V34PrimaryListen) => {
                            self.v34_turned = false;
                            self.v34_held = 0;
                        }
                        (Line::V34Primary, _) if source => {
                            modem.to_control(self.call.renegotiate() || change_wanted);
                        }
                        // The recipient's page ends with the far carrier; only
                        // a wish for a new rate is the modem's to hear of
                        // (12.6.2.3/V.34).
                        (Line::V34PrimaryListen, _) if change_wanted => {
                            modem.to_control(true);
                        }
                        _ => {}
                    }
                    self.line = want;
                }
                // The turn itself, from the control channel only: one asked
                // for while the modem is between channels -- a retrain -- is
                // held until the channel is back, never dropped. The recipient
                // turns at once, T.30 having heard the forty ones (F.3.2.2);
                // the source once the modem hears the recipient silent
                // (F.3.2.3), or T.30's "absence of flags" has stood a while
                // longer ([`V34_TURN_HOLD`]). Until then the source's control
                // channel, fed nothing, goes on sending ones.
                if !self.v34_turned
                    && matches!(want, Line::V34Primary | Line::V34PrimaryListen)
                    && modem.state() == State::Control
                {
                    let ready = !source || modem.far_silent() || self.v34_held as f64 >= V34_TURN_HOLD * self.fs;
                    if ready {
                        self.v34_turned = modem.to_primary();
                    } else {
                        self.v34_held += 1;
                    }
                }

                // A recipient with something to say on a channel that is
                // down: its page never came and T.30 has given up on it, with
                // a DCN queued for a channel the modem is still listening for
                // the page over. 12.8.1's retrain brings the channel back for
                // it, or fails the call trying (`wp-h2.md`, "For G and H3").
                if !source && want == Line::V34Control && modem.state() == State::Primary && !modem.page_carrier() {
                    modem.retrain_control();
                }

                // What T.30 has to say, taken only as fast as the modem will
                // send it: control bits -- frames, flags, the source's ones --
                // while the channel is up, page bits while the page is going.
                if matches!(want, Line::V34Control | Line::V34Listen | Line::V34Ones) && modem.state() == State::Control {
                    while modem.pending_control_bits() < V34_CONTROL_AHEAD {
                        match self.call.next_control_bit() {
                            Some(bit) => {
                                modem.send_control_bits(&[bit]);
                            }
                            None => break,
                        }
                    }
                }
                if source && want == Line::V34Primary && modem.state() == State::Primary {
                    let mut bits = Vec::new();
                    while modem.pending_page_bits() + bits.len() < V34_PAGE_AHEAD {
                        match self.call.next_fast_bit() {
                            Some(bit) => bits.push(bit),
                            None => break,
                        }
                    }
                    if !bits.is_empty() {
                        modem.send_page_bits(&bits);
                    }
                }

                // Idle only once nothing of T.30's is left with the modem:
                // the page's bits all in the encoder, or the control
                // channel's all through the pulse and on the line. The
                // procedure ends a page on the first and the call on the
                // second.
                if modem.pending_control_bits() == 0 {
                    self.v34_drained += 1;
                } else {
                    self.v34_drained = 0;
                }
                let flushed = self.v34_drained as f64 >= V34_FLUSH_SYMBOLS as f64 * self.fs / control::BAUD;
                let idle = if want == Line::V34Primary {
                    modem.pending_page_bits() == 0
                } else {
                    modem.pending_control_bits() == 0 && flushed
                };
                self.call.tick(idle);
                out
            }
        };
        if let Some(why) = failed {
            self.v34_trouble = Some(why);
            self.v34 = Some(HalfDuplex::Over);
            return 0.0;
        }
        out
    }

    /// Run V.8 beside T.30, and in front of it while V.8 has the line.
    /// `Some`, with the sample to send, while it has.
    ///
    /// While it has, T.30 stands still: its clocks do not run and nothing of
    /// it reaches the line, because the far end is not listening to T.30 --
    /// it is waiting for a JM, and a DIS sent past it goes unheard, which is
    /// how the recorded call spent its whole length. Or it is sending ANSam
    /// and waiting for a CM, and the calling tone it would hear instead is
    /// what 8.1.1/V.8 has stopped.
    ///
    /// While it has not, V.8 only listens -- an answering end for a call
    /// menu behind its called tone, or for a CI after its DIS (6.1.4/T.30); a
    /// calling end for ANSam -- and T.30 has the line as though V.8 were not
    /// there.
    fn negotiate(&mut self, input: f64) -> Option<f64> {
        let v8 = self.v8.as_mut()?;
        let role = self.call.role();
        let early = match role {
            Role::Answerer => matches!(
                self.call.phase(),
                Phase::Answering | Phase::Identifying | Phase::AwaitingCommand
            ),
            Role::Caller => matches!(self.call.phase(), Phase::Calling | Phase::Listening),
        };
        if !early && !v8.has_the_line() {
            // A command has arrived, or a DIS: the far end is doing T.30 and
            // there is nobody left to send a menu.
            self.v8 = None;
            return None;
        }
        let was = v8.status();
        let out = v8.step(input);
        if self.far_menu.is_none() {
            self.far_menu = v8.far_menu();
        }
        match (was, v8.status()) {
            (v8line::Status::Negotiating, v8line::Status::Negotiating) if v8.has_the_line() => {
                self.drop_the_line();
                Some(out)
            }
            (v8line::Status::Negotiating, v8line::Status::Negotiating) => None,
            // The calling end heard the plain answering tone: the far end
            // does not do V.8, and T.30, which never stopped, goes on exactly
            // as it was (8.1.1/V.8) -- this sample included, since V.8 was
            // only listening and has nothing to put on the line in its place.
            // A call to a fax without V.8 is the call it was before V.8 was
            // offered, to the sample.
            (v8line::Status::Negotiating, v8line::Status::NoNegotiation)
                if role == Role::Caller =>
            {
                self.v8 = None;
                None
            }
            (v8line::Status::Negotiating, settled) => {
                self.after_v8(settled);
                Some(out)
            }
            // ANSam ran out, the DIS went with bit 6, and the modem stayed as
            // an ear: 6.1.4/T.30, "when an answer terminal, expecting a
            // response to a DIS frame, detects a CI signal, it shall enter
            // the V.8 mode by resending the answer tone ANSam" (Figures F.5-8
            // and F.5-9). Whatever of the DIS is still going out is cut: the
            // caller has stopped reading it.
            (_, v8line::Status::NoNegotiation) if v8.heard_ci() => {
                v8.ansam_again();
                self.drop_the_line();
                Some(out)
            }
            _ => None,
        }
    }

    /// Stop whatever T.30 had on the line, which V.8 has just taken over: the
    /// called tone, or a DIS part way out.
    fn drop_the_line(&mut self) {
        if self.line != Line::Quiet {
            // A new transmitter rather than a silenced one, because the old
            // one still holds the rest of the frame it was sending, and would
            // send it first the next time it was asked for anything.
            self.control_tx = v21::Sender::new(self.fs);
            self.line = Line::Quiet;
        }
    }

    /// V.8 has finished with the line, one way or another -- except the
    /// calling end's plain tone, which [`negotiate`](Self::negotiate) settles
    /// without coming here, since nothing of the line is V.8's to give back.
    fn after_v8(&mut self, status: v8line::Status) {
        let role = self.call.role();
        self.joint_menu = self.v8.as_ref().and_then(v8line::Modem::joint_menu);
        self.control_tx = v21::Sender::new(self.fs);
        self.control_rx = v21::Receiver::new(self.fs);
        self.line = Line::Quiet;
        match status {
            // 6.1.5: V.34 at both ends, and half-duplex, so Annex F. This is
            // the hand-over point: the 75 ms of silence after CJ (8.1.2,
            // 8.2.3/V.8) have passed, and the modem's own start-up begins
            // here -- phase 2's INFO0 goes on this sample (11.1, 12.2/V.34)
            // -- while the procedure begins phase B with the control channel
            // taken as up, and waits on the modem for it. The end that
            // dialled is the call modem and sends the page; the end that
            // answered is the answer modem and receives it (T.30 has no other
            // case on an ordinary call).
            v8line::Status::Agreed(v8::Modulation::V34HalfDuplex) => {
                self.v8 = None;
                let (v34_role, source) = match role {
                    Role::Caller => (V34Role::Call, true),
                    Role::Answerer => (V34Role::Answer, false),
                };
                let mut modem = Box::new(halfduplex::Modem::new(v34_role, source, self.fs));
                // A cap asked for before the call reached here goes into the
                // first MPh this end sends.
                if let Some(cap) = self.v34_cap {
                    modem.limit_rate(cap);
                }
                self.v34 = Some(HalfDuplex::Modem(modem));
                self.v34_facts = V34Facts::default();
                self.v34_turned = false;
                self.v34_held = 0;
                self.v34_drained = 0;
                self.call.start_annex_f();
            }
            // 6.1.3: ANSam ran out with no call menu, so clause 5 from the
            // DIS, with bit 6 set to say V.8 is here -- and the V.8 modem kept
            // as an ear for the CI that bit invites (6.1.4). 75 ms of silence
            // first (8.2.2/V.8; Figures F.5-8 and F.5-9 have 75 +/- 20 ms),
            // which is the pause the restart sits out.
            v8line::Status::NoNegotiation => {
                debug_assert_eq!(role, Role::Answerer, "the calling end's plain tone never gets here");
                self.call.set_v8_capable(true);
                self.call.restart_identifying();
            }
            // 6.1.6: no V.34 at both ends, so clause 5. For the end that
            // answered that begins with its DIS (Figure F.5-10: 75 +/- 5 ms
            // after JM); for the end that dialled it is waiting for that DIS
            // on V.21, which is what it was doing before. A joint menu with
            // nothing in it ends the same way: 8.2.3 lets the caller hang up
            // on that, and if it does not, the DIS is the right thing for it
            // to hear. Bit 6 stays clear: the V.8 the bit invites has just
            // been had.
            v8line::Status::Agreed(_) | v8line::Status::Failed => {
                self.v8 = None;
                if role == Role::Answerer {
                    self.call.set_v8_capable(false);
                    self.call.restart_identifying();
                }
            }
            v8line::Status::Negotiating => {}
        }
    }

    /// Put up or take down whatever changed.
    fn follow(&mut self, want: Line) {
        if want == self.line {
            return;
        }
        match self.line {
            Line::Control => self.control_tx.set_transmitting(false),
            // Nothing should be left running by the time the procedure moves
            // on, since it waits for the line to go idle first. This is only
            // in case something ends a call in the middle of a burst.
            Line::Fast(_) => {
                self.v27ter_tx.abort();
                self.v29_tx.abort();
                self.v17_tx.abort();
            }
            _ => {}
        }
        match want {
            Line::Control => self.control_tx.set_transmitting(true),
            Line::Fast(speed) => match Carrier::of(speed) {
                // Always V.27 ter's long turn-on sequence. T.30 leaves the
                // choice to the sender, and a fax turns the line around between
                // every message, so nothing is remembered from the last burst
                // that a short one could refresh. V.29 has only the one.
                Some(Carrier::V27ter(rate)) => {
                    self.v27ter_tx.start(rate, v27ter::Training::Long);
                }
                Some(Carrier::V29(rate)) => self.v29_tx.start(rate),
                // T.30 5.1, Note 5: the long train for a training check and
                // for the first message after CTC/CTR, and the resync for
                // every other. What CTC/CTR brings is a new speed, and a
                // resync is only any use to a receiver that has had a long
                // train at the speed it is at, so a message at any speed but
                // the last long train's has the long one too.
                Some(Carrier::V17(rate)) => {
                    let long = self.call.phase() == Phase::Training || self.v17_long != Some(speed);
                    if long {
                        self.v17_long = Some(speed);
                    }
                    let training = if long { v17::Training::Long } else { v17::Training::Resync };
                    self.v17_tx.start(rate, training);
                }
                None => {}
            },
            Line::FastListen(speed) => match Carrier::of(speed) {
                Some(Carrier::V27ter(rate)) => {
                    self.v27ter_rx.set_rate(rate);
                    self.v27ter_rx.restart();
                }
                Some(Carrier::V29(rate)) => {
                    self.v29_rx.set_rate(rate);
                    self.v29_rx.restart();
                }
                // The taps the last long train left are kept through this:
                // a resync is read with them.
                Some(Carrier::V17(rate)) => {
                    self.v17_rx.set_rate(rate);
                    self.v17_rx.restart();
                }
                None => {}
            },
            _ => {}
        }
        self.line = want;
    }

    /// Feed whichever receiver belongs to what the line is doing.
    ///
    /// Only one of them, and only while this end is not talking. On a
    /// two-wire line a receiver left running hears its own transmission, and
    /// a fax is half duplex, so anything it hears while sending is its own
    /// echo. The frames in that echo are the frames it just sent, addressed
    /// the same way, and would be read as the far end agreeing with itself.
    fn listen(&mut self, want: Line, input: f64) {
        match want {
            Line::Quiet | Line::Listen | Line::CallingTone => {
                if let Some(bit) = self.control_rx.feed(input) {
                    self.call.control_bit(bit);
                }
                self.call.set_control_carrier(self.control_rx.carrier());
            }
            Line::FastListen(speed) => {
                // The control channel too. Waiting for a page carrier is when
                // a sender whose last command went unanswered sends it again,
                // and under error correction mode that is the ordinary way to
                // recover a lost confirmation. What a V.21 receiver makes of a
                // page carrier is noise, and noise does not pass a frame check.
                if let Some(bit) = self.control_rx.feed(input) {
                    self.call.control_bit(bit);
                }
                let (bits, carrier) = match Carrier::of(speed) {
                    Some(Carrier::V27ter(_)) => {
                        self.v27ter_rx.feed(input);
                        (self.v27ter_rx.take_bits(), self.v27ter_rx.carrier())
                    }
                    Some(Carrier::V29(_)) => {
                        self.v29_rx.feed(input);
                        (self.v29_rx.take_bits(), self.v29_rx.carrier())
                    }
                    Some(Carrier::V17(_)) => {
                        self.v17_rx.feed(input);
                        (self.v17_rx.take_bits(), self.v17_rx.carrier())
                    }
                    // A speed this end has no receiver for hears nothing, and
                    // the training check that never arrives is refused, which
                    // sends the far end down its own ladder.
                    None => (Vec::new(), false),
                };
                if !bits.is_empty() {
                    self.call.fast_bits(&bits);
                }
                self.call.set_fast_carrier(carrier);
            }
            Line::Control | Line::Fast(_) | Line::CalledTone => {}
            // T.30 Annex F is V.34's half-duplex modem's, and `annex_f_step`
            // hears for it: an Annex F call never comes this way.
            Line::V34Control
            | Line::V34Listen
            | Line::V34Ones
            | Line::V34Primary
            | Line::V34PrimaryListen => {}
        }
    }

    /// Produce the sample, and say whether the line has gone quiet.
    fn talk(&mut self, want: Line) -> (f64, bool) {
        match want {
            Line::Control => {
                while self.control_tx.pending_bits() < 16 {
                    match self.call.next_control_bit() {
                        Some(bit) => self.control_tx.push_bits(&[bit]),
                        None => break,
                    }
                }
                let idle = self.control_tx.pending_bits() == 0;
                (self.control_tx.next_sample(), idle)
            }
            Line::Fast(speed) => match Carrier::of(speed) {
                Some(Carrier::V27ter(_)) => {
                    while self.v27ter_tx.pending_bits() < 32 {
                        match self.call.next_fast_bit() {
                            Some(bit) => self.v27ter_tx.push_bits(&[bit]),
                            None => break,
                        }
                    }
                    // Nothing left to hand over and nothing left in the
                    // modulator: the burst is over, so take the carrier down
                    // with a turn-off rather than cutting it.
                    if self.v27ter_tx.trained() && self.v27ter_tx.pending_bits() == 0 {
                        self.v27ter_tx.stop();
                    }
                    let idle = !self.v27ter_tx.is_transmitting();
                    (self.v27ter_tx.next_sample(), idle)
                }
                Some(Carrier::V29(_)) => {
                    while self.v29_tx.pending_bits() < 32 {
                        match self.call.next_fast_bit() {
                            Some(bit) => self.v29_tx.push_bits(&[bit]),
                            None => break,
                        }
                    }
                    if self.v29_tx.trained() && self.v29_tx.pending_bits() == 0 {
                        self.v29_tx.stop();
                    }
                    let idle = !self.v29_tx.is_transmitting();
                    (self.v29_tx.next_sample(), idle)
                }
                Some(Carrier::V17(_)) => {
                    while self.v17_tx.pending_bits() < 32 {
                        match self.call.next_fast_bit() {
                            Some(bit) => self.v17_tx.push_bits(&[bit]),
                            None => break,
                        }
                    }
                    if self.v17_tx.trained() && self.v17_tx.pending_bits() == 0 {
                        self.v17_tx.stop();
                    }
                    let idle = !self.v17_tx.is_transmitting();
                    (self.v17_tx.next_sample(), idle)
                }
                None => (0.0, true),
            },
            Line::CallingTone => {
                let on = self.call.calling_tone_on();
                // The tone keeps running while it is silent, so its phase is
                // continuous across the gaps rather than clicking at every
                // burst.
                let sample = self.cng.next_sample();
                (if on { sample } else { 0.0 }, true)
            }
            Line::CalledTone => (self.ced.next_sample(), true),
            Line::Quiet | Line::Listen | Line::FastListen(_) => (0.0, true),
            // Annex F's channels are V.34's half-duplex modem's, and
            // `annex_f_step` talks for it: an Annex F call never comes this
            // way.
            Line::V34Control
            | Line::V34Listen
            | Line::V34Ones
            | Line::V34Primary
            | Line::V34PrimaryListen => (0.0, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fax::frames::{Message, Reader};
    use fax::page::Resolution;
    use fax::t30::Frame;

    const FS: f64 = 16_000.0;

    /// A page with something recognisable on it.
    fn a_page(lines: usize) -> Page {
        let width = fax::page::WIDTH;
        Page {
            lines: (0..lines)
                .map(|y| {
                    (0..width)
                        .map(|x| (x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30)
                        .collect()
                })
                .collect(),
            resolution: Resolution::Standard,
        }
    }

    /// Run two of these against each other down one clean wire.
    fn between(caller: &mut FaxCall, answerer: &mut FaxCall, seconds: f64) {
        through(caller, answerer, seconds, &mut |s| s);
    }

    /// The same, with something done to the line in both directions.
    ///
    /// `FAX_TRACE=1` prints every change of phase at both ends. A fax call
    /// goes wrong by one end waiting for something the other has stopped
    /// sending, and that is invisible in an assertion at the end of it.
    fn through(
        caller: &mut FaxCall,
        answerer: &mut FaxCall,
        seconds: f64,
        line: &mut dyn FnMut(f64) -> f64,
    ) {
        let trace = std::env::var("FAX_TRACE").is_ok();
        let (mut was_a, mut was_b) = (caller.phase(), answerer.phase());
        let mut from_caller = 0.0;
        let mut from_answerer = 0.0;
        for i in 0..(seconds * FS) as usize {
            let a = caller.step(from_answerer);
            let b = answerer.step(from_caller);
            from_caller = line(a);
            from_answerer = line(b);
            if trace && (caller.phase() != was_a || answerer.phase() != was_b) {
                eprintln!(
                    "{:6.2}s caller {:<32} answerer {}",
                    i as f64 / FS,
                    caller.phase().name(),
                    answerer.phase().name()
                );
                was_a = caller.phase();
                was_b = answerer.phase();
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
    }

    #[test]
    fn the_calling_tone_goes_on_the_line() {
        let mut call = FaxCall::originate(FS, "61400000000", None);
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 0.3) as usize {
            loudest = loudest.max(call.step(0.0).abs());
        }
        assert!(loudest > 0.1, "nothing went out: {loudest}");
    }

    #[test]
    fn the_answering_tone_goes_on_the_line() {
        let mut call = FaxCall::answer(FS, "61399990000");
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 0.3) as usize {
            loudest = loudest.max(call.step(0.0).abs());
        }
        assert!(loudest > 0.1, "nothing went out: {loudest}");
    }

    /// A V.34 fax calling this end, off a real call: it took the plain answer
    /// tone for ANSam and sent V.8 call menus instead of waiting for a DIS,
    /// and went on sending them past every DIS this end sent. Now the call
    /// menu is answered with a joint menu -- without V.34, which this end
    /// does not have -- and after CJ the DIS goes out, as 6.1.6/T.30 has it.
    #[test]
    fn a_v34_fax_call_menu_is_answered_and_then_the_dis_follows() {
        use datapump::bell103::{Bell103Rx, Bell103Tx};
        use datapump::framing::AsyncBits;
        use datapump::v8::{HIGH, LOW};

        let vector = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/fax-v34-cm.wav");
        let wav = line::wav::read(vector).expect("could not read the vector");
        let fs = f64::from(wav.sample_rate);
        let mut call = FaxCall::answer(fs, "61399990000").offering(&MODULATIONS);

        // What this end sends, read two ways: as V.8 on the high channel, and
        // as T.30's frames on V.21 channel 2, which is the same pair of tones.
        let mut v8_ear = Bell103Rx::with_tones(HIGH.0, HIGH.1, fs);
        let mut v8_decoder = v8::Decoder::new();
        let mut joint = None;
        let mut frames_ear = v21::Receiver::new(fs);
        let mut reader = Reader::new();
        let mut said: Vec<Message> = Vec::new();
        let mut v8_had_the_line = false;
        let mut hear = |call: &mut FaxCall, input: f64| {
            let out = call.step(input);
            if let Some(octet) = v8_ear.feed(out)
                && let Some(v8::Heard::Cm(m) | v8::Heard::Jm(m)) = v8_decoder.feed(octet)
            {
                joint.get_or_insert(m);
            }
            if let Some(bit) = frames_ear.feed(out)
                && let Some(m) = reader.feed(bit)
            {
                said.push(m);
            }
        };

        for &s in &wav.channel(0) {
            hear(&mut call, f64::from(s));
            v8_had_the_line |= call.phase_name().starts_with("V.8");
        }
        assert!(v8_had_the_line, "the call menu went unanswered: {}", call.phase_name());
        // The caller has its JMs, so it ends the call menus with CJ on the
        // carrier that is already up (8.1.2).
        let framing = AsyncBits::new(8);
        let mut cj = Bell103Tx::with_tones(LOW.0, LOW.1, fs);
        cj.set_transmitting(true);
        for octet in v8::CJ {
            cj.push_bits(&framing.encode(octet));
        }
        while cj.pending_bits() > 0 {
            hear(&mut call, cj.next_sample());
        }
        for _ in 0..(fs * 3.0) as usize {
            hear(&mut call, 0.0);
        }

        let far = call.far_menu().expect("the call menu was not kept");
        assert_eq!(far.function, v8::CallFunction::TransmitFax);
        assert!(far.modulations.contains(v8::Modulation::V34HalfDuplex), "{far:?}");
        let joint = joint.expect("no joint menu went out");
        assert!(!joint.modulations.contains(v8::Modulation::V34HalfDuplex), "{joint:?}");
        assert!(joint.modulations.contains(v8::Modulation::V17), "{joint:?}");
        let names: Vec<Frame> = said.iter().map(|m| m.frame).collect();
        assert!(names.contains(&Frame::Dis), "no DIS after the V.8 exchange: {names:?}");
        assert_eq!(call.phase(), Phase::AwaitingCommand, "{}", call.phase_name());
    }

    /// A fax that answers the ordinary way is not disturbed by listening for
    /// V.8: the whole call still goes through, with no call menu anywhere.
    #[test]
    fn listening_for_a_call_menu_leaves_an_ordinary_call_alone() {
        let mut caller = FaxCall::originate(FS, "61400000000", Some(a_page(40)));
        let mut answerer = FaxCall::answer(FS, "61399990000");
        between(&mut caller, &mut answerer, 60.0);
        assert_eq!(answerer.phase(), Phase::Done, "{:?}", answerer.trouble());
        assert!(answerer.far_menu().is_none());
        assert_eq!(answerer.pages_received(), 1);
    }

    // ---- V.34 offered: T.30 clause 6 ---------------------------------------

    /// Everything this end has, as a V.34 fax's call or joint menu names it.
    fn super_g3_menu() -> v8::Modulations {
        v8::Modulations::of(&[
            v8::Modulation::V34HalfDuplex,
            v8::Modulation::V17,
            v8::Modulation::V29HalfDuplex,
            v8::Modulation::V27ter,
        ])
    }

    /// One direction of the line read as V.8: the menus, CJ and CI on it,
    /// each with when it finished arriving.
    struct V8Ear {
        rx: datapump::bell103::Bell103Rx,
        decoder: v8::Decoder,
        heard: Vec<(f64, v8::Heard)>,
    }

    impl V8Ear {
        /// On the channel the calling end sends in, or the answering end's.
        fn new(calling: bool, fs: f64) -> Self {
            let (space, mark) = if calling { datapump::v8::LOW } else { datapump::v8::HIGH };
            Self {
                rx: datapump::bell103::Bell103Rx::with_tones(space, mark, fs),
                decoder: v8::Decoder::new(),
                heard: Vec::new(),
            }
        }

        fn feed(&mut self, t: f64, sample: f64) {
            if let Some(octet) = self.rx.feed(sample)
                && let Some(h) = self.decoder.feed(octet)
            {
                self.heard.push((t, h));
            }
        }

        /// The first menu heard, and when.
        fn menu(&self) -> Option<(f64, v8::Menu)> {
            self.heard.iter().find_map(|(t, h)| match h {
                v8::Heard::Cm(m) | v8::Heard::Jm(m) => Some((*t, *m)),
                _ => None,
            })
        }

        fn cj(&self) -> Option<f64> {
            self.heard.iter().find_map(|(t, h)| (*h == v8::Heard::Cj).then_some(*t))
        }
    }

    /// The amplitude of `block` at `f`, as a plain correlation.
    fn amplitude_at(block: &[f64], f: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &x) in block.iter().enumerate() {
            let w = std::f64::consts::TAU * f * i as f64 / FS;
            re += x * w.cos();
            im -= x * w.sin();
        }
        2.0 * re.hypot(im) / block.len() as f64
    }

    /// Whether a block of the calling end's output is its calling tone:
    /// 1100 Hz (5.1.1/T.30) standing well above V.21's low channel either
    /// side of it, which is where its call menu would be instead.
    fn is_calling_tone(block: &[f64]) -> bool {
        let cng = amplitude_at(block, v21::CNG);
        let (space, mark) = datapump::v8::LOW;
        cng > 0.01 && cng > 4.0 * (amplitude_at(block, space) + amplitude_at(block, mark))
    }

    /// What two ends that both offer V.34 said to each other, and when.
    #[derive(Debug, Default)]
    struct Exchange {
        ansam: Option<f64>,
        cm: Option<(f64, v8::Menu)>,
        jm: Option<(f64, v8::Menu)>,
        cj: Option<f64>,
        /// When the calling end's V.8 took the line: on ANSam.
        caller_took_the_line: Option<f64>,
        /// When each end went to Annex F.
        caller_agreed: Option<f64>,
        answerer_agreed: Option<f64>,
        /// Blocks of the calling end's output that were its calling tone,
        /// before and after its V.8 took the line.
        cng_before: usize,
        cng_after: usize,
        /// The loudest sample either end sent once both were in Annex F.
        loudest_after: f64,
    }

    /// Run two ends against each other with `delay` seconds each way, taps
    /// on both directions, for `seconds` or until both are in Annex F and
    /// have been for a second.
    fn negotiate_v34(
        caller: &mut FaxCall,
        answerer: &mut FaxCall,
        delay: f64,
        seconds: f64,
    ) -> Exchange {
        use std::collections::VecDeque;
        let lag = (delay * FS) as usize;
        let mut to_caller: VecDeque<f64> = std::iter::repeat_n(0.0, lag + 1).collect();
        let mut to_answerer: VecDeque<f64> = std::iter::repeat_n(0.0, lag + 1).collect();
        let mut tone = v8::AnswerTone::new(FS);
        let mut low = V8Ear::new(true, FS);
        let mut high = V8Ear::new(false, FS);
        let mut block: Vec<f64> = Vec::new();
        let mut x = Exchange::default();
        for i in 0..(seconds * FS) as usize {
            let t = i as f64 / FS;
            let a = caller.step(to_caller.pop_front().unwrap_or(0.0));
            let b = answerer.step(to_answerer.pop_front().unwrap_or(0.0));
            to_answerer.push_back(a);
            to_caller.push_back(b);
            tone.feed(b);
            if x.ansam.is_none() && tone.is_ansam() {
                x.ansam = Some(t);
            }
            low.feed(t, a);
            high.feed(t, b);
            if x.caller_took_the_line.is_none() && caller.phase_name().starts_with("V.8") {
                x.caller_took_the_line = Some(t);
            }
            if x.caller_agreed.is_none() && caller.v34_agreed() {
                x.caller_agreed = Some(t);
            }
            if x.answerer_agreed.is_none() && answerer.v34_agreed() {
                x.answerer_agreed = Some(t);
            }
            block.push(a);
            if block.len() == (FS * 0.05) as usize {
                if is_calling_tone(&block) {
                    if x.caller_took_the_line.is_some() {
                        x.cng_after += 1;
                    } else {
                        x.cng_before += 1;
                    }
                }
                block.clear();
            }
            if let (Some(ca), Some(aa)) = (x.caller_agreed, x.answerer_agreed) {
                x.loudest_after = x.loudest_after.max(a.abs()).max(b.abs());
                if t > ca.max(aa) + 1.0 {
                    break;
                }
            }
        }
        x.cm = low.menu();
        x.jm = high.menu();
        x.cj = low.cj();
        x
    }

    /// Two of these that both offer V.34 agree it over V.8 and go to Annex F:
    /// ANSam, CM, JM, CJ, and both ends at the hand-over point within a few
    /// seconds, with V.34's phase 2 begun there -- with the calling tone
    /// stopped from ANSam on (8.1.1/V.8), which is why the line has 0.4 s of
    /// delay each way: without it V.8 is over before the second burst of
    /// calling tone is due.
    #[test]
    fn two_of_these_offering_v34_agree_it_over_v8_and_go_to_annex_f() {
        let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(8)))
            .offering(&MODULATIONS)
            .with_v34(true);
        let mut answerer =
            FaxCall::answer(FS, "61388880000").offering(&MODULATIONS).with_v34(true);
        let x = negotiate_v34(&mut caller, &mut answerer, 0.4, 10.0);

        let ansam = x.ansam.expect("no ANSam from the answering end");
        let (cm_at, cm) = x.cm.expect("no call menu from the calling end");
        let (jm_at, jm) = x.jm.expect("no joint menu from the answering end");
        let cj = x.cj.expect("no CJ from the calling end");
        assert!(ansam < cm_at && cm_at < jm_at && jm_at < cj, "out of order: {x:?}");
        assert_eq!(cm.function, v8::CallFunction::TransmitFax, "{cm:?}");
        assert_eq!(cm.modulations, super_g3_menu(), "{cm:?}");
        assert_eq!(jm.modulations, super_g3_menu(), "{jm:?}");
        assert!(
            !cm.modulations.contains(v8::Modulation::V34Duplex),
            "offered duplex, which 7.4/V.8 would pick over half-duplex"
        );

        let caller_agreed = x.caller_agreed.expect("the calling end never went to Annex F");
        let answerer_agreed = x.answerer_agreed.expect("the answering end never went to Annex F");
        assert!(caller_agreed < 7.0 && answerer_agreed < 7.0, "{x:?}");
        assert!(caller_agreed > cj && answerer_agreed > cj, "in Annex F before CJ: {x:?}");
        // A second after the hand-over both ends are in V.34's phase 2 --
        // INFO0, the tones, the probe -- and T.30 has not been asked for a
        // bit: the answerer still has its CSI and DIS queued, the caller is
        // still waiting for them.
        assert_eq!(caller.phase(), Phase::Listening, "{}", caller.phase_name());
        assert_eq!(answerer.phase(), Phase::Identifying, "{}", answerer.phase_name());
        assert!(x.loudest_after > 0.1, "nothing of phase 2 on the line after the hand-over: {x:?}");
        assert!(caller.phase_name().starts_with("V.34"), "{}", caller.phase_name());
        assert!(answerer.phase_name().starts_with("V.34"), "{}", answerer.phase_name());
        assert_eq!(caller.standard(), "V.34");

        // 8.1.1/V.8: "after detection of ANS or ANSam, the call signal shall
        // be stopped". The first burst of it went; none went after.
        let took = x.caller_took_the_line.expect("the calling end's V.8 never took the line");
        assert!(took > ansam, "{x:?}");
        assert!(x.cng_before > 0, "no calling tone before ANSam, so nothing was proved: {x:?}");
        assert_eq!(x.cng_after, 0, "calling tone after ANSam: {x:?}");

        // Both ends keep what was agreed, for the far-end panel.
        assert_eq!(caller.joint_menu(), Some(jm));
        assert_eq!(answerer.joint_menu(), Some(jm));
        assert_eq!(caller.far_menu(), Some(jm), "the caller's far menu is the JM");
        assert_eq!(answerer.far_menu(), Some(cm), "the answerer's far menu is the CM");
    }

    /// The recorded V.34 fax's call menu, answered by an end that offers
    /// V.34: the joint menu carries V.34 half-duplex, and after CJ the call
    /// is in Annex F with no DIS on V.21 -- INFO0a goes out instead, V.34's
    /// phase 2 having the line (11.1, 12.2.1.2.1/V.34).
    #[test]
    fn a_v34_fax_call_menu_is_answered_with_v34_and_the_call_goes_to_annex_f() {
        use datapump::bell103::Bell103Tx;
        use datapump::framing::AsyncBits;
        use datapump::v8::LOW;

        let vector = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/fax-v34-cm.wav");
        let wav = line::wav::read(vector).expect("could not read the vector");
        let fs = f64::from(wav.sample_rate);
        let mut call = FaxCall::answer(fs, "61399990000").offering(&MODULATIONS).with_v34(true);

        let mut high = V8Ear::new(false, fs);
        let mut frames_ear = v21::Receiver::new(fs);
        let mut reader = Reader::new();
        let mut said: Vec<Message> = Vec::new();
        // The answer modem's INFO sequences, as the calling modem would read
        // them (10.1.2.3.1/V.34).
        let mut info_ear = datapump::v34::dpsk::Receiver::half_duplex(datapump::v34::dpsk::Side::Answer, fs);
        let mut info0 = None;
        let mut t = 0.0;
        let mut loudest_after_handover = 0.0f64;
        let mut hear = |call: &mut FaxCall, input: f64, after_handover: bool| {
            let out = call.step(input);
            t += 1.0 / fs;
            high.feed(t, out);
            if let Some(bit) = frames_ear.feed(out)
                && let Some(m) = reader.feed(bit)
            {
                said.push(m);
            }
            if after_handover {
                loudest_after_handover = loudest_after_handover.max(out.abs());
                if let Some(datapump::v34::info::Info::Info0(info)) = info_ear.feed(out) {
                    info0.get_or_insert(info);
                }
            }
        };

        for &s in &wav.channel(0) {
            hear(&mut call, f64::from(s), false);
        }
        assert!(!call.v34_agreed(), "in Annex F before CJ");
        let framing = AsyncBits::new(8);
        let mut cj = Bell103Tx::with_tones(LOW.0, LOW.1, fs);
        cj.set_transmitting(true);
        for octet in v8::CJ {
            cj.push_bits(&framing.encode(octet));
        }
        while cj.pending_bits() > 0 {
            hear(&mut call, cj.next_sample(), false);
        }
        // 8.2.3/V.8: JM stops on CJ, and 75 ms of silence follow -- and then
        // INFO0a, from the hand-over point (the CJ's last stop bit is still
        // going through the ear for a moment, so the hand-over is looked for
        // rather than timed).
        for _ in 0..(fs * 0.2) as usize {
            let agreed = call.v34_agreed();
            hear(&mut call, 0.0, agreed);
        }
        assert!(call.v34_agreed(), "not in Annex F after CJ: {}", call.phase_name());
        for _ in 0..(fs * 3.0) as usize {
            hear(&mut call, 0.0, true);
        }

        let far = call.far_menu().expect("the call menu was not kept");
        assert_eq!(far.function, v8::CallFunction::TransmitFax);
        assert!(far.modulations.contains(v8::Modulation::V34HalfDuplex), "{far:?}");
        let (_, joint) = high.menu().expect("no joint menu went out");
        assert!(joint.modulations.contains(v8::Modulation::V34HalfDuplex), "{joint:?}");
        assert!(joint.modulations.contains(v8::Modulation::V17), "{joint:?}");
        assert_eq!(call.joint_menu(), Some(joint));
        // T.30 has its CSI and DIS queued for a control channel that is not
        // up yet; V.34's phase 2 has the line, and its INFO0a is what went
        // out, not a frame.
        assert_eq!(call.phase(), Phase::Identifying, "{}", call.phase_name());
        assert!(call.phase_name().starts_with("V.34"), "{}", call.phase_name());
        assert!(loudest_after_handover > 0.1, "nothing went out after the hand-over");
        let info0 = info0.expect("no INFO0a after CJ");
        assert!(info0.rate_3429 && info0.constellation_1664, "not this end's capabilities: {info0:?}");
        let names: Vec<Frame> = said.iter().map(|m| m.frame).collect();
        assert!(names.is_empty(), "frames on V.21 in an Annex F call: {names:?}");
    }

    /// Two of these that both offer V.34 fax a page over it: V.8, phase 2,
    /// phase 3, the control channel start-up, phase B's frames on the control
    /// channel, the turn to the primary channel, the page at 33 600, and the
    /// way back for the receipt and the disconnect.
    ///
    /// `FAX_TRACE=1` prints every change of phase at both ends, the modem's
    /// included.
    #[test]
    fn two_of_these_offering_v34_fax_a_page_over_it() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()))
            .offering(&MODULATIONS)
            .with_v34(true);
        let mut answerer = FaxCall::answer(FS, "61388880000").offering(&MODULATIONS).with_v34(true);
        let trace = std::env::var("FAX_TRACE").is_ok();
        let (mut was_a, mut was_b) = ("", "");
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut rate_while_sending = None;
        // The page's points at the answerer, and the range the window's scope
        // is given for them.
        let (mut reach, mut scope) = (0.0f64, None);
        for i in 0..(FS * 60.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            if caller.phase() == Phase::Sending {
                rate_while_sending = caller.primary_rate();
            }
            if answerer.shape().ends_with("TCM")
                && let Some((x, y)) = answerer.constellation_point()
            {
                reach = reach.max(x.abs()).max(y.abs());
                scope = Some(answerer.constellation_peak());
            }
            if trace && (caller.phase_name() != was_a || answerer.phase_name() != was_b) {
                eprintln!("{:6.2}s caller {:<40} answerer {}", i as f64 / FS, caller.phase_name(), answerer.phase_name());
                was_a = caller.phase_name();
                was_b = answerer.phase_name();
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        assert!(caller.v34_agreed() && answerer.v34_agreed());
        assert_eq!(caller.phase(), Phase::Done, "the caller: {:?}", caller.trouble());
        assert_eq!(answerer.phase(), Phase::Done, "the answerer: {:?}", answerer.trouble());
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines, page.lines, "the page came out different");
        assert_eq!(rate_while_sending, Some(33_600));
        assert_eq!(caller.symbol_rate(), Some(3429));
        assert_eq!(answerer.primary_rate(), Some(33_600));
        assert!(caller.error_correction() && answerer.error_correction(), "F.3: ECM is mandatory");
        assert_eq!((caller.standard(), answerer.standard()), ("V.34", "V.34"));
        // Drawn to scale: the page's points fill the scope and stay inside it.
        // Sized in grid units, the range was tens against points of one and
        // a half, and the first real Super G3 page was one dot at the centre.
        let scope = scope.expect("no point of the page to draw");
        assert!(
            reach > 0.5 * scope && reach < 1.2 * scope,
            "the page's points reach {reach:.3} in a scope sized {scope:.3}"
        );
        eprintln!("done in {:.1} s", caller.seconds());
    }

    /// A recipient that retrains the primary channel (12.7/V.34) just as the
    /// source begins its ones for the page. Its flags stop for the 70 ms of
    /// silence in front of its tone, and T.30 takes that for its turn after
    /// a tenth of a second without a flag -- over this little noise, 133 ms
    /// after the recipient stopped, before the source's modem has heard the
    /// 50 ms of tone that send it into the retrain (at about 155 ms). A join
    /// that turned then sent the page into a recipient in phase 2. This one
    /// holds the turn for the modem to hear the recipient silent; the modem
    /// hears the tone instead and joins the retrain; and the turn is made
    /// once the channel is back. The page arrives whole, one retrain each.
    #[test]
    fn a_page_waits_out_a_primary_channel_retrain_begun_as_it_was_due() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()))
            .offering(&MODULATIONS)
            .with_v34(true);
        let mut answerer = FaxCall::answer(FS, "61388880000").offering(&MODULATIONS).with_v34(true);
        // The noise `a_page_gets_through_a_line_with_noise_on_it` has. On a
        // clean line T.30's flags-gone comes 10 ms after the modem has left
        // for the retrain, and there is no race to lose.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 0.02 - 0.01
        };
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut retrain_at = None;
        // When the caller's T.30 asked for the page, with what its modem was
        // doing then, and when the modem took the turn.
        let (mut asked, mut turned) = (None, None);
        for i in 0..(FS * 60.0) as usize {
            let t = i as f64 / FS;
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b + noise();
            to_answerer = a + noise();
            if retrain_at.is_none()
                && caller.call.line() == Line::V34Ones
                && let Some(HalfDuplex::Modem(modem)) = answerer.v34.as_mut()
            {
                assert!(modem.retrain_primary(), "the recipient could not retrain: {}", modem.phase());
                retrain_at = Some(t);
            }
            if asked.is_none()
                && caller.call.line() == Line::V34Primary
                && let Some(HalfDuplex::Modem(modem)) = caller.v34.as_ref()
            {
                asked = Some((t, modem.state()));
            }
            if turned.is_none() && caller.v34_turned {
                turned = Some(t);
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        let retrain_at = retrain_at.expect("the caller never sent its ones");
        let (asked, state) = asked.expect("the caller's T.30 never asked for the page");
        let turned = turned.expect("the caller never turned to the page");
        // T.30 took the stopped flags for the recipient's turn around the
        // moment the modem heard the retrain's tone: before it, on the control
        // channel still, or just after, in the retrain. The race is 130 to
        // 165 ms against about 155, and which side of it a call lands on
        // moves with the call's timing -- the source waiting for the far E
        // (E_WAIT in halfduplex.rs) put this one just after. Either way the
        // turn is asked for while the recipient is leaving for phase 2...
        assert!(
            matches!(state, State::Control | State::Retraining),
            "T.30 asked {:.3} s after the retrain began, with the modem {state:?}",
            asked - retrain_at
        );
        // ...and the turn waited out the retrain -- phase 2, phase 3 and a
        // control channel start-up -- which is seconds, not a tenth of one.
        assert!(turned - asked > 1.0, "turned {:.3} s after T.30 asked", turned - asked);
        assert_eq!(caller.phase(), Phase::Done, "the caller: {:?}", caller.trouble());
        assert_eq!(answerer.phase(), Phase::Done, "the answerer: {:?}", answerer.trouble());
        assert_eq!(answerer.received().expect("no page arrived").lines, page.lines);
        assert_eq!(caller.v34_retrains(), (1, 0), "the caller");
        assert_eq!(answerer.v34_retrains(), (1, 0), "the answerer");
        assert_eq!(caller.primary_rate(), Some(33_600));
    }

    /// An end offering V.34 that dials a fax without it: the plain called
    /// tone says the far end does not do V.8, and the call is clause 5's,
    /// unchanged -- the same page, and the same length of call to the
    /// sample.
    #[test]
    fn a_caller_offering_v34_to_a_plain_answerer_faxes_a_page_as_before() {
        let page = a_page(8);
        let mut ended = Vec::new();
        for v34 in [false, true] {
            let mut caller =
                FaxCall::originate(FS, "61399990000", Some(page.clone())).with_v34(v34);
            let mut answerer = FaxCall::answer(FS, "61388880000");
            between(&mut caller, &mut answerer, 40.0);
            assert_eq!(caller.phase(), Phase::Done, "v34 {v34}: {:?}", caller.trouble());
            let got = answerer.received().expect("no page arrived");
            assert_eq!(got.lines, page.lines, "the page came out different");
            assert!(!caller.v34_agreed() && caller.far_menu().is_none(), "V.8 with a plain fax");
            assert!(answerer.far_menu().is_none(), "a call menu went to a plain fax");
            ended.push((caller.seconds(), answerer.seconds()));
        }
        assert_eq!(ended[0], ended[1], "offering V.34 changed the timing of a plain call");
    }

    /// What one end's V.21 channel 2 carried, and its answering tone.
    #[derive(Debug, Default)]
    struct AnswererTap {
        /// When the answering tone read as ANSam, first and last.
        ansam: Option<(f64, f64)>,
        /// The longest run of the plain tone, in seconds, after ANSam ended.
        plain_after: f64,
        plain_run: f64,
        /// When V.21's carrier first came up after ANSam ended.
        carrier_after: Option<f64>,
        /// The first DIS, with its parameter field.
        dis: Option<(f64, Vec<u8>)>,
    }

    /// An end that answers offering V.34, called by a fax without it: ANSam
    /// runs out inside T.30 6.1.1's 2.6 to 4.0 s, the DIS follows on V.21
    /// with bit 6 set and no called tone in between (6.1.3, Figures F.5-8
    /// and F.5-9), and the page goes through on clause 5.
    #[test]
    fn an_answerer_offering_v34_to_a_plain_caller_times_out_ansam_and_the_page_follows() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000").with_v34(true);
        let mut tone = v8::AnswerTone::new(FS);
        let mut ear = v21::Receiver::new(FS);
        let mut reader = Reader::new();
        let mut tap = AnswererTap::default();
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for i in 0..(FS * 40.0) as usize {
            let t = i as f64 / FS;
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            tone.feed(b);
            if tone.is_ansam() {
                tap.ansam = Some((tap.ansam.map_or(t, |(first, _)| first), t));
            }
            if let Some((_, last)) = tap.ansam
                && t > last + 0.5
            {
                tap.plain_run = if tone.is_plain() { tap.plain_run + 1.0 / FS } else { 0.0 };
                tap.plain_after = tap.plain_after.max(tap.plain_run);
                if tap.carrier_after.is_none() && ear.carrier() {
                    tap.carrier_after = Some(t);
                }
            }
            if let Some(bit) = ear.feed(b)
                && let Some(m) = reader.feed(bit)
                && m.frame == Frame::Dis
                && tap.dis.is_none()
            {
                tap.dis = Some((t, m.fif.clone()));
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }

        let (first, last) = tap.ansam.expect("no ANSam");
        let ran_for = last - first;
        assert!(
            (2.4..=4.0).contains(&ran_for),
            "ANSam read for {ran_for:.2} s, from {first:.2} to {last:.2}"
        );
        assert!(tap.plain_after < 0.3, "a called tone after ANSam, {:.2} s of it", tap.plain_after);
        let carrier = tap.carrier_after.expect("no V.21 after ANSam");
        assert!(carrier < last + 0.8, "V.21 up at {carrier:.2} s, {:.2} s after ANSam", carrier - last);
        let (dis_at, fif) = tap.dis.expect("no DIS");
        assert!(fax::t30::bit(&fif, 6), "bit 6 clear in the DIS after ANSam ran out: {fif:02x?}");
        assert!(dis_at > last, "a DIS during ANSam");
        assert_eq!(answerer.phase(), Phase::Done, "{:?}", answerer.trouble());
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines, page.lines, "the page came out different");
        assert!(!answerer.v34_agreed() && answerer.far_menu().is_none());
    }

    /// What an answering end put on the line, read as its answering tone
    /// and as T.30's frames: every span in which the tone read as ANSam, and
    /// every DIS.
    struct CiTap {
        tone: v8::AnswerTone,
        ear: v21::Receiver,
        reader: Reader,
        dis: Vec<(f64, Vec<u8>)>,
        ansam_spans: Vec<(f64, f64)>,
        t: f64,
    }

    impl CiTap {
        fn new() -> Self {
            Self {
                tone: v8::AnswerTone::new(FS),
                ear: v21::Receiver::new(FS),
                reader: Reader::new(),
                dis: Vec::new(),
                ansam_spans: Vec::new(),
                t: 0.0,
            }
        }

        /// One sample into the call, and what came out of it noted.
        fn hear(&mut self, call: &mut FaxCall, input: f64) {
            let out = call.step(input);
            self.t += 1.0 / FS;
            self.tone.feed(out);
            if self.tone.is_ansam() {
                match self.ansam_spans.last_mut() {
                    Some((_, last)) if self.t - *last < 0.5 => *last = self.t,
                    _ => self.ansam_spans.push((self.t, self.t)),
                }
            }
            if let Some(bit) = self.ear.feed(out)
                && let Some(m) = self.reader.feed(bit)
                && m.frame == Frame::Dis
            {
                self.dis.push((self.t, m.fif.clone()));
            }
        }
    }

    /// T.30 6.1.4: a caller that took the DIS's bit 6 up with CI gets ANSam
    /// again, and the exchange after it goes to Annex F.
    #[test]
    fn a_ci_after_the_dis_brings_ansam_back_and_the_call_to_annex_f() {
        use datapump::bell103::Bell103Tx;
        use datapump::framing::AsyncBits;
        use datapump::v8::LOW;

        let mut call = FaxCall::answer(FS, "61388880000").offering(&MODULATIONS).with_v34(true);
        let mut tap = CiTap::new();

        // Nothing from the caller: ANSam runs out and the DIS goes. Eight
        // seconds, because 300 bit/s is slow: 0.2 s of silence, 3.8 s of
        // ANSam, the 75 ms pause, and then a second of preamble, CSI and DIS
        // -- about 2.1 s of frames -- so the DIS closes at about 6.2 s, and
        // T2 would not bring another before 12.
        for _ in 0..(FS * 8.0) as usize {
            tap.hear(&mut call, 0.0);
        }
        assert_eq!(tap.ansam_spans.len(), 1, "{:?}", tap.ansam_spans);
        assert_eq!(tap.dis.len(), 1, "{} DISes in eight seconds", tap.dis.len());
        assert!(fax::t30::bit(&tap.dis[0].1, 6), "bit 6 clear: {:02x?}", tap.dis[0].1);
        assert_eq!(call.phase(), Phase::AwaitingCommand);

        // The caller's CI: 7.1/V.8, at least three sequences.
        let framing = AsyncBits::new(8);
        let mut caller = Bell103Tx::with_tones(LOW.0, LOW.1, FS);
        caller.set_transmitting(true);
        let menu = v8::Menu {
            function: v8::CallFunction::TransmitFax,
            modulations: super_g3_menu(),
            protocol: v8::Protocol::Unstated,
            access: None,
            pcm: None,
        };
        for _ in 0..3 {
            let mut bits = vec![true; v8::PREAMBLE_ONES];
            for octet in v8::sequence(v8::Signal::Ci, &menu) {
                bits.extend(framing.encode(octet));
            }
            caller.push_bits(&bits);
        }
        while caller.pending_bits() > 0 {
            tap.hear(&mut call, caller.next_sample());
        }
        for _ in 0..(FS * 1.0) as usize {
            tap.hear(&mut call, 0.0);
        }
        assert_eq!(tap.ansam_spans.len(), 2, "no ANSam after the CI: {:?}", tap.ansam_spans);
        assert!(call.phase_name().starts_with("V.8"), "{}", call.phase_name());

        // And the menus, as in any V.8 exchange.
        for _ in 0..6 {
            let mut bits = vec![true; v8::PREAMBLE_ONES];
            for octet in v8::sequence(v8::Signal::Cm, &menu) {
                bits.extend(framing.encode(octet));
            }
            caller.push_bits(&bits);
        }
        for octet in v8::CJ {
            caller.push_bits(&framing.encode(octet));
        }
        while caller.pending_bits() > 0 {
            tap.hear(&mut call, caller.next_sample());
        }
        for _ in 0..(FS * 0.3) as usize {
            tap.hear(&mut call, 0.0);
        }
        assert!(call.v34_agreed(), "not in Annex F: {}", call.phase_name());
        assert_eq!(call.phase(), Phase::Identifying);
        assert_eq!(tap.dis.len(), 1, "a DIS on V.21 after V.8 agreed V.34");
        let joint = call.joint_menu().expect("no joint menu");
        assert!(joint.modulations.contains(v8::Modulation::V34HalfDuplex), "{joint:?}");
    }

    #[test]
    fn a_machine_answering_is_heard_and_answered() {
        // The far end is a recording of a real one: its identification and
        // its capabilities, sent on V.21 as it would send them.
        let mut far_tx = fax::frames::Sender::new();
        far_tx.send(&[
            Message::new(Frame::Csi, false)
                .and_more()
                .with_fif(b"       909 863  0031"),
            Message::new(Frame::Dis, false).with_fif(&[0x00, 0x6e, 0xf8, 0x00]),
        ]);
        let mut far = v21::Sender::new(FS);
        far.set_transmitting(true);

        let mut call = FaxCall::originate(FS, "61400000000", None);
        let mut ours = v21::Receiver::new(FS);
        let mut reader = Reader::new();
        let mut said: Vec<Message> = Vec::new();

        for _ in 0..(FS * 20.0) as usize {
            while far.pending_bits() < 16 {
                match far_tx.next_bit() {
                    Some(b) => far.push_bits(&[b]),
                    None => break,
                }
            }
            let from_far = far.next_sample();
            let from_us = call.step(from_far);
            if let Some(bit) = ours.feed(from_us)
                && let Some(m) = reader.feed(bit)
            {
                said.push(m);
            }
            if call.phase().is_over() {
                break;
            }
        }

        assert_eq!(call.identity(), "1300  368 909");
        let caps = call.capabilities().expect("it said what it can do");
        assert_eq!(caps.modulations.len(), 3, "V.27ter, V.29 and V.17");

        let names: Vec<Frame> = said.iter().map(|m| m.frame).collect();
        assert!(
            names.contains(&Frame::Tsi),
            "this end never identified itself: {names:?}"
        );
        assert!(
            names.contains(&Frame::Dcs),
            "this end never said how it would send: {names:?}"
        );
    }

    #[test]
    fn one_modem_faxes_a_page_to_another() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);

        assert_eq!(
            caller.phase(),
            Phase::Done,
            "the caller ended at {} ({:?})",
            caller.phase().name(),
            caller.trouble()
        );
        assert_eq!(
            answerer.phase(),
            Phase::Done,
            "the answerer ended at {} ({:?})",
            answerer.phase().name(),
            answerer.trouble()
        );
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines.len(), page.lines.len(), "wrong number of lines");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }


    /// A page over a line with noise on it.
    ///
    /// Not a real channel -- there is no filtering and no echo -- but enough
    /// to prove that the page is not getting through because both ends are
    /// working from arithmetic that happens to match. A single bit error in
    /// the training check used to throw the rate away, and a single one in
    /// the page has to cost one line rather than the page.
    #[test]
    fn a_page_gets_through_a_line_with_noise_on_it() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut noise = move || {
            // Xorshift, so the run is the same every time it is looked at.
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 0.02 - 0.01
        };
        through(&mut caller, &mut answerer, 40.0, &mut |s| s + noise());
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }

    /// A page over a line that is simply quiet.
    ///
    /// Thirty decibels down is about what a real one delivered, once the
    /// drive setting and the path between the two machines had had it. The
    /// carrier detector had a threshold picked from a loopback, where the far
    /// end is exactly as loud as this end wrote it, so it never saw the
    /// carrier at all -- and everything downstream of a carrier detector is
    /// held still until it says there is something there.
    #[test]
    fn a_page_gets_through_a_line_thirty_decibels_down() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        through(&mut caller, &mut answerer, 40.0, &mut |s| s * 0.0316);
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }


    /// A fax call has something to put on the scope the whole way through.
    ///
    /// Two different pictures, because there are two carriers. The 300 bit/s
    /// channel is frequency shift keying and what it has is an eye; the page
    /// carrier -- V.29 at 9600, between two of these -- is sixteen points on
    /// two radii, and what it has is a constellation. A panel showing neither for the whole of a call is a
    /// panel that has nothing to say about the one modulation in the call
    /// that can actually go wrong.
    #[test]
    fn both_carriers_of_a_fax_call_reach_the_scope() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");

        let mut shapes: Vec<&str> = Vec::new();
        let mut eye = 0usize;
        let mut points: Vec<(f64, f64)> = Vec::new();
        let mut worst_reception = 0.0f64;

        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for _ in 0..(FS * 40.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;

            let shape = answerer.shape();
            if shapes.last() != Some(&shape) {
                shapes.push(shape);
            }
            if answerer.take_symbol().is_some() {
                eye += 1;
            }
            if let Some(p) = answerer.constellation_point() {
                points.push(p);
            }
            // Only while the page is actually moving: before the carrier
            // arrives the equaliser has nothing to be right or wrong about.
            if answerer.phase() == Phase::Receiving
                && answerer.lines_received() > 2
                && let Some(r) = answerer.reception()
            {
                worst_reception = worst_reception.max(r);
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }

        assert!(answerer.received().is_some(), "the page did not arrive");
        assert!(
            shapes.contains(&"2FSK") && shapes.contains(&"16APM"),
            "the scope was never told what it was drawing: {shapes:?}"
        );
        assert!(eye > 500, "only {eye} readings for the eye");
        assert!(points.len() > 1000, "only {} points", points.len());

        // V.29's points run from the inner diagonals, at the square root of
        // 2 over that of 13.5, to the outer axes at 5 over it: 0.38 to 1.36
        // with the mean power made one. Anything well outside is not a point.
        let strays = points
            .iter()
            .filter(|(x, y)| {
                let r = (x * x + y * y).sqrt();
                !(0.2..1.6).contains(&r)
            })
            .count();
        assert!(
            strays * 20 < points.len(),
            "{strays} of {} points were nowhere near the circle",
            points.len()
        );
        assert!(
            worst_reception < 0.25,
            "the receiver was missing by {worst_reception:.2} of a gap, and \
             half is the decision boundary"
        );
    }

    /// What a real fax machine sends in front of its training.
    ///
    /// A public fax service sending to this modem put V.27 ter's protection
    /// against talker echo in front of every training check: a fifth of a
    /// second of plain carrier, twenty milliseconds of silence, then the
    /// training. The carrier going away for those twenty milliseconds was
    /// taken for the end of the burst, so the answering end judged a training
    /// check made of nothing but that plain carrier, refused it, and did the
    /// same again at 2400 until the far end gave up. Its receiver had read the
    /// training check perfectly -- 7195 zeros out of 7200, both times.
    #[test]
    fn a_silence_inside_the_training_is_not_the_end_of_the_burst() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "1300368909", Some(page.clone()))
            .offering(&[Modulation::V27ter])
            .with_echo_protection(true);
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        let got = answerer
            .received()
            .unwrap_or_else(|| panic!("no page arrived ({:?})", answerer.trouble()));
        assert_eq!(got.lines, page.lines, "the page came out different");
        assert_eq!(caller.rate(), 4800, "the far end had to drop a rate to get through");
    }


    /// The end that is sending draws what it sends.
    ///
    /// A fax is half duplex, so while the training goes out there is nothing
    /// arriving to draw. The panel showed the control channel's eye instead,
    /// which reads as frequency shift keying in the middle of a burst that is
    /// nothing of the kind.
    #[test]
    fn the_sending_end_draws_the_constellation_it_is_sending() {
        let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(4)));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let mut during_training: Vec<&str> = Vec::new();
        let mut radii: Vec<f64> = Vec::new();
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for _ in 0..(FS * 40.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            if matches!(caller.phase(), Phase::Training | Phase::Sending) {
                let shape = caller.shape();
                if during_training.last() != Some(&shape) {
                    during_training.push(shape);
                }
                if let Some((x, y)) = caller.constellation_point() {
                    radii.push((x * x + y * y).sqrt());
                }
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        assert!(
            during_training.contains(&"16APM"),
            "the sending end never said it was sending V.29: {during_training:?}"
        );
        assert!(radii.len() > 10_000, "only {} points while sending", radii.len());
        // Every one of them exactly a point of Figure 1, since these are the
        // points that were sent rather than a receiver's guess at them.
        let rms = 13.5f64.sqrt();
        let figure = [2f64.sqrt(), 3.0, 18f64.sqrt(), 5.0].map(|r| r / rms);
        let off = radii
            .iter()
            .filter(|r| figure.iter().all(|f| (*r - f).abs() > 1e-9))
            .count();
        assert_eq!(off, 0, "{off} points sent were not points of the constellation");
    }


    /// A fine page goes both ways as a fine page, coded two-dimensionally.
    ///
    /// Two of these offer each other every coding and both resolutions, so
    /// this is the page as it should arrive: every line, at the resolution it
    /// was drawn at, in the smallest coding the two ends share -- JBIG with
    /// error correction, now that both have it, and Modified READ without.
    #[test]
    fn a_fine_page_arrives_fine_in_the_smallest_coding_both_ends_have() {
        for (error_correction, want) in [(true, Coding::Jbig), (false, Coding::ModifiedRead)] {
            let mut page = a_page(12);
            page.resolution = Resolution::Fine;
            let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
            let mut answerer =
                FaxCall::answer(FS, "61388880000").with_error_correction(error_correction);
            between(&mut caller, &mut answerer, 40.0);
            assert_eq!(caller.coding(), want, "sent in the wrong coding");
            assert_eq!(answerer.coding(), want, "read in the wrong coding");
            let got = answerer.received().expect("no page arrived");
            assert_eq!(got.resolution, Resolution::Fine, "it arrived as standard");
            assert_eq!(got.lines, page.lines, "the page came out different in {want:?}");
        }
    }

    /// Run a call with a burst of noise dropped onto the page the first time
    /// it goes out, and hand back every frame the answering end heard.
    fn with_a_burst_of_noise(caller: &mut FaxCall, answerer: &mut FaxCall) -> Vec<Frame> {
        let mut heard = Vec::new();
        let mut hit = false;
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut seed = 0x1234_5678u32;
        let mut started: Option<f64> = None;
        for _ in 0..(FS * 60.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            if started.is_none()
                && caller.phase() == Phase::Sending
                && caller.progress().is_some_and(|p| p > 0.3)
            {
                started = Some(caller.seconds());
            }
            let during = !hit && started.is_some_and(|t| caller.seconds() - t < 0.1);
            to_answerer = if during {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                // Loud enough to spoil every symbol it lands on.
                a + (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 2.0
            } else {
                a
            };
            if caller.phase() == Phase::EndingPage && caller.progress().is_some() {
                hit = true;
            }
            to_caller = b;
            heard.extend(answerer.take_heard().into_iter().map(|m| m.frame));
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        heard
    }

    fn a_long_page(rows: usize) -> Page {
        let width = fax::page::WIDTH;
        Page {
            lines: (0..rows)
                .map(|y| {
                    (0..width)
                        .map(|x| (x / 7 + y / 3).is_multiple_of(3) && (x * 13 + y * 7) % 11 < 6)
                        .collect()
                })
                .collect(),
            resolution: Resolution::Standard,
        }
    }

    #[test]
    fn two_of_these_use_error_correction_mode() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let mut sent = Vec::new();
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for _ in 0..(FS * 40.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            sent.extend(answerer.take_heard().into_iter().map(|m| m.frame));
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        assert!(caller.error_correction(), "the caller did not choose it");
        assert!(answerer.error_correction(), "the answerer was not told");
        assert_eq!(answerer.coding(), Coding::Jbig, "error correction and no JBIG");
        assert!(sent.contains(&Frame::Pps), "no partial page signal: {sent:?}");
        assert!(!sent.contains(&Frame::Eop), "a bare EOP under error correction");
        assert_eq!(answerer.received().expect("no page").lines, page.lines);
    }

    /// Run a call and watch the answering end's lines, handing back every
    /// count of them seen while the page was still arriving.
    fn watch_it_arrive(caller: &mut FaxCall, answerer: &mut FaxCall) -> Vec<usize> {
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut seen = Vec::new();
        for _ in 0..(FS * 60.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            if answerer.phase() == Phase::Receiving && answerer.received().is_none() {
                let lines = answerer.lines().len();
                assert_eq!(lines, answerer.lines_received());
                if seen.last() != Some(&lines) {
                    seen.push(lines);
                }
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_page_can_be_watched_arriving_with_error_correction_or_without() {
        // Without it the lines come off the decoder as the bits do. With it
        // they used to come all at once at the end, because the page was only
        // decoded once every block of it was in -- and a page that appears
        // whole a minute after it started is not one anybody can watch
        // arriving.
        for error_correction in [true, false] {
            let page = a_page(400);
            let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
            // MMR under error correction, which brings this page in 21 frames
            // and is what the count below was set for; JBIG brings it in 8,
            // and is watched arriving in the modem crate's tests of it.
            let mut answerer = FaxCall::answer(FS, "61388880000")
                .with_error_correction(error_correction)
                .with_jbig(false);
            let seen = watch_it_arrive(&mut caller, &mut answerer);
            assert_eq!(caller.error_correction(), error_correction);
            let got = answerer.received().expect("no page");
            assert_eq!(got.lines, page.lines, "error correction {error_correction}");
            assert_eq!(answerer.lines(), &page.lines[..], "the lines are not the page");
            // A page drawn as it comes is one seen at many heights on the way:
            // at 9600 a frame of 256 octets is a fifth of a second, and this
            // page is ninety of them.
            let partway = seen.iter().filter(|&&n| n > 0 && n < page.lines.len()).count();
            assert!(
                partway >= 10,
                "error correction {error_correction}: seen at {partway} heights on the way ({seen:?})"
            );
            assert!(seen.windows(2).all(|w| w[0] < w[1]), "lines went away: {seen:?}");
        }
    }

    #[test]
    fn a_burst_of_noise_costs_a_retransmission_and_not_the_page() {
        // What error correction mode is for. Without it the same burst spoils
        // lines that nobody can ask for again; with it the frames it landed on
        // are asked for, sent again, and the page arrives exactly as it left.
        let page = a_long_page(120);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let heard = with_a_burst_of_noise(&mut caller, &mut answerer);
        let pps = heard.iter().filter(|f| **f == Frame::Pps).count();
        assert!(caller.error_correction());
        assert!(pps >= 2, "the damaged block was never sent again: {heard:?}");
        let got = answerer.received().unwrap_or_else(|| panic!("no page ({:?})", answerer.trouble()));
        assert_eq!(got.lines, page.lines, "the page was not put right");
    }

    #[test]
    fn without_it_the_same_burst_spoils_the_page() {
        // The control for the test above: the same noise, the same place, and
        // error correction turned off at the answering end.
        let page = a_long_page(120);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000").with_error_correction(false);
        with_a_burst_of_noise(&mut caller, &mut answerer);
        assert!(!caller.error_correction(), "used it with a far end that has not got it");
        let got = answerer.received().map(|p| p.lines.clone()).unwrap_or_default();
        assert_ne!(got, page.lines, "the noise missed the page, so this proves nothing");
    }

    #[test]
    fn the_two_ends_learn_each_others_numbers() {
        let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(4)));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        assert_eq!(caller.identity(), "61388880000", "the CSI did not arrive");
        assert_eq!(answerer.identity(), "61399990000", "the TSI did not arrive");
    }

    #[test]
    fn a_call_with_no_page_says_so_and_hangs_up() {
        let mut caller = FaxCall::originate(FS, "61399990000", None);
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        assert_eq!(caller.phase(), Phase::Done);
        assert!(answerer.received().is_none(), "a page arrived from nowhere");
    }

    /// Three pages that differ, so one arriving in another's place shows.
    fn three_pages() -> Vec<Page> {
        (0..3)
            .map(|n| {
                let mut page = a_page(6 + 2 * n);
                for line in &mut page.lines {
                    for pel in &mut line[n * 300..n * 300 + 100] {
                        *pel = true;
                    }
                }
                page
            })
            .collect()
    }

    /// Run a call to the end, handing back the pages the answering end
    /// received with their numbers, every frame it heard, and which
    /// modulation it was listening on while the caller was part way through a
    /// page.
    fn run_pages(
        caller: &mut FaxCall,
        answerer: &mut FaxCall,
    ) -> (Vec<(usize, Page)>, Vec<Frame>, Vec<&'static str>) {
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let (mut pages, mut heard, mut listening) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..(FS * 120.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            heard.extend(answerer.take_heard().into_iter().map(|m| m.frame));
            pages.extend(answerer.take_received());
            // Early in the burst: at its end, under error correction, the
            // receiver has seen the RCP frames and gone back to control
            // before the sender's modulator has emptied.
            if caller.phase() == Phase::Sending
                && caller.progress().is_some_and(|p| (0.2..0.4).contains(&p))
            {
                let standard = answerer.standard();
                if listening.last() != Some(&standard) {
                    listening.push(standard);
                }
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        (pages, heard, listening)
    }

    #[test]
    fn several_pages_arrive_in_one_call_with_error_correction_or_without() {
        for error_correction in [false, true] {
            let sent = three_pages();
            let mut caller = FaxCall::originate_pages(FS, "61399990000", sent.clone());
            let mut answerer =
                FaxCall::answer(FS, "61388880000").with_error_correction(error_correction);
            let (got, heard, listening) = run_pages(&mut caller, &mut answerer);
            for (end, call) in [("caller", &caller), ("answerer", &answerer)] {
                assert_eq!(
                    call.phase(),
                    Phase::Done,
                    "the {end} ended at {} ({:?}), ecm {error_correction}",
                    call.phase().name(),
                    call.trouble()
                );
                assert_eq!(call.trouble(), None, "the {end}, ecm {error_correction}: {heard:?}");
            }
            assert_eq!((caller.sheet(), caller.sheets()), (3, 3));
            assert_eq!((answerer.sheet(), answerer.sheets()), (3, 3));
            let numbers: Vec<usize> = got.iter().map(|(n, _)| *n).collect();
            assert_eq!(numbers, [1, 2, 3], "ecm {error_correction}: {heard:?}");
            for ((n, page), want) in got.iter().zip(&sent) {
                assert_eq!(page.lines, want.lines, "page {n} came out different, ecm {error_correction}");
            }
            assert_eq!(answerer.pages_received(), 3);
            // What the user saw on a real call: the second page drawn as the
            // control channel's FSK, because nothing was listening for it.
            assert_eq!(listening, ["V.29"], "ecm {error_correction}");
            let count = |f: Frame| heard.iter().filter(|h| **h == f).count();
            if error_correction {
                assert!(count(Frame::Pps) >= 3, "{heard:?}");
                assert_eq!(count(Frame::Mps) + count(Frame::Eop), 0, "{heard:?}");
            } else {
                assert_eq!((count(Frame::Mps), count(Frame::Eop)), (2, 1), "{heard:?}");
            }
        }
    }
}
