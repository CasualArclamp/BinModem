//! What the two machines say to each other around the page: T.30.
//!
//! The control channel of a fax call is V.21 channel 2 at 300 bit/s carrying
//! HDLC frames, and every frame has the same shape: address 0xFF, a control
//! octet, a function code, and for some of them a field of parameters. This
//! is the reading of those, and in particular of the one that says what the
//! machine on the other end can do.
//!
//! Bit order is the thing to get right. 5.3.6.2.4: the parameter field is
//! numbered from bit 1, and bit 1 is the first bit transmitted, which is the
//! least significant bit of the first octet. Read an octet the other way and
//! a machine that receives faxes at 14 400 turns into one that does not
//! receive faxes at all.

/// Every T.30 frame is addressed to 0xFF (5.3.6.1).
pub const ADDRESS: u8 = 0xFF;
/// Control octet for a frame with more to follow, and for the last one.
pub const CONTROL_MORE: u8 = 0x03;
pub const CONTROL_FINAL: u8 = 0x13;

/// Facsimile control field: which frame this is (Table 3/T.30).
///
/// The values here are as they go on the line. Several differ only in the
/// bit that says which end originated the call, which is why DIS and DTC,
/// and CSI and CIG, share a number below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// Non-standard facilities: a manufacturer's own extensions.
    Nsf,
    /// Called subscriber identification: the answering machine's number.
    Csi,
    /// Digital identification signal: what the answering machine can do.
    Dis,
    /// Transmitting subscriber identification.
    Tsi,
    /// Digital command signal: what the calling machine has chosen.
    Dcs,
    /// Confirmation to receive: the training was good enough.
    Cfr,
    /// Failure to train: it was not.
    Ftt,
    /// Message confirmation: the page arrived.
    Mcf,
    /// Retrain positive and negative: the page arrived, or did not, and the
    /// two ends should train again either way.
    Rtp,
    Rtn,
    /// End of procedure, end of message, multi-page signal.
    Eop,
    Eom,
    Mps,
    /// Disconnect.
    Dcn,
    /// Error correction mode, T.30 A.4: a partial page has ended (PPS), the
    /// sender has given up on some frames (EOR), is asking whether the
    /// receiver is ready (RR), or will correct at a new rate (CTC).
    Pps,
    Eor,
    Rr,
    Ctc,
    /// And the answers: these frames again (PPR), not ready (RNR), end of
    /// retransmission acknowledged (ERR), the new rate accepted (CTR).
    Ppr,
    Rnr,
    Err,
    Ctr,
    Unknown(u8),
}

impl Frame {
    /// Read a control field.
    ///
    /// Every code here is 5.3.6.1's own bit string turned into the octet it
    /// goes out as. The strings are written first bit leftmost and the first
    /// bit is the least significant of the octet, so DIS's "0000 0001" is
    /// 0x80 and not 0x01 -- which is what a real machine sends, and what a
    /// recording of one confirms.
    ///
    /// Bit 1 is X, and X says which end is speaking rather than what it is
    /// saying: "set to 1 by the terminal which receives a valid DIS signal".
    /// So it comes off before the comparison, and DIS and DTC, CSI and CIG,
    /// NSF and NSC each collapse onto one code -- which is the truth about
    /// them, since each pair is the same frame from the two ends.
    pub fn from_code(code: u8) -> Self {
        match code & !0x01 {
            0x20 => Self::Nsf,
            0x40 => Self::Csi,
            0x80 => Self::Dis,
            0x42 => Self::Tsi,
            0x82 => Self::Dcs,
            0x84 => Self::Cfr,
            0x44 => Self::Ftt,
            0x8C => Self::Mcf,
            0xCC => Self::Rtp,
            0x4C => Self::Rtn,
            0x2E => Self::Eop,
            0x8E => Self::Eom,
            0x4E => Self::Mps,
            0xFA => Self::Dcn,
            0xBE => Self::Pps,
            0xCE => Self::Eor,
            0x6E => Self::Rr,
            0x12 => Self::Ctc,
            0xBC => Self::Ppr,
            0xEC => Self::Rnr,
            0x1C => Self::Err,
            0xC4 => Self::Ctr,
            other => Self::Unknown(other),
        }
    }

