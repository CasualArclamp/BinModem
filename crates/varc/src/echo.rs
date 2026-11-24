//! An echo canceller, for a line that is one pair of wires.
//!
//! Everything else in V.ARC supposes a wire each way: what this end sends
//! goes to the far end and nowhere else. A two-wire line is not like that.
//! Both ends send into the same pair, and what comes back to each is the far
//! end's signal with its own on top, usually the louder of the two. The far
//! end's signal can only be had by taking this end's own away.
//!
//! That is possible because this end knows exactly what it sent. What it
//! does not know is what the line did to it on the way round: how long it
//! took, and what shape it came back. So there are two parts.
//!
//! **Where the echo is.** A sound card puts tens of milliseconds between a
//! sample being written and its being heard, which is thousands of samples,
//! and the echo itself is a few dozen samples long. A filter long enough to
//! cover all of that would be mostly noughts and would never settle. So the
//! delay is found first, by laying a stretch of what was sent against the
//! same stretch of what was heard at every lag there could be (one
//! transform does all of them at once) and taking the lag where they agree.
//! It is looked for again every few seconds, because a sound card that
//! hiccups moves it.
//!
//! **What shape it is.** A filter of a few milliseconds, fitted by the
//! normalised least-mean-squares rule: predict the echo from what was sent,
//! subtract, and nudge each tap by how much of what was left over looks like
//! the sample under it.
//!
//! The awkward thing is that the far end does not stop talking while this is
//! learned. To the filter the far end's signal is noise, exactly as loud as
//! the signal this is all for, and a filter that takes big steps follows that
//! noise about. Small steps average it away and take a long time. So the
//! step starts large and shrinks as one over the samples seen, which is the
//! running average: after `k` samples the echo left is about `taps / k` of
//! the far end's signal. At 48 kHz with a 4 ms filter that is 25 dB down in a
//! second, 35 dB in ten and 43 dB in a minute, and it goes on improving, 3 dB
//! for each doubling, until the step reaches its floor.
//!
//! It works on samples and knows nothing of what the modulation is.

use dsp::Fft;

/// How much of the line the filter covers, in seconds.
const SPAN: f64 = 0.004;

/// How far round the line an echo may be: a third of a second.
const FURTHEST: f64 = 0.34;

/// The smallest step. Below this the filter would take minutes to follow a
/// line that drifted, and what is left by then is under the noise of any
/// converter there is.
const SMALLEST_STEP: f64 = 2e-5;

/// How much the best lag has to stand above the rest before it is believed,
/// as a ratio of powers.
const SURE: f64 = 40.0;

/// One channel's canceller.
pub struct Canceller {
    taps: Vec<f64>,
    /// What was sent, twice over so that any stretch of it is one slice.
    sent: Vec<f64>,
    /// Where the next sample sent will go in the first copy.
    at: usize,
    /// How long the history is: the furthest lag and the filter.
    kept: usize,
    /// Samples between one being sent and its reaching the filter's newest
    /// tap, once that is known.
    delay: Option<usize>,
    /// The energy under the filter, kept up as it slides.
    energy: f64,
    /// Samples the filter has learned from since it last started.
    learned: u64,
    /// Samples since the energy was last added up from nothing.
    since_summed: u32,
    /// A stretch of what was sent and of what was heard, for finding the
    /// delay, and how much of it there is so far.
    probe_sent: Vec<f64>,
    probe_heard: Vec<f64>,
    probe_len: usize,
    /// What arrives and what is passed on, as powers, slowly averaged.
    power_in: f64,
    power_out: f64,
    /// Told to stop learning; see [`Canceller::hold`].
    held: bool,
    /// Times the delay was found somewhere other than where it had been.
    pub moved: u64,
}

impl std::fmt::Debug for Canceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canceller")
            .field("taps", &self.taps.len())
            .field("delay", &self.delay)
            .field("learned", &self.learned)
            .finish_non_exhaustive()
    }
}

