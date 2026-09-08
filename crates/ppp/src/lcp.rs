//! The Link Control Protocol's own options (RFC 1661 section 6, and RFC 1662
//! section 7 for the one that lives there).
//!
//! The automaton in [`super::control`] runs the negotiation; this decides what
//! to ask for and what to make of what the peer asks.
//!
//! ## Which direction an option is about
//!
//! The part that is easy to get backwards, and 6 states it once: "all
//! Configuration Options apply in a half-duplex fashion; typically, in the
//! receive direction of the link from the point of view of the Configure-
//! Request sender."
//!
//! So an option in the peer's request is a statement about what the peer will
//! receive, which makes it an instruction about how this end must send. Its
//! MRU is the largest frame this end may put on the line. Its
//! Async-Control-Character-Map is what this end must escape. Its
//! Authentication-Protocol is a demand that this end prove who it is. And the
//! options this end sends are the mirror of that, about what arrives here.

use crate::control::ConfigOption;

/// The option type numbers. 6 leaves them to the Assigned Numbers RFC, and
/// each option's own section states its own.
pub mod option {
    /// 6.1, the largest frame this end will receive.
    pub const MRU: u8 = 1;
    /// RFC 1662 7.1, which octets this end cannot receive literally.
    pub const ACCM: u8 = 2;
    /// 6.2, a demand that the other end say who it is.
    pub const AUTHENTICATION: u8 = 3;
    /// 6.3, link quality monitoring, which this modem does not do.
    pub const QUALITY: u8 = 4;
    /// 6.4, a number to tell this end's frames from its own echo.
    pub const MAGIC: u8 = 5;
    /// 6.5 and 6.6, the two ways to make a frame shorter.
    pub const PFC: u8 = 7;
    pub const ACFC: u8 = 8;
}

/// 6.1: "The default value is 1500 octets."
pub const DEFAULT_MRU: u16 = 1500;

/// How a far end wants to be told who is calling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// RFC 1334: the password goes across as it is.
    Pap,
    /// RFC 1994 with algorithm 5: a hash of a challenge, so the password does
    /// not.
    ChapMd5,
}

impl Auth {
    /// The two octets of 6.2's Authentication-Protocol field, and the
    /// algorithm octet where there is one.
    pub fn to_value(self) -> Vec<u8> {
        match self {
            Self::Pap => vec![0xc0, 0x23],
            // RFC 1994 3: "5  CHAP with MD5".
            Self::ChapMd5 => vec![0xc2, 0x23, 5],
        }
    }

    fn from_value(value: &[u8]) -> Option<Self> {
        match value {
            [0xc0, 0x23] => Some(Self::Pap),
            [0xc2, 0x23, 5] => Some(Self::ChapMd5),
            _ => None,
        }
    }
}

/// What this end asks the far end to do when sending to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub mru: u16,
    /// What this end cannot receive literally.
    ///
    /// Nothing, over a modem carrying V.42: the link is transparent to every
    /// octet and there is no flow control on it to trip over. Asking for the
    /// default instead would have every control character in every IP packet
    /// sent as two, which at 9600 is a cost worth not paying. A far end that
    /// disagrees will say so.
    pub accm: u32,
    /// 6.4: zero means the option is not being asked for at all.
    pub magic: u32,
    pub pfc: bool,
    pub acfc: bool,
}

impl Default for Wanted {
    fn default() -> Self {
        Self {
            mru: DEFAULT_MRU,
            accm: 0,
            magic: 0,
            pfc: true,
            acfc: true,
        }
    }
}

impl Wanted {
    /// The options to put in a Configure-Request.
    ///
    /// 6: "It is not necessary to send the default values for the options in a
    /// Configure-Request", so anything already at its default is left out. A
    /// shorter request is a shorter negotiation, and every option sent is one
    /// the peer may Nak.
    pub fn to_options(&self) -> Vec<ConfigOption> {
        let mut out = Vec::new();
        if self.mru != DEFAULT_MRU {
            out.push(ConfigOption {
                kind: option::MRU,
                value: self.mru.to_be_bytes().to_vec(),
            });
        }
        // The default here is every octet escaped (RFC 1662 A), so anything
        // else has to be asked for.
        if self.accm != crate::frame::DEFAULT_ACCM {
            out.push(ConfigOption {
                kind: option::ACCM,
                value: self.accm.to_be_bytes().to_vec(),
            });
        }
        if self.magic != 0 {
            out.push(ConfigOption {
                kind: option::MAGIC,
                value: self.magic.to_be_bytes().to_vec(),
            });
        }
        if self.pfc {
            out.push(ConfigOption { kind: option::PFC, value: Vec::new() });
        }
        if self.acfc {
            out.push(ConfigOption { kind: option::ACFC, value: Vec::new() });
        }
        out
    }
}

