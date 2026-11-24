//! A whole call, against a far end that is no more forgiving than a trunk.
//!
//! Everything underneath `sip::ua` is written against a document and can be
//! read against it, and has unit tests that do exactly that. The user agent
//! cannot be read that way: it owns a socket and a thread, so what it does is
//! a sequence in time rather than a function of its input, and the parts of it
//! most likely to be subtly wrong -- the order of an ACK against a retry, a
//! response absorbed from a cache, a call that must leave the line reusable --
//! are invisible to anything that does not actually run it.
//!
//! So this runs it. A fake trunk lives in this file, on 127.0.0.1 with the
//! system choosing its ports, and the agent registers with it, is challenged,
//! dials, is challenged again, comes up, carries audio through real sockets,
//! and is put down from both ends. The trunk is deliberately not helpful: it
//! challenges the first REGISTER and the first INVITE, it checks the digest
//! that comes back by computing it here from RFC 2617's formula rather than by
//! asking the code under test what it meant, and it expects the ACK that
//! 17.1.1.3 says a failure response gets. Anything this end would get wrong
//! against Asterisk or a wholesale trunk it gets wrong here, which is the only
//! reason for the file: the other way to find these faults is a live call.
//!
//! Every wait is a poll loop with a deadline that says what it was waiting for
//! and prints both transcripts when it gives up. A test of a running agent
//! that ends in a bare assertion tells a reader nothing about which of the
//! dozen things in flight did not happen.
//!
//! Nothing here is `#[ignore]`d any more.
//! `a_retransmitted_re_invite_gets_the_same_answer_twice` was, because it
//! described what RFC 3261 17.2.1 asks for and what `crates/sip/src/ua.rs`
//! did not do; the fault it names is fixed, so it runs with the rest. What
//! was wrong and where is still written against the test itself.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sip::line::Line;
use sip::message::{Headers, Message, Method, Request, Response};
use sip::uri::Uri;
use sip::{Account, Law, md5, rand};

/// What the trunk issued the account, and what it will check the digest
/// against. None of it leaves 127.0.0.1.
const USERNAME: &str = "0398765432";
const PASSWORD: &str = "a password with a space";
const REALM: &str = "trunk.invalid";
/// The number every test dials.
const DIALLED: &str = "0312345678";

/// How long any one thing is given before the test gives up on it.
///
/// Three seconds rather than one, and not because anything here is slow: the
/// tests run in parallel and each one is four threads of its own, so the
/// figure has to cover a scheduler that is busy rather than a protocol that
/// is. Nothing in a passing run waits anything like this long -- the first
/// retransmission timer in the agent is half a second, and a passing test
/// never reaches it.
const PATIENCE: Duration = Duration::from_secs(3);

/// How often a waiting test looks again. Short enough that a whole call is
/// still quick, long enough not to spin.
const GLANCE: Duration = Duration::from_millis(5);

/// The level the modem side sends while the audio path is being proved.
const TONE: f32 = 0.5;

/// How far off that level a sample may come back and still be the tone.
///
/// mu-law's step at half of full scale is a little over one per cent of it,
/// so a codeword that went out as 0.5 decodes a shade beside it. Nothing else
/// on the line is anywhere near: the only other thing the trunk echoes is the
/// silence the pacer sends when the modem has produced nothing.
const TOLERANCE: f32 = 0.05;

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// The whole sequence, end to end, through real sockets: registration with a
/// challenge, a call with another challenge, an answer, and audio that goes
/// out as G.711 and comes back.
///
/// The modem rate is the network rate on purpose. The rate conversion has its
/// own tests in `dsp`; leaving it as an identity here makes this test about
/// the packets and the negotiation that pointed them at the right port.
#[test]
fn a_whole_call_registers_dials_and_carries_audio() {
    let (far, line, mut said) = registered();

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    let progress = line.progress();
    assert!(progress.registered, "the registration went away: {progress:?}");
    assert_eq!(progress.call, "up", "{progress:?}");
    let peer = progress.peer.clone().unwrap_or_default();
    assert!(peer.contains(DIALLED), "the peer is {peer:?}, not who we dialled");
    assert_eq!(progress.law.as_deref(), Some("PCMU"), "{progress:?}");

    // The offer and the answer agreed on the trunk's own RTP port, which is
    // the thing that decides whether any of the rest of this can work.
    let agreed = line.negotiated().expect("nothing was recorded as negotiated");
    assert_eq!(agreed.port, far.rtp_port(), "the SDP answer's port was not taken up");
    assert_eq!(agreed.address, "127.0.0.1");

    // 13.2.2.4: the 2xx is acknowledged, or the trunk resends it for half a
    // minute and then tears the call down underneath us.
    let invite = answered_invite(&far);
    let (number, _) = invite.headers.cseq().expect("the INVITE had no CSeq");
    assert!(
        far.requests(Method::Ack)
            .iter()
            .any(|ack| ack.headers.cseq().map(|(n, _)| n) == Some(number)),
        "no ACK for the 200:\n{}",
        far.transcript()
    );

    // And the audio itself. One packet's worth of tone per glance, which is
    // the rate the pacer drains it at, so the queue neither starves nor grows.
    let chunk = vec![TONE; 40];
    let mut heard: Vec<f32> = Vec::new();
    wait(&line, &far, &mut said, "the tone to come back off the trunk", || {
        line.transmit(&chunk);
        line.receive(&mut heard);
        heard.iter().filter(|s| (**s - TONE).abs() < TOLERANCE).count() >= 320
    });

    let stats = line.progress();
    assert!(stats.packets_sent > 0 && stats.packets_received > 0, "{stats:?}");
    assert_eq!(stats.lost, 0, "packets went missing over the loopback: {stats:?}");
}

/// The digest the agent computes is the one RFC 2617's formula gives, on the
/// registrar's challenge and on the proxy's.
///
/// Computed here from the definition. A test that asked `sip::auth` what the
/// answer was would agree with the code under test however wrong both were,
/// and the failure this guards against -- a response hashed over the wrong
/// URI, the wrong realm or an unquoted nonce -- is exactly the one that looks
/// like a working stack until a provider refuses it.
#[test]
fn the_digest_answer_is_what_the_formula_gives() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    let checked = far.checked();
    for method in ["REGISTER", "INVITE"] {
        let one = checked
            .iter()
            .find(|c| c.method == method)
            .unwrap_or_else(|| panic!("no {method} ever arrived with credentials on it\n{}", far.transcript()));
        assert_eq!(
            one.offered, one.expected,
            "the {method} digest was wrong: it answered {} where the formula gives {} \
             (username {:?}, realm {:?}, nonce {:?}, uri {:?})",
            one.offered, one.expected, one.username, one.realm, one.nonce, one.uri
        );
        assert!(one.known_nonce, "the {method} answered a nonce this trunk never issued");
        assert_eq!(one.realm, REALM, "the {method} answered the wrong realm");
        assert_eq!(one.username, USERNAME, "the {method} authenticated as somebody else");
        // 22.4: the uri in the credentials is the request URI, and a
        // registrar that hashes the one it was sent gets a different answer
        // from a client that hashed a different one.
        assert_eq!(one.uri, one.request_uri, "the {method}'s stated uri is not its request URI");
    }

    // The first of each was sent bare, which is what makes the second one an
    // answer to a challenge rather than a guess.
    let registers = far.requests(Method::Register);
    assert!(registers.len() >= 2, "only {} REGISTER(s) arrived", registers.len());
    assert!(registers[0].headers.get("Authorization").is_none());
    assert!(registers[1].headers.get("Authorization").is_some());
}

/// 17.1.1.3: the 407 is acknowledged before the INVITE goes again.
///
/// Without the ACK the trunk has an INVITE transaction it believes is still
/// unanswered, and it retransmits its 407 on 17.1.1.2's schedule for about
/// half a minute -- into a call that by then has been answered on another
/// branch. It is a fault that never shows up as a failed call, only as a
/// trunk that behaves oddly.
#[test]
fn a_challenged_invite_is_acknowledged_before_it_is_sent_again() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    let seen = far.seen();
    let bare = position(&seen, |r| {
        r.method == Method::Invite && r.headers.get("Proxy-Authorization").is_none()
    })
    .unwrap_or_else(|| panic!("no INVITE arrived without credentials\n{}", far.transcript()));
    let branch = seen[bare]
        .request()
        .and_then(|r| r.headers.branch())
        .expect("the INVITE had no branch")
        .to_owned();

    let acked = position(&seen, |r| {
        r.method == Method::Ack && r.headers.branch() == Some(branch.as_str())
    })
    .unwrap_or_else(|| {
        panic!("the 407 was never acknowledged on branch {branch}\n{}", far.transcript())
    });
    let retried = position(&seen, |r| {
        r.method == Method::Invite && r.headers.get("Proxy-Authorization").is_some()
    })
    .unwrap_or_else(|| panic!("the INVITE was never sent again\n{}", far.transcript()));

    assert!(
        acked < retried,
        "the INVITE was sent again before the 407 was acknowledged\n{}",
        far.transcript()
    );
    // The retry is a new transaction, not a retransmission (8.1.3.5).
    let first = seen[bare].request().unwrap();
    let again = seen[retried].request().unwrap();
    assert_ne!(again.headers.branch(), first.headers.branch(), "the retry reused the branch");
    assert!(
        again.headers.cseq().map(|(n, _)| n) > first.headers.cseq().map(|(n, _)| n),
        "the retry reused the sequence number"
    );
}

