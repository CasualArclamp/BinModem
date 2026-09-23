//! The user agent: registering, placing a call, and staying on it.
//!
//! RFC 3261, and specifically the subset of it a modem needs. That subset is
//! smaller than the document but not as small as it first looks, because the
//! parts that can be skipped and the parts that cannot are not sorted by how
//! interesting they are. A trunk will not accept a call from an agent that
//! cannot answer a digest challenge (22), will send its answers to the wrong
//! address unless the agent asks for rport (RFC 3581), and will route
//! in-dialog requests into a hole unless the agent keeps the route set from
//! the Record-Route headers (12.1.2). None of that is optional in practice.
//!
//! What is left out, deliberately: TLS, ICE, SRTP, a general transaction
//! layer, forking, and the subscribe/notify machinery. One call at a time, and
//! the handful of transactions that a call needs. A proper stack keeps a table
//! of transactions keyed by branch because it might have hundreds; this has at
//! most three, so they are three fields and the code that services them can be
//! read in one sitting.
//!
//! # How it runs
//!
//! One thread owns the link and everything derived from it. The window and
//! the modem talk to it through a queue of commands in and a queue of events
//! out, both behind mutexes, the same arrangement the audio line uses. That is
//! not just convention: a SIP agent is a set of timers, and the thread that
//! owns the socket is the only place a timer can be serviced promptly without
//! a great deal of ceremony about who may touch what.
//!
//! # Which transport
//!
//! UDP or TCP, as the account says (18). Almost none of this file knows which:
//! [`transport::Link`] holds either the datagram socket or the connection and
//! its half-read buffer, and offers the two things the agent wants -- put
//! these octets on the wire, and give me the next message that has arrived.
//! Three places do know, and they are the three the RFC makes different: the
//! framing, which is [`transport`]'s own business; the Via and Contact, which
//! have to name the transport they arrived over (18.1.1); and the
//! retransmission timers below, which 17.1.1.2 and 17.1.2.2 turn off entirely
//! over a reliable transport.
//!
//! The media is not part of this choice. RTP is UDP whatever SIP travels over.
//!
//! # Timers
//!
//! 17.1.1.2's, unchanged. T1 is half a second -- the estimated round trip --
//! and every retransmission doubles until Timer B gives up at 64*T1. The
//! temptation on a known-slow path is to start T1 higher; the reason not to is
//! that these timers are what a far end's own duplicate suppression is built
//! around, and an agent that retransmits on its own schedule is one whose
//! duplicates arrive where the far end is not expecting them. The path here
//! carries about 750 ms one way, which means the first retransmission of
//! anything is normal and expected and not a sign of trouble.
//!
//! Over TCP the retransmissions do not happen at all -- 17.1.1.2's Timer A and
//! 17.1.2.2's Timer E simply do not run over a reliable transport, and neither
//! does 13.3.1.4's resending of a 2xx. Timer B and Timer F still do: a far end
//! that never answers is not something a transport can fix.

use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::account::Account;
use crate::auth::Challenge;
use crate::message::{Headers, Message, Method, Request, Response};
use crate::sdp::{self, Negotiated};
use crate::uri::{Address, Uri};
use crate::{message, rand};

/// The transport, declared here rather than in `lib.rs` because it is this
/// module's own business: which socket the octets go out of is not part of
/// what the crate offers, and nothing outside `ua` has any use for it.
#[path = "transport.rs"]
mod transport;

use transport::Link;

/// 17.1.1.1's estimate of a round trip, and the unit every other timer is
/// expressed in.
const T1: Duration = Duration::from_millis(500);
/// The ceiling on retransmission intervals for anything but an INVITE.
const T2: Duration = Duration::from_secs(4);
/// 64*T1: when a transaction has gone unanswered long enough to call it dead.
const TIMER_B: Duration = Duration::from_secs(32);
/// How long to keep answering retransmissions of a request we have already
/// answered. 17.2.1's Timer I, rounded up.
const ABSORB: Duration = Duration::from_secs(5);
/// How long the far end is given to say anything at all about an INVITE we
/// sent before we treat the call as failed, when it has not even sent a 100.
/// Longer than Timer B because a trunk that is ringing a real telephone can
/// take this long to say so, and it sends provisional responses while it does.
const RING_LIMIT: Duration = Duration::from_secs(120);
/// How many times one request may go out carrying credentials.
///
/// One answer to one challenge is the ordinary case. A second is allowed only
/// for RFC 7616 3.3's `stale=true` with a new nonce, which is a registrar
/// saying its nonce timed out rather than that the password is wrong -- and
/// the bound is here because a registrar that answers every set of credentials
/// with another stale challenge would otherwise be answered for ever.
const AUTH_ATTEMPTS: u32 = 3;
/// How long a number dialled while the call before it was still being
/// cancelled may wait for that call to settle.
///
/// There is one leg this thread cannot simply let go of when somebody hangs
/// up and dials again: a cancelled INVITE. 9.1's race is why -- the far end
/// may have committed to a 200 before our CANCEL reached it, and that dialog
/// has to be acknowledged and then ended or its half of the call stays up and
/// is billed -- so the number waits for the 487 instead of being refused.
///
/// Four seconds because the wait has to end somewhere and because of what is
/// above: the window gives a dial ten seconds before it tells the modem the
/// carrier is gone, and a call placed after that is one nobody is waiting for
/// and somebody is paying for. On this path a 487 takes a round trip, about
/// 1.5 s, once the CANCEL has been answered.
const HOLD_A_DIAL: Duration = Duration::from_secs(4);

/// The words [`Status::call`] is ever set to.
///
/// Named rather than written out at each end, because two crates compare
/// them. The window watches this field to know when a call has ended, so
/// that it can tell the modem -- and a state renamed on one side of that
/// comparison and not the other would not fail to compile. It would simply
/// stop noticing that calls end, which is a modem left waiting for a carrier
/// and a person typing `+++` to find out why.
pub mod state {
    /// No call, and none being placed.
    pub const IDLE: &str = "idle";
    /// An INVITE has gone out.
    pub const DIALLING: &str = "dialling";
    /// The far end is ringing.
    pub const RINGING: &str = "ringing";
    /// Somebody is calling us, and nothing has answered yet.
    pub const RINGING_HERE: &str = "ringing here";
    /// A call we placed, answered.
    pub const UP: &str = "up";
    /// A call that came to us, which we answered.
    pub const ANSWERED: &str = "answered";
    /// On its way out, and not coming back: a CANCEL sent or waiting for a
    /// provisional response to send it after.
    pub const CANCELLING: &str = "cancelling";
    /// The same, with a BYE.
    ///
    /// The agent does not sit here any more. 15.1.1: the session is over the
    /// moment the BYE is passed to its client transaction, so a call being
    /// hung up goes straight to [`IDLE`] and the BYE sees itself out. The word
    /// stays because it is part of what this crate offers and because
    /// [`is_a_call`] still has to answer for it -- a window built against an
    /// older version of this crate would otherwise read it as a live call.
    pub const HANGING_UP: &str = "hanging up";

    /// Whether this is a call something above should still be waiting on.
    ///
    /// False for a call on its way out as well as for no call at all. The
    /// difference matters because a CANCEL can go unanswered for as long as
    /// the far end likes, and a modem held waiting through that is a modem
    /// waiting on a carrier that was given up on minutes ago.
    pub fn is_a_call(state: &str) -> bool {
        !matches!(state, IDLE | CANCELLING | HANGING_UP)
    }
}

/// What the agent is doing, for something above to display.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub account: String,
    /// Whether a registration is currently good.
    pub registered: bool,
    /// When the current registration expires, in seconds from now.
    pub registration_left: u32,
    /// The address the far end sees us at, once a registrar has told us
    /// (RFC 3581's rport). Worth showing: a wrong one is the usual reason a
    /// call connects and carries no audio.
    pub public: Option<String>,
    pub local: String,
    /// Who we are on a call with, if anyone.
    pub peer: Option<String>,
    /// Where the call has got to, in words.
    pub call: String,
}

/// Something the agent did, for the log and for the line above to act on.
#[derive(Debug, Clone)]
pub enum Event {
    /// Anything worth putting in the transcript.
    Note(String),
    Registered {
        expires: u32,
    },
    RegistrationFailed {
        code: u16,
        reason: String,
    },
    /// An INVITE has gone out.
    Dialling {
        to: String,
    },
    /// 180: the far end is ringing.
    Ringing,
    /// 183 with a description: there is audio before the call is answered.
    /// A telephone would play it to the person. A modem is not interested in
    /// ringback, so the line ignores it -- but the negotiation in it is the
    /// real one, and a far end that sends early media and then answers does
    /// not always repeat it.
    EarlyMedia(Box<Negotiated>),
    /// 200: the call is up, and this is what the two ends agreed to.
    Answered(Box<Negotiated>),
    /// Somebody is calling us.
    Incoming {
        from: String,
    },
    /// The call ended, for this reason.
    Ended {
        reason: String,
    },
    /// The call never started, for this reason.
    Failed {
        code: u16,
        reason: String,
    },
}

/// What the layer above asks for.
#[derive(Debug, Clone)]
enum Command {
    Dial(String),
    Answer,
    HangUp,
    Register(bool),
}

#[derive(Debug)]
struct Shared {
    commands: Mutex<VecDeque<Command>>,
    events: Mutex<VecDeque<Event>>,
    status: Mutex<Status>,
    quit: AtomicBool,
}

/// A running agent. Dropping it takes the registration down and stops the
/// thread.
#[derive(Debug)]
pub struct Agent {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The RTP port that went into the offer, kept so the caller can match up
    /// what it opened with what was promised.
    rtp_port: u16,
}

impl Agent {
    /// Bind the socket, start the thread, and register if the account says to.
    ///
    /// `rtp_port` is the port the media side has already bound. It has to be
    /// known before the first offer goes out, which is why the media is opened
    /// first and handed here rather than the other way round.
    pub fn start(account: Account, rtp_port: u16) -> Result<Self, String> {
        let hop = resolve(account.next_hop())?;
        // Which transport comes off the account, so this signature does not
        // have to change and neither does anything that calls it.
        let link = Link::open(account.transport, account.local_port, hop)?;
        let local = link.local();

        let shared = Arc::new(Shared {
            commands: Mutex::new(VecDeque::new()),
            events: Mutex::new(VecDeque::new()),
            status: Mutex::new(Status {
                account: account.name.clone(),
                local: local.to_string(),
                call: state::IDLE.to_owned(),
                ..Status::default()
            }),
            quit: AtomicBool::new(false),
        });

        let worker = Worker::new(account, Arc::clone(&shared), link, local, hop, rtp_port);
        let thread = thread::spawn(move || worker.run());
        Ok(Self {
            shared,
            thread: Some(thread),
            rtp_port,
        })
    }

    pub fn rtp_port(&self) -> u16 {
        self.rtp_port
    }

    /// Place a call. The string is whatever was dialled: a bare number, a
    /// number with spaces in it, or a whole SIP URI.
    pub fn dial(&self, number: &str) {
        self.command(Command::Dial(number.to_owned()));
    }

    /// Take the call that is ringing.
    pub fn answer(&self) {
        self.command(Command::Answer);
    }

    /// Put it down, whatever stage it is at: cancel a call still ringing, BYE
    /// one that is up, decline one arriving.
    pub fn hang_up(&self) {
        self.command(Command::HangUp);
    }

    /// Register now, or unregister.
    pub fn register(&self, on: bool) {
        self.command(Command::Register(on));
    }

    fn command(&self, command: Command) {
        if let Ok(mut queue) = self.shared.commands.lock() {
            queue.push_back(command);
        }
    }

    /// Everything that has happened since this was last called.
    pub fn events(&self) -> Vec<Event> {
        self.shared
            .events
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // The thread wakes every 20 ms at the latest, so this is a short
            // wait -- and worth making, because what it does on the way out is
            // end the call and take the registration down. A trunk left
            // holding a registration for a program that has gone sends the
            // next incoming call into silence.
            let _ = thread.join();
        }
    }
}

/// Resolve a `host` or `host:port` to one address, defaulting to 5060.
fn resolve(target: &str) -> Result<SocketAddr, String> {
    let (host, port) = crate::uri::split_host_port(target);
    let port = port.unwrap_or(5060);
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("could not look up {target}: {e}"))?;
    prefer_ipv4(addresses).ok_or_else(|| format!("{target} resolved to nothing"))
}

/// The first IPv4 address, or the first of any family if there is no IPv4 one.
///
/// This used to be `.next()`, which is wrong here for a reason that is entirely
/// this crate's own doing rather than the resolver's: the SIP socket is bound
/// to `0.0.0.0`, so it can only speak IPv4. A trunk whose name answers AAAA
/// first -- and plenty do -- therefore failed every single `send_to` with
/// `AddrNotAvailable`, every failure was swallowed by a `let _ =`, and the only
/// thing the person saw was "dialling" for the whole two minutes of `RING_LIMIT`
/// before being told nobody answered. An address of the family the socket
/// cannot use is still returned when it is all there is, because a send that
/// fails and says so is a better answer than a lookup that claims to have found
/// nothing.
fn prefer_ipv4(addresses: impl Iterator<Item = SocketAddr>) -> Option<SocketAddr> {
    let mut fallback = None;
    for address in addresses {
        if address.is_ipv4() {
            return Some(address);
        }
        fallback = fallback.or(Some(address));
    }
    fallback
}

/// A client transaction we are waiting on: 17.1's state, minus the states
/// that only matter to a stack holding many at once.
#[derive(Debug)]
struct Pending {
    request: Request,
    to: SocketAddr,
    /// When to send it again, and how long to wait after that. Set whatever
    /// the transport is and read only over an unreliable one: 17.1.1.2's
    /// Timer A and 17.1.2.2's Timer E do not run over TCP at all, and
    /// `service_timers` is the one place that knows it.
    next: Instant,
    interval: Duration,
    /// When to give up.
    deadline: Instant,
    /// Whether a provisional response has arrived, which stops retransmission
    /// of an INVITE (17.1.1.2) without ending the transaction.
    answered_provisionally: bool,
    /// How many times this request has gone out with credentials on it, and
    /// the nonce the last set was computed over. Both are needed to tell a
    /// registrar whose nonce has expired (RFC 7616 3.3, answered again) from
    /// one that does not like our password (not answered again).
    attempts: u32,
    nonce: String,
    /// Whether a CANCEL has been sent for this INVITE. 9.1's wait is then
    /// over and what is left is the 487 -- which, if it never comes, must not
    /// hold the line at "cancelling" for the whole of `RING_LIMIT`.
    cancelled: bool,
}

impl Pending {
    fn new(request: Request, to: SocketAddr, invite: bool) -> Self {
        let now = Instant::now();
        Self {
            request,
            to,
            next: now + T1,
            interval: T1,
            deadline: now + if invite { RING_LIMIT } else { TIMER_B },
            answered_provisionally: false,
            attempts: 0,
            nonce: String::new(),
            cancelled: false,
        }
    }
}

/// A dialog: 12's shared state between the two ends of a call.
#[derive(Debug)]
struct Dialog {
    call_id: String,
    /// Us, with our tag; and them, with theirs once we know it.
    local: Address,
    remote: Address,
    /// Our sequence number, which only goes up (12.2.1.1).
    cseq: u32,
    /// The sequence number of the INVITE that made the dialog, kept apart
    /// from `cseq` because `cseq` moves on with the BYE and this is what a
    /// retransmitted 200 will still be carrying (13.3.1.4).
    invite_cseq: u32,
    /// Where in-dialog requests are addressed: their Contact (12.1.2).
    target: Uri,
    /// The route set, from the Record-Route headers, already reversed for a
    /// caller (12.1.2). Sent as Route headers on everything in the dialog.
    routes: Vec<String>,
    /// Where the packets actually go, which is the first route's host if
    /// there is a route set and the target's otherwise.
    hop: SocketAddr,
    /// Whether they answered.
    established: bool,
    /// What the media ended up as.
    media: Option<Negotiated>,
}

/// The thread.
#[derive(Debug)]
struct Worker {
    account: Account,
    shared: Arc<Shared>,
    /// The socket or the connection, and the framing that goes with it (18).
    link: Link,
    /// Where we were when the link was opened. The live answer is
    /// `self.link.local()`, which over TCP only becomes true once there is a
    /// connection to read a port off; this is what the status line shows and
    /// what the Call-IDs were seeded from.
    local: SocketAddr,
    hop: SocketAddr,
    rtp_port: u16,
    /// The address the outside world sees, learned from a Via that came back
    /// with `received` and `rport` on it (RFC 3581 4). Until a registrar has
    /// told us, our own address is the best guess there is.
    public: Option<SocketAddr>,

    registration: Option<Pending>,
    /// When to register again, and the tag and call identifier to do it under
    /// -- 10.2 wants every REGISTER for one binding in one call.
    register_at: Option<Instant>,
    register_call_id: String,
    register_tag: String,
    register_cseq: u32,
    registered_until: Option<Instant>,
    want_registration: bool,

