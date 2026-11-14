//! Turning 8000 upstream levels a second into line samples (6.2): a clock
//! slaved to the network through the downstream receiver, and the one-off
//! shifts of half a symbol and then epsilon that S-bar-u carries (8.6.3, with
//! 9.5.2.1.7-9.5.2.1.8).
//!
//! Everything upstream of here thinks in symbols. [`crate::v92::up_source`]
//! hands out one level a symbol, in units of LU, and knows nothing about the
//! sound card. What this module does is put those levels on the line at the
//! instants the far end's A/D is going to read them, which is the whole
//! difficulty of PCM upstream and the only part of the transmitter the
//! Recommendation leaves to the implementer: "The upstream symbol rate shall
//! be 8000 symbol/s derived from the digital network" (6.2), and not one word
//! about how an analogue modem with its own crystal is to derive it.
//!
//! Three things follow from that sentence.
//!
//! **The rate is the network's, and the only sight of the network's clock is
//! the downstream.** So the transmitter free-runs at its own nominal 8000
//! symbol/s until the downstream receiver has trained -- Ru, TRN1u and Ja all
//! go out before the digital modem has sent anything at all -- and then
//! follows [`SymbolClock::period`], which is the smoothed rate and not the
//! timing loop's jittering sampling instants. The changeover belongs in the
//! silence after Ja (9.5.2.1.3), where a step in the upstream costs nothing,
//! and it is a change of rate only: the symbol already scheduled keeps its
//! time, so there is no phase step to cost anything in the first place.
//!
//! **The phase is not ours to choose, and not ours to keep either.** The
//! digital modem cannot move its A/D: "The digital modem is not capable of
//! changing the sampling phase of the central office A/D. Hence, it shall use
//! signal Jp to indicate its desire to the analogue modem to adjust its
//! transmitter phase from [0, 1) symbol or [0, T) seconds" (8.6.3). So the
//! transmitter starts wherever it likes, the digital modem measures the phase
//! off Su, and the answer comes back in Jp bits 18:33 as a sixteen-bit
//! fraction of a symbol. [`PcmTransmitter::delay`] is where that is applied,
//! twice and only twice: half a symbol at the S-bar-u of 24.5T (9.5.2.1.7),
//! and then epsilon at the S-bar-u of "24T plus any fractional amount from 0
//! to 1 symbol as specified in Jp" (9.5.2.1.8). Both are read as a **lasting
//! delay of every later upstream symbol**, not as extra symbols in the
//! pattern: 8.5.6 requires Su and S-bar-u to be "an integer multiple of 12
//! symbols in length", so the half and the fraction cannot be pattern at all.
//! A third call is refused, because nothing after Jp re-steps an upstream --
//! the next thing that may move it is a retrain, which builds a new
//! transmitter.
//!
//! **What a line sample is worth depends on how it is made.** Between two
//! symbol instants there is no symbol, and a project that runs its calls over
//! a softphone has two quite different answers about what belongs there. On a
//! real telephone loop the far codec integrates a waveform, so the honest
//! thing is [`Mode::Interpolated`]: a windowed-sinc reconstruction whose
//! cutoff is the symbol rate's own Nyquist ([`CUTOFF`]), evaluated at whatever
//! fraction of a symbol each line sample falls at, which gives the arbitrary
//! fractional delay epsilon needs for free. Through a softphone at 16 kHz
//! there may be no analogue loop at all: our samples are decimated one in two
//! and handed straight to a G.711 encoder, and anything we put between them is
//! thrown away, so [`Mode::Straight`] puts the level itself on the codec's
//! phase and a band-limited midpoint between. That mode is exact only at
//! offsets which are multiples of 0.5 T -- at 16 kHz, with no clock skew and
//! with every shift a multiple of half a symbol -- and anywhere else it takes
//! the nearer of its two phases. Which mode a live line wants is an open
//! question that only a capture can settle; both are built, and the network
//! model carries both paths.
//!
//! The two modes share one kernel. [`Mode::Straight`] is [`Mode::Interpolated`]
//! restricted to the two phases where the same windowed sinc is a delta and a
//! midpoint, so a bug in the reconstruction is a bug in both.
//!
//! Levels arrive in units of LU and leave scaled by it. 3.8 sets LU so that
//! "TRN1U is transmitted at the desired data mode transmit power" and names no
//! ceiling; this project has one anyway, because the softphone capture path
//! has a limiter of its own -- see [`PEAK`].

use std::collections::VecDeque;
use std::f64::consts::PI;

use crate::v90::dil;
use crate::v90::pcm::{BAUD, SymbolClock};

// ---------------------------------------------------------------------------
// The reconstruction
// ---------------------------------------------------------------------------

/// The reconstruction's cutoff, in hertz: half the 8000 symbol/s upstream
/// rate.
///
/// Exactly half, not "just under 4 kHz", and the difference matters. A
/// reconstruction filter for a *sample* stream has to be an interpolator: its
/// zeros have to land on the other symbols' instants, or an unshifted line
/// sample is the symbol plus a few per cent of its neighbours and the far A/D
/// never reads back what was sent. Only a sinc whose cutoff is the symbol
/// rate's own Nyquist has its zeros there. The window then rolls the response
/// off from about 3.4 kHz, so what reaches the line is "just under 4 kHz" in
/// the only sense that matters, and 6.5's note applies: V.92 specifies no
/// transmit mask for PCM upstream, because the prefilter the digital modem
/// downloads equalizes whatever is left.
pub const CUTOFF: f64 = BAUD / 2.0;

/// Symbols the reconstruction reaches either side of a line sample: 41 taps.
///
/// Wide enough that the window's own roll-off, and not the truncation, sets
/// the transition; narrow enough that a ten-second call costs tens of
/// milliseconds. It is also [`PcmTransmitter::lookahead`]: a symbol is asked
/// for this far before it reaches the line, exactly as `v34::qam::Transmitter`
/// asks a span ahead of its pulse.
pub const REACH: usize = 20;

