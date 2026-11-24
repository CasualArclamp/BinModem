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

    pair.run(13.0);
    assert_eq!(
        pair.caller.state(),
        State::Data,
        "{}",
        Pair::said(&pair.at_caller)
    );
    assert_eq!(
        pair.host.state(),
        State::Data,
        "{}",
        Pair::said(&pair.at_host)
    );
    let told = Pair::said(&pair.at_caller);
    assert!(
        told.contains("CONNECT ") && told.contains("/VARC"),
        "{told}"
    );
    // One clean signal at 48 kHz, on both channels: several hundred kilobits.
    let rate = pair.caller.rate().expect("a rate once connected");
    assert!(rate > 300_000, "{rate} bit/s");
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
    assert!(
        pair.at_host == page,
        "{} of {} octets",
        pair.at_host.len(),
        page.len()
    );
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
    assert!(
        Pair::said(&pair.at_host).contains("NO CARRIER"),
        "{}",
        Pair::said(&pair.at_host)
    );
}

#[test]
fn the_single_carrier_modes_connect_too() {
    for (carrier, name, least) in [
        ("VARCQ", "V.ARC/QAM", 125_000),
        ("VARCP", "V.ARC/PCM", 75_000),
    ] {
        let mut pair = Pair::new(carrier);
        type_at(&mut pair.host, "ATA\r");
        type_at(&mut pair.caller, "ATD1\r");
        pair.run(14.0);
        assert_eq!(pair.caller.standard(), name);
        assert_eq!(
            pair.caller.state(),
            State::Data,
            "{name}: {}",
            Pair::said(&pair.at_caller)
        );
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
    // PCM is looked at sooner than the others. On this line, which has no
    // noise at all, it goes on asking for more levels until it asks for more
    // than it can read, and a channel starts again; that is a few seconds
    // on its starting table every so often, and not what this is about.
    for (carrier, shape, after) in [
        ("VARC", "QAM", 14.0),
        ("VARCQ", "QAM", 14.0),
        ("VARCP", "PCM", 9.0),
    ] {
        let mut pair = Pair::new(carrier);
        assert_eq!(pair.caller.shape(), "-", "{carrier}: before the call");
        type_at(&mut pair.host, "ATA\r");
        type_at(&mut pair.caller, "ATD1\r");
        pair.run(after);
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
        let reach = points
            .iter()
            .map(|p| p.0.abs().max(p.1.abs()))
            .fold(0.0, f64::max);
        assert!((0.9..1.05).contains(&reach), "{carrier}: reaches {reach}");
        // Taken means taken.
        assert!(pair.caller.varc_points().len() < points.len(), "{carrier}");
    }
}

#[test]
fn a_call_of_exact_samples_draws_its_samples() {
    // No carrier, so no symbols in the sense the other modes have them; a
    // sample is sixteen bits at once, and that is what is drawn.
    let (mut caller, _, _, _) = on_a_linear_call(1.0, 3.0);
    let points = caller.varc_points();
    assert!(!points.is_empty());
    assert!(points.iter().all(|(x, y)| x.abs() <= 1.0 && y.abs() <= 1.0));
    assert_eq!(caller.shape(), "L16");
    assert_eq!(caller.states(), 65_536);
}

#[test]
fn a_lead_with_one_channel_wired_still_connects() {
    let mut pair = Pair::new("VARC");
    pair.wired = 1;
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD1\r");
    pair.run(19.0);
    assert_eq!(
        pair.caller.state(),
        State::Data,
        "{}",
        Pair::said(&pair.at_caller)
    );
    pair.at_host.clear();
    let page: Vec<u8> = (0..20_000u32).map(|i| (i * 7 + (i >> 5)) as u8).collect();
    for &b in &page {
        pair.caller.feed_dte(b);
    }
    pair.run(3.0);
    assert!(
        pair.at_host == page,
        "{} of {} octets",
        pair.at_host.len(),
        page.len()
    );
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
    // A sound card at 8 kHz, to get through two minutes quickly.
    for _ in 0..8_000 * 121 {
        modem.varc_step(8_000.0, &[0.0, 0.0], &mut out);
        said.extend(modem.take_dte());
    }
    assert_eq!(modem.state(), State::Command);
    assert!(
        Pair::said(&said).contains("NO ANSWER"),
        "{}",
        Pair::said(&said)
    );
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

/// Two modems on a SIP call in G.711: 8000 codewords a second, each
/// arriving as the level it is, which for mu-law's two noughts is the same
/// one; or every level moved a little, as a call that is recoded on the way
/// has them.
fn on_a_g711_call(a_law: bool, recoded: bool, seconds: f64) -> (Modem, Modem, String, String) {
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARC\r");
    type_at(&mut host, "AT+MS=VARC\rATA\r");
    type_at(&mut caller, "ATD100\r");
    let carry = |x: f64| if recoded { 0.93 * x + 0.0007 } else { x };
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let mut typed = false;
    for _ in 0..(seconds * 8000.0) as usize {
        let (to_host, to_caller) = ([carry(from_caller[0])], [carry(from_host[0])]);
        // As a line does it: told what it is before every block, and
        // stepped only while the call is still V.ARC's.
        for (modem, heard, said) in [
            (&mut caller, &to_caller, &mut from_caller),
            (&mut host, &to_host, &mut from_host),
        ] {
            if modem.varc_wanted().is_some() {
                modem.varc_codeword_line(Some(a_law));
                modem.varc_step(8000.0, heard, said);
            } else {
                said[0] = 0.0;
            }
        }
        at_caller.extend(caller.take_dte());
        at_host.extend(host.take_dte());
        if !typed && caller.state() == State::Data && host.state() == State::Data {
            typed = true;
            at_host.clear();
            type_at(&mut caller, "codewords that are the data");
        }
    }
    (caller, host, Pair::said(&at_caller), Pair::said(&at_host))
}

#[test]
fn on_a_call_in_g711_the_codewords_are_the_data_in_either_law() {
    for a_law in [true, false] {
        let (caller, host, told, got) = on_a_g711_call(a_law, false, 4.0);
        // Whatever was asked for: nothing else has anything to run on.
        assert_eq!(caller.standard(), "V.ARC/G.711", "a-law {a_law}: {told}");
        assert_eq!(host.varc_wanted(), Some(VarcMode::G711));
        assert!(told.contains("CONNECT 48640/VARC"), "a-law {a_law}: {told}");
        assert_eq!(got, "codewords that are the data", "a-law {a_law}");
    }
}

#[test]
fn a_call_in_g711_that_is_recoded_carries_on_as_an_ordinary_one() {
    let (caller, host, told, _) = on_a_g711_call(true, true, 12.0);
    assert_eq!(caller.varc_wanted(), None, "{told}");
    assert_eq!(host.varc_wanted(), None);
    assert!(!told.contains("CONNECT"), "{told}");
}

/// A linear call with `sent` written at the calling end once it is up,
/// each end having typed `setup` first: what arrived, and what the calling
/// end says of its compression.
/// What a call says of its compression: the method each way, and the octets.
type Squeezed = Option<(&'static str, &'static str, [u64; 4])>;

fn compressed(setup: [&str; 2], sent: &[u8], seconds: f64) -> (Vec<u8>, Squeezed) {
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, &format!("{}AT+MS=VARC\r", setup[0]));
    type_at(&mut host, &format!("{}AT+MS=VARC\rATA\r", setup[1]));
    type_at(&mut caller, "ATD100\r");
    caller.varc_exact_line(true);
    host.varc_exact_line(true);
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let mut at_host = Vec::new();
    let mut left = Some(sent);
    for _ in 0..(seconds * 16_000.0) as usize {
        let (to_host, to_caller) = ([from_caller[0]], [from_host[0]]);
        caller.varc_step(16_000.0, &to_caller, &mut from_caller);
        host.varc_step(16_000.0, &to_host, &mut from_host);
        caller.take_dte();
        at_host.extend(host.take_dte());
        if caller.state() == State::Data && host.state() == State::Data {
            // As a terminal does it: a piece at a time, when there is room.
            if let Some(rest) = left
                && caller.queued() < 2000
            {
                let take = rest.len().min(1500);
                for &octet in &rest[..take] {
                    caller.feed_dte(octet);
                }
                left = (take < rest.len()).then(|| &rest[take..]);
            }
        }
    }
    (at_host, caller.varc_compression())
}

#[test]
fn a_call_compresses_its_stream_with_the_best_both_ends_have() {
    let text: Vec<u8> = (0..120_000)
        .map(|i| {
            b"what the terminal wrote is mostly text, and text is mostly the same words. "[i % 75]
        })
        .collect();
    for (setup, method) in [
        (["", ""], "LZ"),
        (["AT+VARCZ=1\r", ""], "V.44"),
        (["AT+DS44=0\r", "AT+VARCZ=1\r"], "V.42bis"),
        (["AT+VARCZ=0\r", ""], "none"),
    ] {
        let (got, said) = compressed(setup, &text, 8.0);
        let (out, back, [plain, coded, _, _]) = said.expect("a V.ARC call");
        assert_eq!(out, method, "{setup:?}");
        assert_eq!(back, method, "{setup:?}");
        assert!(
            got.ends_with(&text[text.len() - 4000..]),
            "{setup:?}: {} octets arrived",
            got.len()
        );
        if method != "none" {
            assert!(
                coded * 4 < plain,
                "{setup:?}: {plain} octets sent as {coded}"
            );
        }
    }
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
    let (mut caller, host, told, got) = on_a_linear_call(0.989, 24.0);
    assert_eq!(caller.standard(), "V.ARC/DMT", "{told}");
    assert_eq!(host.state(), State::Data);
    assert!(told.contains("/VARC"), "{told}");
    assert_eq!(got, "samples that are the data");
    // Sixteen-bit samples are a clean line by any other measure.
    assert!(
        caller.rate().unwrap_or(0) > 100_000,
        "{:?} bit/s",
        caller.rate()
    );
    assert!(
        caller
            .take_line_notes()
            .iter()
            .any(|n| n.contains("does not carry samples exactly"))
    );
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
    assert!(
        modem
            .take_line_notes()
            .iter()
            .any(|n| n.contains("cannot carry it"))
    );
    // It makes sound again through the ordinary door.
    let loud = (0..16_000)
        .map(|_| modem.step(0.0).abs())
        .fold(0.0, f64::max);
    assert!(
        loud > 0.0 || modem.line_phase() == "quiet",
        "{}",
        modem.line_phase()
    );
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
    for tick in 0..(15.0 * fast) as usize {
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
    // The fast way carries a good deal more than the slow way does.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(
        down > 3 * up / 2 && up > 300_000,
        "{down} arriving at the caller, {up} at the host"
    );
}

/// One pair of wires: both ends send into it, and each hears the other
/// with its own signal on top, come back some milliseconds later through
/// whatever the line is.
#[test]
fn on_one_pair_of_wires_each_end_takes_its_own_echo_out() {
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARC2W\r");
    type_at(&mut host, "AT+MS=VARC2W\rATA\r");
    type_at(&mut caller, "ATD1\r");
    assert_eq!(caller.standard(), "V.ARC/DMT");
    // What each end's own signal comes back as, and how long after.
    let shape = [0.1, 0.7, 0.3, -0.1];
    let delay = 1500;
    let mut line_caller = std::collections::VecDeque::from(vec![0.0; delay + shape.len()]);
    let mut line_host = line_caller.clone();
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let mut typed = false;
    let echo = |line: &std::collections::VecDeque<f64>| -> f64 {
        shape
            .iter()
            .enumerate()
            .map(|(i, s)| s * line[delay + i])
            .sum()
    };
    for _ in 0..(45.0 * FS) as usize {
        // The far end arrives 6 dB down; the echo is the louder.
        let to_caller = [0.5 * from_host[0] + echo(&line_caller)];
        let to_host = [0.5 * from_caller[0] + echo(&line_host)];
        caller.varc_step(FS, &to_caller, &mut from_caller);
        host.varc_step(FS, &to_host, &mut from_host);
        line_caller.pop_back();
        line_caller.push_front(from_caller[0]);
        line_host.pop_back();
        line_host.push_front(from_host[0]);
        at_caller.extend(caller.take_dte());
        at_host.extend(host.take_dte());
        if !typed && caller.state() == State::Data && host.state() == State::Data {
            typed = true;
            at_host.clear();
            type_at(&mut caller, "both ways down one pair");
        }
    }
    assert_eq!(caller.state(), State::Data, "{}", Pair::said(&at_caller));
    assert_eq!(Pair::said(&at_host), "both ways down one pair");
    let echo = caller.varc_echo();
    assert_eq!(echo.len(), 1);
    let found = echo[0].1.expect("a delay");
    assert!(found.abs_diff(delay) < 8, "found {found}");
    // The rate after 40 s. The canceller in front of the receiver leaves
    // the echo some 37 dB down by then, which is 150 kbit/s; the receiver's
    // own stage, which learns from what is left once the far end's part is
    // taken out, is what takes it past that.
    let rate = caller.rate().unwrap_or(0);
    assert!(rate > 200_000, "{rate} bit/s");
    let far = caller.varc_far_end().expect("a report").0;
    assert!(
        far.snr_db > 50.0,
        "the far end reads this end at {:.1} dB",
        far.snr_db
    );
    // Each end had the line to itself first and said how that went.
    let told = caller.take_line_notes().join("\n") + &host.take_line_notes().join("\n");
    assert_eq!(
        told.matches("learned with the line to itself").count(),
        2,
        "{told}"
    );
    eprintln!("two-wire: {rate} bit/s, echo {:.1} dB down", echo[0].0);
}

/// Without the canceller the same line carries nothing: each end hears
/// itself louder than the other.
#[test]
fn the_same_pair_without_a_canceller_never_connects() {
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARC\r");
    type_at(&mut host, "AT+MS=VARC\rATA\r");
    type_at(&mut caller, "ATD1\r");
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let mut line = std::collections::VecDeque::from(vec![(0.0, 0.0); 1500]);
    for _ in 0..(12.0 * FS) as usize {
        let (own_caller, own_host) = line[1499];
        caller.varc_step(FS, &[0.5 * from_host[0] + own_caller], &mut from_caller);
        host.varc_step(FS, &[0.5 * from_caller[0] + own_host], &mut from_host);
        line.pop_back();
        line.push_front((from_caller[0], from_host[0]));
    }
    assert_ne!(caller.state(), State::Data);
    assert!(caller.varc_echo().is_empty());
}

/// Two ends on a line each way, each with its own rates hearing and
/// sending, stepped on a clock of 192 kHz. What one sends the other hears a
/// sample at a time, held or skipped where their rates differ, which is
/// crude and is what a pair of sound cards does to each other anyway.
fn across(
    carrier: &str,
    caller_rates: (f64, f64),
    host_rates: (f64, f64),
    seconds: f64,
) -> (Modem, Modem, String, String, Vec<String>) {
    let clock = 192_000.0;
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, &format!("AT+MS={carrier}\r"));
    type_at(&mut host, &format!("AT+MS={carrier}\rATA\r"));
    type_at(&mut caller, "ATD1\r");
    let (mut from_caller, mut from_host) = ([0.0; 2], [0.0; 2]);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let mut notes = Vec::new();
    // Owed samples: the caller hearing and sending, then the host.
    let mut owed = [0.0f64; 4];
    let rates = [caller_rates.0, caller_rates.1, host_rates.0, host_rates.1];
    let mut typed = false;
    for _ in 0..(seconds * clock) as usize {
        for (owed, rate) in owed.iter_mut().zip(rates) {
            *owed += rate / clock;
        }
        if owed[1] >= 1.0 {
            owed[1] -= 1.0;
            caller.varc_say(&mut from_caller);
        }
        if owed[3] >= 1.0 {
            owed[3] -= 1.0;
            host.varc_say(&mut from_host);
        }
        if owed[0] >= 1.0 {
            owed[0] -= 1.0;
            caller.varc_hear(caller_rates.0, caller_rates.1, &from_host);
        }
        if owed[2] >= 1.0 {
            owed[2] -= 1.0;
            host.varc_hear(host_rates.0, host_rates.1, &from_caller);
        }
        at_caller.extend(caller.take_dte());
        at_host.extend(host.take_dte());
        notes.extend(caller.take_line_notes());
        if !typed && caller.state() == State::Data && host.state() == State::Data {
            typed = true;
            at_host.clear();
            type_at(&mut caller, "said, then agreed, then sent");
        }
    }
    (
        caller,
        host,
        Pair::said(&at_caller),
        Pair::said(&at_host),
        notes,
    )
}

