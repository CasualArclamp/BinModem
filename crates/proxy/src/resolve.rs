//! Names to addresses, for connections that go straight to the internet.
//!
//! This is the one step that does not cross the call. Looking a name up over
//! the link would take a DNS client, and this program has none: nothing in it
//! is written from memory of a protocol, and the documents for one are not
//! here. So the machine's own resolver is asked, over whatever connection the
//! machine already has, and only the address comes back to be used. The
//! connection to that address goes over the call.
//!
//! Which has a cost worth knowing: the answer is the one the machine's own
//! network was given, and a name served from many places may be pointed
//! somewhere near that network rather than near the provider. It is still an
//! address on the internet, and the provider's router carries it there.
//!
//! IPv4 only, because the stack under the call is. A lookup blocks for as long
//! as the resolver takes, so it runs on a thread of its own and the answer is
//! collected later.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use tcp::Endpoint;

/// How long an answer is used before the resolver is asked again.
///
/// A browser opens several connections to one host in a few seconds, and
/// asking each time would be a lookup per connection for nothing.
const KEEP_FOUND: Duration = Duration::from_secs(600);
/// And how long "no such name" is believed, which is not long: the name
/// might have been mistyped once and fixed.
const KEEP_FAILED: Duration = Duration::from_secs(30);

/// What became of a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Pending,
    Found(Endpoint),
    Failed(String),
}

/// A lookup running on its own thread, and when it was started.
type Running = (Receiver<Result<[u8; 4], String>>, Instant);

#[derive(Debug)]
enum Known {
    Found([u8; 4], Instant),
    Failed(String, Instant),
}

/// Lookups, running and remembered.
#[derive(Debug, Default)]
pub struct Resolver {
    known: HashMap<String, Known>,
    asked: HashMap<String, Running>,
    /// Lookups made, and how many found nothing.
    pub lookups: u64,
    pub failures: u64,
    /// Things worth a line in the transcript.
    log: Vec<String>,
}

/// A host and port out of an authority (RFC 9110 4.2.1's `uri-host [":"
/// port]`, which a proxy request always carries with a port by now).
fn split(authority: &str) -> Result<(&str, u16), String> {
    if authority.starts_with('[') {
        return Err("it is an IPv6 address, and only IPv4 crosses this link".to_owned());
    }
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| format!("{authority} has no port"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("{authority} has no port that is a number"))?;
    if host.is_empty() {
        return Err(format!("{authority} has no host"));
    }
    Ok((host, port))
}

/// Addresses that are this machine or nowhere, which a router has no business
/// being sent.
fn not_on_the_internet(address: [u8; 4]) -> Option<&'static str> {
    let ip = Ipv4Addr::from(address);
    if ip.is_loopback() {
        Some("that is this machine, not somewhere on the internet")
    } else if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        Some("that is not an address a connection can go to")
    } else {
        None
    }
}

impl Resolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// Start finding where `authority` is, if it is not already known or
    /// being found.
    pub fn ask(&mut self, authority: &str) {
        let Ok((host, port)) = split(authority) else {
            return;
        };
        if host.parse::<Ipv4Addr>().is_ok() || self.asked.contains_key(host) {
            return;
        }
        let fresh = match self.known.get(host) {
            Some(Known::Found(_, at)) => at.elapsed() < KEEP_FOUND,
            Some(Known::Failed(_, at)) => at.elapsed() < KEEP_FAILED,
            None => false,
        };
        if fresh {
            return;
        }
        self.lookups += 1;
        let name = host.to_owned();
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let _ = sender.send(look_up(&name, port));
        });
        self.asked.insert(host.to_owned(), (receiver, Instant::now()));
    }

    /// Where `authority` is, if that is known yet.
    pub fn answer(&mut self, authority: &str) -> Answer {
        let (host, port) = match split(authority) {
            Ok(split) => split,
            Err(why) => return Answer::Failed(why),
        };
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return self.checked(ip.octets(), port);
        }
        if let Some((receiver, since)) = self.asked.get(host) {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return Answer::Pending,
                Err(TryRecvError::Disconnected) => Err("the lookup went away".to_owned()),
            };
            let took = since.elapsed().as_millis();
            self.asked.remove(host);
            match result {
                Ok(address) => {
                    self.log.push(format!(
                        "proxy: {host} is {} ({took} ms to look up)",
                        Ipv4Addr::from(address)
                    ));
                    self.known.insert(host.to_owned(), Known::Found(address, Instant::now()));
                }
                Err(why) => {
                    self.failures += 1;
                    self.log.push(format!("proxy: {host} could not be looked up: {why}"));
                    self.known.insert(host.to_owned(), Known::Failed(why, Instant::now()));
                }
            }
        }
        match self.known.get(host) {
            Some(Known::Found(address, _)) => self.checked(*address, port),
            Some(Known::Failed(why, _)) => Answer::Failed(why.clone()),
            // Never asked, or asked long enough ago to have been forgotten.
            None => {
                self.ask(authority);
                Answer::Pending
            }
        }
    }

    fn checked(&self, address: [u8; 4], port: u16) -> Answer {
        match not_on_the_internet(address) {
            Some(why) => Answer::Failed(why.to_owned()),
            None => Answer::Found(Endpoint::new(address, port)),
        }
    }
}

/// The machine's resolver, and the first IPv4 address it gives.
fn look_up(host: &str, port: u16) -> Result<[u8; 4], String> {
    let found: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .collect();
    found
        .iter()
        .find_map(|a| match a {
            SocketAddr::V4(v4) => Some(v4.ip().octets()),
            SocketAddr::V6(_) => None,
        })
        .ok_or_else(|| {
            if found.is_empty() {
                "no address".to_owned()
            } else {
                "only IPv6 addresses, and only IPv4 crosses this link".to_owned()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An address needs no lookup at all.
    #[test]
    fn an_address_is_its_own_answer() {
        let mut r = Resolver::new();
        r.ask("93.184.216.34:443");
        assert_eq!(r.answer("93.184.216.34:443"), Answer::Found(Endpoint::new([93, 184, 216, 34], 443)));
        assert_eq!(r.lookups, 0, "an address was looked up");
    }

    #[test]
    fn what_cannot_cross_the_link_is_refused_with_a_reason() {
        let mut r = Resolver::new();
        for (authority, says) in [
            ("[2001:db8::1]:443", "IPv6"),
            ("127.0.0.1:80", "this machine"),
            ("example.invalid", "no port"),
            ("example.invalid:http", "number"),
            ("0.0.0.0:80", "not an address"),
        ] {
            match r.answer(authority) {
                Answer::Failed(why) => assert!(why.contains(says), "{authority}: {why}"),
                other => panic!("{authority}: {other:?}"),
            }
        }
    }

    /// A name is looked up once, off the thread asking, and the answer is
    /// kept for the next connection to the same host.
    #[test]
    fn a_name_is_looked_up_once_and_remembered() {
        let mut r = Resolver::new();
        r.ask("localhost:80");
        let started = Instant::now();
        let answer = loop {
            match r.answer("localhost:80") {
                Answer::Pending if started.elapsed() < Duration::from_secs(20) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => break other,
            }
        };
        // localhost is this machine, which is not somewhere to be sent.
        assert!(matches!(answer, Answer::Failed(ref why) if why.contains("this machine")), "{answer:?}");
        assert_eq!(r.lookups, 1);
        // Asked again, on another port: no second lookup.
        r.ask("localhost:443");
        assert!(matches!(r.answer("localhost:443"), Answer::Failed(_)));
        assert_eq!(r.lookups, 1, "the answer was not kept");
    }
}
