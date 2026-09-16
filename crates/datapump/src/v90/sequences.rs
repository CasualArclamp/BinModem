//! The framed sequences of V.90's phases 3 and 4: Jd (8.4.2), the DIL
//! descriptor that Ja repeats (8.3.1), CP (8.5.2), and MP as the digital
//! modem sends it (8.6.3).
//!
//! All four are built the way V.34 builds MP: seventeen ones of frame sync,
//! then the information in sixteen-bit blocks each behind a zero start bit,
//! then the CRC of 10.1.2.3.2/V.34 behind one more, then fill. The CRC is
//! over the information alone -- not the sync, not the start bits -- which is
//! what a real Conexant modem's Ja checks against, read off a recording.
//!
//! Every field is written "LSB:MSB", least significant first in time.

use crate::v34::info::crc;
use crate::v34::mp::Mp;

use super::sign::Redundancy;
use super::ucode::UCODES;

/// "Frame Sync: 11111111111111111".
pub const SYNC_ONES: usize = 17;

/// Information bits between start bits.
pub const BLOCK: usize = 16;

/// The sync, each block behind its start bit, and the CRC behind one: every
/// sequence here up to its fill.
fn frame(information: &[bool]) -> Vec<bool> {
    debug_assert_eq!(information.len() % BLOCK, 0, "information is whole blocks");
    let mut bits = vec![true; SYNC_ONES];
    for block in information.chunks(BLOCK) {
        bits.push(false);
        bits.extend_from_slice(block);
    }
    bits.push(false);
    put(&mut bits, u32::from(crc(information)), 16);
    bits
}

/// The information of a sequence of `blocks` blocks, if the sync, every start
/// bit and the CRC are what they should be. `bits` starts at the frame sync
/// and runs at least to the end of the CRC.
fn unframe(bits: &[bool], blocks: usize) -> Option<Vec<bool>> {
    let crc_at = SYNC_ONES + blocks * (BLOCK + 1) + 1;
    if bits.len() < crc_at + 16 || !bits[..SYNC_ONES].iter().all(|b| *b) {
        return None;
    }
    let mut information = Vec::with_capacity(blocks * BLOCK);
    for b in 0..=blocks {
        let start = SYNC_ONES + b * (BLOCK + 1);
        if bits[start] {
            return None;
        }
        if b < blocks {
            information.extend_from_slice(&bits[start + 1..start + 1 + BLOCK]);
        }
    }
    (crc(&information) == get(bits, crc_at, 16) as u16).then_some(information)
}

fn put(bits: &mut Vec<bool>, value: u32, width: usize) {
    for i in 0..width {
        bits.push(value >> i & 1 == 1);
    }
}

fn get(bits: &[bool], from: usize, width: usize) -> u32 {
    (0..width).fold(0, |value, i| value | u32::from(bits[from + i]) << i)
}

/// Downstream rates are numbered from 28 000 up in steps of 8000/6: the
/// mask of Jd and the `drn` of CP both count this way.
pub const DOWNSTREAM_RATES: usize = 22;

/// The downstream rate a data mode `drn` names: "(drn+20)*8000/6 in CP"
/// (Table 14/V.90), so 1 is 28 000 and 22 is 56 000. Zero is cleardown.
pub fn data_rate(drn: u8) -> Option<u32> {
    (1..=DOWNSTREAM_RATES as u8).contains(&drn).then(|| super::rate_for(u32::from(drn) + 20))
}

/// And the frame bits D it carries, which is `drn + 20`.
pub fn data_bits(drn: u8) -> usize {
    usize::from(drn) + 20
}

/// The phase 4 rate a CPt's `drn` names: "(drn+8)*8000/6 in CPt", which is
/// Table 17's range from 12 000 up.
pub fn training_bits(drn: u8) -> usize {
    usize::from(drn) + 8
}

/// Jd (Table 13/V.90): the digital modem's downstream capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Jd {
    /// Bit n set: the rate of `drn` n + 1 is "supported and enabled in the
    /// transmitter of the digital modem" -- 28 000 is bit 0, 56 000 bit 21.
    /// The table spreads them across a start bit, bits 18:33 and 35:40.
    pub rates: u32,
    /// Bit 47: CP, E and SCR in training go on 16 points rather than 4.
    pub sixteen_in_training: bool,
    /// Bit 48: the same in a rate renegotiation.
    pub sixteen_in_renegotiation: bool,
    /// Bits 49:50: "the digital modem's maximum lookahead for spectral
    /// shaping", 1 to 3.
    pub lookahead: u8,
}

