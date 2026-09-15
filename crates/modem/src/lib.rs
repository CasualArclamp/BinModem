//! A modem: the whole of one, from `ATD` to samples on the line.
//!
//! Everything below this has been built and tested on its own. A data pump
//! turns bits into a waveform and back; a handshake gets two of them to agree;
//! V.42 makes the resulting bit stream reliable; an AT interpreter turns what
//! a terminal types into requests. None of that is a modem until something
//! decides when each of them applies, which is what this does.
//!
//! The states are V.250's, and there are only four that matter. In *command*
//! state the terminal is talking to the modem and what it types is parsed. In
//! *handshaking* the line is up but the two ends have not agreed anything yet,
//! and the terminal is told nothing until they do. In *data* state everything
//! the terminal types goes down the line and everything arriving comes back
//! up, and nothing is parsed at all. *Online command* state is the odd one:
//! the connection is still there but the terminal has escaped back to talking
//! to the modem, which is how a caller hangs up without dropping carrier
//! first.
//!
//! The escape is the well-known three plusses, and the reason it needs a
//! second of quiet either side is that otherwise a file containing them would
//! drop the call carrying it.

use at::escape::EscapeDetector;
use at::result::ResultCode;
use at::{Action, Interpreter};
use datapump::AsyncBits;
use datapump::bell103;
use datapump::v22bis;
use datapump::v32;
use datapump::v34;
mod faxcall;

pub use faxcall::FaxCall;

use datapump::v8 as v8line;
use v8::{CallFunction, Modulation, Modulations};
use ec::stack::Phase;
use ec::xid::Compression;
use ec::{Params, Role as EcRole, Stack};

/// Which end of the call this modem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Calling,
    Answering,
}

/// What the modem is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// On hook. The line is not connected and nothing is transmitted.
    Command,
    /// Off hook, negotiating. V.250 has the terminal hear nothing until this
    /// ends, one way or the other.
    Handshaking,
    /// Connected, and everything the terminal types goes down the line.
    Data,
    /// Connected, but the terminal has escaped back to talking to the modem.
    OnlineCommand,
}

/// Why a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The terminal asked, with `ATH`.
    LocalRequest,
    /// The terminal typed something while the call was being placed, which
    /// V.250 5.6.1 makes an instruction to give up on it.
    Aborted,
    /// The far end went away.
    CarrierLost,
    /// The handshake never completed.
    NoAnswer,
    /// The terminal asked for compression and the far end would not.
    NoCompression,
    /// The terminal asked for error control and there was none to be had.
    ///
    /// V.250 Table 20: an `<orig_fbk>` of 2 or above means "if error control
    /// not established, disconnect". A connection the DTE said it would not
    /// accept unprotected is not one to hand it anyway.
    NoErrorControl,
}

/// The line side, whichever modulation is in use.
///
/// The two are shaped alike on purpose: a sample in, a sample out, a status,
/// and bits either way. What sits above them has no business knowing which is
/// which, and the only place that decides is `+MS`.
#[derive(Debug)]
enum Pump {
    /// V.22bis: two directions in two halves of the band, 1200 or 2400 bit/s.
    V22bis(Box<v22bis::handshake::Modem>),
    /// V.32: both directions in the whole band at once, 4800 or 9600 bit/s,
    /// with the echo canceller that makes that possible.
    V32(Box<v32::startup::Modem>),
    /// Bell 103: 300 bit/s, two tones a direction, and nothing else at all.
    Bell103(Box<bell103::Modem>),
    /// V.34: the line probed and ranged in phase 2, both receivers trained
    /// and data mode's parameters exchanged in phases 3 and 4, and data after.
    V34(Box<v34::startup::Modem>),
}

impl Pump {
    fn step(&mut self, line: f64) -> f64 {
        match self {
            Self::V22bis(m) => m.step(line),
            Self::V32(m) => m.step(line),
            Self::Bell103(m) => m.step(line),
            Self::V34(m) => m.step(line),
        }
    }

    /// Whether the handshake is still going, has finished, or has given up.
    fn status(&self) -> Progress {
        match self {
            Self::V22bis(m) => match m.status() {
                v22bis::handshake::Status::Negotiating => Progress::Negotiating,
                v22bis::handshake::Status::Connected(r) => {
                    Progress::Connected(r.bits_per_second())
                }
                v22bis::handshake::Status::Failed => Progress::Failed,
            },
            Self::V32(m) => match m.status() {
                v32::startup::Status::Negotiating => Progress::Negotiating,
                v32::startup::Status::Retraining => Progress::Retraining,
                v32::startup::Status::Connected(rate) => Progress::Connected(rate),
                v32::startup::Status::Failed => Progress::Failed,
            },
            Self::Bell103(m) => match m.status() {
                bell103::Status::Negotiating => Progress::Negotiating,
                bell103::Status::Connected(rate) => Progress::Connected(rate),
                bell103::Status::Failed => Progress::Failed,
            },
            // Rates can differ each way; the one reported is what arrives,
            // which is what the terminal on this end gets to see.
            Self::V34(m) => match m.status() {
                v34::startup::Status::Running => Progress::Negotiating,
                v34::startup::Status::Connected { receive, .. } => Progress::Connected(receive),
                // A rate renegotiation keeps the call up as a V.32bis retrain
                // does; a cleardown is the far end hanging up politely.
                v34::startup::Status::Retraining => Progress::Retraining,
                v34::startup::Status::Done
                | v34::startup::Status::ClearedDown
                | v34::startup::Status::Failed(_) => Progress::Failed,
            },
        }
    }

    fn carrier(&self) -> bool {
        match self {
            Self::V22bis(m) => m.carrier(),
            // V.32's start-up measures the line rather than watching a
            // carrier detector, but once connected the receiver has one and it
            // is the only thing that will notice the far end hanging up.
            // Answering `true` here meant a V.32 call never ended: the near end
            // went back to command state and the far end sat in data for ever,
            // waiting for a carrier that had gone before it started waiting.
            Self::V32(m) => m.carrier(),
            Self::Bell103(m) => m.carrier(),
            Self::V34(m) => m.carrier(),
        }
    }

    fn take_bits(&mut self) -> Vec<bool> {
        match self {
            Self::V22bis(m) => m.take_bits(),
            Self::V32(m) => m.take_bits(),
            Self::Bell103(m) => m.take_bits(),
            Self::V34(m) => m.take_bits(),
        }
    }

    fn send_bits(&mut self, bits: &[bool]) {
        match self {
            Self::V22bis(m) => m.send_bits(bits),
            Self::V32(m) => m.send_bits(bits),
            Self::Bell103(m) => m.send_bits(bits),
            Self::V34(m) => m.send_bits(bits),
        }
    }

    /// Whether the pump has anything to send bits into right now.
    ///
    /// A V.34 retrain (11.5) takes data mode away while it runs phase 2
    /// again: the pump holds nothing and accepts nothing until it is back.
    /// Feeding one that cannot take them never ends, and every bit handed
    /// over is lost.
    fn accepts_bits(&self) -> bool {
        match self {
            Self::V34(m) => m.accepts_bits(),
            Self::V22bis(_) | Self::V32(_) | Self::Bell103(_) => true,
        }
    }

    fn pending_bits(&self) -> usize {
        match self {
            Self::V22bis(m) => m.pending_bits(),
            Self::V32(m) => m.pending_bits(),
            Self::Bell103(m) => m.pending_bits(),
            Self::V34(m) => m.pending_bits(),
        }
    }

    /// The point the receiver last decided on, where the modulation has one.
    ///
    /// Frequency shift keying does not: what it decides is which of two tones
    /// arrived, and a scope for that is an eye rather than a constellation.
    fn constellation_point(&self) -> Option<(f64, f64)> {
        match self {
            Self::V22bis(m) => Some(m.constellation_point()),
            Self::V32(m) => Some(m.constellation_point()),
            Self::V34(m) => m.constellation_point(),
            Self::Bell103(_) => None,
        }
    }

    /// Discriminator output for the modulations whose scope is that eye, where
    /// `+1` is a mark and `-1` a space.
    fn discriminator(&self) -> Option<f64> {
        match self {
            Self::Bell103(m) => Some(m.level()),
            _ => None,
        }
    }

    /// Characters the line lost, where the pump is the one that frames them.
    fn line_framing_errors(&self) -> Option<u64> {
        match self {
            Self::Bell103(m) => Some(m.framing_errors()),
            // The synchronous pumps hand up a bit stream and have no idea
            // where a character begins, so the framing is done above them and
            // the count belongs there.
            _ => None,
        }
    }

    /// The discriminator reading at the centre of each recovered bit.
    fn take_symbol(&mut self) -> Option<f64> {
        match self {
            Self::Bell103(m) => m.take_symbol(),
            _ => None,
        }
    }

    /// Mean distance from the decisions being made, which is how well the
    /// receiver is doing.
    fn residual_error(&self) -> Option<f64> {
        match self {
            Self::V22bis(m) => Some(m.residual_error()),
            Self::V32(m) => Some(m.residual_error()),
            Self::Bell103(_) | Self::V34(_) => None,
        }
    }

    /// How far the receiver is missing by, as a fraction of the distance
    /// between neighbouring points.
    ///
    /// The residual error on its own is not comparable between rates: the same
    /// number is a comfortable lock at 4800 and a receiver reading noise at
    /// 14 400, where the points are a sixth as far apart. Divided by the gap it
    /// means one thing everywhere -- half is the decision boundary, and V.32bis
    /// 7 begins a retrain well before that.
    ///
    /// Only V.32, which is the only pump here that knows its own spacing.
    fn reception(&self) -> Option<f64> {
        match self {
            Self::V32(m) => Some(m.residual_error() / m.point_spacing()),
            Self::V22bis(_) | Self::Bell103(_) | Self::V34(_) => None,
        }
    }

