//! What a ZFILE frame's subpacket says about the file (13, and 8.2).
//!
//! 8.2: a ZFILE header is "followed by a ZCRCW data subpacket containing the
//! file name, file length, modification date, and other information identical
//! to that used by YMODEM Batch".
//!
//! The name comes first and ends with a NUL. Everything after it is one
//! space-separated string, also NUL-terminated, and clause 13 is firm about
//! what goes in it: the length in decimal, the modification time in octal
//! seconds since 1970 UTC, the mode in octal, then a serial number. "Fields
//! may not be skipped" -- they are optional only from the right.

/// What the sender says about a file it is about to send.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileInfo {
    /// The name, with directories delimited by `/`. Clause 13: "if directories
    /// are included, they are delimited by /; i.e., `subdir/foo` is
    /// acceptable, `subdir\foo` is not."
    pub name: String,
    /// Length in bytes, where the sender gave one.
    ///
    /// An estimate, and the document says so: "the ZMODEM receiver uses the
    /// file length as an estimate only ... a file may grow after transmission
    /// commences, and all the data will be sent." Good for a progress bar and
    /// not for deciding when to stop.
    pub length: Option<u64>,
    /// Seconds since 1 January 1970 UTC, where the sender gave one.
    ///
    /// "A date of 0 implies the modification date is unknown and should be
    /// left as the date the file is received", so zero arrives here as `None`.
    pub modified: Option<u64>,
    /// Unix file mode, or zero where the file did not come from one.
    pub mode: u32,
}

impl FileInfo {
    /// The subpacket body for this file.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.name.len() + 32);
        // Backslashes are not a path separator here whatever this machine
        // thinks, and a name carrying one would arrive at the far end as a
        // file with a backslash in it.
        out.extend(self.name.replace('\\', "/").as_bytes());
        out.push(0);
        let rest = format!(
            "{} {:o} {:o} 0 0 0",
            self.length.unwrap_or(0),
            self.modified.unwrap_or(0),
            self.mode
        );
        out.extend(rest.as_bytes());
        out.push(0);
        out
    }

    /// Read one back.
    ///
    /// Tolerant of everything after the name, because clause 13 makes those
    /// fields optional and real senders stop at different points along the
    /// row. Not tolerant of a missing name: without it there is no file.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let end = body.iter().position(|&b| b == 0)?;
        let name = String::from_utf8_lossy(&body[..end]).into_owned();
        if name.is_empty() {
            return None;
        }
        let tail = &body[end + 1..];
        let tail = &tail[..tail.iter().position(|&b| b == 0).unwrap_or(tail.len())];
        let tail = String::from_utf8_lossy(tail);
        let mut fields = tail.split_ascii_whitespace();

        let length = fields.next().and_then(|f| f.parse::<u64>().ok());
        // Octal, because clause 13 says so twice: the date "is sent as an
        // octal number" and the mode "is stored as an octal string".
        let modified = fields
            .next()
            .and_then(|f| u64::from_str_radix(f, 8).ok())
            .filter(|t| *t != 0);
        let mode = fields.next().and_then(|f| u32::from_str_radix(f, 8).ok()).unwrap_or(0);

        Some(Self { name, length, modified, mode })
    }

    /// The name with anything that could escape a directory taken out.
    ///
    /// 8.2 puts this on the receiver: "the receiving program should insure the
    /// pathname and options are compatible with its operating environment and
    /// local security requirements." A board is not a trusted party, and a
    /// name it chose is the one part of a transfer that decides where the
    /// bytes land.
    ///
    /// Taking the last component is not the whole of it on Windows, where a
    /// name with no separator in it can still leave the folder. `C:x.dll` is
    /// drive-relative, and a path joined onto it is replaced by it, so the
    /// bytes landed in drive C's current directory -- for a program started
    /// by double-clicking, the program's own folder, where a DLL it loads by
    /// bare name is looked for first. `notes.txt:x` is an alternate stream of
    /// a file that may not be the one it seems, and `NUL` or `COM1.txt` is a
    /// device rather than a file. The colon and the other characters Windows
    /// will not have in a name become `_`, and a device's name gets one in
    /// front of it.
    pub fn safe_name(&self) -> String {
        let last = self
            .name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .trim_matches(|c: char| c == '.' || c.is_whitespace() || c.is_control());
        if last.is_empty() {
            return "received".to_owned();
        }
        let name: String = last
            .chars()
            .map(|c| if c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*') { '_' } else { c })
            .collect();
        if is_device(&name) { format!("_{name}") } else { name }
    }
}

