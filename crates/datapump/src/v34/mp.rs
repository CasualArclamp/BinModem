//! Modulation parameter sequences: MP (10.1.3.9, Tables 20 and 21), and its
//! half-duplex form MPh (10.2.4.4, Tables 23 and 24).
//!
//! What each modem will accept for data mode, sent back and forth in phase 4:
//! the fastest rate each way, the trellis code, the shaping and non-linear
//! encoding the far transmitter is to use, which rates are enabled at all, and
//! -- in a Type 1 sequence -- the three precoding coefficients the far
//! transmitter's precoder is to use. MP' is the same with the acknowledge bit
//! set, saying the far end's own has arrived.
//!
//! MPh is the same frame on half-duplex's control channel, exchanged at each
//! control channel start-up (12.4): the same sync, start bits, CRC and fill,
//! and most of the fields where MP has them. It says less, since data goes
//! only one way -- one fastest rate, and no auxiliary channel -- and one thing
//! more, the control channel's own rate. It has no acknowledge bit and so no
//! MPh': an end sends E once it has heard one of the other's (12.4.1.3,
//! 12.4.2.4).

use super::info::crc;
use super::signals::E_BITS;

/// Bits of a Type 0 sequence and of a Type 1, MP or MPh (Tables 20 and 21, 23
/// and 24).
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

/// Bits 0:19, which MP and MPh lay out alike: the sync, a start bit, the type
/// and a reserved bit.
fn head(type1: bool) -> Vec<bool> {
    let mut bits = vec![true; SYNC_ONES];
    bits.push(false); // 17: start
    bits.push(type1); // 18: type
    bits.push(false); // 19: reserved
    bits
}

/// Bits 34 to the end, which MP and MPh lay out alike too: a start bit, the
/// rate mask, bit 50 (asymmetric rates -- of the data in MP, of the control
/// channel in MPh), a start bit, then the precoding coefficients or sixteen
/// reserved bits with their start bits, the CRC and the fill.
fn tail(mut bits: Vec<bool>, rates: u16, bit_50: bool, precoding: Option<[Coefficient; 3]>) -> Vec<bool> {
    debug_assert_eq!(bits.len(), 34);
    let length = if precoding.is_some() { TYPE1_BITS } else { TYPE0_BITS };
    bits.push(false); // 34: start
    put(&mut bits, u32::from(rates & 0x3fff), 15); // 35:49, 49 reserved
    bits.push(bit_50);
    bits.push(false); // 51: start
    match precoding {
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

/// Whether `bits`, which start at a frame sync, are a whole sequence of either
/// kind -- its start bits where they should be, and its CRC checking -- and if
/// so, whether it is Type 1.
///
/// The fill after the CRC need not be there.
fn check(bits: &[bool]) -> Option<bool> {
    if bits.len() < SYNC_ONES + 2 || bits[..SYNC_ONES].iter().any(|b| !b) || bits[SYNC_ONES] {
        return None;
    }
    let type1 = bits[18];
    let length = if type1 { TYPE1_BITS } else { TYPE0_BITS };
    let at = crc_at(length);
    if bits.len() < at + 16 || start_bits(length).iter().any(|&i| bits[i]) {
        return None;
    }
    (crc(&covered(bits, length)) == get(bits, at, 16) as u16).then_some(type1)
}

/// h(1), h(2) and h(3) out of a Type 1 sequence.
fn coefficients(bits: &[bool]) -> [Coefficient; 3] {
    let coefficient = |from: usize| -> Coefficient { (get(bits, from, 16) as u16 as i16, get(bits, from + 17, 16) as u16 as i16) };
    [coefficient(52), coefficient(86), coefficient(120)]
}

/// Bits 29:30 for a trellis code: "0 = 16 State; 1 = 32 State; 2 = 64 State".
fn trellis_bits(trellis: Trellis) -> u32 {
    match trellis {
        Trellis::States16 => 0,
        Trellis::States32 => 1,
        Trellis::States64 => 2,
    }
}

/// The trellis code bits 29:30 ask for. Three is "Reserved for ITU-T" and
/// names no code; it is read as 16 states.
fn trellis_of(bits: u32) -> Trellis {
    match bits {
        1 => Trellis::States32,
        2 => Trellis::States64,
        _ => Trellis::States16,
    }
}

impl Mp {
    /// An MP' of this.
    pub fn acknowledged(mut self) -> Self {
        self.acknowledge = true;
        self
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let mut bits = head(self.precoding.is_some());
        put(&mut bits, u32::from(self.call_to_answer), 4);
        put(&mut bits, u32::from(self.answer_to_call), 4);
        bits.push(self.auxiliary);
        put(&mut bits, trellis_bits(self.trellis), 2);
        bits.push(self.non_linear);
        bits.push(self.expanded_shaping);
        bits.push(self.acknowledge);
        tail(bits, self.rates, self.asymmetric, self.precoding)
    }

    /// An MP out of `bits`, which start at its frame sync, if its start bits
    /// are where they should be and its CRC checks.
    ///
    /// The fill after the CRC need not be there.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let type1 = check(bits)?;
        Some(Self {
            call_to_answer: get(bits, 20, 4) as u8,
            answer_to_call: get(bits, 24, 4) as u8,
            auxiliary: bits[28],
            trellis: trellis_of(get(bits, 29, 2)),
            non_linear: bits[31],
            expanded_shaping: bits[32],
            acknowledge: bits[33],
            rates: get(bits, 35, 14) as u16,
            asymmetric: bits[50],
            precoding: type1.then(|| coefficients(bits)),
        })
    }

    /// The rate mask with every rate from 2400 up to `highest` times 2400.
    pub fn rates_up_to(highest: u8) -> u16 {
        (1u16 << highest.min(14)) - 1
    }
}

/// The control channel's two data signalling rates, at 600 symbols a second
/// either way (10.2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ControlRate {
    /// 1200 bit/s, with Q1 and Q2 zero: what every training and
    /// synchronisation signal of the control channel goes at, MPh and E
    /// among them.
    #[default]
    Bps1200,
    Bps2400,
}

