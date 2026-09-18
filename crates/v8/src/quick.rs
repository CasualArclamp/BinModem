//! V.92's quick connect: the four QC sequences, in V.8's own framing.
//!
//! V.8 costs a call about a second and a half before any modem has trained.
//! The answering modem has to be heard, the calling modem then sends CM over
//! and over until a JM comes back, and only then does CJ end the conversation
//! and the start-up begin. None of that is wasted -- it is what stops two
//! modems talking past each other -- but it is spent again on every call to a
//! server this modem has already recognised.
//!
//! V.92's short Phase 1 cuts it to two sequences. The calling modem says what
//! it wants in the first 200 ms of V.21 traffic, before CM; the answering
//! modem replies with one 233 ms sequence and stops; and both go straight to
//! the training. Four sequences carry it, one for each end in each role:
//! QC1a and QCA1a from an analogue modem (8.2.1, 8.2.3) and QC1d and QCA1d
//! from a digital one (8.3.2, 8.3.4).
//!
//! They ride in V.8's own coding format -- ten ONEs, a ten-bit
//! synchronisation, then start/stop framed octets -- which is why they are
//! here rather than in the pump: everything about them except the FSK is
//! octets. Table 1/V.8 reserves the synchronisation `0101010101` for V.92, so
//! a modem that has never heard of V.92 meets an unknown sequence, ignores it
//! (clause 6/V.8: "a receiver shall ignore all bits, codes and octets
//! reserved for such future definition") and reads the CM that follows. That
//! is the whole safety argument for sending QC1a to a stranger.
//!
//! What is here is those four sequences as data -- the information octet, the
//! whole 60 or 70 bits, and a watcher that finds one in a stream of bits. No
//! line, no timers and no V.21: the channel each sequence belongs on is named
//! but its frequencies are not, because `datapump::v8` already has them.
//!
//! The watcher is deliberately not [`crate::Decoder`]. A QC information octet
//! can equal the CI synchronisation (`0x00`, with P = 0 and WXYZ = `0000`) or
//! the CM and JM synchronisation (`0xE0`, with P = 0 and WXYZ = `0111`), and
//! `0x55` is itself a legal V.8 modulation extension octet. Nothing in the
//! octets tells these apart. The only thing that marks a QC is *where* the
//! `0x55` sits: directly after a run of at least ten ONEs. A parser that
//! hunts for synchronisation octets anywhere in the stream misreads them, so
//! this one anchors every sequence to its preamble and never looks anywhere
//! else.
//!
//! The V.8 bis pair of each sequence -- QC2a, QCA2a, QC2d, QCA2d, which are
//! V.8 bis messages rather than V.8 frames -- is not here. This modem has no
//! V.8 bis to send them on.

use crate::PREAMBLE_ONES;

/// The synchronisation both copies of a QC or a QCA open with, as an octet.
///
/// Table 1/V.8 prints it as the bit pattern `0101010101` against the words
/// "Defined in ITU-T V.92", and Tables 2, 4, 11 and 13/V.92 print the same ten
/// bits at positions 10:19. Framed the way V.8 frames an octet -- start bit,
/// `b0` to `b7` least significant first, stop bit -- those ten bits are a
/// start bit, `0x55`, and a stop bit.
pub const SYNC_QC: u8 = 0x55;

/// How many bits a QC1a or QC1d is (Tables 2 and 11: bits 0:59).
///
/// 200 ms at the 300 bit/s of 8.2, after which CM begins with no gap at all.
pub const QC_BITS: usize = 60;

/// How many bits a QCA1a or QCA1d is (Tables 4 and 13: bits 0:69).
///
/// 233.3 ms. The extra ten are a closing run of ONEs, which the QCs do not
/// need because the ten ONEs that open CM close them instead.
pub const QCA_BITS: usize = 70;

/// The bit a QC is accepted on, counted from the first of its ten ONEs.
///
/// Bit 59 is the stop bit of the second copy of the information frame, which
/// is the last bit that carries anything. A QCA's trailing ten ONEs are not
/// waited for: they are a run of marks, and a receiver that insisted on them
/// would be trusting the one part of the sequence that a jitter slip is most
/// likely to smear.
pub const ACCEPT_AT_BIT: usize = QC_BITS - 1;

/// Whether both copies of the information frame must agree before a QC is
/// believed.
///
/// An ambiguous reading. Tables 2, 4, 11 and 13 print bits 50:59 as "Bits
/// 20:29 repeated", and 8.2.1 says QC1a "is transmitted once", but no clause
/// says what a receiver must have in hand before it acts on one. The reading
/// taken here, and the printed shape of the signal read strictly, is: both
/// frames well formed, both equal, accepted on [`ACCEPT_AT_BIT`].
///
/// The alternative reading is one good copy behind a good preamble and
/// synchronisation, which would survive a jitter slip landing in the other
/// copy; setting this to `false` takes it, and leaves the preamble and both
/// synchronisations still required. The strict reading is the default because
/// the two failures are not the same size: a QC invented out of noise costs
/// the whole call, while a QC missed costs only the V.8 fallback that is
/// already following it down the line.
pub const BOTH_COPIES_MUST_AGREE: bool = true;

/// Which of V.21's two channels a sequence is modulated on (8.2, 8.3).
///
/// The frequencies are not here. They belong to whatever drives the line, and
/// `datapump::v8` has them already; this module has no line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// V.21(L), the low-band channel. CM, CI and CJ go on it, so the calling
    /// modem's QC1a and QC1d share it and follow straight into CM.
    Low,
    /// V.21(H), the high-band channel. JM goes on it, so the answering
    /// modem's QCA1a and QCA1d share it.
    High,
}

/// Which of the four V.8-initiated quick-connect sequences this is.
///
/// The name is the Recommendation's: "QC" asks for a quick connect and "QCA"
/// acknowledges one, the `1` means the call was started under V.8 rather than
/// V.8 bis, and the `a` or `d` says whether the analogue or the digital modem
/// is speaking. Bits 21 and 22 of the sequence carry exactly those last two
/// distinctions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 8.2.1, Table 2: the analogue modem calling, before its CM.
    Qc1a,
    /// 8.2.3, Table 4: the analogue modem answering.
    Qca1a,
    /// 8.3.2, Table 11: the digital modem calling, before its CM.
    Qc1d,
    /// 8.3.4, Table 13: the digital modem answering.
    Qca1d,
}

