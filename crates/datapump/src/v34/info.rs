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
//!
//! V.92 leaves that shape alone and changes three things inside it. Two bits
//! of each INFO0, reserved in V.90, now say "I can do V.92" and "let us skip
//! the probing" -- and INFO0d and INFO0a put those two the other way round
//! from each other (Tables 15 and 16/V.92), which is the one trap in the whole
//! clause. Bit 70 of INFO1d, a carrier flag in V.90, now says whether the
//! channel will carry PCM upstream at all (Table 17). And the answer modem has
//! two more layouts to choose between: Table 18, which asks for PCM upstream by
//! naming 8000 symbols a second in *both* directions, and Table 19, which asks
//! for V.90 data mode after a short phase 2 and has to name its own upstream
//! carrier, because there was no probing to name one.
//!
//! One more sequence rides on the same modulation and the same frame: the MH
//! sequence of Table 32/V.92, forty bits of it, by which one modem asks the
//! other to hold the call while its owner takes a telephone call.

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

/// Bits in an MH sequence, fill to fill (Table 32/V.92).
///
/// The INFO frame with an eight-bit information field: four fill, eight of
/// frame sync, four signal indication bits, four information bits, sixteen of
/// CRC and four more fill. Forty bits is 66.67 ms at 600 bit/s.
pub const MH_BITS: usize = 40;

/// Bits 37:39 of an INFO1a that asks for V.90: "Symbol rate of 8000 to be used
/// by the digital modem: The integer 6" (Table 10/V.90). Six is not one of
/// V.34's symbol rates, which is what tells the two INFO1a apart.
///
/// V.92's Table 18 puts the same integer in bits 34:36 as well, which is how
/// it asks for 8000 symbols a second in the upstream direction too.
pub const PCM_SYMBOL_RATE: u32 = 6;

/// The lowest U_INFO an INFO1a may name (Tables 10/V.90, 18 and 19/V.92).
///
/// "U_INFO shall be greater than 66", because the codeword has to be loud
/// enough for the two-point train to be heard at all.
pub const UINFO_LOWEST: u8 = 67;

/// The highest U_INFO an INFO1a may name.
///
/// Not printed: the tables give only the lower bound. The upper one comes from
/// 8.4.4/V.90, where the digital modem trains Sd on the codeword whose Ucode is
/// "16 + U_INFO", and there are only 128 Ucodes. A sequence naming more than
/// this still parses -- refusing a far end over a value we could merely not use
/// would cost the whole call -- and the end that *chooses* U_INFO stays inside.
pub const UINFO_HIGHEST: u8 = 111;

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

/// A four-bit pattern into a sequence, leftmost bit first in time.
///
/// Clause 8/V.92 draws the line: "values given as bit patterns are transmitted
/// leftmost bit first in time and values given as integers are transmitted
/// least-significant bit first in time", in the tables it lists -- which
/// include Tables 32 and 33, the MH ones. Everything those two hold is a
/// pattern, so `0011` means bit 12 = 0, 13 = 0, 14 = 1, 15 = 1. The nibble is
/// carried here with its leftmost bit in bit 3, so that `0011` is 3 and Table
/// 33's codes count 1 to 13 as its rows do.
///
/// A pattern is four bits and no more. Anything wider is a caller that has lost
/// count, and quietly sending the low four would put a different code on the
/// wire from the one it meant.
fn put_pattern(bits: &mut Vec<bool>, nibble: u8) {
    debug_assert!(nibble < 16, "a bit pattern wider than the four bits it is sent in");
    for i in (0..4).rev() {
        bits.push(nibble >> i & 1 == 1);
    }
}

/// A four-bit pattern out of a sequence, leftmost bit first in time.
fn get_pattern(bits: &[bool], from: usize) -> u8 {
    (0..4).fold(0, |value, i| value << 1 | u8::from(bits[from + i]))
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
    ///
    /// The two bits themselves, whatever the sequence carrying them means by
    /// them. V.34 means the clock source; V.92's INFO0a overloads the same two
    /// bits with the V.92 capability and the short phase 2 request, read
    /// through `pcm_flags` (N-3, `spec-phase2-signals.md`), because Table
    /// 16/V.92 has no clock field at all.
    ///
    /// An INFO0d's pair is **not** here. V.90's Table 7 reserves its bits 26:27
    /// and sets both to 0, so that sequence carries no clock, and V.92's Table
    /// 15 gives the pair to the same two flags the other way round. They travel
    /// in bits 2 and 3 of this field instead -- `Info0d::pcm_flags` and
    /// `Info0d::set_pcm_flags` -- so that an `Info0` naming an external
    /// transmit clock can never go out inside an INFO0d as "V.92 capability:
    /// 1". Nothing in the low two bits reaches an INFO0d's wire bits, and
    /// nothing in the high two reaches an INFO0a's.
    pub clock: u8,
    /// Bit 28: an INFO0 from the far end has been received correctly -- set
    /// only during error recovery.
    pub acknowledge: bool,
}

/// Bits 26 and 27 of an INFO0, as V.92 reads them (Tables 15 and 16/V.92).
///
/// V.34 has its transmit-clock source there. V.90 reserved both, "set to 0 by
/// the analogue modem" and by the digital one, and neither interprets them. So
/// zero -- which is what a V.90 modem sends and what `Default` gives -- reads
/// as "not V.92, no short phase 2" whichever way round the two are, and that
/// is why a V.92 modem can set its own bit before it knows anything about the
/// far end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PcmFlags {
    /// "V.92 capability: 1". INFO0a bit 26; INFO0d bit 27.
    pub v92: bool,
    /// "Set to 1 requests short Phase 2 to be used". INFO0a bit 27; INFO0d
    /// bit 26.
    pub short_phase2: bool,
}

impl Info0 {
    /// Bits 26:27 as **INFO0a** lays them out (Table 16/V.92): bit 26 is the
    /// V.92 capability and bit 27 the short phase 2 request.
    ///
    /// INFO0d has the two the other way round (`Info0d::pcm_flags`). 9.3 and
    /// 9.4 spell the asymmetry out -- "bit 27 of INFO0d and bit 26 of INFO0a"
    /// for the capability, "bit 26 of INFO0d and bit 27 of INFO0a" for the
    /// request -- so the two cannot be read through one accessor.
    pub fn pcm_flags(&self) -> PcmFlags {
        PcmFlags { v92: self.clock & 1 == 1, short_phase2: self.clock & 2 == 2 }
    }

    /// Write bits 26:27 as INFO0a lays them out.
    pub fn set_pcm_flags(&mut self, flags: PcmFlags) {
        self.clock = u8::from(flags.v92) | u8::from(flags.short_phase2) << 1;
    }

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

/// Bits 71:78 of a V.92 INFO1d: what the digital modem's probe found at 3429
/// (Table 17/V.92).
///
/// Eight bits, not nine, because V.92 took bit 70 for its PCM-upstream flag.
/// The other two fields have not moved -- "the coding of these 8 bits is
/// identical to that for bits 26-33" -- so a V.90 INFO1c parser reads a V.92
/// INFO1d correctly and only bit 70 changes meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probed3429 {
    /// Bits 71:74: pre-emphasis filter index, 0 to 10.
    pub pre_emphasis: u8,
    /// Bits 75:78: projected maximum data rate as a multiple of 2400 bit/s.
    pub max_rate: u8,
}