/// The rig that the handshake was written for: each end hears on one sound
/// device and sends on another at a different rate, and both were told the
/// line was two-wire, which on two devices cannot be run. They used to hear
/// nothing and say NO CARRIER.
#[test]
fn ends_that_were_told_two_wire_and_cannot_be_run_a_wire_each_way() {
    let (caller, host, told, got, notes) =
        across("VARC2W", (48_000.0, 192_000.0), (192_000.0, 48_000.0), 16.0);
    assert_eq!(caller.state(), State::Data, "{told}\n{notes:?}");
    assert_eq!(host.state(), State::Data);
    assert_eq!(got, "said, then agreed, then sent");
    assert!(
        caller.varc_echo().is_empty(),
        "no canceller on a wire each way"
    );
    let all = notes.join("\n");
    assert!(
        all.contains("the far end hears at 192000 Hz and sends at 48000 Hz"),
        "{all}"
    );
    assert!(all.contains("two-wire was asked for"), "{all}");
    assert!(
        all.contains("agreed on a wire each way, 192000 Hz out and 48000 Hz back"),
        "{all}"
    );
}

/// A card at 192 kHz calling one at 48 kHz. The modulation runs at 48 kHz
/// both ways and the faster end makes up the difference. They used to send
/// each other signals built for different rates.
#[test]
fn a_fast_card_and_a_slow_card_run_at_the_slow_cards_rate() {
    let (caller, host, told, got, notes) =
        across("VARC", (192_000.0, 192_000.0), (48_000.0, 48_000.0), 16.0);
    assert_eq!(caller.state(), State::Data, "{told}\n{notes:?}");
    assert_eq!(got, "said, then agreed, then sent");
    let all = notes.join("\n");
    assert!(
        all.contains("agreed on a wire each way, 48000 Hz out and 48000 Hz back"),
        "{all}"
    );
    assert!(all.contains("the ends' sound cards differ"), "{all}");
    // What a 48 kHz line carries, each way.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(down > 300_000 && up > 300_000, "{down} and {up} bit/s");
}

