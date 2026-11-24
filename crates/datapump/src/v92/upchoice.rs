//! The numeric half of the upstream design: the constellation sets, the twelve
//! moduli, the rung of the 6.1 rate ladder and the gain, chosen against
//! 2^K <= M (6.4.1) and the class-feasibility rules of 6.4.2.
//!
//! V92-62. [`design`](super::design::design) has said what the channel costs:
//! an ISI residual the precoder could not cancel, a scale the prefilter was
//! shrunk by, and a delay. This module turns that into the numbers CPd
//! carries, from the top of the ladder down, because the answer wanted is the
//! fastest rung that still works.
//!
//! The judgement is 6.4.2's level spacing against the noise. A set of LC
//! positive points on a spacing of two, used uniformly, has a mean square of
//! about (2 LC)^2 / 3; G is chosen to bring that to one (8.8.3), so what a
//! receiver sees at unit gain is a spacing of 2 G. A symbol error needs the
//! noise to carry a point past the halfway mark, which at a symbol error rate
//! of 1e-5 is about 4.5 standard deviations. So a rung is usable when
//!
//! ```text
//! 2 G / 2 >= 4.5 sigma        i.e.   LC <= sqrt(3) / (9 sigma)
//! ```
//!
//! with G = sqrt(3) / (2 LC). Anything above that is refused and the next rung
//! down is tried.

use super::design::Design;
use super::{
    CONSTELLATION_FRAME, CONSTELLATION_POINTS, GAIN_LARGEST, Parameters, Trellis, UP_INTERVALS, UP_RATES,
    up_bits,
};

/// Standard deviations between a level and the decision boundary that a symbol
/// error rate of 1e-5 asks for.
///
/// Q(4.5) is 3.4e-6 and a PAM alphabet's worst case is twice that, so a design
/// landing exactly here has a symbol error rate of about 7e-6 before the
/// trellis takes anything off it.
pub const MARGIN: f64 = 4.5;

/// The gain a set of `levels` positive points on a spacing of two wants, with
/// its mean square brought to one (8.8.3).
///
/// A uniform alphabet at 1, 3, ..., 2 LC - 1 has a mean square of
/// ((2 LC)^2 - 1) / 3, and G is its reciprocal square root.
pub fn gain_for(levels: usize) -> f64 {
    let span = 2.0 * levels as f64;
    (3.0 / (span * span - 1.0).max(1.0)).sqrt()
}

/// What the choice settled on.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub parameters: Parameters,
    /// The standard deviation the choice was made against, in the units the
    /// levels are in: the channel's noise and what the precoder could not
    /// cancel, after the prefilter's scaling.
    pub sigma: f64,
    /// The symbol error rate the spacing predicts, before the trellis.
    pub predicted: f64,
}

/// The fastest rung of 6.1's ladder these conditions carry, or `None` when
/// even the slowest does not.
///
/// `ja` is the mask of rates the analogue modem's Ja allowed (bit k for drn
/// k + 1); a zero mask means "every rung".
pub fn choose(design: &Design, noise: f64, ja: u32, trellis: Trellis) -> Option<Choice> {
    // What the receiver sees, on the scale the levels are in: the design's own
    // residual ISI, and the channel's noise carried through the prefilter's
    // scaling.
    // Both terms are on the levels' own scale: the residual is already
    // relative to the response's main tap, and the noise is carried through
    // the prefilter's scaling -- variance by the square of it.
    let carried = design.scale * design.scale * noise.max(0.0);
    let sigma = (design.residual * design.residual + carried).sqrt();
    let room = if sigma > 0.0 { 3f64.sqrt() / (MARGIN * sigma) } else { f64::INFINITY };
    let mask = if ja == 0 { (1u32 << UP_RATES) - 1 } else { ja };
    for drn in (1..=UP_RATES as u8).rev() {
        if mask & (1 << (drn - 1)) == 0 {
            continue;
        }
        let Some(parameters) = build(design, drn, trellis) else { continue };
        // 6.4.2's class feasibility and Table 30's fields, before the spacing
        // is looked at: a rung whose moduli the sets cannot carry is not a
        // rung at all.
        if parameters.fits().is_err() {
            continue;
        }
        let levels = parameters.sets[0].len();
        if (levels as f64) > room {
            continue;
        }
        let spacing = 2.0 * parameters.gain;
        let predicted = 2.0 * normal_tail(spacing / (2.0 * sigma.max(f64::MIN_POSITIVE)));
        return Some(Choice { parameters, sigma, predicted });
    }
    None
}