/// Putting the telephone down sends a BYE, and the trunk's 200 ends the call
/// here: the line goes down and the progress goes back to idle.
#[test]
fn hanging_up_sends_a_bye_and_the_two_hundred_ends_the_call() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    line.hang_up();
    wait(&line, &far, &mut said, "a BYE to reach the trunk", || {
        !far.requests(Method::Bye).is_empty()
    });
    wait(&line, &far, &mut said, "the call to go back to idle", || {
        line.progress().call == "idle"
    });

    assert!(!line.up(), "the audio path is still up after the BYE");
    let progress = line.progress();
    assert!(progress.peer.is_none(), "still on a call with {:?}", progress.peer);
    // The line is a line, not a call: the registration outlives the call.
    assert!(progress.registered, "hanging up took the registration down too");
    assert!(
        said.iter().any(|s| s.contains("the call ended")),
        "nothing in the transcript says the call ended:\n{}",
        lines(&said)
    );
}

/// Ten hang-ups, one BYE.
///
/// The caller above this crate is level-triggered: while the modem is on hook
/// and the SIP call is still up it calls `Line::hang_up` on every turn of its
/// loop, which is every 2 ms, and `Agent::status().call` does not change until
/// the agent thread next runs -- up to 20 ms later. So about ten `HangUp`
/// commands land in the queue for one hang-up, `take_commands` drains the lot,
/// and every one of them used to reach `end_call`: ten BYEs, each with the next
/// sequence number and a new branch, each replacing the transaction before it.
/// A real trunk answered 481 to nine of them, and the one answer that meant
/// anything arrived for a transaction that had already been thrown away.
///
/// The caller is being made less eager as well, but the agent has to be
/// idempotent whatever the caller does: it is the only thing that knows
/// whether a hang-up is already on its way out, and asking twice is not an
/// unreasonable thing to do.
#[test]
fn hanging_up_ten_times_over_sends_one_bye() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    for _ in 0..10 {
        line.hang_up();
    }
    wait(&line, &far, &mut said, "the call to go back to idle", || {
        line.progress().call == "idle"
    });

    let byes = far.requests(Method::Bye);
    assert_eq!(
        byes.len(),
        1,
        "one hang-up put {} BYEs on the wire\n{}",
        byes.len(),
        far.transcript()
    );
    // And the hang-up still worked, which is the other half of it: a guard
    // that made the first ask do nothing either would pass the count above.
    assert!(!line.up(), "the audio path is still up after the BYE");
    assert!(line.progress().peer.is_none(), "still on a call");
    assert!(line.progress().registered, "hanging up took the registration down");
}

/// A dial the agent will not place is refused in words the line passes on.
///
/// Over SIP the modem is stepped by arriving RTP and by nothing else, so `ATD`
/// into a refusal that only writes a note leaves it off hook in the middle of
/// its handshake with the dial string already spent: no call, so no samples, so
/// none of its own timers advance and nothing ever times out. It waits for a
/// carrier on a call that was never placed until somebody forces the line down.
/// The reconciliation in the window cannot rescue it either -- that asks
/// whether a call which was active has ended, and this one never was.
///
/// `line.rs` words a code of 0 as "the call never left this machine", which is
/// the sentence being waited for here.
///
/// What is asserted afterwards is the agent's own state, and deliberately not
/// `Line::up`. `Line::poll` calls `Media::disconnect` on *every* `Failed`,
/// whichever call it was about, so a refusal raised while another call is live
/// costs that call its audio path -- the far end's address and the outgoing
/// queue are both dropped. That is `line.rs`'s to fix and not this crate's, and
/// it costs nothing on the path that actually produces this refusal: the modem
/// cannot dial while it is off hook, so a dial arriving while a call is up
/// means the call is already stale and on its way down.
#[test]
fn a_dial_while_a_call_is_up_is_refused_in_words_the_line_passes_on() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());
    let invites = far.requests(Method::Invite).len();

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the refusal to reach the line", || {
        line.progress()
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("never left this machine"))
    });

    let refusal = said
        .iter()
        .find(|s| s.contains("never left this machine"))
        .unwrap_or_else(|| panic!("the refusal never reached the transcript:\n{}", lines(&said)));
    assert!(
        refusal.contains(DIALLED),
        "the refusal does not say what was not dialled: {refusal}"
    );
    assert_eq!(
        far.requests(Method::Invite).len(),
        invites,
        "a second INVITE went out for a dial that was refused\n{}",
        far.transcript()
    );
    // And the dialog that was already up is still the agent's call: refusing
    // a second number must not end the first one.
    assert_eq!(
        line.progress().call, "up",
        "the refused dial ended the call that was already up\n{}",
        far.transcript()
    );
    assert!(
        far.requests(Method::Bye).is_empty(),
        "the refused dial put a BYE on the wire for the call that was up\n{}",
        far.transcript()
    );
}

/// And the other direction: the trunk hangs up, which is what happens when
/// the person at the far end puts their telephone down.
#[test]
fn the_far_end_hanging_up_is_answered_and_ends_the_call_here() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.order(Order::Bye);
    wait(&line, &far, &mut said, "the BYE to be answered 200", || {
        far.answers_to(Method::Bye).iter().any(|r| r.code == 200)
    });
    wait(&line, &far, &mut said, "the call to go back to idle", || {
        line.progress().call == "idle" && !line.up()
    });

    assert!(
        said.iter().any(|s| s.contains("the far end hung up")),
        "the transcript does not say who hung up:\n{}",
        lines(&said)
    );
    assert!(line.progress().registered, "the registration went with the call");
}

/// A call the trunk refuses is acknowledged, is explained in words a person
/// could act on, and leaves the line ready for the next one.
///
/// The second call is the point. A line that cannot place a second call is a
/// line nobody can use, and the state a failed call leaves behind -- a dialog
/// that was never cleared, a pending transaction, a media path still pointed
/// somewhere -- is exactly what a single-call test does not look at.
#[test]
fn a_refused_call_is_acknowledged_and_the_line_takes_another() {
    let (far, line, mut said) = registered();
    far.refuse_next_call(486, "Busy Here");

    line.dial(DIALLED);
    // Idle alone would be true before the INVITE had even gone out, so the
    // ACK for the refusal is waited for as well: the two together are the end
    // of the attempt and not the start of it.
    wait(&line, &far, &mut said, "the refused call to be given up on", || {
        line.progress().call == "idle" && !far.requests(Method::Ack).is_empty()
    });

    // 17.1.1.3 again: a failure response is acknowledged in its own
    // transaction, on the INVITE's branch.
    let refused = answered_invite(&far);
    let branch = refused.headers.branch().expect("no branch on the INVITE").to_owned();
    assert!(
        far.requests(Method::Ack)
            .iter()
            .any(|a| a.headers.branch() == Some(branch.as_str())),
        "the 486 was never acknowledged\n{}",
        far.transcript()
    );

    let failure = said
        .iter()
        .find(|s| s.contains("failed"))
        .unwrap_or_else(|| panic!("nothing said the call failed:\n{}", lines(&said)));
    assert!(failure.contains("486"), "the failure does not give the code: {failure}");
    assert!(failure.contains("busy"), "the failure does not say what 486 means: {failure}");
    assert!(!line.up(), "a refused call left the audio path connected");
    assert!(line.progress().peer.is_none(), "a refused call left a peer behind");

    // And now the same line places a call that works.
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the second call to come up", || line.up());
    assert_eq!(line.progress().call, "up");
    assert_eq!(
        far.requests(Method::Invite)
            .iter()
            .filter(|i| i.headers.get("Proxy-Authorization").is_some())
            .count(),
        2,
        "the second call did not reach the trunk as its own INVITE"
    );
}

