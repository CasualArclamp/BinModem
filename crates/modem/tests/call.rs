//! Two modems calling each other, driven the way a terminal drives one.
//!
//! Nothing here reaches past the two interfaces a modem actually has: bytes to
//! and from the terminal, and samples to and from the line. If a thing cannot
//! be done through those, a person with a terminal and a telephone line cannot
//! do it either.

use modem::{Modem, State};

const FS: f64 = 16_000.0;

/// Both ends of a call, joined by a line that sums the two directions.
struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: f64,
    from_host: f64,
    /// Everything each end has said to its terminal.
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
}

impl Pair {
    fn new() -> Self {
        Self {
            caller: Modem::new(FS),
            host: Modem::new(FS),
            from_caller: 0.0,
            from_host: 0.0,
            at_caller: Vec::new(),
            at_host: Vec::new(),
        }
    }

    /// Type a command line at one end.
    fn type_at(modem: &mut Modem, line: &str) {
        for b in line.bytes() {
            modem.feed_dte(b);
        }
        modem.feed_dte(b'\r');
    }

    /// Run the line for `seconds`, collecting what each terminal is told.
    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(b);
            self.from_host = self.host.step(a);
            self.at_caller.extend(self.caller.take_dte());
            self.at_host.extend(self.host.take_dte());
        }
    }

    fn caller_saw(&self) -> String {
        String::from_utf8_lossy(&self.at_caller).into_owned()
    }

    fn host_saw(&self) -> String {
        String::from_utf8_lossy(&self.at_host).into_owned()
    }
}

/// Place a call and wait for both ends to report a connection.
fn connect() -> Pair {
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(10.0);
    p
}

#[test]
fn a_terminal_talks_to_the_modem_before_there_is_a_call() {
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT");
    p.run(0.01);
    assert!(
        p.caller_saw().contains("OK"),
        "a bare AT was answered with {:?}",
        p.caller_saw()
    );
    assert_eq!(p.caller.state(), State::Command);
}

#[test]
fn dialling_reaches_a_connection_and_says_so() {
    let p = connect();
    assert_eq!(p.caller.state(), State::Data, "the caller is not online");
    assert_eq!(p.host.state(), State::Data, "the host is not online");
    for (name, saw) in [("caller", p.caller_saw()), ("host", p.host_saw())] {
        assert!(
            saw.contains("CONNECT"),
            "the {name}'s terminal was told {saw:?} rather than CONNECT"
        );
    }
    // V.250 6.2.7: with X at its default the rate is reported, since it is the
    // only way a terminal learns what it got rather than what it asked for.
    //
    // Which rate is not this test's business and is no longer fixed: with
    // automode on, a plain ATD negotiates through V.8 and comes out at
    // whichever modulation both ends liked best. What has to hold is that the
    // number the terminal was told is the number the modem actually got.
    let rate = p.caller.rate().expect("connected without a rate");
    assert!(
        p.caller_saw().contains(&rate.to_string()),
        "CONNECT carried no rate: {:?}",
        p.caller_saw()
    );
    assert_eq!(p.caller.rate(), p.host.rate(), "the two ends disagree");
}

#[test]
fn typing_at_one_terminal_comes_out_at_the_other() {
    let mut p = connect();
    assert_eq!(p.caller.state(), State::Data);
    // Let error control finish establishing before speaking.
    p.run(2.0);
    p.at_caller.clear();
    p.at_host.clear();

    for b in b"cactus\r\n" {
        p.caller.feed_dte(*b);
    }
    p.run(3.0);
    assert!(
        p.host_saw().contains("cactus"),
        "the host's terminal saw {:?}",
        p.host_saw()
    );

    for b in b"Password:" {
        p.host.feed_dte(*b);
    }
    p.run(3.0);
    assert!(
        p.caller_saw().contains("Password:"),
        "the caller's terminal saw {:?}",
        p.caller_saw()
    );
}

#[test]
fn error_control_comes_up_on_its_own() {
    // V.42 is not something the terminal asks for. The originator offers, the
    // answerer accepts, and neither terminal is told anything about it beyond
    // what the CONNECT says.
    let mut p = connect();
    p.run(3.0);
    assert!(
        p.caller.error_controlled(),
        "the caller has no error control"
    );
    assert!(p.host.error_controlled(), "the host has no error control");
}

#[test]
fn compression_is_agreed_without_either_terminal_asking() {
    // V.42bis is negotiated in XID during the connection, and what runs is the
    // intersection of the two offers. Neither terminal is consulted.
    let mut p = connect();
    p.run(3.0);
    assert!(p.caller.compressing(), "the caller is not compressing");
    assert!(p.host.compressing(), "the host is not compressing");
}