impl Kind {
    /// The name the Recommendation gives this sequence.
    pub fn name(self) -> &'static str {
        match self {
            Self::Qc1a => "QC1a",
            Self::Qca1a => "QCA1a",
            Self::Qc1d => "QC1d",
            Self::Qca1d => "QCA1d",
        }
    }

    /// Which V.21 channel it is sent on.
    ///
    /// 8.2.1 and 8.3.2 put the two QCs on V.21(L) and 8.2.3 and 8.3.4 put the
    /// two QCAs on V.21(H) -- the same split as CM and JM, and for the same
    /// reason: each end listens on the channel it is not using, so a modem
    /// never hears its own quick connect come back.
    pub fn channel(self) -> Channel {
        match self {
            Self::Qc1a | Self::Qc1d => Channel::Low,
            Self::Qca1a | Self::Qca1d => Channel::High,
        }
    }

    /// Bit 21: whether the digital modem is the one speaking.
    pub fn digital(self) -> bool {
        matches!(self, Self::Qc1d | Self::Qca1d)
    }

    /// Bit 22: whether this acknowledges a quick connect rather than asking
    /// for one.
    pub fn acknowledgement(self) -> bool {
        matches!(self, Self::Qca1a | Self::Qca1d)
    }

    /// How many bits the whole sequence is: [`QC_BITS`] or [`QCA_BITS`].
    pub fn length(self) -> usize {
        if self.acknowledgement() { QCA_BITS } else { QC_BITS }
    }

    /// The `b0` and `b1` of the information octet, which is all of it that
    /// the kind decides.
    fn octet_bits(self) -> u8 {
        u8::from(self.digital()) | (u8::from(self.acknowledgement()) << 1)
    }
}

/// The fifteen Ucodes of Table 2, in the order the table lists their patterns.
///
/// Read from the rendered page, not the extracted text. They are not a
/// sequence with a rule: Table 1/V.90's numbering climbs in chords of sixteen
/// and this picks fifteen codes out of chords 3, 4 and 5, from Ucode 61
/// (mu-law linear 1756) to Ucode 87 (5884). The index is the WXYZ pattern
/// read as a number, W most significant.
const UCODES: [u8; 15] = [61, 62, 63, 66, 67, 70, 71, 74, 75, 78, 79, 82, 83, 86, 87];

/// One of the fifteen Ucodes Table 2 lists, and so one U_QTS can ask for.
///
/// It holds the WXYZ pattern, not the Ucode, which is what makes
/// [`Uqts::pattern`] total: there is no way to build a `Uqts::Ucode` around a
/// Ucode with no pattern to be sent as. That matters because the Ucode a
/// modem wants comes from outside -- the memo of the last call to this server
/// -- and a value Table 2 does not list has to be turned away at
/// [`Table2Ucode::new`], where the caller can choose a neighbour, rather than
/// half way through building a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table2Ucode(u8);

impl Table2Ucode {
    /// The code for a Ucode, if Table 2 lists it. Fifteen of the hundred and
    /// twenty-eight can be asked for, and a modem that wants one of the other
    /// hundred and thirteen has to settle for a neighbour.
    pub fn new(ucode: u8) -> Option<Self> {
        UCODES.iter().position(|&c| c == ucode).map(|p| Self(p as u8))
    }

    /// The Ucode itself, as Table 1/V.90 numbers it.
    pub fn get(self) -> u8 {
        UCODES[usize::from(self.0)]
    }

    /// The WXYZ pattern that asks for it, W in the most significant bit.
    fn pattern(self) -> u8 {
        self.0
    }
}

/// U_QTS: which PCM codeword the digital modem is to use for QTS (Table 2).
///
/// The analogue modem chooses it, in QC1a or QCA1a, and the digital modem
/// obeys. It is the one number in short Phase 1 that a modem picks rather
/// than reports, and V.92 gives no rule for picking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uqts {
    /// One of the fifteen Ucodes Table 2 lists. Build it with
    /// [`Uqts::from_ucode`] or [`Uqts::from_pattern`]: a Ucode the table does
    /// not list has no pattern to be sent as, and [`Table2Ucode`] is what
    /// keeps one out of here.
    Ucode(Table2Ucode),
    /// The sixteenth pattern, `1111`, which Table 2 gives no Ucode at all:
    /// "Cleardown from on-hold state". 9.10.2.1: "If signal QC is detected
    /// with the UQTS code set to 1111, cleardown from on-hold state, the
    /// modem shall disconnect." It is how a modem hangs up a call it left on
    /// hold, and it must never be sent as an ordinary quick connect.
    Cleardown,
}

impl Uqts {
    /// Read the WXYZ pattern, W in the most significant of the four bits.
    ///
    /// Clause 8: "values given as bit patterns are transmitted leftmost bit
    /// first in time". W is leftmost in Table 2 and so first on the line, but
    /// it is the *top* bit of the number here, which is the trap: `0001` is
    /// Ucode 62, not Ucode 75.
    pub fn from_pattern(pattern: u8) -> Option<Self> {
        match pattern {
            0b1111 => Some(Self::Cleardown),
            p if p < 0b1111 => Some(Self::Ucode(Table2Ucode(p))),
            _ => None,
        }
    }

    /// The code for a Ucode, if Table 2 lists it. See [`Table2Ucode::new`].
    pub fn from_ucode(ucode: u8) -> Option<Self> {
        Table2Ucode::new(ucode).map(Self::Ucode)
    }

    /// The WXYZ pattern, W in the most significant of the four bits.
    pub fn pattern(self) -> u8 {
        match self {
            Self::Ucode(u) => u.pattern(),
            Self::Cleardown => 0b1111,
        }
    }

    /// The Ucode, unless this is the cleardown code, which names none.
    pub fn ucode(self) -> Option<u8> {
        match self {
            Self::Ucode(u) => Some(u.get()),
            Self::Cleardown => None,
        }
    }
}

/// LM: the level the digital modem will send ANSpcm at (Table 11).
///
/// Unlike U_QTS this is a report, not a request. The digital modem has
/// already chosen, and tells the analogue modem so that its receiver knows
/// what amplitude to expect from a signal that arrives before either end has
/// measured anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnspcmLevel {
    /// `00`, -9.5 dBm0.
    Minus9_5,
    /// `01`, -12 dBm0.
    Minus12,
    /// `10`, -15 dBm0.
    Minus15,
    /// `11`, -18 dBm0.
    Minus18,
}

impl AnspcmLevel {
    /// Read the LM pattern, L in the more significant of the two bits.
    ///
    /// L is leftmost in Table 11 and so first on the line, as clause 8 has
    /// it.
    pub fn from_pattern(pattern: u8) -> Option<Self> {
        match pattern {
            0b00 => Some(Self::Minus9_5),
            0b01 => Some(Self::Minus12),
            0b10 => Some(Self::Minus15),
            0b11 => Some(Self::Minus18),
            _ => None,
        }
    }