/// Whether Windows takes a name for a device rather than a file.
///
/// What counts is the part before the first dot, whatever follows it:
/// `NUL.txt` is the null device as much as `NUL` is. Trailing spaces go too,
/// so `CON .log` is still the console.
fn is_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let digit = |s: &str| {
                let mut c = s.chars();
                matches!((c.next(), c.next()), (Some('0'..='9' | '¹' | '²' | '³'), None))
            };
            stem.strip_prefix("COM").is_some_and(digit) || stem.strip_prefix("LPT").is_some_and(digit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_round_trips() {
        let f = FileInfo {
            name: "README.TXT".into(),
            length: Some(4096),
            modified: Some(0x6000_0000),
            mode: 0o100644,
        };
        assert_eq!(FileInfo::parse(&f.encode()), Some(f));
    }

    #[test]
    fn the_name_is_terminated_and_the_rest_is_one_string() {
        // 8.2, and clause 13: name, NUL, then the fields separated by single
        // spaces, then another NUL.
        let f = FileInfo { name: "a.zip".into(), length: Some(10), ..FileInfo::default() };
        let body = f.encode();
        assert_eq!(&body[..5], b"a.zip");
        assert_eq!(body[5], 0);
        assert_eq!(*body.last().unwrap(), 0);
    }

    #[test]
    fn the_length_is_decimal_and_the_date_and_mode_are_octal() {
        // Clause 13 is explicit about each, and they are not the same base --
        // a length read as octal or a date read as decimal is a plausible
        // number that is simply wrong.
        let f = FileInfo {
            name: "x".into(),
            length: Some(999),
            modified: Some(0o1234),
            mode: 0o644,
        };
        let body = String::from_utf8(f.encode()).unwrap();
        assert!(body.contains("999 1234 644"), "{body:?}");
    }

    #[test]
    fn a_sender_that_stops_early_is_still_understood() {
        // Clause 13: "the file length and each of the succeeding fields are
        // optional". Real senders stop at different points.
        assert_eq!(FileInfo::parse(b"name\x00").unwrap().name, "name");
        assert_eq!(FileInfo::parse(b"name\x00512\x00").unwrap().length, Some(512));
        let f = FileInfo::parse(b"name\x00512 1234\x00").unwrap();
        assert_eq!(f.modified, Some(0o1234));
        assert_eq!(f.mode, 0);
    }

    #[test]
    fn a_date_of_zero_means_nobody_knows() {
        // Clause 13: "a date of 0 implies the modification date is unknown and
        // should be left as the date the file is received".
        assert_eq!(FileInfo::parse(b"name\x00512 0 0\x00").unwrap().modified, None);
    }

    #[test]
    fn a_body_with_no_name_is_not_a_file() {
        assert_eq!(FileInfo::parse(b""), None);
        assert_eq!(FileInfo::parse(b"\x00512\x00"), None);
    }

    #[test]
    fn a_name_cannot_choose_where_the_bytes_land() {
        // 8.2 puts this on the receiver, and a board is not a trusted party.
        // The name is the one part of a transfer that decides what gets
        // written, and it was chosen by the far end.
        for hostile in [
            "../../../etc/passwd",
            "..\\..\\windows\\system32\\drivers\\etc\\hosts",
            "/etc/shadow",
            "C:/Windows/System32/config/SAM",
            "....//....//boot.ini",
        ] {
            let f = FileInfo { name: hostile.into(), ..FileInfo::default() };
            let safe = f.safe_name();
            assert!(!safe.contains('/') && !safe.contains('\\'), "{hostile} -> {safe}");
            assert!(!safe.starts_with('.'), "{hostile} -> {safe}");
            assert!(!safe.is_empty());
        }
        assert_eq!(
            FileInfo { name: "sub/dir/FILE.ZIP".into(), ..FileInfo::default() }.safe_name(),
            "FILE.ZIP"
        );
    }

    /// A name with no separator in it can still leave the folder on Windows:
    /// a drive-relative path replaces the folder it is joined onto, a colon
    /// further along names an alternate stream, and a device's name is not a
    /// file at all.
    #[test]
    fn a_name_cannot_leave_the_folder_on_windows_either() {
        for (hostile, safe) in [
            ("C:opengl32.dll", "C_opengl32.dll"),
            ("c:version.dll", "c_version.dll"),
            ("sub/C:dxgi.dll", "C_dxgi.dll"),
            ("notes.txt:hidden", "notes.txt_hidden"),
            ("evil.exe::$DATA", "evil.exe__$DATA"),
            ("a?b.txt", "a_b.txt"),
            ("x<y>.zip", "x_y_.zip"),
            ("say \"hi\"|now*", "say _hi__now_"),
            ("NUL", "_NUL"),
            ("con", "_con"),
            ("COM1", "_COM1"),
            ("LPT1.txt", "_LPT1.txt"),
            ("aux.tar.gz", "_aux.tar.gz"),
            ("CON .log", "_CON .log"),
            ("COM¹", "_COM¹"),
        ] {
            let f = FileInfo { name: hostile.into(), ..FileInfo::default() };
            assert_eq!(f.safe_name(), safe, "{hostile}");
        }
        // Names that only look like devices are files.
        for fine in ["COM10", "CONSOLE.TXT", "NULL", "COMMAND.COM", "LPT", "ACON"] {
            let f = FileInfo { name: fine.into(), ..FileInfo::default() };
            assert_eq!(f.safe_name(), fine);
        }
    }

    #[test]
    fn a_name_that_is_nothing_usable_still_gets_written_somewhere() {
        for empty in ["...", "/", "   ", "\u{7}"] {
            let f = FileInfo { name: empty.into(), ..FileInfo::default() };
            assert_eq!(f.safe_name(), "received", "{empty:?}");
        }
    }

    #[test]
    fn backslashes_are_not_sent() {
        // Clause 13: "subdir/foo is acceptable, subdir\foo is not". This
        // machine's own separator is the wrong one, and a name carrying it
        // would arrive as a file with a backslash in its name.
        let f = FileInfo { name: r"sub\dir\FILE.ZIP".into(), ..FileInfo::default() };
        let body = f.encode();
        assert!(!body.contains(&b'\\'));
        assert_eq!(FileInfo::parse(&body).unwrap().name, "sub/dir/FILE.ZIP");
    }
}
