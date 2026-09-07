//! The receiving half of a session (8.1 to 8.3, and 9 for recovery).
//!
//! The receiver's job is mostly to say where it has got to. 8.2: "the receiver
//! compares the file position in the ZDATA header with the number of
//! characters successfully received to the file. If they do not agree, a ZRPOS
//! error response is generated to force the sender to the right position
//! within the file."
//!
//! That single rule is the whole of the error recovery. There is no window to
//! keep, no gaps to remember and no blocks to number: a subpacket that fails
//! its check is one the receiver simply did not count, and saying the count
//! puts the sender back where it belongs.

use super::file::FileInfo;
use super::header::{self, Header, HeaderError, Style};
use super::send::{Failure, PATIENCE_MS, RESPONSE_MS, State, cancel_sequence};
use super::subpacket::{self, SubpacketError};
use super::{Kind, ZDLE, capability};

/// What this end can do, for the ZF0 of its ZRINIT (11.2).
///
/// Full duplex, and a 32-bit check because the line is worth checking properly
/// at a megabyte. Overlapped I/O is claimed because the file is assembled in
/// memory here and there is no disk write to be caught behind -- which is the
/// thing that bit is really about, and claiming it without meaning it is how a
/// receiver drops data.
const CAPABILITIES: u8 = capability::FDX | capability::OVERLAP_IO | capability::FC32;

/// What the receiver has, so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub file: FileInfo,
    pub data: Vec<u8>,
}

