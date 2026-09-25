//! A fixed jitter buffer: small, honest, and it never invents audio.
//!
//! Every softphone has one of these and every one of them is adaptive. That is
//! correct for speech. A voice can be stretched a few milliseconds during a
//! pause and nobody hears it; the buffer watches the arrival times, grows when
//! the network wobbles, and shrinks back by swallowing a little audio when it
//! settles, and the result is a call that stays intelligible over a network
//! that is not. None of that reasoning survives contact with a modem.
//!
//! A modem is a clock recovery loop with a decision device hung off it. Audio
//! inserted into its input is not a small delay, it is a phase step: the
//! receiver's idea of where the symbol boundaries are becomes wrong, and it
//! has to fall out of lock and find them again. This is measured, not feared.
//! MicroSIP's adaptive buffer, in the path this crate exists to replace, put
//! about 20 ms of concealment into the stream every few seconds. One insert
//! took a V.34 receiver out of lock for 1.3 seconds. The same inserts made a
//! line that measures 43 dB signal to noise measure 17 dB, because the
//! estimator was measuring the seams and not the line.
//!
//! So this buffer is fixed. It keeps `target` packets in hand at all times and
//! hands over only what is behind them. It never resamples, never stretches,
//! never quietly drops a packet to catch up, and it never adapts its depth to
//! what the network is doing -- because adapting means changing the length of
//! the audio, and changing the length of the audio is the fault. If the far
//! end's clock runs faster than ours, or nothing is draining this, the buffer
//! grows to `capacity` and then sheds packets, and that is counted and
//! visible in [`Counters`] rather than smoothed away. A buffer that hides its
//! own interventions turns every later measurement into a lie, which is
//! exactly what happened before.
//!
//! # The cushion is a depth, not a starting gun
//!
//! This is written down because it was wrong here and the mistake is easy to
//! make twice. `drain` used to prime once -- wait for `target` packets, set a
//! flag -- and from then on empty the map on every call. The caller drains
//! about ten times per packet interval, so after the first drain the steady
//! depth was one packet or none, and the cushion the rest of this file
//! describes did not exist for any of the call after its first moment. An
//! ordinary pair-swap, the exact case `target` is sized for, then came out as
//! a concealed 20 ms hole plus a `late` packet: a fault invented by the buffer
//! and charged to the network, which is worse than useless when `concealed` is
//! the one number this crate exists to produce. So the rule is a depth, tested
//! on every drain: hand over while more than `target` are waiting, and no gap
//! is written off until `target` packets have piled up behind it and proved
//! the missing one is not merely late.
//!
//! # Concealment is silence, and silence is a fault
//!
//! Where a packet never arrives, this hands over the silent codeword repeated
//! for one packet's length. Not the previous packet again, not an
//! interpolation, not a fade: a repeat of audio the modem has already had is
//! precisely the 20 ms insert that broke V.34 here, and it is worse than a
//! gap because it is plausible. A gap of silence is a dropout, which is a
//! thing a receiver knows how to recognise -- the energy goes away, the
//! equaliser and the timing loop coast, error control asks for the frame
//! again -- and it is what a real line does when it drops out. So the
//! concealment is honest, and every octet of it is counted, because
//! `concealed` is the number that says whether a call failed because of this
//! modem or because of the network under it.
//!
//! # A long gap is handed over a turn at a time
//!
//! Concealment is cheap to produce and expensive to consume. The caller steps
//! the modem once for every sample of whatever a `drain` returns, in one loop,
//! and while it is in that loop it is not polling the SIP agent, not seeing a
//! hang-up request and not putting anything in the window. So a gap of
//! [`MAX_DROPOUT`] packets -- 480,000 codewords, about 960,000 samples once the
//! rate conversion has been through it -- would arrive as one `Vec` and stall
//! the line thread for as long as it took to step through it. A far end that
//! goes quiet for under a minute and then resumes with its sequence carrying on
//! where it left off reaches that by construction, and a VoIP trunk that
//! suppresses silence does it on purpose. So one `drain` builds at most
//! [`CONCEAL_BURST`] packets of it and leaves the rest for the next call: the
//! same silence, the same counters, spread over several turns of the caller's
//! loop instead of one.
//!
//! # Sequence numbers wrap, and a call runs through the wrap
//!
//! Sixteen bits at fifty packets a second is twenty-two minutes. A download
//! is longer than that. So the sequence is extended to a running 64-bit
//! number by counting the wraps, along the lines of RFC 3550 A.1's
//! `update_seq`: a step of less than half the space is forwards, and a step
//! forwards that lands below where we were is a wrap. Getting this wrong does
//! not cost a packet, it costs the rest of the call -- every packet after the
//! wrap looks ancient and gets thrown away as late.
//!
//! Unlike A.1 there is no probation period here and no source validation. A.1
//! is guarding against a stream of packets from somewhere else being taken up
//! as the source; this buffer is fed by `media`'s reader, which has already
//! kept to the call's stream -- its address, or its SSRC carrying on from
//! another -- and a modem call cannot afford to discard the first two packets
//! of a stream while it makes up its mind.

use std::collections::BTreeMap;

/// Where the extended sequence numbering starts, in cycles.
///
/// Not zero, so that a packet reordered backwards across the very first wrap
/// -- or simply the second packet of a call arriving before the first -- has
/// somewhere below the start to be placed without the arithmetic going round
/// the bottom. One cycle of headroom is enough: a reorder spans milliseconds
/// and a cycle is twenty-two minutes.
const FIRST_CYCLE: u64 = 1 << 16;

