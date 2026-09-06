//! Pulse shaping and symbol timing.
//!
//! V.22bis 2.4 asks for "the square root of a raised cosine shaping with 75%
//! roll-off". Splitting the raised cosine between transmitter and receiver puts
//! a matched filter at each end: the pair multiply to a full raised cosine,
//! which is free of intersymbol interference at the sampling instants, and the
//! receiver gets the best noise rejection available.

use std::f64::consts::PI;

/// Root-raised-cosine taps, `span` symbols long at `sps` samples per symbol.
///
/// Normalised to unit energy so the filter neither amplifies nor attenuates.
pub fn rrc_taps(sps: f64, rolloff: f64, span: usize) -> Vec<f64> {
    let len = (span as f64 * sps).round() as usize | 1; // odd, so there is a centre tap
    let mid = (len / 2) as f64;
    let mut taps = Vec::with_capacity(len);
    for i in 0..len {
        let t = (i as f64 - mid) / sps;
        taps.push(rrc_at(t, rolloff));
    }
    let energy: f64 = taps.iter().map(|x| x * x).sum::<f64>().sqrt();
    for t in &mut taps {
        *t /= energy;
    }
    taps
}

/// The root-raised-cosine impulse response at `t` symbol periods from centre.
///
/// Public because a transmitter working at a sample rate that is not a whole
/// multiple of the symbol rate has to evaluate the pulse at arbitrary offsets
/// rather than index a fixed tap table. 16 kHz against 600 baud is exactly that
/// case, at 26.67 samples per symbol.
pub fn rrc_at(t: f64, beta: f64) -> f64 {
    // Both closed-form singularities are removed by their limits.
    if t.abs() < 1e-9 {
        return 1.0 + beta * (4.0 / PI - 1.0);
    }
    if beta > 0.0 {
        let edge = 1.0 / (4.0 * beta);
        if (t.abs() - edge).abs() < 1e-9 {
            let a = (1.0 + 2.0 / PI) * (PI / (4.0 * beta)).sin();
            let b = (1.0 - 2.0 / PI) * (PI / (4.0 * beta)).cos();
            return beta / 2f64.sqrt() * (a + b);
        }
    }
    let num = (PI * t * (1.0 - beta)).sin()
        + 4.0 * beta * t * (PI * t * (1.0 + beta)).cos();
    let den = PI * t * (1.0 - (4.0 * beta * t).powi(2));
    num / den
}

/// Linear-phase low-pass taps: a windowed sinc of `taps` length.
///
/// Linear phase is the point. A Butterworth of the same sharpness delays
/// different frequencies by different amounts, and that group-delay distortion
/// smears a pulse worse than the adjacent channel it was meant to remove. A
/// symmetric finite impulse response delays every frequency equally, so it can
/// be made as steep as wanted and still leave the pulse shape intact.
///
/// A Hamming window gives about 53 dB of stopband rejection with a transition
/// roughly `3.3 * fs / taps` wide.
pub fn fir_lowpass(cutoff: f64, taps: usize, fs: f64) -> Vec<f64> {
    let n = taps | 1; // odd, for a true centre and exact linear phase
    let mid = (n / 2) as f64;
    let omega = 2.0 * PI * cutoff / fs;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 - mid;
        // sinc, with its removable singularity at the centre.
        let ideal = if t.abs() < 1e-9 {
            omega / PI
        } else {
            (omega * t).sin() / (PI * t)
        };
        let window = 0.54 - 0.46 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
        out.push(ideal * window);
    }
    // Unit gain at zero frequency, so the filter neither lifts nor drops the level.
    let sum: f64 = out.iter().sum();
    for tap in &mut out {
        *tap /= sum;
    }
    out
}

/// Zeroth-order modified Bessel function of the first kind.
///
/// The Kaiser window is defined in terms of it. The series converges quickly
/// for the arguments a window design produces, and the loop stops when a term
/// stops changing the sum.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        let ratio = half / f64::from(k);
        term *= ratio * ratio;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// A low-pass designed from what it has to do, rather than from a tap count.