#[test]
fn a_far_end_without_error_control_still_carries_data() {
    // The case V.42 7.2.1 exists for. A modem that treated a far end without
    // error control as a failure would refuse connections that work perfectly
    // well, which is most of what was answering telephones when V.42 was new.
    let mut p = Pair::new();
    p.host.set_error_control(false);
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(12.0);

    assert_eq!(p.caller.state(), State::Data, "the caller never connected");
    assert_eq!(p.host.state(), State::Data, "the host never connected");
    assert!(
        !p.caller.error_controlled(),
        "error control was agreed with an end that does not do it"
    );

    p.at_host.clear();
    for b in b"cactus" {
        p.caller.feed_dte(*b);
    }
    p.run(3.0);
    assert!(
        p.host_saw().contains("cactus"),
        "an unprotected connection carried {:?}",
        p.host_saw()
    );
}

#[test]
fn the_escape_sequence_returns_to_command_state_without_dropping_the_call() {
    // V.250 6.1.4. The point of the guard time either side is that a file
    // containing three plusses must not drop the call carrying it, which is
    // why the sequence alone is not enough.
    let mut p = connect();
    p.run(2.0);
    p.at_caller.clear();

    // Quiet, the sequence, then quiet again.
    p.run(1.5);
    for _ in 0..3 {
        p.caller.feed_dte(b'+');
    }
    p.run(1.5);

    assert_eq!(
        p.caller.state(),
        State::OnlineCommand,
        "the escape did not take"
    );
    assert!(
        p.caller_saw().contains("OK"),
        "no OK after escaping: {:?}",
        p.caller_saw()
    );
    assert_eq!(p.host.state(), State::Data, "the far end lost the call");

    // And back again.
    Pair::type_at(&mut p.caller, "ATO");
    p.run(0.5);
    assert_eq!(p.caller.state(), State::Data, "ATO did not return online");
}

#[test]
fn three_plusses_in_the_middle_of_data_are_just_data() {
    // The case the guard time exists for.
    let mut p = connect();
    p.run(2.0);
    for b in b"a+++b" {
        p.caller.feed_dte(*b);
    }
    p.run(0.5);
    assert_eq!(
        p.caller.state(),
        State::Data,
        "plusses inside a stream of data dropped the call out of it"
    );
}

#[test]
fn hanging_up_ends_the_call_at_both_ends() {
    let mut p = connect();
    p.run(2.0);
    p.at_caller.clear();
    p.at_host.clear();

    // Escape first, as a terminal must: ATH is a command and commands are not
    // read while the modem is passing data.
    p.run(1.5);
    for _ in 0..3 {
        p.caller.feed_dte(b'+');
    }
    p.run(1.5);
    Pair::type_at(&mut p.caller, "ATH0");
    // Long enough for the far end to notice the carrier has gone, which is a
    // thing it can only do by waiting.
    p.run(6.0);

    assert_eq!(p.caller.state(), State::Command, "the caller stayed online");
    assert_eq!(
        p.host.state(),
        State::Command,
        "the host did not notice the carrier go"
    );
    assert!(
        p.host_saw().contains("NO CARRIER"),
        "the host's terminal saw {:?}",
        p.host_saw()
    );
}

#[test]
fn a_call_to_nobody_gives_up_and_says_so() {
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "ATD5551234");
    // The far end never answers. The handshake's own patience is a minute, so
    // this only checks that it is still trying rather than that it has stopped.
    p.run(5.0);
    assert_eq!(p.caller.state(), State::Handshaking);
    assert!(
        !p.caller_saw().contains("CONNECT"),
        "connected to nothing: {:?}",
        p.caller_saw()
    );
}

#[test]
fn what_is_typed_while_dialling_is_not_treated_as_a_command() {
    // A terminal that types during a dial must not have it parsed, and must
    // not have it delivered as a burst the moment a connection comes up
    // either. What it does instead is stop the dial: V.250 5.6.1, and the
    // abortability clause of the D command.
    //
    // The distinction is visible in what comes back. A parsed ATH would
    // answer OK; an aborted dial answers NO CARRIER, and the T and the H
    // never reach a parser at all.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(0.5);
    let before = p.caller_saw().len();
    Pair::type_at(&mut p.caller, "ATH");
    p.run(1.0);

    assert_eq!(p.caller.state(), State::Command, "carried on dialling");
    let after = &p.caller_saw()[before..];
    assert!(
        after.contains("NO CARRIER"),
        "did not report the dial as abandoned: {after:?}"
    );
    assert!(
        !after.contains("OK"),
        "the ATH was parsed as a command: {after:?}"
    );
}

