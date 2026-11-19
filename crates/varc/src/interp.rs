//! A resampler a timing loop can steer.
//!
//! Two sound cards never agree about what 48 kHz is. Fifty parts in a million
//! between them is ordinary, and at the top carrier of the multicarrier mode
//! that is a seventh of a radian of phase every symbol, against a
//! constellation whose points are a fraction of a degree apart. Rotating each
//! carrier back afterwards mends the phase and leaves the damage: a symbol
//! sampled at the wrong rate is no longer made of whole cycles, and every
//! carrier leaks into every other at about -28 dB.
//!
//! So the receiver resamples what it hears onto the *transmitter's* clock
//! before anything else looks at it, and the loop that measures the remaining
//! timing error moves this.
//!
//! [`dsp::Resampler`] does the same arithmetic for a fixed ratio between a
//! sound card and a telephone-rate modem, and is good to about 80 dB over a
//! band that stops well short of Nyquist. This one has a ratio that is always
//! within a hair of one and has to be good to 100 dB up to seven eighths of
//! Nyquist, so it trades the stretched kernel for a longer one, tabulated.

use std::f64::consts::PI;

/// Taps either side of the point being interpolated.
///
/// The kernel is a sinc cut off at Nyquist under a Kaiser window, and the
/// window is what limits how near Nyquist it stays flat: forty-eight a side
/// with the window below holds to about 0.87 of Nyquist, 20.8 kHz at 48 kHz.
/// Above that the response depends on where between samples the output falls,
/// which a receiver sees as noise on its top carriers and loads them less.
const HALF: usize = 48;

/// Fractional positions tabulated, with straight lines between.
const PHASES: usize = 1024;

/// The Kaiser window's shape: about 100 dB of sidelobe.
const BETA: f64 = 10.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// A fractional-delay resampler with a rate and phase that can be moved
/// while it runs.
#[derive(Clone)]
pub struct Interp {
    /// `PHASES + 1` rows of `2 * HALF` taps.
    table: Vec<f64>,
    /// The last `2 * LEN` inputs twice over, so a run of taps never wraps.
    history: Vec<f64>,
    at: usize,
    /// Input samples per output sample.
    step: f64,
    /// How far behind the newest input the next output falls.
    behind: f64,
}

/// History kept: the kernel and room for the position to wander.
const LEN: usize = 2 * HALF + 8;

