//! The adaptive arithmetic coder of 6.8/T.82: the QM coder.
//!
//! One binary decision at a time, each in a context. The coder keeps an
//! interval, `A`, and the bottom of it, `C`; a decision the context thinks
//! likely (the MPS) shrinks the interval a little and a decision it thinks
//! unlikely (the LPS) shrinks it a lot and moves the bottom up, and the bytes
//! that come out are the binary expansion of wherever the bottom ends. Nothing
//! is multiplied: the LPS share of the interval is a stored number, `LSZ`, one
//! per estimation state, because `A` is always kept between 0x8000 and
//! 0x10000 and its average is near enough to stand in for it (6.8.1.2).
//!
//! Every procedure here is one of the flow diagrams of 6.8.2 and 6.8.3, read
//! off the rendered figures, under its own name. 6.8 makes those diagrams
//! normative only in the output they define -- any procedure giving the same
//! bytes will do -- and the bytes are proved against 7.1's SCD and the
//! registers of Table 26's trace below. Where a figure is wrong on its face
//! the comment says so.

/// One row of Table 24/T.82: a probability-estimation state.
///
/// `lsz` is the LPS interval; `nlps` and `nmps` the state to go to after an
/// LPS, and after an MPS that needed a renormalization; `swtch` whether an LPS
/// in this state turns the context's idea of the MPS round (6.8.2.3).
#[derive(Debug, Clone, Copy)]
struct Estimate {
    lsz: u16,
    nlps: u8,
    nmps: u8,
    swtch: bool,
}

const fn e(lsz: u16, nlps: u8, nmps: u8, swtch: u8) -> Estimate {
    Estimate { lsz, nlps, nmps, swtch: swtch == 1 }
}

