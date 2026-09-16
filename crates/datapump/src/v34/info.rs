//! The INFO sequences of phase 2 (10.1.2.3): what each modem can do, what the
//! line probing found, and what the two of them will use.
//!
//! Four of them, and a sequence is only ever one of two things: a capabilities
//! frame, INFO0, which both ends send with the same layout, or a results
//! frame, INFO1, which the call modem sends as INFO1c after probing the line
//! and the answer modem answers with INFO1a. All four share a shape -- four
//! fill ones, eight bits of frame sync, the information, sixteen bits of CRC
//! and four more fill ones -- and differ only in what the information is.
//!
//! Every multi-bit field is written "LSB:MSB": the lower bit number is the
//! least significant, and bit 0 goes first in time.

/// Bits 0:3 and the last four of every INFO sequence: "Fill bits: 1111".
pub const FILL: [bool; 4] = [true; 4];

/// Bits 4:11: "Frame sync: 01110010, where the left-most bit is first in
/// time."
pub const SYNC: [bool; 8] = [false, true, true, true, false, false, true, false];

/// Bits in each sequence, fill to fill (Tables 14, 15 and 16).
pub const INFO0_BITS: usize = 49;
pub const INFO1C_BITS: usize = 109;
pub const INFO1A_BITS: usize = 70;

/// V.90's INFO0d (Table 7/V.90), the one sequence of V.90's phase 2 whose
/// length is not one of V.34's. The other three are V.34's lengths: INFO0a
/// and both INFO1a are laid out as V.34 lays them, and "the bit definitions
/// [of INFO1d] are identical to those of INFO1c in Recommendation V.34".
pub const INFO0D_BITS: usize = 62;

/// Bits 37:39 of an INFO1a that asks for V.90: "Symbol rate of 8000 to be used
/// by the digital modem: The integer 6" (Table 10/V.90). Six is not one of
/// V.34's symbol rates, which is what tells the two INFO1a apart.
pub const PCM_SYMBOL_RATE: u32 = 6;

/// Where the information starts: after the fill and the frame sync.
const INFORMATION: usize = FILL.len() + SYNC.len();

/// The CRC of 10.1.2.3.2, as Figure 14 draws it.
///
/// Sixteen cells, numbered 15 on the left to 0 on the right, shifting right.
/// The bit coming in is added to cell 0 on its way out, and that sum is fed
/// back into cell 15 and into the two adders in front of cells 10 and 3 --
/// which is x^16 + x^12 + x^5 + 1 read from the low end. "Load the shift
/// register in the CRC generator with all ones", shift the information in, and
/// the CRC is what the register holds, "starting with bit 0". Nothing is
/// inverted on the way out.
pub fn crc(bits: &[bool]) -> u16 {
    bits.iter().fold(0xffff, |register, &bit| shift(register, bit))
}

/// One bit into Figure 14's register.
fn shift(register: u16, bit: bool) -> u16 {
    let feedback = (register & 1 == 1) != bit;
    let register = register >> 1;
    if feedback {
        register ^ (1 << 15 | 1 << 10 | 1 << 3)
    } else {
        register
    }
}

/// The symbol rates of 5.2, in the order INFO1a numbers them: "0 represents
/// 2400 and a 5 represents 3429".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolRate {
    S2400,
    S2743,
    S2800,
    S3000,
    S3200,
    S3429,
}

impl SymbolRate {
    pub const ALL: [Self; 6] = [
        Self::S2400,
        Self::S2743,
        Self::S2800,
        Self::S3000,
        Self::S3200,
        Self::S3429,
    ];

    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    pub fn index(self) -> u32 {
        self as u32
    }

    /// Symbols per second, to the nearest: 2743 and 3429 are 8/7 and 10/7 of
    /// 2400 and 3000, and are named by their round figures.
    pub fn nominal(self) -> u32 {
        match self {
            Self::S2400 => 2400,
            Self::S2743 => 2743,
            Self::S2800 => 2800,
            Self::S3000 => 3000,
            Self::S3200 => 3200,
            Self::S3429 => 3429,
        }
    }
}

