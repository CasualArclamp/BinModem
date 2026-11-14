//! ITU-T V.8 bis (11/2000): choosing what a call is going to be before any
//! modem has started.
//!
//! V.8 settles a modulation between two modems that have already decided to
//! be modems. V.8 bis comes before that. Either end of a telephone call can
//! ask the other what it can do, be told, and pick one thing: a data call on
//! V.34, a fax, or going back to talking. It is two kinds of thing on the
//! line (clause 7):
//!
//! - **Signals**, which are tones and carry no data: 400 ms of two tones at
//!   once and then 100 ms of one. The pair says which station is speaking and
//!   the single tone says what it wants (Table 1). They are made to be heard
//!   over speech.
//! - **Messages**, which are HDLC frames at 300 bit/s on V.21: the initiating
//!   station on the low channel, the responding one on the high (7.2). `CL`
//!   lists capabilities, `CLR` lists them and asks for the other end's, `MS`
//!   selects a mode, and `ACK` and `NAK` answer.
//!
//! This module is the part with no signal processing in it, as the rest of
//! this crate is for V.8: what the signals are, what a message's octets
//! mean, and the tree of parameters a `CL` carries (clause 8). Making the
//! tones and the frames, and the exchange of them, is `datapump::v8bis`.
//!
//! Bit order is the Recommendation's (7.2.2): bits are numbered 1 to 8, bit 1
//! is sent first, and in a one-octet field bit 1 is the least significant.

/// The revision this implements, and puts in every message (Table 4): the
/// 2000 edition is Revision 2.
pub const REVISION: u8 = 2;

/// Segment 1 of every signal, in seconds (7.1.2).
pub const SEGMENT_1: f64 = 0.400;
/// Segment 1 of MRe and CRe where the far end may be a modem without V.8 bis
/// (7.1.2): "may be reduced to 285 ms".
pub const SEGMENT_1_SHORT: f64 = 0.285;
/// Segment 2 of every signal (7.1.2).
pub const SEGMENT_2: f64 = 0.100;
/// The V.21 mark in front of every message (7.2.4). After an escape signal
/// its second segment is this already: it is the same tone.
pub const PREAMBLE: f64 = 0.100;
/// The most an information field may hold (9.10).
pub const MAX_INFORMATION: usize = 64;

/// Which of the two stations of a transaction. Whoever sends the first
/// signal is the initiating one "for the duration of the ensuing
/// transaction" (9.2), whichever of them placed the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Station {
    Initiating,
    Responding,
}

impl Station {
    pub fn other(self) -> Self {
        match self {
            Self::Initiating => Self::Responding,
            Self::Responding => Self::Initiating,
        }
    }

    /// The two tones of segment 1 from this station (Table 1).
    pub fn pair(self) -> (f64, f64) {
        match self {
            Self::Initiating => (1375.0, 2002.0),
            Self::Responding => (1529.0, 2225.0),
        }
    }
}

/// The six signals (7.1, Table 1).
///
/// The `e` ones are sent by a station that has answered a call by itself, at
/// call establishment; the `d` ones at any other time. `ES` is the escape
/// that goes in front of a message where no `MR` or `CR` did (9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Mode request: "tell me what mode to use".
    MRe,
    MRd,
    /// Capabilities request: "tell me what you can do".
    CRe,
    CRd,
    /// Escape, from the initiating station.
    ESi,
    /// Escape, from the responding station.
    ESr,
}

impl Signal {
    pub const ALL: [Self; 6] = [Self::MRe, Self::MRd, Self::CRe, Self::CRd, Self::ESi, Self::ESr];

    /// The single tone of segment 2, which is what tells the signals apart
    /// (Table 1; 10.1.3).
    pub fn segment_2(self) -> f64 {
        match self {
            Self::MRe => 650.0,
            Self::MRd => 1150.0,
            Self::CRe => 400.0,
            Self::CRd => 1900.0,
            Self::ESi => 980.0,
            Self::ESr => 1650.0,
        }
    }

    /// Whether `station` may send this one. MRe, CRe and ESi are the
    /// initiating station's and ESr the responding one's; MRd and CRd are
    /// either's, each on its own pair of tones (Table 1).
    pub fn sent_by(self, station: Station) -> bool {
        match self {
            Self::MRe | Self::CRe | Self::ESi => station == Station::Initiating,
            Self::ESr => station == Station::Responding,
            Self::MRd | Self::CRd => true,
        }
    }