#[test]
fn ms_chooses_which_modulation_the_call_uses() {
    // V.250 6.4.1. Both ends have to be told, because a modulation is not
    // negotiated across the whole set: V.22bis and V.32 do not share a
    // handshake and a modem listening for one hears nothing of the other.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=V32");
    Pair::type_at(&mut p.caller, "AT+MS=V32");
    p.run(0.01);
    assert!(p.caller_saw().contains("OK"), "{:?}", p.caller_saw());

    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(12.0);

    assert_eq!(p.caller.state(), State::Data, "the V.32 caller never connected");
    assert_eq!(p.host.state(), State::Data, "the V.32 host never connected");
    // Both ends offer 4800 and 9600, and the rate exchange settles on the
    // better of what both can do.
    assert_eq!(p.caller.rate(), Some(9600), "not the V.32 rate");
    assert!(
        p.caller_saw().contains("9600"),
        "CONNECT did not report the V.32 rate: {:?}",
        p.caller_saw()
    );
}

#[test]
fn a_v32_call_carries_data_both_ways() {
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=V32");
    Pair::type_at(&mut p.caller, "AT+MS=V32");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(14.0);
    assert_eq!(p.caller.state(), State::Data);

    p.at_caller.clear();
    p.at_host.clear();
    for b in b"cactus
" {
        p.caller.feed_dte(*b);
    }
    for b in b"Password:" {
        p.host.feed_dte(*b);
    }
    p.run(4.0);
    assert!(
        p.host_saw().contains("cactus"),
        "the host saw {:?}",
        p.host_saw()
    );
    assert!(
        p.caller_saw().contains("Password:"),
        "the caller saw {:?}",
        p.caller_saw()
    );
}

#[test]
fn turning_compression_off_is_obeyed() {
    // AT+DS=0. Error control stays, and only the compression goes.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+DS=0");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(12.0);
    assert!(p.caller.error_controlled(), "lost error control as well");
    assert!(
        !p.caller.compressing() && !p.host.compressing(),
        "compression was used after being turned off"
    );
}

#[test]
fn turning_error_control_off_is_obeyed() {
    // AT+ES=0 is direct mode: no V.42 at all, and the characters go down the
    // line with nothing but their own start and stop bits.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+ES=0");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(12.0);
    assert_eq!(p.caller.state(), State::Data);
    assert!(!p.caller.error_controlled(), "V.42 ran after +ES=0");

    p.at_host.clear();
    for b in b"cactus" {
        p.caller.feed_dte(*b);
    }
    p.run(3.0);
    assert!(
        p.host_saw().contains("cactus"),
        "direct mode carried {:?}",
        p.host_saw()
    );
}

#[test]
fn a_bell_103_call_carries_a_bbs_session() {
    // The oldest thing this modem can do, and the one a board from 1985 would
    // recognise. No error control, no compression, no negotiation to speak
    // of: the answering end whistles, the calling end whistles back, and
    // whatever is typed goes down the line as start-stop characters.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=B103");
    Pair::type_at(&mut p.caller, "AT+MS=B103");
    p.run(0.01);
    assert!(p.caller_saw().contains("OK"), "{:?}", p.caller_saw());

    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(5.0);

    assert_eq!(p.caller.state(), State::Data, "the 300 bit/s caller never connected");
    assert_eq!(p.host.state(), State::Data, "the 300 bit/s host never connected");
    assert_eq!(p.caller.rate(), Some(300));
    assert!(
        p.caller_saw().contains("300"),
        "CONNECT did not report the rate: {:?}",
        p.caller_saw()
    );
    // Nothing from 1985 has heard of V.42, and this pump could not carry it
    // if it had: the line format is already start-stop.
    assert!(!p.caller.error_controlled());

    let banner = "\r\nThe Dead Zone BBS\r\nLogin: ";
    for b in banner.bytes() {
        p.host.feed_dte(b);
    }
    Pair::type_at(&mut p.caller, "guest");
    // Thirty characters a second, so this takes a moment.
    p.run(3.0);

    assert!(
        p.caller_saw().contains(banner),
        "the banner did not come through: {:?}",
        p.caller_saw()
    );
    assert!(
        p.host_saw().contains("guest\r"),
        "the login did not go down the line: {:?}",
        p.host_saw()
    );
}

