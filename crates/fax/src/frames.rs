//! T.30's frames on the wire: HDLC over V.21 channel 2.
//!
//! The framing is the same HDLC that carries V.42, so nothing here builds a
//! frame check or stuffs a bit -- [`ec::hdlc`] already does both. What is
//! here is the shape T.30 puts inside one, and the preamble it insists on
//! either side.

use ec::hdlc::{self, Fcs};

use crate::t30::{ADDRESS, CONTROL_FINAL, CONTROL_MORE, Frame};

/// One frame, as it goes out or as it came in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub frame: Frame,
    /// Whether this is the last of a burst (5.3.6.1: the control field is
    /// 0x13 for the final frame and 0x03 for one with more behind it).
    pub last: bool,
    /// The X bit of the control field: set by whichever end received the
    /// capabilities, which on an ordinary call is the one that dialled.
    pub from_caller: bool,
    pub fif: Vec<u8>,
}

impl Message {
    pub fn new(frame: Frame, from_caller: bool) -> Self {
        Self { frame, last: true, from_caller, fif: Vec::new() }
    }

    pub fn with_fif(mut self, fif: &[u8]) -> Self {
        self.fif = fif.to_vec();
        self
    }

    /// More frames follow this one in the same burst.
    pub fn and_more(mut self) -> Self {
        self.last = false;
        self
    }

    /// The information field: address, control, function, parameters.
    ///
    /// The frame check is not here. HDLC appends it, and putting it in twice
    /// is a frame every fax on earth discards.
    pub fn octets(&self) -> Vec<u8> {
        let mut out = vec![
            ADDRESS,
            if self.last { CONTROL_FINAL } else { CONTROL_MORE },
            self.frame.code(self.from_caller),
        ];
        out.extend_from_slice(&self.fif);
        out
    }

    /// Read one back, from a frame HDLC has already checked.
    ///
    /// The frame check is gone by then, so what arrives is address, control,
    /// function and parameters and nothing else.
    pub fn parse(octets: &[u8]) -> Option<Self> {
        if octets.len() < 3 || octets[0] != ADDRESS {
            return None;
        }
        Some(Self {
            frame: Frame::from_code(octets[2]),
            last: octets[1] & 0x10 != 0,
            from_caller: octets[2] & 0x01 != 0,
            fif: octets[3..].to_vec(),
        })
    }
}

/// Flags before a burst: "a series of flag sequences for 1 s +/- 15%"
/// (5.3.5).
///
/// Not decoration. It is there so that everything between the two machines --
/// echo suppressors, a network's own gain control, the far end's receiver --
/// has settled before any bit that matters arrives. A fax that starts
/// straight in on its first frame is a fax whose first frame is not read.
pub const PREAMBLE_SECONDS: f64 = 1.0;
/// Flags in that second at 300 bit/s: eight bits each.
pub const PREAMBLE_FLAGS: usize = 37;

/// Builds the bit stream for a burst of frames.
#[derive(Debug)]
pub struct Sender {
    encoder: hdlc::Encoder,
}

impl Default for Sender {
    fn default() -> Self {
        Self::new()
    }
}

impl Sender {
    pub fn new() -> Self {
        Self { encoder: hdlc::Encoder::new(Fcs::Bits16) }
    }

    /// Queue a whole burst: the preamble, then every frame, then the flags
    /// that close it.
    pub fn send(&mut self, messages: &[Message]) {
        self.encoder.idle(PREAMBLE_FLAGS);
        for m in messages {
            self.encoder.frame(&m.octets());
        }
        // 5.3.5 again: the carrier stays up for a moment after the last
        // frame, so the far end sees the closing flag before the silence.
        self.encoder.idle(2);
    }

    pub fn next_bit(&mut self) -> Option<bool> {
        self.encoder.next_bit()
    }

    pub fn pending_bits(&self) -> usize {
        self.encoder.pending_bits()
    }

    pub fn is_empty(&self) -> bool {
        self.encoder.is_empty()
    }
}

