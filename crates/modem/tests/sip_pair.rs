//! Two `modem-sip` processes on one machine, one calling the other.
//!
//! The tool exists to run calls against another softmodem, and the nearest
//! other softmodem is a second copy of itself: two accounts in one file that
//! name each other's port, neither registering, G.711 between them over the
//! loopback interface. Nothing is in the way of that call but the tool, so
//! what it connects at is a measure of the tool and not of a line.
//!
//! It did not measure well. `modem-sip` put the modem's samples on the line
//! as the modem handed them over, at a root mean square of 0.707 with the
//! shaped constellations peaking near 2, and G.711 has nothing above one: one
//! sample in six of V.34's training went out flat against the top of the
//! scale. Each end heard the other at about twenty decibels, the two settled
//! on 12000 bit/s when they settled at all, and in more than half the calls
//! an MP' or an E was lost to the clipping and both terminals were told
//! NO CARRIER about eleven seconds after dialling. The window never had the
//! fault, because the window has a drive setting and its default is a tenth.
//! The tool has the same default now, and `--drive 1` is how it used to be.
//!
//! A clean G.711 path carries V.34 at its top rates, so this asks for one of
//! them rather than for any CONNECT at all: 12000 was a connection too.
//!
//! Two modems in real time want an optimised build. Unoptimised, each falls
//! behind its own packet clock now and then -- about one run in six here had
//! silence go out in the middle of V.8 and the answering end never saw CJ --
//! so without `--release` this is ignored, and it is run as
//!
//! ```text
//! cargo test --release -p modem --test sip_pair
//! ```

use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

/// The lowest rate a call over nothing but G.711 should come up at. It comes
/// up at 33600 here; the margin is for a machine busy enough to slip a packet
/// during the training.
const LEAST: u32 = 26_400;

/// A UDP port nothing is using, as far as anybody can tell before using it.
fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .and_then(|s| s.local_addr())
        .expect("no UDP port to be had on 127.0.0.1")
        .port()
}

/// The rate in the first CONNECT a terminal was shown, if it was shown one.
fn connected_at(transcript: &str) -> Option<u32> {
    let rest = transcript.split("CONNECT").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "two modems in real time: run it with --release"
)]
fn two_of_them_on_one_machine_connect_at_a_rate_worth_having() {
    let host_port = free_port();
    let caller_port = free_port();
    let dir = std::env::temp_dir().join(format!("binmodem-sip-pair-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("could not make a directory for the accounts");
    let accounts = dir.join("sip.txt");
    std::fs::write(
        &accounts,
        format!(
            "[host]\nregistrar = 127.0.0.1:{caller_port}\nusername = host\npassword = unused\n\
             register = no\nlocal_port = {host_port}\n\
             [caller]\nregistrar = 127.0.0.1:{host_port}\nusername = caller\npassword = unused\n\
             register = no\nlocal_port = {caller_port}\n"
        ),
    )
    .expect("could not write the accounts");

    let tool = env!("CARGO_BIN_EXE_modem-sip");
    let run = |account: &str, rest: &[&str]| {
        Command::new(tool)
            .arg("--accounts")
            .arg(&accounts)
            .args(["--account", account])
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("could not start modem-sip")
    };
    let host = run(
        "host",
        &[
            "--answer",
            "--type",
            "AT+MS=V34",
            "--hold",
            "30",
            "--seconds",
            "50",
        ],
    );
    // The answering end has to be listening before the INVITE is sent.
    thread::sleep(Duration::from_secs(2));
    let caller = run(
        "caller",
        &[
            "--type",
            "AT+MS=V34",
            "--type",
            "ATD100",
            "--send",
            "hello",
            "--hold",
            "3",
            "--seconds",
            "45",
        ],
    );

    let caller = caller
        .wait_with_output()
        .expect("the calling end did not finish");
    let host = host
        .wait_with_output()
        .expect("the answering end did not finish");
    let _ = std::fs::remove_dir_all(&dir);

    let said = |o: &std::process::Output| {
        (
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    let (caller_terminal, caller_notes) = said(&caller);
    let (host_terminal, host_notes) = said(&host);
    let both = format!(
        "the calling end's terminal:\n{caller_terminal}\nits notes:\n{caller_notes}\n\
         the answering end's terminal:\n{host_terminal}\nits notes:\n{host_notes}"
    );

    let calling = connected_at(&caller_terminal)
        .unwrap_or_else(|| panic!("the calling end never connected\n{both}"));
    let answering = connected_at(&host_terminal)
        .unwrap_or_else(|| panic!("the answering end never connected\n{both}"));
    assert!(
        calling >= LEAST,
        "the calling end connected at {calling}\n{both}"
    );
    assert!(
        answering >= LEAST,
        "the answering end connected at {answering}\n{both}"
    );
    assert!(
        host_terminal.contains("hello"),
        "what was sent did not arrive\n{both}"
    );
}
