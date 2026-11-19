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

/// The symbol diagram has something to draw in every mode that has symbols,
/// and what it is given fits the box it is drawn in.
#[test]
fn every_mode_with_symbols_gives_the_symbol_diagram_its_points() {
    for (carrier, shape) in [("VARC", "QAM"), ("VARCQ", "QAM"), ("VARCP", "PCM")] {
        let mut pair = Pair::new(carrier);
        assert_eq!(pair.caller.shape(), "-", "{carrier}: before the call");
        type_at(&mut pair.host, "ATA\r");
        type_at(&mut pair.caller, "ATD1\r");
        pair.run(9.0);
        assert_eq!(pair.caller.state(), State::Data, "{carrier}");
        // Whatever gathered while the tables were being settled.
        pair.caller.varc_points();
        pair.run(1.0);
        let points = pair.caller.varc_points();
        assert!(points.len() > 500, "{carrier}: {} points", points.len());
        assert_eq!(pair.caller.shape(), shape, "{carrier}");
        let states = pair.caller.states();
        assert!(states >= 16, "{carrier}: {states} points in the grid");
        // A clean line: every point within its grid, and the grid filled
        // out to its edge on the wider axis.
        let reach = points.iter().map(|p| p.0.abs().max(p.1.abs())).fold(0.0, f64::max);
        assert!((0.9..1.05).contains(&reach), "{carrier}: reaches {reach}");
        // Taken means taken.
        assert!(pair.caller.varc_points().len() < points.len(), "{carrier}");
    }
}

#[test]
fn a_call_of_exact_samples_has_no_symbols_to_draw() {
    let (mut caller, _, _, _) = on_a_linear_call(1.0, 3.0);
    assert!(caller.varc_points().is_empty());
    assert_eq!(caller.shape(), "-");
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

/// Two modems on a SIP call that carries linear samples: one channel at
/// 16 kHz, every sample a sixteen-bit integer, and a gain on the way if the
/// call is not as exact as it said.
fn on_a_linear_call(gain: f64, seconds: f64) -> (Modem, Modem, String, String) {
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARC\r");
    type_at(&mut host, "AT+MS=VARC\rATA\r");
    type_at(&mut caller, "ATD100\r");
    caller.varc_exact_line(true);
    host.varc_exact_line(true);
    let carry = |x: f64| ((x * 32768.0 * gain).round() / 32768.0).clamp(-1.0, 1.0);
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let mut typed = false;
    for _ in 0..(seconds * 16_000.0) as usize {
        let (to_host, to_caller) = ([carry(from_caller[0])], [carry(from_host[0])]);
        caller.varc_step(16_000.0, &to_caller, &mut from_caller);
        host.varc_step(16_000.0, &to_host, &mut from_host);
        at_caller.extend(caller.take_dte());
        at_host.extend(host.take_dte());
        if !typed && caller.state() == State::Data && host.state() == State::Data {
            typed = true;
            at_host.clear();
            type_at(&mut caller, "samples that are the data");
        }
    }
    (caller, host, Pair::said(&at_caller), Pair::said(&at_host))
}

#[test]
fn on_a_call_of_exact_samples_the_samples_are_the_data() {
    let (caller, host, told, got) = on_a_linear_call(1.0, 3.0);
    assert_eq!(caller.standard(), "V.ARC/L16");
    assert_eq!(host.varc_wanted(), Some(VarcMode::Linear));
    assert!(told.contains("CONNECT 200434/VARC"), "{told}");
    assert_eq!(got, "samples that are the data");
}

#[test]
fn a_call_that_is_not_exact_after_all_carries_on_as_the_multicarrier_mode() {
    // A tenth of a decibel of gain somewhere along the way.
    let (mut caller, host, told, got) = on_a_linear_call(0.989, 16.0);
    assert_eq!(caller.standard(), "V.ARC/DMT", "{told}");
    assert_eq!(host.state(), State::Data);
    assert!(told.contains("/VARC"), "{told}");
    assert_eq!(got, "samples that are the data");
    // Sixteen-bit samples are a clean line by any other measure.
    assert!(caller.rate().unwrap_or(0) > 100_000, "{:?} bit/s", caller.rate());
    assert!(caller.take_line_notes().iter().any(|n| n.contains("does not carry samples exactly")));
}

#[test]
fn a_call_answered_in_g711_becomes_an_ordinary_call() {
    let mut modem = Modem::new(16_000.0);
    type_at(&mut modem, "AT+MS=VARC\rATD100\r");
    assert_eq!(modem.varc_wanted(), Some(VarcMode::Dmt));
    modem.varc_fall_back();
    assert_eq!(modem.varc_wanted(), None);
    // Still the same call, still being placed, now from V.8.
    assert_eq!(modem.state(), State::Handshaking);
    assert!(modem.off_hook());
    assert_eq!(modem.standard(), "V.8");
    assert_eq!(modem.modulation(), "V92");
    assert!(modem.take_line_notes().iter().any(|n| n.contains("G.711")));
    // It makes sound again through the ordinary door.
    let loud = (0..16_000).map(|_| modem.step(0.0).abs()).fold(0.0, f64::max);
    assert!(loud > 0.0 || modem.line_phase() == "quiet", "{}", modem.line_phase());
}

/// A line made of two sound devices that do not run at the same rate: this
/// end hears on one at 192 kHz and sends on the other at 48 kHz, and the far
/// end the other way about. Each direction is a line of its own.
#[test]
fn two_devices_at_two_rates_are_a_line_each_way() {
    let (fast, slow) = (192_000.0, 48_000.0);
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARC\r");
    type_at(&mut host, "AT+MS=VARC\rATA\r");
    type_at(&mut caller, "ATD1\r");
    // The caller sends slowly and hears fast.
    let (mut from_caller, mut from_host) = ([0.0; 2], [0.0; 2]);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let mut typed = false;
    for tick in 0..(10.0 * fast) as usize {
        host.varc_say(&mut from_host);
        caller.varc_hear(fast, slow, &from_host);
        if tick % 4 == 0 {
            caller.varc_say(&mut from_caller);
            host.varc_hear(slow, fast, &from_caller);
        }
        at_caller.extend(caller.take_dte());
        at_host.extend(host.take_dte());
        if !typed && caller.state() == State::Data && host.state() == State::Data {
            typed = true;
            at_host.clear();
            type_at(&mut caller, "each way at its own rate");
        }
    }
    assert_eq!(caller.state(), State::Data, "{}", Pair::said(&at_caller));
    assert_eq!(Pair::said(&at_host), "each way at its own rate");
    // The fast way carries about four times what the slow way does.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(down > 3 * up && up > 300_000, "{down} arriving at the caller, {up} at the host");
}
