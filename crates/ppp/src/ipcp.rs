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
use crate::vj;

pub mod option {
    /// 3.1, superseded by IP-Address and refused here.
    pub const IP_ADDRESSES: u8 = 1;
    /// 3.2, header compression.
    pub const IP_COMPRESSION: u8 = 2;
    /// 3.3, the one that matters.
    pub const IP_ADDRESS: u8 = 3;
}

/// 3.3: "By default, no IP address is assigned", and all four octets zero is
/// how an end says it has none and would like one.
pub const UNSPECIFIED: [u8; 4] = [0, 0, 0, 0];

/// 4: the IP-Compression-Protocol value for Van Jacobson, and the only one
/// this end knows. Anything else named in the option is refused.
pub const VAN_JACOBSON: u16 = 0x002d;

/// Which way round a compression agreement runs.
///
/// 4: "The IP-Compression-Protocol Configuration Option is used to indicate
/// the ability to receive compressed packets. Each end of the link must
/// separately request this option if bi-directional compression is desired."
///
/// So the option in this end's own request settles what this end will have to
/// decompress, and the one in the far end's request settles what this end may
/// compress. They are negotiated separately and need not match, which is why
/// they are kept apart here rather than as one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Compression {
    /// Agreed by the far end acknowledging this end's request: it will send
    /// compressed, so this end must be ready to read it.
    pub receiving: Option<vj::Params>,
    /// Agreed by this end acknowledging the far end's request: it can read
    /// compressed, so this end may send it.
    pub sending: Option<vj::Params>,
}

fn compression_option(params: vj::Params) -> ConfigOption {
    // 4.1: two octets of protocol, then Max-Slot-Id and Comp-Slot-Id, for a
    // Length of six counting the type and the length itself.
    ConfigOption {
        kind: option::IP_COMPRESSION,
        value: vec![
            (VAN_JACOBSON >> 8) as u8,
            VAN_JACOBSON as u8,
            params.max_slot,
            u8::from(params.compress_slot),
        ],
    }
}

/// Read one back, if it names Van Jacobson and is the right length.
fn compression_params(value: &[u8]) -> Option<vj::Params> {
    let [hi, lo, max_slot, comp_slot] = *value else {
        return None;
    };
    if u16::from_be_bytes([hi, lo]) != VAN_JACOBSON {
        return None;
    }
    // 4.1 gives Comp-Slot-Id only two meanings, 0 and 1. Anything else is a
    // far end saying something this end cannot act on.
    let compress_slot = match comp_slot {
        0 => false,
        1 => true,
        _ => return None,
    };
    Some(vj::Params { max_slot, compress_slot })
}

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
    /// Whether this end does header compression at all. Off refuses the far
    /// end's option as well as asking for nothing, because a setting called
    /// off should be off; the two directions are otherwise independent.
    pub header_compression: bool,
    /// What this end asks to receive compressed, if it asks at all. Cleared
    /// when the far end rejects the option -- which says the far end will not
    /// send compressed, and says nothing about whether it can read it.
    pub offering: Option<vj::Params>,
    /// What the two ends settled on, in each direction.
    pub compression: Compression,
}

impl Ipcp {
    pub fn new(local: [u8; 4], remote: [u8; 4]) -> Self {
        Self {
            addresses: Addresses { local, remote },
            settled: false,
            header_compression: true,
            // Appendix A: "The following Configurations Options are
            // recommended: IP-Compression-Protocol -- with at least 4 slots,
            // usually 16 slots."
            offering: Some(vj::Params::DEFAULT),
            compression: Compression::default(),
        }
    }

    /// Do not ask for header compression at all.
    pub fn without_header_compression(mut self) -> Self {
        self.header_compression = false;
        self.offering = None;
        self
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
        let mut out = vec![ConfigOption {
            kind: option::IP_ADDRESS,
            value: self.addresses.local.to_vec(),
        }];
        if let Some(params) = self.offering {
            out.push(compression_option(params));
        }
        out
    }