#[test]
fn a_scope_can_see_what_the_modem_is_doing() {
    // Everything the live window puts on screen comes through these, and none
    // of it is visible from the terminal side, which sees a CONNECT and a rate
    // and nothing else. If they lie, the scope lies.
    let mut p = Pair::new();
    assert!(!p.caller.off_hook(), "on hook before a call");
    assert_eq!(p.caller.standard(), "V.22bis", "the default modulation");
    assert_eq!(p.caller.constellation_point(), None, "a point with no call");

    Pair::type_at(&mut p.host, "AT+MS=V32");
    Pair::type_at(&mut p.caller, "AT+MS=V32");
    p.run(0.01);
    assert_eq!(p.caller.standard(), "V.32", "+MS did not change what is reported");

    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(0.5);
    assert!(p.caller.off_hook(), "still on hook while dialling");
    // V.8 comes first and is not V.32, and says so rather than claiming to be
    // a modulation it is only choosing between.
    assert_eq!(p.caller.standard(), "V.8", "did not negotiate first");

    // Once it has chosen, the V.32 start-up runs entirely in the four states
    // whatever rate is being negotiated, so a scope watching it sees four.
    p.run(4.0);
    assert_eq!(p.caller.standard(), "V.32", "never reached the modulation");
    assert_eq!(p.caller.states(), 4);

    p.run(13.0);
    assert_eq!(p.caller.state(), State::Data, "never connected");
    assert_eq!(p.caller.rate(), Some(9600));
    // And sixteen once the rate exchange has settled on 9600.
    assert_eq!(p.caller.states(), 16);
    assert_eq!(p.caller.shape(), "16QAM");
    assert!(p.caller.carrier(), "connected with no carrier");
    let point = p.caller.constellation_point().expect("no point once connected");
    let radius = point.0.hypot(point.1);
    // Sixteen points on three rings, normalised by the constellation's own
    // root-mean-square: four inner corners at sqrt(2/10) = 0.447, eight at 1,
    // and four outer at sqrt(18/10) = 1.342. Any of the three is a correct
    // answer, and which one this is depends on the byte being carried when the
    // run stopped -- so the range has to hold all of them. The bound started at
    // 0.5, which excluded the inner ring, and passed for as long as nothing
    // landed on it.
    assert!(
        (0.35..2.0).contains(&radius),
        "the constellation is at radius {radius:.2}, so the scope would draw it \
         off the edge or in a dot"
    );
    let error = p.caller.residual_error().expect("no residual error");
    assert!(error < 0.3, "residual error {error:.2} on a clean line");
    // FSK has no constellation and QAM has no discriminator: each modulation
    // offers the scope the one it actually has.
    assert_eq!(p.caller.discriminator(), None);
}

#[test]
fn a_three_hundred_baud_scope_gets_an_eye_and_not_a_constellation() {
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=B103");
    Pair::type_at(&mut p.caller, "AT+MS=B103");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(5.0);
    assert_eq!(p.caller.state(), State::Data);
    assert_eq!(p.caller.standard(), "Bell 103");
    assert_eq!(p.caller.shape(), "2FSK");
    assert_eq!(p.caller.states(), 2);
    assert_eq!(p.caller.constellation_point(), None, "FSK has no constellation");
    let level = p.caller.discriminator().expect("no discriminator");
    assert!(
        level > 0.5,
        "an idle line sits at mark, so the discriminator should read near +1, \
         not {level:.2}"
    );
}

/// Root mean square and peak of a second of one modulation, once it is up.
fn level_of(carrier: &str, seconds: f64) -> (f64, f64) {
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, &format!("AT+MS={carrier}"));
    Pair::type_at(&mut p.caller, &format!("AT+MS={carrier}"));
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(seconds);
    assert_eq!(p.caller.state(), State::Data, "{carrier} never connected");

    // Measure the caller alone, with something to say, so the figure is a
    // modem carrying data rather than one idling.
    for b in b"the quick brown fox jumps over the lazy dog " {
        p.caller.feed_dte(*b);
    }
    let (mut sum, mut peak, mut n) = (0.0f64, 0.0f64, 0u32);
    let (mut a, mut b) = (p.from_caller, p.from_host);
    for _ in 0..(FS as usize) {
        let out = p.caller.step(b);
        b = p.host.step(a);
        a = out;
        p.caller.take_dte();
        p.host.take_dte();
        sum += out * out;
        peak = peak.max(out.abs());
        n += 1;
    }
    ((sum / f64::from(n)).sqrt(), peak)
}

#[test]
fn every_modulation_goes_out_at_the_same_level() {
    // A real modem transmits at a level the network expects and does not
    // change it because the modulation changed, so neither does this one. It
    // is also what makes a single drive control on the line honest: one
    // setting has to mean the same power whichever of these is running.
    let mut measured = Vec::new();
    for (carrier, seconds) in [("B103", 5.0), ("V22B", 10.0), ("V32", 14.0)] {
        let (rms, peak) = level_of(carrier, seconds);
        println!("{carrier:>5}: rms {rms:.3}  peak {peak:.3}  crest {:.2}", peak / rms);
        measured.push((carrier, rms, peak));
    }
    let quietest = measured.iter().map(|m| m.1).fold(f64::MAX, f64::min);
    let loudest = measured.iter().map(|m| m.1).fold(0.0, f64::max);
    assert!(
        20.0 * (loudest / quietest).log10() < 1.0,
        "the modulations differ by more than a decibel: {measured:?}"
    );

    // What they do differ in, enormously, is how peaky they are at that same
    // power. Frequency shift keying has a constant envelope and sits at its
    // peak permanently; a shaped constellation goes nearly three times above
    // its own average. Anything choosing a transmit level has to leave room
    // for the worst of them or the peaks are simply flattened, and a receiver
    // trains happily on a clipped constellation because every outer point has
    // moved inwards together.
    let crest = |name: &str| {
        let m = measured.iter().find(|m| m.0 == name).expect("not measured");
        m.2 / m.1
    };
    assert!(
        (1.35..1.50).contains(&crest("B103")),
        "constant envelope should crest at the root of two, not {:.2}",
        crest("B103")
    );
    assert!(
        crest("V32") > 2.5,
        "a shaped constellation crests far above its average, not at {:.2}",
        crest("V32")
    );
}