    /// The octet this frame goes out as.
    ///
    /// The inverse of [`from_code`](Self::from_code), and X has to be put
    /// back: it is set by whichever end received the capabilities, so on an
    /// ordinary call every frame from the end that dialled carries it and
    /// every frame from the end that answered does not.
    pub fn code(self, from_caller: bool) -> u8 {
        let base = match self {
            Self::Nsf => 0x20,
            Self::Csi => 0x40,
            Self::Dis => 0x80,
            Self::Tsi => 0x42,
            Self::Dcs => 0x82,
            Self::Cfr => 0x84,
            Self::Ftt => 0x44,
            Self::Mcf => 0x8C,
            Self::Rtp => 0xCC,
            Self::Rtn => 0x4C,
            Self::Eop => 0x2E,
            Self::Eom => 0x8E,
            Self::Mps => 0x4E,
            Self::Dcn => 0xFA,
            // A.4.1 to A.4.4, first bit on the left: X111 1101, X111 0011,
            // X111 0110, X100 1000, X011 1101, X011 0111, X011 1000 and
            // X010 0011.
            Self::Pps => 0xBE,
            Self::Eor => 0xCE,
            Self::Rr => 0x6E,
            Self::Ctc => 0x12,
            Self::Ppr => 0xBC,
            Self::Rnr => 0xEC,
            Self::Err => 0x1C,
            Self::Ctr => 0xC4,
            Self::Unknown(code) => code,
        };
        base | u8::from(from_caller)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Nsf => "NSF",
            Self::Csi => "CSI",
            Self::Dis => "DIS",
            Self::Tsi => "TSI",
            Self::Dcs => "DCS",
            Self::Cfr => "CFR",
            Self::Ftt => "FTT",
            Self::Mcf => "MCF",
            Self::Rtp => "RTP",
            Self::Rtn => "RTN",
            Self::Eop => "EOP",
            Self::Eom => "EOM",
            Self::Mps => "MPS",
            Self::Dcn => "DCN",
            Self::Pps => "PPS",
            Self::Eor => "EOR",
            Self::Rr => "RR",
            Self::Ctc => "CTC",
            Self::Ppr => "PPR",
            Self::Rnr => "RNR",
            Self::Err => "ERR",
            Self::Ctr => "CTR",
            Self::Unknown(_) => "?",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Self::Nsf => "non-standard facilities",
            Self::Csi => "called subscriber identification",
            Self::Dis => "what the far end can do",
            Self::Tsi => "transmitting subscriber identification",
            Self::Dcs => "what this call will use",
            Self::Cfr => "confirmation to receive",
            Self::Ftt => "failure to train",
            Self::Mcf => "the page arrived",
            Self::Rtp => "the page arrived; train again",
            Self::Rtn => "the page did not arrive; train again",
            Self::Eop => "end of procedure",
            Self::Eom => "end of message",
            Self::Mps => "another page follows",
            Self::Dcn => "disconnect",
            Self::Pps => "end of a partial page",
            Self::Eor => "end of retransmission",
            Self::Rr => "are you ready",
            Self::Ctc => "continue to correct, at this rate",
            Self::Ppr => "these frames again",
            Self::Rnr => "not ready",
            Self::Err => "end of retransmission understood",
            Self::Ctr => "correcting at that rate",
            Self::Unknown(_) => "not a frame this knows",
        }
    }
}

