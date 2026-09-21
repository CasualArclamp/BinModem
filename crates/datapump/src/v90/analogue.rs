//! The analogue modem from phase 3 on (9.3.2, 9.4.2): V.34 going up, PCM
//! coming down.
//!
//! ```text
//! analogue S S' PP TRN Ja ...          (quiet)       S ... S S'  (quiet)   S S' CPt ... CP CP' E B1 data
//! digital                 Sd S'd TRN1d Jd ... Jd J'd DIL ... ... DIL Ri ... R'i TRN2d MP MP' Ed B1d data
//! ```
//!
//! A rate renegotiation (9.6) goes back to phase 4 from data mode, the
//! frames kept in step throughout: Rd and its turn to R-bar-d from the
//! digital modem, S, S-bar and CP from this end, and phase 4 from there.
//!
//! Everything upstream is V.34's, at the symbol rate, carrier and
//! pre-emphasis phase 2 settled: S, S-bar and PP as phase 3 sends them, TRN
//! at four points, Ja in J's modulation (8.3.1), CP, SCR and E as MP goes
//! (8.5.2), and then V.34's data mode as the digital modem's MP asks. What
//! comes down is read by [`super::pcm`], and what it means is worked out
//! here: Jd and J'd from the signs, the route from the DIL, R from its sign
//! pattern, and TRN2d, MP, Ed, B1d and data from whole data frames. What CP
//! asks for is the DIL's to say, and whether to ask for spectral shaping is
//! what the equaliser left of TRN1d and Jd has to say ([`super::shaping`]).

use std::collections::VecDeque;

use dsp::Complex;

use crate::v32::{Mode, Scrambler};
use crate::v34::constellation::Point;
use crate::v34::data::{Encoder as UpstreamEncoder, Params};
use crate::v34::frame::Framing;
use crate::v34::info::{Info0d, Info1aPcm, Info1c};
use crate::v34::mp::{Finder, Found, Mp, Trellis};
use crate::v34::qam::{Band, Transmitter};
use crate::v34::receiver;
use crate::v34::phase2::Role;
use crate::v34::signals::{self, Sender, Size};
use crate::v34::training::RetrainWatch;
use crate::v34::trellis::Code;

use super::INTERVALS;
use super::dil::{self, Analysis, Choice, Route};
use super::digital;
use super::encoder::{Decoder, Frame, Mapping};
use super::pcm::{self, Heard, Slicer};
use super::sequences::{self, Cp, Descriptor, JD_BITS, JD_PRIME_BITS, Jd};
use super::shaping::{self, Shaping};
use super::ucode::{self, Law};

/// TRN in phase 3: "at least 512T" (9.3.2.3), and a far receiver trains
/// better on more, as V.34's own phase 3 has found.
const PHASE3_TRN: f64 = 1.0;

/// "70 +- 5 ms" of silence after INFO1a (9.3.2.1).
const SILENCE_BEFORE_S: f64 = 0.070;

/// Sd: "within 1500 ms from the start of Ja" (9.3.2.4), which a digital
/// modem that "may wait for up to 500 ms" after reading Ja (9.3.1.3) cannot
/// meet across a VoIP call's second-long round trip. A live server's came
/// two seconds after Ja began; waiting is cheaper than retraining.
const SD_WAIT: f64 = 2.0;

/// Jd: "Within 4000 ms of starting to transmit TRN1d the digital modem shall
/// transmit Jd" (9.3.1.4), and S is wanted back "within 5100 ms plus a
/// round-trip delay from the start of TRN1d" (9.3.1.5). A live server sent
/// Jd at the last moment and gave up on S a second later, round trip or
/// none: one that takes a second to cross could not answer Jd in time. So
/// S goes before Jd arrives, to reach the digital modem this long after the
/// latest it can have begun Jd -- S goes on until J'd, and a digital modem
/// not yet listening for it hears it when it is.
const JD_LATEST: f64 = 4.0;
const S_AFTER_JD: f64 = 0.1;

/// Frames of R in a row before it is believed, and of R a whole number of
/// symbols out of step before the frames are taken to have moved.
const R_HEARD: usize = 8;
const R_MOVED: usize = 6;

/// The DIL: symbols read before they are counted, so that what arrived just
/// before a loss was noticed is held with what comes after it; symbols a
/// search for where the DIL went looks at; and how far a slip can move it.
const DIL_DELAY: usize = 96;
const DIL_SEARCH: usize = 128;
const DIL_MOST_MOVED: i64 = 400;

/// When to look for where the DIL went, and how often after that: a buffer
/// that made up what it lost plays a faded copy of what went before for
/// twenty milliseconds or more, and nothing fits that until it is over. What
/// is kept meanwhile, and how closely the DIL must fit what arrived, as the
/// error's power against the signal's.
const DIL_FIRST_LOOK: usize = 288;
const DIL_LOOK_EVERY: usize = 32;
const DIL_KEPT_LOST: usize = 2048;
const DIL_FIT: f64 = 0.01;

/// DIL levels above this are not learned from or judged by: the loudest
/// codewords are where a softphone's conversion runs out of headroom, and
/// they come back wherever it leaves them. As a fraction of full scale.
const DIL_TRUSTED: f64 = 0.3;

/// Symbols at the start of a DIL segment that a loud one before it spoils:
/// its references, and the frame after them, which on a live call still
/// carried a few thousandths of full scale of it.
const DIL_SPILL: usize = 2 * INTERVALS;

/// A segment after one more than this many times as loud, and louder than
/// this, is spilled into too: where one sweep up the codewords ends and the
/// next begins.
const DIL_SPILL_RATIO: f64 = 4.0;
const DIL_SPILL_LEVEL: f64 = 0.01;

/// Training symbols a stretch of the DIL has to have, loud enough to judge,
/// before where it falls can be judged from it.
const DIL_TRAINED_JUDGED: usize = 32;

/// Signs a move has to agree with, of the DIL symbols it is judged on.
const DIL_SIGNS: f64 = 0.9;

/// Finding the DIL when J'd went unread: how long after the last Jd before
/// looking, how often, how much is kept to look in, and how much of it a
/// start is judged on.
const JD_GONE: u64 = 3 * JD_BITS as u64;
const DIL_START_KEPT: usize = 1200;
const DIL_START_WINDOW: usize = 480;

/// B1d: "48 data frames" (8.6.1).
const B1D_FRAMES: usize = 48;

/// A renegotiation's Ed: "within 5000 ms plus 2 round-trip delays after
/// sending the S-bar-to-S transition" (9.6.2).
const RENEGOTIATION_ED: f64 = 5.0;

/// Whole CPs asking for nothing sent before a cleardown is over.
const CLEARDOWN_CPS: usize = 4;

/// How often data mode's margin is looked at, and how long data mode runs
/// before the watch begins: its loops settle on a new constellation first.
const MARGIN_EVERY: f64 = 0.25;
const MARGIN_SETTLE: f64 = 2.0;

/// A decision whose error went more than this share of the way to where the
/// next level's decisions begin is a miss.
///
/// Errors are what cost a call, and are what is counted, as near as a
/// receiver that does not know what was sent can count them: a symbol read
/// wrongly lands just past the boundary, measured from the level it was taken
/// for, and so does one that nearly was. Gaussian noise that reads one symbol
/// in a hundred thousand wrongly takes one in two and a half thousand past
/// four-fifths of the way, so misses come often enough to count long before
/// errors do, and a disturbance that makes errors makes misses by the dozen.
const MISSED: f64 = 0.8;

/// Misses in a look that say nothing, and the most one look can count for.
///
/// A line at its rate's margin misses now and then, from the noise's own
/// tail: over a simulated VoIP call's round trip at 54 666, whose errors were
/// half a minute apart, a look had a miss in one in five and three at worst.
/// What a slower rate is for is misses that come together -- a disturbance --
/// or so steadily that a look often has more than a couple. And one look is
/// one look, however bad: a click is not a line.
const MISSES_ALLOWED: f64 = 2.0;
const LOOK_AT_MOST: f64 = 5.0;

/// How long the evidence is remembered, as a time constant in seconds, and
/// how much of it is enough for a slower rate.
///
/// Not "so many looks in a row", which a disturbance that comes and goes
/// never is: every look adds what it has, and what has been added fades.
/// Fifteen seconds, so that bursts a few seconds apart add up and a line
/// that was disturbed a minute ago is not held against the rate now; and
/// three looks at their most, so that no one look ever is enough. Noise every
/// second or two is then a slower rate within a few bursts, and a step in the
/// floor within a second; a floor that only just makes errors, every few
/// seconds, takes ten or so.
const REMEMBERED: f64 = 15.0;
const ENOUGH: f64 = 12.5;

/// Symbols over which the error's power is taken when looking for the worst
/// of a disturbance: 32 ms.
///
/// This sizes the fall back and says nothing about what counts as a
/// disturbance -- [`STORM_GARBLED`] and [`STORM_SHORT`] say that -- so it is
/// not a shortest anything. It wants to be short enough that a burst fills
/// it, since the rate is chosen for the block's mean power and a burst that
/// fills a third of a block reads as a third of its own power, and long
/// enough that the power in it is a measurement and not a handful of symbols.
/// 32 ms is a third of the hundred-millisecond bursts this watch was written
/// for, and about three times the shortest burst it now falls back for --
/// ten milliseconds over a 0.6 s round trip, as the measurements below say.
///
/// Measured, with noise ten decibels over the line's own error every second
/// and a half: over a 20 ms round trip, thirty milliseconds of it takes
/// 50 666 to 41 333 and twenty to 44 000; over a 0.6 s round trip, thirty
/// takes 54 666 to 44 000, twenty to 46 666 and ten to 50 666. Five
/// milliseconds asks for nothing at either round trip, and has nothing to ask
/// for: it errors 6 of 1484 blocks in thirty seconds and 23 of 1602, against
/// 2 of 1602 on a clean line. So the shortest burst held against the rate is
/// well under this block, and the block dilutes what such a burst asks for
/// rather than hiding it.
const BLOCK: usize = 256;

/// How many of the recent looks have to have reached a level before a rate is
/// chosen for it.
///
/// A look's worst block is one window of 32 ms out of hundreds, and the rate
/// is chosen from it, so taking the worst of them lets any single window
/// decide how far the call falls -- and a single window can hold anything.
/// Three looks, each a quarter of a second apart, having reached a level is
/// the line reaching it. Three is also what the evidence already asks for:
/// no look counts for more than [`LOOK_AT_MOST`] and [`ENOUGH`] is two and a
/// half times that, so a renegotiation never comes of fewer.
const WORST_OF: usize = 3;

/// The most one renegotiation may take off the downstream rate, in bits a
/// frame.
///
/// A V.90 rate is D bits in every six-codeword frame -- "(drn+20)*8000/6 in
/// CP" (Table 14/V.90) -- so one bit a frame is 1333 bit/s and eight of them
/// are 10 666. Eight is as far as any line here has honestly needed in one
/// step: a hundred milliseconds of noise every second and a half took 50 666
/// to 40 000. Further than that in one go is a measurement to doubt rather
/// than to act on, and there is no need to act on all of it at once -- the
/// line is measured again at the new rate, and a renegotiation that stopped
/// short is followed by another that goes the rest of the way.
const MOST_DROPPED: u8 = 8;

/// How much less of what data mode found is believed on each try at keeping
/// a renegotiation inside [`MOST_DROPPED`]: half a decibel of error power.
const BELIEVED_LESS: f64 = 1.0594;

/// Clean decisions that end a stretch of misses: sixteen milliseconds.
///
/// A disturbance does not miss every decision it touches, nor nearly. A
/// tenth of a second of noise ten decibels over what a clean line leaves
/// misses about one decision in ten, and 32 clean ones in a row turn up
/// inside one several times over (0.9^32 is one in thirty); 128 in a row do
/// not (one in a million), so the whole hundred milliseconds stays one
/// stretch. Nothing here puts two disturbances within sixteen milliseconds of
/// each other, and a line at its own margin goes thousands of decisions
/// between misses, so neither joins two stretches into one.
const STORM_GAP: usize = 128;

/// Misses in a stretch before it is a disturbance at all, rather than the
/// line's own tail.
///
/// A look of 2000 decisions over a simulated VoIP round trip at 54 666 had
/// three misses at worst, and a floor stepped up to where errors come every
/// few seconds leaves a miss or two in a look: neither puts six of them
/// within sixteen milliseconds of each other. It was sixteen, which is how
/// many a twenty-millisecond packet leaves, and a ten-millisecond one leaves
/// half of that: measured over the round trip, six to fifty-one, so sixteen
/// sat inside the distribution and half the packets were weighed as the line.
/// A count cannot be the test, because a count is the length of the garbage;
/// what tells garbage from the line is [`STORM_GARBLED`], and all this floor
/// does now is keep the one-decision stretch out, where a single miss carries
/// all its own sound and would pass that outright.
const STORM_MISSES: usize = 6;

/// The share of the sound under a stretch that the decisions which missed
/// have to carry before the stretch is made-up audio rather than the line.
///
/// This is what tells a jitter buffer's garbage from a disturbance on the
/// line, and it has to be this rather than how long the stretch lasted,
/// because a packet holds ten, twenty or thirty milliseconds of G.711 and a
/// crackle can last exactly as long. What differs is which decisions miss.
/// Noise adds to a codeword that is still there, so it can only push one
/// four-fifths of the way to the boundary where the boundary is near -- at
/// the quiet levels, a mu-law segment apart -- while the loud codewords,
/// which carry nearly all the sound there is, are read as cleanly as before.
/// Made-up audio is in the codewords' place rather than on top of them, so it
/// misses at every level alike and its misses carry their share of the sound.
/// That is also why no slower rate reads it: wider levels shrink what noise
/// does and leave made-up audio where it was.
///
/// Measured as the power of the missed decisions over the power of all of
/// them, from the first miss in a stretch to the last, over eight routes --
/// packets of ten, twenty and thirty milliseconds, concealed by a fading
/// repeat and by comfort noise, mu-law and A-law, round trips of 0.6 s and
/// 20 ms, with and without a softphone's slips running as well: 126 stretches
/// of made-up audio carried 0.033 to 0.56 of it, and 40 stretches of noise
/// ten decibels over the line's own error -- thirty milliseconds of it and a
/// hundred, over the same routes -- carried 0.005 to 0.027. Nothing of either
/// kind fell on the wrong side of a thirtieth. A clean line and a floor
/// stepped up to where errors come every few seconds make no stretch of
/// [`STORM_MISSES`] at all.
const STORM_GARBLED: f64 = 0.03;

