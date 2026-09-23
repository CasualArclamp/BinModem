//! The accounts file: what a provider gave us, and what we choose about how to
//! use it.
//!
//! There is no way round having one. A trunk is a username, a password and a
//! host that somebody else issued, and none of it can be guessed or
//! discovered. The question is only what shape it is kept in, and the answer
//! here is the same one [`crate::message`] gives for headers and
//! `gui::remembered` gives for settings: a text file a person can open, read
//! and fix. A configuration that can only be edited by the program that wrote
//! it is a configuration that has to be deleted when it goes wrong, and a
//! trunk that will not register is exactly when somebody needs to read the
//! file to find out why.
//!
//! So: `[section]` per account, `name = value` under it, `#` for a comment.
//!
//! Forgiving about what is missing, strict about what is misspelt. Every
//! optional key has a default that is right for an ordinary trunk, so a
//! four-line section works. But an unknown key is refused, by name and line
//! number, rather than ignored -- because the key most likely to be misspelt
//! is `password`, and a password line that silently does nothing produces a
//! registration failure with no visible cause at all. A refusal that names
//! the line is a minute's work; the other thing is an evening's.
//!
//! The file holds the password in clear text, and that is not a lapse to be
//! fixed later. Digest authentication (RFC 7616 3.4.2) takes the password
//! itself as an input to the hash, so there is nothing else that could be
//! stored: anything reversible enough to compute a response with is a
//! password in a costume. The file is as private as the user's profile
//! directory, which is where [`Account::default_path`] puts it, and the
//! template says so plainly.

use std::path::{Path, PathBuf};

use crate::g711::Law;
use crate::uri::{self, Uri};

/// One SIP account: what a provider gave us, and what we choose about how to
/// use it.
///
/// Comparable so that a window editing one can tell whether anything has
/// actually changed, and only write the file when something has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The section name in the file, e.g. "trunk". What the account is
    /// called everywhere else in the program.
    pub name: String,
    /// Where REGISTER and INVITE are sent: host, or host:port.
    pub registrar: String,
    /// The domain in our own SIP URI, which is usually but not always the
    /// registrar's host.
    pub domain: String,
    pub username: String,
    /// The name used for authentication when it differs from the username,
    /// which on some trunks it does.
    pub auth_username: Option<String>,
    pub password: String,
    pub display: Option<String>,
    /// Where requests are actually sent if that is not the registrar.
    pub outbound_proxy: Option<String>,
    /// Whether to register at all. A PBX on the LAN often wants no
    /// registration and will take an INVITE from a known address.
    pub register: bool,
    /// Seconds. What we ask for; the registrar may grant less, and what it
    /// grants is what the next refresh goes by (RFC 3261 10.2.4).
    pub expires: u32,
    /// Companding laws to offer, in preference order. Default mu-law first.
    pub laws: Vec<Law>,
    /// The local SIP port. 0 = let the system choose, which is right unless
    /// something upstream is forwarding a fixed one.
    pub local_port: u16,
    /// The local RTP port. 0 = let the system choose.
    pub rtp_port: u16,
    /// Milliseconds of audio in each RTP packet. 20 is what everything
    /// expects and what the jitter buffer is sized around.
    pub ptime_ms: u32,
    /// Which transport the SIP messages themselves travel over. The media
    /// never changes: RTP is UDP whatever this says.
    pub transport: Transport,
}

/// How SIP messages reach the trunk. RFC 3261 18, and 7.5 for the framing
/// difference that makes them two things rather than one setting.
///
/// UDP is what a trunk expects and what every timer in [`crate::ua`] is
/// written around: a datagram is a message, and the protocol does its own
/// retransmission because nothing underneath it will. TCP is a stream, so a
/// message has to be found in it by its Content-Length, and the
/// retransmissions must *not* happen -- sending a request twice over a
/// reliable transport is a duplicate the far end has to sort out for no
/// reason.
///
/// Worth having because some trunks insist on it, and because a message that
/// grows past the path MTU -- which an INVITE with a long SDP can -- is
/// fragmented over UDP and dropped by a surprising number of routers. RFC
/// 3261 18.1.1 in fact requires a user agent to switch to TCP at 1300 octets;
/// this one does not, and says so rather than pretending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    #[default]
    Udp,
    Tcp,
}

impl Transport {
    /// The name as it is written in a Via header and a URI parameter, which
    /// RFC 3261 7.1 wants in upper case.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Udp => "UDP",
            Self::Tcp => "TCP",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "udp" => Some(Self::Udp),
            "tcp" => Some(Self::Tcp),
            _ => None,
        }
    }
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Default for Account {
    /// The defaults an ordinary trunk wants, so that a section needs only the
    /// four things nobody could guess.
    fn default() -> Self {
        Self {
            name: String::new(),
            registrar: String::new(),
            domain: String::new(),
            username: String::new(),
            auth_username: None,
            password: String::new(),
            display: None,
            outbound_proxy: None,
            register: true,
            expires: 300,
            // mu-law first: a V.90 server's codewords are mu-law, and on a
            // transcoding trunk the law that needs no conversion is the one
            // that keeps them.
            laws: vec![Law::Mu, Law::A],
            local_port: 0,
            rtp_port: 0,
            ptime_ms: 20,
            transport: Transport::Udp,
        }
    }
}