/// Bits into a sequence, least significant first.
fn put(bits: &mut Vec<bool>, value: u32, width: usize) {
    for i in 0..width {
        bits.push(value >> i & 1 == 1);
    }
}

/// A field out of a sequence, least significant first.
fn get(bits: &[bool], from: usize, width: usize) -> u32 {
    (0..width).fold(0, |value, i| value | u32::from(bits[from + i]) << i)
}

/// A frequency offset field: ten bits of two's complement in steps of 0.02 Hz,
/// with -512 meaning "this field is to be ignored" (Tables 15 and 16).
fn offset_from(raw: u32) -> Option<f64> {
    let signed = if raw & 0x200 != 0 { raw as i32 - 0x400 } else { raw as i32 };
    (signed != -512).then(|| f64::from(signed) * 0.02)
}

fn offset_to(hz: Option<f64>) -> u32 {
    let steps = hz.map_or(-512, |hz| ((hz / 0.02).round() as i32).clamp(-511, 511));
    (steps & 0x3ff) as u32
}

/// Fill, sync, information, CRC and fill: a whole sequence around `info`.
fn frame(info: &[bool]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(info.len() + 32);
    bits.extend(FILL);
    bits.extend(SYNC);
    bits.extend_from_slice(info);
    put(&mut bits, u32::from(crc(info)), 16);
    bits.extend(FILL);
    bits
}

/// The information of a sequence that is `length` bits long, if its frame
/// sync is where it should be and its CRC checks.
///
/// The fill ones in front are required, since they are what makes the sync
/// findable in a stream of anything; the ones after are not, since nothing
/// depends on them and a receiver has already read all it needs by then.
pub fn unframe(bits: &[bool], length: usize) -> Option<&[bool]> {
    if bits.len() + FILL.len() < length || length < INFORMATION + 16 + FILL.len() {
        return None;
    }
    if bits[..FILL.len()] != FILL || bits[FILL.len()..INFORMATION] != SYNC {
        return None;
    }
    let info = &bits[INFORMATION..length - 16 - FILL.len()];
    let sent = get(bits, length - 16 - FILL.len(), 16) as u16;
    (crc(info) == sent).then_some(info)
}

/// INFO0a or INFO0c (Table 14): one modem's capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Info0 {
    /// Bits 12, 13 and 14: symbol rates 2743, 2800 and 3429 supported.
    pub rate_2743: bool,
    pub rate_2800: bool,
    pub rate_3429: bool,
    /// Bits 15 to 18: which carriers this transmitter can use at 3000 and at
    /// 3200 symbols per second.
    pub low_carrier_3000: bool,
    pub high_carrier_3000: bool,
    pub low_carrier_3200: bool,
    pub high_carrier_3200: bool,
    /// Bit 19: "Set to 0 indicates that transmission with a symbol rate of
    /// 3429 is disallowed."
    pub transmit_3429: bool,
    /// Bit 20: the transmitter can go below the nominal power.
    pub can_reduce_power: bool,
    /// Bits 21:23: how many symbol rate steps apart the two directions may
    /// be, 0 to 5.
    pub asymmetry: u8,
    /// Bit 24: sent by a CME modem.
    pub cme: bool,
    /// Bit 25: signal constellations of up to 1664 points.
    pub constellation_1664: bool,
    /// Bits 26:27: transmit clock, 0 internal, 1 synchronised to the receive
    /// timing, 2 external.
    pub clock: u8,
    /// Bit 28: an INFO0 from the far end has been received correctly -- set
    /// only during error recovery.
    pub acknowledge: bool,
}