impl Info1c {
    /// Bit 70 of an INFO1d, read as V.92 reads it: "Set to 0 indicates that
    /// the channel does not support PCM upstream" (Table 17/V.92).
    ///
    /// Only worth reading when both modems have shown V.92 capability. In V.90
    /// the same bit is the 3429 high-carrier flag, so a V.90 digital modem may
    /// have set it for a reason of its own and means nothing by it here (N-4).
    /// A set bit is a permission and not an instruction: 8.4.1 forbids Table 18
    /// when it is clear, and leaves the analogue modem free either way when it
    /// is set.
    pub fn pcm_upstream(&self) -> bool {
        self.probed[5].high_carrier
    }

    /// Write bit 70 as V.92's digital modem writes it.
    pub fn set_pcm_upstream(&mut self, supported: bool) {
        self.probed[5].high_carrier = supported;
    }

    /// Bits 71:78: the 3429 probing result, with no carrier bit in front of
    /// it, which is how V.92 reads that field.
    pub fn probed_3429(&self) -> Probed3429 {
        Probed3429 { pre_emphasis: self.probed[5].pre_emphasis, max_rate: self.probed[5].max_rate }
    }

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
    /// Bits 12:28, laid out as INFO0a's.
    ///
    /// Bits 26:27 are "Reserved for the ITU" in V.90, where V.34 has its
    /// transmit clock, and V.92 gave them to the short phase 2 request and the
    /// V.92 capability -- in the opposite order to INFO0a. They travel above
    /// V.34's own two bits inside `Info0::clock`, because that is the one place
    /// this layout has for them, and they are read and written through
    /// `pcm_flags` and `set_pcm_flags`, never as a clock. A transmit clock left
    /// in this `Info0` goes nowhere: V.90 sets both bits to 0 in an INFO0d, and
    /// so does this.
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
    /// Where this layout's two V.92 bits sit inside `Info0::clock`: above
    /// V.34's own two, never in them.
    ///
    /// V.90's Table 7 reserves bits 26:27 of INFO0d and sets both to 0, so an
    /// INFO0d carries no transmit clock; Table 15/V.92 gives the pair to the
    /// short phase 2 request and the V.92 capability. Keeping the two readings
    /// apart is what stops an `Info0` naming V.34's external transmit clock
    /// (the value 2) from going out inside an INFO0d as "V.92 capability: 1",
    /// which by 9.3 would take both ends to the Table 17 INFO1d.
    const FLAGS_SHIFT: u32 = 2;

    /// Bits 26:27 as **INFO0d** lays them out (Table 15/V.92): bit 26 is the
    /// short phase 2 request and bit 27 the V.92 capability.
    ///
    /// The other way round from INFO0a, which is not a slip of the pen: the
    /// procedure text names them separately in both directions (9.3, 9.4).
    pub fn pcm_flags(&self) -> PcmFlags {
        let flags = self.v34.clock >> Self::FLAGS_SHIFT;
        PcmFlags { short_phase2: flags & 1 == 1, v92: flags & 2 == 2 }
    }

    /// Write bits 26:27 as INFO0d lays them out.
    ///
    /// Note that a modem whose phase 2 rebuilds its INFO0d around its own
    /// `Info0` capabilities carries the two bits in *that* `Info0`, so this is
    /// the call to make on the sequence that actually goes out.
    pub fn set_pcm_flags(&mut self, flags: PcmFlags) {
        let pair = u8::from(flags.short_phase2) | u8::from(flags.v92) << 1;
        let v34 = self.v34.clock & ((1 << Self::FLAGS_SHIFT) - 1);
        self.v34.clock = v34 | pair << Self::FLAGS_SHIFT;
    }

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
        // The first seventeen information bits are INFO0a's; take them from
        // its own encoding so the two layouts cannot drift apart -- all but
        // information bits 14 and 15, absolute 26 and 27, which this layout
        // does not share. V.90 reserves them and "set[s] to 0" both, so V.34's
        // clock is left behind here, and V.92's two flags are written in over
        // the zeros. A modem that never asks for V.92 sends what it always
        // sent, whatever clock its `Info0` names.
        let info0 = Info0 { clock: 0, ..self.v34 }.to_bits();
        let mut info: Vec<bool> = unframe(&info0, INFO0_BITS).expect("an INFO0 unframes").to_vec();
        let flags = self.pcm_flags();
        info[14] = flags.short_phase2;
        info[15] = flags.v92;
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
            // Bits 26:27, which are not a clock in this layout: V.90 reserves
            // them and a V.90 digital modem sends zeros, V.92 puts the short
            // phase 2 request and the V.92 capability there. Kept as they
            // arrived but above V.34's two bits, so that `pcm_flags` reads
            // them and nothing reads them as a clock.
            clock: (get(info, 14, 2) as u8) << Self::FLAGS_SHIFT,
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

/// INFO1a when PCM upstream is selected (Table 18/V.92): the analogue modem
/// asking to send PCM as well as receive it.
///
/// V.34's seventy bits again, and told from every other INFO1a by its two
/// symbol rates. Where V.90's Table 10 names one of V.34's rates for the
/// upstream, this names "the integer 6" in bits 34:36 as well as in 37:39:
/// 8000 symbols a second in both directions, which is what PCM upstream is.
/// Bits 40:49, V.90's frequency offset, go back to the ITU as ten ones -- ten
/// ones because ten zeros in a DPSK stream are ten symbols with no reversal in
/// them, which is a tone.
///
/// What it carries instead is the shape of the precoder and prefilter the
/// digital modem may design: how many sections, how many coefficients in all,
/// and how many in the longest one. The digital modem answers in CPd.
///
/// "The analogue modem shall not use this sequence if bit 70 of INFO1d is
/// clear" (8.4.1), and 9.3 allows it only when both modems have shown V.92
/// capability. Neither is a property of these bits, so neither is checked here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info1aPcmUp {
    /// Bits 12:13: "Number of filter sections in precoder and prefilter". 0 is
    /// p1 and z2, 1 adds z1, 2 adds p2, 3 has all four -- so bit 12 says z1 is
    /// supported and bit 13 says p2 is.
    pub sections: u8,
    /// Bits 14:15: L_tot, the most coefficients in all four sections together,
    /// "in multiples of 64 starting at 192" -- 192, 256, 320, 384.
    pub ltot_code: u8,
    /// Bits 16:17: L_max, the most in any one section, "in multiples of 64
    /// starting at 128" -- 128, 192, 256, 320.
    pub lmax_code: u8,
    /// Bits 18:24: the analogue modem's MD in phase 3, "in 276 symbol (34.5
    /// ms) increments" -- not the 35 ms every other INFO counts in. 276 is 23
    /// twelve-symbol upstream data frames, which 35 ms would not be.
    pub md_length: u8,
    /// Bits 25:31: U_INFO, "Ucode of the PCM codeword to be used by the
    /// digital modem for the 2 point train", `UINFO_LOWEST` to
    /// `UINFO_HIGHEST`.
    pub uinfo: u8,
}