    /// The signal a second segment at `hz` belongs to, from `station`.
    pub fn from_segment_2(hz: f64, station: Station) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.segment_2() == hz && s.sent_by(station))
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::MRe => "MRe",
            Self::MRd => "MRd",
            Self::CRe => "CRe",
            Self::CRd => "CRd",
            Self::ESi => "ESi",
            Self::ESr => "ESr",
        }
    }
}

/// What a message is (8.3.1, Table 3): bits 1 to 4 of its first octet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Mode select.
    Ms,
    /// Capabilities list.
    Cl,
    /// Capabilities list, and a request for the other end's.
    Clr,
    /// "the receipt of an MS message and that the mode is to be invoked".
    Ack1,
    /// A request for the rest of a list that said it had more (9.10).
    Ack2,
    /// An invalid frame was received (9.8).
    Nak1,
    /// The mode asked for cannot be invoked just now (9.5).
    Nak2,
    /// The mode asked for is not supported, or is turned off (9.5).
    Nak3,
    /// The message could not be interpreted; its use is for further study.
    Nak4,
    /// "Defined in ITU-T V.92": the quick connect messages, whose octets
    /// after the first are V.92's and not a tree (V.92 Tables 3 and 14).
    V92,
    /// Any other code point, kept so that it can be said what arrived.
    Other(u8),
}

impl Kind {
    pub fn code(self) -> u8 {
        match self {
            Self::Ms => 0b0001,
            Self::Cl => 0b0010,
            Self::Clr => 0b0011,
            Self::Ack1 => 0b0100,
            Self::Ack2 => 0b0101,
            Self::Nak1 => 0b1000,
            Self::Nak2 => 0b1001,
            Self::Nak3 => 0b1010,
            Self::Nak4 => 0b1011,
            Self::V92 => 0b1101,
            Self::Other(code) => code & 0xF,
        }
    }

    pub fn from_code(code: u8) -> Self {
        match code & 0xF {
            0b0001 => Self::Ms,
            0b0010 => Self::Cl,
            0b0011 => Self::Clr,
            0b0100 => Self::Ack1,
            0b0101 => Self::Ack2,
            0b1000 => Self::Nak1,
            0b1001 => Self::Nak2,
            0b1010 => Self::Nak3,
            0b1011 => Self::Nak4,
            0b1101 => Self::V92,
            other => Self::Other(other),
        }
    }

    /// Whether the message carries the parameter trees of clause 8. ACK and
    /// NAK do not: their parameter field "shall be of zero length" (8.3.3).
    pub fn has_parameters(self) -> bool {
        matches!(self, Self::Ms | Self::Cl | Self::Clr)
    }

    pub fn is_nak(self) -> bool {
        matches!(self, Self::Nak1 | Self::Nak2 | Self::Nak3 | Self::Nak4)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Ms => "MS",
            Self::Cl => "CL",
            Self::Clr => "CLR",
            Self::Ack1 => "ACK(1)",
            Self::Ack2 => "ACK(2)",
            Self::Nak1 => "NAK(1)",
            Self::Nak2 => "NAK(2)",
            Self::Nak3 => "NAK(3)",
            Self::Nak4 => "NAK(4)",
            Self::V92 => "V.92",
            Self::Other(_) => "reserved",
        }
    }
}

/// Bit 8, which ends a level 1 block and a Par(2) block (8.2.3).
const END_1: u8 = 0x80;
/// Bit 7, which ends a level 2 or level 3 block (8.2.3).
const END_2: u8 = 0x40;

/// One Par(2) block: the parameters under one bit of an SPar(1) (8.2.2).
///
/// Octets are kept without their delimiting bits, so six bits each.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Par2 {
    pub npar2: Vec<u8>,
    pub spar2: Vec<u8>,
    /// One block for each bit set in `spar2`, in the order of the bits.
    pub npar3: Vec<Vec<u8>>,
}

impl Par2 {
    /// A block of NPar(2) octets and nothing under them.
    pub fn flat(npar2: &[u8]) -> Self {
        Self { npar2: npar2.to_vec(), ..Self::default() }
    }