/// Ends that were asked for different modulations run the multicarrier one.
#[test]
fn ends_asked_for_different_modulations_agree_on_one() {
    let clock = 48_000.0;
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARCQ\r");
    type_at(&mut host, "AT+MS=VARCP\rATA\r");
    type_at(&mut caller, "ATD1\r");
    let (mut from_caller, mut from_host) = ([0.0; 2], [0.0; 2]);
    for _ in 0..(14.0 * clock) as usize {
        let (to_caller, to_host) = (from_host, from_caller);
        caller.varc_step(clock, &to_caller, &mut from_caller);
        host.varc_step(clock, &to_host, &mut from_host);
    }
    assert_eq!(caller.state(), State::Data);
    assert_eq!(caller.standard(), "V.ARC/DMT");
    assert_eq!(host.standard(), "V.ARC/DMT");
}

/// A line that passes only so much of the band: a windowed-sinc low-pass.
struct Dull {
    taps: Vec<f64>,
    past: std::collections::VecDeque<f64>,
}

impl Dull {
    fn new(fs: f64, top: f64) -> Self {
        let half = 200i64;
        let cut = 2.0 * top / fs;
        let taps: Vec<f64> = (-half..=half)
            .map(|i| {
                let t = i as f64;
                let sinc = if i == 0 {
                    cut
                } else {
                    (std::f64::consts::PI * cut * t).sin() / (std::f64::consts::PI * t)
                };
                sinc * (0.5 + 0.5 * (std::f64::consts::PI * t / (half + 1) as f64).cos())
            })
            .collect();
        Self {
            past: std::collections::VecDeque::from(vec![0.0; taps.len()]),
            taps,
        }
    }