#[test]
#[ignore]
fn report_levels() {
    for (carrier, seconds) in [("B103", 5.0), ("V22B", 10.0), ("V32", 14.0)] {
        let (rms, peak) = level_of(carrier, seconds);
        println!(
            "{carrier:>5}: rms {rms:.3} ({:>6.2} dB)   peak {peak:.3}   crest {:.2}",
            20.0 * rms.log10(),
            peak / rms
        );
    }
}

#[test]
fn typing_during_a_call_attempt_gives_up_on_it() {
    // V.250 5.6.1 and the abortability clause of the D command: a single
    // character from the terminal while a call is being placed is an
    // instruction to stop, and the modem "disconnects from the line in an
    // orderly manner". Dropping those characters instead leaves a terminal
    // with no way back from a handshake that is not going to finish, short of
    // waiting out the whole patience of the modem, which is a minute.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(1.0);
    assert_eq!(p.caller.state(), State::Handshaking, "never went off hook");

    p.caller.feed_dte(b'x');
    p.run(0.05);
    assert_eq!(p.caller.state(), State::Command, "went on regardless");
    assert!(
        p.caller_saw().contains("NO CARRIER"),
        "said nothing about giving up: {:?}",
        p.caller_saw()
    );

    // And the terminal is answered again straight away, which is the point.
    Pair::type_at(&mut p.caller, "AT");
    p.run(0.05);
    assert!(
        p.caller_saw().ends_with("OK\r\n"),
        "would not talk afterwards: {:?}",
        p.caller_saw()
    );
}

#[test]
fn a_line_feed_after_the_dial_does_not_abort_it() {
    // The reason 5.6.1 puts an eighth of a second in front of the rule: a
    // terminal that ends its lines with a return and a line feed would
    // otherwise be hanging up on itself the instant it dialled.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.caller.feed_dte(b'\n');
    p.run(0.05);
    assert_eq!(
        p.caller.state(),
        State::Handshaking,
        "a trailing line feed dropped the call"
    );
}

#[test]
fn a_clean_300_bit_link_reports_no_bad_frames() {
    // Bell 103 recovers characters on the line, by their own start and stop
    // bits, and then hands them up as bits for the layer above to frame again.
    // That round trip is lossless by construction: what goes in is a character
    // and what comes out is the same character wrapped the same way. So on a
    // line with nothing wrong with it the count has to be zero, and if it is
    // not then the fault is in the handing over rather than in the line, which
    // is a distinction no amount of staring at corrupted text will make.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=B103");
    Pair::type_at(&mut p.caller, "AT+MS=B103");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(5.0);
    assert_eq!(p.caller.state(), State::Data, "never connected");

    // Something with every byte value in it, including the escape that a
    // board's colour sequences begin with.
    let payload: Vec<u8> = (0..=255u8).collect();
    for b in &payload {
        p.host.feed_dte(*b);
    }
    // 256 characters at thirty a second.
    p.run(10.0);

    assert_eq!(
        p.caller.framing_errors(),
        0,
        "{} characters lost between the line and the terminal on a clean link",
        p.caller.framing_errors()
    );
    let saw = &p.at_caller[p.at_caller.len().saturating_sub(payload.len())..];
    assert_eq!(saw, &payload[..], "the bytes came back changed");
}

#[test]
fn a_plain_dial_negotiates_before_it_starts() {
    // V.250 6.4.1 names the mechanism: <automode> "enables or disables
    // automatic modulation negotiation (e.g., Annex A/V.32 bis or ITU-T
    // Rec. V.8)", and it is on by default. So an ordinary ATD asks first.
    //
    // This is the thing no modem start-up can do for itself. Every one of them
    // assumes both ends already agree which Recommendation is being followed,
    // and nothing in any of them says so; two modems that guessed differently
    // transmit past each other until one gives up, which from either end looks
    // exactly like a modem that never answered.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(0.5);
    assert_eq!(p.caller.standard(), "V.8", "dialled without negotiating");
    assert_eq!(p.host.standard(), "V.8", "answered without negotiating");

    p.run(20.0);
    assert_eq!(p.caller.state(), State::Data, "the caller never connected");
    assert_eq!(p.host.state(), State::Data, "the host never connected");
    // The point of it: both ends at the same place, having agreed rather than
    // guessed.
    assert_eq!(p.caller.standard(), p.host.standard());
    assert_eq!(p.caller.rate(), p.host.rate());
}