/// Fractions of a symbol the kernel is tabulated at.
///
/// The table is not read by rounding to one of these, as `pcm::Receiver` reads
/// its own: it is interpolated between two neighbours, because epsilon is a
/// sixteenth-thousandth of a symbol (Table 22, bits 18:33) and a grid of 1/512
/// would quantise it to about a thousandth. The kernel is smooth enough that
/// straightening it between two points a five-hundredth of a symbol apart
/// costs about 5e-7.
const PHASES: usize = 512;

/// Symbols the reconstruction keeps: the taps it reaches, and one either side
/// so a pull never has to look at one that has gone.
const LEVELS: usize = 2 * REACH + 2;

/// Symbol instants kept for [`PcmTransmitter::symbol_at`], in symbols: two
/// seconds.
///
/// The longest thing V.92 measures at the line terminals is the CP repeat
/// window of 100 ms plus a round-trip delay (9.6.1.2.2), and a round trip on
/// this project's lines is about 1.5 s (memory `voip-line-round-trip`). Older
/// instants are extrapolated back at the current rate rather than remembered.
const KEPT: usize = 2 * BAUD as usize;

// ---------------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------------

/// The loudest line sample this transmitter means to make, as a fraction of
/// full scale: the same ceiling the DIL uses downstream, [`dil::LOUDEST`].
///
/// V.92 names no ceiling. 3.8 sets LU from "the desired data mode transmit
/// power" and leaves it there, and 6.5 specifies no transmit mask at all. The
/// ceiling is this project's, and it comes from a live call: through a
/// softphone everything to about a third of full scale arrived exactly, and
/// everything much above it was held down by something with a gain control
/// which then read low for a third of a second (memory `v90-live-test-pending`).
/// An upstream that crossed it would have its TRN1u squashed, and the digital
/// modem would measure the gain G and design the precoder against a level the
/// data never has.
pub const PEAK: f64 = dil::LOUDEST;

/// How far above LU a precoded upstream's peaks are allowed to reach.
///
/// Reading: 3, a crest factor of 9.5 dB. 3.8 makes LU the level of TRN1u and
/// 8.8.3 makes the mean square of G x v(n) one, so data leaves at an RMS of LU
/// and its peaks are whatever the precoder's feedback makes them -- x(n) is
/// never saturated (6.4.2), so they are not bounded by the constellation. The
/// loudest thing the Recommendation does name is Su, "sqrt(3/2) x LU" (8.5.6),
/// which is 1.22 LU, so three leaves better than half the room for the
/// precoder. The alternative reading is 2, which is all an unprecoded +/-LU
/// sequence and Su between them need, and which would let LU be half of
/// [`PEAK`]; it is declined because it puts nothing aside for the one part of
/// the chain whose peaks nobody can predict.
pub const CREST: f64 = 3.0;

/// The loudest LU this transmitter will take: [`PEAK`] over [`CREST`].
pub const LU_LOUDEST: f64 = PEAK / CREST;

// ---------------------------------------------------------------------------
// The one-off shifts
// ---------------------------------------------------------------------------

/// The fraction of a symbol the first S-bar-u carries, from the 24.5T of
/// 9.5.2.1.7: "the analogue modem shall transmit signal S-bar-u for length of
/// 24.5T followed by signal Su".
pub const SU_BAR_HALF: f64 = 0.5;

/// One-off shifts an upstream may take: the half symbol of 9.5.2.1.7 and then
/// the epsilon of 9.5.2.1.8, and no more.
pub const SHIFTS: u8 = 2;

// ---------------------------------------------------------------------------
// The transmitter
// ---------------------------------------------------------------------------

/// How a line sample between two symbol instants is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A windowed-sinc reconstruction at [`CUTOFF`], read at whatever fraction
    /// of a symbol the line sample falls at: what a real telephone loop wants,
    /// and what an arbitrary fractional delay needs.
    Interpolated,
    /// The level itself on the codec's phase, and a band-limited midpoint
    /// between: what a softphone path that hands our 16 kHz samples straight
    /// to a decimator and a G.711 encoder wants.
    ///
    /// Exact only where every offset is a multiple of half a symbol -- at
    /// fs = 16 000 with the two clocks together and every shift a multiple of
    /// 0.5 T. Anywhere else it takes the nearer of its two phases, so a rate
    /// it is following walks across them in half-sample steps rather than
    /// sliding, which is what a path that forwards samples verbatim really
    /// does to a clock offset.
    Straight,
}

/// Upstream levels in, line samples out (6.2).
///
/// Levels are asked for as they are needed, one at a time, so whatever decides
/// them counts them exactly -- the same pull shape as `v34::qam::Transmitter`,
/// and for the same reason: every V.92 upstream segment is a whole number of
/// symbols and several of them are a whole number of twelve-symbol frames.
#[derive(Debug, Clone)]
pub struct PcmTransmitter {
    mode: Mode,
    /// Line samples one symbol takes with neither clock off: `fs / 8000`.
    nominal: f64,
    /// Line samples one symbol takes now.
    period: f64,
    /// Whether the downstream receiver's rate has been taken up.
    slaved: bool,
    /// LU (3.8): what a level of 1 leaves at.
    lu: f64,
    /// The kernel at `k / PHASES` symbols. It is even, so only the right half
    /// is kept.
    kernel: Vec<f64>,
    /// Line samples given out: the time of the next one.
    taken: u64,
    /// Symbols asked for.
    placed: u64,
    /// Where the next symbol asked for falls, in line samples.
    next_at: f64,
    /// The symbols the reconstruction reaches, oldest first, scaled by LU.
    levels: VecDeque<f64>,
    /// Where each of the last [`KEPT`] symbols fell, oldest first.
    times: VecDeque<f64>,
    /// One-off shifts taken so far.
    shifts: u8,
    /// The loudest sample so far, for measuring against [`PEAK`].
    peak: f64,
}