/// The longest a stretch of misses can be and still be one packet of
/// made-up audio, in symbols: sixty-four milliseconds.
///
/// A softphone's packets hold ten, twenty or thirty milliseconds of G.711,
/// and what the buffer makes up, plays twice or drops is one of them: the
/// garbage lasts exactly that long, and the line either side of it is the
/// line it always was. Longer than a packet, a disturbance that [`STORM_GARBLED`]
/// would call made-up audio is something else -- a line that has gone on
/// being bad, or a receiver losing its way -- and a slower rate is worth
/// asking for.
///
/// Measured over eight routes, ten, twenty and thirty milliseconds concealed
/// in place, by a fading repeat and by comfort noise, mu-law and A-law, round
/// trips of 0.6 s and 20 ms: a ten-millisecond packet made stretches of 45 to
/// 380 decisions, a twenty-millisecond one 144 to 415, and a thirty of 223 to
/// 284 -- its own 80, 160 or 240 and the loops coming back after them. A
/// hundred milliseconds of noise made 619 to 876. 512 lies between the
/// longest packet and the shortest hundred-millisecond burst.
///
/// The length alone tells nothing else, and this constant no longer pretends
/// to: thirty milliseconds of noise ten decibels over the line's own error
/// made stretches of 177 to 237, inside a thirty-millisecond packet's own
/// range, and is held against the rate all the same. What tells them apart is
/// [`STORM_GARBLED`]; all this does is stop a stretch that goes on and on
/// from being excused as a packet.
const STORM_SHORT: usize = 512;

/// A hole in the audio: the line in front of the equaliser at under a
/// hundred-thousandth of its own level, that many symbols in a row, and how
/// long that level is taken over -- half a second of symbols.
///
/// Some softphones do not conceal a lost packet at all: they play zeroes.
/// Nothing is made up, so [`STORM_GARBLED`] has nothing to weigh -- silence
/// misses hardly anything, since a decision at nothing is nearer the
/// quietest level than the boundary in most intervals -- and nothing moves
/// either, so a hole leaves no mark but its own silence.
///
/// The decisions cannot see that silence. The equaliser is 63 half symbols
/// of line and a feedback filter of its own past decisions, so it goes on
/// putting out codeword-sized numbers through a hole with nothing at all
/// behind it. Measured over thirty seconds of twenty-millisecond holes every
/// second and a half on the mu-law 0.6 s route, the longest run of decisions
/// under a ten-thousandth of the decisions' own level was 2 -- against 1 on
/// a clean line, 2 under a concealer's comfort noise and 7 under its fading
/// repeat. There is nothing in the decisions to tell a hole by, and a rule
/// written on them fires on the wrong things or not at all.
///
/// The line the equaliser drew the symbol from ([`pcm::Symbol::line`]) tells
/// it at once. That window is 63 half symbols, 3.94 ms, so a hole longer
/// than the window empties it, and once it is empty there is nothing in it
/// but the codec's own ringing from either side of the hole. Measured over
/// 130 s of the same known data on each of eighty routes -- both laws, round
/// trips of 0.6 s and 20 ms, judging exactly as [`Decisions::line`] judges,
/// with a hole held out of the level as it holds one out -- the quietest the
/// window reached against that level, and the longest run of symbols under
/// each threshold:
///
/// | what happened | quietest | run under 1e-3 | under 1e-4 | under 1e-5 |
/// |---|---|---|---|---|
/// | nothing: a clean line | -5.8 to -5.2 dB | none | none | none |
/// | 5 to 300 ms of noise at 1e-3 | -5.8 to -4.0 dB | none | none | none |
/// | the floor stepped or ramped to 6e-4 | -4.8 to -4.2 dB | none | none | none |
/// | a slip, and a softphone's gain control | -23.8 to -21.6 dB | none | none | none |
/// | 10 to 60 ms concealed with comfort noise | -6.8 to -6.4 dB | none | none | none |
/// | 10 to 60 ms concealed with a fading repeat | -32.1 to -15.1 dB | 0 to 3 | none | none |
/// | 100 ms concealed with a fading repeat | -34.8 to -33.2 dB | 12 to 17 | none | none |
/// | 200 ms concealed with a fading repeat | -39.0 to -37.6 dB | 43 to 49 | none | none |
/// | 5 ms of digital silence | -59.4 dB | 10 | 9 | 6 |
/// | 10 to 30 ms of digital silence | -86 to -84 dB | 49 to 211 | 47 to 209 | 41 to 203 |
///
/// So the thousandth this once used was not a line at all. A concealer that
/// repeats the last packet fades it out linearly, and the end of the fade is
/// silence: the longer the packet it is repeating, the longer the window
/// spends under any given level. At a thousandth, sixty milliseconds of
/// fading repeat already leaves a run of 3 and a hundred leaves 12 to 17 --
/// which, against [`HOLE`]'s sixteen, is a hundred milliseconds of made-up
/// audio counted as a hole on A-law and not on mu-law. There was no margin
/// there to speak of and the note claimed a wide one.
///
/// There is a wide one a hundred times further down. A hole empties the
/// window altogether and reaches 4e-9 of the level -- the codec's ringing
/// and nothing else -- while the quietest thing that is not a hole, two
/// hundred milliseconds of fading repeat, stops at 1.26e-4. A
/// hundred-thousandth sits 11 dB under everything that is not a hole and
/// 34 dB over every hole, and nothing that is not a hole leaves a run of one
/// symbol under it anywhere in the eighty routes.
///
/// Sixteen symbols under that level is two milliseconds of line gone on top
/// of the window emptying, so about seven milliseconds of silence in all:
/// ten milliseconds leaves a run of 41 to 42, twenty leaves 119 to 121 and
/// thirty leaves 203, while five leaves 6 and is left alone, as five
/// milliseconds of noise is (see [`BLOCK`]), and four never empties the
/// window at all.
///
/// The line's own level is a slow mean of that same window's power, and a
/// hole is held out of it: a hole cannot be allowed to drag its own
/// yardstick down after it, which is why [`carrier::Watch`] holds its
/// reference for exactly as long.
///
/// A far end that has genuinely stopped is not this, and is not this end's
/// to cure by a slower rate either. It differs only in lasting: a buffer's
/// hole is over in tens of milliseconds, and a far end on its way out holds
/// the line quiet until [`carrier::Watch::quiet`] sees it -- 20 dB under the
/// reference on a 50 ms time constant, which measured wants 0.237 s -- and
/// then the looks go for that reason instead.
const HOLE: usize = 16;
const HOLE_LEVEL: f64 = 1e-5;
const LEVEL_OVER: f64 = 4000.0;

/// Data mode's levels closer than this many of the receiver's own averaged
/// errors, this many looks running, are a line steadily short of margin: the
/// rule the watch on the averaged error kept before any of this counted
/// misses, and the rule this keeps for that kind of trouble still.
///
/// A line that is steadily short of margin is the one thing the receiver's
/// average does read honestly. It holds its loops through a burst of noise or
/// a packet of made-up audio and barely takes either in -- which is what
/// makes it a poor judge of a disturbance, and is why the decisions are
/// counted at all -- but a floor that has risen it follows exactly, and
/// unlike a decision's error it is not compressed by the levels it is
/// measured against. So that line is left where it was drawn: seven averaged
/// errors, four looks running.
///
/// Seven, measured here as well as there. Over 130 s of call in ten-second
/// stretches, a rung was worth taking where the levels stood 3.7 and 6.3
/// averaged errors apart -- a floor stepped to 6e-4 over the 0.6 s round
/// trip, and the same floor reached over twenty seconds -- and not where they
/// stood 7.2, 7.4, 7.8 or 8.0, where holding the rate left 11 to 17 of about
/// 2470 blocks errored in every later stretch and a rung would have cost ten
/// per cent of them to take that to nothing. The measurements straddle seven
/// and do not pin it closer than between 6.3 and 7.2; the old note's own
/// measurement -- "a frame in some hundreds at six, none in a quarter of a
/// million bits at eight" -- puts it in the same place, and it stays at
/// seven.
const MARGIN: f64 = 7.0;
const MARGIN_SHORT: u32 = 4;

/// Levels fewer than this many of the receiver's averaged errors apart are a
/// receiver that is reading nothing at all: two.
///
/// Whether the receiver is reading anything is a question about the receiver,
/// not about the line, so it is asked of the receiver's own averaged error;
/// and a receiver that has lost the constellation is not handed it back by a
/// slower rate, so it is trained again instead. It is asked only where
/// [`MARGIN`] has already found four looks running short of margin, which is
/// what keeps a hole in the audio from asking for one: through a hole the
/// average is a mean of nothing at all and climbs for that reason alone.
const UNREADABLE_GAP: f64 = 2.0;

/// Looks held after a stretch of garbage that was not the line's: what the
/// receiver reads while its loops come back in is not the line either. Half a
/// second, which is how long it holds them still before deciding the errors
/// are the line's after all ([`pcm`]'s `HELD_AT_MOST`).
const HOLD_AFTER: u32 = 1;

/// Frames over which a read of impossible numbers is counted, how many make
/// it a lost place, and symbols kept for finding the place again.
const PLACE_WINDOW: usize = 24;
const PLACE_LOST: usize = 3;
const PLACE_KEPT: usize = 48 * INTERVALS;

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

/// What phase 2 settled, as the analogue modem needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub law: Law,
    pub uinfo: u8,
    pub server: Info0d,
    /// This end's transmitter, as INFO1a chose it and INFO1d set it up.
    pub upstream: Band,
    pub pre_emphasis: u8,
    pub power_reduction: u8,
    pub round_trip: f64,
    /// Whether both ends have the 1664-point constellation the upstream's
    /// top rates need.
    pub wide: bool,
    /// What V.34 would carry downstream instead, as phase 2's probe put it:
    /// a V.90 slower than that is not worth having.
    pub v34_receive: u32,
}

impl Settings {
    /// From the digital modem's INFO0d and INFO1d, and this end's INFO1a.
    pub fn new(server: &Info0d, info1d: &Info1c, asked: &Info1aPcm, round_trip: f64, ours_wide: bool) -> Self {
        let probed = info1d.probed[asked.upstream.index() as usize];
        Self {
            law: if server.a_law { Law::A } else { Law::Mu },
            uinfo: asked.uinfo,
            server: *server,
            upstream: Band::new(asked.upstream, probed.high_carrier),
            pre_emphasis: probed.pre_emphasis,
            power_reduction: info1d.min_power_reduction,
            round_trip,
            wide: ours_wide && server.v34.constellation_1664,
            v34_receive: 0,
        }
    }
}

/// What goes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Up {
    Silence,
    S,
    SBar,
    Pp,
    Trn,
    Ja,
    Cp,
    E,
    Data,
}

fn grid(point: Point, size: Size) -> Complex {
    Complex::new(f64::from(point.0), f64::from(point.1)).scale(receiver::unit(size))
}

/// Something the upstream has just begun sending that the transcript is
/// told of.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Began {
    /// A CP sequence unlike the last one this phase 4 sent, as bits.
    Cp(Vec<bool>),
    E,
}

/// The upstream, one symbol at a time.
#[derive(Debug, Clone)]
struct Source {
    sender: Sender,
    up: Up,
    count: usize,
    /// How long S runs: to a count, or until changed.
    s_length: Option<usize>,
    after_s_bar: Up,
    trn_length: usize,
    pending: Option<Up>,
    queue: VecDeque<bool>,
    ja: Vec<bool>,
    cp: Vec<bool>,
    cp_is_ack: bool,
    next_cp: Option<(Vec<bool>, bool)>,
    /// Whole CP sequences sent, and whole CP' sequences.
    cps: usize,
    acknowledged: usize,
    restarted: bool,
    size: Size,
    encoder: Option<UpstreamEncoder>,
    data: VecDeque<bool>,
    hold: usize,
    silent: usize,
    /// What has just begun going out, for the transcript, and the last CP
    /// told of since phase 4 began: each different CP is told once, and not
    /// each repetition of it.
    began: Option<Began>,
    told: Vec<bool>,
}

impl Source {
    fn new(trn_length: usize) -> Self {
        Self {
            // The analogue modem scrambles with GPA (8.3).
            sender: Sender::new(Mode::Answer),
            up: Up::Silence,
            count: 0,
            s_length: Some(signals::S_SYMBOLS),
            after_s_bar: Up::Pp,
            trn_length,
            pending: None,
            queue: VecDeque::new(),
            ja: Vec::new(),
            cp: Vec::new(),
            cp_is_ack: false,
            next_cp: None,
            cps: 0,
            acknowledged: 0,
            restarted: false,
            size: Size::Four,
            encoder: None,
            data: VecDeque::new(),
            hold: 0,
            silent: 0,
            began: None,
            told: Vec::new(),
        }
    }

    fn start(&mut self, up: Up) {
        self.up = up;
        self.count = 0;
        self.silent = 0;
        self.queue.clear();
        if up == Up::Cp {
            // A phase 4 of its own, from the start or from data mode: every
            // CP in it is news.
            self.told.clear();
        }
        match up {
            Up::Trn => self.sender.restart(),
            // "The scrambler and differential encoder are initialized to zero
            // prior to the transmission of the first CPt sequence" (8.5.2).
            Up::Cp if !self.restarted => {
                self.restarted = true;
                self.sender.restart();
            }
            Up::E => {
                self.queue.extend(std::iter::repeat_n(true, signals::E_BITS));
                self.began = Some(Began::E);
            }
            _ => {}
        }
    }

    fn change(&mut self, up: Up) {
        self.pending = Some(up);
    }

    fn differential(&mut self, size: Size) -> Complex {
        let bits: Vec<bool> = (0..size.bits()).map(|_| self.queue.pop_front().unwrap_or(true)).collect();
        self.count += 1;
        grid(self.sender.differential(&bits), size)
    }