#[test]
fn error_control_is_settled_in_v8_and_not_only_after_it() {
    // V.8 Table 6 carries a protocol octet, and 7.3 says it is there "in order
    // to negotiate LAPM without requiring the ODP/ADP exchange". Both ends can
    // know before a data carrier exists.
    //
    // The exchange still runs -- V.42 Appendix VI.2 says many answering modems
    // run it whatever V.8 said, and V.8 7.3 warns that some indicate LAPM and
    // then require it anyway. What the earlier answer buys is a reading of
    // silence: a detection phase that hears nothing has not contradicted a far
    // end that already said, in its own words, that it does LAPM.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(20.0);
    assert!(p.caller.error_control_negotiated(), "the caller never asked in V.8");
    assert!(p.host.error_control_negotiated(), "the host never answered in V.8");
    assert!(p.caller.error_controlled(), "and it never came up");
    assert!(p.host.error_controlled());
}

#[test]
fn a_modem_told_not_to_do_error_control_does_not_ask_for_it_in_v8() {
    // 7.4 completes the negotiation only when the JM answers a CM that asked.
    // A modem with error control turned off has nothing to ask about, and
    // saying LAPM in V.8 and then declining it is a way of being wrong twice.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+ES=0");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(20.0);
    assert_eq!(p.caller.state(), State::Data, "the call should still connect");
    assert!(!p.caller.error_control_negotiated());
    assert!(!p.host.error_control_negotiated(), "there was nothing to answer");
}

#[test]
fn turning_automode_off_says_the_modulation_and_means_it() {
    // 6.4.1 lists disabling automode among the constraints on switching, and
    // a terminal that has named a modulation and turned negotiation off has
    // said what it wants twice.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=V22B,0");
    Pair::type_at(&mut p.caller, "AT+MS=V22B,0");
    p.run(0.01);
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(0.5);
    assert_eq!(
        p.caller.standard(),
        "V.22bis",
        "negotiated after being told not to"
    );

    p.run(12.0);
    assert_eq!(p.caller.state(), State::Data);
    assert_eq!(p.caller.rate(), Some(2400));
}

#[test]
fn a_rate_ceiling_is_honoured_through_the_negotiation() {
    // The setting that matters on a line that cannot carry the faster rate,
    // and the one a negotiation could quietly undo: V.8 settles on "the
    // modulation mode with the lowest item number", which is the fastest, so
    // a ceiling has to be applied to what is offered rather than to what comes
    // back. Offer V.32 and V.32 is what will be agreed.
    let mut p = Pair::new();
    for m in [&mut p.host, &mut p.caller] {
        Pair::type_at(m, "AT+MS=V22B,1,1200,1200");
    }
    p.run(0.01);
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(20.0);

    assert_eq!(p.caller.state(), State::Data, "never connected");
    assert_eq!(p.caller.standard(), "V.22bis", "went faster than it was allowed");
    assert_eq!(p.caller.rate(), Some(1200));
    assert_eq!(p.host.rate(), Some(1200));
}

#[test]
fn a_far_end_that_cannot_go_as_fast_is_met_where_it_is() {
    // One end able to do V.32 and the other not. Without V.8 this is the case
    // that fails silently at both ends; with it, both come out at V.22bis.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+MS=V32,1,1200,9600");
    Pair::type_at(&mut p.host, "AT+MS=V22B,1,1200,2400");
    p.run(0.01);
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(20.0);

    assert_eq!(p.caller.state(), State::Data, "the caller never connected");
    assert_eq!(p.host.state(), State::Data, "the host never connected");
    assert_eq!(p.caller.standard(), "V.22bis");
    assert_eq!(p.host.standard(), "V.22bis");
}

// ---------------------------------------------------------------------------
// What the terminal is told, and when.

#[test]
fn the_reports_come_out_in_the_order_v250_gives_them() {
    // V.250 6.5.5: +ER is issued "before the final result code (e.g.,
    // CONNECT) is transmitted", and "after the modulation report ... and
    // before the data compression report (+DR)". So there is an order, and
    // the CONNECT is last -- which means it cannot be sent while the thing
    // being reported is still being negotiated.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+ER=1;+DR=1");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(20.0);

    let saw = p.caller_saw();
    let er = saw.find("+ER: LAPM").expect("no error control report");
    let dr = saw.find("+DR: V42B").expect("no compression report");
    let connect = saw.find("CONNECT").expect("never connected");
    assert!(er < dr, "+DR should follow +ER");
    assert!(dr < connect, "CONNECT is the final result code and comes last");
}