impl PcmTransmitter {
    /// A transmitter putting 8000 symbols a second on a line running at `fs`.
    ///
    /// It free-runs at `fs / 8000` line samples a symbol until it is given a
    /// trained [`SymbolClock`], and puts its first symbol on its first line
    /// sample. [`Mode::Straight`] means what it says only at fs = 16 000.
    pub fn new(fs: f64, mode: Mode) -> Self {
        let nominal = fs / BAUD;
        Self {
            mode,
            nominal,
            period: nominal,
            slaved: false,
            lu: LU_LOUDEST,
            kernel: kernel_table(),
            taken: 0,
            placed: 0,
            next_at: 0.0,
            levels: VecDeque::with_capacity(LEVELS),
            times: VecDeque::with_capacity(KEPT),
            shifts: 0,
            peak: 0.0,
        }
    }

    /// LU, as a fraction of full scale: what a level of 1 leaves at (3.8).
    ///
    /// LU is a magnitude -- TRN1u is "a sequence of +/- LU values" (8.5.7) --
    /// so a negative one is its size. An LU louder than [`LU_LOUDEST`] is
    /// taken down to it rather than refused, because the ceiling is this
    /// project's and not the Recommendation's: see [`PEAK`]. [`Self::level`]
    /// says what was taken.
    pub fn with_level(mut self, lu: f64) -> Self {
        self.lu = lu.abs().min(LU_LOUDEST);
        self
    }

    /// LU as it stands.
    pub fn level(&self) -> f64 {
        self.lu
    }

    /// The loudest line sample made so far, for measuring against [`PEAK`].
    pub fn peak(&self) -> f64 {
        self.peak
    }

    /// How a line sample between two symbols is made.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Symbols asked for that have not reached the line yet: the
    /// reconstruction reaches this far ahead of the sample going out, so a
    /// segment cut short is cut this many symbols before the line shows it.
    pub fn lookahead() -> usize {
        REACH
    }

    /// Symbols asked for so far.
    pub fn symbols(&self) -> u64 {
        self.placed
    }

    /// Line samples one symbol takes now.
    pub fn period(&self) -> f64 {
        self.period
    }

    /// Whether the network's rate has been taken up from a trained downstream
    /// receiver, or the transmitter is still free-running (6.2).
    pub fn slaved(&self) -> bool {
        self.slaved
    }

    /// One-off shifts taken: at most [`SHIFTS`].
    pub fn shifts(&self) -> u8 {
        self.shifts
    }

    /// Where symbol `n` reaches the line, in line samples since the first.
    ///
    /// Exact for the last [`KEPT`] symbols, which is what the 40 +/- 1 ms
    /// reversal turnarounds of short Phase 2 and the 100 ms plus round-trip CP
    /// repeat window are measured over; a symbol still to come, or one older
    /// than that, is carried at the rate in force now.
    pub fn symbol_at(&self, n: u64) -> f64 {
        if n >= self.placed {
            return self.next_at + (n - self.placed) as f64 * self.period;
        }
        let first = self.placed - self.times.len() as u64;
        if n >= first {
            return self.times[(n - first) as usize];
        }
        self.times.front().copied().unwrap_or(self.next_at) - (first - n) as f64 * self.period
    }

    /// A one-off shift of `fraction_of_t` symbols, applied to every symbol
    /// from the next one asked for onwards.
    ///
    /// Used twice and only twice: [`SU_BAR_HALF`] at the S-bar-u of 24.5T
    /// (9.5.2.1.7), then epsilon at the S-bar-u of 24T "plus any fractional
    /// amount from 0 to 1 symbol as specified in Jp" (9.5.2.1.8). Neither adds
    /// a symbol -- 8.5.6 makes S-bar-u a whole number of twelve, so the extra
    /// half and the extra fraction can only be time -- and after the second
    /// the upstream is never re-stepped, so a third call is refused.
    ///
    /// The shift lands where the *source* is, which is [`Self::lookahead`]
    /// symbols ahead of the line, and that is the point: the caller asks for
    /// it as it starts sending the S-bar-u, not as the S-bar-u comes out.
    pub fn delay(&mut self, fraction_of_t: f64) -> Result<(), &'static str> {
        if self.shifts >= SHIFTS {
            return Err("the upstream has already taken both of s-bar-u's shifts");
        }
        if !(0.0..1.0).contains(&fraction_of_t) {
            return Err("a one-off shift is from 0 to 1 symbol");
        }
        self.next_at += fraction_of_t * self.period;
        self.shifts += 1;
        Ok(())
    }

    /// The next line sample, asking `next` for levels as the reconstruction
    /// needs them.
    ///
    /// `clock` is read for its rate alone. Its sampling instants move on every
    /// symbol, and an upstream that followed them would carry every one of
    /// those steps to the far A/D, where nothing takes them out again; the
    /// phase this transmitter is at is the digital modem's business to measure
    /// off Su and to correct in Jp (8.6.3).
    pub fn next_sample(&mut self, clock: SymbolClock, mut next: impl FnMut() -> f64) -> f64 {
        self.follow(clock);
        let t = self.taken as f64;
        let ahead = t + REACH as f64 * self.period;
        while self.next_at <= ahead {
            let level = next() * self.lu;
            self.place(level);
        }
        let sample = self.at(t);
        self.taken += 1;
        self.peak = self.peak.max(sample.abs());
        sample
    }

    /// The network's rate, once the downstream receiver has one to give (6.2).
    ///
    /// Nothing is re-anchored: the symbol already scheduled keeps the time it
    /// was given, and only the ones after it are spaced at the new rate, so
    /// taking the clock up is not a phase step. Once taken, a rate is kept
    /// through a receiver that has been hunted again for a retrain -- the
    /// network's clock did not change when our receiver lost the line, and
    /// dropping back to nominal would put a hundred parts per million on the
    /// line for no reason.
    fn follow(&mut self, clock: SymbolClock) {
        if clock.trained && clock.period > 0.0 && clock.nominal > 0.0 {
            self.period = clock.period * self.nominal / clock.nominal;
            self.slaved = true;
        } else if !self.slaved {
            self.period = self.nominal;
        }
    }

    /// One symbol, at `next_at`, and the next one a period later.
    fn place(&mut self, level: f64) {
        self.levels.push_back(level);
        if self.levels.len() > LEVELS {
            self.levels.pop_front();
        }
        self.times.push_back(self.next_at);
        if self.times.len() > KEPT {
            self.times.pop_front();
        }
        self.next_at += self.period;
        self.placed += 1;
    }

    /// The line at `t` line samples: every symbol the kernel reaches, each at
    /// its own instant, so that a shift applied part-way through shows up as
    /// the delay it is and not as a jump.
    fn at(&self, t: f64) -> f64 {
        // Both rings are pushed together and [`KEPT`] is the longer, so the
        // instants of the levels still here are always still here too.
        let offset = self.times.len() - self.levels.len();
        let mut sum = 0.0;
        for (j, &level) in self.levels.iter().enumerate() {
            // Silence is most of a start-up, and a tap of zero is worth
            // skipping when the answer is exactly zero either way.
            if level == 0.0 {
                continue;
            }
            sum += level * self.tap((t - self.times[offset + j]) / self.period);
        }
        sum
    }

    /// The kernel `tau` symbols from a symbol's instant.
    fn tap(&self, tau: f64) -> f64 {
        let tau = match self.mode {
            Mode::Interpolated => tau,
            // The two phases this mode has: the symbol itself, and the
            // band-limited midpoint between two of them.
            Mode::Straight => (tau * 2.0).round() / 2.0,
        };
        let x = tau.abs() * PHASES as f64;
        let last = (self.kernel.len() - 1) as f64;
        if !x.is_finite() || x >= last {
            return 0.0;
        }
        let k = x as usize;
        let f = x - k as f64;
        self.kernel[k] * (1.0 - f) + self.kernel[k + 1] * f
    }
}

