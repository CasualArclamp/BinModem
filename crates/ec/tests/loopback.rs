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
    assert_eq!(params.n2, ec::v42bis::DEFAULT_N2, "the lower value should win");
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