    fn next(&mut self) -> Complex {
        loop {
            match self.up {
                Up::Silence => {
                    if self.silent >= self.hold
                        && let Some(next) = self.pending.take()
                    {
                        self.hold = 0;
                        self.start(next);
                        continue;
                    }
                    self.silent += 1;
                    return Complex::ZERO;
                }
                Up::S => {
                    let done = match self.s_length {
                        Some(n) => self.count >= n,
                        // Until something else is asked for, and then at the
                        // end of a pair, so S-bar follows in step.
                        None => self.pending.is_some() && self.count.is_multiple_of(2),
                    };
                    if done {
                        let next = if self.s_length.is_some() { Up::SBar } else { self.pending.take().unwrap_or(Up::SBar) };
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s(self.count - 1), Size::Four);
                }
                Up::SBar => {
                    if self.count == signals::S_BAR_SYMBOLS {
                        let next = self.after_s_bar;
                        self.start(next);
                        continue;
                    }
                    self.count += 1;
                    return grid(signals::s_bar(self.count - 1), Size::Four);
                }
                Up::Pp => {
                    if self.count == signals::PP_SYMBOLS {
                        self.start(Up::Trn);
                        continue;
                    }
                    self.count += 1;
                    return signals::pp(self.count - 1).into();
                }
                Up::Trn => {
                    if self.count >= self.trn_length {
                        self.start(Up::Ja);
                        continue;
                    }
                    self.count += 1;
                    return grid(self.sender.trn(Size::Four), Size::Four);
                }
                Up::Ja => {
                    // "Transmission of sequence Ja may be terminated without
                    // completing the final DIL descriptor" (8.3.1).
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    if self.queue.is_empty() {
                        self.queue.extend(self.ja.iter().copied());
                    }
                    return self.differential(Size::Four);
                }
                Up::Cp => {
                    if self.queue.is_empty() {
                        if self.count > 0 {
                            self.cps += 1;
                            if self.cp_is_ack {
                                self.acknowledged += 1;
                            }
                        }
                        if let Some(next) = self.pending.take() {
                            self.start(next);
                            continue;
                        }
                        if let Some((cp, ack)) = self.next_cp.take() {
                            self.cp = cp;
                            self.cp_is_ack = ack;
                        }
                        if self.cp != self.told {
                            self.told = self.cp.clone();
                            self.began = Some(Began::Cp(self.cp.clone()));
                        }
                        self.queue.extend(self.cp.iter().copied());
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::E => {
                    if self.queue.is_empty() {
                        let next = if self.encoder.is_some() { Up::Data } else { Up::Silence };
                        self.start(next);
                        continue;
                    }
                    let size = self.size;
                    return self.differential(size);
                }
                Up::Data => {
                    if let Some(next) = self.pending.take() {
                        self.start(next);
                        continue;
                    }
                    let Some(encoder) = self.encoder.as_mut() else {
                        self.start(Up::Silence);
                        continue;
                    };
                    // B1 is one data frame of scrambled ones (8.5.1, and
                    // 10.1.3.1/V.34).
                    let b1 = encoder.mapping_frames() < encoder.params().framing.p as u64;
                    let data = &mut self.data;
                    self.count += 1;
                    return encoder.next_symbol(&mut || if b1 { true } else { data.pop_front().unwrap_or(true) });
                }
            }
        }
    }
}

/// Reads Jd and J'd off the signs (8.4.2, 8.4.3).
#[derive(Debug, Clone)]
struct JdReader {
    descrambler: Scrambler,
    differential: bool,
    previous: bool,
    bits: VecDeque<bool>,
    /// The last Jd read whole, and the symbol after it.
    last: Option<(u64, Jd)>,
    /// Its bits, as they went.
    jd_bits: Vec<bool>,
}

/// The end of a Jd that J'd is found after: its CRC and fill.
const JD_TAIL: usize = 24;

impl JdReader {
    fn new() -> Self {
        Self {
            descrambler: Scrambler::new(Mode::Call),
            differential: false,
            previous: false,
            bits: VecDeque::new(),
            last: None,
            jd_bits: Vec::new(),
        }
    }

    /// One symbol's sign. True when this symbol ended a J'd.
    fn feed(&mut self, index: u64, positive: bool) -> bool {
        let before = self.descrambler.clone();
        let mut bit = self.descrambler.descramble(if self.differential { positive ^ self.previous } else { positive });
        if !self.differential && !bit {
            // TRN1d descrambles to ones: the first zero is Jd, which is
            // differential from here -- and was for the symbol that showed
            // it.
            self.differential = true;
            self.descrambler = before;
            bit = self.descrambler.descramble(positive ^ self.previous);
        }
        self.previous = positive;
        self.bits.push_back(bit);
        if self.bits.len() > JD_BITS + JD_PRIME_BITS {
            self.bits.pop_front();
        }
        let n = self.bits.len();
        let bits = self.bits.make_contiguous();
        if n >= JD_BITS
            && let Some(jd) = Jd::from_bits(&bits[n - JD_BITS..])
        {
            self.last = Some((index + 1, jd));
            self.jd_bits = bits[n - JD_BITS..].to_vec();
        }
        // "12 binary zeroes" where the next Jd's sync would start: after the
        // end of a Jd, wherever that fell. A softphone that cut a few
        // milliseconds out of the last Jd leaves it unreadable whole, but
        // its tail is the tail of every other.
        self.jd_bits.len() == JD_BITS
            && n >= JD_TAIL + JD_PRIME_BITS
            && bits[n - JD_PRIME_BITS..].iter().all(|b| !*b)
            && bits[n - JD_PRIME_BITS - JD_TAIL..n - JD_PRIME_BITS] == self.jd_bits[JD_BITS - JD_TAIL..]
    }
}

/// Watches for R and its turn to R-bar (8.6.4).
///
/// The equaliser has already turned a line that inverts the signal back
/// over, so R and R-bar are told apart by their signs. What the watch cannot
/// take for granted is where the frames are: a jitter buffer's slip moves
/// every symbol after it by a whole number of symbols, and R, which is the
/// same frame over and over, shows by how many. R-bar is R moved by three,
/// which no slip of whole milliseconds does.
#[derive(Debug, Clone, Default)]
struct RWatch {
    frame: [f64; INTERVALS],
    /// Frames in a row of R moved by `moved` symbols.
    run: usize,
    moved: usize,
    heard: bool,
    /// Whether the last whole frame looked like R or R-bar, however moved.
    looked: bool,
}

/// What the watch made of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RSeen {
    Nothing,
    /// R has turned into R-bar: TRN2d begins at this symbol.
    Turned(u64),
    /// R is arriving this many symbols late: the frames have moved.
    Moved(usize),
}

impl RWatch {
    /// One symbol, and the level R has in each interval.
    fn feed(&mut self, symbol: &pcm::Symbol, levels: &[f64; INTERVALS]) -> RSeen {
        let i = symbol.interval();
        self.frame[i] = symbol.value;
        if i != INTERVALS - 1 {
            return RSeen::Nothing;
        }
        // R moved by m: "+ + + - - -" starting m symbols in.
        let late = (0..INTERVALS).find(|&m| {
            (0..INTERVALS).all(|j| {
                let k = (j + INTERVALS - m) % INTERVALS;
                let v = self.frame[j];
                (v >= 0.0) == (k < 3) && (0.5 * levels[k]..1.5 * levels[k]).contains(&v.abs())
            })
        });
        self.looked = late.is_some();
        match late {
            Some(0) => {
                self.run = if self.moved == 0 { self.run + 1 } else { 1 };
                self.moved = 0;
                if self.run >= R_HEARD {
                    self.heard = true;
                }
                RSeen::Nothing
            }
            Some(3) if self.heard => {
                // R-bar's first frame. TRN2d starts after the other three.
                let frame_start = symbol.index + 1 - INTERVALS as u64;
                RSeen::Turned(frame_start + 4 * INTERVALS as u64)
            }
            Some(m) if m != 3 => {
                self.run = if self.moved == m { self.run + 1 } else { 1 };
                self.moved = m;
                if self.run < R_MOVED {
                    return RSeen::Nothing;
                }
                self.run = 0;
                self.moved = 0;
                RSeen::Moved(m)
            }
            _ => {
                if !self.heard {
                    self.run = 0;
                }
                RSeen::Nothing
            }
        }
    }
}

/// Signed levels of each interval's constellation, as the route delivers
/// them: (level, Ucode, positive).
type Levels = [Vec<(f64, u8, bool)>; INTERVALS];

fn levels_for(cp: &Cp, route: &Route) -> Levels {
    std::array::from_fn(|i| {
        cp.points(i)
            .into_iter()
            .flat_map(|u| {
                let level = route.levels[i][usize::from(u)];
                [(level, u, true), (-level, u, false)]
            })
            .collect()
    })
}

/// The least distance between two of a CP's levels, either sign, as the
/// route delivers them.
fn least_gap(cp: &Cp, route: &Route) -> f64 {
    (0..INTERVALS)
        .map(|i| {
            let mut levels: Vec<f64> =
                cp.points(i).iter().flat_map(|&u| [route.levels[i][usize::from(u)], -route.levels[i][usize::from(u)]]).collect();
            levels.sort_by(f64::total_cmp);
            levels.windows(2).map(|w| w[1] - w[0]).fold(f64::INFINITY, f64::min)
        })
        .fold(f64::INFINITY, f64::min)
}

/// A CP as the transcript tells it (Table 14): which kind it is, its rate
/// and drn, K, how many points each interval has and which constellation
/// field each is on, Sr, the look-ahead and the acknowledge bit.
fn describe_cp(cp: &Cp) -> String {
    // Bit 19 and bit 33: "0 indicates CPt; 1 indicates CP", and "received MP
    // from far end".
    let name = match (cp.data_mode, cp.acknowledge) {
        (false, false) => "CPt",
        (false, true) => "CPt'",
        (true, false) => "CP",
        (true, true) => "CP'",
    };
    if cp.drn == 0 {
        // "drn = 0 indicates cleardown".
        return format!("{name} asking for a cleardown (drn 0)");
    }
    let d = cp.frame_bits();
    let sizes: Vec<usize> = (0..INTERVALS).map(|i| cp.points(i).len()).collect();
    format!(
        "{name}: {} bit/s (drn {}), K {}, sizes {sizes:?} on fields {:?}, Sr {}, look-ahead {}, acknowledge {}",
        super::rate_for(d as u32),
        cp.drn,
        d.saturating_sub(cp.redundancy.data_bits()),
        cp.intervals,
        cp.redundancy.spent(),
        cp.lookahead,
        u8::from(cp.acknowledge),
    )
}

/// An MP as the transcript tells it (Table 16): its type, the fastest
/// upstream it allows, its acknowledge bit, and whether its precoder does
/// anything.
fn describe_mp(mp: &Mp) -> String {
    // Bit 18: "1 = Type 1 with precoder coefficients". A type 1 MP whose
    // coefficients are all zero asks for no precoding at all.
    let precoding = mp.precoding.is_some_and(|h| h.iter().any(|&c| c != (0, 0)));
    // V.90's bits 24:27 are where V.34 reads its answer-to-call rate:
    // "Data rate = drn*2400".
    format!(
        "{}: type {}, upstream at most {} bit/s (drn {}), acknowledge {}, precoding {}",
        if mp.acknowledge { "MP'" } else { "MP" },
        u8::from(mp.precoding.is_some()),
        2400 * u32::from(mp.answer_to_call),
        mp.answer_to_call,
        u8::from(mp.acknowledge),
        if precoding { "on" } else { "off" },
    )
}

/// What a look saw of data mode's decisions.
#[derive(Debug, Clone, Copy, Default)]
struct Look {
    symbols: usize,
    misses: usize,
    /// The error's power, summed.
    power: f64,
    /// The mean power of the error over its worst block.
    worst: f64,
    /// Whether something that is not the line happened while it ran.
    spoiled: bool,
}

/// A stretch of decisions with misses in it, bounded by clean ones either
/// side (see [`Decisions::storm`]).
#[derive(Debug, Clone, Copy, Default)]
struct Storm {
    /// Decisions since the first miss in it, and since the first to the last.
    spanned: usize,
    ended: usize,
    misses: usize,
    /// The sound under it: the decisions' power since the first miss, the
    /// same as it stood at the last miss, and the power of the decisions
    /// that missed (see [`STORM_GARBLED`]).
    sound: f64,
    at_last: f64,
    garbled: f64,
}

/// Data mode's decisions, as the watch on the margin sees them.
#[derive(Debug, Clone, Default)]
struct Decisions {
    /// Each interval's levels, both signs, in order.
    levels: [Vec<f64>; INTERVALS],
    /// Whether the watch has begun.
    started: bool,
    /// The look under way, and the block under way in it: symbols and power.
    look: Look,
    block: (usize, f64),
    /// The stretch of misses under way, and looks still to be held after one
    /// that was garbage.
    storm: Option<Storm>,
    holding: u32,
    /// The look before, held back until this one is over (see [`Self::look`]).
    held: Option<Look>,
    /// Times the frames had moved when the look under way began.
    moved: u32,
    /// The evidence so far, and the looks it was gathered from.
    evidence: f64,
    recent: VecDeque<Look>,
    /// The line's own level, as a slow mean of what the equaliser was given,
    /// and the symbols since the last one whose line reached a thousandth of
    /// it (see [`HOLE`]).
    level: f64,
    hole: usize,
}

impl Decisions {
    /// A watch on decisions against data mode's levels, not yet begun.
    fn new(levels: &Levels) -> Self {
        let levels = std::array::from_fn(|i| {
            let mut sorted: Vec<f64> = levels[i].iter().map(|l| l.0).collect();
            sorted.sort_by(f64::total_cmp);
            sorted
        });
        Self { levels, ..Self::default() }
    }

    /// One symbol of data mode, as the equaliser gave it, in interval `i`.
    ///
    /// Judged against the two levels either side of it, of that interval and
    /// either sign: the error is its distance from the nearer, and the miss
    /// is in how far it went towards the boundary between them. Levels stand
    /// further apart the louder they are, and a symbol at a loud level can
    /// wander further without being misread; beyond the outermost there is
    /// no other level to take it for at all.
    fn symbol(&mut self, i: usize, value: f64) {
        if !self.started {
            return;
        }
        let levels = &self.levels[i];
        let k = levels.partition_point(|&l| l <= value);
        let (error, share) = match (k.checked_sub(1).map(|b| levels[b]), levels.get(k)) {
            (Some(below), Some(&above)) => {
                let error = (value - below).min(above - value);
                (error, error / (0.5 * (above - below)))
            }
            (Some(outermost), None) => (value - outermost, 0.0),
            (None, Some(&outermost)) => (outermost - value, 0.0),
            (None, None) => return,
        };
        let look = &mut self.look;
        look.symbols += 1;
        look.power += error * error;
        let missed = share > MISSED;
        if missed {
            look.misses += 1;
        }
        self.block.0 += 1;
        self.block.1 += error * error;
        if self.block.0 == BLOCK {
            look.worst = look.worst.max(self.block.1 / BLOCK as f64);
            self.block = (0, 0.0);
        }
        self.storm(missed, value);
    }

    /// One symbol's worth of the line in front of the equaliser, through the
    /// hole in the audio under way, if there is one.
    ///
    /// A hole is judged on the line itself, not on what the equaliser made
    /// of it: a decision is still a codeword-sized number through a hole
    /// with nothing behind it, and the line is nothing. [`HOLE`] symbols in
    /// a row whose line is under a thousandth of the line's own level is a
    /// buffer playing zeroes, and the looks go as they do for made-up audio
    /// -- silence in the codewords' place is no more the line's than noise
    /// in their place is, and no slower rate reads a codeword that never
    /// arrived.
    /// True on the one symbol that declares a hole, so that it can be
    /// counted.
    fn line(&mut self, power: f64) -> bool {
        if self.level > 0.0 && power < HOLE_LEVEL * self.level {
            self.hole += 1;
            // Held out of the level: see [`HOLE`].
            if self.hole != HOLE {
                return false;
            }
            self.look.spoiled = true;
            self.holding = HOLD_AFTER;
            return true;
        }
        self.hole = 0;
        self.level += (power - self.level) / LEVEL_OVER;
        false
    }

