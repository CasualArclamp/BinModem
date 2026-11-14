//! The digital modem's precoder and prefilter design (6.4.2, 8.8.3 and its
//! NOTE): an estimate of the upstream channel, turned into the coefficients
//! and the gain that CPd downloads to the analogue modem, inside the limits
//! the analogue modem announced in INFO1a bits 12:17.
//!
//! V92-21. The shape of the answer, and why it works:
//!
//! ```text
//! analogue:  u(n) -> precoder x(n) -> prefilter v(n) -> line
//! line:      v(n) -> channel c -> codec's A/D -> digital modem
//! ```
//!
//! 8.8.3's NOTE says to "design the precoder coefficients assuming the
//! analogue modem minimizes the power at the precoder output symbol by
//! symbol", which is what [`Chain`](super::precoder::Chain) does when it picks
//! the constellation member nearest the precoder's tail. That gives the design
//! its freedom: the prefilter's job is to shorten the *channel*, so that the
//! precoder's feedback can cancel what is left of it.
//!
//! So there are two steps. The prefilter `z2` is the feed-forward equaliser
//! for the channel, found as the least-squares inverse of the estimated
//! impulse response within the length the far end offered. The precoder's
//! feedback `p1` is then the negative of the residual response's tail, which is
//! what makes the cascade `u -> precoder -> prefilter -> channel` come back to
//! `u` at the codec. What neither can reach -- a leading tap, or a tail longer
//! than `p1` may be -- is reported as [`Design::residual`], so that the rate
//! choice can add it to the measured noise before it picks moduli (V92-62).
//!
//! Nothing here is on the wire: `Cpd` is `v92::sequences`' business, and this
//! package deals in [`Filters`], as AD-3 asks.

use dsp::Complex;

use super::{FilterLimits, Filters};

/// How many taps of the channel are estimated when a caller has no better
/// number. The upstream channel is the analogue modem's transmit filter, the
/// local loop, the central office's band limiting and the A/D; TRN1u is 2040
/// symbols, so a least-squares fit of this many taps has many times as many
/// equations as unknowns.
pub const CHANNEL_TAPS: usize = 64;

/// The floor under the inverse's ridge, as a fraction of the autocorrelation's
/// trace: the smallest the arithmetic will take.
///
/// A least-squares inverse of a channel with a null in its band -- every real
/// loop has one at DC -- puts an enormous coefficient on that null, and the
/// quantiser then either saturates it or loses it. `RIDGE` alone was the whole
/// story here once, and it is far too small to stop that: on the harness the
/// section came out needing twelve and a half times the dynamic range Q0.15 has
/// (`Design::scale` 12.586), which is a prefilter no far end can run at the
/// amplitude `G` names.
///
/// What the ridge should be is the reciprocal of the signal to noise the design
/// is being made into. Minimising ISI alone is the wrong problem -- it is
/// solvable and it is what the constant below did -- and minimising ISI subject
/// to a transmit power the line can carry is the right one. That trade is
/// exactly what a ridge is, so it is [`Channel::noise_ratio`], floored here and
/// capped by [`ALPHA_MOST`].
const RIDGE: f64 = 1e-6;

/// The largest ridge a design will take.
///
/// The ridge trades residual ISI for transmit power, and past about a tenth the
/// inverse stops being an inverse at all -- it is a matched filter, which puts
/// the whole of the channel's memory back as ISI for the precoder to cancel.
/// A design that asks for more than this is a design for a channel that should
/// not be carrying this rung, not a design with a better ridge.
const ALPHA_MOST: f64 = 0.1;

/// The upstream channel as the digital modem sees it: T-spaced taps and the
/// noise the estimate was made in.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub taps: Vec<f64>,
    /// The power of what the taps do not explain, as a mean square on the
    /// same scale as the taps' input.
    pub noise: f64,
    /// The mean square of the line the estimate was made on, on that same
    /// scale: what `noise` is a noise *against*.
    ///
    /// A telephone loop's loss is not fixed -- it depends on the loop, on the
    /// far end's transmit level and on the network -- so the one number the
    /// design must not take from a constant is the signal to noise it is
    /// designing into. This is the measurement, taken on every call from the
    /// same window the taps came from.
    pub signal: f64,
}