///
/// `pass` is the highest frequency to keep, `stop` the lowest to reject, and
/// `stopband_db` how far down everything above `stop` has to be. How many taps
/// that costs follows from the three of them, by Kaiser's formulas, and the
/// caller does not get to choose it -- which is the point. A filter asked for
/// by tap count is a filter whose rejection nobody has checked.
///
/// The window matters more than the length here. A Hamming window has a
/// stopband floor of about 53 dB and stays there however many taps it is given,
/// so a design that needs 80 cannot be had by making a Hamming filter longer.
/// Kaiser's has a parameter for exactly this: ask for the depth, and pay for it
/// in taps.
pub fn fir_lowpass_kaiser(pass: f64, stop: f64, stopband_db: f64, fs: f64) -> Vec<f64> {
    let transition = ((stop - pass) / fs).clamp(1e-5, 0.5);
    // Below 21 dB the window is rectangular and the formulas do not apply.
    let a = stopband_db.max(21.0);
    let beta = if a > 50.0 {
        0.1102 * (a - 8.7)
    } else {
        0.5842 * (a - 21.0).powf(0.4) + 0.07886 * (a - 21.0)
    };
    let n = ((a - 8.0) / (2.285 * (2.0 * PI * transition))).ceil().max(3.0) as usize | 1;

    // Halfway between the two edges, which is where a windowed sinc sits 6 dB
    // down: the passband edge and the stopband edge come out either side of it.
    let cutoff = (pass + stop) / 2.0;
    let mid = (n / 2) as f64;
    let omega = 2.0 * PI * cutoff / fs;
    let denominator = bessel_i0(beta);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 - mid;
        let ideal = if t.abs() < 1e-9 {
            omega / PI
        } else {
            (omega * t).sin() / (PI * t)
        };
        let r = t / mid;
        let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denominator;
        out.push(ideal * window);
    }
    let sum: f64 = out.iter().sum();
    for tap in &mut out {
        *tap /= sum;
    }
    out
}

/// A real finite impulse response filter with a sliding history.
#[derive(Debug, Clone)]
pub struct Fir {
    taps: Vec<f64>,
    history: Vec<f64>,
    pos: usize,
}

impl Fir {
    pub fn new(taps: Vec<f64>) -> Self {
        let n = taps.len();
        Self { taps, history: vec![0.0; n], pos: 0 }
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }

    /// Group delay in samples: half the filter length.
    pub fn delay(&self) -> usize {
        self.taps.len() / 2
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let n = self.history.len();
        self.history[self.pos] = x;
        self.pos = (self.pos + 1) % n;
        // Walk the history newest-first against the taps.
        let mut acc = 0.0;
        let mut idx = self.pos;
        for &tap in self.taps.iter().rev() {
            acc += tap * self.history[idx];
            idx = (idx + 1) % n;
        }
        acc
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.pos = 0;
    }
}

/// The same filter applied to both halves of a complex signal.
#[derive(Debug, Clone)]
pub struct ComplexFir {
    re: Fir,
    im: Fir,
}

impl ComplexFir {
    pub fn new(taps: Vec<f64>) -> Self {
        Self { re: Fir::new(taps.clone()), im: Fir::new(taps) }
    }

    #[inline]
    pub fn process(&mut self, x: (f64, f64)) -> (f64, f64) {
        (self.re.process(x.0), self.im.process(x.1))
    }

    pub fn delay(&self) -> usize {
        self.re.delay()
    }

    pub fn reset(&mut self) {
        self.re.reset();
        self.im.reset();
    }
}