    fn pass(&mut self, x: f64) -> f64 {
        self.past.pop_back();
        self.past.push_front(x);
        self.taps.iter().zip(&self.past).map(|(t, p)| t * p).sum()
    }
}

/// A line that is a cable one way and something dull the other: a
/// loudspeaker and a microphone, say, with nothing above 5 kHz. The single
/// carrier at 16 000 baud sits between 1.5 and 19.5 kHz and will never get
/// through that. The end that cannot hear says so in the handshake's tones,
/// and the far end sends slower until it can.
#[test]
fn an_end_that_cannot_hear_asks_for_it_slower_and_gets_it() {
    let mut caller = Modem::new(16_000.0);
    let mut host = Modem::new(16_000.0);
    type_at(&mut caller, "AT+MS=VARCQ\r");
    type_at(&mut host, "AT+MS=VARCQ\rATA\r");
    type_at(&mut caller, "ATD1\r");
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let mut dull = Dull::new(FS, 5_000.0);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let (mut caller_notes, mut host_notes) = (Vec::new(), Vec::new());
    let mut typed = false;
    for _ in 0..(70.0 * FS) as usize {
        // The caller hears the host through the dull path; the host hears
        // the caller as it was sent.
        let to_caller = [dull.pass(from_host[0])];
        let to_host = from_caller;
        caller.varc_step(FS, &to_caller, &mut from_caller);
        host.varc_step(FS, &to_host, &mut from_host);
        at_caller.extend(caller.take_dte());
        at_host.extend(host.take_dte());
        caller_notes.extend(caller.take_line_notes());
        host_notes.extend(host.take_line_notes());
        if !typed && caller.state() == State::Data && host.state() == State::Data {
            typed = true;
            at_caller.clear();
            type_at(&mut host, "slower, and through");
        }
    }
    let (told, heard) = (caller_notes.join("\n"), host_notes.join("\n"));
    assert_eq!(caller.state(), State::Data, "{told}");
    assert_eq!(host.state(), State::Data, "{heard}");
    // What crossed the dull path, the way that could not be heard at first.
    assert_eq!(Pair::said(&at_caller), "slower, and through");
    // The dull way is slowed four times: 4000 baud between 375 Hz and
    // 4.9 kHz. The other way is as it was.
    assert_eq!(caller.varc_slowed(), (4, 1), "{told}");
    assert_eq!(host.varc_slowed(), (1, 4), "{heard}");
    assert!(
        told.contains("asking it to send 2 times slower") && told.contains("4 times slower"),
        "{told}"
    );
    assert!(
        heard.contains("the far end cannot hear this end; sending 4 times slower"),
        "{heard}"
    );
    // And the rates say so: a quarter of the baud one way.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(
        down > 10_000 && down < 100_000 && up > 150_000,
        "{down} down the dull way, {up} up the cable"
    );
}

