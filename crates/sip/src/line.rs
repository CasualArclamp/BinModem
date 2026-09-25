//! A SIP call presented as a line, with the same two methods a sound card has.
//!
//! [`crate::ua::Agent`] knows about dialogs and [`crate::media::Media`] knows
//! about packets; neither knows about the other, and something has to. This is
//! that something: it opens the media port first so the offer can name it,
//! starts the agent, and when a call is answered it points the media at
//! whatever the two ends agreed to.
//!
//! The shape is deliberately `line::Duplex`'s -- `receive` fills a buffer at
//! the modem's rate, `transmit` takes one -- so that the loop driving the
//! modem does not have to know which kind of line it has got. The one
//! difference is that this line has a dialling state: a sound card is either
//! open or not, where a call is idle, ringing, up or over, and nothing arrives
//! from it until it is up. That difference is carried in [`Progress`] rather
//! than in the sample path, which means the loop above sees exactly what it
//! sees with a sound card that has not been spoken to yet: nothing, until
//! there is something.
//!
//! **The modem is clocked by the call.** Before the far end answers there are
//! no packets, so `receive` hands over nothing, so the modem is never stepped
//! and does not start its handshake into a call that has not connected. That
//! falls out of the arrangement rather than being arranged, and it is the
//! right behaviour: a modem that starts transmitting at ringback is a modem
//! whose first second of V.8 was spent talking to a switch.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::account::Account;
use crate::media::{Media, Stats};
use crate::sdp::{Direction, Negotiated};
use crate::ua::{Agent, Event, Status};

/// Where a call has got to, and everything worth showing about it.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub account: String,
    pub registered: bool,
    /// Seconds until the registration has to be renewed.
    pub registration_left: u32,
    /// The address the outside world sees us at, once a registrar has said.
    pub public: Option<String>,
    /// Where the call has got to, in words: idle, dialling, ringing, up.
    pub call: String,
    /// Who is at the other end.
    pub peer: Option<String>,
    /// Whether audio is flowing.
    pub media: bool,
    /// The companding law in force: what the far end is actually sending.
    pub law: Option<String>,
    pub packets_sent: u64,
    pub packets_received: u64,
    /// Packets that never arrived, and octets of silence handed to the modem
    /// in their place. On this rig these are the first numbers to look at when
    /// a call that should have worked did not.
    pub lost: u64,
    pub concealed: u64,
    /// Packets we sent as silence because the modem had nothing ready.
    pub underruns: u64,
    /// And the subset of those that cut into audio the modem was already
    /// producing, which is the only subset that did any harm.
    ///
    /// The difference matters more than it sounds. On the first live call
    /// over this crate the window showed 38 gaps sent, which reads as 760 ms
    /// of silence transmitted into the far end's handshake; most of them were
    /// in the stretch before the far end's first packet had come through the
    /// jitter buffer, when the modem had not been stepped and so had nothing
    /// to say. Silence there is a hole in nothing. Counting the two together
    /// makes a healthy call look like a damaged one, which is the opposite of
    /// what these numbers are for.
    pub underruns_mid_call: u64,
    /// Octets of silence put in on purpose, to build the outgoing cushion
    /// back up after it has been spent. Deliberate, and still silence.
    pub cushioned: u64,
    pub last_error: Option<String>,
}

/// A line made of packets.
#[derive(Debug)]
pub struct Line {
    agent: Agent,
    media: Media,
    /// Held so that a caller can ask what the last call agreed to without
    /// having watched the events go by.
    negotiated: Mutex<Option<Negotiated>>,
    /// Whether the description in force lets the modem's samples out.
    ///
    /// False while the far end has said it is not listening -- `a=recvonly`
    /// from it, or either form of hold -- and true the rest of the time. It is
    /// here rather than in [`Media`] because it is a fact about what the two
    /// ends agreed to, which is this file's business; the media path knows
    /// about packets and has no idea what was said in the SDP.
    sending: AtomicBool,
    error: Mutex<Option<String>>,
    account: String,
}