impl ControlRate {
    /// "For a data rate of 1200 bit/s, 2 bits are transmitted every symbol
    /// interval. For a data rate of 2400 bit/s, 4 bits".
    pub fn bits_per_symbol(self) -> usize {
        match self {
            Self::Bps1200 => 2,
            Self::Bps2400 => 4,
        }
    }
}

/// A half-duplex modulation parameter sequence, MPh (10.2.4.4, Tables 23 and
/// 24): what one end will take the primary channel up to, and the rate it
/// wants the control channel sent to it at.
///
/// Both ends send one, but only the source transmits on the primary channel,
/// so the trellis code, non-linear encoding, shaping and precoding that count
/// are the ones the recipient asks for -- the encoder "shall be selected by
/// the receiving modem" (9.6.3.2). "Source modem does not use bits 29-32, and
/// should set these bits to 0" (NOTE 2), and has no precoder to ask for, so it
/// sends Type 0. Before the first MPh of a start-up the precoding coefficients
/// are zero, and a Type 0 leaves them as they were (10.2.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mph {
    /// Bits 20:23: the fastest the primary channel may go, as a multiple of
    /// 2400 from 1 to 14. Above 12 only if the far end's INFO0 says it handles
    /// 1664-point constellations (NOTE 1).
    pub max_rate: u8,
    /// Bit 27: the rate the far end's transmitter is to send control channel
    /// data at -- so the rate this end receives at, if both ends allow the two
    /// directions to differ (`control_rates`).
    pub control_rate: ControlRate,
    /// Bits 29:30: the trellis code the source is to use.
    pub trellis: Trellis,
    /// Bit 31: theta of 0.3125 for the source's non-linear encoder rather than
    /// 0.
    pub non_linear: bool,
    /// Bit 32: expanded rather than minimum constellation shaping (Table 10).
    pub expanded_shaping: bool,
    /// Bits 35:48, one for each rate from 2400 to 33 600, least first: the
    /// rates "supported and enabled in both transmitter and receiver" of this
    /// end.
    pub rates: u16,
    /// Bit 50: the two directions of the control channel may run at different
    /// rates.
    pub asymmetric_control: bool,
    /// Type 1 only: h(1), h(2) and h(3), each real then imaginary, for the
    /// source's precoder.
    pub precoding: Option<[Coefficient; 3]>,
}

