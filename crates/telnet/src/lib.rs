//! Telnet, RFC 854, as much of it as a terminal onto a bulletin board needs.
//!
//! This exists so the terminal can be exercised without a line under it. What
//! a board sends is the same either way — the same ANSI, the same CP437, the
//! same cursor addressing — and separating "did the screen draw correctly"
//! from "did every byte survive the modem" is worth a great deal when both are
//! being written at once. A capture answers the first question only if the
//! second was already answered; this answers it directly.
//!
//! What is here is the option negotiation and the escaping, which are the two
//! parts a socket does not give you for nothing. Everything else about telnet
//! — the line mode, the timing marks, the network virtual printer — belongs to
//! a world of teletypes on time-sharing machines and no board has ever used
//! any of it.
//!
//! The shape is the workspace's: [`Telnet::feed`] takes one byte and hands
//! back the data byte it turned into, if it turned into one at all. Anything
//! the far end has to be told in reply accumulates, and is collected with
//! [`Telnet::take_reply`]. Nothing blocks and nothing owns a socket, so the
//! whole protocol can be tested without one.

#![forbid(unsafe_code)]

/// Interpret As Command: the escape that introduces everything below.
pub const IAC: u8 = 255;
/// "Stop doing that", or "I will not".
pub const DONT: u8 = 254;
/// "Please do that."
pub const DO: u8 = 253;
/// "I refuse to do that."
pub const WONT: u8 = 252;
/// "I am willing to do that."
pub const WILL: u8 = 251;
/// Subnegotiation begins.
pub const SB: u8 = 250;
/// Go ahead: the half-duplex turn marker, suppressed on every real connection.
pub const GA: u8 = 249;
/// Subnegotiation ends.
pub const SE: u8 = 240;

/// The options this end has an opinion about.
pub mod option {
    /// RFC 856. Eight-bit data both ways.
    ///
    /// Not optional in practice. Board art is CP437, which is to say that more
    /// than half of what arrives has its top bit set, and NVT ASCII is defined
    /// as seven bits.
    pub const BINARY: u8 = 0;
    /// RFC 857. The far end echoes what is typed at it.
    pub const ECHO: u8 = 1;
    /// RFC 858. No go-aheads, which is what makes the connection full duplex
    /// and character at a time rather than line at a time.
    pub const SUPPRESS_GO_AHEAD: u8 = 3;
    /// RFC 1091. The far end asks what sort of terminal this is.
    pub const TERMINAL_TYPE: u8 = 24;
    /// RFC 1073. How large the window is, and when it changes.
    pub const NAWS: u8 = 31;
}

use option::{BINARY, ECHO, NAWS, SUPPRESS_GO_AHEAD, TERMINAL_TYPE};

/// Subnegotiation verbs for [`option::TERMINAL_TYPE`], RFC 1091.
const TT_IS: u8 = 0;
const TT_SEND: u8 = 1;

/// Anything longer than this in one subnegotiation is not a subnegotiation.
///
/// The only ones answered here are a handful of bytes. A far end that opens
/// one and never closes it would otherwise grow this buffer for as long as the
/// connection lasts.
const SB_LIMIT: usize = 256;

/// Where the byte stream is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    /// An `IAC WILL`, `WONT`, `DO` or `DONT` waiting for its option code.
    Negotiate(u8),
    /// Inside `IAC SB` ... `IAC SE`.
    Sub,
    /// An `IAC` inside a subnegotiation, which is either an escaped 255 or
    /// the `SE` that ends it.
    SubIac,
}

/// One end of a telnet connection: the option state and the escaping.
#[derive(Debug)]
pub struct Telnet {
    state: State,
    /// Options the far end is performing, because it offered and was accepted.
    him: [bool; 256],
    /// Options this end is performing.
    us: [bool; 256],
    /// Options this end has already sent a `WILL` for and not withdrawn.
    us_offered: [bool; 256],
    /// Options this end has already sent a `DO` for and not withdrawn.
    him_requested: [bool; 256],
    sub: Vec<u8>,
    reply: Vec<u8>,
    /// Whether the byte just handed out was a carriage return, so that the NUL
    /// which RFC 854 requires after one can be dropped.
    after_cr: bool,
    terminal_type: String,
    cols: u16,
    rows: u16,
}