/// An answer that chose a codec we never offered does not become a call.
///
/// G.729 models a voice tract and transmits the model; a modem signal does not
/// survive it, so a call carrying it is not a worse call but no call at all.
/// The agent has to put it down rather than sit on it, because everything
/// above would otherwise spend a minute failing to explain the silence.
#[test]
fn an_answer_of_a_codec_we_never_offered_is_hung_up_on() {
    let (far, line, mut said) = registered();
    far.answer_with(Answer::G729Only);

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the agent to put down a call it cannot carry", || {
        !far.requests(Method::Bye).is_empty()
    });
    wait(&line, &far, &mut said, "the call to go back to idle", || {
        line.progress().call == "idle"
    });

    assert!(!line.up(), "a call we cannot carry came up anyway");
    assert!(
        line.negotiated().is_none(),
        "something was recorded as negotiated: {:?}",
        line.negotiated()
    );
    assert!(
        said.iter().any(|s| s.contains("G729")),
        "the transcript does not name what the far end answered with:\n{}",
        lines(&said)
    );
    // The 200 is still acknowledged before the BYE: a dialog is established by
    // the 2xx whether or not we want it, and a BYE inside an unacknowledged
    // one is a BYE some trunks will not accept.
    let seen = far.seen();
    let acked = position(&seen, |r| r.method == Method::Ack);
    let byed = position(&seen, |r| r.method == Method::Bye);
    assert!(
        matches!((acked, byed), (Some(a), Some(b)) if a < b),
        "the 200 was not acknowledged before the BYE\n{}",
        far.transcript()
    );
}

/// An OPTIONS from the trunk is answered 200.
///
/// Several trunks send one every thirty seconds and take the registration down
/// when it goes unanswered, so this is not politeness: it is the difference
/// between a line that can be called and one that quietly cannot.
#[test]
fn an_options_from_the_trunk_is_answered() {
    let (far, line, mut said) = registered();

    far.order(Order::Options);
    // And again while nothing has answered.
    //
    // 17.1.2.2: a trunk's keep-alive is a non-INVITE client transaction and
    // it retransmits at T1 until it is answered. This trunk does not, which
    // made this the one test in the file whose whole path has no
    // retransmission in it anywhere -- so a single loopback datagram going
    // astray failed it, rarely and only under load. Pinging again is what a
    // real trunk does, and it is the difference between a test that measures
    // the agent and one that measures the network stack underneath it.
    let mut ping_again = Instant::now() + Duration::from_millis(500);
    wait(&line, &far, &mut said, "the OPTIONS to be answered", || {
        if !far.answers_to(Method::Options).is_empty() {
            return true;
        }
        if Instant::now() >= ping_again {
            far.order(Order::Options);
            ping_again = Instant::now() + Duration::from_millis(500);
        }
        false
    });

    let answer = far.answers_to(Method::Options).remove(0);
    assert_eq!(answer.code, 200, "the trunk's keep-alive got {} {}", answer.code, answer.reason);
    // 11.2: an answer worth having says what the agent will accept.
    assert!(
        answer.headers.all("Allow").any(|a| a.contains("INVITE")),
        "the 200 to OPTIONS does not say what this agent allows"
    );
    assert!(line.progress().registered, "the registration did not survive an OPTIONS");
}

/// A re-INVITE asking to switch the call to T.38 is declined with 488, and the
/// call stays up on G.711.
///
/// This is what keeps a fax working until T.38 itself is built: the far end
/// hears no for an answer and carries on sending the fax as audio, which the
/// V.17, V.29 and V.27 ter receivers in this workspace can already read. The
/// failure to guard against is not the 488 -- it is the call being torn down,
/// or the media path being repointed, by a renegotiation that was refused.
#[test]
fn a_re_invite_asking_for_t38_is_declined_and_the_call_stays_up() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    let before = line.progress().packets_received;
    far.order(Order::T38ReInvite);
    wait(&line, &far, &mut said, "the re-INVITE to be declined", || {
        far.answers_to(Method::Invite).iter().any(|r| r.code >= 300)
    });

    let declined = far
        .answers_to(Method::Invite)
        .into_iter()
        .find(|r| r.code >= 300)
        .expect("checked in the wait above");
    assert_eq!(declined.code, 488, "T.38 was answered {} {}", declined.code, declined.reason);
    assert!(
        declined.reason.to_ascii_lowercase().contains("t.38"),
        "the refusal does not say what was refused: {}",
        declined.reason
    );

    assert!(line.up(), "declining T.38 took the call down");
    assert_eq!(line.progress().call, "up");
    assert_eq!(
        line.negotiated().map(|n| n.law),
        Some(Law::Mu),
        "the call did not stay on the law it came up with"
    );
    // Still carrying audio, which is the whole point of declining rather than
    // hanging up.
    wait(&line, &far, &mut said, "audio to keep flowing after the 488", || {
        line.progress().packets_received > before
    });
}

/// 17.2.1: a request that arrives twice is answered twice with the same
/// answer, not processed twice.
///
/// The path to this trunk carries about 750 ms one way, so a retransmission is
/// not an edge case here; it happens on most calls. An agent that builds a
/// fresh response instead of replaying the old one sends a second answer with
/// a different tag or a different body, and the far end has to decide which of
/// two disagreeing answers to believe.
#[test]
fn a_retransmitted_bye_gets_the_same_answer_twice() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.order(Order::Bye);
    wait(&line, &far, &mut said, "the BYE to be answered", || {
        !far.answers_to(Method::Bye).is_empty()
    });

    far.order(Order::ByeAgain);
    wait(&line, &far, &mut said, "the retransmitted BYE to be answered again", || {
        far.answers_to(Method::Bye).len() >= 2
    });

    let answers = far.answer_octets(Method::Bye);
    assert_eq!(answers.len(), 2, "expected exactly two answers, got {}", answers.len());
    assert_eq!(
        String::from_utf8_lossy(&answers[0]),
        String::from_utf8_lossy(&answers[1]),
        "the second answer to the same BYE is a different message"
    );
}

/// The far end hangs up, and the person presses Call again straight away.
///
/// The second call is the whole test. A line that ends a call tidily and then
/// will not place another is a line that works once, and the state a finished
/// call leaves behind -- a dialog nobody cleared, a transaction still running
/// -- is exactly what a test of one call cannot see.
#[test]
fn the_far_end_hanging_up_leaves_the_line_ready_for_the_next_number() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.order(Order::Bye);
    wait(&line, &far, &mut said, "the call to go back to idle", || {
        line.progress().call == "idle" && !line.up()
    });

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the second call to come up", || line.up());
    assert_eq!(line.progress().call, "up", "{:?}", line.progress());
    assert_eq!(
        answered_invites(&far),
        2,
        "the second call never reached the trunk as its own INVITE\n{}",
        far.transcript()
    );
}

/// Hang up, and dial again in the same breath.
///
/// This is the fault the person reported, in the order they did it in: the
/// call has dropped, they press Hang up, and then they press Call. Both land
/// in the agent's queue and `take_commands` drains the pair in one pass, so
/// the number arrives while the BYE is still unanswered -- and on this path
/// the answer is about 1.5 s away, with 32 s before the transaction gives up.
/// A dial refused through all of that is what "you can not redial" means.
///
/// The BYE count is the other half: ending the call here at once must not
/// become a second way to send a second BYE.
#[test]
fn a_number_dialled_the_moment_the_call_was_put_down_still_becomes_a_call() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    line.hang_up();
    line.dial(DIALLED);

    wait(&line, &far, &mut said, "the second call to come up", || line.up());
    assert_eq!(line.progress().call, "up", "{:?}", line.progress());
    // Waited for rather than counted where it stands. The BYE and the INVITE
    // leave in the same pass and the trunk is a thread of its own, so which of
    // them it has finished writing down at any instant is not something this
    // test is about -- that there is one BYE, and one only, is.
    wait(&line, &far, &mut said, "the BYE for the call that was put down", || {
        !far.requests(Method::Bye).is_empty()
    });
    assert_eq!(
        far.requests(Method::Bye).len(),
        1,
        "one hang-up put {} BYEs on the wire\n{}",
        far.requests(Method::Bye).len(),
        far.transcript()
    );
    assert_eq!(
        answered_invites(&far),
        2,
        "the redial never reached the trunk as its own INVITE\n{}",
        far.transcript()
    );
}

/// The far end answers nothing at all after the BYE.
///
/// Which is what a call that has genuinely dropped looks like: the thing that
/// was at the other end has gone, so the BYE goes unanswered and 17.1.2.2's
/// Timer F sits on it for 64*T1 -- 32 seconds. Nobody waits 32 seconds to make
/// a telephone call, so the line has to be free of it long before then: the
/// window reads `idle`, and the next number goes out.
#[test]
fn a_bye_the_far_end_never_answers_still_frees_the_line_at_once() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.treat_byes(ByeManner::Ignore);
    line.hang_up();
    wait(&line, &far, &mut said, "the BYE to reach the trunk", || {
        !far.requests(Method::Bye).is_empty()
    });
    wait(&line, &far, &mut said, "the line to come back to idle", || {
        line.progress().call == "idle"
    });

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the next call to come up", || line.up());
    assert_eq!(line.progress().call, "up", "{:?}", line.progress());
}

