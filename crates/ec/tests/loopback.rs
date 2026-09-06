//! Two LAPM entities talking through the real HDLC layer.
//!
//! The unit tests hand `Frame` values straight across, which proves the state
//! machine but skips everything between: address encoding, bit stuffing, the
//! frame check sequence and framing errors. Here the frames are encoded to bits
//! and decoded back, so a mistake in any of that shows up as data that fails to
//! arrive.

use ec::frame::{DLCI_DATA, Frame, Role};
use ec::hdlc::{Decoder, Encoder, Fcs};
use ec::lapm::{Event, Lapm, Params, State};

/// One end of the link: a LAPM entity plus its HDLC codec.
struct End {
    lapm: Lapm,
    encoder: Encoder,
    decoder: Decoder,
    role: Role,
    received: Vec<u8>,
}

impl End {
    fn new(role: Role, params: Params) -> Self {
        Self {
            lapm: Lapm::new(role, DLCI_DATA, params),
            encoder: Encoder::new(Fcs::Bits16),
            decoder: Decoder::new(Fcs::Bits16),
            role,
            received: Vec::new(),
        }
    }

    /// Encode everything LAPM wants to send, returning the bits for the line.
    fn transmit(&mut self) -> Vec<bool> {
        while let Some((frame, kind)) = self.lapm.poll_transmit() {
            let body = frame.encode(DLCI_DATA, self.role, kind);
            self.encoder.frame(&body);
        }
        let mut bits = Vec::new();
        while let Some(b) = self.encoder.next_bit() {
            bits.push(b);
        }
        bits
    }

    /// Feed line bits in, decoding and dispatching any frames they carry.
    fn receive(&mut self, bits: &[bool]) {
        for &bit in bits {
            let Some(result) = self.decoder.feed(bit) else { continue };
            let Ok(body) = result else { continue }; // damaged frames are dropped
            let Ok((address, frame)) = Frame::decode(&body, self.role) else {
                continue;
            };
            self.lapm.receive(frame, address.kind);
        }
        self.drain_events();
    }

    fn drain_events(&mut self) {
        while let Some(event) = self.lapm.poll_event() {
            if let Event::Data(d) = event {
                self.received.extend_from_slice(&d);
            }
        }
    }
}

/// Run both ends until neither has anything left to say.
fn settle(a: &mut End, b: &mut End) {
    settle_with(a, b, |bits, _| bits.to_vec());
}

/// Run both ends, passing every burst of bits through `channel` first.
///
/// The closure receives the bits and a running burst counter, so a test can
/// corrupt a chosen burst.
fn settle_with<F>(a: &mut End, b: &mut End, mut channel: F)
where
    F: FnMut(&[bool], usize) -> Vec<bool>,
{
    let mut burst = 0usize;
    let mut quiet = 0usize;
    for _ in 0..400 {
        let from_a = a.transmit();
        let from_b = b.transmit();
        if from_a.is_empty() && from_b.is_empty() {
            // Let the acknowledgement timer run before giving up. A frame lost
            // at the very end of a transfer has nothing following it to arrive
            // out of sequence, so no reject is ever provoked and T401 recovery
            // is the only thing that can retrieve it.
            quiet += 1;
            if quiet > 3 {
                return;
            }
            let t401 = Params::default().t401_ms;
            a.lapm.tick(t401);
            b.lapm.tick(t401);
            continue;
        }
        quiet = 0;
        if !from_a.is_empty() {
            let delivered = channel(&from_a, burst);
            burst += 1;
            b.receive(&delivered);
        }
        if !from_b.is_empty() {
            let delivered = channel(&from_b, burst);
            burst += 1;
            a.receive(&delivered);
        }
    }
    panic!("the link never settled");
}

fn pair() -> (End, End) {
    (
        End::new(Role::Originator, Params::default()),
        End::new(Role::Answerer, Params::default()),
    )
}

#[test]
fn a_connection_establishes_through_real_framing() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);
    assert_eq!(a.lapm.state(), State::Connected);
    assert_eq!(b.lapm.state(), State::Connected);
}

#[test]
fn data_survives_the_full_stack() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    let message = b"CONNECT 300\r\nWelcome to the board.\r\n";
    a.lapm.send_data(message);
    settle(&mut a, &mut b);
    assert_eq!(b.received, message);
}

