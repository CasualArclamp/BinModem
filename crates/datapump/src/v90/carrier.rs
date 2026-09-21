//! Whether the far end is still sending, once data mode has begun.
//!
//! V.90 has no carrier detector of its own to lean on. The analogue modem's
//! receiver reads codewords, and silence reads as the quietest of them, so a
//! server that has hung up decodes as a stream of perfectly good zeros and the
//! receiver never counts itself lost. The digital modem had nothing at all.
//! So a call whose far end had gone kept the near end in data mode for as
//! long as anyone let it.
//!
//! What goes instead is the level, measured against what data mode itself
//! carried rather than against a fixed figure: through a softphone the far
//! end arrives at whatever level the path leaves it, and the silence after a
//! hang-up is digital zero or comfort noise, far below either. A line that
//! drops that far and stays there is a far end that has stopped.

/// Time constant of the level being judged, in seconds.
const FAST: f64 = 0.050;

/// Time constant of the level it is judged against.
const SLOW: f64 = 2.0;

/// How long data mode runs before the reference is taken, in seconds.
const WARM_UP: f64 = 0.25;

/// How far below the reference counts as quiet, as a power ratio: 20 dB.
/// A softphone's gain control moves a signal a few decibels; a hang-up takes
/// it tens of decibels down.
const QUIET: f64 = 0.01;

/// A power below which the line is quiet whatever the reference was.
const FLOOR: f64 = 1e-10;

/// How long quiet has to last before the far end is taken to have gone, in
/// seconds. Longer than any gap a jitter buffer leaves, and short of the three
/// seconds after which the analogue modem would start a retrain instead.
const GONE: f64 = 2.0;

/// Watches the line in data mode.
#[derive(Debug, Clone)]
pub(crate) struct Watch {
    fast_step: f64,
    slow_step: f64,
    warm_up: u64,
    gone_after: u64,
    fast: f64,
    reference: Option<f64>,
    heard: u64,
    quiet_for: u64,
}

impl Watch {
    pub(crate) fn new(fs: f64) -> Self {
        Self {
            fast_step: 1.0 - (-1.0 / (FAST * fs)).exp(),
            slow_step: 1.0 - (-1.0 / (SLOW * fs)).exp(),
            warm_up: (WARM_UP * fs) as u64,
            gone_after: (GONE * fs) as u64,
            fast: 0.0,
            reference: None,
            heard: 0,
            quiet_for: 0,
        }
    }

    /// Forget everything: data mode is starting, or starting again after a
    /// renegotiation, whose sequences are not data mode's level.
    pub(crate) fn reset(&mut self) {
        self.fast = 0.0;
        self.reference = None;
        self.heard = 0;
        self.quiet_for = 0;
    }

    /// One line sample from data mode, or from a renegotiation begun from
    /// it: `learn` says which. A renegotiation's sequences are not data
    /// mode's level, so they are judged against it but not taken into it;
    /// neither end goes quiet in one.
    pub(crate) fn feed(&mut self, sample: f64, learn: bool) {
        self.fast += self.fast_step * (sample * sample - self.fast);
        self.heard += 1;
        let Some(reference) = self.reference.as_mut() else {
            if self.heard >= self.warm_up {
                self.reference = Some(self.fast);
            }
            return;
        };
        if self.fast < FLOOR || self.fast < *reference * QUIET {
            self.quiet_for += 1;
        } else {
            self.quiet_for = 0;
            // Followed only while the far end is there, so that a line going
            // quiet cannot drag its own yardstick down after it.
            if learn {
                *reference += self.slow_step * (self.fast - *reference);
            }
        }
    }

    /// Whether the far end has been quiet for long enough to have gone.
    pub(crate) fn gone(&self) -> bool {
        self.quiet_for >= self.gone_after
    }

    /// Whether the far end is quiet now, gone or not.
    ///
    /// Quiet, not silent. The level this is read from has a 50 ms time
    /// constant ([`FAST`]) and [`QUIET`] is 20 dB under the reference, so it
    /// takes about a quarter of a second of nothing before this is ever true:
    /// the twenty milliseconds a jitter buffer leaves when it drops a packet
    /// move the level 1.7 dB and never show here at all. What this catches is
    /// a far end that has stopped -- on its way to [`Self::gone`], which
    /// wants two seconds more of it -- and not a gap in the audio.
    pub(crate) fn quiet(&self) -> bool {
        self.quiet_for > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8000.0;

    fn tone(n: usize, amplitude: f64) -> impl Iterator<Item = f64> {
        (0..n).map(move |i| amplitude * (i as f64 * 0.7).sin())
    }

    #[test]
    fn a_far_end_that_stops_is_gone_two_seconds_later() {
        let mut w = Watch::new(FS);
        tone(8000, 0.3).for_each(|s| w.feed(s, true));
        assert!(!w.gone());
        let mut after = 0;
        while !w.gone() {
            w.feed(0.0, true);
            after += 1;
            assert!(after < 3 * 8000, "never noticed");
        }
        let seconds = after as f64 / FS;
        // Two seconds of quiet, once the level has taken its 0.23 s to fall.
        assert!((2.0..2.4).contains(&seconds), "noticed after {seconds} s");
    }

    #[test]
    fn a_quieter_far_end_and_short_gaps_are_not_a_hang_up() {
        let mut w = Watch::new(FS);
        tone(8000, 0.3).for_each(|s| w.feed(s, true));
        // Twelve decibels down, as a gain control might leave it, and a
        // jitter buffer's 60 ms of nothing now and then.
        for _ in 0..20 {
            tone(4000, 0.075).for_each(|s| w.feed(s, true));
            (0..480).for_each(|_| w.feed(0.0, true));
            assert!(!w.gone());
        }
    }

    /// What `quiet` can see and what it cannot: a far end that has stopped,
    /// within a quarter of a second, and never a packet's worth of gap
    /// however many of them there are.
    #[test]
    fn a_packet_s_gap_is_never_quiet_and_a_far_end_that_stopped_is_within_a_quarter_of_a_second() {
        let mut w = Watch::new(FS);
        tone(8000, 0.3).for_each(|s| w.feed(s, true));
        for _ in 0..20 {
            (0..(0.020 * FS) as usize).for_each(|_| w.feed(0.0, true));
            assert!(!w.quiet(), "twenty milliseconds of nothing read as quiet");
            tone(4000, 0.3).for_each(|s| w.feed(s, true));
        }
        // And then silence that does not stop.
        let mut after = 0;
        while !w.quiet() {
            w.feed(0.0, true);
            after += 1;
            assert!(after < 8000, "never quiet");
        }
        let seconds = after as f64 / FS;
        println!("quiet after {seconds:.3} s of silence");
        assert!((0.2..0.3).contains(&seconds), "quiet after {seconds} s");
    }

    #[test]
    fn comfort_noise_after_a_hang_up_is_still_quiet() {
        let mut w = Watch::new(FS);
        tone(8000, 0.3).for_each(|s| w.feed(s, true));
        // Noise 40 dB under the signal.
        let mut seed = 0x1234_5678u32;
        for _ in 0..(3.0 * FS) as usize {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            w.feed((f64::from(seed) / f64::from(u32::MAX) - 0.5) * 0.006, true);
        }
        assert!(w.gone());
    }

    #[test]
    fn nothing_is_judged_before_data_mode_has_been_heard() {
        let mut w = Watch::new(FS);
        (0..(3.0 * FS) as usize).for_each(|_| w.feed(0.0, true));
        // Silence from the very start is still silence, and still a far end
        // that is not there, once the warm-up is over.
        assert!(w.gone());
        w.reset();
        assert!(!w.gone());
    }
}