    invite: Option<Pending>,
    bye: Option<Pending>,
    /// The CANCEL transaction. 9.1 makes a CANCEL a request in its own right
    /// and 17.1.2.2 makes it retransmit like any other non-INVITE one; it used
    /// to be sent once and forgotten, so a single lost datagram left the far
    /// end ringing a number the user had already hung up on.
    cancel: Option<Pending>,
    /// A hang-up that arrived before the far end had said anything at all.
    /// 9.1: a CANCEL must not be sent until a provisional response has, so the
    /// wish is kept here and acted on when one does.
    cancel_wanted: bool,
    /// A number dialled while the call before it was still being cancelled,
    /// and the moment the waiting stops being worth it.
    ///
    /// See [`HOLD_A_DIAL`]. This is the one case where a dial is neither
    /// placed nor refused straight away: the leg being got rid of cannot be
    /// abandoned, so the number waits for it rather than being thrown away.
    held_dial: Option<(String, Instant)>,
    call: Option<Dialog>,
    /// A call that is ringing here and has not been answered: the request, so
    /// that a 200 can be built from it when somebody picks up.
    ringing: Option<(Request, SocketAddr)>,
    /// The last provisional response sent to a request we have not given a
    /// final answer to, kept for the same reason as `answered` below. A 100 or
    /// a 180 is not final, so it never went in there, so an INVITE that
    /// arrived again fell past both of them into the dialog handling and was
    /// answered 200 by a telephone nobody had picked up.
    provisional: Vec<(String, Vec<u8>, SocketAddr, Instant)>,
    /// 13.3.1.4: our own 2xx to an INVITE, sent again at T1 until the ACK
    /// arrives, because a 2xx is the one response a UAS has to get through
    /// itself -- there is no transaction underneath it doing it.
    answer: Option<Answer>,
    /// Responses already sent, kept so a retransmitted request gets the same
    /// answer rather than a new one (17.2.1).
    answered: Vec<(String, Vec<u8>, SocketAddr, Instant)>,
}

/// A 2xx we have sent and not yet had acknowledged (13.3.1.4).
#[derive(Debug)]
struct Answer {
    octets: Vec<u8>,
    to: SocketAddr,
    /// What an ACK has to say to be this one's. The ACK for a 2xx is its own
    /// transaction with its own branch (13.2.2.4), so the branch is no use
    /// here and the Call-ID and the sequence number are what is left.
    call_id: String,
    cseq: u32,
    next: Instant,
    interval: Duration,
    deadline: Instant,
    /// Whether an answer that is never acknowledged should end the call. True
    /// for the 2xx that made the dialog, which is what 13.3.1.4 is about: no
    /// ACK means the caller never got the answer, so there is no call. False
    /// for a re-INVITE's answer, where the call is already up and tearing it
    /// down over one unacknowledged renegotiation would be the worse fault.
    bye_if_unacknowledged: bool,
}

impl Worker {
    fn new(
        account: Account,
        shared: Arc<Shared>,
        link: Link,
        local: SocketAddr,
        hop: SocketAddr,
        rtp_port: u16,
    ) -> Self {
        let want = account.register;
        Self {
            account,
            shared,
            link,
            local,
            hop,
            rtp_port,
            public: None,
            registration: None,
            register_at: want.then(Instant::now),
            register_call_id: rand::call_id(&local.ip().to_string()),
            register_tag: rand::token(10),
            register_cseq: 1,
            registered_until: None,
            want_registration: want,
            invite: None,
            bye: None,
            cancel: None,
            cancel_wanted: false,
            held_dial: None,
            call: None,
            ringing: None,
            provisional: Vec::new(),
            answer: None,
            answered: Vec::new(),
        }
    }

    fn run(mut self) {
        self.note(format!(
            "sip: {} as {} through {} over {}",
            self.account.name,
            self.account.uri(),
            self.account.next_hop(),
            self.account.transport,
        ));
        while !self.shared.quit.load(Ordering::Relaxed) {
            self.take_commands();
            self.service_timers();
            // One message at a time, with the link doing the pacing: a quiet
            // link waits 20 ms before it says there is nothing, which is often
            // enough for every timer in the protocol, and a busy one hands
            // them over as fast as this loop will take them.
            if let Some((message, from)) = self.link.receive() {
                self.handle(message, from);
            }
            self.link_news();
            self.publish();
        }
        self.shut_down();
    }

    /// Whatever the link has to say for itself, and the one thing it says that
    /// the agent has to act on.
    fn link_news(&mut self) {
        for note in self.link.notes() {
            self.note(format!("sip: {note}"));
        }
        if self.link.remade() {
            self.connection_remade();
        }
    }

    /// The connection to the next hop went, and a new one is up (18).
    ///
    /// TCP delivers what it accepts, which is exactly why 17.1.1.2 and
    /// 17.1.2.2 turn the retransmission timers off over a reliable transport.
    /// A connection that breaks takes that guarantee with it: a request
    /// written to the socket that died may have reached the far end or may
    /// not, nothing underneath will try again, and the transaction is left
    /// waiting out the whole of Timer B for an answer to something that was
    /// never delivered. So everything still in flight goes out once more on
    /// the new connection. Not on a timer, and not repeatedly -- once, for
    /// precisely the reason the timer was turned off.
    ///
    /// The registration is refreshed for a different reason. Nothing about it
    /// is cleared -- `registered_until` stands, so a line does not report
    /// itself unregistered over a hiccup the trunk may not even have noticed
    /// -- but the binding the registrar is holding names a connection that has
    /// gone, and 18.2.1 is how an inbound call would have come down it. A
    /// REGISTER now rather than at nine tenths of the expiry is what puts the
    /// binding back on a connection that exists.
    fn connection_remade(&mut self) {
        let mut again: Vec<(Vec<u8>, SocketAddr)> = [
            self.registration.as_ref(),
            self.invite.as_ref(),
            self.cancel.as_ref(),
            self.bye.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|pending| {
            (
                Message::Request(pending.request.clone()).to_bytes(),
                pending.to,
            )
        })
        .collect();
        // And 13.3.1.4's 2xx, which is waiting for an ACK that cannot come if
        // the connection went before the answer got there.
        if let Some(answer) = self.answer.as_ref() {
            again.push((answer.octets.clone(), answer.to));
        }
        if !again.is_empty() {
            self.note(format!(
                "sip: sending {} thing(s) again on the new connection",
                again.len()
            ));
        }
        for (bytes, to) in again {
            let _ = self.link.send(&bytes, to);
        }
        if self.want_registration && self.registration.is_none() {
            self.register_at = Some(Instant::now());
        }
    }

    /// What to do on the way out: end the call, then unregister. In that
    /// order, because a trunk that is holding a call open for a registration
    /// it is about to lose does not always notice the call has gone.
    fn shut_down(&mut self) {
        if self.call.is_some() {
            self.send_bye("shutting down");
            self.wait_briefly();
        }
        if self.registered_until.is_some() {
            self.send_register(0);
            self.wait_briefly();
        }
    }

    /// Give an answer a moment to arrive, without pretending to run the whole
    /// loop. Used only while shutting down.
    ///
    /// This waited about twenty milliseconds rather than four hundred. The
    /// socket's own read timeout is 20 ms, it reports that timeout as an
    /// error, and the error arm was `break` -- so the first quiet pass ended
    /// the wait, and neither the BYE nor the un-REGISTER on the way out was
    /// ever confirmed or sent again. A trunk left holding a registration and a
    /// call for a program that has gone sends the next incoming call into
    /// silence, which is the whole reason this function exists.
    fn wait_briefly(&mut self) {
        let until = Instant::now() + Duration::from_millis(400);
        while Instant::now() < until {
            // The timers run here too, or a lost BYE is lost for good: there
            // is no other loop left to retransmit it. Over TCP there is
            // nothing to retransmit and this is simply the time the answer is
            // given to arrive.
            self.service_timers();
            if let Some((message, from)) = self.link.receive() {
                self.handle(message, from);
            }
        }
    }

    // ---- what the layer above asked for -----------------------------

    fn take_commands(&mut self) {
        let commands: Vec<Command> = self
            .shared
            .commands
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default();
        for command in commands {
            match command {
                Command::Dial(number) => self.place_call(&number),
                Command::Answer => self.accept_call(),
                Command::HangUp => self.end_call(),
                Command::Register(on) => {
                    self.want_registration = on;
                    if on {
                        self.register_at = Some(Instant::now());
                    } else if self.registered_until.is_some() {
                        self.send_register(0);
                    }
                }
            }
        }
    }

    /// Place a call, or say in an event why there is not going to be one.
    ///
    /// Every path out of here that does not end in an INVITE raises
    /// [`Event::Failed`] with a code of 0, which `line.rs` reads as "the call
    /// never left this machine" and words that way. That is not tidiness. Over
    /// SIP the modem above is stepped by arriving RTP and by nothing else:
    /// `ATD` has already put it off hook and taken its dial string, and with no
    /// call there are no samples, so none of the modem's own timers advance and
    /// it never times out. A dial refused in silence therefore leaves it
    /// waiting for a carrier on a call that was never placed, until somebody
    /// forces the line down. The reconciliation above cannot rescue it either:
    /// that asks whether a call which *was* active has ended, and here there
    /// never was one.
    ///
    /// The refusal below used to be a note and nothing else, which is exactly
    /// that fault.
    ///
    /// The refusal is for a call that is genuinely up. A call on its way out
    /// is not one to refuse a number over, and that distinction is the whole
    /// of the fault people kept reporting as "you can not redial": `self.call`
    /// was cleared in `finish_call` and nowhere else, so from the moment a BYE
    /// went out until the far end answered it -- about 1.5 s on this path, and
    /// 32 s of Timer F when the far end has gone, which is exactly when a call
    /// drops -- every number dialled was refused. Somebody whose call had just
    /// dropped and who pressed Call again got nothing. 15.1.1 says the session
    /// is over when the BYE is passed to its transaction, so `send_bye` now
    /// ends it there and this sees no call at all.
    fn place_call(&mut self, dialled: &str) {
        // Except for the one leg that cannot be let go of yet, which the
        // number waits for rather than being refused over.
        if self.still_getting_rid_of_a_call() {
            self.hold_a_dial(dialled);
            return;
        }
        if self.call.is_some() || self.invite.is_some() {
            self.event(Event::Failed {
                code: 0,
                reason: format!(
                    "this line is already on a call, so {dialled} was not dialled; \
                     put the first one down before placing another"
                ),
            });
            return;
        }
        let target = self.account.dial_uri(dialled);
        let hop = match resolve(self.account.next_hop()) {
            Ok(hop) => hop,
            // Nothing went out, so there is no status code to report and 0 is
            // what says so. The call state has never become anything but idle
            // here, which is why this event is the only thing the layer above
            // has to go on.
            Err(e) => {
                self.event(Event::Failed {
                    code: 0,
                    reason: e,
                });
                return;
            }
        };
        let call_id = rand::call_id(&self.local.ip().to_string());
        let mut local = Address::new(self.account.uri());
        if let Some(display) = &self.account.display {
            local = local.with_display(display);
        }
        local.set_parameter("tag", &rand::token(10));
        let remote = Address::new(target.clone());
        let branch = rand::branch();

        let offer = sdp::offer(
            &self.contact_host(),
            self.rtp_port,
            &self.account.laws,
            self.account.ptime_ms,
            None,
        );
        let mut request = self.request(
            Method::Invite,
            target.clone(),
            &call_id,
            &local,
            &remote,
            1,
            &branch,
        );
        request
            .headers
            .set("Content-Type", sdp::CONTENT_TYPE.to_owned());
        request.body = offer.into_bytes();

        self.call = Some(Dialog {
            call_id,
            local,
            remote,
            cseq: 1,
            invite_cseq: 1,
            target,
            routes: Vec::new(),
            hop,
            established: false,
            media: None,
        });
        self.event(Event::Dialling {
            to: dialled.to_owned(),
        });
        self.send(&Message::Request(request.clone()), hop);
        self.invite = Some(Pending::new(request, hop, true));
        self.set_call_state(state::DIALLING);
    }

    /// Answer the call ringing here, or say in an event why there is not going
    /// to be one.
    ///
    /// [`Worker::place_call`]'s rule, from the answering side and for the same
    /// reason: `ATA` has put the modem off hook into its answer handshake, and
    /// over SIP nothing steps it but arriving RTP, so a path out of here that
    /// refuses the call in silence leaves it waiting for a carrier for ever.
    /// The first one below used to be a note and nothing else.
    fn accept_call(&mut self) {
        let Some((request, from)) = self.ringing.take() else {
            self.event(Event::Failed {
                code: 0,
                reason: "there was no call ringing here to answer".to_owned(),
            });
            return;
        };
        let description = String::from_utf8_lossy(&request.body).into_owned();
        let offer = match sdp::Sdp::parse(&description) {
            Ok(offer) => offer,
            Err(e) => {
                self.respond(&request, from, 488, "Not Acceptable Here");
                self.refused_here(488, format!("the offer made no sense: {e}"));
                return;
            }
        };
        let answered = sdp::answer(
            &offer,
            &self.contact_host(),
            self.rtp_port,
            &self.account.laws,
            self.account.ptime_ms,
        );
        let (body, negotiated) = match answered {
            Ok(pair) => pair,
            Err(e) => {
                self.respond(&request, from, 488, "Not Acceptable Here");
                self.refused_here(488, e);
                return;
            }
        };
        let mut response = self.response_to(&request, 200, "OK");
        response
            .headers
            .set("Contact", self.contact().to_string());
        response
            .headers
            .set("Content-Type", sdp::CONTENT_TYPE.to_owned());
        response.body = body.into_bytes();
        // 12.1.1: our tag goes on the To of the response, and that is what
        // makes the dialog.
        let mut to = response.headers.to().unwrap_or_default();
        if to.tag().is_none() {
            to.set_parameter("tag", &rand::token(10));
            response.headers.set("To", to.to_string());
        }
        let local = response.headers.to().unwrap_or_default();
        // Through `send_final` rather than `send`: this used to go straight
        // out, which left it out of the `answered` list that 17.2.1's absorber
        // reads, so a retransmitted INVITE was answered a second time with a
        // freshly built description and another `Answered` event -- and the
        // layer above turns every `Answered` into `Media::connect`, which
        // empties the jitter buffer and the outgoing queue.
        self.send_final(&request, from, response, true);
        if let Some(call) = self.call.as_mut() {
            call.local = local;
            call.established = true;
            call.media = Some(negotiated.clone());
        }
        self.set_call_state(state::ANSWERED);
        self.event(Event::Answered(Box::new(negotiated)));
    }

    /// A call ringing here that we have just refused on the wire: let go of it,
    /// and say so.
    ///
    /// `self.ringing` has been taken by the time either 488 above is sent, and
    /// the dialog `incoming_invite` stored beside it used to be left where it
    /// was -- so the line stayed at "ringing here" for good, and `place_call`
    /// refused every number after it because it could still see a call. The
    /// refusal is final (17.2.1 keeps the octets long enough to answer a
    /// retransmission with them), so the call goes with it.
    fn refused_here(&mut self, code: u16, reason: String) {
        self.call = None;
        self.set_call_state(state::IDLE);
        self.event(Event::Failed { code, reason });
    }

    /// Put the call down, whatever stage it is at -- and only once.
    ///
    /// Asking twice has to be free, because the layer above is level-triggered:
    /// while the modem is on hook and the call is still up it asks again on
    /// every turn of its loop, which is every 2 ms, and nothing it can see
    /// changes until this thread next runs -- up to 20 ms later. A live call
    /// ended that way put about ten BYEs on the wire, each with its own
    /// sequence number and its own branch, and each one replacing the
    /// transaction before it: the trunk answered 481 to nine of them, and the
    /// one answer that meant anything arrived for a transaction that had
    /// already been thrown away. Which of these is already on its way out is
    /// something only this thread knows, so this is where a second ask has to
    /// be absorbed rather than in whoever asked.
    fn end_call(&mut self) {
        // A number waiting on the leg that is being got rid of, and then the
        // line is put down instead: whoever changed their mind must not get a
        // call a few seconds later. Said in an event rather than dropped, for
        // the reason `give_up_held_dial` gives.
        self.give_up_held_dial("the line was put down before it could be placed");
        match hang_up_now(Underway {
            ringing: self.ringing.is_some(),
            established: self.call.as_ref().is_some_and(|c| c.established),
            inviting: self.invite.is_some(),
            bye: self.bye_underway(),
            cancelling: self.cancelling(),
        }) {
            // Silently. A line in the transcript every 2 ms would be a fault of
            // the same kind as the one above.
            HangUp::AlreadyUnderWay => {}
            HangUp::Decline => self.decline(),
            HangUp::Bye => self.send_bye("hung up"),
            HangUp::Cancel => self.want_cancel(),
            HangUp::NoCall => self.set_call_state(state::IDLE),
        }
    }

    /// 21.4.4: saying no to a call ringing here that nobody answered.
    ///
    /// Idempotent by construction, and that is what `self.ringing.take()` is
    /// doing: the 603 is a final response with no transaction of ours behind
    /// it, so there is no second one to suppress -- there is simply nothing
    /// left to decline. A retransmitted INVITE gets these same octets back out
    /// of 17.2.1's list.
    fn decline(&mut self) {
        let Some((request, from)) = self.ringing.take() else {
            return;
        };
        self.respond(&request, from, 603, "Decline");
        self.call = None;
        self.set_call_state(state::IDLE);
        self.event(Event::Ended {
            reason: "declined".to_owned(),
        });
    }

