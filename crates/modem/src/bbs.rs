//! A small bulletin board, for the far end of a call.
//!
//! What a caller in 1994 found when the modems stopped screeching: a screen
//! of ANSI art, a login, a menu, bulletins, and a chat channel with people in
//! it. The screens are files in a folder -- `welcome.ans`, `menu.ans`,
//! `bulletins.ans`, `logoff.ans`, in CP437 with ANSI escapes as a board's
//! were -- and the channel's regulars say what `chat.txt` gives them to say,
//! when it says to.
//!
//! Nothing here touches a line. Bytes the caller typed go in with
//! [`Board::hear`], time goes in with [`Board::tick`], and what the board
//! says comes out of [`Board::take_output`] for the modem to send.
//!
//! `chat.txt` is sections of lines. A section starts `== join` (said when the
//! caller comes into the channel) or `== after N` (said once the caller has
//! said N things). A line is `<seconds> <nick> <text>`: the pause before it,
//! who says it, and what -- `*` as the nick for the channel's own notices
//! (joins, parts, topics). `{you}` is the caller's name. `#` starts a
//! comment.
//!
//! Files in a `files` folder beside them are offered by `[D]ownload`: the
//! board says so and hands the file over with [`Board::take_download`], for
//! whatever is driving the line to send by ZMODEM, and is told how it went
//! with [`Board::download_finished`].

use std::collections::VecDeque;
use std::path::Path;

/// Where the caller is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Not yet connected.
    Waiting,
    Login,
    Password,
    Menu,
    /// Reading something, until a key is pressed.
    Paused,
    /// A file going out; the line is the transfer's until it is over.
    Download,
    Chat,
    /// Said goodbye; the line is about to go down.
    Gone,
}

/// One thing said in the channel.
#[derive(Debug, Clone, PartialEq)]
struct Said {
    after: f64,
    nick: String,
    text: String,
}

/// The board.
#[derive(Debug)]
pub struct Board {
    name: String,
    welcome: Vec<u8>,
    menu: Vec<u8>,
    bulletins: Vec<u8>,
    logoff: Vec<u8>,
    /// The channel's sections: on joining, and after each thing the caller says.
    join: Vec<Said>,
    after: Vec<(usize, Vec<Said>)>,
    place: Place,
    out: Vec<u8>,
    /// What the caller has typed of the line in progress.
    typing: String,
    caller: String,
    /// How the call came up, for "who's online".
    carrier: String,
    /// Lines waiting to be said, and when the next is due.
    queue: VecDeque<Said>,
    next_at: f64,
    /// Things the caller has said in the channel.
    spoken: usize,
    /// When goodbye was said.
    gone_at: Option<f64>,
    /// The files to offer, and the one asked for and not yet taken.
    files: Vec<(String, Vec<u8>)>,
    offered: Option<(String, Vec<u8>)>,
}

/// The channel the regulars are in.
const CHANNEL: &str = "#arclamp-dx";

/// Colours for nicks, by a hash of the nick, so each keeps its own.
const NICK_COLOURS: [u8; 6] = [31, 32, 33, 35, 36, 34];