    fn encode(&self, out: &mut Vec<u8>) {
        let has_spar = self.spar2.iter().any(|o| o & 0x3F != 0);
        block(out, &self.npar2, 0x3F, END_2);
        if has_spar {
            block(out, &self.spar2, 0x3F, END_2);
            for npar3 in &self.npar3 {
                block(out, npar3, 0x3F, END_2);
            }
        }
        // Bit 8 on the last octet of the whole Par(2), whichever block that
        // was: with only NPar(2) octets, that is bits 7 and 8 together.
        if let Some(last) = out.last_mut() {
            *last |= END_1;
        }
    }

    fn decode(octets: &[u8], at: &mut usize) -> Option<Self> {
        let (npar2, ended) = take(octets, at, END_2, 0x3F)?;
        let mut par = Self { npar2, ..Self::default() };
        if ended {
            return Some(par);
        }
        let (spar2, mut ended) = take(octets, at, END_2, 0x3F)?;
        let below = spar2.iter().map(|o| o.count_ones()).sum::<u32>();
        par.spar2 = spar2;
        for _ in 0..below {
            if ended {
                // Fewer blocks than bits: a far end that set a bit and sent
                // nothing under it. Taken as it came (8.2.3 asks a receiver
                // to parse what it is given).
                break;
            }
            let (npar3, last) = take(octets, at, END_2, 0x3F)?;
            par.npar3.push(npar3);
            ended = last;
        }
        // Whatever is left of a Par(2) this end has no name for.
        while !ended {
            let (_, last) = take(octets, at, END_2, 0x3F)?;
            ended = last;
        }
        Some(par)
    }
}

/// Append a block: `octets` masked to their parameter bits, the last with
/// its delimiter set. A block with nothing in it is still one octet.
fn block(out: &mut Vec<u8>, octets: &[u8], mask: u8, end: u8) {
    if octets.is_empty() {
        out.push(end);
        return;
    }
    for (i, octet) in octets.iter().enumerate() {
        let last = i + 1 == octets.len();
        out.push(octet & mask | if last { end } else { 0 });
    }
}

/// Take one block delimited by `end` from `octets` at `at`: its octets with
/// the delimiters taken off, and whether bit 8 was set on its last one.
fn take(octets: &[u8], at: &mut usize, end: u8, mask: u8) -> Option<(Vec<u8>, bool)> {
    let mut block = Vec::new();
    loop {
        let octet = *octets.get(*at)?;
        *at += 1;
        block.push(octet & mask);
        if octet & end != 0 {
            return Some((block, octet & END_1 != 0));
        }
    }
}

/// A tree of parameters (8.2, Figure 6): the NPar(1) block, the SPar(1)
/// block, and a Par(2) block for each bit set in the SPar(1), in order.
///
/// Level 1 octets are kept without bit 8, so seven bits each. Each of the
/// two level 1 blocks is at least one octet, set or not, since that is what
/// is on the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub npar1: Vec<u8>,
    pub spar1: Vec<u8>,
    pub par2: Vec<Par2>,
}

impl Default for Tree {
    fn default() -> Self {
        Self { npar1: vec![0], spar1: vec![0], par2: Vec::new() }
    }
}

impl Tree {
    /// The octets on the line.
    pub fn encode(&self, out: &mut Vec<u8>) {
        block(out, &self.npar1, 0x7F, END_1);
        block(out, &self.spar1, 0x7F, END_1);
        for par in &self.par2 {
            par.encode(out);
        }
    }

    /// Read a tree from `octets` at `at`, leaving `at` after it.
    pub fn decode(octets: &[u8], at: &mut usize) -> Option<Self> {
        let (npar1, _) = take(octets, at, END_1, 0x7F)?;
        let (spar1, _) = take(octets, at, END_1, 0x7F)?;
        let below = spar1.iter().map(|o| o.count_ones()).sum::<u32>();
        let mut par2 = Vec::new();
        for _ in 0..below {
            par2.push(Par2::decode(octets, at)?);
        }
        Some(Self { npar1, spar1, par2 })
    }

    /// Whether bit `bit` (1 to 7) of NPar(1) octet `octet` (from 1) is set.
    pub fn npar1_bit(&self, octet: usize, bit: u8) -> bool {
        self.npar1.get(octet - 1).is_some_and(|o| o & (1 << (bit - 1)) != 0)
    }

