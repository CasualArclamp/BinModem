//! MD5, RFC 1321, because digest authentication is specified in terms of it.
//!
//! Not a choice. RFC 3261 22.4 defines the credentials a registrar will accept
//! as MD5 over a particular set of strings, so a user agent that wants to
//! register computes MD5 or does not register. Its weakness as a hash is real
//! and beside the point: nothing here is being protected by it that is not
//! already being sent over unencrypted UDP.
//!
//! Sixty lines rather than a dependency, for the same reason everything else
//! in this workspace is: the algorithm is fully specified in the document, and
//! a hash that agrees with the test vectors in RFC 1321 A.5 is the hash.

/// 4.3's four non-linear functions, one per round.
const fn f(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (!x & z)
}
const fn g(x: u32, y: u32, z: u32) -> u32 {
    (x & z) | (y & !z)
}
const fn h(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}
const fn i(x: u32, y: u32, z: u32) -> u32 {
    y ^ (x | !z)
}

/// 3.4's table: the integer part of 4294967296 times abs(sin(n)), for n from
/// one to sixty-four, with n in radians.
const T: [u32; 64] = [
    0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee, 0xf57c_0faf, 0x4787_c62a, 0xa830_4613,
    0xfd46_9501, 0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be, 0x6b90_1122, 0xfd98_7193,
    0xa679_438e, 0x49b4_0821, 0xf61e_2562, 0xc040_b340, 0x265e_5a51, 0xe9b6_c7aa, 0xd62f_105d,
    0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8, 0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed,
    0xa9e3_e905, 0xfcef_a3f8, 0x676f_02d9, 0x8d2a_4c8a, 0xfffa_3942, 0x8771_f681, 0x6d9d_6122,
    0xfde5_380c, 0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70, 0x289b_7ec6, 0xeaa1_27fa,
    0xd4ef_3085, 0x0488_1d05, 0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665, 0xf429_2244,
    0x432a_ff97, 0xab94_23a7, 0xfc93_a039, 0x655b_59c3, 0x8f0c_cc92, 0xffef_f47d, 0x8584_5dd1,
    0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1, 0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb,
    0xeb86_d391,
];

/// Which word of the block each operation reads, round by round.
const K: [usize; 64] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, // round 1: in order
    1, 6, 11, 0, 5, 10, 15, 4, 9, 14, 3, 8, 13, 2, 7, 12, // round 2: 5i + 1
    5, 8, 11, 14, 1, 4, 7, 10, 13, 0, 3, 6, 9, 12, 15, 2, // round 3: 3i + 5
    0, 7, 14, 5, 12, 3, 10, 1, 8, 15, 6, 13, 4, 11, 2, 9, // round 4: 7i
];

/// And by how much each rotates left.
const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, //
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, //
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, //
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// The digest of a message, as the sixteen octets 3.5 produces.
pub fn digest(message: &[u8]) -> [u8; 16] {
    // 3.3's initial state, little-endian as everything in MD5 is.
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

    // 3.1 and 3.2: a one bit, then zeros until the length is 56 mod 64, then
    // the original length in bits as a 64-bit little-endian number.
    let mut padded = message.to_vec();
    let bits = (message.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bits.to_le_bytes());

    // 3.4 works on sixteen-word blocks, and the padding above has made the
    // length a multiple of one, so there is no remainder to account for.
    let (blocks, _) = padded.as_chunks::<64>();
    for block in blocks {
        let mut x = [0u32; 16];
        let (words, _) = block.as_chunks::<4>();
        for (slot, word) in x.iter_mut().zip(words) {
            *slot = u32::from_le_bytes(*word);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for step in 0..64 {
            let mixed = match step / 16 {
                0 => f(b, c, d),
                1 => g(b, c, d),
                2 => h(b, c, d),
                _ => i(b, c, d),
            };
            let sum = a
                .wrapping_add(mixed)
                .wrapping_add(x[K[step]])
                .wrapping_add(T[step]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(S[step]));
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }

    let mut out = [0u8; 16];
    let (slots, _) = out.as_chunks_mut::<4>();
    for (slot, word) in slots.iter_mut().zip(state.iter()) {
        *slot = word.to_le_bytes();
    }
    out
}

/// The digest as the thirty-two lower-case hex characters a digest
/// authentication header carries. RFC 7616 3.4.1 calls this LHEX, and the
/// case is not optional: it is hashed again as part of the response.
pub fn hex(message: &[u8]) -> String {
    let mut s = String::with_capacity(32);
    for byte in digest(message) {
        s.push(char::from_digit(u32::from(byte >> 4), 16).unwrap());
        s.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 1321 A.5's suite, all of it. A hash that passes this is the hash.
    #[test]
    fn the_rfc_1321_test_suite() {
        let cases: [(&str, &str); 7] = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex(input.as_bytes()), expected, "MD5 of {input:?}");
        }
    }

    /// A message that lands exactly on a block boundary, which is where a
    /// padding rule goes wrong if it is going to.
    #[test]
    fn padding_at_the_boundary() {
        // 55 octets pad into one block; 56 need a second one.
        assert_eq!(hex(&[b'a'; 55]).len(), 32);
        assert_eq!(
            hex(&[b'a'; 56]),
            "3b0c8ac703f828b04c6c197006d17218",
            "56 a's"
        );
    }
}