impl Channel {
    /// The least-squares fit of `received` against `known`, T-spaced.
    ///
    /// `known` is what the analogue modem was told to send -- TRN1u's GPA
    /// sequence, which this side can generate exactly -- and `received` what
    /// the codec made of it. The fit is over `taps` taps and every symbol whose
    /// whole support is inside the two runs.
    pub fn estimate(known: &[f64], received: &[f64], taps: usize) -> Option<Self> {
        let length = known.len().min(received.len());
        let taps = taps.clamp(1, (length / 2).max(1));
        if length < 2 * taps {
            return None;
        }
        // The normal equations of the convolution: A is the known sequence's
        // autocorrelation, b its cross-correlation with what arrived.
        let mut a = vec![Complex::ZERO; taps * taps];
        let mut b = vec![Complex::ZERO; taps];
        for i in 0..taps {
            for j in 0..taps {
                let mut sum = 0.0;
                for n in taps - 1..length {
                    sum += known[n - i] * known[n - j];
                }
                a[i * taps + j] = Complex::new(sum, 0.0);
            }
            let mut sum = 0.0;
            for n in taps - 1..length {
                sum += known[n - i] * received[n];
            }
            b[i] = Complex::new(sum, 0.0);
        }
        // A ridge, so that a null in the channel's band cannot take the fit
        // with it.
        let trace: f64 = (0..taps).map(|i| a[i * taps + i].re).sum();
        let ridge = RIDGE * trace / taps as f64;
        for i in 0..taps {
            a[i * taps + i].re += ridge;
        }
        let solved = dsp::complex::solve_hermitian(&a, &b)?;
        let fitted: Vec<f64> = solved.iter().map(|value| value.re).collect();
        // What the fit leaves: the noise the design has to work in.
        let mut power = 0.0;
        let mut count = 0usize;
        for n in taps - 1..length {
            let mut model = 0.0;
            for (k, &tap) in fitted.iter().enumerate() {
                model += tap * known[n - k];
            }
            power += (received[n] - model) * (received[n] - model);
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let mut signal = 0.0;
        for n in taps - 1..length {
            signal += received[n] * received[n];
        }
        Some(Self { taps: fitted, noise: power / count as f64, signal: signal / count as f64 })
    }

    /// The reciprocal of this channel's signal to noise, which is the ridge its
    /// prefilter's inverse wants: see [`RIDGE`].
    ///
    /// Bounded at both ends so that a measurement taken in the wrong window --
    /// silence, or a burst -- cannot take the design with it.
    #[must_use]
    pub fn noise_ratio(&self) -> f64 {
        if !(self.signal > 0.0) || !self.noise.is_finite() {
            return RIDGE;
        }
        (self.noise / self.signal).clamp(RIDGE, ALPHA_MOST)
    }

    /// The channel's response to a unit impulse over `length` symbols.
    pub fn impulse(&self, length: usize) -> Vec<f64> {
        let mut out = vec![0.0; length.max(self.taps.len())];
        for (k, &tap) in self.taps.iter().enumerate() {
            out[k] = tap;
        }
        out
    }

    /// How much response a symbol's own position carries against the rest of
    /// it: zero for a channel with no memory, and infinity for none at all.
    pub fn severity(&self) -> f64 {
        let peak = self.taps.iter().fold(0.0f64, |most, &tap| most.max(tap.abs()));
        if peak == 0.0 {
            return f64::INFINITY;
        }
        let rest: f64 = self.taps.iter().map(|tap| tap * tap).sum::<f64>().sqrt();
        rest / peak
    }
}

/// What the design settled on.
#[derive(Debug, Clone, PartialEq)]
pub struct Design {
    /// z1, p1, z2 and p2, on the scale the levels are in.
    pub filters: Filters,
    /// The residual intersymbol interference, as a fraction of the level
    /// spacing: what the precoder could not cancel.
    pub residual: f64,
    /// How far the whole design was scaled to keep the prefilter inside
    /// Q0.15, which the rate choice multiplies into G.
    pub scale: f64,
    /// Whether the far end's limits, rather than the channel, decided the
    /// sections' lengths.
    pub limited: bool,
    /// The symbol delay the design puts between what the analogue modem sends
    /// and what the codec reads: the position of the response's main tap.
    /// A receiver has to find this on TRN1u before it can frame data, and
    /// [`Decoder`](super::decoder::Decoder) is built to be started at it.
    pub delay: usize,
}

impl Design {
    /// The residual as a signal-to-interference ratio, in decibels.
    pub fn residual_db(&self) -> f64 {
        if self.residual <= 0.0 { f64::INFINITY } else { -20.0 * self.residual.log10() }
    }
}

/// Design the prefilter and the precoder for `channel` inside `limits`.
///
/// The prefilter is the least-squares inverse of the channel inside LZ2; the
/// precoder's feedback is the residual response's tail inside LP1, negated;
/// and where the far end offered z1, the response's leading taps are moved into
/// it rather than left as ISI. What could not be reached is reported as
/// [`Design::residual`] rather than failing: a design that is merely poor is
/// still a design, and the rate choice is what decides whether it is worth
/// trying.
pub fn design(channel: &Channel, limits: &FilterLimits) -> Result<Design, &'static str> {
    if channel.taps.is_empty() {
        return Err("there is no channel to design against");
    }
    let most = usize::from(limits.most).max(1);
    let total = usize::from(limits.total).max(1);
    if limits.total == 0 || limits.most == 0 {
        return Err("the far end offered no filter sections at all");
    }
    let peak = channel.taps.iter().fold(0.0f64, |a, &tap| a.max(tap.abs()));
    if peak == 0.0 {
        return Err("the channel is empty");
    }
    // z2 is the feed-forward section, and it is the one section every PCM
    // upstream modem must offer (Table 18 bit 12: p1 and z2 at least).
    // `V92_LZ2` overrides the section length, which is the one lever on how
    // hard the prefilter inverts: the normalised inverse's peak does not move
    // with the ridge (raising it scales the inverse and the response's own main
    // tap by the same factor), so if a channel's prefilter does not fit Q0.15
    // the length is what has to give, not the ridge.
    let lz2 = std::env::var("V92_LZ2").ok().and_then(|text| text.parse().ok()).unwrap_or(most.min(total));
    let inverse = inverse(channel, lz2);
    // h is c convolved with z2: what is left for the precoder to cancel.
    let mut response = convolve(&channel.taps, &inverse);
    // Where the response's own energy is: the precoder's reference tap.
    let main = response
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map(|(at, _)| at)
        .unwrap_or(0);
    let unit = response[main];
    if unit.abs() < 1e-9 {
        return Err("the prefilter's response has no main tap to build on");
    }
    // Normalising on the main tap is what lets p1 talk about levels rather
    // than about the channel's own scale, and it is done before the
    // prefilter is scaled so that the two do not chase each other: p1 is a
    // ratio of the response to itself, and survives the scaling below
    // untouched.
    response.iter_mut().for_each(|value| *value /= unit);
    let mut z2: Vec<f64> = inverse.iter().map(|value| value / unit).collect();
    // z2 is Q0.15, [-1, 1). A channel whose inverse needs more than that is
    // answered by scaling the whole prefilter down and telling the rate
    // choice, which folds the scale into G.
    let largest = z2.iter().fold(0.0f64, |a, &tap| a.max(tap.abs()));
    let mut scale = 1.0;
    if largest >= 0.99 {
        scale = largest / 0.99;
        z2.iter_mut().for_each(|value| *value /= scale);
    }
    let mut filters = Filters { z2, ..Filters::default() };