/// One file, coming in.
#[derive(Debug)]
pub struct Receiver {
    state: State,
    file: Option<FileInfo>,
    data: Vec<u8>,
    out: Vec<u8>,
    inbox: Vec<u8>,
    previous: u8,
    /// Whether the current frame's subpackets carry a 32-bit check, which is
    /// decided by the header that opened it (7.3.2).
    wide: bool,
    /// Inside a ZDATA frame, so bytes are subpackets rather than headers.
    in_data: bool,
    since_said: u32,
    waited: u32,
    /// Subpackets that failed their check, which is what the line cost.
    bad: u32,
    /// Times this end had to send the sender back.
    rewinds: u32,
    /// Whether a ZEOF has arrived whose length matched what was received.
    ///
    /// The difference between a session that ended and a file that arrived.
    /// 8.2 puts the test in the receiver's hands -- "the receiver compares
    /// this number with the number of characters received" -- and without it
    /// a session torn down early hands up whatever happened to be in the
    /// buffer as though it were the file. Under heavy damage that was an
    /// empty file reported as complete, which is worse than any failure.
    complete: bool,
    cans: usize,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    pub fn new() -> Self {
        let mut me = Self {
            state: State::Greeting,
            file: None,
            data: Vec::new(),
            out: Vec::new(),
            inbox: Vec::new(),
            previous: 0,
            wide: false,
            in_data: false,
            since_said: 0,
            waited: 0,
            bad: 0,
            rewinds: 0,
            complete: false,
            cans: 0,
        };
        // 8.1: "when the ZMODEM receive program starts, it immediately sends a
        // ZRINIT header to initiate ZMODEM file transfers".
        me.announce();
        me
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Subpackets that failed their check sequence.
    pub fn damaged(&self) -> u32 {
        self.bad
    }

    pub fn progress(&self) -> super::send::Progress {
        super::send::Progress {
            name: self.file.as_ref().map(|f| f.name.clone()).unwrap_or_default(),
            position: self.data.len() as u64,
            total: self.file.as_ref().and_then(|f| f.length),
            rewinds: self.rewinds,
            resent: 0,
        }
    }

    /// The file, once it is all here.
    ///
    /// All of it: a session that ended is not the same as a file that
    /// arrived, and only a ZEOF whose length matched what was counted says
    /// the second.
    pub fn finished(&self) -> Option<Received> {
        match (self.state, self.complete, &self.file) {
            (State::Done, true, Some(file)) => {
                Some(Received { file: file.clone(), data: self.data.clone() })
            }
            _ => None,
        }
    }

    pub fn take_out(&mut self) -> Vec<u8> {
        let out = std::mem::take(&mut self.out);
        if let Some(last) = out.last() {
            self.previous = *last;
        }
        out
    }

    pub fn cancel(&mut self) {
        self.out.extend(cancel_sequence());
        self.state = State::Failed(Failure::Cancelled);
    }

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
        // 8.1: "the receive program resends its header at response time
        // (default 10 second) intervals".
        if self.since_said >= RESPONSE_MS {
            match self.state {
                State::Greeting => self.announce(),
                State::Sending => self.rewind(),
                _ => {}
            }
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if matches!(self.state, State::Done | State::Failed(_)) {
            return;
        }
        for &b in bytes {
            self.cans = if b == ZDLE { self.cans + 1 } else { 0 };
            if self.cans >= super::CAN_TO_ABORT {
                self.state = State::Failed(Failure::Cancelled);
                return;
            }
        }
        self.inbox.extend_from_slice(bytes);
        self.consume();
    }

    fn consume(&mut self) {
        loop {
            if self.in_data {
                match subpacket::decode(&self.inbox, self.wide) {
                    Ok((packet, used)) => {
                        self.inbox.drain(..used);
                        self.waited = 0;
                        // 8.2: the subpacket after a ZFILE is the name and
                        // length, not the file. The only place a subpacket
                        // means something other than data.
                        if self.state == State::Offering {
                            self.in_data = false;
                            self.take_offer(&packet.data);
                            continue;
                        }
                        self.data.extend_from_slice(&packet.data);
                        // 8.2: ZCRCQ and ZCRCW "expect a ZACK response with
                        // the receiver's file offset". ZCRCG and ZCRCE do not.
                        if packet.ending.acknowledged() {
                            self.say(
                                Header::position(Kind::Ack, self.data.len() as u32),
                                Style::Hex,
                            );
                        }
                        if !matches!(packet.ending, super::subpacket::Ending::Go) {
                            self.in_data = false;
                        }
                    }
                    Err(SubpacketError::Incomplete) => return,
                    Err(_) => {
                        // 8.2's whole recovery: a subpacket that failed is one
                        // this end did not count, and saying the count is what
                        // puts the sender back. No gaps to remember, because
                        // nothing past the gap was accepted.
                        self.bad += 1;
                        self.in_data = false;
                        self.inbox.clear();
                        self.rewind();
                        return;
                    }
                }
                continue;
            }
            let Some(start) = header::find(&self.inbox) else {
                // 8.3: the sender ends with "OO", which is not a header and is
                // the one thing worth looking for in what is otherwise a board
                // talking to a person.
                if self.state == State::Finishing && self.inbox.windows(2).any(|w| w == b"OO") {
                    self.state = State::Done;
                    return;
                }
                let keep = self.inbox.len().min(2);
                self.inbox.drain(..self.inbox.len() - keep);
                return;
            };
            self.inbox.drain(..start);
            match header::decode(&self.inbox) {
                Ok((h, style, used)) => {
                    self.inbox.drain(..used);
                    self.wide = style == Style::Binary32;
                    self.act(h);
                    if matches!(self.state, State::Done | State::Failed(_)) {
                        return;
                    }
                }
                Err(HeaderError::Incomplete) => return,
                Err(_) => {
                    self.inbox.drain(..1);
                }
            }
        }
    }

    fn act(&mut self, h: Header) {
        self.waited = 0;
        match h.kind {
            // 8.1: "if the receiving program receives a ZRQINIT header, it
            // resends the ZRINIT header".
            Kind::Rqinit => self.announce(),
            // 8.2: the receiver "examines the file name, length, and date
            // information provided by the sender", and a ZRPOS starts the
            // data. Position 0, because nothing here resumes yet.
            Kind::File => {
                self.state = State::Offering;
                self.in_data = true;
            }
            // 8.2: "the sender sends a ZDATA binary header (with file
            // position) ... the receiver compares the file position in the
            // ZDATA header with the number of characters successfully
            // received. If they do not agree, a ZRPOS error response is
            // generated."
            Kind::Data => {
                self.state = State::Sending;
                if u64::from(h.to_position()) != self.data.len() as u64 {
                    self.rewind();
                } else {
                    self.in_data = true;
                }
            }
            // 8.2: "the receiver compares this number with the number of
            // characters received. If the receiver has received all of the
            // file, it closes the file ... the receiver responds with ZRINIT.
            // If the receiver has not received all the bytes of the file, the
            // receiver ignores the ZEOF because a new ZDATA is coming."
            Kind::Eof => {
                if u64::from(h.to_position()) == self.data.len() as u64 {
                    self.complete = true;
                    self.state = State::Finishing;
                    self.announce();
                } else {
                    self.rewind();
                }
            }
            // 8.3: "the receiver acknowledges this with its own ZFIN header".
            Kind::Fin => {
                self.state = State::Finishing;
                self.say(Header::position(Kind::Fin, 0), Style::Hex);
            }
            Kind::Abort | Kind::Can => self.state = State::Failed(Failure::Cancelled),
            _ => {}
        }
    }

    /// The file's name and size, from the subpacket after a ZFILE.
    ///
    /// Kept apart from [`Self::act`] because it arrives as data rather than as
    /// a header, and because a name from a board is the one part of a transfer
    /// that decides where bytes land.
    fn take_offer(&mut self, body: &[u8]) {
        match FileInfo::parse(body) {
            Some(file) => {
                self.file = Some(file);
                self.data.clear();
                self.complete = false;
                self.state = State::Sending;
                self.rewind_to(0);
            }
            // A ZFILE nobody can read is not a file to start writing.
            None => self.cancel(),
        }
    }

    /// Say where this end has got to (8.2).
    fn rewind(&mut self) {
        self.rewinds += 1;
        let at = self.data.len() as u32;
        self.rewind_to(at);
    }

    fn rewind_to(&mut self, at: u32) {
        self.in_data = false;
        self.say(Header::position(Kind::Rpos, at), Style::Hex);
    }

    /// 11.2: the capability flags and the buffer size, which is zero here
    /// because "ZP0 and ZP1 contain the size of the receiver's buffer in
    /// bytes, or 0 if nonstop I/O is allowed".
    fn announce(&mut self) {
        self.say(Header::flags(Kind::Rinit, 0, 0, 0, CAPABILITIES), Style::Hex);
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