    /// The LM pattern, L in the more significant of the two bits.
    pub fn pattern(self) -> u8 {
        match self {
            Self::Minus9_5 => 0b00,
            Self::Minus12 => 0b01,
            Self::Minus15 => 0b10,
            Self::Minus18 => 0b11,
        }
    }

    /// The level itself, in dBm0 (Table 11).
    pub fn dbm0(self) -> f64 {
        match self {
            Self::Minus9_5 => -9.5,
            Self::Minus12 => -12.0,
            Self::Minus15 => -15.0,
            Self::Minus18 => -18.0,
        }
    }

    /// `scl` for a mu-law network (Table 6), the amplitude the 301-codeword
    /// ANSpcm sequence is generated at.
    ///
    /// The two laws get their own method rather than one taking a law,
    /// because the type that names a companding law lives in `datapump` and
    /// this crate is underneath it.
    ///
    /// 8.3.1 generates the sequence as
    /// `x = floor(scl * sqrt(2) * cos(2*pi*k*79/301 + theta) + 0.5)` and then
    /// quantises x "to a linear PCM value according to ITU-T G.711", so scl
    /// is the RMS amplitude on *G.711's own* linear scale -- magnitudes to
    /// 8159 on mu-law and to 4096 on A-law -- and that is why Table 6 prints
    /// one law at exactly twice the other.
    ///
    /// It is not the 16-bit scale Table 1/V.90 prints, which is four times
    /// G.711's on mu-law and eight times it on A-law. On that scale the two
    /// laws are within half a percent of each other -- Ucode 127 is 32124 and
    /// 32256 -- so a 2:1 ratio could not arise there at all. Generating
    /// ANSpcm against Table 1/V.90's column would send it 12 dB too quiet on
    /// mu-law and 18 dB too quiet on A-law, and the octets would not be the
    /// ones Tables 7 to 10 print.
    pub fn scl_mu_law(self) -> u16 {
        match self {
            Self::Minus9_5 => 1334,
            Self::Minus12 => 1000,
            Self::Minus15 => 708,
            Self::Minus18 => 500,
        }
    }

    /// `scl` for an A-law network (Table 6). See [`AnspcmLevel::scl_mu_law`].
    pub fn scl_a_law(self) -> u16 {
        match self {
            Self::Minus9_5 => 667,
            Self::Minus12 => 500,
            Self::Minus15 => 354,
            Self::Minus18 => 250,
        }
    }
}

/// What bits 24:28 of a sequence carry, which depends on who is speaking.
///
/// The analogue modem's sequences put U_QTS there as `W 0 X Y Z`; the digital
/// modem's put LM there as `0 0 0 L M`. Both leave bit 25 -- V.8's `b4` --
/// zero, which is the rule in 5.1/V.8 that stops a category octet simulating
/// an HDLC flag, and which is why the two layouts are not simply four bits
/// and two bits in the same place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// U_QTS, from the analogue modem (Tables 2 and 4).
    Uqts(Uqts),
    /// LM, from the digital modem (Tables 11 and 13).
    Level(AnspcmLevel),
}

impl Field {
    /// The `b3` to `b7` of the information octet.
    fn octet_bits(self) -> u8 {
        match self {
            // Table 2 bits 24:29, "W0XYZ1": W at b3, the fixed zero at b4,
            // then X, Y and Z at b5, b6 and b7. WXYZ is not contiguous in the
            // octet, and the gap is the whole point of it.
            Self::Uqts(uqts) => {
                let p = uqts.pattern();
                let (w, x, y, z) = ((p >> 3) & 1, (p >> 2) & 1, (p >> 1) & 1, p & 1);
                (w << 3) | (x << 5) | (y << 6) | (z << 7)
            }
            // Table 11 bits 24:29, "000LM1": three zeros, then L and M at b6
            // and b7.
            Self::Level(level) => {
                let p = level.pattern();
                (((p >> 1) & 1) << 6) | ((p & 1) << 7)
            }
        }
    }
}

/// One quick-connect sequence.
///
/// Everything a QC says fits in one V.8 octet: two bits for who is speaking
/// and what they want, one for the protocol, and five for the field. The rest
/// of the 60 or 70 bits is preamble, synchronisation and the repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Qc {
    /// Which sequence this is, which is bits 21 and 22.
    pub kind: Kind,
    /// Bit 23, P. Tables 2, 4, 11 and 13: "Set to 1 calls for LAPM protocol
    /// according to ITU-T V.42 (see 9.2.5)". When both ends set it the V.42
    /// detection phase is skipped, which is the rest of the second that
    /// quick connect is trying to save.
    pub lapm: bool,
    /// Bits 24:28. The kind decides which of the two layouts belongs here;
    /// the constructors pair them, and [`Qc::from_octet`] never returns a
    /// mismatched pair.
    pub field: Field,
}

impl Qc {
    /// QC1a: the analogue modem calling (8.2.1).
    pub fn qc1a(uqts: Uqts, lapm: bool) -> Self {
        Self { kind: Kind::Qc1a, lapm, field: Field::Uqts(uqts) }
    }

    /// QCA1a: the analogue modem answering (8.2.3).
    pub fn qca1a(uqts: Uqts, lapm: bool) -> Self {
        Self { kind: Kind::Qca1a, lapm, field: Field::Uqts(uqts) }
    }

    /// QC1d: the digital modem calling (8.3.2).
    pub fn qc1d(level: AnspcmLevel, lapm: bool) -> Self {
        Self { kind: Kind::Qc1d, lapm, field: Field::Level(level) }
    }

    /// QCA1d: the digital modem answering (8.3.4).
    pub fn qca1d(level: AnspcmLevel, lapm: bool) -> Self {
        Self { kind: Kind::Qca1d, lapm, field: Field::Level(level) }
    }

    /// The information octet, bits 21 to 28, as V.8 frames one.
    ///
    /// 5.1/V.8 lays a framed octet out as `start-bit (0) b0 b1 b2 b3 0 b5 b6
    /// b7 stop-bit (1)`, and bit 20 of a QC is the start bit, so bit 21 is
    /// `b0` and bit 28 is `b7`. Bits 21 and 22 come from the kind, bit 23
    /// from `lapm` and bits 24 to 28 from the field -- and `b4`, which is bit
    /// 25, is zero in both field layouts.
    pub fn octet(&self) -> u8 {
        self.kind.octet_bits() | (u8::from(self.lapm) << 2) | self.field.octet_bits()
    }

