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

use std::collections::VecDeque;

/// Adaptive canceller for a modem's own echo.
///
/// Adapts by normalised least mean squares: each sample, the taps move along
/// the reference in proportion to what is left over. Normalising by the power
/// of the reference is what makes the step size mean the same thing at every
/// signal level, so one setting works on a loud line and a quiet one.
#[derive(Debug, Clone)]
pub struct EchoCanceller {
    /// The estimated echo path, most recent sample first.
    taps: Vec<f64>,
    /// What we transmitted, most recent first, the same length as the taps.
    history: VecDeque<f64>,
    step: f64,
    /// Energy currently inside the filter, kept exactly rather than averaged.
    energy: f64,
    adapting: bool,
    /// Running powers of what arrived and what is left, for the return loss.
    heard: f64,
    residue: f64,
}

/// Guards the division when the line is silent.
const FLOOR: f64 = 1.0e-9;

/// How quickly the return-loss meters follow the signal. About a millisecond
/// at the rates used here, which is long enough to average a symbol and short
/// enough to follow a signal starting.
const POWER_TRACK: f64 = 0.01;

impl EchoCanceller {
    /// `taps` should span the whole echo path in samples, delay included.
    ///
    /// Too short and the tail it cannot reach is left uncancelled; too long
    /// and every extra tap adds its own adaptation noise while modelling
    /// nothing. The near echo of a hybrid arrives within a millisecond or two.
    /// A network reflection can be tens of milliseconds behind, and is what
    /// sets the length.
    ///
    /// `step` between 0 and 2 is stable, but only in theory and only without
    /// noise. Something around a tenth converges in a few thousand samples and
    /// leaves the taps quiet once it has.
    pub fn new(taps: usize, step: f64) -> Self {
        Self {
            taps: vec![0.0; taps],
            history: VecDeque::from(vec![0.0; taps]),
            step,
            energy: 0.0,
            adapting: true,
            heard: 0.0,
            residue: 0.0,
        }
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
        // Keep the energy in the filter exactly, by adding what came in and
        // taking off what fell out the end.
        //
        // A running average of the reference power will not do here, however
        // slowly it moves. It is what the step is divided by, so whenever it
        // reads low the step comes out large, and a signal that is not white
        // spends much of its time away from its own average. Filtering noise
        // to a thousand hertz was enough: the canceller diverged completely,
        // and reported a return loss of minus two hundred decibels.
        let dropped = self.history.pop_back().unwrap_or(0.0);
        self.history.push_front(transmitted);
        self.energy += transmitted * transmitted - dropped * dropped;
        self.energy = self.energy.max(0.0);

        let echo: f64 = self
            .taps
            .iter()
            .zip(self.history.iter())
            .map(|(t, x)| t * x)
            .sum();
        let left = received - echo;

        // Track what arrived and what is left, for the return loss.
        self.heard += POWER_TRACK * (received * received - self.heard);
        self.residue += POWER_TRACK * (left * left - self.residue);

        if self.adapting {
            // Normalised least mean squares. The gradient of the squared
            // residue with respect to each tap is the reference at that tap's
            // delay, so moving every tap along its own sample by the same
            // fraction of the residue reduces it.
            let gain = self.step * left / (self.energy + FLOOR);
            for (tap, x) in self.taps.iter_mut().zip(self.history.iter()) {
                *tap += gain * x;
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
        self.taps.iter_mut().for_each(|t| *t = 0.0);
        self.history.iter_mut().for_each(|x| *x = 0.0);
        self.energy = 0.0;
        self.heard = 0.0;
        self.residue = 0.0;
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
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
        assert!(ec.taps.iter().all(|t| t.abs() < 1.0e-12));
    }
}