/// Table 24/T.82, the probability estimation table, row by row from ST 0.
///
/// Transcribed from the rendered page and not from the extracted text, whose
/// columns wander. Table 26's trace checks the states 7.1 passes through, and
/// 7.2.2's byte counts, over nearly four million pels in a thousand contexts,
/// would not survive a wrong entry in any state those visit.
const TABLE: [Estimate; 113] = [
    e(0x5a1d, 1, 1, 1), e(0x2586, 14, 2, 0), e(0x1114, 16, 3, 0), e(0x080b, 18, 4, 0),
    e(0x03d8, 20, 5, 0), e(0x01da, 23, 6, 0), e(0x00e5, 25, 7, 0), e(0x006f, 28, 8, 0),
    e(0x0036, 30, 9, 0), e(0x001a, 33, 10, 0), e(0x000d, 35, 11, 0), e(0x0006, 9, 12, 0),
    e(0x0003, 10, 13, 0), e(0x0001, 12, 13, 0), e(0x5a7f, 15, 15, 1), e(0x3f25, 36, 16, 0),
    e(0x2cf2, 38, 17, 0), e(0x207c, 39, 18, 0), e(0x17b9, 40, 19, 0), e(0x1182, 42, 20, 0),
    e(0x0cef, 43, 21, 0), e(0x09a1, 45, 22, 0), e(0x072f, 46, 23, 0), e(0x055c, 48, 24, 0),
    e(0x0406, 49, 25, 0), e(0x0303, 51, 26, 0), e(0x0240, 52, 27, 0), e(0x01b1, 54, 28, 0),
    e(0x0144, 56, 29, 0), e(0x00f5, 57, 30, 0), e(0x00b7, 59, 31, 0), e(0x008a, 60, 32, 0),
    e(0x0068, 62, 33, 0), e(0x004e, 63, 34, 0), e(0x003b, 32, 35, 0), e(0x002c, 33, 9, 0),
    e(0x5ae1, 37, 37, 1), e(0x484c, 64, 38, 0), e(0x3a0d, 65, 39, 0), e(0x2ef1, 67, 40, 0),
    e(0x261f, 68, 41, 0), e(0x1f33, 69, 42, 0), e(0x19a8, 70, 43, 0), e(0x1518, 72, 44, 0),
    e(0x1177, 73, 45, 0), e(0x0e74, 74, 46, 0), e(0x0bfb, 75, 47, 0), e(0x09f8, 77, 48, 0),
    e(0x0861, 78, 49, 0), e(0x0706, 79, 50, 0), e(0x05cd, 48, 51, 0), e(0x04de, 50, 52, 0),
    e(0x040f, 50, 53, 0), e(0x0363, 51, 54, 0), e(0x02d4, 52, 55, 0), e(0x025c, 53, 56, 0),
    e(0x01f8, 54, 57, 0), e(0x01a4, 55, 58, 0), e(0x0160, 56, 59, 0), e(0x0125, 57, 60, 0),
    e(0x00f6, 58, 61, 0), e(0x00cb, 59, 62, 0), e(0x00ab, 61, 63, 0), e(0x008f, 61, 32, 0),
    e(0x5b12, 65, 65, 1), e(0x4d04, 80, 66, 0), e(0x412c, 81, 67, 0), e(0x37d8, 82, 68, 0),
    e(0x2fe8, 83, 69, 0), e(0x293c, 84, 70, 0), e(0x2379, 86, 71, 0), e(0x1edf, 87, 72, 0),
    e(0x1aa9, 87, 73, 0), e(0x174e, 72, 74, 0), e(0x1424, 72, 75, 0), e(0x119c, 74, 76, 0),
    e(0x0f6b, 74, 77, 0), e(0x0d51, 75, 78, 0), e(0x0bb6, 77, 79, 0), e(0x0a40, 77, 48, 0),
    e(0x5832, 80, 81, 1), e(0x4d1c, 88, 82, 0), e(0x438e, 89, 83, 0), e(0x3bdd, 90, 84, 0),
    e(0x34ee, 91, 85, 0), e(0x2eae, 92, 86, 0), e(0x299a, 93, 87, 0), e(0x2516, 86, 71, 0),
    e(0x5570, 88, 89, 1), e(0x4ca9, 95, 90, 0), e(0x44d9, 96, 91, 0), e(0x3e22, 97, 92, 0),
    e(0x3824, 99, 93, 0), e(0x32b4, 99, 94, 0), e(0x2e17, 93, 86, 0), e(0x56a8, 95, 96, 1),
    e(0x4f46, 101, 97, 0), e(0x47e5, 102, 98, 0), e(0x41cf, 103, 99, 0), e(0x3c3d, 104, 100, 0),
    e(0x375e, 99, 93, 0), e(0x5231, 105, 102, 0), e(0x4c0f, 106, 103, 0), e(0x4639, 107, 104, 0),
    e(0x415e, 103, 99, 0), e(0x5627, 105, 106, 1), e(0x50e7, 108, 107, 0), e(0x4b85, 109, 103, 0),
    e(0x5597, 110, 109, 0), e(0x504f, 111, 107, 0), e(0x5a10, 110, 111, 1), e(0x5522, 112, 109, 0),
    e(0x59eb, 112, 111, 1),
];

/// The adaptive estimate of every context: `ST[CX]` and `MPS[CX]` of 6.8.2.3.
///
/// Kept apart from the coder because it outlives it. A coder lasts a stripe --
/// INITENC and FLUSH bracket each one -- while the estimates run on from
/// stripe to stripe unless an SDRST says otherwise (6.2.5), which is half of
/// what makes a page cheaper coded whole than in pieces.
#[derive(Debug, Clone)]
pub struct Contexts {
    /// `ST << 1 | MPS`, one byte a context.
    state: Vec<u8>,
}

impl Contexts {
    /// Every context in the equiprobable state 0 with an MPS of 0, which is
    /// where INITENC and INITDEC put them at the top of an image.
    pub fn new(count: usize) -> Self {
        Self { state: vec![0; count] }
    }

    /// Back to the top of the image, as an SDRST has it (6.2.5).
    pub fn reset(&mut self) {
        self.state.fill(0);
    }

    fn st(&self, cx: usize) -> usize {
        usize::from(self.state[cx] >> 1)
    }

    fn mps(&self, cx: usize) -> bool {
        self.state[cx] & 1 == 1
    }

    fn set(&mut self, cx: usize, st: u8, mps: bool) {
        self.state[cx] = st << 1 | u8::from(mps);
    }
}