impl Mph {
    pub fn to_bits(&self) -> Vec<bool> {
        let mut bits = head(self.precoding.is_some());
        put(&mut bits, u32::from(self.max_rate), 4); // 20:23
        put(&mut bits, 0, 3); // 24:26 reserved
        bits.push(self.control_rate == ControlRate::Bps2400); // 27
        // 28 reserved, where MP says whether there is an auxiliary channel:
        // half-duplex has none.
        bits.push(false);
        put(&mut bits, trellis_bits(self.trellis), 2); // 29:30
        bits.push(self.non_linear); // 31
        bits.push(self.expanded_shaping); // 32
        bits.push(false); // 33 reserved, where MP has its acknowledge bit
        tail(bits, self.rates, self.asymmetric_control, self.precoding)
    }

    /// An MPh out of `bits`, which start at its frame sync, if its start bits
    /// are where they should be and its CRC checks.
    ///
    /// The reserved bits -- 19, 24:26, 28, 33, 49, and 52:67 in Type 0 or
    /// 154:169 in Type 1 -- are "not interpreted by the receiving modem": the
    /// CRC is over them, and nothing else reads them. The fill after the CRC
    /// need not be there.
    pub fn from_bits(bits: &[bool]) -> Option<Self> {
        let type1 = check(bits)?;
        Some(Self {
            max_rate: get(bits, 20, 4) as u8,
            control_rate: if bits[27] { ControlRate::Bps2400 } else { ControlRate::Bps1200 },
            trellis: trellis_of(get(bits, 29, 2)),
            non_linear: bits[31],
            expanded_shaping: bits[32],
            rates: get(bits, 35, 14) as u16,
            asymmetric_control: bits[50],
            precoding: type1.then(|| coefficients(bits)),
        })
    }
}

/// The primary channel's rate, as a multiple of 2400, once each end has the
/// other's MPh: "the maximum rate enabled that is less than or equal to the
/// data signalling rates specified in both modems' MPh sequences" -- the
/// source's transmit rate by 12.4.1.3 and the recipient's receive rate by
/// 12.4.2.4, the same rule and so the same number at both ends.
///
/// Enabled is enabled in both masks, since each says what its own end's
/// transmitter and receiver will do. If no rate is enabled in both at or below
/// both maxima -- or a maximum is 0 -- there is none, a case the text does not
/// provide for. A maximum of 15, which the field can carry and Table 23 cannot,
/// counts as 14.
pub fn primary_rate(source: &Mph, recipient: &Mph) -> Option<u8> {
    let enabled = source.rates & recipient.rates;
    let limit = source.max_rate.min(recipient.max_rate).min(14);
    (1..=limit).rev().find(|r| enabled >> (r - 1) & 1 == 1)
}

/// The control channel's rates once each end has the other's MPh: what this
/// end transmits at, then what it receives at.
///
/// Each end asks in bit 27 for the rate the other is to transmit at, and
/// transmits at what the other asked of it (12.4.1.4, 12.4.2.5) -- but only
/// when both set bit 50: "Asymmetric mode shall be used only when both modems
/// set bit 50 to 1. If different data rates are selected in symmetric mode,
/// both modems shall transmit at the lower rate" (Tables 23 and 24).
pub fn control_rates(ours: &Mph, far: &Mph) -> (ControlRate, ControlRate) {
    if ours.asymmetric_control && far.asymmetric_control {
        (far.control_rate, ours.control_rate)
    } else {
        let lower = ours.control_rate.min(far.control_rate);
        (lower, lower)
    }
}