/// Jd's length: sync, two blocks, the CRC and "Fill bits: 0000".
pub const JD_BITS: usize = 72;

/// J'd, "12 binary zeroes" that end Jd (8.4.3).
pub const JD_PRIME_BITS: usize = 12;

impl Jd {
    /// Every rate enabled.
    pub const ALL_RATES: u32 = (1 << DOWNSTREAM_RATES) - 1;

    pub fn to_bits(&self) -> Vec<bool> {
        let mut information = Vec::with_capacity(2 * BLOCK);
        put(&mut information, self.rates & 0xffff, 16);
        put(&mut information, self.rates >> 16 & 0x3f, 6);
        // Bits 41:46: "Reserved for ITU".
        put(&mut information, 0, 6);
        information.push(self.sixteen_in_training);
        information.push(self.sixteen_in_renegotiation);
        put(&mut information, u32::from(self.lookahead), 2);
        let mut bits = frame(&information);
        bits.resize(JD_BITS, false);
        bits
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let information = unframe(bits, 2)?;
        Some(Self {
            rates: get(&information, 0, 22),
            sixteen_in_training: information[28],
            sixteen_in_renegotiation: information[29],
            lookahead: get(&information, 30, 2) as u8,
        })
    }

    /// Whether the rate of `drn` is enabled.
    pub fn enables(&self, drn: u8) -> bool {
        (1..=DOWNSTREAM_RATES as u8).contains(&drn) && self.rates >> (drn - 1) & 1 == 1
    }
}

/// The DIL descriptor (Table 12/V.90): what the analogue modem asks the
/// digital modem to send so that it can learn the route (8.4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    /// SP, 1 to 128 bits: which sign each symbol of a segment has, the first
    /// bit for the first symbol. "0 shall represent negative and 1 shall
    /// represent positive."
    pub signs: Vec<bool>,
    /// TP, 1 to 128 bits: whether each symbol is the segment's reference
    /// (0) or its training symbol (1).
    pub training: Vec<bool>,
    /// H1 to H8: a segment training a code from Uchord c is (Hc + 1) * 6
    /// symbols long.
    pub h: [u8; 8],
    /// REF1 to REF8: the reference symbol's Ucode in a segment training a
    /// code from Uchord c.
    pub refs: [u8; 8],
    /// The training symbol of each segment, N of them, 0 to 255.
    pub ucodes: Vec<u8>,
}

impl Descriptor {
    /// A descriptor asking for no DIL at all: "When N = 0, DIL is not
    /// transmitted", and the patterns then have "no significance".
    pub fn none() -> Self {
        Self { signs: vec![false], training: vec![false], h: [0; 8], refs: [0; 8], ucodes: Vec::new() }
    }

    /// How long a segment training `ucode` is, in symbols (8.4.1).
    pub fn segment_length(&self, ucode: u8) -> usize {
        (usize::from(self.h[usize::from(ucode >> 4) & 7]) + 1) * 6
    }

