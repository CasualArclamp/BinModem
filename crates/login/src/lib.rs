//! The text that came before PPP on a dial-up account.
//!
//! PPP starts on a line that is already carrying octets, and on a real
//! provider's modem pool something else had the line first: a terminal server
//! printing a banner and `login:`, asking for a password, and then either a
//! menu with `ppp` on it or PPP straight away. A person with a terminal could
//! log in by hand; Windows' Dial-Up Networking ran a script that did the same,
//! or skipped the text altogether and started sending PPP frames, which the
//! better terminal servers noticed and answered as PPP.
//!
//! Both ends of that are here. [`Server`] is the terminal server, for a modem
//! answering calls; [`Script`] is the dialler's side, answering the prompts of
//! a far end that asks. Neither has a standard behind it -- this is the part
//! of dial-up that every provider did its own way -- so each is written to
//! cope with the ways that were common, and to stop and say so rather than
//! guess when it meets something else.

pub mod script;
pub mod server;

pub use ppp::auth::Account;
pub use script::Script;
pub use server::Server;

/// RFC 1662 4.1's Flag Sequence, which every frame starts with.
const FLAG: u8 = 0x7e;

/// The opening octets of the first frame a PPP end sends.
///
/// RFC 1661 6.6 is what makes these recognisable: the Address and Control
/// fields "MUST NOT be compressed when sending any LCP packet", and 6.5 says
/// the same of the Protocol field, so an LCP frame always opens with the flag,
/// 0xff, 0x03 and 0xc021. RFC 1662 7.1 has the default map escape every
/// control character, so the 0x03 normally arrives as 0x7d 0x23 -- but not
/// every sender escapes before it has been asked to, and both are accepted.
const LCP_OPENINGS: [&[u8]; 2] = [&[FLAG, 0xff, 0x7d, 0x23, 0xc0, 0x21], &[FLAG, 0xff, 0x03, 0xc0, 0x21]];

/// What a [`PppWatch`] made of some octets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Watched {
    /// Octets that are certainly text, in order.
    pub text: Vec<u8>,
    /// Everything from an LCP frame's opening flag onwards, once one has been
    /// recognised.
    pub ppp: Option<Vec<u8>>,
}

/// Watches text for the moment it turns into PPP.
///
/// Octets that could be the start of a frame are held back until they either
/// are one or cannot be, so nothing that turns out to be PPP is ever treated
/// as typing -- echoed back into the caller's frame, say. The cost is that a
/// `~` at the very end of what arrived waits for the next octet.
#[derive(Debug, Default)]
pub struct PppWatch {
    /// The octets since the last flag, while they could still be the start of
    /// an LCP frame.
    held: Vec<u8>,
}

impl PppWatch {
    /// Look at what arrived. The moment an LCP frame is recognised, gives back
    /// everything from its opening flag onwards -- including octets held over
    /// from an earlier call -- so none of it is lost on the way to the link.
    pub fn feed(&mut self, bytes: &[u8]) -> Watched {
        let mut text = Vec::new();
        for (i, &byte) in bytes.iter().enumerate() {
            if byte == FLAG {
                text.append(&mut self.held);
                self.held.push(byte);
                continue;
            }
            if self.held.is_empty() {
                text.push(byte);
                continue;
            }
            self.held.push(byte);
            if LCP_OPENINGS.contains(&self.held.as_slice()) {
                let mut early = std::mem::take(&mut self.held);
                early.extend_from_slice(&bytes[i + 1..]);
                return Watched { text, ppp: Some(early) };
            }
            if !LCP_OPENINGS.iter().any(|o| o.starts_with(&self.held)) {
                text.append(&mut self.held);
            }
        }
        Watched { text, ppp: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Configure-Request as a pppd client sends it, with the default
    /// map in force: flag, 0xff, escaped 0x03, 0xc021, escaped code 1.
    const PPPD: &[u8] = &[
        0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7d, 0x21, 0x7d, 0x21, 0x7d, 0x20, 0x7d, 0x34,
    ];

    #[test]
    fn a_frame_is_recognised_wherever_the_buffers_split_it() {
        for split in 0..PPPD.len() {
            let mut watch = PppWatch::default();
            let mut typed = b"guest\r".to_vec();
            typed.extend_from_slice(&PPPD[..split]);
            let first = watch.feed(&typed);
            assert_eq!(first.text, b"guest\r", "split {split}: part of the frame was let through as text");
            let early = match first.ppp {
                // Recognised already: what arrives after it is the link's.
                Some(mut early) => {
                    early.extend_from_slice(&PPPD[split..]);
                    early
                }
                None => watch.feed(&PPPD[split..]).ppp.unwrap_or_else(|| panic!("missed at split {split}")),
            };
            assert_eq!(early, PPPD, "split {split}");
        }
    }

    #[test]
    fn a_tilde_in_ordinary_typing_is_not_ppp() {
        let mut watch = PppWatch::default();
        let typing = b"cd ~/mail\r~~~ password~!\r";
        assert_eq!(watch.feed(typing), Watched { text: typing.to_vec(), ppp: None });
        // A tilde last is held for one octet, and then let go.
        assert_eq!(watch.feed(b"abc~").text, b"abc");
        assert_eq!(watch.feed(b"\r").text, b"~\r");
        // And an unescaped opening is taken too.
        assert!(watch.feed(&[0x7e, 0xff, 0x03, 0xc0, 0x21, 0x01]).ppp.is_some());
    }
}
