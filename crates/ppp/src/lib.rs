//! The Point-to-Point Protocol, which is what turns a modem call into a
//! network connection.
//!
//! A modem carries octets. The internet is packets, and PPP is the agreement
//! that turns one into the other: where a packet begins and ends (RFC 1662),
//! what the two ends can do (RFC 1661), who is calling (RFC 1334, RFC 1994)
//! and what address they get (RFC 1332).
//!
//! Written the same way as everything else here, against the documents rather
//! than against a memory of them, with section numbers on the constants.
//! `tools/fetch_specs.sh` downloads the RFCs beside the ITU Recommendations.

pub mod control;
pub mod frame;
pub mod lcp;

pub use control::{Action, Code, ConfigOption, Event, Message, State, transition};
pub use frame::{Deframer, Discarded, Framer, Packet};

/// The protocol numbers this implementation knows, from the "PPP DLL Protocol
/// Numbers" registry that RFC 1661 3.1 points at.
pub mod protocol {
    /// Internet Protocol, the thing all of it is for.
    pub const IP: u16 = 0x0021;
    /// Link Control Protocol: what the two ends can do (RFC 1661).
    pub const LCP: u16 = 0xc021;
    /// Password Authentication Protocol (RFC 1334).
    pub const PAP: u16 = 0xc023;
    /// Challenge Handshake Authentication Protocol (RFC 1994).
    pub const CHAP: u16 = 0xc223;
    /// IP Control Protocol: addresses and header compression (RFC 1332).
    pub const IPCP: u16 = 0x8021;
}
