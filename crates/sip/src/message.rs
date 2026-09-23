//! SIP messages: RFC 3261 7, and the header handling 7.3 asks for.
//!
//! A message is a start line, headers, a blank line and a body. What makes it
//! more work than that sounds is 7.3.1: header names are case-insensitive,
//! most have a one-letter compact form that means exactly the same thing, a
//! header may be folded across lines, and several headers may be combined onto
//! one line with commas or repeated on several lines with equal meaning. A far
//! end is free to choose differently from one message to the next, and several
//! of them do.
//!
//! So headers are kept as they arrived -- in order, with their original names
//! -- and looked up through a comparison that knows about the compact forms.
//! Nothing is canonicalised on the way in, because a Via or a Record-Route has
//! to be echoed back exactly as it was received or the far end will not
//! recognise its own routing.

use std::fmt;

use crate::uri::{Address, Uri};

/// The methods this agent knows. Anything else arrives as `Other` and is
/// answered with 405, which is what 8.2.1 asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Invite,
    Ack,
    Bye,
    Cancel,
    Register,
    Options,
    Info,
    Update,
    Other(String),
}

impl Method {
    pub fn parse(text: &str) -> Self {
        match text.to_ascii_uppercase().as_str() {
            "INVITE" => Self::Invite,
            "ACK" => Self::Ack,
            "BYE" => Self::Bye,
            "CANCEL" => Self::Cancel,
            "REGISTER" => Self::Register,
            "OPTIONS" => Self::Options,
            "INFO" => Self::Info,
            "UPDATE" => Self::Update,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Invite => "INVITE",
            Self::Ack => "ACK",
            Self::Bye => "BYE",
            Self::Cancel => "CANCEL",
            Self::Register => "REGISTER",
            Self::Options => "OPTIONS",
            Self::Info => "INFO",
            Self::Update => "UPDATE",
            Self::Other(s) => s,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 7.3.3's compact forms, paired with the names they stand for.
const COMPACT: &[(&str, &str)] = &[
    ("i", "Call-ID"),
    ("m", "Contact"),
    ("e", "Content-Encoding"),
    ("l", "Content-Length"),
    ("c", "Content-Type"),
    ("f", "From"),
    ("s", "Subject"),
    ("k", "Supported"),
    ("t", "To"),
    ("v", "Via"),
    ("o", "Event"),
    ("r", "Refer-To"),
    ("x", "Session-Expires"),
];

/// The full name a header name stands for, whichever form it was written in.
fn canonical(name: &str) -> &str {
    if name.len() == 1
        && let Some((_, full)) = COMPACT
            .iter()
            .find(|(short, _)| short.eq_ignore_ascii_case(name))
    {
        return full;
    }
    name
}

fn same_header(a: &str, b: &str) -> bool {
    canonical(a.trim()).eq_ignore_ascii_case(canonical(b.trim()))
}

/// A message's headers, in the order they were written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one at the end. Several headers of the same name are legal and in
    /// the case of Via are the whole point, so nothing is replaced here.
    pub fn push(&mut self, name: &str, value: impl Into<String>) {
        self.0.push((name.to_owned(), value.into()));
    }

    /// Add one at the front, which is where a Via goes on the way out (18.1.1)
    /// and where a Route goes.
    pub fn push_front(&mut self, name: &str, value: impl Into<String>) {
        self.0.insert(0, (name.to_owned(), value.into()));
    }

    /// Replace every header of this name with one, or add it.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.0.retain(|(n, _)| !same_header(n, name));
        self.push(name, value);
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(n, _)| !same_header(n, name));
    }