impl Line {
    /// Open the media port, start the agent, and register if the account says
    /// to. No call is placed; that is [`Line::dial`].
    ///
    /// `modem_rate` is the rate the modem runs at. Everything on the network
    /// side of this is 8000, and the conversion between them is the only
    /// processing in the path.
    pub fn open(account: Account, modem_rate: f64) -> Result<Self, String> {
        let media = Media::open(account.rtp_port, modem_rate)?;
        let name = account.name.clone();
        let agent = Agent::start(account, media.local_port())?;
        Ok(Self {
            agent,
            media,
            negotiated: Mutex::new(None),
            sending: AtomicBool::new(true),
            error: Mutex::new(None),
            account: name,
        })
    }

    /// What was dialled: a number, a number with spaces in it, or a SIP URI.
    pub fn dial(&self, number: &str) {
        self.agent.dial(number);
    }

    /// Take the call that is ringing here.
    pub fn answer(&self) {
        self.agent.answer();
    }

    /// Put it down, at whatever stage it is at.
    pub fn hang_up(&self) {
        self.agent.hang_up();
        self.media.disconnect();
        // The next call on this line starts able to talk. A hold that was
        // never taken off before the call ended would otherwise leave the
        // transmitter shut for the call after it, which is a silent line with
        // nothing in the transcript to say why.
        self.sending.store(true, Ordering::Relaxed);
    }

    pub fn register(&self, on: bool) {
        self.agent.register(on);
    }

    /// Whether audio is flowing: the modem's idea of carrier, near enough.
    pub fn up(&self) -> bool {
        self.media.connected()
    }

    /// Everything the far end has sent, at the modem's rate.
    pub fn receive(&self, into: &mut Vec<f32>) {
        self.media.receive(into);
    }

    /// And what the modem has to say.
    ///
    /// Dropped on the floor while the description in force says the far end is
    /// not listening. The pacer goes on sending its packet every packet time,
    /// which during a hold is silence: that keeps the address binding through
    /// whatever router is in the way open, so that the audio can still arrive
    /// the moment the hold is taken off. Stopping the packets instead would
    /// win nothing and could cost the rest of the call.
    pub fn transmit(&self, samples: &[f32]) {
        if !self.sending.load(Ordering::Relaxed) {
            return;
        }
        self.media.transmit(samples);
    }

    /// Whether the description in force lets the modem's samples out. False
    /// during a hold, and while a far end has said `a=recvonly`.
    pub fn sending(&self) -> bool {
        self.sending.load(Ordering::Relaxed)
    }

