//! SIP URIs and the addresses that wrap them, RFC 3261 19.1 and 20.10.
//!
//! Two shapes, and the difference between them causes more interoperability
//! trouble than anything else in the protocol. A bare URI is what a request
//! line carries. An *address* -- 20.10's name-addr -- is a URI in angle
//! brackets with an optional display name in front and parameters after, and
//! it is what From, To and Contact carry. The brackets decide where the URI
//! stops: without them a semicolon starts a header parameter, with them it
//! starts a URI parameter, and a registrar reading `sip:me@host;transport=udp`
//! the wrong way round will register a different address than it was sent.
//!
//! So everything this crate builds uses the brackets, and everything it parses
//! copes with their absence.

use std::fmt;

/// A SIP URI, in the fields 19.1.1 names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Uri {
    /// `sip` or `sips`. Kept as written: comparison is case-insensitive but
    /// what goes back out should be what came in.
    pub scheme: String,
    /// The user part, which for a trunk is the telephone number being called.
    pub user: Option<String>,
    /// Host or address. No name resolution happens here.
    pub host: String,
    /// The port, when one was written. Absent means 5060, but absent and 5060
    /// are not the same string and a URI compared as text notices.
    pub port: Option<u16>,
    /// URI parameters: what follows a semicolon inside the brackets.
    pub parameters: Vec<(String, Option<String>)>,
}

impl Uri {
    /// A plain `sip:host` URI, which is what a registrar's address looks like.
    pub fn host(host: &str) -> Self {
        Self {
            scheme: "sip".to_owned(),
            host: host.to_owned(),
            ..Self::default()
        }
    }

    /// `sip:user@host`, the ordinary form.
    pub fn user_at(user: &str, host: &str) -> Self {
        Self {
            scheme: "sip".to_owned(),
            user: Some(user.to_owned()),
            host: host.to_owned(),
            ..Self::default()
        }
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn with_parameter(mut self, name: &str, value: Option<&str>) -> Self {
        self.parameters
            .push((name.to_owned(), value.map(str::to_owned)));
        self
    }

    /// A URI parameter's value, by a name compared without case.
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_deref())
    }

    pub fn has_parameter(&self, name: &str) -> bool {
        self.parameters
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// Host and port as something `std::net` will take, with 5060 filled in
    /// where the URI left it out (19.1.2).
    pub fn socket_address(&self) -> String {
        format!("{}:{}", self.host, self.port.unwrap_or(5060))
    }

    /// Parse one, being liberal about what is accepted: a far end that sends
    /// a malformed URI in a header we only echo back is not worth dropping a
    /// call over.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (scheme, rest) = text.split_once(':')?;
        if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
            return None;
        }
        // Parameters end the host part. The user part can contain neither a
        // semicolon nor an at sign, so finding the last at sign before the
        // first semicolon is unambiguous.
        let (body, params) = match rest.find(';') {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (rest, ""),
        };
        // A URI header list (?x=y) is legal and nothing here uses one; it is
        // dropped rather than carried, which is honest about what happens.
        let body = body.split('?').next().unwrap_or(body);
        let (user, hostport) = match body.rfind('@') {
            Some(at) => (Some(body[..at].to_owned()), &body[at + 1..]),
            None => (None, body),
        };
        // A password after the user is legal, never used on a trunk, and a
        // security hazard to carry about. Dropped.
        let user = user.map(|u| u.split(':').next().unwrap_or(&u).to_owned());
        let (host, port) = split_host_port(hostport);
        if host.is_empty() {
            return None;
        }
        Some(Self {
            scheme: scheme.to_owned(),
            user,
            host,
            port,
            parameters: parse_parameters(params),
        })
    }
}

impl fmt::Display for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.scheme)?;
        if let Some(user) = &self.user {
            write!(f, "{user}@")?;
        }
        write!(f, "{}", self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        for (name, value) in &self.parameters {
            match value {
                Some(v) => write!(f, ";{name}={v}")?,
                None => write!(f, ";{name}")?,
            }
        }
        Ok(())
    }
}

/// A URI with a display name in front and header parameters after: what From,
/// To and Contact carry (20.10, 20.20, 20.39).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Address {
    pub display: Option<String>,
    pub uri: Uri,
    /// Header parameters -- the tag, an expiry, a q value -- which belong to
    /// the header and not to the URI, whatever they look like.
    pub parameters: Vec<(String, Option<String>)>,
}

impl Address {
    pub fn new(uri: Uri) -> Self {
        Self {
            display: None,
            uri,
            parameters: Vec::new(),
        }
    }

    pub fn with_display(mut self, display: &str) -> Self {
        self.display = Some(display.to_owned());
        self
    }

    pub fn with_parameter(mut self, name: &str, value: &str) -> Self {
        self.parameters
            .push((name.to_owned(), Some(value.to_owned())));
        self
    }

