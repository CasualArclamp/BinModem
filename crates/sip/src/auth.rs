//! Digest authentication: RFC 3261 22, and RFC 7616 (which restates RFC 2617)
//! for the arithmetic itself.
//!
//! What this is for. A trunk will not take a REGISTER or an INVITE from
//! anybody who asks. It answers the first one with 401 (22.2) or 407 (22.3),
//! carrying a challenge, and expects the same request again with credentials
//! computed over the challenge, the method and the request URI. The two
//! flavours differ only in which headers carry them -- a registrar uses
//! WWW-Authenticate and Authorization, a proxy uses Proxy-Authenticate and
//! Proxy-Authorization -- and putting the answer in the wrong one of those is
//! a fault far ends do notice, so the distinction is carried in the
//! [`Challenge`] rather than left to the caller to remember.
//!
//! On MD5. It is broken as a hash and that is beside the point here. RFC 3261
//! 22.4 defines the credentials as MD5 over a particular set of strings; a
//! registrar that demands MD5 is answered in MD5 or it is not answered. There
//! is nothing to protect by refusing: the whole conversation, credentials
//! included, travels over plain UDP either way, and the password is never in
//! it -- only a hash of it with a nonce the server chose. Newer algorithms
//! (SHA-256 and SHA-256-sess, RFC 8760) exist and almost nothing deploys them;
//! [`Challenge::supported`] says no to them honestly instead of computing
//! something wrong and leaving the far end to explain why.
//!
//! The parsing is the part that actually goes wrong in the field. The
//! parameter list is comma-separated, and a nonce is a server's opaque blob
//! that is quite entitled to contain a comma. Splitting on commas without
//! regard to the quotes gives a truncated nonce, a response computed over the
//! wrong string, and the symptom everyone has met: registration works with one
//! provider and not with another, for no reason visible in the log. So the
//! splitter here tracks quotes, and there is a test for exactly that case.

use crate::md5;

/// A challenge out of a WWW-Authenticate (22.2) or Proxy-Authenticate (22.3)
/// header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// `Digest` in every case that matters. Kept as written rather than
    /// matched away, so that a far end offering something else can be named in
    /// a log instead of appearing as a silent failure to authenticate.
    pub scheme: String,
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    /// `MD5`, `MD5-sess`, or something newer we refuse. Absent means MD5 by
    /// RFC 7616 3.4, but absent and present are different on the wire: what
    /// was not offered is not echoed back.
    pub algorithm: Option<String>,
    /// The offered qop list, split and trimmed. Empty means the server did not
    /// offer one, which puts the exchange back into the RFC 2069 shape that a
    /// surprising number of registrars still use.
    pub qop: Vec<String>,
    /// The server saying the nonce has expired but the credentials were
    /// otherwise right: retry with the new nonce rather than telling the
    /// person their password is wrong (RFC 7616 3.3).
    pub stale: bool,
    /// Whether it came from a proxy, which decides which header the answer
    /// goes back in and is a distinction far ends do notice.
    pub from_proxy: bool,
}

impl Challenge {
    /// Parse one header value. `None` only when there is not even a scheme
    /// token to name; anything else parses, and whether it can be answered is
    /// [`supported`](Self::supported)'s question, not this one's.
    pub fn parse(header_value: &str, from_proxy: bool) -> Option<Self> {
        let text = header_value.trim();
        // The scheme is the first token; the parameters are whatever follows
        // the run of whitespace after it.
        let (scheme, rest) = match text.find(char::is_whitespace) {
            Some(at) => (&text[..at], text[at..].trim_start()),
            None => (text, ""),
        };
        if scheme.is_empty() {
            return None;
        }

        let mut challenge = Self {
            scheme: scheme.to_owned(),
            realm: String::new(),
            nonce: String::new(),
            opaque: None,
            algorithm: None,
            qop: Vec::new(),
            stale: false,
            from_proxy,
        };
        for (name, value) in split_parameters(rest) {
            match name.to_ascii_lowercase().as_str() {
                "realm" => challenge.realm = value,
                "nonce" => challenge.nonce = value,
                "opaque" => challenge.opaque = Some(value),
                "algorithm" => challenge.algorithm = Some(value),
                "qop" => {
                    // Quoted as one string, `qop="auth,auth-int"`, so the
                    // commas inside it are the list's and not the parameter
                    // list's. Unquoting happened above; splitting is here.
                    challenge.qop = value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                // Some servers quote it, some do not, and RFC 7616 3.3 says
                // it is an unquoted token. Both were unquoted on the way in.
                "stale" => challenge.stale = value.eq_ignore_ascii_case("true"),
                // Anything else -- domain, charset, userhash, a vendor's own
                // -- is not needed to compute a response and is ignored
                // rather than refused. A challenge is the server's to extend.
                _ => {}
            }
        }
        Some(challenge)
    }

    /// The header name the credentials go back in: Authorization or
    /// Proxy-Authorization.
    pub fn header_name(&self) -> &'static str {
        if self.from_proxy {
            "Proxy-Authorization"
        } else {
            "Authorization"
        }
    }