    /// One decision, a miss or not, through the stretch of misses under way.
    ///
    /// A stretch begins at a miss and runs to the last miss within
    /// [`STORM_GAP`] decisions of it. When it ends, a stretch of more than
    /// the line's own tail ([`STORM_MISSES`]) is judged on two counts: how
    /// much of the sound under it its misses carried, which says the audio
    /// was made up rather than disturbed ([`STORM_GARBLED`]), and how long it
    /// lasted, which says it was one packet of made-up audio and not a line
    /// that has gone on being bad ([`STORM_SHORT`]). Both, and it is a jitter
    /// buffer's doing rather than the line's: the look it happened in -- and
    /// so the look before it, which a look always waits for -- go, and so do
    /// the next [`HOLD_AFTER`], for the loops to come back in.
    fn storm(&mut self, missed: bool, value: f64) {
        let sound = value * value;
        let Some(mut storm) = self.storm else {
            self.storm = missed.then_some(Storm { spanned: 0, ended: 0, misses: 1, sound, at_last: sound, garbled: sound });
            return;
        };
        storm.spanned += 1;
        storm.sound += sound;
        if missed {
            storm.ended = storm.spanned;
            storm.misses += 1;
            storm.at_last = storm.sound;
            storm.garbled += sound;
        }
        if storm.spanned - storm.ended <= STORM_GAP {
            self.storm = Some(storm);
            return;
        }
        self.storm = None;
        // Strictly more, so that a stretch with no sound under it at all is
        // not garbage by default: silence carries nothing for this to weigh,
        // and is [`HOLE`]'s to catch.
        let garbled = storm.garbled > STORM_GARBLED * storm.at_last;
        if storm.misses >= STORM_MISSES && garbled && storm.ended < STORM_SHORT {
            self.look.spoiled = true;
            self.holding = HOLD_AFTER;
        }
    }

    /// Something that is not the line happened in the look under way.
    fn spoil(&mut self) {
        self.look.spoiled = true;
    }

    /// End the look under way, whose end finds the frames moved `moved`
    /// times so far; and the look before it, if it stands.
    ///
    /// A look is held back until the next is over, and thrown away if either
    /// was spoiled. What spoils one is a jitter buffer, in any of the three
    /// shapes it comes in.
    ///
    /// A slip inserts a packet of made-up audio or drops one, and moves every
    /// symbol after it by a packet's length. The garbage itself is a stretch
    /// of misses a packet long, which [`Self::storm`] catches whatever the
    /// length; and when the length is not a whole number of frames -- 160
    /// codewords never is -- the frames turn up somewhere else a few frames
    /// later, which says it was a slip too. The receiver holding its loops
    /// says nothing: it holds them through any sudden rise in error, a burst
    /// of noise as much as a slip.
    ///
    /// A packet lost and concealed where it was moves nothing at all, and a
    /// slip of a whole number of frames -- 240 codewords is 40 of them --
    /// leaves the frame place where it was as well, so `moved` never changes
    /// for either. Nothing but the garbage marks them, and the garbage is
    /// what is judged.
    ///
    /// And a far end going quiet is not a disturbance a slower rate cures
    /// either -- though that is a far end on its way out and not a jitter
    /// buffer at all: it wants about a quarter of a second of silence before
    /// it shows (see [`carrier::Watch::quiet`]), where a buffer's gap is
    /// twenty milliseconds.
    fn look(&mut self, moved: u32) -> Option<Look> {
        if !self.started {
            // The first look only begins the watch.
            self.started = true;
            self.moved = moved;
            return None;
        }
        if self.block.0 >= BLOCK / 2 {
            self.look.worst = self.look.worst.max(self.block.1 / self.block.0 as f64);
        }
        self.block = (0, 0.0);
        if moved != self.moved {
            self.look.spoiled = true;
        }
        self.moved = moved;
        let look = std::mem::take(&mut self.look);
        if self.holding > 0 {
            self.holding -= 1;
            self.look.spoiled = true;
        }
        let before = self.held.replace(look)?;
        (!before.spoiled && !look.spoiled).then_some(before)
    }

    /// Weigh a look that stands. True once there is evidence enough for a
    /// slower rate.
    fn weigh(&mut self, look: Look) -> bool {
        let counted = (look.misses as f64 - MISSES_ALLOWED).clamp(0.0, LOOK_AT_MOST);
        self.evidence = self.evidence * (-MARGIN_EVERY / REMEMBERED).exp() + counted;
        self.recent.push_back(look);
        if self.recent.len() as f64 > REMEMBERED / MARGIN_EVERY {
            self.recent.pop_front();
        }
        self.evidence >= ENOUGH
    }

    /// The error's RMS over the worst 32 ms block that [`WORST_OF`] of the
    /// recent looks reached: the third worst of them, not the worst.
    fn worst(&self) -> f64 {
        let mut blocks: Vec<f64> = self.recent.iter().map(|l| l.worst).collect();
        blocks.sort_by(f64::total_cmp);
        blocks.iter().rev().nth(WORST_OF - 1).copied().unwrap_or_default().sqrt()
    }

    /// The error's RMS over all the recent looks.
    fn rms(&self) -> f64 {
        let (power, symbols) = self.recent.iter().fold((0.0, 0), |(p, n), l| (p + l.power, n + l.symbols));
        (power / symbols.max(1) as f64).sqrt()
    }
}

/// Which of a DIL's symbols can be learned from and judged by (see
/// `Modem::dil_trusted`).
fn trusted_symbols(descriptor: &Descriptor, law: Law) -> Vec<Trust> {
    let level = |u: u8| ucode::level(law, u);
    let loud = |u: u8| level(u) > DIL_TRUSTED;
    // What a louder segment leaves behind is small, but not beside a
    // segment a great deal quieter.
    let spills = |before: u8, u: u8| loud(before) || level(before) > (DIL_SPILL_RATIO * level(u)).max(DIL_SPILL_LEVEL);
    let mut out = Vec::with_capacity(descriptor.len());
    // The DIL repeats, so the first segment comes after the last.
    let mut before = descriptor.ucodes.last().copied();
    for &u in &descriptor.ucodes {
        let spoiled = before.is_some_and(|b| spills(b, u));
        for n in 0..descriptor.segment_length(u) {
            out.push(if spoiled && n < DIL_SPILL {
                Trust::Spoiled
            } else if loud(u) {
                Trust::Loud
            } else {
                Trust::Yes
            });
        }
        before = Some(u);
    }
    out
}

/// What a DIL symbol can be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    /// Learned from, judged by, and counted.
    Yes,
    /// A codeword too loud to trust: only counted, so that the route shows
    /// what happened to it.
    Loud,
    /// The start of a segment a loud codeword spilled into: none of those.
    Spoiled,
}

/// The nearest of an interval's levels to `value`, as (Ucode, positive).
fn nearest(levels: &[(f64, u8, bool)], value: f64) -> (u8, bool) {
    levels
        .iter()
        .min_by(|a, b| (a.0 - value).abs().total_cmp(&(b.0 - value).abs()))
        .map_or((0, false), |l| (l.1, l.2))
}

/// Where the frames are, as a shift from where they were taken to be: the one
/// under which the symbols kept make the fewest numbers the digital modem
/// could not have sent. None if that is where they already are.
fn find_place(frames: &Frames) -> Option<u64> {
    let mut best: Option<(usize, u64)> = None;
    for shift in 0..INTERVALS as u64 {
        let mut current = [(0u8, false); INTERVALS];
        let mut have = 0usize;
        let mut impossible = 0usize;
        for &(index, value) in &frames.history {
            let i = ((index + shift) % INTERVALS as u64) as usize;
            current[i] = nearest(&frames.levels[i], value);
            have = if i == 0 { 1 } else if have > 0 { have + 1 } else { 0 };
            if i == INTERVALS - 1 && have == INTERVALS {
                let frame = Frame {
                    ucodes: std::array::from_fn(|k| current[k].0),
                    positive: std::array::from_fn(|k| current[k].1),
                };
                if !frames.decoder.could_have_sent(&frame) {
                    impossible += 1;
                }
            }
        }
        if best.is_none_or(|(count, _)| impossible < count) {
            best = Some((impossible, shift));
        }
    }
    best.map(|(_, shift)| shift).filter(|&shift| shift != 0)
}

fn slicer_for(levels: &Levels) -> Slicer {
    Slicer::Levels(Box::new(std::array::from_fn(|i| levels[i].iter().map(|l| l.0).collect())))
}

/// Downstream data frames: TRN2d, MP, Ed, B1d and data.
#[derive(Debug, Clone)]
struct Frames {
    from: u64,
    decoder: Decoder,
    levels: Levels,
    frame: [(u8, bool); INTERVALS],
    descrambler: Scrambler,
    finder: Finder,
    mp: Option<Mp>,
    far_acknowledged: bool,
    zero_frames: usize,
    ed: bool,
    b1d_left: usize,
    data: bool,
    /// The last symbols, as (index, value), and whether each of the last
    /// frames was one the digital modem could have sent.
    history: VecDeque<(u64, f64)>,
    impossible: VecDeque<bool>,
    /// Times the frames were found somewhere else.
    moved: u32,
    /// Data from frames that looked like Rd, kept back until the next frame
    /// shows whether Rd is what they were.
    held: VecDeque<Vec<bool>>,
}

impl Frames {
    /// Frames from symbol `from` on, read with `mapping` against `levels`,
    /// with the scrambler, differential decoder and shaper started afresh.
    fn new(from: u64, mapping: Mapping, levels: Levels, moved: u32) -> Self {
        Self {
            from,
            decoder: Decoder::new(mapping),
            levels,
            frame: [(0, false); INTERVALS],
            descrambler: Scrambler::new(Mode::Call),
            finder: Finder::new(),
            mp: None,
            far_acknowledged: false,
            zero_frames: 0,
            ed: false,
            b1d_left: 0,
            data: false,
            history: VecDeque::with_capacity(PLACE_KEPT),
            impossible: VecDeque::with_capacity(PLACE_WINDOW),
            moved,
            held: VecDeque::new(),
        }
    }
}

/// Where the analogue modem has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    SendTraining,
    AwaitSd,
    Training,
    AwaitJd,
    AwaitJdPrime,
    Dil,
    Phase4,
    Data,
    Finished,
}