/// Nothing asked for, except the one field that has no harmless value.
///
/// Zero is the natural default everywhere else in these layouts and is not a
/// U_INFO at all -- Table 18 says "U_INFO shall be greater than 66" -- so the
/// default names the quietest codeword the Recommendation allows instead, and a
/// sequence built up from `..Info1aPcmUp::default()` is sendable before anyone
/// has thought about it.
impl Default for Info1aPcmUp {
    fn default() -> Self {
        Self { sections: 0, ltot_code: 0, lmax_code: 0, md_length: 0, uinfo: UINFO_LOWEST }
    }
}

impl Info1aPcmUp {
    /// Whether the U_INFO this names is one we may send.
    ///
    /// "U_INFO shall be greater than 66" (Table 18/V.92), and no more than
    /// `UINFO_HIGHEST`, because the digital modem trains Sd on Ucode
    /// 16 + U_INFO and there are only 128 of them (8.4.4/V.90).
    ///
    /// Only the end that *chooses* U_INFO is held to this. `from_bits` takes
    /// any value, because refusing a far end over a number we could merely not
    /// use would cost the whole call.
    pub fn uinfo_is_sendable(&self) -> bool {
        (UINFO_LOWEST..=UINFO_HIGHEST).contains(&self.uinfo)
    }

    pub fn to_bits(&self) -> Vec<bool> {
        debug_assert!(self.uinfo_is_sendable(), "a table 18 u_info outside 67 to 111");
        let mut info = Vec::with_capacity(38);
        put(&mut info, u32::from(self.sections), 2);
        put(&mut info, u32::from(self.ltot_code), 2);
        put(&mut info, u32::from(self.lmax_code), 2);
        put(&mut info, u32::from(self.md_length), 7);
        put(&mut info, u32::from(self.uinfo), 7);
        // Bits 32:33: "Reserved for the ITU: These bits are set to 0".
        put(&mut info, 0, 2);
        put(&mut info, PCM_SYMBOL_RATE, 3);
        put(&mut info, PCM_SYMBOL_RATE, 3);
        // Bits 40:49: "Reserved for the ITU: These bits are set to 1 ... NOTE
        // -- These bits are set to 1 to avoid generating a tone." Ten zeros in
        // a DPSK stream are ten symbols with no reversal in them, which is a
        // tone; ten ones are ten reversals, which is not.
        put(&mut info, 0x3ff, 10);
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, INFO1A_BITS)?;
        // The two dispatch fields, and nothing else: bits 32:33 and 40:49 are
        // reserved and "not interpreted by the digital modem", so a far end
        // that leaves them at some other value still connects.
        if get(info, 22, 3) != PCM_SYMBOL_RATE || get(info, 25, 3) != PCM_SYMBOL_RATE {
            return None;
        }
        Some(Self {
            sections: get(info, 0, 2) as u8,
            ltot_code: get(info, 2, 2) as u8,
            lmax_code: get(info, 4, 2) as u8,
            md_length: get(info, 6, 7) as u8,
            uinfo: get(info, 13, 7) as u8,
        })
    }
}

/// INFO1a when V.34 upstream is selected during short phase 2 (Table
/// 19/V.92): V.90 data mode, asked for without any probing having happened.
///
/// Bit for bit it is V.90's Table 10, with one exception. Short phase 2 sends
/// no INFO1d, so the sentence Table 10 leans on -- "the carrier frequency and
/// pre-emphasis filter to be used are those already indicated for this symbol
/// rate in INFO1d" -- has nothing to point at. Table 19 therefore gives bit
/// 33, reserved and zero in Table 10, to the upstream carrier. The pre-emphasis
/// filter is not signalled at all.
///
/// Nothing in the seventy bits says which of the two tables they are, and bit
/// 33 cannot say: Table 10 reserves it, "set to 0 by the analogue modem and ...
/// not interpreted by the digital modem", so a Table 10 frame that arrives with
/// it set -- stale, or a future ITU extension -- is still a Table 10 frame and
/// still has to get through phase 3. Only the phase decides, and Table 19 is
/// used "during short Phase 2" alone, which is why it is read only by a
/// receiver told it is in one (`dpsk::Receiver::in_short_phase2`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info1aV34Up {
    /// Bits 18:24, 25:31, 34:36 and 40:49, exactly as V.90's Table 10 has
    /// them. The MD length is in 35 ms steps here, not Table 18's 276 symbols.
    pub v90: Info1aPcm,
    /// Bit 33: "Set to 1 indicates that the high carrier frequency is to be
    /// used in transmitting from the analogue modem to the digital modem".
    pub high_carrier: bool,
}

impl Info1aV34Up {
    /// Information bit 21, which is absolute bit 33.
    const HIGH_CARRIER: usize = 33 - INFORMATION;

    pub fn to_bits(&self) -> Vec<bool> {
        // Table 10's own encoding, with the one bit V.92 gave a meaning to
        // written over the zero it puts there -- so the two layouts cannot
        // drift apart, as INFO0d takes its first seventeen bits from INFO0a's.
        let table10 = self.v90.to_bits();
        let mut info = unframe(&table10, INFO1A_BITS).expect("a Table 10 INFO1a unframes").to_vec();
        info[Self::HIGH_CARRIER] = self.high_carrier;
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let high_carrier = unframe(bits, INFO1A_BITS)?[Self::HIGH_CARRIER];
        Some(Self { v90: Info1aPcm::from_bits(bits)?, high_carrier })
    }
}

/// What an MH sequence says (Table 32/V.92, bits 12:15).
///
/// Six of the sixteen patterns are defined, and every one of them ends in a
/// one. "Bit combinations not defined in bits 12-15 are reserved for the ITU.
/// MH sequences with undefined bit combinations should be ignored", which is
/// why this is an `Option` at the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MhIndication {
    /// `0011`: "Request remote modem to go on-hold".
    Req,
    /// `0101`: "Indicate agreement to go on hold and timeout".
    Ack,
    /// `0111`: "Deny on-hold, request cleardown or fast reconnect".
    Nack,
    /// `1001`: "Request cleardown".
    Clrd,
    /// `1011`: "Acknowledge cleardown".
    Cda,
    /// `1101`: "Request fast reconnect".
    Frr,
}

impl MhIndication {
    pub const ALL: [Self; 6] = [Self::Req, Self::Ack, Self::Nack, Self::Clrd, Self::Cda, Self::Frr];

    /// The pattern, leftmost bit first in time, as a nibble whose bit 3 went
    /// first.
    pub fn code(self) -> u8 {
        match self {
            Self::Req => 0b0011,
            Self::Ack => 0b0101,
            Self::Nack => 0b0111,
            Self::Clrd => 0b1001,
            Self::Cda => 0b1011,
            Self::Frr => 0b1101,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|indication| indication.code() == code)
    }
}

/// The timeout period an MHack grants (Table 33/V.92, bits 16:19).
///
/// Thirteen of the sixteen codes name a period and three are "Reserved for the
/// ITU". The code is kept rather than only its length, because a reserved code
/// must not be read as a period *or* as "no limit": the Recommendation gives it
/// no meaning at all, and a far end sending `1111` must not thereby obtain an
/// unbounded hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T1 {
    /// `0001` to `1100`: 10, 20, 30 or 40 seconds, or 1, 2, 3, 4, 6, 8, 12 or
    /// 16 minutes, in seconds.
    Limit(u32),
    /// `1101`: "no limit".
    NoLimit,
    /// `0000`, `1110` or `1111`: "Reserved for the ITU", kept as it arrived.
    /// The four-bit code itself, which is all `from_code` ever puts here.
    Reserved(u8),
}

