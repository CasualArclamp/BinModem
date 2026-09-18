//! A complete V.42 endpoint: compression over error control over framing.
//!
//! The three layers are separately testable and separately useless. Data from
//! the terminal is compressed by V.42bis, carried in the information field of
//! a LAPM frame, wrapped in HDLC with a check sequence, and handed to the data
//! pump a bit at a time; and the reverse coming back. What the layers need
//! from each other is narrow but fiddly, and every place that wanted a working
//! link was assembling it by hand.
//!
//! The line side is a bit at a time and never runs dry. A synchronous
//! connection always carries something, and when there is nothing to say that
//! something is the flag: it keeps the far end's framing synchronised, and its
//! absence is how a modem notices the connection has gone.

use crate::detect::{Answer, Answerer, Originator, Outcome};
use crate::frame::{Address, Frame, Kind, Role};
use crate::hdlc::{Decoder, Encoder, Fcs};
use crate::lapm::{Cause, Event, Lapm, Params, State};
use crate::{v42bis, v44};
use crate::xid::{Compression, Xid};

/// The data link both ends use for user data (V.42 8.1.2).
const DLCI_DATA: u8 = 0;

/// Flags before the first protocol frame (V.42 8.10.2, Note).
///
/// "When sending the above frame as the first protocol frame following the
/// detection phase (if used) or establishment of the physical connection (if
/// the detection phase is not used), the originator shall first transmit flag
/// patterns for a period of time sufficient to guarantee the transmission of
/// at least 16-flag patterns."
///
/// The reason is on the other side of the line. The two ends leave the
/// detection phase at different moments -- the answerer has to finish saying
/// what it is saying -- and while it is still there, every bit it is handed
/// goes to its detector rather than its deframer. Flags are what tell it the
/// protocol phase has begun (7.2.1.3), and sixteen of them is long enough that
/// it cannot miss the change and then miss the frame as well.
const LEADING_FLAGS: usize = 16;

/// N400 for the XID exchange: how many times the command is sent again.
///
/// 8.10.3 bounds the retransmissions by N400 and 9.2.2 sets no default for it,
/// only "a minimum value of 1". Appendix III.2 asks for more than that, and
/// asks twice. Its first paragraph asks unconditionally: "for increase
/// robustness under adverse channel conditions, this parameter should be set
/// to a relatively large value (e.g. 16), such that repeated attempts of a
/// procedure requiring a response shall be made over a span of several
/// seconds". Its second adds a reason that applies only after the detection
/// phase, where "the originator possesses a high degree of confidence that
/// the answerer is indeed capable of LAPM operation", and a caveat for where
/// the detection phase is omitted.
///
/// So the appendix wants sixteen here too, and this is one. What it is buying
/// with the other fifteen is the case where a response *is* coming and noise
/// spoils it, and that case is worth much less to this procedure than to the
/// one the appendix has in mind. An establishment that fails leaves no link at
/// all; this failing leaves a link without compression, and the far end's XID
/// is still read and still answered if it turns up afterwards
/// ([`Stack::receive_xid`] runs in [`Phase::Protocol`] too), so a spoiled
/// response costs a feature and not a call. Against that, every retry is paid
/// for on each call whose far end simply does not negotiate -- a far end the
/// detection phase has shown does LAPM has shown nothing about XID -- and it
/// is paid in dead time before the terminal's CONNECT ([`XID_LAST_WAIT_MS`]).
/// 9.2.2 leaves the value to each end, so this is that choice.
///
/// It decides how long a call spends negotiating, because the wait is exactly
/// the retransmissions: one XID, T401, one more, and then a round trip before
/// 8.10.3's other ending ([`XID_LAST_WAIT_MS`]). One retransmission is the
/// least any can cost, and T401 is already the line's own figure -- 1038 ms at
/// 28 800 bit/s with nothing measured, 1751 ms behind a measured 1142 ms round
/// trip -- so the budget comes to 2.0 s and 2.9 s on those two lines, the same
/// order as the fixed second this replaces. Allowing two retransmissions would
/// have put a third of a call's set-up time into asking a question twice over.
///
/// The one loss this covers is the reason the second copy is worth sending at
/// all: 7.2.1.3 has the answerer send its detection pattern at least ten times
/// and stay in the detection phase until it has, feeding what arrives to its
/// detector rather than its deframer, so an XID that reaches it inside that
/// window is swallowed whole.
const XID_N400: u32 = 1;

/// What the exchange waits after its last XID command, in place of the T401
/// 8.10.3 would restart: one round trip, and this where the line was never
/// measured.
///
/// A deliberate departure, and this is what it departs from. 8.10.2 starts
/// T401 on the command and 8.10.3 restarts it on each retransmission, so read
/// literally the exchange ends one whole T401 after the last copy goes out.
/// T401 is not a wait for an answer, though. [`crate::lapm::t401_for_line`]
/// sizes it at half again the measured round trip and never below a second,
/// because what T401 governs everywhere else is a *retransmission*: deciding
/// too early that a SABME was lost sends a second one, and a far end that
/// honours it resets its sequence variables under a link already carrying
/// data. After the last retransmission there is nothing left to send. The only
/// question is when to stop waiting, and an answer, if one is coming at all,
/// arrives a round trip after the copy that asked for it -- so the margin
/// T401 carries is spent on nothing. Stopping early is cheap besides: an XID
/// that lands after this end has gone on is still read and still answered
/// ([`Stack::receive_xid`] runs in [`Phase::Protocol`] too), so the exchange
/// completes late rather than not at all.
///
/// The whole of it was dead time. [`Stack::settled`] is false throughout
/// [`Phase::Negotiating`] and `Modem::announce_connect` returns while it is,
/// so the terminal sees no CONNECT until the exchange is over. Measured on
/// the production path against a far end that does LAPM and never answers
/// XID, from answering the call to the give-up: an unmeasured line went from
/// 1005 to 2085 ms when the second T401 came in, a 200 ms line from 1405 to
/// 2285, a 1142 ms line from 3289 to 4653, and a 4 s line -- where T401
/// saturates at `MAX_T401_MS` and the second one is six whole seconds -- from
/// 9005 to 16009 ms, or 13008 to 20012 to a connected link. Replaying
/// `live-1789647424.wav` reached Data 1.47 s later than before, about 0.88 s
/// of it here.
///
/// With this, the same four lines give up at 2040, 1440, 4060 and 14 000 ms
/// against the 2080, 2280, 4670 and 16 000 the same harness measures for a
/// second T401.
///
/// Where the data pump measured nothing the figure changes almost nothing, and
/// that is right: it is [`crate::lapm::PROPAGATION_MS`], the same
/// Ta + Tb + Te + Tf that T401 itself is built on where there is no
/// measurement, so on that line T401 *is* one round trip and there is no
/// margin in it to take out.
const XID_LAST_WAIT_MS: u32 = crate::lapm::PROPAGATION_MS;

/// Where a V.42 connection has got to.
///
/// The three stages are separate because they answer separate questions, in
/// order: whether the far end does error control at all, what the two of them
/// can agree to do, and then the doing of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The detection phase of 7.2.1, which asks whether there is a V.42 modem
    /// at the far end by sending a pattern only one would recognise.
    Detecting,
    /// Exchanging XID, which settles compression (8.10).
    Negotiating,
    /// LAPM: establishing, established, or releasing.
    Protocol,
    /// The far end does not do error control, or never answered. The
    /// connection is perfectly usable and simply has no protection; what runs
    /// over it is not this stack's business.
    Transparent,
}

/// One frame as it crossed the line, for a record of what a call carried.
///
/// The whole frame between the flags, without the check sequence the framing
/// adds and before anything above has looked at it -- so a frame that did not
/// survive the line is here too, and is the only place it exists. That is the
/// point: a link that comes up and then carries nothing is a question about
/// the frames that could not be read, and by the time anybody asks, every
/// layer above has already dropped them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crossed {
    /// Sent by this end, rather than received.
    pub outbound: bool,
    /// Whether it survived its check sequence. Always true for outbound.
    pub intact: bool,
    pub body: Vec<u8>,
}

/// How many frames are kept before the oldest is dropped.
///
/// A call at 2400 bit/s cannot produce more than about twenty a second, and
/// whatever is draining this is doing so every audio block. The cap is here so
/// that nothing draining it is a bounded mistake rather than an unbounded one.
const LOG_FRAMES: usize = 4096;

