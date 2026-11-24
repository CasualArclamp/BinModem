//! The connections this modem recognises, kept in a file between runs.
//!
//! V.92's short start-up is for "recognized connections" (1 j), and a
//! connection here is a dial string: the same string dialled again is taken
//! to reach the same server over the same line. Each one that has carried a
//! V.92 call to data mode has a line in the file -- the string, a space, and
//! what the call taught ([`Recognized::to_text`]) -- and the next call to it
//! opens with QC1a.
//!
//! It is its own file so that it can be looked at and so that it can be
//! thrown away: a memo that has gone stale costs a fall-back to the full
//! start-up on the call that finds out, and is dropped by that call, but a
//! person who suspects one can also just delete the line.

use std::collections::BTreeMap;
use std::path::PathBuf;

use datapump::v92::memo::Recognized;

/// The key a call dialled with no number at all is kept under: the window's
/// Originate types a bare `ATD`, and there is still only one place such a
/// call can go.
const NO_NUMBER: &str = "-";

/// What is remembered, by dial string, and the file it lives in.
#[derive(Debug, Clone, Default)]
pub struct Memos {
    path: Option<PathBuf>,
    known: BTreeMap<String, Recognized>,
}

impl Memos {
    /// Read `path`, or start empty if it is not there or will not read. A
    /// line that will not parse is left out, and so is gone at the next
    /// write.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut known = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, memo)) = line.split_once(' ')
                    && let Some(memo) = Recognized::from_text(memo)
                {
                    known.insert(key.to_owned(), memo);
                }
            }
        }
        Self { path: Some(path), known }
    }

    /// The dial string as a key: no white space, and never empty.
    fn key(number: &str) -> String {
        let key: String = number.chars().filter(|c| !c.is_whitespace()).collect();
        if key.is_empty() { NO_NUMBER.to_owned() } else { key }
    }

    /// What an earlier call to `number` taught, if one did.
    pub fn get(&self, number: &str) -> Option<Recognized> {
        self.known.get(&Self::key(number)).copied()
    }

    /// Remember what a call to `number` taught, and write the file.
    pub fn put(&mut self, number: &str, memo: Recognized) {
        if self.known.insert(Self::key(number), memo) != Some(memo) {
            self.save();
        }
    }

    /// Forget `number`, and write the file if there was anything to forget.
    pub fn forget(&mut self, number: &str) {
        if self.known.remove(&Self::key(number)).is_some() {
            self.save();
        }
    }

    /// Best effort, as the window's settings are: a memo that cannot be
    /// written is a full start-up next time, which always works.
    fn save(&self) {
        let Some(path) = self.path.as_ref() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = String::from(
            "# BinModem: V.92 connections that have connected before, for the short start-up.\n\
             # Delete a line, or the file, to have the next call start up in full.\n",
        );
        for (key, memo) in &self.known {
            text.push_str(key);
            text.push(' ');
            text.push_str(&memo.to_text());
            text.push('\n');
        }
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_put_is_there_after_the_file_is_read_again() {
        let path = std::env::temp_dir().join(format!("binmodem-memos-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let memo = Recognized { uqts: 70, a_law: false, round_trip_ms: 41, pcm_upstream: true };
        let mut memos = Memos::open(&path);
        assert_eq!(memos.get("sip:modem@127.0.0.1:5071"), None);
        memos.put("sip:modem@127.0.0.1:5071", memo);
        memos.put("555 1234", Recognized::unmeasured());
        memos.put("", memo);

        let mut again = Memos::open(&path);
        assert_eq!(again.get("sip:modem@127.0.0.1:5071"), Some(memo));
        assert_eq!(again.get("5551234"), Some(Recognized::unmeasured()), "white space is not part of a number");
        assert_eq!(again.get("  "), Some(memo), "a bare ATD has a slot of its own");
        again.forget("5551234");
        assert_eq!(Memos::open(&path).get("555 1234"), None);
        assert_eq!(Memos::open(&path).get(""), Some(memo));
        let _ = std::fs::remove_file(&path);
    }
}
