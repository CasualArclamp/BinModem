//! The V.32 start-up procedure (clause 5.4).
//!
//! Two modems that have never met have to agree a data rate, train an
//! equaliser at each end and an echo canceller at each end, and measure how
//! long the line takes to carry a signal there and back. None of that can use
//! a demodulator, because the demodulator is one of the things being set up,
//! so the whole exchange is conducted in fixed patterns of constellation
//! states that can be recognised as waveforms.
//!
//! The round trip is measured directly, and rather elegantly. Each modem
//! reverses the phase of what it is sending at a moment of its own choosing,
//! and starts a clock; the far end, on hearing that reversal, reverses its own
//! transmission exactly 64 symbols later; the first modem stops its clock when
//! that comes back. What is left after taking off the 64 is the time the line
//! adds, which is what the echo canceller needs to know how far back to look.

use super::{Coding, Mode, Receiver, Signal, Transmitter};
use dsp::{EchoCanceller, EchoFinder, ReversalDetector, ToneDetector};
// Part of this module's surface: `Modem::reflection` hands one back, and what
// is above a data pump should not have to reach past it to name the type.
pub use dsp::Reflection;

/// Half the symbol rate: where an alternating pattern puts its sidebands.
const OFFSET: f64 = super::BAUD / 2.0;

/// Level below which the line is carrying nothing.
///
/// A modem has to work across the range of levels the network delivers, which
/// is some 34 dB between a short local call and a long one. This sits below
/// the bottom of that: a signal arriving 20 dB down, which is ordinary once it
/// has crossed a network, reads about twelve times this.
const QUIET: f64 = 0.005;

/// Amplitude a tone must reach before its phase is worth watching, on the same
/// footing as [`QUIET`].
const AUDIBLE: f64 = 0.008;

/// How far a spectral line must stand above the average level of the whole
/// signal before it counts as standing there.
///
/// Measured against each signal in turn, as a ratio to the mean of the
/// rectified waveform: a bare carrier reads 1.60, the sidebands of an
/// alternation 1.28, and the conditioning signal 1.06 at the carrier with 0.68
/// at its sidebands. Everything scrambled reads 0.33 or less at every line,
/// which is the detector collecting its own bandwidth's worth of a signal
/// spread across the band rather than finding anything there. The two
/// thresholds sit in the gaps.
const STANDING: f64 = 0.7;
const STANDING_SIDEBAND: f64 = 0.5;

/// What the line is carrying, as far as the start-up needs to know.
///
/// The distinctions are all between waveforms rather than between messages,
/// which is what makes them available before anything has been demodulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// Not enough signal to say anything.
    Nothing,
    /// The V.25 answering tone at 2100 Hz (5.1).
    AnswerTone,
    /// A repeated state: the bare carrier. AA or CC in Figure 4.
    Carrier,
    /// An alternation between opposite states: sidebands at 600 and 3000 with
    /// the carrier suppressed. AC or CA.
    Alternation,
    /// An alternation between states a quarter turn apart: the same sidebands
    /// but with the carrier still standing. The conditioning signal of 5.2.
    Conditioning,
    /// Energy across the band and no line standing out: TRN, a rate signal, or
    /// data. All three are scrambled, which is what makes them look alike here
    /// and why telling them apart is the demodulator's job rather than this.
    Spread,
}

/// Recognises the start-up signals from the shape of the spectrum.
#[derive(Debug)]
pub struct Listener {
    answer: ToneDetector,
    /// Envelope of the answering tone, which has to survive its own reversals.
    ///
    /// V.25's answering tone is a plain 2100 Hz. V.8's is the same tone with a
    /// phase reversal every 450 ms, and the reversals are the whole point of
    /// it: they are how an answering modem says it can do V.8 at all. Every
    /// answering modem made since 1994 sends that one.
    ///
    /// A reversal takes the tone detector's phasor through zero. Judged on the
    /// phasor, the tone therefore stops existing for a few milliseconds twice a
    /// second -- and 5.4.1 wants it heard for a whole second before the calling
    /// modem may join in, which it can now never be. The modem sits mute
    /// waiting for a second of tone that arrives in 450 ms instalments, and the
    /// far end, hearing nothing back, concludes it is talking to something that
    /// is not a V.32 modem.
    ///
    /// The envelope is of the amplitude, which does not care about the sign,
    /// and is slow enough that a reversal is a ripple in it.
    answer_envelope: dsp::filter::OnePole,
    carrier: ToneDetector,
    low: ToneDetector,
    high: ToneDetector,
    /// Envelope of the weaker sideband, on the same footing as the answering
    /// tone's.
    ///
    /// The two are compared against each other, and a comparison between a
    /// fast measure and a slow one is decided by their time constants rather
    /// than by the signal: whichever rises first wins the first tenth of a
    /// second of every call, whatever is on the line.
    sideband_envelope: dsp::filter::OnePole,
    /// Total power on the line, to tell a spread signal from silence.
    power: dsp::filter::OnePole,
}

impl Listener {
    pub fn new(fs: f64) -> Self {
        // Narrow enough to separate lines 1200 Hz apart with room to spare,
        // wide enough to answer within a few tens of symbols.
        const BANDWIDTH: f64 = 60.0;
        Self {
            answer: ToneDetector::new(super::ANSWER_TONE, BANDWIDTH, fs),
            // Long against the few milliseconds a reversal costs, short
            // against the second the tone has to be held for.
            answer_envelope: dsp::filter::OnePole::new(0.100, fs),
            sideband_envelope: dsp::filter::OnePole::new(0.100, fs),
            carrier: ToneDetector::new(super::CARRIER, BANDWIDTH, fs),
            low: ToneDetector::new(super::CARRIER - OFFSET, BANDWIDTH, fs),
            high: ToneDetector::new(super::CARRIER + OFFSET, BANDWIDTH, fs),
            power: dsp::filter::OnePole::new(0.020, fs),
        }
    }

    pub fn feed(&mut self, x: f64) {
        self.answer.feed(x);
        self.answer_envelope.process(self.answer.amplitude());
        self.carrier.feed(x);
        self.low.feed(x);
        self.high.feed(x);
        self.sideband_envelope
            .process(self.low.amplitude().min(self.high.amplitude()));
        self.power.process(x.abs());
    }

    /// Amplitude of the bare carrier, which 5.4.2 watches for and then
    /// watches for a drop in.
    pub fn carrier_amplitude(&self) -> f64 {
        self.carrier.amplitude()
    }

    /// Amplitude of the weaker of the two sidebands, which is what 5.4.1 has
    /// the calling modem listen for as "600 Hz and 3000 Hz".
    pub fn sideband_amplitude(&self) -> f64 {
        // The envelope, so that this and the answering tone it is weighed
        // against are measured the same way.
        self.sideband_envelope.value()
    }

    /// Amplitude of the answering tone (5.1).
    pub fn answer_amplitude(&self) -> f64 {
        // The envelope, for the same reason [`classify`] uses it: measured on
        // the phasor, a tone with reversals in it keeps vanishing. That
        // mattered here too. This is compared against the sidebands, so an
        // answering tone that read as nothing for a few milliseconds made the
        // skirt of itself reaching the sideband detectors look, for exactly
        // those milliseconds, like a far end alternating -- and a loud enough
        // answering tone would let the calling modem out of Listening on the
        // strength of that glitch, which is not hearing anything, it is being
        // startled by a discontinuity.
        self.answer_envelope.value()
    }

    pub fn level(&self) -> f64 {
        self.power.value()
    }

    /// What the line is carrying, judged by which lines stand above the level
    /// of the whole signal.
    ///
    /// Only usable when the modem's own echo is either absent or cancelled.
    /// Everything here is a ratio to the total, and an uncancelled echo is
    /// part of that total: a modem sending a bare carrier and hearing it come
    /// back off the hybrid will find the carrier standing proud of everything
    /// else and conclude the far end is sending one.
    ///
    /// The start-up avoids depending on it until then, and can, because the
    /// signals of its half-duplex opening are in different places: a modem
    /// repeating a state puts everything at 1800 Hz and listens at 600 and
    /// 3000, and the modem alternating states does the exact reverse. Each is
    /// deaf to its own echo by construction rather than by cancelling it,
    /// which is what lets the exchange happen before anything is trained.
    pub fn classify(&self) -> Heard {
        let level = self.power.value();
        if level < QUIET {
            return Heard::Nothing;
        }
        // Everything is judged against the level of the whole signal rather
        // than against an absolute, so that a quiet line and a loud one are
        // read alike and no threshold has to be told what the line is scaled
        // to.
        let answer = self.answer_envelope.value() / level;
        let carrier = self.carrier.amplitude() / level;
        let sidebands = self.low.amplitude().min(self.high.amplitude()) / level;

        // The answering tone is nowhere near the others, so it is decided on
        // its own and first.
        if answer > STANDING {
            return Heard::AnswerTone;
        }
        // A pattern repeating every two symbols puts a line at each sideband
        // whether or not its two states are opposite; what separates the cases
        // is the carrier. An opposite pair averages to nothing and leaves
        // none; an unequal pair leaves it standing above them.
        match (
            carrier > STANDING,
            sidebands > STANDING_SIDEBAND,
        ) {
            (true, true) => Heard::Conditioning,
            (true, false) => Heard::Carrier,
            (false, true) => Heard::Alternation,
            (false, false) => Heard::Spread,
        }
    }
}

/// Which end of the call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Calling,
    Answering,
}

impl Role {
    pub fn mode(self) -> Mode {
        match self {
            Self::Calling => Mode::Call,
            Self::Answering => Mode::Answer,
        }
    }
}

/// Make a transmitter and receiver for one end of a V.32 call.
pub fn endpoints(role: Role, fs: f64) -> (Transmitter, Receiver) {
    let mode = role.mode();
    (Transmitter::new(mode, fs), Receiver::new(mode, fs))
}

/// The rates a modem is offering.
///
/// Table 5/V.32bis gives each one a bit and the bits are not in rate order:
/// 4800 and 9600 are where V.32's Table 6 put them, and the three V.32bis adds
/// are fitted into the gaps V.32 left -- B9, B10 and B12, which V.32's own
/// Note 2 reserves for exactly this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rates {
    pub at_4800: bool,
    pub at_7200: bool,
    pub at_9600: bool,
    pub at_12000: bool,
    pub at_14400: bool,
}

/// Every rate there is, fastest first.
pub const EVERY_RATE: [u32; 5] = [14_400, 12_000, 9600, 7200, 4800];

impl Rates {
    /// Every rate from `lowest` to `highest`, which is what a modem told to
    /// work between two rates offers.
    pub fn between(lowest: u32, highest: u32) -> Self {
        let mut rates = Self::default();
        for rate in EVERY_RATE {
            if rate >= lowest && rate <= highest {
                rates.set(rate, true);
            }
        }
        rates
    }

    /// Just the one.
    pub fn only(rate: u32) -> Self {
        let mut rates = Self::default();
        rates.set(rate, true);
        rates
    }

    pub fn holds(self, rate: u32) -> bool {
        match rate {
            4800 => self.at_4800,
            7200 => self.at_7200,
            9600 => self.at_9600,
            12_000 => self.at_12000,
            14_400 => self.at_14400,
            _ => false,
        }
    }

    pub fn set(&mut self, rate: u32, on: bool) {
        match rate {
            4800 => self.at_4800 = on,
            7200 => self.at_7200 = on,
            9600 => self.at_9600 = on,
            12_000 => self.at_12000 = on,
            14_400 => self.at_14400 = on,
            _ => {}
        }
    }

