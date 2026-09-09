//! The Transmission Control Protocol, in userspace.
//!
//! Written against RFC 9293, which obsoletes RFC 793 and gathers fifty years
//! of amendments to it into one document -- so that is the one the clause
//! numbers in here refer to, and where it names a requirement (MUST-15,
//! SHLD-5) the name is quoted with it.
//!
//! What it is for: a modem call carries octets, PPP turns those into IP
//! datagrams, and this turns those into streams a program can use. Nothing
//! here touches the operating system's own stack. There is no driver, no
//! adapter and nothing to install, which is the whole reason for writing it
//! rather than asking Windows for a socket.
//!
//! It knows nothing about modems, PPP or sound cards. A connection is handed
//! segments and gives segments back; who carries them is not its business.

pub mod connection;
pub mod segment;
pub mod seq;

pub use connection::{Connection, State};
pub use segment::Segment;
pub use seq::Seq;