/// The analogue modem, phase 3 on.
#[derive(Debug, Clone)]
pub struct Modem {
    settings: Settings,
    fs: f64,
    now: u64,
    stage: Stage,
    status: Status,
    deadline: Option<(u64, &'static str)>,
    /// B1d "within 15 s plus 5 round-trip delays after sending INFO1a".
    start_deadline: (u64, &'static str),
    /// When Sd is to have come by.
    sd_deadline: (u64, &'static str),
    /// Whether S is going out ahead of Jd.
    sending_s: bool,
    tx: Transmitter,
    source: Source,
    rx: pcm::Receiver,
    descriptor: Descriptor,
    jd: JdReader,
    far_jd: Option<Jd>,
    dil: Vec<(u8, bool)>,
    /// Whether each DIL symbol can be learned from and judged by: not in a
    /// segment of a codeword too loud to trust, and not at the start of the
    /// segment after one, which what the loud one did spills into.
    dil_trusted: Vec<Trust>,
    /// The DIL as it is read (9.3.2.9): the receiver's count at its first
    /// symbol, moved by any slip since; the frame interval that symbol was
    /// in; which symbols have been read, and how many are left.
    dil_base: i64,
    dil_interval: usize,
    dil_read: Vec<bool>,
    dil_left: usize,
    /// Symbols not yet counted, and since a slip was noticed, how many have
    /// been gathered to find where the DIL went.
    dil_recent: VecDeque<(u64, f64)>,
    dil_lost: Option<usize>,
    dil_moved: u32,
    /// Symbols while J'd is awaited, for finding the DIL if J'd goes unread,
    /// and whether it was found that way.
    before_dil: VecDeque<(u64, f64)>,
    dil_found_late: bool,
    jd_gone: bool,
    /// Times R showed the frames had moved.
    r_moved: u32,
    analysis: Analysis,
    route: Option<Route>,
    choice: Option<Choice>,
    r_watch: RWatch,
    trn2d_from: Option<u64>,
    frames: Option<Frames>,
    received: Vec<bool>,
    upstream_rate: u32,
    downstream_rate: u32,
    /// The last two symbols as the equaliser gave them, for the scope.
    last: [f64; 2],
    /// The last symbol whole, for anything reading a call back.
    last_symbol: Option<pcm::Symbol>,
    heard_any: bool,
    /// The digital modem's tone B, which starts a retrain (9.5.2.2), and
    /// whether one is wanted.
    retrain_watch: RetrainWatch,
    wants_retrain: bool,
    /// Holes in the audio seen in data mode (see [`HOLE`]).
    holes: u32,
    /// Since the receiver last held a place, in samples.
    lost_since: Option<u64>,
    /// The CP data mode is running on, which Rd and a renegotiation's
    /// shaping are taken from.
    in_use: Option<Cp>,
    /// Rd and its turn, in data mode and a renegotiation (9.6.2).
    rd_watch: RWatch,
    /// Whether the digital modem is still sending, in data mode.
    far_end: super::carrier::Watch,
    far_end_went: bool,
    renegotiating: bool,
    /// Whether this end began the renegotiation, and whether R-bar-d is
    /// still to come in it.
    initiated: bool,
    awaiting_turn: bool,
    clearing: bool,
    renegotiations: u32,
    /// The least distance between data mode's levels as the route delivers
    /// them, when the margin is next looked at, and how data mode's
    /// decisions are going.
    least_gap: f64,
    margin_at: u64,
    decisions: Decisions,
    /// Looks in a row leaving the levels short of margin (see [`MARGIN`]).
    short: u32,
    /// How much worse than the DIL showed data mode has found the line.
    worse: f64,
    /// The spectral shaping asked for, and the share of the DIL's error
    /// power it was expected to leave (5.4.5).
    shaping: (Shaping, f64),
    /// What the transcript is to be told, a line each, not yet taken.
    notes: Vec<String>,
}

impl Modem {
    /// Phase 3 from its start: the moment INFO1a has gone.
    pub fn new(settings: Settings, fs: f64) -> Self {
        let trn_length = (PHASE3_TRN * settings.upstream.baud()) as usize;
        let mut source = Source::new(trn_length);
        let silence = (SILENCE_BEFORE_S * settings.upstream.baud()).round() as usize;
        source.hold = silence.saturating_sub(Transmitter::lookahead());
        source.after_s_bar = Up::Pp;
        source.change(Up::S);
        let descriptor = dil::design(settings.law, settings.uinfo);
        source.ja = descriptor.to_bits();
        // Nothing to hunt for until Ja: before then, the digital modem's
        // tone from phase 2 can still be arriving, and a tone near 1333 Hz
        // looks enough like Sd to set a hunt off.
        let rx = pcm::Receiver::new(settings.law, fs);
        let mut modem = Self {
            settings,
            fs,
            now: 0,
            stage: Stage::SendTraining,
            status: Status::Running,
            deadline: None,
            start_deadline: (0, ""),
            sd_deadline: (u64::MAX, ""),
            sending_s: false,
            tx: Transmitter::new(settings.upstream, settings.pre_emphasis, settings.power_reduction, fs),
            source,
            rx,
            dil: descriptor.symbols().collect(),
            dil_trusted: trusted_symbols(&descriptor, settings.law),
            descriptor,
            jd: JdReader::new(),
            far_jd: None,
            dil_base: 0,
            dil_interval: 0,
            dil_read: Vec::new(),
            dil_left: 0,
            dil_recent: VecDeque::new(),
            dil_lost: None,
            dil_moved: 0,
            before_dil: VecDeque::new(),
            dil_found_late: false,
            jd_gone: false,
            r_moved: 0,
            analysis: Analysis::new(),
            route: None,
            choice: None,
            r_watch: RWatch::default(),
            trn2d_from: None,
            frames: None,
            received: Vec::new(),
            upstream_rate: 0,
            downstream_rate: 0,
            last: [0.0; 2],
            last_symbol: None,
            heard_any: false,
            // The digital modem takes V.34's call side, and tone B is its.
            retrain_watch: RetrainWatch::new(Role::Call, fs),
            wants_retrain: false,
            holes: 0,
            lost_since: None,
            in_use: None,
            rd_watch: RWatch::default(),
            far_end: super::carrier::Watch::new(fs),
            far_end_went: false,
            renegotiating: false,
            initiated: false,
            awaiting_turn: false,
            clearing: false,
            renegotiations: 0,
            least_gap: f64::INFINITY,
            margin_at: 0,
            decisions: Decisions::default(),
            short: 0,
            worse: 1.0,
            shaping: (Shaping::NONE, 1.0),
            notes: Vec::new(),
        };
        // 9.4.2: B1d "within 15 s plus 5 round-trip delays after sending
        // INFO1a".
        modem.dil_left = modem.dil.len();
        modem.start_deadline = (modem.samples(15.0 + 5.0 * settings.round_trip), "no B1d from the digital modem");
        modem.deadline = Some(modem.start_deadline);
        modem
    }

    fn samples(&self, seconds: f64) -> u64 {
        self.now + (seconds * self.fs).round() as u64
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// Whether the digital modem's signal is there: in data mode, or in a
    /// renegotiation begun from it.
    pub fn carrier(&self) -> bool {
        self.watching().is_some()
    }

    /// Whether the call ended because the digital modem stopped sending.
    pub fn far_end_went(&self) -> bool {
        self.far_end_went
    }

    /// Whether the far end's level is being watched, and whether what
    /// arrives is data mode's to learn from: in data mode, and in phase 4
    /// again from it -- a renegotiation, and the moment after one.
    fn watching(&self) -> Option<bool> {
        match self.status {
            Status::Connected { .. } => Some(!self.renegotiating),
            Status::Running if self.renegotiations > 0 => Some(false),
            _ => None,
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn phase(&self) -> &'static str {
        match self.stage {
            Stage::SendTraining | Stage::AwaitSd | Stage::Training => "V.90 phase 3: training",
            Stage::AwaitJd | Stage::AwaitJdPrime => "V.90 phase 3: Jd",
            Stage::Dil => "V.90 phase 3: DIL",
            Stage::Phase4 => "V.90 phase 4",
            Stage::Data if self.renegotiating => "V.90 rate renegotiation",
            Stage::Data => "V.90 data",
            Stage::Finished => "V.90 finished",
        }
    }

    /// The downstream receiver, for looking at.
    pub fn receiver(&self) -> &pcm::Receiver {
        &self.rx
    }

    /// The last two downstream symbols, each against the one after it, for a
    /// scope: there is no plane to plot PCM in, but a sample set against the
    /// next one lays the levels out on a grid of its own. Scaled so the
    /// loudest level in use is one.
    pub fn pair(&self) -> Option<(f64, f64)> {
        self.heard_any.then(|| (self.last[0] / self.scale(), self.last[1] / self.scale()))
    }

    /// What the scope's one is.
    fn scale(&self) -> f64 {
        let law = self.settings.law;
        let loudest = |cp: &Cp| (0..INTERVALS).flat_map(|i| cp.points(i)).map(|u| ucode::level(law, u)).fold(0.0, f64::max);
        match (self.frames.as_ref(), self.choice.as_ref()) {
            (Some(f), Some(c)) if f.ed => loudest(&c.data),
            (Some(_), Some(c)) => loudest(&c.training),
            _ => 1.5 * ucode::level(law, self.settings.uinfo),
        }
        .max(1e-6)
    }

    /// Signed levels in the constellation being read, for a scope's legend.
    pub fn points(&self) -> usize {
        match (self.frames.as_ref(), self.choice.as_ref()) {
            (Some(f), Some(c)) if f.ed => 2 * c.data.points(0).len(),
            (Some(_), Some(c)) => 2 * c.training.points(0).len(),
            _ => 2,
        }
    }

    /// The last downstream symbol the receiver gave, and which stage of the
    /// start-up read it: for reading a call back.
    pub fn last_symbol(&self) -> Option<(pcm::Symbol, &'static str)> {
        self.last_symbol.map(|s| (s, self.phase()))
    }

    /// Where the DIL's first symbol is, as the receiver counts, and the
    /// frame interval it is in.
    pub fn dil_start(&self) -> (i64, usize) {
        (self.dil_base, self.dil_interval)
    }

    /// The DIL this end asked for.
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// The digital modem's Jd.
    pub fn far_jd(&self) -> Option<Jd> {
        self.far_jd
    }

    /// What the DIL showed of the route.
    pub fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// What this end asked for.
    pub fn choice(&self) -> Option<&Choice> {
        self.choice.as_ref()
    }

    /// The spectral shaping this end asked for, and the share of the DIL's
    /// error power it expected the shaping to leave.
    pub fn shaping(&self) -> (Shaping, f64) {
        self.shaping
    }

    /// The digital modem's MP.
    pub fn far_mp(&self) -> Option<Mp> {
        self.frames.as_ref().and_then(|f| f.mp)
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.received)
    }

    /// What phase 4 has done since this was last asked, a line each for the
    /// transcript: every different CP sent and MP found, E sent, Ed and B1d
    /// found, and why the start-up or the rate stopped being what it was. The
    /// time is the caller's to put on them; each is made on the sample that
    /// did it.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// Whether V.90's phase 2 should be run again: read once, and cleared.
    pub fn take_retrain(&mut self) -> bool {
        std::mem::take(&mut self.wants_retrain)
    }

    /// Start a retrain (9.5.2.1).
    pub fn start_retrain(&mut self) {
        self.wants_retrain = true;
    }

    /// Rate renegotiations and cleardowns since the call began, from either
    /// end.
    pub fn renegotiations(&self) -> u32 {
        self.renegotiations
    }

    /// Holes in the audio seen since data mode began (see [`HOLE`]).
    pub fn holes(&self) -> u32 {
        self.holes
    }

    /// Start a rate renegotiation from data mode (9.6.2.1), asking for the
    /// fastest downstream the route carries at no more than `most` bit/s.
    /// False, and nothing done, outside data mode or if the route carries no
    /// rate that slow.
    pub fn renegotiate(&mut self, most: u32) -> bool {
        self.renegotiate_within(most, 0)
    }

    /// The same, and not below `least` if believing less of what data mode
    /// found will keep it there.
    fn renegotiate_within(&mut self, most: u32, least: u32) -> bool {
        if !self.in_data_mode() {
            return false;
        }
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return false };
        let law = self.settings.law;
        let limit = super::power_limit(&self.settings.server);
        let jd = self.far_jd.unwrap_or_default();
        let slow_enough = |drn: u8| jd.enables(drn) && sequences::data_rate(drn).is_some_and(|rate| rate <= most);
        // The shaping asked for at the start, and the errors it was expected
        // to leave, and then as much worse as data mode has found the line --
        // or as much of that as keeps the step inside the cap.
        let (shaping, left) = self.shaping;
        let mut worse = self.worse;
        let new = loop {
            let scaled = shaping::scaled(route, left * worse * worse);
            let Some(new) = dil::choose_shaped(&scaled, law, limit, slow_enough, shaping) else { return false };
            if worse <= 1.0 || sequences::data_rate(new.data.drn).is_some_and(|rate| rate >= least) {
                break new;
            }
            worse = (worse / BELIEVED_LESS).max(1.0);
        };
        let mut data = new.data;
        self.finish_cp(&mut data);
        let training = choice.training.clone();
        self.choice = Some(Choice { data, training });
        self.begin_renegotiation(true);
        true
    }

    /// End the call from data mode (9.7): a renegotiation whose CP asks for
    /// nothing. False, and nothing done, outside data mode.
    pub fn clear_down(&mut self) -> bool {
        if !self.in_data_mode() {
            return false;
        }
        let Some(choice) = self.choice.as_mut() else { return false };
        choice.data.drn = 0;
        self.clearing = true;
        self.begin_renegotiation(true);
        true
    }

    fn in_data_mode(&self) -> bool {
        matches!(self.status, Status::Connected { .. }) && !self.renegotiating
    }

    /// Back from data mode to phase 4 (9.6.2.1.1, 9.6.2.2.1).
    fn begin_renegotiation(&mut self, initiating: bool) {
        self.notes.push(
            if initiating { "rate renegotiation, begun by this end" } else { "rate renegotiation, begun by the digital modem's Rd" }.into(),
        );
        self.renegotiations += 1;
        self.renegotiating = true;
        self.initiated = initiating;
        self.awaiting_turn = true;
        self.status = Status::Running;
        if initiating {
            // The digital modem's data is data until its Rd.
            self.rd_watch = RWatch::default();
            self.send_s_then_cp();
        } else {
            self.clamp();
        }
        let wait = RENEGOTIATION_ED + 2.0 * self.settings.round_trip + 0.1;
        self.deadline = Some((self.samples(wait), "no Ed in the rate renegotiation"));
    }

    /// Rd: circuit 104 clamped, and what was held back for it dropped.
    fn clamp(&mut self) {
        if let Some(frames) = self.frames.as_mut() {
            frames.data = false;
            frames.held.clear();
        }
    }

    /// S for 128T, S-bar for 16T, and CP (9.6.2.1.1 to 9.6.2.1.3).
    fn send_s_then_cp(&mut self) {
        let Some(choice) = self.choice.as_ref() else { return };
        let sixteen = self.far_jd.is_some_and(|jd| jd.sixteen_in_renegotiation);
        let source = &mut self.source;
        source.s_length = Some(signals::S_SYMBOLS);
        source.after_s_bar = Up::Cp;
        source.cp = choice.data.to_bits();
        source.cp_is_ack = false;
        source.next_cp = None;
        source.cps = 0;
        source.acknowledged = 0;
        source.restarted = false;
        source.size = if sixteen { Size::Sixteen } else { Size::Four };
        source.encoder = None;
        source.change(Up::S);
    }

    /// R-bar-d has begun: TRN2d, MP and Ed start at `from`, on CPt's
    /// constellations with data mode's shaping (8.6).
    fn turned(&mut self, from: u64) {
        self.awaiting_turn = false;
        let (Some(route), Some(choice), Some(in_use)) = (self.route.as_ref(), self.choice.as_ref(), self.in_use.as_ref()) else {
            return;
        };
        let Some(mapping) = Mapping::for_renegotiation(&choice.training, in_use) else {
            self.fail("the renegotiation has no mapping to train on");
            return;
        };
        let levels = levels_for(&choice.training, route);
        let moved = self.frames.as_ref().map_or(0, |f| f.moved);
        self.frames = Some(Frames::new(from, mapping, levels, moved));
        if !self.initiated {
            // 9.6.2.2.2: "transmit S for 128T".
            self.send_s_then_cp();
        }
    }

    /// R has shown the frames arriving `late` symbols later than they were
    /// taken to.
    fn move_frames(&mut self, late: usize) {
        let offset = (self.rx.frame_offset() + (INTERVALS - late) as u64) % INTERVALS as u64;
        self.rx.set_frame_offset(offset);
        self.r_moved += 1;
        if let Some(frames) = self.frames.as_mut() {
            frames.impossible.clear();
            frames.history.clear();
        }
    }

    /// Rd's level in each interval: "the highest power PCM codeword from the
    /// data mode constellation" (8.6.4), as the route delivers it.
    fn rd_levels(&self) -> [f64; INTERVALS] {
        let (Some(cp), Some(route)) = (self.in_use.as_ref(), self.route.as_ref()) else {
            return [f64::INFINITY; INTERVALS];
        };
        std::array::from_fn(|i| cp.points(i).last().map_or(f64::INFINITY, |&u| route.levels[i][usize::from(u)]))
    }

    /// One end has asked for nothing: the call is over (9.7).
    fn cleared_down(&mut self) {
        self.status = Status::ClearedDown;
        self.stage = Stage::Finished;
        self.renegotiating = false;
        self.source.pending = None;
        self.source.start(Up::Silence);
        self.rx.idle();
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        self.source.data.extend(bits.iter().copied());
    }

    /// Data waiting to go, less what the next mapping frame takes at once.
    pub fn pending_bits(&self) -> usize {
        let frame = self.source.encoder.as_ref().map_or(0, |e| e.params().framing.b);
        self.source.data.len().saturating_sub(frame)
    }

    fn fail(&mut self, why: &'static str) {
        self.notes.push(format!("failed: {why}"));
        self.status = Status::Failed(why);
        self.stage = Stage::Finished;
        self.source.pending = None;
        self.source.start(Up::Silence);
        self.rx.idle();
    }

    /// One line sample in, one out.
    pub fn step(&mut self, line: f64) -> f64 {
        self.now += 1;
        self.rx.feed(line);
        match self.watching() {
            Some(learn) => {
                self.far_end.feed(line, learn);
                if self.far_end.quiet() {
                    self.decisions.spoil();
                }
                if self.far_end.gone() {
                    // A far end that has hung up says nothing first. Nothing
                    // more goes to it, and the call is over, as if it had
                    // cleared down: a retrain would only call into silence.
                    self.far_end_went = true;
                    self.notes.push("the digital modem stopped sending: the call is over".into());
                    self.cleared_down();
                }
            }
            None => self.far_end.reset(),
        }
        // 9.3.2, 9.4.2 and 9.6.2: tone B, in phase 3, phase 4 or data mode, is
        // the digital modem retraining.
        if self.stage != Stage::Finished && self.retrain_watch.feed(line, self.fs) {
            if !self.wants_retrain {
                self.notes.push("tone B: the digital modem is retraining".into());
            }
            self.wants_retrain = true;
        }
        // A receiver that has held still for three seconds is not going to
        // find its place again: 9.5.2.1, "The analogue modem may initiate a
        // retrain at any time".
        match (self.rx.is_lost(), self.lost_since) {
            (true, None) => self.lost_since = Some(self.now),
            (false, Some(_)) => self.lost_since = None,
            (true, Some(since))
                if self.now - since > (3.0 * self.fs) as u64 && self.stage == Stage::Data && !self.renegotiating =>
            {
                self.wants_retrain = true;
                self.lost_since = None;
            }
            _ => {}
        }
        while let Some(heard) = self.rx.heard() {
            if self.stage != Stage::Finished {
                self.heard(heard);
            }
        }
        if let Some((at, why)) = self.deadline
            && self.now > at
            && self.status == Status::Running
        {
            if self.renegotiating {
                // 9.6.2: a renegotiation that goes nowhere is a retrain.
                self.deadline = None;
                self.wants_retrain = true;
            } else {
                self.fail(why);
            }
        }
        self.stage_step();
        let source = &mut self.source;
        let out = self.tx.next_sample(|| source.next());
        match self.source.began.take() {
            Some(Began::Cp(bits)) => {
                if let Some(cp) = Cp::from_bits(&bits) {
                    self.notes.push(format!("sent {}", describe_cp(&cp)));
                }
            }
            Some(Began::E) => self.notes.push(format!("sent E: B1 and data next, up at {} bit/s", self.upstream_rate)),
            None => {}
        }
        out
    }

    fn stage_step(&mut self) {
        match self.stage {
            Stage::SendTraining if self.source.up == Up::Ja => {
                self.stage = Stage::AwaitSd;
                self.rx.hunt(self.settings.uinfo);
                // 9.3.2.4: S-bar-d within 1500 ms of the start of Ja.
                self.sd_deadline = (self.samples(SD_WAIT + 2.0 * self.settings.round_trip), "no Sd from the digital modem");
                self.deadline = Some(self.sd_deadline);
            }
            Stage::Data if self.clearing => {
                if self.source.up == Up::Cp && self.source.cps >= CLEARDOWN_CPS {
                    self.cleared_down();
                }
            }
            Stage::Phase4 | Stage::Data => {
                // 9.4.2.4: a CP' sent, and MP' or Ed heard: E once the
                // current CP' is whole.
                let heard_back = !self.awaiting_turn && self.frames.as_ref().is_some_and(|f| f.far_acknowledged || f.ed);
                if self.source.up == Up::Cp && self.source.acknowledged >= 1 && heard_back && self.source.pending.is_none() {
                    self.prepare_upstream();
                    self.source.change(Up::E);
                }
                let receiving = self.frames.as_ref().is_some_and(|f| f.data);
                if self.status == Status::Running && !self.renegotiating && receiving && self.source.up == Up::Data {
                    self.status = Status::Connected { downstream: self.downstream_rate, upstream: self.upstream_rate };
                    self.deadline = None;
                    self.margin_at = self.samples(MARGIN_SETTLE);
                    self.short = 0;
                    if let Some(frames) = self.frames.as_ref() {
                        self.decisions = Decisions::new(&frames.levels);
                    }
                }
                if self.in_data_mode() && self.now >= self.margin_at {
                    self.margin_at = self.samples(MARGIN_EVERY);
                    self.watch_margin();
                }
            }
            _ => {}
        }
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Reversal { .. } => {
                if self.stage == Stage::AwaitSd {
                    // 9.3.2.4: "terminate Ja and transmit silence".
                    self.source.change(Up::Silence);
                    self.stage = Stage::Training;
                    self.deadline = Some((self.samples(4.5 + self.settings.round_trip), "no Jd from the digital modem"));
                }
            }
            Heard::Trained { .. } => {
                if self.stage == Stage::Training {
                    self.stage = Stage::AwaitJd;
                }
            }
            Heard::Untrained if self.stage == Stage::Training && self.now < self.sd_deadline.0 => {
                // Something that was not Sd set the hunt off: back to Ja
                // and the hunt, while Sd can still come.
                self.stage = Stage::AwaitSd;
                self.source.change(Up::Ja);
                self.rx.hunt(self.settings.uinfo);
                self.deadline = Some(self.sd_deadline);
            }
            Heard::Untrained => self.fail("the digital modem's TRN1d did not train this end"),
            Heard::Symbol(symbol) => self.symbol(symbol),
            Heard::Lost if self.stage == Stage::Dil && self.dil_lost.is_none() => {
                // What arrived just before the loss was noticed is held with
                // the rest until it is known whether anything moved.
                self.dil_lost = Some(0);
            }
            // A slip, or something like one: the receiver holds its loops,
            // and where the frames went is worked out from what comes after.
            Heard::Lost | Heard::Found => {}
        }
    }

    fn symbol(&mut self, symbol: pcm::Symbol) {
        self.last = [self.last[1], symbol.value];
        self.last_symbol = Some(symbol);
        self.heard_any = true;
        match self.stage {
            Stage::AwaitJd | Stage::AwaitJdPrime => {
                let jd_prime = self.jd.feed(symbol.index, symbol.positive());
                let early = (JD_LATEST + S_AFTER_JD - self.settings.round_trip).max(0.0) * digital::FS;
                if self.stage == Stage::AwaitJd && !self.sending_s && symbol.raw as f64 >= early {
                    self.send_s();
                }
                if self.stage == Stage::AwaitJd
                    && let Some((_, jd)) = self.jd.last
                {
                    // 9.3.2.7: S, and listen for J'd.
                    self.far_jd = Some(jd);
                    self.source.size = if jd.sixteen_in_training { Size::Sixteen } else { Size::Four };
                    if !self.sending_s {
                        self.send_s();
                    }
                    self.stage = Stage::AwaitJdPrime;
                    self.deadline = Some(self.start_deadline);
                }
                if self.stage != Stage::AwaitJdPrime {
                    return;
                }
                if jd_prime {
                    self.begin_dil(symbol.raw + 1, &[]);
                    return;
                }
                self.before_dil.push_back((symbol.raw, symbol.value));
                if self.before_dil.len() > DIL_START_KEPT {
                    self.before_dil.pop_front();
                }
                // A Jd stream that has stopped with no J'd read: the DIL is
                // under way, and is looked for in what has arrived.
                let gone = self.jd.last.is_some_and(|(end, _)| symbol.index + 1 > end + JD_GONE);
                if gone && !self.jd_gone {
                    // Whatever is arriving is not two levels any more, and
                    // learning from it as if it were would ruin the
                    // equaliser before the DIL is found.
                    self.jd_gone = true;
                    self.rx.set_slicer(Slicer::Free);
                }
                if gone && self.before_dil.len() >= DIL_START_WINDOW + INTERVALS && symbol.raw.is_multiple_of(64) {
                    self.find_dil_start();
                }
            }
            Stage::Dil => self.dil_symbol(symbol.raw, symbol.value),
            Stage::Phase4 | Stage::Data => self.phase4_symbol(symbol),
            _ => {}
        }
    }

    /// S until J'd (9.3.2.7).
    fn send_s(&mut self) {
        self.sending_s = true;
        self.source.s_length = None;
        self.source.change(Up::S);
    }

    /// The DIL from the receiver's count `first` (9.3.2.8): S-bar for 16T,
    /// the frames put where the DIL says they are, and the symbols in
    /// `already` read as its first.
    fn begin_dil(&mut self, first: u64, already: &[(u64, f64)]) {
        self.source.after_s_bar = Up::Silence;
        self.source.change(Up::SBar);
        self.stage = Stage::Dil;
        self.dil_base = first as i64;
        // J'd ends on a frame boundary -- Jd does, and J'd is two frames --
        // so the DIL's first symbol is in interval 0, whatever a slip did to
        // the frames on the way.
        self.dil_interval = 0;
        self.rx.set_frame_offset((INTERVALS as u64 - first % INTERVALS as u64) % INTERVALS as u64);
        self.dil_read = vec![false; self.dil.len()];
        self.dil_left = self.dil.len();
        self.dil_recent.clear();
        self.dil_lost = None;
        self.before_dil.clear();
        let next = already.last().map_or(first, |s| s.0 + 1);
        // Two passes: one, and what a slip loses of it read again.
        let levels = self.dil_levels(next as i64, 2 * self.dil.len());
        self.rx.expect_from(next, levels);
        for &(raw, value) in already {
            self.dil_symbol(raw, value);
        }
    }

    /// The DIL's start, from what has arrived since J'd was due: where the
    /// DIL fits what came after it closely and far better than anywhere else.
    fn find_dil_start(&mut self) {
        let arrived: Vec<(u64, f64)> = self.before_dil.iter().copied().collect();
        let Some(&(newest, _)) = arrived.last() else { return };
        let oldest = arrived[0].0;
        let mut fits = Vec::new();
        for first in oldest..=newest.saturating_sub(DIL_START_WINDOW as u64) {
            let from = (first - oldest) as usize;
            let window = &arrived[from..(from + DIL_START_WINDOW).min(arrived.len())];
            if let Some((fit, agree)) = self.fit_dil(window, first as i64)
                && agree >= DIL_SIGNS
            {
                fits.push((first, fit));
            }
        }
        let Some(&(first, fit)) = fits.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { return };
        let next = fits.iter().filter(|f| f.0.abs_diff(first) > 1).map(|f| f.1).fold(f64::INFINITY, f64::min);
        if fit > DIL_FIT || next < 4.0 * fit {
            return;
        }
        self.dil_found_late = true;
        let from = (first - oldest) as usize;
        self.begin_dil(first, &arrived[from..]);
    }

    /// How well `window` fits the DIL taken to start at the receiver's count
    /// `first`, over the symbols it can be judged on: the error's power
    /// against the DIL's, and the share of signs that agree. None if too few
    /// of the symbols are training symbols to tell one segment from another:
    /// every segment's references are alike.
    fn fit_dil(&self, window: &[(u64, f64)], first: i64) -> Option<(f64, f64)> {
        let law = self.settings.law;
        let len = self.dil.len() as i64;
        let (mut cost, mut power, mut agree, mut judged, mut trained) = (0.0, 0.0, 0usize, 0usize, 0usize);
        for &(raw, v) in window {
            let at = (raw as i64 - first).rem_euclid(len) as usize;
            let (u, positive) = self.dil[at];
            let level = ucode::level(law, u);
            // Too quiet for a sign to mean anything, or too near a loud
            // codeword to trust.
            if self.dil_trusted[at] != Trust::Yes || level < 0.004 {
                continue;
            }
            let e = if positive { level } else { -level };
            cost += (v - e).powi(2);
            power += e * e;
            judged += 1;
            if u != self.settings.uinfo {
                trained += 1;
            }
            if (v >= 0.0) == positive {
                agree += 1;
            }
        }
        (trained >= DIL_TRAINED_JUDGED && power > 0.0).then(|| (cost / power, agree as f64 / judged as f64))
    }

    /// Whether the DIL had to be found without J'd.
    pub fn dil_found_late(&self) -> bool {
        self.dil_found_late
    }

    /// The DIL's signed levels from the receiver's count `from`, for `n`
    /// symbols, as the DIL now stands against that count: NaN where a level
    /// is too loud to learn from.
    fn dil_levels(&self, from: i64, n: usize) -> Vec<f64> {
        let law = self.settings.law;
        let len = self.dil.len() as i64;
        (0..n as i64)
            .map(|k| {
                let at = (from + k - self.dil_base).rem_euclid(len) as usize;
                let (u, positive) = self.dil[at];
                let level = ucode::level(law, u);
                if self.dil_trusted[at] != Trust::Yes {
                    f64::NAN
                } else if positive {
                    level
                } else {
                    -level
                }
            })
            .collect()
    }

    /// Times a slip moved the DIL and it was found again.
    pub fn dil_moved(&self) -> u32 {
        self.dil_moved
    }

    /// How much of the DIL has been read, of how much there is, and whether
    /// the reading is waiting to find where it went.
    pub fn dil_progress(&self) -> (usize, usize, bool) {
        (self.dil.len() - self.dil_left, self.dil.len(), self.dil_lost.is_some())
    }

    fn dil_symbol(&mut self, raw: u64, value: f64) {
        self.dil_recent.push_back((raw, value));
        if let Some(gathered) = self.dil_lost {
            if self.dil_recent.len() > DIL_KEPT_LOST {
                self.dil_recent.pop_front();
            }
            self.dil_lost = Some(gathered + 1);
            if gathered + 1 >= DIL_FIRST_LOOK && (gathered + 1).is_multiple_of(DIL_LOOK_EVERY) {
                self.find_dil();
            }
            return;
        }
        while self.dil_recent.len() > DIL_DELAY {
            let Some((raw, value)) = self.dil_recent.pop_front() else { break };
            self.count_dil(raw, value);
            if self.stage != Stage::Dil {
                return;
            }
        }
    }

    /// One DIL symbol read, wherever in the DIL it falls: the DIL repeats,
    /// so a symbol a slip spoiled comes round again.
    fn count_dil(&mut self, raw: u64, value: f64) {
        let len = self.dil.len() as i64;
        let at = (raw as i64 - self.dil_base).rem_euclid(len) as usize;
        if self.dil_read[at] {
            return;
        }
        self.dil_read[at] = true;
        self.dil_left -= 1;
        let (u, positive) = self.dil[at];
        if self.dil_trusted[at] != Trust::Spoiled {
            self.analysis.feed(u, positive, (at + self.dil_interval) % INTERVALS, value);
        }
        if self.dil_left == 0 {
            self.finish_dil();
        }
    }

    /// Where the DIL went after a loss: the move that makes what has arrived
    /// lately most like it, once one does so clearly -- closely, and far
    /// better than any other. No move at all, if that fits: a route that
    /// robs a bit, or a burst of noise, spoils the reading without moving
    /// anything.
    fn find_dil(&mut self) {
        let window: Vec<(u64, f64)> = self.dil_recent.iter().skip(self.dil_recent.len().saturating_sub(DIL_SEARCH)).copied().collect();
        // (move, relative error) for each move whose signs agree.
        let fits: Vec<(i64, f64)> = (-DIL_MOST_MOVED..=DIL_MOST_MOVED)
            .filter_map(|m| {
                let (fit, agree) = self.fit_dil(&window, self.dil_base + m)?;
                (agree >= DIL_SIGNS).then_some((m, fit))
            })
            .collect();
        let Some(&(moved, fit)) = fits.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { return };
        let next = fits.iter().filter(|f| (f.0 - moved).abs() > 1).map(|f| f.1).fold(f64::INFINITY, f64::min);
        // A move is only a move against staying put. Where the stretch is
        // too quiet to say whether it is where it was, everything since the
        // loss is asked instead -- waiting for a louder stretch can wait
        // until the DIL has wrapped round to its own quiet start, where the
        // true move cannot be judged either -- and failing that, wait.
        if moved != 0 && !fits.iter().any(|f| f.0 == 0) && self.fit_dil(&window, self.dil_base).is_none() {
            let gathered = self.dil_lost.unwrap_or(0);
            let since: Vec<(u64, f64)> = self.dil_recent.iter().skip(self.dil_recent.len().saturating_sub(gathered)).copied().collect();
            match self.fit_dil(&since, self.dil_base) {
                Some((fit, agree)) if fit > DIL_FIT || agree < DIL_SIGNS => {}
                _ => return,
            }
        }
        if fit > DIL_FIT || (moved != 0 && next < 4.0 * fit) {
            return;
        }
        self.dil_lost = None;
        // Nothing moved: everything held is good. A move: only what it was
        // found from is sure to be past the slip, and the next pass has the
        // rest.
        let recent: Vec<(u64, f64)> = if moved == 0 { self.dil_recent.drain(..).collect() } else { window };
        self.dil_recent.clear();
        let next = recent.last().map_or(0, |r| r.0 as i64 + 1);
        if moved != 0 {
            self.dil_moved += 1;
            self.dil_base += moved;
            // The frames moved with it.
            let offset = (self.rx.frame_offset() as i64 - moved).rem_euclid(INTERVALS as i64) as u64;
            self.rx.set_frame_offset(offset);
        }
        let levels = self.dil_levels(next, 2 * self.dil.len());
        self.rx.expect_from(next as u64, levels);
        for (raw, value) in recent {
            self.count_dil(raw, value);
            if self.stage != Stage::Dil {
                return;
            }
        }
    }

    /// A whole pass of the DIL is in: choose, and say so (9.3.2.10).
    fn finish_dil(&mut self) {
        let route = self.analysis.route();
        let law = self.settings.law;
        let limit = super::power_limit(&self.settings.server);
        let jd = self.far_jd.unwrap_or_default();
        // Shaped or not, whichever carries more (5.4.5): what the equaliser
        // left of TRN1d and Jd says what shaping would take away.
        let leftover = self.rx.residue().leftover();
        let Some(asked) = shaping::choose(&route, law, limit, |drn| jd.enables(drn), jd.lookahead, leftover.as_ref()) else {
            self.route = Some(route);
            self.fail("the route cannot carry V.90's slowest rate");
            return;
        };
        self.shaping = (asked.shaping, asked.left);
        let mut choice = asked.choice;
        let rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
        if rate < self.settings.v34_receive {
            // A route that is an ordinary line with G.711's noise on it --
            // a softphone that converted the sample rate on the way to its
            // encoder -- carries V.34 at least as well.
            self.route = Some(route);
            self.fail("V.34 carries more than V.90 on this route");
            return;
        }
        self.finish_cp(&mut choice.data);
        self.finish_cp(&mut choice.training);
        self.downstream_rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
        // "S for 128T followed by S-bar for 16T", and phase 4: CPt.
        self.source.s_length = Some(signals::S_SYMBOLS);
        self.source.after_s_bar = Up::Cp;
        self.source.cp = choice.training.to_bits();
        self.source.cp_is_ack = false;
        self.source.change(Up::S);
        self.route = Some(route);
        self.choice = Some(choice);
        // What comes down now is more DIL, and then R: nothing to learn from
        // until R is sure.
        self.rx.set_slicer(Slicer::Free);
        self.stage = Stage::Phase4;
    }

    fn phase4_symbol(&mut self, symbol: pcm::Symbol) {
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return };
        if self.trn2d_from.is_none() {
            let level = ucode::level(self.settings.law, self.settings.uinfo);
            let was_heard = self.r_watch.heard;
            let seen = self.r_watch.feed(&symbol, &[level; INTERVALS]);
            if let RSeen::Moved(late) = seen {
                self.move_frames(late);
                return;
            }
            if let RSeen::Turned(from) = seen {
                // 9.4.2.2: the current CPt whole, then CP.
                self.trn2d_from = Some(from);
                self.source.next_cp = Some((choice.data.to_bits(), false));
                let Some(training) = Mapping::from_cp(&choice.training) else {
                    self.fail("this end's CPt is not a mapping");
                    return;
                };
                let levels = levels_for(&choice.training, route);
                self.frames = Some(Frames::new(from, training, levels, 0));
            } else if self.r_watch.heard && !was_heard {
                self.rx.set_slicer(Slicer::Binary(level));
            }
            return;
        }
        // Data mode, and a renegotiation until R-bar-d: Rd, from either end's
        // asking (9.6.2.1.4, 9.6.2.2.1).
        let receiving = self.frames.as_ref().is_some_and(|f| f.data);
        let mut looked = false;
        if self.stage == Stage::Data && (receiving || self.awaiting_turn) {
            let levels = self.rd_levels();
            let was_heard = self.rd_watch.heard;
            match self.rd_watch.feed(&symbol, &levels) {
                RSeen::Turned(from) => {
                    self.turned(from);
                    return;
                }
                RSeen::Moved(late) => {
                    self.move_frames(late);
                    return;
                }
                RSeen::Nothing => {}
            }
            if self.rd_watch.heard {
                if !was_heard {
                    if self.renegotiating {
                        self.clamp();
                    } else {
                        self.begin_renegotiation(false);
                    }
                }
                return;
            }
            looked = self.rd_watch.looked && symbol.interval() == INTERVALS - 1;
        }
        let (Some(route), Some(choice)) = (self.route.as_ref(), self.choice.as_ref()) else { return };
        let Some(frames) = self.frames.as_mut() else { return };
        if symbol.index + 1 == frames.from {
            // TRN2d's first symbol is next: decide against CPt's levels.
            self.rx.set_slicer(slicer_for(&frames.levels));
            return;
        }
        if symbol.index < frames.from {
            return;
        }
        let i = symbol.interval();
        frames.frame[i] = nearest(&frames.levels[i], symbol.value);
        if frames.data && self.stage == Stage::Data && !self.renegotiating {
            self.decisions.symbol(i, symbol.value);
            self.holes += u32::from(self.decisions.line(symbol.line));
        }
        if frames.history.len() == PLACE_KEPT {
            frames.history.pop_front();
        }
        frames.history.push_back((symbol.index, symbol.value));
        if i != INTERVALS - 1 {
            return;
        }
        let frame = Frame {
            ucodes: std::array::from_fn(|k| frames.frame[k].0),
            positive: std::array::from_fn(|k| frames.frame[k].1),
        };
        if frames.impossible.len() == PLACE_WINDOW {
            frames.impossible.pop_front();
        }
        // Rd is no frame of data, and says nothing about where the frames are.
        frames.impossible.push_back(!looked && !frames.decoder.could_have_sent(&frame));
        if frames.impossible.iter().filter(|x| **x).count() >= PLACE_LOST
            && let Some(shift) = find_place(frames)
        {
            // The frames are somewhere else: a slip has moved every symbol
            // after it by a whole twenty milliseconds.
            frames.moved += 1;
            frames.impossible.clear();
            frames.history.clear();
            let offset = self.rx.frame_offset() + shift;
            self.rx.set_frame_offset(offset);
            return;
        }
        let bits: Vec<bool> = frames.decoder.frame(frame).into_iter().map(|b| frames.descrambler.descramble(b)).collect();
        if frames.data {
            if looked {
                frames.held.push_back(bits);
            } else {
                for held in frames.held.drain(..) {
                    self.received.extend(held);
                }
                self.received.extend(bits);
            }
            return;
        }
        if frames.ed {
            // B1d: 48 frames of scrambled ones.
            frames.b1d_left -= 1;
            if frames.b1d_left == 0 {
                self.notes.push(format!("found B1d: data mode, down at {} bit/s", self.downstream_rate));
                frames.data = true;
                self.stage = Stage::Data;
                self.renegotiating = false;
                self.rd_watch = RWatch::default();
            }
            return;
        }
        let zeros = bits.iter().all(|b| !*b);
        frames.zero_frames = if zeros && frames.mp.is_some() { frames.zero_frames + 1 } else { 0 };
        if frames.zero_frames == 2 {
            // Ed: B1d next, at data mode's constellation, with the coding
            // started afresh (8.6.1).
            frames.ed = true;
            frames.b1d_left = B1D_FRAMES;
            self.deadline = None;
            self.in_use = Some(choice.data.clone());
            self.least_gap = least_gap(&choice.data, route);
            self.downstream_rate = sequences::data_rate(choice.data.drn).unwrap_or(0);
            self.notes.push(format!("found Ed: B1d next, down at {} bit/s", self.downstream_rate));
            let Some(data) = Mapping::from_cp(&choice.data) else { return };
            frames.decoder = Decoder::new(data);
            frames.levels = levels_for(&choice.data, route);
            let slicer = slicer_for(&frames.levels);
            self.rx.set_slicer(slicer);
            return;
        }
        for bit in bits {
            if let Some(Found::Mp(mp)) = frames.finder.feed(bit) {
                if frames.mp != Some(mp) {
                    // Once for each different MP, and not for every repetition.
                    self.notes.push(format!("found {}", describe_mp(&mp)));
                }
                if mp.answer_to_call == 0 {
                    // 9.7: the digital modem has cleared down.
                    self.cleared_down();
                    return;
                }
                if mp.acknowledge {
                    frames.far_acknowledged = true;
                }
                if frames.mp.is_none() {
                    // 9.4.2.3: "complete sending the current CP sequence,
                    // and then send CP' sequences".
                    self.source.next_cp = Some((choice.data.acknowledged().to_bits(), true));
                }
                frames.mp = Some(mp);
            }
        }
    }

    /// Whether data mode is reading its levels cleanly enough for the rate,
    /// and a slower rate (9.6.2.1) if it is not. When is this end's to say:
    /// "The rate renegotiation procedure can be initiated at any time during
    /// data mode" (9.6).
    ///
    /// Two things are watched, because a line goes wrong in two ways.
    ///
    /// One is a line steadily short of margin -- a floor that has risen --
    /// and that is the receiver's own averaged error to judge, on the rule it
    /// was judged on before any of this counted misses: levels closer than
    /// [`MARGIN`] of that error, four looks running. The average follows a
    /// risen floor exactly, and is not compressed by the levels the way a
    /// decision's error is, so nothing here reads such a line better than it
    /// is.
    ///
    /// The other is a disturbance, and that same average is blind to it. A
    /// tenth of a second of noise spoils several blocks of data, and an
    /// average looked at a few times a second sees it only if a look falls
    /// inside one -- and the receiver holds its loops through a burst as it
    /// would through a slip, so its average barely takes the burst in at all.
    /// That is what the decisions are counted for, symbol by symbol.
    fn watch_margin(&mut self) {
        // What the receiver itself is making of the line, as the margin was
        // watched before the decisions were counted (see [`MARGIN`]).
        let law = self.settings.law;
        let receiver = ucode::level(law, self.settings.uinfo) / 10f64.powf(self.rx.snr_db() / 20.0);
        // A receiver holding its loops says nothing about the margin either
        // way: it holds them through any sudden rise in error, a burst of
        // noise as much as a slip, and the average it is holding is the
        // average from before the trouble. The count of looks short of margin
        // stands still while it does, as it did before this watch counted
        // anything.
        //
        // Only that count stands down. The looks, the evidence and the
        // stretches of misses go on being gathered, because a long burst of
        // real noise makes the receiver hold its loops too, and what is
        // gathered through one is exactly what says the line is bad. Standing
        // those down as well cost three hundred milliseconds of noise every
        // second and a half its fall back altogether -- 54 666 for ever,
        // where it should reach 40 000 and error nothing after it.
        let holding_loops = self.rx.is_lost();
        let losing_it = self.least_gap < UNREADABLE_GAP * receiver;
        if losing_it {
            // A receiver that is not reading the constellation is no judge
            // of what rate the line would carry.
            self.decisions.spoil();
        }
        let stands = self.decisions.look(self.frames_moved());
        // A line steadily short of margin, on the rule that judged one before
        // any of this: levels closer than [`MARGIN`] of the receiver's own
        // averaged error, four looks running. Measured over the A-law 0.6 s
        // round trip with ten milliseconds of digital silence every second
        // and a half, where a count of looks that found the receiver
        // unreadable used to ask for a retrain instead: this asks for the
        // renegotiation main asks for, at the moment main asks for it, and
        // ends the two minutes at 46 666 with 5877 clean blocks of 6097,
        // where the retrains left it at 44 000 with 3834 of 4533.
        let steady = self.least_gap < MARGIN * receiver;
        self.short = if holding_loops {
            self.short
        } else if steady {
            self.short + 1
        } else {
            0
        };
        if self.short >= MARGIN_SHORT {
            self.short = 0;
            if losing_it {
                // Nothing is being read at all: that is a receiver to train
                // again.
                self.wants_retrain = true;
            } else {
                self.fall_back(0.0, receiver);
            }
            return;
        }
        let Some(look) = stands else { return };
        if !self.decisions.weigh(look) {
            return;
        }
        if self.least_gap < 2.0 * self.decisions.rms() {
            // Nothing is being read at all: that is a receiver to train again.
            self.wants_retrain = true;
            return;
        }
        // The next rate is chosen for the line as the worst of the recent
        // looks found it, not as they found it on average: the average of a
        // line that is clean but for a burst every second or two is nearly a
        // clean line's, and a rate chosen for it goes on making errors in
        // every burst -- and asks again, and again. Chosen for the worst, one
        // renegotiation lands where the bursts are read cleanly, and that is
        // also what stops the next: the same bursts at the new rate come
        // nowhere near its levels' boundaries.
        //
        // And never under the receiver's own averaged error, which is the
        // same measurement read a second way and is not compressed as this
        // one is. A decision's error here is its distance from the nearer of
        // the two levels either side of it, so a decision that has crossed a
        // boundary is measured to the wrong one and can never be out by more
        // than half a gap, however far out it really was. On a line that is
        // clean but for bursts that hardly moves the answer, because the
        // worst block is a burst and a burst is read against the gaps it
        // crosses; on a floor stepped up until it errors every few seconds
        // it reads the line better than it is. Measured over the 0.6 s round
        // trip with the floor stepped to 6e-4, the worst block came to
        // 0.000325 where the receiver's own error was 0.000518 -- the same
        // number main reads -- and the rate chosen from the first was 50 666,
        // which still errored 4 of 495 blocks and then 4 of 357 and had to
        // be asked again, ending at 44 000, a rung below where main settles
        // in one go. The larger of the two lands at 45 333 first time, where
        // nothing errors at all afterwards.
        self.fall_back(self.decisions.worst(), receiver);
    }

    /// Renegotiate for a line whose error is `measured`, never read better
    /// than the receiver's own averaged error `receiver`.
    fn fall_back(&mut self, measured: f64, receiver: f64) {
        if let Some(route) = self.route.as_ref() {
            let law = self.settings.law;
            let limit = f64::from(super::power_limit(&self.settings.server)) / 32768.0;
            let expected = route.noise_at(law, limit) * self.shaping.1.sqrt();
            self.worse = self.worse.max(measured.max(receiver) / expected);
        }
        let most = self.downstream_rate.saturating_sub(1);
        // And no further down than [`MOST_DROPPED`] bits a frame in one go.
        let drn = self.in_use.as_ref().map_or(0, |cp| cp.drn);
        let least = sequences::data_rate(drn.saturating_sub(MOST_DROPPED)).unwrap_or(0);
        if !self.renegotiate_within(most, least) {
            // Nothing slower the route carries: train again from phase 2.
            self.wants_retrain = true;
        }
    }

    /// What every CP this end sends says besides its constellations, rate and
    /// shaping.
    fn finish_cp(&self, cp: &mut Cp) {
        cp.a_law = self.settings.law == Law::A;
        cp.upstream_rates = if self.settings.wide { 0x1fff } else { 0x07ff };
    }

    /// Times the downstream frames were found somewhere else after a slip,
    /// from the data frames themselves or from R.
    pub fn frames_moved(&self) -> u32 {
        self.frames.as_ref().map_or(0, |f| f.moved) + self.r_moved
    }

    /// Data mode's upstream, as the digital modem's MP asks for it.
    fn prepare_upstream(&mut self) {
        let (Some(mp), Some(choice)) = (self.far_mp(), self.choice.as_ref()) else { return };
        let rate = super::digital::upstream_rate(&choice.data, &mp);
        self.upstream_rate = u32::from(rate) * 2400;
        let Some(framing) = Framing::new(self.settings.upstream.rate, self.upstream_rate, false, mp.expanded_shaping) else {
            self.fail("no upstream rate both ends allow");
            return;
        };
        let params = Params {
            framing,
            code: match mp.trellis {
                Trellis::States16 => Code::States16,
                Trellis::States32 => Code::States32,
                Trellis::States64 => Code::States64,
            },
            nonlinear: mp.non_linear,
            precoding: mp.precoding.unwrap_or([(0, 0); 3]),
            mode: Mode::Answer,
        };
        self.source.encoder = Some(UpstreamEncoder::new(params));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TRN1d's signs, `repeats` Jds and J'd, as the digital modem sends them
    /// (8.4.2, 8.4.3, 8.4.5), and then signs that mean nothing.
    fn phase3_signs(jd: &Jd, repeats: usize) -> (Vec<bool>, usize) {
        let mut scrambler = Scrambler::new(Mode::Call);
        let mut sign = false;
        let mut signs = Vec::new();
        for _ in 0..2400 {
            sign = scrambler.scramble(true);
            signs.push(sign);
        }
        for _ in 0..repeats {
            for bit in jd.to_bits() {
                sign ^= scrambler.scramble(bit);
                signs.push(sign);
            }
        }
        for _ in 0..JD_PRIME_BITS {
            sign ^= scrambler.scramble(false);
            signs.push(sign);
        }
        let end = signs.len();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..500 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            signs.push(x & 1 == 1);
        }
        (signs, end)
    }

    /// Where a reader says J'd ended, if it does.
    fn jd_prime_at(signs: &[bool]) -> Option<usize> {
        let mut reader = JdReader::new();
        signs.iter().enumerate().find_map(|(i, &s)| reader.feed(i as u64, s).then_some(i + 1))
    }

    #[test]
    fn j_prime_is_read_after_whole_jds() {
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() };
        let (signs, end) = phase3_signs(&jd, 12);
        assert_eq!(jd_prime_at(&signs), Some(end));
    }

