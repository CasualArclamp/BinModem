//! The V.42bis encoder and decoder (clauses 7, 8 and 9).

use super::bits::{BitReader, BitWriter};
use super::dictionary::{Dictionary, ECM, EID, ETM, FLUSH, N4, Params, RESET, STEPUP};

/// Transparent or compressed operation (V.42bis 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Transparent,
    Compressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A STEPUP would take the codeword size past N1 (V.42bis 5.8 a).
    CodewordTooLarge,
    /// A codeword arrived that names no dictionary entry, so the two
    /// dictionaries have diverged.
    UnknownCodeword(u16),
    /// A command code that V.42bis Table 2 leaves reserved.
    ReservedCommand(u8),
}

/// How often compressibility is reconsidered, in characters.
///
/// V.42bis 7.8 requires the test but explicitly does not specify it: "the
/// nature of the test is not specified in this Recommendation". The window and
/// thresholds below are therefore ours, not the Recommendation's.
const TEST_WINDOW: u32 = 256;

/// Switch to compressed once the estimate is this fraction of transparent cost.
const COMPRESS_AT_PERCENT: u32 = 90;

struct Common {
    dict: Dictionary,
    mode: Mode,
    /// C2, current codeword size in bits.
    c2: u32,
    /// C3, threshold at which the codeword size grows.
    c3: u32,
    max_bits: u32,
    escape: u8,
}

impl Common {
    fn new(params: Params) -> Self {
        Self {
            dict: Dictionary::new(params),
            // V.42bis 7.2: transparent mode, C2 = N3 + 1, C3 = N4 * 2, escape 0.
            mode: Mode::Transparent,
            c2: 9,
            c3: u32::from(N4) * 2,
            max_bits: params.max_code_bits(),
            escape: 0,
        }
    }

    fn reset(&mut self) {
        self.dict.reset();
        self.mode = Mode::Transparent;
        self.c2 = 9;
        self.c3 = u32::from(N4) * 2;
        self.escape = 0;
    }
}

/// Compresses a character stream.
pub struct Encoder {
    inner: Common,
    writer: BitWriter,
    /// Codeword of the string matched so far.
    matched: Option<u16>,
    /// A codeword already sent whose dictionary entry has not been made yet.
    ///
    /// Only a flush leaves one: the string ended because the terminal stopped
    /// talking rather than because a character failed to extend it, so what to
    /// extend it *by* is not known until the next character arrives. The
    /// decoder is in the same position and calls it `previous`.
    owed: Option<u16>,
    /// The entry created by the last match, which V.42bis 6.3 b) forbids
    /// extending into. This is what keeps the decoder from ever meeting a
    /// codeword it has not yet built.
    last_added: Option<u16>,
    chars_in: u32,
    codewords_out: u32,
    bits_out: u32,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("mode", &self.inner.mode)
            .field("c2", &self.inner.c2)
            .finish_non_exhaustive()
    }
}

impl Encoder {
    pub fn new(params: Params) -> Self {
        Self {
            inner: Common::new(params),
            writer: BitWriter::new(),
            matched: None,
            owed: None,
            last_added: None,
            chars_in: 0,
            codewords_out: 0,
            bits_out: 0,
        }
    }

    pub fn mode(&self) -> Mode {
        self.inner.mode
    }

    /// Compress `input`, appending to `out`.
    pub fn encode(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &c in input {
            self.feed(c, out);
        }
    }

    fn feed(&mut self, c: u8, out: &mut Vec<u8>) {
        self.chars_in += 1;

        // V.42bis 9.2: the escape character appearing in data is announced in
        // transparent mode and, in either mode, moves on afterwards.
        let is_escape = c == self.inner.escape;
        if self.inner.mode == Mode::Transparent {
            out.push(c);
            if is_escape {
                out.push(EID);
            }
        }
        if is_escape {
            self.inner.escape = self.inner.escape.wrapping_add(51);
        }

        // String matching (V.42bis 6.3).
        let Some(current) = self.matched else {
            // A codeword was sent for a string that ended at a flush, and the
            // dictionary entry it owes is the one the decoder is about to make
            // from it: clause 8's "previous string extended by this one's
            // first character". Skipping it here is how the two ends came to
            // disagree about what every codeword above 259 meant.
            if let Some(previous) = self.owed.take() {
                self.last_added = self.inner.dict.add(previous, c);
            }
            self.matched = Some(Dictionary::root_code(c));
            return;
        };
        let extension = self
            .inner
            .dict
            .find_child(current, c)
            .filter(|next| Some(*next) != self.last_added);
        if let Some(next) = extension {
            self.matched = Some(next);
            return;
        }

        // The match ends here: emit it, extend the dictionary, restart.
        if self.inner.mode == Mode::Compressed {
            self.emit(current, out);
        }
        self.codewords_out += 1;
        self.last_added = self.inner.dict.add(current, c);
        self.matched = Some(Dictionary::root_code(c));
        self.consider_mode(out);
    }

