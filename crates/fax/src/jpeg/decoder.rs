//! Reading a T.4 Annex E page: a baseline JPEG data stream, as it arrives.
//!
//! The stream comes in by the ECM frame and the page is worth seeing on its
//! way, so this decodes as far as the octets in hand allow and no further.
//! The unit is the MCU row: one is decoded whole or not at all, and a row
//! that runs out of octets is put back and tried again when more have come.
//!
//! The length of the page may not be known until the end. A frame header
//! with Y = 0 means a DNL marker after the scan says it (T.4 E.6.2, T.81
//! B.2.5), so the scan is read until a marker ends it, and the count of rows
//! is whatever had arrived by then.

use super::{G3FAX, ZIGZAG, dct, marker};
use crate::picture::{Gamut, Picture};

/// The widest frame taken, in samples: more than A3 at 400 pels/25.4 mm.
const MAX_WIDTH: usize = 8192;
/// And the longest, in lines: six sheets of A4 at 200 pels/25.4 mm.
const MAX_LINES: usize = 16_384;

/// Why decoding stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// The octets in hand ran out: more will come.
    Starved,
    /// A marker is next, and the entropy-coded segment is over.
    Marker,
    /// The stream is not one this reads.
    Corrupt,
}

/// A Huffman table set out for decoding: MINCODE, MAXCODE and VALPTR of
/// T.81 F.2.2.3 (Figure F.15), by code length from 1 to 16.
#[derive(Debug, Clone)]
struct Table {
    mincode: [i32; 17],
    maxcode: [i32; 17],
    valptr: [usize; 17],
    values: Vec<u8>,
}

impl Table {
    fn new(bits: &[u8; 16], values: &[u8]) -> Self {
        let mut table = Self {
            mincode: [0; 17],
            maxcode: [-1; 17],
            valptr: [0; 17],
            values: values.to_vec(),
        };
        // The codes of each length are consecutive, and the first of a
        // length is the one after the last of the length before with a zero
        // on the end (T.81 C.2, Figure C.2).
        let mut code = 0i32;
        let mut at = 0usize;
        for length in 1..=16 {
            let count = usize::from(bits[length - 1]);
            if count > 0 {
                table.valptr[length] = at;
                table.mincode[length] = code;
                code += count as i32;
                table.maxcode[length] = code - 1;
                at += count;
            }
            code <<= 1;
        }
        table
    }

    /// The DECODE procedure (T.81 F.2.2.3, Figure F.16).
    fn decode(&self, bits: &mut Bits, data: &[u8]) -> Result<u8, Stop> {
        let mut code = bits.bit(data)? as i32;
        let mut length = 1;
        while code > self.maxcode[length] {
            length += 1;
            if length > 16 {
                return Err(Stop::Corrupt);
            }
            code = code << 1 | bits.bit(data)? as i32;
        }
        let at = self.valptr[length] + (code - self.mincode[length]) as usize;
        self.values.get(at).copied().ok_or(Stop::Corrupt)
    }
}

/// The entropy-coded segment, a bit at a time (T.81 F.2.2.5, NEXTBIT).
#[derive(Debug, Clone, Copy, Default)]
struct Bits {
    /// The next octet to take.
    pos: usize,
    acc: u8,
    /// How many bits of `acc` are still to be read: fewer than eight between
    /// symbols, so all that is ever held back is the end of one octet.
    left: u8,
}

impl Bits {
    fn bit(&mut self, data: &[u8]) -> Result<u32, Stop> {
        if self.left == 0 {
            let &octet = data.get(self.pos).ok_or(Stop::Starved)?;
            if octet == 0xFF {
                // B.1.1.5: X'FF' in the data has a zero octet stuffed after
                // it. With anything else after it, it is a marker.
                match data.get(self.pos + 1) {
                    None => return Err(Stop::Starved),
                    Some(0) => self.pos += 2,
                    Some(_) => return Err(Stop::Marker),
                }
            } else {
                self.pos += 1;
            }
            self.acc = octet;
            self.left = 8;
        }
        self.left -= 1;
        Ok(u32::from(self.acc >> self.left & 1))
    }