    /// The fastest of them, or zero if there are none -- which Note 3 to
    /// Table 5 makes a call for the connection to be cleared down.
    pub fn highest(self) -> u32 {
        EVERY_RATE.into_iter().find(|&r| self.holds(r)).unwrap_or(0)
    }

    /// What both ends can do.
    pub fn shared_with(self, other: Self) -> Self {
        let mut both = Self::default();
        for rate in EVERY_RATE {
            both.set(rate, self.holds(rate) && other.holds(rate));
        }
        both
    }

    pub fn any(self) -> bool {
        self.highest() != 0
    }
}

/// Where each rate's bit lives, in the B-numbering both tables use.
const RATE_BITS: [(u32, u32); 5] =
    [(4800, 5), (7200, 9), (9600, 6), (12_000, 10), (14_400, 12)];

fn bit(s: u16, b: u32) -> bool {
    s & (1 << (15 - b)) != 0
}

fn set_bit(s: &mut u16, b: u32) {
    *s |= 1 << (15 - b);
}

/// The signal a V.32bis modem sends to say what it can do (Table 5/V.32bis).
///
/// B0 to B3 are zero and B7, B11 and B15 are one, which is what a receiver
/// synchronises on and what separates a rate signal from the E that ends it.
/// B4 and B8 are one always: Note 1 to V.32's Table 6 makes that pair mean
/// "V.32 bis operation", which is why the two rates V.32 knew about keep their
/// old bits and the new ones went elsewhere.
pub fn rate_signal(rates: Rates) -> u16 {
    let mut s = 0u16;
    for b in [7, 11, 15, 4, 8] {
        set_bit(&mut s, b);
    }
    for (rate, b) in RATE_BITS {
        if rates.holds(rate) {
            set_bit(&mut s, b);
        }
    }
    // B13 and B14 "shall be set to zero when transmitting" (Note 2).
    s
}

/// The same, in the form a modem that is not V.32bis can read (Table 6/V.32).
///
/// Sent once the far end has shown it is not V.32bis. There, B4 means 2400 and
/// B8 means trellis coding at the highest rate offered, so a V.32bis signal
/// read by a V.32 modem would be claiming both -- 2400, which nothing here can
/// do, and trellis at whatever rate, which at 4800 does not exist.
pub fn rate_signal_v32(rates: Rates, trellis: bool) -> u16 {
    let mut s = 0u16;
    for b in [7, 11, 15] {
        set_bit(&mut s, b);
    }
    if rates.at_4800 {
        set_bit(&mut s, 5);
    }
    if rates.at_9600 {
        set_bit(&mut s, 6);
    }
    if trellis {
        set_bit(&mut s, 8);
    }
    s
}

/// What a rate signal offers, read as whichever table it belongs to.
pub fn rates_offered(s: u16) -> Rates {
    let mut rates = Rates::default();
    if is_v32bis(s) {
        for (rate, b) in RATE_BITS {
            rates.set(rate, bit(s, b));
        }
        return rates;
    }
    // Table 6/V.32, where B4 is 2400 -- a rate this modem does not have and
    // V.32 itself does not define a modulation for -- and B9 to B14 are the
    // "absence of special operational modes" rather than rates.
    rates.at_4800 = bit(s, 5);
    rates.at_9600 = bit(s, 6);
    rates
}

/// A rate signal offering exactly one rate.
///
/// R1 and R2 say everything a modem can do. E says the one thing that was
/// settled on, and only that: Table 6/V.32bis has its rate bits "relate to the
/// transmission of scrambled binary ones immediately following signal E". A
/// modem that put its whole offer in E would tell a far end that had agreed to
/// 4800 to start receiving at 14 400.
///
/// `v32bis` is whether the far end has shown it can read the newer table.
pub fn rate_signal_for(bits_per_second: u32, coding: Coding, v32bis: bool) -> u16 {
    let rates = Rates::only(bits_per_second);
    if v32bis {
        rate_signal(rates)
    } else {
        rate_signal_v32(rates, coding == Coding::Trellis)
    }
}

/// The E sequence that ends a rate exchange (Table 6/V.32bis, Table 7/V.32).
///
/// The same as a rate signal except that B0 to B3 are ones, which is the only
/// thing distinguishing the two.
pub fn end_signal(rate: u16) -> u16 {
    rate | 0xf000
}

/// True if `s` has the synchronising bits a rate signal must have (5.3.1).
pub fn is_rate_signal(s: u16) -> bool {
    s & 0xf000 == 0 && bit(s, 7) && bit(s, 11) && bit(s, 15)
}

/// True if `s` is an E sequence rather than a rate signal.
pub fn is_end_signal(s: u16) -> bool {
    s & 0xf000 == 0xf000 && bit(s, 7) && bit(s, 11) && bit(s, 15)
}

/// Whether a rate signal offers trellis coding at its highest rate (B8).
pub fn offers_trellis(s: u16) -> bool {
    bit(s, 8)
}

/// The coding two rate signals settle on for `rate`.
///
/// Between two V.32bis modems there is nothing to settle: 2.3.1 to 2.3.4 give
/// each of the four faster rates exactly one coding and it is the trellis one,
/// and 2.3.5 gives 4800 the four points it has always had.
///
/// With a modem that is not V.32bis it is V.32's rule instead. 2.4.1 gives
/// 9600 two modulations, 1 e) makes the uncoded one mandatory for
/// interworking, and B8 says whether the other is on offer -- so trellis needs
/// both ends to have set it and anything else is the sixteen points.
pub fn agreed_coding(theirs: u16, ours: u16, rate: u32) -> Coding {
    if is_v32bis(theirs) && is_v32bis(ours) {
        return if rate == 4800 { Coding::Uncoded } else { Coding::Trellis };
    }
    if rate == 9600 && offers_trellis(theirs) && offers_trellis(ours) {
        Coding::Trellis
    } else {
        Coding::Uncoded
    }
}

/// Whether a rate signal comes from a V.32bis modem.
///
/// Note 1 to Table 6/V.32: "The combination of B4 equal one and B8 equal one
/// indicates V.32 bis operation".
pub fn is_v32bis(s: u16) -> bool {
    bit(s, 4) && bit(s, 8)
}

/// The fastest rate both ends can do.
///
/// 5.4.1: "R2 shall exclude rates not appearing in the previously received
/// rate signal R1", which is this. Zero means there is nothing in common, and
/// Note 3 to Table 5 makes a rate signal with no rates in it a call for the
/// connection to be cleared down.
pub fn usable_rate(theirs: u16, ours: u16) -> u32 {
    rates_offered(theirs).shared_with(rates_offered(ours)).highest()
}

/// The highest data rate a rate signal offers, in bits per second.
///
/// Zero calls for the connection to be cleared down.
pub fn offered_rate(s: u16) -> u32 {
    rates_offered(s).highest()
}

/// What a 16-bit sequence says, in words.
///
/// For a log. Sixteen bits written out as a number tell nobody anything, and
/// which table they are to be read by is the first thing to know about them.
pub fn describe_sequence(s: u16) -> String {
    let kind = if is_end_signal(s) {
        "E"
    } else if is_rate_signal(s) {
        "R"
    } else {
        return format!("{s:016b} (not a rate sequence)");
    };
    let rates = rates_offered(s);
    let mut which: Vec<String> = EVERY_RATE
        .into_iter()
        .rev()
        .filter(|r| rates.holds(*r))
        .map(|r| r.to_string())
        .collect();
    if which.is_empty() {
        which.push("none, which asks to clear down".to_owned());
    }
    format!(
        "{kind} {s:016b}  {}  {}{}",
        if is_v32bis(s) { "V.32bis" } else { "V.32   " },
        which.join(" "),
        if !is_v32bis(s) && offers_trellis(s) {
            ", trellis"
        } else {
            ""
        }
    )
}

/// Finds the 16-bit sequences a rate exchange is made of (5.3).
///
/// The stream carries no framing, so the boundary has to be found in it.
/// 5.3.1 gives the rule: two consecutive identical sixteens with their
/// synchronising bits in the right places, which data is very unlikely to
/// imitate by accident. That fixes the boundary as well as identifying the
/// signal, and everything after can be read off it directly.
///
/// Reading it off matters, because the sequence that ends the exchange is sent
/// exactly once. 5.3.2 has a modem "first complete the transmission of the
/// current 16-bit rate sequence, and then transmit one 16-bit sequence E", so
/// a detector that insisted on seeing every sequence twice would see every
/// rate signal and never the thing that ends them. It would also be looking
/// for a repetition that cannot occur, since what follows E is data.
#[derive(Debug, Default)]
pub struct RateDetector {
    /// The last thirty-two bits seen, newest at the bottom.
    window: u32,
    filled: u32,
    /// Whether the 16-bit boundary has been found.
    locked: bool,
    /// Bits since that boundary.
    since: u32,
    /// The sequence being counted, and how many times it has come round
    /// unchanged.
    candidate: Option<u16>,
    agreed: u32,
    /// Sequences seen at the locked phase since this detector was reset, which
    /// is how long it has been hoping for a better reading.
    sequences: u32,
}


impl RateDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer one received bit. Yields each complete sequence once the boundary
    /// between them is known.
    pub fn feed(&mut self, bit: bool) -> Option<u16> {
        self.window = (self.window << 1) | u32::from(bit);
        self.filled = (self.filled + 1).min(32);
        if self.filled < 32 {
            return None;
        }
        let group = self.window as u16;
        let previous = (self.window >> 16) as u16;

        // 5.3.1: two identical sixteens with the synchronising bits in place.
        //
        // Two is the document's minimum and, on a real line, one short of
        // enough -- see [`RateDetector::agreement`], which counts how many
        // times the reading has held. That count is not acted on. Raising the
        // bar was tried and made things worse: over a virtual cable returning
        // the transmitter at unity, a correct reading never happens three
        // times running, so a modem that waits for a third never connects at
        // all. The reading needs a better receiver under it, not a stricter
        // test above it.
        // Checked at every position rather than only until a boundary is first
        // found, because a boundary found in noise will never match the real
        // thing, and a detector that could not change its mind stayed wrong
        // for the rest of the call.
        //
        // A repeating sixteen-bit signal matches its own predecessor at every
        // one of the sixteen phases, so what picks the phase out is the
        // synchronising bits -- and this therefore fires once per sequence
        // rather than once per bit.
        if group == previous && is_rate_signal(group) {
            self.locked = true;
            self.since = 0;
            self.sequences += 1;
            self.agreed = if self.candidate == Some(group) { self.agreed + 1 } else { 1 };
            self.candidate = Some(group);
            return Some(group);
        }

        // The sequence that ends the exchange is sent exactly once (5.3.2), so
        // it cannot be asked to repeat. Accepting a lone group is only safe
        // once the boundary is known: the synchronising bits are seven of
        // sixteen, so one group in a hundred and twenty-eight of anything at
        // all matches them, and a detector that accepted lone groups at an
        // unknown boundary finds a rate signal in scrambled data within a
        // second. That is exactly what happened, and what it cost was a modem
        // deciding it had heard R1 partway through the far end's training
        // segment and answering over the top of it.
        if self.locked {
            self.since += 1;
            if self.since >= 16 {
                self.since = 0;
                if is_end_signal(group) {
                    return Some(group);
                }
            }
        }
        None
    }

    /// The sequence being counted, how many times running it has read the
    /// same, and how many have been seen at all.
    ///
    /// A measurement, not a decision. It is here because the difference
    /// between a rate signal that was read and one that was guessed at is
    /// exactly this number, and off one recording it was the difference
    /// between 14 400 and 4800: at the locked phase the true sequence ran 53
    /// consecutive in one exchange and 222 in another, while every wrong
    /// reading ran once -- except the one that was acted on, which ran twice.
    pub fn agreement(&self) -> (Option<u16>, u32, u32) {
        (self.candidate, self.agreed, self.sequences)
    }

    pub fn reset(&mut self) {
        self.candidate = None;
        self.agreed = 0;
        self.sequences = 0;
        self.window = 0;
        self.filled = 0;
        self.locked = false;
        self.since = 0;
    }
}