    /// Encode one codeword, growing the codeword size first if needed
    /// (V.42bis 7.4).
    fn emit(&mut self, code: u16, out: &mut Vec<u8>) {
        while u32::from(code) >= self.inner.c3 && self.inner.c2 < self.inner.max_bits {
            self.write(STEPUP, out);
            self.inner.c2 += 1;
            self.inner.c3 *= 2;
        }
        self.write(code, out);
    }

    fn write(&mut self, code: u16, out: &mut Vec<u8>) {
        self.writer.write(code, self.inner.c2, out);
        self.bits_out += self.inner.c2;
    }

    /// The compressibility test (V.42bis 7.8). The Recommendation requires one
    /// and deliberately leaves its nature open, so this heuristic is ours.
    fn consider_mode(&mut self, out: &mut Vec<u8>) {
        if self.chars_in < TEST_WINDOW {
            return;
        }
        let transparent_bits = self.chars_in * 8;
        match self.inner.mode {
            Mode::Transparent => {
                // Estimate what those characters would have cost as codewords.
                let estimate = self.codewords_out * self.inner.c2;
                if estimate * 100 < transparent_bits * COMPRESS_AT_PERCENT {
                    self.enter_compressed(out);
                }
            }
            Mode::Compressed => {
                if self.bits_out >= transparent_bits {
                    self.enter_transparent(out);
                }
            }
        }
        self.chars_in = 0;
        self.codewords_out = 0;
        self.bits_out = 0;
    }

    /// V.42bis 7.8.1.
    ///
    /// a) is the whole of it, and skipping it is invisible until a real modem
    /// is on the other end: "perform the dictionary update procedure using the
    /// current accumulated string and the next character to be processed by
    /// the string matching procedure (which will be the first character of the
    /// string represented by the first codeword transmitted in compressed
    /// mode)". One entry, added at the transition and at no other time.
    ///
    /// This used to throw the accumulated string away instead, on the grounds
    /// that the switch was taken at a match boundary and both ends would stay
    /// in step. Both ends did -- these two ends. Every dictionary this end
    /// built after the switch was one entry short of the one a far end built,
    /// so every codeword above that point named the wrong string, and a real
    /// board's text came out as itself with the letters moved about: "If you
    /// ar tnot he tn for int fne haccess,tory".
    ///
    /// The character is not known yet, so the update is owed rather than made,
    /// which is what `owed` is already for.
    fn enter_compressed(&mut self, out: &mut Vec<u8>) {
        out.push(self.inner.escape);
        out.push(ECM);
        self.inner.mode = Mode::Compressed;
        self.owed = self.matched;
        self.matched = None;
        self.last_added = None;
    }

    /// V.42bis 7.8.2.
    fn enter_transparent(&mut self, out: &mut Vec<u8>) {
        self.emit_pending(out);
        self.write(ETM, out);
        self.writer.align(out);
        self.inner.mode = Mode::Transparent;
        self.matched = None;
        self.owed = None;
        self.last_added = None;
    }

    fn emit_pending(&mut self, out: &mut Vec<u8>) {
        if let Some(current) = self.matched.take()
            && self.inner.mode == Mode::Compressed
        {
            self.emit(current, out);
        }
    }