/// Reads frames out of a bit stream.
#[derive(Debug)]
pub struct Reader {
    decoder: hdlc::Decoder,
    /// Frames that failed their check, kept because a fax that cannot be read
    /// is exactly when somebody wants to see what arrived.
    pub bad: usize,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    pub fn new() -> Self {
        // A T.30 frame is tens of octets, never hundreds: the longest
        // ordinary one is a 20-character identification.
        Self { decoder: hdlc::Decoder::new(Fcs::Bits16).with_max_octets(256), bad: 0 }
    }

    /// Feed one bit; yields a frame when one completes and checks out.
    pub fn feed(&mut self, bit: bool) -> Option<Message> {
        match self.decoder.feed(bit)? {
            Ok(octets) => Message::parse(&octets),
            Err(_) => {
                self.bad += 1;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t30;

    /// The capability frame of a real machine, and the identification that
    /// came with it, off a recording of a call to a public fax number.
    const DIS: [u8; 4] = [0x00, 0x6e, 0xf8, 0x00];

    #[test]
    fn a_frame_is_address_control_function_and_parameters() {
        let m = Message::new(Frame::Dis, false).with_fif(&DIS);
        assert_eq!(
            m.octets(),
            vec![0xFF, 0x13, 0x80, 0x00, 0x6e, 0xf8, 0x00],
            "the capability frame as a real machine sends it"
        );
    }

    #[test]
    fn a_frame_with_more_behind_it_says_so_in_the_control_octet() {
        let m = Message::new(Frame::Csi, false).and_more();
        assert_eq!(m.octets()[1], 0x03);
        assert_eq!(Message::new(Frame::Csi, false).octets()[1], 0x13);
    }

    #[test]
    fn the_x_bit_says_which_end_is_speaking() {
        // The capabilities go out from the end that answered, so X is clear;
        // the command that replies comes from the end that dialled.
        assert_eq!(Message::new(Frame::Dis, false).octets()[2], 0x80);
        assert_eq!(Message::new(Frame::Dcs, true).octets()[2], 0x83);
    }

    #[test]
    fn what_goes_out_comes_back() {
        let sent = vec![
            Message::new(Frame::Csi, false)
                .and_more()
                .with_fif(b"       909 863  0031"),
            Message::new(Frame::Dis, false).with_fif(&DIS),
        ];
        let mut tx = Sender::new();
        tx.send(&sent);
        let mut rx = Reader::new();
        let mut got = Vec::new();
        while let Some(bit) = tx.next_bit() {
            if let Some(m) = rx.feed(bit) {
                got.push(m);
            }
        }
        assert_eq!(got, sent);
        assert_eq!(rx.bad, 0);
    }

    #[test]
    fn the_burst_begins_with_a_second_of_flags() {
        let mut tx = Sender::new();
        tx.send(&[Message::new(Frame::Dis, false).with_fif(&DIS)]);
        let mut bits = Vec::new();
        while let Some(b) = tx.next_bit() {
            bits.push(b);
        }
        let seconds = bits.len() as f64 / 300.0;
        assert!(
            seconds > 1.0 && seconds < 1.4,
            "a burst of one frame took {seconds:.2} s, and the preamble alone \
             is meant to be a second"
        );
    }

    #[test]
    fn a_real_capability_frame_reads_back_as_itself() {
        let m = Message::parse(&[0xFF, 0x13, 0x80, 0x00, 0x6e, 0xf8, 0x00])
            .expect("a frame");
        assert_eq!(m.frame, Frame::Dis);
        assert!(m.last);
        assert!(!m.from_caller);
        let caps = t30::capabilities(&m.fif);
        assert_eq!(
            caps.modulations,
            vec![t30::Modulation::V27ter, t30::Modulation::V29, t30::Modulation::V17]
        );
    }

    #[test]
    fn a_frame_addressed_to_anything_else_is_not_a_fax_frame() {
        assert_eq!(Message::parse(&[0x03, 0x13, 0x80]), None);
        assert_eq!(Message::parse(&[0xFF, 0x13]), None, "too short");
    }
}
