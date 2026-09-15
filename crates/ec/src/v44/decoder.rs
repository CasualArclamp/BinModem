//! The V.44 decoder (6.4).
//!
//! Simpler than the encoder, and the Recommendation says why: "the encoder must
//! search for string matches and create new and longer strings, whereas the
//! decoder needs only to keep track of the strings it has created."
//!
//! Two things make it more than a lookup table. The first is that a string is
//! created from the code *before* the current one (Table 2), so the decoder is
//! always one code behind the encoder in what it knows -- which leads directly
//! to the second: the encoder may send a codeword the decoder has not built
//! yet. 6.4.1 items 3 and 4 name that case and say what it must mean, and it
//! is the only place where the decoder has to reason about the encoder rather
//! than obey it.

use crate::bits::BitReader;

use super::{Error, Mode, N5, Params, command, control, length};

/// One entry of the string collection (6.2.2): "the position in the history of
/// the last character of the string, and the total length of the string".
#[derive(Debug, Clone, Copy)]
struct Str {
    end: u32,
    len: u16,
}

/// The code that went immediately before, which is what Table 2 keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prev {
    /// Nothing yet, or a dictionary that has just been started again.
    Start,
    Ordinal(u8),
    Codeword(u16),
    Extension,
}

/// One code off the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Code {
    Control(u16),
    Codeword(u16),
    Ordinal(u8),
    Extension(u16),
}

/// The receiving half.
pub struct Decoder {
    params: Params,
    mode: Mode,
    history: Vec<u8>,
    /// Entry `i` carries codeword `N5 + i`.
    strings: Vec<Str>,
    reader: BitReader,
    c2: u32,
    c5: u32,
    prev: Prev,
    /// Whether a codeword went immediately before, which is what 6.6.3 keys
    /// its prefix widths on.
    ///
    /// Not the same as `prev` being a codeword, and the difference is a real
    /// one. 6.6.3's note puts the narrow prefix "immediately after a control
    /// code, ordinal, string-extension length; or after reinitialization", so
    /// a STEPUP between a codeword and an ordinal narrows the prefix. Table 2
    /// says the opposite for string creation: "an intervening STEPUP control
    /// code does not affect string creation", and a FLUSH is to be treated as
    /// though it had not happened. So one of them forgets the codeword and the
    /// other remembers it.
    after_codeword: bool,
    /// 7.11: a STEPUP says a size has grown, and which one is settled by the
    /// code that follows it rather than by the STEPUP itself.
    stepping_up: bool,
    escape: u8,
    /// Transparent mode: whether the octet just read was the ESCAPE.
    awaiting_command: bool,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("mode", &self.mode)
            .field("c1", &self.c1())
            .field("c2", &self.c2)
            .field("history", &self.history.len())
            .finish_non_exhaustive()
    }
}

