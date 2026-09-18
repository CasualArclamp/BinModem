//! Everything the analogue modem sends with the precoder switched out, as
//! senders and as readers, in LU units.
//!
//! Upstream in V.92 there is no carrier and no QAM. A symbol is a voltage the
//! central office's A/D will read back as one of its own codewords, and until
//! the digital modem has designed a precoder and sent it in CPd there is
//! nothing to shape it with. So the whole of Phase 3 upstream, and the first
//! half of Phase 4, is plain levels at 8000 a second: two of them while the
//! digital modem trains its equaliser and reads the DIL descriptor, and four
//! or eight of them while it measures the upstream channel it is about to
//! design against.
//!
//! Three kinds of signal live here.
//!
//! **Fixed patterns.** Ru and Su repeat six symbols each (8.5.5, 8.5.6).
//! Neither is scrambled, so absolute polarity means nothing on the line; what
//! the far end looks for is the reversal into R-bar-u or S-bar-u.
//!
//! **The two-point carrier**, which is TRN1u's modulation (8.5.7) and which
//! Ja, CPt and E1u ride on. Bits are scrambled by GPA, then differentially
//! encoded, and a scrambler output of 0 is a *positive* voltage -- the
//! opposite of the downstream convention of 8.6.2, and the one thing about
//! these signals that is easiest to get backwards.
//!
//! **TRN2u**, four or eight levels as Jp asked for, where only the sign bit
//! is differentially encoded (8.7.6, Tables 28 and 29). CPu, CPus, SUVu and
//! E2u ride on it in Phase 4 and in a rate renegotiation, which is why the
//! sender here is a bit pump with a queue rather than a generator of ones.
//!
//! ```text
//! Ru 384T | R-bar-u 24T | TRN1u >=2040T | Ja .... | Su | S-bar-u | TRN1u | CPt CPt | E1u | TRN2u ....
//!   fixed     fixed         two point     two pt    fixed  fixed   two pt  two point  two pt   4 or 8
//! ```
//!
//! The differential encoder is what joins them: Ja and CPt each start from
//! "the final symbol of the preceding TRN1u" (8.5.4, 8.5.1), E1u carries on
//! from the CPt before it (8.5.2), and TRN2u starts from "the last
//! transmitted sign bit of the preceding E1u" (8.7.6). Each type here takes
//! that seed from the one before rather than inventing it, so a caller cannot
//! forget.
//!
//! Levels are in LU units throughout (3.8): +LU is 1.0. What LU is in line
//! samples belongs to `v92::transmit`, because it depends on the power
//! reduction INFO1d asked for and on what the sound path can carry.

use std::collections::VecDeque;

use crate::v32::{Mode, Scrambler};

use super::UP_INTERVALS;

/// The upstream reference level (3.8): "The value of LU is set such that
/// TRN1u is transmitted at the desired data mode transmit power."
///
/// Everything in this module is a multiple of it, so here it is one.
pub const LU: f64 = 1.0;

/// The unit every upstream sequence and segment is a whole number of.
///
/// "TRN1u segments shall be an integer multiple of 12 symbols in length"
/// (8.5.7); the same of Su and S-bar-u (8.5.6) and of TRN2u (8.7.6); "Ja
/// shall be an integer multiple of 12 bits long" (8.5.4); and CPt, CPu, CPus
/// and SUVu each fill "to the next multiple of 12 symbols" (Tables 23, 24,
/// 27). It is [`super::UP_INTERVALS`] under another name, and that is the
/// point: the rule exists so that the data frame alignment the digital modem
/// takes from the first symbol of the second TRN1u survives every sequence
/// between there and B1u.
///
/// In *bits* it depends on what is carrying the sequence -- twelve for the
/// two-point signals here, twenty-four or thirty-six for the same twelve
/// symbols of four- or eight-point TRN2u -- which is why
/// `v90::sequences::Cp::to_bits_in` is told a unit in bits and not in
/// symbols.
pub const UP_SEQUENCE_UNIT: usize = UP_INTERVALS;

// ---------------------------------------------------------------------------
// The two-point modulation (8.5.7)
// ---------------------------------------------------------------------------

/// What a scrambler output bit is on the upstream line: "A scrambler output
/// of 0 represents a positive voltage; a scrambler output of 1 represents a
/// negative voltage" (8.5.7).
///
/// Downstream is the other way round -- "A sign of 0 represents a negative
/// voltage" (8.6.2) -- so the two directions cannot share one mapping, and
/// Table 28 agrees with this one: TRN2u's sign bit 0 is positive.
pub fn two_point_level(bit: bool) -> f64 {
    if bit { -LU } else { LU }
}

/// The bit a received two-point symbol carries, from its sign alone.
///
/// Zero counts as positive. A slicer choosing between +LU and -LU has
/// nowhere else to put it, and the two-point signals are all differentially
/// decoded anyway, where a wrong guess costs two bits and not the stream.
pub fn two_point_bit(level: f64) -> bool {
    level < 0.0
}

/// Whether the GPA scrambler is zeroed before *each* TRN1u segment.
///
/// 8.5.7 prints one sentence -- "The scrambler shall be initialized to zero
/// prior to the transmission of TRN1u" -- and Phase 3 sends two segments, one
/// before Ja and one while the DIL or SCR arrives (9.5.2.1.2, 9.5.2.1.9). The
/// reading taken here is that the sentence covers both, so each segment opens
/// with the same 48 signs and the digital modem can train on either of them
/// from its first symbol.
///
/// The alternative is that only the first is zeroed and the second carries on
/// from Ja's scrambler state. A receiver that descrambles self-synchronously,
/// as ours does, cannot tell the two apart, so only a capture can; this is
/// the one line that would change.
pub const TRN1U_RESET_EACH_SEGMENT: bool = true;

/// TRN1u (8.5.7): "a sequence of +-LU values", whose signs are "generated by
/// applying binary ones to the input of the scrambler described in 6.3".
///
/// Because the scrambler starts zeroed and is fed ones, the whole segment is
/// known to the far end from its first symbol, which is what makes it a
/// training sequence: the digital modem can least-squares its equaliser
/// against it without deciding anything first.
#[derive(Debug, Clone)]
pub struct Trn1u {
    scrambler: Scrambler,
    last: bool,
}

impl Default for Trn1u {
    fn default() -> Self {
        Self::new()
    }
}

impl Trn1u {
    /// A segment from its first symbol, with the scrambler zeroed (8.5.7).
    ///
    /// GPA is the analogue modem's polynomial whichever end placed the call
    /// (6.3), so the role V.32 names when it picks the taps -- `Answer` for
    /// 1 + x^-5 + x^-23 -- is fixed here and is not the caller's to choose.
    pub fn new() -> Self {
        Self { scrambler: Scrambler::new(Mode::Answer), last: false }
    }

    /// Begin another segment, which under [`TRN1U_RESET_EACH_SEGMENT`] zeroes
    /// the scrambler again.
    pub fn restart(&mut self) {
        if TRN1U_RESET_EACH_SEGMENT {
            self.scrambler.reset();
        }
    }

    /// The next sign, as the bit that chooses it.
    pub fn next_bit(&mut self) -> bool {
        self.last = self.scrambler.scramble(true);
        self.last
    }

    /// The next symbol, in LU units.
    pub fn next_symbol(&mut self) -> f64 {
        two_point_level(self.next_bit())
    }

