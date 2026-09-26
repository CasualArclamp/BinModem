//! The decoding half: a BIE in, a bit or a byte at a time, and lines out as
//! each is complete.

use super::arith::{self, Contexts, Scd};
use super::{
    ABORT, ATMOVE, COMMENT, CONTEXTS, DP_TABLE, ESC, Header, LEFT, MAX_LINES, NEWLEN, Rows, SDNORM,
    SDRST, STUFF, slntp_context,
};

/// Unread bytes a pel is not decoded without, while a stripe's end is not yet
/// in.
///
/// A stripe's data stops short of its end: the zeros at the end of its SCD
/// were taken off (6.8.2.10), and a decoder that reaches the end reads zeros
/// in their place (6.8.3.8). So until the ESC SDNORM arrives, the last bytes
/// that have arrived might be followed by more data or by those zeros, and a
/// pel that needs a byte past them cannot be decoded yet. One decision
/// renormalizes at most fifteen places -- A never falls below 1 -- and so
/// reads at most two bytes; four is room to spare.
const MARGIN: usize = 4;

/// Where the stream has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The twenty bytes of the BIH, arriving.
    Header,
    /// A private DP table the BIH said follows it, being stepped over: T.85
    /// never sends one, but it is part of a BIH when announced (6.2.2).
    Table(usize),
    /// Stripes and floating marker segments.
    Data,
    /// An ESC ABORT (Table 13): the sender has stopped the page.
    Aborted,
    /// Something nothing after can be read past.
    Spoiled,
}

/// A floating marker segment whose parameters are still arriving (6.2.6).
#[derive(Debug, Clone)]
enum Segment {
    /// yAT, τX and τY: six bytes.
    AtMove(Vec<u8>),
    /// The new YD: four bytes.
    NewLength(Vec<u8>),
    /// Lc, four bytes, and then that many bytes of whatever the far end
    /// wanted to say (T.85 4.4 leaves the meaning to the two ends).
    CommentLength(Vec<u8>),
    Comment(u64),
}

/// A BIE going the other way, as it arrives.
///
/// Shaped like the other decoders of the crate -- bits in, the lines so far
/// out -- and like them it keeps up: a pel is decoded as soon as the bytes it
/// needs are here, so a page can be watched arriving. Lines come out as wide
/// as the decoder was made for, cut or filled with white if the header says
/// otherwise.
#[derive(Debug, Clone)]
pub struct Decoder {
    width: usize,
    /// Bits of the octet arriving, least significant first (T.85 clause 3).
    octet: u8,
    bits: u8,
    stage: Stage,
    bih: Vec<u8>,
    header: Option<Header>,
    /// The last byte was ESC, and the next says what it was.
    escaped: bool,
    segment: Option<Segment>,

    /// The stripe in hand: its data, unstuffed; whether any of it, or its
    /// end, has arrived; whether the end has; whether that was an SDRST; and
    /// whether its last lines are being held for a NEWLEN (see `stripe_ends`).
    scd: Scd,
    open: bool,
    ended: bool,
    reset: bool,
    held: bool,
    coder: Option<arith::Decoder>,
    contexts: Contexts,
    slntp: usize,
    /// The line of the page the stripe in hand starts at.
    stripe_start: u64,
    /// ATMOVEs for the stripe in hand, and for the next: yAT and τX.
    moves: Vec<(u32, u8)>,
    pending: Vec<(u32, u8)>,
    /// Where the AT pixel is, as τX.
    at: u8,

    rows: Rows,
    lntp_above: bool,
    /// The line in hand has been started, and how far along it has got.
    line_open: bool,
    x: usize,
    /// YD, from the header or a NEWLEN.
    height: u64,
    lines: Vec<Vec<bool>>,
}

