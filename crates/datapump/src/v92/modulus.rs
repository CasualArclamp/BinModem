//! The upstream modulus encoder and its decoder (6.4.1): twelve moduli, K up
//! to 72 bits, a product that reaches 255^12, and the differential sign step
//! that makes an inverted line decode all the same.
//!
//! The problem is V.90's, taken the other way up the line. Each of the twelve
//! symbols of an upstream data frame carries one of M_i levels, and M_i is
//! whatever the *digital* modem decided it could tell apart on this route --
//! 61, or 90, or anything else. Twelve such intervals carry the product of
//! their moduli between them, which is not a power of two, so a frame's bits
//! cannot be split up and handed out interval by interval. 6.4.1 treats the
//! whole frame as one integer instead and writes it in a mixed radix: "Divide
//! R0 by M0. The remainder of this division gives K0, the quotient becomes R1
//! for use in the calculation for the next data frame interval."
//!
//! Two things make this more than [`crate::v90::modulus`] with six more
//! intervals.
//!
//! The first is size. V.90's six moduli stop at 128 each and its K at 42; here
//! M_i is a CPd byte and K reaches 72, so M can be as large as 255^12, a
//! little under 2^96, and every step of the arithmetic is u128. A u64 would
//! wrap silently and the frame would decode to something plausible and wrong
//! (pitfall P-11).
//!
//! The second is the sign. Steps 2 to 4 take the half of [0, M) that R falls
//! in, differentially encode that one bit from frame to frame, and reflect R
//! about the middle -- R0 = M - 1 - R -- whenever the differential bit says
//! so. What it buys is immunity to an inverted line. The precoder's
//! constellation indices are signed and symmetric, so a receiver holding the
//! polarity backwards sees every index eta as -eta-1, which is every K_i as
//! M_i - 1 - K_i, which is exactly R0 as M - 1 - R0; and reflecting a
//! reflection is the identity. A decoder whose own d starts inverted therefore
//! follows an inverted line frame for frame and hands out the bits that were
//! sent, which is what
//! `an_inverted_channel_decodes_with_the_decoder_started_inverted` shows.
//!
//! Nothing here knows about constellations. The encoder's output is K_0..K_11,
//! "where K0 corresponds to data frame interval 0 and K11 corresponds to data
//! frame interval 11", and turning a K_i into a level is the precoder's work
//! (6.4.2, `v92::precoder`).

use super::{Parameters, UP_INTERVALS};

// ---------------------------------------------------------------------------
// The two readings this clause leaves open (plan section 4)
// ---------------------------------------------------------------------------

/// Which frame's differential bit step 4 reflects on.
///
/// Step 4 is printed "R0 = R if d(f - 1) = 0; R0 = M - 1 - R if d(f - 1) = 1",
/// read off the rendered page and confirmed at 300 dpi, so this is `true`: the
/// bit is the *previous* frame's, the one step 3 has just replaced. The
/// alternative reading is d(f), the bit step 3 has this moment produced.
///
/// The alternative is not another convention that would merely decode
/// differently: it is not a mapping that can be undone at all. With
/// d(f) = s(f) + d(f - 1) and s(f) the half R lies in, reflecting on d(f)
/// folds the whole of [0, M) into one half of it, so two frames share an R0
/// and no decoder can tell them apart -- see
/// `the_alternative_reading_of_step_4_is_not_one_to_one`. A capture that
/// disagreed with the printed reading would therefore not just flip this
/// switch; it would mean the differential step means something else again,
/// and [`Decoder`] says so rather than guessing.
pub const STEP4_USES_PREVIOUS_D: bool = true;

/// d(-1), the differential memory before a frame chain's first frame.
///
/// Clause 6 never states it. 8.7.1 does, for the one moment it matters: "The
/// scrambler, modulus encoder, convolutional encoder, precoder and prefilter
/// memories are initialized to zero prior to transmitting B1u", and 9.9.2
/// zeroes the same memory again at a fast parameter exchange. So zero, and
/// [`Encoder::reset`] is how a caller says one of those moments has come. The
/// alternative would be one, which no clause suggests and which would only
/// invert the first frame's mapping.
pub const D_BEFORE_THE_FIRST_FRAME: bool = false;