/// What both finders do: keep the last sequence's worth of descrambled bits,
/// and at each bit look for E, or for a sequence whose CRC has just come in.
#[derive(Debug, Clone, Default)]
struct Search {
    bits: Vec<bool>,
    ones: usize,
}

/// What a search turned up.
enum Hit<T> {
    Sequence(T),
    E,
}

impl Search {
    /// One more bit, with `read` to make a sequence of bits that start at its
    /// frame sync. With `exact_sync`, seventeen ones with another in front of
    /// them are not a sync.
    fn feed<T>(&mut self, bit: bool, exact_sync: bool, read: fn(&[bool]) -> Option<T>) -> Option<Hit<T>> {
        self.ones = if bit { self.ones + 1 } else { 0 };
        self.bits.push(bit);
        // A sequence is sync, a start bit and the type; a sequence is only
        // looked at once all of it could be here.
        if self.bits.len() > TYPE1_BITS + SYNC_ONES + 2 {
            self.bits.remove(0);
        }
        if self.ones == E_BITS {
            self.bits.clear();
            return Some(Hit::E);
        }
        for length in [TYPE0_BITS, TYPE1_BITS] {
            // Checked when the CRC is in; the fill after it is not waited for.
            let without_fill = crc_at(length) + 16;
            if self.bits.len() < without_fill {
                continue;
            }
            let start = self.bits.len() - without_fill;
            let candidate = &self.bits[start..];
            if exact_sync && start > 0 && self.bits[start - 1] {
                continue;
            }
            // The type bit has to match the length: a Type 0 read off the
            // front of a Type 1's window ended long ago.
            if (length == TYPE1_BITS) == candidate[18]
                && let Some(found) = read(candidate)
            {
                self.bits.clear();
                self.ones = 0;
                return Some(Hit::Sequence(found));
            }
        }
        None
    }
}

/// Finds MP sequences, and E, in a stream of descrambled bits.
#[derive(Debug, Clone, Default)]
pub struct Finder {
    search: Search,
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
        // The sync is 17 ones exactly: an eighteenth in front would be part of
        // something else.
        Some(match self.search.feed(bit, true, Mp::from_bits)? {
            Hit::Sequence(mp) => Found::Mp(mp),
            Hit::E => Found::E,
        })
    }
}

/// Finds MPh sequences, and E, in the control channel's descrambled bits
/// (10.2.4.3, 10.2.4.4).
///
/// `Finder`'s search, except that a sync may have a one in front of it. The
/// first MPh of a start-up follows ALT straight on (12.4.1.2, 12.4.2.3), and
/// ALT is zeros and ones by turns, two to a symbol (10.2.4.2): it ends on a
/// one whenever it starts on a zero, which the text leaves open, and then the
/// first MPh's sync is eighteen ones long. That first MPh may be the only one
/// -- an end that has heard the other's before it starts sending its own
/// finishes the one it is on and sends E (12.4.1.3, 12.4.2.4) -- so it must
/// not be missed.
///
/// E is what it was, twenty ones: more than a sync runs to even with ALT's
/// one in front, and never inside ALT, which descrambles to bits that
/// alternate. After an MPh it follows fill zeros and is found on its last
/// bit. Straight after ALT, as in a resynchronisation (12.6.1.4, 12.6.2.2), a
/// last ALT bit of one makes the run twenty-one long, and E is found a bit
/// before its last. Twenty ones are E only where an E is due -- control
/// channel data can hold as many, T.30's forty ones among them (F.3.2.2) --
/// which is for the caller to know.
#[derive(Debug, Clone, Default)]
pub struct MphFinder {
    search: Search,
}

/// What an MPh finder found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MphFound {
    Mph(Mph),
    /// Twenty ones in a row, which "signal the beginning of control channel
    /// user data" (10.2.4.3).
    E,
}