    /// Whether a BYE is already in flight for the call we have got.
    ///
    /// Asked about *this* call rather than as "is there a BYE anywhere", and
    /// the difference is the whole of a fault this change would otherwise have
    /// created. 15.1.1 ends a call when its BYE is handed to the transaction,
    /// so a number dialled straight afterwards has a call of its own while the
    /// transaction before it is still running -- 32 s of it, against a far end
    /// that answers INVITEs and ignores BYEs. Reading that transaction as this
    /// call's hang-up would be somebody pressing Hang up on the call they are
    /// actually on and watching nothing happen, which is where this started.
    ///
    /// It is nearly always false, and deliberately so: the call a BYE belongs
    /// to is gone by the time the BYE is running. What it still buys is the
    /// backstop -- one hang-up is one BYE -- for any path that ends a call
    /// some other way.
    fn bye_underway(&self) -> bool {
        let Some(call) = self.call.as_ref() else {
            return false;
        };
        self.bye
            .as_ref()
            .is_some_and(|p| p.request.headers.call_id() == Some(call.call_id.as_str()))
    }

    /// Whether this call is already being cancelled.
    ///
    /// Three things mean it, and all three have to be looked at. The wish on
    /// its own, when 9.1 will not let the CANCEL go yet; the CANCEL
    /// transaction, while it is running; and the flag on the INVITE, because
    /// that transaction is cleared the moment the far end answers it and the
    /// call goes on being cancelled afterwards.
    fn cancelling(&self) -> bool {
        self.cancel_wanted
            || self.cancel.is_some()
            || self.invite.as_ref().is_some_and(|p| p.cancelled)
    }

    // ---- building requests ------------------------------------------

    /// The Contact host: what we tell the far end to send to. A registrar that
    /// has seen us through a router knows our public address and told us in
    /// the Via; until then, our own.
    fn contact_host(&self) -> String {
        match self.public {
            Some(address) => address.ip().to_string(),
            None => self.local.ip().to_string(),
        }
    }

    /// Where the far end is told to send in-dialog requests (8.1.1.8).
    ///
    /// 18.1.1 and 19.1.1: the URI carries `;transport=tcp` when that is what
    /// we are on, so that the far end reaches us the same way it was reached.
    /// Without it a trunk reads the default from 19.1.2, which is UDP, and
    /// sends a re-INVITE or a BYE as a datagram to a port nothing is listening
    /// on. Nothing is added for UDP: it is the default, and a parameter
    /// saying so is one more string for a far end to compare wrongly.
    fn contact(&self) -> Address {
        let live = self.link.local();
        let port = self.public.map_or(live.port(), |a| a.port());
        let mut uri = Uri::user_at(&self.account.username, &self.contact_host()).with_port(port);
        if self.link.reliable() {
            uri = uri.with_parameter("transport", Some("tcp"));
        }
        Address::new(uri)
    }