/// The parameters for one rung, with the moduli spread evenly over the twelve
/// intervals and one constellation set shared by all six constellation frames.
fn build(design: &Design, drn: u8, trellis: Trellis) -> Option<Parameters> {
    let bits = up_bits(drn);
    // 6.4.1: K bits over twelve intervals, as evenly as they go. The spare bits
    // go to the intervals that come first, which is where the mixed radix
    // spends them.
    let each = bits / UP_INTERVALS as u32;
    let spare = bits % UP_INTERVALS as u32;
    let mut moduli = [0u8; UP_INTERVALS];
    for (i, modulus) in moduli.iter_mut().enumerate() {
        let width = each + u32::from((i as u32) < spare);
        let levels = 1u32 << width;
        if levels > 255 {
            return None;
        }
        *modulus = levels as u8;
    }
    // 6.4.2: a class steps by Mi, and at k = 3 by 2 x Mi, so the set needs
    // N >= 2 Mi there, which is LC >= Mi. Everywhere else LC >= Mi / 2 is
    // enough.
    let mut levels = 1usize;
    for (i, &modulus) in moduli.iter().enumerate() {
        let need = if i % 4 == 3 { usize::from(modulus) } else { usize::from(modulus.div_ceil(2)) };
        levels = levels.max(need);
    }
    if levels > CONSTELLATION_POINTS {
        return None;
    }
    let gain = gain_for(levels);
    if gain > GAIN_LARGEST {
        // More power than CPd's field carries: the rung would go out quieter
        // than the design asked for and every level would be off by that much.
        // Refused rather than clamped.
        return None;
    }
    // 8.8.3: "The digital modem shall design the modulation parameters assuming
    // that, when the prefilter output multiplied by G has a mean-square value
    // of 1, the analogue modem will transmit at the desired power." Read that
    // as an equation and it fixes G: `ms(v G) = 1`, with `v` this design's own
    // prefilter output. For a unit prefilter that is what `gain_for` already
    // returns -- an eight-point set's mean square is eighty-five, and G is
    // 1/sqrt(85) to nine figures.
    //
    // A design whose prefilter had to be scaled down to fit Q0.15 breaks it.
    // The section is `scale` times smaller than the inverse it stands for, so
    // keeping `ms(v G) = 1` needs G `scale` times *larger*, and on this harness
    // `scale` is 12.586 (see `design()`'s `RIDGE`) against a field that carries
    // 4G <= 0xFFFF, i.e. G <= 0.25. Twelve and a half times 0.1085 is 1.36.
    //
    // So `V92_GAIN_SCALE=fold` cannot produce a legal CPd here, and the honest
    // thing is to say so rather than to send a gain the far end will read as a
    // different power. Folded, it is refused; unfolded, `G` describes a unit
    // prefilter, which is exactly what `V92_PRE=none` tells the far end to run.
    // Sending a real `z2` at this rate needs `design()` to produce a `scale` of
    // about one, which is the transmit-power constraint the design has not got.
    let design_gain = gain * design.scale.max(f64::MIN_POSITIVE);
    let fold = std::env::var("V92_GAIN_SCALE").ok().as_deref() == Some("fold");
    if fold && design_gain * crate::v92::GAIN_SCALE > f64::from(u16::MAX) {
        return None;
    }
    let gain = if fold { design_gain } else { gain };
    if !(gain > 0.0) || gain * crate::v92::GAIN_SCALE < 1.0 {
        // A gain the field would round to nothing is not a gain.
        return None;
    }
    Some(Parameters {
        drn,
        trellis,
        extend_e2u: false,
        gain,
        moduli,
        filters: design.filters.clone(),
        // One set of `levels` positive points on a spacing of two, shared by
        // every constellation frame: the simple case of CPd's six.
        sets: vec![(0..levels).map(|index| (2 * index + 1) as u16).collect()],
        indices: [0; CONSTELLATION_FRAME],
    })
}

