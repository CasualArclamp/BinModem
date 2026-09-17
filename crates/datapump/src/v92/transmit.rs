//! Turning 8000 upstream levels a second into line samples (6.2): a clock
//! slaved to the network through the downstream receiver, and the one-off
//! shifts of half a symbol and then epsilon that S-bar-u carries (8.6.3, with
//! 9.5.2.1.7-9.5.2.1.8).
//!
//! V92-15 fills this in.