/// The encoder's registers, Table 23/T.82, for one stripe.
///
/// `C` is laid out as "0000cbbb, bbbbbsss, xxxxxxxx, xxxxxxxx": a carry bit,
/// the byte about to leave, three spacer bits that keep a carry from running
/// further, and sixteen fraction bits lined up with `A`.
#[derive(Debug, Clone)]
pub struct Encoder {
    a: u32,
    c: u32,
    ct: u32,
    /// 0xff bytes held back until a carry is settled (6.8.2.8).
    sc: u32,
    /// The last byte out that was not 0xff, which a carry may still change.
    buffer: u32,
    out: Vec<u8>,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    /// INITENC (Figure 27), for everything but the estimates: `A` at 0x10000,
    /// `C` clear, and `CT` at eleven, "a byte plus the 3 spacer bits".
    ///
    /// `BUFFER` starts at 0x00, as Table 26's first row has it. The first
    /// BYTEOUT writes it out, and FLUSH takes it off again.
    pub fn new() -> Self {
        Self {
            a: 0x10000,
            c: 0,
            ct: 11,
            sc: 0,
            buffer: 0,
            out: Vec::new(),
        }
    }

    /// ENCODE (Figure 22): one decision, `pix`, in context `cx`.
    pub fn encode(&mut self, contexts: &mut Contexts, cx: usize, pix: bool) {
        if pix == contexts.mps(cx) {
            self.code_mps(contexts, cx);
        } else {
            self.code_lps(contexts, cx);
        }
    }

    /// CODELPS (Figure 23).
    ///
    /// The LPS takes the top of the interval, so the MPS part below it is
    /// added to `C` -- unless the approximation has made the LPS part the
    /// larger, when the two swap and the LPS is coded as the bottom part
    /// instead: the conditional exchange of 6.8.1.2.
    fn code_lps(&mut self, contexts: &mut Contexts, cx: usize) {
        let st = contexts.st(cx);
        let row = TABLE[st];
        let lsz = u32::from(row.lsz);
        self.a -= lsz;
        if self.a >= lsz {
            self.c += self.a;
            self.a = lsz;
        }
        let mps = contexts.mps(cx) ^ row.swtch;
        contexts.set(cx, row.nlps, mps);
        self.renorm();
    }

    /// CODEMPS (Figure 24): the bottom of the interval, and nothing to do at
    /// all unless it has fallen below 0x8000 -- when the estimate moves on,
    /// and the exchange may have happened here too.
    fn code_mps(&mut self, contexts: &mut Contexts, cx: usize) {
        let st = contexts.st(cx);
        let row = TABLE[st];
        let lsz = u32::from(row.lsz);
        self.a -= lsz;
        if self.a < 0x8000 {
            if self.a < lsz {
                self.c += self.a;
                self.a = lsz;
            }
            let mps = contexts.mps(cx);
            contexts.set(cx, row.nmps, mps);
            self.renorm();
        }
    }

