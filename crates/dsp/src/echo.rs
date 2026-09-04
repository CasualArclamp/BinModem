//! Echo cancellation.
//!
//! A modem on a two-wire line hears itself. The hybrid transformer that joins
//! the one pair of the line to the separate transmit and receive paths inside
//! the modem is never perfectly balanced, so some of what is transmitted comes
//! straight back; and the network beyond adds further reflections wherever the
//! impedance changes, arriving tens of milliseconds later. Twelve decibels of
//! near return is ordinary, against a far signal twenty down, which leaves a
//! modem listening to itself eight decibels louder than to the thing it is
//! trying to hear.
//!
//! V.22bis escapes this by putting the two directions in different bands: the
//! filter that selects the far channel discards the echo along with it, and
//! nothing here is needed. From V.32 onwards both directions occupy the same
//! band at the same time, no filter can tell them apart, and the echo has to be
//! subtracted instead.
//!
//! Subtracting it is possible because, uniquely among the things on the line,
//! the echo is of a signal we know exactly: we sent it. What is unknown is only
//! what the line did to it on the way back, and that is a filter, which can be
//! learned by trying one and seeing what is left.
//!
//! There are two of them, and they are nowhere near each other. The hybrid
//! reflects at once; the network reflects from wherever the impedance
//! changes, which on a long connection is tens of milliseconds away. So the
//! canceller is in two pieces, and the second cannot be placed until
//! something has found out where to place it.

use std::collections::VecDeque;

/// Guards the division when the line is silent.
const FLOOR: f64 = 1.0e-9;

/// How quickly the return-loss meters follow the signal. About a millisecond
/// at the rates used here, which is long enough to average a symbol and short
/// enough to follow a signal starting.
const POWER_TRACK: f64 = 0.01;

/// One run of taps, and how far back from the present it begins.
#[derive(Debug, Clone)]
struct Segment {
    offset: usize,
    taps: Vec<f64>,
    /// Energy currently inside this run, kept exactly rather than averaged.
    energy: f64,
}

impl Segment {
    fn new(offset: usize, taps: usize) -> Self {
        Self {
            offset,
            taps: vec![0.0; taps],
            energy: 0.0,
        }
    }

    /// How far back the last of these taps reaches.
    fn end(&self) -> usize {
        self.offset + self.taps.len()
    }

    /// The part of the echo this run accounts for.
    fn echo(&self, history: &VecDeque<f64>) -> f64 {
        self.taps
            .iter()
            .enumerate()
            .map(|(i, t)| t * history[self.offset + i])
            .sum()
    }

    /// Take in the sample that has just entered the window and let go of the
    /// one that has just left it, so the energy stays exact.
    fn shift(&mut self, history: &VecDeque<f64>) {
        let entering = history[self.offset];
        let leaving = history[self.end()];
        self.energy += entering * entering - leaving * leaving;
        self.energy = self.energy.max(0.0);
    }

    fn adapt(&mut self, history: &VecDeque<f64>, gain: f64) {
        for (i, tap) in self.taps.iter_mut().enumerate() {
            *tap += gain * history[self.offset + i];
        }
    }

    /// Count the energy from scratch, for when the window has just been placed
    /// somewhere it has never been.
    fn recount(&mut self, history: &VecDeque<f64>) {
        self.energy = (self.offset..self.end())
            .map(|i| history[i] * history[i])
            .sum();
    }
}

/// Adaptive canceller for a modem's own echo.
///
/// Adapts by normalised least mean squares: each sample, the taps move along
/// the reference in proportion to what is left over. Normalising by the power
/// of the reference is what makes the step size mean the same thing at every
/// signal level, so one setting works on a loud line and a quiet one.
///
/// The taps come in two runs rather than one. The hybrid's reflection arrives
/// at once and is modelled from the first sample; the network's arrives from
/// wherever the line changes impedance, which on a long connection is tens of
/// milliseconds later, with nothing whatever in between. Spanning both with
/// one continuous filter would mean carrying a thousand taps to model two
/// hundred, and paying for the empty ones twice: once in arithmetic, and once
/// in the adaptation noise every idle tap adds to the residue. Least mean
/// squares also converges more slowly the more taps it carries, and the
/// training segment it has to converge inside is fixed.
#[derive(Debug, Clone)]
pub struct EchoCanceller {
    /// The hybrid's own reflection, which comes back immediately.
    near: Segment,
    /// The network's, placed once something has worked out where it is.
    far: Option<Segment>,
    /// What we transmitted, most recent first, one longer than the taps reach
    /// so that each run can see the sample falling out of its far end.
    history: VecDeque<f64>,
    step: f64,
    adapting: bool,
    /// Running powers of what arrived and what is left, for the return loss.
    heard: f64,
    residue: f64,
}