impl Decoder {
    /// A decoder for lines of `width` pels.
    pub fn new(width: usize) -> Self {
        Self {
            width,
            octet: 0,
            bits: 0,
            stage: Stage::Header,
            bih: Vec::with_capacity(Header::LEN),
            header: None,
            escaped: false,
            segment: None,
            scd: Scd::default(),
            open: false,
            ended: false,
            reset: false,
            held: false,
            coder: None,
            contexts: Contexts::new(CONTEXTS),
            slntp: 0,
            stripe_start: 0,
            moves: Vec::new(),
            pending: Vec::new(),
            at: 0,
            rows: Rows::new(0),
            lntp_above: true,
            line_open: false,
            x: 0,
            height: 0,
            lines: Vec::new(),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// The header, once it has arrived.
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// The lines decoded so far.
    ///
    /// Almost always only ever longer. The exception is a NEWLEN that ends
    /// the page part way through a stripe already decoded -- T.85 Amendment 1
    /// shows a sender doing exactly that -- when the lines past the new end,
    /// which were never the sender's, go again.
    pub fn lines(&self) -> &[Vec<bool>] {
        &self.lines
    }

    /// Whether the page is complete: every line the header or a NEWLEN says
    /// it has, and the end of the stripe it ends in, or an ABORT.
    pub fn is_done(&self) -> bool {
        match self.stage {
            Stage::Aborted => true,
            Stage::Data => self.complete() && !(self.open && !self.ended),
            _ => false,
        }
    }

    /// Whether the page stopped short of what it said it was: one if it did,
    /// and every line after that point is lost, and none if not.
    pub fn damaged(&self) -> usize {
        usize::from(matches!(self.stage, Stage::Spoiled | Stage::Aborted))
    }

    /// Bits as they come off the line: each octet least significant bit first
    /// (T.85 clause 3), which is also the order the frames of error
    /// correction mode carry them in.
    pub fn feed_bits(&mut self, bits: &[bool]) {
        for &bit in bits {
            self.octet |= u8::from(bit) << self.bits;
            self.bits += 1;
            if self.bits == 8 {
                let octet = std::mem::take(&mut self.octet);
                self.bits = 0;
                self.feed_byte(octet);
            }
        }
    }

    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    fn feed_byte(&mut self, byte: u8) {
        match self.stage {
            Stage::Header => {
                self.bih.push(byte);
                if self.bih.len() == Header::LEN {
                    self.start();
                }
            }
            Stage::Table(left) => {
                self.stage = if left > 1 { Stage::Table(left - 1) } else { Stage::Data };
            }
            Stage::Data => self.data(byte),
            Stage::Aborted | Stage::Spoiled => {}
        }
    }

    /// The header is in: check it, and get ready for the stripes.
    fn start(&mut self) {
        let bytes: &[u8; Header::LEN] = self.bih[..].try_into().expect("twenty bytes");
        match Header::read(bytes) {
            Ok((header, table)) => {
                self.rows = Rows::new(header.width as usize);
                self.slntp = slntp_context(header.two_line);
                self.height = u64::from(header.height);
                self.header = Some(header);
                self.stage = if table { Stage::Table(DP_TABLE) } else { Stage::Data };
            }
            Err(_) => self.stage = Stage::Spoiled,
        }
    }

    fn header_of(&self) -> Header {
        self.header.expect("the stripes come after the header")
    }

    /// Whether every line the page has is here.
    fn complete(&self) -> bool {
        self.lines.len() as u64 >= self.height
    }

    /// One byte of the BID: marker, marker parameter, or stripe data.
    fn data(&mut self, byte: u8) {
        if let Some(segment) = self.segment.take() {
            self.segment_byte(segment, byte);
            return;
        }
        if self.escaped {
            self.escaped = false;
            match byte {
                STUFF => self.stripe_byte(ESC),
                SDNORM => self.stripe_ends(false),
                SDRST => self.stripe_ends(true),
                ABORT => self.stage = Stage::Aborted,
                ATMOVE => self.segment = Some(Segment::AtMove(Vec::new())),
                NEWLEN => self.segment = Some(Segment::NewLength(Vec::new())),
                COMMENT => self.segment = Some(Segment::CommentLength(Vec::new())),
                // RESERVE "shall never appear in a public datastream" (6.2.7),
                // and nothing else is a marker at all.
                _ => self.stage = Stage::Spoiled,
            }
            return;
        }
        if byte == ESC {
            self.escaped = true;
        } else {
            self.stripe_byte(byte);
        }
    }

    fn segment_byte(&mut self, segment: Segment, byte: u8) {
        match segment {
            Segment::AtMove(mut got) => {
                got.push(byte);
                if got.len() < 6 {
                    self.segment = Some(Segment::AtMove(got));
                } else {
                    self.at_move(&got);
                }
            }
            Segment::NewLength(mut got) => {
                got.push(byte);
                if got.len() < 4 {
                    self.segment = Some(Segment::NewLength(got));
                } else {
                    self.new_height(u32::from_be_bytes([got[0], got[1], got[2], got[3]]));
                }
            }
            Segment::CommentLength(mut got) => {
                got.push(byte);
                if got.len() < 4 {
                    self.segment = Some(Segment::CommentLength(got));
                } else {
                    let length = u32::from_be_bytes([got[0], got[1], got[2], got[3]]);
                    if length > 0 {
                        self.segment = Some(Segment::Comment(u64::from(length)));
                    }
                }
            }
            Segment::Comment(left) => {
                if left > 1 {
                    self.segment = Some(Segment::Comment(left - 1));
                }
            }
        }
    }

    /// An ATMOVE (6.2.6.1): for the first SDE after it, at line yAT of it.
    ///
    /// T.85 fixes MY at 0, so τY is 0, and τX is then either 0 for the
    /// default place or a pel already decoded to the left. Anything else is a
    /// template this cannot build -- a pel from the future, or from a line
    /// T.85 does not let it reach -- and the page cannot go on.
    fn at_move(&mut self, got: &[u8]) {
        let line = u32::from_be_bytes([got[0], got[1], got[2], got[3]]);
        let (tx, ty) = (got[4] as i8, got[5]);
        if ty != 0 || tx < 0 {
            self.stage = Stage::Spoiled;
            return;
        }
        self.pending.push((line, tx as u8));
    }

    /// A NEWLEN (6.2.6.2): the page is this long after all.
    ///
    /// "The new YD shall never be greater than the original", and one that is
    /// is not believed. One that is less than what has been decoded ends the
    /// page there: that is the case of a length the sender learned only after
    /// sending the stripe it falls in, which 6.2.6.2 allows and T.85
    /// Amendment 1 shows. And a stripe held for want of one can be finished.
    fn new_height(&mut self, height: u32) {
        let original = self.header_of().height;
        if height > original {
            return;
        }
        self.height = u64::from(height);
        if self.lines.len() as u64 > self.height {
            self.lines.truncate(height as usize);
        }
        if self.held && self.left() <= MAX_LINES as u64 {
            self.finish_stripe();
        }
    }

    /// Lines of the stripe in hand still to come, as the header and any
    /// NEWLEN have it.
    fn left(&self) -> u64 {
        let end = (self.stripe_start + u64::from(self.header_of().stripe)).min(self.height);
        end.saturating_sub(self.lines.len() as u64)
    }

    /// A byte of a stripe's PSCD, unstuffed.
    fn stripe_byte(&mut self, byte: u8) {
        if self.complete() && !self.open {
            // After the last line, and not part of a stripe that is still
            // arriving: the zeros T.4 A.3.6.2 lets pad out the last frame, or
            // the data of a stripe the page has no lines left for.
            return;
        }
        if self.held {
            // A stripe after one that never ended: see `stripe_ends`.
            self.stage = Stage::Spoiled;
            return;
        }
        if !self.open {
            self.open_stripe();
        }
        self.scd.bytes.push(byte);
        self.run();
    }

    fn open_stripe(&mut self) {
        self.open = true;
        self.ended = false;
        self.reset = false;
        self.coder = None;
        self.scd = Scd::default();
        self.moves = std::mem::take(&mut self.pending);
    }

    /// ESC SDNORM or ESC SDRST: the stripe in hand has all its data.
    ///
    /// Its last lines can be decoded now, the SCD's missing zeros read as
    /// zeros -- unless the stripe says it has more lines than any page this
    /// will decode. That is a sender that does not know how long its page is
    /// and has said so with an enormous YD and L0, and T.85 Amendment 1 (I.2)
    /// has it send the real length after the stripe, with a NEWLEN; so the
    /// stripe is held for that. Anything else after it -- another stripe,
    /// with those lines still owed -- is a page that cannot be followed.
    fn stripe_ends(&mut self, reset: bool) {
        if self.held {
            self.stage = Stage::Spoiled;
            return;
        }
        if !self.open {
            // An SDE with no PSCD at all: the "null stripe" of T.85
            // Amendment 1, or a stripe of lines that all came to nothing.
            self.open_stripe();
        }
        self.ended = true;
        self.reset = reset;
        if self.left() > MAX_LINES as u64 {
            self.held = true;
            self.run();
        } else {
            self.finish_stripe();
        }
    }

    /// Decode what is left of the stripe in hand, and move on to the next.
    fn finish_stripe(&mut self) {
        self.held = false;
        self.run();
        if self.stage != Stage::Data {
            return;
        }
        let header = self.header_of();
        if self.reset {
            // SDRST (6.2.5): the next stripe starts as the top of the image
            // does -- estimates, AT pixel, LNTP and the lines above.
            self.contexts.reset();
            self.at = 0;
            self.lntp_above = true;
            self.rows.clear();
        }
        self.stripe_start += u64::from(header.stripe);
        self.open = false;
        self.ended = false;
        self.coder = None;
        self.scd = Scd::default();
        self.moves.clear();
        // A line can only be part way through here if a NEWLEN has ended the
        // page before it, and then it is not a line of the page.
        self.line_open = false;
    }

    /// Decode as much of the stripe in hand as the bytes here allow: Figure 31
    /// for every pel, with typical prediction's SLNTP in front of each line.
    fn run(&mut self) {
        let header = self.header_of();
        let width = header.width as usize;
        let stripe = u64::from(header.stripe);
        loop {
            if self.stage != Stage::Data || !self.open {
                return;
            }
            let y = self.lines.len() as u64;
            let end = (self.stripe_start + stripe).min(self.height);
            if y >= end {
                return;
            }
            if self.lines.len() >= MAX_LINES {
                self.stage = Stage::Spoiled;
                return;
            }
            if self.coder.is_none() {
                // INITDEC reads three bytes ahead, and once the stripe has
                // ended there are no more to wait for.
                if !(self.ended || self.scd.unread() >= 3 + MARGIN) {
                    return;
                }
                self.coder = Some(arith::Decoder::new(&mut self.scd));
            }
            // Whether the next decision can be decoded with what is here: the
            // data has all arrived and its missing zeros can be read as zeros,
            // or enough of it has arrived that none will be.
            let zeros = self.ended && !self.held;
            let coder = self.coder.as_mut().expect("just made");
            if !self.line_open {
                if !(zeros || self.scd.unread() >= MARGIN) {
                    return;
                }
                let typical = header.typical_prediction && {
                    // 6.5.2, equation 6: LNTP = !(SLNTP xor the last LNTP),
                    // whether this line differs from the one above.
                    let slntp = coder.decode(&mut self.contexts, self.slntp, &mut self.scd);
                    let lntp = slntp == self.lntp_above;
                    self.lntp_above = lntp;
                    !lntp
                };
                let line = (y - self.stripe_start) as u32;
                for &(at, tx) in &self.moves {
                    if at == line {
                        self.at = tx;
                    }
                }
                self.rows.next();
                self.line_open = true;
                self.x = 0;
                if typical {
                    self.rows.repeat();
                    self.x = width;
                }
            }
            while self.x < width {
                if !(zeros || self.scd.unread() >= MARGIN) {
                    return;
                }
                let cx = self.rows.context(header.two_line, self.x, self.at);
                let pel = coder.decode(&mut self.contexts, cx, &mut self.scd);
                self.rows.row[LEFT + self.x] = u8::from(pel);
                self.x += 1;
            }
            self.line_open = false;
            let pels = self.rows.pels();
            let line = (0..self.width).map(|x| pels.get(x) == Some(&1)).collect();
            self.lines.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Options, encode};
    use super::*;

    fn text(rows: usize, width: usize) -> Vec<Vec<bool>> {
        (0..rows)
            .map(|y| {
                let within = y % 24;
                (0..width)
                    .map(|x| within < 18 && ((x + y / 24 * 7) % 29 < 4 || (within == 8 && x % 29 < 20)))
                    .collect()
            })
            .collect()
    }

    fn decode(bie: &[u8], width: usize) -> Decoder {
        let mut decoder = Decoder::new(width);
        decoder.feed_bytes(bie);
        decoder
    }

    #[test]
    fn bits_go_in_least_significant_first() {
        let page = text(40, 200);
        let bie = encode(&page, Options { stripe: 16, ..Options::FAX });
        let bits: Vec<bool> = bie.iter().flat_map(|&b| (0..8).map(move |i| b >> i & 1 == 1)).collect();
        let mut decoder = Decoder::new(200);
        decoder.feed_bits(&bits);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), page.as_slice());
    }

    #[test]
    fn a_byte_at_a_time_it_keeps_up_and_ends_in_the_same_place() {
        let page = text(100, 300);
        let bie = encode(&page, Options { stripe: 32, ..Options::FAX });
        let mut decoder = Decoder::new(300);
        let mut heights = Vec::new();
        for &byte in &bie {
            decoder.feed_bytes(&[byte]);
            heights.push(decoder.lines().len());
            assert!(decoder.lines().len() <= page.len());
            assert_eq!(decoder.lines(), &page[..decoder.lines().len()], "a wrong line on the way");
        }
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), page.as_slice());
        // Lines arrive while the stripes are still arriving, not all at the
        // ends of them.
        let before_the_end = heights[bie.len() * 3 / 4];
        assert!(before_the_end > 50, "only {before_the_end} lines three quarters of the way in");
    }

    #[test]
    fn a_page_narrower_or_wider_than_the_paper_is_fitted_to_it() {
        let page = text(20, 100);
        let bie = encode(&page, Options::FAX);
        let wide = decode(&bie, 120);
        assert!(wide.lines().iter().zip(&page).all(|(got, want)| got[..100] == want[..] && !got[100..].contains(&true)));
        let narrow = decode(&bie, 60);
        assert!(narrow.lines().iter().zip(&page).all(|(got, want)| got[..] == want[..60]));
    }

    #[test]
    fn a_comment_is_stepped_over_whatever_is_in_it() {
        let page = text(30, 200);
        let bie = encode(&page, Options { stripe: 10, ..Options::FAX });
        // A comment of ESC bytes and an SDNORM between the header and the
        // first stripe: counted, so none of it is a marker.
        let mut with = bie[..20].to_vec();
        with.extend_from_slice(&[ESC, COMMENT, 0, 0, 0, 5, ESC, SDNORM, ESC, ESC, 7]);
        with.extend_from_slice(&bie[20..]);
        let decoder = decode(&with, 200);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), page.as_slice());
    }

    #[test]
    fn an_abort_ends_the_page_where_it_is() {
        let page = text(64, 200);
        let bie = encode(&page, Options { stripe: 16, ..Options::FAX });
        // Cut after the second stripe's SDNORM.
        let second = bie
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w == &[ESC, SDNORM])
            .nth(1)
            .map(|(i, _)| i + 2)
            .unwrap();
        let mut cut = bie[..second].to_vec();
        cut.extend_from_slice(&[ESC, ABORT]);
        cut.extend_from_slice(&bie[second..]);
        let decoder = decode(&cut, 200);
        assert!(decoder.is_done());
        assert_eq!(decoder.damaged(), 1);
        assert_eq!(decoder.lines(), &page[..32]);
    }

    #[test]
    fn a_marker_that_is_not_one_spoils_the_page() {
        let page = text(30, 200);
        let bie = encode(&page, Options { stripe: 10, ..Options::FAX });
        for marker in [0x01u8, 0x08, 0x80, ESC] {
            let mut bad = bie[..20].to_vec();
            bad.extend_from_slice(&[ESC, marker]);
            bad.extend_from_slice(&bie[20..]);
            let decoder = decode(&bad, 200);
            assert_eq!(decoder.damaged(), 1, "ESC {marker:#04x}");
            assert!(!decoder.is_done());
        }
    }

    #[test]
    fn an_sdrst_starts_the_next_stripe_as_the_top_of_a_page() {
        // Coded as two separate pages, the second stripe is exactly what a
        // stripe after an SDRST is: estimates, AT pixel, typical prediction
        // and the lines above all as at the top.
        let page = text(40, 200);
        let options = Options { stripe: 20, ..Options::FAX };
        let top = encode(&page[..20], options);
        let bottom = encode(&page[20..], options);
        let mut joined = top[..top.len() - 1].to_vec();
        joined.push(SDRST);
        joined.extend_from_slice(&bottom[20..]);
        joined[8..12].copy_from_slice(&40u32.to_be_bytes());
        let decoder = decode(&joined, 200);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), page.as_slice());
    }

    #[test]
    fn a_stripe_longer_than_any_page_waits_for_its_newlen() {
        // One stripe of a page of unknown length, YD and L0 both 0xffffffff
        // (T.85 Amendment 1, I.2): its last lines are not guessed at from
        // zeros until a NEWLEN says how many there are.
        let page = text(50, 200);
        let bie = encode(&page, Options { stripe: 50, ..Options::FAX });
        let mut unknown = bie.clone();
        unknown[8..16].fill(0xff);
        let mut decoder = decode(&unknown, 200);
        assert!(!decoder.is_done(), "done without knowing how long");
        assert!(decoder.lines().len() <= 50);
        assert_eq!(decoder.lines(), &page[..decoder.lines().len()]);
        decoder.feed_bytes(&[ESC, NEWLEN, 0, 0, 0, 50]);
        assert!(decoder.is_done());
        assert_eq!(decoder.lines(), page.as_slice());
        // Another stripe in its place, with those lines still owed, is a page
        // that cannot be followed -- and what came before it stays.
        let mut decoder = decode(&unknown, 200);
        let before = decoder.lines().len();
        decoder.feed_bytes(&bie[20..]);
        assert_eq!(decoder.damaged(), 1);
        assert_eq!(decoder.lines().len(), before);
    }

    #[test]
    fn an_at_move_that_reaches_the_future_spoils_the_page() {
        let page = text(10, 200);
        let bie = encode(&page, Options::FAX);
        for (tx, ty) in [(0xfeu8, 0u8), (4, 1)] {
            let mut bad = bie[..20].to_vec();
            bad.extend_from_slice(&[ESC, ATMOVE, 0, 0, 0, 0, tx, ty]);
            bad.extend_from_slice(&bie[20..]);
            assert_eq!(decode(&bad, 200).damaged(), 1, "τX {tx:#x}, τY {ty}");
        }
    }
}