    /// The bit that chose the last symbol sent, which is the differential
    /// memory Ja and CPt are initialised with (8.5.4, 8.5.1). TRN1u is not
    /// differentially encoded, so it is simply the last scrambler output.
    pub fn last_bit(&self) -> bool {
        self.last
    }
}

// ---------------------------------------------------------------------------
// Ru and Su (8.5.5, 8.5.6)
// ---------------------------------------------------------------------------

/// The period Ru and Su repeat, "the 6-symbol sequence" of 8.5.5 and 8.5.6.
///
/// Six is the downstream data frame, and half of the upstream one, so the
/// segment lengths 9.5.2.1 gives -- [`super::RU_SYMBOLS`],
/// [`super::RU_BAR_SYMBOLS`] and [`super::SU_SYMBOLS`] -- are whole periods
/// and whole upstream frames at once.
pub const RU_PERIOD: usize = 6;

/// Signal Ru at symbol `n` of the segment, or R-bar-u when `bar`.
///
/// "Signal Ru is transmitted by repeating the 6-symbol sequence
/// {+LU, +LU, +LU, -LU, -LU, -LU}. Signal R-bar-u is transmitted by repeating
/// the 6-symbol sequence {-LU, -LU, -LU, +LU, +LU, +LU}" (8.5.5).
///
/// Neither is scrambled or differentially encoded, and the analogue modem
/// "shall bypass the precoder and prefilter structure" for both, using "the
/// same structure used while transmitting 2 point TRN1u". The digital modem
/// looks for the Ru-to-R-bar-u reversal (9.5.1.1.1), so the line's polarity
/// does not matter.
pub fn ru(n: u64, bar: bool) -> f64 {
    let positive = (n % RU_PERIOD as u64) < RU_PERIOD as u64 / 2;
    if positive == bar { -LU } else { LU }
}

/// Su's non-zero level, "sqrt(3/2) x LU" (8.5.6).
///
/// Two of every three symbols carry it and the third is a true zero, so the
/// mean square is 1.5 x 2/3 = 1: Su goes out at exactly TRN1u's power. The
/// digital modem measures where the codec's sampling instants fall inside the
/// analogue modem's symbol from it (9.5.1.1.6), and a level change in the
/// middle of that measurement would be the last thing either end wants.
pub fn su_level() -> f64 {
    (3.0f64 / 2.0).sqrt() * LU
}

/// Signal Su at symbol `n` of the segment, or S-bar-u when `bar`.
///
/// "Signal Su is transmitted by repeating the 6-symbol sequence
/// {+sqrt(3/2) x LU, 0, +sqrt(3/2) x LU, -sqrt(3/2) x LU, 0,
/// -sqrt(3/2) x LU}", and S-bar-u the same with every sign inverted (8.5.6).
///
/// The zeros are real zeros, which is what tells Su from Ru through a
/// channel that has smeared both: a receiver hunting for the pattern has to
/// hunt for them rather than for a polarity.
pub fn su(n: u64, bar: bool) -> f64 {
    /// The shape of one period, before the level and the bar are applied.
    const SHAPE: [i8; RU_PERIOD] = [1, 0, 1, -1, 0, -1];
    let shape = f64::from(SHAPE[(n % RU_PERIOD as u64) as usize]);
    su_level() * shape * if bar { -1.0 } else { 1.0 }
}

// ---------------------------------------------------------------------------
// The two-point differential carrier: Ja, CPt and E1u
// ---------------------------------------------------------------------------

/// The ones before the first sequence of a group: "24 differentially encoded
/// binary ones shall be transmitted prior to transmitting the first CPt in a
/// series of CPt sequences" (8.5.1), and Ja "consists of 24 binary ones
/// followed by repetitions of the DIL descriptor" (8.5.4).
///
/// Twenty-four is exactly what a far end that is not already locked needs,
/// and not one more. Its first symbol is swallowed as the differential
/// reference and carries no bit; the next twenty-three fill the
/// descrambler's twenty-three-bit register; and the twenty-fourth symbol is
/// the first whose bit comes out right -- which is the first bit of the
/// frame sync.
pub const PREAMBLE_ONES: usize = 24;

/// Whether the preamble's ones are scrambled as well as differentially
/// encoded.
///
/// 8.5.1 calls them "24 differentially encoded binary ones" and says nothing
/// about scrambling; 8.5.4 wraps Ja's twenty-four inside a sequence that is
/// "scrambled and differentially encoded" as a whole. The reading taken here
/// is that both are scrambled, because unscrambled ones would run the far
/// descrambler on nothing it can use -- it would still need twenty-three bits
/// of the sequence itself before the sync came out right, and the preamble
/// would have bought nothing at all.
///
/// The alternative is the literal one: the ones bypass the scrambler, which
/// then does not run over them either, and on the line the preamble is a run
/// of alternating symbols. A capture of a real analogue modem's CPt settles
/// it; flipping this constant is the whole change.
pub const PREAMBLE_IS_SCRAMBLED: bool = true;

/// E1u: "a data frame of scrambled, differentially encoded zeroes used to
/// signal the end of CPt" (8.5.2). An upstream data frame is twelve symbols.
pub const E1U_SYMBOLS: usize = UP_INTERVALS;

/// The sender of everything that rides TRN1u's modulation: Ja (8.5.4), CPt
/// (8.5.1) and E1u (8.5.2).
///
/// One instance carries a whole group, because nothing between them resets
/// anything: the GPA scrambler runs on from the TRN1u before the group,
/// through the preamble, through every repetition, and into E1u, and the
/// differential encoder runs on with it. A sender built per sequence would
/// put the far descrambler back to square one at every repetition and would
/// look, on the line, almost right.
#[derive(Debug, Clone)]
pub struct TwoPointSender {
    scrambler: Scrambler,
    /// d(n-1): the bit that chose the last symbol transmitted.
    last: bool,
    /// Queued bits, each with whether the scrambler runs over it
    /// ([`PREAMBLE_IS_SCRAMBLED`]).
    queue: VecDeque<(bool, bool)>,
}

impl TwoPointSender {
    /// Take over from the TRN1u segment just sent.
    ///
    /// The differential encoder memory "shall be initialized with the final
    /// symbol of the preceding TRN1u" (8.5.1, 8.5.4), and the scrambler is
    /// not reset: no clause says to, and the far end's descrambler is
    /// already locked to it.
    pub fn after(trn1u: Trn1u) -> Self {
        Self { scrambler: trn1u.scrambler, last: trn1u.last, queue: VecDeque::new() }
    }

    /// Queue the [`PREAMBLE_ONES`] that go before the first sequence of a
    /// group.
    pub fn preamble(&mut self) {
        self.queue.extend(std::iter::repeat_n((true, PREAMBLE_IS_SCRAMBLED), PREAMBLE_ONES));
    }

    /// Queue one sequence's bits, bit 0 first, as `v90::sequences` writes
    /// them: a DIL descriptor for Ja, or a CPt.
    pub fn push(&mut self, bits: &[bool]) {
        self.queue.extend(bits.iter().map(|&bit| (bit, true)));
    }

    /// Queue E1u, the twelve zeros that end a CPt series (8.5.2).
    pub fn e1u(&mut self) {
        self.queue.extend(std::iter::repeat_n((false, true), E1U_SYMBOLS));
    }