    /// The first value under this name, compact form included.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| same_header(n, name))
            .map(|(_, v)| v.as_str())
    }

    /// Every value under this name, in order.
    pub fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .filter(move |(n, _)| same_header(n, name))
            .map(|(_, v)| v.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    /// Drop the first header of this name, returning it. Used for popping the
    /// topmost Via off a response (18.1.2).
    pub fn take_first(&mut self, name: &str) -> Option<String> {
        let at = self.0.iter().position(|(n, _)| same_header(n, name))?;
        Some(self.0.remove(at).1)
    }

    // ---- the ones every message has ---------------------------------

    pub fn call_id(&self) -> Option<&str> {
        self.get("Call-ID")
    }

    pub fn from(&self) -> Option<Address> {
        self.get("From").and_then(Address::parse)
    }

    pub fn to(&self) -> Option<Address> {
        self.get("To").and_then(Address::parse)
    }

    pub fn contact(&self) -> Option<Address> {
        // A Contact of `*` is legal in a REGISTER that clears bindings and is
        // not an address; nothing here sends one, and one arriving is not an
        // address either.
        self.get("Contact")
            .filter(|c| c.trim() != "*")
            .and_then(Address::parse)
    }

    /// The sequence number and the method it belongs to (20.16).
    pub fn cseq(&self) -> Option<(u32, Method)> {
        let value = self.get("CSeq")?;
        let mut parts = value.split_whitespace();
        let number = parts.next()?.parse().ok()?;
        let method = Method::parse(parts.next()?);
        Some((number, method))
    }

    /// The branch parameter of the topmost Via, which is the transaction's
    /// identity (17.1.3, 17.2.3).
    pub fn branch(&self) -> Option<&str> {
        let via = self.get("Via")?;
        via_parameter(via, "branch")
    }

    pub fn content_length(&self) -> Option<usize> {
        self.get("Content-Length")?.trim().parse().ok()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.get("Content-Type")
    }
}

/// A parameter of a Via value, which has its own layout: a protocol, a host
/// and then semicolon parameters (20.42).
pub fn via_parameter<'a>(via: &'a str, name: &str) -> Option<&'a str> {
    // Only the first value if several were combined onto one line with
    // commas: the topmost Via is the first one written.
    let first = via.split(',').next().unwrap_or(via);
    for item in first.split(';').skip(1) {
        let item = item.trim();
        let (key, value) = match item.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (item, ""),
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
    }
    None
}

/// The `sent-by` of a Via: the host and port the response is to go back to,
/// before `received` and `rport` are taken into account (18.2.2).
pub fn via_sent_by(via: &str) -> Option<&str> {
    let first = via.split(',').next().unwrap_or(via);
    let body = first.split(';').next().unwrap_or(first);
    // SIP/2.0/UDP host:port
    body.split_whitespace().nth(1)
}

/// A request: 7.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub uri: Uri,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// A response: 7.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub code: u16,
    pub reason: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    /// 7.2's classes, which is how a user agent decides what to do next far
    /// more often than the exact code.
    pub fn is_provisional(&self) -> bool {
        (100..200).contains(&self.code)
    }
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.code)
    }
    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.code)
    }
    pub fn is_failure(&self) -> bool {
        self.code >= 400
    }
}

/// Either one, which is what arrives on a socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Request(Request),
    Response(Response),
}

impl Message {
    pub fn headers(&self) -> &Headers {
        match self {
            Self::Request(r) => &r.headers,
            Self::Response(r) => &r.headers,
        }
    }

    pub fn body(&self) -> &[u8] {
        match self {
            Self::Request(r) => &r.body,
            Self::Response(r) => &r.body,
        }
    }

    pub fn as_response(&self) -> Option<&Response> {
        match self {
            Self::Response(r) => Some(r),
            Self::Request(_) => None,
        }
    }

    pub fn as_request(&self) -> Option<&Request> {
        match self {
            Self::Request(r) => Some(r),
            Self::Response(_) => None,
        }
    }

    /// Parse one datagram's worth.
    ///
    /// Over UDP a message is a datagram and the framing question 7.5 raises
    /// does not arise, so Content-Length is trusted only as far as the
    /// datagram goes: a far end that understates it has still sent the body,
    /// and one that overstates it has not.
    pub fn parse(datagram: &[u8]) -> Option<Self> {
        let split = find_header_end(datagram)?;
        let head = std::str::from_utf8(&datagram[..split.headers_end]).ok()?;
        let mut lines = unfold(head);
        let start = lines.next()?;
        let mut headers = Headers::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            // A line with no colon in it is not a header. Skipped rather than
            // refused: the start line has already been recognised, so this is
            // a SIP message with one bad line in it, and throwing the whole
            // datagram away would turn a stray line from some middlebox into
            // a call that never connects and never says why.
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            headers.push(name.trim(), value.trim().to_owned());
        }

        let available = &datagram[split.body_start..];
        let body = match headers.content_length() {
            Some(n) if n <= available.len() => available[..n].to_vec(),
            _ => available.to_vec(),
        };

