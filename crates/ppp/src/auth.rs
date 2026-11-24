//! Saying who is calling, and checking (RFC 1661 3.5, RFC 1334, RFC 1994).
//!
//! A far end asks for this during LCP, with 6.2's Authentication-Protocol
//! option, and once LCP is up the link sits in 3.5's Authentication phase
//! until it is settled: "Advancement from the Authentication phase to the
//! Network-Layer Protocol phase MUST NOT occur until authentication has
//! completed." So a provider that wants a name and password gets nothing else
//! until it has them, which is why a link without this went as far as LCP and
//! no further.
//!
//! Two protocols, and each has two ends. RFC 1334's PAP is the simple one: the
//! end being checked sends its name and password, again and again until it is
//! answered, and the other end says yes or no. RFC 1994's CHAP turns it round:
//! the checking end sends a challenge, and the answer is an MD5 hash of the
//! challenge and the password together, so the password itself never crosses.
//!
//! Which end checks and which is checked is not fixed. 1994 says so outright --
//! "there is no requirement that authentication be full duplex or that the
//! same protocol be used in both directions" -- so a link can have a
//! [`Prover`], an [`Authenticator`], both, or neither.

use crate::control::{Code, Message};
use crate::lcp::Auth;

/// RFC 1334 2.2: the PAP codes.
pub mod pap {
    pub const AUTHENTICATE_REQUEST: u8 = 1;
    pub const AUTHENTICATE_ACK: u8 = 2;
    pub const AUTHENTICATE_NAK: u8 = 3;
}

/// RFC 1994 4: the CHAP codes.
pub mod chap {
    pub const CHALLENGE: u8 = 1;
    pub const RESPONSE: u8 = 2;
    pub const SUCCESS: u8 = 3;
    pub const FAILURE: u8 = 4;
}

/// How long to wait between tries. The same three seconds as LCP's Restart
/// timer, for the same reason: a round trip over a VoIP trunk is well over a
/// second, and asking again before an answer could have arrived is noise.
pub const RESTART_MS: u32 = 3_000;

/// How many times to try. RFC 1334 2.2.1 leaves it to "an optional retry
/// counter", and 1994 4.1 the same; ten is what LCP's 4.6 suggests for its own.
pub const TRIES: u32 = 10;

/// A name and the password that goes with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub password: String,
}

impl Account {
    pub fn new(name: &str, password: &str) -> Self {
        Self { name: name.to_owned(), password: password.to_owned() }
    }
}

/// Where one direction of authentication has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Waiting,
    Passed,
    /// And why not, in words for a person.
    Failed(String),
}

/// The protocol number an [`Auth`] travels under.
pub fn protocol(method: Auth) -> u16 {
    match method {
        Auth::Pap => crate::protocol::PAP,
        Auth::ChapMd5 => crate::protocol::CHAP,
    }
}

/// The methods an end checking callers will ask for, best first.
///
/// RFC 1334 2: "Any implementations which include a stronger authentication
/// method (such as CHAP, described below) MUST offer to negotiate that method
/// prior to PAP." So CHAP first -- unless an account has no password, because
/// RFC 1994 2.3 requires "the length of the secret MUST be at least 1 octet",
/// and an account CHAP cannot be used with leaves PAP as the only honest
/// offer.
pub fn methods_for(accounts: &[Account]) -> Vec<Auth> {
    if accounts.iter().any(|a| a.password.is_empty()) {
        vec![Auth::Pap]
    } else {
        vec![Auth::ChapMd5, Auth::Pap]
    }
}

/// One packet of either protocol: 1334 2.2 and 1994 4 give them the same
/// header as LCP's, Code, Identifier and Length.
fn packet(code: u8, id: u8, data: Vec<u8>) -> Vec<u8> {
    Message { code: Code::from_u8(code), id, data }.to_bytes()
}

/// A length-prefixed field, as PAP's Peer-ID, Password and Message and CHAP's
/// Value are. One octet of length, so at most 255 of content.
fn counted(field: &[u8], out: &mut Vec<u8>) {
    let field = &field[..field.len().min(255)];
    out.push(field.len() as u8);
    out.extend_from_slice(field);
}