    /// RECEIVE and EXTEND (T.81 F.2.2.4 and F.2.2.1, Figures F.17 and F.12):
    /// `size` bits as the signed value they stand for.
    fn value(&mut self, size: u8, data: &[u8]) -> Result<i32, Stop> {
        let mut v = 0i32;
        for _ in 0..size {
            v = v << 1 | self.bit(data)? as i32;
        }
        if size > 0 && v < 1 << (size - 1) {
            v += (-1 << size) + 1;
        }
        Ok(v)
    }
}

/// A component of the frame (T.81 B.2.2) with its tables for the scan
/// (B.2.3).
#[derive(Debug, Clone, Copy)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    quant: usize,
    dc: usize,
    ac: usize,
}

#[derive(Debug, Clone)]
struct Frame {
    width: usize,
    /// Y of the frame header: zero if a DNL marker is to say.
    height: usize,
    components: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcus_across: usize,
}

impl Frame {
    /// Lines to an MCU row.
    fn row_lines(&self) -> usize {
        8 * self.vmax
    }
}

/// Where the scan has got to: what a row that runs short is put back to.
#[derive(Debug, Clone, Copy, Default)]
struct Scan {
    bits: Bits,
    /// The DC prediction for each component (F.2.1.3.1).
    predictions: [i32; 4],
    /// MCUs decoded since the scan began.
    mcus: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before SOI.
    Start,
    /// Between SOI and SOS: tables and the frame header.
    Tables,
    /// In the entropy-coded segments.
    Scan,
    /// After the scan: DNL, perhaps, and EOI.
    After,
    Done,
}

/// A JPEG data stream in, and the page out, as far as it has come.
#[derive(Debug, Clone)]
pub struct Decoder {
    /// How wide the one-bit preview is, in pels.
    width: usize,
    data: Vec<u8>,
    /// Bits of an octet not yet whole, and how many.
    partial: u8,
    partial_bits: u8,
    /// How far the markers have been read.
    at: usize,
    state: State,
    /// Quantisation tables in zig-zag order, as they are sent (B.2.4.1).
    quant: [Option<[u16; 64]>; 4],
    dc: [Option<Table>; 4],
    ac: [Option<Table>; 4],
    /// MCUs to a restart interval, or zero for none (B.2.4.4).
    restart_interval: usize,
    frame: Option<Frame>,
    scan: Scan,
    /// Each component's samples at its own sampling, row-major: `8 x H x
    /// mcus_across` across, and growing by `8 x V` lines an MCU row.
    planes: Vec<Vec<u8>>,
    mcu_rows: usize,
    /// The number of lines, once the frame header or a DNL has said.
    lines_said: Option<usize>,
    /// The page as ink and paper, for everything that only knows that.
    lines: Vec<Vec<bool>>,
    /// What the `APP1` markers said (T.4 E.6.5 to E.6.7).
    pels_per_inch: Option<u16>,
    gamut: Option<Gamut>,
    illuminant: Option<[u8; 4]>,
    damaged: bool,
    /// One past EOI, once it has come.
    end: Option<usize>,
}