    /// Service the agent and say what happened, in lines fit for a transcript.
    ///
    /// Must be called regularly -- once round the line loop is right -- because
    /// this is where an answered call is connected to its audio. The agent
    /// thread deliberately does not touch the media itself: one thread owns
    /// the socket and its timers, and a call being answered is the one moment
    /// the two sides have to meet.
    pub fn poll(&self) -> Vec<String> {
        let mut said = Vec::new();
        for event in self.agent.events() {
            match event {
                Event::Note(text) => said.push(text),
                Event::Registered { expires } => {
                    said.push(format!("sip: registered for {expires} s"));
                }
                Event::RegistrationFailed { code, reason } => {
                    let text = format!("sip: registration refused: {code} {reason}");
                    self.remember_error(&text);
                    said.push(text);
                }
                Event::Dialling { to } => said.push(format!("sip: calling {to}")),
                Event::Ringing => said.push("sip: ringing".to_owned()),
                Event::EarlyMedia(negotiated) => {
                    // Not connected to. Ringback and announcements are not
                    // what the modem is here for, and a far end that sends
                    // early media then answers with a different description
                    // would leave us sending to the wrong place.
                    said.push(format!(
                        "sip: early media offered ({} from {}:{}), ignored until answer",
                        negotiated.law.encoding_name(),
                        negotiated.address,
                        negotiated.port
                    ));
                }
                // A re-INVITE inside the call arrives here too, so this arm is
                // "the call changed" as much as it is "the call started".
                Event::Answered(negotiated) if negotiated.direction == Direction::Inactive => {
                    // Hold, by either convention: RFC 4566 6.7's a=inactive,
                    // or RFC 3264 8.4's c=IN IP4 0.0.0.0. The second is the
                    // one that bit: it parses as a perfectly good address, so
                    // it used to be treated as a fresh description -- the
                    // pacer was pointed at 0.0.0.0 and the jitter buffer and
                    // the outgoing queue were both restarted, which costs the
                    // modem its training over a hold it could have sat
                    // through. So nothing in the media path is touched here.
                    // Only the transmitter stops, because the far end has
                    // just said in as many words that it is not listening.
                    self.sending.store(false, Ordering::Relaxed);
                    said.push(
                        "sip: the far end has put the call on hold; the line is held open and nothing is going out"
                            .to_owned(),
                    );
                    if let Ok(mut slot) = self.negotiated.lock() {
                        *slot = Some(*negotiated);
                    }
                }
                Event::Answered(negotiated) => match self.start_media(&negotiated) {
                    Ok(remote) => {
                        said.push(format!(
                            "sip: answered; {} with {remote}, {} ms packets",
                            negotiated.law.encoding_name(),
                            negotiated.ptime_ms
                        ));
                        if negotiated.direction == Direction::ReceiveOnly {
                            // The far end will send and will not listen. A
                            // telephone call like that is an announcement; a
                            // modem call like that cannot train, because
                            // every handshake there is needs both directions.
                            // Said plainly here so that the forty seconds of
                            // failed training that follow are not a mystery.
                            said.push(
                                "sip: the far end will send and will not listen, so nothing is being transmitted; no modem handshake can complete on a one-way call"
                                    .to_owned(),
                            );
                        }
                        if let Ok(mut slot) = self.negotiated.lock() {
                            *slot = Some(*negotiated);
                        }
                    }
                    Err(e) => {
                        let text = format!("sip: the call came up but the audio could not: {e}");
                        self.remember_error(&text);
                        said.push(text);
                        self.hang_up();
                    }
                },
                Event::Incoming { from } => {
                    said.push(format!("sip: a call from {from}; ATA to answer"));
                }
                Event::Ended { reason } => {
                    self.media.disconnect();
                    self.sending.store(true, Ordering::Relaxed);
                    said.push(format!("sip: the call ended: {reason}"));
                    said.extend(self.closing_report());
                }
                Event::Failed { code, reason } => {
                    // Only if the failure was about the call the media is
                    // carrying. Not every failure is: a dial the agent
                    // refuses because a call is already up fails the *dial*,
                    // and tearing the audio down for it would cost a working
                    // call its path over something that never touched it.
                    // The agent is the one that knows -- if it still has a
                    // call, this was not about that call.
                    if !crate::ua::state::is_a_call(&self.agent.status().call) {
                        self.media.disconnect();
                        self.sending.store(true, Ordering::Relaxed);
                    }
                    let text = if code == 0 {
                        // Not a status code: nothing came back to carry one,
                        // because the INVITE never left this machine. A next
                        // hop that would not resolve is the way to get here,
                        // and calling that "the call failed" sends whoever
                        // reads it looking at the trunk for a fault that is
                        // on this side of the network.
                        format!("sip: the call never left this machine: {reason}")
                    } else {
                        // The far end's own answer; `ua::describe` has already
                        // put the code at the front of the reason.
                        format!("sip: the call failed: {reason}")
                    };
                    self.remember_error(&text);
                    said.push(text);
                }
            }
        }
        said
    }

