//! Two of these modems with `AT+MS=V92`, one dialling and one answering, on a
//! four-wire line at 48 kHz: what two instances on a pair of virtual cables
//! are. V.8, phase 2 with V.92's INFO sequences, a Table 18 INFO1a, V.92's
//! Phases 3 and 4 at both ends, and data -- PCM going up as well as down.

use std::collections::VecDeque;

use modem::{Modem, State};

const FS: f64 = 16_000.0;

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

/// What a live answerer heard, played into a fresh one: `V92_REPLAY` names a
/// file of little-endian f32 samples at 16 kHz, as
/// `binmodem --answer --record` writes them.
#[test]
#[ignore = "needs a recording"]
fn replay_what_an_answerer_heard() {
    let Some(path) = std::env::var_os("V92_REPLAY") else { return };
    let bytes = std::fs::read(path).expect("no recording");
    let mut host = Modem::new(16_000.0);
    host.set_pcm_upstream(true);
    Pair::type_at(&mut host, "AT+MS=V92");
    Pair::type_at(&mut host, "ATA");
    let mut last = "";
    for (n, chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let x = f32::from_le_bytes(*chunk);
        host.step(f64::from(x));
        let phase = host.line_phase();
        if phase != last {
            last = phase;
            println!("{:.2}s {phase}", n as f64 / 16_000.0);
        }
        for note in host.take_line_notes() {
            println!("{:.2}s   {note}", n as f64 / 16_000.0);
        }
    }
}

/// A V.92 pair with PCM upstream wanted at both ends, not yet dialled.
fn v92_pair() -> Pair {
    let mut pair = Pair::new(1203, 917);
    pair.caller.set_pcm_upstream(true);
    pair.host.set_pcm_upstream(true);
    Pair::type_at(&mut pair.host, "AT+MS=V92");
    Pair::type_at(&mut pair.caller, "AT+MS=V92");
    pair.run(0.05);
    pair
}

/// Dial, and run until both ends are in data mode or `seconds` have gone.
fn connect(pair: &mut Pair, seconds: u32) -> String {
    Pair::type_at(&mut pair.host, "ATA");
    Pair::type_at(&mut pair.caller, "ATD5551234");
    settle(pair, seconds)
}

/// Run until both ends are in data mode with no retrain under way.
fn settle(pair: &mut Pair, seconds: u32) -> String {
    let mut notes = String::new();
    for _ in 0..seconds {
        pair.run(1.0);
        notes.push_str(&pair.notes());
        notes.push_str(&format!(
            "  caller {:?} {}, host {:?} {}
",
            pair.caller.state(),
            pair.caller.line_phase(),
            pair.host.state(),
            pair.host.line_phase()
        ));
        let up = |m: &Modem| m.state() == State::Data && !m.retraining() && m.standard() == "V.92";
        if up(&pair.caller) && up(&pair.host) {
            break;
        }
    }
    notes
}

/// Text both ways, or say what arrived instead.
fn carries_text(pair: &mut Pair, what: &str, notes: &str) {
    pair.run(2.0);
    pair.caller_said.clear();
    pair.host_said.clear();
    for b in b"from the caller" {
        pair.caller.feed_dte(*b);
    }
    for b in b"from the server" {
        pair.host.feed_dte(*b);
    }
    pair.run(3.0);
    let host_saw = String::from_utf8_lossy(&pair.host_said).into_owned();
    assert!(host_saw.contains("from the caller"), "{what}: the server saw {host_saw:?}\n{notes}");
    let caller_saw = String::from_utf8_lossy(&pair.caller_said).into_owned();
    assert!(caller_saw.contains("from the server"), "{what}: the caller saw {caller_saw:?}\n{notes}");
}

fn rates(pair: &Pair) -> (u32, u32) {
    (pair.caller.rate().unwrap_or(0), pair.caller.transmit_rate().unwrap_or(0))
}

/// On a line with nothing on it but the two modems, each end measures as much
/// and takes the top of its ladder: the caller reads it off how TRN1d trained
/// its receiver, and asks for 56 000 down; the server measures as
/// much while the caller is silent under TRN1d, and asks for the top of 6.1's
/// ladder rather than the bottom.
#[test]
fn a_quiet_line_gets_the_fastest_upstream() {
    let mut pair = v92_pair();
    let notes = connect(&mut pair, 40);
    assert_eq!(rates(&pair), (56_000, 48_000), "{notes}");
    carries_text(&mut pair, "the top rung", &notes);
}

