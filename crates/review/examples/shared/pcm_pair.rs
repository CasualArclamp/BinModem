//! A V.90 call between one of these modems and a digital modem on the far
//! side of a telephone network, recorded with an end a channel. Shared by
//! `make_call` and the test that reads such a call back.

use modem::{Modem, State};

const FS: f64 = 16_000.0;

fn type_at(modem: &mut Modem, line: &str) {
    for b in line.bytes() {
        modem.feed_dte(b);
    }
    modem.feed_dte(b'\r');
}

/// A V.90 server on the network's side of a call: V.8, then the digital
/// modem, then V.42 over it. The same one `modem/tests/v90_call.rs` dials.
struct Server {
    v8: Option<datapump::v8::Modem>,
    startup: Option<datapump::v90::startup::Digital>,
    ec: Option<ec::stack::Stack>,
    received: Vec<u8>,
    ticks: u64,
}

impl Server {
    fn new() -> Self {
        use v8::{Access, CallFunction, Modulation, Modulations, Pcm};
        let v8 = datapump::v8::Modem::new(
            datapump::v8::Role::Answering,
            CallFunction::Data,
            Modulations::of(&[Modulation::V34Duplex, Modulation::V32bis]),
            8000.0,
        )
        .offering_lapm()
        .offering_pcm_on(Pcm { digital: true, ..Pcm::default() }, Access { digital: true, ..Access::default() });
        Self { v8: Some(v8), startup: None, ec: None, received: Vec::new(), ticks: 0 }
    }

    fn step(&mut self, input: f64) -> f64 {
        use datapump::v34::info::{Info0, Info0d};
        use datapump::v90::startup::{Digital, Status};
        self.ticks += 1;
        if let Some(v8) = self.v8.as_mut() {
            let out = 0.3 * v8.step(input);
            if let datapump::v8::Status::Agreed(_) = v8.status() {
                self.v8 = None;
                let info0d = Info0d {
                    v34: Info0 { constellation_1664: true, ..Info0::default() },
                    nominal_power: 4,
                    max_power: 23,
                    power_at_codec: true,
                    a_law: false,
                    upstream_3429: false,
                };
                self.startup = Some(Digital::new(info0d, 0.0));
            }
            return out;
        }
        let Some(startup) = self.startup.as_mut() else { return 0.0 };
        let out = startup.step(input);
        if let Status::Connected { transmit, receive } = startup.status() {
            let stack = self.ec.get_or_insert_with(|| {
                let params =
                    ec::Params { t401_ms: ec::lapm::t401_for_line(transmit.min(receive), 60), ..ec::Params::default() };
                ec::stack::Stack::new(ec::Role::Answerer, params).over_a_round_trip(60)
            });
            for bit in startup.take_bits() {
                stack.feed_bit(bit);
            }
            while startup.pending_bits() < 256 {
                let bits: Vec<bool> = (0..64).map(|_| stack.next_bit()).collect();
                startup.send_bits(&bits);
            }
            if self.ticks.is_multiple_of(8) {
                stack.tick(1);
            }
            self.received.extend(stack.take_received());
        }
        out
    }
}

/// What a call left behind.
pub struct Recorded {
    /// Two channels at 8000 samples a second: what the network made of the
    /// analogue modem's signal, and the digital modem's own levels.
    pub samples: Vec<f32>,
    pub server_received: Vec<u8>,
    pub caller_saw: Vec<u8>,
    /// The rates the analogue modem itself reported, down and up.
    pub rates: Option<(u32, u32)>,
}

/// A V.90 call of `seconds`: the caller the analogue modem, the answerer a
/// digital one on the far side of a telephone network, which is a codec and
/// a line. The recording is at the network's rate, as a capture of the two
/// RTP streams of such a call is. Each end sends a line of text once the
/// call is up.
pub fn record(seconds: f64, say: bool) -> Recorded {
    use datapump::v90::network::Network;
    use datapump::v90::ucode::Law;

    let mut net = Network::new(Law::Mu, FS).with_delay(0.015, FS).with_noise(1e-5);
    let mut caller = Modem::new(FS);
    let mut server = Server::new();
    type_at(&mut caller, "AT+MS=V90");
    // Compression off, so that what was typed is in the frames as typed.
    type_at(&mut caller, "AT+DS=0;+DS44=0");
    caller.take_dte();
    type_at(&mut caller, "ATD5551234");

    let mut samples = Vec::new();
    let mut up: Vec<f64> = Vec::new();
    let mut caller_saw = Vec::new();
    let (mut up_at, mut spoken, mut answered, mut rates) = (None, false, false, None);
    for n in 0..(seconds * 8000.0) as usize {
        let now = n as f64 / 8000.0;
        let to_server = net.up(&up);
        up.clear();
        let from_server = server.step(to_server);
        samples.push(to_server as f32);
        samples.push(from_server as f32);
        for x in net.down(from_server) {
            up.push(caller.step(x));
        }
        caller_saw.extend(caller.take_dte());
        if up_at.is_none() && caller.state() == State::Data {
            up_at = Some(now);
            rates = caller.rate().zip(caller.transmit_rate());
        }
        if let Some(at) = up_at.filter(|_| say) {
            if !spoken && now > at + 1.0 {
                spoken = true;
                for b in b"Hello from the analogue modem.\r\n" {
                    caller.feed_dte(*b);
                }
            }
            if !answered && now > at + 1.5 {
                answered = true;
                if let Some(stack) = server.ec.as_mut() {
                    stack.send(b"And hello back from the server.\r\n");
                }
            }
        }
    }
    Recorded { samples, server_received: server.received, caller_saw, rates }
}