/// Bit `n` of a parameter field, numbered from 1 as T.30 numbers them.
///
/// Bit 1 is the first bit on the line, which is the least significant bit of
/// the first octet.
pub fn bit(fif: &[u8], n: usize) -> bool {
    let (octet, within) = ((n - 1) / 8, (n - 1) % 8);
    fif.get(octet).is_some_and(|o| o >> within & 1 == 1)
}

/// A run of bits, read as Table 2 writes it: first bit leftmost.
///
/// Which is the opposite way round from how they arrive, and the reason the
/// distinction is worth a function. The table gives bits 19 and 20 as "0 1"
/// for unlimited paper length, so the value wanted is 0b01 with bit 19 on the
/// left -- read the other way it is 0b10, which is the row above and says the
/// machine takes B4.
pub fn field_of(fif: &[u8], from: usize, to: usize) -> u8 {
    let mut v = 0u8;
    for n in from..=to {
        v = v << 1 | u8::from(bit(fif, n));
    }
    v
}

/// The modulations a fax can carry a page with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modulation {
    /// 2400 and 4800, differential phase shift keying at 1600 baud.
    V27ter,
    /// 7200 and 9600, sixteen points at 2400 baud.
    V29,
    /// 7200 to 14 400, trellis coded at 2400 baud.
    V17,
}

impl Modulation {
    pub fn name(self) -> &'static str {
        match self {
            Self::V27ter => "V.27ter",
            Self::V29 => "V.29",
            Self::V17 => "V.17",
        }
    }

    /// The rates it carries, fastest first.
    pub fn rates(self) -> &'static [u32] {
        match self {
            Self::V27ter => &[4800, 2400],
            Self::V29 => &[9600, 7200],
            Self::V17 => &[14_400, 12_000, 9600, 7200],
        }
    }
}

/// What the far end said it can do, read out of a DIS.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Capabilities {
    /// Bits 11 to 14, as sent.
    pub rate_field: u8,
    pub modulations: Vec<Modulation>,
    /// Bit 10.
    pub receives: bool,
    /// Bit 9: it is offering a document for the caller to fetch.
    pub can_be_polled: bool,
    /// Bit 15.
    pub fine_resolution: bool,
    /// Bit 16.
    pub two_dimensional: bool,
    /// Bits 17 and 18, in millimetres of paper across a line.
    pub widths_mm: Vec<u32>,
    /// Bits 19 and 20.
    pub length: &'static str,
    /// Bits 21 to 23, in milliseconds at 3.85 lines/mm.
    pub scan_line_ms: f64,
    /// Bit 27.
    pub error_correction: bool,
    /// Bit 31.
    pub t6_coding: bool,
    /// How many octets the field ran to.
    pub octets: usize,
}

/// Bits 11 to 14 of a DIS (Table 2/T.30).
///
/// A set rather than a rate: a DIS says which Recommendations the machine
/// has, and the DCS that answers it picks one rate out of them. The three
/// that matter are the three fax has ever used.
fn modulations_of(code: u8) -> Vec<Modulation> {
    use Modulation::{V17, V27ter, V29};
    match code {
        // 0000 is V.27ter at 2400 alone, which the table calls its fall-back
        // mode: the one rate every fax machine ever built has.
        0b0000 | 0b0100 => vec![V27ter],
        0b1000 => vec![V29],
        0b1100 => vec![V27ter, V29],
        0b1101 => vec![V27ter, V29, V17],
        _ => Vec::new(),
    }
}

/// The one rate a command frame names (Table 2, the DCS column).
///
/// A capability frame lists what a machine has; a command frame picks one
/// thing out of it, so the same four bits mean something different in each.
/// Reading a command with the capability table gives "none this knows", which
/// is what a real call's command frame did until this existed.
pub fn command_rate(fif: &[u8]) -> Option<(Modulation, u32)> {
    use Modulation::{V17, V27ter, V29};
    Some(match field_of(fif, 11, 14) {
        0b0000 => (V27ter, 2400),
        0b0100 => (V27ter, 4800),
        0b1000 => (V29, 9600),
        0b1100 => (V29, 7200),
        0b0001 => (V17, 14_400),
        0b0101 => (V17, 12_000),
        0b1001 => (V17, 9600),
        0b1101 => (V17, 7200),
        _ => return None,
    })
}