/// How far forward the sequence may jump and still be read as a gap in one
/// stream rather than the start of another.
///
/// RFC 3550 A.1's `MAX_DROPOUT`, which is the same boundary drawn for the same
/// reason: a step this side of it is loss, and a step beyond it is a source
/// that has restarted. It matters here because a gap is filled with silence to
/// keep the timeline, and a far end that restarts its stream picks a fresh
/// random sequence number -- so without a boundary, one such restart asks this
/// to build tens of minutes of silence for a hole that was never in the line.
/// One minute is already far past anything a line does.
///
/// This bounds the lie, not the work: a gap of a minute is still a minute of
/// silence owed, and [`CONCEAL_BURST`] is what keeps it from arriving all at
/// once.
const MAX_DROPOUT: u64 = 3000;

/// How many packets of concealment one call to [`Jitter::drain`] will build
/// before leaving the rest to the next call.
///
/// Twenty, which is 400 ms of line at the usual packet time and about 6400
/// samples for the caller to step the modem through -- a few milliseconds of
/// work, so the line thread comes back round to the SIP agent and the window
/// well inside one packet interval. The caller drains about ten times per
/// packet interval, so a full [`MAX_DROPOUT`] gap is still emptied in a few
/// hundred milliseconds of wall clock; it just does not arrive in one block.
/// The counters are per packet, so what a gap costs is the same either way.
const CONCEAL_BURST: usize = 20;

/// How many packets of a new length it takes to believe the packet time has
/// really changed. Three in a row is a stream, one is a stray.
const SETTLED: usize = 3;

/// Everything this buffer had to do to the stream, so that a call that went
/// wrong can be explained afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    pub received: u64,
    /// Packets that never arrived, counted when the stream moved past them.
    pub lost: u64,
    /// The same sequence number twice.
    pub duplicated: u64,
    /// Arrived out of order but in time to be put back in place.
    pub reordered: u64,
    /// Arrived after the stream had already moved past: too late to use.
    ///
    /// A second copy of a packet that has already been handed over lands here
    /// rather than in `duplicated`. From where this buffer stands the two are
    /// the same event and have the same consequence -- nothing can be done
    /// with it -- and telling them apart would mean remembering every
    /// sequence number of the call to no purpose.
    pub late: u64,
    /// Octets of silence handed out in place of audio that did not arrive.
    /// Any at all is a hole in the line: this is the number that says whether
    /// a failed call was the modem's fault or the network's.
    pub concealed: u64,
    /// Packets thrown away because nothing was draining the buffer.
    pub overflowed: u64,
    /// And the audio in them, in octets, so that a log and a capture can be
    /// lined up over an overflow the way they can over concealment. A count of
    /// packets alone cannot be turned into a length of line.
    pub overflowed_octets: u64,
    /// Octets of audio that had arrived and was still waiting when the buffer
    /// was restarted: the cushion, plus anything behind it, thrown away
    /// because a new call started on this buffer. At the end of a call it is
    /// the last few milliseconds and means nothing; during one it means
    /// something restarted the media path mid-call, and it is that many octets
    /// the modem was owed and never got.
    pub abandoned: u64,
    /// Times the sequence jumped further forward than [`MAX_DROPOUT`]: a far
    /// end that restarted its stream rather than a gap in this one. The
    /// timeline is stepped to the new numbering without concealing the
    /// difference, because the difference is not a hole in the line -- it is
    /// audio that was never sent. Non-zero on a call that was not
    /// renegotiated means the far end's RTP source changed underneath us.
    pub resynced: u64,
    /// The most packets ever waiting at once.
    pub deepest: usize,
    /// Times the payload type changed mid-call, which means the far end
    /// switched codec under us.
    pub payload_changed: u64,
}

/// A fixed store of packets waiting their turn, in sequence order.
#[derive(Debug)]
pub struct Jitter {
    /// Packets to hold before anything is handed over.
    target: usize,
    /// Packets to hold before the oldest are shed.
    capacity: usize,
    /// What is waiting, keyed by extended sequence number. A map rather than
    /// a ring because the key is what puts a reordered packet back in place,
    /// and because the first key is the one that is due next.
    waiting: BTreeMap<u64, Vec<u8>>,
    /// The extended sequence number due out next. Everything below it has
    /// been handed over, concealed or shed.
    next: u64,
    /// The highest extended sequence accepted so far, which is what makes a
    /// packet behind it a reorder rather than the ordinary case.
    highest: u64,
    /// Wrap counting, per the module header.
    cycles: u64,
    /// The highest raw sequence seen, which is what the next one is compared
    /// against.
    max_seq: u16,
    /// Whether the first packet of this stream has arrived and set it up.
    started: bool,
    /// How long a packet is, learned from the traffic rather than configured.
    /// Concealment is this long, so it is the length the stream has settled
    /// on and not the length of the last thing that arrived: one odd packet
    /// used to move it, and then every concealment for the rest of the call
    /// was that odd length, so `concealed` under-reported by whatever ratio
    /// the odd packet happened to be. A four-octet RFC 4733 event packet did
    /// it by a factor of forty.
    packet_len: usize,
    /// A length that is not `packet_len`, and how many packets in a row have
    /// arrived with it. The packet time genuinely can change mid-call, so a
    /// new length is taken up once the stream has stayed at it.
    other_len: usize,
    other_run: usize,
    /// The payload type in force, to notice it changing.
    payload_type: Option<u8>,
    counters: Counters,
}

