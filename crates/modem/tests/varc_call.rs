//! A V.ARC call between two modems, typed at as a terminal would.
//!
//! The modulation has its own tests. What is tested here is that it is a
//! *modem*: that `AT+MS=VARC` and `ATD` place a call, that the terminal is
//! told `CONNECT` with a rate, that what is typed at one end comes out of
//! the other, and that `+++` and `ATH` end it.

use modem::{Modem, State, VarcMode};

const FS: f64 = 48_000.0;

fn type_at(modem: &mut Modem, text: &str) {
    for b in text.bytes() {
        modem.feed_dte(b);
    }
}

/// Two modems on a stereo lead each way: what one sends on a channel the
/// other hears on the same channel.
struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: [f64; 2],
    from_host: [f64; 2],
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
    /// Channels the lead really has wired.
    wired: usize,
}

impl Pair {
    fn new(carrier: &str) -> Self {
        let mut caller = Modem::new(16_000.0);
        let mut host = Modem::new(16_000.0);
        type_at(&mut caller, &format!("AT+MS={carrier}\r"));
        type_at(&mut host, &format!("AT+MS={carrier}\r"));
        Self {
            caller,
            host,
            from_caller: [0.0; 2],
            from_host: [0.0; 2],
            at_caller: Vec::new(),
            at_host: Vec::new(),
            wired: 2,
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let mut to_host = self.from_caller;
            let mut to_caller = self.from_host;
            for c in self.wired..2 {
                to_host[c] = 0.0;
                to_caller[c] = 0.0;
            }
            self.caller.varc_step(FS, &to_caller, &mut self.from_caller);
            self.host.varc_step(FS, &to_host, &mut self.from_host);
            self.at_caller.extend(self.caller.take_dte());
            self.at_host.extend(self.host.take_dte());
        }
    }

    fn said(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[test]
fn a_call_is_placed_answered_used_and_put_down() {
    let mut pair = Pair::new("VARC");
    assert_eq!(pair.caller.standard(), "V.ARC/DMT");
    assert_eq!(pair.caller.varc_wanted(), None);
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD5551234\r");
    assert_eq!(pair.caller.varc_wanted(), Some(VarcMode::Dmt));
    assert_eq!(pair.caller.state(), State::Handshaking);
    assert!(pair.caller.off_hook());

    pair.run(8.0);
    assert_eq!(pair.caller.state(), State::Data, "{}", Pair::said(&pair.at_caller));
    assert_eq!(pair.host.state(), State::Data, "{}", Pair::said(&pair.at_host));
    let told = Pair::said(&pair.at_caller);
    assert!(told.contains("CONNECT ") && told.contains("/VARC"), "{told}");
    // Two clean channels at 48 kHz: several hundred kilobits each.
    let rate = pair.caller.rate().expect("a rate once connected");
    assert!(rate > 500_000, "{rate} bit/s");
    assert_eq!(pair.host.transmit_rate(), Some(rate));
    assert!(pair.caller.carrier());
    assert!(pair.caller.error_controlled());

    pair.at_caller.clear();
    pair.at_host.clear();
    let page: Vec<u8> = (0..60_000u32).map(|i| (i * 31 + (i >> 7)) as u8).collect();
    for &b in &page {
        pair.caller.feed_dte(b);
    }
    type_at(&mut pair.host, "hello from the far end\r\n");
    pair.run(3.0);
    assert!(pair.at_host == page, "{} of {} octets", pair.at_host.len(), page.len());
    assert_eq!(Pair::said(&pair.at_caller), "hello from the far end\r\n");

    // A second of quiet, the escape, a second of quiet, and the line is
    // put down from command state.
    pair.run(1.2);
    type_at(&mut pair.caller, "+++");
    pair.run(1.2);
    assert_eq!(pair.caller.state(), State::OnlineCommand);
    type_at(&mut pair.caller, "ATH\r");
    assert_eq!(pair.caller.state(), State::Command);
    assert_eq!(pair.caller.varc_wanted(), None);
    assert!(!pair.caller.off_hook());

    // The other end hears nothing more and says so.
    pair.at_host.clear();
    pair.run(7.0);
    assert_eq!(pair.host.state(), State::Command);
    assert!(Pair::said(&pair.at_host).contains("NO CARRIER"), "{}", Pair::said(&pair.at_host));
}

#[test]
fn the_single_carrier_modes_connect_too() {
    for (carrier, name, least) in [("VARCQ", "V.ARC/QAM", 300_000), ("VARCP", "V.ARC/PCM", 150_000)] {
        let mut pair = Pair::new(carrier);
        type_at(&mut pair.host, "ATA\r");
        type_at(&mut pair.caller, "ATD1\r");
        pair.run(9.0);
        assert_eq!(pair.caller.standard(), name);
        assert_eq!(pair.caller.state(), State::Data, "{name}: {}", Pair::said(&pair.at_caller));
        let rate = pair.caller.rate().unwrap_or(0);
        assert!(rate > least, "{name}: {rate} bit/s");
        pair.at_host.clear();
        type_at(&mut pair.caller, "the quick brown fox");
        pair.run(1.0);
        assert_eq!(Pair::said(&pair.at_host), "the quick brown fox", "{name}");
    }
}

#[test]
fn a_lead_with_one_channel_wired_still_connects() {
    let mut pair = Pair::new("VARC");
    pair.wired = 1;
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD1\r");
    pair.run(14.0);
    assert_eq!(pair.caller.state(), State::Data, "{}", Pair::said(&pair.at_caller));
    pair.at_host.clear();
    let page: Vec<u8> = (0..20_000u32).map(|i| (i * 7 + (i >> 5)) as u8).collect();
    for &b in &page {
        pair.caller.feed_dte(b);
    }
    pair.run(3.0);
    assert!(pair.at_host == page, "{} of {} octets", pair.at_host.len(), page.len());
}

#[test]
fn a_lead_that_comes_straight_back_is_a_call_to_oneself() {
    let mut modem = Modem::new(16_000.0);
    type_at(&mut modem, "AT+MS=VARC\r");
    type_at(&mut modem, "ATDLOOP\r");
    let mut wire = [0.0; 2];
    let mut said = Vec::new();
    let step = |modem: &mut Modem, wire: &mut [f64; 2], said: &mut Vec<u8>, seconds: f64| {
        for _ in 0..(seconds * FS) as usize {
            let heard = *wire;
            modem.varc_step(FS, &heard, wire);
            said.extend(modem.take_dte());
        }
    };
    step(&mut modem, &mut wire, &mut said, 8.0);
    assert_eq!(modem.state(), State::Data, "{}", Pair::said(&said));
    said.clear();
    type_at(&mut modem, "round and back again");
    step(&mut modem, &mut wire, &mut said, 1.0);
    assert_eq!(Pair::said(&said), "round and back again");
}

#[test]
fn nobody_there_is_no_answer_and_an_ordinary_step_is_silence_meanwhile() {
    let mut modem = Modem::new(16_000.0);
    type_at(&mut modem, "AT+MS=VARC\r");
    type_at(&mut modem, "ATD1\r");
    let mut said = modem.take_dte();
    assert_eq!(modem.step(0.3), 0.0);
    let mut out = [0.0; 2];
    // A sound card at 8 kHz, to get through a minute quickly.
    for _ in 0..8_000 * 61 {
        modem.varc_step(8_000.0, &[0.0, 0.0], &mut out);
        said.extend(modem.take_dte());
    }
    assert_eq!(modem.state(), State::Command);
    assert!(Pair::said(&said).contains("NO ANSWER"), "{}", Pair::said(&said));
    assert_eq!(modem.varc_wanted(), None);
}
