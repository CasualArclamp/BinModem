//! ZMODEM, after Forsberg (October 1988).

pub mod crc;
pub mod escape;
pub mod file;
pub mod header;
pub mod receive;
pub mod send;
pub mod subpacket;

pub use file::FileInfo;
pub use header::{Header, HeaderError, Style};
pub use receive::{Received, Receiver};
pub use send::{Failure, Progress, Sender, State};
pub use subpacket::{Ending, Subpacket, SubpacketError};

/// The character a header starts with, `*` (7.3.1, Figure 2).
///
/// A binary header opens `ZPAD ZDLE ZBIN`; a hex header opens `ZPAD ZPAD ZDLE
/// ZHEX`. 7.3.3: "the extra ZPAD character allows the sending program to
/// detect an asynchronous header (indicating an error condition)".
pub const ZPAD: u8 = b'*';

/// The data link escape, `030` octal, which is ASCII CAN (7.2).
///
/// Chosen for that: "this particular value was chosen to allow a string of 5
/// consecutive CAN characters to abort a ZMODEM session, compatible with
/// YMODEM session abort".
pub const ZDLE: u8 = 0x18;

/// A binary header with a 16-bit check sequence (7.3.1).
pub const ZBIN: u8 = b'A';
/// A hex header (7.3.3). The receiver answers in these, and so does the sender
/// where no data subpacket follows.
pub const ZHEX: u8 = b'B';
/// A binary header with a 32-bit check sequence (7.3.2).
pub const ZBIN32: u8 = b'C';

/// Consecutive CAN characters that abort a session (7.2).
///
/// "Receipt of five successive CAN characters will abort a ZMODEM session.
/// Eight CAN characters are sent."
pub const CAN_TO_ABORT: usize = 5;
/// And how many to send, which is more than are needed to be recognised.
pub const CAN_TO_SEND: usize = 8;

/// What a frame is for.
///
/// 7.3: "the frame types are cardinal numbers beginning with 0 to minimize
/// state transition table memory requirements", and the document then lists
/// them in clause 11 in that order -- 11.1 ZRQINIT through 11.19 ZCOMMAND.
/// The numbers themselves it gives only by reference to `zmodem.h`, so the
/// ordering of clause 11 is where these come from, and the two agree with
/// every implementation in the wild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Ask the receiver to send its ZRINIT (11.1).
    Rqinit = 0,
    /// The receiver's capabilities and buffer size (11.2).
    Rinit = 1,
    /// The sender's options and attention sequence (11.3).
    Sinit = 2,
    /// Acknowledgement, carrying a file offset (11.4).
    Ack = 3,
    /// A file is starting; the subpacket carries its name and size (11.5).
    File = 4,
    /// The receiver does not want this file (11.6).
    Skip = 5,
    /// The last frame was not understood (11.7).
    Nak = 6,
    /// Give up on the batch (11.8).
    Abort = 7,
    /// The session is over (11.9).
    Fin = 8,
    /// Resend from this position (11.10).
    Rpos = 9,
    /// File data follows, from the position in the header (11.11).
    Data = 10,
    /// End of the file, at the length in the header (11.12).
    Eof = 11,
    /// An error reading or writing the file (11.13).
    Ferr = 12,
    /// Asking for a check sequence over the file so far (11.14).
    Crc = 13,
    /// A number to be echoed back, proving the far end is really there (11.15).
    Challenge = 14,
    /// The request is complete (11.16).
    Compl = 15,
    /// The other end has cancelled (11.17).
    Can = 16,
    /// How much room is left on the receiver's disk (11.18).
    Freecnt = 17,
    /// A command to run at the far end (11.19).
    Command = 18,
    /// Text for the far end's standard error. Not described in the document,
    /// which stops at 11.19, but it follows the sequence and is universal.
    Stderr = 19,
}

impl Kind {
    /// Read a type byte.
    pub fn from_byte(b: u8) -> Option<Self> {
        // Written out rather than transmuted, because a wrong byte from a
        // noisy line must be an error and not a value of this type.
        Some(match b {
            0 => Self::Rqinit,
            1 => Self::Rinit,
            2 => Self::Sinit,
            3 => Self::Ack,
            4 => Self::File,
            5 => Self::Skip,
            6 => Self::Nak,
            7 => Self::Abort,
            8 => Self::Fin,
            9 => Self::Rpos,
            10 => Self::Data,
            11 => Self::Eof,
            12 => Self::Ferr,
            13 => Self::Crc,
            14 => Self::Challenge,
            15 => Self::Compl,
            16 => Self::Can,
            17 => Self::Freecnt,
            18 => Self::Command,
            19 => Self::Stderr,
            _ => return None,
        })
    }

    /// The byte that goes on the line.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Whether data subpackets follow a frame of this type.
    ///
    /// Only three carry them: ZFILE its name and size, ZDATA the file, and
    /// ZSINIT the attention sequence. Everything else is a header alone, and
    /// 7.3.3 has the sender use a hex header "when they are not followed by
    /// binary data subpackets".
    pub fn carries_data(self) -> bool {
        matches!(self, Self::File | Self::Data | Self::Sinit | Self::Command)
    }
}

/// Receiver capability flags, in ZF0 and ZF1 of a ZRINIT (11.2).
pub mod capability {
    /// Rx can send and receive true full duplex.
    pub const FDX: u8 = 0o01;
    /// Rx can receive data during disk I/O.
    pub const OVERLAP_IO: u8 = 0o02;
    /// Rx can send a break signal.
    pub const BREAK: u8 = 0o04;
    /// Receiver can decrypt.
    pub const CRYPT: u8 = 0o10;
    /// Receiver can uncompress.
    pub const LZW: u8 = 0o20;
    /// Receiver can use a 32-bit frame check.
    pub const FC32: u8 = 0o40;
    /// Receiver expects control characters to be escaped.
    pub const ESCCTL: u8 = 0o100;
    /// Receiver expects the eighth bit to be escaped.
    pub const ESC8: u8 = 0o200;
}