impl Decoder {
    /// A decoder whose one-bit preview is `width` pels across.
    pub fn new(width: usize) -> Self {
        Self {
            width,
            data: Vec::new(),
            partial: 0,
            partial_bits: 0,
            at: 0,
            state: State::Start,
            quant: [None; 4],
            dc: [None, None, None, None],
            ac: [None, None, None, None],
            restart_interval: 0,
            frame: None,
            scan: Scan::default(),
            planes: Vec::new(),
            mcu_rows: 0,
            lines_said: None,
            lines: Vec::new(),
            pels_per_inch: None,
            gamut: None,
            illuminant: None,
            damaged: false,
            end: None,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// Bits off the line: each octet least significant bit first (T.4 E.6.9).
    pub fn feed_bits(&mut self, bits: &[bool]) {
        for &bit in bits {
            self.partial |= u8::from(bit) << self.partial_bits;
            self.partial_bits += 1;
            if self.partial_bits == 8 {
                self.data.push(self.partial);
                self.partial = 0;
                self.partial_bits = 0;
            }
        }
        self.advance();
    }

    /// Octets of the stream.
    pub fn feed(&mut self, octets: &[u8]) {
        self.data.extend_from_slice(octets);
        self.advance();
    }

    /// Whether EOI has come, or the stream turned out not to be readable.
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// One if the stream stopped making sense, as for the other codings that
    /// cannot find their place again.
    pub fn damaged(&self) -> usize {
        usize::from(self.damaged)
    }

    /// The page so far as ink and paper: L* through an ordered dither.
    pub fn lines(&self) -> &[Vec<bool>] {
        &self.lines
    }

    /// Lines decoded so far, or the lines of the page once it is over.
    ///
    /// It never goes down. While a DNL is still to say where the page ends,
    /// the last MCU row decoded is held back: some of its lines may be past
    /// the end, there only to make the row whole (A.2.4).
    pub fn height(&self) -> usize {
        let Some(frame) = self.frame.as_ref() else {
            return 0;
        };
        match self.lines_said {
            Some(said) => said.min(self.mcu_rows * frame.row_lines()),
            None if self.state == State::Done => self.mcu_rows * frame.row_lines(),
            None => self.mcu_rows.saturating_sub(1) * frame.row_lines(),
        }
    }

    /// Whether the page has a* and b*; `None` before the frame header.
    pub fn is_colour(&self) -> Option<bool> {
        self.frame.as_ref().map(|f| f.components.len() == 3)
    }

    /// The illuminant a `G3FAX2` marker named, if one did (T.4 E.6.7).
    pub fn illuminant(&self) -> Option<[u8; 4]> {
        self.illuminant
    }

    /// Lines `from..to` of the page as a picture, without the stream.
    pub fn part(&self, from: usize, to: usize) -> Option<Picture> {
        let frame = self.frame.as_ref()?;
        let to = to.min(self.height());
        let from = from.min(to);
        let components = frame.components.len();
        let mut samples = Vec::with_capacity((to - from) * frame.width * components);
        for y in from..to {
            for x in 0..frame.width {
                for (component, plane) in frame.components.iter().zip(&self.planes) {
                    // A component sampled less often gives each of its
                    // samples to the pels it was the mean of (T.4 E.6.3).
                    let across = frame.mcus_across * 8 * component.h;
                    let (sx, sy) = (x * component.h / frame.hmax, y * component.v / frame.vmax);
                    samples.push(plane[sy * across + sx]);
                }
            }
        }
        Some(Picture {
            width: frame.width,
            height: to - from,
            components,
            samples,
            // T.4 E.6.5: "The basic value is 200."
            pels_per_inch: self.pels_per_inch.unwrap_or(200),
            gamut: self.gamut.unwrap_or_default(),
            jpeg: Vec::new(),
        })
    }

    /// The page, with the stream it came in: from SOI to EOI, or to wherever
    /// it stopped.
    pub fn picture(&self) -> Option<Picture> {
        let mut picture = self.part(0, self.height())?;
        if picture.height == 0 {
            return None;
        }
        picture.jpeg = self.data[..self.end.unwrap_or(self.data.len())].to_vec();
        Some(picture)
    }

    fn fail(&mut self) {
        self.damaged = true;
        self.state = State::Done;
    }

    fn advance(&mut self) {
        loop {
            match self.state {
                State::Start => {
                    // The stream starts with SOI (T.4 E.6.5: the first APP1
                    // "directly follows the SOI marker").
                    let Some(pair) = self.data.get(self.at..self.at + 2) else {
                        return;
                    };
                    if pair == [0xFF, marker::SOI] {
                        self.at += 2;
                        self.state = State::Tables;
                    } else {
                        self.at += 1;
                    }
                }
                State::Tables | State::After => {
                    if !self.segment() {
                        return;
                    }
                }
                State::Scan => match self.row() {
                    Ok(true) => {
                        let frame = self.frame.as_ref().expect("a scan has a frame");
                        let lines = self.mcu_rows * frame.row_lines();
                        let whole = frame.height != 0 && lines >= frame.height;
                        if whole || lines >= MAX_LINES {
                            self.damaged |= !whole;
                            self.leave_scan();
                        }
                    }
                    Ok(false) => self.leave_scan(),
                    Err(Stop::Starved) => return,
                    Err(_) => {
                        self.damaged = true;
                        self.leave_scan();
                    }
                },
                State::Done => return,
            }
        }
    }

    /// The entropy-coded data is over: on to the markers after it, from the
    /// end of the last whole row.
    fn leave_scan(&mut self) {
        self.at = self.scan.bits.pos;
        self.state = State::After;
    }

    /// Read one marker segment at `at`. False if it is not all here yet.
    fn segment(&mut self) -> bool {
        // Anything that is not a marker, between segments, is passed over:
        // fill octets X'FF' (B.1.1.2), and after a damaged scan what is left
        // of it.
        while self.data.get(self.at).is_some_and(|&b| b != 0xFF) {
            self.at += 1;
        }
        let Some(&code) = self.data.get(self.at + 1) else {
            return false;
        };
        match code {
            0xFF => {
                self.at += 1;
                return true;
            }
            // A stuffed zero or a restart marker left from a damaged scan,
            // and the markers that stand alone (B.1.1.3).
            0x00 | 0x01 | marker::RST0..=marker::RST7 | marker::SOI => {
                self.at += 2;
                return true;
            }
            marker::EOI => {
                self.at += 2;
                self.end = Some(self.at);
                if self.state == State::Tables {
                    // An image with no scan in it.
                    self.damaged = true;
                }
                self.finish();
                return true;
            }
            _ => {}
        }
        let Some(length) = self.data.get(self.at + 2..self.at + 4) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        let (body, end) = (self.at + 4, self.at + 2 + length);
        if length < 2 {
            self.fail();
            return true;
        }
        if self.data.len() < end {
            return false;
        }
        self.at = end;
        let ok = match (self.state, code) {
            (_, marker::DNL) => self.number_of_lines(body, end),
            (State::After, _) => true,
            (_, marker::DQT) => self.quantisation(body, end),
            (_, marker::DHT) => self.huffman(body, end),
            (_, marker::DRI) => self.restart(body, end),
            (_, marker::APP1) => self.application(body, end),
            (_, marker::SOF0 | marker::SOF1) => self.frame_header(body, end),
            (_, marker::SOS) => self.scan_header(body, end),
            // Every other frame type: progressive, lossless, arithmetic
            // (Table B.1). None of them is a fax (T.4 E.6.2).
            (_, 0xC2..=0xCF) => false,
            // COM and the other APPn: "the decoder shall skip these marker
            // segments and continue" (T.4 E.6.2).
            _ => true,
        };
        if !ok {
            self.fail();
        }
        true
    }

    /// DQT (B.2.4.1): any number of tables, each a precision and a
    /// destination and then 64 elements in zig-zag order.
    fn quantisation(&mut self, mut at: usize, end: usize) -> bool {
        while at < end {
            let (precision, destination) = (self.data[at] >> 4, usize::from(self.data[at] & 15));
            at += 1;
            let size = if precision == 0 { 1 } else { 2 };
            if precision > 1 || destination > 3 || at + 64 * size > end {
                return false;
            }
            let mut table = [0u16; 64];
            for (k, element) in table.iter_mut().enumerate() {
                *element = if size == 1 {
                    u16::from(self.data[at + k])
                } else {
                    u16::from_be_bytes([self.data[at + 2 * k], self.data[at + 2 * k + 1]])
                };
            }
            self.quant[destination] = Some(table);
            at += 64 * size;
        }
        true
    }

    /// DHT (B.2.4.2): any number of tables, each a class and a destination,
    /// sixteen counts and then the values.
    fn huffman(&mut self, mut at: usize, end: usize) -> bool {
        while at < end {
            let (class, destination) = (self.data[at] >> 4, usize::from(self.data[at] & 15));
            at += 1;
            if class > 1 || destination > 3 || at + 16 > end {
                return false;
            }
            let bits: [u8; 16] = self.data[at..at + 16].try_into().expect("sixteen counts");
            at += 16;
            let count: usize = bits.iter().map(|&n| usize::from(n)).sum();
            if count > 256 || at + count > end {
                return false;
            }
            let table = Table::new(&bits, &self.data[at..at + count]);
            at += count;
            if class == 0 {
                self.dc[destination] = Some(table);
            } else {
                self.ac[destination] = Some(table);
            }
        }
        true
    }

    /// DRI (B.2.4.4).
    fn restart(&mut self, at: usize, end: usize) -> bool {
        if end - at < 2 {
            return false;
        }
        self.restart_interval = usize::from(u16::from_be_bytes([self.data[at], self.data[at + 1]]));
        true
    }

    /// DNL (B.2.5): the number of lines, after the first scan.
    fn number_of_lines(&mut self, at: usize, end: usize) -> bool {
        if end - at < 2 || self.state != State::After {
            return false;
        }
        let lines = usize::from(u16::from_be_bytes([self.data[at], self.data[at + 1]]));
        // "The value specified shall be consistent with the number of lines
        // coded at the point where the DNL segment terminates": fewer rows
        // than it says is a page cut short.
        let decoded = self.frame.as_ref().map_or(0, |f| self.mcu_rows * f.row_lines());
        if lines > decoded && self.lines_said.is_none_or(|said| lines > said) {
            self.damaged = true;
        }
        self.lines_said = Some(self.lines_said.map_or(lines, |said| said.min(lines)));
        self.preview();
        true
    }

    /// APP1 (T.4 E.6.5 to E.6.7): "G3FAX" and an octet that says which.
    fn application(&mut self, at: usize, end: usize) -> bool {
        let body = &self.data[at..end];
        if body.len() < 6 || &body[..5] != G3FAX {
            // Some other application's APP1: skipped.
            return true;
        }
        let rest = &body[6..];
        match body[5] {
            // The version, X'07CA', and the resolution.
            0 if rest.len() >= 4 => {
                self.pels_per_inch = Some(u16::from_be_bytes([rest[2], rest[3]]));
            }
            // G3FAX1: six signed integers, an offset and a range each for
            // L*, a* and b*.
            1 if rest.len() >= 12 => {
                let n = |i: usize| i16::from_be_bytes([rest[2 * i], rest[2 * i + 1]]);
                let ranges = [(n(0), n(1)), (n(2), n(3)), (n(4), n(5))];
                if ranges.iter().all(|(_, range)| *range != 0) {
                    self.gamut = Some(Gamut { ranges });
                }
            }
            // G3FAX2: the illuminant, four octets.
            2 if rest.len() >= 4 => {
                self.illuminant = Some([rest[0], rest[1], rest[2], rest[3]]);
            }
            _ => {}
        }
        true
    }

    /// SOF0 (B.2.2), and SOF1 where it is the same thing: eight bits a
    /// sample.
    fn frame_header(&mut self, at: usize, end: usize) -> bool {
        let body = &self.data[at..end];
        if body.len() < 6 || self.frame.is_some() {
            return false;
        }
        let precision = body[0];
        let height = usize::from(u16::from_be_bytes([body[1], body[2]]));
        let width = usize::from(u16::from_be_bytes([body[3], body[4]]));
        let count = usize::from(body[5]);
        // One component is L* alone and three are L*, a*, b* (T.4 E.6.2.2).
        if precision != 8 || width == 0 || width > MAX_WIDTH || !matches!(count, 1 | 3) || body.len() < 6 + 3 * count {
            return false;
        }
        let mut components = Vec::with_capacity(count);
        for c in body[6..6 + 3 * count].chunks_exact(3) {
            let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
                return false;
            }
            components.push(Component { id: c[0], h, v, quant: usize::from(c[2]), dc: 0, ac: 0 });
        }
        if count == 1 {
            // A scan of one component is not interleaved: its MCU is one
            // block whatever the frame header says of it (A.2.2).
            components[0].h = 1;
            components[0].v = 1;
        }
        let hmax = components.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = components.iter().map(|c| c.v).max().unwrap_or(1);
        self.frame = Some(Frame {
            width,
            height,
            components,
            hmax,
            vmax,
            mcus_across: width.div_ceil(8 * hmax),
        });
        self.lines_said = (height != 0).then_some(height);
        true
    }

    /// SOS (B.2.3): which tables each component uses, and then the scan.
    fn scan_header(&mut self, at: usize, end: usize) -> bool {
        let body = &self.data[at..end];
        let Some(frame) = self.frame.as_mut() else {
            return false;
        };
        let count = frame.components.len();
        // "Only one scan is contained within the image data" (T.4 E.6.2.2):
        // every component of the frame, interleaved.
        if body.len() < 1 + 2 * count + 3 || usize::from(body[0]) != count {
            return false;
        }
        for (component, selector) in frame.components.iter_mut().zip(body[1..].chunks_exact(2)) {
            if selector[0] != component.id {
                return false;
            }
            component.dc = usize::from(selector[1] >> 4);
            component.ac = usize::from(selector[1] & 15);
            let have = component.dc < 4
                && component.ac < 4
                && self.dc[component.dc].is_some()
                && self.ac[component.ac].is_some()
                && self.quant[component.quant].is_some();
            if !have {
                return false;
            }
        }
        // Ss = 0, Se = 63, Ah = Al = 0: sequential, and nothing else.
        if body[1 + 2 * count..1 + 2 * count + 3] != [0, 63, 0] {
            return false;
        }
        self.planes = vec![Vec::new(); count];
        self.scan = Scan {
            bits: Bits { pos: end, ..Bits::default() },
            ..Scan::default()
        };
        self.state = State::Scan;
        true
    }

    /// Decode one MCU row, if all of it is here. `Ok(false)` if a marker
    /// ended the scan before it.
    fn row(&mut self) -> Result<bool, Stop> {
        let frame = self.frame.as_ref().ok_or(Stop::Corrupt)?;
        let mut scan = self.scan;
        // The coefficients of the row, dequantised and in row-major order,
        // a block after another in the order they are coded. The transform
        // waits until the row is known to be whole.
        let per_mcu: usize = frame.components.iter().map(|c| c.h * c.v).sum();
        let mut blocks = vec![[0i32; 64]; per_mcu * frame.mcus_across];
        let mut block = 0;
        for mcu in 0..frame.mcus_across {
            if self.restart_interval > 0 && scan.mcus > 0 && scan.mcus.is_multiple_of(self.restart_interval) {
                // A restart marker is due (E.2.4, F.2.2.5): the bits left of
                // the last octet are padding, and the predictions go back to
                // zero. A different marker here is the end of the scan.
                match self.data.get(scan.bits.pos..scan.bits.pos + 2) {
                    None => return Err(Stop::Starved),
                    Some([0xFF, marker::RST0..=marker::RST7]) => {
                        scan.bits.pos += 2;
                        scan.bits.left = 0;
                        scan.predictions = [0; 4];
                    }
                    Some([0xFF, code]) if *code != 0 => {
                        return if mcu == 0 { Ok(false) } else { Err(Stop::Corrupt) };
                    }
                    Some(_) => return Err(Stop::Corrupt),
                }
            }
            for (index, component) in frame.components.iter().enumerate() {
                let dc = self.dc[component.dc].as_ref().ok_or(Stop::Corrupt)?;
                let ac = self.ac[component.ac].as_ref().ok_or(Stop::Corrupt)?;
                let quant = self.quant[component.quant].as_ref().ok_or(Stop::Corrupt)?;
                for part in 0..component.h * component.v {
                    // F.2.2.1: the DC term is a difference from the last of
                    // this component.
                    let size = match dc.decode(&mut scan.bits, &self.data) {
                        // A marker where an MCU would start is where the
                        // scan ends: the bits before it were padding, which
                        // is all ones and so never a whole code (F.1.2.3).
                        Err(Stop::Marker) if index == 0 && part == 0 => {
                            return if mcu == 0 { Ok(false) } else { Err(Stop::Corrupt) };
                        }
                        other => other?,
                    };
                    if size > 11 {
                        return Err(Stop::Corrupt);
                    }
                    scan.predictions[index] += scan.bits.value(size, &self.data)?;
                    let coefficients = &mut blocks[block];
                    coefficients[0] = scan.predictions[index] * i32::from(quant[0]);
                    // F.2.2.2: runs of zeros and the coefficient that ends
                    // each, in zig-zag order, until EOB or the 63rd.
                    let mut k = 1;
                    while k < 64 {
                        let symbol = ac.decode(&mut scan.bits, &self.data)?;
                        let (run, size) = (usize::from(symbol >> 4), symbol & 15);
                        if size == 0 {
                            if run != 15 {
                                break;
                            }
                            k += 16;
                            continue;
                        }
                        k += run;
                        if k > 63 {
                            return Err(Stop::Corrupt);
                        }
                        let value = scan.bits.value(size, &self.data)?;
                        coefficients[usize::from(ZIGZAG[k])] = value * i32::from(quant[k]);
                        k += 1;
                    }
                    block += 1;
                }
            }
            scan.mcus += 1;
        }

        // The row is whole: transform it into the planes.
        let mut samples = [0u8; 64];
        let mut starts = Vec::with_capacity(frame.components.len());
        for (component, plane) in frame.components.iter().zip(&mut self.planes) {
            starts.push(plane.len());
            plane.resize(plane.len() + frame.mcus_across * 8 * component.h * 8 * component.v, 0);
        }
        let mut block = 0;
        for mcu in 0..frame.mcus_across {
            for (index, component) in frame.components.iter().enumerate() {
                let across = frame.mcus_across * 8 * component.h;
                for by in 0..component.v {
                    for bx in 0..component.h {
                        dct::inverse(&blocks[block], &mut samples);
                        block += 1;
                        let left = (mcu * component.h + bx) * 8;
                        for (y, line) in samples.chunks_exact(8).enumerate() {
                            let at = starts[index] + (by * 8 + y) * across + left;
                            self.planes[index][at..at + 8].copy_from_slice(line);
                        }
                    }
                }
            }
        }
        self.scan = scan;
        self.mcu_rows += 1;
        self.preview();
        Ok(true)
    }

    /// Bring the one-bit preview up to the lines decoded.
    fn preview(&mut self) {
        // An ordered dither, the 4 x 4 Bayer matrix: each line is made from
        // itself alone, so lines made as they arrive match the whole page.
        const BAYER: [[u16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let Some(frame) = self.frame.as_ref() else {
            return;
        };
        if self.planes.is_empty() {
            return;
        }
        // Only a DNL that stops a page short of what its frame header said
        // takes lines away (T.4 E.6.2).
        self.lines.truncate(self.height());
        let lightness = &frame.components[0];
        let across = frame.mcus_across * 8 * lightness.h;
        for y in self.lines.len()..self.height() {
            let sy = y * lightness.v / frame.vmax;
            let row = &self.planes[0][sy * across..(sy + 1) * across];
            let line = (0..self.width)
                .map(|x| {
                    // Past the edge of a narrower picture is paper.
                    x < frame.width && u16::from(row[x * lightness.h / frame.hmax]) < BAYER[y % 4][x % 4] * 16 + 8
                })
                .collect();
            self.lines.push(line);
        }
    }

    fn finish(&mut self) {
        self.state = State::Done;
        if self.lines_said.is_none() && self.frame.as_ref().is_some_and(|f| f.height == 0) {
            // Y was zero and no DNL said otherwise: the rows that came are
            // the page, and its true length is not known.
            self.damaged |= self.mcu_rows == 0;
        }
        self.preview();
    }
}

/// A stream as received, made fit for a file: if Y in the frame header was
/// zero it is set to `height`, and the DNL segment that said so is left out.
///
/// Little that reads a JPEG file obeys DNL: some readers refuse a frame
/// header that says no lines at all, and some refuse the DNL marker itself.
/// So the one field is filled in and the one segment dropped, and the stream
/// is otherwise the octets that came off the line. A stream that said its
/// length at the start is not touched.
pub fn for_a_file(stream: &[u8], height: usize) -> Vec<u8> {
    let mut out = stream.to_vec();
    let mut filled = false;
    let mut at = 2;
    while at + 4 <= out.len() && out[at] == 0xFF {
        let code = out[at + 1];
        let length = usize::from(u16::from_be_bytes([out[at + 2], out[at + 3]]));
        if matches!(code, marker::SOF0 | marker::SOF1) {
            if at + 7 <= out.len() && out[at + 5] == 0 && out[at + 6] == 0 {
                let y = (height.min(usize::from(u16::MAX)) as u16).to_be_bytes();
                out[at + 5] = y[0];
                out[at + 6] = y[1];
                filled = true;
            }
            break;
        }
        if code == marker::SOS {
            break;
        }
        at += 2 + length;
    }
    // DNL "shall immediately follow the first scan" (B.2.4), which for the
    // one scan of a fax page is just before EOI: X'FFDC', Ld = 4, NL, and
    // then X'FFD9'. Entropy-coded data never has X'FF' before a non-zero
    // octet, so those eight octets at the end are that and nothing else.
    let n = out.len();
    if filled && n >= 8 && out[n - 8..n - 4] == [0xFF, marker::DNL, 0, 4] && out[n - 2..] == [0xFF, marker::EOI] {
        out.drain(n - 8..n - 2);
    }
    out
}
