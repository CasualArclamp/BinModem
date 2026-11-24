//! The dialler's contacts, and how the last call to each went.
//!
//! A file of its own beside `sip.txt` and the settings, one contact to a
//! line, so that it can be edited by hand, copied to another machine or
//! deleted without touching anything else:
//!
//! ```text
//! The Internet | 638-782-5442 | V.92 56000 42666 1795507202
//! ```
//!
//! Name, number, and -- written by the program -- the last call this end
//! placed to that number that came up: the modulation, the rate down and the
//! rate up, and when, in seconds since 1970. A line with no third field is a
//! contact never called, or never reached.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// How the last call to a contact went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Last {
    /// The modulation the call came up on, as the window names it.
    pub mode: String,
    /// The rate arriving here, and the rate this end sent at.
    pub down: u32,
    pub up: u32,
    /// Seconds since 1970.
    pub when: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub number: String,
    pub last: Option<Last>,
}

/// Where the file lives: the same per-user directory as the settings.
fn path() -> Option<PathBuf> {
    crate::remembered::directory().map(|dir| dir.join("contacts.txt"))
}

const HEADER: &str = "# BinModem contacts: name | number | last call (the program writes the last field)\n";

/// Read the contacts. A missing or unreadable file is no contacts yet.
pub fn load() -> Vec<Contact> {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|text| parse(&text)).unwrap_or_default()
}

/// Write them back, whole.
pub fn save(contacts: &[Contact]) -> std::io::Result<()> {
    let Some(p) = path() else { return Ok(()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, render(contacts))
}

pub fn parse(text: &str) -> Vec<Contact> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('|').map(str::trim);
            let name = fields.next()?.to_owned();
            let number = fields.next()?.to_owned();
            if number.is_empty() {
                return None;
            }
            let last = fields.next().and_then(|f| {
                let mut words = f.split_whitespace();
                let mode = words.next()?.to_owned();
                let down = words.next()?.parse().ok()?;
                let up = words.next()?.parse().ok()?;
                let when = words.next()?.parse().ok()?;
                Some(Last { mode, down, up, when })
            });
            Some(Contact { name, number, last })
        })
        .collect()
}

pub fn render(contacts: &[Contact]) -> String {
    let mut text = HEADER.to_owned();
    for c in contacts {
        // A bar in a name would split it; it is the one character a name
        // cannot keep.
        let name = c.name.replace('|', "/");
        text.push_str(&format!("{} | {}", name.trim(), c.number.trim()));
        if let Some(l) = &c.last {
            text.push_str(&format!(" | {} {} {} {}", l.mode.replace(' ', ""), l.down, l.up, l.when));
        }
        text.push('\n');
    }
    text
}

/// A number reduced to what is dialled: its digits and a leading plus, so
/// that "638-782-5442" and "(638) 782 5442" are the same contact. A SIP
/// address is kept as written, lower case.
pub fn key(number: &str) -> String {
    let n = number.trim();
    if n.contains('@') || n.to_ascii_lowercase().starts_with("sip:") {
        return n.to_ascii_lowercase();
    }
    n.chars().filter(|c| c.is_ascii_digit() || *c == '+' || *c == '*' || *c == '#').collect()
}

/// The contact whose number this is, if any.
pub fn find(contacts: &[Contact], number: &str) -> Option<usize> {
    let k = key(number);
    (!k.is_empty()).then(|| contacts.iter().position(|c| key(&c.number) == k)).flatten()
}

/// Note a call to `number` that came up. True if a contact took it.
pub fn record(contacts: &mut [Contact], number: &str, last: Last) -> bool {
    match find(contacts, number) {
        Some(i) => {
            contacts[i].last = Some(last);
            true
        }
        None => false,
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// "V.92 56000 / 42666 · 3 h ago": the line under a contact.
pub fn describe(last: &Last, now: u64) -> String {
    let rates = if last.down == last.up {
        format!("{}", last.down)
    } else {
        format!("{} / {}", last.down, last.up)
    };
    format!("{} {} · {}", last.mode, rates, ago(now.saturating_sub(last.when)))
}

/// How long ago, in the largest unit that is at least one.
pub fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        86_400..172_800 => "yesterday".to_owned(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_reads_back_as_it_was_written() {
        let contacts = vec![
            Contact {
                name: "The Internet".into(),
                number: "638-782-5442".into(),
                last: Some(Last { mode: "V.92".into(), down: 56000, up: 42666, when: 1_795_507_202 }),
            },
            Contact { name: "Telstra TestFax".into(), number: "1300 368 909".into(), last: None },
        ];
        assert_eq!(parse(&render(&contacts)), contacts);
    }

    #[test]
    fn numbers_are_matched_by_what_is_dialled() {
        let mut contacts = parse("The Internet | (638) 782 5442\nPBX | sip:1000@pbx.local\n");
        assert_eq!(find(&contacts, "638-782-5442"), Some(0));
        assert_eq!(find(&contacts, "SIP:1000@PBX.local"), Some(1));
        assert_eq!(find(&contacts, "5442"), None);
        assert_eq!(find(&contacts, ""), None);
        let last = Last { mode: "V.90".into(), down: 50666, up: 31200, when: 10 };
        assert!(record(&mut contacts, "6387825442", last.clone()));
        assert_eq!(contacts[0].last, Some(last));
        assert!(!record(&mut contacts, "999", Last { mode: "V.34".into(), down: 1, up: 1, when: 1 }));
    }

    #[test]
    fn a_line_that_will_not_parse_is_dropped_not_fatal() {
        let contacts = parse("# header\n\nno number |\nGood | 123 | rubbish\n");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].last, None);
    }

    #[test]
    fn the_line_under_a_contact_reads_plainly() {
        let last = Last { mode: "V.92".into(), down: 56000, up: 42666, when: 1000 };
        assert_eq!(describe(&last, 1000 + 3 * 3600 + 5), "V.92 56000 / 42666 · 3 h ago");
        let same = Last { mode: "V.32bis".into(), down: 14400, up: 14400, when: 0 };
        assert_eq!(describe(&same, 30), "V.32bis 14400 · just now");
        assert_eq!(ago(90_000), "yesterday");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }
}