/// One end of a V.42 connection.
#[derive(Debug)]
pub struct Stack {
    role: Role,
    lapm: Lapm,
    encoder: Encoder,
    decoder: Decoder,
    /// Compression, when it has been agreed. V.42bis is optional and a
    /// connection without it is a perfectly ordinary V.42 connection.
    compression: Option<Compressor>,
    /// Data ready for the terminal.
    delivered: Vec<u8>,
    /// Frames that arrived but could not be read, which is the measure of how
    /// the line is behaving.
    damaged: u64,
    /// Every frame either way, until somebody takes them.
    log: Vec<Crossed>,
    phase: Phase,
    /// The detection phase, until it is over.
    detect: Detect,
    /// What this end offers.
    offer: Compression,
    /// Whether following an unnegotiated switch was tried and failed, so that
    /// it is tried once and not on every frame for the rest of the call.
    guessed_wrong: bool,
    /// Ceilings on the V.42bis parameters, if the terminal set any.
    ///
    /// Ceilings twice over: what goes into XID, and then 6.4 takes the lower
    /// of the two ends' proposals.
    limits: (u16, u8),
    /// Whether the far end has already said it does LAPM, in V.8.
    declared: bool,
    /// Whether V.44 is offered alongside V.42bis, and V.42bis alongside V.44.
    offer_v44: bool,
    offer_v42bis: bool,
    /// Ceilings on the V.44 parameters this end proposes, transmit and
    /// receive, if the terminal set any (V.250 Table 28).
    v44_limits: Option<(v44::Params, v44::Params)>,
    /// Whether this end is answering the detection phase with a refusal.
    ///
    /// It still runs: 7.2.1.3 has the answerer reply to the ODP whatever its
    /// answer is going to be, and Table 3 gives it one for "no
    /// error-correcting protocol desired". What it must not do is send that
    /// and then go on into XID -- the far end has been told there will be no
    /// protocol and has stopped listening for one.
    declining: bool,
    /// What the far end answered in the detection phase, if it answered.
    heard_adp: Option<Answer>,
    /// What the far end proposed in XID, if it sent one.
    heard_xid: Option<Xid>,
    /// The check sequence width the two ends agreed on, once they have.
    ///
    /// Not in use yet when it is set: V.42 8.10.2 keeps XID at 16 bits and
    /// changes over on the SABME, so this is what the connection *will* use.
    agreed_fcs: Fcs,
    /// Whether the link has ever been up, which is what tells a failure to
    /// establish apart from a connection that later ended.
    established: bool,
    /// Compressed streams that would not decode, each of which took the link
    /// down with it.
    ///
    /// Counted rather than inferred. It is the one failure here that looks
    /// like something else from outside: the link goes, error control starts
    /// again, and every rate and level on the panel is still perfect -- so the
    /// call reads as a line fault when it is this end and the far end
    /// disagreeing about what a codeword means.
    undecodable: u64,
    /// Whether establishment was tried and nothing answered.
    gave_up: bool,
    /// Whether the run of flags that opens the protocol phase has been queued.
    opened: bool,
    /// The XID exchange's own copy of the machinery in 8.10.2 and 8.10.3: an
    /// XID command waiting to go out, how long since the last one went, and
    /// how many times it has been sent again.
    xid_due: bool,
    xid_ms: u32,
    xid_retries: u32,
    /// How long the line takes there and back, where the data pump has said.
    /// Zero where nothing has measured it.
    round_trip_ms: u32,
    /// Whether the far end turned out to be talking to this end's terminal
    /// rather than doing V.42.
    heard_text: bool,
    /// What the line brought that nothing here claimed: the detection phase's
    /// bits once it has failed, and every bit after, for the terminal
    /// (Appendix I.3's second option).
    unclaimed: Vec<bool>,
}

/// One end of the detection phase (7.2.1). Which one depends on the role.
#[derive(Debug)]
enum Detect {
    Origin(Box<Originator>),
    Answer(Box<Answerer>),
    Done,
}

impl Detect {
    /// The detection phase from the beginning, for this end of the call.
    fn start(role: Role, t400_ms: u32, declining: bool) -> Self {
        match role {
            Role::Originator => Self::Origin(Box::new(Originator::new(t400_ms))),
            Role::Answerer => Self::Answer(Box::new(Answerer::new(
                t400_ms,
                if declining { Answer::None } else { Answer::ErrorControl },
            ))),
        }
    }
}

/// Whichever compression the two ends settled on.
///
/// V.44 7.3 has the responder "include parameters for at most one compression
/// algorithm (V.42 bis or V.44) in the response XID", so exactly one of these
/// runs on a call and the choice is made once, during negotiation.
#[derive(Debug)]
enum Codec {
    V42bis(Box<Btlz>),
    V44(Box<Lzjh>),
}

/// V.42bis, whose dictionary is a few hundred bytes of node.
#[derive(Debug)]
struct Btlz {
    encoder: v42bis::Encoder,
    decoder: v42bis::Decoder,
    /// What the two ends agreed to, kept so the dictionaries can be built
    /// again from nothing when the link is established again (5.6).
    params: v42bis::Params,
}

/// V.44, whose history alone is thousands.
///
/// Boxed, with its neighbour, because the two differ enough in size that
/// leaving them inline would make every `Option<Compressor>` the size of the
/// larger one -- and a call without compression would carry it too.
#[derive(Debug)]
struct Lzjh {
    encoder: v44::Encoder,
    decoder: v44::Decoder,
    /// V.44 sizes the two directions separately (7.4), so an encoder and its
    /// peer decoder agree while this end's own pair need not.
    transmit: v44::Params,
    receive: v44::Params,
}

#[derive(Debug)]
struct Compressor {
    codec: Codec,
    /// Turned on without an XID exchange, on the strength of the far end
    /// saying so in the data stream. Decodes only: this end goes on sending
    /// uncompressed, because nothing has agreed that the far end would read
    /// anything else.
    speculative: bool,
}

impl Compressor {
    /// 5.6's C-INIT: both dictionaries back to nothing.
    ///
    /// V.42bis builds its dictionary out of the data that has gone past, and
    /// the two ends only agree because they have seen the same data. A link
    /// that re-establishes discards whatever was unacknowledged (V.42
    /// 8.2.4.3), which takes a piece out of the middle of that stream -- so
    /// from there on the encoder's dictionary and the far decoder's disagree,
    /// every codeword means something else, and nothing that crosses is what
    /// was sent. It never recovers on its own.
    fn reinitialize(&mut self) {
        match &mut self.codec {
            Codec::V42bis(c) => {
                c.encoder = v42bis::Encoder::new(c.params);
                c.decoder = v42bis::Decoder::new(c.params);
            }
            // V.44 7.5 asks for the same thing in the same circumstances:
            // a C-INIT on "receipt of L-ESTABLISH_indication or
            // L-ESTABLISH_confirm".
            Codec::V44(c) => {
                c.encoder = v44::Encoder::new(c.transmit);
                c.decoder = v44::Decoder::new(c.receive);
            }
        }
    }

    /// What to call it on a panel.
    fn name(&self) -> &'static str {
        match self.codec {
            Codec::V42bis(_) => "V.42bis",
            Codec::V44(_) => "V.44",
        }
    }

    /// Compress, and flush so nothing waits for a better match.
    ///
    /// The terminal has no idea it is being compressed and will sit waiting
    /// for an echo of what it typed; a dictionary coder holding the last few
    /// characters back looks exactly like a hung line.
    fn encode(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        match &mut self.codec {
            Codec::V42bis(c) => {
                c.encoder.encode(data, &mut out);
                c.encoder.flush(&mut out);
            }
            Codec::V44(c) => {
                c.encoder.encode(data, &mut out);
                c.encoder.flush(&mut out);
            }
        }
        out
    }

    /// Decompress, or say that it would not.
    fn decode(&mut self, data: &[u8], out: &mut Vec<u8>) -> bool {
        match &mut self.codec {
            Codec::V42bis(c) => c.decoder.decode(data, out).is_ok(),
            Codec::V44(c) => c.decoder.decode(data, out).is_ok(),
        }
    }
}



impl Stack {
    pub fn new(role: Role, params: Params) -> Self {
        Self {
            role,
            lapm: Lapm::new(role, DLCI_DATA, params),
            encoder: Encoder::new(Fcs::Bits16),
            decoder: Decoder::new(Fcs::Bits16),
            compression: None,
            delivered: Vec::new(),
            damaged: 0,
            log: Vec::new(),
            phase: Phase::Detecting,
            detect: Detect::start(role, crate::detect::DEFAULT_T400_MS, false),
            offer: Compression::Neither,
            limits: (v42bis::OFFERED_N2, v42bis::OFFERED_N7),
            guessed_wrong: false,
            declared: false,
            offer_v44: true,
            offer_v42bis: true,
            v44_limits: None,
            declining: false,
            heard_adp: None,
            heard_xid: None,
            agreed_fcs: Fcs::Bits16,
            established: false,
            undecodable: 0,
            gave_up: false,
            opened: false,
            xid_due: false,
            xid_ms: 0,
            xid_retries: 0,
            round_trip_ms: 0,
            heard_text: false,
            unclaimed: Vec::new(),
        }
    }

    /// Note that V.8 has already settled this (V.8 Table 6, 7.3).
    ///
    /// The detection phase still runs -- V.42 Appendix VI.2 observes that many
    /// answering modems run it whatever V.8 said, in order to catch protocols
    /// V.8 has no name for, and V.8 7.3 warns that some ends indicate LAPM and
    /// then require the exchange anyway. What this changes is what a silence
    /// means. Without it, an ADP lost to the line is indistinguishable from a
    /// far end that does no error control, and the safe reading is the second.
    /// With a far end that has already said LAPM in its own words, at 300
    /// bit/s, before any data carrier existed, the safe reading is the first.
    pub fn declared_lapm(mut self) -> Self {
        self.declared = true;
        self
    }

    /// Where the connection has got to.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Go straight to protocol establishment (V.42 7.2.1.2).
    ///
    /// "The detection phase actions by the originator may be disabled by the
    /// user. In this case, the originator moves directly to the protocol
    /// establishment phase." Which is what `+ES` with an `<orig_rqst>` of 2
    /// asks for, and what V.92 9.3.1 requires once V.8 has settled LAPM: a
    /// question already answered is not worth three quarters of a second to
    /// ask again.
    ///
    /// It is a real cost if the far end turns out not to do V.42, though, so
    /// the patience is the same as for a detection phase that heard nothing.
    pub fn without_detection(mut self) -> Self {
        self.enter_negotiating();
        self.lapm.set_retransmissions(crate::lapm::UNCONFIRMED_N400);
        self
    }

    /// Answer the detection phase by declining error control (V.42 Table 3).
    ///
    /// For tests, and for a configuration in which a terminal has asked for a
    /// connection without it.
    pub fn declining(mut self) -> Self {
        self.declining = true;
        self.detect = Detect::start(self.role, self.t400_ms(), true);
        self
    }

    /// Allow for a line that takes this long there and back.
    ///
    /// Every wait in this stack is a wait for the far end, and every one of
    /// them was sized for a line that answers promptly. V.42 9.1.1 says as
    /// much of T400: its 750 ms is "the estimated maximum propagation delay of
    /// all required transmissions including a single satellite link", and a
    /// call carried over a SIP trunk is further away than a satellite. The
    /// data pump has measured the line by the time this is built, so the
    /// detection phase waits that much longer.
    ///
    /// Without it, on a line with a 1.1 s round trip, the originator's T400 ran
    /// out before any ADP could arrive, so only a far end that had named LAPM
    /// in V.8 got error control at all.
    ///
    /// T401 is not set here: it depends on the line rate as well, and comes in
    /// with the [`Params`] -- see [`crate::lapm::t401_for_line`]. The XID
    /// exchange takes its retransmission timer from there, and this figure for
    /// the wait after its last command ([`Self::xid_last_wait_ms`]).
    pub fn over_a_round_trip(mut self, round_trip_ms: u32) -> Self {
        self.round_trip_ms = round_trip_ms;
        if !matches!(self.detect, Detect::Done) {
            self.detect = Detect::start(self.role, self.t400_ms(), self.declining);
        }
        self
    }

