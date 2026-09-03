//! HDLC framing for LAPM (V.42 8.1).
//!
//! Frames are delimited by the flag `01111110`, and transparency is maintained
//! by inserting a zero after any five contiguous ones (V.42 8.1.1.2). Bits go
//! out low-order first within each octet (V.42 8.1.2.2), which is why the CRC
//! below is the reflected form.

use std::collections::VecDeque;

/// The flag sequence, `01111110` (V.42 8.1.1.2).
pub const FLAG: u8 = 0x7e;

/// Which frame check sequence is in use (V.42 8.1.1.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fcs {
    #[default]
    Bits16,
    Bits32,
}

impl Fcs {
    pub fn octets(self) -> usize {
        match self {
            Self::Bits16 => 2,
            Self::Bits32 => 4,
        }
    }
}

/// CRC-16 over the generator `x^16 + x^12 + x^5 + 1` (V.42 8.1.1.6.1).
///
/// Reflected, because transmission is low-order bit first. Preset to all ones;
/// the value transmitted is the ones complement of the remainder.
#[derive(Debug, Clone, Copy)]
pub struct Crc16(u16);

impl Default for Crc16 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc16 {
    /// The residue a receiver sees over a good frame including its FCS.
    ///
    /// V.42 gives this as `0001 1101 0000 1111` reading x15 down to x0, which
    /// is this value once expressed in the reflected convention.
    pub const GOOD: u16 = 0xf0b8;

    pub fn new() -> Self {
        Self(0xffff)
    }

    pub fn update(&mut self, byte: u8) {
        self.0 ^= u16::from(byte);
        for _ in 0..8 {
            self.0 = if self.0 & 1 != 0 {
                (self.0 >> 1) ^ 0x8408 // reflected 0x1021
            } else {
                self.0 >> 1
            };
        }
    }

    pub fn update_all(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.update(b);
        }
    }

    /// Raw register, for the receiver's residue check.
    pub fn residue(&self) -> u16 {
        self.0
    }

    /// The two octets to transmit, low-order first.
    pub fn to_bytes(&self) -> [u8; 2] {
        let v = !self.0;
        [(v & 0xff) as u8, (v >> 8) as u8]
    }
}

/// CRC-32 for the optional 32-bit FCS (V.42 8.1.1.6.2).
#[derive(Debug, Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub const GOOD: u32 = 0xdebb20e3;

    pub fn new() -> Self {
        Self(0xffff_ffff)
    }

    pub fn update(&mut self, byte: u8) {
        self.0 ^= u32::from(byte);
        for _ in 0..8 {
            self.0 = if self.0 & 1 != 0 {
                (self.0 >> 1) ^ 0xedb8_8320
            } else {
                self.0 >> 1
            };
        }
    }

    pub fn update_all(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.update(b);
        }
    }

    pub fn residue(&self) -> u32 {
        self.0
    }

    pub fn to_bytes(&self) -> [u8; 4] {
        let v = !self.0;
        [
            (v & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            ((v >> 16) & 0xff) as u8,
            ((v >> 24) & 0xff) as u8,
        ]
    }
}

/// Append the frame check sequence for `payload`.
pub fn append_fcs(payload: &[u8], fcs: Fcs) -> Vec<u8> {
    let mut out = payload.to_vec();
    match fcs {
        Fcs::Bits16 => {
            let mut c = Crc16::new();
            c.update_all(payload);
            out.extend_from_slice(&c.to_bytes());
        }
        Fcs::Bits32 => {
            let mut c = Crc32::new();
            c.update_all(payload);
            out.extend_from_slice(&c.to_bytes());
        }
    }
    out
}

/// True when `frame` (payload plus its FCS) checks out.
pub fn fcs_ok(frame: &[u8], fcs: Fcs) -> bool {
    if frame.len() <= fcs.octets() {
        return false;
    }
    match fcs {
        Fcs::Bits16 => {
            let mut c = Crc16::new();
            c.update_all(frame);
            c.residue() == Crc16::GOOD
        }
        Fcs::Bits32 => {
            let mut c = Crc32::new();
            c.update_all(frame);
            c.residue() == Crc32::GOOD
        }
    }
}