    fn review(&mut self, options: &[ConfigOption]) -> Review {
        let mut reject = Vec::new();
        let mut nak = Vec::new();
        let mut theirs = None;
        let mut agreed_sending = None;

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
                (option::IP_COMPRESSION, _) if !self.header_compression => {
                    reject.push(option.clone());
                }
                (option::IP_COMPRESSION, _) => {
                    // The far end saying what it can read. Agreeing means this
                    // end may compress towards it, on the far end's terms:
                    // its slot count, because they are its slots.
                    match compression_params(&option.value) {
                        Some(params) => agreed_sending = Some(params),
                        // Some other compression protocol, or a malformed
                        // option. 5.4's Configure-Reject is how an end says it
                        // will not discuss this at all, which is the truth.
                        None => reject.push(option.clone()),
                    }
                }
                // 3.1 is the old form of IP-Address, refused rather than
                // haggled over so the peer uses 3.3 instead.
                (option::IP_ADDRESSES, _) => reject.push(option.clone()),
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
        // Only on the acknowledgement: an option this end answered with a Nak
        // or a Reject has not been agreed to, and acting on it before the peer
        // has asked again would compress towards an end that is not reading it.
        self.compression.sending = agreed_sending;
        Review::Ack
    }

    fn acked(&mut self, options: &[ConfigOption]) {
        // 5.2 has the acknowledgement echo the request, so this is the address
        // this end asked for coming back agreed to.
        let mut receiving = None;
        for option in options {
            match option.kind {
                option::IP_ADDRESS => {
                    if let Some(addr) = address(&option.value) {
                        self.addresses.local = addr;
                    }
                }
                option::IP_COMPRESSION => receiving = compression_params(&option.value),
                _ => {}
            }
        }
        self.settled = self.addresses.local != UNSPECIFIED;
        self.compression.receiving = receiving;
    }

    fn naked(&mut self, options: &[ConfigOption]) {
        // 3.3: the Nak carries the address to use. This is how an end that
        // asked with zeroes is told what it is called.
        for option in options {
            match option.kind {
                option::IP_ADDRESS => {
                    if let Some(addr) = address(&option.value) {
                        self.addresses.local = addr;
                    }
                }
                // 5.3: what comes back is what the peer would accept, so this
                // end asks for that instead. A far end with fewer slots than
                // this one offered says so here.
                // It may counter with something this end does not do, in
                // which case there is nothing to counter back with and the
                // asking stops.
                option::IP_COMPRESSION => self.offering = compression_params(&option.value),
                _ => {}
            }
        }
    }

