//! Sample rate conversion, for getting between a sound card and a modem.
//!
//! A modem runs at a rate that suits the modulation; a sound card runs at
//! whatever it runs at, usually 48 kHz and not negotiable. Something has to sit
//! between them, and it cannot be a simple pick-every-third-sample: throwing
//! samples away folds everything above the new Nyquist back down into the band,
//! and what folds down onto a 2400 Hz carrier is indistinguishable from the
//! carrier once it has.
//!
//! The interpolation is a windowed sinc evaluated at whatever fractional
//! position is wanted. Going down in rate the kernel is stretched, which both
//! interpolates and low-passes in one operation: a sinc whose zero crossings
//! are spaced by the *output* sample period is exactly the anti-alias filter
//! the output rate calls for.
//!
//! Streaming, like everything here. A sample goes in and zero, one or several
//! come out, because the two rates are not in any tidy ratio and the number
//! that fall inside one input period is not constant.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Half-width of the interpolation kernel, in output-rate zero crossings.
///
/// Sixteen either side is around 80 dB of stopband with the window below,
/// which is far more than a telephone line will ever justify but costs
/// microseconds and removes a whole category of doubt.
const HALF: usize = 16;

/// Arbitrary-ratio sample rate conversion.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Spacing of the kernel's zero crossings, in input samples. One when
    /// going up in rate, more than one when coming down, which is what makes
    /// the same kernel the anti-alias filter as well.
    spacing: f64,
    /// How far the kernel reaches either side of its centre, in input samples.
    reach: f64,
    history: VecDeque<f64>,
    span: usize,
    /// Where the next output falls, as input samples *before* the newest
    /// sample in the history. It grows by one with every input and shrinks by
    /// `step` with every output, and an output can only be taken once it has
    /// grown past `reach`, because until then the kernel would hang off the
    /// end of what has arrived.
    behind: f64,
}

impl Resampler {
    pub fn new(from_hz: f64, to_hz: f64) -> Self {
        let step = from_hz / to_hz;
        // Going down in rate the kernel has to be stretched to the output
        // period, since that is where the band edge now is. Going up it stays
        // at the input period: there is nothing above the old Nyquist to
        // remove, and stretching would throw away signal that is there.
        let spacing = step.max(1.0);
        let reach = HALF as f64 * spacing;
        // Room for the kernel either side of its centre, and one spare.
        let span = (2.0 * reach).ceil() as usize + 2;
        Self {
            step,
            spacing,
            reach,
            history: VecDeque::from(vec![0.0; span]),
            span,
            behind: 0.0,
        }
    }

    /// Output samples produced per input sample, on average.
    pub fn ratio(&self) -> f64 {
        1.0 / self.step
    }

    /// Feed one input sample, appending whatever output it completes.
    pub fn process(&mut self, x: f64, out: &mut Vec<f64>) {
        self.history.pop_front();
        self.history.push_back(x);
        self.behind += 1.0;
        while self.behind >= self.reach {
            // The newest sample sits at the end of the history, so an output
            // `behind` samples before it sits that far back from the end.
            let centre = (self.span - 1) as f64 - self.behind;
            out.push(self.at(centre));
            self.behind -= self.step;
        }
    }

    /// Interpolate at `centre`, measured in samples from the oldest in the
    /// history. The kernel is entirely inside it, which is what `behind`
    /// waiting for `reach` guarantees.
    fn at(&self, centre: f64) -> f64 {
        let first = (centre - self.reach).ceil().max(0.0) as usize;
        let last = ((centre + self.reach).floor().max(0.0) as usize).min(self.span - 1);

        let mut sum = 0.0;
        let mut weight = 0.0;
        for i in first..=last {
            let t = (i as f64 - centre) / self.spacing;
            let w = sinc(t) * window(t);
            sum += w * self.history[i];
            weight += w;
        }
        // Normalise by the weight actually used, which holds the gain at unity
        // whatever fraction of a sample the output falls at. Without it a
        // steady input comes out with a ripple at the beat between the two
        // rates, which is audible and which an equaliser downstream would
        // waste taps trying to undo.
        if weight.abs() < 1.0e-12 {
            0.0
        } else {
            sum / weight
        }
    }
}

fn sinc(t: f64) -> f64 {
    if t.abs() < 1.0e-9 {
        1.0
    } else {
        (PI * t).sin() / (PI * t)
    }
}