    fn set_npar1_bit(&mut self, octet: usize, bit: u8, on: bool) {
        if self.npar1.len() < octet {
            self.npar1.resize(octet, 0);
        }
        let mask = 1 << (bit - 1);
        if on {
            self.npar1[octet - 1] |= mask;
        } else {
            self.npar1[octet - 1] &= !mask;
        }
    }

    /// Where the Par(2) block of SPar(1) bit `bit` of octet `octet` is, if
    /// that bit is set: the number of set bits in front of it.
    fn slot(&self, octet: usize, bit: u8) -> Option<usize> {
        let mask = 1 << (bit - 1);
        if self.spar1.get(octet - 1).is_none_or(|o| o & mask == 0) {
            return None;
        }
        let before: u32 = self.spar1[..octet - 1].iter().map(|o| o.count_ones()).sum();
        Some((before + (self.spar1[octet - 1] & (mask - 1)).count_ones()) as usize)
    }

    /// The Par(2) block under SPar(1) bit `bit` of octet `octet`.
    pub fn par2(&self, octet: usize, bit: u8) -> Option<&Par2> {
        self.par2.get(self.slot(octet, bit)?)
    }

    /// Set or clear SPar(1) bit `bit` of octet `octet` and the block under it.
    pub fn set_par2(&mut self, octet: usize, bit: u8, block: Option<Par2>) {
        if let Some(slot) = self.slot(octet, bit) {
            self.par2.remove(slot);
            self.spar1[octet - 1] &= !(1 << (bit - 1));
        }
        if let Some(block) = block {
            if self.spar1.len() < octet {
                self.spar1.resize(octet, 0);
            }
            self.spar1[octet - 1] |= 1 << (bit - 1);
            let slot = self.slot(octet, bit).expect("the bit was just set");
            self.par2.insert(slot, block);
        }
    }
}

/// The parameters of the identification field (8.3.3, Tables 5-1 to 5-3):
/// what the station wants of the exchange itself and of the start-up after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Identification {
    /// Table 5-1 bit 1. In a CL, "supports the V.8 start-up procedure"; in an
    /// MS, that one is asked for after the transaction (9.9.1).
    pub v8: bool,
    /// Bit 2: the same for the shortened V.8 start-up (9.9.2).
    pub short_v8: bool,
    /// Bit 3: there is more to this list than fitted (9.10).
    pub more: bool,
    /// Bit 4: an MS that is accepted is to be answered with ACK(1) before the
    /// answer tone, and not by the answer tone alone (9.7).
    pub ack1: bool,
    /// Table 5-2 bit 1 and Table 5-3: the network this end is on. None is an
    /// analogue telephone line (Table 5-2's note).
    pub network: Option<Network>,
}

/// Table 5-3: how the station reaches the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Network {
    pub cellular: bool,
    pub isdn: bool,
    /// "digital PSTN access": digital, but carrying coded speech-band signals,
    /// which is where a V.90 or V.92 digital modem sits.
    pub digital: bool,
}

impl Identification {
    pub fn to_tree(self) -> Tree {
        let mut tree = Tree::default();
        tree.set_npar1_bit(1, 1, self.v8);
        tree.set_npar1_bit(1, 2, self.short_v8);
        tree.set_npar1_bit(1, 3, self.more);
        tree.set_npar1_bit(1, 4, self.ack1);
        if let Some(n) = self.network {
            let octet = u8::from(n.cellular) | u8::from(n.isdn) << 1 | u8::from(n.digital) << 2;
            tree.set_par2(1, 1, Some(Par2::flat(&[octet])));
        }
        tree
    }

    pub fn from_tree(tree: &Tree) -> Self {
        Self {
            v8: tree.npar1_bit(1, 1),
            short_v8: tree.npar1_bit(1, 2),
            more: tree.npar1_bit(1, 3),
            ack1: tree.npar1_bit(1, 4),
            network: tree.par2(1, 1).map(|p| {
                let o = p.npar2.first().copied().unwrap_or(0);
                Network { cellular: o & 1 != 0, isdn: o & 2 != 0, digital: o & 4 != 0 }
            }),
        }
    }
}

