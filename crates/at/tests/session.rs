//! Whole-session behaviour of the AT interpreter, driven a byte at a time as a
//! real DTE would.

use at::result::ResultCode;
use at::{Action, Interpreter};

/// Feed a command line and return everything the DCE sent back.
fn send(it: &mut Interpreter, line: &str) -> (String, Vec<Action>) {
    for b in line.bytes() {
        it.feed(b);
    }
    (String::from_utf8(it.take_output()).unwrap(), it.take_actions())
}

/// An interpreter with echo off, so tests read the responses alone.
fn quiet_dce() -> Interpreter {
    let mut it = Interpreter::new();
    it.config.echo = false;
    it
}

#[test]
fn bare_at_acknowledges() {
    let mut it = quiet_dce();
    let (out, actions) = send(&mut it, "AT\r");
    assert_eq!(out, "\r\nOK\r\n");
    assert!(actions.is_empty());
}

#[test]
fn echo_is_on_by_default_and_e0_turns_it_off() {
    let mut it = Interpreter::new();
    let (out, _) = send(&mut it, "ATE0\r");
    // The command line itself is echoed, then the result code follows.
    assert_eq!(out, "ATE0\r\r\nOK\r\n");
    let (out, _) = send(&mut it, "AT\r");
    assert_eq!(out, "\r\nOK\r\n", "echo should now be off");
}

#[test]
fn unrecognised_commands_report_error() {
    let mut it = quiet_dce();
    for line in ["ATJ\r", "ATE7\r", "AT&Z\r", "AT%%%\r"] {
        let (out, _) = send(&mut it, line);
        assert_eq!(out, "\r\nERROR\r\n", "{line} should be rejected");
    }
}

#[test]
fn s_parameter_read_is_three_digits_with_leading_zeroes() {
    // V.250 5.3.2 requires exactly three characters.
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "ATS7?\r").0, "\r\n050\r\n\r\nOK\r\n");
    assert_eq!(send(&mut it, "ATS0?\r").0, "\r\n000\r\n\r\nOK\r\n");
}

#[test]
fn s_parameters_can_be_set_and_read_back() {
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "ATS7=90\r").0, "\r\nOK\r\n");
    assert_eq!(send(&mut it, "ATS7?\r").0, "\r\n090\r\n\r\nOK\r\n");
}

#[test]
fn out_of_range_s_parameter_values_are_rejected() {
    let mut it = quiet_dce();
    // S6 is defined as 2 to 10 (V.250 6.3.9).
    assert_eq!(send(&mut it, "ATS6=99\r").0, "\r\nERROR\r\n");
    assert_eq!(send(&mut it, "ATS6?\r").0, "\r\n002\r\n\r\nOK\r\n");
}

#[test]
fn a_missing_s_value_means_zero_and_the_range_decides() {
    let mut it = quiet_dce();
    // S0 permits 0, so "ATS0=" is accepted.
    assert_eq!(send(&mut it, "ATS0=\r").0, "\r\nOK\r\n");
    // S7's minimum is 1, so "ATS7=" is out of range.
    assert_eq!(send(&mut it, "ATS7=\r").0, "\r\nERROR\r\n");
    assert_eq!(
        send(&mut it, "ATS7?\r").0,
        "\r\n050\r\n\r\nOK\r\n",
        "the rejected write must leave S7 alone"
    );
}

#[test]
fn unimplemented_s_parameters_are_rejected() {
    // V.250 5.3.2.
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "ATS99?\r").0, "\r\nERROR\r\n");
}

#[test]
fn v0_switches_to_numeric_result_codes() {
    let mut it = quiet_dce();
    // The response to ATV0 itself already uses the new format.
    assert_eq!(send(&mut it, "ATV0\r").0, "0\r");
    assert_eq!(send(&mut it, "AT\r").0, "0\r");
    assert_eq!(send(&mut it, "ATJ\r").0, "4\r");
    assert_eq!(send(&mut it, "ATS7?\r").0, "050\r\n0\r");
}

