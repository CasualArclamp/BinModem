//! Dialling a V.90 server: the modem as a terminal drives it, and a server
//! built from the pieces a digital modem is made of -- V.8's answer, V.90's
//! digital start-up and V.42 -- at the far end of a simulated network.

use datapump::v8 as v8line;
use datapump::v34::info::{Info0, Info0d};
use datapump::v90::network::Network;
use datapump::v90::startup::{Digital, Status};
use datapump::v90::ucode::Law;
use ec::stack::Stack;
use ec::{Params, Role as EcRole};
use modem::{Modem, State};
use v8::{Access, CallFunction, Modulation, Modulations, Pcm, PcmRole};

const FS: f64 = 16_000.0;
const NETWORK_FS: f64 = 8000.0;

/// A V.90 server on the network's side of the call.
struct Server {
    v8: Option<v8line::Modem>,
    startup: Option<Digital>,
    ec: Option<Stack>,
    received: Vec<u8>,
    ticks: u64,
}

impl Server {
    fn new() -> Self {
        let v8 = v8line::Modem::new(
            v8line::Role::Answering,
            CallFunction::Data,
            Modulations::of(&[Modulation::V34Duplex, Modulation::V32bis]),
            NETWORK_FS,
        )
        .offering_lapm()
        .offering_pcm_on(Pcm { digital: true, ..Pcm::default() }, Access { digital: true, ..Access::default() });
        Self { v8: Some(v8), startup: None, ec: None, received: Vec::new(), ticks: 0 }
    }

    fn info0d() -> Info0d {
        Info0d {
            v34: Info0 { constellation_1664: true, ..Info0::default() },
            nominal_power: 4,
            max_power: 23,
            power_at_codec: true,
            a_law: false,
            upstream_3429: false,
        }
    }

    fn step(&mut self, input: f64) -> f64 {
        self.ticks += 1;
        if let Some(v8) = self.v8.as_mut() {
            // V.8 at the network's rate, at a sensible level.
            let out = 0.3 * v8.step(input);
            match v8.status() {
                v8line::Status::Negotiating => {}
                v8line::Status::Agreed(_) => {
                    assert_eq!(v8.pcm_role(), Some(PcmRole::Digital), "V.8 did not settle on V.90");
                    self.v8 = None;
                    self.startup = Some(Digital::new(Self::info0d()));
                }
                other => panic!("V.8 came to {other:?}"),
            }
            return out;
        }
        let startup = self.startup.as_mut().expect("V.8 is over");
        let out = startup.step(input);
        if let Status::Connected { transmit, receive } = startup.status() {
            let stack = self.ec.get_or_insert_with(|| {
                let params = Params { t401_ms: ec::lapm::t401_for_line(transmit.min(receive), 60), ..Params::default() };
                Stack::new(EcRole::Answerer, params).over_a_round_trip(60)
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

struct Call {
    net: Network,
    caller: Modem,
    server: Server,
    up: Vec<f64>,
    said: Vec<u8>,
}

impl Call {
    fn new() -> Self {
        Self {
            net: Network::new(Law::Mu, FS).with_delay(0.015, FS).with_noise(1e-5),
            caller: Modem::new(FS),
            server: Server::new(),
            up: Vec::new(),
            said: Vec::new(),
        }
    }

    fn type_at(&mut self, line: &str) {
        for b in line.bytes() {
            self.caller.feed_dte(b);
        }
        self.caller.feed_dte(b'\r');
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * NETWORK_FS) as usize {
            let to_server = self.net.up(&self.up);
            self.up.clear();
            let from_server = self.server.step(to_server);
            for x in self.net.down(from_server) {
                self.up.push(self.caller.step(x));
                self.said.extend(self.caller.take_dte());
            }
        }
    }

    fn saw(&self) -> String {
        String::from_utf8_lossy(&self.said).into_owned()
    }
}

#[test]
fn dialling_a_v90_server_connects_at_pcm_rates_and_carries_text() {
    let mut call = Call::new();
    call.type_at("AT+MS=V90");
    call.run(0.05);
    assert!(call.saw().contains("OK"), "{:?}", call.saw());
    call.type_at("ATD5551234");
    call.run(20.0);
    println!("terminal saw {:?}, standard {}, phase {}", call.saw(), call.caller.standard(), call.caller.line_phase());
    assert_eq!(call.caller.state(), State::Data, "not online: {:?}", call.saw());
    assert_eq!(call.caller.standard(), "V.90");
    let down = call.caller.rate().expect("no rate");
    let up = call.caller.transmit_rate().expect("no sending rate");
    assert!(down >= 48_000, "downstream {down}");
    assert!((24_000..=33_600).contains(&up), "upstream {up}");
    assert!(call.saw().contains(&format!("CONNECT {down}")), "{:?}", call.saw());

    // Text from the terminal reaches the server, through V.42 over V.90.
    call.run(3.0);
    for b in b"hello over fifty-six thousand" {
        call.caller.feed_dte(*b);
    }
    call.run(3.0);
    let got = String::from_utf8_lossy(&call.server.received).into_owned();
    assert!(got.contains("hello over fifty-six thousand"), "the server got {got:?}");
    // And back down.
    if let Some(stack) = call.server.ec.as_mut() {
        stack.send(b"and back again");
    }
    call.run(3.0);
    assert!(call.saw().contains("and back again"), "the terminal saw {:?}", call.saw());

    // A server that retrains (9.5.1) takes the call back through phase 2 and
    // up again, and V.42 carries on over it.
    assert!(call.server.startup.as_mut().unwrap().retrain());
    call.run(0.5);
    assert!(call.caller.retraining(), "the caller never saw the retrain");
    call.run(15.0);
    assert!(!call.caller.retraining(), "still retraining, phase {}", call.caller.line_phase());
    assert_eq!(call.caller.state(), State::Data, "{:?}", call.saw());
    assert_eq!(call.caller.standard(), "V.90");
    for b in b"after the retrain" {
        call.caller.feed_dte(*b);
    }
    call.run(3.0);
    let got = String::from_utf8_lossy(&call.server.received).into_owned();
    assert!(got.contains("after the retrain"), "the server got {got:?}");
}
