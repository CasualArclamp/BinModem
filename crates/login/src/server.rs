//! A terminal server: what a caller to a dial-up provider met first.
//!
//! A banner, `login:`, `Password:`, and then a prompt with three things worth
//! typing at it -- `ppp`, `help` and `logout`. That was the shape of it, give
//! or take a menu, on everything from a Livingston PortMaster to a Linux box
//! with `mgetty` in front of `pppd`. Anything a caller types is echoed, except
//! the password.
//!
//! And PPP from a standing start. A dialler set up to use PPP straight away
//! never looks at the text at all, and simply starts sending frames the moment
//! it connects; a terminal server that waited for a login would never hear
//! from it. So whatever is being asked for, an LCP frame arriving is taken as
//! the caller choosing PPP -- and a caller who has not logged in is then asked
//! who it is by PPP instead, with PAP or CHAP, which is the link's business
//! rather than this.

use crate::{Account, PppWatch};

/// How long after connecting before the banner.
///
/// Both receivers have only just finished training, and error control may
/// still be settling on top of them. Text sent into that is text half of which
/// is never seen.
pub const SETTLE_MS: u32 = 1_000;

/// How long a caller may sit at `login:` or `Password:` without typing.
pub const LOGIN_IDLE_MS: u32 = 60_000;

/// And at the prompt once logged in.
pub const SHELL_IDLE_MS: u32 = 10 * 60_000;

/// Wrong passwords before the line is put down.
pub const ATTEMPTS: u32 = 3;

/// How long to leave for a goodbye to cross before hanging up on it.
///
/// Hanging the modem up throws away whatever it has not sent yet, and a
/// caller who is hung up on in silence cannot tell a logout from a fault.
pub const GOODBYE_MS: u32 = 2_000;

/// The longest line kept: longer than any name or password, short enough that
/// a caller holding a key down is not filling memory.
const LINE: usize = 64;

/// What the server says and whom it lets in.
#[derive(Debug, Clone)]
pub struct Config {
    /// What this machine calls itself, in the greeting and the prompt.
    pub host: String,
    /// Shown before the first `login:`.
    pub banner: String,
    /// Who may log in. Empty lets nobody in, and says so.
    pub accounts: Vec<Account>,
    /// What `ppp` says before the frames start, which is where a provider
    /// told the caller its address.
    pub ppp_message: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "binmodem".to_owned(),
            banner: "BinModem dial-in".to_owned(),
            accounts: Vec::new(),
            ppp_message: "Entering PPP mode.".to_owned(),
        }
    }
}

/// How a session at the terminal server ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The caller chose PPP. `user` is who logged in first, if anyone did --
    /// None means PPP itself has to ask -- and `early` is whatever of PPP has
    /// already arrived.
    Ppp { user: Option<String>, early: Vec<u8> },
    /// The call should be put down, for this reason.
    HangUp(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    Settling,
    Name,
    Password(String),
    Shell(String),
    /// Saying goodbye, and then hanging up for this reason.
    Leaving(String),
    Done,
}

/// The terminal server for one call.
#[derive(Debug)]
pub struct Server {
    config: Config,
    stage: Stage,
    line: Vec<u8>,
    /// The last octet ended a line, so a line feed straight after it is the
    /// other half of the same end of line rather than an empty one.
    after_return: bool,
    attempts: u32,
    /// Milliseconds since the caller last typed, or since the stage began.
    idle_ms: u32,
    /// Milliseconds left of the current wait.
    wait_ms: u32,
    watch: PppWatch,
    out: Vec<u8>,
    notes: Vec<String>,
    outcome: Option<Outcome>,
}

impl Server {
    pub fn new(config: Config) -> Self {
        let mut notes = Vec::new();
        if config.accounts.is_empty() {
            notes.push("no account is set, so nobody can log in".to_owned());
        }
        Self {
            config,
            stage: Stage::Settling,
            line: Vec::new(),
            after_return: false,
            attempts: 0,
            idle_ms: 0,
            wait_ms: SETTLE_MS,
            watch: PppWatch::default(),
            out: Vec::new(),
            notes,
            outcome: None,
        }
    }

    /// Octets for the caller.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Things worth telling whoever runs the server: who logged in, who got
    /// the password wrong.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// How the session ended, once it has. Given once.
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// Whether anyone is logged in, and who.
    pub fn user(&self) -> Option<&str> {
        match &self.stage {
            Stage::Shell(user) => Some(user),
            _ => None,
        }
    }