/// The baud, the code and the bits a symbol, chosen by hand at one end for
/// what that end hears: `AT+VARC=8000,50,4`. The other way is left to the
/// modem and is whatever the line bears.
#[test]
fn a_baud_a_code_and_a_grid_can_be_chosen_by_hand() {
    let mut pair = Pair::new("VARCQ");
    type_at(&mut pair.caller, "AT+VARC=8000,50,4\r");
    type_at(&mut pair.caller, "AT+VARC?\r");
    assert!(Pair::said(&pair.caller.take_dte()).contains("+VARC: 8000,50,4"));
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD1\r");
    pair.run(16.0);
    assert_eq!(
        pair.caller.state(),
        State::Data,
        "{}",
        Pair::said(&pair.at_caller)
    );
    // What the caller hears is what it asked for: half the baud a 48 kHz
    // line would give it, sixteen points, half of every word checks.
    assert_eq!(pair.caller.varc_bauds(), (8_000, 16_000));
    assert_eq!(pair.host.varc_bauds(), (16_000, 8_000));
    // The grid is whatever the symbol diagram was last given.
    pair.caller.varc_points();
    pair.run(0.5);
    assert!(!pair.caller.varc_points().is_empty());
    assert_eq!(pair.caller.states(), 16);
    let (n, k) = pair.caller.varc_code().expect("a code");
    assert!(
        (k as f64 / n as f64 - 0.5).abs() < 0.03,
        "a code of {k}/{n}"
    );
    // 8000 baud of four bits, half of them data, less the sync words.
    let rate = pair.caller.rate().unwrap_or(0);
    assert!((14_000..16_500).contains(&rate), "{rate} bit/s");
    // And the way nobody chose is as fast as it would have been.
    assert!(
        pair.host.rate().unwrap_or(0) > 100_000,
        "{:?}",
        pair.host.rate()
    );
    pair.at_caller.clear();
    type_at(&mut pair.host, "as asked");
    pair.run(1.5);
    assert_eq!(Pair::said(&pair.at_caller), "as asked");
}