impl Board {
    /// A board called `name`, its screens and its channel read from `dir`.
    /// A screen whose file is missing is a plain one written here.
    pub fn open(name: &str, dir: &Path) -> std::io::Result<Self> {
        let read = |file: &str| std::fs::read(dir.join(file)).ok();
        let chat = std::fs::read_to_string(dir.join("chat.txt")).unwrap_or_default();
        let mut board = Self::new(name, &chat);
        board.welcome = read("welcome.ans").unwrap_or(board.welcome);
        board.menu = read("menu.ans").unwrap_or(board.menu);
        board.bulletins = read("bulletins.ans").unwrap_or(board.bulletins);
        board.logoff = read("logoff.ans").unwrap_or(board.logoff);
        if let Ok(entries) = std::fs::read_dir(dir.join("files")) {
            let mut files: Vec<(String, Vec<u8>)> = entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .filter_map(|e| {
                    Some((
                        e.file_name().to_string_lossy().into_owned(),
                        std::fs::read(e.path()).ok()?,
                    ))
                })
                .collect();
            files.sort();
            board.files = files;
        }
        if !dir.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not a folder", dir.display()),
            ));
        }
        Ok(board)
    }

    /// A board with plain screens and the channel `chat` describes.
    pub fn new(name: &str, chat: &str) -> Self {
        let (join, after) = parse_chat(chat);
        Self {
            name: name.to_owned(),
            welcome: format!("\x1b[2J\x1b[H\x1b[1;36m{name}\x1b[0m\r\n\r\n").into_bytes(),
            menu: Vec::new(),
            bulletins: b"No bulletins today.\r\n".to_vec(),
            logoff: b"\r\n73, and thanks for calling.\r\n".to_vec(),
            join,
            after,
            place: Place::Waiting,
            out: Vec::new(),
            typing: String::new(),
            caller: String::new(),
            carrier: String::new(),
            queue: VecDeque::new(),
            next_at: 0.0,
            spoken: 0,
            gone_at: None,
            files: Vec::new(),
            offered: None,
        }
    }

    /// The call is up, at `carrier` ("V.34 33600", say): the welcome screen
    /// and the login prompt.
    pub fn connected(&mut self, carrier: &str) {
        self.carrier = carrier.to_owned();
        self.out.extend_from_slice(&self.welcome.clone());
        self.say("\r\n\x1b[0;37mlogin: \x1b[1;37m");
        self.place = Place::Login;
    }

    /// What the caller typed.
    pub fn hear(&mut self, bytes: &[u8], now: f64) {
        for &b in bytes {
            match self.place {
                Place::Login | Place::Password | Place::Chat => self.line_byte(b, now),
                Place::Menu => self.command(b, now),
                Place::Paused => self.show_menu(),
                Place::Waiting | Place::Gone | Place::Download => {}
            }
        }
    }

    /// Time passing: the channel's regulars say what is due.
    pub fn tick(&mut self, now: f64) {
        while self.place == Place::Chat
            && let Some(next) = self.queue.front()
            && now >= self.next_at
        {
            let said = next.clone();
            self.queue.pop_front();
            self.chat_line(&said.nick, &said.text);
            self.next_at = now + self.queue.front().map_or(0.0, |s| s.after);
        }
    }

    /// What the board has said since this was last asked.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// The file the caller asked for, once, for the line to send by ZMODEM.
    pub fn take_download(&mut self) -> Option<(String, Vec<u8>)> {
        self.offered.take()
    }

    /// How the download went: back to the menu with a word about it.
    pub fn download_finished(&mut self, bytes: usize, seconds: f64, ok: bool) {
        if ok {
            let seconds = seconds.max(0.001);
            self.say(&format!(
                "\r\n\x1b[1;32mTransfer complete:\x1b[0m {bytes} bytes in {seconds:.1} s, \x1b[1;33m{:.0} cps\x1b[0m\r\n",
                bytes as f64 / seconds
            ));
        } else {
            self.say("\r\n\x1b[1;31mTransfer aborted.\x1b[0m\r\n");
        }
        self.show_menu();
    }

    /// When goodbye was said, so the caller can put the line down once it
    /// has gone out.
    pub fn gone_at(&self) -> Option<f64> {
        self.gone_at
    }

    fn say(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    /// A byte of a typed line: echoed, a password as stars, and acted on at
    /// the end of the line.
    fn line_byte(&mut self, b: u8, now: f64) {
        match b {
            b'\r' | b'\n' => {
                if b == b'\n' && self.typing.is_empty() {
                    return;
                }
                let line = std::mem::take(&mut self.typing);
                self.line(line.trim(), now);
            }
            8 | 127 => {
                if self.typing.pop().is_some() {
                    self.say("\x08 \x08");
                }
            }
            32..=126 => {
                self.typing.push(b as char);
                let echo = if self.place == Place::Password {
                    '*'
                } else {
                    b as char
                };
                self.say(&echo.to_string());
            }
            _ => {}
        }
    }

    fn line(&mut self, line: &str, now: f64) {
        match self.place {
            Place::Login => {
                self.caller = if line.is_empty() {
                    "guest".to_owned()
                } else {
                    line.to_owned()
                };
                self.say("\x1b[0m\r\n\x1b[0;37mpassword: \x1b[1;37m");
                self.place = Place::Password;
            }
            Place::Password => {
                let caller = self.caller.clone();
                self.say(&format!(
                    "\x1b[0m\r\n\r\n\x1b[1;32mWelcome back, {caller}.\x1b[0m You are caller \x1b[1;33m#1,994\x1b[0m. \
                     Connected at \x1b[1;36m{}\x1b[0m.\r\n",
                    self.carrier
                ));
                self.show_menu();
            }
            Place::Chat => {
                if line.eq_ignore_ascii_case("/quit") || line.eq_ignore_ascii_case("/part") {
                    let caller = self.caller.clone();
                    self.say("\r\x1b[K");
                    self.notice(&format!("{caller} has left {CHANNEL}"));
                    self.queue.clear();
                    self.show_menu();
                    return;
                }
                let caller = self.caller.clone();
                // Over the echo of what was typed, the way a client draws it.
                self.say("\r\x1b[K");
                self.chat_line(&caller, line);
                self.spoken += 1;
                if let Some((_, lines)) = self.after.iter().find(|(n, _)| *n == self.spoken) {
                    self.queue.extend(lines.iter().cloned());
                    self.next_at = now + self.queue.front().map_or(0.0, |s| s.after);
                }
            }
            _ => {}
        }
    }

    fn show_menu(&mut self) {
        if self.menu.is_empty() {
            let name = self.name.clone();
            self.say(&format!(
                "\r\n\x1b[1;36m{name}\x1b[0m\r\n\
                 \x1b[1;33m[C]\x1b[0m Chat {CHANNEL}   \x1b[1;33m[B]\x1b[0m Bulletins   \
                 \x1b[1;33m[D]\x1b[0m Download   \x1b[1;33m[W]\x1b[0m Who's online   \x1b[1;33m[G]\x1b[0m Goodbye\r\n"
            ));
        } else {
            self.out.extend_from_slice(&self.menu.clone());
        }
        self.say("\r\n\x1b[1;37mCommand: \x1b[0m");
        self.place = Place::Menu;
    }

    /// A key at the menu.
    fn command(&mut self, b: u8, now: f64) {
        match b.to_ascii_uppercase() {
            b'C' => {
                self.say("C\r\n\x1b[2J\x1b[H");
                self.say(&format!(
                    "\x1b[1;37;44m {CHANNEL} \x1b[0;37;44m  DX, digital modes, and modems that should not work \
                     but do   /quit to leave \x1b[0m\r\n\r\n"
                ));
                let caller = self.caller.clone();
                self.notice(&format!("{caller} has joined {CHANNEL}"));
                self.place = Place::Chat;
                self.spoken = 0;
                self.queue = self.join.iter().cloned().collect();
                self.next_at = now + self.queue.front().map_or(0.0, |s| s.after);
            }
            b'B' => {
                self.say("B\r\n");
                self.out.extend_from_slice(&self.bulletins.clone());
                self.say("\r\n\x1b[0;37m[ press any key ]\x1b[0m");
                self.place = Place::Paused;
            }
            b'D' => {
                self.say("D\r\n");
                match self.files.first().cloned() {
                    Some((name, data)) => {
                        self.say(&format!(
                            "\r\n\x1b[1;37m{name}\x1b[0m, {} bytes. Sending by \x1b[1;36mZMODEM\x1b[0m: start your download now.\r\n",
                            data.len()
                        ));
                        self.offered = Some((name, data));
                        self.place = Place::Download;
                    }
                    None => {
                        self.say("Nothing in the file area today.\r\n");
                        self.say("\r\n\x1b[1;37mCommand: \x1b[0m");
                    }
                }
            }
            b'W' => {
                let caller = self.caller.clone();
                self.say(&format!(
                    "W\r\n\r\n\x1b[1;37mNode  Who            Where              How\x1b[0m\r\n\
                     \x1b[0;36m  1   \x1b[1;37m{caller:<14} \x1b[0;37mmain menu          \x1b[1;36m{}\x1b[0m\r\n\
                     \x1b[0;36m  2   \x1b[1;37mSysop          \x1b[0;37mtuning a dish      \x1b[0;36mlocal\x1b[0m\r\n",
                    self.carrier
                ));
                self.say("\r\n\x1b[1;37mCommand: \x1b[0m");
            }
            b'G' => {
                self.say("G\r\n");
                self.out.extend_from_slice(&self.logoff.clone());
                self.place = Place::Gone;
                self.gone_at = Some(now);
            }
            _ => {}
        }
    }

    fn notice(&mut self, text: &str) {
        self.say(&format!("\x1b[0;90m-!- \x1b[0;37m{text}\x1b[0m\r\n"));
    }

    fn chat_line(&mut self, nick: &str, text: &str) {
        let text = text.replace("{you}", &self.caller.clone());
        if nick == "*" {
            self.notice(&text);
            return;
        }
        let colour =
            NICK_COLOURS[nick.bytes().map(usize::from).sum::<usize>() % NICK_COLOURS.len()];
        let me = nick == self.caller;
        self.say(&format!(
            "\x1b[0;90m<\x1b[{}{colour}m{nick}\x1b[0;90m>\x1b[0m {}{text}\x1b[0m\r\n",
            if me { "1;" } else { "0;" },
            if me { "\x1b[1;37m" } else { "" },
        ));
    }
}