    let mut unreached = 0.0f64;
    let mut limited = false;

    // The lead-in: taps before the main one. With z1 the precoder can cancel
    // them from u's own history; without it they are ISI.
    if main > 0 {
        if limits.sections.z1 {
            let room = most.min(total.saturating_sub(filters.z2.len()));
            let take = room.min(main);
            filters.z1 = (1..=take).map(|k| -response[main - k]).collect();
            for k in take + 1..=main {
                unreached += response[main - k] * response[main - k];
            }
            if take < main {
                limited = true;
            }
        } else {
            for k in 1..=main {
                unreached += response[main - k] * response[main - k];
            }
            limited = true;
        }
    }

    // The tail: what the precoder's feedback cancels.
    let room = most.min(total.saturating_sub(filters.z2.len() + filters.z1.len()));
    let tail = response.len().saturating_sub(main + 1);
    let take = room.min(tail);
    filters.p1 = (1..=take).map(|k| -response[main + k]).collect();
    for k in take + 1..=tail {
        unreached += response[main + k] * response[main + k];
    }
    // A channel with nothing to cancel gets no precoder at all rather than a
    // section of near-zero coefficients: a far end told to run a 320-tap
    // feedback of zeros is being asked to do work for nothing.
    while filters.p1.last().is_some_and(|value| value.abs() < 1e-9) {
        filters.p1.pop();
    }
    if take < tail {
        limited = true;
    }