    /// RENORME (Figure 25): double `A` and `C` until `A` is back at 0x8000 or
    /// over, sending a byte every eight doublings.
    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byte_out();
            }
            if self.a >= 0x8000 {
                break;
            }
        }
    }

    /// BYTEOUT (Figure 26): take the byte at the top of `C`, settling any
    /// carry into the one before it.
    ///
    /// A byte of 0xff is held rather than written, since a carry would turn it
    /// and every 0xff before it into 0x00 and bump the byte in `BUFFER`; `SC`
    /// counts them.
    fn byte_out(&mut self) {
        let temp = self.c >> 19;
        if temp > 0xff {
            debug_assert!(self.buffer < 0xff, "a carry past a settled byte");
            self.out.push((self.buffer + 1) as u8);
            self.out.extend(std::iter::repeat_n(0x00, self.sc as usize));
            self.sc = 0;
            self.buffer = temp & 0xff;
        } else if temp == 0xff {
            self.sc += 1;
        } else {
            self.out.push(self.buffer as u8);
            self.out.extend(std::iter::repeat_n(0xff, self.sc as usize));
            self.sc = 0;
            self.buffer = temp;
        }
        self.c &= 0x7ffff;
        self.ct = 8;
    }

    /// FLUSH (Figure 28): CLEARBITS and FINALWRITES, then the first byte off
    /// the front -- the `BUFFER` that was never a byte of the code -- and every
    /// 0x00 off the end.
    ///
    /// 6.8.2.10 leaves the trailing zeros to the encoder ("if desired"); 7.2.2
    /// counts bytes with all of them gone, and a decoder reads zeros once the
    /// data runs out (6.8.3.8), so taking them costs nothing and is what the
    /// test data is measured against.
    pub fn flush(mut self) -> Vec<u8> {
        self.clear_bits();
        self.final_writes();
        let mut out = self.out;
        out.remove(0);
        while out.last() == Some(&0x00) {
            out.pop();
        }
        out
    }

    /// CLEARBITS (Figure 29): the value in `[C, C + A - 1]` that ends in the
    /// most zeros, so that the zeros FLUSH then drops are as many as can be.
    fn clear_bits(&mut self) {
        let temp = (self.a - 1 + self.c) & 0xffff_0000;
        self.c = if temp < self.c { temp + 0x8000 } else { temp };
    }

    /// FINALWRITES (Figure 30): settle the last carry and write the two bytes
    /// left in `C`.
    ///
    /// The rendered figure has "Write BUFFER + 1" on both of its branches.
    /// The one without a carry cannot mean it: it is BYTEOUT's no-carry case
    /// over again, where the byte goes out as it is and the held 0xff bytes
    /// with it, and adding one there would change the last byte of every
    /// stripe that happens to end without a carry. Table 26's stripe ends with
    /// one, so it cannot tell the two apart; the round trips of the tests can.
    fn final_writes(&mut self) {
        self.c <<= self.ct;
        if self.c > 0x7ff_ffff {
            debug_assert!(self.buffer < 0xff, "a carry past a settled byte");
            self.out.push((self.buffer + 1) as u8);
            self.out.extend(std::iter::repeat_n(0x00, self.sc as usize));
        } else {
            self.out.push(self.buffer as u8);
            self.out.extend(std::iter::repeat_n(0xff, self.sc as usize));
        }
        self.out.push((self.c >> 19 & 0xff) as u8);
        self.out.push((self.c >> 11 & 0xff) as u8);
    }
}

/// A stripe's coded data being read, and zeros once it runs out.
///
/// 6.8.3.8: "Bytes are read from SCD until it exhausts, after which further
/// reads are satisfied by returning 0x00." Which is what makes it safe for an
/// encoder to leave the zeros off the end, and why a decoder has to know where
/// a stripe's data ends before it can read the end of the stripe.
#[derive(Debug, Clone, Default)]
pub struct Scd {
    /// The bytes that have arrived, with the stuffing taken out.
    pub bytes: Vec<u8>,
    /// How many bytes BYTEIN has taken.
    at: usize,
}

impl Scd {
    fn next(&mut self) -> u32 {
        let byte = self.bytes.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        u32::from(byte)
    }

    /// Bytes that have arrived and not been read yet.
    pub fn unread(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }
}

/// The decoder's registers, Table 25/T.82.
///
/// `C` is `CHIGH` and `CLOW` as one 32-bit register: the comparisons use the
/// high half, and new bytes go into the top byte of the low half.
#[derive(Debug, Clone)]
pub struct Decoder {
    a: u32,
    c: u32,
    ct: u32,
}

impl Decoder {
    /// INITDEC (Figure 37), for everything but the estimates: three bytes into
    /// `C`, and `A` at 0x10000.
    pub fn new(scd: &mut Scd) -> Self {
        let mut decoder = Self { a: 0, c: 0, ct: 0 };
        decoder.byte_in(scd);
        decoder.c <<= 8;
        decoder.byte_in(scd);
        decoder.c <<= 8;
        decoder.byte_in(scd);
        decoder.a = 0x10000;
        decoder
    }

