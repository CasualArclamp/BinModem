//! The link over the call: PPP on top of the modem, and a ping on top of that.
//!
//! What the modem hands up is octets. [`ppp`] turns those into frames and the
//! frames into an agreement about what the two ends are and what they are
//! called, after which an IP datagram can cross. This is where the two meet:
//! everything the modem gives up goes into the link, everything the link
//! produces goes back down as if it had been typed, and the window is told
//! what is happening.
//!
//! While it runs it owns the byte stream, the same way a file transfer does.
//! A PPP frame is not something a person wants on their screen and a keystroke
//! in the middle of one is a corrupt frame, so the terminal is put aside until
//! the link is dropped. That is exactly what happened on a real dial-up
//! account: a menu, `ppp` typed at it, and then the terminal was no longer a
//! terminal.

use modem::Role;
use ppp::link::{Link, Phase};
use ppp::ping::{Event, Pinger, Stats};
use telemetry::{Direction, Publisher};

/// The address the end that hands them out keeps for itself.
///
/// A private range (RFC 1918) because these two ends are the whole internet as
/// far as this link is concerned, and picking anything else would be squatting
/// on somebody's real address.
pub const SERVER_ADDRESS: [u8; 4] = [10, 0, 0, 1];
/// And the one it gives the caller.
pub const CLIENT_ADDRESS: [u8; 4] = [10, 0, 0, 2];

/// What the window has asked the link to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Bring PPP up on the call that is already there.
    Start,
    /// Put it down again and give the terminal back.
    Stop,
    /// One echo, now.
    PingOnce,
    /// Keep sending them, or stop.
    PingRepeatedly(bool),
}

/// What the link is doing, for the window to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    pub running: bool,
    /// Where it has got to, in words.
    pub phase: String,
    /// Whether IP can cross right now.
    pub up: bool,
    /// Which end of the negotiation this is: the one handing out addresses or
    /// the one being given one.
    pub serving: bool,
    pub local: String,
    pub remote: String,
    pub pinging: bool,
    pub stats: Stats,
    /// Echoes still waiting for an answer.
    pub in_flight: usize,
    /// Octets that have crossed as PPP, which is not the same as the call's
    /// own count: this starts when the link does.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

fn dotted(address: [u8; 4]) -> String {
    let [a, b, c, d] = address;
    format!("{a}.{b}.{c}.{d}")
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        // RFC 1661 3.2's own names, which are worth keeping: somebody reading
        // this next to the document should not have to translate.
        Phase::Dead => "dead",
        Phase::Establish => "establishing",
        Phase::Network => "network",
        Phase::Terminate => "terminating",
    }
}

/// One PPP link running over the call.
#[derive(Debug)]
pub struct Networking {
    link: Link,
    pinger: Pinger,
    /// Whether this end is the one with addresses to give.
    serving: bool,
    /// Set once the network phase has been reached, so it is announced once
    /// rather than every round.
    announced: bool,
    rx_bytes: u64,
    tx_bytes: u64,
}