    // p1 is Q1.14, [-2, 2): a coefficient outside it means the response is not
    // the shortened one the design intended.
    if let Some(&largest) = filters.p1.iter().max_by(|a, b| a.abs().total_cmp(&b.abs()))
        && largest.abs() >= 2.0
    {
        return Err("a precoder coefficient does not fit q1.14");
    }
    if filters.z2.iter().any(|value| value.abs() >= 1.0) {
        return Err("a prefilter coefficient does not fit q0.15");
    }
    filters.fits(limits)?;
    Ok(Design { filters, residual: unreached.sqrt(), scale, limited, delay: main })
}

/// The least-squares inverse of the channel inside `length` taps.
///
/// Wiener-Hopf on the channel's own autocorrelation: the coefficients that
/// make `c * w` nearest a unit impulse at the channel's first significant tap,
/// with the ridge that keeps a null in the band from running away with them.
fn inverse(channel: &Channel, length: usize) -> Vec<f64> {
    inverse_with(channel, length, channel.noise_ratio())
}

/// [`inverse`] at a stated ridge, as a fraction of the autocorrelation's trace.
fn inverse_with(channel: &Channel, length: usize, relative_ridge: f64) -> Vec<f64> {
    let taps = &channel.taps;
    let length = length.max(1);
    let mut a = vec![Complex::ZERO; length * length];
    for i in 0..length {
        for j in 0..length {
            let mut sum = 0.0;
            for k in 0..taps.len() {
                let left = k as i64 - i as i64;
                let right = k as i64 - j as i64;
                if left >= 0 && right >= 0 {
                    sum += taps[left as usize] * taps[right as usize];
                }
            }
            a[i * length + j] = Complex::new(sum, 0.0);
        }
    }
    let trace: f64 = (0..length).map(|i| a[i * length + i].re).sum();
    let ridge = relative_ridge * trace / length as f64;
    for i in 0..length {
        a[i * length + i].re += ridge;
    }
    // The target impulse sits where the channel's own energy is, so that the
    // inverse is causal and as short as it can be.
    let at = taps
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .map_or(0, |(index, _)| index);
    // (C^T delta)[i] is the channel at the target's position minus i, so the
    // right-hand side is the channel read backwards from `at`.
    let mut b = vec![Complex::ZERO; length];
    for i in 0..length.min(at + 1) {
        b[i] = Complex::new(taps[at - i], 0.0);
    }
    let solved = dsp::complex::solve_hermitian(&a, &b).unwrap_or_else(|| {
        // A channel so degenerate that the ridge cannot make the system
        // solvable is answered with a pass-through: the design reports a poor
        // residual, and the rate choice takes that as it finds it.
        let mut fallback = vec![Complex::ZERO; length];
        fallback[0] = Complex::ONE;
        fallback
    });
    solved.iter().map(|value| value.re).collect()
}

