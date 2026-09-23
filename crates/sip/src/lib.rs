//! A telephone line made of packets: SIP to place the call, RTP to carry it,
//! G.711 to be the samples.
//!
//! What this replaces. Until now a live call went: modem, sound card, virtual
//! cable, softphone, network. Every stage of that middle stretch is a
//! resampler and a compromise. The softphone converts 48 kHz to 8, runs an
//! adaptive jitter buffer that inserts and deletes audio when the network
//! wobbles, and applies whatever gain control it thinks a voice wants. None of
//! those are faults in the softphone -- they are exactly right for a telephone
//! call -- and all of them are wrong for a modem. Two of them have already
//! cost real debugging time here: the jitter buffer's twenty-millisecond
//! inserts look like a receiver losing lock, and the rate conversion puts
//! noise on a V.90 server's codewords that no equaliser can undo.
//!
//! So the cable and the softphone come out. This crate is the far end of the
//! call: it registers with a trunk, places the call, and hands the modem the
//! G.711 codewords that arrived from the network, with nothing in between that
//! the modem did not ask for.
//!
//! # The layers, and what each is written against
//!
//! - [`uri`], [`message`]: RFC 3261 7, 19 and 20 -- what a SIP message is.
//! - [`auth`], [`md5`]: RFC 3261 22 and RFC 1321 -- what a registrar wants
//!   before it will believe who we are.
//! - [`sdp`]: RFC 4566 and RFC 3264 -- what the two ends agree to send.
//! - [`rtp`], [`jitter`]: RFC 3550 -- the packets, and the small amount of
//!   patience needed to put them back in order.
//! - [`g711`]: ITU-T G.711 -- the samples themselves.
//! - [`ua`]: the user agent: registration, dialogs, transactions, a call.
//! - [`media`]: the RTP socket, and the rate conversion to the modem's rate.
//! - [`line`]: all of it behind the same two methods `line::Duplex` offers, so
//!   the thing above does not have to know which kind of line it has got.
//!
//! # What it deliberately does not do
//!
//! No TLS, no SRTP: a modem call over a trunk is plaintext at the far end
//! anyway, and encrypting the first hop would hide nothing worth hiding while
//! making every capture unreadable. No ICE, no STUN: a trunk with symmetric
//! RTP and rport gets through the ordinary home router, which is the case that
//! exists here. No codec but G.711, ever -- see [`g711`] for why. T.38 is
//! meant, and is not here yet; [`sdp`] recognises an `image/udptl` offer so
//! that a far end asking for one is declined politely rather than confusing
//! the call.

pub mod account;
pub mod auth;
pub mod g711;
pub mod jitter;
pub mod line;
pub mod md5;
pub mod media;
pub mod message;
pub mod rand;
pub mod rtp;
pub mod sdp;
pub mod ua;
pub mod uri;

pub use account::{Account, Transport};
pub use g711::Law;
pub use line::{Line, Progress};
pub use message::{Message, Method, Request, Response};
pub use uri::{Address, Uri};
