//! Two PPP ends where one wants to know who is calling.
//!
//! What a dial-up provider was: the caller dials, LCP comes up, and nothing
//! else happens until the caller has said who it is (RFC 1661 3.5). These run
//! a provider and a caller against each other with the octets handed across
//! whole, the way `two_ends.rs` does, and check both halves of it -- that the
//! right account gets in and that nothing else does.

use ppp::auth::Account;
use ppp::frame::{Deframer, Framer, Packet};
use ppp::lcp::Auth;
use ppp::link::{Authentication, Link, Phase};

const SERVER: [u8; 4] = [10, 0, 0, 1];
const CLIENT: [u8; 4] = [10, 0, 0, 2];

fn provider(accounts: Vec<Account>) -> Link {
    Link::with_authentication(
        SERVER,
        CLIENT,
        Authentication { callers: Some(accounts), name: "binmodem".into(), seed: 0x1234_5678, ..Authentication::default() },
    )
}

fn caller(account: Option<Account>) -> Link {
    Link::with_authentication([0; 4], [0; 4], Authentication { account, ..Authentication::default() })
}

/// Run both until `done`, or for `ms`. Gives back the milliseconds it took.
fn run(a: &mut Link, b: &mut Link, ms: u32, done: impl Fn(&Link, &Link) -> bool) -> Option<u32> {
    for elapsed in 0..ms {
        let from_a = a.take_line();
        if !from_a.is_empty() {
            b.feed(&from_a);
        }
        let from_b = b.take_line();
        if !from_b.is_empty() {
            a.feed(&from_b);
        }
        a.tick(1);
        b.tick(1);
        if done(a, b) {
            return Some(elapsed);
        }
    }
    None
}

fn both_up(a: &Link, b: &Link) -> bool {
    a.up() && b.up()
}

#[test]
fn the_right_account_gets_in_over_chap() {
    let mut server = provider(vec![Account::new("rory", "hunter2"), Account::new("guest", "letmein")]);
    let mut client = caller(Some(Account::new("guest", "letmein")));
    server.open();
    client.open();
    let took = run(&mut server, &mut client, 30_000, both_up).expect("never came up");
    assert_eq!(server.who(), Some("guest"));
    assert_eq!(server.checked_with(), Some(Auth::ChapMd5), "CHAP is to be offered before PAP");
    assert_eq!(client.proved_with(), Some(Auth::ChapMd5));
    assert_eq!(client.addresses(), (CLIENT, SERVER));
    println!("  up in {took} ms, over CHAP");

    // And IP crosses, which is what all of it was for.
    let _ = server.take_arrived();
    assert!(client.ping(1, 1, b"let in"));
    run(&mut server, &mut client, 50, |_, _| false);
    assert_eq!(client.take_arrived().iter().filter(|a| a.echo.reply).count(), 1);
}

/// An account with no password cannot be used with CHAP (RFC 1994 2.3), so the
/// provider asks for PAP instead.
#[test]
fn an_account_without_a_password_is_let_in_over_pap() {
    let mut server = provider(vec![Account::new("guest", "")]);
    let mut client = caller(Some(Account::new("guest", "")));
    server.open();
    client.open();
    run(&mut server, &mut client, 30_000, both_up).expect("never came up");
    assert_eq!(server.checked_with(), Some(Auth::Pap));
    assert_eq!(server.who(), Some("guest"));
}

#[test]
fn a_wrong_password_is_refused_and_the_link_ends_with_the_reason() {
    let mut server = provider(vec![Account::new("guest", "letmein")]);
    let mut client = caller(Some(Account::new("guest", "guessing")));
    server.open();
    client.open();
    let ended = run(&mut server, &mut client, 30_000, |a, b| a.ended() && b.ended());
    assert!(ended.is_some(), "still going: {:?} and {:?}", server.phase(), client.phase());
    assert!(!server.up() && !client.up());
    assert_eq!(server.who(), None);
    let server_says = server.trouble().unwrap_or_default();
    let client_says = client.trouble().unwrap_or_default();
    assert!(server_says.contains("not right"), "server: {server_says}");
    assert!(client_says.contains("did not accept"), "client: {client_says}");
    println!("  server: {server_says}\n  client: {client_says}");
}