/// The convolution of two sequences.
fn convolve(first: &[f64], second: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; first.len() + second.len() - 1];
    for (i, &a) in first.iter().enumerate() {
        for (j, &b) in second.iter().enumerate() {
            out[i + j] += a * b;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::decoder::{Decoder, SURVIVOR_DEPTH};
    use crate::v92::upstream::UpEncoder;
    use crate::v92::{CONSTELLATION_FRAME, Parameters, Trellis, up_bits};

    fn limits(sections: u8, total: u8, most: u8) -> FilterLimits {
        FilterLimits::from_info1a(sections, total, most)
    }

    /// One tap at unit gain: nothing to equalise.
    #[test]
    fn an_ideal_channel_gets_a_pass_through_and_no_precoder() {
        let channel = Channel { taps: vec![1.0], noise: 0.0, signal: 1.0 };
        let design = design(&channel, &limits(3, 3, 3)).expect("an ideal channel is designable");
        assert!(design.filters.z2[0] > 0.98, "{:?}", design.filters.z2[0]);
        assert_eq!(design.delay, 0, "an ideal channel adds no delay");
        // The rest of the section is nothing: an ideal channel needs one tap.
        let rest = design.filters.z2[1..].iter().fold(0.0f64, |a, &tap| a.max(tap.abs()));
        assert!(rest < 1e-3, "the prefilter's tail is {rest}, not nothing");
        assert!(design.filters.p1.is_empty(), "nothing to cancel: {:?}", design.filters.p1);
        assert!(design.residual < 1e-3, "residual {}", design.residual);
    }

    /// A channel with the shape a local loop and a codec give it: a null near
    /// DC, flat to 3.4 kHz and a roll-off after. The design has to leave less
    /// ISI than a symbol's noise margin.
    #[test]
    fn a_loop_model_is_equalised_to_well_under_the_level_spacing() {
        let mut taps = vec![0.0; 40];
        taps[0] = 1.0;
        taps[1] = -0.9;
        for k in 2..18 {
            taps[k] = 0.12 / (k as f64 - 1.0);
        }
        let norm = taps.iter().map(|t| t * t).sum::<f64>().sqrt();
        taps.iter_mut().for_each(|t| *t /= norm);
        let channel = Channel { taps, noise: 1e-6, signal: 1.0 };
        let design = design(&channel, &limits(3, 3, 3)).expect("a loop is designable");
        assert!(design.residual_db() > 25.0, "residual {:.1} dB, filters {:?}", design.residual_db(), design.filters);
    }

    /// 8.8.3 and Table 18 bits 14:17: the coefficients have to fit what the
    /// analogue modem offered, whatever the channel wants.
    #[test]
    fn the_design_fits_inside_every_set_of_limits() {
        let mut taps = vec![0.0; 30];
        taps[0] = 1.0;
        for k in 1..30 {
            taps[k] = 0.3 / k as f64;
        }
        let channel = Channel { taps, noise: 1e-6, signal: 1.0 };
        for sections in 0..4u8 {
            for (total, most) in [(0u8, 0u8), (1, 1), (2, 2), (3, 3)] {
                let limit = limits(sections, total, most);
                let design = design(&channel, &limit).expect("designable");
                design.filters.fits(&limit).unwrap_or_else(|error| panic!("{sections}/{total}/{most}: {error}"));
                assert!(design.filters.total() <= usize::from(limit.total));
                if sections & 1 == 0 {
                    assert!(design.filters.z1.is_empty(), "z1 was not offered");
                }
                assert!(design.filters.p2.is_empty(), "p2 is never designed here");
            }
        }
    }

    /// A coefficient set that does not fit is refused by the check it will
    /// face when a CPd carries it.
    #[test]
    fn a_broken_set_is_caught_by_the_self_check() {
        let too_long = Filters { z2: vec![1.0; 130], ..Filters::default() };
        assert!(too_long.fits(&limits(0, 0, 0)).is_err(), "a 130-tap section is over Lmax 128");
        let too_many = Filters { z2: vec![1.0; 100], p1: vec![0.5; 100], ..Filters::default() };
        assert!(too_many.fits(&limits(0, 0, 3)).is_err(), "200 taps is over Ltot 192");
        let unoffered = Filters { z1: vec![0.1], z2: vec![1.0], ..Filters::default() };
        assert!(unoffered.fits(&limits(0, 3, 3)).is_err(), "z1 was not offered");
    }

    /// 8.8.3's NOTE, end to end: the design is what makes the precoder's
    /// symbol-by-symbol power minimisation enough on its own. The encoder's
    /// prefilter output goes down the same channel the design was made for,
    /// and the decoder gets every bit back.
    #[test]
    fn a_designed_chain_decodes_through_its_own_channel() {
        let mut taps = vec![0.0; 24];
        taps[0] = 1.0;
        for k in 1..24 {
            taps[k] = 0.25 / k as f64;
        }
        let norm = taps.iter().map(|t| t * t).sum::<f64>().sqrt();
        taps.iter_mut().for_each(|t| *t /= norm);
        let channel = Channel { taps: taps.clone(), noise: 1e-6, signal: 1.0 };
        let plan = limits(3, 3, 3);
        let design = design(&channel, &plan).expect("designable");
        assert!(design.residual_db() > 20.0, "residual {:.1} dB", design.residual_db());

        let drn = 13u8;
        let parameters = Parameters {
            drn,
            trellis: Trellis::Sixteen,
            extend_e2u: false,
            gain: 1.0,
            moduli: [1u8 << (up_bits(drn) / 12); 12],
            filters: design.filters.clone(),
            sets: vec![(0..64).map(|index| (2 * index + 1) as u16).collect()],
            indices: [0; CONSTELLATION_FRAME],
        };
        let mut encoder = UpEncoder::new(&parameters).expect("feasible");
        let mut decoder = Decoder::new(&parameters);
        let mut seed = 0x1234_5678_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed >> 63 == 0
        };
        // The channel's own history, as the line carries it.
        let mut line = vec![0.0f64; taps.len()];
        let mut sent = Vec::new();
        let mut received = Vec::new();
        // The design's delay is what a receiver finds on TRN1u before it can
        // frame data, so the first `delay` symbols of the line are skipped.
        let mut skip = design.delay;
        for _ in 0..(design.delay + SURVIVOR_DEPTH * 6 * 12) {
            let symbol = encoder.next_symbol(|| {
                let bit = next();
                sent.push(bit);
                bit
            });
            line.rotate_right(1);
            line[0] = symbol.v;
            let mut seen = 0.0;
            for (k, &tap) in taps.iter().enumerate() {
                seen += tap * line[k];
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            // The design scales the prefilter down to fit Q0.15, so what
            // arrives is that much quieter than the levels are. A receiver
            // normalises it, which is what `Design::scale` is for -- the rate
            // choice folds the same number into G.
            decoder.feed(seen * design.scale);
            received.extend(decoder.take_bits());
        }
        assert!(received.len() > sent.len() / 2, "{} bits decoded of {}", received.len(), sent.len());
        let shortest = sent.len().min(received.len());
        let at = (0..shortest.min(400))
            .find(|&offset| {
                let length = shortest - offset;
                length > 100
                    && sent[offset..offset + length].iter().zip(&received[..length]).all(|(a, b)| a == b)
            })
            .unwrap_or_else(|| {
                let errors = sent.iter().zip(&received).filter(|(a, b)| a != b).count();
                panic!("the streams do not line up: {errors} of {shortest} bits wrong")
            });
        let length = shortest - at;
        let errors = sent[at..].iter().zip(&received[..length]).filter(|(a, b)| a != b).count();
        assert_eq!(errors, 0, "{errors} of {length} bits wrong through the designed channel");
    }

    /// The whole upstream chain as a bench sees it: the analogue modem's
    /// precoder and prefilter, the gain, a quantiser, and this end's receiver at
    /// the scale the design tells it to use.
    ///
    /// Returns (bits wrong, bits compared) after lining the two streams up.
    fn through_the_line(quantise: impl Fn(f64) -> f64) -> (usize, usize) {
        let mut taps = vec![0.0; 20];
        taps[0] = 1.0;
        for k in 1..20 {
            taps[k] = 0.35 / k as f64;
        }
        let norm = taps.iter().map(|t| t * t).sum::<f64>().sqrt();
        taps.iter_mut().for_each(|t| *t /= norm);
        let channel = Channel { taps: taps.clone(), noise: 1e-6, signal: 1.0 };
        let design = design(&channel, &limits(3, 3, 3)).expect("designable");
        let drn = 1u8;
        let choice = crate::v92::upchoice::choose(&design, 1e-6, 1 << (drn - 1), Trellis::Sixteen)
            .expect("the slowest rung carries this line");
        let parameters = choice.parameters;
        let mut encoder = UpEncoder::new(&parameters).expect("feasible");
        let mut decoder = Decoder::new(&parameters);
        let mut seed = 0x5eed_1234_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed >> 63 == 0
        };
        let mut line = vec![0.0f64; taps.len()];
        let (mut sent, mut received) = (Vec::new(), Vec::new());
        for _ in 0..(SURVIVOR_DEPTH * 6 * 12 + 2400) {
            let symbol = encoder.next_symbol(|| {
                let bit = next();
                sent.push(bit);
                bit
            });
            line.rotate_right(1);
            line[0] = symbol.out;
            let seen: f64 = taps.iter().enumerate().map(|(k, &tap)| tap * line[k]).sum();
            // The analogue modem transmits at its own power, well below the
            // codec's full scale: G x v having mean square one is the *design's*
            // unit, not a level to put on a 16-bit codec at.
            let level = 0.25;
            decoder.feed(quantise(seen * level) / level * design.scale);
            received.extend(decoder.take_bits());
        }
        let shortest = sent.len().min(received.len());
        assert!(shortest > 400, "{} bits decoded of {}", received.len(), sent.len());
        let at = (0..shortest.min(400))
            .find(|&offset| {
                let length = shortest - offset;
                length > 200
                    && sent[offset..offset + length].iter().zip(&received[..length]).all(|(a, b)| a == b)
            })
            .unwrap_or(0);
        let length = shortest - at;
        let errors = sent[at..].iter().zip(&received[..length]).filter(|(a, b)| a != b).count();
        (errors, length)
    }

    /// A quantiser fine enough that the design's own residual is all that is
    /// left: the chain is what it should be.
    #[test]
    fn a_designed_chain_survives_a_fine_quantiser() {
        let (errors, length) = through_the_line(|seen| (seen * 32768.0).round() / 32768.0);
        assert_eq!(errors, 0, "{errors} of {length} bits wrong through a 16 bit line");
    }

    /// Through the codec a PCM upstream line actually ends in: the same chain,
    /// to the bit, once the levels are put on the line at a power a codec has
    /// room for.
    ///
    /// That last clause is the whole of it. `G x v` having mean square one is
    /// the design's own unit (Table 30's NOTE), not a level to hand a 16-bit
    /// codec at: sent at full scale the top constellation points sit on
    /// mu-law's 32124 ceiling, clip, and cost six per cent of the bits. The
    /// analogue modem transmits at its own power instead, and the receiver's
    /// `Design::scale` is what folds that back out (which is why it is measured
    /// from the channel estimate and not assumed).
    #[test]
    fn a_mu_law_line_reads_back_through_the_codec() {
        let law = crate::v90::ucode::Law::Mu;
        let (errors, length) = through_the_line(|seen| {
            let (u, negative) =
                crate::v90::ucode::nearest(law, (seen * 32768.0).round().clamp(-32768.0, 32767.0) as i32);
            crate::v90::ucode::amplitude(law, u, negative) as f64 / 32768.0
        });
        assert_eq!(errors, 0, "{errors} of {length} bits wrong through the codec");
    }

    /// [`Channel::estimate`] against a channel it was given: TRN1u's own
    /// levels in, the same channel's output with a little noise, and the taps
    /// come back.
    #[test]
    fn the_estimate_finds_the_channel_it_was_shown() {
        let truth = [0.6, -0.25, 0.1, -0.05];
        let mut seed = 0xfeed_face_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed >> 63 == 0
        };
        // A two-level GPA-like run, as TRN1u is.
        let known: Vec<f64> = (0..600).map(|_| if next() { 1.0 } else { -1.0 }).collect();
        let mut received = vec![0.0; known.len()];
        for n in truth.len()..known.len() {
            let mut sum = 0.0;
            for (k, &tap) in truth.iter().enumerate() {
                sum += tap * known[n - k];
            }
            received[n] = sum;
        }
        let channel = Channel::estimate(&known, &received, 8).expect("a two-level run is enough");
        for (k, &tap) in truth.iter().enumerate() {
            assert!((channel.taps[k] - tap).abs() < 0.05, "tap {k}: {} against {tap}", channel.taps[k]);
        }
        assert!(channel.noise < 1e-6, "noise {}", channel.noise);
        assert!(channel.severity() > 0.0);
        assert_eq!(channel.impulse(16).len(), 16);
    }
}