/// And that unanswered BYE, answered at last, must not end the call placed
/// since.
///
/// 17.1.2.2 keeps a BYE's transaction running for 32 s, and a trunk that was
/// slow rather than gone answers it somewhere in there -- by which time the
/// person has redialled and is on another call. The answer ends its own
/// transaction and nothing else: the dialog it named is one this end finished
/// with when the BYE went out.
#[test]
fn a_late_answer_to_the_last_calls_bye_does_not_end_the_next_call() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.treat_byes(ByeManner::Ignore);
    line.hang_up();
    wait(&line, &far, &mut said, "the BYE to reach the trunk", || {
        !far.requests(Method::Bye).is_empty()
    });

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the next call to come up", || line.up());

    // The trunk gets round to the BYE it never answered.
    far.order(Order::AnswerTheOldBye);
    wait(&line, &far, &mut said, "the trunk's late 200 to arrive", || {
        far.answered_the_old_bye()
    });
    // A moment for the agent to act on it, if it is going to.
    let settle = Instant::now() + Duration::from_millis(300);
    while Instant::now() < settle {
        said.extend(line.poll());
        thread::sleep(GLANCE);
    }

    assert_eq!(
        line.progress().call,
        "up",
        "a late answer to the last call's BYE ended the call placed since\n{}",
        far.transcript()
    );
    assert!(line.up(), "the audio path went with it\n{}", far.transcript());
}

/// Hang up, redial, and put that second call down while the first call's BYE
/// is still running.
///
/// 17.1.2.2 keeps a BYE's transaction going for 32 s when nothing answers it,
/// and a far end that takes INVITEs and ignores BYEs keeps it for all of them.
/// The call placed in the meantime is a different call: putting it down has to
/// put it down. A hang-up read as "there is already a BYE out" is a person
/// pressing Hang up and watching nothing happen, which is where this started
/// -- and it is the fault that letting a new call be placed during a teardown
/// would otherwise have introduced.
#[test]
fn a_second_call_can_be_put_down_while_the_first_ones_bye_is_still_running() {
    let (far, line, mut said) = registered();
    far.treat_byes(ByeManner::Ignore);

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the first call to come up", || line.up());
    let first = line
        .progress()
        .call;
    assert_eq!(first, "up");

    line.hang_up();
    wait(&line, &far, &mut said, "the first BYE to reach the trunk", || {
        !far.requests(Method::Bye).is_empty()
    });

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the second call to come up", || line.up());

    line.hang_up();
    wait(
        &line,
        &far,
        &mut said,
        "a BYE for the second call, which is a different call",
        || byes_for_distinct_calls(&far) >= 2,
    );
    wait(&line, &far, &mut said, "the line to come back to idle", || {
        line.progress().call == "idle" && !line.up()
    });

    // And a third, because a line that works twice and not three times is
    // still a line with something left over in it.
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the third call to come up", || line.up());
}

/// The far end refuses our BYE with 481: it had destroyed the dialog already.
///
/// 481 means the call is over at that end, which is the same news as a 200 and
/// must leave this end in the same place -- idle, and able to dial.
#[test]
fn a_bye_refused_with_481_still_ends_the_call_here() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.treat_byes(ByeManner::Refuse);
    line.hang_up();
    // The 481 is the trunk's, so it is not in what the trunk *saw*: what says
    // it arrived is the BYE reaching the trunk and the line then settling.
    wait(&line, &far, &mut said, "the BYE to reach the trunk", || {
        !far.requests(Method::Bye).is_empty()
    });
    wait(&line, &far, &mut said, "the line to come back to idle", || {
        line.progress().call == "idle"
    });

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the next call to come up", || line.up());
    assert_eq!(
        far.requests(Method::Bye).len(),
        1,
        "a BYE refused 481 was sent again\n{}",
        far.transcript()
    );
    assert!(
        said.iter().any(|s| s.contains("the call ended")),
        "nothing said the call ended:\n{}",
        lines(&said)
    );
}

/// Hanging up a call that has already gone.
///
/// The person presses Hang up after the far end has already hung up -- which
/// is most of the time, because the far end's BYE and their finger arrive in
/// either order. There is nothing to end, so nothing may go out, and nothing
/// may be left behind that stops the next number.
#[test]
fn hanging_up_a_call_that_has_already_gone_leaves_nothing_behind() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    far.order(Order::Bye);
    wait(&line, &far, &mut said, "the call to go back to idle", || {
        line.progress().call == "idle"
    });

    // Ten, because the window's own loop asks again while it can see a call.
    for _ in 0..10 {
        line.hang_up();
    }
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the next call to come up", || line.up());

    assert!(
        far.requests(Method::Bye).is_empty(),
        "a BYE went out for a call the far end had already ended\n{}",
        far.transcript()
    );
    assert_eq!(line.progress().call, "up", "{:?}", line.progress());
}

/// The same, one stage earlier: a number dialled while a CANCEL is in flight.
///
/// 9.1 will not let a cancelled INVITE simply be dropped -- the far end may
/// have committed to answering it, and an unacknowledged 487 is a trunk
/// retransmitting for half a minute -- so this is the one leg that cannot be
/// finished here the moment the person asks. What must not happen is the dial
/// being thrown away: the number waits for the 487 and then goes out.
#[test]
fn a_number_dialled_while_a_cancel_is_in_flight_still_becomes_a_call() {
    let (far, line, mut said) = registered();
    far.hold_next_call();

    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the far end to ring", || {
        line.progress().call == "ringing"
    });

    line.hang_up();
    line.dial(DIALLED);

    wait(&line, &far, &mut said, "the second call to come up", || line.up());
    assert_eq!(line.progress().call, "up", "{:?}", line.progress());
    assert_eq!(
        far.requests(Method::Cancel).len(),
        1,
        "one hang-up put {} CANCELs on the wire\n{}",
        far.requests(Method::Cancel).len(),
        far.transcript()
    );
    assert_eq!(
        answered_invites(&far),
        2,
        "the number dialled while the CANCEL was in flight never became an \
         INVITE\n{}",
        far.transcript()
    );
    // 17.1.1.3: the 487 is acknowledged, whatever else was going on by then.
    let cancelled = far
        .requests(Method::Invite)
        .into_iter()
        .find(|i| i.headers.get("Proxy-Authorization").is_some())
        .expect("no INVITE with credentials on it");
    let branch = cancelled.headers.branch().expect("no branch").to_owned();
    assert!(
        far.requests(Method::Ack)
            .iter()
            .any(|a| a.headers.branch() == Some(branch.as_str())),
        "the 487 for the cancelled call was never acknowledged\n{}",
        far.transcript()
    );
}

/// The same rule, on the one request where obeying it visibly matters.
///
/// A BYE's answer would come out the same however it was built, so the pair
/// above proves that a second answer arrives rather than that it came from the
/// cache. A re-INVITE is different: its answer carries a session description,
/// and RFC 4566 5.2's origin id is fresh every time one is built. So two
/// identical answers can only be one answer sent twice.
///
/// This used to be ignored, because it did not pass: `Worker::re_invite` sent
/// its 200 with `send` where every other final response went out through
/// `respond`, and `respond` was the only thing that put a response into the
/// `answered` list that `handle_request` consults for 17.2.1. So a
/// retransmitted re-INVITE was processed a second time: the far end got a
/// second, different description, and worse, the agent raised another
/// `Answered` event, which sends `line::Line::poll` back into
/// `Media::connect` -- restarting the jitter buffer and clearing the outgoing
/// queue in the middle of a call that nothing was wrong with. On this rig that
/// is precisely the twenty-millisecond discontinuity that costs a V.34
/// receiver its training. `Worker::accept_call` sent its 200 the same way and
/// had the same hole. Both now go out through `Worker::send_final`, which is
/// `respond`'s own last step.
#[test]
fn a_retransmitted_re_invite_gets_the_same_answer_twice() {
    let (far, line, mut said) = registered();
    line.dial(DIALLED);
    wait(&line, &far, &mut said, "the call to come up", || line.up());

    let answered = |far: &FarEnd| -> Vec<Vec<u8>> {
        far.seen()
            .iter()
            .filter(|s| {
                s.response().is_some_and(|r| {
                    r.code == 200 && r.headers.cseq().is_some_and(|(_, m)| m == Method::Invite)
                })
            })
            .map(|s| s.octets.clone())
            .collect()
    };

    far.order(Order::ReInvite);
    wait(&line, &far, &mut said, "the re-INVITE to be answered", || {
        !answered(&far).is_empty()
    });
    far.order(Order::ReInviteAgain);
    wait(&line, &far, &mut said, "the retransmitted re-INVITE to be answered", || {
        answered(&far).len() >= 2
    });

    let answers = answered(&far);
    assert_eq!(
        String::from_utf8_lossy(&answers[0]),
        String::from_utf8_lossy(&answers[1]),
        "the same re-INVITE was answered twice over, with a new description each time"
    );
}