impl std::fmt::Debug for Interp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Interp").field("step", &self.step).field("behind", &self.behind).finish_non_exhaustive()
    }
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        let taps = 2 * HALF;
        let mut table = vec![0.0; (PHASES + 1) * taps];
        let norm = bessel_i0(BETA);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let row = &mut table[p * taps..(p + 1) * taps];
            let mut sum = 0.0;
            for (j, tap) in row.iter_mut().enumerate() {
                // Tap j sits at offset j - (HALF - 1) from the sample at or
                // before the output point.
                let x = j as f64 - (HALF as f64 - 1.0) - frac;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (PI * x).sin() / (PI * x) };
                let r = x / HALF as f64;
                let window = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(BETA * (1.0 - r * r).sqrt()) / norm };
                *tap = sinc * window;
                sum += *tap;
            }
            for tap in row.iter_mut() {
                *tap /= sum;
            }
        }
        Self { table, history: vec![0.0; 2 * LEN], at: 0, step: 1.0, behind: HALF as f64 }
    }

    /// The delay through the resampler at a ratio of one, in samples.
    pub fn delay(&self) -> usize {
        HALF
    }

    /// How far the output clock runs from the input clock, in parts per
    /// million: positive when outputs are taken faster than inputs arrive.
    pub fn ppm(&self) -> f64 {
        (1.0 / self.step - 1.0) * 1e6
    }

    /// Change the ratio by `delta` input samples per output sample.
    pub fn adjust_step(&mut self, delta: f64) {
        self.step = (self.step + delta).clamp(0.999, 1.001);
    }

    /// Take every later output `samples` input samples later than it would
    /// have been taken.
    pub fn nudge(&mut self, samples: f64) {
        self.behind -= samples;
    }

    /// One sample in; `out` is called for each sample that comes out, which
    /// is one nearly always and none or two when the clocks slip a place.
    pub fn feed(&mut self, x: f64, mut out: impl FnMut(f64)) {
        self.history[self.at] = x;
        self.history[self.at + LEN] = x;
        self.at = (self.at + 1) % LEN;
        self.behind += 1.0;
        // A nudge can push the point further back than the history reaches;
        // nothing can be done for those outputs but to skip them.
        let limit = (LEN - HALF - 1) as f64;
        if self.behind > limit {
            self.behind = limit;
        }
        let taps = 2 * HALF;
        while self.behind >= HALF as f64 {
            let whole = self.behind.ceil();
            let frac = whole - self.behind;
            // `at` is one past the newest; the sample at or before the output
            // point is `whole` behind the newest.
            let centre = self.at + LEN - 1 - whole as usize;
            let first = centre + 1 - HALF;
            let window = &self.history[first..first + taps];
            let scaled = frac * PHASES as f64;
            let p = (scaled as usize).min(PHASES - 1);
            let between = scaled - p as f64;
            let lower = &self.table[p * taps..(p + 1) * taps];
            let upper = &self.table[(p + 1) * taps..(p + 2) * taps];
            let mut a = 0.0;
            let mut b = 0.0;
            for ((&h, &l), &u) in window.iter().zip(lower).zip(upper) {
                a += h * l;
                b += h * u;
            }
            out(a + between * (b - a));
            self.behind -= self.step;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// Resample a tone and measure what is left after the tone itself is
    /// taken away, in dB below it.
    fn residue(freq: f64, step_ppm: f64, nudge: f64) -> f64 {
        let mut interp = Interp::new();
        interp.adjust_step(step_ppm * 1e-6);
        interp.nudge(nudge);
        let mut out = Vec::new();
        for t in 0..20_000 {
            interp.feed((TAU * freq * t as f64).sin(), |y| out.push(y));
        }
        // Output m is the input at time m * step + constant, so it is a tone
        // at freq * step; fit that and look at the remainder.
        let w = TAU * freq * (1.0 + step_ppm * 1e-6);
        let tail = &out[1000..];
        let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (m, &y) in tail.iter().enumerate() {
            let (s, c) = (w * m as f64).sin_cos();
            ss += s * s;
            sc += s * c;
            cc += c * c;
            ys += y * s;
            yc += y * c;
        }
        let det = ss * cc - sc * sc;
        let a = (ys * cc - yc * sc) / det;
        let b = (yc * ss - ys * sc) / det;
        let mut err = 0.0;
        let mut sig = 0.0;
        for (m, &y) in tail.iter().enumerate() {
            let (s, c) = (w * m as f64).sin_cos();
            let fit = a * s + b * c;
            err += (y - fit) * (y - fit);
            sig += fit * fit;
        }
        10.0 * (err / sig).log10()
    }

    #[test]
    fn a_tone_comes_through_clean_wherever_between_samples_it_is_taken() {
        for &freq in &[0.01, 0.1, 0.25, 0.4] {
            for &nudge in &[0.0, 0.25, 0.5, 0.731] {
                let db = residue(freq, 0.0, nudge);
                assert!(db < -95.0, "{freq} of the rate, {nudge} of a sample: {db:.1} dB");
            }
        }
    }

    #[test]
    fn a_tone_comes_through_clean_while_the_clocks_slide() {
        for &freq in &[0.05, 0.3, 0.42] {
            for &ppm in &[-200.0, 37.0, 150.0] {
                let db = residue(freq, ppm, 0.0);
                assert!(db < -90.0, "{freq} of the rate at {ppm} ppm: {db:.1} dB");
            }
        }
    }

    #[test]
    fn a_nudge_moves_the_output_later_by_what_was_asked() {
        let freq = 0.05;
        let run = |nudge: f64| {
            let mut interp = Interp::new();
            interp.nudge(nudge);
            let mut out = Vec::new();
            for t in 0..2000 {
                interp.feed((TAU * freq * t as f64).sin(), |y| out.push(y));
            }
            out
        };
        let plain = run(0.0);
        let late = run(0.3);
        // Taken 0.3 of a sample later, the tone has moved on by that much.
        let phase = |x: &[f64]| {
            let (mut s, mut c) = (0.0, 0.0);
            for (i, &y) in x[500..1500].iter().enumerate() {
                s += y * (TAU * freq * i as f64).sin();
                c += y * (TAU * freq * i as f64).cos();
            }
            c.atan2(s)
        };
        let moved = (phase(&late) - phase(&plain)) / (TAU * freq);
        assert!((moved - 0.3).abs() < 1e-3, "moved {moved}");
    }
}