/// The channel's sections out of `chat.txt`.
fn parse_chat(text: &str) -> (Vec<Said>, Vec<(usize, Vec<Said>)>) {
    let mut join = Vec::new();
    let mut after: Vec<(usize, Vec<Said>)> = Vec::new();
    // Which section lines go into: None for `join`, Some(n) for `after n`.
    let mut into: Option<usize> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(head) = line.strip_prefix("==") {
            let head = head.trim();
            into = head
                .strip_prefix("after")
                .and_then(|n| n.trim().parse().ok());
            if let Some(n) = into
                && !after.iter().any(|(k, _)| *k == n)
            {
                after.push((n, Vec::new()));
            }
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let (Some(delay), Some(nick)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(after_s) = delay.parse::<f64>() else {
            continue;
        };
        let said = Said {
            after: after_s,
            nick: nick.to_owned(),
            text: parts.next().unwrap_or("").to_owned(),
        };
        match into {
            None => join.push(said),
            Some(n) => {
                if let Some((_, lines)) = after.iter_mut().find(|(k, _)| *k == n) {
                    lines.push(said);
                }
            }
        }
    }
    (join, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: &str = "\
# the regulars
== join
1.0 * Topic: 20m is open to Europe
2.0 vk4bob morning {you}
== after 1
1.5 dxnut ha, welcome back
";

    fn text(board: &mut Board) -> String {
        String::from_utf8_lossy(&board.take_output()).into_owned()
    }

    fn typed(board: &mut Board, line: &str, now: f64) {
        board.hear(line.as_bytes(), now);
        board.hear(b"\r", now);
    }

    #[test]
    fn a_caller_logs_in_and_reaches_the_menu() {
        let mut b = Board::new("Arclamp DX BBS", CHAT);
        b.connected("V.34 33600");
        assert!(text(&mut b).contains("login: "));
        typed(&mut b, "rory", 0.0);
        let shown = text(&mut b);
        assert!(
            shown.contains("rory") && shown.contains("password: "),
            "{shown:?}"
        );
        typed(&mut b, "hunter2", 0.0);
        let shown = text(&mut b);
        assert!(
            shown.contains("*******"),
            "the password was echoed: {shown:?}"
        );
        assert!(!shown.contains("hunter2"));
        assert!(shown.contains("Welcome back, rory") && shown.contains("V.34 33600"));
        assert!(shown.contains("Command: "));
    }

    #[test]
    fn the_channel_talks_on_time_and_answers_the_caller() {
        let mut b = Board::new("Arclamp DX BBS", CHAT);
        b.connected("V.ARC");
        typed(&mut b, "rory", 0.0);
        typed(&mut b, "x", 0.0);
        text(&mut b);
        b.hear(b"c", 10.0);
        assert!(text(&mut b).contains("rory has joined #arclamp-dx"));
        b.tick(10.5);
        assert!(!text(&mut b).contains("Topic"), "said before its time");
        b.tick(11.0);
        assert!(text(&mut b).contains("Topic: 20m is open to Europe"));
        b.tick(12.5);
        assert!(text(&mut b).is_empty(), "the next line came early");
        b.tick(13.0);
        assert!(
            text(&mut b).contains("morning rory"),
            "{{you}} not filled in"
        );
        typed(&mut b, "hello all", 20.0);
        let shown = text(&mut b);
        assert!(shown.contains("rory") && shown.contains("hello all"));
        b.tick(21.5);
        assert!(text(&mut b).contains("welcome back"));
    }

    #[test]
    fn bulletins_wait_for_a_key_and_goodbye_ends_the_call() {
        let mut b = Board::new("Arclamp DX BBS", CHAT);
        b.connected("V.34 26400");
        typed(&mut b, "rory", 0.0);
        typed(&mut b, "x", 0.0);
        text(&mut b);
        b.hear(b"b", 1.0);
        assert!(text(&mut b).contains("press any key"));
        b.hear(b" ", 2.0);
        assert!(text(&mut b).contains("Command: "));
        b.hear(b"w", 3.0);
        assert!(text(&mut b).contains("V.34 26400"));
        assert_eq!(b.gone_at(), None);
        b.hear(b"g", 4.0);
        assert!(text(&mut b).contains("73"));
        assert_eq!(b.gone_at(), Some(4.0));
    }

    #[test]
    fn a_download_is_handed_over_once_and_the_board_comes_back_after_it() {
        let mut b = Board::new("Arclamp DX BBS", CHAT);
        b.files = vec![("bandplan.txt".to_owned(), vec![b'x'; 1000])];
        b.connected("V.ARC/L16 601302");
        typed(&mut b, "rory", 0.0);
        typed(&mut b, "x", 0.0);
        text(&mut b);
        b.hear(b"d", 1.0);
        let shown = text(&mut b);
        assert!(
            shown.contains("bandplan.txt") && shown.contains("ZMODEM"),
            "{shown:?}"
        );
        assert_eq!(
            b.take_download().map(|(n, d)| (n, d.len())),
            Some(("bandplan.txt".to_owned(), 1000))
        );
        assert_eq!(b.take_download(), None, "handed over twice");
        b.hear(b"g", 2.0);
        assert_eq!(
            b.gone_at(),
            None,
            "a key in the middle of the transfer was taken as a command"
        );
        b.download_finished(1000, 0.5, true);
        let shown = text(&mut b);
        assert!(
            shown.contains("2000 cps") && shown.contains("Command: "),
            "{shown:?}"
        );
    }

    #[test]
    fn a_chat_file_reads_into_its_sections() {
        let (join, after) = parse_chat(CHAT);
        assert_eq!(join.len(), 2);
        assert_eq!(
            join[1],
            Said {
                after: 2.0,
                nick: "vk4bob".into(),
                text: "morning {you}".into()
            }
        );
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].0, 1);
        assert_eq!(after[0].1[0].nick, "dxnut");
    }
}