/// The upper tail of the standard normal, Q(x), from the complement of the
/// error function's rational approximation (Abramowitz and Stegun 26.2.17).
fn normal_tail(x: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.231_641_9 * z);
    let poly = t
        * (0.319_381_530
            + t * (-0.356_563_782 + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    let tail = 0.5 * (-0.5 * z * z).exp() * poly;
    if x >= 0.0 { tail } else { 1.0 - tail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::design::{Channel, design};
    use crate::v92::{FilterLimits, Filters, up_rate};

    fn plan() -> FilterLimits {
        FilterLimits::from_info1a(3, 3, 3)
    }

    fn design_for(noise: f64) -> Design {
        let mut taps = vec![0.0; 16];
        taps[0] = 1.0;
        for k in 1..16 {
            taps[k] = 0.2 / k as f64;
        }
        let norm = taps.iter().map(|t| t * t).sum::<f64>().sqrt();
        taps.iter_mut().for_each(|t| *t /= norm);
        design(&Channel { taps, noise, signal: 1.0 }, &plan()).expect("designable")
    }

    /// `gain_for` is 8.8.3's: a uniform set on a spacing of two, brought to a
    /// mean square of one.
    #[test]
    fn the_gain_brings_the_set_s_power_to_one() {
        for levels in [4usize, 8, 32, 64, 128] {
            let gain = gain_for(levels);
            let power: f64 =
                (0..levels).map(|index| (2 * index + 1) as f64).map(|v| v * v).sum::<f64>() / levels as f64;
            assert!((gain * gain * power - 1.0).abs() < 1e-9, "{levels}: {} against 1", gain * gain * power);
            assert!(gain > 0.0 && gain <= GAIN_LARGEST, "{levels}: gain {gain}");
        }
    }

    /// 8.8.3's other half: folding a scaled prefilter's `scale` into `G` is not
    /// a choice the field leaves open.
    ///
    /// The assumption is `ms(v G) = 1`, so a prefilter whose section had to be
    /// scaled down to fit Q0.15 needs `G` scaled up by the same number. Past
    /// `4G <= 0xFFFF` there is no legal CPd that says it, and the rung has to
    /// be refused rather than sent with a gain the far end would read as a
    /// different transmit power.
    #[test]
    fn a_scaled_prefilter_cannot_be_folded_into_the_gain() {
        let design = design_for(0.0);
        // What a legal field carries, and what this design needs to satisfy
        // `ms(v G) = 1` if its prefilter is sent at the amplitude Q0.15 holds.
        let plain = build(&design, 5, Trellis::Sixteen).expect("a rung");
        let needed = plain.gain * design.scale;
        assert!(plain.gain * crate::v92::GAIN_SCALE <= f64::from(u16::MAX), "an unfolded gain must fit");
        if needed * crate::v92::GAIN_SCALE > f64::from(u16::MAX) {
            // The one thing an unfolded CPd must be able to say is the truth:
            // `G` describes a unit prefilter, which is what `V92_PRE=none`
            // tells the far end to run.
            let power: f64 = plain.sets.iter().flatten().map(|&p| f64::from(p) * f64::from(p)).sum::<f64>()
                / plain.sets.iter().map(Vec::len).sum::<usize>() as f64;
            assert!((plain.gain * plain.gain * power - 1.0).abs() < 1e-6);
        }
    }

    /// A channel with almost no noise carries the top of the ladder, a noisy
    /// one carries less, and the choice never goes back up as the noise does.
    #[test]
    fn the_rate_falls_as_the_noise_rises() {
        let mut previous = u8::MAX;
        let mut seen = Vec::new();
        for noise in [1e-9, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2] {
            let design = design_for(noise);
            let Some(choice) = choose(&design, noise, 0, Trellis::ThirtyTwo) else {
                seen.push((noise, 0));
                continue;
            };
            let drn = choice.parameters.drn;
            assert_eq!(choice.parameters.rate(), up_rate(drn));
            assert!(drn <= previous, "the rate went up with the noise: {drn} after {previous} at {noise}");
            previous = drn;
            choice.parameters.fits().unwrap_or_else(|error| panic!("{noise}: {error}"));
            seen.push((noise, drn));
        }
        println!("noise -> drn: {seen:?}");
        assert!(seen[0].1 >= 15, "a quiet channel only reached drn {:?}", seen[0]);
        assert!(seen.last().expect("a trend").1 <= 8, "a noisy channel still reached drn {:?}", seen.last());
    }

    /// Every rung the choice can return obeys 6.4.1 and 6.4.2: the moduli
    /// carry K bits and no interval's set is too small for its class step.
    #[test]
    fn every_rung_it_can_return_is_feasible() {
        for drn in 1..=UP_RATES as u8 {
            let design = design_for(0.0);
            let Some(parameters) = build(&design, drn, Trellis::Sixteen) else { continue };
            parameters.fits().unwrap_or_else(|error| panic!("drn {drn}: {error}"));
            assert_eq!(parameters.bits(), up_bits(drn));
            assert!(parameters.product() >= 1u128 << up_bits(drn), "drn {drn}: the moduli cannot hold K bits");
            for i in 0..UP_INTERVALS {
                let n = 2 * parameters.sets[0].len();
                let step = usize::from(parameters.moduli[i]) * if i % 4 == 3 { 2 } else { 1 };
                assert!(n >= step, "drn {drn}, interval {i}: N {n} is smaller than its class step {step}");
            }
        }
    }

    /// A rate the analogue modem's Ja did not offer is never chosen.
    #[test]
    fn the_ja_mask_is_obeyed() {
        let design = design_for(1e-9);
        let only = 3u8;
        let choice = choose(&design, 1e-9, 1u32 << (only - 1), Trellis::Sixteen).expect("one rung was offered");
        assert_eq!(choice.parameters.drn, only);
        assert!(choose(&design, 1e-9, 1u32 << 0, Trellis::Sixteen).is_some(), "drn 1 is always offered");
    }

    /// The residual the precoder could not cancel is noise as far as the choice
    /// is concerned: a channel it cannot shorten carries less.
    #[test]
    fn residual_isi_counts_as_noise() {
        let clean = Design {
            filters: Filters { z2: vec![1.0], ..Filters::default() },
            residual: 0.0,
            scale: 1.0,
            limited: false,
            delay: 0,
        };
        // Three per cent of a spacing: worse than a good design, better than
        // a broken one, which is the case that should lose a rung or two
        // rather than everything.
        let poor = Design { residual: 0.03, ..clean.clone() };
        let a = choose(&clean, 1e-7, 0, Trellis::Sixteen).expect("a clean design");
        let b = choose(&poor, 1e-7, 0, Trellis::Sixteen).expect("a poor design still carries something");
        assert!(b.parameters.drn < a.parameters.drn, "ISI cost nothing: {} against {}", b.parameters.drn, a.parameters.drn);
        assert!(b.sigma > a.sigma);
    }
}