/// The data mode's parameters (Tables 6-3a to 6-3d): what runs over the
/// modem and which modems there are to run it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Data {
    // Table 6-3a.
    pub transparent: bool,
    /// V.42 error control; "LAPM in V.8 bis" as 9.3.1/V.92 has it.
    pub v42: bool,
    pub v42bis: bool,
    pub v14: bool,
    // Table 6-3b.
    pub v34: bool,
    pub v32bis: bool,
    // Table 6-3c.
    pub v32: bool,
    pub v22bis: bool,
    pub v22: bool,
    pub v21: bool,
    pub v90_analogue: bool,
    pub v90_digital: bool,
    // Table 6-3d.
    pub v91: bool,
    pub v92_analogue: bool,
    pub v92_digital: bool,
}

/// A modulation a data call can be on, fastest first: what an MS names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modulation {
    V21,
    V22,
    V22bis,
    V32,
    V32bis,
    V34,
    V90,
    V92,
}

impl Data {
    fn octets(self) -> [u8; 4] {
        let bits = |list: [bool; 6]| list.iter().enumerate().fold(0u8, |o, (i, b)| o | u8::from(*b) << i);
        [
            bits([self.transparent, self.v42, self.v42bis, self.v14, false, false]),
            bits([false, false, false, false, self.v34, self.v32bis]),
            bits([self.v32, self.v22bis, self.v22, self.v21, self.v90_analogue, self.v90_digital]),
            bits([self.v91, self.v92_analogue, self.v92_digital, false, false, false]),
        ]
    }

    fn from_octets(octets: &[u8]) -> Self {
        let bit = |octet: usize, bit: u8| octets.get(octet).is_some_and(|o| o & (1 << bit) != 0);
        Self {
            transparent: bit(0, 0),
            v42: bit(0, 1),
            v42bis: bit(0, 2),
            v14: bit(0, 3),
            v34: bit(1, 4),
            v32bis: bit(1, 5),
            v32: bit(2, 0),
            v22bis: bit(2, 1),
            v22: bit(2, 2),
            v21: bit(2, 3),
            v90_analogue: bit(2, 4),
            v90_digital: bit(2, 5),
            v91: bit(3, 0),
            v92_analogue: bit(3, 1),
            v92_digital: bit(3, 2),
        }
    }

    /// The Par(2) block these make: as few octets as hold the bits that are
    /// set, since an MS "shall contain the minimum number of information
    /// octets required" (9.1, Note 2).
    pub fn to_par2(self) -> Par2 {
        let octets = self.octets();
        let used = octets.iter().rposition(|o| *o != 0).map_or(1, |last| last + 1);
        Par2::flat(&octets[..used])
    }

    pub fn from_par2(par: &Par2) -> Self {
        Self::from_octets(&par.npar2)
    }

    /// The fastest modulation both ends have, with `self` this end's list
    /// and `far` the other's.
    ///
    /// V.90 and V.92 are not one capability but two halves, and a call needs
    /// one of each: this end's analogue half with the far end's digital, or
    /// the other way about (Table 6-3c's note).
    pub fn best_with(&self, far: &Self) -> Option<Modulation> {
        let pair = |a1: bool, d1: bool, a2: bool, d2: bool| (a1 && d2) || (d1 && a2);
        let list = [
            (pair(self.v92_analogue, self.v92_digital, far.v92_analogue, far.v92_digital), Modulation::V92),
            (pair(self.v90_analogue, self.v90_digital, far.v90_analogue, far.v90_digital), Modulation::V90),
            (self.v34 && far.v34, Modulation::V34),
            (self.v32bis && far.v32bis, Modulation::V32bis),
            (self.v32 && far.v32, Modulation::V32),
            (self.v22bis && far.v22bis, Modulation::V22bis),
            (self.v22 && far.v22, Modulation::V22),
            (self.v21 && far.v21, Modulation::V21),
        ];
        list.into_iter().find(|(both, _)| *both).map(|(_, m)| m)
    }

    /// The fastest modulation in this list alone, for a station that has to
    /// choose with nothing of the far end's to go on.
    pub fn best(&self) -> Option<Modulation> {
        let list = [
            (self.v92_analogue || self.v92_digital, Modulation::V92),
            (self.v90_analogue || self.v90_digital, Modulation::V90),
            (self.v34, Modulation::V34),
            (self.v32bis, Modulation::V32bis),
            (self.v32, Modulation::V32),
            (self.v22bis, Modulation::V22bis),
            (self.v22, Modulation::V22),
            (self.v21, Modulation::V21),
        ];
        list.into_iter().find(|(has, _)| *has).map(|(_, m)| m)
    }