impl EchoCanceller {
    /// `taps` should span the near echo in samples.
    ///
    /// Too short and the tail it cannot reach is left uncancelled; too long
    /// and every extra tap adds its own adaptation noise while modelling
    /// nothing. The near echo of a hybrid arrives within a millisecond or two,
    /// and that is all this is for: a network reflection is a long way behind
    /// it and belongs to [`watch_far_echo`](Self::watch_far_echo).
    ///
    /// `step` between 0 and 2 is stable, but only in theory and only without
    /// noise. Something around a tenth converges in a few thousand samples and
    /// leaves the taps quiet once it has.
    pub fn new(taps: usize, step: f64) -> Self {
        Self {
            near: Segment::new(0, taps),
            far: None,
            history: VecDeque::from(vec![0.0; taps + 1]),
            step,
            adapting: true,
            heard: 0.0,
            residue: 0.0,
        }
    }

    /// Put a second run of taps `delay` samples back, for a reflection that
    /// arrives from further away than the near ones reach.
    ///
    /// The delay has to come from somewhere, and a modem has two ways of
    /// getting it. V.32's start-up measures the round trip outright (5.4), and
    /// nothing can return later than that. Within that bound the reflection
    /// can be found by [`EchoFinder`], which is the more useful of the two
    /// because a bound is not an address.
    ///
    /// Anything already learned about the near echo is kept.
    pub fn watch_far_echo(&mut self, delay: usize, taps: usize) {
        let mut far = Segment::new(delay, taps);
        self.history.resize(self.near.end().max(far.end()) + 1, 0.0);
        far.recount(&self.history);
        self.far = Some(far);
    }

    /// Where the second run of taps sits, if there is one.
    pub fn far_echo(&self) -> Option<(usize, usize)> {
        self.far.as_ref().map(|f| (f.offset, f.taps.len()))
    }

    /// How far back the canceller can see, in samples.
    pub fn span(&self) -> usize {
        self.near.end().max(self.far.as_ref().map_or(0, Segment::end))
    }

    /// Whether the taps are being updated.
    ///
    /// They should not be while the far end is talking. The canceller is
    /// trying to explain everything it hears as an echo of what it sent, and
    /// what the far end sends cannot be explained that way, so it appears as a
    /// large unexplained residue and drags the taps away from the answer. Real
    /// modems train the canceller during the part of the handshake when the
    /// far end is required to be silent, and hold it still afterwards.
    pub fn set_adapting(&mut self, adapting: bool) {
        self.adapting = adapting;
    }

    pub fn is_adapting(&self) -> bool {
        self.adapting
    }