        // 7.1 and 7.2: a status line begins with the version, a request line
        // ends with it.
        if let Some(rest) = start.strip_prefix("SIP/2.0") {
            let rest = rest.trim_start();
            let (code, reason) = match rest.split_once(' ') {
                Some((c, r)) => (c, r.trim().to_owned()),
                None => (rest, String::new()),
            };
            Some(Self::Response(Response {
                code: code.trim().parse().ok()?,
                reason,
                headers,
                body,
            }))
        } else {
            let mut parts = start.split_whitespace();
            let method = Method::parse(parts.next()?);
            let uri = Uri::parse(parts.next()?)?;
            let version = parts.next()?;
            if !version.eq_ignore_ascii_case("SIP/2.0") {
                return None;
            }
            Some(Self::Request(Request {
                method,
                uri,
                headers,
                body,
            }))
        }
    }

    /// Back onto the wire. Content-Length is written from the body rather than
    /// from whatever a caller set, because a mismatch there is the one error
    /// that makes a message unparseable at the far end.
    pub fn to_bytes(&self) -> Vec<u8> {
        let (start, headers, body) = match self {
            Self::Request(r) => (
                format!("{} {} SIP/2.0", r.method, r.uri),
                &r.headers,
                &r.body,
            ),
            Self::Response(r) => (
                format!("SIP/2.0 {} {}", r.code, r.reason),
                &r.headers,
                &r.body,
            ),
        };
        let mut out = Vec::with_capacity(512 + body.len());
        out.extend_from_slice(start.as_bytes());
        out.extend_from_slice(b"\r\n");
        for (name, value) in headers.iter() {
            if same_header(name, "Content-Length") {
                continue;
            }
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(body);
        out
    }
}

struct Split {
    headers_end: usize,
    body_start: usize,
}

/// Where the headers stop. CRLF CRLF by the letter of 7, and LF LF as well,
/// because a far end that writes bare line feeds is not worth refusing.
fn find_header_end(data: &[u8]) -> Option<Split> {
    for at in 0..data.len() {
        if data[at..].starts_with(b"\r\n\r\n") {
            return Some(Split {
                headers_end: at,
                body_start: at + 4,
            });
        }
        if data[at..].starts_with(b"\n\n") {
            return Some(Split {
                headers_end: at,
                body_start: at + 2,
            });
        }
    }
    // A message with no body need not have a blank line after its headers in
    // practice, though it should.
    Some(Split {
        headers_end: data.len(),
        body_start: data.len(),
    })
}