/// Bits 21 to 23: how long a scan line must take at the receiver.
///
/// Not a property of the page but of the paper going through the machine: a
/// thermal head can only print so fast, so the sender pads each coded line
/// with fill bits until it has taken this long. Zero means the machine can
/// take them as fast as they come.
fn scan_line_ms(code: u8) -> f64 {
    match code {
        0b000 => 20.0,
        0b001 => 40.0,
        0b010 => 10.0,
        0b100 => 5.0,
        // The three where the fine resolution takes half as long per line as
        // the standard one, which is the same milliseconds at 3.85.
        0b011 => 10.0,
        0b110 => 20.0,
        0b101 => 40.0,
        0b111 => 0.0,
        _ => 20.0,
    }
}

/// Read a DIS or DTC parameter field.
pub fn capabilities(fif: &[u8]) -> Capabilities {
    let rate_field = field_of(fif, 11, 14);
    let widths = match field_of(fif, 17, 18) {
        0b00 => vec![215],
        0b01 => vec![215, 255, 303],
        0b10 => vec![215, 255],
        _ => vec![215],
    };
    Capabilities {
        rate_field,
        modulations: modulations_of(rate_field),
        receives: bit(fif, 10),
        can_be_polled: bit(fif, 9),
        fine_resolution: bit(fif, 15),
        two_dimensional: bit(fif, 16),
        widths_mm: widths,
        length: match field_of(fif, 19, 20) {
            0b00 => "A4, 297 mm",
            0b01 => "unlimited",
            0b10 => "A4 and B4, 364 mm",
            _ => "invalid",
        },
        scan_line_ms: scan_line_ms(field_of(fif, 21, 23)),
        error_correction: bit(fif, 27),
        // Note 9: "valid only when bit 27 (error correction mode) is set".
        t6_coding: bit(fif, 31) && bit(fif, 27),
        octets: fif.len(),
    }
}

impl Capabilities {
    /// The fastest rate a modulation may be used at, given what was said.
    ///
    /// Table 2 has two rows that both come to V.27 ter: 0100 is the whole
    /// Recommendation, and 0000 is what the table calls its fall-back mode,
    /// which is 2400 bit/s and nothing else. Reading both as "V.27 ter" and
    /// then taking the fastest rate it has offers a machine 4800 that it has
    /// just finished saying it does not have.
    pub fn ceiling(&self, modulation: Modulation) -> u32 {
        match (modulation, self.rate_field) {
            (Modulation::V27ter, 0b0000) => 2400,
            _ => modulation.rates()[0],
        }
    }

    /// The fastest rate the two ends have in common.
    pub fn best_shared(&self, ours: &[Modulation]) -> Option<(Modulation, u32)> {
        let mut best: Option<(Modulation, u32)> = None;
        for m in &self.modulations {
            if !ours.contains(m) {
                continue;
            }
            let rate = self.ceiling(*m);
            if best.is_none_or(|(_, r)| rate > r) {
                best = Some((*m, rate));
            }
        }
        best
    }