#[test]
fn q1_suppresses_result_codes_but_not_information_text() {
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "ATQ1\r").0, "", "the OK for ATQ1 is itself suppressed");
    assert_eq!(send(&mut it, "AT\r").0, "");
    assert_eq!(
        send(&mut it, "ATS7?\r").0,
        "\r\n050\r\n",
        "V.250 6.2.5 suppresses result codes only"
    );
}

#[test]
fn s3_and_s4_retarget_the_framing_characters() {
    let mut it = quiet_dce();
    // V.250 6.2.1: the line containing S3= is terminated with the OLD value,
    // but the result code uses the NEW one.
    assert_eq!(send(&mut it, "ATS3=30\r").0, "\x1e\nOK\x1e\n");
    // Subsequent lines must now be terminated with the new character.
    assert_eq!(send(&mut it, "AT\x1e").0, "\x1e\nOK\x1e\n");
}

#[test]
fn backspace_edits_the_command_line() {
    // V.250 5.2.2: S5 deletes the preceding character.
    let mut it = quiet_dce();
    let (out, _) = send(&mut it, "ATJ\x08\r");
    assert_eq!(out, "\r\nOK\r\n", "the J should have been deleted");
}

#[test]
fn concatenated_commands_all_take_effect() {
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "ATE0V1X4S0=2\r").0, "\r\nOK\r\n");
    assert_eq!(it.config.x, 4);
    assert_eq!(send(&mut it, "ATS0?\r").0, "\r\n002\r\n\r\nOK\r\n");
}

#[test]
fn a_failing_command_stops_the_line_and_reports_error() {
    let mut it = quiet_dce();
    // X9 is invalid, so S0=5 after it must not take effect.
    assert_eq!(send(&mut it, "ATX9S0=5\r").0, "\r\nERROR\r\n");
    assert_eq!(send(&mut it, "ATS0?\r").0, "\r\n000\r\n\r\nOK\r\n");
}

#[test]
fn dial_yields_an_action_and_no_ok() {
    let mut it = quiet_dce();
    let (out, actions) = send(&mut it, "ATDT5551234\r");
    assert_eq!(actions, vec![Action::Dial("T5551234".into())]);
    assert_eq!(out, "", "the result code comes later, when the call resolves");
}

#[test]
fn answer_ignores_the_rest_of_the_line() {
    // V.250 5.3.1 and 6.3.5. Echo starts on, so if the trailing E0 were
    // executed it would turn echo off; that is what makes this observable.
    let mut it = Interpreter::new();
    assert!(it.config.echo);
    let (_, actions) = send(&mut it, "ATAE0\r");
    assert_eq!(actions, vec![Action::Answer]);
    assert!(it.config.echo, "E0 after A must not have been executed");
}

#[test]
fn hook_and_reset_commands_produce_their_actions() {
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "ATH0\r").1, vec![Action::HangUp]);
    assert_eq!(send(&mut it, "ATH1\r").1, vec![Action::OffHook]);
    assert_eq!(send(&mut it, "ATO\r").1, vec![Action::ReturnOnline]);
    assert_eq!(send(&mut it, "ATZ\r").1, vec![Action::ResetProfile(0)]);
    assert_eq!(send(&mut it, "AT&F\r").1, vec![Action::FactoryDefaults(0)]);
}

#[test]
fn hangup_and_reset_still_acknowledge_immediately() {
    // V.250 6.1.1: Z completes all of its work before issuing the result code.
    // Only D, A and O defer, because only they leave command state.
    for line in ["ATH0\r", "ATZ\r", "AT&F\r"] {
        // A fresh DCE each time: Z and &F restore E1, so reusing one would
        // start echoing partway through.
        let mut it = quiet_dce();
        assert_eq!(send(&mut it, line).0, "\r\nOK\r\n", "{line}");
    }
}

#[test]
fn reset_restores_echo_because_e1_is_the_factory_default() {
    // V.250 6.2.4 recommends E1, and 6.1.1 / 6.1.2 restore factory defaults, so
    // a reset turns echo back on. Software that sends ATZ then assumes echo is
    // still off will see its own commands come back.
    for reset in ["ATZ\r", "AT&F\r"] {
        let mut it = quiet_dce();
        assert!(!it.config.echo);
        send(&mut it, reset);
        assert!(it.config.echo, "{reset} should have restored E1");
        assert_eq!(send(&mut it, "AT\r").0, "AT\r\r\nOK\r\n");
    }
}