#[test]
fn nothing_is_reported_unless_the_terminal_asked() {
    // Both parameters default to 0 (V.250 6.5.5, 6.6.3), so an ordinary call
    // says what it always said.
    let p = connect();
    let saw = p.caller_saw();
    assert!(saw.contains("CONNECT"), "never connected");
    assert!(!saw.contains("+ER:"), "reported without being asked");
    assert!(!saw.contains("+DR:"));
}

#[test]
fn the_connect_is_not_sent_before_it_is_true() {
    // The reason the report has to come first is that it describes something
    // that is not settled when the carriers come up. A CONNECT sent then is a
    // promise about a negotiation that has not happened.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    // Step until the terminal is told, then look at what was true when it was.
    let mut told = false;
    for _ in 0..(30.0 * FS) as usize {
        let (a, b) = (p.from_caller, p.from_host);
        p.from_caller = p.caller.step(b);
        p.from_host = p.host.step(a);
        let out = p.caller.take_dte();
        if String::from_utf8_lossy(&out).contains("CONNECT") {
            told = true;
            break;
        }
        // And the state agrees with what the terminal has been told. A modem
        // in data state that has not said CONNECT is telling two stories.
        assert_ne!(p.caller.state(), State::Data, "in data before the CONNECT");
    }
    assert!(told, "the caller was never told it had connected");
    assert!(
        p.caller.error_controlled(),
        "CONNECT arrived while error control was still being negotiated"
    );
}

#[test]
fn a_call_without_error_control_still_says_connect_at_once() {
    // The wait is for an answer, not for a protocol. A modem with error
    // control turned off has its answer already.
    let mut p = Pair::new();
    Pair::type_at(&mut p.caller, "AT+ES=0;+ER=1");
    Pair::type_at(&mut p.host, "AT+ES=0");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(20.0);
    let saw = p.caller_saw();
    assert!(saw.contains("+ER: NONE"), "no report: {saw:?}");
    assert!(saw.contains("CONNECT"), "never connected");
    assert!(!p.caller.error_controlled());
}

#[test]
fn error_control_reports_where_it_has_got_to() {
    // Nothing about this reaches the terminal, and on a real line the
    // interesting part is which step did not happen. So the steps are
    // reportable while they are happening, in the order V.42 puts them:
    // 7.2.1's detection phase, then 8.10's XID exchange, then establishment.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");

    let mut seen: Vec<&'static str> = Vec::new();
    for _ in 0..(20.0 * FS) as usize {
        let (a, b) = (p.from_caller, p.from_host);
        p.from_caller = p.caller.step(b);
        p.from_host = p.host.step(a);
        p.caller.take_dte();
        p.host.take_dte();
        let phase = p.caller.error_control_phase();
        if seen.last() != Some(&phase) {
            seen.push(phase);
        }
    }
    assert_eq!(
        seen,
        ["", "detecting", "negotiating", "establishing", "connected"],
        "the phases a call goes through, in order"
    );
}

// ---------------------------------------------------------------------------
// A line that eats samples.
//
// The fault a real VoIP call actually has. It is not noise: noise flips a bit
// and the frame check sequence catches it. A dropped sample moves the clock,
// so the receiver's idea of where a bit ends slides by a fraction and then
// stays slid -- and what comes out is not a damaged frame but a stream that
// has lost its place. Everything above has to notice and recover, and the only
// thing that can notice is the frame check sequence.

impl Pair {
    /// Run the line, losing a sample every `every` sample periods.
    ///
    /// Lost, not corrupted. Each end is stepped a second time on the input it
    /// has already been given, and only the second output goes out -- so each
    /// direction is short one sample and each receiver has been handed one
    /// twice. That is what a jitter buffer running dry does to a modem: not a
    /// gap the receiver can see, but a moment that never came, after which
    /// everything is early. Noise flips a bit and the frame check sequence
    /// catches it; this moves the clock, and what comes out is a stream that
    /// has lost its place rather than a frame with a hole in it.
    fn run_lossy(&mut self, seconds: f64, every: usize) {
        for i in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(b);
            self.from_host = self.host.step(a);
            if every > 0 && i % every == 0 {
                self.from_caller = self.caller.step(b);
                self.from_host = self.host.step(a);
            }
            self.at_caller.extend(self.caller.take_dte());
            self.at_host.extend(self.host.take_dte());
        }
    }
}