impl Jitter {
    /// `target` packets of delay before anything is handed over, `capacity`
    /// before packets are dropped. Both in packets, not milliseconds: the
    /// caller knows the packet size and this does not need to.
    ///
    /// A capacity that is not at least one packet above the target would shed
    /// every packet the moment it arrived on top of the cushion, and hand over
    /// nothing at all: a silent dead line rather than a loud mistake. It is
    /// raised instead, and that is the only number here that is not the
    /// caller's.
    pub fn new(target: usize, capacity: usize) -> Self {
        Self {
            target,
            capacity: capacity.max(target + 1),
            waiting: BTreeMap::new(),
            next: 0,
            highest: 0,
            cycles: 0,
            max_seq: 0,
            started: false,
            packet_len: 0,
            other_len: 0,
            other_run: 0,
            payload_type: None,
            counters: Counters::default(),
        }
    }

    /// A packet off the network. `payload` is the raw G.711 codewords.
    pub fn push(&mut self, sequence: u16, payload_type: u8, payload: Vec<u8>) {
        self.counters.received += 1;

        match self.payload_type {
            Some(current) if current != payload_type => {
                // The far end has switched codec under us. The packet is
                // still handed over: this layer does not know which law is
                // which, and the layer above has to decide whether to follow
                // the change or hang up on it.
                self.counters.payload_changed += 1;
                self.payload_type = Some(payload_type);
            }
            None => self.payload_type = Some(payload_type),
            _ => {}
        }

        if !payload.is_empty() {
            self.note_length(payload.len());
        }

        if !self.started {
            // The first packet sets the stream. Where it starts is where the
            // numbering starts: RFC 3550 5.1 makes the far end's first
            // sequence number arbitrary, so there is nothing to compare it
            // to and nothing before it to wait for.
            self.started = true;
            self.cycles = FIRST_CYCLE;
            self.max_seq = sequence;
            self.next = FIRST_CYCLE + u64::from(sequence);
            self.highest = self.next;
        }

        let extended = self.extend(sequence);

        if extended < self.next {
            // The stream has already gone past this. Inserting it now would
            // put audio in front of the modem that belongs behind audio it
            // has already had, which is worse than the hole it would fill.
            self.counters.late += 1;
            return;
        }
        if self.waiting.contains_key(&extended) {
            self.counters.duplicated += 1;
            return;
        }
        if extended < self.highest {
            self.counters.reordered += 1;
        } else {
            self.highest = extended;
        }

        self.waiting.insert(extended, payload);

        // Over capacity, the oldest go. Not the newest: audio the modem never
        // got is worth nothing to it, and the thing actually being shed is
        // the delay those packets represent -- a buffer this deep means the
        // far end is ahead of us or nothing is draining this, and either way
        // holding on to the front of it only makes the line longer. So the
        // stream steps past them without concealment. Concealing would emit
        // the same number of octets and shed no delay at all, which would
        // make the overflow pointless as well as audible.
        while self.waiting.len() > self.capacity {
            let oldest = *self.waiting.keys().next().expect("over capacity but empty");
            let shed = self.waiting.remove(&oldest).expect("just read the key");
            self.counters.overflowed += 1;
            self.counters.overflowed_octets += shed.len() as u64;
            self.next = self.next.max(oldest + 1);
        }

        self.counters.deepest = self.counters.deepest.max(self.waiting.len());
    }

    /// What a packet of this stream is worth in octets, which is how long a
    /// concealment is. One packet of an odd length does not move it; a stream
    /// of them does, because a re-INVITE really can change the packet time
    /// mid-call.
    fn note_length(&mut self, len: usize) {
        if self.packet_len == 0 {
            // Nothing to compare against: the first packet is the stream.
            self.packet_len = len;
            return;
        }
        if len == self.packet_len {
            self.other_len = 0;
            self.other_run = 0;
            return;
        }
        if len == self.other_len {
            self.other_run += 1;
        } else {
            self.other_len = len;
            self.other_run = 1;
        }
        if self.other_run >= SETTLED {
            self.packet_len = self.other_len;
            self.other_len = 0;
            self.other_run = 0;
        }
    }

    /// Extend a sixteen-bit sequence number to the running count, by RFC 3550
    /// A.1's reasoning: a difference of less than half the sequence space is
    /// forwards, and anything else is a packet from behind.
    fn extend(&mut self, sequence: u16) -> u64 {
        let step = sequence.wrapping_sub(self.max_seq);
        if step < 0x8000 {
            // Forwards, or the same packet again. Landing below where we were
            // while still moving forwards is the wrap.
            if sequence < self.max_seq {
                self.cycles += 0x1_0000;
            }
            self.max_seq = sequence;
            self.cycles + u64::from(sequence)
        } else if sequence > self.max_seq {
            // Behind the highest, and numerically above it: a straggler from
            // before the most recent wrap. This is the case that costs a
            // whole call when it is got wrong, because at the wrap every
            // reordered packet takes this branch.
            self.cycles - 0x1_0000 + u64::from(sequence)
        } else {
            // Behind the highest, in the same cycle: an ordinary reorder.
            self.cycles + u64::from(sequence)
        }
    }