    /// A Via for a request we are sending, with rport asked for.
    ///
    /// 18.1.1: the sent-protocol names the transport the request went out
    /// over, and a far end reads it to know how to answer. Getting this wrong
    /// is not cosmetic -- a proxy told UDP by a request that arrived over a
    /// connection will answer to the address in the sent-by, which over TCP is
    /// a port nothing is listening on.
    ///
    /// RFC 3581 1: without rport, a far end sends its response to the address
    /// in the Via, which behind a router is an address that means nothing
    /// outside this machine. With it, the response comes back to the port the
    /// request came from, which is the only one a router has a mapping for. It
    /// is asked for over TCP as well, where 18.2.1 makes it redundant --
    /// responses come back down the connection whatever the Via says -- but
    /// where the `received` that comes with it is still how we learn what
    /// address the world sees us at.
    fn via(&self, branch: &str) -> String {
        let local = self.link.local();
        // Asked of the link rather than of the account: what belongs here is
        // the transport the octets are actually going out over.
        format!(
            "SIP/2.0/{} {}:{};rport;branch={branch}",
            self.link.transport(),
            local.ip(),
            local.port()
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn request(
        &self,
        method: Method,
        uri: Uri,
        call_id: &str,
        from: &Address,
        to: &Address,
        cseq: u32,
        branch: &str,
    ) -> Request {
        let mut headers = Headers::new();
        headers.push("Via", self.via(branch));
        headers.push("Max-Forwards", "70");
        headers.push("From", from.to_string());
        headers.push("To", to.to_string());
        headers.push("Call-ID", call_id.to_owned());
        headers.push("CSeq", format!("{cseq} {method}"));
        headers.push("Contact", self.contact().to_string());
        headers.push("User-Agent", user_agent());
        headers.push("Allow", "INVITE, ACK, CANCEL, BYE, OPTIONS, INFO, UPDATE");
        Request {
            method,
            uri,
            headers,
            body: Vec::new(),
        }
    }

    fn send_register(&mut self, expires: u32) {
        let registrar = Uri::host(&split_host(self.account.registrar.clone()));
        let mut local = Address::new(self.account.uri());
        if let Some(display) = &self.account.display {
            local = local.clone().with_display(display);
        }
        local.set_parameter("tag", &self.register_tag.clone());
        let to = Address::new(self.account.uri());
        self.register_cseq += 1;
        let cseq = self.register_cseq;
        let call_id = self.register_call_id.clone();
        let branch = rand::branch();
        let mut request = self.request(
            Method::Register,
            registrar,
            &call_id,
            &local,
            &to,
            cseq,
            &branch,
        );
        request.headers.set("Expires", expires.to_string());
        let hop = self.hop;
        self.send(&Message::Request(request.clone()), hop);
        self.registration = Some(Pending::new(request, hop, false));
        if expires == 0 {
            self.note("sip: unregistering".to_owned());
        }
    }

    fn send_bye(&mut self, why: &str) {
        // One BYE at a time. 17.1.2.2 gives it a client transaction of its own
        // and that transaction is what sends it again if it needs sending
        // again; building a second one here is not a retransmission but a
        // second hang-up, with the next sequence number and a new branch, and
        // it throws away the transaction whose answer was on its way. The
        // guard is on a BYE being *in flight* rather than on one ever having
        // been sent, so a transaction that has given up can still be followed
        // by another attempt. `end_call` decides this too, and this is said
        // twice on purpose: `shut_down`, `invite_answered` and the answer that
        // goes unacknowledged in `service_timers` all reach here without
        // passing through it.
        //
        // About *this* call, for the reason `bye_underway` gives: a BYE still
        // running for the call before this one is not this one's hang-up, and
        // treating it as one leaves a person unable to put down the call they
        // are on. A new BYE does replace that older transaction -- there is
        // one slot -- so the older one stops being sent again. What is given
        // up there is small and the alternative is not: by the time a second
        // call has been placed and answered, the first BYE has gone out three
        // times on 17.1.2.2's schedule, and a far end that missed all three
        // has its own session timer.
        if self.bye_underway() {
            return;
        }
        // Whatever 13.3.1.4 was still trying to get through, it is moot now:
        // the call is being ended, and a 2xx retransmitted at a far end that
        // is about to be told the call is over says nothing useful.
        self.answer = None;
        let Some(call) = self.call.as_mut() else { return };
        call.cseq += 1;
        let (cseq, call_id) = (call.cseq, call.call_id.clone());
        let (local, remote, target, routes, hop) = (
            call.local.clone(),
            call.remote.clone(),
            call.target.clone(),
            call.routes.clone(),
            call.hop,
        );
        let branch = rand::branch();
        let mut request = self.request(
            Method::Bye,
            target,
            &call_id,
            &local,
            &remote,
            cseq,
            &branch,
        );
        for route in routes.iter().rev() {
            request.headers.push_front("Route", route.clone());
        }
        self.send(&Message::Request(request.clone()), hop);
        self.bye = Some(Pending::new(request, hop, false));
        self.note(format!("sip: BYE ({why})"));
        // 15.1.1: the session is over the moment the BYE is passed to the
        // client transaction. Not when the far end answers it -- the RFC is
        // explicit that the UAC stops sending and listening for media at the
        // point the request is handed over, because the answer says only that
        // the BYE arrived, and it may never come.
        //
        // So the call ends here, and what is left is one transaction with one
        // job: getting the BYE through (17.1.2.2 sends it again until it is
        // answered, and Timer F gives up at 32 s). It used to be left standing
        // instead, with the line at "hanging up" and `self.call` still set, so
        // `place_call` refused every number for as long as the answer took --
        // for ever, in effect, when what ended the call was the far end going
        // away. That is the fault: a person whose call had dropped could not
        // redial, and the window sat showing a call that was over.
        self.finish_call(why);
    }

    /// Whether there is a call on its way out that cannot simply be let go of.
    ///
    /// Exactly one thing qualifies: an INVITE of ours that is being cancelled.
    /// 9.1's race is the reason -- the far end may have committed to a 200
    /// before the CANCEL reached it, and then the dialog is real and has to be
    /// acknowledged and ended, or its half of the call stays up and goes on
    /// being billed. Throwing that leg away to make room for the next number
    /// would be the fault the CANCEL machinery exists to prevent, and the 487
    /// it is waiting for has to be acknowledged too (17.1.1.3).
    ///
    /// A BYE is deliberately not here. 15.1.1 ends that call the moment the
    /// BYE goes to its transaction, so there is nothing left of it to wait for.
    fn still_getting_rid_of_a_call(&self) -> bool {
        self.invite.is_some() && self.cancelling()
    }

    /// Keep a number that was dialled while the call before it was still being
    /// cancelled, and say so.
    ///
    /// Kept rather than refused, because the person has hung up and dialled
    /// again and the only thing in the way is a leg this end is not allowed to
    /// abandon. It is spent in `service_timers` the moment that leg settles,
    /// which is a round trip away, and given up on in words if it does not --
    /// see [`HOLD_A_DIAL`] for why the waiting has to end.
    fn hold_a_dial(&mut self, dialled: &str) {
        // The last number typed is the one wanted; an earlier one that is
        // still waiting was superseded by this and never went anywhere, so it
        // has to be answered for or the modem above waits on it for ever.
        self.give_up_held_dial("another number was dialled after it");
        self.held_dial = Some((dialled.to_owned(), Instant::now() + HOLD_A_DIAL));
        self.note(format!(
            "sip: {dialled} is waiting for the call before it to finish being \
             cancelled (9.1)"
        ));
    }

    /// Let go of a held number, saying why nothing came of it.
    ///
    /// Through [`Event::Failed`] with a code of 0, like every other dial that
    /// does not become a call attempt: over SIP the modem is stepped by
    /// arriving RTP and by nothing else, so a number dropped in silence leaves
    /// it off hook waiting for a carrier on a call nobody ever placed.
    fn give_up_held_dial(&mut self, why: &str) {
        let Some((dialled, _)) = self.held_dial.take() else {
            return;
        };
        self.event(Event::Failed {
            code: 0,
            reason: format!("{dialled} was not dialled: {why}"),
        });
    }

    /// Put the call down while it is still ringing, as soon as 9.1 allows.
    ///
    /// 9.1: a client MUST NOT send a CANCEL until a provisional response has
    /// arrived for the request it cancels. The old code sent one immediately,
    /// and `ATD` followed by `ATH` inside the 750 ms this path carries did
    /// exactly what the rule is there to prevent: the CANCEL reached the proxy
    /// before any server transaction existed to match it, the proxy answered
    /// 481, our INVITE went on being retransmitted, the trunk rang the number,
    /// somebody answered it, and the 200 brought up a call the user had hung
    /// up on -- and was billed for it. So the wish is remembered instead, and
    /// spent the moment the far end says anything at all.
    fn want_cancel(&mut self) {
        // Asked again while the first one is still being seen through. The
        // wish is already recorded or the CANCEL is already out, so there is
        // nothing to do but wait for the far end -- and doing it again would
        // put the same line in the transcript on every turn of the caller's
        // loop.
        if self.cancelling() {
            return;
        }
        let now = Instant::now();
        let Some(pending) = self.invite.as_mut() else { return };
        let ready = pending.answered_provisionally;
        // Either way the line must not sit at "cancelling" for the two
        // minutes an unanswered INVITE is given. A cancelled call is settled
        // inside a transaction's lifetime or it is not going to be, and until
        // it is settled `place_call` will not take another number.
        pending.deadline = pending.deadline.min(now + TIMER_B);
        if ready {
            self.send_cancel();
            return;
        }
        self.cancel_wanted = true;
        self.set_call_state(state::CANCELLING);
        self.note(
            "sip: hung up before the far end had said anything; \
             the CANCEL waits for a provisional response (9.1)"
                .to_owned(),
        );
    }

    /// 9.1: a CANCEL is a new transaction that copies the INVITE's branch, so
    /// that the far end knows which request it is cancelling.
    fn send_cancel(&mut self) {
        let Some(pending) = self.invite.as_mut() else { return };
        if pending.cancelled {
            return;
        }
        pending.cancelled = true;
        let invite = pending.request.clone();
        let to = pending.to;
        let mut headers = Headers::new();
        headers.push("Via", invite.headers.get("Via").unwrap_or_default().to_owned());
        headers.push("Max-Forwards", "70");
        headers.push("From", invite.headers.get("From").unwrap_or_default().to_owned());
        headers.push("To", invite.headers.get("To").unwrap_or_default().to_owned());
        headers.push("Call-ID", invite.headers.call_id().unwrap_or_default().to_owned());
        let cseq = invite.headers.cseq().map_or(1, |(n, _)| n);
        headers.push("CSeq", format!("{cseq} CANCEL"));
        headers.push("User-Agent", user_agent());
        let request = Request {
            method: Method::Cancel,
            uri: invite.uri.clone(),
            headers,
            body: Vec::new(),
        };
        self.cancel_wanted = false;
        self.send(&Message::Request(request.clone()), to);
        // 17.1.2.2: a CANCEL is an ordinary non-INVITE request and retransmits
        // on T1 doubling to T2 until it is answered. It used to be sent once
        // and forgotten, so one lost datagram was indistinguishable from never
        // having hung up: the trunk rang on, and whoever answered started a
        // call that was already over at this end.
        self.cancel = Some(Pending::new(request, to, false));
        self.set_call_state(state::CANCELLING);
        self.note("sip: CANCEL".to_owned());
    }

    /// 13.2.2.4: the ACK for a 2xx is its own transaction, addressed to the
    /// dialog's remote target rather than to wherever the INVITE went, and it
    /// is never retransmitted by the transaction layer -- if the far end
    /// resends its 200, we resend the ACK.
    fn send_ack(&mut self, response: &Response) {
        let Some(call) = self.call.as_ref() else { return };
        let (cseq, _) = response.headers.cseq().unwrap_or((call.cseq, Method::Invite));
        let mut headers = Headers::new();
        headers.push("Via", self.via(&rand::branch()));
        headers.push("Max-Forwards", "70");
        headers.push("From", call.local.to_string());
        headers.push("To", call.remote.to_string());
        headers.push("Call-ID", call.call_id.clone());
        headers.push("CSeq", format!("{cseq} ACK"));
        headers.push("Contact", self.contact().to_string());
        headers.push("User-Agent", user_agent());
        let mut request = Request {
            method: Method::Ack,
            uri: call.target.clone(),
            headers,
            body: Vec::new(),
        };
        for route in call.routes.iter().rev() {
            request.headers.push_front("Route", route.clone());
        }
        let hop = call.hop;
        self.send(&Message::Request(request), hop);
    }

    /// 17.1.1.3: the ACK for a failure response goes in the same transaction,
    /// with the same branch and the original request URI, and it is the
    /// transaction's job rather than the dialog's -- there may be no dialog.
    fn ack_failure(&mut self, response: &Response) {
        let Some(pending) = self.invite.as_ref() else { return };
        let invite = pending.request.clone();
        let to = pending.to;
        let mut headers = Headers::new();
        headers.push("Via", invite.headers.get("Via").unwrap_or_default().to_owned());
        headers.push("Max-Forwards", "70");
        headers.push("From", invite.headers.get("From").unwrap_or_default().to_owned());
        // Their tag, off the response: this ACK has to match the response and
        // not the request.
        headers.push(
            "To",
            response
                .headers
                .get("To")
                .unwrap_or(invite.headers.get("To").unwrap_or_default())
                .to_owned(),
        );
        headers.push("Call-ID", invite.headers.call_id().unwrap_or_default().to_owned());
        let cseq = invite.headers.cseq().map_or(1, |(n, _)| n);
        headers.push("CSeq", format!("{cseq} ACK"));
        headers.push("User-Agent", user_agent());
        let request = Request {
            method: Method::Ack,
            uri: invite.uri.clone(),
            headers,
            body: Vec::new(),
        };
        self.send(&Message::Request(request), to);
    }

    // ---- what arrives ------------------------------------------------

    fn handle(&mut self, message: Message, from: SocketAddr) {
        match message {
            Message::Response(response) => self.handle_response(response, from),
            Message::Request(request) => self.handle_request(request, from),
        }
    }

    /// Which transaction, if any, a response answers.
    ///
    /// Nothing did this before: a response was dispatched on the method in its
    /// CSeq and nothing else, so anything of the right method was acted on
    /// whatever call it belonged to. Three things went wrong because of it,
    /// all of them on the wire rather than in theory. A duplicate 486 from the
    /// call before -- and this path retransmits, so duplicates are the norm --
    /// tore down the *next* call, placed a second later. A duplicate 401 for a
    /// REGISTER we had already answered went down the "challenged twice" path,
    /// said the password was wrong when it was not, and cleared
    /// `self.registration`, destroying the authorised REGISTER then in flight.
    /// And a BYE or a CANCEL was honoured for any Call-ID at all.
    fn transaction_for(&self, response: &Response) -> Option<Transaction> {
        let matches = |slot: &Option<Pending>| {
            slot.as_ref()
                .is_some_and(|p| answers(&p.request, response))
        };
        if matches(&self.registration) {
            Some(Transaction::Register)
        } else if matches(&self.invite) {
            Some(Transaction::Invite)
        } else if matches(&self.cancel) {
            Some(Transaction::Cancel)
        } else if matches(&self.bye) {
            Some(Transaction::Bye)
        } else {
            None
        }
    }

    fn handle_response(&mut self, response: Response, _from: SocketAddr) {
        let Some(kind) = self.transaction_for(&response) else {
            // Not ours, or ours and already finished with. The one thing a
            // response to no transaction can still mean is 13.3.1.4's
            // retransmitted 2xx, which is handled where the rest of the INVITE
            // handling is.
            if response.headers.cseq().is_some_and(|(_, m)| m == Method::Invite) {
                self.stray_invite_response(&response);
            }
            return;
        };
        // RFC 3581 4: the registrar tells us what address it saw. Believing it
        // is what makes a Contact work from behind a router -- and believing
        // it only from a response to a transaction of ours is what keeps a
        // stray datagram from moving our Contact somewhere it is not.
        self.learn_public_address(&response);

        match kind {
            Transaction::Register => self.registration_answered(response),
            Transaction::Invite => self.invite_answered(response),
            Transaction::Cancel => {
                // 9.1: whatever the answer is, there is nothing further to do
                // with the CANCEL itself. The 487 for the INVITE is what ends
                // the call, and a 481 here means the far end had already
                // finished with the INVITE anyway.
                if !response.is_provisional() {
                    self.cancel = None;
                }
            }
            Transaction::Bye => {
                // 200 says the far end got it; 481 says it had already thrown
                // the dialog away, which is the same news. Either ends the
                // transaction and nothing else: 15.1.1 ended the call here
                // when the BYE was handed over, and a number dialled in the
                // meantime has a call of its own that this answer has nothing
                // to do with. Ending "the call" here used to be harmless only
                // because there could not be another one yet.
                if response.is_success() || response.is_failure() {
                    self.bye = None;
                }
            }
        }
    }

    /// A response to an INVITE that answers no transaction we are running.
    ///
    /// 13.3.1.4: a UAS retransmits its 2xx every T1 until the ACK arrives, and
    /// on this path -- about 750 ms each way -- it always gets at least one
    /// retransmission out before our ACK can reach it, so this fires on
    /// essentially every call. Every 2xx used to be handled identically:
    /// the dialog was adopted again, the answer read again, and another
    /// `Answered` went up, which the layer above turns into `Media::connect`
    /// -- flushing the jitter buffer and the outgoing queue, and with them
    /// half a second of the modem's V.8 handshake. Re-ACKing is required
    /// (13.2.2.4); doing anything else is not.
    fn stray_invite_response(&mut self, response: &Response) {
        if !response.is_success() {
            return;
        }
        let theirs = response.headers.to().and_then(|t| t.tag().map(str::to_owned));
        let ours = self.call.as_ref().is_some_and(|call| {
            call.established
                && Some(call.call_id.as_str()) == response.headers.call_id()
                && response.headers.cseq().map(|(n, _)| n) == Some(call.invite_cseq)
                && tags_agree(call.remote.tag(), theirs.as_deref())
        });
        if ours {
            self.send_ack(response);
        }
    }

    fn learn_public_address(&mut self, response: &Response) {
        let Some(via) = response.headers.get("Via") else {
            return;
        };
        let (Some(received), Some(rport)) = (
            message::via_parameter(via, "received"),
            message::via_parameter(via, "rport"),
        ) else {
            return;
        };
        let Ok(port) = rport.parse::<u16>() else { return };
        let Ok(ip) = received.parse::<std::net::IpAddr>() else {
            return;
        };
        let seen = SocketAddr::new(ip, port);
        if self.public != Some(seen) {
            self.note(format!("sip: the network sees us at {seen}"));
            self.public = Some(seen);
        }
    }

    fn registration_answered(&mut self, response: Response) {
        if response.is_provisional() {
            return;
        }
        if let Some(challenge) = self.challenge_in(&response) {
            let retried = self.retry_with_credentials(Kind::Register, &challenge);
            if retried {
                return;
            }
        }
        self.registration = None;
        if response.is_success() {
            // 10.2.4: the registrar decides the expiry, whatever we asked for.
            let expires = expires_of(&response).unwrap_or(self.account.expires);
            if expires == 0 {
                self.registered_until = None;
                self.register_at = None;
                self.note("sip: unregistered".to_owned());
                return;
            }
            self.registered_until = Some(Instant::now() + Duration::from_secs(u64::from(expires)));
            // Well before it lapses. A registration that expires between one
            // refresh and the next is a telephone that cannot be called and
            // does not know it.
            let refresh = expires.saturating_mul(9) / 10;
            self.register_at = Some(Instant::now() + Duration::from_secs(u64::from(refresh.max(10))));
            self.event(Event::Registered { expires });
        } else {
            self.registered_until = None;
            // Not immediately: a registrar that said no will say no again,
            // and a tight loop against a provider is how an account gets
            // blocked.
            self.register_at = Some(Instant::now() + Duration::from_secs(30));
            self.event(Event::RegistrationFailed {
                code: response.code,
                reason: response.reason.clone(),
            });
        }
    }

    fn invite_answered(&mut self, response: Response) {
        if response.is_provisional() {
            if let Some(pending) = self.invite.as_mut() {
                pending.answered_provisionally = true;
            }
            // A far end's tag arrives on the provisional response, and with it
            // the early dialog. Kept, because a CANCEL and the eventual 200
            // both have to agree with it.
            if let Some(to) = response.headers.to()
                && to.tag().is_some()
                && let Some(call) = self.call.as_mut()
            {
                call.remote = to;
            }
            match response.code {
                180 => {
                    self.set_call_state(state::RINGING);
                    self.event(Event::Ringing);
                }
                183 => {
                    self.set_call_state(state::RINGING);
                    if let Some(negotiated) = self.read_answer(&response) {
                        self.event(Event::EarlyMedia(Box::new(negotiated)));
                    }
                }
                _ => {}
            }
            // 9.1's wait is over: the far end has said something, so there is
            // a server transaction at the other end for a CANCEL to match.
            // Last, after the state has been set above, or a 180 arriving
            // after the user hung up would put the line back to "ringing".
            if self.cancel_wanted {
                self.send_cancel();
            }
            return;
        }

        // Whether the failure below has been acknowledged already, on the way
        // past the challenge. It used to be possible to fall out of the
        // challenge arm without returning -- when `retry_with_credentials`
        // said no -- and then run `ack_failure` a second time at the bottom,
        // so one 401 got two ACKs.
        let mut acknowledged = false;
        if let Some(challenge) = self.challenge_in(&response) {
            // 22.2: the failure response is acknowledged before the request
            // is sent again. Skipping this leaves the far end retransmitting
            // its 401 for half a minute.
            self.ack_failure(&response);
            acknowledged = true;
            if self.retry_with_credentials(Kind::Invite, &challenge) {
                return;
            }
        }

        if response.is_success() {
            // Was this call hung up while it was still ringing? Then the far
            // end answered anyway -- it had already committed to the 200 when
            // our CANCEL reached it, which 9.1 says is exactly what happens in
            // the race it warns about. The dialog is real and has to be
            // acknowledged and then ended properly, or the far end's call
            // stays up and is billed.
            let cancelled = self.cancelling();
            self.invite = None;
            self.adopt_dialog(&response);
            self.send_ack(&response);
            if cancelled {
                if let Some(call) = self.call.as_mut() {
                    call.established = true;
                }
                self.cancel = None;
                self.cancel_wanted = false;
                self.send_bye("hung up while it was still ringing");
                return;
            }
            match self.read_answer(&response) {
                Some(negotiated) => {
                    if let Some(call) = self.call.as_mut() {
                        call.established = true;
                        call.media = Some(negotiated.clone());
                    }
                    self.set_call_state(state::UP);
                    self.event(Event::Answered(Box::new(negotiated)));
                }
                None => {
                    // Answered with a description we cannot use. Ending it is
                    // the only honest thing: a call that is up and carrying a
                    // codec a modem cannot read is worse than no call, because
                    // everything above will spend a minute failing to explain
                    // it.
                    if let Some(call) = self.call.as_mut() {
                        call.established = true;
                    }
                    self.send_bye("the answer offered nothing we can carry");
                }
            }
            return;
        }

        // Anything else is the end of it.
        if !acknowledged {
            self.ack_failure(&response);
        }
        self.invite = None;
        self.cancel = None;
        self.cancel_wanted = false;
        let code = response.code;
        let reason = describe(code, &response.reason);
        self.call = None;
        self.set_call_state(state::IDLE);
        self.event(Event::Failed { code, reason });
    }

    /// 12.1.2: what a 2xx to an INVITE tells us about where to send the rest.
    fn adopt_dialog(&mut self, response: &Response) {
        let contact = response.headers.contact();
        let routes: Vec<String> = response
            .headers
            .all("Record-Route")
            .map(str::to_owned)
            .collect();
        let to = response.headers.to();
        let cseq = response.headers.cseq().map(|(n, _)| n);
        let Some(call) = self.call.as_mut() else { return };
        if let Some(to) = to {
            call.remote = to;
        }
        if let Some(contact) = contact {
            call.target = contact.uri;
        }
        // 13.3.1.4's retransmitted 200 arrives carrying this, long after
        // `cseq` has moved on to the BYE, so it is what a stray 2xx is
        // recognised by.
        if let Some(cseq) = cseq {
            call.invite_cseq = cseq;
        }
        // Reversed: the caller's route set is the Record-Route headers in
        // reverse order, and getting this backwards sends every in-dialog
        // request out through the far end's edge proxy first.
        call.routes = routes.into_iter().rev().collect();
        // Where the datagrams go is the first route if there is one, and the
        // target otherwise. A trunk that record-routes and a trunk that does
        // not both work; one that record-routes and is ignored does not.
        let first = call
            .routes
            .first()
            .and_then(|r| Address::parse(r))
            .map(|a| a.uri.socket_address())
            .unwrap_or_else(|| call.target.socket_address());
        if let Ok(hop) = resolve(&first) {
            call.hop = hop;
        }
    }

    fn read_answer(&mut self, response: &Response) -> Option<Negotiated> {
        if response.body.is_empty() {
            return None;
        }
        let text = String::from_utf8_lossy(&response.body);
        match sdp::Sdp::parse(&text) {
            Ok(answer) => match sdp::read_answer(&answer, &self.account.laws) {
                Ok(negotiated) => Some(negotiated),
                Err(e) => {
                    self.note(format!("sip: the answer will not do: {e}"));
                    None
                }
            },
            Err(e) => {
                self.note(format!("sip: the answer would not parse: {e}"));
                None
            }
        }
    }

    fn handle_request(&mut self, request: Request, from: SocketAddr) {
        // 17.2.1: a request we have already answered gets the same answer
        // again rather than being processed twice. Over UDP on a slow path
        // this is not an edge case; it happens on most calls.
        //
        // Provisional responses are looked at as well as final ones. They are
        // not final, so they are kept apart, but the rule is the same and the
        // omission was expensive: a request still ringing here had only a 100
        // and a 180 sent for it, neither of which was remembered, so a
        // retransmission of it went through to the dialog handling as though
        // it were something new.
        if let Some(branch) = request.headers.branch() {
            let key = format!("{branch}/{}", request.method);
            let kept = self
                .answered
                .iter()
                .chain(self.provisional.iter())
                .find(|(k, ..)| *k == key);
            if let Some((_, bytes, to, _)) = kept {
                let (bytes, to) = (bytes.clone(), *to);
                // The same octets as before, whatever the transport: 17.2.1 is
                // about answering a request the same way twice, and a request
                // can arrive twice over TCP as well -- after a reconnection, or
                // from a far end that decided it had waited long enough.
                let _ = self.link.send(&bytes, to);
                return;
            }
        }
        match request.method {
            Method::Invite => self.incoming_invite(request, from),
            Method::Ack => {
                // The far end has acknowledged our 200, so 13.3.1.4's
                // retransmission of it stops here. The call itself was up from
                // the moment the 200 went out.
                self.answer_acknowledged(&request);
            }
            Method::Bye => {
                // 12.2.2: a BYE names a dialog, and one naming a dialog we do
                // not have is 481. It used to be honoured for any Call-ID at
                // all, so a duplicate BYE from the call before -- and this
                // path duplicates everything -- ended the call placed since.
                if self.in_our_dialog(&request) {
                    self.respond(&request, from, 200, "OK");
                    self.finish_call("the far end hung up");
                } else {
                    self.respond(&request, from, 481, "Call/Transaction Does Not Exist");
                }
            }
            Method::Cancel => {
                // 9.2: a CANCEL matches a server transaction that has not been
                // given a final response yet, and there is nothing else it can
                // mean. A CANCEL arriving after our 200 had gone used to find
                // `ringing` empty, fall through to `finish_call` and tear the
                // call down here alone -- no BYE, so the far end's half of it
                // stayed up and went on being billed.
                let ours = self.in_our_dialog(&request) && self.ringing.is_some();
                if !ours {
                    self.respond(&request, from, 481, "Call/Transaction Does Not Exist");
                    return;
                }
                self.respond(&request, from, 200, "OK");
                if let Some((invite, invite_from)) = self.ringing.take() {
                    self.respond(&invite, invite_from, 487, "Request Terminated");
                }
                self.finish_call("the caller gave up");
            }
            Method::Options => {
                // 11.2: an OPTIONS is how a trunk checks we are alive, and
                // several send one every thirty seconds. Answering it is what
                // keeps the registration usable.
                self.respond(&request, from, 200, "OK");
            }
            Method::Info | Method::Update => self.respond(&request, from, 200, "OK"),
            _ => self.respond(&request, from, 405, "Method Not Allowed"),
        }
    }

    fn incoming_invite(&mut self, request: Request, from: SocketAddr) {
        // 12.2.2: a request is inside a dialog when the Call-ID *and both
        // tags* agree, and an INVITE is only a re-INVITE if it is inside an
        // established one. This used to be decided on the Call-ID alone, with
        // the dialog stored before anybody had answered, and the consequence
        // was as bad as it sounds: a caller's ordinary INVITE retransmission
        // reached `re_invite`, which built an SDP answer and sent 200 OK.
        // The call was answered by nobody, with no tag of ours on the To, and
        // `ringing` still set as though it were still ringing.
        let same_call = self
            .call
            .as_ref()
            .is_some_and(|c| Some(c.call_id.as_str()) == request.headers.call_id());
        if same_call {
            let theirs = request.headers.to().and_then(|t| t.tag().map(str::to_owned));
            let established = self.call.as_ref().is_some_and(|c| c.established);
            let ours = self
                .call
                .as_ref()
                .and_then(|c| c.local.tag().map(str::to_owned));
            if established && theirs.is_some() && theirs == ours {
                return self.re_invite(request, from);
            }
            if self.ringing.is_some() {
                // The initial INVITE over again on a branch we have no answer
                // filed under -- a far end that retransmits with a new branch
                // rather than the same one. Still ringing, so say so again and
                // nothing else (17.2.1's spirit, if not its letter).
                self.respond(&request, from, 180, "Ringing");
                return;
            }
            // Our Call-ID, but addressed to a tag that is not ours: whatever
            // dialog it means, it is not this one.
            self.respond(&request, from, 481, "Call/Transaction Does Not Exist");
            return;
        }
        if self.call.is_some() || self.ringing.is_some() {
            // 21.4.7: one line, and it is busy.
            self.respond(&request, from, 486, "Busy Here");
            return;
        }
        let description = String::from_utf8_lossy(&request.body).into_owned();
        if let Ok(offer) = sdp::Sdp::parse(&description)
            && sdp::wants_t38(&offer)
        {
            // A fax machine calling in. Worth a clear refusal rather than a
            // call that comes up and carries nothing: T.38 is the next thing
            // to build here and is not built yet.
            self.respond(&request, from, 488, "Not Acceptable Here (no T.38 yet)");
            return;
        }
        self.respond(&request, from, 100, "Trying");
        self.respond(&request, from, 180, "Ringing");
        let caller = request
            .headers
            .from()
            .map(|a| a.uri.user.unwrap_or(a.uri.host))
            .unwrap_or_else(|| "someone".to_owned());
        let call_id = request.headers.call_id().unwrap_or_default().to_owned();
        let local = request.headers.to().unwrap_or_default();
        let remote = request.headers.from().unwrap_or_default();
        let target = request
            .headers
            .contact()
            .map(|c| c.uri)
            .unwrap_or_else(|| remote.uri.clone());
        let routes: Vec<String> = request
            .headers
            .all("Record-Route")
            .map(str::to_owned)
            .collect();
        let invite_cseq = request.headers.cseq().map_or(1, |(n, _)| n);
        self.call = Some(Dialog {
            call_id,
            local,
            remote,
            cseq: 1,
            invite_cseq,
            target,
            // A callee's route set is the Record-Route headers in the order
            // they arrived (12.1.1), which is the opposite of a caller's.
            routes,
            hop: from,
            established: false,
            media: None,
        });
        self.ringing = Some((request, from));
        self.set_call_state(state::RINGING_HERE);
        self.event(Event::Incoming { from: caller });
    }

    /// A second INVITE inside a dialog: hold, a codec change, or a fax
    /// machine asking to switch to T.38.
    fn re_invite(&mut self, request: Request, from: SocketAddr) {
        // 12.2.2: a re-INVITE is a target refresh, and the target is where
        // everything else in this dialog goes. Ignoring it meant that a far
        // end which moves mid-call -- a proxy handing the call to another
        // media server is the usual way -- got our BYE at the address it had
        // left, and its half of the call stayed up and went on being billed.
        if let Some(contact) = request.headers.contact() {
            let routed = self.call.as_ref().is_some_and(|c| !c.routes.is_empty());
            let target = contact.uri;
            // Only when there is no route set: with one, the first route is
            // where the datagrams go whatever the target says (12.2.1.1).
            let hop = (!routed).then(|| resolve(&target.socket_address())).and_then(Result::ok);
            if let Some(call) = self.call.as_mut() {
                call.target = target;
                if let Some(hop) = hop {
                    call.hop = hop;
                }
            }
        }
        let text = String::from_utf8_lossy(&request.body).into_owned();
        let offer = match sdp::Sdp::parse(&text) {
            Ok(offer) => offer,
            Err(e) => {
                self.respond(&request, from, 488, "Not Acceptable Here");
                self.note(format!("sip: a re-INVITE made no sense: {e}"));
                return;
            }
        };
        if sdp::wants_t38(&offer) {
            // The far end has detected a fax tone and wants to switch the
            // call to T.38. When that is built, this is where it happens: the
            // answer describes a UDPTL stream and the media side changes from
            // RTP audio to T.38 packets mid-call. Until then, refusing is the
            // correct answer and leaves the call on G.711, where our own fax
            // modulations still work.
            self.respond(&request, from, 488, "Not Acceptable Here (no T.38 yet)");
            self.note("sip: the far end asked for T.38; declined, staying on G.711".to_owned());
            return;
        }
        match sdp::answer(
            &offer,
            &self.contact_host(),
            self.rtp_port,
            &self.account.laws,
            self.account.ptime_ms,
        ) {
            Ok((body, negotiated)) => {
                let mut response = self.response_to(&request, 200, "OK");
                response.headers.set("Contact", self.contact().to_string());
                response
                    .headers
                    .set("Content-Type", sdp::CONTENT_TYPE.to_owned());
                response.body = body.into_bytes();
                // Through `send_final`, for the reason `accept_call` says and
                // for one more that only shows up here: RFC 4566 5.2's origin
                // id is fresh every time a description is built, so a second
                // answer to the same re-INVITE is a *different* answer, and
                // the far end is left choosing between two descriptions that
                // disagree. And every one of them raised another `Answered`,
                // which restarts the jitter buffer in the middle of a call
                // nothing was wrong with -- on this rig, exactly the twenty
                // milliseconds that costs a V.34 receiver its training.
                //
                // False: 13.3.1.4's "BYE if it is never acknowledged" is about
                // the 2xx that makes a dialog. Ending a call that is already
                // up over one unacknowledged renegotiation would be a worse
                // fault than the one this fixes.
                self.send_final(&request, from, response, false);
                if let Some(call) = self.call.as_mut() {
                    call.media = Some(negotiated.clone());
                }
                self.note("sip: the call was renegotiated".to_owned());
                self.event(Event::Answered(Box::new(negotiated)));
            }
            Err(e) => {
                self.respond(&request, from, 488, "Not Acceptable Here");
                self.note(format!("sip: a re-INVITE offered nothing usable: {e}"));
            }
        }
    }

    // ---- authentication ----------------------------------------------

    fn challenge_in(&self, response: &Response) -> Option<Challenge> {
        match response.code {
            401 => response
                .headers
                .get("WWW-Authenticate")
                .and_then(|v| Challenge::parse(v, false)),
            407 => response
                .headers
                .get("Proxy-Authenticate")
                .and_then(|v| Challenge::parse(v, true)),
            _ => None,
        }
    }

    /// Send the request again with credentials on it. False when there is
    /// nothing to retry, or when we already tried.
    fn retry_with_credentials(&mut self, kind: Kind, challenge: &Challenge) -> bool {
        let slot = match kind {
            Kind::Register => self.registration.as_ref(),
            Kind::Invite => self.invite.as_ref(),
        };
        let Some(pending) = slot else { return false };
        if !may_answer_again(pending.attempts, &pending.nonce, challenge) {
            // Which of the two it is matters to whoever reads the log: one is
            // a setting to change and the other is a registrar to complain
            // about, and saying the first when it is the second sends a person
            // to check a password that was right all along.
            let why = if challenge.stale {
                "its nonce is stale again and again"
            } else {
                "the password or the realm is wrong"
            };
            self.note(format!(
                "sip: {} was challenged again and will not be answered again; {why}",
                pending.request.method
            ));
            return false;
        }
        if !challenge.supported() {
            self.note(format!(
                "sip: cannot answer a {} challenge",
                challenge.algorithm.as_deref().unwrap_or(&challenge.scheme)
            ));
            return false;
        }
        let mut request = pending.request.clone();
        let to = pending.to;
        let attempts = pending.attempts + 1;
        let uri = request.uri.to_string();
        let method = request.method.to_string();
        let credentials = crate::auth::respond(
            challenge,
            self.account.auth_name(),
            &self.account.password,
            &method,
            &uri,
            1,
        );
        request.headers.set(challenge.header_name(), credentials);
        // 8.1.3.5: a new branch and the next sequence number, because this is
        // a new transaction rather than a retransmission of the old one.
        let (cseq, _) = request.headers.cseq().unwrap_or((1, request.method.clone()));
        let cseq = cseq + 1;
        request.headers.set("CSeq", format!("{cseq} {method}"));
        request.headers.set("Via", self.via(&rand::branch()));
        match kind {
            Kind::Register => {
                self.register_cseq = cseq;
                self.send(&Message::Request(request.clone()), to);
                let mut pending = Pending::new(request, to, false);
                pending.attempts = attempts;
                pending.nonce = challenge.nonce.clone();
                self.registration = Some(pending);
            }
            Kind::Invite => {
                if let Some(call) = self.call.as_mut() {
                    call.cseq = cseq;
                    call.invite_cseq = cseq;
                }
                self.send(&Message::Request(request.clone()), to);
                let mut pending = Pending::new(request, to, true);
                pending.attempts = attempts;
                pending.nonce = challenge.nonce.clone();
                self.invite = Some(pending);
            }
        }
        true
    }

    // ---- timers --------------------------------------------------------

    fn service_timers(&mut self) {
        let now = Instant::now();
        // 17.1.1.2 and 17.1.2.2: Timer A and Timer E do not run over a
        // reliable transport. Every retransmission below is guarded by this
        // and every deadline is not, because Timer B and Timer F are about a
        // far end that will not answer rather than about a datagram that went
        // missing. Sending a request twice down a connection is a duplicate
        // the far end has to disentangle for no reason at all -- and on a
        // proxy that answers the second one separately, a duplicate that gets
        // its own 481.
        let retransmits = !self.link.reliable();

        // A number waiting for the call before it to finish being cancelled.
        // Placed the moment that leg is gone, and given up on in words if it
        // will not go: see `hold_a_dial`.
        if let Some((number, by)) = self.held_dial.take() {
            if !self.still_getting_rid_of_a_call() {
                self.place_call(&number);
            } else if now >= by {
                self.held_dial = Some((number, by));
                self.give_up_held_dial(
                    "the call before it was still being cancelled, and the far \
                     end never settled it",
                );
            } else {
                self.held_dial = Some((number, by));
            }
        }

        if self.want_registration
            && self.registration.is_none()
            && self.register_at.is_some_and(|at| now >= at)
        {
            self.register_at = None;
            let expires = self.account.expires;
            self.send_register(expires);
        }

        // Retransmission, and giving up. Written out rather than looped over
        // because the three transactions fail in different ways and the
        // difference is the useful part.
        if let Some(pending) = self.registration.as_mut() {
            if now >= pending.deadline {
                self.registration = None;
                self.registered_until = None;
                self.register_at = Some(now + Duration::from_secs(30));
                self.event(Event::RegistrationFailed {
                    code: 408,
                    reason: "the registrar never answered".to_owned(),
                });
            } else if retransmits && now >= pending.next {
                pending.interval = (pending.interval * 2).min(T2);
                pending.next = now + pending.interval;
                let (bytes, to) = (Message::Request(pending.request.clone()).to_bytes(), pending.to);
                let _ = self.link.send(&bytes, to);
            }
        }

        if let Some(pending) = self.invite.as_mut() {
            if now >= pending.deadline {
                // A call the user hung up on ends as ended rather than as
                // failed, whichever timer got there first. `want_cancel` has
                // already shortened this deadline to a transaction's life, so
                // the line comes back within half a minute rather than sitting
                // at "cancelling" for the whole two minutes an unanswered
                // INVITE is otherwise given -- two minutes in which
                // `place_call` would refuse to dial anything.
                let cancelled = pending.cancelled || self.cancel_wanted;
                self.invite = None;
                self.cancel = None;
                self.cancel_wanted = false;
                self.call = None;
                self.set_call_state(state::IDLE);
                if cancelled {
                    self.event(Event::Ended {
                        reason: "cancelled".to_owned(),
                    });
                } else {
                    self.event(Event::Failed {
                        code: 408,
                        reason: "the far end never answered the INVITE".to_owned(),
                    });
                }
            } else if retransmits && !pending.answered_provisionally && now >= pending.next {
                // 17.1.1.2: an INVITE's interval doubles without the T2
                // ceiling, because the far end may be ringing a telephone.
                pending.interval *= 2;
                pending.next = now + pending.interval;
                let (bytes, to) = (Message::Request(pending.request.clone()).to_bytes(), pending.to);
                let _ = self.link.send(&bytes, to);
            }
        }

        // 17.1.2.2: the CANCEL is a non-INVITE client transaction of its own
        // and retransmits like one. Giving up on it changes nothing here --
        // what ends the call is the 487 for the INVITE, or that INVITE's own
        // deadline, which `want_cancel` has already brought forward.
        if let Some(pending) = self.cancel.as_mut() {
            if now >= pending.deadline {
                self.cancel = None;
                self.note("sip: the CANCEL went unanswered".to_owned());
            } else if retransmits && now >= pending.next {
                pending.interval = (pending.interval * 2).min(T2);
                pending.next = now + pending.interval;
                let (bytes, to) = (Message::Request(pending.request.clone()).to_bytes(), pending.to);
                let _ = self.link.send(&bytes, to);
            }
        }

        if let Some(pending) = self.bye.as_mut() {
            if now >= pending.deadline {
                self.bye = None;
                // 17.1.2.2's Timer F, and nothing to tidy up: the call ended
                // here when the BYE was passed to this transaction (15.1.1).
                // The far end will work it out when its own session timer
                // lapses. This used to call `finish_call`, which is how a line
                // whose BYE went unanswered stayed unusable for the whole 32 s
                // -- and, once a number could be dialled in that time, how the
                // call placed since was torn down by the transaction before it.
                self.note(
                    "sip: the BYE went unanswered; the call ended here when it \
                     was sent (15.1.1)"
                        .to_owned(),
                );
            } else if retransmits && now >= pending.next {
                pending.interval = (pending.interval * 2).min(T2);
                pending.next = now + pending.interval;
                let (bytes, to) = (Message::Request(pending.request.clone()).to_bytes(), pending.to);
                let _ = self.link.send(&bytes, to);
            }
        }

        // 13.3.1.4: our own 2xx, sent again until it is acknowledged. A 2xx is
        // the one response with no transaction underneath it -- the INVITE
        // server transaction ends the moment it goes out -- so getting it
        // through is the user agent's own job, and on a 750 ms path the first
        // attempt regularly is not enough.
        //
        // The same rule as above about which transport, and from the other
        // side of the call: over TCP the answer was delivered or the
        // connection is gone, and sending it again would only give the caller
        // a second copy of a description it has already acted on. The deadline
        // stands either way -- an answer nobody acknowledges is still a call
        // the caller never got.
        if let Some(answer) = self.answer.as_mut() {
            if now >= answer.deadline {
                let give_up = answer.bye_if_unacknowledged;
                self.answer = None;
                if give_up {
                    self.note(
                        "sip: the far end never acknowledged our answer (13.3.1.4)".to_owned(),
                    );
                    self.send_bye("the answer was never acknowledged");
                }
            } else if retransmits && now >= answer.next {
                answer.interval = (answer.interval * 2).min(T2);
                answer.next = now + answer.interval;
                let (bytes, to) = (answer.octets.clone(), answer.to);
                let _ = self.link.send(&bytes, to);
            }
        }

        self.answered.retain(|(.., at)| now.duration_since(*at) < ABSORB);
        self.provisional
            .retain(|(.., at)| now.duration_since(*at) < ABSORB);
    }

    // ---- odds and ends --------------------------------------------------

    fn finish_call(&mut self, reason: &str) {
        let had_one = self.call.take().is_some() || self.ringing.take().is_some();
        // Whatever is left over from it goes with it. A cancel still waiting
        // for a provisional response, or a 2xx still being retransmitted at
        // something that has gone, belongs to a call that is over.
        self.invite = None;
        self.cancel = None;
        self.cancel_wanted = false;
        self.answer = None;
        if had_one {
            self.set_call_state(state::IDLE);
            self.event(Event::Ended {
                reason: reason.to_owned(),
            });
        }
    }

    /// 12.2.2: whether an in-dialog request is for the call we actually have.
    ///
    /// The Call-ID and both tags, because the Call-ID alone is not an identity
    /// -- a BYE from a call that ended a second ago used to end the call
    /// placed since. A tag we do not know yet matches anything, because an
    /// early dialog has no remote tag until the far end sends one and a CANCEL
    /// for a call ringing here carries no tag of ours.
    fn in_our_dialog(&self, request: &Request) -> bool {
        let Some(call) = self.call.as_ref() else {
            return false;
        };
        if Some(call.call_id.as_str()) != request.headers.call_id() {
            return false;
        }
        let theirs = request.headers.from().and_then(|a| a.tag().map(str::to_owned));
        let ours = request.headers.to().and_then(|a| a.tag().map(str::to_owned));
        tags_agree(call.remote.tag(), theirs.as_deref())
            && tags_agree(call.local.tag(), ours.as_deref())
    }

    /// The ACK for a 2xx of ours, which stops 13.3.1.4's retransmission.
    fn answer_acknowledged(&mut self, request: &Request) {
        let Some(answer) = self.answer.as_ref() else {
            return;
        };
        let matches = Some(answer.call_id.as_str()) == request.headers.call_id()
            && request.headers.cseq().map(|(n, _)| n) == Some(answer.cseq);
        if matches {
            self.answer = None;
        }
    }

    /// 8.2.6.2: a response copies the Via, From, To, Call-ID and CSeq of the
    /// request, exactly as they arrived. Copying them rather than rebuilding
    /// them is not laziness: a proxy in the path recognises its own Via by
    /// the text of it.
    fn response_to(&self, request: &Request, code: u16, reason: &str) -> Response {
        let mut headers = Headers::new();
        for name in ["Via", "From", "To", "Call-ID", "CSeq"] {
            for value in request.headers.all(name) {
                headers.push(name, value.to_owned());
            }
        }
        // 12.1.1: a UAS copies the Record-Route headers into the response that
        // makes the dialog, in the order they arrived, because that is the
        // only place the caller can get its route set from. Without them a
        // trunk with an edge proxy in front of it takes the caller's in-dialog
        // BYE, sent straight at our Contact, and sends it into a hole -- and
        // the call stays up at the caller's end.
        if request.method == Method::Invite && (180..300).contains(&code) {
            for value in request.headers.all("Record-Route") {
                headers.push("Record-Route", value.to_owned());
            }
        }
        headers.push("User-Agent", user_agent());
        Response {
            code,
            reason: reason.to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn respond(&mut self, request: &Request, to: SocketAddr, code: u16, reason: &str) {
        let mut response = self.response_to(request, code, reason);
        if code == 200 && request.method == Method::Options {
            response
                .headers
                .push("Allow", "INVITE, ACK, CANCEL, BYE, OPTIONS, INFO, UPDATE");
            response.headers.push("Accept", sdp::CONTENT_TYPE.to_owned());
        }
        if (180..300).contains(&code) {
            response.headers.push("Contact", self.contact().to_string());
        }
        if code >= 200 {
            // A 2xx built here is a decline or an OPTIONS keep-alive rather
            // than an answer to an INVITE, so there is nothing for 13.3.1.4
            // to retransmit.
            self.send_final(request, to, response, false);
            return;
        }
        let bytes = Message::Response(response).to_bytes();
        self.put(&bytes, to, "a response");
        // Kept so a retransmission of the request gets the same provisional
        // back instead of being taken for something new (17.2.1). One entry
        // per transaction, so a 180 replaces the 100 that went before it and
        // what comes back is the last thing we said.
        self.remember(Where::Provisional, request, bytes, to);
    }

    /// Send a final response, and keep the octets so that a retransmission of
    /// the request is answered with these same ones (17.2.1).
    ///
    /// Everything used to go out through `respond`, except the two responses
    /// where it mattered most: the 200 that answers a call and the 200 that
    /// answers a re-INVITE were both built by hand and sent with `send`, which
    /// is the one path that does not file the answer anywhere. So a
    /// retransmitted INVITE was answered a second time from scratch -- with a
    /// new SDP origin id (RFC 4566 5.2), so a visibly different answer -- and
    /// the agent raised a second `Answered` for it, which the layer above
    /// turns into `Media::connect`: a flushed jitter buffer and an emptied
    /// send queue, in the middle of a call nothing was wrong with.
    fn send_final(
        &mut self,
        request: &Request,
        to: SocketAddr,
        response: Response,
        answers_a_dialog: bool,
    ) {
        let code = response.code;
        let call_id = response.headers.call_id().unwrap_or_default().to_owned();
        let cseq = response.headers.cseq().map_or(0, |(n, _)| n);
        let bytes = Message::Response(response).to_bytes();
        self.put(&bytes, to, "a response");
        // The transaction has been decided, so the provisional kept for it is
        // no longer the right thing to send back.
        self.remember(Where::Final, request, bytes.clone(), to);

        // 13.3.1.4: a 2xx to an INVITE has no transaction underneath it to
        // retransmit it, so the user agent does it, until the ACK arrives.
        if (200..300).contains(&code) && request.method == Method::Invite {
            let now = Instant::now();
            self.answer = Some(Answer {
                octets: bytes,
                to,
                call_id,
                cseq,
                next: now + T1,
                interval: T1,
                deadline: now + TIMER_B,
                bye_if_unacknowledged: answers_a_dialog,
            });
        }
    }

    /// File a response under its transaction, replacing whatever was there.
    fn remember(&mut self, which: Where, request: &Request, bytes: Vec<u8>, to: SocketAddr) {
        let Some(branch) = request.headers.branch() else {
            return;
        };
        let key = format!("{branch}/{}", request.method);
        self.provisional.retain(|(k, ..)| *k != key);
        self.answered.retain(|(k, ..)| *k != key);
        let slot = match which {
            Where::Provisional => &mut self.provisional,
            Where::Final => &mut self.answered,
        };
        slot.push((key, bytes, to, Instant::now()));
    }

    /// Put a message on the wire, and say so once if it will not go.
    fn send(&mut self, message: &Message, to: SocketAddr) {
        self.put(&message.to_bytes(), to, message_kind(message));
    }

    /// The one place a datagram leaves that is not a retransmission, and so
    /// the one place a failure is worth a line in the transcript.
    ///
    /// Every send used to be `let _ = ...`. A trunk whose name resolves to an
    /// IPv6 address fails every one of them with `AddrNotAvailable` on a
    /// socket bound to `0.0.0.0`, and with all of them swallowed the only
    /// thing anybody saw was "dialling" for the full two minutes of
    /// `RING_LIMIT` before being told nobody had answered. Retransmissions
    /// stay quiet: one line a transaction is a diagnosis, one line every half
    /// second is a log nobody reads.
    fn put(&mut self, bytes: &[u8], to: SocketAddr, what: &str) {
        if let Err(e) = self.link.send(bytes, to) {
            self.note(format!("sip: could not send {what} to {to}: {e}"));
        }
    }

    fn event(&self, event: Event) {
        if let Ok(mut queue) = self.shared.events.lock() {
            queue.push_back(event);
        }
    }

    fn note(&self, text: String) {
        self.event(Event::Note(text));
    }

    fn set_call_state(&self, state: &str) {
        if let Ok(mut status) = self.shared.status.lock() {
            state.clone_into(&mut status.call);
        }
    }

    fn publish(&self) {
        let Ok(mut status) = self.shared.status.lock() else {
            return;
        };
        status.registered = self.registered_until.is_some_and(|at| at > Instant::now());
        status.registration_left = self
            .registered_until
            .map(|at| at.saturating_duration_since(Instant::now()).as_secs() as u32)
            .unwrap_or(0);
        status.public = self.public.map(|a| a.to_string());
        status.peer = self
            .call
            .as_ref()
            .map(|c| c.remote.uri.to_string());
    }
}

/// Which request a challenge is being answered for.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Register,
    Invite,
}

/// Which transaction a response was matched to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transaction {
    Register,
    Invite,
    Cancel,
    Bye,
}

/// Which list a response is filed in: 17.2.1's, or the one for the last
/// provisional sent before there was an answer to file.
#[derive(Debug, Clone, Copy)]
enum Where {
    Provisional,
    Final,
}

/// What a hang-up has left to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HangUp {
    /// 21.4.4: say no to a call ringing here that nobody answered.
    Decline,
    /// 15.1.1: end a call the far end answered.
    Bye,
    /// 9.1: stop one that is still being placed.
    Cancel,
    /// Nothing at all. The same thing is already out there and unanswered, and
    /// a second one of it would be a second transaction for one hang-up.
    AlreadyUnderWay,
    /// There is no call; only the state is left to settle.
    NoCall,
}

/// What [`Worker::end_call`] finds when it is asked.
///
/// A type of its own so that the decision below can be read -- and tested --
/// without a socket, a dialog and three transactions to build first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Underway {
    /// A call ringing here that has been neither answered nor declined.
    ringing: bool,
    /// A call the far end answered.
    established: bool,
    /// An INVITE of ours still running.
    inviting: bool,
    /// A BYE sent and not yet answered.
    bye: bool,
    /// A CANCEL sent, or wanted and waiting on 9.1's provisional response.
    cancelling: bool,
}