#[test]
fn a_payload_of_flag_bytes_survives_stuffing() {
    // 0x7E and 0xFF runs are exactly what transparency exists for: unstuffed
    // they would read as flags or aborts and destroy the frame.
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    let mut message = vec![0x7eu8; 200];
    message.extend(std::iter::repeat_n(0xffu8, 200));
    a.lapm.send_data(&message);
    settle(&mut a, &mut b);
    assert_eq!(b.received, message);
}

#[test]
fn a_large_transfer_crosses_intact() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    // Comfortably more than one window of full-size frames, so the sender
    // has to stop and wait for acknowledgements repeatedly.
    let payload: Vec<u8> = (0..8192).map(|i| (i * 7 % 256) as u8).collect();
    a.lapm.send_data(&payload);
    settle(&mut a, &mut b);
    assert_eq!(b.received.len(), payload.len());
    assert_eq!(b.received, payload);
}

#[test]
fn both_directions_at_once() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    let up: Vec<u8> = (0..1500).map(|i| (i % 253) as u8).collect();
    let down: Vec<u8> = (0..1500).map(|i| (i % 247) as u8).collect();
    a.lapm.send_data(&up);
    b.lapm.send_data(&down);
    settle(&mut a, &mut b);
    assert_eq!(b.received, up);
    assert_eq!(a.received, down);
}

#[test]
fn a_corrupted_frame_is_recovered() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    let payload: Vec<u8> = (0..2000).map(|i| (i % 251) as u8).collect();
    a.lapm.send_data(&payload);

    // Flip a bit in the middle of one burst. The frame check sequence should
    // catch it, the frame gets dropped, and go-back-N should replace it.
    settle_with(&mut a, &mut b, |bits, burst| {
        let mut out = bits.to_vec();
        if burst == 2 && out.len() > 64 {
            let i = out.len() / 2;
            out[i] = !out[i];
        }
        out
    });

    assert_eq!(
        b.received.len(),
        payload.len(),
        "sent {} bytes, received {}",
        payload.len(),
        b.received.len()
    );
    assert_eq!(
        b.received, payload,
        "error control should have recovered the damaged frame"
    );
}

#[test]
fn several_corrupted_frames_are_recovered() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    let payload: Vec<u8> = (0..4000).map(|i| (i % 249) as u8).collect();
    a.lapm.send_data(&payload);

    settle_with(&mut a, &mut b, |bits, burst| {
        let mut out = bits.to_vec();
        // Damage every fifth burst that is large enough to be carrying data.
        if burst % 5 == 3 && out.len() > 128 {
            let i = out.len() / 3;
            out[i] = !out[i];
        }
        out
    });

    assert_eq!(b.received, payload, "repeated damage should still recover");
}

#[test]
fn a_release_is_confirmed_through_the_stack() {
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    a.lapm.disconnect();
    settle(&mut a, &mut b);
    assert_eq!(a.lapm.state(), State::Disconnected);
    assert_eq!(b.lapm.state(), State::Disconnected);
}

#[test]
fn addressing_distinguishes_commands_from_responses() {
    // V.42 Table 6: the same octet means opposite things at each end, so if the
    // roles were confused a response would be read as a command and the link
    // would never settle. Reaching the connected state proves it does not.
    let (mut a, mut b) = pair();
    a.lapm.connect();
    settle(&mut a, &mut b);

    // Prove it in the other direction too.
    let (mut c, mut d) = pair();
    d.lapm.connect();
    settle(&mut c, &mut d);
    assert_eq!(c.lapm.state(), State::Connected);
    assert_eq!(d.lapm.state(), State::Connected);
}

// -- the complete stack ------------------------------------------------------

/// One end running compression on top of error control, as a real modem does.
struct FullStack {
    end: End,
    encoder: ec::v42bis::Encoder,
    decoder: ec::v42bis::Decoder,
    plain: Vec<u8>,
}

impl FullStack {
    fn new(role: Role, params: ec::v42bis::Params) -> Self {
        Self {
            end: End::new(role, Params::default()),
            encoder: ec::v42bis::Encoder::new(params),
            decoder: ec::v42bis::Decoder::new(params),
            plain: Vec::new(),
        }
    }