impl Info0 {
    pub fn to_bits(&self) -> Vec<bool> {
        let mut info = Vec::with_capacity(17);
        for flag in [
            self.rate_2743,
            self.rate_2800,
            self.rate_3429,
            self.low_carrier_3000,
            self.high_carrier_3000,
            self.low_carrier_3200,
            self.high_carrier_3200,
            self.transmit_3429,
            self.can_reduce_power,
        ] {
            info.push(flag);
        }
        put(&mut info, u32::from(self.asymmetry), 3);
        info.push(self.cme);
        info.push(self.constellation_1664);
        put(&mut info, u32::from(self.clock), 2);
        info.push(self.acknowledge);
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, INFO0_BITS)?;
        Some(Self {
            rate_2743: info[0],
            rate_2800: info[1],
            rate_3429: info[2],
            low_carrier_3000: info[3],
            high_carrier_3000: info[4],
            low_carrier_3200: info[5],
            high_carrier_3200: info[6],
            transmit_3429: info[7],
            can_reduce_power: info[8],
            asymmetry: get(info, 9, 3) as u8,
            cme: info[12],
            constellation_1664: info[13],
            clock: get(info, 14, 2) as u8,
            acknowledge: info[16],
        })
    }
}

/// What the call modem found for one symbol rate (Table 15, bits 25:33 and
/// the five nine-bit fields after them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probed {
    /// The high carrier, from the answer modem to the call modem.
    pub high_carrier: bool,
    /// Pre-emphasis filter index, 0 to 10 (Tables 3 and 4).
    pub pre_emphasis: u8,
    /// Projected maximum data rate as a multiple of 2400 bit/s, 0 to 14. Zero
    /// says the symbol rate cannot be used.
    pub max_rate: u8,
}

impl Probed {
    fn put(&self, info: &mut Vec<bool>) {
        info.push(self.high_carrier);
        put(info, u32::from(self.pre_emphasis), 4);
        put(info, u32::from(self.max_rate), 4);
    }

    fn get(info: &[bool], from: usize) -> Self {
        Self {
            high_carrier: info[from],
            pre_emphasis: get(info, from + 1, 4) as u8,
            max_rate: get(info, from + 5, 4) as u8,
        }
    }
}

/// INFO1c (Table 15): the call modem's results of probing the line from the
/// answer modem.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Info1c {
    /// Bits 12:14: power reduction the answer modem's transmitter is to make,
    /// in dB.
    pub min_power_reduction: u8,
    /// Bits 15:17: further reduction the call modem's receiver can tolerate.
    pub additional_power_reduction: u8,
    /// Bits 18:24: length of the call modem's MD in phase 3, in 35 ms steps.
    pub md_length: u8,
    /// Bits 25:78: for each symbol rate, 2400 to 3429.
    pub probed: [Probed; 6],
    /// Bits 79:88: the 1050 Hz probing tone's offset as received, or none.
    pub frequency_offset: Option<f64>,
}

impl Info1c {
    pub fn to_bits(&self) -> Vec<bool> {
        let mut info = Vec::with_capacity(77);
        put(&mut info, u32::from(self.min_power_reduction), 3);
        put(&mut info, u32::from(self.additional_power_reduction), 3);
        put(&mut info, u32::from(self.md_length), 7);
        for probed in &self.probed {
            probed.put(&mut info);
        }
        put(&mut info, offset_to(self.frequency_offset), 10);
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, INFO1C_BITS)?;
        let mut probed = [Probed::default(); 6];
        for (i, slot) in probed.iter_mut().enumerate() {
            *slot = Probed::get(info, 13 + 9 * i);
        }
        Some(Self {
            min_power_reduction: get(info, 0, 3) as u8,
            additional_power_reduction: get(info, 3, 3) as u8,
            md_length: get(info, 6, 7) as u8,
            probed,
            frequency_offset: offset_from(get(info, 67, 10)),
        })
    }
}

