//! A login in front of PPP, on a live call.
//!
//! [`login`] has the two ends of it and knows nothing about modems or windows;
//! this is where they meet the call. A call this end answered gets the
//! terminal server, if the window says to answer that way, and a call it
//! placed can run the script that logs in to a far end and starts PPP.
//!
//! While either runs it owns the byte stream, the way PPP and a file transfer
//! do, and the screen shows the conversation: on the answering end what the
//! caller sees, and on the calling end what the far end said. The password
//! never appears in either, because the server does not echo it and nothing
//! this end types is drawn.

use login::Account;
use telemetry::{Direction, Publisher};

use crate::network::{CLIENT_ADDRESS, SERVER_ADDRESS};

/// What the window has set for logging in, both ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Whether a call this end answers gets a login prompt.
    pub serve: bool,
    /// The one account: what this end logs in with when it calls, and what a
    /// caller has to give when it answers.
    pub account: Account,
    /// What to type at the far end's prompt once logged in.
    pub command: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { serve: false, account: Account::default(), command: "ppp".to_owned() }
    }
}

impl Settings {
    /// The accounts a caller may use. None set is nobody at all.
    pub fn callers(&self) -> Vec<Account> {
        if self.account.name.trim().is_empty() {
            Vec::new()
        } else {
            vec![self.account.clone()]
        }
    }

    /// Kept between runs, in the same file as everything else. The password
    /// too, in plain text, which is what the window says beside the box.
    pub fn remember(&self, r: &mut crate::remembered::Remembered) {
        r.set("dialin_serve", self.serve);
        r.set("account_name", &self.account.name);
        r.set("account_password", &self.account.password);
        r.set("login_command", &self.command);
    }

    pub fn recall(r: &crate::remembered::Remembered) -> Self {
        let mut s = Self::default();
        if let Some(v) = r.get("dialin_serve") {
            s.serve = v;
        }
        if let Some(v) = r.text("account_name") {
            v.clone_into(&mut s.account.name);
        }
        if let Some(v) = r.text("account_password") {
            v.clone_into(&mut s.account.password);
        }
        if let Some(v) = r.text("login_command") {
            v.clone_into(&mut s.command);
        }
        s
    }

    /// Everything in one string, for noticing that something changed.
    pub fn fingerprint(&self) -> String {
        format!("{}\u{1}{}\u{1}{}\u{1}{}", self.serve, self.account.name, self.account.password, self.command)
    }
}

/// Something different every call, for CHAP's challenges (RFC 1994 2.3 wants
/// them unpredictable). The clock to the nanosecond, hashed with the random
/// keys the standard library already draws for its hash maps.
pub fn challenge_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    hasher.finish()
}

/// What a login has come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Start PPP. `user` is who logged in at the text prompt, if anyone did;
    /// when a caller did not, PPP has to ask. `early` is the far end's frame
    /// already arriving.
    Ppp { user: Option<String>, early: Vec<u8> },
    /// Put the call down.
    HangUp(String),
    /// Stop, and give the terminal back.
    GiveBack(String),
}

fn from_script(outcome: login::script::Outcome) -> Next {
    match outcome {
        login::script::Outcome::Ppp { early } => Next::Ppp { user: None, early },
        login::script::Outcome::Failed(why) => Next::GiveBack(why),
    }
}

/// The login on one call.
#[derive(Debug)]
pub struct Login {
    kind: Kind,
    /// What it came to, held until the next step hands it on. Anything that
    /// arrives in between is PPP's, and is kept with it rather than dropped:
    /// losing the far end's first frames would cost a three-second retry.
    pending: Option<Next>,
}

#[derive(Debug)]
enum Kind {
    Serving(Box<login::Server>),
    Calling(Box<login::Script>),
}

impl Login {
    /// A terminal server for a caller.
    pub fn serve(settings: &Settings, tx: &Publisher) -> Self {
        let dotted = |[a, b, c, d]: [u8; 4]| format!("{a}.{b}.{c}.{d}");
        let config = login::server::Config {
            accounts: settings.callers(),
            ppp_message: format!(
                "PPP session from {} to {} beginning....",
                dotted(SERVER_ADDRESS),
                dotted(CLIENT_ADDRESS)
            ),
            ..login::server::Config::default()
        };
        tx.log(Direction::Note, "dial-in: answering with a login prompt");
        let mut server = login::Server::new(config);
        for note in server.take_notes() {
            tx.log(Direction::Note, format!("dial-in: {note}"));
        }
        Self { kind: Kind::Serving(Box::new(server)), pending: None }
    }

    /// A script to log in to the far end.
    pub fn call(settings: &Settings, tx: &Publisher) -> Self {
        tx.log(
            Direction::Note,
            format!("login: logging in as {} and waiting for PPP", settings.account.name),
        );
        let script = login::Script::new(settings.account.clone(), &settings.command);
        Self { kind: Kind::Calling(Box::new(script)), pending: None }
    }

    /// Whether this is the answering end's terminal server.
    pub fn serving(&self) -> bool {
        matches!(self.kind, Kind::Serving(_))
    }