/// Read one of those back: the field, and what follows it.
fn read_counted(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&length, rest) = data.split_first()?;
    let length = usize::from(length);
    (rest.len() >= length).then(|| rest.split_at(length))
}

/// 1994 4.1: "the one-way hash calculated over a stream of octets consisting
/// of the Identifier, followed by (concatenated with) the 'secret', followed
/// by (concatenated with) the Challenge Value."
pub fn chap_response(id: u8, secret: &[u8], challenge: &[u8]) -> [u8; 16] {
    let mut stream = Vec::with_capacity(1 + secret.len() + challenge.len());
    stream.push(id);
    stream.extend_from_slice(secret);
    stream.extend_from_slice(challenge);
    crate::md5::digest(&stream)
}

/// Two byte strings compared without stopping at the first difference, so how
/// long the comparison took says nothing about how much of a guess was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A human-readable message field, as both protocols' replies carry: 1334 and
/// 1994 both say it "MUST NOT affect operation of the protocol", so it is
/// only ever shown.
fn readable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
        .collect()
}

/// This end, proving who it is to a far end that asked.
#[derive(Debug)]
pub struct Prover {
    method: Auth,
    account: Account,
    /// PAP: the Identifier of the latest request, and of the first, so that a
    /// reply to an earlier try -- delayed rather than lost, on a line with a
    /// second of round trip -- still counts.
    first_id: u8,
    id: u8,
    tries: u32,
    timer: u32,
    /// CHAP: the challenges answered, by Identifier, so a Success can be
    /// matched to one of them.
    answered: Vec<u8>,
    outcome: Outcome,
    out: Vec<Vec<u8>>,
}

impl Prover {
    pub fn new(method: Auth, account: Account) -> Self {
        let mut prover = Self {
            method,
            account,
            first_id: 1,
            id: 0,
            tries: 0,
            timer: 0,
            answered: Vec::new(),
            outcome: Outcome::Waiting,
            out: Vec::new(),
        };
        // 1334 2.2.1: "The link peer MUST transmit a PAP packet with the Code
        // field set to 1 (Authenticate-Request) during the Authentication
        // phase." CHAP's peer waits to be challenged instead.
        if method == Auth::Pap {
            prover.request();
        }
        prover
    }

    pub fn method(&self) -> Auth {
        self.method
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub fn take_output(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.out)
    }

    fn request(&mut self) {
        // "The Identifier field MUST be changed each time an
        // Authenticate-Request packet is issued."
        self.id = self.id.wrapping_add(1);
        self.tries += 1;
        self.timer = RESTART_MS;
        let mut data = Vec::new();
        counted(self.account.name.as_bytes(), &mut data);
        counted(self.account.password.as_bytes(), &mut data);
        self.out.push(packet(pap::AUTHENTICATE_REQUEST, self.id, data));
    }

    pub fn tick(&mut self, ms: u32) {
        if self.outcome != Outcome::Waiting {
            return;
        }
        if self.timer > ms {
            self.timer -= ms;
            return;
        }
        match self.method {
            // 1334 2.2.1: "MUST be repeated until a valid reply packet is
            // received, or an optional retry counter expires."
            Auth::Pap if self.tries < TRIES => self.request(),
            Auth::Pap => {
                self.outcome = Outcome::Failed(format!(
                    "the far end asked for a password and never answered {TRIES} of them"
                ));
            }
            // CHAP's peer has nothing to repeat; the challenger does. Waiting
            // as long as a challenger would keep trying is waiting long
            // enough.
            Auth::ChapMd5 => {
                self.tries += 1;
                if self.tries >= TRIES {
                    self.outcome = Outcome::Failed(
                        "the far end asked for CHAP and never sent a challenge".to_owned(),
                    );
                } else {
                    self.timer = RESTART_MS;
                }
            }
        }
    }