    /// DECODE (Figure 32): one decision in context `cx`.
    ///
    /// The MPS is the bottom of the interval, so a `CHIGH` below the new `A`
    /// is one -- and only a renormalization can have hidden an exchange.
    pub fn decode(&mut self, contexts: &mut Contexts, cx: usize, scd: &mut Scd) -> bool {
        let st = contexts.st(cx);
        let lsz = u32::from(TABLE[st].lsz);
        self.a -= lsz;
        if self.c >> 16 < self.a {
            if self.a < 0x8000 {
                let pix = Self::mps_exchange(self.a, contexts, cx);
                self.renorm(scd);
                pix
            } else {
                contexts.mps(cx)
            }
        } else {
            let pix = self.lps_exchange(contexts, cx);
            self.renorm(scd);
            pix
        }
    }

    /// LPS_EXCHANGE (Figure 33): `C` is in the top part of the interval. That
    /// is the LPS, unless the exchange made it the MPS's.
    fn lps_exchange(&mut self, contexts: &mut Contexts, cx: usize) -> bool {
        let st = contexts.st(cx);
        let row = TABLE[st];
        let lsz = u32::from(row.lsz);
        let mps = contexts.mps(cx);
        let exchanged = self.a < lsz;
        self.c -= self.a << 16;
        self.a = lsz;
        if exchanged {
            contexts.set(cx, row.nmps, mps);
            mps
        } else {
            contexts.set(cx, row.nlps, mps ^ row.swtch);
            !mps
        }
    }

    /// MPS_EXCHANGE (Figure 34): `C` is in the bottom part and `A` has fallen
    /// below 0x8000. The MPS, unless the exchange gave that part to the LPS.
    fn mps_exchange(a: u32, contexts: &mut Contexts, cx: usize) -> bool {
        let st = contexts.st(cx);
        let row = TABLE[st];
        let mps = contexts.mps(cx);
        if a < u32::from(row.lsz) {
            contexts.set(cx, row.nlps, mps ^ row.swtch);
            !mps
        } else {
            contexts.set(cx, row.nmps, mps);
            mps
        }
    }

    /// RENORMD (Figure 35): double `A` and `C` until `A` is back at 0x8000 or
    /// over, a byte in whenever `CLOW` has run dry -- and once more at the end,
    /// so that `CT` never rests at zero.
    fn renorm(&mut self, scd: &mut Scd) {
        loop {
            if self.ct == 0 {
                self.byte_in(scd);
            }
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.a >= 0x8000 {
                break;
            }
        }
        if self.ct == 0 {
            self.byte_in(scd);
        }
    }