    /// Append every packet that is now due, in sequence order, to `out`.
    /// Where a packet is missing and a later one has arrived, append
    /// `conceal` repeated for the expected packet length and count it.
    /// Returns how many octets were appended.
    ///
    /// `conceal` is the caller's because silence is a codeword and not a
    /// zero: it is 0xFF in mu-law and 0xD5 in A-law, and a buffer full of
    /// zero octets is a loud tone in both.
    ///
    /// A missing packet is only concealed once `target` packets have piled up
    /// behind it. Until then it may still be in flight, and there is nothing
    /// to be gained by deciding early -- the delay of waiting is the delay
    /// this buffer exists to spend.
    ///
    /// Nothing is handed over unless more than `target` packets are waiting,
    /// which is what leaves `target` of them in hand afterwards. That holds on
    /// every call and not only at the start of the stream: the caller drains
    /// far more often than packets arrive, so a cushion that is only checked
    /// once is a cushion that exists for one drain and never again. Two
    /// consequences worth expecting. The line starts `target` packets late,
    /// which is the delay being bought. And when the far end stops sending,
    /// the last `target` packets stay here rather than coming out -- the
    /// buffer cannot tell the end of a call from a pause, and inventing an
    /// end would mean handing over a gap that later turns out not to be one.
    /// They are counted as `abandoned` when the buffer is restarted.
    ///
    /// At most [`CONCEAL_BURST`] packets of concealment are built in one call,
    /// for the reason in the module header: what is returned is stepped
    /// through the modem in one loop by a thread that has other work, and a
    /// gap of a minute would hold it there. The rest is left where it is and
    /// comes out of the next call, so nothing is lost and no counter moves
    /// twice -- only the block the caller is handed is bounded.
    pub fn drain(&mut self, out: &mut Vec<u8>, conceal: u8) -> usize {
        let before = out.len();
        let mut concealed = 0;
        while self.waiting.len() > self.target {
            let first = *self.waiting.keys().next().expect("more than target are waiting");
            if first == self.next {
                let payload = self.waiting.remove(&first).expect("just read the key");
                out.extend_from_slice(&payload);
                self.next += 1;
                continue;
            }
            // Something later is here, and enough of it that the one due now
            // is not merely behind it.
            debug_assert!(first > self.next, "the map handed back an old packet");
            if first - self.next > MAX_DROPOUT {
                // Not a gap: a far end that has started a new stream at a new
                // random sequence number. Filling this with silence would put
                // however many minutes of it into the modem in one go, for a
                // hole that was never in the line. The timeline steps instead,
                // and says so.
                self.counters.resynced += 1;
                self.next = first;
                continue;
            }
            if concealed == CONCEAL_BURST {
                // Enough for this turn. `next` has already moved over what was
                // concealed and the rest of the gap is still ahead of it, so
                // the next call carries on from here -- there is no state to
                // remember beyond what the stream already keeps.
                break;
            }
            out.resize(out.len() + self.packet_len, conceal);
            self.counters.lost += 1;
            self.counters.concealed += self.packet_len as u64;
            self.next += 1;
            concealed += 1;
        }
        out.len() - before
    }