    /// One packet of this method's protocol.
    pub fn receive(&mut self, bytes: &[u8]) {
        let Some(message) = Message::parse(bytes) else { return };
        let code = message.code.to_u8();
        match self.method {
            Auth::Pap => {
                let ours = message.id.wrapping_sub(self.first_id) < self.id.wrapping_sub(self.first_id).wrapping_add(1);
                if !ours || self.outcome != Outcome::Waiting {
                    return;
                }
                let text = read_counted(&message.data).map(|(m, _)| readable(m)).unwrap_or_default();
                match code {
                    pap::AUTHENTICATE_ACK => self.outcome = Outcome::Passed,
                    pap::AUTHENTICATE_NAK => {
                        self.outcome = Outcome::Failed(if text.is_empty() {
                            "the far end refused the name and password".to_owned()
                        } else {
                            format!("the far end refused the name and password: {text}")
                        });
                    }
                    _ => {}
                }
            }
            Auth::ChapMd5 => match code {
                chap::CHALLENGE => {
                    // 1994 4.1: "Whenever a Challenge packet is received, the
                    // peer MUST transmit a CHAP packet with the Code field set
                    // to 2 (Response)" -- including after success, since a
                    // challenger may check again at any time.
                    let Some((value, _name)) = read_counted(&message.data) else { return };
                    if value.is_empty() {
                        return;
                    }
                    let hash = chap_response(message.id, self.account.password.as_bytes(), value);
                    let mut data = Vec::new();
                    counted(&hash, &mut data);
                    data.extend_from_slice(self.account.name.as_bytes());
                    self.out.push(packet(chap::RESPONSE, message.id, data));
                    if !self.answered.contains(&message.id) {
                        self.answered.push(message.id);
                    }
                    // Waiting for the verdict now, which is a round trip.
                    self.timer = RESTART_MS;
                }
                chap::SUCCESS if self.answered.contains(&message.id) => {
                    if self.outcome == Outcome::Waiting {
                        self.outcome = Outcome::Passed;
                    }
                }
                chap::FAILURE if self.answered.contains(&message.id) => {
                    let text = readable(&message.data);
                    self.outcome = Outcome::Failed(if text.is_empty() {
                        "the far end did not accept the answer to its challenge".to_owned()
                    } else {
                        format!("the far end did not accept the answer to its challenge: {text}")
                    });
                }
                _ => {}
            },
        }
    }
}

/// This end, checking a far end it asked to say who it is.
#[derive(Debug)]
pub struct Authenticator {
    method: Auth,
    accounts: Vec<Account>,
    /// What this end calls itself in a challenge's Name field.
    name: String,
    /// Where the challenge values come from.
    seed: u64,
    id: u8,
    tries: u32,
    timer: u32,
    /// CHAP: every challenge sent, by Identifier, so an answer to an earlier
    /// one is checked against the value it was an answer to.
    challenges: Vec<(u8, [u8; 16])>,
    /// The reply given, once there is one: 1334 and 1994 both require any
    /// repeat to be given the same answer.
    verdict: Option<bool>,
    who: Option<String>,
    outcome: Outcome,
    out: Vec<Vec<u8>>,
}

impl Authenticator {
    /// `seed` makes the challenges; it has to differ from call to call, and
    /// the caller is the one with a clock.
    pub fn new(method: Auth, accounts: Vec<Account>, name: &str, seed: u64) -> Self {
        let mut authenticator = Self {
            method,
            accounts,
            name: name.to_owned(),
            seed,
            id: 0,
            tries: 0,
            timer: RESTART_MS,
            challenges: Vec::new(),
            verdict: None,
            who: None,
            outcome: Outcome::Waiting,
            out: Vec::new(),
        };
        // 1994 4.1: "The authenticator MUST transmit a CHAP packet with the
        // Code field set to 1 (Challenge)." PAP's authenticator "SHOULD expect
        // the peer to send an Authenticate-Request packet" and says nothing
        // until it does.
        if method == Auth::ChapMd5 {
            authenticator.challenge();
        }
        authenticator
    }

    pub fn method(&self) -> Auth {
        self.method
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    /// The name the far end proved, once it has.
    pub fn who(&self) -> Option<&str> {
        self.who.as_deref()
    }

    pub fn take_output(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.out)
    }

    fn challenge(&mut self) {
        // "The Identifier field MUST be changed each time a Challenge is
        // sent", and "the Challenge Value MUST be changed each time a
        // Challenge is sent" -- 2.3 adds that it should be unpredictable, which
        // is what hashing a per-call seed with the count gives.
        self.id = self.id.wrapping_add(1);
        self.tries += 1;
        self.timer = RESTART_MS;
        let mut source = self.seed.to_le_bytes().to_vec();
        source.push(self.id);
        source.extend_from_slice(&self.tries.to_le_bytes());
        let value = crate::md5::digest(&source);
        self.challenges.push((self.id, value));
        let mut data = Vec::new();
        counted(&value, &mut data);
        data.extend_from_slice(self.name.as_bytes());
        self.out.push(packet(chap::CHALLENGE, self.id, data));
    }