impl T1 {
    /// The twelve bounded periods in seconds, in the order Table 33 codes them
    /// from `0001` to `1100`.
    const PERIODS: [u32; 12] = [10, 20, 30, 40, 60, 120, 180, 240, 360, 480, 720, 960];

    pub fn from_code(code: u8) -> Self {
        match code {
            1..=12 => Self::Limit(Self::PERIODS[code as usize - 1]),
            13 => Self::NoLimit,
            other => Self::Reserved(other),
        }
    }

    /// The pattern this T1 is sent as.
    ///
    /// Table 33 codes twelve periods and nothing between them, so a `Limit`
    /// that is none of the twelve has no pattern of its own and goes out as the
    /// longest tabled period that does not exceed it: five minutes is granted
    /// as four. A grant is then never longer than the one intended, which
    /// falling back to `0000` would not guarantee either way -- that pattern is
    /// "Reserved for the ITU", and section 4's reading makes it no grant at all.
    /// Below ten seconds there is no period to name and `0000` is what goes
    /// out, which is the honest answer: Table 33 cannot express it.
    pub fn code(self) -> u8 {
        match self {
            Self::Limit(seconds) => {
                Self::PERIODS.iter().rposition(|&p| p <= seconds).map_or(0, |i| i as u8 + 1)
            }
            Self::NoLimit => 13,
            Self::Reserved(code) => code,
        }
    }

    /// How long the hold may last, when the code named a length at all.
    ///
    /// `None` for "no limit" and `None` for a reserved code, which are not the
    /// same thing and must not be told apart by this.
    ///
    /// Read from the pattern that will actually be sent, never from the length
    /// asked for, so that the end granting a hold cannot believe it granted
    /// longer than the wire says.
    pub fn seconds(self) -> Option<u32> {
        match Self::from_code(self.code()) {
            Self::Limit(seconds) => Some(seconds),
            Self::NoLimit | Self::Reserved(_) => None,
        }
    }
}

/// Why an MHclrd asks for the call to be cleared (Table 32/V.92, bits 16:19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleardown {
    /// `0101`: "Cleardown due to incoming call".
    IncomingCall,
    /// `0110`: "Cleardown due to outgoing call".
    OutgoingCall,
    /// `1010`: "Cleardown due to other reason".
    Other,
    /// Anything else. "Bit combinations not defined in bits 16-19 for MHclrd
    /// are reserved for the ITU and should not be interpreted by the receiving
    /// modem" -- the sequence is still a good MHclrd and still wants its MHcda;
    /// only the reason is unknown. The four-bit code itself, which is all
    /// `from_code` ever puts here.
    Unknown(u8),
}

impl Cleardown {
    pub fn code(self) -> u8 {
        match self {
            Self::IncomingCall => 0b0101,
            Self::OutgoingCall => 0b0110,
            Self::Other => 0b1010,
            Self::Unknown(code) => code,
        }
    }

    pub fn from_code(code: u8) -> Self {
        match code {
            0b0101 => Self::IncomingCall,
            0b0110 => Self::OutgoingCall,
            0b1010 => Self::Other,
            other => Self::Unknown(other),
        }
    }
}

/// Bits 16:19 of an MH sequence, read as the indication in front of them asks
/// (Table 32/V.92).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MhInformation {
    /// MHreq, MHnack, MHcda and MHfrr "repeat signal indication bits". The
    /// nibble as it arrived, so that a far end which did not repeat them can
    /// be told from one which did.
    Repeat(u8),
    /// MHack: "T1 -- Timeout period for on-hold".
    Timeout(T1),
    /// MHclrd: why the far end wants the call cleared.
    Reason(Cleardown),
}

/// An MH sequence (Table 32/V.92): forty bits on the phase 2 modulation, by
/// which one modem asks the other to hold the call.
///
/// The same frame as an INFO sequence with an eight-bit information field, and
/// the same CRC over it. MH sequences are sent back to back (9.10.1), so a run
/// of them is one group and only the first gets the leading point at an
/// arbitrary carrier phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mh {
    /// Bits 12:15.
    pub indication: MhIndication,
    /// Bits 16:19.
    pub information: MhInformation,
}

impl Mh {
    /// "Request remote modem to go on-hold".
    pub fn req() -> Self {
        Self::repeating(MhIndication::Req)
    }

    /// "Indicate agreement to go on hold and timeout".
    pub fn ack(t1: T1) -> Self {
        Self { indication: MhIndication::Ack, information: MhInformation::Timeout(t1) }
    }

    /// "Deny on-hold, request cleardown or fast reconnect".
    pub fn nack() -> Self {
        Self::repeating(MhIndication::Nack)
    }

    /// "Request cleardown".
    pub fn clrd(reason: Cleardown) -> Self {
        Self { indication: MhIndication::Clrd, information: MhInformation::Reason(reason) }
    }

    /// "Acknowledge cleardown".
    pub fn cda() -> Self {
        Self::repeating(MhIndication::Cda)
    }

    /// "Request fast reconnect".
    pub fn frr() -> Self {
        Self::repeating(MhIndication::Frr)
    }

    fn repeating(indication: MhIndication) -> Self {
        Self { indication, information: MhInformation::Repeat(indication.code()) }
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let mut info = Vec::with_capacity(8);
        put_pattern(&mut info, self.indication.code());
        put_pattern(
            &mut info,
            match self.information {
                MhInformation::Repeat(code) => code,
                MhInformation::Timeout(t1) => t1.code(),
                MhInformation::Reason(reason) => reason.code(),
            },
        );
        frame(&info)
    }

    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let info = unframe(bits, MH_BITS)?;
        let indication = MhIndication::from_code(get_pattern(info, 0))?;
        let code = get_pattern(info, 4);
        let information = match indication {
            MhIndication::Ack => MhInformation::Timeout(T1::from_code(code)),
            MhIndication::Clrd => MhInformation::Reason(Cleardown::from_code(code)),
            _ => MhInformation::Repeat(code),
        };
        Some(Self { indication, information })
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
    /// V.90's INFO1a, asking for phase 3 of V.90. Every seventy-bit INFO1a
    /// that names 8000 downstream and a V.34 rate upstream arrives as this,
    /// unless the receiver was told it is in a short phase 2.
    Info1aPcm(Info1aPcm),
    /// V.92's Table 18 INFO1a, asking for PCM upstream.
    Info1aPcmUp(Info1aPcmUp),
    /// V.92's Table 19 INFO1a, asking for V.90 data mode on a named upstream
    /// carrier. Heard only by a receiver in a short phase 2, which is the only
    /// phase that may read bit 33.
    Info1aV34Up(Info1aV34Up),
    /// A modem-on-hold sequence, heard only by a receiver asked for them.
    Mh(Mh),
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

