//! The two check sequences ZMODEM uses.
//!
//! Both are named in the document and neither is defined there. The 16-bit one
//! is the CCITT polynomial every XMODEM-CRC and YMODEM implementation uses,
//! which is what "compatible with YMODEM" in 7.2 is appealing to; the 32-bit
//! one is the ordinary reflected CRC-32 of everything else. Each is pinned
//! here by a published check value rather than by assertion, so a wrong
//! polynomial cannot pass for a right one.

/// CRC-16/XMODEM: `x^16 + x^12 + x^5 + 1`, no reflection, zero seed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Crc16(u16);

impl Crc16 {
    pub const POLY: u16 = 0x1021;

    pub fn new() -> Self {
        Self(0)
    }

    pub fn update(&mut self, byte: u8) {
        self.0 ^= u16::from(byte) << 8;
        for _ in 0..8 {
            self.0 = if self.0 & 0x8000 != 0 {
                (self.0 << 1) ^ Self::POLY
            } else {
                self.0 << 1
            };
        }
    }

    pub fn update_all(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.update(b);
        }
    }

    /// The value, most significant byte first, which is the order it goes on
    /// the line in.
    pub fn to_bytes(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    pub fn value(self) -> u16 {
        self.0
    }
}

/// CRC-32: the reflected `0xEDB88320` polynomial, seeded and finished with all
/// ones, as in HDLC and everywhere else.
#[derive(Debug, Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub const POLY: u32 = 0xEDB8_8320;

    pub fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    pub fn update(&mut self, byte: u8) {
        self.0 ^= u32::from(byte);
        for _ in 0..8 {
            self.0 = if self.0 & 1 != 0 { (self.0 >> 1) ^ Self::POLY } else { self.0 >> 1 };
        }
    }

    pub fn update_all(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.update(b);
        }
    }

    /// The value, least significant byte first, which is the order ZMODEM
    /// sends it in -- the opposite way round from the 16-bit one.
    pub fn to_bytes(self) -> [u8; 4] {
        (self.0 ^ 0xFFFF_FFFF).to_le_bytes()
    }

    pub fn value(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sixteen_bit_check_matches_its_published_value() {
        // The CRC-16/XMODEM check value: "123456789" gives 0x31C3. Every
        // catalogue of these lists it, and a wrong polynomial or a wrong seed
        // gives something else.
        let mut c = Crc16::new();
        c.update_all(b"123456789");
        assert_eq!(c.value(), 0x31C3);
    }

    #[test]
    fn the_thirty_two_bit_check_matches_its_published_value() {
        // CRC-32: "123456789" gives 0xCBF43926.
        let mut c = Crc32::new();
        c.update_all(b"123456789");
        assert_eq!(c.value(), 0xCBF4_3926);
    }

    #[test]
    fn the_two_go_on_the_line_in_opposite_orders() {
        // Not a detail to get wrong: the 16-bit check is sent high byte first
        // and the 32-bit one low byte first, which is why they are separate
        // methods rather than one generic one.
        let mut a = Crc16::new();
        a.update_all(b"123456789");
        assert_eq!(a.to_bytes(), [0x31, 0xC3]);

        let mut b = Crc32::new();
        b.update_all(b"123456789");
        assert_eq!(b.to_bytes(), [0x26, 0x39, 0xF4, 0xCB]);
    }

    #[test]
    fn an_empty_run_is_the_seed() {
        assert_eq!(Crc16::new().value(), 0);
        assert_eq!(Crc32::new().value(), 0);
    }
}