    /// Remove our own echo from one received sample.
    ///
    /// `transmitted` is what went out on the line this instant; `received` is
    /// what came back in. Returns what is left once the echo is accounted for,
    /// which is the far end plus whatever the canceller has not learned yet.
    pub fn process(&mut self, transmitted: f64, received: f64) -> f64 {
        // Keep the energy under each run exactly, by adding what came in and
        // taking off what fell out the end.
        //
        // A running average of the reference power will not do here, however
        // slowly it moves. It is what the step is divided by, so whenever it
        // reads low the step comes out large, and a signal that is not white
        // spends much of its time away from its own average. Filtering noise
        // to a thousand hertz was enough: the canceller diverged completely,
        // and reported a return loss of minus two hundred decibels.
        self.history.pop_back();
        self.history.push_front(transmitted);
        self.near.shift(&self.history);
        if let Some(far) = self.far.as_mut() {
            far.shift(&self.history);
        }

        let echo = self.near.echo(&self.history)
            + self.far.as_ref().map_or(0.0, |f| f.echo(&self.history));
        let left = received - echo;

        // Track what arrived and what is left, for the return loss.
        self.heard += POWER_TRACK * (received * received - self.heard);
        self.residue += POWER_TRACK * (left * left - self.residue);

        if self.adapting {
            // Normalised least mean squares. The gradient of the squared
            // residue with respect to each tap is the reference at that tap's
            // delay, so moving every tap along its own sample by the same
            // fraction of the residue reduces it.
            //
            // The two runs share one division, by the energy under both of
            // them together. They are one filter with a hole in it rather than
            // two filters, and normalising each by its own energy would let
            // the pair take a step twice the size of the one that is stable.
            let energy = self.near.energy + self.far.as_ref().map_or(0.0, |f| f.energy);
            let gain = self.step * left / (energy + FLOOR);
            self.near.adapt(&self.history, gain);
            if let Some(far) = self.far.as_mut() {
                far.adapt(&self.history, gain);
            }
        }
        left
    }

    /// How much of what arrives is being removed, in decibels.
    ///
    /// Meaningful only while the far end is quiet: with both present this
    /// measures the ratio of everything heard to everything left, and the far
    /// end is in both.
    pub fn echo_return_loss(&self) -> f64 {
        if self.heard < FLOOR {
            return 0.0;
        }
        10.0 * (self.heard / (self.residue + FLOOR)).log10()
    }

    /// Restart the return-loss measurement without disturbing the taps.
    pub fn reset_meters(&mut self) {
        self.heard = 0.0;
        self.residue = 0.0;
    }

    /// Forget everything learned.
    pub fn reset(&mut self) {
        self.near.taps.iter_mut().for_each(|t| *t = 0.0);
        self.near.energy = 0.0;
        if let Some(far) = self.far.as_mut() {
            far.taps.iter_mut().for_each(|t| *t = 0.0);
            far.energy = 0.0;
        }
        self.history.iter_mut().for_each(|x| *x = 0.0);
        self.heard = 0.0;
        self.residue = 0.0;
    }