impl Canceller {
    /// A canceller for a line sampled at `fs`.
    pub fn new(fs: f64) -> Self {
        let taps = (SPAN * fs).round() as usize;
        let furthest = (FURTHEST * fs) as usize;
        let kept = furthest + taps + 1;
        // Twice the furthest lag, so that half of it lines up at any lag.
        let probe = (2 * furthest).next_power_of_two();
        Self {
            taps: vec![0.0; taps],
            sent: vec![0.0; 2 * kept],
            at: 0,
            kept,
            delay: None,
            energy: 0.0,
            learned: 0,
            since_summed: u32::MAX / 2,
            probe_sent: vec![0.0; probe],
            probe_heard: vec![0.0; probe],
            probe_len: 0,
            power_in: 0.0,
            power_out: 0.0,
            held: false,
            moved: 0,
        }
    }

    /// How far down the echo has been taken, in decibels: what arrives
    /// against what is passed on. With the far end talking this stops at
    /// however much louder the echo is than the far end, since the far end
    /// is passed on whole.
    pub fn down_db(&self) -> f64 {
        if self.power_out <= 0.0 || self.power_in <= 0.0 {
            return 0.0;
        }
        10.0 * (self.power_in / self.power_out).log10()
    }

    /// The line's answer to one sample sent, as the filter has it: oldest
    /// part of the line first, so it reads left to right as time. Nothing
    /// until the echo has been found.
    pub fn response(&self) -> Vec<f64> {
        if self.delay.is_none() {
            return Vec::new();
        }
        // The filter is laid against history oldest first, so its last tap
        // is the soonest part of the echo.
        self.taps.iter().rev().copied().collect()
    }

    /// Stop learning, or start again.
    ///
    /// While the far end talks this filter never comes to rest: every step
    /// it takes is partly a step after the far end's signal, and it goes on
    /// wandering about the right answer by a little. A receiver with a stage
    /// of its own, which learns from what is left once the far end's part is
    /// taken out, can find what this one missed far more exactly than this
    /// one can, but only if what this one missed stays where it is. So once
    /// that stage is working this one is held, and the other follows
    /// whatever the line does from then on. An echo that moves outright
    /// starts both again.
    pub fn hold(&mut self, held: bool) {
        self.held = held;
    }

    /// What was learned is right: from here on take only the smallest steps.
    ///
    /// For a canceller that has just had the line to itself. With nobody
    /// else talking it learns its echo against the line's noise alone and
    /// has it almost exactly within a second. The moment the far end starts,
    /// the steps it was taking would be steps after the far end's signal,
    /// large enough to undo that in another second. So it is told that it
    /// has seen as many samples as it takes for the steps to be at their
    /// smallest, which is the same thing.
    pub fn settle(&mut self) {
        let enough = (self.taps.len() as f64 / SMALLEST_STEP) as u64;
        self.learned = self.learned.max(enough);
    }

    /// Whether it has been told to hold still.
    pub fn held(&self) -> bool {
        self.held
    }

    /// The newest sample under the filter: the one sent a delay ago, which
    /// the newest part of the echo now arriving is the echo of. Nought until
    /// the delay is known.
    pub fn under_now(&self) -> f64 {
        self.under().map_or(0.0, |range| self.sent[range.end - 1])
    }

    /// The delay round the line, in samples, once found.
    pub fn delay(&self) -> Option<usize> {
        self.delay.map(|d| d + self.taps.len() / 4)
    }

    /// The stretch of history the filter is laid against: oldest first, and
    /// the newest of it `delay` samples back from the last one sent.
    fn under(&self) -> Option<std::ops::Range<usize>> {
        let delay = self.delay?;
        // `at + kept` is one past the newest sample in the second copy.
        let end = self.at + self.kept - delay;
        Some(end - self.taps.len()..end)
    }