    /// Compress, then hand the result to LAPM.
    fn send(&mut self, data: &[u8]) {
        let mut compressed = Vec::new();
        self.encoder.encode(data, &mut compressed);
        self.encoder.flush(&mut compressed);
        self.end.lapm.send_data(&compressed);
    }

    /// Decompress whatever error control has delivered.
    fn collect(&mut self) {
        if self.end.received.is_empty() {
            return;
        }
        let wire = std::mem::take(&mut self.end.received);
        self.decoder.decode(&wire, &mut self.plain).expect("decompression failed");
    }
}

fn settle_stack(a: &mut FullStack, b: &mut FullStack) {
    settle(&mut a.end, &mut b.end);
    a.collect();
    b.collect();
}

#[test]
fn compression_over_error_control_round_trips() {
    let params = ec::v42bis::Params::default();
    let mut a = FullStack::new(Role::Originator, params);
    let mut b = FullStack::new(Role::Answerer, params);
    a.end.lapm.connect();
    settle_stack(&mut a, &mut b);

    let text: Vec<u8> = b"Welcome to the board. Please log in.\r\n"
        .iter()
        .copied()
        .cycle()
        .take(20_000)
        .collect();
    a.send(&text);
    settle_stack(&mut a, &mut b);
    assert_eq!(b.plain, text);
}

#[test]
fn compression_survives_a_damaged_link() {
    // The whole point of the two layers together: compression cannot tolerate a
    // single lost octet, so error control has to make the link clean first.
    let params = ec::v42bis::Params::default();
    let mut a = FullStack::new(Role::Originator, params);
    let mut b = FullStack::new(Role::Answerer, params);
    a.end.lapm.connect();
    settle_stack(&mut a, &mut b);

    let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog "
        .iter()
        .copied()
        .cycle()
        .take(30_000)
        .collect();
    a.send(&text);

    settle_with(&mut a.end, &mut b.end, |bits, burst| {
        let mut out = bits.to_vec();
        if burst % 4 == 2 && out.len() > 128 {
            let i = out.len() / 2;
            out[i] = !out[i];
        }
        out
    });
    a.collect();
    b.collect();
    assert_eq!(b.plain, text, "a damaged link corrupted the compressed stream");
}

#[test]
fn the_link_carries_less_than_it_delivers() {
    // Compression should mean fewer octets on the wire than the DTE handed over.
    let params = ec::v42bis::Params::default();
    let mut a = FullStack::new(Role::Originator, params);
    let mut b = FullStack::new(Role::Answerer, params);
    a.end.lapm.connect();
    settle_stack(&mut a, &mut b);

    let text: Vec<u8> = b"MAIN MENU\r\n[1] Messages\r\n[2] Files\r\n[3] Doors\r\n"
        .iter()
        .copied()
        .cycle()
        .take(40_000)
        .collect();

    let mut compressed = Vec::new();
    let mut encoder = ec::v42bis::Encoder::new(params);
    encoder.encode(&text, &mut compressed);
    encoder.flush(&mut compressed);

    a.send(&text);
    settle_stack(&mut a, &mut b);
    assert_eq!(b.plain, text);
    assert!(
        compressed.len() < text.len() / 4,
        "{} bytes of menu text compressed to {}",
        text.len(),
        compressed.len()
    );
}

#[test]
fn negotiation_settles_the_parameters_both_ends_use() {
    use ec::xid::{Compression, Xid};

    // One end wants a big dictionary, the other only the minimum.
    let initiator = Xid {
        codewords: Some(4096),
        max_string: Some(32),
        compression: Some(Compression::Both),
        ..Xid::proposal(Compression::Both)
    };
    let responder = Xid::proposal(Compression::Both);
    let agreed = initiator.resolve(&responder);
    let params = agreed.v42bis_params().expect("compression should be on");

    // Both ends must build the same dictionary from the settled values.
    let mut a = FullStack::new(Role::Originator, params);
    let mut b = FullStack::new(Role::Answerer, params);
    a.end.lapm.connect();
    settle_stack(&mut a, &mut b);

    let text: Vec<u8> = b"negotiated parameters ".iter().copied().cycle().take(12_000).collect();
    a.send(&text);
    settle_stack(&mut a, &mut b);
    assert_eq!(b.plain, text);
    // V.42bis 6.4: "the lower value shall be selected and assigned to N2 in
    // both DCEs". Which here is this end's own proposal, not the minimum --
    // and that is the point of proposing something above the minimum.
    assert_eq!(params.n2, ec::v42bis::OFFERED_N2, "the lower value should win");
    assert_eq!(params.n7, 32, "and for the string length too");
}