/// INFO1a (Table 16): what the answer modem has settled for both directions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info1a {
    /// Bits 12:14 and 15:17: the call modem's transmit power reduction, and
    /// what the answer modem's receiver could stand on top of it.
    pub min_power_reduction: u8,
    pub additional_power_reduction: u8,
    /// Bits 18:24: length of the answer modem's MD in phase 3, in 35 ms steps.
    pub md_length: u8,
    /// Bits 25, 26:29 and 30:33: carrier, pre-emphasis and projected rate from
    /// the call modem to the answer modem.
    pub probed: Probed,
    /// Bits 34:36: the symbol rate from the answer modem to the call modem.
    pub answer_to_call: SymbolRate,
    /// Bits 37:39: the symbol rate from the call modem to the answer modem.
    pub call_to_answer: SymbolRate,
    /// Bits 40:49: the 1050 Hz probing tone's offset as received, or none.
    pub frequency_offset: Option<f64>,
}

impl Info1a {
    pub fn to_bits(&self) -> Vec<bool> {
        let mut info = Vec::with_capacity(38);
        put(&mut info, u32::from(self.min_power_reduction), 3);
        put(&mut info, u32::from(self.additional_power_reduction), 3);
        put(&mut info, u32::from(self.md_length), 7);
        self.probed.put(&mut info);
        put(&mut info, self.answer_to_call.index(), 3);
        put(&mut info, self.call_to_answer.index(), 3);
        put(&mut info, offset_to(self.frequency_offset), 10);
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, INFO1A_BITS)?;
        Some(Self {
            min_power_reduction: get(info, 0, 3) as u8,
            additional_power_reduction: get(info, 3, 3) as u8,
            md_length: get(info, 6, 7) as u8,
            probed: Probed::get(info, 13),
            // Six and seven are not symbol rates. A sequence that checks and
            // names one is not one this can act on.
            answer_to_call: SymbolRate::from_index(get(info, 22, 3))?,
            call_to_answer: SymbolRate::from_index(get(info, 25, 3))?,
            frequency_offset: offset_from(get(info, 28, 10)),
        })
    }
}

/// INFO0d (Table 7/V.90): a V.90 digital modem's capabilities.
///
/// Bits 12 to 28 are INFO0a's, word for word -- the digital modem falls back
/// to V.34 as readily as any modem does, and says what it can do there in the
/// same place. What follows is what only a modem on a digital network can say:
/// how loud it will be, where that is measured, and which companding law the
/// network it sits on uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Info0d {
    /// Bits 12:28, laid out as INFO0a's. Bits 26:27 are "Reserved for the
    /// ITU" here, where V.34 has its transmit clock, so they read as zero.
    pub v34: Info0,
    /// Bits 29:32: "Digital modem nominal transmit power for Phase 2 ... in
    /// -1 dBm0 steps where 0 represents -6 dBm0 and 15 represents -21 dBm0".
    pub nominal_power: u8,
    /// Bits 33:37: "Maximum digital modem transmit power ... in -0.5 dBm0
    /// steps where 0 represents -0.5 dBm0 and 31 represents -16 dBm0".
    pub max_power: u8,
    /// Bit 38: the power is measured "at the output of the codec" rather than
    /// at the digital modem's terminals.
    pub power_at_codec: bool,
    /// Bit 39: "PCM coding in use by digital modem: 0 = mu-law, 1 = A-law".
    pub a_law: bool,
    /// Bit 40: V.90 with an upstream symbol rate of 3429.
    pub upstream_3429: bool,
}

impl Info0d {
    /// Bits 29:32 as a level.
    pub fn nominal_dbm0(&self) -> f64 {
        -6.0 - f64::from(self.nominal_power)
    }