/// A rate chosen by hand is the rate the call comes up at, from either end:
/// the server puts it in its CPd, the caller allows no other in its Ja, and
/// the downstream rate is the caller's CPt's.
#[test]
fn a_pinned_rate_is_the_rate_the_call_gets() {
    for (who, up, down) in [("server", 7u8, None), ("caller", 13, None), ("caller", 4, Some(22u8)), ("caller", 1, Some(10))] {
        let mut pair = v92_pair();
        let chooser = if who == "server" { &mut pair.host } else { &mut pair.caller };
        assert_eq!(chooser.choose_upstream(Some(up)), modem::RateChosen::Pinned(Some(up)));
        if down.is_some() {
            pair.caller.choose_rate(down);
        }
        let notes = connect(&mut pair, 40);
        let wanted_up = (u32::from(up) + 17) * 8000 / 6;
        let wanted_down = down.map_or(56_000, |drn| (u32::from(drn) + 20) * 8000 / 6);
        assert_eq!(rates(&pair), (wanted_down, wanted_up), "{who} pinned {up} up, {down:?} down\n{notes}");
        carries_text(&mut pair, &format!("{who} pinned {up} up, {down:?} down"), &notes);
    }
}

/// And with the call up: the choice sends it back through its start-up, and
/// it comes back at the new rate still carrying data.
#[test]
fn a_rate_chosen_during_a_call_retrains_to_it() {
    for who in ["server", "caller"] {
        let mut pair = v92_pair();
        let mut notes = connect(&mut pair, 40);
        assert_eq!(rates(&pair).1, 48_000, "{who}\n{notes}");
        let chooser = if who == "server" { &mut pair.host } else { &mut pair.caller };
        assert_eq!(chooser.choose_upstream(Some(7)), modem::RateChosen::Retraining(Some(7)), "{who}");
        pair.run(1.0);
        notes.push_str(&settle(&mut pair, 60));
        assert_eq!(rates(&pair), (56_000, 32_000), "{who}\n{notes}");
        carries_text(&mut pair, who, &notes);
    }
}

/// V.8 bis in front of it all (`AT+A8E=,,,1` at both ends): the transaction
/// chooses the mode, V.8 follows it, and the call is V.92 with PCM both ways
/// as it is without.
#[test]
fn v8_bis_in_front_of_a_v92_call() {
    let mut pair = v92_pair();
    Pair::type_at(&mut pair.host, "AT+A8E=,,,1");
    Pair::type_at(&mut pair.caller, "AT+A8E=,,,1");
    pair.run(0.05);
    let notes = connect(&mut pair, 50);
    assert!(notes.contains("V.8 bis"), "no V.8 bis on the line:\n{notes}");
    assert_eq!(rates(&pair), (56_000, 48_000), "{notes}");
    carries_text(&mut pair, "after V.8 bis", &notes);
}

// ---------------------------------------------------------------------------
// Quick connect (V.92 9.2): the short Phase 1
// ---------------------------------------------------------------------------