impl Account {
    /// Parse the whole file. Every account in it, or the first complaint.
    pub fn parse(text: &str) -> Result<Vec<Account>, String> {
        let mut accounts: Vec<Account> = Vec::new();
        // Where the current section started, so that a missing required key
        // can be complained about at the line a person would look at.
        let mut section_line = 0usize;

        for (index, raw) in text.lines().enumerate() {
            let number = index + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }

            if let Some(rest) = line.strip_prefix('[') {
                let Some(name) = rest.strip_suffix(']') else {
                    return Err(format!(
                        "line {number}: a section heading needs a closing bracket, as in [trunk]"
                    ));
                };
                let name = name.trim();
                if name.is_empty() {
                    return Err(format!("line {number}: this section has no name"));
                }
                if accounts.iter().any(|a| a.name == name) {
                    return Err(format!(
                        "line {number}: there is already an account called \"{name}\""
                    ));
                }
                if let Some(previous) = accounts.last() {
                    previous.check_complete(section_line)?;
                }
                section_line = number;
                accounts.push(Account {
                    name: name.to_owned(),
                    ..Account::default()
                });
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(format!(
                    "line {number}: expected \"name = value\", or a [section] heading"
                ));
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            let Some(account) = accounts.last_mut() else {
                return Err(format!(
                    "line {number}: \"{key}\" comes before any [section] heading, so there is no \
                     account for it to belong to"
                ));
            };
            account.set(&key, value, number)?;
        }

        if let Some(previous) = accounts.last() {
            previous.check_complete(section_line)?;
        }
        Ok(accounts.into_iter().map(Account::settled).collect())
    }

    /// One `name = value` from the file. The place where a misspelt key is
    /// refused rather than dropped.
    fn set(&mut self, key: &str, value: &str, number: usize) -> Result<(), String> {
        match key {
            "registrar" => self.registrar = unquote(value),
            "domain" => self.domain = unquote(value),
            "username" => self.username = unquote(value),
            "auth_username" => self.auth_username = optional(&unquote(value)),
            "password" => self.password = unquote(value),
            "display" => self.display = optional(&unquote(value)),
            "outbound_proxy" => self.outbound_proxy = optional(&unquote(value)),
            "register" => self.register = parse_yes_no(value, key, number)?,
            "expires" => self.expires = parse_number(value, key, number)?,
            "laws" => self.laws = parse_laws(value, number)?,
            "local_port" => self.local_port = parse_number(value, key, number)?,
            "rtp_port" => self.rtp_port = parse_number(value, key, number)?,
            "ptime_ms" => self.ptime_ms = parse_number(value, key, number)?,
            "transport" => {
                self.transport = Transport::parse(value).ok_or_else(|| {
                    format!("line {number}: \"transport\" wants udp or tcp, not \"{value}\"")
                })?;
            }
            _ => {
                return Err(format!(
                    "line {number}: \"{key}\" is not a setting this understands. The settings are \
                     registrar, domain, username, auth_username, password, display, \
                     outbound_proxy, register, expires, laws, local_port, rtp_port, ptime_ms \
                     and transport"
                ));
            }
        }
        Ok(())
    }

    /// The four things that cannot be defaulted, plus the domain, which can
    /// be: it falls back to the registrar's host, which is right for nearly
    /// every trunk.
    fn check_complete(&self, section_line: usize) -> Result<(), String> {
        let missing = |what: &str| {
            Err(format!(
                "line {section_line}: the account \"{}\" has no {what} line",
                self.name
            ))
        };
        if self.registrar.is_empty() {
            return missing("registrar");
        }
        if self.username.is_empty() {
            return missing("username");
        }
        // A password is required even when register is off, because a PBX
        // that takes an unregistered INVITE will still challenge it.
        if self.password.is_empty() {
            return missing("password");
        }
        if self.laws.is_empty() {
            return Err(format!(
                "line {section_line}: the account \"{}\" offers no companding law, so there is \
                 nothing it could carry",
                self.name
            ));
        }
        Ok(())
    }

    /// Fill in what was left out, once the section is known to be complete.
    fn settled(mut self) -> Self {
        if self.domain.is_empty() {
            // The registrar's host, without its port: a port belongs in a
            // socket address and never in the domain part of a URI.
            let (host, _) = uri::split_host_port(&self.registrar);
            self.domain = host;
        }
        self
    }