    pub fn tick(&mut self, ms: u32) {
        if self.outcome != Outcome::Waiting {
            return;
        }
        if self.timer > ms {
            self.timer -= ms;
            return;
        }
        self.timer = RESTART_MS;
        match self.method {
            // 1994 4.1: "Additional Challenge packets MUST be sent until a
            // valid Response packet is received, or an optional retry counter
            // expires."
            Auth::ChapMd5 if self.tries < TRIES => self.challenge(),
            Auth::ChapMd5 => {
                self.outcome = Outcome::Failed(format!(
                    "the caller answered none of {TRIES} challenges"
                ));
            }
            Auth::Pap => {
                self.tries += 1;
                if self.tries >= TRIES {
                    self.outcome =
                        Outcome::Failed("the caller never sent a name and password".to_owned());
                }
            }
        }
    }

    /// One packet of this method's protocol.
    pub fn receive(&mut self, bytes: &[u8]) {
        let Some(message) = Message::parse(bytes) else { return };
        let code = message.code.to_u8();
        match (self.method, code) {
            (Auth::Pap, pap::AUTHENTICATE_REQUEST) => {
                let Some((name, rest)) = read_counted(&message.data) else { return };
                let Some((password, _)) = read_counted(rest) else { return };
                let verdict = match self.verdict {
                    Some(v) => v,
                    None => {
                        let name = String::from_utf8_lossy(name).into_owned();
                        let ok = self
                            .accounts
                            .iter()
                            .any(|a| same(a.name.as_bytes(), name.as_bytes()) && same(a.password.as_bytes(), password));
                        self.settle(ok, name);
                        ok
                    }
                };
                // 2.2.2: the Identifier "MUST be copied from the Identifier
                // field of the Authenticate-Request which caused this reply."
                let (code, text): (u8, &[u8]) = if verdict {
                    (pap::AUTHENTICATE_ACK, b"Welcome")
                } else {
                    (pap::AUTHENTICATE_NAK, b"Login incorrect")
                };
                let mut data = Vec::new();
                counted(text, &mut data);
                self.out.push(packet(code, message.id, data));
            }
            (Auth::ChapMd5, chap::RESPONSE) => {
                // An answer to a challenge this end never sent is not one:
                // 4.1 has any other Response "silently discarded".
                let Some(&(_, value)) = self.challenges.iter().find(|(id, _)| *id == message.id) else {
                    return;
                };
                let Some((answer, name)) = read_counted(&message.data) else { return };
                let verdict = match self.verdict {
                    Some(v) => v,
                    None => {
                        let name = String::from_utf8_lossy(name).into_owned();
                        let ok = self.accounts.iter().any(|a| {
                            same(a.name.as_bytes(), name.as_bytes())
                                && same(&chap_response(message.id, a.password.as_bytes(), &value), answer)
                        });
                        self.settle(ok, name);
                        ok
                    }
                };
                // 4.2: the Identifier "MUST be copied from the Identifier
                // field of the Response which caused this reply."
                let (code, text): (u8, &[u8]) = if verdict {
                    (chap::SUCCESS, b"Welcome")
                } else {
                    (chap::FAILURE, b"Authentication failed")
                };
                self.out.push(packet(code, message.id, text.to_vec()));
            }
            _ => {}
        }
    }