    /// A softphone that cut ten milliseconds out of the Jds, into the start
    /// of the last, leaves that one unreadable whole; J'd is still found
    /// after its tail. (A cut nearer J'd than the descrambler's memory is
    /// beyond reading at all, and the DIL is found from its own levels.)
    #[test]
    fn j_prime_is_read_after_a_jd_a_slip_cut_into() {
        let jd = Jd { rates: Jd::ALL_RATES, lookahead: 1, ..Jd::default() };
        let (mut signs, end) = phase3_signs(&jd, 12);
        let last = end - JD_PRIME_BITS - JD_BITS;
        signs.drain(last - 60..last + 20);
        assert_eq!(jd_prime_at(&signs), Some(end - 80));
    }

    /// A watch on decisions against levels at 1 and 3, either sign, in every
    /// interval, begun.
    fn watching() -> Decisions {
        let levels: Levels = std::array::from_fn(|_| vec![(1.0, 1, true), (-1.0, 1, false), (3.0, 3, true), (-3.0, 3, false)]);
        let mut decisions = Decisions::new(&levels);
        assert!(decisions.look(0).is_none(), "the first look only begins the watch");
        decisions
    }

    /// A look's worth of symbols, `misses` of them misses, spread evenly
    /// through it as a line at its own margin misses.
    fn symbols(decisions: &mut Decisions, misses: usize) {
        stretch(decisions, misses, 2000);
    }