    /// A sample as it arrived; the same sample with this end's echo taken
    /// out of it.
    pub fn clean(&mut self, heard: f64) -> f64 {
        if self.probe_len < self.probe_heard.len() {
            self.probe_heard[self.probe_len] = heard;
        }
        let Some(range) = self.under() else {
            self.power_in += (heard * heard - self.power_in) / 4096.0;
            self.power_out = self.power_in;
            return heard;
        };
        let under = &self.sent[range];
        let echo: f64 = self.taps.iter().zip(under).map(|(t, x)| t * x).sum();
        let left = heard - echo;
        // Only while this end is sending is there anything to learn from,
        // and not while it has been told to hold still.
        if self.energy > 1e-9 && !self.held {
            let n = self.taps.len() as f64;
            let step = (n / (self.learned as f64 + n)).max(SMALLEST_STEP);
            let push = step * left / (self.energy + 1e-9);
            for (t, x) in self.taps.iter_mut().zip(under) {
                *t += push * x;
            }
            self.learned += 1;
        }
        self.power_in += (heard * heard - self.power_in) / 4096.0;
        self.power_out += (left * left - self.power_out) / 4096.0;
        left
    }

    /// A sample this end has just sent.
    pub fn sent(&mut self, sample: f64) {
        if self.probe_len == 0 && sample == 0.0 {
            // Nothing is being sent, so there is no echo to look for yet.
            // The stretch starts with the first thing sent: an end that has
            // been keeping silent for its turn then has a whole stretch of
            // its own signal to look in, and not the tail of one.
        } else if self.probe_len < self.probe_sent.len() {
            self.probe_sent[self.probe_len] = sample;
            self.probe_len += 1;
            if self.probe_len == self.probe_sent.len() {
                self.look();
            }
        } else {
            // Wait as long again before looking again.
            self.probe_len += 1;
            if self.probe_len >= 8 * self.probe_sent.len() {
                self.probe_len = 0;
            }
        }
        // The energy under the filter, kept up as the filter slides: the
        // sample that comes under it in, the one that leaves out, and the
        // whole sum afresh every so often so that the rounding of a million
        // additions does not gather.
        let leaving = self.under().map(|range| self.sent[range.start]);
        self.sent[self.at] = sample;
        self.sent[self.at + self.kept] = sample;
        self.at = (self.at + 1) % self.kept;
        self.since_summed += 1;
        self.energy = match (self.under(), leaving) {
            (Some(range), Some(leaving)) if self.since_summed < 4096 => {
                let arriving = self.sent[range.end - 1];
                (self.energy + arriving * arriving - leaving * leaving).max(0.0)
            }
            (Some(range), _) => {
                self.since_summed = 0;
                self.sent[range].iter().map(|x| x * x).sum()
            }
            (None, _) => 0.0,
        };
    }

