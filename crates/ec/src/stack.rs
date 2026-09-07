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
use crate::v42bis;
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

/// How long to wait for the far end's XID before giving up on negotiating.
///
/// V.42 8.10 does not name a figure. A second is generous at any rate this
/// modem reaches and short enough that a modem which does error control but
/// not parameter negotiation is not left waiting.
const XID_WAIT_MS: u32 = 1000;

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
    /// Ceilings on the V.42bis parameters, if the terminal set any.
    ///
    /// Ceilings twice over: what goes into XID, and then 6.4 takes the lower
    /// of the two ends' proposals.
    limits: (u16, u8),
    /// Whether the far end has already said it does LAPM, in V.8.
    declared: bool,
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
    /// Whether establishment was tried and nothing answered.
    gave_up: bool,
    /// Whether the run of flags that opens the protocol phase has been queued.
    opened: bool,
    waited_ms: u32,
}

/// One end of the detection phase (7.2.1). Which one depends on the role.
#[derive(Debug)]
enum Detect {
    Origin(Box<Originator>),
    Answer(Box<Answerer>),
    Done,
}

#[derive(Debug)]
struct Compressor {
    encoder: v42bis::Encoder,
    decoder: v42bis::Decoder,
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
            detect: match role {
                Role::Originator => {
                    Detect::Origin(Box::new(Originator::new(crate::detect::DEFAULT_T400_MS)))
                }
                Role::Answerer => Detect::Answer(Box::new(Answerer::new(
                    crate::detect::DEFAULT_T400_MS,
                    Answer::ErrorControl,
                ))),
            },
            offer: Compression::Neither,
            limits: (v42bis::OFFERED_N2, v42bis::OFFERED_N7),
            declared: false,
            declining: false,
            heard_adp: None,
            heard_xid: None,
            agreed_fcs: Fcs::Bits16,
            established: false,
            gave_up: false,
            opened: false,
            waited_ms: 0,
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
        self.detect = Detect::Done;
        self.phase = Phase::Negotiating;
        self.waited_ms = 0;
        self.lapm.set_retransmissions(crate::lapm::UNCONFIRMED_N400);
        self
    }

    /// Answer the detection phase by declining error control (V.42 Table 3).
    ///
    /// For tests, and for a configuration in which a terminal has asked for a
    /// connection without it.
    pub fn declining(mut self) -> Self {
        self.declining = true;
        self.detect = Detect::Answer(Box::new(Answerer::new(
            crate::detect::DEFAULT_T400_MS,
            Answer::None,
        )));
        self
    }

    /// Offer V.42bis in the XID exchange.
    ///
    /// Offering is all either end can do. What is used is the intersection of
    /// the two offers, because compression that only one end is doing is worse
    /// than none: the far end would decompress data that was never compressed.
    pub fn offer_compression(&mut self, compression: Compression) {
        self.offer = compression;
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
            encoder: v42bis::Encoder::new(params),
            decoder: v42bis::Decoder::new(params),
        });
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
                self.waited_ms = self.waited_ms.saturating_add(dt_ms);
                if self.waited_ms >= XID_WAIT_MS {
                    // A modem that does error control but declines to negotiate
                    // is a modem to talk to without compression, not one to
                    // wait for indefinitely.
                    self.begin_protocol();
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
            Some(c) => {
                let mut out = Vec::new();
                c.encoder.encode(data, &mut out);
                // Flush, because the terminal has no idea it is being
                // compressed and will sit waiting for an echo of what it
                // typed. A dictionary coder that holds the last few characters
                // back for a better match would look exactly like a hung line.
                c.encoder.flush(&mut out);
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
            if self.phase == Phase::Negotiating {
                // Repeated, rather than sent once, and this is not belt and
                // braces. The two ends leave the detection phase at slightly
                // different moments, since the answerer has to finish saying
                // what it is saying, and while it is still there every bit it
                // is handed goes to its detector rather than its deframer. An
                // XID sent into that window is simply consumed. Repeating it
                // costs nothing and makes the window harmless.
                let body = Frame::Xid {
                    pf: self.role == Role::Originator,
                    info: self.proposal().encode(),
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
        if let Some(params) = agreed.v42bis_params() {
            self.enable_compression(params);
        }
        if agreed.fcs32 {
            self.agreed_fcs = Fcs::Bits32;
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
            let body = Frame::Xid {
                pf: false,
                info: self.proposal().encode(),
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
            Outcome::Answered(a) if a.error_controlled() => {
                if matches!(&self.detect, Detect::Answer(a) if !a.finished_sending()) {
                    return;
                }
                self.detect = Detect::Done;
                self.phase = Phase::Negotiating;
                self.waited_ms = 0;
            }
            // The far end is already talking protocol, so there is nothing
            // left to detect and nothing to answer.
            Outcome::ProtocolStarted => {
                self.detect = Detect::Done;
                self.phase = Phase::Negotiating;
                self.waited_ms = 0;
                self.lapm.set_retransmissions(crate::lapm::UNCONFIRMED_N400);
            }
            // Said its piece and meant it. Going on to XID after sending
            // Table 3's refusal would be talking protocol at an end that has
            // just been told there would not be one.
            Outcome::OriginatorDetected if self.declining => {
                if matches!(&self.detect, Detect::Answer(a) if !a.finished_sending()) {
                    return;
                }
                self.detect = Detect::Done;
                self.phase = Phase::Transparent;
            }
            Outcome::OriginatorDetected => {
                // The answerer has to finish saying what it is saying: cutting
                // its own reply short would leave the originator waiting for
                // the rest of it.
                if matches!(&self.detect, Detect::Answer(a) if !a.finished_sending()) {
                    return;
                }
                self.detect = Detect::Done;
                self.phase = Phase::Negotiating;
                self.waited_ms = 0;
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
                self.detect = Detect::Done;
                self.phase = Phase::Negotiating;
                self.waited_ms = 0;
                self.lapm.set_retransmissions(crate::lapm::UNCONFIRMED_N400);
            }
            Outcome::Answered(_) | Outcome::TimedOut => {
                // No error control at the far end, or nothing there that
                // recognised the question. Either way there is nothing to
                // establish, and the connection carries on without it.
                self.detect = Detect::Done;
                self.phase = Phase::Transparent;
            }
        }
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
                Event::Connected => self.established = true,
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
        match &mut self.compression {
            Some(c) => {
                if c.decoder.decode(&arrived, &mut self.delivered).is_err() {
                    // A compressed stream that will not decode cannot be
                    // recovered from by asking again: the dictionary at each
                    // end is built from everything that came before, so once
                    // they disagree they stay disagreed. V.42bis 6.4 has the
                    // receiver ask for the link to be reset.
                    self.damaged += 1;
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
}