/// Idempotence, in one function: what is worth doing about a hang-up, given
/// what is already on its way out.
///
/// The whole of it is that a second ask while the first is in flight is
/// nothing, and that "in flight" means a transaction that is still running --
/// not a BYE or a CANCEL that was ever sent. A BYE whose transaction has given
/// up has not ended anything, and asking again must still be able to send one.
///
/// A BYE outranks everything else because it is the last thing a call does:
/// nothing that arrives afterwards can make a call worth cancelling or
/// declining again.
fn hang_up_now(underway: Underway) -> HangUp {
    if underway.bye {
        return HangUp::AlreadyUnderWay;
    }
    if underway.ringing {
        return HangUp::Decline;
    }
    if underway.established {
        return HangUp::Bye;
    }
    if underway.inviting {
        if underway.cancelling {
            return HangUp::AlreadyUnderWay;
        }
        return HangUp::Cancel;
    }
    HangUp::NoCall
}

/// Whether a response answers the transaction this request started.
///
/// 17.1.3: a client transaction is identified by the branch of the topmost Via
/// and the method in the CSeq. The Call-ID and the sequence number are checked
/// as well, because they are free and because what this is guarding against is
/// a *duplicate of a real response from a moment ago*, which has every field
/// right except the ones that say which request it belonged to.
///
/// The method matters more than it looks: 9.1 gives a CANCEL the INVITE's
/// branch and the INVITE's sequence number on purpose, so the method in the
/// CSeq is the only thing that tells those two transactions apart.
///
/// A response with no branch at all is matched on the rest. Nothing legitimate
/// sends one -- 8.2.6.2 has a far end copy the Via exactly as it arrived -- but
/// refusing to act on a real answer is a worse failure than acting on a strange
/// one that agrees about everything else.
fn answers(request: &Request, response: &Response) -> bool {
    if request.headers.call_id() != response.headers.call_id() {
        return false;
    }
    let (Some((asked, method)), Some((answered, about))) =
        (request.headers.cseq(), response.headers.cseq())
    else {
        return false;
    };
    if asked != answered || method != about {
        return false;
    }
    match (request.headers.branch(), response.headers.branch()) {
        (Some(ours), Some(theirs)) => ours == theirs,
        _ => true,
    }
}

