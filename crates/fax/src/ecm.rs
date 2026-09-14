//! Error correction mode: T.4 Annex A's frames and T.30 Annex A's counters.
//!
//! Without it a page is one long bit stream, and a burst of noise in the
//! middle of it is a few spoiled lines that nobody can ask for again. With it
//! the page is cut into numbered frames of 256 octets, each with its own frame
//! check, up to 256 frames to a block; after each block the receiving end says
//! which frames failed, and only those are sent again. "Half-duplex page
//! selective repeat ARQ", A.1.3 calls it.
//!
//! It matters here for a second reason. Every coding scheme newer than T.4's
//! own two -- T.6, JBIG, JPEG, and the rest of Note 17 of Table 2 -- is only
//! allowed with it, because none of them can find its place again after an
//! error the way an end-of-line code lets T.4 do.
//!
//! This module is the frames and fields. Which of them to send, and when, is
//! the procedure's business.

use ec::hdlc::{self, Fcs};

use crate::t30::{ADDRESS, CONTROL_MORE};

/// A.3.2 of T.30: the two frame sizes, in octets of page data.
pub const FRAME_OCTETS: usize = 256;
pub const SMALL_FRAME_OCTETS: usize = 64;

/// A.3.3: "block size: 256 frames".
pub const BLOCK_FRAMES: usize = 256;

/// A.3.5 of T.4: "FCF for the FCD frame. Format: 0110 0000", first bit on the
/// left, which is the least significant bit of the octet.
pub const FCD: u8 = 0x06;
/// "FCF for the RCP frame. Format: 0110 0001".
pub const RCP: u8 = 0x86;

/// A.3.8 of T.4: "three consecutive RCP frames" end a partial page.
pub const RCP_FRAMES: usize = 3;

/// A.3.1 of T.4: after the training, "a series of flag sequences for nominal
/// 200 ms".
pub const SYNCHRONIZATION_SECONDS: f64 = 0.2;

/// A.1.3 of T.30: "When PPR is received four times for the same block, either
/// the EOR command is transmitted ... or CTC".
pub const PPRS_BEFORE_GIVING_WAY: u32 = 4;

/// The post-message command inside a PPS or EOR: FCF2 of Figure A.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostMessage {
    /// "NULL code which indicates the partial page boundary": more of this page
    /// to come.
    Null,
    /// End of message: another page follows, after a return to phase B.
    Eom,
    /// Multi-page signal: another page follows at once.
    Mps,
    /// End of procedure: that was the last page.
    Eop,
}

impl PostMessage {
    /// Figure A.1's codes, first bit on the left: 0000 0000, 1111 0001,
    /// 1111 0010 and 1111 0100.
    pub fn code(self) -> u8 {
        match self {
            Self::Null => 0x00,
            Self::Eom => 0x8F,
            Self::Mps => 0x4F,
            Self::Eop => 0x2F,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0x00 => Self::Null,
            0x8F => Self::Eom,
            0x4F => Self::Mps,
            0x2F => Self::Eop,
            _ => return None,
        })
    }
}

/// A page's bits as octets for frames: first bit of the page as the least
/// significant bit of the first octet, padded at the end with zeros.
///
/// Least significant first because that is the order HDLC puts an octet on
/// the line in, and the order of the coded bits on the line is the thing that
/// has to survive: T.4 A.3 has fields "transmitted ... from left to right as
/// printed", and a page's code words are printed left to right like anything
/// else. The zeros at the end are A.3.6.2's pad bits, which a receiver
/// "is able to receive".
pub fn pack(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |octet, (i, &bit)| octet | u8::from(bit) << i)
        })
        .collect()
}

/// The reverse of [`pack`].
pub fn unpack(octets: &[u8]) -> Vec<bool> {
    octets
        .iter()
        .flat_map(|&octet| (0..8).map(move |i| octet >> i & 1 == 1))
        .collect()
}