/// Blackman window over the kernel's support.
fn window(t: f64) -> f64 {
    let x = t.abs() / HALF as f64;
    if x >= 1.0 {
        return 0.0;
    }
    let a = PI * (1.0 - x);
    0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// Amplitude at one frequency, over a run of samples at `fs`.
    fn tone(samples: &[f64], fs: f64, freq: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &s) in samples.iter().enumerate() {
            let w = TAU * freq * n as f64 / fs;
            re += s * w.cos();
            im -= s * w.sin();
        }
        2.0 * (re * re + im * im).sqrt() / samples.len() as f64
    }

    fn run(from: f64, to: f64, freq: f64, n: usize) -> Vec<f64> {
        let mut r = Resampler::new(from, to);
        let mut out = Vec::new();
        for i in 0..n {
            r.process((TAU * freq * i as f64 / from).sin(), &mut out);
        }
        out
    }

    #[test]
    fn a_tone_keeps_its_frequency_and_its_level() {
        // Both directions, and the awkward ratios a real sound card offers.
        for (from, to) in [
            (48_000.0, 16_000.0),
            (16_000.0, 48_000.0),
            (44_100.0, 16_000.0),
            (16_000.0, 44_100.0),
            (48_000.0, 8_000.0),
        ] {
            let out = run(from, to, 1_200.0, 48_000);
            // Skip the start, where the kernel is still filling.
            let settled = &out[out.len() / 4..];
            let level = tone(settled, to, 1_200.0);
            assert!(
                (level - 1.0).abs() < 0.05,
                "{from} to {to}: 1200 Hz came out at {level:.3}"
            );
        }
    }

    #[test]
    fn the_output_arrives_at_the_rate_asked_for() {
        for (from, to) in [(48_000.0, 16_000.0), (16_000.0, 48_000.0), (44_100.0, 16_000.0)] {
            let out = run(from, to, 1_000.0, 48_000);
            let want = 48_000.0 * to / from;
            let error = (out.len() as f64 - want).abs() / want;
            assert!(
                error < 0.01,
                "{from} to {to}: produced {} samples against {want:.0}",
                out.len()
            );
        }
    }

    #[test]
    fn what_is_above_the_new_nyquist_is_removed_rather_than_folded() {
        // The whole reason this is not a matter of picking every third sample.
        // At 48 kHz into 16 kHz, a tone at 10 kHz has nowhere legitimate to go,
        // and a decimator that ignored it would put it at 6 kHz. Worse cases
        // land inside the modem's own band, where nothing can tell them from
        // signal.
        let out = run(48_000.0, 16_000.0, 10_000.0, 48_000);
        let settled = &out[out.len() / 4..];
        let folded = tone(settled, 16_000.0, 6_000.0);
        let anywhere = (1..80)
            .map(|k| tone(settled, 16_000.0, f64::from(k) * 100.0))
            .fold(0.0f64, f64::max);
        assert!(
            folded < 0.01,
            "10 kHz folded down to 6 kHz at a level of {folded:.4}"
        );
        assert!(
            anywhere < 0.01,
            "10 kHz reappeared somewhere in the band at {anywhere:.4}"
        );
    }

    #[test]
    fn a_signal_survives_the_round_trip() {
        // Up to a sound card's rate and back, which is what a modem's own
        // signal goes through on its way to the line and back from it.
        let mut up = Resampler::new(16_000.0, 48_000.0);
        let mut down = Resampler::new(48_000.0, 16_000.0);
        let mut middle = Vec::new();
        let mut back = Vec::new();
        for i in 0..16_000 {
            let x = (TAU * 1_800.0 * i as f64 / 16_000.0).sin();
            middle.clear();
            up.process(x, &mut middle);
            for &m in &middle {
                down.process(m, &mut back);
            }
        }
        let settled = &back[back.len() / 4..];
        let level = tone(settled, 16_000.0, 1_800.0);
        assert!(
            (level - 1.0).abs() < 0.05,
            "1800 Hz came back at {level:.3} after a round trip"
        );
    }

    #[test]
    fn the_whole_telephone_band_comes_through_flat() {
        // A modem's signal is not one tone. V.32 fills 600 to 3000 Hz and the
        // receiver's equaliser will try to undo any tilt put there here,
        // spending taps on a fault that need not exist.
        for freq in [300.0, 600.0, 1_200.0, 1_800.0, 2_400.0, 3_000.0, 3_400.0] {
            let out = run(48_000.0, 16_000.0, freq, 48_000);
            let settled = &out[out.len() / 4..];
            let level = tone(settled, 16_000.0, freq);
            assert!(
                (level - 1.0).abs() < 0.05,
                "{freq} Hz came out at {level:.3}"
            );
        }
    }
}