/// How far the start-up has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Negotiating,
    /// A call that was up and is going through the start-up again (7).
    ///
    /// Told apart from `Negotiating` because it is: the line is still there,
    /// the far end is still on it, and what is being settled is a rate that
    /// stopped working rather than one that was never chosen.
    Retraining,
    /// Agreed, at this many bits per second.
    Connected(u32),
    /// The far end called for the connection to be cleared, or nothing
    /// recognisable arrived in time.
    Failed,
}

/// What fraction of the distance between neighbouring points the equaliser
/// may be left with before reception counts as unsatisfactory (7).
///
/// A fraction and not a distance, because the distance is different at every
/// rate: normalised to unit root-mean-square the closest two points are 1.41
/// apart at 4800 and 0.22 at 14 400. Half of whichever it is, is where a
/// decision is as likely to be wrong as right, and an equaliser driven by
/// decisions that are wrong half the time is not converging on anything.
///
/// This was a distance, 0.35, which is a quarter of the gap at 4800 and one
/// and a half gaps at 14 400 -- further than a symbol can land from the
/// nearest point, so at the rates it mattered most for it could not be
/// reached at all. Measured on a call that came up at 14 400 and never
/// decoded a byte: the equaliser sat at 0.073 to 0.088 for thirty-seven
/// seconds, which is a third of a gap and a receiver reading noise, and 7's
/// retrain never once looked like firing. Locked, the same call ran 0.028 to
/// 0.050.
///
/// A quarter is what 0.35 was at 4800, so the rate this was tuned on keeps the
/// threshold it had.
pub const UNSATISFACTORY_GAP: f64 = 0.25;

/// Durations from clause 5, in symbol intervals.
mod timing {
    /// The answering tone, V.25 2.2: 3.3 s, which at 2400 baud is this many.
    pub const ANSWER_TONE: u64 = 7920;
    /// How long the calling modem must hear the answering tone before joining
    /// in (5.4.1). Note 1 there lets it start on the tones alone, which is
    /// what makes this a minimum rather than a wait.
    pub const HEARD_ANSWER_TONE: u64 = 2400;
    /// The response delay both ends owe each other, 5.4.1 and 5.4.2: "64 plus
    /// or minus 2 symbol periods".
    pub const RESPONSE: u64 = 64;
    /// Alternating states before the answering modem may move on (5.4.2).
    pub const MIN_ALTERNATION: u64 = 128;
    /// How long the far end's retrain tone must hold before it is believed.
    ///
    /// V.32bis 7.1 and 7.2 both say "for more than 128 symbol intervals",
    /// which is 53 ms -- long enough that nothing in scrambled data imitates
    /// it and short enough that a call spends a twentieth of a second
    /// carrying rubbish before it notices.
    pub const RETRAIN_TONE: u64 = 128;
    /// How long reception must stay unsatisfactory before this end asks for a
    /// retrain of its own.
    ///
    /// 7 leaves the judgement open -- "if either modem incorporates a means of
    /// detecting unsatisfactory signal reception" -- and says nothing about
    /// how long to wait. A second of it: long enough that a burst of noise is
    /// ridden out rather than answered with thirty seconds of handshake,
    /// short enough that a line which has genuinely changed is not carrying
    /// nonsense for a minute.
    pub const UNSATISFACTORY: u64 = 2400;
    /// The incoming carrier must be heard this long first (5.4.2).
    pub const HEARD_CARRIER: u64 = 64;
    /// Shortest a rate signal may be sent for.
    ///
    /// 5.3.1 identifies one by two identical sixteen-bit sequences, so four of
    /// them is twice what a far end needs to see and still only thirteen
    /// milliseconds.
    pub const MIN_RATE_SIGNAL: u64 = 32;
    /// Silence after the amplitude drop (5.4.2).
    pub const GAP: u64 = 16;
    /// Segment 1 of the conditioning signal (5.2.1).
    pub const SEGMENT_S: u64 = 256;
    /// Segment 2 (5.2.2).
    pub const SEGMENT_S_BAR: u64 = 16;
    /// Segment 3, at its shortest (5.2.3 gives 1280 to 8192).
    pub const SEGMENT_TRN: u64 = 1280;
    /// Segment 3 on a line long enough to reflect as well as attenuate.
    ///
    /// Note 3 to 5.4.2 says the training segment "is suitable for training the
    /// echo canceller in the transmitting modem", and allows a longer sequence
    /// still if one is wanted. One is wanted here. A network reflection has to
    /// be found before the taps that cancel it can be placed, and then those
    /// taps have to converge, and both have to happen inside the one stretch
    /// of the start-up the far end is silent for. The shortest segment allowed
    /// is half a second, which is enough for one of those jobs.
    pub const SEGMENT_TRN_LONG: u64 = 4096;
    /// Scrambled ones before data may flow (5.4.1 e, 5.4.2).
    pub const SETTLE: u64 = 128;
    /// The latest an R3 could still be on its way, once the round trip is
    /// added to it.
    ///
    /// 5.4.1 has R2 continue "until an incoming rate signal R3 is detected"
    /// and sets no limit on the waiting. The far end has one. Before R3 it
    /// sends a second conditioning signal, and 5.2 fixes the shape of that:
    /// 256 symbols of S, 16 of S-bar, and a training segment 5.2.3 allows
    /// "at least 1280 and not exceed 8192". Past the longest of those there is
    /// nothing an R3 could still be behind.
    ///
    /// What is on the line instead, on the call this was written for, was the
    /// far end starting the whole procedure again: the answer tone, then eight
    /// seconds of alternations, twice over. This end held R2 up through all of
    /// it for twenty-three seconds and then let the far end hang up.
    pub const R3_AT_THE_LATEST: u64 = SEGMENT_S + SEGMENT_S_BAR + 8192;
    /// Nothing recognisable for this long and the attempt is abandoned. The
    /// recommendation sets no overall limit; a modem that waits for ever is no
    /// use to whatever is waiting on it.
    pub const PATIENCE: u64 = 2400 * 60;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Calling: silent, waiting for something to answer (5.4.1).
    Listening,
    /// Calling: repeating state A, waiting for a first reversal in the tones.
    Aa,
    /// Calling: the 64 symbols owed between hearing a reversal and answering.
    AaToCc,
    /// Calling: repeating state C, waiting for the second reversal.
    Cc,
    /// Calling: silent, waiting for the conditioning signal and then R1.
    AwaitingR1,
    /// Calling: the extra S of 5.4.1, sent for the measured round trip.
    PreRoll,

    /// Answering: sending the V.25 answering tone (5.1).
    AnswerTone,
    /// Answering: V.32bis 7.2's opening alternation, before 6.2 resumes.
    RetrainAc,
    /// Answering: alternating A and C, waiting to hear the calling modem.
    Ac,
    /// Answering: alternating the other way round, waiting for the reversal.
    Ca,
    /// Answering: the 64 symbols owed before turning back again.
    CaToAc,
    /// Answering: alternating again, waiting for the far end to stop.
    AcAgain,
    /// Answering: the 16 symbols of silence after the drop.
    Gap,
    /// Answering: silent again after R1, waiting out the measured round trip.
    AfterR1,
    /// Answering: waiting for the far end's conditioning signal, then R2.
    AwaitingR2,

    /// Both: the conditioning signal, in its three segments.
    SendS,
    SendSBar,
    SendTrn,
    /// Both: the rate signal, until the far end answers with its own.
    SendRate,
    /// Both: the single E that ends the exchange.
    SendEnd,
    /// Both: scrambled ones while the far end settles.
    Settling,
    Connected(u32),
    Failed,
}

/// Runs one end of the V.32 start-up.
///
/// Stepped one sample at a time, like everything else here. It owns the
/// listening: the sample handed in goes to its own tone detectors and to the
/// receiver, so the caller should not also feed the receiver.
#[derive(Debug)]
pub struct Startup {
    role: Role,
    state: State,
    listener: Listener,
    /// Reversals in the bare carrier, which the answering modem watches.
    carrier_reversals: ReversalDetector,
    /// Reversals in each sideband, which the calling modem watches. Both
    /// belong to the one signal and turn over together, so a reversal seen on
    /// either counts once and the other is ignored for a while afterwards.
    low_reversals: ReversalDetector,
    high_reversals: ReversalDetector,
    carrier_quiet: u64,
    sideband_quiet: u64,
    rates: RateDetector,
    /// Samples until the next symbol boundary.
    countdown: f64,
    sps: f64,
    /// Symbols in the current state, and since the start.
    symbols: u64,
    total: u64,
    /// Symbols since the round-trip timer was started, and the two answers
    /// taken off it: what the clock read, and what the line adds.
    timer: Option<u64>,
    counted: u64,
    round_trip: u64,
    /// Consecutive symbols the condition being waited for has held.
    held: u64,
    /// Consecutive symbols reception has been unsatisfactory for (7).
    unsatisfactory: u64,
    /// Whether this call has ever been up, which is what makes going through
    /// the start-up again a retrain rather than a first attempt.
    connected_once: bool,
    /// How many times it has retrained, which is worth showing: a call that
    /// retrains repeatedly is a call the line cannot hold.
    retrains: u32,
    /// Set by [`Startup::ask_for_retrain`] and taken by the next step.
    asked_to_retrain: bool,
    /// Every 16-bit sequence the far end has sent since anyone asked, without
    /// the repeats -- a rate signal is sent over and over, and one line per
    /// symbol is not a log.
    ///
    /// A diagnostic and nothing else. What rate a call settled on is decided
    /// entirely by these, and off a recording they are the difference between
    /// knowing why it went to 4800 and guessing.
    seen: Vec<u16>,
    /// The last one, kept separately so that draining `seen` does not make
    /// every sequence look new again.
    last_seen: Option<u16>,
    /// Amplitude of the incoming carrier while it was up, for spotting a drop.
    carrier_peak: f64,
    /// Whether a training segment has been sent yet. Only the first is a
    /// window the echo canceller can learn anything from.
    trained: bool,
    /// A sequence with a rate signal's synchronising bits has arrived, so the
    /// far end's training segment is behind us whether or not enough of them
    /// have arrived to act on.
    seen_a_sequence: bool,
    /// Whether an incoming E sequence has been seen, which ends the rate
    /// exchange for good.
    ///
    /// It has to be latched, and not because a false detection is unlikely
    /// enough to ignore. What follows E is scrambled ones, and scrambled ones
    /// descramble to ones: sixteen of those in a row satisfy every
    /// synchronising bit an E sequence has and offer every rate in Table 6 at
    /// once. So it is not a rare collision but a certainty, arriving every
    /// sixteen bits from the moment the exchange ends. Left running, the
    /// detector conditioned a 4800 connection to receive at 9600 and the data
    /// never arrived.
    heard_end: bool,
    /// Held until the next symbol boundary, where the machine can act on them.
    pending_carrier_reversal: bool,
    pending_sideband_reversal: bool,
    pending_sequence: Option<u16>,
    /// What this modem offers, and what has been settled on.
    offer: u16,
    /// Which of 9600's two modulations the rate exchange settled on.
    coding: Coding,
    agreed: u32,
}