/// The most bits one upstream data frame can carry: K = 72, at drn 19.
///
/// 6.1 runs the ladder from "24 000 bit/s to 48 000 bit/s in increments of
/// 8000/6 bit/s" and a data frame is twelve symbols (Figure 1), so
/// K = 12 x rate/8000 and [`super::up_bits`] is the same number by another
/// route. It is a bound on this module's arithmetic, not a rule of 6.4.1:
/// nothing in the clause stops a larger K, but nothing can ask for one.
pub const LONGEST_FRAME: u32 = 72;

// ---------------------------------------------------------------------------
// One data frame's bits
// ---------------------------------------------------------------------------

/// The K bits of one data frame, held the way step 1 asks for them.
///
/// "Represent the incoming K bits as an integer, R:
/// R = b0 + b1 x 2^1 + ... + b(K-1) x 2^(K-1)", where "b0 is first in time".
/// First in time is lowest in value, which is the opposite of the way a
/// codeword is written down, and getting it backwards gives a frame that
/// decodes to something plausible and wrong -- so the integer *is* the frame
/// here, and the bit order lives in one place.
///
/// It is kept as a value rather than a `Vec<bool>` because the data path runs
/// 667 frames a second and the arithmetic below wants the integer anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameBits {
    value: u128,
    len: u32,
}

impl FrameBits {
    /// A frame of `len` bits whose value is `value`, b0 the least significant.
    pub fn new(value: u128, len: u32) -> Self {
        debug_assert!(len <= LONGEST_FRAME, "a data frame does not hold that many bits");
        debug_assert!(value >> len == 0, "the value has bits above K");
        Self { value, len }
    }

    /// The same, from the bits in the order they go on the wire: `bits[0]` is
    /// b0, first in time.
    pub fn from_bits(bits: &[bool]) -> Self {
        debug_assert!(bits.len() as u32 <= LONGEST_FRAME, "a data frame is not that long");
        let mut value = 0u128;
        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                value |= 1u128 << i;
            }
        }
        Self { value, len: bits.len() as u32 }
    }

    /// K, the number of bits in the frame.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the frame carries no bits at all, which no rung of the 6.1
    /// ladder asks for.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// R, the integer of step 1.
    pub fn value(&self) -> u128 {
        self.value
    }

    /// Bit `i` of the frame, b0 first in time. Bits at or above K are zero.
    pub fn bit(&self, i: u32) -> bool {
        i < self.len && self.value >> i & 1 == 1
    }

    /// The bits in the order they go on the wire, b0 first.
    pub fn bits(self) -> impl Iterator<Item = bool> {
        (0..self.len).map(move |i| self.bit(i))
    }
}

// ---------------------------------------------------------------------------
// The twelve moduli
// ---------------------------------------------------------------------------

/// M0 to M11 and their product.
///
/// "The values of Mi and K shall satisfy the inequality 2^K <= M = product of
/// Mi for i = 0 to 11". The product is worked out once, in a u128, because it
/// is wanted on every frame by both the sign step and [`Self::fits`], and
/// because it is the number that overflows anything narrower.
///
/// [`Parameters::product`] is the same arithmetic on the CPd's own array; this
/// is the form the encoder and the decoder carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moduli12 {
    moduli: [u8; UP_INTERVALS],
    product: u128,
}

impl Moduli12 {
    /// The moduli of one data frame, interval 0 first.
    pub fn new(moduli: [u8; UP_INTERVALS]) -> Self {
        let product = moduli.iter().map(|&m| u128::from(m)).product();
        Self { moduli, product }
    }

    /// M0 to M11, interval 0 first.
    pub fn values(&self) -> &[u8; UP_INTERVALS] {
        &self.moduli
    }

    /// M_i, the modulus of data frame interval `i`.
    pub fn modulus(&self, i: usize) -> u8 {
        self.moduli[i % UP_INTERVALS]
    }