    /// The CRC of a sequence as it sits on the wire, which is the sixteen bits
    /// between the information and the trailing fill.
    fn sent_crc(bits: &[bool]) -> u16 {
        get(bits, bits.len() - 16 - FILL.len(), 16) as u16
    }

    /// V.34's capabilities, all of them, as both V.92 INFO0 vectors of
    /// `spec-phase2-signals.md` 15 carry them: bits 12 to 19 set, 21:23 = 5
    /// and bit 25 set.
    fn every_v34_capability() -> Info0 {
        Info0 {
            rate_2743: true,
            rate_2800: true,
            rate_3429: true,
            low_carrier_3000: true,
            high_carrier_3000: true,
            low_carrier_3200: true,
            high_carrier_3200: true,
            transmit_3429: true,
            asymmetry: 5,
            constellation_1664: true,
            ..Info0::default()
        }
    }

    /// Table 15/V.92: bit 26 asks for short phase 2 and bit 27 says V.92 --
    /// that way round in INFO0d and only in INFO0d.
    ///
    /// Against the 62-bit vector of `spec-phase2-signals.md` 15 (CRC 0xDB49,
    /// bit 20 clear and bit 38 set) and the one of
    /// `spec-phase2-procedures.md` 3.4 (CRC 0xA8A9, bit 20 set and bit 38
    /// clear), both computed from Figure 14/V.34 by hand.
    #[test]
    fn a_v92_info0d_says_so_in_bit_27_and_asks_for_short_phase_2_in_bit_26() {
        let both = PcmFlags { v92: true, short_phase2: true };
        let mut p2s = Info0d {
            v34: every_v34_capability(),
            nominal_power: 3,
            max_power: 23,
            power_at_codec: true,
            a_law: false,
            upstream_3429: true,
        };
        p2s.set_pcm_flags(both);
        let bits = p2s.to_bits();
        assert_eq!(bits.len(), INFO0D_BITS);
        assert_eq!(sent_crc(&bits), 0xDB49);
        assert!(bits[26], "bit 26 asks for short phase 2");
        assert!(bits[27], "bit 27 says V.92");
        assert_eq!(Info0d::from_bits(&bits), Some(p2s));
        assert_eq!(Info0d::from_bits(&bits).unwrap().pcm_flags(), both);

        let mut p2p = Info0d { power_at_codec: false, ..p2s };
        p2p.v34.can_reduce_power = true;
        p2p.set_pcm_flags(both);
        assert_eq!(sent_crc(&p2p.to_bits()), 0xA8A9);

        // One without the other, and each in its own bit.
        let mut capable = p2s;
        capable.set_pcm_flags(PcmFlags { v92: true, short_phase2: false });
        let bits = capable.to_bits();
        assert!(!bits[26] && bits[27]);
        assert_eq!(capable.pcm_flags(), PcmFlags { v92: true, short_phase2: false });
        let mut asking = p2s;
        asking.set_pcm_flags(PcmFlags { v92: false, short_phase2: true });
        let bits = asking.to_bits();
        assert!(bits[26] && !bits[27]);
    }

    /// Table 16/V.92: INFO0a puts the same two bits the other way round, bit
    /// 26 for V.92 and bit 27 for the short phase 2 request.
    ///
    /// Against the 49-bit vectors of `spec-phase2-signals.md` 15: CRC 0xAF5A,
    /// and 0x2B52 once bit 28 acknowledges the far INFO0d.
    #[test]
    fn a_v92_info0a_has_the_two_bits_the_other_way_round() {
        let both = PcmFlags { v92: true, short_phase2: true };
        let mut info0a = Info0 { can_reduce_power: true, ..every_v34_capability() };
        info0a.set_pcm_flags(both);
        let bits = info0a.to_bits();
        assert_eq!(bits.len(), INFO0_BITS);
        assert_eq!(sent_crc(&bits), 0xAF5A);
        assert!(bits[26], "bit 26 says V.92");
        assert!(bits[27], "bit 27 asks for short phase 2");
        assert_eq!(Info0::from_bits(&bits).unwrap().pcm_flags(), both);

        let acknowledging = Info0 { acknowledge: true, ..info0a };
        assert_eq!(sent_crc(&acknowledging.to_bits()), 0x2B52);

        // The two orders are genuinely opposite: the same pair of flags gives
        // different bits in the two layouts, and one layout's pair read as the
        // other's would be its own mirror image.
        let mut info0d = Info0d::default();
        info0d.set_pcm_flags(PcmFlags { v92: true, short_phase2: false });
        let mut only_v92 = Info0::default();
        only_v92.set_pcm_flags(PcmFlags { v92: true, short_phase2: false });
        let (d, a) = (info0d.to_bits(), only_v92.to_bits());
        assert!(d[27] && !d[26]);
        assert!(a[26] && !a[27]);
        assert_eq!((d[26], d[27]), (a[27], a[26]), "one layout's pair is the other's, swapped");
        // Which is why INFO0d's pair is kept out of the two bits INFO0a reads:
        // there is no reading of `clock` that is right for both.
        assert_eq!(info0d.v34.pcm_flags(), PcmFlags::default(), "INFO0d's pair is not V.34's");
    }

    /// V.90's Table 7 reserves bits 26:27 of INFO0d and "set[s] to 0" both, so
    /// that sequence carries no transmit clock -- while V.92's Table 15 reads
    /// bit 27 as "V.92 capability: 1", which by 9.3 takes both ends to the
    /// Table 17 INFO1d.
    ///
    /// So an `Info0` that names V.34's external transmit clock, reused for an
    /// INFO0d as phase 2 reuses it, must not thereby claim V.92.
    #[test]
    fn an_info0d_never_claims_v92_because_of_a_v34_transmit_clock() {
        for clock in 0..4u8 {
            let v90 = Info0d {
                v34: Info0 { clock, ..every_v34_capability() },
                nominal_power: 4,
                max_power: 23,
                power_at_codec: true,
                a_law: false,
                upstream_3429: false,
            };
            let bits = v90.to_bits();
            assert!(!bits[26] && !bits[27], "clock {clock} reached bits 26:27");
            assert_eq!(v90.pcm_flags(), PcmFlags::default(), "clock {clock} read as V.92");
            // Every V.90 INFO0d is the same sixty-two bits whatever clock it
            // was built around, which is the encoding V.90 tests are pinned to.
            assert_eq!(bits, Info0d { v34: Info0 { clock: 0, ..v90.v34 }, ..v90 }.to_bits());

            // And the V.92 flags still go out, above whatever clock is beneath
            // them, without disturbing it.
            let mut v92 = v90;
            v92.set_pcm_flags(PcmFlags { v92: true, short_phase2: false });
            let bits = v92.to_bits();
            assert!(!bits[26] && bits[27], "clock {clock} swallowed the V.92 bit");
            assert_eq!(v92.v34.clock & 3, clock, "the V.34 clock was overwritten");
        }
    }