/// Turns frames into a stuffed bit stream.
#[derive(Debug)]
pub struct Encoder {
    fcs: Fcs,
    bits: VecDeque<bool>,
}

impl Encoder {
    pub fn new(fcs: Fcs) -> Self {
        Self { fcs, bits: VecDeque::new() }
    }

    /// Queue one frame: opening flag, stuffed payload and FCS, closing flag.
    pub fn frame(&mut self, payload: &[u8]) {
        self.raw_flag();
        let body = append_fcs(payload, self.fcs);
        let mut ones = 0u32;
        for &byte in &body {
            for i in 0..8 {
                let bit = byte & (1 << i) != 0; // low-order bit first
                self.bits.push_back(bit);
                if bit {
                    ones += 1;
                    if ones == 5 {
                        // Transparency: a zero after five contiguous ones.
                        self.bits.push_back(false);
                        ones = 0;
                    }
                } else {
                    ones = 0;
                }
            }
        }
        self.raw_flag();
    }

    /// Queue `n` flags as interframe time fill (V.42 8.1.5).
    pub fn idle(&mut self, n: usize) {
        for _ in 0..n {
            self.raw_flag();
        }
    }

    /// Queue an abort: at least seven contiguous ones (V.42 8.1.4).
    pub fn abort(&mut self) {
        for _ in 0..8 {
            self.bits.push_back(true);
        }
    }

    fn raw_flag(&mut self) {
        for i in 0..8 {
            self.bits.push_back(FLAG & (1 << i) != 0);
        }
    }

    pub fn next_bit(&mut self) -> Option<bool> {
        self.bits.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    pub fn pending_bits(&self) -> usize {
        self.bits.len()
    }
}

/// Why a received frame was discarded (V.42 8.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer octets than an address, control and FCS require.
    TooShort,
    /// Not an integral number of octets.
    NotOctetAligned,
    /// The frame check sequence did not verify.
    BadFcs,
    /// Seven or more contiguous ones arrived mid-frame.
    Aborted,
    /// Longer than the configured maximum.
    TooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Looking for an opening flag.
    Hunt,
    /// Collecting frame content.
    Frame,
    /// Five ones then a sixth: a flag or an abort, decided by the next bit.
    Terminator,
}

/// Turns a stuffed bit stream back into frames.
#[derive(Debug)]
pub struct Decoder {
    fcs: Fcs,
    state: State,
    history: u8,
    bits: Vec<bool>,
    ones: u32,
    max_octets: usize,
    overlong: bool,
}

impl Decoder {
    pub fn new(fcs: Fcs) -> Self {
        Self {
            fcs,
            state: State::Hunt,
            history: 0,
            bits: Vec::new(),
            ones: 0,
            // N401 defaults to 128 information octets; allow generous headroom
            // for the header, FCS and a negotiated larger frame size.
            max_octets: 2048,
            overlong: false,
        }
    }

    pub fn with_max_octets(mut self, max: usize) -> Self {
        self.max_octets = max;
        self
    }

    /// Feed one received bit. Yields a frame when one completes.
    pub fn feed(&mut self, bit: bool) -> Option<Result<Vec<u8>, FrameError>> {
        match self.state {
            State::Hunt => {
                // Shift low-order first, so a completed flag reads as 0x7E.
                self.history = (self.history >> 1) | if bit { 0x80 } else { 0 };
                if self.history == FLAG {
                    self.enter_frame();
                }
                None
            }
            State::Frame => {
                if self.ones == 5 {
                    self.ones = 0;
                    if !bit {
                        // A stuffed zero: discard it and carry on.
                        return None;
                    }
                    // A sixth one. Flag or abort, decided by the next bit.
                    self.state = State::Terminator;
                    return None;
                }
                if bit {
                    self.ones += 1;
                } else {
                    self.ones = 0;
                }
                if self.bits.len() >= self.max_octets * 8 {
                    // Keep consuming until the delimiter, but remember to fail.
                    self.overlong = true;
                } else {
                    self.bits.push(bit);
                }
                None
            }
            State::Terminator => {
                if bit {
                    // Seven ones: an abort (V.42 8.1.4).
                    let had_content = !self.bits.is_empty();
                    self.reset_to_hunt();
                    return had_content.then_some(Err(FrameError::Aborted));
                }
                // A complete flag. The six bits it already contributed to the
                // buffer -- its leading zero and five ones -- are not frame
                // content, so drop them.
                let keep = self.bits.len().saturating_sub(6);
                let bits = self.bits[..keep].to_vec();
                let overlong = self.overlong;
                // A closing flag may open the next frame (V.42 8.1.1.2).
                self.enter_frame();
                Self::finish(bits, self.fcs, overlong)
            }
        }
    }

