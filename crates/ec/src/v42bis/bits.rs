//! Bit packing for compressed mode (V.42bis 7.5, Figure 3).
//!
//! Codewords are packed so that "the least significant bit of a codeword
//! immediately follows the most significant bit of the preceding codeword", so
//! both codewords and the octets holding them run low-order bit first.

/// Packs codewords into octets, low-order bit first.
#[derive(Debug, Default)]
pub struct BitWriter {
    acc: u32,
    bits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `count` low-order bits of `value`.
    pub fn write(&mut self, value: u16, count: u32, out: &mut Vec<u8>) {
        self.acc |= (u32::from(value) & ((1u32 << count) - 1)) << self.bits;
        self.bits += count;
        while self.bits >= 8 {
            out.push((self.acc & 0xff) as u8);
            self.acc >>= 8;
            self.bits -= 8;
        }
    }

    /// Bits held back short of a whole octet.
    pub fn pending(&self) -> u32 {
        self.bits
    }

    /// Pad with zeroes to the next octet boundary (V.42bis 7.5, 7.8.2 d).
    pub fn align(&mut self, out: &mut Vec<u8>) {
        if self.bits > 0 {
            out.push((self.acc & 0xff) as u8);
            self.acc = 0;
            self.bits = 0;
        }
    }
}

/// Reads codewords back out of octets, low-order bit first.
///
/// Octets queue up rather than shifting straight into the accumulator, so any
/// number may be pushed before anything is read. Shifting them in directly
/// would overflow the accumulator as soon as more than a few were buffered.
#[derive(Debug, Default)]
pub struct BitReader {
    queue: std::collections::VecDeque<u8>,
    acc: u32,
    bits: u32,
}

impl BitReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_octet(&mut self, byte: u8) {
        self.queue.push_back(byte);
    }

    /// Move queued octets into the accumulator until `count` bits are ready.
    fn fill(&mut self, count: u32) {
        while self.bits < count {
            let Some(byte) = self.queue.pop_front() else { return };
            self.acc |= u32::from(byte) << self.bits;
            self.bits += 8;
        }
    }

    /// Take `count` bits if that many have arrived.
    pub fn read(&mut self, count: u32) -> Option<u16> {
        debug_assert!(count <= 16, "codewords are at most 16 bits");
        self.fill(count);
        if self.bits < count {
            return None;
        }
        let value = (self.acc & ((1u32 << count) - 1)) as u16;
        self.acc >>= count;
        self.bits -= count;
        Some(value)
    }

    /// Bits held, including octets still queued.
    pub fn available(&self) -> u32 {
        self.bits + self.queue.len() as u32 * 8
    }

    /// Drop the remainder of the current octet, as after ETM or FLUSH.
    pub fn align(&mut self) {
        let drop = self.bits % 8;
        self.acc >>= drop;
        self.bits -= drop;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nine_bit_codeword_spans_two_octets() {
        // Figure 3: A1 through A8 fill octet i, A9 onward continue in i+1.
        let mut w = BitWriter::new();
        let mut out = Vec::new();
        w.write(0b1_0000_0001, 9, &mut out);
        assert_eq!(out, vec![0b0000_0001]);
        assert_eq!(w.pending(), 1);
    }

    #[test]
    fn codewords_round_trip_at_a_fixed_width() {
        let values: Vec<u16> = (0..200).map(|i| (i * 7 % 512) as u16).collect();
        let mut w = BitWriter::new();
        let mut out = Vec::new();
        for &v in &values {
            w.write(v, 9, &mut out);
        }
        w.align(&mut out);

        let mut r = BitReader::new();
        for b in out {
            r.push_octet(b);
        }
        for &v in &values {
            assert_eq!(r.read(9), Some(v));
        }
    }

    #[test]
    fn a_changing_codeword_width_round_trips() {
        // The width grows as STEPUP is issued, so the reader must follow.
        let mut w = BitWriter::new();
        let mut out = Vec::new();
        w.write(300, 9, &mut out);
        w.write(700, 10, &mut out);
        w.write(1500, 11, &mut out);
        w.align(&mut out);

        let mut r = BitReader::new();
        for b in out {
            r.push_octet(b);
        }
        assert_eq!(r.read(9), Some(300));
        assert_eq!(r.read(10), Some(700));
        assert_eq!(r.read(11), Some(1500));
    }

    #[test]
    fn reading_more_than_has_arrived_yields_nothing() {
        let mut r = BitReader::new();
        r.push_octet(0xff);
        assert_eq!(r.read(9), None, "only eight bits are available");
        r.push_octet(0x01);
        assert_eq!(r.read(9), Some(0b1_1111_1111));
    }

    #[test]
    fn alignment_discards_only_the_partial_octet() {
        let mut r = BitReader::new();
        r.push_octet(0xab);
        r.push_octet(0xcd);
        let _ = r.read(3);
        r.align();
        assert_eq!(r.available(), 8, "the whole second octet should remain");
        assert_eq!(r.read(8), Some(0xcd));
    }

    #[test]
    fn writer_alignment_pads_with_zeroes() {
        let mut w = BitWriter::new();
        let mut out = Vec::new();
        w.write(0b111, 3, &mut out);
        assert!(out.is_empty());
        w.align(&mut out);
        assert_eq!(out, vec![0b0000_0111], "padded, not shifted");
        assert_eq!(w.pending(), 0);
    }
}