    /// V.42 9.1.1's default, and the line on top of it.
    fn t400_ms(&self) -> u32 {
        crate::detect::DEFAULT_T400_MS.saturating_add(self.round_trip_ms)
    }

    /// How long the XID exchange waits after the last command N400 allows it,
    /// before 8.10.3's other ending: one round trip, and not the T401 it
    /// waited before that one. [`XID_LAST_WAIT_MS`] is why.
    fn xid_last_wait_ms(&self) -> u32 {
        let round_trip = match self.round_trip_ms {
            0 => XID_LAST_WAIT_MS,
            measured => measured,
        };
        // Never longer than the wait before it. A round trip past T401 is one
        // `MAX_T401_MS` has already called too long to keep retransmitting
        // over, and waiting it out here would put back what that cap took off.
        round_trip.min(self.lapm.t401_ms())
    }

    /// Offer V.42bis in the XID exchange.
    ///
    /// Offering is all either end can do. What is used is the intersection of
    /// the two offers, because compression that only one end is doing is worse
    /// than none: the far end would decompress data that was never compressed.
    pub fn offer_compression(&mut self, compression: Compression) {
        self.offer = compression;
    }

    /// Do not offer V.44, leaving V.42bis.
    ///
    /// For talking to a far end that predates it, and for showing that the
    /// fall-back 7.3 provides for actually happens.
    pub fn without_v44(&mut self) {
        self.offer_v44 = false;
    }

    /// Do not offer V.42bis, leaving V.44.
    ///
    /// The XID still carries V.42bis's group, saying no direction: a far end
    /// that knows only V.42bis then settles on nothing, as a terminal that
    /// asked for V.44 alone wants.
    pub fn without_v42bis(&mut self) {
        self.offer_v42bis = false;
    }

    /// Cap the V.44 parameters this end proposes, each way.
    ///
    /// V.250 Table 28's `<max_codewords>`, `<max_string>` and `<max_history>`.
    /// 7.4 then takes the lower of the two ends' proposals.
    pub fn offer_v44_limits(&mut self, transmit: v44::Params, receive: v44::Params) {
        self.v44_limits = Some((transmit, receive));
    }

    /// Cap the V.42bis parameters this end proposes.
    ///
    /// V.250 Table 27's `<max_dict>` and `<max_string>`, which a terminal sets
    /// "based on its knowledge of the nature of the data to be transmitted".
    pub fn offer_dictionary(&mut self, codewords: u16, max_string: u8) {
        self.limits = (codewords, max_string);
    }

    /// What to put in an XID: the standing proposal, capped by the terminal.
    fn proposal(&self) -> Xid {
        let mut xid = Xid::proposal(self.offer);
        xid.codewords = Some(self.limits.0.min(v42bis::OFFERED_N2));
        xid.max_string = Some(self.limits.1.min(v42bis::OFFERED_N7));
        if !self.offer_v42bis {
            xid.compression = Some(Compression::Neither);
        }
        if !self.offer_v44 {
            xid.v44 = None;
        } else if let (Some(offer), Some((transmit, receive))) = (xid.v44.as_mut(), self.v44_limits) {
            offer.transmit = offer.transmit.resolve(transmit);
            offer.receive = offer.receive.resolve(receive);
        }
        xid
    }

    /// Turn on V.42bis directly, bypassing the XID exchange.
    ///
    /// For tests and for a link whose parameters are known from elsewhere.
    /// Doing it at one end only does not fail cleanly: the far end would
    /// decompress data that was never compressed and deliver nonsense.
    pub fn with_compression(mut self, params: v42bis::Params) -> Self {
        self.enable_compression(params);
        self
    }

    fn enable_compression(&mut self, params: v42bis::Params) {
        self.compression = Some(Compressor {
            codec: Codec::V42bis(Box::new(Btlz {
                encoder: v42bis::Encoder::new(params),
                decoder: v42bis::Decoder::new(params),
                params,
            })),
            speculative: false,
        });
    }

    /// The same, for the newer one.
    fn enable_v44(&mut self, transmit: v44::Params, receive: v44::Params) {
        self.compression = Some(Compressor {
            codec: Codec::V44(Box::new(Lzjh {
                encoder: v44::Encoder::new(transmit),
                decoder: v44::Decoder::new(receive),
                transmit,
                receive,
            })),
            speculative: false,
        });
    }

