//! The sending half of a session (8.1 to 8.3, and 9 for recovery).
//!
//! Driven a chunk at a time, like everything else here: bytes arrive from the
//! far end, bytes are handed back for the line, and time passes through
//! [`Sender::tick`] rather than being read off a clock. Nothing here blocks
//! and nothing here owns a thread.
//!
//! The shape of a transfer, from 12.1:
//!
//! ```text
//!   Sender          Receiver
//!   ZRQINIT
//!                   ZRINIT
//!   ZFILE
//!                   ZRPOS
//!   ZDATA data ...
//!   ZEOF
//!                   ZRINIT
//!   ZFIN
//!                   ZFIN
//!   OO
//! ```

use super::file::FileInfo;
use super::header::{self, Header, HeaderError, Style};
use super::subpacket::{self, Ending};
use super::{CAN_TO_SEND, Kind, ZDLE, capability};

/// How long to wait for an answer before saying it again (8.1).
///
/// "The receive program resends its header at response time (default 10
/// second) intervals." The same patience serves the sender.
pub const RESPONSE_MS: u32 = 10_000;

/// How long to go on trying before giving up (8.1).
///
/// "for a suitable period of time (40 seconds total) before falling back to
/// YMODEM protocol". There is no YMODEM here to fall back to, so this is where
/// a transfer that will not start stops pretending.
pub const PATIENCE_MS: u32 = 40_000;

/// Where a transfer has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Asking the receiver to say what it can do.
    Greeting,
    /// Offering the file.
    Offering,
    /// Sending it.
    Sending,
    /// Sent, and saying so.
    Finishing,
    /// Over, and the file arrived.
    Done,
    /// Over, and it did not.
    Failed(Failure),
}

/// Why a transfer stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The far end never answered.
    NoAnswer,
    /// The far end cancelled, with ZABORT or the CAN sequence of 8.4.
    Cancelled,
    /// The far end did not want this file (11.6).
    Skipped,
    /// The far end could not write it (11.13).
    FarEndError,
}

/// What to show while it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub name: String,
    /// Bytes the far end has confirmed, or that have been sent where it has
    /// not been asked to confirm any.
    pub position: u64,
    pub total: Option<u64>,
    /// Times the far end asked to go back, which is what an error costs here.
    pub rewinds: u32,
    /// Bytes sent again because of those.
    pub resent: u64,
}

/// One file, going out.
#[derive(Debug)]
pub struct Sender {
    file: FileInfo,
    data: Vec<u8>,
    /// Where the next subpacket comes from.
    at: usize,
    /// The last position the receiver confirmed.
    confirmed: u64,
    state: State,
    out: Vec<u8>,
    /// What has arrived and not yet been made sense of.
    inbox: Vec<u8>,
    /// The last byte put on the line, for 7.2's carriage return rule.
    previous: u8,
    /// How big a subpacket to send (7.4).
    chunk: usize,
    /// Whether the receiver said it can take a 32-bit check (11.2).
    wide: bool,
    /// Whether it said it can receive while writing to disk (11.2). Without
    /// it, 8.2 requires ZCRCW so it has time to empty its buffer.
    overlap: bool,
    since_said: u32,
    waited: u32,
    /// Waiting for a ZACK before sending more (8.2's ZCRCW).
    holding: bool,
    /// A ZDATA header is owed before the next subpacket.
    ///
    /// Set rather than sent, because two ZRPOS headers can arrive in one
    /// chunk -- the receiver asks again on a timeout, and a repeat is
    /// cheap for it and free for the line -- and a ZDATA emitted for each
    /// would put two headers back to back. The second lands where the far
    /// end is expecting a subpacket, which is a header read as data.
    owe_header: bool,
    /// Whether a data frame is open and still needs closing.
    ///
    /// 8.2: "if the end of file is encountered within a frame, the frame is
    /// closed with a ZCRCE data subpacket". An empty file, or a resume that
    /// starts at the end, reaches the end without ever sending one -- and the
    /// frame still has to be closed or the ZEOF header arrives where a
    /// subpacket was expected.
    frame_open: bool,
    /// How much more the line will take before this end should stop.
    ///
    /// ZMODEM streams and clause 9 is about not stopping, but streaming down a
    /// line is not the same as streaming into a queue. Nothing below here
    /// pushes back -- hand it a megabyte and it will take a megabyte -- so a
    /// sender that fills the buffer has put a quarter of an hour of line into
    /// it, and 8.2's recovery is then a quarter of an hour long: the receiver
    /// asks to go back, and everything already queued is stale and still has
    /// to be sent before the answer to that question is even started.
    ///
    /// Measured on a real transfer: one rewind, half a megabyte resent, and
    /// the far end stopped dead at the position it had asked for.
    room: usize,
    rewinds: u32,
    resent: u64,
    /// Consecutive CANs from the far end (8.4).
    cans: usize,
}

