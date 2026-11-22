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
//!
//! Phase 4 has no data mode level to judge against, and a far end can stop
//! in it without going quiet at all: the GlobalPOPs / NetZero server froze
//! there twice, once playing its last 20.25 ms over and over for 57 ms and
//! then digital zeros (live-1789986037), and once playing one such block for
//! 6.7 s (the retrain in live-1789986211). Neither is anything a working
//! digital modem sends in phase 4, where every sequence but R is scrambled,
//! and that is what [`Stopped`] watches for instead.

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

/// The longest block a frozen far end is looked for replaying, in seconds.
///
/// Both freezes replayed 324 samples of the line, 162 codewords, 20.25 ms --
/// the server's last block, over and over. A tenth of a second is five of
/// those, and longer than the packets a softphone carries.
const LONGEST_BLOCK: f64 = 0.1;

/// The shortest, in seconds: anything that repeats more often than this is
/// phase 4's own. R is "the 6 symbol sequence ... repeated" (8.6.4), 0.75 ms,
/// and the digital modem sends it until it has CPt -- over a VoIP call's
/// round trip, well over a second.
const SHORTEST_BLOCK: f64 = 0.002;

/// How near a sample has to come to the one a block earlier to be the same:
/// a ten-thousandth of full scale.
///
/// Not exact. The softphone's path is not quite bit-exact even when the
/// codewords are: measured at the 324-sample lag, 79 per cent of the
/// replayed samples in live-1789986037 came back exactly and 99 per cent
/// within one step of sixteen bits, and in live-1789986211 71 and 92 per
/// cent, every other sample of it inside the softphone's own interruptions
/// (see [`LOST_AFTER`]). The block's RMS was 3600 steps in both. A
/// ten-thousandth of full scale is three and a bit steps, and 61 dB under
/// what the block carried.
const SAME: f64 = 1e-4;

/// Samples that must repeat before the line is taken to be repeating, and
/// how often that is looked for: two milliseconds each.
const MATCHED: usize = 32;
const LOOK_EVERY: usize = 32;

/// How long a block may go unreplayed and still be the same block, in
/// seconds.
///
/// The replay in live-1789986211 came through in stretches of a third of a
/// second to two seconds, broken by 25, 43, 53, 53, 54 and 103 ms where the
/// softphone did something of its own -- concealment, a slip -- after which
/// it went on at the same lag. A quarter of a second is well over the
/// longest of those.
const LOST_AFTER: f64 = 0.25;

/// How much of the line held still or replaying itself is a far end that has
/// stopped, in seconds: a second of it. That is twenty times the longest
/// stretch of the trunk's own concealment on live-1789986211 -- 25 to 50 ms,
/// repeating itself at a lag of 5 to 15 ms, about once a second -- and
/// thirty times a jitter buffer's hole of digital silence, and nothing of a
/// working phase 4 holds still or repeats at all but R.
const STOPPED: f64 = 1.0;

/// Whether the far end has stopped in phase 4: the line holding one value --
/// digital silence, or DC -- or playing the same block of itself over and
/// over, for [`STOPPED`] in all.
///
/// The repetition is found as the shortest lag at which the latest
/// [`MATCHED`] samples come back, [`SAME`] apart at most. A lag of one is a
/// line that is not moving. A lag of [`SHORTEST_BLOCK`] up to
/// [`LONGEST_BLOCK`] is a block being replayed. Anything between is R, which
/// repeats every six symbols by design, and is let be.
#[derive(Debug, Clone)]
pub(crate) struct Stopped {
    shortest: usize,
    longest: usize,
    lost_after: u64,
    stopped_after: u64,
    /// The latest samples, newest last.
    kept: std::collections::VecDeque<f64>,
    since_look: usize,
    /// The lag being followed, samples that have come back at it, and
    /// samples in a row that have not.
    lag: Option<usize>,
    matched: u64,
    missed: u64,
}

impl Stopped {
    pub(crate) fn new(fs: f64) -> Self {
        let longest = (LONGEST_BLOCK * fs) as usize;
        Self {
            shortest: (SHORTEST_BLOCK * fs) as usize,
            longest,
            lost_after: (LOST_AFTER * fs) as u64,
            stopped_after: (STOPPED * fs) as u64,
            kept: std::collections::VecDeque::with_capacity(longest + MATCHED + 1),
            since_look: 0,
            lag: None,
            matched: 0,
            missed: 0,
        }
    }

    /// Forget everything: phase 4 is starting, or is over.
    pub(crate) fn reset(&mut self) {
        self.kept.clear();
        self.since_look = 0;
        self.drop_lag();
    }

    fn drop_lag(&mut self) {
        self.lag = None;
        self.matched = 0;
        self.missed = 0;
    }

    /// The sample `back` samples before the newest.
    fn at(&self, back: usize) -> f64 {
        self.kept[self.kept.len() - 1 - back]
    }

    /// One line sample.
    pub(crate) fn feed(&mut self, sample: f64) {
        if self.kept.len() == self.longest + MATCHED + 1 {
            self.kept.pop_front();
        }
        self.kept.push_back(sample);
        if let Some(lag) = self.lag {
            if (self.at(0) - self.at(lag)).abs() <= SAME {
                self.matched += 1;
                self.missed = 0;
            } else {
                self.missed += 1;
                if self.missed > self.lost_after {
                    self.drop_lag();
                }
            }
            return;
        }
        self.since_look += 1;
        if self.since_look < LOOK_EVERY {
            return;
        }
        self.since_look = 0;
        if let Some(lag) = self.shortest_lag()
            && (lag == 1 || lag >= self.shortest)
        {
            self.lag = Some(lag);
            self.matched = MATCHED as u64;
        }
    }