impl Telnet {
    /// A client, ready to say hello.
    ///
    /// `terminal_type` is what the far end will be told when it asks, and for
    /// a board the answer that unlocks the colour is `ANSI`. Boards branch on
    /// this: answer `dumb` or say nothing and a great many of them will send
    /// the plain ASCII menus instead, which is precisely the thing this mode
    /// exists to look at.
    pub fn new(terminal_type: &str, cols: u16, rows: u16) -> Self {
        let mut telnet = Self {
            state: State::Data,
            him: [false; 256],
            us: [false; 256],
            us_offered: [false; 256],
            him_requested: [false; 256],
            sub: Vec::new(),
            reply: Vec::new(),
            after_cr: false,
            terminal_type: terminal_type.to_owned(),
            cols,
            rows,
        };
        telnet.greet();
        telnet
    }

    /// Open the negotiation rather than waiting to be asked.
    ///
    /// A client is entitled to wait — most boards start negotiating the moment
    /// the socket opens. Some do not, and one that does not will happily leave
    /// the connection in NVT ASCII line mode, where the art arrives with its
    /// top bits stripped and nothing is sent until a whole line has been
    /// typed. Asking first costs six commands and removes the question.
    fn greet(&mut self) {
        for opt in [SUPPRESS_GO_AHEAD, BINARY] {
            self.request(opt);
            self.offer(opt);
        }
        // Not echo: this end never echoes for the far end, it only accepts
        // being echoed to. A terminal that offered to echo would be offering
        // to be a host.
        for opt in [TERMINAL_TYPE, NAWS] {
            self.offer(opt);
        }
    }

    /// One byte from the socket. Returns the data byte it was, if it was one.
    ///
    /// Most bytes are data and come straight back. The rest are commands, and
    /// what they demand in reply is queued for [`take_reply`](Self::take_reply).
    pub fn feed(&mut self, byte: u8) -> Option<u8> {
        match self.state {
            State::Data => self.data(byte),
            State::Iac => {
                self.state = State::Data;
                match byte {
                    // Doubled: one real 255. The reason this escaping cannot
                    // be skipped even on a connection carrying nothing but
                    // text -- CP437 puts a character at 255, and it is the
                    // hard space that board art is padded with.
                    IAC => Some(IAC),
                    WILL | WONT | DO | DONT => {
                        self.state = State::Negotiate(byte);
                        None
                    }
                    SB => {
                        self.sub.clear();
                        self.state = State::Sub;
                        None
                    }
                    // Go ahead, no-op, and the rest of the two-byte commands.
                    // Nothing here is half duplex, so there is nothing to do
                    // with a turn marker but discard it.
                    _ => None,
                }
            }
            State::Negotiate(verb) => {
                self.state = State::Data;
                self.negotiate(verb, byte);
                None
            }
            State::Sub => {
                if byte == IAC {
                    self.state = State::SubIac;
                } else if self.sub.len() < SB_LIMIT {
                    self.sub.push(byte);
                }
                None
            }
            State::SubIac => {
                match byte {
                    SE => {
                        self.state = State::Data;
                        self.subnegotiate();
                    }
                    // An escaped 255 inside the body. Without this a window
                    // size of 255 columns would end the subnegotiation early.
                    IAC => {
                        self.state = State::Sub;
                        if self.sub.len() < SB_LIMIT {
                            self.sub.push(IAC);
                        }
                    }
                    // Anything else is a far end that has lost its place.
                    // Abandoning the body is safer than trusting it.
                    _ => {
                        self.state = State::Data;
                        self.sub.clear();
                    }
                }
                None
            }
        }
    }