    /// How many packets are waiting. The delay this is adding, in packets.
    pub fn depth(&self) -> usize {
        self.waiting.len()
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// A new call on the same buffer: everything forgotten, counters kept.
    ///
    /// The counters survive because they are the record of the line and not
    /// of the call, and a retry after a failed connect is exactly when the
    /// question being asked is whether the first attempt was the network.
    ///
    /// What was still waiting is counted into `abandoned` on the way out. It
    /// used to go silently, and silently discarded audio is the same lie as
    /// silently inserted audio: a restart in the middle of a call -- which a
    /// re-INVITE or a repeated 200 OK can cause -- took the cushion with it
    /// and left `concealed` at zero, so the call looked clean at exactly the
    /// moment it was not.
    pub fn restart(&mut self) {
        for (_, payload) in std::mem::take(&mut self.waiting) {
            self.counters.abandoned += payload.len() as u64;
        }
        self.next = 0;
        self.highest = 0;
        self.cycles = 0;
        self.max_seq = 0;
        self.started = false;
        self.packet_len = 0;
        self.other_len = 0;
        self.other_run = 0;
        self.payload_type = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::g711::{PCMA, PCMU};

    /// Real packets are 160 octets; four is enough to tell them apart and
    /// short enough to read in an assertion.
    fn packet(n: u8) -> Vec<u8> {
        vec![n; 4]
    }

    /// The silent mu-law codeword, which is what a caller would really pass.
    const SILENCE: u8 = 0xFF;

    /// Drain the way the caller actually does.
    ///
    /// `Media::receive` is called once per turn of the modem loop, which is
    /// many times per packet interval -- about ten on this rig. A test that
    /// pushes a handful of packets and then drains once is not exercising the
    /// same code, and that is precisely how the cushion came to be missing
    /// from every drain after the first while the tests all passed.
    fn drained_hard(jitter: &mut Jitter, out: &mut Vec<u8>) {
        for _ in 0..10 {
            jitter.drain(out, SILENCE);
        }
    }

    /// And keep at it until there is nothing more to come.
    ///
    /// `drained_hard` is one turn of the caller's loop. A gap longer than
    /// [`CONCEAL_BURST`] takes several, by design, so a test that is about
    /// what a gap cost in total rather than about how it was spread asks for
    /// all of it here.
    fn drained_until_quiet(jitter: &mut Jitter, out: &mut Vec<u8>) {
        for _ in 0..1000 {
            if jitter.drain(out, SILENCE) == 0 {
                return;
            }
        }
        panic!("the buffer never ran out of things to hand over");
    }

    #[test]
    fn a_clean_run_comes_out_exactly_as_it_went_in() {
        let mut jitter = Jitter::new(2, 16);
        let mut sent = Vec::new();
        let mut got = Vec::new();
        for n in 0..20u8 {
            let payload = packet(n);
            sent.extend_from_slice(&payload);
            jitter.push(4000 + u16::from(n), PCMU, payload);
            drained_hard(&mut jitter, &mut got);
        }
        // Everything but the cushion, which is still in hand: two packets of
        // delay is what a target of two means, and the eight octets this run
        // is short are those two packets.
        assert_eq!(got, sent[..sent.len() - 8], "the stream was not handed over unaltered");
        assert_eq!(jitter.depth(), 2, "the cushion is not there");
        assert_eq!(jitter.counters().received, 20);
        assert_eq!(jitter.counters().lost, 0);
        assert_eq!(jitter.counters().concealed, 0);
        assert_eq!(jitter.counters().reordered, 0);
        assert_eq!(jitter.counters().overflowed, 0);
    }

    /// Nothing at all comes out until the cushion is there, however eagerly
    /// it is asked for.
    #[test]
    fn nothing_is_handed_over_until_the_buffer_has_primed() {
        let mut jitter = Jitter::new(3, 16);
        let mut out = Vec::new();
        for sequence in 1..=3u16 {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            assert_eq!(jitter.drain(&mut out, SILENCE), 0);
        }
        assert!(out.is_empty());
        // The fourth is the first one with three behind it.
        jitter.push(4, PCMU, packet(4));
        assert_eq!(jitter.drain(&mut out, SILENCE), 4);
        assert_eq!(out, packet(1));
        assert_eq!(jitter.depth(), 3);
        assert_eq!(jitter.counters().concealed, 0, "priming was mistaken for loss");
    }

    /// Two packets that overtake each other in the network, which on a home
    /// connection happens without anything being wrong -- and at the cadence
    /// the caller really drains at, which is the case that used to fail.
    ///
    /// What went wrong: the buffer emptied itself on every drain, so when 4
    /// arrived with 3 still in flight there was nothing behind 3 to wait
    /// with. It concealed 20 ms of silence into the modem, counted a packet
    /// lost, and then threw 3 away as late when it turned up a millisecond
    /// later. Two faults in the audio and two lies in the counters, for an
    /// event that costs nothing anywhere else.
    #[test]
    fn two_packets_swapped_in_flight_come_out_in_order() {
        let mut jitter = Jitter::new(2, 16);
        let mut out = Vec::new();
        for sequence in [1u16, 2, 4, 3, 5, 6] {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            drained_hard(&mut jitter, &mut out);
        }
        assert_eq!(out, [packet(1), packet(2), packet(3), packet(4)].concat());
        assert_eq!(jitter.counters().reordered, 1);
        assert_eq!(jitter.counters().lost, 0);
        assert_eq!(jitter.counters().late, 0, "the packet that came second was thrown away");
        assert_eq!(
            jitter.counters().concealed,
            0,
            "a packet that arrived in time was concealed for anyway"
        );
    }

    /// And the cushion is still the same depth a thousand packets later. The
    /// fault this guards against is a cushion that is set up once and then
    /// spent, which is what a primed flag gives.
    #[test]
    fn the_cushion_is_still_there_a_thousand_packets_later() {
        let mut jitter = Jitter::new(2, 50);
        let mut out = Vec::new();
        for n in 0..1000u16 {
            jitter.push(n, PCMU, packet(n as u8));
            drained_hard(&mut jitter, &mut out);
            if n >= 2 {
                assert_eq!(jitter.depth(), 2, "the cushion was spent at packet {n}");
            }
        }
        assert_eq!(out.len(), 998 * 4);
        assert_eq!(jitter.counters().concealed, 0);
    }

    #[test]
    fn a_packet_that_never_arrives_becomes_one_packet_of_silence() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        jitter.push(1, PCMU, packet(1));
        drained_hard(&mut jitter, &mut out);
        // Two is lost. Three and four arriving is what settles it: one packet
        // behind the gap is the cushion, and the next one is the proof.
        jitter.push(3, PCMU, packet(3));
        drained_hard(&mut jitter, &mut out);
        jitter.push(4, PCMU, packet(4));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, [packet(1), vec![SILENCE; 4], packet(3)].concat());
        assert_eq!(jitter.counters().lost, 1);
        assert_eq!(jitter.counters().concealed, 4);
    }