/// The single-carrier modes fill the band of whatever card they are on: a
/// third of its rate in baud for the carrier, two-thirds for the levels.
#[test]
fn the_single_carrier_modes_run_at_the_cards_own_rate() {
    for (carrier, fs, baud) in [
        ("VARCQ", 48_000.0, 16_000),
        ("VARCP", 48_000.0, 32_000),
        ("VARCQ", 192_000.0, 64_000),
    ] {
        let mut caller = Modem::new(16_000.0);
        let mut host = Modem::new(16_000.0);
        type_at(&mut caller, &format!("AT+MS={carrier}\r"));
        type_at(&mut host, &format!("AT+MS={carrier}\rATA\r"));
        type_at(&mut caller, "ATD1\r");
        let (mut from_caller, mut from_host) = ([0.0], [0.0]);
        for _ in 0..(12.0 * fs) as usize {
            let (to_caller, to_host) = (from_host, from_caller);
            caller.varc_step(fs, &to_caller, &mut from_caller);
            host.varc_step(fs, &to_host, &mut from_host);
        }
        assert_eq!(caller.state(), State::Data, "{carrier} at {fs} Hz");
        assert_eq!(caller.varc_bauds(), (baud, baud), "{carrier} at {fs} Hz");
        // Several bits a symbol on a line with no noise in it.
        let rate = caller.rate().unwrap_or(0);
        assert!(
            rate > 4 * baud,
            "{carrier} at {fs} Hz: {rate} bit/s at {baud} baud"
        );
    }
}

