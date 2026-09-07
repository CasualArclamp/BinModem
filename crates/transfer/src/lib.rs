//! File transfer over a modem connection.
//!
//! A board offers to send you a file and the two ends have to agree how, which
//! is a protocol of its own sitting on top of everything else here: the data
//! pump carries bits, V.42 makes them reliable, and this decides what a file
//! is and how much of one has arrived.
//!
//! ZMODEM is the one worth having. It streams rather than stopping for an
//! acknowledgement after every block, it recovers from an error by asking for
//! the position it wants rather than starting again, it carries the file's
//! name and length and date, and it can resume a transfer that was interrupted
//! days ago. It is also what every board actually speaks.
//!
//! It is not an ITU Recommendation and there is no clause numbering to cite.
//! The reference is Chuck Forsberg's *The ZMODEM Inter Application File
//! Transfer Protocol*, October 1988, and the section numbers in these comments
//! are its own. Two things in it are given by reference to a C header rather
//! than in the text -- the frame type numbers and the subpacket terminators --
//! and where this code relies on a value that is not written down in the
//! document, it says so and says what it was derived from.

pub mod zmodem;
