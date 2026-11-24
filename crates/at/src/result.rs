//! Result codes and response formatting (V.250 5.7).

use crate::registers::Registers;

/// Result codes of Table 1/V.250.
///
/// Numeric 5 is deliberately absent: V.250 assigns 0-4 and 6-8, leaving 5
/// unallocated. Hayes practice used 5 for `CONNECT 1200`, but V.250 makes the
/// numeric form of `CONNECT <text>` manufacturer-specific, so nothing standard
/// occupies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultCode {
    Ok,
    Connect,
    Ring,
    NoCarrier,
    Error,
    NoDialtone,
    Busy,
    NoAnswer,
    /// `CONNECT <text>`: manufacturer-specific detail such as line speed and
    /// error-control state, issued when X is 1 or above.
    ConnectText(String),
    /// An extended-format result code such as `+ER: LAPM` (V.250 5.7.2).
    ///
    /// Alphabetic whatever V says -- "unlike basic format result codes,
    /// extended syntax result codes have no numeric equivalent, and are always
    /// issued in alphabetic form" -- while the headers and trailers around it
    /// follow V exactly as a basic code's do. Q1 suppresses it like any other
    /// result code; X does not reach it at all.
    Extended(String),
}

impl ResultCode {
    /// Numeric form, used when V0 is selected.
    ///
    /// `None` for the extended codes, which have no numeric form in any mode.
    pub fn numeric(&self) -> Option<u8> {
        Some(match self {
            Self::Ok => 0,
            // V.250 leaves numeric CONNECT <text> to the manufacturer, so it
            // degrades to plain CONNECT rather than inventing a code.
            Self::Connect | Self::ConnectText(_) => 1,
            Self::Ring => 2,
            Self::NoCarrier => 3,
            Self::Error => 4,
            Self::NoDialtone => 6,
            Self::Busy => 7,
            Self::NoAnswer => 8,
            Self::Extended(_) => return None,
        })
    }

    /// Verbose form, used when V1 is selected. V.250 5.1 requires upper case.
    pub fn verbose(&self) -> String {
        match self {
            Self::Ok => "OK".into(),
            Self::Connect => "CONNECT".into(),
            Self::Ring => "RING".into(),
            Self::NoCarrier => "NO CARRIER".into(),
            Self::Error => "ERROR".into(),
            Self::NoDialtone => "NO DIALTONE".into(),
            Self::Busy => "BUSY".into(),
            Self::NoAnswer => "NO ANSWER".into(),
            Self::ConnectText(t) => format!("CONNECT {t}"),
            Self::Extended(t) => t.clone(),
        }
    }

    /// Final result codes signal that the DCE will accept new commands
    /// (V.250 5.7.1). `CONNECT` is intermediate, `RING` unsolicited.
    pub fn is_final(&self) -> bool {
        // The extended codes this modem issues -- +ER and +DR -- are
        // intermediate by their own definitions (V.250 6.5.5, 6.6.3): they
        // come during the handshake, before the CONNECT that ends it.
        !matches!(
            self,
            Self::Connect | Self::ConnectText(_) | Self::Ring | Self::Extended(_)
        )
    }
}

/// Formats responses according to the V setting and S3/S4 (V.250 Table 3).
#[derive(Debug, Clone, Copy)]
pub struct Formatter {
    /// V parameter: false is V0 (numeric, limited headers), true is V1.
    pub verbose: bool,
    /// Q parameter: when true, result codes are suppressed (V.250 6.2.5).
    pub quiet: bool,
}

impl Default for Formatter {
    fn default() -> Self {
        // V.250 6.2.6 and 6.2.5 recommend V1 and Q0.
        Self { verbose: true, quiet: false }
    }
}

impl Formatter {
    /// Render a result code.
    ///
    /// Table 3/V.250:
    /// - V0: `<numeric code><cr>`
    /// - V1: `<cr><lf><verbose code><cr><lf>`
    pub fn result(&self, code: &ResultCode, regs: &Registers, out: &mut Vec<u8>) {
        if self.quiet {
            return;
        }
        let (cr, lf) = (regs.terminator(), regs.formatter());
        if self.verbose {
            out.push(cr);
            out.push(lf);
            out.extend_from_slice(code.verbose().as_bytes());
            out.push(cr);
            out.push(lf);
        } else if let Some(n) = code.numeric() {
            out.extend_from_slice(n.to_string().as_bytes());
            out.push(cr);
        } else {
            // No numeric form to fall back to, so the body is the same in
            // either mode and only the trailer changes.
            out.extend_from_slice(code.verbose().as_bytes());
            out.push(cr);
        }
    }