/// Gardner symbol timing recovery.
///
/// Chosen over an early-late gate because its error estimate does not depend on
/// carrier phase, so timing can be recovered before the carrier loop has locked.
/// It needs two samples per symbol, taking one at the symbol instant and one
/// halfway between.
#[derive(Debug, Clone)]
pub struct Gardner {
    /// Samples per symbol, which need not be a whole number.
    sps: f64,
    /// Correction added to the next half-symbol interval.
    phase: f64,
    gain: f64,
    /// Accumulated correction, which learns a difference between the two
    /// clocks rather than merely reacting to the present error.
    integral: f64,
    integral_gain: f64,
    /// Previous symbol and the midpoint before it.
    previous: (f64, f64),
    midpoint: (f64, f64),
    /// Toggles between midpoint and symbol.
    at_symbol: bool,
    last_error: f64,
    /// Running mean symbol power, used to normalise the error.
    mean_power: f64,
    adapting: bool,
}

impl Gardner {
    /// `gain` is in samples of correction per unit of normalised error, and
    /// governs how quickly the loop finds the symbol instant from a standing
    /// start. It has to be large enough to cross half a symbol during
    /// acquisition: too small a value leaves the loop sampling wherever the
    /// group delay of the preceding filters happened to put it, which looks
    /// like working whenever that guess is lucky.
    pub fn new(sps: f64, gain: f64) -> Self {
        Self {
            sps,
            phase: 0.0,
            gain,
            integral: 0.0,
            // A hundredth of the proportional gain: slow enough that it plays
            // no part in acquisition, and only settles afterwards on whatever
            // standing offset is left.
            integral_gain: gain / 100.0,
            previous: (0.0, 0.0),
            midpoint: (0.0, 0.0),
            at_symbol: true,
            last_error: 0.0,
            mean_power: 1.0,
            adapting: true,
        }
    }

    /// Interval to the next sample, in samples.
    pub fn interval(&self) -> f64 {
        self.sps / 2.0 + self.phase
    }

    /// Whether the loop is being corrected.
    ///
    /// Held still, it goes on producing symbols at the interval it had found
    /// and simply stops looking for a better one. That is what a receiver
    /// wants while the far end is silent: the only thing on the line then is
    /// its own transmission, and a timing loop that locks onto its own echo
    /// has found a clock that is real, is not the one it needs, and will not
    /// be given up easily afterwards.
    pub fn set_adapting(&mut self, adapting: bool) {
        self.adapting = adapting;
    }

    /// Offer a sample taken at the interval this returned last time.
    ///
    /// Yields a symbol on every second call, once at the symbol instant.
    pub fn feed(&mut self, sample: (f64, f64)) -> Option<(f64, f64)> {
        self.at_symbol = !self.at_symbol;
        if !self.at_symbol {
            self.midpoint = sample;
            return None;
        }

        // Gardner's error: the midpoint should sit where the two symbols cross,
        // so it correlates with the change between them when timing is off.
        let error = self.midpoint.0 * (sample.0 - self.previous.0)
            + self.midpoint.1 * (sample.1 - self.previous.1);

        // Normalise against a *running* mean power rather than this symbol's
        // own. Gardner's detector assumes a constant modulus; a sixteen-point
        // constellation has magnitudes spanning three to one, so dividing by
        // the instantaneous power turns an ordinary amplitude change into a
        // huge apparent timing error and the loop thrashes. Clamping keeps a
        // single outlier from throwing the sampling instant across a symbol.
        if !self.adapting {
            // Still keep the symbols coming and the history moving; only the
            // correction stops.
            self.previous = sample;
            return Some(sample);
        }

        let power = sample.0 * sample.0 + sample.1 * sample.1;
        self.mean_power += 0.02 * (power - self.mean_power);
        self.last_error = (error / (self.mean_power + 1e-9)).clamp(-1.0, 1.0);

        // Two terms, because there are two things to correct. Shortening or
        // lengthening the interval moves every later instant, so the
        // proportional term alone already removes a standing offset in the
        // sampling phase. What it cannot remove is a difference in clock rate:
        // against that it settles at whatever error is needed to hold the
        // correction, and once that error costs more than half a symbol the
        // receiver slips one. The integral holds the correction on its own, so
        // the error it is holding can go to nothing.
        self.integral = (self.integral - self.integral_gain * self.last_error)
            .clamp(-self.sps / 8.0, self.sps / 8.0);
        self.phase =
            (-self.gain * self.last_error + self.integral).clamp(-self.sps / 4.0, self.sps / 4.0);
        self.previous = sample;
        Some(sample)
    }

