//! The IP Control Protocol (RFC 1332): agreeing what the two ends are called.
//!
//! LCP settles the link and this settles the addresses on it, using the same
//! automaton and the same packets with a different option in them.
//!
//! There is one asymmetry worth knowing about, and 3.3 states it plainly: an
//! IP-Address of all zeroes is "a request that the peer provide the
//! information". So the end that knows -- a server -- names its own address
//! and Naks the other's zeroes with one to use; the end that does not asks
//! with zeroes and takes what comes back. Nothing marks either end as the
//! server: it is whichever one has an address to give.

use crate::control::ConfigOption;
use crate::lcp::Review;
use crate::session::Protocol;

pub mod option {
    /// 3.1, superseded by IP-Address and refused here.
    pub const IP_ADDRESSES: u8 = 1;
    /// 3.2, Van Jacobson header compression, which is not implemented.
    pub const IP_COMPRESSION: u8 = 2;
    /// 3.3, the one that matters.
    pub const IP_ADDRESS: u8 = 3;
}

/// 3.3: "By default, no IP address is assigned", and all four octets zero is
/// how an end says it has none and would like one.
pub const UNSPECIFIED: [u8; 4] = [0, 0, 0, 0];

/// One end's view of the addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Addresses {
    /// What this end will call itself. Zeroes mean it is asking to be told.
    pub local: [u8; 4],
    /// What this end believes the far end is called, and will offer if the far
    /// end asks. Zeroes mean it has nothing to offer.
    pub remote: [u8; 4],
}

/// IPCP as one end sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ipcp {
    pub addresses: Addresses,
    /// Set once the far end has agreed to this end's address, so the layer
    /// above knows the one it has is settled rather than merely wanted.
    pub settled: bool,
}

impl Ipcp {
    pub fn new(local: [u8; 4], remote: [u8; 4]) -> Self {
        Self { addresses: Addresses { local, remote }, settled: false }
    }

    /// The address this end ended up with.
    pub fn local(&self) -> [u8; 4] {
        self.addresses.local
    }

    /// And the one at the other end of the link.
    pub fn remote(&self) -> [u8; 4] {
        self.addresses.remote
    }
}

fn address(value: &[u8]) -> Option<[u8; 4]> {
    value.try_into().ok()
}

impl Protocol for Ipcp {
    fn number(&self) -> u16 {
        crate::protocol::IPCP
    }

    fn request(&self) -> Vec<ConfigOption> {
        // Always sent, even when it is zeroes: 3.3 makes the zeroes the
        // question, and an end that leaves the option out has not asked
        // anything and will not be told.
        vec![ConfigOption {
            kind: option::IP_ADDRESS,
            value: self.addresses.local.to_vec(),
        }]
    }

    fn review(&mut self, options: &[ConfigOption]) -> Review {
        let mut reject = Vec::new();
        let mut nak = Vec::new();
        let mut theirs = None;

        for option in options {
            match (option.kind, address(&option.value)) {
                (option::IP_ADDRESS, Some(addr)) => {
                    if addr == UNSPECIFIED {
                        // 3.3: "The peer can provide this information by NAKing
                        // the option, and returning a valid IP-address." Only
                        // if there is one to return; an end with nothing to
                        // give has to let the other keep asking.
                        if self.addresses.remote != UNSPECIFIED {
                            nak.push(ConfigOption {
                                kind: option::IP_ADDRESS,
                                value: self.addresses.remote.to_vec(),
                            });
                        } else {
                            theirs = Some(addr);
                        }
                    } else if self.addresses.remote == UNSPECIFIED
                        || self.addresses.remote == addr
                    {
                        // Either this end had no opinion or the far end has
                        // named what this end was going to offer anyway.
                        theirs = Some(addr);
                    } else {
                        // It named something else. 3.3 has the Nak carry what
                        // would be acceptable.
                        nak.push(ConfigOption {
                            kind: option::IP_ADDRESS,
                            value: self.addresses.remote.to_vec(),
                        });
                    }
                }
                // 3.1 is the old form of the same thing and 3.2 is header
                // compression this does not do. Both are refused rather than
                // haggled over, which leaves the peer using neither.
                (option::IP_ADDRESSES, _) | (option::IP_COMPRESSION, _) => {
                    reject.push(option.clone());
                }
                _ => reject.push(option.clone()),
            }
        }

        if !reject.is_empty() {
            return Review::Reject(reject);
        }
        if !nak.is_empty() {
            return Review::Nak(nak);
        }
        if let Some(addr) = theirs
            && addr != UNSPECIFIED
        {
            self.addresses.remote = addr;
        }
        Review::Ack
    }

    fn acked(&mut self, options: &[ConfigOption]) {
        // 5.2 has the acknowledgement echo the request, so this is the address
        // this end asked for coming back agreed to.
        for option in options {
            if option.kind == option::IP_ADDRESS
                && let Some(addr) = address(&option.value)
            {
                self.addresses.local = addr;
            }
        }
        self.settled = self.addresses.local != UNSPECIFIED;
    }