    /// M, the product of the twelve.
    ///
    /// It reaches 255^12, about 2^96, so it does not fit in a u64 -- see
    /// `seventy_two_bits_need_u128`.
    pub fn product(&self) -> u128 {
        self.product
    }

    /// Whether K bits fit: "2^K <= M".
    ///
    /// A modulus of zero makes the product zero and nothing fits, which is the
    /// answer that keeps the divisions below away from a zero divisor.
    pub fn fits(&self, k: u32) -> bool {
        if k >= 128 {
            return false;
        }
        self.product >= 1u128 << k
    }

    /// The largest K these moduli can carry, which is the top rung of the 6.1
    /// ladder they allow.
    pub fn capacity(&self) -> u32 {
        if self.product == 0 {
            return 0;
        }
        (0..=127).rev().find(|&k| self.product >= 1u128 << k).unwrap_or(0)
    }
}

impl From<&Parameters> for Moduli12 {
    /// CPd bits 52:152, once `v92::sequences` has read them (AD-3).
    fn from(parameters: &Parameters) -> Self {
        Self::new(parameters.moduli)
    }
}

// ---------------------------------------------------------------------------
// The encoder
// ---------------------------------------------------------------------------

/// The modulus encoder of 6.4.1: K scrambled bits in, K0 to K11 out.
///
/// The only memory is d(f - 1), the differential sign bit, which is why the
/// moduli are an argument and not a field: a rate renegotiation changes them
/// between one frame and the next without disturbing the sign chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoder {
    d_prev: bool,
}

impl Default for Encoder {
    fn default() -> Self {
        Self { d_prev: D_BEFORE_THE_FIRST_FRAME }
    }
}

impl Encoder {
    /// A fresh encoder, its memory zeroed as 8.7.1 asks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Zero the memory again, for B1u (8.7.1) or a fast parameter exchange
    /// (9.9.2).
    pub fn reset(&mut self) {
        self.d_prev = D_BEFORE_THE_FIRST_FRAME;
    }

    /// d(f - 1), the bit the next frame will be reflected on.
    pub fn d(&self) -> bool {
        self.d_prev
    }

    /// Set d(f - 1) by hand, for a caller that has to pick the sign chain up
    /// where something else left it.
    pub fn set_d(&mut self, d: bool) {
        self.d_prev = d;
    }

    /// Steps 1 to 6 of 6.4.1, for one data frame.
    ///
    /// The frame is refused, and the memory left alone, when the moduli cannot
    /// carry K bits -- 6.4.1's one "shall", and the case where step 5 would
    /// otherwise divide by a zero modulus.
    pub fn encode(
        &mut self,
        frame: FrameBits,
        moduli: &Moduli12,
    ) -> Result<[u8; UP_INTERVALS], &'static str> {
        if !moduli.fits(frame.len()) {
            return Err("the data frame carries more bits than the moduli can hold");
        }
        let m = moduli.product();
        // Step 1: R is the frame itself, b0 the least significant.
        let r = frame.value();
        // Step 2: "s(f) = 0 if R <= (M - 1)/2; s(f) = 1 if R > (M - 1)/2",
        // which in integers is 2R > M - 1. 2R cannot overflow: R < 2^K <= M,
        // and M is under 2^96.
        let s = 2 * r > m - 1;
        // Step 3: "d(f) = s(f) + d(f - 1), where + represents modulo 2
        // addition".
        let d = s ^ self.d_prev;
        // Step 4, on the previous frame's bit as printed (see
        // STEP4_USES_PREVIOUS_D).
        let reflect = if STEP4_USES_PREVIOUS_D { self.d_prev } else { d };
        let mut r0 = if reflect { m - 1 - r } else { r };
        // Step 5: the mixed radix, interval 0 first. "Ki = Ri modulo Mi, where
        // 0 <= Ki < Mi; R(i+1) = (Ri - Ki)/Mi".
        let mut labels = [0u8; UP_INTERVALS];
        for (label, &modulus) in labels.iter_mut().zip(moduli.values()) {
            let modulus = u128::from(modulus);
            *label = (r0 % modulus) as u8;
            r0 /= modulus;
        }
        self.d_prev = d;
        Ok(labels)
    }
}