    /// How many states the modulation has, for a scope to size itself by.
    fn states(&self) -> usize {
        match self {
            Self::V22bis(m) => match m.status() {
                v22bis::handshake::Status::Connected(v22bis::Rate::Bps2400) => 16,
                _ => 4,
            },
            // Four during the whole start-up and at 4800. Above that it
            // depends on the rate and, at 9600, on which of the two
            // modulations the rate exchange settled on.
            Self::V32(m) => match m.status() {
                v32::startup::Status::Connected(rate) => {
                    v32::constellation_size(rate, m.coding())
                }
                _ => 4,
            },
            Self::Bell103(_) => 2,
            // Phase 3's TRN and J are four points, phase 4 is sixteen, and
            // data mode is however many hundred its rate and shaping make.
            Self::V34(m) => m.constellation_size().unwrap_or(2),
        }
    }

    /// Short name for the signal shape, as a faceplate would print it.
    fn shape(&self) -> &'static str {
        match self {
            Self::V22bis(_) | Self::V32(_) => match self.states() {
                // A trellis code carries one bit fewer than its alphabet
                // suggests; the extra one is the encoder's, so the name says
                // coded rather than a larger alphabet.
                128 => "128TCM",
                64 => "64TCM",
                32 => "32TCM",
                16 => match self {
                    // Sixteen points is either V.32 2.4.1.1's uncoded 9600 or
                    // V.32bis 2.3.4's coded 7200, and they are not the same
                    // signal at all.
                    Self::V32(m) if m.coding() == v32::Coding::Trellis => "16TCM",
                    _ => "16QAM",
                },
                _ => "4PSK",
            },
            Self::Bell103(_) => "2FSK",
            Self::V34(m) => match m.constellation_size() {
                Some(4) => "4PSK",
                Some(16) => "16QAM",
                Some(_) => "QAM",
                None => "DPSK",
            },
        }
    }

    /// The name of the modulation itself.
    fn standard(&self) -> &'static str {
        match self {
            Self::V22bis(_) => "V.22bis",
            // The two names are one modulation with two ceilings, so which of
            // them a connected call is depends on where it ended up rather
            // than on what was asked for.
            Self::V32(m) => match m.status() {
                v32::startup::Status::Connected(rate) if rate > 9600 => "V.32bis",
                _ => "V.32",
            },
            Self::Bell103(_) => "Bell 103",
            Self::V34(_) => "V.34",
        }
    }

    /// Which step of the handshake the line is on, for anything that wants to
    /// show progress or work out where one stalled.
    fn phase(&self) -> &'static str {
        match self {
            Self::V22bis(m) => m.phase(),
            Self::V32(m) => m.phase(),
            Self::Bell103(m) => m.line_phase(),
            Self::V34(m) => m.phase(),
        }
    }
}

/// What a V.34 start-up found, kept after the call it ended.
///
/// The start-up is all of V.34 there is so far, so this is the whole of what a
/// V.34 call can tell anybody: what the far end said it could do, how long the
/// line takes there and back, what the probing measured, what the two ends
/// settled on, how well each trained, and the data mode each asked for.
#[derive(Debug, Clone)]
pub struct V34Report {
    pub role: v34::phase2::Role,
    /// None if phase 2 got to the end of itself, and why not if it did not.
    pub failed: Option<&'static str>,
    pub far: Option<v34::info::Info0>,
    pub round_trip: Option<f64>,
    pub reading: Option<v34::probe::Reading>,
    pub info1c: Option<v34::info::Info1c>,
    pub info1a: Option<v34::info::Info1a>,
    /// Phases 3 and 4, if phase 2 got as far as starting them.
    pub training: Option<V34Training>,
}

/// What phases 3 and 4 came to.
#[derive(Debug, Clone)]
pub struct V34Training {
    /// Whether they got to the end, and if not where they stopped and why.
    pub done: bool,
    /// Data mode's rates in bit/s, this end's transmitter's and receiver's,
    /// once B1 has arrived.
    pub connected: Option<(u32, u32)>,
    pub stopped: Option<(&'static str, &'static str)>,
    /// The constellation each end's J asked the other to use in phase 4.
    pub far_asked: Option<v34::signals::Size>,
    pub asked: v34::signals::Size,
    /// Signal to noise this end's receiver trained to in each phase.
    pub phase3_snr: Option<f64>,
    pub phase4_snr: Option<f64>,
    /// The far end's clock against this one, in parts per million.
    pub drift_ppm: f64,
    /// Slips in the far end's signal followed: jumps a VoIP jitter buffer
    /// makes when it plays audio it made up, or drops some.
    pub slips: u32,
    pub our_mp: Option<v34::mp::Mp>,
    pub far_mp: Option<v34::mp::Mp>,
    /// This end's transmit and receive rates, as multiples of 2400.
    pub rates: Option<(u8, u8)>,
    /// Rate renegotiations and cleardowns from data mode, either end's, and
    /// whether the last was a cleardown that ended the call.
    pub renegotiations: u32,
    /// Full retrains from data mode, all the way back through phase 2 (11.5).
    pub retrains: u32,
    pub cleared_down: bool,
    /// Times data mode's frames were found again from the data: after a slip,
    /// or an E the line lost.
    pub found_again: u32,
}

impl V34Report {
    fn of(startup: &v34::startup::Modem) -> Self {
        let m = startup.phase2();
        Self {
            role: m.role(),
            failed: match m.status() {
                v34::phase2::Status::Failed(why) => Some(why),
                _ => None,
            },
            far: m.far_capabilities(),
            round_trip: m.round_trip(),
            reading: m.reading().cloned(),
            info1c: m.info1c(),
            info1a: m.info1a(),
            training: startup.training().map(|t| V34Training {
                // Data mode reached is the start-up done, whatever happened in
                // data mode afterwards.
                done: t.renegotiations() > 0
                    || matches!(
                        t.status(),
                        v34::training::Status::Done
                            | v34::training::Status::Connected { .. }
                            | v34::training::Status::ClearedDown
                    ),
                connected: match t.status() {
                    v34::training::Status::Connected { transmit, receive } => Some((transmit, receive)),
                    _ => None,
                },
                stopped: match t.status() {
                    v34::training::Status::Failed(why) => Some((t.phase(), why)),
                    _ => None,
                },
                far_asked: t.far_asked(),
                asked: t.asked(),
                phase3_snr: t.phase3_snr(),
                phase4_snr: t.phase4_snr(),
                drift_ppm: t.drift_ppm(),
                slips: t.slips(),
                our_mp: t.our_mp(),
                far_mp: t.far_mp(),
                rates: t.rates(),
                renegotiations: t.renegotiations(),
                retrains: startup.retrains(),
                cleared_down: t.status() == v34::training::Status::ClearedDown,
                found_again: t.found_again(),
            }),
        }
    }

    /// An MP sequence, as the panel says it.
    fn mp(mp: &v34::mp::Mp) -> String {
        let enabled = (1..=14).filter(|r| mp.rates >> (r - 1) & 1 == 1).count();
        format!(
            "up to {} call to answer and {} answer to call, {}-state trellis{}{}, {} rates{}, {}",
            u32::from(mp.call_to_answer) * 2400,
            u32::from(mp.answer_to_call) * 2400,
            match mp.trellis {
                v34::mp::Trellis::States16 => 16,
                v34::mp::Trellis::States32 => 32,
                v34::mp::Trellis::States64 => 64,
            },
            if mp.non_linear { ", non-linear" } else { "" },
            if mp.expanded_shaping { ", expanded shaping" } else { "" },
            enabled,
            if mp.asymmetric { " either way" } else { ", the same both ways" },
            if mp.precoding.is_some() { "precoding coefficients" } else { "no precoding" }
        )
    }