    /// Read a file. A file that is named and cannot be read is an error, in
    /// contrast to [`load_default`](Self::load_default), where an absent file
    /// means something quite different.
    pub fn load(path: &Path) -> Result<Vec<Account>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{} could not be read: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// Where the file lives by default.
    ///
    /// The same place, by the same rules, as the window's settings file: the
    /// per-user application directory, which on Windows is the one directory
    /// that is always the user's to write in. Beside the executable would be
    /// wrong -- that may be read-only, and this program is meant to be one
    /// file that can sit anywhere.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("BinModem").join("sip.txt"))
    }

    /// Read the default file, or `Ok(vec![])` when there is not one. A missing
    /// file is a first run, not an error.
    pub fn load_default() -> Result<Vec<Account>, String> {
        let Some(path) = Self::default_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        Self::load(&path)
    }

    /// Write the commented template to `path` if nothing is there yet, so a
    /// person has something to fill in. Returns whether it wrote one.
    ///
    /// Never overwrites. The one file in this program that a person has typed
    /// a password into is not one to be rewritten by a program that thinks it
    /// knows better.
    pub fn write_example(path: &Path) -> Result<bool, String> {
        if path.exists() {
            return Ok(false);
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{} could not be created: {e}", parent.display()))?;
        }
        std::fs::write(path, Self::EXAMPLE)
            .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
        Ok(true)
    }

    /// The commented template itself.
    pub const EXAMPLE: &'static str = EXAMPLE;

    /// These accounts as the file that would parse back into them.
    ///
    /// Only what is set is written. A default that has not been changed is
    /// left out rather than spelled out, so a file written from an account
    /// somebody typed into a window still reads like something a person would
    /// write -- five lines, not fifteen -- and the defaults stay in one place
    /// instead of being copied into every file and frozen there.
    pub fn to_text(accounts: &[Account]) -> String {
        let mut out = String::from(WRITTEN_HEADER);
        for account in accounts {
            let fallback = Account::default();
            out.push_str(&format!("\n[{}]\n", account.name));
            let mut line = |key: &str, value: &str| {
                out.push_str(&format!("{key:<15}= {}\n", quote_if_needed(value)));
            };
            line("registrar", &account.registrar);
            // The domain is the registrar's host unless somebody said
            // otherwise, and writing it out when it agrees only invites the
            // two to drift apart later.
            if account.domain != uri::split_host_port(&account.registrar).0 {
                line("domain", &account.domain);
            }
            line("username", &account.username);
            if let Some(auth) = &account.auth_username {
                line("auth_username", auth);
            }
            line("password", &account.password);
            if let Some(display) = &account.display {
                line("display", display);
            }
            if let Some(proxy) = &account.outbound_proxy {
                line("outbound_proxy", proxy);
            }
            if !account.register {
                line("register", "no");
            }
            if account.expires != fallback.expires {
                line("expires", &account.expires.to_string());
            }
            if account.laws != fallback.laws {
                let laws: Vec<&str> = account
                    .laws
                    .iter()
                    .map(|l| match l {
                        Law::Mu => "mu",
                        Law::A => "a",
                    })
                    .collect();
                line("laws", &laws.join(", "));
            }
            if account.local_port != fallback.local_port {
                line("local_port", &account.local_port.to_string());
            }
            if account.rtp_port != fallback.rtp_port {
                line("rtp_port", &account.rtp_port.to_string());
            }
            if account.ptime_ms != fallback.ptime_ms {
                line("ptime_ms", &account.ptime_ms.to_string());
            }
            if account.transport != fallback.transport {
                line("transport", &account.transport.to_string().to_ascii_lowercase());
            }
        }
        out
    }

    /// Write them to the file, having first checked they read back.
    ///
    /// The check is not ceremony. This is the only file in the program whose
    /// contents are a password, and a window that wrote one the parser then
    /// refused would leave somebody with an account that had silently stopped
    /// existing -- and no reason to look in the file, because they had just
    /// typed it into a window.
    pub fn save_all(path: &Path, accounts: &[Account]) -> Result<(), String> {
        let text = Self::to_text(accounts);
        let read_back = Self::parse(&text)
            .map_err(|e| format!("what was about to be written would not read back: {e}"))?;
        if read_back.len() != accounts.len() {
            return Err(format!(
                "{} accounts were written and {} read back",
                accounts.len(),
                read_back.len()
            ));
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{} could not be created: {e}", parent.display()))?;
        }
        std::fs::write(path, text)
            .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
        Ok(())
    }

    /// The same, to wherever the file belongs on this machine.
    pub fn save_default(accounts: &[Account]) -> Result<(), String> {
        let path = Self::default_path()
            .ok_or_else(|| "there is nowhere to keep the account file".to_owned())?;
        Self::save_all(&path, accounts)
    }

    /// What is missing before this account could place a call, in words a
    /// person can act on. Empty means it is ready.
    ///
    /// Checked here rather than in the window because the window is not the
    /// only caller, and because what makes an account usable is this file's
    /// business.
    pub fn what_is_missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.name.trim().is_empty() {
            missing.push("a name to call this account by".to_owned());
        }
        if self.registrar.trim().is_empty() {
            missing.push("the registrar the provider gave you".to_owned());
        }
        if self.username.trim().is_empty() {
            missing.push("the username, which on a trunk is usually the number".to_owned());
        }
        if self.password.is_empty() {
            missing.push("the SIP password from the provider's portal".to_owned());
        } else if self.password.starts_with("PUT-THE-") {
            missing.push("a real password -- the example's placeholder is still there".to_owned());
        }
        missing
    }

    /// `sip:username@domain` -- who we are.
    pub fn uri(&self) -> Uri {
        Uri::user_at(&self.username, &self.domain)
    }

    /// The name to authenticate as.
    pub fn auth_name(&self) -> &str {
        self.auth_username.as_deref().unwrap_or(&self.username)
    }

    /// Where to send a request: the outbound proxy if there is one, else the
    /// registrar.
    ///
    /// These come apart more often than one would expect. A trunk that gives
    /// out a registrar name resolving to several machines will often want
    /// every request for one call to go to the same one, and names that
    /// machine as a proxy.
    pub fn next_hop(&self) -> &str {
        self.outbound_proxy.as_deref().unwrap_or(&self.registrar)
    }

    /// `sip:number@domain` -- who we are calling.
    ///
    /// What arrives here is whatever followed `ATD`, and a person dialling a
    /// modem types a telephone number the way a telephone number is written:
    /// with spaces, or brackets round the area code, or the dial modifiers
    /// that meant tone and pulse when those were a choice. None of that
    /// belongs in a URI, and none of it is a reason to refuse the call, so it
    /// comes out here.
    ///
    /// A full SIP URI passes through untouched, so that `ATD sip:1000@pbx.local`
    /// reaches an extension that has no number at all.
    pub fn dial_uri(&self, dialled: &str) -> Uri {
        // A trailing semicolon is AT's "return to command state after
        // dialling" and is not part of anything being dialled.
        let text = dialled.trim().trim_end_matches(';').trim();
        let lower = text.to_ascii_lowercase();
        if lower.starts_with("sip:") || lower.starts_with("sips:") {
            // Parsed to normalise it, and passed through as written if it
            // will not parse: a far end is better placed than we are to say
            // what is wrong with an address somebody typed.
            if let Some(uri) = Uri::parse(text) {
                return uri;
            }
        }
        let written: String = text.chars().filter(|ch| !is_punctuation(*ch)).collect();
        // Whether the dial modifiers in it are modifiers at all.
        //
        // T, P and W are letters, and a PBX extension is allowed to be a
        // word. Stripping them unconditionally meant `ATD support` dialled
        // `suor`, which is not a refusal and not a call either -- it is a
        // wrong number, placed confidently. So they count as modifiers only
        // when what surrounds them is a telephone number; anything with an
        // ordinary letter in it is a name, and a name is kept whole.
        let a_number = written
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '+' | '*' | '#') || is_modifier(ch));
        let number: String = if a_number {
            let stripped: String = written.chars().filter(|ch| !is_modifier(*ch)).collect();
            // Unless there is nothing left, which means it was a name made
            // only of those letters after all.
            if stripped.is_empty() { written } else { stripped }
        } else {
            written
        };
        Uri::user_at(&number, &self.domain)
    }
}