    /// How many taps there are, over both runs.
    pub fn len(&self) -> usize {
        self.near.taps.len() + self.far.as_ref().map_or(0, |f| f.taps.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where a reflection of our own signal is coming back from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reflection {
    /// Delay in samples between sending it and hearing it again.
    pub delay: usize,
    /// How much of what arrives it accounts for, between nothing and one.
    ///
    /// A line that returns a clean copy of what was sent and nothing else
    /// reads one, whatever it attenuates the copy by. Everything else on the
    /// line — the near echo, the far modem, noise — is in the denominator and
    /// not the numerator, so this falls as the reflection becomes a smaller
    /// share of what is heard.
    pub strength: f64,
}

/// Finds how far away a reflection of our own signal is.
///
/// V.32's start-up measures the round trip (5.4) because the echo canceller
/// needs to know where to look, but what that gives is a bound rather than an
/// address: a reflection comes from wherever the line changes impedance, which
/// can be anywhere along it. Guessing has a real cost, because a run of taps
/// placed where the echo is not models nothing at all.
///
/// What settles it is that the signal being reflected is one we know exactly.
/// Comparing what arrives against every delay at once, over a stretch where
/// the far end is silent, leaves the delays that explain nothing hovering
/// around zero and the one that explains the echo standing above them.
///
/// This wants a signal with no pattern in it. The start-up's tones and
/// alternations are periodic, and a periodic reference matches equally well at
/// every delay a whole number of periods away, so it says nothing about which
/// one is right. The training segment is scrambled, and is therefore both the
/// only stretch quiet enough to measure in and the only one with the shape to
/// measure with.
#[derive(Debug, Clone)]
pub struct EchoFinder {
    /// What we transmitted, most recent first, reaching back to the last
    /// candidate delay.
    history: VecDeque<f64>,
    /// How well each candidate explains what is arriving, from `first` up.
    scores: Vec<f64>,
    first: usize,
    /// Energies of the two signals, to put the scores on a scale that depends
    /// neither on how loud the line is nor on how long we have listened.
    reference: f64,
    arriving: f64,
}

impl EchoFinder {
    /// Look for a reflection between `first` and `last` samples back.
    ///
    /// `first` should be past the near taps: the hybrid's reflection is the
    /// loudest thing on the line during training and would win every time, and
    /// it is already covered.
    pub fn new(first: usize, last: usize) -> Self {
        let last = last.max(first);
        Self {
            history: VecDeque::from(vec![0.0; last + 1]),
            scores: vec![0.0; last - first + 1],
            first,
            reference: 0.0,
            arriving: 0.0,
        }
    }

    /// Offer one sample of what went out and what came back.
    pub fn feed(&mut self, transmitted: f64, received: f64) {
        self.history.pop_back();
        self.history.push_front(transmitted);
        self.reference += transmitted * transmitted;
        self.arriving += received * received;
        for (i, score) in self.scores.iter_mut().enumerate() {
            *score += received * self.history[self.first + i];
        }
    }

    /// The strongest reflection found, if the line carried enough to say.
    pub fn best(&self) -> Option<Reflection> {
        if self.reference < FLOOR || self.arriving < FLOOR {
            return None;
        }
        let scale = (self.reference * self.arriving).sqrt();
        let (i, score) = self
            .scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
        Some(Reflection {
            delay: self.first + i,
            strength: score.abs() / scale,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line that returns some of what it is given, after a delay and through
    /// a filter: the thing the canceller has to work out.
    struct Path {
        response: Vec<f64>,
        history: VecDeque<f64>,
    }

    impl Path {
        fn new(response: Vec<f64>) -> Self {
            let n = response.len();
            Self {
                response,
                history: VecDeque::from(vec![0.0; n]),
            }
        }

        fn echo(&mut self, x: f64) -> f64 {
            self.history.pop_back();
            self.history.push_front(x);
            self.response
                .iter()
                .zip(self.history.iter())
                .map(|(h, x)| h * x)
                .sum()
        }
    }

    /// Pseudorandom, deterministic, and not periodic over the lengths used
    /// here: an echo canceller learns nothing from a signal that repeats,
    /// because many different filters explain a repeating input equally well.
    fn noise(n: usize) -> Vec<f64> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn a_static_echo_is_learned_and_removed() {
        // A near reflection and a weaker one a little behind it.
        let mut path = Path::new(vec![0.0, 0.0, 0.25, 0.1, -0.05, 0.0, 0.0, 0.02]);
        let mut ec = EchoCanceller::new(16, 0.5);
        let sent = noise(40_000);
        let mut worst_late: f64 = 0.0;
        for (i, &x) in sent.iter().enumerate() {
            let heard = path.echo(x);
            let left = ec.process(x, heard);
            // Judge only once it has had time to converge.
            if i > 30_000 {
                worst_late = worst_late.max(left.abs());
            }
        }
        assert!(
            worst_late < 1.0e-3,
            "residue still {worst_late:.2e} after convergence"
        );
        // 73 dB measured, so the bar is well clear of where it lands.
        assert!(
            ec.echo_return_loss() > 40.0,
            "only {:.1} dB of the echo removed",
            ec.echo_return_loss()
        );
    }

    #[test]
    fn an_echo_beyond_the_taps_is_partly_left_behind() {
        // Honest about the limit: a reflection further back than the filter
        // reaches cannot be cancelled, and shortening the filter is how a
        // canceller fails on a long line rather than something subtle.
        let mut response = vec![0.0; 40];
        response[2] = 0.25;
        response[35] = 0.2;
        let mut path = Path::new(response);
        let mut ec = EchoCanceller::new(8, 0.5);
        let sent = noise(40_000);
        let mut left_energy = 0.0;
        let mut heard_energy = 0.0;
        for (i, &x) in sent.iter().enumerate() {
            let heard = path.echo(x);
            let left = ec.process(x, heard);
            if i > 30_000 {
                left_energy += left * left;
                heard_energy += heard * heard;
            }
        }
        // The near reflection goes, the far one stays: 0.25 against 0.2 leaves
        // rather more than half the power behind.
        let removed = 10.0 * (heard_energy / left_energy).log10();
        assert!(
            (1.0..6.0).contains(&removed),
            "removed {removed:.1} dB, which is not the partial job expected"
        );
    }

    #[test]
    fn holding_the_taps_still_keeps_what_was_learned() {
        let mut path = Path::new(vec![0.0, 0.3, 0.1]);
        let mut ec = EchoCanceller::new(8, 0.5);
        let sent = noise(40_000);
        for &x in &sent[..30_000] {
            let heard = path.echo(x);
            ec.process(x, heard);
        }
        let trained = ec.echo_return_loss();
        assert!(trained > 40.0, "did not converge: {trained:.1} dB");

        // Now the far end speaks, and the canceller is told to stop learning.
        ec.set_adapting(false);
        let far = noise(10_000);
        let mut worst: f64 = 0.0;
        for (i, &x) in sent[30_000..].iter().enumerate() {
            let heard = path.echo(x) + far[i];
            let left = ec.process(x, heard);
            // What is left should be the far end and nothing else.
            worst = worst.max((left - far[i]).abs());
        }
        assert!(
            worst < 1.0e-3,
            "the far end came through distorted by {worst:.2e}"
        );
    }

    #[test]
    fn the_far_end_pulls_the_taps_astray_if_it_is_allowed_to() {
        // The reason set_adapting exists. Left adapting through double talk,
        // the canceller tries to explain the far end as an echo of us, and
        // gets worse at the job it had already learned.
        let response = vec![0.0, 0.3, 0.1];
        let sent = noise(60_000);
        let far = noise(30_000);

        let train = |adapt_through: bool| {
            let mut path = Path::new(response.clone());
            let mut ec = EchoCanceller::new(8, 0.5);
            for &x in &sent[..30_000] {
                let heard = path.echo(x);
                ec.process(x, heard);
            }
            ec.set_adapting(adapt_through);
            for (i, &x) in sent[30_000..].iter().enumerate() {
                let heard = path.echo(x) + far[i % far.len()] * 3.0;
                ec.process(x, heard);
            }
            // Measure afterwards, on the echo alone, with the taps frozen.
            ec.set_adapting(false);
            ec.reset_meters();
            for &x in &sent[..4_000] {
                let heard = path.echo(x);
                ec.process(x, heard);
            }
            ec.echo_return_loss()
        };

        let held = train(false);
        let dragged = train(true);
        assert!(
            held > dragged + 10.0,
            "holding the taps still gave {held:.1} dB against {dragged:.1} dB \
             for adapting through the far end, which is not the difference \
             the guard is there for"
        );
    }

    #[test]
    fn a_band_limited_reference_converges_more_slowly_but_still_converges() {
        // A modem signal is not white. It occupies a few hundred hertz of a
        // four kilohertz band, which means the reference carries no
        // information at all about how the echo path behaves everywhere else,
        // and least mean squares converges along each direction in proportion
        // to how much energy points that way. The taps outside the band drift
        // rather than settle. What matters is that the echo is still removed
        // where the signal actually is, which is the only place it is heard.
        let fs = 16_000.0;
        let mut shape = crate::Fir::new(crate::fir_lowpass(1000.0, 61, fs));
        let reference: Vec<f64> = noise(200_000).iter().map(|&x| shape.process(x)).collect();

        let mut path = Path::new(vec![0.0, 0.0, 0.25, 0.1, -0.05]);
        let mut ec = EchoCanceller::new(16, 0.5);
        for &x in &reference {
            let heard = path.echo(x);
            ec.process(x, heard);
        }
        // 59 dB measured against 73 for white noise: the price of the
        // colouring, and still far more than a modem needs.
        assert!(
            ec.echo_return_loss() > 30.0,
            "only {:.1} dB removed from a band-limited reference",
            ec.echo_return_loss()
        );
    }

    #[test]
    fn nothing_is_learned_from_silence() {
        let mut ec = EchoCanceller::new(8, 0.5);
        for _ in 0..1000 {
            assert_eq!(ec.process(0.0, 0.0), 0.0);
        }
        assert!(ec.near.taps.iter().all(|t| t.abs() < 1.0e-12));
    }

    /// A near reflection off the hybrid and a far one off the network, with
    /// nothing at all in between: what a long connection actually looks like,
    /// and the shape one continuous filter is the wrong answer to.
    fn split_path(far: usize) -> Vec<f64> {
        let mut response = vec![0.0; far + 8];
        response[2] = 0.25;
        response[3] = 0.1;
        response[far] = 0.18;
        response[far + 1] = -0.06;
        response
    }

    #[test]
    fn a_second_run_of_taps_reaches_an_echo_the_first_cannot() {
        const FAR: usize = 500;

        let run = |place: Option<usize>| {
            let mut path = Path::new(split_path(FAR));
            let mut ec = EchoCanceller::new(32, 0.5);
            if let Some(delay) = place {
                ec.watch_far_echo(delay, 32);
            }
            for &x in &noise(120_000) {
                let heard = path.echo(x);
                ec.process(x, heard);
            }
            ec.echo_return_loss()
        };

        // Near taps alone: the reflection they cannot reach is most of what is
        // left, and no amount of adapting will help, because the samples that
        // would explain it fell out of the filter long ago.
        let near_only = run(None);
        assert!(
            near_only < 12.0,
            "near taps alone removed {near_only:.1} dB, which is more than \
             they can reach"
        );

        let both = run(Some(FAR - 8));
        assert!(
            both > 40.0,
            "with the second run placed on it, only {both:.1} dB removed"
        );
    }

    #[test]
    fn the_finder_says_how_far_away_the_reflection_is() {
        const FAR: usize = 640;
        let mut path = Path::new(split_path(FAR));
        let mut finder = EchoFinder::new(128, 1024);
        for &x in &noise(40_000) {
            let heard = path.echo(x);
            finder.feed(x, heard);
        }
        let found = finder.best().expect("nothing found on a line with an echo");
        assert_eq!(
            found.delay, FAR,
            "put the reflection {} samples from where it is",
            found.delay as i64 - FAR as i64
        );
        // 0.52 measured: the far reflection against everything arriving, which
        // includes the near one and is dominated by it.
        assert!(
            found.strength > 0.2,
            "found it, but only {:.2} of what arrives",
            found.strength
        );
    }

    #[test]
    fn the_finder_is_not_distracted_by_the_hybrid() {
        // The near echo is the loudest thing on the line during training and
        // would win every search that could see it. It is also already
        // covered, so the search starts past it.
        let mut path = Path::new(split_path(300));
        let mut finder = EchoFinder::new(64, 512);
        for &x in &noise(40_000) {
            let heard = path.echo(x);
            finder.feed(x, heard);
        }
        assert_eq!(finder.best().map(|f| f.delay), Some(300));
    }

    #[test]
    fn a_silent_line_gives_the_finder_nothing_to_report() {
        let mut finder = EchoFinder::new(16, 64);
        for _ in 0..1000 {
            finder.feed(0.0, 0.0);
        }
        assert_eq!(finder.best(), None);
    }

    #[test]
    fn placing_the_far_taps_keeps_what_the_near_ones_learned() {
        // This happens partway through the training segment, which is the only
        // stretch of the start-up quiet enough to learn anything in. Throwing
        // away the near model to make room for the far one would spend half
        // that stretch twice.
        let near = vec![0.0, 0.0, 0.25, 0.1, -0.05];
        let mut path = Path::new(near.clone());
        let mut ec = EchoCanceller::new(16, 0.5);
        let sent = noise(40_000);
        for &x in &sent {
            let heard = path.echo(x);
            ec.process(x, heard);
        }
        let before = ec.echo_return_loss();
        assert!(before > 40.0, "did not converge: {before:.1} dB");

        ec.watch_far_echo(400, 32);
        ec.set_adapting(false);
        ec.reset_meters();
        let mut path = Path::new(near);
        for &x in &sent[..4_000] {
            let heard = path.echo(x);
            ec.process(x, heard);
        }
        let after = ec.echo_return_loss();
        assert!(
            after > before - 3.0,
            "the near taps went from {before:.1} dB to {after:.1} dB just by \
             putting a second run behind them"
        );
        assert_eq!(ec.far_echo(), Some((400, 32)));
        assert_eq!(ec.span(), 432);
    }
}
