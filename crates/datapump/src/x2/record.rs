//! x2's records: the client's MP and the server's record, both directions.
//!
//! libv90modem's `docs/history/x2-facts.md`, "Records" (the I-modem's
//! serializer at DSP `b54c`-`b589`, and the records D-Modem and courier-emu
//! published):
//!
//! - 17 ones and a zero;
//! - each 16-bit word, low-order bit first, followed by a zero;
//! - the CRC-16 of the word bits alone (not the zeros): the register from
//!   FFFF, x^16 + x^12 + x^5 + 1 run reflected (0x8408), no final
//!   inversion, its bit 0 first.
//!
//! The server's record is three words padded with zeros to 96 bits, four
//! frames of levels A. The client's MP is four words, then two zeros, 104
//! bits a record.

/// The sync: 17 ones, then a zero.
const SYNC_ONES: usize = 17;

/// Bits in a record of `words` words.
pub const fn bits(words: usize) -> usize {
    SYNC_ONES + 1 + 17 * words + 16
}

/// The server's record padded to whole frames of levels A: 96 bits.
pub const SERVER_BITS: usize = 96;

/// The CRC of a run of bits.
pub fn crc(bits: impl IntoIterator<Item = bool>) -> u16 {
    bits.into_iter().fold(0xffff, |c, b| {
        let feedback = (u16::from(b) ^ c) & 1 != 0;
        let c = c >> 1;
        if feedback { c ^ 0x8408 } else { c }
    })
}

fn word_bits(w: u16) -> impl Iterator<Item = bool> {
    (0..16).map(move |k| (w >> k) & 1 != 0)
}

/// A record of `words` into bits.
pub fn build(words: &[u16]) -> Vec<bool> {
    let mut out = vec![true; SYNC_ONES];
    out.push(false);
    for &w in words {
        out.extend(word_bits(w));
        out.push(false);
    }
    let c = crc(words.iter().flat_map(|&w| word_bits(w)));
    out.extend(word_bits(c));
    out
}

/// The server's record: `F33C 7FFF 0000` as the I-modem sends it, word 0's
/// bit 15 the acknowledge (`733C` without it), padded to [`SERVER_BITS`].
///
/// What the other bits mean is not known; word 1 is presumably the rates
/// offered, one bit an index, and a Courier fails to connect when bit 0 is
/// missing from it (x2-facts, "Our record's word 1"). We have every index's
/// bank, so it is the I-modem's `7FFF`.
pub fn server(ack: bool) -> Vec<bool> {
    let w0 = if ack { 0xf33c } else { 0x733c };
    let mut out = build(&[w0, 0x7fff, 0x0000]);
    out.resize(SERVER_BITS, false);
    out
}

/// The client's MP: four words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp(pub [u16; 4]);

impl Mp {
    /// The downstream rate index the client asks for, 1 to 15 (word 0 bits
    /// 2 to 5): 1 at the start-up, what it measured at a rate change.
    pub fn rate(&self) -> u8 {
        ((self.0[0] >> 2) & 15) as u8
    }

    /// The digital pad it measured (word 0 bits 11 to 14): 0 none, 1 3 dB,
    /// 2 6 dB.
    pub fn pad(&self) -> u8 {
        ((self.0[0] >> 11) & 15) as u8
    }

    /// The acknowledge (word 0 bit 15): MP'.
    pub fn ack(&self) -> bool {
        self.0[0] & 0x8000 != 0
    }

    /// The upstream rates it can send (word 1 bits 0 to 11; bit n - 1 for n
    /// x 2400).
    pub fn upstream(&self) -> u16 {
        self.0[1] & 0x0fff
    }

    /// Downstream signs a frame (word 3's high octet): 5.
    pub fn md(&self) -> u8 {
        (self.0[3] >> 8) as u8
    }

    /// The fastest upstream rate both may use, in bit/s: the highest the mask
    /// names, at most 28 800 (x2-facts, "Upstream"). None if the mask is
    /// empty.
    pub fn upstream_rate(&self) -> Option<u32> {
        (1..=12u32).rev().find(|n| self.upstream() >> (n - 1) & 1 != 0).map(|n| n * 2400)
    }
}