    /// Whether this is one we can actually answer.
    ///
    /// Digest, with a realm and a nonce to hash, in MD5 or MD5-sess. A qop
    /// list that offers only `auth-int` is refused too: auth-int hashes the
    /// body into HA2, this returns a header value and never sees a body, and
    /// a wrong answer is worse than a refusal that says which one it was.
    pub fn supported(&self) -> bool {
        if !self.scheme.eq_ignore_ascii_case("Digest") {
            return false;
        }
        if self.realm.is_empty() || self.nonce.is_empty() {
            return false;
        }
        let algorithm_ok = match &self.algorithm {
            None => true,
            Some(a) => a.eq_ignore_ascii_case("MD5") || a.eq_ignore_ascii_case("MD5-sess"),
        };
        if !algorithm_ok {
            return false;
        }
        self.qop.is_empty() || self.qop.iter().any(|q| q.eq_ignore_ascii_case("auth"))
    }

    /// Whether the session variant is being asked for, which changes HA1
    /// (RFC 7616 3.4.2).
    fn is_session(&self) -> bool {
        self.algorithm
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case("MD5-sess"))
    }

    /// `auth` if it was offered, else nothing -- which is the RFC 2069 form,
    /// with no nc and no cnonce in the response.
    fn chosen_qop(&self) -> Option<&str> {
        self.qop
            .iter()
            .find(|q| q.eq_ignore_ascii_case("auth"))
            .map(String::as_str)
    }
}

/// The credentials header value to send back.
///
/// The client nonce is chosen here. It is not a secret and does not need to
/// be: its only job is to stop the server choosing the entire input to the
/// hash (RFC 7616 5.10), for which unpredictable-enough is enough.
pub fn respond(
    challenge: &Challenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    nonce_count: u32,
) -> String {
    respond_with_cnonce(
        challenge,
        username,
        password,
        method,
        uri,
        nonce_count,
        &crate::rand::token(16),
    )
}