    /// A look's worth of symbols with `misses` misses spread evenly over the
    /// first `over` of them, and nothing missed after.
    fn stretch(decisions: &mut Decisions, misses: usize, over: usize) {
        for n in 0..2000 {
            let missed = n < over && n * misses / over != (n + 1) * misses / over;
            decisions.symbol(n % INTERVALS, if missed { 1.95 } else { 1.05 });
        }
    }

    /// A decision is judged by how far it went towards the boundary with the
    /// level on the side it went, as a share of the way: a miss past
    /// four-fifths of it, whichever level it was nearer, and never beyond
    /// the outermost level, where there is no other level to take it for.
    #[test]
    fn a_decision_is_judged_by_how_far_it_went_towards_the_next_level() {
        let mut decisions = watching();
        let cases = [
            (1.1, false),
            (1.75, false),
            (1.95, true),
            (2.1, true),
            (2.5, false),
            (0.15, true),
            (-0.1, true),
            (-2.9, false),
            (-5.0, false),
            (9.0, false),
        ];
        for (value, missed) in cases {
            let before = decisions.look.misses;
            decisions.symbol(0, value);
            assert_eq!(decisions.look.misses > before, missed, "{value}");
        }
        // The error is the distance from the nearer level, outermost or not.
        let errors = [0.1, 0.75, 0.95, 0.9, 0.5, 0.85, 0.9, 0.1, 2.0, 6.0];
        let power: f64 = errors.iter().map(|e| e * e).sum();
        assert!((decisions.look.power - power).abs() < 1e-9, "{} against {power}", decisions.look.power);
    }

