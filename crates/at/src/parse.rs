//! Command line parsing (V.250 5.2, 5.3 and 5.4).

use std::fmt;

/// One command from a command line body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Basic syntax: a single letter, optionally prefixed with `&`, with an
    /// optional decimal number (V.250 5.3.1).
    Basic { amp: bool, letter: char, number: Option<u32> },
    /// `D<dial string>`; the rest of the line belongs to it (V.250 6.3.1).
    Dial(String),
    /// `S<n>?` — read an S-parameter (V.250 5.3.2).
    ReadS(u8),
    /// `S<n>=<value>`; the value may be absent (V.250 5.3.2).
    SetS(u8, Option<u32>),
    /// Extended syntax, `+NAME` with an operation (V.250 5.4).
    Extended { name: String, op: ExtOp },
}

/// What an extended-syntax command is asking for (V.250 5.4.3, 5.4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtOp {
    /// `+NAME`
    Execute,
    /// `+NAME=<params>`
    Set(String),
    /// `+NAME?`
    Read,
    /// `+NAME=?`
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A character appeared where a command was expected.
    UnexpectedChar(char),
    /// `&` was not followed by an alphabetic character.
    BadAmpersand,
    /// An S-parameter number was absent, malformed, or above 255.
    BadSParameter,
    /// `S<n>` was not followed by `?` or `=` (V.250 5.3.2).
    MissingSOperator,
    /// A numeric value did not fit in the supported range.
    NumberTooLarge,
    /// An extended command name was empty or contained illegal characters.
    BadExtendedName,
    /// An extended command was followed by another command without `;`
    /// (V.250 5.4.5.1).
    MissingSeparator,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedChar(c) => write!(f, "unexpected character {c:?}"),
            Self::BadAmpersand => write!(f, "'&' must be followed by a letter"),
            Self::BadSParameter => write!(f, "malformed S-parameter number"),
            Self::MissingSOperator => write!(f, "S-parameter needs '?' or '='"),
            Self::NumberTooLarge => write!(f, "numeric value out of range"),
            Self::BadExtendedName => write!(f, "malformed extended command name"),
            Self::MissingSeparator => write!(f, "missing ';' after extended command"),
        }
    }
}

/// V.250 5.4.1: characters permitted in an extended command name after the
/// leading `+`. The first must be alphabetic.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '!' | '%' | '-' | '.' | '/' | ':' | '_')
}

struct Cursor<'a> {
    chars: Vec<char>,
    pos: usize,
    _src: &'a str,
}

impl<'a> Cursor<'a> {
    fn new(src: &'a str) -> Self {
        Self { chars: src.chars().collect(), pos: 0, _src: src }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// V.250 5.2.1: space characters are ignored and may be used freely for
    /// formatting, except inside numeric or string constants.
    fn skip_spaces(&mut self) {
        while self.peek() == Some(' ') {
            self.pos += 1;
        }
    }

    /// Read a decimal number. V.250 5.3.1: all leading zeroes are ignored.
    fn number(&mut self) -> Result<Option<u32>, ParseError> {
        self.skip_spaces();
        let mut digits = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                digits.push(c);
                self.pos += 1;
                self.skip_spaces();
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return Ok(None);
        }
        digits
            .trim_start_matches('0')
            .parse::<u32>()
            .or_else(|_| if digits.chars().all(|c| c == '0') { Ok(0) } else { Err(ParseError::NumberTooLarge) })
            .map(Some)
    }

    /// Everything remaining, verbatim.
    fn rest(&mut self) -> String {
        let s: String = self.chars[self.pos..].iter().collect();
        self.pos = self.chars.len();
        s
    }
}