    /// Bits 33:37 as a level. This is the ceiling Table 15/V.90 turns into a
    /// limit on the constellations the analogue modem may ask for.
    pub fn max_dbm0(&self) -> f64 {
        -0.5 * (f64::from(self.max_power) + 1.0)
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let info0 = self.v34.to_bits();
        // The first seventeen information bits are INFO0a's; take them from
        // its own encoding so the two layouts cannot drift apart.
        let mut info: Vec<bool> = unframe(&info0, INFO0_BITS).expect("an INFO0 unframes").to_vec();
        info[14] = false;
        info[15] = false;
        put(&mut info, u32::from(self.nominal_power), 4);
        put(&mut info, u32::from(self.max_power), 5);
        info.push(self.power_at_codec);
        info.push(self.a_law);
        info.push(self.upstream_3429);
        // Bit 41: "Reserved for the ITU: This bit is set to 0".
        info.push(false);
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, INFO0D_BITS)?;
        let v34 = Info0 {
            rate_2743: info[0],
            rate_2800: info[1],
            rate_3429: info[2],
            low_carrier_3000: info[3],
            high_carrier_3000: info[4],
            low_carrier_3200: info[5],
            high_carrier_3200: info[6],
            transmit_3429: info[7],
            can_reduce_power: info[8],
            asymmetry: get(info, 9, 3) as u8,
            cme: info[12],
            constellation_1664: info[13],
            // "not interpreted by the analogue modem".
            clock: 0,
            acknowledge: info[16],
        };
        Some(Self {
            v34,
            nominal_power: get(info, 17, 4) as u8,
            max_power: get(info, 21, 5) as u8,
            power_at_codec: info[26],
            a_law: info[27],
            upstream_3429: info[28],
        })
    }
}

/// INFO1a when V.90 is selected (Table 10/V.90): what the analogue modem asks
/// the digital modem for before phase 3.
///
/// Less than V.34's INFO1a, because there is less to settle. The digital
/// modem's direction is not a symbol rate chosen from probing -- it is 8000,
/// fixed by the network -- so what goes downstream in its place is the one
/// codeword the digital modem is to train with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info1aPcm {
    /// Bits 18:24: the analogue modem's MD in phase 3, in 35 ms steps.
    pub md_length: u8,
    /// Bits 25:31: UINFO, "Ucode of the PCM codeword to be used by the digital
    /// modem for the 2 point train ... UINFO shall be greater than 66".
    pub uinfo: u8,
    /// Bits 34:36: the upstream symbol rate, "an integer between 3 and 5".
    pub upstream: SymbolRate,
    /// Bits 40:49: the 1050 Hz probing tone's offset as received, or none.
    pub frequency_offset: Option<f64>,
}

impl Info1aPcm {
    pub fn to_bits(&self) -> Vec<bool> {
        let mut info = Vec::with_capacity(38);
        // Bits 12:17: "Reserved for the ITU".
        put(&mut info, 0, 6);
        put(&mut info, u32::from(self.md_length), 7);
        put(&mut info, u32::from(self.uinfo), 7);
        // Bits 32:33: reserved again.
        put(&mut info, 0, 2);
        put(&mut info, self.upstream.index(), 3);
        put(&mut info, PCM_SYMBOL_RATE, 3);
        put(&mut info, offset_to(self.frequency_offset), 10);
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, INFO1A_BITS)?;
        if get(info, 25, 3) != PCM_SYMBOL_RATE {
            return None;
        }
        // 3000, 3200 and 3429 are the only upstream rates V.90 allows (6.2).
        let upstream = match get(info, 22, 3) {
            3..=5 => SymbolRate::from_index(get(info, 22, 3))?,
            _ => return None,
        };
        Some(Self {
            md_length: get(info, 6, 7) as u8,
            uinfo: get(info, 13, 7) as u8,
            upstream,
            frequency_offset: offset_from(get(info, 28, 10)),
        })
    }
}

