//! Modulation parameter sequences (10.1.3.9, Tables 20 and 21).
//!
//! What each modem will accept for data mode, sent back and forth in phase 4:
//! the fastest rate each way, the trellis code, the shaping and non-linear
//! encoding the far transmitter is to use, which rates are enabled at all, and
//! -- in a Type 1 sequence -- the three precoding coefficients the far
//! transmitter's precoder is to use. MP' is the same with the acknowledge bit
//! set, saying the far end's own has arrived.

use super::info::crc;

/// Bits of a Type 0 sequence and of a Type 1 (Tables 20 and 21).
pub const TYPE0_BITS: usize = 88;
pub const TYPE1_BITS: usize = 188;

/// "Frame sync: 11111111111111111", seventeen ones.
pub const SYNC_ONES: usize = 17;

/// The trellis code the far transmitter is to use (bits 29:30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trellis {
    #[default]
    States16,
    States32,
    States64,
}

/// A precoding coefficient: 16 bits of two's complement, 14 after the point
/// (9.6.2).
pub type Coefficient = (i16, i16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mp {
    /// Bits 20:23 and 24:27: the fastest rate each way, as a multiple of 2400.
    pub call_to_answer: u8,
    pub answer_to_call: u8,
    /// Bit 28.
    pub auxiliary: bool,
    /// Bits 29:30.
    pub trellis: Trellis,
    /// Bit 31: theta of 0.3125 for the far transmitter's non-linear encoder
    /// rather than 0.
    pub non_linear: bool,
    /// Bit 32: expanded rather than minimum constellation shaping.
    pub expanded_shaping: bool,
    /// Bit 33: this end has received the far end's MP -- which makes this an
    /// MP'.
    pub acknowledge: bool,
    /// Bits 35:48, one for each rate from 2400 to 33 600, least first.
    pub rates: u16,
    /// Bit 50.
    pub asymmetric: bool,
    /// Type 1 only: h(1), h(2) and h(3), each real then imaginary.
    pub precoding: Option<[Coefficient; 3]>,
}

fn put(bits: &mut Vec<bool>, value: u32, width: usize) {
    for i in 0..width {
        bits.push(value >> i & 1 == 1);
    }
}

fn get(bits: &[bool], from: usize, width: usize) -> u32 {
    (0..width).fold(0, |value, i| value | u32::from(bits[from + i]) << i)
}

/// Where the start bits are, which are left out of the CRC along with the
/// sync and the fill.
fn start_bits(length: usize) -> &'static [usize] {
    if length == TYPE0_BITS {
        &[17, 34, 51, 68]
    } else {
        &[17, 34, 51, 68, 85, 102, 119, 136, 153, 170]
    }
}

/// Where the CRC starts.
fn crc_at(length: usize) -> usize {
    if length == TYPE0_BITS { 69 } else { 171 }
}

/// The bits the CRC is over: everything from bit 18 up to the CRC, less the
/// start bits.
fn covered(bits: &[bool], length: usize) -> Vec<bool> {
    let starts = start_bits(length);
    (SYNC_ONES + 1..crc_at(length)).filter(|i| !starts.contains(i)).map(|i| bits[i]).collect()
}

impl Mp {
    /// An MP' of this.
    pub fn acknowledged(mut self) -> Self {
        self.acknowledge = true;
        self
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let length = if self.precoding.is_some() { TYPE1_BITS } else { TYPE0_BITS };
        let mut bits = vec![true; SYNC_ONES];
        bits.push(false); // 17: start
        bits.push(self.precoding.is_some()); // 18: type
        bits.push(false); // 19: reserved
        put(&mut bits, u32::from(self.call_to_answer), 4);
        put(&mut bits, u32::from(self.answer_to_call), 4);
        bits.push(self.auxiliary);
        put(
            &mut bits,
            match self.trellis {
                Trellis::States16 => 0,
                Trellis::States32 => 1,
                Trellis::States64 => 2,
            },
            2,
        );
        bits.push(self.non_linear);
        bits.push(self.expanded_shaping);
        bits.push(self.acknowledge);
        bits.push(false); // 34: start
        put(&mut bits, u32::from(self.rates & 0x3fff), 15); // 35:49, 49 reserved
        bits.push(self.asymmetric);
        bits.push(false); // 51: start
        match self.precoding {
            None => {
                put(&mut bits, 0, 16); // 52:67 reserved
                bits.push(false); // 68: start
            }
            Some(h) => {
                for (re, im) in h {
                    put(&mut bits, u32::from(re as u16), 16);
                    bits.push(false);
                    put(&mut bits, u32::from(im as u16), 16);
                    bits.push(false);
                }
                put(&mut bits, 0, 16); // 154:169 reserved
                bits.push(false); // 170: start
            }
        }
        debug_assert_eq!(bits.len(), crc_at(length));
        let check = crc(&covered(&bits, length));
        put(&mut bits, u32::from(check), 16);
        bits.resize(length, false); // fill
        bits
    }