#[test]
fn a_parameter_nobody_sent_is_the_one_its_recommendation_gives() {
    use ec::xid::{Compression, Xid};

    // The case that only bites once this end proposes something other than the
    // default. A far end that sends no P1 has not left the choice open: V.42bis
    // 6.4 gives P1 "a default value of 512, which is its minimum value", and
    // that is what the far end is using. An end that read the silence as
    // agreement would build a dictionary of 2048 entries against one of 512,
    // and every codeword above 512 would decode to something else entirely.
    let ours = Xid::proposal(Compression::Both);
    let silent = Xid { compression: Some(Compression::Both), ..Xid::default() };
    let params = ours.resolve(&silent).v42bis_params().expect("compression is on");
    assert_eq!(params.n2, ec::v42bis::DEFAULT_N2, "512 is what silence means");
    assert_eq!(params.n7, ec::v42bis::DEFAULT_N7);

    // And the same for the parameters of the link underneath it.
    let agreed = ours.resolve(&silent);
    assert_eq!(agreed.n401_transmit, Some(ec::lapm::DEFAULT_N401 as u16));
    assert_eq!(agreed.window_transmit, Some(ec::lapm::DEFAULT_K));
}

// ---------------------------------------------------------------------------
// A far end whose answering pattern is not the one in Table 3.
//
// V.42 Appendix VI.1 records two patterns that real modems send *before* the
// `EC` that says what they support: `EM` from a cellular protocol, five or
// more times, and `EP` sixteen times to say the XID user data subfield may
// carry V.44. Both mean V.42 is supported. A detector that acts on the first
// pattern it sees answers either of them by declining error control to a modem
// that has it, and the connection that results is unprotected for no reason.

/// One start-stop character, low-order bit first, then the fill ones.
fn character(out: &mut Vec<bool>, value: u8) {
    out.push(false);
    for i in 0..8 {
        out.push(value & (1 << i) != 0);
    }
    out.push(true);
    out.extend(std::iter::repeat_n(true, 12));
}

/// Run a stack originator against a hand-made answering pattern.
fn against_pattern(seconds: &[(u8, usize)]) -> ec::stack::Phase {
    use ec::detect::ADP_E;
    let mut bits = Vec::new();
    for &(second, times) in seconds {
        for _ in 0..times {
            character(&mut bits, ADP_E);
            character(&mut bits, second);
        }
    }
    let mut stack = ec::Stack::new(Role::Originator, Params::default());
    for bit in bits {
        stack.next_bit();
        stack.feed_bit(bit);
    }
    stack.phase()
}

#[test]
fn a_cellular_far_end_gets_error_control_through_the_stack() {
    use ec::detect::{ADP_C, ADP_M};
    assert_eq!(
        against_pattern(&[(ADP_M, 5), (ADP_C, 10)]),
        ec::stack::Phase::Negotiating,
        "EM then EC is a modem that does V.42"
    );
}

#[test]
fn a_v44_capable_far_end_gets_error_control_through_the_stack() {
    use ec::detect::{ADP_C, ADP_P};
    assert_eq!(
        against_pattern(&[(ADP_P, 16), (ADP_C, 10)]),
        ec::stack::Phase::Negotiating,
        "EP sixteen times then EC is a modem that does V.42"
    );
}

#[test]
fn a_far_end_that_declines_is_still_taken_at_its_word() {
    use ec::detect::ADP_NULL;
    assert_eq!(
        against_pattern(&[(ADP_NULL, 4)]),
        ec::stack::Phase::Transparent,
        "E NUL means no error control, and listening past it would hang"
    );
}

// ---------------------------------------------------------------------------
// What V.8 already knew.
//
// V.8 Table 6 lets both ends name LAPM at 300 bit/s, before a data carrier
// exists. V.42's detection phase then asks the same question again over a line
// that has just been trained, and its answer is ten patterns of start-stop
// characters that a bad line can eat entirely. Without the earlier answer, a
// silence there is indistinguishable from a far end that does no error control
// at all, and the safe reading is the second one.