/// Seconds from dialling to both ends in data mode on V.92, to a twentieth.
fn timed_connect(pair: &mut Pair, what: &str) -> f64 {
    Pair::type_at(&mut pair.host, "ATA");
    Pair::type_at(&mut pair.caller, "ATD5551234");
    let up = |m: &Modem| m.state() == State::Data && !m.retraining() && m.standard() == "V.92";
    let mut notes = String::new();
    for _ in 0..800 {
        pair.run(0.05);
        notes.push_str(&pair.notes());
        if up(&pair.caller) && up(&pair.host) {
            break;
        }
    }
    assert_eq!((pair.caller.state(), pair.host.state()), (State::Data, State::Data), "{what}:
{notes}");
    assert_eq!((pair.caller.standard(), pair.host.standard()), ("V.92", "V.92"), "{what}:
{notes}");
    pair.caller.call_seconds()
}

fn hang_up(pair: &mut Pair) {
    pair.caller.hang_up();
    pair.host.hang_up();
    pair.run(0.5);
    pair.caller_said.clear();
    pair.host_said.clear();
}

/// The first call to a place is the full start-up and leaves a memo; the
/// second, handed the memo, opens with QC1a and is at data mode sooner, with
/// V.42 up and text crossing as before.
#[test]
fn a_second_call_to_the_same_place_starts_up_the_short_way() {
    let mut pair = v92_pair();
    let first = timed_connect(&mut pair, "first call");
    assert_eq!(pair.caller.quick_connected(), None, "nothing was known, so nothing was asked");
    assert!(!pair.caller.short_phase2() && !pair.host.short_phase2(), "only the host asked for a short Phase 2");
    assert_eq!(pair.host.quick_connected(), None);
    let memo = pair.caller.learned().expect("a V.92 call in data mode teaches something");
    assert!(memo.pcm_upstream, "{memo:?}");
    assert_eq!(pair.host.learned(), None, "the answering end has no number to remember");
    hang_up(&mut pair);

    pair.caller.set_quick_memo(Some(memo));
    let second = timed_connect(&mut pair, "second call");
    assert_eq!(pair.caller.quick_connected(), Some(true), "QC1a and QCA1d both called for LAPM");
    assert_eq!(pair.host.quick_connected(), Some(true));
    // And Phase 2 was the short one at both ends: the memo says the last
    // call had PCM upstream, so the caller asked, and the host always does.
    assert!(pair.caller.short_phase2() && pair.host.short_phase2());
    println!("full start-up {first:.2} s, with short Phases 1 and 2 {second:.2} s");
    assert!(second < first - 2.0, "full {first:.2} s, short {second:.2} s");
    // What the short call teaches is what the full one did: the round trip
    // it could not measure is the one it was handed.
    assert_eq!(pair.caller.learned(), Some(memo));
    // 9.2.5: both sequences called for LAPM, so there was no ODP and no ADP
    // -- and LAPM is up all the same.
    assert!(pair.caller.error_controlled() && pair.host.error_controlled());
    let answered = |m: &Modem| m.distant().into_iter().find(|(what, _)| *what == "answered").map(|(_, said)| said);
    assert_eq!(answered(&pair.caller).as_deref(), Some("nothing"), "an ADP came back: {:?}", pair.caller.distant());
    carries_text(&mut pair, "after a quick connect", "");
}

/// A QC1a sent to a modem that does not answer one is ignored, and the CM
/// behind it carries the call on by V.8.
#[test]
fn a_quick_connect_nobody_answers_is_a_full_start_up() {
    let mut pair = v92_pair();
    pair.caller.set_quick_connect(modem::QuickConnect::Always);
    pair.host.set_quick_connect(modem::QuickConnect::Off);
    timed_connect(&mut pair, "QC1a to a modem that does not answer it");
    assert_eq!(pair.caller.quick_connected(), None);
    assert_eq!(pair.host.quick_connected(), None);
    carries_text(&mut pair, "after a QC1a nobody answered", "");
}

/// 9.2.4.3: the digital modem's ANSpcm goes unanswered -- here because the
/// line to the caller is dead while it is sent -- so it sends ANSam again,
/// the caller hears that and sends its CM, and the call is V.8's.
#[test]
fn an_unanswered_anspcm_falls_back_to_v8_and_connects() {
    let mut pair = v92_pair();
    pair.caller.set_quick_connect(modem::QuickConnect::Always);
    Pair::type_at(&mut pair.host, "ATA");
    Pair::type_at(&mut pair.caller, "ATD5551234");
    let mut notes = String::new();
    let mut cut = 0;
    for _ in 0..(40.0 * FS) as usize {
        let heard = pair.to_caller.pop_front().unwrap_or(0.0);
        pair.to_host.push_back(pair.caller.step(heard));
        let heard = pair.to_host.pop_front().unwrap_or(0.0);
        let said = pair.host.step(heard);
        // From the host's QCA1d silence on, nothing reaches the caller until
        // the host has given up.
        let quick = matches!(pair.host.line_phase(), "QCA1d silence" | "QTS" | "QTS-bar" | "ANSpcm");
        cut += usize::from(quick);
        pair.to_caller.push_back(if quick { 0.0 } else { said });
        if pair.caller.state() == State::Data && pair.host.state() == State::Data {
            break;
        }
    }
    notes.push_str(&pair.notes());
    assert!(cut > 0, "the host never answered the QC1a:\n{notes}");
    assert!(notes.contains("no TONEq"), "{notes}");
    assert_eq!((pair.caller.state(), pair.host.state()), (State::Data, State::Data), "{notes}");
    assert_eq!((pair.caller.quick_connected(), pair.host.quick_connected()), (None, None));
    assert_eq!((pair.caller.standard(), pair.host.standard()), ("V.92", "V.92"), "{notes}");
}

/// A modem handed a file keeps the memos itself, by the string dialled: the
/// second call to the same number is the short start-up with nobody having
/// carried anything across, and a call to another number is not.
#[test]
fn a_modem_with_a_file_remembers_the_number_it_dialled() {
    let file = std::env::temp_dir().join(format!("binmodem-v92-call-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut pair = v92_pair();
    pair.caller.remember_connections(Some(modem::memos::Memos::open(&file)));
    timed_connect(&mut pair, "first call");
    assert_eq!(pair.caller.quick_connected(), None);
    hang_up(&mut pair);
    assert!(modem::memos::Memos::open(&file).get("5551234").is_some_and(|m| m.pcm_upstream));

    timed_connect(&mut pair, "second call");
    assert_eq!(pair.caller.quick_connected(), Some(true));
    assert!(pair.caller.short_phase2() && pair.host.short_phase2());
    hang_up(&mut pair);

    // Another number: nothing is known of it.
    Pair::type_at(&mut pair.host, "ATA");
    Pair::type_at(&mut pair.caller, "ATD5559999");
    settle(&mut pair, 40);
    assert_eq!(pair.caller.state(), State::Data);
    assert_eq!(pair.caller.quick_connected(), None);
    hang_up(&mut pair);

    // The first number again, and the far end is not a V.92 server any more:
    // the QC1a goes unanswered, the call is V.34 by V.8, and the memo that
    // was spent on it is gone.
    Pair::type_at(&mut pair.host, "AT+MS=V34");
    pair.run(0.05);
    Pair::type_at(&mut pair.host, "ATA");
    Pair::type_at(&mut pair.caller, "ATD5551234");
    for _ in 0..40 {
        pair.run(1.0);
        if pair.caller.state() == State::Data && pair.host.state() == State::Data {
            break;
        }
    }
    assert_eq!((pair.caller.state(), pair.host.state()), (State::Data, State::Data));
    assert_eq!(pair.caller.standard(), "V.34");
    hang_up(&mut pair);
    let kept = modem::memos::Memos::open(&file);
    assert_eq!(kept.get("5551234"), None, "a memo that did not get its call to V.92 is not spent twice");
    assert!(kept.get("5559999").is_some());
    let _ = std::fs::remove_file(&file);
}

/// What became of a quick connect whose server goes quiet once it has
/// heard TONEq, as the NetZero server in live-1795315036 did.
struct Silenced {
    notes: String,
    /// Seconds from the dial to when the host stopped being heard.
    quiet_at: f64,
    /// Seconds from the dial to when the caller was back on hook, if it was.
    ended_at: Option<f64>,
}

/// Dial a number the caller has a memo of (in `file`), and from the moment
/// the host stops ANSpcm let nothing more of it reach the caller. With
/// `restart`, the host also hangs up then and answers again, as a server
/// whose quick connect went wrong might go back to ANSam and V.8 -- and from
/// then on it is heard as usual.
fn quick_connect_into_silence(pair: &mut Pair, file: &std::path::Path, restart: bool, seconds: f64) -> Silenced {
    let mut memos = modem::memos::Memos::open(file);
    memos.put("5551234", modem::Recognized { uqts: 70, a_law: false, round_trip_ms: 133, pcm_upstream: true });
    pair.caller.remember_connections(Some(modem::memos::Memos::open(file)));
    Pair::type_at(&mut pair.host, "ATA");
    Pair::type_at(&mut pair.caller, "ATD5551234");
    let mut notes = String::new();
    let (mut was_anspcm, mut quiet, mut quiet_at) = (false, false, 0.0);
    let (mut restarted, mut ended_at) = (false, None);
    for i in 0..(seconds * FS) as usize {
        let t = i as f64 / FS;
        let heard = pair.to_caller.pop_front().unwrap_or(0.0);
        pair.to_host.push_back(pair.caller.step(heard));
        pair.caller_said.extend(pair.caller.take_dte());
        let heard = pair.to_host.pop_front().unwrap_or(0.0);
        let said = pair.host.step(heard);
        pair.host_said.extend(pair.host.take_dte());
        let anspcm = pair.host.line_phase() == "ANSpcm";
        if was_anspcm && !anspcm && !quiet {
            quiet = true;
            quiet_at = t;
            if restart {
                pair.host.hang_up();
                Pair::type_at(&mut pair.host, "ATA");
                restarted = true;
            }
        }
        was_anspcm |= anspcm;
        pair.to_caller.push_back(if quiet && !restarted { 0.0 } else { said });
        notes.push_str(&pair.notes());
        if quiet && pair.caller.state() == State::Command {
            ended_at = Some(t);
            break;
        }
        if pair.caller.state() == State::Data && pair.host.state() == State::Data {
            break;
        }
    }
    notes.push_str(&pair.notes());
    assert!(quiet, "the host never sent ANSpcm:\n{notes}");
    Silenced { notes, quiet_at, ended_at }
}

/// live-1795315036: the server heard TONEq, stopped ANSpcm, and said nothing
/// more. The caller gives up on that Phase 2 a round trip and 2.5 s in,
/// forgets the memo there and then, sends CM in case the server has gone
/// back to V.8, and when no JM comes either hangs up -- all well inside the
/// twenty seconds phase 2 would otherwise have sat through.
#[test]
fn a_server_that_goes_silent_after_toneq_loses_its_memo_and_the_call_ends() {
    let file = std::env::temp_dir().join(format!("binmodem-v92-silent-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut pair = v92_pair();
    let run = quick_connect_into_silence(&mut pair, &file, false, 30.0);
    let caller_saw = String::from_utf8_lossy(&pair.caller_said).into_owned();
    let notes = format!("{}\nthe terminal saw {caller_saw:?}", run.notes);
    assert!(run.notes.contains("no INFO0d or tone B after TONEq; memo dropped"), "{notes}");
    assert!(run.notes.contains("no JM to the CM either"), "{notes}");
    assert_eq!(pair.caller.state(), State::Command, "{notes}");
    assert!(caller_saw.contains("NO CARRIER"), "{notes}");
    let took = run.ended_at.expect("the caller never hung up") - run.quiet_at;
    println!("on hook {took:.2} s after the server went quiet");
    assert!(took < 12.0, "the call went on {took:.1} s after the server went quiet\n{notes}");
    assert_eq!(modem::memos::Memos::open(&file).get("5551234"), None, "the memo is still there");
    let _ = std::fs::remove_file(&file);
}

/// The same, with a server that does go back to ANSam: the caller's CM
/// finds it, and the call is a full start-up -- V.8, the full Phase 2 --
/// to V.92 data mode, with no memo left to try the short way again.
#[test]
fn a_server_that_restarts_v8_after_a_failed_quick_connect_still_connects() {
    let file = std::env::temp_dir().join(format!("binmodem-v92-restart-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut pair = v92_pair();
    let run = quick_connect_into_silence(&mut pair, &file, true, 40.0);
    assert!(run.notes.contains("memo dropped"), "{}", run.notes);
    assert_eq!((pair.caller.state(), pair.host.state()), (State::Data, State::Data), "{}", run.notes);
    assert_eq!((pair.caller.standard(), pair.host.standard()), ("V.92", "V.92"), "{}", run.notes);
    assert_eq!(pair.caller.quick_connected(), None, "{}", run.notes);
    assert!(!pair.caller.short_phase2(), "{}", run.notes);
    carries_text(&mut pair, "after a quick connect went back to V.8", &run.notes);
    hang_up(&mut pair);
    // The call reached data mode, so it taught a memo afresh: a full
    // start-up's, which is what a quick connect next time would rest on.
    assert!(modem::memos::Memos::open(&file).get("5551234").is_some());
    let _ = std::fs::remove_file(&file);
}