impl Sender {
    /// A sender for one file, at a line rate that decides the subpacket size.
    pub fn new(file: FileInfo, data: Vec<u8>, bits_per_second: u32) -> Self {
        let mut me = Self {
            file,
            data,
            at: 0,
            confirmed: 0,
            state: State::Greeting,
            out: Vec::new(),
            inbox: Vec::new(),
            previous: 0,
            chunk: subpacket::recommended_length(bits_per_second),
            wide: false,
            overlap: false,
            since_said: 0,
            waited: 0,
            holding: false,
            owe_header: false,
            frame_open: false,
            // Somewhere to start before the caller says. Two seconds of line
            // is enough to keep it busy and short enough to throw away.
            room: (bits_per_second as usize / 4).max(1024),
            rewinds: 0,
            resent: 0,
            cans: 0,
        };
        me.greet();
        me
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Say how much more the line will take.
    ///
    /// Called by whatever owns the queue below, because that is the only thing
    /// that knows. Nothing is produced beyond it.
    pub fn set_room(&mut self, bytes: usize) {
        self.room = bytes;
        self.fill();
    }

    /// Bytes produced and not yet taken.
    pub fn pending(&self) -> usize {
        self.out.len()
    }

    pub fn progress(&self) -> Progress {
        Progress {
            name: self.file.name.clone(),
            position: self.confirmed.max(self.at as u64),
            total: self.file.length.or(Some(self.data.len() as u64)),
            rewinds: self.rewinds,
            resent: self.resent,
        }
    }

    /// Bytes for the line.
    pub fn take_out(&mut self) -> Vec<u8> {
        let out = std::mem::take(&mut self.out);
        if let Some(last) = out.last() {
            self.previous = *last;
        }
        out
    }

    /// Give up, politely (8.4).
    pub fn cancel(&mut self) {
        self.out.extend(cancel_sequence());
        self.state = State::Failed(Failure::Cancelled);
    }

    /// Time passing.
    pub fn tick(&mut self, ms: u32) {
        if matches!(self.state, State::Done | State::Failed(_)) {
            return;
        }
        self.since_said = self.since_said.saturating_add(ms);
        self.waited = self.waited.saturating_add(ms);
        if self.waited >= PATIENCE_MS {
            self.state = State::Failed(Failure::NoAnswer);
            return;
        }
        if self.since_said >= RESPONSE_MS {
            self.since_said = 0;
            // 8.1: "in case of garbled data, the sending program can repeat
            // the invitation to receive a number of times until a session
            // starts". The same goes for every other thing being waited on.
            match self.state {
                State::Greeting => self.greet(),
                State::Offering => self.offer(),
                State::Sending => {
                    self.holding = false;
                    self.fill();
                }
                State::Finishing => self.say(Header::position(Kind::Fin, 0), Style::Hex),
                _ => {}
            }
        }
    }

    /// Bytes from the far end.
    pub fn feed(&mut self, bytes: &[u8]) {
        if matches!(self.state, State::Done | State::Failed(_)) {
            return;
        }
        // 8.4's cancel sequence is eight CANs, and five is enough to act on.
        for &b in bytes {
            self.cans = if b == ZDLE { self.cans + 1 } else { 0 };
            if self.cans >= super::CAN_TO_ABORT {
                self.state = State::Failed(Failure::Cancelled);
                return;
            }
        }
        self.inbox.extend_from_slice(bytes);
        self.consume();
        self.fill();
    }

    /// Read whatever whole headers have arrived.
    fn consume(&mut self) {
        loop {
            let Some(start) = header::find(&self.inbox) else {
                // Nothing that could be a header. Anything before one is a
                // board talking to a person, so it is dropped rather than
                // kept: 8.1 has the sender "display a message intended for
                // human consumption" and the receiver may do the same.
                let keep = self.inbox.len().min(2);
                self.inbox.drain(..self.inbox.len() - keep);
                return;
            };
            self.inbox.drain(..start);
            match header::decode(&self.inbox) {
                Ok((h, _, used)) => {
                    self.inbox.drain(..used);
                    self.act(h);
                    if matches!(self.state, State::Done | State::Failed(_)) {
                        return;
                    }
                }
                Err(HeaderError::Incomplete) => return,
                Err(_) => {
                    // A header that did not survive the line. Step over its
                    // first byte so the hunt does not find the same one again,
                    // and let the far end repeat it.
                    self.inbox.drain(..1);
                }
            }
        }
    }

    fn act(&mut self, h: Header) {
        self.waited = 0;
        self.since_said = 0;
        match h.kind {
            // 11.2: ZF0 and ZF1 are the capability flags, and 8.1 has the
            // sender wait for this before offering anything.
            Kind::Rinit => {
                self.wide = h.zf0() & capability::FC32 != 0;
                self.overlap = h.zf0() & capability::OVERLAP_IO != 0;
                match self.state {
                    // 8.2: a ZRINIT after the file has been sent is the
                    // receiver saying it closed the file happily.
                    State::Finishing | State::Sending if self.finished() => {
                        self.state = State::Finishing;
                        self.say(Header::position(Kind::Fin, 0), Style::Hex);
                    }
                    // Only from the start. 8.1 has the receiver send a ZRINIT
                    // when it starts *and* again for every ZRQINIT, so two of
                    // them at the beginning is the ordinary case -- and a
                    // second offer draws a second ZRPOS, which is a second
                    // copy of the file behind the first. Offering again where
                    // one was lost is the timer's job, not this one's.
                    State::Greeting => self.offer(),
                    _ => {}
                }
            }
            // 8.2: "a ZRPOS header from the receiver initiates transmission of
            // the file data starting at the offset in the file specified".
            // Which is both how a transfer starts and how it recovers.
            Kind::Rpos => {
                let at = h.to_position() as usize;
                // Behind where this end had got to, whatever state it is in.
                // The sender streams: by the time a ZRPOS crosses the line the
                // file may already be sent and the state Finishing, and that
                // is exactly the case worth counting -- everything between
                // here and there has to go again.
                if at < self.at {
                    self.rewinds += 1;
                    self.resent += (self.at - at) as u64;
                }
                self.at = at.min(self.data.len());
                self.confirmed = self.at as u64;
                self.holding = false;
                self.state = State::Sending;
                self.owe_header = true;
                self.frame_open = false;
            }
            // 8.2, for ZCRCQ and ZCRCW: "expect a ZACK response with the
            // receiver's file offset".
            Kind::Ack => {
                self.confirmed = u64::from(h.to_position());
                self.holding = false;
            }
            Kind::Skip => self.state = State::Failed(Failure::Skipped),
            Kind::Ferr => self.state = State::Failed(Failure::FarEndError),
            Kind::Abort | Kind::Can => self.state = State::Failed(Failure::Cancelled),
            // 8.3: "the sender closes the session with a ZFIN header. The
            // receiver acknowledges this with its own ZFIN header. When the
            // sender receives the acknowledging header, it sends two
            // characters, OO (Over and Out)."
            Kind::Fin => {
                self.out.extend(b"OO");
                self.state = State::Done;
            }
            // 8.1: "if the receiving program receives a ZRQINIT header, it
            // resends the ZRINIT header" -- and one arriving here is an echo,
            // which says the far end is hearing itself and not us.
            Kind::Rqinit | Kind::Nak => self.repeat(),
            _ => {}
        }
    }

    fn repeat(&mut self) {
        match self.state {
            State::Greeting => self.greet(),
            State::Offering => self.offer(),
            _ => {}
        }
    }

    /// 8.1: "the sending program may send the string `rz\r` to invoke the
    /// receiving program from a possible command mode", and then a ZRQINIT.
    fn greet(&mut self) {
        self.out.extend(b"rz\r");
        self.say(Header::flags(Kind::Rqinit, 0, 0, 0, 0), Style::Hex);
        self.state = State::Greeting;
    }

    /// 8.2: a ZFILE header and "a ZCRCW data subpacket containing the file
    /// name, file length, modification date".
    fn offer(&mut self) {
        self.state = State::Offering;
        self.say(Header::flags(Kind::File, 0, 0, 0, 0), self.binary());
        let body = self.file.encode();
        let bytes = subpacket::encode(&body, Ending::Wait, self.wide, self.previous);
        self.out.extend(bytes);
    }

    /// Keep the line fed while there is file left and nothing to wait for.
    fn fill(&mut self) {
        if self.state != State::Sending || self.holding {
            return;
        }
        // 8.2: "the sender sends a ZDATA binary header (with file position)
        // followed by one or more data subpackets". One header, however many
        // times the position was asked for.
        if self.owe_header {
            self.owe_header = false;
            self.frame_open = true;
            self.say(Header::position(Kind::Data, self.at as u32), self.binary());
        }
        while self.at < self.data.len() && !self.holding && self.out.len() < self.room {
            let end = (self.at + self.chunk).min(self.data.len());
            let chunk = self.data[self.at..end].to_vec();
            let last = end >= self.data.len();
            // 8.2: ZCRCE closes a frame at end of file and "does not elicit a
            // response except in case of error"; ZCRCG keeps a frame going and
            // is what streaming is made of; ZCRCW is used where the receiver
            // "does not indicate overlapped I/O capability", to give it time
            // to write its buffer.
            let ending = if last {
                Ending::End
            } else if self.overlap {
                Ending::Go
            } else {
                Ending::Wait
            };
            let bytes = subpacket::encode(&chunk, ending, self.wide, self.previous);
            self.previous = *chunk.last().unwrap_or(&self.previous);
            self.out.extend(bytes);
            self.at = end;
            self.holding = ending.waits();
            self.frame_open = ending == Ending::Go;
        }
        // Only once the file has actually all gone out, rather than once the
        // loop stopped -- which it also does when the line is full.
        if self.finished() && !self.holding {
            // An empty file, or a resume that begins at the end, gets here
            // with a frame open and nothing sent in it. 8.2 closes a frame
            // with a ZCRCE subpacket, and an empty one is still one.
            if self.frame_open {
                self.frame_open = false;
                let bytes = subpacket::encode(&[], Ending::End, self.wide, self.previous);
                self.out.extend(bytes);
            }
            // 8.2: "the sender sends a ZEOF header with the file ending offset
            // equal to the number of characters in the file".
            self.say(Header::position(Kind::Eof, self.data.len() as u32), self.binary());
            self.state = State::Finishing;
        }
    }

    fn finished(&self) -> bool {
        self.at >= self.data.len()
    }

    /// 7.3.2: a 32-bit binary header only where the receiver said it can take
    /// one, "iff the receiver indicates the capability with the CANFC32 bit".
    fn binary(&self) -> Style {
        if self.wide { Style::Binary32 } else { Style::Binary16 }
    }

    fn say(&mut self, h: Header, style: Style) {
        let bytes = h.encode(style);
        if let Some(last) = bytes.last() {
            self.previous = *last;
        }
        self.out.extend(bytes);
        self.since_said = 0;
    }
}

/// 8.4's cancel sequence: eight CANs and ten backspaces.
///
/// "ZMODEM only requires five Cancel characters, the other three are
/// insurance. The trailing backspace characters attempt to erase the effects
/// of the CAN characters if they are received by a command interpreter."
pub fn cancel_sequence() -> Vec<u8> {
    let mut out = vec![ZDLE; CAN_TO_SEND];
    out.extend([0x08; 10]);
    out
}