    /// BYTEIN (Figure 36). The figure's "BUFFER = 0?" is an assignment with a
    /// stray question mark: the byte once the data has run out is 0x00, as
    /// 6.8.3.8 says in words.
    fn byte_in(&mut self, scd: &mut Scd) {
        self.c += scd.next() << 8;
        self.ct = 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 7.1's PIX and CX, sixteen bits a word, the most significant first: "the
    /// raw data of a stripe in raster scan order and from MSB to LSB".
    const PIX: [u16; 16] = [
        0x05e0, 0x0000, 0x8b00, 0x01c4, 0x1700, 0x0034, 0x7fff, 0x1a3f,
        0x951b, 0x05d8, 0x1d17, 0xe770, 0x0000, 0x0000, 0x0656, 0x0e6a,
    ];
    const CX: [u16; 16] = [
        0x0fe0, 0x0000, 0x0f00, 0x00f0, 0xff00, 0x0000, 0x0000, 0x0000,
        0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    ];

    /// 7.1's SCD: 25 bytes, with the five zeros FINALWRITES leaves after the
    /// 0x91 already taken off.
    const SCD: [u8; 25] = [
        0x69, 0x89, 0x99, 0x5c, 0x32, 0xea, 0xfa, 0xa0, 0xd5, 0xff, 0x52, 0x7f, 0xff,
        0xff, 0xff, 0xc0, 0x00, 0x00, 0x00, 0x3f, 0xff, 0x2d, 0x20, 0x82, 0x91,
    ];

    fn bits(words: &[u16; 16]) -> Vec<bool> {
        words
            .iter()
            .flat_map(|&w| (0..16).rev().map(move |i| w >> i & 1 == 1))
            .collect()
    }

    #[test]
    fn table_24_holds_together() {
        // Every next state is a state, every LSZ is less than the smallest A,
        // and SWTCH is set in the ten states the rendered table sets it in.
        for (st, row) in TABLE.iter().enumerate() {
            assert!(usize::from(row.nlps) < TABLE.len(), "ST {st}");
            assert!(usize::from(row.nmps) < TABLE.len(), "ST {st}");
            assert!(row.lsz > 0 && row.lsz < 0x8000, "ST {st}");
        }
        let switches: Vec<usize> = (0..TABLE.len()).filter(|&st| TABLE[st].swtch).collect();
        assert_eq!(switches, [0, 14, 36, 64, 80, 88, 95, 105, 110, 112]);
    }

    #[test]
    fn the_encoder_gives_seven_one_scd() {
        let pix = bits(&PIX);
        let cx = bits(&CX);
        let mut contexts = Contexts::new(2);
        let mut encoder = Encoder::new();
        for (&p, &c) in pix.iter().zip(&cx) {
            encoder.encode(&mut contexts, usize::from(c), p);
        }
        assert_eq!(encoder.flush(), SCD);
    }

    /// Rows of Table 26, as the encoder has them before each event: EC, ST,
    /// A, C, CT, SC and BUF.
    const ENCODER_ROWS: [(usize, u8, u32, u32, u32, u32, u32); 9] = [
        (1, 0, 0x10000, 0x0000_0000, 11, 0, 0x00),
        (7, 14, 0x09618, 0x000c_a4a0, 6, 0, 0x00),
        (14, 3, 0x0fd6e, 0x0003_d6f8, 8, 0, 0x69),
        (39, 65, 0x0e834, 0x0009_7864, 7, 0, 0x89),
        (57, 66, 0x0e5e0, 0x0005_60a0, 7, 0, 0x99),
        (92, 77, 0x0a400, 0x001e_b180, 6, 0, 0xea),
        (231, 72, 0x0a120, 0x0040_b220, 4, 0, 0x20),
        (248, 104, 0x0f0f4, 0x000f_920c, 7, 1, 0x90),
        (256, 103, 0x082bc, 0x000f_c144, 7, 2, 0x90),
    ];

    #[test]
    fn the_encoder_registers_follow_table_26() {
        let pix = bits(&PIX);
        let cx = bits(&CX);
        let mut contexts = Contexts::new(2);
        let mut encoder = Encoder::new();
        let mut rows = ENCODER_ROWS.iter().peekable();
        for (i, (&p, &c)) in pix.iter().zip(&cx).enumerate() {
            let ec = i + 1;
            if let Some(&&(row, st, a, c_reg, ct, sc, buf)) = rows.peek()
                && row == ec
            {
                let cx = usize::from(c);
                assert_eq!(contexts.st(cx), usize::from(st), "ST at EC {ec}");
                assert_eq!(
                    (encoder.a, encoder.c, encoder.ct, encoder.sc, encoder.buffer),
                    (a, c_reg, ct, sc, buf),
                    "A, C, CT, SC and BUF at EC {ec}"
                );
                rows.next();
            }
            encoder.encode(&mut contexts, usize::from(c), p);
        }
        assert!(rows.next().is_none(), "a row was never reached");
        // The last row, 257: FINALWRITES, with C after its shift by CT, and
        // the 0x91 it writes.
        assert_eq!((encoder.a, encoder.ct, encoder.sc, encoder.buffer), (0x08c72, 6, 2, 0x90));
        encoder.clear_bits();
        let before = encoder.out.len();
        encoder.final_writes();
        assert_eq!(encoder.c, 0x0800_0000);
        assert_eq!(encoder.out[before], 0x91);
    }

    /// Rows of Table 26 as the decoder has them: EC, C and CT.
    const DECODER_ROWS: [(usize, u32, u32); 8] = [
        (1, 0x6989_9900, 8),
        (2, 0x6989_9900, 8),
        (8, 0xa1f4_4000, 2),
        (9, 0xb06d_5c00, 8),
        (56, 0xeaf9_0000, 1),
        (92, 0xa29a_a000, 3),
        (237, 0x66a8_0000, 1),
        (256, 0x3ebc_0000, 4),
    ];

    #[test]
    fn the_decoder_gives_seven_one_pix_and_follows_table_26() {
        let pix = bits(&PIX);
        let cx = bits(&CX);
        let mut contexts = Contexts::new(2);
        let mut scd = Scd { bytes: SCD.to_vec(), at: 0 };
        let mut decoder = Decoder::new(&mut scd);
        let mut rows = DECODER_ROWS.iter().peekable();
        let mut got = Vec::new();
        for (i, &c) in cx.iter().enumerate() {
            let ec = i + 1;
            if let Some(&&(row, c_reg, ct)) = rows.peek()
                && row == ec
            {
                assert_eq!((decoder.c, decoder.ct), (c_reg, ct), "C and CT at EC {ec}");
                rows.next();
            }
            got.push(decoder.decode(&mut contexts, usize::from(c), &mut scd));
        }
        assert!(rows.next().is_none(), "a row was never reached");
        assert_eq!(got, pix);
    }

    #[test]
    fn the_conditional_exchange_happens_where_table_26_marks_it() {
        // Table 26's CE column marks the events where the MPS's part of the
        // interval has come out smaller than the LPS's, so the two swap.
        // Checked from the encoder's side, on events marked and not.
        let pix = bits(&PIX);
        let cx = bits(&CX);
        let mut contexts = Contexts::new(2);
        let mut encoder = Encoder::new();
        let mut exchanged = Vec::new();
        for (i, (&p, &c)) in pix.iter().zip(&cx).enumerate() {
            let cx = usize::from(c);
            let lsz = u32::from(TABLE[contexts.st(cx)].lsz);
            let mps_part = encoder.a - lsz;
            if mps_part < lsz {
                exchanged.push(i + 1);
            }
            encoder.encode(&mut contexts, cx, p);
        }
        for ec in [2, 7, 65, 69, 71, 72, 252, 254, 256] {
            assert!(exchanged.contains(&ec), "no exchange at EC {ec}");
        }
        for ec in [1, 3, 37, 38, 253, 255] {
            assert!(!exchanged.contains(&ec), "an exchange at EC {ec}");
        }
    }

    #[test]
    fn a_stripe_that_ends_without_a_carry_still_decodes() {
        // FINALWRITES' second branch, which 7.1 never reaches. Stripes of every
        // length up to 400 decisions in three contexts: among them are
        // stripes that end in each of the two ways -- and the ones that end
        // without a carry decode wrongly if BUFFER goes out one higher, as the
        // figure prints it, so the reading is the one the decoder needs.
        let decode = |scd: Vec<u8>, cx: &[usize]| -> Vec<bool> {
            let mut contexts = Contexts::new(3);
            let mut scd = Scd { bytes: scd, at: 0 };
            let mut decoder = Decoder::new(&mut scd);
            cx.iter().map(|&c| decoder.decode(&mut contexts, c, &mut scd)).collect()
        };
        let mut carried = [false; 2];
        let mut misread_fails = 0;
        for len in 1..400usize {
            let pix: Vec<bool> = (0..len).map(|i| (i * 7 + len) % 13 == 0 || i % 29 == 3).collect();
            let cx: Vec<usize> = (0..len).map(|i| (i / 5) % 3).collect();
            let mut contexts = Contexts::new(3);
            let mut encoder = Encoder::new();
            for (&p, &c) in pix.iter().zip(&cx) {
                encoder.encode(&mut contexts, c, p);
            }
            let mut probe = encoder.clone();
            probe.clear_bits();
            let carry = probe.c << probe.ct > 0x7ff_ffff;
            carried[usize::from(carry)] = true;
            // Where FINALWRITES puts BUFFER, once FLUSH has taken the first
            // byte off -- unless nothing has gone out yet, when BUFFER is
            // that first byte and goes either way.
            let buffer_at = probe.out.len().checked_sub(1);
            let scd = encoder.flush();
            assert_eq!(decode(scd.clone(), &cx), pix, "length {len}");
            if let (false, Some(buffer_at)) = (carry, buffer_at) {
                let mut misread = scd;
                misread.resize(misread.len().max(buffer_at + 1), 0);
                misread[buffer_at] += 1;
                misread_fails += usize::from(decode(misread, &cx) != pix);
            }
        }
        assert_eq!(carried, [true, true], "only one way of ending was tried");
        assert!(misread_fails > 0, "BUFFER + 1 made no difference");
    }
}
