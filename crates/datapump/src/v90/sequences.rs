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
//!
//! V.92 keeps every bit of that machinery and moves the fields about inside
//! it. Its CPt and CPu share one table (Table 23/V.92) in which bit 18 tells
//! a CP from an SUV, a two-bit type follows at 19:20, and drn sits at 21:25
//! rather than V.90's 20:24; the silence request and the upstream rate mask
//! have gone, the first to SUVu and the second into the DIL descriptor, where
//! Table 20/V.92 adds two more words to carry it. Both sequences fill to a
//! multiple of twelve symbols instead of V.90's fixed tail, and both arrive
//! behind twenty-four-one preambles that V.90's finder walks straight past.
//! [`Layout`] picks which reading applies. Nothing here changes what V.90
//! sends, or how it reads what arrives.

use crate::v34::info::crc;
use crate::v34::mp::Mp;

use super::sign::Redundancy;
use super::ucode::UCODES;

/// "Frame Sync: 11111111111111111".
pub const SYNC_ONES: usize = 17;

/// Information bits between start bits.
pub const BLOCK: usize = 16;

/// Which Recommendation's reading of a sequence to take.
///
/// The frame, the CRC and everything from bit 49 onwards are shared, so the
/// two layouts differ only in bits 18:48 of a CP (Table 14/V.90 against Table
/// 23/V.92) and in the tail of a DIL descriptor (Table 12/V.90 against Table
/// 20/V.92), plus the fill each ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Table 12/V.90 and Table 14/V.90.
    #[default]
    V90,
    /// Table 20/V.92 and Table 23/V.92.
    V92,
}