impl MphFinder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bit: bool) -> Option<MphFound> {
        Some(match self.search.feed(bit, false, Mph::from_bits)? {
            Hit::Sequence(mph) => MphFound::Mph(mph),
            Hit::E => MphFound::E,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v32::{Mode, Scrambler};

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

    /// A recipient's MPh, Type 1 with its precoder's coefficients.
    fn recipient() -> Mph {
        Mph {
            max_rate: 13,
            control_rate: ControlRate::Bps2400,
            trellis: Trellis::States32,
            non_linear: true,
            expanded_shaping: false,
            // 2400, and 7200 to 31 200: neither 4800 nor 33 600.
            rates: 0x1ffd,
            asymmetric_control: true,
            precoding: Some([(0x2000, -0x1000), (-3, 12345), (-16384, 7)]),
        }
    }

    /// A source's: Type 0, with bits 29 to 32 zero (NOTE 2).
    fn source() -> Mph {
        Mph { max_rate: 14, control_rate: ControlRate::Bps1200, rates: Mp::rates_up_to(14), ..Mph::default() }
    }

    /// A table's fields one after another, each checked to start at the bit
    /// the table says.
    fn laid_out(table: &[(usize, &str)]) -> Vec<bool> {
        let mut bits = Vec::new();
        for &(at, field) in table {
            assert_eq!(bits.len(), at, "the field at bit {at}");
            bits.extend(field.chars().map(|c| c == '1'));
        }
        bits
    }

    /// Table 23 as the rendered page has it (PDF page 44), a field at a time:
    /// the bit each starts at, then its bits in the order they go, least
    /// significant first. The CRC was worked out apart from this code, on a
    /// model of Figure 14's cells, over bits 18 to 33, 35 to 50 and 52 to 67.
    #[test]
    fn a_type_0_mph_is_table_23_bit_for_bit() {
        let mph = Mph { precoding: None, ..recipient() };
        let bits = laid_out(&[
            (0, "11111111111111111"), // frame sync
            (17, "0"),                // start bit
            (18, "0"),                // type 0
            (19, "0"),                // reserved
            (20, "1011"),             // maximum rate 13, which is 31 200
            (24, "000"),              // reserved
            (27, "1"),                // 2400 bit/s for the far transmitter
            (28, "0"),                // reserved
            (29, "10"),               // trellis 1, 32 states
            (31, "1"),                // theta 0.3125
            (32, "0"),                // minimum shaping
            (33, "0"),                // reserved
            (34, "0"),                // start bit
            (35, "10111111111110"),   // 2400, not 4800, 7200 to 31 200, not 33 600
            (49, "0"),                // reserved
            (50, "1"),                // asymmetric control channel rates allowed
            (51, "0"),                // start bit
            (52, "0000000000000000"), // reserved
            (68, "0"),                // start bit
            (69, "1111010000110011"), // CRC
            (85, "000"),              // fill
        ]);
        assert_eq!(bits.len(), TYPE0_BITS);
        assert_eq!(mph.to_bits(), bits);
        assert_eq!(Mph::from_bits(&bits), Some(mph));
    }

    /// Table 24 (PDF page 45) the same way: Table 23's first 52 bits with
    /// type 1, then the coefficients, each 16 bits of two's complement and a
    /// start bit. The CRC is over bits 18 to 169 less the start bits.
    #[test]
    fn a_type_1_mph_is_table_24_bit_for_bit() {
        let bits = laid_out(&[
            (0, "11111111111111111"),  // frame sync
            (17, "0"),                 // start bit
            (18, "1"),                 // type 1
            (19, "0"),                 // reserved
            (20, "1011"),              // maximum rate 13
            (24, "000"),               // reserved
            (27, "1"),                 // 2400 bit/s for the far transmitter
            (28, "0"),                 // reserved
            (29, "10"),                // 32 states
            (31, "1"),                 // theta 0.3125
            (32, "0"),                 // minimum shaping
            (33, "0"),                 // reserved
            (34, "0"),                 // start bit
            (35, "10111111111110"),    // the rate mask
            (49, "0"),                 // reserved
            (50, "1"),                 // asymmetric control channel rates allowed
            (51, "0"),                 // start bit
            (52, "0000000000000100"),  // h(1) real, 0x2000: a half
            (68, "0"),                 // start bit
            (69, "0000000000001111"),  // h(1) imaginary, -0x1000: minus a quarter
            (85, "0"),                 // start bit
            (86, "1011111111111111"),  // h(2) real, -3
            (102, "0"),                // start bit
            (103, "1001110000001100"), // h(2) imaginary, 12345
            (119, "0"),                // start bit
            (120, "0000000000000011"), // h(3) real, -16384: minus one
            (136, "0"),                // start bit
            (137, "1110000000000000"), // h(3) imaginary, 7
            (153, "0"),                // start bit
            (154, "0000000000000000"), // reserved
            (170, "0"),                // start bit
            (171, "1011011100001011"), // CRC
            (187, "0"),                // fill
        ]);
        assert_eq!(bits.len(), TYPE1_BITS);
        assert_eq!(recipient().to_bits(), bits);
        assert_eq!(Mph::from_bits(&bits), Some(recipient()));
    }

    #[test]
    fn both_types_of_mph_come_back_and_a_wrong_bit_does_not() {
        let others = [
            Mph { precoding: None, trellis: Trellis::States64, expanded_shaping: true, ..recipient() },
            Mph { max_rate: 1, rates: 1, control_rate: ControlRate::Bps2400, ..Mph::default() },
            Mph { precoding: Some([(i16::MIN, i16::MAX), (-1, 1), (0, 0)]), ..source() },
        ];
        for mph in [source(), recipient()].into_iter().chain(others) {
            let bits = mph.to_bits();
            let length = if mph.precoding.is_some() { TYPE1_BITS } else { TYPE0_BITS };
            assert_eq!(bits.len(), length);
            assert_eq!(Mph::from_bits(&bits), Some(mph));
            for i in 0..crc_at(length) + 16 {
                let mut spoiled = bits.clone();
                spoiled[i] = !spoiled[i];
                assert_ne!(Mph::from_bits(&spoiled), Some(mph), "bit {i}");
            }
        }
    }

    /// "Reserved for ITU-T: These bits are set to 0 by the transmitting modem
    /// and are not interpreted by the receiving modem" -- but they are
    /// information all the same, and the CRC is over them.
    #[test]
    fn mph_s_reserved_bits_go_as_zeros_and_are_not_read() {
        for mph in [source(), recipient()] {
            let bits = mph.to_bits();
            let length = bits.len();
            let reserved: Vec<usize> =
                [19, 24, 25, 26, 28, 33, 49].into_iter().chain(if length == TYPE0_BITS { 52..68 } else { 154..170 }).collect();
            assert!(reserved.iter().all(|&i| !bits[i]), "sent as zeros");
            // From a modem that has found a use for them, sealed again.
            let mut used = bits.clone();
            for &i in &reserved {
                used[i] = true;
            }
            let sealed = crc(&covered(&used, length));
            for k in 0..16 {
                used[crc_at(length) + k] = sealed >> k & 1 == 1;
            }
            assert_eq!(Mph::from_bits(&used), Some(mph));
        }
        // Trellis code 3 is reserved too, and names none: 16 states.
        let mut bits = source().to_bits();
        bits[29] = true;
        bits[30] = true;
        let sealed = crc(&covered(&bits, TYPE0_BITS));
        for k in 0..16 {
            bits[crc_at(TYPE0_BITS) + k] = sealed >> k & 1 == 1;
        }
        assert_eq!(Mph::from_bits(&bits).map(|m| m.trellis), Some(Trellis::States16));
    }

    /// MPh keeps MP's frame (Tables 20 and 21 against 23 and 24): the one
    /// reads as the other, with the fields the two share where both put them.
    #[test]
    fn an_mph_has_an_mp_s_frame() {
        for mph in [source(), recipient()] {
            let mp = Mp::from_bits(&mph.to_bits()).expect("MP's framing");
            assert_eq!(mp.call_to_answer, mph.max_rate);
            assert_eq!((mp.trellis, mp.non_linear, mp.expanded_shaping), (mph.trellis, mph.non_linear, mph.expanded_shaping));
            assert_eq!((mp.rates, mp.precoding), (mph.rates, mph.precoding));
            assert!(!mp.auxiliary && !mp.acknowledge, "reserved in MPh");
        }
    }

    fn with(max_rate: u8, rates: u16) -> Mph {
        Mph { max_rate, rates, ..Mph::default() }
    }

    #[test]
    fn the_primary_channel_runs_at_the_fastest_rate_both_enable_within_both_maxima() {
        let all = Mp::rates_up_to(14);
        assert_eq!(primary_rate(&with(14, all), &with(14, all)), Some(14));
        // The lower maximum, whichever end sent it.
        assert_eq!(primary_rate(&with(14, all), &with(10, all)), Some(10));
        assert_eq!(primary_rate(&with(9, all), &with(14, all)), Some(9));
        // A maximum that is not enabled: the next rate down that both enable.
        let no_26400_or_28800 = all & !(1 << 11) & !(1 << 10);
        assert_eq!(primary_rate(&with(12, all), &with(12, no_26400_or_28800)), Some(10));
        assert_eq!(primary_rate(&with(14, all & !(1 << 13)), &with(14, all)), Some(13));
        // Enabled means enabled in both masks.
        assert_eq!(primary_rate(&with(14, 0b0111), &with(14, 0b1110)), Some(3));
        // And when nothing is, there is no rate (F-V34 16 in the notes): masks
        // with nothing in common, nothing in common at or below the maxima, a
        // maximum of 0, an empty mask.
        assert_eq!(primary_rate(&with(14, 0b0101), &with(14, 0b1010)), None);
        assert_eq!(primary_rate(&with(12, 0b11 << 12), &with(14, all)), None);
        assert_eq!(primary_rate(&with(0, all), &with(14, all)), None);
        assert_eq!(primary_rate(&with(14, 0), &with(14, all)), None);
        // Fifteen fits the field but is no rate of Table 23's.
        assert_eq!(primary_rate(&with(15, all), &with(15, all)), Some(14));
        // One rule, one number at both ends.
        for (a, b) in [(with(14, all), with(10, 0x0fff)), (with(7, 0x3ff0), with(12, all)), (source(), recipient())] {
            assert_eq!(primary_rate(&a, &b), primary_rate(&b, &a));
        }
        assert_eq!(primary_rate(&source(), &recipient()), Some(13));
    }

    #[test]
    fn the_control_channel_goes_at_what_the_far_end_asks_only_if_both_allow_it() {
        use ControlRate::{Bps1200, Bps2400};
        let asks = |control_rate, asymmetric_control| Mph { control_rate, asymmetric_control, ..Mph::default() };
        // Symmetric unless both say otherwise, and then the lower of the two.
        assert_eq!(control_rates(&asks(Bps2400, false), &asks(Bps1200, false)), (Bps1200, Bps1200));
        assert_eq!(control_rates(&asks(Bps2400, false), &asks(Bps2400, false)), (Bps2400, Bps2400));
        assert_eq!(control_rates(&asks(Bps1200, true), &asks(Bps2400, false)), (Bps1200, Bps1200));
        // Both allow it: each transmits at what the other asked for.
        assert_eq!(control_rates(&asks(Bps2400, true), &asks(Bps1200, true)), (Bps1200, Bps2400));
        // What one end sends at is what the other receives at, every way round.
        for a in [asks(Bps1200, false), asks(Bps2400, false), asks(Bps1200, true), asks(Bps2400, true)] {
            for b in [asks(Bps1200, false), asks(Bps2400, false), asks(Bps1200, true), asks(Bps2400, true)] {
                let (a_sends, a_hears) = control_rates(&a, &b);
                let (b_sends, b_hears) = control_rates(&b, &a);
                assert_eq!((a_sends, a_hears), (b_hears, b_sends), "{a:?} and {b:?}");
            }
        }
        assert_eq!((Bps1200.bits_per_symbol(), Bps2400.bits_per_symbol()), (2, 4));
    }

    /// ALT for `symbols` 600 baud symbols at 1200 bit/s: zeros and ones by
    /// turns, from `first`.
    fn alt(first: bool, symbols: usize) -> Vec<bool> {
        (0..2 * symbols).map(|i| (i % 2 == 1) != first).collect()
    }

    /// HDLC flags, the control channel's user data after E.
    fn flags() -> Vec<bool> {
        [false, true, true, true, true, true, true, false].repeat(4)
    }

    /// Bits as the far end's control channel sends them -- through the call
    /// modem's scrambler, "all zeroes" at the start of ALT (10.2.4.2) -- and as
    /// a receiver descrambles them, after descrambling PPh's points before
    /// them as if they were bits.
    fn through_the_scrambler(bits: &[bool]) -> Vec<bool> {
        let mut scrambler = Scrambler::new(Mode::Call);
        let mut descrambler = Scrambler::new(Mode::Call);
        for i in 0..64 {
            descrambler.descramble(i % 8 < 4);
        }
        bits.iter().map(|&bit| descrambler.descramble(scrambler.scramble(bit))).collect()
    }

    /// Every find, with the bit it was made on.
    fn finds(bits: &[bool]) -> Vec<(usize, MphFound)> {
        let mut finder = MphFinder::new();
        bits.iter().enumerate().filter_map(|(i, &bit)| finder.feed(bit).map(|found| (i, found))).collect()
    }

    #[test]
    fn an_mph_finder_finds_mph_after_alt_and_e_after_mph_and_nothing_in_alt() {
        // 12.4: ALT, then MPh until the far end's has come in, then E. An ALT
        // from zero ends on a one, which runs on into the first sync.
        for first in [false, true] {
            for (symbols, sent) in [(16, vec![source(), source()]), (120, vec![recipient()]), (40, vec![recipient(), recipient()])] {
                let mut bits = alt(first, symbols);
                let alt_ends = bits.len();
                for mph in &sent {
                    bits.extend(mph.to_bits());
                }
                bits.extend([true; E_BITS]);
                let e_ends = bits.len();
                bits.extend(flags());
                let found = finds(&through_the_scrambler(&bits));
                assert!(found.iter().all(|&(at, _)| at >= alt_ends), "{found:?} in ALT");
                let mut expected: Vec<MphFound> = sent.iter().map(|&mph| MphFound::Mph(mph)).collect();
                expected.push(MphFound::E);
                assert_eq!(found.iter().map(|&(_, f)| f).collect::<Vec<_>>(), expected, "ALT from {first}");
                assert_eq!(found.last().map(|&(at, _)| at), Some(e_ends - 1), "E on its last bit");
            }
        }
    }

    #[test]
    fn e_straight_after_alt_is_found_and_alt_alone_is_nothing() {
        // 12.6.1.4 and 12.6.2.2: a resynchronisation has ALT and E and no MPh.
        for first in [false, true] {
            let mut bits = alt(first, 16);
            let e_starts = bits.len();
            bits.extend([true; E_BITS]);
            bits.extend(flags());
            // A one at the end of ALT makes the run twenty-one long, and the
            // twentieth one is E's nineteenth.
            let at = e_starts + E_BITS - if first { 1 } else { 2 };
            assert_eq!(finds(&through_the_scrambler(&bits)), vec![(at, MphFound::E)], "ALT from {first}");
            // ALT at its longest, and nothing.
            assert_eq!(finds(&through_the_scrambler(&alt(first, 120))), vec![]);
        }
    }
}