/// An ordinary call between two modems, each stepped at the telephone
/// line's rate until it asks for the sound card as it is, and at the card's
/// rate from then on. Returns the two and what was noted of the line.
fn bis_call(caller_set: &str, host_set: &str, seconds: f64) -> (Modem, Modem, String, String) {
    const LINE: f64 = 16_000.0;
    let mut caller = Modem::new(LINE);
    let mut host = Modem::new(LINE);
    type_at(&mut caller, &format!("AT+A8E=,,,1{caller_set}\r"));
    type_at(&mut host, &format!("AT+A8E=,,,1{host_set}\r"));
    type_at(&mut host, "ATA\r");
    type_at(&mut caller, "ATD5551234\r");
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let (mut wide_caller, mut wide_host) = ([0.0; 2], [0.0; 2]);
    let mut notes = String::new();
    // The card's clock, 48 kHz, with the line's every third tick of it. An
    // end that has gone over to V.ARC is on the card and silent on the
    // line, and one that has not hears nothing of the card, as it would
    // not through a line's filters.
    for tick in 0..(seconds * FS) as usize {
        let (card_caller, card_host) = (wide_caller, wide_host);
        if caller.varc_wanted().is_some() {
            caller.varc_step(FS, &card_host, &mut wide_caller);
        }
        if host.varc_wanted().is_some() {
            host.varc_step(FS, &card_caller, &mut wide_host);
        }
        if tick % 3 == 0 {
            let (was_caller, was_host) = (from_caller, from_host);
            from_caller = if caller.varc_wanted().is_none() {
                caller.step(was_host)
            } else {
                0.0
            };
            from_host = if host.varc_wanted().is_none() {
                host.step(was_caller)
            } else {
                0.0
            };
        }
        for (who, m) in [("caller", &mut caller), ("host", &mut host)] {
            for line in m.take_line_notes() {
                notes.push_str(&format!("{who}: {line}\n"));
            }
        }
        if caller.state() == State::Data && host.state() == State::Data {
            break;
        }
    }
    let told = Pair::said(&caller.take_dte());
    (caller, host, notes, told)
}