    /// Where it has got to, for the window.
    pub fn stage(&self) -> String {
        match &self.kind {
            Kind::Serving(server) => match server.user() {
                Some(user) => format!("dial-in: {user} at the prompt"),
                None => format!("dial-in: {}", server.stage()),
            },
            Kind::Calling(script) => format!("logging in: {}", script.stage()),
        }
    }

    /// What the modem handed up.
    pub fn feed(&mut self, bytes: &[u8], tx: &Publisher) {
        if let Some(Next::Ppp { early, .. }) = &mut self.pending {
            early.extend_from_slice(bytes);
            return;
        }
        if self.pending.is_some() {
            return;
        }
        match &mut self.kind {
            Kind::Serving(server) => server.feed(bytes),
            Kind::Calling(script) => {
                script.feed(bytes);
                let ended = script.take_outcome();
                // The far end's text goes on the screen, up to where its PPP
                // begins; a frame is not something anyone wants to look at.
                let text = match &ended {
                    Some(login::script::Outcome::Ppp { early }) => &bytes[..bytes.len().saturating_sub(early.len())],
                    _ => bytes,
                };
                if !text.is_empty() {
                    tx.line_data(text);
                }
                self.pending = ended.map(from_script);
            }
        }
        self.collect();
    }

    /// Move whatever the login has come to into `pending`.
    fn collect(&mut self) {
        if self.pending.is_some() {
            return;
        }
        self.pending = match &mut self.kind {
            Kind::Serving(server) => server.take_outcome().map(|o| match o {
                login::server::Outcome::Ppp { user, early } => Next::Ppp { user, early },
                login::server::Outcome::HangUp(why) => Next::HangUp(why),
            }),
            Kind::Calling(script) => script.take_outcome().map(from_script),
        };
    }

    /// Let `ms` pass. Gives back what should go down the line, and what comes
    /// next if the login is over.
    pub fn step(&mut self, ms: u32, tx: &Publisher) -> (Vec<u8>, Option<Next>) {
        let out = match &mut self.kind {
            Kind::Serving(server) => {
                server.tick(ms);
                let out = server.take_output();
                // Whoever is sitting at the answering machine sees what the
                // caller sees.
                if !out.is_empty() {
                    tx.line_data(&out);
                }
                for note in server.take_notes() {
                    tx.log(Direction::Note, format!("dial-in: {note}"));
                }
                out
            }
            Kind::Calling(script) => {
                script.tick(ms);
                for note in script.take_notes() {
                    tx.log(Direction::Note, format!("login: {note}"));
                }
                script.take_output()
            }
        };
        self.collect();
        (out, self.pending.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both halves on one pretend call, octets handed straight across, until
    /// both have said PPP.
    #[test]
    fn a_caller_logs_in_to_an_answering_end_and_both_reach_ppp() {
        let (tx, _rx) = telemetry::channel(64, 32, 8_000.0);
        let settings = Settings { serve: true, account: Account::new("guest", "letmein"), ..Settings::default() };
        let mut server = Login::serve(&settings, &tx);
        let mut caller = Login::call(&settings, &tx);
        let (mut served, mut called) = (None, None);
        for _ in 0..30_000 {
            let (to_caller, next) = server.step(1, &tx);
            served = served.or(next);
            caller.feed(&to_caller, &tx);
            let (to_server, next) = caller.step(1, &tx);
            called = called.or(next);
            server.feed(&to_server, &tx);
            if let Some(Next::Ppp { .. }) = &served
                && called.is_none()
            {
                // The answering end's link would open now and send LCP.
                caller.feed(&[0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21], &tx);
            }
            if served.is_some() && called.is_some() {
                break;
            }
        }
        assert_eq!(served, Some(Next::Ppp { user: Some("guest".into()), early: vec![] }));
        assert!(matches!(called, Some(Next::Ppp { .. })), "{called:?}");
    }

    /// The far end's frame keeps arriving between the login noticing it and
    /// the link being started, and none of it may be lost.
    #[test]
    fn what_arrives_after_ppp_is_spotted_is_kept_for_the_link() {
        let (tx, _rx) = telemetry::channel(64, 32, 8_000.0);
        let mut caller = Login::call(&Settings::default(), &tx);
        caller.feed(&[0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21], &tx);
        caller.feed(&[0x7d, 0x21, 0x7e], &tx);
        let (_, next) = caller.step(1, &tx);
        assert_eq!(
            next,
            Some(Next::Ppp { user: None, early: vec![0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21, 0x7d, 0x21, 0x7e] })
        );
    }

    #[test]
    fn settings_come_back_as_they_were_left() {
        let settings = Settings { serve: true, account: Account::new("rory", "hunter 2"), command: "start ppp".into() };
        let mut r = crate::remembered::Remembered::default();
        settings.remember(&mut r);
        assert_eq!(Settings::recall(&r), settings);
        assert_eq!(Settings::recall(&crate::remembered::Remembered::default()), Settings::default());
    }
}
