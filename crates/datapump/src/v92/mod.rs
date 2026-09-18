//! ITU-T V.92: V.90 with the upstream turned into PCM as well.
//!
//! V.90 made the downstream a list of codewords the network can carry exactly
//! and left the upstream as ordinary V.34 QAM. V.92 keeps that downstream
//! untouched -- clause 5 is one sentence saying the digital modem is V.90
//! clause 5 -- and does the same trick backwards: the analogue modem now aims
//! its samples at the central office's A/D so that each one lands on a
//! codeword the far end can read, "24 000 bit/s to 48 000 bit/s in increments
//! of 8000/6 bit/s" (1 e).
//!
//! Aiming is the whole difficulty. Downstream the digital modem *is* the
//! network, so its codewords are exact by construction. Upstream the analogue
//! modem sits at the wrong end of a telephone line, and the line, the
//! network's filtering and the A/D's sampling instant all stand between what
//! it sends and what the codec decides. V.92 answers that with three things
//! the numbers below describe: a transmit clock taken from the network through
//! the downstream signal (6.2) and shifted by a fraction of a symbol the
//! digital modem measures and sends back in Jp (8.6.3); a precoder and
//! prefilter that the *digital* modem designs and downloads in CPd (6.4.2);
//! and a twelve-symbol data frame with twelve moduli where V.90 had six
//! (6.4.1, Figure 1).
//!
//! ```text
//! data frame interval i  0  1  2  3  4  5  6  7  8  9 10 11   (12 symbols)
//! constellation frame j  0  1  2  3  4  5  0  1  2  3  4  5   (6 symbols)
//! trellis frame      k   0  1  2  3  0  1  2  3  0  1  2  3   (4 symbols)
//! ```
//!
//! What lives here is that arithmetic, plus the two shapes the rest of the
//! module hangs off. [`Parameters`] is what a CPd *means* once its bits have
//! been read, so the transmitter, the decoder and the design can be built and
//! tested without a wire format existing at all. [`Deadlines`] holds timers by
//! name, because Phase 3 alone runs five of them at once and V.90's single
//! overwritten slot cannot carry that.

pub mod analogue;
pub mod anspcm;
pub mod decoder;
pub mod design;
pub mod digital;
pub mod down_source;
pub mod epsilon;
pub mod exchange;
pub mod hold;
pub mod memo;
pub mod modulus;
pub mod precoder;
pub mod receiver;
pub mod sequences;
pub mod transmit;
pub mod up_signals;
pub mod up_source;
pub mod upchoice;
pub mod upstream;

use crate::v90::RATE_STEP;

// ---------------------------------------------------------------------------
// The upstream frame (6.4, Figure 1)
// ---------------------------------------------------------------------------

/// Data frame intervals in an upstream data frame. Figure 1 (6.4) runs the
/// data frame row from i = 0 to 11, and 8.7.1 says it in words: "A data frame
/// in the upstream direction is 12 symbols long." Downstream keeps V.90's six,
/// which is why this is not `v90::INTERVALS`.
pub const UP_INTERVALS: usize = 12;

/// The constellation frame, "6 symbols" (Figure 1). The constellation set in
/// use depends on j = i mod 6, so CPd names one set per pair (i, i + 6).
pub const CONSTELLATION_FRAME: usize = 6;

/// The trellis frame, "4 symbols" -- one 4D symbol (Figure 1). There are three
/// to a data frame, and the convolutional encoder is clocked once per frame.
pub const TRELLIS_FRAME: usize = 4;

/// The data frame interval a symbol falls in, counting n from the first symbol
/// of B1u, which "begins data frame interval 0" and is n = 0 (8.7.1).
pub fn interval(n: u64) -> usize {
    (n % UP_INTERVALS as u64) as usize
}

/// The constellation frame index j = i mod 6 (Figure 1).
pub fn constellation_index(n: u64) -> usize {
    (n % CONSTELLATION_FRAME as u64) as usize
}

/// The trellis frame index k = i mod 4 (Figure 1).
pub fn trellis_index(n: u64) -> usize {
    (n % TRELLIS_FRAME as u64) as usize
}

// ---------------------------------------------------------------------------
// The upstream rate ladder (6.1)
// ---------------------------------------------------------------------------

/// The lowest and highest upstream rates (6.1): "24 000 bit/s to 48 000 bit/s
/// in increments of 8000/6 bit/s".
pub const UP_SLOWEST: u32 = 24_000;
/// The top of the ladder, which is drn 19.
pub const UP_FASTEST: u32 = 48_000;

/// Rungs on the ladder, one per drn from 1 to 19. drn 0 is not a rate: CPd
/// bits 22:26 and CPu bits 21:25 use it for cleardown (9.11).
pub const UP_RATES: usize = 19;

/// What drn is offset from: "upstream rate = (drn + 17) x 8000/6" (Table 30),
/// against V.90's downstream offset of 20 in CPu bits 21:25.
pub const UP_DRN_OFFSET: u32 = 17;

/// The upstream rate `drn` names, rounded down to whole bit/s.
///
/// The same convention as [`crate::v90::rate_for`]: the ladder's step is
/// 8000/6, which is not a whole number, so the reported rate is the floor of
/// it -- 25 333 for drn 2, 26 666 for drn 3.
pub fn up_rate(drn: u8) -> u32 {
    (u32::from(drn) + UP_DRN_OFFSET) * RATE_STEP.0 / RATE_STEP.1
}

/// How many data bits an upstream data frame carries at `drn`:
/// K = 12 x rate/8000 = 2 x (drn + 17), so 36 bits at 24 000 and 72 at 48 000.
///
/// Derived, not printed: 6.1 gives the rate, Figure 1 and 8.7.1 give the twelve
/// symbols, and 6.4.1 calls the result K -- "For each data frame, K scrambled
/// bits ... enter the modulus encoder."
pub fn up_bits(drn: u8) -> u32 {
    2 * (u32::from(drn) + UP_DRN_OFFSET)
}

// ---------------------------------------------------------------------------
// The Ja rate mask (Table 20)
// ---------------------------------------------------------------------------

/// Every rate set in the Ja DIL descriptor's mask, which is the most an
/// analogue modem can offer.
pub const JA_MASK_ALL: u32 = (1 << UP_RATES) - 1;