    /// Where the session is, in a word.
    pub fn stage(&self) -> &'static str {
        match self.stage {
            Stage::Settling => "answering",
            Stage::Name => "login prompt",
            Stage::Password(_) => "password prompt",
            Stage::Shell(_) => "logged in",
            Stage::Leaving(_) => "saying goodbye",
            Stage::Done => "done",
        }
    }

    /// Time passing.
    pub fn tick(&mut self, ms: u32) {
        self.idle_ms = self.idle_ms.saturating_add(ms);
        match &self.stage {
            Stage::Settling => {
                self.wait_ms = self.wait_ms.saturating_sub(ms);
                if self.wait_ms == 0 {
                    self.greet();
                }
            }
            Stage::Name | Stage::Password(_) if self.idle_ms >= LOGIN_IDLE_MS => {
                self.leave("\r\nTimed out waiting for login.\r\n", "nobody logged in for a minute");
            }
            Stage::Shell(user) if self.idle_ms >= SHELL_IDLE_MS => {
                let why = format!("{user} was idle for ten minutes");
                self.leave("\r\nIdle too long. Goodbye.\r\n", &why);
            }
            Stage::Leaving(why) => {
                self.wait_ms = self.wait_ms.saturating_sub(ms);
                if self.wait_ms == 0 {
                    self.outcome = Some(Outcome::HangUp(why.clone()));
                    self.stage = Stage::Done;
                }
            }
            _ => {}
        }
    }

    /// Octets from the caller.
    pub fn feed(&mut self, bytes: &[u8]) {
        if matches!(self.stage, Stage::Leaving(_) | Stage::Done) {
            return;
        }
        let watched = self.watch.feed(bytes);
        self.type_text(&watched.text);
        if let Some(early) = watched.ppp {
            if matches!(self.stage, Stage::Leaving(_) | Stage::Done) {
                return;
            }
            let user = self.user().map(str::to_owned);
            self.notes.push(match &user {
                Some(user) => format!("{user} started PPP"),
                None => "the caller started PPP without logging in, so PPP asks who it is".to_owned(),
            });
            self.outcome = Some(Outcome::Ppp { user, early });
            self.stage = Stage::Done;
        }
    }

    fn type_text(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.idle_ms = 0;
        for &byte in bytes {
            if self.stage == Stage::Settling {
                // A caller pressing return to wake the far end up: show the
                // prompt now rather than making it wait out the second.
                self.greet();
                self.after_return = byte == b'\r';
                continue;
            }
            if !matches!(self.stage, Stage::Name | Stage::Password(_) | Stage::Shell(_)) {
                return;
            }
            self.key(byte);
        }
    }

    fn echoing(&self) -> bool {
        !matches!(self.stage, Stage::Password(_))
    }

    fn key(&mut self, byte: u8) {
        let after_return = std::mem::replace(&mut self.after_return, false);
        match byte {
            b'\n' if after_return => {}
            // A NUL after a return is how telnet sends a bare one (RFC 854),
            // and a terminal program speaking it at a modem does the same.
            0 if after_return => {}
            b'\r' | b'\n' => {
                self.after_return = true;
                self.out.extend_from_slice(b"\r\n");
                let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
                self.enter(line);
            }
            0x08 | 0x7f => {
                if self.line.pop().is_some() && self.echoing() {
                    self.out.extend_from_slice(b"\x08 \x08");
                }
            }
            // Control-U, the kill character: the whole line goes.
            0x15 => {
                let n = self.line.len();
                self.line.clear();
                if self.echoing() {
                    for _ in 0..n {
                        self.out.extend_from_slice(b"\x08 \x08");
                    }
                }
            }
            0x20..=0x7e if self.line.len() < LINE => {
                self.line.push(byte);
                if self.echoing() {
                    self.out.push(byte);
                }
            }
            _ => {}
        }
    }

    fn greet(&mut self) {
        self.out.extend_from_slice(b"\r\n");
        for line in self.config.banner.lines() {
            self.out.extend_from_slice(line.as_bytes());
            self.out.extend_from_slice(b"\r\n");
        }
        self.out.extend_from_slice(b"\r\nlogin: ");
        self.stage = Stage::Name;
        self.idle_ms = 0;
    }

    fn prompt(&mut self) {
        self.out.extend_from_slice(format!("{}> ", self.config.host).as_bytes());
    }

    fn leave(&mut self, goodbye: &str, why: &str) {
        self.out.extend_from_slice(goodbye.as_bytes());
        self.notes.push(why.to_owned());
        self.stage = Stage::Leaving(why.to_owned());
        self.wait_ms = GOODBYE_MS;
    }

    fn enter(&mut self, line: String) {
        match std::mem::replace(&mut self.stage, Stage::Done) {
            Stage::Name => {
                let name = line.trim().to_owned();
                if name.is_empty() {
                    self.out.extend_from_slice(b"login: ");
                    self.stage = Stage::Name;
                } else {
                    self.out.extend_from_slice(b"Password: ");
                    self.stage = Stage::Password(name);
                }
                self.idle_ms = 0;
            }
            Stage::Password(name) => {
                // Asked for whether or not the name exists, and refused the
                // same way either way: which half was wrong is not the
                // caller's to find out.
                let known = self.config.accounts.iter().any(|a| a.name == name && a.password == line);
                if known {
                    self.notes.push(format!("{name} logged in"));
                    self.out.extend_from_slice(
                        format!(
                            "\r\nWelcome to {host}, {name}.\r\n\r\n\
                             Type ppp to start PPP, help for the commands, or logout to hang up.\r\n\r\n",
                            host = self.config.host
                        )
                        .as_bytes(),
                    );
                    self.stage = Stage::Shell(name);
                    self.prompt();
                    return;
                }
                self.attempts += 1;
                self.notes.push(format!("wrong name or password for {name} ({} of {ATTEMPTS})", self.attempts));
                if self.attempts >= ATTEMPTS {
                    self.leave("Login incorrect\r\nToo many tries. Goodbye.\r\n", "three wrong passwords");
                } else {
                    self.out.extend_from_slice(b"Login incorrect\r\n\r\nlogin: ");
                    self.stage = Stage::Name;
                }
            }
            Stage::Shell(user) => {
                let command = line.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                self.stage = Stage::Shell(user.clone());
                match command.as_str() {
                    "" => self.prompt(),
                    "ppp" => {
                        let message = self.config.ppp_message.clone();
                        self.out.extend_from_slice(message.as_bytes());
                        self.out.extend_from_slice(b"\r\n");
                        self.notes.push(format!("{user} started PPP"));
                        self.outcome = Some(Outcome::Ppp { user: Some(user), early: Vec::new() });
                        self.stage = Stage::Done;
                    }
                    "help" | "?" => {
                        self.out.extend_from_slice(
                            b"  ppp      start PPP: the call carries IP from here on\r\n\
                              \x20 whoami   the name you logged in with\r\n\
                              \x20 logout   hang up (exit, quit and bye do too)\r\n",
                        );
                        self.prompt();
                    }
                    "whoami" => {
                        self.out.extend_from_slice(format!("{user}\r\n").as_bytes());
                        self.prompt();
                    }
                    "logout" | "exit" | "quit" | "bye" => {
                        let why = format!("{user} logged out");
                        self.leave("Goodbye.\r\n", &why);
                    }
                    other => {
                        self.out.extend_from_slice(format!("{other}: command not found\r\n").as_bytes());
                        self.prompt();
                    }
                }
            }
            other => self.stage = other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(Config {
            accounts: vec![Account::new("guest", "letmein")],
            ppp_message: "PPP session from 10.0.0.1 to 10.0.0.2 beginning....".to_owned(),
            ..Config::default()
        })
    }

    /// Run the clock past the settling second, and take what was said.
    fn answered(s: &mut Server) -> String {
        s.tick(SETTLE_MS);
        String::from_utf8(s.take_output()).unwrap()
    }

    fn typed(s: &mut Server, text: &str) -> String {
        s.feed(text.as_bytes());
        String::from_utf8(s.take_output()).unwrap()
    }

    #[test]
    fn a_caller_is_greeted_after_a_second_and_asked_for_a_name() {
        let mut s = server();
        s.tick(SETTLE_MS - 1);
        assert!(s.take_output().is_empty(), "spoke before the line had settled");
        s.tick(1);
        let said = String::from_utf8(s.take_output()).unwrap();
        assert!(said.contains("BinModem dial-in"), "{said:?}");
        assert!(said.ends_with("login: "), "{said:?}");
    }

    #[test]
    fn the_right_password_logs_in_and_ppp_starts_it() {
        let mut s = server();
        answered(&mut s);
        assert_eq!(typed(&mut s, "guest\r"), "guest\r\nPassword: ");
        let welcome = typed(&mut s, "letmein\r");
        assert!(!welcome.contains("letmein"), "the password was echoed");
        assert!(welcome.contains("Welcome to binmodem, guest."), "{welcome:?}");
        assert!(welcome.ends_with("binmodem> "), "{welcome:?}");
        assert_eq!(s.user(), Some("guest"));

        let said = typed(&mut s, "ppp\r\n");
        assert!(said.contains("PPP session from 10.0.0.1 to 10.0.0.2 beginning...."), "{said:?}");
        assert_eq!(s.take_outcome(), Some(Outcome::Ppp { user: Some("guest".into()), early: vec![] }));
        assert!(s.take_notes().iter().any(|n| n == "guest logged in"));
    }

    #[test]
    fn a_wrong_password_is_refused_three_times_and_then_hung_up_on() {
        let mut s = server();
        answered(&mut s);
        for attempt in 1..=ATTEMPTS {
            typed(&mut s, "guest\r");
            let said = typed(&mut s, "hunter2\r");
            assert!(said.contains("Login incorrect"), "{said:?}");
            if attempt < ATTEMPTS {
                assert!(said.ends_with("login: "), "{said:?}");
            }
        }
        assert_eq!(s.take_outcome(), None, "hung up before the goodbye could cross");
        s.tick(GOODBYE_MS);
        assert!(matches!(s.take_outcome(), Some(Outcome::HangUp(_))));
    }

    /// Nothing about a name that does not exist is different from a name with
    /// the wrong password.
    #[test]
    fn an_unknown_name_is_refused_exactly_like_a_wrong_password() {
        let mut a = server();
        let mut b = server();
        answered(&mut a);
        answered(&mut b);
        assert_eq!(typed(&mut a, "nobody\r").replace("nobody", "guest"), typed(&mut b, "guest\r"));
        assert_eq!(typed(&mut a, "x\r"), typed(&mut b, "x\r"));
    }

    #[test]
    fn backspace_takes_back_what_was_typed() {
        let mut s = server();
        answered(&mut s);
        assert_eq!(typed(&mut s, "gux\x08"), "gux\x08 \x08");
        typed(&mut s, "est\r");
        typed(&mut s, "letmeix\x7fn\r");
        assert_eq!(s.user(), Some("guest"), "the corrected password was not the one checked");
    }

    #[test]
    fn the_commands_at_the_prompt() {
        let mut s = server();
        answered(&mut s);
        typed(&mut s, "guest\r");
        typed(&mut s, "letmein\r");
        assert!(typed(&mut s, "help\r").contains("ppp"));
        assert_eq!(typed(&mut s, "whoami\r"), "whoami\r\nguest\r\nbinmodem> ");
        assert_eq!(typed(&mut s, "rm -rf /\r"), "rm -rf /\r\nrm: command not found\r\nbinmodem> ");
        assert!(typed(&mut s, "logout\r").contains("Goodbye."));
        s.tick(GOODBYE_MS);
        assert_eq!(s.take_outcome(), Some(Outcome::HangUp("guest logged out".into())));
    }

    /// A dialler that goes straight to PPP is answered as PPP, with the frame
    /// it opened with handed on whole.
    #[test]
    fn a_caller_that_sends_ppp_at_the_login_prompt_gets_ppp() {
        let mut s = server();
        answered(&mut s);
        let frame = [0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7d, 0x21, 0x7d, 0x21];
        s.feed(&frame[..4]);
        s.feed(&frame[4..]);
        assert_eq!(s.take_outcome(), Some(Outcome::Ppp { user: None, early: frame.to_vec() }));
        // And nothing was echoed back into the frame's way.
        assert!(!s.take_output().contains(&0x7e));
    }

    #[test]
    fn a_caller_who_never_types_is_hung_up_on() {
        let mut s = server();
        answered(&mut s);
        s.tick(LOGIN_IDLE_MS);
        s.tick(GOODBYE_MS);
        assert!(matches!(s.take_outcome(), Some(Outcome::HangUp(_))));
    }

    #[test]
    fn with_no_accounts_nobody_gets_in_and_whoever_runs_it_is_told() {
        let mut s = Server::new(Config::default());
        assert!(s.take_notes().iter().any(|n| n.contains("nobody can log in")));
        answered(&mut s);
        typed(&mut s, "\r");
        typed(&mut s, "guest\r");
        assert!(typed(&mut s, "\r").contains("Login incorrect"));
    }
}
