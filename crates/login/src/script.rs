//! The dialler's side of a text login: watch for the prompts, answer them,
//! and start PPP when the far end does.
//!
//! What Windows' Dial-Up Networking scripts and `chat` did, without having to
//! be written for each provider. The prompts it knows are the ones nearly
//! everything used -- a line ending `login:` or `username:`, then one ending
//! `password:` -- and a prompt after that is answered with the command that
//! starts PPP, `ppp` unless told otherwise. A prompt is only answered once
//! the far end has stopped talking for a moment, which is what tells a prompt
//! from a line that merely has the word in it: "Last login: Tue Sep 15" does
//! not stop at the colon.
//!
//! PPP starts the moment the far end's first LCP frame arrives, whatever the
//! script was waiting for. And a far end that goes quiet once the password is
//! in, or says nothing at all from the start, is taken to be waiting for this
//! end to start PPP itself.

use crate::{Account, PppWatch};

/// How long the far end has to be quiet before what it said last is taken as
/// a prompt.
pub const QUIET_MS: u32 = 400;

/// A far end that has said nothing at all this long after connecting is not
/// going to ask for a login; it is waiting for PPP.
pub const SILENT_MS: u32 = 8_000;

/// How long to wait for a prompt before giving up on the script.
pub const PROMPT_MS: u32 = 45_000;

/// How long a far end may stay quiet after the password, or after the
/// command, before this end starts PPP itself.
pub const AFTER_LOGIN_MS: u32 = 4_000;

/// How much of what the far end said is kept to look for prompts in.
const KEPT: usize = 512;

/// How the script ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// PPP is starting. `early` is whatever of the far end's first frame has
    /// already arrived, and is empty when this end is the one starting it.
    Ppp { early: Vec<u8> },
    /// It could not get there, and the terminal should be given back.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waiting to be asked for a name.
    Name,
    /// Sent the name; waiting to be asked for the password.
    Password,
    /// Sent the password; waiting for a prompt, or for PPP.
    Command,
    /// Sent the command; waiting for PPP.
    Starting,
    Done,
}

/// One login, in progress.
#[derive(Debug)]
pub struct Script {
    account: Account,
    command: String,
    stage: Stage,
    /// What the far end has said since this end last said anything, lower
    /// case, with ends of line as `\n`.
    heard: String,
    heard_anything: bool,
    quiet_ms: u32,
    stage_ms: u32,
    watch: PppWatch,
    out: Vec<u8>,
    notes: Vec<String>,
    outcome: Option<Outcome>,
}

const NAME_PROMPTS: [&str; 4] = ["login:", "username:", "user name:", "user:"];
const PASSWORD_PROMPTS: [&str; 2] = ["password:", "passcode:"];
/// What a far end that has let the caller in says when it will not.
const REFUSALS: [&str; 5] = ["incorrect", "denied", "invalid", "failed", "not found"];

impl Script {
    /// Log in as `account`, and at the prompt after it type `command` -- or
    /// nothing, if it is empty, and start PPP at the prompt instead.
    pub fn new(account: Account, command: &str) -> Self {
        Self {
            account,
            command: command.trim().to_owned(),
            stage: Stage::Name,
            heard: String::new(),
            heard_anything: false,
            quiet_ms: 0,
            stage_ms: 0,
            watch: PppWatch::default(),
            out: Vec::new(),
            notes: Vec::new(),
            outcome: None,
        }
    }