    /// One line per thing worth showing on a panel.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let modulations = if self.modulations.is_empty() {
            "none this knows".to_owned()
        } else {
            self.modulations
                .iter()
                .map(|m| m.name())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let rates = self
            .modulations
            .iter()
            .flat_map(|m| m.rates())
            .max()
            .map_or_else(|| "-".to_owned(), |r| format!("{r} bit/s"));
        vec![
            ("receives", if self.receives { "yes" } else { "no" }.to_owned()),
            (
                "has a document",
                if self.can_be_polled { "yes, for polling" } else { "no" }.to_owned(),
            ),
            ("modulations", modulations),
            ("fastest", rates),
            (
                "resolution",
                if self.fine_resolution {
                    "3.85 and 7.7 lines/mm".to_owned()
                } else {
                    "3.85 lines/mm".to_owned()
                },
            ),
            (
                "coding",
                if self.t6_coding {
                    "MH, MR, MMR".to_owned()
                } else if self.two_dimensional {
                    "MH and MR".to_owned()
                } else {
                    "MH".to_owned()
                },
            ),
            (
                "paper",
                format!(
                    "{} mm wide, {}",
                    self.widths_mm
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join("/"),
                    self.length
                ),
            ),
            (
                "scan line",
                if self.scan_line_ms == 0.0 {
                    "no minimum".to_owned()
                } else {
                    format!("{:.0} ms minimum", self.scan_line_ms)
                },
            ),
            (
                "error correction",
                if self.error_correction { "yes" } else { "no" }.to_owned(),
            ),
        ]
    }
}

/// Read a CSI, CIG or TSI identification field (5.3.6.2.3).
///
/// Twenty characters, and they arrive with the last one first: 5.3.6.2.3 has
/// the field "transmitted in the reverse order", so the digits come off the
/// line backwards and the whole thing has to be turned round before it means
/// anything.
pub fn identification(fif: &[u8]) -> String {
    fif.iter()
        .rev()
        .map(|&c| if (32..127).contains(&c) { c as char } else { ' ' })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Set bit `n` of a parameter field being built, in Table 2 numbering.
///
/// Bit 1 is the first bit on the line, which is the least significant bit of
/// the first octet. The field grows to reach whatever bit is asked for.
pub fn set_bit(fif: &mut Vec<u8>, n: usize, on: bool) {
    let (octet, within) = ((n - 1) / 8, (n - 1) % 8);
    if fif.len() <= octet {
        fif.resize(octet + 1, 0);
    }
    if on {
        fif[octet] |= 1 << within;
    } else {
        fif[octet] &= !(1 << within);
    }
}

/// Set bits `from` to `to` from a value written the way Table 2 writes it:
/// first bit leftmost, which is the most significant bit of `value`.
pub fn set_field(fif: &mut Vec<u8>, from: usize, to: usize, value: u8) {
    let width = to - from + 1;
    for (i, n) in (from..=to).enumerate() {
        set_bit(fif, n, value >> (width - 1 - i) & 1 == 1);
    }
}

/// What this modem can receive, as the parameter field of a DIS.
///
/// Three octets, which is the shortest a DIS can be: bit 24 is the extension
/// bit and everything past it is optional, so leaving it clear says there is
/// nothing more to say. Error correction, T.6 coding and every later
/// extension live beyond it and are not offered, because they are not built.
pub fn our_capabilities(offer: &[Modulation], error_correction: bool) -> Vec<u8> {
    let mut fif = vec![0u8; 3];
    // Bit 10: this machine can receive a document. Bit 9 stays clear -- there
    // is nothing here for the far end to poll.
    set_bit(&mut fif, 10, true);
    // Bits 11 to 14, as Table 2 writes the rows that apply. Never the 0000
    // row, which is V.27 ter's fall-back alone and would cost half its rate.
    let rates = match (
        offer.contains(&Modulation::V27ter),
        offer.contains(&Modulation::V29),
    ) {
        (true, true) => 0b1100,
        (false, true) => 0b1000,
        // V.27 ter is what every group 3 machine must have, so it is also
        // what an empty offer comes to.
        _ => 0b0100,
    };
    set_field(&mut fif, 11, 14, rates);
    // Bit 15: 7.7 lines per millimetre as well as 3.85.
    set_bit(&mut fif, 15, true);
    // Bit 16: T.4 4.2's two-dimensional coding as well as the
    // one-dimensional.
    set_bit(&mut fif, 16, true);
    set_field(&mut fif, 17, 18, 0b00); // 215 mm across, and no wider.
    set_field(&mut fif, 19, 20, 0b01); // Any length: nothing here is paper.
    set_field(&mut fif, 21, 23, 0b111); // No minimum scan line time either.
    if error_correction {
        // Bit 24 says another octet follows, and bit 27 in it is error
        // correction mode. Bit 28 is "set to 0" in a DIS: the frame size is
        // the sender's choice, and A.1.3 has a receiver take either.
        set_bit(&mut fif, 24, true);
        set_bit(&mut fif, 27, true);
        // Bit 31: T.6's coding, which Note 9 makes meaningless without bit 27
        // and T.4 4.3 limits to error correction mode.
        set_bit(&mut fif, 31, true);
    }
    fif
}

/// The four bits of Table 2 that name one modulation and rate in a DCS.
///
/// The reverse of [`command_rate`], and the same table read the same way.
pub fn rate_field(modulation: Modulation, bits_per_second: u32) -> Option<u8> {
    use Modulation::{V17, V27ter, V29};
    Some(match (modulation, bits_per_second) {
        (V27ter, 2400) => 0b0000,
        (V27ter, 4800) => 0b0100,
        (V29, 9600) => 0b1000,
        (V29, 7200) => 0b1100,
        (V17, 14_400) => 0b0001,
        (V17, 12_000) => 0b0101,
        (V17, 9600) => 0b1001,
        (V17, 7200) => 0b1101,
        _ => return None,
    })
}

/// What a page is being sent as, for the parameter field of a DCS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub modulation: Modulation,
    pub bits_per_second: u32,
    /// 7.7 lines per millimetre rather than 3.85.
    pub fine: bool,
    /// Bits 21 to 23, as the receiver asked for them in its DIS.
    pub scan_line_field: u8,
    /// Bit 16 for Modified READ, bit 31 for MMR, and neither for Modified
    /// Huffman.
    pub coding: crate::coding::Coding,
    /// Bit 27: the page goes in frames under T.30 Annex A.
    pub error_correction: bool,
}