/// `+VARCBIS` at both ends of a call that was not dialled as V.ARC: the
/// V.8 bis transaction finds it, and the call is a V.ARC one.
#[test]
fn an_ordinary_call_between_two_that_list_varc_becomes_one() {
    let (caller, host, notes, told) = bis_call(";+VARCBIS=1", ";+VARCBIS=1", 40.0);
    println!("{notes}");
    assert!(notes.contains("both ends have V.ARC"), "{notes}");
    assert_eq!(caller.varc_wanted(), Some(VarcMode::Dmt));
    assert_eq!(host.varc_wanted(), Some(VarcMode::Dmt));
    assert_eq!(caller.state(), State::Data, "{notes}");
    assert_eq!(host.state(), State::Data, "{notes}");
    assert!(
        told.contains("CONNECT ") && told.contains("/VARC"),
        "{told}"
    );
    assert!(
        caller.rate().unwrap_or(0) > 300_000,
        "{:?} bit/s",
        caller.rate()
    );
}

/// The same with it at one end only: nothing of V.ARC happens at all.
#[test]
fn a_call_to_one_that_does_not_list_varc_is_an_ordinary_call() {
    for (caller_set, host_set) in [(";+VARCBIS=1", ""), ("", ";+VARCBIS=1")] {
        let (caller, host, notes, _) = bis_call(caller_set, host_set, 20.0);
        assert!(!notes.contains("both ends have V.ARC"), "{notes}");
        assert!(
            notes.contains("V.8 bis: sent MS") || notes.contains("V.8 bis: heard MS"),
            "{notes}"
        );
        assert_eq!(caller.varc_wanted(), None);
        assert_eq!(host.varc_wanted(), None);
    }
}

/// V.ARC chosen in V.8 bis on a line that turns out not to carry it: the
/// call starts again as an ordinary one, and does not choose it twice.
#[test]
fn varc_chosen_and_then_refused_by_the_line_is_not_chosen_again() {
    const LINE: f64 = 16_000.0;
    let mut caller = Modem::new(LINE);
    let mut host = Modem::new(LINE);
    type_at(&mut caller, "AT+A8E=,,,1;+VARCBIS=1\r");
    type_at(&mut host, "AT+A8E=,,,1;+VARCBIS=1\r");
    type_at(&mut host, "ATA\r");
    type_at(&mut caller, "ATD5551234\r");
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let mut notes = String::new();
    let mut fell_back = 0;
    for _ in 0..(30.0 * LINE) as usize {
        for m in [&mut caller, &mut host] {
            if m.varc_wanted().is_some() {
                m.varc_fall_back();
                fell_back += 1;
            }
        }
        let (was_caller, was_host) = (from_caller, from_host);
        from_caller = caller.step(was_host);
        from_host = host.step(was_caller);
        for (who, m) in [("caller", &mut caller), ("host", &mut host)] {
            for line in m.take_line_notes() {
                notes.push_str(&format!("{who}: {line}\n"));
            }
        }
    }
    assert_eq!(fell_back, 2, "once at each end and no more\n{notes}");
    assert_eq!(
        notes.matches("caller: V.8 bis: sent MS").count(),
        2,
        "{notes}"
    );
    assert!(notes.contains("caller: V.8 bis: sent MS (V.92"), "{notes}");
}