/// Punctuation a person writes between the digits of a telephone number,
/// which a URI must not carry. A comma paused the dialling; the rest is
/// simply how a number is written down.
fn is_punctuation(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '-' | '(' | ')' | '.' | '/' | ',' | '!')
}

/// The dial modifiers. T and P chose tone or pulse dialling and W waited for
/// a second dial tone: all of them instructions to a modem about a copper
/// line, and none of them survives into a packet.
fn is_modifier(ch: char) -> bool {
    matches!(ch, 'T' | 't' | 'P' | 'p' | 'W' | 'w')
}

/// An empty value means the key was written and left blank, which is the same
/// as not writing it. Better than storing an empty string that then goes out
/// in a header.
fn optional(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// A `#` starts a comment at the beginning of a line or after whitespace, and
/// nowhere else. A password is perfectly entitled to contain a hash, and
/// swallowing the rest of the line from one would be the silent failure this
/// whole file is arranged to avoid.
/// Except inside quotes, where nothing starts a comment. That is the one
/// escape hatch the format has, and it exists because the rule above is not
/// quite enough: a password of `a b # c` has a hash *after a space*, so by
/// that rule it is a comment, and there was no way to write one down. Found
/// by the test that writes an account and reads it back, which is what that
/// test is for.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quoted = false;
    for (at, ch) in line.char_indices() {
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if !quoted && ch == '#' && (at == 0 || bytes[at - 1] == b' ' || bytes[at - 1] == b'\t') {
            return &line[..at];
        }
    }
    line
}

/// A value as it was meant, with the quotes taken off if it had them.
///
/// `""` inside a quoted value is one quote character. That is the only escape
/// there is, and it is the ordinary convention for this kind of file --
/// enough to write any password down, and little enough that somebody editing
/// the file by hand does not have to learn a language first.
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let Some(inner) = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .filter(|_| trimmed.len() >= 2)
    else {
        return value.to_owned();
    };
    inner.replace("\"\"", "\"")
}

/// And the other way: quoted only when it has to be, so a file the program
/// wrote still looks like one a person wrote.
fn quote_if_needed(value: &str) -> String {
    let awkward =
        value.contains('#') || value.contains('"') || value.trim() != value || value.is_empty();
    if !awkward {
        return value.to_owned();
    }
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn parse_number<T: std::str::FromStr>(value: &str, key: &str, number: usize) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("line {number}: \"{key}\" wants a whole number, not \"{value}\""))
}