/// Parse the body of a command line: everything after the `AT` prefix and
/// before the termination character.
pub fn parse_body(body: &str) -> Result<Vec<Command>, ParseError> {
    let mut cur = Cursor::new(body);
    let mut out = Vec::new();

    loop {
        cur.skip_spaces();
        let Some(c) = cur.peek() else { break };

        // A stray separator between commands is harmless.
        if c == ';' {
            cur.bump();
            continue;
        }

        if c == '+' {
            cur.bump();
            out.push(parse_extended(&mut cur)?);
            cur.skip_spaces();
            // V.250 5.4.5.1: anything further needs a ';' separator.
            match cur.peek() {
                None => break,
                Some(';') => {
                    cur.bump();
                }
                Some(_) => return Err(ParseError::MissingSeparator),
            }
            continue;
        }

        if c == '&' {
            cur.bump();
            cur.skip_spaces();
            let letter = cur.bump().ok_or(ParseError::BadAmpersand)?;
            if !letter.is_ascii_alphabetic() {
                return Err(ParseError::BadAmpersand);
            }
            let number = cur.number()?;
            out.push(Command::Basic {
                amp: true,
                letter: letter.to_ascii_uppercase(),
                number,
            });
            continue;
        }

        if !c.is_ascii_alphabetic() {
            return Err(ParseError::UnexpectedChar(c));
        }
        cur.bump();
        let letter = c.to_ascii_uppercase();

        match letter {
            // V.250 6.3.1: the dial string runs to the end of the line.
            'D' => out.push(Command::Dial(cur.rest())),
            'S' => out.push(parse_s_parameter(&mut cur)?),
            _ => {
                let number = cur.number()?;
                out.push(Command::Basic { amp: false, letter, number });
            }
        }
    }
    Ok(out)
}

fn parse_s_parameter(cur: &mut Cursor<'_>) -> Result<Command, ParseError> {
    let number = cur.number()?.ok_or(ParseError::BadSParameter)?;
    let number = u8::try_from(number).map_err(|_| ParseError::BadSParameter)?;
    cur.skip_spaces();
    match cur.bump() {
        Some('?') => Ok(Command::ReadS(number)),
        // V.250 5.3.2: "If no value is given ... the S-parameter specified may
        // be set to 0, or an ERROR result code issued". We report the absence
        // and let the interpreter choose.
        Some('=') => Ok(Command::SetS(number, cur.number()?)),
        _ => Err(ParseError::MissingSOperator),
    }
}