// ---------------------------------------------------------------------------
// Getting a line up, and waiting for things
// ---------------------------------------------------------------------------

/// A trunk, and a line registered to it. Where every test starts.
///
/// The trunk is returned first so that it is dropped last: the line's own
/// shutdown sends a BYE and an unregister, and a trunk that has already gone
/// leaves the agent retransmitting into nothing for the length of its timer.
fn registered() -> (FarEnd, Line, Vec<String>) {
    let far = FarEnd::start();
    let line = Line::open(account(far.address()), sip::media::NETWORK_RATE)
        .expect("the line would not open");
    let mut said = Vec::new();
    wait(&line, &far, &mut said, "the line to register", || {
        line.progress().registered
    });
    (far, line, said)
}

/// The account, built here rather than written to a file: what is being tested
/// is the agent, and a file would only add a way for the test to fail that has
/// nothing to do with it.
fn account(trunk: SocketAddr) -> Account {
    Account {
        name: "test trunk".to_owned(),
        registrar: trunk.to_string(),
        // Written out rather than left to default from the registrar, because
        // the registrar here carries a port and the domain must not.
        domain: "127.0.0.1".to_owned(),
        username: USERNAME.to_owned(),
        password: PASSWORD.to_owned(),
        display: Some("BinModem".to_owned()),
        register: true,
        expires: 120,
        laws: vec![Law::Mu, Law::A],
        // Both zero: a test that wants a particular port is a test that fails
        // when something else on the machine already has it.
        local_port: 0,
        rtp_port: 0,
        ptime_ms: 20,
        ..Account::default()
    }
}

/// Poll the line until something is true, or say what never happened.
///
/// `Line::poll` is called here and nowhere else, because it is what turns an
/// answered call into a connected media path -- a test that waited without
/// polling would wait for ever on a call the agent had already answered. What
/// it returns is the running commentary on what the agent thought was going
/// on, so it is collected and printed on the way out.
fn wait(
    line: &Line,
    far: &FarEnd,
    said: &mut Vec<String>,
    what: &str,
    mut done: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        said.extend(line.poll());
        if done() {
            // One more look before going, because the two things a test reads
            // are filled in by the agent thread in that order and read here in
            // the other: `Line::poll` drains the events, and then `done` reads
            // the status the agent had already moved on. A call that fails sets
            // the status to idle and *then* queues its `Failed`, so a test that
            // waited for idle and went straight to what the line said could
            // miss the sentence that came with it. Rare, load-dependent, and
            // exactly the shape of a test that fails once a fortnight -- so the
            // wait ends one glance after the condition rather than on it.
            thread::sleep(GLANCE);
            said.extend(line.poll());
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "waited {PATIENCE:?} for {what}, and it never happened.\n\
                 The line is {:?}\n\
                 The line said:\n{}\n\
                 The trunk saw:\n{}",
                line.progress(),
                lines(said),
                far.transcript()
            );
        }
        thread::sleep(GLANCE);
    }
}

/// The INVITE the trunk actually acted on: the one that carried credentials.
fn answered_invite(far: &FarEnd) -> Request {
    far.requests(Method::Invite)
        .into_iter()
        .find(|r| r.headers.get("Proxy-Authorization").is_some())
        .unwrap_or_else(|| {
            panic!("no INVITE ever arrived with credentials on it\n{}", far.transcript())
        })
}

/// How many calls actually reached the trunk: an INVITE without credentials is
/// half of one attempt, and this counts attempts.
fn answered_invites(far: &FarEnd) -> usize {
    far.requests(Method::Invite)
        .iter()
        .filter(|i| i.headers.get("Proxy-Authorization").is_some())
        .count()
}

/// How many calls the BYEs that arrived were for.
///
/// Counted by Call-ID and not by how many BYEs came: 17.1.2.2 sends one again
/// until it is answered, so a trunk that answers none of them collects a pile
/// of copies of the same hang-up.
fn byes_for_distinct_calls(far: &FarEnd) -> usize {
    let mut calls: Vec<String> = far
        .requests(Method::Bye)
        .iter()
        .map(|b| b.headers.call_id().unwrap_or_default().to_owned())
        .collect();
    calls.sort();
    calls.dedup();
    calls.len()
}

/// Where in what the trunk saw the first request matching this sits.
fn position(seen: &[Seen], matches: impl Fn(&Request) -> bool) -> Option<usize> {
    seen.iter()
        .position(|s| s.request().is_some_and(&matches))
}

