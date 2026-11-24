//! Adaptive equalisation.
//!
//! A real line has amplitude and group-delay distortion, which smears a
//! constellation until it cannot be sliced. Bell 103 tolerates this because a
//! frequency discriminator only cares which of two tones is present, but a
//! sixteen-point constellation does not: this is the first piece of the modem
//! that exists purely because the channel is not ideal, and everything above
//! V.22bis needs it too.

use std::collections::VecDeque;

/// A complex symbol-spaced adaptive filter.
///
/// Adaptation runs in two stages. A blind stage uses the constant-modulus
/// criterion, which needs no knowledge of what was sent and so can open a
/// closed eye; once the constellation is recognisable, a decision-directed
/// stage takes over and converges far more tightly. Starting decision-directed
/// on a closed eye simply reinforces whatever nonsense it first decides.
#[derive(Debug, Clone)]
pub struct Equalizer {
    /// Complex taps as (real, imaginary).
    taps: Vec<(f64, f64)>,
    history: VecDeque<(f64, f64)>,
    blind_step: f64,
    tracking_step: f64,
    /// Dispersion constant for the constant-modulus stage.
    modulus: f64,
    /// Running mean of the decision-directed error, used to decide when the
    /// eye has opened far enough to trust decisions.
    error_average: f64,
    blind: bool,
}

impl Equalizer {
    /// `taps` should be odd so there is a true centre; the centre starts at
    /// unity and everything else at zero, so an unadapted filter passes the
    /// signal through untouched.
    ///
    /// `modulus` is the constant-modulus target, the ratio of the fourth moment
    /// of the constellation to its second. For sixteen-point quadrature
    /// amplitude modulation normalised to unit mean power it is 1.32.
    pub fn new(taps: usize, modulus: f64) -> Self {
        let taps = taps | 1;
        let mut weights = vec![(0.0, 0.0); taps];
        weights[taps / 2] = (1.0, 0.0);
        Self {
            taps: weights,
            history: VecDeque::from(vec![(0.0, 0.0); taps]),
            blind_step: 2.0e-3,
            tracking_step: 4.0e-3,
            modulus,
            error_average: 1.0,
            blind: true,
        }
    }

