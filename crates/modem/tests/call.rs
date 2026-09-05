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
    assert!(
        p.caller_saw().contains("2400") || p.caller_saw().contains("1200"),
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
    p.run(3.0);

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
    // V.250 6.3.1. A terminal that types during a dial must not have it
    // parsed, and must not have it delivered as a burst the moment the
    // connection comes up either.
    let mut p = Pair::new();
    Pair::type_at(&mut p.host, "ATA");
    Pair::type_at(&mut p.caller, "ATD5551234");
    p.run(0.5);
    Pair::type_at(&mut p.caller, "ATH");
    p.run(9.5);
    assert_eq!(
        p.caller.state(),
        State::Data,
        "an ATH typed during the dial was obeyed"
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