    /// Octets for the far end.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// What it did, for the transcript.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// How it ended, once it has. Given once.
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// Where it has got to, in a few words.
    pub fn stage(&self) -> &'static str {
        match self.stage {
            Stage::Name => "waiting for login:",
            Stage::Password => "waiting for Password:",
            Stage::Command => "logged in, waiting for a prompt",
            Stage::Starting => "waiting for PPP",
            Stage::Done => "done",
        }
    }

    /// Octets from the far end.
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.stage == Stage::Done || bytes.is_empty() {
            return;
        }
        let watched = self.watch.feed(bytes);
        if let Some(early) = watched.ppp {
            self.notes.push("the far end started PPP".to_owned());
            self.finish(Outcome::Ppp { early });
            return;
        }
        self.heard_anything = true;
        self.quiet_ms = 0;
        for &byte in &watched.text {
            match byte {
                b'\r' | b'\n' => {
                    if !self.heard.ends_with('\n') {
                        self.heard.push('\n');
                    }
                }
                0x20..=0x7e => self.heard.push(char::from(byte.to_ascii_lowercase())),
                _ => {}
            }
        }
        if self.heard.len() > KEPT {
            let cut = self.heard.len() - KEPT;
            self.heard.drain(..cut);
        }
        // Once the password has gone, a refusal is worth stopping for at
        // once rather than after the far end has gone quiet.
        if matches!(self.stage, Stage::Command | Stage::Starting)
            && let Some(line) = self.heard.lines().find(|l| REFUSALS.iter().any(|r| l.contains(r)))
        {
            let line = line.trim().to_owned();
            self.finish(Outcome::Failed(format!("the far end said \"{line}\"")));
        }
    }

    /// Time passing.
    pub fn tick(&mut self, ms: u32) {
        if self.stage == Stage::Done {
            return;
        }
        self.quiet_ms = self.quiet_ms.saturating_add(ms);
        self.stage_ms = self.stage_ms.saturating_add(ms);

        if self.quiet_ms >= QUIET_MS && !self.heard.trim().is_empty() && self.answer() {
            return;
        }

        match self.stage {
            Stage::Name if !self.heard_anything && self.stage_ms >= SILENT_MS => {
                self.notes.push("the far end said nothing, so this end starts PPP".to_owned());
                self.finish(Outcome::Ppp { early: Vec::new() });
            }
            Stage::Name | Stage::Password if self.stage_ms >= PROMPT_MS => {
                let what = if self.stage == Stage::Name { "login" } else { "password" };
                self.finish(Outcome::Failed(format!("no {what} prompt came")));
            }
            Stage::Command | Stage::Starting if self.quiet_ms >= AFTER_LOGIN_MS => {
                self.notes.push("the far end went quiet, so this end starts PPP".to_owned());
                self.finish(Outcome::Ppp { early: Vec::new() });
            }
            _ => {}
        }
    }

    /// The far end has gone quiet after saying something: answer it if it was
    /// a question. Says whether it was.
    fn answer(&mut self) -> bool {
        let last = self.heard.trim_end().to_owned();
        let asks = |prompts: &[&str]| prompts.iter().any(|p| last.ends_with(p));
        match self.stage {
            Stage::Name if asks(&NAME_PROMPTS) => {
                let name = self.account.name.clone();
                self.say(&name, Stage::Password);
                self.notes.push(format!("answered the login prompt as {name}"));
            }
            Stage::Password if asks(&PASSWORD_PROMPTS) => {
                let password = self.account.password.clone();
                self.say(&password, Stage::Command);
                self.notes.push("answered the password prompt".to_owned());
            }
            Stage::Password | Stage::Command | Stage::Starting if asks(&NAME_PROMPTS) || asks(&PASSWORD_PROMPTS) => {
                self.finish(Outcome::Failed(
                    "the far end asked for the login again, so the name or password was not accepted".to_owned(),
                ));
            }
            // No password asked for, and a prompt: some accounts have none.
            // Treated as logged in.
            Stage::Password | Stage::Command if ends_like_a_prompt(&last) => self.at_prompt(),
            _ => return false,
        }
        true
    }

    fn at_prompt(&mut self) {
        if self.command.is_empty() || self.stage == Stage::Starting {
            self.notes.push("at the prompt, starting PPP".to_owned());
            self.finish(Outcome::Ppp { early: Vec::new() });
        } else {
            let command = self.command.clone();
            self.notes.push(format!("typed {command} at the prompt"));
            self.say(&command, Stage::Starting);
        }
    }

    fn say(&mut self, text: &str, next: Stage) {
        self.out.extend_from_slice(text.as_bytes());
        self.out.push(b'\r');
        self.heard.clear();
        self.stage = next;
        self.stage_ms = 0;
        self.quiet_ms = 0;
    }

    fn finish(&mut self, outcome: Outcome) {
        self.outcome = Some(outcome);
        self.stage = Stage::Done;
    }
}