impl Startup {
    /// `offer` is the rate signal this modem sends, from [`rate_signal`].
    pub fn new(role: Role, offer: u16, fs: f64) -> Self {
        let sps = fs / super::BAUD;
        Self {
            role,
            state: match role {
                Role::Calling => State::Listening,
                Role::Answering => State::AnswerTone,
            },
            listener: Listener::new(fs),
            carrier_reversals: ReversalDetector::new(super::CARRIER, 60.0, AUDIBLE, fs),
            low_reversals: ReversalDetector::new(super::CARRIER - OFFSET, 60.0, AUDIBLE, fs),
            high_reversals: ReversalDetector::new(super::CARRIER + OFFSET, 60.0, AUDIBLE, fs),
            carrier_quiet: 0,
            sideband_quiet: 0,
            heard_end: false,
            rates: RateDetector::new(),
            countdown: sps,
            sps,
            symbols: 0,
            total: 0,
            timer: None,
            counted: 0,
            round_trip: 0,
            held: 0,
            unsatisfactory: 0,
            connected_once: false,
            retrains: 0,
            asked_to_retrain: false,
            seen: Vec::new(),
            last_seen: None,
            carrier_peak: 0.0,
            trained: false,
            seen_a_sequence: false,
            pending_carrier_reversal: false,
            pending_sideband_reversal: false,
            pending_sequence: None,
            offer,
            coding: Coding::Uncoded,
            agreed: 0,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Which of 9600's two modulations the rate exchange settled on.
    pub fn coding(&self) -> Coding {
        self.coding
    }

    pub fn status(&self) -> Status {
        match self.state {
            State::Connected(rate) => Status::Connected(rate),
            State::Failed => Status::Failed,
            // Anything else on a call that has been up once is 7's retrain
            // rather than a first negotiation.
            _ if self.connected_once => Status::Retraining,
            _ => Status::Negotiating,
        }
    }

    /// The round trip, in symbol intervals, once it has been measured.
    ///
    /// This is what an echo canceller needs in order to know how far back the
    /// line's reflection of our own signal can be. It is not what 5.4.1 and
    /// 5.4.2 ask a modem to wait; see [`counted`](Self::counted).
    pub fn round_trip(&self) -> u64 {
        self.round_trip
    }

    /// What the counter/timer read, in symbol intervals.
    ///
    /// 5.4.1 has the calling modem send S "for a period NT already estimated
    /// by the counter/timer" and 5.4.2 has the answering modem wait "a period
    /// MT already estimated by the counter/timer". Both are the raw reading
    /// and not [`round_trip`](Self::round_trip), which has the procedure's own
    /// fixed delays taken off it.
    ///
    /// The distinction is the whole point of the two periods. NT and MT are
    /// what makes the two ends meet: the calling modem holds S up for NT and
    /// the answering modem looks for it again after MT, and the 256 symbols
    /// 5.4.1 puts on the end of NT are all the margin there is between them.
    /// Two symmetric clocks measure the same delays, including the ones the
    /// procedure itself contributes, so those cancel and the margin is spent
    /// on the far end's detector. Take them off one side only and the margin
    /// goes with them.
    ///
    /// Measured on a call that failed for it: 166 symbols came off NT, the far
    /// end took 228 to notice the S and cease transmitting as 5.4.2 tells it
    /// to, and when it looked again MT later the S had ended 30 ms earlier. It
    /// waited nearly five seconds for one to reappear and then started the
    /// whole call again. Twice.
    pub fn counted(&self) -> u64 {
        self.counted
    }

    /// Whether this end is sending the training segment, which is the one
    /// stretch of the start-up the far end is required to be silent through
    /// and therefore the only time an echo canceller can learn anything.
    ///
    /// Note 3 to 5.4.2 says as much: the TRN segment "is suitable for training
    /// the echo canceller in the transmitting modem", and allows a separate
    /// sequence before the conditioning signal if a longer one is wanted.
    ///
    /// Only the first one, though. The answering modem sends a conditioning
    /// signal twice, and the second time the calling modem is still sending
    /// R2 over the top of it: 5.4.1 has that continue "until an incoming rate
    /// signal R3 is detected", which cannot arrive until the conditioning
    /// signal it follows is over. Adapting through that has the canceller try
    /// to explain the far end as an echo of us and throw away everything it
    /// learned in the first segment, when the line really was quiet. Left in,
    /// it cost the answering modem its receiver: a residual error of 0.55
    /// against the 0.07 the other end managed on the same call.
    pub fn training_echo(&self) -> bool {
        matches!(self.state, State::SendTrn) && !self.trained
    }

    /// Whether the far end is required to be silent just now.
    ///
    /// True through the whole of this modem's own first conditioning sequence
    /// and not merely its training segment. Everything on the line then is our
    /// own echo, so a receiver that goes on adapting through it is adapting to
    /// the wrong signal, and an equaliser and a carrier loop that have settled
    /// on a modem's own transmission have settled somewhere it will not easily
    /// leave.
    ///
    /// That is not hypothetical either. The calling modem trains its receiver
    /// on the answering modem's first conditioning signal and had it locked, a
    /// residual error of 0.013; it then began its own conditioning sequence,
    /// spent it converging onto its own echo instead, and sat at 0.5 for the
    /// rest of the call, unable to read the R3 it was waiting for. The
    /// answering modem, which is silent while it listens, never had the
    /// problem, which is what made it look like an echo canceller fault.
    ///
    /// Deliberately narrower than [`training_echo`](Self::training_echo),
    /// which is TRN alone. Both are windows where the line carries nothing but
    /// us, but the echo canceller wants only the part of it with no pattern:
    /// S and S-bar repeat every two symbols, and a filter learned from a
    /// periodic reference is one of the many that explain that period and
    /// almost certainly not the one the line is.
    /// Whether the far end's conditioning signal has already taught this
    /// receiver everything it is going to.
    ///
    /// 5.2 makes the conditioning signal S, then S-bar, then a training
    /// segment of at least 1280 symbols, and a receiver has had all of it by
    /// the end of that. What comes next is a rate signal, which is scrambled
    /// and differentially encoded and is data in every way that matters to an
    /// equaliser -- and one that goes on adapting through it is learning from
    /// something it was not given to learn from, on a line that by then has
    /// this end's own echo on it too.
    ///
    /// Left to itself the adaptation ran until this end answered, which is
    /// later and, worse, later by an amount that depends on how well the
    /// adaptation is going: detecting a rate signal takes 5.3.1's two
    /// identical sixteens, and a receiver that has drifted takes longer to see
    /// them, which gives it longer to drift. That is a loop that only turns
    /// one way.
    fn far_end_finished_training(&self) -> bool {
        matches!(self.state, State::AwaitingR1 | State::AwaitingR2)
            && self.seen_a_sequence
    }

    pub fn far_end_quiet(&self) -> bool {
        if self.far_end_finished_training() {
            return true;
        }
        if matches!(self.state, State::Connected(_)) {
            // The listener below is only fed while the start-up is running, so
            // its answer goes stale the moment this connects. Data state has
            // its own reasons to keep adapting and none to stop.
            return false;
        }
        if matches!(
            self.state,
            State::PreRoll | State::SendS | State::SendSBar | State::SendTrn
        ) && !self.trained
        {
            return true;
        }
        // Or the line simply has nothing on it, which the start-up leaves it
        // with more than once: between one modem finishing a sequence and the
        // other reacting there is a round trip of silence, and an adaptive
        // receiver let loose on silence does not stay where it was put. The
        // calling modem lost a residual error of 0.013 to 0.46 in the 68 ms
        // between starting R2 and the answer arriving.
        self.listener.classify() == Heard::Nothing
    }

    /// How long the training segment is being sent for, in symbols.
    ///
    /// 5.2.3 allows anything from 1280 to 8192, and which end of that to use
    /// is decided by the round trip already measured: a line short enough that
    /// everything reflected comes back inside the near taps has only one job
    /// to do here, and a longer one has two.
    pub fn training_symbols(&self) -> u64 {
        let near = (ECHO_SPAN_MS * super::BAUD / 1000.0) as u64;
        if self.round_trip > near {
            timing::SEGMENT_TRN_LONG
        } else {
            timing::SEGMENT_TRN
        }
    }

    /// Which step of the procedure this end is on, for diagnostics and for
    /// anything that wants to show progress.
    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Listening => "listening",
            State::Aa => "AA",
            State::AaToCc => "AA to CC",
            State::Cc => "CC",
            State::AwaitingR1 => "awaiting R1",
            State::PreRoll => "S pre-roll",
            State::AnswerTone => "answer tone",
            State::RetrainAc => "AC (retrain)",
            State::Ac => "AC",
            State::Ca => "CA",
            State::CaToAc => "CA to AC",
            State::AcAgain => "AC again",
            State::Gap => "gap",
            State::AfterR1 => "after R1",
            State::AwaitingR2 => "awaiting R2",
            State::SendS => "S",
            State::SendSBar => "S bar",
            State::SendTrn => "TRN",
            State::SendRate => "rate signal",
            State::SendEnd => "E",
            State::Settling => "settling",
            State::Connected(_) => "connected",
            State::Failed => "failed",
        }
    }

    /// Ask for a retrain: 7's "unsatisfactory signal reception", decided
    /// somewhere other than here.
    ///
    /// Takes effect at the next step, and only on a call that is up. There is
    /// nothing to retrain otherwise, and a start-up told to start again would
    /// simply lose whatever progress it had made.
    pub fn ask_for_retrain(&mut self) {
        if matches!(self.state, State::Connected(_)) {
            self.asked_to_retrain = true;
        }
    }

    /// How many times this call has gone back through the start-up.
    ///
    /// Worth showing. One retrain is a line that changed; a handful is a line
    /// that cannot hold what the two ends keep agreeing on.
    pub fn retrains(&self) -> u32 {
        self.retrains
    }