/// Whether two dialog tags are compatible: a tag nobody has sent yet agrees
/// with anything, and two that have both been sent must be the same (12.2.2).
fn tags_agree(known: Option<&str>, arrived: Option<&str>) -> bool {
    match (known, arrived) {
        (Some(known), Some(arrived)) => known == arrived,
        _ => true,
    }
}

/// Whether a challenge that has already been answered once may be answered
/// again.
///
/// One answer to one challenge is authentication and two in a row normally
/// means the password or the realm is wrong, which is why the second used to
/// be refused outright. But `stale` was parsed and then read nowhere, and it
/// is the case that actually happens: an Asterisk-family registrar expires its
/// nonces on a timer and re-challenges with `stale=true` and a fresh one, which
/// RFC 7616 3.3 says is a client's cue to try again with the new nonce and not
/// a statement about the password at all. Refusing it dropped a perfectly good
/// account into a thirty-second backoff, over and over.
///
/// The nonce has to have changed, or "stale" would be a licence to loop on the
/// same one, and `AUTH_ATTEMPTS` bounds it however honest the registrar looks.
fn may_answer_again(attempts: u32, previous_nonce: &str, challenge: &Challenge) -> bool {
    if attempts == 0 {
        return true;
    }
    challenge.stale && challenge.nonce != previous_nonce && attempts < AUTH_ATTEMPTS
}

/// What a message is, for a line about a send that failed.
fn message_kind(message: &Message) -> &'static str {
    match message {
        Message::Request(r) => match r.method {
            Method::Invite => "an INVITE",
            Method::Ack => "an ACK",
            Method::Bye => "a BYE",
            Method::Cancel => "a CANCEL",
            Method::Register => "a REGISTER",
            _ => "a request",
        },
        Message::Response(_) => "a response",
    }
}

fn user_agent() -> String {
    format!("BinModem/{}", env!("CARGO_PKG_VERSION"))
}

/// The host part of a registrar setting, without its port: what goes in the
/// request URI of a REGISTER (10.2).
fn split_host(setting: String) -> String {
    crate::uri::split_host_port(&setting).0
}

fn expires_of(response: &Response) -> Option<u32> {
    // 10.2.4: the expiry may be on the Contact that came back rather than in
    // an Expires header, and registrars differ about which they use.
    if let Some(contact) = response.headers.contact()
        && let Some(expires) = contact.parameter("expires")
        && let Ok(n) = expires.parse()
    {
        return Some(n);
    }
    response.headers.get("Expires")?.trim().parse().ok()
}