/// A DCS parameter field: one rate, one resolution, one page ahead.
///
/// A command, not a list. 5.3.6.2.2 makes bits 1, 4 and 9 zero in a DCS, and
/// bit 10 says the far end is to receive -- which is the whole point of
/// sending one.
pub fn command(command: Command) -> Vec<u8> {
    let mut fif = vec![0u8; 3];
    set_bit(&mut fif, 10, true);
    if let Some(rate) = rate_field(command.modulation, command.bits_per_second) {
        set_field(&mut fif, 11, 14, rate);
    }
    set_bit(&mut fif, 15, command.fine);
    // Bit 16 is T.4 4.2's coding, and an MMR page is not in it: T.6 says its
    // coding is "in principle the same", and a DCS says which of the two with
    // bit 31 alone.
    set_bit(&mut fif, 16, command.coding == crate::coding::Coding::ModifiedRead);
    set_field(&mut fif, 17, 18, 0b00);
    set_field(&mut fif, 19, 20, 0b01);
    // Whatever the receiver asked for, given back to it: this is the one
    // field of a DCS that is not the sender's choice -- except under error
    // correction mode, where Note 8 has the sender say "1, 1, 1" and send no
    // fill at all.
    let scan_line = if command.error_correction {
        0b111
    } else {
        command.scan_line_field & 0b111
    };
    set_field(&mut fif, 21, 23, scan_line);
    if command.error_correction {
        // Bit 28 clear: frames of 256 octets.
        set_bit(&mut fif, 24, true);
        set_bit(&mut fif, 27, true);
        // Bit 31, "T.6 coding enabled", which Note 17 allows only beside bit
        // 27. A page asked to go in MMR without error correction goes out
        // saying nothing of the kind, and T.4 4.3 would not have it go at all.
        set_bit(&mut fif, 31, command.coding == crate::coding::Coding::Mmr);
    }
    fif
}