    /// The 16-bit sequences the far end has sent, in order, without the
    /// repeats.
    pub fn take_sequences(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.seen)
    }

    /// What the line is carrying at the moment.
    pub fn heard(&self) -> Heard {
        self.listener.classify()
    }

    /// Advance one sample: listen, and decide what to send.
    pub fn step(&mut self, line: f64, tx: &mut Transmitter, rx: &mut Receiver) -> Status {
        self.listener.feed(line);
        rx.feed(line);

        // Reversals and rate sequences arrive whenever they arrive, which is
        // very rarely on one of this machine's symbol boundaries. Both are
        // therefore latched until the next one: the state machine runs a
        // symbol at a time, and anything not held for it is simply lost.
        //
        // The carrier and the sidebands are kept apart rather than run
        // together, because which of them a modem should be listening to
        // depends on which end of the call it is. A calling modem repeats one
        // state, which puts everything at 1800 Hz and nothing at the
        // sidebands; an answering modem alternates, which does the exact
        // reverse. Each therefore listens where its own signal is not, and is
        // deaf to its own reflection by construction — the same trick
        // `Listener::classify` relies on, and it has to be the same here.
        //
        // Watching both at once looks harmless and is not. A modem hears its
        // own hybrid at once and the far end after the length of the line, so
        // whichever of the two came back first stopped the clock, and it was
        // always the hybrid. Both ends duly measured a round trip of zero on a
        // line hundreds of miles long, and did it in a way nothing caught,
        // because a test line with no echo on it has nothing else to hear.
        let at_carrier = self.carrier_reversals.feed(line);
        // Both sidebands belong to the one signal and turn together.
        let at_low = self.low_reversals.feed(line);
        let at_high = self.high_reversals.feed(line);
        let carrier = self.gate(at_carrier, true);
        let sidebands = self.gate(at_low || at_high, false);

        // Bits arriving feed the rate detector while there is still a rate to
        // agree; afterwards they are the caller's, as data.
        self.pending_carrier_reversal |= carrier;
        self.pending_sideband_reversal |= sidebands;
        if !matches!(self.state, State::Connected(_) | State::Failed) {
            // Drained either way, so that nothing accumulates in the receiver
            // to be handed up as data later; fed to the detector only while
            // there is still a rate exchange going on.
            for bit in rx.take_bits() {
                if self.heard_end {
                    continue;
                }
                if let Some(s) = self.rates.feed(bit) {
                    if self.last_seen != Some(s) {
                        self.last_seen = Some(s);
                        self.seen.push(s);
                    }
                    self.pending_sequence = Some(s);
                }
            }
        }

        self.countdown -= 1.0;
        if self.countdown > 0.0 {
            return self.status();
        }
        self.countdown += self.sps;
        // 5.4: "When the modem detects an incoming 16-bit E sequence ... it
        // shall condition itself to receive data at the rate and with the
        // coding indicated by the E sequence." Handled here rather than in the
        // state machine because it is not a step of the procedure: it can
        // arrive in more than one state, and what it changes is the
        // demodulator rather than anything the machine is doing.
        if self.expecting_end()
            && let Some(e) = self.pending_sequence.filter(|&s| is_end_signal(s))
        {
            self.heard_end = true;
            rx.set_data_rate(offered_rate(e));
            rx.set_coding(agreed_coding(e, self.offer, offered_rate(e)));
        }
        let carrier = std::mem::take(&mut self.pending_carrier_reversal);
        let sidebands = std::mem::take(&mut self.pending_sideband_reversal);
        let sequence = self.pending_sequence.take();
        // The far end's training segment ends where its rate signal begins,
        // and one sequence with the synchronising bits of 5.3.1 is enough to
        // know that. Acting on a rate signal needs two identical ones, which
        // is later -- and later by an amount that depends on how well this
        // receiver is doing, so a receiver that has drifted takes longer to
        // see them and is given longer to drift. Training stops at the end of
        // the training, not at the end of the argument about it.
        if sequence.is_some_and(|s| is_rate_signal(s) || is_end_signal(s)) {
            self.seen_a_sequence = true;
        }
        self.symbols += 1;
        self.total += 1;
        if let Some(t) = self.timer.as_mut() {
            *t += 1;
        }
        self.advance(carrier, sidebands, sequence, tx, rx);
        if !matches!(self.state, State::Connected(_) | State::Failed)
            && self.total >= timing::PATIENCE
        {
            self.state = State::Failed;
        }
        self.status()
    }

    /// Whether an E sequence could legitimately arrive just now.
    ///
    /// 5.3.2 sends exactly one E, so a detector cannot ask it to repeat the
    /// way it asks a rate signal to, and seven fixed bits out of sixteen is
    /// all that separates one from scrambled data. That comes up about once a
    /// second at 4800 bit/s, so the only thing keeping the exchange honest is
    /// not listening for an E until the procedure is due to produce one.
    fn expecting_end(&self) -> bool {
        if self.agreed == 0 {
            return false;
        }
        match self.role {
            // 5.4.1: the calling modem answers R3 with an E of its own, and
            // only from then on is there one coming back.
            Role::Calling => matches!(self.state, State::SendEnd | State::Settling),
            // 5.4.2: the answering modem sends R3 until the calling modem
            // closes the exchange, so it is waiting for one the whole time.
            Role::Answering => matches!(
                self.state,
                State::SendRate | State::SendEnd | State::Settling
            ),
        }
    }

    /// Report a reversal at most once, and not again for a while.
    ///
    /// A phase reversal is an event in a signal that goes on either side of
    /// it, and a detector watching for one has no way to tell a second event
    /// from its own recovery from the first.
    fn gate(&mut self, fired: bool, is_carrier: bool) -> bool {
        let quiet = if is_carrier {
            &mut self.carrier_quiet
        } else {
            &mut self.sideband_quiet
        };
        if *quiet > 0 {
            *quiet -= 1;
            false
        } else if fired {
            *quiet = (self.sps * 16.0) as u64;
            true
        } else {
            false
        }
    }

    /// One symbol of the state machine.
    ///
    /// The two reversals are separate arguments rather than one, so that a
    /// state has to say which signal it is listening to. The far end's is the
    /// only right answer, and it is a different one at each end of the call.
    fn advance(
        &mut self,
        carrier_reversal: bool,
        sideband_reversal: bool,
        sequence: Option<u16>,
        tx: &mut Transmitter,
        rx: &mut Receiver,
    ) {
        let heard = self.listener.classify();
        // How long the far end has been sending something with no line in it,
        // which is its conditioning signal and then its rate signal. Broken by
        // anything else, so a gap starts the count again.
        match self.state {
            // ---- calling modem -------------------------------------------
            State::Listening => {
                // 5.4.1: silent until the answering modem is heard. Note 1
                // there allows starting on the alternating tones alone, since
                // the answering tone may have been truncated or suppressed.
                tx.set_signal(Signal::Silent);
                // Either the answering tone heard for a second, or the
                // alternating tones on their own: note 1 to 5.4.2 allows the
                // second, since the answering tone may have been truncated or
                // never sent at all on a national connection.
                //
                // The tones are looked for where they are rather than by
                // classifying the whole line, which is what the rest of this
                // phase does too and for the reason given on `classify`.
                // The sidebands have to stand above the answering tone as well
                // as above the floor. A one-pole detector 900 Hz from a tone
                // still passes a fortieth of it, and a fortieth of the
                // answering tone is well clear of any absolute threshold worth
                // having: without this the calling modem hears the answer as
                // its own cue and starts transmitting over it immediately.
                let sidebands = self.listener.sideband_amplitude();
                let tones =
                    sidebands > AUDIBLE && sidebands > self.listener.answer_amplitude();
                let heard_enough = self.hold(heard == Heard::AnswerTone)
                    >= timing::HEARD_ANSWER_TONE;
                if tones || heard_enough {
                    self.enter(State::Aa);
                }
            }
            State::Aa => {
                tx.set_signal(Signal::StateA);
                // 6.1: "conditioned to detect one of two incoming tones at
                // frequencies 600 and 3000 Hz, and *subsequently* to detect a
                // phase reversal in that tone". The order is the whole of it.
                // A reversal is a comparison between the tone now and the tone
                // a moment ago, so there has to have been a tone a moment ago
                // for the comparison to mean anything.
                //
                // In a first call this changes nothing: the far end alternates
                // for seconds before it turns over. In 7's retrain it decides
                // whether the thing works at all, because there the far end
                // turns over a tenth of a second after this end started
                // listening, and a detector still full of data will believe
                // anything.
                // Measured where the tone is rather than by classifying the
                // whole line: this end is transmitting 1800 Hz into its own
                // hybrid, so `classify` sees a carrier and sidebands together
                // and calls it something else entirely. The sidebands are the
                // one place this modem's own signal is not.
                let sidebands = self.listener.sideband_amplitude();
                let steady =
                    self.hold(sidebands > AUDIBLE) >= timing::HEARD_CARRIER;
                // The far end is alternating, so its reversal is in the
                // sidebands. This end is repeating a state, which puts nothing
                // there at all.
                if steady && sideband_reversal {
                    // The far end has turned its alternation over. Start the
                    // clock and owe it a reversal of our own in 64 symbols.
                    self.timer = Some(0);
                    self.enter(State::AaToCc);
                }
            }
            State::AaToCc => {
                if self.symbols >= timing::RESPONSE {
                    tx.set_signal(Signal::StateC);
                    self.enter(State::Cc);
                }
            }
            State::Cc => {
                // Not before the far end could possibly have answered. 5.4.2
                // holds it to "64 +/- 2 symbol periods" between receiving this
                // end's reversal and putting its own on the line, and that is
                // before the line is crossed twice, so 64 symbols after this
                // end turned over is the earliest an answer can exist.
                //
                // Without this a single turnover was taken for two. The
                // detector reported one 26 ms after the other -- 62 symbols,
                // just inside the floor -- and the clock started and stopped
                // inside that gap, giving a round trip of 53 ms on a line
                // whose real one is 1.2 seconds. The start-up went on with it,
                // and the far end, which had measured the same line properly,
                // spent six seconds waiting for a modem that thought the line
                // was twenty times shorter than it is.
                if self.symbols >= timing::RESPONSE && sideband_reversal {
                    // Our reversal has come back, so stop the clock.
                    self.stop_the_clock();
                    tx.set_signal(Signal::Silent);
                    self.enter(State::AwaitingR1);
                }
            }
            State::AwaitingR1 => {
                tx.set_signal(Signal::Silent);
                if let Some(s) = sequence.filter(|&s| is_rate_signal(s)) {
                    self.agreed = usable_rate(s, self.offer)
                        .min(offered_rate(self.offer));
                    self.coding = agreed_coding(s, self.offer, self.agreed);
                    if self.agreed == 0 {
                        self.state = State::Failed;
                        return;
                    }
                    tx.set_signal(Signal::ConditioningS);
                    self.enter(State::PreRoll);
                    return;
                }
                // 5.5.1: on "detection of one of two tones at frequencies
                // 600 +/- 7 Hz and 3000 +/- 7 Hz for more than 128 symbol
                // intervals", the calling modem goes back to repeating state
                // A and into 5.4.1 again.
                //
                // This is the one state in the whole procedure where this end
                // says nothing at all, and so the one place an answering modem
                // that has given up and started again can go unnoticed for as
                // long as anybody is prepared to hold the line. What is being
                // waited for here is a conditioning signal, which stands a
                // carrier up between those two tones; a bare alternation with
                // no carrier is the answering modem back at the top of 5.4.2,
                // waiting for a state A that is never coming.
                //
                // Not straight away, though. The answering modem goes on
                // alternating until it hears this end stop, and that news
                // takes the round trip that has just been measured to reach
                // it -- so on a slow enough connection the tail of a perfectly
                // healthy AC is longer than the rule's 128 symbols.
                //
                // The clock's own reading and not the line's share of it, for
                // the reason given on `counted`: what has to elapse is a trip
                // out and back plus the far end noticing, and the difference
                // between the two numbers is exactly the noticing. Measured
                // with the wrong one, this fired 104 ms early on a real call,
                // abandoning a start-up that was going perfectly and then
                // restarting it into the far end's conditioning signal.
                if self.hold(heard == Heard::Alternation)
                    > self.counted + timing::MIN_ALTERNATION
                {
                    tx.set_signal(Signal::StateA);
                    self.enter(State::Aa);
                }
            }
            State::PreRoll => {
                // 5.4.1: "an S sequence for a period NT already estimated by
                // the counter/timer", which lines this modem's conditioning
                // signal up with the far end's idea of when it should arrive.
                // The clock's own reading, not the line's share of it: the far
                // end waits out a clock of its own and the two only meet if
                // neither has been trimmed.
                if self.symbols >= self.counted {
                    self.enter(State::SendS);
                }
            }

            // ---- answering modem -----------------------------------------
            State::AnswerTone => {
                tx.set_signal(Signal::AnswerTone);
                if self.symbols >= timing::ANSWER_TONE {
                    tx.set_signal(Signal::AlternateAC);
                    self.enter(State::Ac);
                }
            }
            // 7.2: "transmit alternate carrier states A and C for an even
            // number of symbol intervals not less than 128. It shall then
            // proceed in accordance with 6.2 beginning with the third
            // paragraph." The 128 belong to 7.2 and the third paragraph's own
            // 128 come after them -- which is not pedantry: they are what give
            // the far end a settled tone to measure its first reversal
            // against, and without them it measures one against data.
            State::RetrainAc => {
                tx.set_signal(Signal::AlternateAC);
                if self.symbols >= timing::MIN_ALTERNATION {
                    self.enter(State::Ac);
                }
            }
            State::Ac => {
                // 5.4.2: an even number of symbols, at least 128, and "an
                // incoming tone has been detected at 1800 Hz for 64 symbol
                // periods". Looked for at 1800 Hz exactly, where this modem's
                // own alternation puts nothing at all, so its echo of itself
                // cannot be mistaken for the far end.
                self.note_carrier();
                let long_enough =
                    self.symbols >= timing::MIN_ALTERNATION && self.symbols.is_multiple_of(2);
                let tone = self.listener.carrier_amplitude() > AUDIBLE;
                if long_enough && self.hold(tone) >= timing::HEARD_CARRIER {
                    self.timer = Some(0);
                    tx.set_signal(Signal::AlternateCA);
                    self.enter(State::Ca);
                }
            }
            State::Ca => {
                self.note_carrier();
                // The far end is repeating a state, so its reversal is in the
                // bare carrier at 1800 Hz. This end is alternating, which
                // suppresses the carrier and leaves that place empty.
                if carrier_reversal {
                    self.stop_the_clock();
                    self.enter(State::CaToAc);
                }
            }
            State::CaToAc => {
                self.note_carrier();
                if self.symbols >= timing::RESPONSE {
                    tx.set_signal(Signal::AlternateAC);
                    self.enter(State::AcAgain);
                }
            }
            State::AcAgain => {
                // 5.4.2: wait for the incoming tone to drop away, which is the
                // calling modem ceasing to transmit once it has its own
                // measurement.
                let dropped = self.listener.carrier_amplitude() < self.carrier_peak / 4.0;
                if self.hold(dropped) >= timing::GAP {
                    tx.set_signal(Signal::Silent);
                    self.enter(State::Gap);
                }
            }
            State::Gap => {
                if self.symbols >= timing::GAP {
                    self.enter(State::SendS);
                }
            }
            State::AfterR1 => {
                // 5.4.2: having sent R1 and heard the far end's conditioning
                // signal, "wait for a period MT already estimated by the
                // counter/timer" before believing what arrives next. The
                // clock's own reading, for the reason given on `counted`.
                tx.set_signal(Signal::Silent);
                if self.symbols >= self.counted {
                    self.rates.reset();
                    self.enter(State::AwaitingR2);
                }
            }
            State::AwaitingR2 => {
                tx.set_signal(Signal::Silent);
                if let Some(s) = sequence.filter(|&s| is_rate_signal(s)) {
                    self.agreed = usable_rate(s, self.offer)
                        .min(offered_rate(self.offer));
                    self.coding = agreed_coding(s, self.offer, self.agreed);
                    if self.agreed == 0 {
                        self.state = State::Failed;
                        return;
                    }
                    tx.set_signal(Signal::ConditioningS);
                    self.enter(State::SendS);
                }
            }

            // ---- both ends -----------------------------------------------
            State::SendS => {
                tx.set_signal(Signal::ConditioningS);
                if self.symbols >= timing::SEGMENT_S {
                    tx.set_signal(Signal::ConditioningSbar);
                    self.enter(State::SendSBar);
                }
            }
            State::SendSBar => {
                if self.symbols >= timing::SEGMENT_S_BAR {
                    tx.set_signal(Signal::Trn);
                    self.enter(State::SendTrn);
                }
            }
            State::SendTrn => {
                if self.symbols >= self.training_symbols() {
                    self.trained = true;
                    self.rates.reset();
                    tx.set_signal(Signal::Rate(self.offer));
                    self.enter(State::SendRate);
                }
            }
            State::SendRate => {
                // A rate signal has to be sent long enough to be recognised.
                // 5.3.1 asks for two identical sixteens, so anything shorter
                // than a few of them cannot be detected however good the line
                // is: the answering modem was leaving this state six
                // milliseconds after entering it, having sent twenty-eight
                // bits of a thing that takes thirty-two to identify, and the
                // calling modem never saw an R3 at all.
                if self.symbols < timing::MIN_RATE_SIGNAL {
                    return;
                }
                match self.role {
                    // 5.4.2: the first time through, the answering modem is
                    // sending R1 and is waiting for the calling modem's
                    // conditioning signal, not for a rate. The second time it
                    // is sending R3 and waits to be closed out with an E.
                    // Having agreed a rate already is what tells them apart.
                    Role::Answering if self.agreed == 0 => {
                        if self.hold(heard == Heard::Conditioning) >= timing::HEARD_CARRIER {
                            tx.set_signal(Signal::Silent);
                            self.enter(State::AfterR1);
                        }
                    }
                    Role::Answering => {
                        if let Some(s) = sequence.filter(|&s| is_end_signal(s)) {
                            if self.agreed == 0 {
                                self.agreed = usable_rate(s, self.offer);
                                self.coding =
                                    agreed_coding(s, self.offer, self.agreed);
                            }
                            // In whichever table the far end can read: it
                            // has just shown which by what it sent.
                            tx.set_signal(Signal::Rate(end_signal(
                                rate_signal_for(
                                    self.agreed,
                                    self.coding,
                                    is_v32bis(s),
                                ),
                            )));
                            self.enter(State::SendEnd);
                        }
                    }
                    // 5.4.1: "Transmission of R2 shall continue until an
                    // incoming rate signal R3 is detected."
                    Role::Calling => {
                        // Continue, but not for ever. Past the point where a
                        // far end still following 5.4.2 could have one on the
                        // way, whatever is out there is doing something else,
                        // and 5.5.1's answer to that is this end's answer to
                        // it too: back to repetitively transmitting state A
                        // and on again from 5.4.1's third paragraph.
                        if self.symbols > timing::R3_AT_THE_LATEST + self.counted
                        {
                            self.start_again(tx, rx);
                            return;
                        }
                        // "until an incoming rate signal R3 is detected", and
                        // a rate signal only. An E cannot arrive here: the
                        // answering modem sends R3 until this end closes the
                        // exchange, so this end goes first. Accepting one
                        // anyway looks like tolerance and is not, because a
                        // lone E is what scrambled data imitates most easily,
                        // and the far end's training segment is on the line
                        // for the whole of this wait.
                        let Some(s) = sequence.filter(|&s| is_rate_signal(s)) else {
                            return;
                        };
                        let theirs = usable_rate(s, self.offer);
                        if theirs == 0 {
                            // Table 6: no rate at all is a call to clear down.
                            self.state = State::Failed;
                            return;
                        }
                        // Each step down the chain is the lesser of what the
                        // two ends can do: R2 excludes anything R1 did not
                        // offer, and R3 anything R2 did not.
                        let mine = offered_rate(self.offer);
                        self.agreed = if self.agreed == 0 {
                            theirs.min(mine)
                        } else {
                            self.agreed.min(theirs)
                        };
                        self.coding = agreed_coding(s, self.offer, self.agreed);
                        tx.set_signal(Signal::Rate(end_signal(rate_signal_for(
                            self.agreed,
                            self.coding,
                            is_v32bis(s),
                        ))));
                        self.enter(State::SendEnd);
                    }
                }
            }
            State::SendEnd => {
                // 5.3.2: one complete sixteen-bit sequence, which is eight
                // symbols at two bits each -- counted from where the E began
                // rather than from where it was asked for. The clause before
                // it has the rate sequence it interrupts finish first, so the
                // two are up to seven symbols apart and the transmitter is the
                // only thing that knows which.
                if !tx.rate_pending() && tx.rate_symbols() >= 8 {
                    // 5.4: the E just finished says what the scrambled ones
                    // that follow it are coded at, so this is the moment the
                    // transmitter changes rate and not one symbol earlier.
                    // Everything before it -- conditioning signal, training
                    // segment, both rate exchanges -- is two bits to the
                    // symbol whatever was being negotiated.
                    tx.set_data_rate(self.agreed);
                    tx.set_coding(self.coding);
                    tx.set_signal(Signal::ScrambledOnes);
                    self.enter(State::Settling);
                }
            }
            State::Settling => {
                if let Some(s) = sequence.filter(|&s| is_end_signal(s) && self.agreed == 0) {
                    self.agreed = offered_rate(s);
                }
                if self.symbols >= timing::SETTLE {
                    let rate = if self.agreed == 0 { 4800 } else { self.agreed };
                    // The far end changed rate as it finished its own E, which
                    // was at least a hundred and twenty symbols ago whichever
                    // end this is. If that E went unheard, this is the last
                    // chance to end up demodulating what is actually arriving.
                    rx.set_data_rate(rate);
                    rx.set_coding(self.coding);
                    self.state = State::Connected(rate);
                    self.connected_once = true;
                    self.unsatisfactory = 0;
                }
            }
            State::Connected(running_at) => {
                // 7.1 and 7.2. The tone that says the far end has given up on
                // this connection and gone back to the beginning is the same
                // tone it sent at the beginning, so this is the same test the
                // start-up makes -- a calling modem watches for the answering
                // modem's alternation, 600 and 3000 with the carrier
                // suppressed, and an answering modem for the calling modem's
                // bare 1800.
                //
                // Both clauses give the two ends the same trigger for starting
                // one, which is what makes following the far end and deciding
                // to go first the same piece of code: whichever happens, this
                // end ends up sending its own opening signal.
                let asking = match self.role {
                    Role::Calling => heard == Heard::Alternation,
                    Role::Answering => heard == Heard::Carrier,
                };
                if self.hold(asking) > timing::RETRAIN_TONE || self.asked_to_retrain {
                    self.begin_retrain(tx, rx);
                    return;
                }
                // "detection of unsatisfactory signal reception", which 7
                // leaves each implementation to define. This one calls it
                // unsatisfactory when what the equaliser is left with, symbol
                // after symbol, is a large part of the distance to the next
                // point along -- which is the point at which the decisions
                // driving the equaliser are as likely to be wrong as right and
                // nothing downstream can recover.
                let bad =
                    rx.residual_error() > UNSATISFACTORY_GAP * rx.point_spacing();
                self.unsatisfactory = if bad { self.unsatisfactory + 1 } else { 0 };
                if self.unsatisfactory > timing::UNSATISFACTORY {
                    // Going round again at the rate that just failed would
                    // arrive back here, since nothing about the line has
                    // changed and the rate exchange has no memory of its own.
                    // What it has instead is 5.4.1's advice about what to put
                    // in the rate signal: "It is recommended that R2 should
                    // also take account of the likely receiver performance
                    // with the particular GSTN connection", and 5.4.2 says the
                    // same of R3. A rate this receiver has just spent a second
                    // failing to read is the strongest evidence about the
                    // connection there is.
                    self.stop_offering(running_at, rx.residual_error());
                    self.begin_retrain(tx, rx);
                }
            }
            State::Failed => {}
        }
    }

    /// Go back to the beginning of the start-up, keeping the call.
    ///
    /// 7.1 sends the calling modem to the third paragraph of 6.1 and 7.2 sends
    /// the answering modem to the third paragraph of 6.2, which are exactly
    /// where each of them arrives after the answer tone. So the states are the
    /// ones already here; what has to be undone is everything the last
    /// negotiation settled, because none of it is true any more.
    fn begin_retrain(&mut self, tx: &mut Transmitter, rx: &mut Receiver) {
        self.retrains += 1;
        self.asked_to_retrain = false;
        // The patience of PATIENCE is for one attempt at the start-up, and
        // this is a fresh one. Left running, a call that has been up for a
        // minute retrains straight into a timeout.
        self.total = 0;
        self.start_again(tx, rx);
    }

    /// Stop offering `rate`, and everything above the one the measured error
    /// says this line can carry.
    ///
    /// 5.4.1 and 5.4.2 both ask the rate signals to "take account of the
    /// likely receiver performance with the particular GSTN connection", and
    /// leave what that means open. A rate just found unreadable is the
    /// plainest part of it: it goes, and so does everything above it, since
    /// the rates share one trellis code and differ only in how many bits ride
    /// through it untouched -- a constellation this receiver cannot read is a
    /// floor under every denser one.
    ///
    /// How far below is the rest of it, and the reason for not simply taking
    /// one step. A step costs a whole start-up, and on a line with a second's
    /// delay in it that is fifteen seconds; walking down from 14 400 to 7200
    /// is most of a minute, and a far end asked to sit through three of them
    /// hangs up first. Measured on the call this was written for: 14 400 was
    /// unreadable, one step took it to 12 000, that was unreadable too, and
    /// the far end gave up during the second retrain.
    ///
    /// What the error already says is how much room a symbol needs, and every
    /// rate's room is known -- half the distance between neighbouring points.
    /// So the target is the highest rate whose room the error would fit
    /// inside, which on that call was 9600 and would have skipped the wasted
    /// attempt at 12 000.
    ///
    /// Optimistic rather than pessimistic, and knowingly. The error is
    /// measured against the nearest point rather than the right one, so once
    /// decisions start going wrong it stops growing -- it saturates at about
    /// two fifths of a gap however bad the line really is. A rate chosen from
    /// it may therefore still be too fast, and the next retrain will say so.
    /// One step at a time has the same fault and takes longer to find out.
    ///
    /// 4800 is never given up. It is the only rate V.32 requires of both ends,
    /// so an offer without it is an offer of nothing, and Table 6 reads that
    /// as a call to clear down -- which is a decision for whatever is above
    /// this and not for a receiver having a bad second.
    fn stop_offering(&mut self, rate: u32, error: f64) {
        let coding = if offers_trellis(self.offer) {
            Coding::Trellis
        } else {
            Coding::Uncoded
        };
        let mut rates = rates_offered(self.offer);
        for &r in EVERY_RATE.iter() {
            let room = UNSATISFACTORY_GAP * super::point_spacing_at(r, coding);
            if r >= rate || error >= room {
                rates.set(r, false);
            }
        }
        if !rates.any() {
            rates.set(4800, true);
        }
        // In whichever of the two tables this end has been speaking. Table 6
        // and Table 5/V.32bis put the rates in different bits, and a modem
        // that answered in the other one would be offering something else
        // entirely.
        self.offer = if is_v32bis(self.offer) {
            rate_signal(rates)
        } else {
            rate_signal_v32(rates, offers_trellis(self.offer))
        };
    }

    /// Go back to the top of the start-up without granting fresh patience.
    ///
    /// The difference from a retrain is what the clock does. 7 begins again
    /// after a call has been carrying data, and the minute PATIENCE allows is
    /// for the new attempt. Giving up on a start-up that never got anywhere
    /// and trying it again is not a new call, and a modem that reset its own
    /// deadline every time it did so would try for ever.
    fn start_again(&mut self, tx: &mut Transmitter, rx: &mut Receiver) {
        // Everything that listens starts again, because all of it is full
        // of data. A tone detector that has spent a minute on scrambled
        // fourteen-four believes it can already hear 1800 Hz, and an answering
        // modem that believes that skips 6.2's wait and turns over before the
        // calling modem has begun -- which leaves the calling end watching for
        // a reversal that has already happened.
        let fs = self.sps * super::BAUD;
        self.listener = Listener::new(fs);
        self.carrier_reversals =
            ReversalDetector::new(super::CARRIER, 60.0, AUDIBLE, fs);
        self.low_reversals =
            ReversalDetector::new(super::CARRIER - OFFSET, 60.0, AUDIBLE, fs);
        self.high_reversals =
            ReversalDetector::new(super::CARRIER + OFFSET, 60.0, AUDIBLE, fs);
        self.carrier_quiet = 0;
        self.sideband_quiet = 0;
        self.pending_carrier_reversal = false;
        self.pending_sideband_reversal = false;
        self.pending_sequence = None;
        self.rates = RateDetector::new();
        self.seen_a_sequence = false;
        self.unsatisfactory = 0;
        self.agreed = 0;
        self.coding = Coding::Uncoded;
        self.timer = None;
        self.counted = 0;
        self.round_trip = 0;
        self.carrier_peak = 0.0;
        self.trained = false;
        self.heard_end = false;
        // The start-up is conducted in the four states whatever was agreed
        // (5.4), so both ends go back to two bits a symbol before either of
        // them sends anything.
        tx.set_data_rate(4800);
        tx.set_coding(Coding::Uncoded);
        rx.set_data_rate(4800);
        rx.set_coding(Coding::Uncoded);
        match self.role {
            Role::Calling => {
                tx.set_signal(Signal::StateA);
                self.enter(State::Aa);
            }
            Role::Answering => {
                tx.set_signal(Signal::AlternateAC);
                self.enter(State::RetrainAc);
            }
        }
    }

    /// Remember how loud the calling modem's carrier has been, so that its
    /// going away can be recognised as a drop rather than against a threshold
    /// that would have to be told what the line is scaled to.
    fn note_carrier(&mut self) {
        self.carrier_peak = self.carrier_peak.max(self.listener.carrier_amplitude());
    }

    /// Stop the clock and keep both of the things it has measured.
    ///
    /// The reading itself is NT and MT, which 5.4.1 and 5.4.2 hand straight
    /// back to the two modems as periods to wait; nothing is taken off it,
    /// because both ends measure the same delays and what is on both sides
    /// cancels.
    ///
    /// What the echo canceller wants out of the same measurement is a
    /// different number: how far back down the line its own signal can come
    /// from. Four things sit between the two events the clock is started and
    /// stopped by, and only one of them is the line.
    ///
    /// The two ends do not measure the same interval, which is the part most
    /// easily got wrong. The calling modem starts its clock on *detecting* the
    /// far end's reversal and stops it on detecting the answer, so both 64
    /// symbol waits fall inside: its own and the far end's. The answering
    /// modem starts its clock on *sending* its own reversal, so only the far
    /// end's wait is inside. Subtracting 64 at both would leave the calling
    /// modem reading a round trip 64 symbols too long.
    ///
    /// The rest is the machinery at each end. The shaper holds a pulse back
    /// while it is being formed, and the reversal detector cannot report
    /// anything until its average has followed the signal round; both happen
    /// twice, once going and once coming back, and on a short line they come
    /// to more than the line does.
    fn stop_the_clock(&mut self) {
        let response = match self.role {
            Role::Calling => 2 * timing::RESPONSE,
            Role::Answering => timing::RESPONSE,
        };
        let latency = 2.0 * f64::from(self.low_reversals.latency()) / self.sps;
        let overhead = response + latency.round() as u64 + 2 * super::SHAPING_DELAY;
        self.counted = self.timer.take().unwrap_or(0);
        self.round_trip = self.counted.saturating_sub(overhead);
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.symbols = 0;
        self.held = 0;
        // Each wait for a rate signal is its own: the second conditioning
        // signal is training too, and this end has to be allowed to learn from
        // it.
        self.seen_a_sequence = false;
    }

    /// Symbols the condition being waited for has held unbroken.
    fn hold(&mut self, present: bool) -> u64 {
        if present {
            self.held += 1;
        } else {
            self.held = 0;
        }
        self.held
    }
}