/// What the far end asked for and this end agreed to, which is how this end
/// must now send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agreed {
    /// The largest frame this end may send.
    pub mru: u16,
    /// What this end must escape when sending.
    pub accm: u32,
    /// Set if the far end wants this end to prove who it is.
    pub auth: Option<Auth>,
    pub magic: Option<u32>,
    /// Whether this end may leave the protocol field short, and the address
    /// and control off.
    pub pfc: bool,
    pub acfc: bool,
}

impl Default for Agreed {
    fn default() -> Self {
        Self {
            mru: DEFAULT_MRU,
            accm: crate::frame::DEFAULT_ACCM,
            auth: None,
            magic: None,
            pfc: false,
            acfc: false,
        }
    }
}

/// What to answer a Configure-Request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Review {
    /// 5.2: every option is acceptable.
    Ack,
    /// 5.3: recognised, but not at those values. Carries what would be
    /// acceptable instead.
    Nak(Vec<ConfigOption>),
    /// 5.4: not recognised, or not negotiable at all. Carries them back
    /// unchanged.
    Reject(Vec<ConfigOption>),
}

/// The most this end will agree to receive.
///
/// Larger than the default because a peer may ask for more and there is no
/// reason to refuse; smaller than a link could ever carry, so a peer asking
/// for something absurd is told a number rather than believed.
pub const MAX_MRU: u16 = 4096;

/// Decide what a peer's Configure-Request deserves, and record what is being
/// agreed to.
///
/// `agreed` is only written where the answer is [`Review::Ack`]: 5.2 makes an
/// Ack the moment the options take effect, and half of a rejected request is
/// not an agreement about anything.
pub fn review(options: &[ConfigOption], agreed: &mut Agreed) -> Review {
    let mut reject = Vec::new();
    let mut nak = Vec::new();
    let mut accepted = Agreed::default();

    for option in options {
        match (option.kind, option.value.as_slice()) {
            (option::MRU, &[hi, lo]) => {
                let mru = u16::from_be_bytes([hi, lo]);
                if mru > MAX_MRU {
                    nak.push(ConfigOption {
                        kind: option::MRU,
                        value: MAX_MRU.to_be_bytes().to_vec(),
                    });
                } else {
                    accepted.mru = mru;
                }
            }
            (option::ACCM, &[a, b, c, d]) => {
                // Always acceptable. It costs this end only escaping, and a
                // peer that asks for more escaping knows something about the
                // path that this end does not.
                accepted.accm = u32::from_be_bytes([a, b, c, d]);
            }
            (option::AUTHENTICATION, value) => match Auth::from_value(value) {
                Some(auth) => accepted.auth = Some(auth),
                // 6.2: "If the receiver of a Configure-Request is unwilling to
                // use the specified authentication protocol, it SHOULD respond
                // with a Configure-Nak, suggesting an alternative." So this is
                // a Nak and not a Reject: the far end is entitled to want
                // authentication, and this end is only refusing the method.
                None => nak.push(ConfigOption {
                    kind: option::AUTHENTICATION,
                    value: Auth::ChapMd5.to_value(),
                }),
            },
            (option::MAGIC, &[a, b, c, d]) => {
                accepted.magic = Some(u32::from_be_bytes([a, b, c, d]));
            }
            (option::PFC, []) => accepted.pfc = true,
            (option::ACFC, []) => accepted.acfc = true,
            // 6.3 needs a link quality monitoring protocol underneath it and
            // there is none here, so it is refused outright rather than
            // haggled over.
            (option::QUALITY, _) => reject.push(option.clone()),
            // Anything else, and anything whose length is wrong for what it
            // claims to be. 5.4 covers both: "not recognizable or not
            // acceptable for negotiation".
            _ => reject.push(option.clone()),
        }
    }

    // 5.4 before 5.3. A Configure-Reject says what cannot be discussed and a
    // Configure-Nak says what can; sending the second while the first is
    // outstanding invites a peer to keep offering an option this end will
    // never take.
    if !reject.is_empty() {
        return Review::Reject(reject);
    }
    if !nak.is_empty() {
        return Review::Nak(nak);
    }
    *agreed = accepted;
    Review::Ack
}

