//! The V.92 analogue modem's reading of its own DIL: CPt and CPu from what
//! the route did to it, the way V.90's analogue modem chooses its CP.
//!
//! The DIL is V.90's (8.4.1/V.90, 9.5.1.1.11/V.92), so the descriptor asked
//! for is [`v90::dil::design`]'s and the reading is [`v90::dil::Analysis`]'s,
//! and the choice is [`v90::shaping::choose_with_slack`]'s, unshaped. What is
//! V.92's own is only where the symbols come from: the analogue modem keeps
//! the equaliser's output from the symbol after Jp' on
//! ([`super::analogue::Modem::dil`]), on the scale that calls U_INFO one.
//!
//! A robbed bit is what this is for. On a T1 route the octet LSB of one
//! codeword in six is forced, so in one data frame interval every odd Ucode
//! arrives as its even neighbour. With no DIL the analogue modem cannot see
//! that, and the CPt and CPu it sends have to be ones every interval reads:
//! six levels, 28 000 bit/s. The DIL shows which interval it is, and the
//! choice gives that interval a ladder of its own while the other five keep
//! theirs (`v90::dil::quietest`).
//!
//! Where the DIL begins in the data frame does not need finding. Every
//! DIL-segment is a whole number of frames ((H + 1) x 6 symbols, Table
//! 12/V.90), the digital modem cuts the DIL only at a segment boundary
//! (9.5.1.1.12), and Ri, R-bar-i and TRN2d after it are 384T, 24T and whole
//! frames from there: so the DIL's first symbol is in the interval TRN2d's
//! first symbol is, which is the data frame's interval 0. What is not known
//! is where this end's count of the DIL starts to the symbol -- Jp' is
//! twelve zeroes, and a Jp can hold twelve zeroes too -- so the start, and
//! which way up the equaliser hands the levels over, are found from the DIL
//! itself: the pass, round and round, against what came ([`MOST_LATE`]).

use crate::v90::INTERVALS;
use crate::v90::dil::{Analysis, Route};
use crate::v90::sequences::{Cp, Descriptor, Jd, data_rate};
use crate::v90::shaping;
use crate::v90::ucode::{self, Law};

/// What an owner that wants the CPt and the CPu chosen from the DIL tells
/// the analogue modem ([`super::analogue::Settings::read_dil`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// Table 15/V.90's power limit for the digital modem's INFO0d
    /// ([`crate::v90::power_limit`]): the constellations chosen stay inside
    /// it.
    pub power_limit: u32,
    /// The downstream rate somebody pinned, as its drn, or None for the
    /// fastest the route carries with room to spare.
    pub pinned: Option<u8>,
}

/// How far after where this end's count of it starts the DIL may begin, in
/// symbols: a Jp. The count starts when Jp' has been read after a whole Jp
/// (`analogue::Modem`), which on every call here is exactly where the DIL
/// starts; this is room for a line that reads it a Jp late or early.
pub const MOST_LATE: usize = 72;

/// And how far before it: a Jp too.
const MOST_EARLY: usize = 72;

/// Symbols of DIL wanted beyond one pass before it is read, so that a pass
/// is there wherever in [`MOST_LATE`] it began.
pub const MARGIN: usize = MOST_LATE;

/// What the DIL chose: the CPt for Phase 4's sequences, the CPu for data
/// mode, and a line for the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub training: Cp,
    pub data: Cp,
    pub note: String,
}

/// The route out of `values`, the equaliser's output from the symbol after
/// Jp' on (U_INFO's level is `unit`), for a DIL sent as `descriptor` asks.
/// None if fewer symbols than one pass have arrived.
#[must_use]
pub fn route(descriptor: &Descriptor, law: Law, unit: f64, values: &[f64]) -> Option<Route> {
    let sent: Vec<(u8, bool)> = descriptor.symbols().collect();
    let len = sent.len();
    if len == 0 || values.len() < len {
        return None;
    }
    let expected: Vec<f64> = sent
        .iter()
        .map(|&(u, positive)| {
            let level = ucode::level(law, u);
            if positive { level } else { -level }
        })
        .collect();
    // Where the DIL begins and which way up it is. The DIL repeats whole
    // (8.4.1/V.90), so the symbol `m` of what came is the sent one `m -
    // shift` round the pass. The references are U_INFO in every segment
    // alike and say nothing of which segment is which, so only the training
    // symbols, a codeword a segment, are matched.
    let trained: Vec<bool> = (0..len)
        .map(|n| descriptor.training[n_in_segment(descriptor, n) % descriptor.training.len()])
        .collect();
    let fit = |shift: i64| -> f64 {
        values
            .iter()
            .enumerate()
            .filter_map(|(m, &v)| {
                let n = (m as i64 - shift).rem_euclid(len as i64) as usize;
                trained[n].then(|| v * expected[n])
            })
            .sum()
    };
    let (shift, sum) = (-(MOST_EARLY as i64)..=MOST_LATE as i64)
        .map(|shift| (shift, fit(shift)))
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let sign = if sum < 0.0 { -unit } else { unit };
    let mut analysis = Analysis::new();
    for (m, &value) in values.iter().enumerate().skip(shift.max(0) as usize) {
        let n = (m as i64 - shift).rem_euclid(len as i64) as usize;
        let (u, positive) = sent[n];
        analysis.feed(u, positive, n % INTERVALS, sign * value);
    }
    Some(analysis.route())
}