impl Decoder {
    pub fn new(params: Params) -> Self {
        Self {
            params,
            mode: Mode::Compressed,
            history: Vec::new(),
            strings: Vec::new(),
            reader: BitReader::new(),
            c2: 6,
            c5: 7,
            prev: Prev::Start,
            after_codeword: false,
            stepping_up: false,
            escape: 0,
            awaiting_command: false,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }


    /// C1, "codeword value of next available entry of ... string collection".
    fn c1(&self) -> u16 {
        N5 + self.strings.len() as u16
    }

    /// 7.5.2: the state a decoder dictionary starts in.
    fn reinitialize(&mut self) {
        self.history.clear();
        self.strings.clear();
        self.c2 = 6;
        self.c5 = 7;
        self.prev = Prev::Start;
        self.after_codeword = false;
        self.stepping_up = false;
    }

    /// Feed octets from the far end.
    pub fn decode(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
        for &byte in input {
            match self.mode {
                Mode::Transparent => self.transparent(byte, out)?,
                Mode::Compressed => {
                    self.reader.push_octet(byte);
                    self.drain(out)?;
                }
            }
        }
        Ok(())
    }

    /// One octet while transparent (6.5, 7.14).
    ///
    /// The history and the string collection are not touched here: 6.5 says
    /// "the decoder shall process the transparent characters and commands but
    /// not update the history or string collection", and the dictionary is
    /// started again on the way back into compressed mode instead.
    fn transparent(&mut self, byte: u8, out: &mut Vec<u8>) -> Result<(), Error> {
        if self.awaiting_command {
            self.awaiting_command = false;
            match byte {
                command::ECM => {
                    self.reinitialize();
                    self.mode = Mode::Compressed;
                    self.escape = 0;
                }
                command::EID => {
                    // 7.14: the ESCAPE was data. It goes out as itself, and
                    // then moves on by 51.
                    out.push(self.escape);
                    self.escape = self.escape.wrapping_add(51);
                }
                other => return Err(Error::ReservedCommand(other)),
            }
            return Ok(());
        }
        if byte == self.escape {
            self.awaiting_command = true;
            return Ok(());
        }
        out.push(byte);
        Ok(())
    }

    /// Take whole codes off the reader for as long as there are any.
    fn drain(&mut self, out: &mut Vec<u8>) -> Result<(), Error> {
        while self.mode == Mode::Compressed {
            let Some(code) = self.take_code()? else { break };
            self.apply(code, out)?;
        }
        Ok(())
    }

    /// Read one code, or nothing if all of it has not arrived (6.6.3).
    ///
    /// All-or-nothing: a code that is half here leaves the reader untouched,
    /// because the octets it needs are still on their way.
    fn take_code(&mut self) -> Result<Option<Code>, Error> {
        let mut peek = self.reader.clone();
        let Some(first) = peek.read(1) else { return Ok(None) };
        if first == 1 {
            // A control code or a codeword; which, is settled by the value.
            // 7.11.2: a STEPUP for the codeword size goes before the codeword
            // that needed it, so the growth applies to what comes after.
            let mut c2 = self.c2;
            if self.stepping_up {
                if c2 + 1 > self.params.max_code_bits() {
                    return Err(Error::CodewordTooLarge);
                }
                c2 += 1;
            }
            let Some(value) = peek.read(c2) else { return Ok(None) };
            self.reader = peek;
            self.c2 = c2;
            self.stepping_up = false;
            return Ok(Some(if value < N5 {
                Code::Control(value)
            } else {
                Code::Codeword(value)
            }));
        }
        // 6.6.3: after a codeword an ordinal takes "0" "0" and an extension
        // takes "0" "1". Anywhere else there is no extension to confuse it
        // with, so an ordinal is a bare "0".
        let extension = if self.after_codeword {
            let Some(second) = peek.read(1) else { return Ok(None) };
            second == 1
        } else {
            false
        };
        if extension {
            let Some(len) = length::read(&mut peek, self.params.n7) else {
                return Ok(None);
            };
            self.reader = peek;
            return Ok(Some(Code::Extension(len)));
        }
        // 7.11.1: a STEPUP before an ordinal is the ordinal size growing, and
        // it only ever grows once, from seven to eight.
        let mut c5 = self.c5;
        if self.stepping_up {
            if c5 + 1 > 8 {
                return Err(Error::OrdinalTooLarge);
            }
            c5 = 8;
        }
        let Some(value) = peek.read(c5) else { return Ok(None) };
        self.reader = peek;
        self.c5 = c5;
        self.stepping_up = false;
        Ok(Some(Code::Ordinal(value as u8)))
    }

    /// The characters of one string, as they sit in the history.
    fn string(&self, code: u16) -> Option<(usize, usize)> {
        let s = *self.strings.get(usize::from(code.checked_sub(N5)?))?;
        let len = usize::from(s.len);
        let end = s.end as usize;
        let start = end.checked_sub(len - 1)?;
        Some((start, len))
    }

    /// Act on one code: 6.4.1 for what it means, Table 2 for what it creates.
    fn apply(&mut self, code: Code, out: &mut Vec<u8>) -> Result<(), Error> {
        // Every control code narrows the next prefix (6.6.3's note), whatever
        // it does or does not do to string creation.
        if matches!(code, Code::Control(_)) {
            self.after_codeword = false;
            return self.control(code, out);
        }
        self.after_codeword = matches!(code, Code::Codeword(_));
        self.data(code, out)
    }

    /// The control codes of Table 8.
    fn control(&mut self, code: Code, _out: &mut [u8]) -> Result<(), Error> {
        match code {
            Code::Control(control::ETM) => {
                // 6.5.1: the encoder padded to the octet boundary before it
                // stopped, so the rest of this octet is nothing.
                self.reader.align();
                self.mode = Mode::Transparent;
                self.awaiting_command = false;
            }
            Code::Control(control::FLUSH) => {
                // 7.13: alignment only. "The receipt of the FLUSH control code
                // does not affect the creation of new strings", so `prev` is
                // deliberately left as it was.
                self.reader.align();
            }
            Code::Control(control::STEPUP) => self.stepping_up = true,
            // REINIT (7.12), and Table 8 defines nothing else.
            _ => self.reinitialize(),
        }
        Ok(())
    }

    /// Everything that is not a control code.
    fn data(&mut self, code: Code, out: &mut Vec<u8>) -> Result<(), Error> {
        // Where whatever this code produces will land.
        let at = self.history.len();
        let produced = match code {
            Code::Ordinal(c) => {
                out.push(c);
                self.history.push(c);
                1
            }
            Code::Codeword(value) if value < self.c1() => {
                let Some((start, len)) = self.string(value) else {
                    return Err(Error::UnknownCodeword(value));
                };
                self.copy(start, len, out);
                len
            }
            Code::Codeword(value) if value == self.c1() => {
                // 6.4.1 items 3 and 4: a codeword the decoder has not created
                // yet can only be the previous string followed by its own
                // first character, because that is the only string the encoder
                // could have made since.
                let (start, len) = match self.prev {
                    Prev::Codeword(k) => {
                        self.string(k).ok_or(Error::UnknownCodeword(value))?
                    }
                    // Item 4: "as if it were a string of length one".
                    Prev::Ordinal(_) => (self.history.len() - 1, 1),
                    _ => return Err(Error::UnknownCodeword(value)),
                };
                self.copy(start, len, out);
                let first = self.history[start];
                out.push(first);
                self.history.push(first);
                len + 1
            }
            Code::Codeword(value) => return Err(Error::UnknownCodeword(value)),
            Code::Extension(len) => {
                // Item 5: "use the preceding codeword to access the characters
                // in the history immediately following the string represented
                // by that codeword".
                let Prev::Codeword(k) = self.prev else {
                    return Err(Error::BadExtension);
                };
                let Some((start, was)) = self.string(k) else {
                    return Err(Error::BadExtension);
                };
                let after = start + was;
                let len = usize::from(len);
                // Only the first character has to be there already. The rest
                // may be characters this copy is about to write, which is how
                // a run of one character crosses as a codeword and a length.
                if after >= self.history.len() {
                    return Err(Error::BadExtension);
                }
                self.copy(after, len, out);
                len
            }
            Code::Control(_) => unreachable!("handled above"),
        };

        self.create(code, at, produced);
        self.prev = match code {
            Code::Ordinal(c) => Prev::Ordinal(c),
            Code::Codeword(v) => Prev::Codeword(v),
            Code::Extension(_) => Prev::Extension,
            Code::Control(_) => unreachable!("handled above"),
        };
        Ok(())
    }

    /// Copy `len` characters from `start` in the history to the output, and
    /// back onto the end of the history.
    ///
    /// Not a slice copy: 6.4.1 item 3 can ask for characters that are being
    /// written as they are read, so it goes one at a time.
    fn copy(&mut self, start: usize, len: usize, out: &mut Vec<u8>) {
        for i in 0..len {
            let c = self.history[start + i];
            out.push(c);
            self.history.push(c);
        }
    }

    /// Table 2: what string this code and the one before it make between them.
    ///
    /// `at` is where this code's characters started and `produced` is how many
    /// there were, which together give the position Table 2's entries need.
    fn create(&mut self, code: Code, at: usize, produced: usize) {
        if self.c1() >= self.params.n2 {
            // The far end's node-tree is full too, and it will send a REINIT.
            return;
        }
        let made = match (code, self.prev) {
            // "The corresponding character is appended to the previous
            // character to create a 2-character string."
            (Code::Ordinal(_), Prev::Ordinal(_)) => Some(Str { end: at as u32, len: 2 }),
            // "...appended to the previous string to create a longer string."
            (Code::Ordinal(_), Prev::Codeword(k)) => {
                self.longer(k, at as u32, 1)
            }
            // "The first character of the codeword's string is appended to the
            // previous character to create a 2-character string."
            (Code::Codeword(_), Prev::Ordinal(_)) => Some(Str { end: at as u32, len: 2 }),
            (Code::Codeword(_), Prev::Codeword(k)) => self.longer(k, at as u32, 1),
            // "The previous string is extended to create a new and longer
            // string."
            (Code::Extension(_), Prev::Codeword(k)) => {
                self.longer(k, (at + produced - 1) as u32, produced as u16)
            }
            // Everything else in Table 2 creates nothing: after an extension,
            // after a reinitialisation, and at the very start.
            _ => None,
        };
        if let Some(s) = made
            && s.len <= u16::from(self.params.n7)
        {
            // The note under Table 2: "strings of length greater than N7R
            // shall not be created".
            self.strings.push(s);
        }
    }

    /// The previous string with `more` characters after it, ending at `end`.
    fn longer(&self, previous: u16, end: u32, more: u16) -> Option<Str> {
        let s = self.strings.get(usize::from(previous.checked_sub(N5)?))?;
        Some(Str { end, len: s.len + more })
    }
}
