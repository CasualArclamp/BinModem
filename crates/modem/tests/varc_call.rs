//! A V.ARC call between two modems, typed at as a terminal would.
//!
//! The modulation has its own tests. What is tested here is that it is a
//! *modem*: that `AT+MS=VARC` and `ATD` place a call, that the call starts
//! with V.8 and then the handshake of V.ARC as every call does, that the
//! terminal is told `CONNECT` with a rate, that what is typed at one end
//! comes out of the other, and that `+++` and `ATH` end it. And that it is
//! so on every line a call can be on: a sound card, and a SIP call in
//! linear samples or in G.711 of either law.

use modem::{Modem, State, VarcMode};
use sip::g711::Law;

const FS: f64 = 48_000.0;

/// The rate of the modem itself, which V.8 runs at.
const LINE: f64 = 16_000.0;

fn type_at(modem: &mut Modem, text: &str) {
    for b in text.bytes() {
        modem.feed_dte(b);
    }
}

fn said(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Two modems that are set, with one told to answer and the other to dial.
fn dialled(caller_set: &str, host_set: &str) -> (Modem, Modem) {
    let mut caller = Modem::new(LINE);
    let mut host = Modem::new(LINE);
    type_at(&mut caller, &format!("{caller_set}\r"));
    type_at(&mut host, &format!("{host_set}\rATA\r"));
    type_at(&mut caller, "ATD5551234\r");
    (caller, host)
}

/// Take two modems that have dialled and answered through V.8, on a clean
/// line at the rate of the modem, to where both ask for the sound card as
/// it is. The tests that follow have a line of their own for the V.ARC
/// part of the call, and V.8 has its own tests on lines of every kind.
fn through_v8(caller: &mut Modem, host: &mut Modem) {
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    for _ in 0..(20.0 * LINE) as usize {
        if caller.varc_wanted().is_some() && host.varc_wanted().is_some() {
            return;
        }
        let (was_caller, was_host) = (from_caller, from_host);
        from_caller = caller.step(was_host);
        from_host = host.step(was_caller);
    }
    panic!("V.8 did not find V.ARC at both ends: the caller is in {}, the host in {}", caller.line_phase(), host.line_phase());
}

/// Two modems on a stereo lead each way: what one sends on a channel the
/// other hears on the same channel. The whole call is on the lead, V.8
/// included.
struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: [f64; 2],
    from_host: [f64; 2],
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
    /// What each end noted of the line, in order, with who noted it.
    notes: Vec<String>,
    /// Channels the lead really has wired.
    wired: usize,
}

impl Pair {
    fn new(carrier: &str) -> Self {
        let mut caller = Modem::new(LINE);
        let mut host = Modem::new(LINE);
        type_at(&mut caller, &format!("AT+MS={carrier}\r"));
        type_at(&mut host, &format!("AT+MS={carrier}\r"));
        Self {
            caller,
            host,
            from_caller: [0.0; 2],
            from_host: [0.0; 2],
            at_caller: Vec::new(),
            at_host: Vec::new(),
            notes: Vec::new(),
            wired: 2,
        }
    }

    fn tick(&mut self) {
        let mut to_host = self.from_caller;
        let mut to_caller = self.from_host;
        for c in self.wired..2 {
            to_host[c] = 0.0;
            to_caller[c] = 0.0;
        }
        self.caller.wide_step(FS, &to_caller, &mut self.from_caller);
        self.host.wide_step(FS, &to_host, &mut self.from_host);
        self.at_caller.extend(self.caller.take_dte());
        self.at_host.extend(self.host.take_dte());
        for (who, modem) in [("caller", &mut self.caller), ("host", &mut self.host)] {
            for note in modem.take_line_notes() {
                self.notes.push(format!("{who}: {note}"));
            }
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            self.tick();
        }
    }

    /// Run until both ends are connected, or for `limit` seconds.
    fn connect(&mut self, limit: f64) {
        for _ in 0..(limit * FS) as usize {
            if self.caller.state() == State::Data && self.host.state() == State::Data {
                return;
            }
            self.tick();
        }
    }
}

/// Where each of `wanted` first is in `notes`, which has to be in that
/// order.
fn in_order(notes: &[String], wanted: &[&str]) {
    let mut from = 0;
    for want in wanted {
        match notes[from..].iter().position(|note| note.contains(want)) {
            Some(at) => from += at + 1,
            None => panic!("no \"{want}\" after note {from} of:\n{}", notes.join("\n")),
        }
    }
}

#[test]
fn a_call_is_placed_answered_used_and_put_down() {
    let mut pair = Pair::new("VARC");
    assert_eq!(pair.caller.standard(), "V.ARC/DMT");
    assert_eq!(pair.caller.varc_wanted(), None);
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD5551234\r");
    assert_eq!(pair.caller.state(), State::Handshaking);
    assert!(pair.caller.off_hook());
    // The call starts with V.8 and not with a signal of its own: it is not
    // yet a V.ARC call, and it is on the line as any call is.
    assert_eq!(pair.caller.varc_wanted(), None);
    assert_eq!(pair.caller.standard(), "V.8");
    pair.run(3.0);
    assert_eq!(pair.caller.varc_wanted(), None);
    assert_eq!(pair.host.standard(), "V.8");

    pair.connect(40.0);
    assert_eq!(pair.caller.varc_wanted(), Some(VarcMode::Dmt));
    assert_eq!(pair.caller.state(), State::Data, "{}", said(&pair.at_caller));
    assert_eq!(pair.host.state(), State::Data, "{}", said(&pair.at_host));
    let told = said(&pair.at_caller);
    assert!(told.contains("CONNECT ") && told.contains("/VARC"), "{told}");
    // V.8, then the handshake of V.ARC, then the mode: at both ends.
    for who in ["caller", "host"] {
        let own: Vec<String> = pair.notes.iter().filter(|note| note.starts_with(who)).cloned().collect();
        in_order(
            &own,
            &[
                "V.8: both ends have V.ARC",
                "V.ARC: calling the far end",
                "V.ARC: agreeing with the far end",
                "V.ARC: agreed on 48000 Hz out and 48000 Hz back",
                // Each end had the line to itself and heard nothing back.
                "V.ARC: no echo of this end on the line",
                "V.ARC: loaded",
                "V.ARC/DMT: ",
            ],
        );
    }
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
    assert!(pair.at_host == page, "{} of {} octets", pair.at_host.len(), page.len());
    assert_eq!(said(&pair.at_caller), "hello from the far end\r\n");

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
    pair.run(8.0);
    assert_eq!(pair.host.state(), State::Command);
    assert!(said(&pair.at_host).contains("NO CARRIER"), "{}", said(&pair.at_host));
}