    /// Find the delay from the stretch just gathered.
    fn look(&mut self) {
        let n = self.probe_sent.len();
        let loud: f64 = self.probe_sent.iter().map(|x| x * x).sum();
        if loud < 1e-6 {
            // Nothing was sent, so there is nothing to find.
            self.probe_len = 0;
            return;
        }
        let fft = Fft::new(n);
        let (mut sr, mut si) = (self.probe_sent.clone(), vec![0.0; n]);
        let (mut hr, mut hi) = (self.probe_heard.clone(), vec![0.0; n]);
        fft.process(&mut sr, &mut si);
        fft.process(&mut hr, &mut hi);
        // Heard times the conjugate of sent, and then the transform of its
        // own conjugate, which is the inverse transform but for a scale.
        for k in 0..n {
            let (re, im) = (hr[k] * sr[k] + hi[k] * si[k], hi[k] * sr[k] - hr[k] * si[k]);
            hr[k] = re;
            hi[k] = -im;
        }
        fft.process(&mut hr, &mut hi);
        let furthest = self.kept - self.taps.len() - 1;
        let (lag, peak) =
            hr[..furthest]
                .iter()
                .map(|c| c * c)
                .enumerate()
                .fold(
                    (0, 0.0),
                    |best, (i, p)| if p > best.1 { (i, p) } else { best },
                );
        let mean = hr[..furthest].iter().map(|c| c * c).sum::<f64>() / furthest as f64;
        if peak < SURE * mean {
            return;
        }
        // The filter starts a quarter of itself ahead of the peak, since a
        // line's response has a little before its largest part and more
        // after.
        let lead = self.taps.len() / 4;
        let delay = lag.saturating_sub(lead).max(1);
        match self.delay {
            // A sample or two either way is the same echo: the filter has
            // it, wherever under itself it lies.
            Some(old) if old.abs_diff(delay) <= lead / 2 => {}
            was => {
                if was.is_some() {
                    self.moved += 1;
                }
                self.delay = Some(delay);
                self.taps.fill(0.0);
                self.learned = 0;
                self.held = false;
                self.since_summed = u32::MAX / 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Noise(u64);
    impl Noise {
        fn uniform(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn gaussian(&mut self) -> f64 {
            (-2.0 * self.uniform().ln()).sqrt() * (std::f64::consts::TAU * self.uniform()).cos()
        }
    }

    /// A line that brings what was sent back `delay` samples later, through
    /// a short response, with a far end at `far` talking over it. Returns
    /// the echo left in what the canceller passed on, against the far end's
    /// signal, in decibels, over the last second.
    fn left_over(fs: f64, delay: usize, far: f64, seconds: f64) -> (f64, Canceller) {
        let shape = [0.05, -0.2, 0.9, 0.35, -0.15, 0.08, -0.03];
        let mut canceller = Canceller::new(fs);
        let (mut ours, mut theirs) = (Noise(3), Noise(5));
        let mut line = std::collections::VecDeque::from(vec![0.0; delay + shape.len()]);
        let total = (seconds * fs) as usize;
        let (mut residue, mut wanted) = (0.0, 0.0);
        for n in 0..total {
            let echo: f64 = shape
                .iter()
                .enumerate()
                .map(|(i, s)| s * line[delay + i])
                .sum();
            let far_now = far * 0.1 * theirs.gaussian();
            let out = canceller.clean(echo + far_now);
            if n + fs as usize >= total {
                residue += (out - far_now) * (out - far_now);
                wanted += far_now * far_now;
            }
            let sending = 0.1 * ours.gaussian();
            canceller.sent(sending);
            line.pop_back();
            line.push_front(sending);
        }
        (10.0 * (residue / wanted.max(1e-30)).log10(), canceller)
    }

    #[test]
    fn the_delay_is_found_and_the_echo_taken_out_with_the_far_end_talking() {
        // A far end as loud as the echo, and one 20 dB quieter than it.
        for (far, delay) in [(1.0, 2000usize), (0.1, 9000)] {
            let (left, canceller) = left_over(48_000.0, delay, far, 12.0);
            let found = canceller.delay().expect("a delay");
            assert!(found.abs_diff(delay) < 8, "found {found}, was {delay}");
            // Ten seconds of learning with 192 taps: about 34 dB.
            assert!(
                left < -30.0,
                "echo left {left:.1} dB against the far end at {far}"
            );
            assert_eq!(canceller.moved, 0);
        }
    }

    #[test]
    fn it_goes_on_improving() {
        let (soon, _) = left_over(48_000.0, 3000, 1.0, 4.0);
        let (later, _) = left_over(48_000.0, 3000, 1.0, 30.0);
        assert!(
            later < soon - 6.0,
            "{soon:.1} dB after 4 s, {later:.1} dB after 30"
        );
        assert!(later < -36.0, "{later:.1} dB after 30 s");
    }

    #[test]
    fn a_line_with_no_echo_is_passed_on_untouched() {
        let mut canceller = Canceller::new(48_000.0);
        let (mut ours, mut theirs) = (Noise(3), Noise(5));
        for _ in 0..200_000 {
            let far = 0.1 * theirs.gaussian();
            assert_eq!(canceller.clean(far), far);
            canceller.sent(0.1 * ours.gaussian());
        }
        assert_eq!(canceller.delay(), None);
    }
}
