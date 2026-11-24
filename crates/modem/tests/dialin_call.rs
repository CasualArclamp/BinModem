//! Dialling in: a login prompt, a password, `ppp`, and a ping over the call.
//!
//! The sibling of `ppp_call.rs`, with what came before PPP on a real account
//! put back in front of it. The answering modem runs the terminal server; the
//! calling one runs the script that logs in to it. Everything the two type at
//! each other crosses V.22bis and V.42 as text, and the moment the server
//! starts PPP the script sees the frame, both hand the stream to a PPP link,
//! and an echo goes across.
//!
//! And the other way in, which Windows' Dial-Up Networking used when it was
//! not told to show a terminal: no text at all, PPP from the first octet, and
//! the server asking for the same account with CHAP instead.

use login::server::{self, Server};
use login::{Account, Script, script};
use modem::{Modem, State};
use ppp::lcp::Auth;
use ppp::link::{Authentication, Link};
use ppp::ping::Pinger;

const FS: f64 = 16_000.0;
const SERVER: [u8; 4] = [10, 0, 0, 1];
const CLIENT: [u8; 4] = [10, 0, 0, 2];

fn account() -> Account {
    Account::new("guest", "letmein")
}

/// What is running above each modem.
enum Above<L> {
    Login(L),
    Ppp(Box<Link>),
}

/// What came of a call.
struct Outcome {
    /// What the caller's screen would have shown before PPP.
    screen: String,
    /// Who the server says logged in, at the prompt or over PPP.
    user: Option<String>,
    checked_with: Option<Auth>,
    client_address: [u8; 4],
    echoes: u32,
    up_at: f64,
}