/// Nothing gets past authentication by not taking part in it. A far end that
/// never answers a challenge is not let through to IPCP, and IP sent anyway
/// is not delivered.
#[test]
fn a_caller_that_stays_silent_is_not_let_through() {
    let mut server = provider(vec![Account::new("guest", "letmein")]);
    server.open();
    let far = Framer::new();
    let mut deframer = Deframer::new();
    let mut to_server = Vec::new();
    // Agree to LCP by hand, as a far end that knows only that much would:
    // acknowledge whatever the provider asks, ask for nothing, and then send
    // an IPCP request and an echo straight away.
    for _ in 0..40_000 {
        for byte in server.take_line() {
            if let Ok(Some(packet)) = deframer.feed(byte)
                && packet.protocol == ppp::protocol::LCP
                && packet.payload[0] == 1
            {
                let mut ack = packet.payload.clone();
                ack[0] = 2;
                far.frame(&Packet { protocol: ppp::protocol::LCP, payload: ack }, &mut to_server);
                far.frame(&Packet { protocol: ppp::protocol::LCP, payload: vec![1, 1, 0, 4] }, &mut to_server);
                far.frame(&Packet { protocol: ppp::protocol::IPCP, payload: vec![1, 1, 0, 10, 3, 6, 10, 0, 0, 2] }, &mut to_server);
            }
        }
        if !to_server.is_empty() {
            server.feed(&std::mem::take(&mut to_server));
        }
        server.tick(1);
        assert_ne!(server.phase(), Phase::Network, "a caller that never answered was let through");
    }
    assert!(server.ended(), "the provider is still waiting: {:?}", server.phase());
    assert!(server.trouble().unwrap_or_default().contains("challenges"), "{:?}", server.trouble());
}

#[test]
fn a_caller_with_no_account_is_told_no_rather_than_hanging() {
    let mut server = provider(vec![Account::new("guest", "letmein")]);
    let mut client = caller(None);
    server.open();
    client.open();
    run(&mut server, &mut client, 30_000, |a, b| a.ended() && b.ended()).expect("never ended");
    assert!(client.trouble().is_some());
}

/// RFC 1661 5.7: a protocol this end does not run gets a Protocol-Reject, not
/// silence. pppd offers CCP and IPv6CP on every link.
#[test]
fn a_protocol_this_end_does_not_run_is_rejected_by_number() {
    let mut a = Link::new(SERVER, CLIENT);
    let mut b = Link::new(CLIENT, SERVER);
    a.open();
    b.open();
    run(&mut a, &mut b, 30_000, both_up).expect("never came up");

    // Compression Control Protocol, 0x80fd, a Configure-Request with nothing
    // in it -- framed the way the far end would frame it now.
    let mut wire = Vec::new();
    let mut framer = Framer::new();
    framer.set_accm(0);
    framer.frame(&Packet { protocol: 0x80fd, payload: vec![1, 7, 0, 4] }, &mut wire);
    a.feed(&wire);

    let mut deframer = Deframer::new();
    deframer.set_accm(0);
    let rejects: Vec<_> = a
        .take_line()
        .into_iter()
        .filter_map(|b| deframer.feed(b).ok().flatten())
        .filter(|p| p.protocol == ppp::protocol::LCP && p.payload[0] == 8)
        .collect();
    assert_eq!(rejects.len(), 1, "no Protocol-Reject");
    // Code, Identifier, Length, then the protocol refused and what it said.
    assert_eq!(&rejects[0].payload[4..6], &[0x80, 0xfd]);
    assert_eq!(&rejects[0].payload[6..], &[1, 7, 0, 4]);
    assert!(a.up(), "the link went down over it");
}

/// A far end that hangs up the link is followed down, and once it is down a
/// new request from the far end brings it back: RFC 1661 4.4's zrc pause has
/// to end for that to be possible.
#[test]
fn a_link_the_far_end_ends_can_be_started_again() {
    let mut a = Link::new(SERVER, CLIENT);
    let mut b = Link::new(CLIENT, SERVER);
    a.open();
    b.open();
    run(&mut a, &mut b, 30_000, both_up).expect("never came up");

    a.close();
    let went = run(&mut a, &mut b, 30_000, |a, b| a.ended() && b.ended());
    assert!(went.is_some(), "a {:?} {} {:?}, b {:?} {} {:?}", a.phase(), a.ended(), a.trouble(), b.phase(), b.ended(), b.trouble());
    assert_eq!(b.trouble(), Some("the far end ended the link"));
    assert_eq!(a.trouble(), None, "closing it was this end's own doing");

    let mut again = Link::new(SERVER, CLIENT);
    again.open();
    run(&mut again, &mut b, 30_000, both_up).expect("the far end could not be brought back");
}