    fn naked(&mut self, options: &[ConfigOption]) {
        // 3.3: the Nak carries the address to use. This is how an end that
        // asked with zeroes is told what it is called.
        for option in options {
            if option.kind == option::IP_ADDRESS
                && let Some(addr) = address(&option.value)
            {
                self.addresses.local = addr;
            }
        }
    }

    fn rejected(&mut self, _options: &[ConfigOption]) {
        // There is only one option here and a link without it carries no IP,
        // so there is nothing to stop asking for. The negotiation will fail on
        // its own, which is the honest outcome.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary case: a server that knows both addresses and a client that
    /// knows neither.
    #[test]
    fn a_client_with_no_address_is_given_one() {
        let mut server = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let mut client = Ipcp::new(UNSPECIFIED, UNSPECIFIED);

        // The client asks with zeroes and is told.
        let review = server.review(&client.request());
        assert_eq!(
            review,
            Review::Nak(vec![ConfigOption {
                kind: option::IP_ADDRESS,
                value: vec![10, 0, 0, 2],
            }])
        );
        let Review::Nak(counter) = review else { unreachable!() };
        client.naked(&counter);
        assert_eq!(client.local(), [10, 0, 0, 2]);

        // It asks again with what it was given, and that is agreed to.
        assert_eq!(server.review(&client.request()), Review::Ack);
        assert_eq!(server.remote(), [10, 0, 0, 2]);

        // And the client agrees to the server's own address.
        assert_eq!(client.review(&server.request()), Review::Ack);
        assert_eq!(client.remote(), [10, 0, 0, 1]);
    }

    /// Two ends that both already know who they are, which is what happens
    /// between two of these.
    #[test]
    fn two_ends_that_agree_settle_at_once() {
        let mut a = Ipcp::new([192, 168, 1, 1], [192, 168, 1, 2]);
        let mut b = Ipcp::new([192, 168, 1, 2], [192, 168, 1, 1]);
        assert_eq!(a.review(&b.request()), Review::Ack);
        assert_eq!(b.review(&a.request()), Review::Ack);
        assert_eq!(a.remote(), [192, 168, 1, 2]);
        assert_eq!(b.remote(), [192, 168, 1, 1]);
    }

    /// A far end that names something this end was not expecting is corrected.
    #[test]
    fn an_address_this_end_will_not_have_is_answered_with_one_it_will() {
        let mut server = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let squatter = vec![ConfigOption {
            kind: option::IP_ADDRESS,
            value: vec![10, 0, 0, 99],
        }];
        assert_eq!(
            server.review(&squatter),
            Review::Nak(vec![ConfigOption {
                kind: option::IP_ADDRESS,
                value: vec![10, 0, 0, 2],
            }])
        );
    }

    /// An end with nothing to give lets the asking end keep asking rather than
    /// pretending to answer.
    #[test]
    fn an_end_with_no_address_to_offer_does_not_invent_one() {
        let mut nobody = Ipcp::new([10, 0, 0, 1], UNSPECIFIED);
        let asking = vec![ConfigOption {
            kind: option::IP_ADDRESS,
            value: UNSPECIFIED.to_vec(),
        }];
        assert_eq!(nobody.review(&asking), Review::Ack);
        assert_eq!(nobody.remote(), UNSPECIFIED, "it made an address up");
    }

    /// 3.2 is header compression this does not implement, and 3.1 the
    /// superseded form of 3.3. Both refused.
    #[test]
    fn the_options_this_does_not_do_are_refused_outright() {
        let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let asking = vec![
            ConfigOption { kind: option::IP_COMPRESSION, value: vec![0x00, 0x2d, 0x0f, 0x01] },
            ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0, 0, 2] },
        ];
        match ipcp.review(&asking) {
            Review::Reject(o) => {
                assert_eq!(o.len(), 1);
                assert_eq!(o[0].kind, option::IP_COMPRESSION);
            }
            other => panic!("expected a reject, got {other:?}"),
        }
    }

    /// An address of the wrong length is not an address.
    #[test]
    fn a_malformed_address_is_rejected() {
        let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let asking = vec![ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0] }];
        assert!(matches!(ipcp.review(&asking), Review::Reject(_)));
    }

    /// The request always carries the option, even empty: the zeroes are the
    /// question.
    #[test]
    fn an_end_with_no_address_still_asks() {
        let ipcp = Ipcp::new(UNSPECIFIED, UNSPECIFIED);
        let request = ipcp.request();
        assert_eq!(request.len(), 1);
        assert_eq!(request[0].kind, option::IP_ADDRESS);
        assert_eq!(request[0].value, UNSPECIFIED.to_vec());
    }
}