/// Finds MPs in the bits the client sends: a sync, four words with their
/// zeros, and a CRC that holds.
#[derive(Debug, Clone, Default)]
pub struct Reader {
    window: Vec<bool>,
}

impl Reader {
    pub fn new() -> Self {
        Self::default()
    }

    /// One descrambled bit. Yields an MP the bit completes.
    pub fn push(&mut self, bit: bool) -> Option<Mp> {
        const N: usize = bits(4);
        self.window.push(bit);
        if self.window.len() > N {
            self.window.remove(0);
        }
        if self.window.len() < N {
            return None;
        }
        let mp = parse(&self.window, 4)?;
        self.window.clear();
        Some(Mp([mp[0], mp[1], mp[2], mp[3]]))
    }
}

/// A record of `n` words from bits that begin with its sync, if it checks
/// out.
pub fn parse(bits: &[bool], n: usize) -> Option<Vec<u16>> {
    if bits.len() < self::bits(n) {
        return None;
    }
    if !bits[..SYNC_ONES].iter().all(|&b| b) || bits[SYNC_ONES] {
        return None;
    }
    let mut at = SYNC_ONES + 1;
    let mut words = Vec::with_capacity(n);
    for _ in 0..n {
        let w = (0..16).fold(0u16, |w, k| w | u16::from(bits[at + k]) << k);
        at += 16;
        if bits[at] {
            return None;
        }
        at += 1;
        words.push(w);
    }
    let c = crc(words.iter().flat_map(|&w| word_bits(w)));
    let got = (0..16).fold(0u16, |w, k| w | u16::from(bits[at + k]) << k);
    (got == c).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crcs_are_the_published_ones() {
        // x2-facts "Records", check values; 14AD is courier-emu's too.
        let crc_of = |w: &[u16]| crc(w.iter().flat_map(|&w| word_bits(w)));
        assert_eq!(crc_of(&[0x0344, 0x03fe, 0x0000, 0x0500]), 0x14ad);
        assert_eq!(crc_of(&[0x8344, 0x03fe, 0x0000, 0x0500]), 0x92b0);
        assert_eq!(crc_of(&[0xf33c, 0x7fff, 0x0000]), 0x73f3);
        assert_eq!(crc_of(&[0x733c, 0x7fff, 0x0000]), 0xf9a6);
    }

    #[test]
    fn the_server_record_is_96_bits_and_reads_back() {
        for ack in [false, true] {
            let r = server(ack);
            assert_eq!(r.len(), 96);
            assert_eq!(bits(3), 85);
            let w = parse(&r, 3).unwrap();
            assert_eq!(w, vec![if ack { 0xf33c } else { 0x733c }, 0x7fff, 0]);
        }
    }

    #[test]
    fn a_couriers_mp_is_found_among_other_bits_and_read() {
        // The Courier's start-up MP (x2-facts): index 1, no pad, upstream to
        // 24000, MD 5. Behind sixty ones and between its two zeros' worth.
        let mut stream = vec![true; 60];
        stream.extend(build(&[0x0344, 0x03fe, 0x0000, 0x0500]));
        stream.extend([false, false]);
        stream.extend(build(&[0x8344, 0x03fe, 0x0000, 0x0500]));
        let mut r = Reader::new();
        let found: Vec<Mp> = stream.into_iter().filter_map(|b| r.push(b)).collect();
        assert_eq!(found.len(), 2);
        let mp = found[0];
        assert_eq!((mp.rate(), mp.pad(), mp.ack(), mp.md()), (1, 0, false, 5));
        assert_eq!(mp.upstream_rate(), Some(24000));
        assert!(found[1].ack());
        // At a rate change behind 3 dB: index 12, pad 1, upstream to 28800.
        let mp = Mp([0x0b70, 0x1ffe, 0x8888, 0x0588]);
        assert_eq!((mp.rate(), mp.pad(), mp.md()), (12, 1, 5));
        assert_eq!(mp.upstream_rate(), Some(28800));
    }

    #[test]
    fn a_damaged_mp_is_not_read() {
        let mut b = build(&[0x0344, 0x03fe, 0x0000, 0x0500]);
        b[40] = !b[40];
        let mut r = Reader::new();
        assert!(b.into_iter().all(|bit| r.push(bit).is_none()));
    }
}