#[test]
fn ampersand_f_does_not_swallow_the_rest_of_the_line() {
    // "AT&F&C1&D2" is a common initialisation string; the settings after &F
    // have to take effect.
    let mut it = quiet_dce();
    it.config.dcd = 0;
    it.config.dtr = 0;
    let (out, actions) = send(&mut it, "AT&F&C1&D2\r");
    assert_eq!(out, "\r\nOK\r\n");
    assert_eq!(actions, vec![Action::FactoryDefaults(0)]);
    assert_eq!(it.config.dcd, 1);
    assert_eq!(it.config.dtr, 2);
}

#[test]
fn ampersand_f_restores_defaults_before_later_commands_apply() {
    let mut it = quiet_dce();
    send(&mut it, "ATS7=99\r");
    send(&mut it, "AT&FS7=80\r");
    it.config.echo = false; // &F restored E1; silence it again to read the reply
    assert_eq!(send(&mut it, "ATS7?\r").0, "\r\n080\r\n\r\nOK\r\n");
}

#[test]
fn z_ignores_commands_after_it() {
    // V.250 6.1.1: "commands ... after the Z command ... may be ignored".
    let mut it = quiet_dce();
    send(&mut it, "ATS0=5\r");
    let (_, actions) = send(&mut it, "ATZS0=9\r");
    assert_eq!(actions, vec![Action::ResetProfile(0)]);
    it.config.echo = false; // Z restored E1
    assert_eq!(
        send(&mut it, "ATS0?\r").0,
        "\r\n000\r\n\r\nOK\r\n",
        "Z should have reset S0, and the trailing S0=9 been ignored"
    );
}

#[test]
fn one_line_can_produce_two_actions() {
    // Neither &F nor H terminates the line, so both actions are reported.
    let mut it = quiet_dce();
    let (out, actions) = send(&mut it, "AT&FH0\r");
    assert_eq!(out, "\r\nOK\r\n");
    assert_eq!(actions, vec![Action::FactoryDefaults(0), Action::HangUp]);
}

#[test]
fn a_failed_command_discards_actions_queued_earlier_on_the_line() {
    let mut it = quiet_dce();
    let (out, actions) = send(&mut it, "AT&FX9\r");
    assert_eq!(out, "\r\nERROR\r\n");
    assert!(actions.is_empty(), "the line failed, so nothing should be acted on");
}

#[test]
fn a_slash_repeats_the_previous_line() {
    // V.250 5.2.4. No termination character is needed.
    let mut it = quiet_dce();
    send(&mut it, "ATS7=77\r");
    assert_eq!(send(&mut it, "ATS7?\r").0, "\r\n077\r\n\r\nOK\r\n");
    assert_eq!(
        send(&mut it, "A/").0,
        "\r\n077\r\n\r\nOK\r\n",
        "A/ should rerun the S7 query"
    );
}

#[test]
fn a_slash_before_any_command_line_acts_as_an_empty_line() {
    // V.250 5.2.4: "the preceding command line is assumed to have been empty
    // (that results in an OK result code)".
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "A/").0, "\r\nOK\r\n");
}

#[test]
fn junk_before_the_prefix_is_ignored() {
    // V.250 5.5.
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "\r\n\0xyzAT\r").0, "\r\nOK\r\n");
}

#[test]
fn identification_commands_answer() {
    let mut it = quiet_dce();
    assert!(send(&mut it, "ATI0\r").0.contains("SOFTMODEM"));
    assert!(send(&mut it, "AT+GMI\r").0.contains("dialupmodem2"));
    assert!(send(&mut it, "AT+GCAP\r").0.contains("+GCAP:"));
    assert_eq!(send(&mut it, "AT+NOSUCH\r").0, "\r\nERROR\r\n");
}