/// The mask bit that stands for `drn`, or `None` if `drn` is off the ladder.
///
/// Table 20 lays the mask out by rate, not by drn, in two runs either side of
/// a start bit. Every position in it is offset by the DIL descriptor's
/// beta + ceil(N/2) x 17, written P below: bits 188+P to 203+P are 24 000,
/// 25 333, ... 44 000, and bits 205+P to 207+P are 45 333, 46 666 and 48 000,
/// with 208+P to 220+P reserved. 24 000 is drn 1, so mask bit k is drn k + 1.
///
/// The table prints 46 666, not 46 667, which is the same floor of 46 666 2/3
/// that [`up_rate`] takes -- see the test.
pub fn ja_mask_bit(drn: u8) -> Option<u32> {
    (1..=UP_RATES as u8).contains(&drn).then(|| u32::from(drn) - 1)
}

/// Whether a Ja mask offers `drn`.
pub fn ja_mask_has(mask: u32, drn: u8) -> bool {
    ja_mask_bit(drn).is_some_and(|k| mask & (1 << k) != 0)
}

/// The same mask with `drn` added. A drn off the ladder changes nothing, which
/// keeps the reserved bits 208+P..220+P clear ("sent 0, not interpreted").
pub fn ja_mask_with(mask: u32, drn: u8) -> u32 {
    ja_mask_bit(drn).map_or(mask, |k| mask | (1 << k))
}

/// Every drn a Ja mask offers, slowest first.
pub fn ja_mask_drns(mask: u32) -> Vec<u8> {
    (1..=UP_RATES as u8).filter(|&drn| ja_mask_has(mask, drn)).collect()
}

// ---------------------------------------------------------------------------
// MD, and the filter limits INFO1a announces (Table 18)
// ---------------------------------------------------------------------------

/// MD's step in INFO1a bits 18:24: "276 symbols (34.5 ms)" (Table 18), where
/// V.90's Table 10 counted in 35 ms.
///
/// 276 is 23 x 12, so an MD asked for this way is always a whole number of
/// upstream data frames. 280 symbols, which is what 35 ms would be, is not.
pub const MD_STEP_SYMBOLS: usize = 276;

/// How many symbols an INFO1a MD length of `length` asks for (Table 18 bits
/// 18:24, "integer 0 to 127").
pub fn md_symbols(length: u8) -> usize {
    usize::from(length) * MD_STEP_SYMBOLS
}

/// L_tot by INFO1a bits 14:15: "the maximum number of coefficients the
/// analogue modem supports, in multiples of 64 starting at 192" (Table 18),
/// counting LZ1 + LP1 + LZ2 + LP2 together.
pub const FILTER_TOTALS: [u16; 4] = [192, 256, 320, 384];

/// L_max by INFO1a bits 16:17: "the maximum number of coefficients per filter
/// section, in multiples of 64 starting at 128" (Table 18).
pub const FILTER_EACH: [u16; 4] = [128, 192, 256, 320];

/// Which filter sections the analogue modem supports, from INFO1a bits 12:13.
///
/// The field is printed as four combinations -- "0 = p1(i) and z2(i); 1 = z1,
/// p1, z2; 2 = p1, p2, z2; 3 = z1, p1, p2, z2" (Table 18) -- and p1 and z2 are
/// in all four, so the two bits each stand for one optional section: bit 12
/// for the precoder's feed-forward z1, bit 13 for the prefilter's feedback p2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterSections {
    /// z1, the precoder's feed-forward section.
    pub z1: bool,
    /// p2, the prefilter's feedback section.
    pub p2: bool,
}

impl FilterSections {
    /// From INFO1a bits 12:13.
    pub fn from_code(code: u8) -> Self {
        Self { z1: code & 1 != 0, p2: code & 2 != 0 }
    }

    /// Back to the two bits, for an INFO1a this end sends.
    pub fn code(self) -> u8 {
        u8::from(self.z1) | (u8::from(self.p2) << 1)
    }
}

/// What INFO1a bits 12:17 allow a CPd to download: the sections, the total
/// number of coefficients and the most any one section may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterLimits {
    /// The optional sections, from bits 12:13.
    pub sections: FilterSections,
    /// L_tot, from bits 14:15.
    pub total: u16,
    /// L_max, from bits 16:17.
    pub most: u16,
}

impl FilterLimits {
    /// From INFO1a bits 12:13, 14:15 and 16:17, each a two-bit code.
    pub fn from_info1a(sections: u8, total: u8, most: u8) -> Self {
        Self {
            sections: FilterSections::from_code(sections & 3),
            total: FILTER_TOTALS[usize::from(total & 3)],
            most: FILTER_EACH[usize::from(most & 3)],
        }
    }
}

// ---------------------------------------------------------------------------
// Segment lengths (clause 8, with the 9.5 and 9.6 procedures)
// ---------------------------------------------------------------------------

/// Ru is "384T" and R-bar-u "24T" (9.5.2.1.1), each a repeat of a six-symbol
/// block that bypasses the precoder and prefilter (8.5.5). The same lengths
/// come back at a rate renegotiation (9.8.2.1.1).
pub const RU_SYMBOLS: usize = 384;
/// R-bar-u, the inverted tail that marks the reversal.
pub const RU_BAR_SYMBOLS: usize = 24;

/// Su is "144T" (9.5.2.1.6), the signal whose sampling phase the digital modem
/// measures. Its bar is sent for 24.5T and then for (24 + epsilon)T, which is
/// a lasting delay rather than a length, so it is not a constant here.
pub const SU_SYMBOLS: usize = 144;

/// The TRN1u before Ja: "Signal TRN1u shall be transmitted for at least 2040T"
/// (9.5.2.1.2). "TRN1u segments shall be an integer multiple of 12 symbols in
/// length" (8.5.7) holds for every segment, this one included.
///
/// The *second* TRN1u, sent while the DIL or SCR is received, has a floor of
/// its own and it is conditional: 9.5.2.1.9 asks for "at least 2040T long if a
/// non-zero DIL was requested". Section 4's policy is always to request a
/// non-zero N, so today the two coincide, but a package that ever asks for a
/// zero-length DIL must not hold the second segment to this.
pub const TRN1U_MINIMUM: usize = 2040;

/// B1u is "48 data frames" (8.7.1), which at twelve symbols a frame is 576
/// symbols, and FB1u the same (8.7.7).
pub const B1U_FRAMES: usize = 48;

/// E2u is "one data frame" of scrambled, differentially encoded zeros (8.7.2),
/// with one symbol more when CPd bit 29 asks for it.
pub const E2U_FRAMES: usize = 1;