    /// A stretch of misses dense enough to be garbage and short enough to be
    /// one packet of it is a jitter buffer's, not the line's: the look it fell
    /// in goes, and with it the look before -- which every look waits for --
    /// and the look after, while the loops come back. A stretch as long as a
    /// burst of noise on the line is weighed like any other.
    #[test]
    fn a_packet_s_worth_of_garbage_is_not_the_line_and_a_hundred_milliseconds_of_noise_is() {
        let mut decisions = watching();
        // One twenty-millisecond packet: 160 decisions, a fifth of them missed.
        stretch(&mut decisions, 32, 160);
        assert!(decisions.look(0).is_none(), "held until the next is over");
        symbols(&mut decisions, 0);
        assert!(decisions.look(0).is_none(), "the packet's look goes, and the one before it");
        symbols(&mut decisions, 0);
        assert!(decisions.look(0).is_none(), "and the look after it, while the loops come back");
        symbols(&mut decisions, 0);
        assert!(decisions.look(0).is_some(), "and then the line is the line again");
        // A hundred milliseconds of noise: 800 decisions, a tenth missed.
        let mut decisions = watching();
        stretch(&mut decisions, 80, 800);
        assert!(decisions.look(0).is_none(), "held until the next is over");
        symbols(&mut decisions, 0);
        assert_eq!(decisions.look(0).map(|l| l.misses), Some(80), "a burst of noise is the line's");
    }

    /// Levels a route's are like, rather than evenly spaced: a quiet pair a
    /// quarter of a unit apart and a loud pair two apart, either sign, in
    /// every interval.
    fn watching_uneven() -> Decisions {
        let levels: Levels =
            std::array::from_fn(|_| vec![(1.0, 1, true), (1.5, 2, true), (8.0, 3, true), (12.0, 4, true), (-1.0, 1, false), (-1.5, 2, false), (-8.0, 3, false), (-12.0, 4, false)]);
        let mut decisions = Decisions::new(&levels);
        assert!(decisions.look(0).is_none(), "the first look only begins the watch");
        decisions
    }

    /// What tells a packet of made-up audio from a burst of noise on the
    /// line is not how long the stretch lasted -- both can be thirty
    /// milliseconds of it -- but which decisions missed. Noise reaches a
    /// boundary only where the boundary is near, at the quiet codewords,
    /// which carry next to none of the sound; made-up audio is in the
    /// codewords' place and misses at every level alike, so its misses carry
    /// their share of the sound ([`STORM_GARBLED`]).
    ///
    /// Both stretches here are 160 decisions with 32 misses or more, which
    /// the count this used to be judged on could not tell apart at all.
    #[test]
    fn a_stretch_is_made_up_audio_only_if_its_misses_carry_the_sound() {
        // Every fifth decision is a quiet codeword, and in the noisy stretch
        // every one of those has been pushed to the boundary between the
        // quiet levels. Nothing else has moved: the loud codewords, which
        // are nearly all the sound there is, are read as cleanly as ever.
        let noise = |n: usize| if n.is_multiple_of(5) { 1.25 } else { 8.3 };
        // Made-up audio misses at the loud levels as well.
        let made_up = |n: usize| match n % 5 {
            0 => 1.25,
            1 => 10.0,
            _ => 8.3,
        };
        for (what, value, stands) in [("noise", &noise as &dyn Fn(usize) -> f64, true), ("made-up audio", &made_up, false)] {
            let mut decisions = watching_uneven();
            for n in 0..2000 {
                decisions.symbol(n % INTERVALS, if n < 160 { value(n) } else { 8.3 });
            }
            assert!(decisions.look(0).is_none(), "{what}: held until the next is over");
            for n in 0..2000 {
                decisions.symbol(n % INTERVALS, 8.3);
            }
            assert_eq!(decisions.look(0).is_some(), stands, "{what}");
        }
    }

    /// Silence in the codewords' place is not the line either, and neither
    /// the sound its misses carry nor the decisions themselves can say so:
    /// silence carries no sound for [`STORM_GARBLED`] to weigh, and the
    /// equaliser goes on putting out codeword-sized numbers from its own
    /// feedback while nothing at all arrives. What says so is the line in
    /// front of the equaliser, [`HOLE`] symbols of it under a thousandth of
    /// the line's own level; a shorter gap -- what a concealer's fading
    /// repeat leaves -- is left where it fell.
    #[test]
    fn a_hole_in_the_line_is_not_the_line_and_a_shorter_gap_is_left_alone() {
        for (gap, stands) in [(HOLE - 1, true), (HOLE, false)] {
            let mut decisions = watching_uneven();
            for n in 0..2000 {
                // The decisions say nothing either way: a hole is judged on
                // the line alone, and these are what the equaliser puts out
                // right through one.
                decisions.symbol(n % INTERVALS, 8.3);
                decisions.line(if (100..100 + gap).contains(&n) { 0.0 } else { 1.0 });
            }
            assert!(decisions.look(0).is_none(), "gap {gap}: held until the next is over");
            for n in 0..2000 {
                decisions.symbol(n % INTERVALS, 8.3);
                decisions.line(1.0);
            }
            assert_eq!(decisions.look(0).is_some(), stands, "a gap of {gap} symbols");
        }
    }

    /// The line's own level is a slow mean of what the equaliser was given,
    /// and a hole is held out of it, so that however long a hole lasts it
    /// cannot bring the level down to meet itself and stop being a hole.
    #[test]
    fn a_long_hole_does_not_drag_its_own_yardstick_down_after_it() {
        let mut decisions = watching_uneven();
        for n in 0..2000 {
            decisions.symbol(n % INTERVALS, 8.3);
            decisions.line(1.0);
        }
        assert!(decisions.look(0).is_none(), "held until the next is over");
        // Four times [`LEVEL_OVER`] of nothing: were the level taking it in,
        // it would be under a thousandth of where it started long before the
        // end of this.
        for n in 0..4 * LEVEL_OVER as usize {
            decisions.symbol(n % INTERVALS, 8.3);
            decisions.line(0.0);
        }
        assert!(decisions.look(0).is_none(), "the hole spoiled the look it fell in");
        for n in 0..2000 {
            decisions.symbol(n % INTERVALS, 8.3);
            decisions.line(1.0);
        }
        // Still a hole at the end of it, not a line the watch has learnt.
        assert!(decisions.look(0).is_none(), "and the look after it, held for the loops");
    }

    /// A look stands only once the look after it is over, and only if the
    /// frames moved in neither and the far end went quiet in neither: a
    /// slip's garbage, and the look before it that may hold its start, are
    /// not the line's.
    #[test]
    fn a_look_stands_only_if_no_slip_or_silence_touched_it_or_the_next() {
        let mut decisions = watching();
        let mut look = |misses: usize, moved: u32, quiet: bool| {
            symbols(&mut decisions, misses);
            if quiet {
                decisions.spoil();
            }
            decisions.look(moved).map(|l| l.misses)
        };
        assert_eq!(look(7, 0, false), None, "held until the next is over");
        assert_eq!(look(1, 0, false), Some(7));
        // The frames move while the third runs: the second and third go.
        assert_eq!(look(30, 1, false), None);
        assert_eq!(look(2, 1, false), None);
        assert_eq!(look(3, 1, false), Some(2));
        // The far end goes quiet in the sixth: the fifth and sixth go.
        assert_eq!(look(40, 1, true), None);
        assert_eq!(look(5, 1, false), None);
        assert_eq!(look(0, 1, false), Some(5));
    }

    /// The rate is chosen for the worst 32 ms block that three of the recent
    /// looks reached, and not for the worst one of them: one window out of
    /// hundreds can hold anything, and the worst would let one window decide
    /// how far the call falls.
    #[test]
    fn the_rate_is_chosen_for_the_worst_block_three_looks_reached() {
        let look = |worst: f64| Look { symbols: 2000, misses: 3, power: 2000.0 * 0.01, worst, spoiled: false };
        let mut decisions = watching();
        for worst in [0.01, 0.01, 0.04, 0.09, 1.0] {
            decisions.weigh(look(worst));
        }
        // 1.0 and 0.09 are above it, so 0.04 is the block three of them
        // reached, and its RMS is 0.2.
        assert!((decisions.worst() - 0.2).abs() < 1e-9, "{}", decisions.worst());
    }

    /// No one look is ever enough, however bad, nor a line that misses once
    /// or twice a look for ever; three bad looks close together are, and so
    /// are bursts every second and a half by the third, while bursts ten
    /// seconds apart never add up to enough. A slower rate is then chosen
    /// for the worst block the recent looks saw.
    #[test]
    fn evidence_is_enough_after_bad_looks_close_together_and_never_after_one() {
        let bad = Look { symbols: 2000, misses: 40, power: 2000.0 * 0.04, worst: 0.09, spoiled: false };
        let margin = Look { symbols: 2000, misses: 2, power: 2000.0 * 0.01, worst: 0.012, spoiled: false };
        let mut decisions = watching();
        assert!(!decisions.weigh(bad));
        assert!(!decisions.weigh(bad));
        assert!(decisions.weigh(bad));
        assert!((decisions.worst() - 0.3).abs() < 1e-9);
        let mut decisions = watching();
        assert!((0..4000).all(|_| !decisions.weigh(margin)));
        // Bursts `apart` seconds apart: which of them was enough, if any.
        let bursts = |apart: f64| {
            let mut decisions = watching();
            (0..40).position(|_| {
                let enough = decisions.weigh(bad);
                for _ in 1..(apart / MARGIN_EVERY) as usize {
                    decisions.weigh(margin);
                }
                enough
            })
        };
        assert_eq!(bursts(1.5), Some(2));
        assert_eq!(bursts(10.0), None);
    }
}