    /// Send everything outstanding (V.42bis 7.9).
    ///
    /// A flush is about getting bits onto the line, not about forgetting
    /// anything. The dictionary is a history both ends build from the same
    /// characters, and the decoder is told nothing by a flush that would let
    /// it drop its own -- so an encoder that dropped its context here would
    /// walk away from a shared state the far end still holds.
    ///
    /// Which it did. Every other test fed a whole payload in one call; a modem
    /// hands over whatever the terminal typed, whenever it typed it, and
    /// flushes each time so that an echo is not held back waiting for a better
    /// match. A few characters at a time, the two dictionaries came apart
    /// within a couple of thousand bytes and what arrived was fragments of the
    /// right text in the wrong order.
    pub fn flush(&mut self, out: &mut Vec<u8>) {
        if self.inner.mode != Mode::Compressed {
            // Transparent mode has already put every character on the line,
            // and its matching state is the decoder's too.
            return;
        }
        if let Some(current) = self.matched.take() {
            self.emit(current, out);
            // The dictionary entry this string owes, made when the next
            // character arrives to say what to extend it by.
            self.owed = Some(current);
        }
        if self.writer.pending() > 0 {
            // A partial octet would otherwise sit unsent; FLUSH lets the
            // decoder discard the padding that follows.
            self.write(FLUSH, out);
            self.writer.align(out);
        }
    }

    /// Re-initialise and tell the peer (V.42bis 7.8.3).
    pub fn reset(&mut self, out: &mut Vec<u8>) {
        if self.inner.mode == Mode::Compressed {
            self.enter_transparent(out);
        }
        out.push(self.inner.escape);
        out.push(RESET);
        self.inner.reset();
        self.matched = None;
        self.owed = None;
        self.last_added = None;
        self.chars_in = 0;
        self.codewords_out = 0;
        self.bits_out = 0;
    }
}

/// Decompresses a character stream.
pub struct Decoder {
    inner: Common,
    reader: BitReader,
    /// Codeword decoded on the previous step, which the next entry extends.
    previous: Option<u16>,
    last_added: Option<u16>,
    /// Set after the escape character, while awaiting a command code.
    awaiting_command: bool,
    /// Transparent-mode match state, kept so the dictionary tracks the encoder.
    matched: Option<u16>,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("mode", &self.inner.mode)
            .field("c2", &self.inner.c2)
            .finish_non_exhaustive()
    }
}

impl Decoder {
    pub fn new(params: Params) -> Self {
        Self {
            inner: Common::new(params),
            reader: BitReader::new(),
            previous: None,
            last_added: None,
            awaiting_command: false,
            matched: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.inner.mode
    }

    /// Decompress `input`, appending to `out`.
    pub fn decode(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
        for &byte in input {
            match self.inner.mode {
                Mode::Transparent => self.transparent_octet(byte, out)?,
                Mode::Compressed => {
                    self.reader.push_octet(byte);
                    self.drain_codewords(out)?;
                }
            }
        }
        Ok(())
    }

    fn transparent_octet(&mut self, byte: u8, out: &mut Vec<u8>) -> Result<(), Error> {
        if self.awaiting_command {
            self.awaiting_command = false;
            match byte {
                ECM => {
                    self.inner.mode = Mode::Compressed;
                    // 7.8.1 a), from this side. The far end has just extended
                    // its accumulated string by the first character of the
                    // string its first codeword names -- so carrying that
                    // string over as the previous one is the same update, made
                    // when the character arrives rather than before.
                    self.previous = self.matched;
                    self.last_added = None;
                    self.matched = None;
                }
                EID => {
                    // The escape character was literal data (V.42bis 9.2).
                    let literal = self.inner.escape;
                    out.push(literal);
                    self.match_step(literal);
                    self.inner.escape = self.inner.escape.wrapping_add(51);
                }
                RESET => {
                    self.inner.reset();
                    self.previous = None;
                    self.last_added = None;
                    self.matched = None;
                }
                other => return Err(Error::ReservedCommand(other)),
            }
            return Ok(());
        }
        if byte == self.inner.escape {
            // Hold it back: only the command code that follows says whether it
            // was data or the start of a sequence.
            self.awaiting_command = true;
            return Ok(());
        }
        out.push(byte);
        self.match_step(byte);
        Ok(())
    }

    /// Maintain the dictionary while transparent, so it matches the peer's
    /// encoder dictionary when compressed mode resumes (V.42bis clause 8).
    fn match_step(&mut self, c: u8) {
        let Some(current) = self.matched else {
            self.matched = Some(Dictionary::root_code(c));
            return;
        };
        let extension = self
            .inner
            .dict
            .find_child(current, c)
            .filter(|next| Some(*next) != self.last_added);
        if let Some(next) = extension {
            self.matched = Some(next);
            return;
        }
        self.last_added = self.inner.dict.add(current, c);
        self.matched = Some(Dictionary::root_code(c));
    }

    fn drain_codewords(&mut self, out: &mut Vec<u8>) -> Result<(), Error> {
        while self.inner.mode == Mode::Compressed && self.reader.available() >= self.inner.c2 {
            let Some(code) = self.reader.read(self.inner.c2) else { break };
            match code {
                STEPUP => {
                    if self.inner.c2 + 1 > self.inner.max_bits {
                        return Err(Error::CodewordTooLarge);
                    }
                    self.inner.c2 += 1;
                    self.inner.c3 *= 2;
                }
                ETM => {
                    self.reader.align();
                    self.inner.mode = Mode::Transparent;
                    self.previous = None;
                    self.last_added = None;
                    self.matched = None;
                }
                FLUSH => self.reader.align(),
                _ => self.decode_string(code, out)?,
            }
        }
        Ok(())
    }

    fn decode_string(&mut self, code: u16, out: &mut Vec<u8>) -> Result<(), Error> {
        // V.42bis 6.3 b) stops the encoder emitting a codeword it has only just
        // created, so an unknown codeword means the dictionaries have diverged
        // rather than the usual Lempel-Ziv self-reference.
        if !self.inner.dict.in_use(code) {
            return Err(Error::UnknownCodeword(code));
        }
        let string = self.inner.dict.string(code);
        out.extend_from_slice(&string);

        // The new entry is the previous string extended by this one's first
        // character (V.42bis clause 8).
        if let Some(previous) = self.previous
            && let Some(&first) = string.first()
        {
            self.last_added = self.inner.dict.add(previous, first);
        }
        self.previous = Some(code);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8]) -> Vec<u8> {
        round_trip_with(input, Params::default())
    }