/// A complete V.32 modem: one end of a call, on a two-wire line.
///
/// Ties together the four things that have to run at once and cannot be run
/// separately. The transmitter and receiver share a band, so the receiver
/// hears the transmitter; the echo canceller removes that, but only once it
/// has been trained, and the only time it can be trained is while the far end
/// is required to be silent; and knowing when that is means following the
/// start-up. Each of those is testable on its own and none of them is much use
/// on its own.
#[derive(Debug)]
pub struct Modem {
    tx: Transmitter,
    rx: Receiver,
    startup: Startup,
    echo: EchoCanceller,
    /// Looks for the network's reflection while the line is quiet enough to
    /// find it. Dropped once it has answered.
    finder: Option<EchoFinder>,
    /// Samples spent looking, and how many to spend.
    searched: usize,
    search_for: usize,
    /// What the search turned up, kept for diagnostics: the number is the
    /// difference between a canceller that works on a long line and one that
    /// does not, and there is no way to see it from outside.
    reflection: Option<Reflection>,
    fs: f64,
    /// The return loss as training ended, which is the last moment it means
    /// anything: with both ends talking the meter compares everything heard
    /// against everything left, and the far end is in both.
    trained_loss: f64,
    was_training: bool,
}

/// How far back the first run of taps looks, in milliseconds.
///
/// The reflection off a hybrid, which is immediate, and a little of what the
/// network adds behind it.
const ECHO_SPAN_MS: f64 = 8.0;