/// Any of them, as a receiver hands it over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Info {
    Info0(Info0),
    /// INFO1c, and V.90's INFO1d, which is the same sequence.
    Info1c(Info1c),
    Info1a(Info1a),
    /// V.90's digital modem's INFO0.
    Info0d(Info0d),
    /// V.90's INFO1a, asking for phase 3 of V.90.
    Info1aPcm(Info1aPcm),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sequence_is_the_length_its_table_says() {
        assert_eq!(Info0::default().to_bits().len(), INFO0_BITS);
        assert_eq!(Info1c::default().to_bits().len(), INFO1C_BITS);
        let info1a = Info1a {
            min_power_reduction: 0,
            additional_power_reduction: 0,
            md_length: 0,
            probed: Probed::default(),
            answer_to_call: SymbolRate::S3429,
            call_to_answer: SymbolRate::S3200,
            frequency_offset: None,
        };
        assert_eq!(info1a.to_bits().len(), INFO1A_BITS);
    }

    #[test]
    fn the_fill_and_sync_are_where_the_tables_put_them() {
        let bits = Info0::default().to_bits();
        let printed = |b: &[bool]| b.iter().map(|&x| if x { '1' } else { '0' }).collect::<String>();
        assert_eq!(printed(&bits[0..4]), "1111");
        assert_eq!(printed(&bits[4..12]), "01110010");
        assert_eq!(printed(&bits[45..49]), "1111");
    }

    #[test]
    fn a_crc_sent_after_its_bits_leaves_nothing_in_the_register() {
        // The property that makes Figure 14 a CRC: shift the register's own
        // contents in after the information, bit 0 first, and it comes back to
        // zero -- whatever the information was.
        for seed in 0..200u32 {
            let info: Vec<bool> = (0..77).map(|i| (seed.wrapping_mul(2_654_435_761) >> (i % 32)) & 1 == 1).collect();
            let mut with_crc = info.clone();
            put(&mut with_crc, u32::from(crc(&info)), 16);
            assert_eq!(crc(&with_crc), 0, "seed {seed}");
        }
    }

    #[test]
    fn the_crc_polynomial_is_x16_x12_x5_1() {
        // A single one shifted into a register of zeros comes out as the
        // polynomial itself, read from the low end: taps 15, 10 and 3 in a
        // register numbered as Figure 14 numbers it.
        assert_eq!(shift(0, true), 0x8408);
        // And the CRC of nothing is the ones it was loaded with.
        assert_eq!(crc(&[]), 0xffff);
    }

    #[test]
    fn every_field_comes_back_as_it_went() {
        let info0 = Info0 {
            rate_2743: true,
            rate_2800: false,
            rate_3429: true,
            low_carrier_3000: true,
            high_carrier_3000: false,
            low_carrier_3200: true,
            high_carrier_3200: true,
            transmit_3429: true,
            can_reduce_power: true,
            asymmetry: 5,
            cme: false,
            constellation_1664: true,
            clock: 2,
            acknowledge: true,
        };
        assert_eq!(Info0::from_bits(&info0.to_bits()), Some(info0));

        let mut info1c = Info1c {
            min_power_reduction: 3,
            additional_power_reduction: 7,
            md_length: 127,
            probed: [Probed::default(); 6],
            frequency_offset: Some(-3.24),
        };
        for (i, p) in info1c.probed.iter_mut().enumerate() {
            *p = Probed { high_carrier: i % 2 == 1, pre_emphasis: i as u8 + 4, max_rate: 14 - i as u8 };
        }
        let back = Info1c::from_bits(&info1c.to_bits()).expect("it did not check");
        assert_eq!(back.probed, info1c.probed);
        assert_eq!(back.md_length, 127);
        assert!((back.frequency_offset.unwrap() + 3.24).abs() < 1e-9);

        let info1a = Info1a {
            min_power_reduction: 1,
            additional_power_reduction: 2,
            md_length: 9,
            probed: Probed { high_carrier: true, pre_emphasis: 10, max_rate: 13 },
            answer_to_call: SymbolRate::S3429,
            call_to_answer: SymbolRate::S2743,
            frequency_offset: None,
        };
        assert_eq!(Info1a::from_bits(&info1a.to_bits()), Some(info1a));
    }

    #[test]
    fn one_wrong_bit_anywhere_is_caught() {
        let bits = Info1c::default().to_bits();
        for i in 0..INFO1C_BITS - FILL.len() {
            let mut spoiled = bits.clone();
            spoiled[i] = !spoiled[i];
            assert_eq!(Info1c::from_bits(&spoiled), None, "bit {i}");
        }
    }

    #[test]
    fn the_frequency_offset_is_two_s_complement_and_minus_512_is_nothing() {
        assert_eq!(offset_from(0x200), None);
        assert_eq!(offset_from(0x1ff), Some(511.0 * 0.02));
        assert_eq!(offset_from(0x3ff), Some(-0.02));
        assert_eq!(offset_to(None), 0x200);
        assert_eq!(offset_to(Some(-0.02)), 0x3ff);
    }

    /// Table 7/V.90: sixty-two bits, INFO0a's first seventeen information
    /// bits in the same places, and the law in bit 39.
    #[test]
    fn info0d_is_info0a_with_the_digital_modem_s_levels_after_it() {
        let info0d = Info0d {
            v34: Info0 { rate_3429: true, low_carrier_3200: true, asymmetry: 5, acknowledge: true, ..Info0::default() },
            nominal_power: 3,
            max_power: 31,
            power_at_codec: false,
            a_law: true,
            upstream_3429: true,
        };
        let bits = info0d.to_bits();
        assert_eq!(bits.len(), INFO0D_BITS);
        assert_eq!(Info0d::from_bits(&bits), Some(info0d));
        // Absolute bit numbers, as the table prints them.
        assert!(bits[14], "bit 14 is 3429 in V.34 mode");
        assert!(bits[28], "bit 28 is the acknowledgement");
        assert!(bits[39], "bit 39 is the law");
        assert!(bits[40], "bit 40 is 3429 upstream");
        assert!(!bits[41], "bit 41 is reserved");
        assert_eq!(get(&bits, 33, 5), 31);
        assert_eq!(&bits[58..62], &FILL);
        assert_eq!(info0d.nominal_dbm0(), -9.0);
        assert_eq!(info0d.max_dbm0(), -16.0);
        // It is not an INFO0, and an INFO0 is not one of these.
        assert_eq!(Info0::from_bits(&bits), None);
        assert_eq!(Info0d::from_bits(&Info0::default().to_bits()), None);
    }

    /// Table 10/V.90: V.34's length, UINFO in bits 25:31, and six in 37:39 --
    /// which V.34's own INFO1a refuses, so the two cannot be mistaken.
    #[test]
    fn a_v90_info1a_carries_uinfo_and_names_8000() {
        let asked = Info1aPcm { md_length: 0, uinfo: 73, upstream: SymbolRate::S3200, frequency_offset: Some(-0.5) };
        let bits = asked.to_bits();
        assert_eq!(bits.len(), INFO1A_BITS);
        assert_eq!(Info1aPcm::from_bits(&bits), Some(asked));
        assert_eq!(get(&bits, 25, 7), 73);
        assert_eq!(get(&bits, 34, 3), 4);
        assert_eq!(get(&bits, 37, 3), 6);
        assert_eq!(Info1a::from_bits(&bits), None, "V.34 took a V.90 INFO1a for its own");
        // And the other way round.
        let v34 = Info1a {
            min_power_reduction: 0,
            additional_power_reduction: 0,
            md_length: 0,
            probed: Probed::default(),
            answer_to_call: SymbolRate::S3429,
            call_to_answer: SymbolRate::S3200,
            frequency_offset: None,
        };
        assert_eq!(Info1aPcm::from_bits(&v34.to_bits()), None);
    }
}