/// The same, with the client nonce fixed, so the worked examples in the RFCs
/// can be checked exactly.
pub fn respond_with_cnonce(
    challenge: &Challenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    nonce_count: u32,
    cnonce: &str,
) -> String {
    // RFC 7616 3.4.2. The unsalted A1 is the same in both variants; MD5-sess
    // then folds the two nonces into it once, so that a long-lived password
    // is not the direct input to every response in a session.
    let mut ha1 = md5::hex(format!("{username}:{}:{password}", challenge.realm).as_bytes());
    if challenge.is_session() {
        ha1 = md5::hex(format!("{ha1}:{}:{cnonce}", challenge.nonce).as_bytes());
    }
    // 3.4.3, for qop=auth or no qop. auth-int would hash the body in here,
    // and `supported` has already declined to get into that.
    let ha2 = md5::hex(format!("{method}:{uri}").as_bytes());

    // 3.4.1's nc: eight hex digits, lower case, because it is hashed as text
    // and a server computing the same number in the other case gets a
    // different answer.
    let nc = format!("{nonce_count:08x}");
    let qop = challenge.chosen_qop();
    let response = match qop {
        Some(qop) => {
            md5::hex(format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", challenge.nonce).as_bytes())
        }
        None => md5::hex(format!("{ha1}:{}:{ha2}", challenge.nonce).as_bytes()),
    };

    // Which of these are quoted is not a matter of taste. RFC 7616 3.4 makes
    // username, realm, nonce, uri, response, cnonce and opaque quoted strings
    // and algorithm, qop and nc bare tokens, and registrars are strict about
    // it in both directions: quoting qop or nc gets a 400 from some of them,
    // and leaving the quotes off a nonce gets a 401 forever from others. An
    // afternoon has been spent on that; it need not be spent again.
    let mut out = String::from("Digest ");
    out.push_str(&format!("username=\"{}\"", quoted(username)));
    out.push_str(&format!(", realm=\"{}\"", quoted(&challenge.realm)));
    out.push_str(&format!(", nonce=\"{}\"", quoted(&challenge.nonce)));
    out.push_str(&format!(", uri=\"{}\"", quoted(uri)));
    if let Some(qop) = qop {
        out.push_str(&format!(", qop={qop}"));
        out.push_str(&format!(", nc={nc}"));
        out.push_str(&format!(", cnonce=\"{}\"", quoted(cnonce)));
    }
    out.push_str(&format!(", response=\"{response}\""));
    // Echoed only when it was offered. A client that volunteers
    // `algorithm=MD5` to a server that never mentioned it is within the
    // letter of the RFC and outside what some registrars expect.
    if let Some(algorithm) = &challenge.algorithm {
        out.push_str(&format!(", algorithm={algorithm}"));
    }
    if let Some(opaque) = &challenge.opaque {
        // 3.4.6: returned unchanged, whatever it is. It is the server's
        // state, not ours to interpret.
        out.push_str(&format!(", opaque=\"{}\"", quoted(opaque)));
    }
    out
}

/// A value going into a quoted string. Backslash and quote are the only two
/// characters that can end it early (RFC 3261 25.1's quoted-pair), and a
/// password or a realm with one in is rare but not impossible.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Split a challenge's comma-separated parameter list, respecting quotes.
///
/// This is the whole reason this module does not split on commas: a nonce is
/// an opaque server string and several providers put commas in theirs.
fn split_parameters(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut start = 0;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                push_parameter(&mut out, &text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    push_parameter(&mut out, &text[start..]);
    out
}

fn push_parameter(out: &mut Vec<(String, String)>, item: &str) {
    let item = item.trim();
    if item.is_empty() {
        return;
    }
    // Only the first equals sign: a base64 value ends in them, and a nonce
    // very often is base64.
    match item.split_once('=') {
        Some((name, value)) => out.push((name.trim().to_owned(), unquote(value.trim()))),
        // A parameter with no value is not useful to us but is not a reason
        // to abandon the rest of the list.
        None => out.push((item.to_owned(), String::new())),
    }
}

/// Take the quotes off a value and undo the backslash escapes inside them.
/// An unquoted value comes back as it was.
fn unquote(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The challenge from RFC 2617 3.5, as a registrar would send it.
    const WORKED_EXAMPLE: &str = concat!(
        "Digest realm=\"testrealm@host.com\", ",
        "qop=\"auth,auth-int\", ",
        "nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", ",
        "opaque=\"5ccc069c403ebaf9f0171e9517f40e41\""
    );

    /// RFC 2617 3.5's worked example, which is the one number in this module
    /// that is not ours to choose. If this passes, the digest is the digest.
    #[test]
    fn the_rfc_2617_worked_example() {
        let challenge = Challenge::parse(WORKED_EXAMPLE, false).unwrap();
        assert!(challenge.supported());
        assert_eq!(challenge.realm, "testrealm@host.com");
        assert_eq!(challenge.qop, ["auth", "auth-int"]);
        assert_eq!(challenge.nonce, "dcd98b7102dd2f0e8b11d0f600bfb0c093");

        let header = respond_with_cnonce(
            &challenge,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            1,
            "0a4f113b",
        );
        assert!(
            header.contains("response=\"6629fae49393a05397450978507c4ef1\""),
            "{header}"
        );
        assert!(header.contains("nc=00000001"), "{header}");
        assert!(header.contains("cnonce=\"0a4f113b\""), "{header}");
        assert!(
            header.contains("opaque=\"5ccc069c403ebaf9f0171e9517f40e41\""),
            "{header}"
        );
    }

    /// qop and nc go back bare, and everything that is a string goes back in
    /// quotes. Registrars reject both of the opposite mistakes.
    #[test]
    fn qop_and_nc_are_unquoted_and_the_strings_are_quoted() {
        let challenge = Challenge::parse(WORKED_EXAMPLE, false).unwrap();
        let header = respond_with_cnonce(
            &challenge,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            1,
            "0a4f113b",
        );
        assert!(header.contains(", qop=auth,"), "{header}");
        assert!(!header.contains("qop=\""), "{header}");
        assert!(!header.contains("nc=\""), "{header}");
        assert!(header.contains("username=\"Mufasa\""), "{header}");
        assert!(header.contains("uri=\"/dir/index.html\""), "{header}");
        assert!(
            header.contains("nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\""),
            "{header}"
        );
        // Never offered, so never echoed.
        assert!(!header.contains("algorithm"), "{header}");
    }

    /// The RFC 2069 shape, which plenty of SIP registrars still send: no qop,
    /// so no nc and no cnonce in the answer and a two-part hash.
    #[test]
    fn a_challenge_without_qop_gets_the_rfc_2069_response() {
        let challenge =
            Challenge::parse("Digest realm=\"sip.example.net\", nonce=\"abc123\"", false).unwrap();
        assert!(challenge.qop.is_empty());
        assert!(challenge.supported());

        let header = respond_with_cnonce(
            &challenge,
            "1001",
            "secret",
            "REGISTER",
            "sip:sip.example.net",
            1,
            "ignored",
        );
        // Computed here the long way round, from the definition rather than
        // from the code under test.
        let ha1 = md5::hex(b"1001:sip.example.net:secret");
        let ha2 = md5::hex(b"REGISTER:sip:sip.example.net");
        let expected = md5::hex(format!("{ha1}:abc123:{ha2}").as_bytes());
        assert!(
            header.contains(&format!("response=\"{expected}\"")),
            "{header}"
        );
        assert!(!header.contains("cnonce"), "{header}");
        assert!(!header.contains("nc="), "{header}");
        assert!(!header.contains("qop"), "{header}");
    }

    /// MD5-sess folds both nonces into HA1 once, and says so in the header.
    #[test]
    fn the_session_variant_hashes_ha1_again() {
        let challenge = Challenge::parse(
            "Digest realm=\"sip.example.net\", nonce=\"n0nce\", qop=\"auth\", algorithm=MD5-sess",
            false,
        )
        .unwrap();
        assert!(challenge.supported());

        let header = respond_with_cnonce(
            &challenge,
            "1001",
            "secret",
            "REGISTER",
            "sip:sip.example.net",
            7,
            "0a4f113b",
        );
        let ha1_once = md5::hex(b"1001:sip.example.net:secret");
        let ha1 = md5::hex(format!("{ha1_once}:n0nce:0a4f113b").as_bytes());
        let ha2 = md5::hex(b"REGISTER:sip:sip.example.net");
        let expected = md5::hex(format!("{ha1}:n0nce:00000007:0a4f113b:auth:{ha2}").as_bytes());
        assert!(
            header.contains(&format!("response=\"{expected}\"")),
            "{header}"
        );
        assert!(header.contains("algorithm=MD5-sess"), "{header}");
        assert!(header.contains("nc=00000007"), "{header}");
        // It differs from the plain MD5 answer, which is the point of testing
        // it at all.
        let plain = Challenge {
            algorithm: Some("MD5".to_owned()),
            ..challenge.clone()
        };
        let other = respond_with_cnonce(
            &plain,
            "1001",
            "secret",
            "REGISTER",
            "sip:sip.example.net",
            7,
            "0a4f113b",
        );
        assert_ne!(header, other);
    }

    /// The fault that makes registration work with one provider and not
    /// another: a nonce with a comma in it, split on the comma.
    #[test]
    fn a_comma_inside_a_quoted_nonce_is_not_a_separator() {
        let challenge = Challenge::parse(
            "Digest realm=\"sip.example.net\", nonce=\"1700000000,abc==,9f2\", qop=\"auth\"",
            false,
        )
        .unwrap();
        assert_eq!(challenge.nonce, "1700000000,abc==,9f2");
        assert_eq!(challenge.realm, "sip.example.net");
        assert_eq!(challenge.qop, ["auth"]);
    }

    /// A real-looking line off a trunk: unquoted algorithm, a base64 nonce
    /// ending in equals signs, stale, and a folded continuation that the
    /// message parser has already joined up with a space.
    #[test]
    fn a_real_looking_sip_challenge_line() {
        let value = concat!(
            "Digest realm=\"sip.example.net\", ",
            "nonce=\"YmFzZTY0bm9uY2U9PQ==\", ",
            "algorithm=MD5, qop=\"auth\", stale=true, ",
            "opaque=\"\""
        );
        let challenge = Challenge::parse(value, true).unwrap();
        assert_eq!(challenge.realm, "sip.example.net");
        assert_eq!(challenge.nonce, "YmFzZTY0bm9uY2U9PQ==");
        assert_eq!(challenge.algorithm.as_deref(), Some("MD5"));
        assert!(challenge.stale);
        assert_eq!(challenge.opaque.as_deref(), Some(""));
        assert!(challenge.supported());
        // 22.3: a proxy's challenge is answered in the proxy's header.
        assert_eq!(challenge.header_name(), "Proxy-Authorization");

        let plain = Challenge::parse(value, false).unwrap();
        assert_eq!(plain.header_name(), "Authorization");
    }

    /// What we will not answer, and why each one is refused rather than
    /// guessed at.
    #[test]
    fn what_cannot_be_answered_says_so() {
        // RFC 8760's newer algorithms. Not implemented, so not claimed.
        let sha = Challenge::parse(
            "Digest realm=\"r\", nonce=\"n\", algorithm=SHA-256, qop=\"auth\"",
            false,
        )
        .unwrap();
        assert_eq!(sha.algorithm.as_deref(), Some("SHA-256"));
        assert!(!sha.supported());

        // Another scheme entirely.
        let basic = Challenge::parse("Basic realm=\"r\"", false).unwrap();
        assert_eq!(basic.scheme, "Basic");
        assert!(!basic.supported());

        // auth-int only: HA2 would need the body, which this never sees.
        let integrity =
            Challenge::parse("Digest realm=\"r\", nonce=\"n\", qop=\"auth-int\"", false).unwrap();
        assert!(!integrity.supported());

        // Nothing to hash against.
        let empty = Challenge::parse("Digest realm=\"r\"", false).unwrap();
        assert!(!empty.supported());
        assert!(Challenge::parse("   ", false).is_none());
    }

    /// The nonce count is eight lower-case hex digits, because it is hashed
    /// as text: a server that writes it the other way gets another answer.
    #[test]
    fn the_nonce_count_is_eight_lower_case_hex_digits() {
        let challenge = Challenge::parse(WORKED_EXAMPLE, false).unwrap();
        let header = respond_with_cnonce(&challenge, "u", "p", "REGISTER", "sip:h", 0x2ab, "c");
        assert!(header.contains("nc=000002ab"), "{header}");
    }

    /// A quote inside a value does not end the value early, in either
    /// direction.
    #[test]
    fn quotes_inside_values_survive_both_ways() {
        let challenge = Challenge::parse(
            "Digest realm=\"a \\\"quoted\\\" realm\", nonce=\"n\"",
            false,
        )
        .unwrap();
        assert_eq!(challenge.realm, "a \"quoted\" realm");
        let header = respond(&challenge, "us\"er", "p", "REGISTER", "sip:h", 1);
        assert!(header.contains("username=\"us\\\"er\""), "{header}");
        assert!(
            header.contains("realm=\"a \\\"quoted\\\" realm\""),
            "{header}"
        );
    }

    /// Two calls to `respond` differ, because the client nonce does. That is
    /// the only thing the client contributes to the hash.
    #[test]
    fn the_client_nonce_is_chosen_afresh() {
        let challenge = Challenge::parse(WORKED_EXAMPLE, false).unwrap();
        let one = respond(&challenge, "u", "p", "REGISTER", "sip:h", 1);
        let two = respond(&challenge, "u", "p", "REGISTER", "sip:h", 1);
        assert_ne!(one, two);
    }
}