    /// The symbols of one pass through the DIL, as (Ucode, positive).
    ///
    /// "The patterns are restarted at the beginning of each DIL-segment. The
    /// patterns are repeated independently within DIL-segments whose lengths
    /// exceed that of LSP or LTP."
    pub fn symbols(&self) -> impl Iterator<Item = (u8, bool)> + '_ {
        self.ucodes.iter().flat_map(move |&ucode| {
            let chord = usize::from(ucode >> 4) & 7;
            (0..self.segment_length(ucode)).map(move |n| {
                let positive = self.signs[n % self.signs.len()];
                let trained = self.training[n % self.training.len()];
                (if trained { ucode } else { self.refs[chord] }, positive)
            })
        })
    }

    /// Symbols in one pass.
    pub fn len(&self) -> usize {
        self.ucodes.iter().map(|&u| self.segment_length(u)).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ucodes.is_empty()
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let n = self.ucodes.len().min(255);
        let (signs, training) = if n == 0 {
            // "LSP - 1 = LTP - 1 = 0 when N = 0."
            (&[false][..], &[false][..])
        } else {
            (&self.signs[..self.signs.len().clamp(1, 128)], &self.training[..self.training.len().clamp(1, 128)])
        };
        let mut information = Vec::new();
        put(&mut information, n as u32, 8);
        put(&mut information, 0, 8);
        put(&mut information, signs.len() as u32 - 1, 7);
        information.push(false);
        put(&mut information, training.len() as u32 - 1, 7);
        information.push(false);
        // "When LSP is not a multiple of 16, zeroes shall be used to pad SP to
        // the next multiple of 16 bits", and the same for TP.
        for pattern in [signs, training] {
            information.extend_from_slice(pattern);
            let padded = pattern.len().div_ceil(BLOCK) * BLOCK;
            information.resize(information.len() + padded - pattern.len(), false);
        }
        // H1 to H8, then REF1 to REF8, then the training Ucodes: seven bits
        // each and a reserved bit after, two to a block, with "9 reserved bits
        // to fill the final 16 bits if N is odd".
        for value in self.h.iter().chain(self.refs.iter()).chain(self.ucodes[..n].iter()) {
            put(&mut information, u32::from(*value & 0x7f), 7);
            information.push(false);
        }
        let padded = information.len().div_ceil(BLOCK) * BLOCK;
        information.resize(padded, false);
        let mut bits = frame(&information);
        // "Fill bit: 0", and another if it takes one to make the length even.
        bits.push(false);
        if bits.len() % 2 == 1 {
            bits.push(false);
        }
        bits
    }

    /// A descriptor out of `bits`, which start at its frame sync.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if bits.len() < SYNC_ONES + 2 * (BLOCK + 1) {
            return None;
        }
        // The first two blocks say how long the rest is.
        let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
        let n = get(bits, at(0, 0), 8) as usize;
        let lsp = get(bits, at(1, 0), 7) as usize + 1;
        let ltp = get(bits, at(1, 8), 7) as usize + 1;
        let blocks = 2 + lsp.div_ceil(BLOCK) + ltp.div_ceil(BLOCK) + 8 + n.div_ceil(2);
        let information = unframe(bits, blocks)?;
        let mut from = 2 * BLOCK;
        let signs = information[from..from + lsp].to_vec();
        from += lsp.div_ceil(BLOCK) * BLOCK;
        let training = information[from..from + ltp].to_vec();
        from += ltp.div_ceil(BLOCK) * BLOCK;
        let seven = |k: usize| get(&information, from + 8 * k, 7) as u8;
        let h = std::array::from_fn(&seven);
        let refs = std::array::from_fn(|c| seven(8 + c));
        let ucodes = (0..n).map(|k| seven(16 + k)).collect();
        Some(Self { signs, training, h, refs, ucodes })
    }
}

/// A constellation mask: bit u set when the constellation includes Ucode u
/// (8.5.2).
pub type Mask = u128;

/// CP (Table 14/V.90): the constellations the analogue modem wants the
/// digital modem to send with, and how it wants them shaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cp {
    /// Bit 19: "0 indicates CPt; 1 indicates CP". A CPt names phase 4's
    /// training constellations; a CP names data mode's.
    pub data_mode: bool,
    /// Bits 20:24: the downstream rate. Zero is cleardown; otherwise see
    /// [`data_rate`] and [`training_bits`].
    pub drn: u8,
    /// Bit 30: a silent period is asked for -- CPs (9.6).
    pub silence: bool,
    /// Bits 31:32: Sr.
    pub redundancy: Redundancy,
    /// Bit 33: the far end's MP has arrived -- which makes this a CP'.
    pub acknowledge: bool,
    /// Bit 35: "Codec type: 0 = mu-law; 1 = A-law".
    pub a_law: bool,
    /// Bits 36:48: upstream rates the analogue modem's transmitter has
    /// enabled, 4800 in bit 0 up to 33 600 in bit 12.
    pub upstream_rates: u16,
    /// Bits 49:50: ld, the shaper's look-ahead.
    pub lookahead: u8,
    /// Bits 52:67: TRN1d's RMS at the digital modem's transmitter over its
    /// RMS at the codec's D/A convertor, unsigned Q3.13.
    pub trn1d_ratio: u16,
    /// Bits 69:76, 77:84, 86:93 and 94:101: a1, a2, b1 and b2 of the shaping
    /// filter, signed Q1.6.
    pub shaping: [i8; 4],
    /// Bits 103:127: which constellation each data frame interval uses.
    pub intervals: [u8; 6],
    /// The constellations the intervals name, index 0 first.
    pub constellations: Vec<Mask>,
    /// Bit 128 and what it adds: the constellations as they come out of the
    /// codec, where those differ from what the digital modem sends.
    pub codec: Option<Vec<Mask>>,
}