/// Cut a page into frames of `size` octets. The last is as long as what is
/// left: "the facsimile data field length of the final frame ... may be less
/// than 256 or 64 octets" (T.4 A.3.6.2, Note 2).
pub fn frames(octets: &[u8], size: usize) -> Vec<Vec<u8>> {
    octets.chunks(size.max(1)).map(<[u8]>::to_vec).collect()
}

/// An FCD frame's HDLC information field: address, control, FCF, frame number
/// and the data (T.4 A.3.3 to A.3.6).
///
/// The control field is "1100 X000" with X set to 0 for both kinds of frame,
/// which is the same octet as a T.30 frame with more to follow.
pub fn fcd_frame(number: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&[ADDRESS, CONTROL_MORE, FCD, number]);
    out.extend_from_slice(data);
    out
}

/// An RCP frame's information field, which has no FIF at all (T.4 A.3.6 Note 2).
pub fn rcp_frame() -> Vec<u8> {
    vec![ADDRESS, CONTROL_MORE, RCP]
}

/// The bits of one partial page at the high speed: synchronization flags, the
/// frames, three RCP frames, and a flag to close.
///
/// `frames` is each frame's number and data, in the order they are to go. A
/// block goes in order from frame 0; a retransmission is whichever frames were
/// asked for, still in order.
pub fn partial_page(frames: &[(u8, &[u8])], bits_per_second: u32) -> Vec<bool> {
    let mut encoder = hdlc::Encoder::new(Fcs::Bits16);
    let flags = (SYNCHRONIZATION_SECONDS * f64::from(bits_per_second) / 8.0).ceil() as usize;
    encoder.idle(flags);
    for &(number, data) in frames {
        encoder.frame(&fcd_frame(number, data));
    }
    for _ in 0..RCP_FRAMES {
        encoder.frame(&rcp_frame());
    }
    // "The flag sequence following the last RCP frame shall be less than
    // 50 ms" (A.3.8): one is enough to close it.
    encoder.idle(1);
    std::iter::from_fn(|| encoder.next_bit()).collect()
}

/// A PPS's FIF: FCF2, then I1 to I3 of Figure A.1 -- the page counter, the
/// block counter, and one less than the number of frames.
///
/// Each counter goes least significant bit first (Note 5), which is an
/// ordinary octet here.
pub fn pps_field(command: PostMessage, page: u8, block: u8, frames: usize) -> Vec<u8> {
    let count = frames.clamp(1, BLOCK_FRAMES) - 1;
    vec![command.code(), page, block, count as u8]
}

/// What a PPS's FIF says: the command, the page, the block, and how many
/// frames.
pub fn read_pps(fif: &[u8]) -> Option<(PostMessage, u8, u8, usize)> {
    let [code, page, block, count, ..] = *fif else {
        return None;
    };
    Some((PostMessage::from_code(code)?, page, block, usize::from(count) + 1))
}

/// A PPR's FIF: 256 bits, one to a frame, set for every frame wanted again.
///
/// A.4.4: "the first bit to the first frame", and Note 1: bits past the last
/// frame of a short block "are set to '1'". The first bit on the line is the
/// least significant bit of the first octet.
pub fn ppr_field(frames: usize, have: impl Fn(usize) -> bool) -> Vec<u8> {
    let mut fif = vec![0u8; BLOCK_FRAMES / 8];
    for i in 0..BLOCK_FRAMES {
        if i >= frames || !have(i) {
            fif[i / 8] |= 1 << (i % 8);
        }
    }
    fif
}

/// The frames a PPR asks for, of a block of `frames`.
pub fn read_ppr(fif: &[u8], frames: usize) -> Vec<usize> {
    (0..frames.min(BLOCK_FRAMES))
        .filter(|&i| fif.get(i / 8).is_some_and(|octet| octet >> (i % 8) & 1 == 1))
        .collect()
}