    /// The dialog tag, which is what makes one leg of a call distinct from
    /// another between the same two addresses (19.3).
    pub fn tag(&self) -> Option<&str> {
        self.parameter("tag")
    }

    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.as_deref())
    }

    /// Replace a parameter, or add it. Used for the tag a dialog settles on.
    pub fn set_parameter(&mut self, name: &str, value: &str) {
        if let Some(slot) = self
            .parameters
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            slot.1 = Some(value.to_owned());
        } else {
            self.parameters
                .push((name.to_owned(), Some(value.to_owned())));
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        // The bracketed form first, because inside brackets a semicolon is
        // part of the URI and outside them it is not.
        if let Some(open) = text.find('<') {
            let close = text[open..].find('>')? + open;
            let display = text[..open].trim();
            let display = strip_quotes(display);
            let uri = Uri::parse(&text[open + 1..close])?;
            let params = text[close + 1..].trim_start();
            let params = params.strip_prefix(';').unwrap_or(params);
            return Some(Self {
                display: (!display.is_empty()).then(|| display.to_owned()),
                uri,
                parameters: parse_parameters(params),
            });
        }
        // Bare: everything up to the first semicolon is the URI, and what
        // follows are header parameters.
        let (uri_text, params) = match text.find(';') {
            Some(at) => (&text[..at], &text[at + 1..]),
            None => (text, ""),
        };
        Some(Self {
            display: None,
            uri: Uri::parse(uri_text)?,
            parameters: parse_parameters(params),
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(display) = &self.display {
            write!(f, "\"{display}\" ")?;
        }
        write!(f, "<{}>", self.uri)?;
        for (name, value) in &self.parameters {
            match value {
                Some(v) => write!(f, ";{name}={v}")?,
                None => write!(f, ";{name}")?,
            }
        }
        Ok(())
    }
}

/// Split a `host:port`, leaving a bracketed IPv6 literal alone.
pub fn split_host_port(text: &str) -> (String, Option<u16>) {
    if let Some(rest) = text.strip_prefix('[') {
        // [::1]:5060
        if let Some(close) = rest.find(']') {
            let host = format!("[{}]", &rest[..close]);
            let port = rest[close + 1..]
                .strip_prefix(':')
                .and_then(|p| p.parse().ok());
            return (host, port);
        }
    }
    match text.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => (host.to_owned(), Some(port)),
            // Not a number, so not a port: leave it in the host, where a
            // malformed URI is at least visible in a log.
            Err(_) => (text.to_owned(), None),
        },
        None => (text.to_owned(), None),
    }
}

/// A semicolon-separated parameter list, each possibly `name=value`.
pub fn parse_parameters(text: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for item in text.split(';') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        match item.split_once('=') {
            Some((name, value)) => out.push((
                name.trim().to_owned(),
                Some(strip_quotes(value.trim()).to_owned()),
            )),
            None => out.push((item.to_owned(), None)),
        }
    }
    out
}

fn strip_quotes(text: &str) -> &str {
    let trimmed = text.trim();
    match (trimmed.strip_prefix('"'), trimmed.strip_suffix('"')) {
        (Some(_), Some(_)) if trimmed.len() >= 2 => &trimmed[1..trimmed.len() - 1],
        _ => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trunk_uri() {
        let uri = Uri::parse("sip:0398765432@sip.example.net:5060").unwrap();
        assert_eq!(uri.user.as_deref(), Some("0398765432"));
        assert_eq!(uri.host, "sip.example.net");
        assert_eq!(uri.port, Some(5060));
        assert_eq!(uri.to_string(), "sip:0398765432@sip.example.net:5060");
    }

    #[test]
    fn a_uri_without_a_user() {
        let uri = Uri::parse("sip:sip.example.com").unwrap();
        assert!(uri.user.is_none());
        assert_eq!(uri.port, None);
        assert_eq!(uri.socket_address(), "sip.example.com:5060");
    }

    /// The distinction the brackets make. Both of these are one line of a real
    /// message and they mean different things.
    #[test]
    fn a_semicolon_belongs_to_whichever_side_of_the_bracket_it_is_on() {
        let bracketed = Address::parse("<sip:me@host;transport=udp>;tag=abc").unwrap();
        assert_eq!(bracketed.uri.parameter("transport"), Some("udp"));
        assert_eq!(bracketed.tag(), Some("abc"));

        let bare = Address::parse("sip:me@host;tag=abc").unwrap();
        assert!(bare.uri.parameter("tag").is_none());
        assert_eq!(bare.tag(), Some("abc"));
    }

    #[test]
    fn a_display_name_in_quotes() {
        let addr = Address::parse("\"BinModem\" <sip:1000@pbx.local>;tag=7f2").unwrap();
        assert_eq!(addr.display.as_deref(), Some("BinModem"));
        assert_eq!(addr.uri.user.as_deref(), Some("1000"));
        assert_eq!(addr.tag(), Some("7f2"));
        assert_eq!(
            addr.to_string(),
            "\"BinModem\" <sip:1000@pbx.local>;tag=7f2"
        );
    }

    #[test]
    fn a_password_is_dropped_rather_than_carried() {
        let uri = Uri::parse("sip:user:secret@host").unwrap();
        assert_eq!(uri.user.as_deref(), Some("user"));
        assert!(!uri.to_string().contains("secret"));
    }

    #[test]
    fn an_address_with_no_brackets_and_no_parameters() {
        let addr = Address::parse("sip:pbx.local").unwrap();
        assert_eq!(addr.uri.host, "pbx.local");
        assert!(addr.display.is_none());
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        assert!(Uri::parse("").is_none());
        assert!(Uri::parse("not a uri").is_none());
        assert!(Uri::parse("sip:").is_none());
    }
}
