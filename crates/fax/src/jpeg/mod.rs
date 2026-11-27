//! JPEG as a fax uses it: T.81's baseline process, profiled by T.4 Annex E.
//!
//! A greyscale or colour page goes down the line as one JPEG data stream:
//! sequential DCT, Huffman coded, eight bits a sample (T.4 E.2.3). What makes
//! it a fax and not a photograph is around the coding and not in it:
//!
//! - the components are CIELAB's L*, a* and b* as T.42 codes them, not YCbCr,
//!   so a decoder that hands back RGB has done the wrong arithmetic. This one
//!   hands back the components and stops;
//! - an `APP1` marker, `"G3FAX"`, says so, and carries the resolution
//!   (T.4 E.6.5);
//! - a machine that scans as it sends does not know how long the page is when
//!   it writes the frame header. It writes zero, and says the number of lines
//!   in a DNL marker after the scan (T.4 E.6.2, T.81 B.2.5).
//!
//! The decoder takes the stream as it arrives and gives up MCU rows as they
//! are complete, so that a page can be watched on its way in. The encoder is
//! the least that makes a stream a fax machine reads: the tables of T.81
//! Annex K and no optimisation.

mod dct;
mod decoder;
mod encoder;

pub use decoder::{Decoder, for_a_file};
pub use encoder::{Options, encode};

/// Marker codes, the octet after X'FF' (T.81 Table B.1).
pub mod marker {
    /// Baseline DCT frame.
    pub const SOF0: u8 = 0xC0;
    /// Extended sequential DCT frame, Huffman coded.
    pub const SOF1: u8 = 0xC1;
    pub const DHT: u8 = 0xC4;
    pub const RST0: u8 = 0xD0;
    pub const RST7: u8 = 0xD7;
    pub const SOI: u8 = 0xD8;
    pub const EOI: u8 = 0xD9;
    pub const SOS: u8 = 0xDA;
    pub const DQT: u8 = 0xDB;
    pub const DNL: u8 = 0xDC;
    pub const DRI: u8 = 0xDD;
    pub const APP1: u8 = 0xE1;
}

/// The identifier in T.4's `APP1` markers: "G3FAX", and then one octet that
/// says which of them this is (T.4 E.6.5 to E.6.8).
pub const G3FAX: &[u8; 5] = b"G3FAX";

/// The version in the first `APP1`: X'07CA', which is 1994, "the year of
/// approval of the standard" (T.4 E.6.5).
pub const G3FAX_VERSION: u16 = 0x07CA;

/// Where coefficient `k` of the zig-zag sequence sits in the 8 x 8 block,
/// row-major (T.81 Figure A.6).
pub const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52,
    45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// A Huffman table as a DHT segment gives it: how many codes there are of
/// each length from 1 to 16, and the values in order of code (T.81 B.2.4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuffmanSpec {
    pub bits: [u8; 16],
    pub values: Vec<u8>,
}

impl HuffmanSpec {
    /// The code and its length for each value, by value: HUFFSIZE and
    /// HUFFCODE of T.81 C.2 (Figures C.1 and C.2), ordered as Figure C.3
    /// orders them into EHUFCO and EHUFSI.
    pub fn codes(&self) -> [(u16, u8); 256] {
        let mut out = [(0u16, 0u8); 256];
        let mut code = 0u16;
        let mut at = 0;
        for (length, &count) in self.bits.iter().enumerate() {
            for _ in 0..count {
                if let Some(&value) = self.values.get(at) {
                    out[usize::from(value)] = (code, length as u8 + 1);
                }
                code = code.wrapping_add(1);
                at += 1;
            }
            code <<= 1;
        }
        out
    }
}

/// T.81 Table K.3: DC differences, luminance. With K.4 to K.6, "the
/// preferred Huffman tables" of T.4 E.6.2.1 Note 2. The octets are those of
/// T.81 K.3.3.
pub fn dc_luminance() -> HuffmanSpec {
    HuffmanSpec {
        bits: [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
        values: (0..=11).collect(),
    }
}

/// T.81 Table K.4: DC differences, chrominance (K.3.3.1).
pub fn dc_chrominance() -> HuffmanSpec {
    HuffmanSpec {
        bits: [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0],
        values: (0..=11).collect(),
    }
}

/// T.81 Table K.5: AC coefficients, luminance (K.3.3.2).
pub fn ac_luminance() -> HuffmanSpec {
    HuffmanSpec {
        bits: [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D],
        values: vec![
            0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61,
            0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52,
            0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25,
            0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45,
            0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64,
            0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83,
            0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
            0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6,
            0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3,
            0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8,
            0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
        ],
    }
}

/// T.81 Table K.6: AC coefficients, chrominance (K.3.3.2).
pub fn ac_chrominance() -> HuffmanSpec {
    HuffmanSpec {
        bits: [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77],
        values: vec![
            0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61,
            0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33,
            0x52, 0xF0, 0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18,
            0x19, 0x1A, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44,
            0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63,
            0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A,
            0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
            0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4,
            0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA,
            0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7,
            0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
        ],
    }
}

/// T.81 Table K.1: a quantisation table for luminance, in row-major order.
pub const QUANT_LUMINANCE: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// T.81 Table K.2: a quantisation table for chrominance, in row-major order.
pub const QUANT_CHROMINANCE: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zigzag_visits_every_coefficient_once() {
        let mut seen = [false; 64];
        for &at in &ZIGZAG {
            seen[usize::from(at)] = true;
        }
        assert!(seen.iter().all(|s| *s));
        // Figure A.6: 0, 1, 5, 6 across the top row, 2 below the first.
        assert_eq!(&ZIGZAG[..6], &[0, 1, 8, 16, 9, 2]);
        assert_eq!(ZIGZAG[63], 63);
    }

    #[test]
    fn the_typical_tables_have_as_many_values_as_they_count() {
        for (table, values) in
            [(dc_luminance(), 12), (dc_chrominance(), 12), (ac_luminance(), 162), (ac_chrominance(), 162)]
        {
            let counted: usize = table.bits.iter().map(|&n| usize::from(n)).sum();
            assert_eq!(counted, values);
            assert_eq!(table.values.len(), values);
        }
    }

    #[test]
    fn the_codes_of_table_k3_are_the_ones_it_prints() {
        // T.81 Table K.3: category 0 is "00", 1 is "010", 5 is "110", 6 is
        // "1110" and 11 is "111111110".
        let codes = dc_luminance().codes();
        assert_eq!(codes[0], (0b00, 2));
        assert_eq!(codes[1], (0b010, 3));
        assert_eq!(codes[5], (0b110, 3));
        assert_eq!(codes[6], (0b1110, 4));
        assert_eq!(codes[11], (0b1_1111_1110, 9));
        // Table K.5: EOB is "1010" and ZRL is "11111111001".
        let codes = ac_luminance().codes();
        assert_eq!(codes[0x00], (0b1010, 4));
        assert_eq!(codes[0xF0], (0b111_1111_1001, 11));
        assert_eq!(codes[0x01], (0b00, 2));
    }
}