#[test]
fn windows_dial_up_networking_init_string_is_accepted() {
    // The sequence Windows DUN and most terminal software actually send.
    let mut it = Interpreter::new();
    for line in ["AT&F\r", "ATE0\r", "ATV1\r", "AT&C1\r", "AT&D2\r", "ATS0=0\r", "ATS7=60\r"] {
        let (out, _) = send(&mut it, line);
        assert!(out.ends_with("OK\r\n"), "{line} produced {out:?}");
    }
    assert_eq!(it.config.dcd, 1);
    assert_eq!(it.config.dtr, 2);
    assert!(!it.config.echo);
}

#[test]
fn an_over_long_line_errors_once_terminated() {
    // V.250 5.5.
    let mut it = quiet_dce();
    let line = format!("AT{}\r", "E0".repeat(300));
    assert_eq!(send(&mut it, &line).0, "\r\nERROR\r\n");
}

#[test]
fn deferred_result_codes_can_be_emitted_later() {
    // The modem reports CONNECT or NO CARRIER once the call resolves.
    let mut it = quiet_dce();
    let (_, actions) = send(&mut it, "ATDT5551234\r");
    assert_eq!(actions.len(), 1);
    it.emit(ResultCode::ConnectText("33600/V42BIS".into()));
    assert_eq!(
        String::from_utf8(it.take_output()).unwrap(),
        "\r\nCONNECT 33600/V42BIS\r\n"
    );
}

// -- the capabilities +GCAP advertises --------------------------------------

/// Terminate a command line, as a DTE does.
const CR: &str = "\r";

#[test]
fn every_command_gcap_names_is_answered() {
    // V.250 6.1.9. A DCE that lists a command it does not implement is worse
    // than one that lists nothing at all, because a DTE will believe it and
    // configure itself around a capability that is not there.
    let mut it = quiet_dce();
    let (out, _) = send(&mut it, &format!("AT+GCAP{CR}"));
    let listed: Vec<String> = out
        .lines()
        .find(|l| l.contains("+GCAP:"))
        .expect("no +GCAP response")
        .split(':')
        .nth(1)
        .unwrap()
        .split(',')
        .map(|s| s.trim().to_owned())
        .collect();
    assert!(!listed.is_empty());
    for name in listed {
        let (out, _) = send(&mut it, &format!("AT{name}=?{CR}"));
        assert!(
            !out.contains("ERROR"),
            "+GCAP names {name} but {name}=? answers {out:?}"
        );
    }
}

#[test]
fn modulation_selection_accepts_what_it_advertises() {
    let mut it = quiet_dce();
    let (out, _) = send(&mut it, &format!("AT+MS=?{CR}"));
    let offered = out
        .lines()
        .find(|l| l.contains("+MS:"))
        .expect("no +MS test response")
        .to_owned();
    for carrier in ["V22B", "V32"] {
        assert!(offered.contains(carrier), "+MS=? does not offer {carrier}");
        let (out, actions) = send(&mut it, &format!("AT+MS={carrier}{CR}"));
        assert!(out.contains("OK"), "+MS={carrier} answered {out:?}");
        assert!(
            matches!(&actions[..], [Action::SelectModulation(m)] if m.carrier == carrier),
            "+MS={carrier} produced {actions:?}"
        );
    }
}

#[test]
fn a_modulation_that_is_not_offered_is_refused() {
    // Accepting one and then using something else is how a terminal ends up
    // believing a connection is something it is not. V21 is the pointed case:
    // the 300 bit/s tones implemented here are Bell 103's, and a modem that
    // answered to V21 and then whistled at 1270 Hz would be lying to whoever
    // asked. V90 is simply not implemented at all.
    let mut it = quiet_dce();
    for refused in ["V90", "V21", "V34"] {
        let (out, actions) = send(&mut it, &format!("AT+MS={refused}{CR}"));
        assert!(out.contains("ERROR"), "+MS={refused} answered {out:?}");
        assert!(actions.is_empty());
    }
    // And the three that are.
    for offered in ["B103", "V22B", "V32"] {
        let (out, _) = send(&mut it, &format!("AT+MS={offered}{CR}"));
        assert!(out.contains("OK"), "+MS={offered} answered {out:?}");
    }
}