#[test]
fn a_line_that_drops_samples_still_delivers_every_byte() {
    // The whole point of error control, stated as a test. A byte that arrives
    // wrong is worse than one that does not arrive, and V.42 exists so that
    // neither happens: what the far end reads is what was typed, or the call
    // ends.
    let mut p = connect();
    assert!(p.caller.error_controlled(), "no error control to test");

    let text = "The quick brown fox jumps over the lazy dog. 0123456789\r\n";
    for _ in 0..8 {
        for b in text.bytes() {
            p.caller.feed_dte(b);
        }
    }
    // A sample lost every 20 ms, which is one whole packet's worth of jitter
    // buffer arriving late, over and over.
    p.run_lossy(25.0, (FS * 0.020) as usize);

    let heard = p.host_saw();
    let wanted = text.repeat(8);
    assert!(
        heard.contains(&wanted) || heard.is_empty(),
        "what arrived was neither the text nor nothing:\n{heard:?}"
    );
    assert!(heard.contains(&wanted), "the text never arrived intact");
    // And the recovery actually ran. A test that loses nothing proves nothing,
    // and the count is the only evidence either way.
    assert!(
        p.host.damaged_frames() > 0,
        "no frame was damaged, so nothing here was tested"
    );
}

/// How much sample loss a call survives, and what it costs.
///
/// `cargo test -p modem --test call -- --ignored --nocapture report_loss`
///
/// Not a clean threshold, and it is not expected to be one. The loss here is
/// perfectly regular, so how much harm it does depends on how its period sits
/// against the symbol clock -- a rate that lands on the clock is tracked out
/// like any other frequency offset, and one that beats against it is not. The
/// figure to take from it is the order of magnitude, which is a lost sample
/// every millisecond or so at 16 kHz.
#[test]
#[ignore = "reports rather than asserts"]
fn report_loss_tolerance() {
    let text = "The quick brown fox jumps over the lazy dog. 0123456789\r\n";
    let wanted = text.repeat(8);

    let sample = connect();
    println!(
        "\n  over {} at {} bit/s, error control {}, compression {}",
        sample.caller.standard(),
        sample.caller.rate().unwrap_or(0),
        if sample.caller.error_controlled() { "V.42" } else { "off" },
        if sample.caller.compressing() { "V.42bis" } else { "off" }
    );
    println!("\n  one sample lost every   damaged  delivered");
    for ms in [50.0, 20.0, 10.0, 5.0, 2.0, 1.0, 0.5, 0.25, 0.125] {
        let mut p = connect();
        if !p.caller.error_controlled() {
            println!("  {ms:>7.3} ms           no error control");
            continue;
        }
        for b in wanted.bytes() {
            p.caller.feed_dte(b);
        }
        p.run_lossy(25.0, (FS * ms / 1000.0) as usize);
        println!(
            "  {ms:>7.3} ms         {:8}  {}",
            p.host.damaged_frames(),
            if p.host_saw().contains(&wanted) {
                "yes"
            } else if p.host.state() == State::Data {
                "not within 25 s, still retrying"
            } else {
                "no, the call dropped"
            }
        );
    }
    println!();
}

#[test]
fn error_control_comes_up_on_every_pump_that_can_carry_it() {
    // V.42 wants a synchronous bit pipe. Two of the three pumps are one; the
    // third is not, and the difference is not a detail that shows up anywhere
    // above. Worth checking on each rather than on whichever the default
    // happens to be, because the layer that would notice is the layer being
    // tested.
    for (carrier, seconds) in [("V22B", 12.0), ("V32", 14.0)] {
        let mut p = Pair::new();
        Pair::type_at(&mut p.host, &format!("AT+MS={carrier}"));
        Pair::type_at(&mut p.caller, &format!("AT+MS={carrier}"));
        Pair::type_at(&mut p.host, "ATA");
        Pair::type_at(&mut p.caller, "ATD5551234");
        p.run(seconds);
        assert_eq!(p.caller.state(), State::Data, "{carrier} never connected");
        assert!(p.caller.error_controlled(), "{carrier}: no error control");
        assert!(p.host.error_controlled(), "{carrier}: none at the far end");
        assert!(p.caller.compressing(), "{carrier}: no compression");
    }

    // Bell 103 is asynchronous all the way down: its line format *is*
    // start-stop framing and its receiver re-synchronises on every start bit,
    // so there is no synchronous pipe for V.42 to run on. Which is also how
    // anyone ever dialled a board at 300 bit/s.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "AT+MS=B103");
    Pair::type_at(&mut p.caller, "AT+MS=B103");
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(12.0);
    assert_eq!(p.caller.state(), State::Data, "Bell 103 never connected");
    assert!(!p.caller.error_controlled(), "Bell 103 cannot carry V.42");
    assert_eq!(p.caller.error_control_phase(), "none");

    // And it still carries what is typed, which is the point.
    for b in b"HELLO\r" {
        p.caller.feed_dte(*b);
    }
    p.run(4.0);
    assert!(p.host_saw().contains("HELLO"), "{:?}", p.host_saw());
}