/// Drive a stack through a detection phase in which nothing comes back.
fn through_silence(stack: &mut ec::Stack) {
    for _ in 0..40_000 {
        stack.next_bit();
        stack.feed_bit(true);
    }
    stack.tick(ec::detect::DEFAULT_T400_MS);
}

#[test]
fn a_far_end_that_named_lapm_in_v8_is_believed_through_a_silence() {
    let mut stack = ec::Stack::new(Role::Originator, Params::default()).declared_lapm();
    through_silence(&mut stack);
    assert_eq!(
        stack.phase(),
        ec::stack::Phase::Negotiating,
        "V.8 said LAPM; a lost ADP does not unsay it"
    );
}

#[test]
fn a_silence_on_its_own_is_still_no_error_control() {
    let mut stack = ec::Stack::new(Role::Originator, Params::default());
    through_silence(&mut stack);
    assert_eq!(stack.phase(), ec::stack::Phase::Transparent);
}

#[test]
fn a_refusal_beats_what_v8_said() {
    // A far end that names LAPM in V.8 and then sends E NUL has changed its
    // mind, or was never asking about the same thing. Either way the later and
    // more specific statement is the one to act on: V.42 Table 3's `E` and
    // NUL is "no error-correcting protocol desired", which is not a silence to
    // be read around.
    use ec::detect::ADP_NULL;
    let mut bits = Vec::new();
    for _ in 0..4 {
        character(&mut bits, ec::detect::ADP_E);
        character(&mut bits, ADP_NULL);
    }
    let mut stack = ec::Stack::new(Role::Originator, Params::default()).declared_lapm();
    for bit in bits {
        stack.next_bit();
        stack.feed_bit(bit);
    }
    assert_eq!(stack.phase(), ec::stack::Phase::Transparent);
}

/// What the negotiated parameters are worth, on the sort of text a board sends.
///
/// Not an assertion about a number, which would only pin whatever this happens
/// to do today. It is here to be read: `cargo test -p ec -- --ignored
/// --nocapture report_compression`.
#[test]
#[ignore = "reports rather than asserts"]
fn report_compression() {
    let text: Vec<u8> = b"MAIN MENU\r\n[1] Messages\r\n[2] Files\r\n[3] Doors\r\n\
                          \x1b[1;36m--- Synchronet BBS ---\x1b[0m\r\n"
        .iter()
        .copied()
        .cycle()
        .take(60_000)
        .collect();

    println!("\n  N2    N7   octets  ratio");
    for (n2, n7) in [
        (ec::v42bis::DEFAULT_N2, ec::v42bis::DEFAULT_N7),
        (1024, 32),
        (ec::v42bis::OFFERED_N2, ec::v42bis::OFFERED_N7),
        (4096, 250),
    ] {
        let params = ec::v42bis::Params { n2, n7 };
        let mut out = Vec::new();
        let mut encoder = ec::v42bis::Encoder::new(params);
        encoder.encode(&text, &mut out);
        encoder.flush(&mut out);
        println!(
            "{n2:6} {n7:5} {:8} {:6.2}:1",
            out.len(),
            text.len() as f64 / out.len() as f64
        );
    }
    println!();
}