    /// Only `modulation`, with the half of a PCM pair this end is, and the
    /// protocol bits kept: what goes in an MS.
    pub fn only(&self, modulation: Modulation) -> Self {
        let mut one = Self { transparent: self.transparent, v42: self.v42, v42bis: self.v42bis, v14: self.v14, ..Self::default() };
        match modulation {
            Modulation::V21 => one.v21 = true,
            Modulation::V22 => one.v22 = true,
            Modulation::V22bis => one.v22bis = true,
            Modulation::V32 => one.v32 = true,
            Modulation::V32bis => one.v32bis = true,
            Modulation::V34 => one.v34 = true,
            Modulation::V90 => {
                one.v90_analogue = self.v90_analogue;
                one.v90_digital = self.v90_digital;
            }
            Modulation::V92 => {
                one.v92_analogue = self.v92_analogue;
                one.v92_digital = self.v92_digital;
            }
        }
        one
    }

    /// The one modulation an MS names: the fastest set, which in an MS is the
    /// only one.
    pub fn selected(&self) -> Option<Modulation> {
        self.best()
    }

    /// Whether this end, with these capabilities, can do what `ms` asks:
    /// the modulation it names, as the other half where it is a pair.
    pub fn can_do(&self, ms: &Self) -> bool {
        match ms.selected() {
            None => false,
            Some(Modulation::V92) => {
                (self.v92_analogue && ms.v92_digital) || (self.v92_digital && ms.v92_analogue)
            }
            Some(Modulation::V90) => {
                (self.v90_analogue && ms.v90_digital) || (self.v90_digital && ms.v90_analogue)
            }
            Some(Modulation::V34) => self.v34,
            Some(Modulation::V32bis) => self.v32bis,
            Some(Modulation::V32) => self.v32,
            Some(Modulation::V22bis) => self.v22bis,
            Some(Modulation::V22) => self.v22,
            Some(Modulation::V21) => self.v21,
        }
    }
}

/// SPar(1) octet 1 of the standard information field (Table 6-2a): the bit
/// of each mode a station can be in.
pub mod mode {
    pub const DATA: u8 = 1;
    pub const SVD: u8 = 2;
    pub const H324: u8 = 3;
    pub const V18: u8 = 4;
    pub const FAX: u8 = 5;
    pub const TELEPHONY: u8 = 6;
    pub const T101: u8 = 7;
}

/// One message (7.2, clause 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: Kind,
    /// Table 4. Sent as [`REVISION`]; a receiver takes whatever arrives.
    pub revision: u8,
    /// The identification field's parameters (8.3.3). Empty in ACK and NAK.
    pub identification: Tree,
    /// The standard information field (8.4). Empty in ACK and NAK.
    pub standard: Tree,
    /// Whatever followed the first octet of a message that is not a tree:
    /// V.92's, or a reserved type's.
    pub raw: Vec<u8>,
}

impl Message {
    /// A message of `kind` with nothing in it, which for ACK and NAK is all
    /// there is.
    pub fn bare(kind: Kind) -> Self {
        Self { kind, revision: REVISION, identification: Tree::default(), standard: Tree::default(), raw: Vec::new() }
    }

    /// A CL, CLR or MS for a data station: `identification` for the
    /// exchange, and `data` as the one mode in the standard field.
    pub fn data(kind: Kind, identification: Identification, data: Data) -> Self {
        let mut message = Self::bare(kind);
        message.identification = identification.to_tree();
        message.standard.set_par2(1, mode::DATA, Some(data.to_par2()));
        message
    }

    pub fn identify(&self) -> Identification {
        Identification::from_tree(&self.identification)
    }

    /// The data mode's parameters, if the message has the data mode in it.
    pub fn data_mode(&self) -> Option<Data> {
        self.standard.par2(1, mode::DATA).map(Data::from_par2)
    }

    /// Whether the message offers or selects plain telephony (Table 6-2a).
    pub fn telephony(&self) -> bool {
        self.standard.par2(1, mode::TELEPHONY).is_some()
    }