    /// Which compression is running, if any.
    pub fn compression_name(&self) -> Option<&'static str> {
        self.compression.as_ref().map(Compressor::name)
    }

    /// Whether an unnegotiated switch to compressed data should be followed.
    ///
    /// Only where this end offered to receive it and the far end never
    /// answered. 6.4 negotiates V.42bis in XID and this far end sends none at
    /// all -- but it reads the offer, turns compression on, and says so the
    /// way 9.1 provides for. Refusing to decode what this end advertised it
    /// could accept leaves a link that is up and unreadable, which is what it
    /// did: a board's whole screen arrived as codewords and went to the
    /// terminal as codewords.
    ///
    /// Nothing is assumed until the far end says it. Before the announcement
    /// every octet goes through untouched, exactly as now.
    fn may_follow_ecm(&self) -> bool {
        !self.guessed_wrong
            && self.heard_xid.is_none()
            && matches!(
                self.offer,
                Compression::Both | Compression::ResponderToInitiator
            )
    }

    /// What the far end said in the detection phase.
    ///
    /// `None` where it said nothing at all, which is not the same as declining
    /// -- V.42 Table 3 has a pattern for declining and this is the absence of
    /// any pattern.
    pub fn far_answer(&self) -> Option<Answer> {
        self.heard_adp
    }

    /// What the far end proposed in XID, if it sent one.
    pub fn far_xid(&self) -> Option<Xid> {
        self.heard_xid
    }

    /// Whether the detection phase ended on the far end's terminal's text.
    pub fn far_text(&self) -> bool {
        self.heard_text
    }

    /// The line's bits that error control did not claim, once the connection
    /// has gone without it: what arrived during a detection phase that
    /// failed, and everything since. V.42 Appendix I.3 has the choice of
    /// throwing them away or giving them to the terminal, and they are very
    /// often exactly what the terminal is waiting for -- a login prompt sent
    /// the moment the far end connected.
    pub fn take_unclaimed(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.unclaimed)
    }

    /// Whether the question of error control has been answered.
    ///
    /// Three ways it can be: the far end declined or was not there, the link
    /// came up, or establishment was tried and got nothing back. Until one of
    /// them the answer is not known -- and V.250 6.5.5 has the DCE report what
    /// it negotiated "before the final result code", so this is the thing a
    /// CONNECT has to wait for.
    pub fn settled(&self) -> bool {
        match self.phase {
            Phase::Detecting | Phase::Negotiating => false,
            Phase::Transparent => true,
            Phase::Protocol => self.lapm.is_connected() || self.gave_up,
        }
    }

    /// The check sequence width the connection is using.
    ///
    /// 16 bits unless both ends offered 32 in XID and a SABME has since gone
    /// across at that width (V.42 8.10.2).
    pub fn fcs(&self) -> Fcs {
        self.encoder.fcs()
    }

    /// Bytes handed down and not yet framed for the line.
    pub fn queued(&self) -> usize {
        self.lapm.queued()
    }

    /// Whether compression was agreed and is running.
    pub fn compressing(&self) -> bool {
        self.compression.is_some()
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn state(&self) -> State {
        self.lapm.state()
    }

    pub fn is_connected(&self) -> bool {
        self.lapm.is_connected()
    }

    /// Frames that arrived damaged and were dropped.
    pub fn damaged_frames(&self) -> u64 {
        self.damaged
    }

    /// Compressed streams that would not decode.
    ///
    /// Not a line measurement: a damaged frame never reaches the decoder, so
    /// anything counted here arrived intact and still made no sense, which
    /// only happens when the two dictionaries have come apart.
    pub fn undecodable_streams(&self) -> u64 {
        self.undecodable
    }

    /// Take the frames that have crossed since this was last called.
    pub fn take_log(&mut self) -> Vec<Crossed> {
        std::mem::take(&mut self.log)
    }

    fn note(&mut self, outbound: bool, intact: bool, body: &[u8]) {
        if self.log.len() >= LOG_FRAMES {
            self.log.remove(0);
        }
        self.log.push(Crossed { outbound, intact, body: body.to_vec() });
    }

    /// Ask for the link to be established.
    ///
    /// Only meaningful once the detection phase has decided there is something
    /// to establish it with; before that it is remembered and acted on then.
    pub fn connect(&mut self) {
        if self.phase == Phase::Protocol {
            self.lapm.connect();
        }
    }

    /// Ask for it to be released.
    pub fn disconnect(&mut self) {
        self.lapm.disconnect();
    }

    /// Time passing, which is what drives every timer here.
    pub fn tick(&mut self, dt_ms: u32) {
        match self.phase {
            Phase::Detecting => {
                let outcome = match &mut self.detect {
                    Detect::Origin(o) => o.tick(dt_ms),
                    Detect::Answer(a) => a.tick(dt_ms),
                    Detect::Done => Outcome::Pending,
                };
                self.settle_detection(outcome);
            }
            Phase::Negotiating => {
                self.xid_ms = self.xid_ms.saturating_add(dt_ms);
                // T401 is LAPM's own, because it is the same line and the same
                // round trip, and it is the one the data pump's measurement
                // went into. It is also the whole of the wait while a
                // retransmission is still to come: an XID command that is out
                // with the timer running has not gone unanswered yet, so there
                // is nothing else to give up on.
                //
                // Not while one is due and unsent. The timer 8.10.2 starts is
                // started by transmitting the frame, and the encoder may still
                // be laying out the last one; counting a retransmission before
                // the copy it belongs to has left would spend N400 on frames
                // the far end never saw.
                if !self.xid_due {
                    if self.xid_retries < XID_N400 {
                        if self.xid_ms >= self.lapm.t401_ms() {
                            // 8.10.3: "retransmit the XID command as above;
                            // restart timer T401; and increment the
                            // retransmission counter (N400)". The restart is
                            // where the frame goes out.
                            self.xid_retries += 1;
                            self.xid_due = true;
                        }
                    } else if self.xid_ms >= self.xid_last_wait_ms() {
                        // 8.10.3's other ending: "after retransmission of the
                        // XID command N400 times and failure to receive an XID
                        // response ... notify the control function that the
                        // negotiation/indication procedure did not complete".
                        // Here that notification is simply going on without
                        // anything negotiated -- a modem that does error
                        // control but declines to negotiate is a modem to talk
                        // to without compression, not one to wait for
                        // indefinitely.
                        //
                        // Sooner than the T401 8.10.3 restarts: see
                        // [`XID_LAST_WAIT_MS`].
                        self.begin_protocol();
                    }
                }
            }
            Phase::Protocol | Phase::Transparent => {}
        }
        self.lapm.tick(dt_ms);
        self.drain();
    }

    /// Queue data from the terminal.
    pub fn send(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        match &mut self.compression {
            // Decoding only. Nothing has agreed that the far end would read
            // compressed data from this end, and it is reading what is sent
            // now.
            Some(c) if c.speculative => self.lapm.send_data(data),
            Some(c) => {
                let out = c.encode(data);
                self.lapm.send_data(&out);
            }
            None => self.lapm.send_data(data),
        }
    }

    /// Data that has arrived, decompressed and in order.
    pub fn take_received(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.delivered)
    }

    /// One bit for the line.
    ///
    /// Never nothing: a synchronous link always carries something, and when
    /// there is nothing to say it carries flags.
    pub fn next_bit(&mut self) -> bool {
        // The detection phase is not framed and does not go through the
        // encoder: it is async characters laid straight onto the synchronous
        // stream, which is the point of it. Nothing that is not looking for
        // them could mistake them for a frame.
        if self.phase == Phase::Detecting {
            return match &mut self.detect {
                Detect::Origin(o) => o.transmit(),
                Detect::Answer(a) => a.transmit(),
                Detect::Done => true,
            };
        }
        if self.phase == Phase::Transparent {
            return true;
        }
        if self.encoder.is_empty() {
            let mut queued = false;
            if !self.opened {
                self.opened = true;
                self.encoder.idle(LEADING_FLAGS);
                return self.encoder.next_bit().unwrap_or(true);
            }
            if self.phase == Phase::Negotiating && self.xid_due {
                // One of them, and then only when T401 says so. 8.10.2 has the
                // entity "transmit an XID command frame", singular, and then
                // start T401 and reset N400; 8.10.3 is the only thing that
                // sends another.
                //
                // What used to send a fresh copy every time the encoder ran
                // dry was the window in which the far end is still in the
                // detection phase, where every bit it is handed goes to its
                // detector rather than its deframer, so an XID is simply
                // consumed. The window is shut by the flags above and not by
                // repetition: 7.2.1.3 starts the answerer's protocol phase on
                // "receipt of continuous flags, or of an LAPM ... protocol
                // frame", and LEADING_FLAGS is the run 8.10.2's Note asks for.
                // An answerer that was still transmitting its own pattern when
                // they went past is covered by the retransmission, which is
                // what a retransmission is for.
                //
                // P is 0 on a command as on a response: 8.2.4.13 says "the
                // P/F bit of an XID frame is set to 0" and names no exception.
                self.xid_due = false;
                self.xid_ms = 0;
                let body = Frame::Xid {
                    pf: false,
                    info: self.proposal().encode(Kind::Command),
                }
                .encode(DLCI_DATA, self.role, Kind::Command);
                self.encoder.frame_with(&body, Fcs::Bits16);
                self.note(true, true, &body);
                queued = true;
            }
            while let Some((frame, kind)) = self.lapm.poll_transmit() {
                let body = frame.encode(DLCI_DATA, self.role, kind);
                self.encoder.frame(&body);
                self.note(true, true, &body);
                queued = true;
            }
            if !queued {
                self.encoder.idle(1);
            }
        }
        self.encoder.next_bit().unwrap_or(true)
    }

    /// One bit from the line.
    pub fn feed_bit(&mut self, bit: bool) {
        if self.phase == Phase::Detecting {
            let outcome = match &mut self.detect {
                Detect::Origin(o) => o.receive(bit),
                Detect::Answer(a) => a.receive(bit),
                Detect::Done => Outcome::Pending,
            };
            self.settle_detection(outcome);
            return;
        }
        if self.phase == Phase::Transparent {
            self.unclaimed.push(bit);
            return;
        }
        let Some(result) = self.decoder.feed(bit) else {
            return;
        };
        let Ok(body) = result else {
            let discarded = self.decoder.discarded().to_vec();
            self.note(false, false, &discarded);
            // A frame that did not survive the line is dropped and left to the
            // retransmission machinery, which is what it is for. Counting them
            // is worth doing: it is the difference between a link that is
            // working and one that is only apparently working.
            self.damaged += 1;
            return;
        };
        self.note(false, true, &body);
        let Ok((address, frame)) = Frame::decode(&body, self.role) else {
            self.damaged += 1;
            return;
        };
        self.dispatch(address, frame);
    }

    fn dispatch(&mut self, address: Address, frame: Frame) {
        // XID is handled here rather than by LAPM, because what it negotiates
        // is not LAPM's: the compression sits above it and the framing below.
        if let Frame::Xid { info, .. } = &frame {
            self.receive_xid(info.clone(), address.kind);
            return;
        }
        // The set-mode command settles the width for good, in whichever
        // direction it was travelling: "receipt of a SABME frame with 16- or
        // 32-bit FCS indicates use of the corresponding FCS for all subsequent
        // frames", and the answer to one says the same thing back.
        //
        // Not an I or supervisory frame that LAPM takes in place of a lost UA
        // (8.3.2.1): the far end may have sent it before the SABME reached
        // it, at the width it was using then. Where 32 bits were agreed the
        // decoder goes on reading both until a UA or SABME settles it.
        if matches!(frame, Frame::Sabme { .. } | Frame::Ua { .. }) {
            let width = self.decoder.matched_fcs();
            self.decoder.set_fcs(width);
            self.encoder.set_fcs(width);
        }
        self.lapm.receive(frame, address.kind);
        self.drain();
    }

    fn receive_xid(&mut self, info: Vec<u8>, kind: Kind) {
        let Ok(theirs) = Xid::decode(&info) else {
            self.damaged += 1;
            return;
        };
        self.heard_xid = Some(theirs);
        let agreed = self.proposal().resolve(&theirs);
        // V.44 first where both were offered and both ends know it, and
        // V.42bis otherwise. 7.3 has the responder answer about at most one of
        // the two, so a far end that named neither leaves this alone and the
        // call carries on uncompressed.
        if let Some(offer) = agreed.v44 {
            self.enable_v44(offer.transmit, offer.receive);
        } else if let Some(params) = agreed.v42bis_params() {
            self.enable_compression(params);
        }
        if agreed.fcs32 {
            self.agreed_fcs = Fcs::Bits32;
            // Late: this end stopped waiting and went on to the protocol
            // before the far end's XID arrived. The reply below still tells
            // it 32 bits were agreed, so its SABME may come at 32, and a
            // decoder still reading 16 turns every one of them into damage --
            // which is what an answerer did on a line longer than the XID
            // wait, and the originator gave up on error control after three.
            // 8.10.2 settles the width on the SABME, so until one has arrived
            // either is right.
            if self.phase == Phase::Protocol && !self.lapm.is_connected() {
                self.decoder.accept_either();
            }
        }
        // 8.4.5.1: only after both ends have said so. An end that did not
        // agree treats an SREJ as an unrecognized control field, which under
        // 8.5.5 ends the connection -- so this is a capability to use when it
        // has been granted rather than one to try.
        self.lapm.set_selective_reject(agreed.srej_single);
        // Answer every command and no responses. 8.10.2: "on receipt of an
        // L-SETPARM response primitive ... an error control function shall
        // return the indicated parameter values/procedure settings in the
        // information field of an XID response frame", and "receipt of another
        // XID command frame ... shall be responded to". Both ends send a
        // command here, so both end up replying, and neither replies to a
        // reply -- which is what would go round for ever.
        //
        // Answering *every* command matters: 8.10.3 has a far end that heard
        // no response retransmit its XID up to N400 times, and an end that
        // answered only the first of them leaves it retransmitting into
        // silence until it gives up on compression or, following Appendix
        // III.3, on the call.
        if kind == Kind::Command {
            // What goes back is this end's proposal with one compression
            // algorithm in it, the one `agreed` came to -- V.44 7.3's NOTE,
            // and the same choice made a few lines above.
            let body = Frame::Xid {
                pf: false,
                info: self.proposal().answering(&agreed).encode(Kind::Response),
            }
            .encode(DLCI_DATA, self.role, Kind::Response);
            // 8.10.2 keeps this one at 16 bits whatever the connection has
            // moved to, because the command it answers was sent at 16.
            self.encoder.frame_with(&body, Fcs::Bits16);
            self.note(true, true, &body);
        }
        self.begin_protocol();
    }

    /// Act on how the detection phase came out (7.2.1.2, 7.2.1.3).
    fn settle_detection(&mut self, outcome: Outcome) {
        if let Outcome::Answered(a) = outcome {
            self.heard_adp = Some(a);
        }
        match outcome {
            Outcome::Pending => {}
            // The far end's terminal is already talking, whatever V.8 said:
            // there is no V.42 to wait out a timer for, and what it said goes
            // to this end's terminal.
            Outcome::Text => {
                self.heard_text = true;
                self.detect_failed();
            }
            Outcome::Answered(a) if a.error_controlled() => {
                if matches!(&self.detect, Detect::Answer(a) if !a.finished_sending()) {
                    return;
                }
                self.enter_negotiating();
            }
            // The far end is already talking protocol, so there is nothing
            // left to detect and nothing to answer.
            Outcome::ProtocolStarted => {
                self.enter_negotiating();
                self.lapm.set_retransmissions(crate::lapm::UNCONFIRMED_N400);
            }
            // Said its piece and meant it. Going on to XID after sending
            // Table 3's refusal would be talking protocol at an end that has
            // just been told there would not be one.
            Outcome::OriginatorDetected if self.declining => {
                if matches!(&self.detect, Detect::Answer(a) if !a.finished_sending()) {
                    return;
                }
                self.detect_failed();
            }
            Outcome::OriginatorDetected => {
                // The answerer has to finish saying what it is saying: cutting
                // its own reply short would leave the originator waiting for
                // the rest of it.
                if matches!(&self.detect, Detect::Answer(a) if !a.finished_sending()) {
                    return;
                }
                self.enter_negotiating();
            }
            Outcome::TimedOut if self.declared => {
                // Nothing came back, but the far end has already said it does
                // LAPM. A detection phase that heard nothing has not
                // contradicted that -- an ADP is ten patterns of async
                // characters on a line that has just been trained, and losing
                // all of them is what a bad line does.
                //
                // Patience is cut right back, though. Appendix III.2 asks for
                // a small N400 wherever detection has not confirmed the far
                // end, so that a modem which turns out not to be listening is
                // fallen back from quickly rather than talked at for a minute.
                self.enter_negotiating();
                self.lapm.set_retransmissions(crate::lapm::UNCONFIRMED_N400);
            }
            Outcome::Answered(_) | Outcome::TimedOut => {
                // No error control at the far end, or nothing there that
                // recognised the question. Either way there is nothing to
                // establish, and the connection carries on without it.
                self.detect_failed();
            }
        }
    }

    /// The detection phase is over and the XID exchange begins (8.10.2).
    ///
    /// One XID command is queued behind the opening flags, and the timer and
    /// the counter that decide whether it is ever sent again both start from
    /// nothing.
    fn enter_negotiating(&mut self) {
        self.detect = Detect::Done;
        self.phase = Phase::Negotiating;
        self.xid_due = true;
        self.xid_ms = 0;
        self.xid_retries = 0;
    }

    /// No error control: the connection goes on without it, and what the
    /// detection phase heard is kept for the terminal.
    fn detect_failed(&mut self) {
        let heard = match &mut self.detect {
            Detect::Origin(o) => o.take_heard(),
            Detect::Answer(a) => a.take_heard(),
            Detect::Done => Vec::new(),
        };
        self.unclaimed.extend(heard);
        self.detect = Detect::Done;
        self.phase = Phase::Transparent;
    }

    fn begin_protocol(&mut self) {
        if self.phase == Phase::Protocol {
            return;
        }
        self.phase = Phase::Protocol;
        if self.agreed_fcs == Fcs::Bits32 {
            // 8.10.2: the width changes over on the SABME and not before, so
            // until one has been seen neither end can be sure which it is
            // reading -- the answerer because the SABME has not arrived, and
            // the originator because a far end that agreed to 32 and then
            // answered at 16 is a far end to go on talking to rather than one
            // to stop hearing.
            self.decoder.accept_either();
            if self.role == Role::Originator {
                self.encoder.set_fcs(Fcs::Bits32);
            }
        }
        if self.role == Role::Originator {
            self.lapm.connect();
        }
    }

    fn drain(&mut self) {
        let mut arrived: Vec<u8> = Vec::new();
        while let Some(event) = self.lapm.poll_event() {
            match event {
                Event::Data(d) => arrived.extend_from_slice(&d),
                // 5.6 a): a C-INIT on "L-ESTABLISH indication or confirm",
                // which is this. The first one costs nothing -- no data has
                // been through the codec yet -- and every one after it is a
                // link that came back, where it is the whole difference
                // between a connection that carries something and one that
                // stays up carrying nonsense.
                Event::Connected => {
                    self.established = true;
                    if let Some(compression) = self.compression.as_mut() {
                        compression.reinitialize();
                    }
                }
                // The far end re-established under us: 8.2.4.3 discards what
                // was unacknowledged, so the stream both dictionaries were
                // built from has a hole in it and they have to start again.
                Event::Reset => {
                    if let Some(compression) = self.compression.as_mut() {
                        compression.reinitialize();
                    }
                }
                // N400 attempts at a SABME that nothing answered, or a far end
                // that refused. Whatever it said earlier, it is not doing LAPM
                // now -- and a connection without error control is still a
                // connection, which is the whole reason V.42 7.2.1 exists. The
                // line reverts to start-stop characters, which is what a far
                // end that never answered a SABME was expecting all along.
                Event::Released(Cause::NoResponse | Cause::Refused)
                    if !self.established =>
                {
                    self.gave_up = true;
                    self.phase = Phase::Transparent;
                }
                _ => {}
            }
        }
        if arrived.is_empty() {
            return;
        }
        // A decoder for a far end that may switch without having asked.
        //
        // Built here, on the first octet of the connection, and not where the
        // switch appears -- which is where it was first put, and it does not
        // work there. 7.4 has the encoder adding strings to its dictionary
        // while it is still in transparent mode, so a decoder that starts at
        // the escape has an empty dictionary against a full one and disagrees
        // from the first codeword. It has to see everything the far end sent.
        //
        // Costing nothing until then: a V.42bis decoder in transparent mode
        // hands back what it is given. What it also does is read an escape
        // character as an escape character, so a far end that never intended
        // any of this and sends a literal zero will be misread -- and that is
        // the price. It is bounded: the decode fails, the guess is dropped for
        // the rest of the call, and the octets go through raw again.
        if self.compression.is_none() && self.may_follow_ecm() {
            // The far end never said what parameters it was using, so this is
            // the only figure there is: what this end offered, which is what
            // it read before deciding to compress at all.
            let proposed = self.proposal();
            self.enable_compression(v42bis::Params {
                n2: proposed.codewords.unwrap_or(v42bis::OFFERED_N2),
                n7: proposed.max_string.unwrap_or(v42bis::OFFERED_N7),
            });
            if let Some(c) = self.compression.as_mut() {
                c.speculative = true;
            }
        }
        match &mut self.compression {
            Some(c) => {
                let speculative = c.speculative;
                if !c.decode(&arrived, &mut self.delivered) {
                    if speculative {
                        // A guess that did not come off. Nothing negotiated
                        // this, so nothing is owed to it: put the stream back
                        // the way it was and stop guessing for the rest of the
                        // call. Dropping a working link over a guess would be
                        // worse than the garbled screen it was meant to fix.
                        self.compression = None;
                        self.guessed_wrong = true;
                        self.delivered.extend_from_slice(&arrived);
                        return;
                    }
                    // A compressed stream that will not decode cannot be
                    // recovered from by asking again: the dictionary at each
                    // end is built from everything that came before, so once
                    // they disagree they stay disagreed. V.42bis 6.4 has the
                    // receiver ask for the link to be reset.
                    self.undecodable += 1;
                    self.lapm.disconnect();
                }
            }
            None => self.delivered.extend_from_slice(&arrived),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Run two ends against each other until neither has anything to say.
    ///
    /// `channel` is given each bit and returns what arrives, which is where a
    /// test puts errors.
    fn settle(a: &mut Stack, b: &mut Stack, bits: usize, mut channel: impl FnMut(usize, bool) -> bool) {
        for i in 0..bits {
            let to_b = a.next_bit();
            let to_a = b.next_bit();
            b.feed_bit(channel(i, to_b));
            a.feed_bit(channel(i, to_a));
            if i % 160 == 0 {
                // A bit at 9600 is about a tenth of a millisecond, so this is
                // roughly real time for the retransmission timers.
                a.tick(16);
                b.tick(16);
            }
        }
    }

    fn pair() -> (Stack, Stack) {
        (
            Stack::new(Role::Originator, Params::default()),
            Stack::new(Role::Answerer, Params::default()),
        )
    }

    #[test]
    fn a_link_establishes_and_carries_data() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(a.is_connected(), "the originator is {:?}", a.state());
        assert!(b.is_connected(), "the answerer is {:?}", b.state());

        a.send(b"login: cactus\r\n");
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"login: cactus\r\n");
    }

    #[test]
    fn an_idle_link_carries_flags_rather_than_nothing() {
        // A synchronous connection has to keep something on the line: the far
        // end's framing stays synchronised on it, and its absence is how a
        // modem tells that the connection has gone.
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        let idle: Vec<bool> = (0..80).map(|_| a.next_bit()).collect();
        // Where in a flag the stream was caught is arbitrary, so look for the
        // alignment at which it reads as flags rather than assuming one.
        let aligned = (0..8).any(|offset| {
            idle[offset..offset + 64]
                .chunks(8)
                .all(|c| c.iter().rev().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)) == 0x7e)
        });
        assert!(
            aligned,
            "an idle link carried {:?} rather than flags",
            idle.iter().map(|&b| u8::from(b)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn data_crosses_in_both_directions_at_once() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        a.send(b"what the caller typed");
        b.send(b"what the host answered");
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"what the caller typed");
        assert_eq!(a.take_received(), b"what the host answered");
    }

    #[test]
    fn a_line_that_damages_frames_still_delivers() {
        // The point of error control. Every so often a bit is flipped, which
        // fails a check sequence and loses a whole frame; what arrives at the
        // far end must still be exactly what was sent.
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);

        let payload: Vec<u8> = (0..400).map(|i| (i % 251) as u8).collect();
        a.send(&payload);
        settle(&mut a, &mut b, 400_000, |i, bit| {
            if i % 4099 == 0 { !bit } else { bit }
        });
        assert_eq!(b.take_received(), payload);
        assert!(
            a.damaged_frames() + b.damaged_frames() > 0,
            "the channel did not actually damage anything, so nothing was tested"
        );
    }

    #[test]
    fn the_detection_phase_runs_before_anything_else() {
        // V.42 7.2.1: before a protocol can be established the two ends have to
        // find out whether there is anything to establish it with. Neither is
        // told; the originator sends a pattern only a V.42 modem recognises,
        // and the answerer's reply says whether it did.
        let (mut a, mut b) = pair();
        assert_eq!(a.phase(), Phase::Detecting);
        assert_eq!(b.phase(), Phase::Detecting);
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert_eq!(a.phase(), Phase::Protocol, "the originator got stuck");
        assert_eq!(b.phase(), Phase::Protocol, "the answerer got stuck");
        assert!(a.is_connected() && b.is_connected());
    }

    #[test]
    fn a_far_end_without_error_control_is_recognised_rather_than_waited_for() {
        // 7.2.1.2: the originator's detection times out, and a modem that
        // treated that as a failure would drop a connection that was working
        // perfectly well without protection.
        let mut a = Stack::new(Role::Originator, Params::default());
        a.connect();
        // Something on the line that is not a V.42 modem answering: a
        // continuous mark, which is what an idle asynchronous line carries.
        for i in 0..40_000 {
            a.next_bit();
            a.feed_bit(true);
            if i % 160 == 0 {
                a.tick(16);
            }
        }
        assert_eq!(a.phase(), Phase::Transparent);
        assert!(!a.is_connected(), "established a link with nothing");
    }

    #[test]
    fn an_answerer_that_declines_is_taken_at_its_word() {
        // V.42 Table 3 gives the answerer a way to say no, and a modem that
        // ignored it would frame data the far end reads as characters.
        let mut a = Stack::new(Role::Originator, Params::default());
        let mut b = Stack::new(Role::Answerer, Params::default()).declining();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(a.phase(), Phase::Transparent, "the refusal was not heard");
        assert!(!a.is_connected());
    }

    #[test]
    fn compression_is_used_only_when_both_ends_offer_it() {
        // V.42bis is negotiated in XID, and the result is the intersection of
        // what the two ends asked for. Compression running at one end only
        // does not degrade: the far end decompresses data that was never
        // compressed and delivers nonsense.
        let (mut a, mut b) = pair();
        a.offer_compression(Compression::Both);
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected());
        assert!(
            !a.compressing() && !b.compressing(),
            "one-sided compression was agreed to"
        );

        let (mut a, mut b) = pair();
        a.offer_compression(Compression::Both);
        b.offer_compression(Compression::Both);
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert!(
            a.compressing() && b.compressing(),
            "both ends offered compression and only {} got it",
            if a.compressing() { "the originator" } else { "the answerer" }
        );

        // And it works: what goes in comes out.
        let payload = b"the same words over and over, the same words over and over".to_vec();
        a.send(&payload);
        settle(&mut a, &mut b, 80_000, |_, bit| bit);
        assert_eq!(b.take_received(), payload);
    }

    #[test]
    fn compression_is_carried_through_the_same_interface() {
        let params = v42bis::Params::default();
        let mut a = Stack::new(Role::Originator, Params::default()).with_compression(params);
        let mut b = Stack::new(Role::Answerer, Params::default()).with_compression(params);
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);

        // Something with the repetition a dictionary coder exists for.
        let payload = b"the same words over and over, the same words over and over, \
                        the same words over and over, the same words over and over"
            .to_vec();
        a.send(&payload);
        settle(&mut a, &mut b, 80_000, |_, bit| bit);
        assert_eq!(b.take_received(), payload);
    }

    /// V.42bis 5.6 a): the dictionaries start again whenever the link is
    /// established, and a link that re-establishes mid-call has done that.
    ///
    /// The dictionary is built out of the data that has gone past, and the two
    /// ends only agree because they have seen the same data. Re-establishment
    /// discards whatever was unacknowledged (V.42 8.2.4.3), taking a piece out
    /// of the middle of that stream. Unless both ends start again, every
    /// codeword after it means something else, and the connection stays up
    /// carrying nonsense for the rest of the call -- which is what a real one
    /// did: error control re-established fourteen seconds in and nothing
    /// crossed afterwards, on a link whose every other number looked healthy.
    #[test]
    fn compression_starts_again_when_the_link_does() {
        let params = v42bis::Params::default();
        let mut a = Stack::new(Role::Originator, Params::default()).with_compression(params);
        let mut b = Stack::new(Role::Answerer, Params::default()).with_compression(params);
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected());

        let block = |from: usize| {
            let mut out = Vec::new();
            for i in from..from + 200 {
                out.extend_from_slice(
                    format!("the same words over and over, line {i}\r\n").as_bytes(),
                );
            }
            out
        };

        // Enough to put compression properly to work, so both dictionaries
        // are full of the same strings.
        let warmed = block(0);
        a.send(&warmed);
        settle(&mut a, &mut b, 200_000, |_, bit| bit);
        assert!(a.compressing(), "compression never engaged, so nothing could diverge");
        assert_eq!(b.take_received().len(), warmed.len());

        // Then a second lot, cut off partway. What is still unacknowledged
        // when the link goes is discarded, so the far decoder never sees the
        // data the encoder learned its newest strings from.
        let lost = block(1_000);
        a.send(&lost);
        settle(&mut a, &mut b, 4_000, |_, bit| bit);
        let crossed = b.take_received().len();
        assert!(crossed < lost.len(), "all {crossed} of it crossed before the link went");

        // The far end re-establishes underneath it, which is what a line that
        // went away and came back does.
        b.connect();
        settle(&mut a, &mut b, 60_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected(), "it did not come back");
        // What survived the discard is not the point and is not checked: V.42
        // 8.2.4.3 throws unacknowledged frames away and whatever is above asks
        // for them again. What has to be true is that everything from here on
        // means what it says.
        let _ = a.take_received();
        let _ = b.take_received();

        let after = block(2_000);
        a.send(&after);
        settle(&mut a, &mut b, 200_000, |_, bit| bit);
        assert_eq!(b.take_received(), after, "what arrived was not what was sent");
        b.send(&after);
        settle(&mut a, &mut b, 200_000, |_, bit| bit);
        assert_eq!(a.take_received(), after, "and not the other way either");
    }

    /// A negotiated pair, the way a call makes one: XID settles compression
    /// and everything else that goes with it, rather than a test reaching in.
    fn negotiated_pair() -> (Stack, Stack) {
        let mut a = Stack::new(Role::Originator, Params::default());
        let mut b = Stack::new(Role::Answerer, Params::default());
        a.offer_compression(Compression::Both);
        b.offer_compression(Compression::Both);
        (a, b)
    }

    /// Bulk traffic both ways at once on a line that damages frames.
    ///
    /// The condition a real call is in for most of its life and no other test
    /// here puts it in: both ends compressing, both ends with data outstanding,
    /// and retransmission running underneath. Compression cannot tolerate a
    /// single octet delivered twice, out of order, or not at all, so this is
    /// where error control either holds the stream together or quietly stops.
    #[test]
    fn bulk_traffic_both_ways_survives_a_line_that_damages_frames() {
        let (mut a, mut b) = negotiated_pair();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected(), "the link never came up");
        assert!(a.compressing() && b.compressing(), "compression was not negotiated");

        // Web traffic, near enough: headers that repeat and a body that does
        // not, so the dictionary is being added to throughout.
        let stream = |tag: u8, n: usize| -> Vec<u8> {
            let mut out = Vec::new();
            let mut x: u32 = 0x2545_f491 ^ u32::from(tag);
            while out.len() < n {
                out.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n");
                for _ in 0..64 {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    out.push((x & 0xff) as u8);
                }
            }
            out.truncate(n);
            out
        };

        let up = stream(1, 6_000);
        let down = stream(2, 6_000);
        a.send(&up);
        b.send(&down);
        settle(&mut a, &mut b, 2_000_000, |i, bit| {
            if i % 8191 == 0 { !bit } else { bit }
        });
        assert!(
            a.damaged_frames() + b.damaged_frames() > 0,
            "the channel damaged nothing, so nothing was tested"
        );
        assert!(a.is_connected() && b.is_connected(), "the link did not survive");
        assert_eq!(b.take_received(), up, "what reached the answerer was not what was sent");
        assert_eq!(a.take_received(), down, "and not the other way either");
    }

    /// Two ends that both know V.44 use it, and V.42bis is what is left for
    /// an end that does not.
    ///
    /// V.44 7.3: "the responder shall include parameters for at most one
    /// compression algorithm (V.42 bis or V.44) in the response XID", so the
    /// choice is made once, in the negotiation, and the call runs one of them.
    #[test]
    fn two_ends_that_know_v44_use_it_and_fall_back_when_one_does_not() {
        let (mut a, mut b) = negotiated_pair();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(a.compression_name(), Some("V.44"));
        assert_eq!(b.compression_name(), Some("V.44"));

        // The same text through each, to see that the choice is worth making.
        let text: Vec<u8> = b"the same line of text over and over and over
"
            .iter()
            .copied()
            .cycle()
            .take(24_000)
            .collect();
        a.send(&text);
        settle(&mut a, &mut b, 3_000_000, |_, bit| bit);
        assert_eq!(b.take_received(), text, "V.44 did not carry it");

        // A far end that never heard of V.44 answers about V.42bis instead,
        // and nothing is lost but the newer algorithm.
        let mut a = Stack::new(Role::Originator, Params::default());
        let mut b = Stack::new(Role::Answerer, Params::default());
        a.offer_compression(Compression::Both);
        b.offer_compression(Compression::Both);
        b.without_v44();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(a.compression_name(), Some("V.42bis"));
        assert_eq!(b.compression_name(), Some("V.42bis"));
        a.send(b"and this still crosses");
        settle(&mut a, &mut b, 200_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"and this still crosses");
    }

    /// V.44 7.3, NOTE: "The responder shall include parameters for at most one
    /// compression algorithm (V.42 bis or V.44) in the response XID."
    ///
    /// This end answered every command with its whole standing proposal, which
    /// names both, and that is not an answer: it says what this end can do
    /// rather than what the call is going to use. A far end reading the
    /// V.42bis half of it and turning V.42bis on would then be compressing
    /// with one algorithm against a decoder running the other, which does not
    /// fail cleanly -- every codeword means something else and the screen
    /// fills with nonsense on a link that looks perfect.
    #[test]
    fn an_xid_response_names_one_compression_algorithm() {
        /// Every XID an end sent as a response, read as the far end reads it.
        fn answers(stack: &mut Stack, role: Role) -> Vec<Xid> {
            stack
                .take_log()
                .into_iter()
                .filter(|f| f.outbound)
                .filter_map(|f| Frame::decode(&f.body, role.peer()).ok())
                .filter_map(|(address, frame)| match frame {
                    Frame::Xid { info, .. } if address.kind == Kind::Response => {
                        Some(Xid::decode(&info).expect("this end's own XID would not decode"))
                    }
                    _ => None,
                })
                .collect()
        }

        // Both ends offering both algorithms: V.44 is what they settle on, so
        // V.44 is the one their answers name.
        let (mut a, mut b) = negotiated_pair();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(a.compression_name(), Some("V.44"), "nothing was settled to answer about");
        let mut both = answers(&mut a, Role::Originator);
        both.extend(answers(&mut b, Role::Answerer));
        assert!(!both.is_empty(), "neither end answered an XID command");
        for xid in both {
            assert!(xid.v44.is_some(), "an answer dropped the agreed V.44");
            assert!(xid.compression.is_none(), "an answer named V.42bis as well as V.44");
        }

        // And an end that has never heard of V.44 is answered about the
        // algorithm it did offer, which is the whole of the choice.
        let mut a = Stack::new(Role::Originator, Params::default());
        let mut b = Stack::new(Role::Answerer, Params::default());
        a.offer_compression(Compression::Both);
        b.offer_compression(Compression::Both);
        b.without_v44();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(a.compression_name(), Some("V.42bis"), "nothing was settled to answer about");
        let answered = answers(&mut a, Role::Originator);
        assert!(!answered.is_empty(), "the originator answered no XID command");
        for xid in answered {
            assert!(xid.compression.is_some(), "the originator's answer dropped V.42bis");
            assert!(xid.v44.is_none(), "the originator offered V.44 to an end without it");
        }
    }

    #[test]
    fn v44_alone_is_offered_and_used_with_a_far_end_that_has_both() {
        let (mut a, mut b) = negotiated_pair();
        a.without_v42bis();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(a.compression_name(), Some("V.44"));
        assert_eq!(b.compression_name(), Some("V.44"));
        a.send(b"through V.44 alone");
        settle(&mut a, &mut b, 200_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"through V.44 alone");
    }

    #[test]
    fn v44_alone_meets_v42bis_alone_and_nothing_is_compressed() {
        // Each end asked for one and not the other; there is nothing both
        // will run, and the link carries on uncompressed rather than failing.
        let (mut a, mut b) = negotiated_pair();
        a.without_v42bis();
        b.without_v44();
        a.connect();
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected());
        assert_eq!(a.compression_name(), None);
        assert_eq!(b.compression_name(), None);
        a.send(b"plain");
        settle(&mut a, &mut b, 200_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"plain");
    }

    #[test]
    fn the_terminal_s_v44_ceilings_go_into_the_offer() {
        let (mut a, _) = negotiated_pair();
        a.offer_v44_limits(v44::Params { n2: 512, n7: 40, n8: 1024 }, v44::Params { n2: 1024, n7: 255, n8: 65535 });
        let offer = a.proposal().v44.expect("V.44 not offered");
        assert_eq!(offer.transmit, v44::Params { n2: 512, n7: 40, n8: 1024 });
        // A ceiling above what the coder offers leaves the coder's offer.
        assert_eq!(offer.receive, v44::Params::of(v44::OFFERED_N2, v44::OFFERED_N7).resolve(v44::Params { n2: 1024, n7: 255, n8: 65535 }));
        assert_eq!(offer.receive.n2, 1024);
    }

    /// V.42 8.2.4.13: "The P/F bit of an XID frame is set to 0."
    ///
    /// Commands too. This end polled with every XID it sent, and the far ends
    /// it called on real lines answered none of them; the calling modem in
    /// the V.22bis vector sends its XID command with the bit clear.
    #[test]
    fn an_xid_command_goes_out_with_its_poll_bit_clear() {
        for role in [Role::Originator, Role::Answerer] {
            let mut stack = Stack::new(role, Params::default()).without_detection();
            let mut decoder = Decoder::new(Fcs::Bits16);
            let mut control = None;
            for _ in 0..4_000 {
                if let Some(Ok(body)) = decoder.feed(stack.next_bit()) {
                    control = Some(body[1]);
                    break;
                }
            }
            let control = control.expect("no frame went out");
            assert_eq!(control & !0x10, 0xaf, "the first frame was not an XID");
            assert_eq!(control, 0xaf, "the {role:?}'s XID has P set");
        }
    }

    /// V.42 8.10.2 sends one XID command and starts T401; 8.10.3 is the only
    /// thing that sends another, and it sends it N400 times. Then the exchange
    /// stops, and the whole of it is time the terminal spends waiting.
    ///
    /// On the T401 figures a call is really built with, and on the path a call
    /// really takes: a stack the way `Modem` assembles one, with the detection
    /// phase running, against a far end that answers it with `EC` (7.2.1.3)
    /// and then never answers the XID. A test that asked the same question
    /// `.without_detection()` -- the path only a terminal that asked for it
    /// takes -- had the first XID leave at 20 ms on every line, which is not a
    /// call's figure for any of them.
    ///
    /// This end first queued a fresh XID every time the encoder ran dry --
    /// between 50 and 206 identical copies on a real call. The timer that
    /// replaced it was then raced by a separate wait for the response that
    /// expired first on nearly every line, so a call sent one XID and never
    /// asked again; and one XID is exactly the frame an answerer still
    /// finishing its detection pattern swallows whole. Then the retransmission
    /// that fixed *that* was followed by a second full T401 of dead time,
    /// twelve seconds of it on a long line, which is what the budget below is
    /// written out to stop coming back.
    #[test]
    fn the_xid_exchange_costs_a_call_t401_and_a_round_trip_and_no_more() {
        // The tick is the granularity of every time below: a frame is seen to
        // have gone out at the tick after its closing flag, so a gap measured
        // here can be a tick shorter than the timer that opened it, and the
        // frame's own 11 ms on the line shorter again.
        const RATE: u32 = 28_800;
        const TICK_MS: u32 = 10;
        const BITS_PER_TICK: usize = (RATE * TICK_MS / 1000) as usize;
        const SLACK_MS: u32 = 2 * TICK_MS;

        // What builds a stack: no measurement at all, which is every V.22bis,
        // V.32 and V.32bis call and a V.34 one whose phase 2 measured nothing,
        // and then three lines the data pump has put a figure on. Against each
        // one, what the whole exchange is allowed to cost from the first XID
        // to the give-up -- T401 and then a round trip -- written out rather
        // than computed, so that a budget which grows has to be typed here.
        for (round_trip_ms, budget_ms) in
            [(None, 2038), (Some(200), 1238), (Some(1142), 2893), (Some(4_000), 10_000)]
        {
            let t401_ms = match round_trip_ms {
                Some(ms) => crate::lapm::t401_for_line(RATE, ms),
                None => crate::lapm::t401_for(RATE),
            };
            let params = Params { t401_ms, ..Default::default() };
            let mut stack = Stack::new(Role::Originator, params);
            if let Some(ms) = round_trip_ms {
                stack = stack.over_a_round_trip(ms);
            }
            // The far end: 7.2.1.3's answerer, which hears the ODP and answers
            // `EC`, and has nothing at all to say about XID. A far end that
            // does error control and does not negotiate is what 8.10.3's other
            // ending is for.
            let t400_ms = crate::detect::DEFAULT_T400_MS.saturating_add(round_trip_ms.unwrap_or(0));
            let mut far = Answerer::new(t400_ms, Answer::ErrorControl);
            // Half the round trip in each direction, since the round trip is
            // what the ODP and the ADP make between them.
            let one_way = RATE as usize * (round_trip_ms.unwrap_or(0) as usize / 2) / 1000;
            let mut out = VecDeque::from(vec![true; one_way]);
            let mut back = VecDeque::from(vec![true; one_way]);

            let mut decoder = Decoder::new(Fcs::Bits16);
            let mut sent: Vec<u32> = Vec::new();
            let mut elapsed = 0;
            let mut gave_up = None;
            let mut i = 0;
            while gave_up.is_none() && elapsed < 60_000 {
                // Read off this end of the line rather than the far end of it,
                // since what is being timed is when the frame left here.
                let bit = stack.next_bit();
                if let Some(Ok(body)) = decoder.feed(bit)
                    && body[1] & !0x10 == 0xaf
                {
                    sent.push(elapsed);
                }
                out.push_back(bit);
                let to_far = out.pop_front().expect("the line lost a bit");
                far.receive(to_far);
                back.push_back(far.transmit());
                stack.feed_bit(back.pop_front().expect("the line lost a bit"));
                if i % BITS_PER_TICK == BITS_PER_TICK - 1 {
                    stack.tick(TICK_MS);
                    far.tick(TICK_MS);
                    elapsed += TICK_MS;
                    if stack.phase() != Phase::Negotiating && !sent.is_empty() {
                        gave_up = Some(elapsed);
                    }
                }
                i += 1;
            }
            let line = match round_trip_ms {
                Some(ms) => format!("a {ms} ms line"),
                None => "an unmeasured line".to_string(),
            };
            let gave_up =
                gave_up.unwrap_or_else(|| panic!("on {line} the XID exchange never ended"));
            let what = format!(
                "on {line} (T401 {t401_ms} ms) XIDs went out at {sent:?} ms and the exchange ended at {gave_up} ms"
            );
            // The one of 8.10.2 and N400 retransmissions of it, on every line:
            // the give-up cannot come first, because it is what happens when
            // the last of those retransmissions goes unanswered.
            assert_eq!(sent.len(), 1 + XID_N400 as usize, "{what}");
            for pair in sent.windows(2) {
                assert!(pair[1] - pair[0] + TICK_MS >= t401_ms, "{what}");
            }
            // And the last command was given its round trip to be answered in,
            // which is the only reason for sending it at all.
            let last_wait = round_trip_ms.unwrap_or(XID_LAST_WAIT_MS).min(t401_ms);
            let after_last = gave_up - sent[sent.len() - 1];
            assert!(
                after_last + SLACK_MS >= last_wait,
                "{what}, only {after_last} ms after the last of them"
            );
            let budget = gave_up - sent[0];
            assert!(
                budget <= budget_ms,
                "{what}, {budget} ms in all against a budget of {budget_ms}"
            );
        }
    }

    /// Each frame the line spoils is recorded and counted once.
    ///
    /// From `live-1789647424.wav`, whose record of frames showed pairs of
    /// identical damaged frames 20 ms apart: the second of each was an abort,
    /// logged with the octets of the damaged frame before it and counted as
    /// damage again. And a line that goes to marks after a flag has not
    /// damaged anything.
    #[test]
    fn a_damaged_frame_is_logged_and_counted_once() {
        let mut stack = Stack::new(Role::Originator, Params::default()).without_detection();
        let mut e = Encoder::new(Fcs::Bits16);
        let rr = Frame::Rr { nr: 0, pf: true }.encode(DLCI_DATA, Role::Answerer, Kind::Command);
        e.frame(&rr);
        let mut bits: Vec<bool> = std::iter::from_fn(|| e.next_bit()).collect();
        bits[12] = !bits[12];
        // Two octets of a frame after the damaged one's closing flag, then an
        // abort; then a flag, and marks.
        let octet = |o: u8| (0..8).map(move |i| o & (1 << i) != 0);
        bits.extend(octet(0x41).chain(octet(0x42)));
        bits.extend([true; 8]);
        bits.extend(octet(0x7e));
        bits.extend([true; 64]);
        for bit in bits {
            stack.next_bit();
            stack.feed_bit(bit);
        }
        let heard: Vec<Crossed> = stack.take_log().into_iter().filter(|f| !f.outbound).collect();
        assert_eq!(heard.len(), 2, "{heard:02x?}");
        assert!(heard.iter().all(|f| !f.intact));
        assert_eq!(heard[0].body.len(), rr.len() + 2, "the damaged frame was not kept");
        assert_eq!(heard[1].body, [0x41, 0x42], "the abort was not logged as itself");
        assert_eq!(stack.damaged_frames(), 2);
    }

    #[test]
    fn a_release_is_seen_at_both_ends() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected());
        a.disconnect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(!a.is_connected(), "the originator is {:?}", a.state());
        assert!(!b.is_connected(), "the answerer is {:?}", b.state());
    }

    /// An XID that arrives after this end stopped waiting still decides the
    /// width of what follows.
    ///
    /// On a line longer than the XID wait, an answerer goes on to the protocol
    /// before the originator's XID can reach it. The XID then arrives and is
    /// answered -- and the answer agrees 32-bit check sequences, so the SABME
    /// that follows comes at 32. An answerer still reading 16 found every one
    /// of them damaged, the originator gave up after three, and the call went
    /// on without error control between two modems that both do it.
    #[test]
    fn an_xid_that_arrives_after_the_wait_still_decides_the_check_sequence() {
        use crate::frame::Kind;

        let mut answerer = Stack::new(Role::Answerer, Params::default()).without_detection();
        let wire = |stack: &mut Stack, body: &[u8], fcs: Fcs| {
            let mut e = Encoder::new(fcs);
            e.idle(LEADING_FLAGS);
            e.frame(body);
            e.idle(2);
            while let Some(bit) = e.next_bit() {
                stack.next_bit();
                stack.feed_bit(bit);
            }
            stack.tick(0);
        };
        // The exchange runs out with nothing heard: one XID, T401, the
        // retransmission, T401 again (8.10.3).
        for _ in 0..80 {
            for _ in 0..1_000 {
                answerer.next_bit();
                answerer.feed_bit(true);
            }
            answerer.tick(100);
        }
        assert_eq!(answerer.phase(), Phase::Protocol, "still waiting for an XID");

        // Then the originator's arrives, offering 32 bits as this modem does.
        let offer = Xid::proposal(Compression::Neither);
        assert!(offer.fcs32, "nothing here to agree to");
        let xid = Frame::Xid { pf: false, info: offer.encode(Kind::Command) }
            .encode(DLCI_DATA, Role::Originator, Kind::Command);
        wire(&mut answerer, &xid, Fcs::Bits16);
        let answered = answerer
            .take_log()
            .iter()
            .any(|f| f.outbound && Frame::decode(&f.body, Role::Originator)
                .is_ok_and(|(_, frame)| matches!(frame, Frame::Xid { .. })));
        assert!(answered, "the late XID was not answered");

        // And the SABME comes at the width the answer agreed to.
        let sabme = Frame::Sabme { poll: true }.encode(DLCI_DATA, Role::Originator, Kind::Command);
        wire(&mut answerer, &sabme, Fcs::Bits32);
        assert_eq!(answerer.damaged_frames(), 0, "the SABME could not be read");
        assert!(answerer.is_connected(), "the answerer is {:?}", answerer.state());
        assert_eq!(answerer.fcs(), Fcs::Bits32, "and it did not follow the SABME to 32");
    }

    /// A far end that compresses without ever negotiating it.
    ///
    /// From `live-1788830261.wav`, a call to a real board. It answered the
    /// SABME, polled, acknowledged everything sent to it -- and never sent an
    /// XID, not one, damaged or otherwise. Then two octets of zero appeared in
    /// the data and everything after them was V.42bis codewords, which went to
    /// the terminal as codewords and filled the screen with noise. Turning
    /// error control off fixed it, which is the wrong way round.
    ///
    /// 6.4 negotiates V.42bis in XID and this far end does not, so strictly
    /// there is nothing to follow. But this end's own XID said it could
    /// receive compressed data and the far end took it at its word; refusing
    /// to decode what was advertised leaves a link that is up and unreadable.
    /// Nothing is assumed until the far end says it -- before the escape every
    /// octet goes through untouched -- and the guess is made once.
    #[test]
    fn a_far_end_that_compresses_without_asking_is_followed() {
        use crate::frame::Kind;

        // What the far end puts on the line: text, and then V.42bis deciding
        // it is worth compressing, which it announces with the escape
        // character and ECM and in no other way.
        let params = v42bis::Params { n2: v42bis::OFFERED_N2, n7: v42bis::OFFERED_N7 };
        let text: Vec<u8> = b"Welcome to the board. Please log in. "
            .iter()
            .copied()
            .cycle()
            .take(2000)
            .collect();
        let mut encoder = v42bis::Encoder::new(params);
        let mut stream = Vec::new();
        encoder.encode(&text, &mut stream);
        encoder.flush(&mut stream);
        assert!(
            stream.windows(2).any(|w| w == [0, 0]),
            "the encoder never left transparent mode, so there is nothing here to follow",
        );

        // This end: offering V.42bis, and told by V.8 that the far end does
        // LAPM, so the detection phase is skipped and XID is all there is.
        let mut stack = Stack::new(Role::Originator, Params::default()).without_detection();
        stack.offer_compression(Compression::Both);
        stack.connect();

        // Everything the far end says, framed as it would arrive.
        let wire = |stack: &mut Stack, body: &[u8]| {
            let mut e = Encoder::new(Fcs::Bits16);
            e.frame(body);
            while let Some(bit) = e.next_bit() {
                stack.next_bit();
                stack.feed_bit(bit);
            }
            stack.tick(0);
        };
        // Let it send its XID and its SABME into a silence that answers
        // neither, which is what the recording has.
        // Time as well as bits: the XID exchange ends on T401 and N400
        // (8.10.3), and a stack that is only ever handed bits never reaches
        // the end of it.
        for _ in 0..80 {
            for _ in 0..4_000 {
                stack.next_bit();
                stack.feed_bit(true);
            }
            stack.tick(100);
        }
        wire(
            &mut stack,
            &Frame::Ua { final_bit: true }.encode(DLCI_DATA, Role::Answerer, Kind::Response),
        );
        assert!(stack.is_connected(), "the link never came up");
        assert_eq!(stack.far_xid(), None, "the far end was not supposed to answer XID");

        for (i, chunk) in stream.chunks(64).enumerate() {
            let frame = Frame::I {
                ns: (i % 128) as u8,
                nr: 0,
                poll: false,
                info: chunk.to_vec(),
            };
            wire(&mut stack, &frame.encode(DLCI_DATA, Role::Answerer, Kind::Command));
        }

        assert_eq!(
            stack.take_received(),
            text,
            "the screen got something other than what the far end sent",
        );
    }


    /// And a far end that was never compressing at all keeps its link.
    ///
    /// The price of watching for a switch nobody negotiated: a decoder in
    /// transparent mode reads an escape character as an escape character, so a
    /// far end that sends a literal zero followed by something that is not a
    /// command is misread. That has to cost the guess and nothing else --
    /// dropping a working link over it would be worse than the garbled screen
    /// the guess was made to fix.
    #[test]
    fn a_guess_that_does_not_come_off_costs_only_the_guess() {
        use crate::frame::Kind;

        let mut stack = Stack::new(Role::Originator, Params::default()).without_detection();
        stack.offer_compression(Compression::Both);
        stack.connect();
        let wire = |stack: &mut Stack, body: &[u8]| {
            let mut e = Encoder::new(Fcs::Bits16);
            e.frame(body);
            while let Some(bit) = e.next_bit() {
                stack.next_bit();
                stack.feed_bit(bit);
            }
            stack.tick(0);
        };
        // Long enough for the XID exchange to give up on its own terms: T401,
        // the one retransmission, and T401 again (8.10.3).
        for _ in 0..80 {
            for _ in 0..4_000 {
                stack.next_bit();
                stack.feed_bit(true);
            }
            stack.tick(100);
        }
        wire(
            &mut stack,
            &Frame::Ua { final_bit: true }.encode(DLCI_DATA, Role::Answerer, Kind::Response),
        );
        assert!(stack.is_connected());

        // A zero, and then a code 9.1 Table 3 reserves. Not a switch, and not
        // anything a decoder can make sense of.
        let mut ns = 0u8;
        let mut send = |stack: &mut Stack, info: &[u8]| {
            let frame = Frame::I { ns, nr: 0, poll: false, info: info.to_vec() };
            ns = (ns + 1) % 128;
            let body = frame.encode(DLCI_DATA, Role::Answerer, Kind::Command);
            let mut e = Encoder::new(Fcs::Bits16);
            e.frame(&body);
            while let Some(bit) = e.next_bit() {
                stack.next_bit();
                stack.feed_bit(bit);
            }
            stack.tick(0);
        };
        send(&mut stack, b"login: \x00rest");
        assert!(stack.is_connected(), "a misread guess dropped the link");

        // And the next frame goes through untouched, because the guess is not
        // made twice.
        stack.take_received();
        send(&mut stack, b"password: ");
        assert_eq!(stack.take_received(), b"password: ");
    }
}