fn parse_extended(cur: &mut Cursor<'_>) -> Result<Command, ParseError> {
    let mut name = String::new();
    // V.250 5.4.1: the first character after '+' must be alphabetic.
    match cur.peek() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return Err(ParseError::BadExtendedName),
    }
    while let Some(c) = cur.peek() {
        if is_name_char(c) {
            name.push(c.to_ascii_uppercase());
            cur.pos += 1;
        } else {
            break;
        }
    }
    // V.250 5.4.1: one to sixteen characters follow the '+'.
    if name.is_empty() || name.len() > 16 {
        return Err(ParseError::BadExtendedName);
    }

    let op = match cur.peek() {
        Some('?') => {
            cur.bump();
            ExtOp::Read
        }
        Some('=') => {
            cur.bump();
            if cur.peek() == Some('?') {
                cur.bump();
                ExtOp::Test
            } else {
                // Parameters run to the ';' separator or the end of the line.
                let mut params = String::new();
                let mut in_string = false;
                while let Some(c) = cur.peek() {
                    if c == '"' {
                        in_string = !in_string;
                    } else if c == ';' && !in_string {
                        break;
                    }
                    params.push(c);
                    cur.pos += 1;
                }
                ExtOp::Set(params.trim().to_string())
            }
        }
        _ => ExtOp::Execute,
    };
    Ok(Command::Extended { name, op })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(letter: char, number: Option<u32>) -> Command {
        Command::Basic { amp: false, letter, number }
    }

    #[test]
    fn parses_concatenated_basic_commands() {
        // V.250 5.3.1: additional commands may follow with no separator.
        assert_eq!(
            parse_body("E0Q0V1").unwrap(),
            vec![basic('E', Some(0)), basic('Q', Some(0)), basic('V', Some(1))]
        );
    }

    #[test]
    fn a_missing_number_is_reported_as_absent() {
        // The interpreter substitutes 0; the parser reports what was written.
        assert_eq!(parse_body("E").unwrap(), vec![basic('E', None)]);
    }

    #[test]
    fn leading_zeroes_are_ignored() {
        assert_eq!(parse_body("X004").unwrap(), vec![basic('X', Some(4))]);
        assert_eq!(parse_body("X000").unwrap(), vec![basic('X', Some(0))]);
    }

    #[test]
    fn lower_case_is_equivalent_to_upper() {
        assert_eq!(parse_body("e0").unwrap(), parse_body("E0").unwrap());
    }

    #[test]
    fn spaces_are_ignored_for_formatting() {
        // V.250 5.2.1.
        assert_eq!(parse_body(" E 0  Q 1 ").unwrap(),
                   vec![basic('E', Some(0)), basic('Q', Some(1))]);
    }

    #[test]
    fn parses_ampersand_commands() {
        assert_eq!(
            parse_body("&C1&D2").unwrap(),
            vec![
                Command::Basic { amp: true, letter: 'C', number: Some(1) },
                Command::Basic { amp: true, letter: 'D', number: Some(2) },
            ]
        );
    }

    #[test]
    fn parses_s_parameter_read_and_write() {
        assert_eq!(parse_body("S7?").unwrap(), vec![Command::ReadS(7)]);
        assert_eq!(parse_body("S7=60").unwrap(), vec![Command::SetS(7, Some(60))]);
        assert_eq!(parse_body("S7=").unwrap(), vec![Command::SetS(7, None)]);
    }

    #[test]
    fn s_parameter_needs_an_operator() {
        assert_eq!(parse_body("S7"), Err(ParseError::MissingSOperator));
    }

    #[test]
    fn dial_string_takes_the_rest_of_the_line() {
        // Including characters that would otherwise parse as commands.
        assert_eq!(
            parse_body("DT5551234W;E0").unwrap(),
            vec![Command::Dial("T5551234W;E0".into())]
        );
    }

    #[test]
    fn parses_the_four_extended_forms() {
        let cases = [
            ("+GMI", "GMI", ExtOp::Execute),
            ("+GMI?", "GMI", ExtOp::Read),
            ("+GMI=?", "GMI", ExtOp::Test),
            ("+MS=V34,1,2400,33600", "MS", ExtOp::Set("V34,1,2400,33600".into())),
        ];
        for (src, want_name, want_op) in cases {
            assert_eq!(
                parse_body(src).unwrap(),
                vec![Command::Extended { name: want_name.into(), op: want_op }],
                "{src}"
            );
        }
    }

    #[test]
    fn a_bare_name_swallows_following_letters() {
        // V.250 5.4.1 allows A-Z and 0-9 inside a name, and there is no
        // delimiter, so "+GMIE0" is the single name "GMIE0" rather than "+GMI"
        // followed by "E0". The interpreter rejects it as unrecognised.
        assert_eq!(
            parse_body("+GMIE0").unwrap(),
            vec![Command::Extended { name: "GMIE0".into(), op: ExtOp::Execute }]
        );
    }

    #[test]
    fn extended_commands_need_a_semicolon_before_more() {
        // V.250 5.4.5.1. The name has to end for the next command to be
        // distinguishable at all, so use a form that terminates it.
        assert_eq!(parse_body("+GMI?E0"), Err(ParseError::MissingSeparator));
        assert_eq!(parse_body("+GMI=?E0"), Err(ParseError::MissingSeparator));
        // After a Set the parameters run to ';' or end of line, so they absorb
        // anything following and this error cannot arise there.
        assert_eq!(
            parse_body("+MS=V34E0").unwrap(),
            vec![Command::Extended { name: "MS".into(), op: ExtOp::Set("V34E0".into()) }]
        );
        assert_eq!(
            parse_body("+GMI;E0").unwrap(),
            vec![
                Command::Extended { name: "GMI".into(), op: ExtOp::Execute },
                basic('E', Some(0)),
            ]
        );
    }

    #[test]
    fn extended_may_follow_basic_without_a_separator() {
        // V.250 5.4.5.2.
        assert_eq!(
            parse_body("E0+GMI").unwrap(),
            vec![
                basic('E', Some(0)),
                Command::Extended { name: "GMI".into(), op: ExtOp::Execute },
            ]
        );
    }

    #[test]
    fn semicolon_does_not_split_a_quoted_string() {
        let got = parse_body("+ASTO=\"555;1234\"").unwrap();
        assert_eq!(
            got,
            vec![Command::Extended {
                name: "ASTO".into(),
                op: ExtOp::Set("\"555;1234\"".into())
            }]
        );
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(parse_body("&"), Err(ParseError::BadAmpersand));
        assert_eq!(parse_body("&1"), Err(ParseError::BadAmpersand));
        assert_eq!(parse_body("+"), Err(ParseError::BadExtendedName));
        assert_eq!(parse_body("+1BAD"), Err(ParseError::BadExtendedName));
        assert_eq!(parse_body("?"), Err(ParseError::UnexpectedChar('?')));
        assert_eq!(parse_body("S300=1"), Err(ParseError::BadSParameter));
    }

    #[test]
    fn extended_name_length_is_bounded() {
        // V.250 5.4.1: one to sixteen characters after the '+'.
        assert!(parse_body("+ABCDEFGHIJKLMNOP").is_ok(), "16 characters is legal");
        assert_eq!(
            parse_body("+ABCDEFGHIJKLMNOPQ"),
            Err(ParseError::BadExtendedName),
            "17 characters is not"
        );
    }
}