    /// One direction's settlement, as the panel says it.
    fn direction(rate: v34::info::SymbolRate, probed: v34::info::Probed) -> String {
        if probed.max_rate == 0 {
            return format!("{} symbols/s, unusable", rate.nominal());
        }
        format!(
            "{} symbols/s, {} bit/s, {} carrier, pre-emphasis {}",
            rate.nominal(),
            u32::from(probed.max_rate) * 2400,
            if probed.high_carrier { "high" } else { "low" },
            probed.pre_emphasis
        )
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![(
            "V.34 phase 2",
            match self.failed {
                None => "done".to_owned(),
                Some(why) => format!("stopped: {why}"),
            },
        )];
        if let Some(far) = self.far {
            let mut rates = vec!["2400", "3000", "3200"];
            if far.rate_2743 {
                rates.insert(1, "2743");
            }
            if far.rate_2800 {
                rates.insert(rates.len() - 2, "2800");
            }
            if far.rate_3429 {
                rates.push("3429");
            }
            rows.push((
                "V.34 far end",
                format!(
                    "{} symbols/s{}{}",
                    rates.join(" "),
                    if far.constellation_1664 { ", 1664 points" } else { "" },
                    if far.transmit_3429 { "" } else { ", 3429 not allowed" }
                ),
            ));
        }
        if let Some(rtd) = self.round_trip {
            rows.push(("round trip", format!("{:.0} ms", rtd * 1000.0)));
        }
        if let Some(reading) = self.reading.as_ref() {
            let mut snr: Vec<f64> = reading.tones.iter().map(|t| t.snr_db).collect();
            snr.sort_by(f64::total_cmp);
            let gain = |f: f64| reading.tones.iter().find(|t| t.frequency == f).map_or(0.0, |t| t.gain_db);
            rows.push((
                "probed line",
                format!(
                    "SNR {:.0} dB median ({:.0} to {:.0}), {:+.1} dB at 1050 Hz and {:+.1} at 3300{}",
                    snr[snr.len() / 2],
                    snr[0],
                    snr[snr.len() - 1],
                    gain(1050.0),
                    gain(3300.0),
                    reading
                        .frequency_offset
                        .map_or(String::new(), |hz| format!(", {hz:+.2} Hz offset"))
                ),
            ));
        }
        if let (Some(info1c), Some(info1a)) = (self.info1c, self.info1a) {
            let answer_to_call = Self::direction(
                info1a.answer_to_call,
                info1c.probed[info1a.answer_to_call.index() as usize],
            );
            let call_to_answer = Self::direction(info1a.call_to_answer, info1a.probed);
            let (towards_us, towards_them) = match self.role {
                v34::phase2::Role::Call => (answer_to_call, call_to_answer),
                v34::phase2::Role::Answer => (call_to_answer, answer_to_call),
            };
            rows.push(("V.34 to this end", towards_us));
            rows.push(("V.34 from this end", towards_them));
        }
        if let Some(t) = self.training.as_ref() {
            let points = |size: v34::signals::Size| match size {
                v34::signals::Size::Four => "4",
                v34::signals::Size::Sixteen => "16",
            };
            let db = |snr: Option<f64>| snr.map_or("not reached".to_owned(), |x| format!("{x:.0} dB"));
            rows.push((
                "V.34 phases 3 and 4",
                match (t.done, t.stopped) {
                    (true, _) if t.connected.is_some() => "done, and in data mode".to_owned(),
                    (true, _) if t.cleared_down => "done; data mode ended in a cleardown".to_owned(),
                    (true, Some((stage, why))) if t.renegotiations > 0 => {
                        format!("done; data mode then stopped at {stage}: {why}")
                    }
                    (true, _) if t.renegotiations > 0 => "done; data mode is renegotiating".to_owned(),
                    (true, _) => "done, but the two MPs left no rate to run data at".to_owned(),
                    (_, Some((stage, why))) => format!("stopped at {stage}: {why}"),
                    _ => "still going".to_owned(),
                },
            ));
            if t.renegotiations > 0 {
                rows.push((
                    "V.34 renegotiated",
                    format!("{} time{}", t.renegotiations, if t.renegotiations == 1 { "" } else { "s" }),
                ));
            }
            if t.retrains > 0 {
                rows.push((
                    "V.34 retrained",
                    format!(
                        "{} time{}, back through phase 2",
                        t.retrains,
                        if t.retrains == 1 { "" } else { "s" }
                    ),
                ));
            }
            if t.found_again > 0 {
                rows.push((
                    "V.34 frames found",
                    format!(
                        "again {} time{}, after slips or a lost E",
                        t.found_again,
                        if t.found_again == 1 { "" } else { "s" }
                    ),
                ));
            }
            rows.push((
                "V.34 trained",
                format!(
                    "phase 3 {}, phase 4 {}, far clock {:+.0} ppm, {} slip{} followed",
                    db(t.phase3_snr),
                    db(t.phase4_snr),
                    t.drift_ppm,
                    t.slips,
                    if t.slips == 1 { "" } else { "s" }
                ),
            ));
            rows.push((
                "V.34 J",
                format!(
                    "far end asked for {}, this end for {} points",
                    t.far_asked.map_or("nothing yet".to_owned(), |s| format!("{} points", points(s))),
                    points(t.asked)
                ),
            ));
            if let Some(mp) = t.far_mp.as_ref() {
                rows.push(("V.34 far MP", Self::mp(mp)));
            }
            if let Some(mp) = t.our_mp.as_ref() {
                rows.push(("V.34 our MP", Self::mp(mp)));
            }
            if let Some((transmit, receive)) = t.rates {
                rows.push((
                    "V.34 rates",
                    format!(
                        "{} bit/s to this end, {} from it",
                        u32::from(receive) * 2400,
                        u32::from(transmit) * 2400
                    ),
                ));
            }
        }
        rows
    }
}

/// How far a handshake has got, in terms neither modulation owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    Negotiating,
    /// A call that was up and is going through its start-up again, which only
    /// V.32bis 7 defines.
    Retraining,
    Connected(u32),
    Failed,
}

/// How long after dialling a character is taken as an instruction to stop.
///
/// V.250 5.6.1: "characters transmitted during the first 125 milliseconds
/// after transmission of the termination character shall be ignored (to allow
/// for the DTE to append additional control characters such as line feed after
/// the command line termination character)".
const ABORT_GUARD_MS: u32 = 125;

/// One modem.
#[derive(Debug)]
pub struct Modem {
    at: Interpreter,
    escape: EscapeDetector,
    state: State,
    fs: f64,
    /// The line side, once off hook.
    pump: Option<Pump>,
    /// The rate the handshake settled on.
    rate: u32,
    /// Whether the line is going through its start-up again (V.32bis 7).
    retraining: bool,
    /// Error control over it, once connected.
    ec: Option<Stack>,
    /// Whether error control is wanted at all. Without it the connection is
    /// still perfectly usable and simply has no protection.
    want_error_control: bool,
    role: Role,
    /// Bytes waiting to go down the line, held while the link comes up.
    outbound: Vec<u8>,
    /// Start-stop framing for a connection without error control, where
    /// nothing else says where one character ends and the next begins.
    async_bits: AsyncBits,
    /// Milliseconds since the last tick, accumulated from samples.
    elapsed_samples: f64,
    /// Milliseconds since the call was placed, for the guard in V.250 5.6.1.
    since_dial_ms: u32,
    /// The V.8 negotiation, while one is running.
    ///
    /// It comes before the data pump and instead of it. Every modem
    /// Recommendation's start-up assumes both ends already agree which one is
    /// being followed, and nothing in any of them says so; V.8 is the
    /// conversation that settles it, and it has to finish before there is a
    /// pump to build.
    negotiation: Option<v8line::Modem>,
    /// The fax call, when `+FCLASS=1` made this a fax rather than a modem.
    ///
    /// Beside the data pump rather than inside it. A fax call shares nothing
    /// with a data call: no V.8, no error control, no rate to agree, and a
    /// procedure that owns the line from the first tone to the last frame.
    fax: Option<FaxCall>,
    /// The last fax call, kept after it ends so the window can still show
    /// what the far end was.
    fax_result: Option<FaxCall>,
    /// The modulations a fax call may carry a page with.
    ///
    /// What goes in this end's DIS when it answers, and what it chooses from
    /// when it dials. Anything not built is ignored, and nothing at all comes
    /// to V.27 ter, which every fax must have.
    pub fax_offer: Vec<fax::t30::Modulation>,
    /// Whether a fax call offers T.30 Annex A's error correction mode.
    ///
    /// On unless told otherwise. It is only used when the far end offers it
    /// too, so leaving it on costs nothing against a machine without it; the
    /// reason to turn it off is to see what a page looks like without it.
    pub fax_error_correction: bool,
    /// The page waiting to be sent, taken by the next fax call that dials.
    ///
    /// Taken rather than borrowed, so that a second call does not send the
    /// first one again by accident: putting a page in is a deliberate act and
    /// so is putting the same one in twice.
    pub fax_page: Option<fax::page::Page>,
    /// What this end calls itself in a fax call, sent as a TSI.
    ///
    /// Twenty characters of digits, spaces and a plus, and blank is legal:
    /// 5.3.6.2.3 makes the identification optional and plenty of machines
    /// send nothing at all.
    pub fax_identification: String,
    /// What V.8 heard the far end say, kept after the negotiation is put away.
    far_menu: Option<v8::Menu>,
    /// What V.34's phase 2 found on the last call that ran it, kept after the
    /// call it ended.
    v34_report: Option<V34Report>,
    /// What the error control layer heard, kept after it is put away.
    ///
    /// A connection that ends up without error control drops the stack, and
    /// with it every fact about why. That is the one moment those facts are
    /// worth most: whether the far end declined, or answered something nobody
    /// has defined, or said nothing at all are three different faults, and
    /// afterwards they look identical.
    far_ec: Vec<(&'static str, String)>,
    /// Characters that arrived while error control was being asked about and
    /// turned out not to be there, waiting for the terminal.
    recovered: Vec<u8>,
    /// Whether V.8 settled on LAPM before the data carriers went up.
    declared_lapm: bool,
    /// The rate of a connection the terminal has not been told about yet.
    ///
    /// V.250 6.5.5 puts the error control report "before the final result
    /// code", so the CONNECT cannot go out until the negotiation that report
    /// describes has finished. Nothing is lost by the wait: anything typed
    /// into the gap is queued, and the far end is not listening for it yet
    /// either.
    announce: Option<u32>,
}

impl Modem {
    pub fn new(fs: f64) -> Self {
        Self {
            at: Interpreter::new(),
            escape: EscapeDetector::new(),
            state: State::Command,
            fs,
            pump: None,
            rate: 0,
            retraining: false,
            ec: None,
            want_error_control: true,
            role: Role::Calling,
            outbound: Vec::new(),
            async_bits: AsyncBits::new(8),
            elapsed_samples: 0.0,
            since_dial_ms: 0,
            negotiation: None,
            fax: None,
            fax_result: None,
            fax_offer: fax::call::OUR_MODULATIONS.to_vec(),
            fax_error_correction: true,
            fax_page: None,
            fax_identification: String::new(),
            far_menu: None,
            v34_report: None,
            far_ec: Vec::new(),
            recovered: Vec::new(),
            declared_lapm: false,
            announce: None,
        }
    }

    /// Whether to attempt V.42 on the next call.
    pub fn set_error_control(&mut self, on: bool) {
        self.want_error_control = on;
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn is_online(&self) -> bool {
        matches!(self.state, State::Data | State::OnlineCommand)
    }

    /// Put the call down, as `ATH` does, for a program driving the modem
    /// rather than a person typing at it.
    ///
    /// A person in data state has to escape to command state first, and the
    /// escape wants a second of silence either side of it. A dial-in server
    /// ending a session it has already said goodbye to has no reason to wait
    /// for that, and nothing it sends after the goodbye would be data.
    pub fn hang_up(&mut self) {
        if self.pump.is_some() || self.negotiation.is_some() || self.fax.is_some() {
            self.end_call(Ended::LocalRequest);
        }
    }

    /// The rate agreed, once there is a connection.
    pub fn rate(&self) -> Option<u32> {
        (self.rate > 0).then_some(self.rate)
    }

    /// The modulation in use, by the name `+MS` knows it as.
    pub fn modulation(&self) -> &str {
        &self.at.modulation.carrier
    }

    /// Which step of the handshake the line is on.
    pub fn line_phase(&self) -> &'static str {
        if let Some(fax) = self.fax.as_ref() {
            return fax.phase().name();
        }
        if let Some(negotiation) = self.negotiation.as_ref() {
            return negotiation.phase();
        }
        self.pump.as_ref().map_or("on hook", Pump::phase)
    }