/// Where symbol `n` of the DIL is in its segment.
fn n_in_segment(descriptor: &Descriptor, mut n: usize) -> usize {
    for &u in &descriptor.ucodes {
        let length = descriptor.segment_length(u);
        if n < length {
            return n;
        }
        n -= length;
    }
    n
}

/// CPt and CPu from this call's DIL, for a digital modem whose Jd is `jd`.
/// `cpt` is the CPt the modem was built with, whose fields other than the
/// constellations and the rate the chosen one keeps.
///
/// # Errors
///
/// Why nothing was chosen: too little DIL, or a route that carries none of
/// the rates Jd enables.
pub fn choose(
    descriptor: &Descriptor,
    law: Law,
    unit: f64,
    values: &[f64],
    jd: &Jd,
    reading: Reading,
    cpt: &Cp,
) -> Result<Chosen, &'static str> {
    let route =
        route(descriptor, law, unit, values).ok_or("less than a pass of the DIL arrived")?;
    let enabled = |drn: u8| jd.enables(drn) && reading.pinned.is_none_or(|pinned| drn <= pinned);
    let asked = shaping::choose_with_slack(
        &route,
        law,
        reading.power_limit,
        enabled,
        jd.lookahead,
        None,
    )
    .ok_or("the route carries none of the rates Jd enables")?;
    let mut training = asked.choice.training;
    training.a_law = cpt.a_law;
    training.upstream_rates = cpt.upstream_rates;
    let data = asked.choice.data;
    let sizes: Vec<usize> = (0..INTERVALS).map(|i| data.points(i).len()).collect();
    let note = format!(
        "the DIL chose {} bit/s (drn {}), {sizes:?} points an interval, and a CPt for drn {}",
        data_rate(data.drn).unwrap_or(0),
        data.drn,
        training.drn
    );
    Ok(Chosen {
        training,
        data,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::dil::design;

    /// The DIL as an equaliser that calls U_INFO one would hand it over,
    /// `late` symbols after this end's count began, the other way up, round
    /// and round, through a route that robs the LSB of interval `robbed`.
    fn arrived(descriptor: &Descriptor, robbed: usize, late: usize) -> Vec<f64> {
        let unit = ucode::level(Law::Mu, 79);
        let mut values = vec![0.3; late];
        let pass: Vec<(u8, bool)> = descriptor.symbols().collect();
        for n in 0..pass.len() + MARGIN {
            let (u, positive) = pass[n % pass.len()];
            // The octet's LSB forced to one, as `v90::network` robs it.
            let mut octet = ucode::octet(Law::Mu, u, !positive);
            if n % INTERVALS == robbed {
                octet |= 1;
            }
            let (u, negative) = ucode::from_octet(Law::Mu, octet);
            let level = ucode::level(Law::Mu, u) / unit;
            values.push(if negative { level } else { -level });
        }
        values
    }

    /// One interval robbed: the DIL shows which, the others keep every
    /// level, and the rate is well above the 28 000 a CPt every interval
    /// reads would be.
    #[test]
    fn a_robbed_interval_gets_a_ladder_of_its_own() {
        let descriptor = design(Law::Mu, 79);
        let jd = Jd {
            rates: (1 << 22) - 1,
            ..Jd::default()
        };
        let reading = Reading {
            power_limit: 4024,
            pinned: None,
        };
        for (robbed, late) in [(0, 0), (3, 2), (5, 60)] {
            let values = arrived(&descriptor, robbed, late);
            let unit = ucode::level(Law::Mu, 79);
            let chosen = choose(
                &descriptor,
                Law::Mu,
                unit,
                &values,
                &jd,
                reading,
                &Cp::default(),
            )
            .expect("a choice");
            let sizes: Vec<usize> = (0..INTERVALS)
                .map(|i| chosen.data.points(i).len())
                .collect();
            assert!(
                data_rate(chosen.data.drn).unwrap() > 40_000,
                "{}",
                chosen.note
            );
            let smallest = (0..INTERVALS).min_by_key(|&i| sizes[i]).unwrap();
            assert_eq!(smallest, robbed, "{}", chosen.note);
        }
    }

    /// A pinned rate is a ceiling.
    #[test]
    fn a_pinned_rate_is_not_exceeded() {
        let descriptor = design(Law::Mu, 79);
        let jd = Jd {
            rates: (1 << 22) - 1,
            ..Jd::default()
        };
        let reading = Reading {
            power_limit: 4024,
            pinned: Some(10),
        };
        let unit = ucode::level(Law::Mu, 79);
        let chosen = choose(
            &descriptor,
            Law::Mu,
            unit,
            &arrived(&descriptor, 1, 0),
            &jd,
            reading,
            &Cp::default(),
        )
        .unwrap();
        assert!(chosen.data.drn <= 10, "{}", chosen.note);
    }

    /// Every DIL-segment is shorter than the 50 ms that would make it Tone B
    /// to a receiver watching for it (9.5.2.2).
    #[test]
    fn no_segment_of_the_dil_is_long_enough_to_be_tone_b() {
        for law in [Law::Mu, Law::A] {
            let descriptor = design(law, 79);
            for &u in &descriptor.ucodes {
                let chord = usize::from(u >> 4) & 7;
                let symbols = (usize::from(descriptor.h[chord]) + 1) * INTERVALS;
                assert!(
                    symbols as f64 / 8000.0 < 0.050,
                    "a segment of {symbols} symbols"
                );
            }
        }
    }
}