impl Default for Cp {
    fn default() -> Self {
        Self {
            data_mode: false,
            drn: 0,
            silence: false,
            redundancy: Redundancy::None,
            acknowledge: false,
            a_law: false,
            upstream_rates: 0,
            lookahead: 0,
            // A ratio of one: nothing between the transmitter and the codec.
            trn1d_ratio: 1 << 13,
            shaping: [0; 4],
            intervals: [0; 6],
            constellations: vec![0],
            codec: None,
        }
    }
}

/// Blocks before the constellations: bits 18:135.
const CP_HEADER_BLOCKS: usize = 7;

/// Blocks per constellation: one for each Uchord.
const MASK_BLOCKS: usize = 8;

impl Cp {
    /// A CP' of this.
    pub fn acknowledged(&self) -> Self {
        Self { acknowledge: true, ..self.clone() }
    }

    /// The frame bits D this CP's rate carries.
    pub fn frame_bits(&self) -> usize {
        if self.data_mode { data_bits(self.drn) } else { training_bits(self.drn) }
    }

    /// The Ucodes of the constellation data frame interval `i` uses, as the
    /// digital modem is to send them.
    pub fn points(&self, i: usize) -> Vec<u8> {
        let mask = self.constellations.get(usize::from(self.intervals[i])).copied().unwrap_or(0);
        (0..UCODES as u8).filter(|&u| mask >> u & 1 == 1).collect()
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let mut information = Vec::new();
        information.push(false); // 18: reserved
        information.push(self.data_mode);
        put(&mut information, u32::from(self.drn), 5);
        put(&mut information, 0, 5); // 25:29 reserved
        information.push(self.silence);
        put(&mut information, self.redundancy.spent() as u32, 2);
        information.push(self.acknowledge);

        information.push(self.a_law);
        put(&mut information, u32::from(self.upstream_rates), 13);
        put(&mut information, u32::from(self.lookahead), 2);

        put(&mut information, u32::from(self.trn1d_ratio), 16);
        for coefficient in self.shaping {
            put(&mut information, u32::from(coefficient as u8), 8);
        }
        for index in self.intervals {
            put(&mut information, u32::from(index), 4);
        }
        information.push(self.codec.is_some());
        put(&mut information, 0, 7); // 129:135 reserved

        // "Only the number of different constellations need to be sent", and
        // how many that is follows from the largest index.
        let count = usize::from(self.intervals.iter().copied().max().unwrap_or(0)) + 1;
        let masks = self.constellations.iter().chain(self.codec.iter().flatten());
        let wanted = if self.codec.is_some() { 2 * count } else { count };
        for mask in masks.take(wanted) {
            for chord in 0..8 {
                put(&mut information, (mask >> (16 * chord)) as u32 & 0xffff, 16);
            }
        }
        let mut bits = frame(&information);
        put(&mut bits, 0, 3); // "Fill bits: 000"
        bits
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        if bits.len() < SYNC_ONES + CP_HEADER_BLOCKS * (BLOCK + 1) {
            return None;
        }
        let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
        let intervals: [u8; 6] = std::array::from_fn(|i| {
            let (block, bit) = if i < 4 { (5, 4 * i) } else { (6, 4 * (i - 4)) };
            get(bits, at(block, bit), 4) as u8
        });
        // "An integer between 0 and 5"; anything else is not a CP.
        if intervals.iter().any(|&i| i > 5) {
            return None;
        }
        let codec = bits[at(6, 8)];
        let count = usize::from(*intervals.iter().max().unwrap_or(&0)) + 1;
        let masks = if codec { 2 * count } else { count };
        let information = unframe(bits, CP_HEADER_BLOCKS + MASK_BLOCKS * masks)?;
        let mask = |k: usize| -> Mask {
            let from = CP_HEADER_BLOCKS * BLOCK + k * MASK_BLOCKS * BLOCK;
            (0..MASK_BLOCKS).fold(0, |m, chord| m | Mask::from(get(&information, from + chord * BLOCK, 16)) << (16 * chord))
        };
        let constellations: Vec<Mask> = (0..count).map(mask).collect();
        let codec = codec.then(|| (count..2 * count).map(mask).collect());
        Some(Self {
            data_mode: information[1],
            drn: get(&information, 2, 5) as u8,
            silence: information[12],
            redundancy: match get(&information, 13, 2) {
                0 => Redundancy::None,
                1 => Redundancy::One,
                2 => Redundancy::Two,
                _ => Redundancy::Three,
            },
            acknowledge: information[15],
            a_law: information[16],
            upstream_rates: get(&information, 17, 13) as u16,
            lookahead: get(&information, 30, 2) as u8,
            trn1d_ratio: get(&information, 32, 16) as u16,
            shaping: std::array::from_fn(|k| get(&information, 48 + 8 * k, 8) as u8 as i8),
            intervals,
            constellations,
            codec,
        })
    }
}