/// LCP as one end sees it: what it will ask for, and what it has agreed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lcp {
    pub wanted: Wanted,
    pub agreed: Agreed,
}

impl Lcp {
    pub fn new(wanted: Wanted) -> Self {
        Self { wanted, agreed: Agreed::default() }
    }
}

impl crate::session::Protocol for Lcp {
    fn number(&self) -> u16 {
        crate::protocol::LCP
    }

    fn request(&self) -> Vec<ConfigOption> {
        self.wanted.to_options()
    }

    fn review(&mut self, options: &[ConfigOption]) -> Review {
        review(options, &mut self.agreed)
    }

    fn acked(&mut self, _options: &[ConfigOption]) {
        // Nothing to record. 5.2 echoes the request back, so an acknowledgement
        // says only that what was asked for is what will happen -- and what was
        // asked for is already in `wanted`.
    }

    fn naked(&mut self, options: &[ConfigOption]) {
        // 5.3: what comes back is what the peer would accept, so the next
        // request carries that instead of what it refused.
        for option in options {
            match (option.kind, option.value.as_slice()) {
                (option::MRU, &[hi, lo]) => {
                    self.wanted.mru = u16::from_be_bytes([hi, lo]);
                }
                (option::ACCM, &[a, b, c, d]) => {
                    // A peer that wants more escaping knows something about the
                    // path this end does not, so it is taken as given.
                    self.wanted.accm = u32::from_be_bytes([a, b, c, d]);
                }
                (option::MAGIC, &[a, b, c, d]) => {
                    // 6.4: a Nak of a magic number means it collided, and the
                    // answer is a different one rather than the one suggested.
                    self.wanted.magic = u32::from_be_bytes([a, b, c, d]) ^ 0x5555_5555;
                }
                _ => {}
            }
        }
    }