    /// Every LAPM frame that has crossed since this was last called.
    ///
    /// Empty when there is no error control, which is the honest answer: with
    /// none there are no frames, only characters, and those the terminal
    /// already sees.
    pub fn take_frame_log(&mut self) -> Vec<ec::stack::Crossed> {
        self.ec.as_mut().map(ec::stack::Stack::take_log).unwrap_or_default()
    }

    /// The round trip the handshake measured, where it measures one.
    pub fn round_trip_symbols(&self) -> Option<u64> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => Some(m.round_trip()),
            _ => None,
        }
    }

    /// How much of its own echo the line is removing, in decibels.
    pub fn echo_return_loss(&self) -> Option<f64> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => Some(m.echo_return_loss()),
            _ => None,
        }
    }

    /// What the canceller is taking out at this instant, rather than what it
    /// managed by the end of training.
    pub fn echo_return_loss_now(&self) -> Option<f64> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => Some(m.echo_return_loss_now()),
            _ => None,
        }
    }

    /// The point the receiver last decided on, for a constellation scope.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        if let Some(fax) = self.fax.as_ref() {
            return fax.constellation_point();
        }
        self.pump.as_ref().and_then(Pump::constellation_point)
    }

    /// Discriminator output, for the modulations whose scope is an eye.
    pub fn discriminator(&self) -> Option<f64> {
        if let Some(fax) = self.fax.as_ref() {
            return fax.discriminator();
        }
        self.pump.as_ref().and_then(Pump::discriminator)
    }

    /// One discriminator reading per recovered bit, taken at the bit centre.
    pub fn take_symbol(&mut self) -> Option<f64> {
        if let Some(fax) = self.fax.as_mut() {
            return fax.take_symbol();
        }
        self.pump.as_mut().and_then(Pump::take_symbol)
    }

    /// How far the received points are sitting from the decisions made about
    /// them, which is the one number that says whether a call is healthy.
    pub fn residual_error(&self) -> Option<f64> {
        if let Some(fax) = self.fax.as_ref() {
            return fax.residual_error();
        }
        self.pump.as_ref().and_then(Pump::residual_error)
    }

    /// How far the receiver is missing by, in units of the distance between
    /// neighbouring points. See [`Pump::reception`].
    pub fn reception(&self) -> Option<f64> {
        if let Some(fax) = self.fax.as_ref() {
            return fax.reception();
        }
        self.pump.as_ref().and_then(Pump::reception)
    }

    /// How many states the modulation in use has.
    pub fn states(&self) -> usize {
        if let Some(fax) = self.fax.as_ref() {
            return fax.states();
        }
        self.pump.as_ref().map_or(2, Pump::states)
    }

    /// How far the constellation in use reaches, in the units
    /// [`Self::constellation_point`] reports.
    ///
    /// One for everything that fits the scope's box. V.32's trellis code does
    /// not: 2.4.1.2's thirty-two points are normalised by a root-mean-square
    /// of sqrt(10), and the eight with a coordinate of four reach a quarter
    /// beyond it.
    pub fn constellation_peak(&self) -> f32 {
        if let Some(fax) = self.fax.as_ref() {
            return fax.constellation_peak() as f32;
        }
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => match m.status() {
                v32::startup::Status::Connected(rate) => {
                    v32::constellation_peak(rate, m.coding()) as f32
                }
                _ => 1.0,
            },
            // A shaped constellation of several hundred points at unit mean
            // power reaches about one and a half: drawn at one, its outer
            // rings were all piled up along the edge of the box.
            Some(Pump::V34(m)) => m.constellation_peak().map_or(1.0, |peak| peak.max(1.0) as f32),
            _ => 1.0,
        }
    }

    /// Short name for the signal shape: "16QAM", "2FSK" and so on.
    pub fn shape(&self) -> &'static str {
        if let Some(fax) = self.fax.as_ref() {
            return fax.shape();
        }
        self.pump.as_ref().map_or("-", Pump::shape)
    }

    /// The modulation in use, or the one the next call will use.
    pub fn standard(&self) -> &'static str {
        if let Some(fax) = self.fax.as_ref() {
            return fax.standard();
        }
        if self.negotiation.is_some() {
            return "V.8";
        }
        match self.pump.as_ref() {
            Some(p) => p.standard(),
            None => match self.at.modulation.carrier.as_str() {
                "V34" => "V.34",
                "V32B" => "V.32bis",
                "V32" => "V.32",
                "B103" => "Bell 103",
                _ => "V.22bis",
            },
        }
    }

    /// Whether the line is off hook, which is to say there is a call on it.
    pub fn off_hook(&self) -> bool {
        // A negotiation is a call too. It is the first thing on the line after
        // the far end picks up, and a front panel that showed the lamp out
        // until a pump existed would show it out for the loudest three seconds
        // of the call.
        self.pump.is_some() || self.negotiation.is_some() || self.fax.is_some()
    }

    /// Whether the modem is a fax rather than a modem just now (V.250 6.1.10).
    ///
    /// It stays whatever it was told until it is told otherwise, which is
    /// what the recommendation asks for and is also a good way to dial a
    /// bulletin board and send it a calling tone.
    pub fn is_fax_class(&self) -> bool {
        self.at.service_class == at::ServiceClass::Fax
    }

    /// Whether the far end's carrier is present.
    pub fn carrier(&self) -> bool {
        self.pump.as_ref().is_some_and(Pump::carrier)
    }

    /// Whether this end placed the call or took it.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Where the line puts our own signal back, if the handshake went looking.
    ///
    /// Worth reporting on a real line, because it is the one number that says
    /// whether the echo canceller is pointed at anything: taps placed where
    /// the reflection is not are taps modelling nothing.
    pub fn reflection(&self) -> Option<datapump::v32::startup::Reflection> {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => m.reflection(),
            _ => None,
        }
    }

    /// Characters whose stop bit was not where it should have been.
    ///
    /// Only meaningful on a connection with no error control, which is the
    /// only kind that puts start-stop framing on the line. It is the cheapest
    /// measure of how a link is really doing, and more than that it says what
    /// *kind* of trouble it is in: errors from noise arrive evenly, a few a
    /// second, for as long as the noise lasts, while errors from a network
    /// that lost a packet arrive dozens at a time with nothing in between. The
    /// two want completely different answers and look identical in the text.
    pub fn framing_errors(&self) -> u64 {
        // Wherever the framing actually happens. Bell 103 finds characters on
        // the line itself and hands them up already framed, so asking the
        // layer above would always answer zero -- it is being handed a round
        // trip through bytes we recovered ourselves, which cannot fail.
        self.pump
            .as_ref()
            .and_then(Pump::line_framing_errors)
            .unwrap_or_else(|| self.async_bits.framing_errors())
    }

    /// Whether V.8 named LAPM before the data carriers went up.
    ///
    /// Not the same question as [`Modem::error_controlled`], which is about
    /// what is running now. This is about what both ends said they would do,
    /// at 300 bit/s, in the protocol category of V.8 Table 6.
    pub fn error_control_negotiated(&self) -> bool {
        self.declared_lapm
    }

    /// Whether error control is running on the current call.
    pub fn error_controlled(&self) -> bool {
        self.ec.as_ref().is_some_and(Stack::is_connected)
    }

    /// Everything the far end has said about itself, as label and value.
    ///
    /// Gathered from the three places it says anything: the V.8 menu, which is
    /// what it can do; the detection phase, which is whether it does error
    /// control; and XID, which is the terms. None of it reaches the terminal
    /// and all of it is the answer to why a call went the way it did.
    ///
    /// Empty before there is a call, because saying nothing is the honest
    /// report of a far end that has not spoken.
    /// What V.34's phase 2 found on the last call that ran it.
    pub fn v34_report(&self) -> Option<&V34Report> {
        self.v34_report.as_ref()
    }

    pub fn distant(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        if let Some(menu) = self.far_menu {
            // What the call is for, which the menu has always carried and this
            // panel never showed. 6.2: a far end answering a data call says
            // so, and one that thinks it is being asked for a fax says that
            // instead -- which is worth seeing before wondering why the modem
            // that answered will not talk.
            rows.push((
                "call function",
                match menu.function {
                    v8::CallFunction::Data => "data",
                    v8::CallFunction::TransmitFax => "fax, sending",
                    v8::CallFunction::ReceiveFax => "fax, receiving",
                    v8::CallFunction::Textphone => "textphone (V.18)",
                    v8::CallFunction::Videotext => "videotext (T.101)",
                    v8::CallFunction::MultimediaTerminal => "multimedia (H.324)",
                }
                .to_owned(),
            ));
            let modes: Vec<&str> = menu.modulations.iter().map(v8::Modulation::name).collect();
            rows.push((
                "modulations",
                if modes.is_empty() { "none in common".to_owned() } else { modes.join(", ") },
            ));
            rows.push((
                "V.8 protocol",
                match menu.protocol {
                    v8::Protocol::Lapm => "LAPM".to_owned(),
                    v8::Protocol::Extended => "an extension octet".to_owned(),
                    v8::Protocol::Unstated => "not stated".to_owned(),
                },
            ));
            // Table 7, and Note 1 to it: absence conveys no information about
            // the type of access, so a far end that said nothing is reported
            // as having said nothing rather than as being analogue.
            rows.push((
                "line",
                match menu.access {
                    None => "not stated".to_owned(),
                    Some(a) => {
                        let mut what = vec![if a.digital {
                            "digital network"
                        } else {
                            "analogue network"
                        }];
                        if a.call_cellular {
                            what.push("this end cellular");
                        }
                        if a.answer_cellular {
                            what.push("far end cellular");
                        }
                        what.join(", ")
                    }
                },
            ));
        }
        rows.extend(self.error_control_rows());
        if let Some(report) = self.v34_report.as_ref() {
            rows.extend(report.rows());
        }
        rows
    }

    /// What the error control layer heard the far end say.
    fn error_control_rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        if let Some(ec) = self.ec.as_ref() {
            // V.42 Table 3 and Appendix VI.1.
            rows.push((
                "answered",
                match ec.far_answer() {
                    Some(ec::detect::Answer::ErrorControl) => "EC, V.42 supported".to_owned(),
                    Some(ec::detect::Answer::None) => "E NUL, no error control".to_owned(),
                    Some(ec::detect::Answer::Extended(c)) => {
                        format!("E{}, V.42 and more", c as char)
                    }
                    Some(ec::detect::Answer::Reserved(c)) => format!("E {c:#04x}, reserved"),
                    None if ec.far_text() => "text, so no V.42 at the far end".to_owned(),
                    None => "nothing".to_owned(),
                },
            ));
            match ec.far_xid() {
                Some(xid) => {
                    if let Some(n) = xid.n401_transmit {
                        rows.push(("frame size", format!("{n} octets")));
                    }
                    if let Some(k) = xid.window_transmit {
                        rows.push(("window", format!("{k} frames")));
                    }
                    rows.push((
                        "check sequence",
                        if xid.fcs32 { "32 bit offered" } else { "16 bit" }.to_owned(),
                    ));
                    if xid.srej_single || xid.srej_multiple {
                        rows.push(("selective reject", "offered".to_owned()));
                    }
                    rows.push((
                        "compression",
                        match (xid.compression, xid.codewords, xid.max_string) {
                            (Some(c), Some(n2), Some(n7))
                                if c != ec::xid::Compression::Neither =>
                            {
                                format!("V.42bis, {n2} codewords, strings to {n7}")
                            }
                            _ => "none offered".to_owned(),
                        },
                    ));
                }
                // "none received", and the wording is the whole of it. Every
                // row in this panel is about the far end, and this one used to
                // say "none sent" -- which reads as a statement about this
                // end, and was read that way: a call with no compression on it
                // was put down to this modem not sending XID, when the frame
                // log shows it sending one six times over and getting no
                // answer. A label that can be read as the opposite of what it
                // means is worse than no label.
                None => rows.push(("XID", "none received".to_owned())),
            }
        } else {
            // The stack has been put away; what it heard was kept.
            rows.extend(self.far_ec.iter().cloned());
        }
        rows
    }

    /// Where error control has got to, as a call is happening.
    ///
    /// None of this reaches the terminal, which sees a CONNECT and whatever
    /// `+ER` and `+DR` were asked for -- and by then it is all over. On a real
    /// line the interesting part is which step did not happen, and the only
    /// way to know that is to watch it not happen.
    ///
    /// Empty when there is no call, which is not the same as no error control.
    pub fn error_control_phase(&self) -> &'static str {
        // Nothing to say until the line below has settled: the pump's own
        // handshake is reported separately, and until it finishes there is no
        // bit stream for any of this to run on.
        if self.pump.is_none() || self.rate == 0 {
            return "";
        }
        let Some(ec) = self.ec.as_ref() else {
            // Either the terminal turned it off, or the far end declined and
            // the stack has been put away. Both are calls without it.
            return "none";
        };
        match ec.phase() {
            // The ODP/ADP exchange of 7.2.1: is there a V.42 modem there.
            Phase::Detecting => "detecting",
            // XID: what the two of them can agree to do (8.10).
            Phase::Negotiating => "negotiating",
            Phase::Protocol if ec.is_connected() => "connected",
            // Which way it is going matters, and calling both of them
            // "establishing" hid a fault for a whole call: a link taken down
            // by a decoder that could not read what arrived looked exactly
            // like one coming back up, so the transcript read as a retrain
            // with nothing after it rather than as a release.
            Phase::Protocol => match ec.state() {
                ec::lapm::State::AwaitingRelease => "releasing",
                ec::lapm::State::Disconnected => "ended",
                _ => "establishing",
            },
            Phase::Transparent => "none",
        }
    }

    /// How error control is running, in the form the live view shows.
    ///
    /// The check sequence width is worth having on the screen because it is
    /// the one negotiated thing whose absence is silent: a connection with a
    /// 16-bit FCS works, and goes on working, and is letting one damaged frame
    /// in 65536 through while it does.
    pub fn error_control_detail(&self) -> &'static str {
        match self.ec.as_ref() {
            Some(ec) if ec.is_connected() => match ec.fcs() {
                ec::hdlc::Fcs::Bits32 => "V.42, FCS-32",
                ec::hdlc::Fcs::Bits16 => "V.42, FCS-16",
            },
            _ => "off",
        }
    }

    /// Bytes the terminal has handed over that are not yet on the line.
    ///
    /// The number anything streaming needs. A file transfer that hands over a
    /// megabyte because nothing stopped it has not sent a megabyte -- it has
    /// queued one, and at 9600 bit/s that is a quarter of an hour of line it
    /// cannot take back. The first time the far end asks it to go back to an
    /// earlier position, everything in that queue is already stale and still
    /// has to be sent before anything new is heard.
    pub fn queued(&self) -> usize {
        self.outbound.len() + self.ec.as_ref().map_or(0, Stack::queued)
    }

    /// Compressed streams that arrived intact and would not decode.
    ///
    /// Zero on a healthy call and not a line measurement at all: it counts the
    /// times this end and the far end disagreed about what a codeword meant,
    /// which is a fault in the compression and not in the line.
    pub fn undecodable_streams(&self) -> u64 {
        self.ec.as_ref().map_or(0, Stack::undecodable_streams)
    }

    /// Frames that arrived and did not survive the line.
    ///
    /// The difference between a link that is working and one that is only
    /// apparently working. LAPM retransmits, so a call can be delivering every
    /// byte correctly and still be losing most of what is sent -- and the
    /// terminal, which sees only the bytes, cannot tell.
    pub fn damaged_frames(&self) -> u64 {
        self.ec.as_ref().map_or(0, Stack::damaged_frames)
    }

    /// Whether V.42bis was agreed, which needs both ends to have offered it.
    pub fn compressing(&self) -> bool {
        self.ec.as_ref().is_some_and(Stack::compressing)
    }

    /// Which compression is running, if any.
    pub fn compression_name(&self) -> Option<&'static str> {
        self.ec.as_ref().and_then(Stack::compression_name)
    }

    /// Bytes for the terminal.
    pub fn take_dte(&mut self) -> Vec<u8> {
        let mut out = self.at.take_output();
        if self.state == State::Data {
            out.append(&mut self.recovered);
            match self.ec.as_mut() {
                Some(ec) => out.extend(ec.take_received()),
                None => {
                    // Without error control there are no frames, so the
                    // characters are found by their own start and stop bits.
                    if let Some(pump) = self.pump.as_mut() {
                        for bit in pump.take_bits() {
                            if let Some(c) = self.async_bits.feed(bit) {
                                out.push(c);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// One byte typed by the terminal.
    pub fn feed_dte(&mut self, byte: u8) {
        match self.state {
            State::Command | State::OnlineCommand => {
                self.at.feed(byte);
                self.run_actions();
            }
            State::Handshaking => {
                // V.250 5.6.1, and the abortability clause of the D command:
                // a single character from the terminal while a call is being
                // placed is an instruction to give up on it, and the modem
                // "disconnects from the line in an orderly manner".
                //
                // Not for the first eighth of a second, though. The character
                // that ended the command line is very often followed by a line
                // feed, and a terminal that appended one would otherwise be
                // hanging up on itself the instant it dialled.
                if self.since_dial_ms >= ABORT_GUARD_MS {
                    self.end_call(Ended::Aborted);
                }
            }
            State::Data => {
                // The escape detector sees everything, because the sequence
                // that returns to command state is made of ordinary data.
                self.escape.data(byte, &self.at.regs);
                self.outbound.push(byte);
            }
        }
    }

    /// Advance the line by one sample, returning the sample to transmit.
    pub fn step(&mut self, line: f64) -> f64 {
        self.elapsed_samples += 1.0;
        let ms = 1000.0 / self.fs;
        if self.elapsed_samples * ms >= 1.0 {
            let whole = (self.elapsed_samples * ms) as u32;
            self.elapsed_samples -= f64::from(whole) / ms;
            self.tick(whole);
        }

        if self.fax.is_some() {
            return self.carry_fax(line);
        }

        if self.negotiation.is_some() {
            return self.negotiate(line);
        }

        let Some(pump) = self.pump.as_mut() else {
            return 0.0;
        };
        let out = pump.step(line);

        match self.state {
            State::Handshaking => self.advance_handshake(),
            State::Data | State::OnlineCommand => {
                self.watch_for_retrain();
                self.carry_data();
            }
            State::Command => {}
        }
        out
    }

    /// Time passing, which drives the escape guard and V.42's timers.
    pub fn tick(&mut self, ms: u32) {
        if self.state == State::Handshaking {
            self.since_dial_ms = self.since_dial_ms.saturating_add(ms);
        }
        // V.42's clocks stop while the line is being rebuilt under them.
        //
        // On a link that is up a retrain is nothing: T401 expires on whatever
        // was in flight and it is sent again, which is what the timer is for.
        // On a link that is still being established it is fatal. N400 counts
        // attempts at a SABME rather than seconds of silence, and a retrain
        // supplies silence for as long as it takes -- so the attempts are
        // spent on a line that is not there, N400 runs out, and 7.2.1's answer
        // to a far end that will not do error control is applied to a far end
        // that was never asked.
        //
        // That is how one call ended up in start-stop characters with a far
        // end that had said LAPM in V.8 before any data carrier existed: it
        // connected, spent its second of XID hearing nothing, began protocol
        // establishment, and retrained 400 ms later. Every SABME after that
        // went into the retrain. The terminal then got every noise byte on the
        // line, which is what error control is for keeping off it.
        //
        // Nothing is lost by stopping: `drain` still runs, so anything that
        // does arrive is still delivered. Only the clocks are held.
        let retraining = self.pump.as_ref().is_some_and(|p| {
            matches!(p.status(), Progress::Retraining)
        });
        if let Some(ec) = self.ec.as_mut() {
            ec.tick(if retraining { 0 } else { ms });
        }
        if self.state == State::Data && self.escape.idle(ms, &self.at.regs) {
            // V.250 6.1.4: the sequence is only an escape if it is surrounded
            // by quiet, which is what keeps a file containing three plusses
            // from dropping the call carrying it.
            self.state = State::OnlineCommand;
            self.at.emit(ResultCode::Ok);
        }
    }

    fn advance_handshake(&mut self) {
        // The pump connecting is not the handshake ending. The detection phase
        // and the XID exchange run on top of it, and V.250 6.5.5 has the
        // terminal told what was negotiated before it is told it has connected
        // -- so until the CONNECT goes out this is still handshaking. Which is
        // also the honest answer to what a character typed into that gap
        // means: 5.6.1's instruction to give up on the call, because from the
        // terminal's side there is not yet a call.
        if self.announce.is_some() {
            self.carry_data();
            return;
        }
        // Read out and let go of it: the connected arm needs the pump back
        // mutably, to throw away what it heard while it was training.
        let Some(status) = self.pump.as_ref().map(Pump::status) else { return };
        match status {
            // A retrain cannot happen before the call is up, so during the
            // handshake it means nothing.
            Progress::Negotiating | Progress::Retraining => {}
            Progress::Connected(rate) => {
                self.rate = rate;
                if let Some(Pump::V34(m)) = self.pump.as_ref() {
                    self.v34_report = Some(V34Report::of(m));
                }
                // Everything the receiver made of the handshake is thrown
                // away. A demodulator that has not finished training still
                // hands up bits, and by the time it has there are thousands
                // of them waiting -- all of them noise, and all of them about
                // to be handed to a detection phase that has just this moment
                // been created.
                //
                // V.42 7.2.1.2 starts that phase "when circuits RFS and RSD go
                // ON, indicating a successful connection between the signal
                // converters", which is now. What came before is not part of
                // it, and reading it as though it were is how a real call
                // found the pattern for "no error control" in its own training
                // garbage and went transparent against a modem that was
                // establishing LAPM.
                if let Some(pump) = self.pump.as_mut() {
                    pump.take_bits();
                }
                // Bell 103 is asynchronous all the way down: its line format
                // *is* start-stop framing, and its receiver finds the frames
                // by re-synchronising on each start bit rather than by holding
                // a bit clock. V.42 wants a synchronous bit pipe underneath it
                // and would hand this one HDLC, which the framer would take
                // apart into characters that were never there. So error
                // control is off at 300 bit/s -- which is also how anyone ever
                // dialled a board at 300 bit/s.
                let framed = matches!(self.pump, Some(Pump::Bell103(_)));
                // An answering modem with error control turned off still owes
                // the caller an answer. 7.2.1.3 requires the answerer, on
                // seeing the ODP, to "immediately send one of the Answerer
                // Detection Patterns defined in Table 3 at least ten times",
                // and Table 3 has one for exactly this: `E` and NUL, "no
                // error-correcting protocol desired".
                //
                // Saying nothing works, because the caller times out. It costs
                // it three quarters of a second, and it costs anyone looking
                // at why a call has no error control the difference between a
                // far end that declined and one that was not listening.
                let answering = self.role == Role::Answering;
                if (self.want_error_control || answering) && !framed {
                    let role = match self.role {
                        Role::Calling => EcRole::Originator,
                        Role::Answering => EcRole::Answerer,
                    };
                    // The timer that decides how long a silence is worth
                    // waiting through, sized to the rate the silence is on
                    // (V.42 Appendix IV).
                    let params =
                        Params { t401_ms: ec::lapm::t401_for(rate), ..Params::default() };
                    let mut stack = Stack::new(role, params);
                    if !self.want_error_control {
                        stack = stack.declining();
                    }
                    if self.declared_lapm {
                        stack = stack.declared_lapm();
                    }
                    // V.250 Table 20, `<orig_rqst>` of 2: "initiate V.42
                    // without Detection Phase. If ITU-T Rec. V.8 is in use,
                    // this is a request to disable V.42 Detection Phase".
                    if !self.at.error_control.detect() {
                        stack = stack.without_detection();
                    }
                    // Offer compression in both directions and let the far end
                    // decide. What runs is the intersection, so offering more
                    // than the far end can do costs nothing.
                    let c = self.at.compression;
                    if c.wanted() {
                        stack.offer_compression(Compression::Both);
                        // V.250 Table 27: `<max_dict>` and `<max_string>` are
                        // the terminal's ceilings on V.42bis P1 and P2, "based
                        // on its knowledge of the nature of the data to be
                        // transmitted". 6.4 then takes the lower of the two
                        // ends' proposals, so these are ceilings twice over.
                        stack.offer_dictionary(c.max_dict, c.max_string);
                    }
                    self.ec = Some(stack);
                }
                // Held rather than sent. What goes out first is the report
                // of what was negotiated, and that is not known yet.
                self.announce = Some(rate);
                self.announce_connect();
            }
            Progress::Failed => {
                if let Some(Pump::V34(m)) = self.pump.as_ref() {
                    // Not a call that never answered: one that answered, got
                    // through as much of V.34 as there is, and stopped. What it
                    // learned is the point of having placed it.
                    self.v34_report = Some(V34Report::of(m));
                    self.end_call(Ended::CarrierLost);
                } else {
                    self.end_call(Ended::NoAnswer);
                }
            }
        }
    }

    /// Tell the terminal the call is up, once there is nothing left to say
    /// about it.
    ///
    /// V.250 6.5.5: the `+ER` report is issued "at the point during error
    /// control negotiation (handshaking) at which the DCE has determined which
    /// error control protocol will be used (if any), before the final result
    /// code (e.g., CONNECT) is transmitted", and 6.6.3 puts `+DR` between the
    /// two. So the order is fixed and the CONNECT is last, which means it
    /// cannot go out while the answer is still being worked out.
    fn announce_connect(&mut self) {
        let Some(rate) = self.announce else { return };
        // No stack at all is an answer: this is a call without error control,
        // and there is nothing to wait for.
        if self.ec.as_ref().is_some_and(|e| !e.settled()) {
            return;
        }
        // V.250 Table 20 again, from the other end of the same setting: an
        // `<orig_fbk>` of 2 or above requires error control, and a connection
        // without it is one the terminal has already said it does not want.
        if self.at.error_control.required() && !self.error_controlled() {
            self.announce = None;
            self.end_call(Ended::NoErrorControl);
            return;
        }
        // And V.250 Table 27's <compression_negotiation> of 1: "disconnect if
        // ITU-T Rec. V.42 bis is not negotiated by the remote DCE as specified
        // in <direction>".
        if self.at.compression.required && !self.compressing() {
            self.announce = None;
            self.end_call(Ended::NoCompression);
            return;
        }
        self.announce = None;
        self.state = State::Data;
        self.escape.reset();

        // Table 24/V.250. `ALT` is for the alternative protocol of Annex A,
        // which this modem does not do, so the report is between two.
        if self.at.config.report_error_control {
            let kind = if self.error_controlled() { "LAPM" } else { "NONE" };
            self.at.emit(ResultCode::Extended(format!("+ER: {kind}")));
        }
        // Table 29/V.250. V.42bis is negotiated as a pair here -- both
        // directions or neither -- so the one-directional reports cannot
        // arise.
        if self.at.config.report_compression {
            let kind = if self.compressing() { "V42B" } else { "NONE" };
            self.at.emit(ResultCode::Extended(format!("+DR: {kind}")));
        }
        // V.250 6.2.7: with X at 1 or above the CONNECT carries the rate,
        // which is the only way a terminal finds out what it got rather than
        // what it asked for.
        let code = if self.at.config.x == 0 {
            ResultCode::Connect
        } else {
            ResultCode::ConnectText(format!("{rate}"))
        };
        self.at.emit(code);
    }

    /// Follow a retrain, if the line has started one (V.32bis 7).
    ///
    /// The call stays up throughout: 7.3 keeps circuit 107 ON and 109 ON, and
    /// what is clamped is the received data. So the terminal is told nothing
    /// and sees nothing -- no second CONNECT, no NO CARRIER -- and the only
    /// thing that changes underneath is the rate, which is very often the
    /// point of the exercise.
    ///
    /// The error control above notices anyway, in the only way it can: V.42's
    /// T401 expires on whatever was in flight and it is sent again. That is
    /// what the timer is for, and a retrain is exactly the sort of gap it was
    /// written against.
    fn watch_for_retrain(&mut self) {
        let Some(status) = self.pump.as_ref().map(Pump::status) else { return };
        match status {
            Progress::Retraining => self.retraining = true,
            Progress::Connected(rate) if self.retraining => {
                self.retraining = false;
                self.rate = rate;
                // A V.34 renegotiation settles new MPs and new rates.
                if let Some(Pump::V34(m)) = self.pump.as_ref() {
                    self.v34_report = Some(V34Report::of(m));
                }
            }
            // A retrain that never finishes is a call that has ended, whatever
            // the line is still carrying.
            Progress::Failed if self.retraining => {
                if let Some(Pump::V34(m)) = self.pump.as_ref() {
                    self.v34_report = Some(V34Report::of(m));
                }
                self.end_call(Ended::CarrierLost);
            }
            _ => {}
        }
    }

    /// Whether the line is retraining right now.
    pub fn retraining(&self) -> bool {
        self.retraining
    }

    /// Ask the data pump to go back through its start-up (V.32bis 7).
    ///
    /// 7 begins a retrain "if either modem incorporates a means of detecting
    /// unsatisfactory signal reception", and this modem has one; this is the
    /// same door from outside, for a test that wants a retrain without having
    /// to build a line bad enough to earn one. Nothing on a real call calls
    /// it. Ignored by the modulations that have no such procedure.
    ///
    /// V.34 has a rate renegotiation for this (11.6), and that is what it is
    /// asked for: two steps below the rate arriving now.
    pub fn ask_for_retrain(&mut self) {
        let arriving = self.rate;
        match self.pump.as_mut() {
            Some(Pump::V32(m)) => m.ask_for_retrain(),
            Some(Pump::V34(m)) => {
                m.renegotiate(((arriving / 2400) as u8).saturating_sub(2).max(1));
            }
            _ => {}
        }
    }

    /// Retrain the line the whole way (V.34 11.5): back through phase 2 and
    /// train again, on the same call. For V.34 this is the full retrain, not
    /// the in-band rate renegotiation [`Self::ask_for_retrain`] does; other
    /// modulations fall back to whatever retrain they have. Does nothing
    /// outside data mode.
    pub fn retrain(&mut self) {
        match self.pump.as_mut() {
            Some(Pump::V34(m)) => {
                m.retrain();
            }
            Some(Pump::V32(m)) => m.ask_for_retrain(),
            _ => {}
        }
    }

    /// How many times this call has retrained.
    ///
    /// One is a line that changed. A handful is a line that cannot hold what
    /// the two ends keep agreeing on, and is worth seeing on the panel.
    pub fn retrains(&self) -> u32 {
        match self.pump.as_ref() {
            Some(Pump::V32(m)) => m.retrains(),
            Some(Pump::V34(m)) => m.renegotiations() + m.retrains(),
            _ => 0,
        }
    }

    fn carry_data(&mut self) {
        let Some(pump) = self.pump.as_ref() else { return };
        // A carrier that has gone is only news when one was supposed to be
        // there, and through a retrain there are stretches where one is not.
        // The training segment of 5.2.3 is the plainest: each end sends its
        // own while the other is required to be silent, so for a second or
        // more the line carries nothing but this modem's own reflection.
        //
        // On a line that reflects almost nothing that is indistinguishable
        // from a far end that has hung up, and this read it as one. It ended
        // the call in the middle of the retrain it had itself asked for --
        // dropped the pump, stopped transmitting, and left the far end
        // listening to silence for a signal that was never going to come.
        //
        // Asked of the pump rather than of the flag `watch_for_retrain` keeps,
        // so that it cannot be a step behind on the one step where it matters.
        // The retrain has an ending of its own: the start-up gives up on it
        // and reports `Failed`, which becomes this same NO CARRIER a moment
        // later and by a route that knows what it is doing.
        let retraining = matches!(pump.status(), Progress::Retraining);
        if !pump.carrier() && !retraining {
            self.end_call(Ended::CarrierLost);
            return;
        }
        // A far end that does not do error control is a perfectly ordinary far
        // end, and V.42 7.2.1 exists to find that out rather than to fail on
        // it. Once the detection phase has said so there is nothing for the
        // stack to do, and the characters go down the line as they are.
        if self.ec.as_ref().is_some_and(|e| e.phase() == Phase::Transparent) {
            // Take what it knew before it goes. This is the moment somebody
            // will want to know why there is no error control, and it is the
            // moment the answer would otherwise be thrown away.
            self.far_ec = self.error_control_rows();
            // And what the line brought while the question was being asked,
            // for the terminal: a far end without V.42 has usually said
            // something already, a banner or a prompt, by the time this end
            // stops listening for a pattern.
            if let Some(mut ec) = self.ec.take() {
                for bit in ec.take_unclaimed() {
                    if let Some(c) = self.async_bits.feed(bit) {
                        self.recovered.push(c);
                    }
                }
            }
        }
        self.announce_connect();

        let Some(pump) = self.pump.as_mut() else { return };
        match self.ec.as_mut() {
            Some(ec) => {
                // Error control owns the bit stream in both directions: it
                // frames what goes out and unframes what comes back, and the
                // line is never idle because a synchronous link always carries
                // something.
                for bit in pump.take_bits() {
                    ec.feed_bit(bit);
                }
                if !self.outbound.is_empty() && ec.is_connected() {
                    let queued = std::mem::take(&mut self.outbound);
                    ec.send(&queued);
                }
                // Keep the transmitter fed. Running it dry would put the
                // pump's own idle pattern on the line in the middle of a
                // frame, which the far end would read as an abort.
                //
                // Only while there is a transmitter to feed. A V.34 retrain
                // takes data mode away for the seconds phase 2 and the
                // training after it need, and a pump that cannot hold a bit
                // never fills: this asked for one for ever, and threw away
                // every frame the error control handed over on the way. The
                // frames wait instead, and go when data mode is back.
                while pump.accepts_bits() && pump.pending_bits() < 64 {
                    let bit = ec.next_bit();
                    pump.send_bits(&[bit]);
                }
            }
            None => {
                // Each character wrapped in a start and a stop bit (V.14), so
                // that the far end can find where it begins. A synchronous
                // line carries bits whether or not anything is sending, and
                // nothing else in an unprotected connection marks the
                // boundaries.
                if !self.outbound.is_empty() {
                    let queued = std::mem::take(&mut self.outbound);
                    let mut bits = Vec::new();
                    for byte in queued {
                        bits.extend(self.async_bits.encode(byte));
                    }
                    pump.send_bits(&bits);
                }
            }
        }
    }

    fn run_actions(&mut self) {
        for action in self.at.take_actions() {
            match action {
                Action::Dial(_) => self.place_call(Role::Calling),
                Action::Answer => self.place_call(Role::Answering),
                Action::HangUp => {
                    if self.pump.is_some() || self.fax.is_some() {
                        self.end_call(Ended::LocalRequest);
                    } else {
                        self.at.emit(ResultCode::Ok);
                    }
                }
                Action::OffHook => self.at.emit(ResultCode::Ok),
                Action::ReturnOnline => {
                    if self.state == State::OnlineCommand {
                        self.state = State::Data;
                        self.escape.reset();
                        self.at.emit(ResultCode::Connect);
                    } else {
                        // V.250 6.3.7: there is nothing to return to.
                        self.at.emit(ResultCode::Error);
                    }
                }
                Action::SelectServiceClass(_) => {
                    // A change of class is a change of what this machine is,
                    // so nothing from before it survives. Anything on the
                    // line goes, because a fax call and a data call have no
                    // state in common to carry across.
                    if self.pump.is_some() || self.fax.is_some() {
                        self.end_call(Ended::LocalRequest);
                    } else {
                        self.at.emit(ResultCode::Ok);
                    }
                }
                Action::SelectModulation(_) | Action::SelectCompression(_) => {
                    // Both take effect on the next call, so there is nothing
                    // to do now beyond acknowledging: the interpreter has
                    // already recorded what was asked for.
                    self.at.emit(ResultCode::Ok);
                }
                Action::SelectErrorControl(e) => {
                    self.want_error_control = e.wanted();
                    self.at.emit(ResultCode::Ok);
                }
                Action::ResetProfile(_) | Action::FactoryDefaults(_) => {
                    if self.pump.is_some() {
                        self.end_call(Ended::LocalRequest);
                    }
                    self.at.emit(ResultCode::Ok);
                }
            }
        }
    }

    fn place_call(&mut self, role: Role) {
        self.role = role;
        // A fax call goes nowhere near any of this. There is no modulation to
        // select, nothing to negotiate, and V.8 in particular must not run:
        // a group 3 fax has never heard of it, and answers the 2100 Hz tone
        // of T.30 rather than the one V.8 puts reversals in.
        if self.at.service_class == at::ServiceClass::Fax {
            self.since_dial_ms = 0;
            self.rate = 0;
            self.ec = None;
            self.pump = None;
            self.negotiation = None;
            self.announce = None;
            self.state = State::Handshaking;
            // The end that dialled sends; the end that answered receives.
            // T.30 has no way to swap those round on an ordinary call, and
            // nothing here wants one: a page goes out of the machine whose
            // operator put it in and asked for a number.
            let call = match role {
                Role::Calling => {
                    FaxCall::originate(self.fs, &self.fax_identification, self.fax_page.take())
                }
                Role::Answering => FaxCall::answer(self.fs, &self.fax_identification),
            };
            self.fax = Some(
                call.offering(&self.fax_offer)
                    .with_error_correction(self.fax_error_correction),
            );
            return;
        }
        self.since_dial_ms = 0;
        self.rate = 0;
        self.ec = None;
        self.far_menu = None;
        self.v34_report = None;
        self.far_ec.clear();
        self.recovered.clear();
        self.declared_lapm = false;
        self.announce = None;
        self.outbound.clear();
        self.async_bits.reset();
        self.state = State::Handshaking;

        // V.250 6.4.1's automode: the modem "may fall back to another
        // modulation on its own". V.8 is how two modems do that on purpose
        // rather than by each guessing and hoping, so that is what automode
        // means here. With it off, the modulation named is the modulation
        // used and there is nothing to negotiate.
        let offered = self.offered();
        if self.at.modulation.automode && !offered.is_empty() {
            let role = match role {
                Role::Calling => v8line::Role::Calling,
                Role::Answering => v8line::Role::Answering,
            };
            self.pump = None;
            let mut negotiation =
                v8line::Modem::new(role, CallFunction::Data, offered, self.fs);
            // V.8 Table 6 has an octet for error control, and 7.3 says it is
            // there "in order to negotiate LAPM without requiring the ODP/ADP
            // exchange". Asking costs one octet in a sequence already being
            // sent, and what comes back is a second opinion on the question
            // the detection phase is about to ask over a much worse channel.
            if self.want_error_control {
                negotiation = negotiation.offering_lapm();
            }
            self.negotiation = Some(negotiation);
            return;
        }
        self.start_pump(None);
    }

    /// What to put in a call menu.
    ///
    /// Only what this modem can actually demodulate, and only inside the range
    /// `+MS` asked for -- offering a modulation and then failing to hold it is
    /// worse than never offering it, and the whole value of the exchange is
    /// that what comes back can be believed.
    ///
    /// Bell 103 is not in the list and cannot be: Table 4 of V.8 is a table of
    /// V-series modulations and Bell 103 is not one of them. A modem told to
    /// use it is therefore told something V.8 has no way to express, and the
    /// honest answer is not to negotiate at all.
    fn offered(&self) -> Modulations {
        let (lowest, highest) = self.rate_range();
        let settings = &self.at.modulation;
        let Some(preferred) = (match settings.carrier.as_str() {
            // One bit in V.8's menu covers both (Table 4: "V.32 bis/V.32
            // availability"), so the two carriers offer the same thing and
            // differ only in what they will then agree to.
            "V32" | "V32B" => Some(Modulation::V32bis),
            // Offered first, and V.32bis and V.22bis beside it below: a far end
            // without V.34 picks one of those, and the call goes ahead on it.
            "V34" => Some(Modulation::V34Duplex),
            "B103" => None,
            _ => Some(Modulation::V22bis),
        }) else {
            return Modulations::NONE;
        };
        let mut offered = Modulations::NONE;
        offered.insert(preferred);
        // Everything else this modem has, within the rates asked for. V.32
        // starts at 4800 and V.22bis spans 1200 to 2400, so a ceiling of 1200
        // rules the first out entirely rather than merely discouraging it.
        if highest >= 4800 {
            offered.insert(Modulation::V32bis);
        }
        if highest >= 1200 && lowest <= 2400 {
            offered.insert(Modulation::V22bis);
        }
        offered
    }

    /// The rate range `+MS` asked for, with V.250's "unspecified" resolved.
    ///
    /// 6.4.1 makes zero mean no limit rather than a rate of nothing, so every
    /// comparison wants it turned into the limit it stands for first.
    fn rate_range(&self) -> (u32, u32) {
        let settings = &self.at.modulation;
        let highest = if settings.max_rate == 0 { u32::MAX } else { settings.max_rate };
        (settings.min_rate, highest)
    }

    /// Carry a fax call one sample further.
    ///
    /// Every phase of one: the tones, the capabilities over 300 bit/s, the
    /// training check and the page over V.27 ter, and the receipt and the
    /// disconnect back on 300 again. Which of those is on the line at any
    /// moment is [`FaxCall`]'s business; this only has to notice when there
    /// is no longer a call.
    fn carry_fax(&mut self, line: f64) -> f64 {
        let Some(fax) = self.fax.as_mut() else { return 0.0 };
        let out = fax.step(line);
        match fax.phase() {
            fax::call::Phase::Done => {
                // Whatever was learned is kept: the window wants to show it
                // after the call as much as during it.
                self.fax_result = self.fax.take();
                self.state = State::Command;
                self.at.emit(ResultCode::Ok);
            }
            fax::call::Phase::Failed => {
                self.fax_result = self.fax.take();
                self.state = State::Command;
                self.at.emit(ResultCode::NoAnswer);
            }
            _ => {}
        }
        out
    }

    /// What the far end of a fax call said, during or after it.
    pub fn fax_call(&self) -> Option<&FaxCall> {
        self.fax.as_ref().or(self.fax_result.as_ref())
    }

    /// A page a fax call received, handed over once and then gone.
    pub fn take_received_page(&mut self) -> Option<fax::page::Page> {
        self.fax
            .as_mut()
            .and_then(FaxCall::take_received)
            .or_else(|| self.fax_result.as_mut().and_then(FaxCall::take_received))
    }

    /// Carry the negotiation one sample further, and build the pump when it
    /// has decided.
    fn negotiate(&mut self, line: f64) -> f64 {
        let negotiation = self.negotiation.as_mut().expect("checked by the caller");
        let out = negotiation.step(line);
        match negotiation.status() {
            v8line::Status::Negotiating => {}
            v8line::Status::Agreed(modulation) => {
                self.declared_lapm = negotiation.lapm();
                self.far_menu = negotiation.far_menu();
                self.negotiation = None;
                self.start_pump(Some(modulation));
            }
            // 8.1.1: a far end that sent the plain answering tone of V.25 does
            // not speak V.8, and the call goes on "in accordance with Annex
            // A/V.32 bis, ITU-T T.30, or other appropriate Recommendations" --
            // which here means the modulation +MS named, exactly as before any
            // of this existed.
            v8line::Status::NoNegotiation => {
                self.negotiation = None;
                self.start_pump(None);
            }
            // Nothing in common, or nothing heard. Both ends know, which is
            // the difference between this and a minute of silence.
            v8line::Status::Failed => {
                self.negotiation = None;
                self.end_call(Ended::NoAnswer);
            }
        }
        out
    }

    /// Build the data pump and let its own start-up begin.
    ///
    /// `chosen` is what V.8 agreed, where it ran. Without one the modulation
    /// is whichever `+MS` named.
    fn start_pump(&mut self, chosen: Option<Modulation>) {
        let role = self.role;
        let carrier = match chosen {
            Some(Modulation::V34Duplex) => "V34".to_owned(),
            // V.8 cannot tell the two apart, so what it agreed does not
            // change which of them was asked for. Anything else -- a far end
            // that offered it when this end had chosen V.22bis, say -- takes
            // the faster carrier, since there is no reason to hold back.
            Some(Modulation::V32bis) => match self.at.modulation.carrier.as_str() {
                chosen @ ("V32" | "V32B") => chosen.to_owned(),
                _ => "V32B".to_owned(),
            },
            Some(Modulation::V22bis) => "V22B".to_owned(),
            // Nothing else is ever offered, so nothing else can come back.
            // Except that V.34 without V.8 is not V.34: 11.1.1.3 sends a call
            // whose far end answered with a plain ANS on to V.32bis's own
            // start-up, and so does a V.8 that was turned off.
            _ => match self.at.modulation.carrier.as_str() {
                "V34" => "V32B".to_owned(),
                other => other.to_owned(),
            },
        };
        self.pump = Some(match carrier.as_str() {
            "V34" => {
                let role = match role {
                    Role::Calling => v34::phase2::Role::Call,
                    Role::Answering => v34::phase2::Role::Answer,
                };
                Pump::V34(Box::new(v34::startup::Modem::new(role, self.fs)))
            }
            "V32" | "V32B" => {
                let hs_role = match role {
                    Role::Calling => v32::startup::Role::Calling,
                    Role::Answering => v32::startup::Role::Answering,
                };
                // Offer what this receiver can actually demodulate, and no
                // more: offering a rate and then failing to read it is worse
                // than never offering it. Both of these are read here --
                // 4800 by 2.4.2 and 9600 by the nonredundant coding of
                // 2.4.1.1, which is the alternative every V.32 modem is
                // required to be able to fall back on. Trellis coding is not,
                // so B8 of the rate signal stays clear.
                //
                // And only within the range +MS allows. <max_rate> is "the
                // highest value at which the DCE may establish a connection",
                // which is not advice: a modem that offers 9600 to a terminal
                // that asked for at most 4800 will get 9600, because the far
                // end has no way to know it was not meant.
                let (lowest, highest) = self.rate_range();
                // V.250's two carriers are two different ceilings. V.32 stops
                // at 9600 and V.32bis goes to 14 400, and a terminal that
                // asked for the first is not to be given the second -- which
                // is also how to make this modem interwork with a far end that
                // says V.32bis and cannot hold it.
                let ceiling = if carrier == "V32" { 9600 } else { 14_400 };
                let offer = v32::startup::rate_signal(
                    v32::startup::Rates::between(lowest, highest.min(ceiling)),
                );
                Pump::V32(Box::new(v32::startup::Modem::new(hs_role, offer, self.fs)))
            }
            "B103" => {
                let hs_role = match role {
                    Role::Calling => bell103::Role::Originate,
                    Role::Answering => bell103::Role::Answer,
                };
                Pump::Bell103(Box::new(bell103::Modem::new(hs_role, self.fs)))
            }
            _ => {
                let hs_role = match role {
                    Role::Calling => v22bis::handshake::Role::Calling,
                    Role::Answering => v22bis::handshake::Role::Answering,
                };
                // +MS carries a maximum rate and it is not decoration. The
                // sixteen points of 2400 need about 20 dB of signal to noise
                // to be told apart and the four of 1200 need about 13, so on
                // a line that cannot give the first, 2400 is not the faster
                // connection but the one that carries nothing.
                let ceiling = if self.rate_range().1 >= 2400 {
                    v22bis::Rate::Bps2400
                } else {
                    v22bis::Rate::Bps1200
                };
                Pump::V22bis(Box::new(v22bis::handshake::Modem::at_most(
                    hs_role, ceiling, self.fs,
                )))
            }
        });
        self.state = State::Handshaking;
    }

    fn end_call(&mut self, why: Ended) {
        // A fax call is a call too, and putting the line down has to stop it
        // -- otherwise the only way out of a fax that has gone wrong is to
        // close the program. What it learned is kept for the window.
        if self.fax.is_some() {
            self.fax_result = self.fax.take();
        }
        self.pump = None;
        self.negotiation = None;
        self.rate = 0;
        self.ec = None;
        // A call that ends before its CONNECT went out never connected, and
        // the terminal is about to be told why instead.
        self.announce = None;
        self.outbound.clear();
        self.escape.reset();
        self.state = State::Command;
        self.at.emit(match why {
            Ended::LocalRequest => ResultCode::Ok,
            // 6.3.1: what a dial that did not get there reports.
            Ended::Aborted => ResultCode::NoCarrier,
            Ended::CarrierLost | Ended::NoErrorControl | Ended::NoCompression => {
                ResultCode::NoCarrier
            }
            Ended::NoAnswer => ResultCode::NoAnswer,
        });
    }
}

/// Scale a recording so that it fits, and report what it was scaled by.
///
/// A modem's output is not bounded by one. The pulse shaping sums the tails of
/// several symbols, so the peak runs well above the average, and two modems on
/// one pair sum again on top of that: a V.22bis call between two of these
/// reaches about one and a half. Written to a sixteen-bit file as it stands,
/// every one of those peaks comes back clipped, and every measurement made
/// from the file afterwards is of something else.
///
/// The level is not information. A real line delivers whatever it delivers,
/// which is why a receiver has gain control at all, so scaling a recording to
/// fit loses nothing that was in it.
pub fn fit_to_scale(samples: &mut [f32], target: f32) -> f32 {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak <= 0.0 {
        return 1.0;
    }
    let gain = target / peak;
    if gain >= 1.0 {
        return 1.0;
    }
    for s in samples.iter_mut() {
        *s *= gain;
    }
    gain
}