    /// What the call cost in packets, said once at the end.
    ///
    /// The numbers that matter are not the totals but the losses: a call that
    /// failed with nothing lost and nothing concealed failed in the modem, and
    /// one with a hundred concealed octets failed in the network. Saying so at
    /// the end of every call saves working it out afterwards from a capture.
    fn closing_report(&self) -> Vec<String> {
        let stats = self.media.stats();
        if stats.packets_received == 0 && stats.packets_sent == 0 {
            return Vec::new();
        }
        let mut said = vec![format!(
            "sip: {} packets in, {} out",
            stats.packets_received, stats.packets_sent
        )];
        if stats.jitter.lost > 0 || stats.jitter.concealed > 0 {
            said.push(format!(
                "sip: {} packets lost, {} samples of silence handed to the modem",
                stats.jitter.lost, stats.jitter.concealed
            ));
        }
        if stats.jitter.reordered > 0 || stats.jitter.late > 0 {
            said.push(format!(
                "sip: {} packets put back in order, {} too late to use",
                stats.jitter.reordered, stats.jitter.late
            ));
        }
        if stats.underruns > 0 {
            // The harmful ones first and by name. The rest are the stretch
            // before the far end's audio had arrived, when the modem had not
            // been stepped and had nothing to say: silence into a gap that
            // was already silent.
            if stats.underruns_mid_call > 0 {
                said.push(format!(
                    "sip: {} packets of silence were sent into audio the modem was                      producing, out of {} sent in all",
                    stats.underruns_mid_call, stats.underruns
                ));
            } else {
                said.push(format!(
                    "sip: {} packets sent as silence before the modem had anything to                      say, which costs nothing",
                    stats.underruns
                ));
            }
        }
        if stats.cushioned > 0 {
            said.push(format!(
                "sip: {} samples of silence were added on purpose to rebuild the                  outgoing cushion",
                stats.cushioned
            ));
        }
        if stats.jitter.overflowed > 0 {
            said.push(format!(
                "sip: {} packets ({} samples) shed because the buffer was full, which means the far end's clock runs faster than ours",
                stats.jitter.overflowed, stats.jitter.overflowed_octets
            ));
        }
        // Everything below is audio that went missing for a reason that is
        // not the network's. Said separately and only when it happened,
        // because the whole use of the numbers above is that they are the
        // network's: a call blamed on a lost packet that was really thrown
        // away in here would be debugged in the wrong place for a week.
        if stats.off_codec > 0 {
            said.push(format!(
                "sip: {} packets were not the codec we agreed and were dropped rather than decoded -- dialled digits sent as events, comfort noise, or a transcoder in the path",
                stats.off_codec
            ));
        }
        if stats.flushed_out > 0 {
            said.push(format!(
                "sip: {} samples the modem produced never left this machine, thrown away when the media path was started or stopped",
                stats.flushed_out
            ));
        }
        if stats.jitter.abandoned > 0 {
            said.push(format!(
                "sip: {} samples were still waiting to be read when the call ended",
                stats.jitter.abandoned
            ));
        }
        if stats.jitter.resynced > 0 {
            said.push(format!(
                "sip: the far end restarted its stream {} time(s); the timeline stepped rather than filling the jump with silence",
                stats.jitter.resynced
            ));
        }
        if stats.latched > 0 {
            said.push(format!(
                "sip: the far end's audio came from somewhere other than the address it gave, {} time(s), and was followed there",
                stats.latched
            ));
        }
        if stats.strangers > 0 {
            said.push(format!(
                "sip: {} packet(s) arrived from somewhere other than the call's stream and were dropped rather than followed",
                stats.strangers
            ));
        }
        said
    }

    fn start_media(&self, negotiated: &Negotiated) -> Result<SocketAddr, String> {
        let remote = resolve_media(&negotiated.address, negotiated.port)?;
        self.media.connect(
            remote,
            negotiated.law,
            negotiated.payload_type,
            negotiated.ptime_ms,
        );
        // `Negotiated::direction` is ours, not theirs -- `sdp` has already
        // flipped it -- so receive-only here means the far end will not
        // listen, and inactive means neither end will. Either way our
        // transmitter has no business being on.
        self.sending.store(may_send(negotiated.direction), Ordering::Relaxed);
        Ok(remote)
    }

