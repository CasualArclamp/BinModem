//! U.S. Robotics x2, the answering (server) side: 56k downstream in G.711
//! octets, V.34 upstream.
//!
//! x2 is the PCM scheme U.S. Robotics shipped in 1997, before V.90. The
//! server sits on the digital network and picks the octets the network
//! carries to the client's codec, as a V.90 digital modem does, but by rules
//! of its own: a fixed training script, records in a format of their own,
//! and a mapper over level banks no Recommendation describes.
//!
//! Nothing of it is standardised. What is here is written from facts others
//! established, each cited where it is used: libv90modem's
//! `docs/history/x2-facts.md` (github.com/georgica/libv90modem), which
//! gathers D-Modem's notes, courier-emu's specification and its own reading
//! of the server firmware; D-Modem's published level banks are carried as
//! data in `banks.rs`. No code of any of them is used.

pub mod bank;
pub mod client;
mod banks;
pub mod map;
pub mod record;
pub mod script;
pub mod server;