#[test]
fn reading_back_a_modulation_gives_what_was_set() {
    let mut it = quiet_dce();
    send(&mut it, &format!("AT+MS=V32,0,1200,4800{CR}"));
    let (out, _) = send(&mut it, &format!("AT+MS?{CR}"));
    assert!(out.contains("+MS: V32,0,1200,4800"), "read back {out:?}");
}

#[test]
fn error_control_selection_follows_table_20() {
    let mut it = quiet_dce();
    // 3 is V.42 with the detection phase, which is the default.
    let (_, actions) = send(&mut it, &format!("AT+ES=3,0{CR}"));
    let Some(Action::SelectErrorControl(e)) = actions.first() else {
        panic!("no action from +ES: {actions:?}");
    };
    assert!(e.wanted() && e.detect() && !e.required());

    // 2 asks for V.42 but without the detection phase.
    let (_, actions) = send(&mut it, &format!("AT+ES=2,0{CR}"));
    let Some(Action::SelectErrorControl(e)) = actions.first() else {
        panic!("no action");
    };
    assert!(e.wanted() && !e.detect());

    // 0 is direct mode: no error control at all.
    let (_, actions) = send(&mut it, &format!("AT+ES=0{CR}"));
    let Some(Action::SelectErrorControl(e)) = actions.first() else {
        panic!("no action");
    };
    assert!(!e.wanted());

    // A fallback of 2 or more requires it, and hangs up without it.
    let (_, actions) = send(&mut it, &format!("AT+ES=3,2{CR}"));
    let Some(Action::SelectErrorControl(e)) = actions.first() else {
        panic!("no action");
    };
    assert!(e.required());
}

#[test]
fn the_alternative_protocol_is_refused_rather_than_pretended_at() {
    // Table 20 value 4 is the alternative protocol, meaning MNP, which this
    // DCE does not implement. Accepting it would have a terminal expect a
    // protocol that never appears.
    let mut it = quiet_dce();
    let (out, actions) = send(&mut it, &format!("AT+ES=4{CR}"));
    assert!(out.contains("ERROR"), "+ES=4 answered {out:?}");
    assert!(actions.is_empty());
}

#[test]
fn compression_can_be_turned_off_and_on() {
    let mut it = quiet_dce();
    let (out, actions) = send(&mut it, &format!("AT+DS=0{CR}"));
    assert!(out.contains("OK"), "{out:?}");
    assert_eq!(actions.len(), 1);
    assert!(!it.compression.wanted());
    send(&mut it, &format!("AT+DS=3{CR}"));
    assert!(it.compression.wanted());

    let (out, _) = send(&mut it, &format!("AT+DS?{CR}"));
    assert!(out.contains("+DS: 3"), "read back {out:?}");
}

#[test]
fn fclass_reports_data_and_only_data() {
    // Facsimile is a different recommendation and is not implemented, so
    // claiming class 1 or 2 would be a lie a fax program would act on.
    let mut it = quiet_dce();
    let (out, _) = send(&mut it, &format!("AT+FCLASS=?{CR}"));
    assert!(out.contains("+FCLASS: (0)"), "{out:?}");
    let (out, _) = send(&mut it, &format!("AT+FCLASS=1{CR}"));
    assert!(out.contains("ERROR"), "claimed a fax class: {out:?}");
}

// ---------------------------------------------------------------------------
// +ER and +DR: whether the DCE says what it negotiated.

#[test]
fn error_control_reporting_is_off_until_it_is_asked_for() {
    // V.250 6.5.5 recommends a default of 0, and 6.6.3 the same for +DR. A
    // terminal that wants to be told asks to be told.
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "AT+ER?\r").0, "\r\n+ER: 0\r\n\r\nOK\r\n");
    assert_eq!(send(&mut it, "AT+DR?\r").0, "\r\n+DR: 0\r\n\r\nOK\r\n");

    send(&mut it, "AT+ER=1\r");
    send(&mut it, "AT+DR=1\r");
    assert!(it.config.report_error_control);
    assert!(it.config.report_compression);
    assert_eq!(send(&mut it, "AT+ER?\r").0, "\r\n+ER: 1\r\n\r\nOK\r\n");
}