    /// V.90 reserves bits 26 and 27 of INFO0d and "set[s] to 0" both, so a
    /// modem that never asks for V.92 sends exactly what it sent before.
    ///
    /// The vector is the digital modem this repository's test server sends
    /// (`v90/server.rs`), CRC 0x01C5, computed from Figure 14/V.34 by hand.
    #[test]
    fn a_v90_info0d_still_sends_zeros_in_bits_26_and_27() {
        let v90 = Info0d {
            v34: Info0 { constellation_1664: true, ..Info0::default() },
            nominal_power: 4,
            max_power: 23,
            power_at_codec: true,
            a_law: false,
            upstream_3429: false,
        };
        assert_eq!(v90.pcm_flags(), PcmFlags::default());
        let bits = v90.to_bits();
        assert_eq!(bits.len(), INFO0D_BITS);
        assert_eq!(sent_crc(&bits), 0x01C5);
        assert!(!bits[26] && !bits[27], "V.90 reserves both");
        assert_eq!(Info0d::from_bits(&bits), Some(v90));
        // And so does every other V.90 INFO0d, whatever else it carries.
        for nominal in 0..16 {
            for max_power in [0, 15, 31] {
                let other = Info0d { nominal_power: nominal, max_power, ..v90 };
                let bits = other.to_bits();
                assert!(!bits[26] && !bits[27], "{nominal} {max_power}");
            }
        }
    }

    /// A V.90 modem reads a V.92 modem's INFO0 as it always did: the CRC
    /// checks, every field it knows is where it was, and the two new bits are
    /// ones it never looks at.
    #[test]
    fn a_v90_peer_reads_a_v92_info0_as_v90() {
        let mut v92 = every_v34_capability();
        v92.set_pcm_flags(PcmFlags { v92: true, short_phase2: true });
        let read = Info0::from_bits(&v92.to_bits()).expect("the CRC checks");
        assert_eq!(Info0 { clock: 0, ..read }, every_v34_capability());

        let mut digital = Info0d {
            v34: every_v34_capability(),
            nominal_power: 3,
            max_power: 23,
            power_at_codec: true,
            a_law: true,
            upstream_3429: true,
        };
        digital.set_pcm_flags(PcmFlags { v92: true, short_phase2: true });
        let read = Info0d::from_bits(&digital.to_bits()).expect("the CRC checks");
        assert_eq!(read.nominal_dbm0(), -9.0);
        assert_eq!(read.max_dbm0(), -12.0);
        assert!(read.a_law && read.upstream_3429 && read.power_at_codec);
        assert_eq!(Info0 { clock: 0, ..read.v34 }, every_v34_capability());
    }

    /// Table 17/V.92: bit 70, the 3429 carrier flag in V.90, now says whether
    /// the channel will carry PCM upstream -- and the eight bits after it have
    /// not moved.
    ///
    /// Against the 109-bit vector of `spec-phase2-signals.md` 15, CRC 0x086C.
    #[test]
    fn info1d_bit_70_reads_as_pcm_upstream_and_the_3429_field_keeps_its_place() {
        let mut info1d = Info1c {
            min_power_reduction: 2,
            additional_power_reduction: 1,
            md_length: 0,
            probed: [Probed::default(); 6],
            frequency_offset: Some(-0.06),
        };
        for (slot, (carrier, pre, rate)) in info1d.probed.iter_mut().zip([
            (true, 3, 10),
            (false, 2, 11),
            (false, 2, 11),
            (true, 4, 13),
            (true, 5, 14),
            (true, 6, 13),
        ]) {
            *slot = Probed { high_carrier: carrier, pre_emphasis: pre, max_rate: rate };
        }
        let bits = info1d.to_bits();
        assert_eq!(bits.len(), INFO1C_BITS);
        assert_eq!(sent_crc(&bits), 0x086C);
        assert!(bits[70], "bit 70 says the channel carries PCM upstream");
        assert_eq!(get(&bits, 71, 4), 6, "bits 71:74 are still the pre-emphasis");
        assert_eq!(get(&bits, 75, 4), 13, "bits 75:78 are still the projected rate");

        let read = Info1c::from_bits(&bits).expect("the CRC checks");
        assert!(read.pcm_upstream());
        assert_eq!(read.probed_3429(), Probed3429 { pre_emphasis: 6, max_rate: 13 });
        // The bit is written on purpose, and clearing it moves nothing else.
        let mut refusing = read;
        refusing.set_pcm_upstream(false);
        assert!(!refusing.pcm_upstream());
        assert!(!refusing.to_bits()[70]);
        assert_eq!(refusing.probed_3429(), read.probed_3429());
        assert_eq!(refusing.probed[..5], read.probed[..5]);
    }

    /// Table 18/V.92: the INFO1a that asks for PCM upstream, which today's
    /// V.90 parser throws away because bits 34:36 hold six.
    ///
    /// Against the two 70-bit vectors of `spec-phase2-signals.md` 15 (CRC
    /// 0xA858) and `spec-phase2-procedures.md` 3.4 (CRC 0xF59A).
    #[test]
    fn a_table_18_info1a_round_trips_and_is_not_thrown_away() {
        let asked =
            Info1aPcmUp { sections: 3, ltot_code: 1, lmax_code: 0, md_length: 4, uinfo: 90 };
        let bits = asked.to_bits();
        assert_eq!(bits.len(), INFO1A_BITS);
        assert_eq!(sent_crc(&bits), 0xA858);
        assert_eq!(get(&bits, 34, 3), PCM_SYMBOL_RATE, "8000 from the analogue modem");
        assert_eq!(get(&bits, 37, 3), PCM_SYMBOL_RATE, "8000 from the digital modem");
        assert_eq!(get(&bits, 32, 2), 0, "bits 32:33 are set to 0");
        assert_eq!(get(&bits, 40, 10), 0x3ff, "bits 40:49 are set to 1");
        assert_eq!(Info1aPcmUp::from_bits(&bits), Some(asked));

        let other =
            Info1aPcmUp { sections: 3, ltot_code: 3, lmax_code: 3, md_length: 0, uinfo: 77 };
        assert_eq!(sent_crc(&other.to_bits()), 0xF59A);

        // Every filter code, and the ends of both seven-bit fields.
        for sections in 0..4 {
            for ltot_code in 0..4 {
                for lmax_code in 0..4 {
                    for (md_length, uinfo) in [(0, UINFO_LOWEST), (127, UINFO_HIGHEST)] {
                        let asked =
                            Info1aPcmUp { sections, ltot_code, lmax_code, md_length, uinfo };
                        assert_eq!(Info1aPcmUp::from_bits(&asked.to_bits()), Some(asked));
                    }
                }
            }
        }

    }