    fn rejected(&mut self, options: &[ConfigOption]) {
        // 5.4: stop asking. A far end that will not do header compression is
        // an ordinary far end, and the link carries on without it -- unlike
        // the address, which a link cannot do without and so is not something
        // to stop asking for. That negotiation fails on its own, which is the
        // honest outcome.
        for option in options {
            if option.kind == option::IP_COMPRESSION {
                self.offering = None;
            }
        }
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

    /// 3.1 is the superseded form of 3.3, and is refused so the peer uses the
    /// one this end reads.
    #[test]
    fn the_superseded_address_option_is_refused_outright() {
        let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let asking = vec![
            ConfigOption { kind: option::IP_ADDRESSES, value: vec![10, 0, 0, 2, 10, 0, 0, 1] },
            ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0, 0, 2] },
        ];
        match ipcp.review(&asking) {
            Review::Reject(o) => {
                assert_eq!(o.len(), 1);
                assert_eq!(o[0].kind, option::IP_ADDRESSES);
            }
            other => panic!("expected a reject, got {other:?}"),
        }
    }

    /// A far end that can read compressed headers says so, and agreeing means
    /// this end may send them.
    #[test]
    fn a_far_end_that_can_read_compressed_headers_gets_them() {
        let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let asking = vec![
            ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0, 0, 2] },
            ConfigOption { kind: option::IP_COMPRESSION, value: vec![0x00, 0x2d, 0x0f, 0x01] },
        ];
        assert_eq!(ipcp.review(&asking), Review::Ack);
        assert_eq!(
            ipcp.compression.sending,
            Some(vj::Params { max_slot: 15, compress_slot: true })
        );
        // And nothing has been agreed in the other direction by that alone:
        // 4 negotiates each separately.
        assert_eq!(ipcp.compression.receiving, None);
    }

    /// The far end's slot count is the far end's business, because they are
    /// its slots and it is the one rebuilding headers out of them.
    #[test]
    fn the_slots_agreed_to_are_the_ones_the_far_end_asked_for() {
        let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let asking = vec![
            ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0, 0, 2] },
            ConfigOption { kind: option::IP_COMPRESSION, value: vec![0x00, 0x2d, 0x03, 0x00] },
        ];
        assert_eq!(ipcp.review(&asking), Review::Ack);
        assert_eq!(
            ipcp.compression.sending,
            Some(vj::Params { max_slot: 3, compress_slot: false })
        );
    }

    /// Some other compression protocol is refused rather than mistaken for
    /// the one this end knows.
    #[test]
    fn a_compression_protocol_this_end_does_not_know_is_refused() {
        for value in [
            // RFC 3544's IP header compression, which this does not do.
            vec![0x00, 0x61, 0x0f, 0x01],
            // Van Jacobson, but with a Comp-Slot-Id 4.1 does not define.
            vec![0x00, 0x2d, 0x0f, 0x02],
            // And the wrong length entirely.
            vec![0x00, 0x2d],
        ] {
            let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
            let asking = vec![ConfigOption { kind: option::IP_COMPRESSION, value: value.clone() }];
            match ipcp.review(&asking) {
                Review::Reject(o) => assert_eq!(o[0].kind, option::IP_COMPRESSION),
                other => panic!("{value:02x?} got {other:?}"),
            }
            assert_eq!(ipcp.compression.sending, None);
        }
    }

    /// 5.4: a far end that rejects the option is one that does not do this,
    /// and this end stops asking rather than failing the link over it.
    #[test]
    fn a_far_end_that_will_not_compress_is_taken_at_its_word() {
        let mut ipcp = Ipcp::new(UNSPECIFIED, UNSPECIFIED);
        assert_eq!(ipcp.request().len(), 2);
        ipcp.rejected(&[ConfigOption {
            kind: option::IP_COMPRESSION,
            value: vec![0x00, 0x2d, 0x0f, 0x01],
        }]);
        assert_eq!(ipcp.offering, None);
        let request = ipcp.request();
        assert_eq!(request.len(), 1, "it asked again after being refused");
        assert_eq!(request[0].kind, option::IP_ADDRESS);
        assert_eq!(ipcp.compression.receiving, None);
    }

    /// 5.3: a Nak carries what the peer would accept, so this end asks for
    /// that. A far end with fewer slots says so this way.
    #[test]
    fn a_far_end_with_fewer_slots_is_asked_again_for_what_it_will_take() {
        let mut ipcp = Ipcp::new(UNSPECIFIED, UNSPECIFIED);
        ipcp.naked(&[ConfigOption {
            kind: option::IP_COMPRESSION,
            value: vec![0x00, 0x2d, 0x03, 0x00],
        }]);
        assert_eq!(ipcp.offering, Some(vj::Params { max_slot: 3, compress_slot: false }));
        let request = ipcp.request();
        assert_eq!(request[1].value, vec![0x00, 0x2d, 0x03, 0x00]);
    }

    /// The acknowledgement of this end's own request is what settles the
    /// direction this end has to read.
    #[test]
    fn an_acknowledged_request_settles_what_this_end_must_decompress() {
        let mut ipcp = Ipcp::new([10, 0, 0, 2], [10, 0, 0, 1]);
        let request = ipcp.request();
        ipcp.acked(&request);
        assert_eq!(ipcp.compression.receiving, Some(vj::Params::DEFAULT));
        // And an acknowledgement that came back without it, from a far end
        // that had already rejected it, settles nothing.
        let mut ipcp = Ipcp::new([10, 0, 0, 2], [10, 0, 0, 1]);
        ipcp.acked(&[ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0, 0, 2] }]);
        assert_eq!(ipcp.compression.receiving, None);
    }

    /// Turned off, nothing is asked for and nothing is agreed.
    #[test]
    fn an_end_that_does_not_want_it_does_not_ask() {
        let ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]).without_header_compression();
        let request = ipcp.request();
        assert_eq!(request.len(), 1);
        assert_eq!(request[0].kind, option::IP_ADDRESS);
    }

    /// An address of the wrong length is not an address.
    #[test]
    fn a_malformed_address_is_rejected() {
        let mut ipcp = Ipcp::new([10, 0, 0, 1], [10, 0, 0, 2]);
        let asking = vec![ConfigOption { kind: option::IP_ADDRESS, value: vec![10, 0] }];
        assert!(matches!(ipcp.review(&asking), Review::Reject(_)));
    }

    /// The request always carries the address option, even empty: the zeroes
    /// are the question.
    #[test]
    fn an_end_with_no_address_still_asks() {
        let ipcp = Ipcp::new(UNSPECIFIED, UNSPECIFIED);
        let request = ipcp.request();
        assert_eq!(request[0].kind, option::IP_ADDRESS);
        assert_eq!(request[0].value, UNSPECIFIED.to_vec());
        // 4.1: two octets of protocol, then Max-Slot-Id and Comp-Slot-Id.
        // Sixteen slots, as Appendix A recommends.
        assert_eq!(request[1].kind, option::IP_COMPRESSION);
        assert_eq!(request[1].value, vec![0x00, 0x2d, 0x0f, 0x01]);
    }
}
