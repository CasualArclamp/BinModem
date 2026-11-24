//! The MD5 message digest (RFC 1321), which is what CHAP hashes with.
//!
//! RFC 1994 4.1 has the answer to a challenge be "the one-way hash calculated
//! over a stream of octets consisting of the Identifier, followed by the
//! secret, followed by the Challenge Value", and its option 3 names the hash:
//! algorithm 5, MD5. That is the only use of it here. MD5 has not been a
//! sound hash for twenty years, and CHAP is not made sound by it; what it
//! still does is keep the password itself off the line, which is the whole
//! of CHAP's advantage over PAP.
//!
//! Written from 1321 section 3, step by step, and checked against the test
//! suite in its appendix A.5.

/// 3.4: the table built from the sine function. "Let T[i] denote the i-th
/// element of the table, which is equal to the integer part of 4294967296
/// times abs(sin(i)), where i is in radians."
///
/// Written out rather than computed, and the test below computes it again to
/// check.
const T: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
    0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
    0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
    0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
    0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
    0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
    0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
    0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// 3.4: how far each operation rotates, four to a round. The rounds write them
/// out as `[abcd k s i]`, and `s` goes round these.
const SHIFTS: [[u32; 4]; 4] = [[7, 12, 17, 22], [5, 9, 14, 20], [4, 11, 16, 23], [6, 10, 15, 21]];

/// 3.3: the buffer's starting words, "in hexadecimal, low-order bytes first",
/// which as little-endian words are these.
const START: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

/// The digest of `message`, sixteen octets.
pub fn digest(message: &[u8]) -> [u8; 16] {
    // 3.1: a single one bit, then zeroes until the length is 448 modulo 512
    // bits. "Padding is always performed, even if the length of the message is
    // already congruent to 448, modulo 512."
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    // 3.2: the length in bits, 64 of them, low-order word first.
    let bits = (message.len() as u64).wrapping_mul(8);
    padded.extend_from_slice(&bits.to_le_bytes());

    let mut state = START;
    for block in padded.as_chunks::<64>().0 {
        // 3.4: "Copy block i into X", a word at a time, low-order byte first.
        let mut x = [0u32; 16];
        for (word, bytes) in x.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_le_bytes(*bytes);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let round = i / 16;
            // The four auxiliary functions of 3.4, and which word of the block
            // each step of each round reads: round 1 takes them in order,
            // round 2 from 1 in steps of 5, round 3 from 5 in steps of 3, and
            // round 4 from 0 in steps of 7.
            let (f, k) = match round {
                0 => ((b & c) | (!b & d), i),
                1 => ((b & d) | (c & !d), (1 + 5 * i) % 16),
                2 => (b ^ c ^ d, (5 + 3 * i) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            // "a = b + ((a + F(b,c,d) + X[k] + T[i]) <<< s)", and the four
            // words then move round one place: [abcd], [dabc], [cdab], [bcda].
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(x[k])
                .wrapping_add(T[i])
                .rotate_left(SHIFTS[round][i % 4]);
            let next = b.wrapping_add(rotated);
            a = d;
            d = c;
            c = b;
            b = next;
        }
        // "Then perform the following additions... increment each of the four
        // registers by the value it had before this block was started."
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }

    // 3.5: "The message digest produced as output is A, B, C, D ... we begin
    // with the low-order byte of A, and end with the high-order byte of D."
    let mut out = [0u8; 16];
    for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_le_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Appendix A.5, the test suite, all seven.
    #[test]
    fn the_test_suite_in_the_appendix() {
        let suite: [(&str, &str); 7] = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (message, want) in suite {
            assert_eq!(hex(&digest(message.as_bytes())), want, "MD5 ({message:?})");
        }
    }

    /// The lengths where padding spills into another block, or only just does
    /// not: 3.1's "always performed" is what makes 56 and 64 two blocks.
    #[test]
    fn messages_either_side_of_a_block_boundary() {
        let cases: [(usize, &str); 7] = [
            (55, "52c0e574e1198de5fe3f8f11440dcb1b"),
            (56, "46c9907fc908ee68b1e7b8e71286a518"),
            (63, "a62f6d59e837867693f042f5b8f5a236"),
            (64, "7160b8fb5e9e4023d549c3971fbaeead"),
            (65, "70bd662e7aefbda85a0f7244167b7897"),
            (119, "e84905d4214f4d1ca56c2cdcc152b143"),
            (120, "e3eb5a6c8669ea01a8c185b8abc8a5dc"),
        ];
        for (length, want) in cases {
            let message: Vec<u8> = (0..length).map(|i| ((i * 7 + 3) % 256) as u8).collect();
            assert_eq!(hex(&digest(&message)), want, "{length} octets");
        }
    }

    /// 3.4's definition of the table, against the table.
    #[test]
    fn the_table_is_the_sine_function() {
        for (i, &t) in T.iter().enumerate() {
            let from_sine = (((i + 1) as f64).sin().abs() * 4_294_967_296.0) as u32;
            assert_eq!(t, from_sine, "T[{}]", i + 1);
        }
    }
}