    /// Read an information octet back, if it can be one.
    ///
    /// The checks are the fixed zeros the tables print. `b4` must be zero in
    /// every sequence, because bit 25 is printed as `0` in all four tables
    /// and 5.1/V.8 requires it of a category octet. The digital sequences
    /// must also have `b3` and `b5` zero, which are the first two zeros of
    /// `000LM1`; the analogue ones carry W and X there and so cannot be
    /// checked at all.
    pub fn from_octet(octet: u8) -> Option<Self> {
        let bit = |n: u8| octet & (1 << n) != 0;
        if bit(4) {
            return None;
        }
        let kind = match (bit(0), bit(1)) {
            (false, false) => Kind::Qc1a,
            (false, true) => Kind::Qca1a,
            (true, false) => Kind::Qc1d,
            (true, true) => Kind::Qca1d,
        };
        let field = if kind.digital() {
            if bit(3) || bit(5) {
                return None;
            }
            Field::Level(AnspcmLevel::from_pattern((u8::from(bit(6)) << 1) | u8::from(bit(7)))?)
        } else {
            let pattern =
                (u8::from(bit(3)) << 3) | (u8::from(bit(5)) << 2) | (u8::from(bit(6)) << 1) | u8::from(bit(7));
            Field::Uqts(Uqts::from_pattern(pattern)?)
        };
        Some(Self { kind, lapm: bit(2), field })
    }

    /// The whole sequence, every bit of it, in the order it goes on the line.
    ///
    /// Sixty bits for a QC and seventy for a QCA, the runs of ONEs included,
    /// so a transmitter can queue the lot and think about nothing else. They
    /// are not all octets -- the ten ONEs between the two copies are the idle
    /// line, not a frame -- which is why this is bits and not the `Vec<u8>`
    /// that [`crate::sequence`] returns for CM.
    pub fn bits(&self) -> Vec<bool> {
        let octet = self.octet();
        let mut out = Vec::with_capacity(self.kind.length());
        out.extend([true; PREAMBLE_ONES]);
        out.extend(frame(SYNC_QC));
        out.extend(frame(octet));
        out.extend([true; PREAMBLE_ONES]);
        out.extend(frame(SYNC_QC));
        out.extend(frame(octet));
        // Tables 4 and 13 bits 60:69. A QC has none: 8.2.1 and 8.3.2 have it
        // "followed immediately by CM", and CM opens with ten ONEs of its own.
        if self.kind.acknowledgement() {
            out.extend([true; PREAMBLE_ONES]);
        }
        out
    }
}

/// One octet as V.8 frames it: start bit, `b0` to `b7`, stop bit.
fn frame(octet: u8) -> [bool; 10] {
    let mut out = [false; 10];
    for (n, slot) in out[1..9].iter_mut().enumerate() {
        *slot = octet & (1 << n) != 0;
    }
    out[9] = true;
    out
}

/// Finds a quick-connect sequence in a stream of demodulated bits.
///
/// It keeps the last sixty bits and asks, on every one of them, whether they
/// are a whole QC: ten ONEs, the synchronisation, an information frame, ten
/// more ONEs, the synchronisation again and the same frame again. Nothing is
/// remembered between attempts and there is no state to get stuck in, which
/// matters because the alternative -- a state machine that latches onto a
/// preamble -- can be walked out of step by one jitter slip and then has to
/// be taught how to recover.
///
/// Anchoring the whole window is also what keeps it honest about the octets
/// that collide with V.8's own. A `0x55` in the middle of a CM has an octet
/// behind it, not ten ONEs, so it is never mistaken for the synchronisation;
/// and an information octet that happens to equal `0x00` or `0xE0` is read as
/// what it is, because nothing here looks at octets on their own.
#[derive(Debug, Clone, Default)]
pub struct BitWatcher {
    /// The last [`QC_BITS`] bits, the most recent in the least significant
    /// place.
    history: u64,
    /// How many bits have arrived since the last sequence was reported, up to
    /// a full window.
    filled: usize,
}

/// The window as a mask, since only the low [`QC_BITS`] bits of the history
/// mean anything.
const WINDOW: u64 = (1 << QC_BITS) - 1;