/// How much line the second run of taps covers, in milliseconds.
///
/// A network reflection is one path among many rather than one impedance step,
/// so it arrives smeared rather than as a copy. This is what that smearing is
/// allowed to be; anything longer is taps modelling nothing.
const FAR_SPAN_MS: f64 = 4.0;

/// Weakest reflection worth a second run of taps.
///
/// Under this it is not clear there is a reflection at all. The search takes
/// the largest of some hundreds of candidates, and the largest of hundreds of
/// numbers that should all be zero is not zero; a bar has to sit above what
/// that alone produces. Above it, the reflection is within about 8 dB of the
/// far modem, which is close enough to be worth removing.
const FAINTEST: f64 = 0.15;

impl Modem {
    /// `offer` is the rate signal this modem sends, from [`rate_signal`].
    pub fn new(role: Role, offer: u16, fs: f64) -> Self {
        let (tx, rx) = endpoints(role, fs);
        Self {
            tx,
            rx,
            startup: Startup::new(role, offer, fs),
            echo: EchoCanceller::new((ECHO_SPAN_MS * fs / 1000.0) as usize, 0.5),
            finder: None,
            searched: 0,
            search_for: 0,
            reflection: None,
            fs,
            trained_loss: 0.0,
            was_training: false,
        }
    }

    /// Take one sample from the line and give back the one to put on it.
    pub fn step(&mut self, line: f64) -> f64 {
        // The canceller is told what went out and what came back, and returns
        // what is left. Its own history remembers how long ago each sample
        // was sent, so the caller need not.
        let sent = self.tx.last_sample();

        // The training segment is the only stretch of the start-up where the
        // line carries our own signal and nothing else, so it is the only
        // chance to find out where the line puts it back as well as what shape
        // it comes back in. The first half goes on finding it and the second
        // on cancelling it.
        if self.startup.training_echo() && !self.was_training {
            self.begin_search();
        }
        if let Some(finder) = self.finder.as_mut() {
            // What arrived, rather than what the canceller left of it: the
            // near echo it removes is nowhere near the delays being searched,
            // and this way the search does not depend on how the near taps are
            // getting on.
            finder.feed(sent, line);
            self.searched += 1;
            if self.searched >= self.search_for {
                self.place_far_taps();
            }
        }

        let cleaned = self.echo.process(sent, line);

        // Adapt only while this end is transmitting its training segment,
        // which is the one stretch the far end is required to be quiet for.
        // Adapting through the far end would have the canceller try to explain
        // it as an echo of us, which it is not, and unlearn what it knows.
        let training = self.startup.training_echo();
        // The equaliser is held still for the same reason the canceller is let
        // loose, and over a longer stretch: what is on the line through the
        // whole of our own conditioning sequence is this modem's own echo, and
        // there is nothing in it for a receiver to learn.
        self.rx.set_adapting(!self.startup.far_end_quiet());
        if self.was_training && !training {
            self.trained_loss = self.echo.echo_return_loss();
        }
        self.was_training = training;
        self.echo.set_adapting(training);

        // The start-up keeps running for the whole call, and not because it
        // has anything left to do. 7 has a retrain begin with the tone the
        // far end opened with, and it may begin at any moment; 8.2 says the
        // same of rate renegotiation -- "a modem shall be conditioned to
        // detect an incoming preamble at any time while receiving data". A
        // machine that stops listening once it is connected cannot hear
        // either, which is what a far end retraining looked like from here:
        // nothing at all, and a demodulator quietly turning a conditioning
        // signal into rubbish.
        //
        // It feeds the receiver itself, so there is one path in either case.
        self.startup.step(cleaned, &mut self.tx, &mut self.rx);
        self.tx.next_sample()
    }