    pub fn error(&self) -> f64 {
        self.last_error
    }
}

#[cfg(test)]
mod tests {

    /// Magnitude response of a tap set at one frequency.
    fn response(taps: &[f64], freq: f64, fs: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &t) in taps.iter().enumerate() {
            let w = 2.0 * PI * freq * i as f64 / fs;
            re += t * w.cos();
            im -= t * w.sin();
        }
        (re * re + im * im).sqrt()
    }

    #[test]
    fn a_kaiser_design_keeps_what_it_was_told_to_keep() {
        let fs = 16_000.0;
        let taps = fir_lowpass_kaiser(525.0, 675.0, 80.0, fs);
        for f in [0.0, 100.0, 300.0, 500.0, 525.0] {
            let db = 20.0 * response(&taps, f, fs).log10();
            assert!(db > -1.0, "passband sags {db:.2} dB at {f} Hz");
        }
    }

    #[test]
    fn a_kaiser_design_rejects_by_as_much_as_it_was_asked_for() {
        // The property a windowed sinc cannot be talked into by length alone: a
        // Hamming window bottoms out near 53 dB however many taps it is given,
        // so a design needing eighty has to change window rather than grow.
        let fs = 16_000.0;
        let taps = fir_lowpass_kaiser(525.0, 675.0, 80.0, fs);
        for f in [675.0, 800.0, 1200.0, 1725.0, 3000.0] {
            let db = 20.0 * response(&taps, f, fs).log10();
            assert!(db < -78.0, "stopband only {db:.1} dB down at {f} Hz");
        }
    }

    #[test]
    fn asking_for_more_rejection_costs_taps_and_nothing_else() {
        let fs = 16_000.0;
        let cheap = fir_lowpass_kaiser(525.0, 675.0, 40.0, fs);
        let dear = fir_lowpass_kaiser(525.0, 675.0, 90.0, fs);
        assert!(
            dear.len() > cheap.len(),
            "ninety decibels came out no longer than forty"
        );
        // Both odd, so both have a true centre tap and exact linear phase.
        assert_eq!(cheap.len() % 2, 1);
        assert_eq!(dear.len() % 2, 1);
    }

    #[test]
    fn a_narrower_transition_costs_taps_too() {
        let fs = 16_000.0;
        let wide = fir_lowpass_kaiser(500.0, 1500.0, 80.0, fs);
        let narrow = fir_lowpass_kaiser(500.0, 600.0, 80.0, fs);
        assert!(narrow.len() > wide.len() * 4, "a tenth the transition cost too little");
    }

    #[test]
    fn a_kaiser_design_passes_direct_current_untouched() {
        // Normalised to unity at zero, so putting one in the signal path
        // neither lifts nor drops the level.
        let taps = fir_lowpass_kaiser(525.0, 675.0, 80.0, 16_000.0);
        assert!((taps.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_window_is_symmetric_so_the_phase_is_linear() {
        // The whole reason a finite impulse response is used here rather than a
        // Butterworth: every frequency is delayed by the same amount, so the
        // pulse arrives with its shape intact and the matched pair still meets
        // the Nyquist criterion.
        let taps = fir_lowpass_kaiser(525.0, 675.0, 80.0, 16_000.0);
        for (a, b) in taps.iter().zip(taps.iter().rev()) {
            assert!((a - b).abs() < 1e-15, "not symmetric about its centre");
        }
    }
    use super::*;

    #[test]
    fn rrc_taps_have_unit_energy() {
        let taps = rrc_taps(16.0, 0.75, 8);
        let energy: f64 = taps.iter().map(|x| x * x).sum();
        assert!((energy - 1.0).abs() < 1e-12, "energy {energy}");
    }

    #[test]
    fn rrc_is_symmetric_with_a_central_peak() {
        let taps = rrc_taps(16.0, 0.75, 8);
        let n = taps.len();
        assert_eq!(n % 2, 1, "an odd length gives a true centre tap");
        for i in 0..n / 2 {
            assert!(
                (taps[i] - taps[n - 1 - i]).abs() < 1e-12,
                "asymmetric at {i}"
            );
        }
        let peak = taps.iter().cloned().fold(f64::MIN, f64::max);
        assert!((taps[n / 2] - peak).abs() < 1e-12, "peak should be central");
    }

    #[test]
    fn the_singularities_are_finite() {
        // The closed form divides by zero at t=0 and t=1/(4*beta); both are
        // replaced by their limits.
        for beta in [0.25, 0.35, 0.5, 0.75, 1.0] {
            assert!(rrc_at(0.0, beta).is_finite(), "t=0, beta={beta}");
            let edge = 1.0 / (4.0 * beta);
            assert!(rrc_at(edge, beta).is_finite(), "t=edge, beta={beta}");
            assert!(rrc_at(-edge, beta).is_finite());
        }
    }

    /// Two root-raised-cosine filters in series make a raised cosine, which is
    /// zero at every symbol instant but the centre. That is the property the
    /// whole scheme rests on.
    #[test]
    fn a_matched_pair_has_no_intersymbol_interference() {
        let sps = 8usize;
        let taps = rrc_taps(sps as f64, 0.75, 10);
        // Convolve the filter with itself.
        let n = taps.len();
        let mut full = vec![0.0; 2 * n - 1];
        for (i, a) in taps.iter().enumerate() {
            for (j, b) in taps.iter().enumerate() {
                full[i + j] += a * b;
            }
        }
        let centre = n - 1;
        let peak = full[centre];
        assert!(peak > 0.0);
        for k in 1..=4 {
            let at = full[centre + k * sps].abs() / peak;
            assert!(at < 0.02, "symbol {k} away carries {at} of the peak");
        }
    }

    /// A raised cosine: the pulse a matched pair of root-raised-cosines makes,
    /// and the one whose samples are free of intersymbol interference.
    fn raised_cosine(t: f64, beta: f64) -> f64 {
        let sinc = if t.abs() < 1e-12 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        };
        let denominator = 1.0 - (2.0 * beta * t).powi(2);
        if denominator.abs() < 1e-9 {
            // Removable singularity at t = 1/2beta.
            return sinc * PI / 4.0;
        }
        sinc * (PI * beta * t).cos() / denominator
    }

    /// Sample a pulse train through a Gardner loop, returning what it took at
    /// each symbol instant. `offset` displaces the start, in symbols.
    fn acquire(gain: f64, offset: f64, count: usize) -> Vec<f64> {
        let sps = 16.0;
        let beta = 0.75;
        // A repeating but not trivially periodic pattern, so the loop sees
        // transitions to work from without the sequence helping it.
        let symbols: Vec<f64> = (0..count + 16)
            .map(|i: usize| if (i * 7 + i / 3).is_multiple_of(2) { 1.0 } else { -1.0 })
            .collect();
        let at = |t: f64| -> f64 {
            symbols
                .iter()
                .enumerate()
                .map(|(k, &a)| a * raised_cosine(t / sps - k as f64, beta))
                .sum()
        };

        let mut gardner = Gardner::new(sps, gain);
        let mut taken = Vec::new();
        // Start eight symbols in so the train is established, plus the offset
        // under test.
        let mut t = 8.0 * sps + offset * sps;
        while taken.len() < count {
            let next = gardner.interval();
            if let Some(s) = gardner.feed((at(t), 0.0)) {
                taken.push(s.0);
            }
            t += next;
        }
        taken
    }

    #[test]
    fn the_timing_loop_finds_the_symbol_instant_from_any_phase() {
        // Half a symbol out is the worst case, and the one a receiver lands in
        // whenever the filters ahead of it happen to delay by an odd multiple
        // of half a symbol period. A loop that cannot cross that distance is
        // not recovering timing at all; it is trusting its group delay.
        for &offset in &[0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9] {
            let taken = acquire(0.1, offset, 600);
            let settled = &taken[400..];
            let worst = settled
                .iter()
                .map(|s| (1.0 - s.abs()).abs())
                .fold(0.0f64, f64::max);
            assert!(
                worst < 0.2,
                "started {offset} of a symbol out and settled {worst} from the peak"
            );
        }
    }

    #[test]
    fn the_timing_loop_travels_far_enough_to_acquire() {
        // What went wrong before was quantitative, so state the quantity. At
        // 0.005 the correction could move the instant by five thousandths of a
        // sample per symbol, which over an entire call never covers the eight
        // samples of a half symbol at sixteen per symbol.
        let sps = 16.0;
        let mut gardner = Gardner::new(sps, 0.1);
        // Hold the detector at full-scale error and see how far the instant
        // moves. Alternating symbols give a change of two either way, and a
        // midpoint of the same sign as that change keeps the error positive
        // throughout instead of averaging itself away.
        let mut travelled = 0.0;
        let mut sign = 1.0;
        for _ in 0..200 {
            sign = -sign;
            gardner.feed((sign, 0.0));
            gardner.feed((sign, 0.0));
            // Two half-symbol intervals make up each symbol.
            travelled += (gardner.interval() - sps / 2.0).abs() * 2.0;
        }
        assert!(
            travelled > sps / 2.0,
            "two hundred symbols moved the instant {travelled} samples,              short of the {} needed to cross half a symbol",
            sps / 2.0
        );
    }

    #[test]
    fn a_linear_phase_lowpass_passes_its_band_and_rejects_beyond() {
        let fs = 16_000.0;
        // The configuration the V.22bis receiver actually uses, so this test
        // guards the choice rather than a filter nothing builds.
        let taps = fir_lowpass(600.0, 401, fs);
        let response = |freq: f64| {
            let mut f = Fir::new(taps.clone());
            let n = 4000;
            let mut peak: f64 = 0.0;
            for i in 0..n {
                // Cosine, so direct current is a constant rather than
                // identically zero.
                let y = f.process((2.0 * PI * freq * i as f64 / fs).cos());
                if i > n / 2 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        // Flat across the signal band, which is what keeps the pulse intact.
        // A 75 per cent roll-off at 600 baud reaches exactly 525 Hz, so the
        // edge has to pass unweakened, not merely nearly so.
        for hz in [0.0, 200.0, 400.0, 525.0] {
            let g = response(hz);
            assert!((g - 1.0).abs() < 0.02, "{hz} Hz passed at {g}");
        }
        // The other direction occupies 675 Hz upwards once downconverted, its
        // lowest edge abutting our highest. Rejection has to start there, not
        // somewhere convenient above it.
        for hz in [675.0, 800.0, 1000.0, 1200.0, 1500.0] {
            let db = 20.0 * response(hz).log10();
            assert!(db < -50.0, "{hz} Hz only rejected by {db:.1} dB");
        }
    }

    #[test]
    fn a_symmetric_filter_has_linear_phase() {
        // Symmetry is what linear phase means, and it is what a Butterworth
        // cannot offer at any order.
        let taps = fir_lowpass(600.0, 101, 16_000.0);
        assert_eq!(taps.len() % 2, 1);
        for i in 0..taps.len() / 2 {
            let (a, b) = (taps[i], taps[taps.len() - 1 - i]);
            assert!((a - b).abs() < 1e-12, "asymmetric at {i}: {a} against {b}");
        }
    }

    #[test]
    fn the_filter_passes_a_constant_through() {
        let mut f = Fir::new(vec![0.25; 4]);
        for _ in 0..8 {
            f.process(1.0);
        }
        assert!((f.process(1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn the_filter_reproduces_a_known_convolution() {
        let mut f = Fir::new(vec![1.0, 2.0, 3.0]);
        // Impulse in, taps out, newest tap first.
        assert_eq!(f.process(1.0), 1.0);
        assert_eq!(f.process(0.0), 2.0);
        assert_eq!(f.process(0.0), 3.0);
        assert_eq!(f.process(0.0), 0.0);
    }

    /// Gardner should pull the sampling instant onto the symbol centre from a
    /// deliberate offset.
    #[test]
    fn timing_recovery_converges() {
        let sps = 8.0;
        let symbols: Vec<f64> = (0..400)
            .map(|i| if (i * 7 + 3) % 5 < 2 { 1.0 } else { -1.0 })
            .collect();
        // A shaped waveform, sampled with an offset the loop has to remove.
        let taps = rrc_taps(sps, 0.75, 8);
        let mut shaped = Vec::new();
        let mut fir = Fir::new(taps);
        for &s in &symbols {
            shaped.push(fir.process(s));
            for _ in 1..sps as usize {
                shaped.push(fir.process(0.0));
            }
        }

        let mut g = Gardner::new(sps, 0.05);
        let mut position = 3.0f64; // deliberately off the symbol centre
        let mut errors = Vec::new();
        while (position as usize) < shaped.len() {
            let sample = shaped[position as usize];
            g.feed((sample, 0.0));
            errors.push(g.error().abs());
            position += g.interval();
        }
        let early: f64 = errors[..40].iter().sum::<f64>() / 40.0;
        let late: f64 = errors[errors.len() - 40..].iter().sum::<f64>() / 40.0;
        assert!(
            late < early,
            "timing error did not settle: {early} at the start, {late} at the end"
        );
    }

    #[test]
    fn a_loop_held_still_keeps_the_instant_it_had_and_stops_looking() {
        // What a receiver needs while the far end is silent. Everything on the
        // line then is its own transmission, and a timing loop is perfectly
        // capable of locking onto that: it is a real clock, it is not the one
        // the receiver needs, and having found it the loop will not give it up
        // when the far end comes back.
        //
        // A V.32 modem is silent for a second and a half of its own start-up
        // and transmitting for two more, so this is not a rare corner. The
        // calling modem lost a receiver it had already locked exactly this
        // way, and looked for all the world like an echo canceller fault.
        let sps = 6.0;
        let settled = Gardner::new(sps, 0.1);
        let mut held = Gardner::new(sps, 0.1);
        held.set_adapting(false);

        // Feed it something with plenty of transitions and a timing error in
        // it. A loop that is adapting will move; one held still will not.
        let mut moved = 0;
        for i in 0..2000 {
            let x = if (i / 3) % 2 == 0 { 0.8 } else { -0.8 };
            held.feed((x, x * 0.3));
            if (held.interval() - settled.interval()).abs() > 1.0e-12 {
                moved += 1;
            }
        }
        assert_eq!(moved, 0, "the interval moved on a loop held still");

        // And it goes on producing symbols while it is held: a receiver that
        // stopped delivering would have nothing to hand the descrambler when
        // the far end returned, and no state to carry across.
        let mut symbols = 0;
        for i in 0..100 {
            let x = if (i / 3) % 2 == 0 { 0.8 } else { -0.8 };
            if held.feed((x, 0.0)).is_some() {
                symbols += 1;
            }
        }
        assert_eq!(symbols, 50, "symbols stopped coming out");

        // Let go and it works again.
        held.set_adapting(true);
        for i in 0..2000 {
            let x = if (i / 3) % 2 == 0 { 0.8 } else { -0.8 };
            held.feed((x, x * 0.3));
        }
        assert!(
            (held.interval() - settled.interval()).abs() > 1.0e-9,
            "the loop stayed frozen after being let go"
        );
    }
}