impl BitWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// One demodulated bit, `true` for a ONE. Returns a sequence the moment
    /// its [`ACCEPT_AT_BIT`] arrives.
    pub fn feed(&mut self, bit: bool) -> Option<Qc> {
        self.history = ((self.history << 1) | u64::from(bit)) & WINDOW;
        self.filled = (self.filled + 1).min(QC_BITS);
        if self.filled < QC_BITS {
            return None;
        }
        let found = self.whole_sequence();
        if found.is_some() {
            // Start again rather than let the tail of this sequence be the
            // head of the next candidate. A QC is followed by CM and a QCA by
            // silence, and neither is worth scanning against leftovers.
            *self = Self::new();
        }
        found
    }

    /// Bit `position` of the window, counting as the tables count: 0 is the
    /// first of the ten ONEs and 59 is the last bit to arrive.
    fn at(&self, position: usize) -> bool {
        self.history >> (ACCEPT_AT_BIT - position) & 1 == 1
    }

    /// The ten bits at `position` read as a framed octet, if they are one.
    fn frame(&self, position: usize) -> Option<u8> {
        // 5.1/V.8: the start bit is a ZERO and the stop bit a ONE. A frame
        // that fails either is not a frame, and a receiver that took the
        // eight bits anyway would be reading a QC out of the middle of
        // something else.
        if self.at(position) || !self.at(position + 9) {
            return None;
        }
        Some((1..=8).fold(0u8, |acc, n| acc | (u8::from(self.at(position + n)) << (n - 1))))
    }

    /// The window read as a whole sequence, if it is one.
    fn whole_sequence(&self) -> Option<Qc> {
        // Bits 0:9 and 30:39: "Ten ONEs". These are what anchor the two
        // synchronisations, and without them the rest of this would find QCs
        // inside CMs.
        let ones = |from: usize| (from..from + PREAMBLE_ONES).all(|p| self.at(p));
        if !ones(0) || !ones(30) {
            return None;
        }
        if self.frame(10)? != SYNC_QC || self.frame(40)? != SYNC_QC {
            return None;
        }
        let first = self.frame(20)?;
        let second = self.frame(50)?;
        if BOTH_COPIES_MUST_AGREE && first != second {
            return None;
        }
        Qc::from_octet(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SYNC_CI, SYNC_MENU};

    /// Bits as the tables print them, so a test can be compared with the page.
    fn text(bits: &[bool]) -> String {
        bits.iter().map(|&b| if b { '1' } else { '0' }).collect()
    }

    /// A printed bit string with its grouping spaces taken out.
    fn printed(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Everything a watcher finds in a stream of bits, with some silence in
    /// front so that nothing depends on the stream starting at bit 0.
    fn heard(bits: &[bool]) -> Vec<Qc> {
        let mut watcher = BitWatcher::new();
        let mut out = Vec::new();
        for &bit in [false; 7].iter().chain(bits) {
            if let Some(qc) = watcher.feed(bit) {
                out.push(qc);
            }
        }
        out
    }

    /// Every sequence this module can build, for the tests that want to sweep
    /// the lot.
    fn all_sequences() -> Vec<Qc> {
        let mut out = Vec::new();
        for lapm in [false, true] {
            for pattern in 0..16 {
                let uqts = Uqts::from_pattern(pattern).unwrap();
                out.push(Qc::qc1a(uqts, lapm));
                out.push(Qc::qca1a(uqts, lapm));
            }
            for pattern in 0..4 {
                let level = AnspcmLevel::from_pattern(pattern).unwrap();
                out.push(Qc::qc1d(level, lapm));
                out.push(Qc::qca1d(level, lapm));
            }
        }
        out
    }

    #[test]
    fn every_table_2_code_names_its_ucode_and_1111_is_cleardown() {
        // Table 2/V.92, read from the rendered page. The patterns are sent W
        // first (clause 8), so `0001` is the second row and not the ninth --
        // which is the mistake this table exists to catch.
        let table = [
            (0b0000, 61),
            (0b0001, 62),
            (0b0010, 63),
            (0b0011, 66),
            (0b0100, 67),
            (0b0101, 70),
            (0b0110, 71),
            (0b0111, 74),
            (0b1000, 75),
            (0b1001, 78),
            (0b1010, 79),
            (0b1011, 82),
            (0b1100, 83),
            (0b1101, 86),
            (0b1110, 87),
        ];
        for (pattern, ucode) in table {
            let uqts = Uqts::from_pattern(pattern).expect("a pattern Table 2 lists");
            assert_eq!(uqts.ucode(), Some(ucode), "pattern {pattern:04b}");
            assert_eq!(uqts.pattern(), pattern, "and back again");
            assert_eq!(Uqts::from_ucode(ucode), Some(uqts), "found by its Ucode");
            assert_eq!(Table2Ucode::new(ucode).map(Uqts::Ucode), Some(uqts));
        }
        // The sixteenth row has no Ucode at all: "Cleardown from on-hold
        // state". 9.10.2.1: "If signal QC is detected with the UQTS code set
        // to 1111 ... the modem shall disconnect."
        let cleardown = Uqts::from_pattern(0b1111).expect("the sixteenth row");
        assert_eq!(cleardown, Uqts::Cleardown);
        assert_eq!(cleardown.ucode(), None, "no codeword to send QTS at");
        assert_eq!(cleardown.pattern(), 0b1111);
        assert_eq!(Uqts::from_pattern(0b1_0000), None, "there are only sixteen");
        // A Ucode beside one of Table 2's is not one of Table 2's.
        for ucode in [0, 60, 64, 65, 88, 127] {
            assert_eq!(Uqts::from_ucode(ucode), None, "Ucode {ucode} is not in Table 2");
        }
    }

    #[test]
    fn a_ucode_table_2_does_not_list_is_turned_away_before_a_sequence_is_built() {
        // Table 2 gives patterns to fifteen of the hundred and twenty-eight
        // Ucodes. The other hundred and thirteen have no pattern at all, and
        // the place to find that out is here -- not part-way through building
        // a QC1a out of a U_QTS remembered from an earlier call, where the
        // only honest answers left would be a panic or a Ucode nobody asked
        // for. Table2Ucode is the gate, and it is the only way into
        // Uqts::Ucode.
        for ucode in 0..=u8::MAX {
            let listed = UCODES.contains(&ucode);
            assert_eq!(Table2Ucode::new(ucode).is_some(), listed, "Ucode {ucode}");
            assert_eq!(Uqts::from_ucode(ucode).is_some(), listed);
            if let Some(code) = Table2Ucode::new(ucode) {
                assert_eq!(code.get(), ucode, "and it remembers which one it is");
            }
        }
        // Every Uqts there is has a pattern, an octet and a sequence, and
        // none of the three can fail.
        for pattern in 0..16u8 {
            let uqts = Uqts::from_pattern(pattern).expect("sixteen patterns");
            assert_eq!(uqts.pattern(), pattern);
            let qc = Qc::qc1a(uqts, true);
            assert_eq!(Qc::from_octet(qc.octet()), Some(qc));
            assert_eq!(qc.bits().len(), QC_BITS);
        }
    }

    #[test]
    fn lm_codes_name_the_four_anspcm_levels() {
        // Table 11/V.92, and Table 6/V.92 for scl, both read from the
        // rendered pages. L is first in time and so the upper bit here.
        let table = [
            (0b00, AnspcmLevel::Minus9_5, -9.5, 1334, 667),
            (0b01, AnspcmLevel::Minus12, -12.0, 1000, 500),
            (0b10, AnspcmLevel::Minus15, -15.0, 708, 354),
            (0b11, AnspcmLevel::Minus18, -18.0, 500, 250),
        ];
        for (pattern, level, dbm0, mu, a) in table {
            assert_eq!(AnspcmLevel::from_pattern(pattern), Some(level));
            assert_eq!(level.pattern(), pattern);
            assert_eq!(level.dbm0(), dbm0);
            assert_eq!(level.scl_mu_law(), mu, "Table 6 mu-law scl");
            assert_eq!(level.scl_a_law(), a, "Table 6 A-law scl");
            // Table 6 prints the A-law column at exactly half the mu-law one,
            // which is the ratio of the two G.711 scales 8.3.1 quantises to
            // (magnitudes to 8159 and to 4096). It is not the ratio of Table
            // 1/V.90's linear columns, which print 32124 and 32256 at Ucode
            // 127 -- within half a percent of each other, where no 2:1 ratio
            // could come from. A generator that read scl against that column
            // would send ANSpcm 12 dB and 18 dB too quiet.
            assert_eq!(mu, 2 * a, "Table 6: {mu} is twice {a}");
        }
        assert_eq!(AnspcmLevel::from_pattern(0b100), None, "there are only four");

        // And scl is an RMS amplitude on one linear scale, so the ratio
        // between two rows is the gap between the levels they name: 2.5 dB
        // from -9.5 to -12, then 3 dB, then 3 dB, on both laws. Within
        // 0.05 dB, which is all the rounding in four printed integers leaves.
        let steps = [
            (AnspcmLevel::Minus9_5, AnspcmLevel::Minus12),
            (AnspcmLevel::Minus12, AnspcmLevel::Minus15),
            (AnspcmLevel::Minus15, AnspcmLevel::Minus18),
        ];
        for (louder, quieter) in steps {
            let printed = louder.dbm0() - quieter.dbm0();
            for (l, q) in [
                (louder.scl_mu_law(), quieter.scl_mu_law()),
                (louder.scl_a_law(), quieter.scl_a_law()),
            ] {
                let measured = 20.0 * (f64::from(l) / f64::from(q)).log10();
                assert!(
                    (measured - printed).abs() < 0.05,
                    "{l} over {q} is {measured} dB, not the printed {printed}"
                );
            }
        }
    }

    #[test]
    fn qc1a_with_p_and_wxyz_0101_is_the_printed_60_bits_and_octet_0xa4() {
        // Table 2/V.92 with P = 1 and WXYZ = 0101, which is Ucode 70. The
        // information frame is bits 20:29, "000PW0XYZ1" = 0001001011, and the
        // whole sequence is those sixty bits.
        let qc = Qc::qc1a(Uqts::from_ucode(70).unwrap(), true);
        assert_eq!(qc.octet(), 0xA4);
        assert_eq!(qc.kind.length(), QC_BITS);
        let bits = qc.bits();
        assert_eq!(bits.len(), 60, "200 ms at 300 bit/s");
        assert_eq!(
            text(&bits),
            printed("1111111111 0101010101 0001001011 1111111111 0101010101 0001001011")
        );
        // The synchronisation is the framed 0x55 Table 1/V.8 reserves for
        // V.92, and it is there twice.
        assert_eq!(text(&bits[10..20]), text(&frame(SYNC_QC)));
        assert_eq!(text(&bits[40..50]), text(&frame(SYNC_QC)));
        assert_eq!(heard(&bits), vec![qc], "and a watcher reads it back");
        assert_eq!(qc.kind.channel(), Channel::Low, "8.2.1: V.21(L), like the CM after it");
    }

    #[test]
    fn qca1a_is_70_bits_ending_in_ten_ones_and_octet_0xa6() {
        // Table 4/V.92 with the same fields as Table 2's worked case. It
        // differs from QC1a in bit 22 alone -- b1 of the octet, so 0xA4
        // becomes 0xA6 -- and in the ten ONEs at bits 60:69, which are there
        // because 9.2.3.1 follows QCA1a with silence rather than with CM.
        let qca = Qc::qca1a(Uqts::from_ucode(70).unwrap(), true);
        assert_eq!(qca.octet(), 0xA6);
        assert_eq!(qca.kind.length(), QCA_BITS);
        let bits = qca.bits();
        assert_eq!(bits.len(), 70, "233.3 ms at 300 bit/s");
        assert_eq!(
            text(&bits),
            printed("1111111111 0101010101 0011001011 1111111111 0101010101 0011001011 1111111111")
        );
        assert!(bits[60..].iter().all(|&b| b), "Table 4 bits 60:69, ten ONEs");
        assert_eq!(qca.kind.channel(), Channel::High, "8.2.3: V.21(H), like JM");
        assert_eq!(heard(&bits), vec![qca]);
    }

    #[test]
    fn a_qc_is_accepted_on_bit_59_before_a_qca_trailing_ones_arrive() {
        // The reading in BOTH_COPIES_MUST_AGREE: accepted on the stop bit of
        // the second copy. A QCA's last ten bits are marks, and waiting for
        // them would cost 33 ms of a procedure whose whole point is the time
        // it saves.
        let qca = Qc::qca1d(AnspcmLevel::Minus12, true);
        let bits = qca.bits();
        let mut watcher = BitWatcher::new();
        for (n, &bit) in bits.iter().enumerate() {
            let found = watcher.feed(bit);
            if n == ACCEPT_AT_BIT {
                assert_eq!(found, Some(qca), "accepted on bit {ACCEPT_AT_BIT}");
            } else {
                assert_eq!(found, None, "nothing on bit {n}");
            }
        }
    }

    #[test]
    fn qc1d_and_qca1d_carry_lm_where_tables_11_and_13_put_it() {
        // Tables 11 and 13/V.92, every printed row, read from the rendered
        // pages. The digital sequences put three zeros where the analogue
        // ones put W and X, so LM lands on b6 and b7 alone.
        let rows = [
            (Kind::Qc1d, false, 0b00, "1111111111 0101010101 0100000001 1111111111 0101010101 0100000001"),
            (Kind::Qc1d, false, 0b01, "1111111111 0101010101 0100000011 1111111111 0101010101 0100000011"),
            (Kind::Qc1d, false, 0b10, "1111111111 0101010101 0100000101 1111111111 0101010101 0100000101"),
            (Kind::Qc1d, false, 0b11, "1111111111 0101010101 0100000111 1111111111 0101010101 0100000111"),
            (Kind::Qc1d, true, 0b00, "1111111111 0101010101 0101000001 1111111111 0101010101 0101000001"),
            (Kind::Qc1d, true, 0b01, "1111111111 0101010101 0101000011 1111111111 0101010101 0101000011"),
            (Kind::Qc1d, true, 0b10, "1111111111 0101010101 0101000101 1111111111 0101010101 0101000101"),
            (Kind::Qc1d, true, 0b11, "1111111111 0101010101 0101000111 1111111111 0101010101 0101000111"),
            (
                Kind::Qca1d,
                false,
                0b00,
                "1111111111 0101010101 0110000001 1111111111 0101010101 0110000001 1111111111",
            ),
            (
                Kind::Qca1d,
                false,
                0b01,
                "1111111111 0101010101 0110000011 1111111111 0101010101 0110000011 1111111111",
            ),
            (
                Kind::Qca1d,
                false,
                0b10,
                "1111111111 0101010101 0110000101 1111111111 0101010101 0110000101 1111111111",
            ),
            (
                Kind::Qca1d,
                false,
                0b11,
                "1111111111 0101010101 0110000111 1111111111 0101010101 0110000111 1111111111",
            ),
            (
                Kind::Qca1d,
                true,
                0b00,
                "1111111111 0101010101 0111000001 1111111111 0101010101 0111000001 1111111111",
            ),
            (
                Kind::Qca1d,
                true,
                0b01,
                "1111111111 0101010101 0111000011 1111111111 0101010101 0111000011 1111111111",
            ),
            (
                Kind::Qca1d,
                true,
                0b10,
                "1111111111 0101010101 0111000101 1111111111 0101010101 0111000101 1111111111",
            ),
            (
                Kind::Qca1d,
                true,
                0b11,
                "1111111111 0101010101 0111000111 1111111111 0101010101 0111000111 1111111111",
            ),
        ];
        for (kind, lapm, pattern, string) in rows {
            let level = AnspcmLevel::from_pattern(pattern).unwrap();
            let qc = if kind == Kind::Qc1d { Qc::qc1d(level, lapm) } else { Qc::qca1d(level, lapm) };
            assert_eq!(text(&qc.bits()), printed(string), "{} P={lapm} LM={pattern:02b}", kind.name());
            assert_eq!(heard(&qc.bits()), vec![qc]);
        }
        // The four octets the digests worked out by hand, as a check on the
        // bit strings above from the other direction.
        assert_eq!(Qc::qc1d(AnspcmLevel::Minus9_5, true).octet(), 0x05);
        assert_eq!(Qc::qc1d(AnspcmLevel::Minus12, true).octet(), 0x85);
        assert_eq!(Qc::qca1d(AnspcmLevel::Minus12, true).octet(), 0x87);
        assert_eq!(Qc::qca1d(AnspcmLevel::Minus18, false).octet(), 0xC3);
        // Bit 21 is what tells the two ends apart, and it is b0 of the octet.
        assert!(Kind::Qc1d.digital() && Kind::Qca1d.digital());
        assert!(!Kind::Qc1a.digital() && !Kind::Qca1a.digital());
    }

    #[test]
    fn a_cleardown_qc1a_is_octet_0xec() {
        // 9.10.2.1: a QC whose U_QTS is 1111 is not a quick connect at all --
        // it hangs up a call left on hold. With P = 1 the information frame
        // is 0001101111, which is octet 0xEC.
        let qc = Qc::qc1a(Uqts::Cleardown, true);
        assert_eq!(qc.octet(), 0xEC);
        assert_eq!(
            text(&qc.bits()),
            printed("1111111111 0101010101 0001101111 1111111111 0101010101 0001101111")
        );
        assert_eq!(heard(&qc.bits()), vec![qc]);
        assert_eq!(qc.field, Field::Uqts(Uqts::Cleardown));
        // With P = 0 the digests worked the same case out as 0xE8, which is a
        // second check on where P sits.
        assert_eq!(Qc::qc1a(Uqts::Cleardown, false).octet(), 0xE8);
    }

    #[test]
    fn a_qc_body_equal_to_a_v8_sync_is_still_a_qc() {
        // The collision that keeps this watcher away from Decoder. With
        // P = 0, QC1a's information octet is 0x00 when WXYZ is 0000 and 0xE0
        // when WXYZ is 0111 -- the CI synchronisation and the CM and JM
        // synchronisation. Read as octets on their own they are V.8 signals
        // starting; read in place they are the body of a quick connect.
        let ci = Qc::qc1a(Uqts::from_ucode(61).unwrap(), false);
        assert_eq!(ci.octet(), SYNC_CI, "the CI synchronisation, 0000000001");
        assert_eq!(heard(&ci.bits()), vec![ci]);

        let menu = Qc::qc1a(Uqts::from_ucode(74).unwrap(), false);
        assert_eq!(menu.octet(), SYNC_MENU, "the CM and JM synchronisation, 0000001111");
        assert_eq!(heard(&menu.bits()), vec![menu]);

        let mut stream = Vec::new();
        for _ in 0..3 {
            stream.extend(ci.bits());
        }
        assert_eq!(heard(&stream), vec![ci; 3]);

        // And this is what Decoder makes of the same three. A framer hands it
        // 0x55, 0x00, 0x55, 0x00 and so on: the 0x00 is the CI
        // synchronisation as far as Decoder is concerned, and the 0x55 behind
        // it is then taken for the one-octet CI body, whose b5 b6 b7 are
        // 0 1 0 -- a call function, and a textphone one. Not CJ, which is the
        // other guess an octet-hunting parser might be expected to make:
        // Decoder clears its zero counter on every octet that is not zero, so
        // it never reaches the three consecutive ZERO octets of 3.5/V.8.
        let mut decoder = crate::Decoder::new();
        let mut misread = Vec::new();
        for _ in 0..6 {
            for octet in [SYNC_QC, ci.octet()] {
                if let Some(heard) = decoder.feed(octet) {
                    misread.push(heard);
                }
            }
        }
        assert!(!misread.is_empty(), "Decoder reads something out of a QC1a");
        assert!(
            misread.iter().all(|h| *h == crate::Heard::Ci(crate::CallFunction::Textphone)),
            "a bogus CI, and nothing else: {misread:?}"
        );
    }

    #[test]
    fn a_cm_carrying_0x55_as_an_extension_octet_is_not_a_qc() {
        // The collision the other way round. 0x55 is a legal V.8 modulation
        // extension octet -- b3 = 0, b4 = 1, b5 = 0, the shape 5.2/V.8 gives
        // one -- so a CM can carry the QC synchronisation in its body. What
        // makes it a synchronisation is sitting directly behind ten ONEs, and
        // in a CM the octets run back to back with no idle between them.
        assert!(crate::is_extension(SYNC_QC), "5.2/V.8, and why this test exists");
        let menu = crate::Menu {
            function: crate::CallFunction::Data,
            modulations: crate::Modulations::of(&[crate::Modulation::V34Duplex]),
            protocol: crate::Protocol::Lapm,
            access: None,
            pcm: None,
        };
        let mut octets = crate::sequence(crate::Signal::Cm, &menu);
        // Spliced in directly behind the modulation category octet, which is
        // where 5.2/V.8 puts an extension octet: "any number of extension
        // octets may follow directly after a category octet". `sequence` is
        // the synchronisation, the call function octet, then the three
        // modulation octets, so index 3 is behind the first of those three --
        // and the two real extension octets of the category follow it.
        octets.insert(3, SYNC_QC);

        // CM is "a repetitive sequence of bits" (7.3/V.8): ten ONEs and then
        // the octets, over and over.
        let mut stream = Vec::new();
        for _ in 0..4 {
            stream.extend([true; PREAMBLE_ONES]);
            for &octet in &octets {
                stream.extend(frame(octet));
            }
        }
        assert_eq!(heard(&stream), vec![], "no quick connect anywhere in a CM");

        // That stream is easy, because its two 0x55s are eighty bits apart
        // and the window needs thirty. The hard one is a CM carrying 0x55
        // twice, three octets apart, with the same octet behind each: that
        // puts two framed synchronisations exactly thirty bits apart with a
        // well-formed information frame after each, which is everything bits
        // 10:59 of a QC1a hold. All that is left to tell them apart is bits
        // 0:9 and 30:39, and in a CM those hold framed octets, because 5/V.8
        // puts the preamble in front of a sequence and nowhere inside it.
        let category = octets[1];
        let body = Qc::qc1a(Uqts::from_ucode(70).unwrap(), true).octet();
        let mut stream = Vec::new();
        for _ in 0..3 {
            for &octet in &[category, SYNC_QC, body, category, SYNC_QC, body] {
                stream.extend(frame(octet));
            }
        }
        assert_eq!(
            text(&stream[..10]),
            text(&frame(category)),
            "a framed octet where a QC has its ten ONEs"
        );
        assert_eq!(text(&stream[10..20]), text(&frame(SYNC_QC)), "and the synchronisation behind it");
        assert_eq!(heard(&stream), vec![], "ten ONEs, and nothing else, is what marks a QC");
    }

    #[test]
    fn every_bit_of_the_preamble_and_both_synchronisations_is_required() {
        // The other half of the anchor, swept rather than argued. Tables 2, 4,
        // 11 and 13 print bits 0:9 and 30:39 as ten ONEs and bits 10:19 and
        // 40:49 as 0101010101, and a receiver that let any one of those forty
        // bits go would be back to hunting for 0x55 octets in a stream that is
        // full of them.
        let qc = Qc::qc1a(Uqts::from_ucode(70).unwrap(), true);
        for position in (0..20).chain(30..50) {
            let mut bits = qc.bits();
            bits[position] = !bits[position];
            assert_eq!(heard(&bits), vec![], "bit {position} of the preamble or synchronisation");
        }
    }

    #[test]
    fn a_qc_whose_two_copies_differ_is_not_accepted() {
        // BOTH_COPIES_MUST_AGREE, which is a reading and not a printed rule:
        // the tables say bits 50:59 are "Bits 20:29 repeated" but no clause
        // says what a receiver does when they are not. A single bit out of
        // place is what a jitter slip leaves behind, and taking the first
        // copy on trust would mean acting on a U_QTS nobody sent. Flipping
        // BOTH_COPIES_MUST_AGREE to the other reading is what this test would
        // then have to be rewritten against.
        let qc = Qc::qc1a(Uqts::from_ucode(70).unwrap(), true);
        for position in 21..29 {
            let mut bits = qc.bits();
            bits[30 + position] = !bits[30 + position];
            assert_eq!(heard(&bits), vec![], "bit {position} of the second copy flipped");
        }
        // The same damage in the first copy is just as fatal, and for the
        // same reason.
        for position in 21..29 {
            let mut bits = qc.bits();
            bits[position] = !bits[position];
            assert_eq!(heard(&bits), vec![], "bit {position} of the first copy flipped");
        }
        // A broken frame is not a frame, whichever bit broke it.
        for position in [20, 29, 50, 59] {
            let mut bits = qc.bits();
            bits[position] = !bits[position];
            assert_eq!(heard(&bits), vec![], "the framing bit at {position}");
        }
        // And the preamble is what anchors the whole thing.
        let mut bits = qc.bits();
        bits[9] = false;
        assert_eq!(heard(&bits), vec![], "nine ONEs is not ten");
    }

    #[test]
    fn every_information_octet_keeps_b4_zero_and_reads_back_as_itself() {
        // Bit 25 is printed as 0 in all four tables, which is 5.1/V.8's rule
        // that a category octet has b4 = ZERO "to prevent flag simulation".
        // It is also the one bit that tells a QC information frame from the
        // QC synchronisation, whose b4 is a ONE -- so a sequence can never
        // look like its own preamble.
        for qc in all_sequences() {
            let octet = qc.octet();
            assert_eq!(octet & (1 << 4), 0, "{qc:?} sets b4");
            assert_ne!(octet, SYNC_QC, "{qc:?} would look like the synchronisation");
            assert_eq!(Qc::from_octet(octet), Some(qc), "{qc:?} does not read back");
            assert_eq!(qc.bits().len(), qc.kind.length());
            assert_eq!(heard(&qc.bits()), vec![qc], "{qc:?} is not heard");
        }
    }

    #[test]
    fn an_octet_that_breaks_a_printed_zero_is_not_a_qc() {
        // from_octet's only job beyond the arithmetic: refuse octets whose
        // fixed zeros are not zero. b4 is fixed in all four tables; b3 and b5
        // are fixed in Tables 11 and 13 alone, because Tables 2 and 4 put W
        // and X there.
        assert_eq!(Qc::from_octet(1 << 4), None, "b4 set");
        assert_eq!(Qc::from_octet(0x05 | (1 << 3)), None, "QC1d with a one where 000LM1 begins");
        assert_eq!(Qc::from_octet(0x05 | (1 << 5)), None, "QC1d with a one in the third zero");
        assert_eq!(Qc::from_octet(0x07 | (1 << 3)), None, "QCA1d likewise");
        // The analogue sequences have no such check to make: every one of the
        // sixteen patterns is a legal W0XYZ1.
        for pattern in 0..16u8 {
            let qc = Qc::qc1a(Uqts::from_pattern(pattern).unwrap(), false);
            assert_eq!(Qc::from_octet(qc.octet()), Some(qc));
        }
    }

    #[test]
    fn the_four_sequences_use_the_channels_82_and_83_name() {
        // 8.2.1 and 8.3.2 put the QCs on V.21(L) and 8.2.3 and 8.3.4 the QCAs
        // on V.21(H). Each modem therefore listens on the channel it is not
        // sending on, and never hears its own quick connect echoed back.
        assert_eq!(Kind::Qc1a.channel(), Channel::Low);
        assert_eq!(Kind::Qc1d.channel(), Channel::Low);
        assert_eq!(Kind::Qca1a.channel(), Channel::High);
        assert_eq!(Kind::Qca1d.channel(), Channel::High);
        assert!(!Kind::Qc1a.acknowledgement() && Kind::Qca1a.acknowledgement());
        assert_eq!(Kind::Qc1a.length(), QC_BITS);
        assert_eq!(Kind::Qca1a.length(), QCA_BITS);
        assert_eq!(Kind::Qca1d.name(), "QCA1d");
    }

    #[test]
    fn a_watcher_finds_a_second_sequence_after_the_first() {
        // Nothing here is a one-shot. The analogue answerer hears QC1a, and
        // if the call falls back and is tried again it must hear the next one
        // too; and after QC1a comes CM, which the watcher has to sit through
        // without latching.
        let qc = Qc::qc1a(Uqts::from_ucode(79).unwrap(), true);
        let qca = Qc::qca1d(AnspcmLevel::Minus15, true);
        let mut stream = qc.bits();
        stream.extend([true; 40]);
        stream.extend(qca.bits());
        assert_eq!(heard(&stream), vec![qc, qca]);
    }
}