#[test]
fn the_protocol_phase_opens_with_sixteen_flags() {
    // V.42 8.10.2, Note: the first protocol frame after the detection phase is
    // preceded by "flag patterns for a period of time sufficient to guarantee
    // the transmission of at least 16-flag patterns".
    //
    // The reason is at the other end. The answerer is still in its detection
    // phase when this end leaves, sending its pattern until flags say the
    // protocol phase has begun (7.2.1.3) -- so the flags are not padding, they
    // are the message, and a frame sent before them is a frame sent into a
    // detector.
    use ec::detect::{ADP_C, ADP_E};
    let mut bits = Vec::new();
    for _ in 0..4 {
        character(&mut bits, ADP_E);
        character(&mut bits, ADP_C);
    }
    let mut stack = ec::Stack::new(Role::Originator, Params::default());
    let mut sent = Vec::new();
    let mut opened = None;
    for bit in bits {
        sent.push(stack.next_bit());
        stack.feed_bit(bit);
        if opened.is_none() && stack.phase() == ec::stack::Phase::Negotiating {
            opened = Some(sent.len());
        }
    }
    let start = opened.expect("the detection phase never finished");
    while sent.len() < start + 16 * 8 {
        sent.push(stack.next_bit());
    }

    // Checked without knowing where in a flag the stream begins, because the
    // phase changed part-way through a bit and nothing here is aligned to it.
    // A run of flags is periodic with a period of eight carrying two zeros, so
    // any window of sixteen periods holds thirty-two zeros wherever it starts,
    // and never more than six ones together.
    let sent = &sent[start..start + 16 * 8];
    assert_eq!(
        sent.iter().filter(|b| !**b).count(),
        32,
        "sixteen flags carry thirty-two zeros, at any alignment"
    );
    let longest = sent
        .split(|b| !*b)
        .map(<[bool]>::len)
        .max()
        .unwrap_or(0);
    assert!(longest <= 6, "a run of {longest} ones is not flags");
}

#[test]
fn every_repeated_xid_command_is_answered() {
    // V.42 8.10.3: a far end that hears no response "shall retransmit the XID
    // command as above" up to N400 times. An end that answers only the first
    // leaves it retransmitting into silence -- and Appendix III.3 says what a
    // far end should do when the exchange fails, which is release the call.
    //
    // Only commands, though. Answering a response would go round for ever, and
    // both ends here open with a command.
    use ec::frame::{Address, Kind};
    use ec::xid::{Compression, Xid};

    let mut stack = ec::Stack::new(Role::Answerer, Params::default());
    stack.offer_compression(Compression::Both);
    // Into the protocol phase: the answerer needs the originator's pattern.
    let mut odp = Vec::new();
    for _ in 0..8 {
        character(&mut odp, ec::detect::ODP_EVEN);
        character(&mut odp, ec::detect::ODP_ODD);
    }
    for bit in &odp {
        stack.next_bit();
        stack.feed_bit(*bit);
    }
    // The pattern is sent for at least ten repetitions and then until the
    // clock says the originator is not coming (7.2.1.3, III.1), so the phase
    // does not change until both the timer has run and the last repetition is
    // off the queue.
    for _ in 0..40_000 {
        stack.next_bit();
    }
    stack.tick(ec::detect::DEFAULT_T400_MS);
    for _ in 0..4096 {
        if stack.phase() != ec::stack::Phase::Detecting {
            break;
        }
        stack.next_bit();
        // Fed as well as drained: what re-examines the detection phase is a
        // bit arriving or the clock, and the clock has already run.
        stack.feed_bit(true);
    }
    assert_eq!(stack.phase(), ec::stack::Phase::Negotiating, "never left detection");

    let command = Frame::Xid { pf: true, info: Xid::proposal(Compression::Both).encode() }
        .encode(DLCI_DATA, Role::Originator, Kind::Command);
    let mut encoder = Encoder::new(Fcs::Bits16);
    let mut answers = 0;
    for round in 0..3 {
        encoder.frame(&command);
        let mut decoder = Decoder::new(Fcs::Bits16);
        while let Some(bit) = encoder.next_bit() {
            // Read what comes back while feeding, since the reply is queued
            // against the same encoder the next bit is drawn from.
            if let Some(Ok(body)) = decoder.feed(stack.next_bit()) {
                // Responses only. This end sends XID *commands* of its own
                // while it is negotiating, and counting those would make the
                // test pass on a stack that never replied at all.
                if let Ok((Address { kind: Kind::Response, .. }, Frame::Xid { .. })) =
                    Frame::decode(&body, Role::Originator)
                {
                    answers += 1;
                }
            }
            stack.feed_bit(bit);
        }
        // Drain the reply, which is queued behind whatever was already going.
        for _ in 0..4096 {
            if let Some(Ok(body)) = decoder.feed(stack.next_bit())
                && let Ok((Address { kind: Kind::Response, .. }, Frame::Xid { .. })) =
                    Frame::decode(&body, Role::Originator)
            {
                answers += 1;
            }
        }
        assert!(answers > round, "command {} went unanswered", round + 1);
    }
}