// ---------------------------------------------------------------------------
// The decoder
// ---------------------------------------------------------------------------

/// 6.4.1 read backwards, for our own digital modem.
///
/// The mixed radix goes back together most significant interval last, the
/// reflection of step 4 is its own inverse, and then the sign chain is picked
/// up again -- from **R**, not from the half R0 fell in.
///
/// Those two differ exactly when M is odd and R is the middle value
/// (M - 1)/2. There the reflection leaves R0 where it was, so a decoder that
/// shortcuts by taking the half from R0 and undoing the reflection reads
/// s(f) as 1 when it was 0, and every frame after it is mapped the wrong way
/// up. The test of that case is named for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoder {
    d_prev: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self { d_prev: D_BEFORE_THE_FIRST_FRAME }
    }
}

impl Decoder {
    /// A fresh decoder, its memory zeroed as the transmitter's is (8.7.1).
    pub fn new() -> Self {
        Self::default()
    }

    /// Zero the memory again, for B1u or a fast parameter exchange.
    pub fn reset(&mut self) {
        self.d_prev = D_BEFORE_THE_FIRST_FRAME;
    }

    /// d(f - 1), as this decoder believes it.
    pub fn d(&self) -> bool {
        self.d_prev
    }

    /// Start the sign chain somewhere other than zero -- inverted, for a line
    /// whose polarity is the other way up.
    pub fn set_d(&mut self, d: bool) {
        self.d_prev = d;
    }

