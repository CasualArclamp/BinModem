//! Where the central office's A/D instants fall inside the analogue modem's
//! symbol: measured by the digital modem on **Su**, and sent back in Jp bits
//! 18:33 (8.6.3, 9.5.1.1.6-9.5.1.1.8).
//!
//! The digital modem cannot move the network's sampler, so the far end moves
//! instead: S-bar-u is 24.5T rather than 24T, which slides every later upstream
//! symbol half a symbol, and whatever is left over comes back as epsilon in Jp
//! and the analogue modem applies it by lengthening the next S-bar-u to
//! (24 + epsilon)T. From then on its symbols land on the network's instants,
//! which is the whole point: a two-point stream read half a symbol out is two
//! symbols added together, and that is 19 dB rather than 160 (see
//! [`crate::v92::receiver`]'s table) -- not enough for a CPt's CRC.
//!
//! The Recommendation fixes the signal and the format and says nothing about the
//! estimator. This one is a direct phase measurement on the fundamental of Su
//! in the **line's own samples**, for two reasons.
//!
//! **Su's fundamental is the cleanest thing in Phase 3.** Su has
//! `x[n+3] = -x[n]`, so only odd harmonics of 8000/6 Hz exist in it: a third of
//! its power is at 1333.3 Hz and two thirds at 4000 Hz. The 1333.3 Hz part is
//! inside any line's passband, is not what a two-point slice destroys, and its
//! phase is what a symbol arriving `d` late turns back by `2*pi*d/6`.
//!
//! **And the equaliser's output is the wrong place to measure it.** The
//! equaliser is fitted on TRN1u, which is a two-point stream -- nearly all of
//! its content is at 4000 Hz -- and fitted at 15 dB on a line whose symbols do
//! not land on the sampling instants (that is the reason this measurement
//! exists). A filter fitted to rescue a Nyquist-rate stream at 15 dB has a
//! response at 1333 Hz that is nobody's idea of a pure delay, so the phase
//! measured through it carries an error of the same size as the thing being
//! measured. Measured on *that* an Agere SV92PP's Phase 3 came out at
//! epsilon = 0xaa57 (0.665 of a symbol), which is not consistent with anything
//! the rest of the bench saw. The line's own samples have no such problem.
//!
//! 9.5.1.1.6-8 measure "the Su before and after the first S-bar-u", and this
//! module is used on the run *after* it: that run is the longer one (Su runs
//! from the S-bar-u until Jp is detected, 144T before it), it is the one whose
//! phase already holds the half symbol, and what is left over is exactly the
//! epsilon Jp carries. See [`after_half`] and [`before_half`].

use std::f64::consts::TAU;

use crate::v92::up_signals::{RU_PERIOD, su};

/// A fraction of a symbol, in epsilon's own units: "16-bit unsigned integer
/// covering the range [0, 1) symbol" (Table 22 bits 18:33).
#[must_use]
pub fn to_field(fraction: f64) -> u16 {
    let wrapped = fraction.rem_euclid(1.0);
    let field = (wrapped * 65536.0).round() as u32;
    field.min(u32::from(u16::MAX)) as u16
}

/// How much of a window's power has to sit in Su's fundamental before the
/// window is called a run of Su.
///
/// Su puts a third of its power in the fundamental, and the line's samples of
/// one period of it correlate with the fundamental's own shape at
/// `sqrt(1/3) = 0.577`. A window of anything else -- the two-point stream of
/// TRN1d or TRN1u, a scrambled CPt, noise -- lands near `1/sqrt(samples)` of
/// that. A quarter is between the two by a wide margin either way.
const COHERENT: f64 = 0.25;

/// The phase of Su's fundamental in a run of the line's samples, in radians,
/// against the first sample of `received`.
///
/// Su repeats every six symbols (`RU_PERIOD`), so one bin of a six-point DFT
/// is all of it. Whole periods only: a window that stops mid-period leaks the
/// neighbouring harmonics into the bin, and there is a strong one at 4000 Hz.
///
/// Returns None when the window is too short to have a phase, or when what is
/// in it is not Su (see [`COHERENT`]).
#[must_use]
pub fn phase(received: &[f64]) -> Option<f64> {
    let periods = received.len() / RU_PERIOD;
    if periods < 2 {
        return None;
    }
    let used = periods * RU_PERIOD;
    let (mut re, mut im, mut power) = (0.0, 0.0, 0.0);
    for (n, &x) in received[..used].iter().enumerate() {
        let theta = -TAU * (n % RU_PERIOD) as f64 / RU_PERIOD as f64;
        re += x * theta.cos();
        im += x * theta.sin();
        power += x * x;
    }
    if !power.is_finite() || power <= 0.0 {
        return None;
    }
    // What share of the window's power this one bin holds, normalised so that
    // Su's own fundamental comes out at exactly the figure below.
    let share = re.hypot(im) / (power * used as f64).sqrt();
    if share < COHERENT {
        return None;
    }
    Some(im.atan2(re))
}