    /// How many symbols are still to go: these signals carry one bit each, so
    /// a sequence's bits and its symbols are the same count.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue has run dry. Ja "may be terminated without
    /// completing the final DIL descriptor" (8.5.4), so a source is expected
    /// to stop this sender mid-sequence; it is never expected to let it run
    /// past the end and invent symbols.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The next symbol in LU units, or `None` when nothing is queued.
    pub fn next_symbol(&mut self) -> Option<f64> {
        let (bit, scramble) = self.queue.pop_front()?;
        let source = if scramble { self.scrambler.scramble(bit) } else { bit };
        // "differentially encoded by modulo 2 addition of the present bit
        // with the previously transmitted bit" (8.5.1, 8.5.4).
        self.last ^= source;
        Some(two_point_level(self.last))
    }

    /// The bit that chose the last symbol transmitted, which seeds TRN2u's
    /// sign differential encoder after E1u (8.7.6).
    pub fn last_sign(&self) -> bool {
        self.last
    }
}

/// The reader of everything that rides TRN1u's modulation.
///
/// It starts plain, which is what TRN1u itself is -- the sign is the
/// scrambler output, with nothing differential about it -- and is switched to
/// differential where Ja or CPt begins. The switch keeps the symbol it lands
/// on as d(n-1) rather than dropping it, so a reader that was already locked
/// to the TRN1u stays locked across the join and reads the first bit of the
/// sequence correctly. That is why a far end which followed the TRN1u does
/// not need the twenty-four ones at all; they are there for one that did not.
///
/// Differential decoding makes the reader blind to the line's polarity
/// (s(n) = d(n) XOR d(n-1) is unchanged when every symbol is inverted), which
/// is just as well: nothing upstream in Phase 3 fixes an absolute polarity.
#[derive(Debug, Clone)]
pub struct TwoPointReader {
    descrambler: Scrambler,
    /// The last symbol fed, as its bit: d(n-1) once the reader is
    /// differential.
    previous: Option<bool>,
    differential: bool,
}

impl Default for TwoPointReader {
    fn default() -> Self {
        Self::new()
    }
}

impl TwoPointReader {
    /// A reader of plain TRN1u, descrambling with GPA.
    pub fn new() -> Self {
        Self { descrambler: Scrambler::new(Mode::Answer), previous: None, differential: false }
    }

    /// A reader that is differential from its first symbol, which is what a
    /// receiver that has just found a preamble wants. The first symbol fed
    /// becomes the reference and yields no bit.
    pub fn differential() -> Self {
        Self { differential: true, ..Self::new() }
    }

    /// Switch to differential decoding, replaying the symbol the switch lands
    /// on as the reference so that no bit is lost at the join.
    pub fn switch_to_differential(&mut self) {
        self.differential = true;
    }

    /// Whether the reader is decoding differentially.
    pub fn is_differential(&self) -> bool {
        self.differential
    }

    /// Feed one symbol's sign, and get the descrambled bit it carried.
    ///
    /// `None` only at the very first symbol of a differential reader, which
    /// is the reference.
    pub fn feed_sign(&mut self, negative: bool) -> Option<bool> {
        let previous = self.previous.replace(negative);
        let received = if self.differential { negative ^ previous? } else { negative };
        Some(self.descrambler.descramble(received))
    }

    /// Feed one symbol, in LU units.
    pub fn feed(&mut self, level: f64) -> Option<bool> {
        self.feed_sign(two_point_bit(level))
    }
}

// ---------------------------------------------------------------------------
// TRN2u (8.7.6, Tables 28 and 29)
// ---------------------------------------------------------------------------

/// Which of the two or three bits of a TRN2u symbol goes out first in time.
///
/// Tables 28 and 29 head their column "MSB:LSB" and the MSB is the sign, but
/// neither table is in the list of clause 8's introduction, which is what
/// says a value written as an integer goes out least significant bit first,
/// and no clause says which of a symbol's bits is sent first. The reading
/// taken here is the one that matches the rest of the Recommendation and
/// V.34's own TRN (10.1.3.8, where I1n comes first and In = 2 x I2n + I1n):
/// **the LSB is first in time and the sign bit is last**.
///
/// The alternative is the table's own layout read left to right, the sign
/// first. Both ends of this project would agree either way, so only a real
/// V.92 server settles it; if one never answers our SUVu with a CPd, this is
/// the line to flip. [`Trn2uSender`] and [`Trn2uReader`] each take it as a
/// field so that both settings can be, and are, tested.
pub const TRN2U_SIGN_LAST: bool = true;

/// Whether only the sign bit of a TRN2u symbol is differentially encoded.
///
/// 8.7.6 names one bit and one only: "The sign bit of TRN2u is differentially
/// encoded by modulo 2 addition of the present sign bit with the previously
/// transmitted sign bit." The magnitude bits are left alone. The alternative
/// would be to read "the sign bit" as loose wording for the whole symbol, and
/// differentially encode all b bits against the previous symbol's; nothing in
/// the text supports it, and it is written down here only because a wrong
/// reading of this costs the same as a wrong reading of the bit order and
/// looks the same on the line.
pub const TRN2U_ONLY_SIGN_IS_DIFFERENTIAL: bool = true;

/// The size of the TRN2u constellation, "as requested by the digital modem
/// via Jp bits 48 and 49" (8.7.6): bit 48 during training, bit 49 during a
/// rate renegotiation, 0 meaning four points and 1 eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trn2uSize {
    /// Table 28: two bits a symbol, magnitudes 1 and 3 over sqrt(5).
    Four,
    /// Table 29: three bits a symbol, magnitudes 1, 3, 5 and 7 over sqrt(21).
    Eight,
}

impl Trn2uSize {
    /// From Jp bit 48 or 49.
    pub fn from_jp_bit(bit: bool) -> Self {
        if bit { Self::Eight } else { Self::Four }
    }

    /// Back to that bit.
    pub fn jp_bit(self) -> bool {
        self == Self::Eight
    }

    /// Points in the constellation.
    pub fn points(self) -> usize {
        match self {
            Self::Four => 4,
            Self::Eight => 8,
        }
    }

    /// Bits a symbol carries: b = log2 of the points, so one twelve-symbol
    /// frame holds 24 bits on four points and 36 on eight.
    pub fn bits(self) -> usize {
        match self {
            Self::Four => 2,
            Self::Eight => 3,
        }
    }

    /// What the odd magnitudes are divided by: sqrt(5) for four points and
    /// sqrt(21) for eight (Tables 28, 29).
    ///
    /// Both are the root of the mean square of the odd integers the
    /// constellation uses -- (1 + 9)/2 and (1 + 9 + 25 + 49)/4 -- which is
    /// what makes TRN2u's mean power exactly LU squared, the same as TRN1u's.
    pub fn root(self) -> f64 {
        match self {
            Self::Four => 5.0f64.sqrt(),
            Self::Eight => 21.0f64.sqrt(),
        }
    }

    /// The magnitude the `m` magnitude bits select: (2m + 1) / root.
    pub fn magnitude(self, m: usize) -> f64 {
        (2.0 * m as f64 + 1.0) / self.root() * LU
    }

    /// The level a group of bits maps to, the group read as Tables 28 and 29
    /// write it: MSB first, the MSB the sign, 0 positive.
    pub fn level(self, group: u8) -> f64 {
        let magnitude = self.magnitude(usize::from(group) & (self.points() / 2 - 1));
        if usize::from(group) & (self.points() / 2) == 0 { magnitude } else { -magnitude }
    }