    /// K0 to K11 and the moduli back to the frame's K bits.
    ///
    /// The memory is left alone on every refusal, so a frame the Viterbi got
    /// wrong does not also drag the sign chain off for every frame after it.
    pub fn decode(
        &mut self,
        labels: &[u8; UP_INTERVALS],
        moduli: &Moduli12,
        k: u32,
    ) -> Result<FrameBits, &'static str> {
        if !STEP4_USES_PREVIOUS_D {
            return Err("step 4's other reading folds two frames onto one and cannot be decoded");
        }
        if !moduli.fits(k) {
            return Err("the data frame carries more bits than the moduli can hold");
        }
        let m = moduli.product();
        // Step 5 backwards: R0 = sum of Ki times the product of the moduli
        // below it, which is the mixed radix read from interval 11 down.
        let mut r0: u128 = 0;
        for (&label, &modulus) in labels.iter().zip(moduli.values()).rev() {
            if label >= modulus {
                return Err("a label is outside its interval's modulus");
            }
            r0 = r0 * u128::from(modulus) + u128::from(label);
        }
        // Step 4 backwards: the reflection is its own inverse.
        let r = if self.d_prev { m - 1 - r0 } else { r0 };
        if r >> k != 0 {
            return Err("the frame holds a larger value than the rate carries");
        }
        // Steps 2 and 3 again, on R.
        let s = 2 * r > m - 1;
        let d = s ^ self.d_prev;
        self.d_prev = d;
        Ok(FrameBits::new(r, k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::{CONSTELLATION_FRAME, Filters, Trellis, up_bits};

    /// The 64-bit LCG the V.90 tests use, so a failure repeats exactly.
    #[derive(Debug)]
    struct Lcg(u64);

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn step(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0
        }

        /// A value of `k` bits, taken from the halves of two draws that carry
        /// the state's better bits.
        fn value(&mut self, k: u32) -> u128 {
            let high = u128::from(self.step() >> 8);
            let low = u128::from(self.step() >> 8);
            let bits = (high << 56) | low;
            if k == 0 { 0 } else { bits & ((1u128 << k) - 1) }
        }
    }

    /// Moduli whose product is exactly 2^72, so that every 72-bit frame is a
    /// legal R and R = M - 1 is one of them -- the only shape of moduli for
    /// which it is, since 2^K <= M and M - 1 < 2^K together force M = 2^K.
    const EXACT: [u8; UP_INTERVALS] = [64; UP_INTERVALS];

    /// An odd product, a little over 2^72: the case where the middle value
    /// (M - 1)/2 is a whole number and a legal frame.
    const ODD: [u8; UP_INTERVALS] = [67, 63, 69, 65, 61, 67, 63, 69, 65, 61, 67, 63];

    /// Moduli of the shape a real route gives, two of them robbed.
    const MIXED: [u8; UP_INTERVALS] = [72, 68, 64, 66, 70, 64, 72, 68, 64, 66, 70, 64];

    /// 6.4.1 steps 1 to 6 and their inverse, over every rung of the 6.1 ladder
    /// and both parities of the product. The special values are the ones the
    /// sign step turns on: R = 0, the largest frame, R = M - 1 where the
    /// moduli allow it, and the middle value (M - 1)/2.
    #[test]
    fn random_frames_round_trip_for_even_and_odd_products() {
        let mut saw_top = false;
        let mut saw_last = false;
        let mut saw_middle = false;
        for values in [EXACT, ODD, MIXED] {
            let moduli = Moduli12::new(values);
            assert!(moduli.capacity() >= up_bits(19), "{values:?} cannot reach drn 19");
            for k in up_bits(1)..=up_bits(19) {
                let mut encoder = Encoder::new();
                let mut decoder = Decoder::new();
                let mut rng = Lcg::new(0x5eed_0f00 + u64::from(k));
                let top = (1u128 << k) - 1;
                let middle = (moduli.product() - 1) / 2;
                for frame in 0..2000u32 {
                    let r = match frame {
                        0 => 0,
                        1 => top,
                        2 if middle <= top => middle,
                        3 if moduli.product() - 1 <= top => moduli.product() - 1,
                        _ => rng.value(k),
                    };
                    saw_top |= r == top;
                    saw_last |= r == moduli.product() - 1;
                    saw_middle |= moduli.product() % 2 == 1 && r == middle;
                    let sent = FrameBits::new(r, k);
                    let labels = encoder.encode(sent, &moduli).expect("a frame that fits");
                    for (i, &label) in labels.iter().enumerate() {
                        assert!(label < moduli.modulus(i), "K{i} = {label} is outside M{i}");
                    }
                    let heard = decoder.decode(&labels, &moduli, k).expect("a frame that fits");
                    assert_eq!(heard, sent, "{values:?} at K = {k}, frame {frame}, R = {r}");
                    assert_eq!(decoder.d(), encoder.d(), "the sign chains parted at frame {frame}");
                }
            }
        }
        assert!(saw_top, "no frame took the largest value K holds");
        assert!(saw_last, "no frame took R = M - 1");
        assert!(saw_middle, "no frame took the middle value of an odd product");
    }

    /// Step 2 splits [0, M) at (M - 1)/2, and the half R fell in is what step
    /// 3 carries into the next frame. From a zeroed memory d(f) is s(f), so
    /// the encoder's own d is the reading.
    #[test]
    fn the_sign_splits_the_frame_at_the_middle() {
        // M = 8, so (M - 1)/2 = 3.5: R up to 3 is the low half.
        let moduli = Moduli12::new([2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(moduli.product(), 8);
        for (r, sign) in [(0u128, false), (3, false), (4, true), (7, true)] {
            let mut encoder = Encoder::new();
            encoder.encode(FrameBits::new(r, 3), &moduli).expect("a frame that fits");
            assert_eq!(encoder.d(), sign, "R = {r} fell in the wrong half");
        }
        // An interval whose modulus is 1 carries nothing at all.
        let mut encoder = Encoder::new();
        let labels = encoder.encode(FrameBits::new(7, 3), &moduli).expect("a frame that fits");
        assert_eq!(labels, [1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    /// The middle value of an odd product is the one frame that step 4's
    /// reflection leaves exactly where it was, so R0 cannot say which half R
    /// was in. The decoder takes s(f) from R and gets it right; the shortcut
    /// of taking it from R0's half and undoing the reflection gets it wrong.
    #[test]
    fn the_middle_value_of_an_odd_product_keeps_the_sign_chain() {
        let moduli = Moduli12::new([3, 5, 7, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        let m = moduli.product();
        assert_eq!(m, 105, "the product is not the odd one the test wants");
        let middle = (m - 1) / 2;
        assert_eq!(middle, 52);
        let frame = FrameBits::new(middle, 6);

        // With d(f - 1) = 1 the reflection is M - 1 - R, which is R itself.
        let mut encoder = Encoder::new();
        encoder.set_d(true);
        let reflected = encoder.encode(frame, &moduli).expect("a frame that fits");
        let mut encoder = Encoder::new();
        let plain = encoder.encode(frame, &moduli).expect("a frame that fits");
        assert_eq!(reflected, plain, "the middle value moved under the reflection");

        // s(f) = 0 either way, because 2R is M - 1 and not more, so d(f)
        // follows d(f - 1).
        let mut decoder = Decoder::new();
        decoder.set_d(true);
        assert_eq!(decoder.decode(&reflected, &moduli, 6), Ok(frame));
        assert!(decoder.d(), "the sign chain did not carry the previous frame's bit");

        // The shortcut: read the half from R0 and undo the reflection with
        // d(f - 1). R0 is 52, which is the low half, so the shortcut reads
        // s(f) = 1 where R itself makes it 0.
        let r0: u128 = reflected
            .iter()
            .zip(moduli.values())
            .rev()
            .fold(0, |acc, (&label, &modulus)| acc * u128::from(modulus) + u128::from(label));
        assert_eq!(r0, middle);
        let true_sign = 2 * middle > m - 1;
        assert!(!true_sign, "the middle value is not in the low half after all");
        let shortcut_sign = (2 * r0 > m - 1) ^ true;
        assert_ne!(shortcut_sign, true_sign, "the shortcut agreed, and the test proves nothing");
    }

    /// Why the differential step is there. An inverted line turns every index
    /// eta into -eta-1, so every K_i arrives as M_i - 1 - K_i and R0 arrives
    /// as M - 1 - R0. A decoder whose d starts inverted reflects it straight
    /// back and reads the bits that were sent, frame after frame.
    #[test]
    fn an_inverted_channel_decodes_with_the_decoder_started_inverted() {
        let moduli = Moduli12::new(MIXED);
        let k = up_bits(13);
        let mut encoder = Encoder::new();
        let mut decoder = Decoder::new();
        decoder.set_d(!D_BEFORE_THE_FIRST_FRAME);
        let mut rng = Lcg::new(0x1_1111);
        for frame in 0..500u32 {
            let sent = FrameBits::new(rng.value(k), k);
            let labels = encoder.encode(sent, &moduli).expect("a frame that fits");
            let inverted: [u8; UP_INTERVALS] =
                std::array::from_fn(|i| moduli.modulus(i) - 1 - labels[i]);
            let heard = decoder.decode(&inverted, &moduli, k).expect("a frame that fits");
            assert_eq!(heard, sent, "frame {frame} of an inverted line");
            assert_eq!(decoder.d(), !encoder.d(), "the inversion did not stay inverted");
        }
    }

    /// The alternative reading of step 4, and why it is not one a decoder
    /// could mirror: reflecting on d(f) folds [0, M) into half of itself, so
    /// two frames share an R0.
    #[test]
    fn the_alternative_reading_of_step_4_is_not_one_to_one() {
        let m: u128 = 105;
        let mut seen = std::collections::HashMap::new();
        let mut collisions = 0;
        for r in 0..m {
            let s = 2 * r > m - 1;
            let d = s ^ D_BEFORE_THE_FIRST_FRAME;
            // Step 4 as the alternative would have it.
            let r0 = if d { m - 1 - r } else { r };
            if seen.insert(r0, r).is_some() {
                collisions += 1;
            }
        }
        assert_eq!(collisions, 52, "the fold is not the one the doc describes");
        assert_eq!(seen.len(), 53, "{} of {m} values of R0 are reachable", seen.len());
        // The pair the doc names: 51 and 53 both land on 51.
        assert_eq!(seen.get(&51), Some(&53));

        // As printed, on d(f - 1), the map is a bijection for either memory.
        for d_prev in [false, true] {
            let reached: std::collections::HashSet<u128> =
                (0..m).map(|r| if d_prev { m - 1 - r } else { r }).collect();
            assert_eq!(reached.len() as u128, m, "d(f - 1) = {d_prev} lost a frame");
        }

        // And the encoder takes the printed reading: R = 53 is over the
        // middle, so the alternative would reflect it to 51 in the same frame,
        // where d(-1) = 0 leaves it at 53.
        let moduli = Moduli12::new([3, 5, 7, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        let frame = FrameBits::new(53, 6);
        let labels = Encoder::new().encode(frame, &moduli).expect("a frame that fits");
        let r0: u128 = labels
            .iter()
            .zip(moduli.values())
            .rev()
            .fold(0, |acc, (&label, &modulus)| acc * u128::from(modulus) + u128::from(label));
        assert_eq!(r0, 53, "step 4 is no longer read on the previous frame's d");
    }

    /// "The values of Mi and K shall satisfy the inequality 2^K <= M". Moduli
    /// that cannot carry the rate are refused, at both ends and without
    /// touching the sign chain.
    #[test]
    fn a_frame_that_does_not_fit_is_refused() {
        // Twelve intervals of three carry 19 bits; the slowest rung wants 36.
        let thin = Moduli12::new([3; UP_INTERVALS]);
        assert_eq!(thin.capacity(), 19);
        assert!(!thin.fits(up_bits(1)));
        let mut encoder = Encoder::new();
        encoder.set_d(true);
        let before = encoder;
        assert_eq!(
            encoder.encode(FrameBits::new(1, 36), &thin),
            Err("the data frame carries more bits than the moduli can hold")
        );
        assert_eq!(encoder, before, "a refused frame clocked the sign chain");
        let mut decoder = Decoder::new();
        decoder.set_d(true);
        let before = decoder;
        assert_eq!(
            decoder.decode(&[0; UP_INTERVALS], &thin, 36),
            Err("the data frame carries more bits than the moduli can hold")
        );
        assert_eq!(decoder, before, "a refused frame clocked the sign chain");

        // An interval with no modulus carries nothing, so nothing fits and
        // step 5 never reaches a zero divisor.
        let mut broken = MIXED;
        broken[5] = 0;
        let broken = Moduli12::new(broken);
        assert_eq!(broken.product(), 0);
        assert_eq!(broken.capacity(), 0);
        assert!(!broken.fits(0));
        assert!(Encoder::new().encode(FrameBits::new(0, 0), &broken).is_err());

        // And a label outside its own interval is not a frame this modulus
        // ever produced.
        let moduli = Moduli12::new(MIXED);
        let mut labels = [0u8; UP_INTERVALS];
        labels[2] = moduli.modulus(2);
        assert_eq!(
            Decoder::new().decode(&labels, &moduli, up_bits(1)),
            Err("a label is outside its interval's modulus")
        );
    }

    /// 8.7.1: the memories are zero before B1u, so the first frame of a chain
    /// is the unreflected one. Worked by hand on M = 3 x 5 x 7 x 2^9 = 53 760,
    /// K = 15, R = 30 000: 30 000 is over the middle, so s(0) = 1, but d(-1)
    /// is 0 and R0 is R -- 30 000 = 0 + 3(0 + 5(5 + 7(1 + 2(0 + 2(1 + ...))))).
    /// The next frame carries the same R with d(f - 1) = 1, so R0 is
    /// 53 759 - 30 000 = 23 759 and the labels are different.
    #[test]
    fn the_memory_starts_at_zero() {
        let moduli = Moduli12::new([3, 5, 7, 2, 2, 2, 2, 2, 2, 2, 2, 2]);
        assert_eq!(moduli.product(), 53_760);
        assert_eq!(moduli.capacity(), 15);
        let frame = FrameBits::new(30_000, 15);

        let mut encoder = Encoder::new();
        assert!(!encoder.d(), "the memory did not start at zero");
        let first = encoder.encode(frame, &moduli).expect("a frame that fits");
        assert_eq!(first, [0, 0, 5, 1, 0, 1, 1, 1, 0, 0, 0, 1]);
        assert!(encoder.d(), "R was over the middle and the sign did not turn");

        let second = encoder.encode(frame, &moduli).expect("a frame that fits");
        assert_eq!(second, [2, 4, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0]);
        assert!(!encoder.d(), "the sign turned back the wrong way");

        // And reset puts it back where B1u needs it.
        encoder.reset();
        assert_eq!(encoder.encode(frame, &moduli), Ok(first));

        let mut decoder = Decoder::new();
        assert_eq!(decoder.decode(&first, &moduli, 15), Ok(frame));
        assert_eq!(decoder.decode(&second, &moduli, 15), Ok(frame));
        decoder.reset();
        assert_eq!(decoder.decode(&first, &moduli, 15), Ok(frame));
    }

    /// Step 1 makes the first bit in time the least significant, which is the
    /// opposite of the way the number is written down.
    #[test]
    fn the_first_bit_in_time_is_the_lowest_in_value() {
        let frame = FrameBits::from_bits(&[true, false, false, true]);
        assert_eq!(frame.value(), 9, "b0 was not the least significant bit");
        assert_eq!(frame.len(), 4);
        assert!(!frame.is_empty());
        assert_eq!(frame.bits().collect::<Vec<_>>(), vec![true, false, false, true]);
        assert!(!frame.bit(4), "a bit above K is not part of the frame");
        assert_eq!(FrameBits::new(9, 4), frame);

        // In a frame, that puts b0 in interval 0's label.
        let moduli = Moduli12::new([16; UP_INTERVALS]);
        let labels = Encoder::new().encode(frame, &moduli).expect("a frame that fits");
        assert_eq!(labels[0], 9);
    }

    /// P-11: M reaches 255^12, which is about 2^96, so nothing narrower than a
    /// u128 holds the product -- and 72 bits is what the top of the ladder
    /// asks the moduli to carry.
    #[test]
    fn seventy_two_bits_need_u128() {
        let widest = Moduli12::new([255; UP_INTERVALS]);
        assert_eq!(widest.product(), 75_593_101_654_204_447_168_212_890_625);
        assert!(widest.product() > u128::from(u64::MAX), "the product fitted in a u64");
        assert_eq!(widest.capacity(), 95);
        assert_eq!(up_bits(19), LONGEST_FRAME);
        assert!(widest.fits(LONGEST_FRAME));

        // The top of the ladder, and the smallest moduli that carry it: twelve
        // sixes are not enough and twelve sixty-fours are exactly enough.
        assert!(!Moduli12::new([63; UP_INTERVALS]).fits(LONGEST_FRAME));
        assert_eq!(Moduli12::new(EXACT).product(), 1u128 << LONGEST_FRAME);
        assert!(Moduli12::new(EXACT).fits(LONGEST_FRAME));
        assert!(!Moduli12::new(EXACT).fits(LONGEST_FRAME + 1));
        // Nothing fits a K the shift itself could not take.
        assert!(!widest.fits(128));
    }

    /// AD-3: the moduli come from a `Parameters`, never from a `Cpd`.
    #[test]
    fn the_moduli_come_out_of_the_parameters() {
        let parameters = Parameters {
            drn: 13,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            gain: 0.1,
            moduli: MIXED,
            filters: Filters { z2: vec![1.0], ..Default::default() },
            sets: vec![(1..=72).map(|p| p * 100).collect()],
            indices: [0; CONSTELLATION_FRAME],
        };
        parameters.fits().expect("the parameters are self-consistent");
        let moduli = Moduli12::from(&parameters);
        assert_eq!(moduli.values(), &MIXED);
        assert_eq!(moduli.product(), parameters.product());
        assert!(moduli.fits(parameters.bits()), "drn 13 does not fit its own moduli");
    }
}