fn parse_yes_no(value: &str, key: &str, number: usize) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Ok(true),
        "no" | "false" | "off" | "0" => Ok(false),
        _ => Err(format!(
            "line {number}: \"{key}\" wants yes or no, not \"{value}\""
        )),
    }
}

/// `laws = mu, a`. The order is the preference order, and it is offered to the
/// far end in that order.
fn parse_laws(value: &str, number: usize) -> Result<Vec<Law>, String> {
    let mut out = Vec::new();
    for item in value.split([',', ' ', '\t']) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let law = match item.to_ascii_lowercase().as_str() {
            // The spellings people actually write: the law, the RTP encoding
            // name, and the letter.
            "mu" | "u" | "ulaw" | "mulaw" | "mu-law" | "u-law" | "pcmu" => Law::Mu,
            "a" | "alaw" | "a-law" | "pcma" => Law::A,
            _ => {
                return Err(format!(
                    "line {number}: \"{item}\" is not a companding law. G.711 has two, mu and a, \
                     and this modem will offer no other codec"
                ));
            }
        };
        if !out.contains(&law) {
            out.push(law);
        }
    }
    if out.is_empty() {
        return Err(format!(
            "line {number}: \"laws\" was given nothing to offer"
        ));
    }
    Ok(out)
}

/// What goes at the top of a file this program wrote.
///
/// Said plainly, because a file that is edited in two places -- a window and a
/// text editor -- has to tell whoever opens it which of the two wins. Comments
/// are the thing lost when the window writes, and somebody who has spent an
/// evening annotating this file deserves to have been warned first.
const WRITTEN_HEADER: &str = "# BinModem SIP accounts.
#
# Written by the program, from the SIP window. Anything can be edited here by
# hand and will be read back -- but the next save from the window rewrites the
# whole file, and comments added here do not survive that.
#
# This file contains passwords in clear text. There is no alternative: SIP
# digest authentication computes its answer from the password itself, so what
# is stored has to be usable as the password is. It is as private as the
# directory it sits in and no more than that.
";

const EXAMPLE: &str = "\
# BinModem SIP accounts.
#
# One [section] for each account; the name in the brackets is what the account
# is called in the program. Under it, one `name = value` to a line. A `#`
# starts a comment, blank lines are ignored, and a setting that is left out
# takes the default noted against it below.
#
# A misspelt setting is refused by name and line number rather than ignored.
# That is deliberate: the line most worth misspelling is the password, and a
# password line that quietly did nothing would show up only as a trunk that
# will not register, with nothing in the log to say why.
#
# This file contains a password in clear text. There is no alternative to
# that: SIP digest authentication computes its answer from the password
# itself, so anything stored here has to be usable as the password is. The
# file is as private as the directory it sits in -- your own profile
# directory, which is where the program puts it -- and no more private than
# that. Treat it the way you would treat a note of the password, because that
# is what it is.

# There is no account in here yet, and that is deliberate: an account is a
# username, a password and a host that a provider issued to you, and nothing
# else can stand in for them. The easiest way in is the window -- the Dial
# button, then Credentials -- which writes this file for you. What follows is
# the same thing in longhand, for filling in by hand.
#
# Uncomment a section and edit it. Four settings matter; the rest have
# defaults that suit an ordinary trunk and can be left out entirely.
#
# [a name for this account]
# Where REGISTER and INVITE are sent. A host, or host:port for a trunk that
# does not use 5060.
# registrar  = sip.example.net
# The username the provider issued. On a telephone trunk it is often the
# number itself.
# username   = 1000
# The SIP password from the provider's portal -- not the password you log in
# to the portal with.
# password   = the-sip-password
# udp or tcp. UDP unless the provider says otherwise, or unless a long INVITE
# is being fragmented and dropped on the way out. Default udp.
# transport  = udp
#
# And the rest, each with the default it takes when left out.
#
# The domain in our own address. Defaults to the registrar host, which is
# right unless the provider has told you otherwise.
# domain     = sip.example.net
# Only needed when the provider authenticates you as something other than the
# username. Most trunks do not.
# auth_username = 1000
# The name shown to the far end, where anything shows it at all.
# display    = BinModem
# Only if the provider names a proxy separate from the registrar. Requests go
# there instead when it is set.
# outbound_proxy = proxy.example.net
# Whether to register at all. A trunk that authenticates by address rather
# than by registration wants no, and will never go green whatever you put in
# the password. Default yes.
# register   = yes
# How long a registration is asked to last, in seconds. The trunk may grant
# less, and what it grants is what gets used. Default 300.
# expires    = 300
# Which G.711 companding laws to offer, best first. Default mu, a. A V.90
# server's codewords are mu-law, so mu-law first means one conversion less on
# the path where it matters most.
# laws       = mu, a
# Local ports. 0 lets the system choose, which is right unless something
# upstream forwards a fixed port to this machine. Default 0 for both.
# local_port = 0
# rtp_port   = 0
# Milliseconds of audio in each RTP packet. Default 20, which is what
# everything at the far end expects.
# ptime_ms   = 20
#
# A PBX on the local network is the other common case. It usually wants no
# registration at all: it knows this machine already and will take a call
# from it.
#
# [pbx]
# registrar = 192.168.1.10:5060
# domain    = pbx.local
# username  = 1001
# password  = the-extension-password
# register  = no
";