    /// And not before: while the packet may still be in flight, the buffer
    /// waits rather than deciding early.
    #[test]
    fn a_gap_is_not_concealed_while_the_packet_may_still_be_coming() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        jitter.push(1, PCMU, packet(1));
        drained_hard(&mut jitter, &mut out);
        // Nothing behind it yet, so nothing to hand over and nothing to
        // conceal.
        assert_eq!(jitter.drain(&mut out, SILENCE), 0);
        assert_eq!(jitter.counters().lost, 0);
        assert_eq!(jitter.counters().concealed, 0);
        // Three arrives before two does, and two is still not written off.
        jitter.push(3, PCMU, packet(3));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(jitter.counters().concealed, 0, "two was given up on while in flight");
        jitter.push(2, PCMU, packet(2));
        jitter.push(4, PCMU, packet(4));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, [packet(1), packet(2), packet(3)].concat());
        assert_eq!(jitter.counters().concealed, 0);
        assert_eq!(jitter.counters().late, 0);
    }

    #[test]
    fn a_packet_that_arrives_twice_is_counted_once() {
        let mut jitter = Jitter::new(1, 16);
        jitter.push(7, PCMU, packet(7));
        jitter.push(8, PCMU, packet(8));
        jitter.push(8, PCMU, packet(8));
        jitter.push(9, PCMU, packet(9));
        let mut out = Vec::new();
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, [packet(7), packet(8)].concat());
        assert_eq!(jitter.counters().duplicated, 1);
        assert_eq!(jitter.counters().received, 4);
        assert_eq!(jitter.counters().lost, 0);
    }

    /// The stream does not go backwards for it. A duplicate of a packet
    /// already handed over lands here too, which is why `late` and
    /// `duplicated` should be read together.
    #[test]
    fn a_packet_that_arrives_after_the_stream_passed_it_is_not_put_back_in() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        // One, then three and four: enough behind the gap that two is given
        // up on and the stream is past it.
        for sequence in [1u16, 3, 4] {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            drained_hard(&mut jitter, &mut out);
        }
        assert_eq!(jitter.counters().lost, 1, "two was not given up on");
        let so_far = out.clone();

        jitter.push(2, PCMU, packet(2));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, so_far, "a late packet was spliced in behind the stream");
        assert_eq!(jitter.counters().late, 1);

        // A second copy of one already gone by is the same event.
        jitter.push(3, PCMU, packet(3));
        assert_eq!(jitter.counters().late, 2);
        assert_eq!(jitter.drain(&mut out, SILENCE), 0);
    }

    /// Twenty-two minutes into a call, at fifty packets a second. A buffer
    /// that reads the wrap as a jump backwards throws away everything after
    /// it, which is the rest of the download.
    #[test]
    fn the_sequence_wrapping_through_65535_costs_nothing() {
        let mut jitter = Jitter::new(2, 16);
        let mut sent = Vec::new();
        let mut got = Vec::new();
        for (n, sequence) in [65533u16, 65534, 65535, 0, 1, 2].into_iter().enumerate() {
            let payload = packet(n as u8);
            sent.extend_from_slice(&payload);
            jitter.push(sequence, PCMU, payload);
            drained_hard(&mut jitter, &mut got);
        }
        // All of it but the two still in hand, and the wrap in the middle of
        // it cost nothing.
        assert_eq!(got, sent[..sent.len() - 8]);
        assert_eq!(jitter.counters().lost, 0);
        assert_eq!(jitter.counters().concealed, 0);
        assert_eq!(jitter.counters().late, 0);
        assert_eq!(jitter.counters().reordered, 0);
    }

    /// And a reorder that straddles the wrap, which is the awkward one: 65535
    /// arriving after 0 is numerically far ahead and actually just behind.
    #[test]
    fn a_straggler_from_before_the_wrap_is_put_back_in_its_place() {
        let mut jitter = Jitter::new(1, 16);
        for sequence in [65534u16, 0, 65535, 1, 2] {
            jitter.push(sequence, PCMU, packet(sequence as u8));
        }
        let mut out = Vec::new();
        drained_hard(&mut jitter, &mut out);
        assert_eq!(
            out,
            [packet(65534u16 as u8), packet(65535u16 as u8), packet(0), packet(1)].concat()
        );
        assert_eq!(jitter.counters().reordered, 1);
        assert_eq!(jitter.counters().concealed, 0);
        assert_eq!(jitter.counters().late, 0);
    }

    /// Nothing drained it, so it filled. What it keeps is the newest audio:
    /// the old packets are worthless to a modem that never got them, and the
    /// delay they stand for is the thing being shed.
    #[test]
    fn a_buffer_nothing_is_draining_sheds_its_oldest_packets() {
        let mut jitter = Jitter::new(2, 4);
        for n in 0..8u8 {
            jitter.push(100 + u16::from(n), PCMU, packet(n));
        }
        assert_eq!(jitter.depth(), 4, "the buffer grew past its capacity");
        assert_eq!(jitter.counters().overflowed, 4);
        // In octets as well as packets, so that this can be lined up against
        // a capture the way concealment can.
        assert_eq!(jitter.counters().overflowed_octets, 16);
        assert_eq!(jitter.counters().deepest, 4);

        let mut out = Vec::new();
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, [packet(4), packet(5)].concat());
        assert_eq!(jitter.depth(), 2, "the cushion was spent catching up");
        assert_eq!(
            jitter.counters().concealed,
            0,
            "shed delay came back out as silence, which sheds nothing"
        );
        assert_eq!(jitter.counters().lost, 0);
    }

    /// A far end that stops sending stops the line too: what is left is the
    /// cushion, and the buffer does not invent an ending for it. Concealing
    /// it would mean handing over a hole that a pause is not.
    #[test]
    fn a_far_end_that_goes_quiet_leaves_the_cushion_alone() {
        let mut jitter = Jitter::new(2, 16);
        let mut out = Vec::new();
        for sequence in 1..=6u16 {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            drained_hard(&mut jitter, &mut out);
        }
        let settled = out.len();
        // And now nothing arrives for a long time, while the caller keeps
        // asking.
        for _ in 0..500 {
            jitter.drain(&mut out, SILENCE);
        }
        assert_eq!(out.len(), settled, "the buffer invented an end to the call");
        assert_eq!(jitter.counters().lost, 0);
        assert_eq!(jitter.counters().concealed, 0);
        assert_eq!(jitter.depth(), 2);

        // The two it kept are counted when the buffer is started again,
        // rather than going quietly.
        jitter.restart();
        assert_eq!(jitter.counters().abandoned, 8);
    }

    /// A gap of many packets comes out as exactly that much silence and the
    /// stream carries on from where it resumed. The timeline is what is being
    /// kept: a modem handed a short stream has been told the line ran faster
    /// than it did.
    #[test]
    fn a_long_gap_is_filled_with_silence_and_the_stream_catches_up() {
        let mut jitter = Jitter::new(2, 400);
        let mut out = Vec::new();
        jitter.push(1, PCMU, packet(1));
        jitter.push(2, PCMU, packet(2));
        drained_hard(&mut jitter, &mut out);
        assert!(out.is_empty());

        // Two hundred packets -- four seconds -- went missing. Ten times that
        // much silence as one block is what `CONCEAL_BURST` exists to stop, so
        // this asks until there is nothing left rather than once; what it cost
        // is the same either way, which is the point.
        for sequence in 203..=206u16 {
            jitter.push(sequence, PCMU, packet(sequence as u8));
        }
        drained_until_quiet(&mut jitter, &mut out);
        assert_eq!(jitter.counters().lost, 200);
        assert_eq!(jitter.counters().concealed, 800);
        assert_eq!(out.len(), 4 + 4 + 800 + 8);
        assert_eq!(&out[..4], &packet(1)[..]);
        assert_eq!(&out[8..808], &[SILENCE; 800][..]);
        assert_eq!(&out[808..812], &packet(203u16 as u8)[..]);
        assert_eq!(jitter.depth(), 2);
    }

    /// A far end that goes quiet for the best part of a minute and comes back
    /// with its sequence carrying on where it left off. Nothing in the
    /// numbering says a packet was skipped, so the whole of it is owed
    /// silence -- and none of it may arrive in one block.
    ///
    /// What went wrong: `drain` built the whole gap in one call, up to
    /// `MAX_DROPOUT` packets. That is 480,000 codewords, about 960,000 samples
    /// once the rate conversion has been through them, handed over as one
    /// `Vec` and stepped through the modem in one loop -- and for as long as
    /// that took, the line thread was not polling the SIP agent, not seeing a
    /// hang-up request and not putting anything in the window. A VoIP trunk
    /// that suppresses silence reaches it without anything being wrong.
    #[test]
    fn a_minute_of_silence_is_handed_over_a_turn_at_a_time() {
        /// Packets the far end did not send. Just inside `MAX_DROPOUT`, so
        /// this is read as a gap in one stream and not as a new one.
        const GAP: u16 = 2900;

        let mut jitter = Jitter::new(2, 16);
        let mut out = Vec::new();
        jitter.push(1, PCMU, packet(1));
        jitter.push(2, PCMU, packet(2));
        drained_hard(&mut jitter, &mut out);
        assert!(out.is_empty(), "the cushion was handed over");

        for sequence in (3 + GAP)..=(6 + GAP) {
            jitter.push(sequence, PCMU, packet(sequence as u8));
        }

        // Every drain measured, because what is being tested is the size of
        // the block the caller is handed and not only the total.
        let mut turns = Vec::new();
        loop {
            let got = jitter.drain(&mut out, SILENCE);
            turns.push(got);
            if got == 0 {
                break;
            }
            assert!(turns.len() < 1000, "the gap never finished");
        }

        // The first drain: the two packets that were in hand, then one burst
        // of silence and no more.
        assert_eq!(turns[0], 2 * 4 + CONCEAL_BURST * 4, "the first drain was not bounded");
        let one_turn: usize = turns[..10].iter().sum();
        assert_eq!(
            one_turn,
            2 * 4 + 10 * CONCEAL_BURST * 4,
            "a whole turn of the caller's loop handed over {one_turn} octets"
        );
        let biggest = turns.iter().copied().max().unwrap_or(0);
        assert!(
            biggest <= (CONCEAL_BURST + 2) * 4,
            "one drain handed over {biggest} octets, which is the fault this test is about"
        );
        assert!(
            turns.len() > usize::from(GAP) / CONCEAL_BURST,
            "the gap was not spread over turns: {} of them",
            turns.len()
        );

        // And it comes to exactly what the gap was worth, which is the whole
        // point of spreading it: the same silence and the same counters, in
        // pieces the caller can get round its loop between.
        assert_eq!(jitter.counters().lost, u64::from(GAP));
        assert_eq!(jitter.counters().concealed, u64::from(GAP) * 4);
        assert_eq!(out.len(), 2 * 4 + usize::from(GAP) * 4 + 2 * 4);
        assert_eq!(&out[..4], &packet(1)[..]);
        assert_eq!(&out[8..12], &[SILENCE; 4], "the silence did not start where the gap did");
        assert_eq!(
            &out[out.len() - 8..],
            [packet((3 + GAP) as u8), packet((4 + GAP) as u8)].concat(),
            "the stream did not resume where the far end did"
        );
        assert_eq!(jitter.depth(), 2, "the cushion is not there");
    }

    /// A jump no line could produce is a far end that started a new stream,
    /// not a hole in this one. It is stepped over and counted, because
    /// building an hour of silence for it would stall the modem loop for as
    /// long as it took to allocate -- and would be an hour of line that never
    /// existed.
    #[test]
    fn a_sequence_that_jumps_a_whole_stream_forward_is_not_a_gap() {
        let mut jitter = Jitter::new(2, 16);
        let mut out = Vec::new();
        for sequence in 1..=4u16 {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            drained_hard(&mut jitter, &mut out);
        }
        let before = out.len();
        for sequence in 30000..=30003u16 {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            drained_hard(&mut jitter, &mut out);
        }
        assert_eq!(jitter.counters().resynced, 1);
        assert_eq!(jitter.counters().concealed, 0, "a new stream was read as a hole");
        // What it had in hand at the jump went out, then the new stream from
        // its own beginning.
        assert_eq!(out.len(), before + 4 * 4);
        assert_eq!(&out[out.len() - 4..], &packet(30001u16 as u8)[..]);
    }

    /// The far end changing codec mid-call is counted and passed on. This
    /// layer cannot know whether the change is to be followed or hung up on.
    #[test]
    fn a_codec_change_is_counted_and_the_packet_is_still_handed_over() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        jitter.push(1, PCMU, packet(1));
        jitter.push(2, PCMA, packet(2));
        jitter.push(3, PCMA, packet(3));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, [packet(1), packet(2)].concat());
        assert_eq!(jitter.counters().payload_changed, 1, "the switch was not noticed");
    }

    /// One odd-sized packet does not decide how long a concealment is.
    ///
    /// What went wrong: the length was taken from whatever arrived last, so a
    /// single four-octet RFC 4733 event packet left every concealment for the
    /// rest of the call four octets long instead of a hundred and sixty. The
    /// audio was short by the difference and `concealed` under-reported it by
    /// a factor of forty -- the one counter this buffer exists to produce,
    /// wrong in the direction that says the network was fine.
    #[test]
    fn one_odd_packet_does_not_change_how_long_a_concealment_is() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        jitter.push(1, PCMU, vec![1; 160]);
        jitter.push(2, PCMU, vec![2; 4]);
        drained_hard(&mut jitter, &mut out);
        // Three and four are missing, and five and six settle it.
        jitter.push(5, PCMU, vec![5; 160]);
        jitter.push(6, PCMU, vec![6; 160]);
        drained_hard(&mut jitter, &mut out);
        assert_eq!(jitter.counters().lost, 2);
        assert_eq!(
            jitter.counters().concealed,
            320,
            "the concealment was sized from the odd packet"
        );
    }

    /// But a packet time that really changes is taken up, because a re-INVITE
    /// can change it and then every later concealment is the new length.
    #[test]
    fn a_packet_time_that_really_changes_is_taken_up() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        for sequence in 1..=6u16 {
            jitter.push(sequence, PCMU, vec![sequence as u8; 160]);
            drained_hard(&mut jitter, &mut out);
        }
        // The far end moves to 40 ms packets and stays there.
        for sequence in 7..=10u16 {
            jitter.push(sequence, PCMU, vec![sequence as u8; 320]);
            drained_hard(&mut jitter, &mut out);
        }
        // Then one goes missing at the new length.
        for sequence in 12..=13u16 {
            jitter.push(sequence, PCMU, vec![sequence as u8; 320]);
            drained_hard(&mut jitter, &mut out);
        }
        assert_eq!(jitter.counters().lost, 1);
        assert_eq!(jitter.counters().concealed, 320, "the new packet time was not taken up");
    }

    /// A second call on the same buffer starts from nothing, including its
    /// idea of where the sequence numbering is -- the new far end's first
    /// packet is unrelated to the old one's last.
    #[test]
    fn a_restart_forgets_the_stream_and_keeps_the_counters() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        jitter.push(60000, PCMU, packet(1));
        jitter.push(60002, PCMU, packet(2));
        jitter.push(60003, PCMU, packet(3));
        drained_hard(&mut jitter, &mut out);
        let before = jitter.counters().clone();
        assert_eq!(before.lost, 1);
        assert_eq!(jitter.depth(), 1);

        jitter.restart();
        assert_eq!(jitter.depth(), 0);
        assert_eq!(
            *jitter.counters(),
            Counters {
                // The one packet still in hand went with the restart, and is
                // the only thing about the record that changed.
                abandoned: before.abandoned + 4,
                ..before.clone()
            },
            "the record of the line was thrown away"
        );

        // A sequence far below the old one, which before the restart would
        // have been dismissed as late.
        out.clear();
        jitter.push(10, PCMU, packet(3));
        jitter.push(11, PCMU, packet(4));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(out, packet(3));
        assert_eq!(jitter.counters().late, 0);
        assert_eq!(jitter.counters().lost, before.lost, "the second call lost nothing");
    }

    /// The whole reason for the counters: a stretch of packets that simply
    /// did not arrive comes out as exactly that much silence, so a capture
    /// and a log agree on where the hole was and how long it was.
    #[test]
    fn a_burst_of_loss_is_the_same_length_of_silence() {
        let mut jitter = Jitter::new(1, 16);
        let mut out = Vec::new();
        jitter.push(1, PCMU, packet(1));
        drained_hard(&mut jitter, &mut out);
        // Two through six went missing.
        jitter.push(7, PCMU, packet(7));
        jitter.push(8, PCMU, packet(8));
        drained_hard(&mut jitter, &mut out);
        assert_eq!(jitter.counters().lost, 5);
        assert_eq!(jitter.counters().concealed, 20);
        assert_eq!(out.len(), 4 + 20 + 4);
        assert_eq!(&out[4..24], &[SILENCE; 20]);
    }

    /// A capacity no deeper than the cushion would shed every packet as it
    /// landed and hand over none of them, which is a line that is up and
    /// silent. It is raised to where it can work.
    #[test]
    fn a_capacity_that_could_never_hand_anything_over_is_raised() {
        let mut jitter = Jitter::new(3, 3);
        let mut out = Vec::new();
        for sequence in 1..=6u16 {
            jitter.push(sequence, PCMU, packet(sequence as u8));
            drained_hard(&mut jitter, &mut out);
        }
        assert!(!out.is_empty(), "the line came up silent");
    }
}