/// The reconstruction, at `k / PHASES` symbols from the centre.
///
/// A sinc at [`CUTOFF`] -- so its zeros are the other symbols' instants -- in a
/// Blackman window a symbol past the last symbol it carries, which is the same
/// shape `v90::network` draws its codec's filters with.
fn kernel_table() -> Vec<f64> {
    let edge = REACH as f64 + 1.0;
    (0..=REACH * PHASES)
        .map(|k| {
            let t = k as f64 / PHASES as f64;
            let x = 2.0 * CUTOFF / BAUD * t;
            let sinc = if x < 1e-12 { 1.0 } else { (PI * x).sin() / (PI * x) };
            let window = 0.42 + 0.5 * (PI * t / edge).cos() + 0.08 * (2.0 * PI * t / edge).cos();
            sinc * window
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::network::Network;
    use crate::v90::ucode::Law;
    use std::f64::consts::TAU;

    const FS: f64 = 16_000.0;

    /// The sound card `ppm` fast, as `Network::with_clock` means it: one far
    /// symbol then takes this many of our line samples.
    fn true_period(ppm: f64) -> f64 {
        FS / ((1.0 - ppm * 1e-6) * BAUD)
    }

    /// A clock as `pcm::Receiver::symbol_clock` would report it for a network
    /// `ppm` fast, once training has stood.
    ///
    /// The receiver's own accuracy is V92-06's question and is settled by its
    /// tests; what is under test here is the transmitter that follows it.
    fn clock(ppm: f64) -> SymbolClock {
        SymbolClock { period: true_period(ppm), nominal: FS / BAUD, at: 0.0, index: 0, trained: true }
    }

    /// A clock with nothing to say yet: what a receiver reports until it has
    /// trained, and what makes an upstream free-run (6.2).
    fn untrained() -> SymbolClock {
        SymbolClock { period: FS / BAUD, nominal: FS / BAUD, at: 0.0, index: 0, trained: false }
    }

    /// The transmitter's line samples, without a network: `ticks` symbols'
    /// worth at the nominal rate.
    fn line(tx: &mut PcmTransmitter, clock: SymbolClock, samples: usize, mut next: impl FnMut() -> f64) -> Vec<f64> {
        (0..samples).map(|_| tx.next_sample(clock, &mut next)).collect()
    }

    /// The transmitter carried to the far codec's A/D: one network sample per
    /// tick, with as many line samples in between as the analogue side's own
    /// clock makes.
    fn through(
        net: &mut Network,
        tx: &mut PcmTransmitter,
        clock: SymbolClock,
        ticks: usize,
        mut next: impl FnMut() -> f64,
    ) -> Vec<f64> {
        let mut out = Vec::with_capacity(ticks);
        let mut up = Vec::new();
        for _ in 0..ticks {
            let made = net.down(0.0).len();
            up.clear();
            for _ in 0..made {
                up.push(tx.next_sample(clock, &mut next));
            }
            out.push(net.up(&up));
        }
        out
    }

    /// A network that carries a waveform and does nothing else to it: no
    /// quantising, no pad, the analogue level taken at face value, and the
    /// codec's anti-alias filter opened to the line's own Nyquist, where it
    /// is transparent.
    ///
    /// The last of those is the point. The transmitter's own reconstruction is
    /// already the band-limiting filter, and it is a half-band one: what it
    /// puts above 4 kHz is the mirror image of what it puts below, so an A/D
    /// that samples on the symbol instants and folds the image back reads the
    /// level exactly. A *second* filter at 4 kHz in front of that A/D breaks
    /// it, because two half-band filters in cascade are not one -- at 4 kHz
    /// each passes half and the pair passes a quarter twice over, which is
    /// 6 dB down on a band edge that a random level sequence fills, and it
    /// costs about 22 dB of read-back. That filter is a real thing on a real
    /// gateway, but it is a channel, and equalizing the channel is what the
    /// prefilter the digital modem designs and downloads is for (6.4.2). It
    /// is not the transmitter's to answer for, so it is taken out of the way
    /// where the transmitter is what is being measured.
    fn clear() -> Network {
        Network::new(Law::Mu, FS).unquantised().with_upstream_gain(1.0).with_upstream_cutoff(FS / 2.0)
    }

    /// Four numbers from a counter, in [-1, 1): levels with nothing in common
    /// with the reconstruction that carries them.
    fn random(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    /// How late `x` is against a sine of `f` hertz, in symbols, with `x`
    /// sampled at the network's 8000 a second and `n` counted from its first.
    ///
    /// Leakage-free as long as the window holds a whole number of cycles,
    /// which every caller here arranges, so the answer is the delay and not an
    /// estimate of it. It wraps at one cycle of `f`.
    fn lateness(x: &[f64], from: usize, f: f64) -> f64 {
        let w = TAU * f / BAUD;
        let (mut re, mut im) = (0.0, 0.0);
        for (k, &v) in x.iter().enumerate().skip(from) {
            re += v * (w * k as f64).cos();
            im += v * (w * k as f64).sin();
        }
        (-re).atan2(im) / w
    }

    /// The power of the difference between `a` and `b` against the power of
    /// `b`, in decibels.
    fn error_db(a: &[f64], b: &[f64]) -> f64 {
        let (mut e, mut s) = (0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            e += (x - y) * (x - y);
            s += y * y;
        }
        10.0 * (e / s.max(1e-300)).log10()
    }

    /// 6.2 puts 8000 symbols a second on the line, and a sound card at twice
    /// that has a sample of its own exactly where each of them belongs. Both
    /// modes therefore have to give the level back untouched on the even
    /// samples: the reconstruction's zeros are the other symbols' instants
    /// ([`CUTOFF`]), so nothing of the neighbours reaches them.
    ///
    /// The odd samples are a band-limited midpoint, which for a level that is
    /// not changing is that level again -- and which for a level that is
    /// changing is nothing so simple, because a band-limited interpolation
    /// rings.
    #[test]
    fn at_twice_the_rate_every_other_sample_is_the_level() {
        let levels = [1.0, -1.0, 0.5, 0.5, -0.25, 1.0, 0.0, -0.75, 0.25, 1.0, -1.0, 0.5];
        for mode in [Mode::Interpolated, Mode::Straight] {
            let mut tx = PcmTransmitter::new(FS, mode).with_level(0.1);
            let mut k = 0;
            let out = line(&mut tx, untrained(), 2 * levels.len(), || {
                let v = levels[k % levels.len()];
                k += 1;
                v
            });
            for (n, &want) in levels.iter().enumerate() {
                let got = out[2 * n];
                assert!((got - want * 0.1).abs() < 1e-12, "{mode:?}: symbol {n} came out {got} against {}", want * 0.1);
            }
            // A steady level: then the midpoints are that level as well, and
            // every sample of the line is it.
            let mut steady = PcmTransmitter::new(FS, mode).with_level(0.1);
            let out = line(&mut steady, untrained(), 200, || 1.0);
            for (n, &got) in out.iter().enumerate().skip(2 * REACH) {
                assert!((got - 0.1).abs() < 1e-4, "{mode:?}: sample {n} of a steady level came out {got}");
            }
        }
    }

    /// The whole of PCM upstream in one sentence: the codeword the far A/D
    /// makes has to be the one the analogue modem meant. With the route
    /// carrying the waveform and nothing else -- no quantiser, no pad, unit
    /// gain, a transparent A/D filter -- what comes out of the A/D is the
    /// level that went in, and not approximately: measured 305 dB down, which
    /// is arithmetic and not filtering.
    #[test]
    fn through_the_network_the_codec_reads_back_the_levels_sent() {
        let mut net = clear();
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let lu = tx.level();
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut sent = Vec::new();
        let heard = through(&mut net, &mut tx, untrained(), 4000, || {
            let v = random(&mut seed);
            sent.push(v * lu);
            v
        });
        // The A/D reads a fixed number of codewords behind the newest line
        // sample, so the two streams line up at one lag and no other. Find it
        // rather than assert it: the lag is the network's business.
        let (mut best, mut at) = (0.0f64, 0usize);
        for lag in 60..120 {
            let db = error_db(&heard[lag..3500], &sent[..3500 - lag]);
            if at == 0 || db < best {
                best = db;
                at = lag;
            }
        }
        println!("the codec read the levels back {best:.1} dB down, {at} codewords behind");
        assert!(best < -40.0, "the levels came back {best:.1} dB down at a lag of {at}");
    }

    /// 9.5.2.1.7: the first S-bar-u is 24.5T, and the half symbol is a lasting
    /// delay of everything after it. Measured where it has to be right: at the
    /// far A/D, whose sampling phase the digital modem cannot move (8.6.3).
    ///
    /// With the A/D reading half a symbol after our instants it gets the
    /// midpoints, which are nothing like the levels; with the transmitter
    /// delayed by the same half symbol it is back on the instants and gets the
    /// levels again.
    #[test]
    fn a_half_symbol_delay_moves_the_codec_samples_by_half_a_symbol() {
        let run = |shift: Option<f64>| {
            let mut net = clear().with_upstream_phase(SU_BAR_HALF);
            let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
            if let Some(f) = shift {
                tx.delay(f).expect("the first shift is always taken");
            }
            let lu = tx.level();
            let mut seed = 0x2545_f491_4f6c_dd1du64;
            let mut sent = Vec::new();
            let heard = through(&mut net, &mut tx, untrained(), 3000, || {
                let v = random(&mut seed);
                sent.push(v * lu);
                v
            });
            let mut best = 0.0f64;
            for lag in 60..120 {
                let db = error_db(&heard[lag..2500], &sent[..2500 - lag]);
                if best == 0.0 || db < best {
                    best = db;
                }
            }
            best
        };
        let off = run(None);
        let on = run(Some(SU_BAR_HALF));
        println!("half a symbol out the codec read {off:.1} dB down; shifted to meet it, {on:.1} dB");
        assert!(off > -10.0, "an A/D half a symbol out still read the levels, {off:.1} dB down");
        assert!(on < -40.0, "the half symbol shift left the codec {on:.1} dB out");
    }

    /// Table 22 bits 18:33: "Fractional amount that signal S-bar-u
    /// corresponding to signal Jp to Jp' transition needs to be extended.
    /// 16-bit unsigned integer covering the range [0, 1) symbol or [0, T)
    /// seconds" -- so the transmitter has to resolve a sixty-five thousandth
    /// of a symbol, and the code is read as code / 65536 (the reading
    /// `v92::epsilon` will own).
    ///
    /// Measured both ways round: the shift the transmitter makes, read off the
    /// far A/D, and the one `Network::with_upstream_phase` makes, which is the
    /// A/D instant the digital modem measured in the first place. They have to
    /// cancel.
    #[test]
    fn epsilon_resolves_to_a_sixty_five_thousandth_of_a_symbol() {
        /// A tone at a whole number of cycles in the measuring window, so the
        /// phase that comes back is the delay and not an estimate of it.
        const TONE: f64 = 1000.0;
        /// Symbols the measurement is allowed to be out by: half of one code
        /// of Jp bits 18:33, so that a transmitter which rounded epsilon to a
        /// coarser grid than the field's own would fail here. Measured at most
        /// 1.3e-6, which is a twelfth of a code.
        const WITHIN: f64 = 0.5 / 65_536.0;
        let measure = |shift: f64, phase: f64| {
            let mut net = clear().with_upstream_phase(phase);
            let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
            if shift > 0.0 {
                tx.delay(shift).expect("the first shift is always taken");
            }
            let mut m = 0u64;
            let heard = through(&mut net, &mut tx, untrained(), 4200, || {
                let v = (TAU * TONE * m as f64 / BAUD).sin();
                m += 1;
                v
            });
            // From 200 codewords in, the filters have settled; the window is
            // 4000 codewords, 500 whole cycles of the tone.
            lateness(&heard[..4200], 200, TONE)
        };
        let base = measure(0.0, 0.0);
        for code in [0x0000u32, 0x0001, 0x0100, 0x1000, 0x2000, 0x4000, 0x8000, 0xffff] {
            let epsilon = code as f64 / 65_536.0;
            let late = measure(epsilon, 0.0) - base;
            let back = measure(epsilon, epsilon) - base;
            println!("epsilon {code:#06x} = {epsilon:.6} T: the waveform went {late:+.6} T late, and {back:+.6} T with the A/D moved to meet it");
            assert!((late - epsilon).abs() < WITHIN, "epsilon {code:#06x} moved the waveform {late:.6} T, not {epsilon:.6} T");
            assert!(back.abs() < WITHIN, "epsilon {code:#06x} and the same A/D phase left {back:.6} T between them");
        }
        // The plan's own figure: a quarter of a symbol, within one per cent.
        let quarter = measure(0.25, 0.0) - base;
        assert!((quarter / 0.25 - 1.0).abs() < 0.01, "0x4000 moved the waveform {quarter:.6} T, not a quarter");
    }

    /// 6.2 again, over the ten seconds a start-up and its Phase 4 take: the
    /// upstream symbol rate is the network's, so a sound card 120 parts per
    /// million fast has to be taken out of it. Uncorrected that is 1.2 ms in
    /// ten seconds, nearly ten whole symbols at the far A/D, and V.92 has no
    /// way to re-align an upstream short of a retrain (8.6.3).
    ///
    /// The clock here is a perfect one, because what the receiver's is worth
    /// is V92-06's question: `an_upstream_timed_by_the_symbol_clock_keeps_its_
    /// phase_for_ten_seconds` measures a real one and puts it inside 0.05 T
    /// over the same ten seconds.
    #[test]
    fn the_transmitter_follows_a_network_120_ppm_fast() {
        /// Symbols the far A/D's reading of our instants may walk in ten
        /// seconds, with a clock that carries the rate exactly: a
        /// thousandth, which is the measurement's own floor and nothing like
        /// what the upstream could stand anyway (measured 3.8e-7). What a real
        /// receiver's clock leaves is V92-06's 0.05 T.
        const WITHIN: f64 = 1e-3;
        const PPM: f64 = 120.0;
        const TONE: f64 = 1000.0;
        let walk = |clock: SymbolClock| {
            let mut net = clear().with_clock(PPM);
            let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
            let mut m = 0u64;
            let heard = through(&mut net, &mut tx, clock, 10 * BAUD as usize, || {
                let v = (TAU * TONE * m as f64 / BAUD).sin();
                m += 1;
                v
            });
            // A reading a second, unwrapped: a cycle of the tone is eight
            // symbols and the walk is under one a second either way, so
            // nothing is ambiguous.
            let cycle = BAUD / TONE;
            let mut walked: Vec<f64> = Vec::new();
            for second in 1..10 {
                let at = second * BAUD as usize;
                let mut late = lateness(&heard[..at + BAUD as usize], at, TONE);
                if let Some(&before) = walked.last() {
                    late -= ((late - before) / cycle).round() * cycle;
                }
                walked.push(late);
            }
            let first = walked[0];
            walked.iter().map(|w| w - first).fold(0.0f64, |a, b| a.max(b.abs()))
        };
        let slaved = walk(clock(PPM));
        let free = walk(untrained());
        println!("slaved to the network the instants walked {slaved:.3e} T in ten seconds; free-running, {free:.2} T");
        assert!(slaved < WITHIN, "the instants walked {slaved:.3e} T in ten seconds");
        assert!(free > 5.0, "free-running only walked {free:.2} T, so the test saw nothing");
    }

    /// 9.5.2.1.8 is the last word an upstream's timing ever gets: the digital
    /// modem measures the phase on Su, asks for epsilon in Jp, and after that
    /// nothing in V.92 moves a transmitter again short of a retrain. So the
    /// half symbol and epsilon are taken and a third shift is refused, with
    /// nothing moved by the refusal.
    #[test]
    fn the_transmitter_is_never_re_stepped_after_epsilon() {
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut ones = || 1.0;
        line(&mut tx, untrained(), 400, &mut ones);
        let before = tx.symbol_at(tx.symbols());
        assert_eq!(tx.shifts(), 0);
        assert_eq!(tx.delay(SU_BAR_HALF), Ok(()));
        assert!((tx.symbol_at(tx.symbols()) - before - 1.0).abs() < 1e-12, "half a symbol is one line sample at 16 kHz");
        let epsilon = f64::from(0x4000) / 65_536.0;
        assert_eq!(tx.delay(epsilon), Ok(()));
        assert_eq!(tx.shifts(), SHIFTS);
        let after = tx.symbol_at(tx.symbols());
        assert_eq!(tx.delay(0.25), Err("the upstream has already taken both of s-bar-u's shifts"));
        assert_eq!(tx.delay(0.0), Err("the upstream has already taken both of s-bar-u's shifts"));
        assert_eq!(tx.shifts(), SHIFTS);
        assert_eq!(tx.symbol_at(tx.symbols()), after, "a refused shift moved the upstream anyway");
        // And the shifts themselves have to be a fraction of one symbol:
        // "any fractional amount from 0 to 1 symbol" (9.5.2.1.8).
        let mut fresh = PcmTransmitter::new(FS, Mode::Interpolated);
        assert_eq!(fresh.delay(1.0), Err("a one-off shift is from 0 to 1 symbol"));
        assert_eq!(fresh.delay(-0.25), Err("a one-off shift is from 0 to 1 symbol"));
        assert_eq!(fresh.shifts(), 0);
    }

    /// 8.5.6 makes S-bar-u "an integer multiple of 12 symbols in length", so
    /// the 24.5T of 9.5.2.1.7 cannot be 24 symbols and a half one: the half is
    /// a delay of every later symbol and no symbol is added. The symbols
    /// asked for either side of a shift are therefore the same symbols, and
    /// only their instants move.
    #[test]
    fn a_shift_delays_the_symbols_rather_than_adding_one() {
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut ones = || 1.0;
        line(&mut tx, untrained(), 200, &mut ones);
        let n = tx.symbols();
        let was = tx.symbol_at(n - 1);
        tx.delay(0.5).expect("the first shift is always taken");
        assert_eq!(tx.symbols(), n, "a shift asked for a symbol");
        assert_eq!(tx.symbol_at(n - 1), was, "a shift moved a symbol that had already gone");
        line(&mut tx, untrained(), 200, &mut ones);
        // Every symbol after the shift is one line sample later than the
        // twelve-symbol grid it would have been on.
        for k in 0..12 {
            let want = was + (k + 1) as f64 * 2.0 + 1.0;
            assert!((tx.symbol_at(n + k) - want).abs() < 1e-9, "symbol {k} after the shift landed at {}", tx.symbol_at(n + k));
        }
    }

    /// The V.92 procedures are timed at the line terminals, not at the source:
    /// the 40 +/- 1 ms turnarounds of short Phase 2 and the CP repeat window
    /// of 100 ms plus a round trip are about when a symbol went out. So a
    /// symbol that has gone says when it went, exactly, and one still to come
    /// says when it is due.
    #[test]
    fn symbol_at_says_when_a_symbol_reached_the_line() {
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut ones = || 1.0;
        // Four seconds of line: more than the two seconds of instants kept,
        // so the oldest are carried rather than remembered.
        line(&mut tx, untrained(), 4 * FS as usize, &mut ones);
        assert_eq!(tx.symbol_at(0), 0.0, "the first symbol is on the first line sample");
        for n in [1u64, 320, 12_000, 31_000] {
            let want = n as f64 * 2.0;
            assert!((tx.symbol_at(n) - want).abs() < 1e-9, "symbol {n} landed at {}", tx.symbol_at(n));
        }
        // Forty milliseconds is 320 symbols, and a millisecond either side of
        // it is eight: the turnaround 9.4.1.1.2 asks for is measurable here.
        let ms = (tx.symbol_at(320) - tx.symbol_at(0)) / FS * 1000.0;
        assert!((ms - 40.0).abs() < 1e-9, "320 symbols came to {ms} ms");
        // And a symbol that has not been asked for yet is due at the rate in
        // force now.
        let next = tx.symbols();
        assert!((tx.symbol_at(next + 100) - tx.symbol_at(next) - 200.0).abs() < 1e-9);
    }

    /// The reconstruction reaches [`REACH`] symbols ahead of the sample going
    /// out, which is what a segment cut at a boundary has to allow for: the
    /// analogue modem decides on a symbol this long before the far end can
    /// possibly hear it. `v34::qam::Transmitter::lookahead` is the same
    /// promise for V.34 upstream, and `v90::analogue` already subtracts it
    /// from its opening silence.
    #[test]
    fn a_symbol_is_asked_for_a_lookahead_before_it_reaches_the_line() {
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut ones = || 1.0;
        line(&mut tx, untrained(), 1, &mut ones);
        assert_eq!(tx.symbols(), PcmTransmitter::lookahead() as u64 + 1);
        line(&mut tx, untrained(), 199, &mut ones);
        // A hundred symbols have reached the line; the lookahead is ahead of
        // them.
        assert_eq!(tx.symbols(), 100 + PcmTransmitter::lookahead() as u64);
    }

    /// 6.2's rate is the network's, and until the downstream receiver has
    /// trained there is none to be had: the transmitter free-runs at its own
    /// 8000 symbol/s. A receiver hunted again for a retrain says it has not
    /// trained either, but the network's clock did not change when ours lost
    /// the line, so the rate already learnt is kept rather than thrown away.
    #[test]
    fn a_transmitter_free_runs_until_the_receiver_has_trained() {
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut ones = || 1.0;
        // A clock that has not trained is not to be believed even when it
        // carries a rate: `symbol_clock` holds the period at nominal, and a
        // transmitter must not take one from anywhere else.
        let stale = SymbolClock { period: 2.5, nominal: FS / BAUD, at: 0.0, index: 0, trained: false };
        line(&mut tx, stale, 4, &mut ones);
        assert!(!tx.slaved());
        assert_eq!(tx.period(), FS / BAUD);
        line(&mut tx, clock(120.0), 4, &mut ones);
        assert!(tx.slaved());
        assert!((tx.period() - true_period(120.0)).abs() < 1e-12);
        // The retrain: the same receiver is hunted again.
        line(&mut tx, untrained(), 4, &mut ones);
        assert!((tx.period() - true_period(120.0)).abs() < 1e-12, "a hunting receiver stepped the rate back to nominal");
    }

    /// Taking the network's rate up is a change of rate and not a step of
    /// phase: the symbol already scheduled keeps its instant, which is why
    /// 9.5.2.1.3's silence is a good place for it and why nothing after it is
    /// disturbed.
    #[test]
    fn taking_the_clock_up_does_not_step_the_phase() {
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut ones = || 1.0;
        line(&mut tx, untrained(), 200, &mut ones);
        let next = tx.symbols();
        let due = tx.symbol_at(next);
        line(&mut tx, clock(120.0), 2, &mut ones);
        assert_eq!(tx.symbol_at(next), due, "the symbol already scheduled moved");
        // And the ones after it are spaced at the new rate.
        let step = tx.symbol_at(next + 2) - tx.symbol_at(next + 1);
        assert!((step - true_period(120.0)).abs() < 1e-9, "the symbols after the changeover were {step} apart");
    }

    /// The two modes are one kernel: [`Mode::Straight`] is the same
    /// reconstruction read at the only two phases it has. At 16 kHz with the
    /// clocks together every line sample falls on one of them, so the two
    /// modes have to agree sample for sample -- and a bug in the
    /// reconstruction is a bug in both.
    #[test]
    fn straight_is_the_interpolated_reconstruction_at_its_two_exact_phases() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let levels: Vec<f64> = (0..600).map(|_| random(&mut seed)).collect();
        let run = |mode: Mode| {
            let mut tx = PcmTransmitter::new(FS, mode);
            let mut k = 0;
            line(&mut tx, untrained(), 1000, || {
                let v = levels[k % levels.len()];
                k += 1;
                v
            })
        };
        let a = run(Mode::Interpolated);
        let b = run(Mode::Straight);
        for (n, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!((x - y).abs() < 1e-12, "sample {n}: {x} against {y}");
        }
        // Half a symbol is still one of the two phases, so a shift of 0.5 T
        // keeps them together; a shift of anything else does not, and the
        // module doc says so.
        let shifted = |mode: Mode, by: f64| {
            let mut tx = PcmTransmitter::new(FS, mode);
            tx.delay(by).expect("the first shift is always taken");
            let mut k = 0;
            line(&mut tx, untrained(), 1000, || {
                let v = levels[k % levels.len()];
                k += 1;
                v
            })
        };
        let half = shifted(Mode::Interpolated, 0.5);
        let half_straight = shifted(Mode::Straight, 0.5);
        assert!(half.iter().zip(&half_straight).all(|(x, y)| (x - y).abs() < 1e-12));
        let third = shifted(Mode::Interpolated, 1.0 / 3.0);
        let third_straight = shifted(Mode::Straight, 1.0 / 3.0);
        let apart = error_db(&third_straight, &third);
        println!("a third of a symbol out, the straight mode is {apart:.1} dB from the interpolated one");
        assert!(apart > -20.0, "the straight mode interpolated after all");
    }

    /// The reconstruction has to carry a steady level as that level, at every
    /// phase and not only on the symbol instants: a sum of taps that drifts
    /// with the phase would put a ripple at 8 kHz on every upstream, and the
    /// far A/D reads exactly there. Measured 2.3e-5 over a symbol.
    #[test]
    fn the_reconstruction_carries_a_steady_level_at_every_phase() {
        let tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let mut worst = 0.0f64;
        for p in 0..64 {
            let phase = p as f64 / 64.0;
            let sum: f64 = (-(REACH as i64)..=REACH as i64).map(|k| tx.tap(phase - k as f64)).sum();
            worst = worst.max((sum - 1.0).abs());
        }
        println!("the taps came to one within {worst:.3e} over a symbol");
        assert!(worst < 1e-4, "a steady level came out {worst:.3e} off at some phase");
    }

    /// 3.8 sets LU from the wanted data-mode power and names no ceiling. This
    /// project has one -- a softphone capture path with a limiter of its own
    /// (see [`PEAK`]) -- and it is enforced where LU is chosen rather than by
    /// squashing samples, because squashing is what the limiter does and what
    /// the transmitter exists to stay clear of.
    #[test]
    fn lu_is_held_under_the_ceiling_the_capture_path_imposes() {
        assert!((LU_LOUDEST - 0.1).abs() < 1e-12, "the ceiling moved: {LU_LOUDEST}");
        let tx = PcmTransmitter::new(FS, Mode::Interpolated);
        assert_eq!(tx.level(), LU_LOUDEST, "an unasked LU is the loudest one allowed");
        assert_eq!(PcmTransmitter::new(FS, Mode::Interpolated).with_level(1.0).level(), LU_LOUDEST);
        assert_eq!(PcmTransmitter::new(FS, Mode::Interpolated).with_level(-0.04).level(), 0.04);
        assert_eq!(PcmTransmitter::new(FS, Mode::Interpolated).with_level(0.02).level(), 0.02);
        // Su is the loudest thing clause 8 names, sqrt(3/2) LU (8.5.6), and it
        // has to fit under the ceiling with the precoder's room to spare.
        let a = (1.5f64).sqrt();
        let mut tx = PcmTransmitter::new(FS, Mode::Interpolated);
        let su = [a, 0.0, a, -a, 0.0, -a];
        let mut k = 0;
        line(&mut tx, untrained(), 600, || {
            let v = su[k % su.len()];
            k += 1;
            v
        });
        println!("Su peaked at {:.4} of full scale against a ceiling of {PEAK}", tx.peak());
        assert!(tx.peak() < PEAK, "Su peaked at {} of full scale", tx.peak());
        assert!(tx.peak() > a * LU_LOUDEST * 0.99, "Su never reached its own level");
    }
}