/// Rf is "384T" and Rf-bar "24T" (9.9.1.1.1, 9.9.1.2.2). Both repeat one
/// twelve-symbol sequence of "the PCM codewords with the sign pattern
/// + + - - + + - - + + - -", left-most sign first (8.8.4).
///
/// Rf is the *digital* modem's signal, so it counts in downstream data frames:
/// the block is two whole [`crate::v90::INTERVALS`] frames, and 9.9.1.1.1 has
/// it "begin on the boundary of a data frame", which 384 keeps.
pub const RF_SYMBOLS: usize = 384;
/// Rf's sign pattern repeats every four symbols. Four and the downstream
/// frame's six meet at twelve, which is why the block is twelve symbols and
/// not six.
pub const RF_SIGN_PERIOD: usize = 4;
/// Rf-bar, "2 repetitions of the 12-symbol sequence" carrying the same
/// codewords with the pattern inverted to "- - + + - - + + - - + +" (8.8.4).
/// Inverting a pattern of period four is the same as shifting it by two, so
/// what marks the reversal is the run of four equal signs at the join.
pub const RF_BAR_SYMBOLS: usize = 24;

/// TR5 and TR6, the watchdog over everything from Phase 2 to data mode: B1u
/// "within 20 s + 6 x RTD from the end of INFO1a" for the digital modem
/// (9.6.1.2.1), and B1d the same from the end of *sending* INFO1a for the
/// analogue modem (9.6.2.2.1). Missing it is a retrain, not a failure.
///
/// V.90's was 15 s + 5 RTD. On the project's VoIP rig, where the round trip
/// runs to about 1.5 s, this reaches roughly 29 s.
pub const START_UP: f64 = 20.0;
/// How many round trips TR5 and TR6 add.
pub const START_UP_RTDS: f64 = 6.0;

/// TR5 and TR6 in seconds, on a line with this round-trip delay.
pub fn start_up_watchdog(round_trip: f64) -> f64 {
    START_UP + START_UP_RTDS * round_trip
}

// ---------------------------------------------------------------------------
// Q formats (3.5)
// ---------------------------------------------------------------------------

/// Unsigned Qa.b (3.5), read as raw / 2^b.
///
/// 3.5 prints the range as "[0, 2^(a+1))", which would make the value
/// raw / 2^(b-1) -- twice this. The printed digit patterns say otherwise: "4G"
/// is ".xxxxxxxxxxxxxxxx", sixteen fractional bits with no integer bit, and
/// CPu's Q3.13 is "xxx.xxxxxxxxxxxxx", three integer bits in sixteen. So the
/// range is [0, 2^a) and the divisor is 2^b. That is the reading fixed here;
/// a capture that disagrees changes this one function.
pub fn unsigned_q(raw: u32, fraction: u32) -> f64 {
    f64::from(raw) / (1u64 << fraction) as f64
}

/// Signed Qa.b (3.5): "an (a+b+1)-bit two's complement number" with b bits
/// after the binary point, in the range [-2^a, 2^a). `width` is the whole
/// field, a + b + 1.
pub fn signed_q(raw: u32, width: u32, fraction: u32) -> f64 {
    let span = 1i64 << width;
    let raw = i64::from(raw) & (span - 1);
    let value = if raw >= span / 2 { raw - span } else { raw };
    value as f64 / (1u64 << fraction) as f64
}

/// A value written back as an unsigned Qa.b field of `width` bits, rounded and
/// clamped to what the field can hold.
pub fn to_unsigned_q(value: f64, width: u32, fraction: u32) -> u32 {
    let scale = (1u64 << fraction) as f64;
    let top = (1u64 << width) - 1;
    (value * scale).round().clamp(0.0, top as f64) as u32
}

/// A value written back as a signed Qa.b field of `width` bits, rounded,
/// clamped to [-2^a, 2^a) and returned in two's complement.
pub fn to_signed_q(value: f64, width: u32, fraction: u32) -> u32 {
    let scale = (1u64 << fraction) as f64;
    let span = 1i64 << width;
    let raw = (value * scale).round().clamp(-(span / 2) as f64, (span / 2 - 1) as f64) as i64;
    (raw & (span - 1)) as u32
}

/// What CPd bits 35:50 are divided by to give G.
///
/// The field is "4 x G > 0" in "unsigned Q0.16" (Table 30), so 4G is
/// raw / 65 536 and G is raw / 262 144. Under the alternative reading of 3.5's
/// printed range (see [`unsigned_q`]) it would be raw / 131 072.
pub const GAIN_SCALE: f64 = 262_144.0;

/// The largest G a CPd can carry. The field is sixteen bits wide, so 4G stops
/// just short of 1 and the top value 0xFFFF is 65 535 / [`GAIN_SCALE`] --
/// 0.249 996 under the reading fixed here, half of what an unwary reader of
/// "4 x G" might assume the range to be (Table 30, bits 35:50). It is derived
/// from `GAIN_SCALE` so that flipping the [`unsigned_q`] reading moves this
/// too.
pub const GAIN_LARGEST: f64 = u16::MAX as f64 / GAIN_SCALE;

/// G from CPd bits 35:50.
pub fn gain_from_4g(raw: u16) -> f64 {
    f64::from(raw) / GAIN_SCALE
}

/// CPd bits 35:50 from G. Table 30 requires 4 x G > 0, so a gain that rounds
/// to nothing is sent as the smallest step instead of as zero.
///
/// A gain above [`GAIN_LARGEST`] has no field to go in and is clamped to
/// 0xFFFF, which would put a quieter modem on the wire than the design asked
/// for. [`Parameters::fits`] refuses that before it reaches here, so the clamp
/// only ever catches rounding at the very top of the range.
pub fn four_g_from_gain(gain: f64) -> u16 {
    (gain * GAIN_SCALE).round().clamp(1.0, f64::from(u16::MAX)) as u16
}

// ---------------------------------------------------------------------------
// Parameters: what a CPd means (AD-3)
// ---------------------------------------------------------------------------

/// The convolutional code the digital modem requires of the analogue
/// transmitter, from CPd bits 27:28: "0 = 16-state, 1 = 32-state, 2 = 64-state,
/// 3 = reserved" (Table 30). They are the V.34 codes of 9.6.3.2/V.34 with the
/// 2T delays replaced by 4T ones (6.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trellis {
    /// V.34 Figure 10, rate 2/3.
    Sixteen,
    /// V.34 Figure 11, rate 3/4.
    ThirtyTwo,
    /// V.34 Figure 12, rate 4/5.
    SixtyFour,
}

impl Trellis {
    /// From CPd bits 27:28. Code 3 is reserved and has no code to select.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Sixteen),
            1 => Some(Self::ThirtyTwo),
            2 => Some(Self::SixtyFour),
            _ => None,
        }
    }

    /// Back to CPd bits 27:28.
    pub fn code(self) -> u8 {
        match self {
            Self::Sixteen => 0,
            Self::ThirtyTwo => 1,
            Self::SixtyFour => 2,
        }
    }

    /// How many states the code has.
    pub fn states(self) -> u8 {
        match self {
            Self::Sixteen => 16,
            Self::ThirtyTwo => 32,
            Self::SixtyFour => 64,
        }
    }
}