    fn remember_error(&self, text: &str) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(text.to_owned());
        }
    }

    /// What the last answered call agreed to.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.negotiated.lock().ok().and_then(|n| n.clone())
    }

    pub fn stats(&self) -> Stats {
        self.media.stats()
    }

    pub fn status(&self) -> Status {
        self.agent.status()
    }

    /// Everything the window shows about this line, in one read.
    pub fn progress(&self) -> Progress {
        let status = self.agent.status();
        let stats = self.media.stats();
        Progress {
            account: self.account.clone(),
            registered: status.registered,
            registration_left: status.registration_left,
            public: status.public,
            call: status.call,
            peer: status.peer,
            media: stats.connected,
            law: stats.connected.then(|| {
                stats
                    .law
                    .unwrap_or(crate::g711::Law::Mu.encoding_name())
                    .to_owned()
            }),
            packets_sent: stats.packets_sent,
            packets_received: stats.packets_received,
            lost: stats.jitter.lost,
            concealed: stats.jitter.concealed,
            underruns: stats.underruns,
            underruns_mid_call: stats.underruns_mid_call,
            cushioned: stats.cushioned,
            last_error: self.error.lock().ok().and_then(|e| e.clone()),
        }
    }
}

/// Whether a description in this direction lets us put anything on the line.
///
/// The direction is ours, [`crate::sdp::Direction::flipped`] having already
/// been applied, so this is a question about our own transmitter and not about
/// the far end's. Send-receive and send-only both send; receive-only and
/// inactive both do not, and a rig that transmits anyway is transmitting into
/// an ear that is not there -- on a hold, into whatever the far end switched
/// the path to, which after a transfer is somebody else's call.
fn may_send(direction: Direction) -> bool {
    match direction {
        Direction::SendReceive | Direction::SendOnly => true,
        Direction::ReceiveOnly | Direction::Inactive => false,
    }
}

/// Where to send RTP, out of what the far end wrote in its `c=` line.
///
/// Two ways to end up with a call that comes up and carries nothing, and this
/// is where both are caught.
///
/// The first is a name. `c=IN IP4 sbc.example.net` is legal SDP -- 4566 5.7
/// allows a host name -- and some Asterisk configurations emit one. This used
/// to be `format!("{address}:{port}").parse::<SocketAddr>()`, which fails on
/// anything that is not a literal, and the call was then hung up with "the
/// call came up but the audio could not", which reads as a far-end fault and
/// is not one. So a name is looked up. It is a blocking lookup on the thread
/// that polls the line, which is worth saying out loud, but it happens once
/// when a call is answered rather than per packet, and a call that cannot be
/// placed without it is worth waiting on.
///
/// The second is the address family. [`Media`] binds `0.0.0.0`, which is an
/// IPv4 socket, and a datagram sent from one of those to an IPv6 address fails
/// inside the pacer where nothing reads the error -- so the call would run for
/// its whole length with `packets_sent` stuck at zero and no reason given.
/// An IPv4 address is therefore preferred when a name resolves to both, and an
/// answer that leaves only IPv6 is refused here, in a sentence.
fn resolve_media(address: &str, port: u16) -> Result<SocketAddr, String> {
    // A literal is parsed as an address rather than as "host:port": an IPv6
    // literal in a c= line has no brackets round it, and `SocketAddr`'s parser
    // requires them, so the obvious spelling refuses every IPv6 far end before
    // it gets as far as deciding whether it could talk to one.
    if let Ok(ip) = address.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(_) => Ok(SocketAddr::new(ip, port)),
            IpAddr::V6(_) => Err(ipv4_only(address, None)),
        };
    }

    let resolved: Vec<SocketAddr> = (address, port)
        .to_socket_addrs()
        .map_err(|e| {
            format!("the far end's media address {address:?} is neither an address nor a name that resolves: {e}")
        })?
        .collect();
    if let Some(found) = resolved.iter().find(|a| a.is_ipv4()) {
        return Ok(*found);
    }
    match resolved.first() {
        Some(only) => Err(ipv4_only(address, Some(*only))),
        None => Err(format!("the far end's media address {address:?} resolved to nothing")),
    }
}