    /// Feed a whole buffer, appending the data bytes to `out`.
    pub fn feed_bytes(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            if let Some(data) = self.feed(b) {
                out.push(data);
            }
        }
    }

    fn data(&mut self, byte: u8) -> Option<u8> {
        let was_after_cr = std::mem::replace(&mut self.after_cr, false);
        match byte {
            IAC => {
                self.state = State::Iac;
                // A command may be inserted anywhere, including between a
                // carriage return and the NUL that pads it, so the CR has to
                // survive the interruption.
                self.after_cr = was_after_cr;
                None
            }
            // RFC 854: in NVT ASCII a carriage return that is not part of a
            // new line is sent as CR NUL, so that CR keeps its meaning of "go
            // to the left margin" and nothing else. Handing the NUL on would
            // put a character on the screen -- CP437 has one at zero.
            0 if was_after_cr && !self.him[BINARY as usize] => None,
            b'\r' => {
                self.after_cr = true;
                Some(byte)
            }
            _ => Some(byte),
        }
    }

    fn negotiate(&mut self, verb: u8, opt: u8) {
        let i = opt as usize;
        match verb {
            // "I am willing to." Accepted for the few worth having, refused
            // for everything else. Refusing by default is the rule that keeps
            // this small: an option nobody here understands is an option that
            // would change the byte stream in a way nothing here would undo.
            WILL => {
                if matches!(opt, BINARY | ECHO | SUPPRESS_GO_AHEAD) {
                    // Only answer when the answer changes something. Replying
                    // to an offer already accepted is how two ends that both
                    // acknowledge everything talk to each other until the
                    // connection is closed.
                    if !self.him[i] {
                        self.him[i] = true;
                        if !self.him_requested[i] {
                            self.him_requested[i] = true;
                            self.command(DO, opt);
                        }
                    }
                } else {
                    self.command(DONT, opt);
                }
            }
            WONT => {
                self.him_requested[i] = false;
                if self.him[i] {
                    self.him[i] = false;
                    self.command(DONT, opt);
                }
            }
            // "Please do." Same shape, the other way round.
            DO => {
                if matches!(opt, BINARY | SUPPRESS_GO_AHEAD | TERMINAL_TYPE | NAWS) {
                    if !self.us[i] {
                        self.us[i] = true;
                        if !self.us_offered[i] {
                            self.us_offered[i] = true;
                            self.command(WILL, opt);
                        }
                        // RFC 1073: the size follows the agreement rather than
                        // waiting to be asked for, because there is no way to
                        // ask for it.
                        if opt == NAWS {
                            self.send_naws();
                        }
                    }
                } else {
                    self.command(WONT, opt);
                }
            }
            DONT => {
                self.us_offered[i] = false;
                if self.us[i] {
                    self.us[i] = false;
                    self.command(WONT, opt);
                }
            }
            // Nothing else reaches here: the verb came from the four cases
            // that put this state machine into `Negotiate` at all.
            _ => {}
        }
    }

    fn subnegotiate(&mut self) {
        // Taken rather than borrowed, so the answer can be built while the
        // question is still readable. Put back afterwards to keep the buffer
        // this connection has already grown.
        let body = std::mem::take(&mut self.sub);
        // RFC 1091: asked what we are. The same name every time, which is also
        // how the exchange terminates -- a far end cycling through a client's
        // list stops when a name repeats.
        if let [TERMINAL_TYPE, TT_SEND, ..] = body.as_slice() {
            let name = self.terminal_type.clone();
            self.reply.extend_from_slice(&[IAC, SB, TERMINAL_TYPE, TT_IS]);
            self.reply.extend_from_slice(name.as_bytes());
            self.reply.extend_from_slice(&[IAC, SE]);
        }
        self.sub = body;
        self.sub.clear();
    }

    fn send_naws(&mut self) {
        let (c, r) = (self.cols.to_be_bytes(), self.rows.to_be_bytes());
        self.reply.extend_from_slice(&[IAC, SB, NAWS]);
        // The body is escaped like any other stream, and a window 255 columns
        // wide is not far-fetched enough to leave that to chance.
        for b in [c[0], c[1], r[0], r[1]] {
            self.reply.push(b);
            if b == IAC {
                self.reply.push(IAC);
            }
        }
        self.reply.extend_from_slice(&[IAC, SE]);
    }

    fn offer(&mut self, opt: u8) {
        self.us_offered[opt as usize] = true;
        self.command(WILL, opt);
    }

    fn request(&mut self, opt: u8) {
        self.him_requested[opt as usize] = true;
        self.command(DO, opt);
    }

    fn command(&mut self, verb: u8, opt: u8) {
        self.reply.extend_from_slice(&[IAC, verb, opt]);
    }

    /// Everything the far end is owed, and nothing after it is collected.
    pub fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.reply)
    }

    /// Encode bytes on their way out: escape the escape, and mind the CR.
    pub fn encode(&self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            match b {
                IAC => out.extend_from_slice(&[IAC, IAC]),
                // The transmit half of the rule in [`data`]. Once this end is
                // sending eight-bit data a carriage return means itself and
                // needs no help; until then it has to be told apart from the
                // start of a new line.
                b'\r' if !self.us[BINARY as usize] => out.extend_from_slice(b"\r\0"),
                _ => out.push(b),
            }
        }
    }

    /// Tell the far end the window changed, if it asked to be told.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        if self.us[NAWS as usize] {
            self.send_naws();
        }
    }

    /// Whether the far end has taken on echoing what is typed.
    ///
    /// Worth knowing because the alternative is echoing locally, and doing
    /// both puts every character on the screen twice.
    pub fn echo(&self) -> bool {
        self.him[ECHO as usize]
    }

    /// Whether eight-bit data has been agreed in the receive direction.
    pub fn binary(&self) -> bool {
        self.him[BINARY as usize]
    }

    /// Whether an option is enabled on the far end.
    pub fn far_end(&self, opt: u8) -> bool {
        self.him[opt as usize]
    }

    /// Whether an option is enabled on this end.
    pub fn near_end(&self, opt: u8) -> bool {
        self.us[opt as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a slice, returning the data that came out of it.
    fn data(t: &mut Telnet, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        t.feed_bytes(bytes, &mut out);
        out
    }

    fn client() -> Telnet {
        let mut t = Telnet::new("ANSI", 80, 24);
        // The opening offers are not what any of these tests are about.
        t.take_reply();
        t
    }

    #[test]
    fn plain_text_passes_through_untouched() {
        let mut t = client();
        assert_eq!(data(&mut t, b"Welcome to the board\r\n"), b"Welcome to the board\r\n");
        assert!(t.take_reply().is_empty(), "answered something nobody asked");
    }

    #[test]
    fn every_high_byte_survives() {
        // The whole point of the exercise. Board art is CP437 and lives in the
        // top half of the byte range; a stream where any of it went missing
        // would draw as holes in the boxes.
        let mut t = client();
        let mut sent = Vec::new();
        for b in 0x80u8..=0xfe {
            sent.push(b);
        }
        assert_eq!(data(&mut t, &sent), sent);
    }

    #[test]
    fn a_doubled_escape_is_one_data_byte() {
        // 255 is a character in CP437 -- the hard space -- so this is not a
        // corner case, it is what arrives in the middle of ordinary art.
        let mut t = client();
        assert_eq!(data(&mut t, &[b'a', IAC, IAC, b'b']), vec![b'a', 255, b'b']);
    }

    #[test]
    fn an_offer_worth_having_is_accepted() {
        let mut t = client();
        assert!(data(&mut t, &[IAC, WILL, ECHO]).is_empty());
        assert_eq!(t.take_reply(), vec![IAC, DO, ECHO]);
        assert!(t.echo(), "agreed to be echoed to and did not remember");
    }

    #[test]
    fn an_offer_nobody_understands_is_refused() {
        // Refusing by default is what keeps this small. An option accepted
        // without being implemented would change the byte stream in a way
        // nothing here would undo.
        let mut t = client();
        const STATUS: u8 = 5;
        data(&mut t, &[IAC, WILL, STATUS]);
        assert_eq!(t.take_reply(), vec![IAC, DONT, STATUS]);
        assert!(!t.far_end(STATUS));
    }

    #[test]
    fn a_request_nobody_understands_is_refused() {
        let mut t = client();
        const LINEMODE: u8 = 34;
        data(&mut t, &[IAC, DO, LINEMODE]);
        assert_eq!(t.take_reply(), vec![IAC, WONT, LINEMODE]);
        assert!(!t.near_end(LINEMODE));
    }

    #[test]
    fn an_offer_already_accepted_is_not_answered_again() {
        // The loop this avoids is not hypothetical: two ends that acknowledge
        // every acknowledgement will fill the socket with WILL and DO until
        // one of them gives up, and neither will ever send a byte of text.
        // Echo, because it is the one option accepted here that the greeting
        // does not already ask for, so the first WILL really is the first
        // anybody has said about it.
        let mut t = client();
        data(&mut t, &[IAC, WILL, ECHO]);
        assert_eq!(t.take_reply(), vec![IAC, DO, ECHO]);
        data(&mut t, &[IAC, WILL, ECHO]);
        assert!(t.take_reply().is_empty(), "answered the same offer twice");
    }

    #[test]
    fn an_option_this_end_opened_is_not_confirmed_again() {
        // The opening greeting has already said WILL SGA. The far end's DO is
        // the answer to that, not a fresh request, and answering it would be
        // the same loop from the other side.
        let mut t = Telnet::new("ANSI", 80, 24);
        let greeting = t.take_reply();
        assert!(
            greeting.windows(3).any(|w| w == [IAC, WILL, SUPPRESS_GO_AHEAD]),
            "did not open the negotiation: {greeting:?}"
        );
        data(&mut t, &[IAC, DO, SUPPRESS_GO_AHEAD]);
        assert!(t.take_reply().is_empty(), "confirmed its own offer");
        assert!(t.near_end(SUPPRESS_GO_AHEAD));
    }

    #[test]
    fn a_withdrawal_is_acknowledged_once() {
        let mut t = client();
        data(&mut t, &[IAC, WILL, ECHO]);
        t.take_reply();
        data(&mut t, &[IAC, WONT, ECHO]);
        assert_eq!(t.take_reply(), vec![IAC, DONT, ECHO]);
        assert!(!t.echo());
        data(&mut t, &[IAC, WONT, ECHO]);
        assert!(t.take_reply().is_empty(), "acknowledged a withdrawal twice");
    }

    #[test]
    fn the_board_is_told_this_is_an_ansi_terminal() {
        // The answer that decides whether the art arrives at all. Boards
        // branch on this name, and one that hears nothing sends the plain
        // menus -- which would make a terminal with a broken ANSI parser look
        // as though it were working.
        let mut t = client();
        // The greeting already offered, so this is the board agreeing and
        // there is nothing to say back to it.
        data(&mut t, &[IAC, DO, TERMINAL_TYPE]);
        assert!(t.take_reply().is_empty(), "confirmed its own offer");
        assert!(t.near_end(TERMINAL_TYPE));

        data(&mut t, &[IAC, SB, TERMINAL_TYPE, TT_SEND, IAC, SE]);
        let mut want = vec![IAC, SB, TERMINAL_TYPE, TT_IS];
        want.extend_from_slice(b"ANSI");
        want.extend_from_slice(&[IAC, SE]);
        assert_eq!(t.take_reply(), want);
    }

    #[test]
    fn the_same_name_comes_back_every_time() {
        // RFC 1091 ends the exchange when a name repeats, so answering the
        // same thing twice is how this terminates rather than a shortcut.
        let mut t = client();
        data(&mut t, &[IAC, DO, TERMINAL_TYPE]);
        t.take_reply();
        data(&mut t, &[IAC, SB, TERMINAL_TYPE, TT_SEND, IAC, SE]);
        let first = t.take_reply();
        data(&mut t, &[IAC, SB, TERMINAL_TYPE, TT_SEND, IAC, SE]);
        assert_eq!(t.take_reply(), first);
    }

    #[test]
    fn the_window_size_follows_the_agreement_unasked() {
        // RFC 1073 has no way to ask for it, so a client that waits to be
        // asked never sends it and the board lays out for whatever it guesses.
        let mut t = client();
        data(&mut t, &[IAC, DO, NAWS]);
        let mut want = vec![IAC, SB, NAWS];
        want.extend_from_slice(&[0, 80, 0, 24]);
        want.extend_from_slice(&[IAC, SE]);
        assert_eq!(t.take_reply(), want);
    }

    #[test]
    fn a_resize_is_reported_only_once_and_only_if_agreed() {
        let mut t = client();
        t.resize(132, 43);
        assert!(t.take_reply().is_empty(), "reported a size nobody agreed to hear");

        data(&mut t, &[IAC, DO, NAWS]);
        t.take_reply();
        t.resize(132, 50);
        let mut want = vec![IAC, SB, NAWS];
        want.extend_from_slice(&[0, 132, 0, 50]);
        want.extend_from_slice(&[IAC, SE]);
        assert_eq!(t.take_reply(), want);

        t.resize(132, 50);
        assert!(t.take_reply().is_empty(), "reported a size that had not changed");
    }

    #[test]
    fn a_window_255_wide_does_not_end_its_own_subnegotiation() {
        let mut t = client();
        data(&mut t, &[IAC, DO, NAWS]);
        t.take_reply();
        t.resize(255, 24);
        let reply = t.take_reply();
        let mut want = vec![IAC, SB, NAWS];
        want.extend_from_slice(&[0, 255, IAC, 0, 24]);
        want.extend_from_slice(&[IAC, SE]);
        assert_eq!(reply, want);
    }

    #[test]
    fn an_escaped_255_inside_a_subnegotiation_does_not_end_it() {
        // The receive half of the same hazard. If the doubled 255 in a body
        // were read as the start of IAC SE, everything after it would be
        // parsed as data and the screen would fill with rubbish.
        let mut t = client();
        data(&mut t, &[IAC, DO, TERMINAL_TYPE]);
        t.take_reply();
        let out = data(
            &mut t,
            &[IAC, SB, TERMINAL_TYPE, TT_SEND, IAC, IAC, IAC, SE, b'h', b'i'],
        );
        assert_eq!(out, b"hi", "lost its place inside a subnegotiation");
        assert!(!t.take_reply().is_empty(), "did not answer the request");
    }

    #[test]
    fn a_carriage_return_alone_does_not_print_its_padding() {
        // RFC 854 sends a bare CR as CR NUL. CP437 has a character at zero, so
        // passing the NUL on would put one on the screen for every line a
        // board redraws in place.
        let mut t = client();
        assert_eq!(data(&mut t, b"top\r\0left"), b"top\rleft");
    }

    #[test]
    fn a_new_line_keeps_both_of_its_bytes() {
        let mut t = client();
        assert_eq!(data(&mut t, b"one\r\ntwo\r\n"), b"one\r\ntwo\r\n");
    }

    #[test]
    fn a_nul_that_follows_something_else_is_left_alone() {
        let mut t = client();
        assert_eq!(data(&mut t, &[b'a', 0, b'b']), vec![b'a', 0, b'b']);
    }

    #[test]
    fn once_binary_is_agreed_a_nul_is_data() {
        // In binary the pair is not padding, it is two bytes that were sent,
        // and a file transfer would notice one of them going missing.
        let mut t = client();
        data(&mut t, &[IAC, WILL, BINARY]);
        t.take_reply();
        assert_eq!(data(&mut t, &[b'\r', 0, b'x']), vec![b'\r', 0, b'x']);
    }

    #[test]
    fn what_is_typed_has_its_escape_escaped() {
        let t = client();
        let mut out = Vec::new();
        t.encode(&[b'a', 255, b'b'], &mut out);
        assert_eq!(out, vec![b'a', IAC, IAC, b'b']);
    }

    #[test]
    fn a_typed_return_is_padded_until_binary_is_agreed() {
        let mut t = client();
        let mut out = Vec::new();
        t.encode(b"hi\r", &mut out);
        assert_eq!(out, b"hi\r\0");

        data(&mut t, &[IAC, DO, BINARY]);
        t.take_reply();
        let mut out = Vec::new();
        t.encode(b"hi\r", &mut out);
        assert_eq!(out, b"hi\r", "still padding after agreeing to eight bits");
    }

    #[test]
    fn a_command_split_across_reads_still_arrives() {
        // A socket hands over whatever happened to have arrived, and a three
        // byte command straddling two reads is ordinary. The state is in the
        // struct rather than in a buffer for exactly this reason.
        let mut t = client();
        let mut out = data(&mut t, &[b'x', IAC]);
        out.extend(data(&mut t, &[WILL]));
        out.extend(data(&mut t, &[ECHO, b'y']));
        assert_eq!(out, b"xy");
        assert_eq!(t.take_reply(), vec![IAC, DO, ECHO]);
    }

    #[test]
    fn a_go_ahead_is_swallowed_rather_than_drawn() {
        let mut t = client();
        assert_eq!(data(&mut t, &[b'a', IAC, GA, b'b']), b"ab");
    }

    #[test]
    fn a_subnegotiation_that_never_ends_does_not_grow_without_bound() {
        let mut t = client();
        data(&mut t, &[IAC, SB, 99]);
        let flood = vec![b'x'; SB_LIMIT * 4];
        data(&mut t, &flood);
        assert!(t.sub.len() <= SB_LIMIT);
    }
}