#[test]
fn the_reporting_parameters_say_what_they_support() {
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "AT+ER=?\r").0, "\r\n+ER: (0,1)\r\n\r\nOK\r\n");
    assert_eq!(send(&mut it, "AT+DR=?\r").0, "\r\n+DR: (0,1)\r\n\r\nOK\r\n");
}

#[test]
fn a_reporting_value_that_does_not_exist_is_refused() {
    let mut it = quiet_dce();
    send(&mut it, "AT+ER=1\r");
    assert_eq!(send(&mut it, "AT+ER=2\r").0, "\r\nERROR\r\n");
    assert!(it.config.report_error_control, "a refused set changes nothing");
}

#[test]
fn factory_defaults_turn_the_reporting_back_off() {
    // V.250 Table 4 lists +ER and +DR among the parameters &F restores.
    let mut it = quiet_dce();
    // V.250 5.4.3: extended commands on one line are separated by ";".
    send(&mut it, "AT+ER=1;+DR=1\r");
    assert!(it.config.report_error_control);
    send(&mut it, "AT&F\r");
    assert!(!it.config.report_error_control);
    assert!(!it.config.report_compression);
}

#[test]
fn the_capability_list_names_what_is_answered() {
    // V.250 6.1.9. A DCE that lists a command it does not implement is worse
    // than one that lists nothing, because a DTE will believe it.
    let mut it = quiet_dce();
    let (out, _) = send(&mut it, "AT+GCAP\r");
    for name in ["+FCLASS", "+MS", "+ES", "+ER", "+DS", "+DR"] {
        assert!(out.contains(name), "{name} is missing from +GCAP");
        let (answer, _) = send(&mut it, &format!("AT{name}=?\r"));
        assert!(!answer.contains("ERROR"), "{name} is listed but not answered");
    }
}

#[test]
fn the_compression_parameters_are_all_four_of_them() {
    // V.250 6.6.1: +DS=[<direction>[,<compression_negotiation>[,<max_dict>
    // [,<max_string>]]]]. Three of the four used to be read and thrown away,
    // and the read-back reported values the modem had stopped using.
    let mut it = quiet_dce();
    assert_eq!(send(&mut it, "AT+DS?\r").0, "\r\n+DS: 3,0,2048,250\r\n\r\nOK\r\n");

    send(&mut it, "AT+DS=3,1,1024,32\r");
    assert_eq!(send(&mut it, "AT+DS?\r").0, "\r\n+DS: 3,1,1024,32\r\n\r\nOK\r\n");

    // V.250 5.4.2.1: an omitted subparameter keeps what it had.
    send(&mut it, "AT+DS=,,4096\r");
    assert_eq!(send(&mut it, "AT+DS?\r").0, "\r\n+DS: 3,1,4096,32\r\n\r\nOK\r\n");
}

#[test]
fn a_dictionary_outside_the_permitted_range_is_refused() {
    // V.42bis 6.4 on P1: "any attempt to specify less than the minimum value
    // shall be considered a procedural error"; and on P2: "the permitted range
    // is from 6 to 250. The values outside this range are invalid".
    let mut it = quiet_dce();
    for bad in ["AT+DS=3,0,256\r", "AT+DS=3,0,2048,5\r", "AT+DS=3,0,2048,251\r"] {
        assert_eq!(send(&mut it, bad).0, "\r\nERROR\r\n", "accepted {bad:?}");
    }
    // And a direction this DCE cannot honour, since V.42bis is negotiated as a
    // pair and offering half of it would be a promise it cannot keep.
    assert_eq!(send(&mut it, "AT+DS=1\r").0, "\r\nERROR\r\n");
    assert_eq!(send(&mut it, "AT+DS?\r").0, "\r\n+DS: 3,0,2048,250\r\n\r\nOK\r\n");
}

#[test]
fn the_compression_test_syntax_says_what_is_supported() {
    let mut it = quiet_dce();
    assert_eq!(
        send(&mut it, "AT+DS=?\r").0,
        "\r\n+DS: (0,3),(0,1),(512-65535),(6-250)\r\n\r\nOK\r\n"
    );
}