    pub fn feed_bytes_lsb_first(&mut self, bytes: &[u8]) -> Vec<Result<Vec<u8>, FrameError>> {
        let mut out = Vec::new();
        for &byte in bytes {
            for i in 0..8 {
                if let Some(r) = self.feed(byte & (1 << i) != 0) {
                    out.push(r);
                }
            }
        }
        out
    }

    fn enter_frame(&mut self) {
        self.state = State::Frame;
        self.bits.clear();
        self.ones = 0;
        self.overlong = false;
    }

    fn reset_to_hunt(&mut self) {
        self.state = State::Hunt;
        self.history = 0;
        self.bits.clear();
        self.ones = 0;
        self.overlong = false;
    }

    fn finish(bits: Vec<bool>, fcs: Fcs, overlong: bool) -> Option<Result<Vec<u8>, FrameError>> {
        // Back-to-back flags leave nothing between them; that is idle fill, not
        // a frame, so say nothing rather than reporting an error.
        if bits.is_empty() {
            return None;
        }
        if overlong {
            return Some(Err(FrameError::TooLong));
        }
        if !bits.len().is_multiple_of(8) {
            return Some(Err(FrameError::NotOctetAligned));
        }
        let frame: Vec<u8> = bits
            .as_chunks::<8>().0.iter()
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, &b)| acc | (u8::from(b) << i))
            })
            .collect();
        // An address octet, a control octet and the FCS are the minimum.
        if frame.len() < 2 + fcs.octets() {
            return Some(Err(FrameError::TooShort));
        }
        if !fcs_ok(&frame, fcs) {
            return Some(Err(FrameError::BadFcs));
        }
        let keep = frame.len() - fcs.octets();
        Some(Ok(frame[..keep].to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a payload through the encoder and back through the decoder.
    fn round_trip(payload: &[u8], fcs: Fcs) -> Vec<Result<Vec<u8>, FrameError>> {
        let mut enc = Encoder::new(fcs);
        enc.frame(payload);
        let mut dec = Decoder::new(fcs);
        let mut out = Vec::new();
        while let Some(bit) = enc.next_bit() {
            if let Some(r) = dec.feed(bit) {
                out.push(r);
            }
        }
        out
    }

    #[test]
    fn crc16_matches_the_known_x25_vector() {
        // "123456789" is the standard check string for CRC-16/X-25, whose
        // published check value is 0x906E.
        let mut c = Crc16::new();
        c.update_all(b"123456789");
        assert_eq!(!c.residue(), 0x906e, "got {:04x}", !c.residue());
    }

    #[test]
    fn crc32_matches_the_known_vector() {
        let mut c = Crc32::new();
        c.update_all(b"123456789");
        assert_eq!(!c.residue(), 0xcbf4_3926, "got {:08x}", !c.residue());
    }

    #[test]
    fn a_good_frame_leaves_the_expected_residue() {
        // V.42 8.1.1.6.1: the receiver's register reads a fixed value over an
        // error-free frame including its FCS.
        let framed = append_fcs(b"\x03\x73hello", Fcs::Bits16);
        let mut c = Crc16::new();
        c.update_all(&framed);
        assert_eq!(c.residue(), Crc16::GOOD);
        assert!(fcs_ok(&framed, Fcs::Bits16));
    }

    #[test]
    fn crc32_residue_holds_too() {
        let framed = append_fcs(b"\x03\x73hello", Fcs::Bits32);
        assert!(fcs_ok(&framed, Fcs::Bits32));
    }

    #[test]
    fn round_trips_a_frame() {
        let payload = b"\x03\x73the quick brown fox";
        let got = round_trip(payload, Fcs::Bits16);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_ref().unwrap(), payload);
    }

    #[test]
    fn round_trips_with_a_32_bit_fcs() {
        let payload = b"\x03\x73wide check sequence";
        let got = round_trip(payload, Fcs::Bits32);
        assert_eq!(got[0].as_ref().unwrap(), payload);
    }

    #[test]
    fn bit_stuffing_survives_a_payload_full_of_ones() {
        // 0xFF runs are what stuffing exists for: without it these would look
        // like flags or aborts.
        let payload = vec![0xff; 32];
        let got = round_trip(&payload, Fcs::Bits16);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_ref().unwrap(), &payload);
    }

    #[test]
    fn a_payload_containing_the_flag_pattern_survives() {
        let payload = b"\x03\x73\x7e\x7e\x7e ends with flags \x7e";
        let got = round_trip(payload, Fcs::Bits16);
        assert_eq!(got[0].as_ref().unwrap(), payload);
    }

    #[test]
    fn the_encoder_never_emits_six_contiguous_ones_inside_a_frame() {
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.frame(&[0xff; 16]);
        let mut bits = Vec::new();
        while let Some(b) = enc.next_bit() {
            bits.push(b);
        }
        // Skip the opening and closing flags, which legitimately contain six.
        let body = &bits[8..bits.len() - 8];
        let mut run = 0;
        for &b in body {
            run = if b { run + 1 } else { 0 };
            assert!(run < 6, "stuffing failed: {run} contiguous ones");
        }
    }

    #[test]
    fn several_frames_in_one_stream() {
        let mut enc = Encoder::new(Fcs::Bits16);
        for payload in [b"\x03\x73one".as_slice(), b"\x03\x73two", b"\x03\x73three"] {
            enc.frame(payload);
        }
        let mut dec = Decoder::new(Fcs::Bits16);
        let mut got = Vec::new();
        while let Some(bit) = enc.next_bit() {
            if let Some(Ok(f)) = dec.feed(bit) {
                got.push(f);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(got[2], b"\x03\x73three");
    }

    #[test]
    fn interframe_flags_are_not_mistaken_for_frames() {
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.idle(6);
        enc.frame(b"\x03\x73payload");
        enc.idle(6);
        let mut dec = Decoder::new(Fcs::Bits16);
        let mut results = Vec::new();
        while let Some(bit) = enc.next_bit() {
            if let Some(r) = dec.feed(bit) {
                results.push(r);
            }
        }
        assert_eq!(results.len(), 1, "idle fill produced spurious results");
        assert_eq!(results[0].as_ref().unwrap(), b"\x03\x73payload");
    }

    #[test]
    fn a_corrupted_frame_is_rejected() {
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.frame(b"\x03\x73corrupt me");
        let mut bits = Vec::new();
        while let Some(b) = enc.next_bit() {
            bits.push(b);
        }
        // Flip a bit well inside the payload.
        bits[40] = !bits[40];
        let mut dec = Decoder::new(Fcs::Bits16);
        let mut results = Vec::new();
        for b in bits {
            if let Some(r) = dec.feed(b) {
                results.push(r);
            }
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Err(FrameError::BadFcs));
    }

    #[test]
    fn an_abort_discards_the_partial_frame() {
        let mut dec = Decoder::new(Fcs::Bits16);
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.idle(1);
        // Some content, then an abort rather than a closing flag.
        let mut bits = Vec::new();
        while let Some(b) = enc.next_bit() {
            bits.push(b);
        }
        bits.extend(std::iter::repeat_n(false, 16));
        bits.extend(std::iter::repeat_n(true, 8));
        let mut results = Vec::new();
        for b in bits {
            if let Some(r) = dec.feed(b) {
                results.push(r);
            }
        }
        assert_eq!(results, vec![Err(FrameError::Aborted)]);
    }

    #[test]
    fn a_runt_frame_is_rejected() {
        // One octet cannot carry an address, a control field and an FCS.
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.frame(&[]);
        let mut dec = Decoder::new(Fcs::Bits16);
        let mut results = Vec::new();
        while let Some(bit) = enc.next_bit() {
            if let Some(r) = dec.feed(bit) {
                results.push(r);
            }
        }
        assert_eq!(results, vec![Err(FrameError::TooShort)]);
    }

    #[test]
    fn an_over_long_frame_is_rejected() {
        let dec_max = 32;
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.frame(&vec![0x55; dec_max * 2]);
        let mut dec = Decoder::new(Fcs::Bits16).with_max_octets(dec_max);
        let mut results = Vec::new();
        while let Some(bit) = enc.next_bit() {
            if let Some(r) = dec.feed(bit) {
                results.push(r);
            }
        }
        assert_eq!(results, vec![Err(FrameError::TooLong)]);
    }

    #[test]
    fn the_decoder_syncs_mid_stream() {
        // Junk before the first flag must not prevent the frame being found.
        let mut enc = Encoder::new(Fcs::Bits16);
        enc.frame(b"\x03\x73found me");
        let mut bits = vec![true, false, true, true, false, false, true, false, true];
        while let Some(b) = enc.next_bit() {
            bits.push(b);
        }
        let mut dec = Decoder::new(Fcs::Bits16);
        let mut got = None;
        for b in bits {
            if let Some(Ok(f)) = dec.feed(b) {
                got = Some(f);
            }
        }
        assert_eq!(got.as_deref(), Some(b"\x03\x73found me".as_slice()));
    }

    #[test]
    fn a_shared_flag_delimits_both_neighbours() {
        // The closing flag of one frame may open the next (V.42 8.1.1.2), so a
        // stream with single flags between frames must decode as two.
        let a = append_fcs(b"\x03\x73aa", Fcs::Bits16);
        let b = append_fcs(b"\x03\x73bb", Fcs::Bits16);
        let mut bits = Vec::new();
        let push_flag = |bits: &mut Vec<bool>| {
            for i in 0..8 {
                bits.push(FLAG & (1 << i) != 0);
            }
        };
        let push_body = |bits: &mut Vec<bool>, body: &[u8]| {
            let mut ones = 0;
            for &byte in body {
                for i in 0..8 {
                    let bit = byte & (1 << i) != 0;
                    bits.push(bit);
                    if bit {
                        ones += 1;
                        if ones == 5 {
                            bits.push(false);
                            ones = 0;
                        }
                    } else {
                        ones = 0;
                    }
                }
            }
        };
        push_flag(&mut bits);
        push_body(&mut bits, &a);
        push_flag(&mut bits); // serves as closing for a and opening for b
        push_body(&mut bits, &b);
        push_flag(&mut bits);

        let mut dec = Decoder::new(Fcs::Bits16);
        let mut got = Vec::new();
        for bit in bits {
            if let Some(Ok(f)) = dec.feed(bit) {
                got.push(f);
            }
        }
        assert_eq!(got.len(), 2, "shared flag lost a frame");
        assert_eq!(got[0], b"\x03\x73aa");
        assert_eq!(got[1], b"\x03\x73bb");
    }

    #[test]
    fn every_payload_length_round_trips() {
        for n in 0..64usize {
            let payload: Vec<u8> = (0..n).map(|i| (i * 31 + 7) as u8).collect();
            let mut full = vec![0x03, 0x73];
            full.extend_from_slice(&payload);
            let got = round_trip(&full, Fcs::Bits16);
            assert_eq!(got.len(), 1, "length {n} produced {} results", got.len());
            assert_eq!(got[0].as_ref().unwrap(), &full, "length {n}");
        }
    }
}