/// 7.3.1's line folding: a header may be continued on the next line if that
/// line starts with whitespace, and the fold means a single space.
fn unfold(head: &str) -> std::vec::IntoIter<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in head.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        // A continuation with nothing in front of it is not a continuation.
        // It cannot be folded onto anything, so it stands as its own line and
        // is refused later as the header it is not.
        if line.starts_with([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
            continue;
        }
        lines.push(line.to_owned());
    }
    lines.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = concat!(
        "INVITE sip:0398765432@sip.example.net SIP/2.0\r\n",
        "Via: SIP/2.0/UDP 192.0.2.4:5060;branch=z9hG4bK776asdhds;rport\r\n",
        "Max-Forwards: 70\r\n",
        "To: <sip:0398765432@sip.example.net>\r\n",
        "From: \"BinModem\" <sip:1001@sip.example.net>;tag=1928301774\r\n",
        "Call-ID: a84b4c76e66710\r\n",
        "CSeq: 314159 INVITE\r\n",
        "Contact: <sip:1001@192.0.2.4:5060>\r\n",
        "Content-Type: application/sdp\r\n",
        "Content-Length: 4\r\n",
        "\r\n",
        "v=0\n",
    );

    #[test]
    fn a_request_round_trips() {
        let message = Message::parse(INVITE.as_bytes()).unwrap();
        let request = message.as_request().unwrap();
        assert_eq!(request.method, Method::Invite);
        assert_eq!(request.uri.user.as_deref(), Some("0398765432"));
        assert_eq!(request.headers.call_id(), Some("a84b4c76e66710"));
        assert_eq!(
            request.headers.cseq(),
            Some((314159, Method::Invite))
        );
        assert_eq!(request.headers.branch(), Some("z9hG4bK776asdhds"));
        assert_eq!(request.body, b"v=0\n");

        let bytes = message.to_bytes();
        let again = Message::parse(&bytes).unwrap();
        assert_eq!(again, message);
    }

    #[test]
    fn a_response_parses() {
        let text = concat!(
            "SIP/2.0 401 Unauthorized\r\n",
            "Via: SIP/2.0/UDP 192.0.2.4:5060;branch=z9hG4bK1;received=203.0.113.9\r\n",
            "To: <sip:1001@sip.example.net>;tag=a6c85cf\r\n",
            "From: <sip:1001@sip.example.net>;tag=1928301774\r\n",
            "Call-ID: a84b4c76e66710\r\n",
            "CSeq: 1 REGISTER\r\n",
            "WWW-Authenticate: Digest realm=\"sip.example.net\",\r\n",
            " nonce=\"ea9c8e88df84f1cec4341ae6cbe5a359\"\r\n",
            "Content-Length: 0\r\n\r\n",
        );
        let message = Message::parse(text.as_bytes()).unwrap();
        let response = message.as_response().unwrap();
        assert_eq!(response.code, 401);
        assert_eq!(response.reason, "Unauthorized");
        assert_eq!(response.headers.to().unwrap().tag(), Some("a6c85cf"));
        // The folded line came back as one header.
        let challenge = response.headers.get("WWW-Authenticate").unwrap();
        assert!(challenge.contains("nonce=\"ea9c8e88df84f1cec4341ae6cbe5a359\""));
        assert!(challenge.contains("realm=\"sip.example.net\""));
    }

    #[test]
    fn compact_names_are_the_same_names() {
        let text = concat!(
            "SIP/2.0 200 OK\r\n",
            "v: SIP/2.0/UDP 192.0.2.4;branch=z9hG4bK2\r\n",
            "f: <sip:a@b>;tag=1\r\n",
            "t: <sip:c@d>;tag=2\r\n",
            "i: call-1\r\n",
            "m: <sip:c@203.0.113.9>\r\n",
            "l: 0\r\n",
            "CSeq: 2 INVITE\r\n\r\n",
        );
        let message = Message::parse(text.as_bytes()).unwrap();
        let headers = message.headers();
        assert_eq!(headers.call_id(), Some("call-1"));
        assert_eq!(headers.from().unwrap().tag(), Some("1"));
        assert_eq!(headers.to().unwrap().tag(), Some("2"));
        assert_eq!(headers.contact().unwrap().uri.host, "203.0.113.9");
        assert_eq!(headers.content_length(), Some(0));
        assert_eq!(headers.branch(), Some("z9hG4bK2"));
    }

    #[test]
    fn several_vias_stay_in_order() {
        let text = concat!(
            "SIP/2.0 200 OK\r\n",
            "Via: SIP/2.0/UDP proxy.example.net;branch=z9hG4bKouter\r\n",
            "Via: SIP/2.0/UDP 192.0.2.4;branch=z9hG4bKinner\r\n",
            "CSeq: 1 INVITE\r\n\r\n",
        );
        let mut message = Message::parse(text.as_bytes()).unwrap();
        assert_eq!(message.headers().branch(), Some("z9hG4bKouter"));
        let Message::Response(r) = &mut message else {
            unreachable!()
        };
        // 18.1.2: the response's own Via comes off before it is matched.
        r.headers.take_first("Via");
        assert_eq!(r.headers.branch(), Some("z9hG4bKinner"));
    }

    #[test]
    fn the_length_written_is_the_body_there_is() {
        let request = Request {
            method: Method::Bye,
            uri: Uri::user_at("x", "y"),
            headers: {
                let mut h = Headers::new();
                // Deliberately wrong, as a caller that edited the body would
                // leave it.
                h.push("Content-Length", "99");
                h
            },
            body: b"12345".to_vec(),
        };
        let bytes = Message::Request(request).to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Content-Length: 5\r\n"));
        assert!(!text.contains("99"));
    }

    /// One bad line does not cost the message. A proxy that inserts
    /// something odd, or a line that arrives damaged, should not be the
    /// difference between a call and silence.
    #[test]
    fn a_line_that_is_not_a_header_is_stepped_over() {
        let text = concat!(
            "SIP/2.0 200 OK\r\n",
            "Via: SIP/2.0/UDP 192.0.2.4;branch=z9hG4bK9\r\n",
            "this line has no colon in it\r\n",
            "CSeq: 4 BYE\r\n",
            "Call-ID: still-here\r\n\r\n",
        );
        let message = Message::parse(text.as_bytes()).expect("the message was thrown away");
        assert_eq!(message.headers().call_id(), Some("still-here"));
        assert_eq!(message.headers().cseq(), Some((4, Method::Bye)));
    }

    #[test]
    fn rport_and_received_are_read_off_the_via() {
        let via = "SIP/2.0/UDP 192.168.1.9:5060;rport=41234;received=203.0.113.9;branch=z9hG4bK3";
        assert_eq!(via_parameter(via, "rport"), Some("41234"));
        assert_eq!(via_parameter(via, "received"), Some("203.0.113.9"));
        assert_eq!(via_sent_by(via), Some("192.168.1.9:5060"));
    }
}