/// Finds one kind of framed sequence in a stream of descrambled bits.
///
/// A sequence starts where a zero follows seventeen ones and not eighteen --
/// an eighteenth would make the run fill or the last of something else --
/// and how long it is follows from its first few blocks. Every place that
/// could be a start is kept until there are enough bits to read it there.
#[derive(Debug, Clone)]
struct Finder<T> {
    bits: Vec<bool>,
    ones: usize,
    /// Where candidates start, in `bits`.
    starts: Vec<usize>,
    /// Bits needed to know the length, and the length from them.
    header: usize,
    length: fn(&[bool]) -> usize,
    parse: fn(&[bool]) -> Option<T>,
}

impl<T> Finder<T> {
    fn new(header_blocks: usize, length: fn(&[bool]) -> usize, parse: fn(&[bool]) -> Option<T>) -> Self {
        Self { bits: Vec::new(), ones: 0, starts: Vec::new(), header: SYNC_ONES + header_blocks * (BLOCK + 1), length, parse }
    }

    fn feed(&mut self, bit: bool) -> Option<T> {
        if !bit && self.ones == SYNC_ONES {
            self.starts.push(self.bits.len() - SYNC_ONES);
        }
        self.ones = if bit { self.ones + 1 } else { 0 };
        self.bits.push(bit);
        let mut found = None;
        let bits = &self.bits;
        let (header, length, parse) = (self.header, self.length, self.parse);
        self.starts.retain(|&start| {
            if found.is_some() {
                return false;
            }
            let have = bits.len() - start;
            if have < header {
                return true;
            }
            let needed = length(&bits[start..]);
            if have < needed {
                return true;
            }
            found = parse(&bits[start..start + needed]);
            false
        });
        if found.is_some() {
            self.starts.clear();
        }
        // Keep what the oldest candidate needs, or the last sync's worth.
        let keep_from = self.starts.first().copied().unwrap_or(self.bits.len().saturating_sub(SYNC_ONES));
        if keep_from > 4096 {
            self.bits.drain(..keep_from);
            for start in &mut self.starts {
                *start -= keep_from;
            }
        }
        found
    }
}

/// A DIL descriptor's length from its first two blocks.
fn descriptor_length(bits: &[bool]) -> usize {
    let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
    let n = get(bits, at(0, 0), 8) as usize;
    let lsp = get(bits, at(1, 0), 7) as usize + 1;
    let ltp = get(bits, at(1, 8), 7) as usize + 1;
    let blocks = 2 + lsp.div_ceil(BLOCK) + ltp.div_ceil(BLOCK) + 8 + n.div_ceil(2);
    SYNC_ONES + (blocks + 1) * (BLOCK + 1)
}

/// A CP's length from its first seven blocks.
fn cp_length(bits: &[bool]) -> usize {
    let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
    let largest = (0..6)
        .map(|i| {
            let (block, bit) = if i < 4 { (5, 4 * i) } else { (6, 4 * (i - 4)) };
            get(bits, at(block, bit), 4) as usize
        })
        .max()
        .unwrap_or(0)
        .min(5);
    let masks = (largest + 1) * if bits[at(6, 8)] { 2 } else { 1 };
    SYNC_ONES + (CP_HEADER_BLOCKS + MASK_BLOCKS * masks + 1) * (BLOCK + 1)
}