impl Networking {
    /// Bring a link up over a call in `role`.
    ///
    /// The answering end hands out the addresses and the calling end asks for
    /// one, which is what dialling a provider was: the machine that answered
    /// knew what everything was called and the machine that called did not.
    /// Nothing in RFC 1332 says it has to be that way round -- 3.3 makes it
    /// whichever end has an address to give -- but a modem call already has an
    /// end that answered, so there is no need to ask anybody which is which.
    pub fn start(role: Role, tx: &Publisher) -> Self {
        let serving = role == Role::Answering;
        let (local, remote) = if serving {
            (SERVER_ADDRESS, CLIENT_ADDRESS)
        } else {
            // Zeroes both ways: RFC 1332 3.3 makes that the question rather
            // than an address, and the answer comes back in a Configure-Nak.
            (ppp::ipcp::UNSPECIFIED, ppp::ipcp::UNSPECIFIED)
        };
        tx.log(
            Direction::Note,
            if serving {
                format!(
                    "ppp: handing out {} and keeping {}",
                    dotted(remote),
                    dotted(local)
                )
            } else {
                "ppp: asking the far end what to call ourselves".to_owned()
            },
        );
        let mut link = Link::new(local, remote);
        link.open();
        Self {
            link,
            // The identifier only has to tell this end's echoes from the far
            // end's, and the two ends of one call are never the same role.
            pinger: Pinger::new(if serving { 0x0b17 } else { 0x0b18 }),
            serving,
            announced: false,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    /// Everything the modem handed up.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.rx_bytes += bytes.len() as u64;
        self.link.feed(bytes);
    }

    /// Let `ms` pass, and give back what should go down the line.
    pub fn step(&mut self, ms: u32, tx: &Publisher) -> Vec<u8> {
        self.link.tick(ms);
        // Anything the far end sent that was not an answer to one of ours has
        // already been replied to inside the link; there is nothing above IP
        // here to hand it to.
        let _ = self.pinger.poll(&mut self.link, ms);
        self.report(tx);
        let out = self.link.take_line();
        self.tx_bytes += out.len() as u64;
        out
    }

    /// Put the link down. What comes back is the last of it: RFC 1661 3.7's
    /// Terminate-Request, which the far end deserves rather than silence.
    pub fn stop(&mut self, tx: &Publisher) -> Vec<u8> {
        self.link.close();
        tx.log(Direction::Note, "ppp: down");
        self.link.take_line()
    }

    pub fn ping_once(&mut self, tx: &Publisher) {
        if !self.pinger.ping_once(&mut self.link) {
            tx.log(Direction::Note, "ppp: nowhere to send it yet");
        }
    }

    pub fn ping_repeatedly(&mut self, on: bool) {
        if on {
            self.pinger.start();
        } else {
            self.pinger.stop();
        }
    }

    pub fn view(&self) -> View {
        let (local, remote) = self.link.addresses();
        View {
            running: true,
            phase: phase_name(self.link.phase()).to_owned(),
            up: self.link.up(),
            serving: self.serving,
            local: dotted(local),
            remote: dotted(remote),
            pinging: self.pinger.running(),
            stats: self.pinger.stats,
            in_flight: self.pinger.in_flight(),
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
        }
    }

    /// Say what has happened since last time.
    fn report(&mut self, tx: &Publisher) {
        if self.link.up() && !self.announced {
            self.announced = true;
            let (local, remote) = self.link.addresses();
            tx.log(
                Direction::Note,
                format!("ppp: up, {} talking to {}", dotted(local), dotted(remote)),
            );
        }
        for event in self.pinger.take_events() {
            match event {
                Event::Reply { sequence, round_trip_ms } => tx.log(
                    Direction::Note,
                    format!(
                        "ping: {} octets from {}, seq {sequence}, {round_trip_ms} ms",
                        self.pinger.payload.len(),
                        self.view().remote
                    ),
                ),
                Event::Lost(sequence) => {
                    tx.log(Direction::Note, format!("ping: seq {sequence} never came back"))
                }
                Event::Answered => {
                    tx.log(Direction::Note, "ping: answered one from the far end")
                }
                // Not logged: at one a second it would be half the transcript,
                // and a reply says everything a request would have.
                Event::Sent(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_reads_the_way_it_is_written() {
        assert_eq!(dotted([10, 0, 0, 1]), "10.0.0.1");
        assert_eq!(dotted([255, 255, 255, 0]), "255.255.255.0");
        assert_eq!(dotted(ppp::ipcp::UNSPECIFIED), "0.0.0.0");
    }

    /// The end that answered the call is the end that knows the addresses.
    #[test]
    fn the_answering_end_is_the_one_with_addresses_to_give() {
        let (tx, _rx) = telemetry::channel(64, 32, 8_000.0);
        let answering = Networking::start(Role::Answering, &tx);
        assert!(answering.serving);
        assert_eq!(answering.view().local, "10.0.0.1");

        let calling = Networking::start(Role::Calling, &tx);
        assert!(!calling.serving);
        assert_eq!(calling.view().local, "0.0.0.0", "it made an address up");
    }

    /// Two of these, wired to each other the way a call wires them, come up
    /// and carry an echo. The whole thing without a sound card in it.
    #[test]
    fn two_ends_of_a_call_come_up_and_ping() {
        let (tx, _rx) = telemetry::channel(64, 32, 8_000.0);
        let mut answering = Networking::start(Role::Answering, &tx);
        let mut calling = Networking::start(Role::Calling, &tx);

        let mut came_up = None;
        for ms in 0..30_000u32 {
            let from_answering = answering.step(1, &tx);
            if !from_answering.is_empty() {
                calling.feed(&from_answering);
            }
            let from_calling = calling.step(1, &tx);
            if !from_calling.is_empty() {
                answering.feed(&from_calling);
            }
            if answering.view().up && calling.view().up {
                came_up = Some(ms);
                break;
            }
        }
        let came_up = came_up.expect("the link never came up");

        // The calling end was told what it is called.
        assert_eq!(calling.view().local, "10.0.0.2");
        assert_eq!(calling.view().remote, "10.0.0.1");
        assert_eq!(answering.view().remote, "10.0.0.2");

        calling.ping_once(&tx);
        for _ in 0..100 {
            let out = calling.step(1, &tx);
            if !out.is_empty() {
                answering.feed(&out);
            }
            let back = answering.step(1, &tx);
            if !back.is_empty() {
                calling.feed(&back);
            }
        }
        let view = calling.view();
        assert_eq!(view.stats.sent, 1);
        assert_eq!(view.stats.received, 1, "the echo did not come back");
        assert_eq!(view.stats.lost, 0);
        assert!(view.tx_bytes > 0 && view.rx_bytes > 0);
        println!("  up in {came_up} ms, round trip {} ms", view.stats.last_ms);
    }
}