/// An identification field: twenty characters, and the digits reversed.
///
/// 5.3.6.2.4 allows only digits, spaces and a plus, and the field is sent so
/// that it fills from the right. A machine given fewer than twenty characters
/// pads with spaces, and one given more keeps the last twenty.
pub fn identification_field(text: &str) -> Vec<u8> {
    let mut fif = vec![b' '; 20];
    let kept: Vec<u8> = text
        .bytes()
        .filter(|c| c.is_ascii_digit() || *c == b' ' || *c == b'+')
        .collect();
    for (slot, c) in fif.iter_mut().zip(kept.iter().rev()) {
        *slot = *c;
    }
    fif
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DIS off the line from a real fax machine.
    ///
    /// Recorded from a call to a public fax number, decoded from V.21
    /// channel 2 at 300 bit/s. Everything below is what that machine said
    /// about itself, and it is the only test here that is not this code
    /// checking its own arithmetic.
    const REAL_DIS: [u8; 4] = [0x00, 0x6e, 0xf8, 0x00];

    /// The CSI that came with it, which is the number that was dialled.
    const REAL_CSI: [u8; 20] = [
        0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x39, 0x30, 0x39, 0x20, 0x38,
        0x36, 0x33, 0x20, 0x20, 0x30, 0x30, 0x33, 0x31,
    ];

    #[test]
    fn bit_one_is_the_first_bit_on_the_line() {
        // Which is the least significant bit of the first octet. Reading an
        // octet the other way round is the mistake this exists to catch.
        assert!(bit(&[0x01], 1));
        assert!(!bit(&[0x01], 8));
        assert!(bit(&[0x80], 8));
        assert!(bit(&[0x00, 0x01], 9));
    }

    #[test]
    fn a_real_machine_says_what_it_said() {
        let caps = capabilities(&REAL_DIS);
        assert!(caps.receives, "it is a fax machine");
        assert!(!caps.can_be_polled, "it had nothing to be fetched");
        assert_eq!(caps.rate_field, 0b1101);
        assert_eq!(
            caps.modulations,
            vec![Modulation::V27ter, Modulation::V29, Modulation::V17]
        );
        assert!(caps.fine_resolution, "7.7 lines/mm as well as 3.85");
        assert!(!caps.two_dimensional, "one-dimensional coding only");
        assert_eq!(caps.widths_mm, vec![215]);
        assert_eq!(caps.length, "unlimited");
        assert_eq!(caps.scan_line_ms, 0.0, "as fast as they come");
        assert!(!caps.error_correction);
        assert_eq!(caps.octets, 4);
    }

    #[test]
    fn the_fastest_shared_rate_is_the_fastest_both_ends_have() {
        let caps = capabilities(&REAL_DIS);
        assert_eq!(
            caps.best_shared(&[Modulation::V27ter, Modulation::V29, Modulation::V17]),
            Some((Modulation::V17, 14_400))
        );
        // And with only the slowest pump written, the answer is the slowest.
        assert_eq!(
            caps.best_shared(&[Modulation::V27ter]),
            Some((Modulation::V27ter, 4800))
        );
        assert_eq!(caps.best_shared(&[]), None);
    }

    #[test]
    fn an_identification_field_is_backwards_on_the_line() {
        assert_eq!(identification(&REAL_CSI), "1300  368 909");
    }

    #[test]
    fn the_control_field_says_which_frame_and_not_which_end() {
        // 5.3.6.2.1 puts the originating end in the top bit, so a DIS from
        // one end and a DTC from the other come off the same underneath.
        assert_eq!(Frame::from_code(0x80), Frame::Dis, "DIS from the called end");
        assert_eq!(Frame::from_code(0x81), Frame::Dis, "DTC from the calling end");
        assert_eq!(Frame::from_code(0x40), Frame::Csi);
        assert_eq!(Frame::from_code(0x41), Frame::Csi, "CIG");
        assert_eq!(Frame::from_code(0x83), Frame::Dcs);
        assert_eq!(Frame::from_code(0xFB), Frame::Dcn);
        assert_eq!(Frame::from_code(0x8C), Frame::Mcf);
        assert_eq!(Frame::from_code(0x84), Frame::Cfr);
        assert_eq!(Frame::from_code(0x44), Frame::Ftt);
    }

    #[test]
    fn every_frame_survives_being_written_and_read_again() {
        for frame in [
            Frame::Nsf, Frame::Csi, Frame::Dis, Frame::Tsi, Frame::Dcs,
            Frame::Cfr, Frame::Ftt, Frame::Mcf, Frame::Rtp, Frame::Rtn,
            Frame::Eop, Frame::Eom, Frame::Mps, Frame::Dcn,
        ] {
            for from_caller in [false, true] {
                let code = frame.code(from_caller);
                assert_eq!(Frame::from_code(code), frame, "{code:02x}");
                assert_eq!(
                    code & 0x01 == 1,
                    from_caller,
                    "{frame:?} lost which end sent it"
                );
            }
        }
    }

    #[test]
    fn the_codes_are_the_ones_a_real_call_put_on_the_line() {
        // Read off a recording of a complete two-page transaction: the
        // answering end's identification and capabilities, the calling end's
        // identification and command, then the confirmations and the end.
        for (code, frame) in [
            (0x40u8, Frame::Csi), (0x80, Frame::Dis), (0x43, Frame::Tsi),
            (0x83, Frame::Dcs), (0x84, Frame::Cfr), (0x4F, Frame::Mps),
            (0x8C, Frame::Mcf), (0x2F, Frame::Eop), (0xFB, Frame::Dcn),
        ] {
            assert_eq!(Frame::from_code(code), frame, "{code:02x}");
            assert_eq!(frame.code(code & 1 == 1), code, "{frame:?}");
        }
    }

    #[test]
    fn a_missing_octet_reads_as_a_clear_bit_rather_than_a_panic() {
        // A DIS is as long as the far end chose to make it, and the extend
        // bits say where it stops. Asking about a bit past the end is an
        // ordinary thing to do.
        let caps = capabilities(&[0x00]);
        assert!(!caps.receives);
        assert!(!caps.error_correction);
        assert_eq!(caps.octets, 1);
    }

    #[test]
    fn a_command_frame_names_one_rate_and_not_a_set() {
        // Off a recording of a complete two-page call that the blog it came
        // from says ran at 14.4 kbit/s, and whose command frame is this.
        assert_eq!(
            command_rate(&[0x00, 0x62, 0x78]),
            Some((Modulation::V17, 14_400))
        );
        // The slowest there is, which every fax machine has.
        assert_eq!(command_rate(&[0x00, 0x00]), Some((Modulation::V27ter, 2400)));
        // And each of the four V.17 carries, which are the four this modem
        // has constellations for.
        for (bits, rate) in [(0b0001u8, 14_400), (0b0101, 12_000), (0b1001, 9600), (0b1101, 7200)] {
            // Bits 11 to 14 sit in the second octet, first bit lowest.
            let packed: u8 = (0..4).fold(0, |a, i| a | (((bits >> (3 - i)) & 1) << (i + 2)));
            assert_eq!(
                command_rate(&[0x00, packed]).map(|(_, r)| r),
                Some(rate),
                "field {bits:04b}"
            );
        }
    }

    #[test]
    fn every_rate_field_the_recommendation_names_gives_a_modulation() {
        for (code, want) in [
            (0b0000, 1),
            (0b0100, 1),
            (0b1000, 1),
            (0b1100, 2),
            (0b1101, 3),
        ] {
            assert_eq!(modulations_of(code).len(), want, "field {code:04b}");
        }
    }
}