    /// Start looking for a network reflection, if there is anywhere for one to
    /// be that the near taps do not already cover.
    ///
    /// The round trip measured during the start-up is what bounds the search.
    /// Nothing can come back later than that, so the delays past it need not
    /// be considered, and on a short line there is nothing to consider at all.
    ///
    /// The bound is the measurement plus the width of the taps being placed,
    /// and it needs the slack to be in that direction rather than the other:
    /// searching too far costs arithmetic during a stretch where there is time
    /// for it, and searching too close in misses the reflection entirely. What
    /// slack there is turns out to be spare, because the measurement errs
    /// long — 81 symbols against a true 80 on a clean line, 106 against 96
    /// on one where the returning signal is weak enough to slow the detector
    /// down, which is the error the subtraction models least well.
    fn begin_search(&mut self) {
        let first = self.echo.span();
        let last = self.round_trip_samples() + self.far_taps();
        if last <= first {
            return;
        }
        self.finder = Some(EchoFinder::new(first, last));
        self.searched = 0;
        self.search_for =
            (self.startup.training_symbols() as f64 / 2.0 * self.fs / super::BAUD) as usize;
    }

    /// Put the second run of taps where the search says the reflection is.
    fn place_far_taps(&mut self) {
        let Some(finder) = self.finder.take() else {
            return;
        };
        let Some(found) = finder.best().filter(|f| f.strength >= FAINTEST) else {
            return;
        };
        // Centred on the reflection rather than starting at it: what comes
        // back is the signal through whatever the line did to it, and that
        // spreads either side of where the bulk of it lands.
        let taps = self.far_taps();
        let offset = found.delay.saturating_sub(taps / 2).max(self.echo.span());
        self.echo.watch_far_echo(offset, taps);
        self.reflection = Some(found);
    }

    fn far_taps(&self) -> usize {
        (FAR_SPAN_MS * self.fs / 1000.0) as usize
    }

    fn round_trip_samples(&self) -> usize {
        (self.startup.round_trip() as f64 * self.fs / super::BAUD) as usize
    }

    /// The network reflection the canceller went looking for, if it found one.
    pub fn reflection(&self) -> Option<Reflection> {
        self.reflection
    }

    /// Which of 9600's two modulations the rate exchange settled on.
    pub fn coding(&self) -> Coding {
        self.startup.coding()
    }

    /// Whether the receiver is being allowed to learn from what is arriving.
    pub fn receiver_adapting(&self) -> bool {
        !self.startup.far_end_quiet()
    }

    pub fn status(&self) -> Status {
        self.startup.status()
    }

    pub fn phase(&self) -> &'static str {
        self.startup.phase()
    }

    /// Ask for a retrain (7).
    pub fn ask_for_retrain(&mut self) {
        self.startup.ask_for_retrain();
    }

    /// How many times this call has gone back through the start-up.
    pub fn retrains(&self) -> u32 {
        self.startup.retrains()
    }

    /// The rate sequences the far end has sent.
    pub fn take_sequences(&mut self) -> Vec<u16> {
        self.startup.take_sequences()
    }

    /// The round trip the start-up measured, in symbol intervals.
    /// Whether the far end is still there.
    ///
    /// Measured on the echo-cancelled signal, which is the only place it can
    /// honestly be measured: V.32 puts both directions in one band, so the raw
    /// line carries this modem's own transmission whether or not anybody is
    /// listening to it, and a detector pointed at that would report a carrier
    /// for as long as we kept talking to ourselves.
    pub fn carrier(&self) -> bool {
        self.rx.carrier()
    }

    pub fn round_trip(&self) -> u64 {
        self.startup.round_trip()
    }

    /// What the counter/timer read: 5.4.1's NT and 5.4.2's MT.
    pub fn counted(&self) -> u64 {
        self.startup.counted()
    }

    /// How much of its own echo the modem was removing when it finished
    /// training, in decibels.
    ///
    /// Taken then because that is the last moment the figure means anything.
    /// The meter is the ratio of everything heard to everything left, and once
    /// the far end is talking it is in both, so the number falls towards the
    /// ratio of echo to far signal however well the cancelling is going.
    pub fn echo_return_loss(&self) -> f64 {
        self.trained_loss
    }

    /// What the canceller is taking out right now.
    ///
    /// Not the same number as [`Self::echo_return_loss`], which is frozen at
    /// the end of training because that is the last moment it means what it
    /// says. With both ends talking the meter compares everything heard
    /// against everything left and the far end is in both, so perfect
    /// cancellation of an echo as loud as the far end reads about three
    /// decibels rather than forty.
    ///
    /// It is still worth watching. Nothing at all reads zero, and zero while a
    /// reflection has been found and taps placed on it is a canceller that has
    /// been pointed at the wrong place.
    pub fn echo_return_loss_now(&self) -> f64 {
        self.echo.echo_return_loss()
    }

    /// Queue data for transmission. Only meaningful once connected.
    pub fn send(&mut self, bytes: &[u8]) {
        self.tx.push_bytes(bytes);
    }

    pub fn take_bytes(&mut self) -> Vec<u8> {
        self.rx.take_bytes()
    }

    /// Bits recovered from the line.
    ///
    /// What sits above a data pump under V.42 wants bits rather than bytes:
    /// the frames say where the octet boundaries are and the pump has no
    /// business guessing at them.
    pub fn take_bits(&mut self) -> Vec<bool> {
        self.rx.take_bits()
    }

    /// Queue bits for transmission.
    pub fn send_bits(&mut self, bits: &[bool]) {
        self.tx.push_bits(bits);
    }

    /// How many are still waiting to go out, so that whatever is feeding this
    /// knows when to hand over more.
    pub fn pending_bits(&self) -> usize {
        self.tx.pending_bits()
    }

    pub fn constellation_point(&self) -> (f64, f64) {
        self.rx.constellation_point()
    }

    /// Mean distance of the received symbols from the decisions made about
    /// them, which is how well the receiver is doing.
    pub fn residual_error(&self) -> f64 {
        self.rx.residual_error()
    }

    /// Whether the equaliser is still adapting blind, for a test to look at.
    pub fn equalizer_blind(&self) -> bool {
        self.rx.equalizer_blind()
    }

    /// The distance between neighbouring points of the constellation being
    /// received, which is what [`residual_error`](Self::residual_error) has to
    /// be read against.
    pub fn point_spacing(&self) -> f64 {
        self.rx.point_spacing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    /// What a listener makes of half a second of one signal.
    fn hear(signal: Signal, mode: Mode) -> Heard {
        let mut tx = Transmitter::new(mode, FS);
        tx.set_signal(signal);
        let mut listener = Listener::new(FS);
        for _ in 0..(FS as usize / 2) {
            listener.feed(tx.next_sample());
        }
        listener.classify()
    }

    #[test]
    #[ignore]
    fn lines_of_each_signal() {
        for (name, signal, mode) in [
            ("Silent", Signal::Silent, Mode::Call),
            ("AnswerTone", Signal::AnswerTone, Mode::Answer),
            ("StateA", Signal::StateA, Mode::Call),
            ("AlternateAC", Signal::AlternateAC, Mode::Answer),
            ("ConditioningS", Signal::ConditioningS, Mode::Call),
            ("Trn", Signal::Trn, Mode::Call),
            ("ScrambledOnes", Signal::ScrambledOnes, Mode::Call),
            ("Rate", Signal::Rate(0x0699), Mode::Call),
        ] {
            let mut tx = Transmitter::new(mode, FS);
            tx.set_signal(signal);
            let mut l = Listener::new(FS);
            for _ in 0..(FS as usize / 2) {
                l.feed(tx.next_sample());
            }
            let level = l.level();
            println!(
                "{name:>14}: level {level:.3}  answer {:.3}  carrier {:.3}                   low {:.3}  high {:.3}   ratios {:.2} {:.2} {:.2}",
                l.answer.amplitude(), l.carrier.amplitude(),
                l.low.amplitude(), l.high.amplitude(),
                l.answer.amplitude() / level, l.carrier.amplitude() / level,
                l.low.amplitude().min(l.high.amplitude()) / level,
            );
        }
    }

    #[test]
    fn each_start_up_signal_is_recognised_for_what_it_is() {
        assert_eq!(hear(Signal::Silent, Mode::Call), Heard::Nothing);
        assert_eq!(hear(Signal::AnswerTone, Mode::Answer), Heard::AnswerTone);
        assert_eq!(hear(Signal::StateA, Mode::Call), Heard::Carrier);
        assert_eq!(hear(Signal::StateC, Mode::Call), Heard::Carrier);
        assert_eq!(hear(Signal::AlternateAC, Mode::Answer), Heard::Alternation);
        assert_eq!(hear(Signal::AlternateCA, Mode::Answer), Heard::Alternation);
        assert_eq!(
            hear(Signal::ConditioningS, Mode::Call),
            Heard::Conditioning
        );
        assert_eq!(
            hear(Signal::ConditioningSbar, Mode::Call),
            Heard::Conditioning
        );
        assert_eq!(hear(Signal::Trn, Mode::Call), Heard::Spread);
        assert_eq!(hear(Signal::ScrambledOnes, Mode::Call), Heard::Spread);
        assert_eq!(hear(Signal::Rate(0x0699), Mode::Call), Heard::Spread);
    }

    #[test]
    fn an_alternation_is_not_confused_with_the_conditioning_signal() {
        // The two put their sidebands in the same place and differ only in
        // whether the carrier survives, which is the one thing the start-up
        // turns on: hearing 5.2's conditioning signal when 5.4's alternation
        // was sent would have a modem skip the whole delay measurement.
        assert_ne!(
            hear(Signal::AlternateAC, Mode::Answer),
            hear(Signal::ConditioningS, Mode::Answer)
        );
    }

    #[test]
    fn a_rate_signal_says_what_it_offers() {
        let r = rate_signal(Rates { at_4800: true, ..Rates::default() });
        assert!(is_rate_signal(r), "{r:016b} lacks its synchronising bits");
        assert!(!is_end_signal(r));
        assert_eq!(offered_rate(r), 4800);

        let both = rate_signal(Rates { at_4800: true, at_9600: true, ..Rates::default() });
        assert_eq!(offered_rate(both), 9600, "the highest offered wins");

        let e = end_signal(r);
        assert!(is_end_signal(e), "{e:016b} is not recognised as E");
        assert!(!is_rate_signal(e), "E was taken for a rate signal");
        assert_eq!(offered_rate(e), 4800, "E carries the rate it settles on");
    }

    #[test]
    fn no_rate_at_all_calls_for_a_cleardown() {
        // Table 6: B4 to B6 all zero. A modem that reads that as some default
        // rate would keep talking to one that has given up.
        let none = rate_signal(Rates::default());
        assert!(is_rate_signal(none));
        assert_eq!(offered_rate(none), 0);
    }
}