    fn round_trip_with(input: &[u8], params: Params) -> Vec<u8> {
        let mut enc = Encoder::new(params);
        let mut wire = Vec::new();
        enc.encode(input, &mut wire);
        enc.flush(&mut wire);

        let mut dec = Decoder::new(params);
        let mut out = Vec::new();
        dec.decode(&wire, &mut out).expect("decode failed");
        out
    }

    fn compressed_size(input: &[u8]) -> usize {
        let mut enc = Encoder::new(Params::default());
        let mut wire = Vec::new();
        enc.encode(input, &mut wire);
        enc.flush(&mut wire);
        wire.len()
    }

    #[test]
    fn short_text_round_trips() {
        let text = b"Welcome to the board.";
        assert_eq!(round_trip(text), text);
    }

    #[test]
    fn every_byte_value_round_trips() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(round_trip(&all), all);
    }

    #[test]
    fn an_empty_input_produces_nothing() {
        assert!(round_trip(b"").is_empty());
    }

    #[test]
    fn the_escape_character_in_data_round_trips() {
        // The escape starts at 0 and moves by 51 each time it appears, so a
        // stream full of those values exercises the mechanism repeatedly.
        let mut data = vec![0u8];
        let mut escape = 0u8;
        for _ in 0..40 {
            escape = escape.wrapping_add(51);
            data.push(escape);
            data.extend_from_slice(b"filler");
        }
        assert_eq!(round_trip(&data), data);
    }

    #[test]
    fn repetitive_data_round_trips_and_compresses() {
        let input: Vec<u8> = b"ABCABCABCABC".iter().copied().cycle().take(20_000).collect();
        assert_eq!(round_trip(&input), input);
        let size = compressed_size(&input);
        assert!(
            size < input.len() / 2,
            "20000 bytes of a repeating pattern compressed to only {size}"
        );
    }

    #[test]
    fn english_like_text_round_trips_and_compresses() {
        let line = b"The quick brown fox jumps over the lazy dog. ";
        let input: Vec<u8> = line.iter().copied().cycle().take(30_000).collect();
        assert_eq!(round_trip(&input), input);
        assert!(compressed_size(&input) < input.len() / 3);
    }

    #[test]
    fn incompressible_data_does_not_expand_much() {
        // The mode switch exists so that random data is passed through rather
        // than inflated. A little overhead is expected, runaway growth is not.
        let input: Vec<u8> = (0..20_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        assert_eq!(round_trip(&input), input);
        let size = compressed_size(&input);
        assert!(
            size < input.len() * 11 / 10,
            "random data grew from {} to {size}",
            input.len()
        );
    }

    #[test]
    fn the_encoder_reaches_compressed_mode_on_compressible_data() {
        let mut enc = Encoder::new(Params::default());
        let mut wire = Vec::new();
        let input: Vec<u8> = b"abcabcabc".iter().copied().cycle().take(10_000).collect();
        enc.encode(&input, &mut wire);
        assert_eq!(enc.mode(), Mode::Compressed);
    }

    #[test]
    fn the_codeword_size_grows_and_the_decoder_follows() {
        // Enough distinct strings to push past 512 codewords and force STEPUP.
        let params = Params { n2: 2048, n7: 32 };
        let input: Vec<u8> = (0..40_000u32).map(|i| (i % 97) as u8).collect();
        assert_eq!(round_trip_with(&input, params), input);
    }

    #[test]
    fn data_split_across_calls_decodes_the_same() {
        let input: Vec<u8> = b"the same data, arriving in pieces. "
            .iter()
            .copied()
            .cycle()
            .take(9_000)
            .collect();

        let mut enc = Encoder::new(Params::default());
        let mut wire = Vec::new();
        for chunk in input.chunks(7) {
            enc.encode(chunk, &mut wire);
        }
        enc.flush(&mut wire);

        let mut dec = Decoder::new(Params::default());
        let mut out = Vec::new();
        for chunk in wire.chunks(5) {
            dec.decode(chunk, &mut out).unwrap();
        }
        assert_eq!(out, input);
    }

    #[test]
    fn a_reset_returns_both_ends_to_the_initial_state() {
        let params = Params::default();
        let mut enc = Encoder::new(params);
        let mut dec = Decoder::new(params);
        let mut wire = Vec::new();
        let mut out = Vec::new();

        let first: Vec<u8> = b"first stretch of data ".iter().copied().cycle().take(5_000).collect();
        enc.encode(&first, &mut wire);
        enc.flush(&mut wire);
        enc.reset(&mut wire);
        let second = b"after the reset".to_vec();
        enc.encode(&second, &mut wire);
        enc.flush(&mut wire);

        dec.decode(&wire, &mut out).unwrap();
        assert_eq!(dec.mode(), Mode::Transparent);
        let mut expected = first.clone();
        expected.extend_from_slice(&second);
        assert_eq!(out, expected);
    }

    #[test]
    fn a_long_run_of_one_byte_round_trips() {
        // Maximum string length caps how far a run can be absorbed, so this
        // exercises the N7 limit repeatedly.
        let input = vec![b'z'; 10_000];
        assert_eq!(round_trip(&input), input);
    }

    #[test]
    fn larger_dictionaries_round_trip() {
        for n2 in [512u16, 1024, 2048, 4096] {
            let params = Params { n2, n7: 16 };
            let input: Vec<u8> = b"mixed content 12345 "
                .iter()
                .copied()
                .cycle()
                .take(25_000)
                .collect();
            assert_eq!(round_trip_with(&input, params), input, "N2 = {n2}");
        }
    }

    #[test]
    fn a_reserved_command_code_is_reported() {
        let mut dec = Decoder::new(Params::default());
        let mut out = Vec::new();
        // Escape starts at 0, so 0 followed by a reserved code.
        assert_eq!(
            dec.decode(&[0, 200], &mut out),
            Err(Error::ReservedCommand(200))
        );
    }
}