/// Said the same way whether the far end wrote an IPv6 literal or a name that
/// only has IPv6 behind it, because it is the same fault and the same cure.
fn ipv4_only(address: &str, resolved: Option<SocketAddr>) -> String {
    let what = match resolved {
        Some(found) => format!("the far end's media address {address} resolved only to {found}"),
        None => format!("the far end wants RTP at {address}"),
    };
    format!("{what}, which is IPv6, and the RTP socket here is bound to 0.0.0.0 -- IPv4 only")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary case, and the one the old spelling got right: a literal
    /// off a trunk's c= line.
    #[test]
    fn an_address_literal_is_taken_as_it_stands() {
        assert_eq!(
            resolve_media("203.0.113.11", 14884).unwrap(),
            SocketAddr::from(([203, 0, 113, 11], 14884))
        );
    }

    /// And the case that cost a call: a name where an address was expected.
    /// `localhost` rather than anything on the network, so the test asks the
    /// resolver a question it can answer without one.
    #[test]
    fn a_host_name_is_looked_up_rather_than_refused() {
        let found = resolve_media("localhost", 40000).expect("localhost did not resolve");
        assert!(found.is_ipv4(), "the IPv4 answer was not preferred: {found}");
        assert!(found.ip().is_loopback(), "that is not localhost: {found}");
        assert_eq!(found.port(), 40000);
    }

    /// A name that cannot resolve is a sentence, not a panic and not a
    /// silently dead audio path. `.invalid` is reserved by RFC 2606 exactly so
    /// that this can be relied on.
    #[test]
    fn a_name_that_will_not_resolve_says_so_and_names_itself() {
        let refused = resolve_media("no-such-trunk.invalid", 5004).unwrap_err();
        assert!(refused.contains("no-such-trunk.invalid"), "{refused}");
    }

    /// An IPv6 far end is refused in words rather than accepted and then sent
    /// to from a socket that cannot reach it. Note the spelling: a c= line
    /// carries a bare IPv6 address with no brackets round it, which
    /// "{address}:{port}" could not have parsed either -- but it would have
    /// blamed the far end for writing it instead of naming the end that
    /// cannot do it.
    #[test]
    fn an_ipv6_far_end_is_refused_for_the_reason_it_is_refused() {
        let refused = resolve_media("2001:db8::1", 9000).unwrap_err();
        assert!(refused.contains("IPv6"), "{refused}");
        assert!(refused.contains("0.0.0.0"), "{refused}");
    }

    /// Hold does not become an address. This is the whole of the second fault:
    /// 0.0.0.0 parses, so without the direction being looked at first the
    /// pacer would be pointed at it and the media restarted.
    #[test]
    fn the_hold_address_would_otherwise_parse_perfectly_well() {
        assert!(resolve_media("0.0.0.0", 19010).is_ok());
        let held = concat!(
            "v=0\r\n",
            "o=- 17 18 IN IP4 0.0.0.0\r\n",
            "s=-\r\n",
            "c=IN IP4 0.0.0.0\r\n",
            "t=0 0\r\n",
            "m=audio 19010 RTP/AVP 0\r\n",
            "a=sendrecv\r\n",
        );
        let sdp = crate::sdp::Sdp::parse(held).unwrap();
        let agreed = crate::sdp::read_answer(&sdp, &[crate::g711::Law::Mu]).unwrap();
        // Which is why the direction is what decides, and not the address.
        assert_eq!(agreed.direction, Direction::Inactive);
        assert!(!may_send(agreed.direction));
    }

    /// Ours, not theirs. A far end's `a=sendonly` has already been flipped by
    /// the time it reaches here, so the two that stop the transmitter are the
    /// two where nobody at the other end is listening.
    #[test]
    fn only_the_directions_with_an_ear_at_the_far_end_transmit() {
        assert!(may_send(Direction::SendReceive));
        assert!(may_send(Direction::SendOnly));
        assert!(!may_send(Direction::ReceiveOnly));
        assert!(!may_send(Direction::Inactive));
    }
}