    /// The shortest lag at which the latest [`MATCHED`] samples come back.
    fn shortest_lag(&self) -> Option<usize> {
        let most = self.longest.min(self.kept.len().saturating_sub(MATCHED));
        (1..=most).find(|&lag| (0..MATCHED).all(|k| (self.at(k) - self.at(k + lag)).abs() <= SAME))
    }

    /// Whether the line is holding still or replaying itself now.
    pub(crate) fn replaying(&self) -> bool {
        self.lag.is_some() && self.missed == 0
    }

    /// Whether it has done so for long enough that the far end has stopped.
    pub(crate) fn stopped(&self) -> bool {
        self.matched >= self.stopped_after
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

    /// The analogue modem's line rate, which is what [`Stopped`] is fed at.
    const LINE_FS: f64 = 16_000.0;

    /// Something like a scrambled phase 4 sequence: a new level every sample,
    /// never the same twice.
    fn scrambled(seed: &mut u32) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (f64::from(*seed) / f64::from(u32::MAX) - 0.5) * 0.4
    }

    /// Samples fed until the far end is taken to have stopped, if it is
    /// within `most` of them.
    fn until_stopped(w: &mut Stopped, most: usize, mut next: impl FnMut(usize) -> f64) -> Option<usize> {
        (0..most).find(|&n| {
            w.feed(next(n));
            w.stopped()
        })
    }

    /// live-1789986037: the server's line went to exact digital zeros in
    /// phase 4 and stayed there. A second of it is a far end that has
    /// stopped, and so is a second of the DC just before it.
    #[test]
    fn a_second_of_digital_silence_or_of_dc_is_a_far_end_that_has_stopped() {
        for (what, level) in [("silence", 0.0), ("DC", -0.79)] {
            let mut w = Stopped::new(LINE_FS);
            let mut seed = 0x2545_f491;
            (0..8000).for_each(|_| w.feed(scrambled(&mut seed)));
            assert!(!w.replaying() && !w.stopped(), "{what}");
            let after = until_stopped(&mut w, 32_000, |_| level).expect(what);
            let seconds = (after + 1) as f64 / LINE_FS;
            assert!((0.99..1.01).contains(&seconds), "{what}: stopped after {seconds} s");
            // And a line holding still is not carrying anything from the
            // moment it is seen, long before it has been a second.
            let mut w = Stopped::new(LINE_FS);
            (0..8000).for_each(|_| w.feed(scrambled(&mut seed)));
            (0..80).for_each(|_| w.feed(level));
            assert!(w.replaying(), "{what} was not seen within five milliseconds");
        }
    }

    /// The retrain in live-1789986211: the server replayed one 324-sample
    /// block for 6.7 s, each sample back within a step or two of sixteen bits
    /// of where it was, broken now and then by the softphone's own 25 to
    /// 103 ms. A second of that, the interruptions not counted, is a far end
    /// that has stopped.
    #[test]
    fn a_block_played_over_and_over_is_a_far_end_that_has_stopped() {
        let mut w = Stopped::new(LINE_FS);
        let mut seed = 0x9e37_79b9;
        let block: Vec<f64> = (0..324).map(|_| scrambled(&mut seed)).collect();
        block.iter().for_each(|&s| w.feed(s));
        let step = 1.0 / 32768.0;
        // An interruption of 103 ms, 0.4 s in.
        let broken = |n: usize| (6400..6400 + 1648).contains(&n);
        let after = until_stopped(&mut w, 64_000, |n| {
            if broken(n) {
                scrambled(&mut seed)
            } else {
                let jitter = f64::from(n.is_multiple_of(4) as u8) * if n.is_multiple_of(8) { step } else { -step };
                block[n % 324] + jitter
            }
        })
        .expect("never stopped");
        let seconds = (after - 1648) as f64 / LINE_FS;
        assert!((1.0..1.05).contains(&seconds), "stopped after {seconds} s of replay");
    }

    /// Nothing a working far end sends in phase 4 is a far end that has
    /// stopped: not R, which is six symbols over and over (8.6.4); not a
    /// scrambled sequence; not a softphone's concealment, 25 to 50 ms of the
    /// line repeating itself at 5 to 15 ms, once a second; and not a jitter
    /// buffer's holes of digital silence.
    #[test]
    fn r_scrambled_sequences_concealment_and_holes_are_not_a_far_end_that_has_stopped() {
        let mut seed = 0x1234_5678;
        // R at the line rate: six symbols, twelve samples, over and over.
        let r: Vec<f64> = (0..12).map(|k| if k < 6 { 0.12 } else { -0.12 } + 0.01 * k as f64).collect();
        let mut w = Stopped::new(LINE_FS);
        assert_eq!(until_stopped(&mut w, 48_000, |n| r[n % 12]), None, "R");
        assert!(!w.replaying(), "R");
        let mut w = Stopped::new(LINE_FS);
        assert_eq!(until_stopped(&mut w, 48_000, |_| scrambled(&mut seed)), None, "scrambled");
        // Concealment: 40 ms of the line 10 ms back, every 1.1 s.
        let mut w = Stopped::new(LINE_FS);
        let mut line: Vec<f64> = Vec::new();
        let concealed = until_stopped(&mut w, 160_000, |n| {
            let s = if n % 17_600 < 640 && line.len() > 160 { line[line.len() - 160] } else { scrambled(&mut seed) };
            line.push(s);
            s
        });
        assert_eq!(concealed, None, "concealment");
        // Holes: 30 ms of nothing every 1.5 s.
        let mut w = Stopped::new(LINE_FS);
        let holes = until_stopped(&mut w, 160_000, |n| if n % 24_000 < 480 { 0.0 } else { scrambled(&mut seed) });
        assert_eq!(holes, None, "holes");
    }
}