/// What a real V.42bis encoder put on a real line.
///
/// Every other test here runs this encoder into this decoder, which proves
/// they agree and cannot prove they are right: for years they agreed on
/// skipping 7.8.1 a), and a pair that is wrong the same way is a pair that
/// passes. This is the other kind of evidence -- 260 octets off
/// `live-1788830261.wav`, the information fields of the first LAPM frames a
/// board sent, in the order they arrived.
///
/// It opens in transparent mode, which is why the banner is legible in the
/// bytes; `00 00` is the escape character and ECM (5.3 e, 9.1) and everything
/// after it is codewords. Before 7.8.1 a) was implemented this decoded to
/// "If you ar tnot he tn for int fne haccess,tory" -- the same letters, moved
/// about, because every dictionary entry above the switch was off by one.
#[cfg(test)]
mod real_encoder {
    use super::*;

    const FROM_THE_LINE: &[u8] = &[
    0x0d, 0x41, 0x72, 0x6d, 0x62, 0x69, 0x61, 0x6e, 0x20, 0x32, 0x33, 0x2e, 0x35, 0x2e, 0x31, 0x20,
    0x42, 0x6f, 0x6f, 0x6b, 0x77, 0x6f, 0x72, 0x6d, 0x20, 0x6c, 0x20, 0x0a, 0x0d, 0x0a, 0x0d, 0x2a,
    0x2a, 0x45, 0x4d, 0x53, 0x49, 0x5f, 0x52, 0x45, 0x51, 0x41, 0x37, 0x37, 0x45, 0x0d, 0x11, 0x20,
    0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x0d, 0x0d, 0x0a,
    0x00, 0x00, 0x5a, 0xd0, 0xbc, 0x31, 0x23, 0x07, 0x0e, 0x9a, 0x11, 0x77, 0xe4, 0x8c, 0x38, 0x83,
    0xf0, 0x0d, 0x9e, 0x39, 0x31, 0x2e, 0xd6, 0x79, 0x13, 0x00, 0x67, 0x48, 0x8c, 0xf0, 0xc8, 0x24,
    0xcd, 0x08, 0x3e, 0x72, 0xf0, 0x8c, 0x20, 0x53, 0x87, 0x67, 0x1c, 0x39, 0x77, 0x46, 0xac, 0x41,
    0x63, 0x75, 0x44, 0x1a, 0x8c, 0x23, 0xd8, 0xc4, 0xb9, 0xd3, 0x35, 0x0e, 0x1a, 0xad, 0x64, 0xcc,
    0x98, 0x41, 0x63, 0xc7, 0xce, 0x8b, 0x9e, 0x75, 0xf8, 0xf4, 0x18, 0xe1, 0x65, 0xce, 0x1a, 0xab,
    0x64, 0xdc, 0x80, 0xa9, 0x0b, 0xb6, 0x4f, 0x9f, 0x00, 0x7c, 0xbd, 0xf4, 0xc1, 0xb8, 0xb7, 0xef,
    0x58, 0x32, 0x6f, 0x06, 0x9f, 0x0d, 0x6c, 0xd5, 0x0c, 0x19, 0x38, 0x73, 0xf8, 0x86, 0xf5, 0x62,
    0x07, 0x0e, 0x9b, 0x3a, 0x69, 0xc0, 0x78, 0x7c, 0x23, 0x47, 0xcd, 0xd8, 0x1e, 0x1e, 0x07, 0x16,
    0x3c, 0x98, 0x70, 0x61, 0xc3, 0x87, 0x11, 0x27, 0x56, 0x3c, 0x0a, 0x47, 0x23, 0xc7, 0xd0, 0x20,
    0x45, 0x92, 0x34, 0x89, 0x52, 0x25, 0x4b, 0x97, 0x32, 0x65, 0xe6, 0x66, 0x5a, 0xf3, 0x66, 0xce,
    0x9d, 0x3d, 0x7f, 0x06, 0x0d, 0x00, 0x88, 0x89, 0x1a, 0xc5, 0x98, 0x74, 0x69, 0xd3, 0xa7, 0x51,
    0xa7, 0x56, 0xbd, 0x9a, 0x75, 0x6b, 0x57, 0x9e, 0x60, 0xeb, 0x88, 0x25, 0x6b, 0x16, 0x2d, 0xd5,
    0xb5, 0x6d, 0xdf, 0xc6,
    ];

    #[test]
    fn a_real_encoders_stream_decodes_to_what_it_said() {
        let mut decoder = Decoder::new(Params { n2: 2048, n7: 250 });
        let mut out = Vec::new();
        decoder
            .decode(FROM_THE_LINE, &mut out)
            .expect("a real encoder's stream would not decode");
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("Welcome to dialup.world!"),
            "the transparent part is wrong, which would be a different fault: {text:?}",
        );
        assert!(
            text.contains("If you are not here for internet access"),
            "the compressed part came out as {text:?}",
        );
    }
}