/// Whether the last line looks like something waiting for a command: a
/// shell's `$`, `#`, `%` or `>`, or a menu's colon.
fn ends_like_a_prompt(text: &str) -> bool {
    text.lines().last().is_some_and(|line| {
        let line = line.trim_end();
        !line.is_empty() && line.ends_with(['>', '$', '#', '%', ':'])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{self, Server};

    fn script() -> Script {
        Script::new(Account::new("guest", "letmein"), "ppp")
    }

    /// Hand text over and let the far end go quiet.
    fn hear(s: &mut Script, text: &str) -> String {
        s.feed(text.as_bytes());
        s.tick(QUIET_MS);
        String::from_utf8(s.take_output()).unwrap()
    }

    #[test]
    fn it_answers_a_login_and_a_password_and_types_ppp() {
        let mut s = script();
        assert_eq!(hear(&mut s, "\r\nWelcome to the Internet\r\n\r\nlogin: "), "guest\r");
        assert_eq!(hear(&mut s, "guest\r\nPassword: "), "letmein\r");
        assert_eq!(hear(&mut s, "\r\nLast login: Tue Sep 15 on ttyS0\r\n$ "), "ppp\r");
        s.feed(&[0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7d, 0x21]);
        assert_eq!(
            s.take_outcome(),
            Some(Outcome::Ppp { early: vec![0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7d, 0x21] })
        );
    }

    /// A prompt is only a prompt once the far end stops: the colon in "Last
    /// login:" arriving at the end of a buffer is not a question.
    #[test]
    fn a_word_at_the_end_of_a_buffer_is_not_a_prompt_until_it_stops() {
        let mut s = script();
        s.feed(b"Last login:");
        s.tick(QUIET_MS / 2);
        s.feed(b" Tue Sep 15 on ttyS0\r\n");
        s.tick(QUIET_MS);
        assert!(s.take_output().is_empty(), "answered a line that was not a prompt");
    }

    #[test]
    fn a_refusal_ends_it_with_what_the_far_end_said() {
        let mut s = script();
        hear(&mut s, "login: ");
        hear(&mut s, "Password: ");
        s.feed(b"\r\nLogin incorrect\r\n");
        match s.take_outcome() {
            Some(Outcome::Failed(why)) => assert!(why.contains("login incorrect"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_far_end_that_says_nothing_is_waiting_for_ppp() {
        let mut s = script();
        s.tick(SILENT_MS);
        assert_eq!(s.take_outcome(), Some(Outcome::Ppp { early: vec![] }));
    }

    #[test]
    fn a_far_end_that_goes_quiet_after_the_password_is_waiting_for_ppp() {
        let mut s = script();
        hear(&mut s, "login: ");
        hear(&mut s, "Password: ");
        s.feed(b"\r\nStarting PPP...\r\n");
        s.tick(AFTER_LOGIN_MS);
        assert_eq!(s.take_outcome(), Some(Outcome::Ppp { early: vec![] }));
    }

    /// And against this crate's own terminal server, octet for octet, with
    /// the server's echo coming back the way a far end's would.
    #[test]
    fn it_logs_in_to_the_server_in_this_crate() {
        let mut server = Server::new(server::Config {
            accounts: vec![Account::new("guest", "letmein")],
            ..server::Config::default()
        });
        let mut s = script();
        let mut ppp = None;
        for _ in 0..20_000 {
            let to_caller = server.take_output();
            s.feed(&to_caller);
            let to_server = s.take_output();
            server.feed(&to_server);
            server.tick(1);
            s.tick(1);
            if let Some(server::Outcome::Ppp { user, .. }) = server.take_outcome() {
                assert_eq!(user.as_deref(), Some("guest"));
                // The server's link would start now; its first frame is what
                // the script is waiting for.
                s.feed(&[0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21]);
                ppp = s.take_outcome();
                break;
            }
            assert_eq!(s.take_outcome(), None, "the script gave up");
        }
        assert!(matches!(ppp, Some(Outcome::Ppp { .. })), "never got to PPP");
    }
}