/// The ideal Su's own phase, so that [`phase`] can be turned into a distance.
fn ideal_phase() -> f64 {
    let ideal: Vec<f64> = (0..RU_PERIOD).map(|n| su(n as u64, false)).collect();
    phase(&ideal).unwrap_or(0.0)
}

/// How much later the far end's symbols arrive than this end's sampling
/// instants, as a fraction of a symbol in `[0, 1)`.
///
/// `received` is a run of the far end's Su in the line's own samples -- not
/// the equaliser's output, see the module comment. The answer only ever needed
/// to a fraction: a whole symbol of delay is one whole turn of a fundamental
/// that repeats every six, so `0.75` late and `0.25` early are the same
/// reading, and the sign of the reading is what says which.
#[must_use]
pub fn lag(received: &[f64]) -> Option<f64> {
    let measured = phase(received)?;
    // A symbol arriving `d` late puts the fundamental `d/RU_PERIOD` of a turn
    // behind where it would have been, so the distance is minus the phase.
    Some(((ideal_phase() - measured) / TAU * RU_PERIOD as f64).rem_euclid(1.0))
}

/// Which way the far end's transmitter moves when its S-bar-u is *extended*.
///
/// The Recommendation's wording ("the fractional amount that signal S-bar-u ...
/// needs to be extended", Table 22 bits 18:33) reads as a delay: a longer
/// S-bar-u pushes everything after it later. An **Agere SV92PP moves the other
/// way** -- it is the only far end that gets this far on this bench, and the
/// measurement says so twice:
///
/// * its 24.5T S-bar-u moves its grid by half a symbol, and (with the lag
///   being a fraction of a symbol, there is no telling which way from that
///   alone) ...
/// * a Jp of 0x4180 -- the lag itself, 0.256 of a symbol -- puts its second
///   TRN1u on the sampling instants, its CPt's CRC passes, and Phase 3
///   finishes; a Jp of 0xbe80 -- the same lag negated, 0.744 -- leaves the
///   two-point stream where it was, the CPt never checks, and the far end
///   clears down four seconds later.
///
/// So this is a convention, not a measurement, and it is named rather than
/// buried for the same reason [`crate::v92::digital::ZERO_DIL_USES_SCR`] is: a
/// far end that reads Table 22 the other way would need it flipped.
pub const EXTENDS_LATE: bool = false;

/// [`EXTENDS_LATE`] as a signed step of the far end's transmitter, in symbols:
/// what [`crate::v92::receiver::Receiver::expect_step`] wants.
#[must_use]
pub fn step_direction() -> f64 {
    if EXTENDS_LATE { 1.0 } else { -1.0 }
}

/// The epsilon Jp carries, from a lag measured on the Su **after** the first
/// (24.5T) S-bar-u -- 9.5.1.1.7's run, which is the one this module is used on.
///
/// That run is the far end's own grid after the half symbol, so what is left to
/// ask for is whatever it takes to bring the grid back to nought: the lag
/// itself, in the direction [`EXTENDS_LATE`] says.
#[must_use]
pub fn after_half(lag: f64) -> u16 {
    if EXTENDS_LATE {
        to_field(1.0 - lag)
    } else {
        to_field(lag)
    }
}

/// The same, from a lag measured on the Su **before** that S-bar-u
/// (9.5.1.1.6), where the half symbol is still to come.
#[must_use]
pub fn before_half(lag: f64) -> u16 {
    let after = (lag + 0.5).rem_euclid(1.0);
    after_half(after)
}