    /// An MP out of `bits`, which start at its frame sync, if its start bits
    /// are where they should be and its CRC checks.
    ///
    /// The fill after the CRC need not be there.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if bits.len() < SYNC_ONES + 2 || bits[..SYNC_ONES].iter().any(|b| !b) || bits[SYNC_ONES] {
            return None;
        }
        let type1 = bits[18];
        let length = if type1 { TYPE1_BITS } else { TYPE0_BITS };
        if bits.len() < crc_at(length) + 16 {
            return None;
        }
        if start_bits(length).iter().any(|&i| bits[i]) {
            return None;
        }
        let at = crc_at(length);
        if crc(&covered(bits, length)) != get(bits, at, 16) as u16 {
            return None;
        }
        let coefficient = |from: usize| -> Coefficient { (get(bits, from, 16) as u16 as i16, get(bits, from + 17, 16) as u16 as i16) };
        Some(Self {
            call_to_answer: get(bits, 20, 4) as u8,
            answer_to_call: get(bits, 24, 4) as u8,
            auxiliary: bits[28],
            trellis: match get(bits, 29, 2) {
                1 => Trellis::States32,
                2 => Trellis::States64,
                _ => Trellis::States16,
            },
            non_linear: bits[31],
            expanded_shaping: bits[32],
            acknowledge: bits[33],
            rates: get(bits, 35, 14) as u16,
            asymmetric: bits[50],
            precoding: type1.then(|| [coefficient(52), coefficient(86), coefficient(120)]),
        })
    }

    /// The rate mask with every rate from 2400 up to `highest` times 2400.
    pub fn rates_up_to(highest: u8) -> u16 {
        (1u16 << highest.min(14)) - 1
    }
}

/// Finds MP sequences, and E, in a stream of descrambled bits.
#[derive(Debug, Clone, Default)]
pub struct Finder {
    bits: Vec<bool>,
    ones: usize,
}

/// What a finder found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Mp(Mp),
    /// Twenty ones in a row: more than an MP's sync ever runs to.
    E,
}

impl Finder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bit: bool) -> Option<Found> {
        self.ones = if bit { self.ones + 1 } else { 0 };
        self.bits.push(bit);
        // An MP is sync, a start bit and the type; a sequence is only looked
        // at once all of it could be here.
        if self.bits.len() > TYPE1_BITS + SYNC_ONES + 2 {
            self.bits.remove(0);
        }
        if self.ones == crate::v34::signals::E_BITS {
            self.bits.clear();
            return Some(Found::E);
        }
        for length in [TYPE0_BITS, TYPE1_BITS] {
            // Checked when the CRC is in; the fill after it is not waited for.
            let without_fill = length - if length == TYPE0_BITS { 3 } else { 1 };
            if self.bits.len() < without_fill {
                continue;
            }
            let start = self.bits.len() - without_fill;
            let candidate = &self.bits[start..];
            // The sync is 17 ones exactly: an eighteenth in front would be
            // part of something else.
            if start > 0 && self.bits[start - 1] {
                continue;
            }
            if let Some(mp) = Mp::from_bits(candidate)
                && (length == TYPE1_BITS) == candidate[18]
            {
                self.bits.clear();
                self.ones = 0;
                return Some(Found::Mp(mp));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours() -> Mp {
        Mp {
            call_to_answer: 14,
            answer_to_call: 14,
            auxiliary: false,
            trellis: Trellis::States16,
            non_linear: false,
            expanded_shaping: false,
            acknowledge: false,
            rates: Mp::rates_up_to(14),
            asymmetric: true,
            precoding: None,
        }
    }

    #[test]
    fn the_two_types_are_the_lengths_their_tables_give() {
        assert_eq!(ours().to_bits().len(), TYPE0_BITS);
        let with = Mp { precoding: Some([(1, -1), (16383, -16384), (0, 7)]), ..ours() };
        assert_eq!(with.to_bits().len(), TYPE1_BITS);
    }

    #[test]
    fn fields_are_where_table_20_puts_them() {
        let bits = ours().to_bits();
        assert!(bits[..17].iter().all(|b| *b));
        assert!(!bits[17] && !bits[18], "start bit, then type 0");
        assert_eq!(get(&bits, 20, 4), 14);
        assert_eq!(get(&bits, 24, 4), 14);
        assert!(!bits[34] && !bits[51] && !bits[68]);
        // Bit 35 is 2400 and bit 48 is 33 600.
        assert!(bits[35] && bits[48] && !bits[49]);
        assert!(bits[50], "asymmetric");
        assert_eq!(&bits[85..88], &[false, false, false], "fill");
    }

    #[test]
    fn both_types_come_back_and_a_wrong_bit_does_not() {
        for mp in [ours(), ours().acknowledged(), Mp { precoding: Some([(1, -1), (16383, -16384), (0, 7)]), ..ours() }] {
            let bits = mp.to_bits();
            assert_eq!(Mp::from_bits(&bits), Some(mp));
            for i in 17..bits.len() - 3 {
                let mut spoiled = bits.clone();
                spoiled[i] = !spoiled[i];
                assert_ne!(Mp::from_bits(&spoiled), Some(mp), "bit {i}");
            }
        }
    }

    #[test]
    fn a_finder_picks_mps_and_e_out_of_a_stream() {
        let mut finder = Finder::new();
        let mut stream: Vec<bool> = (0..50).map(|i| i % 3 == 0).collect();
        stream.extend(ours().to_bits());
        stream.extend(ours().acknowledged().to_bits());
        stream.extend(std::iter::repeat_n(true, 20));
        let found: Vec<Found> = stream.iter().filter_map(|&b| finder.feed(b)).collect();
        assert_eq!(found, vec![Found::Mp(ours()), Found::Mp(ours().acknowledged()), Found::E]);
    }
}