    fn rejected(&mut self, options: &[ConfigOption]) {
        // 5.4: stop asking. Every one of these is something the link can do
        // without.
        for option in options {
            match option.kind {
                option::MAGIC => self.wanted.magic = 0,
                option::PFC => self.wanted.pfc = false,
                option::ACFC => self.wanted.acfc = false,
                option::ACCM => self.wanted.accm = crate::frame::DEFAULT_ACCM,
                option::MRU => self.wanted.mru = DEFAULT_MRU,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(options: Vec<ConfigOption>) -> (Review, Agreed) {
        let mut agreed = Agreed::default();
        let review = review(&options, &mut agreed);
        (review, agreed)
    }

    /// The request a far end usually sends, and what it means for this end.
    #[test]
    fn an_ordinary_request_is_agreed_to() {
        let (review, agreed) = request(vec![
            ConfigOption { kind: option::MRU, value: vec![0x05, 0xdc] },
            ConfigOption { kind: option::ACCM, value: vec![0, 0, 0, 0] },
            ConfigOption { kind: option::MAGIC, value: vec![1, 2, 3, 4] },
            ConfigOption { kind: option::PFC, value: vec![] },
            ConfigOption { kind: option::ACFC, value: vec![] },
        ]);
        assert_eq!(review, Review::Ack);
        assert_eq!(agreed.mru, 1500);
        assert_eq!(agreed.accm, 0);
        assert_eq!(agreed.magic, Some(0x0102_0304));
        assert!(agreed.pfc && agreed.acfc);
        assert_eq!(agreed.auth, None);
    }

    /// A far end that wants to know who is calling says so here, which is the
    /// answer to whether it wants PAP or CHAP -- no guessing required.
    #[test]
    fn a_demand_for_authentication_is_read_and_accepted() {
        let (review, agreed) = request(vec![ConfigOption {
            kind: option::AUTHENTICATION,
            value: vec![0xc2, 0x23, 5],
        }]);
        assert_eq!(review, Review::Ack);
        assert_eq!(agreed.auth, Some(Auth::ChapMd5));

        let (review, agreed) = request(vec![ConfigOption {
            kind: option::AUTHENTICATION,
            value: vec![0xc0, 0x23],
        }]);
        assert_eq!(review, Review::Ack);
        assert_eq!(agreed.auth, Some(Auth::Pap));
    }

    /// 6.2: an authentication protocol this end cannot do is a Nak with one it
    /// can, not a refusal to authenticate.
    #[test]
    fn an_authentication_method_we_lack_is_answered_with_one_we_have() {
        // Microsoft CHAP, which this does not implement.
        let (review, _) = request(vec![ConfigOption {
            kind: option::AUTHENTICATION,
            value: vec![0xc2, 0x23, 0x80],
        }]);
        assert_eq!(
            review,
            Review::Nak(vec![ConfigOption {
                kind: option::AUTHENTICATION,
                value: vec![0xc2, 0x23, 5],
            }])
        );
    }

    /// 6.3 wants a monitoring protocol this modem has not got.
    #[test]
    fn link_quality_monitoring_is_refused_outright() {
        let (review, _) = request(vec![ConfigOption {
            kind: option::QUALITY,
            value: vec![0xc0, 0x25, 0, 0, 0, 10],
        }]);
        assert!(matches!(review, Review::Reject(ref o) if o.len() == 1));
    }

    /// 5.4: a Reject outranks a Nak, so a peer is not invited to keep
    /// improving an offer this end will never take.
    #[test]
    fn something_unnegotiable_is_answered_before_something_merely_wrong() {
        let (review, _) = request(vec![
            ConfigOption { kind: option::AUTHENTICATION, value: vec![0xff, 0xff] },
            ConfigOption { kind: 0xfe, value: vec![] },
        ]);
        match review {
            Review::Reject(options) => {
                assert_eq!(options.len(), 1);
                assert_eq!(options[0].kind, 0xfe);
            }
            other => panic!("expected a reject first, got {other:?}"),
        }
    }

    /// An option the right shape but the wrong size is not that option.
    #[test]
    fn an_option_of_the_wrong_length_is_rejected_rather_than_guessed_at() {
        for bad in [
            ConfigOption { kind: option::MRU, value: vec![0x05] },
            ConfigOption { kind: option::ACCM, value: vec![0, 0] },
            ConfigOption { kind: option::MAGIC, value: vec![] },
            ConfigOption { kind: option::PFC, value: vec![0] },
        ] {
            let (review, _) = request(vec![bad.clone()]);
            assert!(
                matches!(review, Review::Reject(ref o) if o == std::slice::from_ref(&bad)),
                "{bad:?} was not rejected"
            );
        }
    }

    /// A peer asking to send more than this end can hold is told a number.
    #[test]
    fn an_impossible_frame_size_is_answered_with_a_possible_one() {
        let (review, _) = request(vec![ConfigOption {
            kind: option::MRU,
            value: 60000u16.to_be_bytes().to_vec(),
        }]);
        assert_eq!(
            review,
            Review::Nak(vec![ConfigOption {
                kind: option::MRU,
                value: MAX_MRU.to_be_bytes().to_vec(),
            }])
        );
    }

    /// 6: defaults are left out of a request, because every option sent is one
    /// the peer may argue with.
    #[test]
    fn a_request_carries_only_what_is_not_already_the_default() {
        let bare = Wanted { mru: DEFAULT_MRU, accm: 0, magic: 0, pfc: false, acfc: false };
        // ACCM is the exception: its default is everything escaped, so asking
        // for nothing escaped is a thing that has to be said.
        let options = bare.to_options();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].kind, option::ACCM);
        assert_eq!(options[0].value, vec![0, 0, 0, 0]);

        let quiet = Wanted {
            accm: crate::frame::DEFAULT_ACCM,
            ..bare
        };
        assert!(quiet.to_options().is_empty(), "a request with nothing to say says nothing");
    }

    /// And what a full one looks like.
    #[test]
    fn the_default_request_asks_for_the_four_things_worth_asking_for() {
        let wanted = Wanted { magic: 0xdead_beef, ..Wanted::default() };
        let kinds: Vec<u8> = wanted.to_options().iter().map(|o| o.kind).collect();
        assert_eq!(kinds, vec![option::ACCM, option::MAGIC, option::PFC, option::ACFC]);
    }

    /// A request that was answered with anything but an Ack changes nothing.
    #[test]
    fn a_rejected_request_agrees_to_nothing() {
        let mut agreed = Agreed::default();
        let before = agreed;
        let outcome = review(
            &[
                ConfigOption { kind: option::MRU, value: vec![0x02, 0x00] },
                ConfigOption { kind: 0xfe, value: vec![] },
            ],
            &mut agreed,
        );
        assert!(matches!(outcome, Review::Reject(_)));
        assert_eq!(agreed, before, "half a rejected request was agreed to");
    }
}