/// A failure code in words, because the number alone tells a person nothing
/// and these are the ones a trunk actually sends.
fn describe(code: u16, reason: &str) -> String {
    let sense = match code {
        403 => "the trunk refused the call: the number, the caller ID or the account is not allowed to dial it",
        404 => "there is no such number",
        408 => "nobody answered the request",
        480 => "the far end is not available",
        486 => "busy",
        487 => "cancelled",
        488 => "the two ends could not agree on what to send",
        503 => "the trunk says it cannot take the call now",
        603 => "declined",
        _ => return format!("{code} {reason}"),
    };
    format!("{code} {reason} -- {sense}")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, UdpSocket};

    use super::*;
    use crate::account::Transport;

    #[test]
    fn a_failure_code_reads_as_a_sentence() {
        assert!(describe(403, "Forbidden").contains("not allowed to dial"));
        // One with no note of its own still says what it was.
        assert_eq!(describe(599, "Odd"), "599 Odd");
    }

    #[test]
    fn an_expiry_is_read_from_either_place() {
        let with_header = Response {
            code: 200,
            reason: "OK".to_owned(),
            headers: {
                let mut h = Headers::new();
                h.push("Expires", "120");
                h
            },
            body: Vec::new(),
        };
        assert_eq!(expires_of(&with_header), Some(120));

        let on_the_contact = Response {
            code: 200,
            reason: "OK".to_owned(),
            headers: {
                let mut h = Headers::new();
                h.push("Contact", "<sip:1001@192.0.2.4:5060>;expires=60");
                h
            },
            body: Vec::new(),
        };
        assert_eq!(expires_of(&on_the_contact), Some(60));
    }

    #[test]
    fn a_registrar_setting_gives_up_its_host() {
        assert_eq!(split_host("sip.example.net:5070".to_owned()), "sip.example.net");
        assert_eq!(split_host("sip.example.net".to_owned()), "sip.example.net");
    }

    /// A request as a transaction would hold it: enough headers to be matched
    /// against, and nothing else.
    fn request(method: Method, call_id: &str, cseq: u32, branch: &str) -> Request {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP 192.0.2.4:5060;rport;branch={branch}"));
        headers.push("Call-ID", call_id.to_owned());
        headers.push("CSeq", format!("{cseq} {method}"));
        Request {
            method,
            uri: Uri::user_at("0398765432", "sip.example.net"),
            headers,
            body: Vec::new(),
        }
    }

    /// And the response a far end would send to it, with the parts that say
    /// which request it is answering settable one at a time.
    fn response(code: u16, call_id: &str, cseq: u32, method: Method, branch: &str) -> Response {
        let mut headers = Headers::new();
        headers.push("Via", format!("SIP/2.0/UDP 192.0.2.4:5060;rport;branch={branch}"));
        headers.push("Call-ID", call_id.to_owned());
        headers.push("CSeq", format!("{cseq} {method}"));
        Response {
            code,
            reason: "Whatever".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    /// The three duplicates that used to be acted on, and the one response
    /// that has to be.
    #[test]
    fn a_response_is_matched_to_the_transaction_it_answers() {
        let invite = request(Method::Invite, "call-2", 2, "z9hG4bKtwo");
        assert!(answers(
            &invite,
            &response(200, "call-2", 2, Method::Invite, "z9hG4bKtwo")
        ));

        // A duplicate 486 from the call before. The whole fault in one line:
        // right method, right shape, wrong call -- and it used to tear this
        // one down.
        assert!(!answers(
            &invite,
            &response(486, "call-1", 1, Method::Invite, "z9hG4bKone")
        ));
        // The same call, but the request before the credentials went on:
        // 8.1.3.5 gives the retry a new branch and the next sequence number,
        // and a duplicate of the challenge answers neither.
        assert!(!answers(
            &invite,
            &response(407, "call-2", 1, Method::Invite, "z9hG4bKone")
        ));

        // 9.1: a CANCEL carries the INVITE's branch and the INVITE's sequence
        // number, so the method in the CSeq is the only thing separating the
        // two transactions. Getting this wrong would end a call on the 200
        // that merely acknowledged the CANCEL.
        let cancel = request(Method::Cancel, "call-2", 2, "z9hG4bKtwo");
        assert!(!answers(
            &invite,
            &response(200, "call-2", 2, Method::Cancel, "z9hG4bKtwo")
        ));
        assert!(answers(
            &cancel,
            &response(200, "call-2", 2, Method::Cancel, "z9hG4bKtwo")
        ));
        assert!(!answers(
            &cancel,
            &response(200, "call-2", 2, Method::Invite, "z9hG4bKtwo")
        ));
    }

    /// A far end that sends no Via back at all is still answering something,
    /// and the rest of the message says what.
    #[test]
    fn a_response_without_a_branch_is_matched_on_everything_else() {
        let bye = request(Method::Bye, "call-3", 4, "z9hG4bKthree");
        let mut naked = response(200, "call-3", 4, Method::Bye, "ignored");
        naked.headers.remove("Via");
        assert!(answers(&bye, &naked));

        let mut wrong_call = naked.clone();
        wrong_call.headers.set("Call-ID", "call-4");
        assert!(!answers(&bye, &wrong_call));
    }

    /// 12.2.2's tag comparison, and why it is not a plain equality: a dialog
    /// that is only half made has one tag, and a CANCEL for a call ringing
    /// here carries none of ours.
    #[test]
    fn a_tag_nobody_has_sent_yet_agrees_with_anything() {
        assert!(tags_agree(Some("abc"), Some("abc")));
        assert!(!tags_agree(Some("abc"), Some("def")));
        assert!(tags_agree(None, Some("def")));
        assert!(tags_agree(Some("abc"), None));
        assert!(tags_agree(None, None));
    }

    /// RFC 7616 3.3: a stale nonce is worth another try and a wrong password
    /// is not, and the two used to be treated alike.
    #[test]
    fn a_stale_nonce_is_answered_again_and_a_wrong_password_is_not() {
        let fresh = |stale: bool, nonce: &str| Challenge {
            scheme: "Digest".to_owned(),
            realm: "trunk.invalid".to_owned(),
            nonce: nonce.to_owned(),
            opaque: None,
            algorithm: None,
            qop: vec!["auth".to_owned()],
            stale,
            from_proxy: false,
        };

        // The first challenge is always answered, stale or not.
        assert!(may_answer_again(0, "", &fresh(false, "n1")));

        // A second challenge that says nothing about staleness is the
        // registrar saying no to the credentials themselves.
        assert!(!may_answer_again(1, "n1", &fresh(false, "n2")));
        // Stale, with a new nonce: the one case that used to be refused and
        // should not have been. It cost the account a thirty-second backoff
        // every time the registrar's nonce timer came round.
        assert!(may_answer_again(1, "n1", &fresh(true, "n2")));
        // Stale, but the same nonce we just answered: nothing has changed, so
        // answering again would only be a faster way of doing the same thing.
        assert!(!may_answer_again(1, "n1", &fresh(true, "n1")));
        // And the bound, so that a registrar which calls every nonce stale
        // cannot keep this going.
        assert!(!may_answer_again(AUTH_ATTEMPTS, "n3", &fresh(true, "n4")));
    }

    /// The whole of a hang-up's idempotence, as a table.
    ///
    /// The rule that matters is in the pairs: the first ask does something and
    /// the second does nothing, until whatever the first ask started has
    /// finished -- and then the next ask is a new attempt rather than a
    /// duplicate of the old one.
    #[test]
    fn a_hang_up_asked_for_twice_over_is_one_hang_up() {
        assert_eq!(hang_up_now(Underway::default()), HangUp::NoCall);

        // A call that is up. The first ask sends a BYE and every ask while
        // that BYE is unanswered does nothing at all; ten of them used to be
        // ten BYEs with ten sequence numbers and nine 481s.
        let up = Underway {
            established: true,
            ..Underway::default()
        };
        assert_eq!(hang_up_now(up), HangUp::Bye);
        assert_eq!(
            hang_up_now(Underway { bye: true, ..up }),
            HangUp::AlreadyUnderWay
        );
        // But not for ever. A BYE whose transaction has given up has ended
        // nothing, and asking again has to be able to send another one.
        assert_eq!(hang_up_now(Underway { bye: false, ..up }), HangUp::Bye);

        // One still being placed is cancelled once. Wanted and sent are the
        // same answer here, because 9.1 makes the wish the first half of the
        // CANCEL: it is spent the moment the far end says anything.
        let ringing_there = Underway {
            inviting: true,
            ..Underway::default()
        };
        assert_eq!(hang_up_now(ringing_there), HangUp::Cancel);
        assert_eq!(
            hang_up_now(Underway {
                cancelling: true,
                ..ringing_there
            }),
            HangUp::AlreadyUnderWay
        );
        // A BYE outranks it: once a call has been answered and then ended,
        // nothing arriving afterwards makes it worth cancelling again.
        assert_eq!(
            hang_up_now(Underway {
                bye: true,
                ..ringing_there
            }),
            HangUp::AlreadyUnderWay
        );

        // And one ringing here is declined once, because `decline` takes the
        // request: the second ask finds no call to say no to.
        let ringing_here = Underway {
            ringing: true,
            ..Underway::default()
        };
        assert_eq!(hang_up_now(ringing_here), HangUp::Decline);
        assert_eq!(
            hang_up_now(Underway {
                ringing: false,
                ..ringing_here
            }),
            HangUp::NoCall
        );
    }

    /// The socket is bound to 0.0.0.0, so an AAAA-first lookup has to be
    /// looked past or every datagram fails where nobody can see it.
    #[test]
    fn an_ipv4_address_is_preferred_to_an_ipv6_one() {
        let six: SocketAddr = "[2001:db8::1]:5060".parse().unwrap();
        let four: SocketAddr = "198.51.100.7:5060".parse().unwrap();
        assert_eq!(prefer_ipv4([six, four].into_iter()), Some(four));
        assert_eq!(prefer_ipv4([four, six].into_iter()), Some(four));
        // All there is, so all that can be offered: a send that fails and
        // says so beats a lookup that claims to have found nothing.
        assert_eq!(prefer_ipv4([six].into_iter()), Some(six));
        assert_eq!(prefer_ipv4([].into_iter()), None);
    }

    /// A worker with a socket of its own, and a second socket standing in for
    /// the far end so that what the worker sends can be read back.
    ///
    /// No thread and no `run` loop: these tests call the handlers directly,
    /// which is the only way to say "this datagram arrived and then this one
    /// did" without waiting on a timer to decide it for us.
    fn worker_and_far_end() -> (Worker, UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("the agent's socket");
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let local = socket.local_addr().unwrap();
        let far = UdpSocket::bind("127.0.0.1:0").expect("the far end's socket");
        // Generous: everything here crosses the loopback and arrives at once,
        // and the only reason to wait at all is a scheduler that is busy.
        far.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let hop = far.local_addr().unwrap();
        let shared = Arc::new(Shared {
            commands: Mutex::new(VecDeque::new()),
            events: Mutex::new(VecDeque::new()),
            status: Mutex::new(Status::default()),
            quit: AtomicBool::new(false),
        });
        let link = Link::on_socket(socket, local);
        let worker = Worker::new(Account::default(), shared, link, local, hop, 40000);
        (worker, far, hop)
    }

    /// One message off the far end's socket, or nothing if none came.
    fn heard(far: &UdpSocket) -> Option<Message> {
        let mut buffer = vec![0u8; 8192];
        let (n, _) = far.recv_from(&mut buffer).ok()?;
        Message::parse(&buffer[..n])
    }

    /// Everything the worker has had to say, drained.
    fn said(worker: &Worker) -> Vec<Event> {
        worker
            .shared
            .events
            .lock()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    }

    /// 13.3.1.4 and 13.2.2.4 together: the far end's retransmitted 200 is
    /// acknowledged again, and does nothing else whatever.
    ///
    /// This path carries about 750 ms each way, so the far end always gets a
    /// retransmission out before our ACK can reach it: it fires on
    /// essentially every call. Every 2xx used to go through `adopt_dialog`,
    /// `send_ack`, `read_answer` and `Answered` alike, and the layer above
    /// turns each `Answered` into `Media::connect`, which flushes the jitter
    /// buffer and the outgoing queue -- half a second of the modem's V.8
    /// handshake, thrown away for nothing.
    /// The dialog as it stands once a call is up: both tags known, the INVITE
    /// answered and acknowledged, and the sequence number sitting where the
    /// next in-dialog request will take it from.
    fn a_call_that_is_up(call_id: &str, hop: SocketAddr) -> Dialog {
        let mut local = Address::new(Uri::user_at("0398765432", "127.0.0.1"));
        local.set_parameter("tag", "ours");
        let mut remote = Address::new(Uri::user_at("0312345678", "trunk.invalid"));
        remote.set_parameter("tag", "theirs");
        Dialog {
            call_id: call_id.to_owned(),
            local,
            remote,
            cseq: 2,
            invite_cseq: 2,
            target: Uri::user_at("0312345678", "127.0.0.1"),
            routes: Vec::new(),
            hop,
            established: true,
            media: None,
        }
    }

    #[test]
    fn a_retransmitted_two_hundred_is_acknowledged_and_nothing_else() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.call = Some(a_call_that_is_up("call-6", hop));
        let _ = said(&worker);

        let mut again = response(200, "call-6", 2, Method::Invite, "z9hG4bKsix");
        again.headers.push("To", "<sip:0312345678@trunk.invalid>;tag=theirs");
        again.headers.push("Contact", "<sip:0312345678@127.0.0.1:5060>");
        worker.handle_response(again, hop);

        let answer = heard(&far).expect("the retransmitted 200 was not acknowledged at all");
        assert_eq!(
            answer.as_request().map(|r| r.method.clone()),
            Some(Method::Ack),
            "something other than an ACK went back to a retransmitted 200"
        );
        assert!(
            said(&worker).is_empty(),
            "a retransmitted 200 raised an event, and the layer above turns \
             an Answered into a Media::connect"
        );
    }

    /// 12.2.2 and 17.2.1: a caller's INVITE arriving twice is answered twice
    /// with the same provisional response, not answered 200.
    ///
    /// Being in a dialog used to be decided on the Call-ID alone, and the
    /// dialog was stored before anybody had picked up, so an ordinary
    /// retransmission reached `re_invite` -- which built an SDP answer and
    /// sent 200 OK. The telephone answered itself, with no tag of ours on the
    /// To and `ringing` still set as though it were still ringing.
    #[test]
    fn a_retransmitted_invite_does_not_answer_the_telephone() {
        let (mut worker, far, hop) = worker_and_far_end();
        let mut invite = request(Method::Invite, "call-7", 1, "z9hG4bKseven");
        invite
            .headers
            .push("From", "<sip:0312345678@trunk.invalid>;tag=theirs");
        invite.headers.push("To", "<sip:0398765432@127.0.0.1>");
        invite
            .headers
            .push("Contact", "<sip:0312345678@127.0.0.1:5060>");

        worker.handle_request(invite.clone(), hop);
        assert_eq!(code_of(heard(&far)), Some(100));
        assert_eq!(code_of(heard(&far)), Some(180));
        assert!(worker.ringing.is_some(), "the call is not ringing here");

        // The same INVITE, octet for octet, as a 750 ms path produces.
        worker.handle_request(invite, hop);
        assert_eq!(
            code_of(heard(&far)),
            Some(180),
            "a retransmitted INVITE was not answered with the 180 it was \
             answered with the first time"
        );
        assert!(
            worker.ringing.is_some(),
            "a retransmitted INVITE took the call off the hook"
        );
        assert!(
            !worker.call.as_ref().is_some_and(|c| c.established),
            "a retransmitted INVITE established the dialog by itself"
        );
        assert!(heard(&far).is_none(), "something else went out as well");
    }

    fn code_of(message: Option<Message>) -> Option<u16> {
        message.and_then(|m| m.as_response().map(|r| r.code))
    }

    /// 9.1: the CANCEL waits for the far end to say something, and then goes
    /// -- and goes again if it has to.
    ///
    /// `ATD` and then `ATH` inside the 750 ms this path carries used to send a
    /// CANCEL to a proxy that had no server transaction to match it with. It
    /// answered 481, our INVITE went on being retransmitted, the trunk rang
    /// the number, somebody picked it up, and the 200 brought up a call the
    /// user had already hung up on -- and was billed for. One lost CANCEL
    /// datagram did the same thing, because there was no transaction behind it
    /// to send it again.
    #[test]
    fn a_hang_up_before_the_far_end_speaks_waits_for_it_before_cancelling() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;

        worker.place_call("0312345678");
        let sent = heard(&far).expect("no INVITE went out at all");
        let invite = sent.as_request().expect("the INVITE was not a request").clone();
        assert_eq!(invite.method, Method::Invite);
        let call_id = invite.headers.call_id().expect("the INVITE had no Call-ID");
        let branch = invite.headers.branch().expect("the INVITE had no branch");

        // ATH, before the trunk has said anything whatever.
        worker.end_call();
        assert!(
            heard(&far).is_none(),
            "a CANCEL went out before any provisional response had arrived (9.1)"
        );
        assert!(worker.cancel_wanted, "the hang-up was forgotten instead of kept");
        assert!(
            worker
                .invite
                .as_ref()
                .is_some_and(|p| p.deadline <= Instant::now() + TIMER_B),
            "a cancelled call would sit at \"cancelling\" for the whole of RING_LIMIT"
        );

        // The trunk's 100. Now there is something at the far end for a CANCEL
        // to match, and it goes.
        worker.handle_response(response(100, call_id, 1, Method::Invite, branch), hop);
        let sent = heard(&far).expect("the CANCEL was never sent at all");
        let cancel = sent.as_request().expect("the CANCEL was not a request");
        assert_eq!(cancel.method, Method::Cancel);
        assert_eq!(
            cancel.headers.branch(),
            Some(branch),
            "9.1: a CANCEL copies the branch of the INVITE it cancels"
        );
        assert!(!worker.cancel_wanted);
        assert!(
            worker.cancel.is_some(),
            "17.1.2.2: without a transaction behind it, one lost CANCEL is \
             indistinguishable from never having hung up"
        );
    }

    /// Ten hang-ups, one BYE.
    ///
    /// The caller above is level-triggered: while the modem is on hook and the
    /// call is still up it asks again on every turn of its loop, which is every
    /// 2 ms, and nothing it can see changes until this thread next runs -- up
    /// to 20 ms later. About ten `HangUp` commands therefore land in the queue
    /// for one hang-up, `take_commands` drains the lot, and every one of them
    /// used to reach `send_bye`: ten BYEs, each with the next sequence number
    /// and a new branch, each replacing the transaction before it. On a live
    /// trunk nine of them came back 481, and the one answer that meant
    /// anything arrived for a transaction that had already been thrown away.
    #[test]
    fn hanging_up_ten_times_over_sends_one_bye_with_one_sequence_number() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.call = Some(a_call_that_is_up("call-8", hop));
        worker.set_call_state(state::UP);
        let _ = said(&worker);

        for _ in 0..10 {
            worker.end_call();
        }

        let sent = heard(&far).expect("no BYE went out at all");
        let bye = sent.as_request().expect("what went out was not a request");
        assert_eq!(bye.method, Method::Bye);
        assert_eq!(
            bye.headers.cseq().map(|(n, _)| n),
            Some(3),
            "12.2.1.1: the BYE takes the next sequence number in the dialog"
        );
        assert!(
            heard(&far).is_none(),
            "a second BYE went out for one hang-up; a trunk answers 481 to \
             every one after the first"
        );
        assert!(
            worker.bye.is_some(),
            "the BYE transaction is gone, and with it 17.1.2.2's retransmission \
             of a request that may never have arrived"
        );
        // The same guard as before, read where the sequence number now lives.
        // It used to be `worker.call`'s, which is gone by here: 15.1.1 ends the
        // call when the BYE is passed to its transaction, and the transaction
        // is holding the request it will send again. The fault it catches is
        // the one it always caught -- a sequence number that moved once per
        // ask, leaving the answer to the BYE matching nothing.
        assert_eq!(
            worker
                .bye
                .as_ref()
                .and_then(|p| p.request.headers.cseq())
                .map(|(n, _)| n),
            Some(3),
            "the dialog's sequence number moved once per ask rather than once \
             per hang-up, so the answer to the BYE will match nothing"
        );
        // 15.1.1: the session is over the moment the BYE goes to its
        // transaction, so the line is free for the next number at once. It
        // used to stand until the far end answered -- about 1.5 s on this
        // path, and the whole of Timer F's 32 s when the far end had gone --
        // and `place_call` refused every number for all of it.
        assert!(
            worker.call.is_none(),
            "the call is still here after its BYE went out, so the next number \
             dialled will be refused (15.1.1)"
        );
        assert_eq!(state_of(&worker), state::IDLE);
    }

    /// What the window is reading, so that a test can say what it would show.
    fn state_of(worker: &Worker) -> String {
        worker
            .shared
            .status
            .lock()
            .map(|s| s.call.clone())
            .unwrap_or_default()
    }

    /// A number dialled the moment the call before it was put down.
    ///
    /// The fault people kept reporting as "you can not redial", in the two
    /// commands it takes: the window's Hang up button and then its Call
    /// button, both landing in the queue and both drained in one pass. The
    /// BYE's answer is about 1.5 s away on this path and 32 s away when the
    /// far end has gone -- which is what has usually happened when a call
    /// drops -- and for all of that time the dial used to be refused, because
    /// `self.call` stood until `finish_call` and `place_call` looked at it.
    #[test]
    fn a_number_dialled_while_the_bye_is_still_in_flight_is_placed_anyway() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;
        worker.call = Some(a_call_that_is_up("call-10", hop));
        worker.set_call_state(state::UP);
        let _ = said(&worker);

        worker.end_call();
        let sent = heard(&far).expect("no BYE went out at all");
        assert_eq!(sent.as_request().map(|r| r.method.clone()), Some(Method::Bye));
        assert!(
            worker.bye.is_some(),
            "the BYE has no transaction behind it to send it again (17.1.2.2)"
        );

        // And the next number, before anything has answered that BYE.
        worker.place_call("0312345678");
        let sent = heard(&far).expect(
            "the redial never left this machine: a call on its way out is still \
             blocking the next one",
        );
        let invite = sent.as_request().expect("what went out was not a request");
        assert_eq!(invite.method, Method::Invite);
        assert_eq!(state_of(&worker), state::DIALLING);
        assert!(
            said(&worker)
                .iter()
                .all(|e| !matches!(e, Event::Failed { .. })),
            "the redial was refused"
        );
    }

    /// And the answer to that BYE, when it finally comes, must not end the
    /// call placed since.
    ///
    /// 17.1.2.2 leaves a BYE's transaction running for 32 s. A trunk that was
    /// slow rather than gone answers somewhere in there, by which time the
    /// person has redialled. The answer ends its own transaction and nothing
    /// else -- the dialog it names was finished with here when the BYE went
    /// out (15.1.1). This is what `finish_call` in that arm would have done
    /// the moment a second call became possible: taken it.
    #[test]
    fn a_late_answer_to_an_old_bye_ends_its_transaction_and_nothing_else() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;
        worker.call = Some(a_call_that_is_up("call-11", hop));
        worker.set_call_state(state::UP);

        worker.end_call();
        let sent = heard(&far).expect("no BYE went out at all");
        let bye = sent.as_request().expect("the BYE was not a request").clone();
        let (cseq, _) = bye.headers.cseq().expect("the BYE had no CSeq");
        let branch = bye.headers.branch().expect("the BYE had no branch").to_owned();

        worker.place_call("0312345678");
        let _ = heard(&far).expect("the redial never went out");
        let _ = said(&worker);

        // The trunk gets round to the BYE at last.
        worker.handle_response(response(200, "call-11", cseq, Method::Bye, &branch), hop);

        assert!(worker.bye.is_none(), "the BYE transaction is still running");
        assert!(
            worker.call.is_some(),
            "a late answer to the last call's BYE took the call placed since"
        );
        assert_eq!(state_of(&worker), state::DIALLING);
        assert!(
            worker.invite.is_some(),
            "the INVITE for the new call was thrown away with the old BYE"
        );
        assert!(
            !said(&worker)
                .iter()
                .any(|e| matches!(e, Event::Ended { .. })),
            "the call placed since was reported as ended"
        );
    }

    /// And the BYE that is never answered at all.
    ///
    /// 17.1.2.2's Timer F comes round 32 s after it went out -- which is what
    /// happens when the thing at the far end has gone, and that is what a call
    /// dropping usually is. By then the person has redialled. Giving up on a
    /// transaction ends that transaction: this arm used to call `finish_call`,
    /// which takes whatever call there is, and the call there is now is the
    /// one they are on.
    ///
    /// The deadline is brought forward rather than waited out; what is being
    /// tested is what happens when it passes.
    #[test]
    fn a_bye_that_is_never_answered_ends_its_transaction_and_nothing_else() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;
        worker.call = Some(a_call_that_is_up("call-12", hop));
        worker.set_call_state(state::UP);

        worker.end_call();
        let sent = heard(&far).expect("no BYE went out at all");
        assert_eq!(sent.as_request().map(|r| r.method.clone()), Some(Method::Bye));
        worker.place_call("0312345678");
        let sent = heard(&far).expect("the redial never went out");
        assert_eq!(
            sent.as_request().map(|r| r.method.clone()),
            Some(Method::Invite)
        );
        let _ = said(&worker);

        if let Some(pending) = worker.bye.as_mut() {
            pending.deadline = Instant::now();
        }
        worker.service_timers();

        assert!(worker.bye.is_none(), "the transaction was not given up on");
        assert!(
            worker.call.is_some(),
            "giving up on the last call's BYE took the call placed since"
        );
        assert!(
            worker.invite.is_some(),
            "the INVITE for the new call went with the old BYE"
        );
        assert_eq!(state_of(&worker), state::DIALLING);
        assert!(
            !said(&worker)
                .iter()
                .any(|e| matches!(e, Event::Ended { .. })),
            "the call placed since was reported as ended"
        );
    }

    /// A number dialled while a CANCEL is in flight waits for it rather than
    /// being thrown away.
    ///
    /// 9.1 is why this one leg cannot be let go of: the far end may have
    /// committed to a 200 before the CANCEL reached it, and that dialog has to
    /// be acknowledged and then ended or its half of the call goes on being
    /// billed. So the number is kept, and spent the moment the leg settles.
    #[test]
    fn a_number_dialled_while_a_cancel_is_in_flight_waits_and_is_then_placed() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;

        worker.place_call("0312345678");
        let sent = heard(&far).expect("no INVITE went out at all");
        let invite = sent.as_request().expect("the INVITE was not a request").clone();
        let call_id = invite.headers.call_id().expect("no Call-ID").to_owned();
        let branch = invite.headers.branch().expect("no branch").to_owned();

        // The far end rings, the person hangs up, and the CANCEL goes.
        worker.handle_response(response(180, &call_id, 1, Method::Invite, &branch), hop);
        worker.end_call();
        let sent = heard(&far).expect("the CANCEL never went out");
        assert_eq!(sent.as_request().map(|r| r.method.clone()), Some(Method::Cancel));

        // And the next number straight after it.
        worker.place_call("0398765432");
        assert!(worker.held_dial.is_some(), "the number was thrown away");
        worker.service_timers();
        assert!(
            worker.held_dial.is_some(),
            "the held number was placed before the cancelled call had settled"
        );
        assert_eq!(
            worker
                .invite
                .as_ref()
                .and_then(|p| p.request.headers.call_id()),
            Some(call_id.as_str()),
            "the INVITE being cancelled was replaced by the new one, so the \
             200 the far end may still send for it would answer nothing (9.1)"
        );
        // Drained rather than asked for one message: 17.1.2.2 gives the CANCEL
        // a transaction of its own and it retransmits while all this is going
        // on, so what is asserted is that no *INVITE* went out.
        assert!(
            !sent_any(&far, Method::Invite),
            "a second INVITE went out while the first was still being \
             cancelled; the far end may still answer that one (9.1)"
        );

        // The 487 settles it, and the number that was waiting goes out.
        let _ = said(&worker);
        worker.handle_response(
            response(487, &call_id, 1, Method::Invite, &branch),
            hop,
        );
        worker.service_timers();
        let out = everything(&far);
        assert!(
            out.iter()
                .any(|m| m.as_request().is_some_and(|r| r.method == Method::Ack)),
            "the 487 was not acknowledged (17.1.1.3): {out:#?}"
        );
        assert!(
            out.iter()
                .any(|m| m.as_request().is_some_and(|r| r.method == Method::Invite)),
            "the number that was waiting was never dialled: {out:#?}"
        );
        assert!(worker.held_dial.is_none());
        assert_eq!(state_of(&worker), state::DIALLING);
    }

    /// Everything sitting on the far end's socket, drained.
    fn everything(far: &UdpSocket) -> Vec<Message> {
        let mut all = Vec::new();
        while let Some(message) = heard(far) {
            all.push(message);
        }
        all
    }

    /// Whether anything of this method went out.
    fn sent_any(far: &UdpSocket, method: Method) -> bool {
        everything(far)
            .iter()
            .any(|m| m.as_request().is_some_and(|r| r.method == method))
    }

    /// And a held number that nothing ever settles is given up on in words.
    ///
    /// `Event::Failed` and not silence: over SIP the modem above is stepped by
    /// arriving RTP and by nothing else, so a number dropped quietly leaves it
    /// off hook waiting for a carrier on a call that was never placed.
    #[test]
    fn a_held_number_that_waits_too_long_is_given_up_on_in_words() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;

        worker.place_call("0312345678");
        let sent = heard(&far).expect("no INVITE went out at all");
        let invite = sent.as_request().expect("not a request").clone();
        let call_id = invite.headers.call_id().expect("no Call-ID").to_owned();
        let branch = invite.headers.branch().expect("no branch").to_owned();
        worker.handle_response(response(180, &call_id, 1, Method::Invite, &branch), hop);
        worker.end_call();
        let _ = heard(&far);

        worker.place_call("0398765432");
        // The far end says nothing more, ever. Rather than wait the four
        // seconds out, the deadline is moved to now: what is being tested is
        // what happens when it passes, not the clock.
        if let Some((_, by)) = worker.held_dial.as_mut() {
            *by = Instant::now();
        }
        let _ = said(&worker);
        worker.service_timers();

        assert!(worker.held_dial.is_none(), "the number is still waiting");
        assert!(
            !sent_any(&far, Method::Invite),
            "it was placed after all, at a far end nobody is waiting on"
        );
        let events = said(&worker);
        let refusal = events
            .iter()
            .find_map(|e| match e {
                Event::Failed { code, reason } => Some((*code, reason.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the number was dropped in silence: {events:#?}"));
        assert_eq!(refusal.0, 0, "nothing left this machine, so there is no code");
        assert!(
            refusal.1.contains("0398765432"),
            "the refusal does not say what was not dialled: {}",
            refusal.1
        );
    }

    /// The same, for a call that is still ringing: ten hang-ups, one CANCEL,
    /// and one line in the transcript rather than ten.
    #[test]
    fn hanging_up_ten_times_before_the_far_end_speaks_sends_one_cancel() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.account.registrar = hop.to_string();
        worker.account.domain = "127.0.0.1".to_owned();
        worker.account.username = "0398765432".to_owned();
        worker.want_registration = false;

        worker.place_call("0312345678");
        let sent = heard(&far).expect("no INVITE went out at all");
        let invite = sent.as_request().expect("the INVITE was not a request").clone();
        let call_id = invite.headers.call_id().expect("the INVITE had no Call-ID").to_owned();
        let branch = invite.headers.branch().expect("the INVITE had no branch").to_owned();
        let _ = said(&worker);

        // ATH, and then the caller's loop asking again nine more times before
        // the trunk has said anything at all.
        for _ in 0..10 {
            worker.end_call();
        }
        assert!(
            heard(&far).is_none(),
            "a CANCEL went out before any provisional response had arrived (9.1)"
        );
        let notes = said(&worker);
        assert_eq!(
            notes.len(),
            1,
            "one hang-up put {} things in the transcript: {notes:#?}",
            notes.len()
        );

        // The trunk's 100 spends the wish, and then the asking starts again.
        worker.handle_response(response(100, &call_id, 1, Method::Invite, &branch), hop);
        for _ in 0..10 {
            worker.end_call();
        }
        let sent = heard(&far).expect("the CANCEL was never sent at all");
        assert_eq!(
            sent.as_request().map(|r| r.method.clone()),
            Some(Method::Cancel)
        );
        assert!(
            heard(&far).is_none(),
            "a second CANCEL went out for a call that was already being cancelled"
        );
    }

    /// A dial the agent will not place says so in an event, not only in a note.
    ///
    /// Over SIP the modem above is stepped by arriving RTP and by nothing else.
    /// `ATD` has already put it off hook and taken its dial string, so a
    /// refusal that only writes a line in the transcript leaves it waiting for
    /// a carrier on a call that was never placed -- with no samples arriving,
    /// none of its own timers advance and nothing ever times out. It waits
    /// until somebody forces the line down.
    #[test]
    fn a_dial_refused_because_a_call_is_up_is_a_failure_and_not_a_note() {
        let (mut worker, far, hop) = worker_and_far_end();
        worker.call = Some(a_call_that_is_up("call-9", hop));
        let _ = said(&worker);

        worker.place_call("0312345678");

        assert!(heard(&far).is_none(), "a second INVITE went out anyway");
        let events = said(&worker);
        let (code, reason) = events
            .iter()
            .find_map(|e| match e {
                Event::Failed { code, reason } => Some((*code, reason.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the dial was refused in silence: {events:#?}"));
        assert_eq!(
            code, 0,
            "nothing left this machine, so there is no status code to report \
             and `line.rs` words 0 as exactly that"
        );
        assert!(
            reason.contains("0312345678"),
            "the refusal does not say what was not dialled: {reason}"
        );
    }

    /// And the answering side: `ATA` with nothing ringing puts the modem off
    /// hook just as surely, so it cannot be refused in silence either.
    #[test]
    fn an_answer_with_nothing_ringing_is_a_failure_and_not_a_note() {
        let (mut worker, _far, _hop) = worker_and_far_end();
        let _ = said(&worker);

        worker.accept_call();

        let events = said(&worker);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Failed { code: 0, .. })),
            "ATA with nothing ringing said nothing the layer above could act \
             on: {events:#?}"
        );
    }

    /// 12.1.1: the Record-Route headers are copied into the response that
    /// makes the dialog, in order, and left off the ones that do not.
    #[test]
    fn record_route_is_echoed_in_the_answer_to_an_invite() {
        let (worker, _far, _hop) = worker_and_far_end();
        let mut invite = request(Method::Invite, "call-5", 1, "z9hG4bKfive");
        invite.headers.push("Record-Route", "<sip:edge.example.net;lr>");
        invite.headers.push("Record-Route", "<sip:core.example.net;lr>");

        let answer = worker.response_to(&invite, 200, "OK");
        let routes: Vec<&str> = answer.headers.all("Record-Route").collect();
        assert_eq!(
            routes,
            ["<sip:edge.example.net;lr>", "<sip:core.example.net;lr>"],
            "without these the caller has no route set and its BYE goes nowhere"
        );

        // A 100 is not the response that makes a dialog, and neither is a
        // failure, so neither carries them.
        assert_eq!(worker.response_to(&invite, 100, "Trying").headers.all("Record-Route").count(), 0);
        assert_eq!(worker.response_to(&invite, 486, "Busy Here").headers.all("Record-Route").count(), 0);

        let bye = request(Method::Bye, "call-5", 2, "z9hG4bKsix");
        assert_eq!(worker.response_to(&bye, 200, "OK").headers.all("Record-Route").count(), 0);
    }

    // ---- over TCP ------------------------------------------------------

    /// The trunk's own framing, for the test below: find the blank line, read
    /// the Content-Length, take that many octets of body.
    ///
    /// Written out here rather than borrowed from `transport`, because a far
    /// end that frames with the same function as the code under test cannot
    /// fail when that function is wrong. It is the short, strict version: this
    /// end of the test controls what it is sent.
    fn next_message(buffer: &mut Vec<u8>) -> Option<Message> {
        let at = (0..buffer.len()).find(|i| buffer[*i..].starts_with(b"\r\n\r\n"))?;
        let head = String::from_utf8_lossy(&buffer[..at]).to_ascii_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())?;
        let end = at + 4 + length;
        if buffer.len() < end {
            return None;
        }
        let message = Message::parse(&buffer[..end]);
        buffer.drain(..end);
        message
    }

    /// A registration over TCP, against a trunk that takes its time.
    ///
    /// Three things at once, and they are the three that make TCP a different
    /// transport rather than a different socket. The REGISTER is found in a
    /// byte stream by its Content-Length (7.5). Its Via says TCP and its
    /// Contact says `transport=tcp`, so that the trunk answers and calls back
    /// the way it was reached (18.1.1, 19.1.1). And it arrives exactly once:
    /// the trunk sits on it for longer than 17.1.1.2's first two intervals
    /// before answering, which over UDP would have brought a second copy, and
    /// over a reliable transport must not, because 17.1.2.2 does not run Timer
    /// E at all.
    ///
    /// Not flaky, and it is worth saying why rather than hoping. The count is
    /// asserted in the direction a busy machine cannot break: a scheduler that
    /// is behind produces *fewer* messages, never more. The registration
    /// itself is waited for rather than timed, with a deadline long enough
    /// that only a stack which never registers will reach it. And nothing here
    /// depends on the order two threads get to the processor in, because the
    /// listener is bound before the agent is told where to connect.
    #[test]
    fn a_registration_over_tcp_arrives_once_and_is_answered_on_the_connection() {
        /// How long the trunk sits on the REGISTER before answering it. Longer
        /// than T1 and than the 2*T1 after it, so that a stack retransmitting
        /// on 17.1.1.2's schedule would have sent two more by the end of it.
        const HELD: Duration = Duration::from_millis(1200);

        let listener = TcpListener::bind("127.0.0.1:0").expect("the trunk's listener");
        let trunk = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let seen = Arc::new(Mutex::new(Vec::<Message>::new()));

        let theirs = Arc::clone(&seen);
        let far_end = thread::spawn(move || {
            let finish = Instant::now() + HELD + Duration::from_secs(2);
            let answer_at = Instant::now() + HELD;
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(_) if Instant::now() < finish => thread::sleep(Duration::from_millis(5)),
                    Err(e) => panic!("nothing ever connected to the trunk: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(20)))
                .unwrap();
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 2048];
            let mut first: Option<Request> = None;
            let mut answered = false;
            while Instant::now() < finish {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    Err(_) => {}
                }
                while let Some(message) = next_message(&mut buffer) {
                    if let Some(request) = message.as_request()
                        && request.method == Method::Register
                        && first.is_none()
                    {
                        first = Some(request.clone());
                    }
                    if let Ok(mut book) = theirs.lock() {
                        book.push(message);
                    }
                }
                if !answered
                    && Instant::now() >= answer_at
                    && let Some(request) = &first
                {
                    let headers = request.headers.clone();
                    let take = |name: &str| headers.get(name).unwrap_or_default().to_owned();
                    let ok = format!(
                        "SIP/2.0 200 OK\r\n\
                         Via: {}\r\n\
                         From: {}\r\n\
                         To: {};tag=trunkside\r\n\
                         Call-ID: {}\r\n\
                         CSeq: {}\r\n\
                         Expires: 120\r\n\
                         Content-Length: 0\r\n\
                         \r\n",
                        take("Via"),
                        take("From"),
                        take("To"),
                        take("Call-ID"),
                        take("CSeq"),
                    );
                    stream.write_all(ok.as_bytes()).expect("the 200 would not go");
                    answered = true;
                }
            }
        });

        let account = Account {
            name: "tcp trunk".to_owned(),
            registrar: trunk.to_string(),
            domain: "127.0.0.1".to_owned(),
            username: "0398765432".to_owned(),
            password: "not checked here".to_owned(),
            register: true,
            expires: 120,
            transport: Transport::Tcp,
            ..Account::default()
        };
        let agent = Agent::start(account, 40000).expect("the agent would not start over TCP");

        let deadline = Instant::now() + Duration::from_secs(8);
        while !agent.status().registered {
            assert!(
                Instant::now() < deadline,
                "the line never registered over TCP. It said:\n{:#?}",
                agent.events()
            );
            thread::sleep(Duration::from_millis(10));
        }

        // Before the agent is dropped, which would put an un-REGISTER in here
        // as well.
        let arrived = seen.lock().map(|book| book.clone()).unwrap_or_default();
        let registers: Vec<&Request> = arrived
            .iter()
            .filter_map(|m| m.as_request())
            .filter(|r| r.method == Method::Register)
            .collect();
        assert_eq!(
            registers.len(),
            1,
            "17.1.2.2: a request must not be sent twice over a reliable \
             transport, and this one was held unanswered for {HELD:?}"
        );
        let register = registers[0];
        let via = register.headers.get("Via").unwrap_or_default();
        assert!(
            via.contains("SIP/2.0/TCP"),
            "18.1.1: the Via has to name the transport the request went out \
             over, and this one says \"{via}\""
        );
        let contact = register.headers.get("Contact").unwrap_or_default();
        assert!(
            contact.to_ascii_lowercase().contains("transport=tcp"),
            "19.1.1: without this the trunk reads 19.1.2's default and sends \
             in-dialog requests as datagrams to a port nothing is listening \
             on. The Contact was \"{contact}\""
        );

        drop(agent);
        let _ = far_end.join();
    }
}