fn lines(said: &[String]) -> String {
    if said.is_empty() {
        return "  (it said nothing at all)".to_owned();
    }
    said.iter()
        .map(|s| format!("  {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// The far end
// ---------------------------------------------------------------------------

/// One message the trunk saw, kept parsed and as octets.
///
/// The octets are kept because one thing worth asserting -- that a
/// retransmitted request is answered identically -- is about the bytes and not
/// about what they parse to.
#[derive(Debug, Clone)]
struct Seen {
    message: Message,
    octets: Vec<u8>,
}

impl Seen {
    fn request(&self) -> Option<&Request> {
        self.message.as_request()
    }

    fn response(&self) -> Option<&Response> {
        self.message.as_response()
    }
}

/// A digest the trunk checked, with both answers kept so that a test can say
/// which one was wrong and over what.
#[derive(Debug, Clone)]
struct Checked {
    method: String,
    username: String,
    realm: String,
    nonce: String,
    /// The uri the credentials claim to have been computed over.
    uri: String,
    /// And the request URI they arrived on, which has to be the same thing.
    request_uri: String,
    offered: String,
    expected: String,
    /// Whether the nonce is one this trunk ever issued.
    known_nonce: bool,
}

/// Something a test has asked the trunk to do of its own accord.
#[derive(Debug, Clone, Copy)]
enum Order {
    /// The keep-alive several trunks send every thirty seconds.
    Options,
    /// Hang up from the far end.
    Bye,
    /// And send that same BYE again, octet for octet, as a slow path does.
    ByeAgain,
    /// Ask to switch the call to T.38, the way a gateway does when it hears a
    /// fax tone.
    T38ReInvite,
    /// Renegotiate the call it is already on, offering the same audio again.
    ReInvite,
    /// And send that same re-INVITE again, octet for octet.
    ReInviteAgain,
    /// Answer, at last, a BYE it took in and said nothing about. 17.1.2.2
    /// leaves that transaction running for 32 s, so a trunk that was busy
    /// rather than gone gets round to it long after the person has redialled.
    AnswerTheOldBye,
}

/// What the trunk does with a BYE it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByeManner {
    /// 200, which is what a trunk that is there does.
    Answer,
    /// Nothing at all: the thing at the far end has gone, which is what a call
    /// that drops of its own accord looks like from this side.
    Ignore,
    /// 481, from a far end that had already destroyed the dialog.
    Refuse,
}

/// What the trunk answers an INVITE with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// mu-law on its own RTP port, which is what a trunk sends.
    G711,
    /// A codec that was never offered, to prove the call is refused rather
    /// than carried.
    G729Only,
}

/// How the trunk is to behave. Read afresh for every message, so a test can
/// change its mind between calls.
#[derive(Debug, Clone)]
struct Manner {
    /// Final responses to send instead of answering, one per call.
    refuse: VecDeque<(u16, String)>,
    answer: Answer,
    /// What a BYE gets.
    bye: ByeManner,
    /// Calls to ring and never answer, one per entry: a far end with a
    /// telephone that goes on ringing until the caller gives up.
    hold: usize,
}

/// The trunk's half of a dialog, kept so it can send an in-dialog request.
#[derive(Debug, Clone)]
struct Leg {
    call_id: String,
    /// The agent's From with its tag: our To on anything we send.
    caller: String,
    /// Our To with our tag: our From on anything we send.
    callee: String,
    cseq: u32,
}

/// Everything a test can read back out of the trunk.
#[derive(Debug, Default)]
struct Book {
    seen: Vec<Seen>,
    /// Both directions, in order, one line each: what a capture would show.
    transcript: Vec<String>,
    orders: VecDeque<Order>,
    checked: Vec<Checked>,
    /// Where the agent's SIP port is, learned from what arrives rather than
    /// assumed, exactly as a trunk behind a router has to learn it.
    agent: Option<SocketAddr>,
    /// The agent's Contact, which is where in-dialog requests are addressed.
    agent_contact: Option<String>,
    call: Option<Leg>,
    /// The last BYE sent, kept so it can be sent again unchanged.
    last_bye: Option<Vec<u8>>,
    /// And the last re-INVITE, for the same reason.
    last_re_invite: Option<Vec<u8>>,
    /// A BYE that arrived and was deliberately not answered, kept so that it
    /// can be answered late.
    unanswered_bye: Option<Request>,
    /// Whether that late answer has gone out.
    answered_late: bool,
}

/// A trunk on the loopback: a SIP socket, an RTP socket that echoes, and a
/// thread apiece.
#[derive(Debug)]
struct FarEnd {
    sip: SocketAddr,
    rtp_port: u16,
    book: Arc<Mutex<Book>>,
    manner: Arc<Mutex<Manner>>,
    quit: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl FarEnd {
    fn start() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("the trunk's SIP port");
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .expect("a read timeout on the SIP port");
        let sip = socket.local_addr().expect("the SIP port's own address");

        let rtp = UdpSocket::bind("127.0.0.1:0").expect("the trunk's RTP port");
        rtp.set_read_timeout(Some(Duration::from_millis(10)))
            .expect("a read timeout on the RTP port");
        let rtp_port = rtp.local_addr().expect("the RTP port's own address").port();

        let book = Arc::new(Mutex::new(Book::default()));
        let manner = Arc::new(Mutex::new(Manner {
            refuse: VecDeque::new(),
            answer: Answer::G711,
            bye: ByeManner::Answer,
            hold: 0,
        }));
        let quit = Arc::new(AtomicBool::new(false));

        let trunk = Trunk {
            socket,
            rtp_port,
            book: Arc::clone(&book),
            manner: Arc::clone(&manner),
            quit: Arc::clone(&quit),
            tag: rand::token(8),
            nonces: Vec::new(),
            cseq: 1,
            version: 1,
            held: None,
        };
        let serving = thread::spawn(move || trunk.run());
        let echoing = {
            let quit = Arc::clone(&quit);
            thread::spawn(move || echo(rtp, quit))
        };

        Self {
            sip,
            rtp_port,
            book,
            manner,
            quit,
            threads: vec![serving, echoing],
        }
    }

    fn address(&self) -> SocketAddr {
        self.sip
    }

    fn rtp_port(&self) -> u16 {
        self.rtp_port
    }

    /// Refuse the next call with this code, and answer the one after it.
    fn refuse_next_call(&self, code: u16, reason: &str) {
        if let Ok(mut manner) = self.manner.lock() {
            manner.refuse.push_back((code, reason.to_owned()));
        }
    }

    fn answer_with(&self, answer: Answer) {
        if let Ok(mut manner) = self.manner.lock() {
            manner.answer = answer;
        }
    }

    /// What every BYE from here on gets.
    fn treat_byes(&self, manner: ByeManner) {
        if let Ok(mut held) = self.manner.lock() {
            held.bye = manner;
        }
    }

    /// Ring the next call and never answer it.
    fn hold_next_call(&self) {
        if let Ok(mut manner) = self.manner.lock() {
            manner.hold += 1;
        }
    }

    /// Whether the BYE it sat on has been answered after the fact.
    fn answered_the_old_bye(&self) -> bool {
        self.book.lock().map(|b| b.answered_late).unwrap_or(false)
    }

    fn order(&self, order: Order) {
        if let Ok(mut book) = self.book.lock() {
            book.orders.push_back(order);
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.book.lock().map(|b| b.seen.clone()).unwrap_or_default()
    }

    fn requests(&self, method: Method) -> Vec<Request> {
        self.seen()
            .iter()
            .filter_map(Seen::request)
            .filter(|r| r.method == method)
            .cloned()
            .collect()
    }

    /// Every response the agent sent to a request of this method, which is
    /// what the CSeq of the response says rather than what it is a response
    /// to by position.
    fn answers_to(&self, method: Method) -> Vec<Response> {
        self.seen()
            .iter()
            .filter_map(Seen::response)
            .filter(|r| r.headers.cseq().is_some_and(|(_, m)| m == method))
            .cloned()
            .collect()
    }

    /// The same, as the octets that actually arrived.
    fn answer_octets(&self, method: Method) -> Vec<Vec<u8>> {
        self.seen()
            .iter()
            .filter(|s| {
                s.response()
                    .and_then(|r| r.headers.cseq())
                    .is_some_and(|(_, m)| m == method)
            })
            .map(|s| s.octets.clone())
            .collect()
    }

    fn checked(&self) -> Vec<Checked> {
        self.book.lock().map(|b| b.checked.clone()).unwrap_or_default()
    }

    /// Both directions, in order: what a capture of this call would show.
    fn transcript(&self) -> String {
        self.book
            .lock()
            .map(|b| {
                if b.transcript.is_empty() {
                    "  (nothing ever reached the trunk)".to_owned()
                } else {
                    b.transcript
                        .iter()
                        .map(|l| format!("  {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            })
            .unwrap_or_default()
    }
}

impl Drop for FarEnd {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// The RTP far end: whatever arrives goes straight back where it came from.
///
/// Back to the source rather than to the address in the offer, because that is
/// what symmetric RTP means and what every trunk does -- and because an echo
/// aimed at the advertised address would prove nothing about the one-way-audio
/// case this crate takes trouble over.
fn echo(socket: UdpSocket, quit: Arc<AtomicBool>) {
    let mut buffer = vec![0u8; 2048];
    while !quit.load(Ordering::Relaxed) {
        if let Ok((n, from)) = socket.recv_from(&mut buffer)
            && n > 0
        {
            let _ = socket.send_to(&buffer[..n], from);
        }
    }
}

/// The SIP half of the trunk, and the thread that is it.
#[derive(Debug)]
struct Trunk {
    socket: UdpSocket,
    rtp_port: u16,
    book: Arc<Mutex<Book>>,
    manner: Arc<Mutex<Manner>>,
    quit: Arc<AtomicBool>,
    /// The tag on this end of the dialog, fresh for each call it answers.
    tag: String,
    /// Every nonce issued, so that one coming back can be recognised as ours.
    nonces: Vec<String>,
    /// The sequence number for requests the trunk sends out of dialog.
    cseq: u32,
    /// The o= version of the descriptions it sends, which has to move when the
    /// description does (RFC 4566 5.2).
    version: u64,
    /// An INVITE it is ringing and has not answered, kept so that a CANCEL can
    /// be answered the way 9.2 says: 200 for the CANCEL, 487 for the INVITE.
    held: Option<(Request, SocketAddr)>,
}

impl Trunk {
    fn run(mut self) {
        let mut buffer = vec![0u8; 8192];
        while !self.quit.load(Ordering::Relaxed) {
            self.take_orders();
            let Ok((n, from)) = self.socket.recv_from(&mut buffer) else {
                continue;
            };
            let Some(message) = Message::parse(&buffer[..n]) else {
                continue;
            };
            self.record(&message, &buffer[..n], from);
            if let Message::Request(request) = message {
                self.handle(request, from);
            }
        }
    }

    // ---- what arrives ---------------------------------------------------

    fn handle(&mut self, request: Request, from: SocketAddr) {
        match request.method {
            Method::Register => self.register(&request, from),
            Method::Invite => self.invite(&request, from),
            Method::Bye => {
                let manner = self.manner.lock().map(|m| m.bye).unwrap_or(ByeManner::Answer);
                match manner {
                    ByeManner::Ignore => {
                        // Kept, so that a test can have it answered late, which
                        // is what 17.1.2.2's 32 s of transaction is for.
                        self.note("out: nothing at all -- this trunk is sitting on the BYE".to_owned());
                        if let Ok(mut book) = self.book.lock() {
                            book.unanswered_bye = Some(request.clone());
                        }
                        return;
                    }
                    ByeManner::Answer => {
                        let response = self.reply(&request, from, 200, "OK");
                        self.send(&Message::Response(response), from);
                    }
                    ByeManner::Refuse => {
                        let response =
                            self.reply(&request, from, 481, "Call/Transaction Does Not Exist");
                        self.send(&Message::Response(response), from);
                    }
                }
                if let Ok(mut book) = self.book.lock() {
                    book.call = None;
                }
            }
            // An ACK is answered by nothing at all (17.1.1.3); it is recorded,
            // which is the whole of what this trunk wants with one.
            Method::Ack => {}
            // 9.2: the CANCEL is answered 200, and the INVITE it names gets a
            // 487 of its own. A trunk that answered only the CANCEL would leave
            // the caller's INVITE transaction running to its own deadline.
            Method::Cancel => {
                let response = self.reply(&request, from, 200, "OK");
                self.send(&Message::Response(response), from);
                if let Some((invite, to)) = self.held.take() {
                    let mut gone = self.reply(&invite, to, 487, "Request Terminated");
                    self.tagged(&mut gone);
                    self.send(&Message::Response(gone), to);
                    if let Ok(mut book) = self.book.lock() {
                        book.call = None;
                    }
                }
            }
            Method::Options | Method::Info | Method::Update => {
                let response = self.reply(&request, from, 200, "OK");
                self.send(&Message::Response(response), from);
            }
            _ => {
                let response = self.reply(&request, from, 405, "Method Not Allowed");
                self.send(&Message::Response(response), from);
            }
        }
    }

    /// 22.2: nobody registers on the first try.
    fn register(&mut self, request: &Request, from: SocketAddr) {
        let Some(credentials) = request.headers.get("Authorization") else {
            let challenge = self.challenge(false);
            let mut response = self.reply(request, from, 401, "Unauthorized");
            response.headers.push("WWW-Authenticate", challenge);
            self.tagged(&mut response);
            self.send(&Message::Response(response), from);
            return;
        };
        if !self.check(request, credentials, "REGISTER") {
            let challenge = self.challenge(false);
            let mut response = self.reply(request, from, 401, "Unauthorized");
            response.headers.push("WWW-Authenticate", challenge);
            self.tagged(&mut response);
            self.send(&Message::Response(response), from);
            return;
        }

        let expires = request.headers.get("Expires").unwrap_or("120").to_owned();
        let mut response = self.reply(request, from, 200, "OK");
        // 10.2.4: the registrar decides the expiry, and says so.
        response.headers.push("Expires", expires);
        if let Some(contact) = request.headers.get("Contact") {
            response.headers.push("Contact", contact.to_owned());
        }
        self.tagged(&mut response);
        self.send(&Message::Response(response), from);
    }

    /// 22.3, then 13.3: challenge it, then ring it.
    fn invite(&mut self, request: &Request, from: SocketAddr) {
        let answered = match request.headers.get("Proxy-Authorization") {
            None => false,
            Some(credentials) => self.check(request, credentials, "INVITE"),
        };
        if !answered {
            let challenge = self.challenge(true);
            let mut response = self.reply(request, from, 407, "Proxy Authentication Required");
            response.headers.push("Proxy-Authenticate", challenge);
            self.tagged(&mut response);
            self.send(&Message::Response(response), from);
            return;
        }

        let refusal = self
            .manner
            .lock()
            .ok()
            .and_then(|mut manner| manner.refuse.pop_front());
        let trying = self.reply(request, from, 100, "Trying");
        self.send(&Message::Response(trying), from);
        if let Some((code, reason)) = refusal {
            let mut response = self.reply(request, from, code, &reason);
            self.tagged(&mut response);
            self.send(&Message::Response(response), from);
            return;
        }

        // A fresh tag: this is a new dialog, and a trunk that reused the last
        // call's tag would be describing the last call.
        self.tag = rand::token(8);
        self.version += 1;

        let mut ringing = self.reply(request, from, 180, "Ringing");
        self.tagged(&mut ringing);
        ringing.headers.push("Contact", self.contact());
        // The To with this trunk's tag on it, which is this end of the early
        // dialog (12.1.1).
        let callee = ringing.headers.get("To").unwrap_or_default().to_owned();
        self.send(&Message::Response(ringing), from);

        // A call this trunk has been told to ring and not answer. The leg is
        // recorded all the same: an early dialog is a dialog, and a CANCEL for
        // it has to be answered out of it.
        let hold = self
            .manner
            .lock()
            .map(|mut m| {
                let holding = m.hold > 0;
                m.hold = m.hold.saturating_sub(1);
                holding
            })
            .unwrap_or(false);
        if hold {
            self.held = Some((request.clone(), from));
            let leg = Leg {
                call_id: request.headers.call_id().unwrap_or_default().to_owned(),
                caller: request.headers.get("From").unwrap_or_default().to_owned(),
                callee,
                cseq: 1,
            };
            if let Ok(mut book) = self.book.lock() {
                book.call = Some(leg);
            }
            return;
        }

        let answer = self
            .manner
            .lock()
            .map(|m| m.answer)
            .unwrap_or(Answer::G711);
        let mut ok = self.reply(request, from, 200, "OK");
        self.tagged(&mut ok);
        ok.headers.push("Contact", self.contact());
        ok.headers.push("Content-Type", "application/sdp");
        ok.body = self.description(answer).into_bytes();

        let leg = Leg {
            call_id: request.headers.call_id().unwrap_or_default().to_owned(),
            caller: request.headers.get("From").unwrap_or_default().to_owned(),
            callee: ok.headers.get("To").unwrap_or_default().to_owned(),
            cseq: 1,
        };
        if let Ok(mut book) = self.book.lock() {
            book.call = Some(leg);
        }
        self.send(&Message::Response(ok), from);
    }

    // ---- authentication, checked the long way round ----------------------

    fn challenge(&mut self, from_proxy: bool) -> String {
        let nonce = format!("{}{:02}", rand::token(20), self.nonces.len());
        self.nonces.push(nonce.clone());
        if from_proxy {
            // With the algorithm named, which a client is then expected to
            // echo back and which several trunks do send.
            format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\", qop=\"auth\", algorithm=MD5")
        } else {
            format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\", qop=\"auth\"")
        }
    }

    /// Whether the credentials are right, computed here from RFC 2617's own
    /// formula. Both answers are kept whatever the verdict, so a test can say
    /// what the agent hashed rather than only that it was wrong.
    fn check(&self, request: &Request, credentials: &str, method: &str) -> bool {
        let items = parameters(credentials);
        let ha1 = md5::hex(
            format!(
                "{}:{}:{PASSWORD}",
                field(&items, "username"),
                field(&items, "realm")
            )
            .as_bytes(),
        );
        let ha2 = md5::hex(format!("{method}:{}", field(&items, "uri")).as_bytes());
        let qop = field(&items, "qop");
        let expected = if qop.is_empty() {
            // The RFC 2069 shape, for a challenge that offered no qop. This
            // trunk always offers one, so this branch is here to be honest
            // about the formula rather than because it is reached.
            md5::hex(format!("{ha1}:{}:{ha2}", field(&items, "nonce")).as_bytes())
        } else {
            md5::hex(
                format!(
                    "{ha1}:{}:{}:{}:{qop}:{ha2}",
                    field(&items, "nonce"),
                    field(&items, "nc"),
                    field(&items, "cnonce")
                )
                .as_bytes(),
            )
        };

        let checked = Checked {
            method: method.to_owned(),
            username: field(&items, "username").to_owned(),
            realm: field(&items, "realm").to_owned(),
            nonce: field(&items, "nonce").to_owned(),
            uri: field(&items, "uri").to_owned(),
            request_uri: request.uri.to_string(),
            offered: field(&items, "response").to_owned(),
            expected,
            known_nonce: self.nonces.iter().any(|n| n == field(&items, "nonce")),
        };
        let good = checked.known_nonce
            && checked.offered == checked.expected
            && checked.realm == REALM
            && checked.username == USERNAME;
        if let Ok(mut book) = self.book.lock() {
            book.checked.push(checked);
        }
        good
    }

    // ---- what the trunk does of its own accord ---------------------------

    fn take_orders(&mut self) {
        let orders: Vec<Order> = self
            .book
            .lock()
            .map(|mut b| b.orders.drain(..).collect())
            .unwrap_or_default();
        for order in orders {
            self.obey(order);
        }
    }

    fn obey(&mut self, order: Order) {
        let (agent, contact, leg, last_bye, last_re_invite) = {
            let Ok(book) = self.book.lock() else { return };
            (
                book.agent,
                book.agent_contact.clone(),
                book.call.clone(),
                book.last_bye.clone(),
                book.last_re_invite.clone(),
            )
        };
        let (Some(agent), Some(contact)) = (agent, contact) else {
            return;
        };

        match order {
            Order::Options => {
                self.cseq += 1;
                let mut headers = self.heading(&rand::branch());
                headers.push("From", format!("<sip:trunk@{REALM}>;tag={}", self.tag));
                headers.push("To", format!("<{contact}>"));
                headers.push("Call-ID", rand::call_id("127.0.0.1"));
                headers.push("CSeq", format!("{} OPTIONS", self.cseq));
                headers.push("Contact", self.contact());
                self.send_request(Method::Options, &contact, headers, Vec::new(), agent);
            }
            Order::Bye => {
                let Some(mut leg) = leg else { return };
                leg.cseq += 1;
                let mut headers = self.heading(&rand::branch());
                headers.push("From", leg.callee.clone());
                headers.push("To", leg.caller.clone());
                headers.push("Call-ID", leg.call_id.clone());
                headers.push("CSeq", format!("{} BYE", leg.cseq));
                let octets = self.send_request(Method::Bye, &contact, headers, Vec::new(), agent);
                if let Ok(mut book) = self.book.lock() {
                    book.last_bye = Some(octets);
                    book.call = Some(leg);
                }
            }
            Order::ByeAgain => {
                let Some(octets) = last_bye else { return };
                self.note("out: that same BYE once more".to_owned());
                let _ = self.socket.send_to(&octets, agent);
            }
            Order::T38ReInvite => {
                let Some(mut leg) = leg else { return };
                leg.cseq += 1;
                self.version += 1;
                let mut headers = self.heading(&rand::branch());
                headers.push("From", leg.callee.clone());
                headers.push("To", leg.caller.clone());
                headers.push("Call-ID", leg.call_id.clone());
                headers.push("CSeq", format!("{} INVITE", leg.cseq));
                headers.push("Contact", self.contact());
                headers.push("Content-Type", "application/sdp");
                let fax = self.fax_offer();
                self.send_request(Method::Invite, &contact, headers, fax.into_bytes(), agent);
                if let Ok(mut book) = self.book.lock() {
                    book.call = Some(leg);
                }
            }
            Order::ReInvite => {
                let Some(mut leg) = leg else { return };
                leg.cseq += 1;
                self.version += 1;
                let mut headers = self.heading(&rand::branch());
                headers.push("From", leg.callee.clone());
                headers.push("To", leg.caller.clone());
                headers.push("Call-ID", leg.call_id.clone());
                headers.push("CSeq", format!("{} INVITE", leg.cseq));
                headers.push("Contact", self.contact());
                headers.push("Content-Type", "application/sdp");
                let offer = self.description(Answer::G711);
                let octets =
                    self.send_request(Method::Invite, &contact, headers, offer.into_bytes(), agent);
                if let Ok(mut book) = self.book.lock() {
                    book.last_re_invite = Some(octets);
                    book.call = Some(leg);
                }
            }
            Order::ReInviteAgain => {
                let Some(octets) = last_re_invite else { return };
                self.note("out: that same re-INVITE once more".to_owned());
                let _ = self.socket.send_to(&octets, agent);
            }
            Order::AnswerTheOldBye => {
                let waiting = self
                    .book
                    .lock()
                    .ok()
                    .and_then(|b| b.unanswered_bye.clone());
                let Some(request) = waiting else { return };
                let response = self.reply(&request, agent, 200, "OK");
                self.send(&Message::Response(response), agent);
                if let Ok(mut book) = self.book.lock() {
                    book.answered_late = true;
                }
            }
        }
    }

    /// The first two headers of anything the trunk sends.
    fn heading(&self, branch: &str) -> Headers {
        let mut headers = Headers::new();
        headers.push(
            "Via",
            format!("SIP/2.0/UDP {};branch={branch}", self.own_address()),
        );
        headers.push("Max-Forwards", "70");
        headers
    }

    fn send_request(
        &self,
        method: Method,
        uri: &str,
        headers: Headers,
        body: Vec<u8>,
        to: SocketAddr,
    ) -> Vec<u8> {
        let uri = Uri::parse(uri).unwrap_or_else(|| Uri::user_at(USERNAME, "127.0.0.1"));
        let request = Request {
            method,
            uri,
            headers,
            body,
        };
        self.send(&Message::Request(request), to)
    }

    // ---- building what goes back -----------------------------------------

    /// 8.2.6.2: a response copies the request's Via, From, To, Call-ID and
    /// CSeq exactly as they arrived, with RFC 3581's rport filled in on the
    /// topmost Via.
    fn reply(&self, request: &Request, from: SocketAddr, code: u16, reason: &str) -> Response {
        let mut headers = Headers::new();
        let mut topmost = true;
        for value in request.headers.all("Via") {
            let value = if topmost {
                topmost = false;
                fill_in_rport(value, from)
            } else {
                value.to_owned()
            };
            headers.push("Via", value);
        }
        for name in ["From", "To", "Call-ID", "CSeq"] {
            for value in request.headers.all(name) {
                headers.push(name, value.to_owned());
            }
        }
        headers.push("Server", "a trunk that exists only in this test");
        Response {
            code,
            reason: reason.to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    /// 12.1.1: the tag on the To of a response is what makes the dialog.
    fn tagged(&self, response: &mut Response) {
        let Some(mut to) = response.headers.to() else {
            return;
        };
        if to.tag().is_none() {
            to.set_parameter("tag", &self.tag);
            response.headers.set("To", to.to_string());
        }
    }

    fn own_address(&self) -> SocketAddr {
        self.socket
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    fn contact(&self) -> String {
        format!("<sip:trunk@{}>", self.own_address())
    }

    /// What the trunk answers with. One law, its own port, and nothing else --
    /// or, when a test asks for it, a codec that was never offered.
    fn description(&self, answer: Answer) -> String {
        let (formats, rtpmap) = match answer {
            Answer::G711 => ("0", "a=rtpmap:0 PCMU/8000\r\n"),
            Answer::G729Only => ("18", "a=rtpmap:18 G729/8000\r\n"),
        };
        format!(
            "v=0\r\n\
             o=- {version} {version} IN IP4 127.0.0.1\r\n\
             s=-\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=audio {port} RTP/AVP {formats}\r\n\
             {rtpmap}\
             a=ptime:20\r\n\
             a=sendrecv\r\n",
            version = self.version,
            port = self.rtp_port,
        )
    }

    /// The offer a gateway sends when it has heard a fax tone: UDPTL rather
    /// than RTP, and a format that is a name and not a number.
    fn fax_offer(&self) -> String {
        format!(
            "v=0\r\n\
             o=- {version} {version} IN IP4 127.0.0.1\r\n\
             s=-\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=image {port} udptl t38\r\n\
             a=T38FaxVersion:0\r\n\
             a=T38MaxBitRate:14400\r\n\
             a=T38FaxRateManagement:transferredTCF\r\n\
             a=T38FaxUdpEC:t38UDPRedundancy\r\n",
            version = self.version,
            port = self.rtp_port,
        )
    }

    // ---- the socket, and the transcript ----------------------------------

    fn send(&self, message: &Message, to: SocketAddr) -> Vec<u8> {
        let octets = message.to_bytes();
        self.note(format!("out: {}", summarise(message)));
        let _ = self.socket.send_to(&octets, to);
        octets
    }

    fn record(&self, message: &Message, octets: &[u8], from: SocketAddr) {
        let Ok(mut book) = self.book.lock() else {
            return;
        };
        book.agent = Some(from);
        if let Some(request) = message.as_request()
            && let Some(contact) = request.headers.contact()
        {
            book.agent_contact = Some(contact.uri.to_string());
        }
        book.transcript.push(format!("in:  {}", summarise(message)));
        book.seen.push(Seen {
            message: message.clone(),
            octets: octets.to_vec(),
        });
    }

    fn note(&self, line: String) {
        if let Ok(mut book) = self.book.lock() {
            book.transcript.push(line);
        }
    }
}

/// One line of the transcript: enough to follow a call, not enough to drown
/// the failure it is printed beside.
fn summarise(message: &Message) -> String {
    match message {
        Message::Request(r) => format!(
            "{} {} [{}]",
            r.method,
            r.uri,
            r.headers.get("CSeq").unwrap_or("no CSeq")
        ),
        Message::Response(r) => format!(
            "{} {} [{}]",
            r.code,
            r.reason,
            r.headers.get("CSeq").unwrap_or("no CSeq")
        ),
    }
}

/// RFC 3581 4: a client that asked for rport is told the address and port its
/// request actually arrived from.
///
/// The value replaces the bare parameter rather than being added beside it,
/// because a reader looking for `rport` finds the first one -- and a valueless
/// first one is exactly the shape that makes this look as though it worked.
fn fill_in_rport(via: &str, from: SocketAddr) -> String {
    let mut out = String::with_capacity(via.len() + 32);
    for (n, part) in via.split(';').enumerate() {
        if n > 0 {
            out.push(';');
        }
        if part.trim().eq_ignore_ascii_case("rport") {
            out.push_str(&format!("rport={};received={}", from.port(), from.ip()));
        } else {
            out.push_str(part);
        }
    }
    out
}

/// Split the parameters of an Authorization or Proxy-Authorization value.
///
/// Its own parser, deliberately. A test that asked `sip::auth` to read back
/// what `sip::auth` wrote would agree with it however wrong both were. The
/// only thing here that is more than a split on commas is the quoting: a nonce
/// is a server's opaque string and is entitled to have a comma in it.
fn parameters(value: &str) -> Vec<(String, String)> {
    let text = value.trim();
    let rest = match text.find(char::is_whitespace) {
        Some(at) => text[at..].trim_start(),
        None => "",
    };
    let mut out = Vec::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut start = 0;
    for (at, ch) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                push_parameter(&mut out, &rest[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    push_parameter(&mut out, &rest[start..]);
    out
}

fn push_parameter(out: &mut Vec<(String, String)>, item: &str) {
    let item = item.trim();
    let Some((name, value)) = item.split_once('=') else {
        return;
    };
    let value = value.trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    out.push((name.trim().to_ascii_lowercase(), unquoted.to_owned()));
}

fn field<'a>(items: &'a [(String, String)], name: &str) -> &'a str {
    items
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
        .unwrap_or_default()
}
