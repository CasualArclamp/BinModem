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
fn field(fif: &[u8], from: usize, to: usize) -> u8 {
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
    Some(match field(fif, 11, 14) {
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
    let rate_field = field(fif, 11, 14);
    let widths = match field(fif, 17, 18) {
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
        length: match field(fif, 19, 20) {
            0b00 => "A4, 297 mm",
            0b01 => "unlimited",
            0b10 => "A4 and B4, 364 mm",
            _ => "invalid",
        },
        scan_line_ms: scan_line_ms(field(fif, 21, 23)),
        error_correction: bit(fif, 27),
        t6_coding: bit(fif, 31),
        octets: fif.len(),
    }
}

impl Capabilities {
    /// The fastest rate the two ends have in common.
    pub fn best_shared(&self, ours: &[Modulation]) -> Option<(Modulation, u32)> {
        let mut best: Option<(Modulation, u32)> = None;
        for m in &self.modulations {
            if !ours.contains(m) {
                continue;
            }
            let rate = m.rates()[0];
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