#[cfg(test)]
mod tests {
    use super::*;

    const TRUNK: &str = "\
[trunk]
registrar = sip.example.net
username  = 0398765432
password  = hunter2
";

    fn one(text: &str) -> Account {
        let accounts = Account::parse(text).expect("parses");
        assert_eq!(accounts.len(), 1);
        accounts.into_iter().next().unwrap()
    }

    /// Four lines is a working account, and everything else takes a default
    /// that suits an ordinary trunk.
    #[test]
    fn the_smallest_account_that_works() {
        let account = one(TRUNK);
        assert_eq!(account.name, "trunk");
        assert_eq!(account.registrar, "sip.example.net");
        // Not written, so taken from the registrar.
        assert_eq!(account.domain, "sip.example.net");
        assert_eq!(
            account.uri().to_string(),
            "sip:0398765432@sip.example.net"
        );
        assert_eq!(account.auth_name(), "0398765432");
        assert_eq!(account.next_hop(), "sip.example.net");
        assert!(account.register);
        assert_eq!(account.expires, 300);
        assert_eq!(account.laws, vec![Law::Mu, Law::A]);
        assert_eq!(account.ptime_ms, 20);
        assert_eq!(account.local_port, 0);
        assert_eq!(account.rtp_port, 0);
    }

    /// A registrar written with a port does not put the port in the domain:
    /// a port belongs in a socket address, and a URI with one in the domain
    /// part is a different address to the far end.
    #[test]
    fn a_port_on_the_registrar_stays_out_of_the_domain() {
        let account = one("[pbx]\nregistrar = 192.168.1.10:5060\nusername = 1001\npassword = x\n");
        assert_eq!(account.domain, "192.168.1.10");
        assert_eq!(account.uri().to_string(), "sip:1001@192.168.1.10");
        assert_eq!(account.next_hop(), "192.168.1.10:5060");
    }