    /// The group a received level, in LU units, decides to: its sign, and the
    /// nearest of the odd magnitudes.
    pub fn group_for(self, level: f64) -> u8 {
        let step = ((level.abs() * self.root() - 1.0) / 2.0).round();
        let m = step.clamp(0.0, (self.points() / 2 - 1) as f64) as usize;
        let sign = if level < 0.0 { self.points() / 2 } else { 0 };
        (m | sign) as u8
    }
}

/// The upstream bit pump of Phase 4 and of a rate renegotiation: TRN2u
/// itself, and every sequence that rides its modulation.
///
/// TRN2u "consists of scrambled binary ones" (8.7.6), and CPu, CPus, SUVu and
/// E2u are the same modulation carrying their own bits instead (8.7.2, 8.7.3,
/// 8.7.5). Nothing between them resets anything -- only the start of TRN2u
/// does, and then B1u -- so one instance runs the whole exchange and an empty
/// queue simply means ones. Padding each sequence to a whole number of
/// symbols is the caller's, because only the caller knows which sequence it
/// is sending.
#[derive(Debug, Clone)]
pub struct Trn2uSender {
    size: Trn2uSize,
    first_bit_is_lsb: bool,
    scrambler: Scrambler,
    /// The last sign bit transmitted, which is the whole differential memory.
    last_sign: bool,
    queue: VecDeque<bool>,
}

impl Trn2uSender {
    /// A TRN2u from its first symbol: "The scrambler shall be reset at the
    /// beginning of TRN2u" (8.7.6), and the sign differential memory is
    /// seeded by the caller.
    ///
    /// 8.7.6 names one seed, "the last transmitted sign bit of the preceding
    /// E1u sequence", which is true in Phase 4 only; in a rate renegotiation
    /// TRN2u follows R-bar-u, and in the silence path it follows E2u. Which
    /// sign that is belongs to `v92::up_source`, so it arrives here as an
    /// argument.
    pub fn seeded(size: Trn2uSize, sign: bool) -> Self {
        Self {
            size,
            first_bit_is_lsb: TRN2U_SIGN_LAST,
            scrambler: Scrambler::new(Mode::Answer),
            last_sign: sign,
            queue: VecDeque::new(),
        }
    }

    /// Send with the other reading of the bit order (see
    /// [`TRN2U_SIGN_LAST`]). For tests, and for the day a capture says so.
    pub fn with_bit_order(mut self, first_bit_is_lsb: bool) -> Self {
        self.first_bit_is_lsb = first_bit_is_lsb;
        self
    }

    /// The constellation in use.
    pub fn size(&self) -> Trn2uSize {
        self.size
    }

    /// Queue a sequence's bits, bit 0 first, already padded to a whole number
    /// of symbols.
    pub fn push(&mut self, bits: &[bool]) {
        self.queue.extend(bits.iter().copied());
    }

    /// How many queued bits are left to send.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The next symbol in LU units: the queue if there is one, and otherwise
    /// TRN2u's binary ones.
    pub fn next_symbol(&mut self) -> f64 {
        let b = self.size.bits();
        debug_assert!(self.queue.is_empty() || self.queue.len() >= b, "a sequence rides TRN2u in whole symbols");
        let mut group = 0u8;
        for k in 0..b {
            let scrambled = self.scrambler.scramble(self.queue.pop_front().unwrap_or(true));
            let place = if self.first_bit_is_lsb { k } else { b - 1 - k };
            let out = if place == b - 1 {
                // The sign, and by TRN2U_ONLY_SIGN_IS_DIFFERENTIAL only the
                // sign: d(n) = s(n) XOR d(n-1) (8.7.6).
                self.last_sign ^= scrambled;
                self.last_sign
            } else {
                scrambled
            };
            group |= u8::from(out) << place;
        }
        self.size.level(group)
    }

    /// The last sign bit transmitted, which the sequence after this one
    /// carries on from (8.7.3, 8.7.5).
    pub fn last_sign(&self) -> bool {
        self.last_sign
    }
}

/// The bits one TRN2u symbol carried, in time order: two or three of them,
/// or none for the symbol a reader took as its differential reference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SymbolBits {
    bits: [bool; 3],
    len: usize,
}

impl SymbolBits {
    fn push(&mut self, bit: bool) {
        self.bits[self.len] = bit;
        self.len += 1;
    }

    /// How many bits there are.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the symbol carried none.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl IntoIterator for SymbolBits {
    type Item = bool;
    type IntoIter = std::iter::Take<std::array::IntoIter<bool, 3>>;

    fn into_iter(self) -> Self::IntoIter {
        self.bits.into_iter().take(self.len)
    }
}

/// The reader that matches [`Trn2uSender`]: a slicer, the sign
/// differentially decoded, and GPA descrambling.
///
/// Like the two-point reader it is blind to the line's polarity, because an
/// inverted line flips every sign bit and the differential decoder takes the
/// flip back out. The magnitudes are untouched by an inversion, so nothing
/// else has to care.
#[derive(Debug, Clone)]
pub struct Trn2uReader {
    size: Trn2uSize,
    first_bit_is_lsb: bool,
    descrambler: Scrambler,
    previous: Option<bool>,
}

impl Trn2uReader {
    /// A reader of `size` points, from the first symbol of a TRN2u. That
    /// first symbol is the differential reference and carries no bits.
    pub fn new(size: Trn2uSize) -> Self {
        Self { size, first_bit_is_lsb: TRN2U_SIGN_LAST, descrambler: Scrambler::new(Mode::Answer), previous: None }
    }

    /// Read with the other reading of the bit order (see
    /// [`TRN2U_SIGN_LAST`]).
    pub fn with_bit_order(mut self, first_bit_is_lsb: bool) -> Self {
        self.first_bit_is_lsb = first_bit_is_lsb;
        self
    }

    /// The constellation in use.
    pub fn size(&self) -> Trn2uSize {
        self.size
    }