/// The precoder and prefilter coefficients, in the order 6.4.2 uses them.
///
/// The two feed-forward sections are indexed differently and it matters:
/// x(n) = u(n) + sum over kappa = 1..LZ1 of u(n-kappa) z1(kappa) + sum over
/// kappa = 1..LP1 of x(n-kappa) p1(kappa), while
/// v(n) = sum over kappa = 0..LZ2-1 of x(n-kappa) z2(kappa) + sum over
/// kappa = 1..LP2 of v(n-kappa) p2(kappa). So `z1[0]` is z1(1) and `z2[0]` is
/// z2(0), the tap on the current sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filters {
    /// z1(1..LZ1), the precoder's feed-forward section. Empty unless INFO1a
    /// bit 12 offered it.
    pub z1: Vec<f64>,
    /// p1(1..LP1), the precoder's feedback section.
    pub p1: Vec<f64>,
    /// z2(0..LZ2-1), the prefilter's feed-forward section.
    pub z2: Vec<f64>,
    /// p2(1..LP2), the prefilter's feedback section. Empty unless INFO1a
    /// bit 13 offered it.
    pub p2: Vec<f64>,
}

impl Filters {
    /// LZ1 + LP1 + LZ2 + LP2, which 8.8.3 holds to L_tot.
    pub fn total(&self) -> usize {
        self.z1.len() + self.p1.len() + self.z2.len() + self.p2.len()
    }

    /// The longest section, which 8.8.3 holds to L_max.
    pub fn most(&self) -> usize {
        self.z1.len().max(self.p1.len()).max(self.z2.len()).max(self.p2.len())
    }

    /// Whether these fit what INFO1a announced (8.8.3, with Table 18 bits
    /// 12:17).
    pub fn fits(&self, limits: &FilterLimits) -> Result<(), &'static str> {
        if !limits.sections.z1 && !self.z1.is_empty() {
            return Err("the precoder has a feed-forward section this end did not offer");
        }
        if !limits.sections.p2 && !self.p2.is_empty() {
            return Err("the prefilter has a feedback section this end did not offer");
        }
        if self.total() > usize::from(limits.total) {
            return Err("the filters have more coefficients than this end offered");
        }
        if self.most() > usize::from(limits.most) {
            return Err("a filter section is longer than this end offered");
        }
        Ok(())
    }
}

/// Everything CPd settles about the upstream, with the bit layout left behind
/// (AD-3).
///
/// The transmitter, the decoder and the digital modem's design all speak this
/// and never Table 30, so each can be built and tested before the wire format
/// exists, and only `v92::sequences` ever has to know where a field sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameters {
    /// CPd bits 22:26, the rung of the 6.1 ladder. drn 0 is cleardown and
    /// carries no parameters.
    pub drn: u8,
    /// CPd bits 27:28.
    pub trellis: Trellis,
    /// CPd bit 29: "extend E2u by 1 symbol". It "shall be 0" in a rate
    /// renegotiation or a fast parameter exchange.
    pub extend_e2u: bool,
    /// G, the gain at the prefilter output (CPd bits 35:50 over 262 144).
    pub gain: f64,
    /// M0..M11, "the number of positive levels" the modulus encoder works to
    /// in each of the twelve intervals (CPd bits 52:152).
    pub moduli: [u8; UP_INTERVALS],
    /// The precoder and prefilter coefficients.
    pub filters: Filters,
    /// The constellation sets, each the positive magnitudes of one set in
    /// ascending order: "one word per point, ascending magnitude". A set of
    /// LC points becomes N = 2 x LC levels, a(eta) = P[eta] for eta >= 0 and
    /// -P[-eta-1] for eta < 0.
    pub sets: Vec<Vec<u16>>,
    /// Which set each constellation frame index j = 0..5 uses, and so each
    /// pair of intervals (j, j + 6): CPd bits 222+alpha onward.
    pub indices: [u8; CONSTELLATION_FRAME],
}

impl Parameters {
    /// K, the data bits in one twelve-symbol frame at this rate.
    pub fn bits(&self) -> u32 {
        up_bits(self.drn)
    }

    /// The upstream rate, rounded down as [`up_rate`] rounds it.
    pub fn rate(&self) -> u32 {
        up_rate(self.drn)
    }

    /// M = M0 x M1 x ... x M11, which reaches 255^12 -- about 2^96, so it is
    /// kept in a u128 (6.4.1).
    pub fn product(&self) -> u128 {
        self.moduli.iter().map(|&m| u128::from(m)).product()
    }

    /// The modulus for data frame interval i.
    pub fn modulus(&self, i: usize) -> u8 {
        self.moduli[i % UP_INTERVALS]
    }

    /// The constellation for data frame interval i, by way of j = i mod 6.
    pub fn set_for(&self, i: usize) -> Option<&[u16]> {
        let index = usize::from(self.indices[i % CONSTELLATION_FRAME]);
        self.sets.get(index).map(Vec::as_slice)
    }