    /// The information field: what goes between the flags, before the FCS.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.kind.code() | (self.revision & 0xF) << 4];
        if self.kind.has_parameters() {
            self.identification.encode(&mut out);
            self.standard.encode(&mut out);
        } else {
            out.extend_from_slice(&self.raw);
        }
        out
    }

    /// Read an information field. None if a tree in it runs off the end.
    pub fn decode(octets: &[u8]) -> Option<Self> {
        let first = *octets.first()?;
        let mut message = Self::bare(Kind::from_code(first));
        message.revision = first >> 4;
        if message.kind.has_parameters() {
            let mut at = 1;
            message.identification = Tree::decode(octets, &mut at)?;
            message.standard = Tree::decode(octets, &mut at)?;
            // What follows is the non-standard field, which this end has no
            // use for and is not an error (8.5).
        } else {
            message.raw = octets[1..].to_vec();
        }
        Some(message)
    }

    /// The MS this end sends to select `modulation`, from its own
    /// capabilities `ours`: the one mode, the one modulation, and the
    /// start-up wanted after it.
    pub fn select(ours: &Self, modulation: Modulation) -> Self {
        let data = ours.data_mode().unwrap_or_default().only(modulation);
        let mut id = ours.identify();
        id.more = false;
        Self::data(Kind::Ms, id, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8.3.1, Tables 3 and 4: the type in bits 1 to 4 and the revision in
    /// bits 5 to 8, so a Revision 2 message opens with 0x2 and its type.
    #[test]
    fn the_first_octet_is_the_type_and_the_revision() {
        for (kind, octet) in [
            (Kind::Ms, 0x21),
            (Kind::Cl, 0x22),
            (Kind::Clr, 0x23),
            (Kind::Ack1, 0x24),
            (Kind::Ack2, 0x25),
            (Kind::Nak1, 0x28),
            (Kind::Nak2, 0x29),
            (Kind::Nak3, 0x2A),
            (Kind::Nak4, 0x2B),
            (Kind::V92, 0x2D),
        ] {
            let octets = Message::bare(kind).encode();
            assert_eq!(octets[0], octet, "{}", kind.name());
            // ACK and NAK are that octet and no more (8.3.3, 8.4.1).
            assert_eq!(octets.len() == 1, !kind.has_parameters(), "{}", kind.name());
            assert_eq!(Message::decode(&octets).unwrap().kind, kind);
        }
    }

    /// A CL for "V.8 start-up, data, V.42, V.92 analogue" and nothing else,
    /// octet for octet as clause 8 lays it out.
    #[test]
    fn a_capabilities_list_is_the_octets_clause_8_gives() {
        let cl = Message::data(
            Kind::Cl,
            Identification { v8: true, ..Identification::default() },
            Data { v42: true, v92_analogue: true, ..Data::default() },
        );
        assert_eq!(
            cl.encode(),
            [
                0x22, // CL, Revision 2
                0x81, // I NPar(1): V.8 start-up, and the last octet
                0x80, // I SPar(1): nothing, so no network type: an analogue line
                0x80, // S NPar(1): nothing
                0x81, // S SPar(1): data
                0x02, 0x00, 0x00, 0xC2, // data NPar(2): V.42; -; -; V.92 analogue, bits 7 and 8
            ]
        );
        assert_eq!(Message::decode(&cl.encode()), Some(cl));
    }

    /// The network type is a Par(2) of the identification field, and its
    /// absence means an analogue line (Table 5-2).
    #[test]
    fn a_digital_station_says_so_in_the_identification_field() {
        let id = Identification { short_v8: true, ack1: true, network: Some(Network { digital: true, ..Network::default() }), ..Identification::default() };
        let cl = Message::data(Kind::Cl, id, Data { v34: true, v90_digital: true, ..Data::default() });
        let octets = cl.encode();
        // I: NPar(1) short V.8 + ACK(1); SPar(1) network type; its NPar(2).
        assert_eq!(&octets[..4], [0x22, 0x8A, 0x81, 0xC4]);
        let back = Message::decode(&octets).unwrap();
        assert_eq!(back.identify(), id);
        assert_eq!(back.data_mode(), Some(Data { v34: true, v90_digital: true, ..Data::default() }));
    }

    /// 8.2.3: a Par(2) with SPar(2) and NPar(3) blocks under it is delimited
    /// by bit 7 within and bit 8 at its end, and a decoder that knows none of
    /// its bits still comes out at the right octet.
    #[test]
    fn a_tree_with_three_levels_reads_back_and_is_stepped_over() {
        let mut tree = Tree::default();
        tree.set_par2(1, mode::SVD, Some(Par2 { npar2: vec![0x05, 0x01], spar2: vec![0x03], npar3: vec![vec![0x11], vec![0x02, 0x04]] }));
        tree.set_par2(1, mode::DATA, Some(Data { v34: true, ..Data::default() }.to_par2()));
        let mut octets = Vec::new();
        tree.encode(&mut octets);
        assert_eq!(
            octets,
            [
                0x80, // NPar(1): nothing
                0x83, // SPar(1): data and SVD, so two Par(2) blocks, data's first
                0x00, 0xD0, // data: NPar(2) alone, bits 7 and 8 on its last octet
                0x05, 0x41, // SVD: NPar(2), bit 7 on its last
                0x43, // its SPar(2): two bits, so two NPar(3) blocks
                0x51, // the first NPar(3)
                0x02, 0xC4, // the second, and bit 8 ends the Par(2)
            ]
        );
        let mut at = 0;
        assert_eq!(Tree::decode(&octets, &mut at), Some(tree));
        assert_eq!(at, octets.len());
    }

    /// V.90 and V.92 are halves: two analogue modems have neither.
    #[test]
    fn the_fastest_common_modulation_needs_one_of_each_half() {
        let caller = Data { v34: true, v32bis: true, v90_analogue: true, v92_analogue: true, ..Data::default() };
        let server = Data { v34: true, v90_digital: true, v92_digital: true, ..Data::default() };
        let v90_server = Data { v34: true, v90_digital: true, ..Data::default() };
        assert_eq!(caller.best_with(&server), Some(Modulation::V92));
        assert_eq!(caller.best_with(&v90_server), Some(Modulation::V90));
        assert_eq!(caller.best_with(&caller), Some(Modulation::V34));
        assert_eq!(Data { v22bis: true, ..Data::default() }.best_with(&caller), None);
    }

    /// An MS names one modulation, in as few octets as hold it (9.1 Note 2),
    /// and the end that gets it can tell whether it is the other half.
    #[test]
    fn a_mode_select_names_one_modulation_in_the_fewest_octets() {
        let ours = Message::data(
            Kind::Cl,
            Identification { v8: true, ack1: true, ..Identification::default() },
            Data { v42: true, v34: true, v32bis: true, v90_analogue: true, ..Data::default() },
        );
        let ms = Message::select(&ours, Modulation::V34);
        assert_eq!(ms.encode(), [0x21, 0x89, 0x80, 0x80, 0x81, 0x02, 0xD0]);
        assert_eq!(ms.data_mode().unwrap().selected(), Some(Modulation::V34));

        let server = Data { v34: true, v90_digital: true, ..Data::default() };
        let v90 = Message::select(&ours, Modulation::V90).data_mode().unwrap();
        assert!(server.can_do(&v90), "a digital modem can answer an analogue one's V.90");
        assert!(!ours.data_mode().unwrap().can_do(&v90), "an analogue modem cannot");
    }

    /// V.92's quick connect messages are V.8 bis frames of type 1101 whose
    /// second octet is V.92's own (V.92 Tables 3 and 14); they pass through.
    #[test]
    fn a_v92_message_keeps_its_octets() {
        let qc2a = Message::decode(&[0x2D, 0x25]).unwrap();
        assert_eq!(qc2a.kind, Kind::V92);
        assert_eq!(qc2a.raw, [0x25]);
        assert_eq!(qc2a.encode(), [0x2D, 0x25]);
    }

    /// Table 1: every signal has its own second tone, and the pair of the
    /// first says which station it came from.
    #[test]
    fn the_second_segment_names_the_signal() {
        let mut tones: Vec<f64> = Signal::ALL.iter().map(|s| s.segment_2()).collect();
        tones.dedup();
        assert_eq!(tones, [650.0, 1150.0, 400.0, 1900.0, 980.0, 1650.0]);
        assert_eq!(Signal::from_segment_2(400.0, Station::Initiating), Some(Signal::CRe));
        assert_eq!(Signal::from_segment_2(400.0, Station::Responding), None);
        assert_eq!(Signal::from_segment_2(1900.0, Station::Responding), Some(Signal::CRd));
        assert_eq!(Signal::from_segment_2(1650.0, Station::Responding), Some(Signal::ESr));
        assert_eq!(Station::Initiating.pair(), (1375.0, 2002.0));
        assert_eq!(Station::Responding.pair(), (1529.0, 2225.0));
    }
}