/// Finds Ja's DIL descriptors.
#[derive(Debug, Clone)]
pub struct DescriptorFinder(Finder<Descriptor>);

impl Default for DescriptorFinder {
    fn default() -> Self {
        Self(Finder::new(2, descriptor_length, Descriptor::from_bits))
    }
}

impl DescriptorFinder {
    pub fn feed(&mut self, bit: bool) -> Option<Descriptor> {
        self.0.feed(bit)
    }
}

/// Finds CP sequences.
#[derive(Debug, Clone)]
pub struct CpFinder(Finder<Cp>);

impl Default for CpFinder {
    fn default() -> Self {
        Self(Finder::new(CP_HEADER_BLOCKS, cp_length, Cp::from_bits))
    }
}

impl CpFinder {
    pub fn feed(&mut self, bit: bool) -> Option<Cp> {
        self.0.feed(bit)
    }
}

/// An MP as the digital modem sends it (Table 16/V.90).
///
/// Table 16 is V.34's MP with the call-to-answer rate, the auxiliary channel
/// and the asymmetry bit reserved, and V.34's reading of it reads it: the
/// upstream rate comes out as V.34's answer-to-call rate, and the rate mask
/// with V.34's 2400 bit clear. What differs is the fill. V.90's MP goes
/// downstream in data frames, so it is padded with "0s to extend the MP
/// sequence length to the next multiple of 6 symbols" -- a whole number of
/// frames of D bits -- where V.34 pads to a fixed length.
pub fn mp_bits(mp: &Mp, frame_bits: usize) -> Vec<bool> {
    let v90 = Mp { call_to_answer: 0, auxiliary: false, asymmetric: false, rates: mp.rates & !1, ..*mp };
    let mut bits = v90.to_bits();
    // Up to and including "Fill bit: 0" after the CRC.
    let end = if mp.precoding.is_some() { 188 } else { 86 };
    bits.truncate(end);
    let frames = end.div_ceil(frame_bits.max(1));
    bits.resize(frames * frame_bits.max(1), false);
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_of(text: &str) -> Vec<bool> {
        text.bytes().filter(|b| !b.is_ascii_whitespace()).map(|b| b == b'1').collect()
    }

    /// Jd off a real digital modem, as the equaliser read it off the
    /// recording and descrambled it (tests/vectors/v90-56k.wav, 16.53 s).
    const SERVER_JD: &str = "11111111111111111 0 1111111111111111 0 1111110000000010 0 0111011011101110 0000";

    #[test]
    fn a_real_jd_checks_and_enables_every_rate() {
        let bits = bits_of(SERVER_JD);
        assert_eq!(bits.len(), JD_BITS);
        let jd = Jd::from_bits(&bits).expect("the CRC did not check");
        assert_eq!(jd.rates, Jd::ALL_RATES);
        assert!(!jd.sixteen_in_training && !jd.sixteen_in_renegotiation);
        assert_eq!(jd.lookahead, 1);
        assert_eq!(jd.to_bits(), bits, "our Jd is not the server's");
    }

    #[test]
    fn jd_names_its_rates_from_28000_to_56000() {
        assert_eq!(data_rate(1), Some(28_000));
        assert_eq!(data_rate(22), Some(56_000));
        assert_eq!(data_rate(0), None, "cleardown");
        assert_eq!(data_rate(23), None);
        let jd = Jd { rates: 1 << 16, ..Jd::default() };
        let bits = jd.to_bits();
        // 49 333 is the first rate past the start bit at 34.
        assert!(bits[35] && !bits[34]);
        assert!(jd.enables(17));
        assert_eq!(data_rate(17), Some(49_333));
        assert_eq!(Jd::from_bits(&bits), Some(jd));
    }

    /// The DIL descriptor a Conexant V.92 modem sent, read off the same
    /// recording: 147 segments, patterns of 126, and Ucodes 0 to 117 with
    /// UINFO after every fourth.
    fn conexant() -> Descriptor {
        let signs = bits_of(
            "000111011000101001011111010101000010110111100111001010110011000001101101011101000110010001000000100100110100111101110000111111",
        );
        let training = bits_of(
            "000000000000000000000000000000101010010101000000000000101010010101000000111111111111111111111111111111111111111111111111111111",
        );
        let mut ucodes = Vec::new();
        for u in 0..118u8 {
            ucodes.push(u);
            if ucodes.len() % 5 == 4 {
                ucodes.push(78);
            }
        }
        ucodes.truncate(147);
        Descriptor { signs, training, h: [20, 20, 20, 20, 20, 20, 11, 11], refs: [78; 8], ucodes }
    }

    #[test]
    fn a_real_dil_descriptor_is_1736_bits_and_checks() {
        let bits = conexant().to_bits();
        // The recording repeats every 1736 bits.
        assert_eq!(bits.len(), 1736);
        assert_eq!(Descriptor::from_bits(&bits), Some(conexant()));
        // Its CRC, as the modem sent it, sits where Table 12 puts it:
        // 187 + beta + ceil(N/2)*17, with beta two patterns of eight blocks.
        let beta = 16 * 17;
        let crc_start = 187 + beta + 74 * 17;
        assert!(!bits[crc_start]);
        assert_eq!(crc_start + 17 + 2, 1736);
    }

    #[test]
    fn the_real_dil_is_as_long_as_the_recording_says() {
        let dil = conexant();
        // Six Uchords at 126 symbols, two at 72, and every reference segment
        // at 126 because Ucode 78 is in Uchord 5.
        assert_eq!(dil.len(), 17_334);
        let symbols: Vec<(u8, bool)> = dil.symbols().collect();
        assert_eq!(symbols.len(), 17_334);
        // The first segment trains Ucode 0: thirty references, then the
        // pattern's mixture.
        assert!(symbols[..30].iter().all(|&(u, _)| u == 78));
        assert_eq!(symbols[30].0, 0);
        assert_eq!(symbols[31].0, 78);
        // Signs follow SP from its first bit.
        assert!(symbols[3].1);
        assert!(!symbols[0].1);
        // And the patterns restart at every segment.
        assert_eq!(symbols[126], (78, false));
    }

    #[test]
    fn no_dil_is_a_short_descriptor() {
        let none = Descriptor::none();
        let bits = none.to_bits();
        // Two blocks, one pattern block each, and four blocks each of H and
        // REF: twelve blocks.
        assert_eq!(bits.len(), 17 + 12 * 17 + 17 + 1 + 1);
        assert_eq!(bits.len() % 2, 0);
        let back = Descriptor::from_bits(&bits).unwrap();
        assert!(back.is_empty());
        assert_eq!(back.len(), 0);
    }

    #[test]
    fn an_odd_number_of_segments_is_padded_and_read_back() {
        let mut d = conexant();
        d.ucodes.truncate(5);
        d.signs.truncate(17);
        d.training.truncate(33);
        let bits = d.to_bits();
        assert_eq!(bits.len() % 2, 0);
        assert_eq!(Descriptor::from_bits(&bits), Some(d));
        // A wrong bit anywhere before the fill is caught.
        for i in 0..bits.len() - 2 {
            let mut spoiled = bits.clone();
            spoiled[i] = !spoiled[i];
            let back = Descriptor::from_bits(&spoiled);
            assert!(back.is_none() || back != Descriptor::from_bits(&bits), "bit {i}");
        }
    }

    fn a_cp() -> Cp {
        let mask: Mask = (24..112).fold(0, |m, u| m | 1 << u);
        let robbed: Mask = (24..112).step_by(2).fold(0, |m, u| m | 1 << u);
        Cp {
            data_mode: true,
            drn: 15,
            silence: false,
            redundancy: Redundancy::Two,
            acknowledge: false,
            a_law: false,
            upstream_rates: 0x0fff,
            lookahead: 1,
            trn1d_ratio: 1 << 13,
            shaping: [-64, 12, 63, -1],
            intervals: [0, 0, 0, 1, 0, 0],
            constellations: vec![mask, robbed],
            codec: None,
        }
    }

    #[test]
    fn a_cp_is_as_long_as_its_constellations_and_comes_back() {
        let cp = a_cp();
        let bits = cp.to_bits();
        // Two constellations: gamma is 136.
        assert_eq!(bits.len(), 292 + 136);
        assert_eq!(Cp::from_bits(&bits), Some(cp.clone()));
        // Table 14's absolute positions.
        assert!(bits[19], "bit 19 says CP rather than CPt");
        assert_eq!(get(&bits, 20, 5), 15);
        assert_eq!(get(&bits, 31, 2), 2);
        assert_eq!(get(&bits, 36, 13), 0x0fff);
        assert_eq!(get(&bits, 115, 4), 1, "interval 3 uses constellation 1");
        // "bit 154 corresponds to Ucode 16", and 24 is the first code in.
        assert!(!bits[154 + 7], "Ucode 23 is not in it");
        assert!(bits[154 + 8], "Ucode 24 is");
        assert_eq!(cp.points(3).len(), 44);
        assert_eq!(cp.points(0).len(), 88);
        assert_eq!(cp.frame_bits(), 35);
        // Shaping coefficients keep their signs.
        assert_eq!(get(&bits, 69, 8) as u8 as i8, -64);
    }

    #[test]
    fn a_cp_with_codec_constellations_carries_twice_as_many() {
        let mut cp = a_cp();
        cp.codec = Some(cp.constellations.clone());
        let bits = cp.to_bits();
        assert_eq!(bits.len(), 292 + 2 * 136 + 136);
        assert!(bits[128]);
        assert_eq!(Cp::from_bits(&bits), Some(cp));
    }

    #[test]
    fn one_constellation_is_the_short_cp() {
        let cp = Cp { constellations: vec![1 << 100 | 1 << 80], ..Cp::default() };
        let bits = cp.to_bits();
        assert_eq!(bits.len(), 292);
        assert!(!bits[19], "a CPt");
        assert_eq!(cp.frame_bits(), 8, "drn 0 of a CPt");
        assert_eq!(Cp::from_bits(&bits), Some(cp));
    }

    /// Repeated, with rubbish before and between, as a receiver gets them.
    #[test]
    fn the_finders_pick_sequences_out_of_a_stream() {
        let mut stream: Vec<bool> = vec![true; 40];
        stream.extend(bits_of("0110100110"));
        let d = conexant();
        for _ in 0..2 {
            stream.extend(d.to_bits());
        }
        let mut finder = DescriptorFinder::default();
        let found: Vec<Descriptor> = stream.iter().filter_map(|&b| finder.feed(b)).collect();
        assert_eq!(found, vec![d.clone(), d]);

        let cp = a_cp();
        let mut stream: Vec<bool> = vec![true; 25];
        for acknowledge in [false, false, true] {
            stream.extend(Cp { acknowledge, ..cp.clone() }.to_bits());
        }
        let mut finder = CpFinder::default();
        let found: Vec<bool> = stream.iter().filter_map(|&b| finder.feed(b)).map(|c| c.acknowledge).collect();
        // The first is behind 25 ones, which make its sync something longer.
        assert_eq!(found, vec![false, true]);
    }

    #[test]
    fn a_v90_mp_reads_as_v34_s_and_fills_to_whole_frames() {
        let mp = Mp {
            call_to_answer: 9,
            answer_to_call: 12,
            auxiliary: true,
            trellis: crate::v34::mp::Trellis::States16,
            non_linear: true,
            expanded_shaping: false,
            acknowledge: true,
            rates: Mp::rates_up_to(12),
            asymmetric: true,
            precoding: None,
        };
        let bits = mp_bits(&mp, 29);
        assert_eq!(bits.len(), 87, "86 bits is three frames of 29");
        let back = Mp::from_bits(&bits).expect("V.34 cannot read it");
        // What V.90 reserves comes back as zeros.
        assert_eq!(back.call_to_answer, 0);
        assert!(!back.auxiliary && !back.asymmetric);
        assert_eq!(back.answer_to_call, 12);
        assert_eq!(back.rates, Mp::rates_up_to(12) & !1);
        assert!(back.acknowledge && back.non_linear);
        // Type 1 fills from bit 188.
        let typed = Mp { precoding: Some([(1, -1), (2, -2), (3, -3)]), ..mp };
        let bits = mp_bits(&typed, 40);
        assert_eq!(bits.len(), 200);
        assert_eq!(Mp::from_bits(&bits).unwrap().precoding, typed.precoding);
    }
}