    fn settle(&mut self, ok: bool, name: String) {
        self.verdict = Some(ok);
        if ok {
            self.who = Some(name);
            self.outcome = Outcome::Passed;
        } else {
            // 1334 2.2.2 and 1994 4.2 both follow a refusal with "SHOULD take
            // action to terminate the link", which is the link's to do.
            self.outcome = Outcome::Failed(format!("the caller gave a name ({}) and password that are not right", readable(name.as_bytes())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand packets from one end to the other until neither has anything to
    /// say, ticking a millisecond a round.
    fn run(prover: &mut Prover, authenticator: &mut Authenticator, ms: u32) {
        for _ in 0..ms {
            for p in prover.take_output() {
                authenticator.receive(&p);
            }
            for p in authenticator.take_output() {
                prover.receive(&p);
            }
            prover.tick(1);
            authenticator.tick(1);
            if prover.outcome() != &Outcome::Waiting && authenticator.outcome() != &Outcome::Waiting {
                // One more round, for the replies still in flight.
                for p in authenticator.take_output() {
                    prover.receive(&p);
                }
                return;
            }
        }
    }

    fn accounts() -> Vec<Account> {
        vec![Account::new("rory", "hunter2"), Account::new("guest", "letmein")]
    }

    #[test]
    fn pap_lets_the_right_password_in() {
        let mut prover = Prover::new(Auth::Pap, Account::new("guest", "letmein"));
        let mut checker = Authenticator::new(Auth::Pap, accounts(), "binmodem", 1);
        run(&mut prover, &mut checker, 1000);
        assert_eq!(prover.outcome(), &Outcome::Passed);
        assert_eq!(checker.outcome(), &Outcome::Passed);
        assert_eq!(checker.who(), Some("guest"));
    }

    #[test]
    fn pap_keeps_the_wrong_one_out_and_says_so() {
        let mut prover = Prover::new(Auth::Pap, Account::new("guest", "hunter2"));
        let mut checker = Authenticator::new(Auth::Pap, accounts(), "binmodem", 1);
        run(&mut prover, &mut checker, 1000);
        assert!(matches!(prover.outcome(), Outcome::Failed(why) if why.contains("Login incorrect")));
        assert!(matches!(checker.outcome(), Outcome::Failed(_)));
        assert_eq!(checker.who(), None);
    }

    /// 1334 2.2.1: the exact bytes of a request, laid out as the figure draws
    /// them.
    #[test]
    fn a_pap_request_is_laid_out_as_the_figure_draws_it() {
        let mut prover = Prover::new(Auth::Pap, Account::new("ab", "xyz"));
        let out = prover.take_output();
        assert_eq!(out, vec![vec![1, 1, 0, 11, 2, b'a', b'b', 3, b'x', b'y', b'z']]);
    }

    /// 1334 2.2.1: repeated, with a new Identifier each time, until answered.
    #[test]
    fn pap_asks_again_and_then_gives_up() {
        let mut prover = Prover::new(Auth::Pap, Account::new("a", "b"));
        let mut ids = vec![];
        for _ in 0..(RESTART_MS * (TRIES + 2)) {
            for p in prover.take_output() {
                ids.push(p[1]);
            }
            prover.tick(1);
        }
        assert_eq!(ids.len(), TRIES as usize);
        ids.dedup();
        assert_eq!(ids.len(), TRIES as usize, "an Identifier was reused");
        assert!(matches!(prover.outcome(), Outcome::Failed(_)));
    }

    /// A reply to an earlier try is still a reply, on a line slower than the
    /// retry timer.
    #[test]
    fn a_late_answer_to_an_earlier_request_still_counts() {
        let mut prover = Prover::new(Auth::Pap, Account::new("guest", "letmein"));
        let first = prover.take_output().remove(0);
        prover.tick(RESTART_MS);
        assert_eq!(prover.take_output().len(), 1, "no second try");
        let mut checker = Authenticator::new(Auth::Pap, accounts(), "binmodem", 1);
        checker.receive(&first);
        for reply in checker.take_output() {
            prover.receive(&reply);
        }
        assert_eq!(prover.outcome(), &Outcome::Passed);
    }

    /// The same question gets the same answer, however many times it is asked.
    #[test]
    fn a_verdict_once_given_is_never_changed() {
        let mut checker = Authenticator::new(Auth::Pap, accounts(), "binmodem", 1);
        let wrong = Prover::new(Auth::Pap, Account::new("guest", "nope")).take_output().remove(0);
        let mut right = Prover::new(Auth::Pap, Account::new("guest", "letmein")).take_output().remove(0);
        right[1] = 9;
        checker.receive(&wrong);
        checker.receive(&right);
        let replies = checker.take_output();
        assert_eq!(replies[0][0], pap::AUTHENTICATE_NAK);
        assert_eq!(replies[1][0], pap::AUTHENTICATE_NAK, "a second guess got in");
        assert_eq!(replies[1][1], 9, "the reply did not copy the request's Identifier");
    }

    #[test]
    fn chap_lets_the_right_password_in_without_sending_it() {
        let mut prover = Prover::new(Auth::ChapMd5, Account::new("rory", "hunter2"));
        let mut checker = Authenticator::new(Auth::ChapMd5, accounts(), "binmodem", 42);
        let mut crossed = Vec::new();
        for _ in 0..1000 {
            for p in prover.take_output() {
                crossed.extend_from_slice(&p);
                checker.receive(&p);
            }
            for p in checker.take_output() {
                crossed.extend_from_slice(&p);
                prover.receive(&p);
            }
            prover.tick(1);
            checker.tick(1);
        }
        assert_eq!(prover.outcome(), &Outcome::Passed);
        assert_eq!(checker.outcome(), &Outcome::Passed);
        assert_eq!(checker.who(), Some("rory"));
        assert!(
            !crossed.windows(7).any(|w| w == b"hunter2"),
            "the password went across the link"
        );
    }

    #[test]
    fn chap_keeps_the_wrong_password_out() {
        let mut prover = Prover::new(Auth::ChapMd5, Account::new("rory", "hunter3"));
        let mut checker = Authenticator::new(Auth::ChapMd5, accounts(), "binmodem", 42);
        run(&mut prover, &mut checker, 1000);
        assert!(matches!(prover.outcome(), Outcome::Failed(_)));
        assert!(matches!(checker.outcome(), Outcome::Failed(_)));
    }

    /// 4.1's formula, against the same thing worked out a different way: the
    /// digest of the Identifier, secret and challenge written out by hand.
    #[test]
    fn a_response_is_the_hash_of_identifier_secret_and_challenge() {
        let challenge = [0x11, 0x22, 0x33, 0x44];
        let mut stream = vec![0x07];
        stream.extend_from_slice(b"secret");
        stream.extend_from_slice(&challenge);
        assert_eq!(chap_response(7, b"secret", &challenge), crate::md5::digest(&stream));
        // And a different Identifier is a different answer, which is what
        // stops an old answer being replayed to a new challenge.
        assert_ne!(chap_response(8, b"secret", &challenge), chap_response(7, b"secret", &challenge));
    }

    /// 4.1: the Identifier and the value both change each time.
    #[test]
    fn every_challenge_is_a_new_one() {
        let mut checker = Authenticator::new(Auth::ChapMd5, accounts(), "binmodem", 42);
        let mut seen = Vec::new();
        for _ in 0..(RESTART_MS * 3) {
            seen.extend(checker.take_output());
            checker.tick(1);
        }
        assert!(seen.len() >= 3);
        for (i, a) in seen.iter().enumerate() {
            for b in &seen[i + 1..] {
                assert_ne!(a[1], b[1], "an Identifier came round again");
                assert_ne!(a[5..21], b[5..21], "a challenge value came round again");
            }
        }
        // And two calls with different seeds do not challenge alike.
        let other = Authenticator::new(Auth::ChapMd5, accounts(), "binmodem", 43).take_output();
        assert_ne!(other[0][5..21], seen[0][5..21]);
    }

    /// An answer to a challenge that was never sent is not looked at.
    #[test]
    fn a_response_to_no_challenge_is_ignored() {
        let mut checker = Authenticator::new(Auth::ChapMd5, accounts(), "binmodem", 42);
        let _ = checker.take_output();
        let mut data = Vec::new();
        counted(&chap_response(200, b"hunter2", b"made up"), &mut data);
        data.extend_from_slice(b"rory");
        checker.receive(&packet(chap::RESPONSE, 200, data));
        assert!(checker.take_output().is_empty());
        assert_eq!(checker.outcome(), &Outcome::Waiting);
    }

    #[test]
    fn chap_is_offered_before_pap_unless_it_cannot_be() {
        assert_eq!(methods_for(&accounts()), vec![Auth::ChapMd5, Auth::Pap]);
        assert_eq!(methods_for(&[Account::new("guest", "")]), vec![Auth::Pap]);
    }
}