/// The receiving end's half: frames collected off the high-speed carrier.
#[derive(Debug)]
pub struct Collector {
    decoder: hdlc::Decoder,
    frames: Vec<Option<Vec<u8>>>,
    rcps: usize,
    /// Frames that failed their check, which is what the PPR will ask for.
    pub bad: usize,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        Self {
            // The largest FCD frame is 262 octets: address, control, FCF, the
            // frame number, 256 of data and two of check.
            decoder: hdlc::Decoder::new(Fcs::Bits16).with_max_octets(300),
            frames: vec![None; BLOCK_FRAMES],
            rcps: 0,
            bad: 0,
        }
    }

    pub fn feed_bits(&mut self, bits: &[bool]) {
        for &bit in bits {
            match self.decoder.feed(bit) {
                Some(Ok(octets)) => self.frame(&octets),
                Some(Err(_)) => self.bad += 1,
                None => {}
            }
        }
    }

    fn frame(&mut self, octets: &[u8]) {
        let [address, control, fcf, rest @ ..] = octets else {
            return;
        };
        if *address != ADDRESS || *control != CONTROL_MORE {
            return;
        }
        match *fcf {
            FCD => {
                let [number, data @ ..] = rest else { return };
                // A frame that arrives twice is the same frame twice: a
                // retransmission of one that was already good changes nothing.
                self.frames[usize::from(*number)] = Some(data.to_vec());
            }
            RCP => self.rcps += 1,
            _ => {}
        }
    }

    /// Whether the partial page has ended: any one of the three RCP frames is
    /// enough to know it has.
    pub fn ended(&self) -> bool {
        self.rcps > 0
    }

    /// Whether frame `number` has arrived intact.
    pub fn has(&self, number: usize) -> bool {
        self.frames.get(number).is_some_and(Option::is_some)
    }

    /// The frames of the block in hand that have arrived without a gap,
    /// counting from the first.
    pub fn leading(&self) -> impl Iterator<Item = &[u8]> {
        self.frames.iter().map_while(|f| f.as_deref())
    }

    /// How many frames have arrived intact.
    pub fn count(&self) -> usize {
        self.frames.iter().filter(|f| f.is_some()).count()
    }

    /// Whether every frame of a block of `frames` is here.
    pub fn complete(&self, frames: usize) -> bool {
        (0..frames.min(BLOCK_FRAMES)).all(|i| self.has(i))
    }

    /// Get ready for the next partial page of the same block: what has arrived
    /// is kept, and only the RCP count starts again.
    pub fn next_partial_page(&mut self) {
        self.rcps = 0;
    }

    /// Hand over the block's data in frame order, and start a new block.
    ///
    /// Frames that never arrived are left out, which is what happens at the
    /// end of retransmission: whatever could not be had is gone, and the
    /// coding's own resynchronization makes what it can of the rest.
    pub fn take_block(&mut self, frames: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for slot in self.frames.iter_mut().take(frames.min(BLOCK_FRAMES)) {
            if let Some(data) = slot.take() {
                out.extend_from_slice(&data);
            }
        }
        self.frames.fill(None);
        self.rcps = 0;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_codes_are_the_bits_the_annex_prints() {
        // T.4 A.3.5, first bit on the left.
        let printed = |octet: u8| -> String { (0..8).map(|i| if octet >> i & 1 == 1 { '1' } else { '0' }).collect() };
        assert_eq!(printed(FCD), "01100000");
        assert_eq!(printed(RCP), "01100001");
        // A.3.4's control field, 1100 X000 with X at 0.
        assert_eq!(printed(CONTROL_MORE), "11000000");
        assert_eq!(printed(PostMessage::Null.code()), "00000000");
        assert_eq!(printed(PostMessage::Eom.code()), "11110001");
        assert_eq!(printed(PostMessage::Mps.code()), "11110010");
        assert_eq!(printed(PostMessage::Eop.code()), "11110100");
    }

    #[test]
    fn figure_a1s_example_counters_go_out_as_it_draws_them() {
        // PC = 1, BC = 2, FC = 10, and the bits "transmit left to right":
        // 1000 0000, 0100 0000, 0101 0000.
        let fif = pps_field(PostMessage::Null, 1, 2, 11);
        let line: String = unpack(&fif[1..])
            .iter()
            .map(|&b| if b { '1' } else { '0' })
            .collect();
        assert_eq!(line, "100000000100000001010000");
        assert_eq!(read_pps(&fif), Some((PostMessage::Null, 1, 2, 11)));
    }

    #[test]
    fn a_ppr_asks_for_what_did_not_arrive_and_everything_past_the_block() {
        // Figure A.5: frames past the last of a short block are set.
        let fif = ppr_field(6, |i| i != 1 && i != 4);
        assert_eq!(fif.len(), 32, "256 bits");
        let bits = unpack(&fif);
        assert_eq!(bits[..6], [false, true, false, false, true, false]);
        assert!(bits[6..].iter().all(|&b| b), "extra bits are set");
        assert_eq!(read_ppr(&fif, 6), vec![1, 4]);
    }

    #[test]
    fn packing_keeps_the_first_bit_first_on_the_line() {
        let bits: Vec<bool> = (0..37).map(|i| i % 3 == 0 || i % 7 == 1).collect();
        let octets = pack(&bits);
        assert_eq!(octets.len(), 5);
        assert_eq!(unpack(&octets)[..37], bits[..]);
        // And through HDLC, which sends each octet low bit first, the page's
        // own bits come out in their own order.
        let mut encoder = hdlc::Encoder::new(Fcs::Bits16);
        encoder.frame(&octets);
        let mut decoder = hdlc::Decoder::new(Fcs::Bits16);
        let mut got = None;
        while let Some(bit) = encoder.next_bit() {
            if let Some(Ok(frame)) = decoder.feed(bit) {
                got = Some(frame);
            }
        }
        assert_eq!(unpack(&got.expect("no frame"))[..37], bits[..]);
    }

    #[test]
    fn a_partial_page_is_collected_whole() {
        let data: Vec<Vec<u8>> = (0..5u8).map(|n| vec![n.wrapping_mul(37); 256]).collect();
        let frames: Vec<(u8, &[u8])> = data.iter().enumerate().map(|(i, d)| (i as u8, d.as_slice())).collect();
        let bits = partial_page(&frames, 9600);
        let mut collector = Collector::new();
        collector.feed_bits(&bits);
        assert!(collector.ended(), "no RCP");
        assert!(collector.complete(5));
        let block = collector.take_block(5);
        assert_eq!(block, data.concat());
    }

    #[test]
    fn a_damaged_frame_is_missing_and_the_rest_are_not() {
        let data: Vec<Vec<u8>> = (0..4u8).map(|n| vec![n; 64]).collect();
        let frames: Vec<(u8, &[u8])> = data.iter().enumerate().map(|(i, d)| (i as u8, d.as_slice())).collect();
        let mut bits = partial_page(&frames, 4800);
        // Somewhere in the middle of the frames, well past the flags.
        let at = bits.len() / 2;
        for bit in &mut bits[at..at + 5] {
            *bit = !*bit;
        }
        let mut collector = Collector::new();
        collector.feed_bits(&bits);
        assert!(collector.ended());
        assert!(!collector.complete(4), "the damage went unnoticed");
        let missing: Vec<usize> = (0..4).filter(|&i| !collector.has(i)).collect();
        assert_eq!(missing.len(), 1, "one burst of errors, one frame: {missing:?}");
        // Asked for again and sent again, the block is whole.
        let again: Vec<(u8, &[u8])> = missing.iter().map(|&i| (i as u8, data[i].as_slice())).collect();
        collector.next_partial_page();
        collector.feed_bits(&partial_page(&again, 4800));
        assert!(collector.complete(4));
        assert_eq!(collector.take_block(4), data.concat());
    }

    #[test]
    fn the_synchronization_is_two_hundred_milliseconds_of_flags() {
        let bits = partial_page(&[], 9600);
        // 0.2 s at 9600 is 1920 bits, 240 flags; then three RCP frames and one
        // closing flag, which are a few dozen more.
        let leading_flags = bits.chunks(8).take_while(|c| *c == [false, true, true, true, true, true, true, false]).count();
        assert!((240..=242).contains(&leading_flags), "{leading_flags} flags");
    }
}