/// Two modems on a pair; the answering one a dial-in server, the calling one
/// either logging in at the prompt (`script`) or going straight to PPP.
fn dial_in(script: bool, password: &str) -> Option<Outcome> {
    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        for b in b"AT+MS=V22B,0\r" {
            m.feed_dte(*b);
        }
        m.take_dte();
    }
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }
    for b in b"ATD5551234\r" {
        caller.feed_dte(*b);
    }

    let mut serving = Above::Login(Server::new(server::Config {
        accounts: vec![account()],
        ppp_message: "PPP session from 10.0.0.1 to 10.0.0.2 beginning....".to_owned(),
        ..server::Config::default()
    }));
    let offered = Account::new("guest", password);
    let client_link = || {
        Box::new(Link::with_authentication(
            [0; 4],
            [0; 4],
            Authentication { account: Some(offered.clone()), ..Authentication::default() },
        ))
    };
    let mut calling = if script {
        Above::Login(Script::new(offered.clone(), "ppp"))
    } else {
        let mut link = client_link();
        link.open();
        Above::Ppp(link)
    };
    let mut pinger = Pinger::new(0x0b18);
    pinger.every_ms = 500;
    pinger.timeout_ms = 20_000;

    let mut screen = Vec::new();
    let mut user = None;
    let mut up_at = None;
    let per_ms = FS as usize / 1000;
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let mut started = false;

    for i in 0..(120.0 * FS) as usize {
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);

        if caller.state() != State::Data || host.state() != State::Data {
            caller.take_dte();
            host.take_dte();
            continue;
        }
        if !started {
            started = true;
            // A dialler going straight to PPP starts the moment the call is
            // up, which is the moment its first frame could have gone.
            if let Above::Ppp(link) = &mut calling {
                let mut fresh = client_link();
                fresh.open();
                *link = fresh;
            }
        }

        let to_host = caller.take_dte();
        let to_caller = host.take_dte();
        let tick = i % per_ms == 0;

        // The answering end.
        let mut next_serving = None;
        match &mut serving {
            Above::Login(server) => {
                server.feed(&to_caller);
                if tick {
                    server.tick(1);
                }
                for b in server.take_output() {
                    host.feed_dte(b);
                }
                match server.take_outcome() {
                    Some(server::Outcome::Ppp { user: logged_in, early }) => {
                        user = logged_in.clone();
                        let mut link = Box::new(Link::with_authentication(
                            SERVER,
                            CLIENT,
                            Authentication {
                                // Logged in at the prompt is enough; a caller
                                // that was not is asked now.
                                callers: logged_in.is_none().then(|| vec![account()]),
                                name: "binmodem".into(),
                                seed: 7,
                                ..Authentication::default()
                            },
                        ));
                        link.open();
                        link.feed(&early);
                        next_serving = Some(Above::Ppp(link));
                    }
                    Some(server::Outcome::HangUp(why)) => panic!("the server hung up: {why}"),
                    None => {}
                }
            }
            Above::Ppp(link) => {
                link.feed(&to_caller);
                if tick {
                    link.tick(1);
                }
                for b in link.take_line() {
                    host.feed_dte(b);
                }
            }
        }
        if let Some(next) = next_serving {
            serving = next;
        }

        // The calling end.
        let mut next_calling = None;
        match &mut calling {
            Above::Login(script) => {
                screen.extend_from_slice(&to_host);
                script.feed(&to_host);
                if tick {
                    script.tick(1);
                }
                for b in script.take_output() {
                    caller.feed_dte(b);
                }
                match script.take_outcome() {
                    Some(script::Outcome::Ppp { early }) => {
                        let mut link = client_link();
                        link.open();
                        link.feed(&early);
                        next_calling = Some(Above::Ppp(link));
                        pinger.start();
                    }
                    Some(script::Outcome::Failed(why)) => panic!("the script gave up: {why}"),
                    None => {}
                }
            }
            Above::Ppp(link) => {
                link.feed(&to_host);
                if tick {
                    link.tick(1);
                    if !pinger.running() {
                        pinger.start();
                    }
                    let _ = pinger.poll(link, 1);
                }
                for b in link.take_line() {
                    caller.feed_dte(b);
                }
            }
        }
        if let Some(next) = next_calling {
            calling = next;
        }

        if let (Above::Ppp(server_link), Above::Ppp(client_link)) = (&serving, &calling) {
            if server_link.ended() || client_link.ended() {
                return None;
            }
            if up_at.is_none() && server_link.up() && client_link.up() {
                up_at = Some(i as f64 / FS);
            }
            if pinger.stats.received >= 2 {
                return Some(Outcome {
                    screen: String::from_utf8_lossy(&screen).into_owned(),
                    user: user.or_else(|| server_link.who().map(str::to_owned)),
                    checked_with: server_link.checked_with(),
                    client_address: client_link.addresses().0,
                    echoes: pinger.stats.received,
                    up_at: up_at.unwrap_or_default(),
                });
            }
        }
    }
    None
}

#[test]
fn a_caller_logs_in_at_the_prompt_types_ppp_and_pings() {
    let outcome = dial_in(true, "letmein").expect("never got a ping across");
    assert!(outcome.screen.contains("login: "), "{:?}", outcome.screen);
    assert!(outcome.screen.contains("Welcome to binmodem, guest."), "{:?}", outcome.screen);
    assert!(outcome.screen.contains("PPP session from 10.0.0.1 to 10.0.0.2"), "{:?}", outcome.screen);
    assert!(!outcome.screen.contains("letmein"), "the password came back on the screen");
    assert_eq!(outcome.user.as_deref(), Some("guest"));
    // Logged in at the prompt, so PPP did not ask again.
    assert_eq!(outcome.checked_with, None);
    assert_eq!(outcome.client_address, CLIENT);
    println!("  logged in, PPP up {:.1} s into the call, {} echoes back", outcome.up_at, outcome.echoes);
}

#[test]
fn a_caller_that_goes_straight_to_ppp_is_asked_with_chap() {
    let outcome = dial_in(false, "letmein").expect("never got a ping across");
    assert_eq!(outcome.user.as_deref(), Some("guest"));
    assert_eq!(outcome.checked_with, Some(Auth::ChapMd5));
    assert_eq!(outcome.client_address, CLIENT);
    println!("  straight to PPP, CHAP, up {:.1} s into the call", outcome.up_at);
}

#[test]
fn a_caller_with_the_wrong_password_gets_no_link() {
    assert!(dial_in(false, "guessing").is_none(), "a wrong password got IP across");
}