/// The epsilon Jp carries, from a run of the far end's Su in the line's
/// samples.
#[must_use]
pub fn measure(received: &[f64]) -> Option<u16> {
    Some(after_half(lag(received)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::v92::up_signals::{fractional_delay as delayed, su_level};

    /// A run of Su, `symbols` long, as the far end would put it on a line.
    fn ideal(symbols: usize) -> Vec<f64> {
        (0..symbols as u64).map(|n| su(n, false)).collect()
    }

    /// Su says how far its symbols are from this end's, at every offset, and
    /// with the sign a line has rather than the one a symmetric test would
    /// forgive.
    ///
    /// The sign is the whole of it: epsilon is the *fraction the far end has to
    /// move later*, so a reading that is right in magnitude and inverted leaves
    /// twice the offset rather than none, and twice a half symbol is a two-point
    /// stream whose symbols are gone.
    #[test]
    fn su_says_how_far_its_symbols_are_from_ours() {
        let source = ideal(240);
        for delay in [0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9] {
            let line = delayed(&source, delay);
            // Well inside the run: the interpolator's edges are not the line's.
            let window = &line[48..48 + 96];
            let lag = lag(window).unwrap_or_else(|| panic!("no phase at {delay}"));
            let want = delay.rem_euclid(1.0);
            let error = ((lag - want + 0.5).rem_euclid(1.0) - 0.5).abs();
            assert!(error < 0.03, "delay {delay}: lag {lag:.3} (error {error:.3})");
        }
    }

    /// And epsilon closes the loop: a far end `d` of a symbol out before the
    /// first S-bar-u, half a symbol's worth at that S-bar-u, and epsilon more at
    /// the second, arrives on the sampling instants.
    ///
    /// The step at each S-bar-u is in the direction [`EXTENDS_LATE`] names, and
    /// that is the whole of the convention: the measurement does not care which
    /// way it is, only the answer does.
    #[test]
    fn epsilon_moves_the_far_end_onto_the_sampling_instants() {
        let source = ideal(240);
        let step = if EXTENDS_LATE { 1.0 } else { -1.0 };
        for d in [0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9] {
            let line = delayed(&source, d);
            // What the digital modem sees: the Su before the first S-bar-u, so
            // what it asks for takes that half symbol into account.
            let epsilon = f64::from(before_half(lag(&line[48..48 + 96]).unwrap())) / 65536.0;
            let rest = (d + step * (0.5 + epsilon)).rem_euclid(1.0);
            let error = (rest + 0.5).rem_euclid(1.0) - 0.5;
            assert!(error.abs() < 0.03, "lag {d}: epsilon {epsilon:.3} leaves {error:.3}");
        }
        // And the same read after the half symbol -- the run the digital modem
        // actually measures -- asks for the same thing.
        for d in [0.0, 0.3, 0.7] {
            // The far end's grid once its 24.5T S-bar-u has had its half
            // symbol: that is the run measured after 9.5.1.1.7.
            let after_half_symbol = (d + step * 0.5).rem_euclid(1.0);
            let line = delayed(&source, after_half_symbol);
            let after = f64::from(after_half(lag(&line[48..48 + 96]).unwrap())) / 65536.0;
            let rest = (d + step * 0.5 + step * after).rem_euclid(1.0);
            let error = (rest + 0.5).rem_euclid(1.0) - 0.5;
            assert!(error.abs() < 0.03, "lag {d}: epsilon {after:.3} leaves {error:.3}");
        }
    }

    /// The reading needs no phase reference of its own: where the window starts
    /// is a whole number of symbols, and a window inside the same run of Su says
    /// the same thing.
    #[test]
    fn any_window_of_the_same_run_reads_the_same_lag() {
        let source = ideal(240);
        let line = delayed(&source, 0.3);
        let a = lag(&line[24..24 + 96]).unwrap();
        let b = lag(&line[90..90 + 96]).unwrap();
        assert!(((a - b + 0.5).rem_euclid(1.0) - 0.5).abs() < 0.01, "{a} against {b}");
    }

    /// Anything that is not Su has no fundamental to be measured on. A
    /// two-point stream is what Phase 3 has on the line before Su, and reading
    /// a phase off it is how epsilon came out at 0.665 of a symbol.
    #[test]
    fn a_two_point_stream_is_not_a_run_of_su() {
        let mut state = 0x1234_5678u32;
        let mut bits = Vec::with_capacity(96);
        for _ in 0..96 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            bits.push(if state & 0x8000 != 0 { su_level() } else { -su_level() });
        }
        assert_eq!(lag(&bits), None);
        assert_eq!(measure(&bits), None);
    }

    /// And the field is the fraction it stands for, in the units Table 22 uses.
    #[test]
    fn epsilon_is_the_fraction_it_stands_for() {
        for fraction in [0.0, 0.25, 0.5, 0.75, 0.999] {
            let field = to_field(fraction);
            let back = f64::from(field) / f64::from(u16::MAX);
            assert!((back - fraction).abs() < 1.0 / 1024.0, "{fraction} -> {field:#06x} -> {back}");
        }
        assert_eq!(to_field(0.5), 0x8000);
        assert_eq!(to_field(0.25), 0x4000);
        assert_eq!(to_field(0.0), 0x0000);
    }

    /// The two runs before and after the S-bar-u ask for the same thing, half a
    /// symbol apart -- 9.5.1.1.6-7 measure both, and they have to agree.
    #[test]
    fn the_run_before_the_half_symbol_agrees_with_the_one_after_it() {
        for lag in [0.0, 0.2, 0.5, 0.8] {
            assert_eq!(before_half(lag), after_half((lag + 0.5).rem_euclid(1.0)), "lag {lag}");
        }
    }
}