    /// Whether these parameters are self-consistent: everything 8.8.3 and
    /// 6.4 require of a set of parameters on their own, before INFO1a's limits
    /// are brought in ([`Filters::fits`] does those).
    ///
    /// The class-feasibility rules are the two the Recommendation never
    /// states: an equivalence class E(Ki) is non-empty for every Ki only if
    /// N >= Mi, and at k = 3, where the class steps by 2 x Mi, only if
    /// N >= 2 x Mi (6.4.2).
    pub fn fits(&self) -> Result<(), &'static str> {
        if !(1..=UP_RATES as u8).contains(&self.drn) {
            return Err("the upstream rate is not a rung of the ladder");
        }
        if !self.gain.is_finite() || self.gain <= 0.0 {
            return Err("the gain is not above zero");
        }
        if self.gain > GAIN_LARGEST {
            return Err("the gain is larger than cpd can carry");
        }
        if self.moduli.contains(&0) {
            return Err("an interval has no modulus");
        }
        if u128::from(self.bits()) >= 128 || (1u128 << self.bits()) > self.product() {
            return Err("the data frame carries more bits than the moduli can hold");
        }
        if self.filters.z2.is_empty() {
            return Err("the prefilter has no feed-forward section");
        }
        for points in &self.sets {
            if points.is_empty() {
                return Err("a constellation set is empty");
            }
            if points[0] == 0 {
                return Err("a constellation set contains the zero point");
            }
            if points.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err("a constellation set is not in ascending magnitude");
            }
        }
        for i in 0..UP_INTERVALS {
            let Some(points) = self.set_for(i) else {
                return Err("a constellation index points past the sets");
            };
            let n = 2 * points.len();
            let wanted = if i % TRELLIS_FRAME == TRELLIS_FRAME - 1 { 2 } else { 1 } * usize::from(self.modulus(i));
            if n < wanted {
                return Err("an interval's constellation is too small for its modulus");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Deadlines (AD-10)
// ---------------------------------------------------------------------------

/// One deadline: the sample it falls at, and the lowercase reason to report if
/// nothing has happened by then.
pub type Deadline = (u64, &'static str);

/// Timers held by name, checked together.
///
/// V.90's analogue modem has one `Option<(u64, &'static str)>` that five
/// places overwrite, which works because only one of its timers is ever live.
/// V.92 Phase 3 alone runs TR3, TR4, the Su window, the Jp wait and TR6 at
/// once, and Phase 4 adds the CP repeat window on top, so they have to be
/// separable.
///
/// Slots are named by `&'static str` rather than by an enum variant on
/// purpose: a later package can arm a timer this module has never heard of
/// without editing this file, and the name is what turns up in a trace.
#[derive(Debug, Clone, Default)]
pub struct Deadlines {
    slots: Vec<(&'static str, Deadline)>,
}

impl Deadlines {
    /// Nothing armed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm `name` to fall at sample `at`, reporting `why` if it does. Arming a
    /// name that is already armed replaces it, which is how a timer is
    /// restarted.
    pub fn arm(&mut self, name: &'static str, at: u64, why: &'static str) {
        match self.slots.iter_mut().find(|(slot, _)| *slot == name) {
            Some((_, deadline)) => *deadline = (at, why),
            None => self.slots.push((name, (at, why))),
        }
    }

    /// Disarm `name`, whether or not it was armed.
    pub fn clear(&mut self, name: &'static str) {
        self.slots.retain(|(slot, _)| *slot != name);
    }

    /// Disarm everything, which is what a retrain does.
    pub fn clear_all(&mut self) {
        self.slots.clear();
    }

    /// What `name` is armed for, if it is.
    pub fn armed(&self, name: &'static str) -> Option<Deadline> {
        self.slots.iter().find(|(slot, _)| *slot == name).map(|(_, deadline)| *deadline)
    }

    /// The earliest slot that has passed, as its name and its reason, or
    /// `None` while they all still have time.
    ///
    /// Earliest, not first armed: when two have gone by the time the modem
    /// looks -- which a slip or a long block of samples makes ordinary -- the
    /// one that would have fired first is the one that says what went wrong.
    pub fn expired(&self, now: u64) -> Option<(&'static str, &'static str)> {
        self.slots
            .iter()
            .filter(|(_, (at, _))| now > *at)
            .min_by_key(|(_, (at, _))| *at)
            .map(|(name, (_, why))| (*name, *why))
    }
}

// ---------------------------------------------------------------------------
// What the shared SUV/CP/E exchange is driven by (AD-9)
// ---------------------------------------------------------------------------

/// The flags the exchange of 9.6 takes from a peer's SUV, whichever side sent
/// it: SUVu bits 26, 32 and 33 (Table 27) and SUVd bits 32 and 33 (Table 31).
///
/// They live here, not in `v92::exchange`, because the exchange is deliberately
/// side-agnostic and knows nothing of the wire types.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerSuv {
    /// "The other modem's CP sequence has been received."
    pub ack: bool,
    /// A silent period is being asked for, or granted (9.8.1.1.3). Only a rate
    /// renegotiation defines one.
    pub silence: bool,
    /// SUVu bit 26: wait for my CPu before sending CPd (9.6.1.1.2, a [MAY]).
    pub wait_for_cp: bool,
}

/// The one flag the exchange takes from a peer's CP, which is bit 33 in every
/// one of them: CPu and CPt (Table 23), CPus (Table 24) and CPd (Table 30) all
/// print "Acknowledge bit" at 33 and "Start bit: 0" at 34. Bit 34 is therefore
/// never the acknowledge -- an acknowledge written there goes out clear and
/// every primed sequence reads as unacknowledged. `v92::sequences` owns the
/// encoding; the position is here only so that it is not guessed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerCp {
    /// "The other modem's CP sequence has been received."
    pub ack: bool,
}

/// What this end is sending, as far as the exchange needs to know: the
/// acknowledge bit may only change at a sequence boundary, and E may only
/// follow an acknowledged sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceKind {
    /// TRN2u or TRN2d.
    Trn2,
    /// SUVu or SUVd, and their primed forms.
    Suv,
    /// CPu or CPd, and their primed forms.
    Cp,
    /// CPus, the short form of CPu, which counts as a CP.
    Cpus,
    /// E2u or Ed.
    E,
    /// B1u or B1d, and FB1u.
    B1,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 e) and 6.1: "24 000 bit/s to 48 000 bit/s in increments of
    /// 8000/6 bit/s", carried in CPd bits 22:26 as drn 1..19.
    #[test]
    fn the_upstream_ladder_runs_from_24000_to_48000_in_steps_of_8000_over_6() {
        assert_eq!(up_rate(1), UP_SLOWEST);
        assert_eq!(up_rate(UP_RATES as u8), UP_FASTEST);
        // Nineteen rungs, and each one step of 8000/6 above the last.
        let rungs: Vec<u32> = (1..=UP_RATES as u8).map(up_rate).collect();
        assert_eq!(rungs.len(), UP_RATES);
        for pair in rungs.windows(2) {
            let step = pair[1] - pair[0];
            assert!(step == 1333 || step == 1334, "{pair:?} was a step of {step}");
        }
        // 6.1 prints no table -- it is three lines of prose giving the range
        // and the step -- so the rungs below are derived from that step, and
        // spot-checked here where the fraction bites. The two Table 20 prints,
        // 45 333 and 46 666, are checked against the table in the Ja test.
        assert_eq!(up_rate(2), 25_333, "25 1/3 rounded down");
        assert_eq!(up_rate(3), 26_666, "26 2/3 rounded down");
        assert_eq!(up_rate(13), 40_000);
        // Rounding down is what v90::rate_for does, and the two ladders share
        // the step, so they agree wherever they meet.
        assert_eq!(up_rate(1), crate::v90::rate_for(up_bits(1) / 2));
    }

    /// 6.1's rate over Figure 1's twelve-symbol frame: K = 12 x rate/8000 =
    /// 2 x (drn + 17) bits, the K that 6.4.1 feeds to the modulus encoder.
    #[test]
    fn a_data_frame_holds_2_drn_plus_34_bits() {
        assert_eq!(up_bits(1), 36);
        assert_eq!(up_bits(UP_RATES as u8), 72);
        for drn in 1..=UP_RATES as u8 {
            let k = up_bits(drn);
            assert_eq!(k, 2 * (u32::from(drn) + 17), "drn {drn}");
            assert!((36..=72).contains(&k) && k.is_multiple_of(2), "drn {drn} gave K = {k}");
            // The rate is those bits spread over the frame. Both sides floor
            // the same fraction, so this holds exactly at every rung, which
            // `up_bits(drn) * 8000 == up_rate(drn) * 12` does not: only the
            // seven rungs with 3 | (drn + 17) land on a whole bit/s.
            assert_eq!(k * 8000 / UP_INTERVALS as u32, up_rate(drn), "drn {drn}");
            if (u32::from(drn) + UP_DRN_OFFSET).is_multiple_of(3) {
                assert_eq!(u64::from(k) * 8000, u64::from(up_rate(drn)) * 12, "drn {drn}");
            }
        }
    }

    /// Figure 1: twelve data frame intervals, "constellation frame index"
    /// j = i mod 6 and "trellis frame index" k = i mod 4, with n counted from
    /// the first symbol of B1u (8.7.1).
    #[test]
    fn a_twelve_symbol_frame_holds_two_constellation_frames_and_three_trellis_frames() {
        assert_eq!(UP_INTERVALS / CONSTELLATION_FRAME, 2);
        assert_eq!(UP_INTERVALS / TRELLIS_FRAME, 3);
        // The three rows of Figure 1, as printed.
        let printed_j = [0, 1, 2, 3, 4, 5, 0, 1, 2, 3, 4, 5];
        let printed_k = [0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3];
        for n in 0..24u64 {
            let i = interval(n);
            assert_eq!(i, (n % 12) as usize);
            assert_eq!(constellation_index(n), printed_j[i]);
            assert_eq!(trellis_index(n), printed_k[i]);
        }
        // B1u's first symbol "begins data frame interval 0" and is n = 0.
        assert_eq!((interval(0), constellation_index(0), trellis_index(0)), (0, 0, 0));
    }

    /// 3.5 and Table 30: the Q formats read as the digit patterns the
    /// Recommendation prints, not as the range 3.5 states (see `unsigned_q`).
    #[test]
    fn the_q_formats_read_as_the_printed_digit_patterns() {
        // 4G is unsigned Q0.16, ".xxxxxxxxxxxxxxxx": 0x4000 is a quarter, so
        // G is a sixteenth.
        assert_eq!(unsigned_q(0x4000, 16), 0.25);
        assert_eq!(gain_from_4g(0x4000), 1.0 / 16.0);
        assert_eq!(four_g_from_gain(1.0 / 16.0), 0x4000);
        // CPu's a1, a2, b1, b2 are signed Q1.6, "sx.xxxxxx", eight bits wide.
        assert_eq!(signed_q(0x80, 8, 6), -2.0);
        assert_eq!(signed_q(0x3F, 8, 6), 63.0 / 64.0);
        assert_eq!(to_signed_q(-2.0, 8, 6), 0x80);
        // CPu bits 52:67 are unsigned Q3.13, "xxx.xxxxxxxxxxxxx".
        assert_eq!(unsigned_q(0x2000, 13), 1.0);
        assert_eq!(to_unsigned_q(1.0, 16, 13), 0x2000);
        // The coefficient formats of Table 30, at both ends of their ranges.
        assert_eq!(signed_q(0x8000, 16, 15), -1.0, "z1, z2: signed Q0.15");
        assert_eq!(signed_q(0x8000, 16, 14), -2.0, "p1, p2: signed Q1.14");
        assert_eq!(signed_q(0x10, 5, 2), -4.0, "SUVu bits 27:31: signed Q2.2");
        assert_eq!(signed_q(0x0F, 5, 2), 3.75);
        // Clamping, so a design that asks for too much still sends a legal
        // field, and 4G is never the zero Table 30 forbids.
        assert_eq!(to_signed_q(9.0, 5, 2), 0x0F);
        assert_eq!(to_signed_q(-9.0, 5, 2), 0x10);
        assert_eq!(four_g_from_gain(0.0), 1);
        assert_eq!(four_g_from_gain(1.0), u16::MAX);
        // Sixteen bits of 4G put the top of the range just under a quarter,
        // and it round-trips there.
        assert_eq!(gain_from_4g(u16::MAX), GAIN_LARGEST);
        assert_eq!(four_g_from_gain(GAIN_LARGEST), u16::MAX);
        assert!((gain_from_4g(u16::MAX) - 0.25).abs() < 1e-5, "4G stops just short of one");
        assert!(gain_from_4g(u16::MAX) < 0.25);
    }

    /// Table 18 bits 18:24: the MD the analogue modem sends, "in steps of
    /// 276 symbols (34.5 ms)", against V.90 Table 10's 35 ms.
    #[test]
    fn md_length_counts_in_276_symbols() {
        assert_eq!(md_symbols(0), 0);
        assert_eq!(md_symbols(1), 276);
        assert_eq!(md_symbols(127), 35_052);
        // 276 = 23 x 12, so an MD is always a whole number of data frames --
        // which 35 ms, or 280 symbols, would not be.
        assert!(MD_STEP_SYMBOLS.is_multiple_of(UP_INTERVALS));
        assert_eq!(MD_STEP_SYMBOLS, 23 * UP_INTERVALS);
        assert!(!280usize.is_multiple_of(UP_INTERVALS));
        // 34.5 ms a step at 8000 symbols a second.
        assert!((MD_STEP_SYMBOLS as f64 / 8000.0 - 0.0345).abs() < 1e-9);
    }

    /// Table 18 bits 12:17: the filter sections the analogue modem supports,
    /// L_tot "in multiples of 64 starting at 192" and L_max "in multiples of
    /// 64 starting at 128".
    #[test]
    fn the_filter_limits_decode_from_info1a() {
        for code in 0..4u8 {
            let limits = FilterLimits::from_info1a(code, code, code);
            assert_eq!(limits.total, FILTER_TOTALS[usize::from(code)]);
            assert_eq!(limits.most, FILTER_EACH[usize::from(code)]);
            assert_eq!(limits.total, 192 + 64 * u16::from(code));
            assert_eq!(limits.most, 128 + 64 * u16::from(code));
            assert_eq!(limits.sections.code(), code);
        }
        // The four combinations as Table 18 prints them: p1 and z2 are in all
        // of them, so the two bits are z1 and p2.
        assert_eq!(FilterSections::from_code(0), FilterSections { z1: false, p2: false });
        assert_eq!(FilterSections::from_code(1), FilterSections { z1: true, p2: false });
        assert_eq!(FilterSections::from_code(2), FilterSections { z1: false, p2: true });
        assert_eq!(FilterSections::from_code(3), FilterSections { z1: true, p2: true });
        // And a CPd is held to what was announced (8.8.3). Code 0 offers
        // neither optional section, so those two refusals are what a z1 or a
        // p2 meets first.
        let announced = FilterLimits::from_info1a(0, 0, 0);
        let mut filters = Filters { z2: vec![1.0; 128], p1: vec![0.0; 64], ..Filters::default() };
        assert_eq!(filters.fits(&announced), Ok(()));
        filters.z1.push(0.5);
        assert_eq!(
            filters.fits(&announced),
            Err("the precoder has a feed-forward section this end did not offer")
        );
        filters.z1.clear();
        filters.p2.push(0.5);
        assert_eq!(
            filters.fits(&announced),
            Err("the prefilter has a feedback section this end did not offer")
        );

        // L_tot and L_max are separate refusals and each case below can only
        // be caught by its own, so deleting either branch fails this test:
        // 128 + 128 sits exactly on L_max but over L_tot, and one 129-tap
        // section sits well inside L_tot but over L_max. Both sections are
        // offered here, so neither case can trip on bits 12:13 instead.
        let both = FilterLimits::from_info1a(3, 0, 0);
        let over_total = Filters { z2: vec![1.0; 128], p1: vec![0.0; 128], ..Filters::default() };
        assert_eq!(over_total.most(), usize::from(both.most), "L_max is not what catches this");
        assert_eq!(
            over_total.fits(&both),
            Err("the filters have more coefficients than this end offered")
        );
        let over_section = Filters { z2: vec![1.0; 129], ..Filters::default() };
        assert!(over_section.total() < usize::from(both.total), "L_tot is not what catches this");
        assert_eq!(
            over_section.fits(&both),
            Err("a filter section is longer than this end offered")
        );
    }

    /// AD-10: five Phase 3 timers run at once, so the one that says what went
    /// wrong is the earliest that has passed, not the first one looked at.
    #[test]
    fn deadlines_report_the_earliest_that_has_passed() {
        let mut deadlines = Deadlines::new();
        assert_eq!(deadlines.expired(1_000_000), None);
        // TR6 armed at the end of INFO1a, then the Phase 3 timers inside it.
        deadlines.arm("TR6", 8000 * 26, "no B1d from the digital modem");
        deadlines.arm("TR3", 8000 * 5, "no Sd from the digital modem");
        deadlines.arm("TR4", 8000 * 9, "no Jd from the digital modem");
        assert_eq!(deadlines.armed("TR3"), Some((8000 * 5, "no Sd from the digital modem")));
        assert_eq!(deadlines.armed("TR5"), None);
        // Nothing has passed yet, and a deadline exactly reached has not
        // passed -- the V.90 modems compare `now > at`.
        assert_eq!(deadlines.expired(8000 * 5), None);
        // Both Phase 3 timers gone by the time the modem looks: TR3 is what
        // is reported, because it would have fired first.
        assert_eq!(deadlines.expired(8000 * 10), Some(("TR3", "no Sd from the digital modem")));
        deadlines.clear("TR3");
        assert_eq!(deadlines.expired(8000 * 10), Some(("TR4", "no Jd from the digital modem")));
        // Re-arming restarts a timer rather than adding a second one.
        deadlines.arm("TR4", 8000 * 20, "no Jd from the digital modem");
        assert_eq!(deadlines.expired(8000 * 10), None);
        assert_eq!(deadlines.expired(8000 * 30), Some(("TR4", "no Jd from the digital modem")));
        // And a retrain drops the lot.
        deadlines.clear_all();
        assert_eq!(deadlines.expired(8000 * 30), None);
    }

    /// Table 20: the Ja mask runs by rate from 24 000, and 24 000 is drn 1.
    #[test]
    fn the_ja_mask_counts_rates_from_drn_one() {
        assert_eq!(ja_mask_bit(1), Some(0));
        assert_eq!(ja_mask_bit(19), Some(18));
        assert_eq!(ja_mask_bit(0), None, "drn 0 is cleardown, not a rate");
        assert_eq!(ja_mask_bit(20), None);
        // The first run is 24 000 to 44 000, the second 45 333 to 48 000, and
        // Table 20 prints every one of them. The second run is where the
        // ladder's floor shows: 46 666 2/3 is printed 46 666.
        assert_eq!(up_rate(16), 44_000, "the last rate of mask run 1");
        assert_eq!(up_rate(17), 45_333, "the first rate of mask run 2");
        assert_eq!(up_rate(18), 46_666, "Table 20 prints 46 666, not 46 667");
        assert_eq!(up_rate(19), 48_000, "the last rate of mask run 2");
        let mask = ja_mask_with(ja_mask_with(0, 19), 1);
        assert_eq!(ja_mask_drns(mask), vec![1, 19]);
        assert!(ja_mask_has(mask, 19) && !ja_mask_has(mask, 18));
        // A rate off the ladder leaves the reserved bits at 208+P alone.
        assert_eq!(ja_mask_with(mask, 20), mask);
        assert_eq!(ja_mask_drns(JA_MASK_ALL).len(), UP_RATES);
        assert_eq!(JA_MASK_ALL, 0x7_FFFF);
    }

    /// 9.6.1.2.1 and 9.6.2.2.1: "20 s + 6 x RTD", where V.90 9.4 had
    /// 15 s + 5 RTD.
    #[test]
    fn the_start_up_watchdog_is_twenty_seconds_and_six_round_trips() {
        assert_eq!(start_up_watchdog(0.0), 20.0);
        assert!((start_up_watchdog(0.05) - 20.3).abs() < 1e-9);
        // The project's VoIP rig, about 1.5 s each way round.
        assert!((start_up_watchdog(1.5) - 29.0).abs() < 1e-9);
        assert!(start_up_watchdog(1.5) > 15.0 + 5.0 * 1.5, "looser than V.90's");
    }

    /// Table 30 and 6.4: a set of parameters has to hold together on its own
    /// before anything sends it -- 2^K <= M (6.4.1), and a class that has a
    /// member for every Ki (6.4.2).
    #[test]
    fn parameters_check_themselves_against_clause_6() {
        let params = Parameters {
            drn: 1,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            gain: 1.0 / 16.0,
            moduli: [8; UP_INTERVALS],
            filters: Filters { z2: vec![1.0], ..Filters::default() },
            sets: vec![(1..=16u16).map(|p| p * 100).collect()],
            indices: [0; CONSTELLATION_FRAME],
        };
        // K = 36 bits, M = 8^12 = 2^36, so the frame fits exactly; N = 32 is
        // 4 x M0, which leaves room at k = 3 as well.
        assert_eq!(params.bits(), 36);
        assert_eq!(params.product(), 1u128 << 36);
        assert_eq!(params.fits(), Ok(()));
        assert_eq!(params.rate(), UP_SLOWEST);
        assert_eq!(params.modulus(12), 8, "intervals wrap at twelve");
        assert_eq!(params.set_for(7).map(<[u16]>::len), Some(16));

        // One bit more than the moduli can hold.
        let mut faster = params.clone();
        faster.drn = 2;
        assert!(faster.fits().is_err(), "K = 38 does not fit 2^36");

        // N >= Mi everywhere, but k = 3 needs N >= 2 Mi: intervals 3, 7 and 11.
        let mut tight = params.clone();
        tight.moduli = [17; UP_INTERVALS];
        assert!(tight.fits().is_err(), "N = 32 is not 2 x 17 at k = 3");
        tight.moduli[3] = 16;
        tight.moduli[7] = 16;
        tight.moduli[11] = 16;
        assert_eq!(tight.fits(), Ok(()), "now every class has a member, 32 = 2 x 16");

        // The rest of the self-checks, each one a SHALL of 8.8.3 or a rule of
        // clause 6.
        let mut broken = params.clone();
        broken.drn = 0;
        assert!(broken.fits().is_err(), "drn 0 is cleardown");
        broken = params.clone();
        broken.gain = 0.0;
        assert!(broken.fits().is_err(), "4G > 0");
        // And the other end of that field: 4G is sixteen bits, so a design
        // asking for a third of a volt has nowhere to put it and would be
        // clamped down to 0.249 996 without a word.
        broken = params.clone();
        broken.gain = 0.30;
        assert_eq!(broken.fits(), Err("the gain is larger than cpd can carry"));
        assert_eq!(four_g_from_gain(0.30), u16::MAX, "what the clamp would have done");
        broken.gain = GAIN_LARGEST;
        assert_eq!(broken.fits(), Ok(()), "the top of the field is still legal");
        broken = params.clone();
        broken.moduli[5] = 0;
        assert!(broken.fits().is_err());
        broken = params.clone();
        broken.filters.z2.clear();
        assert!(broken.fits().is_err(), "LZ2 = 0 makes v(n) zero");
        broken = params.clone();
        broken.sets[0][0] = 0;
        assert!(broken.fits().is_err(), "constellations shall not contain the zero point");
        broken = params.clone();
        broken.sets[0].swap(0, 1);
        assert!(broken.fits().is_err(), "points are sent in ascending magnitude");
        broken = params.clone();
        broken.indices[2] = 1;
        assert!(broken.fits().is_err(), "there is no set 1");

        // 255^12 is about 2^96, which is why the product is a u128.
        let widest = Parameters { drn: 19, moduli: [255; UP_INTERVALS], ..params };
        assert!(widest.product() > u128::from(u64::MAX));
        assert_eq!(widest.bits(), 72);
    }

    /// Table 30 bits 27:28: "0 = 16-state, 1 = 32-state, 2 = 64-state,
    /// 3 = reserved".
    #[test]
    fn the_trellis_code_is_chosen_by_two_bits_and_three_is_reserved() {
        for (code, expected, states) in [(0, Trellis::Sixteen, 16), (1, Trellis::ThirtyTwo, 32), (2, Trellis::SixtyFour, 64)] {
            let trellis = Trellis::from_code(code).expect("a defined code");
            assert_eq!(trellis, expected);
            assert_eq!(trellis.code(), code);
            assert_eq!(trellis.states(), states);
        }
        assert_eq!(Trellis::from_code(3), None);
    }

    /// Clause 8 segment lengths, which the sources and the receivers count in.
    #[test]
    fn the_segment_lengths_are_the_printed_ones() {
        assert_eq!((RU_SYMBOLS, RU_BAR_SYMBOLS), (384, 24));
        assert_eq!((RF_SYMBOLS, RF_BAR_SYMBOLS, RF_SIGN_PERIOD), (384, 24, 4));
        assert_eq!(SU_SYMBOLS, 144);
        assert_eq!(TRN1U_MINIMUM, 2040);
        assert_eq!((B1U_FRAMES, E2U_FRAMES), (48, 1));
        // 8.5.6 and 8.5.7: Su and TRN1u are whole numbers of twelve symbols,
        // and so are Ru and its bar, which are six-symbol blocks.
        for symbols in [RU_SYMBOLS, RU_BAR_SYMBOLS, SU_SYMBOLS, TRN1U_MINIMUM] {
            assert!(symbols.is_multiple_of(UP_INTERVALS), "{symbols} symbols");
        }
        // B1u is 48 x 12 = 576 symbols, against B1d's 48 x 6 = 288.
        assert_eq!(B1U_FRAMES * UP_INTERVALS, 576);

        // Rf is the digital modem's, so it counts in downstream data frames:
        // 8.8.4's block is two of them, and 9.9.1.1.1 has Rf begin on a data
        // frame boundary, which both lengths keep.
        let block = 2 * crate::v90::INTERVALS;
        assert_eq!(block, 12);
        for symbols in [RF_SYMBOLS, RF_BAR_SYMBOLS] {
            assert!(symbols.is_multiple_of(block), "{symbols} symbols");
        }
        assert_eq!(RF_BAR_SYMBOLS / block, 2, "Rf-bar is 2 repetitions of the block");
        // The two sign patterns 8.8.4 prints, left-most sign first. Rf-bar's
        // is Rf's inverted, which for a pattern of period four is the same as
        // a shift by two -- so nothing inside either one tells them apart, and
        // it is the run of four equal signs at the join that marks the
        // reversal.
        let rf: [i8; 12] = [1, 1, -1, -1, 1, 1, -1, -1, 1, 1, -1, -1];
        let rf_bar: [i8; 12] = [-1, -1, 1, 1, -1, -1, 1, 1, -1, -1, 1, 1];
        for i in 0..block {
            assert_eq!(rf_bar[i], -rf[i], "symbol {i} is inverted");
            assert_eq!(rf[(i + RF_SIGN_PERIOD) % block], rf[i], "symbol {i} repeats at four");
            assert_eq!(rf_bar[i], rf[(i + 2) % block], "symbol {i} is a two-symbol shift");
        }
        let longest_run = |signs: &[i8]| {
            signs
                .windows(2)
                .fold((1usize, 1usize), |(best, run), pair| {
                    let run = if pair[0] == pair[1] { run + 1 } else { 1 };
                    (best.max(run), run)
                })
                .0
        };
        assert_eq!(longest_run(&rf), 2, "no more than two alike inside Rf");
        let join: Vec<i8> = rf.iter().chain(&rf_bar).copied().collect();
        assert_eq!(longest_run(&join), 4, "four alike where Rf turns into Rf-bar");
    }
}