    pub fn with_steps(mut self, blind: f64, tracking: f64) -> Self {
        self.blind_step = blind;
        self.tracking_step = tracking;
        self
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// Delay through the filter, in symbols: the centre tap's position.
    pub fn delay(&self) -> usize {
        self.taps.len() / 2
    }

    /// True while still adapting blind.
    pub fn is_blind(&self) -> bool {
        self.blind
    }

    /// Mean magnitude of the decision error, as a measure of convergence.
    pub fn error(&self) -> f64 {
        self.error_average
    }

    /// Filter one symbol.
    pub fn equalize(&mut self, x: (f64, f64)) -> (f64, f64) {
        self.history.pop_front();
        self.history.push_back(x);
        let mut out = (0.0, 0.0);
        for (tap, sample) in self.taps.iter().zip(self.history.iter()) {
            // Complex multiply and accumulate.
            out.0 += tap.0 * sample.0 - tap.1 * sample.1;
            out.1 += tap.0 * sample.1 + tap.1 * sample.0;
        }
        out
    }

    /// Largest tap energy tolerated before the filter is considered lost.
    ///
    /// A constant-modulus update grows with the cube of the magnitude, so once
    /// it starts running away it reaches infinity in a few symbols and every
    /// value downstream becomes a quiet NaN. Catching it is far better than
    /// letting a display draw nothing and give no reason.
    const MAX_TAP_ENERGY: f64 = 1.0e4;

    /// Adapt towards `decision`, the constellation point `output` should have
    /// been. Call once per equalised symbol.
    pub fn adapt(&mut self, output: (f64, f64), decision: (f64, f64)) {
        if !output.0.is_finite() || !output.1.is_finite() {
            self.reset();
            return;
        }
        let dd_error = (output.0 - decision.0, output.1 - decision.1);
        let magnitude = (dd_error.0 * dd_error.0 + dd_error.1 * dd_error.1).sqrt();
        self.error_average += 0.01 * (magnitude - self.error_average);

        // Leave the blind stage once decisions look trustworthy. The threshold
        // is well inside the distance between neighbouring points, so a wrong
        // decision is unlikely by the time it trips.
        if self.blind && self.error_average < 0.25 {
            self.blind = false;
        }

        let (error, step) = if self.blind {
            // Godard's constant-modulus error: drive the output magnitude
            // towards a fixed dispersion, ignoring which point was sent.
            let power = output.0 * output.0 + output.1 * output.1;
            let scale = power - self.modulus;
            ((output.0 * scale, output.1 * scale), self.blind_step)
        } else {
            (dd_error, self.tracking_step)
        };

        // Least mean squares: step against the gradient, which is the error
        // times the conjugate of the input.
        for (tap, sample) in self.taps.iter_mut().zip(self.history.iter()) {
            tap.0 -= step * (error.0 * sample.0 + error.1 * sample.1);
            tap.1 -= step * (error.1 * sample.0 - error.0 * sample.1);
        }

        // Start again rather than carry on into infinity.
        let energy: f64 = self.taps.iter().map(|t| t.0 * t.0 + t.1 * t.1).sum();
        if !energy.is_finite() || energy > Self::MAX_TAP_ENERGY {
            self.reset();
        }
    }

    /// Restore the initial centre spike.
    pub fn reset(&mut self) {
        let n = self.taps.len();
        self.taps.fill((0.0, 0.0));
        self.taps[n / 2] = (1.0, 0.0);
        self.history.iter_mut().for_each(|s| *s = (0.0, 0.0));
        self.error_average = 1.0;
        self.blind = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixteen-point constellation, normalised to unit mean power.
    fn points() -> Vec<(f64, f64)> {
        let mut v = Vec::new();
        for i in [-3.0, -1.0, 1.0, 3.0] {
            for q in [-3.0, -1.0, 1.0, 3.0] {
                v.push((i / 10f64.sqrt(), q / 10f64.sqrt()));
            }
        }
        v
    }

    fn nearest(p: (f64, f64), set: &[(f64, f64)]) -> (f64, f64) {
        *set.iter()
            .min_by(|a, b| {
                let da = (a.0 - p.0).powi(2) + (a.1 - p.1).powi(2);
                let db = (b.0 - p.0).powi(2) + (b.1 - p.1).powi(2);
                da.partial_cmp(&db).unwrap()
            })
            .unwrap()
    }

    #[test]
    fn an_unadapted_filter_passes_the_signal_through_after_its_delay() {
        // A centre spike is still a filter: it delays by half its length.
        let mut eq = Equalizer::new(11, 1.32);
        assert_eq!(eq.equalize((0.4, -0.7)), (0.0, 0.0), "output starts empty");
        for _ in 0..eq.delay() - 1 {
            eq.equalize((0.0, 0.0));
        }
        assert_eq!(eq.equalize((0.0, 0.0)), (0.4, -0.7), "the sample should emerge");
    }

    #[test]
    fn the_modulus_constant_matches_the_constellation() {
        // The constant-modulus target is the fourth moment over the second.
        let set = points();
        let second: f64 = set.iter().map(|p| p.0 * p.0 + p.1 * p.1).sum::<f64>() / 16.0;
        let fourth: f64 = set
            .iter()
            .map(|p| (p.0 * p.0 + p.1 * p.1).powi(2))
            .sum::<f64>()
            / 16.0;
        assert!((second - 1.0).abs() < 1e-12, "not unit power: {second}");
        assert!((fourth / second - 1.32).abs() < 0.01, "modulus {}", fourth / second);
    }

    /// The point of the whole thing: a channel that smears the constellation
    /// should be undone.
    #[test]
    fn a_distorting_channel_is_equalised() {
        let set = points();
        let mut eq = Equalizer::new(21, 1.32);
        // A two-path channel: a strong direct term plus a delayed echo, which
        // is what group-delay distortion looks like symbol to symbol.
        let mut delayed = [(0.0, 0.0); 2];
        let mut seed = 12345u64;
        let mut rand = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as usize
        };

        let mut before = 0.0;
        let mut after = 0.0;
        let total = 20_000;
        for n in 0..total {
            let sent = set[rand() % 16];
            // channel: y[n] = x[n] + 0.45 x[n-1] - 0.2 x[n-2]
            let y = (
                sent.0 + 0.45 * delayed[0].0 - 0.2 * delayed[1].0,
                sent.1 + 0.45 * delayed[0].1 - 0.2 * delayed[1].1,
            );
            delayed[1] = delayed[0];
            delayed[0] = sent;

            let out = eq.equalize(y);
            let decision = nearest(out, &set);
            eq.adapt(out, decision);

            let distance = ((out.0 - decision.0).powi(2) + (out.1 - decision.1).powi(2)).sqrt();
            if n < 500 {
                before += distance / 500.0;
            }
            if n >= total - 500 {
                after += distance / 500.0;
            }
        }
        assert!(
            after < before / 3.0,
            "equaliser did not converge: {before:.4} at the start, {after:.4} at the end"
        );
        assert!(after < 0.12, "residual error {after:.4} is too large to slice");
        assert!(!eq.is_blind(), "should have handed over to decision direction");
    }

    #[test]
    fn a_clean_channel_is_left_alone() {
        let set = points();
        let mut eq = Equalizer::new(11, 1.32);
        let mut seed = 999u64;
        let mut rand = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as usize
        };
        // Compare against the symbol the filter's own delay is presenting, not
        // the one just fed in.
        let delay = eq.delay();
        let mut history: Vec<(f64, f64)> = Vec::new();
        let mut worst: f64 = 0.0;
        for n in 0..8000 {
            let sent = set[rand() % 16];
            history.push(sent);
            let out = eq.equalize(sent);
            let decision = nearest(out, &set);
            eq.adapt(out, decision);
            if n >= delay {
                let expected = history[n - delay];
                worst = worst.max((out.0 - expected.0).abs().max((out.1 - expected.1).abs()));
            }
        }
        assert!(worst < 0.2, "an undistorted signal was disturbed by {worst}");
    }

    #[test]
    fn resetting_restores_the_centre_spike() {
        let mut eq = Equalizer::new(9, 1.32);
        for _ in 0..200 {
            let out = eq.equalize((0.9, 0.2));
            eq.adapt(out, (1.0, 0.0));
        }
        eq.reset();
        assert!(eq.is_blind());
        eq.equalize((0.3, 0.5));
        for _ in 0..eq.delay() - 1 {
            eq.equalize((0.0, 0.0));
        }
        assert_eq!(eq.equalize((0.0, 0.0)), (0.3, 0.5));
    }
}
