//! Two of these modems with `AT+MS=V92`, one dialling and one answering, on a
//! four-wire line at 48 kHz: what two instances on a pair of virtual cables
//! are. V.8, phase 2 with V.92's INFO sequences, a Table 18 INFO1a, V.92's
//! Phases 3 and 4 at both ends, and data -- PCM going up as well as down.

use std::collections::VecDeque;

use modem::{Modem, State};

const FS: f64 = 48_000.0;

/// Both ends and the two wires between them.
struct Pair {
    caller: Modem,
    host: Modem,
    to_host: VecDeque<f64>,
    to_caller: VecDeque<f64>,
    caller_said: Vec<u8>,
    host_said: Vec<u8>,
}

impl Pair {
    /// `up` and `down` line samples each way.
    fn new(up: usize, down: usize) -> Self {
        Self {
            caller: Modem::new(FS),
            host: Modem::new(FS),
            to_host: VecDeque::from(vec![0.0; up.max(1)]),
            to_caller: VecDeque::from(vec![0.0; down.max(1)]),
            caller_said: Vec::new(),
            host_said: Vec::new(),
        }
    }

    fn type_at(modem: &mut Modem, line: &str) {
        for b in line.bytes() {
            modem.feed_dte(b);
        }
        modem.feed_dte(b'\r');
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let heard = self.to_caller.pop_front().unwrap_or(0.0);
            self.to_host.push_back(self.caller.step(heard));
            self.caller_said.extend(self.caller.take_dte());
            let heard = self.to_host.pop_front().unwrap_or(0.0);
            self.to_caller.push_back(self.host.step(heard));
            self.host_said.extend(self.host.take_dte());
        }
    }

    fn notes(&mut self) -> String {
        let mut notes = String::new();
        for (who, m) in [("caller", &mut self.caller), ("host", &mut self.host)] {
            for line in m.take_line_notes() {
                notes.push_str(&format!("{who}: {line}\n"));
            }
        }
        notes
    }
}

/// The whole call, at a few different delays each way: both ends in data
/// mode on V.92, PCM going up, and text crossing in both directions.
#[test]
fn two_of_these_connect_on_v92_with_pcm_upstream_and_carry_text() {
    for (up, down) in [(480, 480), (1203, 917), (2401, 61)] {
        let mut pair = Pair::new(up, down);
        pair.caller.set_pcm_upstream(true);
        pair.host.set_pcm_upstream(true);
        Pair::type_at(&mut pair.host, "AT+MS=V92");
        Pair::type_at(&mut pair.caller, "AT+MS=V92");
        pair.run(0.05);
        Pair::type_at(&mut pair.host, "ATA");
        Pair::type_at(&mut pair.caller, "ATD5551234");
        let mut notes = String::new();
        for _ in 0..40 {
            pair.run(1.0);
            notes.push_str(&pair.notes());
            if pair.caller.state() == State::Data && pair.host.state() == State::Data {
                break;
            }
        }
        let saw = String::from_utf8_lossy(&pair.caller_said).into_owned();
        assert_eq!(
            (pair.caller.state(), pair.host.state()),
            (State::Data, State::Data),
            "{up}/{down}: {saw:?}\n{notes}"
        );
        assert_eq!(pair.caller.standard(), "V.92", "{up}/{down}:\n{notes}");
        assert_eq!(pair.host.standard(), "V.92", "{up}/{down}:\n{notes}");
        let (down_rate, up_rate) = (pair.caller.rate().unwrap_or(0), pair.caller.transmit_rate().unwrap_or(0));
        println!("{up}/{down}: {down_rate} down, {up_rate} up: {saw:?}");
        assert!(down_rate >= 40_000 && up_rate >= 24_000, "{up}/{down}: {down_rate}/{up_rate}\n{notes}");

        pair.run(2.0);
        for b in b"from the caller" {
            pair.caller.feed_dte(*b);
        }
        for b in b"from the server" {
            pair.host.feed_dte(*b);
        }
        pair.run(3.0);
        notes.push_str(&pair.notes());
        let host_saw = String::from_utf8_lossy(&pair.host_said).into_owned();
        assert!(host_saw.contains("from the caller"), "{up}/{down}: the server saw {host_saw:?}\n{notes}");
        let caller_saw = String::from_utf8_lossy(&pair.caller_said).into_owned();
        assert!(caller_saw.contains("from the server"), "{up}/{down}: the caller saw {caller_saw:?}\n{notes}");
    }
}