    /// Table 18/V.92: "U_INFO shall be greater than 66", and 8.4.4/V.90 trains
    /// Sd on the codeword whose Ucode is 16 + U_INFO, of which there are 128.
    ///
    /// The end that *chooses* the value is held to both ends of that range,
    /// because a sequence naming a codeword outside it goes out asking for
    /// something the Recommendation does not allow. The end that receives one
    /// is not: a number we could merely not use is no reason to lose the call.
    #[test]
    fn the_u_info_we_choose_is_one_table_18_allows() {
        assert_eq!(UINFO_LOWEST, 67);
        assert_eq!(UINFO_HIGHEST, 111);
        assert!(u32::from(UINFO_HIGHEST) + 16 < 128, "Sd would train past Ucode 127");
        for uinfo in 0..=127u8 {
            let asked = Info1aPcmUp { uinfo, ..Info1aPcmUp::default() };
            assert_eq!(asked.uinfo_is_sendable(), (67..=111).contains(&uinfo), "U_INFO {uinfo}");
        }
        // A sequence built up from the default is sendable before anyone has
        // chosen anything, which a zero -- the one value Table 18 names as
        // forbidden -- would not have been.
        assert_eq!(Info1aPcmUp::default().uinfo, UINFO_LOWEST);
        assert!(Info1aPcmUp::default().uinfo_is_sendable());

        // A Table 18 frame with any U_INFO in it, built without `to_bits`,
        // which is the side the rule binds.
        let raw = |uinfo: u32| {
            let mut info = Vec::with_capacity(38);
            put(&mut info, 0, 13);
            put(&mut info, uinfo, 7);
            put(&mut info, 0, 2);
            put(&mut info, PCM_SYMBOL_RATE, 3);
            put(&mut info, PCM_SYMBOL_RATE, 3);
            put(&mut info, 0x3ff, 10);
            frame(&info)
        };
        for uinfo in [0, 66, 112, 127] {
            let heard = Info1aPcmUp::from_bits(&raw(uinfo)).expect("the CRC checks");
            assert_eq!(heard.uinfo, uinfo as u8, "a far end's U_INFO was refused");
            assert!(!heard.uinfo_is_sendable(), "and it is not one we would send");
        }
    }

    /// Table 19/V.92: V.90's Table 10 with bit 33, reserved there, saying
    /// which carrier the analogue modem will transmit on -- because short
    /// phase 2 sends no INFO1d for it to be read out of.
    ///
    /// Against the two 70-bit vectors of `spec-phase2-signals.md` 15, CRC
    /// 0x6742 and 0x7A52.
    #[test]
    fn a_table_19_info1a_carries_the_high_carrier_in_bit_33() {
        let high = Info1aV34Up {
            v90: Info1aPcm {
                md_length: 0,
                uinfo: 90,
                upstream: SymbolRate::S3200,
                frequency_offset: None,
            },
            high_carrier: true,
        };
        let bits = high.to_bits();
        assert_eq!(bits.len(), INFO1A_BITS);
        assert_eq!(sent_crc(&bits), 0x6742);
        assert!(bits[33], "bit 33 asks for the high carrier");
        assert!(!bits[32], "bit 32 is still reserved");
        assert_eq!(Info1aV34Up::from_bits(&bits), Some(high));

        let other = Info1aV34Up {
            v90: Info1aPcm { uinfo: 77, upstream: SymbolRate::S3429, ..high.v90 },
            high_carrier: true,
        };
        assert_eq!(sent_crc(&other.to_bits()), 0x7A52);

        // Clear bit 33 and the seventy bits are V.90's Table 10 exactly, which
        // is why neither layout can be told from the other by its bits, and
        // why the phase and not the frame decides which reading applies.
        let low = Info1aV34Up { high_carrier: false, ..high };
        assert_eq!(low.to_bits(), high.v90.to_bits());
        assert_eq!(Info1aPcm::from_bits(&high.to_bits()), Some(high.v90));
        assert_eq!(Info1aV34Up::from_bits(&high.v90.to_bits()), Some(low));
    }

    /// 10.4 of `spec-phase2-signals.md`: bits 37:39 and 34:36 between them
    /// name the layout, and a pair that names none of them is dropped rather
    /// than guessed at.
    #[test]
    fn the_receiver_tells_the_info1a_layouts_apart_by_bits_34_to_39() {
        // A 70-bit INFO1a with whatever is asked for in 34:36 and 37:39, and
        // everything else zero.
        let raw = |lower: u32, upper: u32| {
            let mut info = Vec::with_capacity(38);
            put(&mut info, 0, 22);
            put(&mut info, lower, 3);
            put(&mut info, upper, 3);
            put(&mut info, 0, 10);
            frame(&info)
        };
        let reads = |bits: &[bool]| {
            (
                Info1a::from_bits(bits).is_some(),
                Info1aPcm::from_bits(bits).is_some(),
                Info1aPcmUp::from_bits(bits).is_some(),
            )
        };
        // V.90's Table 11, which is V.34's own: both fields a symbol rate.
        for upper in 0..6 {
            for lower in 0..6 {
                assert_eq!(reads(&raw(lower, upper)), (true, false, false), "{lower} {upper}");
            }
        }
        // V.90's Table 10 and V.92's Table 19: 8000 down, one of V.34's three
        // fastest up.
        for lower in 3..6 {
            assert_eq!(reads(&raw(lower, PCM_SYMBOL_RATE)), (false, true, false), "{lower}");
        }
        // V.92's Table 18: 8000 both ways.
        assert_eq!(reads(&raw(PCM_SYMBOL_RATE, PCM_SYMBOL_RATE)), (false, false, true));
        // Six above with 0, 1, 2 or 7 below is no layout at all, and seven
        // above is no layout whatever is below it.
        for lower in [0, 1, 2, 7] {
            assert_eq!(reads(&raw(lower, PCM_SYMBOL_RATE)), (false, false, false), "{lower}");
        }
        for lower in 0..8 {
            assert_eq!(reads(&raw(lower, 7)), (false, false, false), "{lower}");
        }
    }

    /// Table 18's bits 40:49 are ten reserved ones, which a frequency-offset
    /// parser would read as a perfectly plausible -0.02 Hz.
    #[test]
    fn a_table_18_frame_is_never_read_as_a_frequency_offset() {
        let asked = Info1aPcmUp { sections: 0, ltot_code: 0, lmax_code: 0, md_length: 0, uinfo: 67 };
        let bits = asked.to_bits();
        assert_eq!(get(&bits, 40, 10), 0x3ff);
        // What the trap looks like: those ten bits as a V.34 offset field.
        assert_eq!(offset_from(0x3ff), Some(-0.02));
        // And what actually happens to them: nothing. No layout that carries a
        // frequency offset will parse the frame at all.
        assert_eq!(Info1a::from_bits(&bits), None);
        assert_eq!(Info1aPcm::from_bits(&bits), None);
        assert_eq!(Info1aV34Up::from_bits(&bits), None);
        assert_eq!(Info1aPcmUp::from_bits(&bits), Some(asked));
    }