    /// Render an information text response.
    ///
    /// Table 3/V.250:
    /// - V0: `<text><cr><lf>`
    /// - V1: `<cr><lf><text><cr><lf>`
    ///
    /// Information text is emitted even when Q1 suppresses result codes:
    /// V.250 6.2.5 speaks only of result codes.
    pub fn info(&self, text: &str, regs: &Registers, out: &mut Vec<u8>) {
        let (cr, lf) = (regs.terminator(), regs.formatter());
        if self.verbose {
            out.push(cr);
            out.push(lf);
        }
        out.extend_from_slice(text.as_bytes());
        out.push(cr);
        out.push(lf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(f: Formatter, code: ResultCode) -> String {
        let mut v = Vec::new();
        f.result(&code, &Registers::default(), &mut v);
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn numeric_values_match_table_1() {
        assert_eq!(ResultCode::Ok.numeric(), Some(0));
        assert_eq!(ResultCode::Connect.numeric(), Some(1));
        assert_eq!(ResultCode::Ring.numeric(), Some(2));
        assert_eq!(ResultCode::NoCarrier.numeric(), Some(3));
        assert_eq!(ResultCode::Error.numeric(), Some(4));
        assert_eq!(ResultCode::NoDialtone.numeric(), Some(6));
        assert_eq!(ResultCode::Busy.numeric(), Some(7));
        assert_eq!(ResultCode::NoAnswer.numeric(), Some(8));
    }

    #[test]
    fn verbose_format_wraps_in_cr_lf_both_sides() {
        let f = Formatter { verbose: true, quiet: false };
        assert_eq!(rendered(f, ResultCode::Ok), "\r\nOK\r\n");
        assert_eq!(rendered(f, ResultCode::NoCarrier), "\r\nNO CARRIER\r\n");
    }

    #[test]
    fn numeric_format_has_a_bare_trailing_cr() {
        // Table 3: V0 result codes are <numeric code><cr>, with no leading
        // <cr><lf> and no <lf> after.
        let f = Formatter { verbose: false, quiet: false };
        assert_eq!(rendered(f, ResultCode::Ok), "0\r");
        assert_eq!(rendered(f, ResultCode::Busy), "7\r");
    }

    #[test]
    fn quiet_suppresses_result_codes() {
        let f = Formatter { verbose: true, quiet: true };
        assert_eq!(rendered(f, ResultCode::Ok), "");
    }

    #[test]
    fn information_text_format_follows_table_3() {
        let regs = Registers::default();
        let mut v = Vec::new();
        Formatter { verbose: true, quiet: false }.info("050", &regs, &mut v);
        assert_eq!(String::from_utf8(v).unwrap(), "\r\n050\r\n");

        let mut v = Vec::new();
        Formatter { verbose: false, quiet: false }.info("050", &regs, &mut v);
        assert_eq!(String::from_utf8(v).unwrap(), "050\r\n");
    }

    #[test]
    fn s3_and_s4_change_the_framing_characters() {
        // V.250 6.2.1: the terminator is whatever S3 says, not necessarily CR.
        let mut regs = Registers::default();
        regs.set(3, 30).unwrap();
        regs.set(4, 31).unwrap();
        let mut v = Vec::new();
        Formatter::default().result(&ResultCode::Ok, &regs, &mut v);
        assert_eq!(v, b"\x1e\x1fOK\x1e\x1f");
    }

    #[test]
    fn connect_is_intermediate_not_final() {
        assert!(!ResultCode::Connect.is_final());
        assert!(!ResultCode::ConnectText("33600/V42BIS".into()).is_final());
        assert!(!ResultCode::Ring.is_final());
        assert!(ResultCode::Ok.is_final());
        assert!(ResultCode::NoCarrier.is_final());
    }

    #[test]
    fn connect_text_renders_with_its_detail() {
        let f = Formatter::default();
        assert_eq!(
            rendered(f, ResultCode::ConnectText("33600/V42BIS".into())),
            "\r\nCONNECT 33600/V42BIS\r\n"
        );
    }
}