    #[test]
    fn everything_that_can_be_set_can_be_set() {
        let account = one("\
[trunk]
registrar      = sip.example.net:5070
domain         = example.net
username       = 0398765432
auth_username  = 61398765432
password       = a secret with spaces
display        = BinModem
outbound_proxy = proxy.example.net
register       = no
expires        = 1800
laws           = a, mu
local_port     = 5062
rtp_port       = 40000
ptime_ms       = 10
");
        assert_eq!(account.domain, "example.net");
        assert_eq!(account.auth_name(), "61398765432");
        assert_eq!(account.password, "a secret with spaces");
        assert_eq!(account.display.as_deref(), Some("BinModem"));
        assert_eq!(account.next_hop(), "proxy.example.net");
        assert!(!account.register);
        assert_eq!(account.expires, 1800);
        assert_eq!(account.laws, vec![Law::A, Law::Mu]);
        assert_eq!(account.local_port, 5062);
        assert_eq!(account.rtp_port, 40000);
        assert_eq!(account.ptime_ms, 10);
    }

    #[test]
    fn several_accounts_and_comments_and_blank_lines() {
        let accounts = Account::parse(
            "\
# The trunk.

[trunk]
registrar = sip.example.net   # where it goes
username  = 0398765432
password  = hunter2

# And the PBX in the cupboard.
[pbx]
registrar = 192.168.1.10
domain    = pbx.local
username  = 1001
password  = x
register  = no
",
        )
        .unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].name, "trunk");
        assert_eq!(accounts[0].registrar, "sip.example.net");
        assert_eq!(accounts[1].name, "pbx");
        assert!(!accounts[1].register);
    }

    /// A hash in the middle of a password is part of the password. Only one
    /// that starts a word starts a comment.
    #[test]
    fn a_hash_inside_a_value_is_not_a_comment() {
        let account = one("[t]\nregistrar=h\nusername=u\npassword = a#b#c\n");
        assert_eq!(account.password, "a#b#c");
        let account = one("[t]\nregistrar=h\nusername=u\npassword = abc # and a note\n");
        assert_eq!(account.password, "abc");
    }

    /// The whole reason unknown keys are refused: this is what a misspelt
    /// password looks like, and it must not be silence.
    #[test]
    fn a_misspelt_setting_is_refused_by_name_and_line() {
        let error =
            Account::parse("[t]\nregistrar = h\nusername = u\npasword = hunter2\n").unwrap_err();
        assert!(error.contains("line 4"), "{error}");
        assert!(error.contains("pasword"), "{error}");
    }

    #[test]
    fn what_is_missing_is_named_at_the_section_it_is_missing_from() {
        let error = Account::parse(
            "[first]\nregistrar = h\nusername = u\npassword = p\n\n[second]\nregistrar = h2\n",
        )
        .unwrap_err();
        assert!(error.contains("line 6"), "{error}");
        assert!(error.contains("second"), "{error}");
        assert!(error.contains("username"), "{error}");
    }

    #[test]
    fn nonsense_values_are_refused_as_sentences() {
        for (text, wanted) in [
            (
                "[t]\nregistrar=h\nusername=u\npassword=p\nexpires = soon\n",
                "expires",
            ),
            (
                "[t]\nregistrar=h\nusername=u\npassword=p\nregister = maybe\n",
                "register",
            ),
            (
                "[t]\nregistrar=h\nusername=u\npassword=p\nlaws = opus\n",
                "opus",
            ),
        ] {
            let error = Account::parse(text).unwrap_err();
            assert!(error.contains("line 5"), "{error}");
            assert!(error.contains(wanted), "{error}");
        }
    }

    #[test]
    fn a_setting_before_any_section_says_so() {
        let error = Account::parse("registrar = h\n").unwrap_err();
        assert!(error.contains("line 1"), "{error}");
        assert!(error.contains("section"), "{error}");
        let error = Account::parse("[unclosed\n").unwrap_err();
        assert!(error.contains("line 1"), "{error}");
    }

    #[test]
    fn two_accounts_cannot_share_a_name() {
        let error = Account::parse("[t]\nregistrar=h\nusername=u\npassword=p\n[t]\n").unwrap_err();
        assert!(error.contains("already"), "{error}");
    }

    /// Every way a person writes a telephone number, and the one way they
    /// write an extension that has no number.
    /// What the window writes reads back as what it wrote. The one property
    /// that matters: this file is the only place a password lives, and a save
    /// that quietly changed an account would show up as a trunk refusing to
    /// register, with nothing to say why.
    #[test]
    fn what_is_written_reads_back_the_same() {
        let accounts = vec![
            Account {
                name: "trunk".to_owned(),
                registrar: "sip.example.net".to_owned(),
                domain: "sip.example.net".to_owned(),
                username: "0398765432".to_owned(),
                password: "a password with spaces and a # in it".to_owned(),
                display: Some("BinModem".to_owned()),
                ..Account::default()
            },
            Account {
                name: "pbx".to_owned(),
                registrar: "192.168.1.10:5060".to_owned(),
                domain: "pbx.local".to_owned(),
                username: "1001".to_owned(),
                auth_username: Some("1001-auth".to_owned()),
                password: "another".to_owned(),
                outbound_proxy: Some("proxy.local".to_owned()),
                register: false,
                expires: 120,
                laws: vec![Law::A],
                local_port: 5062,
                rtp_port: 40000,
                ptime_ms: 30,
                ..Account::default()
            },
        ];
        let text = Account::to_text(&accounts);
        let back = Account::parse(&text).expect("what was written would not parse");
        assert_eq!(back.len(), 2);
        for (wrote, read) in accounts.iter().zip(back.iter()) {
            assert_eq!(wrote.name, read.name);
            assert_eq!(wrote.registrar, read.registrar);
            assert_eq!(wrote.domain, read.domain, "the domain of {}", wrote.name);
            assert_eq!(wrote.username, read.username);
            assert_eq!(wrote.auth_username, read.auth_username);
            assert_eq!(wrote.password, read.password, "the password of {}", wrote.name);
            assert_eq!(wrote.display, read.display);
            assert_eq!(wrote.outbound_proxy, read.outbound_proxy);
            assert_eq!(wrote.register, read.register);
            assert_eq!(wrote.expires, read.expires);
            assert_eq!(wrote.laws, read.laws);
            assert_eq!(wrote.local_port, read.local_port);
            assert_eq!(wrote.rtp_port, read.rtp_port);
            assert_eq!(wrote.ptime_ms, read.ptime_ms);
        }
        // An account left at its defaults is written short, not spelled out.
        assert!(!text.contains("expires        = 300"), "{text}");
    }

    /// A password made entirely of the characters this format uses for its
    /// own purposes still survives a trip through the file.
    ///
    /// Not a hypothetical: a provider's portal will hand out `#` and quotes
    /// as readily as letters, and the failure this guards against is the
    /// quiet one -- a password saved, truncated at the hash, and a trunk that
    /// refuses to register with nothing anywhere to say why.
    #[test]
    fn a_password_of_nothing_but_punctuation_survives() {
        for password in [
            "a password with spaces and a # in it",
            "#leading",
            "trailing#",
            "has \"quotes\" in it",
            "\"wrapped in quotes\"",
            "  leading and trailing spaces  ",
            "everything # \" at once",
        ] {
            let account = Account {
                name: "t".to_owned(),
                registrar: "host".to_owned(),
                username: "1".to_owned(),
                password: password.to_owned(),
                ..Account::default()
            };
            let text = Account::to_text(std::slice::from_ref(&account));
            let back = Account::parse(&text)
                .unwrap_or_else(|e| panic!("{password:?} was written as\n{text}\nand refused: {e}"));
            assert_eq!(back[0].password, password, "written as\n{text}");
        }
    }

    /// The window asks what is missing before it lets somebody press Dial.
    #[test]
    fn an_account_says_what_it_still_needs() {
        let empty = Account::default();
        assert_eq!(empty.what_is_missing().len(), 4, "{:?}", empty.what_is_missing());

        let half_done = Account {
            name: "trunk".to_owned(),
            registrar: "sip.example.net".to_owned(),
            username: "1000".to_owned(),
            ..Account::default()
        };
        let missing = half_done.what_is_missing();
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("password"), "{missing:?}");

        let ready = Account {
            name: "t".to_owned(),
            registrar: "host".to_owned(),
            username: "1".to_owned(),
            password: "p".to_owned(),
            ..Account::default()
        };
        assert!(ready.what_is_missing().is_empty());
    }

    #[test]
    fn a_dialled_number_becomes_a_uri() {
        let account = one(TRUNK);
        let expected = "sip:0398765432@sip.example.net";
        // ATD 0398765432
        assert_eq!(account.dial_uri("0398765432").to_string(), expected);
        // ATD 03 9876 5432
        assert_eq!(account.dial_uri(" 03 9876 5432").to_string(), expected);
        // ATDT0398765432
        assert_eq!(account.dial_uri("T0398765432").to_string(), expected);
        // ATDP, brackets, dashes, and the trailing semicolon that means
        // "back to command state afterwards".
        assert_eq!(account.dial_uri("P(03) 9876-5432;").to_string(), expected);
        assert_eq!(account.dial_uri("03-9876-5432").to_string(), expected);
        assert_eq!(account.dial_uri("W 03 9876 5432,,").to_string(), expected);

        // A name is a name. The dial modifiers are letters, so stripping
        // them from anything and everything turned `support` into `suor` --
        // a wrong number placed with confidence, which is worse than a
        // refusal. They are modifiers only among digits.
        assert_eq!(
            account.dial_uri("support").to_string(),
            "sip:support@sip.example.net"
        );
        assert_eq!(
            account.dial_uri("press-room").to_string(),
            "sip:pressroom@sip.example.net"
        );

        // ATD sip:1000@pbx.local -- through untouched, which is the only way
        // to reach an extension that is not a number.
        assert_eq!(
            account.dial_uri("sip:1000@pbx.local").to_string(),
            "sip:1000@pbx.local"
        );
        assert_eq!(
            account
                .dial_uri("SIP:1000@pbx.local:5070;transport=udp")
                .to_string(),
            "SIP:1000@pbx.local:5070;transport=udp"
        );
        // A star code keeps its star, and a plus keeps its plus.
        assert_eq!(
            account.dial_uri("+61 3 9876 5432").to_string(),
            "sip:+61398765432@sip.example.net"
        );
        assert_eq!(
            account.dial_uri("*82").to_string(),
            "sip:*82@sip.example.net"
        );
    }

    /// The template has to be a file that loads, or it is not a template --
    /// and it has to bring no account with it.
    ///
    /// A shipped program that arrives already holding somebody's provider,
    /// number and half a password is wrong twice over: it is a credential
    /// nobody entered, and it is a preset that reads as a working account
    /// until the first call fails. Every section in the template is
    /// commented out, and this is the test that keeps it that way.
    #[test]
    fn the_example_template_presets_no_account() {
        let accounts = Account::parse(Account::EXAMPLE).expect("the template parses");
        assert!(
            accounts.is_empty(),
            "the template ships with an account in it: {accounts:?}"
        );
        // Nothing but comments and blank lines, which is the same claim made
        // a second way: no line in it is a setting or a section heading.
        for (number, line) in Account::EXAMPLE.lines().enumerate() {
            let line = line.trim();
            assert!(
                line.is_empty() || line.starts_with('#'),
                "line {} of the template is live: {line}",
                number + 1
            );
        }
        // And it still says the things a person needs to read.
        assert!(Account::EXAMPLE.contains("clear text"));
        assert!(Account::EXAMPLE.contains("transport"));
    }

    /// Written once, and never over the top of a file somebody has typed a
    /// password into.
    #[test]
    fn an_example_is_written_once_and_not_again() {
        let dir = std::env::temp_dir().join(format!("binmodem-sip-{}", crate::rand::token(12)));
        let path = dir.join("sip.txt");
        assert!(Account::write_example(&path).unwrap(), "the first time");
        assert!(!Account::write_example(&path).unwrap(), "the second time");
        let loaded = Account::load(&path).unwrap();
        assert!(loaded.is_empty(), "the template brought an account with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_there_is_named_in_the_error() {
        let error = Account::load(Path::new("no-such-directory/no-such-file.txt")).unwrap_err();
        assert!(error.contains("no-such-file.txt"), "{error}");
    }

    /// An empty file is a file with no accounts in it, not a fault.
    #[test]
    fn an_empty_file_is_no_accounts() {
        assert!(Account::parse("").unwrap().is_empty());
        assert!(
            Account::parse("# nothing but a comment\n\n")
                .unwrap()
                .is_empty()
        );
    }
}