    /// Tables 32 and 33/V.92, against the ten worked vectors of
    /// `spec-modem-on-hold.md` 1.2.5 and its sixteen MHack CRCs, all computed
    /// from Figure 14/V.34 by hand.
    #[test]
    fn every_mh_sequence_matches_its_worked_vector() {
        let vectors = [
            (Mh::req(), 0x03E7u16),
            (Mh::ack(T1::from_code(1)), 0x24D5),
            (Mh::ack(T1::from_code(5)), 0x05D7),
            (Mh::ack(T1::NoLimit), 0x1556),
            (Mh::nack(), 0x01F7),
            (Mh::clrd(Cleardown::IncomingCall), 0x374C),
            (Mh::clrd(Cleardown::OutgoingCall), 0xF140),
            (Mh::clrd(Cleardown::Other), 0xC0C3),
            (Mh::cda(), 0x02EF),
            (Mh::frr(), 0x04DF),
        ];
        for (mh, want) in vectors {
            let bits = mh.to_bits();
            assert_eq!(bits.len(), MH_BITS, "{mh:?}");
            assert_eq!(sent_crc(&bits), want, "{mh:?}");
            assert_eq!(&bits[..4], &FILL);
            assert_eq!(&bits[4..12], &SYNC);
            assert_eq!(&bits[36..], &FILL);
            // The receiver's cheap check: the CRC run on after its own bits
            // leaves the register at nothing.
            assert_eq!(crc(&bits[12..MH_BITS - FILL.len()]), 0, "{mh:?}");
            assert_eq!(Mh::from_bits(&bits), Some(mh), "{mh:?}");
        }

        // Every T1 code an MHack can carry, and the patterns leftmost first.
        let acks = [
            0xA0DDu16, 0x24D5, 0xE2D9, 0x66D1, 0x81DF, 0x05D7, 0xC3DB, 0x47D3, 0xB05C, 0x3454,
            0xF258, 0x7650, 0x915E, 0x1556, 0xD35A, 0x5752,
        ];
        for (code, want) in (0..16u8).zip(acks) {
            let mh = Mh::ack(T1::from_code(code));
            let bits = mh.to_bits();
            assert_eq!(sent_crc(&bits), want, "code {code:04b}");
            assert_eq!(get_pattern(&bits, 12), MhIndication::Ack.code());
            assert_eq!(get_pattern(&bits, 16), code, "bit 16 carries the code's MSB");
            assert_eq!(Mh::from_bits(&bits), Some(mh));
        }

        // Every defined indication ends in a one, and the four that repeat
        // their indication really do.
        for indication in MhIndication::ALL {
            assert_eq!(indication.code() & 1, 1, "{indication:?}");
            assert_eq!(MhIndication::from_code(indication.code()), Some(indication));
        }
        for mh in [Mh::req(), Mh::nack(), Mh::cda(), Mh::frr()] {
            assert_eq!(mh.information, MhInformation::Repeat(mh.indication.code()));
        }
    }

    /// Table 32 NOTE 1: "MH sequences with undefined bit combinations should
    /// be ignored". NOTE 2 is the softer one -- an unlisted MHclrd reason is
    /// still a good MHclrd, and still wants its MHcda.
    #[test]
    fn an_undefined_mh_indication_is_ignored_and_a_reserved_cleardown_reason_is_kept() {
        let with = |indication: u8, information: u8| {
            let mut info = Vec::with_capacity(8);
            put_pattern(&mut info, indication);
            put_pattern(&mut info, information);
            frame(&info)
        };
        for indication in 0..16u8 {
            let heard = Mh::from_bits(&with(indication, indication));
            match MhIndication::from_code(indication) {
                Some(defined) => assert_eq!(heard.map(|mh| mh.indication), Some(defined)),
                None => assert_eq!(heard, None, "indication {indication:04b} is reserved"),
            }
        }
        for reason in 0..16u8 {
            let heard = Mh::from_bits(&with(MhIndication::Clrd.code(), reason))
                .expect("MHclrd is a defined indication");
            assert_eq!(heard.indication, MhIndication::Clrd);
            let kept = Cleardown::from_code(reason);
            assert_eq!(heard.information, MhInformation::Reason(kept));
            assert_eq!(kept.code(), reason, "the reason goes back out as it came in");
            match reason {
                0b0101 => assert_eq!(kept, Cleardown::IncomingCall),
                0b0110 => assert_eq!(kept, Cleardown::OutgoingCall),
                0b1010 => assert_eq!(kept, Cleardown::Other),
                other => assert_eq!(kept, Cleardown::Unknown(other)),
            }
        }
    }

    /// Table 33 leaves `0000`, `1110` and `1111` "Reserved for the ITU" and
    /// says nothing about what a receiver should make of one.
    ///
    /// Whatever that turns out to be, it must not be a hold: a reserved code
    /// decodes as itself and never as a period, and never as "no limit"
    /// either, so nothing downstream can take one for a grant.
    #[test]
    fn a_reserved_t1_code_in_an_mhack_is_not_taken_as_a_grant() {
        let printed = [
            (1, 10),
            (2, 20),
            (3, 30),
            (4, 40),
            (5, 60),
            (6, 120),
            (7, 180),
            (8, 240),
            (9, 360),
            (10, 480),
            (11, 720),
            (12, 960),
        ];
        for (code, seconds) in printed {
            let t1 = T1::from_code(code);
            assert_eq!(t1, T1::Limit(seconds), "code {code:04b}");
            assert_eq!(t1.seconds(), Some(seconds));
            assert_eq!(t1.code(), code);
        }
        assert_eq!(T1::from_code(13), T1::NoLimit);
        assert_eq!(T1::NoLimit.seconds(), None);
        assert_eq!(T1::NoLimit.code(), 13);

        for code in [0b0000, 0b1110, 0b1111] {
            let t1 = T1::from_code(code);
            assert_eq!(t1, T1::Reserved(code), "code {code:04b}");
            assert_eq!(t1.seconds(), None, "a reserved code is not a period");
            assert_ne!(t1, T1::NoLimit, "nor is it an unbounded hold");
            assert_eq!(t1.code(), code, "and it goes back out as it came in");
            // Through the wire as well as through the enum.
            let heard = Mh::from_bits(&Mh::ack(t1).to_bits()).expect("a good MHack");
            assert_eq!(heard.information, MhInformation::Timeout(T1::Reserved(code)));
        }
        // Thirteen codes name a period and three do not; no code does both.
        let granted = (0..16u8).filter(|&c| T1::from_code(c) != T1::Reserved(c)).count();
        assert_eq!(granted, 13);
    }

    /// Table 33 codes twelve periods and nothing between them, so a timeout
    /// that is not one of the twelve has no pattern of its own.
    ///
    /// What goes out is the longest tabled period that does not exceed the one
    /// asked for -- five minutes is granted as four -- so a grant is never
    /// longer than intended and never falls back to `0000`, which is reserved
    /// and, by section 4's reading, no grant at all. `seconds` reports what the
    /// wire will carry, not what was asked for, so the granting end and the
    /// held end agree on how long the hold may run.
    #[test]
    fn a_timeout_table_33_cannot_name_is_granted_short_rather_than_reserved() {
        for (asked, sent, granted) in [
            (10, 1, Some(10)),
            (59, 4, Some(40)),
            (300, 8, Some(240)),
            (1000, 12, Some(960)),
            // Shorter than anything Table 33 names: there is no period to send.
            (9, 0, None),
            (0, 0, None),
        ] {
            let t1 = T1::Limit(asked);
            assert_eq!(t1.code(), sent, "{asked} s went out as {:04b}", t1.code());
            assert_eq!(t1.seconds(), granted, "{asked} s");
            // And what the far end will make of it, which is what settles it.
            let heard = Mh::from_bits(&Mh::ack(t1).to_bits()).expect("a good MHack");
            assert_eq!(heard.information, MhInformation::Timeout(T1::from_code(sent)), "{asked} s");
        }
        // Every pattern the wire can carry goes back out as itself, so nothing
        // is turned into something else on the way through.
        for code in 0..16u8 {
            assert_eq!(T1::from_code(code).code(), code, "code {code:04b}");
        }
    }
}
