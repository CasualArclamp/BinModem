//! Unpredictable-enough tokens: branches, tags, call identifiers, cnonces.
//!
//! SIP asks for randomness in several places and means different things by it.
//! A branch parameter has to be unique over time so that a retransmission is
//! matched to its own transaction and not to a previous one (8.1.1.7); a From
//! tag has to be unique so that two calls between the same pair of addresses
//! are distinguishable (19.3); a cnonce is a nonce the client chooses, whose
//! job is to stop a server from picking the whole input to a hash. None of
//! them is a secret and none of them protects anything.
//!
//! So: a counter, the clock, and the process, mixed. Uniqueness is what is
//! actually required here, and this delivers it without pretending to be a
//! source of cryptographic randomness that the rest of the protocol -- plain
//! text over UDP -- would have no use for.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Appleby's mix, which is what makes a counter look like anything at all.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A number no other call to this will return.
pub fn number() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    mix(nanos ^ mix(n).rotate_left(17) ^ u64::from(std::process::id()).rotate_left(41))
}

/// Hex characters, as many as asked for.
pub fn token(length: usize) -> String {
    let mut out = String::with_capacity(length);
    while out.len() < length {
        let word = number();
        for shift in (0..64).step_by(4) {
            if out.len() == length {
                break;
            }
            let nibble = (word >> shift) & 0x0F;
            out.push(char::from_digit(nibble as u32, 16).unwrap());
        }
    }
    out
}

/// A branch parameter: 8.1.1.7's magic cookie and then something unique.
///
/// The cookie is not decoration. It is how a far end tells an agent that
/// computes branches the way RFC 3261 asks from one that does it the way RFC
/// 2543 did, and the two are matched by entirely different rules.
pub fn branch() -> String {
    format!("z9hG4bK{}", token(16))
}

/// A Call-ID: unique here, and qualified by this host so that it is unique
/// everywhere (8.1.1.4).
pub fn call_id(host: &str) -> String {
    format!("{}@{host}", token(24))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tokens_do_not_repeat() {
        let made: HashSet<String> = (0..1000).map(|_| token(16)).collect();
        assert_eq!(made.len(), 1000);
    }

    #[test]
    fn a_token_is_the_length_asked_for() {
        for length in [1, 7, 16, 17, 32, 64] {
            assert_eq!(token(length).len(), length);
        }
    }

    #[test]
    fn a_branch_carries_the_cookie() {
        assert!(branch().starts_with("z9hG4bK"));
    }
}