    /// Feed one symbol in LU units, and get the descrambled bits it carried,
    /// in time order.
    pub fn feed(&mut self, level: f64) -> SymbolBits {
        let b = self.size.bits();
        let group = self.size.group_for(level);
        let sign = usize::from(group) & (self.size.points() / 2) != 0;
        let Some(previous) = self.previous.replace(sign) else {
            return SymbolBits::default();
        };
        let mut out = SymbolBits::default();
        for k in 0..b {
            let place = if self.first_bit_is_lsb { k } else { b - 1 - k };
            let received = if place == b - 1 { sign ^ previous } else { group >> place & 1 == 1 };
            out.push(self.descrambler.descramble(received));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::sequences::{
        ALL_PCM_UPSTREAM_RATES, Cp, CpFinder, Descriptor, DescriptorFinder, JA_UNIT_BITS, Layout, Mask,
    };

    /// The signs `text` names, 1 being -LU, as levels.
    fn signs(text: &str) -> Vec<f64> {
        text.chars().map(|c| two_point_level(c == '1')).collect()
    }

    /// A CPt in the shape P3S 4.1 worked out: drn 16, one constellation of
    /// Ucodes 0 to 79, 290 bits filled to 300.
    fn a_cpt() -> Cp {
        Cp { drn: 16, constellations: vec![((1u128 << 80) - 1) as Mask], ..Cp::default() }
    }

    /// A Ja descriptor with no DIL, the one length 8.5.4 prints: 276 bits.
    fn a_descriptor() -> Vec<bool> {
        Descriptor::none().to_bits_in(Some(ALL_PCM_UPSTREAM_RATES))
    }

    /// `trn1u` symbols of TRN1u, then the preamble and `body`, as levels,
    /// with the line multiplied by `polarity`.
    fn two_point_stream(trn1u: usize, body: &[bool], polarity: f64) -> Vec<f64> {
        let mut generator = Trn1u::new();
        let mut levels: Vec<f64> = (0..trn1u).map(|_| generator.next_symbol()).collect();
        let mut sender = TwoPointSender::after(generator);
        sender.preamble();
        sender.push(body);
        while let Some(level) = sender.next_symbol() {
            levels.push(level);
        }
        levels.iter().map(|level| level * polarity).collect()
    }

    /// Every bit a cold differential reader gets out of `levels`.
    fn read_cold(levels: &[f64]) -> Vec<bool> {
        let mut reader = TwoPointReader::differential();
        levels.iter().filter_map(|&level| reader.feed(level)).collect()
    }

    /// 8.5.7 with 6.3: ones into GPA (1 + x^-5 + x^-23) from a zeroed
    /// register, output 1 being a negative voltage. The first 48 signs are
    /// the vector P3S 3.1 derives, and the same one P3P 3.3 prints.
    #[test]
    fn trn1u_starts_with_the_gpa_signs_the_digest_lists() {
        const FIRST_48: &str = "111110000011111000001110011111000110000011100100";
        let mut trn1u = Trn1u::new();
        let sent: Vec<f64> = (0..48).map(|_| trn1u.next_symbol()).collect();
        assert_eq!(sent, signs(FIRST_48));
        // The head of it, spelled out: the register is zero and the input is
        // ones, so the first five outputs are ones with nothing to feed back
        // yet, and they are five negative symbols.
        assert_eq!(&sent[..5], &[-LU; 5]);
        assert_eq!(sent[5], LU, "the first output reaches the x^-5 tap");
        // Upstream is the opposite of downstream, where a sign of 0 is the
        // negative voltage (8.6.2).
        assert_eq!(two_point_level(false), LU);
        assert_eq!(two_point_level(true), -LU);
        assert!(two_point_bit(-0.5) && !two_point_bit(0.5) && !two_point_bit(0.0));
        // Mean power is LU squared, which is what 3.8 sets LU by.
        let power: f64 = sent.iter().map(|level| level * level).sum::<f64>() / sent.len() as f64;
        assert!((power - LU * LU).abs() < 1e-12, "{power}");
        // And the last symbol is what seeds Ja's and CPt's differential
        // encoder (8.5.4, 8.5.1).
        assert_eq!(two_point_level(trn1u.last_bit()), sent[47]);
    }

    /// 8.5.7: "The scrambler shall be initialized to zero prior to the
    /// transmission of TRN1u", read as covering the second segment too
    /// ([`TRN1U_RESET_EACH_SEGMENT`], P3S A6). The digital modem trains on
    /// that segment and takes its frame alignment from its first symbol
    /// (9.5.1.1.10), so it matters that it is the same known sequence.
    #[test]
    fn a_second_trn1u_segment_starts_the_same_way_as_the_first() {
        let mut trn1u = Trn1u::new();
        let first: Vec<f64> = (0..2040).map(|_| trn1u.next_symbol()).collect();
        trn1u.restart();
        let second: Vec<f64> = (0..2040).map(|_| trn1u.next_symbol()).collect();
        // Which is what TRN1U_RESET_EACH_SEGMENT buys. Without the reset the
        // second segment would carry on from the first and share no symbol
        // with it.
        assert_eq!(first, second);
        // 9.5.2.1.2's floor, and 8.5.7's "integer multiple of 12 symbols".
        assert_eq!(second.len(), super::super::TRN1U_MINIMUM);
        assert!(second.len().is_multiple_of(UP_SEQUENCE_UNIT));
        assert_eq!(UP_SEQUENCE_UNIT, 12);
    }

    /// 8.5.5: Ru repeats "{+LU, +LU, +LU, -LU, -LU, -LU}" and R-bar-u
    /// "{-LU, -LU, -LU, +LU, +LU, +LU}", both at TRN1u's power.
    #[test]
    fn ru_and_its_bar_are_the_printed_patterns() {
        let period: Vec<f64> = (0..RU_PERIOD as u64).map(|n| ru(n, false)).collect();
        assert_eq!(period, vec![LU, LU, LU, -LU, -LU, -LU]);
        let bar: Vec<f64> = (0..RU_PERIOD as u64).map(|n| ru(n, true)).collect();
        assert_eq!(bar, vec![-LU, -LU, -LU, LU, LU, LU]);
        // It is a repetition, and the bar is the inversion, at every symbol
        // of a whole segment.
        for n in 0..super::super::RU_SYMBOLS as u64 {
            assert_eq!(ru(n, false), period[(n % RU_PERIOD as u64) as usize], "symbol {n}");
            assert_eq!(ru(n, true), -ru(n, false), "symbol {n}");
        }
        // 9.5.2.1.1's lengths are whole periods: 64 of them and 4.
        assert_eq!(super::super::RU_SYMBOLS / RU_PERIOD, 64);
        assert_eq!(super::super::RU_BAR_SYMBOLS / RU_PERIOD, 4);
        // Mean power LU squared, and the two lines P3S 4.5 derives: 1333.3 Hz
        // at 4 LU and 4000 Hz at 2 LU, with nothing at 0, 2666.7 or 5333.3.
        assert!((mean_square(&period) - LU * LU).abs() < 1e-12);
        let spectrum = dft(&period);
        assert!((spectrum[1] - 4.0 * LU).abs() < 1e-9, "{:?}", spectrum);
        assert!((spectrum[3] - 2.0 * LU).abs() < 1e-9, "{:?}", spectrum);
        for k in [0, 2, 4] {
            assert!(spectrum[k] < 1e-12, "bin {k} of {spectrum:?}");
        }
    }

    /// 8.5.6: Su repeats "{+sqrt(3/2) x LU, 0, +sqrt(3/2) x LU,
    /// -sqrt(3/2) x LU, 0, -sqrt(3/2) x LU}". Two thirds of the symbols at
    /// 1.5 LU squared is exactly LU squared, so the analogue modem does not
    /// change level while the digital modem measures its sampling phase
    /// (9.5.1.1.6).
    #[test]
    fn su_has_the_same_power_as_trn1u() {
        let a = su_level();
        assert!((a * a - 1.5).abs() < 1e-12, "sqrt(3/2)");
        let period: Vec<f64> = (0..RU_PERIOD as u64).map(|n| su(n, false)).collect();
        assert_eq!(period, vec![a, 0.0, a, -a, 0.0, -a]);
        let bar: Vec<f64> = (0..RU_PERIOD as u64).map(|n| su(n, true)).collect();
        assert_eq!(bar, vec![-a, 0.0, -a, a, 0.0, a]);
        assert!(period.iter().filter(|level| **level == 0.0).count() == 2, "the zeros are true zeros");

        let mut trn1u = Trn1u::new();
        let trained: Vec<f64> = (0..super::super::SU_SYMBOLS).map(|_| trn1u.next_symbol()).collect();
        let power = mean_square(&period);
        println!("Su {power}, TRN1u {}, Ru {}", mean_square(&trained), mean_square(&(0..6).map(|n| ru(n, false)).collect::<Vec<f64>>()));
        assert!((power - LU * LU).abs() < 1e-12);
        assert!((power - mean_square(&trained)).abs() < 1e-12);

        // The two lines P3S 4.6 derives, at 8000/6 Hz and 4000 Hz.
        let spectrum = dft(&period);
        assert!((spectrum[1] - 2.0 * a).abs() < 1e-9, "1333.3 Hz: {spectrum:?}");
        assert!((spectrum[3] - 4.0 * a).abs() < 1e-9, "4000 Hz: {spectrum:?}");
        assert!((spectrum[5] - 2.0 * a).abs() < 1e-9, "its image: {spectrum:?}");
        for k in [0, 2, 4] {
            assert!(spectrum[k] < 1e-12, "bin {k} of {spectrum:?}");
        }
        // Su's strong line is at 4000 Hz where Ru's is at 1333.3, which is
        // the whole reason there are two patterns.
        assert!(spectrum[3] > spectrum[1]);
        assert_eq!(super::super::SU_SYMBOLS / RU_PERIOD, 24, "144T is 24 periods");
    }

    /// The mean square of a stretch of symbols.
    fn mean_square(levels: &[f64]) -> f64 {
        levels.iter().map(|level| level * level).sum::<f64>() / levels.len() as f64
    }

    /// The magnitude of each DFT bin of one six-symbol period. Bin k is at
    /// k x 8000/6 Hz: bin 1 is 1333.3 and bin 3 is 4000.
    fn dft(period: &[f64]) -> Vec<f64> {
        (0..period.len())
            .map(|k| {
                let (mut re, mut im) = (0.0, 0.0);
                for (n, level) in period.iter().enumerate() {
                    let angle = -2.0 * std::f64::consts::PI * (k * n) as f64 / period.len() as f64;
                    re += level * angle.cos();
                    im += level * angle.sin();
                }
                re.hypot(im)
            })
            .collect()
    }

    /// 8.5.4 and 8.5.1: Ja and CPt are "scrambled and differentially encoded
    /// by modulo 2 addition of the present bit with the previously
    /// transmitted bit", seeded from the TRN1u before them, and 8.5.2's E1u
    /// carries on from the CPt. Differential decoding is what makes the line
    /// polarity and the seed both irrelevant at the far end, which is just as
    /// well: nothing upstream fixes either.
    #[test]
    fn a_two_point_sequence_decodes_whatever_the_line_polarity() {
        let cpt = a_cpt();
        let descriptor = a_descriptor();
        let mut cpt_bits = cpt.to_bits_in(Layout::V92, JA_UNIT_BITS);
        cpt_bits.extend(cpt.acknowledged().to_bits_in(Layout::V92, JA_UNIT_BITS));

        // Two TRN1u lengths, which leave the differential encoder seeded
        // differently, and both polarities of the line.
        let seeds: Vec<bool> = [12usize, 24].iter().map(|&n| {
            let mut trn1u = Trn1u::new();
            (0..n).for_each(|_| {
                trn1u.next_symbol();
            });
            trn1u.last_bit()
        }).collect();
        assert_ne!(seeds[0], seeds[1], "the two lengths really do seed differently");

        for trn1u in [12usize, 24] {
            for polarity in [1.0, -1.0] {
                let what = format!("{trn1u} TRN1u symbols at polarity {polarity}");
                // Ja: the preamble, then repetitions of the descriptor.
                let mut ja = descriptor.clone();
                ja.extend_from_slice(&descriptor);
                let bits = read_cold(&two_point_stream(trn1u, &ja, polarity));
                let mut finder = DescriptorFinder::v92();
                let found: Vec<Descriptor> = bits.iter().filter_map(|&bit| finder.feed(bit)).collect();
                assert_eq!(found, vec![Descriptor::none(), Descriptor::none()], "Ja at {what}");
                assert_eq!(finder.upstream_rates(), Some(ALL_PCM_UPSTREAM_RATES), "the mask at {what}");

                // CPt, its acknowledged repetition, and E1u behind them.
                let levels = two_point_stream(trn1u, &cpt_bits, polarity);
                let bits = read_cold(&levels);
                let mut finder = CpFinder::v92(JA_UNIT_BITS);
                let found: Vec<bool> = bits.iter().filter_map(|&bit| finder.feed(bit)).map(|cp| cp.acknowledge).collect();
                assert_eq!(found, vec![false, true], "CPt at {what}");
            }
        }
    }

    /// 8.5.1 and 8.5.4: twenty-four ones before the first sequence of a
    /// group. They are exactly what a far end that was not already following
    /// the TRN1u needs -- one symbol for the differential reference and
    /// twenty-three to fill GPA's twenty-three-bit register -- so the sync
    /// behind them is the first thing that descrambles correctly, with
    /// nothing to spare.
    #[test]
    fn twenty_four_ones_let_the_far_descrambler_lock_before_the_sync() {
        assert_eq!(PREAMBLE_ONES, 24);
        let descriptor = a_descriptor();
        let levels = two_point_stream(2040, &descriptor, 1.0);
        let bits = read_cold(&levels[2040..]);
        // The preamble's first symbol is the reference and yields no bit, so
        // twenty-three garbage bits come out of it and then the sequence
        // itself, from its very first bit.
        assert_eq!(bits.len(), PREAMBLE_ONES - 1 + descriptor.len());
        assert_eq!(&bits[PREAMBLE_ONES - 1..], &descriptor[..], "the whole descriptor, undamaged");

        let mut finder = DescriptorFinder::v92();
        assert_eq!(bits.iter().filter_map(|&bit| finder.feed(bit)).count(), 1, "found behind its preamble");

        // And the ones are scrambled (PREAMBLE_IS_SCRAMBLED). Unscrambled
        // ones differentially encoded would be a bare alternation on the
        // line, and would descramble at a locked far end into anything but
        // ones -- which is the whole argument for this reading.
        let preamble = &levels[2040..2040 + PREAMBLE_ONES];
        assert!(!preamble.windows(2).all(|pair| pair[0] != pair[1]), "not a bare alternation");
        let mut locked = TwoPointReader::new();
        levels[..2040].iter().for_each(|&level| {
            locked.feed(level);
        });
        locked.switch_to_differential();
        let read: Vec<bool> = preamble.iter().filter_map(|&level| locked.feed(level)).collect();
        assert_eq!(read, vec![true; PREAMBLE_ONES], "twenty-four ones to a reader that is already locked");

        // Without a preamble the first sequence is lost: the descrambler is
        // still filling while the sync goes past. This is the case the
        // twenty-four ones exist for.
        let mut bare = Trn1u::new();
        (0..2040).for_each(|_| {
            bare.next_symbol();
        });
        let mut sender = TwoPointSender::after(bare);
        sender.push(&descriptor);
        sender.push(&descriptor);
        let mut levels = Vec::new();
        while let Some(level) = sender.next_symbol() {
            levels.push(level);
        }
        let mut finder = DescriptorFinder::v92();
        let found = read_cold(&levels).iter().filter_map(|&bit| finder.feed(bit)).count();
        assert_eq!(found, 1, "only the second of the two");
    }

    /// The other half of the same rule (P3S 3.3, A5): nothing resets the
    /// scrambler between TRN1u and the sequence after it, and the reader's
    /// switch to differential decoding keeps the symbol it lands on as
    /// d(n-1). A far end that followed the TRN1u is therefore still locked at
    /// the join and needs no preamble at all -- which is why a preamble that
    /// was not scrambled would buy nothing.
    #[test]
    fn the_switch_to_differential_keeps_the_descrambler_locked_across_the_join() {
        let descriptor = a_descriptor();
        let mut generator = Trn1u::new();
        let trn1u: Vec<f64> = (0..48).map(|_| generator.next_symbol()).collect();
        let mut sender = TwoPointSender::after(generator);
        sender.push(&descriptor);
        let mut reader = TwoPointReader::new();
        for &level in &trn1u {
            assert!(reader.feed(level).is_some(), "plain reading loses nothing");
        }
        assert!(!reader.is_differential());
        reader.switch_to_differential();
        assert!(reader.is_differential());
        let mut read = Vec::new();
        while let Some(level) = sender.next_symbol() {
            read.push(reader.feed(level).expect("the join replays its symbol"));
        }
        assert_eq!(read, descriptor, "every bit, from the first, with no preamble");
    }

    /// 8.5.2: "E1u is a data frame of scrambled, differentially encoded
    /// zeroes used to signal the end of CPt", twelve symbols upstream. At the
    /// twelve-symbol boundary where one CPt ends, another CPt shows as the
    /// ones of a frame sync and E1u shows as zeros, which is the only thing
    /// telling the analogue modem's receiver that the series is over.
    #[test]
    fn e1u_is_twelve_zeros_and_is_told_from_another_cpt() {
        assert_eq!(E1U_SYMBOLS, 12);
        assert_eq!(E1U_SYMBOLS, UP_SEQUENCE_UNIT, "one upstream data frame");
        let cpt_bits = a_cpt().to_bits_in(Layout::V92, JA_UNIT_BITS);
        let mut tails = Vec::new();
        for ending in [false, true] {
            let mut generator = Trn1u::new();
            let trn1u: Vec<f64> = (0..48).map(|_| generator.next_symbol()).collect();
            let mut sender = TwoPointSender::after(generator);
            sender.preamble();
            sender.push(&cpt_bits);
            if ending {
                sender.e1u();
            } else {
                sender.push(&cpt_bits);
            }
            let mut reader = TwoPointReader::new();
            trn1u.iter().for_each(|&level| {
                reader.feed(level);
            });
            reader.switch_to_differential();
            let mut read = Vec::new();
            while let Some(level) = sender.next_symbol() {
                read.push(reader.feed(level).expect("no bit is lost at the join"));
            }
            assert_eq!(read.len(), PREAMBLE_ONES + cpt_bits.len() + if ending { E1U_SYMBOLS } else { cpt_bits.len() });
            tails.push(read[PREAMBLE_ONES + cpt_bits.len()..][..E1U_SYMBOLS].to_vec());
        }
        assert_eq!(tails[1], vec![false; E1U_SYMBOLS], "E1u is twelve zeros");
        assert_eq!(tails[0], vec![true; E1U_SYMBOLS], "another CPt is the start of its sync");
        assert_ne!(tails[0], tails[1]);
    }

    /// Tables 28 and 29, as the rendered pages print them: the MSB is the
    /// sign, 0 positive, and the rest read as an integer m choosing the odd
    /// magnitude (2m + 1) over sqrt(5) or sqrt(21).
    #[test]
    fn the_tables_map_every_group_to_its_printed_level() {
        let four: Vec<f64> = (0..4u8).map(|group| Trn2uSize::Four.level(group)).collect();
        let r5 = 5.0f64.sqrt();
        assert_eq!(four, vec![LU / r5, 3.0 * LU / r5, -LU / r5, -3.0 * LU / r5]);
        let eight: Vec<f64> = (0..8u8).map(|group| Trn2uSize::Eight.level(group)).collect();
        let r21 = 21.0f64.sqrt();
        let printed = [1.0, 3.0, 5.0, 7.0, -1.0, -3.0, -5.0, -7.0];
        assert_eq!(eight, printed.iter().map(|m| m * LU / r21).collect::<Vec<f64>>());
        // Jp bit 48 during training and bit 49 during a rate renegotiation,
        // 0 = 4-point (8.7.6).
        assert_eq!(Trn2uSize::from_jp_bit(false), Trn2uSize::Four);
        assert_eq!(Trn2uSize::from_jp_bit(true), Trn2uSize::Eight);
        for size in [Trn2uSize::Four, Trn2uSize::Eight] {
            assert_eq!(Trn2uSize::from_jp_bit(size.jp_bit()), size);
            assert_eq!(size.points(), 1 << size.bits());
            // One twelve-symbol frame is 24 bits on four points, 36 on eight.
            assert_eq!(size.bits() * UP_SEQUENCE_UNIT, if size == Trn2uSize::Four { 24 } else { 36 });
            // And the slicer takes every point back, including at the ends.
            // The points are 2/root apart, so anything inside 1/root of one
            // decides to it; 0.8 of that is the margin checked here.
            let margin = 0.8 / size.root();
            for group in 0..size.points() as u8 {
                assert_eq!(size.group_for(size.level(group)), group, "{size:?} group {group}");
                for nudge in [margin, -margin] {
                    assert_eq!(size.group_for(size.level(group) + nudge), group, "{size:?} group {group} off by {nudge}");
                }
            }
            assert_eq!(size.group_for(9.0), (size.points() / 2 - 1) as u8, "clamped to the outer point");
        }
    }

    /// 8.7.6 with Tables 28 and 29: "the average of 1 and 9 over 5" and "of
    /// 1, 9, 25 and 49 over 21" are both one, so TRN2u goes out at LU
    /// squared, the same power as TRN1u and Su. The digital modem estimates
    /// the upstream channel from it, so a level step here would be a level
    /// step in everything it designs.
    #[test]
    fn trn2u_has_mean_square_lu_squared_at_both_sizes() {
        for size in [Trn2uSize::Four, Trn2uSize::Eight] {
            let constellation: Vec<f64> = (0..size.points() as u8).map(|group| size.level(group)).collect();
            assert!((mean_square(&constellation) - LU * LU).abs() < 1e-12, "{size:?}");
            // And over the signal itself, at 9.6.2.1.1's minimum length of
            // 12000T. Scrambled ones are not quite equiprobable over a finite
            // run, so this is a tolerance and not an identity.
            let mut sender = Trn2uSender::seeded(size, false);
            let sent: Vec<f64> = (0..12_000).map(|_| sender.next_symbol()).collect();
            let power = mean_square(&sent);
            println!("{size:?} over 12000T: {power}");
            assert!((power - LU * LU).abs() < 0.05, "{size:?} gave {power}");
            assert!(sent.len().is_multiple_of(UP_SEQUENCE_UNIT), "an integer multiple of 12 symbols");
        }
    }

    /// 8.7.6: TRN2u "consists of scrambled binary ones", and 8.7.3/8.7.5 send
    /// CPu and SUVu on the same modulation with the scrambler running on. So
    /// after a far descrambler has locked, TRN2u reads as ones and the
    /// sequence behind it starts at the first 0 -- which is the rule V.90's
    /// finder does not have, because it wants exactly seventeen ones and gets
    /// hundreds (P4A P6).
    #[test]
    fn trn2u_descrambles_to_ones_and_a_following_sequence_parses_from_its_first_zero() {
        for size in [Trn2uSize::Four, Trn2uSize::Eight] {
            let unit = size.bits() * UP_SEQUENCE_UNIT;
            let cpu = Cp { data_mode: true, drn: 22, ..a_cpt() };
            let mut sender = Trn2uSender::seeded(size, false);
            let mut levels: Vec<f64> = (0..240).map(|_| sender.next_symbol()).collect();
            let bits = cpu.to_bits_in(Layout::V92, unit);
            assert!(bits.len().is_multiple_of(unit), "{} bits fill {unit}", bits.len());
            sender.push(&bits);
            while sender.queued() > 0 {
                levels.push(sender.next_symbol());
            }

            let mut reader = Trn2uReader::new(size);
            let read: Vec<bool> = levels.iter().flat_map(|&level| reader.feed(level)).collect();
            // The first symbol is the reference, so the stream is short by
            // b bits; the descrambler locks 23 bits later, and everything
            // from there to the sequence is ones.
            assert_eq!(read.len(), levels.len() * size.bits() - size.bits());
            let trn2u = &read[23..240 * size.bits() - size.bits()];
            assert!(trn2u.iter().all(|bit| *bit), "{size:?}: TRN2u descrambles to ones");

            let mut finder = CpFinder::v92(unit);
            let found: Vec<Cp> = read.iter().filter_map(|&bit| finder.feed(bit)).collect();
            assert_eq!(found, vec![cpu], "{size:?}: the CPu behind the ones");
        }
    }

    /// Section 4's one switch, and R3: the time order of a TRN2u symbol's
    /// bits is not printed anywhere (P4A A2, RRF Q-6). Sender and reader
    /// agree at either setting, the two settings are different signals, and a
    /// reader on the wrong one gets nothing -- so if a real V.92 server ever
    /// refuses our SUVu, flipping [`TRN2U_SIGN_LAST`] is the whole change.
    #[test]
    fn the_trn2u_bit_order_is_one_switch() {
        for size in [Trn2uSize::Four, Trn2uSize::Eight] {
            let unit = size.bits() * UP_SEQUENCE_UNIT;
            let cpu = Cp { data_mode: true, drn: 22, ..a_cpt() };
            let bits = cpu.to_bits_in(Layout::V92, unit);
            let mut both = Vec::new();
            for first_bit_is_lsb in [true, false] {
                let mut sender = Trn2uSender::seeded(size, false).with_bit_order(first_bit_is_lsb);
                let mut levels: Vec<f64> = (0..120).map(|_| sender.next_symbol()).collect();
                sender.push(&bits);
                while sender.queued() > 0 {
                    levels.push(sender.next_symbol());
                }
                let mut reader = Trn2uReader::new(size).with_bit_order(first_bit_is_lsb);
                let read: Vec<bool> = levels.iter().flat_map(|&level| reader.feed(level)).collect();
                let mut finder = CpFinder::v92(unit);
                let found: Vec<Cp> = read.iter().filter_map(|&bit| finder.feed(bit)).collect();
                assert_eq!(found, vec![cpu.clone()], "{size:?} with the LSB first: {first_bit_is_lsb}");
                both.push(levels);
            }
            assert_ne!(both[0], both[1], "{size:?}: the two readings are different signals");

            // And the far end has to have the same one. The magnitudes still
            // slice, so what a mismatched reader gets is a plausible bit
            // stream that no CRC passes.
            let mut reader = Trn2uReader::new(size).with_bit_order(false);
            let read: Vec<bool> = both[0].iter().flat_map(|&level| reader.feed(level)).collect();
            let mut finder = CpFinder::v92(unit);
            assert_eq!(read.iter().filter_map(|&bit| finder.feed(bit)).count(), 0, "{size:?}: the wrong order finds nothing");
        }
    }

    /// 8.7.6: "The sign bit of TRN2u is differentially encoded by modulo 2
    /// addition of the present sign bit with the previously transmitted sign
    /// bit." Only that bit ([`TRN2U_ONLY_SIGN_IS_DIFFERENTIAL`]): the
    /// magnitude bits go out as the scrambler made them, which is what lets
    /// an inverted line cost nothing at all.
    #[test]
    fn only_the_sign_bit_of_a_trn2u_symbol_is_differential() {
        let size = Trn2uSize::Eight;
        // The same sender seeded both ways: every magnitude is identical and
        // every sign is opposite, which is exactly what one differential bit
        // does and what two or three would not.
        let mut runs = Vec::new();
        for seed in [false, true] {
            let mut sender = Trn2uSender::seeded(size, seed);
            runs.push((0..600).map(|_| sender.next_symbol()).collect::<Vec<f64>>());
        }
        for (a, b) in runs[0].iter().zip(&runs[1]) {
            assert!((a + b).abs() < 1e-12, "{a} against {b}");
        }
        // And a reader does not care which seed was used, nor which way up
        // the line is.
        let mut plain = Trn2uReader::new(size);
        let one: Vec<bool> = runs[0].iter().flat_map(|&level| plain.feed(level)).collect();
        let mut inverted = Trn2uReader::new(size);
        let other: Vec<bool> = runs[1].iter().map(|level| -level).flat_map(|level| inverted.feed(level)).collect();
        assert_eq!(one, other);
        assert!(one[23..].iter().all(|bit| *bit), "scrambled ones, once locked");
    }

    /// 8.7.6 and 8.7.5 with P4A P2: one bit pump for the whole run. The
    /// scrambler is reset at the start of TRN2u and at nothing after it, and
    /// the differential sign runs on from sequence to sequence, so the sender
    /// hands its last sign to whatever follows.
    #[test]
    fn one_pump_carries_trn2u_into_the_sequences_that_follow_it() {
        let size = Trn2uSize::Four;
        let mut sender = Trn2uSender::seeded(size, false);
        assert_eq!(sender.size(), size);
        let first: Vec<f64> = (0..24).map(|_| sender.next_symbol()).collect();
        // A second TRN2u, reset as 8.7.6 requires, repeats the first.
        let mut again = Trn2uSender::seeded(size, sender.last_sign());
        let second: Vec<f64> = (0..24).map(|_| again.next_symbol()).collect();
        let signs = |levels: &[f64]| levels.iter().map(|level| level.is_sign_negative()).collect::<Vec<bool>>();
        assert_ne!(signs(&first), signs(&second), "the seed differs, so the signs do");
        let magnitudes = |levels: &[f64]| levels.iter().map(|level| level.abs()).collect::<Vec<f64>>();
        assert_eq!(magnitudes(&first), magnitudes(&second), "but the scrambler starts again the same way");

        // E1u's last sign is what seeds a Phase 4 TRN2u (8.7.6), and it comes
        // off the two-point sender that sent E1u.
        let mut generator = Trn1u::new();
        (0..48).for_each(|_| {
            generator.next_symbol();
        });
        let mut two_point = TwoPointSender::after(generator);
        two_point.e1u();
        let mut last = 0.0;
        while let Some(level) = two_point.next_symbol() {
            last = level;
        }
        assert_eq!(two_point_level(two_point.last_sign()), last);
        let seeded = Trn2uSender::seeded(size, two_point.last_sign());
        assert_eq!(seeded.last_sign(), two_point.last_sign());
    }
}
