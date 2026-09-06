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
use crate::lapm::{Event, Lapm, Params, State};
use crate::v42bis;
use crate::xid::{Compression, Xid};

/// The data link both ends use for user data (V.42 8.1.2).
const DLCI_DATA: u8 = 0;

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
    phase: Phase,
    /// The detection phase, until it is over.
    detect: Detect,
    /// What this end offers.
    offer: Compression,
    /// Whether a reply to the far end's XID has gone out. Exactly one is sent,
    /// because a reply to a reply would go round for ever.
    replied: bool,
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
            replied: false,
            waited_ms: 0,
        }
    }

    /// Where the connection has got to.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Answer the detection phase by declining error control (V.42 Table 3).
    ///
    /// For tests, and for a configuration in which a terminal has asked for a
    /// connection without it.
    pub fn declining(mut self) -> Self {
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
                    info: Xid::proposal(self.offer).encode(),
                }
                .encode(DLCI_DATA, self.role, Kind::Command);
                self.encoder.frame(&body);
                queued = true;
            }
            while let Some((frame, kind)) = self.lapm.poll_transmit() {
                let body = frame.encode(DLCI_DATA, self.role, kind);
                self.encoder.frame(&body);
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
            // A frame that did not survive the line is dropped and left to the
            // retransmission machinery, which is what it is for. Counting them
            // is worth doing: it is the difference between a link that is
            // working and one that is only apparently working.
            self.damaged += 1;
            return;
        };
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
            self.receive_xid(info.clone());
            return;
        }
        self.lapm.receive(frame, address.kind);
        self.drain();
    }

    fn receive_xid(&mut self, info: Vec<u8>) {
        let Ok(theirs) = Xid::decode(&info) else {
            self.damaged += 1;
            return;
        };
        let agreed = Xid::proposal(self.offer).resolve(&theirs);
        if let Some(params) = agreed.v42bis_params() {
            self.enable_compression(params);
        }
        // Answer, exactly once. The far end may not have heard anything this
        // end said while it was still in its detection phase, so it needs to
        // be told; but a reply to a reply would go back and forth for ever.
        if !self.replied {
            self.replied = true;
            let body = Frame::Xid {
                pf: false,
                info: Xid::proposal(self.offer).encode(),
            }
            .encode(DLCI_DATA, self.role, Kind::Response);
            self.encoder.frame(&body);
        }
        self.begin_protocol();
    }

    /// Act on how the detection phase came out (7.2.1.2, 7.2.1.3).
    fn settle_detection(&mut self, outcome: Outcome) {
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
        if self.role == Role::Originator {
            self.lapm.connect();
        }
    }

    fn drain(&mut self) {
        let mut arrived: Vec<u8> = Vec::new();
        while let Some(event) = self.lapm.poll_event() {
            if let Event::Data(d) = event {
                arrived.extend_from_slice(&d);
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