#[test]
fn the_single_carrier_modes_connect_too() {
    for (carrier, name, least) in [("VARCQ", "V.ARC/QAM", 125_000), ("VARCP", "V.ARC/PCM", 75_000)] {
        let mut pair = Pair::new(carrier);
        type_at(&mut pair.host, "ATA\r");
        type_at(&mut pair.caller, "ATD1\r");
        pair.connect(40.0);
        assert_eq!(pair.caller.standard(), name);
        assert_eq!(pair.caller.state(), State::Data, "{name}: {}", said(&pair.at_caller));
        in_order(&pair.notes, &["V.8: both ends have V.ARC", "V.ARC: agreed on", "V.ARC: loaded"]);
        let rate = pair.caller.rate().unwrap_or(0);
        assert!(rate > least, "{name}: {rate} bit/s");
        pair.at_host.clear();
        type_at(&mut pair.caller, "the quick brown fox");
        pair.run(1.0);
        assert_eq!(said(&pair.at_host), "the quick brown fox", "{name}");
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
        pair.connect(40.0);
        assert_eq!(pair.caller.state(), State::Data, "{carrier}");
        // PCM is looked at sooner than the others. On this line, which has
        // no noise at all, it goes on asking for more levels until it asks
        // for more than it can read, and a channel starts again; that is a
        // few seconds on its starting table every so often, and not what
        // this is about.
        pair.run(if carrier == "VARCP" { 0.2 } else { 1.0 });
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
fn a_lead_with_one_channel_wired_still_connects() {
    let mut pair = Pair::new("VARC");
    pair.wired = 1;
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD1\r");
    pair.connect(45.0);
    assert_eq!(pair.caller.state(), State::Data, "{}", said(&pair.at_caller));
    pair.at_host.clear();
    let page: Vec<u8> = (0..20_000u32).map(|i| (i * 7 + (i >> 5)) as u8).collect();
    for &b in &page {
        pair.caller.feed_dte(b);
    }
    pair.run(3.0);
    assert!(pair.at_host == page, "{} of {} octets", pair.at_host.len(), page.len());
}

/// The one call that does not start with V.8: there is nobody to negotiate
/// with on a lead that comes straight back.
#[test]
fn a_lead_that_comes_straight_back_is_a_call_to_oneself() {
    let mut modem = Modem::new(LINE);
    type_at(&mut modem, "AT+MS=VARC\r");
    type_at(&mut modem, "ATDLOOP\r");
    assert_eq!(modem.varc_wanted(), Some(VarcMode::Dmt));
    let mut wire = [0.0; 2];
    let mut heard = Vec::new();
    let step = |modem: &mut Modem, wire: &mut [f64; 2], heard: &mut Vec<u8>, seconds: f64| {
        for _ in 0..(seconds * FS) as usize {
            let was = *wire;
            modem.varc_step(FS, &was, wire);
            heard.extend(modem.take_dte());
        }
    };
    step(&mut modem, &mut wire, &mut heard, 8.0);
    assert_eq!(modem.state(), State::Data, "{}", said(&heard));
    heard.clear();
    type_at(&mut modem, "round and back again");
    step(&mut modem, &mut wire, &mut heard, 1.0);
    assert_eq!(said(&heard), "round and back again");
}

#[test]
fn nobody_there_ends_the_call_and_it_never_becomes_a_varc_call() {
    let mut modem = Modem::new(LINE);
    type_at(&mut modem, "AT+MS=VARC\r");
    type_at(&mut modem, "ATD1\r");
    let mut heard = modem.take_dte();
    let mut out = [0.0; 2];
    // A sound card at 8 kHz, to get through two minutes quickly.
    for _ in 0..8_000 * 121 {
        modem.wide_step(8_000.0, &[0.0, 0.0], &mut out);
        assert_eq!(modem.varc_wanted(), None);
        // A calling modem says nothing until it hears an answer.
        assert_eq!(out, [0.0, 0.0]);
        heard.extend(modem.take_dte());
    }
    assert_eq!(modem.state(), State::Command);
    let told = said(&heard);
    assert!(told.contains("NO ANSWER") || told.contains("NO CARRIER"), "{told}");
}

/// What a SIP call carries at one end of it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Coding {
    /// Sixteen-bit linear samples at 16 kHz.
    Linear,
    /// G.711 at 8 kHz, in one law or the other.
    G711(Law),
}

impl Coding {
    fn rate(self) -> f64 {
        match self {
            Self::Linear => 16_000.0,
            Self::G711(_) => 8_000.0,
        }
    }

    /// What the coding makes of a level: the nearest number it has.
    fn carry(self, level: f64) -> f64 {
        let sample = (level * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        match self {
            Self::Linear => f64::from(sample) / 32768.0,
            Self::G711(law) => f64::from(law.decode(law.encode(sample))) / 32768.0,
        }
    }
}

/// A SIP call between two modems, run as `modem-sip` and the window run
/// one.
///
/// While an end is not on a V.ARC call it is stepped at its own rate
/// through the coding: what it hears is the call's numbers drawn out to
/// 16 kHz, and what it says is cut down to the rate of the call and coded.
/// That is how V.8 crosses the call. Once an end is on a V.ARC call it is
/// told what the call is made of and stepped with the numbers of the call
/// themselves, one for one.
struct Sip {
    /// What the call carries at each end: the caller, then the host. They
    /// differ where something between the two changes one into the other.
    coding: (Coding, Coding),
    /// Something on the way decodes the call, moves every level a little
    /// and codes it again.
    recoded: bool,
    caller: Modem,
    host: Modem,
    told: String,
    notes: Vec<String>,
    /// What the call passes of the band, if not all of it: a filter each
    /// way, as a network of telephone lines has.
    band: Option<[(Dull, Dull); 2]>,
    /// What each end last said, and last heard, at the rate of the call.
    from: [f64; 2],
    heard: [f64; 2],
    seconds: f64,
}

impl Sip {
    fn new(coding: (Coding, Coding), recoded: bool, caller_set: &str, host_set: &str) -> Self {
        assert_eq!(coding.0.rate(), coding.1.rate());
        let (caller, host) = dialled(caller_set, host_set);
        Self { coding, recoded, caller, host, told: String::new(), notes: Vec::new(), band: None, from: [0.0; 2], heard: [0.0; 2], seconds: 0.0 }
    }

    /// A call in one coding from end to end, with nothing in the way, and
    /// both ends set to the same V.ARC mode.
    fn clean(coding: Coding, carrier: &str) -> Self {
        Self::new((coding, coding), false, &format!("AT+MS={carrier}"), &format!("AT+MS={carrier}"))
    }

    /// One sample of the call at one end: what arrived, and what goes.
    fn end(modem: &mut Modem, coding: Coding, arrived: f64, was: &mut f64) -> f64 {
        let said = if modem.varc_wanted().is_some() {
            match coding {
                Coding::Linear => modem.varc_exact_line(true),
                Coding::G711(law) => modem.varc_codeword_line(Some(law == Law::A)),
            }
            let mut out = [0.0];
            modem.varc_step(coding.rate(), &[arrived], &mut out);
            out[0]
        } else {
            let per = (LINE / coding.rate()) as usize;
            let mut sum = 0.0;
            for n in 1..=per {
                sum += modem.step(*was + (arrived - *was) * n as f64 / per as f64);
            }
            sum / per as f64
        };
        *was = arrived;
        coding.carry(said)
    }

    fn tick(&mut self) {
        let filtered = self.band.is_some();
        let across = |level: f64, to: Coding, from: Coding, recoded: bool| {
            if recoded {
                to.carry(if filtered { level } else { 0.93 * level + 0.0007 })
            } else if to == from {
                level
            } else {
                to.carry(level)
            }
        };
        // What is between the two ends decodes the call to filter it, and
        // codes it again.
        let mut passed = [self.from[0], self.from[1]];
        if let Some(band) = self.band.as_mut() {
            for (level, (top, bottom)) in passed.iter_mut().zip(band.iter_mut()) {
                *level = top.pass(*level) - bottom.pass(*level);
            }
        }
        let recoded = self.recoded || self.band.is_some();
        let to_caller = across(passed[1], self.coding.0, self.coding.1, recoded);
        let to_host = across(passed[0], self.coding.1, self.coding.0, recoded);
        self.from[0] = Self::end(&mut self.caller, self.coding.0, to_caller, &mut self.heard[0]);
        self.from[1] = Self::end(&mut self.host, self.coding.1, to_host, &mut self.heard[1]);
        self.told.push_str(&said(&self.caller.take_dte()));
        for (who, modem) in [("caller", &mut self.caller), ("host", &mut self.host)] {
            for note in modem.take_line_notes() {
                self.notes.push(format!("{who}: {note}"));
            }
        }
        self.seconds += 1.0 / self.coding.0.rate();
    }

    /// Run until both ends are connected, or for `limit` seconds.
    fn connect(&mut self, limit: f64) -> bool {
        while self.seconds < limit {
            if self.caller.state() == State::Data && self.host.state() == State::Data {
                return true;
            }
            self.tick();
        }
        false
    }

    /// Write `page` at the calling end and run until all of it has come
    /// out of the host, or for `limit` seconds more: what came out.
    fn send(&mut self, page: &[u8], limit: f64) -> Vec<u8> {
        self.host.take_dte();
        let mut got = Vec::new();
        let mut left = page;
        let until = self.seconds + limit;
        while self.seconds < until && got.len() < page.len() {
            // As a terminal does it: a piece at a time, when there is room.
            if !left.is_empty() && self.caller.queued() < 2000 {
                let take = left.len().min(1500);
                for &octet in &left[..take] {
                    self.caller.feed_dte(octet);
                }
                left = &left[take..];
            }
            self.tick();
            got.extend(self.host.take_dte());
        }
        got
    }

    fn all_notes(&self) -> String {
        self.notes.join("\n")
    }
}

fn page(octets: u32) -> Vec<u8> {
    (0..octets).map(|i| (i * 31 + (i >> 7) + 5) as u8).collect()
}

/// Every mode on a call in G.711, in either law: V.8 through the coding,
/// then the handshake of V.ARC, then the mode, and a page with no octet
/// wrong. The multicarrier mode finds that the codewords arrive as they
/// were sent and uses them as the data. The single-carrier modes were asked
/// for by name and run as themselves, through what the law does to a
/// waveform.
#[test]
fn every_mode_runs_on_a_call_in_g711_in_either_law() {
    for law in [Law::A, Law::Mu] {
        for (carrier, name, least) in [
            ("VARC", "V.ARC/G.711", 48_000),
            // Fitted to the call, 3700 baud where a third of the call's
            // rate is 2667, and on the trellis code: more than a V.34
            // carries at its best.
            ("VARCQ", "V.ARC/QAM", 35_000),
            ("VARCP", "V.ARC/PCM", 12_000),
            // A name from when one pair of wires was a choice.
            ("VARCQ2W", "V.ARC/QAM", 35_000),
        ] {
            let what = format!("{carrier} in {law:?}");
            let mut call = Sip::clean(Coding::G711(law), carrier);
            assert!(call.connect(60.0), "{what}:\n{}", call.all_notes());
            assert_eq!(call.caller.standard(), name, "{what}");
            assert_eq!(call.host.standard(), name, "{what}");
            assert!(call.told.contains("CONNECT ") && call.told.contains("/VARC"), "{what}: {}", call.told);
            for who in ["caller", "host"] {
                let own: Vec<String> = call.notes.iter().filter(|note| note.starts_with(who)).cloned().collect();
                in_order(&own, &["V.8: both ends have V.ARC", "V.ARC: agreeing with the far end", "V.ARC: agreed on 8000 Hz out and 8000 Hz back", "V.ARC: loaded"]);
            }
            let rate = call.caller.rate().unwrap_or(0);
            assert!(rate >= least, "{what}: {rate} bit/s");
            // A call of packets has a path each way: no echo is looked for.
            assert!(!call.all_notes().contains("echo"), "{what}:\n{}", call.all_notes());
            assert!(call.caller.varc_echo().is_empty(), "{what}: no canceller on a call of packets");
            let sent = page(6000);
            let got = call.send(&sent, 12.0);
            assert!(got == sent, "{what}: {} of {} octets, at {rate} bit/s", got.len(), sent.len());
        }
    }
}

/// A call in G.711 that something on the way decodes and codes again: no
/// codeword arrives as it was sent, and the waveform does. Every mode
/// still comes up. The multicarrier mode tries the codewords, finds in a
/// few seconds that they do not arrive, and carries on as the modulation.
#[test]
fn every_mode_runs_on_a_call_in_g711_that_is_recoded() {
    for (law, carrier, name) in [
        (Law::A, "VARC", "V.ARC/DMT"),
        (Law::Mu, "VARC", "V.ARC/DMT"),
        (Law::Mu, "VARCQ", "V.ARC/QAM"),
        (Law::A, "VARCP", "V.ARC/PCM"),
    ] {
        let what = format!("{carrier} in {law:?}, recoded");
        let mut call = Sip::new((Coding::G711(law), Coding::G711(law)), true, &format!("AT+MS={carrier}"), &format!("AT+MS={carrier}"));
        assert!(call.connect(80.0), "{what}:\n{}", call.all_notes());
        assert_eq!(call.caller.standard(), name, "{what}");
        assert_eq!(call.host.standard(), name, "{what}");
        if carrier == "VARC" {
            assert_eq!(call.all_notes().matches("does not carry G.711 codewords as they were sent").count(), 2, "{what}:\n{}", call.all_notes());
        }
        let rate = call.caller.rate().unwrap_or(0);
        assert!(rate > 10_000, "{what}: {rate} bit/s");
        let sent = page(6000);
        let got = call.send(&sent, 15.0);
        assert!(got == sent, "{what}: {} of {} octets, at {rate} bit/s", got.len(), sent.len());
    }
}

/// A call that is A-law at one end and mu-law at the other, with something
/// between the two that changes one into the other. The two ends say what
/// they are on in the handshake, see that they differ, and do not try the
/// codewords at all.
#[test]
fn ends_in_different_laws_run_the_modulation_and_do_not_try_the_codewords() {
    let mut call = Sip::new((Coding::G711(Law::A), Coding::G711(Law::Mu)), false, "AT+MS=VARC", "AT+MS=VARC");
    assert!(call.connect(60.0), "{}", call.all_notes());
    assert_eq!(call.caller.standard(), "V.ARC/DMT");
    assert_eq!(call.host.standard(), "V.ARC/DMT");
    assert!(!call.all_notes().contains("codewords"), "{}", call.all_notes());
    let sent = page(6000);
    let got = call.send(&sent, 12.0);
    assert!(got == sent, "{} of {} octets", got.len(), sent.len());
}

/// A single carrier is fitted to the call: the handshake's probe finds
/// that a call in G.711 passes nearly all of its 4 kHz, and the carrier
/// runs at 3700 baud across it. Where the call goes through something that
/// passes a telephone's band and no more, the probe finds that too, and
/// the carrier runs as it is built, inside that band.
#[test]
fn a_single_carrier_is_fitted_to_the_band_the_call_passes() {
    let mut call = Sip::clean(Coding::G711(Law::Mu), "VARCQ");
    assert!(call.connect(60.0), "{}", call.all_notes());
    assert_eq!(call.caller.varc_bauds(), (3700, 3700));
    assert_eq!(call.host.varc_bauds(), (3700, 3700));
    let wide = call.caller.rate().unwrap_or(0);
    assert!(wide > 35_000, "{wide} bit/s");
    // And it is a rate the call keeps: a quarter of a megabyte crosses
    // whole, at nine-tenths of that rate or better, with the tables left
    // as they were first made.
    let before = call.notes.len();
    // Octets with no pattern to them, or the call's compression would
    // carry them in a fraction of the time.
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let sent: Vec<u8> = (0..250_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();
    let started = call.seconds;
    let got = call.send(&sent, 80.0);
    assert!(got == sent, "{} of {} octets", got.len(), sent.len());
    let carried = 8.0 * sent.len() as f64 / (call.seconds - started);
    eprintln!("V.ARC/QAM through mu-law: {wide} bit/s on the line, {carried:.0} bit/s carried");
    assert!(carried > 0.9 * f64::from(wide), "{carried:.0} bit/s carried at {wide}
{}", call.notes[before..].join("
"));
    assert!(!call.notes[before..].iter().any(|note| note.contains("exchanging tables")), "{}", call.notes[before..].join("
"));

    let mut call = Sip::clean(Coding::G711(Law::Mu), "VARCQ");
    let band = || (Dull::new(8_000.0, 3_400.0), Dull::new(8_000.0, 300.0));
    call.band = Some([band(), band()]);
    assert!(call.connect(80.0), "{}", call.all_notes());
    assert_eq!(call.caller.varc_bauds(), (2666, 2666), "{}", call.all_notes());
    assert_eq!(call.host.varc_bauds(), (2666, 2666));
    let narrow = call.caller.rate().unwrap_or(0);
    assert!(narrow > 12_000 && narrow < wide, "{narrow} bit/s");
    let sent = page(4000);
    let got = call.send(&sent, 12.0);
    assert!(got == sent, "{} of {} octets", got.len(), sent.len());
}

/// A baud, a code and a grid chosen by hand hold on a call in G.711 too,
/// where a carrier at full speed is 2666 baud.
#[test]
fn a_mode_set_by_hand_runs_on_a_call_in_g711() {
    let mut call = Sip::new((Coding::G711(Law::Mu), Coding::G711(Law::Mu)), false, "AT+MS=VARCQ;+VARC=1333,50,4", "AT+MS=VARCQ");
    assert!(call.connect(70.0), "{}", call.all_notes());
    // The way nobody chose is the widest fit the call passes.
    assert_eq!(call.caller.varc_bauds(), (1333, 3700));
    assert_eq!(call.host.varc_bauds(), (3700, 1333));
    let (n, k) = call.caller.varc_code().expect("a code");
    assert!((k as f64 / n as f64 - 0.5).abs() < 0.03, "a code of {k}/{n}");
    // 1333 baud of four bits, half of them data, less the sync words.
    let rate = call.caller.rate().unwrap_or(0);
    assert!((2_200..2_800).contains(&rate), "{rate} bit/s");
    let sent = page(1500);
    let got = call.send(&sent, 15.0);
    // The page goes the way nobody chose, which is as fast as it bears.
    assert!(got == sent, "{} of {} octets", got.len(), sent.len());
}

#[test]
fn on_a_call_in_g711_the_codewords_are_the_data_in_either_law() {
    for law in [Law::A, Law::Mu] {
        let mut call = Sip::clean(Coding::G711(law), "VARC");
        assert!(call.connect(30.0), "{law:?}:\n{}", call.all_notes());
        assert_eq!(call.host.varc_wanted(), Some(VarcMode::G711));
        assert!(call.told.contains("CONNECT 48640/VARC"), "{law:?}: {}", call.told);
        let got = call.send(b"codewords that are the data", 2.0);
        assert_eq!(said(&got), "codewords that are the data", "{law:?}");
    }
}

#[test]
fn on_a_call_of_exact_samples_the_samples_are_the_data() {
    let mut call = Sip::clean(Coding::Linear, "VARC");
    assert!(call.connect(30.0), "{}", call.all_notes());
    assert_eq!(call.caller.standard(), "V.ARC/L16");
    assert_eq!(call.host.varc_wanted(), Some(VarcMode::Linear));
    assert!(call.told.contains("CONNECT 200434/VARC"), "{}", call.told);
    in_order(&call.notes, &["V.8: both ends have V.ARC", "V.ARC: agreed on 16000 Hz out and 16000 Hz back"]);
    let got = call.send(b"samples that are the data", 2.0);
    assert_eq!(said(&got), "samples that are the data");

    // No carrier, so no symbols in the sense the other modes have them; a
    // sample is sixteen bits at once, and that is what is drawn.
    let points = call.caller.varc_points();
    assert!(!points.is_empty());
    assert!(points.iter().all(|(x, y)| x.abs() <= 1.0 && y.abs() <= 1.0));
    assert_eq!(call.caller.shape(), "L16");
    assert_eq!(call.caller.states(), 65_536);
}

/// The single-carrier modes on a linear call modulate the samples, as
/// they did.
#[test]
fn the_single_carrier_modes_run_on_a_linear_call() {
    for (carrier, name) in [("VARCQ", "V.ARC/QAM"), ("VARCP", "V.ARC/PCM")] {
        let mut call = Sip::clean(Coding::Linear, carrier);
        assert!(call.connect(50.0), "{carrier}:\n{}", call.all_notes());
        assert_eq!(call.caller.standard(), name);
        let rate = call.caller.rate().unwrap_or(0);
        assert!(rate > 40_000, "{carrier}: {rate} bit/s");
        let sent = page(20_000);
        let got = call.send(&sent, 8.0);
        assert!(got == sent, "{carrier}: {} of {} octets", got.len(), sent.len());
    }
}

#[test]
fn a_call_that_is_not_exact_after_all_carries_on_as_the_multicarrier_mode() {
    // A little gain somewhere along the way.
    let mut call = Sip::new((Coding::Linear, Coding::Linear), true, "AT+MS=VARC", "AT+MS=VARC");
    assert!(call.connect(60.0), "{}", call.all_notes());
    assert_eq!(call.caller.standard(), "V.ARC/DMT", "{}", call.told);
    assert!(call.told.contains("/VARC"), "{}", call.told);
    assert!(call.all_notes().contains("does not carry samples exactly"), "{}", call.all_notes());
    // The handshake is not run again: what the two ends agreed stands.
    assert_eq!(call.all_notes().matches("V.ARC: agreed on").count(), 2, "{}", call.all_notes());
    let got = call.send(b"samples that are the data", 3.0);
    assert_eq!(said(&got), "samples that are the data");
    // Sixteen-bit samples are a clean line by any other measure, once the
    // table has followed the line up.
    for _ in 0..(20.0 * 16_000.0) as usize {
        call.tick();
    }
    assert!(call.caller.rate().unwrap_or(0) > 100_000, "{:?} bit/s
{}", call.caller.rate(), call.all_notes());
}

/// A far end that has no V.ARC: V.8 finds that out, and the call is one of
/// the ordinary modes the menu offered, whichever end has V.ARC.
#[test]
fn a_far_end_with_no_varc_gets_an_ordinary_call() {
    for (caller_set, host_set, name) in [
        ("AT+MS=VARC", "AT+MS=V32B", "V.32bis"),
        ("AT+MS=V32B", "AT+MS=VARCQ", "V.32bis"),
        ("AT+MS=VARCP2W", "AT+MS=V34", "V.34"),
        ("AT+MS=V34", "AT+MS=VARC", "V.34"),
    ] {
        let what = format!("{caller_set} calling {host_set}");
        let (mut caller, mut host) = dialled(caller_set, host_set);
        let (mut from_caller, mut from_host) = (0.0, 0.0);
        let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
        let mut notes = String::new();
        let mut typed = false;
        for _ in 0..(60.0 * LINE) as usize {
            assert_eq!(caller.varc_wanted(), None, "{what}");
            assert_eq!(host.varc_wanted(), None, "{what}");
            let (was_caller, was_host) = (from_caller, from_host);
            from_caller = caller.step(was_host);
            from_host = host.step(was_caller);
            at_caller.extend(caller.take_dte());
            at_host.extend(host.take_dte());
            for (who, modem) in [("caller", &mut caller), ("host", &mut host)] {
                for note in modem.take_line_notes() {
                    notes.push_str(&format!("{who}: {note}\n"));
                }
            }
            if caller.state() == State::Data && host.state() == State::Data {
                if !typed {
                    typed = true;
                    at_host.clear();
                    type_at(&mut caller, "an ordinary call after all");
                } else if at_host.len() >= 26 {
                    break;
                }
            }
        }
        assert_eq!(caller.state(), State::Data, "{what}: {}\n{notes}", said(&at_caller));
        assert_eq!(host.state(), State::Data, "{what}\n{notes}");
        assert_eq!(caller.standard(), name, "{what}");
        let told = said(&at_caller);
        assert!(told.contains("CONNECT") && !told.contains("/VARC"), "{what}: {told}");
        assert_eq!(said(&at_host), "an ordinary call after all", "{what}");
        assert!(notes.contains("V.8: the far end has no V.ARC; an ordinary call instead"), "{what}\n{notes}");
        assert!(!notes.contains("both ends have V.ARC"), "{what}\n{notes}");
        // `+MS` still says what was asked for.
        assert!(caller.modulation() == caller_set.trim_start_matches("AT+MS="), "{what}");
    }
}

/// What a call says of its compression: the method each way, and the octets.
type Squeezed = Option<(&'static str, &'static str, [u64; 4])>;

/// A linear call with `sent` written at the calling end once it is up,
/// each end having typed `setup` first: what arrived, and what the calling
/// end says of its compression.
fn compressed(setup: [&str; 2], sent: &[u8]) -> (Vec<u8>, Squeezed) {
    let mut call = Sip::new((Coding::Linear, Coding::Linear), false, &format!("{}AT+MS=VARC", setup[0]), &format!("{}AT+MS=VARC", setup[1]));
    assert!(call.connect(30.0), "{setup:?}:\n{}", call.all_notes());
    let got = call.send(sent, 8.0);
    (got, call.caller.varc_compression())
}

#[test]
fn a_call_compresses_its_stream_with_the_best_both_ends_have() {
    let text: Vec<u8> = (0..120_000).map(|i| b"what the terminal wrote is mostly text, and text is mostly the same words. "[i % 75]).collect();
    for (setup, method) in [
        (["", ""], "LZ"),
        (["AT+VARCZ=1\r", ""], "V.44"),
        (["AT+DS44=0\r", "AT+VARCZ=1\r"], "V.42bis"),
        (["AT+VARCZ=0\r", ""], "none"),
    ] {
        let (got, squeezed) = compressed(setup, &text);
        let (out, back, [plain, coded, _, _]) = squeezed.expect("a V.ARC call");
        assert_eq!(out, method, "{setup:?}");
        assert_eq!(back, method, "{setup:?}");
        assert!(got.ends_with(&text[text.len() - 4000..]), "{setup:?}: {} octets arrived", got.len());
        if method != "none" {
            assert!(coded * 4 < plain, "{setup:?}: {plain} octets sent as {coded}");
        }
    }
}

/// A line made of two sound devices that do not run at the same rate: this
/// end hears on one at 192 kHz and sends on the other at 48 kHz, and the far
/// end the other way about. Each direction is a line of its own.
#[test]
fn two_devices_at_two_rates_are_a_line_each_way() {
    let (fast, slow) = (192_000.0, 48_000.0);
    let (mut caller, mut host) = dialled("AT+MS=VARC", "AT+MS=VARC");
    through_v8(&mut caller, &mut host);
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
    assert_eq!(caller.state(), State::Data, "{}", said(&at_caller));
    assert_eq!(said(&at_host), "each way at its own rate");
    // The fast way carries a good deal more than the slow way does.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(down > 3 * up / 2 && up > 300_000, "{down} arriving at the caller, {up} at the host");
}

/// One pair of wires: both ends send into it, and each hears the other
/// with its own signal on top, come back some milliseconds later through
/// whatever the line is.
#[test]
fn on_one_pair_of_wires_each_end_takes_its_own_echo_out() {
    let (mut caller, mut host) = dialled("AT+MS=VARC", "AT+MS=VARC");
    through_v8(&mut caller, &mut host);
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
        shape.iter().enumerate().map(|(i, s)| s * line[delay + i]).sum()
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
    assert_eq!(caller.state(), State::Data, "{}", said(&at_caller));
    assert_eq!(said(&at_host), "both ways down one pair");
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
    assert!(far.snr_db > 50.0, "the far end reads this end at {:.1} dB", far.snr_db);
    // Each end had the line to itself first and said how that went.
    let told = caller.take_line_notes().join("\n") + &host.take_line_notes().join("\n");
    assert_eq!(told.matches("learned with the line to itself").count(), 2, "{told}");
    eprintln!("two-wire: {rate} bit/s, echo {:.1} dB down", echo[0].0);
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
    let (mut caller, mut host) = dialled(&format!("AT+MS={carrier}"), &format!("AT+MS={carrier}"));
    through_v8(&mut caller, &mut host);
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
    (caller, host, said(&at_caller), said(&at_host), notes)
}

/// The rig that the handshake was written for: each end hears on one sound
/// device and sends on another at a different rate. Two devices cannot
/// cancel an echo, so no echo is looked for. They used to hear nothing and
/// say NO CARRIER.
#[test]
fn ends_on_two_sound_cards_each_look_for_no_echo() {
    let (caller, host, told, got, notes) = across("VARC", (48_000.0, 192_000.0), (192_000.0, 48_000.0), 16.0);
    assert_eq!(caller.state(), State::Data, "{told}\n{notes:?}");
    assert_eq!(host.state(), State::Data);
    assert_eq!(got, "said, then agreed, then sent");
    assert!(caller.varc_echo().is_empty(), "no canceller on a wire each way");
    let all = notes.join("\n");
    assert!(all.contains("the far end hears at 192000 Hz and sends at 48000 Hz"), "{all}");
    assert!(all.contains("cannot cancel an echo"), "{all}");
    assert!(all.contains("agreed on 192000 Hz out and 48000 Hz back"), "{all}");
    assert!(!all.contains("looks for its own echo"), "{all}");
}

/// A card at 192 kHz calling one at 48 kHz. The modulation runs at 48 kHz
/// both ways and the faster end makes up the difference. They used to send
/// each other signals built for different rates.
#[test]
fn a_fast_card_and_a_slow_card_run_at_the_slow_cards_rate() {
    let (caller, host, told, got, notes) = across("VARC", (192_000.0, 192_000.0), (48_000.0, 48_000.0), 16.0);
    assert_eq!(caller.state(), State::Data, "{told}\n{notes:?}");
    assert_eq!(got, "said, then agreed, then sent");
    let all = notes.join("\n");
    assert!(all.contains("agreed on 48000 Hz out and 48000 Hz back"), "{all}");
    assert!(all.contains("the ends' sound cards differ"), "{all}");
    // What a 48 kHz line carries, each way.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(down > 300_000 && up > 300_000, "{down} and {up} bit/s");
}

/// Ends that were asked for different modulations run the multicarrier one.
/// V.8 says only that each has V.ARC; which mode is for the handshake.
#[test]
fn ends_asked_for_different_modulations_agree_on_one() {
    let mut pair = Pair::new("VARCQ");
    type_at(&mut pair.host, "AT+MS=VARCP\rATA\r");
    type_at(&mut pair.caller, "ATD1\r");
    pair.connect(40.0);
    assert_eq!(pair.caller.state(), State::Data, "{}", pair.notes.join("\n"));
    assert_eq!(pair.caller.standard(), "V.ARC/DMT");
    assert_eq!(pair.host.standard(), "V.ARC/DMT");
    assert!(pair.notes.join("\n").contains("the ends asked for different modulations"));
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
                let sinc = if i == 0 { cut } else { (std::f64::consts::PI * cut * t).sin() / (std::f64::consts::PI * t) };
                sinc * (0.5 + 0.5 * (std::f64::consts::PI * t / (half + 1) as f64).cos())
            })
            .collect();
        Self { past: std::collections::VecDeque::from(vec![0.0; taps.len()]), taps }
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
    let (mut caller, mut host) = dialled("AT+MS=VARCQ", "AT+MS=VARCQ");
    through_v8(&mut caller, &mut host);
    let (mut from_caller, mut from_host) = ([0.0], [0.0]);
    let mut dull = Dull::new(FS, 5_000.0);
    let (mut at_caller, mut at_host) = (Vec::new(), Vec::new());
    let (mut caller_notes, mut host_notes) = (Vec::new(), Vec::new());
    let mut typed = false;
    for _ in 0..(95.0 * FS) as usize {
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
    assert_eq!(said(&at_caller), "slower, and through");
    // The dull way is slowed four times: 4000 baud between 375 Hz and
    // 4.9 kHz. The other way is as it was.
    assert_eq!(caller.varc_slowed(), (4, 1), "{told}");
    assert_eq!(host.varc_slowed(), (1, 4), "{heard}");
    assert!(told.contains("asking it to send 2 times slower") && told.contains("4 times slower"), "{told}");
    assert!(heard.contains("the far end cannot hear this end; sending 4 times slower"), "{heard}");
    // And the rates say so: a quarter of the baud one way.
    let (down, up) = (caller.rate().unwrap_or(0), host.rate().unwrap_or(0));
    assert!(down > 10_000 && down < 100_000 && up > 150_000, "{down} down the dull way, {up} up the cable
{told}
--
{heard}");
}

/// The baud, the code and the bits a symbol, chosen by hand at one end for
/// what that end hears: `AT+VARC=8000,50,4`. The other way is left to the
/// modem and is whatever the line bears.
#[test]
fn a_baud_a_code_and_a_grid_can_be_chosen_by_hand() {
    let mut pair = Pair::new("VARCQ");
    type_at(&mut pair.caller, "AT+VARC=8000,50,4\r");
    type_at(&mut pair.caller, "AT+VARC?\r");
    assert!(said(&pair.caller.take_dte()).contains("+VARC: 8000,50,4"));
    type_at(&mut pair.host, "ATA\r");
    type_at(&mut pair.caller, "ATD1\r");
    pair.connect(45.0);
    assert_eq!(pair.caller.state(), State::Data, "{}", said(&pair.at_caller));
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
    assert!((k as f64 / n as f64 - 0.5).abs() < 0.03, "a code of {k}/{n}");
    // 8000 baud of four bits, half of them data, less the sync words.
    let rate = pair.caller.rate().unwrap_or(0);
    assert!((14_000..16_500).contains(&rate), "{rate} bit/s");
    // And the way nobody chose is as fast as it would have been.
    assert!(pair.host.rate().unwrap_or(0) > 100_000, "{:?}", pair.host.rate());
    pair.at_caller.clear();
    type_at(&mut pair.host, "as asked");
    pair.run(1.5);
    assert_eq!(said(&pair.at_caller), "as asked");
}

/// The single-carrier modes fill the band of whatever card they are on: a
/// third of its rate in baud for the carrier, two-thirds for the levels.
#[test]
fn the_single_carrier_modes_run_at_the_cards_own_rate() {
    for (carrier, fs, baud) in [("VARCQ", 48_000.0, 16_000), ("VARCP", 48_000.0, 32_000), ("VARCQ", 192_000.0, 64_000)] {
        let (mut caller, mut host) = dialled(&format!("AT+MS={carrier}"), &format!("AT+MS={carrier}"));
        through_v8(&mut caller, &mut host);
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
        assert!(rate > 4 * baud, "{carrier} at {fs} Hz: {rate} bit/s at {baud} baud");
    }
}

/// An ordinary call between two modems with V.8 bis on, each stepped at
/// the rate of the telephone line until it asks for the sound card as it
/// is, and at the rate of the card from then on. Returns the two and what
/// was noted of the line.
fn bis_call(caller_set: &str, host_set: &str, seconds: f64) -> (Modem, Modem, Vec<String>, String) {
    let (mut caller, mut host) = dialled(&format!("AT+A8E=,,,1{caller_set}"), &format!("AT+A8E=,,,1{host_set}"));
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let (mut wide_caller, mut wide_host) = ([0.0; 2], [0.0; 2]);
    let mut notes = Vec::new();
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
            from_caller = if caller.varc_wanted().is_none() { caller.step(was_host) } else { 0.0 };
            from_host = if host.varc_wanted().is_none() { host.step(was_caller) } else { 0.0 };
        }
        for (who, m) in [("caller", &mut caller), ("host", &mut host)] {
            for line in m.take_line_notes() {
                notes.push(format!("{who}: {line}"));
            }
        }
        if caller.state() == State::Data && host.state() == State::Data {
            break;
        }
    }
    let told = said(&caller.take_dte());
    (caller, host, notes, told)
}

/// `+VARCBIS` at both ends of a call that was not dialled as V.ARC: the
/// V.8 bis transaction finds it, V.8 follows as it follows any
/// transaction, and the call is a V.ARC one.
#[test]
fn an_ordinary_call_between_two_that_list_varc_becomes_one() {
    let (caller, host, notes, told) = bis_call(";+VARCBIS=1", ";+VARCBIS=1", 50.0);
    let all = notes.join("\n");
    for who in ["caller", "host"] {
        let own: Vec<String> = notes.iter().filter(|note| note.starts_with(who)).cloned().collect();
        in_order(&own, &["V.8 bis: both ends have V.ARC; V.8 first", "V.8: both ends have V.ARC", "V.ARC: agreed on", "V.ARC: loaded"]);
    }
    assert_eq!(caller.varc_wanted(), Some(VarcMode::Dmt));
    assert_eq!(host.varc_wanted(), Some(VarcMode::Dmt));
    assert_eq!(caller.state(), State::Data, "{all}");
    assert_eq!(host.state(), State::Data, "{all}");
    assert!(told.contains("CONNECT ") && told.contains("/VARC"), "{told}");
    assert!(caller.rate().unwrap_or(0) > 300_000, "{:?} bit/s", caller.rate());
}

/// The same with it at one end only: nothing of V.ARC happens at all.
#[test]
fn a_call_to_one_that_does_not_list_varc_is_an_ordinary_call() {
    for (caller_set, host_set) in [(";+VARCBIS=1", ""), ("", ";+VARCBIS=1")] {
        let (caller, host, notes, _) = bis_call(caller_set, host_set, 20.0);
        let all = notes.join("\n");
        assert!(!all.contains("both ends have V.ARC"), "{all}");
        assert!(all.contains("V.8 bis: sent MS") || all.contains("V.8 bis: heard MS"), "{all}");
        assert_eq!(caller.varc_wanted(), None);
        assert_eq!(host.varc_wanted(), None);
    }
}

/// A call dialled as V.ARC with V.8 bis on: the transaction lists V.ARC
/// from `+MS`, with no `+VARCBIS`, and the call is V.8 bis, then V.8, then
/// the handshake, then the mode.
#[test]
fn a_varc_call_with_v8_bis_on_runs_v8_bis_then_v8_then_the_handshake() {
    let (caller, host, notes, told) = bis_call(";+MS=VARCQ", ";+MS=VARCQ", 60.0);
    let all = notes.join("\n");
    in_order(&notes, &["V.8 bis: both ends have V.ARC; V.8 first", "V.8: both ends have V.ARC", "V.ARC: agreed on"]);
    assert_eq!(caller.state(), State::Data, "{all}");
    assert_eq!(host.state(), State::Data, "{all}");
    assert_eq!(caller.standard(), "V.ARC/QAM");
    assert!(told.contains("/VARC"), "{told}");
}
