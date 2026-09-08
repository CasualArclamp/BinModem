//! What the window remembers between runs.
//!
//! The controls in the window are the modem's settings: when a line opens they
//! are asserted onto it with `AT&F` and then everything else, so what the
//! window shows is what the modem is running. That is the right arrangement
//! while the program is up and the wrong one across a restart, because the
//! window starts at its defaults and asserts *those* -- so a modem left on
//! V.32 with V.8 off comes back as V.22bis with V.8 on, having told nobody.
//!
//! It matters more than a lost preference. The whole point of asserting the
//! settings is that the window and the modem cannot disagree; a person who set
//! something for a reason and finds it undone, silently, has been given a
//! reason to distrust the display rather than a default.
//!
//! The format is one `name value` to a line. Not a format anyone should have
//! to look at, but one that can be looked at: a settings file that cannot be
//! read with a text editor is a settings file that has to be deleted when it
//! goes wrong.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Where the file lives.
///
/// Beside the executable is wrong -- that may be somewhere unwritable, and the
/// program is meant to be a single file that can sit anywhere. The per-user
/// application directory is the one place on Windows that is always the user's
/// to write in.
fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("BinModem"))
}

/// Settings read from the file, by name.
#[derive(Debug, Default)]
pub struct Remembered(BTreeMap<String, String>);

impl Remembered {
    /// Read what the last run left. Missing or unreadable is not an error: it
    /// is a first run, and the defaults are right for one.
    pub fn load() -> Self {
        let Some(dir) = path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(dir.join("settings.txt")) else {
            return Self::default();
        };
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((name, value)) = line.split_once(' ') {
                map.insert(name.to_owned(), value.trim().to_owned());
            }
        }
        Self(map)
    }

    /// Whatever was under `name`, parsed. Anything that will not parse is
    /// treated as absent, so a file edited into nonsense degrades to defaults
    /// rather than to a panic.
    pub fn get<T: std::str::FromStr>(&self, name: &str) -> Option<T> {
        self.0.get(name)?.parse().ok()
    }

    /// The text under `name`, if any.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: impl ToString) {
        self.0.insert(name.to_owned(), value.to_string());
    }

    /// Write it out, best effort. A settings file that cannot be written is
    /// not worth interrupting anybody over: the program works without one.
    pub fn save(&self) {
        let Some(dir) = path() else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let mut text = String::from(
            "# BinModem. Written when a setting changes; delete it to start \
             again.\n",
        );
        for (name, value) in &self.0 {
            text.push_str(name);
            text.push(' ');
            text.push_str(value);
            text.push('\n');
        }
        let _ = std::fs::write(dir.join("settings.txt"), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_reads_as_nothing_remembered() {
        let empty = Remembered::default();
        assert_eq!(empty.get::<u32>("carrier"), None);
        assert_eq!(empty.text("carrier"), None);
    }

    #[test]
    fn what_is_written_comes_back() {
        let mut r = Remembered::default();
        r.set("carrier", "V32");
        r.set("automode", false);
        r.set("max_dict", 2048u16);
        assert_eq!(r.text("carrier"), Some("V32"));
        assert_eq!(r.get::<bool>("automode"), Some(false));
        assert_eq!(r.get::<u16>("max_dict"), Some(2048));
    }

    /// A file someone has edited into nonsense gives defaults, not a panic.
    #[test]
    fn nonsense_reads_as_absent() {
        let mut r = Remembered::default();
        r.set("automode", "yes please");
        assert_eq!(r.get::<bool>("automode"), None);
    }
}