/// The sync, each block behind its start bit, and the CRC behind one: every
/// sequence here up to its fill.
pub(crate) fn frame(information: &[bool]) -> Vec<bool> {
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
pub(crate) fn unframe(bits: &[bool], blocks: usize) -> Option<Vec<bool>> {
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

pub(crate) fn put(bits: &mut Vec<bool>, value: u32, width: usize) {
    for i in 0..width {
        bits.push(value >> i & 1 == 1);
    }
}

pub(crate) fn get(bits: &[bool], from: usize, width: usize) -> u32 {
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

/// The PCM upstream ladder: nineteen rates numbered from 24 000 up in steps
/// of 8000/6, which is "24 000 bit/s to 48 000 bit/s" (6.1/V.92) as the mask
/// Table 20/V.92 adds to the DIL descriptor spells it out (8.5.4).
///
/// "PCM" is in the name because this is the third thing in this crate called
/// an upstream rate, and the only one that means V.92's PCM upstream: the
/// others are [`Cp::upstream_rates`], Table 14/V.90's thirteen-bit mask of
/// the V.34 ladder from 4800 to 33 600, and `super::digital::upstream_rate`,
/// which picks a V.34 rate out of that mask and an MP.
pub const PCM_UPSTREAM_RATES: usize = 19;

/// Every PCM upstream rate enabled, for [`Descriptor::to_bits_in`].
pub const ALL_PCM_UPSTREAM_RATES: u32 = (1 << PCM_UPSTREAM_RATES) - 1;

/// The PCM upstream rate bit `urn` of Table 20/V.92 names: 24 000 +
/// urn*8000/6, so bit 0 is 24 000 and bit 18 is 48 000.
pub fn pcm_upstream_rate(urn: usize) -> Option<u32> {
    (urn < PCM_UPSTREAM_RATES).then(|| super::rate_for(urn as u32 + 18))
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

/// Bit 47, which V.92 took over as the "Jd/Jp identifier": 0 = Jd (Table
/// 21/V.92), 1 = Jp (Table 22/V.92), where V.90's Table 13 had the training
/// constellation flag [`Jd::sixteen_in_training`].
///
/// Jd and Jp are the same length, framed the same way, and both carry a good
/// CRC, so a reader that does not look here first reads a Jp as a Jd whose
/// rate mask is really epsilon (8.6.3) -- nonsense rates, with
/// `sixteen_in_training` set from Jp's own constellation bit.
pub const J_IDENTIFIER: usize = 47;

/// The acceptance predicate a V.92 reader hands [`Jd::from_bits_if`]: bit 47
/// clear, so only a Jd is read as one (Table 21/V.92).
pub fn is_jd(bits: &[bool]) -> bool {
    bits.len() > J_IDENTIFIER && !bits[J_IDENTIFIER]
}

/// The other half of [`is_jd`]: bit 47 set marks a Jp (Table 22/V.92).
pub fn is_jp(bits: &[bool]) -> bool {
    bits.len() > J_IDENTIFIER && bits[J_IDENTIFIER]
}

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
        Self::from_bits_if(bits, |_| true)
    }

    /// The same, but only where `accept` agrees. It sees the raw framed bits,
    /// starting at the frame sync, and is asked before a single field is
    /// read, because in V.92 bit 47 alone says whether these bits are a rate
    /// mask or an epsilon: pass [`is_jd`] and a Jp is refused rather than
    /// decoded (8.6.2, 8.6.3). V.90 asks nothing and [`Jd::from_bits`] keeps
    /// reading bit 47 as [`Jd::sixteen_in_training`].
    pub fn from_bits_if(bits: &[bool], accept: impl Fn(&[bool]) -> bool) -> Option<Self> {
        if !accept(bits) {
            return None;
        }
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
///
/// Table 20/V.92 adds an upstream rate mask to the same descriptor, and that
/// mask is deliberately **not** a field here. It travels as an argument to
/// [`Descriptor::to_bits_in`], as the second half of what
/// [`Descriptor::from_bits_in`] returns, and out of a finder through
/// [`DescriptorFinder::upstream_rates`]. A field would have been the tidier
/// reading, but `Descriptor` derives no `Default` and is built by struct
/// literal in `v90::dil::design` and in `tests/dil_sounds.rs`, neither of
/// which V92-03 may edit; the mask is a property of the descriptor's
/// *carriage* rather than of the DIL it describes, so a parameter says the
/// same thing. Anything that reads a V.92 descriptor must therefore take the
/// mask from beside it, not from it.
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

/// The two words Table 20/V.92 adds after V.90's Table 12, holding the
/// nineteen-bit upstream rate mask and thirteen reserved bits (8.5.4).
const MASK_WORDS: usize = 2;

/// "Ja shall be a whole number of 12 bit units in length" (8.5.4/V.92), and
/// each descriptor is padded to that unit, so the repetitions stay on the
/// upstream data frame grid. Ja carries one bit per symbol, so twelve
/// symbols are twelve bits.
pub const JA_UNIT_BITS: usize = 12;

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
        self.to_bits_in(None)
    }

    /// The descriptor as Ja repeats it, in whichever layout `upstream_rates`
    /// chooses.
    ///
    /// `None` is Table 12/V.90: the CRC, "Fill bit: 0", and one more fill bit
    /// if it takes one to make the length even. `Some(mask)` is Table
    /// 20/V.92, which keeps every V.90 field and then adds two more words --
    /// the nineteen-bit rate mask of [`pcm_upstream_rate`] at 188+P, and
    /// thirteen bits reserved for the ITU -- before the CRC at 222+P, a
    /// single fill bit, and zeros to a multiple of [`JA_UNIT_BITS`] (8.5.4).
    /// The CRC covers the new words like any other information bits.
    pub fn to_bits_in(&self, upstream_rates: Option<u32>) -> Vec<bool> {
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
        if let Some(rates) = upstream_rates {
            // "188+P : 203+P", then 45 333, 46 667 and 48 000, then thirteen
            // bits "Reserved for ITU: sent 0".
            put(&mut information, rates & 0xffff, BLOCK);
            put(&mut information, rates >> 16 & 0x7, 3);
            put(&mut information, 0, BLOCK - 3);
        }
        let mut bits = frame(&information);
        // "Fill bit: 0", and then whichever tail the layout asks for.
        bits.push(false);
        match upstream_rates {
            None => {
                if bits.len() % 2 == 1 {
                    bits.push(false);
                }
            }
            Some(_) => bits.resize(bits.len().div_ceil(JA_UNIT_BITS) * JA_UNIT_BITS, false),
        }
        bits
    }

    /// A descriptor out of `bits`, which start at its frame sync.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        Self::from_bits_in(Layout::V90, bits).map(|(descriptor, _)| descriptor)
    }

    /// The same, in whichever layout is being read, and with the upstream
    /// rate mask [`Layout::V92`] carries -- `None` under [`Layout::V90`],
    /// which has no such field.
    pub fn from_bits_in(layout: Layout, bits: &[bool]) -> Option<(Self, Option<u32>)> {
        if bits.len() < SYNC_ONES + 2 * (BLOCK + 1) {
            return None;
        }
        // The first two blocks say how long the rest is.
        let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
        let n = get(bits, at(0, 0), 8) as usize;
        let lsp = get(bits, at(1, 0), 7) as usize + 1;
        let ltp = get(bits, at(1, 8), 7) as usize + 1;
        let v90_blocks = 2 + lsp.div_ceil(BLOCK) + ltp.div_ceil(BLOCK) + 8 + n.div_ceil(2);
        let blocks = v90_blocks + if layout == Layout::V92 { MASK_WORDS } else { 0 };
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
        let rates = (layout == Layout::V92).then(|| {
            let mask = v90_blocks * BLOCK;
            get(&information, mask, BLOCK) | get(&information, mask + BLOCK, 3) << BLOCK
        });
        Some((Self { signs, training, h, refs, ucodes }, rates))
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
    ///
    /// Under [`Layout::V92`] this is the whole two-bit "Type" of bits 19:20
    /// (Table 23/V.92): 0 = CPt, 1 = CPu. Type 2 is CPus, a different and
    /// much shorter table, and 3 is unassigned; neither is read as a [`Cp`].
    pub data_mode: bool,
    /// Bits 20:24, and **21:25** under [`Layout::V92`], which shifted them
    /// along to make room for the two-bit type: the downstream rate. Zero is
    /// cleardown; otherwise see [`data_rate`] and [`training_bits`].
    pub drn: u8,
    /// Bit 30: a silent period is asked for -- CPs (9.6).
    ///
    /// V.92 has no such bit. The request moved to SUVu bit 32 (Table
    /// 27/V.92), so under [`Layout::V92`] this is neither sent nor read, and
    /// comes back false.
    pub silence: bool,
    /// Bits 31:32: Sr.
    pub redundancy: Redundancy,
    /// Bit 33: the far end's MP has arrived -- which makes this a CP'.
    pub acknowledge: bool,
    /// Bit 35: "Codec type: 0 = mu-law; 1 = A-law".
    pub a_law: bool,
    /// Bits 36:48: upstream rates the analogue modem's transmitter has
    /// enabled, 4800 in bit 0 up to 33 600 in bit 12.
    ///
    /// This is the V.34 ladder, not V.92's PCM one. Reserved in V.92, whose
    /// PCM upstream mask ([`PCM_UPSTREAM_RATES`]) went into the DIL
    /// descriptor instead ([`Descriptor::to_bits_in`]), so under
    /// [`Layout::V92`] this is neither sent nor read, and comes back zero.
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

/// Table 23/V.92 bit 18, "CP: 0". SUVu and SUVd carry 1 in the same place
/// (Tables 27 and 31/V.92), so this one bit tells the CP family from the SUV
/// family before anything else is read (8.7.3).
pub const CP_MARK: usize = 18;

/// Table 23/V.92 bits 19:20, "Type", read as an integer: 0 = CPt.
pub const CP_TYPE_CPT: u32 = 0;

/// Type 1 = CPu, the same table read for data mode rather than training.
pub const CP_TYPE_CPU: u32 = 1;

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
        self.to_bits_in(Layout::V90, 1)
    }

    /// The CP in whichever layout is being spoken.
    ///
    /// [`Layout::V90`] is Table 14/V.90, exactly as [`Cp::to_bits`] writes
    /// it, and ignores `pad_unit_bits`: its tail is "Fill bits: 000".
    ///
    /// [`Layout::V92`] is Table 23/V.92. Bit 18 goes out as 0 to mark the CP
    /// family, [`Cp::data_mode`] becomes the two-bit type at 19:20, drn moves
    /// to 21:25, and bits 26:30 and 36:48 go out as the zeros Table 23 asks
    /// for -- so neither [`Cp::silence`] nor [`Cp::upstream_rates`] is sent.
    /// Everything from bit 49 on is V.90's. The tail is one fill bit and then
    /// zeros "to extend the length to the next multiple of 12 symbols", which
    /// is `pad_unit_bits`: twelve for CPt, which carries one bit per symbol
    /// (8.5.1), and twenty-four or thirty-six for a CPu on four- or
    /// eight-point TRN2u (8.7.3 with Jp bits 48:49).
    pub fn to_bits_in(&self, layout: Layout, pad_unit_bits: usize) -> Vec<bool> {
        let mut information = Vec::new();
        match layout {
            Layout::V90 => {
                information.push(false); // 18: reserved
                information.push(self.data_mode); // 19
                put(&mut information, u32::from(self.drn), 5); // 20:24
                put(&mut information, 0, 5); // 25:29 reserved
                information.push(self.silence); // 30
            }
            Layout::V92 => {
                information.push(false); // 18: "CP: 0"
                put(&mut information, u32::from(self.data_mode), 2); // 19:20 type
                put(&mut information, u32::from(self.drn), 5); // 21:25
                put(&mut information, 0, 5); // 26:30 reserved
            }
        }
        put(&mut information, self.redundancy.spent() as u32, 2);
        information.push(self.acknowledge);

        information.push(self.a_law);
        // 36:48: the upstream rate mask in V.90, reserved in V.92.
        let upstream = if layout == Layout::V92 { 0 } else { u32::from(self.upstream_rates) };
        put(&mut information, upstream, 13);
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
        match layout {
            Layout::V90 => put(&mut bits, 0, 3), // "Fill bits: 000"
            Layout::V92 => {
                bits.push(false); // "Fill bit: 0"
                let unit = pad_unit_bits.max(1);
                bits.resize(bits.len().div_ceil(unit) * unit, false);
            }
        }
        bits
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        Self::from_bits_in(Layout::V90, bits)
    }

    /// The CP `bits` hold, read in `layout`.
    ///
    /// Under [`Layout::V92`] the dispatch fields are read first and are the
    /// only ones that can refuse a sequence: bit 18 set means an SUV rather
    /// than a CP, and a type of 2 (CPus, Table 24/V.92) or 3 (unassigned) is
    /// not this table. Every other reserved run -- 26:30, 36:48 and 129:135
    /// -- is ignored rather than refused, which is what "set to 0" and "not
    /// interpreted" ask for, and keeps a far end that uses a later ITU
    /// extension, or leaves stale bits set, connecting.
    pub fn from_bits_in(layout: Layout, bits: &[bool]) -> Option<Self> {
        if bits.len() < SYNC_ONES + CP_HEADER_BLOCKS * (BLOCK + 1) {
            return None;
        }
        let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
        if layout == Layout::V92 && (bits[CP_MARK] || get(bits, CP_MARK + 1, 2) > CP_TYPE_CPU) {
            return None;
        }
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
        // Bits 18:30 are the only ones that moved. Information bit i is
        // absolute bit 18 + i here, so V.90 reads 19, 20:24 and 30 where V.92
        // reads 19:20 and 21:25 and has nothing at all.
        let (data_mode, drn, silence, upstream_rates) = match layout {
            Layout::V90 => {
                (information[1], get(&information, 2, 5) as u8, information[12], get(&information, 17, 13) as u16)
            }
            Layout::V92 => (get(&information, 1, 2) == CP_TYPE_CPU, get(&information, 3, 5) as u8, false, 0),
        };
        Some(Self {
            data_mode,
            drn,
            silence,
            redundancy: match get(&information, 13, 2) {
                0 => Redundancy::None,
                1 => Redundancy::One,
                2 => Redundancy::Two,
                _ => Redundancy::Three,
            },
            acknowledge: information[15],
            a_law: information[16],
            upstream_rates,
            lookahead: get(&information, 30, 2) as u8,
            trn1d_ratio: get(&information, 32, 16) as u16,
            shaping: std::array::from_fn(|k| get(&information, 48 + 8 * k, 8) as u8 as i8),
            intervals,
            constellations,
            codec,
        })
    }
}

/// What a finder is looking for: whose layout, and, for a V.92 CP, how many
/// bits twelve symbols carry, which is what its fill runs to.
#[derive(Debug, Clone, Copy)]
struct Shape {
    layout: Layout,
    pad_unit: usize,
}

/// Finds one kind of framed sequence in a stream of descrambled bits.
///
/// A sequence starts where a zero follows seventeen ones and not eighteen --
/// an eighteenth would make the run fill or the last of something else --
/// and how long it is follows from its first few blocks. Every place that
/// could be a start is kept until there are enough bits to read it there.
///
/// V.92 relaxes that rule to **at least** seventeen, taking the last
/// seventeen as the sync, because Ja's first descriptor and the first CPt of
/// a group sit behind twenty-four-one preambles (8.5.4, 8.5.1) and so arrive
/// behind forty-one ones. The exact rule drops them, which costs a repetition
/// on every group; the CRC throws out the false starts the loose rule lets
/// in.
#[derive(Debug, Clone)]
struct Finder<T> {
    bits: Vec<bool>,
    ones: usize,
    /// Where candidates start, in `bits`.
    starts: Vec<usize>,
    /// Bits needed to know the length, and the length from them.
    header: usize,
    shape: Shape,
    length: fn(Shape, &[bool]) -> usize,
    parse: fn(Shape, &[bool]) -> Option<T>,
}

impl<T> Finder<T> {
    fn new(header_blocks: usize, shape: Shape, length: fn(Shape, &[bool]) -> usize, parse: fn(Shape, &[bool]) -> Option<T>) -> Self {
        Self {
            bits: Vec::new(),
            ones: 0,
            starts: Vec::new(),
            header: SYNC_ONES + header_blocks * (BLOCK + 1),
            shape,
            length,
            parse,
        }
    }

    fn feed(&mut self, bit: bool) -> Option<T> {
        let sync = match self.shape.layout {
            Layout::V90 => self.ones == SYNC_ONES,
            Layout::V92 => self.ones >= SYNC_ONES,
        };
        if !bit && sync {
            self.starts.push(self.bits.len() - SYNC_ONES);
        }
        self.ones = if bit { self.ones + 1 } else { 0 };
        self.bits.push(bit);
        let mut found = None;
        let bits = &self.bits;
        let (header, shape, length, parse) = (self.header, self.shape, self.length, self.parse);
        self.starts.retain(|&start| {
            if found.is_some() {
                return false;
            }
            let have = bits.len() - start;
            if have < header {
                return true;
            }
            let needed = length(shape, &bits[start..]);
            if have < needed {
                return true;
            }
            found = parse(shape, &bits[start..start + needed]);
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

/// A DIL descriptor's length from its first two blocks: up to the end of the
/// CRC under V.90, and up to the end of the fill under V.92, whose
/// descriptors run back to back on the [`JA_UNIT_BITS`] grid.
fn descriptor_length(shape: Shape, bits: &[bool]) -> usize {
    let at = |block: usize, bit: usize| SYNC_ONES + block * (BLOCK + 1) + 1 + bit;
    let n = get(bits, at(0, 0), 8) as usize;
    let lsp = get(bits, at(1, 0), 7) as usize + 1;
    let ltp = get(bits, at(1, 8), 7) as usize + 1;
    let mut blocks = 2 + lsp.div_ceil(BLOCK) + ltp.div_ceil(BLOCK) + 8 + n.div_ceil(2);
    if shape.layout == Layout::V92 {
        blocks += MASK_WORDS;
    }
    let through_crc = SYNC_ONES + (blocks + 1) * (BLOCK + 1);
    match shape.layout {
        Layout::V90 => through_crc,
        Layout::V92 => (through_crc + 1).div_ceil(JA_UNIT_BITS) * JA_UNIT_BITS,
    }
}

/// A CP's length from its first seven blocks, the same way.
fn cp_length(shape: Shape, bits: &[bool]) -> usize {
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
    let through_crc = SYNC_ONES + (CP_HEADER_BLOCKS + MASK_BLOCKS * masks + 1) * (BLOCK + 1);
    match shape.layout {
        Layout::V90 => through_crc,
        Layout::V92 => {
            let unit = shape.pad_unit.max(1);
            (through_crc + 1).div_ceil(unit) * unit
        }
    }
}

/// Finds Ja's DIL descriptors.
///
/// Under [`Layout::V92`] each descriptor brings an upstream rate mask, which
/// is no part of [`Descriptor`] (see its doc): [`Self::feed`] reports the
/// descriptor and puts the mask where [`Self::upstream_rates`] can be asked
/// for it. Read it after the feed that reported the descriptor it belongs
/// to; the next descriptor found replaces it.
#[derive(Debug, Clone)]
pub struct DescriptorFinder {
    finder: Finder<(Descriptor, Option<u32>)>,
    upstream_rates: Option<u32>,
}

impl Default for DescriptorFinder {
    fn default() -> Self {
        Self::reading(Layout::V90)
    }
}

impl DescriptorFinder {
    /// Reading Table 20/V.92 instead: two more words of information, a fill
    /// to a multiple of [`JA_UNIT_BITS`], and the loose sync rule that gets
    /// the first descriptor out from behind Ja's twenty-four ones (8.5.4).
    pub fn v92() -> Self {
        Self::reading(Layout::V92)
    }

    fn reading(layout: Layout) -> Self {
        let shape = Shape { layout, pad_unit: JA_UNIT_BITS };
        let parse = |shape: Shape, bits: &[bool]| Descriptor::from_bits_in(shape.layout, bits);
        Self { finder: Finder::new(2, shape, descriptor_length, parse), upstream_rates: None }
    }

    pub fn feed(&mut self, bit: bool) -> Option<Descriptor> {
        let found = self.finder.feed(bit);
        if let Some((_, rates)) = &found {
            self.upstream_rates = *rates;
        }
        found.map(|(descriptor, _)| descriptor)
    }

    /// The upstream rate mask that came with the descriptor [`Self::feed`]
    /// last reported: `None` under V.90's layout, which has no such field,
    /// and `Some` of the nineteen bits of [`pcm_upstream_rate`] under V.92's.
    pub fn upstream_rates(&self) -> Option<u32> {
        self.upstream_rates
    }
}

/// Finds CP sequences.
#[derive(Debug, Clone)]
pub struct CpFinder(Finder<Cp>);

impl Default for CpFinder {
    fn default() -> Self {
        Self(Self::finder(Shape { layout: Layout::V90, pad_unit: 1 }))
    }
}

impl CpFinder {
    /// Reading Table 23/V.92 instead, with `pad_unit_bits` as
    /// [`Cp::to_bits_in`] takes it. CPus (type 2, Table 24/V.92) is a
    /// different table and is not found here.
    pub fn v92(pad_unit_bits: usize) -> Self {
        Self(Self::finder(Shape { layout: Layout::V92, pad_unit: pad_unit_bits }))
    }

    fn finder(shape: Shape) -> Finder<Cp> {
        let parse = |shape: Shape, bits: &[bool]| Cp::from_bits_in(shape.layout, bits);
        Finder::new(CP_HEADER_BLOCKS, shape, cp_length, parse)
    }

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

    fn text(bits: &[bool]) -> String {
        bits.iter().map(|&b| if b { '1' } else { '0' }).collect()
    }

    /// `bits` with every position in `set` made a one and the CRC put right
    /// again, as a far end using a later ITU extension would send it.
    fn with_bits_set(bits: &[bool], blocks: usize, set: &[usize]) -> Vec<bool> {
        let mut out = bits.to_vec();
        for &p in set {
            out[p] = true;
        }
        let mut information = Vec::new();
        for b in 0..blocks {
            let start = SYNC_ONES + b * (BLOCK + 1) + 1;
            information.extend_from_slice(&out[start..start + BLOCK]);
        }
        let crc_at = SYNC_ONES + blocks * (BLOCK + 1) + 1;
        let value = u32::from(crc(&information));
        for i in 0..16 {
            out[crc_at + i] = value >> i & 1 == 1;
        }
        out
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

    /// The worked example of `spec-phase3-signals.md` 4.1, from Table
    /// 23/V.92: drn 16, one constellation holding Ucodes 0 to 79, everything
    /// else at its default.
    fn a_cpt() -> Cp {
        Cp { drn: 16, constellations: vec![(1 << 80) - 1], ..Cp::default() }
    }

    /// A V.92 CP whose largest constellation index is `max`, and which
    /// carries the codec's own constellations too when `codec`.
    fn sized(max: u8, codec: bool) -> Cp {
        let masks: Vec<Mask> = (0..=max).map(|k| 1 << (k + 1)).collect();
        Cp {
            intervals: [0, 0, 0, 0, 0, max],
            constellations: masks.clone(),
            codec: codec.then_some(masks),
            ..Cp::default()
        }
    }

    /// Table 23/V.92 with the worked example of P3S 4.1 in it, which is the
    /// only CPt anyone has written down.
    #[test]
    fn a_v92_cpt_has_its_type_at_19_and_drn_at_21() {
        let cpt = a_cpt();
        let bits = cpt.to_bits_in(Layout::V92, JA_UNIT_BITS);
        assert_eq!(bits.len(), 300, "290 bits filled to a multiple of twelve symbols");
        assert!(!bits[CP_MARK], "bit 18 marks the CP family, not the SUV family");
        assert_eq!(get(&bits, 19, 2), CP_TYPE_CPT);
        assert_eq!(get(&bits, 21, 5), 16, "drn at 21:25, where V.90 had 20:24");
        assert_eq!(text(&bits[18..34]), "0000000100000000");
        assert_eq!(text(&bits[35..51]), "0000000000000000");
        assert_eq!(text(&bits[52..68]), "0000000000000100", "the Q3.13 ratio of one");
        assert_eq!(get(&bits, 273, 16), 0xAC4D, "the CRC the digest derived");
        assert_eq!(text(&bits[272..]), "0101100100011010100000000000", "start bit, CRC, fill bit, padding");
        assert_eq!(cpt.frame_bits(), 24, "(drn + 8) * 8000/6 is 32 000 in a CPt");
        assert_eq!(Cp::from_bits_in(Layout::V92, &bits), Some(cpt));
    }

    /// Table 23/V.92, the other type that shares it: CPu, whose drn names a
    /// data mode rate rather than a training one.
    #[test]
    fn a_v92_cpu_with_drn_22_puts_0_1_1_0_1_in_bits_21_to_25() {
        let cpu = Cp { data_mode: true, drn: 22, ..a_cpt() };
        let bits = cpu.to_bits_in(Layout::V92, 24);
        assert_eq!(text(&bits[21..26]), "01101", "twenty-two, least significant bit first");
        assert_eq!(get(&bits, 19, 2), CP_TYPE_CPU);
        assert_eq!(data_rate(22), Some(56_000), "(drn + 20) * 8000/6 in a CPu");
        assert_eq!(bits.len(), 312, "290 bits filled to a multiple of twenty-four");
        assert_eq!(Cp::from_bits_in(Layout::V92, &bits), Some(cpu));
    }

    /// "bit 137 = Ucode 0 ... bit 152 = Ucode 15", "bit 154 = Ucode 16" and
    /// "bit 271 = Ucode 127" (Table 23/V.92), which is V.90's placing kept.
    #[test]
    fn mask_bits_sit_where_table_23_puts_them() {
        for (u, at) in [(0u8, 137usize), (15, 152), (16, 154), (127, 271)] {
            let cp = Cp { constellations: vec![1 << u], ..Cp::default() };
            let bits = cp.to_bits_in(Layout::V92, JA_UNIT_BITS);
            assert!(bits[at], "Ucode {u} belongs at bit {at}");
            assert_eq!(bits[136..272].iter().filter(|b| **b).count(), 1, "and nowhere else");
            assert_eq!(Cp::from_bits_in(Layout::V92, &bits).unwrap().points(0), vec![u]);
        }
    }

    /// "gamma = 136 x (the largest constellation index)", "delta = 2 gamma +
    /// 136" when bit 128 is set and gamma when it is not, and a length of
    /// 290 + delta before the fill (8.5.1, 8.7.3). The padded lengths are the
    /// tables of P3S 4.1 and P4A 2.4: twelve bits to the frame for CPt, and
    /// twenty-four or thirty-six for a CPu on four- or eight-point TRN2u.
    #[test]
    fn cp_lengths_follow_gamma_and_delta_for_each_pad_unit() {
        for (max, codec, unpadded, padded) in [
            (0u8, false, 290usize, [300usize, 312, 324]),
            (0, true, 426, [432, 432, 432]),
            (1, false, 426, [432, 432, 432]),
            (5, false, 970, [972, 984, 972]),
            (5, true, 1786, [1788, 1800, 1800]),
        ] {
            let cp = sized(max, codec);
            for (unit, want) in [12usize, 24, 36].into_iter().zip(padded) {
                let bits = cp.to_bits_in(Layout::V92, unit);
                assert_eq!(bits.len(), unpadded.div_ceil(unit) * unit);
                assert_eq!(bits.len(), want, "index {max}, codec {codec}, unit {unit}");
                assert_eq!(bits[128], codec, "bit 128 says whether the codec's own follow");
                assert_eq!(Cp::from_bits_in(Layout::V92, &bits), Some(cp.clone()));
            }
            // V.90's tail is "Fill bits: 000" whatever the frame is.
            assert_eq!(cp.to_bits().len(), unpadded + 2);
        }
    }

    /// The hazard of CD 4.4: a V.92 CPt read through Table 14/V.90 does not
    /// fail, because the CRC covers the same information bits either way. It
    /// comes back with bits 20:24 as drn, which is 2 x drn modulo 32.
    #[test]
    fn a_v92_cpt_is_not_misread_through_the_v90_layout_or_back() {
        for drn in 0..=22u8 {
            let bits = Cp { drn, ..a_cpt() }.to_bits_in(Layout::V92, JA_UNIT_BITS);
            let misread = Cp::from_bits(&bits).expect("the CRC checks either way, which is the hazard");
            assert_eq!(misread.drn, drn.wrapping_mul(2) % 32, "drn {drn} doubled");
            assert_eq!(Cp::from_bits_in(Layout::V92, &bits).unwrap().drn, drn);
        }
        // The other way about, a V.90 CP's bit 19 and the bottom of its drn
        // land in V.92's two-bit type, where only 0 and 1 are this table.
        let bits = a_cp().to_bits();
        assert_eq!(get(&bits, 19, 2), 3, "data mode, and an odd drn");
        assert_eq!(Cp::from_bits_in(Layout::V92, &bits), None);
    }

    /// Table 23/V.92 and Table 20/V.92 both say of their reserved runs only
    /// that the sender sets them to 0 and the receiver does not interpret
    /// them. Nothing is refused for one, so a far end using a later ITU
    /// extension, or leaving stale bits set, still connects.
    #[test]
    fn reserved_bits_set_by_a_far_end_are_ignored_not_rejected() {
        let cpt = a_cpt();
        let bits = cpt.to_bits_in(Layout::V92, JA_UNIT_BITS);
        let reserved: Vec<usize> = (26..=30).chain(36..=48).chain(129..=135).collect();
        let spoiled = with_bits_set(&bits, CP_HEADER_BLOCKS + MASK_BLOCKS, &reserved);
        assert_ne!(spoiled, bits);
        assert_eq!(Cp::from_bits_in(Layout::V92, &spoiled), Some(cpt), "not one of them is interpreted");

        let d = Descriptor::none();
        let bits = d.to_bits_in(Some(ALL_PCM_UPSTREAM_RATES));
        // Table 20's own reserved runs with N = 0, where beta and P are 34:
        // after N, either side of L_TP - 1, after H1, and the thirteen bits
        // above the nineteenth upstream rate.
        let reserved: Vec<usize> = (26..=33).chain([42, 50, 93]).chain(242..=254).collect();
        let spoiled = with_bits_set(&bits, 14, &reserved);
        assert_ne!(spoiled, bits);
        let read = Descriptor::from_bits_in(Layout::V92, &spoiled);
        assert_eq!(read, Some((d, Some(ALL_PCM_UPSTREAM_RATES))));
    }

    /// Bit 18 and the type at 19:20 are the only dispatch fields, and the
    /// only ones a V.92 reader may refuse a sequence over (8.7.3).
    #[test]
    fn a_cp_marked_as_an_suv_or_a_cpus_is_refused() {
        let bits = a_cpt().to_bits_in(Layout::V92, JA_UNIT_BITS);
        let blocks = CP_HEADER_BLOCKS + MASK_BLOCKS;
        assert!(Cp::from_bits(&bits).is_some(), "V.90's reading refuses none of this");
        // Bit 18 set is an SUV (Tables 27 and 31/V.92), not a CP at all.
        assert_eq!(Cp::from_bits_in(Layout::V92, &with_bits_set(&bits, blocks, &[CP_MARK])), None);
        // Type 2 is CPus, a shorter table of its own; type 3 is unassigned.
        for (set, want) in [(&[20usize][..], 2), (&[19, 20][..], 3)] {
            let spoiled = with_bits_set(&bits, blocks, set);
            assert_eq!(get(&spoiled, 19, 2), want);
            assert_eq!(Cp::from_bits_in(Layout::V92, &spoiled), None);
        }
    }

    /// "When N = 0 the descriptor is 276 bits long" (8.5.4), which is the one
    /// length the Recommendation prints for Table 20.
    #[test]
    fn a_v92_descriptor_with_no_dil_is_276_bits() {
        let d = Descriptor::none();
        let bits = d.to_bits_in(Some(ALL_PCM_UPSTREAM_RATES));
        assert_eq!(bits.len(), 276, "273 bits filled to a multiple of twelve");
        // With N = 0, beta and P are both 34: the mask words sit at 188 + P
        // and 205 + P, and the CRC at 222 + P.
        assert_eq!(get(&bits, 222, 16), 0xffff, "24 000 up to 44 000");
        assert_eq!(get(&bits, 239, 3), 0x7, "45 333, 46 667 and 48 000");
        assert_eq!(get(&bits, 242, 13), 0, "and thirteen bits reserved above them");
        assert_eq!(get(&bits, 256, 16), 0xB71C, "the CRC the digest derived");
        assert_eq!(Descriptor::from_bits_in(Layout::V92, &bits), Some((d, Some(ALL_PCM_UPSTREAM_RATES))));
        assert_eq!(pcm_upstream_rate(0), Some(24_000));
        assert_eq!(pcm_upstream_rate(18), Some(48_000));
        assert_eq!(pcm_upstream_rate(PCM_UPSTREAM_RATES), None);
        // V.90's own descriptor ends after the CRC with a fill bit and, if it
        // takes one, a second to make the length even.
        assert_eq!(Descriptor::none().to_bits().len(), 240);
    }

    /// "Ja shall be a whole number of 12 bit units in length" (8.5.4), and
    /// each descriptor is padded to that unit. The lengths are P3S 4.4's
    /// table of 290 -> 300, 817 -> 828 and 2687 -> 2688.
    #[test]
    fn v92_descriptor_lengths_pad_to_12_bits() {
        for (n, lsp, ltp, want) in [(1usize, 16usize, 16usize, 300usize), (64, 16, 16, 828), (255, 128, 128, 2688)] {
            let d = Descriptor {
                signs: (0..lsp).map(|k| k % 3 == 0).collect(),
                training: (0..ltp).map(|k| k % 5 == 0).collect(),
                h: [3; 8],
                refs: [70; 8],
                ucodes: (0..n).map(|k| (k % 118) as u8).collect(),
            };
            let rates = 0x2_aaaa;
            let bits = d.to_bits_in(Some(rates));
            assert_eq!(bits.len(), want, "N = {n}");
            assert_eq!(bits.len() % JA_UNIT_BITS, 0);
            assert_eq!(Descriptor::from_bits_in(Layout::V92, &bits), Some((d, Some(rates))));
        }
    }

    /// "24 differentially encoded binary ones" go before the first CPt of a
    /// series (8.5.1) and before Ja's first descriptor (8.5.4), so the first
    /// sequence of each group arrives behind forty-one ones. V.90's finder
    /// opens a candidate only after exactly seventeen, and drops it.
    #[test]
    fn a_frame_behind_forty_one_ones_is_found_by_the_v92_finder() {
        let cpt = a_cpt();
        let mut stream = vec![true; 24];
        for acknowledge in [false, true] {
            stream.extend(Cp { acknowledge, ..cpt.clone() }.to_bits_in(Layout::V92, JA_UNIT_BITS));
        }
        let mut finder = CpFinder::v92(JA_UNIT_BITS);
        let found: Vec<bool> = stream.iter().filter_map(|&b| finder.feed(b)).map(|c| c.acknowledge).collect();
        assert_eq!(found, vec![false, true], "both of them, preamble and all");

        let mut v90 = CpFinder::default();
        assert_eq!(stream.iter().filter_map(|&b| v90.feed(b)).count(), 1, "V.90 still skips the first");
    }

    /// Ja is "24 binary ones followed by repetitions of the DIL descriptor"
    /// (8.5.4), and the rate mask rides along with each one. The mask is no
    /// field of [`Descriptor`], so the accessor beside `feed` is the only way
    /// to it, and it must name the descriptor just reported rather than the
    /// first one seen: the Ja of a later training carries a mask of its own.
    #[test]
    fn a_ja_descriptor_behind_its_preamble_brings_its_upstream_rate_mask() {
        let d = conexant();
        let masks = [0x7_ffff & !(1 << 7), 0x5_0003];
        let mut stream = vec![true; 24];
        for rates in masks {
            stream.extend(d.to_bits_in(Some(rates)));
        }
        let mut finder = DescriptorFinder::v92();
        let mut found = Vec::new();
        for &bit in &stream {
            if let Some(got) = finder.feed(bit) {
                found.push((got, finder.upstream_rates()));
            }
        }
        assert_eq!(found, vec![(d.clone(), Some(masks[0])), (d.clone(), Some(masks[1]))]);
        assert_eq!(stream.len() % JA_UNIT_BITS, 0, "and Ja is a whole number of twelve-bit units");
        // The same descriptor in V.90's layout ends differently and carries
        // no mask at all.
        let mut v90 = DescriptorFinder::default();
        let plain: Vec<bool> = std::iter::repeat_n(true, 40).chain([false]).chain(d.to_bits()).collect();
        assert_eq!(plain.iter().filter_map(|&b| v90.feed(b)).next(), Some(d));
        assert_eq!(v90.upstream_rates(), None);
    }

    /// The Jp vector of P3S 5.3: epsilon 0x8000, eight-point TRN2u asked for
    /// in training, CRC 0x3E4E. Jd and Jp are the same length, framed the
    /// same way and both carry a good CRC, so only bit 47 tells them apart
    /// (8.6.2, 8.6.3).
    #[test]
    fn a_jd_with_bit_47_set_is_refused_by_the_v92_predicate() {
        const SERVER_JP: &str =
            "11111111111111111 0 0000000000000001 0 0000000000001100 0 0111001001111100 0000";
        let jp = bits_of(SERVER_JP);
        assert_eq!(jp.len(), JD_BITS);
        let misread = Jd::from_bits(&jp).expect("the CRC checks, which is the hazard");
        assert_eq!(misread.rates, 0x8000, "epsilon read as a rate mask");
        assert!(misread.sixteen_in_training, "and Jp's constellation bit read as V.90's");
        assert!(is_jp(&jp) && !is_jd(&jp) && jp[J_IDENTIFIER]);
        assert_eq!(Jd::from_bits_if(&jp, is_jd), None, "a V.92 reader refuses it");

        let jd = bits_of(SERVER_JD);
        assert!(is_jd(&jd) && !is_jp(&jd));
        assert_eq!(Jd::from_bits_if(&jd, is_jd), Jd::from_bits(&jd));
        assert_eq!(Jd::from_bits_if(&jd, is_jd).unwrap().rates, Jd::ALL_RATES);
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
